//! The shape of misura's output, with durations and ports redacted.

mod common;

use common::{misura, server};

fn run(args: &[&str]) -> String {
    let output = misura().args(args).output().unwrap();
    String::from_utf8(output.stdout).unwrap()
}

macro_rules! redacted {
    ($value:expr) => {
        insta::with_settings!({filters => vec![
            (r"(localhost|127\.0\.0\.1):\d+", "$1:[port]"),
            // On macOS, localhost resolves to ::1 first, where nothing listens.
            (r" \(after trying \d+ addresses\)", ""),
            // JSON durations.
            (r#"": \d+\.\d+"#, r#"": [ms]"#),
            // A waterfall row: the duration and the bar after it.
            (r"(?m)^(  [A-Za-z ]+?) +\d+\.\d+ ms.*$", "$1 [ms]"),
            // A row of percentiles or a --compare row.
            (r"(?m)^(  [A-Za-z ]+?)( +(\d+\.\d+|—))+$", "$1 [ms...]"),
            (r"\d+\.\d+ ms", "[ms]"),
        ]}, {
            insta::assert_snapshot!($value);
        });
    };
}

#[test]
fn waterfall() {
    redacted!(run(&[&server().https_url("/?size=2048")]));
}

#[test]
fn waterfall_plain_http() {
    redacted!(run(&[&server().http_url("/")]));
}

#[test]
fn redirect_chain() {
    redacted!(run(&["-L", &server().https_url("/redirect?to=/")]));
}

#[test]
fn percentiles() {
    redacted!(run(&["-n", "3", "--reuse", &server().https_url("/")]));
}

#[test]
fn compare() {
    redacted!(run(&["--compare", &server().https_url("/")]));
}

#[test]
fn threshold_failure() {
    redacted!(run(&[
        "--max-ttfb",
        "1ms",
        &server().http_url("/?headers_ms=20")
    ]));
}

#[test]
fn json_shape() {
    redacted!(run(&[
        "--json",
        "-L",
        &server().https_url("/redirect?to=/")
    ]));
}
