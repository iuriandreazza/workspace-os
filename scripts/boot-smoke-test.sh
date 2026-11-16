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
# The kernel is also booted through the bootloader, from a disk image under
# QEMU with UEFI firmware (ADR 0013): the bootloader must load the kernel from
# the EFI system partition, leave boot services and hand over the boot info,
# and the kernel must say it was entered that way and report a coherent
# memory map. The firmware is found at the usual places, or in UEFI_FIRMWARE.
#
#   uefi      256 MiB, the default
#   uefi-768  768 MiB: the kernel sizes memory from the bootloader's map, so
#             the amount it reports follows
#   uefi-el2  the firmware runs the bootloader at EL2, which the bootloader
#             must leave for EL1 itself before the kernel starts
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
# Not readonly: the boots that start from UEFI firmware take longer to reach the
# kernel, and set a longer wait for their own duration.
BOOT_TIMEOUT_SECONDS=10
readonly UEFI_BOOT_TIMEOUT_SECONDS=90
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

# find_uefi_firmware: prints the AArch64 UEFI firmware image QEMU can load with
# `-bios`: UEFI_FIRMWARE if set, else the first of the packaged locations.
find_uefi_firmware() {
    if [[ -n "${UEFI_FIRMWARE:-}" ]]; then
        echo "$UEFI_FIRMWARE"
        return
    fi
    local candidate
    for candidate in \
        /opt/homebrew/share/qemu/edk2-aarch64-code.fd \
        /usr/local/share/qemu/edk2-aarch64-code.fd \
        /usr/share/qemu-efi-aarch64/QEMU_EFI.fd \
        /usr/share/AAVMF/AAVMF_CODE.fd \
        /usr/share/qemu/edk2-aarch64-code.fd; do
        if [[ -f "$candidate" ]]; then
            echo "$candidate"
            return
        fi
    done
    echo "boot smoke test: no AArch64 UEFI firmware found. Install qemu-efi-aarch64 (Debian, Ubuntu) or QEMU's edk2 files (Homebrew), or set UEFI_FIRMWARE." >&2
    exit 1
}

# check_uefi_boot NAME MEMORY_MIB BOOTLOADER_EL [QEMU_ARGS...]: boots the disk image through the UEFI
# firmware and the bootloader. The firmware publishes a device tree only with
# ACPI off (ADR 0013, "What is not yet known"), hence `acpi=off`.
check_uefi_boot() {
    local mib="$2" bootloader_el="$3" counts total reserved free copy_start copy_end
    BOOT_TIMEOUT_SECONDS=$UEFI_BOOT_TIMEOUT_SECONDS
    start_qemu "$1" -machine virt,acpi=off -m "${mib}M" -bios "$uefi_firmware" \
        -drive "file=$disk_image,format=raw,if=virtio" "${@:4}"

    wait_for_banner
    wait_for_line "$RESUME_MARKER"
    sleep "$SETTLE_SECONDS"
    qemu_is_running || fail "QEMU exited after the boot banner"
    if grep -qF "$PANIC_MARKER" "$serial_log"; then
        fail "kernel panicked after the boot banner"
    fi

    # The bootloader said where it put the kernel, and that is where the linker
    # placed the image (the stack included): a loader that misplaces or cuts
    # the image cannot print this.
    expect_once "workspace-os bootloader: started at EL$bootloader_el" "bootloader's exception level"
    expect_once "workspace-os bootloader: kernel loaded at $(hex "$(symbol_address __image_start | sed 's/^0x//')")..$(hex "$(symbol_address __stack_top | sed 's/^0x//')"), entry $(hex "$(symbol_address _start | sed 's/^0x//')")" \
        "bootloader's report of the kernel placement"
    expect_once "$EXPECTED_BANNER" "boot banner"
    expect_once "$UART_REPORT" "UART report"
    expect_once "$BREAKPOINT_REPORT" "breakpoint report"
    expect_once "$RESUME_MARKER" "resume after the breakpoint"
    # The origin of the entry is visible on the serial console.
    expect_once "workspace-os kernel: entry: bootloader, boot info 1.0, mode 0," "entry through the bootloader"
    if grep -qF "entry: direct load" "$serial_log"; then
        fail "kernel reports a direct load after a boot through the bootloader"
    fi
    # The processor identity, the tables and the registers the ADR fixes to zero:
    # the boot processor of QEMU `virt` is 0, ACPI is off, the system table is set.
    expect_once "workspace-os kernel: handoff: boot cpu 0x0, rsdp 0x0, efi system table 0x" "handoff fields"
    if grep -qF "efi system table 0x0," "$serial_log"; then
        fail "boot info carries no EFI system table"
    fi
    expect_once ", x2|x3 0" "x2 and x3 at entry"
    expect_once "workspace-os kernel: reserved kernel image $image_range" "kernel image reservation"
    expect_once "workspace-os kernel: reserved boot stack $stack_range" "boot stack reservation"

    # The kernel found an intact copy of the device tree, as big as the firmware's.
    local blob_size copy
    blob_size="$(tr -d '\r' < "$serial_log" | sed -n 's/^workspace-os bootloader: device tree at 0x[0-9a-f]* (\([0-9]*\) bytes).*$/\1/p')"
    copy="$(tr -d '\r' < "$serial_log" | sed -n 's/^workspace-os kernel: reserved device tree \(0x[0-9a-f]*\)\.\.\(0x[0-9a-f]*\)$/\1 \2/p')"
    [[ -n "$blob_size" && "$(echo "$copy" | wc -l)" -eq 1 && -n "$copy" ]] || fail "device tree copy not reported exactly once"
    read -r copy_start copy_end <<< "$copy"
    [[ $((copy_end - copy_start)) -eq "$blob_size" ]] \
        || fail "kernel reserves $((copy_end - copy_start)) bytes of device tree, the bootloader copied $blob_size"

    # The memory line comes from the bootloader's map: most of the machine's RAM.
    local reported
    reported="$(tr -d '\r' < "$serial_log" | sed -n 's/^workspace-os kernel: memory \([0-9]*\) MiB at 0x40000000$/\1/p')"
    [[ "$(echo "$reported" | wc -l)" -eq 1 && -n "$reported" ]] || fail "memory report not found exactly once"
    [[ "$reported" -le "$mib" && "$reported" -ge $((mib * 9 / 10)) ]] \
        || fail "kernel reports $reported MiB of memory, expected most of ${mib} MiB"

    counts="$(tr -d '\r' < "$serial_log" | sed -n 's/^workspace-os kernel: frames: \([0-9]*\) in memory map, \([0-9]*\) reserved, \([0-9]*\) free$/\1 \2 \3/p')"
    [[ -n "$counts" ]] || fail "frame map report has an unexpected format"
    read -r total reserved free <<< "$counts"
    [[ "$total" -le $((mib * 1024 * 1024 / FRAME_SIZE)) && "$total" -ge $((mib * 1024 * 1024 / FRAME_SIZE * 9 / 10)) ]] \
        || fail "frame map holds $total frames, expected most of ${mib} MiB"
    [[ $((reserved + free)) -eq "$total" ]] || fail "$reserved reserved and $free free frames do not add up to $total"
    [[ "$free" -gt 0 ]] || fail "no free frames"

    # The memory map the kernel printed is sorted and disjoint, holds the kernel
    # image as one entry exactly where the linker put it, and the allocator
    # follows it: RAM is everything but firmware regions, and what is not usable
    # (the kernel, the bootloader's memory, ACPI tables) is reserved.
    local start end kind previous_end=0 ram_frames=0 held_frames=0 kernel_entries=0 entries=0
    local kernel_end=$(( (image_end_address + FRAME_SIZE - 1) / FRAME_SIZE * FRAME_SIZE ))
    while read -r start end kind; do
        entries=$((entries + 1))
        [[ $((start)) -ge "$previous_end" && $((end)) -gt $((start)) ]] || fail "memory map entry $start..$end is out of order or empty"
        previous_end=$((end))
        if [[ "$kind" == kernel ]]; then
            kernel_entries=$((kernel_entries + 1))
            [[ $((start)) -eq "$image_start_address" && $((end)) -eq "$kernel_end" ]] \
                || fail "kernel entry $start..$end is not the image $(printf '%#x..%#x' "$image_start_address" "$kernel_end")"
        fi
        case "$kind" in
            usable | kernel | bootloader-reclaimable | acpi-reclaimable)
                ram_frames=$((ram_frames + (end - start) / FRAME_SIZE))
                if [[ "$kind" != usable ]]; then
                    held_frames=$((held_frames + (end - start) / FRAME_SIZE))
                fi
                ;;
        esac
    done < <(tr -d '\r' < "$serial_log" | sed -n 's/^workspace-os kernel: memory map: \(0x[0-9a-f]*\)\.\.\(0x[0-9a-f]*\) \(.*\)$/\1 \2 \3/p')
    [[ "$entries" -gt 0 && "$kernel_entries" -eq 1 ]] || fail "memory map has $entries entries and $kernel_entries kernel entries"
    [[ "$total" -eq "$ram_frames" ]] || fail "allocator map holds $total frames but the memory map has $ram_frames of RAM"
    [[ "$reserved" -eq "$held_frames" ]] \
        || fail "$reserved frames reserved, but the kernel and the bootloader's memory in the map take $held_frames"

    # Normal cacheable memory for the kernel and Device memory for the UART:
    # the translation the bootloader installed, as the hardware reports it.
    # The kernel runs on the bootloader's own tables, not the firmware's (the
    # root is the pool the bootloader printed), and what the tables map holds up
    # at the kernel, the UART and the boot info.
    local tables
    tables="$(tr -d '\r' < "$serial_log" | sed -n 's/^workspace-os bootloader: page tables at \(0x[0-9a-f]*\)$/\1/p')"
    [[ -n "$tables" ]] || fail "bootloader did not report its page tables"
    expect_once "workspace-os kernel: mapping: kernel image 0xff, uart 0x00, boot info 0xff, tables at $tables" "memory types and root of the bootloader's map"
    expect_once "workspace-os kernel: frames: exhausted after $free allocations, next request refused" "exhaustion report"
    expect_once "workspace-os kernel: frames: returned $free, $free free again" "return of every frame"
    expect_once "workspace-os kernel: frames: double free refused" "double free refusal"

    BOOT_TIMEOUT_SECONDS=10
    stop_qemu
    echo "PASS ($boot_name): kernel booted through the bootloader and stayed up"
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
# The boot code is first in the image, so the reserved image must begin at the
# entry point: a start symbol moved past it would leave the first frame free.
if [[ "$image_start_address" -ne $((entry)) ]]; then
    fail "__image_start is not the entry point $entry"
fi
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
uefi_firmware="$(find_uefi_firmware)"
disk_image="$work_dir/disk.img"
./scripts/build-disk-image.sh "$disk_image" > /dev/null
check_uefi_boot uefi 256 1
check_uefi_boot uefi-768 768 1
check_uefi_boot uefi-el2 256 2 -machine virtualization=on
check_panic unhandled-class "$class_elf" "SVC (AArch64)"
check_panic unhandled-vector "$vector_elf" "synchronous, current EL with SP_EL0"
check_panic compiler-trap "$trap_elf" "BRK (AArch64) (class 0x3c, iss 0x0000001"
