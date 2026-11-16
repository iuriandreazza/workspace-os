//! The conversational terminal interface (roadmap S3c, ADR 0015).
//!
//! An adapter only: the interface owns the terminal on the main thread, and
//! the shell runs on a worker thread whose `Confirm` and `Planner` talk to it
//! over channels. The core keeps its blocking traits and learns nothing about
//! screens. Everything shown comes from `screen` and `Outcome::render`.

mod state;
mod view;
mod wrap;

use std::io::stdout;
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::thread;
use std::time::{Duration, Instant};

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event as Input, KeyEventKind, MouseEventKind,
};
use ratatui::crossterm::execute;

use crate::plain::Setup;
use crate::ports::{Confirm, Planner, Retry, User};
use crate::session::Shell;
use state::{Block, Effect, Event, Reply, State};

const TICK: Duration = Duration::from_millis(100);

/// The worker's `Confirm`: asks the interface and waits. If the interface is
/// gone or answers something else, the answer is no.
struct ChannelConfirm {
    events: Sender<Event>,
    replies: Receiver<Reply>,
}

impl ChannelConfirm {
    fn ask(&self, event: Event) -> Option<Reply> {
        self.events.send(event).ok()?;
        self.replies.recv().ok()
    }
}

impl Confirm for ChannelConfirm {
    fn plan(&mut self, screen: &str, details: &str) -> bool {
        let event = Event::AskPlan {
            screen: screen.to_owned(),
            details: details.to_owned(),
        };
        self.ask(event) == Some(Reply::Yes)
    }

    fn root_action(&mut self, panel: &str, _who: &User) -> bool {
        let event = Event::AskRoot {
            panel: panel.to_owned(),
        };
        self.ask(event) == Some(Reply::Yes)
    }

    fn choose(&mut self, question: &str, options: &[String]) -> Option<usize> {
        let event = Event::AskChoose {
            question: question.to_owned(),
            options: options.to_vec(),
        };
        match self.ask(event) {
            Some(Reply::Choice(Some(index))) if index < options.len() => Some(index),
            _ => None,
        }
    }
}

/// Tells the interface the model is being asked, which can take minutes.
struct ProgressPlanner<'a> {
    inner: &'a dyn Planner,
    events: Sender<Event>,
}

impl Planner for ProgressPlanner<'_> {
    fn plan(&self, request: &str, retry: Option<&Retry>) -> Result<String, String> {
        let _ = self.events.send(Event::Busy("asking the model"));
        self.inner.plan(request, retry)
    }
}

fn worker(
    setup: Setup,
    requests: Receiver<String>,
    events: Sender<Event>,
    replies: Receiver<Reply>,
) {
    let Setup {
        authority,
        host,
        planner,
        mut log,
        ..
    } = setup;
    let planner = ProgressPlanner {
        inner: &planner,
        events: events.clone(),
    };
    let mut confirm = ChannelConfirm {
        events: events.clone(),
        replies,
    };
    for request in requests {
        let outcome = Shell {
            authority: &authority,
            host: &host,
            planner: &planner,
            confirm: &mut confirm,
            log: &mut log,
        }
        .handle(&request)
        .render();
        if events.send(Event::Outcome(outcome)).is_err() {
            return;
        }
    }
}

/// Discard what was typed before a question was drawn, so a key meant for
/// something else cannot answer it.
fn discard_pending_keys() -> std::io::Result<()> {
    while event::poll(Duration::ZERO)? {
        event::read()?;
    }
    Ok(())
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    state: &mut State,
    requests: &Sender<String>,
    replies: &Sender<Reply>,
    events: &Receiver<Event>,
) -> std::io::Result<()> {
    let started = Instant::now();
    loop {
        let size = terminal.size()?;
        state.set_size(size.width, size.height);
        state.clock_ms = started.elapsed().as_millis() as u64;
        terminal.draw(|frame| view::draw(frame, state))?;
        if event::poll(TICK)? {
            let effect = match event::read()? {
                Input::Key(key) if key.kind != KeyEventKind::Release => state.on_key(key),
                Input::Mouse(mouse) => {
                    match mouse.kind {
                        MouseEventKind::ScrollUp => state.wheel(true),
                        MouseEventKind::ScrollDown => state.wheel(false),
                        _ => {}
                    }
                    Effect::Nothing
                }
                _ => Effect::Nothing,
            };
            match effect {
                Effect::Nothing => {}
                Effect::Quit => return Ok(()),
                Effect::Submit(request) => {
                    let _ = requests.send(request);
                }
                Effect::Reply(reply) => {
                    let _ = replies.send(reply);
                }
            }
        }
        loop {
            match events.try_recv() {
                Ok(event) => {
                    if state.on_event(event) {
                        // Draw the question first, then drop earlier keys.
                        terminal.draw(|frame| view::draw(frame, state))?;
                        discard_pending_keys()?;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    state
                        .blocks
                        .push(Block::Note("The shell stopped unexpectedly.".to_owned()));
                    return Ok(());
                }
            }
        }
    }
}

/// Open the interface and run until the person quits. The history is printed
/// to the normal screen on exit, and written to `transcript` when given.
pub fn run(setup: Setup, transcript: Option<&Path>) -> Result<(), String> {
    let mut state = State::new(setup.header());
    let (request_tx, requests) = channel();
    let (event_tx, events) = channel();
    let (reply_tx, replies) = channel();
    thread::spawn(move || worker(setup, requests, event_tx, replies));

    let mut terminal = ratatui::init();
    // `ratatui::init` restores the screen on a panic; the mouse mode is ours.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(stdout(), DisableMouseCapture);
        previous(info);
    }));
    let _ = execute!(stdout(), EnableMouseCapture);
    let result = event_loop(&mut terminal, &mut state, &request_tx, &reply_tx, &events);
    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();

    let text = state.transcript();
    print!("{text}");
    if let Some(path) = transcript {
        std::fs::write(path, &text)
            .map_err(|e| format!("cannot write the transcript {}: {e}", path.display()))?;
    }
    result.map_err(|e| format!("terminal error: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn confirm() -> (ChannelConfirm, Receiver<Event>, Sender<Reply>) {
        let (events_tx, events) = channel();
        let (replies_tx, replies) = channel();
        (
            ChannelConfirm {
                events: events_tx,
                replies,
            },
            events,
            replies_tx,
        )
    }

    fn user() -> User {
        User {
            name: "u".to_owned(),
            administrator: true,
        }
    }

    #[test]
    fn a_yes_from_the_interface_confirms_and_the_question_reaches_it() {
        let (mut confirm, events, replies) = confirm();
        replies.send(Reply::Yes).unwrap();
        assert!(confirm.root_action("the panel", &user()));
        assert_eq!(
            events.try_recv(),
            Ok(Event::AskRoot {
                panel: "the panel".to_owned()
            })
        );
    }

    #[test]
    fn a_closed_interface_refuses_and_cancels() {
        let (mut confirm, events, replies) = confirm();
        drop(replies);
        assert!(!confirm.plan("screen", "details"));
        assert!(!confirm.root_action("panel", &user()));
        assert_eq!(confirm.choose("which", &["a".to_owned()]), None);
        drop(events);
        assert!(!confirm.plan("screen", "details"));
    }

    #[test]
    fn an_answer_of_the_wrong_kind_is_a_refusal() {
        let (mut confirm, _events, replies) = confirm();
        replies.send(Reply::Choice(Some(0))).unwrap();
        assert!(!confirm.plan("screen", "details"));
        replies.send(Reply::Yes).unwrap();
        assert_eq!(confirm.choose("which", &["a".to_owned()]), None);
    }

    #[test]
    fn a_choice_outside_the_list_is_a_cancel() {
        let (mut confirm, _events, replies) = confirm();
        replies.send(Reply::Choice(Some(5))).unwrap();
        assert_eq!(confirm.choose("which", &["a".to_owned()]), None);
        replies.send(Reply::Choice(Some(0))).unwrap();
        assert_eq!(confirm.choose("which", &["a".to_owned()]), Some(0));
    }
}
