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

echo "==> cargo test"
cargo test

echo "✓ CI gate passed (build + test)"
