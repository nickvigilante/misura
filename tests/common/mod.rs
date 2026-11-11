//! Helpers for the integration tests.

#![allow(dead_code)]

pub mod server;

use std::sync::OnceLock;

use assert_cmd::Command;
use serde_json::Value;

/// One server for every test in a test binary.
pub fn server() -> &'static server::Server {
    static SERVER: OnceLock<(tempfile::TempDir, server::Server)> = OnceLock::new();
    &SERVER
        .get_or_init(|| {
            let dir = tempfile::tempdir().unwrap();
            let server = server::start(dir.path(), 0, 0);
            (dir, server)
        })
        .1
}

/// misura, trusting the test server's certificate.
pub fn misura() -> Command {
    let mut cmd = Command::cargo_bin("misura").unwrap();
    cmd.arg("--cacert").arg(&server().cert);
    cmd
}

/// Runs misura with `--json` and returns the exit code and the report.
pub fn json(args: &[&str]) -> (i32, Value) {
    let output = misura().arg("--json").args(args).output().unwrap();
    let code = output.status.code().unwrap();
    let value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "{e}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (code, value)
}

/// The first run's hops.
pub fn hops(report: &Value) -> &Vec<Value> {
    report["results"][0]["runs"][0]["hops"].as_array().unwrap()
}

/// A duration from the JSON, in milliseconds.
pub fn ms(value: &Value) -> f64 {
    value
        .as_f64()
        .unwrap_or_else(|| panic!("not a duration: {value}"))
}
