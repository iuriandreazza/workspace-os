//! The contracts between the deterministic core and everything around it:
//! the machine it acts on, the model, the person and the log. The core only
//! sees these traits, so every test of the broker runs without a model, a
//! terminal or a real filesystem.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::action::{Access, Action};

/// Who the shell acts for. `administrator` means allowed to act as root
/// after confirming (ADR 0007).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub name: String,
    pub administrator: bool,
}

/// What an executed action produced. `paths` feeds `from_step` references.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Output {
    pub text: String,
    pub paths: Vec<PathBuf>,
}

pub trait Host {
    /// Turn what the model wrote (`~`, relative) into the path the action uses.
    fn resolve(&self, path: &Path) -> PathBuf;
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
    /// Show the broker's rendering of the whole plan and ask whether to go on.
    fn plan(&mut self, rendering: &str) -> bool;
    /// Ask `who`, an administrator, to confirm one root action, shown exactly.
    fn root_action(&mut self, rendering: &str, who: &User) -> bool;
}

pub trait DecisionLog {
    /// Append one event. The shell runs nothing while the log cannot be
    /// written, because every model decision must be logged (ADR 0002).
    fn record(&mut self, event: Value) -> Result<(), String>;
}
