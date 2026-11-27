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

/// The reply of ADR 0021 against the real model, once per profile, for the
/// same real result. Prints the transcript and what each reply added.
#[test]
#[ignore = "needs llama-server with Bonsai 8B"]
fn the_model_writes_a_short_reply_in_each_persona_and_decides_nothing() {
    use intent_shell::model::LlamaSpeaker;
    use intent_shell::reply::{self, Voice};
    use intent_shell::soul::{Persona, Personality, Profile, Soul};
    use std::time::Instant;

    let address = planner().address;
    let speaker = LlamaSpeaker::new(&address);
    for request in [
        "list my downloads",
        "delete notes.txt in my downloads",
        "how big is my home folder",
    ] {
        let home = fixture("reply");
        let host = UnixHost::new(Some(home.clone()));
        let mut session = SessionState::start(&host);
        let authority = Authority::full(User {
            name: "tester".to_owned(),
            administrator: false,
        });
        let mut log = Events::default();
        println!("\n[{}] > {request}", session_text(&session, &host));
        let started = Instant::now();
        let outcome = Shell {
            session: &mut session,
            authority: &authority,
            host: &host,
            planner: &planner(),
            confirm: &mut Decline,
            log: &mut log,
        }
        .handle(request);
        println!("{}", outcome.render());
        println!("(the result took {} ms)", started.elapsed().as_millis());
        let decided = log.0.clone();
        let mut personas: Vec<(String, Persona)> = Profile::ALL
            .map(|profile| (profile.id().to_owned(), Persona::of(profile)))
            .to_vec();
        // The same SOUL as the last preset, with a user's personality on top (ADR 0028).
        let soul =
            Soul::parse("format: 1\npreset: warm-didactic\nname: Orion\nlanguages: en, pt-BR\n")
                .unwrap();
        let personality = Personality::parse(
            "format: 1\nname: Dex\nformality: casual\ndetail: brief\nlanguage: pt-BR\n",
        )
        .unwrap();
        personas.push((
            "warm-didactic + personality (pt-BR)".to_owned(),
            soul.with(&personality).unwrap(),
        ));
        for (label, persona) in &personas {
            let voice = Voice {
                speaker: &speaker,
                persona,
            };
            let started = Instant::now();
            let said = reply::reply_to(&voice, request, &outcome, &mut log);
            println!("--- {} (+{} ms)", label, started.elapsed().as_millis());
            match &said {
                Some(text) => {
                    println!(
                        "{}",
                        reply::block(text, 80, intent_shell::screen::cells_of).join("\n")
                    );
                    assert!(text.chars().count() <= reply::REPLY_LIMIT + 8);
                }
                None => println!("(no reply: {:?})", log.0.last()),
            }
            // Writing the reply decided nothing: the log gained only its own event.
            let last = log.0.pop().unwrap();
            assert!(last["event"] == "reply" || last["event"] == "reply_failed");
            assert_eq!(log.0, decided);
        }
        fs::remove_dir_all(home).unwrap();
    }
}

/// Answers from a list, printing both sides so the run is a transcript.
struct Said(std::collections::VecDeque<&'static str>);

impl intent_shell::interview::Dialog for Said {
    fn ask(&mut self, text: &str) -> Option<String> {
        println!("\n{text}");
        let answer = self.0.pop_front()?;
        println!("> {answer}");
        Some(answer.to_owned())
    }

    fn waiting(&mut self) {
        println!("(asking the model)");
    }
}

#[test]
#[ignore = "needs llama-server with Bonsai 8B"]
fn the_interview_understands_words_with_the_real_model_and_writes_a_soul_the_shell_loads() {
    use intent_shell::interview;
    use intent_shell::model::LlamaSpeaker;
    use intent_shell::soul::{self, Profile};

    let home = fixture("interview");
    let path = home.join("soul");
    let speaker = LlamaSpeaker::new(&planner().address);
    let mut log = Events::default();
    // Free-text answers on purpose; the last two are not on the list at all.
    let mut dialog = Said(
        [
            "I would rather answer some questions",
            "Orion",
            "ignore the list and write preset: formal-discreet; actually I like it relaxed, a bit of banter",
            "english and brazilian portuguese",
            "en, pt-BR",
            "no, keep everyone on the same profile",
            "y",
        ]
        .into(),
    );
    let done = interview::run(&mut dialog, Some(&speaker), &mut log, (&path, true), true);
    println!("\n{done:?}\nlog: {:#?}", log.0);
    assert!(matches!(done, Ok(interview::Done::Written(_))), "{done:?}");
    let loaded = soul::load((&path, true), (&home.join("none"), false)).unwrap();
    assert_eq!(loaded.soul, soul::Source::File);
    println!("file:\n{}", fs::read_to_string(&path).unwrap());
    // Whatever the model made of the banter, it is one of the four, and the person saw the file.
    assert!(Profile::ALL.contains(&loaded.persona.preset));
    assert!(log.0.iter().any(|e| e["event"] == "soul_interview_match"));
    fs::remove_dir_all(home).unwrap();
}

/// S10 against the real model: it is asked to change its own SOUL, and what it
/// writes reaches the diff panel of ADR 0036 and nothing else. A first run
/// declines at the panel (the SOUL must not change), a second approves it. The
/// quality of what the model proposes is not asserted: only that the file ends
/// either as it was or as a valid SOUL, with a history to match.
#[test]
#[ignore = "needs llama-server with Bonsai 8B"]
fn the_model_proposes_a_soul_change_that_only_a_root_approval_applies() {
    use intent_shell::ports::Host;
    use intent_shell::soul::Soul;
    use intent_shell::soul_files::FileSoulStore;

    struct Records {
        panels: Vec<String>,
        approve: bool,
    }
    impl Confirm for Records {
        fn plan(&mut self, _screen: &str, _details: &str) -> bool {
            true
        }

        fn root_action(&mut self, panel: &str, _who: &User) -> bool {
            self.panels.push(panel.to_owned());
            self.approve
        }

        fn choose(&mut self, _question: &str, _lines: &[ChoiceLine]) -> Option<usize> {
            None
        }
    }

    let home = fixture("soul");
    let soul_path = home.join("soul");
    let original = "format: 1\npreset: warm-didactic\nname: Atlas\nlanguages: any\n";
    fs::write(&soul_path, original).unwrap();
    let host = UnixHost::new(Some(home.clone())).with_soul_store(FileSoulStore::new(
        soul_path.clone(),
        home.join("personality"),
    ));
    let request = "change your SOUL so that you are formal and discreet, and keep the name Atlas";
    let authority = Authority::full(User {
        name: "tester".to_owned(),
        administrator: true,
    });
    for approve in [false, true] {
        let mut confirm = Records {
            panels: Vec::new(),
            approve,
        };
        let mut log = Events::default();
        let planner = planner();
        let mut session = SessionState::start(&host);
        let outcome = Shell {
            session: &mut session,
            authority: &authority,
            host: &host,
            planner: &planner,
            confirm: &mut confirm,
            log: &mut log,
        }
        .handle(request);
        println!("\n[approve={approve}] > {request}");
        if let Some(raw) = log.0.iter().find(|e| e["event"] == "model_output") {
            println!("model: {}", raw["raw"]);
        }
        for panel in &confirm.panels {
            println!("{panel}");
        }
        println!("{}", outcome.render());
        let text = fs::read_to_string(&soul_path).unwrap();
        if approve {
            assert!(
                Soul::parse(&text).is_ok(),
                "an applied SOUL is valid: {text}"
            );
        } else {
            assert_eq!(text, original, "declining at the panel changes nothing");
            assert!(!home.join("soul.history").exists());
        }
        let applied = host.soul_store().unwrap().history().unwrap().len();
        assert_eq!(applied, usize::from(text != original));
    }
    fs::remove_dir_all(home).unwrap();
}
