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
use crate::interview::{self, Dialog};
use crate::model::NoModel;
use crate::ports::{ChoiceLine, Confirm, DecisionLog, Host, Planner, Retry, User};
use crate::reply::{self, Voice};
use crate::session::{Entry, SessionState, Shell};
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
    /// Set once a question could not be put or answered: the interface has
    /// been closed, and nothing more is worth asking the model for.
    gone: bool,
}

impl ChannelConfirm {
    fn ask(&mut self, event: Event) -> Option<Reply> {
        let answer = self
            .events
            .send(event)
            .ok()
            .and_then(|()| self.replies.recv().ok());
        self.gone |= answer.is_none();
        answer
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
    model: Option<&dyn Planner>,
    log: &mut dyn DecisionLog,
    wires: Wires,
    header: &dyn Fn(&str) -> String,
    voice: Option<Voice>,
) {
    let Wires {
        requests,
        events,
        replies,
    } = wires;
    let entry = Entry::for_model(model.is_some());
    let planner = ProgressPlanner {
        inner: model.unwrap_or(&NoModel),
        events: events.clone(),
    };
    let mut confirm = ChannelConfirm {
        events: events.clone(),
        replies,
        gone: false,
    };
    let mut session = SessionState::start(host);
    for request in requests {
        if entry == Entry::Command {
            // With no model nothing else says the shell is working, and leaving
            // must wait for the step rather than abandon it.
            let _ = events.send(Event::Busy("working"));
        }
        let outcome = Shell {
            session: &mut session,
            authority,
            host,
            planner: &planner,
            confirm: &mut confirm,
            log,
        }
        .handle_as(entry, &request);
        let rendered = outcome.render();
        // A `cd` changes where the session is, which the header says.
        let _ = events.send(Event::Header(header(&session.cwd_text(host))));
        // Leaving at a question ends the request as a refusal: no reply is
        // worth a call to the model for an interface that is closing.
        let speaking = voice
            .as_ref()
            .filter(|_| entry == Entry::Request && !confirm.gone);
        let shown = match speaking {
            Some(_) => Event::OutcomeAwaitingReply(rendered),
            None => Event::Outcome(rendered),
        };
        // The result is on screen before the reply is asked for; the reply is only ever an addition.
        if events.send(shown).is_err() {
            return;
        }
        if let Some(voice) = speaking {
            let said = reply::reply_to(voice, &request, &outcome, log);
            if events.send(Event::Said(said)).is_err() {
                return;
            }
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
    let speaker = setup.speaker();
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
    serve(
        &authority,
        &host,
        planner.as_ref().map(|p| p as &dyn Planner),
        &mut log,
        wires,
        &header,
        speaker
            .as_ref()
            .map(|(speaker, persona)| Voice { speaker, persona }),
    );
}

/// Whether leaving may abandon what the worker is doing: only a wait for the
/// model, to plan or to write a reply, because nothing has run then.
fn abandons_when_leaving(mode: &Mode) -> bool {
    matches!(
        mode,
        Mode::Busy {
            label: "asking the model" | "saying",
            ..
        }
    )
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

/// The state a session opens with: the header, and the notice the person must
/// see first (a SOUL or a personality that could not be used) as the first note.
fn opening_state(header: String, entry: Entry, notice: Option<String>) -> State {
    let mut state = State::for_entry(header, entry);
    state.blocks.extend(notice.map(state::Block::Note));
    state
}

/// Open the interface and run until the person quits. The history is printed
/// to the normal screen on exit, and written to `transcript` when given.
pub fn run(setup: Setup, transcript: Option<&Path>) -> Result<(), String> {
    let state = opening_state(
        setup.header(&SessionState::start(&setup.host)),
        setup.entry(),
        setup.notice.clone(),
    );
    run_with(state, transcript, move |requests, events, replies| {
        worker(setup, requests, events, replies);
    })
}

/// The worker's `Dialog` for the SOUL interview: a line the person submits is the answer.
struct ChannelDialog {
    requests: Receiver<String>,
    events: Sender<Event>,
}

impl Dialog for ChannelDialog {
    fn ask(&mut self, text: &str) -> Option<String> {
        self.events.send(Event::Outcome(text.to_owned())).ok()?;
        self.requests.recv().ok()
    }

    fn waiting(&mut self) {
        let _ = self.events.send(Event::Busy("asking the model"));
    }

    fn writing(&mut self) {
        // Leaving while a step is "working" waits for it instead of abandoning it.
        let _ = self.events.send(Event::Busy("working"));
    }
}

/// Runs the SOUL interview for the interface (S9). It ends with a message,
/// and the worker then waits, because the interface owns the screen until the
/// person leaves.
fn serve_interview(mut task: interview::Task, requests: Receiver<String>, events: Sender<Event>) {
    let mut dialog = ChannelDialog { requests, events };
    let ended = match task.run(&mut dialog) {
        Ok(done) => done.message(),
        Err(why) => why,
    };
    let _ = dialog
        .events
        .send(Event::Outcome(format!("{ended}\n\nCtrl-D leaves.")));
    // A line typed after the end must not leave the interface waiting for an answer.
    while dialog.requests.recv().is_ok() {
        let _ = dialog.events.send(Event::Outcome(
            "The interview is over. Ctrl-D leaves.".to_owned(),
        ));
    }
}

/// A submitted line starts as "working", never as a wait for the model: the
/// worker says when it asks the model, and the write that ends the interview
/// must not be abandoned by leaving.
fn interview_state() -> State {
    State::for_entry("SOUL interview".to_owned(), Entry::Command)
}

/// The SOUL interview in the conversational interface (S9).
pub fn interview(task: interview::Task, transcript: Option<&Path>) -> Result<(), String> {
    run_with(
        interview_state(),
        transcript,
        move |requests, events, _replies| {
            serve_interview(task, requests, events);
        },
    )
}

fn run_with(
    mut state: State,
    transcript: Option<&Path>,
    work: impl FnOnce(Receiver<String>, Sender<Event>, Receiver<Reply>) + Send + 'static,
) -> Result<(), String> {
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
    let worker = thread::spawn(move || work(requests, event_tx, replies));
    let result = event_loop(&mut terminal, &mut state, &request_tx, &reply_tx, &events);

    // Questions sent but not yet read from the channel still count as asked.
    while let Ok(event) = events.try_recv() {
        state.on_event(event);
    }
    // Leaving at a question is a refusal, and the decision log must say so:
    // closing the channels lets the worker's wait fail and record it. A running
    // step is waited for rather than killed half way. Only a wait for the
    // model is abandoned, because nothing has run and the request is logged.
    let waiting_for_model = abandons_when_leaving(&state.mode);
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
    // To the terminal's own width: it wraps what is wider, and a wrapped row has no mark.
    let text = state.transcript_for(usize::from(state.width));
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
                gone: false,
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
            Some(&ScriptedPlanner::new(&[DELETE_LARGEST])),
            &mut log,
            Wires {
                requests,
                events: events_tx,
                replies,
            },
            &|cwd| format!("header in {cwd}"),
            None,
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
    fn with_no_model_a_line_is_a_command_and_the_planner_is_never_asked() {
        use crate::pipeline_tests::files;
        use crate::testing::{MemoryLog, user};

        let (request_tx, requests) = channel();
        let (events_tx, events) = channel();
        let (reply_tx, replies) = channel();
        request_tx
            .send("fs list --in downloads".to_owned())
            .unwrap();
        request_tx.send("delete the big files".to_owned()).unwrap();
        drop((request_tx, reply_tx));
        let mut log = MemoryLog::default();
        serve(
            &Authority::full(user(false)),
            &files(),
            None,
            &mut log,
            Wires {
                requests,
                events: events_tx,
                replies,
            },
            &|cwd| format!("header in {cwd}"),
            None,
        );
        let seen: Vec<Event> = events.try_iter().collect();
        assert!(
            seen.iter()
                .all(|e| !matches!(e, Event::Busy(label) if *label == "asking the model"))
        );
        assert_eq!(
            seen.iter()
                .filter(|e| matches!(e, Event::Busy("working")))
                .count(),
            2,
            "each typed line starts as work, not as a wait for a model"
        );
        let outcomes: Vec<&String> = seen
            .iter()
            .filter_map(|e| match e {
                Event::Outcome(text) => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(outcomes.len(), 2);
        assert!(outcomes[0].contains("-> done"), "{}", outcomes[0]);
        assert!(outcomes[1].contains("Not a command"), "{}", outcomes[1]);
        assert!(log.events.iter().all(|e| e["event"] != "model_output"));
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

    fn serve_with(
        speaker: Option<&dyn crate::ports::Speaker>,
        entry_request: &str,
        model: bool,
    ) -> (Vec<Event>, crate::testing::MemoryLog) {
        use crate::pipeline_tests::{LARGEST, files};
        use crate::testing::{MemoryLog, ScriptedPlanner, user};

        let persona = crate::soul::Persona::of(crate::soul::Profile::WarmDidactic);
        let (request_tx, requests) = channel();
        let (events_tx, events) = channel();
        let (reply_tx, replies) = channel();
        request_tx.send(entry_request.to_owned()).unwrap();
        drop((request_tx, reply_tx));
        let mut log = MemoryLog::default();
        let planner = ScriptedPlanner::new(&[LARGEST]);
        serve(
            &Authority::full(user(false)),
            &files(),
            model.then_some(&planner as &dyn Planner),
            &mut log,
            Wires {
                requests,
                events: events_tx,
                replies,
            },
            &|cwd| format!("header in {cwd}"),
            speaker.map(|speaker| Voice {
                speaker,
                persona: &persona,
            }),
        );
        (events.try_iter().collect(), log)
    }

    #[test]
    fn the_result_is_sent_first_and_the_reply_after_it_with_saying_in_between() {
        let speaker = crate::testing::ScriptedSpeaker::says("Found them.");
        let (seen, log) = serve_with(Some(&speaker), "largest", true);
        let at = |wanted: &dyn Fn(&Event) -> bool| seen.iter().position(wanted).unwrap();
        let outcome = at(&|e| matches!(e, Event::OutcomeAwaitingReply(_)));
        let said = at(&|e| matches!(e, Event::Said(Some(text)) if text == "Found them."));
        assert!(outcome < said, "{seen:?}");
        assert!(seen.iter().all(|e| !matches!(e, Event::Outcome(_))));
        assert_eq!(seen.len() - 1, said, "the reply is the last thing said");
        assert_eq!(log.events.last().unwrap()["event"], "reply");
    }

    #[test]
    fn without_a_reply_the_events_are_those_of_today_and_a_failed_one_only_ends_the_wait() {
        let (without, _) = serve_with(None, "largest", true);
        assert!(without.iter().all(|e| !matches!(e, Event::Said(_))));
        let failing = crate::testing::ScriptedSpeaker {
            answer: Err("timed out".to_owned()),
            asked: Default::default(),
        };
        let (failed, log) = serve_with(Some(&failing), "largest", true);
        let kept: Vec<Event> = failed
            .iter()
            .filter(|e| !matches!(e, Event::Said(None)))
            .map(|e| match e {
                Event::OutcomeAwaitingReply(text) => Event::Outcome(text.clone()),
                other => other.clone(),
            })
            .collect();
        assert_eq!(kept, without);
        assert!(failed.iter().any(|e| matches!(e, Event::Said(None))));
        assert_eq!(log.events.last().unwrap()["event"], "reply_failed");
    }

    #[test]
    fn with_no_model_there_is_no_reply_even_if_a_speaker_is_given() {
        let speaker = crate::testing::ScriptedSpeaker::says("never");
        let (seen, log) = serve_with(Some(&speaker), "fs list --in downloads", false);
        assert!(seen.iter().all(|e| !matches!(e, Event::Said(_))));
        assert!(speaker.asked.borrow().is_empty());
        assert!(log.events.iter().all(|e| e["event"] != "reply"));
    }

    #[test]
    fn leaving_abandons_a_wait_for_the_model_or_for_a_reply_but_never_a_running_step() {
        let busy = |label| Mode::Busy { label, since_ms: 0 };
        assert!(abandons_when_leaving(&busy("asking the model")));
        assert!(abandons_when_leaving(&busy("saying")));
        assert!(!abandons_when_leaving(&busy("working")));
        assert!(!abandons_when_leaving(&Mode::Input));
    }

    #[test]
    fn a_notice_is_the_first_thing_in_the_history_and_none_adds_nothing() {
        let with = opening_state(
            "h".to_owned(),
            Entry::Request,
            Some("bad profile".to_owned()),
        );
        assert_eq!(with.transcript_for(80), "bad profile\n");
        let without = opening_state("h".to_owned(), Entry::Request, None);
        assert!(without.blocks.is_empty());
    }

    #[test]
    fn leaving_at_a_question_does_not_wait_for_a_reply_to_the_refusal() {
        use crate::pipeline_tests::{DELETE_LARGEST, files};
        use crate::testing::{MemoryLog, ScriptedPlanner, ScriptedSpeaker, user};

        let persona = crate::soul::Persona::of(crate::soul::Profile::DirectTechnical);
        let (request_tx, requests) = channel();
        let (events_tx, events) = channel();
        let (reply_tx, replies) = channel();
        request_tx.send("largest".to_owned()).unwrap();
        drop((request_tx, reply_tx));
        let speaker = ScriptedSpeaker::says("never asked");
        let mut log = MemoryLog::default();
        serve(
            &Authority::full(user(false)),
            &files(),
            Some(&ScriptedPlanner::new(&[DELETE_LARGEST])),
            &mut log,
            Wires {
                requests,
                events: events_tx,
                replies,
            },
            &|cwd| format!("header in {cwd}"),
            Some(Voice {
                speaker: &speaker,
                persona: &persona,
            }),
        );
        assert!(speaker.asked.borrow().is_empty());
        let seen: Vec<Event> = events.try_iter().collect();
        assert!(seen.iter().any(|e| matches!(e, Event::Outcome(_))));
        assert!(
            seen.iter()
                .all(|e| !matches!(e, Event::OutcomeAwaitingReply(_) | Event::Said(_)))
        );
        assert!(log.events.iter().all(|e| e["event"] != "reply"));
    }

    #[test]
    fn the_interview_takes_each_submitted_line_as_an_answer_and_ends_with_a_message() {
        let dir =
            std::env::temp_dir().join(format!("intent-shell-tui-interview-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (request_tx, requests) = channel();
        let (events_tx, events) = channel();
        // The last line is typed after the interview ended.
        for line in ["preset", "Dex", "2", "y", "anything"] {
            request_tx.send(line.to_owned()).unwrap();
        }
        drop(request_tx);
        serve_interview(
            interview::Task {
                model: None,
                soul: (dir.join("soul"), true),
                administrator: true,
                log: crate::log::JsonlLog::open(&dir.join("log.jsonl")).unwrap(),
            },
            requests,
            events_tx,
        );
        // Leaving while the file is written waits for it, so it is announced as work.
        let all: Vec<Event> = events.try_iter().collect();
        assert_eq!(all[4], Event::Busy("working"), "{all:?}");
        let shown: Vec<String> = all
            .into_iter()
            .filter_map(|event| match event {
                Event::Outcome(text) => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(
            shown.len(),
            6,
            "four questions, the ending and the answer to a late line: {shown:?}"
        );
        assert!(shown[5].contains("The interview is over"), "{}", shown[5]);
        assert!(
            shown[0].contains("How do you want to set up the SOUL"),
            "{}",
            shown[0]
        );
        assert!(
            shown[4].contains("was written") && shown[4].contains("Ctrl-D leaves."),
            "{}",
            shown[4]
        );
        let file = std::fs::read_to_string(dir.join("soul")).unwrap();
        assert_eq!(
            file,
            "format: 1\npreset: warm-didactic\nname: Dex\nlanguages: any\n"
        );
    }

    #[test]
    fn a_refused_interview_says_why_in_the_interface_and_waits_for_the_person_to_leave() {
        let dir =
            std::env::temp_dir().join(format!("intent-shell-tui-refused-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (request_tx, requests) = channel();
        let (events_tx, events) = channel();
        drop(request_tx);
        serve_interview(
            interview::Task {
                model: None,
                soul: (dir.join("soul"), true),
                administrator: false,
                log: crate::log::JsonlLog::open(&dir.join("log.jsonl")).unwrap(),
            },
            requests,
            events_tx,
        );
        let shown: Vec<Event> = events.try_iter().collect();
        assert!(
            matches!(&shown[..], [Event::Outcome(text)] if text.contains("root authority")),
            "{shown:?}"
        );
        assert!(!dir.join("soul").exists());
    }

    #[test]
    fn a_line_submitted_in_the_interview_is_never_a_wait_that_leaving_abandons() {
        let mut state = interview_state();
        for code in [KeyCode::Char('y'), KeyCode::Enter] {
            state.on_key(KeyEvent::new(code, KeyModifiers::NONE));
        }
        assert!(matches!(state.mode, Mode::Busy { .. }), "{:?}", state.mode);
        assert!(!abandons_when_leaving(&state.mode), "{:?}", state.mode);
    }
}
