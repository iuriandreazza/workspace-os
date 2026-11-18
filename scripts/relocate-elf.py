#!/usr/bin/env python3
"""Writes a copy of a kernel ELF whose loadable segments start at another
address, for the boot smoke test: the bootloader must refuse a kernel whose
load address the firmware does not give it. The code is not changed, so the
copy is not meant to run.

    relocate-elf.py KERNEL.ELF OUTPUT NEW_BASE
"""
import struct
import sys

PT_LOAD = 1
E_ENTRY, E_PHOFF, E_PHENTSIZE = 24, 32, 54
P_VADDR, P_PADDR = 16, 24


def main():
    if len(sys.argv) != 4:
        raise SystemExit(__doc__)
    source, target, new_base = sys.argv[1], sys.argv[2], int(sys.argv[3], 0)
    with open(source, "rb") as f:
        elf = bytearray(f.read())
    entry = struct.unpack_from("<Q", elf, E_ENTRY)[0]
    phoff = struct.unpack_from("<Q", elf, E_PHOFF)[0]
    phentsize, phnum = struct.unpack_from("<HH", elf, E_PHENTSIZE)
    headers = [phoff + i * phentsize for i in range(phnum)]
    loads = [h for h in headers if struct.unpack_from("<I", elf, h)[0] == PT_LOAD]
    delta = new_base - min(struct.unpack_from("<Q", elf, h + P_PADDR)[0] for h in loads)
    for header in loads:
        for field in (P_VADDR, P_PADDR):
            value = struct.unpack_from("<Q", elf, header + field)[0]
            struct.pack_into("<Q", elf, header + field, value + delta)
    struct.pack_into("<Q", elf, E_ENTRY, entry + delta)
    with open(target, "wb") as f:
        f.write(elf)


if __name__ == "__main__":
    main()
