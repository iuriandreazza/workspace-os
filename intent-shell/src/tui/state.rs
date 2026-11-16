//! The interface as data: what was said, what is being asked, what is typed.
//!
//! Keys and worker events go in, an `Effect` for the shell comes out, and
//! nothing here touches a terminal, so every rule of the interface (only `y`
//! confirms, Esc refuses, a selection never leaves its list, a root action is
//! never confirmed without being shown whole) is unit tested.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::wrap::{text_width, wrap};
use crate::action::printable;

/// An entry of the history. The text is already safe to print: it comes from
/// `screen` and `Outcome::render`, which escape what the model and the machine
/// wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Request(String),
    Note(String),
    Plan(String),
    Root(String),
    Outcome(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Input,
    Busy {
        label: &'static str,
        since_ms: u64,
    },
    Plan {
        details: String,
    },
    Root {
        panel: String,
    },
    Choose {
        options: Vec<String>,
        selected: usize,
    },
}

/// What the shell's worker tells the interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Busy(&'static str),
    AskPlan {
        screen: String,
        details: String,
    },
    AskRoot {
        panel: String,
    },
    AskChoose {
        question: String,
        options: Vec<String>,
    },
    Outcome(String),
}

/// What the person answered, for the worker's `Confirm`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    Yes,
    No,
    Choice(Option<usize>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Nothing,
    Submit(String),
    Reply(Reply),
    Quit,
}

/// After a question is drawn, answer keys are ignored for this long. A terminal
/// without the kitty keyboard protocol reports a held or double-tapped `y` as
/// fresh presses, and one of those must not answer a question not yet read.
pub const DWELL_MS: u64 = 700;

pub const ROOT_KEYS: &str = "[y] confirm this exact action   [n] refuse";

/// How a line of the history is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Request,
    Note,
    Plan,
    Root,
    Outcome,
}

pub struct State {
    pub header: String,
    pub blocks: Vec<Block>,
    pub input: String,
    pub mode: Mode,
    /// Lines the history is scrolled up from its end.
    pub scroll: usize,
    pub width: u16,
    pub height: u16,
    pub clock_ms: u64,
    /// Set by the first Ctrl-C while a step runs: the next one leaves without
    /// waiting for the step.
    pub forced_quit: bool,
    /// When the current question was put to the person.
    pub asked_ms: u64,
}

/// A character the person may type: one `printable` would leave as it is, so
/// a pasted escape sequence or an invisible character never reaches the shell.
/// A key press meant as an answer: a fresh press (not a repeat or a release)
/// with no Ctrl, Alt or Super held. An escape byte followed by `y`, which a
/// terminal reports as Alt-y, is not a confirmation.
fn deliberate(key: &KeyEvent) -> bool {
    key.kind == KeyEventKind::Press
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
}

fn typeable(c: char) -> bool {
    let text = c.to_string();
    printable(&text, false) == text
}

impl State {
    pub fn new(header: String) -> Self {
        Self {
            header,
            blocks: Vec::new(),
            input: String::new(),
            mode: Mode::Input,
            scroll: 0,
            width: 80,
            height: 24,
            clock_ms: 0,
            asked_ms: 0,
            forced_quit: false,
        }
    }

    pub fn set_size(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
    }

    pub fn is_asking(&self) -> bool {
        matches!(
            self.mode,
            Mode::Plan { .. } | Mode::Root { .. } | Mode::Choose { .. }
        )
    }

    fn busy(&mut self, label: &'static str) {
        self.mode = Mode::Busy {
            label,
            since_ms: self.clock_ms,
        };
    }

    /// Whether the whole exact action fits in the root panel. A root action
    /// is never confirmed from a screen that cuts it off (ADR 0007).
    pub fn root_fits(&self) -> bool {
        match &self.mode {
            Mode::Root { panel } => {
                let columns = usize::from(self.width.saturating_sub(2));
                let lines = wrap(panel, columns);
                // A character wider than the panel is dropped by the screen,
                // so an action containing one would not be shown whole.
                columns >= 1
                    && lines.len() <= self.root_capacity()
                    && lines.iter().all(|line| text_width(line) <= columns)
            }
            _ => false,
        }
    }

    /// Lines for the action: the screen minus the border and the keys, which
    /// wrap too, so that the keys are never cut off either.
    pub fn root_capacity(&self) -> usize {
        let keys = wrap(ROOT_KEYS, self.width.saturating_sub(2).into()).len();
        usize::from(self.height).saturating_sub(2 + keys)
    }

    /// Apply what the worker said. Returns whether the person is now asked
    /// something, so the caller can discard keys typed before it was drawn.
    pub fn on_event(&mut self, event: Event) -> bool {
        self.scroll = 0;
        self.asked_ms = self.clock_ms;
        match event {
            Event::Busy(label) => self.busy(label),
            Event::AskPlan { screen, details } => {
                self.blocks.push(Block::Plan(screen));
                self.mode = Mode::Plan { details };
            }
            Event::AskRoot { panel } => {
                self.blocks.push(Block::Root(panel.clone()));
                self.mode = Mode::Root { panel };
            }
            Event::AskChoose { question, options } => {
                self.blocks.push(Block::Note(question));
                self.mode = Mode::Choose {
                    options,
                    selected: 0,
                };
            }
            Event::Outcome(text) => {
                self.blocks.push(Block::Outcome(text));
                self.mode = Mode::Input;
            }
        }
        self.is_asking()
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Effect {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('c') if control => {
                let running = matches!(
                    self.mode,
                    Mode::Busy {
                        label: "working",
                        ..
                    }
                );
                if running && !self.forced_quit {
                    // Quitting now would stop a step half way and lose its
                    // result; a second Ctrl-C is for a step that never ends.
                    self.forced_quit = true;
                    self.blocks.push(Block::Note(
                        "A step is running. Ctrl-C again to leave without waiting for it."
                            .to_owned(),
                    ));
                    return Effect::Nothing;
                }
                return Effect::Quit;
            }
            KeyCode::Char('d') if control && self.mode == Mode::Input => return Effect::Quit,
            KeyCode::PageUp => return self.scroll_by(self.page(), true),
            KeyCode::PageDown => return self.scroll_by(self.page(), false),
            KeyCode::Home if control => return self.scroll_by(usize::MAX, true),
            KeyCode::End if control => return self.scroll_by(usize::MAX, false),
            _ => {}
        }
        match self.mode.clone() {
            Mode::Input => self.input_key(key),
            Mode::Busy { .. } => Effect::Nothing,
            Mode::Plan { details } => self.plan_key(key, &details),
            Mode::Root { .. } => self.root_key(key),
            Mode::Choose { options, selected } => self.choose_key(key, &options, selected),
        }
    }

    fn input_key(&mut self, key: KeyEvent) -> Effect {
        match key.code {
            KeyCode::Char(c) if typeable(c) && !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.input.push(c);
            }
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Enter => {
                let request = std::mem::take(&mut self.input).trim().to_owned();
                if request.is_empty() {
                    return Effect::Nothing;
                }
                self.scroll = 0;
                self.blocks.push(Block::Request(request.clone()));
                self.busy("asking the model");
                return Effect::Submit(request);
            }
            _ => {}
        }
        Effect::Nothing
    }

    fn answer(&mut self, note: &str, reply: Reply) -> Effect {
        self.blocks.push(Block::Note(note.to_owned()));
        self.busy("working");
        Effect::Reply(reply)
    }

    fn settled(&self) -> bool {
        self.clock_ms.saturating_sub(self.asked_ms) >= DWELL_MS
    }

    fn plan_key(&mut self, key: KeyEvent, details: &str) -> Effect {
        if !deliberate(&key) || !self.settled() {
            return Effect::Nothing;
        }
        match key.code {
            KeyCode::Char('y' | 'Y') => self.answer("▸ confirmed", Reply::Yes),
            KeyCode::Char('n' | 'N') | KeyCode::Esc => self.answer("▸ cancelled", Reply::No),
            KeyCode::Char('d' | 'D') => {
                self.blocks.push(Block::Plan(details.to_owned()));
                self.scroll = 0;
                Effect::Nothing
            }
            _ => Effect::Nothing,
        }
    }

    fn root_key(&mut self, key: KeyEvent) -> Effect {
        if !deliberate(&key) || !self.settled() {
            return Effect::Nothing;
        }
        match key.code {
            KeyCode::Char('y' | 'Y') if self.root_fits() => {
                self.answer("▸ root action confirmed", Reply::Yes)
            }
            KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                self.answer("▸ root action refused", Reply::No)
            }
            _ => Effect::Nothing,
        }
    }

    fn choose_key(&mut self, key: KeyEvent, options: &[String], selected: usize) -> Effect {
        let select = |state: &mut Self, index: usize| {
            state.mode = Mode::Choose {
                options: options.to_vec(),
                selected: index,
            };
            Effect::Nothing
        };
        if !deliberate(&key) || !self.settled() {
            return Effect::Nothing;
        }
        match key.code {
            KeyCode::Up => select(self, selected.saturating_sub(1)),
            KeyCode::Down => select(self, (selected + 1).min(options.len().saturating_sub(1))),
            KeyCode::Enter => match options.get(selected) {
                Some(option) => {
                    let note = format!("▸ chose {option}");
                    self.answer(&note, Reply::Choice(Some(selected)))
                }
                None => Effect::Nothing,
            },
            KeyCode::Esc => self.answer("▸ cancelled", Reply::Choice(None)),
            _ => Effect::Nothing,
        }
    }

    /// Pasted text goes into the request, on one line; elsewhere it is ignored,
    /// so a paste can never answer a question.
    pub fn paste(&mut self, text: &str) {
        if self.mode != Mode::Input {
            return;
        }
        for c in text.chars() {
            if matches!(c, '\n' | '\r') {
                self.input.push(' ');
            } else if typeable(c) {
                self.input.push(c);
            }
        }
    }

    fn page(&self) -> usize {
        usize::from(self.height / 2).max(1)
    }

    fn scroll_by(&mut self, lines: usize, up: bool) -> Effect {
        let last = self
            .history_lines(self.width.into())
            .len()
            .saturating_sub(1);
        self.scroll = if up {
            self.scroll.saturating_add(lines).min(last)
        } else {
            self.scroll.saturating_sub(lines)
        };
        Effect::Nothing
    }

    pub fn wheel(&mut self, up: bool) {
        self.scroll_by(3, up);
    }

    /// The history as wrapped lines, each with how it is drawn.
    pub fn history_lines(&self, width: usize) -> Vec<(Kind, String)> {
        let mut lines = Vec::new();
        for block in &self.blocks {
            let (kind, text) = match block {
                Block::Request(text) => (Kind::Request, format!("you ▸ {text}")),
                Block::Note(text) => (Kind::Note, text.clone()),
                Block::Plan(text) => (Kind::Plan, text.clone()),
                Block::Root(text) => (Kind::Root, text.clone()),
                Block::Outcome(text) => (Kind::Outcome, text.trim_end().to_owned()),
            };
            if !lines.is_empty() {
                lines.push((Kind::Note, String::new()));
            }
            lines.extend(wrap(&text, width).into_iter().map(|line| (kind, line)));
        }
        lines
    }

    /// The part of the history that fits `height` lines, honouring the scroll.
    pub fn visible_history(&self, width: usize, height: usize) -> Vec<(Kind, String)> {
        let lines = self.history_lines(width);
        // Never scrolled past the point where the first line is at the top.
        let up = self.scroll.min(lines.len().saturating_sub(height));
        let end = lines.len() - up;
        let start = end.saturating_sub(height);
        lines[start..end].to_vec()
    }

    /// The history as plain text, for the terminal after the interface closes
    /// and for `--transcript`.
    pub fn transcript(&self) -> String {
        let mut out = String::new();
        for block in &self.blocks {
            match block {
                Block::Request(text) => out.push_str(&format!("intent> {text}\n")),
                Block::Note(text) | Block::Plan(text) | Block::Root(text) => {
                    out.push_str(text);
                    out.push('\n');
                }
                Block::Outcome(text) => {
                    out.push_str(text.trim_end());
                    out.push('\n');
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_text(state: &mut State, text: &str) {
        for c in text.chars() {
            state.on_key(press(KeyCode::Char(c)));
        }
    }

    fn asking_plan() -> State {
        let mut state = State::new("header".to_owned());
        state.on_event(Event::AskPlan {
            screen: "the plan".to_owned(),
            details: "the details".to_owned(),
        });
        state.clock_ms = DWELL_MS;
        state
    }

    fn asking_root(panel: &str) -> State {
        let mut state = State::new("header".to_owned());
        state.on_event(Event::AskRoot {
            panel: panel.to_owned(),
        });
        state.clock_ms = DWELL_MS;
        state
    }

    fn asking_choice(count: usize) -> State {
        let mut state = State::new("header".to_owned());
        state.on_event(Event::AskChoose {
            question: "Which one?".to_owned(),
            options: (1..=count).map(|n| format!("option {n}")).collect(),
        });
        state.clock_ms = DWELL_MS;
        state
    }

    #[test]
    fn a_request_is_submitted_with_enter_and_the_input_clears() {
        let mut state = State::new(String::new());
        type_text(&mut state, "  list downloads ");
        let effect = state.on_key(press(KeyCode::Enter));
        assert_eq!(effect, Effect::Submit("list downloads".to_owned()));
        assert!(state.input.is_empty());
        assert!(matches!(state.mode, Mode::Busy { .. }));
        assert_eq!(state.blocks, [Block::Request("list downloads".to_owned())]);
    }

    #[test]
    fn an_empty_request_is_ignored() {
        let mut state = State::new(String::new());
        type_text(&mut state, "   ");
        assert_eq!(state.on_key(press(KeyCode::Enter)), Effect::Nothing);
        assert_eq!(state.mode, Mode::Input);
        assert!(state.blocks.is_empty());
    }

    #[test]
    fn control_and_invisible_characters_are_not_typed() {
        let mut state = State::new(String::new());
        for c in ['\u{1b}', '\u{9b}', '\u{202e}', '\u{200b}', '\u{7f}', '\t'] {
            state.on_key(press(KeyCode::Char(c)));
        }
        type_text(&mut state, "ok ç");
        assert_eq!(state.input, "ok ç");
    }

    #[test]
    fn typing_while_the_model_works_does_nothing() {
        let mut state = State::new(String::new());
        type_text(&mut state, "x");
        state.on_key(press(KeyCode::Enter));
        type_text(&mut state, "yyyn");
        assert!(state.input.is_empty());
        assert_eq!(state.blocks.len(), 1);
    }

    #[test]
    fn only_y_confirms_a_plan() {
        for key in ["a", "x", "1", " "] {
            let mut state = asking_plan();
            type_text(&mut state, key);
            assert!(state.is_asking(), "{key:?} must not answer");
        }
        let mut state = asking_plan();
        state.on_key(press(KeyCode::Enter));
        assert!(state.is_asking(), "Enter must not confirm");
        let mut state = asking_plan();
        assert_eq!(
            state.on_key(press(KeyCode::Char('y'))),
            Effect::Reply(Reply::Yes)
        );
        let mut state = asking_plan();
        assert_eq!(state.on_key(press(KeyCode::Esc)), Effect::Reply(Reply::No));
        let mut state = asking_plan();
        assert_eq!(
            state.on_key(press(KeyCode::Char('n'))),
            Effect::Reply(Reply::No)
        );
    }

    #[test]
    fn a_modified_or_repeated_key_never_answers() {
        for modifiers in [
            KeyModifiers::CONTROL,
            KeyModifiers::ALT,
            KeyModifiers::SUPER,
            KeyModifiers::ALT | KeyModifiers::SHIFT,
        ] {
            let key = KeyEvent::new(KeyCode::Char('y'), modifiers);
            for mut state in [asking_plan(), asking_root("x")] {
                state.set_size(80, 24);
                assert_eq!(state.on_key(key), Effect::Nothing, "{modifiers:?}");
                assert!(state.is_asking());
            }
        }
        let repeat =
            KeyEvent::new_with_kind(KeyCode::Char('y'), KeyModifiers::NONE, KeyEventKind::Repeat);
        let mut state = asking_plan();
        assert_eq!(state.on_key(repeat), Effect::Nothing);
        let shifted = KeyEvent::new(KeyCode::Char('Y'), KeyModifiers::SHIFT);
        assert_eq!(asking_plan().on_key(shifted), Effect::Reply(Reply::Yes));
    }

    #[test]
    fn a_key_arriving_right_after_a_question_does_not_answer_it() {
        let mut state = State::new(String::new());
        state.set_size(80, 24);
        state.clock_ms = 5_000;
        state.on_event(Event::AskRoot {
            panel: "fs.delete the exact location".to_owned(),
        });
        for after in [0, 100, DWELL_MS - 1] {
            state.clock_ms = 5_000 + after;
            assert_eq!(state.on_key(press(KeyCode::Char('y'))), Effect::Nothing);
            assert!(state.is_asking());
        }
        state.clock_ms = 5_000 + DWELL_MS;
        assert_eq!(
            state.on_key(press(KeyCode::Char('y'))),
            Effect::Reply(Reply::Yes)
        );
    }

    #[test]
    fn a_double_tap_across_a_plan_and_its_root_question_confirms_only_the_plan() {
        let mut state = State::new(String::new());
        state.set_size(80, 24);
        state.on_event(Event::AskPlan {
            screen: "plan".to_owned(),
            details: String::new(),
        });
        state.clock_ms = DWELL_MS;
        assert_eq!(
            state.on_key(press(KeyCode::Char('y'))),
            Effect::Reply(Reply::Yes)
        );
        state.clock_ms += 10;
        state.on_event(Event::AskRoot {
            panel: "exact".to_owned(),
        });
        state.clock_ms += 50;
        assert_eq!(state.on_key(press(KeyCode::Char('y'))), Effect::Nothing);
        assert!(state.is_asking());
    }

    #[test]
    fn a_paste_can_fill_the_request_but_never_answers() {
        let mut state = State::new(String::new());
        state.paste("list\ndownloads\u{1b}y\u{202e}");
        assert_eq!(state.input, "list downloadsy");
        for mut asking in [asking_plan(), asking_root("x"), asking_choice(2)] {
            asking.set_size(80, 24);
            asking.paste("y\n1");
            assert!(asking.is_asking());
            assert!(asking.input.is_empty());
        }
    }

    #[test]
    fn a_root_action_with_a_character_wider_than_the_panel_cannot_be_confirmed() {
        let mut state = asking_root("漢字");
        state.set_size(3, 20);
        assert!(
            !state.root_fits(),
            "one column cannot hold a wide character"
        );
        state.set_size(2, 20);
        assert!(!state.root_fits(), "no column at all");
        state.set_size(6, 20);
        assert!(state.root_fits());
    }

    #[test]
    fn quitting_while_a_step_runs_needs_a_second_ctrl_c() {
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let mut state = asking_plan();
        state.on_key(press(KeyCode::Char('y')));
        assert_eq!(state.on_key(ctrl_c), Effect::Nothing);
        assert!(matches!(state.blocks.last(), Some(Block::Note(n)) if n.contains("running")));
        assert_eq!(state.on_key(ctrl_c), Effect::Quit);
        assert!(state.forced_quit);
        let mut waiting = State::new(String::new());
        type_text(&mut waiting, "x");
        waiting.on_key(press(KeyCode::Enter));
        assert_eq!(
            waiting.on_key(ctrl_c),
            Effect::Quit,
            "waiting for the model can be left"
        );
    }

    #[test]
    fn details_are_shown_without_answering() {
        let mut state = asking_plan();
        assert_eq!(state.on_key(press(KeyCode::Char('d'))), Effect::Nothing);
        assert!(state.is_asking());
        assert_eq!(
            state.blocks.last(),
            Some(&Block::Plan("the details".to_owned()))
        );
    }

    #[test]
    fn a_root_action_is_confirmed_only_with_y_and_only_when_shown_whole() {
        let mut state = asking_root("fs.delete the exact real location");
        state.set_size(80, 24);
        state.on_key(press(KeyCode::Enter));
        assert!(state.is_asking());
        assert_eq!(
            state.on_key(press(KeyCode::Char('y'))),
            Effect::Reply(Reply::Yes)
        );
    }

    #[test]
    fn a_root_action_that_does_not_fit_cannot_be_confirmed() {
        let panel = (1..=30)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut state = asking_root(&panel);
        state.set_size(80, 10);
        assert_eq!(state.on_key(press(KeyCode::Char('y'))), Effect::Nothing);
        assert!(state.is_asking());
        assert_eq!(
            state.on_key(press(KeyCode::Char('n'))),
            Effect::Reply(Reply::No),
            "it can still be refused"
        );
    }

    #[test]
    fn a_narrow_terminal_wraps_the_root_action_instead_of_cutting_it() {
        let mut state = asking_root(&"x".repeat(100));
        // 100 cells in 20 columns is 5 lines; the keys wrap to 3 more at this width.
        state.set_size(22, 9);
        assert!(!state.root_fits());
        state.set_size(22, 10);
        assert!(state.root_fits());
    }

    #[test]
    fn a_selection_stays_inside_its_list() {
        let mut state = asking_choice(3);
        for _ in 0..5 {
            state.on_key(press(KeyCode::Down));
        }
        assert_eq!(
            state.on_key(press(KeyCode::Enter)),
            Effect::Reply(Reply::Choice(Some(2)))
        );
        let mut state = asking_choice(3);
        for _ in 0..5 {
            state.on_key(press(KeyCode::Up));
        }
        assert_eq!(
            state.on_key(press(KeyCode::Enter)),
            Effect::Reply(Reply::Choice(Some(0)))
        );
    }

    #[test]
    fn a_choice_can_be_cancelled() {
        let mut state = asking_choice(2);
        assert_eq!(
            state.on_key(press(KeyCode::Esc)),
            Effect::Reply(Reply::Choice(None))
        );
    }

    #[test]
    fn letters_do_not_answer_a_choice() {
        let mut state = asking_choice(2);
        type_text(&mut state, "y1n");
        assert!(state.is_asking());
    }

    #[test]
    fn control_c_quits_in_every_mode_and_control_d_only_when_idle() {
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        for mut state in [asking_plan(), asking_root("x"), asking_choice(1)] {
            assert_eq!(state.on_key(ctrl('c')), Effect::Quit);
            assert_eq!(state.on_key(ctrl('d')), Effect::Nothing);
        }
        assert_eq!(State::new(String::new()).on_key(ctrl('d')), Effect::Quit);
    }

    #[test]
    fn the_outcome_returns_to_the_input() {
        let mut state = asking_plan();
        state.on_key(press(KeyCode::Char('y')));
        assert!(!state.on_event(Event::Outcome("done\n".to_owned())));
        assert_eq!(state.mode, Mode::Input);
    }

    #[test]
    fn scrolling_stays_inside_the_history_and_new_events_return_to_the_end() {
        let mut state = State::new(String::new());
        state.on_event(Event::Outcome(
            (1..=50).map(|n| format!("l{n}\n")).collect(),
        ));
        assert_eq!(state.visible_history(80, 5).last().unwrap().1, "l50");
        state.on_key(press(KeyCode::PageUp));
        assert!(state.visible_history(80, 5).last().unwrap().1 != "l50");
        state.on_key(KeyEvent::new(KeyCode::Home, KeyModifiers::CONTROL));
        assert_eq!(state.visible_history(80, 5)[0].1, "l1");
        state.on_event(Event::Busy("working"));
        assert_eq!(state.visible_history(80, 5).last().unwrap().1, "l50");
    }

    #[test]
    fn the_transcript_is_plain_text_with_the_requests() {
        let mut state = State::new(String::new());
        type_text(&mut state, "list downloads");
        state.on_key(press(KeyCode::Enter));
        state.on_event(Event::Outcome("done\n".to_owned()));
        assert_eq!(state.transcript(), "intent> list downloads\ndone\n");
    }
}
