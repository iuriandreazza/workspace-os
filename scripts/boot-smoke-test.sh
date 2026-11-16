#!/usr/bin/env bash
# Boots the kernel in QEMU several times and checks the serial console each
# time. A normal boot must print the banner exactly once, in EL1, take one
# deliberate breakpoint, resume after it, and stay up without a panic.
#
#   elf  the ELF image through `-kernel`, the way `cargo run` boots it
#   raw  a raw image over a `.bss` region filled with garbage first. QEMU's
#        ELF loader zeroes `.bss` itself, so only this boot proves that the
#        boot code clears it.
#   smp  four cores that all enter `_start`. With `secure=on` QEMU does not
#        hold the secondary cores back, so only this boot proves that the
#        boot code parks them. It also starts at EL3.
#   el2  started at EL2, which the boot code must leave for EL1
#   el3  started at EL3, which the boot code must leave for EL1
#   el3-el2  started at EL3 with EL2 present: the boot code goes through EL2
#
# Two more normal boots give the machine a different amount of memory each. The
# kernel must print the size the device tree holds, so a kernel that fixes the
# size, or reads it from the wrong place, fails one of them. Every other boot
# also checks the memory line, against the default size below.
#
#   mem-64   `-m 64M`
#   mem-512  `-m 512M`
#
# Every normal boot also checks the physical frame allocator through what the
# kernel prints: the map covers the whole of RAM, the kernel image and boot
# stack it reserves are the ones the linker placed (read back from the ELF
# symbols), draining the allocator ends in a reported refusal, every frame
# comes back, a freed frame is reused and a double free is refused. The 64 MiB
# and 512 MiB boots show the frame counts following the size.
#
# One more normal boot hands the kernel a device tree with two `/memreserve/`
# entries (one unaligned, overlapping a frame boundary): the frames they touch
# must be reserved on top of the kernel's own.
#
#   memreserve  `-dtb` with the machine's tree plus those entries
#
# Three more boots use kernels built to raise an exception nothing resumes
# from, and must end in a kernel panic that names it:
#
#   unhandled-class   a synchronous exception of a class with no handler
#   unhandled-vector  an exception on a vector entry that is not handled
#   compiler-trap     `brk #0x1`, which the compiler emits for `abort`, must
#                     not be resumed like the kernel's own breakpoint
set -euo pipefail

readonly EXPECTED_BANNER="workspace-os kernel: booted on aarch64 in EL1"
readonly DEFAULT_MEMORY_MIB=128
readonly RAM_BASE="0x40000000"
readonly UART_REPORT="workspace-os kernel: uart pl011 at 0x9000000"
readonly BREAKPOINT_REPORT="exception: breakpoint #0x800 at"
readonly RESUME_MARKER="workspace-os kernel: resumed after breakpoint"
readonly PANIC_MARKER="kernel panic"
readonly FRAME_SIZE=4096
# `/memreserve/` entries the memreserve boot adds: 16 frames, then a range that
# starts inside the last of them and ends inside the next frame, 17 in all.
readonly MEMRESERVE_FRAMES=17
# The kernel image, boot stack and device tree blob together stay well below this.
readonly MAX_RESERVED_FRAMES=1024
readonly BOOT_TIMEOUT_SECONDS=10
readonly SETTLE_SECONDS=1
# Fixed target directory: with CARGO_TARGET_DIR set, cargo would build
# elsewhere and the test would boot a stale binary from here.
readonly TARGET_DIR="target"
readonly KERNEL_ELF="$TARGET_DIR/aarch64-unknown-none-softfloat/debug/kernel"
# Machine flags must match the cargo runner in .cargo/config.toml.
# -nodefaults: the default virtio network card needs an option ROM that
# minimal QEMU installs (such as the CI runner) do not ship.
readonly QEMU_MACHINE=(-machine virt -cpu cortex-a72 -nodefaults -display none -m "${DEFAULT_MEMORY_MIB}M")

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

# wait_for_line TEXT: the self-tests after the banner take longer on a slow
# host or a big memory, so wait for the last frame line instead of a fixed time.
wait_for_line() {
    for _ in $(seq 1 $((BOOT_TIMEOUT_SECONDS * 10))); do
        if grep -qF "$1" "$serial_log"; then
            return 0
        fi
        if grep -qF "$PANIC_MARKER" "$serial_log"; then
            fail "kernel panicked before '$1'"
        fi
        qemu_is_running || fail "QEMU exited before '$1'"
        sleep 0.1
    done
    fail "'$1' not found within ${BOOT_TIMEOUT_SECONDS}s"
}

# expect_once TEXT WHAT: the serial log holds TEXT exactly once.
expect_once() {
    local count
    count="$(grep -cF "$1" "$serial_log" || true)"
    if [[ "$count" -ne 1 ]]; then
        fail "$2 seen $count times, expected once"
    fi
}

start_qemu() {
    boot_name="$1"
    shift
    serial_log="$work_dir/$boot_name.log"
    : > "$serial_log"

    # QEMU's own messages are kept aside and only shown when the boot fails.
    qemu-system-aarch64 "${QEMU_MACHINE[@]}" -serial "file:$serial_log" "$@" \
        2> "$work_dir/$boot_name.qemu" &
    qemu_pid=$!
}

# memory_mib_of QEMU_ARGS...: the size the last `-m` asks for, in MiB.
memory_mib_of() {
    local mib="$DEFAULT_MEMORY_MIB" previous=""
    for arg in "$@"; do
        if [[ "$previous" == "-m" ]]; then
            mib="${arg%M}"
        fi
        previous="$arg"
    done
    echo "$mib"
}

extra_reserved_frames=0
extra_reserved_ranges=0
expected_blob_size=""

# check_frames MEMORY_MIB: the frame allocator's report is coherent with the
# amount of RAM and the layout the linker chose.
check_frames() {
    local mib="$1" counts total reserved free
    expect_once "workspace-os kernel: reserved kernel image $image_range" "kernel image reservation"
    expect_once "workspace-os kernel: reserved boot stack $stack_range" "boot stack reservation"
    expect_once "in memory map" "frame map report"
    counts="$(tr -d '\r' < "$serial_log" | sed -n 's/^workspace-os kernel: frames: \([0-9]*\) in memory map, \([0-9]*\) reserved, \([0-9]*\) free$/\1 \2 \3/p')"
    [[ -n "$counts" ]] || fail "frame map report has an unexpected format"
    read -r total reserved free <<< "$counts"

    [[ "$total" -eq $((mib * 1024 * 1024 / FRAME_SIZE)) ]] \
        || fail "frame map holds $total frames, expected all of ${mib} MiB"
    [[ $((reserved + free)) -eq "$total" ]] \
        || fail "$reserved reserved and $free free frames do not add up to $total"
    [[ "$reserved" -gt 0 && "$reserved" -le "$MAX_RESERVED_FRAMES" ]] \
        || fail "$reserved frames reserved, expected between 1 and $MAX_RESERVED_FRAMES"
    [[ "$free" -gt 0 ]] || fail "no free frames"

    # The device tree blob must be reserved too, and the reserved count must
    # cover the kernel (image and stack) and the blob, whichever way it is built.
    local blob blob_start blob_end kernel_frames blob_frames
    blob="$(tr -d '\r' < "$serial_log" | sed -n 's/^workspace-os kernel: reserved device tree \(0x[0-9a-f]*\)\.\.\(0x[0-9a-f]*\)$/\1 \2/p')"
    [[ "$(echo "$blob" | wc -l)" -eq 1 && -n "$blob" ]] || fail "device tree reservation not reported exactly once"
    read -r blob_start blob_end <<< "$blob"
    # QEMU loads the tree at the start of RAM, with the size its header holds.
    # (QEMU rewrites a tree it is handed with `-dtb`, so only the start is exact then.)
    [[ "$blob_start" -eq "$RAM_BASE" && "$blob_end" -gt "$blob_start" ]] \
        || fail "device tree reserved as $blob_start..$blob_end, expected it to start at $RAM_BASE"
    [[ -n "$expected_blob_size" && "$blob_end" -ne $((RAM_BASE + expected_blob_size)) ]] \
        && fail "device tree reserved as $blob_start..$blob_end, expected $expected_blob_size bytes"
    kernel_frames=$(( (image_end_address + FRAME_SIZE - 1) / FRAME_SIZE - image_start_address / FRAME_SIZE ))
    blob_frames=$(( (blob_end + FRAME_SIZE - 1) / FRAME_SIZE - blob_start / FRAME_SIZE ))
    # The machine's own tree declares nothing else to reserve.
    [[ "$reserved" -eq $((kernel_frames + blob_frames + extra_reserved_frames)) ]] \
        || fail "$reserved frames reserved, expected $((kernel_frames + blob_frames + extra_reserved_frames))"
    expect_once "workspace-os kernel: reserved by the device tree: $extra_reserved_ranges regions" \
        "device tree reservation count"

    expect_once "workspace-os kernel: frames: exhausted after $free allocations, next request refused" \
        "exhaustion report"
    expect_once "workspace-os kernel: frames: returned $free, $free free again" "return of every frame"
    expect_once "workspace-os kernel: frames: double free refused" "double free refusal"

    local reused ram_end
    reused="$(tr -d '\r' < "$serial_log" | sed -n 's/^workspace-os kernel: frames: reused \(0x[0-9a-f]*\) after freeing it$/\1/p')"
    [[ "$(echo "$reused" | wc -l)" -eq 1 && -n "$reused" ]] || fail "frame reuse not reported exactly once"
    ram_end=$((RAM_BASE + mib * 1024 * 1024))
    [[ "$reused" -ge "$RAM_BASE" && "$reused" -lt "$ram_end" && $((reused % FRAME_SIZE)) -eq 0 ]] \
        || fail "reused frame $reused is not a frame of RAM"
}

# check_boot NAME QEMU_IMAGE_ARGS...
check_boot() {
    local expected_memory
    expected_memory="workspace-os kernel: memory $(memory_mib_of "$@") MiB at $RAM_BASE"
    start_qemu "$@"

    wait_for_banner
    wait_for_line "$RESUME_MARKER"

    # A kernel that panics, resets or exits right after the banner is still broken.
    sleep "$SETTLE_SECONDS"
    qemu_is_running || fail "QEMU exited after the boot banner"
    if grep -qF "$PANIC_MARKER" "$serial_log"; then
        fail "kernel panicked after the boot banner"
    fi
    expect_once "$EXPECTED_BANNER" "boot banner"
    expect_once "$expected_memory" "memory report"
    expect_once "$UART_REPORT" "UART report"
    expect_once "$BREAKPOINT_REPORT" "breakpoint report"
    expect_once "$RESUME_MARKER" "resume after the breakpoint"
    check_frames "$(memory_mib_of "$@")"

    stop_qemu
    echo "PASS ($boot_name): kernel booted and stayed up"
}

# check_panic NAME ELF EXPECTED_TEXT: the kernel must end in a kernel panic
# whose message holds EXPECTED_TEXT.
check_panic() {
    local elf="$2" expected="$3"
    start_qemu "$1" -kernel "$elf"

    # The panic line reaches the log in pieces: wait for the text itself.
    for _ in $(seq 1 $((BOOT_TIMEOUT_SECONDS * 10))); do
        if grep -qF "$expected" "$serial_log"; then
            break
        fi
        qemu_is_running || fail "QEMU exited without the expected kernel panic"
        sleep 0.1
    done
    grep -qF "$expected" "$serial_log" || fail "no kernel panic mentioning '$expected' within ${BOOT_TIMEOUT_SECONDS}s"
    grep -qF "$PANIC_MARKER" "$serial_log" || fail "'$expected' appeared without a kernel panic"

    stop_qemu
    echo "PASS ($boot_name): unhandled exception ended in a kernel panic"
}

# build_with_feature FEATURE DESTINATION: builds the kernel with a self-test
# feature and keeps the image, so the next build cannot replace it.
build_with_feature() {
    cargo build --quiet --target-dir "$TARGET_DIR" -p kernel --features "$1"
    cp "$KERNEL_ELF" "$2"
}

# llvm-nm and llvm-objcopy come with the `llvm-tools` rustup component.
llvm_tools="$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin"

symbol_address() {
    "$llvm_tools/llvm-nm" "$KERNEL_ELF" | awk -v name="$1" '$3 == name { print "0x" $1 }'
}

entry="$(symbol_address _start)"
image_start_address=$(( $(symbol_address __image_start) ))
image_end_address=$(( $(symbol_address __stack_top) ))
hex() { printf '0x%x' "$((0x$1))"; }
image_range="$(hex "$(symbol_address __image_start | sed 's/^0x//')")..$(hex "$(symbol_address __stack_bottom | sed 's/^0x//')")"
stack_range="$(hex "$(symbol_address __stack_bottom | sed 's/^0x//')")..$(hex "$(symbol_address __stack_top | sed 's/^0x//')")"
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

class_elf="$work_dir/unhandled-class.elf"
vector_elf="$work_dir/unhandled-vector.elf"
trap_elf="$work_dir/compiler-trap.elf"
build_with_feature selftest-unhandled-class "$class_elf"
build_with_feature selftest-unhandled-vector "$vector_elf"
build_with_feature selftest-compiler-trap "$trap_elf"
cargo build --quiet --target-dir "$TARGET_DIR"

# The machine's device tree, dumped, and a copy with two `/memreserve/` entries
# put in front of the structure block (the dump leaves room: it is padded).
dtb="$work_dir/machine.dtb"
qemu-system-aarch64 "${QEMU_MACHINE[@]}" -machine dumpdtb="$dtb" 2> /dev/null
dtb_size="$(wc -c < "$dtb" | tr -d ' ')"
expected_blob_size="$dtb_size"
memreserve_dtb="$work_dir/memreserve.dtb"
python3 -I - "$dtb" "$memreserve_dtb" <<'PY'
import struct, sys
source, target = sys.argv[1:3]
blob = open(source, "rb").read()
fields = list(struct.unpack(">10I", blob[:40]))
rsvmap, structure, strings, strings_size = fields[4], fields[2], fields[3], fields[8]
entries = struct.pack(">4Q", 0x41000000, 0x10000, 0x4100F800, 0x1000)
# Keep only the used part: QEMU grows a tree it is handed with `-dtb`, and
# the machine's own is padded to 1 MiB, which then no longer fits below the kernel.
body = entries + blob[rsvmap:strings + strings_size]
fields[2], fields[3] = structure + len(entries), strings + len(entries)
total = (rsvmap + len(body) + 7) // 8 * 8
fields[1] = total
open(target, "wb").write((struct.pack(">10I", *fields) + blob[40:rsvmap] + body).ljust(total, b"\0"))
PY

check_boot elf -kernel "$KERNEL_ELF"
check_boot raw \
    -device "loader,file=$raw_image,addr=$entry,cpu-num=0,force-raw=on" \
    -device "loader,file=$garbage,addr=$bss_start,force-raw=on"
check_boot mem-64 -m 64M -kernel "$KERNEL_ELF"
check_boot mem-512 -m 512M -kernel "$KERNEL_ELF"
check_boot smp -machine secure=on -smp 4 -kernel "$KERNEL_ELF"
check_boot el2 -machine virtualization=on -kernel "$KERNEL_ELF"
check_boot el3 -machine secure=on -kernel "$KERNEL_ELF"
check_boot el3-el2 -machine secure=on,virtualization=on -kernel "$KERNEL_ELF"
extra_reserved_frames=$MEMRESERVE_FRAMES
extra_reserved_ranges=2
expected_blob_size=""
check_boot memreserve -dtb "$memreserve_dtb" -kernel "$KERNEL_ELF"
extra_reserved_frames=0
extra_reserved_ranges=0
check_panic unhandled-class "$class_elf" "SVC (AArch64)"
check_panic unhandled-vector "$vector_elf" "synchronous, current EL with SP_EL0"
check_panic compiler-trap "$trap_elf" "BRK (AArch64) (class 0x3c, iss 0x0000001"
