"""Format augmentation of the corpus (B2b, option A of the accuracy note): the same kind of file in the container
formats the training releases do not ship.

B2's training set holds initrds only as gzip (and xz in the releases that were held out together), kernels only as
PE images, one bare gzip. Here a *payload* is built from files that are always in the training split (the Alpine
minirootfs releases: real aarch64 binaries and libraries) and encoded as a real stream of each format, and only the
first 4 KiB of the stream are kept, as the corpus does for every file:

- an initrd: a `newc` cpio archive of the rootfs, as dracut and mkinitramfs write one, in gzip, xz, bzip2, zstd or
  lz4 (the frame format and the legacy one the kernel's `lz4 -l` writes);
- a kernel: the 4 KiB header of a real kernel of the training fold, followed by aarch64 code of the rootfs, in the
  same formats, which the loader cannot start (protocol `none`, `catalogue.refine`).

Why not recompress the prefix: a stream made from 4 KiB has a first block, a LZMA2 chunk and a zstd frame header
that describe 4 KiB, and a real initrd's describe megabytes. A payload of megabytes gives the real framing.

Nothing here reads an asset of a held-out family. gzip, xz and bzip2 are in the standard library; zstd and lz4 need
the `zstandard` and `lz4` packages (BSD), imported when used, so the gate tests only the standard-library formats.
"""
import bz2
import copy
import gzip
import io
import lzma
import random
import struct
import tarfile

import catalogue

FORMATS = ("gzip", "xz", "bzip2", "zstd", "lz4")
EXTENSION = {"gzip": ".gz", "xz": ".xz", "bzip2": ".bz2", "zstd": ".zst", "lz4": ".lz4"}
PREFIX = 4096
# `None` keeps the real bytes: the original format stays in the mix.
INITRD_CHOICES = (None,) + FORMATS
KERNEL_CHOICES = (None, None) + FORMATS


def cpio_newc(members):
    """A `newc` cpio archive of `[(name, mode, data)]`, with the trailer."""
    out = io.BytesIO()
    for index, (name, mode, data) in enumerate(list(members) + [("TRAILER!!!", 0, b"")]):
        encoded = name.encode() + b"\0"
        fields = (index + 1, mode, 0, 0, 1, 0, len(data), 0, 0, 0, 0, len(encoded), 0)
        out.write(b"070701" + b"".join(b"%08X" % f for f in fields) + encoded)
        out.write(bytes(-(110 + len(encoded)) % 4))
        out.write(data)
        out.write(bytes(-len(data) % 4))
    return out.getvalue()


def rootfs_members(tarballs):
    """`(name, mode, data, is_elf)` of the regular files of Alpine minirootfs tarballs (a list of bytes)."""
    found = []
    for tar_bytes in tarballs:
        with tarfile.open(fileobj=io.BytesIO(tar_bytes)) as archive:
            for member in archive.getmembers():
                if member.isfile() and member.size:
                    data = archive.extractfile(member).read()
                    found.append((member.name.lstrip("./"), 0o100000 | (member.mode & 0o777), data,
                                  data[:4] == b"\x7fELF"))
    return found


def initrd_payload(members, seed, limit=3 << 20):
    """A cpio of a random subset of the rootfs in a random order, `.` first as dracut writes it."""
    rng = random.Random(seed)
    chosen = [m for m in members if rng.random() < 0.7]
    rng.shuffle(chosen)
    total, kept = 0, []
    for name, mode, data, _ in chosen:
        if total + len(data) > limit:
            continue
        kept.append((name, mode, data))
        total += len(data)
    return cpio_newc([(".", 0o040755, b"")] + kept)


def code_payload(members, seed, limit=1 << 20):
    """Machine code for the body of a kernel: the executables and libraries of the rootfs, in a random order."""
    rng = random.Random(seed)
    code = [data for _, _, data, is_elf in members if is_elf]
    rng.shuffle(code)
    out = b""
    for data in code:
        if len(out) + len(data) > limit:
            break
        out += data
    return out


def encode(fmt, data, variant=0):
    """`data` as a stream of `fmt`; `variant` picks the options a distribution may have chosen."""
    if fmt == "gzip":
        return gzip.compress(data, compresslevel=(6, 9)[variant % 2], mtime=0)
    if fmt == "xz":
        check = (lzma.CHECK_CRC32, lzma.CHECK_CRC64)[variant % 2]  # dracut writes crc32, xz's own default is crc64
        return lzma.compress(data, format=lzma.FORMAT_XZ, check=check, preset=(6, 9)[(variant // 2) % 2])
    if fmt == "bzip2":
        return bz2.compress(data, (9, 6)[variant % 2])
    if fmt == "zstd":
        import zstandard
        return zstandard.ZstdCompressor(level=(3, 19)[variant % 2], write_content_size=bool((variant // 2) % 2)).compress(data)
    if fmt == "lz4":
        import lz4.block
        import lz4.frame
        if variant % 2:
            return lz4.frame.compress(data, compression_level=9, content_checksum=bool((variant // 2) % 2))
        # the legacy format: magic, then blocks of at most 8 MiB, each preceded by its compressed size
        out = struct.pack("<I", 0x184C2102)
        for start in range(0, len(data), 8 << 20):
            block = lz4.block.compress(data[start:start + (8 << 20)], mode="high_compression", store_size=False)
            out += struct.pack("<I", len(block)) + block
        return out
    raise ValueError(f"unknown format {fmt!r}")


def stream_prefix(fmt, data, variant=0):
    return encode(fmt, data, variant)[:PREFIX]


def reencoded(asset, fmt, payload, variant):
    """A copy of an initrd or kernel asset whose bytes are the first 4 KiB of a stream of `fmt`.

    A kernel keeps its own first 4 KiB at the head of the payload, then is relabelled by `catalogue.refine`
    (a kernel that is not a PE file cannot be started). An initrd's name takes the compressor's extension where it
    had one (`initrd.gz`), so name and bytes agree as they do on a real disk.
    """
    new = copy.copy(asset)
    if asset.role == "kernel":
        new.content = stream_prefix(fmt, asset.content + payload, variant)
        new.family, new.protocol, new.role = catalogue.refine((asset.family, "linux", asset.role), new.content)
    else:
        new.content = stream_prefix(fmt, payload, variant)
        if asset.name.endswith(".gz"):
            new.alias = asset.name[:-3] + EXTENSION[fmt]  # `name` stays the key the layouts look the file up by
    return new


def augment_assets(assets, rootfs_tarballs, seed, copies=2):
    """Extra releases of the training assets with some initrds and kernels re-encoded.

    `assets` must already be the assets of the training fold; every release that holds a Linux kernel or an initrd
    is copied `copies` times with a random format for each (or none, which keeps the real bytes), under the release
    name `<release>~<kernel format>/<initrd format>`, so the layouts draw them like any other release.
    """
    members = rootfs_members(rootfs_tarballs)
    by_release = {}
    for a in assets:
        by_release.setdefault((a.system, a.release), []).append(a)
    extra = []
    for (system, release), group in sorted(by_release.items()):
        if not any(a.role == "initrd" or (a.role == "kernel" and a.family == "linux") for a in group):
            continue
        rng = random.Random(f"{seed}/{system}/{release}")  # per release: the same whatever else is in `assets`
        for _ in range(copies):
            kernel_format, initrd_format = rng.choice(KERNEL_CHOICES), rng.choice(INITRD_CHOICES)
            variant = rng.randrange(8)
            tag = f"{release}~{kernel_format or 'real'}/{initrd_format or 'real'}"
            for a in group:
                new = copy.copy(a)
                new.release = tag
                if a.role == "kernel" and a.family == "linux" and kernel_format:
                    new = reencoded(new, kernel_format, code_payload(members, rng.randrange(1 << 30)), variant)
                elif a.role == "initrd" and initrd_format:
                    new = reencoded(new, initrd_format, initrd_payload(members, rng.randrange(1 << 30)), variant)
                extra.append(new)
    return extra
