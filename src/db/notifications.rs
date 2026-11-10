use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::error::AppError;

fn default_true() -> bool {
    true
}

/// Relational data model for a unified notification channel.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct NotificationChannel {
    pub id: String,
    pub name: String,
    #[sqlx(rename = "type")]
    #[serde(rename = "type")]
    pub channel_type: String,
    pub url: String,
    pub is_default: bool,
    #[serde(default = "default_true")]
    pub is_enabled: bool,
    #[serde(default = "default_true")]
    pub use_default_template: bool,
    #[serde(default = "default_true")]
    pub notify_on_resolve: bool,
    #[serde(default)]
    pub custom_template: Option<String>,
    pub created_at: String,
    pub bot_name: Option<String>,
    pub avatar_url: Option<String>,
    pub telegram_bot_token: Option<String>,
    pub telegram_chat_id: Option<String>,
    pub telegram_thread_id: Option<String>,
    pub last_error: Option<String>,
    pub last_error_at: Option<String>,
}

/// Relational data model for an alert notification failure log entry.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[allow(dead_code)]
pub struct NotificationFailureLog {
    pub id: String,
    pub channel_id: String,
    pub channel_name: String,
    pub channel_type: String,
    pub error_message: String,
    pub status_code: Option<i64>,
    pub attempt_count: i64,
    pub created_at: String,
}

impl NotificationChannel {
    /// Returns a cloned view with secret token/path/query segments masked for API responses (`SEC-04`).
    pub fn masked_view(&self) -> Self {
        let mut copy = self.clone();
        copy.url = mask_webhook_url(&self.url);
        copy.telegram_bot_token = self.telegram_bot_token.as_ref().map(|_| "****".to_string());
        copy
    }
}

/// Returns `true` if `url` still contains the `****` redaction marker from `masked_view()`.
pub fn is_masked_url(url: &str) -> bool {
    url.contains("****")
}

/// Redacts sensitive webhook tokens, path suffixes, and query strings while preserving scheme + host.
pub fn mask_webhook_url(raw: &str) -> String {
    let trimmed = raw.trim();
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        return "****".to_string();
    };

    // Strip any userinfo@ before host
    let rest_no_auth = match rest.split_once('@') {
        Some((_, after_at)) => after_at,
        None => rest,
    };

    let (host, tail) = match rest_no_auth.find(['/', '?', '#']) {
        Some(idx) => (&rest_no_auth[..idx], &rest_no_auth[idx..]),
        None => (rest_no_auth, ""),
    };

    let path_only = tail.split(['?', '#']).next().unwrap_or("");
    let segments: Vec<&str> = path_only
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();

    if segments.len() >= 4 && segments[0] == "api" && segments[1] == "webhooks" {
        // Discord: https://discord.com/api/webhooks/<id>/****
        format!("{scheme}://{host}/api/webhooks/{}/****", segments[2])
    } else if segments.len() >= 3 && segments[0] == "services" {
        // Slack: https://hooks.slack.com/services/<workspace>/****
        format!("{scheme}://{host}/services/{}/****", segments[1])
    } else if !segments.is_empty() && segments[0].starts_with("bot") {
        // Telegram: https://api.telegram.org/bot****/sendMessage
        let action = segments.get(1).copied().unwrap_or("sendMessage");
        format!("{scheme}://{host}/bot****/{action}")
    } else if !segments.is_empty() || tail.contains('?') {
        format!("{scheme}://{host}/****")
    } else {
        format!("{scheme}://{host}")
    }
}

const SELECT_CHANNEL_COLS: &str = "\
    id, name, type, url, \
    (is_default != 0) AS is_default, \
    (is_enabled != 0) AS is_enabled, \
    (use_default_template != 0) AS use_default_template, \
    (notify_on_resolve != 0) AS notify_on_resolve, \
    custom_template, created_at, \
    bot_name, avatar_url, telegram_bot_token, telegram_chat_id, telegram_thread_id, \
    last_error, last_error_at";

/// Retrieve all notification channels ordered by creation date (newest first).
pub async fn list_channels(pool: &SqlitePool) -> Result<Vec<NotificationChannel>, sqlx::Error> {
    let sql = format!(
        "SELECT {SELECT_CHANNEL_COLS} FROM notification_channels ORDER BY created_at DESC"
    );
    sqlx::query_as::<_, NotificationChannel>(&sql)
        .fetch_all(pool)
        .await
}

/// Retrieve a single notification channel by ID.
pub async fn get_channel(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<NotificationChannel>, sqlx::Error> {
    let sql = format!("SELECT {SELECT_CHANNEL_COLS} FROM notification_channels WHERE id = ?");
    sqlx::query_as::<_, NotificationChannel>(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// Insert a new notification channel into the database.
pub async fn create_channel(
    pool: &SqlitePool,
    channel: NotificationChannel,
) -> Result<NotificationChannel, sqlx::Error> {
    sqlx::query(
        "INSERT INTO notification_channels (\
         id, name, type, url, is_default, is_enabled, use_default_template, notify_on_resolve, custom_template, created_at, \
         bot_name, avatar_url, telegram_bot_token, telegram_chat_id, telegram_thread_id\
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&channel.id)
    .bind(&channel.name)
    .bind(&channel.channel_type)
    .bind(&channel.url)
    .bind(channel.is_default as i32)
    .bind(channel.is_enabled as i32)
    .bind(channel.use_default_template as i32)
    .bind(channel.notify_on_resolve as i32)
    .bind(&channel.custom_template)
    .bind(&channel.created_at)
    .bind(&channel.bot_name)
    .bind(&channel.avatar_url)
    .bind(&channel.telegram_bot_token)
    .bind(&channel.telegram_chat_id)
    .bind(&channel.telegram_thread_id)
    .execute(pool)
    .await?;

    Ok(channel)
}

/// Update an existing notification channel.
#[allow(clippy::too_many_arguments)]
pub async fn update_channel(
    pool: &SqlitePool,
    id: &str,
    name: &str,
    channel_type: &str,
    url: &str,
    is_default: bool,
    is_enabled: bool,
    use_default_template: bool,
    notify_on_resolve: bool,
    custom_template: Option<&str>,
    bot_name: Option<&str>,
    avatar_url: Option<&str>,
    telegram_bot_token: Option<&str>,
    telegram_chat_id: Option<&str>,
    telegram_thread_id: Option<&str>,
) -> Result<NotificationChannel, AppError> {
    let result = sqlx::query(
        "UPDATE notification_channels \
         SET name = ?, type = ?, url = ?, is_default = ?, is_enabled = ?, \
             use_default_template = ?, notify_on_resolve = ?, custom_template = ?, \
             bot_name = ?, avatar_url = ?, telegram_bot_token = ?, telegram_chat_id = ?, \
             telegram_thread_id = ?, last_error = NULL, last_error_at = NULL \
         WHERE id = ?",
    )
    .bind(name)
    .bind(channel_type)
    .bind(url)
    .bind(is_default as i32)
    .bind(is_enabled as i32)
    .bind(use_default_template as i32)
    .bind(notify_on_resolve as i32)
    .bind(custom_template)
    .bind(bot_name)
    .bind(avatar_url)
    .bind(telegram_bot_token)
    .bind(telegram_chat_id)
    .bind(telegram_thread_id)
    .bind(id)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!(
            "Notification channel {id} not found"
        )));
    }

    get_channel(pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("Notification channel {id} not found")))
}

/// Delete a notification channel by ID.
/// Monitor associations are automatically removed via ON DELETE CASCADE.
pub async fn delete_channel(pool: &SqlitePool, id: &str) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM notification_channels WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;

    Ok(result.rows_affected() > 0)
}

/// Get all channels explicitly bound to a monitor.
#[allow(dead_code)]
pub async fn get_channels_for_monitor(
    pool: &SqlitePool,
    monitor_id: &str,
) -> Result<Vec<NotificationChannel>, sqlx::Error> {
    let sql = format!(
        "SELECT {SELECT_CHANNEL_COLS} \
         FROM notification_channels \
         INNER JOIN monitor_notification_channels mnc ON notification_channels.id = mnc.channel_id \
         WHERE mnc.monitor_id = ? \
         ORDER BY notification_channels.name ASC"
    );
    sqlx::query_as::<_, NotificationChannel>(&sql)
        .bind(monitor_id)
        .fetch_all(pool)
        .await
}

/// Get IDs of channels explicitly attached to a monitor.
pub async fn get_channel_ids_for_monitor(
    pool: &SqlitePool,
    monitor_id: &str,
) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT channel_id FROM monitor_notification_channels WHERE monitor_id = ?",
    )
    .bind(monitor_id)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Overwrite channel associations for a monitor within a transaction, enforcing foreign keys.
#[allow(dead_code)]
pub async fn set_monitor_channels(
    pool: &SqlitePool,
    monitor_id: &str,
    channel_ids: &[String],
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;

    sqlx::query("DELETE FROM monitor_notification_channels WHERE monitor_id = ?")
        .bind(monitor_id)
        .execute(&mut *tx)
        .await?;

    let mut seen = HashSet::new();
    for channel_id in channel_ids {
        if seen.insert(channel_id.as_str()) {
            sqlx::query(
                "INSERT INTO monitor_notification_channels (monitor_id, channel_id) \
                 VALUES (?, ?)",
            )
            .bind(monitor_id)
            .bind(channel_id)
            .execute(&mut *tx)
            .await?;
        }
    }

    tx.commit().await?;
    Ok(())
}

/// Get effective enabled channels for a monitor: all explicitly associated channels
/// union all default channels (`is_default = 1`).
pub async fn get_effective_channels_for_monitor(
    pool: &SqlitePool,
    monitor_id: &str,
) -> Result<Vec<NotificationChannel>, sqlx::Error> {
    let sql = format!(
        "SELECT DISTINCT {SELECT_CHANNEL_COLS} \
         FROM notification_channels \
         LEFT JOIN monitor_notification_channels mnc ON notification_channels.id = mnc.channel_id AND mnc.monitor_id = ? \
         WHERE notification_channels.is_enabled != 0 AND (mnc.monitor_id IS NOT NULL OR notification_channels.is_default = 1) \
         ORDER BY notification_channels.name ASC"
    );
    sqlx::query_as::<_, NotificationChannel>(&sql)
        .bind(monitor_id)
        .fetch_all(pool)
        .await
}

/// Record an alert notification delivery failure log entry and set warning state on the channel.
pub async fn record_channel_failure(
    pool: &SqlitePool,
    channel_id: &str,
    channel_name: &str,
    channel_type: &str,
    error_message: &str,
    status_code: Option<u16>,
    attempt_count: u32,
) -> Result<(), sqlx::Error> {
    let log_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let sc = status_code.map(|s| s as i64);

    sqlx::query(
        "INSERT INTO notification_failure_logs (\
         id, channel_id, channel_name, channel_type, error_message, status_code, attempt_count, created_at\
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&log_id)
    .bind(channel_id)
    .bind(channel_name)
    .bind(channel_type)
    .bind(error_message)
    .bind(sc)
    .bind(attempt_count as i64)
    .bind(&now)
    .execute(pool)
    .await?;

    sqlx::query(
        "UPDATE notification_channels \
         SET last_error = ?, last_error_at = ? \
         WHERE id = ?",
    )
    .bind(error_message)
    .bind(&now)
    .bind(channel_id)
    .execute(pool)
    .await?;

    Ok(())
}

/// Clear any warning state on the channel after a successful delivery.
pub async fn record_channel_success(
    pool: &SqlitePool,
    channel_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE notification_channels \
         SET last_error = NULL, last_error_at = NULL \
         WHERE id = ?",
    )
    .bind(channel_id)
    .execute(pool)
    .await?;

    Ok(())
}

/// Retrieve recent failure logs for a given channel (or all channels).
#[allow(dead_code)]
pub async fn list_failure_logs(
    pool: &SqlitePool,
    channel_id: Option<&str>,
    limit: i64,
) -> Result<Vec<NotificationFailureLog>, sqlx::Error> {
    if let Some(cid) = channel_id {
        sqlx::query_as::<_, NotificationFailureLog>(
            "SELECT id, channel_id, channel_name, channel_type, error_message, status_code, attempt_count, created_at \
             FROM notification_failure_logs \
             WHERE channel_id = ? \
             ORDER BY created_at DESC LIMIT ?",
        )
        .bind(cid)
        .bind(limit)
        .fetch_all(pool)
        .await
    } else {
        sqlx::query_as::<_, NotificationFailureLog>(
            "SELECT id, channel_id, channel_name, channel_type, error_message, status_code, attempt_count, created_at \
             FROM notification_failure_logs \
             ORDER BY created_at DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(pool)
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_v2_migration_and_pool_foreign_keys_and_permissions() {
        let tmp_path = std::env::temp_dir().join(format!(
            "keepup-task2-{}.db",
            uuid::Uuid::new_v4()
        ));
        let tmp_str = tmp_path.to_str().unwrap();

        let pool = crate::db::init_pool(tmp_str).await.unwrap();
        crate::db::run_migrations(&pool).await.unwrap();

        let mut conn1 = pool.acquire().await.unwrap();
        let mut conn2 = pool.acquire().await.unwrap();
        let fk1: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&mut *conn1)
            .await
            .unwrap();
        let fk2: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&mut *conn2)
            .await
            .unwrap();
        assert_eq!(fk1, 1);
        assert_eq!(fk2, 1);
        drop(conn1);
        drop(conn2);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&tmp_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "SQLite database file must have 0600 permissions");
        }

        // Legacy webhooks tables dropped, new incidents tables created
        let legacy_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('webhooks', 'monitor_webhooks')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(legacy_count, 0);

        let incident_tables: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('incidents', 'incident_updates')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(incident_tables, 2);

        pool.close().await;
        let _ = std::fs::remove_file(&tmp_path);
        let _ = std::fs::remove_file(format!("{tmp_str}-wal"));
        let _ = std::fs::remove_file(format!("{tmp_str}-shm"));
    }

    #[tokio::test]
    async fn test_unified_channel_fields_masking_and_fk_enforcement() {
        let tmp_path = std::env::temp_dir().join(format!(
            "keepup-task2-chan-{}.db",
            uuid::Uuid::new_v4()
        ));
        let tmp_str = tmp_path.to_str().unwrap();
        let pool = crate::db::init_pool(tmp_str).await.unwrap();
        crate::db::run_migrations(&pool).await.unwrap();

        let chan = NotificationChannel {
            id: "chan-1".to_string(),
            name: "Ops Discord".to_string(),
            channel_type: "discord".to_string(),
            url: "https://discord.com/api/webhooks/123456789/super-secret-token-xyz".to_string(),
            is_default: true,
            is_enabled: true,
            use_default_template: false,
            notify_on_resolve: false,
            custom_template: Some("{\"content\":\"{{monitor_name}} {{status}}\"}".to_string()),
            created_at: chrono::Utc::now().to_rfc3339(),
            bot_name: None,
            avatar_url: None,
            telegram_bot_token: None,
            telegram_chat_id: None,
            telegram_thread_id: None,
            last_error: None,
            last_error_at: None,
        };

        create_channel(&pool, chan.clone()).await.unwrap();
        let loaded = get_channel(&pool, "chan-1").await.unwrap().unwrap();
        assert!(loaded.is_enabled);
        assert!(!loaded.use_default_template);
        assert!(!loaded.notify_on_resolve);
        assert_eq!(
            loaded.custom_template.as_deref(),
            Some("{\"content\":\"{{monitor_name}} {{status}}\"}")
        );

        // Verify masked_view redacts secrets across provider URL formats
        let masked_discord = loaded.masked_view();
        assert!(!masked_discord.url.contains("super-secret-token-xyz"));
        assert!(masked_discord.url.contains("****"));

        let mut tg = loaded.clone();
        tg.channel_type = "telegram".to_string();
        tg.url = "https://api.telegram.org/bot998877:AAHsecretBotToken123/sendMessage?chat_id=42".to_string();
        assert!(!tg.masked_view().url.contains("AAHsecretBotToken123"));

        let mut generic = loaded.clone();
        generic.channel_type = "webhook".to_string();
        generic.url = "https://hooks.example.com/ingest/secretpath?token=topsecret".to_string();
        let masked_gen = generic.masked_view();
        assert!(!masked_gen.url.contains("topsecret"));
        assert!(!masked_gen.url.contains("secretpath"));

        // Seed monitor and verify FK enforcement on set_monitor_channels
        sqlx::query(
            "INSERT INTO monitors (id, name, url) VALUES ('mon-1', 'Test', 'https://example.com')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let fk_err = set_monitor_channels(&pool, "mon-1", &["non-existent-channel".to_string()]).await;
        assert!(fk_err.is_err(), "Foreign key violation must return Err");

        pool.close().await;
        let _ = std::fs::remove_file(&tmp_path);
        let _ = std::fs::remove_file(format!("{tmp_str}-wal"));
        let _ = std::fs::remove_file(format!("{tmp_str}-shm"));
    }
}
