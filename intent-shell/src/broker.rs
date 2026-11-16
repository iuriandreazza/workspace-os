//! The deterministic capability broker (ADR 0002, ADR 0007, ADR 0011).
//!
//! An action is allowed only when both the session's capabilities and the
//! user's own permissions allow it: the intersection, never the union. Root is
//! decided here from the action table, never by the model, and a step that
//! fails the user's permissions is refused, not promoted to root.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

use crate::action::{Access, Action, Capability, RootRule, WriteMode, quoted_path};
use crate::label::Labels;
use crate::plan::{Plan, Step, StepId};
use crate::ports::{Host, Output, User};
use crate::reference::{Destination, Slot, Spec, Target};
use crate::resolve::{Ambiguity, Choices, ResolveError, destination, is_place_root, locate};

/// Most paths one `from_step` reference may expand to.
pub const MAX_EXPANDED_PATHS: usize = 100;

/// Programs `process.run` may start. Empty in S3: no program is vetted yet, so
/// every `process.run` is refused.
pub const PROGRAM_REGISTRY: &[&str] = &[];

/// Keys `system.configure` accepts. Empty in S3 for the same reason.
pub const CONFIG_KEYS: &[&str] = &[];

/// What the AI session may do, before the user's permissions narrow it.
pub struct Authority {
    pub user: User,
    pub capabilities: BTreeSet<Capability>,
}

impl Authority {
    /// Every capability, the starting point in ADR 0011.
    pub fn full(user: User) -> Self {
        Self {
            user,
            capabilities: Capability::ALL.into_iter().collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    CapabilityNotGranted(Capability),
    PermissionDenied {
        access: Access,
        path: PathBuf,
    },
    NotAdministrator,
    ProgramNotRegistered(String),
    ConfigKeyNotAllowed(String),
    ProtectedPath(PathBuf),
    MoveIntoItself(PathBuf),
    /// A move whose destination already holds something: it would be replaced.
    DestinationTaken(PathBuf),
    /// An earlier action of the same plan already removes, moves or replaces this location.
    ConflictsWithStep(StepId, PathBuf),
    /// A place is a container: it can be read, never removed, moved or overwritten whole.
    PlaceRoot(PathBuf),
    DependencyRefused(StepId),
    CannotExpand(String),
    PreRunFailed(String),
    /// A reference that did not resolve to exactly one resource (ADR 0014).
    Unresolved(ResolveError),
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CapabilityNotGranted(c) => write!(f, "the session does not hold capability {c}"),
            Self::PermissionDenied { access, path } => {
                write!(
                    f,
                    "your permissions do not allow {access} on {}",
                    quoted_path(path)
                )
            }
            Self::NotAdministrator => {
                f.write_str("root authority needs an administrator, and you are not one")
            }
            Self::ProgramNotRegistered(p) => write!(f, "program {p:?} is not in the registry"),
            Self::ProtectedPath(path) => write!(
                f,
                "{} cannot be written or removed, not even as root",
                quoted_path(path)
            ),
            Self::MoveIntoItself(path) => {
                write!(
                    f,
                    "{} cannot be moved onto or into itself",
                    quoted_path(path)
                )
            }
            Self::DestinationTaken(path) => write!(
                f,
                "{} already exists and would be replaced",
                quoted_path(path)
            ),
            Self::ConflictsWithStep(step, path) => write!(
                f,
                "step {step} already removes, moves or replaces {}",
                quoted_path(path)
            ),
            Self::PlaceRoot(path) => write!(
                f,
                "{} is a whole place and cannot be removed, moved or overwritten",
                quoted_path(path)
            ),
            Self::ConfigKeyNotAllowed(k) => write!(f, "configuration key {k:?} is not accepted"),
            Self::DependencyRefused(id) => {
                write!(f, "step {id} did not pass, and this step depends on it")
            }
            Self::CannotExpand(why) => write!(f, "cannot expand the reference: {why}"),
            Self::PreRunFailed(why) => write!(f, "the read this step needed failed: {why}"),
            Self::Unresolved(why) => why.fmt(f),
        }
    }
}

impl Refusal {
    /// The reason in friendly names. `Display` keeps the real locations, for the log.
    pub fn friendly(&self, labels: &Labels) -> String {
        match self {
            Self::PermissionDenied { access, path } => {
                format!(
                    "your permissions do not allow {access} on {}",
                    labels.of(path)
                )
            }
            Self::ProtectedPath(path) => format!(
                "{} cannot be written or removed, not even as root",
                labels.of(path)
            ),
            Self::MoveIntoItself(path) => {
                format!("{} cannot be moved onto or into itself", labels.of(path))
            }
            Self::DestinationTaken(path) => {
                format!("{} already exists and would be replaced", labels.of(path))
            }
            Self::ConflictsWithStep(step, path) => format!(
                "step {step} already removes, moves or replaces {}",
                labels.of(path)
            ),
            Self::PlaceRoot(path) => format!(
                "{} is a whole place and cannot be removed, moved or overwritten",
                labels.of(path)
            ),
            other => other.to_string(),
        }
    }

    /// The question the person must answer, when that is what is missing.
    pub fn ambiguity(&self) -> Option<&Ambiguity> {
        match self {
            Self::Unresolved(ResolveError::Ambiguous(a)) => Some(a),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Allowed,
    /// Held until a person allowed to act as root confirms this exact action.
    NeedsRoot,
    Refused(Refusal),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewedAction {
    pub action: Action,
    pub verdict: Verdict,
    /// Set when a read ran before confirmation to expand a later reference.
    pub output: Option<Result<Output, String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewedStep {
    pub id: StepId,
    pub depends_on: Vec<StepId>,
    /// What the step asks for in words, for the person when it has no action yet.
    pub asks: String,
    /// What the search that found the step's resources could not look at.
    pub notes: Vec<String>,
    /// Why the whole step passed no further, when it did not get as far as actions.
    pub blocked: Option<Refusal>,
    pub items: Vec<ReviewedAction>,
}

impl ReviewedStep {
    /// A step whose dependents may go on: nothing blocked, nothing refused.
    pub fn passes(&self) -> bool {
        self.blocked.is_none()
            && self
                .items
                .iter()
                .all(|i| !matches!(i.verdict, Verdict::Refused(_)))
    }

    fn pre_run(&self) -> bool {
        self.blocked.is_none() && self.items.iter().all(|i| i.output.is_some())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Review {
    pub summary: String,
    /// In execution order.
    pub steps: Vec<ReviewedStep>,
    /// Friendly names for every location the review mentions.
    pub labels: Labels,
}

impl Review {
    /// The first reference waiting for the person to choose, in execution order.
    pub fn first_ambiguity(&self) -> Option<&Ambiguity> {
        self.steps
            .iter()
            .find_map(|s| s.blocked.as_ref().and_then(Refusal::ambiguity))
    }

    /// Every location the review mentions, for labelling.
    fn locations(&self) -> Vec<&Path> {
        let mut found: Vec<&Path> = Vec::new();
        for step in &self.steps {
            if let Some(a) = step.blocked.as_ref().and_then(Refusal::ambiguity) {
                found.extend(a.candidates.iter().map(PathBuf::as_path));
            }
            for item in &step.items {
                found.extend(item.action.accesses().into_iter().map(|(_, p)| p));
                if let Some(Ok(output)) = &item.output {
                    found.extend(output.paths().map(PathBuf::as_path));
                }
            }
        }
        found
    }
}

/// Check every step of a plan that already passed the schema and semantic rules.
/// References are resolved here, before any action is judged.
pub fn review(plan: &Plan, authority: &Authority, host: &dyn Host, choices: &Choices) -> Review {
    let referenced: HashSet<&StepId> = plan
        .steps
        .iter()
        .filter_map(|s| s.spec.source_step())
        .collect();
    let mut done: HashMap<StepId, ReviewedStep> = HashMap::new();
    let mut steps = Vec::new();
    // Each step is resolved against the filesystem as it is now, so two steps
    // aimed at the same location would not see each other.
    let mut claimed: HashMap<PathBuf, StepId> = HashMap::new();

    for index in plan.execution_order() {
        let step = &plan.steps[index];
        let mut reviewed = ReviewedStep {
            id: step.id.clone(),
            depends_on: step.depends_on.clone(),
            asks: step.spec.describe(),
            notes: Vec::new(),
            blocked: None,
            items: Vec::new(),
        };

        reviewed.blocked = step
            .depends_on
            .iter()
            .find(|d| !done[*d].passes())
            .map(|d| Refusal::DependencyRefused(d.clone()));

        if reviewed.blocked.is_none() {
            match resolve_step(step, host, choices, &done) {
                Ok(resolved) => {
                    reviewed.notes = resolved.notes;
                    reviewed.items = resolved
                        .actions
                        .into_iter()
                        .map(|action| {
                            let mut verdict = judge(&action, step.elevate, authority, host);
                            // Creating and moving never replace what is already there.
                            let creates = match &action {
                                Action::Move { to, .. } => Some(to),
                                Action::Write {
                                    path,
                                    mode: WriteMode::Create,
                                    ..
                                } => Some(path),
                                _ => None,
                            };
                            if let Some(taken) = creates.filter(|p| host.exists(p))
                                && !matches!(verdict, Verdict::Refused(_))
                            {
                                verdict =
                                    Verdict::Refused(Refusal::DestinationTaken(taken.clone()));
                            }
                            if !matches!(verdict, Verdict::Refused(_)) {
                                for path in changed_locations(&action) {
                                    let earlier = claimed
                                        .iter()
                                        .find(|(taken, _)| host.same(taken, path))
                                        .map(|(_, step)| step);
                                    match earlier {
                                        Some(earlier) if *earlier != step.id => {
                                            verdict = Verdict::Refused(Refusal::ConflictsWithStep(
                                                earlier.clone(),
                                                path.to_path_buf(),
                                            ));
                                        }
                                        _ => {
                                            claimed.insert(path.to_path_buf(), step.id.clone());
                                        }
                                    }
                                }
                            }
                            ReviewedAction {
                                verdict,
                                action,
                                output: None,
                            }
                        })
                        .collect();
                }
                Err(refusal) => reviewed.blocked = Some(refusal),
            }
        }

        if reviewed.blocked.is_none() && referenced.contains(&step.id) {
            pre_run(
                &mut reviewed,
                step.depends_on.iter().all(|d| done[d].pre_run()),
                host,
            );
        }

        done.insert(step.id.clone(), reviewed.clone());
        steps.push(reviewed);
    }
    let mut review = Review {
        summary: plan.summary.clone(),
        steps,
        labels: Labels::default(),
    };
    review.labels = Labels::build(host, review.locations());
    review
}

/// Run the reads a later step refers to, so the person sees real paths when
/// asked to confirm (ADR 0011). Only plain reads run: never a held or
/// mutating step, and never one that waits on a step that has not run.
fn pre_run(step: &mut ReviewedStep, dependencies_ran: bool, host: &dyn Host) {
    let plain_reads = step
        .items
        .iter()
        .all(|i| i.action.is_read_only() && i.verdict == Verdict::Allowed);
    if !(plain_reads && dependencies_ran) {
        return;
    }
    for item in &mut step.items {
        item.output = Some(host.execute(&item.action));
    }
    if let Some(Err(why)) = step
        .items
        .iter()
        .find_map(|i| i.output.as_ref().filter(|o| o.is_err()))
    {
        step.blocked = Some(Refusal::PreRunFailed(why.clone()));
    }
}

struct Resolved {
    actions: Vec<Action>,
    notes: Vec<String>,
}

/// The concrete actions of a step: references resolved to real locations.
fn resolve_step(
    step: &Step,
    host: &dyn Host,
    choices: &Choices,
    done: &HashMap<StepId, ReviewedStep>,
) -> Result<Resolved, Refusal> {
    let mut notes = Vec::new();
    let mut sources: Vec<PathBuf> = Vec::new();
    let mut dest: Option<Destination> = None;
    for (slot, target) in step.spec.targets() {
        match (slot, target) {
            (_, Target::FromStep(source)) if slot == Slot::Primary => {
                sources = from_step(source, &step.spec, done)?;
            }
            (_, Target::FromStep(_)) => {
                return Err(Refusal::CannotExpand(
                    "a destination cannot be the result of a step".to_owned(),
                ));
            }
            (Slot::Primary, Target::Direct(direct)) => {
                let located =
                    locate(host, choices, &step.id, slot, direct).map_err(Refusal::Unresolved)?;
                sources = located.paths;
                notes.extend(located.notes);
            }
            (_, Target::Direct(direct)) => {
                let (found, more) =
                    destination(host, choices, &step.id, direct).map_err(Refusal::Unresolved)?;
                dest = Some(found);
                notes.extend(more);
            }
        }
    }
    // "The N largest" is one ranking: per-folder rankings would return more than N.
    if matches!(step.spec, Spec::FindLargest { .. }) && sources.len() > 1 {
        return Err(Refusal::CannotExpand(
            "the place lives in several folders, and the largest files of each would not be the largest overall"
                .to_owned(),
        ));
    }
    Ok(Resolved {
        actions: step.spec.instantiate(&sources, dest.as_ref()),
        notes,
    })
}

/// What an earlier step found, as the locations a later step acts on.
fn from_step(
    source: &StepId,
    spec: &Spec,
    done: &HashMap<StepId, ReviewedStep>,
) -> Result<Vec<PathBuf>, Refusal> {
    let producer = &done[source];
    if !producer.pre_run() {
        return Err(Refusal::CannotExpand(format!(
            "the paths of {source} are not known before confirmation"
        )));
    }
    let paths: Vec<PathBuf> = producer
        .items
        .iter()
        .filter_map(|i| i.output.as_ref().and_then(|o| o.as_ref().ok()))
        .flat_map(|o| o.paths().cloned())
        .collect();
    if paths.len() > MAX_EXPANDED_PATHS {
        return Err(Refusal::CannotExpand(format!(
            "{source} produced {} paths, the limit is {MAX_EXPANDED_PATHS}",
            paths.len()
        )));
    }
    if matches!(spec, Spec::Move { .. }) && paths.len() != 1 {
        return Err(Refusal::CannotExpand(format!(
            "fs.move needs exactly one path from {source}, got {}",
            paths.len()
        )));
    }
    Ok(paths)
}

/// The locations an action removes, moves away, creates or replaces.
fn changed_locations(action: &Action) -> Vec<&Path> {
    match action {
        Action::Delete { path } | Action::Write { path, .. } => vec![path],
        Action::Move { from, to } => vec![from, to],
        _ => Vec::new(),
    }
}

pub(crate) fn judge(
    action: &Action,
    elevate: bool,
    authority: &Authority,
    host: &dyn Host,
) -> Verdict {
    let capability = action.capability();
    if !authority.capabilities.contains(&capability) {
        return Verdict::Refused(Refusal::CapabilityNotGranted(capability));
    }
    match action {
        Action::Run { program, .. } if !PROGRAM_REGISTRY.contains(&program.as_str()) => {
            return Verdict::Refused(Refusal::ProgramNotRegistered(program.clone()));
        }
        Action::SystemConfigure { key, .. } if !CONFIG_KEYS.contains(&key.as_str()) => {
            return Verdict::Refused(Refusal::ConfigKeyNotAllowed(key.clone()));
        }
        _ => {}
    }
    // No authority, root included, makes the filesystem root a target.
    if let Some((_, path)) = action.accesses().into_iter().find(|(access, path)| {
        matches!(access, Access::Write | Access::Delete) && path.parent().is_none()
    }) {
        return Verdict::Refused(Refusal::ProtectedPath(path.to_path_buf()));
    }
    // However a path was reached (a name, a listing), a place is never a target.
    let removed_or_replaced = match action {
        Action::Delete { path } | Action::Write { path, .. } => Some(path),
        Action::Move { from, .. } => Some(from),
        _ => None,
    };
    if let Some(path) = removed_or_replaced.filter(|p| is_place_root(host, p)) {
        return Verdict::Refused(Refusal::PlaceRoot(path.clone()));
    }
    if let Action::Move { from, to } = action
        && to.starts_with(from)
    {
        return Verdict::Refused(Refusal::MoveIntoItself(from.clone()));
    }
    if action.root_rule() == RootRule::Always || elevate {
        return if authority.user.administrator {
            Verdict::NeedsRoot
        } else {
            Verdict::Refused(Refusal::NotAdministrator)
        };
    }
    for (access, path) in action.accesses() {
        if !host.allows(access, path) {
            return Verdict::Refused(Refusal::PermissionDenied {
                access,
                path: path.to_path_buf(),
            });
        }
    }
    Verdict::Allowed
}
