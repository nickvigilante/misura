# 0004: HTTP/3 comes later, through quinn and h3

## Decision

Don't build HTTP/3 in v0.1.
Keep the phase model general enough that it fits later.

## What it would take

- **Crates:**
  `quinn` (QUIC), `h3` and `h3-quinn`.
  `h3` is still at version 0.0.x, so expect API changes.
- **Different phases:**
  QUIC replaces TCP and TLS with one handshake over UDP.
  The waterfall would show DNS, then a combined "QUIC handshake" bar, then waiting and receiving.
  0-RTT resumption would make the handshake bar disappear on repeat requests, which `--reuse` and `-n` should show.
- **Discovery:**
  servers advertise HTTP/3 in the `Alt-Svc` response header, so `--compare` would first make an HTTP/1.1 or h2 request, read `Alt-Svc`, then try h3.
- **Crypto:**
  quinn uses rustls, so the same `ring` provider and platform verifier apply.
- **Networks:**
  UDP is often blocked where TCP isn't, so a failed h3 attempt has to be reported as "not reachable over QUIC", not as the server being down.
