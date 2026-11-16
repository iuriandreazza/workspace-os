//! A session WITH a model, end to end, against a fake `llama-server` on a local
//! port: the real binary, the line adapter, requests reach the model and are not
//! read as commands. It guards the wiring that `--no-model` (ADR 0022) added
//! around `Setup.planner`, which no other test of the gate drives.

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::thread;

/// Answers every request with a `clarify` document, and counts them.
fn fake_server() -> (
    String,
    thread::JoinHandle<usize>,
    std::sync::mpsc::Sender<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let (stop, stopped) = std::sync::mpsc::channel::<()>();
    let handle = thread::spawn(move || {
        let mut served = 0;
        while stopped.try_recv().is_err() {
            let Ok((mut stream, _)) = listener.accept() else {
                thread::sleep(std::time::Duration::from_millis(10));
                continue;
            };
            stream.set_nonblocking(false).unwrap();
            let mut raw = Vec::new();
            let mut buffer = [0u8; 4096];
            let (mut head_end, mut length) = (None, 0usize);
            loop {
                let n = stream.read(&mut buffer).unwrap_or(0);
                if n == 0 {
                    break;
                }
                raw.extend_from_slice(&buffer[..n]);
                if head_end.is_none()
                    && let Some(at) = raw.windows(4).position(|w| w == b"\r\n\r\n")
                {
                    head_end = Some(at + 4);
                    let head = String::from_utf8_lossy(&raw[..at]).to_lowercase();
                    length = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .and_then(|v| v.trim().parse().ok())
                        .unwrap_or(0);
                }
                if head_end.is_some_and(|end| raw.len() >= end + length) {
                    break;
                }
            }
            let content =
                r#"{"kind":"clarify","version":2,"question":"Which folder do you mean?"}"#;
            let body =
                serde_json::json!({"choices": [{"message": {"content": content}}]}).to_string();
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(reply.as_bytes());
            served += 1;
        }
        served
    });
    (address, handle, stop)
}

#[test]
fn with_a_model_a_request_goes_to_the_model_and_is_not_read_as_a_command() {
    let (address, server, stop) = fake_server();
    let home =
        std::env::temp_dir().join(format!("intent-shell-model-session-{}", std::process::id()));
    let _ = fs::remove_dir_all(&home);
    fs::create_dir_all(&home).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_intent-shell"))
        .args(["--model", &address, "--plain", "--log"])
        .arg(home.join("d.jsonl"))
        .env("HOME", &home)
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
        .write_all(b"list my downloads\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    stop.send(()).unwrap();
    let served = server.join().unwrap();
    let out = String::from_utf8(output.stdout).unwrap();
    let log = fs::read_to_string(home.join("d.jsonl")).unwrap_or_default();
    let _ = fs::remove_dir_all(&home);
    assert!(output.status.success(), "{out}");
    assert!(out.contains(&format!("model  \"{address}\"")), "{out}");
    assert!(out.contains("intent> "), "{out}");
    assert!(
        out.contains("The model asks: \"Which folder do you mean?\""),
        "{out}"
    );
    assert!(
        !out.contains("Not a command") && !out.contains("command> "),
        "{out}"
    );
    assert_eq!(served, 1, "the model was asked once");
    assert!(
        log.contains("model_output") && !log.contains("\"event\":\"command\""),
        "{log}"
    );
}
