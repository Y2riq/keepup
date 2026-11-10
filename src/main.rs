mod alerting;
mod api;
mod banner;
mod checker;
mod config;
mod db;
mod error;
mod middleware;
mod net_guard;
mod ui;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::http::{header, HeaderName, HeaderValue, Method};
use reqwest::Client;
use sqlx::SqlitePool;
use tokio::net::TcpListener;
use tower_http::compression::CompressionLayer;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::set_header::SetResponseHeaderLayer;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::config::Config;

/// Shared application state threaded through Axum handlers.
#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub write_db: SqlitePool,
    pub config: Config,
    pub http_client: Client,
    pub reload_tx: tokio::sync::broadcast::Sender<String>,
    pub session_version: Arc<AtomicU64>,
    pub status_cache: crate::api::public::PublicStatusCache,
    pub monitor_states: Arc<
        tokio::sync::Mutex<
            std::collections::HashMap<String, crate::checker::scheduler::MonitorState>,
        >,
    >,
    pub events_tx: tokio::sync::broadcast::Sender<String>,
}

impl AppState {
    pub fn current_session_version(&self) -> u64 {
        self.session_version.load(Ordering::SeqCst)
    }

    pub async fn invalidate_status_cache(&self) {
        self.status_cache.invalidate(None).await;
    }

    #[cfg(test)]
    pub fn new_for_test(db: SqlitePool, config: Config) -> Arc<Self> {
        let (reload_tx, _) = tokio::sync::broadcast::channel(16);
        let http_client = net_guard::build_http_client(
            config.allow_private_targets,
            std::time::Duration::from_secs(5),
        )
        .expect("valid test http client");
        let (events_tx, _) = tokio::sync::broadcast::channel(100);
        Arc::new(Self {
            write_db: db.clone(),
            db,
            config,
            http_client,
            reload_tx,
            session_version: Arc::new(AtomicU64::new(1)),
            status_cache: crate::api::public::PublicStatusCache::default(),
            monitor_states: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            events_tx,
        })
    }
}

/// Assemble the complete Axum HTTP application router with body size limits,
/// explicit-only CORS, gzip compression, and defensive HTTP security headers.
pub fn build_http_app(state: Arc<AppState>) -> axum::Router {
    let cors = if !state.config.cors_allowed_origins.is_empty() {
        let parsed_origins: Vec<HeaderValue> = state
            .config
            .cors_allowed_origins
            .iter()
            .filter_map(|o| HeaderValue::from_str(o).ok())
            .collect();
        if !parsed_origins.is_empty() {
            CorsLayer::new()
                .allow_methods([
                    Method::GET,
                    Method::POST,
                    Method::PUT,
                    Method::DELETE,
                    Method::PATCH,
                    Method::OPTIONS,
                ])
                .allow_headers([
                    header::CONTENT_TYPE,
                    header::AUTHORIZATION,
                    header::ACCEPT,
                    header::COOKIE,
                ])
                .allow_origin(parsed_origins)
                .allow_credentials(true)
        } else {
            CorsLayer::new()
        }
    } else {
        CorsLayer::new()
    };

    axum::Router::new()
        .route("/robots.txt", axum::routing::get(api::public::robots_txt))
        .route("/metrics", axum::routing::get(api::observability::metrics_handler))
        .route("/status", axum::routing::get(ui::status_page_handler))
        .route("/status/*path", axum::routing::get(ui::status_page_handler))
        .route("/badge/:id", axum::routing::get(api::public::badge))
        .merge(api::router(state.clone()))
        .fallback(ui::static_handler)
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(CompressionLayer::new())
        .layer(cors)
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("referrer-policy"),
            HeaderValue::from_static("same-origin"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'self'; img-src 'self' https: data:; style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline'; connect-src 'self'",
            ),
        ))
        .with_state(state)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Install ring crypto provider for rustls
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();

    // Load .env file (optional – not an error if missing).
    dotenvy::dotenv().ok();

    // Initialise structured logging.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,keepup=debug")),
        )
        .init();

    // Load configuration from environment variables.
    let mut config = Config::from_env();
    banner::print_banner(env!("CARGO_PKG_VERSION"), config.port, &config.database_url);
    info!(port = config.port, db = %config.database_url, "Starting KeepUp");

    // Initialise SQLite dual pools (read_pool: 10 conns, write_pool: 1 conn) and run migrations.
    let (read_pool, write_pool) = db::init_pools(&config.database_url).await?;
    db::run_migrations(&write_pool).await?;

    // Bootstrap persistent JWT secret, initial Argon2id admin password, and session_version.
    let initial_session_ver = api::auth::bootstrap_security_state(&write_pool, &mut config).await?;

    // Create broadcast channels for monitor reload and live SSE events.
    let (reload_tx, reload_rx) = tokio::sync::broadcast::channel::<String>(100);
    let (events_tx, _) = tokio::sync::broadcast::channel::<String>(100);

    // Build shared HTTP client with outbound SSRF guard for health checks & webhooks.
    let http_client = net_guard::build_http_client(
        config.allow_private_targets,
        std::time::Duration::from_secs(10),
    )?;

    let status_cache = crate::api::public::PublicStatusCache::default();

    // Build shared state.
    let state = Arc::new(AppState {
        db: read_pool.clone(),
        write_db: write_pool.clone(),
        config: config.clone(),
        http_client: http_client.clone(),
        reload_tx,
        session_version: Arc::new(AtomicU64::new(initial_session_ver)),
        status_cache: status_cache.clone(),
        monitor_states: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        events_tx,
    });

    // Spawn the background health-check scheduler with reload receiver.
    tokio::spawn(checker::scheduler::run_scheduler(
        state.clone(),
        reload_rx,
    ));

    let app = build_http_app(state);

    // Bind and serve with connection info for client IP rate limiting.
    let addr = format!("0.0.0.0:{}", config.port);
    let listener = TcpListener::bind(&addr).await?;
    info!("Listening on {}", addr);
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;

    Ok(())
}
