//! A stand-in for the EFI loader of another system, for the boot smoke test
//! (K2d): it says it started and where the bootloader said it was loaded from,
//! then returns, so the gate sees both the chainload and the way back to the
//! menu. It does nothing else.

#![no_std]
#![no_main]

use uefi::prelude::*;
use uefi::proto::device_path::DeviceType;
use uefi::proto::device_path::media::FilePath;
use uefi::proto::loaded_image::LoadedImage;
use uefi::{boot, println};

#[entry]
fn main() -> Status {
    println!();
    println!("  chainprobe: started");
    let Ok(image) = boot::open_protocol_exclusive::<LoadedImage>(boot::image_handle()) else {
        println!("  chainprobe: no loaded image protocol");
        return Status::LOAD_ERROR;
    };
    println!(
        "  chainprobe: device handle {}",
        if image.device().is_some() {
            "set"
        } else {
            "missing"
        }
    );
    if let Some(path) = image.file_path() {
        for node in path.node_iter() {
            if node.device_type() != DeviceType::MEDIA {
                continue;
            }
            if let Ok(file) = <&FilePath>::try_from(node) {
                uefi::print!("  chainprobe: file path ");
                for unit in file.path_name().iter().take_while(|unit| *unit != 0) {
                    uefi::print!("{}", char::from_u32(u32::from(unit)).unwrap_or('?'));
                }
                println!();
            }
        }
    }
    Status::SUCCESS
}
