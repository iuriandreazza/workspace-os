//! `intent-shell`: type a request, see the plan the broker would run, confirm.
//!
//! Needs `llama-server` running Bonsai 8B (docs/s3-intent-shell-prototype.md),
//! or `--no-model` for the deterministic shell, where lines are commands (ADR 0022).
//! On a terminal it opens the conversational interface; with `--plain`, with
//! no terminal, or on a `dumb` one it uses the line-by-line adapter (ADR 0015).

use std::io::IsTerminal;
use std::path::PathBuf;

use serde_json::json;

use intent_shell::broker::Authority;
use intent_shell::host::UnixHost;
use intent_shell::log::JsonlLog;
use intent_shell::model::LlamaPlanner;
use intent_shell::plain;
use intent_shell::ports::{DecisionLog, User};
use intent_shell::reply::{Profile, Source, load_profile};
use intent_shell::setup::Setup;

const USAGE: &str = "usage: intent-shell [--model HOST:PORT | --no-model] [--log FILE] [--administrator] [--plain] [--transcript FILE] [--no-reply] [--voice-profile FILE]";

struct Options {
    /// `None` is the deterministic shell: no model, lines are commands (ADR 0022).
    model: Option<String>,
    log: PathBuf,
    administrator: bool,
    plain: bool,
    transcript: Option<PathBuf>,
    /// No replies from the model after a result (ADR 0021).
    no_reply: bool,
    /// The voice profile file, and whether the person named it.
    voice_profile: (PathBuf, bool),
}

/// The value of a flag. One that looks like a flag is not taken for it, so
/// `--log --no-model` cannot swallow the flag that picks the deterministic shell.
fn value(args: &mut impl Iterator<Item = String>) -> Result<String, String> {
    args.next()
        .filter(|value| !value.starts_with("--"))
        .ok_or_else(|| USAGE.to_owned())
}

fn parse_options(home: &str, args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut options = Options {
        model: Some("127.0.0.1:8089".to_owned()),
        log: PathBuf::from(home).join(".local/state/workspace-os/intent-shell/decisions.jsonl"),
        administrator: false,
        plain: false,
        transcript: None,
        no_reply: false,
        voice_profile: (
            PathBuf::from(home).join(".config/workspace-os/voice-profile"),
            false,
        ),
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
            "--voice-profile" => options.voice_profile = (PathBuf::from(value(&mut args)?), true),
            "--transcript" => options.transcript = Some(PathBuf::from(value(&mut args)?)),
            _ => return Err(USAGE.to_owned()),
        }
    }
    if chose_model && chose_none {
        return Err(format!(
            "--model and --no-model exclude each other\n{USAGE}"
        ));
    }
    Ok(options)
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

/// The profile of the replies, or `None` when there are none: turned off, or
/// no model to write them. A profile file that cannot be used is said once, on
/// the error output, and the default is used (ADR 0021).
fn voice_of(options: &Options, log: &mut dyn DecisionLog) -> Result<Option<Profile>, String> {
    if options.no_reply || options.model.is_none() {
        return Ok(None);
    }
    let (profile, source) = load_profile(&options.voice_profile.0, options.voice_profile.1)?;
    let (how, problem) = match &source {
        Source::Default => ("default", None),
        Source::File => ("file", None),
        Source::DefaultAfterProblem(problem) => ("default_after_problem", Some(problem)),
    };
    if let Some(problem) = problem {
        eprintln!("{problem}");
    }
    log.record(json!({"event": "voice_profile", "profile": profile.id(), "source": how, "problem": problem}))
        .map_err(|e| format!("cannot write the decision log: {e}"))?;
    Ok(Some(profile))
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
    let reply = voice_of(&options, &mut log)?;
    let setup = Setup {
        authority: Authority::full(User {
            name: std::env::var("USER").unwrap_or_else(|_| "unknown".to_owned()),
            administrator: options.administrator,
        }),
        host: UnixHost::from_environment(),
        planner: options.model.map(|address| LlamaPlanner { address }),
        log,
        reply,
        log_name: options
            .log
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };
    #[cfg(feature = "tui")]
    if !options.plain && can_draw() {
        return intent_shell::tui::run(setup, options.transcript.as_deref());
    }
    #[cfg(not(feature = "tui"))]
    let _ = can_draw;
    if options.transcript.is_some() {
        return Err("--transcript needs the conversational interface".to_owned());
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
    fn replies_are_on_by_default_and_the_profile_file_defaults_to_the_config_directory() {
        let on = options(&[]).unwrap();
        assert!(!on.no_reply);
        assert_eq!(
            on.voice_profile,
            (
                std::path::PathBuf::from("/home/x/.config/workspace-os/voice-profile"),
                false
            )
        );
        assert!(options(&["--no-reply"]).unwrap().no_reply);
        let named = options(&["--voice-profile", "/p"]).unwrap();
        assert_eq!(named.voice_profile, (std::path::PathBuf::from("/p"), true));
        assert!(options(&["--voice-profile", "--no-reply"]).is_err());
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
}
