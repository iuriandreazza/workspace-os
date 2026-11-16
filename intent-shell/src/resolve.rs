//! The resolver: turns a reference into real locations (ADR 0014).
//!
//! Deterministic code, run before the broker. It answers "what is this?" and
//! never "may you?": the broker is the only authority, so a resource the user
//! may not touch resolves like any other and is refused there.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::plan::StepId;
use crate::ports::{Entry, Host};
use crate::reference::{Destination, Direct, Kind, Place, Slot, Target, is_single_name};

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
        }
    }
}

/// Real locations, and what the search could not look at.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Located {
    pub paths: Vec<PathBuf>,
    pub notes: Vec<String>,
}

/// The locations a reference stands for. A `from_step` reference is not
/// handled here: its locations are what an earlier step produced, and only the
/// broker knows that.
pub fn locate(
    host: &dyn Host,
    choices: &Choices,
    step: &StepId,
    slot: Slot,
    direct: &Direct,
) -> Result<Located, ResolveError> {
    let what = direct.describe();
    match direct {
        Direct::Place(place) => {
            let paths = host.place_roots(*place);
            if paths.is_empty() {
                return Err(ResolveError::NotFound(what));
            }
            Ok(Located {
                paths,
                notes: Vec::new(),
            })
        }
        Direct::New { name, within } => {
            let roots = host.place_roots(*within);
            match (roots.as_slice(), is_single_name(name)) {
                ([root], true) if host.exists(&root.join(name)) => {
                    Err(ResolveError::AlreadyExists(what))
                }
                ([root], true) => Ok(Located {
                    paths: vec![root.join(name)],
                    notes: Vec::new(),
                }),
                _ => Err(ResolveError::CannotCreateThere(what)),
            }
        }
        Direct::Named { name, within, kind } => {
            if !is_single_name(name) {
                return Err(ResolveError::NotFound(what));
            }
            let roots = host.place_roots(*within);
            if roots.is_empty() {
                return Err(ResolveError::NotFound(what));
            }
            let search = host
                .search(&roots, name)
                .map_err(ResolveError::SearchFailed)?;
            // A place is a container the plan may read, never a thing it may
            // remove or overwrite, so naming it does not find it.
            let places: Vec<PathBuf> = Place::ALL
                .into_iter()
                .flat_map(|place| host.place_roots(place))
                .collect();
            let entries = search
                .entries
                .into_iter()
                .filter(|e| !places.contains(&e.path))
                .collect();
            let matches = best_matches(entries, name, *kind);
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
    }
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
    host: &dyn Host,
    choices: &Choices,
    step: &StepId,
    direct: &Direct,
) -> Result<(Destination, Vec<String>), ResolveError> {
    let located = locate(host, choices, step, Slot::Destination, direct)?;
    let into = matches!(direct, Direct::Place(_) | Direct::Named { .. });
    let [path] = located.paths.as_slice() else {
        return Err(ResolveError::CannotCreateThere(direct.describe()));
    };
    let destination = if into {
        Destination::Into(path.clone())
    } else {
        Destination::Exact(path.clone())
    };
    Ok((destination, located.notes))
}

/// A target that is not a step reference.
pub fn direct_of(target: &Target) -> Option<&Direct> {
    match target {
        Target::Direct(direct) => Some(direct),
        Target::FromStep(_) => None,
    }
}
