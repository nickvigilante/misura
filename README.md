# misura

*Misura* is Italian for "measure".

`misura` shows where an HTTP request spends its time: DNS, TCP, TLS, waiting for the server, and receiving the response.
It replaces `curl -w` with a format file, and is built to be at least as fast and as accurate as curl.

## Status

Not usable yet: the design is settled and the CLI is being built.
The research behind it is in [`docs/decisions/`](docs/decisions/).

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
