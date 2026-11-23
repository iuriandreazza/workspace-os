//! The SOUL interview and the preset path (ADR 0006, roadmap S9) as a person
//! meets them: the real binary on the line adapter, a throwaway home, and for
//! the model a fake `llama-server`. The decisions are tested in the unit
//! tests; this guards the wiring and that what is written is what the shell
//! then loads.

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

static RUNS: AtomicUsize = AtomicUsize::new(0);

fn home() -> PathBuf {
    let home = std::env::temp_dir().join(format!(
        "intent-shell-soul-interview-{}-{}",
        std::process::id(),
        RUNS.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&home);
    fs::create_dir_all(&home).unwrap();
    home
}

fn soul_file(home: &Path) -> PathBuf {
    home.join(".config/workspace-os/soul")
}

fn shell(home: &Path, args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_intent-shell"))
        .args(["--plain", "--log"])
        .arg(home.join("decisions.jsonl"))
        .args(args)
        .env("HOME", home)
        .env("USER", "tester")
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
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[test]
fn the_preset_path_with_no_model_writes_a_soul_that_the_shell_then_loads_and_that_is_not_replaced()
{
    let home = home();
    let args = ["--no-model", "--administrator", "--setup-soul"];
    let out = shell(&home, &args, "preset\nDex\nwarm-didactic\ny\n");
    assert!(out.status.success(), "{out:?}");
    assert!(
        text(&out.stdout).contains("The SOUL was written to"),
        "{}",
        text(&out.stdout)
    );
    let written = fs::read_to_string(soul_file(&home)).unwrap();
    assert_eq!(
        written,
        "format: 1\npreset: warm-didactic\nname: Dex\nlanguages: any\n"
    );

    // The shell, started with a model address, loads it and says so in its log.
    let started = shell(&home, &["--model", "127.0.0.1:1"], "");
    assert!(started.status.success(), "{started:?}");
    let log = fs::read_to_string(home.join("decisions.jsonl")).unwrap();
    let event = log
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .find(|e| e["event"] == "voice_profile")
        .unwrap();
    assert_eq!(
        (
            event["profile"].as_str(),
            event["name"].as_str(),
            event["source"].as_str()
        ),
        (Some("warm-didactic"), Some("Dex"), Some("file")),
        "{event}"
    );

    // A second run changes nothing, and fails.
    let again = shell(&home, &args, "preset\nOther\nformal-discreet\ny\n");
    assert!(!again.status.success());
    assert!(
        text(&again.stderr).contains("already exists"),
        "{}",
        text(&again.stderr)
    );
    assert!(!text(&again.stdout).contains("This is the file"));
    assert_eq!(fs::read_to_string(soul_file(&home)).unwrap(), written);
}

#[test]
fn without_root_authority_nothing_is_asked_and_nothing_is_written() {
    let home = home();
    let out = shell(&home, &["--no-model", "--setup-soul"], "preset\n\n1\ny\n");
    assert!(!out.status.success());
    assert!(
        text(&out.stderr).contains("root authority"),
        "{}",
        text(&out.stderr)
    );
    assert!(!soul_file(&home).exists());
}

#[test]
fn not_confirming_writes_nothing_and_the_input_ending_is_not_an_error() {
    let home = home();
    let out = shell(
        &home,
        &["--no-model", "--administrator", "--setup-soul"],
        "preset\n\n2\nn\n",
    );
    assert!(out.status.success(), "{out:?}");
    assert!(
        text(&out.stdout).contains("Nothing was written"),
        "{}",
        text(&out.stdout)
    );
    assert!(!soul_file(&home).exists());
    let out = shell(
        &home,
        &["--no-model", "--administrator", "--setup-soul"],
        "preset\n",
    );
    assert!(out.status.success());
    assert!(!soul_file(&home).exists());
}

#[test]
fn the_interview_with_a_model_understands_words_and_a_soul_file_the_person_named_is_where_it_goes()
{
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let (log, stop) = (Arc::clone(&seen), Arc::new(Mutex::new(false)));
    let stopping = Arc::clone(&stop);
    let server = thread::spawn(move || {
        while !*stopping.lock().unwrap() {
            let Ok((mut stream, _)) = listener.accept() else {
                thread::sleep(Duration::from_millis(10));
                continue;
            };
            stream.set_nonblocking(false).unwrap();
            let mut raw = Vec::new();
            let mut chunk = [0u8; 4096];
            while !String::from_utf8_lossy(&raw).contains("\r\n\r\n") || !complete(&raw) {
                let n = stream.read(&mut chunk).unwrap();
                if n == 0 {
                    break;
                }
                raw.extend_from_slice(&chunk[..n]);
            }
            log.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&raw).into_owned());
            let body = r#"{"choices":[{"message":{"content":"good-humoured\nlanguages: en"}}]}"#;
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    let home = home();
    let target = home.join("elsewhere/my-soul");
    let args = [
        "--model",
        address.as_str(),
        "--administrator",
        "--setup-soul",
        "--soul",
        target.to_str().unwrap(),
    ];
    let out = shell(
        &home,
        &args,
        "preset\nOrion\nsomething with a sense of humour\ny\n",
    );
    *stop.lock().unwrap() = true;
    server.join().unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "format: 1\npreset: good-humoured\nname: Orion\nlanguages: any\n"
    );
    assert!(!soul_file(&home).exists());
    let asked = seen.lock().unwrap().join("\n");
    assert!(
        asked.contains("something with a sense of humour"),
        "{asked}"
    );
}

/// Whether the request holds the whole body its head announces.
fn complete(raw: &[u8]) -> bool {
    let text = String::from_utf8_lossy(raw);
    let Some((head, body)) = text.split_once("\r\n\r\n") else {
        return false;
    };
    let length = head
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    body.len() >= length
}

#[test]
fn a_bare_file_name_is_written_in_the_current_directory() {
    let home = home();
    let mut child = Command::new(env!("CARGO_BIN_EXE_intent-shell"))
        .args([
            "--plain",
            "--no-model",
            "--administrator",
            "--setup-soul",
            "--soul",
            "soul",
            "--log",
        ])
        .arg(home.join("decisions.jsonl"))
        .current_dir(&home)
        .env("HOME", &home)
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
        .write_all(b"preset\n-\n1\ny\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(
        fs::read_to_string(home.join("soul")).unwrap(),
        "format: 1\npreset: direct-technical\nlanguages: any\n"
    );
    let log = fs::read_to_string(home.join("decisions.jsonl")).unwrap();
    assert!(log.contains("soul_written"), "{log}");
}
