//! How a plan refers to resources without a path (ADR 0014).
//!
//! A `Spec` is what the model asked for: an action whose resources are
//! `Target`s. Only the resolver and the broker's `from_step` expansion turn a
//! spec into `Action`s that hold real locations, so an unresolved reference
//! cannot reach the permission checks or the host.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::action::{Action, RootRule, WriteMode, quoted};
use crate::plan::StepId;

/// The places a plan may name. The adapter decides where each one lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Place {
    Home,
    Downloads,
    Documents,
    Desktop,
    Pictures,
    Music,
    Videos,
    /// All the person's files, wherever they live. Not a path.
    Everywhere,
}

impl Place {
    pub const ALL: [Place; 8] = [
        Self::Home,
        Self::Downloads,
        Self::Documents,
        Self::Desktop,
        Self::Pictures,
        Self::Music,
        Self::Videos,
        Self::Everywhere,
    ];

    /// What the person sees.
    pub fn title(self) -> &'static str {
        match self {
            Self::Home => "Home",
            Self::Downloads => "Downloads",
            Self::Documents => "Documents",
            Self::Desktop => "Desktop",
            Self::Pictures => "Pictures",
            Self::Music => "Music",
            Self::Videos => "Videos",
            Self::Everywhere => "everywhere",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    File,
    Folder,
    Any,
}

/// A name is one directory entry: it cannot say where to look.
pub fn is_single_name(name: &str) -> bool {
    let length = name.chars().count();
    (1..=255).contains(&length) && !name.contains(['/', '\0']) && name != "." && name != ".."
}

/// A resource the resolver can find or build on its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Direct {
    Place(Place),
    /// An existing item found by name inside a place.
    Named {
        name: String,
        within: Place,
        kind: Kind,
    },
    /// An item that does not exist yet, to be created inside a place.
    New {
        name: String,
        within: Place,
    },
}

impl Direct {
    /// For the person: never a path.
    pub fn describe(&self) -> String {
        match self {
            Self::Place(place) => place.title().to_owned(),
            Self::Named { name, within, kind } => {
                let what = match kind {
                    Kind::File => "file ",
                    Kind::Folder => "folder ",
                    Kind::Any => "",
                };
                format!("{what}{} in {}", quoted(name), within.title())
            }
            Self::New { name, within } => format!("new {} in {}", quoted(name), within.title()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Direct(Direct),
    /// Everything an earlier step produced.
    FromStep(StepId),
}

impl Target {
    fn describe(&self) -> String {
        match self {
            Self::Direct(direct) => direct.describe(),
            Self::FromStep(step) => format!("what {step} found"),
        }
    }
}

/// Which resource position of a step a reference fills.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Slot {
    Primary,
    Destination,
}

/// Where a move puts its source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    /// Inside this folder, keeping the name.
    Into(PathBuf),
    /// At exactly this location.
    Exact(PathBuf),
}

/// An action as the model proposed it, before any reference is resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Spec {
    List {
        target: Target,
    },
    Stat {
        target: Target,
    },
    Read {
        target: Target,
    },
    Delete {
        target: Target,
    },
    FindLargest {
        target: Target,
        count: u32,
    },
    Write {
        target: Target,
        content: String,
        mode: WriteMode,
    },
    Move {
        from: Target,
        to: Target,
    },
    /// An action with no resource to resolve (accounts, configuration, programs, the SOUL).
    Plain(Action),
}

impl Spec {
    pub fn name(&self) -> &'static str {
        match self {
            Self::List { .. } => "fs.list",
            Self::Stat { .. } => "fs.stat",
            Self::Read { .. } => "fs.read",
            Self::Delete { .. } => "fs.delete",
            Self::FindLargest { .. } => "fs.find_largest",
            Self::Write { .. } => "fs.write",
            Self::Move { .. } => "fs.move",
            Self::Plain(action) => action.name(),
        }
    }

    pub fn root_rule(&self) -> RootRule {
        match self {
            Self::Plain(action) => action.root_rule(),
            _ => RootRule::OnRequest,
        }
    }

    pub fn produces_paths(&self) -> bool {
        matches!(self, Self::List { .. } | Self::FindLargest { .. })
    }

    /// The reference in each resource position, primary first.
    pub fn targets(&self) -> Vec<(Slot, &Target)> {
        match self {
            Self::List { target }
            | Self::Stat { target }
            | Self::Read { target }
            | Self::Delete { target }
            | Self::FindLargest { target, .. }
            | Self::Write { target, .. } => vec![(Slot::Primary, target)],
            Self::Move { from, to } => vec![(Slot::Primary, from), (Slot::Destination, to)],
            Self::Plain(_) => Vec::new(),
        }
    }

    /// The step whose results this step uses, if any.
    pub fn source_step(&self) -> Option<&StepId> {
        self.targets().into_iter().find_map(|(_, t)| match t {
            Target::FromStep(step) => Some(step),
            Target::Direct(_) => None,
        })
    }

    /// What the step asks for, in words, before it has any real location.
    pub fn describe(&self) -> String {
        match self {
            Self::List { target } => format!("List {}", target.describe()),
            Self::Stat { target } => format!("Show details of {}", target.describe()),
            Self::Read { target } => format!("Read {}", target.describe()),
            Self::Delete { target } => format!("Delete {}", target.describe()),
            Self::FindLargest { target, count } => {
                format!("Find the {count} largest files in {}", target.describe())
            }
            Self::Write { target, mode, .. } => {
                format!("Write ({mode:?}) {}", target.describe())
            }
            Self::Move { from, to } => {
                format!("Move {} to {}", from.describe(), to.describe())
            }
            Self::Plain(action) => action.describe(),
        }
    }

    /// The concrete actions for the locations the resolver found: one per
    /// source location, and a single one when the step has no source list.
    pub fn instantiate(
        &self,
        sources: &[PathBuf],
        destination: Option<&Destination>,
    ) -> Vec<Action> {
        let each = |build: &dyn Fn(PathBuf) -> Action| sources.iter().cloned().map(build).collect();
        match self {
            Self::List { .. } => each(&|path| Action::List { path }),
            Self::Stat { .. } => each(&|path| Action::Stat { path }),
            Self::Read { .. } => each(&|path| Action::Read { path }),
            Self::Delete { .. } => each(&|path| Action::Delete { path }),
            Self::FindLargest { count, .. } => each(&|path| Action::FindLargest {
                path,
                count: *count,
            }),
            Self::Write { content, mode, .. } => each(&|path| Action::Write {
                path,
                content: content.clone(),
                mode: *mode,
            }),
            Self::Move { .. } => sources
                .iter()
                .filter_map(|from| {
                    Some(Action::Move {
                        from: from.clone(),
                        to: destination?.apply(from),
                    })
                })
                .collect(),
            Self::Plain(action) => vec![action.clone()],
        }
    }
}

impl Destination {
    fn apply(&self, source: &Path) -> PathBuf {
        match self {
            Self::Exact(path) => path.clone(),
            Self::Into(folder) => folder.join(source.file_name().unwrap_or_default()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_one_directory_entry() {
        for good in ["a", "report.pdf", ".hidden", "a b", "ação"] {
            assert!(is_single_name(good), "{good}");
        }
        for bad in [
            "",
            ".",
            "..",
            "a/b",
            "/etc",
            "../x",
            "a\0b",
            &"x".repeat(256),
        ] {
            assert!(!is_single_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn a_move_into_a_folder_keeps_the_name_and_a_move_to_a_new_name_does_not() {
        let spec = Spec::Move {
            from: Target::FromStep(StepId::parse("s1").unwrap()),
            to: Target::FromStep(StepId::parse("s1").unwrap()),
        };
        let source = [PathBuf::from("/a/x.txt")];
        let into = spec.instantiate(&source, Some(&Destination::Into(PathBuf::from("/b"))));
        let exact = spec.instantiate(&source, Some(&Destination::Exact(PathBuf::from("/b/y"))));
        assert_eq!(into[0].accesses()[1].1, Path::new("/b/x.txt"));
        assert_eq!(exact[0].accesses()[1].1, Path::new("/b/y"));
    }
}
