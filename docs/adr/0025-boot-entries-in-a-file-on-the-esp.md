---
title: "ADR 0025: Hand-written boot entries for other kernels live in a file on the ESP"
type: adr
status: proposed
created: 2026-11-19
updated: 2026-11-19
tags: [boot, bootloader, uefi, linux, chainload]
---

# ADR 0025: Hand-written boot entries for other kernels live in a file on the ESP

## Context

K2d teaches the bootloader to start Linux and the EFI loader of another system
(ADR 0008). The boot model of ADR 0009 will one day write these entries; until
then a person writes them. Something has to say where an entry is written
down. Nothing in ADR 0008 or 0009 settles that.

The entries for our own kernel (Normal, Recovery) are not affected: they stay
compiled in, first in the menu, and need nothing else (ADR 0002, ADR 0007).

## Decision

Entries for other systems are read from a text file, `\WSOS.CFG`, in the root of
the EFI system partition.

- No file: the menu holds only the built-in entries.
- A file with an error: the bootloader says which line is wrong, shows the
  built-in entries only, and boots as usual. A broken file can never hide
  our own kernel.
- At most seven entries from the file, so that every entry has a digit key
  (the built-in two take 1 and 2).
- Format: one `key value` per line, `#` starts a comment; an `entry <label>`
  line opens an entry, and the keys after it belong to it.

```
entry Alpine Linux
linux \VMLINUZ
cmdline console=ttyAMA0 earlycon
initrd \INITRD.IMG

entry Other system
chainload \EFI\OTHER\BOOTAA64.EFI
```

- Paths are on the EFI system partition the bootloader started from, with
  backslashes. Another partition is not supported in this item; it needs the
  partition's GUID in the entry and a test image with a second partition, and
  is left for the item that needs it.
- The loader reads the whole image file, checks that it is a PE image for
  aarch64 with the EFI application subsystem (ADR 0009, point 4), and gives
  those same bytes to `LoadImage`, so what was checked is what is started. A
  failure of any kind prints one message and returns to the menu, with the
  countdown stopped.
- Linux gets its command line through `LoadOptions` and its initial ramdisk
  through a `LoadFile2` protocol under the vendor media device path the Linux
  stub looks for. The sources are in `docs/k2d-other-kernels.md`.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| Entries compiled into `bootinfo` (like `BUILT_IN`) | No parser, nothing to misread at boot | Changing an entry means rebuilding and re-signing the bootloader; the gate would need one bootloader build per case (each refusal, with and without an initrd) |
| A file on the ESP (chosen) | One bootloader build serves every test and every machine; the installer and later the boot model write the same file; the parser is plain Rust tested on the host | A parser in the boot path (small, bounded, and its failure falls back to the built-in entries) |
| UEFI `Boot####` variables | The firmware's own mechanism | Not reachable from QEMU images without a variable store; not readable by a person; nothing to test in the gate |

## Consequences

- `bootinfo` gains the entry kinds, the file parser and the PE check, all
  tested on the host. The bootloader only talks to the firmware.
- The file is trusted as much as the ESP is. Secure Boot and the integrity of
  the entries are not designed here (see B4 for the model's own weights).
- The boot model (B3) writes entries in this format, or replaces the parser
  with its own source of entries; the menu does not change either way.
