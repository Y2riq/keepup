use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

const CLOUD_METADATA_IPV4: Ipv4Addr = Ipv4Addr::new(169, 254, 169, 254);
const AWS_IMDS_IPV6: Ipv6Addr = Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254);

#[derive(Debug, thiserror::Error)]
pub enum NetGuardError {
    #[error("Blocked outbound target IP {0} by SSRF policy")]
    BlockedIp(IpAddr),
    #[error("Invalid or empty target host: {0}")]
    InvalidHost(String),
    #[error("DNS resolution failed for {0}: {1}")]
    ResolveFailed(String, String),
}

fn is_always_blocked_v4(v4: Ipv4Addr) -> bool {
    v4.is_unspecified()
        || v4.is_broadcast()
        || v4.is_multicast()
        || v4 == CLOUD_METADATA_IPV4
}

fn is_non_public_v4(v4: Ipv4Addr) -> bool {
    let octets = v4.octets();
    is_always_blocked_v4(v4)
        || v4.is_private()
        || v4.is_loopback()
        || v4.is_link_local()
        || v4.is_documentation()
        || octets[0] == 0
        // Carrier-Grade NAT 100.64.0.0/10
        || (octets[0] == 100 && (octets[1] & 0b1100_0000) == 64)
        // IETF protocol assignments 192.0.0.0/24
        || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
        // Benchmarking 198.18.0.0/15
        || (octets[0] == 198 && (octets[1] & 0b1111_1110) == 18)
        // Reserved Class E 240.0.0.0/4
        || octets[0] >= 240
}

fn is_always_blocked_v6(v6: Ipv6Addr) -> bool {
    if let Some(v4) = v6.to_ipv4_mapped() {
        return is_always_blocked_v4(v4);
    }
    let seg = v6.segments();

    // 6to4 2002::/16 extracts embedded IPv4 in segments 1 and 2
    if seg[0] == 0x2002 {
        let v4 = Ipv4Addr::new(
            (seg[1] >> 8) as u8,
            seg[1] as u8,
            (seg[2] >> 8) as u8,
            seg[2] as u8,
        );
        if is_always_blocked_v4(v4) {
            return true;
        }
    }

    // NAT64 Well-Known Prefix 64:ff9b::/96 extracts embedded IPv4 in segments 6 and 7
    if seg[0] == 0x0064
        && seg[1] == 0xff9b
        && seg[2] == 0
        && seg[3] == 0
        && seg[4] == 0
        && seg[5] == 0
    {
        let v4 = Ipv4Addr::new(
            (seg[6] >> 8) as u8,
            seg[6] as u8,
            (seg[7] >> 8) as u8,
            seg[7] as u8,
        );
        if is_always_blocked_v4(v4) {
            return true;
        }
    }

    v6.is_unspecified() || v6.is_multicast() || v6 == AWS_IMDS_IPV6
}

fn is_non_public_v6(v6: Ipv6Addr) -> bool {
    if let Some(v4) = v6.to_ipv4_mapped() {
        return is_non_public_v4(v4);
    }
    let seg = v6.segments();

    // 6to4 2002::/16 extracts embedded IPv4 in segments 1 and 2
    if seg[0] == 0x2002 {
        let v4 = Ipv4Addr::new(
            (seg[1] >> 8) as u8,
            seg[1] as u8,
            (seg[2] >> 8) as u8,
            seg[2] as u8,
        );
        if is_non_public_v4(v4) {
            return true;
        }
    }

    // NAT64 Well-Known Prefix 64:ff9b::/96 extracts embedded IPv4 in segments 6 and 7
    if seg[0] == 0x0064
        && seg[1] == 0xff9b
        && seg[2] == 0
        && seg[3] == 0
        && seg[4] == 0
        && seg[5] == 0
    {
        let v4 = Ipv4Addr::new(
            (seg[6] >> 8) as u8,
            seg[6] as u8,
            (seg[7] >> 8) as u8,
            seg[7] as u8,
        );
        if is_non_public_v4(v4) {
            return true;
        }
    }

    is_always_blocked_v6(v6)
        || v6.is_loopback()
        // Unique Local fc00::/7
        || (seg[0] & 0xfe00) == 0xfc00
        // Link-Local fe80::/10
        || (seg[0] & 0xffc0) == 0xfe80
        // Documentation 2001:db8::/32
        || (seg[0] == 0x2001 && seg[1] == 0x0db8)
}

/// Evaluates whether an outbound socket connection to `ip` is permitted.
///
/// Cloud metadata (`169.254.169.254`, `::ffff:169.254.169.254`, `fd00:ec2::254`),
/// unspecified (`0.0.0.0`, `::`), broadcast, and multicast addresses are **always**
/// rejected regardless of `allow_private`.
pub fn is_allowed_ip(ip: IpAddr, allow_private: bool) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            if is_always_blocked_v4(v4) {
                false
            } else if allow_private {
                true
            } else {
                !is_non_public_v4(v4)
            }
        }
        IpAddr::V6(v6) => {
            if is_always_blocked_v6(v6) {
                false
            } else if allow_private {
                true
            } else {
                !is_non_public_v6(v6)
            }
        }
    }
}

fn parse_host_ip(host: &str) -> Option<IpAddr> {
    let trimmed = host
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']');
    trimmed.parse::<IpAddr>().ok()
}

/// Detects cross-zone redirects from a public origin to a private/loopback IP,
/// or any redirect targeting an unconditionally blocked IP (cloud metadata / unspecified).
pub fn is_cross_zone_redirect(initial_ip_or_host: &str, next_ip: IpAddr) -> bool {
    if !is_allowed_ip(next_ip, true) {
        return true;
    }
    // Redirecting to a public IP is never a public -> private escalation
    if is_allowed_ip(next_ip, false) {
        return false;
    }
    // `next_ip` is private/loopback/internal. Check if origin was public.
    if let Some(orig_ip) = parse_host_ip(initial_ip_or_host) {
        return is_allowed_ip(orig_ip, false);
    }
    let lower = initial_ip_or_host.trim().to_ascii_lowercase();
    if lower == "localhost"
        || lower.ends_with(".localhost")
        || lower.ends_with(".local")
        || lower.ends_with(".internal")
        || !lower.contains('.')
    {
        false
    } else {
        true
    }
}

/// Resolves `(host, port)` and verifies that **every** returned IP address satisfies
/// `is_allowed_ip(..., allow_private)` before any TCP/TLS/DNS socket is opened.
pub async fn resolve_checked(
    host: &str,
    port: u16,
    allow_private: bool,
) -> Result<Vec<SocketAddr>, NetGuardError> {
    let clean = host
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']');
    if clean.is_empty() {
        return Err(NetGuardError::InvalidHost(host.to_string()));
    }

    if let Ok(ip) = clean.parse::<IpAddr>() {
        if !is_allowed_ip(ip, allow_private) {
            return Err(NetGuardError::BlockedIp(ip));
        }
        return Ok(vec![SocketAddr::new(ip, port)]);
    }

    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((clean, port))
        .await
        .map_err(|e| NetGuardError::ResolveFailed(clean.to_string(), e.to_string()))?
        .collect();

    if addrs.is_empty() {
        return Err(NetGuardError::ResolveFailed(
            clean.to_string(),
            "no addresses returned".to_string(),
        ));
    }

    for addr in &addrs {
        if !is_allowed_ip(addr.ip(), allow_private) {
            return Err(NetGuardError::BlockedIp(addr.ip()));
        }
    }

    Ok(addrs)
}

#[derive(Debug, Clone)]
struct GuardedResolver {
    allow_private: bool,
}

impl Resolve for GuardedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_string();
        let allow_private = self.allow_private;
        Box::pin(async move {
            if host.eq_ignore_ascii_case("blocked.ssrf.invalid") {
                return Err(Box::new(NetGuardError::InvalidHost(
                    "Blocked outbound target IP or scheme by SSRF policy".to_string(),
                )) as Box<dyn std::error::Error + Send + Sync>);
            }
            let addrs = resolve_checked(&host, 0, allow_private)
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            let iter: Addrs = Box::new(addrs.into_iter());
            Ok(iter)
        })
    }
}

/// Builds a shared `reqwest::Client` hardened with:
/// 1. Custom `Proxy::custom` gate catching hop-0 IP literals and non-HTTP(S) schemes before `HttpConnector` bypasses DNS.
/// 2. Custom `GuardedResolver` rejecting private/loopback/link-local/metadata IPs on every DNS lookup (mitigating DNS rebinding).
/// 3. Explicit redirect policy: max 5 hops, `http`/`https` schemes only, literal IP validation, and cross-zone (public -> private) redirect blocking.
pub fn build_http_client(
    allow_private: bool,
    default_timeout: Duration,
) -> anyhow::Result<reqwest::Client> {
    let sink_url = reqwest::Url::parse("http://blocked.ssrf.invalid:1")
        .expect("static SSRF sink URL must parse");
    let literal_ip_gate = reqwest::Proxy::custom(move |url| {
        let scheme = url.scheme();
        if scheme != "http" && scheme != "https" {
            return Some(sink_url.clone());
        }
        if let Some(host) = url.host_str() {
            if let Some(ip) = parse_host_ip(host) {
                if !is_allowed_ip(ip, allow_private) {
                    return Some(sink_url.clone());
                }
            }
        }
        None
    });

    let redirect_policy = reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= 5 {
            return attempt.error("Too many HTTP redirects (max 5)");
        }

        let scheme = attempt.url().scheme().to_string();
        if scheme != "http" && scheme != "https" {
            return attempt.error(format!("Blocked redirect to disallowed scheme: {scheme}"));
        }

        let next_host_opt = attempt.url().host_str().map(str::to_string);
        let Some(next_host) = next_host_opt else {
            return attempt.error("Redirect target missing host");
        };

        if let Some(next_ip) = parse_host_ip(&next_host) {
            if !is_allowed_ip(next_ip, allow_private) {
                return attempt.error(format!(
                    "Blocked HTTP redirect to disallowed IP {next_ip}"
                ));
            }
            let orig_host_opt = attempt
                .previous()
                .first()
                .and_then(|u| u.host_str())
                .map(str::to_string);
            if let Some(orig_host) = orig_host_opt {
                if is_cross_zone_redirect(&orig_host, next_ip) {
                    return attempt.error(format!(
                        "Blocked cross-zone HTTP redirect from {orig_host} to {next_ip}"
                    ));
                }
            }
        }

        attempt.follow()
    });

    Ok(reqwest::Client::builder()
        .timeout(default_timeout)
        .connect_timeout(Duration::from_secs(5))
        .user_agent("KeepUp-Monitor/2.0")
        .no_proxy()
        .proxy(literal_ip_gate)
        .dns_resolver(Arc::new(GuardedResolver { allow_private }))
        .redirect(redirect_policy)
        .build()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn test_is_allowed_ip_blocks_private_loopback_and_always_blocks_metadata() {
        let blocked_by_default = [
            "127.0.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "172.31.255.254",
            "192.168.1.1",
            "100.64.0.1",
            "169.254.1.10",
            "169.254.169.254",
            "0.0.0.0",
            "::1",
            "::",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
        ];

        for raw in blocked_by_default {
            let ip: IpAddr = raw.parse().unwrap();
            assert!(
                !is_allowed_ip(ip, false),
                "expected {raw} to be blocked when allow_private=false"
            );
        }

        for public_raw in ["8.8.8.8", "1.1.1.1", "93.184.216.34", "2606:4700:4700::1111"] {
            let ip: IpAddr = public_raw.parse().unwrap();
            assert!(is_allowed_ip(ip, false), "expected public IP {public_raw} to be allowed");
            assert!(is_allowed_ip(ip, true), "expected public IP {public_raw} to be allowed");
        }

        // When allow_private=true, RFC1918 & loopback are permitted, ONLY cloud metadata & unspecified stay blocked
        for private_ok in ["127.0.0.1", "10.0.0.5", "192.168.1.20", "::1"] {
            let ip: IpAddr = private_ok.parse().unwrap();
            assert!(
                is_allowed_ip(ip, true),
                "expected {private_ok} to be allowed when allow_private=true"
            );
        }

        for always_blocked in [
            "169.254.169.254",
            "::ffff:169.254.169.254",
            "fd00:ec2::254",
            "0.0.0.0",
            "::",
        ] {
            let ip: IpAddr = always_blocked.parse().unwrap();
            assert!(
                !is_allowed_ip(ip, true),
                "expected {always_blocked} to remain ALWAYS blocked even when allow_private=true"
            );
        }
    }

    #[tokio::test]
    async fn test_http_client_blocks_redirect_ssrf_and_metadata_redirect() {
        // Target listener that must NEVER receive a request under allow_private=false
        let secret_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let secret_addr = secret_listener.local_addr().unwrap();
        let secret_hits = Arc::new(AtomicUsize::new(0));
        let secret_hits_clone = secret_hits.clone();
        tokio::spawn(async move {
            if let Ok((_sock, _)) = secret_listener.accept().await {
                secret_hits_clone.fetch_add(1, Ordering::SeqCst);
            }
        });

        // Redirector 1 -> redirects to 127.0.0.1:<secret_port>
        let redir1 = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let redir1_addr = redir1.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = redir1.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let resp = format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://{secret_addr}/secret\r\nContent-Length: 0\r\n\r\n"
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });

        // Client with allow_private=false blocks direct & redirect access to 127.0.0.1
        let strict_client = build_http_client(false, Duration::from_secs(3)).unwrap();
        let direct_res = strict_client
            .get(format!("http://{secret_addr}/direct"))
            .send()
            .await;
        assert!(direct_res.is_err(), "strict client must block direct loopback IP literal");

        let strict_res = strict_client
            .get(format!("http://{redir1_addr}/start"))
            .send()
            .await;
        assert!(strict_res.is_err(), "strict client must block loopback target/redirect");
        assert_eq!(secret_hits.load(Ordering::SeqCst), 0);

        // Redirector 2 -> redirects to cloud metadata http://169.254.169.254/latest/meta-data
        let redir2 = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let redir2_addr = redir2.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = redir2.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let resp = "HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/latest/meta-data/\r\nContent-Length: 0\r\n\r\n";
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });

        // Even with allow_private=true, redirect to 169.254.169.254 MUST fail!
        let priv_client = build_http_client(true, Duration::from_secs(3)).unwrap();
        let meta_res = priv_client
            .get(format!("http://{redir2_addr}/start"))
            .send()
            .await;
        assert!(
            meta_res.is_err(),
            "redirect to 169.254.169.254 must be blocked even when allow_private=true"
        );

        // Cross-zone redirect from a public IP/host to a private/loopback IP MUST be flagged even under allow_private=true
        assert!(is_cross_zone_redirect(
            "93.184.216.34",
            "127.0.0.1".parse().unwrap()
        ));
        assert!(is_cross_zone_redirect(
            "93.184.216.34",
            "10.0.0.5".parse().unwrap()
        ));
        assert!(!is_cross_zone_redirect(
            "10.0.0.1",
            "10.0.0.5".parse().unwrap()
        ));
    }
}
