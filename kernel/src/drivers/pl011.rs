//! ARM PL011 UART, transmit only.
//!
//! No baud rate or line setup: QEMU's PL011 transmits without it. Real
//! hardware will need initialisation before the first byte.

use core::fmt;
use core::ptr::{read_volatile, write_volatile};

const DATA_REGISTER: usize = 0x00;
const FLAG_REGISTER: usize = 0x18;
const FLAG_TRANSMIT_FIFO_FULL: u32 = 1 << 5;

pub struct Pl011 {
    base: usize,
}

impl Pl011 {
    /// # Safety
    ///
    /// `base` must be the address of a mapped PL011 register block that no
    /// other code is driving.
    pub const unsafe fn new(base: usize) -> Self {
        Self { base }
    }

    fn transmit_fifo_is_full(&self) -> bool {
        // SAFETY: `new` guarantees `base` points at a PL011 register block.
        let flags = unsafe { read_volatile((self.base + FLAG_REGISTER) as *const u32) };
        flags & FLAG_TRANSMIT_FIFO_FULL != 0
    }

    fn write_byte(&mut self, byte: u8) {
        while self.transmit_fifo_is_full() {
            core::hint::spin_loop();
        }
        // SAFETY: `new` guarantees `base` points at a PL011 register block.
        unsafe { write_volatile((self.base + DATA_REGISTER) as *mut u32, u32::from(byte)) };
    }
}

impl fmt::Write for Pl011 {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for byte in text.bytes() {
            // A terminal on a raw serial line needs CR before LF.
            if byte == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(byte);
        }
        Ok(())
    }
}
