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
    /// What the person must know about how complete the result is. Shown
    /// with the plan, because a reference expands from it.
    pub notes: Vec<String>,
}

impl Output {
    pub fn paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.found.iter().map(|f| &f.path)
    }
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
    /// The model's raw text for a request. Nothing here is trusted.
    fn plan(&self, request: &str, retry: Option<&Retry>) -> Result<String, String>;
}

pub trait Confirm {
    /// Show the plan screen and ask whether to go on. `details` is the same
    /// screen with the real locations, which the person may ask to see.
    fn plan(&mut self, screen: &str, details: &str) -> bool;
    /// Ask `who`, an administrator, to confirm one root action. The panel
    /// always carries the real location.
    fn root_action(&mut self, panel: &str, who: &User) -> bool;
    /// Ask the person which of several matches they mean. `None` cancels.
    fn choose(&mut self, question: &str, options: &[String]) -> Option<usize>;
}

pub trait DecisionLog {
    /// Append one event. The shell runs nothing while the log cannot be
    /// written, because every model decision must be logged (ADR 0002).
    fn record(&mut self, event: Value) -> Result<(), String>;
}
