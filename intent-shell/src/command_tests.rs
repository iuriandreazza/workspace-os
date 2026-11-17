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
use crate::session::{ItemResult, Outcome, SessionState, Shell};
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
        "cd-to-a-folder-under-home.json",
        "session cd --name films --under-home",
    ),
    ("cd-to-a-path.json", "session cd --path ~/Downloads"),
    (
        "chain-through-a-folder.json",
        "fs list --name holiday --in pictures --kind folder; fs stat --from-step s1",
    ),
    (
        "delete-an-absolute-path-as-root.json",
        "fs delete --path /var/log/old.log --elevate",
    ),
    (
        "delete-largest-with-reference.json",
        "fs find_largest --in downloads --count 3; fs delete --from-step s1",
    ),
    (
        "elevated-write.json",
        "fs write --path /etc/motd --content \"Maintenance on Friday.\\n\" --mode append --elevate",
    ),
    (
        "independent-steps.json",
        "fs move --name report.txt --in documents --kind file --to-name archive --to-in documents; \
         fs write --new-name notes.txt --in documents --content 'Report archived.'",
    ),
    ("largest-files.json", "fs find_largest --path ~ --count 5"),
    ("list-a-path.json", "fs list --path ~/Downloads/films"),
    (
        "move-into-a-path.json",
        "fs move --path report.pdf --to-path ~/Documents",
    ),
    (
        "name-inside-a-path.json",
        "fs stat --name report.pdf --in-path ~/Documents --kind file",
    ),
    (
        "new-item-in-a-path.json",
        "fs write --new-name notes.txt --in-path ~/Documents --content hello",
    ),
    (
        "read-by-name.json",
        "fs read --name budget.csv --under-home --kind file",
    ),
    ("relative-path.json", "fs read --path notes/today.txt"),
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
        ("chmod -R 777 /", "unknown command"),
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
        (
            "fs move --name a --in home --kind file",
            "needs a reference",
        ),
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
    for (line, reason) in [
        ("fs delete --in downloads", "not accepted here"),
        ("fs read --in downloads", "not accepted here"),
        (
            "fs write --name a --in home --kind file --content x --mode wipe",
            "wipe",
        ),
        ("fs write --in home --content x", "not accepted here"),
        ("fs find_largest --in home --count 0", "1 to 100"),
        ("fs find_largest --in home --count 101", "1 to 100"),
        (
            "fs find_largest --from-step s1 --count 1",
            "not accepted here",
        ),
        (
            "fs move --name a --in home --to-name b --to-in home --from-step s1",
            "stands alone",
        ),
        ("fs stat --from-step s1", "itself"),
        (
            "fs stat --from-step s2; fs stat --name a --in home",
            "does not produce paths",
        ),
        (
            "fs stat --from-step s1; fs stat --from-step s1",
            "depends on itself",
        ),
        ("process run --program 'rm -rf' -- x", "invalid form"),
        ("process run --program Rm", "invalid form"),
        (
            "user manage --operation add --user bob --group staff",
            "group",
        ),
        ("user manage --operation add_to_group --user bob", "group"),
        ("user manage --operation add --user BOB", "invalid form"),
        ("system configure --key 'Bad Key' --value x", "invalid form"),
        ("fs list --in home --after s1", "depends on itself"),
        ("fs list --in home --after s9", "unknown step"),
    ] {
        let why = refused(line);
        assert!(why.contains(reason), "{line:?}: {why}");
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
    fn plan(&self, _request: &str, _cwd: &str, _retry: Option<&Retry>) -> Result<String, String> {
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
        session: &mut SessionState::start(host),
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
        session: &mut SessionState::start(host),
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
                let cut = ["  Typed commands:", "  The model says:"]
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
        "questions": trace.answers.questions.iter().map(|(question, lines)| {
            (question, lines.iter().map(|line| line.text.as_str()).collect::<Vec<_>>())
        }).collect::<Vec<_>>(),
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
        screen.contains("Typed commands: \"fs.find_largest\""),
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
    // `NeverAsked` panics when called, so reaching it fails the test.
    let host = files();
    let listed = typed(
        "fs list --in downloads",
        &host,
        false,
        Answers::yes(),
        MemoryLog::default(),
    );
    assert!(matches!(listed.outcome, Outcome::Executed { .. }));
    let garbage = typed(
        "this is not a command",
        &host,
        false,
        Answers::yes(),
        MemoryLog::default(),
    );
    assert!(matches!(garbage.outcome, Outcome::BadCommand(_)));
}

#[test]
fn an_escaped_character_is_a_value_never_a_flag() {
    let content = |text: &str| {
        let line = format!("fs write --new-name n --in home --content {text}");
        let Ok(Parsed::Plan(json)) = command::parse(&line) else {
            panic!("{line}")
        };
        let value: Value = serde_json::from_str(&json).unwrap();
        value["steps"][0]["args"]["content"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(content("\\--x"), "--x");
    assert!(refused("fs delete --name a --in home --kind file \\--elevate").contains("unexpected"));
}

#[test]
fn the_only_defaults_are_the_weakest_choices() {
    let plan = plan_of("fs stat --name a --in home; fs write --new-name n --in home --content c");
    let json = |step: usize| format!("{:?}", plan.steps[step].spec);
    assert!(json(0).contains("kind: Any"), "{}", json(0));
    assert!(json(1).contains("mode: Create"), "{}", json(1));
    // Nothing else is assumed: root is never requested unless the flag is there.
    assert!(plan.steps.iter().all(|s| !s.elevate));
    assert!(plan_of("fs delete --name a --in home --kind file --elevate").steps[0].elevate);
}

#[test]
fn a_mistyped_word_names_its_flag() {
    for (line, flag) in [
        ("fs list --in HOME", "--in"),
        ("fs list --in dwnloads", "--in"),
        ("fs list --in home\u{a0}--elevate", "--in"),
        (
            "fs move --name a --in home --kind file --to-in nowhere",
            "--to-in",
        ),
        ("fs read --name a --in home --kind dir", "--kind"),
    ] {
        let why = refused(line);
        assert!(
            why.contains(flag) && why.contains("is not one of"),
            "{line:?}: {why}"
        );
    }
    assert!(refused("fs list --in HOME").contains("downloads"));
}

#[test]
fn what_is_refused_cannot_forge_the_screen() {
    let host = files();
    for line in [
        "fs read --name 'a\u{1b}[2J' --in home\u{1b}]0;x\u{7}",
        "\u{202e}fs list --in home",
        "fs list --\u{1b}[31mred",
    ] {
        let result = typed(line, &host, false, Answers::yes(), MemoryLog::default());
        let text = result.outcome.render();
        assert!(matches!(result.outcome, Outcome::BadCommand(_)), "{line:?}");
        assert!(
            !text.contains('\u{1b}') && !text.contains('\u{202e}'),
            "{text:?}"
        );
    }
}

#[test]
fn the_forms_each_argument_takes_are_the_ones_the_schema_allows() {
    let schema: Value = serde_json::from_str(&docs("action-plan.schema.json")).unwrap();
    let defs = &schema["$defs"];
    let form_of = |reference: &str| match reference.rsplit('/').next().unwrap() {
        "refPlace" => "place",
        "refPath" => "path",
        "refNamed" | "refFolder" => "named",
        "refUnderHome" | "refUnderHomeFolder" => "under_home",
        "refNew" => "new",
        "refFromStep" => "step",
        other => panic!("{other}"),
    };
    let mut checked = 0;
    for action in command::actions() {
        let forms = command::reference_forms(&action);
        let step_def = defs["step"]["oneOf"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["$ref"].as_str().unwrap().rsplit('/').next().unwrap())
            .find(|name| defs[name]["properties"]["action"]["const"] == action.as_str())
            .unwrap();
        let args = &defs[step_def]["properties"]["args"]["properties"];
        for key in ["target", "from", "to"] {
            let Some(argument) = args.get(key) else {
                continue;
            };
            let type_name = argument["$ref"]
                .as_str()
                .unwrap()
                .rsplit('/')
                .next()
                .unwrap();
            let mut allowed: Vec<&str> = defs[type_name]["oneOf"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| form_of(r["$ref"].as_str().unwrap()))
                .collect();
            let mut ours = forms.iter().find(|(k, _)| *k == key).unwrap().1.clone();
            allowed.sort();
            ours.sort();
            assert_eq!(allowed, ours, "{action} {key}");
            checked += 1;
        }
        assert_eq!(
            forms.len(),
            args.as_object()
                .unwrap()
                .keys()
                .filter(|k| ["target", "from", "to"].contains(&k.as_str()))
                .count(),
            "{action}"
        );
    }
    assert_eq!(checked, 9);
}

#[test]
fn flags_after_the_double_dash_belong_to_the_program() {
    let plan = plan_of("process run --program tar -- x --elevate --after s1");
    assert!(!plan.steps[0].elevate && plan.steps[0].depends_on.is_empty());
    assert!(format!("{:?}", plan.steps[0].spec).contains("\"--elevate\""));
    let Ok(Parsed::Help(text)) = command::parse("help") else {
        panic!("help")
    };
    let line = text.lines().find(|l| l.contains("process run")).unwrap();
    assert!(line.ends_with("[-- ARGUMENT...]"), "{line}");
}

#[test]
fn a_word_the_format_checks_names_its_flag() {
    for (line, flag) in [
        (
            "fs write --new-name n --in home --content x --mode Create",
            "--mode",
        ),
        ("user manage --operation foo --user bob", "--operation"),
    ] {
        let why = refused(line);
        assert!(
            why.contains(flag) && why.contains("is not one of"),
            "{line}: {why}"
        );
    }
}

#[test]
fn the_shell_applies_the_formats_semantic_rules_to_a_typed_line() {
    // Each line is well formed as words and as structure, and breaks a rule only `check_rules` knows.
    for line in [
        "fs list --in home --after s9",
        "fs stat --name a --in home --after s2; fs stat --name b --in home --after s1",
        "fs stat --from-step s2; fs stat --name a --in home",
        "user manage --operation add --user bob --group staff",
    ] {
        let host = files();
        let result = typed(line, &host, false, Answers::yes(), MemoryLog::default());
        assert!(
            matches!(result.outcome, Outcome::BadCommand(_)),
            "{line}: {:?}",
            result.outcome
        );
        assert!(host.executed.borrow().is_empty(), "{line}");
        assert_eq!(result.log.events.len(), 1, "{line}");
        assert_eq!(result.log.events[0]["accepted"], false, "{line}");
    }
}

#[test]
fn the_words_of_each_enum_flag_are_the_schemas_and_each_one_can_be_typed() {
    let schema: Value = serde_json::from_str(&docs("action-plan.schema.json")).unwrap();
    let defs = &schema["$defs"];
    let mut checked = 0;
    for (def, action, key, flag) in [
        ("fsWrite", "fs.write", "mode", "--mode"),
        ("userManage", "user.manage", "operation", "--operation"),
    ] {
        let mut schema_words: Vec<String> =
            defs[def]["properties"]["args"]["properties"][key]["enum"]
                .as_array()
                .unwrap()
                .iter()
                .map(|w| w.as_str().unwrap().to_owned())
                .collect();
        let mut ours: Vec<String> = command::choice_words(action)
            .into_iter()
            .find(|(k, _)| *k == key)
            .unwrap()
            .1
            .into_iter()
            .map(str::to_owned)
            .collect();
        schema_words.sort();
        ours.sort();
        assert_eq!(schema_words, ours, "{action}");
        for word in &schema_words {
            let line = if action == "fs.write" {
                format!("fs write --new-name n --in home --content c {flag} {word}")
            } else if word.ends_with("group") || word.ends_with("_group") {
                format!("user manage {flag} {word} --user bob --group staff")
            } else {
                format!("user manage {flag} {word} --user bob")
            };
            plan_of(&line);
            checked += 1;
        }
    }
    assert_eq!(checked, 7);
}

#[test]
fn errors_name_what_to_type_and_what_is_wrong() {
    let why = refused("fs delete");
    assert!(
        why.contains("--kind file|folder|any or --from-step sN") && !why.contains("--new-name"),
        "{why}"
    );
    let why = refused("fs find_largest --count 3");
    assert!(
        why.contains("--in PLACE") && !why.contains("--from-step"),
        "{why}"
    );
    assert!(
        refused("fs write --content x").contains("--new-name NAME (--in PLACE | --in-path PATH)")
    );
    assert!(refused("fs move --name a --in home --kind file").contains("--to-in PLACE"));
    let why = refused("fs find_largest --in home --count 5000000000");
    assert!(why.contains("--count") && why.contains("1 to 100"), "{why}");
    for line in [
        "fs \"list\" --in home",
        "\"fs\" list --in home",
        "f\\s list --in home",
    ] {
        assert!(refused(line).contains("without quotes"), "{line}");
    }
}

#[test]
fn help_states_the_bound_the_code_enforces() {
    let Ok(Parsed::Help(text)) = command::parse("help") else {
        panic!("help")
    };
    assert!(text.contains(&format!("up to {} steps", crate::plan::MAX_STEPS)));
    assert!(text.contains("--count is 1 to 100"));
}

#[test]
fn what_removes_or_moves_an_item_never_gets_a_default_kind() {
    for line in [
        "fs delete --name a --in home",
        "fs move --name a --in home --to-in documents",
        "fs write --name a --in home --content x --mode overwrite",
    ] {
        let why = refused(line);
        assert!(
            why.contains("--kind") && why.contains("writes, removes or moves"),
            "{line}: {why}"
        );
    }
    for kind in ["file", "folder", "any"] {
        plan_of(&format!("fs delete --name a --in home --kind {kind}"));
    }
    // A name that only reads still has a default, and says so.
    plan_of("fs read --name a --in home");
    let Ok(Parsed::Help(text)) = command::parse("help") else {
        panic!("help")
    };
    let line = text.lines().find(|l| l.contains("fs delete")).unwrap();
    assert!(line.contains("--kind file|folder|any"), "{line}");
}

#[test]
fn the_arguments_of_each_command_are_the_schemas() {
    let schema: Value = serde_json::from_str(&docs("action-plan.schema.json")).unwrap();
    let defs = &schema["$defs"];
    for reference in defs["step"]["oneOf"].as_array().unwrap() {
        let def = &defs[reference["$ref"]
            .as_str()
            .unwrap()
            .rsplit('/')
            .next()
            .unwrap()];
        let action = def["properties"]["action"]["const"].as_str().unwrap();
        let mut in_schema: Vec<&str> = def["properties"]["args"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let mut ours = command::arg_keys(action);
        in_schema.sort();
        ours.sort();
        assert_eq!(in_schema, ours, "{action}");
    }
}

#[test]
fn the_lists_and_bounds_the_command_layer_builds_are_not_cut_short() {
    let argv = |n: usize| format!("process run --program ls -- {}", vec!["a"; n].join(" "));
    plan_of(&argv(32));
    assert!(refused(&argv(33)).contains("more than 32"));
    let long = "x".repeat(1025);
    assert!(refused(&format!("process run --program ls -- {long}")).contains("1024"));
    // The count of argument items is the format's, so nothing is dropped on the way.
    let plan = plan_of(&argv(32));
    assert!(format!("{:?}", plan.steps[0].spec).matches("\"a\"").count() == 32);
}

#[test]
fn separators_flags_and_dependencies_are_read_exactly() {
    assert!(refused("fs list ---in home").contains("unknown flag"));
    assert!(refused("fs list -in home").contains("unexpected"));
    // A carriage return separates like a space or a newline.
    plan_of("fs\rlist\r--in\rhome");
    plan_of("fs\tlist\t--in\thome");
    // Naming a dependency twice, or once and again by a reference, is one dependency.
    let plan = plan_of("fs list --in home; fs stat --from-step s1 --after s1 --after s1");
    assert_eq!(plan.steps[1].depends_on.len(), 1);
}

#[test]
fn help_and_errors_advertise_only_flags_that_are_accepted() {
    let Ok(Parsed::Help(text)) = command::parse("help") else {
        panic!("help")
    };
    assert!(!text.contains("--to-kind"), "{text}");
    assert!(!refused("fs move --name a --in home --kind file").contains("--to-kind"));
    // Every `--flag` the help prints is one some command accepts.
    let known = command::known_flags();
    let mut seen = 0;
    for word in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')) {
        if let Some(flag) = word.strip_prefix("--").filter(|f| !f.is_empty()) {
            assert!(known.iter().any(|k| k == flag), "help prints --{flag}");
            seen += 1;
        }
    }
    assert!(seen > 10);
}

#[test]
fn an_empty_argument_is_an_argument() {
    let plan = plan_of("process run --program echo -- \"\" x");
    assert!(format!("{:?}", plan.steps[0].spec).contains("argv: [\"\", \"x\"]"));
}

#[test]
fn a_number_is_digits_and_a_dotted_name_gets_a_hint() {
    for count in ["+3", "007x", "-1", "3.0"] {
        let why = refused(&format!("fs find_largest --in home --count {count}"));
        assert!(why.contains("whole number"), "{count}: {why}");
    }
    plan_of("fs find_largest --in home --count 007");
    let why = refused("fs.list --in downloads");
    assert!(
        why.contains("unknown command fs.list;") && why.contains("`fs list`"),
        "{why}"
    );
    assert!(!why.contains("--in"), "{why}");
}

#[test]
fn a_line_the_format_refuses_is_logged_with_its_document_and_the_reason() {
    let result = typed(
        "fs find_largest --in home --count 0",
        &files(),
        false,
        Answers::yes(),
        MemoryLog::default(),
    );
    let event = &result.log.events[0];
    assert_eq!(event["accepted"], false);
    assert!(event["error"].as_str().unwrap().contains("1 to 100"));
    let json = event["plan"]
        .as_str()
        .expect("the document is in the record");
    assert!(json.contains("\"count\":0"), "{json}");
}

// --- paths, `--under-home`, `session cd` and the short words (ADR 0018, ADR 0022) ---

fn home_host() -> FakeHost {
    FakeHost::with_files(&[
        ("/home/ana/Downloads/films/new.mkv", 2_300),
        ("/home/ana/Downloads/notes.txt", 5),
        ("/var/log/old.log", 3),
    ])
    .dir("/home/ana/Downloads")
    .dir("/home/ana/Downloads/films")
    .dir("/var/log")
    .place(crate::reference::Place::Downloads, "/home/ana/Downloads")
}

fn one_step_plan(action: &str, args: &str, elevate: bool) -> String {
    let elevate = if action == "session.cd" {
        String::new()
    } else {
        format!(r#","elevate":{elevate}"#)
    };
    format!(
        r#"{{"kind":"plan","version":3,"summary":"{action}","steps":[{{"id":"s1","action":"{action}","args":{args},"depends_on":[]{elevate}}}]}}"#
    )
}

#[test]
fn a_path_lists_and_ranks_through_the_same_pipeline_as_the_models_path() {
    let json = one_step_plan(
        "fs.list",
        r#"{"target":{"path":"~/Downloads/films"}}"#,
        false,
    );
    let seen = same_both_ways(
        "fs list --path ~/Downloads/films",
        &json,
        home_host,
        false,
        Answers::yes,
    );
    assert_eq!(seen["executed"].as_array().unwrap().len(), 1, "{seen}");
}

#[test]
fn a_path_to_a_system_folder_is_judged_by_the_broker_not_by_the_command_layer() {
    // The command layer names `/var/log`; asking for root does not help a person
    // who is not an administrator, exactly as for the model's plan.
    let json = one_step_plan(
        "fs.delete",
        r#"{"target":{"path":"/var/log/old.log"}}"#,
        true,
    );
    let seen = same_both_ways(
        "fs delete --path /var/log/old.log --elevate",
        &json,
        home_host,
        false,
        Answers::yes,
    );
    assert!(seen["executed"].as_array().unwrap().is_empty(), "{seen}");
    assert!(seen["root_panels"].as_array().unwrap().is_empty(), "{seen}");
    // An administrator gets the panel, and its own confirmation, as for the model.
    let seen = same_both_ways(
        "fs delete --path /var/log/old.log --elevate",
        &json,
        home_host,
        true,
        Answers::yes,
    );
    assert_eq!(seen["root_panels"].as_array().unwrap().len(), 1, "{seen}");
}

#[test]
fn a_typed_cd_enters_the_folder_like_the_models_and_a_refused_one_stays_put() {
    let host = home_host();
    let mut session = SessionState {
        cwd: "/home/ana".into(),
    };
    let go = |session: &mut SessionState, line: &str| {
        let (mut answers, mut log) = (Answers::yes(), MemoryLog::default());
        let outcome = Shell {
            session,
            authority: &Authority::full(user(false)),
            host: &host,
            planner: &NeverAsked,
            confirm: &mut answers,
            log: &mut log,
        }
        .handle_command(line);
        (outcome, log)
    };
    let (outcome, log) = go(&mut session, "cd ~/Downloads");
    assert!(matches!(outcome, Outcome::Executed { asked: false, .. }));
    assert!(outcome.render().contains("now in ~/Downloads"));
    assert!(log.events.iter().any(|e| e["event"] == "cd"));
    let command = &log.events[0];
    assert_eq!(command["event"], "command");
    assert_eq!(command["cwd"], "\"/home/ana\"");
    // A relative path now starts from there.
    let (outcome, _) = go(&mut session, "ls films");
    let Outcome::Executed { review, .. } = &outcome else {
        panic!("{}", outcome.render())
    };
    assert_eq!(
        review.steps[0].items[0].action.describe(),
        r#"fs.list path="/home/ana/Downloads/films""#
    );
    // A file, a missing folder and a folder name that is not there leave it where it was.
    for line in [
        "cd notes.txt",
        "cd nowhere",
        "session cd --name nowhere --under-home",
    ] {
        let (outcome, _) = go(&mut session, line);
        assert!(!matches!(outcome, Outcome::Executed { .. }), "{line}");
    }
    assert_eq!(session.cwd, std::path::PathBuf::from("/home/ana/Downloads"));
    // A `cd` is a plan of its own, as for the model.
    let (outcome, _) = go(&mut session, "cd ~; fs list --in home");
    assert!(
        matches!(outcome, Outcome::BadCommand(_)),
        "{}",
        outcome.render()
    );
    assert_eq!(session.cwd, std::path::PathBuf::from("/home/ana/Downloads"));
}

#[test]
fn the_short_words_expand_to_the_mechanical_commands_and_nothing_else() {
    let same = |short: &str, long: &str| {
        assert_eq!(plan_of(short).steps, plan_of(long).steps, "{short}");
    };
    same("cd ~/Downloads", "session cd --path ~/Downloads");
    same("ls", "fs list --path .");
    same("ls ~/Downloads", "fs list --path ~/Downloads");
    same(
        "ls; ls --after s1",
        "fs list --path .; fs list --path . --after s1",
    );
    // A reference by flag is the person's own: no default path next to it.
    same("ls --in downloads", "fs list --in downloads");
    same("ls --name a --under-home", "fs list --name a --under-home");
    same("ls --path x", "fs list --path x");
    same("rm notes/old.txt", "fs delete --path notes/old.txt");
    same(
        "rm notes/old.txt --elevate",
        "fs delete --path notes/old.txt --elevate",
    );
    // A quoted path is one path, and a quoted `--x` is a path, not a flag.
    same(
        "rm 'my notes/a b.txt'",
        "fs delete --path 'my notes/a b.txt'",
    );
    same("ls '--odd'", "fs list --path '--odd'");
    // The summary says what the action is, as for the long form.
    assert_eq!(plan_of("rm x").summary, "fs.delete");
}

#[test]
fn a_short_word_refuses_what_it_cannot_say_and_never_guesses_a_flag_of_the_shells_own() {
    for (line, mention) in [
        ("cd", "for example `cd ~/Downloads`"),
        ("rm", "needs a path"),
        ("rm --elevate", "needs a path"),
        ("rm -rf /", "looks like a flag"),
        ("rm -f x", "looks like a flag"),
        ("ls -la", "looks like a flag"),
        ("ls a b", "one path only"),
        ("cd a b", "one path only"),
        ("ls x --path y", "twice"),
        ("rm -'rf' x", "looks like a flag"),
        ("rm -\"f\"", "looks like a flag"),
        ("'rm' x", "without quotes"),
        ("r\\m x", "without quotes"),
        ("tree", "group and an action"),
    ] {
        let why = refused(line);
        assert!(why.contains(mention), "{line:?}: {why}");
    }
    // A file really named `-x` is written so that it cannot be mistaken for a flag.
    assert_eq!(
        plan_of("rm ./-x").steps,
        plan_of("fs delete --path ./-x").steps
    );
    assert_eq!(
        plan_of("rm '-x'").steps,
        plan_of("fs delete --path -x").steps
    );
}

#[test]
fn the_new_references_are_refused_where_the_format_refuses_them() {
    for (line, mention) in [
        ("fs list --path a --in home", "stands alone"),
        ("fs list --path a --name b", "stands alone"),
        ("fs list --path a --kind file", "stands alone"),
        ("fs list --path a --under-home", "stands alone"),
        ("fs list --path a --path b", "twice"),
        ("fs list --in home --in-path a", "two ways to say where"),
        ("fs list --in-path a", "to name a path, use --path"),
        ("fs list --under-home", "needs --name"),
        ("fs list --name a --under-home --in home", "takes no --in"),
        ("fs list --name a --under-home --in-path x", "takes no --in"),
        (
            "fs list --new-name a --under-home",
            "two different references",
        ),
        ("fs list --name a --under-home --under-home", "twice"),
        ("fs list --name a --in-path", "needs a value"),
        ("fs list --name a", "needs --in or --in-path"),
        (
            "fs write --new-name a --content x",
            "needs --in or --in-path",
        ),
        ("fs write --name a --under-home --content x", "needs --kind"),
        ("fs delete --name a --under-home", "needs --kind"),
        ("fs delete --name a --in-path ~ ", "needs --kind"),
        ("fs move --path a --to-under-home", "needs --to-name"),
        (
            "fs move --path a --to-in-path ~",
            "to name a path, use --to-path",
        ),
        (
            "fs move --path a --to-name b --to-in home --to-kind folder",
            "unknown flag",
        ),
        ("fs move --path a --to-path b --to-in home", "stands alone"),
        ("session cd --from-step s1", "unknown flag"),
        ("session cd --new-name a --in home", "unknown flag"),
        (
            "session cd --name a --in home --kind folder",
            "unknown flag",
        ),
        ("session cd --path ~ --elevate", "unknown flag"),
        ("session cd", "needs a reference"),
        ("fs list --path ''", "1 to 4096"),
        ("fs list --path 'a\u{0}b'", "NUL"),
    ] {
        let why = refused(line);
        assert!(why.contains(mention), "{line:?}: {why}");
    }
    let long = format!("fs list --path {}", "a".repeat(4097));
    assert!(refused(&long).contains("4096"), "{}", refused(&long));
}

#[test]
fn a_name_in_a_folder_or_below_the_home_is_a_folder_for_cd_and_for_a_destination() {
    let json = |line: &str| {
        let Ok(Parsed::Plan(json)) = command::parse(line) else {
            panic!("{line}")
        };
        serde_json::from_str::<Value>(&json).unwrap()["steps"][0]["args"].clone()
    };
    assert_eq!(
        json("session cd --name films --in-path ~/Downloads")["target"],
        serde_json::json!({"name": "films", "in": {"path": "~/Downloads"}, "kind": "folder"})
    );
    assert_eq!(
        json("fs move --path a --to-name films --to-under-home")["to"],
        serde_json::json!({"under_home": true, "name": "films", "kind": "folder"})
    );
}

#[test]
fn help_lists_the_short_words_and_the_new_flags_and_only_flags_that_are_accepted() {
    let Ok(Parsed::Help(text)) = command::parse("help") else {
        panic!("help")
    };
    for needle in [
        "session cd",
        "cd PATH",
        "ls [PATH]",
        "rm PATH",
        "--path PATH",
        "--in-path PATH",
        "--under-home",
        "--to-path PATH",
    ] {
        assert!(text.contains(needle), "{needle}\n{text}");
    }
    assert!(!text.contains("everywhere"), "{text}");
}
