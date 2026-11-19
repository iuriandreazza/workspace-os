//! The line-by-line terminal adapter: no screen control, no dependency, no
//! terminal required. It is what scripts, transcripts, the deterministic shell
//! and the recovery mode use (ADR 0015); the conversational interface in `tui`
//! is the same shell with a different adapter.

use std::io::{self, IsTerminal, Write};

use crate::action::printable;
use crate::ports::{ChoiceLine, Confirm, User};
use crate::reply::{self, Voice};
use crate::screen::fit;
use crate::session::{Entry, SessionState, Shell};
use crate::setup::{Setup, planner_of};

struct TerminalConfirm {
    /// Set when the input ended at a question: the session is over, and there
    /// is no one to read a reply.
    input_ended: bool,
}

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

/// What the number takes in front of a line: `   NN. `.
const NUMBER_COLUMN: usize = 7;

/// The tree of a choice with a number on each candidate, and none on the
/// context around it.
fn numbered(lines: &[ChoiceLine], width: usize) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            let text = crate::screen::fit_choice(
                &line.text,
                width.saturating_sub(NUMBER_COLUMN),
                crate::screen::cells_of,
            );
            match line.option {
                Some(index) => format!("   {:>2}. {text}", index + 1),
                None => format!("       {text}"),
            }
        })
        .collect()
}

/// Columns to lay `text` out in, which is to be printed `indent` cells in: the
/// terminal's, or, when output is redirected and has no width, what keeps every
/// line whole.
fn width_for(text: &str, indent: usize) -> usize {
    if !io::stdout().is_terminal() {
        return crate::screen::natural_width(text) + indent;
    }
    terminal_width()
}

/// Where output is redirected there is no width to wrap to.
const REDIRECTED_REPLY_WIDTH: usize = 80;

/// Columns of the terminal; 80 when it does not say.
fn terminal_width() -> usize {
    let mut size = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: `size` is live for the call and is what `TIOCGWINSZ` fills in.
    let known = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut size) } == 0;
    if known && size.ws_col > 0 {
        usize::from(size.ws_col)
    } else {
        80
    }
}

/// The one-based number the person typed, as an index into `count` options.
fn parse_choice(answer: &str, count: usize) -> Option<usize> {
    let number: usize = answer.trim().parse().ok()?;
    (1..=count).contains(&number).then(|| number - 1)
}

impl TerminalConfirm {
    fn answer(&mut self) -> Option<String> {
        let line = read_line();
        self.input_ended |= line.is_none();
        line
    }
}

impl Confirm for TerminalConfirm {
    fn plan(&mut self, screen: &str, details: &str) -> bool {
        println!("{screen}");
        loop {
            print!("  [y] run the steps marked ok   [d] details   [n] cancel > ");
            let Some(answer) = self.answer() else {
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
        self.answer().is_some_and(|answer| is_yes(&answer))
    }

    fn choose(&mut self, question: &str, lines: &[ChoiceLine]) -> Option<usize> {
        println!("\n  {question}");
        let text = lines
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        for line in numbered(lines, width_for(&text, NUMBER_COLUMN)) {
            println!("{line}");
        }
        let options = lines.iter().filter(|l| l.option.is_some()).count();
        loop {
            print!("  number, or Enter to cancel > ");
            let answer = self.answer()?;
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

/// The prompt shows the directory, because the header is printed only once.
fn prompt(entry: Entry, cwd: &str) -> String {
    match entry {
        Entry::Command => format!("command {cwd}> "),
        Entry::Request => format!("intent {cwd}> "),
    }
}

/// The reply as the lines to print under the result: marked, and wrapped to the
/// terminal so that no continuation line is left unmarked.
fn reply_text(reply: &str, width: usize) -> String {
    reply::block(reply, width, reply::cautious_cells).join("\n")
}

pub fn run(mut setup: Setup) -> Result<(), String> {
    let speaker = setup.speaker();
    let voice = speaker.as_ref().map(|(speaker, profile)| Voice {
        speaker,
        profile: *profile,
    });
    let mut confirm = TerminalConfirm { input_ended: false };
    let mut session = SessionState::start(&setup.host);
    println!("{}", setup.header(&session));
    loop {
        print!("{}", prompt(setup.entry(), &session.cwd_text(&setup.host)));
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
        let entry = setup.entry();
        let mut shell = Shell {
            session: &mut session,
            authority: &setup.authority,
            host: &setup.host,
            planner: planner_of(setup.planner.as_ref()),
            confirm: &mut confirm,
            log: &mut setup.log,
        };
        let outcome = shell.handle_as(entry, request);
        let text = outcome.render();
        println!("{}", fit(&text, width_for(&text, 0)));
        // After the result is on screen: the reply is only ever an addition.
        if let Some(reply) = voice
            .as_ref()
            .filter(|_| !confirm.input_ended)
            .and_then(|voice| reply::reply_to(voice, request, &outcome, &mut setup.log))
        {
            let width = if io::stdout().is_terminal() {
                terminal_width()
            } else {
                REDIRECTED_REPLY_WIDTH
            };
            println!("{}", reply_text(&reply, width));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Entry, is_details, is_yes, numbered, parse_choice, prompt};
    use crate::ports::ChoiceLine;

    #[test]
    fn the_prompt_says_whether_a_model_is_listening_and_where_the_session_is() {
        assert_eq!(
            prompt(Entry::Request, "~/Documents"),
            "intent ~/Documents> "
        );
        assert_eq!(prompt(Entry::Command, "~"), "command ~> ");
    }

    #[test]
    fn only_the_candidates_of_a_choice_are_numbered() {
        let line = |text: &str, option| ChoiceLine {
            text: text.to_owned(),
            option,
            label: text.trim().to_owned(),
        };
        assert_eq!(
            numbered(
                &[
                    line(" ~    2 folders", None),
                    line(" ├─ Documents/", None),
                    line(" │  └─ report.pdf", Some(0)),
                    line(" └─ report.pdf", Some(1)),
                ],
                80
            ),
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
