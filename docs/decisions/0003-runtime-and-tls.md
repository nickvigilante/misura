# 0003: A current-thread tokio runtime, and rustls with ring

## Decision

- **Runtime:**
  tokio with only the `rt`, `net`, `time`, `macros` and `io-util` features, on the current-thread scheduler.
- **TLS:**
  `rustls` 0.23 through `tokio-rustls`, with the `ring` crypto provider, ALPN offering `h2` and `http/1.1`, and `rustls-platform-verifier` for trust.
  `--cacert FILE` adds trust anchors through `Verifier::new_with_extra_roots`, which works on Linux and macOS.

## Why async at all

hyper's HTTP/2 client only exists as an async API.
HTTP/1.1 alone could be done with blocking IO, but two code paths would cost more than one small runtime.
Since misura makes one request at a time, the multi-threaded scheduler would only add startup cost.

## Why ring rather than aws-lc-rs

rustls defaults to `aws-lc-rs`, which needs CMake and a C toolchain to build and complicates static and cross builds.
`ring` builds with only a C compiler, is what the rest of this crate's dependencies already pull in, and is fast enough for one handshake.
`cargo tree` confirms `aws-lc-rs` is not in the build.

## Why the platform verifier

misura should report trust the way the user's operating system would: Security.framework on macOS, the system bundle on Linux.
Shipping `webpki-roots` would make a certificate the OS trusts look untrusted, or the other way round.

## Startup cost

With `hyperfine -N`, 200–300 runs, against the local test server:

| Request | misura (spike, release, not stripped) | curl |
| --- | --- | --- |
| `http://localhost:8080/` | 5.2 ms ± 0.6 | 9.0 ms ± 1.0 |
| `https://localhost:8443/` | 7.0 ms ± 0.8 | 12.9 ms ± 1.8 |

The release binary is 4.8 MB before `strip`, LTO and `panic = "abort"`.
