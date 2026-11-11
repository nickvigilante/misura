# 0001: Time each phase by performing it, not through an HTTP client

## Decision

misura does each phase itself, in order, and times it with `std::time::Instant`:

1. resolve the host;
2. open the TCP connection;
3. run the TLS handshake with `tokio-rustls`;
4. hand the connected stream to hyper's low-level client (`hyper::client::conn::http1` or `http2`) and send the request.

Each phase's start and end are therefore measured, not estimated.

## Why

High-level clients such as `reqwest` and `ureq` resolve, connect, handshake and send in one call, and report only the total.
The connection-level API in hyper takes any stream we give it, so we can do the first three phases ourselves and still use a correct HTTP/1.1 and HTTP/2 implementation for the rest.

## What "waiting" and "receiving" mean

- **Waiting (time to first byte):**
  from handing the request to hyper until hyper returns the response headers.
- **Receiving:**
  from the headers until the body has been read to the end.

curl's `time_starttransfer` is the first byte of the response, not the parsed headers.
For ordinary header sizes the two differ by much less than a millisecond, and the comparison in 0005 bears that out.
We considered timestamping the first byte read from the socket instead, but on HTTP/2 the first bytes read are the server's SETTINGS frame, not the response, so the header-based definition is the one that means the same thing on both protocols.

## TCP_NODELAY

The client sets `TCP_NODELAY` on every connection.
Without it, a request written in more than one piece can stall about 40 ms on Nagle's algorithm meeting the server's delayed ACK.
The test server hit exactly this before it set the option itself (see 0005).
