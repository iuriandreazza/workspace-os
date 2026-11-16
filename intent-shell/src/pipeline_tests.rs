//! The broker and the whole pipeline, with no model: every plan is a canned
//! string, the filesystem is in memory, and the answers are scripted.
//!
//! These run in the gate. Each refusal class has its own test, so a broker
//! that lets one class through fails exactly there.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::action::{Access, Capability};
use crate::broker::{Authority, MAX_EXPANDED_PATHS, Refusal, Verdict};
use crate::session::{ItemResult, Outcome, Shell};
use crate::testing::{Answers, FakeHost, MemoryLog, ScriptedPlanner, user};

const LARGEST: &str = r#"{"kind":"plan","version":1,"summary":"List the largest files","steps":[
  {"id":"s1","action":"fs.find_largest","args":{"path":"/work","count":2},"depends_on":[],"elevate":false}]}"#;

const DELETE_LARGEST: &str = r#"{"kind":"plan","version":1,"summary":"Delete the two largest","steps":[
  {"id":"s1","action":"fs.find_largest","args":{"path":"/work","count":2},"depends_on":[],"elevate":false},
  {"id":"s2","action":"fs.delete","args":{"path":{"from_step":"s1"}},"depends_on":["s1"],"elevate":false}]}"#;

fn step(id: &str, action: &str, args: &str, elevate: &str) -> String {
    format!(
        r#"{{"id":"{id}","action":"{action}","args":{args},"depends_on":[],"elevate":{elevate}}}"#
    )
}

fn plan(steps: &[String]) -> String {
    format!(
        r#"{{"kind":"plan","version":1,"summary":"test","steps":[{}]}}"#,
        steps.join(",")
    )
}

fn files() -> FakeHost {
    FakeHost::with_files(&[("/work/a", 10), ("/work/b", 300), ("/work/c", 200)])
}

struct Run {
    outcome: Outcome,
    log: MemoryLog,
    answers: Answers,
}

fn run_with(
    outputs: &[&str],
    host: &FakeHost,
    authority: &Authority,
    mut answers: Answers,
    log: MemoryLog,
) -> (Run, ScriptedPlanner) {
    let planner = ScriptedPlanner::new(outputs);
    let mut log = log;
    let outcome = Shell {
        authority,
        host,
        planner: &planner,
        confirm: &mut answers,
        log: &mut log,
    }
    .handle("do it");
    (
        Run {
            outcome,
            log,
            answers,
        },
        planner,
    )
}

fn run(outputs: &[&str], host: &FakeHost, administrator: bool) -> Run {
    run_with(
        outputs,
        host,
        &Authority::full(user(administrator)),
        Answers::yes(),
        MemoryLog::default(),
    )
    .0
}

fn events(run: &Run, name: &str) -> Vec<Value> {
    run.log
        .events
        .iter()
        .filter(|e| e["event"] == name)
        .cloned()
        .collect()
}

fn reviewed(outcome: &Outcome) -> &crate::broker::Review {
    match outcome {
        Outcome::Executed { review, .. }
        | Outcome::AllRefused(review)
        | Outcome::Declined(review) => review,
        other => panic!("no review in {other:?}"),
    }
}

#[test]
fn a_valid_plan_is_validated_confirmed_and_run() {
    let host = files();
    let result = run(&[LARGEST], &host, false);
    let Outcome::Executed { steps, .. } = &result.outcome else {
        panic!("{:?}", result.outcome)
    };
    let ItemResult::Done(output) = &steps[0].items[0].result else {
        panic!("{:?}", steps[0])
    };
    assert_eq!(
        output.paths,
        vec!["/work/b".into(), "/work/c".into()] as Vec<std::path::PathBuf>
    );
    assert_eq!(host.executed.borrow().len(), 1);
}

#[test]
fn every_decision_is_logged_in_order() {
    let result = run(&[LARGEST], &files(), false);
    let names: Vec<_> = result
        .log
        .events
        .iter()
        .map(|e| e["event"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        names,
        [
            "request",
            "model_output",
            "plan_accepted",
            "review",
            "plan_confirmation",
            "execute",
            "result"
        ]
    );
    assert_eq!(events(&result, "model_output")[0]["raw"], LARGEST);
    assert_eq!(
        events(&result, "review")[0]["review"]["steps"][0]["items"][0]["verdict"],
        "allowed"
    );
}

#[test]
fn a_reference_is_expanded_into_each_real_path_before_confirmation() {
    let host = files();
    let result = run(&[DELETE_LARGEST], &host, false);
    let deletes: Vec<_> = reviewed(&result.outcome).steps[1]
        .items
        .iter()
        .map(|i| i.action.describe())
        .collect();
    assert_eq!(
        deletes,
        [r#"fs.delete path="/work/b""#, r#"fs.delete path="/work/c""#]
    );
}

#[test]
fn a_session_without_the_capability_is_refused_even_when_the_user_may() {
    let host = files();
    let capabilities: BTreeSet<_> = Capability::ALL
        .into_iter()
        .filter(|c| *c != Capability::FsRead)
        .collect();
    let authority = Authority {
        user: user(false),
        capabilities,
    };
    let (result, _) = run_with(
        &[LARGEST],
        &host,
        &authority,
        Answers::yes(),
        MemoryLog::default(),
    );
    let Outcome::AllRefused(review) = &result.outcome else {
        panic!("{:?}", result.outcome)
    };
    assert_eq!(
        review.steps[0].items[0].verdict,
        Verdict::Refused(Refusal::CapabilityNotGranted(Capability::FsRead))
    );
    assert!(host.executed.borrow().is_empty());
}

#[test]
fn an_action_beyond_the_users_permissions_is_refused_even_when_the_capability_is_held() {
    let host = files().forbid(Access::Read, "/work");
    let result = run(&[LARGEST], &host, false);
    let Outcome::AllRefused(review) = &result.outcome else {
        panic!("{:?}", result.outcome)
    };
    assert!(matches!(
        review.steps[0].items[0].verdict,
        Verdict::Refused(Refusal::PermissionDenied {
            access: Access::Read,
            ..
        })
    ));
    assert!(host.executed.borrow().is_empty());
}

#[test]
fn failing_the_users_permissions_is_never_promoted_to_root() {
    let host = files().forbid(Access::Delete, "/work/doomed");
    let doomed = plan(&[step(
        "s1",
        "fs.delete",
        r#"{"path":"/work/doomed"}"#,
        "false",
    )]);
    let result = run(&[&doomed], &host, true);
    assert!(matches!(
        reviewed(&result.outcome).steps[0].items[0].verdict,
        Verdict::Refused(Refusal::PermissionDenied { .. })
    ));
    assert!(result.answers.root_asked.is_empty());
}

#[test]
fn a_root_action_is_held_and_runs_only_after_its_own_confirmation() {
    let host = files();
    let elevated = plan(&[step("s1", "fs.delete", r#"{"path":"/work/a"}"#, "true")]);
    let result = run(&[&elevated], &host, true);
    assert_eq!(result.answers.root_asked, [r#"fs.delete path="/work/a""#]);
    assert_eq!(
        host.executed.borrow().as_slice(),
        [r#"fs.delete path="/work/a""#]
    );
    let confirmation = &events(&result, "root_confirmation")[0];
    assert_eq!(confirmation["confirmed_by"], "ana");
    assert_eq!(confirmation["confirmed"], true);
}

#[test]
fn a_root_action_without_confirmation_does_not_run() {
    let host = files();
    let elevated = plan(&[step("s1", "fs.delete", r#"{"path":"/work/a"}"#, "true")]);
    let answers = Answers {
        root: false,
        ..Answers::yes()
    };
    let (result, _) = run_with(
        &[&elevated],
        &host,
        &Authority::full(user(true)),
        answers,
        MemoryLog::default(),
    );
    assert!(host.executed.borrow().is_empty());
    let Outcome::Executed { steps, .. } = &result.outcome else {
        panic!("{:?}", result.outcome)
    };
    assert!(matches!(steps[0].items[0].result, ItemResult::NotRun(_)));
    assert_eq!(events(&result, "root_confirmation")[0]["confirmed"], false);
}

#[test]
fn confirming_the_plan_does_not_confirm_a_root_step() {
    let host = files();
    let elevated = plan(&[step("s1", "fs.delete", r#"{"path":"/work/a"}"#, "true")]);
    let result = run(&[&elevated], &host, true);
    assert_eq!(
        reviewed(&result.outcome).steps[0].items[0].verdict,
        Verdict::NeedsRoot
    );
    assert_eq!(events(&result, "root_confirmation").len(), 1);
}

#[test]
fn a_user_who_is_not_an_administrator_cannot_elevate() {
    let host = files();
    let elevated = plan(&[step("s1", "fs.delete", r#"{"path":"/work/a"}"#, "true")]);
    let result = run(&[&elevated], &host, false);
    let Outcome::AllRefused(review) = &result.outcome else {
        panic!("{:?}", result.outcome)
    };
    assert_eq!(
        review.steps[0].items[0].verdict,
        Verdict::Refused(Refusal::NotAdministrator)
    );
    assert!(host.executed.borrow().is_empty());
}

#[test]
fn actions_that_are_always_root_are_held_for_an_administrator_and_refused_for_others() {
    let add = plan(&[step(
        "s1",
        "user.manage",
        r#"{"operation":"add","user":"bia","group":null}"#,
        "false",
    )
    .replace(r#","elevate":false"#, "")]);
    let held = run(&[&add], &files(), true);
    assert_eq!(
        reviewed(&held.outcome).steps[0].items[0].verdict,
        Verdict::NeedsRoot
    );
    assert_eq!(held.answers.root_asked.len(), 1);
    let refused = run(&[&add], &files(), false);
    assert!(matches!(refused.outcome, Outcome::AllRefused(_)));
}

#[test]
fn system_configure_and_process_run_are_refused_while_their_registries_are_empty() {
    let configure = plan(&[step(
        "s1",
        "system.configure",
        r#"{"key":"net.hostname","value":"x"}"#,
        "false",
    )
    .replace(r#","elevate":false"#, "")]);
    let run_program = plan(&[step(
        "s1",
        "process.run",
        r#"{"program":"ls","argv":["-l"]}"#,
        "false",
    )]);
    for text in [&configure, &run_program] {
        let result = run(&[text], &files(), true);
        let Outcome::AllRefused(review) = &result.outcome else {
            panic!("{:?}", result.outcome)
        };
        assert!(matches!(
            review.steps[0].items[0].verdict,
            Verdict::Refused(Refusal::ConfigKeyNotAllowed(_) | Refusal::ProgramNotRegistered(_))
        ));
    }
}

#[test]
fn malformed_output_runs_nothing_and_is_retried_once_with_the_error() {
    let host = files();
    let (result, planner) = run_with(
        &["not json", "{\"kind\":\"plan\"}"],
        &host,
        &Authority::full(user(false)),
        Answers::yes(),
        MemoryLog::default(),
    );
    let Outcome::NoPlan(why) = &result.outcome else {
        panic!("{:?}", result.outcome)
    };
    assert!(why.contains("2 attempts"));
    assert!(host.executed.borrow().is_empty());
    let retries = planner.retries.borrow();
    assert_eq!(retries.len(), 2);
    assert!(retries[0].is_none() && retries[1].as_deref().unwrap().contains("not JSON"));
    let attempts = events(&result, "model_output");
    assert!(attempts.iter().all(|a| a["accepted"] == false));
    assert_eq!(attempts.len(), 2);
}

#[test]
fn a_valid_retry_is_accepted() {
    let result = run(&["garbage", LARGEST], &files(), false);
    assert!(matches!(result.outcome, Outcome::Executed { .. }));
    assert_eq!(events(&result, "model_output").len(), 2);
}

#[test]
fn a_plan_that_breaks_a_rule_runs_none_of_its_valid_steps() {
    let cyclic = r#"{"kind":"plan","version":1,"summary":"x","steps":[
      {"id":"s1","action":"fs.stat","args":{"path":"/work/a"},"depends_on":[],"elevate":false},
      {"id":"s2","action":"fs.stat","args":{"path":"/work/b"},"depends_on":["s3"],"elevate":false},
      {"id":"s3","action":"fs.stat","args":{"path":"/work/c"},"depends_on":["s2"],"elevate":false}]}"#;
    let host = files();
    let result = run(&[cyclic, cyclic], &host, false);
    assert!(matches!(result.outcome, Outcome::NoPlan(_)));
    assert!(host.executed.borrow().is_empty());
}

#[test]
fn a_question_runs_nothing() {
    let host = files();
    let result = run(
        &[r#"{"kind":"clarify","version":1,"question":"Which directory?"}"#],
        &host,
        false,
    );
    assert_eq!(
        result.outcome,
        Outcome::Clarify("Which directory?".to_owned())
    );
    assert!(host.executed.borrow().is_empty());
}

#[test]
fn declining_the_plan_runs_nothing() {
    let host = files();
    let answers = Answers {
        plan: false,
        ..Answers::yes()
    };
    let (result, _) = run_with(
        &[LARGEST],
        &host,
        &Authority::full(user(false)),
        answers,
        MemoryLog::default(),
    );
    assert!(matches!(result.outcome, Outcome::Declined(_)));
    assert!(host.executed.borrow().is_empty());
    assert_eq!(events(&result, "plan_confirmation")[0]["confirmed"], false);
}

#[test]
fn a_refused_step_stops_its_dependents_and_independent_steps_still_run() {
    let host = files().forbid(Access::Read, "/secret");
    let text = r#"{"kind":"plan","version":1,"summary":"x","steps":[
      {"id":"s1","action":"fs.list","args":{"path":"/secret"},"depends_on":[],"elevate":false},
      {"id":"s2","action":"fs.delete","args":{"path":{"from_step":"s1"}},"depends_on":["s1"],"elevate":false},
      {"id":"s3","action":"fs.stat","args":{"path":"/work/a"},"depends_on":[],"elevate":false}]}"#;
    let result = run(&[text], &host, false);
    let review = reviewed(&result.outcome);
    assert!(matches!(
        review.steps[1].blocked,
        Some(Refusal::DependencyRefused(_))
    ));
    assert_eq!(
        host.executed.borrow().as_slice(),
        ["fs.stat path=\"/work/a\""]
    );
}

#[test]
fn a_reference_to_a_step_that_is_held_cannot_be_expanded() {
    let host = files();
    let text = r#"{"kind":"plan","version":1,"summary":"x","steps":[
      {"id":"s1","action":"fs.find_largest","args":{"path":"/work","count":2},"depends_on":[],"elevate":true},
      {"id":"s2","action":"fs.delete","args":{"path":{"from_step":"s1"}},"depends_on":["s1"],"elevate":false}]}"#;
    let result = run(&[text], &host, true);
    assert!(matches!(
        reviewed(&result.outcome).steps[1].blocked,
        Some(Refusal::CannotExpand(_))
    ));
    assert!(
        host.executed
            .borrow()
            .iter()
            .all(|a| !a.starts_with("fs.delete"))
    );
}

#[test]
fn a_reference_that_expands_past_the_limit_is_refused() {
    let many: Vec<(String, u64)> = (0..=MAX_EXPANDED_PATHS)
        .map(|i| (format!("/big/{i:03}"), 1))
        .collect();
    let borrowed: Vec<(&str, u64)> = many.iter().map(|(p, s)| (p.as_str(), *s)).collect();
    let host = FakeHost::with_files(&borrowed);
    let text = r#"{"kind":"plan","version":1,"summary":"x","steps":[
      {"id":"s1","action":"fs.list","args":{"path":"/big"},"depends_on":[],"elevate":false},
      {"id":"s2","action":"fs.delete","args":{"path":{"from_step":"s1"}},"depends_on":["s1"],"elevate":false}]}"#;
    let result = run(&[text], &host, false);
    assert!(matches!(
        reviewed(&result.outcome).steps[1].blocked,
        Some(Refusal::CannotExpand(_))
    ));
}

#[test]
fn a_string_that_looks_like_a_reference_deletes_only_that_literal_path() {
    let host = files();
    let text = r#"{"kind":"plan","version":1,"summary":"x","steps":[
      {"id":"s1","action":"fs.find_largest","args":{"path":"/work","count":1},"depends_on":[],"elevate":false},
      {"id":"s2","action":"fs.delete","args":{"path":"s1.path"},"depends_on":["s1"],"elevate":false}]}"#;
    let result = run(&[text], &host, false);
    let deletes: Vec<_> = reviewed(&result.outcome).steps[1]
        .items
        .iter()
        .map(|i| i.action.describe())
        .collect();
    assert_eq!(deletes, [r#"fs.delete path="s1.path""#]);
}

#[test]
fn nothing_runs_while_the_log_cannot_be_written() {
    let host = files();
    let log = MemoryLog::failing_after(0);
    let (result, _) = run_with(
        &[LARGEST],
        &host,
        &Authority::full(user(false)),
        Answers::yes(),
        log,
    );
    assert!(matches!(result.outcome, Outcome::LogFailed { .. }));
    assert!(host.executed.borrow().is_empty());
}

#[test]
fn the_rendering_shows_the_broker_action_and_labels_the_models_summary() {
    let host = files();
    let sneaky = r#"{"kind":"plan","version":1,"summary":"Harmless tidy-up","steps":[
      {"id":"s1","action":"fs.delete","args":{"path":"/work/a\u001b[2J"},"depends_on":[],"elevate":false}]}"#;
    let result = run(&[sneaky], &host, false);
    let text = reviewed(&result.outcome).render();
    assert!(text.contains("The model says"));
    assert!(text.contains(r#"fs.delete path="/work/a\u001b[2J""#));
    assert!(!text.contains('\u{1b}'));
}

#[test]
fn nothing_is_read_before_the_plan_is_on_record() {
    // The log takes the request and the model output, then fails.
    let host = files();
    let (result, _) = run_with(
        &[DELETE_LARGEST],
        &host,
        &Authority::full(user(false)),
        Answers::yes(),
        MemoryLog::failing_after(2),
    );
    assert!(matches!(result.outcome, Outcome::LogFailed { .. }));
    assert!(host.executed.borrow().is_empty());
}

#[test]
fn a_log_that_fails_mid_execution_stops_before_the_next_action_and_keeps_what_ran() {
    let host = files();
    let two = plan(&[
        step("s1", "fs.stat", r#"{"path":"/work/a"}"#, "false"),
        step("s2", "fs.stat", r#"{"path":"/work/b"}"#, "false"),
    ]);
    // request, model_output, plan_accepted, review, plan_confirmation, execute s1: then fail on its result.
    let (result, _) = run_with(
        &[&two],
        &host,
        &Authority::full(user(false)),
        Answers::yes(),
        MemoryLog::failing_after(6),
    );
    let Outcome::LogFailed { steps, .. } = &result.outcome else {
        panic!("{:?}", result.outcome)
    };
    assert_eq!(host.executed.borrow().len(), 1);
    assert_eq!(steps.len(), 1);
}

#[test]
fn a_read_that_fails_before_confirmation_leaves_nothing_to_confirm() {
    let host = files().failing("/work");
    let result = run(&[DELETE_LARGEST], &host, false);
    let Outcome::AllRefused(review) = &result.outcome else {
        panic!("{:?}", result.outcome)
    };
    assert!(matches!(
        review.steps[0].blocked,
        Some(Refusal::PreRunFailed(_))
    ));
    assert!(matches!(
        review.steps[1].blocked,
        Some(Refusal::DependencyRefused(_))
    ));
    assert!(!review.render().contains("[ok]"));
    assert!(events(&result, "plan_confirmation").is_empty());
}

#[test]
fn references_chain_from_one_read_to_the_next() {
    let host = files();
    let text = r#"{"kind":"plan","version":1,"summary":"x","steps":[
      {"id":"s1","action":"fs.find_largest","args":{"path":"/work","count":2},"depends_on":[],"elevate":false},
      {"id":"s2","action":"fs.list","args":{"path":{"from_step":"s1"}},"depends_on":["s1"],"elevate":false},
      {"id":"s3","action":"fs.delete","args":{"path":{"from_step":"s2"}},"depends_on":["s2"],"elevate":false}]}"#;
    let result = run(&[text], &host, false);
    let deletes: Vec<_> = reviewed(&result.outcome).steps[2]
        .items
        .iter()
        .map(|i| i.action.describe())
        .collect();
    assert_eq!(
        deletes,
        [r#"fs.delete path="/work/b""#, r#"fs.delete path="/work/c""#]
    );
}

#[test]
fn a_refused_path_among_several_skips_only_that_action_and_the_rest_run() {
    // Per-action decisions inside one step: the rendering labels the refused one,
    // the person confirms the plan knowing it, and its dependents are blocked.
    let host = files().forbid(Access::Delete, "/work/b");
    let result = run(&[DELETE_LARGEST], &host, false);
    let rendering = reviewed(&result.outcome).render();
    assert!(rendering.contains(r#"fs.delete path="/work/b"  [REFUSED"#));
    let executed: Vec<_> = host.executed.borrow().clone();
    assert!(executed.contains(&r#"fs.delete path="/work/c""#.to_owned()));
    assert!(!executed.contains(&r#"fs.delete path="/work/b""#.to_owned()));
}

#[test]
fn a_denied_path_with_control_characters_cannot_forge_report_lines() {
    let host = files().forbid(
        Access::Write,
        "/work/evil\n\u{1b}[2Jstep s9 ok [ok]\u{202e}",
    );
    let text = plan(&[step(
        "s1",
        "fs.write",
        r#"{"path":"/work/evil\n\u001b[2Jstep s9 ok [ok]\u202e","content":"x","mode":"create"}"#,
        "false",
    )]);
    let result = run(&[&text], &host, false);
    let shown = result.outcome.render();
    assert!(!shown.contains('\u{1b}') && !shown.contains('\u{202e}'));
    assert_eq!(
        shown
            .lines()
            .filter(|l| l.trim_start().starts_with("s9"))
            .count(),
        0
    );
}

#[test]
fn a_read_is_logged_before_it_runs_and_not_run_when_the_log_fails_there() {
    // request, model_output and plan_accepted are accepted; the read's own event fails.
    let host = files();
    let (result, _) = run_with(
        &[DELETE_LARGEST],
        &host,
        &Authority::full(user(false)),
        Answers::yes(),
        MemoryLog::failing_after(3),
    );
    assert!(matches!(result.outcome, Outcome::LogFailed { .. }));
    assert!(host.executed.borrow().is_empty());

    let ok = run(&[DELETE_LARGEST], &files(), false);
    let names: Vec<_> = ok
        .log
        .events
        .iter()
        .map(|e| e["event"].as_str().unwrap())
        .collect();
    assert_eq!(
        &names[..5],
        [
            "request",
            "model_output",
            "plan_accepted",
            "pre_run",
            "review"
        ]
    );
}

#[test]
fn model_text_in_an_error_cannot_reach_the_terminal_raw() {
    let hostile =
        "{\"kind\":\"plan\",\"version\":1,\"summary\":\"x\",\"steps\":[],\"\\u001b[2J\\u202e\":1}";
    let result = run(&[hostile, hostile], &files(), false);
    let shown = result.outcome.render();
    assert!(matches!(result.outcome, Outcome::NoPlan(_)));
    assert!(!shown.contains('\u{1b}') && !shown.contains('\u{202e}'));
}

#[test]
fn exactly_the_limit_of_paths_still_expands() {
    let many: Vec<(String, u64)> = (0..MAX_EXPANDED_PATHS)
        .map(|i| (format!("/big/{i:03}"), 1))
        .collect();
    let borrowed: Vec<(&str, u64)> = many.iter().map(|(p, s)| (p.as_str(), *s)).collect();
    let host = FakeHost::with_files(&borrowed);
    let text = r#"{"kind":"plan","version":1,"summary":"x","steps":[
      {"id":"s1","action":"fs.list","args":{"path":"/big"},"depends_on":[],"elevate":false},
      {"id":"s2","action":"fs.stat","args":{"path":{"from_step":"s1"}},"depends_on":["s1"],"elevate":false}]}"#;
    let result = run(&[text], &host, false);
    assert_eq!(
        reviewed(&result.outcome).steps[1].items.len(),
        MAX_EXPANDED_PATHS
    );
}

#[test]
fn an_incomplete_search_says_so_where_the_person_confirms_and_where_results_are_shown() {
    let host = files().with_note("2 directories could not be read: larger files may exist");
    let with_reference = run(&[DELETE_LARGEST], &host, false);
    assert!(
        reviewed(&with_reference.outcome)
            .render()
            .contains("NOTE: 2 directories could not be read")
    );
    let plain = run(&[LARGEST], &host, false);
    assert!(
        plain
            .outcome
            .render()
            .contains("NOTE: 2 directories could not be read")
    );
}

fn move_plan(from: &str) -> String {
    plan(&[step(
        "s1",
        "fs.move",
        &format!(r#"{{"from":"{from}","to":"/work/dest"}}"#),
        "false",
    )])
}

#[test]
fn a_move_needs_both_the_right_to_remove_the_source_and_to_write_the_destination() {
    for forbidden in [(Access::Delete, "/work/a"), (Access::Write, "/work/dest")] {
        let host = files().forbid(forbidden.0, forbidden.1);
        let result = run(&[&move_plan("/work/a")], &host, false);
        assert!(
            matches!(result.outcome, Outcome::AllRefused(_)),
            "{forbidden:?}"
        );
    }
    let host = files();
    assert!(matches!(
        run(&[&move_plan("/work/a")], &host, false).outcome,
        Outcome::Executed { .. }
    ));
}

#[test]
fn a_move_from_a_reference_needs_exactly_one_path() {
    let text = r#"{"kind":"plan","version":1,"summary":"x","steps":[
      {"id":"s1","action":"fs.find_largest","args":{"path":"/work","count":2},"depends_on":[],"elevate":false},
      {"id":"s2","action":"fs.move","args":{"from":{"from_step":"s1"},"to":"/work/dest"},"depends_on":["s1"],"elevate":false}]}"#;
    let result = run(&[text], &files(), false);
    assert!(matches!(
        reviewed(&result.outcome).steps[1].blocked,
        Some(Refusal::CannotExpand(_))
    ));
}

#[test]
fn a_step_whose_dependency_failed_at_run_time_does_not_run() {
    // The read passes the broker, then fails when it runs.
    let host = files().failing("/work/gone");
    let text = r#"{"kind":"plan","version":1,"summary":"x","steps":[
      {"id":"s1","action":"fs.list","args":{"path":"/work/gone"},"depends_on":[],"elevate":false},
      {"id":"s2","action":"fs.stat","args":{"path":"/work/a"},"depends_on":["s1"],"elevate":false},
      {"id":"s3","action":"fs.stat","args":{"path":"/work/b"},"depends_on":[],"elevate":false}]}"#;
    let result = run(&[text], &host, false);
    let Outcome::Executed { steps, .. } = &result.outcome else {
        panic!("{:?}", result.outcome)
    };
    assert!(steps[1].not_run.is_some());
    assert!(!host.executed.borrow().iter().any(|a| a.contains("/work/a")));
    assert!(host.executed.borrow().iter().any(|a| a.contains("/work/b")));
}

#[test]
fn a_step_whose_dependency_was_not_confirmed_as_root_does_not_run() {
    let host = files();
    let text = r#"{"kind":"plan","version":1,"summary":"x","steps":[
      {"id":"s1","action":"fs.delete","args":{"path":"/work/a"},"depends_on":[],"elevate":true},
      {"id":"s2","action":"fs.stat","args":{"path":"/work/b"},"depends_on":["s1"],"elevate":false}]}"#;
    let answers = Answers {
        root: false,
        ..Answers::yes()
    };
    let (result, _) = run_with(
        &[text],
        &host,
        &Authority::full(user(true)),
        answers,
        MemoryLog::default(),
    );
    assert!(matches!(result.outcome, Outcome::Executed { .. }));
    assert!(host.executed.borrow().is_empty());
}

#[test]
fn a_blocked_step_never_runs_its_items_even_when_other_steps_do() {
    let host = files().failing("/work");
    let text = r#"{"kind":"plan","version":1,"summary":"x","steps":[
      {"id":"s1","action":"fs.find_largest","args":{"path":"/work","count":2},"depends_on":[],"elevate":false},
      {"id":"s2","action":"fs.delete","args":{"path":{"from_step":"s1"}},"depends_on":["s1"],"elevate":false},
      {"id":"s3","action":"fs.stat","args":{"path":"/work/a"},"depends_on":[],"elevate":false}]}"#;
    let result = run(&[text], &host, false);
    // s1 was read once before confirmation and failed; nothing runs it again.
    let finds = host
        .executed
        .borrow()
        .iter()
        .filter(|a| a.starts_with("fs.find_largest"))
        .count();
    assert_eq!(finds, 1);
    assert!(
        host.executed
            .borrow()
            .iter()
            .any(|a| a.starts_with("fs.stat"))
    );
    assert!(matches!(result.outcome, Outcome::Executed { .. }));
}

#[test]
fn a_read_is_not_run_before_the_step_it_depends_on_has_run() {
    let host = files();
    let text = r#"{"kind":"plan","version":1,"summary":"x","steps":[
      {"id":"s1","action":"fs.delete","args":{"path":"/work/a"},"depends_on":[],"elevate":false},
      {"id":"s2","action":"fs.find_largest","args":{"path":"/work","count":1},"depends_on":["s1"],"elevate":false},
      {"id":"s3","action":"fs.delete","args":{"path":{"from_step":"s2"}},"depends_on":["s2"],"elevate":false}]}"#;
    let result = run(&[text], &host, false);
    assert!(matches!(
        reviewed(&result.outcome).steps[2].blocked,
        Some(Refusal::CannotExpand(_))
    ));
    // Before confirmation nothing was read; after it, s1 ran and then s2.
    let order = host.executed.borrow().clone();
    assert_eq!(order[0], r#"fs.delete path="/work/a""#);
}

#[test]
fn a_read_reused_at_execution_is_logged_as_such_and_long_results_are_cut() {
    let result = run(&[DELETE_LARGEST], &files(), false);
    assert_eq!(events(&result, "use_pre_run").len(), 1);
    assert_eq!(events(&result, "execute").len(), 2);
}

#[test]
fn a_long_result_is_cut_in_the_log_but_not_in_what_the_person_sees() {
    let text = plan(&[step("s1", "fs.read", r#"{"path":"/work/a"}"#, "false")]);
    let result = run(&[&text], &files(), false);
    assert_eq!(
        events(&result, "result")[0]["detail"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        2000
    );
    assert!(result.outcome.render().contains(&"x".repeat(5000)));
}

#[test]
fn a_write_beyond_the_users_permissions_is_refused() {
    let host = files().forbid(Access::Write, "/work/new");
    let text = plan(&[step(
        "s1",
        "fs.write",
        r#"{"path":"/work/new","content":"x","mode":"create"}"#,
        "false",
    )]);
    let result = run(&[&text], &host, false);
    assert!(matches!(
        reviewed(&result.outcome).steps[0].items[0].verdict,
        Verdict::Refused(Refusal::PermissionDenied {
            access: Access::Write,
            ..
        })
    ));
}

#[test]
fn a_move_and_a_write_need_the_write_capability_not_only_the_read_one() {
    let capabilities: BTreeSet<_> = [Capability::FsRead].into_iter().collect();
    let authority = Authority {
        user: user(false),
        capabilities,
    };
    let write = plan(&[step(
        "s1",
        "fs.write",
        r#"{"path":"/work/new","content":"x","mode":"create"}"#,
        "false",
    )]);
    for text in [move_plan("/work/a"), write] {
        let (result, _) = run_with(
            &[&text],
            &files(),
            &authority,
            Answers::yes(),
            MemoryLog::default(),
        );
        let Outcome::AllRefused(review) = &result.outcome else {
            panic!("{:?}", result.outcome)
        };
        assert_eq!(
            review.steps[0].items[0].verdict,
            Verdict::Refused(Refusal::CapabilityNotGranted(Capability::FsWrite))
        );
    }
}

#[test]
fn an_action_is_not_run_when_its_own_record_cannot_be_written() {
    // request, model_output, plan_accepted, review, plan_confirmation: then the execute event fails.
    let host = files();
    let text = plan(&[step("s1", "fs.stat", r#"{"path":"/work/a"}"#, "false")]);
    let (result, _) = run_with(
        &[&text],
        &host,
        &Authority::full(user(false)),
        Answers::yes(),
        MemoryLog::failing_after(5),
    );
    assert!(matches!(result.outcome, Outcome::LogFailed { .. }));
    assert!(host.executed.borrow().is_empty());
}

#[test]
fn paths_are_resolved_before_the_user_permissions_are_checked() {
    use crate::host::UnixHost;
    use std::path::PathBuf;
    // `/work/sub/..` is `/work`; the fake host above resolves nothing, so use the real adapter's rule.
    let host = UnixHost::new(None, PathBuf::from("/work/sub"));
    let text = plan(&[step("s1", "fs.delete", r#"{"path":"/"}"#, "false")]);
    let planner = ScriptedPlanner::new(&[&text]);
    let mut answers = Answers::yes();
    let mut log = MemoryLog::default();
    let outcome = Shell {
        authority: &Authority::full(user(false)),
        host: &host,
        planner: &planner,
        confirm: &mut answers,
        log: &mut log,
    }
    .handle("x");
    assert!(matches!(outcome, Outcome::AllRefused(_)), "{outcome:?}");
}

#[test]
fn the_full_session_holds_every_capability() {
    let authority = Authority::full(user(false));
    assert_eq!(authority.capabilities.len(), Capability::ALL.len());
    for capability in Capability::ALL {
        assert!(authority.capabilities.contains(&capability), "{capability}");
    }
}

#[test]
fn the_real_hosts_resolution_is_what_the_broker_checks_and_shows() {
    use crate::host::UnixHost;
    let host = UnixHost::new(None, std::path::PathBuf::from("/work/sub"));
    let text = plan(&[step("s1", "fs.delete", r#"{"path":"/tmp/.."}"#, "false")]);
    let planner = ScriptedPlanner::new(&[&text]);
    let mut answers = Answers::yes();
    let mut log = MemoryLog::default();
    let outcome = Shell {
        authority: &Authority::full(user(false)),
        host: &host,
        planner: &planner,
        confirm: &mut answers,
        log: &mut log,
    }
    .handle("x");
    let Outcome::AllRefused(review) = &outcome else {
        panic!("{outcome:?}")
    };
    assert_eq!(
        review.steps[0].items[0].action.describe(),
        r#"fs.delete path="/""#
    );
}

#[test]
fn a_move_from_a_single_reference_expands_with_the_destination_kept() {
    let host = files();
    let text = r#"{"kind":"plan","version":1,"summary":"x","steps":[
      {"id":"s1","action":"fs.find_largest","args":{"path":"/work","count":1},"depends_on":[],"elevate":false},
      {"id":"s2","action":"fs.move","args":{"from":{"from_step":"s1"},"to":"/work/dest"},"depends_on":["s1"],"elevate":false}]}"#;
    let result = run(&[text], &host, false);
    let moves: Vec<_> = reviewed(&result.outcome).steps[1]
        .items
        .iter()
        .map(|i| i.action.describe())
        .collect();
    assert_eq!(moves, [r#"fs.move from="/work/b" to="/work/dest""#]);
}

#[test]
fn steps_listed_out_of_dependency_order_are_reviewed_in_dependency_order() {
    let text = r#"{"kind":"plan","version":1,"summary":"x","steps":[
      {"id":"s2","action":"fs.stat","args":{"path":{"from_step":"s1"}},"depends_on":["s1"],"elevate":false},
      {"id":"s1","action":"fs.find_largest","args":{"path":"/work","count":1},"depends_on":[],"elevate":false}]}"#;
    let result = run(&[text], &files(), false);
    let ids: Vec<_> = reviewed(&result.outcome)
        .steps
        .iter()
        .map(|s| s.id.as_str().to_owned())
        .collect();
    assert_eq!(ids, ["s1", "s2"]);
}

#[test]
fn the_plan_prompt_marks_root_steps_and_empty_references() {
    let host = files();
    let elevated = plan(&[step("s1", "fs.delete", r#"{"path":"/work/a"}"#, "true")]);
    let result = run(&[&elevated], &host, true);
    assert!(
        reviewed(&result.outcome)
            .render()
            .contains("ROOT, held for your confirmation")
    );

    let empty = FakeHost::with_files(&[]);
    let text = r#"{"kind":"plan","version":1,"summary":"x","steps":[
      {"id":"s1","action":"fs.find_largest","args":{"path":"/none","count":1},"depends_on":[],"elevate":false},
      {"id":"s2","action":"fs.delete","args":{"path":{"from_step":"s1"}},"depends_on":["s1"],"elevate":false}]}"#;
    let result = run(&[text], &empty, false);
    assert!(reviewed(&result.outcome).render().contains("nothing to do"));
}

#[test]
fn blocked_steps_are_logged_and_reported_and_never_run_their_items() {
    let host = files().failing("/work");
    let text = r#"{"kind":"plan","version":1,"summary":"x","steps":[
      {"id":"s1","action":"fs.find_largest","args":{"path":"/work","count":2},"depends_on":[],"elevate":false},
      {"id":"s2","action":"fs.delete","args":{"path":{"from_step":"s1"}},"depends_on":["s1"],"elevate":false},
      {"id":"s3","action":"fs.stat","args":{"path":"/work/a"},"depends_on":[],"elevate":false}]}"#;
    let result = run(&[text], &host, false);
    assert!(!events(&result, "step_not_run").is_empty());
    let Outcome::Executed { steps, .. } = &result.outcome else {
        panic!("{:?}", result.outcome)
    };
    assert!(steps[0].not_run.is_some() && steps[0].items.is_empty());
    assert!(result.outcome.render().contains("s1 not run"));
}

#[test]
fn the_log_names_the_user_the_question_and_the_steps() {
    let result = run(
        &[r#"{"kind":"clarify","version":1,"question":"Which?"}"#],
        &files(),
        false,
    );
    assert_eq!(events(&result, "request")[0]["user"], "ana");
    assert_eq!(events(&result, "clarify")[0]["question"], "Which?");
    let planned = run(&[DELETE_LARGEST], &files(), false);
    assert_eq!(
        events(&planned, "plan_accepted")[0]["steps"],
        serde_json::json!(["s1", "s2"])
    );
}

#[test]
fn a_model_that_cannot_be_reached_is_logged_and_a_dead_log_stops_even_that() {
    let planner = ScriptedPlanner::new(&[]);
    let host = files();
    let mut answers = Answers::yes();
    let mut log = MemoryLog::default();
    let authority = Authority::full(user(false));
    let outcome = Shell {
        authority: &authority,
        host: &host,
        planner: &planner,
        confirm: &mut answers,
        log: &mut log,
    }
    .handle("x");
    assert!(matches!(outcome, Outcome::NoPlan(_)));
    assert_eq!(
        events(
            &Run {
                outcome,
                log,
                answers
            },
            "model_error"
        )
        .len(),
        1
    );

    let planner = ScriptedPlanner::new(&[]);
    let mut answers = Answers::yes();
    let mut log = MemoryLog::failing_after(1);
    let outcome = Shell {
        authority: &authority,
        host: &host,
        planner: &planner,
        confirm: &mut answers,
        log: &mut log,
    }
    .handle("x");
    assert!(matches!(outcome, Outcome::LogFailed { .. }));
}

#[test]
fn what_a_read_prints_is_indented_so_it_cannot_pass_for_a_report_line() {
    let text = plan(&[step("s1", "fs.read", r#"{"path":"/work/a"}"#, "false")]);
    let result = run(&[&text], &files(), false);
    let shown = result.outcome.render();
    assert!(shown.lines().any(|l| l.starts_with("    xxxx")), "{shown}");
}

#[test]
fn the_filesystem_root_is_never_a_target_even_for_an_administrator_who_elevates() {
    for (action, args) in [
        ("fs.delete", r#"{"path":"/"}"#),
        (
            "fs.write",
            r#"{"path":"/","content":"x","mode":"overwrite"}"#,
        ),
        ("fs.move", r#"{"from":"/","to":"/work/dest"}"#),
        ("fs.move", r#"{"from":"/work/a","to":"/"}"#),
    ] {
        let text = plan(&[step("s1", action, args, "true")]);
        let result = run(&[&text], &files(), true);
        let Outcome::AllRefused(review) = &result.outcome else {
            panic!("{action}: {:?}", result.outcome)
        };
        assert!(
            matches!(
                review.steps[0].items[0].verdict,
                Verdict::Refused(Refusal::ProtectedPath(_))
            ),
            "{action}"
        );
        assert!(result.answers.root_asked.is_empty());
    }
}

#[test]
fn stat_is_judged_by_its_own_access_not_by_read() {
    let host = files().forbid(Access::Read, "/work/a");
    let text = plan(&[step("s1", "fs.stat", r#"{"path":"/work/a"}"#, "false")]);
    let result = run(&[&text], &host, false);
    assert!(matches!(result.outcome, Outcome::Executed { .. }));
    let host = files().forbid(Access::Stat, "/work/a");
    let result = run(&[&text], &host, false);
    assert!(matches!(result.outcome, Outcome::AllRefused(_)));
}
