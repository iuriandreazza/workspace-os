#!/usr/bin/env bash
# Boots the kernel in QEMU and checks the boot banner on the serial console.
set -euo pipefail

readonly EXPECTED_BANNER="workspace-os kernel: booted on aarch64"
readonly BOOT_TIMEOUT_SECONDS=10
readonly KERNEL_ELF="target/aarch64-unknown-none-softfloat/debug/kernel"

cd "$(dirname "$0")/.."
cargo build --quiet

serial_log="$(mktemp)"
qemu_pid=""

cleanup() {
    if [[ -n "$qemu_pid" ]]; then
        kill "$qemu_pid" 2>/dev/null || true
    fi
    rm -f "$serial_log"
}
trap cleanup EXIT

# -nodefaults: the default virtio network card needs an option ROM that
# minimal QEMU installs (such as the CI runner) do not ship.
qemu-system-aarch64 -machine virt -cpu cortex-a72 -nodefaults -display none \
    -serial "file:$serial_log" -kernel "$KERNEL_ELF" &
qemu_pid=$!

# The kernel never exits, so poll the serial log instead of waiting for QEMU.
for _ in $(seq 1 $((BOOT_TIMEOUT_SECONDS * 10))); do
    if grep -qF "$EXPECTED_BANNER" "$serial_log"; then
        echo "PASS: boot banner found on serial console"
        exit 0
    fi
    if ! kill -0 "$qemu_pid" 2>/dev/null; then
        echo "FAIL: QEMU exited before the boot banner appeared" >&2
        cat "$serial_log" >&2
        exit 1
    fi
    sleep 0.1
done

echo "FAIL: boot banner not found within ${BOOT_TIMEOUT_SECONDS}s" >&2
cat "$serial_log" >&2
exit 1
