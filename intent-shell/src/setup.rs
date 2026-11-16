//! What every terminal adapter needs to run one session: who, on which machine,
//! with which model, writing which log. Neutral between the adapters, so
//! neither depends on the other (ADR 0015).

use crate::broker::Authority;
use crate::host::UnixHost;
use crate::log::JsonlLog;
use crate::model::{LlamaPlanner, NoModel};
use crate::ports::Planner;
use crate::screen::{self, Header};
use crate::session::Entry;

static NO_MODEL: NoModel = NoModel;

/// What both adapters need to run one session.
pub struct Setup {
    pub authority: Authority,
    pub host: UnixHost,
    /// `None` is the deterministic shell: no model, lines are commands (ADR 0022).
    pub planner: Option<LlamaPlanner>,
    pub log: JsonlLog,
    /// The decision log's file name, for the header (its directory is in the details).
    pub log_name: String,
}

impl Setup {
    pub fn entry(&self) -> Entry {
        if self.planner.is_some() {
            Entry::Request
        } else {
            Entry::Command
        }
    }

    pub fn header(&self) -> String {
        screen::header(&Header {
            user: &self.authority.user.name,
            administrator: self.authority.user.administrator,
            model: model_label(self.planner.as_ref()),
            log: &self.log_name,
        })
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
            log: "d.jsonl",
        });
        assert!(header.contains("none (commands only)"), "{header}");
    }
}
