---
title: "ADR 0019: Kernel address space, higher-half and write-xor-execute"
type: adr
status: accepted
created: 2026-11-19
updated: 2026-11-19
tags: [kernel, memory, mmu, aarch64, security]
---

# ADR 0019: Kernel address space, higher-half and write-xor-execute

## Context

K4 gives the kernel its own translation tables. Until now the kernel ran with
the MMU off (direct load by QEMU) or on the bootloader's identity map
(ADR 0013), linked at `0x40200000`, with every byte of RAM readable, writable
and executable. What the item asks for: the kernel in the upper half under
`TTBR1_EL1`, tables made from frames of the K3 allocator (ADR 0012), and
write-xor-execute that a boot can show: a write to the kernel's code and a
jump into data each end in a reported fault.

Facts that shape the decision:

- The loaders do not change. QEMU's `-kernel` loads the ELF segments at
  `p_paddr` and jumps to `e_entry` as written; the bootloader places segments by
  `p_paddr` and checks `e_entry` against them (`bootinfo/src/elf.rs`, ADR 0013).
  Both therefore need the entry point and the load addresses to be physical.
- ADR 0013 fixes what the kernel receives and does not forbid what it does
  with `TTBR1_EL1`, `TCR_EL1`, `MAIR_EL1` or `SCTLR_EL1`: it says they are
  "otherwise unspecified", and that the kernel replaces the bootloader's tables
  in K4. The entry contract (registers, EL1, MMU and caches on with an identity
  map in `TTBR0_EL1` on the bootloader path; MMU off on the direct path) is used
  as written. **This ADR does not change it.**
- The kernel's code reads physical addresses everywhere today: the boot info and
  its memory map, the device tree blob, the UART, and the frames it will turn
  into tables. Making all of them virtual touches `bootinfo` and `devicetree`
  readers that other work is editing, and is not what K4 asks for.
- The QEMU TCG models neither caches nor TLBs (ADR 0013, "What no test here can
  show"). The gate proves permissions and layout, not invalidation.

## Decision

### Address spaces

4 KiB granule. Each half translates 39 bits (`T0SZ = T1SZ = 25`), so every
root is a level 1 table and a walk has three levels. Leaves are 4 KiB pages,
2 MiB blocks and 1 GiB blocks (whole gigabytes of RAM are one entry each).

| Half | Range | Root | Holds |
|---|---|---|---|
| upper | `0xffff_ff80_0000_0000` up | `TTBR1_EL1` | the kernel image |
| lower | `0` to `2^39` | `TTBR0_EL1` | a window onto RAM and the UART, for physical pointers (below) |

The kernel image keeps its physical address, `0x40200000`, and its virtual
address is the physical one plus `KERNEL_OFFSET = 0xffff_ff80_0000_0000`, so the
kernel starts at `0xffff_ff80_4020_0000`. The offset is defined twice, in
`kernelvm/src/lib.rs` and in `link.ld`; the boot smoke test compares them.
Because virtual is physical plus a constant, a later linear map of RAM at the
same offset contains the image mapping as a subset (K5).

### The image, by section

Every section starts on a page. The linker script defines the boundaries
(`__text_end`, `__rodata_end`, `__data_end`, `__stack_bottom`, `__stack_top`)
and the page tables give each kind its own permissions, with 4 KiB pages:

| Part | Access | Execute | Notes |
|---|---|---|---|
| `.text` (boot code, vectors, code) | read | yes (`PXN` clear) | read only (`AP[2]`) |
| `.rodata` | read | never | read only |
| `.data`, `.bss`, boot tables | read, write | never | |
| guard page | none | none | one page below the stack, not mapped |
| boot stack (64 KiB) | read, write | never | |
| device registers | read, write | never | Device-nGnRnE |

Every descriptor also has `UXN` set and `AP[1]` clear: nothing is reachable
from EL0, and K8 will add what it needs. Entries are global (`nG` clear) and
inner shareable (Device: outer). `SCTLR_EL1.WXN` is set: whatever is writable is
never executable, whatever a descriptor says. Two mechanisms, so a wrong bit in
one table does not silently open a hole. The fault boots cannot tell them
apart (a page with `PXN` wrongly clear still faults on a fetch, because of
`WXN`, with the same syndrome); the walk of the live descriptors in `verify`
can, and it is what checks the bits.

The memory types are not assumed: `MAIR_EL1` is read and the indexes of
Normal write-back (`0xff`) and Device-nGnRnE (`0x00`) are looked up, as the
bootloader's map builder does (ADR 0013 leaves `MAIR_EL1` as the firmware had
it). The direct path installs `0xff00` itself.

### The lower half, after the switch

`TTBR0_EL1` is **kept, as a kernel-owned window**: all RAM the memory map (or the
device tree, on the direct path) describes, identity mapped read and write, never
executed, with 1 GiB and 2 MiB blocks where the addresses allow it, plus the page of the
UART as Device. It exists because the kernel still holds physical pointers (boot
info, memory map, device tree, the frames it turns into tables) and K4 does not
rewrite those readers. The image and its guard page are **not** in it: no alias
of the kernel's code or data is writable. A page of RAM that the kernel does not
own (bootloader memory, runtime services' neighbours) is readable and writable
here only for the kernel itself; nothing is executable.

The bootloader's tables, which lived in BootloaderReclaimable memory, are no
longer used after the switch. Reclaiming that memory is **not** done in K4: the
boot info, the memory map and the device tree copy are in the same kind of
memory and the `Board` still points into them. It becomes possible when those
pointers are virtual and the window is dropped (K5 or later, a new ADR).

### Sequence of activation

The first instructions run at the physical address, while the linker placed
them in the upper half, so `adrp` of a symbol gives its physical address and
`ldr x, =symbol` its virtual one. The image is linked so that the ELF entry is
`_start - KERNEL_OFFSET` (physical), the segments' `p_paddr` are contiguous from
`0x40200000` and their `p_vaddr` are the upper-half addresses. The loaders are
unchanged.

**Phase A, `boot.s`, no stack, no vectors, assembly.** Common to both paths:

1. Entry registers, and on the direct path the drop to EL1, exactly as before.
2. Clear `.bss` (the boot tables are in it).
3. Find the `MAIR_EL1` indexes of the two memory types (a loop over the eight
   attribute bytes; neither missing parks the core).
4. Build one level 3 table of 512 entries that covers the 2 MiB from the start
   of the image (`link.ld` asserts that the image, guard page and stack fit):
   text `RX`, `.rodata` `R`, `.data`/`.bss`/stack `RW` and never executed, the
   guard page empty, and the rest of the 2 MiB after the stack read and write
   (frames the allocator may hand to phase B's tables). No page is writable and
   executable. Link it under a level 2
   and a level 1 table at the indexes of the image's virtual address.
5. Clean and invalidate the tables by address (`dc civac` over the line size
   read from `CTR_EL0`, never by set and way,
   which the bootloader contract forbids), `dsb sy`.

Direct path, MMU off: install `MAIR_EL1` and `TCR_EL1` (both halves, `IPS` from
`ID_AA64MMFR0_EL1`, at most 48 bits), build a lower half, and set `TTBR0_EL1`,
`TTBR1_EL1`. The lower half is needed because the instruction after the MMU is
enabled is still at its physical address, and the kernel reads the device tree
and the UART at their physical addresses: a level 1 table of 1 GiB blocks, the
first Device (QEMU `virt` keeps its devices below 1 GiB; the direct path exists
for QEMU only, ADR 0008) and the rest Normal read and write, never executed,
except the gigabyte of the image, which goes through a level 2 table of 2 MiB
blocks of the same kind down to the level 3 table above, so the boot code is
executable there and nothing else is. `tlbi vmalle1`, `dsb nsh`, `isb`, then
`SCTLR_EL1` with the reserved bits and `M`. **The caches stay off on this path**
(`C` and `I` clear): with `M` set and `C` clear, Normal memory is non-cacheable,
which keeps the behaviour of today, avoids stale lines from an unknown cache
state, and spares the direct path a clean of everything the earlier boot wrote.
The cost is that the direct path runs uncached, as it always has; it is the
development path.

Bootloader path, MMU and caches on: `TTBR0_EL1` and the lower half of
`TCR_EL1` stay as the bootloader left them. Replace the upper half of `TCR_EL1`
(read, clear bits 31:16, or in `T1SZ`, `IRGN1`, `ORGN1`, `SH1`, `TG1 = 0b10`,
`EPD1` clear; `TG1` encodes 4 KiB differently from `TG0`), write `TTBR1_EL1`,
`isb`, `tlbi vmalle1`, `dsb nsh`, `isb`. The tables were written through
Normal cacheable memory that the walker's attributes (`IRGN1`, `ORGN1` write-back)
agree with; the clean above makes them visible to a walker that does not
snoop.

Both paths then `br` to the upper-half address of the next label. From there
`adrp` is virtual: set `VBAR_EL1`, then set `SCTLR_EL1.WXN` and invalidate
(`tlbi vmalle1`, `dsb nsh`, `isb`), set `sp`, and call `kernel_main`.

`WXN` is not set earlier because the bootloader's map holds the code the first
instructions run from as writable and executable (RAM, Normal, no distinction),
so on that path setting it before the jump would fault the very next fetch. This
is the window in which the contract of ADR 0013 and W^X disagree: the first
instructions of `boot.s` on the bootloader path, no more than the stretch above.
It ends before any Rust runs.

**Phase B, Rust, `kernel/src/arch/aarch64/mmu.rs`, after the allocator.**
`mmu::init` runs after `memory::selftest` (which hands out every frame and gives
them back, so it must not see the tables as free):

1. Audit the boot tables of phase A, read back from `TTBR1_EL1`, against the
   layout the linker gave (`kernelvm::ImageLayout`): the assembly has no host
   test, so the kernel checks it against the code that has. A difference stops
   the boot.
2. Take frames from the allocator and build the upper-half tables with
   `kernelvm`: the image by section, as above, in 4 KiB pages.
3. Build the lower-half window: each RAM region (widened to whole pages) minus the
   image and its guard page, then the UART page.
4. Clean and invalidate the tables to the point of coherency by address
   (`dc civac`, line size from `CTR_EL0`) and `dsb ish`.
5. Switch. **`TTBR0_EL1`:** set `TCR_EL1.EPD0` (walks through it off) and `isb`;
   write the register; `isb`; `tlbi vmalle1`; `dsb ish`; `isb`; write `TCR_EL1`
   with `EPD0` clear and the lower half of the kernel (`T0SZ = 25`, 4 KiB,
   write-back); `isb`. This is break-before-make: no old and new entry for one
   address can be live together. The stretch is one `asm!` block of register
   operations and touches no memory the lower half maps. **`TTBR1_EL1`:** write
   the register, `isb`, `tlbi vmalle1`, `dsb ish`, `isb`, with the code running
   from the upper half. Break-before-make is impossible here, so the new tables
   map the image with the same page size, output address and attributes as the
   old ones: the translation of every address in use does not change, only the
   tables that hold it. A conflict abort needs two different translations of one
   address to be live, which this does not produce.
6. `mmu::verify` walks the live tables (the registers' values, not what `init`
   meant to build) and panics unless: no leaf is writable and executable (the
   descriptors alone, not counting on `WXN`); no leaf is reachable from EL0;
   the upper half holds exactly the sizes the linker gave to text, read only and
   read write; the guard page translates to nothing; the lower half holds no
   text and no read-only memory and nothing whose output lies in the image;
   `SCTLR_EL1.WXN` is set. The result is printed, and the boot smoke test checks
   the numbers against the symbols of the ELF.

Both paths end in the same state, and both start entry-specific code before this
point only.

### Where the code lives

- `kernelvm/` (new crate, plain Rust, tested on the host): descriptor layout,
  the permissions of each kind, `Mapper` (map ranges with blocks and pages,
  refuse overlap and out of range, run out of frames cleanly), `translate`,
  `audit`, `ImageLayout`, the RAM window and device mapping, and the register
  constants `boot.s` is assembled with. The tables live behind a trait
  (`TableMemory`), so the host builds them in an arena and the kernel in frames.
- `kernel/src/arch/aarch64/{boot.s,link.ld,mmu.rs}`: phase A, the linker layout
  and phase B. Everything with `unsafe` is here, each block with its `SAFETY`
  comment. `kernel/src/memory.rs` is unchanged apart from the physical addresses
  of the image, which now come from a conversion.

## Alternatives considered

### Where the kernel runs

| Option | For | Against |
|---|---|---|
| Higher-half, image keeps its physical address (chosen) | What K4 asks for; a null or small pointer faults; the loaders and the load address of ADR 0013 do not change; room for K8's lower half | Two sets of tables to get there (phase A and B); the first instructions run at an address the linker did not place them at |
| Identity mapped, W^X only | One address space, no phase A trick, no change to the linker | The roadmap and ADR 0013 foresee the higher half; user space (K8) needs the lower half free; null pointer dereferences in the kernel do not fault |
| Higher-half and a position-independent kernel, placed by the bootloader | Removes the fixed load address, so VirtualBox needs less RAM (ADR 0013) | Relocation in the loader or in `boot.s` before the MMU exists, a new loader contract (a new ADR for ADR 0013), and the physical placement still has to come from somewhere. Not needed for this item |

The restriction of ADR 0013 ("fixed `0x40200000`, fail loudly") **stays**: the
higher half decouples virtual from physical, but the loader still places the
image at the physical address the ELF names. Lifting it is a separate item.

### Size of each half

| Option | For | Against |
|---|---|---|
| 39 bits, three levels (chosen) | One fewer table per walk and per half; 512 GiB per half covers every machine this runs on; matches the 512 GiB limit the bootloader's map already has | RAM above 512 GiB is refused at boot |
| 48 bits, four levels | Covers any RAM | A level 0 table per root and a longer walk for no machine that exists here; a bootloader TCR that is 48 bits would need a root of a different level for the lower half |

### Image permissions: granularity and enforcement

| Option | For | Against |
|---|---|---|
| 4 KiB pages by section, descriptors and `WXN` (chosen) | Each section has exactly its permissions; a hole in one mechanism does not open memory; the self-test can check each | 4 KiB pages for a 500 KiB image: one level 3 table, no measurable cost |
| 2 MiB block, read write execute | Trivial | The opposite of the item |
| Descriptors only, no `WXN` | One mechanism | A single wrong bit makes a page writable and executable and nothing notices |
| `WXN` only, executable descriptors everywhere | Hardware does the work | `.rodata` would be executable; a jump into constants would succeed |

### What happens to `TTBR0_EL1`

| Option | For | Against |
|---|---|---|
| Kernel-owned RAM and UART window, never executable, without the image (chosen) | The kernel's physical pointers keep working; the bootloader's tables are no longer used; no alias of kernel code is writable | A second, large mapping of RAM stays until the pointers become virtual; any RAM page is reachable at its physical address from kernel code |
| Disable it (`EPD0`) and rewrite every pointer to a virtual one | The cleanest end state: the lower half is empty and K8 can take it | Touches the readers of `bootinfo` and `devicetree` and the console driver, which other work edits, and needs a linear map in the upper half first. A step for K5 |
| Keep the bootloader's tables | Nothing to build | Writable and executable RAM, an alias of the code, and memory the allocator must never hand out (ADR 0013) |

### The boot tables of phase A

| Option | For | Against |
|---|---|---|
| Assembly in `boot.s`, one page-granular level 3 table (chosen) | No stack, no Rust before the MMU, small; the kernel audits it | Assembly the host cannot test; the permissions per section are written once there and once in `kernelvm`, which is why phase B audits phase A and the tests compare the bit patterns |
| A coarse 2 MiB read write execute block in phase A, final tables in Rust | Much less assembly | A writable and executable window until phase B, in code that runs at every boot |
| Build everything in Rust running at the physical address | Tested code only | The compiled code and its constants are linked for the upper half: its statics, vtables and string addresses are virtual, so it cannot run before the MMU is on |

### Caches on the direct path

| Option | For | Against |
|---|---|---|
| Leave `C` and `I` off with `M` on (chosen) | Nothing is cached that was not before; no set/way maintenance; the path is for QEMU | Slow, as before |
| Turn them on with `M` | A cached kernel on both paths | Needs invalidation of whatever the unknown state of the caches holds, by set and way, which the bootloader contract forbids on the other path and the TCG cannot test on this one |

### Where the logic lives

| Option | For | Against |
|---|---|---|
| New crate `kernelvm` (chosen) | Host tests for the maps the kernel builds; the kernel cannot be tested on the host (bare metal, `test = false`). It holds AArch64 descriptor and register encodings outside `kernel/src/arch/`, as `bootinfo::paging` already does: the rule that architecture code lives under `arch/` is kept for everything that touches the machine, and this crate touches none | A crate to maintain; one more line in `scripts/check.sh` |
| Extend `bootinfo::paging` | One crate | That module builds the bootloader's identity map from a pool of tables with no permissions (Normal means read, write, execute) and no way to read it back; `bootinfo` is shared with the bootloader and being edited by K2d. Only `MemoryTypes::from_mair` is common, and it is five lines |
| In the kernel | No crate | Not testable on the host |

## Consequences

- `link.ld` changes: virtual and physical addresses differ, sections are
  page aligned, there is a guard page and `ENTRY` is the physical address of
  `_start`. The ELF has `p_vaddr != p_paddr`; `bootloader/` and `bootinfo/` read
  `p_paddr` and are not changed. The boot smoke test checks that the entry point
  is physical and that a segment is linked at the upper-half address and loaded
  at the physical one.
- Symbols (`__image_start` and the others) are virtual. `kernel_image()` and
  `boot_stack()` return physical addresses, converted in one place, because the
  frame allocator reserves physical ranges and silently ignores ranges outside
  its window: a virtual range would leave the kernel's own frames free.
- `boot.s` sets `VBAR_EL1` after the jump. `VBAR_EL1` has an unknown reset
  value, so a fault in phase A (a few dozen instructions) goes to an unknown
  address (a hang on the direct path, the firmware's handler on the other)
  instead of ending in a panic.
- The frame allocator hands out frames for the tables, so the free count after
  boot is a handful of frames lower (eight on a 128 MiB machine) than the count the
  allocator reports before `mmu::init`.
- The gate adds: the walk of the live tables at every boot, with the sizes
  checked against the ELF's symbols; and boots that end in a fault, each with the
  exception class, the syndrome's permission-fault bits, the faulting instruction
  and the fault address checked (a write to the first byte of the code; a jump to
  the first byte of `.rodata`; a jump to the first byte of `.data`), through the
  direct load and the bootloader, from EL1 and from EL2. Each check was confirmed
  by breaking the behaviour it covers (listed in the pull request).
- Reclaiming BootloaderReclaimable memory, a linear map, virtual pointers for the
  boot info and the device tree, and dropping the lower half's window are the next
  steps (K5 and after) and each needs its own decision.

## Known limits

- **The TCG does not model caches or TLBs.** A missing `tlbi`, a missing clean of
  the tables, a TLB conflict at the `TTBR1_EL1` switch, or a wrong cache state at
  `M` on the direct path pass every boot of the gate. The rules above (break
  before make for `TTBR0_EL1`, identical translation for `TTBR1_EL1`, clean by
  address, `tlbi` after each register write the walker depends on) are read
  against the architecture, not demonstrated. Only hardware, or an accelerator
  that runs on it, validates them. On Apple silicon `-accel hvf -cpu host` can be
  tried by hand for the bootloader path; it is not part of the gate.
- **The guard page weakens an overflow, it does not stop it.** An overflow
  faults at the guard page, but the handler of a synchronous exception saves its
  context (288 bytes) on the same stack pointer (`exceptions.s`), so it faults
  again, and each nested fault moves `SP` down by another 288 bytes. After about
  fifteen rounds `SP` is below the guard page, in the top of `.bss`, the saves
  succeed, a report is printed, and the register frames have overwritten what
  was there (found in review by recursing until the guard page was hit). A frame
  larger than the guard page's 4 KiB skips it altogether. A separate exception
  stack, which fixes both, is an item of its own.
- **The direct path assumes QEMU `virt`'s layout** in phase A (devices below
  1 GiB, RAM above), until phase B rebuilds the lower half from the device tree.
- **The audit does not look at memory type or shareability** of a leaf, only at
  permissions and aliasing: an image page with a wrong `AttrIndx` passes the
  walk and every fault boot.
- **Cores with permission indirection (FEAT_S1PIE)** reset `TCR2_EL1.PIE`
  to an unknown value; if it is set, the descriptors' `AP` and `PXN` bits mean
  something else. The boot code does not initialise `TCR2_EL1` or `SCTLR2_EL1`
  (writing them on a core without the feature is undefined). No machine here
  has the feature; it is untested.
- **Stale cache lines.** Phase A and `FrameTables::allocate` invalidate
  (`dc ivac`) table memory before writing it and clean and invalidate it after;
  the TCG cannot show whether that is enough on a core with caches in an unknown
  state.
- **The walk checks totals, not addresses**: it does not compare a leaf's
  output address with the page it should map, and after the switch nothing but
  the UART is read through the lower window, so a window that dropped a region
  the kernel still points into would pass every boot.
- **The direct path's lower map in phase A** treats the first gigabyte as Device
  and the rest as Normal (non-cacheable with `C` clear), which allows
  speculative accesses to addresses with nothing behind them; QEMU `virt`
  tolerates it and the path is for QEMU only (ADR 0008). The layout assumption
  is in arch code because it is gone once phase B rebuilds the window from the
  device tree.
- **Memory above 512 GiB** (RAM, not only the UART) is refused with an error at
  boot, as the bootloader's identity map already limits it.
- **Executable memory outside the image** does not exist after phase B on either
  path. The firmware's runtime services regions are not mapped at all in the
  kernel's tables; calling them (ADR 0013: not before K4's successors) needs a
  decision.
- **The first instructions on the bootloader path** run from the bootloader's
  map, which is writable and executable by contract, until `WXN` is set a few
  instructions into the upper half.
