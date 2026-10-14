# misura

*Misura* is Italian for "measure".

`misura` shows where an HTTP request spends its time: DNS, TCP, TLS, waiting for the server, and receiving the response.
It replaces `curl -w` with a format file, and it's built to be at least as fast and as accurate as curl.

```console
$ misura https://localhost:8443/
GET https://localhost:8443/ → 127.0.0.1:8443
h2 · TLSv1.3 · TLS13_AES_256_GCM_SHA384 · 200 OK · 46.9 KiB

  DNS lookup          0.15 ms  ▏
  TCP connect         0.14 ms  ▏
  TLS handshake       0.96 ms  █
  Server wait        46.59 ms   ██████████████████████████████
  Transfer           13.52 ms                                 █████████
  Total              61.79 ms
  TTFB               48.27 ms
```

The samples here come from the bundled test server (`cargo run --example testserver`), which answers after a delay you choose.
Each bar starts where the previous phase ended, so the waterfall shows both how long each phase took and when it happened.

It's the same information as this curl command, without the format file or the arithmetic, since curl's times add up from the start:

```sh
curl -so /dev/null -w 'dns %{time_namelookup}\nconnect %{time_connect}\ntls %{time_appconnect}\nttfb %{time_starttransfer}\ntotal %{time_total}\n' URL
```

## Install

```sh
cargo install --locked --git https://github.com/nickvigilante/misura
```

misura runs on Linux and macOS.

## Usage

```sh
misura URL                  # one request, as a waterfall
misura -n 20 URL            # 20 requests, with percentiles for each phase
misura -n 20 --reuse URL    # 20 requests on one connection
misura --compare URL        # HTTP/1.1 and HTTP/2 side by side
misura -L URL               # follow redirects, timing each hop
misura --json URL           # machine-readable, in milliseconds
```

Requests are `GET` with no body unless you pass `-X`.
Add headers with `-H 'Name: value'`, as many as you need.

### Repeats and percentiles

`-n N` makes N requests and reports the minimum, p50, p90, p99 and maximum of each phase.
Percentiles are nearest-rank, so each one is a time misura measured, never an average of two.

```console
$ misura -n 20 https://localhost:8443/
GET https://localhost:8443/ → 127.0.0.1:8443
h2 · TLSv1.3 · 200 OK ×20 · 20 requests, a new connection for each

  ms                   min      p50      p90      p99      max
  DNS lookup          0.05     0.05     0.07     0.14     0.14
  TCP connect         0.08     0.09     0.10     0.15     0.15
  TLS handshake       0.49     0.61     0.77     1.02     1.02
  Server wait        45.68    46.81    47.04    47.18    47.18
  Transfer            0.00     0.00     0.01     0.01     0.01
  TTFB               46.65    47.75    48.13    48.27    48.27
  Total              46.65    47.76    48.13    48.28    48.28
```

Each request opens a new connection, unless you add `--reuse`.
With `--reuse`, DNS, TCP and TLS happen once, and the gap between the first request and the rest is what a new connection costs.

### Comparing HTTP/1.1 and HTTP/2

By default misura uses whichever protocol the server picks through ALPN.
`--http1` and `--http2` force one, and `--compare` runs both, taking turns so a change in the network affects both alike:

```console
$ misura --compare -n 10 https://localhost:8443/
GET https://localhost:8443/
  HTTP/1.1 → 127.0.0.1:8443 · TLSv1.3 · 200 OK ×10
  h2       → 127.0.0.1:8443 · TLSv1.3 · 200 OK ×10

  p50 of 10, ms     HTTP/1.1         h2
  DNS lookup            0.06       0.06
  TCP connect           0.09       0.08
  TLS handshake         0.55       0.59
  Server wait          46.65      46.65
  Transfer              0.03       0.00
  TTFB                 47.47      47.78
  Total                47.51      47.79
```

HTTP/2 needs an `https://` URL.
HTTP/3 isn't supported yet; [`docs/decisions/0004-http3-later.md`](docs/decisions/0004-http3-later.md) says what it would take.

### Redirects

misura shows a redirect without following it.
With `-L` it follows up to 10 redirects (change that with `--max-redirects`), and times each hop.
Hops to the same origin reuse the connection, as a browser would.
A redirect to a different host is only followed with `--cross-host`; without it, misura stops and says where the redirect pointed.

### Name resolution

misura asks the operating system's resolver, so it sees the same addresses as other programs on the machine.
`--resolve HOST:PORT:ADDR` skips the lookup, like curl's `--resolve`, and `ADDR` can be a comma-separated list.
When a name has several addresses, misura tries them with [Happy Eyeballs](https://www.rfc-editor.org/rfc/rfc8305): IPv6 and IPv4 take turns, and the next address starts after 250 ms or as soon as one fails.
A dead route costs a quarter of a second rather than a connect timeout.

### TLS

Certificates are checked with the operating system's trust store and rules, through [rustls-platform-verifier](https://github.com/rustls/rustls-platform-verifier).
`--cacert PATH` trusts the certificates in a PEM file as well.

## Monitoring and exit codes

| Exit code | Meaning                                                                                        |
| --------- | ---------------------------------------------------------------------------------------------- |
| 0         | Success.                                                                                       |
| 1         | The response was 4xx or 5xx, or a request went over `--max-ttfb` or `--max-total`.             |
| 2         | misura couldn't make the request: a usage error, a DNS, connection or TLS error, or a timeout. |

So misura works as a check in cron, a Kubernetes CronJob or Uptime Kuma, and its output says which phase got slow:

```sh
misura --max-ttfb 500ms --max-total 2s --timeout 10s https://example.com/health
```

Durations take `ms`, `s` or `m`, and a bare number means seconds.
`--timeout` (30 s by default) covers one request with its redirects.

## JSON

`--json` prints one object, with every duration in milliseconds to the nearest microsecond.
Its `version` changes only if a field changes meaning or goes away.

```json
{
  "version": 1,
  "method": "GET",
  "url": "https://localhost:8443/",
  "reuse": false,
  "results": [
    {
      "protocol": "auto",
      "runs": [
        {
          "hops": [
            {
              "url": "https://localhost:8443/",
              "peer": "127.0.0.1:8443",
              "connect_attempts": 1,
              "reused_connection": false,
              "protocol": "h2",
              "tls_version": "TLSv1.3",
              "tls_cipher": "TLS13_AES_256_GCM_SHA384",
              "status": 200,
              "body_bytes": 16,
              "ms": { "dns": 0.061, "tcp": 0.089, "tls": 0.604, "wait": 46.812, "transfer": 0.004, "ttfb": 47.751, "total": 47.755 }
            }
          ],
          "ms": { "dns": 0.061, "tcp": 0.089, "tls": 0.604, "wait": 46.812, "transfer": 0.004, "ttfb": 47.751, "total": 47.755 }
        }
      ],
      "summary": {
        "total": { "min": 47.755, "p50": 47.755, "p90": 47.755, "p99": 47.755, "max": 47.755 }
      }
    }
  ],
  "failed": false,
  "reasons": []
}
```

- `results` has one entry, or two with `--compare`, whose `protocol` is `auto`, `http1` or `http2`.
- Each run lists its `hops`, more than one when `-L` follows redirects, and its `ms` adds them up.
  A run's `ttfb` is when the last hop's headers arrived, as `curl -L` reports it.
- `summary` has `min`, `p50`, `p90`, `p99` and `max` for `dns`, `tcp`, `tls`, `wait`, `transfer`, `ttfb` and `total` (shortened above).
- A phase that didn't happen is `null`: DNS for an IP address or `--resolve`, TLS for `http://`, and DNS, TCP and TLS on a reused connection.
- `stopped` appears on a run when a redirect wasn't followed, and `location` on a hop that redirected.
- `failed` and `reasons` say why misura exits with 1.

When misura can't make the request, it prints `{"version": 1, "error": "..."}` and exits with 2.

## Accuracy and speed

misura does each phase itself and reads the clock at its start and end, rather than asking an HTTP library afterwards.
The reasons for each choice are in [`docs/decisions/`](docs/decisions/), and [`0005-validation-against-curl.md`](docs/decisions/0005-validation-against-curl.md) checks the numbers against curl's.

The tests check that each phase lands in the right bucket: a server that waits 300 ms before its headers and 200 ms before its body has to show up as 300 ms of server wait and 200 ms of transfer.

misura starts faster than curl, so a check or a loop of requests spends less time on the tool and more on the request.
These are means from `scripts/bench.sh`, which runs [hyperfine](https://github.com/sharkdp/hyperfine) against the local test server on Linux, against curl 8.5 with OpenSSL 3.0:

| Request                   | misura | curl   |
| ------------------------- | ------ | ------ |
| HTTPS, h2, new connection | 4.4 ms | 9.6 ms |
| Plain HTTP/1.1            | 2.1 ms | 6.2 ms |

CI runs the same comparison on every pull request and shows it in the job summary.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
