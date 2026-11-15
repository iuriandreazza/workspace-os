//! Self-test of the memory image the kernel starts with.
//!
//! The `.bss` probe checks the boot code, which clears `.bss`, and keeps the
//! section non-empty so there is always something to clear. The `.data` probe
//! checks whoever loaded the image: nothing in the kernel prepares `.data`.

use core::hint::black_box;
use core::sync::atomic::{AtomicU64, Ordering};

const INITIALISED_PATTERN: u64 = 0x5AFE_C0DE_D00D_F00D;

static ZEROED_PROBE: AtomicU64 = AtomicU64::new(0);
static INITIALISED_PROBE: AtomicU64 = AtomicU64::new(INITIALISED_PATTERN);

pub fn assert_initialised() {
    // `black_box` keeps the compiler from folding the loads into constants.
    let zeroed = black_box(&ZEROED_PROBE).load(Ordering::Relaxed);
    let initialised = black_box(&INITIALISED_PROBE).load(Ordering::Relaxed);

    assert!(zeroed == 0, ".bss was not cleared by the boot code");
    assert!(
        initialised == INITIALISED_PATTERN,
        ".data was not loaded with the kernel image"
    );
}
