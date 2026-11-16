//! Parsing and checking what the model emitted (ADR 0011).
//!
//! `parse` is the structural half, equivalent to `docs/schemas/action-plan.schema.json`;
//! `check_rules` is the semantic half, equivalent to `docs/schemas/validate.py`.
//! The tests run both over every example next to the schema, so the Rust
//! reading of the format cannot drift from the schema file unnoticed.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::PathBuf;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use crate::action::{Action, RootRule, UserOperation, WriteMode};

pub const MAX_STEPS: usize = 16;
const MAX_DEPENDENCIES: usize = 15;
const MAX_PATH_CHARS: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StepId(String);

impl StepId {
    fn parse(text: &str) -> Result<Self, PlanError> {
        let digits = text.strip_prefix('s').unwrap_or("");
        let valid = (1..=2).contains(&digits.len()) && digits.chars().all(|c| c.is_ascii_digit());
        if valid {
            Ok(Self(text.to_owned()))
        } else {
            Err(PlanError::new(format!("invalid step id {text:?}")))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for StepId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanError(String);

impl PlanError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PlanError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub id: StepId,
    pub depends_on: Vec<StepId>,
    /// The model's request for root. Only a request: the broker decides.
    pub elevate: bool,
    /// With `reference` set, the path position the reference fills is empty
    /// until the broker expands it.
    pub action: Action,
    pub reference: Option<StepId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub summary: String,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Document {
    Plan(Plan),
    /// A question instead of a plan. It carries no action, so it runs nothing.
    Clarify(String),
}

impl Plan {
    /// Step indexes with every step after the steps it depends on. Only
    /// meaningful for a plan that passed `check_rules`.
    pub fn execution_order(&self) -> Vec<usize> {
        let mut placed: HashSet<&StepId> = HashSet::new();
        let mut order = Vec::new();
        while order.len() < self.steps.len() {
            let before = order.len();
            for (index, step) in self.steps.iter().enumerate() {
                if !placed.contains(&step.id) && step.depends_on.iter().all(|d| placed.contains(d))
                {
                    placed.insert(&step.id);
                    order.push(index);
                }
            }
            if order.len() == before {
                break;
            }
        }
        order
    }
}

/// Structure and semantics together: what the model's text must pass before
/// anything else looks at it.
pub fn parse_checked(text: &str) -> Result<Document, PlanError> {
    let document = parse(text)?;
    if let Document::Plan(plan) = &document {
        check_rules(plan)?;
    }
    Ok(document)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPlan {
    #[serde(rename = "kind")]
    _kind: String,
    version: u32,
    summary: String,
    steps: Vec<RawStep>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawClarify {
    #[serde(rename = "kind")]
    _kind: String,
    version: u32,
    question: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStep {
    id: String,
    action: String,
    args: Value,
    depends_on: Vec<String>,
    #[serde(default, deserialize_with = "present_bool")]
    elevate: Option<bool>,
}

pub fn parse(text: &str) -> Result<Document, PlanError> {
    let value: Value =
        serde_json::from_str(text).map_err(|e| PlanError::new(format!("not JSON: {e}")))?;
    match value.get("kind").and_then(Value::as_str) {
        Some("plan") => {
            structs_are_objects(&value)?;
            let raw: RawPlan = from_value(value)?;
            check_version(raw.version)?;
            bounded("summary", &raw.summary, 1, 200)?;
            if raw.steps.is_empty() || raw.steps.len() > MAX_STEPS {
                return Err(PlanError::new(format!("a plan has 1 to {MAX_STEPS} steps")));
            }
            let steps = raw
                .steps
                .into_iter()
                .map(convert_step)
                .collect::<Result<_, _>>()?;
            Ok(Document::Plan(Plan {
                summary: raw.summary,
                steps,
            }))
        }
        Some("clarify") => {
            let raw: RawClarify = from_value(value)?;
            check_version(raw.version)?;
            bounded("question", &raw.question, 1, 300)?;
            Ok(Document::Clarify(raw.question))
        }
        _ => Err(PlanError::new("kind must be \"plan\" or \"clarify\"")),
    }
}

fn check_version(version: u32) -> Result<(), PlanError> {
    if version == 1 {
        Ok(())
    } else {
        Err(PlanError::new(format!("unsupported version {version}")))
    }
}

/// serde also reads a struct from a JSON array (by position); the schema does not.
fn structs_are_objects(plan: &Value) -> Result<(), PlanError> {
    let Some(steps) = plan.get("steps").and_then(Value::as_array) else {
        return Ok(());
    };
    for step in steps {
        let args = step.get("args");
        let step_is_object = step.is_object();
        let args_are_object = args.is_none_or(Value::is_object);
        let path_is_array = ["path", "from"]
            .iter()
            .any(|key| args.and_then(|a| a.get(key)).is_some_and(Value::is_array));
        // Unit enums are strings; serde also reads `{"create": null}`.
        let enum_is_not_string = ["mode", "operation"].iter().any(|key| {
            args.and_then(|a| a.get(key))
                .is_some_and(|v| !v.is_string())
        });
        if !(step_is_object && args_are_object) || path_is_array {
            return Err(PlanError::new(
                "steps, args and references must be JSON objects",
            ));
        }
        if enum_is_not_string {
            return Err(PlanError::new("mode and operation must be strings"));
        }
    }
    Ok(())
}

fn from_value<T: DeserializeOwned>(value: Value) -> Result<T, PlanError> {
    serde_json::from_value(value).map_err(|e| PlanError::new(e.to_string()))
}

fn bounded(field: &str, text: &str, min: usize, max: usize) -> Result<(), PlanError> {
    let length = text.chars().count();
    if (min..=max).contains(&length) {
        Ok(())
    } else {
        Err(PlanError::new(format!(
            "{field} must have {min} to {max} characters, has {length}"
        )))
    }
}

/// `first` then up to `max_rest` characters of `rest`.
fn named(
    field: &str,
    text: &str,
    first: fn(char) -> bool,
    rest: fn(char) -> bool,
    max_rest: usize,
) -> Result<(), PlanError> {
    let mut chars = text.chars();
    let valid = chars.next().is_some_and(first) && {
        let tail: Vec<char> = chars.collect();
        tail.len() <= max_rest && tail.into_iter().all(rest)
    };
    if valid {
        Ok(())
    } else {
        Err(PlanError::new(format!(
            "{field} {text:?} has an invalid form"
        )))
    }
}

fn account_name(field: &str, text: &str) -> Result<(), PlanError> {
    named(
        field,
        text,
        |c| c.is_ascii_lowercase() || c == '_',
        |c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-',
        31,
    )
}

#[derive(Deserialize)]
#[serde(untagged)]
enum PathArg {
    Literal(String),
    Reference(Reference),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reference {
    from_step: String,
}

impl PathArg {
    /// A plain string is a literal path, even one that looks like a reference.
    fn split(self, field: &str) -> Result<(PathBuf, Option<StepId>), PlanError> {
        match self {
            Self::Literal(text) => Ok((literal_path(field, text)?, None)),
            Self::Reference(r) => Ok((PathBuf::new(), Some(StepId::parse(&r.from_step)?))),
        }
    }
}

fn literal_path(field: &str, text: String) -> Result<PathBuf, PlanError> {
    bounded(field, &text, 1, MAX_PATH_CHARS)?;
    Ok(PathBuf::from(text))
}

/// A key that is there must hold a boolean: `null` is not the same as absent.
fn present_bool<'de, D: Deserializer<'de>>(d: D) -> Result<Option<bool>, D::Error> {
    bool::deserialize(d).map(Some)
}

fn nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(d)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathArgs {
    path: PathArg,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FindLargestArgs {
    path: String,
    count: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteArgs {
    path: String,
    content: String,
    mode: WriteMode,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MoveArgs {
    from: PathArg,
    to: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunArgs {
    program: String,
    argv: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UserArgs {
    operation: UserOperation,
    user: String,
    #[serde(deserialize_with = "nullable")]
    group: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigArgs {
    key: String,
    value: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SoulArgs {
    rationale: String,
    new_text: String,
}

fn convert_step(raw: RawStep) -> Result<Step, PlanError> {
    let id = StepId::parse(&raw.id)?;
    if raw.depends_on.len() > MAX_DEPENDENCIES {
        return Err(PlanError::new(format!(
            "{id} has more than {MAX_DEPENDENCIES} dependencies"
        )));
    }
    let depends_on = raw
        .depends_on
        .iter()
        .map(|d| StepId::parse(d))
        .collect::<Result<Vec<_>, _>>()?;
    let (action, reference) = convert_action(&raw.action, raw.args)?;
    let elevate = match (action.root_rule(), raw.elevate) {
        (RootRule::OnRequest, Some(requested)) => requested,
        (RootRule::OnRequest, None) => {
            return Err(PlanError::new(format!("{id}: elevate is required")));
        }
        (_, Some(_)) => {
            return Err(PlanError::new(format!(
                "{id}: {} does not take elevate",
                action.name()
            )));
        }
        (_, None) => false,
    };
    Ok(Step {
        id,
        depends_on,
        elevate,
        action,
        reference,
    })
}

fn convert_action(name: &str, args: Value) -> Result<(Action, Option<StepId>), PlanError> {
    let path_action = |args: Value, build: fn(PathBuf) -> Action| {
        let (path, reference) = from_value::<PathArgs>(args)?.path.split("path")?;
        Ok((build(path), reference))
    };
    match name {
        "fs.list" => path_action(args, |path| Action::List { path }),
        "fs.stat" => path_action(args, |path| Action::Stat { path }),
        "fs.read" => path_action(args, |path| Action::Read { path }),
        "fs.delete" => path_action(args, |path| Action::Delete { path }),
        "fs.find_largest" => {
            let a: FindLargestArgs = from_value(args)?;
            if !(1..=100).contains(&a.count) {
                return Err(PlanError::new("count must be 1 to 100"));
            }
            Ok((
                Action::FindLargest {
                    path: literal_path("path", a.path)?,
                    count: a.count,
                },
                None,
            ))
        }
        "fs.write" => {
            let a: WriteArgs = from_value(args)?;
            bounded("content", &a.content, 0, 65536)?;
            Ok((
                Action::Write {
                    path: literal_path("path", a.path)?,
                    content: a.content,
                    mode: a.mode,
                },
                None,
            ))
        }
        "fs.move" => {
            let a: MoveArgs = from_value(args)?;
            let (from, reference) = a.from.split("from")?;
            Ok((
                Action::Move {
                    from,
                    to: literal_path("to", a.to)?,
                },
                reference,
            ))
        }
        "process.run" => {
            let a: RunArgs = from_value(args)?;
            named(
                "program",
                &a.program,
                |c| c.is_ascii_lowercase(),
                |c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'),
                63,
            )?;
            if a.argv.len() > 32 {
                return Err(PlanError::new("argv has more than 32 items"));
            }
            a.argv
                .iter()
                .try_for_each(|arg| bounded("argv item", arg, 0, 1024))?;
            Ok((
                Action::Run {
                    program: a.program,
                    argv: a.argv,
                },
                None,
            ))
        }
        "user.manage" => {
            let a: UserArgs = from_value(args)?;
            account_name("user", &a.user)?;
            if let Some(group) = &a.group {
                account_name("group", group)?;
            }
            Ok((
                Action::UserManage {
                    operation: a.operation,
                    user: a.user,
                    group: a.group,
                },
                None,
            ))
        }
        "system.configure" => {
            let a: ConfigArgs = from_value(args)?;
            named(
                "key",
                &a.key,
                |c| c.is_ascii_lowercase(),
                |c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '.'),
                63,
            )?;
            bounded("value", &a.value, 0, 256)?;
            Ok((
                Action::SystemConfigure {
                    key: a.key,
                    value: a.value,
                },
                None,
            ))
        }
        "soul.propose_change" => {
            let a: SoulArgs = from_value(args)?;
            bounded("rationale", &a.rationale, 1, 500)?;
            bounded("new_text", &a.new_text, 1, 16384)?;
            Ok((
                Action::SoulProposeChange {
                    rationale: a.rationale,
                    new_text: a.new_text,
                },
                None,
            ))
        }
        other => Err(PlanError::new(format!("unknown action {other:?}"))),
    }
}

/// The rules JSON Schema cannot express (ADR 0011, "How the broker validates").
pub fn check_rules(plan: &Plan) -> Result<(), PlanError> {
    let mut by_id: HashMap<&StepId, &Step> = HashMap::new();
    for step in &plan.steps {
        if by_id.insert(&step.id, step).is_some() {
            return Err(PlanError::new(format!("duplicate step id {}", step.id)));
        }
    }
    for step in &plan.steps {
        for dependency in &step.depends_on {
            if dependency == &step.id {
                return Err(PlanError::new(format!("{} depends on itself", step.id)));
            }
            if !by_id.contains_key(dependency) {
                return Err(PlanError::new(format!(
                    "{} depends on unknown step {dependency}",
                    step.id
                )));
            }
        }
        if let Some(source) = &step.reference {
            if !step.depends_on.contains(source) {
                return Err(PlanError::new(format!(
                    "{} uses {source} without depending on it",
                    step.id
                )));
            }
            if !by_id[source].action.produces_paths() {
                return Err(PlanError::new(format!("{source} does not produce paths")));
            }
        }
        if let Action::UserManage {
            operation, group, ..
        } = &step.action
            && group.is_some() != operation.needs_group()
        {
            return Err(PlanError::new(format!(
                "{}: group must be set exactly for group operations",
                step.id
            )));
        }
    }
    if plan.execution_order().len() != plan.steps.len() {
        return Err(PlanError::new("dependency cycle"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;

    fn examples(kind: &str) -> Vec<(String, String)> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../docs/schemas/examples")
            .join(kind);
        let mut found: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .map(|p| {
                (
                    p.file_name().unwrap().to_string_lossy().into_owned(),
                    fs::read_to_string(&p).unwrap(),
                )
            })
            .collect();
        found.sort();
        found
    }

    #[test]
    fn every_valid_example_passes() {
        let valid = examples("valid");
        assert!(!valid.is_empty());
        for (name, text) in valid {
            parse_checked(&text).unwrap_or_else(|e| panic!("{name} should pass: {e}"));
        }
    }

    #[test]
    fn every_schema_example_is_refused_by_the_structure() {
        for (name, text) in examples("invalid")
            .into_iter()
            .filter(|(n, _)| n.starts_with("schema-"))
        {
            assert!(
                parse(&text).is_err(),
                "{name} should be refused by the structural half"
            );
        }
    }

    #[test]
    fn every_semantic_example_passes_the_structure_and_fails_the_rules() {
        let cases: Vec<_> = examples("invalid")
            .into_iter()
            .filter(|(n, _)| n.starts_with("semantic-"))
            .collect();
        assert!(!cases.is_empty());
        for (name, text) in cases {
            match parse(&text) {
                Ok(Document::Plan(plan)) => {
                    assert!(check_rules(&plan).is_err(), "{name} should break a rule")
                }
                other => panic!("{name} should parse as a plan, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_string_that_looks_like_a_reference_is_a_literal_path() {
        let text = r#"{"kind":"plan","version":1,"summary":"x","steps":[
            {"id":"s1","action":"fs.delete","args":{"path":"s1.path"},"depends_on":[],"elevate":false}]}"#;
        let Document::Plan(plan) = parse_checked(text).unwrap() else {
            panic!("not a plan")
        };
        assert_eq!(
            plan.steps[0].action,
            Action::Delete {
                path: PathBuf::from("s1.path")
            }
        );
        assert_eq!(plan.steps[0].reference, None);
    }

    #[test]
    fn a_null_elevate_is_not_the_same_as_no_elevate() {
        let text = r#"{"kind":"plan","version":1,"summary":"x","steps":[
            {"id":"s1","action":"user.manage","args":{"operation":"add","user":"bob","group":null},"depends_on":[],"elevate":null}]}"#;
        assert!(parse(text).is_err());
    }

    #[test]
    fn structs_given_as_arrays_are_refused_like_the_schema_does() {
        for text in [
            r#"{"kind":"plan","version":1,"summary":"x","steps":[["s1","fs.list",["."],[],false]]}"#,
            r#"{"kind":"plan","version":1,"summary":"x","steps":[{"id":"s1","action":"fs.write","args":["/tmp/x","c","create"],"depends_on":[],"elevate":false}]}"#,
            r#"{"kind":"plan","version":1,"summary":"x","steps":[
                {"id":"s1","action":"fs.list","args":{"path":"."},"depends_on":[],"elevate":false},
                {"id":"s2","action":"fs.stat","args":{"path":["s1"]},"depends_on":["s1"],"elevate":false}]}"#,
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn an_enum_given_as_an_object_is_refused_like_the_schema_does() {
        let text = r#"{"kind":"plan","version":1,"summary":"x","steps":[
            {"id":"s1","action":"fs.write","args":{"path":"a","content":"c","mode":{"create":null}},"depends_on":[],"elevate":false}]}"#;
        assert!(parse(text).is_err());
    }

    #[test]
    fn the_limits_of_the_format_are_enforced() {
        let step = |id: String, deps: &str| {
            format!(
                r#"{{"id":"{id}","action":"fs.stat","args":{{"path":"."}},"depends_on":{deps},"elevate":false}}"#
            )
        };
        let plan = |summary: &str, steps: &[String]| {
            format!(
                r#"{{"kind":"plan","version":1,"summary":"{summary}","steps":[{}]}}"#,
                steps.join(",")
            )
        };
        let steps = |n: usize| {
            (1..=n)
                .map(|i| step(format!("s{i}"), "[]"))
                .collect::<Vec<_>>()
        };
        assert!(parse(&plan("x", &steps(MAX_STEPS))).is_ok());
        assert!(parse(&plan("x", &steps(MAX_STEPS + 1))).is_err());
        assert!(parse(&plan(&"x".repeat(200), &steps(1))).is_ok());
        assert!(parse(&plan(&"x".repeat(201), &steps(1))).is_err());
        let deps = |n: usize| {
            format!(
                "[{}]",
                (1..=n)
                    .map(|i| format!(r#""s{}""#, i + 20))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        };
        assert!(parse(&plan("x", &[step("s1".into(), &deps(15))])).is_ok());
        assert!(parse(&plan("x", &[step("s1".into(), &deps(16))])).is_err());
    }

    #[test]
    fn step_ids_and_argv_items_follow_their_patterns_and_bounds() {
        let with_id = |id: &str| {
            format!(
                r#"{{"kind":"plan","version":1,"summary":"x","steps":[{{"id":"{id}","action":"fs.stat","args":{{"path":"."}},"depends_on":[],"elevate":false}}]}}"#
            )
        };
        for good in ["s1", "s12"] {
            assert!(parse(&with_id(good)).is_ok(), "{good}");
        }
        for bad in ["s", "s123", "x1", "S1", "1", "s1a", ""] {
            assert!(parse(&with_id(bad)).is_err(), "{bad}");
        }
        let run = |arg: &str| {
            format!(
                r#"{{"kind":"plan","version":1,"summary":"x","steps":[{{"id":"s1","action":"process.run","args":{{"program":"ls","argv":["{arg}"]}},"depends_on":[],"elevate":false}}]}}"#
            )
        };
        assert!(parse(&run(&"a".repeat(1024))).is_ok());
        assert!(parse(&run(&"a".repeat(1025))).is_err());
        let many = |n: usize| {
            let items = vec![r#""a""#; n].join(",");
            format!(
                r#"{{"kind":"plan","version":1,"summary":"x","steps":[{{"id":"s1","action":"process.run","args":{{"program":"ls","argv":[{items}]}},"depends_on":[],"elevate":false}}]}}"#
            )
        };
        assert!(parse(&many(32)).is_ok());
        assert!(parse(&many(33)).is_err());
    }

    #[test]
    fn a_proposed_soul_text_is_bounded() {
        let soul = |n: usize| {
            format!(
                r#"{{"kind":"plan","version":1,"summary":"x","steps":[{{"id":"s1","action":"soul.propose_change","args":{{"rationale":"r","new_text":"{}"}},"depends_on":[]}}]}}"#,
                "t".repeat(n)
            )
        };
        assert!(parse(&soul(16384)).is_ok());
        assert!(parse(&soul(16385)).is_err());
        assert!(parse(&soul(0)).is_err());
    }

    #[test]
    fn elevate_is_required_where_it_is_allowed() {
        let text = r#"{"kind":"plan","version":1,"summary":"x","steps":[
            {"id":"s1","action":"fs.stat","args":{"path":"."},"depends_on":[]}]}"#;
        assert!(parse(text).is_err());
    }

    #[test]
    fn text_around_the_json_is_refused() {
        assert!(parse("Sure! {\"kind\":\"clarify\",\"version\":1,\"question\":\"q\"}").is_err());
        assert!(parse(r#"{"kind":"clarify","version":1,"question":"q"} extra"#).is_err());
    }

    #[test]
    fn a_step_with_no_dependency_order_is_executed_after_its_dependencies() {
        let text = r#"{"kind":"plan","version":1,"summary":"x","steps":[
            {"id":"s2","action":"fs.stat","args":{"path":{"from_step":"s1"}},"depends_on":["s1"],"elevate":false},
            {"id":"s1","action":"fs.list","args":{"path":"."},"depends_on":[],"elevate":false}]}"#;
        let Document::Plan(plan) = parse_checked(text).unwrap() else {
            panic!("not a plan")
        };
        assert_eq!(plan.execution_order(), vec![1, 0]);
    }
}
