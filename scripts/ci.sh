#!/usr/bin/env bash
# The CI gate — single source of truth.
#
# Run by BOTH .github/workflows/ci.yml AND .githooks/pre-push, so "green
# locally" and "green in CI" are byte-identical commands. There is no path where
# a build succeeds and tests are skipped: `set -e` makes the gate fail (and the
# push abort) if EITHER step fails. Build-passing always implies tests ran.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

echo "==> cargo build --bins"
cargo build --bins

echo "==> cargo test --features test-support"
# test-support gates the in-process transport seam (AcpChannelClient::test_connected,
# FakeTransport) the REAL-loop harness tests use — the mid-turn m/leader and steering
# guards. Without the feature those #[cfg(feature="test-support")] tests are compiled
# out and gate NOTHING. This run is a superset of the default `cargo test`.
cargo test --features test-support

echo "✓ CI gate passed (build + test)"
