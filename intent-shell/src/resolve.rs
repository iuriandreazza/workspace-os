//! The resolver: turns a reference into real locations (ADR 0014, ADR 0018).
//!
//! Deterministic code, run before the broker. It answers "what is this?" and
//! never "may you?": the broker is the only authority, so a resource the user
//! may not touch resolves like any other and is refused there. That holds for
//! a typed path too: `/etc`, a link out of the home or a folder the user
//! cannot enter resolve to the location they name, and the broker refuses
//! them. What the resolver rejects is only what names no location at all.

use std::collections::HashMap;
use std::fmt;
use std::path::{Component, Path, PathBuf};

use crate::action::quoted;
use crate::plan::StepId;
use crate::ports::{Entry, Host};
use crate::reference::{
    Destination, Direct, Kind, Place, Slot, Target, Within, is_path_text, is_single_name,
};

/// More matches than this are not offered: the person must say more.
pub const MAX_CANDIDATES: usize = 20;

/// The person's picks, by the step and position they answered for.
#[derive(Debug, Clone, Default)]
pub struct Choices(HashMap<(StepId, Slot), PathBuf>);

impl Choices {
    pub fn pick(&mut self, step: StepId, slot: Slot, location: PathBuf) {
        self.0.insert((step, slot), location);
    }

    fn get(&self, step: &StepId, slot: Slot) -> Option<&PathBuf> {
        self.0.get(&(step.clone(), slot))
    }
}

/// A reference that matched more than one thing. The person decides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ambiguity {
    pub step: StepId,
    pub slot: Slot,
    pub what: String,
    pub candidates: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    NotFound(String),
    Ambiguous(Ambiguity),
    TooMany {
        what: String,
        found: usize,
    },
    CannotCreateThere(String),
    /// A "new" item whose name is already taken.
    AlreadyExists(String),
    SearchFailed(String),
    /// Text that cannot be a path: empty, too long or with a NUL.
    BadPath(String),
    /// A `..` that would go above the root of the filesystem.
    EscapesRoot(String),
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(what) => write!(f, "nothing found for {what}"),
            Self::Ambiguous(a) => write!(f, "{} matches for {}", a.candidates.len(), a.what),
            Self::TooMany { what, found } => write!(
                f,
                "{found} matches for {what}, more than {MAX_CANDIDATES}: say more about it"
            ),
            Self::CannotCreateThere(what) => write!(f, "cannot create {what}: no single place"),
            Self::AlreadyExists(what) => write!(f, "{what} already exists"),
            Self::SearchFailed(why) => write!(f, "the search could not run: {why}"),
            Self::BadPath(what) => write!(f, "{what} cannot be a path"),
            Self::EscapesRoot(what) => write!(f, "{what} goes above the root of the filesystem"),
        }
    }
}

/// Real locations, and what the search could not look at.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Located {
    pub paths: Vec<PathBuf>,
    pub notes: Vec<String>,
}

/// What a reference is resolved against: the host and the session's directory.
pub struct Scope<'a> {
    pub host: &'a dyn Host,
    pub cwd: &'a Path,
}

/// The real location a typed path names (ADR 0018, decision 1): `~` and relative
/// paths are made absolute, `.` and `..` are cleaned lexically, and the host
/// resolves links. Nothing is checked about permission or existence.
pub fn resolve_path(scope: &Scope, text: &str, follow_last: bool) -> Result<PathBuf, ResolveError> {
    let what = format!("path {}", quoted(text));
    if !is_path_text(text) {
        return Err(ResolveError::BadPath(what));
    }
    let absolute = if text == "~" || text.starts_with("~/") {
        let home = scope
            .host
            .home()
            .ok_or_else(|| ResolveError::NotFound(what.clone()))?;
        home.join(text[1..].trim_start_matches('/'))
    } else if text.starts_with('/') {
        PathBuf::from(text)
    } else {
        scope.cwd.join(text)
    };
    let cleaned = clean(&absolute).ok_or(ResolveError::EscapesRoot(what))?;
    Ok(scope.host.canonicalize(&cleaned, follow_last))
}

/// `.` and `..` removed without asking the filesystem, or `None` when a `..`
/// would leave the root.
fn clean(path: &Path) -> Option<PathBuf> {
    let mut cleaned = PathBuf::from("/");
    for part in path.components() {
        match part {
            Component::Normal(name) => cleaned.push(name),
            Component::ParentDir => {
                if !cleaned.pop() {
                    return None;
                }
            }
            Component::RootDir | Component::CurDir | Component::Prefix(_) => {}
        }
    }
    Some(cleaned)
}

/// The folders a `within` stands for.
fn roots_of(scope: &Scope, within: &Within, what: &str) -> Result<Vec<PathBuf>, ResolveError> {
    let roots = match within {
        Within::Place(place) => scope.host.place_roots(*place),
        Within::Path(text) => vec![resolve_path(scope, text, true)?],
    };
    if roots.is_empty() {
        Err(ResolveError::NotFound(what.to_owned()))
    } else {
        Ok(roots)
    }
}

/// The locations a reference stands for. A `from_step` reference is not
/// handled here: its locations are what an earlier step produced, and only the
/// broker knows that.
pub fn locate(
    scope: &Scope,
    choices: &Choices,
    step: &StepId,
    slot: Slot,
    direct: &Direct,
    follow_last: bool,
) -> Result<Located, ResolveError> {
    let host = scope.host;
    let what = direct.describe();
    let found = |paths| Located {
        paths,
        notes: Vec::new(),
    };
    match direct {
        Direct::Place(place) => {
            let paths = host.place_roots(*place);
            if paths.is_empty() {
                return Err(ResolveError::NotFound(what));
            }
            Ok(found(paths))
        }
        Direct::Path(text) => Ok(found(vec![resolve_path(scope, text, follow_last)?])),
        Direct::New { name, within } => {
            let roots = roots_of(scope, within, &what)?;
            match (roots.as_slice(), is_single_name(name)) {
                ([root], true) if host.exists(&root.join(name)) => {
                    Err(ResolveError::AlreadyExists(what))
                }
                ([root], true) => Ok(found(vec![root.join(name)])),
                _ => Err(ResolveError::CannotCreateThere(what)),
            }
        }
        Direct::Named { name, within, kind } => {
            let roots = roots_of(scope, within, &what)?;
            find_by_name(host, choices, step, slot, &roots, name, *kind, what)
        }
        Direct::UnderHome { name, kind } => {
            let roots: Vec<PathBuf> = host.home().into_iter().collect();
            if roots.is_empty() {
                return Err(ResolveError::NotFound(what));
            }
            find_by_name(host, choices, step, slot, &roots, name, *kind, what)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn find_by_name(
    host: &dyn Host,
    choices: &Choices,
    step: &StepId,
    slot: Slot,
    roots: &[PathBuf],
    name: &str,
    kind: Kind,
    what: String,
) -> Result<Located, ResolveError> {
    if !is_single_name(name) {
        return Err(ResolveError::NotFound(what));
    }
    let search = host
        .search(roots, name)
        .map_err(ResolveError::SearchFailed)?;
    // A place is a container the plan may read, never a thing it may
    // remove or overwrite, so naming it does not find it.
    let entries = search
        .entries
        .into_iter()
        .filter(|e| !is_place_root(host, &e.path))
        .collect();
    let matches = best_matches(entries, name, kind);
    // A search that did not finish must not look like one that did.
    let what = if search.notes.is_empty() {
        what
    } else {
        format!("{what} (search incomplete: {})", search.notes.join("; "))
    };
    let path = match matches.as_slice() {
        [] => return Err(ResolveError::NotFound(what)),
        // A search that did not finish may have missed the better match.
        [only] if search.notes.is_empty() => only.clone(),
        many if many.len() > MAX_CANDIDATES => {
            return Err(ResolveError::TooMany {
                what,
                found: many.len(),
            });
        }
        many => match choices.get(step, slot).filter(|p| many.contains(p)) {
            Some(picked) => picked.clone(),
            None => {
                return Err(ResolveError::Ambiguous(Ambiguity {
                    step: step.clone(),
                    slot,
                    what,
                    candidates: many.to_vec(),
                }));
            }
        },
    };
    Ok(Located {
        paths: vec![path],
        notes: search.notes,
    })
}

fn file_name_lowercase(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase()
}

/// The whole name when something has it, otherwise names that contain it
/// (ADR 0014, decision 3). The host is asked for names that contain the
/// needle, and this does not take its word for it.
fn best_matches(entries: Vec<Entry>, needle: &str, kind: Kind) -> Vec<PathBuf> {
    let needle = needle.to_lowercase();
    let mut contains: Vec<PathBuf> = entries
        .into_iter()
        .filter(|e| match kind {
            Kind::File => !e.is_dir,
            Kind::Folder => e.is_dir,
            Kind::Any => true,
        })
        .map(|e| e.path)
        .filter(|p| file_name_lowercase(p).contains(&needle))
        .collect();
    contains.sort();
    contains.dedup();
    let exact: Vec<PathBuf> = contains
        .iter()
        .filter(|p| file_name_lowercase(p) == needle)
        .cloned()
        .collect();
    if exact.is_empty() { contains } else { exact }
}

/// Where a move puts what it moves.
pub fn destination(
    scope: &Scope,
    choices: &Choices,
    step: &StepId,
    direct: &Direct,
) -> Result<(Destination, Vec<String>), ResolveError> {
    let located = locate(scope, choices, step, Slot::Destination, direct, false)?;
    let [path] = located.paths.as_slice() else {
        return Err(ResolveError::CannotCreateThere(direct.describe()));
    };
    // A path that names a folder that exists is where the source goes; any
    // other is the exact new location.
    let into = match direct {
        Direct::Place(_) | Direct::Named { .. } | Direct::UnderHome { .. } => true,
        Direct::Path(_) => scope.host.is_dir(path),
        Direct::New { .. } => false,
    };
    let destination = if into {
        Destination::Into(path.clone())
    } else {
        Destination::Exact(path.clone())
    };
    Ok((destination, located.notes))
}

/// Whether `path` is the folder of some place, however its name is written.
pub fn is_place_root(host: &dyn Host, path: &Path) -> bool {
    Place::ALL
        .into_iter()
        .flat_map(|place| host.place_roots(place))
        .any(|root| host.same(&root, path))
}

/// A target that is not a step reference.
pub fn direct_of(target: &Target) -> Option<&Direct> {
    match target {
        Target::Direct(direct) => Some(direct),
        Target::FromStep(_) => None,
    }
}
