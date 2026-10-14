//! The TLS handshake, with trust checked the way the operating system does.
//!
//! See docs/decisions/0003-runtime-and-tls.md for why rustls, `ring` and
//! rustls-platform-verifier.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustls::pki_types::{CertificateDer, ServerName, pem::PemObject};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;

/// Which application protocols to offer through ALPN.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Offer {
    Both,
    Http1,
    Http2,
}

pub fn client_config(
    cacert: Option<&Path>,
    offer: Offer,
) -> Result<Arc<rustls::ClientConfig>, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let extra: Vec<CertificateDer<'static>> = match cacert {
        Some(path) => CertificateDer::pem_file_iter(path)
            .and_then(|certs| certs.collect())
            .map_err(|e| format!("reading --cacert {}: {e}", path.display()))?,
        None => Vec::new(),
    };
    let verifier =
        rustls_platform_verifier::Verifier::new_with_extra_roots(extra, provider.clone())
            .map_err(|e| format!("loading trusted certificates: {e}"))?;
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("TLS setup: {e}"))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    config.alpn_protocols = match offer {
        Offer::Both => vec![b"h2".to_vec(), b"http/1.1".to_vec()],
        Offer::Http1 => vec![b"http/1.1".to_vec()],
        Offer::Http2 => vec![b"h2".to_vec()],
    };
    Ok(Arc::new(config))
}

pub struct Handshake {
    pub stream: TlsStream<TcpStream>,
    pub took: Duration,
    pub version: String,
    pub cipher: String,
    pub h2: bool,
}

pub async fn handshake(
    config: Arc<rustls::ClientConfig>,
    host: &str,
    tcp: TcpStream,
) -> Result<Handshake, String> {
    let name = ServerName::try_from(host.to_owned())
        .map_err(|e| format!("invalid TLS name {host:?}: {e}"))?;
    let start = Instant::now();
    let stream = tokio_rustls::TlsConnector::from(config)
        .connect(name, tcp)
        .await
        .map_err(|e| format!("TLS handshake with {host} failed: {e}"))?;
    let took = start.elapsed();
    let conn = stream.get_ref().1;
    let version = conn
        .protocol_version()
        .map_or("unknown".to_owned(), |v| match v {
            rustls::ProtocolVersion::TLSv1_3 => "TLSv1.3".to_owned(),
            rustls::ProtocolVersion::TLSv1_2 => "TLSv1.2".to_owned(),
            other => format!("{other:?}"),
        });
    let cipher = conn
        .negotiated_cipher_suite()
        .map_or("unknown".to_owned(), |s| {
            s.suite().as_str().unwrap_or("unknown").to_owned()
        });
    let h2 = conn.alpn_protocol() == Some(b"h2");
    Ok(Handshake {
        stream,
        took,
        version,
        cipher,
        h2,
    })
}
