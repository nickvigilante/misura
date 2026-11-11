//! Name resolution through the system resolver.
//!
//! See docs/decisions/0002-dns.md: the lookup runs on a blocking thread so a
//! timeout can apply, and its duration is measured inside that thread so
//! the hand-off to it isn't counted as DNS.

use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::time::{Duration, Instant};

/// A `--resolve HOST:PORT:ADDR[,ADDR...]` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Override {
    pub host: String,
    pub port: u16,
    pub addrs: Vec<IpAddr>,
}

impl Override {
    pub fn parse(spec: &str) -> Result<Self, String> {
        let bad = || format!("--resolve {spec:?}: expected HOST:PORT:ADDR[,ADDR...]");
        let (host, rest) = spec.split_once(':').ok_or_else(bad)?;
        let (port, addrs) = rest.split_once(':').ok_or_else(bad)?;
        let port = port.parse().map_err(|_| bad())?;
        let addrs = addrs
            .split(',')
            .map(|a| a.trim_start_matches('[').trim_end_matches(']').parse())
            .collect::<Result<Vec<IpAddr>, _>>()
            .map_err(|_| bad())?;
        if host.is_empty() || addrs.is_empty() {
            return Err(bad());
        }
        Ok(Self {
            host: host.to_owned(),
            port,
            addrs,
        })
    }
}

/// The addresses for `host:port`, and how long the lookup took. The
/// duration is `None` when no lookup happened: an IP literal or a
/// `--resolve` override.
pub async fn resolve(
    host: &str,
    port: u16,
    overrides: &[Override],
) -> Result<(Vec<SocketAddr>, Option<Duration>), String> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok((vec![SocketAddr::new(ip, port)], None));
    }
    if let Some(o) = overrides
        .iter()
        .find(|o| o.port == port && o.host.eq_ignore_ascii_case(host))
    {
        let addrs = o
            .addrs
            .iter()
            .map(|ip| SocketAddr::new(*ip, port))
            .collect();
        return Ok((addrs, None));
    }
    let name = host.to_owned();
    let (result, took) = tokio::task::spawn_blocking(move || {
        let start = Instant::now();
        let result = (name.as_str(), port)
            .to_socket_addrs()
            .map(|addrs| addrs.collect::<Vec<_>>());
        (result, start.elapsed())
    })
    .await
    .map_err(|e| format!("DNS lookup for {host} failed: {e}"))?;
    let addrs = result.map_err(|e| format!("DNS lookup for {host} failed: {e}"))?;
    if addrs.is_empty() {
        return Err(format!("DNS lookup for {host} returned no addresses"));
    }
    Ok((addrs, Some(took)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_overrides() {
        let o = Override::parse("example.com:443:192.0.2.1,[2001:db8::1]").unwrap();
        assert_eq!(o.host, "example.com");
        assert_eq!(o.port, 443);
        assert_eq!(o.addrs.len(), 2);
        assert!(Override::parse("example.com:443").is_err());
        assert!(Override::parse("example.com:x:192.0.2.1").is_err());
        assert!(Override::parse("example.com:443:not-an-ip").is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn literals_and_overrides_skip_dns() {
        let (addrs, took) = resolve("127.0.0.1", 80, &[]).await.unwrap();
        assert_eq!(addrs, vec!["127.0.0.1:80".parse().unwrap()]);
        assert_eq!(took, None);
        let o = Override::parse("example.com:443:192.0.2.7").unwrap();
        let (addrs, took) = resolve("EXAMPLE.com", 443, &[o]).await.unwrap();
        assert_eq!(addrs, vec!["192.0.2.7:443".parse().unwrap()]);
        assert_eq!(took, None);
    }
}
