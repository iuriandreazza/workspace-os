---
title: "ADR 0012: Physical frame allocator on a bitmap in the kernel image"
type: adr
status: accepted
created: 2026-11-15
updated: 2026-11-15
tags: [kernel, memory, safety]
---

# ADR 0012: Physical frame allocator on a bitmap in the kernel image

> **Amended by [ADR 0030](0030-linear-map-and-virtual-pointers.md):** `FrameAllocator::release` and `alloc_in` exist; boot memory is reclaimed.

## Context

The MMU (K4) needs frames for page tables and the kernel heap (K5) needs
frames for its arenas, before any heap exists. The memory map and the
reserved regions come from the device tree, which is untrusted input
(ADR 0010).

## Decision

A bitmap allocator in the host-testable crate `physmem`, with no allocation.

- One bit per 4 KiB frame over a window of 1 Mi frames (4 GiB) that starts at
  the lowest RAM address. The storage is a static array in `.bss` (256 KiB),
  so it is part of the kernel image and is reserved with it. RAM beyond the
  window is reported and left unused.
- Two bitmaps: `available` and `allocated`. Only a frame that `alloc` handed
  out can be freed, so a stray or repeated `free` cannot turn a reserved
  frame, the kernel image or the device tree blob into an available one.
- The map is built from independent `add_memory` and `reserve` calls in any
  order, and a reservation always wins. Memory is rounded inward and
  reservations outward, so a partial frame is never handed out. Zero-sized,
  overlapping, wrapping and out-of-window ranges can only reduce what is
  available.
- Reserved: the kernel image (linker symbols `__image_start` to
  `__stack_bottom`), the boot stack, the device tree blob, `/memreserve/`
  entries and the children of `reserved-memory` that have a `reg`.
- Lowest address first, with a cursor that `free` rewinds, so allocation is
  deterministic and a freed frame is reused at once.
- The allocator is an owned value passed to whoever needs it, not a global.
  K4 receives `&mut FrameAllocator`. Amended by ADR 0029: after the MMU is
  built, the allocator moves behind the kernel heap's lock.

Alternatives: a free list threaded through the free frames needs the frames
mapped and writable and trusts their contents; it also cannot refuse a double
free. A buddy allocator gives contiguous multi-frame blocks, which nothing
needs yet: page tables take single frames. It can replace the bitmap behind the
same `alloc` and `free`.

## Consequences

- Allocation is a word scan: constant per frame when memory is fresh, linear in
  the window after heavy fragmentation. Acceptable through K5, which asks for one frame at a
  time and does not fragment the bitmap; revisit with K8.
- A frame is handed out with whatever it held; page tables must be zeroed by
  the caller.
- Not covered yet: frames that need contiguity or alignment above 4 KiB (DMA
  buffers), memory above the window, and `reserved-memory` children that only
  ask for a size (dynamic regions), which reserve nothing until something
  assigns them an address.
- The boot self-test drains the whole allocator, so boot time grows with RAM.
