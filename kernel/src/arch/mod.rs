mod aarch64;

#[cfg(feature = "selftest-compiler-trap")]
pub use aarch64::raise_compiler_trap;
#[cfg(feature = "selftest-unhandled-class")]
pub use aarch64::raise_unhandled_class;
#[cfg(feature = "selftest-unhandled-vector")]
pub use aarch64::raise_unhandled_vector;
pub use aarch64::{
    boot_stack, current_el, halt, kernel_image, memory_attributes, run_selftest, translation_root,
};
