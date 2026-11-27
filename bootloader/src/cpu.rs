//! The processor-level steps of the handoff: reading the translation setup the
//! firmware left, cache maintenance and the jump into the kernel (ADR 0013,
//! "Processor state at entry, aarch64").

use core::arch::asm;

const SCTLR_M: u64 = 1 << 0;
const SCTLR_A: u64 = 1 << 1;
const SCTLR_C: u64 = 1 << 2;
const SCTLR_I: u64 = 1 << 12;
const SCTLR_WXN: u64 = 1 << 19;
const TCR_TG0_MASK: u64 = 0b11 << 14;
const TTBR_ASID_MASK: u64 = 0xffff << 48;
/// Aff3 and Aff2..0 of `MPIDR_EL1`: the identity of the processor, without the
/// multiprocessing and uniprocessor flag bits.
const MPIDR_AFFINITY: u64 = 0xff_00ff_ffff;

pub fn current_el() -> u64 {
    let value: u64;
    // SAFETY: reading `CurrentEL` has no side effects.
    unsafe { asm!("mrs {}, CurrentEL", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value >> 2
}

/// Whether `HCR_EL2.E2H` is set, which makes EL2 use the EL1 register formats.
pub fn virtualization_host_extensions() -> bool {
    let hcr: u64;
    // SAFETY: reading `HCR_EL2` has no side effects; only called at EL2.
    unsafe { asm!("mrs {}, hcr_el2", out(reg) hcr, options(nomem, nostack, preserves_flags)) };
    hcr & (1 << 34) != 0
}

/// Masks every interrupt: the firmware's handlers must not run while the
/// bootloader rebuilds what they could depend on.
pub fn mask_interrupts() {
    // SAFETY: setting the DAIF mask bits only stops interrupts being taken.
    unsafe { asm!("msr daifset, #0xf", options(nomem, nostack)) };
}

pub fn boot_cpu() -> u64 {
    let value: u64;
    // SAFETY: reading `MPIDR_EL1` has no side effects.
    unsafe { asm!("mrs {}, mpidr_el1", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value & MPIDR_AFFINITY
}

/// What the live translation setup of EL1 looks like.
pub struct Translation {
    pub mair: u64,
    /// `64 - T0SZ`: the width of the addresses `TTBR0_EL1` translates.
    pub address_bits: u8,
    /// Whether `TCR_EL1` selects the 4 KiB granule for `TTBR0_EL1`.
    pub granule_4k: bool,
}

/// The translation the firmware runs with, at the level it started us at: EL1,
/// or EL2 with its own registers (the host is not using VHE, which would alias
/// the EL1 names to EL2's; `run` refuses that case).
pub fn translation() -> Translation {
    let (mair, tcr): (u64, u64);
    // SAFETY: reading translation registers has no side effects.
    unsafe {
        if current_el() == 2 {
            asm!("mrs {}, mair_el2", out(reg) mair, options(nomem, nostack, preserves_flags));
            asm!("mrs {}, tcr_el2", out(reg) tcr, options(nomem, nostack, preserves_flags));
        } else {
            asm!("mrs {}, mair_el1", out(reg) mair, options(nomem, nostack, preserves_flags));
            asm!("mrs {}, tcr_el1", out(reg) tcr, options(nomem, nostack, preserves_flags));
        }
    }
    Translation {
        mair,
        address_bits: (64 - (tcr & 0x3f)) as u8,
        granule_4k: tcr & TCR_TG0_MASK == 0,
    }
}

fn data_cache_line() -> usize {
    let ctr: u64;
    // SAFETY: reading `CTR_EL0` has no side effects.
    unsafe { asm!("mrs {}, ctr_el0", out(reg) ctr, options(nomem, nostack, preserves_flags)) };
    4 << ((ctr >> 16) & 0xf)
}

/// Cleans the data cache to the point of coherency over `start..end`, so that
/// whatever reads the memory without the cache (the table walker, if the
/// firmware set it up that way) sees what was written.
pub fn clean_to_coherency(start: usize, end: usize) {
    let line = data_cache_line();
    let mut address = start & !(line - 1);
    while address < end {
        // SAFETY: cleaning a line by address changes no data, whatever the
        // address; it only writes the line back if it is dirty.
        unsafe { asm!("dc cvac, {}", in(reg) address, options(nostack, preserves_flags)) };
        address += line;
    }
    // SAFETY: a barrier.
    unsafe { asm!("dsb ish", options(nostack, preserves_flags)) };
}

/// Makes the code written to `start..end` visible to instruction fetch.
pub fn sync_instructions(start: usize, end: usize) {
    let line = data_cache_line();
    let mut address = start & !(line - 1);
    while address < end {
        // SAFETY: cleaning a line by address changes no data.
        unsafe { asm!("dc cvau, {}", in(reg) address, options(nostack, preserves_flags)) };
        address += line;
    }
    // SAFETY: barriers and an instruction cache invalidate: no memory changes.
    unsafe {
        asm!(
            "dsb ish",
            "ic ialluis",
            "dsb ish",
            "isb",
            options(nostack, preserves_flags)
        )
    };
}

/// Installs the identity map, turns the caches on as the ADR specifies and
/// enters the kernel at EL1. Never returns.
///
/// # Safety
///
/// Boot services must be gone and nothing may use the firmware afterwards.
/// `root` must be the root of a map that identity maps this code, its stack,
/// the kernel image and the boot info, with the memory types of the live
/// `MAIR` of the level the bootloader runs at, built for that level's `TCR`.
/// `entry` must be the kernel's entry point.
pub unsafe fn enter_kernel(entry: u64, boot_info: u64, root: u64, entry_magic: u64) -> ! {
    if current_el() == 2 {
        // SAFETY: forwarded from the caller.
        unsafe { enter_kernel_from_el2(entry, boot_info, root, entry_magic) }
    }
    // SAFETY: the caller guarantees the map is sound. DAIF is masked first so
    // that nothing interrupts the switch. The firmware's tables are replaced
    // with the MMU off, which is the break of break-before-make: the two trees
    // may describe the same addresses with blocks of different sizes, and a
    // TLB holding entries of both would be a conflict. The code runs from the
    // identity map, so it fetches from the same addresses with the MMU off,
    // and the window touches no memory: registers only. SCTLR_EL1 keeps what
    // the firmware set and turns the MMU and caches on, with alignment checks
    // and write-implies-execute-never off.
    unsafe {
        asm!(
            "msr daifset, #0xf",
            "msr spsel, #1",
            "mrs x6, sctlr_el1",
            "orr x10, x6, x8",
            "bic x10, x10, x9",
            "bic x11, x6, x12",
            "mrs x13, ttbr0_el1",
            "and x13, x13, x7",
            "orr x13, x13, x5",
            "dsb sy",
            "msr sctlr_el1, x11",
            "isb",
            "msr ttbr0_el1, x13",
            "isb",
            "tlbi vmalle1",
            "dsb nsh",
            "isb",
            "msr sctlr_el1, x10",
            "isb",
            "br x4",
            in("x7") TTBR_ASID_MASK,
            in("x8") SCTLR_M | SCTLR_C | SCTLR_I,
            in("x9") SCTLR_A | SCTLR_WXN,
            in("x12") SCTLR_M,
            in("x0") boot_info,
            in("x1") entry_magic,
            in("x2") 0u64,
            in("x3") 0u64,
            in("x4") entry,
            in("x5") root,
            options(noreturn)
        )
    }
}

/// The same handoff when the firmware ran us at EL2 (ADR 0013): EL1 gets its
/// own translation registers from the ones EL2 used, the map is installed
/// there with the MMU and caches on, EL2 is configured the way the boot code
/// configures it for a direct load (EL1 is AArch64, nothing traps to EL2) and
/// `eret` lands in the kernel at EL1.
unsafe fn enter_kernel_from_el2(entry: u64, boot_info: u64, root: u64, entry_magic: u64) -> ! {
    // TCR_EL2 (no VHE) to TCR_EL1: the same T0SZ, IRGN0, ORGN0, SH0 and TG0
    // fields, the physical address size moved to IPS, and TTBR1 walks off.
    let tcr_el2: u64;
    let mair: u64;
    // SAFETY: reading registers has no side effects.
    unsafe {
        asm!("mrs {}, tcr_el2", out(reg) tcr_el2, options(nomem, nostack, preserves_flags));
        asm!("mrs {}, mair_el2", out(reg) mair, options(nomem, nostack, preserves_flags));
    }
    let tcr_el1 = (tcr_el2 & 0xffff) | (1 << 23) | (((tcr_el2 >> 16) & 7) << 32);
    // SCTLR_EL1: the reserved-as-one bits and the MMU and caches on. Everything
    // else, alignment checks and WXN included, is off.
    let sctlr_el1 = 0x30d0_0800 | SCTLR_M | SCTLR_C | SCTLR_I;
    // The kernel clears TCR2_EL1 and SCTLR2_EL1 where they exist (ADR 0026),
    // and EL1 may only write them if HCRX_EL2.TCR2En and SCTLR2En allow it. The
    // register itself exists only with FEAT_HCX, and its other bits have unknown
    // reset values: write it, with those two enables set for the registers the
    // processor has and the rest clear, only where it exists and there is an
    // enable to set. TCR2_EL1 and SCTLR2_EL1 of EL1 are also unknown at reset
    // and the firmware only used EL2's: they are cleared here, before EL1 runs
    // with the MMU on, where the processor has them.
    let (mmfr1, mmfr3): (u64, u64);
    // SAFETY: reading ID registers has no side effects; ID_AA64MMFR3_EL1 is
    // reserved and reads as zero where it does not exist.
    unsafe {
        asm!("mrs {}, S3_0_C0_C7_1", out(reg) mmfr1, options(nomem, nostack, preserves_flags));
        asm!("mrs {}, S3_0_C0_C7_3", out(reg) mmfr3, options(nomem, nostack, preserves_flags));
    }
    // A GICv3's system registers are off for EL1 until EL2 allows them
    // (ICC_SRE_EL2: SRE, DFB, DIB and Enable), and the register does not exist
    // on a core without the interface (ID_AA64PFR0_EL1.GIC); the kernel turns
    // the interface on itself when the device tree says it drives a GICv3
    // (ADR 0035). A GICv2 machine whose cores have the interface in a legacy
    // mode would lose its memory-mapped CPU interface here.
    let pfr0: u64;
    // SAFETY: reading an ID register has no side effects.
    unsafe {
        asm!("mrs {}, id_aa64pfr0_el1", out(reg) pfr0, options(nomem, nostack, preserves_flags))
    };
    let has_gic_system_registers = u64::from((pfr0 >> 24) & 0xf != 0);
    let has_tcr2 = u64::from(mmfr3 & 0xf != 0);
    let has_sctlr2 = u64::from((mmfr3 >> 4) & 0xf != 0);
    let hcrx = (has_tcr2 << 14) | (has_sctlr2 << 15);
    // Nothing to enable means no write: HCRX_EL2 traps to EL3 unless allowed.
    let write_hcrx = u64::from((mmfr1 >> 40) & 0xf != 0 && hcrx != 0);
    // SAFETY: the caller guarantees the map and the entry. EL1's translation
    // registers are written while EL1 runs nothing, so no switch is visible;
    // the `eret` takes the kernel to EL1 with the MMU on, in the identity map.
    unsafe {
        asm!(
            "msr daifset, #0xf",
            "msr mair_el1, x6",
            "msr tcr_el1, x7",
            "msr ttbr0_el1, x5",
            "isb",
            // EL2 may flush every EL1 entry of any VMID; what EL1's TLB holds
            // out of reset is unknown.
            "tlbi alle1",
            "dsb ish",
            "isb",
            "cbz x12, 3f",
            "msr S3_0_C2_C0_3, xzr",
            "3:",
            "cbz x13, 4f",
            "msr S3_0_C1_C0_3, xzr",
            "4:",
            "msr sctlr_el1, x8",
            "isb",
            // EL1 is AArch64 (HCR_EL2.RW), and nothing it does traps to EL2:
            // reset values are unknown, so every trap control is cleared.
            "mov x9, #(1 << 31)",
            "msr hcr_el2, x9",
            "mov x9, #0x33ff",
            "msr cptr_el2, x9",
            "msr mdcr_el2, xzr",
            "msr hstr_el2, xzr",
            "cbz x10, 2f",
            "msr S3_4_C1_C2_2, x11",
            "2:",
            "mrs x9, midr_el1",
            "msr vpidr_el2, x9",
            "mrs x9, mpidr_el1",
            "msr vmpidr_el2, x9",
            "cbz x14, 5f",
            "mov x9, #0xf",
            "msr S3_4_C12_C9_5, x9",
            "isb",
            "5:",
            "mov x9, #3",
            "msr cnthctl_el2, x9",
            "msr cntvoff_el2, xzr",
            // EL1h, all of DAIF masked.
            "mov x9, #0x3c5",
            "msr spsr_el2, x9",
            "msr elr_el2, x4",
            "eret",
            in("x0") boot_info,
            in("x1") entry_magic,
            in("x2") 0u64,
            in("x3") 0u64,
            in("x4") entry,
            in("x5") root,
            in("x6") mair,
            in("x7") tcr_el1,
            in("x8") sctlr_el1,
            in("x10") write_hcrx,
            in("x11") hcrx,
            in("x12") has_tcr2,
            in("x13") has_sctlr2,
            in("x14") has_gic_system_registers,
            options(noreturn)
        )
    }
}
