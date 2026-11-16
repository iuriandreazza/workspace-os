//! Fakes for the tests: no model, no terminal, no real filesystem.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::action::{Access, Action};
use crate::ports::{Confirm, DecisionLog, Host, Output, Planner, Retry, User};

/// A filesystem in memory: files with sizes, and a set of paths the user may not touch.
#[derive(Default)]
pub struct FakeHost {
    pub files: HashMap<PathBuf, u64>,
    pub forbidden: Vec<(Access, PathBuf)>,
    /// Reads of these paths fail, as when the directory vanishes after the check.
    pub failing: Vec<PathBuf>,
    /// Attached to what a search returns, as when part of the tree was unreadable.
    pub note: Option<String>,
    pub executed: RefCell<Vec<String>>,
}

impl FakeHost {
    pub fn with_files(files: &[(&str, u64)]) -> Self {
        Self {
            files: files.iter().map(|(p, s)| (PathBuf::from(p), *s)).collect(),
            ..Self::default()
        }
    }

    pub fn with_note(mut self, note: &str) -> Self {
        self.note = Some(note.to_owned());
        self
    }

    pub fn failing(mut self, path: &str) -> Self {
        self.failing.push(PathBuf::from(path));
        self
    }

    pub fn forbid(mut self, access: Access, path: &str) -> Self {
        self.forbidden.push((access, PathBuf::from(path)));
        self
    }
}

impl Host for FakeHost {
    fn resolve(&self, path: &Path) -> PathBuf {
        path.to_path_buf()
    }

    fn allows(&self, access: Access, path: &Path) -> bool {
        !self
            .forbidden
            .iter()
            .any(|(a, p)| *a == access && p == path)
    }

    fn execute(&self, action: &Action) -> Result<Output, String> {
        self.executed.borrow_mut().push(action.describe());
        if let Action::FindLargest { path, .. } | Action::List { path } = action
            && self.failing.contains(path)
        {
            return Err("input/output error".to_owned());
        }
        match action {
            Action::FindLargest { path, count } => {
                let mut found: Vec<_> = self
                    .files
                    .iter()
                    .filter(|(p, _)| p.starts_with(path))
                    .collect();
                found.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
                let paths: Vec<PathBuf> = found
                    .into_iter()
                    .take(*count as usize)
                    .map(|(p, _)| p.clone())
                    .collect();
                Ok(Output {
                    text: format!("{} files", paths.len()),
                    paths,
                    notes: self.note.clone().into_iter().collect(),
                })
            }
            Action::List { path } => {
                let mut paths: Vec<PathBuf> = self
                    .files
                    .keys()
                    .filter(|p| p.starts_with(path))
                    .cloned()
                    .collect();
                paths.sort();
                Ok(Output {
                    text: String::new(),
                    paths,
                    ..Output::default()
                })
            }
            _ => Ok(Output::default()),
        }
    }
}

/// Answers a fixed list of raw model outputs, one per call, and keeps what it was asked.
pub struct ScriptedPlanner {
    pub outputs: RefCell<Vec<String>>,
    pub retries: RefCell<Vec<Option<String>>>,
}

impl ScriptedPlanner {
    pub fn new(outputs: &[&str]) -> Self {
        Self {
            outputs: RefCell::new(outputs.iter().rev().map(|s| (*s).to_owned()).collect()),
            retries: RefCell::default(),
        }
    }
}

impl Planner for ScriptedPlanner {
    fn plan(&self, _request: &str, retry: Option<&Retry>) -> Result<String, String> {
        self.retries
            .borrow_mut()
            .push(retry.map(|r| r.error.to_owned()));
        self.outputs
            .borrow_mut()
            .pop()
            .ok_or_else(|| "script exhausted".to_owned())
    }
}

pub struct Answers {
    pub plan: bool,
    pub root: bool,
    pub root_asked: Vec<String>,
}

impl Answers {
    pub fn yes() -> Self {
        Self {
            plan: true,
            root: true,
            root_asked: Vec::new(),
        }
    }
}

impl Confirm for Answers {
    fn plan(&mut self, _rendering: &str) -> bool {
        self.plan
    }

    fn root_action(&mut self, rendering: &str, _who: &User) -> bool {
        self.root_asked.push(rendering.to_owned());
        self.root
    }
}

#[derive(Default)]
pub struct MemoryLog {
    pub events: Vec<Value>,
    /// The log accepts this many events, then fails every one after.
    pub accept: Option<usize>,
}

impl MemoryLog {
    pub fn failing_after(accept: usize) -> Self {
        Self {
            events: Vec::new(),
            accept: Some(accept),
        }
    }
}

impl DecisionLog for MemoryLog {
    fn record(&mut self, event: Value) -> Result<(), String> {
        if self.accept.is_some_and(|n| self.events.len() >= n) {
            return Err("disk full".to_owned());
        }
        self.events.push(event);
        Ok(())
    }
}

pub fn user(administrator: bool) -> User {
    User {
        name: "ana".to_owned(),
        administrator,
    }
}
