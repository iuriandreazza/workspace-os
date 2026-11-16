//! The actions of ADR 0011, the capability each one needs and whether it needs root.

use std::fmt;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteMode {
    Create,
    Overwrite,
    Append,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserOperation {
    Add,
    Remove,
    AddToGroup,
    RemoveFromGroup,
}

impl UserOperation {
    pub fn needs_group(self) -> bool {
        matches!(self, Self::AddToGroup | Self::RemoveFromGroup)
    }
}

/// One concrete action: every location is a real one, nothing is left to resolve.
/// Built only from a `Spec` by the resolver and the broker (ADR 0014).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    List {
        path: PathBuf,
    },
    Stat {
        path: PathBuf,
    },
    Read {
        path: PathBuf,
    },
    FindLargest {
        path: PathBuf,
        count: u32,
    },
    Write {
        path: PathBuf,
        content: String,
        mode: WriteMode,
    },
    Move {
        from: PathBuf,
        to: PathBuf,
    },
    Delete {
        path: PathBuf,
    },
    Run {
        program: String,
        argv: Vec<String>,
    },
    UserManage {
        operation: UserOperation,
        user: String,
        group: Option<String>,
    },
    SystemConfigure {
        key: String,
        value: String,
    },
    SoulProposeChange {
        rationale: String,
        new_text: String,
    },
}

/// Whether any part of the path is hidden (starts with a dot).
fn hidden(path: &Path) -> bool {
    path.components()
        .any(|part| part.as_os_str().as_bytes().first() == Some(&b'.'))
}

impl Action {
    /// Whether the person is asked before this action runs (ADR 0016). Reading
    /// and adding are not asked about; anything that changes, removes or
    /// reaches beyond files is. There is no wildcard: a new action must be
    /// classified before it compiles.
    pub fn needs_confirmation(&self) -> bool {
        match self {
            Self::List { .. } | Self::Stat { .. } | Self::FindLargest { .. } => false,
            // Startup files and credentials are dotfiles or live under one
            // (`.zshrc`, `.ssh/`): showing them, or adding to them, is asked.
            Self::Read { path } => hidden(path),
            Self::Write { path, mode, .. } => match mode {
                WriteMode::Create | WriteMode::Append => hidden(path),
                WriteMode::Overwrite => true,
            },
            Self::Move { .. }
            | Self::Delete { .. }
            | Self::Run { .. }
            | Self::UserManage { .. }
            | Self::SystemConfigure { .. }
            | Self::SoulProposeChange { .. } => true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Capability {
    FsRead,
    FsWrite,
    FsDelete,
    ProcessExec,
    UserAdmin,
    SystemConfig,
    SoulPropose,
}

impl Capability {
    pub const ALL: [Capability; 7] = [
        Self::FsRead,
        Self::FsWrite,
        Self::FsDelete,
        Self::ProcessExec,
        Self::UserAdmin,
        Self::SystemConfig,
        Self::SoulPropose,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::FsRead => "fs.read",
            Self::FsWrite => "fs.write",
            Self::FsDelete => "fs.delete",
            Self::ProcessExec => "process.exec",
            Self::UserAdmin => "user.admin",
            Self::SystemConfig => "system.config",
            Self::SoulPropose => "soul.propose",
        }
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// What a user's own permissions must allow on one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Looking at an entry (`stat`) needs to reach it, not to read it.
    Stat,
    Read,
    Write,
    /// Removing an entry needs write access to the directory that holds it.
    Delete,
}

impl fmt::Display for Access {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Stat => "stat",
            Self::Read => "read",
            Self::Write => "write",
            Self::Delete => "delete",
        })
    }
}

/// The root column of the action table in ADR 0011.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootRule {
    Always,
    OnRequest,
    /// The action cannot be elevated and never needs root.
    Never,
}

/// `text` as a JSON string with every character that could change what a
/// terminal shows escaped too: JSON leaves DEL, the C1 controls (U+009B is a
/// CSI), bidirectional overrides and line separators as they are.
pub fn quoted(text: &str) -> String {
    let json = serde_json::to_string(text).unwrap_or_default();
    json.chars().map(escape_unsafe).collect()
}

/// A path quoted like `quoted`, byte-exact: a byte that is not UTF-8 shows as
/// `\\xNN`, so two different names never show as the same one.
pub fn quoted_path(path: &Path) -> String {
    let mut out = String::from("\"");
    for chunk in path.as_os_str().as_bytes().utf8_chunks() {
        let valid = quoted(chunk.valid());
        out.push_str(&valid[1..valid.len() - 1]);
        for byte in chunk.invalid() {
            out.push_str(&format!("\\x{byte:02x}"));
        }
    }
    out.push('"');
    out
}

/// Make text from the model or the machine safe to print: control and
/// formatting characters become escapes, except newline and tab when `block`.
pub fn printable(text: &str, block: bool) -> String {
    text.chars()
        .map(|c| {
            if block && (c == '\n' || c == '\t') {
                c.to_string()
            } else {
                escape_unsafe(c)
            }
        })
        .collect()
}

fn escape_unsafe(c: char) -> String {
    // Blanks other than the plain space let two different names look alike.
    let blank = c.is_whitespace() && c != ' ';
    // The Unicode format (Cf) characters and the combining joiner: invisible,
    // so two different paths could look the same.
    let formatting = matches!(
        c,
        '\u{AD}'
            | '\u{34F}'
            | '\u{61C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{2028}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{E0000}'..='\u{E007F}'
            | '\u{600}'..='\u{605}'
            | '\u{6DD}'
            | '\u{70F}'
            | '\u{115F}'
            | '\u{1160}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{13430}'..='\u{1343F}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{2800}'
            | '\u{3164}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FFA0}'
            | '\u{E0100}'..='\u{E01EF}'
            | '\u{180B}'..='\u{180D}'
            | '\u{180F}'
            | '\u{17B4}'
            | '\u{17B5}'
            | '\u{1D159}'
            | '\u{FFFC}'
            | '\u{8E2}'
            | '\u{110BD}'
            | '\u{110CD}'
            | '\u{890}'
            | '\u{891}'
            | '\u{FFFE}'
            | '\u{FFFF}'
            | '\u{13440}'
            | '\u{E000}'..='\u{F8FF}'
            | '\u{F0000}'..='\u{10FFFF}'
    );
    if c.is_control() || formatting || blank {
        // Beyond the BMP the digits vary in number, so they are delimited:
        // otherwise U+F0000 and U+F000 followed by "0" would print alike.
        if (c as u32) > 0xFFFF {
            format!("\\u{{{:x}}}", c as u32)
        } else {
            format!("\\u{:04x}", c as u32)
        }
    } else {
        c.to_string()
    }
}

impl Action {
    pub fn name(&self) -> &'static str {
        match self {
            Self::List { .. } => "fs.list",
            Self::Stat { .. } => "fs.stat",
            Self::Read { .. } => "fs.read",
            Self::FindLargest { .. } => "fs.find_largest",
            Self::Write { .. } => "fs.write",
            Self::Move { .. } => "fs.move",
            Self::Delete { .. } => "fs.delete",
            Self::Run { .. } => "process.run",
            Self::UserManage { .. } => "user.manage",
            Self::SystemConfigure { .. } => "system.configure",
            Self::SoulProposeChange { .. } => "soul.propose_change",
        }
    }

    pub fn capability(&self) -> Capability {
        match self {
            Self::List { .. }
            | Self::Stat { .. }
            | Self::Read { .. }
            | Self::FindLargest { .. } => Capability::FsRead,
            Self::Write { .. } | Self::Move { .. } => Capability::FsWrite,
            Self::Delete { .. } => Capability::FsDelete,
            Self::Run { .. } => Capability::ProcessExec,
            Self::UserManage { .. } => Capability::UserAdmin,
            Self::SystemConfigure { .. } => Capability::SystemConfig,
            Self::SoulProposeChange { .. } => Capability::SoulPropose,
        }
    }

    pub fn root_rule(&self) -> RootRule {
        match self {
            Self::UserManage { .. } | Self::SystemConfigure { .. } => RootRule::Always,
            Self::SoulProposeChange { .. } => RootRule::Never,
            _ => RootRule::OnRequest,
        }
    }

    /// Reads change nothing, so the broker may run them before confirmation to
    /// expand a `from_step` reference (ADR 0011).
    pub fn is_read_only(&self) -> bool {
        self.capability() == Capability::FsRead
    }

    /// What the user's own permissions must allow, path by path.
    pub fn accesses(&self) -> Vec<(Access, &Path)> {
        match self {
            Self::Stat { path } => vec![(Access::Stat, path)],
            Self::List { path } | Self::Read { path } => vec![(Access::Read, path)],
            Self::FindLargest { path, .. } => vec![(Access::Read, path)],
            Self::Write { path, .. } => vec![(Access::Write, path)],
            Self::Move { from, to } => vec![(Access::Delete, from), (Access::Write, to)],
            Self::Delete { path } => vec![(Access::Delete, path)],
            Self::Run { .. }
            | Self::UserManage { .. }
            | Self::SystemConfigure { .. }
            | Self::SoulProposeChange { .. } => Vec::new(),
        }
    }

    /// The exact action as it will run. Strings are JSON-quoted so a control
    /// character from the model cannot rewrite what the terminal shows.
    pub fn describe(&self) -> String {
        let q = quoted;
        let p = quoted_path;
        let name = self.name();
        match self {
            Self::List { path }
            | Self::Stat { path }
            | Self::Read { path }
            | Self::Delete { path } => {
                format!("{name} path={}", p(path))
            }
            Self::FindLargest { path, count } => format!("{name} path={} count={count}", p(path)),
            Self::Write {
                path,
                content,
                mode,
            } => {
                format!(
                    "{name} path={} mode={mode:?} content={}",
                    p(path),
                    q(content)
                )
            }
            Self::Move { from, to } => format!("{name} from={} to={}", p(from), p(to)),
            Self::Run { program, argv } => format!(
                "{name} program={} argv=[{}]",
                q(program),
                argv.iter().map(|a| q(a)).collect::<Vec<_>>().join(", ")
            ),
            Self::UserManage {
                operation,
                user,
                group,
            } => format!(
                "{name} operation={operation:?} user={}{}",
                q(user),
                group
                    .as_deref()
                    .map(|g| format!(" group={}", q(g)))
                    .unwrap_or_default()
            ),
            Self::SystemConfigure { key, value } => {
                format!("{name} key={} value={}", q(key), q(value))
            }
            Self::SoulProposeChange {
                rationale,
                new_text,
            } => {
                format!("{name} rationale={} new_text={}", q(rationale), q(new_text))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_escapes_what_json_leaves_alone() {
        let quoted = quoted("a\u{202e}b\u{9b}c\u{7f}d\u{2028}e\u{1b}f");
        assert_eq!(quoted, "\"a\\u202eb\\u009bc\\u007fd\\u2028e\\u001bf\"");
    }

    #[test]
    fn quoting_escapes_invisible_format_characters() {
        let quoted = quoted(
            "a\u{890}\u{f0000}\u{e000}\u{fffe}\u{e0041}\u{61c}\u{ad}\u{3164}\u{2800}\u{3000}\u{a0}\u{fe0f}b",
        );
        assert!(quoted.is_ascii(), "{quoted}");
    }

    #[test]
    fn two_paths_that_differ_in_a_non_utf8_byte_show_differently() {
        let one = Path::new(std::ffi::OsStr::from_bytes(b"/x/a\xff"));
        let other = Path::new(std::ffi::OsStr::from_bytes(b"/x/a\xfe"));
        assert_ne!(quoted_path(one), quoted_path(other));
        assert_eq!(quoted_path(one), "\"/x/a\\xff\"");
        assert_eq!(quoted_path(Path::new("/x/ação")), "\"/x/ação\"");
    }

    #[test]
    fn every_invisible_character_of_the_list_is_escaped() {
        let listed: Vec<char> = [
            0xAD, 0x34F, 0x61C, 0x180E, 0x200B, 0x200C, 0x200D, 0x200E, 0x200F, 0x2028, 0x2029,
            0x202A, 0x202B, 0x202C, 0x202D, 0x202E, 0x2060, 0x2061, 0x2064, 0x2066, 0x2067, 0x2068,
            0x2069, 0x206F, 0xFEFF, 0xFFF9, 0xFFFB, 0xE0001, 0xE007F, 0x600, 0x605, 0x6DD, 0x70F,
            0x115F, 0x1160, 0x2800, 0x3164, 0xFE00, 0xFE0F, 0xFFA0, 0xFFFC, 0x17B4, 0x17B5,
            0x1D173, 0x1D17A, 0x13430, 0x1BCA0, 0x110BD,
        ]
        .into_iter()
        .filter_map(char::from_u32)
        .collect();
        for c in listed {
            assert!(quoted(&format!("a{c}b")).is_ascii(), "U+{:04X}", c as u32);
        }
    }

    #[test]
    fn code_points_beyond_the_bmp_never_print_like_a_bmp_one_followed_by_a_digit() {
        let long = quoted("\u{f0000}");
        let short = quoted("\u{f000}0");
        assert_ne!(long, short);
        assert_eq!(long, "\"\\u{f0000}\"");
    }

    #[test]
    fn printable_keeps_newlines_only_in_blocks() {
        assert_eq!(printable("a\nb\u{1b}", true), "a\nb\\u001b");
        assert_eq!(printable("a\nb", false), "a\\u000ab");
    }

    #[test]
    fn every_action_needs_its_own_capability_and_root_rule() {
        let p = || PathBuf::from("/x");
        let table = [
            (
                Action::List { path: p() },
                Capability::FsRead,
                RootRule::OnRequest,
            ),
            (
                Action::Stat { path: p() },
                Capability::FsRead,
                RootRule::OnRequest,
            ),
            (
                Action::Read { path: p() },
                Capability::FsRead,
                RootRule::OnRequest,
            ),
            (
                Action::FindLargest {
                    path: p(),
                    count: 1,
                },
                Capability::FsRead,
                RootRule::OnRequest,
            ),
            (
                Action::Write {
                    path: p(),
                    content: String::new(),
                    mode: WriteMode::Create,
                },
                Capability::FsWrite,
                RootRule::OnRequest,
            ),
            (
                Action::Move { from: p(), to: p() },
                Capability::FsWrite,
                RootRule::OnRequest,
            ),
            (
                Action::Delete { path: p() },
                Capability::FsDelete,
                RootRule::OnRequest,
            ),
            (
                Action::Run {
                    program: "ls".into(),
                    argv: vec![],
                },
                Capability::ProcessExec,
                RootRule::OnRequest,
            ),
            (
                Action::UserManage {
                    operation: UserOperation::Add,
                    user: "u".into(),
                    group: None,
                },
                Capability::UserAdmin,
                RootRule::Always,
            ),
            (
                Action::SystemConfigure {
                    key: "k".into(),
                    value: "v".into(),
                },
                Capability::SystemConfig,
                RootRule::Always,
            ),
            (
                Action::SoulProposeChange {
                    rationale: "r".into(),
                    new_text: "t".into(),
                },
                Capability::SoulPropose,
                RootRule::Never,
            ),
        ];
        for (action, capability, rule) in table {
            assert_eq!(action.capability(), capability, "{}", action.name());
            assert_eq!(action.root_rule(), rule, "{}", action.name());
        }
    }

    #[test]
    fn the_confirmation_text_shows_every_argument_of_every_action() {
        let p = |s: &str| PathBuf::from(s);
        let shown = [
            (
                Action::Move {
                    from: p("/a"),
                    to: p("/b"),
                },
                r#"fs.move from="/a" to="/b""#,
            ),
            (
                Action::Write {
                    path: p("/a"),
                    content: "0123456789abcdef".into(),
                    mode: WriteMode::Append,
                },
                r#"fs.write path="/a" mode=Append content="0123456789abcdef""#,
            ),
            (
                Action::Run {
                    program: "ls".into(),
                    argv: vec!["-l".into(), "/x".into()],
                },
                r#"process.run program="ls" argv=["-l", "/x"]"#,
            ),
            (
                Action::UserManage {
                    operation: UserOperation::AddToGroup,
                    user: "bia".into(),
                    group: Some("staff".into()),
                },
                r#"user.manage operation=AddToGroup user="bia" group="staff""#,
            ),
            (
                Action::SystemConfigure {
                    key: "k".into(),
                    value: "v".into(),
                },
                r#"system.configure key="k" value="v""#,
            ),
            (
                Action::SoulProposeChange {
                    rationale: "r".into(),
                    new_text: "t".into(),
                },
                r#"soul.propose_change rationale="r" new_text="t""#,
            ),
            (
                Action::FindLargest {
                    path: p("/a"),
                    count: 3,
                },
                r#"fs.find_largest path="/a" count=3"#,
            ),
        ];
        for (action, text) in shown {
            assert_eq!(action.describe(), text);
        }
    }
}
