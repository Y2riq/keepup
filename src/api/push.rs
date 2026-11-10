use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct PushQuery {
    pub status: Option<String>,
    pub latency_ms: Option<i64>,
    pub msg: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PushResponse {
    pub ok: bool,
    pub monitor_id: String,
    pub status: String,
}

/// Generates a cryptographically secure 32-character hexadecimal token for push/heartbeat monitors.
pub fn generate_push_token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// `GET/POST /api/push/:token` — receive an incoming heartbeat ping for a push monitor.
pub async fn push_handler(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    Query(query): Query<PushQuery>,
) -> Result<(StatusCode, Json<PushResponse>), AppError> {
    let raw_status = query
        .status
        .as_deref()
        .unwrap_or("UP")
        .trim()
        .to_uppercase();

    if raw_status != "UP" && raw_status != "DOWN" && raw_status != "DEGRADED" {
        return Err(AppError::BadRequest(
            "Invalid status: must be UP, DOWN, or DEGRADED".to_string(),
        ));
    }

    // Look up active monitor by push_token
    let monitor: Option<crate::db::monitors::Monitor> = sqlx::query_as(
        r#"
        SELECT * FROM monitors
        WHERE push_token = ? AND is_active = 1
        LIMIT 1
        "#,
    )
    .bind(&token)
    .fetch_optional(&state.db)
    .await?;

    let monitor = match monitor {
        Some(m) => m,
        None => return Err(AppError::NotFound("Heartbeat target not found".to_string())),
    };

    let latency_ms = query.latency_ms.unwrap_or(0).max(0);
    let is_up = raw_status != "DOWN";
    let status_code = if is_up { 200 } else { 500 };
    let error_msg = if !is_up {
        query
            .msg
            .clone()
            .or_else(|| Some("Heartbeat reported service DOWN".to_string()))
    } else {
        query.msg.clone()
    };

    let now_str = Utc::now().to_rfc3339();

    // Insert check log
    sqlx::query(
        r#"
        INSERT INTO check_logs (
            monitor_id, status_code, latency_ms, is_up, error_msg, checked_at
        )
        VALUES (?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&monitor.id)
    .bind(status_code)
    .bind(latency_ms)
    .bind(if is_up { 1 } else { 0 })
    .bind(&error_msg)
    .bind(&now_str)
    .execute(&state.db)
    .await?;

    let old_status = monitor.last_status.clone();
    let new_status = raw_status.clone();

    let in_maint = crate::db::maintenance::is_monitor_in_maintenance(&state.db, &monitor.id)
        .await
        .unwrap_or(false);

    if old_status != new_status && !in_maint {
        let new_down_since = if new_status == "DOWN" {
            Some(now_str.clone())
        } else {
            None
        };

        sqlx::query(
            r#"
            UPDATE monitors
            SET last_status = ?, down_since = ?
            WHERE id = ?
            "#,
        )
        .bind(&new_status)
        .bind(&new_down_since)
        .bind(&monitor.id)
        .execute(&state.db)
        .await?;

        // Handle auto-incidents
        if new_status == "DOWN" {
            let msg = error_msg.as_deref().unwrap_or("Heartbeat failure");
            let _ = crate::db::incidents::auto_create_incident_on_down(
                &state.db,
                &monitor.id,
                &monitor.name,
                msg,
            )
            .await;
        } else if old_status == "DOWN" && new_status == "UP" {
            let _ = crate::db::incidents::auto_resolve_incident_on_up(&state.db, &monitor.id).await;
        }

        // Broadcast status changed
        let _ = state.events_tx.send(
            serde_json::json!({
                "event": "status_changed",
                "monitor_id": monitor.id,
                "old_status": old_status,
                "new_status": new_status,
            })
            .to_string(),
        );

        // Dispatch alert notifications to configured channels
        let alert_payload = crate::alerting::AlertPayload {
            monitor_id: Some(monitor.id.clone()),
            monitor_name: monitor.name.clone(),
            monitor_url: monitor.url.clone(),
            new_status: new_status.clone(),
            old_status: old_status.clone(),
            http_status: Some(status_code as u16),
            response_time_ms: Some(latency_ms as u64),
            error_msg: error_msg.clone(),
            downtime_duration: None,
            downtime_duration_secs: None,
            timestamp: now_str.clone(),
            domain_days_remaining: None,
            domain_expires_at: None,
        };
        let pool = state.db.clone();
        let client = state.http_client.clone();
        tokio::spawn(async move {
            crate::alerting::dispatch_alerts(&pool, &client, &alert_payload).await;
        });
    }

    // Broadcast check completed
    let _ = state.events_tx.send(
        serde_json::json!({
            "event": "check_completed",
            "monitor_id": monitor.id,
            "status": new_status,
            "latency_ms": latency_ms,
        })
        .to_string(),
    );

    state.invalidate_status_cache().await;

    Ok((
        StatusCode::OK,
        Json(PushResponse {
            ok: true,
            monitor_id: monitor.id,
            status: new_status,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_push_heartbeat_endpoints_and_transitions() {
        let pool = crate::db::init_pool(":memory:").await.unwrap();
        crate::db::run_migrations(&pool).await.unwrap();

        let config = crate::config::Config::default();
        let state = AppState::new_for_test(pool.clone(), config);
        let app = crate::build_http_app(state.clone());

        let push_token = generate_push_token();

        // Create a push monitor
        sqlx::query(
            "INSERT INTO monitors (id, name, url, check_interval_secs, timeout_secs, max_retries, is_active, is_public, last_status, push_token, grace_secs) \
             VALUES ('mon-push-1', 'Cron Job', 'push://backup-cron', 300, 10, 1, 1, 1, 'UP', ?, 60)"
        )
        .bind(&push_token)
        .execute(&pool)
        .await
        .unwrap();

        // 1. Send heartbeat with status=UP and latency=42
        let req = Request::builder()
            .method("GET")
            .uri(format!("/api/push/{push_token}?status=UP&latency_ms=42"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(res.into_body(), 64 * 1024).await.unwrap();
        let resp: PushResponse = serde_json::from_slice(&bytes).unwrap();
        assert!(resp.ok);
        assert_eq!(resp.status, "UP");

        // 2. Send heartbeat with status=DOWN
        let req_down = Request::builder()
            .method("POST")
            .uri(format!("/api/push/{push_token}?status=DOWN&msg=Disk%20full"))
            .body(Body::empty())
            .unwrap();
        let res_down = app.clone().oneshot(req_down).await.unwrap();
        assert_eq!(res_down.status(), StatusCode::OK);

        // Verify monitor transitioned DOWN
        let mon: crate::db::monitors::Monitor = sqlx::query_as("SELECT * FROM monitors WHERE id = 'mon-push-1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(mon.last_status, "DOWN");
        assert!(mon.down_since.is_some());

        // 3. Unknown token returns 404
        let bad_token_req = Request::builder()
            .method("GET")
            .uri("/api/push/non-existent-token-123456789012")
            .body(Body::empty())
            .unwrap();
        let bad_token_res = app.clone().oneshot(bad_token_req).await.unwrap();
        assert_eq!(bad_token_res.status(), StatusCode::NOT_FOUND);

        // 4. Invalid status string returns 400
        let invalid_req = Request::builder()
            .method("GET")
            .uri(format!("/api/push/{push_token}?status=FOOBAR"))
            .body(Body::empty())
            .unwrap();
        let invalid_res = app.clone().oneshot(invalid_req).await.unwrap();
        assert_eq!(invalid_res.status(), StatusCode::BAD_REQUEST);
    }
}
