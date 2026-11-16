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
            model: self
                .planner
                .as_ref()
                .map_or("none (commands only)", |p| &p.address),
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
