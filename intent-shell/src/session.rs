//! One request through the whole pipeline: model, checks, broker,
//! confirmation, execution, with every decision logged.

use std::collections::HashSet;

use serde_json::{Value, json};

use crate::action::{Action, quoted};
use crate::broker::{Authority, Review, ReviewedAction, ReviewedStep, Verdict, review};
use crate::plan::{Document, StepId, parse_checked};
use crate::ports::{Confirm, DecisionLog, Host, Output, Planner, Retry};

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
    /// Nothing in the plan passed the broker. Nothing ran.
    AllRefused(Review),
    /// The person said no. Nothing ran.
    Declined(Review),
    Executed {
        review: Review,
        steps: Vec<ExecutedStep>,
    },
    /// The log could not be written, so the shell stopped before the next action.
    LogFailed {
        reason: String,
        steps: Vec<ExecutedStep>,
    },
}

pub struct Shell<'a> {
    pub authority: &'a Authority,
    pub host: &'a dyn Host,
    pub planner: &'a dyn Planner,
    pub confirm: &'a mut dyn Confirm,
    pub log: &'a mut dyn DecisionLog,
}

impl Shell<'_> {
    pub fn handle(&mut self, request: &str) -> Outcome {
        self.run(request)
            .unwrap_or_else(|reason| Outcome::LogFailed {
                reason,
                steps: Vec::new(),
            })
    }

    fn record(&mut self, event: Value) -> Result<(), String> {
        self.log.record(event)
    }

    fn run(&mut self, request: &str) -> Result<Outcome, String> {
        self.record(
            json!({"event": "request", "user": self.authority.user.name, "request": request}),
        )?;
        let document = match self.obtain_document(request)? {
            Ok(document) => document,
            Err(failure) => return Ok(failure),
        };
        let plan = match document {
            Document::Clarify(question) => {
                self.record(json!({"event": "clarify", "question": question}))?;
                return Ok(Outcome::Clarify(question));
            }
            Document::Plan(plan) => plan,
        };
        let reviewed = review(&plan, self.authority, self.host);
        self.record(json!({"event": "review", "review": review_json(&reviewed)}))?;

        let anything_runs = reviewed
            .steps
            .iter()
            .flat_map(|s| &s.items)
            .any(|i| !matches!(i.verdict, Verdict::Refused(_)));
        if !anything_runs {
            return Ok(Outcome::AllRefused(reviewed));
        }
        let confirmed = self.confirm.plan(&reviewed.render());
        self.record(json!({"event": "plan_confirmation", "confirmed": confirmed}))?;
        if !confirmed {
            return Ok(Outcome::Declined(reviewed));
        }
        let (steps, logged) = self.execute(&reviewed);
        Ok(match logged {
            Ok(()) => Outcome::Executed {
                review: reviewed,
                steps,
            },
            Err(reason) => Outcome::LogFailed { reason, steps },
        })
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
            let raw = match self.planner.plan(request, retry.as_ref()) {
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
            let (record, outcome) = self.run_step(step, &succeeded);
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
            (Some(why), _) => Some(why.to_string()),
            (None, Some(dependency)) => Some(format!("step {dependency} did not complete")),
            (None, None) => None,
        };
        if record.not_run.is_some() {
            let logged = self.record(json!({"event": "step_not_run", "step": step.id.as_str(), "reason": record.not_run}));
            return (record, logged.map(|()| false));
        }
        let mut complete = true;
        for item in &step.items {
            match self.run_item(&step.id, item, &mut record) {
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
        record: &mut ExecutedStep,
    ) -> Result<bool, String> {
        let rendering = item.action.describe();
        let result = match &item.verdict {
            Verdict::Refused(why) => ItemResult::NotRun(why.to_string()),
            Verdict::NeedsRoot if !self.confirm_root(step, &rendering)? => {
                ItemResult::NotRun("root confirmation was not given".to_owned())
            }
            Verdict::Allowed | Verdict::NeedsRoot => {
                self.record(
                    json!({"event": "execute", "step": step.as_str(), "action": rendering}),
                )?;
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

    fn confirm_root(&mut self, step: &StepId, rendering: &str) -> Result<bool, String> {
        let who = self.authority.user.clone();
        let confirmed = self.confirm.root_action(rendering, &who);
        self.record(json!({"event": "root_confirmation", "step": step.as_str(), "action": rendering, "confirmed_by": who.name, "confirmed": confirmed}))?;
        Ok(confirmed)
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
                    json!({"action": item.action.describe(), "verdict": verdict})
                })
                .collect();
            json!({"step": step.id.as_str(), "blocked": step.blocked.as_ref().map(ToString::to_string), "items": items})
        })
        .collect();
    json!({"summary": reviewed.summary, "steps": steps})
}

impl Outcome {
    /// What to tell the person after the request ended.
    pub fn render(&self) -> String {
        match self {
            Self::Clarify(question) => {
                format!("The model asks: {}\nNothing ran.", quoted(question))
            }
            Self::NoPlan(why) => format!(
                "Could not make a plan: {why}\nNothing ran. Use the deterministic shell instead."
            ),
            Self::AllRefused(review) => {
                format!("{}Every step was refused. Nothing ran.", review.render())
            }
            Self::Declined(_) => "Not confirmed. Nothing ran.".to_owned(),
            Self::Executed { steps, .. } => render_steps(steps),
            Self::LogFailed { reason, steps } => {
                format!(
                    "{}The decision log cannot be written ({reason}); stopped before the next action.",
                    render_steps(steps)
                )
            }
        }
    }
}

fn render_steps(steps: &[ExecutedStep]) -> String {
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
                item.action.describe()
            ));
            if !text.is_empty() {
                out.push_str(&format!("{text}\n"));
            }
        }
    }
    out
}
