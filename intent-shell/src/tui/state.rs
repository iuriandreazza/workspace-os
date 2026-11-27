//! The interface as data: what was said, what is being asked, what is typed.
//!
//! Keys and worker events go in, an `Effect` for the shell comes out, and
//! nothing here touches a terminal, so every rule of the interface (only `y`
//! confirms, Esc refuses, a selection never leaves its list, a root action is
//! never confirmed without being shown whole) is unit tested.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::wrap::{text_width, wrap};
use crate::action::printable;
use crate::ports::ChoiceLine;
use crate::reply;
use crate::session::Entry;

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
    /// What the model wrote after the result, as the person is to read it:
    /// already cleaned and not yet wrapped, since the marks are put on at
    /// each line of the screen it ends up on.
    Reply(String),
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
        lines: Vec<ChoiceLine>,
        /// The candidate selected, counted among the candidates only.
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
        lines: Vec<ChoiceLine>,
    },
    Outcome(String),
    /// A result after which the model is writing a reply: the person may not
    /// type until it is `Said`, so no request can start in between.
    OutcomeAwaitingReply(String),
    /// The reply was written, or there is none: the person may type again.
    Said(Option<String>),
    /// The header with the directory the session is in now.
    Header(String),
    /// Something the person should know that is not a result: a speech
    /// problem. It changes no mode and answers nothing.
    Note(String),
    /// What the recogniser heard, or why nothing was. Only a wait for it takes
    /// it: in any other mode it is dropped, so speech can never answer a question.
    Heard(Result<String, String>),
}

/// What the person answered, for the worker's `Confirm`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    Yes,
    No,
    Choice(Option<usize>),
}

/// What the interface asks the microphone to do (push-to-talk).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listen {
    Start,
    /// Stop recording and transcribe.
    Stop,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Nothing,
    Submit(String),
    Reply(Reply),
    Listen(Listen),
    Quit,
}

/// After a question is drawn, answer keys are ignored for this long. A terminal
/// without the kitty keyboard protocol reports a held or double-tapped `y` as
/// fresh presses, and one of those must not answer a question not yet read.
pub const DWELL_MS: u64 = 700;

/// For a root action: longer than the slowest key-repeat delay of common
/// systems (about 1.8 s on macOS), so a still-held `y` cannot confirm it.
pub const ROOT_DWELL_MS: u64 = 2_000;

pub const ROOT_KEYS: &str = "[y] confirm this exact action   [n] refuse";

/// How a line of the history is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Request,
    Note,
    Plan,
    Root,
    Outcome,
    /// The model's words, under the result.
    Reply,
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
    /// When the warning about a running step was shown, or a Ctrl-C last arrived after it.
    pub quit_warned_ms: u64,
    /// When the current question was put to the person.
    pub asked_ms: u64,
    /// What the shell is doing after a line is sent. With no model it never
    /// waits for one, and leaving then waits for the step like any other.
    submit_label: &'static str,
    /// What the transcript writes before a line: the line adapter's prompt.
    prompt: &'static str,
    /// Whether `--listen` was given: only then is there a microphone key.
    listening: bool,
    /// Until when Enter and the microphone key are ignored: a key still held
    /// from stopping the recording must not send what was just heard, or start
    /// the next recording.
    quiet_until_ms: u64,
}

/// A key press meant as an answer: a fresh press (not a repeat or a release)
/// with no Ctrl, Alt or Super held. An escape byte followed by `y`, which a
/// terminal reports as Alt-y, is not a confirmation.
fn deliberate(key: &KeyEvent) -> bool {
    key.kind == KeyEventKind::Press
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
}

/// A character the person may type: one `printable` would leave as it is, so
/// a pasted escape sequence or an invisible character never reaches the shell.
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
            quit_warned_ms: 0,
            submit_label: "asking the model",
            prompt: "intent> ",
            listening: false,
            quiet_until_ms: 0,
        }
    }

    /// Turn on push-to-talk (`--listen`): Ctrl-T records, Ctrl-T or Enter
    /// stops, and what was heard waits in the input line for Enter.
    pub fn with_listening(mut self) -> Self {
        self.listening = true;
        self
    }

    /// The hint under the input line, with the microphone key only when there is one.
    pub fn input_hint(&self) -> &'static str {
        if self.listening {
            "Enter send · ^T talk · PgUp/Dn scroll · ^D quit"
        } else {
            "Enter send · PgUp/Dn scroll · ^D quit"
        }
    }

    /// A state for how the session reads lines: with no model a line is a
    /// command and nothing waits for one.
    pub fn for_entry(header: String, entry: Entry) -> Self {
        let mut state = Self::new(header);
        if entry == Entry::Command {
            state.submit_label = "working";
            state.prompt = "command> ";
        }
        state
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
        // A reply that arrives while the person reads back through the history
        // must not throw them to the bottom. A speech note or a transcript is not the shell's progress: it must
        // not throw the reader to the bottom, restart the quiet gap of a
        // question or cancel the warning of a running step.
        let aside = matches!(event, Event::Note(_) | Event::Heard(_));
        if !matches!(event, Event::Said(_)) && !aside {
            self.scroll = 0;
        }
        if !aside {
            self.asked_ms = self.clock_ms;
        }
        if !matches!(event, Event::Busy(_)) && !aside {
            self.forced_quit = false;
        }
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
            Event::AskChoose { question, lines } => {
                self.blocks.push(Block::Note(question));
                self.mode = Mode::Choose { lines, selected: 0 };
            }
            Event::Header(text) => self.header = text,
            Event::Note(text) => self.blocks.push(Block::Note(text)),
            Event::Heard(heard) => self.heard(heard),
            Event::Outcome(text) => {
                self.blocks.push(Block::Outcome(text));
                self.mode = Mode::Input;
            }
            Event::OutcomeAwaitingReply(text) => {
                self.blocks.push(Block::Outcome(text));
                self.busy("saying");
            }
            Event::Said(text) => {
                let width = usize::from(self.width);
                let before = self.history_lines(width).len();
                self.blocks.extend(text.map(Block::Reply));
                // The scroll counts lines up from the end: what the reply adds
                // must be added to it, or the text being read moves.
                if self.scroll > 0 {
                    self.scroll += self.history_lines(width).len() - before;
                }
                if matches!(
                    self.mode,
                    Mode::Busy {
                        label: "saying",
                        ..
                    }
                ) {
                    self.mode = Mode::Input;
                }
            }
        }
        // What is aside draws no question, so it never discards the keys typed for one.
        !aside && self.is_asking()
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
                    self.quit_warned_ms = self.clock_ms;
                    self.blocks.push(Block::Note(
                        "A step is running. Ctrl-C again to leave without waiting for it."
                            .to_owned(),
                    ));
                    return Effect::Nothing;
                }
                if running {
                    // A held key repeats: only a fresh press after a quiet gap
                    // is the second Ctrl-C.
                    let quiet = self.clock_ms.saturating_sub(self.quit_warned_ms) >= DWELL_MS;
                    self.quit_warned_ms = self.clock_ms;
                    if !(quiet && key.kind == KeyEventKind::Press) {
                        return Effect::Nothing;
                    }
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
        if let Some(effect) = self.listen_key(&key) {
            return effect;
        }
        match self.mode.clone() {
            Mode::Input => self.input_key(key),
            Mode::Busy { .. } => Effect::Nothing,
            Mode::Plan { details } => self.plan_key(key, &details),
            Mode::Root { .. } => self.root_key(key),
            Mode::Choose { lines, selected } => self.choose_key(key, &lines, selected),
        }
    }

    /// The microphone keys. `None` leaves the key to the mode it is pressed in.
    fn listen_key(&mut self, key: &KeyEvent) -> Option<Effect> {
        if !self.listening || key.kind != KeyEventKind::Press {
            return None;
        }
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let talk = key.code == KeyCode::Char('t') && control;
        match self.mode {
            Mode::Input if talk => {
                if self.clock_ms < self.quiet_until_ms {
                    return Some(Effect::Nothing);
                }
                self.busy("listening");
                Some(Effect::Listen(Listen::Start))
            }
            Mode::Busy {
                label: "listening",
                since_ms,
            } => {
                if key.code == KeyCode::Esc {
                    self.mode = Mode::Input;
                    return Some(Effect::Listen(Listen::Cancel));
                }
                if !(talk || key.code == KeyCode::Enter) {
                    return None;
                }
                // A held key repeats: stopping needs a gap after the start.
                if self.clock_ms.saturating_sub(since_ms) < DWELL_MS {
                    return Some(Effect::Nothing);
                }
                self.busy("transcribing");
                Some(Effect::Listen(Listen::Stop))
            }
            _ => None,
        }
    }

    /// What the recogniser said goes into the input line, to be read, edited
    /// and sent with Enter; it is never sent by itself.
    fn heard(&mut self, heard: Result<String, String>) {
        let waiting = matches!(
            self.mode,
            Mode::Busy {
                label: "listening" | "transcribing",
                ..
            }
        );
        if !waiting {
            return;
        }
        self.mode = Mode::Input;
        self.scroll = 0;
        self.quiet_until_ms = self.clock_ms + DWELL_MS;
        match heard {
            Ok(text) => {
                if !self.input.is_empty() {
                    self.input.push(' ');
                }
                self.input.push_str(&text);
                self.blocks.push(Block::Note(
                    "▸ heard: read it, edit it if it is wrong, then press Enter to send".to_owned(),
                ));
            }
            Err(why) => self.blocks.push(Block::Note(format!("▸ {why}"))),
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
                if self.clock_ms < self.quiet_until_ms {
                    return Effect::Nothing;
                }
                let request = std::mem::take(&mut self.input).trim().to_owned();
                if request.is_empty() {
                    return Effect::Nothing;
                }
                self.scroll = 0;
                self.forced_quit = false;
                self.blocks.push(Block::Request(request.clone()));
                self.busy(self.submit_label);
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

    /// Whether this key may be taken as an answer. Only a confirmation needs
    /// the quiet gap: refusing, cancelling, moving a selection and asking for
    /// details are safe whatever their timing.
    fn may_answer(&mut self, key: &KeyEvent) -> bool {
        if !deliberate(key) {
            return false;
        }
        // Enter confirms only a selection; in a plan or a root panel it does nothing.
        let answers = matches!(key.code, KeyCode::Char('y' | 'Y'))
            || (key.code == KeyCode::Enter && matches!(self.mode, Mode::Choose { .. }));
        !answers || self.settled()
    }

    /// Whether the question has had a quiet gap since it was drawn or since
    /// the last key. A held key keeps restarting the gap, so its auto-repeat
    /// (reported as fresh presses) cannot answer once the window passes.
    fn settled(&mut self) -> bool {
        let dwell = if matches!(self.mode, Mode::Root { .. }) {
            ROOT_DWELL_MS
        } else {
            DWELL_MS
        };
        let quiet = self.clock_ms.saturating_sub(self.asked_ms) >= dwell;
        self.asked_ms = self.clock_ms;
        quiet
    }

    fn plan_key(&mut self, key: KeyEvent, details: &str) -> Effect {
        if !self.may_answer(&key) {
            return Effect::Nothing;
        }
        match key.code {
            KeyCode::Char('y' | 'Y') => self.answer("▸ confirmed", Reply::Yes),
            KeyCode::Char('n' | 'N') | KeyCode::Esc => self.answer("▸ cancelled", Reply::No),
            KeyCode::Char('d' | 'D') => {
                // Once: a held key repeats, and the history is not a log of key presses.
                if self.blocks.last() != Some(&Block::Plan(details.to_owned())) {
                    self.blocks.push(Block::Plan(details.to_owned()));
                }
                self.scroll = 0;
                Effect::Nothing
            }
            _ => Effect::Nothing,
        }
    }

    fn root_key(&mut self, key: KeyEvent) -> Effect {
        if !self.may_answer(&key) {
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

    fn choose_key(&mut self, key: KeyEvent, lines: &[ChoiceLine], selected: usize) -> Effect {
        let count = lines.iter().filter(|l| l.option.is_some()).count();
        let select = |state: &mut Self, index: usize| {
            state.mode = Mode::Choose {
                lines: lines.to_vec(),
                selected: index,
            };
            Effect::Nothing
        };
        if !self.may_answer(&key) {
            return Effect::Nothing;
        }
        match key.code {
            KeyCode::Up => select(self, selected.saturating_sub(1)),
            KeyCode::Down => select(self, (selected + 1).min(count.saturating_sub(1))),
            KeyCode::Enter => match lines.iter().find(|l| l.option == Some(selected)) {
                Some(line) => {
                    let note = format!("▸ chose {}", line.label);
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
                Block::Reply(text) => {
                    if !lines.is_empty() {
                        lines.push((Kind::Note, String::new()));
                    }
                    // Wrapped here, so every line of it carries the mark.
                    lines.extend(
                        reply::block(text, width, super::wrap::cell_width)
                            .into_iter()
                            .map(|line| (Kind::Reply, line)),
                    );
                    continue;
                }
                Block::Request(text) => (Kind::Request, format!("you ▸ {text}")),
                Block::Note(text) => (Kind::Note, text.clone()),
                Block::Plan(text) => (Kind::Plan, text.clone()),
                Block::Root(text) => (Kind::Root, text.clone()),
                Block::Outcome(text) => (Kind::Outcome, text.trim_end().to_owned()),
            };
            if !lines.is_empty() {
                lines.push((Kind::Note, String::new()));
            }
            lines.extend(
                wrap(
                    &crate::screen::fit_with(&text, width, super::wrap::cell_width),
                    width,
                )
                .into_iter()
                .map(|line| (kind, line)),
            );
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
    /// The history as plain text with a reply wrapped to `reply_width` cells:
    /// the width of the terminal it is printed to, so that no wrapped line of
    /// it loses its mark.
    pub fn transcript_for(&self, reply_width: usize) -> String {
        let mut out = String::new();
        for block in &self.blocks {
            match block {
                Block::Request(text) => out.push_str(&format!("{}{text}\n", self.prompt)),
                Block::Note(text) | Block::Plan(text) | Block::Root(text) => {
                    out.push_str(text);
                    out.push('\n');
                }
                Block::Reply(text) => {
                    let lines = reply::block(text, reply_width, reply::cautious_cells);
                    out.push_str(&lines.join("\n"));
                    out.push('\n');
                }
                Block::Outcome(text) => {
                    // A file has no width to lay the columns of a tree out for.
                    out.push_str(
                        {
                            let text = text.trim_end();
                            crate::screen::fit(text, crate::screen::natural_width(text))
                        }
                        .as_str(),
                    );
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

    /// A key pressed after a quiet gap, as a person pressing it deliberately would.
    fn tap(state: &mut State, code: KeyCode) -> Effect {
        state.clock_ms += DWELL_MS;
        state.on_key(press(code))
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
        state.clock_ms = ROOT_DWELL_MS;
        state
    }

    fn asking_choice(count: usize) -> State {
        let mut state = State::new("header".to_owned());
        state.on_event(Event::AskChoose {
            question: "Which one?".to_owned(),
            lines: (1..=count)
                .map(|n| ChoiceLine {
                    text: format!("option {n}"),
                    option: Some(n - 1),
                    label: String::new(),
                })
                .collect(),
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
        for after in [0, 100, ROOT_DWELL_MS - 1] {
            state.clock_ms = 5_000 + after;
            assert_eq!(state.on_key(press(KeyCode::Char('y'))), Effect::Nothing);
            assert!(state.is_asking());
        }
        state.clock_ms = 5_000 + ROOT_DWELL_MS - 1 + ROOT_DWELL_MS;
        assert_eq!(
            state.on_key(press(KeyCode::Char('y'))),
            Effect::Reply(Reply::Yes)
        );
    }

    #[test]
    fn a_held_key_never_answers_however_long_it_is_held() {
        let mut state = State::new(String::new());
        state.set_size(80, 24);
        state.on_event(Event::AskRoot {
            panel: "exact".to_owned(),
        });
        // Auto-repeat: a fresh press every 30 ms for ten seconds.
        for step in 0..333 {
            state.clock_ms = step * 30;
            assert_eq!(state.on_key(press(KeyCode::Char('y'))), Effect::Nothing);
        }
        assert!(state.is_asking());
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
    fn the_warning_about_a_running_step_comes_back_for_the_next_step() {
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let mut state = asking_plan();
        state.on_key(press(KeyCode::Char('y')));
        assert_eq!(state.on_key(ctrl_c), Effect::Nothing);
        state.on_event(Event::Outcome("done\n".to_owned()));
        type_text(&mut state, "again");
        state.on_key(press(KeyCode::Enter));
        state.on_event(Event::AskPlan {
            screen: "plan".to_owned(),
            details: String::new(),
        });
        state.clock_ms += DWELL_MS;
        state.on_key(press(KeyCode::Char('y')));
        assert_eq!(
            state.on_key(ctrl_c),
            Effect::Nothing,
            "warned again, not left"
        );
        assert!(
            !state.forced_quit
                || matches!(state.blocks.last(), Some(Block::Note(n)) if n.contains("Ctrl-C again"))
        );
    }

    #[test]
    fn a_question_clears_the_early_quit_so_leaving_at_it_waits_for_the_log() {
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let mut state = State::new(String::new());
        state.on_event(Event::Busy("working"));
        assert_eq!(state.on_key(ctrl_c), Effect::Nothing);
        assert!(state.forced_quit);
        state.on_event(Event::AskPlan {
            screen: "plan".to_owned(),
            details: String::new(),
        });
        assert!(!state.forced_quit);
        assert_eq!(state.on_key(ctrl_c), Effect::Quit);
        assert!(!state.forced_quit);
    }

    #[test]
    fn a_held_ctrl_c_does_not_leave_a_running_step() {
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let mut state = State::new(String::new());
        state.on_event(Event::Busy("working"));
        assert_eq!(state.on_key(ctrl_c), Effect::Nothing);
        for step in 1..100 {
            state.clock_ms = step * 30;
            assert_eq!(state.on_key(ctrl_c), Effect::Nothing, "repeat {step}");
        }
        state.clock_ms += DWELL_MS;
        assert_eq!(state.on_key(ctrl_c), Effect::Quit);
    }

    #[test]
    fn refusing_needs_no_quiet_gap() {
        let mut state = State::new(String::new());
        state.on_event(Event::AskPlan {
            screen: "plan".to_owned(),
            details: String::new(),
        });
        assert_eq!(
            state.on_key(press(KeyCode::Char('n'))),
            Effect::Reply(Reply::No)
        );
        let mut state = State::new(String::new());
        state.on_event(Event::AskRoot {
            panel: "exact".to_owned(),
        });
        assert_eq!(state.on_key(press(KeyCode::Esc)), Effect::Reply(Reply::No));
    }

    #[test]
    fn details_are_added_once_however_long_d_is_held() {
        let mut state = asking_plan();
        for _ in 0..50 {
            state.on_key(press(KeyCode::Char('d')));
        }
        let shown = state
            .blocks
            .iter()
            .filter(|b| **b == Block::Plan("the details".to_owned()))
            .count();
        assert_eq!(shown, 1);
    }

    #[test]
    fn enter_on_a_plan_does_not_delay_the_answer() {
        let mut state = asking_plan();
        state.on_key(press(KeyCode::Enter));
        state.clock_ms += 10;
        assert_eq!(
            state.on_key(press(KeyCode::Char('y'))),
            Effect::Reply(Reply::Yes)
        );
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
        state.clock_ms += DWELL_MS;
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
        tap(&mut state, KeyCode::Enter);
        assert!(state.is_asking());
        assert_eq!(
            tap(&mut state, KeyCode::Char('y')),
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
        assert_eq!(tap(&mut state, KeyCode::Char('y')), Effect::Nothing);
        assert!(state.is_asking());
        assert_eq!(
            tap(&mut state, KeyCode::Char('n')),
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
            tap(&mut state, KeyCode::Down);
        }
        assert_eq!(
            tap(&mut state, KeyCode::Enter),
            Effect::Reply(Reply::Choice(Some(2)))
        );
        let mut state = asking_choice(3);
        for _ in 0..5 {
            tap(&mut state, KeyCode::Up);
        }
        assert_eq!(
            tap(&mut state, KeyCode::Enter),
            Effect::Reply(Reply::Choice(Some(0)))
        );
    }

    #[test]
    fn the_selection_moves_over_the_candidates_and_skips_their_context() {
        let line = |text: &str, option| ChoiceLine {
            text: text.to_owned(),
            option,
            label: text.trim().to_owned(),
        };
        let mut state = State::new("header".to_owned());
        state.on_event(Event::AskChoose {
            question: "Which one?".to_owned(),
            lines: vec![
                line("~", None),
                line("├─ Documents/", None),
                line("│  └─ report.pdf", Some(0)),
                line("└─ Downloads/", None),
                line("   └─ report.pdf", Some(1)),
            ],
        });
        state.clock_ms = DWELL_MS;
        for _ in 0..5 {
            tap(&mut state, KeyCode::Down);
        }
        assert_eq!(
            tap(&mut state, KeyCode::Enter),
            Effect::Reply(Reply::Choice(Some(1)))
        );
    }

    #[test]
    fn the_history_says_which_location_was_chosen() {
        let mut state = State::new("header".to_owned());
        state.on_event(Event::AskChoose {
            question: "Which one?".to_owned(),
            lines: vec![ChoiceLine {
                text: "└─ report.pdf".to_owned(),
                option: Some(0),
                label: "\"Downloads/report.pdf\"".to_owned(),
            }],
        });
        state.clock_ms = DWELL_MS;
        tap(&mut state, KeyCode::Enter);
        assert!(
            state
                .blocks
                .iter()
                .any(|b| format!("{b:?}").contains("chose \\\"Downloads/report.pdf")),
            "{:?}",
            state.blocks
        );
    }

    #[test]
    fn the_header_follows_the_directory_the_session_is_in() {
        let mut state = State::new("header in ~".to_owned());
        state.on_event(Event::Header("header in ~/Downloads".to_owned()));
        assert_eq!(state.header, "header in ~/Downloads");
    }

    #[test]
    fn a_choice_can_be_cancelled() {
        let mut state = asking_choice(2);
        assert_eq!(
            tap(&mut state, KeyCode::Esc),
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
    fn with_no_model_a_sent_line_is_work_not_a_wait_for_the_model() {
        let sent = |mut state: State| {
            for c in "fs list --in home".chars() {
                state.on_key(press(KeyCode::Char(c)));
            }
            state.on_key(press(KeyCode::Enter));
            state.mode
        };
        assert!(matches!(
            sent(State::for_entry(String::new(), Entry::Request)),
            Mode::Busy {
                label: "asking the model",
                ..
            }
        ));
        // Leaving waits for a running step only under the label "working".
        assert!(matches!(
            sent(State::for_entry(String::new(), Entry::Command)),
            Mode::Busy {
                label: "working",
                ..
            }
        ));
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
        assert_eq!(state.transcript_for(80), "intent> list downloads\ndone\n");
        let mut commands = State::for_entry(String::new(), Entry::Command);
        commands
            .blocks
            .push(Block::Request("fs list --in home".to_owned()));
        assert_eq!(commands.transcript_for(80), "command> fs list --in home\n");
    }

    #[test]
    fn the_transcript_lays_the_columns_of_a_tree_out_instead_of_keeping_the_tab() {
        let mut state = State::new(String::new());
        state.on_event(Event::Outcome("a.txt\t5 KB\n".to_owned()));
        let transcript = state.transcript_for(80);
        assert!(!transcript.contains('\t'), "{transcript:?}");
        assert!(transcript.trim_end().ends_with("5 KB"), "{transcript:?}");
    }

    #[test]
    fn a_reply_is_a_block_under_the_result_and_none_changes_nothing() {
        let mut without = State::new("h".to_owned());
        without.on_event(Event::Outcome("done\n".to_owned()));
        let mut none = State::new("h".to_owned());
        none.on_event(Event::Outcome("done\n".to_owned()));
        none.on_event(Event::Busy("saying"));
        none.on_event(Event::Said(None));
        assert_eq!(none.mode, Mode::Input);
        assert_eq!(none.blocks, without.blocks);
        assert_eq!(none.transcript_for(80), without.transcript_for(80));
        assert_eq!(none.history_lines(40), without.history_lines(40));

        let mut with = State::new("h".to_owned());
        with.on_event(Event::Outcome("done\n".to_owned()));
        with.on_event(Event::Said(Some("All done here.".to_owned())));
        assert_eq!(with.mode, Mode::Input);
        let lines = with.history_lines(40);
        assert_eq!(lines[0], (Kind::Outcome, "done".to_owned()));
        let replied: Vec<_> = lines.iter().filter(|(k, _)| *k == Kind::Reply).collect();
        assert_eq!(replied.last().unwrap().1, "  » All done here.", "{lines:?}");
        assert!(with.transcript_for(80).starts_with("done\n"));
        assert!(with.transcript_for(80).ends_with("  » All done here.\n"));
    }

    #[test]
    fn every_wrapped_line_of_a_reply_on_a_narrow_screen_is_marked() {
        let mut state = State::new("h".to_owned());
        state.on_event(Event::Outcome("done\n".to_owned()));
        state.on_event(Event::Said(Some(
            "s2 delete Documents -> done and a long tail that must wrap ".repeat(4),
        )));
        for width in [12, 24, 40] {
            for (kind, line) in state.history_lines(width) {
                if kind == Kind::Reply {
                    assert!(line.starts_with(crate::reply::MARK), "{line:?}");
                    assert!(text_width(&line) <= width, "{line:?} at {width}");
                } else {
                    assert!(!line.starts_with(crate::reply::MARK), "{line:?}");
                }
            }
        }
    }

    #[test]
    fn leaving_while_the_reply_is_written_is_at_once_because_nothing_is_running() {
        let mut state = State::new("h".to_owned());
        state.on_event(Event::Busy("saying"));
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(state.on_key(key), Effect::Quit);
        assert!(!state.forced_quit);
        // A step that runs still asks for the second Ctrl-C.
        state.on_event(Event::Busy("working"));
        assert_eq!(state.on_key(key), Effect::Nothing);
    }

    #[test]
    fn no_request_can_start_between_the_result_and_its_reply_and_a_late_reply_cannot_end_a_wait() {
        let mut state = State::new("h".to_owned());
        state.on_event(Event::OutcomeAwaitingReply("done\n".to_owned()));
        assert!(matches!(
            state.mode,
            Mode::Busy {
                label: "saying",
                ..
            }
        ));
        assert_eq!(state.on_key(press(KeyCode::Char('x'))), Effect::Nothing);
        assert!(state.input.is_empty());
        // A reply that arrives while the person is being asked something
        // does not end that question.
        state.on_event(Event::AskPlan {
            screen: "s".to_owned(),
            details: "d".to_owned(),
        });
        state.on_event(Event::Said(Some("late".to_owned())));
        assert!(matches!(state.mode, Mode::Plan { .. }));
    }

    #[test]
    fn text_from_a_file_cannot_contain_the_mark_so_no_wrapped_line_can_start_like_a_reply() {
        // A line of file content that, wrapped at a width, would leave `  » ` at the start of a row.
        let hostile = format!("{}  » {}", "A".repeat(80), crate::reply::TITLE);
        let shown = printable(&hostile, true);
        assert!(!shown.contains('\u{bb}'), "{shown}");
        let outcome = format!("s1 Read \"x\" -> done\n    {shown}");
        for width in [20, 40, 80, 100] {
            let mut state = State::new(String::new());
            state.on_event(Event::Outcome(outcome.clone()));
            for (_, line) in state.history_lines(width) {
                assert!(!line.starts_with(crate::reply::MARK), "{line:?} at {width}");
            }
        }
    }

    #[test]
    fn a_reply_does_not_move_the_person_who_is_reading_back() {
        let mut state = State::new("h".to_owned());
        let long: String = (1..=50).map(|n| format!("l{n}\n")).collect();
        state.on_event(Event::OutcomeAwaitingReply(long));
        state.height = 12;
        state.on_key(press(KeyCode::PageUp));
        let reading = state.visible_history(80, 5);
        assert_ne!(reading.last().unwrap().1, "l50");
        state.on_event(Event::Said(Some("hello".to_owned())));
        assert_eq!(state.visible_history(80, 5), reading);
        state.on_event(Event::Outcome("again\n".to_owned()));
        assert_eq!(state.scroll, 0);
    }

    #[test]
    fn a_reply_printed_when_the_interface_closes_is_wrapped_to_the_terminal() {
        let mut state = State::new("h".to_owned());
        state.on_event(Event::Said(Some("word ".repeat(30))));
        for width in [30, 60, 100] {
            for line in state.transcript_for(width).lines() {
                assert!(line.starts_with(crate::reply::MARK), "{line:?}");
                assert!(line.chars().count() <= width, "{line:?} at {width}");
            }
        }
    }

    // ---- push-to-talk (ADR 0031) ------------------------------------------

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn listening_state() -> State {
        State::new("header".to_owned()).with_listening()
    }

    /// Ctrl-T pressed, and Enter after the gap that tells a deliberate stop from a held key.
    fn talking(state: &mut State) {
        assert_eq!(state.on_key(ctrl('t')), Effect::Listen(Listen::Start));
        state.clock_ms += DWELL_MS;
    }

    #[test]
    fn without_listen_there_is_no_microphone_key_and_the_hint_is_unchanged() {
        let mut state = State::new("header".to_owned());
        assert_eq!(state.on_key(ctrl('t')), Effect::Nothing);
        assert_eq!(state.mode, Mode::Input);
        assert_eq!(state.input_hint(), "Enter send · PgUp/Dn scroll · ^D quit");
        assert!(listening_state().input_hint().contains("^T talk"));
        // A transcript that arrives anyway is dropped.
        state.on_event(Event::Heard(Ok("x".to_owned())));
        assert!(state.input.is_empty());
    }

    #[test]
    fn what_was_heard_waits_in_the_input_line_and_is_sent_only_by_enter() {
        let mut state = listening_state();
        talking(&mut state);
        assert!(matches!(
            state.mode,
            Mode::Busy {
                label: "listening",
                ..
            }
        ));
        assert_eq!(
            state.on_key(press(KeyCode::Enter)),
            Effect::Listen(Listen::Stop)
        );
        assert!(matches!(
            state.mode,
            Mode::Busy {
                label: "transcribing",
                ..
            }
        ));
        state.on_event(Event::Heard(Ok("list downloads".to_owned())));
        assert_eq!(state.mode, Mode::Input);
        assert_eq!(state.input, "list downloads");
        // Nothing was submitted by hearing it: the person reads, may edit, and presses Enter.
        assert!(state.blocks.iter().all(|b| !matches!(b, Block::Request(_))));
        // A person reads it for longer than the gap that ignores a held key.
        state.clock_ms += DWELL_MS;
        type_text(&mut state, "!");
        assert_eq!(
            state.on_key(press(KeyCode::Enter)),
            Effect::Submit("list downloads!".to_owned())
        );
    }

    #[test]
    fn a_transcript_is_added_to_what_was_typed_not_in_place_of_it() {
        let mut state = listening_state();
        type_text(&mut state, "show");
        talking(&mut state);
        state.on_key(press(KeyCode::Enter));
        state.on_event(Event::Heard(Ok("the largest files".to_owned())));
        assert_eq!(state.input, "show the largest files");
    }

    #[test]
    fn a_transcript_never_answers_a_question_and_never_enters_any_other_mode() {
        for mut state in [asking_plan(), asking_root("rm /x"), asking_choice(2)] {
            let (mode, blocks) = (state.mode.clone(), state.blocks.len());
            for heard in [
                Ok("y".to_owned()),
                Ok("yes confirm".to_owned()),
                Err("x".to_owned()),
            ] {
                state.on_event(Event::Heard(heard));
                assert_eq!(state.mode, mode);
                assert_eq!(state.blocks.len(), blocks);
                assert!(state.input.is_empty());
            }
            // The microphone key does nothing at a question either.
            assert_eq!(state.on_key(ctrl('t')), Effect::Nothing);
            assert_eq!(state.mode, mode);
        }
        for label in ["working", "asking the model", "saying"] {
            let mut busy = listening_state();
            busy.on_event(Event::Busy(label));
            let mode = busy.mode.clone();
            busy.on_event(Event::Heard(Ok("hello".to_owned())));
            assert_eq!((busy.mode.clone(), busy.input.as_str()), (mode, ""));
            assert_eq!(busy.on_key(ctrl('t')), Effect::Nothing);
        }
        let mut idle = listening_state();
        idle.on_event(Event::Heard(Ok("hello".to_owned())));
        assert!(idle.input.is_empty());
    }

    #[test]
    fn a_held_key_does_not_stop_the_recording_it_started_and_escape_cancels_it() {
        let mut state = listening_state();
        assert_eq!(state.on_key(ctrl('t')), Effect::Listen(Listen::Start));
        // The auto-repeat of the same press, before the gap.
        assert_eq!(state.on_key(ctrl('t')), Effect::Nothing);
        assert_eq!(state.on_key(press(KeyCode::Enter)), Effect::Nothing);
        assert!(matches!(
            state.mode,
            Mode::Busy {
                label: "listening",
                ..
            }
        ));
        assert_eq!(
            state.on_key(press(KeyCode::Esc)),
            Effect::Listen(Listen::Cancel)
        );
        assert_eq!(state.mode, Mode::Input);
        // A transcript of the cancelled recording that still arrives is dropped.
        state.on_event(Event::Heard(Ok("late".to_owned())));
        assert!(state.input.is_empty());
    }

    #[test]
    fn nothing_typed_reaches_the_input_while_listening_and_a_failure_says_why() {
        let mut state = listening_state();
        talking(&mut state);
        assert_eq!(state.on_key(press(KeyCode::Char('x'))), Effect::Nothing);
        assert!(state.input.is_empty());
        state.on_event(Event::Heard(Err("nothing was heard".to_owned())));
        assert_eq!(state.mode, Mode::Input);
        assert!(state.input.is_empty());
        assert_eq!(
            state.blocks.last(),
            Some(&Block::Note("▸ nothing was heard".to_owned()))
        );
        // A note changes no mode.
        let mut busy = listening_state();
        busy.on_event(Event::Busy("working"));
        busy.on_event(Event::Note("▸ speech is off for this reply: x".to_owned()));
        assert!(matches!(
            busy.mode,
            Mode::Busy {
                label: "working",
                ..
            }
        ));
    }

    #[test]
    fn a_speech_note_neither_scrolls_nor_restarts_a_question_nor_cancels_a_warning() {
        let mut state = asking_plan();
        state.scroll = 7;
        state.asked_ms = 5;
        state.clock_ms = 1_000;
        state.forced_quit = true;
        state.on_event(Event::Note("▸ speech is off for this reply: x".to_owned()));
        state.on_event(Event::Heard(Ok("late".to_owned())));
        assert_eq!(
            (state.scroll, state.asked_ms, state.forced_quit),
            (7, 5, true)
        );
        // The result of the shell's own work still does.
        state.on_event(Event::Header("h".to_owned()));
        assert_eq!(
            (state.scroll, state.asked_ms, state.forced_quit),
            (0, 1_000, false)
        );
    }

    #[test]
    fn a_key_still_held_from_stopping_the_recording_neither_sends_nor_restarts_it() {
        for stop in [press(KeyCode::Enter), ctrl('t')] {
            let mut state = listening_state();
            talking(&mut state);
            state.on_key(stop);
            // The transcript lands while the key is still down.
            state.on_event(Event::Heard(Ok("list downloads".to_owned())));
            assert_eq!(state.on_key(press(KeyCode::Enter)), Effect::Nothing);
            assert_eq!(state.on_key(ctrl('t')), Effect::Nothing);
            assert_eq!(
                (state.mode.clone(), state.input.as_str()),
                (Mode::Input, "list downloads")
            );
            // After a quiet gap Enter sends it, and the key works again.
            state.clock_ms += DWELL_MS;
            assert_eq!(
                state.on_key(press(KeyCode::Enter)),
                Effect::Submit("list downloads".to_owned())
            );
        }
    }

    #[test]
    fn a_speech_note_at_a_question_does_not_ask_the_loop_to_discard_pending_keys() {
        let mut state = asking_plan();
        assert!(!state.on_event(Event::Note("▸ speech is off".to_owned())));
        assert!(!state.on_event(Event::Heard(Ok("late".to_owned()))));
        // A question drawn by the shell does.
        assert!(state.on_event(Event::AskRoot {
            panel: "p".to_owned()
        }));
    }
}
