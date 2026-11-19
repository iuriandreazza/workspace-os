//! Starting what is not our kernel (ADR 0008, ADR 0025): Linux through its EFI
//! entry, and the EFI loader of another system. Both are PE images the
//! firmware starts with `LoadImage` and `StartImage`; the loader reads the file
//! itself, checks it (ADR 0009, point 4) and hands the firmware the bytes it
//! checked. The way Limine starts a chainloaded image (read the file, build
//! the file path, load from memory, set the load options, start) was the
//! reference; no code is copied. The sources are in `docs/k2d-other-kernels.md`.

use core::ffi::c_void;
use core::mem::MaybeUninit;

use bootinfo::config::MAX_COMMAND_LINE;
use bootinfo::initrd::{self, Reply};
use bootinfo::menu::{Action, Linux};
use bootinfo::pe::{self, PeError};
use uefi::boot::{self, LoadImageSource};
use uefi::prelude::*;
use uefi::proto::device_path::build::{self, DevicePathBuilder};
use uefi::proto::device_path::{DevicePath, DeviceType};
use uefi::proto::loaded_image::LoadedImage;
use uefi::proto::media::load_file::LoadFile2;
use uefi::{CStr16, Identify, guid};

use crate::{FileError, read_file, release};

/// `LINUX_EFI_INITRD_MEDIA_GUID`, `include/linux/efi.h` in the Linux source:
/// the vendor media device path under which the stub looks for its initrd.
const LINUX_EFI_INITRD_MEDIA_GUID: uefi::Guid = guid!("5568e427-68fc-4f3d-ac74-ca555231cc68");
/// A path is at most `MAX_PATH` characters and a terminating zero.
const PATH_UNITS: usize = bootinfo::config::MAX_PATH + 1;
/// Room for the device path of the disk (a few dozen bytes) and a file node.
const DEVICE_PATH_BYTES: usize = 1024;

pub fn kind_name(action: &Action) -> &'static str {
    match action {
        Action::Kernel { .. } => "kernel",
        Action::Linux(_) => "Linux",
        Action::Chainload { .. } => "chainload",
    }
}

/// Why an entry was not started. The text is what the person reads.
pub enum Refusal<'a> {
    Missing {
        what: &'static str,
        path: &'a str,
    },
    File {
        what: &'static str,
        path: &'a str,
        step: &'static str,
        status: Status,
    },
    NotImage {
        what: &'static str,
        path: &'a str,
        error: PeError,
    },
    Empty {
        what: &'static str,
        path: &'a str,
    },
    Unreadable {
        what: &'static str,
        path: &'a str,
        why: &'static str,
    },
    Firmware {
        step: &'static str,
        status: Status,
    },
}

impl core::fmt::Display for Refusal<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Missing { what, path } => {
                write!(f, "{what} {path} is not on the EFI system partition")
            }
            Self::File {
                what,
                path,
                step,
                status,
            } => write!(f, "cannot {step} {what} {path}: {status:?}"),
            Self::NotImage { what, path, error } => {
                write!(f, "{what} {path} is not a valid EFI image: {error}")
            }
            Self::Empty { what, path } => write!(f, "{what} {path} is empty"),
            Self::Unreadable { what, path, why } => {
                write!(f, "cannot read {what} {path}: {why}")
            }
            Self::Firmware { step, status } => {
                write!(f, "the firmware refused to {step}: {status:?}")
            }
        }
    }
}

fn firmware(step: &'static str) -> impl Fn(uefi::Error) -> Refusal<'static> {
    move |error| Refusal::Firmware {
        step,
        status: error.status(),
    }
}

/// `path` as the firmware's string, in `buffer`.
fn ucs2<'b>(path: &str, buffer: &'b mut [u16; PATH_UNITS]) -> Result<&'b CStr16, Refusal<'static>> {
    CStr16::from_str_with_buf(path, buffer).map_err(|_| Refusal::Firmware {
        step: "convert a path",
        status: Status::INVALID_PARAMETER,
    })
}

/// A file of the partition, checked to be an EFI application for aarch64.
fn read_image<'a>(what: &'static str, path: &'a str) -> Result<&'static mut [u8], Refusal<'a>> {
    let bytes = read_file_at(what, path)?;
    if let Err(error) = pe::check_efi_application(bytes) {
        release(bytes);
        return Err(Refusal::NotImage { what, path, error });
    }
    Ok(bytes)
}

fn read_file_at<'a>(what: &'static str, path: &'a str) -> Result<&'static mut [u8], Refusal<'a>> {
    let mut units = [0u16; PATH_UNITS];
    let name = ucs2(path, &mut units)?;
    let bytes = read_file(name).map_err(|error| match error {
        FileError::Missing => Refusal::Missing { what, path },
        FileError::Step(step, status) => Refusal::File {
            what,
            path,
            step,
            status,
        },
        FileError::Message(why) => Refusal::Unreadable { what, path, why },
        FileError::Firmware(error) => Refusal::File {
            what,
            path,
            step: "open",
            status: error.status(),
        },
    })?;
    if bytes.is_empty() {
        release(bytes);
        return Err(Refusal::Empty { what, path });
    }
    Ok(bytes)
}

/// Starts the entry. `Ok` is the status of an image that came back (a
/// chainloaded loader that declined, or Linux's stub failing); anything that
/// stopped the start is a `Refusal`. Neither ends the boot.
pub fn start<'a>(action: &Action<'a>) -> Result<Status, Refusal<'a>> {
    match *action {
        Action::Kernel { .. } => unreachable!("our own kernel is not started here"),
        Action::Linux(linux) => start_linux(&linux),
        Action::Chainload { path } => {
            let image = read_image("EFI loader", path)?;
            start_image(image, path, None, None)
        }
    }
}

fn start_linux<'a>(linux: &Linux<'a>) -> Result<Status, Refusal<'a>> {
    let image = read_image("Linux image", linux.image)?;
    // Everything the entry declares is checked before anything is started.
    let initrd = match linux.initrd {
        None => None,
        Some(path) => match read_file_at("initrd", path) {
            Ok(bytes) => Some(bytes),
            Err(refusal) => {
                release(image);
                return Err(refusal);
            }
        },
    };
    start_image(image, linux.image, Some(linux.command_line), initrd)
}

/// The device path of `path` on the partition the bootloader started from:
/// the path of that partition's device, then a file node.
fn partition_file_path<'b>(
    path: &str,
    storage: &'b mut [MaybeUninit<u8>],
) -> Result<&'b DevicePath, Refusal<'static>> {
    let mut units = [0u16; PATH_UNITS];
    let name = ucs2(path, &mut units)?;
    let image = boot::open_protocol_exclusive::<LoadedImage>(boot::image_handle())
        .map_err(firmware("open the bootloader's own image"))?;
    let device = image.device().ok_or(Refusal::Firmware {
        step: "find the partition the bootloader started from",
        status: Status::NOT_FOUND,
    })?;
    let device_path = boot::open_protocol_exclusive::<DevicePath>(device)
        .map_err(firmware("read the device path of the partition"))?;
    let too_big = |_| Refusal::Firmware {
        step: "build a device path",
        status: Status::BUFFER_TOO_SMALL,
    };
    let mut builder = DevicePathBuilder::with_buf(storage);
    for node in device_path.node_iter() {
        if node.device_type() == DeviceType::END {
            break;
        }
        builder = builder.push(&node).map_err(too_big)?;
    }
    builder
        .push(&build::media::FilePath { path_name: name })
        .and_then(DevicePathBuilder::finalize)
        .map_err(too_big)
}

/// The initial ramdisk as the Linux stub asks for it: a handle with the
/// vendor media device path and a `LoadFile2` on it.
#[repr(C)]
struct InitrdProtocol {
    // The first field is the protocol interface; the firmware hands the
    // callback a pointer to it, which is a pointer to this whole struct.
    load_file: unsafe extern "efiapi" fn(
        this: *mut InitrdProtocol,
        file_path: *const c_void,
        boot_policy: bool,
        buffer_size: *mut usize,
        buffer: *mut c_void,
    ) -> Status,
    data: *const u8,
    len: usize,
}

unsafe extern "efiapi" fn load_initrd(
    this: *mut InitrdProtocol,
    _file_path: *const c_void,
    boot_policy: bool,
    buffer_size: *mut usize,
    buffer: *mut c_void,
) -> Status {
    // SAFETY: `this` is the pointer installed in `install_initrd`, which stays
    // valid until the protocol is uninstalled; `buffer_size`, when not null,
    // points to a `usize` the caller owns (UEFI specification, LoadFile()).
    let (protocol, size) = unsafe { (&*this, buffer_size.as_mut()) };
    match initrd::reply(
        boot_policy,
        size.as_deref().copied(),
        buffer.is_null(),
        protocol.len,
    ) {
        Reply::Unsupported => Status::UNSUPPORTED,
        Reply::InvalidParameter => Status::INVALID_PARAMETER,
        Reply::BufferTooSmall => {
            if let Some(size) = size {
                *size = protocol.len;
            }
            Status::BUFFER_TOO_SMALL
        }
        Reply::Copy => {
            if let Some(size) = size {
                *size = protocol.len;
            }
            // SAFETY: `reply` said the caller's buffer, not null, holds at
            // least `len` bytes; `data` is the initrd, `len` bytes that live
            // until the protocol is uninstalled; they cannot overlap the
            // caller's freshly allocated buffer.
            unsafe {
                core::ptr::copy_nonoverlapping(protocol.data, buffer.cast::<u8>(), protocol.len)
            };
            Status::SUCCESS
        }
    }
}

/// Installs the initrd on a new handle; the handle is removed when dropped.
struct InstalledInitrd {
    handle: Handle,
    device_path: *const DevicePath,
    protocol: *const InitrdProtocol,
}

impl Drop for InstalledInitrd {
    fn drop(&mut self) {
        // SAFETY: both interfaces were installed on this handle by
        // `install_initrd` with exactly these pointers. A failure to remove
        // them leaves a protocol nobody asks for.
        unsafe {
            let _ = boot::uninstall_protocol_interface(
                self.handle,
                &LoadFile2::GUID,
                self.protocol.cast(),
            );
            let _ = boot::uninstall_protocol_interface(
                self.handle,
                &DevicePath::GUID,
                self.device_path.cast(),
            );
        }
    }
}

fn install_initrd(
    protocol: &InitrdProtocol,
    path_storage: &mut [MaybeUninit<u8>],
) -> Result<InstalledInitrd, Refusal<'static>> {
    let device_path = DevicePathBuilder::with_buf(path_storage)
        .push(&build::media::Vendor {
            vendor_guid: LINUX_EFI_INITRD_MEDIA_GUID,
            vendor_defined_data: &[],
        })
        .and_then(DevicePathBuilder::finalize)
        .map_err(|_| Refusal::Firmware {
            step: "build the initrd device path",
            status: Status::BUFFER_TOO_SMALL,
        })?;
    let device_path: *const DevicePath = device_path;
    // SAFETY: the pointers are to memory that outlives the installation (the
    // caller keeps both buffers until the `InstalledInitrd` is dropped), and
    // `InitrdProtocol` starts with the function `LoadFile2` is defined as.
    unsafe {
        let handle = boot::install_protocol_interface(None, &DevicePath::GUID, device_path.cast())
            .map_err(firmware("install the initrd device path"))?;
        let protocol: *const InitrdProtocol = protocol;
        if let Err(error) =
            boot::install_protocol_interface(Some(handle), &LoadFile2::GUID, protocol.cast())
        {
            let _ =
                boot::uninstall_protocol_interface(handle, &DevicePath::GUID, device_path.cast());
            return Err(firmware("install the initrd protocol")(error));
        }
        Ok(InstalledInitrd {
            handle,
            device_path,
            protocol,
        })
    }
}

/// Loads `image` (already checked) as the file `path` of the partition and
/// starts it, with `command_line` as its load options and `initrd` offered
/// through `LoadFile2`. Takes the image's buffer and gives it back to the
/// firmware, which keeps its own copy.
fn start_image<'a>(
    image: &'static mut [u8],
    path: &str,
    command_line: Option<&str>,
    initrd: Option<&'static mut [u8]>,
) -> Result<Status, Refusal<'a>> {
    let result = run_image(image, path, command_line, initrd.as_deref());
    if let Some(initrd) = initrd {
        release(initrd);
    }
    result
}

fn run_image<'a>(
    image: &'static mut [u8],
    path: &str,
    command_line: Option<&str>,
    initrd: Option<&[u8]>,
) -> Result<Status, Refusal<'a>> {
    let mut path_storage = [MaybeUninit::<u8>::uninit(); DEVICE_PATH_BYTES];
    let loaded = partition_file_path(path, &mut path_storage).and_then(|file_path| {
        boot::load_image(
            boot::image_handle(),
            LoadImageSource::FromBuffer {
                buffer: image,
                file_path: Some(file_path),
            },
        )
        .map_err(firmware("load the image"))
    });
    // The firmware copied the image, loaded or not.
    release(image);
    let child = loaded?;

    // UCS-2, zero terminated, alive until the image returns.
    let mut options = [0u16; MAX_COMMAND_LINE + 1];
    if let Some(command_line) = command_line.filter(|text| !text.is_empty()) {
        let mut length = 0;
        for (slot, character) in options.iter_mut().zip(command_line.chars()) {
            // `bootinfo::config` allows printable ASCII only.
            *slot = u16::from(u8::try_from(character).unwrap_or(b'?'));
            length += 1;
        }
        let mut loaded_image = match boot::open_protocol_exclusive::<LoadedImage>(child) {
            Ok(protocol) => protocol,
            Err(error) => {
                let _ = boot::unload_image(child);
                return Err(firmware("open the image's load options")(error));
            }
        };
        // SAFETY: `options` is `length + 1` UCS-2 units, the last a zero, and
        // lives until this function returns, after the image has.
        unsafe {
            loaded_image.set_load_options(
                options.as_ptr().cast(),
                ((length + 1) * size_of::<u16>()) as u32,
            );
        }
        // The stub opens the protocol itself: do not hold it exclusively.
        drop(loaded_image);
    }

    let mut initrd_path_storage = [MaybeUninit::<u8>::uninit(); 64];
    let protocol = initrd.map(|data| InitrdProtocol {
        load_file: load_initrd,
        data: data.as_ptr(),
        len: data.len(),
    });
    let installed = match &protocol {
        None => None,
        Some(protocol) => match install_initrd(protocol, &mut initrd_path_storage) {
            Ok(installed) => Some(installed),
            Err(refusal) => {
                let _ = boot::unload_image(child);
                return Err(refusal);
            }
        },
    };

    let status = match boot::start_image(child) {
        Ok(()) => Status::SUCCESS,
        Err(error) => error.status(),
    };
    drop(installed);
    let _ = boot::unload_image(child);
    Ok(status)
}
