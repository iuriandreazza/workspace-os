#!/usr/bin/env bash
# Runs every check a change must pass. Used by the pre-commit hook and by CI,
# so both always enforce the same thing.
set -euo pipefail

cd "$(dirname "$0")/.."

require_tool() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "check: '$1' not found on PATH. $2" >&2
        exit 1
    fi
}

require_tool cargo "Install Rust with rustup and add its bin directory to PATH."
require_tool qemu-system-aarch64 "Install QEMU."

echo "==> format"
cargo fmt --all --check

echo "==> lint"
cargo clippy --all-targets -p kernel -- -D warnings
cargo clippy --all-targets -p kernel --features selftest-unhandled-class,selftest-unhandled-vector,selftest-compiler-trap -- -D warnings
# The device tree reader is plain Rust with unit tests, so it is checked and
# tested on the host: the test harness cannot be built for the bare-metal target.
host_target="$(rustc -vV | sed -n 's/^host: //p')"
cargo clippy --all-targets -p devicetree --target "$host_target" -- -D warnings

echo "==> unit tests"
cargo test --quiet -p devicetree --target "$host_target"

echo "==> boot smoke test"
./scripts/boot-smoke-test.sh
