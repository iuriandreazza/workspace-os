//! What the person at the console sees. Plain text on the firmware console, so
//! it reads the same on a screen and on a serial line.
//!
//! A log line is `  boot  <tag>  <message>`: the component, a coloured tag
//! for the kind of event and the message, which is one contiguous run of text
//! in one colour so that a test can search for it. A colour is always reset
//! before the end of its line, so escape codes never start a line.

use uefi::println;
use uefi::proto::console::text::Color;

fn paint(foreground: Color, text: core::fmt::Arguments) {
    uefi::system::with_stdout(|out| {
        let _ = out.set_color(foreground, Color::Black);
    });
    uefi::print!("{text}");
    uefi::system::with_stdout(|out| {
        let _ = out.set_color(Color::LightGray, Color::Black);
    });
    println!();
}

/// Clears the screen and draws the banner.
pub fn banner() {
    uefi::system::with_stdout(|out| {
        let _ = out.clear();
    });
    paint(
        Color::LightCyan,
        format_args!(r"                      _                                          "),
    );
    paint(
        Color::LightCyan,
        format_args!(r" __      _____  _ __| | _____ _ __   __ _  ___ ___   ___  ___ "),
    );
    paint(
        Color::Cyan,
        format_args!(r" \ \ /\ / / _ \| '__| |/ / __| '_ \ / _` |/ __/ _ \ / _ \/ __|"),
    );
    paint(
        Color::Cyan,
        format_args!(r"  \ V  V / (_) | |  |   <\__ \ |_) | (_| | (_|  __/| (_) \__ \"),
    );
    paint(
        Color::Blue,
        format_args!(r"   \_/\_/ \___/|_|  |_|\_\___/ .__/ \__,_|\___\___| \___/|___/"),
    );
    paint(
        Color::Blue,
        format_args!(r"                             |_|                               "),
    );
    paint(
        Color::DarkGray,
        format_args!(
            "  bootloader {}  -  UEFI  -  aarch64",
            env!("CARGO_PKG_VERSION")
        ),
    );
    println!();
}

pub struct Summary {
    pub kernel_base: u64,
    pub kernel_end: u64,
    pub entry: u64,
    pub usable_mib: u64,
    pub regions: usize,
    pub console: u64,
    pub device_tree_bytes: usize,
    pub exception_level: u64,
}

/// The box shown right before the kernel takes over.
pub fn summary(s: &Summary) {
    let rule = "  +------------------------------------------------------------+";
    paint(Color::DarkGray, format_args!("{rule}"));
    row(
        "kernel",
        format_args!(
            "{:#x}..{:#x}  ({} KiB)",
            s.kernel_base,
            s.kernel_end,
            (s.kernel_end - s.kernel_base) / 1024
        ),
    );
    row(
        "entry",
        format_args!("{:#x}  (EL1, from EL{})", s.entry, s.exception_level),
    );
    row(
        "memory",
        format_args!("{} MiB usable in {} regions", s.usable_mib, s.regions),
    );
    row("console", format_args!("PL011 at {:#x}", s.console));
    row(
        "devices",
        format_args!("device tree, {} bytes", s.device_tree_bytes),
    );
    paint(Color::DarkGray, format_args!("{rule}"));
}

fn row(label: &str, value: core::fmt::Arguments) {
    uefi::system::with_stdout(|out| {
        let _ = out.set_color(Color::DarkGray, Color::Black);
    });
    uefi::print!("  | ");
    uefi::system::with_stdout(|out| {
        let _ = out.set_color(Color::LightCyan, Color::Black);
    });
    uefi::print!("{label:<8}");
    uefi::system::with_stdout(|out| {
        let _ = out.set_color(Color::White, Color::Black);
    });
    uefi::print!("{value}");
    uefi::system::with_stdout(|out| {
        let _ = out.set_color(Color::LightGray, Color::Black);
    });
    println!();
}

/// The failure report, in red, with what happens next.
pub fn failure(reason: core::fmt::Arguments) {
    println!();
    log(Level::Error, format_args!("the boot stopped: {reason}"));
    paint(
        Color::DarkGray,
        format_args!("  boot  returning to the firmware in five seconds"),
    );
}

/// The kind of event a log line reports.
#[derive(Clone, Copy)]
pub enum Level {
    Ok,
    Info,
    Error,
    Go,
}

impl Level {
    fn tag(self) -> (&'static str, Color) {
        match self {
            Self::Ok => ("ok  ", Color::LightGreen),
            Self::Info => ("info", Color::LightCyan),
            Self::Error => ("fail", Color::LightRed),
            Self::Go => (" >> ", Color::LightMagenta),
        }
    }
}

fn set(foreground: Color) {
    uefi::system::with_stdout(|out| {
        let _ = out.set_color(foreground, Color::Black);
    });
}

pub fn log(level: Level, message: core::fmt::Arguments) {
    let (tag, color) = level.tag();
    set(Color::DarkGray);
    uefi::print!("  boot  ");
    set(color);
    uefi::print!("{tag}  ");
    set(if matches!(level, Level::Error) {
        Color::LightRed
    } else {
        Color::White
    });
    uefi::print!("{message}");
    set(Color::LightGray);
    println!();
}

pub fn ok(message: core::fmt::Arguments) {
    log(Level::Ok, message);
}

pub fn info(message: core::fmt::Arguments) {
    log(Level::Info, message);
}

pub fn go(message: core::fmt::Arguments) {
    log(Level::Go, message);
}

pub fn error(message: core::fmt::Arguments) {
    log(Level::Error, message);
}
