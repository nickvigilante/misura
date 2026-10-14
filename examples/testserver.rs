//! A local server for checking misura's timings against curl.
//!
//! `cargo run --example testserver -- DIR` writes a self-signed certificate
//! for `localhost` to `DIR/cert.pem`, then serves HTTPS (HTTP/1.1 and h2 via
//! ALPN) on 127.0.0.1:8443 and plain HTTP/1.1 on 127.0.0.1:8080. See
//! tests/common/server.rs for the query parameters that shape responses.

#[path = "../tests/common/server.rs"]
mod server;

fn main() {
    let dir = std::env::args().nth(1).expect("usage: testserver DIR");
    let s = server::start(std::path::Path::new(&dir), 8080, 8443);
    eprintln!("listening on {} and {}", s.https_url("/"), s.http_url("/"));
    loop {
        std::thread::park();
    }
}
