//! What the person sees in the terminal (ADR 0014): a header, a table with one
//! line per concrete action and the broker's verdict beside it, notes, a
//! summary line, and a separate panel for each root action.
//!
//! Pure functions from the broker's data to text, so the screen is unit tested
//! and the terminal adapter only prints it. Nothing here shows a current
//! directory, and real locations appear only in the details view and in the
//! root panel, where ADR 0007 requires the exact action.

use crate::action::{Action, UserOperation, WriteMode, printable, quoted};
use crate::broker::{Review, ReviewedAction, ReviewedStep, Verdict};
use crate::label::Labels;
use crate::plan::{Origin, StepId};
use crate::resolve::Ambiguity;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// Friendly names only.
    Friendly,
    /// Friendly names, with the exact action under each line.
    Details,
}

pub struct Header<'a> {
    pub user: &'a str,
    pub administrator: bool,
    pub model: &'a str,
    /// The file name only: the directory is in the details, not on the screen.
    pub log: &'a str,
}

pub fn header(h: &Header) -> String {
    let rank = if h.administrator {
        " (administrator)"
    } else {
        ""
    };
    printable(
        &format!(
            " intent shell ──────────────────────────────────────────────\n  user  {}{rank}   model  {}   log  {}\n ───────────────────────────────────────────────────────────────",
            quoted(h.user),
            quoted(h.model),
            quoted(h.log),
        ),
        true,
    )
}

/// An action in words, with friendly names for what it touches.
pub fn say(action: &Action, labels: &Labels) -> String {
    let of = |p| labels.of(p);
    match action {
        Action::List { path } => format!("List {}", of(path)),
        Action::Stat { path } => format!("Show details of {}", of(path)),
        Action::Read { path } => format!("Read {}", of(path)),
        Action::Delete { path } => format!("Delete {}", of(path)),
        Action::FindLargest { path, count: 1 } => {
            format!("Find the largest file in {}", of(path))
        }
        Action::FindLargest { path, count } => {
            format!("Find the {count} largest files in {}", of(path))
        }
        Action::Write { path, mode, .. } => {
            let verb = match mode {
                WriteMode::Create => "Create",
                WriteMode::Overwrite => "Overwrite",
                WriteMode::Append => "Append to",
            };
            format!("{verb} {}", of(path))
        }
        Action::Move { from, to } => format!("Move {} to {}", of(from), of(to)),
        Action::Run { program, argv } => format!(
            "Run {} {}",
            quoted(program),
            argv.iter().map(|a| quoted(a)).collect::<Vec<_>>().join(" ")
        ),
        Action::UserManage {
            operation,
            user,
            group,
        } => {
            let verb = match operation {
                UserOperation::Add => "Add user",
                UserOperation::Remove => "Remove user",
                UserOperation::AddToGroup => "Add to a group the user",
                UserOperation::RemoveFromGroup => "Remove from a group the user",
            };
            let group = group
                .as_deref()
                .map(|g| format!(", group {}", quoted(g)))
                .unwrap_or_default();
            format!("{verb} {}{group}", quoted(user))
        }
        Action::SystemConfigure { key, value } => {
            format!("Set {} to {}", quoted(key), quoted(value))
        }
        Action::SoulProposeChange { rationale, .. } => {
            format!("Propose a change to the SOUL: {}", quoted(rationale))
        }
    }
}

struct Row {
    step: String,
    what: String,
    needs: String,
    verdict: String,
    /// Lines under the row: the reason for a refusal, the exact action.
    under: Vec<String>,
}

fn verdict_of(item: &ReviewedAction, labels: &Labels) -> (String, Option<String>) {
    match &item.verdict {
        Verdict::Allowed => ("ok".to_owned(), None),
        Verdict::NeedsRoot => ("ROOT, held".to_owned(), None),
        Verdict::Refused(why) => ("REFUSED".to_owned(), Some(why.friendly(labels))),
    }
}

fn rows(step: &ReviewedStep, labels: &Labels, view: View) -> Vec<Row> {
    let mut out = Vec::new();
    if let Some(why) = &step.blocked {
        let (verdict, under) = match why.ambiguity() {
            Some(_) => ("CHOOSE".to_owned(), "more than one match".to_owned()),
            None => ("REFUSED".to_owned(), why.friendly(labels)),
        };
        out.push(Row {
            step: step.id.to_string(),
            what: step.asks.clone(),
            needs: String::new(),
            verdict,
            under: vec![under],
        });
        return out;
    }
    if step.items.is_empty() {
        out.push(Row {
            step: step.id.to_string(),
            what: step.asks.clone(),
            needs: String::new(),
            verdict: "nothing to do".to_owned(),
            under: vec!["the reference produced no items".to_owned()],
        });
    }
    for (index, item) in step.items.iter().enumerate() {
        let (verdict, reason) = verdict_of(item, labels);
        let mut under: Vec<String> = reason.into_iter().collect();
        if view == View::Details {
            under.push(format!("exact: {}", item.action.describe()));
        }
        out.push(Row {
            step: if index == 0 {
                step.id.to_string()
            } else {
                String::new()
            },
            what: say(&item.action, labels),
            needs: item.action.capability().to_string(),
            verdict,
            under,
        });
    }
    out
}

fn notes(review: &Review) -> Vec<String> {
    let mut found = Vec::new();
    for step in &review.steps {
        found.extend(step.notes.iter().map(|n| format!("{}: {n}", step.id)));
        for item in &step.items {
            if let Some(Ok(output)) = &item.output {
                found.extend(output.notes.iter().map(|n| format!("{}: {n}", step.id)));
            }
        }
    }
    found
}

fn summary_line(review: &Review) -> String {
    let (mut ok, mut held, mut refused, mut choose) = (0, 0, 0, 0);
    for step in &review.steps {
        match step.blocked.as_ref() {
            Some(why) if why.ambiguity().is_some() => choose += 1,
            Some(_) => refused += 1,
            None => {
                for item in &step.items {
                    match item.verdict {
                        Verdict::Allowed => ok += 1,
                        Verdict::NeedsRoot => held += 1,
                        Verdict::Refused(_) => refused += 1,
                    }
                }
            }
        }
    }
    let choose = if choose > 0 {
        format!(", {choose} waiting for your choice")
    } else {
        String::new()
    };
    let who = match review.origin {
        Origin::Model => "The model says",
        Origin::Typed => "Typed commands",
    };
    format!(
        "{} steps, {} actions: {ok} ok, {held} held for root, {refused} refused{choose}.  {who}: {}",
        review.steps.len(),
        ok + held + refused,
        quoted(&review.summary)
    )
}

/// The plan screen: table, notes and summary.
pub fn plan_screen(review: &Review, view: View) -> String {
    let all: Vec<Row> = review
        .steps
        .iter()
        .flat_map(|s| rows(s, &review.labels, view))
        .collect();
    let width = |header: &str, cell: fn(&Row) -> &str| {
        all.iter()
            .map(|r| cell(r).chars().count())
            .chain([header.chars().count()])
            .max()
            .unwrap_or(0)
    };
    let (w_step, w_what, w_needs) = (
        width("Step", |r| &r.step),
        width("What", |r| &r.what),
        width("Needs", |r| &r.needs),
    );
    let line = |step: &str, what: &str, needs: &str, verdict: &str| {
        format!("  {step:<w_step$}  {what:<w_what$}  {needs:<w_needs$}  {verdict}\n")
    };
    let mut out = String::from("\n");
    out.push_str(&line("Step", "What", "Needs", "Verdict"));
    for row in &all {
        out.push_str(&line(&row.step, &row.what, &row.needs, &row.verdict));
        for under in &row.under {
            out.push_str(&format!("  {:w_step$}    ↳ {under}\n", ""));
        }
    }
    let notes = notes(review);
    if !notes.is_empty() {
        out.push_str("\n  Notes\n");
        for note in notes {
            out.push_str(&format!("   · {note}\n"));
        }
    }
    out.push_str(&format!("\n  {}\n", summary_line(review)));
    printable(&out, true)
}

/// One root action, confirmed on its own. The real location is always here.
pub fn root_panel(who: &str, step: &StepId, action: &Action, labels: &Labels) -> String {
    printable(
        &format!(
            "╔═ ROOT CONFIRMATION ═ asked of {} ═ step {step}\n║ Action  {}\n║ Exact   {}\n║ Needs   {}, as root. Confirms this one action only.\n╚═",
            quoted(who),
            say(action, labels),
            action.describe(),
            action.capability(),
        ),
        true,
    )
}

/// The question for a reference that matched several things, and one line per
/// candidate with enough to tell them apart.
pub fn question(ambiguity: &Ambiguity, labels: &Labels) -> (String, Vec<String>) {
    (
        printable(
            &format!(
                "Step {}: {} matches more than one thing. Which one?",
                ambiguity.step, ambiguity.what
            ),
            false,
        ),
        ambiguity
            .candidates
            .iter()
            .map(|path| printable(&labels.of(path), false))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::broker::{Authority, review};
    use crate::plan::{Document, parse_checked};
    use crate::reference::Place;
    use crate::resolve::Choices;
    use crate::testing::{FakeHost, user};

    fn host() -> FakeHost {
        FakeHost::with_files(&[("/work/big.iso", 900), ("/work/movie.mkv", 400)])
            .forbid(crate::action::Access::Delete, "/work/movie.mkv")
            .place(Place::Downloads, "/work")
    }

    fn reviewed(plan: &str, administrator: bool) -> Review {
        let Document::Plan(plan) = parse_checked(plan).unwrap() else {
            panic!("not a plan")
        };
        review(
            &plan,
            &Authority::full(user(administrator)),
            &host(),
            &Choices::default(),
        )
    }

    const DELETE_LARGEST: &str = r#"{"kind":"plan","version":2,"summary":"Delete the two largest","steps":[
      {"id":"s1","action":"fs.find_largest","args":{"target":{"place":"downloads"},"count":2},"depends_on":[],"elevate":false},
      {"id":"s2","action":"fs.delete","args":{"target":{"from_step":"s1"}},"depends_on":["s1"],"elevate":false}]}"#;

    #[test]
    fn the_plan_screen_has_a_table_a_verdict_per_action_and_a_summary_and_no_location() {
        let screen = plan_screen(&reviewed(DELETE_LARGEST, false), View::Friendly);
        let expected = "
  Step  What                                   Needs      Verdict
  s1    Find the 2 largest files in Downloads  fs.read    ok
  s2    Delete \"big.iso\" (Downloads)           fs.delete  ok
        Delete \"movie.mkv\" (Downloads)         fs.delete  REFUSED
          ↳ your permissions do not allow delete on \"movie.mkv\" (Downloads)

  2 steps, 3 actions: 2 ok, 0 held for root, 1 refused.  The model says: \"Delete the two largest\"
";
        assert_eq!(screen, expected);
        assert!(!screen.contains("/work"), "{screen}");
    }

    #[test]
    fn the_details_view_adds_the_exact_action_under_each_line() {
        let screen = plan_screen(&reviewed(DELETE_LARGEST, false), View::Details);
        assert!(
            screen.contains(r#"exact: fs.delete path="/work/big.iso""#),
            "{screen}"
        );
        assert!(
            screen.contains(r#"exact: fs.find_largest path="/work" count=2"#),
            "{screen}"
        );
    }

    #[test]
    fn a_step_that_waits_for_a_choice_says_so() {
        let plan = r#"{"kind":"plan","version":2,"summary":"x","steps":[
          {"id":"s1","action":"fs.read","args":{"target":{"name":"o","in":"downloads","kind":"file"}},"depends_on":[],"elevate":false}]}"#;
        let screen = plan_screen(&reviewed(plan, false), View::Friendly);
        assert!(screen.contains("CHOOSE"), "{screen}");
        assert!(screen.contains("1 waiting for your choice"), "{screen}");
    }

    #[test]
    fn the_root_panel_always_carries_the_exact_action_with_its_real_location() {
        let review = reviewed(DELETE_LARGEST, true);
        let action = &review.steps[1].items[0].action;
        let panel = root_panel("ana", &review.steps[1].id, action, &review.labels);
        assert!(
            panel.contains(r#"Action  Delete "big.iso" (Downloads)"#),
            "{panel}"
        );
        assert!(
            panel.contains(r#"Exact   fs.delete path="/work/big.iso""#),
            "{panel}"
        );
        assert!(panel.contains("fs.delete, as root"), "{panel}");
    }

    #[test]
    fn names_from_the_machine_cannot_rewrite_the_screen() {
        let host = FakeHost::with_files(&[("/work/a\u{1b}[2Jb", 5)]);
        let Document::Plan(plan) = parse_checked(
            r#"{"kind":"plan","version":2,"summary":"s\u001b[31m","steps":[
          {"id":"s1","action":"fs.list","args":{"target":{"place":"downloads"}},"depends_on":[],"elevate":false},
          {"id":"s2","action":"fs.read","args":{"target":{"from_step":"s1"}},"depends_on":["s1"],"elevate":false}]}"#,
        )
        .unwrap() else {
            panic!("not a plan")
        };
        let review = review(
            &plan,
            &Authority::full(user(false)),
            &host,
            &Choices::default(),
        );
        for view in [View::Friendly, View::Details] {
            let screen = plan_screen(&review, view);
            assert!(!screen.contains('\u{1b}'), "{screen:?}");
        }
    }

    #[test]
    fn the_header_shows_the_user_the_model_and_only_the_name_of_the_log() {
        let text = header(&Header {
            user: "ana",
            administrator: true,
            model: "127.0.0.1:8089",
            log: "decisions.jsonl",
        });
        assert!(text.contains(r#"user  "ana" (administrator)"#), "{text}");
        assert!(text.contains(r#"log  "decisions.jsonl""#), "{text}");
    }
}
