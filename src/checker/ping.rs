use std::net::IpAddr;
use std::time::{Duration, Instant};

use super::http::CheckResult;

/// Clean host string by stripping protocols, ports, and paths.
pub fn clean_ping_target(target: &str) -> String {
    let t = target.trim();
    let t = t
        .strip_prefix("ping://")
        .or_else(|| t.strip_prefix("icmp://"))
        .or_else(|| t.strip_prefix("https://"))
        .or_else(|| t.strip_prefix("http://"))
        .unwrap_or(t);
    let t = t.split('/').next().unwrap_or(t);
    let t = t.split(':').next().unwrap_or(t);
    let host = t.trim();
    if host.starts_with('-') {
        return String::new();
    }
    host.to_string()
}

/// Probe target host via ICMP ping using surge-ping with system ping command fallback.
pub async fn ping_icmp(target: &str, timeout_secs: u64) -> CheckResult {
    let start = Instant::now();
    let host = clean_ping_target(target);

    if host.is_empty() {
        return CheckResult {
            status_code: None,
            latency_ms: 0,
            is_up: false,
            is_degraded: false,
            error_msg: Some("ICMP target cannot be empty".to_string()),
        };
    }

    let ip: Option<IpAddr> = match host.parse::<IpAddr>() {
        Ok(addr) => Some(addr),
        Err(_) => {
            tokio::net::lookup_host(format!("{host}:80"))
                .await
                .ok()
                .and_then(|mut addrs| addrs.next().map(|sa| sa.ip()))
        }
    };

    // Try surge-ping raw socket probe first if IP is known
    if let Some(resolved_ip) = ip {
        let payload = [0u8; 8];
        let surge_fut = surge_ping::ping(resolved_ip, &payload);
        if let Ok(Ok((_packet, duration))) =
            tokio::time::timeout(Duration::from_secs(timeout_secs), surge_fut).await
        {
            return CheckResult {
                status_code: None,
                latency_ms: duration.as_millis() as i64,
                is_up: true,
                is_degraded: false,
                error_msg: None,
            };
        }
    }

    // Fallback: System ping command execution (supports unprivileged / container environments)
    let fallback_start = Instant::now();
    let mut cmd = tokio::process::Command::new("ping");
    cmd.arg("-c").arg("1");

    #[cfg(target_os = "macos")]
    cmd.arg("-t").arg(timeout_secs.to_string());

    #[cfg(not(target_os = "macos"))]
    cmd.arg("-W").arg(timeout_secs.to_string());

    cmd.arg("--").arg(&host);

    let sys_result = tokio::time::timeout(Duration::from_secs(timeout_secs), cmd.output()).await;
    let latency_ms = start.elapsed().as_millis() as i64;

    match sys_result {
        Ok(Ok(output)) if output.status.success() => CheckResult {
            status_code: None,
            latency_ms: fallback_start.elapsed().as_millis() as i64,
            is_up: true,
            is_degraded: false,
            error_msg: None,
        },
        Ok(Ok(output)) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let msg = if !stderr.is_empty() {
                stderr
            } else {
                format!("Ping to {host} failed with exit code {}", output.status)
            };
            CheckResult {
                status_code: None,
                latency_ms,
                is_up: false,
                is_degraded: false,
                error_msg: Some(msg),
            }
        }
        Ok(Err(err)) => CheckResult {
            status_code: None,
            latency_ms,
            is_up: false,
            is_degraded: false,
            error_msg: Some(format!("Failed to execute ping probe: {err}")),
        },
        Err(_) => CheckResult {
            status_code: None,
            latency_ms: (timeout_secs * 1000) as i64,
            is_up: false,
            is_degraded: false,
            error_msg: Some(format!("Ping to {host} timed out after {timeout_secs}s")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_ping_target() {
        assert_eq!(clean_ping_target("ping://127.0.0.1"), "127.0.0.1");
        assert_eq!(clean_ping_target("https://8.8.8.8:443/ping"), "8.8.8.8");
        assert_eq!(clean_ping_target("localhost"), "localhost");
    }

    #[tokio::test]
    async fn test_ping_icmp_localhost() {
        let res = ping_icmp("127.0.0.1", 3).await;
        assert!(res.is_up);
        assert!(res.error_msg.is_none());
        assert!(res.latency_ms >= 0);
    }
}
