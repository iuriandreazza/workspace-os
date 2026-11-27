mod aarch64;

#[cfg(feature = "selftest-compiler-trap")]
pub use aarch64::raise_compiler_trap;
#[cfg(feature = "selftest-nested-exception")]
pub use aarch64::raise_nested_exception;
#[cfg(feature = "selftest-unhandled-class")]
pub use aarch64::raise_unhandled_class;
#[cfg(feature = "selftest-unhandled-vector")]
pub use aarch64::raise_unhandled_vector;
pub use aarch64::{
    Handover, IrqGuard, SVC_EXIT, SVC_SAMPLE_PSTATE, SVC_YIELD, Side, TrapFrame, boot_stack,
    current_el, daif, disable_interrupts, enable_interrupts, exception_stack, exit_current_thread,
    give_gicv2_to_non_secure, give_gicv3_to_non_secure, halt, irqs_masked, kernel_image,
    memory_attributes, pointer_to, run_interrupt_selftest, run_selftest, switch_selftest,
    system_control, translation_root, yield_now,
};
pub use aarch64::{gic_cpu, mmu, timer};
#[cfg(feature = "selftest-stack-overflow")]
pub use aarch64::{overflow_stack, stack_probe_intact};
