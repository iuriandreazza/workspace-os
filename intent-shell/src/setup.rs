//! What every terminal adapter needs to run one session: who, on which machine,
//! with which model, writing which log. Neutral between the adapters, so
//! neither depends on the other (ADR 0015).

use crate::broker::Authority;
use crate::host::UnixHost;
use crate::log::JsonlLog;
use crate::model::LlamaPlanner;
use crate::screen::{self, Header};
use crate::session::SessionState;

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
        let (model, log_name) = (self.planner.address.clone(), self.log_name.clone());
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
