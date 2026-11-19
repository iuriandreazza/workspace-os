//! Fakes for the tests: no model, no terminal, no real filesystem.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::action::{Access, Action};
use crate::ports::{
    ChoiceLine, Confirm, DecisionLog, Entry, EntryKind, Found, Host, Output, Planner, Retry,
    Search, Speaker, TREE_DEPTH, TreeEntry, User,
};
use crate::reference::Place;

/// A filesystem in memory: files with sizes, places, and a set of paths the user may not touch.
pub struct FakeHost {
    pub files: HashMap<PathBuf, u64>,
    /// Folders that exist, besides the ones the files are in.
    pub dirs: Vec<PathBuf>,
    pub places: HashMap<Place, Vec<PathBuf>>,
    pub home: Option<PathBuf>,
    /// Links: a path that leads somewhere else, as a symbolic link does.
    pub links: Vec<(PathBuf, PathBuf)>,
    pub forbidden: Vec<(Access, PathBuf)>,
    /// Reads of these paths fail, as when the directory vanishes after the check.
    pub failing: Vec<PathBuf>,
    /// Attached to what a search returns, as when part of the tree was unreadable.
    pub note: Option<String>,
    /// A name search fails, as when the disk does.
    pub search_fails: bool,
    pub executed: RefCell<Vec<String>>,
    pub searched: RefCell<Vec<String>>,
    /// What listing a folder returns, instead of the files below it.
    pub listings: HashMap<PathBuf, Vec<PathBuf>>,
    /// Pairs of locations that are the same thing, as a folder under two cases.
    pub aliases: Vec<(PathBuf, PathBuf)>,
    /// What a read returns, instead of a run of `x`.
    pub content: Option<String>,
}

impl Default for FakeHost {
    /// `downloads` is `/work`, `documents` is `/docs`, and the home is `/home/ana`.
    fn default() -> Self {
        let places = HashMap::from([
            (Place::Downloads, vec![PathBuf::from("/work")]),
            (Place::Documents, vec![PathBuf::from("/docs")]),
        ]);
        Self {
            files: HashMap::new(),
            dirs: Vec::new(),
            places,
            home: Some(PathBuf::from("/home/ana")),
            links: Vec::new(),
            forbidden: Vec::new(),
            failing: Vec::new(),
            note: None,
            search_fails: false,
            executed: RefCell::default(),
            searched: RefCell::default(),
            listings: HashMap::new(),
            aliases: Vec::new(),
            content: None,
        }
    }
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

    pub fn place(mut self, place: Place, root: &str) -> Self {
        self.places.insert(place, vec![PathBuf::from(root)]);
        self
    }

    pub fn listing(mut self, folder: &str, children: &[&str]) -> Self {
        self.listings.insert(
            PathBuf::from(folder),
            children.iter().map(PathBuf::from).collect(),
        );
        self
    }

    pub fn alias(mut self, a: &str, b: &str) -> Self {
        self.aliases.push((PathBuf::from(a), PathBuf::from(b)));
        self
    }

    pub fn with_home(mut self, home: &str) -> Self {
        self.home = Some(PathBuf::from(home));
        self
    }

    pub fn link(mut self, from: &str, to: &str) -> Self {
        self.links.push((PathBuf::from(from), PathBuf::from(to)));
        self
    }

    pub fn dir(mut self, path: &str) -> Self {
        self.dirs.push(PathBuf::from(path));
        self
    }

    fn holds_dir(&self, path: &Path) -> bool {
        self.dirs.iter().any(|d| d == path)
            || self.files.keys().any(|f| f != path && f.starts_with(path))
    }

    /// `path` with the first link on the way replaced by where it leads.
    fn through_links(&self, path: &Path) -> PathBuf {
        let mut path = path.to_path_buf();
        for _ in 0..8 {
            let Some((from, to)) = self.links.iter().find(|(from, _)| path.starts_with(from))
            else {
                break;
            };
            let rest = path
                .strip_prefix(from)
                .unwrap_or(Path::new(""))
                .to_path_buf();
            path = if rest.as_os_str().is_empty() {
                to.clone()
            } else {
                to.join(rest)
            };
        }
        path
    }

    /// What a listing of `path` shows down to the tree's depth.
    fn tree_below(&self, path: &Path) -> Vec<TreeEntry> {
        let mut entries: std::collections::BTreeMap<PathBuf, TreeEntry> = Default::default();
        let mut add = |entry_path: PathBuf, size: Option<u64>, kind: EntryKind| {
            entries.entry(entry_path.clone()).or_insert(TreeEntry {
                path: entry_path,
                kind,
                size,
                readable: true,
                protected: false,
                owner: None,
                created: None,
            });
        };
        let known: Vec<PathBuf> = self
            .files
            .keys()
            .cloned()
            .chain(self.dirs.clone())
            .collect();
        for known in known {
            let Ok(inside) = known.strip_prefix(path) else {
                continue;
            };
            let parts: Vec<_> = inside.components().collect();
            for depth in 1..=parts.len().min(TREE_DEPTH) {
                let here = parts[..depth]
                    .iter()
                    .fold(path.to_path_buf(), |p, c| p.join(c));
                if depth == parts.len() && self.files.contains_key(&known) {
                    add(here, self.files.get(&known).copied(), EntryKind::File);
                } else {
                    add(here, None, EntryKind::Dir);
                }
            }
        }
        entries.into_values().collect()
    }
}

impl Host for FakeHost {
    fn place_roots(&self, place: Place) -> Vec<PathBuf> {
        match (self.places.get(&place), place) {
            (Some(roots), _) => roots.clone(),
            (None, Place::Home) => self.home.clone().into_iter().collect(),
            (None, _) => Vec::new(),
        }
    }

    fn home(&self) -> Option<PathBuf> {
        self.home.clone()
    }

    fn canonicalize(&self, path: &Path, follow_last: bool) -> PathBuf {
        match (follow_last, path.parent(), path.file_name()) {
            (false, Some(parent), Some(last)) => self.through_links(parent).join(last),
            _ => self.through_links(path),
        }
    }

    fn is_symlink(&self, path: &Path) -> bool {
        self.links.iter().any(|(from, _)| from == path)
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.holds_dir(&self.through_links(path))
            || self.places.values().flatten().any(|root| root == path)
            || self.home.as_deref() == Some(path)
    }

    fn same(&self, a: &Path, b: &Path) -> bool {
        a == b
            || a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
            || self
                .aliases
                .iter()
                .any(|(x, y)| (x == a && y == b) || (x == b && y == a))
    }

    fn exists(&self, path: &Path) -> bool {
        self.files.contains_key(path)
            || self.links.iter().any(|(from, _)| from == path)
            || self.aliases.iter().any(|(a, b)| a == path || b == path)
            || Host::is_dir(self, path)
    }

    fn search(&self, roots: &[PathBuf], needle: &str) -> Result<Search, String> {
        self.searched.borrow_mut().push(needle.to_owned());
        if self.search_fails {
            return Err("disk error".to_owned());
        }
        let needle = needle.to_lowercase();
        let mut seen: Vec<PathBuf> = self.dirs.clone();
        seen.extend(self.files.keys().cloned());
        // One walk per root, so overlapping roots report an item once for each.
        let mut found: Vec<PathBuf> = Vec::new();
        for root in roots {
            for path in &seen {
                let inside = path.starts_with(root) && path != root;
                let named = path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().to_lowercase().contains(&needle));
                if inside && named {
                    found.push(path.clone());
                }
            }
        }
        // A search does not follow links, but it does come across them.
        for (link, _) in &self.links {
            let named = link
                .file_name()
                .is_some_and(|n| n.to_string_lossy().to_lowercase().contains(&needle));
            if named && roots.iter().any(|r| link.starts_with(r) && link != r) {
                found.push(link.clone());
            }
        }
        let entries = found
            .into_iter()
            .map(|path| Entry {
                is_dir: self.holds_dir(&path),
                path,
            })
            .collect();
        Ok(Search {
            entries,
            notes: self.note.clone().into_iter().collect(),
        })
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
                let found: Vec<Found> = found
                    .into_iter()
                    .take(*count as usize)
                    .map(|(p, s)| Found {
                        path: p.clone(),
                        size: Some(*s),
                    })
                    .collect();
                Ok(Output {
                    found,
                    notes: self.note.clone().into_iter().collect(),
                    ..Output::default()
                })
            }
            Action::List { path } => {
                let mut paths: Vec<PathBuf> = self
                    .files
                    .keys()
                    .filter(|p| p.starts_with(path))
                    .cloned()
                    .collect();
                if let Some(children) = self.listings.get(path) {
                    paths = children.clone();
                }
                paths.sort();
                Ok(Output {
                    found: paths
                        .into_iter()
                        .map(|path| Found { path, size: None })
                        .collect(),
                    tree: self.tree_below(path),
                    ..Output::default()
                })
            }
            Action::Read { .. } => Ok(Output {
                text: self.content.clone().unwrap_or_else(|| "x".repeat(5000)),
                ..Output::default()
            }),
            _ => Ok(Output::default()),
        }
    }
}

/// Answers a fixed list of raw model outputs, one per call, and keeps what it was asked.
pub struct ScriptedPlanner {
    pub outputs: RefCell<Vec<String>>,
    pub retries: RefCell<Vec<Option<String>>>,
    /// The current directory the model was told, one per call.
    pub directories: RefCell<Vec<String>>,
}

impl ScriptedPlanner {
    pub fn new(outputs: &[&str]) -> Self {
        Self {
            outputs: RefCell::new(outputs.iter().rev().map(|s| (*s).to_owned()).collect()),
            retries: RefCell::default(),
            directories: RefCell::default(),
        }
    }
}

impl Planner for ScriptedPlanner {
    fn plan(&self, _request: &str, cwd: &str, retry: Option<&Retry>) -> Result<String, String> {
        self.directories.borrow_mut().push(cwd.to_owned());
        self.retries
            .borrow_mut()
            .push(retry.map(|r| r.error.to_owned()));
        self.outputs
            .borrow_mut()
            .pop()
            .ok_or_else(|| "script exhausted".to_owned())
    }
}

/// Answers every reply request with the same result and keeps what it was asked.
pub struct ScriptedSpeaker {
    pub answer: Result<String, String>,
    /// The system prompt and the facts of each call.
    pub asked: RefCell<Vec<(String, String)>>,
}

impl ScriptedSpeaker {
    pub fn says(text: &str) -> Self {
        Self {
            answer: Ok(text.to_owned()),
            asked: RefCell::default(),
        }
    }
}

impl Speaker for ScriptedSpeaker {
    fn reply(&self, system: &str, facts: &str) -> Result<String, String> {
        self.asked
            .borrow_mut()
            .push((system.to_owned(), facts.to_owned()));
        self.answer.clone()
    }
}

pub struct Answers {
    pub plan: bool,
    pub root: bool,
    pub root_asked: Vec<String>,
    pub screens: Vec<(String, String)>,
    /// What the person picks at each question, in order; running out means cancelling.
    pub picks: Vec<Option<usize>>,
    pub questions: Vec<(String, Vec<ChoiceLine>)>,
}

impl Answers {
    pub fn yes() -> Self {
        Self {
            plan: true,
            root: true,
            root_asked: Vec::new(),
            screens: Vec::new(),
            picks: Vec::new(),
            questions: Vec::new(),
        }
    }

    pub fn picking(mut self, picks: &[Option<usize>]) -> Self {
        self.picks = picks.iter().rev().copied().collect();
        self
    }
}

impl Confirm for Answers {
    fn plan(&mut self, screen: &str, details: &str) -> bool {
        self.screens.push((screen.to_owned(), details.to_owned()));
        self.plan
    }

    fn root_action(&mut self, panel: &str, _who: &User) -> bool {
        self.root_asked.push(panel.to_owned());
        self.root
    }

    fn choose(&mut self, question: &str, lines: &[ChoiceLine]) -> Option<usize> {
        self.questions.push((question.to_owned(), lines.to_vec()));
        self.picks.pop().flatten()
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
