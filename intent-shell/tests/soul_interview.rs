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

/// A pseudo-terminal pair: what a person's terminal is to the shell.
fn terminal() -> (std::os::fd::OwnedFd, std::os::fd::OwnedFd) {
    use std::os::fd::{FromRawFd, OwnedFd};
    let (mut master, mut slave) = (0, 0);
    // SAFETY: both out-pointers are live; null name, termios and winsize are allowed.
    let made = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(made, 0);
    // A screen of no size draws nothing: a real terminal has one.
    let size = libc::winsize {
        ws_row: 40,
        ws_col: 120,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: `master` is the descriptor `openpty` just made and `size` is what this request reads.
    assert_eq!(unsafe { libc::ioctl(master, libc::TIOCSWINSZ, &size) }, 0);
    // SAFETY: `openpty` returned two new descriptors that nothing else owns.
    unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) }
}

/// What a screen shows once the escape sequences that move the cursor and set colours are removed.
fn without_escapes(text: &str) -> String {
    let mut plain = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            plain.push(c);
        } else if chars.next_if_eq(&'[').is_some() {
            // A control sequence ends at its first byte in @ to ~.
            for end in chars.by_ref() {
                if ('@'..='~').contains(&end) {
                    break;
                }
            }
        }
    }
    plain
}

/// The shell on a terminal: the child, the keyboard, and what the screen showed.
struct Terminal {
    child: std::process::Child,
    input: fs::File,
    /// The bytes the screen was sent, kept whole so a sequence or a character split between two reads is not cut.
    seen: Arc<Mutex<Vec<u8>>>,
    reader: Option<thread::JoinHandle<()>>,
    deadline: std::time::Instant,
}

fn on_a_terminal(home: &Path, args: &[&str], plain: bool) -> Terminal {
    use std::os::fd::AsFd;
    let (master, slave) = terminal();
    let mut command = Command::new(env!("CARGO_BIN_EXE_intent-shell"));
    if plain {
        command.arg("--plain");
    }
    let child = command
        .arg("--log")
        .arg(home.join("decisions.jsonl"))
        .args(args)
        .env("HOME", home)
        .env("USER", "tester")
        .env("TERM", "xterm-256color")
        .stdin(slave.as_fd().try_clone_to_owned().unwrap())
        .stdout(slave.as_fd().try_clone_to_owned().unwrap())
        .stderr(slave.as_fd().try_clone_to_owned().unwrap())
        .spawn()
        .unwrap();
    drop(command);
    drop(slave);
    let input = fs::File::from(master.as_fd().try_clone_to_owned().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let shown = Arc::clone(&seen);
    let reader = thread::spawn(move || {
        let mut output = fs::File::from(master);
        let mut chunk = [0u8; 4096];
        // Reading a terminal whose other side is closed fails: that is the end.
        while let Ok(n) = output.read(&mut chunk) {
            if n == 0 {
                break;
            }
            shown.lock().unwrap().extend_from_slice(&chunk[..n]);
        }
    });
    Terminal {
        child,
        input,
        seen,
        reader: Some(reader),
        deadline: std::time::Instant::now() + Duration::from_secs(30),
    }
}

impl Terminal {
    /// What the screen shows: the bytes sent to it without the escape sequences.
    fn screen(&self) -> String {
        without_escapes(&String::from_utf8_lossy(&self.seen.lock().unwrap()))
    }

    /// Waits for `marker` to appear on the screen after what was already used.
    /// With `compact`, spaces are not compared: the interface draws only the
    /// cells that changed, so two words may arrive with nothing between them.
    fn wait(&mut self, used: &mut usize, marker: &str, compact: bool) {
        let squeeze = |text: &str| -> String {
            if compact {
                text.chars().filter(|c| !c.is_whitespace()).collect()
            } else {
                text.to_owned()
            }
        };
        let marker = squeeze(marker);
        loop {
            let screen = squeeze(&self.screen());
            if let Some(at) = screen[*used..].find(&marker) {
                *used += at + marker.len();
                break;
            }
            assert!(
                std::time::Instant::now() < self.deadline,
                "never saw {marker:?} in the last of the screen: {}",
                &screen[screen.len().saturating_sub(600)..]
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(feature = "tui")]
    fn wait_for(&mut self, used: &mut usize, marker: &str) {
        self.wait(used, marker, true);
    }

    fn wait_for_prompt(&mut self, used: &mut usize) {
        self.wait(used, "> ", false);
    }

    fn type_line(&mut self, line: &str) {
        self.input
            .write_all(format!("{line}\r").as_bytes())
            .unwrap();
    }

    /// Ctrl-D until the shell ends; one that never ends is killed and fails the test.
    fn leave(&mut self) -> (bool, String) {
        let status = loop {
            thread::sleep(Duration::from_millis(200));
            let _ = self.input.write_all(&[4]);
            if let Some(status) = self.child.try_wait().unwrap() {
                break Some(status);
            }
            if std::time::Instant::now() > self.deadline {
                self.child.kill().unwrap();
                self.child.wait().unwrap();
                break None;
            }
        };
        if let Some(reader) = self.reader.take() {
            reader.join().unwrap();
        }
        (status.is_some_and(|s| s.success()), self.screen())
    }
}

impl Drop for Terminal {
    /// A test that fails must not leave the shell running on its terminal.
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Runs the line adapter on a terminal, typing `lines` (each once the question
/// for it is shown), and then Ctrl-D; returns what the person saw.
fn on_terminal(home: &Path, args: &[&str], lines: &str) -> (bool, String) {
    let mut terminal = on_a_terminal(home, args, true);
    let mut used = 0;
    for line in lines.lines() {
        // A question ends with a prompt: each line waits for the next "> ".
        terminal.wait_for_prompt(&mut used);
        terminal.type_line(line);
    }
    terminal.leave()
}

#[test]
fn a_shell_started_on_a_terminal_with_no_soul_builds_one_and_goes_on_with_it() {
    let home = home();
    // A model nothing answers at: the person answers with numbers after one note.
    let (ok, seen) = on_terminal(
        &home,
        &["--model", "127.0.0.1:1", "--administrator"],
        "2\nDex\n2\ny\n",
    );
    assert!(ok, "{seen}");
    assert!(
        seen.contains("How do you want to set up the SOUL"),
        "{seen}"
    );
    assert!(seen.contains("The SOUL was written to"), "{seen}");
    assert_eq!(
        fs::read_to_string(soul_file(&home)).unwrap(),
        "format: 1\npreset: warm-didactic\nname: Dex\nlanguages: any\n"
    );
    // The session that followed read it.
    let log = fs::read_to_string(home.join("decisions.jsonl")).unwrap();
    assert!(
        log.contains("\"event\":\"voice_profile\"") && log.contains("\"source\":\"file\""),
        "{log}"
    );
    assert!(log.contains("\"name\":\"Dex\""), "{log}");
}

#[test]
fn a_shell_started_on_a_terminal_by_a_user_without_root_authority_says_who_can_set_the_soul() {
    let home = home();
    let (ok, seen) = on_terminal(&home, &["--model", "127.0.0.1:1"], "");
    assert!(ok, "{seen}");
    assert!(
        seen.contains("no SOUL yet") && seen.contains("--setup-soul"),
        "{seen}"
    );
    assert!(!seen.contains("How do you want"), "{seen}");
    assert!(!soul_file(&home).exists());
}

#[test]
fn a_shell_that_reads_a_script_never_takes_it_for_the_answers_of_an_interview() {
    let home = home();
    let out = shell(
        &home,
        &["--model", "127.0.0.1:1", "--administrator"],
        "list my downloads\n",
    );
    assert!(out.status.success());
    assert!(
        !text(&out.stdout).contains("How do you want"),
        "{}",
        text(&out.stdout)
    );
    assert!(!soul_file(&home).exists());
}

#[test]
fn a_terminal_shell_that_would_not_read_the_soul_or_has_one_asks_nothing() {
    let home = home();
    for args in [
        vec!["--model", "127.0.0.1:1", "--administrator", "--no-reply"],
        vec!["--no-model", "--administrator"],
    ] {
        let (ok, seen) = on_terminal(&home, &args, "");
        assert!(ok, "{args:?}: {seen}");
        assert!(!seen.contains("How do you want"), "{args:?}: {seen}");
        assert!(!seen.contains("no SOUL yet"), "{args:?}: {seen}");
        assert!(!soul_file(&home).exists());
    }
    let named = home.join("named-soul");
    let (ok, seen) = on_terminal(
        &home,
        &[
            "--model",
            "127.0.0.1:1",
            "--administrator",
            "--soul",
            named.to_str().unwrap(),
        ],
        "",
    );
    // A SOUL the person named and that is missing is the error it always was.
    assert!(!ok, "{seen}");
    assert!(seen.contains("does not exist"), "{seen}");
    assert!(!seen.contains("How do you want"), "{seen}");
    fs::create_dir_all(soul_file(&home).parent().unwrap()).unwrap();
    fs::write(soul_file(&home), "format: 1\npreset: formal-discreet\n").unwrap();
    let (ok, seen) = on_terminal(&home, &["--model", "127.0.0.1:1", "--administrator"], "");
    assert!(ok, "{seen}");
    assert!(!seen.contains("How do you want"), "{seen}");
    assert_eq!(
        fs::read_to_string(soul_file(&home)).unwrap(),
        "format: 1\npreset: formal-discreet\n"
    );
}

#[test]
fn a_transcript_without_the_interface_is_refused_before_anything_is_asked() {
    let home = home();
    let transcript = home.join("t.txt");
    let (ok, seen) = on_terminal(
        &home,
        &[
            "--model",
            "127.0.0.1:1",
            "--administrator",
            "--transcript",
            transcript.to_str().unwrap(),
        ],
        "",
    );
    assert!(!ok, "{seen}");
    assert!(
        seen.contains("--transcript needs the conversational interface"),
        "{seen}"
    );
    assert!(!seen.contains("How do you want"), "{seen}");
    assert!(!soul_file(&home).exists());
}

// Without the interface the interview runs on the line adapter, which these two are about not.
#[cfg(feature = "tui")]
#[test]
fn the_conversational_interface_runs_the_interview_and_then_the_session() {
    let home = home();
    let mut terminal = on_a_terminal(&home, &["--model", "127.0.0.1:1", "--administrator"], false);
    let mut used = 0;
    for (marker, line) in [
        ("How do you want to set up the SOUL", "2"),
        ("call itself", "Dex"),
        ("How should it sound", "2"),
        ("Write it?", "y"),
        ("starts the shell", ""),
    ] {
        terminal.wait_for(&mut used, marker);
        // A line is typed only after its question was drawn; the last one only ends the screen.
        if marker == "starts the shell" {
            terminal.input.write_all(&[4]).unwrap();
        } else {
            terminal.type_line(line);
        }
    }
    // The session screen opens with the header, after the interview.
    terminal.wait_for(&mut used, "intent shell");
    let (ok, seen) = terminal.leave();
    assert!(ok, "{seen}");
    assert_eq!(
        fs::read_to_string(soul_file(&home)).unwrap(),
        "format: 1\npreset: warm-didactic\nname: Dex\nlanguages: any\n"
    );
    let log = fs::read_to_string(home.join("decisions.jsonl")).unwrap();
    assert!(
        log.contains("\"event\":\"voice_profile\"") && log.contains("\"name\":\"Dex\""),
        "{log}"
    );
}

// Without the interface the interview runs on the line adapter, which these two are about not.
#[cfg(feature = "tui")]
#[test]
fn a_hangup_during_the_interview_ends_the_shell_instead_of_opening_the_session_screen() {
    let home = home();
    let mut terminal = on_a_terminal(&home, &["--model", "127.0.0.1:1", "--administrator"], false);
    let mut used = 0;
    terminal.wait_for(&mut used, "How do you want to set up the SOUL");
    // SAFETY: the pid is that of the child this test started and has not waited for.
    unsafe { libc::kill(terminal.child.id() as libc::pid_t, libc::SIGHUP) };
    let status = loop {
        if let Some(status) = terminal.child.try_wait().unwrap() {
            break status;
        }
        assert!(
            std::time::Instant::now() < terminal.deadline,
            "the shell never ended"
        );
        thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(1), "{status:?}");
    assert!(!soul_file(&home).exists());
}

#[test]
fn a_named_personality_that_cannot_be_used_fails_before_any_question() {
    let home = home();
    let missing = home.join("no-personality");
    let (ok, seen) = on_terminal(
        &home,
        &[
            "--model",
            "127.0.0.1:1",
            "--administrator",
            "--personality",
            missing.to_str().unwrap(),
        ],
        "",
    );
    assert!(!ok, "{seen}");
    assert!(seen.contains("does not exist"), "{seen}");
    assert!(!seen.contains("How do you want"), "{seen}");
    assert!(!soul_file(&home).exists());
}
