//! The command syntax of the deterministic shell (S5, ADR 0022).
//!
//! A typed line becomes the same plan document the model would emit, as text.
//! The caller hands that text to `plan::parse_checked`, so every structural and
//! semantic rule of the format applies to a typed command exactly as to the
//! model's output. This module never builds a plan value, and never expands
//! anything: no `~`, no globbing, no variables.

use std::collections::HashMap;
use std::fmt;

use serde_json::{Map, Value, json};

use crate::plan::{MAX_STEPS, VERSION};
use crate::reference::{Kind, Place};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandError(String);

impl CommandError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CommandError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// The text of `help`. Runs nothing.
    Help(String),
    /// A plan document, still to be checked by `plan::parse_checked`.
    Plan(String),
}

/// One argument of an action and how it is written. The flag is the argument's
/// name with `_` written as `-` (ADR 0022).
enum Arg {
    /// A reference in the primary position: `--in`, `--name`, `--kind`,
    /// `--new-name`, `--from-step`, of the forms the argument accepts.
    Reference(&'static str, &'static [Form]),
    /// The destination of a move, written with the same flags behind `--to-`.
    Destination(&'static str, &'static [Form]),
    Number(&'static str),
    Text(&'static str),
    /// A word the format checks, with the value to use when the flag is absent
    /// and the words `help` lists.
    Choice(&'static str, Option<&'static str>, &'static str),
    /// Text that is `null` when the flag is absent.
    OptionalText(&'static str),
    /// Every token after `--`.
    Argv(&'static str),
}

struct Command {
    group: &'static str,
    verb: &'static str,
    /// Whether the format lets the step ask for root (`--elevate`). A test
    /// compares this with the action's root rule, so it cannot drift.
    elevate: bool,
    args: &'static [Arg],
}

impl Command {
    fn action(&self) -> String {
        format!("{}.{}", self.group, self.verb)
    }
}

/// A form of reference, as the schema's argument definitions list them. The
/// format checks the same rule again; this copy lets the error and `help` say
/// which flags an action takes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Form {
    Place,
    Named,
    /// A name whose `--kind` must be written, for what writes, removes or moves
    /// an item: nothing here is left to a default, and the screen does not say
    /// whether a match is a file or a folder.
    NamedKind,
    New,
    Step,
}

impl Form {
    fn flags(self, prefix: &str) -> String {
        match self {
            Self::Place => format!("--{prefix}in PLACE"),
            // A destination is always a folder, so it has no kind to write.
            Self::Named if !prefix.is_empty() => format!("--{prefix}name NAME --{prefix}in PLACE"),
            Self::Named => {
                format!("--{prefix}name NAME --{prefix}in PLACE [--{prefix}kind file|folder|any]")
            }
            Self::NamedKind => {
                format!("--{prefix}name NAME --{prefix}in PLACE --{prefix}kind file|folder|any")
            }
            Self::New => format!("--{prefix}new-name NAME --{prefix}in PLACE"),
            Self::Step => format!("--{prefix}from-step sN"),
        }
    }
}

const READABLE: &[Form] = &[Form::Named, Form::Step];
const REMOVABLE: &[Form] = &[Form::NamedKind, Form::Step];
const TARGET: Arg = Arg::Reference("target", READABLE);

static COMMANDS: &[Command] = &[
    Command {
        group: "fs",
        verb: "list",
        elevate: true,
        args: &[Arg::Reference(
            "target",
            &[Form::Place, Form::Named, Form::Step],
        )],
    },
    Command {
        group: "fs",
        verb: "stat",
        elevate: true,
        args: &[TARGET],
    },
    Command {
        group: "fs",
        verb: "read",
        elevate: true,
        args: &[TARGET],
    },
    Command {
        group: "fs",
        verb: "find_largest",
        elevate: true,
        args: &[
            Arg::Reference("target", &[Form::Place, Form::Named]),
            Arg::Number("count"),
        ],
    },
    Command {
        group: "fs",
        verb: "write",
        elevate: true,
        args: &[
            Arg::Reference("target", &[Form::NamedKind, Form::New]),
            Arg::Text("content"),
            Arg::Choice("mode", Some("create"), "create|overwrite|append"),
        ],
    },
    Command {
        group: "fs",
        verb: "move",
        elevate: true,
        args: &[
            Arg::Reference("from", REMOVABLE),
            Arg::Destination("to", &[Form::Place, Form::Named, Form::New]),
        ],
    },
    Command {
        group: "fs",
        verb: "delete",
        elevate: true,
        args: &[Arg::Reference("target", REMOVABLE)],
    },
    Command {
        group: "process",
        verb: "run",
        elevate: true,
        args: &[Arg::Text("program"), Arg::Argv("argv")],
    },
    Command {
        group: "user",
        verb: "manage",
        elevate: false,
        args: &[
            Arg::Choice(
                "operation",
                None,
                "add|remove|add_to_group|remove_from_group",
            ),
            Arg::Text("user"),
            Arg::OptionalText("group"),
        ],
    },
    Command {
        group: "system",
        verb: "configure",
        elevate: false,
        args: &[Arg::Text("key"), Arg::Text("value")],
    },
    Command {
        group: "soul",
        verb: "propose_change",
        elevate: false,
        args: &[Arg::Text("rationale"), Arg::Text("new_text")],
    },
];

/// The action names the commands reach, for the test that compares them with the schema.
#[cfg(test)]
pub(crate) fn actions() -> Vec<String> {
    COMMANDS.iter().map(Command::action).collect()
}

/// The forms each reference argument accepts, for the test that compares them with the schema.
#[cfg(test)]
pub(crate) fn reference_forms(action: &str) -> Vec<(&'static str, Vec<&'static str>)> {
    let names = |forms: &[Form]| {
        forms
            .iter()
            .map(|form| match form {
                Form::Place => "place",
                Form::Named | Form::NamedKind => "named",
                Form::New => "new",
                Form::Step => "step",
            })
            .collect()
    };
    COMMANDS
        .iter()
        .find(|c| c.action() == action)
        .map(|c| {
            c.args
                .iter()
                .filter_map(|arg| match arg {
                    Arg::Reference(key, forms) | Arg::Destination(key, forms) => {
                        Some((*key, names(forms)))
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The words each enum flag takes, for the test that compares them with the schema.
#[cfg(test)]
pub(crate) fn choice_words(action: &str) -> Vec<(&'static str, Vec<&'static str>)> {
    COMMANDS
        .iter()
        .find(|c| c.action() == action)
        .map(|c| {
            c.args
                .iter()
                .filter_map(|arg| match arg {
                    Arg::Choice(key, _, words) => Some((*key, words.split('|').collect())),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The argument keys each command writes, for the test that compares them with the schema.
#[cfg(test)]
pub(crate) fn arg_keys(action: &str) -> Vec<&'static str> {
    COMMANDS
        .iter()
        .find(|c| c.action() == action)
        .map(|c| {
            c.args
                .iter()
                .map(|arg| match arg {
                    Arg::Reference(key, _)
                    | Arg::Destination(key, _)
                    | Arg::Number(key)
                    | Arg::Text(key)
                    | Arg::Choice(key, ..)
                    | Arg::OptionalText(key)
                    | Arg::Argv(key) => *key,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every flag some command accepts, for the test that checks what `help` prints.
#[cfg(test)]
pub(crate) fn known_flags() -> Vec<String> {
    COMMANDS.iter().flat_map(allowed_flags).collect()
}

/// The commands that take `--elevate`, for the test that compares them with the root rules.
#[cfg(test)]
pub(crate) fn takes_elevate(action: &str) -> Option<bool> {
    COMMANDS
        .iter()
        .find(|c| c.action() == action)
        .map(|c| c.elevate)
}

fn flag_name(key: &str) -> String {
    key.replace('_', "-")
}

/// Turn a typed line into `help` or a plan document.
pub fn parse(line: &str) -> Result<Parsed, CommandError> {
    let steps = split_steps(tokenize(line)?)?;
    if let [only] = steps.as_slice()
        && let [word] = only.as_slice()
        && !word.quoted
        && word.text == "help"
    {
        return Ok(Parsed::Help(help()));
    }
    if steps.len() > MAX_STEPS {
        return Err(CommandError::new(format!(
            "a line has at most {MAX_STEPS} steps"
        )));
    }
    let mut documents = Vec::new();
    let mut actions = Vec::new();
    for (index, words) in steps.iter().enumerate() {
        let (command, step) = parse_step(index + 1, words)?;
        actions.push(command.action());
        documents.push(step);
    }
    Ok(Parsed::Plan(
        json!({"kind": "plan", "version": VERSION, "summary": summary(&actions), "steps": documents})
            .to_string(),
    ))
}

/// The action names only, never what the person typed, so it cannot pass the
/// format's bound on a summary because of a long argument.
fn summary(actions: &[String]) -> String {
    const LIMIT: usize = 200;
    let joined = actions.join(", ");
    if joined.chars().count() <= LIMIT {
        joined
    } else {
        let cut: String = joined.chars().take(LIMIT - 3).collect();
        format!("{cut}...")
    }
}

struct Word {
    text: String,
    /// Whether any part was quoted: a quoted `--x` is a value, not a flag.
    quoted: bool,
}

enum Token {
    Word(Word),
    Separator,
}

fn tokenize(line: &str) -> Result<Vec<Token>, CommandError> {
    let mut tokens = Vec::new();
    let mut current: Option<Word> = None;
    let mut chars = line.chars();
    let flush = |current: &mut Option<Word>, tokens: &mut Vec<Token>| {
        if let Some(word) = current.take() {
            tokens.push(Token::Word(word));
        }
    };
    let push = |current: &mut Option<Word>, c: char, quoted: bool| {
        let word = current.get_or_insert(Word {
            text: String::new(),
            quoted: false,
        });
        word.text.push(c);
        word.quoted |= quoted;
    };
    let open = |current: &mut Option<Word>| {
        current
            .get_or_insert(Word {
                text: String::new(),
                quoted: false,
            })
            .quoted = true;
    };
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' | '\n' | '\r' => flush(&mut current, &mut tokens),
            ';' => {
                flush(&mut current, &mut tokens);
                tokens.push(Token::Separator);
            }
            '\\' => {
                let escaped = chars
                    .next()
                    .ok_or_else(|| CommandError::new("the line ends with a backslash"))?;
                // A backslash makes the character literal, so `\--x` is a value, not a flag.
                push(&mut current, escaped, true);
            }
            '\'' => {
                open(&mut current);
                loop {
                    match chars.next() {
                        None => return Err(CommandError::new("a ' is not closed")),
                        Some('\'') => break,
                        Some(c) => push(&mut current, c, true),
                    }
                }
            }
            '"' => {
                open(&mut current);
                loop {
                    let c = match chars.next() {
                        None => return Err(CommandError::new("a \" is not closed")),
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some('\\') => '\\',
                            Some('"') => '"',
                            Some('n') => '\n',
                            Some('t') => '\t',
                            _ => {
                                return Err(CommandError::new(
                                    "inside \" only \\\\, \\\", \\n and \\t may follow a backslash",
                                ));
                            }
                        },
                        Some(c) => c,
                    };
                    push(&mut current, c, true);
                }
            }
            c => push(&mut current, c, false),
        }
    }
    flush(&mut current, &mut tokens);
    Ok(tokens)
}

fn split_steps(tokens: Vec<Token>) -> Result<Vec<Vec<Word>>, CommandError> {
    let mut steps: Vec<Vec<Word>> = vec![Vec::new()];
    for token in tokens {
        match token {
            Token::Word(word) => steps.last_mut().expect("never empty").push(word),
            Token::Separator => steps.push(Vec::new()),
        }
    }
    if steps.iter().any(Vec::is_empty) {
        return Err(CommandError::new(if steps.len() == 1 {
            "type a command, or help".to_owned()
        } else {
            "a step is empty".to_owned()
        }));
    }
    Ok(steps)
}

/// The value of the flag, taken once.
struct Flags(HashMap<String, String>);

impl Flags {
    fn take(&mut self, name: &str) -> Option<String> {
        self.0.remove(name)
    }
}

fn is_flag(word: &Word) -> bool {
    !word.quoted && word.text.starts_with("--")
}

fn allowed_flags(command: &Command) -> Vec<String> {
    let mut allowed = vec!["after".to_owned()];
    if command.elevate {
        allowed.push("elevate".to_owned());
    }
    for arg in command.args {
        match arg {
            Arg::Reference(..) => {
                allowed.extend(["in", "name", "kind", "new-name", "from-step"].map(str::to_owned));
            }
            Arg::Destination(..) => {
                allowed.extend(["to-in", "to-name", "to-new-name"].map(str::to_owned));
            }
            Arg::Number(key) | Arg::Text(key) | Arg::Choice(key, ..) | Arg::OptionalText(key) => {
                allowed.push(flag_name(key));
            }
            Arg::Argv(_) => {}
        }
    }
    allowed
}

fn parse_step(number: usize, words: &[Word]) -> Result<(&'static Command, Value), CommandError> {
    let [group, verb, rest @ ..] = words else {
        return Err(CommandError::new(
            "a step is a group and an action, for example `fs list --in downloads`; try help",
        ));
    };
    if group.quoted || verb.quoted {
        return Err(CommandError::new(
            "the group and the action are written plain, without quotes or backslashes",
        ));
    }
    let command = COMMANDS
        .iter()
        .find(|c| c.group == group.text && c.verb == verb.text)
        .ok_or_else(|| {
            let hint = if group.text.contains('.') {
                "; write `fs list`, not `fs.list`"
            } else {
                ""
            };
            CommandError::new(format!(
                "unknown command {} {}{hint}; try help",
                group.text, verb.text
            ))
        })?;
    let name = format!("{} {}", command.group, command.verb);
    let allowed = allowed_flags(command);
    let mut flags = Flags(HashMap::new());
    let mut after: Vec<String> = Vec::new();
    let mut elevate = false;
    let mut argv: Vec<String> = Vec::new();
    let mut tokens = rest.iter();
    while let Some(word) = tokens.next() {
        if !is_flag(word) {
            return Err(CommandError::new(format!(
                "{name}: unexpected {:?}; flags start with --",
                word.text
            )));
        }
        let flag = &word.text[2..];
        if flag.is_empty() {
            if !command.args.iter().any(|a| matches!(a, Arg::Argv(_))) {
                return Err(CommandError::new(format!(
                    "{name} takes no arguments after --"
                )));
            }
            argv = tokens.by_ref().map(|w| w.text.clone()).collect();
            break;
        }
        if !allowed.iter().any(|a| a == flag) {
            return Err(CommandError::new(format!(
                "{name}: unknown flag --{flag}; try help"
            )));
        }
        if flag == "elevate" {
            if elevate {
                return Err(CommandError::new(format!("{name}: --elevate given twice")));
            }
            elevate = true;
            continue;
        }
        let value = match tokens.next() {
            Some(value) if !is_flag(value) => value.text.clone(),
            _ => return Err(CommandError::new(format!("{name}: --{flag} needs a value"))),
        };
        if flag == "after" {
            if !after.contains(&value) {
                after.push(value);
            }
        } else if flags.0.insert(flag.to_owned(), value).is_some() {
            return Err(CommandError::new(format!("{name}: --{flag} given twice")));
        }
    }
    let mut depends_on = after;
    let mut args = Map::new();
    for arg in command.args {
        let (key, value) = build_arg(&name, arg, &mut flags, &mut depends_on, &argv)?;
        args.insert(key.to_owned(), value);
    }
    let mut step = json!({
        "id": format!("s{number}"),
        "action": command.action(),
        "args": args,
        "depends_on": depends_on,
    });
    if command.elevate {
        step["elevate"] = Value::Bool(elevate);
    }
    Ok((command, step))
}

fn build_arg(
    name: &str,
    arg: &Arg,
    flags: &mut Flags,
    depends_on: &mut Vec<String>,
    argv: &[String],
) -> Result<(&'static str, Value), CommandError> {
    let required = |flags: &mut Flags, key: &str| {
        let flag = flag_name(key);
        flags
            .take(&flag)
            .ok_or_else(|| CommandError::new(format!("{name} needs --{flag}")))
    };
    Ok(match arg {
        Arg::Reference(key, forms) => (*key, reference(name, flags, "", forms, depends_on)?),
        Arg::Destination(key, forms) => (*key, reference(name, flags, "to-", forms, depends_on)?),
        Arg::Text(key) => (*key, Value::String(required(flags, key)?)),
        Arg::Number(key) => {
            let text = required(flags, key)?;
            // Digits only: no sign, no space, nothing a number would not have.
            let digits = text.chars().all(|c| c.is_ascii_digit());
            let number: u32 = text.parse().ok().filter(|_| digits).ok_or_else(|| {
                CommandError::new(format!(
                    "{name}: --{} must be a whole number from 0 to 4294967295",
                    flag_name(key)
                ))
            })?;
            (*key, json!(number))
        }
        Arg::Choice(key, default, words) => {
            let value = match (flags.take(&flag_name(key)), default) {
                (Some(value), _) if !words.split('|').any(|word| word == value) => {
                    return Err(CommandError::new(format!(
                        "{name}: --{} {value:?} is not one of {}",
                        flag_name(key),
                        words.replace('|', ", ")
                    )));
                }
                (Some(value), _) => value,
                (None, Some(default)) => (*default).to_owned(),
                (None, None) => {
                    return Err(CommandError::new(format!(
                        "{name} needs --{}",
                        flag_name(key)
                    )));
                }
            };
            (*key, Value::String(value))
        }
        Arg::OptionalText(key) => (
            *key,
            flags
                .take(&flag_name(key))
                .map_or(Value::Null, Value::String),
        ),
        Arg::Argv(key) => (*key, json!(argv)),
    })
}

/// The flags of one reference, with `prefix` in front of each. Which forms an
/// argument accepts is the format's rule, not this function's.
fn reference(
    name: &str,
    flags: &mut Flags,
    prefix: &str,
    forms: &[Form],
    depends_on: &mut Vec<String>,
) -> Result<Value, CommandError> {
    let kind_required = forms.contains(&Form::NamedKind);
    let value = reference_value(name, flags, prefix, forms, kind_required, depends_on)?;
    let form = match (
        value.get("place"),
        value.get("from_step"),
        value.get("new_name"),
    ) {
        (Some(_), ..) => Form::Place,
        (_, Some(_), _) => Form::Step,
        (.., Some(_)) => Form::New,
        _ => Form::Named,
    };
    if forms.contains(&form) || (form == Form::Named && forms.contains(&Form::NamedKind)) {
        return Ok(value);
    }
    let takes: Vec<String> = forms.iter().map(|f| f.flags(prefix)).collect();
    Err(CommandError::new(format!(
        "{name}: this reference is not accepted here; use {}",
        takes.join(" or ")
    )))
}

fn reference_value(
    name: &str,
    flags: &mut Flags,
    prefix: &str,
    forms: &[Form],
    kind_required: bool,
    depends_on: &mut Vec<String>,
) -> Result<Value, CommandError> {
    let destination = !prefix.is_empty();
    let mut take = |flag: &str| flags.take(&format!("{prefix}{flag}"));
    let place = take("in");
    let item = take("name");
    let new_item = take("new-name");
    let kind = take("kind");
    let from_step = take("from-step");
    let flag = |f: &str| format!("--{prefix}{f}");
    if let Some(place) = &place {
        check_word::<Place>(name, &flag("in"), place, &place_words())?;
    }
    if let Some(kind) = &kind {
        check_word::<Kind>(name, &flag("kind"), kind, "file, folder, any")?;
    }
    let needs_place =
        |what: &str| CommandError::new(format!("{name}: {what} needs {}", flag("in")));
    match (from_step, new_item, item, place, kind) {
        (Some(step), None, None, None, None) => {
            if !depends_on.contains(&step) {
                depends_on.push(step.clone());
            }
            Ok(json!({"from_step": step}))
        }
        (Some(_), ..) => Err(CommandError::new(format!(
            "{name}: --from-step stands alone"
        ))),
        (None, Some(_), Some(_), ..) => Err(CommandError::new(format!(
            "{name}: {} and {} are two different references",
            flag("name"),
            flag("new-name")
        ))),
        (None, Some(new_name), None, place, None) => {
            let place = place.ok_or_else(|| needs_place(&flag("new-name")))?;
            Ok(json!({"new_name": new_name, "in": place}))
        }
        (None, Some(_), None, _, Some(_)) => Err(CommandError::new(format!(
            "{name}: {} goes with {}, not {}",
            flag("kind"),
            flag("name"),
            flag("new-name")
        ))),
        (None, None, Some(item), place, kind) => {
            let place = place.ok_or_else(|| needs_place(&flag("name")))?;
            // A destination can only be a folder, so there is nothing to choose.
            let kind = match (destination, kind) {
                (true, _) => "folder".to_owned(),
                (false, Some(kind)) => kind,
                (false, None) if kind_required => {
                    return Err(CommandError::new(format!(
                        "{name}: {} needs {}, because it writes, removes or moves what it names",
                        flag("name"),
                        flag("kind")
                    )));
                }
                (false, None) => "any".to_owned(),
            };
            Ok(json!({"name": item, "in": place, "kind": kind}))
        }
        (None, None, None, Some(place), None) => Ok(json!({"place": place})),
        (None, None, None, None, None) => Err(CommandError::new(format!(
            "{name} needs a reference: {}",
            forms
                .iter()
                .map(|f| f.flags(prefix))
                .collect::<Vec<_>>()
                .join(" or ")
        ))),
        (None, None, None, _, Some(_)) => Err(CommandError::new(format!(
            "{name}: {} goes with {}",
            flag("kind"),
            flag("name")
        ))),
    }
}

/// A word the format reads as an enum, checked here only to name the flag in
/// the error: serde's own message for a reference names neither.
fn check_word<T: serde::de::DeserializeOwned>(
    name: &str,
    flag: &str,
    value: &str,
    expected: &str,
) -> Result<(), CommandError> {
    serde_json::from_value::<T>(Value::String(value.to_owned()))
        .map(drop)
        .map_err(|_| {
            CommandError::new(format!("{name}: {flag} {value:?} is not one of {expected}"))
        })
}

fn place_words() -> String {
    Place::ALL
        .map(|place| format!("{place:?}").to_lowercase())
        .join(", ")
}

/// What `help` prints, built from the same table the parser reads.
fn help() -> String {
    let mut out = String::from("Commands (one line, steps separated by `;`):\n");
    for command in COMMANDS {
        let mut line = format!("  {} {}", command.group, command.verb);
        let mut last = String::new();
        for arg in command.args {
            let text = match arg {
                Arg::Reference(_, forms) => forms_help(forms, ""),
                Arg::Destination(_, forms) => forms_help(forms, "to-"),
                Arg::Number(key) => format!("--{} N", flag_name(key)),
                Arg::Text(key) => format!("--{} TEXT", flag_name(key)),
                Arg::Choice(key, Some(default), words) => {
                    format!("[--{} {words}, default {default}]", flag_name(key))
                }
                Arg::Choice(key, None, words) => format!("--{} {words}", flag_name(key)),
                Arg::OptionalText(key) => format!("[--{} TEXT]", flag_name(key)),
                Arg::Argv(_) => "[-- ARGUMENT...]".to_owned(),
            };
            // Everything after `--` is an argument, so it comes last.
            if matches!(arg, Arg::Argv(_)) {
                last = text;
            } else {
                line.push(' ');
                line.push_str(&text);
            }
        }
        if command.elevate {
            line.push_str(" [--elevate]");
        }
        line.push_str(" [--after sN]");
        if !last.is_empty() {
            line.push(' ');
            line.push_str(&last);
        }
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str(&format!(
        "A kind left out means any, and only where the line above shows it in brackets; a destination name is always a folder.\n\
         --from-step sN means what step sN found; flags after `--` belong to the program.\n\
         Places: {places}.\n\
         Steps are s1, s2, ... by position; --after sN makes a step wait for sN.\n\
         Quote with '...' or \"...\"; nothing is expanded. A line holds up to {MAX_STEPS} steps.\n",
        places = place_words()
    ));
    out
}

fn forms_help(forms: &[Form], prefix: &str) -> String {
    let all: Vec<String> = forms.iter().map(|f| f.flags(prefix)).collect();
    format!("({})", all.join(" | "))
}
