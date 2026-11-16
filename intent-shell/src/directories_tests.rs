//! Directories, navigation and the tree view through the whole pipeline
//! (ADR 0018): every way of naming a location, what the broker decides about a
//! typed path, the session's current directory, the tree, and the old schema.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::action::{Access, Action, Capability};
use crate::broker::{Authority, Refusal, Verdict};
use crate::label::Labels;
use crate::pipeline_tests::{Run, events, plan, reviewed, run_in, run_with, step};
use crate::plan::parse_checked;
use crate::ports::{EntryKind, Host, TreeEntry};
use crate::reference::Place;
use crate::resolve::{ResolveError, Scope, resolve_path};
use crate::screen::{self, TreeMarks, TreeOptions, View};
use crate::session::{ItemResult, Outcome, SessionState};
use crate::testing::{Answers, FakeHost, MemoryLog, user};

fn home() -> FakeHost {
    FakeHost::with_files(&[
        ("/home/ana/Downloads/films/old.mkv", 1_900),
        ("/home/ana/Downloads/films/new.mkv", 2_300),
        ("/home/ana/Downloads/notes.txt", 5),
        ("/home/ana/Documents/report.pdf", 7),
        ("/etc/hosts", 1),
        ("/var/log/old.log", 3),
    ])
    .dir("/home/ana/Downloads")
    .dir("/home/ana/Documents")
    .dir("/home/ana/Downloads/films")
    .dir("/etc")
    .place(Place::Downloads, "/home/ana/Downloads")
    .place(Place::Documents, "/home/ana/Documents")
}

fn session_in(cwd: &str) -> SessionState {
    SessionState {
        cwd: PathBuf::from(cwd),
    }
}

/// `session.cd` has no `elevate` field; every other action has one.
fn step_of(id: &str, action: &str, args: &str) -> String {
    let elevate = if action == "session.cd" {
        ""
    } else {
        r#","elevate":false"#
    };
    format!(r#"{{"id":"{id}","action":"{action}","args":{args},"depends_on":[]{elevate}}}"#)
}

fn one_step(action: &str, args: &str) -> String {
    plan(&[step_of("s1", action, args)])
}

fn path_args(path: &str) -> String {
    format!(r#"{{"target":{{"path":"{path}"}}}}"#)
}

fn ask(host: &FakeHost, session: &mut SessionState, text: &str) -> Run {
    run_in(
        session,
        &[text],
        host,
        &Authority::full(user(false)),
        Answers::yes(),
        MemoryLog::default(),
    )
    .0
}

/// The exact action of the first item of the first step.
fn exact(run: &Run) -> String {
    let step = &reviewed(&run.outcome).steps[0];
    match step.items.first() {
        Some(item) => item.action.describe(),
        None => format!("no action: {:?}", step.blocked),
    }
}

#[test]
fn every_form_of_reference_resolves_to_the_resource_it_names() {
    let host = home();
    let cases: [(&str, &str, &str, &str); 15] = [
        (
            "/home/ana",
            "fs.list",
            r#"{"target":{"path":"/home/ana/Downloads"}}"#,
            r#"fs.list path="/home/ana/Downloads""#,
        ),
        (
            "/etc",
            "fs.list",
            r#"{"target":{"path":"~/Downloads"}}"#,
            r#"fs.list path="/home/ana/Downloads""#,
        ),
        (
            "/home/ana/Downloads",
            "fs.list",
            r#"{"target":{"path":"films"}}"#,
            r#"fs.list path="/home/ana/Downloads/films""#,
        ),
        (
            "/home/ana/Downloads",
            "fs.stat",
            r#"{"target":{"path":"./films/../notes.txt"}}"#,
            r#"fs.stat path="/home/ana/Downloads/notes.txt""#,
        ),
        (
            "/home/ana/Downloads/films",
            "fs.list",
            r#"{"target":{"path":".."}}"#,
            r#"fs.list path="/home/ana/Downloads""#,
        ),
        (
            "/home/ana/Downloads",
            "fs.list",
            r#"{"target":{"path":"~"}}"#,
            r#"fs.list path="/home/ana""#,
        ),
        (
            "/home/ana/Downloads",
            "fs.list",
            r#"{"target":{"path":"/home/ana/Downloads/"}}"#,
            r#"fs.list path="/home/ana/Downloads""#,
        ),
        (
            "/home/ana",
            "fs.list",
            r#"{"target":{"place":"downloads"}}"#,
            r#"fs.list path="/home/ana/Downloads""#,
        ),
        (
            "/home/ana",
            "fs.stat",
            r#"{"target":{"name":"old.mkv","in":"downloads","kind":"file"}}"#,
            r#"fs.stat path="/home/ana/Downloads/films/old.mkv""#,
        ),
        (
            "/home/ana/Documents",
            "fs.stat",
            r#"{"target":{"name":"old.mkv","in":{"path":"~/Downloads"},"kind":"file"}}"#,
            r#"fs.stat path="/home/ana/Downloads/films/old.mkv""#,
        ),
        (
            "/home/ana/Documents",
            "fs.stat",
            r#"{"target":{"name":"old.mkv","in":{"path":"../Downloads/films"},"kind":"file"}}"#,
            r#"fs.stat path="/home/ana/Downloads/films/old.mkv""#,
        ),
        (
            "/etc",
            "fs.read",
            r#"{"target":{"under_home":true,"name":"report.pdf","kind":"file"}}"#,
            r#"fs.read path="/home/ana/Documents/report.pdf""#,
        ),
        (
            "/home/ana",
            "fs.write",
            r#"{"target":{"new_name":"n.txt","in":{"path":"Documents"}},"content":"c","mode":"create"}"#,
            r#"fs.write path="/home/ana/Documents/n.txt" mode=Create content="c""#,
        ),
        (
            "/home/ana/Documents",
            "fs.write",
            r#"{"target":{"path":"fresh.txt"},"content":"c","mode":"create"}"#,
            r#"fs.write path="/home/ana/Documents/fresh.txt" mode=Create content="c""#,
        ),
        (
            "/home/ana",
            "session.cd",
            r#"{"target":{"path":"Downloads"}}"#,
            r#"session.cd path="/home/ana/Downloads""#,
        ),
    ];
    for (cwd, action, args, expected) in cases {
        let run = ask(&host, &mut session_in(cwd), &one_step(action, args));
        assert_eq!(exact(&run), expected, "{action} {args} from {cwd}");
    }
}

#[test]
fn a_path_to_a_folder_is_where_a_move_goes_and_a_path_to_a_new_name_is_the_new_name() {
    let host = home();
    let into = r#"{"from":{"path":"report.pdf"},"to":{"path":"~/Downloads"}}"#;
    let run = ask(
        &host,
        &mut session_in("/home/ana/Documents"),
        &one_step("fs.move", into),
    );
    assert_eq!(
        exact(&run),
        r#"fs.move from="/home/ana/Documents/report.pdf" to="/home/ana/Downloads/report.pdf""#
    );
    let rename = r#"{"from":{"path":"report.pdf"},"to":{"path":"renamed.pdf"}}"#;
    let run = ask(
        &host,
        &mut session_in("/home/ana/Documents"),
        &one_step("fs.move", rename),
    );
    assert_eq!(
        exact(&run),
        r#"fs.move from="/home/ana/Documents/report.pdf" to="/home/ana/Documents/renamed.pdf""#
    );
}

#[test]
fn a_dot_dot_above_the_root_names_nothing_and_never_reaches_the_host() {
    let host = home();
    let run = ask(
        &host,
        &mut session_in("/home/ana"),
        &one_step("fs.list", &path_args("../../../../x")),
    );
    assert!(matches!(
        reviewed(&run.outcome).steps[0].blocked,
        Some(Refusal::Unresolved(ResolveError::EscapesRoot(_)))
    ));
    assert!(host.executed.borrow().is_empty());
    let step = crate::plan::StepId::parse("s1").unwrap();
    let scope = Scope {
        host: &host,
        cwd: Path::new("/"),
    };
    let _ = step;
    assert!(matches!(
        resolve_path(&scope, "/..", false),
        Err(ResolveError::EscapesRoot(_))
    ));
}

#[test]
fn a_dot_dot_that_leaves_the_home_is_not_the_resolvers_to_refuse_the_broker_decides() {
    let host = home().forbid(Access::Read, "/etc/hosts");
    let scope = Scope {
        host: &host,
        cwd: Path::new("/home/ana"),
    };
    // The resolver names the location and nothing more.
    assert_eq!(
        resolve_path(&scope, "../../etc/hosts", false).unwrap(),
        PathBuf::from("/etc/hosts")
    );
    let run = ask(
        &host,
        &mut session_in("/home/ana"),
        &one_step("fs.read", &path_args("~/../../etc/hosts")),
    );
    let item = &reviewed(&run.outcome).steps[0].items[0];
    assert_eq!(item.action.describe(), r#"fs.read path="/etc/hosts""#);
    assert!(matches!(
        item.verdict,
        Verdict::Refused(Refusal::PermissionDenied {
            access: Access::Read,
            ..
        })
    ));
    // The refusal is on the screen, with the system location written absolute.
    let shown = run.outcome.render();
    assert!(
        shown.contains(r#"your permissions do not allow read on "/etc/hosts""#),
        "{shown}"
    );
    assert!(host.executed.borrow().is_empty());
}

#[test]
fn a_link_out_of_the_home_resolves_to_its_target_and_the_broker_judges_the_target() {
    let host = home()
        .link("/home/ana/Documents/shortcut", "/etc")
        .forbid(Access::Read, "/etc/hosts");
    let scope = Scope {
        host: &host,
        cwd: Path::new("/home/ana"),
    };
    assert_eq!(
        resolve_path(&scope, "Documents/shortcut/hosts", false).unwrap(),
        PathBuf::from("/etc/hosts")
    );
    let run = ask(
        &host,
        &mut session_in("/home/ana"),
        &one_step("fs.read", &path_args("Documents/shortcut/hosts")),
    );
    let item = &reviewed(&run.outcome).steps[0].items[0];
    assert_eq!(item.action.describe(), r#"fs.read path="/etc/hosts""#);
    assert!(matches!(
        item.verdict,
        Verdict::Refused(Refusal::PermissionDenied { .. })
    ));
    assert!(run.outcome.render().contains("do not allow read"));
}

#[test]
fn naming_a_link_to_remove_it_names_the_link_and_not_what_it_leads_to() {
    let host = home().link("/home/ana/Documents/shortcut", "/etc");
    let run = ask(
        &host,
        &mut session_in("/home/ana"),
        &one_step("fs.delete", &path_args("Documents/shortcut")),
    );
    assert_eq!(
        exact(&run),
        r#"fs.delete path="/home/ana/Documents/shortcut""#
    );
}

#[test]
fn what_the_user_cannot_reach_is_refused_by_permission_and_not_reported_as_missing() {
    let host = home()
        .forbid(Access::Stat, "/root/secret")
        .forbid(Access::Read, "/root/secret");
    let run = ask(
        &host,
        &mut session_in("/home/ana"),
        &one_step("fs.read", &path_args("/root/secret")),
    );
    let item = &reviewed(&run.outcome).steps[0].items[0];
    assert!(matches!(
        item.verdict,
        Verdict::Refused(Refusal::PermissionDenied { .. })
    ));
}

#[test]
fn a_path_that_names_nothing_in_a_folder_the_user_can_reach_is_not_found() {
    let host = home();
    let run = ask(
        &host,
        &mut session_in("/home/ana"),
        &one_step("fs.read", &path_args("Documents/missing.txt")),
    );
    assert!(matches!(
        reviewed(&run.outcome).steps[0].blocked,
        Some(Refusal::Unresolved(ResolveError::NotFound(_)))
    ));
    assert!(host.executed.borrow().is_empty());
}

#[test]
fn naming_a_system_location_is_allowed_and_the_broker_alone_decides() {
    let host = home();
    let run = ask(
        &host,
        &mut session_in("/home/ana"),
        &one_step("fs.list", &path_args("/etc")),
    );
    assert_eq!(exact(&run), r#"fs.list path="/etc""#);
    assert_eq!(
        reviewed(&run.outcome).steps[0].items[0].verdict,
        Verdict::Allowed
    );
    let forbidden = home().forbid(Access::Read, "/etc");
    let run = ask(
        &forbidden,
        &mut session_in("/home/ana"),
        &one_step("fs.list", &path_args("/etc")),
    );
    assert!(matches!(
        reviewed(&run.outcome).steps[0].items[0].verdict,
        Verdict::Refused(Refusal::PermissionDenied { .. })
    ));
}

#[test]
fn a_cd_changes_where_the_next_request_resolves_and_the_model_is_told() {
    let host = home();
    let mut session = session_in("/home/ana");
    let cd = ask(
        &host,
        &mut session,
        &one_step("session.cd", &path_args("~/Downloads")),
    );
    assert!(matches!(cd.outcome, Outcome::Executed { .. }));
    assert_eq!(session.cwd, PathBuf::from("/home/ana/Downloads"));
    assert!(cd.outcome.render().contains("now in ~/Downloads"));

    let (next, planner) = run_in(
        &mut session,
        &[&one_step("fs.list", &path_args("films"))],
        &host,
        &Authority::full(user(false)),
        Answers::yes(),
        MemoryLog::default(),
    );
    assert_eq!(exact(&next), r#"fs.list path="/home/ana/Downloads/films""#);
    assert_eq!(
        planner.directories.borrow().as_slice(),
        ["~/Downloads (/home/ana/Downloads)"]
    );
    // The plan screen says where it was made.
    let screen = screen::plan_screen(reviewed(&next.outcome), View::Friendly);
    assert!(screen.contains("in ~/Downloads"), "{screen}");
    assert_eq!(
        reviewed(&next.outcome).cwd,
        PathBuf::from("/home/ana/Downloads")
    );
}

#[test]
fn a_cd_is_logged_with_where_it_came_from_and_where_it_went() {
    let host = home();
    let mut session = session_in("/home/ana");
    let run = ask(
        &host,
        &mut session,
        &one_step("session.cd", &path_args("Downloads")),
    );
    let cd = &events(&run, "cd")[0];
    assert_eq!(cd["from"], "\"/home/ana\"");
    assert_eq!(cd["to"], "\"/home/ana/Downloads\"");
    assert_eq!(events(&run, "request")[0]["cwd"], "\"/home/ana\"");
}

#[test]
fn a_cd_does_not_leak_into_another_session() {
    let host = home();
    let mut first = session_in("/home/ana");
    let mut second = SessionState::start(&host);
    ask(
        &host,
        &mut first,
        &one_step("session.cd", &path_args("~/Downloads")),
    );
    assert_eq!(first.cwd, PathBuf::from("/home/ana/Downloads"));
    assert_eq!(second.cwd, PathBuf::from("/home/ana"));
    let run = ask(
        &host,
        &mut second,
        &one_step("fs.list", &path_args("Downloads")),
    );
    assert_eq!(exact(&run), r#"fs.list path="/home/ana/Downloads""#);
}

#[test]
fn a_session_starts_at_home_or_at_the_root_when_there_is_none() {
    assert_eq!(SessionState::start(&home()).cwd, PathBuf::from("/home/ana"));
    let mut no_home = home();
    no_home.home = None;
    assert_eq!(SessionState::start(&no_home).cwd, PathBuf::from("/"));
}

#[test]
fn a_cd_must_be_a_plan_of_its_own_because_its_directory_applies_to_the_next_request() {
    let text = plan(&[
        step_of("s1", "session.cd", &path_args("Downloads")),
        step("s2", "fs.list", &path_args("Downloads"), "false"),
    ]);
    let error = parse_checked(&text).unwrap_err().to_string();
    assert!(
        error.contains("session.cd must be a plan of its own"),
        "{error}"
    );
    let host = home();
    let mut session = session_in("/home/ana");
    let run = ask(&host, &mut session, &text);
    assert!(matches!(run.outcome, Outcome::NoPlan(_)));
    assert_eq!(session.cwd, PathBuf::from("/home/ana"));
}

/// A directory that stops being one after the review, as when it is replaced
/// while the person is deciding.
struct Vanishing {
    inner: FakeHost,
    asked: std::cell::Cell<usize>,
}

impl Host for Vanishing {
    fn place_roots(&self, place: Place) -> Vec<PathBuf> {
        self.inner.place_roots(place)
    }
    fn home(&self) -> Option<PathBuf> {
        self.inner.home()
    }
    fn canonicalize(&self, path: &Path, follow_last: bool) -> PathBuf {
        self.inner.canonicalize(path, follow_last)
    }
    fn is_dir(&self, path: &Path) -> bool {
        let first = self.asked.get() == 0;
        self.asked.set(self.asked.get() + 1);
        first && self.inner.is_dir(path)
    }
    fn same(&self, a: &Path, b: &Path) -> bool {
        self.inner.same(a, b)
    }
    fn exists(&self, path: &Path) -> bool {
        self.inner.exists(path)
    }
    fn search(&self, roots: &[PathBuf], needle: &str) -> Result<crate::ports::Search, String> {
        self.inner.search(roots, needle)
    }
    fn allows(&self, access: Access, path: &Path) -> bool {
        self.inner.allows(access, path)
    }
    fn execute(&self, action: &Action) -> Result<crate::ports::Output, String> {
        self.inner.execute(action)
    }
}

#[test]
fn a_cd_is_checked_again_when_it_runs_and_never_enters_what_is_no_longer_a_directory() {
    let host = Vanishing {
        inner: home(),
        asked: std::cell::Cell::new(0),
    };
    let mut session = session_in("/home/ana");
    let text = one_step("session.cd", &path_args("Downloads"));
    let (run, _) = crate::pipeline_tests::run_in_host(
        &mut session,
        &[&text],
        &host,
        &Authority::full(user(false)),
    );
    assert_eq!(session.cwd, PathBuf::from("/home/ana"));
    assert!(
        run.outcome.render().contains("no longer a directory"),
        "{}",
        run.outcome.render()
    );
}

#[test]
fn looking_at_a_link_means_where_it_leads_so_a_hidden_target_still_asks() {
    let host = home()
        .link("/home/ana/Downloads/x", "/home/ana/.ssh")
        .dir("/home/ana/.ssh");
    let mut answers = Answers::yes();
    answers.plan = false;
    let (run, _) = run_in(
        &mut session_in("/home/ana"),
        &[&one_step("fs.list", &path_args("Downloads/x"))],
        &host,
        &Authority::full(user(false)),
        answers,
        MemoryLog::default(),
    );
    assert_eq!(exact(&run), r#"fs.list path="/home/ana/.ssh""#);
    assert!(
        matches!(run.outcome, Outcome::Declined(_)),
        "{:?}",
        run.outcome
    );
    // Removing it still names the link.
    let run = ask(
        &host,
        &mut session_in("/home/ana"),
        &one_step("fs.delete", &path_args("Downloads/x")),
    );
    assert_eq!(exact(&run), r#"fs.delete path="/home/ana/Downloads/x""#);
}

#[test]
fn every_ambiguous_candidate_can_be_picked_however_deep_it_sits() {
    let names: Vec<String> = (0..20)
        .map(|n| format!("/home/ana/p{n}/a/b/c/d/e/f/g/h/i/j/k/l/m/index.js"))
        .collect();
    let files: Vec<(&str, u64)> = names.iter().map(|n| (n.as_str(), 1)).collect();
    let host = FakeHost::with_files(&files);
    let text = one_step(
        "fs.read",
        r#"{"target":{"under_home":true,"name":"index.js","kind":"file"}}"#,
    );
    let run = ask(&host, &mut session_in("/home/ana"), &text);
    let lines = &run.answers.questions[0].1;
    let picks: Vec<usize> = lines.iter().filter_map(|l| l.option).collect();
    assert_eq!(picks, (0..20).collect::<Vec<_>>());
    assert!(!lines.iter().any(|l| l.text.contains("more (")));
}

#[test]
fn a_cd_to_a_file_to_nothing_or_to_what_the_user_cannot_enter_is_refused_and_stays_put() {
    let host = home().forbid(Access::Read, "/etc");
    for (target, why) in [
        ("Downloads/notes.txt", "is not a directory"),
        ("/etc", "do not allow read"),
    ] {
        let mut session = session_in("/home/ana");
        let run = ask(
            &host,
            &mut session,
            &one_step("session.cd", &path_args(target)),
        );
        assert_eq!(session.cwd, PathBuf::from("/home/ana"), "{target}");
        assert!(
            run.outcome.render().contains(why),
            "{target}: {}",
            run.outcome.render()
        );
    }
    let mut session = session_in("/home/ana");
    let run = ask(
        &host,
        &mut session,
        &one_step("session.cd", &path_args("nowhere")),
    );
    assert_eq!(session.cwd, PathBuf::from("/home/ana"));
    assert!(matches!(
        reviewed(&run.outcome).steps[0].blocked,
        Some(Refusal::Unresolved(ResolveError::NotFound(_)))
    ));
}

#[test]
fn a_cd_needs_no_capability_never_asks_and_is_shown_without_one() {
    let host = home();
    let authority = Authority {
        user: user(false),
        capabilities: BTreeSet::<Capability>::new(),
    };
    let mut session = session_in("/home/ana");
    let mut answers = Answers::yes();
    answers.plan = false;
    let (run, _) = run_in(
        &mut session,
        &[&one_step("session.cd", &path_args("Downloads"))],
        &host,
        &authority,
        answers,
        MemoryLog::default(),
    );
    assert!(matches!(
        run.outcome,
        Outcome::Executed { asked: false, .. }
    ));
    assert!(run.answers.screens.is_empty());
    assert_eq!(session.cwd, PathBuf::from("/home/ana/Downloads"));
    let screen = screen::plan_screen(reviewed(&run.outcome), View::Friendly);
    assert!(screen.contains("Go to \"Downloads\""), "{screen}");
    assert!(screen.contains("  -  "), "{screen}");
    assert!(Action::Cd { path: "/x".into() }.capability().is_none());
}

#[test]
fn a_cd_is_not_a_read_to_expand_a_reference_and_never_runs_on_the_host() {
    let host = home();
    ask(
        &host,
        &mut session_in("/home/ana"),
        &one_step("session.cd", &path_args("Downloads")),
    );
    assert!(host.executed.borrow().is_empty());
}

#[test]
fn a_plan_of_the_previous_schema_version_is_refused_as_version_1_was() {
    let old = r#"{"kind":"plan","version":2,"summary":"s","steps":[
      {"id":"s1","action":"fs.list","args":{"target":{"place":"downloads"}},"depends_on":[],"elevate":false}]}"#;
    let error = parse_checked(old).unwrap_err().to_string();
    assert_eq!(error, "unsupported version 2");
    let host = home();
    let (run, planner) = run_with(
        &[old, old],
        &host,
        &Authority::full(user(false)),
        Answers::yes(),
        MemoryLog::default(),
    );
    assert!(matches!(run.outcome, Outcome::NoPlan(_)));
    // The model is shown why, so its second attempt can be a version 3 plan.
    assert_eq!(
        planner.retries.borrow().as_slice(),
        [None, Some("unsupported version 2".to_owned())]
    );
    assert!(host.executed.borrow().is_empty());
    for gone in [
        r#"{"place":"everywhere"}"#,
        r#"{"name":"a","in":"everywhere","kind":"file"}"#,
    ] {
        let text = one_step("fs.list", &format!(r#"{{"target":{gone}}}"#));
        assert!(parse_checked(&text).is_err(), "{gone}");
    }
}

#[test]
fn a_root_action_on_a_relative_path_shows_the_exact_absolute_location() {
    let host = home();
    let mut session = session_in("/var/log");
    let text = plan(&[step("s1", "fs.delete", &path_args("old.log"), "true")]);
    let (run, _) = run_in(
        &mut session,
        &[&text],
        &host,
        &Authority::full(user(true)),
        Answers::yes(),
        MemoryLog::default(),
    );
    let panel = &run.answers.root_asked[0];
    assert!(panel.contains(r#"Action  Delete "old.log""#), "{panel}");
    assert!(
        panel.contains(r#"Exact   fs.delete path="/var/log/old.log""#),
        "{panel}"
    );
    let (friendly, details) = &run.answers.screens[0];
    assert!(!friendly.contains("/var/log/old.log"), "{friendly}");
    assert!(
        details.contains(r#"exact: fs.delete path="/var/log/old.log""#),
        "{details}"
    );
    assert!(matches!(
        &run.outcome,
        Outcome::Executed { steps, .. } if matches!(steps[0].items[0].result, ItemResult::Failed(_) | ItemResult::Done(_))
    ));
}

// ---- the tree ----

fn entry(path: &str, kind: EntryKind, size: Option<u64>) -> TreeEntry {
    TreeEntry {
        path: PathBuf::from(path),
        kind,
        size,
        readable: true,
        protected: false,
    }
}

fn file(path: &str, size: u64) -> TreeEntry {
    entry(path, EntryKind::File, Some(size))
}

fn folder(path: &str) -> TreeEntry {
    entry(path, EntryKind::Dir, None)
}

fn labels() -> Labels {
    Labels::new(Path::new("/h/Downloads"), Some(PathBuf::from("/h")))
}

fn drawn(root: &str, entries: &[TreeEntry], marks: &TreeMarks, options: &TreeOptions) -> String {
    screen::tree(Path::new(root), entries, &[], &labels(), marks, options)
        .into_iter()
        .map(|l| l.text)
        .collect::<Vec<_>>()
        .join("\n")
}

fn default_tree(entries: &[TreeEntry]) -> String {
    drawn(
        "/h/Downloads",
        entries,
        &TreeMarks::default(),
        &TreeOptions::default(),
    )
}

#[test]
fn the_tree_lists_folders_first_then_names_ignoring_case_with_sizes_right_aligned() {
    let shown = default_tree(&[
        file("/h/Downloads/b.txt", 5_000),
        file("/h/Downloads/A.txt", 12),
        folder("/h/Downloads/Zeta"),
        file("/h/Downloads/Zeta/disk.img", 900_000_000),
        folder("/h/Downloads/alpha"),
    ]);
    let expected = [
        " ~/Downloads               2 folders",
        " ├─ alpha/",
        " ├─ Zeta/",
        " │  └─ disk.img  900.0 MB",
        " ├─ A.txt            12 B",
        " └─ b.txt            5 KB",
    ]
    .join("\n");
    assert_eq!(shown, expected, "\n{shown}");
}

#[test]
fn the_tree_stops_at_depth_two_and_says_that_folders_may_hold_more() {
    let shown = default_tree(&[
        folder("/h/Downloads/a"),
        folder("/h/Downloads/a/b"),
        file("/h/Downloads/a/b/c.txt", 1),
    ]);
    assert!(shown.contains("└─ b/"), "{shown}");
    assert!(!shown.contains("c.txt"), "{shown}");
    assert!(shown.contains("may hold more"), "{shown}");
    let deeper = TreeOptions {
        depth: 3,
        ..TreeOptions::default()
    };
    let shown = drawn(
        "/h/Downloads",
        &[
            folder("/h/Downloads/a"),
            folder("/h/Downloads/a/b"),
            file("/h/Downloads/a/b/c.txt", 1),
        ],
        &TreeMarks::default(),
        &deeper,
    );
    assert!(shown.contains("c.txt"), "{shown}");
    assert!(!shown.contains("may hold more"), "{shown}");
}

#[test]
fn sizes_never_print_as_a_thousand_of_the_unit_below() {
    let shown = default_tree(&[file("/h/Downloads/a", 999_999), file("/h/Downloads/b", 999)]);
    assert!(shown.contains("1.0 MB"), "{shown}");
    assert!(!shown.contains("1000"), "{shown}");
    assert!(shown.contains("999 B"), "{shown}");
}

#[test]
fn the_tree_cuts_at_the_line_limit_and_says_how_many_it_left_out() {
    let entries: Vec<TreeEntry> = (0..10)
        .map(|n| file(&format!("/h/Downloads/f{n}"), 1))
        .collect();
    let options = TreeOptions {
        max_lines: 3,
        ..TreeOptions::default()
    };
    let shown = drawn("/h/Downloads", &entries, &TreeMarks::default(), &options);
    assert_eq!(shown.lines().count(), 1 + 3 + 1, "{shown}");
    assert!(
        shown.ends_with("… 7 more (use a path or --depth to open)"),
        "{shown}"
    );
    let all = default_tree(&entries);
    assert!(!all.contains("more ("), "{all}");
}

#[test]
fn the_default_limit_is_two_hundred_lines() {
    let entries: Vec<TreeEntry> = (0..250)
        .map(|n| file(&format!("/h/Downloads/f{n:03}"), 1))
        .collect();
    let shown = default_tree(&entries);
    assert_eq!(shown.lines().count(), 1 + 200 + 1);
    assert!(shown.contains("… 50 more"), "{shown}");
}

#[test]
fn hidden_entries_are_counted_and_listed_only_when_asked_and_always_marked() {
    let entries = [
        file("/h/Downloads/.secret", 1),
        folder("/h/Downloads/.cache"),
        file("/h/Downloads/.cache/x", 1),
        file("/h/Downloads/plain", 1),
    ];
    let hidden_away = default_tree(&entries);
    assert!(hidden_away.contains("2 hidden"), "{hidden_away}");
    assert!(
        !hidden_away.contains(".secret") && !hidden_away.contains(".cache"),
        "{hidden_away}"
    );
    let asked = TreeOptions {
        show_hidden: true,
        ..TreeOptions::default()
    };
    let shown = drawn("/h/Downloads", &entries, &TreeMarks::default(), &asked);
    assert!(
        shown.contains(".cache/") && shown.contains("hidden, asks to open"),
        "{shown}"
    );
    assert_eq!(
        shown
            .lines()
            .filter(|l| l.contains("hidden, asks to open"))
            .count(),
        2,
        "{shown}"
    );
}

#[test]
fn the_tree_marks_system_links_unreadable_and_protected_entries() {
    let mut locked = folder("/etc/ssl");
    locked.readable = false;
    let mut protected = folder("/etc/skel");
    protected.protected = true;
    let link = entry("/etc/alt", EntryKind::Link(PathBuf::from("/usr/alt")), None);
    let shown = drawn(
        "/etc",
        &[locked, protected, link, file("/etc/hosts", 1)],
        &TreeMarks::default(),
        &TreeOptions::default(),
    );
    assert!(
        shown.lines().next().unwrap().contains("(system)"),
        "{shown}"
    );
    assert!(
        shown.contains("ssl/") && shown.contains("unreadable"),
        "{shown}"
    );
    assert!(
        shown.contains("skel/") && shown.contains("protected"),
        "{shown}"
    );
    assert!(
        shown.contains("alt -> /usr/alt") && shown.contains("link"),
        "{shown}"
    );
    // A link is never opened, even when entries below it are known.
    let shown = drawn(
        "/etc",
        &[entry(
            "/etc/alt",
            EntryKind::Link(PathBuf::from("/usr/alt")),
            None,
        )],
        &TreeMarks::default(),
        &TreeOptions::default(),
    );
    assert!(!shown.contains("may hold more"), "{shown}");
}

#[test]
fn a_system_folder_below_the_home_listing_is_marked_where_it_starts() {
    let shown = drawn(
        "/",
        &[folder("/etc"), file("/etc/hosts", 1), folder("/h")],
        &TreeMarks::default(),
        &TreeOptions::default(),
    );
    let etc = shown.lines().find(|l| l.contains("etc/")).unwrap();
    assert!(etc.contains("system"), "{shown}");
    let hosts = shown.lines().find(|l| l.contains("hosts")).unwrap();
    assert!(!hosts.contains("system"), "{shown}");
    let home = shown
        .lines()
        .find(|l| l.contains(" h/") || l.ends_with(" h/"))
        .unwrap();
    assert!(!home.contains("system"), "{shown}");
}

#[test]
fn the_tree_marks_where_a_plan_would_ask_or_hold_an_action_for_root() {
    let mut marks = TreeMarks::default();
    marks.asks.insert(PathBuf::from("/h/Downloads/old.mkv"));
    marks.root.insert(PathBuf::from("/h/Downloads/disk.img"));
    let shown = drawn(
        "/h/Downloads",
        &[
            file("/h/Downloads/old.mkv", 2_000_000_000),
            file("/h/Downloads/disk.img", 900_000_000),
        ],
        &marks,
        &TreeOptions::default(),
    );
    let old = shown.lines().find(|l| l.contains("old.mkv")).unwrap();
    let disk = shown.lines().find(|l| l.contains("disk.img")).unwrap();
    assert!(old.contains("⚠ asks") && !old.contains("ROOT"), "{shown}");
    assert!(disk.contains("ROOT") && !disk.contains("asks"), "{shown}");
}

#[test]
fn a_review_marks_the_locations_it_would_ask_about_or_hold_for_root() {
    let host = home();
    let text = plan(&[step(
        "s1",
        "fs.delete",
        &path_args("~/Downloads/notes.txt"),
        "true",
    )]);
    let (run, _) = run_with(
        &[&text],
        &host,
        &Authority::full(user(true)),
        Answers::yes(),
        MemoryLog::default(),
    );
    let marks = screen::marks_of(reviewed(&run.outcome));
    assert!(
        marks
            .root
            .contains(Path::new("/home/ana/Downloads/notes.txt"))
    );
    assert!(marks.asks.is_empty());
}

#[test]
fn an_ambiguity_shows_the_candidates_in_their_parent_context_and_only_they_are_selectable() {
    let host = FakeHost::with_files(&[
        ("/home/ana/Documents/report.pdf", 1),
        ("/home/ana/Downloads/report.pdf", 2),
    ])
    .dir("/home/ana/Documents")
    .dir("/home/ana/Downloads");
    let text = one_step(
        "fs.read",
        r#"{"target":{"under_home":true,"name":"report.pdf","kind":"file"}}"#,
    );
    let run = ask(&host, &mut session_in("/home/ana"), &text);
    let (_, lines) = &run.answers.questions[0];
    let shown: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(
        shown,
        [
            " ~                   2 folders",
            " ├─ Documents/",
            " │  └─ report.pdf",
            " └─ Downloads/",
            "    └─ report.pdf",
        ],
        "{shown:#?}"
    );
    let selectable: Vec<Option<usize>> = lines.iter().map(|l| l.option).collect();
    assert_eq!(selectable, [None, None, Some(0), None, Some(1)]);
}

#[test]
fn a_listing_prints_as_a_tree_and_a_large_result_marks_what_a_plan_asks_about() {
    let host = home();
    let text = plan(&[step("s1", "fs.list", &path_args("~/Downloads"), "false")]);
    let run = ask(&host, &mut session_in("/home/ana"), &text);
    let shown = run.outcome.render();
    assert!(shown.contains(" Downloads  "), "{shown}");
    assert!(
        shown.contains("├─ films/") || shown.contains("└─ films/"),
        "{shown}"
    );
    assert!(
        shown.contains("old.mkv") && shown.contains("2 KB"),
        "{shown}"
    );
}

#[test]
fn a_find_largest_result_is_a_tree_around_the_files_found() {
    let host = home();
    let text = one_step(
        "fs.find_largest",
        r#"{"target":{"path":"~/Downloads"},"count":2}"#,
    );
    let run = ask(&host, &mut session_in("/home/ana"), &text);
    let shown = run.outcome.render();
    assert!(shown.contains("films/"), "{shown}");
    assert!(
        shown.contains("new.mkv") && shown.contains("old.mkv"),
        "{shown}"
    );
    assert!(!shown.contains("notes.txt"), "{shown}");
}

#[test]
fn what_a_listing_shows_below_its_children_never_feeds_a_from_step_reference() {
    let host = home().listing(
        "/home/ana/Downloads",
        &["/home/ana/Downloads/films", "/home/ana/Downloads/notes.txt"],
    );
    let text = plan(&[
        step("s1", "fs.list", &path_args("~/Downloads"), "false"),
        r#"{"id":"s2","action":"fs.delete","args":{"target":{"from_step":"s1"}},"depends_on":["s1"],"elevate":false}"#
            .to_owned(),
    ]);
    let run = ask(&host, &mut session_in("/home/ana"), &text);
    let deletes: Vec<String> = reviewed(&run.outcome).steps[1]
        .items
        .iter()
        .map(|i| i.action.describe())
        .collect();
    assert_eq!(
        deletes,
        [
            r#"fs.delete path="/home/ana/Downloads/films""#,
            r#"fs.delete path="/home/ana/Downloads/notes.txt""#
        ]
    );
    // The tree the listing carries does reach below them.
    let Some(Ok(output)) = &reviewed(&run.outcome).steps[0].items[0].output else {
        panic!("the list ran before confirmation")
    };
    assert!(
        output
            .tree
            .iter()
            .any(|e| e.path.ends_with("films/old.mkv"))
    );
}

#[test]
fn a_move_into_a_link_to_a_folder_is_judged_where_the_file_lands() {
    let host = home()
        .link("/home/ana/Downloads/lnk", "/home/ana/.ssh")
        .dir("/home/ana/.ssh");
    let args = r#"{"from":{"path":"Documents/report.pdf"},"to":{"path":"Downloads/lnk"}}"#;
    let run = ask(
        &host,
        &mut session_in("/home/ana"),
        &one_step("fs.move", args),
    );
    assert_eq!(
        exact(&run),
        r#"fs.move from="/home/ana/Documents/report.pdf" to="/home/ana/.ssh/report.pdf""#
    );
    // It is a hidden location, so the plan asks.
    assert!(
        reviewed(&run.outcome).steps[0].items[0]
            .action
            .needs_confirmation()
    );
}

#[test]
fn the_candidates_are_numbered_in_the_order_the_rows_show_them() {
    let host = FakeHost::with_files(&[
        ("/home/ana/Zeta/r.pdf", 1),
        ("/home/ana/alpha/r.pdf", 1),
        ("/home/ana/Beta/r.pdf", 1),
    ])
    .dir("/home/ana/Zeta")
    .dir("/home/ana/alpha")
    .dir("/home/ana/Beta");
    let text = one_step(
        "fs.read",
        r#"{"target":{"under_home":true,"name":"r.pdf","kind":"file"}}"#,
    );
    let mut answers = Answers::yes().picking(&[Some(1)]);
    answers.plan = true;
    let (run, _) = run_in(
        &mut session_in("/home/ana"),
        &[&text],
        &host,
        &Authority::full(user(false)),
        answers,
        MemoryLog::default(),
    );
    let lines = &run.answers.questions[0].1;
    let numbers: Vec<usize> = lines.iter().filter_map(|l| l.option).collect();
    assert_eq!(numbers, [0, 1, 2]);
    // The second row down is Beta, and picking it chooses Beta's file.
    assert_eq!(exact(&run), r#"fs.read path="/home/ana/Beta/r.pdf""#);
}

#[test]
fn writing_through_a_link_is_judged_and_shown_where_the_data_lands() {
    let host = home()
        .link("/home/ana/notes.txt", "/home/ana/.ssh/authorized_keys")
        .dir("/home/ana/.ssh");
    let args = r#"{"target":{"path":"notes.txt"},"content":"x","mode":"append"}"#;
    let run = ask(
        &host,
        &mut session_in("/home/ana"),
        &one_step("fs.write", args),
    );
    assert_eq!(
        exact(&run),
        r#"fs.write path="/home/ana/.ssh/authorized_keys" mode=Append content="x""#
    );
    assert!(
        reviewed(&run.outcome).steps[0].items[0]
            .action
            .needs_confirmation()
    );
}

#[test]
fn a_folder_cannot_be_moved_into_itself_written_in_another_case() {
    let host = FakeHost::with_files(&[("/x/Docs/f", 1)])
        .dir("/x/Docs")
        .alias("/x/docs", "/x/Docs");
    let args = r#"{"from":{"path":"/x/docs"},"to":{"path":"/x/Docs/sub"}}"#;
    let run = ask(&host, &mut session_in("/x"), &one_step("fs.move", args));
    assert!(matches!(
        reviewed(&run.outcome).steps[0].items[0].verdict,
        Verdict::Refused(Refusal::MoveIntoItself(_))
    ));
}
