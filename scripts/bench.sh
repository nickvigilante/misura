#!/usr/bin/env bash
# Compares misura's startup and request cost with curl's, against the local
# test server, so the numbers measure the tools rather than the network.
#
# Usage: scripts/bench.sh [OUTPUT.md]
# Needs hyperfine and curl. Writes a Markdown table to OUTPUT.md if given.
set -euo pipefail

cd "$(dirname "$0")/.."
cargo build --release --locked --quiet
cargo build --release --locked --quiet --example testserver

dir=$(mktemp -d)
target/release/examples/testserver "$dir" 2>/dev/null &
server=$!
trap 'kill $server; rm -rf "$dir"' EXIT
for _ in $(seq 100); do
  [[ -f $dir/cert.pem ]] && break
  sleep 0.1
done
if [[ ! -f $dir/cert.pem ]]; then
  echo "bench.sh: the test server didn't start; is something else on port 8080 or 8443?" >&2
  exit 1
fi
sleep 0.2

cert=$dir/cert.pem
args=(-N --warmup 20 --runs 200)
if [[ $# -gt 0 ]]; then
  args+=(--export-markdown "$1")
fi
hyperfine "${args[@]}" \
  -n "misura https" "target/release/misura --cacert $cert https://localhost:8443/" \
  -n "curl https" "curl -so /dev/null --noproxy * --cacert $cert https://localhost:8443/" \
  -n "misura http" "target/release/misura http://127.0.0.1:8080/" \
  -n "curl http" "curl -so /dev/null --noproxy * http://127.0.0.1:8080/"
