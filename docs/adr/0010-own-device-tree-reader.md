---
title: "ADR 0010: Own minimal device tree reader, with no allocation"
type: adr
status: accepted
created: 2026-11-15
updated: 2026-11-15
tags: [kernel, devicetree, boot, safety]
---

# ADR 0010: Own minimal device tree reader, with no allocation

## Context

The kernel learns the memory size and the UART from the device tree instead of
constants in `board.rs` (roadmap item K2). It needs them before it has an
allocator (K3 to K5), and the blob comes from outside the kernel, so it is
untrusted input.

## Decision

A small reader of our own, in the host-testable crate `devicetree`, instead of
a third-party crate.

- It never allocates: it walks the structure block in place.
- Every offset, length, token and string is checked against the bounds of the
  block it lives in. Unit tests cover truncation, bad headers, bad lengths and
  unterminated strings, and flip every byte of a valid blob to check that
  nothing panics.
- Scope: the first `reg` entry of the first enabled `memory` node and of the
  first enabled node with a given `compatible`, among the direct children of
  the root. Cell counts of 1 or 2 are supported. `reg` values are taken as CPU
  physical addresses, so nodes below another bus are ignored rather than
  translated. K3 will extend the reader to the full memory map.

Alternatives considered: `fdt` and `fdt-parser` style crates avoid writing
parsing code, but their allocation behaviour, unsafe use and licences each need
review (ADR 0005), and the untrusted-input handling is the part this project
wants to own and test. Revisit when the reader needs address translation or
interrupt maps.

## Finding the blob

The boot protocol hands the address in `x0`; `boot.s` keeps it and passes it to
`kernel_main`. QEMU's ELF loader (`-kernel`) and `-device loader` leave `x0`
zero but put the blob at the start of RAM (`0x40000000`, which is why the
kernel is linked at `0x40200000`). `board::discover` tries `x0` first when it
is non-zero, then the start of RAM, and reports the first error if both fail.
If neither works the kernel halts silently: there is no console yet.

## Consequences

- `board.rs` holds only the RAM-start fallback and the UART `compatible`.
- The UART address is no longer a constant, so output is dropped before
  discovery.
- The reader runs under `cargo test` on the host, in `scripts/check.sh`.
