use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use axum_extra::extract::cookie::CookieJar;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::info;

use crate::api::dto::UpdateMonitorDto;
use crate::db::checks::{self, DetailCheckLog, LatencyPercentiles, UptimeSlas};
use crate::db::monitors::{self, HourlyLatency, Monitor};
use crate::error::AppError;
use crate::AppState;

// ── Monitor Admin Handlers ───────────────────────────────────────────────────

/// `PUT /api/monitors/:id`
///
/// Updates an existing monitor in SQLite and immediately notifies the background
/// scheduler over `state.reload_tx` to re-arm the health check interval.
pub async fn update_monitor(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(payload): Json<UpdateMonitorDto>,
) -> Result<Json<Monitor>, AppError> {
    payload.validate()?;

    let updated = monitors::update_monitor(&state.db, &id, payload).await?;

    // Signal the background scheduler to re-arm the monitor interval immediately.
    if let Err(err) = state.reload_tx.send(id.clone()) {
        tracing::debug!(
            monitor_id = %id,
            "Reload signal not received by active subscribers: {err}"
        );
    } else {
        info!(
            monitor_id = %id,
            "Dispatched reload signal for updated monitor"
        );
    }

    state.invalidate_status_cache().await;

    Ok(Json(updated))
}

/// `POST /api/v1/monitors/:id/clone` and `POST /api/monitors/:id/clone`
///
/// Clones an existing monitor, generating a new monitor record suffixed with " (Copy)"
/// and copying all protocol parameters, timers, and attached notification channels.
pub async fn clone_monitor(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<Monitor>), AppError> {
    let source = monitors::get_monitor_by_id(&state.db, &id).await?;
    let new_id = uuid::Uuid::new_v4().to_string();
    let new_name = format!("{} (Copy)", source.name);

    let push_token = if source.url.starts_with("push://") {
        Some(crate::api::push::generate_push_token())
    } else {
        None
    };

    sqlx::query(
        "INSERT INTO monitors (id, name, url, check_interval_secs, timeout_secs, \
                               max_retries, is_active, is_public, last_status, down_since, \
                               check_domain_expiry, domain_warning_days, check_ssl, ssl_warning_days, \
                               http_method, expected_status, request_headers, body_keyword, \
                               json_pointer, json_expected, degraded_latency_ms, push_token, \
                               grace_secs, dns_record_type, dns_expected, resend_interval_mins, \
                               check_type, port, dns_expected_value) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'UP', NULL, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&new_id)
    .bind(&new_name)
    .bind(&source.url)
    .bind(source.check_interval_secs)
    .bind(source.timeout_secs)
    .bind(source.max_retries)
    .bind(source.is_active)
    .bind(source.is_public)
    .bind(source.check_domain_expiry)
    .bind(source.domain_warning_days)
    .bind(source.check_ssl)
    .bind(source.ssl_warning_days)
    .bind(&source.http_method)
    .bind(&source.expected_status)
    .bind(&source.request_headers)
    .bind(&source.body_keyword)
    .bind(&source.json_pointer)
    .bind(&source.json_expected)
    .bind(source.degraded_latency_ms)
    .bind(&push_token)
    .bind(source.grace_secs)
    .bind(&source.dns_record_type)
    .bind(&source.dns_expected)
    .bind(source.resend_interval_mins)
    .bind(&source.check_type)
    .bind(source.port)
    .bind(&source.dns_expected_value)
    .execute(&state.write_db)
    .await?;

    // Copy attached notification channels
    let attached_channels = crate::db::notifications::get_channel_ids_for_monitor(&state.db, &id).await?;
    if !attached_channels.is_empty() {
        let _ = crate::db::notifications::set_monitor_channels(&state.write_db, &new_id, &attached_channels).await;
    }

    let cloned = monitors::get_monitor_by_id(&state.db, &new_id).await?;

    // Signal the background scheduler to start polling the cloned monitor
    let _ = state.reload_tx.send(new_id.clone());
    let _ = state.events_tx.send(
        serde_json::json!({
            "event": "monitor_created",
            "monitor_id": new_id,
        })
        .to_string(),
    );

    info!(
        source_id = %id,
        cloned_id = %new_id,
        "Successfully cloned monitor target"
    );

    state.invalidate_status_cache().await;

    Ok((StatusCode::CREATED, Json(cloned)))
}

// ── Monitor Telemetry & Detail Handlers ──────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorDetailInfo {
    pub id: String,
    pub name: String,
    pub url: String,
    pub method: String,
    pub status: String,
    pub is_active: bool,
    pub is_public: bool,
    pub check_interval_secs: i32,
    pub timeout_secs: i32,
    pub check_domain_expiry: bool,
    pub domain_warning_days: i32,
    pub domain_expires_at: Option<String>,
    pub domain_days_remaining: Option<i32>,
    pub check_ssl: bool,
    pub ssl_warning_days: i32,
    pub ssl_expires_at: Option<String>,
    pub ssl_days_remaining: Option<i32>,
    #[serde(default)]
    pub channel_ids: Vec<String>,
    pub check_type: String,
    pub port: Option<i32>,
    pub dns_record_type: Option<String>,
    pub dns_expected_value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetExpiryItem {
    pub monitor_id: String,
    pub name: String,
    pub url: String,
    pub ssl_enabled: bool,
    pub ssl_days_remaining: Option<i32>,
    pub ssl_expires_at: Option<String>,
    pub domain_enabled: bool,
    pub domain_days_remaining: Option<i32>,
    pub domain_expires_at: Option<String>,
    pub urgency: String, // "CRITICAL" | "WARNING" | "HEALTHY"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorDetailResponse {
    pub monitor: MonitorDetailInfo,
    pub slas: UptimeSlas,
    pub latency: LatencyPercentiles,
    pub hourly_latency: Vec<HourlyLatency>,
    pub logs: Vec<DetailCheckLog>,
}

pub(crate) fn sanitize_public_error(err: &str) -> String {
    if err == "MAINTENANCE" {
        return "MAINTENANCE".to_string();
    }
    if let Some(pos) = err.find("status code: ") {
        return format!("HTTP error: {}", &err[pos..]);
    }
    if err.contains("timed out") || err.contains("timeout") {
        return "Request timed out".to_string();
    }
    "Check failed".to_string()
}

/// `GET /api/admin/monitors/:id/details`
///
/// Fetches deep-dive diagnostic telemetry, SLA percentages, latency stats, and 50 recent check logs.
/// For public monitors, unauthenticated access is permitted. For private monitors, authentication is required.
pub async fn get_monitor_details(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<MonitorDetailResponse>, AppError> {
    let monitor = monitors::get_monitor_by_id(&state.db, &id).await?;

    let ver = state.current_session_version();
    let is_authenticated = {
        let has_cookie_auth = jar
            .get(crate::api::auth::SESSION_COOKIE_NAME)
            .and_then(|c| {
                crate::api::auth::validate_jwt(c.value(), &state.config.jwt_secret, ver).ok()
            })
            .is_some();

        let has_bearer_auth = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer "))
            .and_then(|t| crate::api::auth::validate_jwt(t, &state.config.jwt_secret, ver).ok())
            .is_some();

        has_cookie_auth || has_bearer_auth
    };

    if !is_authenticated && (monitor.is_public == 0 || monitor.is_active == 0) {
        return Err(AppError::NotFound(format!("Monitor {id} not found")));
    }

    let slas = checks::get_uptime_slas(&state.db, &id).await?;
    let latency = checks::get_latency_percentiles(&state.db, &id, 24).await?;
    let hourly_latency = monitors::get_24h_latency_history(&state.db, &id).await?;
    let mut logs = checks::get_recent_check_logs(&state.db, &id, 50).await?;

    if !is_authenticated {
        for log in &mut logs {
            if let Some(ref err) = log.error_message {
                log.error_message = Some(sanitize_public_error(err));
            }
        }
    }

    let channel_ids = if is_authenticated {
        crate::db::notifications::get_channel_ids_for_monitor(&state.db, &id)
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let info = MonitorDetailInfo {
        id: monitor.id.clone(),
        name: monitor.name.clone(),
        url: monitor.url.clone(),
        method: if monitor.http_method.is_empty() {
            "GET".to_string()
        } else {
            monitor.http_method.clone()
        },
        status: monitor.last_status.clone(),
        is_active: monitor.is_active == 1,
        is_public: monitor.is_public == 1,
        check_interval_secs: monitor.check_interval_secs,
        timeout_secs: monitor.timeout_secs,
        check_domain_expiry: monitor.check_domain_expiry == 1,
        domain_warning_days: monitor.domain_warning_days,
        domain_expires_at: monitor.domain_expires_at.clone(),
        domain_days_remaining: monitor.domain_days_remaining,
        check_ssl: monitor.check_ssl == 1,
        ssl_warning_days: monitor.ssl_warning_days,
        ssl_expires_at: monitor.ssl_expires_at.clone(),
        ssl_days_remaining: monitor.ssl_days_remaining,
        channel_ids,
        check_type: monitor.check_type.clone(),
        port: monitor.port,
        dns_record_type: monitor.dns_record_type.clone(),
        dns_expected_value: monitor.dns_expected_value.clone(),
    };

    Ok(Json(MonitorDetailResponse {
        monitor: info,
        slas,
        latency,
        hourly_latency,
        logs,
    }))
}

#[derive(Debug, Serialize)]
pub struct PingResponse {
    pub status: String,
    pub status_code: Option<i32>,
    pub latency_ms: i64,
}

/// `POST /api/admin/monitors/:id/ping`
///
/// Executes an immediate synthetic check out-of-band and persists the check log to SQLite.
pub async fn ping_monitor(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<PingResponse>, AppError> {
    let monitor = monitors::get_monitor_by_id(&state.db, &id).await?;

    let in_maintenance = crate::db::maintenance::is_monitor_in_maintenance(&state.db, &monitor.id)
        .await
        .unwrap_or(false);

    if monitor.url.starts_with("push://") {
        return Err(AppError::BadRequest(
            "Push heartbeat monitors cannot be probed outbound via manual ping".to_string(),
        ));
    }

    let sched_monitor: crate::checker::scheduler::Monitor = (&monitor).into();
    let result = crate::checker::scheduler::execute_probe(
        &state.http_client,
        &sched_monitor,
        state.config.allow_private_targets,
    )
    .await;

    let is_http = monitor.check_type.is_empty() || monitor.check_type == "http";

    let mut domain_days = monitor.domain_days_remaining;
    if is_http && monitor.check_domain_expiry == 1 {
        if let Ok(info) = crate::checker::domain::check_domain_expiration(&state.http_client, &monitor.url).await {
            domain_days = Some(info.days_remaining as i32);
            let _ = monitors::update_domain_expiry(
                &state.db,
                &monitor.id,
                Some(&info.expires_at),
                Some(info.days_remaining as i32),
            )
            .await;
        }
    }

    let mut ssl_days = monitor.ssl_days_remaining;
    if is_http && monitor.check_ssl == 1 {
        if let Ok(info) = crate::checker::tls::check_ssl_expiration(
            &monitor.url,
            monitor.timeout_secs as u64,
            state.config.allow_private_targets,
        )
        .await
        {
            ssl_days = Some(info.days_remaining as i32);
            let _ = monitors::update_ssl_expiry(
                &state.db,
                &monitor.id,
                Some(&info.expires_at),
                Some(info.days_remaining as i32),
            )
            .await;
        }
    }

    let error_msg_to_store = if in_maintenance {
        Some("MAINTENANCE".to_string())
    } else {
        result.error_msg.clone()
    };

    // Persist check log to SQLite
    sqlx::query(
        r#"
        INSERT INTO check_logs (monitor_id, status_code, latency_ms, response_time_ms, is_up, error_msg, checked_at, created_at, domain_days_remaining, ssl_days_remaining)
        VALUES (?, ?, ?, ?, ?, ?, datetime('now'), datetime('now'), ?, ?)
        "#,
    )
    .bind(&monitor.id)
    .bind(result.status_code)
    .bind(result.latency_ms)
    .bind(result.latency_ms)
    .bind(if in_maintenance { 1 } else { result.is_up as i32 })
    .bind(&error_msg_to_store)
    .bind(domain_days)
    .bind(ssl_days)
    .execute(&state.db)
    .await?;

    let (status_str, down_since) = if in_maintenance {
        ("MAINTENANCE", None)
    } else if result.is_up {
        if result.is_degraded {
            ("DEGRADED", None)
        } else {
            ("UP", None)
        }
    } else {
        // Preserves existing down_since if monitor was already DOWN
        let preserved = monitor
            .down_since
            .clone()
            .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
        ("DOWN", Some(preserved))
    };

    let _ = monitors::update_monitor_status(
        &state.db,
        &monitor.id,
        status_str,
        down_since.as_deref(),
    )
    .await;

    // Synchronize shared in-memory monitor_states
    {
        let mut states = state.monitor_states.lock().await;
        let entry = states.entry(monitor.id.clone()).or_insert_with(|| {
            crate::checker::scheduler::MonitorState {
                status: status_str.to_string(),
                consecutive_failures: if result.is_up || in_maintenance { 0 } else { 1 },
                last_state_change: chrono::Utc::now(),
                last_alerted_at: None,
            }
        });
        entry.status = status_str.to_string();
        if result.is_up || in_maintenance {
            entry.consecutive_failures = 0;
        } else {
            entry.consecutive_failures += 1;
        }
    }

    state.invalidate_status_cache().await;

    info!(
        monitor_id = %monitor.id,
        status = %status_str,
        latency_ms = result.latency_ms,
        "Executed manual on-demand ping"
    );

    Ok(Json(PingResponse {
        status: status_str.to_string(),
        status_code: result.status_code,
        latency_ms: result.latency_ms,
    }))
}

// ── Bulk Pause / Resume Handlers ─────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct BulkMonitorActionResponse {
    pub message: String,
    pub updated_count: u64,
    pub total_monitors: usize,
    pub active_monitors: usize,
    pub global_state: String,
}

/// `POST /api/admin/monitors/pause-all`
///
/// Disables active health checks across all monitors in SQLite.
pub async fn pause_all_monitors(
    State(state): State<Arc<AppState>>,
) -> Result<Json<BulkMonitorActionResponse>, AppError> {
    let updated = monitors::pause_all_monitors(&state.db).await?;
    let (global_state, active, total) = monitors::get_global_monitoring_state(&state.db).await?;

    info!(
        updated_count = updated,
        active_monitors = active,
        total_monitors = total,
        global_state = %global_state,
        "Executed bulk pause across all monitors"
    );

    state.status_cache.invalidate(None).await;

    Ok(Json(BulkMonitorActionResponse {
        message: format!("Successfully paused {updated} monitors"),
        updated_count: updated,
        total_monitors: total,
        active_monitors: active,
        global_state,
    }))
}

/// `POST /api/admin/monitors/resume-all`
///
/// Re-enables active health checks across all monitors and triggers immediate out-of-band checks.
pub async fn resume_all_monitors(
    State(state): State<Arc<AppState>>,
) -> Result<Json<BulkMonitorActionResponse>, AppError> {
    let updated = monitors::resume_all_monitors(&state.db).await?;
    let (global_state, active, total) = monitors::get_global_monitoring_state(&state.db).await?;

    // Signal scheduler to immediately re-evaluate resumed monitors
    let all_monitors = monitors::get_all_monitors(&state.db).await.unwrap_or_default();
    for monitor in all_monitors {
        let _ = state.reload_tx.send(monitor.id);
    }

    info!(
        updated_count = updated,
        active_monitors = active,
        total_monitors = total,
        global_state = %global_state,
        "Executed bulk resume across all monitors with out-of-band evaluation"
    );

    Ok(Json(BulkMonitorActionResponse {
        message: format!("Successfully resumed {updated} monitors"),
        updated_count: updated,
        total_monitors: total,
        active_monitors: active,
        global_state,
    }))
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum BulkMonitorsPayload {
    Object {
        #[serde(alias = "ids")]
        monitor_ids: Vec<String>,
    },
    Array(Vec<String>),
}

impl BulkMonitorsPayload {
    pub fn ids(&self) -> &[String] {
        match self {
            Self::Object { monitor_ids } => monitor_ids.as_slice(),
            Self::Array(ids) => ids.as_slice(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BulkBatchActionResponse {
    pub message: String,
    pub updated_count: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BulkDeleteResponse {
    pub message: String,
    pub deleted_monitors: u64,
    pub deleted_logs: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BulkPreviewResponse {
    pub monitor_count: usize,
    pub check_logs_count: usize,
}

/// `POST /api/admin/monitors/bulk-pause`
pub async fn bulk_pause_monitors(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<BulkMonitorsPayload>,
) -> Result<Json<BulkBatchActionResponse>, AppError> {
    let ids = payload.ids();
    let updated = monitors::bulk_pause_monitors(&state.db, ids).await?;
    info!(
        count = updated,
        "Executed bulk pause for selected monitors"
    );
    state.status_cache.invalidate(None).await;
    Ok(Json(BulkBatchActionResponse {
        message: format!("Successfully paused {updated} monitors"),
        updated_count: updated,
    }))
}

/// `POST /api/admin/monitors/bulk-resume`
pub async fn bulk_resume_monitors(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<BulkMonitorsPayload>,
) -> Result<Json<BulkBatchActionResponse>, AppError> {
    let ids = payload.ids();
    let updated = monitors::bulk_resume_monitors(&state.db, ids).await?;

    for id in ids {
        let _ = state.reload_tx.send(id.clone());
    }

    info!(
        count = updated,
        "Executed bulk resume for selected monitors"
    );
    state.status_cache.invalidate(None).await;
    Ok(Json(BulkBatchActionResponse {
        message: format!("Successfully resumed {updated} monitors"),
        updated_count: updated,
    }))
}

/// `POST /api/admin/monitors/bulk-delete`
pub async fn bulk_delete_monitors(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<BulkMonitorsPayload>,
) -> Result<Json<BulkDeleteResponse>, AppError> {
    let ids = payload.ids();
    let (deleted_mons, deleted_logs) = monitors::bulk_delete_monitors(&state.db, ids).await?;
    info!(
        monitors = deleted_mons,
        logs = deleted_logs,
        "Executed cascading bulk delete for selected monitors"
    );
    state.status_cache.invalidate(None).await;
    Ok(Json(BulkDeleteResponse {
        message: format!("Successfully deleted {deleted_mons} monitors and {deleted_logs} historical logs"),
        deleted_monitors: deleted_mons,
        deleted_logs,
    }))
}

/// `POST /api/admin/monitors/bulk-preview`
pub async fn bulk_preview_monitors(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<BulkMonitorsPayload>,
) -> Result<Json<BulkPreviewResponse>, AppError> {
    let ids = payload.ids();
    let (mon_count, logs_count) = monitors::get_bulk_monitors_stats(&state.db, ids).await?;
    Ok(Json(BulkPreviewResponse {
        monitor_count: mon_count,
        check_logs_count: logs_count,
    }))
}

// ── Asset Expiry Handlers ────────────────────────────────────────────────────

fn compute_urgency(
    ssl_days: Option<i32>,
    domain_days: Option<i32>,
    ssl_enabled: bool,
    domain_enabled: bool,
) -> String {
    let mut min_days: Option<i32> = None;
    if ssl_enabled {
        if let Some(d) = ssl_days {
            min_days = Some(min_days.map_or(d, |m| m.min(d)));
        }
    }
    if domain_enabled {
        if let Some(d) = domain_days {
            min_days = Some(min_days.map_or(d, |m| m.min(d)));
        }
    }

    match min_days {
        Some(days) if days <= 7 => "CRITICAL".to_string(),
        Some(days) if days <= 30 => "WARNING".to_string(),
        _ => "HEALTHY".to_string(),
    }
}

fn urgency_sort_key(item: &AssetExpiryItem) -> i32 {
    let mut min_val = i32::MAX;
    if item.ssl_enabled {
        if let Some(d) = item.ssl_days_remaining {
            min_val = min_val.min(d);
        }
    }
    if item.domain_enabled {
        if let Some(d) = item.domain_days_remaining {
            min_val = min_val.min(d);
        }
    }
    min_val
}

/// `GET /api/admin/assets/expiry`
///
/// Returns list of all active monitors with TLS or domain expiration tracking,
/// sorted by MIN(ssl_days_remaining, domain_days_remaining) ASC (expiring soonest first).
pub async fn get_asset_expiry(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<AssetExpiryItem>>, AppError> {
    let monitors = monitors::get_asset_expiry_monitors(&state.db).await?;

    let mut items = Vec::with_capacity(monitors.len());
    for m in monitors {
        let ssl_enabled = m.check_ssl == 1;
        let domain_enabled = m.check_domain_expiry == 1;
        let urgency = compute_urgency(
            m.ssl_days_remaining,
            m.domain_days_remaining,
            ssl_enabled,
            domain_enabled,
        );

        items.push(AssetExpiryItem {
            monitor_id: m.id,
            name: m.name,
            url: m.url,
            ssl_enabled,
            ssl_days_remaining: m.ssl_days_remaining,
            ssl_expires_at: m.ssl_expires_at,
            domain_enabled,
            domain_days_remaining: m.domain_days_remaining,
            domain_expires_at: m.domain_expires_at,
            urgency,
        });
    }

    items.sort_by(|a, b| {
        urgency_sort_key(a)
            .cmp(&urgency_sort_key(b))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok(Json(items))
}

/// `POST /api/admin/monitors/:id/refresh-assets`
///
/// Performs on-demand TLS and RDAP checks for a monitor, updates DB, and returns updated AssetExpiryItem.
pub async fn refresh_monitor_assets(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<AssetExpiryItem>, AppError> {
    let mut monitor = monitors::get_monitor_by_id(&state.db, &id).await?;

    if monitor.check_ssl == 1 {
        if let Ok(info) = crate::checker::tls::check_ssl_expiration(
            &monitor.url,
            monitor.timeout_secs as u64,
            state.config.allow_private_targets,
        )
        .await
        {
            let _ = monitors::update_ssl_expiry(
                &state.db,
                &monitor.id,
                Some(&info.expires_at),
                Some(info.days_remaining as i32),
            )
            .await;
            monitor.ssl_expires_at = Some(info.expires_at);
            monitor.ssl_days_remaining = Some(info.days_remaining as i32);
        }
    }

    if monitor.check_domain_expiry == 1 {
        if let Ok(info) = crate::checker::domain::check_domain_expiration(&state.http_client, &monitor.url).await {
            let _ = monitors::update_domain_expiry(
                &state.db,
                &monitor.id,
                Some(&info.expires_at),
                Some(info.days_remaining as i32),
            )
            .await;
            monitor.domain_expires_at = Some(info.expires_at);
            monitor.domain_days_remaining = Some(info.days_remaining as i32);
        }
    }

    let ssl_enabled = monitor.check_ssl == 1;
    let domain_enabled = monitor.check_domain_expiry == 1;
    let urgency = compute_urgency(
        monitor.ssl_days_remaining,
        monitor.domain_days_remaining,
        ssl_enabled,
        domain_enabled,
    );

    let item = AssetExpiryItem {
        monitor_id: monitor.id,
        name: monitor.name,
        url: monitor.url,
        ssl_enabled,
        ssl_days_remaining: monitor.ssl_days_remaining,
        ssl_expires_at: monitor.ssl_expires_at,
        domain_enabled,
        domain_days_remaining: monitor.domain_days_remaining,
        domain_expires_at: monitor.domain_expires_at,
        urgency,
    };

    Ok(Json(item))
}

// ── Password Admin Handlers ───────────────────────────────────────────────────

/// Request payload for changing the admin password.
#[derive(Debug, Deserialize)]
pub struct ChangePasswordPayload {
    pub current_password: String,
    pub new_password: String,
}

/// `POST /api/admin/change-password`
///
/// Changes the administrator password after validating the current password,
/// persists an Argon2id hash into `app_config`, increments `session_version`
/// to revoke existing JWTs (`SEC-06`), and rotates the session cookie.
pub async fn change_password(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Json(payload): Json<ChangePasswordPayload>,
) -> Result<(CookieJar, Json<Value>), AppError> {
    let active_password =
        crate::api::auth::get_active_admin_password(&state.db, &state.config.admin_password).await;

    if !crate::api::auth::verify_password(&payload.current_password, &active_password) {
        return Err(AppError::Unauthorized(
            "Invalid current password".to_string(),
        ));
    }

    let pw_len = payload.new_password.chars().count();
    if pw_len < 8 {
        return Err(AppError::BadRequest(
            "New password must be at least 8 characters long".to_string(),
        ));
    }
    if pw_len > 256 {
        return Err(AppError::BadRequest(
            "New password must not exceed 256 characters".to_string(),
        ));
    }

    let hashed = crate::api::auth::hash_password(&payload.new_password);
    sqlx::query(
        "INSERT INTO app_config (key, value) VALUES ('admin_password', $1) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(&hashed)
    .execute(&state.db)
    .await
    .map_err(AppError::Database)?;

    let new_ver = crate::api::auth::bump_session_version(&state).await?;
    let fresh_jwt = crate::api::auth::create_jwt(
        &state.config.admin_user,
        &state.config.jwt_secret,
        new_ver,
    )
    .map_err(|e| AppError::Internal(anyhow::anyhow!("JWT creation failed: {e}")))?;

    let jar = jar.add(crate::api::auth::build_session_cookie(fresh_jwt));

    info!("Administrator password updated and active sessions rotated");
    Ok((
        jar,
        Json(json!({
            "message": "Password updated successfully"
        })),
    ))
}

// ── Notification Channel Admin Handlers ─────────────────────────────────────

fn default_true_bool() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CreateChannelRequest {
    pub name: String,
    #[serde(rename = "type", alias = "provider")]
    pub channel_type: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub is_default: bool,
    #[serde(default = "default_true_bool")]
    pub is_enabled: bool,
    #[serde(default = "default_true_bool")]
    pub use_default_template: bool,
    #[serde(default = "default_true_bool")]
    pub notify_on_resolve: bool,
    #[serde(default, alias = "message_template")]
    pub custom_template: Option<String>,
    pub bot_name: Option<String>,
    pub avatar_url: Option<String>,
    pub telegram_bot_token: Option<String>,
    pub telegram_chat_id: Option<String>,
    pub telegram_thread_id: Option<String>,
}

impl CreateChannelRequest {
    pub fn validate(&self) -> Result<(), AppError> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err(AppError::BadRequest("Channel name cannot be empty".to_string()));
        }
        let ctype = self.channel_type.trim().to_lowercase();
        if !["discord", "slack", "webhook", "telegram", "generic", "generic_webhook"].contains(&ctype.as_str()) {
            return Err(AppError::BadRequest(
                "Invalid channel type. Supported types: discord, slack, webhook, telegram".to_string(),
            ));
        }

        if ctype == "telegram" {
            let has_token = self.telegram_bot_token.as_deref().map(str::trim).filter(|s| !s.is_empty()).is_some();
            let has_chat = self.telegram_chat_id.as_deref().map(str::trim).filter(|s| !s.is_empty()).is_some();
            let has_url = self.url.as_deref().map(str::trim).filter(|s| !s.is_empty()).is_some();

            if !has_url && (!has_token || !has_chat) {
                return Err(AppError::BadRequest(
                    "Telegram channel requires both Bot Token and Chat ID (or endpoint URL)".to_string(),
                ));
            }
            if let Some(ref u) = self.url {
                let trimmed = u.trim();
                if !trimmed.is_empty() && !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
                    return Err(AppError::BadRequest("Channel URL must begin with http:// or https://".to_string()));
                }
            }
        } else {
            let u = self.url.as_deref().map(str::trim).unwrap_or("");
            if !(u.starts_with("http://") || u.starts_with("https://")) {
                return Err(AppError::BadRequest("Channel URL must begin with http:// or https://".to_string()));
            }
        }

        if let Some(ref av) = self.avatar_url {
            let a = av.trim();
            if !a.is_empty() && !(a.starts_with("http://") || a.starts_with("https://")) {
                return Err(AppError::BadRequest("Avatar URL must begin with http:// or https://".to_string()));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UpdateChannelRequest {
    pub name: Option<String>,
    #[serde(rename = "type", alias = "provider")]
    pub channel_type: Option<String>,
    pub url: Option<String>,
    pub is_default: Option<bool>,
    pub is_enabled: Option<bool>,
    pub use_default_template: Option<bool>,
    pub notify_on_resolve: Option<bool>,
    #[serde(default, alias = "message_template")]
    pub custom_template: Option<String>,
    pub bot_name: Option<String>,
    pub avatar_url: Option<String>,
    pub telegram_bot_token: Option<String>,
    pub telegram_chat_id: Option<String>,
    pub telegram_thread_id: Option<String>,
}

impl UpdateChannelRequest {
    pub fn validate(&self) -> Result<(), AppError> {
        if let Some(ref name) = self.name {
            if name.trim().is_empty() {
                return Err(AppError::BadRequest("Channel name cannot be empty".to_string()));
            }
        }
        if let Some(ref url) = self.url {
            let u = url.trim();
            if !u.is_empty()
                && !crate::db::notifications::is_masked_url(u)
                && !(u.starts_with("http://") || u.starts_with("https://"))
            {
                return Err(AppError::BadRequest("Channel URL must begin with http:// or https://".to_string()));
            }
        }
        if let Some(ref ctype) = self.channel_type {
            let c = ctype.trim().to_lowercase();
            if !["discord", "slack", "webhook", "telegram", "generic", "generic_webhook"].contains(&c.as_str()) {
                return Err(AppError::BadRequest(
                    "Invalid channel type. Supported types: discord, slack, webhook, telegram".to_string(),
                ));
            }
        }
        if let Some(ref av) = self.avatar_url {
            let a = av.trim();
            if !a.is_empty() && !(a.starts_with("http://") || a.starts_with("https://")) {
                return Err(AppError::BadRequest("Avatar URL must begin with http:// or https://".to_string()));
            }
        }
        Ok(())
    }
}

fn normalize_channel_type(raw: &str) -> String {
    match raw.trim().to_lowercase().as_str() {
        "generic" | "generic_webhook" => "webhook".to_string(),
        other => other.to_string(),
    }
}

/// `GET /api/admin/channels` — List all saved notification channels (with masked URLs).
pub async fn list_channels(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<crate::db::notifications::NotificationChannel>>, AppError> {
    let channels = crate::db::notifications::list_channels(&state.db).await?;
    Ok(Json(channels.into_iter().map(|c| c.masked_view()).collect()))
}

/// `POST /api/admin/channels` — Create a reusable notification channel.
pub async fn create_channel(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateChannelRequest>,
) -> Result<(axum::http::StatusCode, Json<crate::db::notifications::NotificationChannel>), AppError> {
    payload.validate()?;

    let bot_name = payload
        .bot_name
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let avatar_url = payload
        .avatar_url
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let telegram_bot_token = payload
        .telegram_bot_token
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let telegram_chat_id = payload
        .telegram_chat_id
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let telegram_thread_id = payload
        .telegram_thread_id
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let ctype = payload.channel_type.trim().to_lowercase();
    let url = match payload.url {
        Some(ref u) if !u.trim().is_empty() => u.trim().to_string(),
        _ if ctype == "telegram" => {
            if let Some(ref tok) = telegram_bot_token {
                format!("https://api.telegram.org/bot{tok}/sendMessage")
            } else {
                "https://api.telegram.org".to_string()
            }
        }
        _ => String::new(),
    };

    let channel = crate::db::notifications::NotificationChannel {
        id: uuid::Uuid::new_v4().to_string(),
        name: payload.name.trim().to_string(),
        channel_type: normalize_channel_type(&ctype),
        url,
        is_default: payload.is_default,
        is_enabled: payload.is_enabled,
        use_default_template: payload.use_default_template,
        notify_on_resolve: payload.notify_on_resolve,
        custom_template: payload.custom_template.filter(|s| !s.trim().is_empty()),
        created_at: chrono::Utc::now().to_rfc3339(),
        bot_name,
        avatar_url,
        telegram_bot_token,
        telegram_chat_id,
        telegram_thread_id,
        last_error: None,
        last_error_at: None,
    };

    let created = crate::db::notifications::create_channel(&state.db, channel).await?;
    Ok((axum::http::StatusCode::CREATED, Json(created.masked_view())))
}

/// `PUT /api/admin/channels/:id` — Update channel details (preserving existing secret URL when omitted or masked).
pub async fn update_channel(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(payload): Json<UpdateChannelRequest>,
) -> Result<Json<crate::db::notifications::NotificationChannel>, AppError> {
    payload.validate()?;

    let existing = crate::db::notifications::get_channel(&state.db, &id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("Notification channel {id} not found")))?;

    let name = payload.name.as_deref().map(str::trim).unwrap_or(&existing.name);
    let channel_type = payload
        .channel_type
        .as_deref()
        .map(normalize_channel_type)
        .unwrap_or(existing.channel_type);
    let url = match payload.url.as_deref().map(str::trim) {
        Some(u) if !u.is_empty() && !crate::db::notifications::is_masked_url(u) => u,
        _ => &existing.url,
    };
    let is_default = payload.is_default.unwrap_or(existing.is_default);
    let is_enabled = payload.is_enabled.unwrap_or(existing.is_enabled);
    let use_default_template = payload
        .use_default_template
        .unwrap_or(existing.use_default_template);
    let notify_on_resolve = payload
        .notify_on_resolve
        .unwrap_or(existing.notify_on_resolve);
    let custom_template = match &payload.custom_template {
        Some(tpl) if tpl.trim().is_empty() => None,
        Some(tpl) => Some(tpl.as_str()),
        None => existing.custom_template.as_deref(),
    };

    let bot_name = match payload.bot_name {
        Some(s) => {
            let t = s.trim().to_string();
            if t.is_empty() {
                None
            } else {
                Some(t)
            }
        }
        None => existing.bot_name,
    };
    let avatar_url = match payload.avatar_url {
        Some(s) => {
            let t = s.trim().to_string();
            if t.is_empty() {
                None
            } else {
                Some(t)
            }
        }
        None => existing.avatar_url,
    };
    let telegram_bot_token = match payload.telegram_bot_token {
        Some(s) => {
            let t = s.trim().to_string();
            if t.is_empty() {
                None
            } else if t == "****" {
                existing.telegram_bot_token
            } else {
                Some(t)
            }
        }
        None => existing.telegram_bot_token,
    };
    let telegram_chat_id = match payload.telegram_chat_id {
        Some(s) => {
            let t = s.trim().to_string();
            if t.is_empty() {
                None
            } else {
                Some(t)
            }
        }
        None => existing.telegram_chat_id,
    };
    let telegram_thread_id = match payload.telegram_thread_id {
        Some(s) => {
            let t = s.trim().to_string();
            if t.is_empty() {
                None
            } else {
                Some(t)
            }
        }
        None => existing.telegram_thread_id,
    };

    let telegram_url;
    let url = if channel_type == "telegram" && existing.url.starts_with("https://api.telegram.org/bot") {
        if let Some(ref tok) = telegram_bot_token {
            telegram_url = format!("https://api.telegram.org/bot{tok}/sendMessage");
            &telegram_url
        } else {
            url
        }
    } else {
        url
    };

    let updated = crate::db::notifications::update_channel(
        &state.db,
        &id,
        name,
        &channel_type,
        url,
        is_default,
        is_enabled,
        use_default_template,
        notify_on_resolve,
        custom_template,
        bot_name.as_deref(),
        avatar_url.as_deref(),
        telegram_bot_token.as_deref(),
        telegram_chat_id.as_deref(),
        telegram_thread_id.as_deref(),
    )
    .await?;

    Ok(Json(updated.masked_view()))
}

/// `DELETE /api/admin/channels/:id` — Remove a channel and unbind from monitors.
pub async fn delete_channel(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<axum::http::StatusCode, AppError> {
    let deleted = crate::db::notifications::delete_channel(&state.db, &id).await?;
    if !deleted {
        return Err(AppError::NotFound(format!("Notification channel {id} not found")));
    }
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// `POST /api/admin/channels/:id/test` — Dispatch an instant test payload to verify webhook functionality.
pub async fn test_channel(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let channel = match crate::db::notifications::get_channel(&state.db, &id).await {
        Ok(Some(c)) => c,
        Ok(None) => {
            return Err((
                axum::http::StatusCode::NOT_FOUND,
                Json(json!({ "error": format!("Notification channel {id} not found") })),
            ));
        }
        Err(err) => {
            tracing::error!("Failed loading notification channel {id} for test: {err}");
            return Err((
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Internal server error" })),
            ));
        }
    };

    match crate::alerting::test_channel(&state.http_client, &channel).await {
        Ok(()) => {
            let _ = crate::db::notifications::record_channel_success(&state.db, &channel.id).await;
            Ok(Json(json!({
                "success": true,
                "message": format!("Test payload successfully delivered to {} ({})", channel.name, channel.channel_type)
            })))
        }
        Err(err) => {
            let _ = crate::db::notifications::record_channel_failure(
                &state.db,
                &channel.id,
                &channel.name,
                &channel.channel_type,
                &err.to_string(),
                None,
                1,
            )
            .await;
            Err((
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({
                    "success": false,
                    "error": format!("Channel test failed: {err}")
                })),
            ))
        }
    }
}

/// `GET /api/admin/monitors/:id/channels` — Get channels bound to a monitor.
pub async fn get_monitor_channels(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Vec<String>>, AppError> {
    let channel_ids = crate::db::notifications::get_channel_ids_for_monitor(&state.db, &id).await?;
    Ok(Json(channel_ids))
}

// ── Status Page Admin Handlers (delegated to crate::api::status_pages) ───────

#[allow(unused_imports)]
pub use crate::api::status_pages::{
    create_status_page, delete_status_page, get_status_page, list_status_pages,
    update_status_page,
};

// ── Maintenance Window Admin Handlers (delegated to crate::api::maintenance) ──

#[allow(unused_imports)]
pub use crate::api::maintenance::{
    create_maintenance, delete_maintenance, get_maintenance, list_maintenance,
    update_maintenance,
};

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use sqlx::sqlite::SqlitePoolOptions;
    use tower::ServiceExt;

    async fn setup_test_app() -> (axum::Router, Arc<AppState>) {
        let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("Failed to run migrations");

        let config = crate::config::Config::from_env();
        let state = AppState::new_for_test(pool, config);

        let app = crate::api::router(state.clone()).with_state(state.clone());
        (app, state)
    }

    #[tokio::test]
    async fn test_change_password_flow() {
        let (app, state) = setup_test_app().await;

        let token = crate::api::auth::create_jwt(
            &state.config.admin_user,
            &state.config.jwt_secret,
            state.current_session_version(),
        )
        .unwrap();

        // 1. Unauthenticated request should fail (401)
        let req = Request::builder()
            .method("POST")
            .uri("/api/admin/change-password")
            .header("Content-Type", "application/json")
            .body(Body::from(
                json!({
                    "current_password": "admin",
                    "new_password": "newpassword123"
                })
                .to_string(),
            ))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        // 2. Authenticated request with invalid current password should return 401 with { "error": "Invalid current password" }
        let req = Request::builder()
            .method("POST")
            .uri("/api/admin/change-password")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(
                json!({
                    "current_password": "wrongpassword",
                    "new_password": "newpassword123"
                })
                .to_string(),
            ))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        let body = axum::body::to_bytes(res.into_body(), 1024 * 16).await.unwrap();
        let json_body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json_body["error"], "Invalid current password");

        // 3. New password too short (< 8 chars) should return 400 Bad Request
        let req = Request::builder()
            .method("POST")
            .uri("/api/admin/change-password")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(
                json!({
                    "current_password": "admin",
                    "new_password": "short"
                })
                .to_string(),
            ))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);

        // 4. Valid change password should return 200 OK with { "message": "Password updated successfully" }
        let req = Request::builder()
            .method("POST")
            .uri("/api/admin/change-password")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(
                json!({
                    "current_password": "admin",
                    "new_password": "supersecretpassword123"
                })
                .to_string(),
            ))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let rotated_cookie = res
            .headers()
            .get(axum::http::header::SET_COOKIE)
            .expect("change-password must issue a fresh session cookie")
            .to_str()
            .unwrap()
            .to_string();
        let body = axum::body::to_bytes(res.into_body(), 1024 * 16).await.unwrap();
        let json_body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json_body["message"], "Password updated successfully");

        // 4b. Previously minted JWT must now be rejected (401), while rotated cookie succeeds
        let old_jwt_res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/monitors")
                    .header("Authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(old_jwt_res.status(), StatusCode::UNAUTHORIZED);

        let new_cookie_kv = rotated_cookie.split(';').next().unwrap();
        let new_jwt_res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/monitors")
                    .header(axum::http::header::COOKIE, new_cookie_kv)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(new_jwt_res.status(), StatusCode::OK);

        // 5. Verify old password no longer works for login
        let req = Request::builder()
            .method("POST")
            .uri("/api/auth/login")
            .header("X-Forwarded-For", "10.0.0.42")
            .header("Content-Type", "application/json")
            .body(Body::from(
                json!({
                    "username": "admin",
                    "password": "admin"
                })
                .to_string(),
            ))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        // 6. Verify new password successfully logs in
        let req = Request::builder()
            .method("POST")
            .uri("/api/auth/login")
            .header("X-Forwarded-For", "10.0.0.42")
            .header("Content-Type", "application/json")
            .body(Body::from(
                json!({
                    "username": "admin",
                    "password": "supersecretpassword123"
                })
                .to_string(),
            ))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_monitor_details_and_ping_endpoints() {
        let (app, state) = setup_test_app().await;

        let token = crate::api::auth::create_jwt(
            &state.config.admin_user,
            &state.config.jwt_secret,
            state.current_session_version(),
        )
        .unwrap();

        // Seed a public monitor and a private monitor
        sqlx::query(
            "INSERT INTO monitors (id, name, url, is_active, is_public) VALUES ('mon-pub', 'Public Test', 'https://example.com', 1, 1)"
        )
        .execute(&state.db)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO monitors (id, name, url, is_active, is_public) VALUES ('mon-priv', 'Private Test', 'https://example.com', 1, 0)"
        )
        .execute(&state.db)
        .await
        .unwrap();

        // Seed 3 check logs for mon-pub
        for i in 1..=3 {
            sqlx::query(
                "INSERT INTO check_logs (monitor_id, status_code, latency_ms, is_up, error_msg, checked_at, created_at)
                 VALUES ('mon-pub', 200, ?, 1, NULL, datetime('now'), datetime('now'))"
            )
            .bind(20 + i * 5)
            .execute(&state.db)
            .await
            .unwrap();
        }

        // 1. Unauthenticated request to private monitor details should return 404 (prevents oracle)
        let req = Request::builder()
            .method("GET")
            .uri("/api/admin/monitors/mon-priv/details")
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);

        // 2. Unauthenticated request to public monitor details should succeed (200 OK)
        let req = Request::builder()
            .method("GET")
            .uri("/api/admin/monitors/mon-pub/details")
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 1024 * 64).await.unwrap();
        let details: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(details["monitor"]["id"], "mon-pub");
        assert_eq!(details["monitor"]["name"], "Public Test");
        assert_eq!(details["monitor"]["method"], "GET");
        assert_eq!(details["slas"]["uptime_24h"], 100.0);
        assert_eq!(details["logs"].as_array().unwrap().len(), 3);
        assert_eq!(details["hourly_latency"].as_array().unwrap().len(), 24);

        // 3. Authenticated request to private monitor details should succeed
        let req = Request::builder()
            .method("GET")
            .uri("/api/admin/monitors/mon-priv/details")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // 4. Unauthenticated ping should fail with 401
        let req = Request::builder()
            .method("POST")
            .uri("/api/admin/monitors/mon-pub/ping")
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        // 5. Authenticated ping to a valid monitor executes out-of-band and persists log
        let req = Request::builder()
            .method("POST")
            .uri("/api/admin/monitors/mon-pub/ping")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 1024 * 16).await.unwrap();
        let ping_res: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(ping_res["status"] == "UP" || ping_res["status"] == "DOWN");
        assert!(ping_res.get("latency_ms").is_some());

        // Check that check_logs has a new row (now 4 logs)
        let log_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM check_logs WHERE monitor_id = 'mon-pub'")
            .fetch_one(&state.db)
            .await
            .unwrap();
        assert_eq!(log_count, 4);
    }

    #[tokio::test]
    async fn test_asset_expiry_endpoints() {
        let (app, state) = setup_test_app().await;

        let token = crate::api::auth::create_jwt(
            &state.config.admin_user,
            &state.config.jwt_secret,
            state.current_session_version(),
        )
        .unwrap();

        // 1. Unauthenticated request should return 401
        let req = Request::builder()
            .method("GET")
            .uri("/api/admin/assets/expiry")
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        // Seed monitors:
        // mon-crit: SSL expiring in 3 days -> CRITICAL
        sqlx::query(
            "INSERT INTO monitors (id, name, url, is_active, check_ssl, ssl_days_remaining, ssl_expires_at)
             VALUES ('mon-crit', 'Critical SSL', 'https://crit.example.com', 1, 1, 3, '2026-04-01T00:00:00Z')"
        )
        .execute(&state.db)
        .await
        .unwrap();

        // mon-warn: Domain expiring in 25 days -> WARNING
        sqlx::query(
            "INSERT INTO monitors (id, name, url, is_active, check_domain_expiry, domain_days_remaining, domain_expires_at)
             VALUES ('mon-warn', 'Warning Domain', 'https://warn.example.com', 1, 1, 25, '2026-04-20T00:00:00Z')"
        )
        .execute(&state.db)
        .await
        .unwrap();

        // mon-healthy: SSL expiring in 60 days, Domain expiring in 180 days -> HEALTHY
        sqlx::query(
            "INSERT INTO monitors (id, name, url, is_active, check_ssl, ssl_days_remaining, check_domain_expiry, domain_days_remaining)
             VALUES ('mon-healthy', 'Healthy Asset', 'https://healthy.example.com', 1, 1, 60, 1, 180)"
        )
        .execute(&state.db)
        .await
        .unwrap();

        // mon-ignored: No SSL or domain monitoring enabled
        sqlx::query(
            "INSERT INTO monitors (id, name, url, is_active, check_ssl, check_domain_expiry)
             VALUES ('mon-ignored', 'Plain HTTP', 'http://plain.example.com', 1, 0, 0)"
        )
        .execute(&state.db)
        .await
        .unwrap();

        // 2. Authenticated request should return the 3 tracked monitors sorted by MIN days remaining ASC
        let req = Request::builder()
            .method("GET")
            .uri("/api/admin/assets/expiry")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let body = axum::body::to_bytes(res.into_body(), 1024 * 64).await.unwrap();
        let items: Vec<AssetExpiryItem> = serde_json::from_slice(&body).unwrap();
        assert_eq!(items.len(), 3);

        // First item must be mon-crit (3 days remaining <= 7 -> CRITICAL)
        assert_eq!(items[0].monitor_id, "mon-crit");
        assert_eq!(items[0].urgency, "CRITICAL");
        assert_eq!(items[0].ssl_days_remaining, Some(3));

        // Second item must be mon-warn (25 days remaining <= 30 -> WARNING)
        assert_eq!(items[1].monitor_id, "mon-warn");
        assert_eq!(items[1].urgency, "WARNING");
        assert_eq!(items[1].domain_days_remaining, Some(25));

        // Third item must be mon-healthy (60 days remaining -> HEALTHY)
        assert_eq!(items[2].monitor_id, "mon-healthy");
        assert_eq!(items[2].urgency, "HEALTHY");

        // 3. Test on-demand refresh endpoint
        // Unauthenticated request should fail (401)
        let req = Request::builder()
            .method("POST")
            .uri("/api/admin/monitors/mon-crit/refresh-assets")
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        // Authenticated request
        let req = Request::builder()
            .method("POST")
            .uri("/api/admin/monitors/mon-crit/refresh-assets")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 1024 * 16).await.unwrap();
        let refreshed: AssetExpiryItem = serde_json::from_slice(&body).unwrap();
        assert_eq!(refreshed.monitor_id, "mon-crit");
        assert!(refreshed.ssl_enabled);
    }

    #[tokio::test]
    async fn test_notification_channels_crud_and_monitor_attachment() {
        let (app, state) = setup_test_app().await;

        let token = crate::api::auth::create_jwt(
            &state.config.admin_user,
            &state.config.jwt_secret,
            state.current_session_version(),
        )
        .unwrap();

        // 1. Unauthenticated requests to /api/admin/channels should return 401
        let req = Request::builder()
            .method("GET")
            .uri("/api/admin/channels")
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        // 2. Create "Engineering Discord" channel with custom bot name and avatar
        let req = Request::builder()
            .method("POST")
            .uri("/api/admin/channels")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(
                json!({
                    "name": "Engineering Discord",
                    "type": "discord",
                    "url": "https://discord.com/api/webhooks/12345/abcdef",
                    "is_default": false,
                    "bot_name": "KeepUp Sentinel",
                    "avatar_url": "https://example.com/sentinel.png"
                })
                .to_string(),
            ))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(res.into_body(), 1024 * 16).await.unwrap();
        let chan_discord: crate::db::notifications::NotificationChannel =
            serde_json::from_slice(&body).unwrap();
        assert_eq!(chan_discord.name, "Engineering Discord");
        assert_eq!(chan_discord.channel_type, "discord");
        assert_eq!(chan_discord.bot_name.as_deref(), Some("KeepUp Sentinel"));
        assert_eq!(chan_discord.avatar_url.as_deref(), Some("https://example.com/sentinel.png"));
        assert!(!chan_discord.is_default);

        // 3. Create "Ops Slack" channel (default = true)
        let req = Request::builder()
            .method("POST")
            .uri("/api/admin/channels")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(
                json!({
                    "name": "Ops Slack",
                    "type": "slack",
                    "url": "https://hooks.slack.com/services/T00/B00/X00",
                    "is_default": true
                })
                .to_string(),
            ))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(res.into_body(), 1024 * 16).await.unwrap();
        let chan_slack: crate::db::notifications::NotificationChannel =
            serde_json::from_slice(&body).unwrap();
        assert_eq!(chan_slack.name, "Ops Slack");
        assert!(chan_slack.is_default);

        // 4. List channels
        let req = Request::builder()
            .method("GET")
            .uri("/api/admin/channels")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 1024 * 16).await.unwrap();
        let channels: Vec<crate::db::notifications::NotificationChannel> =
            serde_json::from_slice(&body).unwrap();
        assert_eq!(channels.len(), 2);

        // 5. Update "Engineering Discord" channel
        let req = Request::builder()
            .method("PUT")
            .uri(format!("/api/admin/channels/{}", chan_discord.id))
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(
                json!({
                    "name": "Engineering Discord Updated",
                    "bot_name": "KeepUp Ops Bot",
                    "avatar_url": "https://example.com/new-avatar.png"
                })
                .to_string(),
            ))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 1024 * 16).await.unwrap();
        let updated_chan: crate::db::notifications::NotificationChannel =
            serde_json::from_slice(&body).unwrap();
        assert_eq!(updated_chan.name, "Engineering Discord Updated");
        assert_eq!(updated_chan.bot_name.as_deref(), Some("KeepUp Ops Bot"));
        assert_eq!(updated_chan.avatar_url.as_deref(), Some("https://example.com/new-avatar.png"));

        // 6. Create a monitor attaching Engineering Discord explicitly
        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/monitors")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(
                json!({
                    "name": "Backend API Service",
                    "url": "https://api.example.com/healthz",
                    "channel_ids": [chan_discord.id]
                })
                .to_string(),
            ))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(res.into_body(), 1024 * 16).await.unwrap();
        let created_mon: Monitor = serde_json::from_slice(&body).unwrap();

        // 7. Verify monitor details returns the attached channel ID
        let req = Request::builder()
            .method("GET")
            .uri(format!("/api/admin/monitors/{}/details", created_mon.id))
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 1024 * 32).await.unwrap();
        let detail_resp: MonitorDetailResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(detail_resp.monitor.channel_ids, vec![chan_discord.id.clone()]);

        // 8. Verify effective channels returns BOTH explicitly attached + default channel
        let effective = crate::db::notifications::get_effective_channels_for_monitor(
            &state.db,
            &created_mon.id,
        )
        .await
        .unwrap();
        let effective_names: Vec<String> = effective.into_iter().map(|c| c.name).collect();
        assert!(effective_names.contains(&"Engineering Discord Updated".to_string()));
        assert!(effective_names.contains(&"Ops Slack".to_string()));

        // 9. Update monitor to attach both channels
        let req = Request::builder()
            .method("PUT")
            .uri(format!("/api/monitors/{}", created_mon.id))
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(
                json!({
                    "channel_ids": [chan_discord.id, chan_slack.id]
                })
                .to_string(),
            ))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let attached_ids = crate::db::notifications::get_channel_ids_for_monitor(
            &state.db,
            &created_mon.id,
        )
        .await
        .unwrap();
        assert_eq!(attached_ids.len(), 2);

        // 10. Delete channel
        let req = Request::builder()
            .method("DELETE")
            .uri(format!("/api/admin/channels/{}", chan_discord.id))
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::NO_CONTENT);

        // Verify monitor association was cascaded
        let attached_ids_after = crate::db::notifications::get_channel_ids_for_monitor(
            &state.db,
            &created_mon.id,
        )
        .await
        .unwrap();
        assert_eq!(attached_ids_after, vec![chan_slack.id]);
    }

    #[tokio::test]
    async fn test_channel_test_and_alert_dispatch() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let (strict_app, strict_state) = setup_test_app().await;

        let mut allow_priv_cfg = crate::config::Config::from_env();
        allow_priv_cfg.allow_private_targets = true;
        let state = AppState::new_for_test(strict_state.db.clone(), allow_priv_cfg);
        let app = crate::api::router(state.clone()).with_state(state.clone());

        let token = crate::api::auth::create_jwt(
            &state.config.admin_user,
            &state.config.jwt_secret,
            state.current_session_version(),
        )
        .unwrap();

        // Spin up a mock HTTP server on 127.0.0.1
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hit_counter = Arc::new(AtomicUsize::new(0));
        let hit_counter_clone = hit_counter.clone();

        tokio::spawn(async move {
            loop {
                if let Ok((mut socket, _)) = listener.accept().await {
                    let mut buf = [0u8; 2048];
                    let _ = socket.read(&mut buf).await;
                    hit_counter_clone.fetch_add(1, Ordering::SeqCst);
                    let response = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
                    let _ = socket.write_all(response.as_bytes()).await;
                }
            }
        });

        // Create channel pointing to mock server
        let mock_url = format!("http://{addr}/test-webhook");
        let req = Request::builder()
            .method("POST")
            .uri("/api/admin/channels")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(
                json!({
                    "name": "Mock Test Channel",
                    "type": "webhook",
                    "url": mock_url,
                    "is_default": true
                })
                .to_string(),
            ))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(res.into_body(), 1024 * 16).await.unwrap();
        let chan: crate::db::notifications::NotificationChannel =
            serde_json::from_slice(&body).unwrap();

        // Default strict app (ALLOW_PRIVATE_TARGETS=false) MUST block 127.0.0.1 webhook SSRF
        let strict_token = crate::api::auth::create_jwt(
            &strict_state.config.admin_user,
            &strict_state.config.jwt_secret,
            strict_state.current_session_version(),
        )
        .unwrap();
        let strict_req = Request::builder()
            .method("POST")
            .uri(format!("/api/admin/channels/{}/test", chan.id))
            .header("Authorization", format!("Bearer {strict_token}"))
            .body(Body::empty())
            .unwrap();
        let strict_res = strict_app.oneshot(strict_req).await.unwrap();
        assert_eq!(strict_res.status(), StatusCode::BAD_REQUEST);
        assert_eq!(hit_counter.load(Ordering::SeqCst), 0);

        // Test POST /api/admin/channels/:id/test
        let req = Request::builder()
            .method("POST")
            .uri(format!("/api/admin/channels/{}/test", chan.id))
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert!(hit_counter.load(Ordering::SeqCst) >= 1);

        // Test parallel alert dispatcher decoupling
        let payload = crate::alerting::AlertPayload {
            monitor_id: None,
            monitor_name: "Mock Service".to_string(),
            monitor_url: "https://mock.service".to_string(),
            new_status: "DOWN".to_string(),
            old_status: "UP".to_string(),
            http_status: Some(503),
            response_time_ms: Some(150),
            error_msg: Some("HTTP 503 Service Unavailable".to_string()),
            downtime_duration: None,
            downtime_duration_secs: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
            domain_days_remaining: None,
            domain_expires_at: None,
        };

        let initial_hits = hit_counter.load(Ordering::SeqCst);
        crate::alerting::dispatch_alerts(&state.db, &state.http_client, &payload).await;

        // Allow parallel Tokio worker task to execute
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        assert!(hit_counter.load(Ordering::SeqCst) > initial_hits);
    }

    #[tokio::test]
    async fn test_unauthenticated_monitor_details_prevents_oracle_hides_channels_sanitizes_errors_and_includes_tls_days() {
        let (app, state) = setup_test_app().await;

        sqlx::query(
            "INSERT INTO monitors (id, name, url, is_public, is_active) VALUES \
             ('mon-priv', 'Private DB', 'https://db.internal', 0, 1), \
             ('mon-paused', 'Paused API', 'https://paused.example.com', 1, 0), \
             ('mon-pub', 'Public API', 'https://pub.example.com', 1, 1)",
        )
        .execute(&state.db)
        .await
        .unwrap();

        let chan = crate::db::notifications::create_channel(
            &state.db,
            crate::db::notifications::NotificationChannel {
                id: "chan-sec-1".to_string(),
                name: "Sec Hook".to_string(),
                channel_type: "webhook".to_string(),
                url: "https://hooks.example.com/secret".to_string(),
                is_default: false,
                is_enabled: true,
                use_default_template: true,
                notify_on_resolve: true,
                custom_template: None,
                created_at: chrono::Utc::now().to_rfc3339(),
                bot_name: None,
                avatar_url: None,
                telegram_bot_token: None,
                telegram_chat_id: None,
                telegram_thread_id: None,
                last_error: None,
                last_error_at: None,
            },
        )
        .await
        .unwrap();
        crate::db::notifications::set_monitor_channels(
            &state.db,
            "mon-pub",
            &[chan.id.clone()],
        )
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO check_logs (monitor_id, status_code, latency_ms, is_up, error_msg, ssl_days_remaining, domain_days_remaining, checked_at) \
             VALUES ('mon-pub', 502, 85, 0, 'dial tcp 10.24.0.9:8080: connection refused', 62, 180, datetime('now'))",
        )
        .execute(&state.db)
        .await
        .unwrap();

        // 1. Private monitor returns 404 (not 401) to unauthenticated callers
        let req_priv = Request::builder()
            .method("GET")
            .uri("/api/admin/monitors/mon-priv/details")
            .body(Body::empty())
            .unwrap();
        let res_priv = app.clone().oneshot(req_priv).await.unwrap();
        assert_eq!(res_priv.status(), StatusCode::NOT_FOUND);

        // 2. Paused monitor returns 404 to unauthenticated callers
        let req_paused = Request::builder()
            .method("GET")
            .uri("/api/admin/monitors/mon-paused/details")
            .body(Body::empty())
            .unwrap();
        let res_paused = app.clone().oneshot(req_paused).await.unwrap();
        assert_eq!(res_paused.status(), StatusCode::NOT_FOUND);

        // 3. Public active monitor returns 200 with empty channel_ids, sanitized error_message, and populated tls_days_remaining
        let req_pub = Request::builder()
            .method("GET")
            .uri("/api/admin/monitors/mon-pub/details")
            .body(Body::empty())
            .unwrap();
        let res_pub = app.clone().oneshot(req_pub).await.unwrap();
        assert_eq!(res_pub.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res_pub.into_body(), 1024 * 32).await.unwrap();
        let details: MonitorDetailResponse = serde_json::from_slice(&body).unwrap();

        assert!(
            details.monitor.channel_ids.is_empty(),
            "unauthenticated details response must not leak channel_ids"
        );
        assert_eq!(details.logs.len(), 1);
        assert_eq!(
            details.logs[0].tls_days_remaining,
            Some(62),
            "tls_days_remaining must be populated from ssl_days_remaining"
        );
        let err_msg = details.logs[0].error_message.as_deref().unwrap_or("");
        assert!(
            !err_msg.contains("10.24.0.9"),
            "unauthenticated details response must sanitize internal error_message, got: {err_msg}"
        );
    }

    #[tokio::test]
    async fn test_ping_monitor_preserves_down_since_and_respects_maintenance() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();

        let initial_down_since = "2026-01-01T00:00:00Z";
        sqlx::query(
            "INSERT INTO monitors (id, name, url, check_interval_secs, timeout_secs, max_retries, is_active, is_public, last_status, down_since) \
             VALUES ('mon-down', 'Down Service', 'http://127.0.0.1:9', 60, 1, 1, 1, 1, 'DOWN', ?)"
        )
        .bind(initial_down_since)
        .execute(&pool)
        .await
        .unwrap();

        let config = crate::config::Config {
            allow_private_targets: true,
            ..Default::default()
        };

        let state = crate::AppState::new_for_test(pool.clone(), config.clone());
        let app = crate::build_http_app(state);

        let token = crate::api::auth::create_jwt(
            &config.admin_user,
            &config.jwt_secret,
            1,
        )
        .unwrap();

        let req = Request::builder()
            .method("POST")
            .uri("/api/admin/monitors/mon-down/ping")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // Verify down_since was NOT overwritten by current timestamp!
        let row: (Option<String>,) = sqlx::query_as("SELECT down_since FROM monitors WHERE id = 'mon-down'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.0.as_deref(), Some(initial_down_since), "ping_monitor must preserve existing down_since on already-DOWN monitor");
    }

    #[tokio::test]
    async fn test_discord_channel_custom_bot_and_avatar_test_dispatch() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let (_base_app, base_state) = setup_test_app().await;
        let mut allow_priv_cfg = crate::config::Config::from_env();
        allow_priv_cfg.allow_private_targets = true;
        let state = AppState::new_for_test(base_state.db.clone(), allow_priv_cfg);
        let app = crate::api::router(state.clone()).with_state(state.clone());

        let token = crate::api::auth::create_jwt(
            &state.config.admin_user,
            &state.config.jwt_secret,
            state.current_session_version(),
        )
        .unwrap();

        // Spin up mock server capturing request body
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let captured_body = Arc::new(tokio::sync::Mutex::new(String::new()));
        let captured_body_clone = captured_body.clone();

        tokio::spawn(async move {
            loop {
                if let Ok((mut socket, _)) = listener.accept().await {
                    let mut buf = vec![0u8; 4096];
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    let req_str = String::from_utf8_lossy(&buf[..n]).to_string();
                    if let Some(pos) = req_str.find("\r\n\r\n") {
                        let mut body_lock = captured_body_clone.lock().await;
                        *body_lock = req_str[pos + 4..].to_string();
                    }
                    let response = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
                    let _ = socket.write_all(response.as_bytes()).await;
                }
            }
        });

        // 1. Create Discord channel with custom bot name and avatar
        let mock_url = format!("http://{addr}/discord-webhook");
        let req = Request::builder()
            .method("POST")
            .uri("/api/admin/channels")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(
                json!({
                    "name": "Custom Discord Alert",
                    "type": "discord",
                    "url": mock_url,
                    "is_default": false,
                    "bot_name": "KeepUp Sentinel",
                    "avatar_url": "https://example.com/custom-avatar.png"
                })
                .to_string(),
            ))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(res.into_body(), 1024 * 16).await.unwrap();
        let chan: crate::db::notifications::NotificationChannel =
            serde_json::from_slice(&body).unwrap();
        assert_eq!(chan.bot_name.as_deref(), Some("KeepUp Sentinel"));
        assert_eq!(chan.avatar_url.as_deref(), Some("https://example.com/custom-avatar.png"));

        // 2. Dispatch test alert via POST /api/admin/channels/:id/test
        let req = Request::builder()
            .method("POST")
            .uri(format!("/api/admin/channels/{}/test", chan.id))
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // 3. Verify outbound Discord JSON payload contains custom username & avatar_url
        let payload_str = captured_body.lock().await.clone();
        let payload: Value = serde_json::from_str(&payload_str).unwrap();
        assert_eq!(payload["username"], "KeepUp Sentinel");
        assert_eq!(payload["avatar_url"], "https://example.com/custom-avatar.png");
        assert_eq!(payload["embeds"][0]["color"], crate::checker::alerting::COLOR_STATUS_GREEN);

        // 4. Update channel to remove custom bot name and avatar (fallback to default branding)
        let req = Request::builder()
            .method("PUT")
            .uri(format!("/api/admin/channels/{}", chan.id))
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(
                json!({
                    "bot_name": "",
                    "avatar_url": ""
                })
                .to_string(),
            ))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // 5. Dispatch test alert again and confirm fallback
        let req = Request::builder()
            .method("POST")
            .uri(format!("/api/admin/channels/{}/test", chan.id))
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let fallback_str = captured_body.lock().await.clone();
        let fallback_payload: Value = serde_json::from_str(&fallback_str).unwrap();
        assert_eq!(fallback_payload["username"], crate::checker::alerting::DEFAULT_BOT_NAME);
        assert_eq!(fallback_payload["avatar_url"], crate::checker::alerting::DEFAULT_AVATAR_URL);
    }

    #[tokio::test]
    async fn test_telegram_channel_bot_and_topic_thread_dispatch() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let (_base_app, base_state) = setup_test_app().await;
        let mut allow_priv_cfg = crate::config::Config::from_env();
        allow_priv_cfg.allow_private_targets = true;
        let state = AppState::new_for_test(base_state.db.clone(), allow_priv_cfg);
        let app = crate::api::router(state.clone()).with_state(state.clone());

        let token = crate::api::auth::create_jwt(
            &state.config.admin_user,
            &state.config.jwt_secret,
            state.current_session_version(),
        )
        .unwrap();

        // Spin up mock server capturing request body and destination
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let captured_req = Arc::new(tokio::sync::Mutex::new(String::new()));
        let captured_req_clone = captured_req.clone();

        tokio::spawn(async move {
            loop {
                if let Ok((mut socket, _)) = listener.accept().await {
                    let mut buf = vec![0u8; 4096];
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    let req_str = String::from_utf8_lossy(&buf[..n]).to_string();
                    let mut lock = captured_req_clone.lock().await;
                    *lock = req_str;
                    let response = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
                    let _ = socket.write_all(response.as_bytes()).await;
                }
            }
        });

        // 1. Create Telegram channel with Bot Token, Chat ID, and Thread Topic ID
        let mock_url = format!("http://{addr}/sendMessage");
        let req = Request::builder()
            .method("POST")
            .uri("/api/admin/channels")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(
                json!({
                    "name": "DevOps Supergroup",
                    "type": "telegram",
                    "url": mock_url,
                    "is_default": false,
                    "telegram_bot_token": "123456:TESTBOTTOKENABC",
                    "telegram_chat_id": "-1001987654321",
                    "telegram_thread_id": "42"
                })
                .to_string(),
            ))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(res.into_body(), 1024 * 16).await.unwrap();
        let chan: crate::db::notifications::NotificationChannel =
            serde_json::from_slice(&body).unwrap();
        assert_eq!(chan.name, "DevOps Supergroup");
        assert_eq!(chan.channel_type, "telegram");
        assert_eq!(chan.telegram_bot_token.as_deref(), Some("****"));
        assert_eq!(chan.telegram_chat_id.as_deref(), Some("-1001987654321"));
        assert_eq!(chan.telegram_thread_id.as_deref(), Some("42"));

        // 2. Dispatch test alert via POST /api/admin/channels/:id/test
        let req = Request::builder()
            .method("POST")
            .uri(format!("/api/admin/channels/{}/test", chan.id))
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // 3. Inspect captured payload
        let raw_req = captured_req.lock().await.clone();
        let pos = raw_req.find("\r\n\r\n").expect("Header boundary");
        let json_body: Value = serde_json::from_str(&raw_req[pos + 4..]).unwrap();

        assert_eq!(json_body["chat_id"], "-1001987654321");
        assert_eq!(json_body["message_thread_id"], 42);
        assert_eq!(json_body["parse_mode"], "HTML");
        let text = json_body["text"].as_str().unwrap();
        assert!(text.contains("🟢 <b>[RESOLVED]</b>"));
        assert!(text.contains("KeepUp Health Monitor"));

        // 4. Update topic thread ID to "99"
        let req = Request::builder()
            .method("PUT")
            .uri(format!("/api/admin/channels/{}", chan.id))
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(
                json!({
                    "telegram_thread_id": "99"
                })
                .to_string(),
            ))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // 5. Dispatch test alert again and verify thread ID updated
        let req = Request::builder()
            .method("POST")
            .uri(format!("/api/admin/channels/{}/test", chan.id))
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let raw_req2 = captured_req.lock().await.clone();
        let pos2 = raw_req2.find("\r\n\r\n").expect("Header boundary");
        let json_body2: Value = serde_json::from_str(&raw_req2[pos2 + 4..]).unwrap();
        assert_eq!(json_body2["message_thread_id"], 99);
    }

    #[tokio::test]
    async fn test_bulk_pause_and_resume_all_endpoints() {
        let (app, state) = setup_test_app().await;

        let token = crate::api::auth::create_jwt(
            &state.config.admin_user,
            &state.config.jwt_secret,
            state.current_session_version(),
        )
        .unwrap();

        // Seed 2 active monitors
        sqlx::query("INSERT INTO monitors (id, name, url, is_active, is_public) VALUES ('bulk-1', 'B1', 'https://example.com', 1, 1)")
            .execute(&state.db).await.unwrap();
        sqlx::query("INSERT INTO monitors (id, name, url, is_active, is_public) VALUES ('bulk-2', 'B2', 'https://example.com', 1, 1)")
            .execute(&state.db).await.unwrap();

        // 1. Unauthenticated pause-all request should return 401
        let req = Request::builder()
            .method("POST")
            .uri("/api/admin/monitors/pause-all")
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        // 2. Authenticated pause-all request
        let req = Request::builder()
            .method("POST")
            .uri("/api/admin/monitors/pause-all")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 1024 * 16).await.unwrap();
        let json_body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json_body["global_state"], "ALL_PAUSED");
        assert_eq!(json_body["active_monitors"], 0);
        assert_eq!(json_body["updated_count"], 2);

        // Verify SQLite state
        let (state_str, active, total) = monitors::get_global_monitoring_state(&state.db).await.unwrap();
        assert_eq!(state_str, "ALL_PAUSED");
        assert_eq!(active, 0);
        assert_eq!(total, 2);

        // 3. Authenticated resume-all request
        let req = Request::builder()
            .method("POST")
            .uri("/api/admin/monitors/resume-all")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 1024 * 16).await.unwrap();
        let json_body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json_body["global_state"], "ALL_ACTIVE");
        assert_eq!(json_body["active_monitors"], 2);
        assert_eq!(json_body["updated_count"], 2);

        // Verify SQLite state
        let (state_str, active, total) = monitors::get_global_monitoring_state(&state.db).await.unwrap();
        assert_eq!(state_str, "ALL_ACTIVE");
        assert_eq!(active, 2);
        assert_eq!(total, 2);
    }

    #[tokio::test]
    async fn test_bulk_batch_endpoints() {
        let (app, state) = setup_test_app().await;

        let token = crate::api::auth::create_jwt(
            &state.config.admin_user,
            &state.config.jwt_secret,
            state.current_session_version(),
        )
        .unwrap();

        // Seed 3 monitors
        sqlx::query("INSERT INTO monitors (id, name, url, is_active, is_public) VALUES ('batch-1', 'B1', 'https://example.com', 1, 1)")
            .execute(&state.db).await.unwrap();
        sqlx::query("INSERT INTO monitors (id, name, url, is_active, is_public) VALUES ('batch-2', 'B2', 'https://example.com', 1, 1)")
            .execute(&state.db).await.unwrap();
        sqlx::query("INSERT INTO monitors (id, name, url, is_active, is_public) VALUES ('batch-3', 'B3', 'https://example.com', 1, 1)")
            .execute(&state.db).await.unwrap();

        sqlx::query("INSERT INTO check_logs (monitor_id, status_code, latency_ms, is_up, checked_at, created_at) VALUES ('batch-1', 200, 10, 1, datetime('now'), datetime('now'))")
            .execute(&state.db).await.unwrap();
        sqlx::query("INSERT INTO check_logs (monitor_id, status_code, latency_ms, is_up, checked_at, created_at) VALUES ('batch-2', 200, 15, 1, datetime('now'), datetime('now'))")
            .execute(&state.db).await.unwrap();

        // 1. Bulk Preview
        let req = Request::builder()
            .method("POST")
            .uri("/api/admin/monitors/bulk-preview")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(json!({ "monitor_ids": ["batch-1", "batch-2"] }).to_string()))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 1024 * 16).await.unwrap();
        let json_body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json_body["monitor_count"], 2);
        assert_eq!(json_body["check_logs_count"], 2);

        // 2. Bulk Pause batch-1 and batch-2
        let req = Request::builder()
            .method("POST")
            .uri("/api/admin/monitors/bulk-pause")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(json!({ "monitor_ids": ["batch-1", "batch-2"] }).to_string()))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 1024 * 16).await.unwrap();
        let json_body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json_body["updated_count"], 2);

        // Verify batch-1 is paused, batch-3 is still active
        let b1 = monitors::get_monitor_by_id(&state.db, "batch-1").await.unwrap();
        assert_eq!(b1.is_active, 0);
        let b3 = monitors::get_monitor_by_id(&state.db, "batch-3").await.unwrap();
        assert_eq!(b3.is_active, 1);

        // 3. Bulk Resume batch-1
        let req = Request::builder()
            .method("POST")
            .uri("/api/admin/monitors/bulk-resume")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(json!(["batch-1"]).to_string()))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let b1 = monitors::get_monitor_by_id(&state.db, "batch-1").await.unwrap();
        assert_eq!(b1.is_active, 1);

        // 4. Bulk Delete batch-1 and batch-2
        let req = Request::builder()
            .method("POST")
            .uri("/api/admin/monitors/bulk-delete")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(json!({ "monitor_ids": ["batch-1", "batch-2"] }).to_string()))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 1024 * 16).await.unwrap();
        let json_body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json_body["deleted_monitors"], 2);
        assert_eq!(json_body["deleted_logs"], 2);

        assert!(monitors::get_monitor_by_id(&state.db, "batch-1").await.is_err());
        assert!(monitors::get_monitor_by_id(&state.db, "batch-2").await.is_err());
        assert!(monitors::get_monitor_by_id(&state.db, "batch-3").await.is_ok());
    }

    #[tokio::test]
    async fn test_multi_protocol_monitors_crud_and_ping() {
        let (app, state) = setup_test_app().await;

        let token = crate::api::auth::create_jwt(
            &state.config.admin_user,
            &state.config.jwt_secret,
            state.current_session_version(),
        )
        .unwrap();

        // 1. Invalid check_type rejection
        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/monitors")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(
                json!({
                    "name": "Invalid Proto",
                    "url": "example.com",
                    "check_type": "ftp"
                })
                .to_string(),
            ))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);

        // 2. Create TCP Monitor
        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/monitors")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(
                json!({
                    "name": "Database Port",
                    "url": "127.0.0.1",
                    "check_type": "tcp",
                    "port": 5432
                })
                .to_string(),
            ))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(res.into_body(), 1024 * 16).await.unwrap();
        let tcp_mon: Monitor = serde_json::from_slice(&body).unwrap();
        assert_eq!(tcp_mon.check_type, "tcp");
        assert_eq!(tcp_mon.port, Some(5432));

        // 3. Create DNS Monitor
        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/monitors")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(
                json!({
                    "name": "Cloudflare DNS",
                    "url": "one.one.one.one",
                    "check_type": "dns",
                    "dns_record_type": "A",
                    "dns_expected_value": "1.1.1.1"
                })
                .to_string(),
            ))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(res.into_body(), 1024 * 16).await.unwrap();
        let dns_mon: Monitor = serde_json::from_slice(&body).unwrap();
        assert_eq!(dns_mon.check_type, "dns");
        assert_eq!(dns_mon.dns_record_type.as_deref(), Some("A"));
        assert_eq!(dns_mon.dns_expected_value.as_deref(), Some("1.1.1.1"));

        // 4. Create ICMP Ping Monitor
        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/monitors")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(
                json!({
                    "name": "Localhost Ping",
                    "url": "127.0.0.1",
                    "check_type": "ping"
                })
                .to_string(),
            ))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(res.into_body(), 1024 * 16).await.unwrap();
        let ping_mon: Monitor = serde_json::from_slice(&body).unwrap();
        assert_eq!(ping_mon.check_type, "ping");

        // 5. Ping Localhost Ping Monitor out-of-band
        let req = Request::builder()
            .method("POST")
            .uri(format!("/api/admin/monitors/{}/ping", ping_mon.id))
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 1024 * 16).await.unwrap();
        let ping_resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(ping_resp["status"], "UP");

        // 6. Monitor details endpoint contains multi-protocol fields
        let req = Request::builder()
            .method("GET")
            .uri(format!("/api/admin/monitors/{}/details", dns_mon.id))
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 1024 * 16).await.unwrap();
        let detail_resp: MonitorDetailResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(detail_resp.monitor.check_type, "dns");
        assert_eq!(detail_resp.monitor.dns_record_type.as_deref(), Some("A"));
        assert_eq!(detail_resp.monitor.dns_expected_value.as_deref(), Some("1.1.1.1"));
    }

    #[tokio::test]
    async fn test_clone_monitor_and_delete_workflow() {
        let (app, state) = setup_test_app().await;

        let token = crate::api::auth::create_jwt(
            &state.config.admin_user,
            &state.config.jwt_secret,
            state.current_session_version(),
        )
        .unwrap();

        // 1. Create a notification channel to test channel inheritance during cloning
        let channel = crate::db::notifications::NotificationChannel {
            id: uuid::Uuid::new_v4().to_string(),
            name: "Engineering Alerts".to_string(),
            channel_type: "webhook".to_string(),
            url: "https://webhook.site/clone-test".to_string(),
            is_default: false,
            is_enabled: true,
            use_default_template: true,
            notify_on_resolve: true,
            custom_template: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            bot_name: None,
            avatar_url: None,
            telegram_bot_token: None,
            telegram_chat_id: None,
            telegram_thread_id: None,
            last_error: None,
            last_error_at: None,
        };
        crate::db::notifications::create_channel(&state.db, channel.clone())
            .await
            .unwrap();

        // 2. Create the source monitor
        let source_id = "source-mon-1";
        sqlx::query(
            "INSERT INTO monitors (id, name, url, check_interval_secs, timeout_secs, \
                                   max_retries, is_active, is_public, last_status, \
                                   check_domain_expiry, domain_warning_days, check_ssl, ssl_warning_days, \
                                   check_type, port, dns_record_type, dns_expected_value) \
             VALUES (?, 'Production API', 'https://api.example.com', 120, 10, 2, 1, 1, 'UP', 1, 15, 1, 20, 'http', NULL, NULL, NULL)",
        )
        .bind(source_id)
        .execute(&state.db)
        .await
        .unwrap();

        crate::db::notifications::set_monitor_channels(&state.db, source_id, &[channel.id.clone()])
            .await
            .unwrap();

        // 3. Dispatch Clone request via POST /api/admin/monitors/:id/clone
        let req = Request::builder()
            .method("POST")
            .uri(format!("/api/admin/monitors/{source_id}/clone"))
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);

        let body = axum::body::to_bytes(res.into_body(), 1024 * 16).await.unwrap();
        let cloned_mon: Monitor = serde_json::from_slice(&body).unwrap();

        assert_ne!(cloned_mon.id, source_id);
        assert_eq!(cloned_mon.name, "Production API (Copy)");
        assert_eq!(cloned_mon.url, "https://api.example.com");
        assert_eq!(cloned_mon.check_interval_secs, 120);
        assert_eq!(cloned_mon.timeout_secs, 10);
        assert_eq!(cloned_mon.max_retries, 2);
        assert_eq!(cloned_mon.check_domain_expiry, 1);
        assert_eq!(cloned_mon.domain_warning_days, 15);
        assert_eq!(cloned_mon.check_ssl, 1);
        assert_eq!(cloned_mon.ssl_warning_days, 20);

        // 4. Verify cloned monitor inherited the notification channels
        let cloned_channels = crate::db::notifications::get_channel_ids_for_monitor(&state.db, &cloned_mon.id)
            .await
            .unwrap();
        assert_eq!(cloned_channels.len(), 1);
        assert_eq!(cloned_channels[0], channel.id);

        // 5. Verify source monitor is intact
        let src = monitors::get_monitor_by_id(&state.db, source_id).await.unwrap();
        assert_eq!(src.name, "Production API");

        // 6. Delete the cloned monitor
        let del_req = Request::builder()
            .method("DELETE")
            .uri(format!("/api/v1/monitors/{}", cloned_mon.id))
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let del_res = app.clone().oneshot(del_req).await.unwrap();
        assert_eq!(del_res.status(), StatusCode::NO_CONTENT);

        // Cloned monitor is gone, source monitor remains
        assert!(monitors::get_monitor_by_id(&state.db, &cloned_mon.id).await.is_err());
        assert!(monitors::get_monitor_by_id(&state.db, source_id).await.is_ok());
    }
}
