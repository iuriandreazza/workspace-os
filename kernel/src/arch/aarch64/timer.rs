//! The Arm generic timer's virtual timer, as EL1 sees it (ADR 0035).
//!
//! The virtual timer is the one an EL1 kernel can always use: the physical
//! one depends on `CNTHCTL_EL2.EL1PCEN`, which firmware that left EL2 for the
//! kernel may not have set. Only registers live here; which interrupt the
//! timer raises, and what to do when it does, is for the layers above.

use core::arch::asm;

const CONTROL_ENABLE: u64 = 1 << 0;
const CONTROL_MASK: u64 = 1 << 1;
const CONTROL_CONDITION_MET: u64 = 1 << 2;

/// `CNTFRQ_EL0`: the counter's frequency in hertz as firmware programmed it.
/// Zero where firmware never did.
#[cfg(not(feature = "selftest-zero-frequency"))]
pub fn frequency() -> u64 {
    let hertz: u64;
    // SAFETY: reading `CNTFRQ_EL0` has no side effects and EL1 may always read it.
    unsafe { asm!("mrs {}, cntfrq_el0", out(reg) hertz, options(nomem, nostack, preserves_flags)) };
    hertz
}

/// A firmware that never programmed the frequency, for the boot that checks
/// the refusal.
#[cfg(feature = "selftest-zero-frequency")]
pub fn frequency() -> u64 {
    0
}

/// `CNTVCT_EL0`: the virtual count. The `isb` keeps the read from being
/// executed ahead of the instructions before it.
pub fn count() -> u64 {
    let ticks: u64;
    // SAFETY: reading `CNTVCT_EL0` has no side effects and EL1 may always read it.
    unsafe {
        asm!(
            "isb",
            "mrs {}, cntvct_el0",
            out(reg) ticks,
            options(nomem, nostack, preserves_flags)
        )
    };
    ticks
}

/// `CNTV_CVAL_EL0`: the timer fires when the count reaches this value.
pub fn set_deadline(count: u64) {
    // SAFETY: writing the compare value only moves when the timer fires.
    unsafe {
        asm!("msr cntv_cval_el0, {}", "isb", in(reg) count, options(nomem, nostack, preserves_flags))
    };
}

/// Whether the timer's condition is met (`ISTATUS`): the count has reached the
/// deadline and the line it drives is asserted.
pub fn is_firing() -> bool {
    let control: u64;
    // SAFETY: reading `CNTV_CTL_EL0` has no side effects.
    unsafe {
        asm!("mrs {}, cntv_ctl_el0", out(reg) control, options(nomem, nostack, preserves_flags))
    };
    control & CONTROL_CONDITION_MET != 0
}

fn set_control(value: u64) {
    // SAFETY: `CNTV_CTL_EL0` only enables, disables or masks the timer.
    unsafe {
        asm!("msr cntv_ctl_el0, {}", "isb", in(reg) value, options(nomem, nostack, preserves_flags))
    };
}

/// Enabled and unmasked: it raises its interrupt when the deadline passes.
pub fn start() {
    set_control(CONTROL_ENABLE);
}

/// Disabled and masked, whatever firmware left running.
pub fn stop() {
    set_control(CONTROL_MASK);
}

/// Whether the timer is enabled and unmasked: it can raise its interrupt.
pub fn is_running() -> bool {
    let control: u64;
    // SAFETY: reading `CNTV_CTL_EL0` has no side effects.
    unsafe {
        asm!("mrs {}, cntv_ctl_el0", out(reg) control, options(nomem, nostack, preserves_flags))
    };
    control & CONTROL_ENABLE != 0 && control & CONTROL_MASK == 0
}
