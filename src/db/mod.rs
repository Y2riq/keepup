pub mod checks;
pub mod incidents;
pub mod maintenance;
pub mod monitors;
pub mod notifications;
pub mod status_pages;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{ConnectOptions, SqlitePool};
use std::str::FromStr;
use std::time::Duration;
use tracing::info;

/// Build optimized SQLite connection options with WAL, NORMAL sync, generous cache,
/// memory-mapped I/O, memory temp store, and busy timeouts.
pub fn build_connect_options(database_url: &str) -> Result<SqliteConnectOptions, sqlx::Error> {
    let is_memory = database_url == ":memory:" || database_url == "sqlite::memory:";

    if !is_memory {
        if let Some(parent) = std::path::Path::new(database_url).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).ok();
            }
        }
    }

    let uri = if database_url.starts_with("sqlite:") {
        database_url.to_string()
    } else {
        format!("sqlite://{database_url}")
    };

    let mut opts = SqliteConnectOptions::from_str(&uri)?
        .create_if_missing(true)
        .busy_timeout(Duration::from_millis(5000))
        .foreign_keys(true)
        .pragma("cache_size", "-4000") // 4 MB page cache per connection (keeps idle RAM < 15 MB)
        .pragma("mmap_size", "33554432") // 32 MB memory-mapped I/O
        .pragma("temp_store", "2") // Memory temp store
        .log_statements(tracing::log::LevelFilter::Debug);

    if !is_memory {
        opts = opts
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .synchronous(sqlx::sqlite::SqliteSynchronous::Normal);
    }

    Ok(opts)
}

/// Initialise dual connection pools:
/// - `read_pool`: 10 connections for concurrent public status queries & dashboard reads.
/// - `write_pool`: 1 dedicated connection for probe telemetry writes and mutations (guaranteeing zero write-write contention).
pub async fn init_pools(database_url: &str) -> Result<(SqlitePool, SqlitePool), sqlx::Error> {
    let is_memory = database_url == ":memory:" || database_url == "sqlite::memory:";
    let connect_options = build_connect_options(database_url)?;

    let read_conns = if is_memory { 1 } else { 10 };
    let (read_pool, write_pool) = if is_memory {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(connect_options)
            .await?;
        (pool.clone(), pool)
    } else {
        let read_pool = SqlitePoolOptions::new()
            .max_connections(read_conns)
            .connect_with(connect_options.clone())
            .await?;
        let write_pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(connect_options)
            .await?;
        (read_pool, write_pool)
    };

    #[cfg(unix)]
    if !is_memory {
        use std::os::unix::fs::PermissionsExt;
        let db_path = database_url.trim_start_matches("sqlite://");
        for suffix in ["", "-wal", "-shm"] {
            let p = format!("{db_path}{suffix}");
            if let Ok(meta) = std::fs::metadata(&p) {
                let mut perms = meta.permissions();
                perms.set_mode(0o600);
                let _ = std::fs::set_permissions(&p, perms);
            }
        }
    }

    info!("SQLite dual pools initialised (WAL mode, read: {read_conns} conns, write: 1 conn)");
    Ok((read_pool, write_pool))
}

/// Initialise the SQLite connection pool with optimized PRAGMAs.
pub async fn init_pool(database_url: &str) -> Result<SqlitePool, sqlx::Error> {
    let (read_pool, _) = init_pools(database_url).await?;
    Ok(read_pool)
}

/// Run embedded sqlx migrations against the given pool.
pub async fn run_migrations(pool: &SqlitePool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!().run(pool).await?;
    info!("Database migrations applied");
    Ok(())
}
