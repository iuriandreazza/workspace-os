//! The CPU interface of a GICv3 as EL1 sees it: system registers, not memory
//! (ADR 0035). The distributor and the redistributors are memory, and belong to
//! `drivers::gicv3`. The registers are named by encoding, which every assembler
//! accepts.

use core::arch::asm;

const PFR0_GIC_SHIFT: u32 = 24;
const SRE_ENABLE: u64 = 1 << 0;
const GROUP_ENABLE: u64 = 1;
/// `ICC_CTLR_EL1` with the EOI mode that splits ending an interrupt in two
/// cleared, and the common binary point off.
const CONTROL_RESET: u64 = 0;

/// Whether this core has the system register interface to a GICv3
/// (`ID_AA64PFR0_EL1.GIC`). A core without it makes the registers below
/// undefined.
pub fn system_registers_present() -> bool {
    let pfr0: u64;
    // SAFETY: reading an ID register has no side effects.
    unsafe {
        asm!("mrs {}, id_aa64pfr0_el1", out(reg) pfr0, options(nomem, nostack, preserves_flags))
    };
    (pfr0 >> PFR0_GIC_SHIFT) & 0xf != 0
}

/// Turns the system register interface on for EL1 (`ICC_SRE_EL1.SRE`) and
/// reports whether it stayed on: a higher exception level that did not enable
/// it leaves the bit clear, or traps the access.
///
/// The caller has seen `system_registers_present`.
pub fn enable_system_registers() -> bool {
    let sre: u64;
    // SAFETY: the caller checked the registers exist. Setting SRE only selects
    // the system register interface for this core, and is read back.
    unsafe {
        asm!(
            "mrs {sre}, S3_0_C12_C12_5",
            "orr {sre}, {sre}, #1",
            "msr S3_0_C12_C12_5, {sre}",
            "isb",
            "mrs {sre}, S3_0_C12_C12_5",
            sre = out(reg) sre,
            options(nomem, nostack, preserves_flags)
        )
    };
    sre & SRE_ENABLE != 0
}

/// Lets every priority through the mask (`ICC_PMR_EL1`), sets the binary point
/// and the control register to known values, and turns Group 1 interrupts on
/// (`ICC_IGRPEN1_EL1`).
pub fn enable_group_1(priority_mask: u8) {
    // SAFETY: the caller turned the system registers on. These registers only
    // set what the interface signals to this core.
    unsafe {
        asm!(
            // The redistributor's and the distributor's writes are complete
            // before the interface starts signalling.
            "dsb sy",
            "msr S3_0_C4_C6_0, {mask}",
            "msr S3_0_C12_C12_3, xzr",
            "msr S3_0_C12_C12_4, {control}",
            "isb",
            "msr S3_0_C12_C12_7, {enable}",
            "isb",
            mask = in(reg) u64::from(priority_mask),
            control = in(reg) CONTROL_RESET,
            enable = in(reg) GROUP_ENABLE,
            options(nomem, nostack, preserves_flags)
        )
    };
}

/// `ICC_IAR1_EL1`: takes the highest priority pending Group 1 interrupt. The
/// value is its ID, or one of the special IDs 1020 to 1023 when there is none.
pub fn acknowledge() -> u32 {
    let raw: u64;
    // SAFETY: reading the register acknowledges an interrupt; the caller ends
    // what it takes.
    unsafe {
        asm!("mrs {}, S3_0_C12_C12_0", out(reg) raw, options(nomem, nostack, preserves_flags))
    };
    raw as u32
}

/// `ICC_EOIR1_EL1`: ends the interrupt `acknowledge` returned.
pub fn end_of_interrupt(raw: u32) {
    // SAFETY: the caller passes what `acknowledge` returned.
    unsafe {
        asm!("msr S3_0_C12_C12_1, {}", "isb", in(reg) u64::from(raw), options(nomem, nostack, preserves_flags))
    };
}

/// This core's affinity as a redistributor's `GICR_TYPER` names it, from
/// `MPIDR_EL1`.
pub fn affinity() -> u32 {
    let mpidr: u64;
    // SAFETY: reading `MPIDR_EL1` has no side effects.
    unsafe { asm!("mrs {}, mpidr_el1", out(reg) mpidr, options(nomem, nostack, preserves_flags)) };
    giclayout::affinity(mpidr)
}
