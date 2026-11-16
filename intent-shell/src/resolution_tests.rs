//! References and the resolver through the whole pipeline (ADR 0014): what
//! happens when a name matches two things, nothing, or something the user may
//! not touch, and that resolving is never a way around the broker.

use std::path::{Path, PathBuf};

use crate::action::Access;
use crate::broker::{Authority, Refusal, Verdict};
use crate::pipeline_tests::{events, files, named, plan, reviewed, run, run_with, step};
use crate::ports::{Host, Search};
use crate::reference::{Direct, Kind, Place, Slot};
use crate::resolve::{Choices, MAX_CANDIDATES, ResolveError, locate};
use crate::session::Outcome;
use crate::testing::{Answers, FakeHost, MemoryLog, user};

fn reports() -> FakeHost {
    FakeHost::with_files(&[("/work/report-2023.pdf", 1), ("/work/report-2024.pdf", 2)])
}

fn delete_report() -> String {
    plan(&[step("s1", "fs.delete", &named("report"), "false")])
}

fn run_picking(host: &FakeHost, picks: &[Option<usize>]) -> (crate::pipeline_tests::Run, usize) {
    let (result, planner) = run_with(
        &[&delete_report()],
        host,
        &Authority::full(user(false)),
        Answers::yes().picking(picks),
        MemoryLog::default(),
    );
    let calls = planner.retries.borrow().len();
    (result, calls)
}

#[test]
fn a_name_that_matches_two_files_asks_the_person_with_friendly_options() {
    let host = reports();
    let (result, calls) = run_picking(&host, &[Some(1)]);
    assert_eq!(result.answers.questions.len(), 1);
    let (question, options) = &result.answers.questions[0];
    assert!(question.contains("s1"));
    assert_eq!(
        options,
        &[
            r#""report-2023.pdf" (Downloads)"#.to_owned(),
            r#""report-2024.pdf" (Downloads)"#.to_owned()
        ]
    );
    assert!(!question.contains("/work") && options.iter().all(|o| !o.contains("/work")));
    // The pick is the person's: logged, never sent back to the model.
    assert_eq!(calls, 1);
    let choice = &events(&result, "choice")[0];
    assert_eq!(choice["chosen"], "\"/work/report-2024.pdf\"");
    assert_eq!(
        host.executed.borrow().as_slice(),
        [r#"fs.delete path="/work/report-2024.pdf""#],
        "only the chosen one runs"
    );
    let review = reviewed(&result.outcome);
    assert_eq!(
        review.steps[0].items[0].action.describe(),
        r#"fs.delete path="/work/report-2024.pdf""#
    );
}

#[test]
fn the_chosen_resource_is_judged_by_the_broker_after_the_pick() {
    let host = reports().forbid(Access::Delete, "/work/report-2024.pdf");
    let (forbidden, _) = run_picking(&host, &[Some(1)]);
    assert!(matches!(
        reviewed(&forbidden.outcome).steps[0].items[0].verdict,
        Verdict::Refused(Refusal::PermissionDenied { .. })
    ));
    assert!(matches!(forbidden.outcome, Outcome::AllRefused(_)));
    let (allowed, _) = run_picking(&host, &[Some(0)]);
    assert_eq!(
        reviewed(&allowed.outcome).steps[0].items[0].verdict,
        Verdict::Allowed
    );
}

#[test]
fn cancelling_the_choice_runs_nothing() {
    let host = reports();
    for picks in [vec![None], vec![Some(7)], vec![]] {
        let (result, _) = run_picking(&host, &picks);
        assert!(matches!(result.outcome, Outcome::Declined(_)), "{picks:?}");
        assert!(host.executed.borrow().is_empty());
        assert!(events(&result, "plan_confirmation").is_empty());
        assert_eq!(
            events(&result, "choice")[0]["chosen"],
            serde_json::Value::Null
        );
    }
}

#[test]
fn a_name_that_matches_nothing_is_refused_and_nothing_is_guessed() {
    let host = files();
    let text = format!(
        r#"{{"kind":"plan","version":2,"summary":"x","steps":[
      {{"id":"s1","action":"fs.delete","args":{},"depends_on":[],"elevate":false}},
      {{"id":"s2","action":"fs.stat","args":{},"depends_on":["s1"],"elevate":false}},
      {{"id":"s3","action":"fs.stat","args":{},"depends_on":[],"elevate":false}}]}}"#,
        named("missing"),
        named("b"),
        named("a")
    );
    let result = run(&[&text], &host, false);
    let review = reviewed(&result.outcome);
    assert!(matches!(
        review.steps[0].blocked,
        Some(Refusal::Unresolved(ResolveError::NotFound(_)))
    ));
    assert!(matches!(
        review.steps[1].blocked,
        Some(Refusal::DependencyRefused(_))
    ));
    assert!(result.answers.questions.is_empty());
    // The independent step still runs once the person confirms.
    assert_eq!(
        host.executed.borrow().as_slice(),
        [r#"fs.stat path="/work/a""#]
    );
}

#[test]
fn more_matches_than_the_limit_is_not_a_choice() {
    let many = |n: usize| {
        let files: Vec<(String, u64)> = (0..n)
            .map(|i| (format!("/work/report-{i:02}"), 1))
            .collect();
        let borrowed: Vec<(&str, u64)> = files.iter().map(|(p, s)| (p.as_str(), *s)).collect();
        FakeHost::with_files(&borrowed)
    };
    let (result, _) = run_picking(&many(MAX_CANDIDATES + 1), &[Some(0)]);
    assert!(matches!(
        reviewed(&result.outcome).steps[0].blocked,
        Some(Refusal::Unresolved(ResolveError::TooMany { .. }))
    ));
    assert!(result.answers.questions.is_empty());
    let (result, _) = run_picking(&many(MAX_CANDIDATES), &[Some(0)]);
    assert_eq!(result.answers.questions[0].1.len(), MAX_CANDIDATES);
}

fn located(host: &FakeHost, name: &str, kind: Kind) -> Result<Vec<PathBuf>, ResolveError> {
    let direct = Direct::Named {
        name: name.to_owned(),
        within: Place::Downloads,
        kind,
    };
    let step = crate::plan::StepId::parse("s1").unwrap();
    locate(host, &Choices::default(), &step, Slot::Primary, &direct).map(|l| l.paths)
}

#[test]
fn the_whole_name_wins_ignoring_case_and_a_partial_name_is_the_fallback() {
    let host = FakeHost::with_files(&[
        ("/work/Report", 1),
        ("/work/report.pdf", 1),
        ("/work/reports", 1),
    ]);
    assert_eq!(
        located(&host, "report", Kind::Any).unwrap(),
        [PathBuf::from("/work/Report")]
    );
    assert!(
        matches!(located(&host, "repo", Kind::Any), Err(ResolveError::Ambiguous(a)) if a.candidates.len() == 3)
    );
    assert!(matches!(
        located(&host, "zzz", Kind::Any),
        Err(ResolveError::NotFound(_))
    ));
}

#[test]
fn the_kind_filters_files_from_folders() {
    let host = FakeHost::with_files(&[("/work/photos.txt", 1)]).dir("/work/photos");
    assert_eq!(
        located(&host, "photos", Kind::Folder).unwrap(),
        [PathBuf::from("/work/photos")]
    );
    assert_eq!(
        located(&host, "photos", Kind::File).unwrap(),
        [PathBuf::from("/work/photos.txt")]
    );
    assert!(
        located(&host, "photos", Kind::Any).is_err()
            || located(&host, "photos", Kind::Any).unwrap().len() == 1
    );
}

#[test]
fn a_file_the_user_may_not_touch_resolves_and_the_broker_refuses_it() {
    let host = FakeHost::with_files(&[("/work/locked", 1)])
        .forbid(Access::Read, "/work/locked")
        .forbid(Access::Delete, "/work/locked");
    for action in ["fs.read", "fs.delete"] {
        let text = plan(&[step("s1", action, &named("locked"), "false")]);
        let result = run(&[&text], &host, false);
        let Outcome::AllRefused(review) = &result.outcome else {
            panic!("{:?}", result.outcome)
        };
        assert!(
            matches!(
                review.steps[0].items[0].verdict,
                Verdict::Refused(Refusal::PermissionDenied { .. })
            ),
            "{action}"
        );
    }
    assert!(!host.searched.borrow().is_empty(), "the resolver did look");
}

#[test]
fn a_step_result_that_is_the_filesystem_root_still_meets_the_protected_path_rule() {
    let host = FakeHost::with_files(&[("/", 5)]).place(Place::Downloads, "/");
    let text = r#"{"kind":"plan","version":2,"summary":"x","steps":[
      {"id":"s1","action":"fs.list","args":{"target":{"place":"downloads"}},"depends_on":[],"elevate":false},
      {"id":"s2","action":"fs.delete","args":{"target":{"from_step":"s1"}},"depends_on":["s1"],"elevate":true}]}"#;
    let result = run(&[text], &host, true);
    assert!(matches!(
        reviewed(&result.outcome).steps[1].items[0].verdict,
        Verdict::Refused(Refusal::ProtectedPath(_))
    ));
}

#[test]
fn a_place_at_the_root_gives_children_never_the_root_itself() {
    let host = FakeHost::with_files(&[("/a", 1)]).place(Place::Documents, "/");
    let text = plan(&[step(
        "s1",
        "fs.move",
        r#"{"from":{"name":"a","in":"documents","kind":"file"},"to":{"place":"documents"}}"#,
        "true",
    )]);
    let result = run(&[&text], &host, true);
    let action = &reviewed(&result.outcome).steps[0].items[0].action;
    assert_eq!(action.describe(), r#"fs.move from="/a" to="/a""#);
    let text = plan(&[step(
        "s1",
        "fs.write",
        r#"{"target":{"new_name":"x","in":"documents"},"content":"c","mode":"create"}"#,
        "true",
    )]);
    let result = run(&[&text], &host, true);
    assert_eq!(
        reviewed(&result.outcome).steps[0].items[0]
            .action
            .describe(),
        r#"fs.write path="/x" mode=Create content="c""#
    );
}

#[test]
fn a_name_that_says_where_to_look_never_reaches_the_host() {
    for bad in ["../etc/passwd", "a/b", "..", ".", "/etc"] {
        let host = files();
        let text = plan(&[step("s1", "fs.delete", &named(bad), "false")]);
        let result = run(&[&text, &text], &host, true);
        assert!(matches!(result.outcome, Outcome::NoPlan(_)), "{bad}");
        assert!(host.searched.borrow().is_empty(), "{bad}");
        assert!(host.executed.borrow().is_empty(), "{bad}");
    }
}

struct Lying(FakeHost);

impl Host for Lying {
    fn place_roots(&self, place: Place) -> Vec<PathBuf> {
        self.0.place_roots(place)
    }

    fn search(&self, _: &[PathBuf], _: &str) -> Result<Search, String> {
        Ok(Search {
            entries: vec![crate::ports::Entry {
                path: PathBuf::from("/work/unrelated"),
                is_dir: false,
            }],
            notes: Vec::new(),
        })
    }

    fn allows(&self, access: Access, path: &Path) -> bool {
        self.0.allows(access, path)
    }

    fn execute(&self, action: &crate::action::Action) -> Result<crate::ports::Output, String> {
        self.0.execute(action)
    }
}

#[test]
fn entries_that_do_not_contain_the_name_are_not_trusted() {
    let host = Lying(files());
    let step_id = crate::plan::StepId::parse("s1").unwrap();
    let direct = Direct::Named {
        name: "zzz".into(),
        within: Place::Downloads,
        kind: Kind::Any,
    };
    let found = locate(&host, &Choices::default(), &step_id, Slot::Primary, &direct);
    assert!(matches!(found, Err(ResolveError::NotFound(_))), "{found:?}");
}

#[test]
fn a_pick_that_was_not_offered_is_ignored() {
    let host = reports();
    let step_id = crate::plan::StepId::parse("s1").unwrap();
    let direct = Direct::Named {
        name: "report".into(),
        within: Place::Downloads,
        kind: Kind::File,
    };
    let mut choices = Choices::default();
    choices.pick(step_id.clone(), Slot::Primary, PathBuf::from("/etc/shadow"));
    assert!(matches!(
        locate(&host, &choices, &step_id, Slot::Primary, &direct),
        Err(ResolveError::Ambiguous(_))
    ));
    choices.pick(
        step_id.clone(),
        Slot::Primary,
        PathBuf::from("/work/report-2023.pdf"),
    );
    assert_eq!(
        locate(&host, &choices, &step_id, Slot::Primary, &direct)
            .unwrap()
            .paths,
        [PathBuf::from("/work/report-2023.pdf")]
    );
}

#[test]
fn a_name_search_is_logged_before_it_runs() {
    let host = files();
    let text = plan(&[step("s1", "fs.stat", &named("a"), "false")]);
    let result = run(&[&text], &host, false);
    let names: Vec<_> = result
        .log
        .events
        .iter()
        .map(|e| e["event"].as_str().unwrap())
        .collect();
    assert_eq!(
        &names[..4],
        ["request", "model_output", "plan_accepted", "search"]
    );
    assert_eq!(events(&result, "search")[0]["needle"], "a");
}

#[test]
fn nothing_is_searched_while_the_log_cannot_record_the_search() {
    let host = files();
    let text = plan(&[step("s1", "fs.stat", &named("a"), "false")]);
    let (result, _) = run_with(
        &[&text],
        &host,
        &Authority::full(user(false)),
        Answers::yes(),
        MemoryLog::failing_after(3),
    );
    assert!(host.searched.borrow().is_empty());
    assert!(host.executed.borrow().is_empty());
    assert!(matches!(result.outcome, Outcome::LogFailed { .. }));
}

#[test]
fn a_failed_search_is_a_refusal_not_an_empty_success() {
    let mut host = files();
    host.search_fails = true;
    let text = plan(&[step("s1", "fs.stat", &named("a"), "false")]);
    let result = run(&[&text], &host, false);
    assert!(matches!(
        reviewed(&result.outcome).steps[0].blocked,
        Some(Refusal::Unresolved(ResolveError::SearchFailed(_)))
    ));
    assert!(matches!(result.outcome, Outcome::AllRefused(_)));
}

#[test]
fn a_new_item_needs_one_place_to_live_in() {
    let text = plan(&[step(
        "s1",
        "fs.write",
        r#"{"target":{"new_name":"x","in":"everywhere"},"content":"c","mode":"create"}"#,
        "false",
    )]);
    let result = run(&[&text], &files(), false);
    assert!(matches!(
        reviewed(&result.outcome).steps[0].blocked,
        Some(Refusal::Unresolved(ResolveError::CannotCreateThere(_)))
    ));
}

#[test]
fn a_move_into_a_folder_keeps_the_name_and_a_move_to_a_new_name_does_not() {
    let host = files().dir("/docs/box");
    let into = plan(&[step(
        "s1",
        "fs.move",
        r#"{"from":{"name":"a","in":"downloads","kind":"file"},"to":{"name":"box","in":"documents","kind":"folder"}}"#,
        "false",
    )]);
    let result = run(&[&into], &host, false);
    assert_eq!(
        reviewed(&result.outcome).steps[0].items[0]
            .action
            .describe(),
        r#"fs.move from="/work/a" to="/docs/box/a""#
    );
    let rename = plan(&[step(
        "s1",
        "fs.move",
        r#"{"from":{"name":"a","in":"downloads","kind":"file"},"to":{"new_name":"b2","in":"documents"}}"#,
        "false",
    )]);
    let result = run(&[&rename], &host, false);
    assert_eq!(
        reviewed(&result.outcome).steps[0].items[0]
            .action
            .describe(),
        r#"fs.move from="/work/a" to="/docs/b2""#
    );
}

#[test]
fn the_default_view_shows_no_location_the_details_do_and_the_root_panel_always_does() {
    let host = files();
    let elevated = plan(&[step("s1", "fs.delete", &named("a"), "true")]);
    let (result, _) = run_with(
        &[&elevated],
        &host,
        &Authority::full(user(true)),
        Answers::yes(),
        MemoryLog::default(),
    );
    let (friendly, details) = &result.answers.screens[0];
    assert!(friendly.contains(r#"Delete "a" (Downloads)"#), "{friendly}");
    assert!(!friendly.contains("/work"), "{friendly}");
    assert!(details.contains(r#"fs.delete path="/work/a""#), "{details}");
    let panel = &result.answers.root_asked[0];
    assert!(
        panel.contains(r#"Delete "a" (Downloads)"#)
            && panel.contains(r#"fs.delete path="/work/a""#),
        "{panel}"
    );
    let refused = {
        let host = files().forbid(Access::Delete, "/work/a");
        let text = plan(&[step("s1", "fs.delete", &named("a"), "false")]);
        run(&[&text], &host, false)
    };
    let screen =
        crate::screen::plan_screen(reviewed(&refused.outcome), crate::screen::View::Friendly);
    assert!(
        screen.contains("REFUSED") && !screen.contains("/work"),
        "{screen}"
    );
}

#[test]
fn an_ambiguity_blocks_its_dependents_until_chosen_and_then_the_chain_expands() {
    let host = FakeHost::with_files(&[("/docs/box1/x", 1), ("/docs/box2/y", 1)])
        .dir("/docs/box1")
        .dir("/docs/box2");
    let text = r#"{"kind":"plan","version":2,"summary":"x","steps":[
      {"id":"s1","action":"fs.list","args":{"target":{"name":"box","in":"documents","kind":"folder"}},"depends_on":[],"elevate":false},
      {"id":"s2","action":"fs.delete","args":{"target":{"from_step":"s1"}},"depends_on":["s1"],"elevate":false}]}"#;
    let (result, _) = run_with(
        &[text],
        &host,
        &Authority::full(user(false)),
        Answers::yes().picking(&[Some(0)]),
        MemoryLog::default(),
    );
    assert_eq!(result.answers.questions.len(), 1);
    let review = reviewed(&result.outcome);
    assert_eq!(
        review.steps[1].items[0].action.describe(),
        r#"fs.delete path="/docs/box1/x""#
    );
    // Declining the same question leaves the dependent blocked and nothing run.
    let (declined, _) = run_with(
        &[text],
        &host,
        &Authority::full(user(false)),
        Answers::yes().picking(&[None]),
        MemoryLog::default(),
    );
    assert!(matches!(declined.outcome, Outcome::Declined(_)));
    assert!(matches!(
        reviewed(&declined.outcome).steps[1].blocked,
        Some(Refusal::DependencyRefused(_))
    ));
}
