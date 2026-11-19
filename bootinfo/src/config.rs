//! The boot entries written by hand in `\WSOS.CFG` on the EFI system
//! partition (ADR 0025). Parsed here, over a borrowed string and a caller
//! supplied array, so that the bootloader needs no allocator and the host can
//! test every refusal. The entries borrow their text from the file.

use core::fmt;

use crate::menu::{Action, Entry, Linux, MAX_ENTRIES};

/// Entries the file may hold: the digit keys, less the built-in two.
pub const MAX_FILE_ENTRIES: usize = MAX_ENTRIES - 2;
/// Linux's own limit on arm64 is 2048; a shorter one leaves room for what the
/// loader may add and keeps the UCS-2 copy on a small buffer.
pub const MAX_COMMAND_LINE: usize = 1023;
/// A label is one line of the menu: longer than this it wraps, and a 64 KiB
/// file could hold a label that takes seconds to print at every redraw.
pub const MAX_LABEL: usize = 48;
/// The longest path the loader will put in a device path node.
pub const MAX_PATH: usize = 255;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathError {
    NotAbsolute,
    TooLong,
    /// A character FAT and UEFI paths do not take, or that is not ASCII.
    BadCharacter(char),
    EmptyComponent,
    DotComponent,
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAbsolute => f.write_str("a path starts with a backslash"),
            Self::TooLong => write!(f, "a path is at most {MAX_PATH} characters"),
            Self::BadCharacter(c) => write!(f, "'{}' cannot be in a path", c.escape_default()),
            Self::EmptyComponent => f.write_str("a path has an empty part between backslashes"),
            Self::DotComponent => f.write_str("a path has no . or .. parts"),
        }
    }
}

/// A path on the EFI system partition, as the entries write it.
pub fn validate_path(path: &str) -> Result<(), PathError> {
    if !path.starts_with('\\') {
        return Err(PathError::NotAbsolute);
    }
    if path.len() > MAX_PATH {
        return Err(PathError::TooLong);
    }
    if let Some(bad) = path
        .chars()
        .find(|c| !c.is_ascii_graphic() && *c != ' ' || "/:*?\"<>|".contains(*c))
    {
        return Err(PathError::BadCharacter(bad));
    }
    for part in path[1..].split('\\') {
        match part {
            "" => return Err(PathError::EmptyComponent),
            "." | ".." => return Err(PathError::DotComponent),
            // FAT drivers drop or keep these differently.
            _ if part.starts_with(' ') || part.ends_with(' ') => {
                return Err(PathError::BadCharacter(' '));
            }
            _ => {}
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    UnknownKey,
    /// A key with nothing after it.
    MissingValue,
    /// A key before the first `entry` line.
    OutsideEntry,
    Duplicate,
    Path(PathError),
    CommandLineTooLong,
    /// Only printable ASCII goes into a command line (it is passed as UCS-2
    /// and read back as ASCII).
    CommandLineCharacter(char),
    /// Only printable ASCII goes into a label: the console cannot print a
    /// character outside the Basic Multilingual Plane (the firmware's text
    /// output fails and the bootloader would panic), and a control character
    /// would rewrite the line.
    LabelCharacter(char),
    LabelTooLong,
    /// The entry has neither `linux` nor `chainload`.
    NoKind,
    /// The entry has both.
    TwoKinds,
    /// `cmdline` or `initrd` without `linux`.
    LinuxKeyWithoutLinux,
    TooManyEntries,
}

/// What is wrong and on which line (counted from 1; for an incomplete entry,
/// the line of its `entry` key).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConfigError {
    pub line: usize,
    pub kind: ErrorKind,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: ", self.line)?;
        match self.kind {
            ErrorKind::UnknownKey => f.write_str("unknown key"),
            ErrorKind::MissingValue => f.write_str("the key has no value"),
            ErrorKind::OutsideEntry => f.write_str("a key before the first entry line"),
            ErrorKind::Duplicate => f.write_str("the key is given twice in the entry"),
            ErrorKind::Path(error) => write!(f, "{error}"),
            ErrorKind::CommandLineTooLong => {
                write!(
                    f,
                    "the command line is at most {MAX_COMMAND_LINE} characters"
                )
            }
            ErrorKind::CommandLineCharacter(c) => {
                write!(
                    f,
                    "'{}' cannot be in a command line (printable ASCII only)",
                    c.escape_default()
                )
            }
            ErrorKind::LabelCharacter(c) => {
                write!(
                    f,
                    "'{}' cannot be in a label (printable ASCII only)",
                    c.escape_default()
                )
            }
            ErrorKind::LabelTooLong => write!(f, "a label is at most {MAX_LABEL} characters"),
            ErrorKind::NoKind => f.write_str("the entry has neither linux nor chainload"),
            ErrorKind::TwoKinds => f.write_str("the entry has both linux and chainload"),
            ErrorKind::LinuxKeyWithoutLinux => {
                f.write_str("cmdline and initrd belong to a linux entry")
            }
            ErrorKind::TooManyEntries => {
                write!(f, "more than {MAX_FILE_ENTRIES} entries")
            }
        }
    }
}

#[derive(Default)]
struct Pending<'a> {
    line: usize,
    label: &'a str,
    linux: Option<&'a str>,
    command_line: Option<&'a str>,
    initrd: Option<&'a str>,
    chainload: Option<&'a str>,
}

impl<'a> Pending<'a> {
    fn finish(self) -> Result<Entry<'a>, ConfigError> {
        let fail = |kind| ConfigError {
            line: self.line,
            kind,
        };
        let action = match (self.linux, self.chainload) {
            (Some(_), Some(_)) => return Err(fail(ErrorKind::TwoKinds)),
            (None, None) => return Err(fail(ErrorKind::NoKind)),
            (Some(image), None) => Action::Linux(Linux {
                image,
                command_line: self.command_line.unwrap_or(""),
                initrd: self.initrd,
            }),
            (None, Some(path)) => {
                if self.command_line.is_some() || self.initrd.is_some() {
                    return Err(fail(ErrorKind::LinuxKeyWithoutLinux));
                }
                Action::Chainload { path }
            }
        };
        Ok(Entry {
            label: self.label,
            action,
        })
    }
}

fn path_value(value: &str, line: usize) -> Result<&str, ConfigError> {
    validate_path(value).map_err(|error| ConfigError {
        line,
        kind: ErrorKind::Path(error),
    })?;
    Ok(value)
}

fn set<'a>(slot: &mut Option<&'a str>, value: &'a str, line: usize) -> Result<(), ConfigError> {
    if slot.replace(value).is_some() {
        return Err(ConfigError {
            line,
            kind: ErrorKind::Duplicate,
        });
    }
    Ok(())
}

/// Reads the entries of `text` into `out`; returns how many. Nothing is
/// written past a refusal that matters: the caller drops `out` on `Err`.
pub fn parse<'a>(text: &'a str, out: &mut [Entry<'a>]) -> Result<usize, ConfigError> {
    let capacity = out.len().min(MAX_FILE_ENTRIES);
    let mut count = 0;
    let mut pending: Option<Pending<'a>> = None;
    let mut push = |pending: Option<Pending<'a>>, count: &mut usize| -> Result<(), ConfigError> {
        if let Some(entry) = pending {
            let line = entry.line;
            if *count == capacity {
                return Err(ConfigError {
                    line,
                    kind: ErrorKind::TooManyEntries,
                });
            }
            out[*count] = entry.finish()?;
            *count += 1;
        }
        Ok(())
    };
    // An editor on another system may put a byte order mark first.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    for (index, raw) in text.split('\n').enumerate() {
        let line = index + 1;
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let (key, value) = match trimmed.split_once(char::is_whitespace) {
            Some((key, value)) => (key, value.trim()),
            None => (trimmed, ""),
        };
        let fail = |kind| ConfigError { line, kind };
        if key == "entry" {
            push(pending.take(), &mut count)?;
            if value.is_empty() {
                return Err(fail(ErrorKind::MissingValue));
            }
            if let Some(bad) = value.chars().find(|c| !(' '..='~').contains(c)) {
                return Err(fail(ErrorKind::LabelCharacter(bad)));
            }
            if value.len() > MAX_LABEL {
                return Err(fail(ErrorKind::LabelTooLong));
            }
            pending = Some(Pending {
                line,
                label: value,
                ..Pending::default()
            });
            continue;
        }
        if !matches!(key, "linux" | "cmdline" | "initrd" | "chainload") {
            return Err(fail(ErrorKind::UnknownKey));
        }
        let entry = pending.as_mut().ok_or(fail(ErrorKind::OutsideEntry))?;
        if value.is_empty() {
            return Err(fail(ErrorKind::MissingValue));
        }
        match key {
            "linux" => set(&mut entry.linux, path_value(value, line)?, line)?,
            "initrd" => set(&mut entry.initrd, path_value(value, line)?, line)?,
            "chainload" => set(&mut entry.chainload, path_value(value, line)?, line)?,
            _ => {
                if value.len() > MAX_COMMAND_LINE {
                    return Err(fail(ErrorKind::CommandLineTooLong));
                }
                if let Some(bad) = value.chars().find(|c| !(' '..='~').contains(c)) {
                    return Err(fail(ErrorKind::CommandLineCharacter(bad)));
                }
                set(&mut entry.command_line, value, line)?;
            }
        }
    }
    push(pending.take(), &mut count)?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILLER: Entry<'static> = Entry::kernel("", 0);

    fn parsed(text: &str) -> Result<([Entry<'_>; MAX_FILE_ENTRIES], usize), ConfigError> {
        let mut out = [FILLER; MAX_FILE_ENTRIES];
        let count = parse(text, &mut out)?;
        Ok((out, count))
    }

    fn error(text: &str) -> ConfigError {
        parsed(text).unwrap_err()
    }

    #[test]
    fn a_linux_entry_and_a_chainload_entry() {
        let text = "# two systems\r\nentry Alpine Linux\r\n  linux \\VMLINUZ\r\n  cmdline console=ttyAMA0 root=/dev/vda\r\n  initrd \\INITRD.IMG\r\n\r\nentry Other\r\nchainload \\EFI\\OTHER\\BOOTAA64.EFI";
        let (entries, count) = parsed(text).unwrap();
        assert_eq!(count, 2);
        assert_eq!(
            entries[0],
            Entry {
                label: "Alpine Linux",
                action: Action::Linux(Linux {
                    image: "\\VMLINUZ",
                    command_line: "console=ttyAMA0 root=/dev/vda",
                    initrd: Some("\\INITRD.IMG"),
                }),
            }
        );
        assert_eq!(
            entries[1],
            Entry {
                label: "Other",
                action: Action::Chainload {
                    path: "\\EFI\\OTHER\\BOOTAA64.EFI"
                },
            }
        );
    }

    #[test]
    fn a_byte_order_mark_and_a_control_character() {
        assert_eq!(parsed("\u{feff}entry A\nlinux \\X").unwrap().1, 1);
        for bad in ['\u{1f427}', '\u{e9}', '\u{65e5}'] {
            let text = format!("entry Linux {bad}\nlinux \\X");
            assert_eq!(error(&text).kind, ErrorKind::LabelCharacter(bad));
        }
        assert_eq!(
            error("entry A\u{7}B\nlinux \\X"),
            ConfigError {
                line: 1,
                kind: ErrorKind::LabelCharacter('\u{7}')
            }
        );
        // A carriage return ends a line; it is not part of the label.
        assert_eq!(parsed("entry A\r\nlinux \\X\r\n").unwrap().0[0].label, "A");
    }

    #[test]
    fn a_label_has_a_length_limit() {
        let exact = format!("entry {}\nlinux \\X", "L".repeat(MAX_LABEL));
        assert_eq!(parsed(&exact).unwrap().1, 1);
        let long = format!("entry {}\nlinux \\X", "L".repeat(MAX_LABEL + 1));
        assert_eq!(error(&long).kind, ErrorKind::LabelTooLong);
    }

    #[test]
    fn linux_needs_only_an_image() {
        let (entries, _) = parsed("entry L\nlinux \\IMG").unwrap();
        assert_eq!(
            entries[0].action,
            Action::Linux(Linux {
                image: "\\IMG",
                command_line: "",
                initrd: None
            })
        );
    }

    #[test]
    fn an_empty_file_has_no_entries() {
        assert_eq!(parsed("").unwrap().1, 0);
        assert_eq!(parsed("# nothing\n\n   \n").unwrap().1, 0);
    }

    #[test]
    fn errors_name_their_line() {
        let cases = [
            ("entry A\nfrobnicate x", 2, ErrorKind::UnknownKey),
            ("linux \\X", 1, ErrorKind::OutsideEntry),
            ("entry A\nlinux", 2, ErrorKind::MissingValue),
            ("entry", 1, ErrorKind::MissingValue),
            ("entry A\nlinux \\X\nlinux \\Y", 3, ErrorKind::Duplicate),
            ("\n\nentry A", 3, ErrorKind::NoKind),
            ("entry A\nlinux \\X\nchainload \\Y", 1, ErrorKind::TwoKinds),
            (
                "entry A\nchainload \\X\ninitrd \\Y",
                1,
                ErrorKind::LinuxKeyWithoutLinux,
            ),
            (
                "entry A\nchainload \\X\ncmdline a",
                1,
                ErrorKind::LinuxKeyWithoutLinux,
            ),
            (
                "entry A\nlinux X",
                2,
                ErrorKind::Path(PathError::NotAbsolute),
            ),
            (
                "entry A\nlinux \\A\\\\B",
                2,
                ErrorKind::Path(PathError::EmptyComponent),
            ),
            (
                "entry A\nlinux \\A\\",
                2,
                ErrorKind::Path(PathError::EmptyComponent),
            ),
            (
                "entry A\nlinux \\A\\..\\B",
                2,
                ErrorKind::Path(PathError::DotComponent),
            ),
            (
                "entry A\nlinux \\A/B",
                2,
                ErrorKind::Path(PathError::BadCharacter('/')),
            ),
            (
                "entry A\nlinux \\caf\u{e9}",
                2,
                ErrorKind::Path(PathError::BadCharacter('\u{e9}')),
            ),
            (
                "entry A\nlinux \\X\ncmdline caf\u{e9}",
                3,
                ErrorKind::CommandLineCharacter('\u{e9}'),
            ),
        ];
        for (text, line, kind) in cases {
            assert_eq!(error(text), ConfigError { line, kind }, "{text:?}");
        }
    }

    #[test]
    fn a_path_and_a_command_line_have_a_length_limit() {
        let long_path = format!("entry A\nlinux \\{}", "A".repeat(MAX_PATH));
        assert_eq!(error(&long_path).kind, ErrorKind::Path(PathError::TooLong));
        let exact_path = format!("entry A\nlinux \\{}", "A".repeat(MAX_PATH - 1));
        assert_eq!(parsed(&exact_path).unwrap().1, 1);
        let long = format!(
            "entry A\nlinux \\X\ncmdline {}",
            "a".repeat(MAX_COMMAND_LINE + 1)
        );
        assert_eq!(error(&long).kind, ErrorKind::CommandLineTooLong);
        let exact = format!(
            "entry A\nlinux \\X\ncmdline {}",
            "a".repeat(MAX_COMMAND_LINE)
        );
        assert_eq!(parsed(&exact).unwrap().1, 1);
    }

    #[test]
    fn the_digit_keys_limit_the_entries() {
        let mut text = std::string::String::new();
        for i in 0..MAX_FILE_ENTRIES {
            text += &format!("entry E{i}\nchainload \\E{i}\n");
        }
        assert_eq!(parsed(&text).unwrap().1, MAX_FILE_ENTRIES);
        text += "entry One more\nchainload \\X\n";
        assert_eq!(
            error(&text),
            ConfigError {
                line: MAX_FILE_ENTRIES * 2 + 1,
                kind: ErrorKind::TooManyEntries
            }
        );
    }

    #[test]
    fn a_bigger_output_array_is_not_a_bigger_limit() {
        // Seven entries is what the digit keys reach, whatever room there is.
        let mut text = std::string::String::new();
        for i in 0..=MAX_FILE_ENTRIES {
            text += &format!("entry E{i}\nchainload \\E{i}\n");
        }
        let mut out = [FILLER; 20];
        assert_eq!(
            parse(&text, &mut out).unwrap_err().kind,
            ErrorKind::TooManyEntries
        );
    }

    #[test]
    fn a_smaller_output_array_is_a_smaller_limit() {
        let mut out = [FILLER; 1];
        let text = "entry A\nchainload \\A\nentry B\nchainload \\B";
        assert_eq!(
            parse(text, &mut out).unwrap_err().kind,
            ErrorKind::TooManyEntries
        );
    }

    #[test]
    fn a_message_never_holds_a_character_the_console_cannot_print() {
        // The message is printed on the firmware console, which fails (and
        // the bootloader panics) on a character above U+FFFF.
        for text in [
            "entry A\u{1f427}",
            "entry A\nlinux \\\u{1f427}",
            "entry A\nlinux \\X\ncmdline \u{1f427}",
        ] {
            let message = format!("{}", error(text));
            assert!(message.is_ascii(), "{message}");
            assert!(message.contains("\\u{1f427}"), "{message}");
        }
    }

    #[test]
    fn a_path_is_valid_or_not() {
        for ok in ["\\A", "\\EFI\\BOOT\\BOOTAA64.EFI", "\\a b"] {
            assert_eq!(validate_path(ok), Ok(()), "{ok}");
        }
        assert_eq!(validate_path(""), Err(PathError::NotAbsolute));
        assert_eq!(validate_path("\\"), Err(PathError::EmptyComponent));
        assert_eq!(validate_path("\\a*"), Err(PathError::BadCharacter('*')));
        assert_eq!(validate_path("\\A \\B"), Err(PathError::BadCharacter(' ')));
        assert_eq!(validate_path("\\ A"), Err(PathError::BadCharacter(' ')));
        assert_eq!(validate_path("\\a\tb"), Err(PathError::BadCharacter('\t')));
    }
}
