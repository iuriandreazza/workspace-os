//! The contracts between the deterministic core and everything around it:
//! the machine it acts on, the model, the person and the log. The core only
//! sees these traits, so every test of the broker runs without a model, a
//! terminal or a real filesystem.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::action::{Access, Action};
use crate::reference::Place;

/// Who the shell acts for. `administrator` means allowed to act as root
/// after confirming (ADR 0007).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub name: String,
    pub administrator: bool,
}

/// One item a search or a listing produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub path: PathBuf,
    pub size: Option<u64>,
}

/// What an executed action produced. `found` feeds `from_step` references.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Output {
    /// Content or details, for actions that return them. Items go in `found`.
    pub text: String,
    pub found: Vec<Found>,
    /// What a listing shows below its direct children, for the tree view only.
    /// It never feeds a `from_step` reference: those expand `found`, so a
    /// delete after a list cannot reach what only the tree displayed.
    pub tree: Vec<TreeEntry>,
    /// What the person must know about how complete the result is. Shown
    /// with the plan, because a reference expands from it.
    pub notes: Vec<String>,
}

impl Output {
    pub fn paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.found.iter().map(|f| &f.path)
    }
}

/// How deep a listing looks for the tree view (ADR 0018, decision 3).
pub const TREE_DEPTH: usize = 2;

/// What an entry of a listing is. A link is never followed in the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
    Link(PathBuf),
    Other,
}

/// One entry the tree view may show, with what only the host can tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    pub path: PathBuf,
    pub kind: EntryKind,
    pub size: Option<u64>,
    /// Whether the user may read it.
    pub readable: bool,
    /// A place or the filesystem root: it cannot be removed or overwritten whole.
    pub protected: bool,
}

/// An item a name search came across.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub is_dir: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Search {
    /// Items whose name contains the needle, ignoring case.
    pub entries: Vec<Entry>,
    /// What the search could not look at.
    pub notes: Vec<String>,
}

pub trait Host {
    /// Where a place lives on this machine, if it has one. The core knows no
    /// directory layout: this is the adapter's mapping (ADR 0014).
    fn place_roots(&self, place: Place) -> Vec<PathBuf>;
    /// The person's home as a real location, where a session starts and what
    /// `~` stands for (ADR 0018).
    fn home(&self) -> Option<PathBuf>;
    /// The real location of an absolute path: the parent is resolved through
    /// links, and so is the last component when `follow_last` (a directory to
    /// enter), but never otherwise, so naming a link to delete or move it
    /// still names the link. What does not exist, or cannot be reached, stays
    /// as written: whether the user may touch it is the broker's question.
    fn canonicalize(&self, path: &Path, follow_last: bool) -> PathBuf;
    /// Whether the location is a directory (through links).
    fn is_dir(&self, path: &Path) -> bool;
    /// Whether the location itself is a symbolic link.
    fn is_symlink(&self, path: &Path) -> bool;
    /// Whether two locations are the same thing on this machine (the same
    /// folder under a different case, or through a link), not only the same text.
    fn same(&self, a: &Path, b: &Path) -> bool;
    /// Whether something already has this location (case-folded where the
    /// filesystem folds case).
    fn exists(&self, path: &Path) -> bool;
    /// Look below `roots`, only where the user may list, for names that
    /// contain `needle`. Says what it could not look at.
    fn search(&self, roots: &[PathBuf], needle: &str) -> Result<Search, String>;
    /// Whether the user's own permissions allow this access to this path.
    fn allows(&self, access: Access, path: &Path) -> bool;
    /// Run one concrete action, or say why this host cannot.
    fn execute(&self, action: &Action) -> Result<Output, String>;
}

/// The model's previous attempt and why it was refused.
pub struct Retry<'a> {
    pub previous_output: &'a str,
    pub error: &'a str,
}

pub trait Planner {
    /// The model's raw text for a request made in `cwd`, as the person sees
    /// it (ADR 0018). Nothing here is trusted.
    fn plan(&self, request: &str, cwd: &str, retry: Option<&Retry>) -> Result<String, String>;
}

/// One line of the question for an ambiguous reference: a candidate the
/// person may pick, or the context it sits in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceLine {
    pub text: String,
    /// The index of the candidate this line stands for; `None` for context.
    pub option: Option<usize>,
    /// For a candidate, its location as the screens write it, for the
    /// history to say which one was chosen. Empty for context.
    pub label: String,
}

pub trait Confirm {
    /// Show the plan screen and ask whether to go on. `details` is the same
    /// screen with the real locations, which the person may ask to see.
    fn plan(&mut self, screen: &str, details: &str) -> bool;
    /// Ask `who`, an administrator, to confirm one root action. The panel
    /// always carries the real location.
    fn root_action(&mut self, panel: &str, who: &User) -> bool;
    /// Ask the person which of several matches they mean, shown in their
    /// parent context (ADR 0018). `None` cancels.
    fn choose(&mut self, question: &str, lines: &[ChoiceLine]) -> Option<usize>;
}

pub trait DecisionLog {
    /// Append one event. The shell runs nothing while the log cannot be
    /// written, because every model decision must be logged (ADR 0002).
    fn record(&mut self, event: Value) -> Result<(), String>;
}
