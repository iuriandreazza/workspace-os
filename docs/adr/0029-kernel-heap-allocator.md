---
title: "ADR 0029: Kernel heap: a first-fit free list in its own window of the upper half"
type: adr
status: accepted
created: 2026-11-20
updated: 2026-11-20
tags: [kernel, memory, heap, safety]
---

# ADR 0029: Kernel heap

## Context

K5 gives the kernel `alloc`: `Box`, `Vec` and the like for what comes next
(interrupt controller state, threads, the scheduler's queues). Constraints:

- The kernel holds mechanism only (ADR 0002). The heap decides nothing about
  who may allocate or how much; it hands out memory and reports refusal.
- Frames come from `physmem` (ADR 0012) one at a time and are not contiguous.
- The address space is ADR 0019's: tables in `kernelvm`, W^X, a 39-bit upper
  half whose first gigabytes hold the image. K4c will drop the lower window and
  add a linear map, so the heap must not need physical = virtual (ADR 0019 only
  guarantees that for the window today).
- There is one core, no interrupts and no threads today. K6, K7 and K11 add
  each of them in that order.
- A new crate must pass `scripts/check-licences.py` (ADR 0005, ADR 0017).

## Decision

### Algorithm: an address-ordered free list, first fit, with coalescing, our own

A new host-testable crate `kernelheap` (no dependencies, `#![no_std]`).

- Free blocks form a singly linked list ordered by address. Each free block
  starts with a 16-byte header `{size, next}`; allocated blocks have no header,
  because `GlobalAlloc::dealloc` passes the `Layout` back. Every request is
  rounded up to a multiple of 16 bytes (minimum 16), so any remainder is either
  empty or large enough to be a block.
- `alloc`: first free block that fits after aligning the start; the unused front
  and tail become free blocks again. `dealloc`: insert in address order and
  merge with both neighbours. Both are linear in the number of free blocks.
- Alignment up to 4096 is served; a larger one is refused (`AlignmentTooLarge`).
  Nothing in the kernel needs more (page-aligned buffers are frames).
- `dealloc` returns an error instead of corrupting the list when the pointer is
  outside the heap, not 16-aligned, or overlaps a free block (double free). A
  caller that passes a wrong `Layout` for a live block is undefined behaviour,
  as `GlobalAlloc` specifies; the heap cannot see it.
- Growth is the caller's: `extend(len)` adds `len` bytes right after the heap's
  current end, and the glue calls it when `alloc` answers `OutOfMemory`.

| Option | For | Against |
|---|---|---|
| Free list, first fit (chosen) | About 250 lines; reads end to end; coalesces; no size classes to tune; exact and cheap to test against a model | Linear in free blocks; fragmentation is first fit's |
| Bump allocator, never frees | Trivial | Fails the "reuse after free" the item asks for |
| Size classes / slab | Constant time, little fragmentation | Needs a fallback for large sizes anyway; classes are a tuning decision with no workload yet |
| Buddy | Bounded fragmentation, fast coalescing | Power-of-two waste, per-order lists and a bitmap: more code than anything needs until K8 |
| `linked_list_allocator` (Apache-2.0 OR MIT) or `buddy_system_allocator` (MIT) | Written and exercised | Both are compatible with ADR 0005, but the first is this same design, so a dependency would only buy code we can read in one sitting, and the host tests here would still be needed to trust it. Not added; `check-licences.py` is not touched |

Revisit when a workload shows the free list is slow (K8 and later): the
`Heap` interface (`alloc`, `dealloc`, `extend`) is what a faster algorithm
would keep.

### Where it lives, how it grows, with which permissions

- A window of the upper half of its own: `HEAP_BASE = KERNEL_OFFSET + 448 GiB`
  (`0xffff_fff0_0000_0000`; the constant is in `kernelvm` and a host test
  checks it lies in the upper half, clear of the image), of `HEAP_WINDOW = 1 GiB`. Nothing else is ever mapped in it, so the pages below
  `HEAP_BASE` and above the mapped end are unmapped: **the guard pages are the
  gaps**, the heap cannot run into the image or into itself.
- It starts empty and grows on demand. Growth: when `alloc` answers
  `OutOfMemory`, the glue takes the request's pages (at least 16 pages, 64 KiB),
  asks `physmem` for one frame per page, maps each with `kernelvm`'s `Mapper`
  resumed on the live `TTBR1_EL1` root, as Normal memory, **read and write,
  never executable** (the same `Permissions::ReadWrite` the data section has;
  `WXN` also holds), and calls `extend`. Frames for new tables come from the
  same allocator.
- Growth stops on the first thing that fails: no frame (`physmem` empty), no
  table frame (`MapError::OutOfFrames`), or the window is full. Pages mapped
  before the failure stay and are used. If the request still does not fit, it
  is refused.
- Writing a descriptor into a slot that was empty needs no break-before-make
  and no TLB invalidation (an invalid entry is not cached); it needs the
  descriptor visible to the walker: clean and invalidate the touched entries by
  address (`dc civac`), `dsb ish`, `isb`. The TCG cannot show a missing one
  (ADR 0019, "Known limits"); this is read against the architecture.
- The heap never gives frames back to `physmem` in this ADR (no shrinking).
  Unmapping needs break-before-make and a TLB invalidation that no boot here
  can show. Recorded as a limit, not a decision against it.
- The heap keeps no physical pointer. It is addressed through the window it
  owns, and frames are only ever passed to the mapper as output addresses. It
  therefore survives K4c unchanged. What does not: the page-table writes of
  growth reach table frames at their *physical* address through the lower
  window, like `FrameTables` already does (ADR 0019); K4c changes that one
  place when it makes table access virtual. If K4c's linear map or 48-bit
  regime wants this range, it moves `HEAP_BASE`: one constant.

### Exhaustion and invalid requests

Every failure is a value, never a hang and never undefined behaviour:

| Request | Result |
|---|---|
| size 0 | `AllocError::ZeroSize` (the global allocator returns null: the contract forbids the call) |
| alignment above 4096 | `AlignmentTooLarge` |
| size that overflows when rounded or aligned | `SizeOverflow` |
| no block fits and growth cannot add enough (no frames, window full) | `OutOfMemory` |
| free of a wrong pointer or a double free | `FreeError::{Outside, Unaligned, Overlap}` |

Through `alloc`, a refusal is a null pointer, which `Box` and `Vec` turn into
`handle_alloc_error`: a panic, which the kernel's panic handler reports on the
console. Kernel code that must survive a refusal uses `try_reserve` or the
heap's `try_alloc`. The gate has a boot that exhausts the heap and checks the
reported refusal and that freeing everything makes the heap usable again.

### What it assumes about the future

The heap is guarded by one lock in the glue (`kernel/src/heap.rs`, `Locked`).
Today the lock only detects re-entry and panics: with one core, no interrupts
and no threads, the only way to get a second entry is an exception handler that
allocates while the interrupted code was inside the allocator, and that is a bug
to report, not to wait on. Its doc comment lists what each item must do:

- **K6 (interrupts):** interrupt handlers must not allocate; or the lock masks
  interrupts while held (save and restore `DAIF`).
- **K7 (threads):** the same masking, because a timer tick must not switch away
  from a thread that holds the lock; or a sleeping lock once the scheduler
  exists.
- **K11 (cores):** a spin lock on an atomic flag with `compare_exchange` (it
  cannot be a plain load and store once two cores race). Not done earlier on
  purpose: exclusive load/store on memory that is Device or non-cacheable may
  never succeed on real cores, and `memory.rs` already avoids it for the same
  reason on the direct path.

The `kernelheap` crate itself is single-threaded by type (`&mut self`); the
lock is the only place that changes.

## Alternatives not taken

- *Fixed-size heap carved at boot.* Smaller code, but it turns "how much heap"
  into a boot-time policy before anything says how much is needed, and wastes
  RAM on small machines. Growth on demand costs a loop in the glue.
- *Heap inside the identity window (physical = virtual).* Needs contiguous
  frames and breaks the day K4c drops the window.
- *Eagerly mapped 1 GiB.* Spends 262144 frames on a 2 GiB machine.

## Consequences

- `alloc` becomes available; `kernel` gets `extern crate alloc` and a
  `#[global_allocator]`. Nothing allocates before `heap::init`; a request before
  it is refused and panics with the allocator's message.
- The frame allocator moves behind the heap's lock after `mmu::init`, because
  growth needs it. The boot summary is printed just before that, so it still
  shows the frames free before the heap took any; the heap's own lines report
  its pages.
- Boot cost: the stress test is bounded (it does not scale with RAM). The
  exhaustion boot scales with RAM up to the 1 GiB window, so it runs on a
  128 MiB machine (the gate's default) for physical exhaustion and on a 2048 MiB
  machine for the window limit.
- `Known limits`: no shrinking; first fit's fragmentation and linear time; the
  TCG shows neither the missing barriers of growth nor TLB behaviour; the
  window address is a constant K4c may move; the lock does not exclude
  anything yet.
- `kernelvm::audit` expects every upper-half leaf at the image's constant
  offset and runs before the heap exists; after it, every heap page is flagged
  `wrong_output`. A later audit of the upper half (K4c's included) must skip
  the heap window.
