//! What every terminal adapter needs to run one session: who, on which machine,
//! with which model, writing which log. Neutral between the adapters, so
//! neither depends on the other (ADR 0015).

use crate::broker::Authority;
use crate::host::UnixHost;
use crate::log::JsonlLog;
use crate::model::LlamaPlanner;
use crate::screen::{self, Header};

/// What both adapters need to run one session.
pub struct Setup {
    pub authority: Authority,
    pub host: UnixHost,
    pub planner: LlamaPlanner,
    pub log: JsonlLog,
    /// The decision log's file name, for the header (its directory is in the details).
    pub log_name: String,
}

impl Setup {
    pub fn header(&self) -> String {
        screen::header(&Header {
            user: &self.authority.user.name,
            administrator: self.authority.user.administrator,
            model: &self.planner.address,
            log: &self.log_name,
        })
    }
}
