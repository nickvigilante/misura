//! Spike: time each phase of one HTTPS request, to check the approach
//! against curl before building the real CLI.
//!
//! Usage: misura URL [--cacert PATH] [--http1 | --http2]

use std::sync::Arc;
use std::time::{Duration, Instant};

use http_body_util::{BodyExt, Empty};
use hyper::body::Bytes;
use hyper::{Request, Uri};
use hyper_util::rt::{TokioExecutor, TokioIo};
use rustls::pki_types::{CertificateDer, ServerName, pem::PemObject};
use tokio::net::TcpStream;

#[derive(Debug, Default)]
struct Timings {
    dns: Duration,
    tcp: Duration,
    tls: Duration,
    /// From the request being handed to hyper to its response headers.
    wait: Duration,
    /// From the response headers to the end of the body.
    transfer: Duration,
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let uri: Uri = args.next().ok_or("usage: misura URL")?.parse()?;
    let mut cacert = None;
    let mut alpn: Vec<Vec<u8>> = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--cacert" => cacert = Some(args.next().ok_or("--cacert needs a path")?),
            "--http1" => alpn = vec![b"http/1.1".to_vec()],
            "--http2" => alpn = vec![b"h2".to_vec()],
            other => return Err(format!("unknown argument {other}").into()),
        }
    }
    let host = uri.host().ok_or("URL needs a host")?.to_owned();
    let https = uri.scheme_str() == Some("https");
    let port = uri.port_u16().unwrap_or(if https { 443 } else { 80 });

    let mut t = Timings::default();
    let start = Instant::now();

    // DNS: the system resolver, as other programs on this machine see it.
    let addrs: Vec<_> = tokio::net::lookup_host((host.as_str(), port))
        .await?
        .collect();
    let after_dns = Instant::now();
    t.dns = after_dns - start;

    // TCP: first address that connects.
    let mut tcp = None;
    for addr in &addrs {
        if let Ok(s) = TcpStream::connect(addr).await {
            tcp = Some((s, *addr));
            break;
        }
    }
    let (tcp, peer) = tcp.ok_or("could not connect to any address")?;
    tcp.set_nodelay(true)?;
    let after_tcp = Instant::now();
    t.tcp = after_tcp - after_dns;

    let req = Request::get(uri.path_and_query().map_or("/", |p| p.as_str()))
        .header("host", &host)
        .header("user-agent", "misura/0.1")
        .body(Empty::<Bytes>::new())?;

    let (status, protocol, body_len);
    let after_conn;
    if https {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let extra: Vec<CertificateDer<'static>> = match &cacert {
            Some(path) => CertificateDer::pem_file_iter(path)?.collect::<Result<_, _>>()?,
            None => Vec::new(),
        };
        let verifier =
            rustls_platform_verifier::Verifier::new_with_extra_roots(extra, provider.clone())?;
        let mut config = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier))
            .with_no_client_auth();
        config.alpn_protocols = alpn;
        let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
        let tls = connector
            .connect(ServerName::try_from(host.clone())?, tcp)
            .await?;
        after_conn = Instant::now();
        t.tls = after_conn - after_tcp;
        let h2 = tls.get_ref().1.alpn_protocol() == Some(b"h2");
        let io = TokioIo::new(tls);
        let resp = if h2 {
            protocol = "h2";
            let (mut send, conn) =
                hyper::client::conn::http2::handshake(TokioExecutor::new(), io).await?;
            tokio::spawn(conn);
            let mut req = req;
            *req.uri_mut() = uri.clone();
            req.headers_mut().remove("host");
            send.send_request(req).await?
        } else {
            protocol = "HTTP/1.1";
            let (mut send, conn) = hyper::client::conn::http1::handshake(io).await?;
            tokio::spawn(conn);
            send.send_request(req).await?
        };
        let after_headers = Instant::now();
        t.wait = after_headers - after_conn;
        status = resp.status();
        body_len = resp.into_body().collect().await?.to_bytes().len();
        t.transfer = after_headers.elapsed();
    } else {
        after_conn = after_tcp;
        protocol = "HTTP/1.1";
        let (mut send, conn) = hyper::client::conn::http1::handshake(TokioIo::new(tcp)).await?;
        tokio::spawn(conn);
        let resp = send.send_request(req).await?;
        let after_headers = Instant::now();
        t.wait = after_headers - after_conn;
        status = resp.status();
        body_len = resp.into_body().collect().await?.to_bytes().len();
        t.transfer = after_headers.elapsed();
    }

    println!("{peer} {protocol} {status} {body_len} bytes");
    println!(
        "dns={:.2} tcp={:.2} tls={:.2} wait={:.2} transfer={:.2} total={:.2}",
        ms(t.dns),
        ms(t.tcp),
        ms(t.tls),
        ms(t.wait),
        ms(t.transfer),
        ms(start.elapsed())
    );
    Ok(())
}
