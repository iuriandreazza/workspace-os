//! `intent-shell`: type a request, see the plan the broker would run, confirm.
//!
//! Needs `llama-server` running Bonsai 8B (docs/s3-intent-shell-prototype.md).

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;

use intent_shell::action::printable;
use intent_shell::broker::Authority;
use intent_shell::host::UnixHost;
use intent_shell::log::JsonlLog;
use intent_shell::model::LlamaPlanner;
use intent_shell::ports::{Confirm, User};
use intent_shell::screen::{self, Header};
use intent_shell::session::Shell;

const USAGE: &str = "usage: intent-shell [--model HOST:PORT] [--log FILE] [--administrator]";

struct Options {
    model: String,
    log: PathBuf,
    administrator: bool,
}

fn parse_options(home: &str) -> Result<Options, String> {
    let mut options = Options {
        model: "127.0.0.1:8089".to_owned(),
        log: PathBuf::from(home).join(".local/state/workspace-os/intent-shell/decisions.jsonl"),
        administrator: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--model" => options.model = args.next().ok_or(USAGE)?,
            "--log" => options.log = PathBuf::from(args.next().ok_or(USAGE)?),
            "--administrator" => options.administrator = true,
            _ => return Err(USAGE.to_owned()),
        }
    }
    Ok(options)
}

struct TerminalConfirm;

fn read_line() -> Option<String> {
    let _ = io::stdout().flush();
    let mut line = String::new();
    match io::stdin().read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => {
            echo_if_scripted(&line);
            Some(line)
        }
    }
}

/// A terminal shows what is typed; a script's input would not appear anywhere,
/// and a transcript has to show it.
fn echo_if_scripted(line: &str) {
    if !io::stdin().is_terminal() {
        println!("{}", printable(line.trim_end(), false));
    }
}

/// Only an explicit yes counts: an empty line, end of input or anything else is no.
fn is_yes(answer: &str) -> bool {
    matches!(answer.trim(), "y" | "Y" | "yes")
}

fn is_details(answer: &str) -> bool {
    matches!(answer.trim(), "d" | "D" | "details")
}

/// The one-based number the person typed, as an index into `count` options.
fn parse_choice(answer: &str, count: usize) -> Option<usize> {
    let number: usize = answer.trim().parse().ok()?;
    (1..=count).contains(&number).then(|| number - 1)
}

impl Confirm for TerminalConfirm {
    fn plan(&mut self, screen: &str, details: &str) -> bool {
        println!("{screen}");
        loop {
            print!("  [y] run the steps marked ok   [d] details   [n] cancel > ");
            let Some(answer) = read_line() else {
                return false;
            };
            if is_details(&answer) {
                println!("{details}");
            } else {
                return is_yes(&answer);
            }
        }
    }

    fn root_action(&mut self, panel: &str, _who: &User) -> bool {
        println!("{panel}");
        print!("  [y] confirm this exact action   [n] refuse > ");
        read_line().is_some_and(|answer| is_yes(&answer))
    }

    fn choose(&mut self, question: &str, options: &[String]) -> Option<usize> {
        println!("\n  {question}");
        for (index, option) in options.iter().enumerate() {
            println!("   {}. {option}", index + 1);
        }
        loop {
            print!("  number, or Enter to cancel > ");
            let answer = read_line()?;
            if answer.trim().is_empty() {
                return None;
            }
            match parse_choice(&answer, options.len()) {
                Some(index) => return Some(index),
                None => println!("  not one of the options"),
            }
        }
    }
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
    let host = UnixHost::from_environment();
    let planner = LlamaPlanner {
        address: options.model.clone(),
    };
    let mut log = JsonlLog::open(&options.log).map_err(|e| {
        format!(
            "cannot open the decision log {}: {e}",
            options.log.display()
        )
    })?;
    let user = User {
        name: std::env::var("USER").unwrap_or_else(|_| "unknown".to_owned()),
        administrator: options.administrator,
    };
    let authority = Authority::full(user);
    let mut confirm = TerminalConfirm;

    println!(
        "{}",
        screen::header(&Header {
            user: &authority.user.name,
            administrator: authority.user.administrator,
            model: &options.model,
            log: &options
                .log
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        })
    );
    loop {
        print!("intent> ");
        let _ = io::stdout().flush();
        let mut request = String::new();
        match io::stdin().read_line(&mut request) {
            Ok(0) => return Ok(()),
            Ok(_) => echo_if_scripted(&request),
            // One bad paste must not end the session.
            Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                println!("  that line is not valid text; try again");
                continue;
            }
            Err(e) => return Err(e.to_string()),
        }
        let request = request.trim();
        if request.is_empty() {
            continue;
        }
        let mut shell = Shell {
            authority: &authority,
            host: &host,
            planner: &planner,
            confirm: &mut confirm,
            log: &mut log,
        };
        println!("{}", shell.handle(request).render());
    }
}

#[cfg(test)]
mod tests {
    use super::{is_details, is_yes, parse_choice};

    #[test]
    fn details_are_asked_for_with_d() {
        for yes in ["d", "D\n", "details"] {
            assert!(is_details(yes), "{yes:?}");
        }
        for no in ["", "y", "dd", "n"] {
            assert!(!is_details(no), "{no:?}");
        }
    }

    #[test]
    fn a_choice_is_a_number_inside_the_list() {
        assert_eq!(parse_choice("2\n", 3), Some(1));
        for bad in ["", "0", "4", "x", "-1", " "] {
            assert_eq!(parse_choice(bad, 3), None, "{bad:?}");
        }
    }

    #[test]
    fn only_an_explicit_yes_confirms() {
        for yes in ["y\n", "Y", "yes\n"] {
            assert!(is_yes(yes), "{yes:?}");
        }
        for no in ["", "\n", "n\n", "yep", "ye", " \n"] {
            assert!(!is_yes(no), "{no:?}");
        }
    }
}
