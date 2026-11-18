//! The evaluation set of S4 (`eval/intents.json`) is data for any model and any
//! personality preset. Every plan it accepts must be one the shell would
//! accept, so the set is run through the shell's own parser and rules.

use std::path::Path;

use intent_shell::plan::parse_checked;
use serde_json::Value;

fn intents() -> Vec<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../eval/intents.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let set: Value = serde_json::from_str(&text).expect("the set is JSON");
    set["intents"].as_array().expect("an intents array").clone()
}

#[test]
fn every_intent_has_text_and_something_to_accept() {
    let intents = intents();
    assert!(
        intents.len() >= 89,
        "the set is too small to tell models apart"
    );
    for intent in &intents {
        let id = intent["id"].as_str().expect("an id");
        assert!(
            !intent["text"]
                .as_str()
                .unwrap_or_default()
                .trim()
                .is_empty(),
            "{id}: no text"
        );
        assert!(
            !intent["accept"].as_array().expect("accept").is_empty(),
            "{id}: nothing accepted"
        );
    }
}

#[test]
fn intent_ids_are_unique() {
    let mut seen = std::collections::HashSet::new();
    for intent in intents() {
        let id = intent["id"].as_str().expect("an id").to_owned();
        assert!(seen.insert(id.clone()), "{id} appears twice");
    }
}

#[test]
fn every_accepted_plan_passes_the_shells_parser_and_rules() {
    for intent in intents() {
        let id = intent["id"].as_str().expect("an id");
        for accepted in intent["accept"].as_array().expect("accept") {
            let text = accepted["plan"].to_string();
            if let Err(error) = parse_checked(&text) {
                panic!("{id}: an accepted plan is refused by the shell: {error}\n{text}");
            }
        }
    }
}

#[test]
fn every_intent_says_what_it_is_for_the_scorer() {
    for intent in intents() {
        for field in ["id", "category", "language", "source", "cwd", "text"] {
            assert!(
                intent[field].as_str().is_some_and(|v| !v.is_empty()),
                "{}: no {field}",
                intent["id"]
            );
        }
    }
}
