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
use std::sync::{Arc, Mutex};
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
use crate::setup::{Setup, SpeechSetup};
use crate::speech::{Ear, SharedLog, Talker};
use state::{Effect, Event, Listen, Mode, Reply, State};

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
    /// Speaks each reply aloud, when `--speak` was given (ADR 0031).
    talker: Option<Talker>,
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
        talker,
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
            if events.send(said_event(talker.as_ref(), said)).is_err() {
                return;
            }
        }
    }
}

/// The event that shows a reply and gives the keyboard back, made only once the
/// reply is queued to be spoken: the microphone is shut from then on, so the
/// person cannot start recording in the gap before the voice begins (ADR 0031,
/// section 5, point 4). There is no way to have the event without the queueing.
fn said_event(talker: Option<&Talker>, said: Option<String>) -> Event {
    if let (Some(text), Some(talker)) = (&said, talker) {
        talker.say(text.clone());
    }
    Event::Said(said)
}

/// Start the threads of speech: the one that speaks, when there is a voice, and
/// the microphone's, when there is an ear. Both send through one closable sink,
/// so neither keeps the event channel open once the worker has ended: drop the
/// returned `Closing` when it has.
fn start_speech(
    speech: SpeechSetup,
    shared: SharedLog,
    events: &Sender<Event>,
    listens: Receiver<Listen>,
) -> (Option<Talker>, Closing) {
    let sink: Sink = Arc::new(Mutex::new(Some(events.clone())));
    let closing = Closing(Arc::clone(&sink));
    let talker = speech.mouth.map(|mouth| {
        let notes = Arc::clone(&sink);
        Talker::spawn(mouth, move |why| {
            send_through(&notes, Event::Note(format!("▸ {why}")));
        })
    });
    if let Some(ear) = speech.ear {
        let language = speech.listen_language;
        thread::spawn(move || listen(ear, language.as_deref(), &listens, &sink, shared));
    }
    (talker, closing)
}

fn worker(
    setup: Setup,
    requests: Receiver<String>,
    events: Sender<Event>,
    replies: Receiver<Reply>,
    listens: Receiver<Listen>,
) {
    let header = setup.header_fn();
    let speaker = setup.speaker();
    let Setup {
        authority,
        host,
        planner,
        log,
        speech,
        ..
    } = setup;
    // The decision log is shared with the microphone's thread only when speech
    // is on: with it off nothing here is wrapped, started or contacted.
    let (mut log, shared): (Box<dyn DecisionLog>, Option<SharedLog>) = if speech.is_some() {
        let shared = SharedLog::new(Box::new(log));
        (Box::new(shared.clone()), Some(shared))
    } else {
        (Box::new(log), None)
    };
    let (talker, closing) = match (speech, shared) {
        (Some(speech), Some(shared)) => {
            let (talker, closing) = start_speech(speech, shared, &events, listens);
            (talker, Some(closing))
        }
        _ => (None, None),
    };
    let wires = Wires {
        requests,
        events,
        replies,
        talker,
    };
    serve(
        &authority,
        &host,
        planner.as_ref().map(|p| p as &dyn Planner),
        &mut *log,
        wires,
        &header,
        speaker
            .as_ref()
            .map(|(speaker, persona)| Voice { speaker, persona }),
    );
    drop(closing);
}

/// Where the microphone's thread sends what it heard. The worker closes it when
/// it ends, by returning or by a panic, so that the interface sees the end of
/// the worker as it always did: the thread keeps no sender of the event channel alive.
type Sink = Arc<Mutex<Option<Sender<Event>>>>;

/// Send an event if the sink is still open; whether it was sent.
fn send_through(sink: &Sink, event: Event) -> bool {
    sink.lock()
        .ok()
        .and_then(|open| open.as_ref().map(|events| events.send(event).is_ok()))
        .unwrap_or(false)
}

struct Closing(Sink);

impl Drop for Closing {
    fn drop(&mut self) {
        if let Ok(mut sink) = self.0.lock() {
            *sink = None;
        }
    }
}

/// The microphone's thread: records on the key, and puts what the recogniser
/// heard on record before the interface shows it.
fn listen(
    mut ear: Ear,
    language: Option<&str>,
    commands: &Receiver<Listen>,
    sink: &Sink,
    mut log: SharedLog,
) {
    // The recording the microphone is held for; a command or an answer for any
    // other one is stale and does nothing.
    let mut current = 0;
    for command in commands {
        let (recording, heard) = match command {
            Listen::Start(recording) => {
                current = recording;
                match ear.start() {
                    Ok(()) => continue,
                    Err(why) => (recording, Err(why)),
                }
            }
            Listen::Cancel(recording) => {
                if recording == current {
                    ear.cancel();
                }
                continue;
            }
            Listen::Forget => {
                log.forget();
                continue;
            }
            Listen::Stop(recording) if recording != current => continue,
            Listen::Stop(recording) => (
                recording,
                ear.finish(language).and_then(|line| {
                    log.record(SharedLog::transcript(&line))
                        .map_err(|e| format!("cannot write the decision log: {e}"))?;
                    Ok(line)
                }),
            ),
        };
        if !send_through(sink, Event::Heard(recording, heard)) {
            return;
        }
    }
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

/// Whether a termination signal (a closed terminal, `kill`) arrived: the next screen must not be opened.
pub fn terminated() -> bool {
    TERMINATIONS.load(Ordering::Relaxed) > 0
}

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
    (requests, replies, listens): (&Sender<String>, &Sender<Reply>, &Sender<Listen>),
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
                Effect::Listen(command) => {
                    let _ = listens.send(command);
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
    let mut state = opening_state(
        setup.header(&SessionState::start(&setup.host)),
        setup.entry(),
        setup.notice.clone(),
    );
    if setup
        .speech
        .as_ref()
        .is_some_and(|speech| speech.ear.is_some())
    {
        state = state.with_listening();
    }
    run_with(
        state,
        transcript,
        move |requests, events, replies, listens| {
            worker(setup, requests, events, replies, listens);
        },
    )
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
fn serve_interview(
    mut task: interview::Task,
    requests: Receiver<String>,
    events: Sender<Event>,
    leave: &str,
) {
    let mut dialog = ChannelDialog { requests, events };
    let ended = match task.run(&mut dialog) {
        Ok(done) => done.message(),
        Err(why) => why,
    };
    let _ = dialog
        .events
        .send(Event::Outcome(format!("{ended}\n\n{leave}")));
    // A line typed after the end must not leave the interface waiting for an answer.
    while dialog.requests.recv().is_ok() {
        let _ = dialog
            .events
            .send(Event::Outcome(format!("The interview is over. {leave}")));
    }
}

/// A submitted line starts as "working", never as a wait for the model: the
/// worker says when it asks the model, and the write that ends the interview
/// must not be abandoned by leaving.
fn interview_state() -> State {
    State::for_entry("SOUL interview".to_owned(), Entry::Command)
}

/// The SOUL interview in the conversational interface (S9).
/// `leave` tells the person how to go on: `Ctrl-D leaves.`, or what follows.
pub fn interview(
    task: interview::Task,
    transcript: Option<&Path>,
    leave: &'static str,
) -> Result<(), String> {
    run_with(
        interview_state(),
        transcript,
        move |requests, events, _replies, _listens| {
            serve_interview(task, requests, events, leave);
        },
    )
}

fn run_with(
    mut state: State,
    transcript: Option<&Path>,
    work: impl FnOnce(Receiver<String>, Sender<Event>, Receiver<Reply>, Receiver<Listen>)
    + Send
    + 'static,
) -> Result<(), String> {
    let (request_tx, requests) = channel();
    let (event_tx, events) = channel();
    let (reply_tx, replies) = channel();
    let (listen_tx, listens) = channel();

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
    let worker = thread::spawn(move || work(requests, event_tx, replies, listens));
    let result = event_loop(
        &mut terminal,
        &mut state,
        (&request_tx, &reply_tx, &listen_tx),
        &events,
    );

    // Questions sent but not yet read from the channel still count as asked.
    while let Ok(event) = events.try_recv() {
        state.on_event(event);
    }
    // Leaving at a question is a refusal, and the decision log must say so:
    // closing the channels lets the worker's wait fail and record it. A running
    // step is waited for rather than killed half way. Only a wait for the
    // model is abandoned, because nothing has run and the request is logged.
    let waiting_for_model = abandons_when_leaving(&state.mode);
    drop((request_tx, reply_tx, listen_tx));
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
                talker: None,
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
                talker: None,
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
                talker: None,
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
                talker: None,
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
            "Ctrl-D leaves.",
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
            "Ctrl-D leaves.",
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

    // ---- speech: a spoken request through the same path as a typed one -------

    use crate::speech::loopback::{
        FakeCapture, FakePlayback, FakeSynthesizer, FakeTranscriber, HeldPlayback, SharedMemory,
        tagged_wav,
    };
    use crate::speech::{Mouth, Speaking, voice};

    struct Talking {
        playback: FakePlayback,
        synthesizer: FakeSynthesizer,
        log: SharedMemory,
        events: Vec<Event>,
        speaking: Speaking,
    }

    /// Hear `heard` through the microphone's thread, press Enter on what the
    /// person is shown, and run the request that comes out of it.
    fn spoken_request(
        heard: &str,
        plan: &str,
        administrator: bool,
        answers: &[Reply],
        spoken: Option<&str>,
    ) -> Talking {
        use crate::pipeline_tests::files;
        use crate::testing::{ScriptedPlanner, ScriptedSpeaker, user};

        let log = SharedMemory::default();
        let shared = SharedLog::new(Box::new(log.clone()));
        let speaking = Speaking::default();
        let (listen_tx, listens) = channel();
        let (events_tx, events) = channel();
        let ear = Ear::new(
            Box::new(FakeCapture::default()),
            Box::new(FakeTranscriber::hearing(heard)),
            speaking.clone(),
        );
        let listener = {
            let sink: Sink = Arc::new(Mutex::new(Some(events_tx.clone())));
            let shared = shared.clone();
            thread::spawn(move || listen(ear, Some("pt-BR"), &listens, &sink, shared))
        };
        let mut state = State::new("header".to_owned()).with_listening();
        state.clock_ms = 0;
        assert_eq!(state.on_key(ctrl_t()), Effect::Listen(Listen::Start(1)));
        listen_tx.send(Listen::Start(state.listen_gen)).unwrap();
        state.clock_ms += state::DWELL_MS;
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(state.on_key(enter), Effect::Listen(Listen::Stop(1)));
        listen_tx.send(Listen::Stop(state.listen_gen)).unwrap();
        let shown = events.recv_timeout(Duration::from_secs(10)).unwrap();
        state.on_event(shown);
        // A person reads what was heard before pressing Enter.
        state.clock_ms += state::DWELL_MS;
        // Heard: it waits in the input line until the person presses Enter.
        let Effect::Submit(request) = state.on_key(enter) else {
            panic!("Enter did not send what was heard");
        };
        drop((listen_tx, events_tx));
        listener.join().unwrap();

        let playback = FakePlayback::default();
        let synthesizer = FakeSynthesizer::with(&["pt_BR-cadu-medium"]);
        let mouth = Mouth::new(
            Box::new(synthesizer.clone()),
            Box::new(playback.clone()),
            voice("cadu").unwrap(),
            speaking.clone(),
        );
        let talker = Talker::spawn(mouth, |_| {});
        let persona = crate::soul::Persona::of(crate::soul::Profile::WarmDidactic);
        let words = ScriptedSpeaker::says(spoken.unwrap_or("Pronto."));
        let (request_tx, requests) = channel();
        let (events_tx, events) = channel();
        let (reply_tx, replies) = channel();
        request_tx.send(request).unwrap();
        for answer in answers {
            reply_tx.send(*answer).unwrap();
        }
        drop(request_tx);
        let mut shared_log = shared;
        serve(
            &Authority::full(user(administrator)),
            &files(),
            Some(&ScriptedPlanner::new(&[plan])),
            &mut shared_log,
            Wires {
                requests,
                events: events_tx,
                replies,
                talker: Some(talker),
            },
            &|cwd| format!("header in {cwd}"),
            Some(Voice {
                speaker: &words,
                persona: &persona,
            }),
        );
        drop(reply_tx);
        for _ in 0..500 {
            if !speaking.active() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        Talking {
            playback,
            synthesizer,
            log,
            events: events.try_iter().collect(),
            speaking,
        }
    }

    #[test]
    fn the_microphones_thread_keeps_no_event_sender_alive_once_the_worker_has_ended() {
        let (events_tx, events) = channel();
        let (_listen_tx, listens) = channel();
        let sink: Sink = Arc::new(Mutex::new(Some(events_tx)));
        let closing = Closing(Arc::clone(&sink));
        let ear = Ear::new(
            Box::new(FakeCapture::default()),
            Box::new(FakeTranscriber::hearing("x")),
            Speaking::default(),
        );
        let shared = SharedLog::new(Box::new(SharedMemory::default()));
        // The thread is alive and waiting for the key, as with `--listen`.
        thread::spawn(move || listen(ear, None, &listens, &sink, shared));
        assert!(matches!(
            events.recv_timeout(Duration::from_millis(100)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        drop(closing);
        assert!(matches!(
            events.recv_timeout(Duration::from_secs(5)),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
        ));
    }

    #[test]
    fn the_event_that_gives_the_keyboard_back_exists_only_once_the_reply_is_queued() {
        let (started_tx, started) = channel();
        let (release_tx, release) = channel();
        let speaking = Speaking::default();
        let mouth = Mouth::new(
            Box::new(FakeSynthesizer::with(&["pt_BR-cadu-medium"])),
            Box::new(HeldPlayback::new(started_tx, release)),
            voice("cadu").unwrap(),
            speaking.clone(),
        );
        let talker = Talker::spawn(mouth, |_| {});
        assert!(!speaking.active());
        let event = said_event(Some(&talker), Some("Done.".to_owned()));
        // The gate is shut as soon as the event can be sent, before the voice has begun.
        assert!(speaking.active());
        assert_eq!(event, Event::Said(Some("Done.".to_owned())));
        // Without a reply, or without a voice, nothing is queued.
        assert_eq!(said_event(Some(&talker), None), Event::Said(None));
        assert_eq!(
            said_event(None, Some("x".to_owned())),
            Event::Said(Some("x".to_owned()))
        );
        started.recv_timeout(Duration::from_secs(10)).unwrap();
        release_tx.send(()).unwrap();
        for _ in 0..300 {
            if !speaking.active() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(!speaking.active());
    }

    #[test]
    fn the_speech_the_worker_starts_speaks_replies_and_listens_in_the_replies_language() {
        let (playback, transcriber) = (FakePlayback::default(), FakeTranscriber::hearing("ola"));
        let speaking = Speaking::default();
        let setup = SpeechSetup {
            ear: Some(Ear::new(
                Box::new(FakeCapture::default()),
                Box::new(transcriber.clone()),
                speaking.clone(),
            )),
            listen_language: Some("pt-BR".to_owned()),
            mouth: Some(Mouth::new(
                Box::new(FakeSynthesizer::with(&["pt_BR-jeff-medium"])),
                Box::new(playback.clone()),
                voice("jeff").unwrap(),
                speaking.clone(),
            )),
        };
        let (events_tx, events) = channel();
        let (listen_tx, listens) = channel();
        let shared = SharedLog::new(Box::new(SharedMemory::default()));
        let (talker, closing) = start_speech(setup, shared, &events_tx, listens);
        // The voice: a reply is spoken in the voice the mouth was built with.
        talker
            .expect("a mouth gives a talker")
            .say("Pronto.".to_owned());
        for _ in 0..300 {
            if !speaking.active() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            playback.played.lock().unwrap().as_slice(),
            [tagged_wav("pt_BR-jeff-medium", "Pronto.")]
        );
        // The ear: the microphone's thread answers a recording, told the language.
        listen_tx.send(Listen::Start(1)).unwrap();
        listen_tx.send(Listen::Stop(1)).unwrap();
        let heard = events.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(heard, Event::Heard(1, Ok("ola".to_owned())));
        assert_eq!(
            transcriber.languages.lock().unwrap().as_slice(),
            [Some("pt-BR".to_owned())]
        );
        // And both let go of the event channel with the worker.
        drop((closing, events_tx, listen_tx));
        assert!(matches!(
            events.recv_timeout(Duration::from_secs(5)),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
        ));
    }

    #[test]
    fn what_the_microphones_thread_is_told_to_forget_no_longer_marks_a_request() {
        let memory = SharedMemory::default();
        let shared = SharedLog::new(Box::new(memory.clone()));
        let ear = Ear::new(
            Box::new(FakeCapture::default()),
            Box::new(FakeTranscriber::hearing("delete the folder")),
            Speaking::default(),
        );
        let (events_tx, events) = channel();
        let (listen_tx, listens) = channel();
        let sink: Sink = Arc::new(Mutex::new(Some(events_tx)));
        let mut log = shared.clone();
        let thread = thread::spawn({
            let shared = shared.clone();
            move || listen(ear, None, &listens, &sink, shared)
        });
        for command in [Listen::Start(1), Listen::Stop(1), Listen::Forget] {
            listen_tx.send(command).unwrap();
        }
        assert!(events.recv_timeout(Duration::from_secs(5)).is_ok());
        drop(listen_tx);
        thread.join().unwrap();
        log.record(serde_json::json!({"event": "request", "request": "delete the folder"}))
            .unwrap();
        let request = memory
            .events()
            .into_iter()
            .find(|e| e["event"] == "request")
            .unwrap();
        assert!(request.get("input").is_none(), "{request}");
    }

    #[test]
    fn nothing_is_sent_through_a_closed_sink_and_the_sender_is_dropped() {
        let (tx, events) = channel();
        let sink: Sink = Arc::new(Mutex::new(Some(tx)));
        assert!(send_through(&sink, Event::Note("a".to_owned())));
        assert_eq!(events.try_recv(), Ok(Event::Note("a".to_owned())));
        drop(Closing(Arc::clone(&sink)));
        assert!(!send_through(&sink, Event::Note("b".to_owned())));
        assert!(matches!(
            events.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn the_microphones_thread_ignores_a_stop_or_a_cancel_for_an_earlier_recording() {
        let capture = FakeCapture::default();
        let ear = Ear::new(
            Box::new(capture.clone()),
            Box::new(FakeTranscriber::hearing("second")),
            Speaking::default(),
        );
        let (events_tx, events) = channel();
        let (listen_tx, listens) = channel();
        let sink: Sink = Arc::new(Mutex::new(Some(events_tx)));
        let shared = SharedLog::new(Box::new(SharedMemory::default()));
        let thread = thread::spawn(move || listen(ear, None, &listens, &sink, shared));
        for command in [
            Listen::Start(1),
            Listen::Cancel(1),
            Listen::Start(2),
            Listen::Cancel(1),
            Listen::Stop(1),
            Listen::Stop(2),
        ] {
            listen_tx.send(command).unwrap();
        }
        let heard = events.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(heard, Event::Heard(2, Ok("second".to_owned())));
        drop(listen_tx);
        thread.join().unwrap();
        // One stale stop and one stale cancel did nothing: no extra transcription and no extra cancel.
        assert_eq!(
            capture.state.lock().unwrap().as_slice(),
            ["start", "cancel", "start", "finish"]
        );
        assert!(events.try_recv().is_err());
    }

    fn ctrl_t() -> KeyEvent {
        KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL)
    }

    #[test]
    fn a_spoken_request_becomes_a_validated_plan_and_the_answer_is_spoken_back() {
        use crate::pipeline_tests::LARGEST;

        let run = spoken_request(
            "list the largest files in downloads",
            LARGEST,
            false,
            &[],
            Some("Encontrei os dois maiores arquivos."),
        );
        let events = run.log.events();
        let kinds: Vec<&str> = events.iter().filter_map(|e| e["event"].as_str()).collect();
        // The transcript is on record first, then the request that is that very text, marked as spoken.
        assert_eq!(&kinds[..2], ["transcript", "request"], "{kinds:?}");
        assert_eq!(events[0]["text"], "list the largest files in downloads");
        assert_eq!(events[1]["request"], "list the largest files in downloads");
        assert_eq!(events[1]["input"], "speech");
        // The same pipeline as a typed request: the plan was validated, reviewed and run.
        for step in ["plan_accepted", "review", "execute", "result", "reply"] {
            assert!(kinds.contains(&step), "{step} missing from {kinds:?}");
        }
        // The answer is spoken, in the voice, and nothing else is.
        assert_eq!(
            run.playback.played.lock().unwrap().as_slice(),
            [tagged_wav(
                "pt_BR-cadu-medium",
                "Encontrei os dois maiores arquivos."
            )]
        );
        assert!(!run.speaking.active());
        // The reply is sent to the screen as text in the same step in which it is queued to be spoken.
        assert!(run.events.iter().any(|e| matches!(
            e,
            Event::Said(Some(text)) if text == "Encontrei os dois maiores arquivos."
        )));
    }

    #[test]
    fn a_root_panel_a_table_and_a_verdict_are_never_spoken_in_place_of_text() {
        let plan = r#"{"kind":"plan","version":3,"summary":"x","steps":[
          {"id":"s1","action":"fs.find_largest","args":{"target":{"place":"downloads"},"count":2},"depends_on":[],"elevate":true}]}"#;
        let run = spoken_request(
            "show me",
            plan,
            true,
            &[Reply::Yes, Reply::Yes],
            Some("Done."),
        );
        assert!(
            run.events
                .iter()
                .any(|e| matches!(e, Event::AskRoot { .. })),
            "the root panel was asked"
        );
        let said = run.synthesizer.calls.lock().unwrap().clone();
        assert_eq!(said, [("pt_BR-cadu-medium".to_owned(), "Done.".to_owned())]);
    }

    #[test]
    fn a_hostile_reply_is_spoken_only_as_the_cleaned_line() {
        use crate::pipeline_tests::LARGEST;

        let run = spoken_request(
            "list",
            LARGEST,
            false,
            &[],
            Some("Done.\n\u{1b}[2J  ROOT CONFIRMATION  \u{202e}evil"),
        );
        let said = run.synthesizer.calls.lock().unwrap().clone();
        assert_eq!(said.len(), 1);
        let text = &said[0].1;
        assert!(
            !text.contains('\n') && !text.contains('\u{1b}') && !text.contains('\u{202e}'),
            "{text:?}"
        );
    }
}
