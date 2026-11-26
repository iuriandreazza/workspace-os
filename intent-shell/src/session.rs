//! One request through the whole pipeline: model, checks, broker,
//! confirmation, execution, with every decision logged. The current directory
//! is session state kept here, in the core, so every adapter and the
//! deterministic shell behave the same way (ADR 0018).

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::action::{Access, Action, is_hidden, printable, quoted, quoted_path};
use crate::broker::{Authority, Refusal, Review, ReviewedAction, ReviewedStep, Verdict, review};
use crate::command::{self, Parsed};
use crate::label::Labels;
use crate::plan::{Document, Origin, Plan, StepId, parse_checked};
use crate::ports::{
    Confirm, DecisionLog, EntryKind, Host, Output, Planner, Retry, Search, SoulStore, TreeEntry,
};
use crate::reference::Place;
use crate::resolve::Choices;
use crate::screen::{self, TreeMarks, TreeOptions, View};
use crate::soul_change::{self, Approval, Failure, history_text};

/// Attempts at a valid document: the first, and one retry with the error (ADR 0011).
const MAX_ATTEMPTS: usize = 2;
const LOGGED_TEXT_LIMIT: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemResult {
    Done(Output),
    Failed(String),
    NotRun(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutedItem {
    pub action: Action,
    pub result: ItemResult,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutedStep {
    pub id: StepId,
    /// Set when the step did not reach its actions.
    pub not_run: Option<String>,
    pub items: Vec<ExecutedItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The model asked a question. Nothing ran.
    Clarify(String),
    /// No valid document after the attempts, or the model was unreachable. Nothing ran.
    NoPlan(String),
    /// A typed command the shell could not turn into a plan. Nothing ran.
    BadCommand(String),
    /// The text of `help` in the deterministic shell. Nothing ran.
    Help(String),
    /// Nothing in the plan passed the broker. Nothing ran.
    AllRefused(Review),
    /// The person said no. Nothing ran.
    Declined(Review),
    Executed {
        review: Review,
        /// Whether the person confirmed the plan. When not (ADR 0016) the
        /// result carries the plan, which no question showed.
        asked: bool,
        steps: Vec<ExecutedStep>,
        labels: Labels,
    },
    /// The log could not be written, so the shell stopped before the next action.
    LogFailed {
        reason: String,
        /// The plan screen, when the plan ran without a question and so was
        /// never shown (ADR 0016).
        plan: Option<String>,
        steps: Vec<ExecutedStep>,
        labels: Labels,
    },
}

/// How the lines of a session are read (ADR 0022).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    /// Natural language, for the model.
    Request,
    /// The deterministic shell: no model, commands only.
    Command,
}

impl Entry {
    /// The one place that decides: a session with a model reads requests,
    /// one without reads commands.
    pub fn for_model(has_model: bool) -> Self {
        if has_model {
            Self::Request
        } else {
            Self::Command
        }
    }
}

/// What a session remembers between requests. One per session: a `cd` in one
/// never reaches another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionState {
    /// A real location, where relative paths in the next request start.
    pub cwd: PathBuf,
}

impl SessionState {
    /// A session starts at the person's home, or at the root when the host has none.
    pub fn start(host: &dyn Host) -> Self {
        Self {
            cwd: host.home().unwrap_or_else(|| PathBuf::from("/")),
        }
    }

    /// The directory as the person writes it (`~/Downloads`).
    pub fn cwd_text(&self, host: &dyn Host) -> String {
        printable(
            &Labels::new(&self.cwd, host.home())
                .cwd_text()
                .to_string_lossy(),
            false,
        )
    }

    /// The directory as the model is told: the way the person writes it and
    /// the real location, so it neither guesses a home nor spells a relative
    /// path from the wrong place.
    pub fn cwd_for_model(&self, host: &dyn Host) -> String {
        let written = self.cwd_text(host);
        let real = printable(&self.cwd.to_string_lossy(), false);
        if written == real {
            real
        } else {
            format!("{written} ({real})")
        }
    }
}

pub struct Shell<'a> {
    pub session: &'a mut SessionState,
    pub authority: &'a Authority,
    pub host: &'a dyn Host,
    pub planner: &'a dyn Planner,
    pub confirm: &'a mut dyn Confirm,
    pub log: &'a mut dyn DecisionLog,
}

impl Shell<'_> {
    pub fn handle(&mut self, request: &str) -> Outcome {
        self.run(request).unwrap_or_else(log_failed)
    }

    /// Read the line as the session's entry says: a request to the model or a command.
    pub fn handle_as(&mut self, entry: Entry, line: &str) -> Outcome {
        match entry {
            Entry::Request => self.handle(line),
            Entry::Command => self.handle_command(line),
        }
    }

    /// The deterministic shell: the line is a command, not a request, and the
    /// planner is never asked (ADR 0022). From the plan document on it is the
    /// same pipeline as `handle`.
    pub fn handle_command(&mut self, line: &str) -> Outcome {
        self.run_command(line).unwrap_or_else(log_failed)
    }

    fn record(&mut self, event: Value) -> Result<(), String> {
        self.log.record(event)
    }

    fn run(&mut self, request: &str) -> Result<Outcome, String> {
        let cwd = self.session.cwd.clone();
        self.record(
            json!({"event": "request", "user": self.authority.user.name, "request": request, "cwd": quoted_path(&cwd)}),
        )?;
        let document = match self.obtain_document(request)? {
            Ok(document) => document,
            Err(failure) => return Ok(failure),
        };
        match document {
            Document::Clarify(question) => {
                self.record(json!({"event": "clarify", "question": question}))?;
                Ok(Outcome::Clarify(question))
            }
            Document::Plan(plan) => self.carry_out(&plan),
        }
    }

    /// Turn the line into a plan document, record it, and carry it out. A line
    /// that is not a command is refused here and runs nothing.
    fn run_command(&mut self, line: &str) -> Result<Outcome, String> {
        let user = self.authority.user.name.clone();
        let cwd = quoted_path(&self.session.cwd);
        let json = match command::parse(line) {
            Ok(Parsed::Help(text)) => {
                self.record(json!({"event": "command", "user": user, "line": line, "cwd": cwd, "help": true}))?;
                return Ok(Outcome::Help(text));
            }
            Ok(Parsed::Plan(json)) => json,
            Err(error) => {
                self.record(json!({"event": "command", "user": user, "line": line, "cwd": cwd, "accepted": false, "error": error.to_string()}))?;
                return Ok(Outcome::BadCommand(error.to_string()));
            }
        };
        // The same structure and rules as the model's output, nothing more lenient.
        let checked: Result<Plan, String> = parse_checked(&json)
            .map_err(|e| e.to_string())
            .and_then(|document| match document {
                Document::Plan(plan) => Ok(plan),
                Document::Clarify(_) => Err("a command makes a plan".to_owned()),
            });
        match checked {
            Ok(mut plan) => {
                self.record(json!({"event": "command", "user": user, "line": line, "cwd": cwd, "accepted": true, "plan": json}))?;
                plan.origin = Origin::Typed;
                self.carry_out(&plan)
            }
            Err(error) => {
                self.record(json!({"event": "command", "user": user, "line": line, "cwd": cwd, "accepted": false, "plan": json, "error": error}))?;
                Ok(Outcome::BadCommand(error))
            }
        }
    }

    /// An accepted plan, from either entry: resolution, the broker, the
    /// question when ADR 0016 asks, execution, and the log of all of it.
    fn carry_out(&mut self, plan: &Plan) -> Result<Outcome, String> {
        // Before the broker may read anything to expand a reference, the plan
        // is on record, and each read is logged just before it runs.
        self.record(json!({"event": "plan_accepted", "summary": plan.summary, "steps": plan.steps.iter().map(|s| s.id.as_str()).collect::<Vec<_>>()}))?;
        let cwd = self.session.cwd.clone();
        let reviewed = match self.review_with_choices(plan, &cwd)? {
            Ok(reviewed) => reviewed,
            Err(declined) => return Ok(declined),
        };
        self.record(json!({"event": "review", "review": review_json(&reviewed)}))?;

        let anything_runs = reviewed
            .steps
            .iter()
            .filter(|s| s.blocked.is_none())
            .flat_map(|s| &s.items)
            .any(|i| !matches!(i.verdict, Verdict::Refused(_)));
        if !anything_runs {
            return Ok(Outcome::AllRefused(reviewed));
        }
        // ADR 0016: a plan that only reads or adds runs without the question.
        // The decision is on record before anything runs, like a confirmation.
        let asked = needs_confirmation(&reviewed);
        if asked {
            let confirmed = self.confirm.plan(
                &screen::plan_screen(&reviewed, View::Friendly),
                &screen::plan_screen(&reviewed, View::Details),
            );
            self.record(json!({"event": "plan_confirmation", "confirmed": confirmed}))?;
            if !confirmed {
                return Ok(Outcome::Declined(reviewed));
            }
        } else {
            self.record(json!({"event": "plan_auto_approved"}))?;
        }
        let (steps, logged) = self.execute(&reviewed);
        let labels = reviewed.labels.clone();
        Ok(match logged {
            Ok(()) => Outcome::Executed {
                review: reviewed,
                asked,
                steps,
                labels,
            },
            Err(reason) => Outcome::LogFailed {
                reason,
                plan: (!asked).then(|| screen::plan_screen(&reviewed, View::Friendly)),
                steps,
                labels,
            },
        })
    }

    /// Review the plan, asking the person about every reference that matched
    /// several things until none is left. The pick is the person's, is logged,
    /// never goes back to the model, and its resource is judged by the broker
    /// like any other. The inner `Err` is the outcome when the person cancels.
    fn review_with_choices(
        &mut self,
        plan: &Plan,
        cwd: &Path,
    ) -> Result<Result<Review, Outcome>, String> {
        let mut choices = Choices::default();
        loop {
            let reviewed = {
                let log = RefCell::new(&mut *self.log);
                review(
                    plan,
                    self.authority,
                    &LoggedReads {
                        inner: self.host,
                        log: &log,
                    },
                    cwd,
                    &choices,
                )
            };
            let Some(ambiguity) = reviewed.first_ambiguity().cloned() else {
                return Ok(Ok(reviewed));
            };
            let (question, lines, offered) = screen::question(&ambiguity, &reviewed.labels);
            let picked = self.confirm.choose(&question, &lines);
            let chosen = picked.and_then(|i| offered.get(i).cloned());
            self.record(json!({
                "event": "choice", "step": ambiguity.step.as_str(), "what": ambiguity.what,
                "candidates": ambiguity.candidates.iter().map(|p| quoted_path(p)).collect::<Vec<_>>(),
                "chosen": chosen.as_deref().map(quoted_path),
            }))?;
            match chosen {
                Some(location) => choices.pick(ambiguity.step, ambiguity.slot, location),
                None => return Ok(Err(Outcome::Declined(reviewed))),
            }
        }
    }

    /// Ask the model until a document passes the schema and semantic rules, or give up.
    /// The inner `Err` is an outcome that ends the request, not a log failure.
    fn obtain_document(&mut self, request: &str) -> Result<Result<Document, Outcome>, String> {
        let mut previous: Option<(String, String)> = None;
        for attempt in 1..=MAX_ATTEMPTS {
            let retry = previous.as_ref().map(|(output, error)| Retry {
                previous_output: output,
                error,
            });
            let cwd = self.session.cwd_for_model(self.host);
            let raw = match self.planner.plan(request, &cwd, retry.as_ref()) {
                Ok(raw) => raw,
                Err(reason) => {
                    self.record(
                        json!({"event": "model_error", "attempt": attempt, "error": reason}),
                    )?;
                    return Ok(Err(Outcome::NoPlan(format!(
                        "the model could not be reached: {reason}"
                    ))));
                }
            };
            match parse_checked(&raw) {
                Ok(document) => {
                    self.record(json!({"event": "model_output", "attempt": attempt, "raw": raw, "accepted": true}))?;
                    return Ok(Ok(document));
                }
                Err(error) => {
                    self.record(json!({"event": "model_output", "attempt": attempt, "raw": raw, "accepted": false, "error": error.to_string()}))?;
                    previous = Some((raw, error.to_string()));
                }
            }
        }
        let last = previous.map(|(_, error)| error).unwrap_or_default();
        Ok(Err(Outcome::NoPlan(format!(
            "no valid plan after {MAX_ATTEMPTS} attempts, last error: {last}"
        ))))
    }

    /// Run the confirmed plan in order. The steps done so far come back even
    /// when the log fails, so the person sees what happened before it stopped.
    fn execute(&mut self, reviewed: &Review) -> (Vec<ExecutedStep>, Result<(), String>) {
        let mut executed = Vec::new();
        let mut succeeded: HashSet<&StepId> = HashSet::new();
        for step in &reviewed.steps {
            let (record, outcome) = self.run_step(step, &succeeded, &reviewed.labels);
            executed.push(record);
            match outcome {
                Ok(true) => {
                    succeeded.insert(&step.id);
                }
                Ok(false) => {}
                Err(reason) => return (executed, Err(reason)),
            }
        }
        (executed, Ok(()))
    }

    /// The step's record, and whether every action of it completed.
    fn run_step(
        &mut self,
        step: &ReviewedStep,
        succeeded: &HashSet<&StepId>,
        labels: &Labels,
    ) -> (ExecutedStep, Result<bool, String>) {
        let mut record = ExecutedStep {
            id: step.id.clone(),
            not_run: None,
            items: Vec::new(),
        };
        record.not_run = match (
            &step.blocked,
            step.depends_on.iter().find(|d| !succeeded.contains(d)),
        ) {
            (Some(why), _) => Some(why.friendly(labels)),
            (None, Some(dependency)) => Some(format!("step {dependency} did not complete")),
            (None, None) => None,
        };
        if record.not_run.is_some() {
            let logged = self.record(json!({"event": "step_not_run", "step": step.id.as_str(), "reason": record.not_run}));
            return (record, logged.map(|()| false));
        }
        let mut complete = true;
        for item in &step.items {
            match self.run_item(&step.id, item, labels, &mut record) {
                Ok(done) => complete &= done,
                Err(reason) => return (record, Err(reason)),
            }
        }
        (record, Ok(complete))
    }

    /// Whether the action completed. The result is also added to `record`.
    fn run_item(
        &mut self,
        step: &StepId,
        item: &ReviewedAction,
        labels: &Labels,
        record: &mut ExecutedStep,
    ) -> Result<bool, String> {
        let rendering = item.action.describe();
        let entering = match &item.action {
            Action::Cd { path } => Some(path),
            _ => None,
        };
        let result = match (&item.verdict, entering) {
            (Verdict::Refused(why), _) => ItemResult::NotRun(why.friendly(labels)),
            (Verdict::NeedsRoot, _) if item.action.changes_soul() => {
                self.change_soul(step, &item.action)?
            }
            (Verdict::NeedsRoot, _) if !self.confirm_root(step, &item.action, labels)? => {
                ItemResult::NotRun("root confirmation was not given".to_owned())
            }
            // The directory was judged at review; what the person waited for
            // may have changed since, and a session never enters what is not one.
            (Verdict::Allowed, Some(path))
                if !self.host.is_dir(path) || !self.host.allows(Access::Read, path) =>
            {
                ItemResult::NotRun(format!(
                    "{} is no longer a directory you can enter",
                    labels.of(path)
                ))
            }
            (Verdict::Allowed, Some(path)) => {
                self.record(json!({"event": "cd", "step": step.as_str(), "from": quoted_path(&self.session.cwd), "to": quoted_path(path)}))?;
                self.session.cwd = path.clone();
                ItemResult::Done(Output {
                    text: format!("now in {}", self.session.cwd_text(self.host)),
                    ..Output::default()
                })
            }
            (Verdict::Allowed, _) if item.action == Action::SoulHistory => {
                self.record(
                    json!({"event": "execute", "step": step.as_str(), "action": rendering}),
                )?;
                match self.soul_store().and_then(|store| store.history_listing()) {
                    Ok((entries, problem)) => ItemResult::Done(Output {
                        text: match problem {
                            Some(problem) => {
                                format!(
                                    "{}\nWARNING: the history is damaged:\n{problem}",
                                    history_text(&entries)
                                )
                            }
                            None => history_text(&entries),
                        },
                        ..Output::default()
                    }),
                    Err(why) => ItemResult::Failed(why),
                }
            }
            (Verdict::Allowed | Verdict::NeedsRoot, _) => {
                let event = if item.output.is_some() {
                    "use_pre_run"
                } else {
                    "execute"
                };
                self.record(json!({"event": event, "step": step.as_str(), "action": rendering}))?;
                match item
                    .output
                    .clone()
                    .unwrap_or_else(|| self.host.execute(&item.action))
                {
                    Ok(output) => ItemResult::Done(output),
                    Err(why) => ItemResult::Failed(why),
                }
            }
        };
        let (done, detail) = match &result {
            ItemResult::Done(output) => (true, output.text.as_str()),
            ItemResult::Failed(why) | ItemResult::NotRun(why) => (false, why.as_str()),
        };
        let logged = json!({
            "event": "result", "step": step.as_str(), "action": rendering, "ok": done,
            "detail": detail.chars().take(LOGGED_TEXT_LIMIT).collect::<String>(),
        });
        record.items.push(ExecutedItem {
            action: item.action.clone(),
            result,
        });
        self.record(logged)?;
        Ok(done)
    }

    fn soul_store(&self) -> Result<&dyn SoulStore, String> {
        self.host
            .soul_store()
            .ok_or_else(|| "this host keeps no SOUL to change".to_owned())
    }

    /// A change or an undo of the SOUL (ADR 0036): built from the file as it is
    /// now, shown whole to the administrator, and written only on their yes, with
    /// the base checked again at the write. Every end of it is a `soul_change` event.
    fn change_soul(&mut self, step: &StepId, action: &Action) -> Result<ItemResult, String> {
        let who = self.authority.user.clone();
        let (kind, prepared) = match (self.soul_store(), action) {
            (Err(why), _) => return Ok(ItemResult::Failed(why)),
            (
                Ok(store),
                Action::SoulProposeChange {
                    rationale,
                    new_text,
                },
            ) => (
                "change",
                soul_change::prepare_change(store, rationale, new_text),
            ),
            (Ok(store), _) => ("undo", soul_change::prepare_undo(store)),
        };
        let ended = |outcome: &str, detail: &str| json!({"event": "soul_change", "step": step.as_str(), "kind": kind, "by": who.name, "outcome": outcome, "detail": detail});
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(Failure { outcome, message }) => {
                self.record(ended(outcome, &message))?;
                return Ok(ItemResult::Failed(message));
            }
        };
        let panel = screen::soul_panel(&who.name, step, &prepared);
        let confirmed = self.confirm.root_action(&panel, &who);
        self.record(json!({"event": "root_confirmation", "step": step.as_str(), "action": action.describe(), "confirmed_by": who.name, "confirmed": confirmed}))?;
        if !confirmed {
            self.record(ended("declined", ""))?;
            return Ok(ItemResult::NotRun(
                "root confirmation was not given".to_owned(),
            ));
        }
        let Some(approval) = Approval::from_root(&who) else {
            self.record(ended("not_administrator", ""))?;
            return Ok(ItemResult::NotRun(Refusal::NotAdministrator.to_string()));
        };
        // On record before the write: the shell changes nothing while the log cannot be written.
        self.record(json!({"event": "soul_change", "step": step.as_str(), "kind": kind, "by": who.name, "outcome": "writing", "written": prepared.change.after}))?;
        let applied = self
            .soul_store()
            .map(|store| store.apply(&prepared.change, &approval));
        match applied {
            Ok(Ok(seq)) => {
                // The SOUL is changed whatever happens to the log now; the result
                // event that follows reports a log that fails, with this step done.
                let _ = self.record(json!({"event": "soul_change", "step": step.as_str(), "kind": kind, "by": who.name, "outcome": "applied", "seq": seq}));
                Ok(ItemResult::Done(Output {
                    text: format!(
                        "The SOUL was changed: history entry {seq}. The new SOUL is read at the next start."
                    ),
                    ..Output::default()
                }))
            }
            Ok(Err(error)) => {
                self.record(ended(error.outcome(), &error.message()))?;
                Ok(ItemResult::Failed(error.message()))
            }
            Err(why) => Ok(ItemResult::Failed(why)),
        }
    }

    fn confirm_root(
        &mut self,
        step: &StepId,
        action: &Action,
        labels: &Labels,
    ) -> Result<bool, String> {
        let who = self.authority.user.clone();
        let rendering = action.describe();
        let panel = screen::root_panel(&who.name, step, action, labels);
        let confirmed = self.confirm.root_action(&panel, &who);
        self.record(json!({"event": "root_confirmation", "step": step.as_str(), "action": rendering, "confirmed_by": who.name, "confirmed": confirmed}))?;
        Ok(confirmed)
    }
}

fn log_failed(reason: String) -> Outcome {
    Outcome::LogFailed {
        reason,
        plan: None,
        steps: Vec::new(),
        labels: Labels::default(),
    }
}

/// The host as the broker sees it while expanding references: every read it
/// runs is logged first, and refused when the log cannot be written.
struct LoggedReads<'a, 'b, 'c> {
    inner: &'a dyn Host,
    log: &'a RefCell<&'b mut (dyn DecisionLog + 'c)>,
}

impl Host for LoggedReads<'_, '_, '_> {
    fn place_roots(&self, place: Place) -> Vec<PathBuf> {
        self.inner.place_roots(place)
    }

    fn home(&self) -> Option<PathBuf> {
        self.inner.home()
    }

    fn canonicalize(&self, path: &Path, follow_last: bool) -> PathBuf {
        self.inner.canonicalize(path, follow_last)
    }

    fn is_symlink(&self, path: &Path) -> bool {
        self.inner.is_symlink(path)
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.inner.is_dir(path)
    }

    fn same(&self, a: &Path, b: &Path) -> bool {
        self.inner.same(a, b)
    }

    fn exists(&self, path: &Path) -> bool {
        self.inner.exists(path)
    }

    fn search(&self, roots: &[PathBuf], needle: &str) -> Result<Search, String> {
        self.log
            .borrow_mut()
            .record(json!({"event": "search", "needle": needle, "in": roots.iter().map(|p| quoted_path(p)).collect::<Vec<_>>()}))
            .map_err(|e| format!("the decision log cannot be written: {e}"))?;
        self.inner.search(roots, needle)
    }

    fn allows(&self, access: Access, path: &Path) -> bool {
        self.inner.allows(access, path)
    }

    fn execute(&self, action: &Action) -> Result<Output, String> {
        self.log
            .borrow_mut()
            .record(json!({"event": "pre_run", "action": action.describe()}))
            .map_err(|e| format!("the decision log cannot be written: {e}"))?;
        self.inner.execute(action)
    }
}

fn review_json(reviewed: &Review) -> Value {
    let steps: Vec<Value> = reviewed
        .steps
        .iter()
        .map(|step| {
            let items: Vec<Value> = step
                .items
                .iter()
                .map(|item| {
                    let verdict = match &item.verdict {
                        Verdict::Allowed => "allowed".to_owned(),
                        Verdict::NeedsRoot => "needs_root".to_owned(),
                        Verdict::Refused(why) => format!("refused: {why}"),
                    };
                    let pre_run = item.output.as_ref().map(|o| match o {
                        Ok(_) => "ok".to_owned(),
                        Err(why) => format!("failed: {why}"),
                    });
                    json!({"action": item.action.describe(), "verdict": verdict, "pre_run": pre_run})
                })
                .collect();
            json!({"step": step.id.as_str(), "blocked": step.blocked.as_ref().map(ToString::to_string), "items": items})
        })
        .collect();
    json!({"summary": reviewed.summary, "cwd": quoted_path(&reviewed.cwd), "steps": steps})
}

/// Whether the person must confirm this plan: any action that could run and
/// changes or removes something, or is held for root (ADR 0016, ADR 0007).
fn needs_confirmation(review: &Review) -> bool {
    review
        .steps
        .iter()
        .filter(|step| step.blocked.is_none())
        .flat_map(|step| &step.items)
        .any(|item| match &item.verdict {
            Verdict::Refused(_) => false,
            Verdict::NeedsRoot => true,
            Verdict::Allowed => item.action.needs_confirmation(),
        })
}

impl Outcome {
    /// What to tell the person after the request ended.
    pub fn render(&self) -> String {
        printable(&self.render_unsafe(), true)
    }

    /// Model and machine text is escaped once, over the whole outcome.
    fn render_unsafe(&self) -> String {
        match self {
            Self::Clarify(question) => {
                format!("The model asks: {}\nNothing ran.", quoted(question))
            }
            Self::NoPlan(why) => format!(
                "Could not make a plan: {}\nNothing ran. Restart with --no-model to type commands instead.",
                // The reason quotes the model's text: on one line, so it cannot forge a report line.
                printable(why, false)
            ),
            Self::BadCommand(why) => format!(
                "Not a command: {}\nNothing ran. Type help for the commands.",
                printable(why, false)
            ),
            Self::Help(text) => text.clone(),
            Self::AllRefused(review) => format!(
                "{}\n  No step can run. Nothing ran.",
                screen::plan_screen(review, View::Friendly)
            ),
            Self::Declined(_) => "Not confirmed. Nothing ran.".to_owned(),
            Self::Executed {
                review,
                asked: false,
                steps,
                labels,
            } => format!(
                "{}\n  Ran without asking: it only read or added.\n{}",
                screen::plan_screen(review, View::Friendly),
                render_steps(steps, labels, &screen::marks_of(review))
            ),
            Self::Executed {
                review,
                steps,
                labels,
                ..
            } => render_steps(steps, labels, &screen::marks_of(review)),
            Self::LogFailed {
                reason,
                plan,
                steps,
                labels,
            } => {
                format!(
                    "{}{}The decision log cannot be written ({reason}); stopped before the next action.",
                    plan.as_deref()
                        .map(|p| format!("{p}\n"))
                        .unwrap_or_default(),
                    render_steps(steps, labels, &TreeMarks::default())
                )
            }
        }
    }
}

fn render_steps(steps: &[ExecutedStep], labels: &Labels, marks: &TreeMarks) -> String {
    let mut out = String::new();
    for step in steps {
        if let Some(why) = &step.not_run {
            out.push_str(&format!("{} not run: {why}\n", step.id));
        }
        for item in &step.items {
            let (label, text) = match &item.result {
                ItemResult::Done(output) => ("done", output.text.as_str()),
                ItemResult::Failed(why) => ("FAILED", why.as_str()),
                ItemResult::NotRun(why) => ("not run", why.as_str()),
            };
            out.push_str(&format!(
                "{} {} -> {label}\n",
                step.id,
                screen::say(&item.action, labels)
            ));
            if let ItemResult::Done(output) = &item.result {
                for note in &output.notes {
                    out.push_str(&format!("{} NOTE: {}\n", step.id, printable(note, false)));
                }
                // Indented, so output cannot pass for a line of the report.
                for line in result_tree(&item.action, output, labels, marks) {
                    out.push_str(&format!("    {line}\n"));
                }
            }
            // The tab is the break of a tree line (`screen::fit`): anything else
            // that holds one is escaped here, so it cannot pass for a column.
            for line in text.lines() {
                out.push_str(&format!("    {}\n", printable(line, false)));
            }
        }
    }
    printable(&out, true)
}

/// What a listing or a ranking found, as the tree of ADR 0018. Hidden entries
/// show only when the listed folder is itself hidden; a ranking names its
/// files, so they all show.
fn result_tree(
    action: &Action,
    output: &Output,
    labels: &Labels,
    marks: &TreeMarks,
) -> Vec<String> {
    let (root, entries, options) = match action {
        Action::List { path } => {
            let entries = if output.tree.is_empty() {
                files_found(output)
            } else {
                output.tree.clone()
            };
            let options = TreeOptions {
                show_hidden: is_hidden(path),
                ..TreeOptions::default()
            };
            (path, entries, options)
        }
        Action::FindLargest { path, .. } => {
            let options = TreeOptions {
                depth: usize::MAX,
                show_hidden: true,
                ..TreeOptions::default()
            };
            (path, files_found(output), options)
        }
        _ => return Vec::new(),
    };
    screen::tree(root, &entries, &[], labels, marks, &options)
        .into_iter()
        .map(|line| line.text)
        .collect()
}

fn files_found(output: &Output) -> Vec<TreeEntry> {
    output
        .found
        .iter()
        .map(|found| TreeEntry {
            path: found.path.clone(),
            kind: EntryKind::File,
            size: found.size,
            readable: true,
            protected: false,
            owner: None,
            created: None,
        })
        .collect()
}
