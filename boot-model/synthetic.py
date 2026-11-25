"""Files that are generated, not downloaded: the kinds of junk found on a boot
partition, entry text in the Boot Loader Specification format, and the layouts of
partition starts (B2). Everything is deterministic from a seed.

Boot Loader Specification text is written here from the specification
(`uapi-group/specifications`), not copied from a machine: a stated limit of the
corpus, because no distribution image that carries real `loader/entries` was
downloaded.
"""
import gzip
import io
import lzma
import random
import struct
import tarfile
import zipfile
import zlib

WORDS = ("kernel boot system update package network firmware driver config data backup image logo theme "
         "recovery install release notes version module loader disk partition cache swap root user").split()


def text(rng, size):
    out = []
    length = 0
    while length < size:
        word = rng.choice(WORDS)
        out.append(word)
        length += len(word) + 1
    return " ".join(out).encode()[:size]


def png(rng):
    width = height = rng.choice((16, 32, 64))
    rows = b"".join(b"\0" + bytes(rng.randrange(256) for _ in range(width * 3)) for _ in range(height))

    def chunk(kind, body):
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body))

    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))


def bmp(rng):
    pixels = bytes(rng.randrange(256) for _ in range(16 * 16 * 3))
    return b"BM" + struct.pack("<IHHI", 54 + len(pixels), 0, 0, 54) + struct.pack(
        "<IiiHHIIiiII", 40, 16, 16, 1, 24, 0, len(pixels), 2835, 2835, 0, 0) + pixels


def zip_file(rng):
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w", zipfile.ZIP_DEFLATED) as archive:
        # a fixed timestamp: the default is the current time, which would make every run's junk differ
        archive.writestr(zipfile.ZipInfo("notes.txt", date_time=(2026, 1, 1, 0, 0, 0)), text(rng, 2000), zipfile.ZIP_DEFLATED)
    return buffer.getvalue()


def tar_file(rng):
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w") as archive:
        body = text(rng, 1500)
        info = tarfile.TarInfo("data/notes.txt")
        info.size = len(body)
        archive.addfile(info, io.BytesIO(body))
    return buffer.getvalue()


def gzip_file(rng):
    return gzip.compress(text(rng, 6000), mtime=0)


def xz_file(rng):
    return lzma.compress(text(rng, 6000))


def random_bytes(rng):
    return bytes(rng.randrange(256) for _ in range(4096))


def zeros(rng):
    return bytes(4096)


def pdf(rng):
    return b"%PDF-1.7\n" + text(rng, 800) + b"\n%%EOF\n"


def shell_script(rng):
    return b"#!/bin/sh\n" + b"\n".join(b"echo " + text(rng, 12) for _ in range(20)) + b"\n"


def model_file(rng):
    """Stand-in for our own `\\WSOS.MDL`: the weights file lies on the same partition."""
    return b"WSMD" + struct.pack("<I", 1) + bytes(rng.randrange(256) for _ in range(4000))


JUNK_KINDS = {
    "text": (lambda rng: text(rng, 3000), ("README.TXT", "notes.txt", "LICENSE", "CHANGELOG.md", "readme.md")),
    "png": (png, ("logo.png", "splash.png", "background.png", "icon.png")),
    "bmp": (bmp, ("splash.bmp", "logo.bmp")),
    "zip": (zip_file, ("backup.zip", "drivers.zip")),
    "tar": (tar_file, ("backup.tar", "data.tar")),
    "gzip": (gzip_file, ("backup.tar.gz", "data.gz", "firmware.gz", "log.gz")),
    "xz": (xz_file, ("data.xz", "backup.tar.xz")),
    "random": (random_bytes, ("data.bin", "blob.bin", "firmware.bin", "swap.img")),
    "zeros": (zeros, ("empty.bin", "padding.bin")),
    "pdf": (pdf, ("manual.pdf",)),
    "script": (shell_script, ("startup.nsh", "install.sh", "boot.sh")),
    "model": (model_file, ("WSOS.MDL",)),
}


def junk_assets(count, seed):
    """`[(kind, file name, bytes)]`, deterministic."""
    rng = random.Random(seed)
    out = []
    for index in range(count):
        kind = rng.choice(sorted(JUNK_KINDS))
        build, names = JUNK_KINDS[kind]
        out.append((kind, rng.choice(names), build(rng)))
    return out


# Files in formats that none of the training or evaluation negatives use (B2b, outlier exposure): the kinds of file a
# model that says "I do not know" should not be forced to class. The magic numbers are the published ones.
OUTLIER_KINDS = {
    "squashfs": (b"hsqs", ".sqfs"), "7z": (b"7z\xbc\xaf\x27\x1c", ".7z"), "rar": (b"Rar!\x1a\x07\x01\x00", ".rar"),
    "sqlite": (b"SQLite format 3\x00", ".db"), "jpeg": (b"\xff\xd8\xff\xe0\x00\x10JFIF", ".jpg"),
    "mp4": (b"\x00\x00\x00\x18ftypmp42", ".mp4"), "ogg": (b"OggS\x00\x02", ".ogg"),
    "wav": (b"RIFF\x24\x08\x00\x00WAVEfmt ", ".wav"), "cab": (b"MSCF\x00\x00\x00\x00", ".cab"),
    "hdf5": (b"\x89HDF\r\n\x1a\n", ".h5"), "class": (b"\xca\xfe\xba\xbe\x00\x00\x00\x34", ".class"),
    "wasm": (b"\x00asm\x01\x00\x00\x00", ".wasm"), "pcap": (b"\xd4\xc3\xb2\xa1\x02\x00\x04\x00", ".pcap"),
    "flac": (b"fLaC\x00\x00\x00\x22", ".flac"), "iso": (bytes(2048), ".iso"), "lzip": (b"LZIP\x01", ".lz"),
}
OUTLIER_WORDS = ("archive", "export", "sample", "media", "capture", "bundle", "snapshot", "dump", "track", "report")


def outlier_files(count, seed):
    """`[(path, bytes)]`: a magic number of a format outside every negative of the corpus, then random bytes or text."""
    rng = random.Random(seed)
    out = []
    for _ in range(count):
        magic, extension = OUTLIER_KINDS[rng.choice(sorted(OUTLIER_KINDS))]
        body = random_bytes(rng) if rng.random() < 0.7 else text(rng, 3000)
        folder = rng.choice(("", "\\media", "\\export", "\\EFI\\data"))
        out.append((f"{folder}\\{rng.choice(OUTLIER_WORDS)}-{rng.randrange(1000)}{extension}", (magic + body)[:4096]))
    return out


def bls_entry(rng, family, linux, initrd=None, devicetree=None, version=None):
    """Entry text in the format of the Boot Loader Specification, type 1.

    `linux`, `initrd` and `devicetree` are the paths the entry names, with `/`, relative to the volume root: the
    caller passes the paths of the files the disk really holds, so the text agrees with the layout.
    """
    version = version or f"{rng.randrange(4, 7)}.{rng.randrange(0, 20)}.{rng.randrange(0, 200)}"
    root = rng.choice((f"root=UUID={'%08x-%04x-%04x-%04x-%012x' % (rng.getrandbits(32), rng.getrandbits(16), rng.getrandbits(16), rng.getrandbits(16), rng.getrandbits(48))}",
                       "root=/dev/vda2", "root=LABEL=rootfs"))
    options = f"{root} ro {rng.choice(('quiet', 'console=ttyAMA0', 'earlycon', 'rhgb quiet'))}"
    title = rng.choice(("Fedora Linux", "openSUSE Leap", "Arch Linux", "Debian GNU/Linux", "Alpine Linux"))
    lines = [f"title {title} ({version})", f"version {version}", f"linux {linux}"]
    if initrd:
        lines.append(f"initrd {initrd}")
    if devicetree:
        lines.append(f"devicetree {devicetree}")
    lines += [f"options {options}", "architecture aa64"]
    if rng.random() < 0.3:
        lines.insert(2, f"sort-key {family}")
    return ("\n".join(lines) + "\n").encode()


# First bytes of a partition (432 are visible to the model after the 16-byte type GUID).
def fat32_boot_sector(rng):
    head = b"\xeb\x58\x90" + rng.choice((b"mkfs.fat", b"MSDOS5.0", b"BSD  4.4")) + struct.pack(
        "<HBHBHHBHHIIIHHIHH", 512, rng.choice((1, 2, 4, 8)), 32, 2, 0, 0, 0xF8, 0, 32, 0, 1 << 20, 1 << 13, 0, 0, 2, 1, 6)
    sector = head + bytes(12) + b"\x80\x00\x29" + struct.pack("<I", rng.getrandbits(32)) + b"NO NAME    FAT32   "
    return sector + bytes(510 - len(sector)) + b"\x55\xaa"


def ntfs_boot_sector(rng):
    sector = b"\xeb\x52\x90NTFS    " + struct.pack("<HBH", 512, 8, 0)
    return sector + bytes(510 - len(sector)) + b"\x55\xaa"


def xfs_start(rng):
    return b"XFSB" + struct.pack(">I", 4096) + bytes(500)


def zero_start(rng):
    return bytes(512)


# (name, type GUID in the byte order stored on disk, start-of-partition builder) for the partitions
# a disk may carry. GUIDs are the published ones (Discoverable Partitions Specification, Microsoft,
# FreeBSD and OpenBSD documentation).
def _guid(text_):
    a, b, c, d, e = text_.split("-")
    return struct.pack("<IHH", int(a, 16), int(b, 16), int(c, 16)) + bytes.fromhex(d + e)


PARTITIONS = (
    (("EFI System Partition", "EFI", "ESP", "boot"), _guid("C12A7328-F81F-11D2-BA4B-00A0C93EC93B"), fat32_boot_sector),
    (("rootfs", "root", "Linux filesystem", "alpine-root"), _guid("B921B045-1DF0-41C3-AF44-4C6F280D3FAE"), zero_start),
    (("home", "data", "Linux filesystem"), _guid("0FC63DAF-8483-4772-8E79-3D69D8477DE4"), zero_start),
    (("swap", "Linux swap"), _guid("0657FD6D-A4AB-43C4-84E5-0933C84B4F4F"), zero_start),
    (("XBOOTLDR", "boot"), _guid("BC13C2FF-59E6-4262-A352-B275FD6F7172"), fat32_boot_sector),
    (("Basic data partition", "Windows"), _guid("EBD0A0A2-B9E5-4433-87C0-68B6B72699C7"), ntfs_boot_sector),
    (("Microsoft reserved partition",), _guid("E3C9E316-0B5C-4DB8-817D-F92DF00215AE"), zero_start),
    (("data",), _guid("0FC63DAF-8483-4772-8E79-3D69D8477DE4"), xfs_start),
    (("freebsd-ufs", "root"), _guid("516E7CB6-6ECF-11D6-8FF8-00022D09712B"), zero_start),
    (("openbsd", "root"), _guid("824CC7A0-36A8-11E3-890A-952519AD3F61"), zero_start),
)
