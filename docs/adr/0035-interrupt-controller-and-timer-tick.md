---
title: "ADR 0035: Interrupts: GICv2 and GICv3 read from the device tree, the virtual timer's tick, and an EL3 stand-in for firmware"
type: adr
status: proposed
created: 2026-11-26
updated: 2026-11-26
tags: [kernel, interrupts, timer, gic, el3, safety]
---

# ADR 0035: Interrupts: GICv2 and GICv3 read from the device tree, the virtual timer's tick, and an EL3 stand-in for firmware

## Context

K6 gives the kernel its first asynchronous exception: a periodic tick from the
Arm generic timer, taken through the interrupt controller. K7 builds the
scheduler on it. Constraints:

- The kernel holds mechanism only (ADR 0002). It counts ticks; what a tick is
  for belongs to whatever decides.
- Addresses come from what the boot delivers, not from the source (ADR 0033):
  the controller's registers go through the device window like the console's.
- Exception entry is ADR 0026's: a separate exception stack, the interrupted SP
  saved in the frame, nested exceptions reported and not handled.
- The kernel is entered at EL1, EL2 or EL3 (direct load) or at EL1 (bootloader,
  which drops from EL2 itself). The gate boots all of them, with one and four
  cores.
- Layers: timer registers, the GICv3's CPU interface (system registers) and IRQ
  masking in `arch`, the controllers in `drivers`, addresses and device tree
  reading in `board`.

What the machines have, measured with `-machine dumpdtb`, and run:

| Machine | Controller | Timer |
|---|---|---|
| QEMU `virt` default, `virtualization=on`, `secure=on`, both, `-smp 4`, through UEFI (`acpi=off`) | `intc@8000000`, `arm,cortex-a15-gic`, GICv2: distributor `0x8000000`, CPU interface `0x8010000`, 64 KiB each | `arm,armv8-timer`, PPIs 13, 14, 11, 10; counter 62.5 MHz (1 GHz on `-cpu max`) |
| the same with `gic-version=3` | `arm,gic-v3`: distributor `0x8000000` (64 KiB), one redistributor region `0x80a0000` (`0xf60000` bytes, room for 123 cores), `#redistributor-regions = 1`, an ITS | the same |
| VirtualBox 7.2.22 Arm VM, ACPI off, run with `docs/booting-the-image.md` | a GICv3, distributor `0xfcd30000`, redistributors `0xfcd40000` (32 MiB), read by the kernel from the device tree | counter 24 MHz |

VirtualBox on Apple silicon is a place the project runs (ADR 0004, K4d's
criterion) and cannot be in the gate. This ADR supports its controller; what was
run there and what was not is in the pull request.

With the security extensions (`secure=on`, where the kernel starts at EL3) the
GIC has two worlds, and the kernel runs in the non-secure one. Measured, not
assumed:

- GICv2: with nothing done at EL3 a non-secure kernel could not take an
  interrupt, for two separate reasons. Interrupts reset to Group 0, which is
  secure, and a non-secure write to `ISENABLER` of a Group 0 interrupt does not
  take; and the secure `GICC_PMR` resets to 0, which makes the interface ignore
  the non-secure side's writes to its own mask and signal it nothing (each
  without its fix: "the controller did not enable interrupt 27", and "no tick").
- GICv3: without anything done at EL3 the enable of the timer's PPI is refused
  ("the controller did not enable interrupt 27"). Putting the distributor in a
  single security state (`GICD_CTLR.DS`, a secure write) is enough: the
  non-secure kernel then sets the groups, affinity routing and enables itself,
  and ticks arrive at EL1, EL2 and EL3 starts.
- Not shown by a failing boot, and so not claimed as needed: on QEMU a GICv3
  boots without `ICC_SRE_EL2` and `ICC_SRE_EL3` being written, and without the
  kernel setting `ICC_SRE_EL1.SRE` (it already reads 1). The architecture
  leaves those unknown at reset, and a kernel at EL1 cannot reach the interface
  where they are clear, so the boot code, the bootloader and the kernel write
  them; no gate boot fails without them, and the consequences say so.

## Decision

### The controllers: GICv2 and GICv3, read from the device tree, checked against the hardware

The kernel finds the controller as the first usable direct child of the root
with an `interrupt-controller` property.

- `arm,cortex-a15-gic` or `arm,gic-400`: a GICv2. The two register blocks are
  `reg` entries 0 (distributor) and 1 (CPU interface). The distributor's
  architecture revision (`GICD_PIDR2`) must read 2.
- `arm,gic-v3` (the tree also calls a GICv4 that): a GICv3. `reg` entry 0 is the
  distributor and entry 1 the redistributor region. Exactly one region and no
  `redistributor-stride` are supported; the revision must read 3 or 4.
- Anything else is refused by name before the kernel's tables are built: a
  controller it does not know, redistributors in several regions or with a
  stride, no device tree (the bootloader's copy is optional in the boot info),
  no controller or no timer node. The gate boots a tree whose compatible string
  was changed to one nobody defines and checks that refusal.

Refusing is chosen over booting without interrupts because K7 and everything
after it depend on the tick: a kernel that silently has none would fail later and
farther from the cause.

Only the boot core is set up, and only interrupts private to a core (IDs 0 to
31) can be enabled; shared peripheral interrupts need targets or routing and a
group, and come with the first device that raises one. The other cores stay
parked in the boot code with IRQs masked, so nothing is routed to them.

The controller's state is never assumed to be the reset state. Firmware (the
UEFI path) uses the timer and the controller, so before anything is enabled the
driver disables the distributor's groups (keeping affinity routing as firmware
left it, since turning it off under a core that uses the system registers is not
something the architecture promises), clears every enable, pending and active bit
(and a GICv2's pending SGIs through `GICD_CPENDSGIR`), writes the CPU interface
control register whole (which clears an EOI mode firmware may have set), sets the
priority mask and binary point, and drains what the interface already signalled.
A GICv3's redistributor is found by walking its region for this core's affinity
(`MPIDR_EL1`; the walk and the packing are the `giclayout` crate, tested on the
host with fake regions, because no QEMU boot has its core anywhere but in the
first slot; a slot whose ID registers do not say GICv3 is never matched), woken (`GICR_WAKER`, polled a bounded number of times), and its
SGIs and PPIs are made Group 1 and cleared; the distributor gets affinity routing
and Group 1, in the bit that the controller's security state says (`DS`).

An acknowledge that returns a special ID (1020 to 1023) says nothing is pending
for this world: it is counted as spurious and nothing is ended. Any other ID is
ended with the whole value the register returned (a GICv2 SGI carries its
sender).

### The timer: the virtual timer, interrupt taken from the device tree

The virtual timer (`CNTV_*`, interrupt PPI 11, ID 27) is used, not the physical
one. An EL1 kernel can always use it; the physical timer depends on
`CNTHCTL_EL2.EL1PCEN`, which the boot code and the bootloader both set (to 3)
but which firmware that leaves EL2 for the kernel on its own may not have.
The interrupt ID is the third entry of the timer node's `interrupts` (secure
physical, non-secure physical, virtual, hypervisor), as a PPI, plus 16: it is
read from the tree, not fixed.

Firmware may have left the timer running: it is stopped before anything else.

A tick is 10 ms (100 Hz): enough resolution for a scheduler, cheap under
emulation. The deadline moves by one period in each interrupt from the previous
deadline (`CNTV_CVAL_EL0`), so the period does not drift with the time the
handler takes; a core that fell more than a period behind skips the ticks it
missed and does not fire them back to back. The line is level-sensitive, so the
deadline moves before the end of the interrupt; the other order would make the
controller signal it again at once.

`CNTFRQ_EL0` is read, never assumed (VirtualBox's is 24 MHz, QEMU's 62.5 MHz or
1 GHz). Where firmware never programmed it (it reads zero) the tree's
`clock-frequency` on the timer node is used; with neither the boot is refused
with a message: a period cannot be computed from zero. A `selftest-zero-frequency`
build reads zero and the gate checks the refusal.

### The IRQ entry: the exception stack of ADR 0026, once

Vector entry 5 (IRQ, current EL with SP_ELx) shares its code with the
synchronous entry (one macro): the interrupted SP is exchanged for the top of the
exception stack, the whole context is saved and scrubbed, the Rust handler runs
and the context comes back. The test of "was this taken on the exception stack
itself" stays.

An IRQ never nests on the exception stack: exception entry masks IRQs, and no
handler unmasks them (`interrupts::handle_irq` prints nothing and unmasks
nothing; the console's SAFETY argument relies on the first). `halt` and the
panic path mask IRQs first.

### The tick is a counter, nothing more

`interrupts::ticks()` is the whole interface to the rest of the kernel. It is
mechanism only (ADR 0002): no time slices, timeouts or wakeups. The scheduler
of K7 decides what a tick means and may change the period.

### The system register interface of a GICv3

A GICv3's CPU interface is system registers (`ICC_*`), off for EL1 until every
level above has allowed it: `ICC_SRE_EL3` and `ICC_SRE_EL2` (SRE, DFB, DIB and
Enable) and then `ICC_SRE_EL1.SRE`. The boot code writes the first two where
it passes through EL3 and EL2, and the bootloader the second where it drops from
EL2, each only where `ID_AA64PFR0_EL1.GIC` says the registers exist (writing a
register the core lacks is undefined, and there is no vector table yet). The
kernel then turns on `ICC_SRE_EL1.SRE`, reads it back and refuses with a message
if it did not stay on.

Limit: a GICv2 machine whose cores have the system register interface in a
legacy mode (a GIC-500 or GIC-600 in GICv2 compatibility) would lose its
memory-mapped CPU interface when started at EL2 or EL3, because the boot code
cannot read the device tree to know. No machine of the project is one.

### EL3: the boot code stands in for the firmware that is missing

When the kernel is started at EL3 there is no firmware (QEMU's `-kernel` with
`secure=on` runs the image at EL3 and nothing else). The boot code leaves for
the non-secure EL1, so it must also do what a secure monitor such as TF-A does
for the GIC. It cannot do that before leaving, because the controller's address
is in the device tree, which is read at EL1 with the MMU on. It therefore
installs a monitor, `VBAR_EL3` and one handler in `boot.s`, that answers `smc`
from EL1 or EL2 (function ids in the Arm SMC Calling Convention's range for
vendor-specific EL3 monitors):

- `0x8700_0000`, GICv2: `x1` = physical address of the distributor, `x2` = that of
  the CPU interface. It sets `GICD_IGROUPR0` to all ones (the boot core's SGIs
  and PPIs are Group 1, which the non-secure side can enable) and `GICC_PMR` to
  `0x80` (so the non-secure side's writes to its own mask take effect).
- `0x8700_0001`, GICv3: `x1` = physical address of the distributor. It sets
  `GICD_CTLR.DS`: one security state, in which the non-secure kernel configures
  the rest.
- it returns 0, or `-1` for another function. It runs with the MMU off, uses no
  stack and trusts its caller completely: nothing else runs in the secure
  world. A monitor that wrote an address of the caller's choosing would be a
  hole if there were a secure world to protect; there is none, and the
  comment says so.

The kernel only calls it when the boot code went through EL3 (`x20`, passed to
`kernel_main`); everywhere else the firmware that started the kernel gave the
non-secure world the controller, and an enable that did not take is a panic with
a message, not a silent hang.

Limits: the monitor's code is read by EL3 with the MMU off, from the physical
address of the image, so on hardware with caches the code must be clean to the
point of coherency; the kernel's tables never change it, and QEMU's TCG models
no cache.

### The gate

Every boot of the gate that reaches the end (direct, raw, memory sizes, one and
four cores, EL1, EL2, EL3, UEFI, and each of the entry levels again on a GICv3:
EL1, EL2, EL3, EL3 with EL2, four cores, `-cpu max`, and through the bootloader
at EL1 and at EL2) checks the interrupt report: the controller at the addresses
of the tree and in the device window at the expected offsets, the timer, at least
`TICKS_TO_WAIT` ticks, no more than the timer can fire (`ticks * period <=
elapsed + period`, which a storm of unacknowledged interrupts breaks) and not
fewer than a fifth of that (`elapsed <= 5 * ticks * period`), none spurious or
unexpected, and a context test: every register, the flags, SP, `SP_EL0` and 512
bytes just below SP, loaded with distinct values, come back after a timer
interrupt taken while they are live. Two more boots check the refusals: a
controller the kernel does not know, and a zero frequency.

Combinations QEMU does not allow or the gate does not boot: UEFI at EL3 (the
firmware does not start there), `gic-version=3` with ACPI (the bootloader refuses
ACPI-only machines at any controller), VirtualBox (it cannot run in the gate).

## Consequences

- The kernel boots on GICv2 and GICv3 machines, with one redistributor region
  and no stride; another layout is refused by name. The refusals of a
  redistributor layout and of a device tree that disagrees with the registers
  (`GICD_PIDR2`) have no boot of their own: a tree for either would have to be
  built by hand.
- The device window holds the console and the controller's two blocks: about
  132 KiB with guard pages for a GICv2, about 15.5 MiB for QEMU's GICv3 and
  32 MiB for VirtualBox's, mapped in 4 KiB pages. `kernelvm`'s audit takes a
  list of device ranges.
- A kernel started at EL3 carries a small monitor in the boot code. It is the
  only secure-world code, and it exists for the direct load only.
- The `ICC_SRE_EL1`, `ICC_SRE_EL2` and `ICC_SRE_EL3` writes, and the refusal when
  `ICC_SRE_EL1.SRE` does not stay on, are not shown necessary by any boot the
  gate can make (QEMU boots without them); they are there because the
  architecture leaves those enables unknown at reset, and are exercised on
  VirtualBox only to the extent that it boots with them.
- The tick is stopped again at the end of the boot's own test (IRQs masked, timer
  stopped) until K7 starts it for the scheduler.
- Cores other than the boot core have no interrupt setup; K11 brings them up.
- The controller and the timer are looked up among the direct children of the
  root, and the timer's `interrupts` are read as the GIC's three cells. A board
  whose GIC sits under another bus is refused ("no interrupt controller") until the
  reader follows `interrupt-parent` and `#interrupt-cells`.
- The virtual timer is the third `interrupts` entry of the timer node, as the
  binding orders them; a tree with `interrupt-names` and fewer entries is refused
  ("the timer's interrupts") until the names are read.
- The context test may return without a tick (a `wfi` is allowed to), and is
  tried up to three times before the boot gives up.
- The fallback to the tree's `clock-frequency` has no boot of its own: neither
  QEMU's tree nor VirtualBox's has one, so only the refusal is exercised.

## Alternatives considered

- **Physical timer.** Works where the boot code or the bootloader set
  `CNTHCTL_EL2`; depends on that. The virtual timer does not.
- **A fixed controller address in `boot.s` for the EL3 setup**, to avoid the
  `smc`. Rejected: it is the address ADR 0033 says must not live in the source,
  in the one place no check would notice it being wrong.
- **Kernel in the secure world** (`SCR_EL3.NS=0`): the GIC would need no
  stand-in, but the tables, the memory attributes and every later device would
  differ from the non-secure case that is what firmware gives on real boards.
- **GICv2 only, GICv3 in the next item.** Decided against: VirtualBox's Arm VM
  is a GICv3, so the first K6 would have stopped the VirtualBox boot of
  `docs/booting-the-image.md` at a refusal. A first version of this ADR did
  that; this one supersedes it.
- **GICv3 with the ITS, LPIs or several redistributor regions.** Nothing
  raises an LPI yet, and the layouts of the machines above need one region.
