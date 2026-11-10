use std::time::{Duration, Instant};

use reqwest::Client;

/// Outcome of a single health-check ping.
#[derive(Debug, Clone)]
pub struct CheckResult {
    /// HTTP status code or port number (if applicable).
    pub status_code: Option<i32>,
    /// Round-trip latency in milliseconds.
    pub latency_ms: i64,
    /// Whether the target is considered healthy.
    pub is_up: bool,
    /// Whether the target is degraded (e.g. latency exceeds degraded_latency_ms).
    pub is_degraded: bool,
    /// Human-readable error description (populated on failure).
    pub error_msg: Option<String>,
}

/// Advanced configuration for an HTTP probe.
#[derive(Debug, Clone)]
pub struct HttpProbeConfig<'a> {
    pub url: &'a str,
    pub method: &'a str,
    pub expected_status: &'a str,
    pub headers_json: Option<&'a str>,
    pub body_keyword: Option<&'a str>,
    pub json_pointer: Option<&'a str>,
    pub json_expected: Option<&'a str>,
    pub degraded_latency_ms: Option<i64>,
    pub timeout_secs: u64,
}

/// Verify if `status` matches the `expected` specification (e.g. "200-299", "200,202,204").
pub fn matches_expected_status(status: u16, expected: &str) -> bool {
    let exp = expected.trim();
    if exp.is_empty() || exp == "2xx" || exp == "200-299" {
        return (200..=299).contains(&status);
    }

    for part in exp.split(',') {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        if let Some((start_s, end_s)) = p.split_once('-') {
            if let (Ok(start), Ok(end)) = (start_s.trim().parse::<u16>(), end_s.trim().parse::<u16>()) {
                if status >= start && status <= end {
                    return true;
                }
            }
        } else if let Ok(code) = p.parse::<u16>() {
            if status == code {
                return true;
            }
        }
    }
    false
}

/// Execute an HTTP probe with full configuration support (method, status assertions, headers, body/json assertions).
pub async fn ping_http_with_config(client: &Client, config: &HttpProbeConfig<'_>) -> CheckResult {
    let start = Instant::now();

    let method = match config.method.to_uppercase().as_str() {
        "POST" => reqwest::Method::POST,
        "PUT" => reqwest::Method::PUT,
        "PATCH" => reqwest::Method::PATCH,
        "DELETE" => reqwest::Method::DELETE,
        "HEAD" => reqwest::Method::HEAD,
        _ => reqwest::Method::GET,
    };

    let mut req = client
        .request(method, config.url)
        .header(reqwest::header::USER_AGENT, "KeepUp-Monitor/1.0")
        .timeout(Duration::from_secs(config.timeout_secs));

    // Custom headers from JSON if provided
    if let Some(json_str) = config.headers_json {
        if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(json_str) {
            for (k, v) in map {
                if let (Ok(hname), Some(v_str)) = (
                    reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                    v.as_str(),
                ) {
                    if let Ok(hval) = reqwest::header::HeaderValue::from_str(v_str) {
                        req = req.header(hname, hval);
                    }
                }
            }
        }
    }

    let result = req.send().await;
    let latency_ms = start.elapsed().as_millis() as i64;

    match result {
        Ok(resp) => {
            let status = resp.status().as_u16();
            if !matches_expected_status(status, config.expected_status) {
                return CheckResult {
                    status_code: Some(status as i32),
                    latency_ms,
                    is_up: false,
                    is_degraded: false,
                    error_msg: Some(format!("Expected HTTP status '{}', got {status}", config.expected_status)),
                };
            }

            // If body keyword or JSON pointer inspection is configured, stream up to 64KB safely to prevent unbounded memory allocation
            if config.body_keyword.is_some() || config.json_pointer.is_some() {
                let max_body_bytes: usize = 64 * 1024;
                let mut bytes = Vec::with_capacity(4096);
                let mut stream = resp;
                while let Ok(Some(chunk)) = stream.chunk().await {
                    let to_take = chunk.len().min(max_body_bytes.saturating_sub(bytes.len()));
                    bytes.extend_from_slice(&chunk[..to_take]);
                    if bytes.len() >= max_body_bytes {
                        break;
                    }
                }

                let body_slice = &bytes[..];
                let body_str = String::from_utf8_lossy(body_slice);

                if let Some(kw) = config.body_keyword {
                    if !body_str.contains(kw) {
                        return CheckResult {
                            status_code: Some(status as i32),
                            latency_ms,
                            is_up: false,
                            is_degraded: false,
                            error_msg: Some(format!("Body keyword '{kw}' not found in response")),
                        };
                    }
                }

                if let Some(pointer) = config.json_pointer {
                    let parsed: serde_json::Value = match serde_json::from_slice(body_slice) {
                        Ok(v) => v,
                        Err(e) => {
                            return CheckResult {
                                status_code: Some(status as i32),
                                latency_ms,
                                is_up: false,
                                is_degraded: false,
                                error_msg: Some(format!("Response body is not valid JSON: {e}")),
                            };
                        }
                    };

                    match parsed.pointer(pointer) {
                        Some(val) => {
                            if let Some(expected_val) = config.json_expected {
                                let actual_str = match val {
                                    serde_json::Value::String(s) => s.clone(),
                                    other => other.to_string(),
                                };
                                if actual_str != expected_val {
                                    return CheckResult {
                                        status_code: Some(status as i32),
                                        latency_ms,
                                        is_up: false,
                                        is_degraded: false,
                                        error_msg: Some(format!(
                                            "JSON pointer '{pointer}' mismatch: expected '{expected_val}', found '{actual_str}'"
                                        )),
                                    };
                                }
                            }
                        }
                        None => {
                            return CheckResult {
                                status_code: Some(status as i32),
                                latency_ms,
                                is_up: false,
                                is_degraded: false,
                                error_msg: Some(format!("JSON pointer '{pointer}' not found")),
                            };
                        }
                    }
                }
            }

            let is_degraded = config
                .degraded_latency_ms
                .map(|thresh| latency_ms >= thresh)
                .unwrap_or(false);

            CheckResult {
                status_code: Some(status as i32),
                latency_ms,
                is_up: true,
                is_degraded,
                error_msg: if is_degraded {
                    Some(format!("Degraded response time: {latency_ms}ms > threshold"))
                } else {
                    None
                },
            }
        }
        Err(err) => CheckResult {
            status_code: None,
            latency_ms,
            is_up: false,
            is_degraded: false,
            error_msg: Some(err.to_string()),
        },
    }
}

/// Simple convenience wrapper for standard GET health checks.
#[allow(dead_code)]
pub async fn ping_http(client: &Client, url: &str, timeout_secs: u64) -> CheckResult {
    let config = HttpProbeConfig {
        url,
        method: "GET",
        expected_status: "200-299",
        headers_json: None,
        body_keyword: None,
        json_pointer: None,
        json_expected: None,
        degraded_latency_ms: None,
        timeout_secs,
    };
    ping_http_with_config(client, &config).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matches_expected_status() {
        assert!(matches_expected_status(200, "200-299"));
        assert!(matches_expected_status(204, "200-299"));
        assert!(!matches_expected_status(301, "200-299"));
        assert!(matches_expected_status(200, "200,202,204"));
        assert!(matches_expected_status(202, "200,202,204"));
        assert!(!matches_expected_status(201, "200,202,204"));
        assert!(matches_expected_status(302, "200-299, 301-308"));
    }

    #[tokio::test]
    async fn test_http_probe_with_custom_headers_and_json_pointer() {
        use axum::routing::post;
        use axum::Json;
        use serde_json::json;

        let app = axum::Router::new().route(
            "/health",
            post(|headers: axum::http::HeaderMap| async move {
                if headers.get("x-health-token").and_then(|v| v.to_str().ok()) == Some("secret-token") {
                    (axum::http::StatusCode::OK, Json(json!({"db": {"state": "ok"}})))
                } else {
                    (axum::http::StatusCode::UNAUTHORIZED, Json(json!({"db": {"state": "unauthorized"}})))
                }
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let client = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{port}/health");

        // 1. Success matching headers, status 200, and JSON pointer
        let config_ok = HttpProbeConfig {
            url: &url,
            method: "POST",
            expected_status: "200,202",
            headers_json: Some(r#"{"X-Health-Token": "secret-token"}"#),
            body_keyword: Some("state"),
            json_pointer: Some("/db/state"),
            json_expected: Some("ok"),
            degraded_latency_ms: None,
            timeout_secs: 3,
        };
        let res = ping_http_with_config(&client, &config_ok).await;
        assert!(res.is_up);
        assert!(!res.is_degraded);
        assert_eq!(res.status_code, Some(200));

        // 2. Failure due to JSON pointer mismatch
        let config_mismatch = HttpProbeConfig {
            url: &url,
            method: "POST",
            expected_status: "200",
            headers_json: Some(r#"{"X-Health-Token": "secret-token"}"#),
            body_keyword: None,
            json_pointer: Some("/db/state"),
            json_expected: Some("down"),
            degraded_latency_ms: None,
            timeout_secs: 3,
        };
        let res_mismatch = ping_http_with_config(&client, &config_mismatch).await;
        assert!(!res_mismatch.is_up);
        assert!(res_mismatch.error_msg.unwrap().contains("mismatch"));

        // 3. Degraded latency threshold
        let config_degraded = HttpProbeConfig {
            url: &url,
            method: "POST",
            expected_status: "200",
            headers_json: Some(r#"{"X-Health-Token": "secret-token"}"#),
            body_keyword: None,
            json_pointer: Some("/db/state"),
            json_expected: Some("ok"),
            degraded_latency_ms: Some(0), // any response time > 0ms is degraded
            timeout_secs: 3,
        };
        let res_deg = ping_http_with_config(&client, &config_degraded).await;
        assert!(res_deg.is_up);
        assert!(res_deg.is_degraded);
    }
}
