//! `intent-shell`: type a request, see the plan the broker would run, confirm.
//!
//! Needs `llama-server` running Bonsai 8B (docs/s3-intent-shell-prototype.md),
//! or `--no-model` for the deterministic shell, where lines are commands (ADR 0022).
//! On a terminal it opens the conversational interface; with `--plain`, with
//! no terminal, or on a `dumb` one it uses the line-by-line adapter (ADR 0015).

use std::io::IsTerminal;
use std::path::PathBuf;

use serde_json::json;

use intent_shell::audio;
use intent_shell::broker::Authority;
use intent_shell::host::UnixHost;
use intent_shell::interview;
use intent_shell::log::JsonlLog;
use intent_shell::model::LlamaPlanner;
use intent_shell::plain;
use intent_shell::ports::{DecisionLog, Synthesizer as _, User};
use intent_shell::setup::{Setup, SpeechSetup};
use intent_shell::soul::{self, Persona, Source};
use intent_shell::soul_files::FileSoulStore;
use intent_shell::speech::{Ear, Mouth, Speaking, reason};
use intent_shell::speech_http::{PiperClient, WhisperClient};

const USAGE: &str = "usage: intent-shell [--model HOST:PORT | --no-model] [--log FILE] [--administrator] [--plain] [--transcript FILE] [--no-reply] [--soul FILE] [--personality FILE] [--setup-soul] [--listen] [--speak] [--whisper 127.0.0.1:PORT] [--tts 127.0.0.1:PORT]";

/// Where `whisper-server` and Piper are expected, unless told otherwise. The
/// docs start them on these ports; both are loopback addresses (ADR 0031).
const WHISPER_ADDRESS: &str = "127.0.0.1:8178";
const TTS_ADDRESS: &str = "127.0.0.1:5178";

struct Options {
    /// `None` is the deterministic shell: no model, lines are commands (ADR 0022).
    model: Option<String>,
    log: PathBuf,
    administrator: bool,
    plain: bool,
    transcript: Option<PathBuf>,
    /// No replies from the model after a result (ADR 0021).
    no_reply: bool,
    /// The SOUL file and the personality file, and whether the person named each.
    soul: (PathBuf, bool),
    personality: (PathBuf, bool),
    /// Build the SOUL with the interview or a preset instead of starting a session (S9).
    setup_soul: bool,
    /// Push-to-talk (`--listen`) and spoken replies (`--speak`), ADR 0031.
    listen: bool,
    speak: bool,
    /// Where the speech servers are, when the person said; `None` is the default.
    whisper: Option<String>,
    tts: Option<String>,
}

/// The value of a flag. One that looks like a flag is not taken for it, so
/// `--log --no-model` cannot swallow the flag that picks the deterministic shell.
fn value(args: &mut impl Iterator<Item = String>) -> Result<String, String> {
    args.next()
        .filter(|value| !value.starts_with("--"))
        .ok_or_else(|| USAGE.to_owned())
}

fn parse_options(home: &str, args: impl Iterator<Item = String>) -> Result<Options, String> {
    let config = PathBuf::from(home).join(".config/workspace-os");
    let mut options = Options {
        model: Some("127.0.0.1:8089".to_owned()),
        log: PathBuf::from(home).join(".local/state/workspace-os/intent-shell/decisions.jsonl"),
        administrator: false,
        plain: false,
        transcript: None,
        no_reply: false,
        soul: (config.join("soul"), false),
        personality: (config.join("personality"), false),
        setup_soul: false,
        listen: false,
        speak: false,
        whisper: None,
        tts: None,
    };
    let mut args = args;
    let (mut chose_model, mut chose_none) = (false, false);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--model" => {
                options.model = Some(value(&mut args)?);
                chose_model = true;
            }
            "--no-model" => {
                options.model = None;
                chose_none = true;
            }
            "--log" => options.log = PathBuf::from(value(&mut args)?),
            "--administrator" => options.administrator = true,
            "--plain" => options.plain = true,
            "--no-reply" => options.no_reply = true,
            // The S8a name of the SOUL file, kept so nothing that used it breaks (ADR 0028).
            "--soul" | "--voice-profile" => options.soul = (PathBuf::from(value(&mut args)?), true),
            "--personality" => options.personality = (PathBuf::from(value(&mut args)?), true),
            "--setup-soul" => options.setup_soul = true,
            "--listen" => options.listen = true,
            "--speak" => options.speak = true,
            "--whisper" => options.whisper = Some(value(&mut args)?),
            "--tts" => options.tts = Some(value(&mut args)?),
            "--transcript" => options.transcript = Some(PathBuf::from(value(&mut args)?)),
            _ => return Err(USAGE.to_owned()),
        }
    }
    if chose_model && chose_none {
        return Err(format!(
            "--model and --no-model exclude each other\n{USAGE}"
        ));
    }
    check_speech_options(&options)?;
    Ok(options)
}

/// What speech may be combined with. A speech flag that could not do anything
/// is refused, never silently ignored.
fn check_speech_options(options: &Options) -> Result<(), String> {
    if !(options.listen || options.speak) {
        if options.whisper.is_some() || options.tts.is_some() {
            return Err("--whisper and --tts need --listen or --speak".to_owned());
        }
        return Ok(());
    }
    if options.model.is_none() {
        return Err("--listen and --speak need a model: with none, lines are commands and there is no reply to speak (ADR 0031)".to_owned());
    }
    if options.speak && options.no_reply {
        return Err("--speak speaks the model's reply, and --no-reply turns it off".to_owned());
    }
    if options.setup_soul {
        return Err("--listen and --speak do not apply to --setup-soul".to_owned());
    }
    if options.whisper.is_some() && !options.listen {
        return Err("--whisper needs --listen".to_owned());
    }
    if options.tts.is_some() && !options.speak {
        return Err("--tts needs --speak".to_owned());
    }
    Ok(())
}

/// A terminal that can draw a screen: not a pipe, and not `dumb` or unset.
fn can_draw_on(stdin_tty: bool, stdout_tty: bool, term: &str) -> bool {
    stdin_tty && stdout_tty && !term.is_empty() && term != "dumb"
}

fn can_draw() -> bool {
    can_draw_on(
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
        &std::env::var("TERM").unwrap_or_default(),
    )
}

fn main() {
    if let Err(message) = run() {
        eprintln!("{message}");
        std::process::exit(1);
    }
}

/// The SOUL file: the one named, else `soul`, else the S8a `voice-profile`
/// beside it when that is what exists (ADR 0028).
fn soul_path(options: &Options) -> PathBuf {
    let (path, named) = &options.soul;
    let legacy = soul::legacy_path(path);
    // A dangling link is a `soul` that is there and broken, not an absent one.
    if *named || path.symlink_metadata().is_ok() || !legacy.exists() {
        path.clone()
    } else {
        legacy
    }
}

/// The persona of the replies, or `None` when there are none: turned off, or
/// no model to write them. A SOUL or a personality that cannot be used is said
/// once, on the error output, and logged (ADR 0021, ADR 0028). Neither file is
/// read when there are no replies.
fn voice_of(
    options: &Options,
    log: &mut dyn DecisionLog,
) -> Result<(Option<Persona>, Option<String>), String> {
    if options.no_reply || options.model.is_none() {
        return Ok((None, None));
    }
    let soul_file = soul_path(options);
    let loaded = soul::load(
        (&soul_file, options.soul.1),
        (&options.personality.0, options.personality.1),
    )?;
    let mut problems = Vec::new();
    for (event, source) in [
        ("voice_profile", &loaded.soul),
        ("personality", &loaded.personality),
    ] {
        let (how, problem) = match source {
            Source::Default => ("default", None),
            Source::File => ("file", None),
            Source::DefaultAfterProblem(problem) => ("default_after_problem", Some(problem)),
        };
        problems.extend(problem.cloned());
        let persona = if event == "voice_profile" {
            &loaded.soul_persona
        } else {
            &loaded.persona
        };
        let mut recorded = persona.describe();
        recorded["event"] = json!(event);
        recorded["source"] = json!(how);
        recorded["problem"] = json!(problem);
        log.record(recorded)
            .map_err(|e| format!("cannot write the decision log: {e}"))?;
    }
    for problem in &problems {
        eprintln!("{problem}");
    }
    let notice = (!problems.is_empty()).then(|| problems.join("\n"));
    Ok((Some(loaded.persona), notice))
}

/// Speech, when asked for (ADR 0031). The addresses must be on this machine,
/// or the shell does not start. Everything else that can go wrong (a server not
/// running yet, a voice not installed, a system with no microphone adapter) is
/// said once and leaves the shell running without that half of speech.
fn speech_setup(
    options: &Options,
    persona: Option<&Persona>,
    log: &mut dyn DecisionLog,
) -> Result<(Option<SpeechSetup>, Vec<String>), String> {
    if !(options.listen || options.speak) {
        return Ok((None, Vec::new()));
    }
    let mut problems = Vec::new();
    let speaking = Speaking::default();
    // The recogniser expects the language of the voice that answers: it is the
    // language the person and the shell speak to each other in.
    let language = persona.map(|persona| persona.spoken_voice().language.to_owned());
    let mut ear = None;
    if options.listen {
        let client = WhisperClient::new(options.whisper.as_deref().unwrap_or(WHISPER_ADDRESS))?;
        match audio::platform_capture() {
            Ok(capture) => {
                if let Err(why) = client.ping() {
                    problems.push(format!("--listen: {}; start whisper-server, the microphone key says so until it answers", reason(&why)));
                }
                ear = Some(Ear::new(capture, Box::new(client), speaking.clone()));
            }
            Err(why) => problems.push(format!("--listen is off: {why}")),
        }
    }
    let mut mouth = None;
    let mut voice = None;
    if options.speak {
        let client = PiperClient::new(options.tts.as_deref().unwrap_or(TTS_ADDRESS))?;
        let chosen = persona
            .ok_or("--speak needs the persona of the replies")?
            .spoken_voice();
        voice = Some(chosen.id);
        match audio::platform_playback() {
            Ok(playback) => {
                match client.voices() {
                    Err(why) => problems.push(format!(
                        "--speak: the speech server at {} does not answer: {}; replies are spoken once it does",
                        options.tts.as_deref().unwrap_or(TTS_ADDRESS),
                        reason(&why)
                    )),
                    Ok(installed) if !installed.iter().any(|name| name == chosen.piper) => {
                        problems.push(format!(
                            "--speak: the voice {} ({}) is not installed on the speech server",
                            chosen.id, chosen.piper
                        ));
                    }
                    Ok(_) => {}
                }
                mouth = Some(Mouth::new(Box::new(client), playback, chosen));
            }
            Err(why) => problems.push(format!("--speak is off: {why}")),
        }
    }
    log.record(json!({"event": "speech", "listen": ear.is_some(), "speak": mouth.is_some(), "voice": voice, "problems": problems}))
        .map_err(|e| format!("cannot write the decision log: {e}"))?;
    let setup = (ear.is_some() || mouth.is_some()).then(|| SpeechSetup {
        ear,
        listen_language: language,
        mouth,
        speaking,
    });
    Ok((setup, problems))
}

/// The SOUL interview, in the interface the session would have used (S9).
fn setup_soul(options: Options, log: JsonlLog) -> Result<(), String> {
    let task = interview::Task {
        model: options.model,
        soul: options.soul,
        administrator: options.administrator,
        log,
    };
    task.gate()?;
    #[cfg(feature = "tui")]
    if !options.plain && can_draw() {
        return intent_shell::tui::interview(task, options.transcript.as_deref(), "Ctrl-D leaves.");
    }
    if options.transcript.is_some() {
        return Err("--transcript needs the conversational interface".to_owned());
    }
    plain::interview(task)
}

/// A shell that starts on a machine with no SOUL builds one first when someone
/// can answer and has the authority, and otherwise says who can (ADR 0032).
/// What goes wrong here never stops the session: it starts with the default.
fn first_soul(options: &Options) -> Result<Option<String>, String> {
    let notice = first_soul_notice(options)?;
    // Like the other problems with the SOUL: said once, and kept for the screen.
    if let Some(notice) = &notice {
        eprintln!("{notice}");
    }
    Ok(notice)
}

fn first_soul_notice(options: &Options) -> Result<Option<String>, String> {
    // A named personality means the person is using the shell: it is checked against
    // the SOUL there is, and an error about it is not to wait for an interview (ADR 0032).
    if options.personality.1 {
        return Ok(None);
    }
    let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let replies = !options.no_reply && options.model.is_some();
    Ok(
        match interview::first_run(
            (&soul_path(options), options.soul.1),
            replies,
            interactive,
            options.administrator,
        ) {
            interview::FirstRun::No => None,
            interview::FirstRun::Notice => Some(interview::NO_SOUL_NOTICE.to_owned()),
            interview::FirstRun::Interview => {
                let task = interview::Task {
                    model: options.model.clone(),
                    soul: options.soul.clone(),
                    administrator: true,
                    log: JsonlLog::open(&options.log)
                        .map_err(|e| format!("cannot open the decision log: {e}"))?,
                };
                #[cfg(feature = "tui")]
                if !options.plain && can_draw() {
                    let ended =
                        intent_shell::tui::interview(task, None, "Ctrl-D starts the shell.");
                    // A closed terminal ends the shell: there is no screen left for the session.
                    if intent_shell::tui::terminated() {
                        return Err("interrupted".to_owned());
                    }
                    return Ok(ended.err());
                }
                plain::interview(task).err()
            }
        },
    )
}

fn run() -> Result<(), String> {
    let home = std::env::var("HOME").map_err(|_| "HOME is not set")?;
    let options = parse_options(&home, std::env::args().skip(1))?;
    let mut log = JsonlLog::open(&options.log).map_err(|e| {
        format!(
            "cannot open the decision log {}: {e}",
            options.log.display()
        )
    })?;
    if options.setup_soul {
        return setup_soul(options, log);
    }
    // Before the interview: a screen is needed for a transcript, and nothing may be asked first.
    let screen = cfg!(feature = "tui") && !options.plain && can_draw();
    if options.transcript.is_some() && !screen {
        return Err("--transcript needs the conversational interface".to_owned());
    }
    if options.listen && !screen {
        return Err("--listen needs the conversational interface: a terminal, without --plain (the transcript goes into its input line)".to_owned());
    }
    let first = first_soul(&options)?;
    let (reply, notice) = voice_of(&options, &mut log)?;
    let (speech, speech_problems) = speech_setup(&options, reply.as_ref(), &mut log)?;
    for problem in &speech_problems {
        eprintln!("{problem}");
    }
    let notice = [
        notice,
        (!speech_problems.is_empty()).then(|| speech_problems.join("\n")),
    ]
    .into_iter()
    .flatten()
    .reduce(|a, b| format!("{a}\n{b}"));
    let notice = match (first, notice) {
        (Some(first), Some(notice)) => Some(format!("{first}\n{notice}")),
        (first, notice) => first.or(notice),
    };
    let setup = Setup {
        authority: Authority::full(User {
            name: std::env::var("USER").unwrap_or_else(|_| "unknown".to_owned()),
            administrator: options.administrator,
        }),
        host: UnixHost::from_environment().with_soul_store(FileSoulStore::new(
            soul_path(&options),
            options.personality.0.clone(),
        )),
        planner: options.model.map(|address| LlamaPlanner { address }),
        log,
        reply,
        notice,
        log_name: options
            .log
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        speech,
    };
    #[cfg(feature = "tui")]
    if screen {
        return intent_shell::tui::run(setup, options.transcript.as_deref());
    }
    plain::run(setup)
}

#[cfg(test)]
mod tests {
    use super::{can_draw_on, parse_options};

    fn options(args: &[&str]) -> Result<super::Options, String> {
        parse_options("/home/x", args.iter().map(|a| (*a).to_owned()))
    }

    #[test]
    fn no_model_means_no_model_address_and_cannot_be_mixed_with_one() {
        assert!(options(&[]).unwrap().model.is_some());
        assert_eq!(options(&["--no-model"]).unwrap().model, None);
        assert_eq!(
            options(&["--model", "h:1"]).unwrap().model.as_deref(),
            Some("h:1")
        );
        assert!(options(&["--model", "h:1", "--no-model"]).is_err());
        assert!(options(&["--no-model", "--model", "h:1"]).is_err());
        assert!(options(&["--no-model", "--bogus"]).is_err());
        // A flag is never taken for the value of the one before it.
        for args in [
            ["--plain", "--log", "--no-model"],
            ["--plain", "--model", "--no-model"],
            ["--plain", "--transcript", "--no-model"],
        ] {
            assert!(options(&args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn replies_are_on_by_default_and_the_soul_and_personality_default_to_the_config_directory() {
        let on = options(&[]).unwrap();
        assert!(!on.no_reply);
        let config = std::path::PathBuf::from("/home/x/.config/workspace-os");
        assert_eq!(on.soul, (config.join("soul"), false));
        assert_eq!(on.personality, (config.join("personality"), false));
        assert!(options(&["--no-reply"]).unwrap().no_reply);
        for flag in ["--soul", "--voice-profile"] {
            let named = options(&[flag, "/p"]).unwrap();
            assert_eq!(named.soul, (std::path::PathBuf::from("/p"), true));
        }
        let named = options(&["--personality", "/q"]).unwrap();
        assert_eq!(named.personality, (std::path::PathBuf::from("/q"), true));
        assert!(options(&["--soul", "--no-reply"]).is_err());
        assert!(options(&["--personality", "--no-reply"]).is_err());
    }

    #[test]
    fn the_interface_needs_a_real_terminal_on_both_ends_and_a_capable_term() {
        assert!(can_draw_on(true, true, "xterm-256color"));
        for (stdin, stdout, term) in [
            (false, true, "xterm"),
            (true, false, "xterm"),
            (true, true, "dumb"),
            (true, true, ""),
        ] {
            assert!(
                !can_draw_on(stdin, stdout, term),
                "{stdin} {stdout} {term:?}"
            );
        }
    }

    #[test]
    fn speech_is_off_by_default_and_asks_for_nothing() {
        let off = options(&[]).unwrap();
        assert!(!off.listen && !off.speak && off.whisper.is_none() && off.tts.is_none());
        struct NoLog;
        impl intent_shell::ports::DecisionLog for NoLog {
            fn record(&mut self, _: serde_json::Value) -> Result<(), String> {
                Err("the log must not be written".to_owned())
            }
        }
        // No flag: nothing is built, no process is contacted, nothing is logged.
        let (setup, problems) = super::speech_setup(&off, None, &mut NoLog).unwrap();
        assert!(setup.is_none() && problems.is_empty());
    }

    #[test]
    fn speech_flags_that_could_do_nothing_are_refused_not_ignored() {
        let on = options(&[
            "--listen",
            "--speak",
            "--whisper",
            "127.0.0.1:1",
            "--tts",
            "[::1]:2",
        ])
        .unwrap();
        assert!(on.listen && on.speak);
        assert_eq!(on.whisper.as_deref(), Some("127.0.0.1:1"));
        for args in [
            &["--no-model", "--listen"][..],
            &["--no-model", "--speak"],
            &["--speak", "--no-reply"],
            &["--listen", "--setup-soul"],
            &["--whisper", "127.0.0.1:1"],
            &["--tts", "127.0.0.1:1"],
            &["--speak", "--whisper", "127.0.0.1:1"],
            &["--listen", "--tts", "127.0.0.1:1"],
            &["--listen", "--whisper"],
        ] {
            assert!(options(args).is_err(), "{args:?}");
        }
        // Listening alone does not need replies: the transcript is only a line to edit.
        assert!(options(&["--listen", "--no-reply"]).is_ok());
    }

    #[test]
    fn an_address_that_is_not_on_this_machine_stops_the_shell_before_it_starts() {
        struct Sink;
        impl intent_shell::ports::DecisionLog for Sink {
            fn record(&mut self, _: serde_json::Value) -> Result<(), String> {
                Ok(())
            }
        }
        let persona = intent_shell::soul::Persona::of(intent_shell::soul::Profile::DirectTechnical);
        for args in [
            &["--speak", "--tts", "example.com:5178"][..],
            &["--listen", "--whisper", "192.168.1.9:8178"],
            &["--speak", "--tts", "localhost:5178"],
        ] {
            let chosen = options(args).unwrap();
            let error = super::speech_setup(&chosen, Some(&persona), &mut Sink)
                .err()
                .unwrap_or_else(|| panic!("{args:?} was accepted"));
            assert!(
                error.contains("machine") || error.contains("IP address"),
                "{error}"
            );
        }
    }
}
