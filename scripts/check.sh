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
require_tool python3 "Install Python 3 (standard library only; the licence check uses it)."

echo "==> format"
cargo fmt --all --check

echo "==> licences"
# Third-party licences against ADR 0005 and ADR 0017; its decision logic has unit tests.
python3 -I scripts/test_check_licences.py
python3 -I scripts/check-licences.py

echo "==> lint"
cargo clippy --all-targets -p kernel -- -D warnings
cargo clippy --all-targets -p kernel --features selftest-unhandled-class,selftest-unhandled-vector,selftest-compiler-trap -- -D warnings
# The device tree reader, the frame allocator and the handoff structure
# (`bootinfo`) are plain Rust with unit tests, so they are checked and tested on
# the host: the test harness cannot be built for the bare-metal target. The
# kernel also builds `bootinfo` for the bare-metal target, as a dependency.
host_target="$(rustc -vV | sed -n 's/^host: //p')"
cargo clippy --all-targets -p devicetree -p physmem -p bootinfo --target "$host_target" -- -D warnings
# The bootloader is a third target: a UEFI application, which neither the
# bare-metal kernel target nor the host can build. It is not a default member
# of the workspace, so a bare `cargo build` never tries.
cargo clippy --all-targets -p bootloader --target aarch64-unknown-uefi -- -D warnings
cargo clippy --all-targets -p bootloader --target aarch64-unknown-uefi --features selftest-unknown-boot-info-version -- -D warnings
# The intent shell is host code (std, sockets, the real filesystem), so like the
# device tree reader it is built for the host. Its tests never need the model.
cargo clippy --all-targets -p intent-shell --target "$host_target" -- -D warnings
# The deterministic shell and the recovery mode build without the interface (ADR 0015).
cargo clippy --all-targets -p intent-shell --target "$host_target" --no-default-features -- -D warnings

echo "==> unit tests"
cargo test --quiet -p devicetree -p physmem -p bootinfo --target "$host_target"
cargo test --quiet -p intent-shell --target "$host_target"
cargo test --quiet -p intent-shell --target "$host_target" --no-default-features

echo "==> boot smoke test"
./scripts/boot-smoke-test.sh
