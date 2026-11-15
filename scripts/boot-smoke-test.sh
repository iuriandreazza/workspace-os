#!/usr/bin/env bash
# Boots the kernel in QEMU three times and checks the serial console each
# time: the boot banner must appear exactly once and the kernel must stay up,
# without a panic, after it.
#
#   elf  the ELF image through `-kernel`, the way `cargo run` boots it
#   raw  a raw image over a `.bss` region filled with garbage first. QEMU's
#        ELF loader zeroes `.bss` itself, so only this boot proves that the
#        boot code clears it.
#   smp  four cores that all enter `_start`. With `secure=on` QEMU does not
#        hold the secondary cores back, so only this boot proves that the
#        boot code parks them.
set -euo pipefail

readonly EXPECTED_BANNER="workspace-os kernel: booted on aarch64"
readonly PANIC_MARKER="kernel panic"
readonly BOOT_TIMEOUT_SECONDS=10
readonly SETTLE_SECONDS=1
# Fixed target directory: with CARGO_TARGET_DIR set, cargo would build
# elsewhere and the test would boot a stale binary from here.
readonly TARGET_DIR="target"
readonly KERNEL_ELF="$TARGET_DIR/aarch64-unknown-none-softfloat/debug/kernel"
# Machine flags must match the cargo runner in .cargo/config.toml.
# -nodefaults: the default virtio network card needs an option ROM that
# minimal QEMU installs (such as the CI runner) do not ship.
readonly QEMU_MACHINE=(-machine virt -cpu cortex-a72 -nodefaults -display none)

cd "$(dirname "$0")/.."
cargo build --quiet --target-dir "$TARGET_DIR"

work_dir="$(mktemp -d)"
boot_name="setup"
serial_log="/dev/null"
qemu_pid=""

stop_qemu() {
    if [[ -n "$qemu_pid" ]]; then
        kill "$qemu_pid" 2>/dev/null || true
        wait "$qemu_pid" 2>/dev/null || true
        qemu_pid=""
    fi
}

cleanup() {
    stop_qemu
    rm -rf "$work_dir"
}
trap cleanup EXIT

fail() {
    echo "FAIL ($boot_name): $1" >&2
    cat "$serial_log" >&2
    if [[ -s "$work_dir/$boot_name.qemu" ]]; then
        cat "$work_dir/$boot_name.qemu" >&2
    fi
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
        if grep -qF "$PANIC_MARKER" "$serial_log"; then
            fail "kernel panicked before the boot banner"
        fi
        qemu_is_running || fail "QEMU exited before the boot banner appeared"
        sleep 0.1
    done
    fail "boot banner not found within ${BOOT_TIMEOUT_SECONDS}s"
}

# check_boot NAME QEMU_IMAGE_ARGS...
check_boot() {
    boot_name="$1"
    shift
    serial_log="$work_dir/$boot_name.log"
    : > "$serial_log"

    # QEMU's own messages are kept aside and only shown when the boot fails.
    qemu-system-aarch64 "${QEMU_MACHINE[@]}" -serial "file:$serial_log" "$@" \
        2> "$work_dir/$boot_name.qemu" &
    qemu_pid=$!

    wait_for_banner

    # A kernel that panics, resets or exits right after the banner is still broken.
    sleep "$SETTLE_SECONDS"
    qemu_is_running || fail "QEMU exited after the boot banner"
    if grep -qF "$PANIC_MARKER" "$serial_log"; then
        fail "kernel panicked after the boot banner"
    fi
    if [[ "$(grep -oF "$EXPECTED_BANNER" "$serial_log" | wc -l)" -ne 1 ]]; then
        fail "boot banner printed more than once"
    fi

    stop_qemu
    echo "PASS ($boot_name): kernel booted and stayed up"
}

# llvm-nm and llvm-objcopy come with the `llvm-tools` rustup component.
llvm_tools="$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin"

symbol_address() {
    "$llvm_tools/llvm-nm" "$KERNEL_ELF" | awk -v name="$1" '$3 == name { print "0x" $1 }'
}

entry="$(symbol_address _start)"
bss_start="$(symbol_address __bss_start)"
bss_end="$(symbol_address __bss_end)"
bss_size=$((bss_end - bss_start))
if [[ "$bss_size" -le 0 ]]; then
    fail ".bss is empty, so the raw boot would not exercise the clearing loop"
fi

raw_image="$work_dir/kernel.bin"
garbage="$work_dir/garbage.bin"
"$llvm_tools/llvm-objcopy" -O binary "$KERNEL_ELF" "$raw_image"
head -c "$bss_size" /dev/zero | LC_ALL=C tr '\0' '\377' > "$garbage"

check_boot elf -kernel "$KERNEL_ELF"
check_boot raw \
    -device "loader,file=$raw_image,addr=$entry,cpu-num=0,force-raw=on" \
    -device "loader,file=$garbage,addr=$bss_start,force-raw=on"
check_boot smp -machine secure=on -smp 4 -kernel "$KERNEL_ELF"
