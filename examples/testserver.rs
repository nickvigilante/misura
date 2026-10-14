//! A local server for checking misura's timings against curl.
//!
//! `cargo run --example testserver -- DIR` writes a self-signed certificate
//! for `localhost` to `DIR/cert.pem`, then serves HTTPS (HTTP/1.1 and h2 via
//! ALPN) on 127.0.0.1:8443 and plain HTTP/1.1 on 127.0.0.1:8080.
//!
//! Query parameters shape the response, so each phase's expected duration is
//! known in advance:
//! - `headers_ms=N`: wait N ms before sending the response headers.
//! - `body_ms=N`: after the headers, wait N ms before sending the body.
//! - `size=N`: body length in bytes (default 16).
//! - `/redirect?to=PATH`: answer 302 with `Location: PATH`.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use http_body_util::{BodyExt, Full, StreamBody, combinators::BoxBody};
use hyper::body::{Bytes, Frame};
use hyper::{Request, Response};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use tokio::net::TcpListener;

fn param(query: &str, name: &str) -> Option<u64> {
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix(name)?.strip_prefix('=')?.parse().ok())
}

async fn handle(
    req: Request<hyper::body::Incoming>,
) -> Result<Response<BoxBody<Bytes, Infallible>>, Infallible> {
    let query = req.uri().query().unwrap_or("").to_owned();
    if req.uri().path() == "/redirect" {
        let to = query
            .split('&')
            .find_map(|p| p.strip_prefix("to="))
            .unwrap_or("/")
            .to_owned();
        return Ok(Response::builder()
            .status(302)
            .header("location", to)
            .body(Full::new(Bytes::new()).boxed())
            .unwrap());
    }
    let headers_ms = param(&query, "headers_ms").unwrap_or(0);
    let body_ms = param(&query, "body_ms").unwrap_or(0);
    let size = param(&query, "size").unwrap_or(16) as usize;
    tokio::time::sleep(Duration::from_millis(headers_ms)).await;
    // The body arrives as one frame after a delay, so the time between the
    // headers and the end of the body is body_ms.
    let stream = futures_lite_stream(body_ms, size);
    Ok(Response::builder()
        .header("content-length", size.to_string())
        .body(BodyExt::boxed(stream))
        .unwrap())
}

fn futures_lite_stream(
    delay_ms: u64,
    size: usize,
) -> impl hyper::body::Body<Data = Bytes, Error = Infallible> + Send + 'static {
    // A one-shot channel-backed body: sleep, then send everything.
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Frame<Bytes>, Infallible>>(1);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        let _ = tx
            .send(Ok(Frame::data(Bytes::from(vec![b'x'; size]))))
            .await;
    });
    StreamBody::new(ReceiverStream(rx))
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

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let dir = std::env::args().nth(1).expect("usage: testserver DIR");
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
    std::fs::write(format!("{dir}/cert.pem"), cert.cert.pem()).unwrap();

    let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![cert.cert.der().clone()],
        rustls::pki_types::PrivateKeyDer::Pkcs8(cert.signing_key.serialize_der().into()),
    )
    .unwrap();
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));

    let tls = TcpListener::bind("127.0.0.1:8443").await.unwrap();
    let plain = TcpListener::bind("127.0.0.1:8080").await.unwrap();
    eprintln!("listening on https://localhost:8443 and http://localhost:8080");

    tokio::spawn(async move {
        loop {
            let (stream, _) = plain.accept().await.unwrap();
            stream.set_nodelay(true).unwrap();
            tokio::spawn(async move {
                let _ = auto::Builder::new(TokioExecutor::new())
                    .serve_connection(TokioIo::new(stream), hyper::service::service_fn(handle))
                    .await;
            });
        }
    });
    loop {
        let (stream, _) = tls.accept().await.unwrap();
        stream.set_nodelay(true).unwrap();
        let acceptor = acceptor.clone();
        tokio::spawn(async move {
            let Ok(stream) = acceptor.accept(stream).await else {
                return;
            };
            let _ = auto::Builder::new(TokioExecutor::new())
                .serve_connection(TokioIo::new(stream), hyper::service::service_fn(handle))
                .await;
        });
    }
}
