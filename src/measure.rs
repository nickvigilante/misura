//! Timing one request, phase by phase.
//!
//! See docs/decisions/0001-time-each-phase-yourself.md: misura resolves,
//! connects and handshakes itself, then hands the stream to hyper's
//! connection-level client, so every phase has a measured start and end.

use std::cell::OnceCell;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use http_body_util::{BodyExt, Empty};
use hyper::body::Bytes;
use hyper::client::conn::{http1, http2};
use hyper::header::{HeaderMap, HeaderName, HeaderValue};
use hyper::{Method, Request};
use hyper_util::rt::{TokioExecutor, TokioIo};
use serde::Serialize;

use crate::connect;
use crate::resolve::{self, Override};
use crate::target::{Scheme, Target};
use crate::tls;

/// Everything about a request that stays the same across runs.
pub struct Plan {
    pub method: Method,
    pub headers: HeaderMap,
    pub overrides: Vec<Override>,
    pub cacert: Option<PathBuf>,
    pub offer: tls::Offer,
    tls: OnceCell<Arc<rustls::ClientConfig>>,
}

impl Plan {
    pub fn new(
        method: Method,
        headers: HeaderMap,
        overrides: Vec<Override>,
        cacert: Option<PathBuf>,
        offer: tls::Offer,
    ) -> Self {
        Self {
            method,
            headers,
            overrides,
            cacert,
            offer,
            tls: OnceCell::new(),
        }
    }

    /// The TLS settings, built on first use. Building them loads the trust
    /// store, which takes milliseconds on Linux, so plain-HTTP requests skip
    /// it and HTTPS requests do it before their clock starts.
    fn tls(&self) -> Result<Arc<rustls::ClientConfig>, String> {
        if let Some(config) = self.tls.get() {
            return Ok(config.clone());
        }
        let config = tls::client_config(self.cacert.as_deref(), self.offer)?;
        Ok(self.tls.get_or_init(|| config).clone())
    }
}

enum Sender {
    H1(http1::SendRequest<Empty<Bytes>>),
    H2(http2::SendRequest<Empty<Bytes>>),
}

/// An open connection that later requests to the same origin can reuse.
pub struct Conn {
    origin: Target,
    sender: Sender,
    peer: SocketAddr,
    protocol: &'static str,
    tls_version: Option<String>,
    tls_cipher: Option<String>,
}

/// Time spent in each phase. Connection phases are `None` when the request
/// reused an open connection, and DNS is `None` when no lookup happened.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Phases {
    #[serde(serialize_with = "ms_opt")]
    pub dns: Option<Duration>,
    #[serde(serialize_with = "ms_opt")]
    pub tcp: Option<Duration>,
    #[serde(serialize_with = "ms_opt")]
    pub tls: Option<Duration>,
    /// From sending the request to receiving the response headers.
    #[serde(serialize_with = "ms")]
    pub wait: Duration,
    /// From the response headers to the end of the body.
    #[serde(serialize_with = "ms")]
    pub transfer: Duration,
    /// Time to first byte, counted from the start like curl's
    /// `time_starttransfer`: every phase up to the response headers.
    #[serde(serialize_with = "ms")]
    pub ttfb: Duration,
    #[serde(serialize_with = "ms")]
    pub total: Duration,
}

fn ms<S: serde::Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_f64(round_ms(*d))
}

fn ms_opt<S: serde::Serializer>(d: &Option<Duration>, s: S) -> Result<S::Ok, S::Error> {
    match d {
        Some(d) => s.serialize_f64(round_ms(*d)),
        None => s.serialize_none(),
    }
}

/// Milliseconds to the nearest microsecond.
pub fn round_ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 1_000_000.0).round() / 1000.0
}

#[derive(Debug, Clone, Serialize)]
pub struct Outcome {
    pub url: String,
    pub peer: SocketAddr,
    /// Addresses tried before one connected; 0 on a reused connection.
    pub connect_attempts: usize,
    pub reused_connection: bool,
    pub protocol: &'static str,
    pub tls_version: Option<String>,
    pub tls_cipher: Option<String>,
    pub status: u16,
    pub body_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(rename = "ms")]
    pub phases: Phases,
}

/// Makes one request to `target`, reusing `conn` when it's open to the same
/// origin. Returns the connection so the next request can reuse it.
pub async fn once(
    plan: &Plan,
    target: &Target,
    conn: Option<Conn>,
) -> Result<(Outcome, Conn), String> {
    let tls = match target.scheme {
        Scheme::Https => Some(plan.tls()?),
        Scheme::Http => None,
    };
    let start = Instant::now();
    let mut phases = Phases::default();
    let mut attempts = 0;
    let reused;
    let mut conn = match conn.filter(|c| c.origin.same_origin(target)) {
        Some(mut c) => {
            let ready = match &mut c.sender {
                Sender::H1(s) => s.ready().await.is_ok(),
                Sender::H2(s) => s.ready().await.is_ok(),
            };
            if ready {
                reused = true;
                c
            } else {
                reused = false;
                open(plan, tls, target, &mut phases, &mut attempts).await?
            }
        }
        None => {
            reused = false;
            open(plan, tls, target, &mut phases, &mut attempts).await?
        }
    };

    let request = build_request(plan, target, matches!(conn.sender, Sender::H2(_)))?;
    let sent = Instant::now();
    let response = match &mut conn.sender {
        Sender::H1(s) => s.send_request(request).await,
        Sender::H2(s) => s.send_request(request).await,
    }
    .map_err(|e| format!("request to {target} failed: {e}"))?;
    let headers_at = Instant::now();
    phases.wait = headers_at - sent;
    phases.ttfb = headers_at - start;

    let status = response.status().as_u16();
    let location = response
        .headers()
        .get(hyper::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    // Read and discard the body as it arrives, so large downloads cost no
    // memory and the transfer time is what the network takes.
    let mut body = response.into_body();
    let mut body_bytes = 0u64;
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|e| format!("reading the response from {target}: {e}"))?;
        if let Some(data) = frame.data_ref() {
            body_bytes += data.len() as u64;
        }
    }
    phases.transfer = headers_at.elapsed();
    phases.total = start.elapsed();

    let outcome = Outcome {
        url: target.to_string(),
        peer: conn.peer,
        connect_attempts: attempts,
        reused_connection: reused,
        protocol: conn.protocol,
        tls_version: conn.tls_version.clone(),
        tls_cipher: conn.tls_cipher.clone(),
        status,
        body_bytes,
        location,
        phases,
    };
    Ok((outcome, conn))
}

async fn open(
    plan: &Plan,
    tls: Option<Arc<rustls::ClientConfig>>,
    target: &Target,
    phases: &mut Phases,
    attempts: &mut usize,
) -> Result<Conn, String> {
    let (addrs, dns) = resolve::resolve(&target.host, target.port, &plan.overrides).await?;
    phases.dns = dns;
    let tcp = connect::connect(&addrs).await?;
    phases.tcp = Some(tcp.took);
    *attempts = tcp.attempts;
    let peer = tcp.peer;

    let (sender, protocol, tls_version, tls_cipher) = match target.scheme {
        Scheme::Http => {
            if plan.offer == tls::Offer::Http2 {
                return Err(
                    "--http2 needs an https:// URL; misura doesn't do HTTP/2 without TLS"
                        .to_owned(),
                );
            }
            let (s, c) = http1::handshake(TokioIo::new(tcp.stream))
                .await
                .map_err(|e| format!("HTTP/1.1 setup with {target}: {e}"))?;
            tokio::spawn(c);
            (Sender::H1(s), "HTTP/1.1", None, None)
        }
        Scheme::Https => {
            let config = tls.ok_or("internal error: no TLS settings for an https:// URL")?;
            let hs = tls::handshake(config, &target.host, tcp.stream).await?;
            phases.tls = Some(hs.took);
            let io = TokioIo::new(hs.stream);
            if hs.h2 {
                let (s, c) = http2::handshake(TokioExecutor::new(), io)
                    .await
                    .map_err(|e| format!("HTTP/2 setup with {target}: {e}"))?;
                tokio::spawn(c);
                (Sender::H2(s), "h2", Some(hs.version), Some(hs.cipher))
            } else {
                if plan.offer == tls::Offer::Http2 {
                    return Err(format!("{} doesn't support HTTP/2", target.authority()));
                }
                let (s, c) = http1::handshake(io)
                    .await
                    .map_err(|e| format!("HTTP/1.1 setup with {target}: {e}"))?;
                tokio::spawn(c);
                (Sender::H1(s), "HTTP/1.1", Some(hs.version), Some(hs.cipher))
            }
        }
    };
    Ok(Conn {
        origin: target.clone(),
        sender,
        peer,
        protocol,
        tls_version,
        tls_cipher,
    })
}

fn build_request(plan: &Plan, target: &Target, h2: bool) -> Result<Request<Empty<Bytes>>, String> {
    // HTTP/2 carries the authority in the request URI; HTTP/1.1 sends a
    // path and a Host header.
    let uri = if h2 {
        target.to_string()
    } else {
        target.path.clone()
    };
    let mut request = Request::builder()
        .method(plan.method.clone())
        .uri(uri)
        .body(Empty::new())
        .map_err(|e| format!("building the request: {e}"))?;
    let headers = request.headers_mut();
    if !h2 {
        headers.insert(
            hyper::header::HOST,
            HeaderValue::from_str(&target.authority()).map_err(|e| format!("Host header: {e}"))?,
        );
    }
    headers.insert(
        hyper::header::USER_AGENT,
        HeaderValue::from_static(concat!("misura/", env!("CARGO_PKG_VERSION"))),
    );
    headers.insert(hyper::header::ACCEPT, HeaderValue::from_static("*/*"));
    // A -H header replaces a default of the same name, and repeating -H
    // sends the header more than once.
    for name in plan.headers.keys() {
        headers.remove(name);
    }
    for (name, value) in &plan.headers {
        if h2 && name == hyper::header::HOST {
            continue;
        }
        headers.append(name.clone(), value.clone());
    }
    Ok(request)
}

/// Parses a `-H 'Name: value'` argument.
pub fn parse_header(spec: &str) -> Result<(HeaderName, HeaderValue), String> {
    let (name, value) = spec
        .split_once(':')
        .ok_or_else(|| format!("-H {spec:?}: expected 'Name: value'"))?;
    let name =
        HeaderName::from_bytes(name.trim().as_bytes()).map_err(|e| format!("-H {spec:?}: {e}"))?;
    let value = HeaderValue::from_str(value.trim()).map_err(|e| format!("-H {spec:?}: {e}"))?;
    Ok((name, value))
}

/// How to treat redirects.
#[derive(Debug, Clone, Copy)]
pub struct Follow {
    pub enabled: bool,
    pub cross_host: bool,
    pub max: u32,
}

/// One run: a request and any redirects followed from it.
#[derive(Debug, Clone, Serialize)]
pub struct Run {
    pub hops: Vec<Outcome>,
    /// Why redirects stopped being followed early, if they did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stopped: Option<String>,
    /// Every hop's phases added up. TTFB is when the last hop's response
    /// headers arrived, which is what `curl -L` reports.
    #[serde(rename = "ms")]
    pub phases: Phases,
}

impl Run {
    fn new(hops: Vec<Outcome>, stopped: Option<String>) -> Self {
        fn sum(values: impl Iterator<Item = Option<Duration>>) -> Option<Duration> {
            values.fold(None, |acc, v| match (acc, v) {
                (None, v) => v,
                (Some(a), None) => Some(a),
                (Some(a), Some(b)) => Some(a + b),
            })
        }
        let total: Duration = hops.iter().map(|h| h.phases.total).sum();
        let last_transfer = hops.last().map_or(Duration::ZERO, |h| h.phases.transfer);
        let phases = Phases {
            dns: sum(hops.iter().map(|h| h.phases.dns)),
            tcp: sum(hops.iter().map(|h| h.phases.tcp)),
            tls: sum(hops.iter().map(|h| h.phases.tls)),
            wait: hops.iter().map(|h| h.phases.wait).sum(),
            transfer: hops.iter().map(|h| h.phases.transfer).sum(),
            ttfb: total - last_transfer,
            total,
        };
        Self {
            hops,
            stopped,
            phases,
        }
    }

    /// The final response.
    pub fn last(&self) -> &Outcome {
        self.hops.last().expect("a run has at least one hop")
    }
}

/// One request, or a chain of them when following redirects. Returns the
/// run, and the connection for the next run to reuse.
pub async fn chain(
    plan: &Plan,
    start: &Target,
    follow: Follow,
    mut conn: Option<Conn>,
) -> Result<(Run, Option<Conn>), String> {
    let mut hops = Vec::new();
    let mut target = start.clone();
    let mut stopped = None;
    loop {
        let (outcome, c) = once(plan, &target, conn.take()).await?;
        let redirect = (300..400).contains(&outcome.status) && follow.enabled;
        let location = outcome.location.clone();
        hops.push(outcome);
        conn = Some(c);
        let Some(location) = location.filter(|_| redirect) else {
            break;
        };
        if hops.len() as u32 > follow.max {
            return Err(format!(
                "stopped after {} redirects (--max-redirects)",
                follow.max
            ));
        }
        let next = target.redirect(&location)?;
        if !follow.cross_host && !next.host.eq_ignore_ascii_case(&target.host) {
            stopped = Some(format!(
                "not following the redirect to {next}, another host; add --cross-host to follow it"
            ));
            break;
        }
        target = next;
    }
    Ok((Run::new(hops, stopped), conn))
}
