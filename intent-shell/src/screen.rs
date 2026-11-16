//! What the person sees in the terminal (ADR 0014, ADR 0018): a header, a table
//! with one line per concrete action and the broker's verdict beside it,
//! notes, a summary line, a separate panel for each root action, and the tree
//! view of listings and choices.
//!
//! Pure functions from the broker's data to text, so the screen is unit tested
//! and the terminal adapter only prints it. Locations are written relative to
//! the current directory or to `~`; the exact absolute location is in the
//! details view and in the root panel, where ADR 0007 requires the exact action.

use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::action::{Action, UserOperation, WriteMode, is_hidden, printable, quoted};
use crate::broker::{Review, ReviewedAction, ReviewedStep, Verdict};
use crate::label::Labels;
use crate::plan::StepId;
use crate::ports::{ChoiceLine, EntryKind, TREE_DEPTH, TreeEntry};
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
    /// The directory the session started in, written as the person would (`~/Downloads`).
    pub cwd: &'a str,
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
            " intent shell ──────────────────────────────────────────────\n  user  {}{rank}   model  {}   log  {}\n  started in  {}\n ───────────────────────────────────────────────────────────────",
            quoted(h.user),
            quoted(h.model),
            quoted(h.log),
            h.cwd,
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
        Action::Cd { path } => format!("Go to {}", of(path)),
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
            needs: item
                .action
                .capability()
                .map_or_else(|| "-".to_owned(), |c| c.to_string()),
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
    format!(
        "{} steps, {} actions: {ok} ok, {held} held for root, {refused} refused{choose}.  The model says: {}",
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
    let mut out = format!(
        "\n  in {}\n\n",
        printable(&review.labels.cwd_text().to_string_lossy(), false)
    );
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
            action
                .capability()
                .map_or_else(String::new, |c| c.to_string()),
        ),
        true,
    )
}

/// The question for a reference that matched several things, and the
/// candidates in their parent context so the person tells them apart by where
/// they sit. Only the candidates can be picked (ADR 0018).
pub fn question(ambiguity: &Ambiguity, labels: &Labels) -> (String, Vec<ChoiceLine>) {
    let entries: Vec<TreeEntry> = ambiguity
        .candidates
        .iter()
        .map(|path| TreeEntry {
            path: path.clone(),
            kind: EntryKind::File,
            size: None,
            readable: true,
            protected: false,
        })
        .collect();
    let options = TreeOptions {
        depth: usize::MAX,
        show_hidden: true,
        // A candidate that is cut off cannot be picked.
        max_lines: usize::MAX,
    };
    (
        printable(
            &format!(
                "Step {}: {} matches more than one thing. Which one?",
                ambiguity.step, ambiguity.what
            ),
            false,
        ),
        tree(
            &common_parent(&ambiguity.candidates),
            &entries,
            &ambiguity.candidates,
            labels,
            &TreeMarks::default(),
            &options,
        ),
    )
}

/// The deepest folder that holds every path.
fn common_parent(paths: &[PathBuf]) -> PathBuf {
    let mut common: Option<PathBuf> = None;
    for path in paths {
        let parent = path.parent().unwrap_or(Path::new("/"));
        common = Some(match common {
            None => parent.to_path_buf(),
            Some(current) => current
                .ancestors()
                .find(|a| parent.starts_with(a))
                .unwrap_or(Path::new("/"))
                .to_path_buf(),
        });
    }
    common.unwrap_or_else(|| PathBuf::from("/"))
}

/// What a tree shows besides the listing: where a plan would ask, or hold an
/// action for root.
#[derive(Debug, Default)]
pub struct TreeMarks {
    pub root: HashSet<PathBuf>,
    pub asks: HashSet<PathBuf>,
}

/// The locations the reviewed plan would ask about or hold for root.
pub fn marks_of(review: &Review) -> TreeMarks {
    let mut marks = TreeMarks::default();
    for item in review
        .steps
        .iter()
        .filter(|s| s.blocked.is_none())
        .flat_map(|s| &s.items)
    {
        let set = match &item.verdict {
            Verdict::NeedsRoot => &mut marks.root,
            Verdict::Allowed if item.action.needs_confirmation() => &mut marks.asks,
            _ => continue,
        };
        set.extend(
            item.action
                .accesses()
                .into_iter()
                .map(|(_, p)| p.to_path_buf()),
        );
    }
    marks
}

/// Depth, length and hidden entries of a tree (ADR 0018, decision 3).
#[derive(Debug, Clone)]
pub struct TreeOptions {
    pub depth: usize,
    pub max_lines: usize,
    pub show_hidden: bool,
}

impl Default for TreeOptions {
    fn default() -> Self {
        Self {
            depth: TREE_DEPTH,
            max_lines: 200,
            show_hidden: false,
        }
    }
}

struct Node {
    name: OsString,
    path: PathBuf,
    entry: Option<TreeEntry>,
    children: BTreeMap<OsString, Node>,
}

impl Node {
    fn new(name: OsString, path: PathBuf) -> Self {
        Self {
            name,
            path,
            entry: None,
            children: BTreeMap::new(),
        }
    }

    fn is_dir(&self) -> bool {
        match &self.entry {
            Some(entry) => entry.kind == EntryKind::Dir || !self.children.is_empty(),
            None => true,
        }
    }

    fn is_hidden(&self) -> bool {
        is_hidden(Path::new(&self.name))
    }
}

fn build(root: &Path, entries: &[TreeEntry]) -> Node {
    let mut top = Node::new(OsString::new(), root.to_path_buf());
    for entry in entries {
        let Ok(inside) = entry.path.strip_prefix(root) else {
            continue;
        };
        let mut node = &mut top;
        for part in inside.components() {
            let name = part.as_os_str().to_owned();
            let path = node.path.join(&name);
            node = node
                .children
                .entry(name.clone())
                .or_insert_with(|| Node::new(name, path));
        }
        node.entry = Some(entry.clone());
    }
    top
}

/// Folders first, then names ignoring case, then the bytes.
fn ordered(node: &Node) -> Vec<&Node> {
    let mut children: Vec<&Node> = node.children.values().collect();
    children.sort_by_key(|c| {
        (
            !c.is_dir(),
            c.name.to_string_lossy().to_lowercase(),
            c.name.clone(),
        )
    });
    children
}

fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    // 999.96 KB prints as 1000.0: it is 1.0 MB.
    while value >= 999.95 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 && value < 999.5 {
        format!("{value:.0} {}", UNITS[unit])
    } else if unit == 0 {
        format!("{:.1} {}", value / 1000.0, UNITS[1])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// A name as it is printed bare: byte-exact like a quoted path, without the quotes.
fn bare(name: &std::ffi::OsStr) -> String {
    let quoted = crate::action::quoted_path(Path::new(name));
    quoted[1..quoted.len() - 1].to_owned()
}

struct TreeRow {
    left: String,
    size: String,
    marks: String,
    option: Option<usize>,
}

struct Walk<'a> {
    labels: &'a Labels,
    marks: &'a TreeMarks,
    options: &'a TreeOptions,
    candidates: &'a [PathBuf],
    rows: Vec<TreeRow>,
    hidden: usize,
    folders: usize,
    last_level_folders: bool,
}

impl Walk<'_> {
    fn children(&mut self, node: &Node, prefix: &str, level: usize) {
        let shown: Vec<&Node> = ordered(node)
            .into_iter()
            .filter(|c| {
                let skipped = c.is_hidden() && !self.options.show_hidden;
                self.hidden += usize::from(skipped);
                !skipped
            })
            .collect();
        for (index, child) in shown.iter().enumerate() {
            let last = index + 1 == shown.len();
            let (branch, indent) = if last {
                ("└─ ", "   ")
            } else {
                ("├─ ", "│  ")
            };
            self.rows
                .push(self.row(child, &format!("{prefix}{branch}"), node));
            if child.is_dir() {
                self.folders += 1;
                if level < self.options.depth {
                    self.children(child, &format!("{prefix}{indent}"), level + 1);
                } else {
                    self.last_level_folders = true;
                }
            }
        }
    }

    fn row(&self, node: &Node, lead: &str, parent: &Node) -> TreeRow {
        let slash = if node.is_dir() { "/" } else { "" };
        let mut name = format!("{}{slash}", bare(&node.name));
        let mut marks: Vec<String> = Vec::new();
        if let Some(entry) = &node.entry {
            if let EntryKind::Link(target) = &entry.kind {
                name = format!("{name} -> {}", bare(target.as_os_str()));
                marks.push("link".to_owned());
            }
            if !entry.readable {
                marks.push("unreadable".to_owned());
            }
            if entry.protected {
                marks.push("protected".to_owned());
            }
        }
        if node.is_hidden() {
            marks.push("hidden, asks to open".to_owned());
        }
        if self.labels.is_system(&node.path)
            && (!self.labels.is_system(&parent.path) || self.labels.holds_home(&parent.path))
        {
            marks.push("system".to_owned());
        }
        if self.marks.asks.contains(&node.path) {
            marks.push("⚠ asks".to_owned());
        }
        if self.marks.root.contains(&node.path) {
            marks.push("ROOT".to_owned());
        }
        let size = node
            .entry
            .as_ref()
            .and_then(|e| e.size)
            .map(size_text)
            .unwrap_or_default();
        TreeRow {
            left: format!("{lead}{name}"),
            size,
            marks: marks.join(" · "),
            option: self.candidates.iter().position(|c| *c == node.path),
        }
    }
}

/// A listing, or paths with their folders around them, as a tree (ADR 0018,
/// decision 3). `entries` are what to show below `root`; folders that only
/// hold them are inferred. Pure, so `--plain`, the deterministic shell and the
/// interface print the same text. A bound is never silent.
pub fn tree(
    root: &Path,
    entries: &[TreeEntry],
    candidates: &[PathBuf],
    labels: &Labels,
    marks: &TreeMarks,
    options: &TreeOptions,
) -> Vec<ChoiceLine> {
    let top = build(root, entries);
    let mut walk = Walk {
        labels,
        marks,
        options,
        candidates,
        rows: Vec::new(),
        hidden: 0,
        folders: 0,
        last_level_folders: false,
    };
    walk.children(&top, " ", 1);

    let root_text = if root == labels.cwd() {
        labels.cwd_text()
    } else {
        labels.display(root)
    };
    let mut head = bare(root_text.as_os_str());
    if labels.is_system(root) && !labels.holds_home(root) {
        head.push_str("  (system)");
    }
    let plural = if walk.folders == 1 {
        "folder"
    } else {
        "folders"
    };
    let mut counts = vec![format!("{} {plural}", walk.folders)];
    if walk.hidden > 0 {
        counts.push(format!("{} hidden", walk.hidden));
    }
    let width = walk
        .rows
        .iter()
        .map(|r| r.left.chars().count())
        .chain([head.chars().count() + 1])
        .max()
        .unwrap_or(0);
    let size_width = walk
        .rows
        .iter()
        .map(|r| r.size.chars().count())
        .max()
        .unwrap_or(0);
    let mut lines = vec![ChoiceLine {
        text: format!(
            "{:<width$}  {:>size_width$}  {}",
            format!(" {head}"),
            "",
            counts.join(" · ")
        )
        .trim_end()
        .to_owned(),
        option: None,
    }];
    let total = walk.rows.len();
    let cut = total > options.max_lines;
    for row in walk.rows.into_iter().take(options.max_lines) {
        lines.push(ChoiceLine {
            text: format!(
                "{:<width$}  {:>size_width$}  {}",
                row.left, row.size, row.marks
            )
            .trim_end()
            .to_owned(),
            option: row.option,
        });
    }
    if cut {
        lines.push(ChoiceLine {
            text: format!(
                " └─ … {} more (use a path or --depth to open)",
                total - options.max_lines
            ),
            option: None,
        });
    }
    if walk.last_level_folders && options.depth != usize::MAX {
        lines.push(ChoiceLine {
            text: " … folders at the last level may hold more (use a path or --depth to open)"
                .to_owned(),
            option: None,
        });
    }
    lines
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
            .with_home("/work")
    }

    fn reviewed(plan: &str, administrator: bool) -> Review {
        let Document::Plan(plan) = parse_checked(plan).unwrap() else {
            panic!("not a plan")
        };
        review(
            &plan,
            &Authority::full(user(administrator)),
            &host(),
            std::path::Path::new("/work"),
            &Choices::default(),
        )
    }

    const DELETE_LARGEST: &str = r#"{"kind":"plan","version":3,"summary":"Delete the two largest","steps":[
      {"id":"s1","action":"fs.find_largest","args":{"target":{"place":"downloads"},"count":2},"depends_on":[],"elevate":false},
      {"id":"s2","action":"fs.delete","args":{"target":{"from_step":"s1"}},"depends_on":["s1"],"elevate":false}]}"#;

    #[test]
    fn the_plan_screen_has_the_directory_a_table_a_verdict_per_action_and_a_summary() {
        let screen = plan_screen(&reviewed(DELETE_LARGEST, false), View::Friendly);
        let expected = "
  in ~

  Step  What                             Needs      Verdict
  s1    Find the 2 largest files in \".\"  fs.read    ok
  s2    Delete \"big.iso\"                 fs.delete  ok
        Delete \"movie.mkv\"               fs.delete  REFUSED
          ↳ your permissions do not allow delete on \"movie.mkv\"

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
        let plan = r#"{"kind":"plan","version":3,"summary":"x","steps":[
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
        assert!(panel.contains(r#"Action  Delete "big.iso""#), "{panel}");
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
            r#"{"kind":"plan","version":3,"summary":"s\u001b[31m","steps":[
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
            std::path::Path::new("/work"),
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
            cwd: "~",
            log: "decisions.jsonl",
        });
        assert!(text.contains(r#"user  "ana" (administrator)"#), "{text}");
        assert!(text.contains(r#"log  "decisions.jsonl""#), "{text}");
    }
}
