//! The answer to a `LoadFile2` call for the initial ramdisk (UEFI
//! specification, `EFI_LOAD_FILE2_PROTOCOL.LoadFile()`; the Linux stub's side
//! of it is in `efi_load_initrd_dev_path`, see `docs/k2d-other-kernels.md`).
//! The firmware calling convention is in the bootloader; the decision is here so
//! the host can test the size handshake the stub depends on.

/// What `LoadFile()` does with one call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    /// `EFI_UNSUPPORTED`: `LoadFile2` is never called with the boot policy.
    Unsupported,
    /// `EFI_INVALID_PARAMETER`: no `BufferSize` to answer through.
    InvalidParameter,
    /// `EFI_BUFFER_TOO_SMALL`, with `*BufferSize` set to the initrd's size.
    /// The stub's first call has a null buffer and expects exactly this.
    BufferTooSmall,
    /// Copy the initrd into the buffer and set `*BufferSize` to its size.
    Copy,
}

/// `buffer_size` is the value `*BufferSize` held, `None` for a null pointer.
pub fn reply(
    boot_policy: bool,
    buffer_size: Option<usize>,
    buffer_is_null: bool,
    initrd_len: usize,
) -> Reply {
    if boot_policy {
        return Reply::Unsupported;
    }
    match buffer_size {
        None => Reply::InvalidParameter,
        Some(size) if buffer_is_null || size < initrd_len => Reply::BufferTooSmall,
        Some(_) => Reply::Copy,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stubs_first_call_learns_the_size() {
        // A null buffer and a size of zero: the call the Linux stub makes first.
        assert_eq!(reply(false, Some(0), true, 4096), Reply::BufferTooSmall);
    }

    #[test]
    fn a_buffer_that_fits_is_filled() {
        assert_eq!(reply(false, Some(4096), false, 4096), Reply::Copy);
        assert_eq!(reply(false, Some(8192), false, 4096), Reply::Copy);
    }

    #[test]
    fn a_buffer_one_byte_short_is_not_filled() {
        assert_eq!(reply(false, Some(4095), false, 4096), Reply::BufferTooSmall);
    }

    #[test]
    fn a_null_buffer_is_never_written_whatever_the_size_says() {
        assert_eq!(
            reply(false, Some(1 << 30), true, 4096),
            Reply::BufferTooSmall
        );
    }

    #[test]
    fn the_boot_policy_and_a_missing_size_are_refused() {
        assert_eq!(reply(true, Some(4096), false, 4096), Reply::Unsupported);
        assert_eq!(reply(false, None, false, 4096), Reply::InvalidParameter);
    }
}
