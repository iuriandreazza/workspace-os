//! How the kernel talks on the console: `  kernel  <tag>  <message>`, with a
//! coloured tag for the kind of event. The message is one contiguous run of
//! text in one colour, so a test can search for it, and the colour is reset
//! before the end of the line, so escape codes never start a line.

use core::fmt::{self, Write};

#[derive(Clone, Copy)]
pub enum Level {
    Ok,
    Info,
    Error,
}

impl Level {
    fn tag(self) -> (&'static str, u8) {
        match self {
            Self::Ok => ("ok  ", 92),
            Self::Info => ("info", 96),
            Self::Error => ("fail", 91),
        }
    }
}

pub fn line(out: &mut impl Write, level: Level, message: fmt::Arguments) {
    // One line is several writes: a tick must not let another thread's line in
    // between them (ADR 0037).
    let _irqs_masked = crate::arch::IrqGuard::new();
    let (tag, color) = level.tag();
    let text = if matches!(level, Level::Error) {
        91
    } else {
        97
    };
    let _ = writeln!(
        out,
        "\x1b[90m  kernel  \x1b[{color}m{tag}  \x1b[{text}m{message}\x1b[0m"
    );
}

/// `klog!(Ok, console, "memory {} MiB", size)`: `writeln!` with a level.
macro_rules! klog {
    ($level:ident, $out:expr, $($arg:tt)*) => {
        $crate::log::line(&mut $out, $crate::log::Level::$level, format_args!($($arg)*))
    };
}
pub(crate) use klog;

/// The box at the start: what is running, and where.
pub fn banner(out: &mut impl Write, el: u64) {
    let rule = "  +----------------------------------------------------------+";
    let _ = writeln!(out, "\x1b[90m{rule}\x1b[0m");
    let _ = writeln!(
        out,
        "\x1b[90m  |\x1b[0m  \x1b[96mworkspace-os\x1b[0m  \x1b[97mkernel {}\x1b[0m   \x1b[90m-  aarch64  -  EL{el}\x1b[0m",
        env!("CARGO_PKG_VERSION")
    );
    let _ = writeln!(out, "\x1b[90m{rule}\x1b[0m");
}

/// Facts for the box at the end of the boot.
pub struct Summary {
    pub entry: &'static str,
    pub memory_mib: u64,
    pub free_frames: usize,
    pub total_frames: usize,
    pub console: u64,
}

pub fn summary(out: &mut impl Write, s: &Summary) {
    let rule = "  +----------------------------------------------------------+";
    let _ = writeln!(out, "\x1b[90m{rule}\x1b[0m");
    let mut row = |label: &str, value: fmt::Arguments| {
        let _ = writeln!(
            out,
            "\x1b[90m  |\x1b[0m \x1b[96m{label:<8}\x1b[97m{value}\x1b[0m"
        );
    };
    row("entry", format_args!("{}", s.entry));
    row("memory", format_args!("{} MiB", s.memory_mib));
    row(
        "frames",
        format_args!("{} free of {}", s.free_frames, s.total_frames),
    );
    row("console", format_args!("PL011 at {:#x}", s.console));
    let _ = writeln!(out, "\x1b[90m{rule}\x1b[0m");
}
