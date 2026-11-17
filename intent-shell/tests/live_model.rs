//! Talks to a real `llama-server` running Bonsai 8B. Out of the gate: CI has no
//! model. Run it by hand, with the server up (docs/s3-intent-shell-prototype.md):
//!
//!     ./scripts/intent-shell-live-test.sh
//!
//! It checks the plumbing and the ADR 0018 vocabulary, not the quality of the
//! plans (that is S4): the constrained output reaches the broker, a path or a
//! relative path resolves against the session's directory, `cd` moves it, and
//! declining runs nothing. Everything happens in a throwaway home directory.

use std::fs;
use std::path::PathBuf;

use intent_shell::action::Action;
use intent_shell::broker::Authority;
use intent_shell::host::UnixHost;
use intent_shell::model::LlamaPlanner;
use intent_shell::ports::{ChoiceLine, Confirm, DecisionLog, User};
use intent_shell::session::{Outcome, SessionState, Shell};
use serde_json::Value;

struct Decline;

impl Confirm for Decline {
    fn plan(&mut self, _screen: &str, _details: &str) -> bool {
        false
    }

    fn root_action(&mut self, _panel: &str, _who: &User) -> bool {
        false
    }

    fn choose(&mut self, _question: &str, _lines: &[ChoiceLine]) -> Option<usize> {
        None
    }
}

#[derive(Default)]
struct Events(Vec<Value>);

impl DecisionLog for Events {
    fn record(&mut self, event: Value) -> Result<(), String> {
        self.0.push(event);
        Ok(())
    }
}

/// A home with a few folders and files, and nothing else.
fn fixture(name: &str) -> PathBuf {
    let home =
        std::env::temp_dir().join(format!("intent-shell-live-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&home);
    fs::create_dir_all(home.join("Downloads/films")).unwrap();
    fs::create_dir_all(home.join("Documents")).unwrap();
    fs::write(home.join("Downloads/films/old.mkv"), vec![0; 4000]).unwrap();
    fs::write(home.join("Downloads/films/new.mkv"), vec![0; 9000]).unwrap();
    fs::write(home.join("Downloads/notes.txt"), "hello").unwrap();
    fs::write(home.join("Documents/report.pdf"), "pdf").unwrap();
    fs::canonicalize(home).unwrap()
}

fn planner() -> LlamaPlanner {
    LlamaPlanner {
        address: std::env::var("INTENT_SHELL_MODEL")
            .unwrap_or_else(|_| "127.0.0.1:8089".to_owned()),
    }
}

/// One request, with the transcript printed (`--nocapture`) for the record.
fn ask(session: &mut SessionState, host: &UnixHost, request: &str) -> (Outcome, Vec<Value>) {
    let authority = Authority::full(User {
        name: "tester".to_owned(),
        administrator: false,
    });
    let mut log = Events::default();
    let planner = planner();
    println!("\n[{}] > {request}", session_text(session, host));
    let outcome = Shell {
        session,
        authority: &authority,
        host,
        planner: &planner,
        confirm: &mut Decline,
        log: &mut log,
    }
    .handle(request);
    if let Some(raw) = log.0.iter().find(|e| e["event"] == "model_output") {
        println!("model: {}", raw["raw"]);
    }
    println!("{}", outcome.render());
    (outcome, log.0)
}

fn session_text(session: &SessionState, host: &UnixHost) -> String {
    session.cwd_text(host)
}

fn first_action(outcome: &Outcome) -> &Action {
    match outcome {
        Outcome::Executed { review, .. } | Outcome::Declined(review) => {
            &review.steps[0].items[0].action
        }
        other => panic!("no plan was reviewed: {other:?}"),
    }
}

#[test]
#[ignore = "needs llama-server with Bonsai 8B"]
fn the_model_answers_through_the_broker_and_declining_runs_nothing() {
    let home = fixture("plumbing");
    let host = UnixHost::new(Some(home.clone()));
    let mut session = SessionState::start(&host);
    let (outcome, events) = ask(&mut session, &host, "delete notes.txt in my downloads");
    assert!(
        matches!(
            outcome,
            Outcome::Declined(_) | Outcome::Clarify(_) | Outcome::AllRefused(_)
        ),
        "a declined or unanswered request must not execute anything, got {outcome:?}"
    );
    let first_attempt = events
        .iter()
        .find(|e| e["event"] == "model_output")
        .expect("the model's output is logged");
    assert_eq!(
        first_attempt["accepted"], true,
        "grammar-constrained output should pass the broker's checks: {first_attempt}"
    );
    assert!(home.join("Downloads/notes.txt").exists());
    fs::remove_dir_all(home).unwrap();
}

#[test]
#[ignore = "needs llama-server with Bonsai 8B"]
fn a_path_and_a_relative_path_resolve_against_the_session_directory() {
    let home = fixture("paths");
    let host = UnixHost::new(Some(home.clone()));
    let mut session = SessionState::start(&host);

    let (outcome, _) = ask(&mut session, &host, "list the folder ~/Downloads/films");
    assert_eq!(
        first_action(&outcome),
        &Action::List {
            path: home.join("Downloads/films")
        }
    );

    session.cwd = home.join("Downloads/films");
    let (outcome, _) = ask(
        &mut session,
        &host,
        "show the details of old.mkv in this folder",
    );
    assert_eq!(
        first_action(&outcome),
        &Action::Stat {
            path: home.join("Downloads/films/old.mkv")
        }
    );
    fs::remove_dir_all(home).unwrap();
}

#[test]
#[ignore = "needs llama-server with Bonsai 8B"]
fn the_model_can_change_directory_and_the_next_request_starts_there() {
    let home = fixture("cd");
    let host = UnixHost::new(Some(home.clone()));
    let mut session = SessionState::start(&host);

    let (outcome, _) = ask(&mut session, &host, "go to the Documents folder");
    assert!(
        matches!(outcome, Outcome::Executed { asked: false, .. }),
        "{outcome:?}"
    );
    assert_eq!(session.cwd, home.join("Documents"));

    let (outcome, events) = ask(
        &mut session,
        &host,
        "list the files in the current directory",
    );
    assert_eq!(
        first_action(&outcome),
        &Action::List {
            path: home.join("Documents")
        }
    );
    assert_eq!(events[0]["cwd"], format!("{:?}", home.join("Documents")));
    fs::remove_dir_all(home).unwrap();
}
