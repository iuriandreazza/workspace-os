#!/usr/bin/env python3
"""Measures the inference time of the boot model in a UEFI application under
QEMU (B1-2, ADR 0027), or checks that the measuring program still runs.

    measure-boot-model.py --check
    measure-boot-model.py [--accel tcg --accel hvf] [--series 7] [--reps 21] [--out DIR]

The program is `bootloader/probe/inferprobe.rs`, built in release, alone on a
disk image. Each series is one boot of QEMU, which runs `--reps` repetitions of
every workload and prints the time of each. The script reports, per workload,
the median of the repetitions of each series and then the median over the
series that were quiet, and says which series it dropped and why. It never
reports a number measured on a loaded machine without saying so.

`--check` is what the gate runs: one short boot on TCG, with the checksums of
the workload compared with the ones below. A checksum that changes means the
arithmetic changed, and the times of ADR 0027 no longer describe the program:
measure again and update EXPECTED_CHECKSUMS.

Needs `cargo`, `qemu-system-aarch64` and an AArch64 UEFI firmware (UEFI_FIRMWARE
to point at one). Standard library only.
"""
import argparse
import os
import re
import statistics
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PROBE = ROOT / "target/aarch64-unknown-uefi/release/inferprobe.efi"
FIRMWARES = [
    "/opt/homebrew/share/qemu/edk2-aarch64-code.fd",
    "/usr/local/share/qemu/edk2-aarch64-code.fd",
    "/usr/share/qemu-efi-aarch64/QEMU_EFI.fd",
    "/usr/share/AAVMF/AAVMF_CODE.fd",
    "/usr/share/qemu/edk2-aarch64-code.fd",
]
# What the gate requires of the first repetition. Integer and fixed-order fp32
# arithmetic, so the same on TCG, HVF and real hardware.
EXPECTED_CHECKSUMS = {
    "classifier-candidate": "0xe969488b9223262d",
    "classifier-disk": "0x64585504656c96b6",
    "slm-fp32-layer": "0x3d0c1fc04a8be4c7",
    "slm-int8-token": "0xb736fd413e0de573",
}
# One flag set per accelerator. `-cpu host` under HVF is the M1 itself.
ACCELERATORS = {
    "tcg": ["-accel", "tcg", "-cpu", "cortex-a72"],
    "hvf": ["-accel", "hvf", "-cpu", "host"],
}
QEMU_COMMON = [
    "-machine", "virt,acpi=off", "-m", "512M", "-nodefaults", "-display", "none",
    "-serial", "stdio", "-monitor", "none",
]
# A series is quiet when the machine was not busy with other work (load average
# at both ends of the boot, on the eight cores of the machine this was written
# on) and the spread of the repetitions of every workload is small
# (interquartile range over median).
QUIET_IQR_OVER_MEDIAN = 0.10
QUIET_LOAD_AVERAGE = 4.0
CLOCK_CHECK_RANGE = (0.9, 1.1)
# Two repetitions: the program compares the checksum of the second with the first.
CHECK_REPS = 2
WARMUP_SECONDS = 1.0
MIN_QUIET_SERIES = 3
ANSI = re.compile(r"\x1b\[[0-9;?]*[a-zA-Z]")
LINE = re.compile(
    r"inferprobe: (?P<name>\S+) reps (?P<reps>\d+) median_us (?P<median>\d+) min_us (?P<min>\d+) "
    r"q1_us (?P<q1>\d+) q3_us (?P<q3>\d+) max_us (?P<max>\d+) checksum (?P<checksum>0x[0-9a-f]+)"
)
RAW = re.compile(r"inferprobe: (?P<name>\S+) raw_us (?P<values>[0-9 ]+)")


def die(message):
    print(f"measure-boot-model: {message}", file=sys.stderr)
    sys.exit(1)


def find_firmware():
    candidates = [os.environ["UEFI_FIRMWARE"]] if os.environ.get("UEFI_FIRMWARE") else FIRMWARES
    for path in candidates:
        if os.path.isfile(path):
            return path
    die("no AArch64 UEFI firmware found; install qemu-efi-aarch64 or set UEFI_FIRMWARE")


def run(command, **kwargs):
    return subprocess.run(command, cwd=ROOT, check=True, **kwargs)


def build():
    run(["cargo", "build", "--release", "--quiet", "--target-dir", "target", "-p", "bootloader",
         "--target", "aarch64-unknown-uefi", "--bin", "inferprobe"])


def make_image(directory, reps, slm, warmup):
    config = directory / "INFER.CFG"
    config.write_text(f"reps {reps}\nslm {int(slm)}\nwarmup_ms {1000 if warmup else 0}\n")
    image = directory / "infer.img"
    run([sys.executable, "-I", "scripts/make-disk-image.py", str(image), str(PROBE),
         "--file", f"\\INFER.CFG={config}"])
    return image


def others_running():
    """How many QEMU processes are running. Called while ours is not, so all of
    them are other people's: what the other sessions run."""
    out = subprocess.run(["ps", "-A", "-o", "comm="], capture_output=True, text=True).stdout
    return sum(1 for line in out.splitlines() if "qemu-system" in line)


def wait_until_quiet(limit_seconds=900):
    """Waits for the other work on the machine to settle, so a series does not
    start in the middle of someone else's test run. Gives up after the limit and
    lets the series run, to be dropped if it was disturbed."""
    deadline = time.monotonic() + limit_seconds
    while os.getloadavg()[0] > QUIET_LOAD_AVERAGE and time.monotonic() < deadline:
        time.sleep(10)


def boot_once(accelerator, image, firmware, timeout):
    """One boot. Returns the parsed lines, the raw repetitions and the host
    time between the two series the clock check uses."""
    command = ["qemu-system-aarch64", *QEMU_COMMON, *ACCELERATORS[accelerator], "-bios", firmware,
               "-drive", f"file={image},format=raw,if=virtio"]
    started = time.monotonic()
    process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                               stderr=subprocess.STDOUT, text=True, bufsize=1)
    summary, raw, arrival, text = {}, {}, {}, []
    # A timer, not a test between lines: a firmware that stops printing would
    # block the read for ever.
    expired = threading.Event()

    def expire():
        expired.set()
        process.kill()

    timer = threading.Timer(timeout, expire)
    timer.start()
    try:
        for line in process.stdout:
            line = ANSI.sub("", line.replace("\r", ""))
            text.append(line)
            if "inferprobe: FAIL" in line:
                break  # the firmware stays up after a failed probe; do not wait for it
            if match := LINE.search(line):
                summary[match["name"]] = {key: (int(v) if key not in ("name", "checksum") else v)
                                         for key, v in match.groupdict().items()}
            elif match := RAW.search(line):
                raw[match["name"]] = [int(v) for v in match["values"].split()]
                arrival[match["name"]] = time.monotonic()
    finally:
        timer.cancel()
        process.kill()
        process.wait()
    log = "".join(text)
    if expired.is_set():
        die(f"{accelerator}: no end after {timeout} s\n{log}")
    if "inferprobe: done" not in log or "FAIL" in log:
        die(f"{accelerator}: the program did not finish cleanly\n{log}")
    return summary, raw, arrival, log


def check():
    build()
    with tempfile.TemporaryDirectory() as directory:
        image = make_image(Path(directory), reps=CHECK_REPS, slm=True, warmup=False)
        summary, _, _, log = boot_once("tcg", image, find_firmware(), timeout=240)
    for name, expected in EXPECTED_CHECKSUMS.items():
        if name not in summary:
            die(f"the program printed no '{name}' line\n{log}")
        if summary[name]["reps"] != CHECK_REPS:
            die(f"'{name}' ran {summary[name]['reps']} repetitions, expected {CHECK_REPS}: \\INFER.CFG was not read")
        got = summary[name]["checksum"]
        if expected is None:
            die(f"EXPECTED_CHECKSUMS['{name}'] is not set; the program printed {got}")
        if got != expected:
            die(f"'{name}' has checksum {got}, expected {expected}: the arithmetic changed; "
                "measure again and update EXPECTED_CHECKSUMS")
    print("measure-boot-model: the measuring program runs, reads its configuration and its checksums match")


def iqr_over_median(summary):
    """The largest spread of any workload, and which one."""
    return max(((e["q3"] - e["q1"]) / e["median"], name) for name, e in summary.items())


def measure(accelerators, series, reps, out):
    build()
    firmware = find_firmware()
    out.mkdir(parents=True, exist_ok=True)
    version = subprocess.run(["qemu-system-aarch64", "--version"], capture_output=True, text=True).stdout.splitlines()[0]
    print(f"{version}; firmware {firmware}; {series} series of {reps} repetitions per accelerator")
    with tempfile.TemporaryDirectory() as directory:
        image = make_image(Path(directory), reps=reps, slm=True, warmup=True)
        for accelerator in accelerators:
            kept, dropped = [], []
            for index in range(series):
                wait_until_quiet()
                load_before, others_before = os.getloadavg()[0], others_running()
                summary, raw, arrival, log = boot_once(accelerator, image, firmware, timeout=1800)
                load_after, others_after = os.getloadavg()[0], others_running()
                (out / f"{accelerator}-{index}.log").write_text(log)
                spread, widest = iqr_over_median(summary)
                note = (f"load {load_before:.2f}->{load_after:.2f}, other qemu {others_before}->{others_after}, "
                        f"widest IQR/median {spread:.3f} ({widest})")
                # The clock check: the host time between the arrival of the
                # repetitions of the candidate series and of the disk series is
                # the warm-up plus the repetitions of the disk series, as the
                # program's counter counted them.
                host = arrival["classifier-disk"] - arrival["classifier-candidate"]
                counter = sum(raw["classifier-disk"]) / 1e6 + WARMUP_SECONDS
                note += f", clock check host/counter {host / counter:.2f}"
                clock = host / counter
                if (spread > QUIET_IQR_OVER_MEDIAN or max(load_before, load_after) > QUIET_LOAD_AVERAGE
                        or not CLOCK_CHECK_RANGE[0] <= clock <= CLOCK_CHECK_RANGE[1]):
                    dropped.append((index, note))
                    print(f"{accelerator} series {index}: DROPPED, noisy ({note})")
                else:
                    kept.append(summary)
                    print(f"{accelerator} series {index}: kept ({note})")
            report(accelerator, kept, dropped, series)


def report(accelerator, kept, dropped, series):
    if len(kept) < MIN_QUIET_SERIES:
        die(f"{accelerator}: only {len(kept)} of {series} series were quiet; the machine is too loaded, try later")
    print(f"\n{accelerator}: {len(kept)} quiet series of {series}; microseconds")
    print(f"  {'workload':24} {'median of medians':>18} {'min of medians':>15} {'max of medians':>15} {'median of mins':>15}")
    for name in kept[0]:
        medians = [s[name]["median"] for s in kept]
        mins = [s[name]["min"] for s in kept]
        print(f"  {name:24} {statistics.median(medians):18.0f} {min(medians):15d} {max(medians):15d} {statistics.median(mins):15.0f}")
    print()


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--check", action="store_true", help="the gate's short run")
    parser.add_argument("--accel", action="append", choices=sorted(ACCELERATORS), help="repeat for several; default tcg")
    parser.add_argument("--series", type=int, default=7)
    parser.add_argument("--reps", type=int, default=21, choices=range(1, 65), metavar="1..64")
    parser.add_argument("--out", type=Path, default=ROOT / "target/boot-model-measurement")
    args = parser.parse_args()
    if args.check:
        check()
    else:
        measure(args.accel or ["tcg"], args.series, args.reps, args.out)


main()
