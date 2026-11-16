//! The deterministic shell: every action of the format is reachable by a
//! command, and a typed command runs through the same pipeline as the model's
//! plan, with the same refusals, questions and log.

use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::action::{Access, RootRule};
use crate::broker::Authority;
use crate::command::{self, Parsed};
use crate::pipeline_tests::{DELETE_LARGEST, LARGEST, files, plan, step};
use crate::plan::{Document, Origin, Plan, parse_checked};
use crate::ports::{Planner, Retry};
use crate::screen::{View, plan_screen};
use crate::session::{ItemResult, Outcome, Shell};
use crate::testing::{Answers, FakeHost, MemoryLog, ScriptedPlanner, user};

fn docs(file: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../docs/schemas")
        .join(file);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The plan a line makes, through the same parser the model's text goes through.
fn plan_of(line: &str) -> Plan {
    let Ok(Parsed::Plan(json)) = command::parse(line) else {
        panic!("{line:?} should make a plan: {:?}", command::parse(line))
    };
    match parse_checked(&json).unwrap_or_else(|e| panic!("{line:?}: {e}\n{json}")) {
        Document::Plan(plan) => plan,
        Document::Clarify(_) => panic!("a command never asks"),
    }
}

fn refused(line: &str) -> String {
    match command::parse(line) {
        Err(error) => error.to_string(),
        Ok(Parsed::Plan(json)) => match parse_checked(&json) {
            Err(error) => error.to_string(),
            Ok(_) => panic!("{line:?} should be refused"),
        },
        Ok(Parsed::Help(_)) => panic!("{line:?} is help"),
    }
}

/// Each example of the format and the line that produces the same steps.
const EXAMPLES: &[(&str, &str)] = &[
    (
        "chain-through-a-folder.json",
        "fs list --name holiday --in pictures --kind folder; fs stat --from-step s1",
    ),
    (
        "delete-largest-with-reference.json",
        "fs find_largest --in downloads --count 3; fs delete --from-step s1",
    ),
    (
        "elevated-write.json",
        "fs write --name motd --in everywhere --kind file --content \"Maintenance on Friday.\\n\" --mode append --elevate",
    ),
    (
        "independent-steps.json",
        "fs move --name report.txt --in documents --kind file --to-name archive --to-in documents; \
         fs write --new-name notes.txt --in documents --content 'Report archived.'",
    ),
    (
        "largest-files.json",
        "fs find_largest --in everywhere --count 5",
    ),
    (
        "read-by-name.json",
        "fs read --name budget.csv --in everywhere --kind file",
    ),
    (
        "run-program.json",
        "process run --program tar -- -czf backup.tgz docs",
    ),
    ("single-list.json", "fs list --in downloads"),
    (
        "soul-propose-change.json",
        "soul propose_change --rationale 'The user asked for shorter answers.' \
         --new-text 'I am the system. I answer briefly and precisely.'",
    ),
    (
        "system-configure.json",
        "system configure --key machine.name --value atlas",
    ),
    (
        "user-add-to-group.json",
        "user manage --operation add_to_group --user ana --group admin",
    ),
];

fn example_plan(file: &str) -> Plan {
    match parse_checked(&docs(&format!("examples/valid/{file}"))).unwrap() {
        Document::Plan(plan) => plan,
        Document::Clarify(_) => panic!("{file} is a question"),
    }
}

#[test]
fn every_valid_example_is_reachable_by_a_command_with_the_same_steps() {
    for (file, line) in EXAMPLES {
        assert_eq!(
            plan_of(line).steps,
            example_plan(file).steps,
            "{file}: {line}"
        );
    }
}

#[test]
fn the_table_of_examples_covers_every_plan_the_format_has() {
    let mut on_disk: Vec<String> =
        fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/schemas/examples/valid"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name != "clarify.json")
            .collect();
    on_disk.sort();
    let mut covered: Vec<String> = EXAMPLES.iter().map(|(f, _)| (*f).to_owned()).collect();
    covered.sort();
    assert_eq!(
        on_disk, covered,
        "a new example needs a command, or the other way round"
    );
}

/// The action names the schema allows a step to have.
fn schema_actions() -> Vec<String> {
    let schema: Value = serde_json::from_str(&docs("action-plan.schema.json")).unwrap();
    let defs = &schema["$defs"];
    let mut names: Vec<String> = defs["step"]["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .map(|reference| {
            let name = reference["$ref"]
                .as_str()
                .unwrap()
                .rsplit('/')
                .next()
                .unwrap();
            defs[name]["properties"]["action"]["const"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    names.sort();
    names
}

#[test]
fn every_action_of_the_schema_has_a_command_and_no_command_is_invented() {
    let mut commands = command::actions();
    commands.sort();
    assert_eq!(schema_actions(), commands);
}

#[test]
fn every_example_step_is_made_with_the_flag_set_the_format_allows() {
    // `--elevate` is accepted exactly where the root rule says a step may ask for root.
    for (file, _) in EXAMPLES {
        for step in &example_plan(file).steps {
            assert_eq!(
                command::takes_elevate(step.spec.name()),
                Some(step.spec.root_rule() == RootRule::OnRequest),
                "{}",
                step.spec.name()
            );
        }
    }
    assert!(refused("user manage --operation add --user bob --elevate").contains("--elevate"));
}

#[test]
fn quoting_expands_nothing_and_keeps_what_is_inside_the_quotes() {
    let content = |line: &str| {
        let Ok(Parsed::Plan(json)) = command::parse(line) else {
            panic!("{line}")
        };
        let value: Value = serde_json::from_str(&json).unwrap();
        value["steps"][0]["args"]["content"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let write = |content: &str| format!("fs write --new-name n --in home --content {content}");
    assert_eq!(content(&write("'a $HOME ~ * \\n b'")), "a $HOME ~ * \\n b");
    assert_eq!(content(&write("\"a\\tb\\n\\\"c\\\\\"")), "a\tb\n\"c\\");
    assert_eq!(content(&write("a\\ b")), "a b");
    assert_eq!(content(&write("\"\"")), "");
    assert_eq!(content(&write("a\"b c\"'d e'")), "ab cd e");
    assert_eq!(content(&write("'--in'")), "--in");
    assert_eq!(content(&write("$HOME")), "$HOME");
    assert_eq!(content(&write("~")), "~");
    assert_eq!(content(&write("'a;b'")), "a;b");
}

#[test]
fn a_location_cannot_be_written_in_a_name() {
    for name in ["~/x", "$HOME/x", "../x", "/etc/passwd", ".."] {
        let line = format!("fs read --name '{name}' --in home");
        assert!(refused(&line).contains("single file name"), "{name}");
    }
}

#[test]
fn what_cannot_be_read_as_a_command_is_refused_with_the_reason_and_runs_nothing() {
    for (line, mention) in [
        ("", "type a command"),
        ("   ", "type a command"),
        ("fs", "group and an action"),
        ("rm -rf /", "unknown command"),
        ("delete the big files", "unknown command"),
        ("fs.list --in home", "unknown command"),
        ("fs list ; ", "step is empty"),
        ("; fs list --in home", "step is empty"),
        ("fs list --in home --in documents", "twice"),
        ("fs list --in", "needs a value"),
        ("fs list --in --count", "needs a value"),
        ("fs list --in home extra", "flags start with --"),
        ("fs list --in home --colour red", "unknown flag"),
        ("fs list --in home --to-in home", "unknown flag"),
        ("fs list --in home --elevate --elevate", "twice"),
        ("fs list --in home -- x", "after --"),
        ("fs list", "needs a reference"),
        ("fs list --kind file", "goes with"),
        ("fs list --in home --kind file", "goes with"),
        ("fs list --name a", "needs --in"),
        ("fs list --new-name a", "needs --in"),
        (
            "fs list --name a --new-name b --in home",
            "different references",
        ),
        ("fs list --new-name a --in home --kind file", "goes with"),
        ("fs list --from-step s1 --in home", "stands alone"),
        ("fs find_largest --in home", "needs --count"),
        ("fs find_largest --in home --count ten", "whole number"),
        ("fs find_largest --in home --count -1", "whole number"),
        ("fs write --new-name a --in home", "needs --content"),
        ("fs move --name a --in home", "needs a reference"),
        (
            "fs move --name a --in home --to-kind folder --to-in home",
            "unknown flag",
        ),
        ("user manage --user bob", "needs --operation"),
        ("fs read --name 'a", "not closed"),
        ("fs read --name \"a", "not closed"),
        ("fs read --name a\\", "backslash"),
        ("fs read --name \"a\\x\" --in home", "backslash"),
    ] {
        assert!(
            refused(line).contains(mention),
            "{line:?}: {}",
            refused(line)
        );
    }
}

#[test]
fn the_format_refuses_what_the_model_would_be_refused() {
    // Each of these parses as words and is refused by the same check as the model's text.
    for line in [
        "fs delete --in downloads",
        "fs read --in downloads",
        "fs write --name a --in home --content x --mode wipe",
        "fs write --in home --content x",
        "fs find_largest --in home --count 0",
        "fs find_largest --in home --count 101",
        "fs find_largest --from-step s1 --count 1",
        "fs move --name a --in home --to-name b --to-in home --from-step s1",
        "fs stat --from-step s1",
        "fs stat --from-step s2; fs stat --name a --in home",
        "fs stat --from-step s1; fs stat --from-step s1",
        "process run --program 'rm -rf' -- x",
        "process run --program Rm",
        "user manage --operation add --user bob --group staff",
        "user manage --operation add_to_group --user bob",
        "user manage --operation add --user BOB",
        "system configure --key 'Bad Key' --value x",
        "fs list --in everywhere --after s1",
        "fs list --in home --after s9",
        "fs list --in home --after s1",
    ] {
        refused(line);
    }
    let sixteen = vec!["fs list --in home"; 16].join("; ");
    plan_of(&sixteen);
    let seventeen = vec!["fs list --in home"; 17].join("; ");
    assert!(refused(&seventeen).contains("at most 16"));
}

#[test]
fn dependencies_come_from_after_and_from_step_only() {
    let plan = plan_of(
        "fs find_largest --in downloads --count 2; fs list --in documents; \
         fs stat --from-step s1; fs list --in home --after s2 --after s3 --after s2",
    );
    let deps: Vec<Vec<&str>> = plan
        .steps
        .iter()
        .map(|s| s.depends_on.iter().map(|d| d.as_str()).collect())
        .collect();
    assert_eq!(deps, [vec![], vec![], vec!["s1"], vec!["s2", "s3"]]);
}

#[test]
fn the_summary_is_the_action_names_whatever_was_typed() {
    let long = "x".repeat(60_000);
    let plan = plan_of(&format!(
        "fs write --new-name n --in home --content {long}; fs list --in home"
    ));
    assert_eq!(plan.summary, "fs.write, fs.list");
    let many = vec!["soul propose_change --rationale r --new-text t"; 16].join("; ");
    let summary = plan_of(&many).summary;
    assert!(summary.chars().count() <= 200 && summary.ends_with("..."));
}

#[test]
fn help_lists_every_command_and_runs_nothing() {
    let Ok(Parsed::Help(text)) = command::parse("help") else {
        panic!("help")
    };
    for action in command::actions() {
        let words = action.replacen('.', " ", 1);
        assert!(text.contains(&words), "{words}");
    }
    assert!(
        command::parse("'help'").is_err(),
        "a quoted help is not help"
    );
    assert!(command::parse("help me").is_err());
}

// --- the pipeline -----------------------------------------------------------

/// A planner that must never be asked.
struct NeverAsked;

impl Planner for NeverAsked {
    fn plan(&self, _request: &str, _retry: Option<&Retry>) -> Result<String, String> {
        panic!("the model was asked in the deterministic shell")
    }
}

struct Trace {
    outcome: Outcome,
    log: MemoryLog,
    answers: Answers,
}

fn typed(
    line: &str,
    host: &FakeHost,
    administrator: bool,
    answers: Answers,
    log: MemoryLog,
) -> Trace {
    let (mut answers, mut log) = (answers, log);
    let outcome = Shell {
        authority: &Authority::full(user(administrator)),
        host,
        planner: &NeverAsked,
        confirm: &mut answers,
        log: &mut log,
    }
    .handle_command(line);
    Trace {
        outcome,
        log,
        answers,
    }
}

fn modelled(
    json: &str,
    host: &FakeHost,
    administrator: bool,
    answers: Answers,
    log: MemoryLog,
) -> Trace {
    let (mut answers, mut log) = (answers, log);
    let planner = ScriptedPlanner::new(&[json]);
    let outcome = Shell {
        authority: &Authority::full(user(administrator)),
        host,
        planner: &planner,
        confirm: &mut answers,
        log: &mut log,
    }
    .handle("do it");
    Trace {
        outcome,
        log,
        answers,
    }
}

/// What the person and the machine saw, apart from who wrote the plan.
fn seen(trace: &Trace, host: &FakeHost) -> Value {
    let events: Vec<Value> = trace
        .log
        .events
        .iter()
        .filter(|e| {
            !matches!(
                e["event"].as_str(),
                Some("request" | "model_output" | "command")
            )
        })
        .cloned()
        .map(|mut e| {
            if let Some(map) = e.as_object_mut() {
                map.remove("summary");
                if let Some(review) = map.get_mut("review").and_then(Value::as_object_mut) {
                    review.remove("summary");
                }
            }
            e
        })
        .collect();
    let outcome = match &trace.outcome {
        Outcome::Executed { steps, .. } => format!("{steps:?}"),
        other => other
            .render()
            .lines()
            .map(|line| {
                // The one place the screen says who wrote the plan.
                let cut = ["  You typed:", "  The model says:"]
                    .iter()
                    .filter_map(|who| line.find(who))
                    .min();
                cut.map_or(line, |at| &line[..at]).to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n"),
    };
    serde_json::json!({
        "events": events,
        "executed": host.executed.borrow().clone(),
        "root_panels": trace.answers.root_asked,
        "questions": trace.answers.questions,
        "outcome": outcome,
    })
}

/// The same scenario typed and as the model's document: nothing but the author differs.
fn same_both_ways(
    line: &str,
    json: &str,
    host: impl Fn() -> FakeHost,
    administrator: bool,
    answers: impl Fn() -> Answers,
) -> Value {
    let (typed_host, model_host) = (host(), host());
    let by_command = typed(
        line,
        &typed_host,
        administrator,
        answers(),
        MemoryLog::default(),
    );
    let by_model = modelled(
        json,
        &model_host,
        administrator,
        answers(),
        MemoryLog::default(),
    );
    let (a, b) = (seen(&by_command, &typed_host), seen(&by_model, &model_host));
    assert_eq!(a, b, "{line}");
    a
}

#[test]
fn a_read_only_command_runs_without_asking_like_the_models_plan() {
    let seen = same_both_ways(
        "fs find_largest --in downloads --count 2",
        LARGEST,
        files,
        false,
        Answers::yes,
    );
    let names: Vec<_> = seen["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["event"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "plan_accepted",
            "review",
            "plan_auto_approved",
            "execute",
            "result"
        ]
    );
}

#[test]
fn a_destructive_command_asks_first_like_the_models_plan() {
    let host = files();
    let result = typed(
        "fs find_largest --in downloads --count 2; fs delete --from-step s1",
        &host,
        false,
        Answers::yes(),
        MemoryLog::default(),
    );
    assert_eq!(
        result.answers.screens.len(),
        1,
        "the plan was shown for confirmation"
    );
    assert!(matches!(
        result.outcome,
        Outcome::Executed { asked: true, .. }
    ));
    same_both_ways(
        "fs find_largest --in downloads --count 2; fs delete --from-step s1",
        DELETE_LARGEST,
        files,
        false,
        Answers::yes,
    );
    // Declining runs nothing, as for the model.
    let declined = || Answers {
        plan: false,
        ..Answers::yes()
    };
    let host = files();
    let result = typed(
        "fs find_largest --in downloads --count 2; fs delete --from-step s1",
        &host,
        false,
        declined(),
        MemoryLog::default(),
    );
    assert!(matches!(result.outcome, Outcome::Declined(_)));
    assert_eq!(
        host.executed.borrow().len(),
        1,
        "only the read the plan needed ran"
    );
}

fn delete_a() -> String {
    plan(&[step(
        "s1",
        "fs.delete",
        r#"{"target":{"name":"a","in":"downloads","kind":"file"}}"#,
        "false",
    )])
}

fn delete_a_as_root() -> String {
    plan(&[step(
        "s1",
        "fs.delete",
        r#"{"target":{"name":"a","in":"downloads","kind":"file"}}"#,
        "true",
    )])
}

#[test]
fn the_brokers_refusals_apply_to_a_command_as_to_the_models_plan() {
    let forbidden = || files().forbid(Access::Delete, "/work/a");
    let seen = same_both_ways(
        "fs delete --name a --in downloads --kind file",
        &delete_a(),
        forbidden,
        false,
        Answers::yes,
    );
    assert!(
        seen["executed"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| !e.as_str().unwrap().contains("fs.delete"))
    );
    // Asking for root does not help a user who is not an administrator.
    let seen = same_both_ways(
        "fs delete --name a --in downloads --kind file --elevate",
        &delete_a_as_root(),
        files,
        false,
        Answers::yes,
    );
    assert!(seen["executed"].as_array().unwrap().is_empty());
    assert!(seen["root_panels"].as_array().unwrap().is_empty());
}

#[test]
fn a_root_action_gets_its_own_panel_and_confirmation_like_the_models_plan() {
    let seen = same_both_ways(
        "fs delete --name a --in downloads --kind file --elevate",
        &delete_a_as_root(),
        files,
        true,
        Answers::yes,
    );
    assert_eq!(seen["root_panels"].as_array().unwrap().len(), 1);
    let events: Vec<_> = seen["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["event"].as_str().unwrap())
        .collect();
    assert!(events.contains(&"root_confirmation"));
    // Refusing at the panel runs nothing.
    let host = files();
    let refuse = Answers {
        root: false,
        ..Answers::yes()
    };
    let result = typed(
        "fs delete --name a --in downloads --kind file --elevate",
        &host,
        true,
        refuse,
        MemoryLog::default(),
    );
    assert!(host.executed.borrow().is_empty());
    let Outcome::Executed { steps, .. } = &result.outcome else {
        panic!("{:?}", result.outcome)
    };
    assert!(matches!(steps[0].items[0].result, ItemResult::NotRun(_)));
}

#[test]
fn an_ambiguous_name_is_the_persons_choice_as_it_is_for_the_models_plan() {
    let reports =
        || FakeHost::with_files(&[("/work/report-2024.pdf", 10), ("/work/report-2025.pdf", 20)]);
    let json = plan(&[step(
        "s1",
        "fs.read",
        r#"{"target":{"name":"report","in":"downloads","kind":"file"}}"#,
        "false",
    )]);
    let seen = same_both_ways(
        "fs read --name report --in downloads --kind file",
        &json,
        reports,
        false,
        || Answers::yes().picking(&[Some(1)]),
    );
    assert_eq!(seen["questions"].as_array().unwrap().len(), 1);
    assert!(
        seen["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["event"] == "choice")
    );
    // Cancelling the choice runs nothing.
    let host = reports();
    let result = typed(
        "fs read --name report --in downloads",
        &host,
        false,
        Answers::yes().picking(&[None]),
        MemoryLog::default(),
    );
    assert!(matches!(result.outcome, Outcome::Declined(_)));
    assert!(host.executed.borrow().is_empty());
}

#[test]
fn a_name_that_is_not_found_is_reported_and_nothing_runs() {
    let host = files();
    let result = typed(
        "fs read --name missing --in downloads",
        &host,
        false,
        Answers::yes(),
        MemoryLog::default(),
    );
    assert!(
        matches!(result.outcome, Outcome::AllRefused(_)),
        "{:?}",
        result.outcome
    );
    assert!(host.executed.borrow().is_empty());
}

#[test]
fn a_log_that_cannot_be_written_stops_the_command_before_anything_runs() {
    for accept in 0..4 {
        let host = files();
        let result = typed(
            "fs find_largest --in downloads --count 2; fs delete --from-step s1",
            &host,
            false,
            Answers::yes(),
            MemoryLog::failing_after(accept),
        );
        assert!(
            matches!(result.outcome, Outcome::LogFailed { .. }),
            "{accept}"
        );
        let ran = host.executed.borrow();
        assert!(
            ran.iter().all(|e| !e.contains("fs.delete")),
            "{accept}: {ran:?}"
        );
    }
}

#[test]
fn a_refused_command_is_logged_runs_nothing_and_is_not_a_model_decision() {
    let host = files();
    for line in ["rm -rf /", "fs delete --in downloads", "help"] {
        let result = typed(line, &host, false, Answers::yes(), MemoryLog::default());
        assert!(host.executed.borrow().is_empty(), "{line}");
        let events: Vec<_> = result
            .log
            .events
            .iter()
            .map(|e| e["event"].as_str().unwrap())
            .collect();
        assert_eq!(events, ["command"], "{line}");
        assert_eq!(result.log.events[0]["line"], line);
        assert!(
            result
                .log
                .events
                .iter()
                .all(|e| e["event"] != "model_output" && e["event"] != "request")
        );
    }
    let result = typed(
        "rm -rf /",
        &host,
        false,
        Answers::yes(),
        MemoryLog::default(),
    );
    assert!(matches!(&result.outcome, Outcome::BadCommand(_)));
    assert!(result.outcome.render().contains("Nothing ran"));
}

#[test]
fn a_dead_log_stops_even_a_refused_command() {
    let result = typed(
        "rm -rf /",
        &files(),
        false,
        Answers::yes(),
        MemoryLog::failing_after(0),
    );
    assert!(matches!(result.outcome, Outcome::LogFailed { .. }));
}

#[test]
fn the_log_holds_the_line_and_the_document_it_became() {
    let result = typed(
        "fs find_largest --in downloads --count 2",
        &files(),
        false,
        Answers::yes(),
        MemoryLog::default(),
    );
    let event = &result.log.events[0];
    assert_eq!(event["event"], "command");
    assert_eq!(event["user"], "ana");
    assert_eq!(event["accepted"], true);
    let json = event["plan"].as_str().unwrap();
    assert!(matches!(parse_checked(json), Ok(Document::Plan(_))));
}

#[test]
fn the_screen_says_who_typed_the_plan() {
    let host = files();
    let result = typed(
        "fs find_largest --in downloads --count 2",
        &host,
        false,
        Answers::yes(),
        MemoryLog::default(),
    );
    let Outcome::Executed { review, .. } = &result.outcome else {
        panic!("{:?}", result.outcome)
    };
    assert_eq!(review.origin, Origin::Typed);
    let screen = plan_screen(review, View::Friendly);
    assert!(
        screen.contains("You typed: \"fs.find_largest\""),
        "{screen}"
    );
    assert!(!screen.contains("The model says"));
    let by_model = modelled(
        LARGEST,
        &files(),
        false,
        Answers::yes(),
        MemoryLog::default(),
    );
    let Outcome::Executed { review, .. } = &by_model.outcome else {
        panic!()
    };
    assert!(plan_screen(review, View::Friendly).contains("The model says"));
}

#[test]
fn a_typed_command_never_reaches_the_model() {
    // `NeverAsked` panics when called: every typed scenario above would fail.
    let host = files();
    let _ = typed(
        "fs list --in downloads",
        &host,
        false,
        Answers::yes(),
        MemoryLog::default(),
    );
    let _ = typed(
        "this is not a command",
        &host,
        false,
        Answers::yes(),
        MemoryLog::default(),
    );
}

#[test]
fn the_only_defaults_are_the_weakest_choices() {
    let plan = plan_of("fs delete --name a --in home; fs write --new-name n --in home --content c");
    let json = |step: usize| format!("{:?}", plan.steps[step].spec);
    assert!(json(0).contains("kind: Any"), "{}", json(0));
    assert!(json(1).contains("mode: Create"), "{}", json(1));
    // Nothing else is assumed: root is never requested unless the flag is there.
    assert!(plan.steps.iter().all(|s| !s.elevate));
    assert!(plan_of("fs delete --name a --in home --elevate").steps[0].elevate);
}
