//! The deterministic capability broker (ADR 0002, ADR 0007, ADR 0011).
//!
//! An action is allowed only when both the session's capabilities and the
//! user's own permissions allow it: the intersection, never the union. Root is
//! decided here from the action table, never by the model, and a step that
//! fails the user's permissions is refused, not promoted to root.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;
use std::path::PathBuf;

use crate::action::{Access, Action, Capability, RootRule};
use crate::plan::{Plan, StepId};
use crate::ports::{Host, Output, User};

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
    PermissionDenied { access: Access, path: PathBuf },
    NotAdministrator,
    ProgramNotRegistered(String),
    ConfigKeyNotAllowed(String),
    DependencyRefused(StepId),
    CannotExpand(String),
    PreRunFailed(String),
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CapabilityNotGranted(c) => write!(f, "the session does not hold capability {c}"),
            Self::PermissionDenied { access, path } => {
                write!(
                    f,
                    "your permissions do not allow {access} on {}",
                    path.display()
                )
            }
            Self::NotAdministrator => {
                f.write_str("root authority needs an administrator, and you are not one")
            }
            Self::ProgramNotRegistered(p) => write!(f, "program {p:?} is not in the registry"),
            Self::ConfigKeyNotAllowed(k) => write!(f, "configuration key {k:?} is not accepted"),
            Self::DependencyRefused(id) => {
                write!(f, "step {id} did not pass, and this step depends on it")
            }
            Self::CannotExpand(why) => write!(f, "cannot expand the reference: {why}"),
            Self::PreRunFailed(why) => write!(f, "the read this step needed failed: {why}"),
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
}

/// Check every step of a plan that already passed the schema and semantic rules.
pub fn review(plan: &Plan, authority: &Authority, host: &dyn Host) -> Review {
    let referenced: HashSet<&StepId> = plan
        .steps
        .iter()
        .filter_map(|s| s.reference.as_ref())
        .collect();
    let mut done: HashMap<StepId, ReviewedStep> = HashMap::new();
    let mut steps = Vec::new();

    for index in plan.execution_order() {
        let step = &plan.steps[index];
        let mut reviewed = ReviewedStep {
            id: step.id.clone(),
            depends_on: step.depends_on.clone(),
            blocked: None,
            items: Vec::new(),
        };

        reviewed.blocked = step
            .depends_on
            .iter()
            .find(|d| !done[*d].passes())
            .map(|d| Refusal::DependencyRefused(d.clone()));

        if reviewed.blocked.is_none() {
            match expand(step.reference.as_ref(), &step.action, &done) {
                Ok(actions) => {
                    reviewed.items = actions
                        .into_iter()
                        .map(|action| action.map_paths(&|p| host.resolve(p)))
                        .map(|action| ReviewedAction {
                            verdict: judge(&action, step.elevate, authority, host),
                            action,
                            output: None,
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
    Review {
        summary: plan.summary.clone(),
        steps,
    }
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

fn expand(
    reference: Option<&StepId>,
    action: &Action,
    done: &HashMap<StepId, ReviewedStep>,
) -> Result<Vec<Action>, Refusal> {
    let Some(source) = reference else {
        return Ok(vec![action.clone()]);
    };
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
        .flat_map(|o| o.paths.iter().cloned())
        .collect();
    if paths.len() > MAX_EXPANDED_PATHS {
        return Err(Refusal::CannotExpand(format!(
            "{source} produced {} paths, the limit is {MAX_EXPANDED_PATHS}",
            paths.len()
        )));
    }
    if matches!(action, Action::Move { .. }) && paths.len() != 1 {
        return Err(Refusal::CannotExpand(format!(
            "fs.move needs exactly one path from {source}, got {}",
            paths.len()
        )));
    }
    Ok(paths
        .into_iter()
        .map(|p| action.with_referenced_path(p))
        .collect())
}

fn judge(action: &Action, elevate: bool, authority: &Authority, host: &dyn Host) -> Verdict {
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

impl Review {
    /// What the plan would do, from the broker's own data. The model's summary
    /// is shown only as a labelled claim, never as the description of an action.
    pub fn render(&self) -> String {
        let mut out = format!("The model says: {:?}\nWhat would run:\n", self.summary);
        for step in &self.steps {
            if let Some(why) = &step.blocked {
                out.push_str(&format!("  {} REFUSED: {why}\n", step.id));
            }
            if step.items.is_empty() && step.blocked.is_none() {
                out.push_str(&format!(
                    "  {} nothing to do: the reference produced no paths\n",
                    step.id
                ));
            }
            for item in &step.items {
                let mark = match &item.verdict {
                    Verdict::Allowed => "ok".to_owned(),
                    Verdict::NeedsRoot => "ROOT, held for your confirmation".to_owned(),
                    Verdict::Refused(why) => format!("REFUSED: {why}"),
                };
                out.push_str(&format!(
                    "  {} {}  [{mark}]\n",
                    step.id,
                    item.action.describe()
                ));
            }
        }
        out
    }
}
