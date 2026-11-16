//! Talks to a real `llama-server` running Bonsai 8B. Out of the gate: CI has no
//! model. Run it by hand, with the server up (docs/s3-intent-shell-prototype.md):
//!
//!     ./scripts/intent-shell-live-test.sh
//!
//! It checks the plumbing, not the quality of the plans (that is S4): the
//! constrained output reaches the broker, and declining runs nothing.

use intent_shell::broker::Authority;
use intent_shell::host::UnixHost;
use intent_shell::model::LlamaPlanner;
use intent_shell::ports::{Confirm, DecisionLog, User};
use intent_shell::session::{Outcome, Shell};
use serde_json::Value;

struct Decline;

impl Confirm for Decline {
    fn plan(&mut self, _screen: &str, _details: &str) -> bool {
        false
    }

    fn root_action(&mut self, _panel: &str, _who: &User) -> bool {
        false
    }

    fn choose(&mut self, _question: &str, _options: &[String]) -> Option<usize> {
        None
    }
}

#[derive(Default)]
struct Events(Vec<Value>);

impl DecisionLog for Events {
    fn record(&mut self, event: Value) -> Result<(), String> {
        self.0.push(event);
        Ok(())
    }
}

#[test]
#[ignore = "needs llama-server with Bonsai 8B"]
fn the_model_answers_through_the_broker_and_declining_runs_nothing() {
    let address =
        std::env::var("INTENT_SHELL_MODEL").unwrap_or_else(|_| "127.0.0.1:8089".to_owned());
    let planner = LlamaPlanner { address };
    let host = UnixHost::from_environment();
    let authority = Authority::full(User {
        name: "tester".to_owned(),
        administrator: false,
    });
    let mut log = Events::default();

    let outcome = Shell {
        authority: &authority,
        host: &host,
        planner: &planner,
        confirm: &mut Decline,
        log: &mut log,
    }
    .handle("list the five largest files in my downloads");

    assert!(
        matches!(
            outcome,
            Outcome::Declined(_) | Outcome::Clarify(_) | Outcome::AllRefused(_)
        ),
        "a declined or unanswered request must not execute anything, got {outcome:?}"
    );
    let first_attempt = log
        .0
        .iter()
        .find(|e| e["event"] == "model_output")
        .expect("the model's output is logged");
    assert_eq!(
        first_attempt["accepted"], true,
        "grammar-constrained output should pass the broker's checks: {first_attempt}"
    );
}
