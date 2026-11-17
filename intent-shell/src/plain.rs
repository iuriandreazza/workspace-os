//! The line-by-line terminal adapter: no screen control, no dependency, no
//! terminal required. It is what scripts, transcripts, the deterministic shell
//! and the recovery mode use (ADR 0015); the conversational interface in `tui`
//! is the same shell with a different adapter.

use std::io::{self, IsTerminal, Write};

use crate::action::printable;
use crate::ports::{ChoiceLine, Confirm, User};
use crate::session::{SessionState, Shell};
use crate::setup::Setup;

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

/// The tree of a choice with a number on each candidate, and none on the
/// context around it.
fn numbered(lines: &[ChoiceLine]) -> Vec<String> {
    lines
        .iter()
        .map(|line| match line.option {
            Some(index) => format!("   {:>2}. {}", index + 1, line.text),
            None => format!("       {}", line.text),
        })
        .collect()
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

    fn choose(&mut self, question: &str, lines: &[ChoiceLine]) -> Option<usize> {
        println!("\n  {question}");
        for line in numbered(lines) {
            println!("{line}");
        }
        let options = lines.iter().filter(|l| l.option.is_some()).count();
        loop {
            print!("  number, or Enter to cancel > ");
            let answer = read_line()?;
            if answer.trim().is_empty() {
                return None;
            }
            match parse_choice(&answer, options) {
                Some(index) => return Some(index),
                None => println!("  not one of the options"),
            }
        }
    }
}

pub fn run(mut setup: Setup) -> Result<(), String> {
    let mut confirm = TerminalConfirm;
    let mut session = SessionState::start(&setup.host);
    println!("{}", setup.header(&session));
    loop {
        print!("intent {}> ", session.cwd_text(&setup.host));
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
            session: &mut session,
            authority: &setup.authority,
            host: &setup.host,
            planner: &setup.planner,
            confirm: &mut confirm,
            log: &mut setup.log,
        };
        println!("{}", shell.handle(request).render());
    }
}

#[cfg(test)]
mod tests {
    use super::{is_details, is_yes, numbered, parse_choice};
    use crate::ports::ChoiceLine;

    #[test]
    fn only_the_candidates_of_a_choice_are_numbered() {
        let line = |text: &str, option| ChoiceLine {
            text: text.to_owned(),
            option,
            label: text.trim().to_owned(),
        };
        assert_eq!(
            numbered(&[
                line(" ~    2 folders", None),
                line(" ├─ Documents/", None),
                line(" │  └─ report.pdf", Some(0)),
                line(" └─ report.pdf", Some(1)),
            ]),
            [
                "        ~    2 folders",
                "        ├─ Documents/",
                "    1.  │  └─ report.pdf",
                "    2.  └─ report.pdf",
            ]
        );
    }

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
