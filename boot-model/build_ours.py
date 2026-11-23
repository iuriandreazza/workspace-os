#!/usr/bin/env python3
"""Builds our own kernel and bootloader at several commits of this repository and
pins the first bytes of each, for the corpus of the boot model (B2).

    build_ours.py            builds, then writes corpus/ours-pins.json

Each commit is built in a throw-away `git worktree` with the toolchain that
commit pins, in a target directory of its own under the cache. The pinned bytes
are the SHA-256 of the first PREFIX_BYTES of the ELF kernel and of the UEFI
bootloader. A rebuild with another compiler may give other bytes; the check in
`fetch.py` then reports a mismatch rather than training on something else.
"""
import json
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import fetch  # noqa: E402

REPO = HERE.parent
OUT = HERE / "corpus" / "ours-pins.json"
# (label, commit): spread over the history, from the first boot to the latest bootloader.
KERNELS = (
    ("k0-first-boot", "123140a"),
    ("k2-device-tree", "cede1ec"),
    ("k1-exceptions", "862ad32"),
    ("k3-frame-allocator", "a44a178"),
    ("k4-mmu", "1c86b95"),
    ("k4b-hardening", "831f9a5"),
    ("k5-heap", "008ddce"),
)
BOOTLOADERS = (
    ("k2b2-bootloader", "e8b71ea"),
    ("k2b3-gaps", "452249a"),
    ("k2c-menu", "3cd7bbb"),
    ("k2d-head", "008ddce"),
)


def run(args, cwd):
    subprocess.run(args, cwd=cwd, check=True, stdout=subprocess.DEVNULL)


def build(commit, label, pins, kernel):
    with tempfile.TemporaryDirectory(prefix="wsos-ours-") as scratch:
        tree = Path(scratch) / "tree"
        run(["git", "worktree", "add", "--detach", str(tree), commit], REPO)
        try:
            target = fetch.CACHE / "ours-target" / label
            if kernel:
                for profile in ("debug", "release"):
                    flag = ["--release"] if profile == "release" else []
                    run(["cargo", "build", "--quiet", "--target-dir", str(target), "-p", "kernel", *flag], tree)
                    add(pins, label, commit, f"kernel-{profile}", target / "aarch64-unknown-none-softfloat" / profile / "kernel", "ours-kernel")
            else:
                run(["cargo", "build", "--quiet", "--target-dir", str(target), "-p", "bootloader",
                     "--target", "aarch64-unknown-uefi"], tree)
                add(pins, label, commit, "bootloader", target / "aarch64-unknown-uefi" / "debug" / "bootloader.efi", "ours-bootloader")
        finally:
            run(["git", "worktree", "remove", "--force", str(tree)], REPO)


def add(pins, label, commit, flavour, path, kind):
    data = path.read_bytes()[:fetch.PREFIX_BYTES]
    pins.append(dict(system="ours", release=f"{label}-{flavour}", url=f"git:{commit}", mode="prefix",
                     bytes=len(data), size=path.stat().st_size, sha256=fetch.store(data),
                     verified_against=f"built from commit {commit}", licence="GPL-2.0-only", kind=kind))


def main():
    pins = []
    for label, commit in KERNELS:
        build(commit, label, pins, kernel=True)
    for label, commit in BOOTLOADERS:
        build(commit, label, pins, kernel=False)
    OUT.write_text(json.dumps(pins, indent=1, sort_keys=True) + "\n")
    print(f"wrote {OUT} with {len(pins)} pins")


if __name__ == "__main__":
    main()
