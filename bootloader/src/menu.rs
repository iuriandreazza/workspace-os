//! The boot menu on the firmware console: reads the keyboard and keeps the
//! time, and leaves every decision to the menu logic in `bootinfo::menu`.
//!
//! The firmware console is the serial line as well as the screen, so the menu
//! is plain text and only ever appends lines; the gate reads it that way, and
//! the graphical menu (K2e) draws beside this one rather than instead of it.

use core::time::Duration;

use bootinfo::menu::{Entry, Key, Menu, Step};
use uefi::boot;
use uefi::proto::console::text::{Key as UefiKey, ScanCode};

use crate::ui;

const POLL: Duration = Duration::from_millis(100);
const POLLS_PER_SECOND: u32 = 10;
/// Keys the firmware may have buffered before the menu was drawn (a key
/// pressed to enter the firmware's own setup, for one) are dropped, so they
/// do not choose an entry nobody saw. A bound keeps a console that never runs
/// dry from holding the boot.
const MAX_STALE_KEYS: usize = 64;
/// The firmware arms a watchdog before it starts a boot application, and a
/// person who stopped the countdown must not be reset while they read.
const WATCHDOG_CODE: u64 = 0x10000;
/// The firmware's own default, put back once the menu is done so that a hang
/// while loading the kernel still resets the machine.
const WATCHDOG_SECONDS: usize = 300;

/// How the entry was chosen, for the log.
#[derive(Clone, Copy)]
pub enum Reason {
    Keyboard,
    Timeout,
}

impl core::fmt::Display for Reason {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Keyboard => "chosen on the keyboard",
            Self::Timeout => "chosen by the timeout",
        })
    }
}

/// Shows the menu until an entry is chosen; returns it.
pub fn choose<'a>(menu: Menu<'a>) -> (Entry<'a>, Reason) {
    // A failed watchdog call is not a reason to stop: the menu works without.
    let _ = boot::set_watchdog_timer(0, WATCHDOG_CODE, None);
    let chosen = wait_for_choice(menu);
    let _ = boot::set_watchdog_timer(WATCHDOG_SECONDS, WATCHDOG_CODE, None);
    chosen
}

fn wait_for_choice<'a>(mut menu: Menu<'a>) -> (Entry<'a>, Reason) {
    let entries = menu.entries();
    for _ in 0..MAX_STALE_KEYS {
        if next_key().is_none() {
            break;
        }
    }
    ui::menu(&menu);

    let mut polls = 0;
    loop {
        if let Some(key) = next_key() {
            match menu.press(key) {
                Step::Boot(index) => return (entries[index], Reason::Keyboard),
                Step::Redraw => ui::menu(&menu),
                Step::Wait => {}
            }
            continue;
        }
        boot::stall(POLL);
        polls += 1;
        if polls == POLLS_PER_SECOND {
            polls = 0;
            match menu.tick() {
                Step::Boot(index) => return (entries[index], Reason::Timeout),
                Step::Redraw => ui::countdown(&menu),
                Step::Wait => {}
            }
        }
    }
}

/// The next key, if there is one. A machine without a keyboard, or a console
/// that fails, is a menu that is never answered: the countdown still starts
/// the default entry.
fn next_key() -> Option<Key> {
    // `with_stdin` panics when the firmware has no input protocol.
    let has_input = uefi::table::system_table_raw()
        // SAFETY: the pointer is the live system table, read once here.
        .is_some_and(|table| !unsafe { table.as_ref() }.stdin.is_null());
    if !has_input {
        return None;
    }
    match uefi::system::with_stdin(|input| input.read_key()) {
        Ok(Some(key)) => Some(translate(key)),
        _ => None,
    }
}

fn translate(key: UefiKey) -> Key {
    match key {
        UefiKey::Special(ScanCode::UP) => Key::Up,
        UefiKey::Special(ScanCode::DOWN) => Key::Down,
        UefiKey::Special(_) => Key::Other,
        UefiKey::Printable(character) => match char::from(character) {
            '\r' | '\n' => Key::Enter,
            other => Key::Char(other),
        },
    }
}
