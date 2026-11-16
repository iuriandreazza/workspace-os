---
title: "ADR 0008: Own UEFI bootloader, written with Limine as the reference"
type: adr
status: accepted
created: 2026-11-15
updated: 2026-11-15
tags: [boot, bootloader, uefi, limine]
---

# ADR 0008: Own UEFI bootloader, written with Limine as the reference

## Context

Today there is no bootloader: QEMU loads the kernel's ELF file directly and
jumps to it. That only works in QEMU. VirtualBox, the other place the system
must run (ADR 0004), starts a guest through UEFI firmware from a disk.

The boot path also has to offer a menu, because recovery mode is entered
from it (ADR 0007), and it has to work on aarch64 now and on x86_64 later.

## Decision

Write our own bootloader, using the source of the Limine bootloader as the
reference for how it is done.

- **UEFI only.** No legacy BIOS path: both VirtualBox targets boot through
  UEFI.
- **Written in Rust**, as a crate in this repository, built for the UEFI
  target of each architecture.
- **What it does:** shows the boot menu, including the recovery entry; loads
  the kernel from the EFI system partition; collects the memory map and the
  firmware tables; leaves UEFI boot services; hands the kernel a boot
  information structure and jumps to it.
- **One handoff structure for both architectures**, defined in an ADR of its
  own before the code is written. The kernel reads memory layout and device
  information from it instead of assuming QEMU's.
- **QEMU's direct kernel loading stays** for the fast boots in the gate. The
  bootloader path is an additional boot in the gate, under QEMU with UEFI
  firmware.

## Licence

Limine is BSD-2-Clause, checked on its repository on 2026-11-15. That licence
allows its code to be reused in a GPL-2.0 project (ADR 0005), provided its
copyright notice and licence text are kept.

- Any file that copies or translates Limine code keeps Limine's copyright
  notice and the BSD-2-Clause text, next to our own.
- Code written from an understanding of how Limine works, without copying or
  translating it, carries only our own notice. The source it was learned from
  is still credited in the documentation.
- Rust crates used by the bootloader must be GPL-2.0 compatible, like every
  other dependency.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| GRUB | The best known; menu included; UEFI on both architectures | On ARM it has no protocol of its own for kernels that are not Linux: ours would have to imitate the Linux image format. GPL-3.0, acceptable only as a separate program |
| Limine as it is | Same protocol on both architectures; menu; hands over memory map and firmware tables | Ties the kernel to its protocol; the boot path would be code we did not write |
| Own bootloader with Limine as reference (chosen) | Full control; the boot path is ours end to end; article material | More work, and the menu and every firmware quirk are ours to handle |

## Consequences

- The kernel gains a second entry path. The boot code reviewed so far stays
  the path for direct loading; the bootloader enters the kernel with the
  handoff structure.
- Firmware differences between QEMU and VirtualBox become our problem. What
  VirtualBox's ARM machine provides has to be researched first.
- The x86_64 port (roadmap item K14) includes the bootloader for x86_64.

## Open questions

- The contents of the handoff structure.
- Whether the bootloader also verifies the kernel it loads, and against what.
