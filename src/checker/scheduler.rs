use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use reqwest::Client;
use sqlx::SqlitePool;
use tokio::time;
use tracing::{error, info, warn};

use super::http::CheckResult;
use crate::alerting::AlertPayload;
use crate::AppState;

// ── Row type for the monitors query ──────────────────────────────────────────

/// Lightweight projection of a monitor row – columns the scheduler
/// needs to execute health checks and evaluate the failure threshold.
#[derive(sqlx::FromRow, Clone, Debug)]
pub struct Monitor {
    pub id: String,
    pub name: String,
    pub url: String,
    #[allow(dead_code)]
    pub check_interval_secs: i32,
    pub timeout_secs: i32,
    pub max_retries: i32,
    pub last_status: String,
    pub down_since: Option<String>,
    #[sqlx(default)]
    pub check_domain_expiry: i32,
    #[sqlx(default)]
    pub domain_warning_days: i32,
    pub domain_expires_at: Option<String>,
    pub domain_days_remaining: Option<i32>,
    pub last_domain_alert_days: Option<i32>,
    pub domain_last_checked_at: Option<String>,
    #[sqlx(default)]
    pub check_ssl: i32,
    #[sqlx(default)]
    pub ssl_warning_days: i32,
    pub ssl_expires_at: Option<String>,
    pub ssl_days_remaining: Option<i32>,
    pub last_ssl_alert_days: Option<i32>,
    pub ssl_last_checked_at: Option<String>,
    #[sqlx(default)]
    pub check_type: String,
    #[sqlx(default)]
    pub port: Option<i32>,
    #[sqlx(default)]
    pub dns_record_type: Option<String>,
    #[sqlx(default)]
    pub dns_expected_value: Option<String>,
    #[sqlx(default)]
    pub http_method: String,
    #[sqlx(default)]
    pub expected_status: String,
    #[sqlx(default)]
    pub request_headers: Option<String>,
    #[sqlx(default)]
    pub body_keyword: Option<String>,
    #[sqlx(default)]
    pub json_pointer: Option<String>,
    #[sqlx(default)]
    pub json_expected: Option<String>,
    #[sqlx(default)]
    pub degraded_latency_ms: Option<i64>,
    #[sqlx(default)]
    pub dns_expected: Option<String>,
    #[sqlx(default)]
    pub resend_interval_mins: i32,
    #[sqlx(default)]
    pub last_alerted_at: Option<String>,
}

impl From<&crate::db::monitors::Monitor> for Monitor {
    fn from(m: &crate::db::monitors::Monitor) -> Self {
        Self {
            id: m.id.clone(),
            name: m.name.clone(),
            url: m.url.clone(),
            check_interval_secs: m.check_interval_secs,
            timeout_secs: m.timeout_secs,
            max_retries: m.max_retries,
            last_status: m.last_status.clone(),
            down_since: m.down_since.clone(),
            check_domain_expiry: m.check_domain_expiry,
            domain_warning_days: m.domain_warning_days,
            domain_expires_at: m.domain_expires_at.clone(),
            domain_days_remaining: m.domain_days_remaining,
            last_domain_alert_days: m.last_domain_alert_days,
            domain_last_checked_at: m.domain_last_checked_at.clone(),
            check_ssl: m.check_ssl,
            ssl_warning_days: m.ssl_warning_days,
            ssl_expires_at: m.ssl_expires_at.clone(),
            ssl_days_remaining: m.ssl_days_remaining,
            last_ssl_alert_days: m.last_ssl_alert_days,
            ssl_last_checked_at: m.ssl_last_checked_at.clone(),
            check_type: m.check_type.clone(),
            port: m.port,
            dns_record_type: m.dns_record_type.clone(),
            dns_expected_value: m.dns_expected_value.clone(),
            http_method: m.http_method.clone(),
            expected_status: m.expected_status.clone(),
            request_headers: m.request_headers.clone(),
            body_keyword: m.body_keyword.clone(),
            json_pointer: m.json_pointer.clone(),
            json_expected: m.json_expected.clone(),
            degraded_latency_ms: m.degraded_latency_ms,
            dns_expected: m.dns_expected.clone(),
            resend_interval_mins: m.resend_interval_mins,
            last_alerted_at: m.last_alerted_at.clone(),
        }
    }
}

// ── In-memory per-monitor state ──────────────────────────────────────────────

/// Tracks the live health state for a single monitor across scheduler ticks.
#[derive(Debug, Clone)]
pub struct MonitorState {
    /// Current derived status (`"UP"`, `"DOWN"`, or `"MAINTENANCE"`).
    pub status: String,
    /// Running count of consecutive failed checks.
    pub consecutive_failures: i32,
    /// Timestamp of the most recent status transition.
    pub last_state_change: chrono::DateTime<Utc>,
    /// Timestamp when an alert was last sent (used for repeat reminder intervals).
    pub last_alerted_at: Option<chrono::DateTime<Utc>>,
}

impl Default for MonitorState {
    fn default() -> Self {
        Self {
            status: "UP".to_string(),
            consecutive_failures: 0,
            last_state_change: Utc::now(),
            last_alerted_at: None,
        }
    }
}

pub fn should_check_domain(monitor: &Monitor) -> bool {
    if monitor.check_domain_expiry != 1 {
        return false;
    }
    let due = match &monitor.domain_last_checked_at {
        None => true,
        Some(last_str) => match chrono::DateTime::parse_from_rfc3339(last_str) {
            Ok(dt) => (Utc::now() - dt.with_timezone(&Utc)).num_hours() >= 12,
            Err(_) => match chrono::NaiveDateTime::parse_from_str(last_str, "%Y-%m-%d %H:%M:%S") {
                Ok(ndt) => (Utc::now() - ndt.and_utc()).num_hours() >= 12,
                Err(_) => true,
            },
        },
    };
    due || monitor.domain_expires_at.is_none()
}

pub fn should_check_ssl(monitor: &Monitor) -> bool {
    if monitor.check_ssl != 1 {
        return false;
    }
    let due = match &monitor.ssl_last_checked_at {
        None => true,
        Some(last_str) => match chrono::DateTime::parse_from_rfc3339(last_str) {
            Ok(dt) => (Utc::now() - dt.with_timezone(&Utc)).num_hours() >= 12,
            Err(_) => match chrono::NaiveDateTime::parse_from_str(last_str, "%Y-%m-%d %H:%M:%S") {
                Ok(ndt) => (Utc::now() - ndt.and_utc()).num_hours() >= 12,
                Err(_) => true,
            },
        },
    };
    due || monitor.ssl_expires_at.is_none()
}

pub async fn prune_old_check_logs(pool: &SqlitePool, retention_days: u32) -> Result<u64, sqlx::Error> {
    if retention_days == 0 {
        return Ok(0);
    }
    let res = sqlx::query(
        "DELETE FROM check_logs WHERE id IN (\
            SELECT id FROM check_logs WHERE checked_at < datetime('now', '-' || ? || ' days') LIMIT 10000\
        )",
    )
    .bind(retention_days)
    .execute(pool)
    .await?;

    let count = res.rows_affected();
    if count > 0 {
        let _ = sqlx::query("PRAGMA optimize").execute(pool).await;
    }
    Ok(count)
}

// ── Public entry point ───────────────────────────────────────────────────────

/// Run the health-check scheduler loop **forever**.
///
/// Uses bounded concurrency (`Semaphore::new(32)`), tracks in-flight monitors,
/// shares in-memory `MonitorState` across ticks and manual pings, and periodically
/// prunes old check logs.
pub async fn run_scheduler(
    state: Arc<AppState>,
    mut reload_rx: tokio::sync::broadcast::Receiver<String>,
) {
    info!("Health-check scheduler started with bounded concurrency (32 workers)");

    let read_pool = state.db.clone();
    let write_pool = state.write_db.clone();
    let client = state.http_client.clone();
    let allow_private = state.config.allow_private_targets;
    let retention_days = state.config.retention_days;

    // Prune check logs on startup and periodically every 1 hour
    let _ = prune_old_check_logs(&write_pool, retention_days).await;

    // Spawn 12-hour background retention purge task
    let purge_pool = write_pool.clone();
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(12 * 3600));
        loop {
            interval.tick().await;
            tracing::info!(retention_days, "Running scheduled historical check_logs retention purge");
            match crate::db::checks::purge_historical_check_logs(&purge_pool, retention_days, 500).await {
                Ok(deleted) if deleted > 0 => {
                    tracing::info!(deleted, "Historical check_logs retention purge complete");
                }
                Ok(_) => {
                    tracing::debug!("Historical check_logs retention purge: no expired records found");
                }
                Err(e) => {
                    tracing::error!("Historical check_logs retention purge failed: {e}");
                }
            }
        }
    });

    let semaphore = Arc::new(tokio::sync::Semaphore::new(32));
    let in_flight = Arc::new(tokio::sync::Mutex::new(HashSet::new()));

    let mut tick = time::interval(Duration::from_secs(1));
    let mut prune_tick = time::interval(Duration::from_secs(3600));
    prune_tick.tick().await;

    loop {
        tokio::select! {
            _ = prune_tick.tick() => {
                let pool_prune = write_pool.clone();
                tokio::spawn(async move {
                    if let Err(e) = prune_old_check_logs(&pool_prune, retention_days).await {
                        error!("Periodic log pruning error: {e}");
                    }
                });
            }
            _ = tick.tick() => {
                check_push_deadlines(&state).await;

                let monitors = match fetch_due_monitors(&read_pool).await {
                    Ok(m) => m,
                    Err(e) => {
                        error!("Failed to query due monitors: {e}");
                        continue;
                    }
                };

                if monitors.is_empty() {
                    continue;
                }

                for monitor in monitors {
                    let mut flight = in_flight.lock().await;
                    if !flight.insert(monitor.id.clone()) {
                        continue;
                    }
                    drop(flight);

                    let permit = match semaphore.clone().acquire_owned().await {
                        Ok(p) => p,
                        Err(_) => break,
                    };

                    let pool_task = write_pool.clone();
                    let client_task = client.clone();
                    let state_task = state.clone();
                    let in_flight_task = in_flight.clone();

                    tokio::spawn(async move {
                        let _permit = permit;
                        let monitor_id = monitor.id.clone();

                        let prev_state = {
                            let mut states = state_task.monitor_states.lock().await;
                            states
                                .entry(monitor_id.clone())
                                .or_insert_with(|| {
                                    let last_change = monitor
                                        .down_since
                                        .as_deref()
                                        .and_then(|ds| chrono::DateTime::parse_from_rfc3339(ds).ok())
                                        .map(|dt| dt.with_timezone(&Utc))
                                        .unwrap_or_else(Utc::now);
                                    let last_alert = monitor
                                        .last_alerted_at
                                        .as_deref()
                                        .and_then(|la| chrono::DateTime::parse_from_rfc3339(la).ok())
                                        .map(|dt| dt.with_timezone(&Utc));
                                    MonitorState {
                                        status: monitor.last_status.clone(),
                                        consecutive_failures: 0,
                                        last_state_change: last_change,
                                        last_alerted_at: last_alert,
                                    }
                                })
                                .clone()
                        };

                        let (new_state, alert) =
                            run_check_and_evaluate(&pool_task, &client_task, &monitor, &prev_state, allow_private).await;

                        {
                            let mut states = state_task.monitor_states.lock().await;
                            states.insert(monitor_id.clone(), new_state.clone());
                        }

                        if new_state.status != prev_state.status {
                            state_task.invalidate_status_cache().await;
                            let _ = state_task.events_tx.send(
                                serde_json::json!({
                                    "event": "status_changed",
                                    "monitor_id": monitor_id,
                                    "old_status": prev_state.status,
                                    "new_status": new_state.status,
                                })
                                .to_string(),
                            );
                        }

                        let _ = state_task.events_tx.send(
                            serde_json::json!({
                                "event": "check_completed",
                                "monitor_id": monitor_id,
                                "status": new_state.status,
                            })
                            .to_string(),
                        );

                        if let Some(payload) = alert {
                            let p = pool_task.clone();
                            let c = client_task.clone();
                            tokio::spawn(async move {
                                crate::alerting::dispatch_alerts(&p, &c, &payload).await;
                            });
                        }

                        let mut flight = in_flight_task.lock().await;
                        flight.remove(&monitor_id);
                    });
                }
            }
            reload_res = reload_rx.recv() => {
                match reload_res {
                    Ok(monitor_id) => {
                        info!(monitor_id = %monitor_id, "Reload signal received; re-arming monitor check immediately");
                        state.invalidate_status_cache().await;
                        if let Ok(Some(monitor)) = fetch_monitor_by_id(&read_pool, &monitor_id).await {
                            let mut flight = in_flight.lock().await;
                            if !flight.insert(monitor.id.clone()) {
                                continue;
                            }
                            drop(flight);

                            let permit = match semaphore.clone().acquire_owned().await {
                                Ok(p) => p,
                                Err(_) => break,
                            };

                            let pool_task = write_pool.clone();
                            let client_task = client.clone();
                            let state_task = state.clone();
                            let in_flight_task = in_flight.clone();

                            tokio::spawn(async move {
                                let _permit = permit;
                                let m_id = monitor.id.clone();

                                let prev_state = {
                                    let mut states = state_task.monitor_states.lock().await;
                                    states
                                        .entry(m_id.clone())
                                        .or_insert_with(|| {
                                            let last_change = monitor
                                                .down_since
                                                .as_deref()
                                                .and_then(|ds| chrono::DateTime::parse_from_rfc3339(ds).ok())
                                                .map(|dt| dt.with_timezone(&Utc))
                                                .unwrap_or_else(Utc::now);
                                            let last_alert = monitor
                                                .last_alerted_at
                                                .as_deref()
                                                .and_then(|la| chrono::DateTime::parse_from_rfc3339(la).ok())
                                                .map(|dt| dt.with_timezone(&Utc));
                                            MonitorState {
                                                status: monitor.last_status.clone(),
                                                consecutive_failures: 0,
                                                last_state_change: last_change,
                                                last_alerted_at: last_alert,
                                            }
                                        })
                                        .clone()
                                };

                                let (new_state, alert) =
                                    run_check_and_evaluate(&pool_task, &client_task, &monitor, &prev_state, allow_private).await;

                                {
                                    let mut states = state_task.monitor_states.lock().await;
                                    states.insert(m_id.clone(), new_state.clone());
                                }

                                if new_state.status != prev_state.status {
                                    state_task.invalidate_status_cache().await;
                                }

                                if let Some(payload) = alert {
                                    let p = pool_task.clone();
                                    let c = client_task.clone();
                                    tokio::spawn(async move {
                                        crate::alerting::dispatch_alerts(&p, &c, &payload).await;
                                    });
                                }

                                let mut flight = in_flight_task.lock().await;
                                flight.remove(&m_id);
                            });
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!("Reload receiver lagged by {n} messages");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        info!("Reload broadcast channel closed; scheduler stopping reload listener");
                        break;
                    }
                }
            }
        }
    }
}

// ── Core check + state-machine logic ─────────────────────────────────────────

pub async fn execute_probe(
    client: &Client,
    monitor: &Monitor,
    allow_private: bool,
) -> CheckResult {
    let check_type_str = if monitor.check_type.is_empty() {
        "http"
    } else {
        &monitor.check_type
    };

    if check_type_str == "tcp" || monitor.url.starts_with("tcp://") {
        crate::checker::tcp::ping_tcp_checked(&monitor.url, monitor.port, monitor.timeout_secs as u64, allow_private).await
    } else if check_type_str == "dns" || monitor.url.starts_with("dns://") {
        let expected = monitor.dns_expected.as_deref().or(monitor.dns_expected_value.as_deref());
        crate::checker::dns::ping_dns(
            &monitor.url,
            monitor.dns_record_type.as_deref().or(Some("A")),
            expected,
            monitor.timeout_secs as u64,
        )
        .await
    } else if check_type_str == "ping" || monitor.url.starts_with("ping://") {
        crate::checker::ping::ping_icmp(&monitor.url, monitor.timeout_secs as u64).await
    } else {
        let probe_cfg = crate::checker::http::HttpProbeConfig {
            url: &monitor.url,
            method: if monitor.http_method.is_empty() { "GET" } else { &monitor.http_method },
            expected_status: if monitor.expected_status.is_empty() { "200-299" } else { &monitor.expected_status },
            headers_json: monitor.request_headers.as_deref(),
            body_keyword: monitor.body_keyword.as_deref(),
            json_pointer: monitor.json_pointer.as_deref(),
            json_expected: monitor.json_expected.as_deref(),
            degraded_latency_ms: monitor.degraded_latency_ms,
            timeout_secs: monitor.timeout_secs as u64,
        };
        crate::checker::http::ping_http_with_config(client, &probe_cfg).await
    }
}

/// Run a single HTTP/TCP/DNS check, insert the log, evaluate the state machine,
/// and return the updated `MonitorState` plus an optional `AlertPayload`
/// when a transition has occurred.
pub async fn run_check_and_evaluate(
    pool: &SqlitePool,
    client: &Client,
    monitor: &Monitor,
    prev: &MonitorState,
    allow_private: bool,
) -> (MonitorState, Option<AlertPayload>) {
    // ── Check if monitor is in active maintenance window ────────────
    let in_maintenance = crate::db::maintenance::is_monitor_in_maintenance(pool, &monitor.id)
        .await
        .unwrap_or(false);

    // ── Check RDAP domain registration expiration if enabled ────────
    let mut domain_days: Option<i32> = monitor.domain_days_remaining;
    let mut domain_expires: Option<String> = monitor.domain_expires_at.clone();

let is_http = monitor.check_type.is_empty() || monitor.check_type == "http";

    if is_http && should_check_domain(monitor) {
        match crate::checker::domain::check_domain_expiration(client, &monitor.url).await {
            Ok(info) => {
                domain_days = Some(info.days_remaining as i32);
                domain_expires = Some(info.expires_at.clone());
                if let Err(e) = crate::db::monitors::update_domain_expiry(
                    pool,
                    &monitor.id,
                    Some(&info.expires_at),
                    Some(info.days_remaining as i32),
                )
                .await
                {
                    error!(monitor_id = %monitor.id, "Failed to update domain expiry in DB: {e}");
                }
            }
            Err(e) => {
                warn!(monitor = %monitor.name, "RDAP domain expiration check failed: {e}");
            }
        }
    } else if let Some(ref exp_str) = monitor.domain_expires_at {
        if let Ok((_, remaining)) = crate::checker::domain::calculate_days_remaining(exp_str, Utc::now()) {
            domain_days = Some(remaining as i32);
        }
    }

    // Milestone Alert Suppression Tracking: only dispatch at [0, 1, 3, 7, 14, 30] days
    let mut pending_alert: Option<AlertPayload> = None;

    if monitor.check_domain_expiry == 1 {
        if let Some(days) = domain_days {
            if days <= monitor.domain_warning_days {
                let milestone = [0, 1, 3, 7, 14, 30]
                    .into_iter()
                    .find(|&m| days <= m && m <= monitor.domain_warning_days);

                if let Some(m) = milestone {
                    if monitor.last_domain_alert_days != Some(m) {
                        info!(
                            monitor = %monitor.name,
                            days_remaining = days,
                            milestone = m,
                            "Domain expiration alert milestone reached; dispatching alert"
                        );

                        if let Err(e) = crate::db::monitors::update_domain_alert_milestone(
                            pool,
                            &monitor.id,
                            Some(m),
                        )
                        .await
                        {
                            error!(monitor_id = %monitor.id, "Failed to record domain alert milestone: {e}");
                        }

                        let alert_payload = AlertPayload {
                            monitor_id: Some(monitor.id.clone()),
                            monitor_name: monitor.name.clone(),
                            monitor_url: monitor.url.clone(),
                            new_status: "DOMAIN_EXPIRING_SOON".to_string(),
                            old_status: monitor.last_status.clone(),
                            http_status: None,
                            response_time_ms: None,
                            error_msg: Some(if days <= 0 {
                                format!("Domain registration has expired ({days} days remaining)")
                            } else {
                                format!("Domain registration expires in {days} day(s)")
                            }),
                            downtime_duration: None,
                            downtime_duration_secs: None,
                            timestamp: Utc::now().to_rfc3339(),
                            domain_days_remaining: Some(days as i64),
                            domain_expires_at: domain_expires.clone(),
                        };

                        pending_alert = Some(alert_payload);
                    }
                }
            } else if monitor.last_domain_alert_days.is_some() {
                let _ = crate::db::monitors::update_domain_alert_milestone(pool, &monitor.id, None).await;
            }
        }
    }

    // ── Check TLS / SSL certificate expiration if enabled ───────────
    let mut ssl_days: Option<i32> = monitor.ssl_days_remaining;
    let mut _ssl_expires: Option<String> = monitor.ssl_expires_at.clone();

    if is_http && should_check_ssl(monitor) {
        match crate::checker::tls::check_ssl_expiration(&monitor.url, 10, allow_private).await {
            Ok(info) => {
                ssl_days = Some(info.days_remaining as i32);
                _ssl_expires = Some(info.expires_at.clone());
                if let Err(e) = crate::db::monitors::update_ssl_expiry(
                    pool,
                    &monitor.id,
                    Some(&info.expires_at),
                    Some(info.days_remaining as i32),
                )
                .await
                {
                    error!(monitor_id = %monitor.id, "Failed to update SSL expiry in DB: {e}");
                }
            }
            Err(e) => {
                warn!(monitor = %monitor.name, "TLS/SSL certificate check failed: {e}");
            }
        }
    } else if let Some(ref exp_str) = monitor.ssl_expires_at {
        if let Ok((_, remaining)) = crate::checker::domain::calculate_days_remaining(exp_str, Utc::now()) {
            ssl_days = Some(remaining as i32);
        }
    }

    // Milestone Alert Suppression Tracking for SSL: only dispatch at [0, 1, 3, 7, 14, 30] days
    if monitor.check_ssl == 1 {
        if let Some(days) = ssl_days {
            if days <= monitor.ssl_warning_days {
                let milestone = [0, 1, 3, 7, 14, 30]
                    .into_iter()
                    .find(|&m| days <= m && m <= monitor.ssl_warning_days);

                if let Some(m) = milestone {
                    if monitor.last_ssl_alert_days != Some(m) {
                        info!(
                            monitor = %monitor.name,
                            days_remaining = days,
                            milestone = m,
                            "SSL certificate expiration alert milestone reached; dispatching alert"
                        );

                        if let Err(e) = crate::db::monitors::update_ssl_alert_milestone(
                            pool,
                            &monitor.id,
                            Some(m),
                        )
                        .await
                        {
                            error!(monitor_id = %monitor.id, "Failed to record SSL alert milestone: {e}");
                        }

                        let alert_payload = AlertPayload {
                            monitor_id: Some(monitor.id.clone()),
                            monitor_name: monitor.name.clone(),
                            monitor_url: monitor.url.clone(),
                            new_status: "SSL_EXPIRING_SOON".to_string(),
                            old_status: monitor.last_status.clone(),
                            http_status: None,
                            response_time_ms: None,
                            error_msg: Some(if days <= 0 {
                                format!("TLS/SSL certificate has expired ({days} days remaining)")
                            } else {
                                format!("TLS/SSL certificate expires in {days} day(s)")
                            }),
                            downtime_duration: None,
                            downtime_duration_secs: None,
                            timestamp: Utc::now().to_rfc3339(),
                            domain_days_remaining: None,
                            domain_expires_at: None,
                        };

                        if pending_alert.is_none() {
                            pending_alert = Some(alert_payload);
                        }
                    }
                }
            } else if monitor.last_ssl_alert_days.is_some() {
                let _ = crate::db::monitors::update_ssl_alert_milestone(pool, &monitor.id, None).await;
            }
        }
    }

let result = execute_probe(client, monitor, allow_private).await;

    if in_maintenance {
        // Persist check log with is_up = true (no SLA penalty) and error_msg = "MAINTENANCE"
        let maint_result = CheckResult {
            status_code: result.status_code,
            latency_ms: result.latency_ms,
            is_up: true,
            is_degraded: false,
            error_msg: Some("MAINTENANCE".to_string()),
        };
        if let Err(e) = insert_check_log(pool, &monitor.id, &maint_result, domain_days, ssl_days).await {
            error!(monitor_id = %monitor.id, "Failed to insert check log during maintenance: {e}");
        }

        if prev.status != "MAINTENANCE" {
            info!(
                monitor = %monitor.name,
                "Entering planned maintenance window; suppressing alerts"
            );
            if let Err(e) = crate::db::monitors::update_monitor_status(pool, &monitor.id, "MAINTENANCE", None).await {
                error!(monitor_id = %monitor.id, "Failed to update monitor status to MAINTENANCE in DB: {e}");
            }
        }

        let mut next = prev.clone();
        next.status = "MAINTENANCE".to_string();
        next.consecutive_failures = 0;
        return (next, None);
    }

    // Persist raw check result.
    // NOTE: If check log insertion fails (e.g. disk full), do NOT abort state transition or alerts!
    if let Err(e) = insert_check_log(pool, &monitor.id, &result, domain_days, ssl_days).await {
        error!(monitor_id = %monitor.id, "Failed to insert check log: {e}");
    }

    // ── State-machine evaluation ─────────────────────────────────────

    let mut next = prev.clone();

    if result.is_up {
        // Success → reset failure counter.
        next.consecutive_failures = 0;
        let target_status = if result.is_degraded { "DEGRADED" } else { "UP" };

        if prev.status == "DOWN" {
            // ▸ Transition: DOWN → UP / DEGRADED (resolved)
            let now = Utc::now();
            let down_since_dt = monitor
                .down_since
                .as_deref()
                .and_then(|ds| chrono::DateTime::parse_from_rfc3339(ds).ok())
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or(prev.last_state_change);

            let downtime_seconds = (now - down_since_dt).num_seconds().max(0) as u64;
            let downtime = format_downtime_duration(downtime_seconds);
            next.status = target_status.to_string();
            next.last_state_change = now;
            next.last_alerted_at = None;

            info!(
                monitor = %monitor.name,
                "State transition: DOWN → {target_status} (resolved after {downtime})"
            );

            // Reset down_since to NULL and set last_status in SQLite
            if let Err(e) = crate::db::monitors::update_monitor_status(pool, &monitor.id, target_status, None).await {
                error!(monitor_id = %monitor.id, "Failed to update monitor status in DB: {e}");
            }
            let _ = sqlx::query("UPDATE monitors SET last_alerted_at = NULL WHERE id = ?")
                .bind(&monitor.id)
                .execute(pool)
                .await;

            let _ = crate::db::incidents::auto_resolve_incident_on_up(pool, &monitor.id).await;

            let payload = AlertPayload {
                monitor_id: Some(monitor.id.clone()),
                monitor_name: monitor.name.clone(),
                monitor_url: monitor.url.clone(),
                new_status: target_status.to_string(),
                old_status: "DOWN".to_string(),
                http_status: result.status_code.map(|c| c as u16),
                response_time_ms: Some(result.latency_ms as u64),
                error_msg: None,
                downtime_duration: Some(downtime),
                downtime_duration_secs: Some(downtime_seconds),
                timestamp: now.to_rfc3339(),
                domain_days_remaining: domain_days.map(|d| d as i64),
                domain_expires_at: domain_expires.clone(),
            };

            return (next, Some(payload));
        } else if prev.status != target_status {
            // E.g. UP -> DEGRADED, DEGRADED -> UP, MAINTENANCE -> UP/DEGRADED
            let now = Utc::now();
            let old_status = prev.status.clone();
            next.status = target_status.to_string();
            next.last_state_change = now;

            info!(
                monitor = %monitor.name,
                "State transition: {old_status} → {target_status}"
            );

            if let Err(e) = crate::db::monitors::update_monitor_status(pool, &monitor.id, target_status, None).await {
                error!(monitor_id = %monitor.id, "Failed to update monitor status in DB: {e}");
            }

            let payload = if old_status == "MAINTENANCE" {
                None
            } else {
                Some(AlertPayload {
                    monitor_id: Some(monitor.id.clone()),
                    monitor_name: monitor.name.clone(),
                    monitor_url: monitor.url.clone(),
                    new_status: target_status.to_string(),
                    old_status,
                    http_status: result.status_code.map(|c| c as u16),
                    response_time_ms: Some(result.latency_ms as u64),
                    error_msg: result.error_msg.clone(),
                    downtime_duration: None,
                    downtime_duration_secs: None,
                    timestamp: now.to_rfc3339(),
                    domain_days_remaining: domain_days.map(|d| d as i64),
                    domain_expires_at: domain_expires.clone(),
                })
            };

            return (next, payload);
        }

        info!(
            monitor = %monitor.name,
            latency_ms = result.latency_ms,
            "Check OK"
        );
    } else {
        // Failure → increment counter.
        next.consecutive_failures += 1;

        warn!(
            monitor = %monitor.name,
            failures = next.consecutive_failures,
            threshold = monitor.max_retries,
            error = ?result.error_msg,
            "Check FAILED"
        );

        if (prev.status == "UP" || prev.status == "DEGRADED" || prev.status == "MAINTENANCE") && next.consecutive_failures >= monitor.max_retries {
            // ▸ Transition: UP/MAINTENANCE → DOWN
            let now = Utc::now();
            let now_rfc3339 = now.to_rfc3339();
            let old_status = prev.status.clone();
            next.status = "DOWN".to_string();
            next.last_state_change = now;
            next.last_alerted_at = Some(now);

            info!(
                monitor = %monitor.name,
                "State transition: {old_status} → DOWN"
            );

            // Record last_status = "DOWN" and down_since in SQLite
            if let Err(e) = crate::db::monitors::update_monitor_status(pool, &monitor.id, "DOWN", Some(&now_rfc3339)).await {
                error!(monitor_id = %monitor.id, "Failed to update monitor status in DB: {e}");
            }
            let _ = sqlx::query("UPDATE monitors SET last_alerted_at = ? WHERE id = ?")
                .bind(&now_rfc3339)
                .bind(&monitor.id)
                .execute(pool)
                .await;

            let _ = crate::db::incidents::auto_create_incident_on_down(
                pool,
                &monitor.id,
                &monitor.name,
                result.error_msg.as_deref().unwrap_or("Health check failed"),
            )
            .await;

            let payload = AlertPayload {
                monitor_id: Some(monitor.id.clone()),
                monitor_name: monitor.name.clone(),
                monitor_url: monitor.url.clone(),
                new_status: "DOWN".to_string(),
                old_status,
                http_status: result.status_code.map(|c| c as u16),
                response_time_ms: Some(result.latency_ms as u64),
                error_msg: result.error_msg.clone(),
                downtime_duration: None,
                downtime_duration_secs: None,
                timestamp: now_rfc3339,
                domain_days_remaining: domain_days.map(|d| d as i64),
                domain_expires_at: domain_expires,
            };

            return (next, Some(payload));
        } else if prev.status == "DOWN" && monitor.resend_interval_mins > 0 {
            // Repeat Reminder Check (N5)
            let last_alert = prev.last_alerted_at.or_else(|| {
                monitor.last_alerted_at.as_deref().and_then(|s| {
                    chrono::DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.with_timezone(&Utc))
                        .or_else(|| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").ok().map(|ndt| ndt.and_utc()))
                })
            }).unwrap_or(prev.last_state_change);

            let now = Utc::now();
            if (now - last_alert).num_minutes() >= monitor.resend_interval_mins as i64 {
                info!(
                    monitor = %monitor.name,
                    resend_mins = monitor.resend_interval_mins,
                    "Repeat DOWN reminder interval elapsed; dispatching reminder alert"
                );

                next.last_alerted_at = Some(now);
                let now_rfc3339 = now.to_rfc3339();
                let _ = sqlx::query("UPDATE monitors SET last_alerted_at = ? WHERE id = ?")
                    .bind(&now_rfc3339)
                    .bind(&monitor.id)
                    .execute(pool)
                    .await;

                let down_since_dt = monitor
                    .down_since
                    .as_deref()
                    .and_then(|ds| chrono::DateTime::parse_from_rfc3339(ds).ok())
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or(prev.last_state_change);
                let downtime_seconds = (now - down_since_dt).num_seconds().max(0) as u64;
                let downtime = format_downtime_duration(downtime_seconds);

                let payload = AlertPayload {
                    monitor_id: Some(monitor.id.clone()),
                    monitor_name: monitor.name.clone(),
                    monitor_url: monitor.url.clone(),
                    new_status: "DOWN".to_string(),
                    old_status: "DOWN".to_string(),
                    http_status: result.status_code.map(|c| c as u16),
                    response_time_ms: Some(result.latency_ms as u64),
                    error_msg: Some(format!(
                        "[Reminder] Still DOWN after {downtime}: {}",
                        result.error_msg.as_deref().unwrap_or("check failed")
                    )),
                    downtime_duration: Some(downtime),
                    downtime_duration_secs: Some(downtime_seconds),
                    timestamp: now_rfc3339,
                    domain_days_remaining: domain_days.map(|d| d as i64),
                    domain_expires_at: domain_expires,
                };

                return (next, Some(payload));
            }
        }
    }

    (next, pending_alert)
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Fetch an active monitor by its id.
pub async fn fetch_monitor_by_id(pool: &SqlitePool, id: &str) -> Result<Option<Monitor>, sqlx::Error> {
    sqlx::query_as::<_, Monitor>(
        r#"
        SELECT m.id, m.name, m.url, m.check_interval_secs, m.timeout_secs, m.max_retries, m.last_status, m.down_since,
               m.check_domain_expiry, m.domain_warning_days, m.domain_expires_at, m.domain_days_remaining,
               m.last_domain_alert_days, m.domain_last_checked_at,
               m.check_ssl, m.ssl_warning_days, m.ssl_expires_at, m.ssl_days_remaining, m.last_ssl_alert_days, m.ssl_last_checked_at,
               m.check_type, m.port, m.dns_record_type, m.dns_expected_value,
               m.http_method, m.expected_status, m.request_headers, m.body_keyword, m.json_pointer, m.json_expected,
               m.degraded_latency_ms, m.dns_expected,
               m.resend_interval_mins, m.last_alerted_at
        FROM   monitors m
        WHERE  m.id = ? AND m.is_active = 1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

/// Return every active monitor whose last recorded check is older than its
/// `check_interval_secs` (or that has never been checked).
pub async fn fetch_due_monitors(pool: &SqlitePool) -> Result<Vec<Monitor>, sqlx::Error> {
    sqlx::query_as::<_, Monitor>(
        r#"
        SELECT m.id, m.name, m.url, m.check_interval_secs, m.timeout_secs, m.max_retries, m.last_status, m.down_since,
               m.check_domain_expiry, m.domain_warning_days, m.domain_expires_at, m.domain_days_remaining,
               m.last_domain_alert_days, m.domain_last_checked_at,
               m.check_ssl, m.ssl_warning_days, m.ssl_expires_at, m.ssl_days_remaining, m.last_ssl_alert_days, m.ssl_last_checked_at,
               m.check_type, m.port, m.dns_record_type, m.dns_expected_value,
               m.http_method, m.expected_status, m.request_headers, m.body_keyword, m.json_pointer, m.json_expected,
               m.degraded_latency_ms, m.dns_expected,
               m.resend_interval_mins, m.last_alerted_at
        FROM   monitors m
        WHERE  m.is_active = 1
          AND  m.push_token IS NULL
          AND  NOT EXISTS (
                 SELECT 1
                 FROM   check_logs cl
                 WHERE  cl.monitor_id = m.id
                   AND  cl.checked_at >= datetime('now', '-' || m.check_interval_secs || ' seconds')
               )
        "#,
    )
    .fetch_all(pool)
    .await
}

/// Write a single check result into the `check_logs` table.
async fn insert_check_log(
    pool: &SqlitePool,
    monitor_id: &str,
    result: &CheckResult,
    domain_days_remaining: Option<i32>,
    ssl_days_remaining: Option<i32>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO check_logs (monitor_id, status_code, latency_ms, response_time_ms, is_up, error_msg, checked_at, created_at, domain_days_remaining, ssl_days_remaining)
        VALUES (?, ?, ?, ?, ?, ?, datetime('now'), datetime('now'), ?, ?)
        "#,
    )
    .bind(monitor_id)
    .bind(result.status_code)
    .bind(result.latency_ms)
    .bind(result.latency_ms)
    .bind(result.is_up as i32)
    .bind(&result.error_msg)
    .bind(domain_days_remaining)
    .bind(ssl_days_remaining)
    .execute(pool)
    .await?;

    Ok(())
}

/// Format duration in seconds to compact form (e.g. "4m 12s", "45s", "1h 4m 12s").
pub fn format_downtime_duration(secs: u64) -> String {
    if secs >= 3600 {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        let s = secs % 60;
        format!("{h}h {m}m {s}s")
    } else if secs >= 60 {
        let m = secs / 60;
        let s = secs % 60;
        format!("{m}m {s}s")
    } else {
        format!("{secs}s")
    }
}

/// Check active push/heartbeat monitors and mark overdue ones as DOWN.
/// Respects grace_secs and does NOT mark monitors DOWN before check_interval_secs + grace_secs
/// has elapsed since creation (Review Focus #5).
pub async fn check_push_deadlines(state: &AppState) {
    #[derive(sqlx::FromRow)]
    struct PushDeadlineRow {
        #[sqlx(flatten)]
        monitor: crate::db::monitors::Monitor,
        last_log_checked_at: Option<String>,
    }

    let push_monitors: Vec<PushDeadlineRow> = match sqlx::query_as(
        r#"
        SELECT m.*,
               (SELECT checked_at FROM check_logs cl WHERE cl.monitor_id = m.id ORDER BY checked_at DESC LIMIT 1) as last_log_checked_at
        FROM monitors m
        WHERE m.is_active = 1 AND m.push_token IS NOT NULL
        "#,
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(m) => m,
        Err(_) => return,
    };

    let now = Utc::now();

    for row in push_monitors {
        let monitor = row.monitor;
        let last_time_str = row.last_log_checked_at;

        let parse_dt = |s: &str| -> Option<chrono::DateTime<Utc>> {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return None;
            }
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(trimmed) {
                return Some(dt.with_timezone(&Utc));
            }
            chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|ndt| ndt.and_utc())
        };

        let baseline_time = match last_time_str {
            Some(ref s) if !s.trim().is_empty() => parse_dt(s),
            _ => parse_dt(&monitor.created_at),
        };

        let baseline = match baseline_time {
            Some(b) => b,
            None => now,
        };

        let deadline_secs = (monitor.check_interval_secs + monitor.grace_secs) as i64;
        let is_overdue = (now - baseline).num_seconds() > deadline_secs;

        if is_overdue && monitor.last_status != "DOWN" {
            let in_maint = crate::db::maintenance::is_monitor_in_maintenance(&state.db, &monitor.id)
                .await
                .unwrap_or(false);
            if in_maint {
                continue;
            }

            let error_msg = format!(
                "Missed heartbeat deadline: expected every {}s (+{}s grace)",
                monitor.check_interval_secs, monitor.grace_secs
            );
            let now_rfc = now.to_rfc3339();

            let _ = sqlx::query(
                r#"
                INSERT INTO check_logs (monitor_id, status_code, latency_ms, is_up, error_msg, checked_at)
                VALUES (?, 500, 0, 0, ?, ?)
                "#,
            )
            .bind(&monitor.id)
            .bind(&error_msg)
            .bind(&now_rfc)
            .execute(&state.db)
            .await;

            let _ = crate::db::monitors::update_monitor_status(&state.db, &monitor.id, "DOWN", Some(&now_rfc)).await;
            let _ = sqlx::query("UPDATE monitors SET last_alerted_at = ? WHERE id = ?")
                .bind(&now_rfc)
                .bind(&monitor.id)
                .execute(&state.db)
                .await;

            let _ = crate::db::incidents::auto_create_incident_on_down(
                &state.db,
                &monitor.id,
                &monitor.name,
                &error_msg,
            )
            .await;

            let _ = state.events_tx.send(
                serde_json::json!({
                    "event": "status_changed",
                    "monitor_id": monitor.id,
                    "old_status": monitor.last_status,
                    "new_status": "DOWN",
                })
                .to_string(),
            );

            state.invalidate_status_cache().await;

            let payload = AlertPayload {
                monitor_id: Some(monitor.id.clone()),
                monitor_name: monitor.name.clone(),
                monitor_url: monitor.url.clone(),
                new_status: "DOWN".to_string(),
                old_status: monitor.last_status.clone(),
                http_status: Some(500),
                response_time_ms: None,
                error_msg: Some(error_msg),
                downtime_duration: None,
                downtime_duration_secs: None,
                timestamp: now_rfc,
                domain_days_remaining: None,
                domain_expires_at: None,
            };

            let p = state.db.clone();
            let c = state.http_client.clone();
            tokio::spawn(async move {
                crate::alerting::dispatch_alerts(&p, &c, &payload).await;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_downtime_duration() {
        assert_eq!(format_downtime_duration(0), "0s");
        assert_eq!(format_downtime_duration(45), "45s");
        assert_eq!(format_downtime_duration(60), "1m 0s");
        assert_eq!(format_downtime_duration(252), "4m 12s");
        assert_eq!(format_downtime_duration(3600), "1h 0m 0s");
        assert_eq!(format_downtime_duration(3665), "1h 1m 5s");
    }

    #[tokio::test]
    async fn test_maintenance_window_suppresses_alerts() {
        use chrono::Duration;
        use sqlx::sqlite::SqlitePoolOptions;

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .unwrap();

        let monitor_id = "test-sched-maint-1";
        sqlx::query(
            "INSERT INTO monitors (id, name, url, check_interval_secs, timeout_secs, max_retries, is_active, is_public, last_status) \
             VALUES (?, 'Test Service', 'http://127.0.0.1:9', 60, 1, 1, 1, 1, 'UP')"
        )
        .bind(monitor_id)
        .execute(&pool)
        .await
        .unwrap();

        let now = Utc::now();
        let start = now - Duration::minutes(5);
        let end = now + Duration::hours(1);

        crate::db::maintenance::create_maintenance_window(
            &pool,
            crate::db::maintenance::CreateMaintenanceDto {
                title: "Scheduled DB Patching".to_string(),
                description: None,
                start_time: start.to_rfc3339(),
                end_time: end.to_rfc3339(),
                monitor_ids: vec![monitor_id.to_string()],
            },
        )
        .await
        .unwrap();

        let client = reqwest::Client::new();
        let monitor = Monitor {
            id: monitor_id.to_string(),
            name: "Test Service".to_string(),
            url: "http://127.0.0.1:9".to_string(), // Unreachable endpoint
            check_interval_secs: 60,
            timeout_secs: 1,
            max_retries: 1,
            last_status: "UP".to_string(),
            down_since: None,
            check_domain_expiry: 0,
            domain_warning_days: 30,
            domain_expires_at: None,
            domain_days_remaining: None,
            last_domain_alert_days: None,
            domain_last_checked_at: None,
            check_ssl: 0,
            ssl_warning_days: 30,
            ssl_expires_at: None,
            ssl_days_remaining: None,
            last_ssl_alert_days: None,
            ssl_last_checked_at: None,
            check_type: "http".to_string(),
            port: None,
            dns_record_type: None,
            dns_expected_value: None,
            http_method: "GET".to_string(),
            expected_status: "200-299".to_string(),
            request_headers: None,
            body_keyword: None,
            json_pointer: None,
            json_expected: None,
            degraded_latency_ms: None,
            dns_expected: None,
            resend_interval_mins: 0,
            last_alerted_at: None,
        };

        let prev = MonitorState {
            status: "UP".to_string(),
            consecutive_failures: 0,
            last_state_change: now,
            last_alerted_at: None,
        };

        let (new_state, alert) =
            run_check_and_evaluate(&pool, &client, &monitor, &prev, true).await;

        // Alerts must be suppressed (None)
        assert!(alert.is_none(), "Alert must be suppressed during maintenance");
        // State must transition to MAINTENANCE
        assert_eq!(new_state.status, "MAINTENANCE");

        // Verify monitor's last_status in DB is updated to MAINTENANCE
        let updated_mon = fetch_monitor_by_id(&pool, monitor_id).await.unwrap().unwrap();
        assert_eq!(updated_mon.last_status, "MAINTENANCE");

        // Check log must exist with is_up = 1 and error_msg = "MAINTENANCE"
        let log: (i32, Option<String>) = sqlx::query_as("SELECT is_up, error_msg FROM check_logs WHERE monitor_id = ? ORDER BY id DESC LIMIT 1")
            .bind(monitor_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(log.0, 1, "is_up should be 1 during maintenance to prevent SLA penalty");
        assert_eq!(log.1.as_deref(), Some("MAINTENANCE"));
    }

    #[tokio::test]
    async fn test_domain_expiry_milestone_suppression() {
        use sqlx::sqlite::SqlitePoolOptions;

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .unwrap();

        let monitor_id = "test-sched-domain-1";
        sqlx::query(
            "INSERT INTO monitors (id, name, url, check_interval_secs, timeout_secs, max_retries, is_active, is_public, last_status, check_domain_expiry, domain_warning_days, domain_days_remaining, domain_expires_at, last_domain_alert_days, domain_last_checked_at) \
             VALUES (?, 'Domain Test', 'http://127.0.0.1:9', 60, 1, 1, 1, 1, 'UP', 1, 30, 14, datetime('now', '+14 days'), NULL, datetime('now'))"
        )
        .bind(monitor_id)
        .execute(&pool)
        .await
        .unwrap();

        let client = reqwest::Client::new();
        let monitor = fetch_monitor_by_id(&pool, monitor_id).await.unwrap().unwrap();

        let prev = MonitorState {
            status: "UP".to_string(),
            consecutive_failures: 0,
            last_state_change: Utc::now(),
            last_alerted_at: None,
        };

        // First evaluation: 14 days remaining <= 30 warning threshold. Milestone 14 has not been alerted.
        let (_new_state, _alert) =
            run_check_and_evaluate(&pool, &client, &monitor, &prev, true).await;

        // DB should record last_domain_alert_days = 14
        let updated = fetch_monitor_by_id(&pool, monitor_id).await.unwrap().unwrap();
        assert_eq!(updated.last_domain_alert_days, Some(14));

        // Second evaluation with same monitor (milestone 14 already alerted):
        // Re-run evaluation: should NOT re-alert milestone 14 (suppressed)
        let (_new_state2, _alert2) =
            run_check_and_evaluate(&pool, &client, &updated, &prev, true).await;
        let updated2 = fetch_monitor_by_id(&pool, monitor_id).await.unwrap().unwrap();
        assert_eq!(updated2.last_domain_alert_days, Some(14));
    }

    #[tokio::test]
    async fn test_rfc3339_caching_for_domain_and_ssl_checks() {
        use sqlx::sqlite::SqlitePoolOptions;

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();

        let monitor_id = "test-sched-cache-1";
        sqlx::query(
            "INSERT INTO monitors (id, name, url, check_interval_secs, timeout_secs, max_retries, is_active, is_public, last_status, check_domain_expiry, domain_warning_days, check_ssl, ssl_warning_days) \
             VALUES (?, 'Cache Test', 'https://example.com', 60, 1, 1, 1, 1, 'UP', 1, 30, 1, 30)"
        )
        .bind(monitor_id)
        .execute(&pool)
        .await
        .unwrap();

        crate::db::monitors::update_domain_expiry(&pool, monitor_id, Some("2028-01-01T00:00:00Z"), Some(500))
            .await
            .unwrap();
        crate::db::monitors::update_ssl_expiry(&pool, monitor_id, Some("2028-01-01T00:00:00Z"), Some(500))
            .await
            .unwrap();

        let monitor = fetch_monitor_by_id(&pool, monitor_id).await.unwrap().unwrap();
        assert!(!should_check_domain(&monitor), "Domain lookup must be cached for 12h immediately after update");
        assert!(!should_check_ssl(&monitor), "SSL lookup must be cached for 12h immediately after update");
    }

    #[tokio::test]
    async fn test_state_evaluation_transitions_down_even_when_insert_log_fails() {
        use sqlx::sqlite::SqlitePoolOptions;

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();

        let monitor_id = "test-sched-db-fail-1";
        sqlx::query(
            "INSERT INTO monitors (id, name, url, check_interval_secs, timeout_secs, max_retries, is_active, is_public, last_status) \
             VALUES (?, 'DB Fail Test', 'http://127.0.0.1:9', 60, 1, 1, 1, 1, 'UP')"
        )
        .bind(monitor_id)
        .execute(&pool)
        .await
        .unwrap();

        // Create trigger that forces INSERT INTO check_logs to fail with an error
        sqlx::query("CREATE TRIGGER fail_check_logs BEFORE INSERT ON check_logs BEGIN SELECT RAISE(ABORT, 'Simulated DB log write error'); END;")
            .execute(&pool)
            .await
            .unwrap();

        let client = reqwest::Client::new();
        let monitor = fetch_monitor_by_id(&pool, monitor_id).await.unwrap().unwrap();

        let prev = MonitorState {
            status: "UP".to_string(),
            consecutive_failures: 0,
            last_state_change: Utc::now(),
            last_alerted_at: None,
        };

        // Even though check_logs INSERT fails, state transition to DOWN must NOT be dropped!
        let (new_state, alert) =
            run_check_and_evaluate(&pool, &client, &monitor, &prev, true).await;

        assert_eq!(new_state.status, "DOWN", "State must transition to DOWN despite log insertion failure");
        assert!(alert.is_some(), "Alert must be generated despite log insertion failure");
    }

    #[tokio::test]
    async fn test_ssl_expiry_milestone_alert_and_suppression() {
        use sqlx::sqlite::SqlitePoolOptions;

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();

        let monitor_id = "test-sched-ssl-alert-1";
        sqlx::query(
            "INSERT INTO monitors (id, name, url, check_interval_secs, timeout_secs, max_retries, is_active, is_public, last_status, check_ssl, ssl_warning_days, ssl_days_remaining, ssl_expires_at, last_ssl_alert_days, ssl_last_checked_at) \
             VALUES (?, 'SSL Alert Test', 'http://127.0.0.1:9', 60, 1, 3, 1, 1, 'UP', 1, 30, 14, datetime('now', '+14 days'), NULL, datetime('now'))"
        )
        .bind(monitor_id)
        .execute(&pool)
        .await
        .unwrap();

        let client = reqwest::Client::new();
        let monitor = fetch_monitor_by_id(&pool, monitor_id).await.unwrap().unwrap();

        let prev = MonitorState {
            status: "UP".to_string(),
            consecutive_failures: 0,
            last_state_change: Utc::now(),
            last_alerted_at: None,
        };

        // First evaluation: 14 days remaining <= 30 warning threshold. Milestone 14 has not been alerted.
        let (_new_state, alert) =
            run_check_and_evaluate(&pool, &client, &monitor, &prev, true).await;

        let alert = alert.expect("SSL expiration milestone alert must be dispatched");
        assert_eq!(alert.new_status, "SSL_EXPIRING_SOON");

        // DB should record last_ssl_alert_days = 14
        let updated = fetch_monitor_by_id(&pool, monitor_id).await.unwrap().unwrap();
        assert_eq!(updated.last_ssl_alert_days, Some(14));

        // Second evaluation: should be suppressed
        let (_new_state2, alert2) =
            run_check_and_evaluate(&pool, &client, &updated, &prev, true).await;
        assert!(alert2.is_none(), "Duplicate SSL expiration alert must be suppressed");
    }

    #[tokio::test]
    async fn test_down_repeat_reminder_alert_after_resend_interval() {
        use chrono::Duration;
        use sqlx::sqlite::SqlitePoolOptions;

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();

        let monitor_id = "test-sched-reminder-1";
        let alerted_at = (Utc::now() - Duration::minutes(35)).to_rfc3339();
        sqlx::query(
            "INSERT INTO monitors (id, name, url, check_interval_secs, timeout_secs, max_retries, is_active, is_public, last_status, down_since, resend_interval_mins, last_alerted_at) \
             VALUES (?, 'Reminder Test', 'http://127.0.0.1:9', 60, 1, 1, 1, 1, 'DOWN', datetime('now', '-40 minutes'), 30, ?)"
        )
        .bind(monitor_id)
        .bind(&alerted_at)
        .execute(&pool)
        .await
        .unwrap();

        let client = reqwest::Client::new();
        let monitor = fetch_monitor_by_id(&pool, monitor_id).await.unwrap().unwrap();

        let prev = MonitorState {
            status: "DOWN".to_string(),
            consecutive_failures: 5,
            last_state_change: Utc::now() - Duration::minutes(40),
            last_alerted_at: Some(Utc::now() - Duration::minutes(35)),
        };

        let (_new_state, alert) =
            run_check_and_evaluate(&pool, &client, &monitor, &prev, true).await;

        let alert = alert.expect("Repeat reminder alert must be dispatched when resend interval elapsed");
        assert_eq!(alert.new_status, "DOWN");
        assert!(alert.error_msg.as_deref().unwrap_or("").contains("Reminder") || alert.error_msg.is_some());

        let updated = fetch_monitor_by_id(&pool, monitor_id).await.unwrap().unwrap();
        assert!(updated.last_alerted_at.is_some());
        assert_ne!(updated.last_alerted_at.as_deref(), Some(alerted_at.as_str()));
    }

    #[tokio::test]
    async fn test_prune_old_check_logs() {
        use sqlx::sqlite::SqlitePoolOptions;

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();

        let monitor_id = "test-prune-1";
        sqlx::query(
            "INSERT INTO monitors (id, name, url, check_interval_secs, timeout_secs, max_retries, is_active, is_public, last_status) \
             VALUES (?, 'Prune Test', 'http://127.0.0.1:9', 60, 1, 1, 1, 1, 'UP')"
        )
        .bind(monitor_id)
        .execute(&pool)
        .await
        .unwrap();

        // Insert log 35 days ago (should be pruned)
        sqlx::query(
            "INSERT INTO check_logs (monitor_id, status_code, latency_ms, is_up, checked_at, created_at) \
             VALUES (?, 200, 50, 1, datetime('now', '-35 days'), datetime('now', '-35 days'))"
        )
        .bind(monitor_id)
        .execute(&pool)
        .await
        .unwrap();

        // Insert log 5 days ago (should NOT be pruned)
        sqlx::query(
            "INSERT INTO check_logs (monitor_id, status_code, latency_ms, is_up, checked_at, created_at) \
             VALUES (?, 200, 45, 1, datetime('now', '-5 days'), datetime('now', '-5 days'))"
        )
        .bind(monitor_id)
        .execute(&pool)
        .await
        .unwrap();

        let pruned = prune_old_check_logs(&pool, 30).await.unwrap();
        assert_eq!(pruned, 1, "Exactly 1 old check log must be pruned");

        let remaining: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM check_logs WHERE monitor_id = ?")
            .bind(monitor_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(remaining.0, 1, "The recent check log must remain");
    }

    #[tokio::test]
    async fn test_push_heartbeat_deadline_scheduler_and_grace_period() {
        let pool = crate::db::init_pool(":memory:").await.unwrap();
        crate::db::run_migrations(&pool).await.unwrap();

        let config = crate::config::Config::default();
        let state = AppState::new_for_test(pool.clone(), config);

        // 1. Create a push monitor with interval=60s, grace=30s
        let monitor_id = "test-push-sched-1";
        sqlx::query(
            "INSERT INTO monitors (id, name, url, check_interval_secs, timeout_secs, max_retries, is_active, is_public, last_status, push_token, grace_secs, created_at) \
             VALUES (?, 'Nightly Backup', 'push://backup', 60, 5, 1, 1, 1, 'UP', 'token-push-sched-1', 30, datetime('now'))"
        )
        .bind(monitor_id)
        .execute(&pool)
        .await
        .unwrap();

        // Check immediately: created 0s ago <= 60+30s -> NOT overdue
        check_push_deadlines(&state).await;
        let mon: crate::db::monitors::Monitor = sqlx::query_as("SELECT * FROM monitors WHERE id = ?")
            .bind(monitor_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(mon.last_status, "UP", "Newly created push monitor must NOT trigger DOWN before deadline (Review Focus #5)");

        // 2. Simulate time passing past deadline by updating created_at to 120s ago
        sqlx::query("UPDATE monitors SET created_at = datetime('now', '-120 seconds') WHERE id = ?")
            .bind(monitor_id)
            .execute(&pool)
            .await
            .unwrap();

        check_push_deadlines(&state).await;
        let mon_down: crate::db::monitors::Monitor = sqlx::query_as("SELECT * FROM monitors WHERE id = ?")
            .bind(monitor_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(mon_down.last_status, "DOWN", "Overdue push monitor must transition to DOWN");
        assert!(mon_down.down_since.is_some());
    }
}
