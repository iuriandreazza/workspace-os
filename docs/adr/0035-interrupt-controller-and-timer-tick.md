---
title: "ADR 0035: Interrupts: a GICv2 read from the device tree, the virtual timer's tick, and an EL3 stand-in for firmware"
type: adr
status: proposed
created: 2026-11-26
updated: 2026-11-26
tags: [kernel, interrupts, timer, gic, el3, safety]
---

# ADR 0035: Interrupts: a GICv2 read from the device tree, the virtual timer's tick, and an EL3 stand-in for firmware

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
- Layers: timer registers and IRQ masking in `arch`, the controller in
  `drivers`, addresses and device tree reading in `board`.

What the machines the gate boots have, measured with `-machine dumpdtb`:

| Boot | Controller | Timer node |
|---|---|---|
| default, `virtualization=on`, `secure=on`, both, `-smp 4` | `intc@8000000`, `arm,cortex-a15-gic`, GICv2, distributor `0x8000000`, CPU interface `0x8010000`, 64 KiB each | `arm,armv8-timer`, interrupts PPI 13, 14, 11, 10 |
| through UEFI (`acpi=off`) | the same tree, copied by the bootloader | the same |
| `gic-version=3` | `arm,gic-v3` | the same |

With the security extensions (`secure=on`, where the kernel starts at EL3) the
GIC has two worlds, and the kernel runs in the non-secure one. Measured, not
assumed: with nothing done at EL3 the non-secure kernel could not take an
interrupt, for two separate reasons. Interrupts reset to Group 0, which is
secure, and a non-secure write to `ISENABLER` of a Group 0 interrupt does not
take; and the secure `GICC_PMR` resets to 0, which makes the interface ignore
the non-secure side's writes to its own mask and signal it nothing.

## Decision

### The controller: GICv2 only, read from the device tree, refused by name otherwise

The kernel finds the controller as the first usable direct child of the root
with an `interrupt-controller` property. It drives it if it is compatible with
`arm,cortex-a15-gic` or `arm,gic-400` (the GICv2 of QEMU `virt` and of most
boards with a GIC-400); the two register blocks are `reg` entries 0 (distributor)
and 1 (CPU interface). It then reads the architecture revision from the
distributor (`GICD_PIDR2`) and requires 2: the tree and the hardware must
agree.

Anything else ends the boot with a panic that says why, before the kernel's
tables are built:

- `arm,gic-v3`: needs redistributors, system registers and affinity routing,
  which no board the project runs on needs yet. The gate boots
  `-machine gic-version=3` and checks the refusal.
- another compatible: the kernel does not know it.
- no device tree (the bootloader's copy is optional in the boot info), no
  controller or no timer node: nothing to wire.

Refusing is chosen over booting without interrupts because K7 and everything
after it depend on the tick: a kernel that silently has none would fail later and
farther from the cause. Supporting GICv3 is a new ADR that supersedes the
list above.

Only the boot core's CPU interface is set up, and only private interrupts (IDs
0 to 31) can be enabled; shared peripheral interrupts need targets and a group
and come with the first device that raises one. The other cores stay parked in
the boot code with IRQs masked, so nothing is routed to them.

The controller's state is never assumed to be the reset state. Firmware (the
UEFI path) uses the timer and the controller, so before anything is enabled the
driver disables the distributor, clears every enable, pending and active bit,
writes `GICC_CTLR` whole (which clears the EOI-mode bit firmware may have set),
sets the priority mask and binary point, and drains what the interface already
signalled.

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

`CNTFRQ_EL0` is read, never assumed. Where firmware never programmed it (it
reads zero) the tree's `clock-frequency` on the timer node is used; with neither
the boot is refused with a message: a period cannot be computed from zero. A
`selftest-zero-frequency` build reads zero and the gate checks the refusal.

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

The handler acknowledges the interrupt. A spurious acknowledge (ID 1023) is
counted and nothing is ended. The tick is counted and ended. Any other ID is
counted as unexpected, since no other interrupt is enabled, and ended.

### The tick is a counter, nothing more

`interrupts::ticks()` is the whole interface to the rest of the kernel. It is
mechanism only (ADR 0002): no time slices, timeouts or wakeups. The scheduler
of K7 decides what a tick means and may change the period.

### EL3: the boot code stands in for the firmware that is missing

When the kernel is started at EL3 there is no firmware (QEMU's `-kernel` with
`secure=on` runs the image at EL3 and nothing else). The boot code leaves for
the non-secure EL1, so it must also do what a secure monitor such as TF-A does
for the GIC. It cannot do that before leaving, because the controller's address
is in the device tree, which is read at EL1 with the MMU on. It therefore
installs a monitor, `VBAR_EL3` and one handler in `boot.s`, that answers one
`smc` from EL1 or EL2:

- function `0x8700_0000` (the SMC Calling Convention's range for vendor
  specific EL3 monitors), `x1` = physical address of the distributor, `x2` =
  that of the CPU interface;
- it sets `GICD_IGROUPR0` to all ones (the boot core's SGIs and PPIs are
  Group 1, which the non-secure side can enable) and `GICC_PMR` to `0x80` (so
  the non-secure side's writes to its own mask take effect);
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
four cores, EL1, EL2, EL3, UEFI) checks the interrupt report: the controller at
the addresses of the tree and in the device window at the expected offsets,
the timer, at least `TICKS_TO_WAIT` ticks, no more than the timer can fire
(`ticks * period <= elapsed + period`, which a storm of unacknowledged interrupts
breaks), none spurious or unexpected, and a context test: every register, the
flags, SP, `SP_EL0` and 512 bytes just below SP, loaded with distinct values, come
back after a timer interrupt taken while they are live. Two more boots check the
refusals: GICv3 and a zero frequency.

## Consequences

- The kernel boots only on GICv2 machines until a new ADR adds another.
- The device window holds the console and the controller's two blocks (about
  132 KiB with guard pages); `kernelvm`'s audit takes a list of device ranges.
- A kernel started at EL3 carries a small monitor in the boot code. It is the
  only secure-world code, and it exists for the direct load only.
- The tick is stopped again at the end of the boot's own test (IRQs masked, timer
  stopped) until K7 starts it for the scheduler.
- Cores other than the boot core have no interrupt setup; K11 brings them up.

## Alternatives considered

- **Physical timer.** Works where the boot code or the bootloader set
  `CNTHCTL_EL2`; depends on that. The virtual timer does not.
- **A fixed controller address in `boot.s` for the EL3 setup**, to avoid the
  `smc`. Rejected: it is the address ADR 0033 says must not live in the source,
  in the one place no check would notice it being wrong.
- **Kernel in the secure world** (`SCR_EL3.NS=0`): the GIC would need no
  stand-in, but the tables, the memory attributes and every later device would
  differ from the non-secure case that is what firmware gives on real boards.
- **GICv3 now.** No machine here needs it, and an unverified driver is worse
  than a refusal by name.
