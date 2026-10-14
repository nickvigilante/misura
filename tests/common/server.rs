//! A local server whose responses take known amounts of time, shared by the
//! integration tests and `examples/testserver.rs`.
//!
//! It serves plain HTTP/1.1, and HTTPS with HTTP/1.1 and h2 through ALPN
//! using a self-signed certificate for `localhost`. Query parameters shape
//! the response:
//! - `headers_ms=N`: wait N ms before sending the response headers.
//! - `body_ms=N`: after the headers, wait N ms before sending the body.
//! - `size=N`: body length in bytes (default 16).
//! - `status=N`: answer with status N.
//! - `method=M`: answer 400 unless the request method is M.
//! - `header=NAME:VALUE`: answer 400 unless the request has that header.
//! - `/redirect?to=URL`: answer 302 with `Location: URL` (URL-encoded).

#![allow(dead_code)]

use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use http_body_util::{BodyExt, Full, StreamBody, combinators::BoxBody};
use hyper::body::{Bytes, Frame};
use hyper::{Request, Response};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use tokio::net::TcpListener;

pub struct Server {
    pub http: SocketAddr,
    pub https: SocketAddr,
    /// The certificate to pass to `--cacert`.
    pub cert: PathBuf,
}

impl Server {
    pub fn http_url(&self, path: &str) -> String {
        format!("http://localhost:{}{path}", self.http.port())
    }

    pub fn https_url(&self, path: &str) -> String {
        format!("https://localhost:{}{path}", self.https.port())
    }
}

/// Starts the server on its own thread. Port 0 picks a free port.
pub fn start(dir: &Path, http_port: u16, https_port: u16) -> Server {
    // macOS only trusts server certificates that say they're for TLS
    // servers and are valid for a short time, even with a custom root.
    let mut params = rcgen::CertificateParams::new(vec!["localhost".to_owned()]).unwrap();
    params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
    let now = time::OffsetDateTime::now_utc();
    params.not_before = now - time::Duration::days(1);
    params.not_after = now + time::Duration::days(30);
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = params.self_signed(&key).unwrap();
    let cert_path = dir.join("cert.pem");
    std::fs::write(&cert_path, cert.pem()).unwrap();
    let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![cert.der().clone()],
        rustls::pki_types::PrivateKeyDer::Pkcs8(key.serialize_der().into()),
    )
    .unwrap();
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));

    let plain = std::net::TcpListener::bind(("127.0.0.1", http_port)).unwrap();
    let tls = std::net::TcpListener::bind(("127.0.0.1", https_port)).unwrap();
    let server = Server {
        http: plain.local_addr().unwrap(),
        https: tls.local_addr().unwrap(),
        cert: cert_path,
    };
    plain.set_nonblocking(true).unwrap();
    tls.set_nonblocking(true).unwrap();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let plain = TcpListener::from_std(plain).unwrap();
            let tls = TcpListener::from_std(tls).unwrap();
            tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = plain.accept().await else {
                        continue;
                    };
                    // Without this, Nagle's algorithm and delayed ACKs add
                    // tens of milliseconds to small responses.
                    stream.set_nodelay(true).unwrap();
                    tokio::spawn(serve(stream));
                }
            });
            loop {
                let Ok((stream, _)) = tls.accept().await else {
                    continue;
                };
                stream.set_nodelay(true).unwrap();
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    if let Ok(stream) = acceptor.accept(stream).await {
                        serve(stream).await;
                    }
                });
            }
        });
    });
    server
}

async fn serve<S>(stream: S)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let _ = auto::Builder::new(TokioExecutor::new())
        .serve_connection(TokioIo::new(stream), hyper::service::service_fn(handle))
        .await;
}

fn param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
}

fn number(query: &str, name: &str) -> Option<u64> {
    param(query, name)?.parse().ok()
}

/// Enough percent-decoding for the URLs and headers tests pass.
fn decode(s: &str) -> String {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap()
}

fn empty(status: u16) -> Response<BoxBody<Bytes, Infallible>> {
    Response::builder()
        .status(status)
        .body(Full::new(Bytes::new()).boxed())
        .unwrap()
}

async fn handle(
    req: Request<hyper::body::Incoming>,
) -> Result<Response<BoxBody<Bytes, Infallible>>, Infallible> {
    let query = req.uri().query().unwrap_or("").to_owned();
    if let Some(method) = param(&query, "method")
        && req.method().as_str() != method
    {
        return Ok(empty(400));
    }
    if let Some(header) = param(&query, "header") {
        let header = decode(header);
        let (name, value) = header.split_once(':').unwrap();
        if req.headers().get(name).and_then(|v| v.to_str().ok()) != Some(value) {
            return Ok(empty(400));
        }
    }
    if req.uri().path() == "/redirect" {
        let to = decode(param(&query, "to").unwrap_or("/"));
        let mut response = empty(302);
        response
            .headers_mut()
            .insert("location", to.parse().unwrap());
        return Ok(response);
    }
    let headers_ms = number(&query, "headers_ms").unwrap_or(0);
    let body_ms = number(&query, "body_ms").unwrap_or(0);
    let size = number(&query, "size").unwrap_or(16) as usize;
    let status = number(&query, "status").unwrap_or(200) as u16;
    if headers_ms > 0 {
        tokio::time::sleep(Duration::from_millis(headers_ms)).await;
    }
    let body = Bytes::from(vec![b'x'; size]);
    let body = if body_ms == 0 {
        Full::new(body).boxed()
    } else {
        // The body arrives as one frame after a delay, so the time between
        // the headers and the end of the body is body_ms.
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Frame<Bytes>, Infallible>>(1);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(body_ms)).await;
            let _ = tx.send(Ok(Frame::data(body))).await;
        });
        BodyExt::boxed(StreamBody::new(ReceiverStream(rx)))
    };
    Ok(Response::builder()
        .status(status)
        .header("content-length", size.to_string())
        .body(body)
        .unwrap())
}

struct ReceiverStream<T>(tokio::sync::mpsc::Receiver<T>);

impl<T> futures_core::Stream for ReceiverStream<T> {
    type Item = T;
    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<T>> {
        self.0.poll_recv(cx)
    }
}
