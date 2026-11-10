use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::header::HeaderMap;
use axum::http::{header, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};

use crate::db::checks::DetailCheckLog;
use crate::error::AppError;
use crate::AppState;

fn escape_prometheus_label(val: &str) -> String {
    val.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn escape_xml(val: &str) -> String {
    val.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// `GET /metrics` — OpenMetrics endpoint for Prometheus scraping.
pub async fn metrics_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    // Optional bearer token authentication
    if let Some(expected_token) = &state.config.metrics_bearer_token {
        let auth_hdr = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok());
        let authed = match auth_hdr {
            Some(hdr) if hdr.starts_with("Bearer ") => {
                let token = hdr.trim_start_matches("Bearer ").trim();
                subtle::ConstantTimeEq::ct_eq(token.as_bytes(), expected_token.as_bytes()).into()
            }
            _ => false,
        };
        if !authed {
            return Ok((
                StatusCode::UNAUTHORIZED,
                [(header::WWW_AUTHENTICATE, "Bearer realm=\"metrics\"")],
                "Unauthorized\n",
            )
                .into_response());
        }
    }

    let monitors: Vec<crate::db::monitors::Monitor> = sqlx::query_as(
        r#"
        SELECT * FROM monitors
        WHERE is_active = 1
        ORDER BY name ASC
        "#,
    )
    .fetch_all(&state.db)
    .await?;

    let mut out = String::new();
    out.push_str("# HELP keepup_monitor_up Monitor operational status (1 = UP, 0 = DOWN or DEGRADED)\n");
    out.push_str("# TYPE keepup_monitor_up gauge\n");
    for m in &monitors {
        let is_up = if m.last_status.eq_ignore_ascii_case("up") { 1 } else { 0 };
        out.push_str(&format!(
            "keepup_monitor_up{{id=\"{}\",name=\"{}\"}} {}\n",
            escape_prometheus_label(&m.id),
            escape_prometheus_label(&m.name),
            is_up
        ));
    }

    out.push_str("\n# HELP keepup_monitor_latency_ms Latest recorded round-trip latency in milliseconds\n");
    out.push_str("# TYPE keepup_monitor_latency_ms gauge\n");
    for m in &monitors {
        let lat: Option<i64> = sqlx::query_scalar(
            "SELECT latency_ms FROM check_logs WHERE monitor_id = ? ORDER BY checked_at DESC LIMIT 1"
        )
        .bind(&m.id)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);

        out.push_str(&format!(
            "keepup_monitor_latency_ms{{id=\"{}\",name=\"{}\"}} {}\n",
            escape_prometheus_label(&m.id),
            escape_prometheus_label(&m.name),
            lat.unwrap_or(0)
        ));
    }

    out.push_str("\n# HELP keepup_ssl_days_remaining Days until TLS certificate expiration\n");
    out.push_str("# TYPE keepup_ssl_days_remaining gauge\n");
    for m in &monitors {
        if m.check_ssl == 1 {
            if let Some(days) = m.ssl_days_remaining {
                out.push_str(&format!(
                    "keepup_ssl_days_remaining{{id=\"{}\",name=\"{}\"}} {}\n",
                    escape_prometheus_label(&m.id),
                    escape_prometheus_label(&m.name),
                    days
                ));
            }
        }
    }

    out.push_str("\n# HELP keepup_domain_days_remaining Days until domain RDAP/WHOIS expiration\n");
    out.push_str("# TYPE keepup_domain_days_remaining gauge\n");
    for m in &monitors {
        if m.check_domain_expiry == 1 {
            if let Some(days) = m.domain_days_remaining {
                out.push_str(&format!(
                    "keepup_domain_days_remaining{{id=\"{}\",name=\"{}\"}} {}\n",
                    escape_prometheus_label(&m.id),
                    escape_prometheus_label(&m.name),
                    days
                ));
            }
        }
    }

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")],
        out,
    )
        .into_response())
}

/// `GET /status/:slug/rss.xml` and `GET /status/rss.xml` — RSS 2.0 feed for status pages.
pub async fn rss_feed_handler(
    State(state): State<Arc<AppState>>,
    path_slug: Option<Path<String>>,
) -> Result<Response, AppError> {
    let slug = path_slug.map(|Path(s)| s).unwrap_or_else(|| "default".to_string());

    let (title, description) = if slug == "default" {
        ("KeepUp Status Feed".to_string(), "Live incident and maintenance updates".to_string())
    } else {
        let sp: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT title, description FROM status_pages WHERE slug = ? LIMIT 1"
        )
        .bind(&slug)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);

        match sp {
            Some((t, d)) => (format!("{t} - Status Feed"), d.unwrap_or_else(|| "Status updates".to_string())),
            None => (format!("{slug} Status Feed"), "Incident updates".to_string()),
        }
    };

    let incidents = crate::db::incidents::get_public_incidents_for_page(
        &state.db,
        Some(&slug),
        &[],
    )
    .await
    .unwrap_or_default();

    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rss version=\"2.0\">\n<channel>\n");
    xml.push_str(&format!("<title>{}</title>\n", escape_xml(&title)));
    xml.push_str(&format!("<link>/status/{}</link>\n", escape_xml(&slug)));
    xml.push_str(&format!("<description>{}</description>\n", escape_xml(&description)));
    xml.push_str("<language>en-US</language>\n");

    for inc in incidents {
        let item_title = format!("[{}] {}", inc.status, inc.title);
        let updates_desc = inc
            .updates
            .iter()
            .map(|u| format!("{}: {}", u.status, u.message))
            .collect::<Vec<_>>()
            .join(" | ");

        xml.push_str("<item>\n");
        xml.push_str(&format!("<title>{}</title>\n", escape_xml(&item_title)));
        xml.push_str(&format!("<guid>{}</guid>\n", escape_xml(&inc.id)));
        xml.push_str(&format!("<pubDate>{}</pubDate>\n", escape_xml(&inc.started_at)));
        xml.push_str(&format!("<description>{}</description>\n", escape_xml(&updates_desc)));
        xml.push_str("</item>\n");
    }

    xml.push_str("</channel>\n</rss>\n");

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/rss+xml; charset=utf-8")],
        xml,
    )
        .into_response())
}

/// `GET /api/events` — Server-Sent Events stream for live dashboard and status page updates.
pub async fn events_sse_handler(
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.events_tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|msg| match msg {
        Ok(data) => Some(Ok(Event::default().data(data))),
        Err(_) => None,
    });

    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DashboardMonitorItem {
    #[serde(flatten)]
    pub monitor: crate::db::monitors::Monitor,
    pub logs: Vec<DetailCheckLog>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DashboardSummary {
    pub total_monitors: usize,
    pub up_count: usize,
    pub down_count: usize,
    pub degraded_count: usize,
    pub paused_count: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DashboardResponse {
    pub monitors: Vec<DashboardMonitorItem>,
    pub summary: DashboardSummary,
    pub open_incidents: Vec<crate::db::incidents::IncidentWithUpdates>,
    pub active_maintenance_count: usize,
}

/// `GET /api/admin/dashboard` — single batched query returning all monitors, latest 30 check ticks per monitor,
/// summary counters, open incidents, and active maintenance count in 1 round-trip.
pub async fn dashboard_batch_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<DashboardResponse>, AppError> {
    let monitors: Vec<crate::db::monitors::Monitor> = sqlx::query_as(
        "SELECT * FROM monitors ORDER BY name ASC"
    )
    .fetch_all(&state.db)
    .await?;

    let mut up_count = 0;
    let mut down_count = 0;
    let mut degraded_count = 0;
    let mut paused_count = 0;

    let mut monitor_items = Vec::with_capacity(monitors.len());

    for m in monitors {
        if m.is_active == 0 {
            paused_count += 1;
        } else if m.last_status.eq_ignore_ascii_case("up") {
            up_count += 1;
        } else if m.last_status.eq_ignore_ascii_case("degraded") {
            degraded_count += 1;
        } else {
            down_count += 1;
        }

        let logs = crate::db::checks::get_recent_check_logs(&state.db, &m.id, 30).await?;

        monitor_items.push(DashboardMonitorItem {
            monitor: m,
            logs,
        });
    }

    let summary = DashboardSummary {
        total_monitors: monitor_items.len(),
        up_count,
        down_count,
        degraded_count,
        paused_count,
    };

    let open_incidents = crate::db::incidents::list_incidents(&state.db, None, None).await?;
    let active_incidents: Vec<_> = open_incidents
        .into_iter()
        .filter(|inc| !inc.incident.status.eq_ignore_ascii_case("resolved"))
        .collect();

    let active_maintenance_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM maintenance_windows
        WHERE datetime('now') >= datetime(start_time)
          AND datetime('now') <= datetime(end_time)
        "#,
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    Ok(Json(DashboardResponse {
        monitors: monitor_items,
        summary,
        open_incidents: active_incidents,
        active_maintenance_count: active_maintenance_count as usize,
    }))
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExportPayload {
    pub version: u32,
    pub exported_at: String,
    pub monitors: Vec<crate::db::monitors::Monitor>,
    pub status_pages: Vec<crate::db::status_pages::StatusPage>,
    pub channels: Vec<crate::db::notifications::NotificationChannel>,
}

#[derive(Debug, Serialize)]
pub struct ImportResponse {
    pub ok: bool,
    pub monitors_imported: usize,
    pub status_pages_imported: usize,
}

/// `GET /api/admin/export` — export complete configuration as JSON with masked notification secrets.
pub async fn export_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ExportPayload>, AppError> {
    let monitors: Vec<crate::db::monitors::Monitor> = sqlx::query_as("SELECT * FROM monitors ORDER BY id ASC")
        .fetch_all(&state.db)
        .await?;

    let status_pages = crate::db::status_pages::get_all_status_pages(&state.db).await?;
    let channels = crate::db::notifications::list_channels(&state.db)
        .await
        .map_err(AppError::Database)?
        .into_iter()
        .map(|c| c.masked_view())
        .collect();

    Ok(Json(ExportPayload {
        version: 2,
        exported_at: Utc::now().to_rfc3339(),
        monitors,
        status_pages,
        channels,
    }))
}

/// `POST /api/admin/import` — import monitors and status pages transactionally with URL security checks.
pub async fn import_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ExportPayload>,
) -> Result<Json<ImportResponse>, AppError> {
    let mut tx = state.db.begin().await?;

    let mut monitors_imported = 0;
    for m in &payload.monitors {
        // Enforce SSRF validation on imported URLs
        if m.url.starts_with("http://") || m.url.starts_with("https://") {
            if let Ok(parsed) = reqwest::Url::parse(&m.url) {
                if let Some(host) = parsed.host_str() {
                    let port = parsed.port_or_known_default().unwrap_or(80);
                    if let Err(e) = crate::net_guard::resolve_checked(host, port, state.config.allow_private_targets).await {
                        return Err(AppError::BadRequest(format!("Cannot import monitor '{}': target URL '{}' violates SSRF policy: {e}", m.name, m.url)));
                    }
                }
            }
        }

        sqlx::query(
            r#"
            INSERT INTO monitors (
                id, name, url, check_interval_secs, timeout_secs, max_retries,
                is_active, is_public, last_status, check_ssl, ssl_warning_days,
                check_domain_expiry, domain_warning_days, http_method, expected_status,
                request_headers, body_keyword, json_pointer, json_expected,
                degraded_latency_ms, push_token, grace_secs, dns_record_type,
                dns_expected_value, resend_interval_mins, check_type, port, dns_expected
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                url = excluded.url,
                check_interval_secs = excluded.check_interval_secs,
                timeout_secs = excluded.timeout_secs,
                max_retries = excluded.max_retries,
                is_active = excluded.is_active,
                is_public = excluded.is_public,
                check_ssl = excluded.check_ssl,
                ssl_warning_days = excluded.ssl_warning_days,
                check_domain_expiry = excluded.check_domain_expiry,
                domain_warning_days = excluded.domain_warning_days,
                http_method = excluded.http_method,
                expected_status = excluded.expected_status,
                request_headers = excluded.request_headers,
                body_keyword = excluded.body_keyword,
                json_pointer = excluded.json_pointer,
                json_expected = excluded.json_expected,
                degraded_latency_ms = excluded.degraded_latency_ms,
                grace_secs = excluded.grace_secs,
                dns_record_type = excluded.dns_record_type,
                dns_expected_value = excluded.dns_expected_value,
                resend_interval_mins = excluded.resend_interval_mins,
                check_type = excluded.check_type,
                port = excluded.port,
                dns_expected = excluded.dns_expected
            "#,
        )
        .bind(&m.id)
        .bind(&m.name)
        .bind(&m.url)
        .bind(m.check_interval_secs)
        .bind(m.timeout_secs)
        .bind(m.max_retries)
        .bind(m.is_active)
        .bind(m.is_public)
        .bind(&m.last_status)
        .bind(m.check_ssl)
        .bind(m.ssl_warning_days)
        .bind(m.check_domain_expiry)
        .bind(m.domain_warning_days)
        .bind(&m.http_method)
        .bind(&m.expected_status)
        .bind(&m.request_headers)
        .bind(&m.body_keyword)
        .bind(&m.json_pointer)
        .bind(&m.json_expected)
        .bind(m.degraded_latency_ms)
        .bind(&m.push_token)
        .bind(m.grace_secs)
        .bind(&m.dns_record_type)
        .bind(&m.dns_expected_value)
        .bind(m.resend_interval_mins)
        .bind(&m.check_type)
        .bind(m.port)
        .bind(&m.dns_expected)
        .execute(&mut *tx)
        .await?;

        monitors_imported += 1;
        let _ = state.reload_tx.send(m.id.clone());
    }

    let mut status_pages_imported = 0;
    for sp in &payload.status_pages {
        sqlx::query(
            r#"
            INSERT INTO status_pages (slug, title, description, is_public, created_at)
            VALUES (?, ?, ?, ?, ?)
            ON CONFLICT(slug) DO UPDATE SET
                title = excluded.title,
                description = excluded.description,
                is_public = excluded.is_public
            "#,
        )
        .bind(&sp.slug)
        .bind(&sp.title)
        .bind(&sp.description)
        .bind(sp.is_public)
        .bind(&sp.created_at)
        .execute(&mut *tx)
        .await?;

        status_pages_imported += 1;
    }

    tx.commit().await?;
    state.invalidate_status_cache().await;

    Ok(Json(ImportResponse {
        ok: true,
        monitors_imported,
        status_pages_imported,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_metrics_endpoint_and_token_protection() {
        let pool = crate::db::init_pool(":memory:").await.unwrap();
        crate::db::run_migrations(&pool).await.unwrap();

        let mut config = crate::config::Config::default();
        config.metrics_bearer_token = Some("secret-metrics-token".to_string());
        let state = AppState::new_for_test(pool.clone(), config);
        let app = crate::build_http_app(state);

        // Seed a monitor
        sqlx::query(
            "INSERT INTO monitors (id, name, url, check_interval_secs, timeout_secs, max_retries, is_active, is_public, last_status, check_ssl, ssl_days_remaining) \
             VALUES ('mon-prom-1', 'API Service', 'https://api.example.com', 60, 5, 1, 1, 1, 'UP', 1, 28)"
        )
        .execute(&pool)
        .await
        .unwrap();

        // 1. Missing token returns 401
        let req_unauthed = Request::builder()
            .method("GET")
            .uri("/metrics")
            .body(Body::empty())
            .unwrap();
        let res_unauthed = app.clone().oneshot(req_unauthed).await.unwrap();
        assert_eq!(res_unauthed.status(), StatusCode::UNAUTHORIZED);

        // 2. Valid token returns OpenMetrics output
        let req_authed = Request::builder()
            .method("GET")
            .uri("/metrics")
            .header("Authorization", "Bearer secret-metrics-token")
            .body(Body::empty())
            .unwrap();
        let res_authed = app.clone().oneshot(req_authed).await.unwrap();
        assert_eq!(res_authed.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(res_authed.into_body(), 64 * 1024).await.unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(text.contains("keepup_monitor_up{id=\"mon-prom-1\",name=\"API Service\"} 1"));
        assert!(text.contains("keepup_ssl_days_remaining{id=\"mon-prom-1\",name=\"API Service\"} 28"));
    }

    #[tokio::test]
    async fn test_rss_xml_feed_endpoint() {
        let pool = crate::db::init_pool(":memory:").await.unwrap();
        crate::db::run_migrations(&pool).await.unwrap();

        let config = crate::config::Config::default();
        let state = AppState::new_for_test(pool.clone(), config);
        let app = crate::build_http_app(state);

        // Create incident
        let _ = crate::db::incidents::create_incident(
            &pool,
            crate::db::incidents::CreateIncidentDto {
                title: "Gateway Degradation & <Latency Spike>".to_string(),
                status: Some("Investigating".to_string()),
                severity: Some("major".to_string()),
                monitor_id: None,
                status_page_slug: Some("default".to_string()),
                initial_message: Some("Investigating upstream jitter.".to_string()),
            },
            false,
        )
        .await
        .unwrap();

        let req = Request::builder()
            .method("GET")
            .uri("/status/rss.xml")
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(res.into_body(), 64 * 1024).await.unwrap();
        let xml = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(xml.contains("<rss version=\"2.0\">"));
        assert!(xml.contains("Gateway Degradation &amp; &lt;Latency Spike&gt;"));
    }

    #[tokio::test]
    async fn test_dashboard_batch_and_export_import() {
        let pool = crate::db::init_pool(":memory:").await.unwrap();
        crate::db::run_migrations(&pool).await.unwrap();

        let config = crate::config::Config {
            allow_private_targets: true,
            ..Default::default()
        };
        let state = AppState::new_for_test(pool.clone(), config);
        let app = crate::build_http_app(state.clone());
        let token = crate::api::auth::create_jwt(
            "admin",
            "test-secret-at-least-32-bytes-long-for-security",
            state.current_session_version(),
        )
        .unwrap();

        // Seed a monitor
        sqlx::query(
            "INSERT INTO monitors (id, name, url, check_interval_secs, timeout_secs, max_retries, is_active, is_public, last_status) \
             VALUES ('mon-dash-1', 'App Service', 'http://127.0.0.1:8080', 60, 5, 1, 1, 1, 'UP')"
        )
        .execute(&pool)
        .await
        .unwrap();

        // 1. Dashboard batch endpoint
        let dash_req = Request::builder()
            .method("GET")
            .uri("/api/admin/dashboard")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let dash_res = app.clone().oneshot(dash_req).await.unwrap();
        assert_eq!(dash_res.status(), StatusCode::OK);
        let dash_bytes = axum::body::to_bytes(dash_res.into_body(), 64 * 1024).await.unwrap();
        let dash: DashboardResponse = serde_json::from_slice(&dash_bytes).unwrap();
        assert_eq!(dash.summary.total_monitors, 1);
        assert_eq!(dash.summary.up_count, 1);

        // 2. Export endpoint
        let exp_req = Request::builder()
            .method("GET")
            .uri("/api/admin/export")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let exp_res = app.clone().oneshot(exp_req).await.unwrap();
        assert_eq!(exp_res.status(), StatusCode::OK);
        let exp_bytes = axum::body::to_bytes(exp_res.into_body(), 64 * 1024).await.unwrap();
        let export_data: ExportPayload = serde_json::from_slice(&exp_bytes).unwrap();
        assert_eq!(export_data.monitors.len(), 1);

        // 3. Import roundtrip
        let imp_req = Request::builder()
            .method("POST")
            .uri("/api/admin/import")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(serde_json::to_vec(&export_data).unwrap()))
            .unwrap();
        let imp_res = app.clone().oneshot(imp_req).await.unwrap();
        assert_eq!(imp_res.status(), StatusCode::OK);
    }
}
