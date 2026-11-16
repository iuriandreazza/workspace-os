//! Parsing and checking what the model emitted (ADR 0011, ADR 0018).
//!
//! `parse` is the structural half, equivalent to `docs/schemas/action-plan.schema.json`;
//! `check_rules` is the semantic half, equivalent to `docs/schemas/validate.py`.
//! The tests run the Rust half over every example next to the schema, and
//! `validate.py` runs the schema over the same examples (by hand: the gate does
//! not run it, ADR 0011), so the two readings are compared one example at a time.

use std::collections::{HashMap, HashSet};
use std::fmt;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use crate::action::{Action, RootRule, UserOperation, WriteMode};
use crate::reference::{Direct, Kind, Place, Spec, Target, Within, is_path_text, is_single_name};

pub const MAX_STEPS: usize = 16;
/// Version 2 replaced paths with references (ADR 0014). Version 3 adds paths as
/// one more reference, `under_home` in place of `everywhere`, and `session.cd`
/// (ADR 0018); a version 2 document is refused like version 1 was.
pub const VERSION: u32 = 3;
const MAX_DEPENDENCIES: usize = 15;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StepId(String);

impl StepId {
    pub(crate) fn parse(text: &str) -> Result<Self, PlanError> {
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
    /// What the model asked for. Its references are resolved before the broker
    /// judges anything (ADR 0014).
    pub spec: Spec,
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
    if version == VERSION {
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
        let path_is_array = ["target", "from", "to"].iter().any(|key| {
            let reference = args.and_then(|a| a.get(key));
            reference.is_some_and(Value::is_array)
                || reference
                    .and_then(|r| r.get("in"))
                    .is_some_and(Value::is_array)
        });
        // `in` is a place (a string) or a path (an object with `path`); serde
        // would also read `{"home": null}` as a place.
        let within_is_not_a_path = ["target", "from", "to"].iter().any(|key| {
            args.and_then(|a| a.get(key))
                .and_then(|r| r.get("in"))
                .is_some_and(|v| v.is_object() && v.get("path").is_none())
        });
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
        // Place and kind are unit enums inside a reference (`in` is a place or a path).
        let reference_enum_is_not_string = ["target", "from", "to"].iter().any(|key| {
            ["place", "kind"].iter().any(|field| {
                args.and_then(|a| a.get(key))
                    .and_then(|r| r.get(field))
                    .is_some_and(|v| !v.is_string())
            })
        });
        if enum_is_not_string || reference_enum_is_not_string || within_is_not_a_path {
            return Err(PlanError::new(
                "mode, operation, place and kind must be strings",
            ));
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
enum RawTarget {
    Place(RawPlace),
    Path(RawPath),
    UnderHome(RawUnderHome),
    Named(RawNamed),
    New(RawNew),
    FromStep(RawFromStep),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPath {
    path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawUnderHome {
    under_home: bool,
    name: String,
    kind: Kind,
}

/// A place is a string and a path is an object.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawWithin {
    Place(Place),
    Path(RawPath),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPlace {
    place: Place,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawNamed {
    name: String,
    #[serde(rename = "in")]
    within: RawWithin,
    kind: Kind,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawNew {
    new_name: String,
    #[serde(rename = "in")]
    within: RawWithin,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFromStep {
    from_step: String,
}

/// The forms of reference an argument accepts. The schema says the same.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Form {
    Place,
    Path,
    UnderHome,
    Named,
    New,
    FromStep,
}

impl RawTarget {
    fn form(&self) -> Form {
        match self {
            Self::Place(_) => Form::Place,
            Self::Path(_) => Form::Path,
            Self::UnderHome(_) => Form::UnderHome,
            Self::Named(_) => Form::Named,
            Self::New(_) => Form::New,
            Self::FromStep(_) => Form::FromStep,
        }
    }

    fn convert(self, field: &str, allowed: &[Form]) -> Result<Target, PlanError> {
        if !allowed.contains(&self.form()) {
            return Err(PlanError::new(format!(
                "{field} does not accept this kind of reference"
            )));
        }
        Ok(match self {
            Self::Place(p) => Target::Direct(Direct::Place(p.place)),
            Self::Path(p) => Target::Direct(Direct::Path(path_text(field, p.path)?)),
            Self::UnderHome(n) => {
                if !n.under_home {
                    return Err(PlanError::new("under_home must be true"));
                }
                single_name(field, &n.name)?;
                Target::Direct(Direct::UnderHome {
                    name: n.name,
                    kind: n.kind,
                })
            }
            Self::Named(n) => {
                single_name(field, &n.name)?;
                Target::Direct(Direct::Named {
                    name: n.name,
                    within: n.within.convert(field)?,
                    kind: n.kind,
                })
            }
            Self::New(n) => {
                single_name(field, &n.new_name)?;
                Target::Direct(Direct::New {
                    name: n.new_name,
                    within: n.within.convert(field)?,
                })
            }
            Self::FromStep(r) => Target::FromStep(StepId::parse(&r.from_step)?),
        })
    }
}

impl RawWithin {
    fn convert(self, field: &str) -> Result<Within, PlanError> {
        Ok(match self {
            Self::Place(place) => Within::Place(place),
            Self::Path(p) => Within::Path(path_text(field, p.path)?),
        })
    }
}

/// The structural half of "a path is bounded": the schema bounds it in
/// characters. That it fits in PATH_MAX bytes and holds no NUL is a rule
/// (`check_rules`): the grammar the model is constrained by cannot carry a
/// `pattern` for it (llama-server fails on one), as for names. Where a path
/// leads, and whether a `..` may leave the root, is for the resolver, which
/// knows the directory.
fn path_text(field: &str, path: String) -> Result<String, PlanError> {
    bounded(field, &path, 1, crate::reference::MAX_PATH_BYTES)?;
    Ok(path)
}

/// The structural half of "a name is one entry": the schema bounds the length
/// in characters. What it may contain, and that it fits in NAME_MAX bytes, is a rule (`check_rules`), because the grammar the model
/// is constrained by cannot be given a pattern for it (ADR 0014).
fn single_name(field: &str, name: &str) -> Result<(), PlanError> {
    bounded(field, name, 1, 255)
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
struct TargetArgs {
    target: RawTarget,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FindLargestArgs {
    target: RawTarget,
    count: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteArgs {
    target: RawTarget,
    content: String,
    mode: WriteMode,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MoveArgs {
    from: RawTarget,
    to: RawTarget,
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
    let spec = convert_spec(&raw.action, raw.args)?;
    let elevate = match (spec.root_rule(), raw.elevate) {
        (RootRule::OnRequest, Some(requested)) => requested,
        (RootRule::OnRequest, None) => {
            return Err(PlanError::new(format!("{id}: elevate is required")));
        }
        (_, Some(_)) => {
            return Err(PlanError::new(format!(
                "{id}: {} does not take elevate",
                spec.name()
            )));
        }
        (_, None) => false,
    };
    Ok(Step {
        id,
        depends_on,
        elevate,
        spec,
    })
}

fn convert_spec(name: &str, args: Value) -> Result<Spec, PlanError> {
    use Form::{FromStep, Named, New, Path, Place, UnderHome};
    let target = |args: Value, allowed: &[Form], build: fn(Target) -> Spec| {
        Ok(build(
            from_value::<TargetArgs>(args)?
                .target
                .convert("target", allowed)?,
        ))
    };
    let plain = |action| Ok(Spec::Plain(action));
    match name {
        "fs.list" => target(args, &[Place, Path, Named, UnderHome, FromStep], |target| {
            Spec::List { target }
        }),
        "fs.stat" => target(args, &[Path, Named, UnderHome, FromStep], |target| {
            Spec::Stat { target }
        }),
        "fs.read" => target(args, &[Path, Named, UnderHome, FromStep], |target| {
            Spec::Read { target }
        }),
        "fs.delete" => target(args, &[Path, Named, UnderHome, FromStep], |target| {
            Spec::Delete { target }
        }),
        "session.cd" => {
            let target = from_value::<TargetArgs>(args)?
                .target
                .convert("target", &[Place, Path, Named, UnderHome])?;
            only_folders("target", &target)?;
            Ok(Spec::Cd { target })
        }
        "fs.find_largest" => {
            let a: FindLargestArgs = from_value(args)?;
            if !(1..=100).contains(&a.count) {
                return Err(PlanError::new("count must be 1 to 100"));
            }
            Ok(Spec::FindLargest {
                target: a
                    .target
                    .convert("target", &[Place, Path, Named, UnderHome])?,
                count: a.count,
            })
        }
        "fs.write" => {
            let a: WriteArgs = from_value(args)?;
            bounded("content", &a.content, 0, 65536)?;
            Ok(Spec::Write {
                target: a.target.convert("target", &[Path, Named, UnderHome, New])?,
                content: a.content,
                mode: a.mode,
            })
        }
        "fs.move" => {
            let a: MoveArgs = from_value(args)?;
            let to = a.to.convert("to", &[Place, Path, Named, UnderHome, New])?;
            only_folders("to", &to)?;
            Ok(Spec::Move {
                from: a
                    .from
                    .convert("from", &[Path, Named, UnderHome, FromStep])?,
                to,
            })
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
            plain(Action::Run {
                program: a.program,
                argv: a.argv,
            })
        }
        "user.manage" => {
            let a: UserArgs = from_value(args)?;
            account_name("user", &a.user)?;
            if let Some(group) = &a.group {
                account_name("group", group)?;
            }
            plain(Action::UserManage {
                operation: a.operation,
                user: a.user,
                group: a.group,
            })
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
            plain(Action::SystemConfigure {
                key: a.key,
                value: a.value,
            })
        }
        "soul.propose_change" => {
            let a: SoulArgs = from_value(args)?;
            bounded("rationale", &a.rationale, 1, 500)?;
            bounded("new_text", &a.new_text, 1, 16384)?;
            plain(Action::SoulProposeChange {
                rationale: a.rationale,
                new_text: a.new_text,
            })
        }
        other => Err(PlanError::new(format!("unknown action {other:?}"))),
    }
}

/// A name that has to be found and then used as a directory must say it is a folder.
fn only_folders(field: &str, target: &Target) -> Result<(), PlanError> {
    if let Target::Direct(Direct::Named { kind, .. } | Direct::UnderHome { kind, .. }) = target
        && *kind != Kind::Folder
    {
        return Err(PlanError::new(format!(
            "{field}: a named directory must be a folder"
        )));
    }
    Ok(())
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
        if let Some(source) = step.spec.source_step() {
            if !step.depends_on.contains(source) {
                return Err(PlanError::new(format!(
                    "{} uses {source} without depending on it",
                    step.id
                )));
            }
            if !by_id[source].spec.produces_paths() {
                return Err(PlanError::new(format!("{source} does not produce paths")));
            }
        }
        for (_, target) in step.spec.targets() {
            if let Target::Direct(
                Direct::Path(text)
                | Direct::Named {
                    within: Within::Path(text),
                    ..
                }
                | Direct::New {
                    within: Within::Path(text),
                    ..
                },
            ) = target
                && !is_path_text(text)
            {
                return Err(PlanError::new(format!(
                    "{}: a path has 1 to {} bytes and no NUL",
                    step.id,
                    crate::reference::MAX_PATH_BYTES
                )));
            }
            if let Target::Direct(
                Direct::Named { name, .. }
                | Direct::New { name, .. }
                | Direct::UnderHome { name, .. },
            ) = target
                && !is_single_name(name)
            {
                return Err(PlanError::new(format!(
                    "{}: {name:?} is not a single file name",
                    step.id
                )));
            }
        }
        if let Spec::Plain(Action::UserManage {
            operation, group, ..
        }) = &step.spec
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
    fn a_plain_string_is_not_a_reference() {
        let text = r#"{"kind":"plan","version":3,"summary":"x","steps":[
            {"id":"s1","action":"fs.delete","args":{"target":"s1.path"},"depends_on":[],"elevate":false}]}"#;
        assert!(parse(text).is_err());
        let old = r#"{"kind":"plan","version":3,"summary":"x","steps":[
            {"id":"s1","action":"fs.delete","args":{"path":"/etc/passwd"},"depends_on":[],"elevate":false}]}"#;
        assert!(parse(old).is_err(), "a path field no longer exists");
    }

    #[test]
    fn version_one_documents_are_refused() {
        let text = r#"{"kind":"plan","version":1,"summary":"x","steps":[
            {"id":"s1","action":"fs.list","args":{"target":{"place":"home"}},"depends_on":[],"elevate":false}]}"#;
        assert!(parse(text).is_err());
    }

    #[test]
    fn each_argument_accepts_only_its_forms_of_reference() {
        let one = |action: &str, args: &str| {
            format!(
                r#"{{"kind":"plan","version":3,"summary":"x","steps":[{{"id":"s1","action":"{action}","args":{args},"depends_on":[],"elevate":false}}]}}"#
            )
        };
        let place = r#"{"place":"downloads"}"#;
        let named = r#"{"name":"a","in":"home","kind":"file"}"#;
        let new = r#"{"new_name":"a","in":"home"}"#;
        let folder = r#"{"name":"a","in":"home","kind":"folder"}"#;
        let arg = |t: &str| format!(r#"{{"target":{t}}}"#);
        // a place may be listed or searched, never read, removed or overwritten
        assert!(parse(&one("fs.list", &arg(place))).is_ok());
        assert!(parse(&one("fs.read", &arg(place))).is_err());
        assert!(parse(&one("fs.delete", &arg(place))).is_err());
        assert!(parse(&one("fs.delete", &arg(new))).is_err());
        assert!(parse(&one("fs.delete", &arg(named))).is_ok());
        let write = |t: &str| format!(r#"{{"target":{t},"content":"c","mode":"create"}}"#);
        assert!(parse(&one("fs.write", &write(place))).is_err());
        assert!(parse(&one("fs.write", &write(new))).is_ok());
        let mv = |to: &str| format!(r#"{{"from":{named},"to":{to}}}"#);
        assert!(parse(&one("fs.move", &mv(place))).is_ok());
        assert!(parse(&one("fs.move", &mv(new))).is_ok());
        assert!(parse(&one("fs.move", &mv(folder))).is_ok());
        assert!(
            parse(&one("fs.move", &mv(named))).is_err(),
            "a file is not a destination folder"
        );
        let from_place = format!(r#"{{"from":{place},"to":{place}}}"#);
        assert!(parse(&one("fs.move", &from_place)).is_err());
    }

    #[test]
    fn a_name_that_says_where_to_look_is_refused() {
        let one = |name: &str| {
            format!(
                r#"{{"kind":"plan","version":3,"summary":"x","steps":[{{"id":"s1","action":"fs.read","args":{{"target":{{"name":{name:?},"in":"home","kind":"file"}}}},"depends_on":[],"elevate":false}}]}}"#
            )
        };
        assert!(parse_checked(&one("report.pdf")).is_ok());
        for bad in [
            "",
            ".",
            "..",
            "a/b",
            "/etc/passwd",
            "../x",
            &"x".repeat(256),
        ] {
            assert!(parse_checked(&one(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_null_elevate_is_not_the_same_as_no_elevate() {
        let text = r#"{"kind":"plan","version":3,"summary":"x","steps":[
            {"id":"s1","action":"user.manage","args":{"operation":"add","user":"bob","group":null},"depends_on":[],"elevate":null}]}"#;
        assert!(parse(text).is_err());
    }

    #[test]
    fn structs_given_as_arrays_are_refused_like_the_schema_does() {
        for text in [
            r#"{"kind":"plan","version":3,"summary":"x","steps":[["s1","fs.list",[{"name":"x"}],[],false]]}"#,
            r#"{"kind":"plan","version":3,"summary":"x","steps":[{"id":"s1","action":"fs.write","args":["x","c","create"],"depends_on":[],"elevate":false}]}"#,
            r#"{"kind":"plan","version":3,"summary":"x","steps":[
                {"id":"s1","action":"fs.list","args":{"target":{"name":"x","in":"home","kind":"any"}},"depends_on":[],"elevate":false},
                {"id":"s2","action":"fs.stat","args":{"target":["s1"]},"depends_on":["s1"],"elevate":false}]}"#,
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn an_enum_given_as_an_object_is_refused_like_the_schema_does() {
        let text = r#"{"kind":"plan","version":3,"summary":"x","steps":[
            {"id":"s1","action":"fs.write","args":{"target":{"new_name":"a","in":"home"},"content":"c","mode":{"create":null}},"depends_on":[],"elevate":false}]}"#;
        assert!(parse(text).is_err());
    }

    #[test]
    fn a_reference_enum_given_as_an_object_is_refused_like_the_schema_does() {
        for target in [
            r#"{"place":{"downloads":null}}"#,
            r#"{"name":"a","in":{"home":null},"kind":"file"}"#,
            r#"{"name":"a","in":"home","kind":{"file":null}}"#,
        ] {
            let text = format!(
                r#"{{"kind":"plan","version":3,"summary":"x","steps":[{{"id":"s1","action":"fs.list","args":{{"target":{target}}},"depends_on":[],"elevate":false}}]}}"#
            );
            assert!(parse(&text).is_err(), "{target}");
        }
    }

    #[test]
    fn the_limits_of_the_format_are_enforced() {
        let step = |id: String, deps: &str| {
            format!(
                r#"{{"id":"{id}","action":"fs.stat","args":{{"target":{{"name":"x","in":"home","kind":"any"}}}},"depends_on":{deps},"elevate":false}}"#
            )
        };
        let plan = |summary: &str, steps: &[String]| {
            format!(
                r#"{{"kind":"plan","version":3,"summary":"{summary}","steps":[{}]}}"#,
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
                r#"{{"kind":"plan","version":3,"summary":"x","steps":[{{"id":"{id}","action":"fs.stat","args":{{"target":{{"name":"x","in":"home","kind":"any"}}}},"depends_on":[],"elevate":false}}]}}"#
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
                r#"{{"kind":"plan","version":3,"summary":"x","steps":[{{"id":"s1","action":"process.run","args":{{"program":"ls","argv":["{arg}"]}},"depends_on":[],"elevate":false}}]}}"#
            )
        };
        assert!(parse(&run(&"a".repeat(1024))).is_ok());
        assert!(parse(&run(&"a".repeat(1025))).is_err());
        let many = |n: usize| {
            let items = vec![r#""a""#; n].join(",");
            format!(
                r#"{{"kind":"plan","version":3,"summary":"x","steps":[{{"id":"s1","action":"process.run","args":{{"program":"ls","argv":[{items}]}},"depends_on":[],"elevate":false}}]}}"#
            )
        };
        assert!(parse(&many(32)).is_ok());
        assert!(parse(&many(33)).is_err());
    }

    #[test]
    fn count_content_and_name_bounds_match_the_schema() {
        let one = |action: &str, args: String| {
            format!(
                r#"{{"kind":"plan","version":3,"summary":"x","steps":[{{"id":"s1","action":"{action}","args":{args},"depends_on":[],"elevate":false}}]}}"#
            )
        };
        let find = |count: u32| {
            one(
                "fs.find_largest",
                format!(r#"{{"target":{{"place":"home"}},"count":{count}}}"#),
            )
        };
        assert!(parse(&find(1)).is_ok() && parse(&find(100)).is_ok());
        assert!(parse(&find(0)).is_err() && parse(&find(101)).is_err());
        let write = |n: usize| {
            one(
                "fs.write",
                format!(
                    r#"{{"target":{{"new_name":"a","in":"home"}},"content":"{}","mode":"create"}}"#,
                    "c".repeat(n)
                ),
            )
        };
        assert!(parse(&write(65536)).is_ok() && parse(&write(65537)).is_err());
        let stat = |n: usize| {
            one(
                "fs.stat",
                format!(
                    r#"{{"target":{{"name":"{}","in":"home","kind":"any"}}}}"#,
                    "p".repeat(n)
                ),
            )
        };
        assert!(parse(&stat(255)).is_ok() && parse(&stat(256)).is_err());
        assert!(parse(&stat(0)).is_err());
    }

    #[test]
    fn a_proposed_soul_text_is_bounded() {
        let soul = |n: usize| {
            format!(
                r#"{{"kind":"plan","version":3,"summary":"x","steps":[{{"id":"s1","action":"soul.propose_change","args":{{"rationale":"r","new_text":"{}"}},"depends_on":[]}}]}}"#,
                "t".repeat(n)
            )
        };
        assert!(parse(&soul(16384)).is_ok());
        assert!(parse(&soul(16385)).is_err());
        assert!(parse(&soul(0)).is_err());
    }

    #[test]
    fn elevate_is_required_where_it_is_allowed() {
        let text = r#"{"kind":"plan","version":3,"summary":"x","steps":[
            {"id":"s1","action":"fs.stat","args":{"target":{"name":"x","in":"home","kind":"any"}},"depends_on":[]}]}"#;
        assert!(parse(text).is_err());
    }

    #[test]
    fn text_around_the_json_is_refused() {
        assert!(parse("Sure! {\"kind\":\"clarify\",\"version\":1,\"question\":\"q\"}").is_err());
        assert!(parse(r#"{"kind":"clarify","version":3,"question":"q"} extra"#).is_err());
    }

    #[test]
    fn a_step_with_no_dependency_order_is_executed_after_its_dependencies() {
        let text = r#"{"kind":"plan","version":3,"summary":"x","steps":[
            {"id":"s2","action":"fs.stat","args":{"target":{"from_step":"s1"}},"depends_on":["s1"],"elevate":false},
            {"id":"s1","action":"fs.list","args":{"target":{"name":"x","in":"home","kind":"any"}},"depends_on":[],"elevate":false}]}"#;
        let Document::Plan(plan) = parse_checked(text).unwrap() else {
            panic!("not a plan")
        };
        assert_eq!(plan.execution_order(), vec![1, 0]);
    }
}
