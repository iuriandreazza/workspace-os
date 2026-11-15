#!/usr/bin/env bash
# Boots the kernel in QEMU and checks the serial console: the boot banner must
# appear exactly once and the kernel must stay up, without a panic, after it.
#
# Known gap: QEMU's ELF loader zeroes `.bss` itself, so this test cannot detect
# boot code that fails to clear it. That needs a raw-image boot over memory
# that was dirtied first.
set -euo pipefail

readonly EXPECTED_BANNER="workspace-os kernel: booted on aarch64"
readonly PANIC_MARKER="kernel panic"
readonly BOOT_TIMEOUT_SECONDS=10
readonly SETTLE_SECONDS=1
# Fixed target directory: with CARGO_TARGET_DIR set, cargo would build
# elsewhere and the test would boot a stale binary from here.
readonly TARGET_DIR="target"
readonly KERNEL_ELF="$TARGET_DIR/aarch64-unknown-none-softfloat/debug/kernel"

cd "$(dirname "$0")/.."
cargo build --quiet --target-dir "$TARGET_DIR"

serial_log="$(mktemp)"
qemu_pid=""

cleanup() {
    if [[ -n "$qemu_pid" ]]; then
        kill "$qemu_pid" 2>/dev/null || true
    fi
    rm -f "$serial_log"
}
trap cleanup EXIT

fail() {
    echo "FAIL: $1" >&2
    cat "$serial_log" >&2
    exit 1
}

qemu_is_running() {
    kill -0 "$qemu_pid" 2>/dev/null
}

wait_for_banner() {
    for _ in $(seq 1 $((BOOT_TIMEOUT_SECONDS * 10))); do
        if grep -qF "$EXPECTED_BANNER" "$serial_log"; then
            return 0
        fi
        qemu_is_running || fail "QEMU exited before the boot banner appeared"
        sleep 0.1
    done
    fail "boot banner not found within ${BOOT_TIMEOUT_SECONDS}s"
}

# Machine flags must match the cargo runner in .cargo/config.toml.
# -nodefaults: the default virtio network card needs an option ROM that
# minimal QEMU installs (such as the CI runner) do not ship.
qemu-system-aarch64 -machine virt -cpu cortex-a72 -nodefaults -display none \
    -serial "file:$serial_log" -kernel "$KERNEL_ELF" &
qemu_pid=$!

wait_for_banner

# A kernel that panics, resets or exits right after the banner is still broken.
sleep "$SETTLE_SECONDS"
qemu_is_running || fail "QEMU exited after the boot banner"
if grep -qF "$PANIC_MARKER" "$serial_log"; then
    fail "kernel panicked after the boot banner"
fi
if [[ "$(grep -cF "$EXPECTED_BANNER" "$serial_log")" -ne 1 ]]; then
    fail "boot banner printed more than once"
fi

echo "PASS: kernel booted and stayed up"
