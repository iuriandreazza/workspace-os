---
title: "ADR 0026: Hardening the kernel address space: exception stack, permission indirection, audited addresses"
type: adr
status: accepted
created: 2026-11-19
updated: 2026-11-19
tags: [kernel, memory, mmu, aarch64, security, exceptions]
---

# ADR 0026: Hardening the kernel address space

## Context

ADR 0019 gave the kernel its own tables and listed what it left open under
"Known limits". This ADR closes four of them (K4b). It complements ADR 0019 and
changes none of its decisions; the only edit to ADR 0019 is that its list of
limits points here for the items closed.

- **A stack overflow overwrites `.bss`.** The guard page makes an overflow
  fault, but the synchronous handler saves its context on the stack it
  interrupted, so it faults again, `SP` falls 288 bytes per round, and after
  about fifteen rounds the saves land in the top of `.bss` and a report is
  printed over it.
- **FEAT_S1PIE.** On a core that has it, `TCR2_EL1.PIE` resets to an unknown
  value, and if it is set the descriptors' `AP` and `PXN` bits mean something
  else.
- **The audit does not look at output addresses or memory types.** A page that
  maps the wrong frame, or an image page with a wrong `AttrIndx`, passes it.
- **The lower window is never exercised.** After the switch only the UART and
  the tables are read through it, so a window that lost the boot info, the
  memory map or the device tree copy passes every boot.

## Decision

### An exception stack

Synchronous exceptions taken from EL1 run on their own stack, never on the one
they interrupted.

- The image gets a second stack of 16 KiB, `.exception_stack`, above the boot
  stack, with one unmapped guard page between them (the same shape as the
  guard page below the boot stack). It is part of the loaded segment (NOLOAD,
  like the boot stack), so the loaders reserve it with the image and the frame
  allocator is told about it (`arch::exception_stack`, which includes the guard
  page).
- The vector entry switches stacks with no stack to save registers on. It parks
  `x0` in `TPIDR_EL1` (which the kernel does not use until K11 gives each core
  its own data, and which then needs another scratch register), loads the top of
  the exception stack, and exchanges it with `SP` using `add`, `sub` and `sub`
  on `sp` and `x0`, which needs no third register. The interrupted `SP` is
  saved in the frame, which grows from 288 to 304 bytes (`FRAME_SP`), and is
  restored the same way before `eret`. The breakpoint self-test checks that `SP`
  comes back.
- An exception taken while `SP` already lies in the exception stack (the handler
  or the report faulted) does not reuse it: it goes to the fatal path, which
  starts again from the top of the exception stack and reports `nested
  exception`. The fatal path (`unhandled_exception_entry`) now uses the
  exception stack too, instead of the boot stack.
- The result of an overflow of the boot stack: the store into the guard page
  faults, the handler runs on the exception stack, the report names a data
  abort with a level 3 translation fault at an address in the guard page, and
  `.bss` is untouched.
- How the gate shows `.bss` intact: the linker script reserves the last 8 KiB of
  `.bss` (the part the pre-fix cascade overwrote, right below the guard page) as
  `__stack_probe_start..__stack_probe_end`. It is zeroed by the boot code like
  the rest of `.bss`. The overflow self-test fills it with a pattern before it
  recurses, and the panic handler of that build prints whether the pattern is
  whole. The gate requires the fault report and the line "intact".
- Not solved: a frame larger than the guard page's 4 KiB can still step over it
  (no stack probes). The 288-byte cascade was the fault the guard page created;
  a large frame is a different defect, found by the compiler's stack probes
  (`probe-stack`) and not addressed here.

### Permission indirection (FEAT_S1PIE)

`boot.s` reads `ID_AA64MMFR3_EL1` (read as zero where it does not exist) before
the MMU is configured, on both paths. If `TCRX` (bits 3:0) is non-zero,
`TCR2_EL1` is written with zero: `PIE` clear, so the descriptors' `AP` and
`PXN` bits mean what ADR 0019 says, and the other fields (128-bit descriptors,
permission overlays) off. If `SCTLRX` (bits 7:4) is non-zero, `SCTLR2_EL1` is
written with zero for the same reason. Where a register does not exist nothing
is written: the write would be UNDEFINED, and the behaviour is the one ADR 0019
describes.

When the kernel is entered at EL3 or EL2 the drop must also let EL1 write
them, or the write traps to a level with no handler and the boot hangs without
output (found in review, reproduced on `-cpu max` at EL2 and EL3): the EL3
path sets `SCR_EL3.TCR2En` and `SCTLR2En` (bits 43 and 44) where the registers
exist and `HXEn` (bit 38) where `HCRX_EL2` does, so that EL2 may write it, and the EL2 path writes `HCRX_EL2` (where `ID_AA64MMFR1_EL1.HCX` says it
exists) with `TCR2En` and `SCTLR2En` (bits 14 and 15) set for the same cases and
the rest clear.

The same holds for the bootloader's own drop from EL2 to EL1
(`enter_kernel_from_el2`, which writes `HCRX_EL2` under the same conditions, and
only when there is an enable to set, since the write itself traps to EL3 unless
`SCR_EL3.HXEn` allows it; it also clears EL1's `TCR2_EL1` and `SCTLR2_EL1`
before the MMU comes on, because they are unknown at reset and the firmware only
used EL2's, so the kernel's first fetches must not be read under a stale `PIE`;
found in round 2: the UEFI boot at EL2 on `-cpu max` ended in the firmware's
exception handler at the first `TCR2_EL1` write).

When the kernel is entered at EL1 or EL2 by something else, the firmware or
hypervisor must have let that level write them (`SCR_EL3.HXEn` and `TCR2En`,
`HCRX_EL2.TCR2En`); fine-grained trap controls (`HFGWTR_EL2.nTCR2_EL1`) are not
cleared, as no other trap control of that kind is. QEMU without EL3 does not
exercise these preconditions, so they are read against the architecture.

The condition is the register's own existence (`TCRX`, `SCTLRX`), not S1PIE
alone: S1PIE implies `TCR2_EL1`, and a core with `TCR2_EL1` but no S1PIE also
gets its unknown reset value cleared. The registers are written with the MMU in
whatever state the entry left it; clearing `PIE` on the bootloader path changes
nothing the firmware's map relied on, since the firmware does not enable it, and
the `tlbi` that follows covers the rest.

`mmu::verify` reads the live registers: where `TCRX` is set it requires
`TCR2_EL1.PIE` and `D128` clear, where `SCTLRX` is set it reads `SCTLR2_EL1`,
and it prints which case it saw. QEMU's `-cpu max` has the features and resets
the register to zero, so a missing write cannot show there by itself: a build
with `selftest-dirty-tcr2` sets `TCR2_EL1.PIE` just before the code that must
clear it, which stands for firmware that left it set, and the gate requires the
bit clear afterwards.

### An audit that checks what a leaf maps

`kernelvm::audit` takes an `Expectation` and reports, besides what it did:

- **Output address.** Both halves map by a constant offset (0 in the lower half,
  `KERNEL_OFFSET` in the upper one), so a leaf whose output is not its virtual
  address minus the root's base is wrong (`wrong_output`).
- **Memory type.** `AttrIndx` must be the Normal or the Device index; a leaf
  that touches the expected Device range (the UART page) must be Device and
  every other leaf Normal (`wrong_memory_type`).
- **Section.** When the expectation lists the known ranges (the image by section
  and, for the boot tables, the 2 MiB tail), every leaf must lie inside one and
  carry that range's permissions (`wrong_section`). This catches what totals
  cannot: permissions exchanged between sections of the same total size, a page
  that maps another section's frame, a page outside the image.

The same walk runs on the boot tables before the switch (`check_boot_tables`)
and on the live tables after it (`verify`). The lower half's window is checked
for identity and memory type; which regions it holds is checked by the next
item, since a dropped region leaves no wrong leaf behind.

### Reading the hand-over through the window

After the switch the kernel reads the boot info, the memory map and the device
tree again, through the lower window, and the gate sees the result. `main`
takes a digest of the hand-over before `mmu::init` (every field the board
holds, every entry of the memory map, every memory and reserved region, and all
the bytes of the device tree blob) and, after `verify`, parses the boot info (or
the device tree, on the direct path) again from its address and digests that.
The two must be equal or the kernel panics; the report line carries both. A
window that lost a region faults on the read (a boot that ends in a reported
abort), one that maps the wrong frame gives a different digest.

The digest is FNV-1a, 64 bits. It is not a security measure: it is a compact
way to say "the same bytes were read".

## Alternatives considered

### Where the exception stack lives

| Option | For | Against |
|---|---|---|
| Part of the image, with a guard page (chosen) | The tables, the loaders and the allocator already treat the image as one reserved range; a guard page makes its own overflow fault, and the fatal path then resets | One more section in `link.ld`, in the boot tables and in `ImageLayout`; 20 KiB more image |
| A static array in `.bss` | No linker change | Its overflow runs into other `.bss`, the thing this item protects |
| A frame from the allocator | Placed anywhere | The allocator runs after the vectors are installed, so early faults have no stack; mapping it needs the tables first |
| `SP_EL0` as the exception stack | No scratch register to swap | K8 gives `SP_EL0` to user space; the frame would have to save and restore it around every use |

### How the stack is switched

| Option | For | Against |
|---|---|---|
| `TPIDR_EL1` for `x0` and the add/sub exchange (chosen) | No memory touched before the switch; two instructions of overhead | `TPIDR_EL1` is spoken for until K11 |
| Save `x0` and `x1` on the interrupted stack first | No system register | The very store that faults on overflow |
| Test `SP` against the stack and switch only on overflow | The common path stays as it is | Two paths to test; the overflow path is the rare one and would be the least tested; a guard page skipped by a large frame goes unseen by the test |

### Proving `.bss` is intact

| Option | For | Against |
|---|---|---|
| Linker-reserved probe at the top of `.bss` (chosen) | It is exactly the region the cascade damaged; no dependence on where the compiler puts a static | 8 KiB of `.bss` reserved in every build |
| A Rust static as a canary | No linker change | Its place in `.bss` is the compiler's: it may not be in the region at risk, and the check passes by luck |
| A checksum of all of `.bss` | No reserved space | `.bss` changes while it is hashed, and the panic path itself writes to it |

### S1PIE: write conditions

| Option | For | Against |
|---|---|---|
| Clear `TCR2_EL1` and `SCTLR2_EL1` where each exists (chosen) | The state is defined whatever the firmware did; nothing is written where the write is UNDEFINED | A write the QEMU gate can only show with a hook (below) |
| Support S1PIE (a `PIR_EL1`) | Finer permissions | Not needed; two mechanisms (descriptors and `WXN`) already stand, and it widens the audit |
| Do nothing and document | No code | The state stays unknown on cores that have it |

### The hook that dirties `TCR2_EL1`

QEMU resets the register to zero, so without a hook the gate cannot tell a boot
that clears it from one that does not. A feature-gated instruction before the
clearing stands for firmware that left `PIE` set. The alternative, a gate that
only reports the register's value, would pass the removal of the write.

## Consequences

- `link.ld`, `boot.s`, `exceptions.s`, `exceptions.rs`, `mmu.rs` and `kernelvm`
  change. The linker script exports `__exception_stack_bottom`,
  `__exception_stack_top`, `__stack_probe_start` and `__stack_probe_end`.
- The ELF segment is 20 KiB larger. `kernel_image()` and `boot_stack()` keep
  their meaning; `exception_stack()` is new and the allocator reserves it.
- `ImageLayout` has a fifth part and the audit has a new signature.
- The gate adds: a boot that overflows the stack, direct and through the
  bootloader, with the fault and the intact probe checked; the S1PIE case on
  `-cpu max`, with the dirty-register build, direct and through the bootloader;
  the section, address and type checks on every normal boot (the audit is part
  of `verify`); the digest line on every boot. Each check was confirmed by
  breaking the behaviour it covers (listed in the pull request).
- The TCG still models neither TLBs nor caches. Everything in "Known limits" of
  ADR 0019 about maintenance sequences stands.

## Known limits

- **A frame larger than the guard page** (4 KiB) still steps over it. The
  compiler's stack probes would close it; not done.
- **`TPIDR_EL1` is a scratch register** of the vector entry until per-core data
  needs it (K11).
- **The `SCTLR2_EL1` clear is not demonstrated.** QEMU resets it to zero and the
  gate has no build that dirties it, so deleting that one write passes. Only the
  `TCR2_EL1` write has a dirty-register boot.
- **The edges of the nested range are not exercised.** The nested boot raises a
  second exception deep in the exception stack; moving either end of the range
  by a page (the guard page above the boot stack, or the top) passes the gate.
- **A nested exception is reported, not recovered.** The exception stack is
  reset to its top by the fatal path, which overwrites the frame the first
  exception saved; nothing returns from it.
- **S1PIE is only cleared, not used.** Only `-cpu max` on QEMU has exercised the
  path; no physical core with the feature was tried.
- **The digest shows the same bytes were read, not that they are right.** It
  compares the read after the switch with the one before.
