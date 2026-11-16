//! The deterministic shell as a user runs it: the real binary, `--no-model`,
//! the line adapter, a real directory, and no server anywhere. Lines go in on
//! stdin; what comes out is what the person sees.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

static RUNS: AtomicUsize = AtomicUsize::new(0);

fn run(lines: &str, extra: &[&str]) -> (String, String) {
    let home: PathBuf = std::env::temp_dir().join(format!(
        "intent-shell-no-model-{}-{}",
        std::process::id(),
        RUNS.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&home);
    fs::create_dir_all(home.join("Downloads")).unwrap();
    fs::write(home.join("Downloads/big.iso"), vec![0u8; 3000]).unwrap();
    fs::write(home.join("Downloads/small.txt"), vec![0u8; 20]).unwrap();
    let log = home.join("decisions.jsonl");
    let mut child = Command::new(env!("CARGO_BIN_EXE_intent-shell"))
        .args(["--no-model", "--plain", "--log"])
        .arg(&log)
        .args(extra)
        .env("HOME", &home)
        .env("USER", "tester")
        // Nothing listens here: if the shell tried the model, the result would say so.
        .env("TERM", "dumb")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(lines.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let logged = fs::read_to_string(&log).unwrap_or_default();
    let _ = fs::remove_dir_all(&home);
    (String::from_utf8(output.stdout).unwrap(), logged)
}

#[test]
fn a_typed_command_runs_through_the_broker_with_no_model() {
    let (out, log) = run("fs find_largest --in downloads --count 1\n", &[]);
    assert!(out.contains("model  \"none (commands only)\""), "{out}");
    assert!(out.contains("command> "), "{out}");
    assert!(out.contains("Typed commands:"), "{out}");
    assert!(out.contains("\"big.iso\" (Downloads)  3000 bytes"), "{out}");
    assert!(!out.contains("model could not be reached"), "{out}");
    assert!(log.contains("\"event\":\"command\""), "{log}");
    assert!(
        !log.contains("model_output") && !log.contains("\"request\""),
        "{log}"
    );
}

#[test]
fn a_line_that_is_not_a_command_is_refused_and_nothing_runs() {
    let (out, log) = run("delete the big files\nfs delete --in downloads\n", &[]);
    assert_eq!(out.matches("Not a command").count(), 2, "{out}");
    assert!(
        !out.contains("-> done") && !log.contains("execute"),
        "{out}{log}"
    );
}

#[test]
fn a_destructive_command_asks_and_a_refusal_runs_nothing() {
    let (out, log) = run(
        "fs delete --name big.iso --in downloads --kind file\nn\n",
        &[],
    );
    assert!(out.contains("[y] run the steps marked ok"), "{out}");
    assert!(log.contains("\"confirmed\":false"), "{log}");
    assert!(out.contains("Not confirmed. Nothing ran."), "{out}");
}

#[test]
fn root_needs_an_administrator_even_when_asked_for() {
    let line = "fs delete --name big.iso --in downloads --kind file --elevate\n";
    let (out, _) = run(line, &[]);
    assert!(out.contains("needs an administrator"), "{out}");
    // The plan is confirmed, then the root action gets its own panel; refusing it runs nothing.
    let (out, _) = run(&format!("{line}y\nn\n"), &["--administrator"]);
    assert!(out.contains("ROOT CONFIRMATION"), "{out}");
    assert!(out.contains("root confirmation was not given"), "{out}");
}

#[test]
fn a_model_and_no_model_cannot_be_asked_for_together() {
    let status = Command::new(env!("CARGO_BIN_EXE_intent-shell"))
        .args(["--no-model", "--model", "127.0.0.1:1"])
        .env("HOME", std::env::temp_dir())
        .output()
        .unwrap();
    assert!(!status.status.success());
}
