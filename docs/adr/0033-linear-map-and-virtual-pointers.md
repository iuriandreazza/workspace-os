---
title: "ADR 0033: A linear map, virtual pointers, no lower half, a 48-bit regime and reclaimed boot memory"
type: adr
status: accepted
created: 2026-11-23
updated: 2026-11-23
tags: [kernel, memory, mmu, aarch64, bootloader, security]
---

# ADR 0033: A linear map and virtual pointers (K4c)

## Context

ADR 0019 gave the kernel an upper half for its image and **kept a lower
window**: all RAM identity mapped, plus the UART, because the kernel still held
physical pointers (the boot info, the memory map, the device tree, the frames it
turns into tables). It named three limits that this item closes:

- the lower window exists only because pointers are physical;
- the BootloaderReclaimable memory (boot info, memory map, device tree copy,
  the bootloader's own tables, former boot services memory) cannot be freed
  while the kernel still reads it;
- both halves translate 39 bits, so a machine with RAM above 512 GiB is refused
  (`kernelvm::map_ram_region` on the direct path, the bootloader's identity map
  on the UEFI path).

Measured before writing this, on QEMU 11.1 on the Mac that develops this
(Apple silicon, 8 GiB, TCG), by booting the unchanged `main`:

| Boot | `-m` | Result |
|---|---|---|
| direct load | 300G | boots (the allocator window is 4 GiB; the rest is "unused") |
| direct load | 520G | kernel panic in `mmu::init`: RAM beyond the 39-bit regime |
| UEFI | 300G | boots |
| UEFI | 520G | the bootloader stops: "memory lies beyond the addresses the translation regime covers" |

So QEMU does run a guest whose RAM ends above 512 GiB (guest RAM is not
touched until used), the firmware starts with it, and **two** places refuse it:
the kernel's 39-bit tables and the bootloader's identity map, which is one
level 1 table (`bootinfo::paging`, `COVERED = 512 GiB`). The bootloader is
therefore part of this item.

Constraints that stay: ADR 0013's entry contract (`x0` is a physical address,
the MMU state at entry, the fixed physical load address until K4d), ADR 0019's
W^X and ADR 0026's audit and exception stack, ADR 0029's heap window, the
single core without interrupts, soft-float.

## Decision

### The 48-bit regime, set up once, in phase A

`T0SZ = 25` and `T1SZ = 25` become, for the upper half, **`T1SZ = 16`**: 48
bits, four levels, a level 0 root of 512 entries each covering 512 GiB. The
kernel's own lower half does not exist any more (below), so `T0SZ` is only what
the entry left: phase A of the direct path still builds its 39-bit lower tables
to run its first instructions (it needs 512 GiB of identity at most, and the
allocator hands out frames from the lowest 4 GiB of RAM, so nothing it
touches before the switch is higher), and the bootloader path keeps the
firmware's.

`TCR_EL1.T1SZ` is written **by `boot.s`**, with a level 0 table for `TTBR1_EL1`,
and phase B only replaces the root with another level 0 table. Changing `T1SZ`
while `TTBR1_EL1` is live would let the walker read tables of one shape with the
size of the other between the two register writes, on instructions fetched
through the very translation being changed. `boot.s` gains one table
(`boot_l0_high`) above `boot_l1_high`.

`kernelvm` changes accordingly: `ADDRESS_BITS = 48`, the walk has levels 0 to 3,
`is_canonical` compares bits 63:48, `Mapper` roots at level 0 (the 39-bit root
disappears from the crate, with its tests moved to the new one),
`walked_descriptors` returns four entries, and `frames_to_map` is unchanged
(the heap window is inside the last level 0 entry, whose level 1 table the image
already made).

Leaves remain 4 KiB, 2 MiB and 1 GiB. There are no 512 GiB blocks (they do not
exist at level 0 with a 4 KiB granule).

### The upper half's layout

| Range | Holds | Mapped how |
|---|---|---|
| `0xffff_0000_0000_0000` .. `0xffff_8000_0000_0000` (`LINEAR_BASE`, 128 TiB) | the linear map of RAM: virtual = physical + `LINEAR_BASE` | read and write, never executable, Normal; 1 GiB and 2 MiB blocks where aligned |
| `0xffff_ff80_0000_0000` (`KERNEL_OFFSET`, unchanged) + physical | the image, as ADR 0019 | by section |
| `KERNEL_OFFSET + 384 GiB` (`DEVICE_BASE`), 1 GiB | device registers | Device-nGnRnE, read and write, never executable, 4 KiB pages |
| `KERNEL_OFFSET + 448 GiB`, 1 GiB | the heap (ADR 0029, unchanged) | on demand |

The linear map does **not** use `KERNEL_OFFSET`: `KERNEL_OFFSET + physical`
overflows 2^64 for physical addresses of 512 GiB or more. Its base is the bottom
of the upper half; it spans half of it (physical addresses below 2^47). RAM
at or above 2^47 (128 TiB) is refused at boot with a message, which no machine
this runs on approaches. Its last entry is far below the level 0 entry 511
where everything else lives, so the two never meet; the gaps in the last
512 GiB are the guard pages, as for the heap.

**What the linear map holds.** Every RAM region of the boot map (or the device
tree), widened to pages, **minus the image and its guard pages** (the same
`hole` as ADR 0019's window): no writable alias of the kernel's code, constants
or stacks exists, and a guard page stays unmapped. Memory the kernel does not
own (bootloader memory, reserved regions) is readable and writable for the
kernel itself and never executable, as the window was. The frames the allocator
gives to page tables, to the heap and to everything else are reached through it.
This **supersedes ADR 0019's sentence** that a later linear map at
`KERNEL_OFFSET` contains the image mapping: the two are separate, so the
image's address does not constrain how big RAM is.

The image keeps `KERNEL_OFFSET + physical`; K4d, which lifts the fixed load
address, decides the image's virtual placement. Until then it requires the
image's physical address to stay below 384 GiB (it is 1 GiB).

### How the kernel reaches a physical address

One rule: **physical addresses are converted to a pointer in one place**,
`arch::pointer_to(physical)`, which adds an offset held in one static: 0 while
the lower half is the way in (the entry's identity map, or phase A's), and
`LINEAR_BASE` from the moment the linear map is live. Page-table access
(`PhysicalTables`, `FrameTables`), the cache maintenance of table lines
(`dc civac`, `dc ivac` take virtual addresses), `grow_heap`'s descriptor
cleaning, and the readers of the hand-over all go through it. After the lower
half is gone nothing may hold a physical address as a pointer: the boot info,
the memory map, the device tree and the UART are read through the linear map or
the device window.

`bootinfo` and `devicetree` keep taking an address. `BootInfo::memory_map` gains
a variant that takes the conversion (the structure holds physical pointers to
the map and to the blob), and `Fdt` keeps the validated blob so the kernel can
digest and copy it.

### Devices

A device region gets an upper-half address from a small bump allocator over the
device window, `kernelvm::DeviceWindow`: it maps the pages that hold the
registers (`physical` rounded down, page offset preserved) at the next free
virtual page, as Device, and leaves one unmapped page after each mapping (the
guard). It returns the virtual address of the first register. There is no
unmapping: a device stays for the life of the kernel.

K4c uses it for the UART, when the upper half is built. K6 will map the
interrupt controller the same way, on the live root, like `grow_heap` does
(`Mapper::resume`), continuing the window from `DeviceWindow::next()`, which
`init` must then hand on (K4c keeps no state beyond the console's mapping); the
descriptors are published (`dc civac`, `dsb ish`, `isb`) and no TLB maintenance
is needed for slots that were empty. The device's physical range is still the
audit's expected Device range.

The console is the first user and the dangerous one: its base must change to
the virtual address **before the lower half goes**, or the next message faults,
the handler reports through the same address, and the boot hangs without a word.
`board::use_console_at(virtual)` is called between the two steps below.

### Sequence

1. (As today) read the hand-over at physical addresses; build the allocator;
   take the digest of the hand-over.
2. Build the new upper half from frames: the image by section, the linear map,
   the UART in the device window. Audit what was built. Clean the tables to the
   point of coherency and wait (`dsb ish`).
3. **Switch `TTBR1_EL1`** to the new root: write, `isb`, `tlbi vmalle1`,
   `dsb ish`, `isb`. Same regime, so the image keeps its size, output address
   and attributes in both root tables: the translation of every address in use
   does not change (ADR 0019's argument, unchanged, because break-before-make
   is impossible for the half the code runs from).
4. Point `arch::pointer_to` at the linear map; point the console at the device
   window.
5. **Drop the lower half**: set `TCR_EL1.EPD0` (walks through `TTBR0_EL1` off),
   `isb`, point `TTBR0_EL1` at an empty table (a zeroed page: defence in depth
   if `EPD0` were ever cleared), `isb`, `tlbi vmalle1`, `dsb ish`, `isb`. After
   this no virtual address below the upper half translates. Nothing is
   written between those instructions that needs the lower half (they are one
   block of register operations, as in ADR 0019).
6. Read the hand-over again through the linear map (ADR 0026's digest, now
   through the way the kernel will use from here on); from this point the
   kernel uses that `Board` and never the one read in 1, whose slices point
   into memory the lower half used to map.
7. `mmu::verify` walks the live tables (below), and probes the hardware with
   `at s1e1r` at addresses that were mapped before: the UART's physical page,
   the image's first page, the hand-over, the first page of each RAM region.
   All must fault. If the highest free RAM lies outside the allocator's window,
   one frame there is written and read back through the linear map.
8. `heap::init` (ADR 0029, unchanged).
9. Copy the hand-over out of reclaimable memory into the heap; reclaim (below).

### Reclaiming BootloaderReclaimable memory

Memory of that kind holds: the boot info structure, the memory map array, the
device tree copy, the bootloader's page tables, and former boot services memory.
It is handed to the frame allocator, once and only after:

- the lower half is gone and its TLB entries invalidated (step 5). The
  bootloader's tables live in this memory: handing it out earlier would let the
  allocator overwrite tables the walker still reads;
- everything the kernel keeps from it is **copied or re-read**: the boot info's
  scalar fields already live in `Board`; the memory map array and the device
  tree blob are copied into the heap (`Board::into_owned`, leaked for `'static`,
  as the kernel's tables are); the digest of the copy must equal the digest taken
  at step 1, which the gate prints. `x0` is not used again.

The order is the heap first, then reclaim (not reclaim before `heap::init`,
pinning the pages `Board` points into): the copy needs the heap, the kernel then
keeps no pointer into reclaimable memory, and "which pages are still live"
stops being a question.

Frames to hand over: each BootloaderReclaimable entry, minus the device tree's
`/memreserve/` regions and `/reserved-memory` (`Board::for_each_tree_reservation`
yields them from the owned copy; `for_each_reserved_region` would also yield the
reclaimable entries themselves), clipped to the allocator's window (the 4 GiB
that its bitmaps manage: the rest is counted as dropped, as it already is).
Other kinds are untouched: Reserved, AcpiNvs, ReservedMapped (runtime services)
and AcpiReclaimable (ACPI tables, no reader yet) stay reserved. Direct load has
no such kind: its device tree blob is Usable memory the kernel reserved itself
and keeps reading in place.

`physmem` gets `FrameAllocator::release(range, keep)`: it makes the frames of
`range` that are reserved (neither available nor allocated), outside every
range of `keep`, available; frames already available or allocated are left
alone, so releasing twice is harmless; it returns how many it released. The
caller promises the range is RAM (the allocator does not keep that bit: its
storage is two bitmaps, and a third would change the size the kernel reserves).
This **amends ADR 0012**, which has no way to free what `Builder::reserve` kept
out. `heap::with_frames(f)` gives the code that reclaims the allocator the heap
now owns (ADR 0029 expected K8 to add such an accessor).

The work of reclaiming is done in two steps, because the heap's lock refuses an
allocation while it is held (and the allocator now lives in it): the lists of
regions and of what to keep are made first (they allocate), then
`heap::with_frames` runs the release, which allocates nothing.

The gate shows it (below): the number of frames released equals what the boot
map says; the kernel then takes every one of them with
`FrameAllocator::alloc_in(range)` (the lowest available frame inside a range:
the frames of a reclaimed range and no others), writes a pattern through the
linear map, reads it back, and frees them; the hand-over digest, taken again
after that, is equal to the one before. A kernel that kept pointing at
reclaimable memory would read the pattern instead of the blob.

### The audit, per window

ADR 0026's audit assumes one offset (output = virtual minus the root's base) for
the whole root. Three windows with three offsets (image `KERNEL_OFFSET`, linear
`LINEAR_BASE`, devices: any frame, but only the expected device pages) break
that, so `Expectation` names a **virtual window** and the **offset** inside it
(or none), and `audit` judges the leaves whose virtual address lies in the window.
A separate walk (`strays`) reports the first leaf outside every window given: the
lower half after the drop is audited with no window at all, and a leaf there is a
stray. The heap window does not exist when `verify` runs (before `heap::init`);
a later audit of the upper half must list it as a window with "any frame, read
and write, never executable" (ADR 0029 already says it must be skipped).

`verify` checks, besides ADR 0019's and ADR 0026's: the linear window holds only
Normal read and write, never an alias of the image, each leaf at output =
virtual minus `LINEAR_BASE`; the device window holds only Device pages inside
the expected range; the lower half's `TTBR0_EL1` root has no entry, `EPD0` is
set, and the `at` probes above fault; and no leaf lies outside the three windows.

### The bootloader's identity map

The UEFI path needs the bootloader to describe RAM above 512 GiB too, because
the kernel enters with the bootloader's map live and reads the boot info
through it until step 5. `bootinfo::build_identity_map` keeps its layout for the
first 512 GiB (everything starts as 1 GiB Device blocks, RAM splits them) and,
where the live `TCR_EL1` has more than 39 bits (the root is already level 0),
adds **a level 1 table per further 512 GiB that holds RAM**, made on demand,
likewise Device blocks first. `tables_needed` counts them. A regime of 39 bits
or fewer still refuses RAM beyond it, because the firmware itself cannot reach
it. The bootloader's own allocations (boot info, map, device tree copy, table
pool) come from the firmware's `AllocatePages`, which on EDK2 may sit at the top
of RAM: the map above is what lets the kernel read them there.

### Where the code lives

- `kernelvm`: the four levels, `LINEAR_BASE`/`LINEAR_SPAN`, `DEVICE_BASE`,
  `DEVICE_WINDOW`, `DeviceWindow`, `map_linear_region` (replacing
  `map_ram_region`), the audit's windows and `strays`; host tests including a
  machine with RAM above 512 GiB and above 2^47, a region touching the image,
  devices, and the audit seeing each mistake.
- `physmem`: `release` and `alloc_in`.
- `bootinfo`: `build_identity_map` with more than one level 1 table; the
  memory map through a conversion.
- `devicetree`: `Fdt::blob`.
- `kernel`: `boot.s` (one table, `T1SZ`), `arch/aarch64/mmu.rs` (build, switch,
  drop, verify, probes), `board.rs` (the conversion, `into_owned`, console),
  `memory.rs` (`Reclaim`, its self-test, and the test of a frame beyond the
  allocator's window), `heap.rs` (`with_frames`), `main.rs` (the order above). Addresses of the layout live in `kernelvm`;
  `board.rs` holds the UART's, as before.

## Alternatives considered

### Where the linear map lives

| Option | For | Against |
|---|---|---|
| Bottom of the upper half, `LINEAR_BASE` (chosen) | No overflow for any physical address the regime reaches; the image's address stays independent of RAM; `virtual = physical + constant` | The image and the linear map are different addresses of the same frame (the image's frames are left out of the linear map, so no alias is writable) |
| `KERNEL_OFFSET + physical`, as ADR 0019 foresaw | One offset; the image is a subset | Wraps 2^64 at 512 GiB, which is the machine this item is for |
| Map all of 2^47 up front | No decision about holes | Maps addresses with nothing behind them as Normal memory: speculative accesses to devices; page tables for 128 TiB |

### The lower half

| Option | For | Against |
|---|---|---|
| Dropped (chosen): `EPD0` and an empty root | A null or small pointer faults (the item's check); nothing in the lower half to audit; K8 gets it for users | Every physical pointer must have been converted; a forgotten one hangs at the drop (found by the boots) |
| Kept, shrunk to what is read | Smaller change | Another set of tables to keep correct, and the item says to drop it |
| Only `TTBR0_EL1` zeroed | Fewer instructions | `EPD0` is what makes the intent a fact the hardware enforces |

### When to reclaim

| Option | For | Against |
|---|---|---|
| After `heap::init`, copying into the heap (chosen) | The kernel keeps no pointer into the memory; one rule | The heap and the copy must exist first; the blob's size (up to 2 MiB) is heap |
| Before the heap, pinning the pages `Board` uses | No copy | Which frames are still live is a list that must be right; a tight machine loses them for good |
| Never | No risk | The item says to; about 10 to 30 MiB of 256 MiB goes unused |

### The regime

| Option | For | Against |
|---|---|---|
| 48 bits, level 0 root (chosen) | RAM anywhere the processor can address; same shape as the bootloader's when firmware uses 48 bits | One more table level for every walk |
| 52 bits (LPA2) | More | A feature not every core has, 16 KiB or 64 KiB granules to be useful, no machine here needs it |
| Stay at 39 bits, a bigger window | None | The item |

## Consequences

- The boot uses about 10 more frames (the level 0 root and its level 1 tables
  for the linear map, the device pages).
- Every existing boot changes what it reports about the lower half: the report
  line now says the half is empty; the digest line says the hand-over was read
  through the linear map. The gate's check of the lower window's size is replaced
  by a check that the linear window holds the RAM minus the image.
- **Amends ADR 0012** (`release`, and the allocator behind `heap::with_frames`).
- **Supersedes in ADR 0019:** the 39-bit size of both halves (it holds neither
  now), "the lower half, after the switch" (kept as a window: it is dropped),
  "reclaiming ... is not done in K4" (it is), the sentence that a linear map at
  `KERNEL_OFFSET` contains the image, and the Known limit "Memory above 512 GiB".
  **In ADR 0026:** the digest is read again through the linear map, not the
  lower window, and the audit's one-offset-per-root rule is replaced by windows.
  **In ADR 0029:** "the page-table writes of growth reach table frames at their
  physical address through the lower window" now go through the linear map.
- The gate adds (each confirmed by a mutation listed in the pull request): a
  boot where the lower half is empty, shown by the audit of the root, `EPD0`, and
  `at` probes that fault; the reclaimed frames, counted against the memory map,
  allocated and used; a machine with RAM above 512 GiB, directly and through the
  bootloader, where the hardware allows it; and every existing boot.
- ADR 0013's statement that "K3's allocator never hands out
  BootloaderReclaimable memory before K4" now has its end: K4c.

## Known limits

- **The TCG models neither caches nor the TLB** (ADR 0019): a missing `tlbi`
  after the root switch or the drop, or a missing clean of the new tables,
  passes every boot of the gate. The probes show the walk's result, not the
  maintenance that makes it hold on real hardware. Read against the architecture.
- **Physical addresses at or above 2^47** are refused. The allocator manages a
  4 GiB window from the lowest RAM address; RAM beyond it is mapped in the linear
  map and reported unused, as before.
- **RAM above 512 GiB on the gate's machines**: both 520 GiB boots (direct and
  UEFI) ran and passed on the Mac that develops this (plain `-m 520G`, about a
  minute for the UEFI one) **and on the CI runner** (Ubuntu, QEMU from the
  distribution): there plain `-m 532480M` was refused (`cannot set up guest memory
  'mach-virt.ram': Cannot allocate memory`) and the gate's second attempt,
  `memory-backend-ram,reserve=off`, started them (run 45200361440, nothing
  skipped).
  Whether another host can start a guest of that size depends on its overcommit
  policy; the gate tries `-m 520G`, then `memory-backend-ram,reserve=off`
  (which macOS's QEMU does not have), and if neither starts it says `SKIP` with
  QEMU's reason and the construction of the tables is left to the host tests
  (`kernelvm`: a 520 GiB machine, RAM beyond 2^47; `bootinfo`: RAM in the second
  512 GiB). Where each machine's firmware allocates what it hands over is the
  firmware's choice: on the Mac's EDK2 it is the top of RAM (above 512 GiB), so
  the UEFI boot reads the boot info and the bootloader's tables up there, which
  the gate does not assert.
- **The keep list of the reclaim** (kernel image, stacks, device-tree reservations) is
  three lines in `Reclaim::plan` that no boot exercises: the QEMU boots have no
  reservation inside reclaimable memory. `FrameAllocator::release` honours a keep
  list (host tests); the wiring is read, not run.
- **A big machine on a host that cannot start one** skips the boots that show the
  linear map beyond the allocator's window; no cheaper boot covers that part.
- **No unmapping of devices and no free of reclaimed memory twice**: devices live
  forever, `release` is idempotent.
- **The firmware's regions** (runtime services) are still not mapped; calling
  them needs its own decision (ADR 0019).
- **AcpiReclaimable memory** is not reclaimed: ACPI has no reader (ADR 0013).
- **Entry code still runs on the entry's map**: phase A's instructions run on
  the bootloader's tables, writable and executable, until `WXN` (ADR 0019).
