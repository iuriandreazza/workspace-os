//! The conversational terminal interface (roadmap S3c, ADR 0015).
//!
//! An adapter only: the interface owns the terminal on the main thread, and
//! the shell runs on a worker thread whose `Confirm` and `Planner` talk to it
//! over channels. The core keeps its blocking traits and learns nothing about
//! screens. Everything shown comes from `screen` and `Outcome::render`.

mod state;
mod view;
mod wrap;

use std::io::{Write, stdout};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::thread;
use std::time::{Duration, Instant};

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event as Input, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind,
};
use ratatui::crossterm::execute;

use crate::broker::Authority;
use crate::ports::{ChoiceLine, Confirm, DecisionLog, Host, Planner, Retry, User};
use crate::session::{SessionState, Shell};
use crate::setup::Setup;
use state::{Effect, Event, Mode, Reply, State};

/// How long a step is waited for after a termination signal.
const SIGNAL_PATIENCE: Duration = Duration::from_secs(5);
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

    fn choose(&mut self, question: &str, lines: &[ChoiceLine]) -> Option<usize> {
        let event = Event::AskChoose {
            question: question.to_owned(),
            lines: lines.to_vec(),
        };
        let count = lines.iter().filter(|l| l.option.is_some()).count();
        match self.ask(event) {
            Some(Reply::Choice(Some(index))) if index < count => Some(index),
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
    fn plan(&self, request: &str, cwd: &str, retry: Option<&Retry>) -> Result<String, String> {
        let _ = self.events.send(Event::Busy("asking the model"));
        let answer = self.inner.plan(request, cwd, retry);
        // From here the shell reviews and may search: it is no longer only a
        // wait for the model, so leaving waits for it (see `run`).
        let _ = self.events.send(Event::Busy("working"));
        answer
    }
}

/// The channels between the worker and the interface.
struct Wires {
    requests: Receiver<String>,
    events: Sender<Event>,
    replies: Receiver<Reply>,
}

/// Runs the shell for each request until the interface closes the channel.
/// Takes trait objects so a test can drive it without a model or a terminal.
fn serve(
    authority: &Authority,
    host: &dyn Host,
    planner: &dyn Planner,
    log: &mut dyn DecisionLog,
    wires: Wires,
    header: &dyn Fn(&str) -> String,
) {
    let Wires {
        requests,
        events,
        replies,
    } = wires;
    let planner = ProgressPlanner {
        inner: planner,
        events: events.clone(),
    };
    let mut confirm = ChannelConfirm {
        events: events.clone(),
        replies,
    };
    let mut session = SessionState::start(host);
    for request in requests {
        let outcome = Shell {
            session: &mut session,
            authority,
            host,
            planner: &planner,
            confirm: &mut confirm,
            log,
        }
        .handle(&request)
        .render();
        // A `cd` changes where the session is, which the header says.
        let _ = events.send(Event::Header(header(&session.cwd_text(host))));
        if events.send(Event::Outcome(outcome)).is_err() {
            return;
        }
    }
}

fn worker(
    setup: Setup,
    requests: Receiver<String>,
    events: Sender<Event>,
    replies: Receiver<Reply>,
) {
    let header = setup.header_fn();
    let Setup {
        authority,
        host,
        planner,
        mut log,
        ..
    } = setup;
    let wires = Wires {
        requests,
        events,
        replies,
    };
    serve(&authority, &host, &planner, &mut log, wires, &header);
}

fn is_ctrl_c(key: &KeyEvent) -> bool {
    key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
}

/// Discard what was typed before a question was drawn, so a key meant for
/// something else cannot answer it. A Ctrl-C among them is not discarded: it
/// is reported, so wanting to leave is never lost.
fn discard_pending_keys() -> std::io::Result<bool> {
    let mut leave = false;
    while event::poll(Duration::ZERO)? {
        if let Input::Key(key) = event::read()? {
            leave |= is_ctrl_c(&key);
        }
    }
    Ok(leave)
}

/// Counts SIGTERM, SIGHUP, SIGINT and SIGQUIT (a closed terminal, `kill`), so the loop can
/// restore the screen instead of leaving the terminal in raw mode.
static TERMINATIONS: AtomicUsize = AtomicUsize::new(0);

extern "C" fn on_terminate(_signal: libc::c_int) {
    TERMINATIONS.fetch_add(1, Ordering::Relaxed);
}

fn watch_termination() {
    for signal in [libc::SIGTERM, libc::SIGHUP, libc::SIGINT, libc::SIGQUIT] {
        // SAFETY: the handler only stores to an atomic, which is async-signal-safe,
        // and the function pointer has the signature `signal` expects.
        unsafe {
            libc::signal(
                signal,
                on_terminate as extern "C" fn(libc::c_int) as libc::sighandler_t,
            );
        }
    }
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
        if TERMINATIONS.load(Ordering::Relaxed) > 0 {
            return Ok(());
        }
        let size = terminal.size()?;
        state.set_size(size.width, size.height);
        state.clock_ms = started.elapsed().as_millis() as u64;
        terminal.draw(|frame| view::draw(frame, state))?;
        if event::poll(TICK)? {
            let input = event::read()?;
            state.clock_ms = started.elapsed().as_millis() as u64;
            // The size may have changed since the draw: an answer is judged
            // against the screen as it is now.
            let size = terminal.size()?;
            state.set_size(size.width, size.height);
            let effect = match input {
                Input::Key(key) if key.kind != KeyEventKind::Release => state.on_key(key),
                Input::Paste(text) => {
                    state.paste(&text);
                    Effect::Nothing
                }
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
                    state.clock_ms = started.elapsed().as_millis() as u64;
                    if state.on_event(event) {
                        // Draw the question first, then drop earlier keys.
                        terminal.draw(|frame| view::draw(frame, state))?;
                        if discard_pending_keys()? {
                            return Ok(());
                        }
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    return Err(std::io::Error::other("the shell stopped unexpectedly"));
                }
            }
        }
    }
}

/// Write the transcript where only the person can read it: it holds requests
/// and real locations, like the decision log.
fn write_transcript(path: &Path, text: &str) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(text.as_bytes())
}

/// Open the interface and run until the person quits. The history is printed
/// to the normal screen on exit, and written to `transcript` when given.
pub fn run(setup: Setup, transcript: Option<&Path>) -> Result<(), String> {
    let mut state = State::new(setup.header(&SessionState::start(&setup.host)));
    let (request_tx, requests) = channel();
    let (event_tx, events) = channel();
    let (reply_tx, replies) = channel();

    let mut terminal = ratatui::init();
    // `ratatui::init` restores the screen on a panic; the mouse and paste
    // modes are ours. The process aborts on a panic (workspace profile), so
    // this runs once and the terminal is left as it was found.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(stdout(), DisableBracketedPaste, DisableMouseCapture);
        previous(info);
    }));
    let _ = execute!(stdout(), EnableMouseCapture, EnableBracketedPaste);
    watch_termination();
    let worker = thread::spawn(move || worker(setup, requests, event_tx, replies));
    let result = event_loop(&mut terminal, &mut state, &request_tx, &reply_tx, &events);

    // Questions sent but not yet read from the channel still count as asked.
    while let Ok(event) = events.try_recv() {
        state.on_event(event);
    }
    // Leaving at a question is a refusal, and the decision log must say so:
    // closing the channels lets the worker's wait fail and record it. A running
    // step is waited for rather than killed half way. Only a wait for the
    // model is abandoned, because nothing has run and the request is logged.
    let waiting_for_model = matches!(
        state.mode,
        Mode::Busy {
            label: "asking the model",
            ..
        }
    );
    drop((request_tx, reply_tx));
    // A second Ctrl-C or signal stops the wait for a stuck step, and a signal
    // alone (a closed terminal never sends a second) waits only a few seconds.
    let waiting_since = Instant::now();
    while !waiting_for_model
        && !state.forced_quit
        && !worker.is_finished()
        && TERMINATIONS.load(Ordering::Relaxed) < 2
        && (TERMINATIONS.load(Ordering::Relaxed) == 0 || waiting_since.elapsed() < SIGNAL_PATIENCE)
    {
        // The loop has ended, so keys are read here: Ctrl-C stops the wait.
        match event::poll(Duration::from_millis(50)) {
            Ok(true) => {
                if let Ok(Input::Key(key)) = event::read()
                    && is_ctrl_c(&key)
                {
                    break;
                }
            }
            Ok(false) => {}
            // A dead terminal fails at once; without this the loop would spin.
            Err(_) => thread::sleep(Duration::from_millis(50)),
        }
    }
    // What the worker finished while it was waited for belongs in the history.
    while let Ok(event) = events.try_recv() {
        state.on_event(event);
    }

    // Saved before the terminal is touched: on a closed terminal restoring it
    // can fail, and that must not stop the transcript from being saved.
    let text = state.transcript();
    let written = transcript.map_or(Ok(()), |path| {
        write_transcript(path, &text)
            .map_err(|e| format!("cannot write the transcript {}: {e}", path.display()))
    });
    let _ = execute!(stdout(), DisableBracketedPaste, DisableMouseCapture);
    let _ = ratatui::try_restore();
    let _ = write!(stdout(), "{text}");
    result.map_err(|e| format!("terminal error: {e}"))?;
    written
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

    fn a_choice() -> ChoiceLine {
        ChoiceLine {
            text: "a".to_owned(),
            option: Some(0),
            label: String::new(),
        }
    }

    #[test]
    fn a_closed_interface_refuses_and_cancels() {
        let (mut confirm, events, replies) = confirm();
        drop(replies);
        assert!(!confirm.plan("screen", "details"));
        assert!(!confirm.root_action("panel", &user()));
        assert_eq!(confirm.choose("which", &[a_choice()]), None);
        drop(events);
        assert!(!confirm.plan("screen", "details"));
    }

    #[test]
    fn an_answer_of_the_wrong_kind_is_a_refusal() {
        let (mut confirm, _events, replies) = confirm();
        replies.send(Reply::Choice(Some(0))).unwrap();
        assert!(!confirm.plan("screen", "details"));
        replies.send(Reply::Yes).unwrap();
        assert_eq!(confirm.choose("which", &[a_choice()]), None);
    }

    #[test]
    fn a_choice_outside_the_list_is_a_cancel() {
        let (mut confirm, _events, replies) = confirm();
        replies.send(Reply::Choice(Some(5))).unwrap();
        assert_eq!(confirm.choose("which", &[a_choice()]), None);
        replies.send(Reply::Choice(Some(0))).unwrap();
        assert_eq!(confirm.choose("which", &[a_choice()]), Some(0));
    }

    #[test]
    fn leaving_at_a_question_is_a_refusal_in_the_decision_log() {
        use crate::pipeline_tests::{DELETE_LARGEST, files};
        use crate::testing::{MemoryLog, ScriptedPlanner, user};

        let (request_tx, requests) = channel();
        let (events_tx, events) = channel();
        let (reply_tx, replies) = channel();
        request_tx.send("largest".to_owned()).unwrap();
        // The person quits at the first question: nothing is ever answered.
        drop((request_tx, reply_tx));
        let mut log = MemoryLog::default();
        serve(
            &Authority::full(user(false)),
            &files(),
            &ScriptedPlanner::new(&[DELETE_LARGEST]),
            &mut log,
            Wires {
                requests,
                events: events_tx,
                replies,
            },
            &|cwd| format!("header in {cwd}"),
        );
        let seen: Vec<Event> = events.try_iter().collect();
        assert!(seen.iter().any(|e| matches!(e, Event::AskPlan { .. })));
        assert!(
            log.events
                .iter()
                .any(|e| e["event"] == "plan_confirmation" && e["confirmed"] == false),
            "{:?}",
            log.events
        );
        assert!(
            !log.events.iter().any(|e| e["event"] == "executed"),
            "nothing ran"
        );
    }

    #[test]
    fn the_transcript_is_private_to_the_user() {
        let dir =
            std::env::temp_dir().join(format!("intent-shell-transcript-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.txt");
        write_transcript(&path, "requests and paths\n").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
