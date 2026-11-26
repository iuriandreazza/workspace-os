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
    /// A folder the session enters: the flags of a reference, with a name
    /// always a folder.
    Directory(&'static str, &'static [Form]),
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
    /// A path as typed (`--path`): the resolver expands `~` and cleans `.` and `..`.
    Path,
    Named,
    /// A name whose `--kind` must be written, for what writes, removes or moves
    /// an item: nothing here is left to a default, and the screen does not say
    /// whether a match is a file or a folder.
    NamedKind,
    /// A search below the home (`--under-home`), with the same kind rule as `Named`.
    UnderHome,
    UnderHomeKind,
    New,
    Step,
}

const WITHIN: &str = "(--{p}in PLACE | --{p}in-path PATH)";

impl Form {
    /// The flags of this form as `help` and the errors write them. A folder
    /// reference (a destination or the target of `cd`) has no kind to write.
    fn flags(self, prefix: &str, folder: bool) -> String {
        let within = WITHIN.replace("{p}", prefix);
        let kind = |required: bool| match (folder, required) {
            (true, _) => String::new(),
            (false, true) => format!(" --{prefix}kind file|folder|any"),
            (false, false) => format!(" [--{prefix}kind file|folder|any]"),
        };
        match self {
            Self::Place => format!("--{prefix}in PLACE"),
            Self::Path => format!("--{prefix}path PATH"),
            Self::Named => format!("--{prefix}name NAME {within}{}", kind(false)),
            Self::NamedKind => format!("--{prefix}name NAME {within}{}", kind(true)),
            Self::UnderHome => format!("--{prefix}name NAME --{prefix}under-home{}", kind(false)),
            Self::UnderHomeKind => {
                format!("--{prefix}name NAME --{prefix}under-home{}", kind(true))
            }
            Self::New => format!("--{prefix}new-name NAME {within}"),
            Self::Step => format!("--{prefix}from-step sN"),
        }
    }

    fn requires_kind(self) -> bool {
        matches!(self, Self::NamedKind | Self::UnderHomeKind)
    }
}

const READABLE: &[Form] = &[Form::Path, Form::Named, Form::UnderHome, Form::Step];
const REMOVABLE: &[Form] = &[Form::Path, Form::NamedKind, Form::UnderHomeKind, Form::Step];
const TARGET: Arg = Arg::Reference("target", READABLE);

static COMMANDS: &[Command] = &[
    Command {
        group: "fs",
        verb: "list",
        elevate: true,
        args: &[Arg::Reference(
            "target",
            &[
                Form::Place,
                Form::Path,
                Form::Named,
                Form::UnderHome,
                Form::Step,
            ],
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
            Arg::Reference(
                "target",
                &[Form::Place, Form::Path, Form::Named, Form::UnderHome],
            ),
            Arg::Number("count"),
        ],
    },
    Command {
        group: "fs",
        verb: "write",
        elevate: true,
        args: &[
            Arg::Reference(
                "target",
                &[Form::Path, Form::NamedKind, Form::UnderHomeKind, Form::New],
            ),
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
            Arg::Destination(
                "to",
                &[
                    Form::Place,
                    Form::Path,
                    Form::Named,
                    Form::UnderHome,
                    Form::New,
                ],
            ),
        ],
    },
    Command {
        group: "fs",
        verb: "delete",
        elevate: true,
        args: &[Arg::Reference("target", REMOVABLE)],
    },
    Command {
        group: "session",
        verb: "cd",
        elevate: false,
        args: &[Arg::Directory(
            "target",
            &[Form::Place, Form::Path, Form::Named, Form::UnderHome],
        )],
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
    Command {
        group: "soul",
        verb: "undo_change",
        elevate: false,
        args: &[],
    },
    Command {
        group: "soul",
        verb: "history",
        elevate: false,
        args: &[],
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
                Form::Path => "path",
                Form::Named | Form::NamedKind => "named",
                Form::UnderHome | Form::UnderHomeKind => "under_home",
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
                    Arg::Reference(key, forms)
                    | Arg::Destination(key, forms)
                    | Arg::Directory(key, forms) => Some((*key, names(forms))),
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
                    | Arg::Directory(key, _)
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
        let words = expand_alias(words)?;
        let (command, step) = parse_step(index + 1, &words)?;
        actions.push(command.action());
        documents.push(step);
    }
    Ok(Parsed::Plan(
        json!({"kind": "plan", "version": VERSION, "summary": summary(&actions), "steps": documents})
            .to_string(),
    ))
}

/// A short word for a command typed often: a path, then the flags of the
/// command. It expands to the mechanical form before parsing, so the table
/// above stays the only definition of what a command means (ADR 0022).
struct Alias {
    word: &'static str,
    group: &'static str,
    verb: &'static str,
    /// The path when none is typed; `None` makes the path required.
    default: Option<&'static str>,
    /// A path that works, for the error that asks for one.
    example: &'static str,
}

const ALIASES: &[Alias] = &[
    Alias {
        word: "cd",
        group: "session",
        verb: "cd",
        default: None,
        example: "~/Downloads",
    },
    Alias {
        word: "ls",
        group: "fs",
        verb: "list",
        default: Some("."),
        example: "~/Downloads",
    },
    Alias {
        word: "rm",
        group: "fs",
        verb: "delete",
        default: None,
        example: "notes/old.txt",
    },
];

fn is_reference_flag(word: &Word) -> bool {
    is_flag(word)
        && [
            "in",
            "in-path",
            "path",
            "name",
            "under-home",
            "new-name",
            "from-step",
            "kind",
        ]
        .contains(&&word.text[2..])
}

fn plain_word(text: &str) -> Word {
    Word {
        text: text.to_owned(),
        quoted: false,
        first_quoted: false,
    }
}

fn expand_alias(words: &[Word]) -> Result<Vec<Word>, CommandError> {
    let [first, rest @ ..] = words else {
        return Ok(Vec::new());
    };
    let Some(alias) = ALIASES
        .iter()
        .find(|a| !first.quoted && a.word == first.text)
    else {
        return Ok(words.to_vec());
    };
    let (path, flags) = match rest {
        // A path that starts with `-` is most likely a flag of the shell's own
        // `rm -rf`: refuse it rather than delete a file with that name.
        [path, ..] if !path.first_quoted && path.text.starts_with('-') && !is_flag(path) => {
            return Err(CommandError::new(format!(
                "{}: {:?} looks like a flag of the shell's {}; this one takes a path and the flags of `help` \
                 (quote the path, or write ./{}, if it really starts with -)",
                alias.word, path.text, alias.word, path.text
            )));
        }
        [path, flags @ ..] if !is_flag(path) => (path.clone(), flags),
        // A reference given by flag (`ls --in downloads`) is the person's own, so
        // the default path is not added next to it.
        _ if alias.default.is_some() && rest.iter().any(is_reference_flag) => {
            return Ok(std::iter::once(plain_word(alias.group))
                .chain(std::iter::once(plain_word(alias.verb)))
                .chain(rest.iter().cloned())
                .collect());
        }
        _ => match alias.default {
            Some(default) => (plain_word(default), rest),
            None if rest.iter().any(is_reference_flag) => {
                return Err(CommandError::new(format!(
                    "{} takes a path, not a reference by flag; for that, write `{} {}`",
                    alias.word, alias.group, alias.verb
                )));
            }
            None => {
                return Err(CommandError::new(format!(
                    "{} needs a path, for example `{} {}`",
                    alias.word, alias.word, alias.example
                )));
            }
        },
    };
    if let Some(extra) = flags.first().filter(|w| !is_flag(w)) {
        return Err(CommandError::new(format!(
            "{}: one path only, and {:?} is a second; quote a path that has spaces",
            alias.word, extra.text
        )));
    }
    let mut expanded = vec![
        plain_word(alias.group),
        plain_word(alias.verb),
        plain_word("--path"),
        path,
    ];
    expanded.extend_from_slice(flags);
    Ok(expanded)
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

#[derive(Clone)]
struct Word {
    text: String,
    /// Whether any part was quoted: a quoted `--x` is a value, not a flag.
    quoted: bool,
    /// Whether the first character was quoted or escaped, so `-'rf'` (a flag
    /// to a person used to a shell) is not mistaken for a literal `-rf`.
    first_quoted: bool,
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
            first_quoted: false,
        });
        if word.text.is_empty() {
            word.first_quoted = quoted;
        }
        word.text.push(c);
        word.quoted |= quoted;
    };
    let open = |current: &mut Option<Word>| {
        current
            .get_or_insert(Word {
                text: String::new(),
                quoted: false,
                first_quoted: false,
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
            Arg::Reference(..) => allowed.extend(
                [
                    "in",
                    "in-path",
                    "path",
                    "name",
                    "under-home",
                    "kind",
                    "new-name",
                    "from-step",
                ]
                .map(str::to_owned),
            ),
            Arg::Destination(..) => allowed.extend(
                [
                    "to-in",
                    "to-in-path",
                    "to-path",
                    "to-name",
                    "to-under-home",
                    "to-new-name",
                ]
                .map(str::to_owned),
            ),
            Arg::Directory(..) => {
                allowed.extend(["in", "in-path", "path", "name", "under-home"].map(str::to_owned));
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
            if group.text.contains('.') {
                return CommandError::new(format!(
                    "unknown command {}; write the group and the action as two words, \
                     for example `fs list`; try help",
                    group.text
                ));
            }
            CommandError::new(format!(
                "unknown command {} {}; try help",
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
        if flag.ends_with("under-home") {
            if flags.0.insert(flag.to_owned(), String::new()).is_some() {
                return Err(CommandError::new(format!("{name}: --{flag} given twice")));
            }
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
        Arg::Reference(key, forms) => (*key, reference(name, flags, "", false, forms, depends_on)?),
        Arg::Destination(key, forms) => (
            *key,
            reference(name, flags, "to-", true, forms, depends_on)?,
        ),
        Arg::Directory(key, forms) => (*key, reference(name, flags, "", true, forms, depends_on)?),
        Arg::Text(key) => (*key, Value::String(required(flags, key)?)),
        Arg::Number(key) => {
            let text = required(flags, key)?;
            // Digits only: no sign, no space, nothing a number would not have.
            let digits = text.chars().all(|c| c.is_ascii_digit());
            let number: u32 = text.parse().ok().filter(|_| digits).ok_or_else(|| {
                CommandError::new(format!(
                    "{name}: --{} must be a whole number (the format allows 1 to 100)",
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

/// Where a named or new item is: a place, or a path as typed.
enum Within {
    Place(String),
    Path(String),
}

impl Within {
    fn value(&self) -> Value {
        match self {
            Self::Place(place) => json!(place),
            Self::Path(path) => json!({ "path": path }),
        }
    }
}

/// The flags of one reference, with `prefix` in front of each. Which forms an
/// argument accepts is the format's rule, not this function's: the check here
/// only lets the error say which flags the action takes. `folder` is a
/// destination or the target of `cd`, whose name is always a folder.
fn reference(
    name: &str,
    flags: &mut Flags,
    prefix: &str,
    folder: bool,
    forms: &[Form],
    depends_on: &mut Vec<String>,
) -> Result<Value, CommandError> {
    let kind_required = forms.iter().any(|f| f.requires_kind());
    let (form, value) = reference_value(
        name,
        flags,
        prefix,
        folder,
        kind_required,
        forms,
        depends_on,
    )?;
    if forms.iter().any(|f| accepts(*f, form)) {
        return Ok(value);
    }
    let takes: Vec<String> = forms.iter().map(|f| f.flags(prefix, folder)).collect();
    Err(CommandError::new(format!(
        "{name}: this reference is not accepted here; use {}",
        takes.join(" or ")
    )))
}

/// Whether an argument that lists `listed` takes a reference written as `written`
/// (the kind rule is a property of the list, not of the written form).
fn accepts(listed: Form, written: Form) -> bool {
    match (listed, written) {
        (Form::NamedKind, Form::Named) | (Form::UnderHomeKind, Form::UnderHome) => true,
        (listed, written) => listed == written,
    }
}

fn reference_value(
    name: &str,
    flags: &mut Flags,
    prefix: &str,
    folder: bool,
    kind_required: bool,
    forms: &[Form],
    depends_on: &mut Vec<String>,
) -> Result<(Form, Value), CommandError> {
    let mut take = |flag: &str| flags.take(&format!("{prefix}{flag}"));
    let place = take("in");
    let place_path = take("in-path");
    let path = take("path");
    let under_home = take("under-home").is_some();
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
    let within = match (place, place_path) {
        (Some(_), Some(_)) => {
            return Err(CommandError::new(format!(
                "{name}: {} and {} are two ways to say where; give one",
                flag("in"),
                flag("in-path")
            )));
        }
        (Some(place), None) => Some(Within::Place(place)),
        (None, Some(path)) => Some(Within::Path(path)),
        (None, None) => None,
    };
    let alone = |what: &str| CommandError::new(format!("{name}: {} stands alone", flag(what)));
    let needs_where = |what: &str| {
        CommandError::new(format!(
            "{name}: {what} needs {} or {}",
            flag("in"),
            flag("in-path")
        ))
    };
    let kind_of = |kind: Option<String>| -> Result<String, CommandError> {
        match (folder, kind) {
            (true, _) => Ok("folder".to_owned()),
            (false, Some(kind)) => Ok(kind),
            (false, None) if kind_required => Err(CommandError::new(format!(
                "{name}: {} needs {}, because it writes, removes or moves what it names",
                flag("name"),
                flag("kind")
            ))),
            (false, None) => Ok("any".to_owned()),
        }
    };
    match (from_step, path, under_home, new_item, item) {
        (Some(step), None, false, None, None) if within.is_none() && kind.is_none() => {
            if !depends_on.contains(&step) {
                depends_on.push(step.clone());
            }
            Ok((Form::Step, json!({"from_step": step})))
        }
        (Some(_), ..) => Err(alone("from-step")),
        (None, Some(path), false, None, None) if within.is_none() && kind.is_none() => {
            Ok((Form::Path, json!({"path": path})))
        }
        (None, Some(_), ..) => Err(alone("path")),
        (None, None, true, None, item) => {
            if within.is_some() {
                return Err(CommandError::new(format!(
                    "{name}: {} searches below the home; it takes no {} or {}",
                    flag("under-home"),
                    flag("in"),
                    flag("in-path")
                )));
            }
            let item = item.ok_or_else(|| {
                CommandError::new(format!(
                    "{name}: {} needs {}",
                    flag("under-home"),
                    flag("name")
                ))
            })?;
            Ok((
                Form::UnderHome,
                json!({"under_home": true, "name": item, "kind": kind_of(kind)?}),
            ))
        }
        (None, None, true, Some(_), _) => Err(CommandError::new(format!(
            "{name}: {} and {} are two different references",
            flag("under-home"),
            flag("new-name")
        ))),
        (None, None, false, Some(_), Some(_)) => Err(CommandError::new(format!(
            "{name}: {} and {} are two different references",
            flag("name"),
            flag("new-name")
        ))),
        (None, None, false, Some(new_name), None) => {
            if kind.is_some() {
                return Err(CommandError::new(format!(
                    "{name}: {} goes with {}, not {}",
                    flag("kind"),
                    flag("name"),
                    flag("new-name")
                )));
            }
            let within = within.ok_or_else(|| needs_where(&flag("new-name")))?;
            Ok((
                Form::New,
                json!({"new_name": new_name, "in": within.value()}),
            ))
        }
        (None, None, false, None, Some(item)) => {
            let within = within.ok_or_else(|| needs_where(&flag("name")))?;
            Ok((
                Form::Named,
                json!({"name": item, "in": within.value(), "kind": kind_of(kind)?}),
            ))
        }
        (None, None, false, None, None) => match (within, kind) {
            (Some(Within::Place(place)), None) => Ok((Form::Place, json!({"place": place}))),
            (Some(Within::Path(_)), None) => Err(CommandError::new(format!(
                "{name}: {} goes with {} or {}; to name a path, use {}",
                flag("in-path"),
                flag("name"),
                flag("new-name"),
                flag("path")
            ))),
            (_, Some(_)) => Err(CommandError::new(format!(
                "{name}: {} goes with {}",
                flag("kind"),
                flag("name")
            ))),
            (None, None) => Err(CommandError::new(format!(
                "{name} needs a reference: {}",
                forms
                    .iter()
                    .map(|f| f.flags(prefix, folder))
                    .collect::<Vec<_>>()
                    .join(" or ")
            ))),
        },
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
                Arg::Reference(_, forms) => forms_help(forms, "", false),
                Arg::Destination(_, forms) => forms_help(forms, "to-", true),
                Arg::Directory(_, forms) => forms_help(forms, "", true),
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
    for alias in ALIASES {
        let path = match alias.default {
            Some(default) => {
                format!("[PATH]  (the path is {default}, the current directory, when left out)")
            }
            None => "PATH".to_owned(),
        };
        out.push_str(&format!(
            "  {} {path}  is  {} {} --path PATH\n",
            alias.word, alias.group, alias.verb
        ));
    }
    out.push_str(&format!(
        "A kind left out means any, and only where the line above shows it in brackets; a destination name and the name of cd are always a folder.\n\
         --path is a path as typed: absolute, ~, or relative to the current directory; --in-path is the same, as the folder of a name. --under-home searches below the home.\n\
         --from-step sN means what step sN found; flags after `--` belong to the program.\n\
         Places: {places}.\n\
         Steps are s1, s2, ... by position; --after sN makes a step wait for sN.\n\
         Quote with '...' or \"...\"; nothing is expanded. A line holds up to {MAX_STEPS} steps; --count is 1 to 100.\n",
        places = place_words()
    ));
    out
}

fn forms_help(forms: &[Form], prefix: &str, folder: bool) -> String {
    let all: Vec<String> = forms.iter().map(|f| f.flags(prefix, folder)).collect();
    format!("({})", all.join(" | "))
}
