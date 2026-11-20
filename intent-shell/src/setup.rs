//! What every terminal adapter needs to run one session: who, on which machine,
//! with which model, writing which log. Neutral between the adapters, so
//! neither depends on the other (ADR 0015).

use crate::broker::Authority;
use crate::host::UnixHost;
use crate::log::JsonlLog;
use crate::model::{LlamaPlanner, LlamaSpeaker, NoModel};
use crate::ports::Planner;
use crate::screen::{self, Header};
use crate::session::{Entry, SessionState};
use crate::soul::Persona;

static NO_MODEL: NoModel = NoModel;

/// What both adapters need to run one session.
pub struct Setup {
    pub authority: Authority,
    pub host: UnixHost,
    /// `None` is the deterministic shell: no model, lines are commands (ADR 0022).
    pub planner: Option<LlamaPlanner>,
    pub log: JsonlLog,
    /// The persona of the replies, or `None` for no replies (`--no-reply`). It
    /// only counts when there is a model to write them (ADR 0021).
    pub reply: Option<Persona>,
    /// Said to the person when the session opens, because on a screen the
    /// error output is hidden: a SOUL or a personality that could not be used.
    pub notice: Option<String>,
    /// The decision log's file name, for the header (its directory is in the details).
    pub log_name: String,
}

impl Setup {
    pub fn entry(&self) -> Entry {
        Entry::for_model(self.planner.is_some())
    }

    /// Who writes the replies and with which persona, when replies are on and
    /// there is a model.
    pub fn speaker(&self) -> Option<(LlamaSpeaker, Persona)> {
        let model = self.planner.as_ref()?;
        Some((LlamaSpeaker::new(&model.address), self.reply.clone()?))
    }

    /// The header for the directory the session is in now.
    pub fn header(&self, session: &SessionState) -> String {
        self.header_fn()(&session.cwd_text(&self.host))
    }

    /// The header as a function of the directory, owning what it needs, so a
    /// worker thread can rebuild it after a `cd`.
    pub fn header_fn(&self) -> impl Fn(&str) -> String + use<> {
        let (user, administrator) = (
            self.authority.user.name.clone(),
            self.authority.user.administrator,
        );
        let (model, log_name) = (
            model_label(self.planner.as_ref()).to_owned(),
            self.log_name.clone(),
        );
        move |cwd| {
            screen::header(&Header {
                user: &user,
                administrator,
                model: &model,
                cwd,
                log: &log_name,
            })
        }
    }
}

/// The model, or the planner that is never asked when there is none.
pub fn planner_of(model: Option<&LlamaPlanner>) -> &dyn Planner {
    match model {
        Some(model) => model,
        None => &NO_MODEL,
    }
}

/// What the header calls the model: its address, or that there is none.
pub fn model_label(model: Option<&LlamaPlanner>) -> &str {
    model.map_or("none (commands only)", |model| &model.address)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(model: Option<LlamaPlanner>) -> Setup {
        let log = std::env::temp_dir().join(format!(
            "intent-shell-setup-{}-{}.jsonl",
            std::process::id(),
            model.is_some()
        ));
        Setup {
            authority: Authority::full(crate::ports::User {
                name: "ana".to_owned(),
                administrator: false,
            }),
            host: UnixHost::from_environment(),
            planner: model,
            log: JsonlLog::open(&log).unwrap(),
            reply: Some(Persona::of(crate::soul::Profile::DirectTechnical)),
            notice: None,
            log_name: "d.jsonl".to_owned(),
        }
    }

    #[test]
    fn a_session_with_a_model_reads_requests_and_asks_the_model_and_one_without_does_neither() {
        // Nothing listens on this address: the answer is an error about the connection.
        let with = setup(Some(LlamaPlanner {
            address: "127.0.0.1:1".to_owned(),
        }));
        assert_eq!(with.entry(), Entry::Request);
        let asked = planner_of(with.planner.as_ref())
            .plan("x", "~", None)
            .unwrap_err();
        assert!(!asked.contains("no model is loaded"), "{asked}");
        let without = setup(None);
        assert_eq!(without.entry(), Entry::Command);
        let answer = planner_of(without.planner.as_ref())
            .plan("x", "~", None)
            .unwrap_err();
        assert!(answer.contains("no model is loaded"), "{answer}");
        let start = SessionState::start(&with.host);
        assert!(with.header(&start).contains("127.0.0.1:1"));
        assert!(without.header(&start).contains("none (commands only)"));
    }

    #[test]
    fn replies_need_a_model_and_are_on_unless_turned_off() {
        let model = || {
            Some(LlamaPlanner {
                address: "127.0.0.1:1".to_owned(),
            })
        };
        assert!(setup(model()).speaker().is_some());
        assert!(setup(None).speaker().is_none());
        let mut off = setup(model());
        off.reply = None;
        assert!(off.speaker().is_none());
    }

    #[test]
    fn the_header_says_when_there_is_no_model() {
        assert_eq!(model_label(None), "none (commands only)");
        let model = LlamaPlanner {
            address: "127.0.0.1:8089".to_owned(),
        };
        assert_eq!(model_label(Some(&model)), "127.0.0.1:8089");
        let header = screen::header(&Header {
            user: "ana",
            administrator: false,
            model: model_label(None),
            cwd: "~",
            log: "d.jsonl",
        });
        assert!(header.contains("none (commands only)"), "{header}");
    }
}
