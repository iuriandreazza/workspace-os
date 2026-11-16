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
