---
title: "ADR 0013: Boot handoff structure between the bootloader and the kernel"
type: adr
status: accepted
created: 2026-11-15
updated: 2026-11-15
tags: [boot, bootloader, handoff, uefi, limine, aarch64, x86_64]
---

# ADR 0013: Boot handoff structure between the bootloader and the kernel

## Context

ADR 0008 decided on an own UEFI bootloader and left two questions open: "the
contents of the handoff structure" and, implicitly, how the kernel is entered.
The research is in `docs/k2b-bootloader-research.md`; the facts that shape this
decision are:

- The two places the system runs disagree on the machine. QEMU `virt` has RAM
  at `0x40000000`; VirtualBox's Arm machine has it at `0x08000000`, allocates
  the UART address, and describes itself with a device tree and, optionally,
  ACPI. The kernel cannot assume either.
- Under UEFI the firmware memory map is the authority, not the device tree.
- It is **not established** that either firmware publishes a device tree to the
  OS: QEMU's EDK2 exposes ACPI by default, and for VirtualBox I did not find the
  driver that would publish one. Either may offer ACPI only.
- VirtualBox's manual says Arm VMs have no serial port; its source builds a
  PL011 when the port is enabled. If it does not work, the kernel's output
  cannot be seen there without a framebuffer.
- The kernel today is entered with the MMU off, is linked at a fixed
  `0x40200000`, parks every core whose MPIDR affinity is not 0, and overwrites
  `x1` and `x2`. The first two are wrong for a UEFI start.
- x86_64 (K14) cannot run in long mode with paging off, so the CPU state at
  entry cannot be identical on both architectures.

## Decision

One handoff structure, `BootInfo`, the same on both architectures. The
bootloader builds it, the kernel reads it. What differs per architecture is
only the state of the processor at entry, specified separately below.

### The structure

A C-layout (`#[repr(C)]`) structure, little-endian, every address physical,
every field naturally aligned. It starts with a frozen header that no future
version may change, followed by a body that only grows.

Frozen header (the first 32 bytes):

| Field | Type | Meaning |
|---|---|---|
| `magic` | u64 | the ASCII bytes `WSOSBOOT` |
| `version_major` | u16 | changes when an existing field changes meaning or layout |
| `version_minor` | u16 | changes when fields are appended |
| `size` | u32 | size of the whole structure in bytes |
| `console_kind` | u32 | 0 none, 1 PL011, 2 NS16550 over MMIO, 3 NS16550 over port I/O |
| `reserved` | u32 | zero |
| `console_base` | u64 | MMIO base or I/O port of the console, 0 if none |

The console is in the frozen header so that a kernel that does not understand
the rest can still tell the user why it stopped.

Body, version 1.0:

| Field | Type | Meaning |
|---|---|---|
| `boot_mode` | u32 | 0 normal, 1 recovery (ADR 0007, K2c) |
| `arch` | u32 | 1 aarch64, 2 x86_64 |
| `boot_cpu` | u64 | MPIDR affinity bits (aarch64) or APIC ID (x86_64) of the processor running the kernel |
| `memory_map` | u64 | address of the array of memory map entries |
| `memory_map_count` | u64 | number of entries |
| `memory_map_entry_size` | u32 | size of one entry; 24 in version 1.0 |
| `reserved` | u32 | zero |
| `device_tree` | u64 | address of a copy of the device tree blob, 0 if none |
| `device_tree_size` | u64 | its size |
| `rsdp` | u64 | address of the ACPI RSDP, 0 if none |
| `kernel_base` | u64 | physical address where the kernel image was loaded |
| `kernel_size` | u64 | its size in memory, including `.bss` |
| `efi_system_table` | u64 | the UEFI system table, 0 when not started through UEFI |

A memory map entry is `{ base: u64, length: u64, kind: u32, flags: u32 }`,
flags zero. Kinds use Limine's numbers 0 to 6 and 8, so reading Limine's code
and protocol stays direct:

| Kind | Value | Meaning |
|---|---|---|
| Usable | 0 | RAM free for the kernel |
| Reserved | 1 | not to be touched |
| AcpiReclaimable | 2 | ACPI tables and AML; free once the kernel has copied what it needs |
| AcpiNvs | 3 | ACPI non-volatile storage |
| BadMemory | 4 | treat as reserved |
| BootloaderReclaimable | 5 | holds `BootInfo`, the memory map, the device tree copy, and former boot services memory; free once the kernel no longer reads them |
| Kernel | 6 | the kernel image |
| ReservedMapped | 8 | runtime services code and data, and other firmware memory the OS must keep |

The EFI conversion follows Limine: LoaderCode, LoaderData, BootServicesCode
and BootServicesData become BootloaderReclaimable; RuntimeServices code and
data become ReservedMapped; ACPIReclaim and ACPINVS keep their own kinds;
ConventionalMemory is Usable; anything else is Reserved.

Guarantees the bootloader gives, and a test checks (the gate, with a
host-side test on the conversion): entries are sorted by `base`; Usable and
BootloaderReclaimable entries are 4 KiB aligned and overlap nothing; the
kernel image is one contiguous Kernel entry. The map is converted into
fixed-size entries after `ExitBootServices`, because the firmware's own
descriptor size is only known per call.

Rules about content:

- **The memory map is the authority for RAM.** The device tree copy keeps its
  `memory` nodes, but the kernel must not size memory from them when entered
  through the bootloader. K3's allocator takes a list of regions, fed from this
  map here and from the device tree on a direct QEMU load.
- **Either `device_tree` or `rsdp` may be absent, both may be present.** Today
  the kernel can use only the device tree (ADR 0010). If the bootloader finds
  neither, it stops on the menu with a message; if it finds only ACPI, it
  says so, and the kernel cannot boot on that machine until an ACPI reader
  exists. This is a risk, not a decision: see "What is not yet known".
- **`SetVirtualAddressMap` is not called.** Runtime services stay at physical
  addresses. The kernel does not call them in K2b to K4; whoever needs them
  revisits this.
- **Boot services are gone** when the kernel starts.
- **No framebuffer yet.** The structure grows by a minor version when K2e needs
  one.

### Versions

- The kernel accepts any `version_minor` of a `version_major` it knows. It reads
  a body field only when the field lies entirely inside `size`; a field past
  `size` is absent and takes its default. Trailing fields it does not know are
  ignored. Minor versions only append.
- An unknown **major** version, a `size` smaller than the frozen header, or a bad
  `magic` after the entry check below: the kernel prints
  `unsupported boot info version <major>.<minor>` on the console named in the
  frozen header if `magic` and the header are valid and `console_kind` is
  known, and halts. With nothing to print on, it halts silently, as it does
  today when the device tree is unreadable.
- The version has one definition in a small crate shared by the bootloader and
  the kernel, like `devicetree` is for ADR 0010, so the two cannot disagree on
  the layout and a host test can check it.

### How the kernel tells the two entries apart

- **Entry from the bootloader:** `x0` is the physical address of `BootInfo` and
  `x1` is a 64-bit magic, `0x57534f53454e5452` (ASCII `WSOSENTR`). The kernel
  checks `x1` first, in assembly, before anything else, then validates `BootInfo`.
- **Direct load by QEMU** (the existing gate, ADR 0008): `x1` is not that magic.
  The existing path stays as it is: `x0` is the device tree address or 0, and
  `board::discover` falls back to the start of RAM. That fallback never runs on
  the bootloader path.
- `boot.s` must save `x1` before it overwrites it, and must **not park the core by
  MPIDR affinity** on the bootloader path: the processor that enters is the boot
  processor by definition, and the field `boot_cpu` records its identity.
  This is a requirement on the code part of K2b.

### Processor state at entry, aarch64

- Non-secure, **at EL1**, little-endian AArch64. If the firmware ran the
  bootloader at EL2, the bootloader drops to EL1 itself before the jump, with
  the MMU state below already set up for EL1. `boot.s` must therefore not take
  its EL2-to-EL1 drop on this path, because that drop would discard the MMU
  configuration; the direct-load path keeps its drop.
- `PSTATE.DAIF` all masked.
- **MMU on, instruction and data caches on**, 4 KiB granule, with an identity
  map in `TTBR0_EL1` built by the bootloader: every address in the memory map
  maps to itself, RAM as Normal write-back cacheable and everything else the
  firmware described as MMIO as Device, read, write and execute. `MAIR_EL1`,
  `TCR_EL1` and `SCTLR_EL1` are set consistently with that map and are
  otherwise unspecified. `TTBR1_EL1` is unused.
- The page tables are in memory of kind BootloaderReclaimable. The kernel must
  not reuse that memory until it has installed its own tables (K4). K3's
  allocator therefore never hands out BootloaderReclaimable memory before K4.
- The bootloader loaded the kernel through the cacheable map, so data is
  coherent; it invalidates the instruction cache over the image before the
  jump.
- `x0` and `x1` as above; `x2` and `x3` are zero. All other registers undefined.
- The kernel sets its own stack, in its own `.bss`; no stack of the bootloader
  is used. Only the boot processor runs; the others are not started until K11.
- FP and SIMD are not used (kernel rule).
- `boot.s` today assumes the MMU and caches are off and writes `SCTLR_EL1`
  with only the reserved bits. On this path it must leave `SCTLR_EL1` alone
  until K4 and must not clean or invalidate by set and way. A kernel that does
  not touch `SCTLR_EL1` also keeps working with the MMU on only if its own
  accesses to device memory and its atomics use the memory types of the
  bootloader's map; that is a requirement on the code part.
- Kernels that need the MMU off (Linux, K2d) are entered by their own protocol
  code, which turns the MMU off from identity-mapped code and cleans the image
  first; that code is part of K2d, not of this structure.

### Processor state at entry, x86_64 (for K14, not implemented now)

Long mode requires paging, so paging is on at entry: an identity mapping of the
memory in the map that the bootloader sets up, interrupts masked, a GDT of the
bootloader's, and the pointer to `BootInfo` and the entry magic in registers
that K14's ADR fixes. The structure above does not change; K14 amends this ADR
with the register choice and the page-table rules. This is the cost of one
structure over two architectures, accepted on purpose: the data contract
stays portable and the CPU state does not.

### Load address

The kernel stays linked where it is, at `0x40200000` for K2b. The bootloader
allocates exactly that range with `AllocatePages(AllocateAddress)`; if it is
taken or not RAM, the bootloader stops on the menu and says which address and
why. On VirtualBox that means the VM needs about 900 MiB or more of RAM. A
relocatable kernel or a higher-half one lifts the restriction (K4), see the
alternatives.

## Alternatives considered

### Relation to the Limine protocol

| Option | For | Against |
|---|---|---|
| Own structure, with Limine's memory map kinds and EFI conversion rules (chosen) | Fits the kernel as it is (MMU off, no relocation); one struct and one entry convention, simple to test on the host; Limine's tested conversion table is reused; the kernel is not tied to another project's revisions | We maintain the contract and the loader's side; we do not get Limine's request features (framebuffer, modules, MP) for free, we add them as minor versions |
| Adopt the Limine protocol as it is | Existing, documented, used by many kernels; framebuffer, modules, MP and entropy exist today; other Limine kernels could be loaded by the same code | The kernel must be higher-half with its own request section and run with the MMU on from the first instruction, which means K4 first or a rewrite of `boot.s` now; base revisions are deprecated every few releases (0 to 5 are deprecated at the pinned commit); the bootloader must build page tables; ADR 0008 chose to avoid tying the kernel to it |
| Adapt only its request and response mechanism, with our own features | Version negotiation per feature, extensible without a global version | The loader scans the kernel image for request magics: more code on both sides for a handoff with a dozen fields; still needs a decision on MMU and memory layout |

### Processor state: MMU

Chosen by the user: MMU on with an identity map. The "MMU off" row is the option that was recommended and not taken.

| Option | For | Against |
|---|---|---|
| MMU off | Matches `boot.s` and the Linux boot document, so one entry convention serves our kernel and Linux (K2d); the bootloader's cache maintenance is a short, fixed list | The bootloader turns the MMU off from identity-mapped code, which is delicate and must be tested; the kernel runs uncached until K4, so early memory is slow and exclusive accesses need care |
| MMU on, identity map (chosen) | The kernel starts cached; no mode switch in the bootloader just before the jump | The kernel must take over page tables it did not make and describe where they are; `boot.s` changes; Linux needs the MMU off anyway, so the bootloader would need both paths |

### How the kernel tells the entries apart

| Option | For | Against |
|---|---|---|
| `x1` carries a magic (chosen) | A register compare in assembly, before any memory is read or any core is parked; unambiguous against QEMU's direct load, which does not set `x1` | Uses a second register; the Linux convention says `x1` is zero, but we are not Linux and the bootloader sets it only for our own kernel |
| Magic at `*x0` | One register; the device tree already starts with `0xd00dfeed` so the two are distinguishable | Reads memory at a register value before the kernel knows it is valid; a wrong `x0` faults early; it cannot tell the core not to park |
| Separate ELF entry symbol | No ambiguity at all | Two entry points to keep alive; QEMU's ELF loader and the bootloader each need to know theirs; still needs the MPIDR decision |

### Load address

| Option | For | Against |
|---|---|---|
| Fixed `0x40200000`, fail loudly (chosen for K2b) | No change to the kernel; the failure is understandable | VirtualBox VMs below about 900 MiB cannot boot; the address may already be used by firmware, which is only found out by running; the restriction stays until K4 |
| Position-independent kernel, relocated by the bootloader | Works wherever there is room | Relocation processing in the kernel's startup or in the loader, a change to the build and to `boot.s`, before the MMU exists to make it cheap |
| Higher-half kernel (as Limine) | One virtual address, any physical placement; what K4 builds anyway | Needs the MMU and page tables, so it is K4's work and not available for K2b |

## Consequences

- The bootloader and the kernel share one `no_std` crate with the layout and
  the checks. `scripts/check.sh` gains a host test for it, and a gate boot under
  QEMU with UEFI firmware.
- `boot.s` grows a second entry convention, loses the MPIDR park and the
  EL2 drop and the `SCTLR_EL1` reset on that path; this is an edit to files the K3 session is also touching, which is why
  the code part of K2b waits for K3.
- K3 should take its input as a list of regions, not read the device tree
  directly, so the bootloader path can feed it. A note for that session is in
  the shared notes folder.
- Reading the device tree for memory becomes the direct-load path only.
- The bootloader's recovery entry works by setting `boot_mode`; what recovery
  does is K13's.
- Kernels other than ours (K2d) use their own protocols, not `BootInfo`.
- Until K4 the kernel runs on the bootloader's identity map; replacing it is
  K4's first job, and K4's test must cover entry through the bootloader too.
- The kernel cannot boot on a machine whose firmware offers only ACPI until it
  has an ACPI reader. This is a known limit, not an oversight.

## What is not yet known

These need an experiment, and are the first job of the code part of K2b:

- Which configuration tables (DTB, ACPI RSDP) QEMU's EDK2 and VirtualBox's
  firmware publish, with ACPI on and off. If either gives ACPI only, the
  gate uses `-machine virt,acpi=off` for QEMU, and for VirtualBox the VM's
  ACPI setting is turned off if the setting allows it; otherwise K2b's
  VirtualBox half waits for an ACPI reader (a new roadmap item).
- Whether the UART works at all on VirtualBox Arm (manual and source disagree);
  if not, VirtualBox output depends on K2e.
- The exact state of the processor while the UEFI application runs (the
  specification could not be read).
- Licences of the transitive dependencies of the UEFI crate chosen
  (research, section 7); with the MIT option elected for `uefi` and `r-efi`.

## Update, when K14 starts

The register choice and page-table rules for x86_64 are added here or in an ADR
that amends this one.

## Findings of K2b-2

The questions under "What is not yet known" were answered by running the
bootloader on both firmwares (QEMU EDK2 `edk2-stable202408`, VirtualBox 7.2.22
on Apple silicon). No decision above changed.

- **Tables.** QEMU `virt` with ACPI on (the default) publishes ACPI 2.0 and
  SMBIOS 3 tables and **no device tree**; with `-machine virt,acpi=off` it
  publishes the device tree. VirtualBox with ACPI on publishes ACPI only; with
  `--acpi off` it publishes a device tree (8 KiB) that names a PL011 UART at
  `0xffdde000`. Both are therefore run with ACPI off, as this ADR foresaw.
- **UART.** VirtualBox Arm has a working PL011 when the virtual serial port is
  enabled (`--uart1` with a file or pipe mode); the manual's statement that Arm
  VMs have no serial port is outdated for 7.2.22. The kernel's banner reaches
  the host through it.
- **Processor state.** Both firmwares start the bootloader at EL1 with the MMU
  and caches on, a 4 KiB granule and a `MAIR_EL1` with Device-nGnRnE at index 0
  and Normal write-back at index 3. `-machine virtualization=on` starts it at
  EL2, which the bootloader leaves itself (it programs EL1's translation
  registers from EL2's and returns to the kernel with `eret`). The
  bootloader keeps the firmware's `MAIR_EL1` and `TCR_EL1` and only installs a
  new `TTBR0_EL1`, so it never changes the translation setup while it runs on it.
- **Load address on VirtualBox.** RAM starts at `0x08000000` and the firmware
  reserves `0x3c170000..0x44000000` on a 1024 MiB machine, which covers
  `0x40200000`. The VM needs 2048 MiB (tested) or more; this is stricter than
  the "about 900 MiB" this ADR estimated.
- **Linker script.** The boot stack is now its own `NOLOAD` section, so the ELF
  segment's size in memory includes it and the bootloader reserves image and
  stack as one Kernel entry.
- **Licences.** `uefi` 0.41, `uefi-raw`, `uefi-macros`, `bitflags`, `bit_field`,
  `cfg-if`, `log`, `uguid`, `ptr_meta` and the build-time `syn`, `quote`,
  `proc-macro2` and `unicode-ident` are MIT or MIT/Apache-2.0 (used under MIT).
  `ucs2` 0.3.3 is MPL-2.0, which lists GPL-2.0 as a permitted secondary licence
  for a combined work; its files stay under the MPL.
- **Limits of K2b-2, known and kept.** The identity map covers the first
  512 GiB (or what `TCR_EL1` translates, if less): Normal memory beyond it
  stops the boot with a message, and a Device above it is unmapped. Firmware
  regions of kinds AcpiReclaimable, AcpiNvs and ReservedMapped are mapped
  Normal write-back whatever EFI attributes they carry, and Device mappings are
  execute-never. The ELF loader places segments by `p_paddr` and checks
  `e_entry` against them, which K4's higher-half link must revisit. The kernel
  file buffer is allocated before the kernel's own range, which assumes a
  firmware that allocates top-down (the range failing is reported with the
  firmware's memory map).
