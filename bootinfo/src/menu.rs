//! The logic of the boot menu, apart from any screen or keyboard so that the
//! host can test it (ADR 0007, ADR 0008).
//!
//! The menu does not know what an entry means: it holds a list and answers
//! with the index of the one chosen. Today the list is [`BUILT_IN`]; a list
//! made by the boot model (ADR 0009) or by the loaders of other kernels takes
//! the same place without changing this module.

use crate::{MODE_NORMAL, MODE_RECOVERY};

/// One line of the menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry<'a> {
    pub label: &'a str,
    /// The `boot_mode` the kernel receives when this entry is chosen
    /// (ADR 0013).
    pub boot_mode: u32,
}

/// The entries every boot offers: the normal start, then recovery (ADR 0007).
pub const BUILT_IN: [Entry<'static>; 2] = [
    Entry {
        label: "Normal",
        boot_mode: MODE_NORMAL,
    },
    Entry {
        label: "Recovery",
        boot_mode: MODE_RECOVERY,
    },
];

/// The entry started when nobody chooses.
pub const DEFAULT_ENTRY: usize = 0;

/// How long the menu waits for a key before it starts the default entry.
pub const AUTOBOOT_SECONDS: u32 = 3;

/// What the person pressed, already reduced to what the menu understands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Enter,
    /// A printable character; the digits `1` to `9` choose an entry by number.
    Char(char),
    /// Any other key. It still stops the countdown.
    Other,
}

/// What the caller does after a key or a tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing changed.
    Wait,
    /// The selection or the countdown changed: draw the menu again.
    Redraw,
    /// Start this entry.
    Boot(usize),
}

#[derive(Debug)]
pub struct Menu<'a> {
    entries: &'a [Entry<'a>],
    selected: usize,
    seconds_left: Option<u32>,
}

impl<'a> Menu<'a> {
    /// A menu over `entries` that starts `default` after `autoboot_seconds`
    /// without a key; `None` waits forever. `None` is returned for an empty
    /// list or a default outside it, which a menu cannot answer.
    pub fn new(
        entries: &'a [Entry<'a>],
        default: usize,
        autoboot_seconds: Option<u32>,
    ) -> Option<Self> {
        (default < entries.len()).then_some(Self {
            entries,
            selected: default,
            seconds_left: autoboot_seconds.filter(|&seconds| seconds > 0),
        })
    }

    pub fn entries(&self) -> &'a [Entry<'a>] {
        self.entries
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Seconds until the default entry starts, `None` once a key stopped it.
    pub fn seconds_left(&self) -> Option<u32> {
        self.seconds_left
    }

    /// A key was pressed. Any key stops the countdown: a person at the
    /// keyboard decides, the timeout is for a machine nobody is watching.
    pub fn press(&mut self, key: Key) -> Step {
        let stopped = self.seconds_left.take().is_some();
        let count = self.entries.len();
        match key {
            Key::Enter => Step::Boot(self.selected),
            Key::Up => {
                self.selected = (self.selected + count - 1) % count;
                Step::Redraw
            }
            Key::Down => {
                self.selected = (self.selected + 1) % count;
                Step::Redraw
            }
            Key::Char(digit @ '1'..='9') => {
                let index = digit as usize - '1' as usize;
                if index < count {
                    self.selected = index;
                    Step::Boot(index)
                } else if stopped {
                    Step::Redraw
                } else {
                    Step::Wait
                }
            }
            Key::Char(_) | Key::Other => {
                if stopped {
                    Step::Redraw
                } else {
                    Step::Wait
                }
            }
        }
    }

    /// One second passed without a key.
    pub fn tick(&mut self) -> Step {
        match self.seconds_left {
            None => Step::Wait,
            Some(1) => {
                self.seconds_left = None;
                Step::Boot(self.selected)
            }
            Some(seconds) => {
                self.seconds_left = Some(seconds - 1);
                Step::Redraw
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boot_mode_name;

    fn built_in() -> Menu<'static> {
        Menu::new(&BUILT_IN, DEFAULT_ENTRY, Some(AUTOBOOT_SECONDS)).unwrap()
    }

    #[test]
    fn built_in_entries_reach_different_boot_modes() {
        assert_eq!(BUILT_IN[DEFAULT_ENTRY].boot_mode, MODE_NORMAL);
        assert_eq!(BUILT_IN[1].boot_mode, MODE_RECOVERY);
        assert_eq!(boot_mode_name(BUILT_IN[0].boot_mode), "normal");
        assert_eq!(boot_mode_name(BUILT_IN[1].boot_mode), "recovery");
        assert_eq!(boot_mode_name(99), "unknown");
    }

    #[test]
    fn timeout_starts_the_default_entry() {
        let mut menu = built_in();
        assert_eq!(menu.seconds_left(), Some(3));
        assert_eq!(menu.tick(), Step::Redraw);
        assert_eq!(menu.tick(), Step::Redraw);
        assert_eq!(menu.seconds_left(), Some(1));
        assert_eq!(menu.tick(), Step::Boot(DEFAULT_ENTRY));
    }

    #[test]
    fn any_key_stops_the_countdown_for_good() {
        for key in [Key::Other, Key::Char('x'), Key::Down, Key::Up] {
            let mut menu = built_in();
            menu.press(key);
            assert_eq!(menu.seconds_left(), None);
            for _ in 0..10 {
                assert_eq!(menu.tick(), Step::Wait);
            }
        }
    }

    #[test]
    fn a_key_that_changes_nothing_else_only_stops_the_countdown() {
        let mut menu = built_in();
        assert_eq!(menu.press(Key::Other), Step::Redraw);
        assert_eq!(menu.press(Key::Other), Step::Wait);
        assert_eq!(menu.selected(), DEFAULT_ENTRY);
    }

    #[test]
    fn arrows_move_the_selection_and_wrap() {
        let mut menu = built_in();
        assert_eq!(menu.press(Key::Down), Step::Redraw);
        assert_eq!(menu.selected(), 1);
        menu.press(Key::Down);
        assert_eq!(menu.selected(), 0);
        menu.press(Key::Up);
        assert_eq!(menu.selected(), 1);
        assert_eq!(menu.press(Key::Enter), Step::Boot(1));
    }

    #[test]
    fn enter_starts_the_selected_entry_without_waiting() {
        assert_eq!(built_in().press(Key::Enter), Step::Boot(DEFAULT_ENTRY));
    }

    #[test]
    fn a_digit_starts_its_entry() {
        assert_eq!(built_in().press(Key::Char('1')), Step::Boot(0));
        assert_eq!(built_in().press(Key::Char('2')), Step::Boot(1));
    }

    #[test]
    fn a_digit_without_an_entry_is_ignored() {
        for digit in ['0', '3', '9'] {
            let mut menu = built_in();
            assert_eq!(menu.press(Key::Char(digit)), Step::Redraw);
            assert_eq!(menu.selected(), DEFAULT_ENTRY);
            assert_eq!(menu.press(Key::Char(digit)), Step::Wait);
        }
    }

    #[test]
    fn without_a_timeout_the_menu_waits_for_a_person() {
        for seconds in [None, Some(0)] {
            let mut menu = Menu::new(&BUILT_IN, DEFAULT_ENTRY, seconds).unwrap();
            assert_eq!(menu.seconds_left(), None);
            assert_eq!(menu.tick(), Step::Wait);
        }
    }

    #[test]
    fn a_menu_needs_an_entry_to_answer_with() {
        assert!(Menu::new(&[], 0, None).is_none());
        assert!(Menu::new(&BUILT_IN, BUILT_IN.len(), None).is_none());
    }

    #[test]
    fn a_list_of_any_length_and_default_works() {
        let entries = [
            Entry {
                label: "a",
                boot_mode: 0,
            },
            Entry {
                label: "b",
                boot_mode: 0,
            },
            Entry {
                label: "c",
                boot_mode: 1,
            },
        ];
        let mut menu = Menu::new(&entries, 2, Some(1)).unwrap();
        assert_eq!(menu.tick(), Step::Boot(2));
        let mut menu = Menu::new(&entries, 0, None).unwrap();
        menu.press(Key::Up);
        assert_eq!(menu.selected(), 2);
        assert_eq!(menu.press(Key::Char('3')), Step::Boot(2));
    }
}
