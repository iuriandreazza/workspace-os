"""The files of the corpus and what each one is (B2).

An *asset* is one file: its system, the release it comes from, its name, the first
bytes of its content (all the model ever reads is 448 of them) and the three
labels of ADR 0027 that do not depend on the disk: family, protocol and role.

How a file is labelled (the rule is written once, here):

- kernel image, initrd, device tree, EFI loader and entry text carry the family of
  the system they belong to; every other file is `none`, `none`, `other`;
- protocol is the protocol of the entry the file belongs to: `ours` for our kernel,
  `linux` for a Linux image, its initrd, its device tree and its entry text,
  `chainload` for an EFI loader, `none` for a kernel the loader cannot start (a BSD
  kernel) and for everything else;
- our own bootloader is `ours`, `none`, `other`: the loader that is running is not
  an entry of its own menu.

Every pinned file comes from `corpus/pins.json` (`fetch.py`) or `corpus/ours-pins.json`
(`build_ours.py`), through the cache; nothing else is read from the network here.
"""
import hashlib
import json
import shutil
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import fetch  # noqa: E402

TRAIN, VALIDATION, TEST = "train", "validation", "test"
# Releases are held out whole, in two sets that were fixed at different times.
#
# VALIDATION: held out from the start and used while the method was developed (the first
# runs on it showed the model memorising compressed content; the three training choices
# in `training.py` that answer that were made looking at it).
# TEST: releases pinned only after those choices were frozen, so no decision was taken
# looking at them. They are the numbers the report headlines.
#
# No file of a held-out release is also a training file under another name, except where the
# publisher ships the same bytes in two releases (the report counts those by window hash).
VALIDATION_RELEASES = {
    "alpine": {"3.23.0", "3.23.8", "3.24.0", "3.24.4"},
    "alpine-rootfs": {"3.24.4"},
    "debian": {"trixie-20250803+deb13u8"},
    "fedora": {"44"},
    "rocky": {"9.4"},
    "opensuse": {"15.6"},
    "openbsd": {"7.8"},
    "ours": {"k4-mmu-kernel-debug", "k4-mmu-kernel-release", "k5-heap-kernel-debug",
             "k5-heap-kernel-release", "k2d-head-bootloader"},
}
TEST_RELEASES = {
    "alpine": {"3.18.12"},
    "alpine-rootfs": {"3.16.9", "3.18.12"},
    "debian": {"buster-20190702+deb10u13"},
    "fedora": {"39", "42"},
    "alma": {"9.3"},
    "rocky": {"8.9", "9.3"},
    "opensuse": {"15.2"},
    "openbsd": {"7.6", "7.9", "8.0"},
    "ours": {"k2-device-tree-kernel-debug", "k2-device-tree-kernel-release", "k4b-hardening-kernel-debug",
             "k4b-hardening-kernel-release", "k2b3-gaps-bootloader"},
}


class Asset:
    def __init__(self, system, release, name, content, family, protocol, role, split, source=""):
        self.system, self.release, self.name, self.content = system, release, name, content
        self.family, self.protocol, self.role, self.split = family, protocol, role, split
        self.source = source

    def labels(self):
        return self.family, self.protocol, self.role

    def key(self):
        return f"{self.system}/{self.release}/{self.name}"


def split_of(system, release):
    if release in TEST_RELEASES.get(system, ()):
        return TEST
    if release in VALIDATION_RELEASES.get(system, ()):
        return VALIDATION
    return TRAIN


LOADERS = {
    "debian": {"grubaa64.efi", "bootnetaa64.efi"},
    "fedora": {"bootaa64.efi", "grubaa64.efi"},
}
# Rebuilds of Red Hat Enterprise Linux ship the Fedora layout.
SAME_LAYOUT = {"alma": "fedora", "rocky": "fedora"}


def classify(system, name):
    """`(family, protocol, role)` of a file by system and base name."""
    system = SAME_LAYOUT.get(system, system)
    low = name.lower()
    other = ("none", "none", "other")
    if low.endswith(".dtb"):
        return ("linux", "linux", "dtb")
    if system == "alpine":
        if low.startswith("vmlinuz-"):
            return ("linux", "linux", "kernel")
        if low.startswith("initramfs-"):
            return ("linux", "linux", "initrd")
    elif system == "debian":
        if low == "linux":
            return ("linux", "linux", "kernel")
        if low == "initrd.gz":
            return ("linux", "linux", "initrd")
    elif system == "fedora":
        if low == "vmlinuz":
            return ("linux", "linux", "kernel")
        if low == "initrd.img":
            return ("linux", "linux", "initrd")
    elif system == "opensuse":
        if low == "linux":
            return ("linux", "linux", "kernel")
        if low == "initrd":
            return ("linux", "linux", "initrd")
    elif system == "openbsd":
        if low.startswith("bsd"):
            return ("bsd", "none", "kernel")
        if low == "bootaa64.efi":
            return ("bsd", "chainload", "loader")
    if low in LOADERS.get(system, ()):
        return ("linux", "chainload", "loader")
    return other


def refine(labels, content):
    """A Linux image the loader cannot start through its EFI stub is a kernel with protocol `none`.

    ADR 0027, section 4: Linux is started through its EFI stub only, so an image that is not a PE
    file is refused. Some distributions ship the kernel as a bare gzip stream (Alpine 3.18 and
    older on `virt` and `lts`, Rocky and AlmaLinux): that is a Linux kernel, but not one this
    loader can start, like a BSD kernel. The check is the first two bytes, `MZ`.
    """
    family, protocol, role = labels
    if role == "kernel" and protocol == "linux" and not content.startswith(b"MZ"):
        return family, "none", role
    return labels


def _pin_assets(pins):
    for pin in pins:
        system, release = pin["system"], pin["release"]
        split = split_of(system, release)
        data = fetch.pinned_bytes(pin)
        if pin["mode"] == "prefix":
            if system == "ours":
                labels = ("ours", "ours", "kernel") if pin["kind"] == "ours-kernel" else ("ours", "none", "other")
                name = "kernel" if pin["kind"] == "ours-kernel" else "bootloader.efi"
            else:
                name = pin["url"].rsplit("/", 1)[1]
                labels = classify(system, name)
            yield Asset(system, release, name, data[:fetch.PREFIX_BYTES], *refine(labels, data), split, pin["url"])
        elif system == "openbsd":
            yield Asset(system, release, "BOOTAA64.EFI", data[:fetch.PREFIX_BYTES], *classify(system, "BOOTAA64.EFI"),
                        split, pin["url"])
        else:
            yield from _archive_assets(pin, split, data)


def _archive_assets(pin, split, data):
    system, release = pin["system"], pin["release"]
    with tarfile.open(fileobj=__import__("io").BytesIO(data)) as archive:
        for member in archive.getmembers():
            if not member.isfile():
                continue
            name = member.name.rsplit("/", 1)[-1]
            content = archive.extractfile(member).read(fetch.PREFIX_BYTES)
            if system == "alpine-rootfs":
                labels = ("none", "none", "other")
            else:
                labels = refine(classify(system, name), content)
            yield Asset(system, release, name, content, *labels, split, f"{pin['url']}!{member.name}")


def pinned_assets():
    pins = json.loads(fetch.PINS.read_text())
    ours = HERE / "corpus" / "ours-pins.json"
    if ours.is_file():
        pins += json.loads(ours.read_text())
    return list(_pin_assets(pins))


def qemu_dtb_assets(seed_memory=("128M", "512M", "2G", "8G"), cpus=("cortex-a53", "cortex-a57", "cortex-a72", "max"),
                    smp=(1, 2, 4, 8), gic=("2", "3")):
    """Device trees that QEMU's own `virt` machine writes (`dumpdtb`), one per setting.

    GPL-2.0-only, like QEMU. Needs `qemu-system-aarch64` on PATH; without it there are
    none, and the report says the corpus had no QEMU trees.
    """
    qemu = shutil.which("qemu-system-aarch64")
    if not qemu:
        return []
    assets = []
    with tempfile.TemporaryDirectory(prefix="wsos-dtb-") as scratch:
        index = 0
        for memory in seed_memory:
            for cpu in cpus:
                for n in smp:
                    for version in gic:
                        out = Path(scratch) / f"{index}.dtb"
                        result = subprocess.run(
                            [qemu, "-machine", f"virt,gic-version={version},dumpdtb={out}", "-cpu", cpu,
                             "-smp", str(n), "-m", memory, "-display", "none", "-nodefaults"],
                            capture_output=True)
                        if result.returncode == 0 and out.is_file():
                            # the trees differ by configuration, not by release: hold out two whole CPU models
                            split = {"cortex-a72": VALIDATION, "max": TEST}.get(cpu, TRAIN)
                            assets.append(Asset("qemu", f"{cpu}", f"virt-{cpu}-{n}-{memory}-gic{version}.dtb",
                                                out.read_bytes()[:fetch.PREFIX_BYTES], "linux", "linux", "dtb",
                                                split, "qemu dumpdtb"))
                        index += 1
    return assets


def fingerprint(assets):
    """One SHA-256 over every asset's key and bytes, to say which corpus a result came from."""
    digest = hashlib.sha256()
    for asset in sorted(assets, key=lambda a: a.key()):
        digest.update(asset.key().encode() + b"\0" + asset.content + b"\0")
    return digest.hexdigest()
