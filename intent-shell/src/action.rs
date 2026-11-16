//! The actions of ADR 0011, the capability each one needs and whether it needs root.

use std::fmt;
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

/// One concrete action: every path is a real path, no step reference is left.
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
    Read,
    Write,
    /// Removing an entry needs write access to the directory that holds it.
    Delete,
}

impl fmt::Display for Access {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
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

/// `text` as a JSON string, so control characters show as escapes.
pub fn quoted(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_default()
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

    pub fn produces_paths(&self) -> bool {
        matches!(self, Self::List { .. } | Self::FindLargest { .. })
    }

    /// What the user's own permissions must allow, path by path.
    pub fn accesses(&self) -> Vec<(Access, &Path)> {
        match self {
            Self::List { path } | Self::Stat { path } | Self::Read { path } => {
                vec![(Access::Read, path)]
            }
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

    /// The same action with every path passed through `f`.
    pub fn map_paths(&self, f: &dyn Fn(&Path) -> PathBuf) -> Action {
        let mut action = self.clone();
        match &mut action {
            Self::List { path }
            | Self::Stat { path }
            | Self::Read { path }
            | Self::FindLargest { path, .. }
            | Self::Write { path, .. }
            | Self::Delete { path } => *path = f(path),
            Self::Move { from, to } => {
                *from = f(from);
                *to = f(to);
            }
            Self::Run { .. }
            | Self::UserManage { .. }
            | Self::SystemConfigure { .. }
            | Self::SoulProposeChange { .. } => {}
        }
        action
    }

    /// The path a `from_step` reference replaces: the only reference position
    /// in each action that accepts one.
    pub fn with_referenced_path(&self, referenced: PathBuf) -> Action {
        let mut action = self.clone();
        match &mut action {
            Self::List { path }
            | Self::Stat { path }
            | Self::Read { path }
            | Self::Delete { path } => *path = referenced,
            Self::Move { from, .. } => *from = referenced,
            _ => {}
        }
        action
    }

    /// The exact action as it will run. Strings are JSON-quoted so a control
    /// character from the model cannot rewrite what the terminal shows.
    pub fn describe(&self) -> String {
        let q = quoted;
        let p = |path: &Path| q(&path.to_string_lossy());
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
