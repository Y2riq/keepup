use std::time::Duration;

use reqwest::Client;
use serde_json::{json, Value};
use sqlx::SqlitePool;
use tracing::{error, info, warn};

use crate::db::notifications::NotificationChannel;

/// Alert context passed to each notification channel dispatcher.
#[derive(Debug, Clone)]
pub struct AlertPayload {
    /// Optional monitor ID.
    pub monitor_id: Option<String>,
    /// Human-readable monitor name.
    pub monitor_name: String,
    /// URL being monitored.
    pub monitor_url: String,
    /// New status after the transition (`"DOWN"`, `"UP"`, `"DEGRADED"`, `"DOMAIN_EXPIRING_SOON"`, `"SSL_EXPIRING_SOON"`).
    pub new_status: String,
    /// Previous status before the transition.
    pub old_status: String,
    /// HTTP status code from check (e.g. 200, 500).
    pub http_status: Option<u16>,
    /// Response time / latency in milliseconds.
    pub response_time_ms: Option<u64>,
    /// Error message (populated when transitioning to DOWN or DEGRADED).
    pub error_msg: Option<String>,
    /// Human-readable downtime duration (populated when recovering to UP).
    pub downtime_duration: Option<String>,
    /// Downtime duration in seconds (populated when recovering to UP).
    pub downtime_duration_secs: Option<u64>,
    /// ISO 8601 timestamp string.
    pub timestamp: String,
    /// Days remaining until domain registration or certificate expires.
    pub domain_days_remaining: Option<i64>,
    /// Formatted expiration timestamp of domain registration or certificate.
    pub domain_expires_at: Option<String>,
}

/// Exponential backoff retry configuration for outbound notifications.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum attempts per dispatch (default: 3).
    pub max_attempts: u32,
    /// Initial base backoff delay (default: 1 second).
    pub initial_delay: Duration,
    /// Maximum backoff cap (default: 30 seconds).
    pub max_delay: Duration,
    /// Absolute execution timeout across the complete retry lifecycle for a single alert event.
    pub max_elapsed_time: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
            max_elapsed_time: Duration::from_secs(60),
        }
    }
}

/// Computes exponential backoff delay with equal randomized jitter.
/// Base doubles on each attempt: min(base_delay * 2^attempt, max_delay).
/// Equal jitter: half is deterministic base, other half is uniformly randomized.
pub fn calculate_retry_delay(
    attempt: u32,
    base_delay: Duration,
    max_delay: Duration,
) -> Duration {
    let exp_factor = 1u64.checked_shl(attempt).unwrap_or(u64::MAX);
    let base_ms = base_delay.as_millis() as u64;
    let max_ms = max_delay.as_millis() as u64;
    let capped_ms = base_ms.saturating_mul(exp_factor).min(max_ms);

    let half_ms = capped_ms / 2;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(54321);

    let jitter = if half_ms > 0 {
        (nanos as u64) % (half_ms + 1)
    } else {
        0
    };

    let total_ms = (half_ms + jitter).max(1).min(max_ms);
    Duration::from_millis(total_ms)
}

/// Helper to parse standard HTTP `Retry-After` headers (integer seconds or RFC 2822 date).
fn parse_retry_after(header: Option<&reqwest::header::HeaderValue>) -> Option<Duration> {
    let header_str = header?.to_str().ok()?.trim();
    if let Ok(secs) = header_str.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc2822(header_str) {
        let diff = dt.signed_duration_since(chrono::Utc::now());
        if diff.num_milliseconds() > 0 {
            return Some(Duration::from_millis(diff.num_milliseconds() as u64));
        }
    }
    None
}

/// Truncate long error bodies to prevent log pollution.
fn truncate_text(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        s
    } else {
        &s[..max_len]
    }
}

/// Classification of notification delivery attempt outcome.
#[derive(Debug)]
pub enum DispatchOutcome {
    Success,
    PermanentFailure {
        error: String,
        status_code: Option<u16>,
    },
    TransientFailure {
        error: String,
        status_code: Option<u16>,
        retry_after: Option<Duration>,
    },
}

fn escape_telegram_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Formats a telegram message text with high-contrast operational status indicators.
pub fn format_telegram_message(payload: &AlertPayload) -> String {
    let is_recovery = payload.new_status == "UP"
        && (payload.old_status == "DOWN" || payload.old_status == "DEGRADED");
    let is_domain_alert = payload.new_status == "DOMAIN_EXPIRING_SOON";
    let is_ssl_alert = payload.new_status == "SSL_EXPIRING_SOON";
    let is_degraded = payload.new_status.to_uppercase() == "DEGRADED";

    let name = escape_telegram_html(&payload.monitor_name);
    let url = escape_telegram_html(&payload.monitor_url);

    if is_recovery {
        let latency_str = payload
            .response_time_ms
            .map(|ms| format!("{ms} ms"))
            .unwrap_or_else(|| "N/A".to_string());
        let duration_str =
            escape_telegram_html(payload.downtime_duration.as_deref().unwrap_or("N/A"));

        format!(
            "🟢 <b>[RESOLVED]</b> <b>{name}</b> is back online\n\
             🔗 <b>Target:</b> <code>{url}</code>\n\
             ⚡ <b>Latency:</b> {latency_str}\n\
             ⏱️ <b>Downtime Duration:</b> {duration_str}\n\
             🕒 <b>Timestamp:</b> <code>{}</code>",
            payload.timestamp
        )
    } else if is_domain_alert || is_ssl_alert {
        let label = if is_ssl_alert {
            "SSL EXPIRING"
        } else {
            "DOMAIN EXPIRING"
        };
        let days_str = payload
            .domain_days_remaining
            .map(|d| format!("{d} days"))
            .unwrap_or_else(|| "N/A".to_string());
        let expires_str =
            escape_telegram_html(payload.domain_expires_at.as_deref().unwrap_or("N/A"));

        format!(
            "⚠️ <b>[{label}]</b> <b>{name}</b>\n\
             🔗 <b>Target:</b> <code>{url}</code>\n\
             📅 <b>Days Remaining:</b> {days_str}\n\
             ⏰ <b>Expiration Date:</b> <code>{expires_str}</code>\n\
             🕒 <b>Timestamp:</b> <code>{}</code>",
            payload.timestamp
        )
    } else if payload.new_status == "DOWN" {
        let error_str = payload
            .error_msg
            .as_deref()
            .unwrap_or("Check failed or timed out");
        let http_status_str = payload
            .http_status
            .map(|st| st.to_string())
            .unwrap_or_else(|| "N/A".to_string());

        format!(
            "🔴 <b>[DOWN]</b> <b>{name}</b> is unreachable!\n\
             🔗 <b>Target:</b> <code>{url}</code>\n\
             ⚠️ <b>HTTP Status:</b> {http_status_str}\n\
             ❌ <b>Error:</b> <code>{}</code>\n\
             🕒 <b>Timestamp:</b> <code>{}</code>",
            escape_telegram_html(error_str),
            payload.timestamp
        )
    } else if is_degraded {
        let latency_str = payload
            .response_time_ms
            .map(|ms| format!("{ms} ms"))
            .unwrap_or_else(|| "N/A".to_string());
        let error_str = payload
            .error_msg
            .as_deref()
            .unwrap_or("Performance or SLA degraded");

        format!(
            "🟡 <b>[DEGRADED]</b> <b>{name}</b> performance is degraded\n\
             🔗 <b>Target:</b> <code>{url}</code>\n\
             ⚡ <b>Latency:</b> {latency_str}\n\
             ⚠️ <b>Details:</b> <code>{}</code>\n\
             🕒 <b>Timestamp:</b> <code>{}</code>",
            escape_telegram_html(error_str),
            payload.timestamp
        )
    } else if payload.new_status == "UP" {
        let latency_str = payload
            .response_time_ms
            .map(|ms| format!("{ms} ms"))
            .unwrap_or_else(|| "N/A".to_string());

        format!(
            "🟢 <b>[UP]</b> <b>{name}</b> is operational\n\
             🔗 <b>Target:</b> <code>{url}</code>\n\
             ⚡ <b>Latency:</b> {latency_str}\n\
             🕒 <b>Timestamp:</b> <code>{}</code>",
            payload.timestamp
        )
    } else {
        format!(
            "ℹ️ <b>[{}]</b> <b>{name}</b>\n\
             🔗 <b>Target:</b> <code>{url}</code>\n\
             🕒 <b>Timestamp:</b> <code>{}</code>",
            escape_telegram_html(&payload.new_status),
            payload.timestamp
        )
    }
}

/// Builds the destination URL and JSON payload for a notification channel.
pub fn build_channel_request(
    channel: &NotificationChannel,
    payload: &AlertPayload,
) -> Result<(String, Value), String> {
    let channel_type = channel.channel_type.to_lowercase();
    let custom_tpl = if !channel.use_default_template {
        channel
            .custom_template
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    } else {
        None
    };

    match channel_type.as_str() {
        "discord" => {
            if channel.url.trim().is_empty() {
                return Err("Discord webhook URL is empty".to_string());
            }
            let body = if let Some(tpl) = custom_tpl {
                let rendered = crate::checker::alerting::render_custom_template(tpl, payload);
                match serde_json::from_str::<Value>(&rendered) {
                    Ok(v) if v.is_object() => v,
                    _ => json!({ "content": rendered }),
                }
            } else {
                crate::checker::alerting::format_discord_payload(
                    payload,
                    channel.bot_name.as_deref(),
                    channel.avatar_url.as_deref(),
                )
            };
            Ok((channel.url.clone(), body))
        }
        "slack" => {
            if channel.url.trim().is_empty() {
                return Err("Slack webhook URL is empty".to_string());
            }
            let body = if let Some(tpl) = custom_tpl {
                let rendered = crate::checker::alerting::render_custom_template(tpl, payload);
                match serde_json::from_str::<Value>(&rendered) {
                    Ok(v) if v.is_object() => v,
                    _ => json!({ "text": rendered }),
                }
            } else {
                crate::checker::alerting::format_slack_default(payload)
            };
            Ok((channel.url.clone(), body))
        }
        "telegram" => {
            let destination_url = if !channel.url.is_empty()
                && !channel.url.starts_with("https://api.telegram.org")
            {
                channel.url.clone()
            } else if let Some(ref token) = channel.telegram_bot_token {
                let t = token.trim();
                if !t.is_empty() {
                    format!("https://api.telegram.org/bot{t}/sendMessage")
                } else {
                    channel.url.clone()
                }
            } else {
                channel.url.clone()
            };

            if destination_url.trim().is_empty() {
                return Err("Telegram destination URL / Bot Token is empty".to_string());
            }

            let text = if let Some(tpl) = custom_tpl {
                escape_telegram_html(&crate::checker::alerting::render_custom_template(
                    tpl, payload,
                ))
            } else {
                format_telegram_message(payload)
            };
            let mut body_map = serde_json::Map::new();
            body_map.insert("text".to_string(), serde_json::Value::String(text));
            body_map.insert("parse_mode".to_string(), serde_json::Value::String("HTML".to_string()));

            if let Some(ref chat_id) = channel.telegram_chat_id {
                let cid = chat_id.trim();
                if !cid.is_empty() {
                    body_map.insert("chat_id".to_string(), serde_json::Value::String(cid.to_string()));
                }
            }

            if let Some(ref thread_id) = channel.telegram_thread_id {
                let tid_str = thread_id.trim();
                if !tid_str.is_empty() {
                    if let Ok(tid_num) = tid_str.parse::<i64>() {
                        body_map.insert("message_thread_id".to_string(), serde_json::json!(tid_num));
                    } else {
                        body_map.insert("message_thread_id".to_string(), serde_json::Value::String(tid_str.to_string()));
                    }
                }
            }

            Ok((destination_url, Value::Object(body_map)))
        }
        _ => {
            if channel.url.trim().is_empty() {
                return Err("Webhook URL is empty".to_string());
            }
            let body = if let Some(tpl) = custom_tpl {
                let rendered = crate::checker::alerting::render_custom_template(tpl, payload);
                if let Ok(json_val) = serde_json::from_str::<Value>(&rendered) {
                    json_val
                } else {
                    json!({ "text": rendered })
                }
            } else {
                crate::checker::alerting::format_generic_default(payload)
            };
            Ok((channel.url.clone(), body))
        }
    }
}

/// Executes a single HTTP dispatch attempt with status-aware error classification.
pub async fn execute_channel_attempt(
    client: &Client,
    channel: &NotificationChannel,
    payload: &AlertPayload,
) -> DispatchOutcome {
    let (url, body) = match build_channel_request(channel, payload) {
        Ok(req) => req,
        Err(err) => return DispatchOutcome::PermanentFailure { error: err, status_code: None },
    };

    let timeout = Duration::from_secs(5);
    let resp = client
        .post(&url)
        .timeout(timeout)
        .json(&body)
        .send()
        .await;

    match resp {
        Ok(r) => {
            let status = r.status();
            if status.is_success() {
                DispatchOutcome::Success
            } else if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                let retry_after = parse_retry_after(r.headers().get(reqwest::header::RETRY_AFTER));
                DispatchOutcome::TransientFailure {
                    error: "HTTP 429 Too Many Requests".to_string(),
                    status_code: Some(429),
                    retry_after,
                }
            } else if status.is_server_error() {
                DispatchOutcome::TransientFailure {
                    error: format!("HTTP {} Server Error", status.as_u16()),
                    status_code: Some(status.as_u16()),
                    retry_after: None,
                }
            } else if status.is_client_error() {
                DispatchOutcome::PermanentFailure {
                    error: format!("HTTP {} Client Error", status.as_u16()),
                    status_code: Some(status.as_u16()),
                }
            } else {
                DispatchOutcome::PermanentFailure {
                    error: format!("Unexpected HTTP status {}", status.as_u16()),
                    status_code: Some(status.as_u16()),
                }
            }
        }
        Err(err) => {
            if err.is_builder() {
                DispatchOutcome::PermanentFailure {
                    error: format!("Malformed request: {err}"),
                    status_code: None,
                }
            } else if err.is_timeout() {
                DispatchOutcome::TransientFailure {
                    error: format!("Request timed out: {err}"),
                    status_code: None,
                    retry_after: None,
                }
            } else if err.is_connect() {
                DispatchOutcome::TransientFailure {
                    error: format!("Connection refused/failed: {err}"),
                    status_code: None,
                    retry_after: None,
                }
            } else {
                DispatchOutcome::TransientFailure {
                    error: format!("Network IO error: {err}"),
                    status_code: None,
                    retry_after: None,
                }
            }
        }
    }
}

/// Detailed dispatch error information capturing attempt count, status code, and error message.
#[derive(Debug, Clone)]
pub struct ChannelDispatchError {
    pub message: String,
    pub status_code: Option<u16>,
    pub attempts: u32,
    #[allow(dead_code)]
    pub is_permanent: bool,
}

impl std::fmt::Display for ChannelDispatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ChannelDispatchError {}

/// Async retry execution wrapper around outbound notification requests.
///
/// Features:
/// - Enforces configurable retry limits (defaults to 3 attempts).
/// - Exponential backoff doubling on each consecutive transient failure (1s, 2s, 4s, 8s...).
/// - Equal randomized jitter to avoid thundering herd conditions.
/// - Status-aware filtering: retries 5xx, 429, timeouts, connection resets; aborts immediately on 4xx / malformed endpoints.
/// - Absolute maximum execution timeout across the complete retry lifecycle.
pub async fn dispatch_single_channel_with_retry(
    client: &Client,
    channel: &NotificationChannel,
    payload: &AlertPayload,
    config: Option<&RetryConfig>,
) -> Result<(), ChannelDispatchError> {
    let cfg = config.cloned().unwrap_or_default();
    let max_attempts = cfg.max_attempts.max(1);
    let start_time = std::time::Instant::now();

    let retry_future = async {
        for attempt in 1..=max_attempts {
            info!(
                channel_id = %channel.id,
                channel_name = %channel.name,
                channel_type = %channel.channel_type,
                attempt = attempt,
                max_attempts = max_attempts,
                "Dispatching notification attempt"
            );

            match execute_channel_attempt(client, channel, payload).await {
                DispatchOutcome::Success => {
                    info!(
                        channel_id = %channel.id,
                        channel_name = %channel.name,
                        channel_type = %channel.channel_type,
                        attempt = attempt,
                        "Notification delivered successfully"
                    );
                    return Ok(());
                }
                DispatchOutcome::PermanentFailure { error, status_code } => {
                    error!(
                        channel_id = %channel.id,
                        channel_name = %channel.name,
                        channel_type = %channel.channel_type,
                        attempt = attempt,
                        status_code = ?status_code,
                        "Permanent delivery failure encountered; aborting retries: {error}"
                    );
                    return Err(ChannelDispatchError {
                        message: error,
                        status_code,
                        attempts: attempt,
                        is_permanent: true,
                    });
                }
                DispatchOutcome::TransientFailure { error, status_code, retry_after } => {
                    if attempt < max_attempts {
                        let delay = if let Some(ra) = retry_after {
                            ra.min(cfg.max_delay)
                        } else {
                            calculate_retry_delay(attempt - 1, cfg.initial_delay, cfg.max_delay)
                        };

                        warn!(
                            channel_id = %channel.id,
                            channel_name = %channel.name,
                            channel_type = %channel.channel_type,
                            attempt = attempt,
                            max_attempts = max_attempts,
                            status_code = ?status_code,
                            delay_ms = delay.as_millis(),
                            "Transient delivery failure; scheduling retry after backoff delay: {error}"
                        );

                        tokio::time::sleep(delay).await;
                    } else {
                        error!(
                            channel_id = %channel.id,
                            channel_name = %channel.name,
                            channel_type = %channel.channel_type,
                            attempt = attempt,
                            max_attempts = max_attempts,
                            status_code = ?status_code,
                            "All notification retry attempts exhausted: {error}"
                        );
                        return Err(ChannelDispatchError {
                            message: format!("All {attempt} attempts exhausted: {error}"),
                            status_code,
                            attempts: attempt,
                            is_permanent: false,
                        });
                    }
                }
            }
        }

        Err(ChannelDispatchError {
            message: "Retry attempts exhausted without result".to_string(),
            status_code: None,
            attempts: max_attempts,
            is_permanent: false,
        })
    };

    match tokio::time::timeout(cfg.max_elapsed_time, retry_future).await {
        Ok(res) => res,
        Err(_) => {
            let elapsed_secs = start_time.elapsed().as_secs();
            error!(
                channel_id = %channel.id,
                channel_name = %channel.name,
                channel_type = %channel.channel_type,
                "Notification retry lifecycle timed out after {elapsed_secs}s"
            );
            Err(ChannelDispatchError {
                message: format!("Retry lifecycle timed out after {elapsed_secs}s"),
                status_code: None,
                attempts: max_attempts,
                is_permanent: false,
            })
        }
    }
}

/// Executes notification retry delivery and records success or failure state into SQLite.
pub async fn dispatch_channel_with_retry_and_logging(
    pool: &SqlitePool,
    client: &Client,
    channel: &NotificationChannel,
    payload: &AlertPayload,
    config: Option<&RetryConfig>,
) -> anyhow::Result<()> {
    match dispatch_single_channel_with_retry(client, channel, payload, config).await {
        Ok(()) => {
            let _ = crate::db::notifications::record_channel_success(pool, &channel.id).await;
            Ok(())
        }
        Err(err) => {
            let _ = crate::db::notifications::record_channel_failure(
                pool,
                &channel.id,
                &channel.name,
                &channel.channel_type,
                &err.message,
                err.status_code,
                err.attempts,
            )
            .await;
            Err(anyhow::anyhow!(err.message))
        }
    }
}

/// Send an alert payload to a single notification channel with exponential backoff retries.
#[allow(dead_code)]
pub async fn dispatch_single_channel(
    client: &Client,
    channel: &NotificationChannel,
    payload: &AlertPayload,
) -> anyhow::Result<()> {
    dispatch_single_channel_with_retry(client, channel, payload, None)
        .await
        .map_err(|e| anyhow::anyhow!(e.message))
}

/// Dispatches an instant test alert to verify a notification channel's configuration.
pub async fn test_channel(client: &Client, channel: &NotificationChannel) -> anyhow::Result<()> {
    test_channel_with_config(client, channel, None).await
}

/// Dispatches an instant test alert with custom retry configuration.
pub async fn test_channel_with_config(
    client: &Client,
    channel: &NotificationChannel,
    config: Option<&RetryConfig>,
) -> anyhow::Result<()> {
    let test_payload = AlertPayload {
        monitor_id: Some("test-channel-probe".to_string()),
        monitor_name: "KeepUp Health Monitor".to_string(),
        monitor_url: "https://keepup.local".to_string(),
        new_status: "UP".to_string(),
        old_status: "DOWN".to_string(),
        http_status: Some(200),
        response_time_ms: Some(42),
        error_msg: None,
        downtime_duration: Some("1m 15s".to_string()),
        downtime_duration_secs: Some(75),
        timestamp: chrono::Utc::now().to_rfc3339(),
        domain_days_remaining: None,
        domain_expires_at: None,
    };

    dispatch_single_channel_with_retry(client, channel, &test_payload, config)
        .await
        .map_err(|e| anyhow::anyhow!(e.message))
}

/// Centralized alert dispatcher across unified `notification_channels`.
pub async fn dispatch_alerts(pool: &SqlitePool, client: &Client, payload: &AlertPayload) {
    if let Some(ref mid) = payload.monitor_id {
        if crate::db::maintenance::is_monitor_in_maintenance(pool, mid)
            .await
            .unwrap_or(false)
        {
            info!(
                monitor = %payload.monitor_name,
                monitor_id = %mid,
                "Bypassing notification channels dispatch: monitor is currently inside an active maintenance window"
            );
            return;
        }
    }

    let channels = match &payload.monitor_id {
        Some(mid) => crate::db::notifications::get_effective_channels_for_monitor(pool, mid)
            .await
            .unwrap_or_default(),
        None => crate::db::notifications::list_channels(pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|c| c.is_default && c.is_enabled)
            .collect(),
    };

    let is_recovery = payload.new_status == "UP"
        && (payload.old_status == "DOWN" || payload.old_status == "DEGRADED");

    let eligible: Vec<NotificationChannel> = channels
        .into_iter()
        .filter(|c| c.is_enabled && (!is_recovery || c.notify_on_resolve))
        .collect();

    if !eligible.is_empty() {
        info!(
            count = eligible.len(),
            monitor = %payload.monitor_name,
            transition = %format!("{} → {}", payload.old_status, payload.new_status),
            "Dispatching alerts across parallel notification channels"
        );

        for channel in eligible {
            let client = client.clone();
            let payload = payload.clone();
            let pool = pool.clone();
            let channel_name = channel.name.clone();
            let channel_id = channel.id.clone();
            let channel_type = channel.channel_type.clone();

            tokio::spawn(async move {
                match dispatch_channel_with_retry_and_logging(&pool, &client, &channel, &payload, None).await {
                    Ok(()) => {
                        info!(
                            channel_id = %channel_id,
                            channel_name = %channel_name,
                            channel_type = %channel_type,
                            "Alert delivered successfully"
                        );
                    }
                    Err(err) => {
                        error!(
                            channel_id = %channel_id,
                            channel_name = %channel_name,
                            channel_type = %channel_type,
                            "Alert delivery failed: {err}"
                        );
                    }
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn test_telegram_message_uses_safe_html_escaping() {
        let payload = AlertPayload {
            monitor_id: Some("m1".to_string()),
            monitor_name: "api_v2 <prod> & co".to_string(),
            monitor_url: "https://example.com/health_check?a=1&b=2".to_string(),
            new_status: "DOWN".to_string(),
            old_status: "UP".to_string(),
            http_status: Some(502),
            response_time_ms: Some(120),
            error_msg: Some("upstream <bad_gateway> & timeout".to_string()),
            downtime_duration: None,
            downtime_duration_secs: None,
            timestamp: "2026-11-02T00:00:00Z".to_string(),
            domain_days_remaining: None,
            domain_expires_at: None,
        };

        let msg = format_telegram_message(&payload);
        assert!(msg.contains("<b>[DOWN]</b>"), "Telegram message must use HTML bold tags: {msg}");
        assert!(msg.contains("api_v2 &lt;prod&gt; &amp; co"));
        assert!(msg.contains("upstream &lt;bad_gateway&gt; &amp; timeout"));
    }

    #[tokio::test]
    async fn test_dispatch_single_channel_redacts_upstream_error_body() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                if let Ok((mut sock, _)) = listener.accept().await {
                    let mut buf = [0u8; 1024];
                    let _ = sock.read(&mut buf).await;
                    let resp = "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 25\r\n\r\nSECRET_INTERNAL_BODY_LEAK";
                    let _ = sock.write_all(resp.as_bytes()).await;
                }
            }
        });

        let channel = NotificationChannel {
            id: "c1".to_string(),
            name: "Test Hook".to_string(),
            channel_type: "webhook".to_string(),
            url: format!("http://{addr}/hook"),
            is_default: false,
            is_enabled: true,
            use_default_template: true,
            notify_on_resolve: true,
            custom_template: None,
            created_at: "2026-11-02T00:00:00Z".to_string(),
            bot_name: None,
            avatar_url: None,
            telegram_bot_token: None,
            telegram_chat_id: None,
            telegram_thread_id: None,
            last_error: None,
            last_error_at: None,
        };

        let err = test_channel(&reqwest::Client::new(), &channel)
            .await
            .expect_err("500 response must fail");
        let err_msg = err.to_string();
        assert!(err_msg.contains("500"));
        assert!(
            !err_msg.contains("SECRET_INTERNAL_BODY_LEAK"),
            "upstream response body must not be echoed in channel error: {err_msg}"
        );
    }

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use sqlx::sqlite::SqlitePoolOptions;

    #[test]
    fn test_calculate_retry_delay_exponential_growth_and_jitter() {
        let base = Duration::from_millis(1000);
        let max = Duration::from_millis(30000);

        // Attempt 0: 1000ms base, half is 500ms, jitter in [0..=500] -> range [500..=1000]
        for _ in 0..50 {
            let delay = calculate_retry_delay(0, base, max);
            assert!(delay.as_millis() >= 500 && delay.as_millis() <= 1000);
        }

        // Attempt 1: 2000ms base, half is 1000ms, jitter in [0..=1000] -> range [1000..=2000]
        for _ in 0..50 {
            let delay = calculate_retry_delay(1, base, max);
            assert!(delay.as_millis() >= 1000 && delay.as_millis() <= 2000);
        }

        // Attempt 2: 4000ms base, half is 2000ms, jitter in [0..=2000] -> range [2000..=4000]
        for _ in 0..50 {
            let delay = calculate_retry_delay(2, base, max);
            assert!(delay.as_millis() >= 2000 && delay.as_millis() <= 4000);
        }

        // Attempt 10: saturated at max (30000ms), half is 15000ms -> range [15000..=30000]
        for _ in 0..50 {
            let delay = calculate_retry_delay(10, base, max);
            assert!(delay.as_millis() >= 15000 && delay.as_millis() <= 30000);
        }
    }

    async fn create_test_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("Failed to run migrations");

        pool
    }

    fn sample_test_payload() -> AlertPayload {
        AlertPayload {
            monitor_id: Some("test-mon-1".to_string()),
            monitor_name: "Production Gateway".to_string(),
            monitor_url: "https://gateway.prod".to_string(),
            new_status: "DOWN".to_string(),
            old_status: "UP".to_string(),
            http_status: Some(500),
            response_time_ms: Some(120),
            error_msg: Some("HTTP 500 Internal Server Error".to_string()),
            downtime_duration: None,
            downtime_duration_secs: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
            domain_days_remaining: None,
            domain_expires_at: None,
        }
    }

    #[tokio::test]
    async fn test_notification_retries_on_500_server_error_and_failure_logging() {
        let pool = create_test_pool().await;
        let client = reqwest::Client::new();

        // 1. Setup mock server returning HTTP 500
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let timestamps = Arc::new(Mutex::new(Vec::<std::time::Instant>::new()));
        let timestamps_clone = timestamps.clone();

        tokio::spawn(async move {
            loop {
                if let Ok((mut socket, _)) = listener.accept().await {
                    let mut buf = [0u8; 1024];
                    let _ = socket.read(&mut buf).await;
                    timestamps_clone.lock().await.push(std::time::Instant::now());
                    let resp = "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 17\r\n\r\n500 server error";
                    let _ = socket.write_all(resp.as_bytes()).await;
                }
            }
        });

        // 2. Insert channel
        let channel = NotificationChannel {
            id: uuid::Uuid::new_v4().to_string(),
            name: "DevOps OpsGenie Webhook".to_string(),
            channel_type: "webhook".to_string(),
            url: format!("http://{addr}/webhook"),
            is_default: true,
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
        crate::db::notifications::create_channel(&pool, channel.clone())
            .await
            .unwrap();

        // 3. Dispatch with fast test retry config (3 attempts, 40ms initial delay)
        let retry_config = RetryConfig {
            max_attempts: 3,
            initial_delay: Duration::from_millis(40),
            max_delay: Duration::from_millis(300),
            max_elapsed_time: Duration::from_secs(5),
        };

        let payload = sample_test_payload();
        let res = dispatch_channel_with_retry_and_logging(&pool, &client, &channel, &payload, Some(&retry_config)).await;
        assert!(res.is_err());

        // 4. Inspect attempt count and backoff intervals
        let recorded = timestamps.lock().await.clone();
        assert_eq!(recorded.len(), 3, "Expected exactly 3 retry attempts");
        let interval_1 = recorded[1].duration_since(recorded[0]);
        let interval_2 = recorded[2].duration_since(recorded[1]);
        assert!(interval_1.as_millis() >= 15);
        assert!(interval_2.as_millis() >= 30);

        // 5. Inspect database failure logging & channel warning state
        let updated_channel = crate::db::notifications::get_channel(&pool, &channel.id)
            .await
            .unwrap()
            .unwrap();
        assert!(updated_channel.last_error.is_some());
        let last_err = updated_channel.last_error.unwrap();
        assert!(last_err.contains("500") || last_err.contains("Server Error"));
        assert!(updated_channel.last_error_at.is_some());

        let failure_logs = crate::db::notifications::list_failure_logs(&pool, Some(&channel.id), 10)
            .await
            .unwrap();
        assert_eq!(failure_logs.len(), 1);
        assert_eq!(failure_logs[0].attempt_count, 3);
        assert_eq!(failure_logs[0].status_code, Some(500));
    }

    #[tokio::test]
    async fn test_notification_immediate_failure_on_400_client_error() {
        let pool = create_test_pool().await;
        let client = reqwest::Client::new();

        // 1. Setup mock server returning HTTP 400 Bad Request
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hit_counter = Arc::new(AtomicUsize::new(0));
        let hit_counter_clone = hit_counter.clone();

        tokio::spawn(async move {
            loop {
                if let Ok((mut socket, _)) = listener.accept().await {
                    let mut buf = [0u8; 1024];
                    let _ = socket.read(&mut buf).await;
                    hit_counter_clone.fetch_add(1, Ordering::SeqCst);
                    let resp = "HTTP/1.1 400 Bad Request\r\nContent-Length: 15\r\n\r\n400 bad payload";
                    let _ = socket.write_all(resp.as_bytes()).await;
                }
            }
        });

        // 2. Insert channel
        let channel = NotificationChannel {
            id: uuid::Uuid::new_v4().to_string(),
            name: "Invalid Client Webhook".to_string(),
            channel_type: "webhook".to_string(),
            url: format!("http://{addr}/webhook"),
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
        crate::db::notifications::create_channel(&pool, channel.clone())
            .await
            .unwrap();

        // 3. Dispatch with retry config configured for up to 5 attempts
        let retry_config = RetryConfig {
            max_attempts: 5,
            initial_delay: Duration::from_millis(50),
            max_delay: Duration::from_millis(500),
            max_elapsed_time: Duration::from_secs(5),
        };

        let payload = sample_test_payload();
        let res = dispatch_channel_with_retry_and_logging(&pool, &client, &channel, &payload, Some(&retry_config)).await;
        assert!(res.is_err());

        // 4. Must have executed EXACTLY 1 attempt without retries!
        assert_eq!(hit_counter.load(Ordering::SeqCst), 1, "400 Client error must fail immediately without retry");

        // 5. Inspect database failure logging & channel warning state
        let updated_channel = crate::db::notifications::get_channel(&pool, &channel.id)
            .await
            .unwrap()
            .unwrap();
        assert!(updated_channel.last_error.is_some());
        let last_err = updated_channel.last_error.unwrap();
        assert!(last_err.contains("400") || last_err.contains("Client Error"));

        let failure_logs = crate::db::notifications::list_failure_logs(&pool, Some(&channel.id), 10)
            .await
            .unwrap();
        assert_eq!(failure_logs.len(), 1);
        assert_eq!(failure_logs[0].attempt_count, 1);
        assert_eq!(failure_logs[0].status_code, Some(400));
    }

    #[tokio::test]
    async fn test_notification_rate_limit_429_retry_after_and_recovery() {
        let pool = create_test_pool().await;
        let client = reqwest::Client::new();

        // 1. Setup mock server returning 429 on first hit with Retry-After: 0, then 200 on second hit
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hit_counter = Arc::new(AtomicUsize::new(0));
        let hit_counter_clone = hit_counter.clone();

        tokio::spawn(async move {
            loop {
                if let Ok((mut socket, _)) = listener.accept().await {
                    let mut buf = [0u8; 1024];
                    let _ = socket.read(&mut buf).await;
                    let count = hit_counter_clone.fetch_add(1, Ordering::SeqCst);
                    if count == 0 {
                        let resp = "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 0\r\nContent-Length: 12\r\n\r\nrate limited";
                        let _ = socket.write_all(resp.as_bytes()).await;
                    } else {
                        let resp = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
                        let _ = socket.write_all(resp.as_bytes()).await;
                    }
                }
            }
        });

        // 2. Insert channel with pre-existing warning state to verify recovery
        let channel = NotificationChannel {
            id: uuid::Uuid::new_v4().to_string(),
            name: "Rate Limited Webhook".to_string(),
            channel_type: "webhook".to_string(),
            url: format!("http://{addr}/webhook"),
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
            last_error: Some("Previous intermittent timeout".to_string()),
            last_error_at: Some(chrono::Utc::now().to_rfc3339()),
        };
        crate::db::notifications::create_channel(&pool, channel.clone())
            .await
            .unwrap();

        // 3. Dispatch with retry config
        let retry_config = RetryConfig {
            max_attempts: 3,
            initial_delay: Duration::from_millis(50),
            max_delay: Duration::from_millis(300),
            max_elapsed_time: Duration::from_secs(5),
        };

        let payload = sample_test_payload();
        let res = dispatch_channel_with_retry_and_logging(&pool, &client, &channel, &payload, Some(&retry_config)).await;
        assert!(res.is_ok(), "Expected delivery to recover after 429 Retry-After");
        assert_eq!(hit_counter.load(Ordering::SeqCst), 2, "Expected 2 attempts (first 429, second 200)");

        // 4. Verify channel warning state was cleared on successful delivery
        let updated_channel = crate::db::notifications::get_channel(&pool, &channel.id)
            .await
            .unwrap()
            .unwrap();
        assert!(updated_channel.last_error.is_none(), "Expected last_error to be cleared on success");
        assert!(updated_channel.last_error_at.is_none());
    }
}
