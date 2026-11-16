//! How a plan refers to resources (ADR 0014, extended by ADR 0018).
//!
//! A `Spec` is what the model asked for: an action whose resources are
//! `Target`s. A target is a place, a name inside a place or a path, a search
//! below the person's home, a path (absolute, `~` or relative to the current
//! directory), or what an earlier step found. Only the resolver and the
//! broker's `from_step` expansion turn a spec into `Action`s that hold real
//! locations, so an unresolved reference cannot reach the permission checks
//! or the host.

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
}

impl Place {
    pub const ALL: [Place; 7] = [
        Self::Home,
        Self::Downloads,
        Self::Documents,
        Self::Desktop,
        Self::Pictures,
        Self::Music,
        Self::Videos,
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
    // NAME_MAX counts bytes, not characters.
    (1..=255).contains(&name.len()) && !name.contains(['/', '\0']) && name != "." && name != ".."
}

/// The longest path text a plan may carry (PATH_MAX).
pub const MAX_PATH_BYTES: usize = 4096;

/// A path as typed: something to look at, not yet a location. The schema
/// bounds the length in characters, and this is the rule in bytes plus the NUL
/// that no path can hold. What `..` means is the resolver's rule.
pub fn is_path_text(text: &str) -> bool {
    (1..=MAX_PATH_BYTES).contains(&text.len()) && !text.contains('\0')
}

/// Where a name is looked for, or an item is created: a place or a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Within {
    Place(Place),
    Path(String),
}

impl Within {
    fn describe(&self) -> String {
        match self {
            Self::Place(place) => place.title().to_owned(),
            Self::Path(path) => format!("path {}", quoted(path)),
        }
    }
}

/// A resource the resolver can find or build on its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Direct {
    Place(Place),
    /// What the person typed or saw in the tree: absolute, `~` or relative to
    /// the current directory (ADR 0018).
    Path(String),
    /// An existing item found by name anywhere below the person's home. Shown
    /// as such, never as a place.
    UnderHome {
        name: String,
        kind: Kind,
    },
    /// An existing item found by name inside a place or below a path.
    Named {
        name: String,
        within: Within,
        kind: Kind,
    },
    /// An item that does not exist yet, to be created inside a place or a path.
    New {
        name: String,
        within: Within,
    },
}

fn kind_prefix(kind: Kind) -> &'static str {
    match kind {
        Kind::File => "file ",
        Kind::Folder => "folder ",
        Kind::Any => "",
    }
}

impl Direct {
    /// What the model asked for, in its own words: the location is not known yet.
    pub fn describe(&self) -> String {
        match self {
            Self::Place(place) => place.title().to_owned(),
            Self::Path(path) => format!("path {}", quoted(path)),
            Self::UnderHome { name, kind } => {
                format!("{}{} under home", kind_prefix(*kind), quoted(name))
            }
            Self::Named { name, within, kind } => {
                format!(
                    "{}{} in {}",
                    kind_prefix(*kind),
                    quoted(name),
                    within.describe()
                )
            }
            Self::New { name, within } => {
                format!("new {} in {}", quoted(name), within.describe())
            }
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
    /// Change the session's current directory (ADR 0018).
    Cd {
        target: Target,
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
            Self::Cd { .. } => "session.cd",
            Self::Plain(action) => action.name(),
        }
    }

    pub fn root_rule(&self) -> RootRule {
        match self {
            Self::Plain(action) => action.root_rule(),
            Self::Cd { .. } => RootRule::Never,
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
            | Self::Cd { target }
            | Self::FindLargest { target, .. }
            | Self::Write { target, .. } => vec![(Slot::Primary, target)],
            Self::Move { from, to } => vec![(Slot::Primary, from), (Slot::Destination, to)],
            Self::Plain(_) => Vec::new(),
        }
    }

    /// Whether a path in this position must name something that exists: only
    /// what a step creates may not yet.
    pub fn needs_existing(&self, slot: Slot) -> bool {
        match self {
            Self::Write { .. } => false,
            Self::Move { .. } => slot == Slot::Primary,
            _ => true,
        }
    }

    /// Whether a path in the primary position that ends in a link means what
    /// the link leads to (a directory to enter) or the link itself.
    pub fn follows_last_link(&self) -> bool {
        matches!(self, Self::Cd { .. })
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
            Self::Cd { target } => format!("Go to {}", target.describe()),
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
            Self::Cd { .. } => each(&|path| Action::Cd { path }),
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
    fn a_path_is_text_of_a_bounded_length_with_no_nul() {
        for good in ["~", "/", "a/b", "../x", "ação/é", &"x".repeat(4096)] {
            assert!(is_path_text(good), "{good}");
        }
        for bad in ["", "a\0b", &"x".repeat(4097), &"é".repeat(2049)] {
            assert!(!is_path_text(bad), "{bad:?}");
        }
    }

    #[test]
    fn a_reference_is_described_in_the_models_words_with_no_location() {
        let path = Direct::Path("~/a b".into());
        assert_eq!(path.describe(), r#"path "~/a b""#);
        let home = Direct::UnderHome {
            name: "x".into(),
            kind: Kind::Folder,
        };
        assert_eq!(home.describe(), r#"folder "x" under home"#);
        let named = Direct::Named {
            name: "x".into(),
            within: Within::Path("d".into()),
            kind: Kind::Any,
        };
        assert_eq!(named.describe(), r#""x" in path "d""#);
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

    #[test]
    fn the_places_are_the_same_in_the_schema_and_the_prompt() {
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../../docs/schemas/action-plan.schema.json"))
                .unwrap();
        let in_schema: Vec<&str> = schema["$defs"]["place"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        let prompt = include_str!("model.rs");
        for place in Place::ALL {
            let key = format!("{place:?}").to_lowercase();
            assert!(
                in_schema.contains(&key.as_str()),
                "{key} missing from the schema"
            );
            assert!(prompt.contains(&key), "{key} missing from the prompt");
        }
        assert_eq!(in_schema.len(), Place::ALL.len());
    }
}
