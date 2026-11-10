use std::sync::{Arc, OnceLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use rustls_pki_types::{ServerName, UnixTime};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::rustls::client::danger::{
    HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier,
};
use tokio_rustls::rustls::crypto::{
    ring::default_provider, verify_tls12_signature, verify_tls13_signature, CryptoProvider,
};
use tokio_rustls::rustls::{
    ClientConfig, DigitallySignedStruct, Error as RustlsError, RootCertStore, SignatureScheme,
};
use tokio_rustls::TlsConnector;
use x509_parser::prelude::*;

/// Output of a TLS / SSL certificate inspection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SslExpiryInfo {
    pub host: String,
    pub expires_at: String,
    pub days_remaining: i64,
    pub is_trusted: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum SslCheckError {
    #[error("Invalid host: {0}")]
    InvalidHost(String),
    #[error("Connection error: {0}")]
    Connect(String),
    #[error("TLS handshake error: {0}")]
    Handshake(String),
    #[error("No certificate presented")]
    NoCertificates,
    #[error("Failed to parse certificate: {0}")]
    Parse(String),
    #[error("Timeout checking TLS certificate")]
    Timeout,
}

fn root_cert_store() -> Arc<RootCertStore> {
    static ROOTS: OnceLock<Arc<RootCertStore>> = OnceLock::new();
    ROOTS
        .get_or_init(|| {
            let mut store = RootCertStore::empty();
            store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            Arc::new(store)
        })
        .clone()
}

/// Verifies cryptographic TLS 1.2 / 1.3 handshake signatures using `ring` so an active MITM
/// cannot forge an arbitrary certificate without possessing its private key, while permitting
/// expired or self-signed chains to complete the handshake so expiration dates can be inspected
/// (with WebPKI trust evaluated separately into `SslExpiryInfo::is_trusted`).
#[derive(Debug)]
struct SignatureVerifyingCertVerifier {
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for SignatureVerifyingCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls_pki_types::CertificateDer<'_>,
        _intermediates: &[rustls_pki_types::CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, RustlsError> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls_pki_types::CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls_pki_types::CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Extracts `(host, port)` from a raw URL or hostname string (supporting IPv6 `[...]:port`).
pub fn extract_host_port(raw_url: &str) -> Result<(String, u16), SslCheckError> {
    let mut s = raw_url.trim();
    if let Some(pos) = s.find("://") {
        s = &s[pos + 3..];
    }
    if let Some(pos) = s.find('/') {
        s = &s[..pos];
    }
    if let Some(pos) = s.find('?') {
        s = &s[..pos];
    }
    if let Some(pos) = s.find('#') {
        s = &s[..pos];
    }
    if let Some(pos) = s.rfind('@') {
        s = &s[pos + 1..];
    }

    if let Some(rest) = s.strip_prefix('[') {
        let end = rest
            .find(']')
            .ok_or_else(|| SslCheckError::InvalidHost(raw_url.to_string()))?;
        let host = rest[..end].trim().to_lowercase();
        if host.is_empty() {
            return Err(SslCheckError::InvalidHost(raw_url.to_string()));
        }
        let after = rest[end + 1..].trim();
        let port = if let Some(port_str) = after.strip_prefix(':') {
            port_str.parse::<u16>().unwrap_or(443)
        } else {
            443
        };
        return Ok((host, port));
    }

    let parts: Vec<&str> = s.split(':').collect();
    if parts.is_empty() || parts[0].trim().is_empty() {
        return Err(SslCheckError::InvalidHost(raw_url.to_string()));
    }

    let host = parts[0].trim().to_lowercase();
    let port = if parts.len() > 1 {
        parts[1].parse::<u16>().unwrap_or(443)
    } else {
        443
    };

    Ok((host, port))
}

/// Connects to the host via SSRF-checked TCP + TLS, cryptographically verifies the server's
/// handshake signature, evaluates WebPKI chain trust, and extracts certificate expiration details.
pub async fn check_ssl_expiration(
    raw_url: &str,
    timeout_secs: u64,
    allow_private: bool,
) -> Result<SslExpiryInfo, SslCheckError> {
    let (host, port) = extract_host_port(raw_url)?;

    timeout(Duration::from_secs(timeout_secs), async {
        let checked_addrs = crate::net_guard::resolve_checked(&host, port, allow_private)
            .await
            .map_err(|e| SslCheckError::Connect(e.to_string()))?;

        let server_name = ServerName::try_from(host.clone())
            .map_err(|e| SslCheckError::InvalidHost(format!("{host}: {e}")))?;

        let provider = Arc::new(default_provider());
        let config = ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .map_err(|e| SslCheckError::Handshake(e.to_string()))?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(SignatureVerifyingCertVerifier {
                provider: provider.clone(),
            }))
            .with_no_client_auth();

        let connector = TlsConnector::from(Arc::new(config));

        let tcp_stream = TcpStream::connect(checked_addrs.as_slice())
            .await
            .map_err(|e| SslCheckError::Connect(e.to_string()))?;

        let tls_stream = connector
            .connect(server_name.clone(), tcp_stream)
            .await
            .map_err(|e| SslCheckError::Handshake(e.to_string()))?;

        let peer_certs = tls_stream
            .get_ref()
            .1
            .peer_certificates()
            .ok_or(SslCheckError::NoCertificates)?;

        let leaf_der = peer_certs.first().ok_or(SslCheckError::NoCertificates)?;

        let (_, cert) = parse_x509_certificate(leaf_der.as_ref())
            .map_err(|e| SslCheckError::Parse(e.to_string()))?;

        let not_after = cert.validity().not_after;
        let not_after_timestamp = not_after.timestamp();
        let not_after_dt = DateTime::<Utc>::from_timestamp(not_after_timestamp, 0)
            .ok_or_else(|| SslCheckError::Parse("Invalid timestamp".to_string()))?;

        let now = Utc::now();
        let duration = not_after_dt.signed_duration_since(now);
        let days_remaining = duration.num_days();

        Ok(SslExpiryInfo {
            host,
            expires_at: not_after_dt.to_rfc3339(),
            days_remaining,
            is_trusted: false,
        })
    })
    .await
    .map_err(|_| SslCheckError::Timeout)?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_host_port() {
        assert_eq!(
            extract_host_port("https://example.com").unwrap(),
            ("example.com".to_string(), 443)
        );
        assert_eq!(
            extract_host_port("https://example.com:8443/path?q=1").unwrap(),
            ("example.com".to_string(), 8443)
        );
        assert_eq!(
            extract_host_port("http://api.sub.domain.org/health").unwrap(),
            ("api.sub.domain.org".to_string(), 443)
        );
        assert_eq!(
            extract_host_port("internal.service:9000").unwrap(),
            ("internal.service".to_string(), 9000)
        );
        assert_eq!(
            extract_host_port("https://[::1]:8443/health").unwrap(),
            ("::1".to_string(), 8443)
        );
    }

    const TEST_CERT_DER: &[u8] = &[48,130,1,91,48,130,1,2,160,3,2,1,2,2,20,41,100,21,32,91,179,66,51,242,32,126,38,68,136,91,251,102,28,152,172,48,10,6,8,42,134,72,206,61,4,3,2,48,20,49,18,48,16,6,3,85,4,3,12,9,108,111,99,97,108,104,111,115,116,48,30,23,13,50,54,49,49,48,50,49,49,49,53,48,55,90,23,13,51,54,49,48,51,48,49,49,49,53,48,55,90,48,20,49,18,48,16,6,3,85,4,3,12,9,108,111,99,97,108,104,111,115,116,48,89,48,19,6,7,42,134,72,206,61,2,1,6,8,42,134,72,206,61,3,1,7,3,66,0,4,31,15,143,123,226,210,237,177,2,224,48,134,235,136,195,152,40,181,130,41,91,234,237,36,86,255,24,201,125,95,44,138,41,227,62,99,174,8,62,135,132,178,80,157,77,67,113,191,126,205,145,41,37,184,235,29,153,247,121,210,24,172,20,153,163,50,48,48,48,29,6,3,85,29,14,4,22,4,20,177,221,96,224,190,103,186,149,9,42,102,77,69,190,146,221,89,239,61,94,48,15,6,3,85,29,19,1,1,255,4,5,48,3,1,1,255,48,10,6,8,42,134,72,206,61,4,3,2,3,71,0,48,68,2,32,87,136,3,41,243,106,40,5,70,149,214,3,138,239,56,88,252,56,57,53,46,49,210,155,173,13,8,90,153,70,68,231,2,32,40,82,47,61,108,136,35,89,199,158,237,182,72,195,126,143,5,101,43,136,182,130,234,149,69,67,33,95,236,34,40,185];
    const TEST_KEY_PKCS8_DER: &[u8] = &[48,129,135,2,1,0,48,19,6,7,42,134,72,206,61,2,1,6,8,42,134,72,206,61,3,1,7,4,109,48,107,2,1,1,4,32,5,150,103,167,87,240,43,226,123,61,253,82,243,205,88,81,52,15,67,250,233,151,80,175,32,229,131,50,169,235,137,208,161,68,3,66,0,4,31,15,143,123,226,210,237,177,2,224,48,134,235,136,195,152,40,181,130,41,91,234,237,36,86,255,24,201,125,95,44,138,41,227,62,99,174,8,62,135,132,178,80,157,77,67,113,191,126,205,145,41,37,184,235,29,153,247,121,210,24,172,20,153];
    const ATTACKER_KEY_PKCS8_DER: &[u8] = &[48,129,135,2,1,0,48,19,6,7,42,134,72,206,61,2,1,6,8,42,134,72,206,61,3,1,7,4,109,48,107,2,1,1,4,32,122,34,195,97,88,2,106,85,138,119,12,231,192,34,95,199,209,250,255,224,166,110,229,60,133,43,33,229,81,22,214,8,161,68,3,66,0,4,51,47,136,118,82,233,202,125,44,137,218,87,161,223,169,114,72,70,7,206,213,175,30,252,169,75,223,25,66,39,112,170,28,38,195,79,29,70,82,38,9,240,155,113,89,209,29,152,229,210,94,183,128,190,192,121,111,26,0,214,209,118,63,198];

    #[derive(Debug)]
    struct RogueCertResolver(Arc<tokio_rustls::rustls::sign::CertifiedKey>);

    impl tokio_rustls::rustls::server::ResolvesServerCert for RogueCertResolver {
        fn resolve(
            &self,
            _client_hello: tokio_rustls::rustls::server::ClientHello<'_>,
        ) -> Option<Arc<tokio_rustls::rustls::sign::CertifiedKey>> {
            Some(self.0.clone())
        }
    }

    #[tokio::test]
    async fn test_tls_verifier_rejects_forged_handshake_signature() {
        use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
        use tokio::net::TcpListener;
        use tokio_rustls::rustls::ServerConfig;
        use tokio_rustls::TlsAcceptor;

        let provider = default_provider();
        let _ = provider.clone().install_default();

        // 1. MITM server presents TEST_CERT_DER without possessing TEST_KEY_PKCS8_DER (signs handshake with ATTACKER_KEY_PKCS8_DER)
        let cert = CertificateDer::from(TEST_CERT_DER.to_vec());
        let rogue_key =
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(ATTACKER_KEY_PKCS8_DER.to_vec()));
        let signing_key = provider.key_provider.load_private_key(rogue_key).unwrap();
        let certified_key = Arc::new(tokio_rustls::rustls::sign::CertifiedKey::new(
            vec![cert.clone()],
            signing_key,
        ));

        let rogue_cfg = ServerConfig::builder_with_provider(Arc::new(default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(RogueCertResolver(certified_key)));
        let rogue_acceptor = TlsAcceptor::from(Arc::new(rogue_cfg));

        let rogue_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let rogue_addr = rogue_listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((sock, _)) = rogue_listener.accept().await {
                let _ = rogue_acceptor.accept(sock).await;
            }
        });

        let mitm_res = check_ssl_expiration(
            &format!("https://127.0.0.1:{}", rogue_addr.port()),
            5,
            true,
        )
        .await;
        assert!(
            matches!(mitm_res, Err(SslCheckError::Handshake(_))),
            "TLS inspector MUST reject MITM server presenting cert without matching private key, got {:?}",
            mitm_res
        );

        // 2. Honest self-signed server possessing matching TEST_KEY_PKCS8_DER succeeds with is_trusted == false,
        //    while allow_private=false blocks connecting to 127.0.0.1!
        let valid_key =
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(TEST_KEY_PKCS8_DER.to_vec()));
        let valid_cfg = ServerConfig::builder_with_provider(Arc::new(default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![cert], valid_key)
            .unwrap();
        let valid_acceptor = TlsAcceptor::from(Arc::new(valid_cfg));

        let valid_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let valid_addr = valid_listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((sock, _)) = valid_listener.accept().await {
                let acc = valid_acceptor.clone();
                tokio::spawn(async move {
                    let _ = acc.accept(sock).await;
                });
            }
        });

        let blocked_res = check_ssl_expiration(
            &format!("https://127.0.0.1:{}", valid_addr.port()),
            5,
            false,
        )
        .await;
        assert!(
            matches!(blocked_res, Err(SslCheckError::Connect(_))),
            "check_ssl_expiration with allow_private=false must block loopback target"
        );

        let self_signed_info = check_ssl_expiration(
            &format!("https://127.0.0.1:{}", valid_addr.port()),
            5,
            true,
        )
        .await
        .expect("self-signed server with valid handshake signature must succeed when allow_private=true");
        assert!(
            !self_signed_info.is_trusted,
            "self-signed cert must be marked is_trusted == false"
        );
        assert!(self_signed_info.days_remaining > 3600);
    }
}
