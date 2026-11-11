//! TCP connection with Happy Eyeballs (RFC 8305).
//!
//! When a name has several addresses, trying them one at a time means a
//! dead IPv6 route stalls the request until its connect times out. Instead,
//! addresses alternate between families, and each new attempt starts after
//! 250 ms or as soon as the previous one fails, whichever comes first. The
//! first connection to succeed wins; the others are dropped.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use tokio::net::TcpStream;
use tokio::task::JoinSet;

/// RFC 8305's recommended "Connection Attempt Delay".
const ATTEMPT_DELAY: Duration = Duration::from_millis(250);

pub struct Connected {
    pub stream: TcpStream,
    pub peer: SocketAddr,
    /// How many addresses were tried before one connected, including it.
    pub attempts: usize,
    pub took: Duration,
}

/// Interleaves address families, starting with the family of the first
/// address the resolver returned.
pub fn interleave(addrs: &[SocketAddr]) -> Vec<SocketAddr> {
    let Some(first) = addrs.first() else {
        return Vec::new();
    };
    let (mut same, mut other): (Vec<SocketAddr>, Vec<SocketAddr>) = addrs
        .iter()
        .copied()
        .partition(|a| a.is_ipv6() == first.is_ipv6());
    same.reverse();
    other.reverse();
    let mut out = Vec::with_capacity(addrs.len());
    while !same.is_empty() || !other.is_empty() {
        out.extend(same.pop());
        out.extend(other.pop());
    }
    out
}

pub async fn connect(addrs: &[SocketAddr]) -> Result<Connected, String> {
    let order = interleave(addrs);
    let start = Instant::now();
    let mut set = JoinSet::new();
    let mut next = 0;
    let mut errors = Vec::new();
    loop {
        if let Some(&addr) = order.get(next) {
            next += 1;
            set.spawn(async move { (addr, TcpStream::connect(addr).await) });
        }
        // Wait for an attempt to finish, or until it's time to start the
        // next one.
        loop {
            let more = next < order.len();
            let finished = if more {
                match tokio::time::timeout(ATTEMPT_DELAY, set.join_next()).await {
                    Ok(finished) => finished,
                    Err(_) => break,
                }
            } else {
                set.join_next().await
            };
            match finished {
                Some(Ok((addr, Ok(stream)))) => {
                    set.abort_all();
                    stream
                        .set_nodelay(true)
                        .map_err(|e| format!("setting TCP_NODELAY: {e}"))?;
                    return Ok(Connected {
                        stream,
                        peer: addr,
                        attempts: next,
                        took: start.elapsed(),
                    });
                }
                Some(Ok((addr, Err(e)))) => {
                    errors.push(format!("{addr}: {e}"));
                    if more {
                        break;
                    }
                }
                Some(Err(e)) => errors.push(format!("connection attempt failed: {e}")),
                None if more => break,
                None => {
                    return Err(format!("could not connect: {}", errors.join("; ")));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn interleaves_families() {
        let addrs = [
            a("[2001:db8::1]:443"),
            a("[2001:db8::2]:443"),
            a("192.0.2.1:443"),
            a("192.0.2.2:443"),
        ];
        assert_eq!(
            interleave(&addrs),
            vec![
                a("[2001:db8::1]:443"),
                a("192.0.2.1:443"),
                a("[2001:db8::2]:443"),
                a("192.0.2.2:443")
            ]
        );
        let v4_first = [a("192.0.2.1:443"), a("[2001:db8::1]:443")];
        assert_eq!(interleave(&v4_first), v4_first.to_vec());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn falls_back_past_a_refused_address() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let good = listener.local_addr().unwrap();
        // A port with nothing listening refuses at once.
        let closed = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap()
        };
        let connected = connect(&[closed, good]).await.unwrap();
        assert_eq!(connected.peer, good);
        assert_eq!(connected.attempts, 2);
        assert!(
            connected.took < ATTEMPT_DELAY,
            "a refusal should start the next attempt at once"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reports_every_failure() {
        let closed = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap()
        };
        let err = connect(&[closed]).await.err().unwrap();
        assert!(err.contains(&closed.to_string()), "{err}");
    }
}
