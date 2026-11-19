//! The check the loader makes on an image before it hands it to the firmware
//! (ADR 0009, point 4): is this a PE image for aarch64 that the firmware can
//! start as an EFI application? Linux on arm64 and the loaders of other
//! systems are both that. Plain reads over a byte slice, so the host tests it.
//!
//! Layout: Microsoft PE format, "PE32+" optional header; the machine and
//! subsystem values are those of the UEFI specification, section 2.1.1
//! (image types) and the PE specification.

use core::fmt;

pub const MACHINE_ARM64: u16 = 0xaa64;
const MACHINE_X86_64: u16 = 0x8664;
const MACHINE_IA32: u16 = 0x014c;
const MACHINE_ARM: u16 = 0x01c4;
const MACHINE_RISCV64: u16 = 0x5064;
/// `IMAGE_SUBSYSTEM_EFI_APPLICATION`.
pub const SUBSYSTEM_EFI_APPLICATION: u16 = 10;

const PE32_PLUS_MAGIC: u16 = 0x20b;
const DOS_POINTER: usize = 0x3c;
const COFF_SIZE: usize = 20;
const SECTION_SIZE: usize = 40;
/// The optional header must reach the Subsystem field (offset 68, 2 bytes).
const MIN_OPTIONAL_SIZE: usize = 70;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeError {
    /// Too short to hold the headers it names.
    Truncated,
    NoDosHeader,
    NoPeSignature,
    WrongMachine(u16),
    NotPe32Plus,
    WrongSubsystem(u16),
    NoEntryPoint,
    /// A section's file data lies past the end of the file: a cut download.
    SectionBeyondFile,
}

fn machine_name(machine: u16) -> &'static str {
    match machine {
        MACHINE_X86_64 => "x86_64",
        MACHINE_IA32 => "x86",
        MACHINE_ARM => "32-bit arm",
        MACHINE_RISCV64 => "riscv64",
        _ => "an unknown machine",
    }
}

impl fmt::Display for PeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => f.write_str("the file ends inside its PE headers"),
            Self::NoDosHeader => f.write_str("not a PE image (no MZ header)"),
            Self::NoPeSignature => f.write_str("not a PE image (no PE signature)"),
            Self::WrongMachine(machine) => write!(
                f,
                "a PE image for {} (machine {machine:#06x}), not aarch64",
                machine_name(*machine)
            ),
            Self::NotPe32Plus => f.write_str("not a 64-bit PE image"),
            Self::WrongSubsystem(subsystem) => write!(
                f,
                "a PE image of subsystem {subsystem}, not an EFI application (10)"
            ),
            Self::NoEntryPoint => f.write_str("the PE image has no entry point inside the image"),
            Self::SectionBeyondFile => f.write_str("a section of the PE image is cut off"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeInfo {
    pub entry_point: u32,
    pub image_size: u32,
}

fn u16_at(file: &[u8], at: usize) -> Result<u16, PeError> {
    let bytes = file.get(at..at + 2).ok_or(PeError::Truncated)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn u32_at(file: &[u8], at: usize) -> Result<u32, PeError> {
    let bytes = file.get(at..at + 4).ok_or(PeError::Truncated)?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// Checks that `file` is a 64-bit PE image for aarch64 whose subsystem is an
/// EFI application, with an entry point inside the image and every section's
/// file data inside the file.
pub fn check_efi_application(file: &[u8]) -> Result<PeInfo, PeError> {
    if file.get(..2) != Some(b"MZ") {
        return Err(PeError::NoDosHeader);
    }
    let pe = u32_at(file, DOS_POINTER)? as usize;
    if file.get(pe..pe.saturating_add(4)) != Some(b"PE\0\0") {
        // A pointer outside the file is as much "no signature" as a wrong one,
        // except when the file is too short to even hold the pointer.
        return Err(PeError::NoPeSignature);
    }
    let coff = pe + 4;
    let machine = u16_at(file, coff)?;
    if machine != MACHINE_ARM64 {
        return Err(PeError::WrongMachine(machine));
    }
    let section_count = usize::from(u16_at(file, coff + 2)?);
    let optional_size = usize::from(u16_at(file, coff + 16)?);
    let optional = coff + COFF_SIZE;
    if optional_size < MIN_OPTIONAL_SIZE {
        return Err(PeError::Truncated);
    }
    if u16_at(file, optional)? != PE32_PLUS_MAGIC {
        return Err(PeError::NotPe32Plus);
    }
    let entry_point = u32_at(file, optional + 16)?;
    let image_size = u32_at(file, optional + 56)?;
    let subsystem = u16_at(file, optional + 68)?;
    if subsystem != SUBSYSTEM_EFI_APPLICATION {
        return Err(PeError::WrongSubsystem(subsystem));
    }
    if entry_point == 0 || entry_point >= image_size {
        return Err(PeError::NoEntryPoint);
    }
    let sections = optional + optional_size;
    for index in 0..section_count {
        let section = sections + index * SECTION_SIZE;
        if file.get(section..section + SECTION_SIZE).is_none() {
            return Err(PeError::Truncated);
        }
        let raw_size = u32_at(file, section + 16)? as usize;
        let raw_pointer = u32_at(file, section + 20)? as usize;
        if raw_size > 0 && raw_pointer.saturating_add(raw_size) > file.len() {
            return Err(PeError::SectionBeyondFile);
        }
    }
    Ok(PeInfo {
        entry_point,
        image_size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PE image with one section, written field by field so that each test
    /// can break one thing. No binary is committed.
    fn image() -> [u8; 0x400] {
        let mut file = [0u8; 0x400];
        file[..2].copy_from_slice(b"MZ");
        file[DOS_POINTER..DOS_POINTER + 4].copy_from_slice(&0x80u32.to_le_bytes());
        file[0x80..0x84].copy_from_slice(b"PE\0\0");
        let coff = 0x84;
        file[coff..coff + 2].copy_from_slice(&MACHINE_ARM64.to_le_bytes());
        file[coff + 2..coff + 4].copy_from_slice(&1u16.to_le_bytes());
        file[coff + 16..coff + 18].copy_from_slice(&112u16.to_le_bytes());
        let optional = coff + COFF_SIZE;
        file[optional..optional + 2].copy_from_slice(&PE32_PLUS_MAGIC.to_le_bytes());
        file[optional + 16..optional + 20].copy_from_slice(&0x1000u32.to_le_bytes());
        file[optional + 56..optional + 60].copy_from_slice(&0x3000u32.to_le_bytes());
        file[optional + 68..optional + 70]
            .copy_from_slice(&SUBSYSTEM_EFI_APPLICATION.to_le_bytes());
        let section = optional + 112;
        file[section + 16..section + 20].copy_from_slice(&0x100u32.to_le_bytes());
        file[section + 20..section + 24].copy_from_slice(&0x200u32.to_le_bytes());
        file
    }

    const COFF: usize = 0x84;
    const OPTIONAL: usize = COFF + COFF_SIZE;
    const SECTION: usize = OPTIONAL + 112;

    fn patched(at: usize, bytes: &[u8]) -> [u8; 0x400] {
        let mut file = image();
        file[at..at + bytes.len()].copy_from_slice(bytes);
        file
    }

    #[test]
    fn an_aarch64_efi_application_passes() {
        assert_eq!(
            check_efi_application(&image()),
            Ok(PeInfo {
                entry_point: 0x1000,
                image_size: 0x3000
            })
        );
    }

    #[test]
    fn each_part_of_the_header_is_checked() {
        let x86 = patched(COFF, &MACHINE_X86_64.to_le_bytes());
        let cases: [(&[u8], PeError); 10] = [
            (&[], PeError::NoDosHeader),
            (b"\x7fELF", PeError::NoDosHeader),
            (b"MZ", PeError::Truncated),
            (&patched(0x80, b"PX")[..], PeError::NoPeSignature),
            (
                &patched(DOS_POINTER, &0xfff0u32.to_le_bytes())[..],
                PeError::NoPeSignature,
            ),
            (&x86[..], PeError::WrongMachine(MACHINE_X86_64)),
            (
                &patched(OPTIONAL, &0x10bu16.to_le_bytes())[..],
                PeError::NotPe32Plus,
            ),
            (
                &patched(OPTIONAL + 68, &11u16.to_le_bytes())[..],
                PeError::WrongSubsystem(11),
            ),
            (
                &patched(OPTIONAL + 16, &0u32.to_le_bytes())[..],
                PeError::NoEntryPoint,
            ),
            (
                &patched(OPTIONAL + 16, &0x3000u32.to_le_bytes())[..],
                PeError::NoEntryPoint,
            ),
        ];
        for (file, error) in cases {
            assert_eq!(check_efi_application(file), Err(error));
        }
    }

    #[test]
    fn a_cut_file_is_refused_wherever_it_is_cut() {
        let file = image();
        // Inside the optional header and inside the section table.
        for end in [0x40, 0x83, COFF + 4, OPTIONAL + 30, SECTION + 10] {
            assert!(
                check_efi_application(&file[..end]).is_err(),
                "cut at {end:#x}"
            );
        }
        // Section data (0x200..0x300) past the end of the file.
        assert_eq!(
            check_efi_application(&file[..0x2ff]),
            Err(PeError::SectionBeyondFile)
        );
        assert!(check_efi_application(&file[..0x300]).is_ok());
    }

    #[test]
    fn an_optional_header_too_small_to_hold_the_subsystem_is_refused() {
        let file = patched(COFF + 16, &60u16.to_le_bytes());
        assert_eq!(check_efi_application(&file), Err(PeError::Truncated));
    }

    #[test]
    fn a_section_pointer_that_would_overflow_is_refused() {
        let file = patched(SECTION + 20, &u32::MAX.to_le_bytes());
        assert_eq!(
            check_efi_application(&file),
            Err(PeError::SectionBeyondFile)
        );
    }

    #[test]
    fn the_messages_name_what_is_wrong() {
        let text = std::format!("{}", PeError::WrongMachine(MACHINE_X86_64));
        assert!(
            text.contains("x86_64") && text.contains("not aarch64"),
            "{text}"
        );
        assert!(std::format!("{}", PeError::NoDosHeader).contains("not a PE image"));
    }
}
