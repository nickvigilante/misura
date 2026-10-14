//! End-to-end tests against an in-process server on 127.0.0.1. None of
//! them touch the network.
//!
//! Timing assertions have a firm lower bound, since the server sleeps for a
//! known time, and a generous upper bound, since CI machines are noisy.

mod common;

use common::{hops, json, misura, ms, server};
use predicates::str::contains;

/// Extra time a phase may take on a slow CI machine.
const SLACK: f64 = 400.0;

#[test]
fn phases_land_in_the_right_buckets() {
    let url = server().https_url("/?headers_ms=300&body_ms=200&size=100000");
    let (code, report) = json(&[&url]);
    assert_eq!(code, 0);
    let hop = &hops(&report)[0];
    let p = &hop["ms"];
    assert_eq!(hop["status"], 200);
    assert_eq!(hop["body_bytes"], 100_000);
    assert_eq!(hop["protocol"], "h2");
    assert_eq!(hop["tls_version"], "TLSv1.3");
    for phase in ["dns", "tcp", "tls"] {
        assert!(ms(&p[phase]) < SLACK, "{phase}: {p}");
    }
    assert!(
        (299.0..300.0 + SLACK).contains(&ms(&p["wait"])),
        "wait: {p}"
    );
    assert!(
        (199.0..200.0 + SLACK).contains(&ms(&p["transfer"])),
        "transfer: {p}"
    );
    // The phases account for the whole request, and TTFB is everything up
    // to the headers.
    let sum: f64 = ["dns", "tcp", "tls", "wait", "transfer"]
        .iter()
        .map(|k| ms(&p[k]))
        .sum();
    let total = ms(&p["total"]);
    assert!(
        sum <= total + 0.01 && total - sum < 25.0,
        "sum {sum} vs total {total}"
    );
    assert!(
        (ms(&p["ttfb"]) + ms(&p["transfer"]) - total).abs() < 0.01,
        "{p}"
    );
}

#[test]
fn plain_http_has_no_tls_phase() {
    let (code, report) = json(&[&server().http_url("/?headers_ms=50")]);
    assert_eq!(code, 0);
    let hop = &hops(&report)[0];
    assert_eq!(hop["protocol"], "HTTP/1.1");
    assert!(hop["ms"]["tls"].is_null());
    assert!(hop["tls_version"].is_null());
    assert!(ms(&hop["ms"]["wait"]) >= 49.0);
}

#[test]
fn ip_addresses_and_resolve_skip_dns() {
    let url = format!("http://{}/", server().http);
    let (_, report) = json(&[&url]);
    assert!(hops(&report)[0]["ms"]["dns"].is_null());
    let pin = format!("localhost:{}:127.0.0.1", server().http.port());
    let (_, report) = json(&["--resolve", &pin, &server().http_url("/")]);
    assert!(hops(&report)[0]["ms"]["dns"].is_null());
}

#[test]
fn protocol_can_be_forced() {
    let url = server().https_url("/");
    let (_, report) = json(&["--http1", &url]);
    assert_eq!(hops(&report)[0]["protocol"], "HTTP/1.1");
    assert_eq!(report["results"][0]["protocol"], "http1");
    let (_, report) = json(&["--http2", &url]);
    assert_eq!(hops(&report)[0]["protocol"], "h2");
    misura()
        .args(["--http2", &server().http_url("/")])
        .assert()
        .code(2)
        .stderr(contains("needs an https:// URL"));
}

#[test]
fn compare_runs_both_protocols_in_turn() {
    let (code, report) = json(&["--compare", "-n", "2", &server().https_url("/")]);
    assert_eq!(code, 0);
    let results = report["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    for (result, protocol) in results.iter().zip(["HTTP/1.1", "h2"]) {
        let runs = result["runs"].as_array().unwrap();
        assert_eq!(runs.len(), 2);
        assert!(runs.iter().all(|r| r["hops"][0]["protocol"] == protocol));
    }
}

#[test]
fn redirects_are_reported_not_followed_by_default() {
    let (code, report) = json(&[&server().http_url("/redirect?to=/next")]);
    assert_eq!(code, 0);
    let hops = hops(&report);
    assert_eq!(hops.len(), 1);
    assert_eq!(hops[0]["status"], 302);
    assert_eq!(hops[0]["location"], "/next");
}

#[test]
fn follows_redirects_on_the_same_connection() {
    let url = server().https_url("/redirect?to=/?headers_ms=100");
    let (code, report) = json(&["-L", &url]);
    assert_eq!(code, 0);
    let run = &report["results"][0]["runs"][0];
    let hops = run["hops"].as_array().unwrap();
    assert_eq!(hops.len(), 2);
    assert_eq!(hops[1]["status"], 200);
    assert_eq!(hops[1]["reused_connection"], true);
    assert!(hops[1]["ms"]["tcp"].is_null());
    // The run's totals add the hops up, and TTFB counts to the last hop's
    // headers, as `curl -L` reports it.
    let total = ms(&hops[0]["ms"]["total"]) + ms(&hops[1]["ms"]["total"]);
    assert!((ms(&run["ms"]["total"]) - total).abs() < 0.01);
    assert!((ms(&run["ms"]["ttfb"]) - (total - ms(&hops[1]["ms"]["transfer"]))).abs() < 0.01);
}

#[test]
fn another_host_needs_cross_host() {
    let s = server();
    let elsewhere = format!("http%3A%2F%2F127.0.0.1%3A{}%2F", s.http.port());
    let url = s.http_url(&format!("/redirect?to={elsewhere}"));
    let (code, report) = json(&["-L", &url]);
    assert_eq!(code, 0);
    let run = &report["results"][0]["runs"][0];
    assert_eq!(run["hops"].as_array().unwrap().len(), 1);
    assert!(run["stopped"].as_str().unwrap().contains("--cross-host"));

    let (code, report) = json(&["-L", "--cross-host", &url]);
    assert_eq!(code, 0);
    let hops = hops(&report);
    assert_eq!(hops.len(), 2);
    assert_eq!(
        hops[1]["url"],
        format!("http://127.0.0.1:{}/", s.http.port())
    );
    assert_eq!(hops[1]["reused_connection"], false);
}

#[test]
fn redirect_limit_is_an_error() {
    misura()
        .args([
            "-L",
            "--max-redirects",
            "0",
            &server().http_url("/redirect?to=/"),
        ])
        .assert()
        .code(2)
        .stderr(contains("--max-redirects"));
}

#[test]
fn error_statuses_exit_with_1() {
    let (code, report) = json(&[&server().http_url("/?status=503")]);
    assert_eq!(code, 1);
    assert_eq!(report["failed"], true);
    assert!(report["reasons"][0].as_str().unwrap().contains("503"));
    let (code, _) = json(&[&server().http_url("/?status=404")]);
    assert_eq!(code, 1);
}

#[test]
fn thresholds_exit_with_1() {
    let url = server().http_url("/?headers_ms=150");
    let (code, report) = json(&["--max-ttfb", "50ms", &url]);
    assert_eq!(code, 1);
    assert!(
        report["reasons"][0]
            .as_str()
            .unwrap()
            .contains("--max-ttfb 50 ms")
    );
    let (code, _) = json(&["--max-total", "0.05", &url]);
    assert_eq!(code, 1);
    let (code, report) = json(&["--max-ttfb", "10s", "--max-total", "10s", &url]);
    assert_eq!(code, 0);
    assert_eq!(report["failed"], false);
}

#[test]
fn connection_errors_exit_with_2() {
    let closed = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let url = format!("http://{closed}/");
    misura()
        .arg(&url)
        .assert()
        .code(2)
        .stderr(contains("could not connect"));
    let (code, report) = json(&[&url]);
    assert_eq!(code, 2);
    assert!(
        report["error"]
            .as_str()
            .unwrap()
            .contains("could not connect")
    );
}

#[test]
fn timeout_exits_with_2() {
    misura()
        .args([
            "--timeout",
            "200ms",
            &server().http_url("/?headers_ms=5000"),
        ])
        .assert()
        .code(2)
        .stderr(contains("--timeout 200 ms"));
}

#[test]
fn usage_errors_exit_with_2() {
    misura().assert().code(2);
    misura()
        .args(["--http1", "--http2", "http://localhost/"])
        .assert()
        .code(2);
    misura()
        .args(["-n", "0", "http://localhost/"])
        .assert()
        .code(2);
    misura().args(["ftp://localhost/"]).assert().code(2);
    misura()
        .args(["-H", "no colon", "http://localhost/"])
        .assert()
        .code(2);
}

#[test]
fn sends_method_and_headers() {
    let url = server().http_url("/?method=POST&header=x-test%3Ayes");
    let (code, _) = json(&["-X", "POST", "-H", "X-Test: yes", &url]);
    assert_eq!(code, 0);
    let (code, _) = json(&["-X", "POST", &url]);
    assert_eq!(code, 1, "the server answers 400 without the header");
    let (code, _) = json(&["-H", "X-Test: yes", &url]);
    assert_eq!(code, 1, "the server answers 400 to GET");
}

#[test]
fn repeats_open_a_new_connection_each_time() {
    let (code, report) = json(&["-n", "3", &server().https_url("/")]);
    assert_eq!(code, 0);
    let runs = report["results"][0]["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 3);
    assert!(
        runs.iter()
            .all(|r| r["hops"][0]["reused_connection"] == false)
    );
    assert!(runs.iter().all(|r| r["ms"]["tls"].is_number()));
    let s = &report["results"][0]["summary"]["total"];
    assert!(ms(&s["min"]) <= ms(&s["p50"]) && ms(&s["p50"]) <= ms(&s["max"]));
}

#[test]
fn reuse_keeps_one_connection() {
    let (code, report) = json(&["-n", "4", "--reuse", &server().https_url("/")]);
    assert_eq!(code, 0);
    let runs = report["results"][0]["runs"].as_array().unwrap();
    assert_eq!(runs[0]["hops"][0]["reused_connection"], false);
    assert!(runs[0]["ms"]["tls"].is_number());
    for run in &runs[1..] {
        assert_eq!(run["hops"][0]["reused_connection"], true);
        assert!(
            run["ms"]["dns"].is_null() && run["ms"]["tcp"].is_null() && run["ms"]["tls"].is_null()
        );
    }
}

#[test]
fn happy_eyeballs_moves_past_a_dead_address() {
    // Nothing listens on [::1] (and some machines have no IPv6 at all), so
    // the first attempt fails at once and the second address wins without
    // waiting out the 250 ms attempt delay.
    let port = server().https.port();
    let pin = format!("localhost:{port}:[::1],127.0.0.1");
    let (code, report) = json(&["--resolve", &pin, &server().https_url("/")]);
    assert_eq!(code, 0);
    let hop = &hops(&report)[0];
    assert_eq!(hop["connect_attempts"], 2);
    assert_eq!(hop["peer"], format!("127.0.0.1:{port}"));
    assert!(ms(&hop["ms"]["tcp"]) < 250.0, "{}", hop["ms"]);
}

#[test]
fn untrusted_certificates_are_refused() {
    assert_cmd::Command::cargo_bin("misura")
        .unwrap()
        .arg(server().https_url("/"))
        .assert()
        .code(2)
        .stderr(contains("TLS handshake"));
}
