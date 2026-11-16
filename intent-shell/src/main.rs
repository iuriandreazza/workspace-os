//! `intent-shell`: type a request, see the plan the broker would run, confirm.
//!
//! Needs `llama-server` running Bonsai 8B (docs/s3-intent-shell-prototype.md).
//! On a terminal it opens the conversational interface; with `--plain`, with
//! no terminal, or on a `dumb` one it uses the line-by-line adapter (ADR 0015).

use std::io::IsTerminal;
use std::path::PathBuf;

use intent_shell::broker::Authority;
use intent_shell::host::UnixHost;
use intent_shell::log::JsonlLog;
use intent_shell::model::LlamaPlanner;
use intent_shell::plain;
use intent_shell::ports::User;
use intent_shell::setup::Setup;

const USAGE: &str = "usage: intent-shell [--model HOST:PORT] [--log FILE] [--administrator] [--plain] [--transcript FILE]";

struct Options {
    model: String,
    log: PathBuf,
    administrator: bool,
    plain: bool,
    transcript: Option<PathBuf>,
}

fn parse_options(home: &str) -> Result<Options, String> {
    let mut options = Options {
        model: "127.0.0.1:8089".to_owned(),
        log: PathBuf::from(home).join(".local/state/workspace-os/intent-shell/decisions.jsonl"),
        administrator: false,
        plain: false,
        transcript: None,
    };
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--model" => options.model = args.next().ok_or(USAGE)?,
            "--log" => options.log = PathBuf::from(args.next().ok_or(USAGE)?),
            "--administrator" => options.administrator = true,
            "--plain" => options.plain = true,
            "--transcript" => options.transcript = Some(PathBuf::from(args.next().ok_or(USAGE)?)),
            _ => return Err(USAGE.to_owned()),
        }
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

fn run() -> Result<(), String> {
    let home = std::env::var("HOME").map_err(|_| "HOME is not set")?;
    let options = parse_options(&home)?;
    let log = JsonlLog::open(&options.log).map_err(|e| {
        format!(
            "cannot open the decision log {}: {e}",
            options.log.display()
        )
    })?;
    let setup = Setup {
        authority: Authority::full(User {
            name: std::env::var("USER").unwrap_or_else(|_| "unknown".to_owned()),
            administrator: options.administrator,
        }),
        host: UnixHost::from_environment(),
        planner: LlamaPlanner {
            address: options.model.clone(),
        },
        log,
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
    use super::can_draw_on;

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
