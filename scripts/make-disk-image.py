#!/usr/bin/env python3
"""Writes a bootable UEFI disk image: a GPT disk with one EFI system partition
(FAT32) holding the bootloader at the removable-media path and the kernel.

    make-disk-image.py OUTPUT BOOTLOADER.EFI [KERNEL.ELF] [--file \\ESP\\PATH=LOCAL]...

Without a kernel the partition holds the bootloader only, which is what the
boot smoke test needs to see the bootloader's refusal of a missing kernel.
Each `--file` adds one more file at that path of the partition (directories are
made as needed): the boot entries of other systems and the images they name.

The image is written without root and without external tools, so it builds the
same way on macOS and on Linux. Only what UEFI firmware reads is written:
file names are 8.3 (no long names), every file is contiguous, and the rest of
the image is a sparse hole. The image is never committed (see .gitignore).
"""
import struct
import sys
import uuid
import zlib

SECTOR = 512
DISK_SECTORS = 64 * 1024 * 1024 // SECTOR
PARTITION_START = 2048
GPT_ENTRIES_SECTORS = 32
FIRST_USABLE = 2 + GPT_ENTRIES_SECTORS
PARTITION_END = DISK_SECTORS - 1 - GPT_ENTRIES_SECTORS - 1  # last usable LBA
PARTITION_SECTORS = PARTITION_END - PARTITION_START + 1
RESERVED_SECTORS = 32
CLUSTER_SECTORS = 1
EFI_SYSTEM_PARTITION = uuid.UUID("C12A7328-F81F-11D2-BA4B-00A0C93EC93B")
# Fixed identifiers, so that the same inputs give the same image.
DISK_GUID = uuid.UUID("5e0e5b1c-7a3c-4a33-9f2d-0a1c3a6b5d01")
PARTITION_GUID = uuid.UUID("5e0e5b1c-7a3c-4a33-9f2d-0a1c3a6b5d02")
VOLUME_ID = 0x57534F53  # "WSOS"


def fat_geometry():
    # Each FAT entry is 4 bytes: solve for the FAT size that covers the clusters
    # left after the two FATs.
    fat_sectors = 1
    while True:
        clusters = (PARTITION_SECTORS - RESERVED_SECTORS - 2 * fat_sectors) // CLUSTER_SECTORS
        needed = (clusters + 2) * 4 // SECTOR + 1
        if needed <= fat_sectors:
            return fat_sectors, clusters
        fat_sectors = needed


FAT_SECTORS, CLUSTERS = fat_geometry()
assert CLUSTERS >= 65525, "too few clusters for FAT32"
DATA_START = RESERVED_SECTORS + 2 * FAT_SECTORS  # relative to the partition


class Volume:
    def __init__(self):
        self.fat = [0] * (CLUSTERS + 2)
        self.fat[0] = 0x0FFFFFF8
        self.fat[1] = 0x0FFFFFFF
        self.data = {}  # cluster number -> bytes
        self.next_cluster = 2

    def allocate(self, payload):
        """Stores payload in a contiguous run of clusters; returns the first."""
        size = CLUSTER_SECTORS * SECTOR
        count = max(1, -(-len(payload) // size))
        first = self.next_cluster
        if first + count > CLUSTERS + 2:
            raise SystemExit("the image is too small for its files")
        for i in range(count):
            cluster = first + i
            self.fat[cluster] = cluster + 1 if i < count - 1 else 0x0FFFFFFF
            self.data[cluster] = payload[i * size:(i + 1) * size].ljust(size, b"\0")
        self.next_cluster += count
        return first


def short_name(name):
    base, _, ext = name.partition(".")
    assert len(base) <= 8 and len(ext) <= 3 and name == name.upper(), name
    return base.ljust(8).encode() + ext.ljust(3).encode()


def raw_entry(name11, attributes, cluster, size):
    # 1 January 2026, 00:00: the date and time are fixed for reproducibility.
    date = ((2026 - 1980) << 9) | (1 << 5) | 1
    return name11 + struct.pack(
        "<BBBHHHHHHHI", attributes, 0, 0, 0, date, date, cluster >> 16, 0, date, cluster & 0xFFFF, size
    )


def entry(name, attributes, cluster, size):
    return raw_entry(short_name(name), attributes, cluster, size)


def directory_data(own_cluster, parent_cluster, entries):
    """A directory's cluster: '.', '..' (parent 0 means the root) and the entries."""
    content = raw_entry(b".".ljust(11), 0x10, own_cluster, 0)
    content += raw_entry(b"..".ljust(11), 0x10, parent_cluster, 0)
    return (content + b"".join(entries)).ljust(CLUSTER_SECTORS * SECTOR, b"\0")


def build_volume(files):
    """files maps a path such as "EFI/BOOT/BOOTAA64.EFI" to its bytes."""
    tree = {}
    for path, payload in files.items():
        *directories, name = path.split("/")
        node = tree
        for directory in directories:
            node = node.setdefault(directory, {})
        node[name] = payload
    volume = Volume()
    root = volume.allocate(b"")  # cluster 2, as the boot sector says

    def fill(node, own, parent):
        # Every directory fits the one cluster it gets: 16 entries of 32 bytes,
        # two of them taken by '.' and '..' below the root.
        children = []
        for name, content in node.items():
            if isinstance(content, dict):
                cluster = volume.allocate(b"")
                children.append((name, 0x10, cluster, 0, content))
            else:
                children.append((name, 0x20, volume.allocate(content), len(content), None))
        entries = [entry(name, attributes, cluster, size) for name, attributes, cluster, size, _ in children]
        if own == root:
            # The root has no '.' or '..'.
            assert len(entries) <= CLUSTER_SECTORS * SECTOR // 32, "too many files in the root"
            volume.data[own] = b"".join(entries).ljust(CLUSTER_SECTORS * SECTOR, b"\0")
        else:
            assert len(entries) <= CLUSTER_SECTORS * SECTOR // 32 - 2, "too many files in a directory"
            volume.data[own] = directory_data(own, parent, entries)
        for _, _, cluster, _, subtree in children:
            if subtree is not None:
                fill(subtree, cluster, 0 if own == root else own)

    fill(tree, root, 0)
    return volume


def boot_sector():
    bpb = b"\xEB\x58\x90" + b"WSOSBOOT"
    bpb += struct.pack(
        "<HBHBHHBHHHIIIHHIHH12x",
        SECTOR, CLUSTER_SECTORS, RESERVED_SECTORS, 2, 0, 0, 0xF8, 0, 63, 255,
        PARTITION_START, PARTITION_SECTORS, FAT_SECTORS, 0, 0, 2, 1, 6,
    )
    bpb += struct.pack("<BBBI11s8s", 0x80, 0, 0x29, VOLUME_ID, b"WORKSPACEOS", b"FAT32   ")
    return bpb.ljust(510, b"\0") + b"\x55\xAA"


def fs_info(free_clusters, next_free):
    sector = struct.pack("<I", 0x41615252).ljust(484, b"\0")
    sector += struct.pack("<IIII", 0x61417272, free_clusters, next_free, 0)
    return sector.ljust(508, b"\0") + struct.pack("<I", 0xAA550000)


def write_partition(image, volume):
    base = PARTITION_START * SECTOR
    boot = boot_sector()
    free = CLUSTERS - (volume.next_cluster - 2)
    for sector, content in ((0, boot), (1, fs_info(free, volume.next_cluster)),
                            (6, boot), (7, fs_info(free, volume.next_cluster))):
        image.seek(base + sector * SECTOR)
        image.write(content)
    fat = b"".join(struct.pack("<I", value) for value in volume.fat).ljust(FAT_SECTORS * SECTOR, b"\0")
    for copy in range(2):
        image.seek(base + (RESERVED_SECTORS + copy * FAT_SECTORS) * SECTOR)
        image.write(fat)
    for cluster, payload in volume.data.items():
        image.seek(base + (DATA_START + (cluster - 2) * CLUSTER_SECTORS) * SECTOR)
        image.write(payload)


def gpt_entry():
    name = "EFI system partition".encode("utf-16-le").ljust(72, b"\0")
    return (
        EFI_SYSTEM_PARTITION.bytes_le + PARTITION_GUID.bytes_le
        + struct.pack("<QQQ", PARTITION_START, PARTITION_END, 0) + name
    ).ljust(128, b"\0")


def gpt_header(current, other, entries_lba, entries_crc):
    header = struct.pack(
        "<8sIIIIQQQQ16sQIII",
        b"EFI PART", 0x00010000, 92, 0, 0, current, other,
        FIRST_USABLE, PARTITION_END,
        DISK_GUID.bytes_le, entries_lba, 128, 128, entries_crc,
    )
    crc = zlib.crc32(header[:16] + b"\0\0\0\0" + header[20:]) & 0xFFFFFFFF
    return (header[:16] + struct.pack("<I", crc) + header[20:]).ljust(SECTOR, b"\0")


def write_gpt(image):
    mbr = bytearray(SECTOR)
    # Protective MBR: one partition of type 0xEE that spans the disk.
    mbr[446:462] = struct.pack("<BBBBBBBBII", 0, 0, 2, 0, 0xEE, 0xFF, 0xFF, 0xFF, 1, min(DISK_SECTORS - 1, 0xFFFFFFFF))
    mbr[510:512] = b"\x55\xAA"
    image.seek(0)
    image.write(mbr)
    entries = gpt_entry().ljust(128 * 128, b"\0")
    entries_crc = zlib.crc32(entries) & 0xFFFFFFFF
    image.seek(SECTOR)
    image.write(gpt_header(1, DISK_SECTORS - 1, 2, entries_crc))
    image.seek(2 * SECTOR)
    image.write(entries)
    backup_entries_lba = DISK_SECTORS - 1 - GPT_ENTRIES_SECTORS
    image.seek(backup_entries_lba * SECTOR)
    image.write(entries)
    image.seek((DISK_SECTORS - 1) * SECTOR)
    image.write(gpt_header(DISK_SECTORS - 1, 1, backup_entries_lba, entries_crc))


def main():
    # Extra files go in with `--file \\ESP\\PATH=LOCAL`, e.g. a Linux image.
    arguments = sys.argv[1:]
    extras = {}
    positional = []
    while arguments:
        argument = arguments.pop(0)
        if argument == "--file":
            if not arguments or "=" not in arguments[0]:
                raise SystemExit(__doc__)
            target, _, local = arguments.pop(0).partition("=")
            if not target.startswith("\\"):
                raise SystemExit("an ESP path starts with a backslash: " + target)
            with open(local, "rb") as f:
                extras[target[1:].replace("\\", "/")] = f.read()
        else:
            positional.append(argument)
    if len(positional) not in (2, 3):
        raise SystemExit(__doc__)
    output, bootloader_path = positional[:2]
    files = {}
    with open(bootloader_path, "rb") as f:
        files["EFI/BOOT/BOOTAA64.EFI"] = f.read()
    if len(positional) == 3:
        with open(positional[2], "rb") as f:
            files["KERNEL.ELF"] = f.read()
    for path in extras:
        if path in files:
            raise SystemExit("--file would replace the bootloader or the kernel: \\" + path.replace("/", "\\"))
    files.update(extras)
    volume = build_volume(files)
    with open(output, "wb") as image:
        image.truncate(DISK_SECTORS * SECTOR)
        write_gpt(image)
        write_partition(image, volume)


if __name__ == "__main__":
    main()
