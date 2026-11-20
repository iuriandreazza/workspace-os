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
#   uefi      256 MiB, the default; nobody presses a key, so the menu's
#             countdown must start the default entry, Normal (boot mode 0)
#   uefi-768  768 MiB: the kernel sizes memory from the bootloader's map, so
#             the amount it reports follows
#   uefi-acpi-only  ACPI left on: no device tree is published, so the
#             bootloader must refuse and the kernel must not start. The refusal
#             shows the ACPI root pointer, the only non-zero `rsdp` a boot here
#             can show: QEMU's firmware never offers a device tree and ACPI
#             together (tried: acpi=on with -dtb, acpi=auto, gic-version=3)
#   uefi-el2-high  EL2 with 3.5 GiB: RAM and the page tables lie above 4 GiB,
#             which only works if the physical address size reaches EL1's
#             translation registers
#   uefi-no-kernel  the partition holds no `\KERNEL.ELF`: the bootloader must
#             say that file is missing
#   uefi-retry  the first run fails after loading the kernel, the second boots
#   uefi-load-at-zero  a kernel linked at address 0
#   uefi-load-beyond-ram  a kernel linked past the end of RAM
#   uefi-load-busy  a kernel linked where the firmware keeps its own memory
#             (the case of VirtualBox with 1024 MiB, which reserves
#             0x40200000): the refusal names the range
#   uefi-unknown-version  a bootloader that hands over boot info version 2.0:
#             the kernel must refuse it on the console the header names
#   uefi-el2  the firmware runs the bootloader at EL2, which the bootloader
#             must leave for EL1 itself before the kernel starts
#
# The boot menu is read and driven over the serial console (ADR 0007, K2c):
# the log is the console's output, and a script sends keys to its input once
# the menu has been drawn. Each entry must reach the kernel with its own boot
# mode (ADR 0013), and the kernel must report the same mode it was given:
#
#   uefi-recovery         the key `2` starts Recovery (boot mode 1)
#   uefi-recovery-arrows  the Down arrow selects Recovery and stops the
#                         countdown: the kernel must not start by itself while
#                         the menu waits, then Enter starts it (boot mode 1)
#   uefi-768, uefi-el2-high  Enter starts Normal (boot mode 0)
#   uefi-el2              the key `1` starts Normal (boot mode 0)
#
# Every normal boot also checks what ADR 0026 adds: the audit of the live
# tables finds each leaf mapping its own frame with its own memory type, the
# kernel reads the boot info, the memory map and the device tree again after
# the switch, through the lower window, and gets the same digest, and the
# permission-indirection registers are as expected. Two boots run on `-cpu max`,
# which has FEAT_S1PIE, directly and through the bootloader; two more use a
# kernel that sets `TCR2_EL1.PIE` just before the code that must clear it.
#
# Other kernels (K2d, ADR 0025): the bootloader starts Linux and the EFI loader
# of another system from the entries of `\WSOS.CFG` on the partition. The
# Linux kernel is a pinned Alpine release fetched and checked by
# scripts/fetch-test-kernel.sh; the gate fails if it cannot be had.
#
#   uefi-linux        Linux with a command line and no initrd: the kernel
#                     prints its first lines, the command line it received,
#                     and nothing about an initrd
#   uefi-linux-initrd Linux with an initrd of a known size: the kernel frees
#                     exactly that many KiB of it, and unpacks it without error
#   uefi-others       every refusal in turn (a missing or invalid Linux image
#                     or loader, a directory, an initrd that is not there), each ending in
#                     its own message and back at the menu with no countdown;
#                     then a chainloaded application that starts and returns,
#                     and our kernel from the same menu afterwards
#   uefi-bad-config   a configuration file with an error: said, the built-in
#                     entries only, and the default entry still boots
#   uefi-with-config  a valid file: the built-in entries stay first, and the
#                     default entry still boots by the timeout
#
# Three more boots use kernels built to raise an exception nothing resumes
# from, and must end in a kernel panic that names it:
#
#   unhandled-class   a synchronous exception of a class with no handler
#   unhandled-vector  an exception on a vector entry that is not handled
#   compiler-trap     `brk #0x1`, which the compiler emits for `abort`, must
#                     not be resumed like the kernel's own breakpoint
set -euo pipefail

readonly EXPECTED_BANNER="booted on aarch64 in EL1"
readonly DEFAULT_MEMORY_MIB=128
readonly RAM_BASE="0x40000000"
readonly UART_REPORT="uart pl011 at 0x9000000"
readonly BREAKPOINT_REPORT="exception: breakpoint #0x800 at"
readonly RESUME_MARKER="resumed after breakpoint"
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

# plain_log: the serial log without carriage returns and colour codes, for the
# checks that read a line as a whole. The text of a message is never split by a
# colour code, so the plain `grep -F` checks read the log as it is.
plain_log() {
    tr -d '\r' < "$serial_log" | LC_ALL=C sed $'s/\033\\[[0-9;]*m//g'
}

cd "$(dirname "$0")/.."
# The menu's countdown, read from where the bootloader defines it, so that the
# test cannot disagree with it.
AUTOBOOT_SECONDS="$(sed -n 's/^pub const AUTOBOOT_SECONDS: u32 = \([0-9][0-9]*\);$/\1/p' bootinfo/src/menu.rs)"
readonly AUTOBOOT_SECONDS
[[ -n "$AUTOBOOT_SECONDS" ]] || { echo "boot smoke test: AUTOBOOT_SECONDS not found in bootinfo/src/menu.rs" >&2; exit 1; }
# Written by the bootloader after it drew the menu and dropped any key the
# firmware buffered, so a key typed after this line is read by the menu.
readonly MENU_READY="autoboot in $AUTOBOOT_SECONDS s, starting Normal"
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

# console_socket: where the serial console takes keys, for the boots that drive
# the boot menu. A socket short enough for the platform's limit (about 100
# bytes) is why it lives in the work directory, under a short name.
console_socket() {
    echo "$work_dir/$1.sock"
}

# start_qemu NAME [--keys] QEMU_ARGS...: the serial console is written to the
# log; with `--keys` it also reads from `console_socket NAME`, and the output
# still lands in the log even while nobody is connected.
start_qemu() {
    boot_name="$1"
    shift
    serial_log="$work_dir/$boot_name.log"
    : > "$serial_log"
    local serial=(-serial "file:$serial_log")
    if [[ "${1:-}" == "--keys" ]]; then
        shift
        local socket
        socket="$(console_socket "$boot_name")"
        # QEMU refuses a longer path, and its option string cannot hold a comma.
        if [[ ${#socket} -ge 100 || "$socket" == *,* ]]; then
            fail "the console socket path '$socket' is too long or holds a comma: set TMPDIR to a short directory"
        fi
        serial=(-chardev "socket,id=console,path=$(console_socket "$boot_name"),server=on,wait=off,logfile=$serial_log"
            -serial chardev:console)
    fi

    # QEMU's own messages are kept aside and only shown when the boot fails.
    qemu-system-aarch64 "${QEMU_MACHINE[@]}" "${serial[@]}" "$@" \
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
# What the kernel must say about the permission-indirection registers: QEMU's
# `cortex-a72` has none, `-cpu max` has them and the kernel clears them.
readonly EXTENSIONS_ABSENT="mmu: permission indirection absent, TCR2_EL1 absent, SCTLR2_EL1 absent"
readonly EXTENSIONS_CLEARED="mmu: permission indirection present, TCR2_EL1 0x0, SCTLR2_EL1 0x0"
expected_extensions="$EXTENSIONS_ABSENT"

# check_frames MEMORY_MIB: the frame allocator's report is coherent with the
# amount of RAM and the layout the linker chose.
check_frames() {
    local mib="$1" counts total reserved free
    expect_once "reserved kernel image $image_range" "kernel image reservation"
    expect_once "reserved boot stack $stack_range" "boot stack reservation"
    expect_once "reserved exception stack $exception_range" "exception stack reservation"
    expect_once "in memory map" "frame map report"
    counts="$(plain_log | sed -n 's/^  kernel  .\{4\}  frames: \([0-9]*\) in memory map, \([0-9]*\) reserved, \([0-9]*\) free$/\1 \2 \3/p')"
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
    blob="$(plain_log | sed -n 's/^  kernel  .\{4\}  reserved device tree \(0x[0-9a-f]*\)\.\.\(0x[0-9a-f]*\)$/\1 \2/p')"
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
    expect_once "reserved by the device tree: $extra_reserved_ranges regions" \
        "device tree reservation count"

    expect_once "frames: exhausted after $free allocations, next request refused" \
        "exhaustion report"
    expect_once "frames: returned $free, $free free again" "return of every frame"
    expect_once "frames: double free refused" "double free refusal"

    local reused ram_end
    reused="$(plain_log | sed -n 's/^  kernel  .\{4\}  frames: reused \(0x[0-9a-f]*\) after freeing it$/\1/p')"
    [[ "$(echo "$reused" | wc -l)" -eq 1 && -n "$reused" ]] || fail "frame reuse not reported exactly once"
    ram_end=$((RAM_BASE + mib * 1024 * 1024))
    [[ "$reused" -ge "$RAM_BASE" && "$reused" -lt "$ram_end" && $((reused % FRAME_SIZE)) -eq 0 ]] \
        || fail "reused frame $reused is not a frame of RAM"
}

# check_mmu [MEMORY_MIB]: the kernel walked the page tables the processor uses
# and reported them (ADR 0019). The upper half must hold the image section by
# section with the sizes the linker gave them, no page may be writable and
# executable, and the lower half must hold the UART and no code. With
# MEMORY_MIB (a machine with one region of RAM) the lower half must also hold
# exactly that RAM less the image and its guard page.
check_mmu() {
    local upper lower text read_only read_write lower_rw device lower_text
    expect_once "mmu: write xor execute holds: no page is writable and executable, WXN is set" \
        "write xor execute report"
    upper="$(plain_log | sed -n 's/^  kernel  .\{4\}  mmu: upper half at 0x[0-9a-f]*: text \(0x[0-9a-f]*\) bytes, read only \(0x[0-9a-f]*\), read write \(0x[0-9a-f]*\)$/\1 \2 \3/p')"
    [[ "$(echo "$upper" | wc -l)" -eq 1 && -n "$upper" ]] || fail "upper half report not found exactly once"
    read -r text read_only read_write <<< "$upper"
    [[ $((text)) -eq "$text_bytes" && $((read_only)) -eq "$rodata_bytes" \
        && $((read_write)) -eq $((data_bytes + stack_bytes + exception_stack_bytes)) ]] \
        || fail "upper half maps text $text, read only $read_only, read write $read_write; the linker placed $text_bytes, $rodata_bytes and $((data_bytes + stack_bytes + exception_stack_bytes)) bytes"
    # The audit also checked every leaf's output address, memory type and
    # section (ADR 0026), and the registers the boot code clears.
    expect_once "mmu: every leaf maps its own frame with its own memory type" "audit of addresses and memory types"
    expect_once "$expected_extensions" "permission indirection registers"
    check_handoff_read_again
    lower="$(plain_log | sed -n 's/^  kernel  .\{4\}  mmu: lower half at 0x[0-9a-f]*: read write \(0x[0-9a-f]*\) bytes, device \(0x[0-9a-f]*\), text \(0x[0-9a-f]*\)$/\1 \2 \3/p')"
    [[ "$(echo "$lower" | wc -l)" -eq 1 && -n "$lower" ]] || fail "lower half report not found exactly once"
    read -r lower_rw device lower_text <<< "$lower"
    [[ $((device)) -eq $FRAME_SIZE && $((lower_text)) -eq 0 ]] \
        || fail "lower half maps $device bytes of devices and $lower_text of code, expected one page and none"
    if [[ -n "${1:-}" ]]; then
        [[ $((lower_rw)) -eq $(($1 * 1024 * 1024 - image_span_bytes)) ]] \
            || fail "lower half maps $lower_rw bytes of RAM, expected $(($1 * 1024 * 1024 - image_span_bytes))"
    fi
}

# check_handoff_read_again: the kernel read the boot info, the memory map and the
# device tree again after the switch, through the lower window (ADR 0026), and
# got what it had read before. The digest must cover at least the device tree
# blob the kernel reserved, or it would not show a window that lost it.
check_handoff_read_again() {
    local line bytes blob blob_start blob_end
    expect_once "handoff: read again after the switch, " "read of the hand-over after the switch"
    line="$(plain_log | sed -n 's/^  kernel  .\{4\}  handoff: read again after the switch, \([0-9]*\) bytes, digest 0x[0-9a-f]*, as before it$/\1/p')"
    bytes="$line"
    [[ -n "$bytes" ]] || fail "the read of the hand-over after the switch has an unexpected format"
    blob="$(plain_log | sed -n 's/^  kernel  .\{4\}  reserved device tree \(0x[0-9a-f]*\)\.\.\(0x[0-9a-f]*\)$/\1 \2/p')"
    [[ -n "$blob" ]] || fail "the device tree reservation was not reported, so the digest cannot be checked against it"
    read -r blob_start blob_end <<< "$blob"
    [[ "$bytes" -gt $((blob_end - blob_start)) ]] \
        || fail "the hand-over digest covers $bytes bytes, fewer than the $((blob_end - blob_start)) of the device tree"
}

# check_boot NAME QEMU_IMAGE_ARGS...
check_boot() {
    local expected_memory
    expected_memory="memory $(memory_mib_of "$@") MiB at $RAM_BASE"
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
    check_mmu "$(memory_mib_of "$@")"

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

# check_fault NAME CLASS_NAME CLASS ISS PC_RANGE FAR QEMU_ARGS...: a boot built
# to break write-xor-execute (ADR 0019) must end in a kernel panic that reports
# the exception exactly: its class, the low bits of its syndrome (ISS, as a
# regular expression), the address of the faulting instruction (a number, or
# START..END it must lie in) and the fault address (the same). A boot where the violation
# is allowed prints `selftest:` and panics with another message, so it fails.
# FAULT_ALSO, when set, is a line the kernel must print after the report, and
# FAULT_ABSENT one it must not print anywhere.
FAULT_ALSO=""
FAULT_ABSENT=""
# in_range VALUE EXPECTED: VALUE is the number EXPECTED, or lies in START..END.
in_range() {
    if [[ "$2" == *..* ]]; then
        [[ $(($1)) -ge $((${2%%..*})) && $(($1)) -lt $((${2##*..})) ]]
    else
        [[ $(($1)) -eq $(($2)) ]]
    fi
}

check_fault() {
    local name="$1" class_name="$2" class="$3" iss="$4" pc_range="$5" far="$6" line pc fault_address
    shift 6
    start_qemu "$name" "$@"
    for _ in $(seq 1 $((BOOT_TIMEOUT_SECONDS * 10))); do
        grep -qF "fault address" "$serial_log" && break
        grep -qF "selftest: " "$serial_log" && grep -qF "$PANIC_MARKER" "$serial_log" && break
        qemu_is_running || fail "QEMU exited without the expected fault"
        sleep 0.1
    done
    line="$(plain_log | grep -F "unhandled synchronous exception" | head -1 || true)"
    [[ -n "$line" ]] || fail "no unhandled synchronous exception in the log: $(plain_log | grep -F -A1 "$PANIC_MARKER" | head -2 | tr '\n' ' ' || true)"
    [[ "$line" =~ ^unhandled\ synchronous\ exception:\ ${class_name}\ \(class\ ${class},\ iss\ ${iss},\ esr\ 0x[0-9a-f]+\)\ at\ (0x[0-9a-f]+),\ fault\ address\ (0x[0-9a-f]+)$ ]] \
        || fail "the fault is not '$class_name (class $class, iss $iss) ... fault address $far': $line"
    pc="${BASH_REMATCH[1]}"
    fault_address="${BASH_REMATCH[2]}"
    in_range "$fault_address" "$far" || fail "fault address $fault_address, expected $far"
    in_range "$pc" "$pc_range" || fail "the faulting instruction $pc is not $pc_range"
    if [[ -n "$FAULT_ALSO" ]]; then
        # Printed after the report, by the panic handler of the build.
        for _ in $(seq 1 $((BOOT_TIMEOUT_SECONDS * 10))); do
            grep -qF "$FAULT_ALSO" "$serial_log" && break
            qemu_is_running || break
            sleep 0.1
        done
        grep -qF "$FAULT_ALSO" "$serial_log" || fail "after the fault the kernel did not print '$FAULT_ALSO'"
    fi
    if [[ -n "$FAULT_ABSENT" ]] && grep -qF "$FAULT_ABSENT" "$serial_log"; then
        fail "the log holds '$FAULT_ABSENT'"
    fi
    # The tables had passed their audit before the violation was tried.
    expect_once "mmu: write xor execute holds" "write xor execute report before the violation"
    grep -qF "selftest: " "$serial_log" || fail "the violation was never tried"
    stop_qemu
    echo "PASS ($boot_name): $class_name at $fault_address ended in a kernel panic"
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

# send_key NAME BYTES: types BYTES on the serial console of the running boot.
# A client connects for each call: the console stays up between them.
send_key() {
    python3 -I -c '
import socket, sys
console = socket.socket(socket.AF_UNIX)
console.connect(sys.argv[1])
console.sendall(sys.argv[2].encode("latin-1"))
' "$(console_socket "$1")" "$2" || fail "could not type on the serial console"
}

# drive_menu NAME KEYS: waits for the menu, then types KEYS, a comma-separated
# list of `enter`, `down`, `up`, a single character, or `hold`: wait longer than
# the countdown and check that nothing started by itself.
drive_menu() {
    local key keys
    wait_for_line "$MENU_READY"
    IFS=, read -ra keys <<< "$2"
    for key in "${keys[@]}"; do
        case "$key" in
            enter) send_key "$1" $'\r' ;;
            down) send_key "$1" $'\033[B' ;;
            up) send_key "$1" $'\033[A' ;;
            hold)
                sleep $((AUTOBOOT_SECONDS + 2))
                # The plain log: a colour code sits between the tag and the text.
                if grep -qF "booted on aarch64" "$serial_log" || log_has "(boot mode "; then
                    fail "the boot continued by itself while the menu waited for a key"
                fi
                ;;
            ?) send_key "$1" "$key" ;;
            *) fail "unknown key '$key' in the menu script" ;;
        esac
    done
}

# log_has TEXT: the plain log holds TEXT. The log is captured first: with
# `pipefail`, `grep -q` ending a pipeline early could make it fail.
log_has() {
    local log
    log="$(plain_log)"
    grep -qF -- "$1" <<< "$log"
}

# now_ms: milliseconds, for the countdown's length. The wall clock and not
# `time.monotonic_ns`: Apple's Python 3.9 on macOS 27 returns a value that
# restarts with every process, which made every reading differ by a few ms.
now_ms() {
    python3 -I -c 'import time; print(time.time_ns() // 1000000)'
}

# expect_menu_entries: the menu on the console lists every entry, numbered.
expect_menu_entries() {
    local entry
    for entry in "1  Normal" "2  Recovery"; do
        if ! log_has "$entry"; then
            fail "the boot menu does not list '$entry'"
        fi
    done
}

# check_uefi_boot NAME MEMORY_MIB BOOTLOADER_EL MODE KEYS [QEMU_ARGS...]: boots
# the disk image through the UEFI firmware and the bootloader. MODE is the
# entry that must be started, `normal` or `recovery`; KEYS is what is typed on
# the menu (see `drive_menu`), empty when nobody answers and the countdown
# starts the default entry. The firmware publishes a device tree only with
# ACPI off (ADR 0013, "What is not yet known"), hence `acpi=off`.
check_uefi_boot() {
    local mib="$2" bootloader_el="$3" counts total reserved free copy_start copy_end
    local mode_name="$4" keys="$5" mode_number label other_label reason
    case "$mode_name" in
        normal) mode_number=0 label=Normal other_label=Recovery ;;
        recovery) mode_number=1 label=Recovery other_label=Normal ;;
        *) fail "unknown boot mode '$mode_name' in the test" ;;
    esac
    reason="chosen on the keyboard"
    [[ -n "$keys" ]] || reason="chosen by the timeout"
    BOOT_TIMEOUT_SECONDS=$UEFI_BOOT_TIMEOUT_SECONDS
    local console=""
    [[ -z "$keys" ]] || console="--keys"
    # Unquoted on purpose: an empty array breaks `set -u` on bash before 4.4.
    # shellcheck disable=SC2086
    start_qemu "$1" $console -machine virt,acpi=off -m "${mib}M" -bios "$uefi_firmware" \
        -drive "file=$disk_image,format=raw,if=virtio" "${@:6}"

    if [[ -n "$keys" ]]; then
        drive_menu "$1" "$keys"
    else
        # Nobody answers: the default entry starts, and only after the whole
        # countdown (less a second for the polling here), not at once.
        local menu_drawn menu_done
        wait_for_line "$MENU_READY"
        menu_drawn="$(now_ms)"
        wait_for_line "starting Normal (boot mode"
        menu_done="$(now_ms)"
        [[ $((menu_done - menu_drawn)) -ge $(((AUTOBOOT_SECONDS - 1) * 1000)) ]] \
            || fail "the default entry started $((menu_done - menu_drawn)) ms after the menu, before the ${AUTOBOOT_SECONDS} s countdown ended"
    fi
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
    expect_once "started at EL$bootloader_el" "bootloader's exception level"
    expect_once "kernel loaded at $(hex "$(physical_symbol __image_start)")..$(hex "$(physical_symbol __exception_stack_top)"), entry $(hex "$entry")" \
        "bootloader's report of the kernel placement"
    expect_once "$EXPECTED_BANNER" "boot banner"
    expect_once "$UART_REPORT" "UART report"
    expect_once "$BREAKPOINT_REPORT" "breakpoint report"
    expect_once "$RESUME_MARKER" "resume after the breakpoint"
    # The origin of the entry is visible on the serial console.
    expect_once "entry: bootloader, boot info 1.0, mode $mode_number ($mode_name)," "entry through the bootloader"
    if grep -qF "entry: direct load" "$serial_log"; then
        fail "kernel reports a direct load after a boot through the bootloader"
    fi
    # The menu offered both entries, the one asked for was started and the
    # other was not: the boot mode the kernel reports is the entry's, not a
    # constant. The kernel's line above is the other half of this check.
    expect_menu_entries
    expect_once "starting $label (boot mode $mode_number, $mode_name), $reason" "the bootloader's choice of the $label entry"
    if grep -qF "starting $other_label (boot mode" "$serial_log"; then
        fail "the bootloader started the $other_label entry as well"
    fi
    # The processor identity, the tables and the registers the ADR fixes to zero:
    # the boot processor of QEMU `virt` is 0, ACPI is off, the system table is set.
    expect_once "handoff: boot cpu 0x0, rsdp 0x0, efi system table 0x" "handoff fields"
    if grep -qF "efi system table 0x0," "$serial_log"; then
        fail "boot info carries no EFI system table"
    fi
    expect_once ", x2|x3 0" "x2 and x3 at entry"
    expect_once "reserved kernel image $image_range" "kernel image reservation"
    expect_once "reserved boot stack $stack_range" "boot stack reservation"
    expect_once "reserved exception stack $exception_range" "exception stack reservation"

    # The kernel found an intact copy of the device tree, as big as the firmware's.
    local blob_size copy
    blob_size="$(plain_log | sed -n 's/.*[^ ] *device tree at 0x[0-9a-f]* (\([0-9]*\) bytes).*/\1/p')"
    copy="$(plain_log | sed -n 's/^  kernel  .\{4\}  reserved device tree \(0x[0-9a-f]*\)\.\.\(0x[0-9a-f]*\)$/\1 \2/p')"
    [[ -n "$blob_size" && "$(echo "$copy" | wc -l)" -eq 1 && -n "$copy" ]] || fail "device tree copy not reported exactly once"
    read -r copy_start copy_end <<< "$copy"
    [[ $((copy_end - copy_start)) -eq "$blob_size" ]] \
        || fail "kernel reserves $((copy_end - copy_start)) bytes of device tree, the bootloader copied $blob_size"

    # The memory line comes from the bootloader's map: most of the machine's RAM.
    local reported
    reported="$(plain_log | sed -n 's/^  kernel  .\{4\}  memory \([0-9]*\) MiB at 0x40000000$/\1/p')"
    [[ "$(echo "$reported" | wc -l)" -eq 1 && -n "$reported" ]] || fail "memory report not found exactly once"
    [[ "$reported" -le "$mib" && "$reported" -ge $((mib * 9 / 10)) ]] \
        || fail "kernel reports $reported MiB of memory, expected most of ${mib} MiB"

    counts="$(plain_log | sed -n 's/^  kernel  .\{4\}  frames: \([0-9]*\) in memory map, \([0-9]*\) reserved, \([0-9]*\) free$/\1 \2 \3/p')"
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
    done < <(plain_log | sed -n 's/^  kernel  .\{4\}  memory map: \(0x[0-9a-f]*\)\.\.\(0x[0-9a-f]*\) \(.*\)$/\1 \2 \3/p')
    [[ "$entries" -gt 0 && "$kernel_entries" -eq 1 ]] || fail "memory map has $entries entries and $kernel_entries kernel entries"
    [[ "$total" -eq "$ram_frames" ]] || fail "allocator map holds $total frames but the memory map has $ram_frames of RAM"
    [[ "$reserved" -eq "$held_frames" ]] \
        || fail "$reserved frames reserved, but the kernel and the bootloader's memory in the map take $held_frames"

    # Normal cacheable memory for the kernel and Device memory for the UART:
    # the translation the bootloader installed, as the hardware reports it.
    # The kernel runs on the bootloader's own tables, not the firmware's (the
    # root is the pool the bootloader printed), and what the tables map holds up
    # at the kernel, the UART and the boot info.
    # The probe prints -1 as 0xffffffff when it finds nothing, so the line ends here.
    [[ "$(plain_log | grep -cE 'mapping: runtime services 0xff$')" -eq 1 ]] \
        || fail "memory type of the runtime services regions is not Normal write-back (0xff)"
    # SCTLR_EL1 as the boot code left it, before the kernel's own tables
    # replace the bootloader's: MMU (bit 0), data cache (2) and instruction
    # cache (12) on as the bootloader had them, alignment check (1) off, and
    # write-implies-execute-never (19) turned on by the boot code (ADR 0019).
    local sctlr
    sctlr="$(plain_log | sed -n 's/^  kernel  .\{4\}  sctlr_el1 \(0x[0-9a-f]*\)$/\1/p')"
    [[ -n "$sctlr" ]] || fail "SCTLR_EL1 not reported"
    [[ $(((sctlr >> 0) & 1)) -eq 1 && $(((sctlr >> 2) & 1)) -eq 1 && $(((sctlr >> 12) & 1)) -eq 1 \
        && $(((sctlr >> 1) & 1)) -eq 0 && $(((sctlr >> 19) & 1)) -eq 1 ]] \
        || fail "SCTLR_EL1 is $sctlr: expected MMU, D-cache, I-cache and WXN on, alignment checks off"
    local tables
    tables="$(plain_log | sed -n 's/.*page tables at \(0x[0-9a-f]*\).*/\1/p')"
    [[ -n "$tables" ]] || fail "bootloader did not report its page tables"
    # The tables are memory the bootloader owns and the kernel may reclaim later.
    local owned=0 table_start table_end table_kind
    while read -r table_start table_end table_kind; do
        if [[ "$table_kind" == bootloader-reclaimable \
            && $((table_start)) -le $((tables)) && $((tables)) -lt $((table_end)) ]]; then
            owned=1
        fi
    done < <(plain_log | sed -n 's/^  kernel  .\{4\}  memory map: \(0x[0-9a-f]*\)\.\.\(0x[0-9a-f]*\) \(.*\)$/\1 \2 \3/p')
    [[ "$owned" -eq 1 ]] || fail "the page tables at $tables are not in a bootloader-reclaimable entry of the memory map"
    expect_once "mapping: kernel image 0xff, uart 0x00, boot info 0xff, tables at $tables" "memory types and root of the bootloader's map"
    expect_once "frames: exhausted after $free allocations, next request refused" "exhaustion report"
    expect_once "frames: returned $free, $free free again" "return of every frame"
    expect_once "frames: double free refused" "double free refusal"
    check_mmu

    BOOT_TIMEOUT_SECONDS=10
    stop_qemu
    echo "PASS ($boot_name): kernel booted through the bootloader and stayed up"
}

# check_uefi_refusal NAME IMAGE MESSAGE QEMU_MACHINE_ARGS...: the bootloader must
# stop the boot with MESSAGE on the serial console and the kernel must not start.
check_uefi_refusal() {
    local image="$2" message="$3"
    BOOT_TIMEOUT_SECONDS=$UEFI_BOOT_TIMEOUT_SECONDS
    start_qemu "$1" "${@:4}" -m 256M -bios "$uefi_firmware" \
        -drive "file=$image,format=raw,if=virtio"
    wait_for_line "$message"
    sleep "$SETTLE_SECONDS"
    if grep -qF "booted on aarch64" "$serial_log"; then
        fail "the kernel started although the bootloader refused: $message"
    fi
    BOOT_TIMEOUT_SECONDS=10
    stop_qemu
    echo "PASS ($boot_name): the bootloader refused with '$message'"
}

# check_uefi_retry NAME IMAGE: a bootloader whose first run fails after it
# loaded the kernel must give the load range back, so that the second run loads
# the kernel at the same address and the kernel boots.
check_uefi_retry() {
    BOOT_TIMEOUT_SECONDS=$UEFI_BOOT_TIMEOUT_SECONDS
    start_qemu "$1" -machine virt,acpi=off -m 256M -bios "$uefi_firmware" \
        -drive "file=$2,format=raw,if=virtio"
    wait_for_line "forced failure of the first run"
    wait_for_banner
    wait_for_line "$RESUME_MARKER"
    expect_once "$EXPECTED_BANNER" "boot banner"
    [[ "$(grep -cF "started at EL1" "$serial_log")" -eq 2 && "$(grep -cF "kernel loaded at 0x" "$serial_log")" -eq 1 ]] \
        || fail "expected two runs of the bootloader and one completed load"
    BOOT_TIMEOUT_SECONDS=10
    stop_qemu
    echo "PASS ($boot_name): the second run loaded the kernel again after the first failed"
}

# build_with_feature FEATURE DESTINATION: builds the kernel with a self-test
# feature and keeps the image, so the next build cannot replace it.
build_with_feature() {
    cargo build --quiet --target-dir "$TARGET_DIR" -p kernel --features "$1"
    cp "$KERNEL_ELF" "$2"
}

# llvm-nm and llvm-objcopy come with the `llvm-tools` rustup component.
llvm_tools="$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin"

# symbol_of ELF NAME: the address of a symbol in an ELF, as 0x...
symbol_of() {
    "$llvm_tools/llvm-nm" "$1" | awk -v name="$2" '$3 == name { print "0x" $1 }'
}

symbol_address() {
    symbol_of "$KERNEL_ELF" "$1"
}

# The kernel runs in the upper half (ADR 0019): the linker gives every symbol
# its virtual address, which is the physical one plus this offset. The number is
# read from where the tables are built, and the linker script must agree.
KERNEL_OFFSET="$(sed -n 's/^pub const KERNEL_OFFSET: u64 = \(0x[0-9a-f_]*\);$/\1/p' kernelvm/src/lib.rs | tr -d '_')"
readonly KERNEL_OFFSET
[[ -n "$KERNEL_OFFSET" ]] || fail "KERNEL_OFFSET not found in kernelvm/src/lib.rs"
[[ "$(( $(symbol_address KERNEL_OFFSET) ))" -eq $((KERNEL_OFFSET)) ]] \
    || fail "link.ld defines KERNEL_OFFSET as $(symbol_address KERNEL_OFFSET), kernelvm as $KERNEL_OFFSET"

# physical_symbol NAME: the physical address of a symbol, as a number.
physical_symbol() {
    echo $(( $(symbol_address "$1") - KERNEL_OFFSET ))
}

# The loaders enter at the ELF entry point and place segments by their physical
# address, so both must stay physical although the code is linked in the upper
# half: the entry is not the virtual `_start`, and it lies in the first segment.
entry="$(physical_symbol _start)"
[[ "$(( $(symbol_address _start_physical) ))" -eq "$entry" ]] || fail "_start_physical is not _start minus the offset"
elf_entry="$("$llvm_tools/llvm-readobj" --file-headers "$KERNEL_ELF" | sed -n 's/^ *Entry: \(0x[0-9A-Fa-f]*\)$/\1/p')"
[[ "$((elf_entry))" -eq "$entry" ]] || fail "the ELF entry point is $elf_entry, expected the physical address $(printf '%#x' "$entry")"
"$llvm_tools/llvm-objdump" -p "$KERNEL_ELF" | grep -qE "LOAD .* vaddr 0x$(printf '%x' $((entry + KERNEL_OFFSET))) paddr 0x$(printf '%016x' "$entry")" \
    || fail "no segment is linked at virtual $(printf '%#x' $((entry + KERNEL_OFFSET))) and loaded at physical $(printf '%#x' "$entry")"
image_start_address=$(( $(physical_symbol __image_start) ))
# The boot code is first in the image, so the reserved image must begin at the
# entry point: a start symbol moved past it would leave the first frame free.
if [[ "$image_start_address" -ne "$entry" ]]; then
    fail "__image_start is not the entry point $entry"
fi
image_end_address=$(( $(physical_symbol __exception_stack_top) ))
hex() { printf '0x%x' "$1"; }
image_range="$(hex "$(physical_symbol __image_start)")..$(hex "$(physical_symbol __stack_bottom)")"
stack_range="$(hex "$(physical_symbol __stack_bottom)")..$(hex "$(physical_symbol __stack_top)")"
# The guard page above the boot stack is reserved with the exception stack.
exception_range="$(hex "$(physical_symbol __stack_top)")..$(hex "$(physical_symbol __exception_stack_top)")"
# What the kernel's page tables must map, by section, from the linker's symbols.
text_bytes=$(( $(symbol_address __text_end) - $(symbol_address __image_start) ))
rodata_bytes=$(( $(symbol_address __rodata_end) - $(symbol_address __text_end) ))
data_bytes=$(( $(symbol_address __data_end) - $(symbol_address __rodata_end) ))
stack_bytes=$(( $(symbol_address __stack_top) - $(symbol_address __stack_bottom) ))
exception_stack_bytes=$(( $(symbol_address __exception_stack_top) - $(symbol_address __exception_stack_bottom) ))
# The probe the stack-overflow boot checks is the last thing in `.bss`, right
# below the guard page: only there does it see what a stack that ran past the
# guard page writes first.
[[ "$(symbol_address __stack_probe_end)" == "$(symbol_address __bss_end)" \
    && "$(symbol_address __bss_end)" == "$(symbol_address __data_end)" ]] \
    || fail "the .bss probe is not the last thing before the guard page"
# exceptions.s decides whether an exception is nested from the size of the
# exception stack, which only link.ld defines: the two must agree.
[[ "$(sed -n 's/^\.equ EXCEPTION_STACK_SIZE, \(0x[0-9a-f]*\)$/\1/p' kernel/src/arch/aarch64/exceptions.s)" ]] \
    && [[ $(( $(sed -n 's/^\.equ EXCEPTION_STACK_SIZE, \(0x[0-9a-f]*\)$/\1/p' kernel/src/arch/aarch64/exceptions.s) )) -eq "$exception_stack_bytes" ]] \
    || fail "exceptions.s and link.ld disagree on the size of the exception stack"
image_span_bytes=$(( $(symbol_address __exception_stack_top) - $(symbol_address __image_start) ))
bss_start="$(physical_symbol __bss_start)"
bss_end="$(physical_symbol __bss_end)"
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

# Every frame below the image reserved: the allocator then hands the page tables
# frames from the tail of the image's own 2 MiB, which the boot tables must map.
below_dtb="$work_dir/reserved-below-image.dtb"
python3 -I - "$dtb" "$below_dtb" <<'PY'
import struct, sys
source, target = sys.argv[1:3]
blob = open(source, "rb").read()
fields = list(struct.unpack(">10I", blob[:40]))
rsvmap, structure, strings, strings_size = fields[4], fields[2], fields[3], fields[8]
entry = struct.pack(">2Q", 0x40000000, 0x200000)
body = entry + blob[rsvmap:strings + strings_size]
fields[2], fields[3] = structure + len(entry), strings + len(entry)
total = (rsvmap + len(body) + 7) // 8 * 8
fields[1] = total
open(target, "wb").write((struct.pack(">10I", *fields) + blob[40:rsvmap] + body).ljust(total, b"\0"))
PY
start_qemu reserved-below-image -dtb "$below_dtb" -kernel "$KERNEL_ELF"
wait_for_line "$RESUME_MARKER"
expect_once "mmu: write xor execute holds" "write xor execute report with the frames below the image reserved"
# The test only means something if the allocator did give out frames from the
# tail of the image's 2 MiB: the first free frame is then above the stack.
reused="$(plain_log | sed -n 's/^  kernel  .\{4\}  frames: reused \(0x[0-9a-f]*\) after freeing it$/\1/p')"
[[ -n "$reused" && $((reused)) -ge "$image_end_address" && $((reused)) -lt $((image_start_address + 2 * 1024 * 1024)) ]] \
    || fail "the first free frame $reused is not in the tail of the image's 2 MiB (above $(printf '%#x' "$image_end_address"))"
stop_qemu
echo "PASS (reserved-below-image): page tables built in the image's own 2 MiB"
extra_reserved_frames=0
extra_reserved_ranges=0
uefi_firmware="$(find_uefi_firmware)"
# The cache or the network failing stops the gate before any boot that needs it, loudly: a green gate
# must have started Linux.
linux_kernel="$("$PWD/scripts/fetch-test-kernel.sh")"
# A bootloader built to hand over a boot info of an unknown version, kept before
# the plain one is built again.
cargo build --quiet --target-dir "$TARGET_DIR" -p bootloader --target aarch64-unknown-uefi \
    --features selftest-unknown-boot-info-version
unknown_version_image="$work_dir/unknown-version.img"
python3 -I scripts/make-disk-image.py "$unknown_version_image" \
    target/aarch64-unknown-uefi/debug/bootloader.efi "$KERNEL_ELF"
cargo build --quiet --target-dir "$TARGET_DIR" -p bootloader --target aarch64-unknown-uefi \
    --features selftest-retry-after-failure
retry_image="$work_dir/retry.img"
python3 -I scripts/make-disk-image.py "$retry_image" \
    target/aarch64-unknown-uefi/debug/bootloader.efi "$KERNEL_ELF"
disk_image="$work_dir/disk.img"
./scripts/build-disk-image.sh "$disk_image" > /dev/null
check_uefi_boot uefi 256 1 normal ""
check_uefi_boot uefi-768 768 1 normal enter
check_uefi_boot uefi-el2 256 2 normal 1 -machine virtualization=on
check_uefi_boot uefi-el2-high 3584 2 normal enter -machine virtualization=on
check_uefi_boot uefi-recovery 256 1 recovery 2
check_uefi_boot uefi-recovery-arrows 256 1 recovery down,hold,enter
check_uefi_retry uefi-retry "$retry_image"
check_uefi_refusal uefi-acpi-only "$disk_image" "the firmware offers ACPI only (rsdp 0x" -machine virt
# The root pointer is the one non-zero `rsdp` any boot here can show: QEMU's
# firmware never publishes ACPI together with a device tree.
acpi_rsdp="$(plain_log | sed -n 's/.*offers ACPI only (rsdp \(0x[0-9a-f]*\)).*/\1/p' | head -1)"
[[ -n "$acpi_rsdp" && "$acpi_rsdp" != 0x0 ]] || fail "the ACPI-only refusal shows no ACPI root pointer"

# The kernel must refuse a boot info of a major version it does not know, and
# say so on the console that header names.
check_uefi_refusal uefi-unknown-version "$unknown_version_image" \
    "unsupported boot info version 2.0" -machine virt,acpi=off

# The kernel file is not on the partition: the message names it.
no_kernel_image="$work_dir/no-kernel.img"
python3 -I scripts/make-disk-image.py "$no_kernel_image" \
    target/aarch64-unknown-uefi/debug/bootloader.efi > /dev/null
check_uefi_refusal uefi-no-kernel "$no_kernel_image" \
    '\KERNEL.ELF is not on the EFI system partition' -machine virt,acpi=off

# The kernel's load address is not free: first an address past the end of RAM,
# whose refusal prints the firmware's memory map, then the start of a firmware
# region taken from that map (the layout depends on the firmware, so it is read
# and not written down here).
relocated_image() {
    python3 -I scripts/relocate-elf.py "$KERNEL_ELF" "$work_dir/relocated.elf" "$1"
    python3 -I scripts/make-disk-image.py "$work_dir/relocated.img" \
        target/aarch64-unknown-uefi/debug/bootloader.efi "$work_dir/relocated.elf"
}
relocated_image 0
check_uefi_refusal uefi-load-at-zero "$work_dir/relocated.img" \
    "the kernel is linked at address 0" -machine virt,acpi=off
relocated_image 0x7ff00000
check_uefi_refusal uefi-load-beyond-ram "$work_dir/relocated.img" \
    "the kernel's load address is not free RAM" -machine virt,acpi=off
firmware_region="$(plain_log | sed -n 's/^  boot  info  memory map: \(0x[0-9a-f]*\)\.\.0x[0-9a-f]* RUNTIME_SERVICES_CODE$/\1/p' | head -1)"
span="$(plain_log | sed -n 's/.*the kernel needs 0x[0-9a-f]*\.\.\(0x[0-9a-f]*\) and.*/\1/p' | head -1)"
[[ -n "$firmware_region" && -n "$span" ]] || fail "the refusal printed no firmware region to use"
span=$((span - 0x7ff00000))
relocated_image "$firmware_region"
check_uefi_refusal uefi-load-busy "$work_dir/relocated.img" \
    "the kernel needs $firmware_region..$(printf '%#x' $((firmware_region + span))) and the firmware refused it" \
    -machine virt,acpi=off

# --- Other kernels (K2d) ---------------------------------------------------
LINUX_RELEASE="$(sed -n 's/^readonly KERNEL_RELEASE="\(.*\)"$/\1/p' scripts/fetch-test-kernel.sh)"
[[ -n "$LINUX_RELEASE" ]] || fail "KERNEL_RELEASE not found in scripts/fetch-test-kernel.sh"
readonly LINUX_CMDLINE="console=ttyAMA0 earlycon wsos_token=k2d-ok"
# Linux ends its boot with a panic (no root filesystem, or an /init that does
# not run): the end of what the gate reads.
readonly LINUX_END="Kernel panic - not syncing"
bootloader_efi="target/aarch64-unknown-uefi/debug/bootloader.efi"
chainprobe_efi="target/aarch64-unknown-uefi/debug/chainprobe.efi"

# plain_count TEXT: how many lines of the plain log hold TEXT.
plain_count() {
    local log
    log="$(plain_log)"
    grep -cF -- "$1" <<< "$log" || true
}

# wait_for_plain_count TEXT COUNT: TEXT is in the plain log COUNT times.
wait_for_plain_count() {
    for _ in $(seq 1 $((BOOT_TIMEOUT_SECONDS * 10))); do
        [[ "$(plain_count "$1")" -ge "$2" ]] && return 0
        qemu_is_running || fail "QEMU exited before '$1' was seen $2 times"
        sleep 0.1
    done
    fail "'$1' not seen $2 times within ${BOOT_TIMEOUT_SECONDS}s"
}

# expect_plain_once TEXT WHAT: the plain log holds TEXT in exactly one line.
expect_plain_once() {
    local count
    count="$(plain_count "$1")"
    [[ "$count" -eq 1 ]] || fail "$2: '$1' seen $count times, expected once"
}

# expect_plain_absent TEXT WHAT
expect_plain_absent() {
    log_has "$1" && fail "$2: '$1' is in the log"
    return 0
}

# start_other NAME IMAGE: boots IMAGE on the console that takes keys, and waits
# for the menu to be drawn with its countdown.
start_other() {
    # Linux is decompressed and booted under emulation, on a runner that may be
    # slower than a laptop: the wait is longer than for our own kernel.
    BOOT_TIMEOUT_SECONDS=$((UEFI_BOOT_TIMEOUT_SECONDS * 2))
    start_qemu "$1" --keys -machine virt,acpi=off -m 512M -bios "$uefi_firmware" \
        -drive "file=$2,format=raw,if=virtio"
    wait_for_line "$MENU_READY"
}

# make_image NAME CONFIG [make-disk-image.py arguments]: an image with our kernel
# and CONFIG as `\WSOS.CFG`; prints its path.
make_image() {
    local name="$1" config="$2"
    shift 2
    printf '%s\n' "$config" > "$work_dir/$name.cfg"
    python3 -I scripts/make-disk-image.py "$work_dir/$name.img" "$bootloader_efi" "$KERNEL_ELF" \
        --file "\\WSOS.CFG=$work_dir/$name.cfg" "$@"
    echo "$work_dir/$name.img"
}

# check_linux NAME IMAGE KEY LABEL [INITRD_BYTES]: the entry KEY, a Linux entry
# called LABEL, starts Linux; what the kernel prints shows the command line it
# was given and whether it got an initrd, and our own kernel never starts.
check_linux() {
    local key="$3" label="$4" initrd_bytes="${5:-}" cmdline
    start_other "$1" "$2"
    send_key "$1" "$key"
    wait_for_line "$LINUX_END"
    sleep "$SETTLE_SECONDS"
    expect_once "starting $label (Linux), chosen on the keyboard" "the bootloader's choice of $label"
    expect_plain_once "Linux version $LINUX_RELEASE" "Linux's first line"
    expect_plain_once "Machine model:" "Linux's machine line"
    # The command line is the load options the bootloader set: the whole line,
    # not a part of it.
    cmdline="$(plain_log | sed -n 's/^\[[ 0-9.]*\] Kernel command line: //p')"
    [[ "$cmdline" == "$LINUX_CMDLINE" ]] \
        || fail "Linux received the command line '$cmdline', expected '$LINUX_CMDLINE'"
    if [[ -n "$initrd_bytes" ]]; then
        # The kernel frees the whole pages of the initrd it was given, so the
        # figure follows the size of the file and of nothing else.
        expect_plain_once "Freeing initrd memory: $((initrd_bytes / 4096 * 4))K" "the initrd Linux received"
        expect_plain_absent "Initramfs unpacking failed" "the initrd Linux received"
        # The kernel found /init in what it unpacked, and tried to run it.
        expect_plain_absent "check access for rdinit=/init failed" "the initrd Linux received"
        expect_plain_once "Failed to execute /init" "the /init of the initrd"
    else
        expect_plain_once "check access for rdinit=/init failed" "a boot with no initrd"
        expect_plain_absent "Freeing initrd memory" "a boot with no initrd"
    fi
    expect_plain_absent "booted on aarch64" "a Linux boot"
    expect_plain_absent "leaving boot services, entering the kernel" "a Linux boot"
    BOOT_TIMEOUT_SECONDS=10
    stop_qemu
    echo "PASS ($boot_name): Linux started by the bootloader${initrd_bytes:+ with a ${initrd_bytes}-byte initrd}"
}

refusals=0
# expect_refusal KEY MESSAGE: choosing the entry KEY ends in MESSAGE and a menu
# that waits for a person (no countdown); nothing else started.
expect_refusal() {
    send_key "$boot_name" "$1"
    wait_for_line "$2"
    refusals=$((refusals + 1))
    wait_for_plain_count "back to the menu" "$refusals"
    wait_for_plain_count "autoboot stopped" "$refusals"
    expect_plain_once "$2" "the refusal of entry $1"
    expect_plain_absent "Linux version" "a refused Linux entry"
    expect_plain_absent "booted on aarch64" "a refused entry"
    expect_plain_absent "chainprobe: started" "a refused entry"
}

# check_others NAME IMAGE: the refusals one after the other on one boot, then
# the chainloaded application, then our own kernel from the same menu.
check_others() {
    start_other "$1" "$2"
    refusals=0
    expect_refusal 3 "cannot start Directory as Linux: cannot read Linux image \\EFI\\TEST: it is a directory"
    expect_refusal 4 "cannot start Not a PE: Linux image \\KERNEL.ELF is not a valid EFI image: not a PE image (no MZ header)"
    expect_refusal 5 "cannot start Linux for x86_64: Linux image \\WRONG.EFI is not a valid EFI image: a PE image for x86_64 (machine 0x8664), not aarch64"
    expect_refusal 6 "cannot start Missing initrd: initrd \\NOINITRD.IMG is not on the EFI system partition"
    expect_refusal 7 "cannot start Missing loader: EFI loader \\NOPE.EFI is not on the EFI system partition"
    expect_refusal 8 "cannot start Cut loader: EFI loader \\EFI\\TEST\\CUT.EFI is not a valid EFI image: a section of the PE image is cut off"
    expect_plain_absent "chainprobe: started" "the refusals"

    # A loader that starts, says where it was loaded from and returns: the
    # menu comes back, with no countdown.
    send_key "$boot_name" 9
    wait_for_line "chainprobe: started"
    wait_for_line "chainprobe: device handle set"
    wait_for_line 'chainprobe: file path \EFI\TEST\PROBE.EFI'
    wait_for_line "Chain probe returned SUCCESS: back to the menu"
    wait_for_plain_count "autoboot stopped" $((refusals + 1))
    expect_plain_once "starting Chain probe (chainload), chosen on the keyboard" "the chainload entry"
    expect_plain_once "chainprobe: started" "the chainloaded application"
    expect_plain_absent "Linux version" "the refusals and the chainload"
    expect_plain_absent "booted on aarch64" "the chainload"

    # And our own kernel is still there after all of that.
    send_key "$boot_name" 1
    wait_for_banner
    wait_for_line "$RESUME_MARKER"
    expect_plain_once "starting Normal (boot mode 0, normal), chosen on the keyboard" "Normal after the other entries"
    expect_once "$EXPECTED_BANNER" "boot banner"
    BOOT_TIMEOUT_SECONDS=10
    stop_qemu
    echo "PASS ($boot_name): $refusals refusals back to the menu, a chainloaded application, and our kernel after them"
}

# An initrd of a known size: a `newc` cpio archive with an /init that is not a
# program and a file of padding. The kernel unpacks it without complaint and
# then fails to run /init, which a missing or damaged copy could not show.
initrd_file="$work_dir/INITRD.IMG"
python3 -I - "$initrd_file" <<'PY'
import sys

def newc(name, data, inode, mode=0o100644):
    name = name.encode() + b"\0"
    header = "070701" + "".join(
        "%08x" % value for value in (inode, mode, 0, 0, 1, 0, len(data), 0, 0, 0, 0, len(name), 0)
    )
    entry = header.encode() + name
    entry += b"\0" * (-len(entry) % 4) + data
    return entry + b"\0" * (-len(entry) % 4)

def archive(padding):
    # An /init that is not a program: when the kernel unpacked the archive it
    # tries to run it and says that it could not, and when the initrd did not
    # arrive intact it says there is no /init at all.
    return (
        newc("init", b"not a program\n", 1, 0o100755)
        + newc("wsos-padding", b"k" * padding, 2)
        + newc("TRAILER!!!", b"", 0)
    )

# A whole number of pages: the kernel frees whole pages, so an initrd cut short
# by even a byte shows in the figure it prints.
padding = 1000000
padding += -len(archive(padding)) % 4096
open(sys.argv[1], "wb").write(archive(padding))
PY
initrd_bytes="$(wc -c < "$initrd_file" | tr -d ' ')"
# The application the bootloader chainloads, a copy that claims another
# machine, and one cut short.
python3 -I - "$chainprobe_efi" "$work_dir/WRONG.EFI" "$work_dir/CUT.EFI" <<'PY'
import struct, sys
source, wrong, cut = sys.argv[1:4]
image = bytearray(open(source, "rb").read())
pe = struct.unpack_from("<I", image, 0x3C)[0]
struct.pack_into("<H", image, pe + 4, 0x8664)
open(wrong, "wb").write(image)
open(cut, "wb").write(open(source, "rb").read()[: len(image) // 2])
PY
probe_files=(--file "\\EFI\\TEST\\PROBE.EFI=$chainprobe_efi" --file "\\EFI\\TEST\\CUT.EFI=$work_dir/CUT.EFI")

linux_config="entry Alpine Linux
linux \\VMLINUZ
cmdline $LINUX_CMDLINE"
linux_image="$(make_image linux "$linux_config" --file "\\VMLINUZ=$linux_kernel")"
check_linux uefi-linux "$linux_image" 3 "Alpine Linux"
linux_initrd_image="$(make_image linux-initrd "$linux_config
initrd \\INITRD.IMG" --file "\\VMLINUZ=$linux_kernel" --file "\\INITRD.IMG=$initrd_file")"
check_linux uefi-linux-initrd "$linux_initrd_image" 3 "Alpine Linux" "$initrd_bytes"

others_image="$(make_image others "entry Directory as Linux
linux \\EFI\\TEST
entry Not a PE
linux \\KERNEL.ELF
entry Linux for x86_64
linux \\WRONG.EFI
entry Missing initrd
linux \\VMLINUZ
cmdline console=ttyAMA0
initrd \\NOINITRD.IMG
entry Missing loader
chainload \\NOPE.EFI
entry Cut loader
chainload \\EFI\\TEST\\CUT.EFI
entry Chain probe
chainload \\EFI\\TEST\\PROBE.EFI" --file "\\VMLINUZ=$linux_kernel" --file "\\WRONG.EFI=$work_dir/WRONG.EFI" "${probe_files[@]}")"
check_others uefi-others "$others_image"

# A file with an error never hides our kernel: the line is named, the built-in
# entries are all there is, and the countdown still starts Normal.
# The label has a character the console cannot print (outside the Basic
# Multilingual Plane): printing it would panic the bootloader, so the file must
# be refused for it, at its line.
bad_config_image="$(make_image bad-config "entry Fine
chainload \\EFI\\TEST\\PROBE.EFI
entry Penguin $(printf '\360\237\220\247')
linux \\VMLINUZ" "${probe_files[@]}")"
disk_image="$bad_config_image"
check_uefi_boot uefi-bad-config 256 1 normal ""
expect_once "\\WSOS.CFG, line 3: '\\u{1f427}' cannot be in a label (printable ASCII only): the built-in entries only" "the configuration error"
expect_plain_absent "3  Fine" "a menu built from a bad file"
config_image="$(make_image with-config "entry Chain probe
chainload \\EFI\\TEST\\PROBE.EFI" "${probe_files[@]}")"
disk_image="$config_image"
check_uefi_boot uefi-with-config 256 1 normal ""
expect_once '\WSOS.CFG: 1 entries' "the configuration file"
expect_plain_once "3  Chain probe" "the entry of the file in the menu"
check_panic unhandled-class "$class_elf" "SVC (AArch64)"
check_panic unhandled-vector "$vector_elf" "synchronous, current EL with SP_EL0"
check_panic compiler-trap "$trap_elf" "BRK (AArch64) (class 0x3c, iss 0x0000001"
# An exception taken by the handler itself, on the exception stack, is reported
# as nested (ADR 0026) and not handled on the same stack again.
nested_elf="$work_dir/nested-exception.elf"
build_with_feature selftest-nested-exception "$nested_elf"
cargo build --quiet --target-dir "$TARGET_DIR"
check_panic nested-exception "$nested_elf" "nested exception on the exception stack: BRK (AArch64) (class 0x3c, iss 0x0000802"

# Write-xor-execute (ADR 0019): a write to the kernel's code, a jump into
# constants and a jump into data each end in a reported fault, in every way the
# kernel is entered. A data abort for a store whose address translates but may
# not be written is a permission fault at level 3 (DFSC 0b001111) with WnR set,
# so its syndrome ends in 0x4f; an instruction fetch from a page that may not
# be executed is the same fault with IFSC 0b001111.
write_elf="$work_dir/write-text.elf"
data_elf="$work_dir/execute-data.elf"
rodata_elf="$work_dir/execute-rodata.elf"
build_with_feature selftest-write-text "$write_elf"
build_with_feature selftest-execute-data "$data_elf"
build_with_feature selftest-execute-rodata "$rodata_elf"
# A stack overflow (ADR 0026): the recursion's first store below the boot stack
# hits the guard page, which is a level 3 translation fault on a store (DFSC
# 0b000111, WnR set: the syndrome ends in 0x47), at an address in the guard
# page, from kernel code. The report must be made on the exception stack: the
# probe the linker reserves at the top of `.bss`, which a handler working on the
# overflowed stack would overwrite, is whole afterwards, and no nested exception
# was reported.
overflow_elf="$work_dir/stack-overflow.elf"
build_with_feature selftest-stack-overflow "$overflow_elf"
cargo build --quiet --target-dir "$TARGET_DIR"
for kind in write execute-data execute-rodata stack-overflow; do
    case "$kind" in
        write) elf="$write_elf" ;;
        execute-data) elf="$data_elf" ;;
        execute-rodata) elf="$rodata_elf" ;;
        stack-overflow) elf="$overflow_elf" ;;
    esac
    # The addresses come from the very ELF that boots: a build with another
    # feature may lay the sections out differently.
    text_range="$(symbol_of "$elf" __image_start)..$(symbol_of "$elf" __text_end)"
    image="$work_dir/$kind.img"
    python3 -I scripts/make-disk-image.py "$image" target/aarch64-unknown-uefi/debug/bootloader.efi "$elf" > /dev/null
    case "$kind" in
        write) fault=(data\ abort 0x25 '0x[0-9a-f]{5}4f' "$text_range" "$(symbol_of "$elf" __image_start)") ;;
        execute-data) fault=(instruction\ abort 0x21 0x000000f "$(symbol_of "$elf" __rodata_end)" "$(symbol_of "$elf" __rodata_end)") ;;
        execute-rodata) fault=(instruction\ abort 0x21 0x000000f "$(symbol_of "$elf" __text_end)" "$(symbol_of "$elf" __text_end)") ;;
        stack-overflow) fault=(data\ abort 0x25 '0x[0-9a-f]{5}47' "$text_range" "$(symbol_of "$elf" __data_end)..$(symbol_of "$elf" __stack_bottom)") ;;
    esac
    FAULT_ALSO=""
    FAULT_ABSENT=""
    if [[ "$kind" == stack-overflow ]]; then
        FAULT_ALSO="selftest: .bss stack probe intact"
        FAULT_ABSENT="nested exception"
    fi
    check_fault "$kind" "${fault[@]}" -kernel "$elf"
    check_fault "$kind-el2" "${fault[@]}" -machine virtualization=on -kernel "$elf"
    BOOT_TIMEOUT_SECONDS=$UEFI_BOOT_TIMEOUT_SECONDS
    check_fault "uefi-$kind" "${fault[@]}" -machine virt,acpi=off -m 256M -bios "$uefi_firmware" \
        -drive "file=$image,format=raw,if=virtio"
    check_fault "uefi-el2-$kind" "${fault[@]}" -machine virt,acpi=off,virtualization=on -m 256M -bios "$uefi_firmware" \
        -drive "file=$image,format=raw,if=virtio"
    BOOT_TIMEOUT_SECONDS=10
done
FAULT_ALSO=""
FAULT_ABSENT=""

# Permission indirection (ADR 0026): `-cpu max` has FEAT_S1PIE, so the kernel
# writes and then reads back TCR2_EL1 and SCTLR2_EL1. QEMU resets them to zero,
# so a kernel that never wrote them would pass; the build with
# `selftest-dirty-tcr2` sets TCR2_EL1.PIE first, as firmware might have left it,
# and only a kernel that clears it passes.
expected_extensions="$EXTENSIONS_CLEARED"
check_boot cpu-max -cpu max -kernel "$KERNEL_ELF"
# The same cores entered at EL2 and EL3: the writes trap there unless the drop
# enabled them (SCR_EL3.TCR2En, HCRX_EL2.TCR2En), and nothing would be printed.
check_boot cpu-max-el2 -cpu max -machine virtualization=on -kernel "$KERNEL_ELF"
check_boot cpu-max-el3 -cpu max -machine secure=on -kernel "$KERNEL_ELF"
check_boot cpu-max-el3-el2 -cpu max -machine secure=on,virtualization=on -kernel "$KERNEL_ELF"
dirty_elf="$work_dir/dirty-tcr2.elf"
build_with_feature selftest-dirty-tcr2 "$dirty_elf"
cargo build --quiet --target-dir "$TARGET_DIR"
dirty_image="$work_dir/dirty-tcr2.img"
python3 -I scripts/make-disk-image.py "$dirty_image" target/aarch64-unknown-uefi/debug/bootloader.efi "$dirty_elf" > /dev/null
# check_extensions NAME QEMU_ARGS...: the kernel built to dirty TCR2_EL1 boots
# and reports the registers cleared.
check_extensions() {
    start_qemu "$@"
    wait_for_line "$RESUME_MARKER"
    expect_once "$EXTENSIONS_CLEARED" "permission indirection registers"
    expect_once "mmu: write xor execute holds" "write xor execute report"
    stop_qemu
    echo "PASS ($boot_name): TCR2_EL1 cleared although it was set before"
}
check_extensions dirty-tcr2 -cpu max -kernel "$dirty_elf"
check_extensions dirty-tcr2-el2 -cpu max -machine virtualization=on -kernel "$dirty_elf"
check_extensions dirty-tcr2-el3-el2 -cpu max -machine secure=on,virtualization=on -kernel "$dirty_elf"
BOOT_TIMEOUT_SECONDS=$UEFI_BOOT_TIMEOUT_SECONDS
# firmware_reaches_boot_manager QEMU_ARGS...: the firmware alone, with no disk,
# gets to its boot manager (the first line of the BDS phase, `BdsDxe:`, whichever
# message follows: the Homebrew build starts its shell, the Ubuntu one reports
# that no boot option was found). Measured on the CI runner (QEMU 10.2.1, Ubuntu's
# qemu-efi-aarch64 2025.11): on `-cpu max` that firmware prints its banner and
# then takes a prefetch abort at its own exception vector over and over (the
# `-d int` log held 621 million lines after 300 s), while the same image boots
# in 9 s on `neoverse-v1` and `cortex-a76`. Turning off pauth, SVE, SME and the
# PMU did not help, and LPA2 off ended in another prefetch abort. The cause
# inside the firmware is not known; it does not touch our code.
firmware_reaches_boot_manager() {
    start_qemu firmware-alone "$@" -machine virt,acpi=off -m 256M -bios "$uefi_firmware"
    local reached=1 _
    for _ in $(seq 1 $((UEFI_BOOT_TIMEOUT_SECONDS * 10))); do
        if grep -qF "BdsDxe:" "$serial_log"; then
            reached=0
            break
        fi
        sleep 0.1
    done
    stop_qemu
    return $reached
}
# The probe checks itself: if the firmware does not reach its boot manager on
# the default CPU either, the marker or the firmware is wrong, and the gate fails
# instead of skipping what it cannot run.
firmware_reaches_boot_manager || fail "the firmware does not reach its boot manager on the default CPU"
if firmware_reaches_boot_manager -cpu max; then
    check_extensions uefi-dirty-tcr2 -cpu max -machine virt,acpi=off -m 256M -bios "$uefi_firmware" \
        -drive "file=$dirty_image,format=raw,if=virtio"
    check_extensions uefi-el2-dirty-tcr2 -cpu max -machine virt,acpi=off,virtualization=on -m 256M -bios "$uefi_firmware" \
        -drive "file=$dirty_image,format=raw,if=virtio"
    check_uefi_boot uefi-cpu-max 256 1 normal enter -cpu max
    # The bootloader's own drop from EL2 must enable the writes too.
    check_uefi_boot uefi-cpu-max-el2 256 2 normal enter -cpu max -machine virtualization=on
else
    echo "SKIP (uefi-dirty-tcr2, uefi-el2-dirty-tcr2, uefi-cpu-max, uefi-cpu-max-el2): $uefi_firmware does not reach its boot manager on -cpu max, although it does on the default CPU; the bootloader on a core with TCR2 and HCRX is not exercised on this machine"
fi
BOOT_TIMEOUT_SECONDS=10
expected_extensions="$EXTENSIONS_ABSENT"
