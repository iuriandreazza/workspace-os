#!/usr/bin/env python3
"""Downloads the corpus of the boot model into a cache outside the repository (B2).

    fetch.py pin      discover the files below and write corpus/pins.json
    fetch.py fetch    download what pins.json lists and check every SHA-256

Nothing downloaded is committed or executed. Large files are never downloaded in
full: the model reads 448 bytes of a file's content, so a *prefix* of
PREFIX_BYTES is fetched with an HTTP Range request and pinned by the SHA-256 of
those bytes. A prefix has no checksum published by anyone, so it is a pin made
on first fetch (trust on first use): it detects the file changing later, not a
bad first download. Small archives are downloaded in full and, where the
publisher gives a checksum (Debian SHA256SUMS, Alpine .sha256, OpenBSD SHA256),
checked against it before the pin is written. `verified_against` says which.
"""
import hashlib
import json
import os
import re
import sys
import time
import http.client as httpclient
import urllib.error
import urllib.request
from pathlib import Path

PREFIX_BYTES = 4096
HERE = Path(__file__).resolve().parent
PINS = HERE / "corpus" / "pins.json"
CACHE = Path(os.environ.get("WSOS_CORPUS_CACHE") or Path.home() / ".cache" / "workspace-os" / "boot-model")
# A generic client name: nothing about the user or the project goes to the mirrors.
AGENT = "corpus-fetch/1"
LIMIT_FULL = 100 * 1024 * 1024  # an archive larger than this is never downloaded whole (checked in `http`)

LISTING_LIMIT = 16 * 1024 * 1024  # a directory listing or a checksum file
ALPINE = "https://dl-cdn.alpinelinux.org/alpine"
ALPINE_RELEASES = ("3.18.12", "3.19.0", "3.19.9", "3.20.0", "3.20.10", "3.21.0", "3.21.10", "3.22.0", "3.22.8",
                   "3.23.0", "3.23.8", "3.24.0", "3.24.4")
ALPINE_ROOTFS = ("3.16.9", "3.18.12", "3.19.9", "3.21.10", "3.24.4")
DEBIAN_INSTALLERS = (  # (release label, base URL of the dated, immutable installer directory)
    ("bookworm-20230607+deb12u15", "https://deb.debian.org/debian/dists/bookworm/main/installer-arm64/20230607+deb12u15/images/"),
    ("trixie-20250803+deb13u8", "https://deb.debian.org/debian/dists/trixie/main/installer-arm64/20250803+deb13u8/images/"),
    ("buster-20190702+deb10u13", "https://archive.debian.org/debian/dists/buster/main/installer-arm64/20190702+deb10u13/images/"),
    ("bullseye-20210731+deb11u12", "https://archive.debian.org/debian/dists/bullseye/main/installer-arm64/20210731+deb11u12/images/"),
)
# (system, release, base URL of the tree). Fedora 40 and 41 are on the archive mirror; Rocky and
# AlmaLinux are rebuilds of Red Hat Enterprise Linux, so a different distribution on the same layout.
EL_TREES = (
    ("fedora", "39", "https://archives.fedoraproject.org/pub/archive/fedora/linux/releases/39/Server/aarch64/os/"),
    ("fedora", "42", "https://archives.fedoraproject.org/pub/archive/fedora/linux/releases/42/Server/aarch64/os/"),
    ("alma", "9.3", "https://repo.almalinux.org/vault/9.3/BaseOS/aarch64/os/"),
    ("rocky", "8.9", "https://dl.rockylinux.org/vault/rocky/8.9/BaseOS/aarch64/os/"),
    ("rocky", "9.3", "https://dl.rockylinux.org/vault/rocky/9.3/BaseOS/aarch64/os/"),
    ("fedora", "40", "https://archives.fedoraproject.org/pub/archive/fedora/linux/releases/40/Server/aarch64/os/"),
    ("fedora", "41", "https://archives.fedoraproject.org/pub/archive/fedora/linux/releases/41/Server/aarch64/os/"),
    ("fedora", "43", "https://dl.fedoraproject.org/pub/fedora/linux/releases/43/Server/aarch64/os/"),
    ("fedora", "44", "https://dl.fedoraproject.org/pub/fedora/linux/releases/44/Server/aarch64/os/"),
    ("alma", "9.4", "https://repo.almalinux.org/vault/9.4/BaseOS/aarch64/os/"),
    ("rocky", "9.4", "https://dl.rockylinux.org/vault/rocky/9.4/BaseOS/aarch64/os/"),
)
OPENSUSE = ("15.2", "15.3", "15.4", "15.5", "15.6")
OPENBSD = (  # (release, mirror); 7.6 is no longer on the main CDN
    ("7.6", "https://ftp.eu.openbsd.org/pub/OpenBSD/"), ("7.7", "https://cdn.openbsd.org/pub/OpenBSD/"),
    ("7.8", "https://cdn.openbsd.org/pub/OpenBSD/"), ("7.9", "https://cdn.openbsd.org/pub/OpenBSD/"),
    ("8.0", "https://cdn.openbsd.org/pub/OpenBSD/"),
)


class Refused(Exception):
    """A download this module will not make (too large, no size given)."""


def http(url, byte_range=None, limit=None):
    request = urllib.request.Request(url, headers={"User-Agent": AGENT})
    if byte_range:
        request.add_header("Range", f"bytes={byte_range[0]}-{byte_range[1]}")
    for attempt in range(4):
        try:
            with urllib.request.urlopen(request, timeout=120) as response:
                total = response.headers.get("Content-Range", "").rpartition("/")[2] or response.headers.get("Content-Length")
                size = int(total) if total and total.isdigit() else None
                if byte_range and response.status != 206 and size is None:
                    raise Refused(f"{url}: no Range answer and no size, a cut prefix could not be told from a whole file")
                if limit is not None and size is not None and size > limit:
                    raise Refused(f"{url}: {size} bytes, more than the {limit} this download may take")
                # one byte more than asked for, so a server that ignores Range is noticed without reading its whole body
                want = byte_range[1] - byte_range[0] + 2 if byte_range else (limit + 1 if limit is not None else None)
                body = response.read(want)
                if limit is not None and not byte_range and len(body) > limit:  # no size announced, and too much of it
                    raise Refused(f"{url}: more than the {limit} bytes this download may take")
                expected = min(want - 1, size) if byte_range and size else (size if not limit or (size or 0) <= limit else None)
                if expected is not None and len(body) < expected:  # read(n) does not raise on a cut body
                    raise httpclient.IncompleteRead(body, expected - len(body))
                return body, size
        except urllib.error.HTTPError as error:
            if error.code < 500 and error.code != 429:  # a missing file stays missing
                raise
            if attempt == 3:
                raise
            time.sleep(5 * (attempt + 1))
        except (OSError, httpclient.HTTPException):  # timeouts and cut bodies: a slow mirror is not a failed corpus
            if attempt == 3:
                raise
            time.sleep(5 * (attempt + 1))


def listing(url):
    html = http(url, limit=LISTING_LIMIT)[0].decode("utf-8", "replace")
    return [n for n in re.findall(r'href="([^"?#][^"]*)"', html) if not n.startswith(("/", "http", "."))]


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def cache_path(digest):
    return CACHE / digest[:2] / digest


def store(data):
    digest = sha256(data)
    path = cache_path(digest)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    return digest


def parse_checksum(text, name):
    """The SHA-256 a checksum file lists for `name`, in either of the two formats in use, or None.

    BSD style `SHA256 (name) = hex` (OpenBSD) and `hex  name` (Debian, Alpine, coreutils);
    a leading `./` or `*` on the name is ignored.
    """
    for line in text.splitlines():
        bsd = re.match(r"^SHA256 \((.+)\) = ([0-9a-f]{64})$", line)
        gnu = re.match(r"^([0-9a-f]{64})\s+\*?(?:\./)?(.+)$", line)
        if bsd and bsd.group(1) == name:
            return bsd.group(2)
        if gnu and gnu.group(2) == name:
            return gnu.group(1)
    return None


def checksum_line(url, name):
    return parse_checksum(http(url, limit=LISTING_LIMIT)[0].decode("utf-8", "replace"), name)


def prefix_pin(pins, system, release, url, licence):
    data, total = http(url, (0, PREFIX_BYTES - 1))
    if len(data) > PREFIX_BYTES:
        raise Refused(f"{url}: the server ignored the Range request and sent {len(data)} bytes")
    pins.append(dict(system=system, release=release, url=url, mode="prefix", bytes=len(data),
                     size=total, sha256=store(data), verified_against="first fetch (trust on first use)",
                     licence=licence))


def archive_pin(pins, system, release, url, checksum_url, checksum_name, licence):
    published = checksum_line(checksum_url, checksum_name)
    cached = cache_path(published) if published else None
    data = cached.read_bytes() if cached and cached.is_file() else http(url, limit=LIMIT_FULL)[0]
    digest = sha256(data)
    if published != digest:
        raise SystemExit(f"{url}: SHA-256 {digest} is not the published {published}")
    store(data)
    pins.append(dict(system=system, release=release, url=url, mode="archive", bytes=len(data),
                     size=len(data), sha256=digest, verified_against=checksum_url, licence=licence))


def discover(pins):
    for version in ALPINE_RELEASES:
        series = ".".join(version.split(".")[:2])
        base = f"{ALPINE}/v{series}/releases/aarch64/netboot-{version}/"
        names = [n for n in listing(base) if not n.endswith("/")]
        for name in names:
            prefix_pin(pins, "alpine", version, base + name, "GPL-2.0-only (Linux), mixed (initramfs)")
        for tree in [n for n in listing(base) if n.startswith("dtbs-")][:2]:
            vendors = [v for v in listing(base + tree) if v.endswith("/")][:3]
            for vendor in vendors:
                for name in [n for n in listing(base + tree + vendor) if n.endswith(".dtb")][:4]:
                    prefix_pin(pins, "alpine", version, base + tree + vendor + name, "GPL-2.0-only OR MIT (device trees)")
    for version in ALPINE_ROOTFS:
        series = ".".join(version.split(".")[:2])
        name = f"alpine-minirootfs-{version}-aarch64.tar.gz"
        url = f"{ALPINE}/v{series}/releases/aarch64/{name}"
        archive_pin(pins, "alpine-rootfs", version, url, url + ".sha256", name, "mixed (MIT, GPL-2.0, ...)")
    for label, base in DEBIAN_INSTALLERS:
        archive_pin(pins, "debian", label, base + "netboot/netboot.tar.gz", base + "SHA256SUMS",
                    "netboot/netboot.tar.gz", "GPL-2.0-only (Linux), GPL-3.0-or-later (GRUB), mixed")
        for vendor in [v for v in listing(base + "device-tree/") if v.endswith("/")][:4]:
            for name in [n for n in listing(base + "device-tree/" + vendor) if n.endswith(".dtb")][:3]:
                prefix_pin(pins, "debian", label, base + "device-tree/" + vendor + name, "GPL-2.0-only OR MIT (device trees)")
    for system, release, base in EL_TREES:
        for path in ("images/pxeboot/vmlinuz", "images/pxeboot/initrd.img", "EFI/BOOT/BOOTAA64.EFI",
                     "EFI/BOOT/grubaa64.efi", "EFI/BOOT/mmaa64.efi", "EFI/BOOT/grub.cfg"):
            prefix_pin(pins, system, release, base + path, "GPL-2.0-only (Linux), GPL-3.0-or-later (GRUB), BSD-2-Clause (shim)")
    for release in OPENSUSE:
        base = f"https://download.opensuse.org/ports/aarch64/distribution/leap/{release}/repo/oss/boot/aarch64/"
        for name in ("linux", "initrd"):
            prefix_pin(pins, "opensuse", release, base + name, "GPL-2.0-only (Linux), mixed")
    for release, mirror in OPENBSD:
        base = f"{mirror}{release}/arm64/"
        archive_pin(pins, "openbsd", release, base + "BOOTAA64.EFI", base + "SHA256", "BOOTAA64.EFI", "ISC/BSD")
        for name in ("bsd", "bsd.mp", "bsd.rd"):
            prefix_pin(pins, "openbsd", release, base + name, "ISC/BSD")


def pin():
    pins = []
    discover(pins)
    PINS.parent.mkdir(exist_ok=True)
    PINS.write_text(json.dumps(pins, indent=1, sort_keys=True) + "\n")
    print(f"wrote {PINS} with {len(pins)} pins")


def load_pins():
    return json.loads(PINS.read_text())


def fetch():
    bad = 0
    for pin_ in load_pins():
        path = cache_path(pin_["sha256"])
        if path.is_file() and sha256(path.read_bytes()) == pin_["sha256"]:
            continue
        try:
            data = (http(pin_["url"], (0, pin_["bytes"] - 1))[0] if pin_["mode"] == "prefix"
                    else http(pin_["url"], limit=LIMIT_FULL)[0])
        except (OSError, httpclient.HTTPException, Refused) as error:
            print(f"UNREACHABLE {pin_['url']}: {error}", file=sys.stderr)
            bad += 1
            continue
        if pin_["mode"] == "prefix" and len(data) > pin_["bytes"]:
            print(f"{pin_['url']}: the server ignored the Range request", file=sys.stderr)
            bad += 1
            continue
        if sha256(data) != pin_["sha256"]:
            print(f"MISMATCH {pin_['url']}: {sha256(data)} != {pin_['sha256']}", file=sys.stderr)
            bad += 1
            continue
        store(data)
    if bad:
        raise SystemExit(f"{bad} pinned files do not match")
    print("corpus cache complete and verified")


def pinned_bytes(pin_):
    """The bytes of one pin, read from the cache and checked again."""
    data = cache_path(pin_["sha256"]).read_bytes()
    if sha256(data) != pin_["sha256"]:
        raise ValueError(f"cache entry of {pin_['url']} is corrupt; run fetch.py fetch")
    return data


if __name__ == "__main__":
    if sys.argv[1:] == ["pin"]:
        pin()
    elif sys.argv[1:] == ["fetch"]:
        fetch()
    else:
        raise SystemExit(__doc__)
