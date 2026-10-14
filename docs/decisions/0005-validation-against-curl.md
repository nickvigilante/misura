# 0005: Validation against curl

## Setup

This was done in a cloud container whose only outbound route is a TLS-intercepting proxy, so real URLs couldn't be measured fairly.
Instead, `examples/testserver.rs` serves HTTPS (HTTP/1.1 and h2 through ALPN) and plain HTTP on 127.0.0.1.
Query parameters set exact delays: `headers_ms` before the response headers, and `body_ms` between the headers and the body.
That gives a known right answer for the waiting and receiving phases.

Repeat this against a few real URLs on a normal network before trusting the DNS, TCP and TLS numbers.

## Results

Request: `https://localhost:8443/?headers_ms=200&body_ms=100&size=1000`, three runs each.
The misura columns are per phase; curl's are cumulative from the start, so its per-phase figures are differences between columns.

| Tool | Protocol | DNS | TCP | TLS | Waiting | Receiving | Total |
| --- | --- | --- | --- | --- | --- | --- | --- |
| misura | HTTP/1.1 | 0.40–0.50 ms | 0.29–0.39 ms | 2.3–2.5 ms | 201.1–202.2 ms | 100.5–101.5 ms | 305.9–306.5 ms |
| misura | h2 | 0.37–0.42 ms | 0.22–0.45 ms | 1.7–3.0 ms | 200.9–201.9 ms | 101.5–101.6 ms | 304.9–307.2 ms |
| curl | HTTP/1.1 | ~0.03 ms | ~0.2 ms | 3–15 ms (to TLS done) | | | 305.9–318.4 ms |
| curl | h2 | ~0.03 ms | ~0.3 ms | 3–5 ms (to TLS done) | | | 305.9–310.4 ms |

Plain HTTP with `headers_ms=50&body_ms=30`: misura waiting 51.9 ms, receiving 31.2 ms, total 84.3 ms; curl first byte 51.8 ms, total 83.3 ms.

## What this shows

- Waiting and receiving land within about 2 ms of the delays the server was told to add, on both protocols.
- Totals agree with curl's to within about 1 ms.
- DNS was 0.4 ms against curl's 0.03 ms for `localhost`.
  That was tokio's thread hand-off, not DNS; 0002 fixes it by timing inside the lookup thread.
- The first benchmark showed both tools taking about 48 ms for an HTTPS request with no added delay.
  That was the test server writing headers and body separately without `TCP_NODELAY` (Nagle's algorithm against delayed ACKs), not either client.
  Setting it on the server brought the request to 7 ms.
