use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::db::monitors::{DailyUptime, HourlyLatency};
use crate::error::AppError;
use crate::AppState;

fn default_true() -> bool {
    true
}

// ── In-Memory TTL Status Cache ──────────────────────────────────────────────

/// Thread-safe in-memory TTL cache for public status queries.
#[derive(Clone)]
pub struct PublicStatusCache {
    entries: Arc<RwLock<HashMap<String, (Instant, PublicStatus)>>>,
    ttl: Duration,
}

impl Default for PublicStatusCache {
    fn default() -> Self {
        Self::new(Duration::from_secs(10))
    }
}

impl PublicStatusCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
            ttl,
        }
    }

    pub async fn get(&self, slug: &str) -> Option<PublicStatus> {
        let read = self.entries.read().await;
        if let Some((inserted_at, status)) = read.get(slug) {
            if inserted_at.elapsed() < self.ttl {
                return Some(status.clone());
            }
        }
        None
    }

    pub async fn set(&self, slug: &str, status: PublicStatus) {
        let mut write = self.entries.write().await;
        write.insert(slug.to_string(), (Instant::now(), status));
    }

    pub async fn invalidate(&self, slug: Option<&str>) {
        let mut write = self.entries.write().await;
        if let Some(s) = slug {
            write.remove(s);
        } else {
            write.clear();
        }
    }
}

// ── Response types ───────────────────────────────────────────────────────────

/// Monitored components grouped by category.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorGroup {
    pub name: String,
    pub monitors: Vec<PublicMonitor>,
}

/// Public representation of an incident in public status pages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicIncident {
    pub id: String,
    pub title: String,
    pub status: String,
    pub severity: String,
    pub monitor_id: Option<String>,
    pub started_at: String,
    pub resolved_at: Option<String>,
    #[serde(default)]
    pub updates: Vec<PublicIncidentUpdate>,
}

/// Public timeline entry for an ongoing or past incident.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicIncidentUpdate {
    pub id: String,
    pub incident_id: String,
    pub status: String,
    pub message: String,
    pub created_at: String,
}

/// Top-level public status response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicStatus {
    /// Status page title (e.g. "System Status").
    pub title: String,
    /// Status page description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Overall operational banner text.
    pub status: String,
    /// Optional status page theme configuration ("default", "oled", "midnight", "light").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_page_theme: Option<String>,
    /// Optional custom brand logo URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_logo_url: Option<String>,
    /// Active and scheduled maintenance windows.
    pub maintenance_windows: Vec<crate::db::maintenance::PublicMaintenanceWindow>,
    /// Public incident timeline.
    #[serde(default)]
    pub incidents: Vec<PublicIncident>,
    /// Monitored components grouped by category.
    pub groups: Vec<MonitorGroup>,
    /// Per-monitor summaries (flat list for backward compatibility).
    pub monitors: Vec<PublicMonitor>,
    /// Whether search engine indexing is permitted.
    #[serde(default = "default_true")]
    pub allow_indexing: bool,
    /// Optional custom SEO title tag override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta_title: Option<String>,
    /// Optional custom meta description for search engine snippet previews.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta_description: Option<String>,
    /// Optional URL for OpenGraph and Twitter social card preview thumbnails.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub og_image_url: Option<String>,
    /// Optional custom canonical URL header and tag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical_url: Option<String>,
}

/// Summary of a single public monitor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicMonitor {
    pub id: String,
    pub name: String,
    pub url: String,
    /// Current status derived from the most recent check.
    pub current_status: String,
    /// Most recent response latency (ms), if available.
    pub last_latency_ms: Option<i64>,
    /// 24-hour uptime percentage (0.0 – 100.0).
    pub uptime_24h: f64,
    /// 7-day daily uptime history buckets (Day -6 to Today).
    pub uptime_history: Vec<DailyUptime>,
    /// 24-hour hourly latency breakdown buckets (23 hours ago to current hour).
    pub latency_24h: Vec<HourlyLatency>,
    /// 24-hour response time percentiles (P50, P95, P99) and average.
    pub latency: crate::db::checks::LatencyPercentiles,
    #[serde(default = "default_check_type")]
    pub check_type: String,
}

fn default_check_type() -> String {
    "http".to_string()
}

// ── Row helpers for sqlx ─────────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
struct MonitorIdName {
    id: String,
    name: String,
    url: String,
    #[sqlx(default)]
    check_type: String,
}

#[derive(sqlx::FromRow)]
struct LatestCheck {
    is_up: i32,
    latency_ms: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct UptimeAgg {
    total: i64,
    up_count: i64,
}

#[derive(sqlx::FromRow)]
struct AssignedMonitorRow {
    id: String,
    name: String,
    url: String,
    group_name: String,
    #[allow(dead_code)]
    display_order: i64,
    #[sqlx(default)]
    check_type: String,
}

// ── Handler ──────────────────────────────────────────────────────────────────

/// Core implementation for fetching public status for a specific page slug.
pub async fn get_public_status_impl(
    state: &AppState,
    slug: &str,
) -> Result<PublicStatus, AppError> {
    if let Some(cached) = state.status_cache.get(slug).await {
        return Ok(cached);
    }

    let is_default = slug.eq_ignore_ascii_case("default");
    let page = crate::db::status_pages::get_status_page(&state.db, slug).await?;

    let (title, description, allow_indexing, meta_title, meta_description, og_image_url, canonical_url) = match (is_default, page) {
        (_, Some(p)) if p.is_public == 0 => {
            return Err(AppError::NotFound(format!("Status page '{slug}' not found")));
        }
        (_, Some(p)) => (
            p.title,
            p.description,
            p.allow_indexing,
            p.meta_title,
            p.meta_description,
            p.og_image_url,
            p.canonical_url,
        ),
        (true, None) => (
            "System Status".to_string(),
            Some("Current status and real-time performance of public services".to_string()),
            true,
            None,
            None,
            None,
            None,
        ),
        (false, None) => {
            return Err(AppError::NotFound(format!("Status page '{slug}' not found")));
        }
    };

    let total_assigned_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM status_page_monitors WHERE LOWER(status_page_slug) = LOWER(?)",
    )
    .bind(slug)
    .fetch_one(&state.db)
    .await?;

    let mut assigned = sqlx::query_as::<_, AssignedMonitorRow>(
        r#"
        SELECT 
            m.id, m.name, m.url, 
            spm.group_name, spm.display_order,
            m.check_type
        FROM status_page_monitors spm
        INNER JOIN monitors m ON m.id = spm.monitor_id
        WHERE LOWER(spm.status_page_slug) = LOWER(?)
          AND m.is_public = 1 AND m.is_active = 1
        ORDER BY spm.display_order ASC, m.name ASC
        "#,
    )
    .bind(slug)
    .fetch_all(&state.db)
    .await?;

    if total_assigned_count == 0 && is_default {
        let all_monitors = sqlx::query_as::<_, MonitorIdName>(
            "SELECT id, name, url, check_type FROM monitors WHERE is_public = 1 AND is_active = 1 ORDER BY name",
        )
        .fetch_all(&state.db)
        .await?;

        assigned = all_monitors
            .into_iter()
            .enumerate()
            .map(|(i, m)| AssignedMonitorRow {
                id: m.id,
                name: m.name,
                url: m.url,
                group_name: "Services".to_string(),
                display_order: i as i64,
                check_type: m.check_type,
            })
            .collect();
    }

    let monitor_ids: Vec<String> = assigned.iter().map(|m| m.id.clone()).collect();
    let batch_metrics =
        crate::db::checks::get_batch_public_metrics(&state.db, &monitor_ids).await?;

    let visible_id_set: std::collections::HashSet<&str> =
        assigned.iter().map(|m| m.id.as_str()).collect();
    let mut maintenance_windows =
        crate::db::maintenance::get_public_maintenance_windows(&state.db).await?;
    for mw in &mut maintenance_windows {
        mw.monitor_ids
            .retain(|mid| mid == "*" || visible_id_set.contains(mid.as_str()));
    }

    let mut public_monitors: Vec<PublicMonitor> = Vec::with_capacity(assigned.len());
    let mut groups_map: Vec<(String, Vec<PublicMonitor>)> = Vec::new();
    let mut any_down = false;
    let mut any_degraded = false;

    for m in assigned {
        let metrics = batch_metrics.get(&m.id);
        let in_maintenance = crate::db::maintenance::is_monitor_in_maintenance(&state.db, &m.id)
            .await
            .unwrap_or(false);

        let (current_status, last_latency_ms) = if in_maintenance {
            ("MAINTENANCE".to_string(), metrics.and_then(|b| b.last_latency_ms))
        } else {
            match metrics.and_then(|b| b.latest_is_up) {
                Some(true) => ("UP".to_string(), metrics.and_then(|b| b.last_latency_ms)),
                Some(false) => {
                    any_down = true;
                    ("DOWN".to_string(), metrics.and_then(|b| b.last_latency_ms))
                }
                None => ("UNKNOWN".to_string(), None),
            }
        };

        let uptime_24h = metrics.map(|b| b.uptime_24h).unwrap_or(100.0);
        if !in_maintenance {
            if current_status == "DEGRADED" || uptime_24h < 99.0 {
                any_degraded = true;
            }
        }

        let uptime_history = metrics.map(|b| b.uptime_history.clone()).unwrap_or_default();
        let latency_24h = metrics.map(|b| b.latency_24h.clone()).unwrap_or_default();
        let latency = metrics
            .map(|b| b.latency.clone())
            .unwrap_or(crate::db::checks::LatencyPercentiles {
                p50: 0,
                p95: 0,
                p99: 0,
                avg: 0,
            });

        let pub_m = PublicMonitor {
            id: m.id.clone(),
            name: m.name.clone(),
            url: m.url.clone(),
            current_status,
            last_latency_ms,
            uptime_24h: (uptime_24h * 100.0).round() / 100.0,
            uptime_history,
            latency_24h,
            latency,
            check_type: m.check_type.clone(),
        };

        public_monitors.push(pub_m.clone());

        let grp = if m.group_name.trim().is_empty() {
            "Services".to_string()
        } else {
            m.group_name.trim().to_string()
        };

        if let Some((_, list)) = groups_map.iter_mut().find(|(name, _)| name == &grp) {
            list.push(pub_m);
        } else {
            groups_map.push((grp, vec![pub_m]));
        }
    }

    let groups: Vec<MonitorGroup> = groups_map
        .into_iter()
        .map(|(name, monitors)| MonitorGroup { name, monitors })
        .collect();

    // ── Overall banner ───────────────────────────────────────────────
    let status = if any_down {
        "Major Outage".to_string()
    } else if any_degraded {
        "Partial System Outage".to_string()
    } else {
        "All Systems Operational".to_string()
    };

    let status_page_theme = std::env::var("STATUS_PAGE_THEME")
        .or_else(|_| std::env::var("KEEPUP_THEME"))
        .ok();

    let monitor_ids: Vec<String> = public_monitors.iter().map(|m| m.id.clone()).collect();
    let incidents = crate::db::incidents::get_public_incidents_for_page(
        &state.db,
        Some(slug),
        &monitor_ids,
    )
    .await
    .unwrap_or_default();

    let public_status = PublicStatus {
        title,
        description,
        status,
        status_page_theme,
        custom_logo_url: state.config.custom_logo_url.clone(),
        maintenance_windows,
        incidents,
        groups,
        monitors: public_monitors,
        allow_indexing,
        meta_title,
        meta_description,
        og_image_url,
        canonical_url,
    };

    state.status_cache.set(slug, public_status.clone()).await;

    Ok(public_status)
}

/// `GET /api/public/status` — aggregated health for the default status page with 5s cache.
pub async fn public_status(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, AppError> {
    let status = get_public_status_impl(&state, "default").await?;
    let robots_val = if status.allow_indexing {
        "index, follow"
    } else {
        "noindex, nofollow"
    };

    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::HeaderName::from_static("x-robots-tag"),
        axum::http::HeaderValue::from_static(robots_val),
    );
    if let Some(ref canon) = status.canonical_url {
        if let Ok(val) = axum::http::HeaderValue::from_str(&format!("<{canon}>; rel=\"canonical\"")) {
            headers.insert(axum::http::header::LINK, val);
        }
    }

    Ok((headers, Json(status)))
}

/// `GET /api/public/status/:slug` — aggregated health for a custom status page with 5s cache.
pub async fn public_status_by_slug(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let status = get_public_status_impl(&state, &slug).await?;
    let robots_val = if status.allow_indexing {
        "index, follow"
    } else {
        "noindex, nofollow"
    };

    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::HeaderName::from_static("x-robots-tag"),
        axum::http::HeaderValue::from_static(robots_val),
    );
    if let Some(ref canon) = status.canonical_url {
        if let Ok(val) = axum::http::HeaderValue::from_str(&format!("<{canon}>; rel=\"canonical\"")) {
            headers.insert(axum::http::header::LINK, val);
        }
    }

    Ok((headers, Json(status)))
}

/// `GET /robots.txt` — Dynamic robots engine inspecting status pages and guarding internal routes.
pub async fn robots_txt(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let pages = crate::db::status_pages::list_status_pages(&state.db)
        .await
        .unwrap_or_default();

    let mut body = String::from("User-agent: *\nDisallow: /admin\nDisallow: /admin/\nDisallow: /api/admin/\nDisallow: /api/\n");

    let default_page = pages.iter().find(|p| p.slug == "default");
    let default_indexed = default_page.map(|p| p.allow_indexing).unwrap_or(true);

    if default_indexed {
        body.push_str("Allow: /status\nAllow: /status/\nAllow: /status/default\n");
    } else {
        body.push_str("Disallow: /status\nDisallow: /status/\nDisallow: /status/default\n");
    }

    for page in pages.iter().filter(|p| p.slug != "default") {
        if page.allow_indexing {
            body.push_str(&format!("Allow: /status/{}\n", page.slug));
        } else {
            body.push_str(&format!("Disallow: /status/{}\n", page.slug));
        }
    }

    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("text/plain; charset=utf-8"),
    );

    (headers, body)
}

pub use public_status as get_default_status;
pub use public_status_by_slug as get_status_by_slug;
#[allow(unused_imports)]
pub use public_status as get_public_status_handler;
#[allow(unused_imports)]
pub use public_status_by_slug as get_public_status_by_slug_handler;

/// Clean public routes sub-router for public status telemetry.
pub fn public_routes() -> axum::Router<Arc<AppState>> {
    axum::Router::new()
        // Default status page (slug = "default")
        .route("/status", axum::routing::get(get_default_status))
        // Custom status page by slug
        .route("/status/:slug", axum::routing::get(get_status_by_slug))
        // Dynamic robots.txt
        .route("/robots.txt", axum::routing::get(robots_txt))
}

// ── SVG Status Badge ─────────────────────────────────────────────────────────

/// Optional query parameters for dynamic badge styling or overrides.
#[derive(Debug, Deserialize, Default)]
pub struct BadgeQuery {
    /// Explicit status override (e.g. "operational", "degraded", "downtime", "not_found").
    pub status: Option<String>,
    /// Whether to include latency in operational badge text (e.g. "operational | 18ms").
    pub latency: Option<bool>,
    /// Custom badge label override (e.g. "Landingpage", "Database Cluster").
    pub label: Option<String>,
    /// Visual style variant: "shield" (default), "dot", "compact".
    pub style: Option<String>,
}

/// Internal monitor status enum for badge rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonitorBadgeStatus {
    Up,
    Down,
    Degraded,
    Maintenance,
    NotFound,
}

impl MonitorBadgeStatus {
    pub fn default_label(&self) -> &'static str {
        match self {
            Self::Up => "operational",
            Self::Down => "downtime",
            Self::Degraded => "degraded",
            Self::Maintenance => "maintenance",
            Self::NotFound => "not found",
        }
    }

    pub fn bg_color(&self) -> &'static str {
        match self {
            Self::Up => "#30D158",
            Self::Down => "#FF453A",
            Self::Degraded => "#FFD60A",
            Self::Maintenance => "#A855F7",
            Self::NotFound => "#71717A",
        }
    }

    pub fn text_color(&self) -> &'static str {
        match self {
            Self::Up | Self::Degraded => "#09090b",
            Self::Down | Self::Maintenance | Self::NotFound => "#ffffff",
        }
    }
}

fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

fn estimate_text_width(text: &str) -> f32 {
    let mut width = 0.0;
    for ch in text.chars() {
        width += match ch {
            'm' | 'M' | 'W' | 'w' | '@' => 9.2,
            'A'..='Z' => 7.5,
            'i' | 'l' | '!' | '|' | '.' | ':' | ';' | '\'' | ' ' => 3.6,
            'f' | 'r' | 't' | 'j' => 4.8,
            'a'..='z' | '0'..='9' | '-' | '_' => 6.2,
            _ => 6.5,
        };
    }
    width
}

/// Renders clean vector badge markup (2-part rounded rectangle).
///
/// Left side: Dark background label ("KeepUp | <Monitor Name>" or "KeepUp").
/// Right side: Status indicator color and label.
pub fn render_badge_svg(
    left_label: &str,
    right_label: &str,
    status_bg: &str,
    status_text_color: &str,
    title_text: &str,
) -> String {
    let left_escaped = escape_xml(left_label);
    let right_escaped = escape_xml(right_label);
    let title_escaped = escape_xml(title_text);

    let left_text_w = estimate_text_width(left_label);
    let right_text_w = estimate_text_width(right_label);

    let padding = 16.0;
    let left_w = (left_text_w + padding).max(40.0).round();
    let right_w = (right_text_w + padding).max(40.0).round();
    let total_w = left_w + right_w;

    let left_center_x = (left_w / 2.0).round();
    let right_center_x = (left_w + right_w / 2.0).round();
    let clip_id = format!("k-clip-{}", total_w as u32);

    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{total_w}" height="20" role="img" aria-label="{left_escaped}: {right_escaped}">
  <title>{title_escaped}</title>
  <clipPath id="{clip_id}">
    <rect width="{total_w}" height="20" rx="4" fill="#fff"/>
  </clipPath>
  <g clip-path="url(#{clip_id})">
    <rect width="{left_w}" height="20" fill="#18181b"/>
    <rect x="{left_w}" width="{right_w}" height="20" fill="{status_bg}"/>
  </g>
  <g fill="#fff" text-anchor="middle" font-family="system-ui,BlinkMacSystemFont,'SF Pro Text','Segoe UI',Helvetica,Arial,sans-serif" text-rendering="geometricPrecision" font-size="11">
    <text x="{left_center_x}" y="14" fill="#a1a1aa">{left_escaped}</text>
    <text x="{right_center_x}" y="14" fill="{status_text_color}" font-weight="600">{right_escaped}</text>
  </g>
</svg>"##
    )
}

/// Renders dot minimal badge SVG markup (pulsing radar dot with label and status).
pub fn render_dot_badge_svg(label: &str, status_text: &str, color: &str) -> String {
    let label_escaped = escape_xml(label);
    let status_upper = escape_xml(&status_text.to_uppercase());
    let full_text = format!("{}: {}", label, status_text.to_uppercase());
    let width = ((full_text.len() as f32 * 7.5) + 38.0).round().max(160.0) as u32;
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="28" viewBox="0 0 {width} 28" fill="none" role="img" aria-label="{label_escaped}: {status_upper}">
  <title>{label_escaped}: {status_upper}</title>
  <rect width="{width}" height="28" rx="6" fill="#18181b" stroke="rgba(255,255,255,0.1)"/>
  <circle cx="16" cy="14" r="4" fill="{color}">
    <animate attributeName="opacity" values="0.7;1;0.7" dur="2s" repeatCount="indefinite"/>
  </circle>
  <circle cx="16" cy="14" r="7" stroke="{color}" stroke-width="1.2" fill="none" opacity="0.4">
    <animate attributeName="r" values="4;8" dur="2s" repeatCount="indefinite"/>
    <animate attributeName="opacity" values="0.6;0" dur="2s" repeatCount="indefinite"/>
  </circle>
  <text x="28" y="18" fill="#e4e4e7" font-family="monospace,Courier,monospace" font-size="11" font-weight="600">{label_escaped}: {status_upper}</text>
</svg>"##
    )
}

/// Renders monospace compact badge SVG markup (telemetry pill with left color accent bar and monospace font).
pub fn render_compact_badge_svg(label: &str, status_text: &str, color: &str) -> String {
    let label_upper = escape_xml(&label.to_uppercase());
    let status_upper = escape_xml(&status_text.to_uppercase());
    let full_text = format!("{} // {}", label.to_uppercase(), status_text.to_uppercase());
    let width = ((full_text.len() as f32 * 7.2) + 32.0).round().max(170.0) as u32;
    let end_x = width.saturating_sub(10);
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="26" viewBox="0 0 {width} 26" fill="none" role="img" aria-label="{label_upper} // {status_upper}">
  <title>{label_upper} // {status_upper}</title>
  <rect width="{width}" height="26" rx="4" fill="#09090b" stroke="{color}" stroke-opacity="0.4" stroke-width="1"/>
  <rect x="5" y="5" width="5" height="16" rx="1.5" fill="{color}"/>
  <text x="16" y="17" fill="#a1a1aa" font-family="monospace,Courier,monospace" font-size="10" font-weight="600">{label_upper}</text>
  <text x="{end_x}" y="17" text-anchor="end" fill="{color}" font-family="monospace,Courier,monospace" font-size="10" font-weight="700">{status_upper}</text>
</svg>"##
    )
}

#[derive(sqlx::FromRow)]
struct MonitorBadgeRow {
    id: String,
    name: String,
    is_active: i32,
    is_public: i32,
    last_status: String,
}

/// `GET /api/public/badge/:id` or `GET /api/v1/public/badge/:id.svg`
///
/// Returns a 2-part vector SVG status badge for embedding in READMEs and documentation.
/// Response Headers: `Content-Type: image/svg+xml; charset=utf-8`, `Cache-Control: no-cache, max-age=30`.
/// Always returns HTTP 200 OK with vector content (including NOT_FOUND fallback).
pub async fn badge(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(params): Query<BadgeQuery>,
) -> Result<impl IntoResponse, AppError> {
    let monitor_id = id.strip_suffix(".svg").unwrap_or(&id);

    // Look up monitor by primary ID or matching name
    let monitor = sqlx::query_as::<_, MonitorBadgeRow>(
        "SELECT id, name, is_active, is_public, last_status \
         FROM monitors WHERE id = ? OR name = ? LIMIT 1",
    )
    .bind(monitor_id)
    .bind(monitor_id)
    .fetch_optional(&state.db)
    .await?;

    let (left_label, right_label, status_bg, status_text_color, title_text) = match monitor {
        Some(m) if m.is_public == 1 && m.is_active == 1 => {
            let left = params
                .label
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("KeepUp | {}", m.name));

            if let Some(ref override_status) = params.status {
                let status_lower = override_status.to_lowercase();
                let status_enum = match status_lower.as_str() {
                    "down" | "downtime" | "outage" => MonitorBadgeStatus::Down,
                    "degraded" => MonitorBadgeStatus::Degraded,
                    "maintenance" => MonitorBadgeStatus::Maintenance,
                    "not_found" | "not found" => MonitorBadgeStatus::NotFound,
                    _ => MonitorBadgeStatus::Up,
                };
                let right = status_enum.default_label().to_string();
                let title = format!("{left}: {right}");
                (left, right, status_enum.bg_color(), status_enum.text_color(), title)
            } else if crate::db::maintenance::is_monitor_in_maintenance(&state.db, &m.id).await.unwrap_or(false) {
                let right = "maintenance".to_string();
                let title = format!("{left}: maintenance");
                (
                    left,
                    right,
                    MonitorBadgeStatus::Maintenance.bg_color(),
                    MonitorBadgeStatus::Maintenance.text_color(),
                    title,
                )
            } else {
                // Fetch latest check
                let latest = sqlx::query_as::<_, LatestCheck>(
                    "SELECT is_up, latency_ms FROM check_logs \
                     WHERE monitor_id = ? ORDER BY checked_at DESC, id DESC LIMIT 1",
                )
                .bind(&m.id)
                .fetch_optional(&state.db)
                .await?;

                // Fetch 24-hour uptime percentage
                let agg = sqlx::query_as::<_, UptimeAgg>(
                    "SELECT COUNT(*) AS total, \
                            COALESCE(SUM(CASE WHEN is_up = 1 THEN 1 ELSE 0 END), 0) AS up_count \
                     FROM check_logs \
                     WHERE monitor_id = ? AND checked_at > datetime('now', '-24 hours')",
                )
                .bind(&m.id)
                .fetch_one(&state.db)
                .await?;

                let uptime_24h = if agg.total > 0 {
                    (agg.up_count as f64 / agg.total as f64) * 100.0
                } else {
                    100.0
                };

                let latest_latency = latest.as_ref().and_then(|c| c.latency_ms);

                // Determine operational state
                let (status_enum, right) = match &latest {
                    Some(c) if c.is_up == 0 => {
                        (MonitorBadgeStatus::Down, "downtime".to_string())
                    }
                    Some(_) if uptime_24h < 99.0 => {
                        (MonitorBadgeStatus::Degraded, "degraded".to_string())
                    }
                    Some(c) => {
                        if params.latency.unwrap_or(false) {
                            if let Some(ms) = c.latency_ms {
                                (MonitorBadgeStatus::Up, format!("operational | {}ms", ms))
                            } else {
                                (MonitorBadgeStatus::Up, "operational".to_string())
                            }
                        } else {
                            (MonitorBadgeStatus::Up, "operational".to_string())
                        }
                    }
                    None => {
                        if m.last_status == "DOWN" {
                            (MonitorBadgeStatus::Down, "downtime".to_string())
                        } else {
                            (MonitorBadgeStatus::Up, "operational".to_string())
                        }
                    }
                };

                let title = match (&status_enum, latest_latency) {
                    (MonitorBadgeStatus::Up, Some(ms)) => {
                        format!("{left}: operational ({ms}ms, {uptime_24h:.1}% 24h)")
                    }
                    (MonitorBadgeStatus::Up, None) => {
                        format!("{left}: operational ({uptime_24h:.1}% 24h)")
                    }
                    (MonitorBadgeStatus::Degraded, Some(ms)) => {
                        format!("{left}: degraded ({ms}ms, {uptime_24h:.1}% 24h)")
                    }
                    (MonitorBadgeStatus::Degraded, None) => {
                        format!("{left}: degraded ({uptime_24h:.1}% 24h)")
                    }
                    (MonitorBadgeStatus::Down, _) => {
                        format!("{left}: downtime")
                    }
                    (MonitorBadgeStatus::Maintenance, _) => {
                        format!("{left}: maintenance")
                    }
                    (MonitorBadgeStatus::NotFound, _) => {
                        "KeepUp: not found".to_string()
                    }
                };

                (left, right, status_enum.bg_color(), status_enum.text_color(), title)
            }
        }
        _ => {
            let status_enum = MonitorBadgeStatus::NotFound;
            let left = params
                .label
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.to_string())
                .unwrap_or_else(|| "KeepUp".to_string());
            let right = status_enum.default_label().to_string();
            let title = format!("{left}: not found");
            (left, right, status_enum.bg_color(), status_enum.text_color(), title)
        }
    };

    let style_choice = params
        .style
        .as_deref()
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    let svg_body = match style_choice.as_str() {
        "dot" => render_dot_badge_svg(&left_label, &right_label, status_bg),
        "compact" => render_compact_badge_svg(&left_label, &right_label, status_bg),
        _ => render_badge_svg(
            &left_label,
            &right_label,
            status_bg,
            status_text_color,
            &title_text,
        ),
    };

    let headers = [
        (header::CONTENT_TYPE, "image/svg+xml; charset=utf-8"),
        (header::CACHE_CONTROL, "no-cache, max-age=30"),
    ];

    Ok((StatusCode::OK, headers, svg_body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_badge_svg_colors_and_text() {
        // UP / Operational: #30D158
        let svg_up = render_badge_svg("KeepUp | API Gateway", "operational", "#30D158", "#09090b", "API Gateway: operational");
        assert!(svg_up.contains("#30D158"));
        assert!(svg_up.contains("KeepUp | API Gateway"));
        assert!(svg_up.contains("operational"));
        assert!(svg_up.contains("<svg"));
        assert!(svg_up.contains("</svg>"));

        // DOWN / Downtime: #FF453A
        let svg_down = render_badge_svg("KeepUp | Database", "downtime", "#FF453A", "#ffffff", "Database: downtime");
        assert!(svg_down.contains("#FF453A"));
        assert!(svg_down.contains("downtime"));

        // DEGRADED: #FFD60A
        let svg_deg = render_badge_svg("KeepUp | Payment", "degraded", "#FFD60A", "#09090b", "Payment: degraded");
        assert!(svg_deg.contains("#FFD60A"));
        assert!(svg_deg.contains("degraded"));

        // NOT_FOUND: #71717A
        let svg_nf = render_badge_svg("KeepUp", "not found", "#71717A", "#ffffff", "KeepUp: monitor not found");
        assert!(svg_nf.contains("#71717A"));
        assert!(svg_nf.contains("not found"));
    }

    #[test]
    fn test_render_badge_svg_xml_escaping() {
        let svg = render_badge_svg("Auth & <Billing> \"v1\"", "status", "#30D158", "#09090b", "Title & 'Quotes'");
        assert!(svg.contains("Auth &amp; &lt;Billing&gt; &quot;v1&quot;"));
        assert!(svg.contains("Title &amp; &apos;Quotes&apos;"));
    }

    #[test]
    fn test_render_dot_and_compact_badge_svg() {
        let dot_svg = render_dot_badge_svg("Landingpage", "operational", "#30D158");
        assert!(dot_svg.contains("Landingpage: OPERATIONAL"));
        assert!(dot_svg.contains("#30D158"));
        assert!(dot_svg.contains("<circle"));

        let compact_svg = render_compact_badge_svg("Landingpage", "degraded", "#FFD60A");
        assert!(compact_svg.contains("LANDINGPAGE"));
        assert!(compact_svg.contains("DEGRADED"));
        assert!(compact_svg.contains("#FFD60A"));
    }

    #[test]
    fn test_monitor_badge_status_properties() {
        assert_eq!(MonitorBadgeStatus::Up.bg_color(), "#30D158");
        assert_eq!(MonitorBadgeStatus::Down.bg_color(), "#FF453A");
        assert_eq!(MonitorBadgeStatus::Degraded.bg_color(), "#FFD60A");
        assert_eq!(MonitorBadgeStatus::NotFound.bg_color(), "#71717A");

        assert_eq!(MonitorBadgeStatus::Up.default_label(), "operational");
        assert_eq!(MonitorBadgeStatus::Down.default_label(), "downtime");
        assert_eq!(MonitorBadgeStatus::Degraded.default_label(), "degraded");
        assert_eq!(MonitorBadgeStatus::NotFound.default_label(), "not found");
    }

    #[tokio::test]
    async fn test_badge_handler_with_db() {
        use axum::response::IntoResponse;

        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS monitors (
                id TEXT PRIMARY KEY NOT NULL,
                name TEXT NOT NULL,
                url TEXT NOT NULL,
                check_interval_secs INTEGER NOT NULL DEFAULT 60,
                timeout_secs INTEGER NOT NULL DEFAULT 5,
                max_retries INTEGER NOT NULL DEFAULT 3,
                is_active INTEGER NOT NULL DEFAULT 1,
                is_public INTEGER NOT NULL DEFAULT 1,
                last_status TEXT NOT NULL DEFAULT 'UP',
                down_since DATETIME,
                created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            CREATE TABLE IF NOT EXISTS check_logs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                monitor_id TEXT NOT NULL,
                status_code INTEGER,
                latency_ms INTEGER,
                is_up INTEGER NOT NULL,
                error_msg TEXT,
                checked_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
            );"
        )
        .execute(&pool)
        .await
        .unwrap();

        let state = AppState::new_for_test(pool.clone(), crate::config::Config::from_env());

        // 1. Insert active public monitor
        sqlx::query(
            "INSERT INTO monitors (id, name, url, is_active, is_public) VALUES ('api-srv', 'API Gateway', 'https://api.test', 1, 1)"
        )
        .execute(&pool)
        .await
        .unwrap();

        // Check log: UP, 18ms
        sqlx::query(
            "INSERT INTO check_logs (monitor_id, is_up, latency_ms) VALUES ('api-srv', 1, 18)"
        )
        .execute(&pool)
        .await
        .unwrap();

        // Test GET /badge/api-srv.svg
        let res = badge(
            State(state.clone()),
            Path("api-srv.svg".to_string()),
            Query(BadgeQuery::default()),
        )
        .await
        .unwrap()
        .into_response();

        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers().get("content-type").unwrap(),
            "image/svg+xml; charset=utf-8"
        );
        assert_eq!(
            res.headers().get("cache-control").unwrap(),
            "no-cache, max-age=30"
        );

        let body_bytes = axum::body::to_bytes(res.into_body(), 1024 * 64).await.unwrap();
        let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
        assert!(body_str.contains("#30D158"));
        assert!(body_str.contains("KeepUp | API Gateway"));
        assert!(body_str.contains("operational"));

        // 2. Test nonexistent monitor
        let res_nf = badge(
            State(state.clone()),
            Path("unknown-service.svg".to_string()),
            Query(BadgeQuery::default()),
        )
        .await
        .unwrap()
        .into_response();

        assert_eq!(res_nf.status(), StatusCode::OK);
        let nf_bytes = axum::body::to_bytes(res_nf.into_body(), 1024 * 64).await.unwrap();
        let nf_str = String::from_utf8(nf_bytes.to_vec()).unwrap();
        assert!(nf_str.contains("#71717A"));
        assert!(nf_str.contains("not found"));

        // 3. Test DOWN check log
        sqlx::query(
            "INSERT INTO check_logs (monitor_id, is_up, latency_ms, checked_at) VALUES ('api-srv', 0, 0, datetime('now', '+1 second'))"
        )
        .execute(&pool)
        .await
        .unwrap();

        let res_down = badge(
            State(state.clone()),
            Path("api-srv.svg".to_string()),
            Query(BadgeQuery::default()),
        )
        .await
        .unwrap()
        .into_response();

        let down_bytes = axum::body::to_bytes(res_down.into_body(), 1024 * 64).await.unwrap();
        let down_str = String::from_utf8(down_bytes.to_vec()).unwrap();
        assert!(down_str.contains("#FF453A"));
        assert!(down_str.contains("downtime"));

        // 4. Test DEGRADED state (latest check UP, but 24h uptime is 66.7% < 99.0%)
        sqlx::query(
            "INSERT INTO check_logs (monitor_id, is_up, latency_ms, checked_at) VALUES ('api-srv', 1, 24, datetime('now', '+2 seconds'))"
        )
        .execute(&pool)
        .await
        .unwrap();

        let res_deg = badge(
            State(state.clone()),
            Path("api-srv.svg".to_string()),
            Query(BadgeQuery::default()),
        )
        .await
        .unwrap()
        .into_response();

        let deg_bytes = axum::body::to_bytes(res_deg.into_body(), 1024 * 64).await.unwrap();
        let deg_str = String::from_utf8(deg_bytes.to_vec()).unwrap();
        assert!(deg_str.contains("#FFD60A"));
        assert!(deg_str.contains("degraded"));

        // 5. Test latency query param (?latency=true) with 100% operational monitor
        sqlx::query(
            "INSERT INTO monitors (id, name, url, is_active, is_public) VALUES ('lat-srv', 'Metrics Service', 'https://metrics.test', 1, 1)"
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO check_logs (monitor_id, is_up, latency_ms, checked_at) VALUES ('lat-srv', 1, 24, datetime('now'))"
        )
        .execute(&pool)
        .await
        .unwrap();

        let res_lat = badge(
            State(state.clone()),
            Path("lat-srv.svg".to_string()),
            Query(BadgeQuery { status: None, latency: Some(true), ..Default::default() }),
        )
        .await
        .unwrap()
        .into_response();

        let lat_bytes = axum::body::to_bytes(res_lat.into_body(), 1024 * 64).await.unwrap();
        let lat_str = String::from_utf8(lat_bytes.to_vec()).unwrap();
        assert!(lat_str.contains("operational | 24ms"));
        assert!(lat_str.contains("#30D158"));

        // 6. Test private monitor is returned as NOT_FOUND (gray #71717A)
        sqlx::query(
            "INSERT INTO monitors (id, name, url, is_active, is_public) VALUES ('priv-mon', 'Private Intranet', 'https://internal.test', 1, 0)"
        )
        .execute(&pool)
        .await
        .unwrap();

        let res_priv = badge(
            State(state.clone()),
            Path("priv-mon.svg".to_string()),
            Query(BadgeQuery::default()),
        )
        .await
        .unwrap()
        .into_response();

        assert_eq!(res_priv.status(), StatusCode::OK);
        let priv_bytes = axum::body::to_bytes(res_priv.into_body(), 1024 * 64).await.unwrap();
        let priv_str = String::from_utf8(priv_bytes.to_vec()).unwrap();
        assert!(priv_str.contains("#71717A"));
        assert!(priv_str.contains("not found"));

        // 7. Test status query override (e.g. ?status=degraded)
        let res_override = badge(
            State(state.clone()),
            Path("api-srv.svg".to_string()),
            Query(BadgeQuery { status: Some("degraded".to_string()), latency: None, ..Default::default() }),
        )
        .await
        .unwrap()
        .into_response();

        let override_bytes = axum::body::to_bytes(res_override.into_body(), 1024 * 64).await.unwrap();
        let override_str = String::from_utf8(override_bytes.to_vec()).unwrap();
        assert!(override_str.contains("#FFD60A"));
        assert!(override_str.contains("degraded"));

        // 8. Full Router integration test: GET /api/public/badge/api-srv.svg
        use tower::ServiceExt;
        let app = crate::api::router(state.clone()).with_state(state.clone());
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/api/public/badge/api-srv.svg")
            .body(axum::body::Body::empty())
            .unwrap();

        let router_res = app.oneshot(req).await.unwrap();
        assert_eq!(router_res.status(), StatusCode::OK);
        assert_eq!(
            router_res.headers().get("content-type").unwrap(),
            "image/svg+xml; charset=utf-8"
        );
        assert_eq!(
            router_res.headers().get("cache-control").unwrap(),
            "no-cache, max-age=30"
        );
        let router_bytes = axum::body::to_bytes(router_res.into_body(), 1024 * 64).await.unwrap();
        let router_str = String::from_utf8(router_bytes.to_vec()).unwrap();
        assert!(router_str.contains("KeepUp | API Gateway"));
        assert!(router_str.contains("<svg"));
        assert!(router_str.contains("</svg>"));

        // 9. Full Router test: GET /api/badge/api-srv?style=compact&label=Landingpage
        let app2 = crate::api::router(state.clone()).with_state(state.clone());
        let req2 = axum::http::Request::builder()
            .method("GET")
            .uri("/api/badge/api-srv?style=compact&label=Landingpage")
            .body(axum::body::Body::empty())
            .unwrap();

        let res2 = app2.oneshot(req2).await.unwrap();
        assert_eq!(res2.status(), StatusCode::OK);
        assert_eq!(res2.headers().get("content-type").unwrap(), "image/svg+xml; charset=utf-8");
        let bytes2 = axum::body::to_bytes(res2.into_body(), 1024 * 64).await.unwrap();
        let str2 = String::from_utf8(bytes2.to_vec()).unwrap();
        assert!(str2.contains("LANDINGPAGE"));
        assert!(str2.contains("DEGRADED"));
        assert!(str2.contains("#FFD60A"));

        // 10. Full Router test: GET /api/badge/<uuid>?style=compact&label=Landingpage (not in DB yet)
        let app3 = crate::api::router(state.clone()).with_state(state.clone());
        let req3 = axum::http::Request::builder()
            .method("GET")
            .uri("/api/badge/9716245a-2032-4303-b46c-8cf54f13330f?style=compact&label=Landingpage")
            .body(axum::body::Body::empty())
            .unwrap();

        let res3 = app3.oneshot(req3).await.unwrap();
        assert_eq!(res3.status(), StatusCode::OK);
        assert_eq!(res3.headers().get("content-type").unwrap(), "image/svg+xml; charset=utf-8");
        let bytes3 = axum::body::to_bytes(res3.into_body(), 1024 * 64).await.unwrap();
        let str3 = String::from_utf8(bytes3.to_vec()).unwrap();
        assert!(str3.contains("LANDINGPAGE"));
        assert!(str3.contains("NOT FOUND"));
        assert!(str3.contains("#71717A"));
    }

    async fn setup_test_pool() -> sqlx::SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("Failed to connect to in-memory SQLite");

        crate::db::run_migrations(&pool)
            .await
            .expect("Failed to run migrations");

        pool
    }

    #[tokio::test]
    async fn test_public_status_multi_page_and_groups() {
        let pool = setup_test_pool().await;
        let config = crate::config::Config::from_env();
        let state = AppState::new_for_test(pool.clone(), config);

        // Seed 3 monitors: m1 (public), m2 (public), m3 (private)
        sqlx::query("INSERT INTO monitors (id, name, url, is_public, is_active) VALUES ('m1', 'Public API', 'https://api.test', 1, 1)")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO monitors (id, name, url, is_public, is_active) VALUES ('m2', 'Auth Service', 'https://auth.test', 1, 1)")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO monitors (id, name, url, is_public, is_active) VALUES ('m3', 'Internal Worker', 'https://worker.test', 0, 1)")
            .execute(&pool).await.unwrap();

        // Check logs for m1 (UP) and m2 (UP)
        sqlx::query("INSERT INTO check_logs (monitor_id, is_up, latency_ms) VALUES ('m1', 1, 20)")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO check_logs (monitor_id, is_up, latency_ms) VALUES ('m2', 1, 35)")
            .execute(&pool).await.unwrap();

        // 1. Default page without explicit assignments should return all active public monitors under "Services"
        let default_status = get_public_status_impl(&state, "default").await.unwrap();
        assert_eq!(default_status.title, "System Status");
        assert_eq!(default_status.monitors.len(), 2);
        assert_eq!(default_status.groups.len(), 1);
        assert_eq!(default_status.groups[0].name, "Services");
        assert_eq!(default_status.groups[0].monitors.len(), 2);

        // 2. Create custom page "dashboards" assigned only to m1 under "Gateways"
        crate::db::status_pages::create_status_page(
            &pool,
            "dashboards",
            "Customer Dashboards",
            Some("Dashboard telemetry"),
            true,
            None,
            true,
            None,
            None,
            None,
            None,
        ).await.unwrap();

        crate::db::status_pages::update_status_page(
            &pool,
            "dashboards",
            "Customer Dashboards",
            Some("Dashboard telemetry"),
            true,
            Some(vec![
                crate::db::status_pages::StatusPageMonitorAssignment {
                    monitor_id: "m1".to_string(),
                    group_name: "Gateways".to_string(),
                    display_order: 1,
                }
            ]),
            true,
            None,
            None,
            None,
            None,
        ).await.unwrap();

        let custom_status = get_public_status_impl(&state, "dashboards").await.unwrap();
        assert_eq!(custom_status.title, "Customer Dashboards");
        assert_eq!(custom_status.monitors.len(), 1);
        assert_eq!(custom_status.monitors[0].id, "m1");
        assert_eq!(custom_status.groups.len(), 1);
        assert_eq!(custom_status.groups[0].name, "Gateways");

        // 3. Querying non-existent slug returns NotFound (404)
        let not_found = get_public_status_impl(&state, "non-existent").await;
        assert!(matches!(not_found, Err(AppError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_custom_page_dashboard_internal_dashboards_integration() {
        use tower::ServiceExt;
        let pool = setup_test_pool().await;
        let config = crate::config::Config::from_env();
        let state = AppState::new_for_test(pool.clone(), config);

        // 1. Seed monitors
        sqlx::query("INSERT INTO monitors (id, name, url, is_public, is_active) VALUES ('m-dash-1', 'Main Dashboard', 'https://dash1.test', 1, 1)")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO monitors (id, name, url, is_public, is_active) VALUES ('m-dash-2', 'Metrics Dashboard', 'https://dash2.test', 1, 1)")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO monitors (id, name, url, is_public, is_active) VALUES ('m-other', 'Billing Worker', 'https://billing.test', 1, 1)")
            .execute(&pool).await.unwrap();

        // 2. Create custom status page "dashboard"
        crate::db::status_pages::create_status_page(
            &pool,
            "dashboard",
            "Internal Dashboards Status",
            Some("Health status of internal dashboards"),
            true,
            None,
            true,
            None,
            None,
            None,
            None,
        ).await.unwrap();

        // 3. Assign 2 monitors with group label "Internal Dashboards"
        crate::db::status_pages::update_status_page(
            &pool,
            "dashboard",
            "Internal Dashboards Status",
            Some("Health status of internal dashboards"),
            true,
            Some(vec![
                crate::db::status_pages::StatusPageMonitorAssignment {
                    monitor_id: "m-dash-1".to_string(),
                    group_name: "Internal Dashboards".to_string(),
                    display_order: 1,
                },
                crate::db::status_pages::StatusPageMonitorAssignment {
                    monitor_id: "m-dash-2".to_string(),
                    group_name: "Internal Dashboards".to_string(),
                    display_order: 2,
                },
            ]),
            true,
            None,
            None,
            None,
            None,
        ).await.unwrap();

        // 4. Test API via Router GET /api/public/status/dashboard
        let app = crate::api::router(state.clone()).with_state(state.clone());
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/api/public/status/dashboard")
            .body(axum::body::Body::empty())
            .unwrap();

        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(res.into_body(), 1024 * 64).await.unwrap();
        let payload: PublicStatus = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(payload.title, "Internal Dashboards Status");
        assert_eq!(payload.monitors.len(), 2);
        assert!(payload.monitors.iter().any(|m| m.id == "m-dash-1"));
        assert!(payload.monitors.iter().any(|m| m.id == "m-dash-2"));
        assert!(!payload.monitors.iter().any(|m| m.id == "m-other"));

        assert_eq!(payload.groups.len(), 1);
        assert_eq!(payload.groups[0].name, "Internal Dashboards");
        assert_eq!(payload.groups[0].monitors.len(), 2);

        // 5. Test API for non-existent slug returns 404
        let app_nf = crate::api::router(state.clone()).with_state(state.clone());
        let req_nf = axum::http::Request::builder()
            .method("GET")
            .uri("/api/public/status/unknown-page")
            .body(axum::body::Body::empty())
            .unwrap();

        let res_nf = app_nf.oneshot(req_nf).await.unwrap();
        assert_eq!(res_nf.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_public_status_with_active_maintenance() {
        use chrono::Duration;

        let pool = setup_test_pool().await;
        let config = crate::config::Config::from_env();
        let state = AppState::new_for_test(pool.clone(), config);

        // 1. Seed monitors
        sqlx::query("INSERT INTO monitors (id, name, url, is_public, is_active) VALUES ('m-maint-1', 'Payments API', 'https://payments.test', 1, 1)")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO monitors (id, name, url, is_public, is_active) VALUES ('m-normal-1', 'Auth Service', 'https://auth.test', 1, 1)")
            .execute(&pool).await.unwrap();

        // Check log for m-maint-1 was failed check
        sqlx::query("INSERT INTO check_logs (monitor_id, status_code, latency_ms, is_up, error_msg) VALUES ('m-maint-1', 503, 120, 0, 'Service Unavailable')")
            .execute(&pool).await.unwrap();
        // Check log for m-normal-1 was up
        sqlx::query("INSERT INTO check_logs (monitor_id, status_code, latency_ms, is_up) VALUES ('m-normal-1', 200, 45, 1)")
            .execute(&pool).await.unwrap();

        // 2. Schedule active maintenance window covering now
        let now = chrono::Utc::now();
        let start = now - Duration::minutes(15);
        let end = now + Duration::hours(1);

        crate::db::maintenance::create_maintenance_window(
            &pool,
            crate::db::maintenance::CreateMaintenanceDto {
                title: "Planned Payments Gateway Upgrade".to_string(),
                description: Some("Upgrading payment processors to v3".to_string()),
                start_time: start.to_rfc3339(),
                end_time: end.to_rfc3339(),
                monitor_ids: vec!["m-maint-1".to_string()],
            },
        ).await.unwrap();

        // 3. Query public status
        let public_status = get_public_status_impl(&state, "default").await.unwrap();

        // Maintenance window must be included and active
        assert_eq!(public_status.maintenance_windows.len(), 1);
        assert_eq!(public_status.maintenance_windows[0].title, "Planned Payments Gateway Upgrade");
        assert!(public_status.maintenance_windows[0].is_active);

        // m-maint-1 must report "MAINTENANCE" instead of "DOWN"
        let maint_mon = public_status.monitors.iter().find(|m| m.id == "m-maint-1").unwrap();
        assert_eq!(maint_mon.current_status, "MAINTENANCE");

        // Overall status should not be marked as Major Outage since failure was in maintenance
        assert_ne!(public_status.status, "Major Outage");

        // 4. Test badge rendering for monitor under maintenance
        let badge_svg = render_badge_svg(
            &format!("KeepUp | {}", maint_mon.name),
            "maintenance",
            MonitorBadgeStatus::Maintenance.bg_color(),
            MonitorBadgeStatus::Maintenance.text_color(),
            "KeepUp | Payments API: maintenance",
        );
        assert!(badge_svg.contains("maintenance"));
        assert!(badge_svg.contains("#A855F7"));
    }

    #[tokio::test]
    async fn test_public_status_latency_percentiles_with_100_samples() {
        use tower::ServiceExt;

        let pool = setup_test_pool().await;
        let config = crate::config::Config::from_env();
        let state = AppState::new_for_test(pool.clone(), config);

        // 1. Seed active public monitor
        sqlx::query(
            "INSERT INTO monitors (id, name, url, is_public, is_active) VALUES ('m-pct', 'Percentile API', 'https://api.pct', 1, 1)"
        )
        .execute(&pool)
        .await
        .unwrap();

        // 2. Seed check_logs with 100 sample response times from 10ms to 406ms (10 + i * 4)
        for i in 0..100 {
            let lat: i32 = 10 + i * 4;
            sqlx::query(
                "INSERT INTO check_logs (monitor_id, status_code, latency_ms, is_up, checked_at, created_at)
                 VALUES ('m-pct', 200, ?, 1, datetime('now'), datetime('now'))"
            )
            .bind(lat)
            .execute(&pool)
            .await
            .unwrap();
        }

        // 3. Call GET /api/public/status through Router
        let app = crate::api::router(state.clone()).with_state(state.clone());
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/api/public/status")
            .body(axum::body::Body::empty())
            .unwrap();

        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let body_bytes = axum::body::to_bytes(res.into_body(), 1024 * 64).await.unwrap();
        let status_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();

        let monitor = &status_json["monitors"][0];
        assert_eq!(monitor["id"], "m-pct");

        let lat_obj = &monitor["latency"];
        assert_eq!(lat_obj["p50"], 210);
        assert_eq!(lat_obj["p95"], 386);
        assert_eq!(lat_obj["p99"], 402);
        assert_eq!(lat_obj["avg"], 208);
    }

    #[tokio::test]
    async fn test_custom_status_page_api_core_services_group() {
        use tower::ServiceExt;

        let pool = setup_test_pool().await;
        let config = crate::config::Config::from_env();
        let state = AppState::new_for_test(pool.clone(), config);

        // 1. Seed 2 monitors
        sqlx::query(
            "INSERT INTO monitors (id, name, url, is_active, is_public, last_status) \
             VALUES ('mon-api-1', 'Users Service', 'https://api.test/users', 1, 1, 'UP')"
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO monitors (id, name, url, is_active, is_public, last_status) \
             VALUES ('mon-api-2', 'Auth Service', 'https://api.test/auth', 1, 1, 'UP')"
        )
        .execute(&pool)
        .await
        .unwrap();

        // Check logs for monitors
        sqlx::query("INSERT INTO check_logs (monitor_id, is_up, latency_ms) VALUES ('mon-api-1', 1, 15)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO check_logs (monitor_id, is_up, latency_ms) VALUES ('mon-api-2', 1, 28)")
            .execute(&pool)
            .await
            .unwrap();

        // 2. Create custom status page `slug: "api"`, `title: "API Services"` with monitors assigned under group `"Core Services"`
        crate::db::status_pages::create_status_page(
            &pool,
            "api",
            "API Services",
            Some("Core API status page"),
            true,
            Some(vec![
                crate::db::status_pages::StatusPageMonitorAssignment {
                    monitor_id: "mon-api-1".to_string(),
                    group_name: "Core Services".to_string(),
                    display_order: 1,
                },
                crate::db::status_pages::StatusPageMonitorAssignment {
                    monitor_id: "mon-api-2".to_string(),
                    group_name: "Core Services".to_string(),
                    display_order: 2,
                },
            ]),
            true,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

        // 3. Test get_public_status_page in DB
        let (page_meta, page_monitors) = crate::db::status_pages::get_public_status_page(&pool, "api")
            .await
            .unwrap()
            .expect("Status page 'api' must exist");

        assert_eq!(page_meta.slug, "api");
        assert_eq!(page_meta.title, "API Services");
        assert_eq!(page_monitors.len(), 2);
        assert_eq!(page_monitors[0].group_name, "Core Services");
        assert_eq!(page_monitors[1].group_name, "Core Services");

        // 4. Test GET /api/public/status/api through Router
        let app = crate::api::router(state.clone()).with_state(state.clone());
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/api/public/status/api")
            .body(axum::body::Body::empty())
            .unwrap();

        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let body_bytes = axum::body::to_bytes(res.into_body(), 1024 * 64).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();

        assert_eq!(json["title"], "API Services");
        assert_eq!(json["monitors"].as_array().unwrap().len(), 2);
        assert_eq!(json["groups"].as_array().unwrap().len(), 1);
        assert_eq!(json["groups"][0]["name"], "Core Services");
        assert_eq!(json["groups"][0]["monitors"].as_array().unwrap().len(), 2);

        let m1 = &json["groups"][0]["monitors"][0];
        let m2 = &json["groups"][0]["monitors"][1];
        assert_eq!(m1["id"], "mon-api-1");
        assert_eq!(m1["current_status"], "UP");
        assert_eq!(m2["id"], "mon-api-2");
        assert_eq!(m2["current_status"], "UP");
    }

    #[tokio::test]
    async fn test_custom_status_page_dri_monitors_and_assets() {
        use tower::ServiceExt;

        let pool = setup_test_pool().await;
        let config = crate::config::Config::from_env();
        let state = AppState::new_for_test(pool.clone(), config);

        // 1. Insert DRI and Rekadigi monitors
        sqlx::query(
            "INSERT INTO monitors (id, name, url, is_public, is_active) \
             VALUES ('mon-dri', 'DRI', 'https://dri.example.com', 1, 1), \
                    ('mon-rekadigi', 'Rekadigi', 'https://rekadigi.example.com', 1, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        // 2. Insert check logs
        sqlx::query(
            "INSERT INTO check_logs (monitor_id, is_up, status_code, response_time_ms, latency_ms) \
             VALUES ('mon-dri', 1, 200, 15, 15), ('mon-rekadigi', 1, 200, 22, 22)",
        )
        .execute(&pool)
        .await
        .unwrap();

        // 3. Create status page `dri` with both monitors assigned
        crate::db::status_pages::create_status_page(
            &pool,
            "dri",
            "DRI",
            Some("DRI systems telemetry"),
            true,
            Some(vec![
                crate::db::status_pages::StatusPageMonitorAssignment {
                    monitor_id: "mon-dri".to_string(),
                    group_name: "DRI Services".to_string(),
                    display_order: 1,
                },
                crate::db::status_pages::StatusPageMonitorAssignment {
                    monitor_id: "mon-rekadigi".to_string(),
                    group_name: "DRI Services".to_string(),
                    display_order: 2,
                },
            ]),
            true,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

        // 4. Test GET /api/public/status/dri
        let app = crate::api::router(state.clone()).with_state(state.clone());
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/api/public/status/dri")
            .body(axum::body::Body::empty())
            .unwrap();

        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let body_bytes = axum::body::to_bytes(res.into_body(), 1024 * 64).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();

        assert_eq!(json["title"], "DRI");
        assert_eq!(json["description"], "DRI systems telemetry");
        let monitors = json["monitors"].as_array().unwrap();
        assert_eq!(monitors.len(), 2);
        assert_eq!(monitors[0]["name"], "DRI");
        assert_eq!(monitors[1]["name"], "Rekadigi");

        // 5. Test Case-insensitivity (GET /api/public/status/DRI)
        let req_upper = axum::http::Request::builder()
            .method("GET")
            .uri("/api/public/status/DRI")
            .body(axum::body::Body::empty())
            .unwrap();
        let res_upper = app.oneshot(req_upper).await.unwrap();
        assert_eq!(res_upper.status(), StatusCode::OK);

        // 6. Test UI status_page_handler for /status/dri serves HTML
        let uri_page = axum::http::Uri::from_static("/status/dri");
        let html_res = crate::ui::status_page_handler(axum::extract::State(state.clone()), uri_page).await;
        assert_eq!(html_res.status(), StatusCode::OK);
        assert_eq!(
            html_res.headers().get(axum::http::header::CONTENT_TYPE).unwrap(),
            "text/html; charset=utf-8"
        );
        let html_bytes = axum::body::to_bytes(html_res.into_body(), 1024 * 64).await.unwrap();
        let html_str = String::from_utf8_lossy(&html_bytes);
        assert!(html_str.contains("id=\"page-title\""));
        assert!(html_str.contains("id=\"monitors-container\""));
        assert!(html_str.contains("id=\"loading-indicator\""));
        assert!(html_str.contains("/status.js"));

        // 7. Test UI status_page_handler for /status/status.js serves JS (NOT HTML!)
        let uri_js = axum::http::Uri::from_static("/status/status.js");
        let js_res = crate::ui::status_page_handler(axum::extract::State(state.clone()), uri_js).await;
        assert_eq!(js_res.status(), StatusCode::OK);
        assert!(
            js_res
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("application/javascript")
        );
        let js_bytes = axum::body::to_bytes(js_res.into_body(), 1024 * 64).await.unwrap();
        let js_str = String::from_utf8_lossy(&js_bytes);
        assert!(js_str.contains("loadStatusData"));
    }

    #[tokio::test]
    async fn test_default_status_page_respects_is_public_and_case_insensitivity() {
        use tower::ServiceExt;

        let pool = setup_test_pool().await;
        let config = crate::config::Config::from_env();
        let state = AppState::new_for_test(pool.clone(), config);

        crate::db::status_pages::update_status_page(
            &pool,
            "default",
            "Private Default Page",
            Some("Internal only"),
            false,
            None,
            true,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

        let app = crate::api::router(state.clone()).with_state(state.clone());

        for uri in ["/api/public/status", "/api/public/status/default", "/api/public/status/DEFAULT"] {
            let req = axum::http::Request::builder()
                .method("GET")
                .uri(uri)
                .body(axum::body::Body::empty())
                .unwrap();
            let res = app.clone().oneshot(req).await.unwrap();
            assert_eq!(
                res.status(),
                StatusCode::NOT_FOUND,
                "expected 404 when default status page is_public=0 on {uri}"
            );
        }
    }

    #[tokio::test]
    async fn test_default_status_page_hides_assigned_private_and_paused_without_fallback_leak() {
        let pool = setup_test_pool().await;
        let config = crate::config::Config::from_env();
        let state = AppState::new_for_test(pool.clone(), config);

        sqlx::query(
            "INSERT INTO monitors (id, name, url, is_public, is_active) VALUES              ('m-pub-unassigned', 'Public Unassigned', 'https://pub.example.com', 1, 1),              ('m-priv-assigned', 'Private Assigned', 'https://priv.example.com', 0, 1),              ('m-paused-assigned', 'Paused Assigned', 'https://paused.example.com', 1, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Assign ONLY private and paused monitors to "default"
        crate::db::status_pages::update_status_page(
            &pool,
            "default",
            "System Status",
            None,
            true,
            Some(vec![
                crate::db::status_pages::StatusPageMonitorAssignment {
                    monitor_id: "m-priv-assigned".to_string(),
                    group_name: "Core".to_string(),
                    display_order: 1,
                },
                crate::db::status_pages::StatusPageMonitorAssignment {
                    monitor_id: "m-paused-assigned".to_string(),
                    group_name: "Core".to_string(),
                    display_order: 2,
                },
            ]),
            true,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

        let status_empty = get_public_status_impl(&state, "default").await.unwrap();
        assert!(
            status_empty.monitors.is_empty(),
            "when all explicitly assigned monitors on default are private/paused, must return [] instead of leaking them or falling back to unassigned monitors: got {:?}",
            status_empty.monitors.iter().map(|m| &m.id).collect::<Vec<_>>()
        );

        // Now assign m-pub-unassigned alongside private & paused -> only m-pub-unassigned should be visible
        state.invalidate_status_cache().await;
        crate::db::status_pages::update_status_page(
            &pool,
            "default",
            "System Status",
            None,
            true,
            Some(vec![
                crate::db::status_pages::StatusPageMonitorAssignment {
                    monitor_id: "m-pub-unassigned".to_string(),
                    group_name: "Core".to_string(),
                    display_order: 1,
                },
                crate::db::status_pages::StatusPageMonitorAssignment {
                    monitor_id: "m-priv-assigned".to_string(),
                    group_name: "Core".to_string(),
                    display_order: 2,
                },
                crate::db::status_pages::StatusPageMonitorAssignment {
                    monitor_id: "m-paused-assigned".to_string(),
                    group_name: "Core".to_string(),
                    display_order: 3,
                },
            ]),
            true,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

        let status_filtered = get_public_status_impl(&state, "default").await.unwrap();
        assert_eq!(status_filtered.monitors.len(), 1);
        assert_eq!(status_filtered.monitors[0].id, "m-pub-unassigned");
    }

    #[tokio::test]
    async fn test_public_status_banner_thresholds_and_scoped_maintenance() {
        use chrono::Duration;

        // Case A: Currently UP with 99.95% 24h uptime (1999 UP + 1 old DOWN) -> "All Systems Operational"
        let pool_a = setup_test_pool().await;
        let mut cfg_a = crate::config::Config::from_env();
        cfg_a.custom_logo_url = Some("https://cdn.example.com/logo.svg".to_string());
        let state_a = AppState::new_for_test(pool_a.clone(), cfg_a);

        sqlx::query(
            "INSERT INTO monitors (id, name, url, is_public, is_active) VALUES              ('m-pub', 'Core API', 'https://api.example.com', 1, 1),              ('m-priv', 'Secret DB', 'https://db.internal', 0, 1)",
        )
        .execute(&pool_a)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO check_logs (monitor_id, is_up, latency_ms, checked_at) VALUES ('m-pub', 0, 100, datetime('now', '-2 hours'))",
        )
        .execute(&pool_a)
        .await
        .unwrap();
        for _ in 0..1999 {
            sqlx::query(
                "INSERT INTO check_logs (monitor_id, is_up, latency_ms, checked_at) VALUES ('m-pub', 1, 15, datetime('now', '-1 minute'))",
            )
            .execute(&pool_a)
            .await
            .unwrap();
        }
        // Latest check is UP
        sqlx::query(
            "INSERT INTO check_logs (monitor_id, is_up, latency_ms, checked_at) VALUES ('m-pub', 1, 12, datetime('now'))",
        )
        .execute(&pool_a)
        .await
        .unwrap();

        // Schedule 1 upcoming window (+2 days, targeting m-pub + m-priv) and 1 far-future window (+20 days)
        let now = chrono::Utc::now();
        crate::db::maintenance::create_maintenance_window(
            &pool_a,
            crate::db::maintenance::CreateMaintenanceDto {
                title: "Near Window".to_string(),
                description: None,
                start_time: (now + Duration::days(2)).to_rfc3339(),
                end_time: (now + Duration::days(2) + Duration::hours(1)).to_rfc3339(),
                monitor_ids: vec!["m-pub".to_string(), "m-priv".to_string()],
            },
        )
        .await
        .unwrap();
        crate::db::maintenance::create_maintenance_window(
            &pool_a,
            crate::db::maintenance::CreateMaintenanceDto {
                title: "Far Future Window".to_string(),
                description: None,
                start_time: (now + Duration::days(20)).to_rfc3339(),
                end_time: (now + Duration::days(20) + Duration::hours(2)).to_rfc3339(),
                monitor_ids: vec!["m-pub".to_string()],
            },
        )
        .await
        .unwrap();

        let status_a = get_public_status_impl(&state_a, "default").await.unwrap();
        assert_eq!(
            status_a.status, "All Systems Operational",
            "99.95% 24h uptime while currently UP must be All Systems Operational"
        );
        assert_eq!(
            status_a.maintenance_windows.len(),
            1,
            "windows starting >7 days in the future must be excluded"
        );
        assert_eq!(
            status_a.maintenance_windows[0].monitor_ids,
            vec!["m-pub".to_string()],
            "private monitor IDs must be filtered out of public maintenance_windows"
        );

        // Case B: Currently UP with 83.33% 24h uptime (5 UP + 1 old DOWN) -> Degraded / Partial Outage
        let pool_b = setup_test_pool().await;
        let state_b = AppState::new_for_test(pool_b.clone(), crate::config::Config::from_env());
        sqlx::query(
            "INSERT INTO monitors (id, name, url, is_public, is_active) VALUES ('m-deg', 'Flaky API', 'https://flaky.example.com', 1, 1)",
        )
        .execute(&pool_b)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO check_logs (monitor_id, is_up, latency_ms, checked_at) VALUES ('m-deg', 0, 500, datetime('now', '-1 hour'))",
        )
        .execute(&pool_b)
        .await
        .unwrap();
        for _ in 0..5 {
            sqlx::query(
                "INSERT INTO check_logs (monitor_id, is_up, latency_ms, checked_at) VALUES ('m-deg', 1, 40, datetime('now'))",
            )
            .execute(&pool_b)
            .await
            .unwrap();
        }

        let status_b = get_public_status_impl(&state_b, "default").await.unwrap();
        assert!(
            status_b.status == "Degraded Performance" || status_b.status == "Partial System Outage",
            "83.33% 24h uptime must render Degraded Performance or Partial System Outage, got {}",
            status_b.status
        );
    }

    #[tokio::test]
    async fn test_seo_and_robots_visibility() {
        use tower::ServiceExt;

        let pool = setup_test_pool().await;
        let config = crate::config::Config::from_env();
        let state = AppState::new_for_test(pool.clone(), config);

        // 1. Create a status page with allow_indexing = true and custom SEO tags
        crate::db::status_pages::create_status_page(
            &pool,
            "seo-public",
            "Public Service Status",
            Some("Public status overview"),
            true,
            None,
            true,
            Some("Custom SEO Title | Acme Inc"),
            Some("Acme Inc system health and uptime."),
            Some("https://example.com/og-banner.png"),
            Some("https://status.example.com/status/seo-public"),
        )
        .await
        .unwrap();

        // 2. Create a status page with allow_indexing = false
        crate::db::status_pages::create_status_page(
            &pool,
            "seo-private",
            "Internal Service Status",
            Some("Internal private telemetry"),
            true,
            None,
            false,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

        // 3. Test /robots.txt endpoint
        let app = axum::Router::new()
            .route("/robots.txt", axum::routing::get(robots_txt))
            .merge(crate::api::router(state.clone()))
            .with_state(state.clone());

        let req_robots = axum::http::Request::builder()
            .method("GET")
            .uri("/robots.txt")
            .body(axum::body::Body::empty())
            .unwrap();

        let res_robots = app.clone().oneshot(req_robots).await.unwrap();
        assert_eq!(res_robots.status(), StatusCode::OK);
        assert_eq!(
            res_robots.headers().get(axum::http::header::CONTENT_TYPE).unwrap(),
            "text/plain; charset=utf-8"
        );
        let robots_bytes = axum::body::to_bytes(res_robots.into_body(), 1024 * 16).await.unwrap();
        let robots_txt = String::from_utf8_lossy(&robots_bytes);
        assert!(robots_txt.contains("User-agent: *"));
        assert!(robots_txt.contains("Disallow: /admin/"));
        assert!(robots_txt.contains("Disallow: /api/admin/"));
        assert!(robots_txt.contains("Disallow: /api/"));
        assert!(robots_txt.contains("Allow: /status/seo-public"));
        assert!(robots_txt.contains("Disallow: /status/seo-private"));

        // 4. Test public status API headers for indexed page
        let req_indexed = axum::http::Request::builder()
            .method("GET")
            .uri("/api/public/status/seo-public")
            .body(axum::body::Body::empty())
            .unwrap();
        let res_indexed = app.clone().oneshot(req_indexed).await.unwrap();
        assert_eq!(res_indexed.status(), StatusCode::OK);
        assert_eq!(
            res_indexed.headers().get("x-robots-tag").unwrap(),
            "index, follow"
        );
        assert_eq!(
            res_indexed.headers().get(axum::http::header::LINK).unwrap(),
            r#"<https://status.example.com/status/seo-public>; rel="canonical""#
        );

        // 5. Test public status API headers for non-indexed page
        let req_noindex = axum::http::Request::builder()
            .method("GET")
            .uri("/api/public/status/seo-private")
            .body(axum::body::Body::empty())
            .unwrap();
        let res_noindex = app.clone().oneshot(req_noindex).await.unwrap();
        assert_eq!(res_noindex.status(), StatusCode::OK);
        assert_eq!(
            res_noindex.headers().get("x-robots-tag").unwrap(),
            "noindex, nofollow"
        );

        // 6. Test Admin / Internal API route has noindex, nofollow, noarchive
        let req_auth = axum::http::Request::builder()
            .method("GET")
            .uri("/api/auth/me")
            .body(axum::body::Body::empty())
            .unwrap();
        let res_auth = app.clone().oneshot(req_auth).await.unwrap();
        assert_eq!(
            res_auth.headers().get("x-robots-tag").unwrap(),
            "noindex, nofollow, noarchive"
        );

        // 7. Test UI status_page_handler HTML tag injection
        let uri_seo = axum::http::Uri::from_static("/status/seo-public");
        let html_res = crate::ui::status_page_handler(axum::extract::State(state.clone()), uri_seo).await;
        assert_eq!(html_res.status(), StatusCode::OK);
        assert_eq!(
            html_res.headers().get("x-robots-tag").unwrap(),
            "index, follow"
        );
        assert_eq!(
            html_res.headers().get(axum::http::header::LINK).unwrap(),
            r#"<https://status.example.com/status/seo-public>; rel="canonical""#
        );
        let html_bytes = axum::body::to_bytes(html_res.into_body(), 1024 * 64).await.unwrap();
        let html = String::from_utf8_lossy(&html_bytes);
        assert!(html.contains("<title>Custom SEO Title | Acme Inc</title>"));
        assert!(html.contains(r#"<meta name="description" content="Acme Inc system health and uptime.">"#));
        assert!(html.contains(r#"<meta name="robots" content="index, follow">"#));
        assert!(html.contains(r#"<meta property="og:title" content="Custom SEO Title | Acme Inc">"#));
        assert!(html.contains(r#"<meta property="og:image" content="https://example.com/og-banner.png">"#));
        assert!(html.contains(r#"<meta name="twitter:image" content="https://example.com/og-banner.png">"#));
        assert!(html.contains(r#"<link rel="canonical" href="https://status.example.com/status/seo-public">"#));
    }

    #[tokio::test]
    async fn test_public_status_cache_ttl_and_invalidation() {
        let cache = PublicStatusCache::new(Duration::from_millis(50));
        let dummy_status = PublicStatus {
            title: "Test Status".to_string(),
            description: None,
            status: "All Systems Operational".to_string(),
            status_page_theme: None,
            custom_logo_url: None,
            maintenance_windows: vec![],
            incidents: vec![],
            groups: vec![],
            monitors: vec![],
            allow_indexing: true,
            meta_title: None,
            meta_description: None,
            og_image_url: None,
            canonical_url: None,
        };

        // Cache miss initially
        assert!(cache.get("default").await.is_none());

        // Cache hit after set
        cache.set("default", dummy_status.clone()).await;
        let cached = cache.get("default").await;
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().title, "Test Status");

        // Explicit slug invalidation
        cache.invalidate(Some("default")).await;
        assert!(cache.get("default").await.is_none());

        // Set again and test clear all
        cache.set("default", dummy_status.clone()).await;
        cache.set("other", dummy_status.clone()).await;
        assert!(cache.get("default").await.is_some());
        assert!(cache.get("other").await.is_some());
        cache.invalidate(None).await;
        assert!(cache.get("default").await.is_none());
        assert!(cache.get("other").await.is_none());

        // Test TTL expiration
        cache.set("expiring", dummy_status).await;
        assert!(cache.get("expiring").await.is_some());
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(cache.get("expiring").await.is_none());
    }
}
