//! The model client: Bonsai 8B behind a local `llama-server` (S1), with the
//! output constrained by the same schema file the broker validates against.
//!
//! HTTP is spoken by hand over `std::net`: the server is on this machine, so
//! there is no TLS, and no dependency is worth a few dozen lines.

use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};

use crate::ports::{Planner, Retry, Speaker};

const SCHEMA: &str = include_str!("../../docs/schemas/action-plan.schema.json");

/// Static text first and the request last, so `cache_prompt` reuses the prefix (ADR 0011).
const SYSTEM_PROMPT: &str = r#"You turn the user's request into one JSON document and output nothing else.
A plan: {"kind":"plan","version":3,"summary":"...","steps":[{"id":"s1","action":"...","args":{...},"depends_on":[],"elevate":false}]}
A question, only when the request cannot be done without guessing: {"kind":"clarify","version":3,"question":"..."}
Name things with a reference, P being one of home, downloads, documents, desktop, pictures, music, videos:
a path {"path":"~/Music/jazz"}: absolute, starting with ~ (the home), or relative to the current directory;
a place {"place":P}; an existing item found by name {"name":"report.pdf","in":P,"kind":"file"} (kind: file, folder or any; in is a place P or a path {"path":"..."}); an existing item found by name anywhere below the home {"under_home":true,"name":"report.pdf","kind":"file"}; an item to create {"new_name":"notes.txt","in":P} (in may be a path too); what an earlier step found {"from_step":"s1"} (put s1 in depends_on).
The current directory is given with the request. When the user writes a path, or means this folder, or a file in this folder, use a path: "this folder" is the current directory written out in full, and a file in it is {"path":"report.pdf"}. Use a place only when the user names one (my downloads). Use under_home only when the user does not say where. A name is a file name only, never a path. To create something that does not exist yet use new_name, never name.
Relative paths start at the current directory as it was when the request was made, for every step of the plan. session.cd is a plan of its own: never put another step in the same plan.
Actions and their args:
fs.list {target: place, path, name, under_home or from_step}; fs.stat {target: path, name, under_home or from_step}; fs.read {target: path, name, under_home or from_step}; fs.find_largest {target: place, path, name or under_home, count}; fs.write {target: path, name, under_home or new_name, content, mode: create|overwrite|append}; fs.move {from: path, name, under_home or from_step, to: place, path, folder name or new_name}; fs.delete {target: path, name, under_home or from_step}; process.run {program,argv}
session.cd {target: place, path or folder name}: changes the current directory for the next request.
user.manage {operation: add|remove|add_to_group|remove_from_group,user,group or null}, system.configure {key,value}, soul.propose_change {rationale,new_text}, soul.undo_change {}, soul.history {} and session.cd have no elevate field.
soul.propose_change: new_text is the whole new SOUL file, one `key: value` per line, nothing else, the first line `format: 1`. For example: "format: 1\npreset: formal-discreet\nname: Atlas\nlanguages: any". Keys: preset (direct-technical|warm-didactic|formal-discreet|good-humoured, required), name, formality (casual|neutral|formal), humour (none|light|marked), detail (minimal|brief|explained), languages (en, pt-BR or any). Use soul.undo_change only when the user asks to go back to the previous SOUL.
Rules: ids are s1, s2, ...; depends_on lists the step ids a step needs; elevate is true only when the user asks for administrator rights.
A from_step reference only works after an fs.list or fs.find_largest step.
Do not ask a question when the request is clear."#;

/// A plan or a reply is a few kilobytes; this only stops a runaway server.
const MAX_ANSWER_BYTES: usize = 8 * 1024 * 1024;
const GENERATION_TIMEOUT: Duration = Duration::from_secs(600);
/// A reply is a few sentences and never worth a long wait (ADR 0021).
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(30);
/// Time to reach a server on this machine: it answers at once or not at all.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// About 400 characters of text, with room for the tokenizer.
const REPLY_MAX_TOKENS: u32 = 200;

pub struct LlamaPlanner {
    /// `host:port` of `llama-server`.
    pub address: String,
}

#[derive(Serialize)]
struct RequestBody<'a> {
    temperature: u8,
    max_tokens: u32,
    cache_prompt: bool,
    messages: Vec<Value>,
    response_format: ResponseFormat<'a>,
}

#[derive(Serialize)]
struct ResponseFormat<'a> {
    r#type: &'static str,
    json_schema: JsonSchema<'a>,
}

#[derive(Serialize)]
struct JsonSchema<'a> {
    name: &'static str,
    /// The schema as written in the file. Parsing it into a `Value` would sort
    /// its keys, and the grammar the server builds follows the order of the
    /// properties, so the model would have to write them alphabetically.
    schema: &'a RawValue,
}

impl LlamaPlanner {
    fn request_body(&self, request: &str, cwd: &str, retry: Option<&Retry>) -> String {
        let mut messages = vec![
            json!({"role": "system", "content": SYSTEM_PROMPT}),
            json!({"role": "user", "content": format!("Current directory: {cwd}\nRequest: {request}")}),
        ];
        if let Some(retry) = retry {
            messages.push(json!({"role": "assistant", "content": retry.previous_output}));
            messages.push(json!({"role": "user", "content": format!("That document was refused: {}. Reply with a corrected document.", retry.error)}));
        }
        let schema = RawValue::from_string(SCHEMA.to_owned()).expect("the schema file is JSON");
        serde_json::to_string(&RequestBody {
            temperature: 0,
            max_tokens: 1024,
            cache_prompt: true,
            messages,
            response_format: ResponseFormat {
                r#type: "json_schema",
                json_schema: JsonSchema {
                    name: "action_plan",
                    schema: &schema,
                },
            },
        })
        .expect("the request serialises")
    }
}

/// The planner of the deterministic shell. It is never asked, because lines go
/// to `Shell::handle_command`; if something asks anyway, it answers that there
/// is no model, and nothing else (ADR 0022).
pub struct NoModel;

impl Planner for NoModel {
    fn plan(&self, _request: &str, _cwd: &str, _retry: Option<&Retry>) -> Result<String, String> {
        Err("no model is loaded".to_owned())
    }
}

impl Planner for LlamaPlanner {
    fn plan(&self, request: &str, cwd: &str, retry: Option<&Retry>) -> Result<String, String> {
        let body = self.request_body(request, cwd, retry);
        completion(&self.address, &body, GENERATION_TIMEOUT)
    }
}

/// The writer of replies: the same server as the planner, asked for free text
/// with no schema, so it cannot produce a plan (ADR 0021).
pub struct LlamaSpeaker {
    /// `host:port` of `llama-server`.
    pub address: String,
    pub timeout: Duration,
}

impl LlamaSpeaker {
    pub fn new(address: &str) -> Self {
        Self {
            address: address.to_owned(),
            timeout: REPLY_TIMEOUT,
        }
    }

    fn request_body(system: &str, facts: &str) -> String {
        json!({
            "temperature": 0.6,
            "max_tokens": REPLY_MAX_TOKENS,
            "cache_prompt": true,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": facts},
            ],
        })
        .to_string()
    }
}

impl Speaker for LlamaSpeaker {
    fn reply(&self, system: &str, facts: &str) -> Result<String, String> {
        completion(
            &self.address,
            &Self::request_body(system, facts),
            self.timeout,
        )
    }
}

/// The text of the first message the server answers with.
fn completion(address: &str, body: &str, timeout: Duration) -> Result<String, String> {
    let response = post(address, "/v1/chat/completions", body, timeout)?;
    let value: Value =
        serde_json::from_str(&response).map_err(|e| format!("server reply is not JSON: {e}"))?;
    value["choices"][0]["message"]["content"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("server reply has no message: {response}"))
}

fn post(address: &str, path: &str, body: &str, timeout: Duration) -> Result<String, String> {
    let answer = exchange(
        address,
        "POST",
        path,
        "application/json",
        body.as_bytes(),
        timeout,
    )?;
    String::from_utf8(answer).map_err(|e| e.to_string())
}

/// One request and its whole answer body. The speech adapters use it too: the
/// body and the answer are bytes, and a GET has no body to send.
pub(crate) fn exchange(
    address: &str,
    method: &str,
    path: &str,
    content_type: &str,
    body: &[u8],
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    // The timeout covers the connection and the answer together.
    let deadline = Instant::now() + timeout;
    let mut stream = connect(address, deadline)?;
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {address}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(body))
        .map_err(|e| e.to_string())?;
    // The timeout bounds the whole answer, not each read: a server that drips
    // a byte every few seconds must not hold the shell.
    let mut raw = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let left = deadline
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
            .ok_or("the server took too long to answer")?;
        stream
            .set_read_timeout(Some(left))
            .map_err(|e| e.to_string())?;
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                return Err("the server took too long to answer".to_owned());
            }
            Err(e) => return Err(e.to_string()),
        }
        if raw.len() > MAX_ANSWER_BYTES {
            return Err("the server's answer is too large".to_owned());
        }
    }
    parse_response(&raw)
}

/// Connects with a limit, so an address nothing answers on cannot hold the shell.
fn connect(address: &str, deadline: Instant) -> Result<TcpStream, String> {
    let cannot = |e: &dyn std::fmt::Display| format!("cannot connect to {address}: {e}");
    let addresses = address.to_socket_addrs().map_err(|e| cannot(&e))?;
    let mut last = "no address".to_owned();
    for candidate in addresses {
        let left = deadline.saturating_duration_since(Instant::now());
        match TcpStream::connect_timeout(
            &candidate,
            CONNECT_TIMEOUT.min(left).max(Duration::from_millis(1)),
        ) {
            Ok(stream) => return Ok(stream),
            Err(e) => last = e.to_string(),
        }
    }
    Err(cannot(&last))
}

fn parse_response(raw: &[u8]) -> Result<Vec<u8>, String> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("incomplete HTTP reply")?;
    let head = String::from_utf8_lossy(&raw[..split]).to_ascii_lowercase();
    let body = &raw[split + 4..];
    let status = head.lines().next().unwrap_or_default();
    if status.split_whitespace().nth(1) != Some("200") {
        return Err(format!(
            "server answered {status:?}: {}",
            String::from_utf8_lossy(body)
        ));
    }
    if head.contains("transfer-encoding: chunked") {
        return dechunk(body);
    }
    // A connection that closes before the promised length is a cut answer,
    // which for audio would be played as if it were whole.
    let promised = head
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|length| length.trim().parse::<usize>().ok());
    if promised.is_some_and(|length| body.len() < length) {
        return Err("the server's answer was cut short".to_owned());
    }
    Ok(body.to_vec())
}

fn dechunk(mut body: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let line_end = body
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or("bad chunk framing")?;
        let size_text = String::from_utf8_lossy(&body[..line_end]);
        let size = usize::from_str_radix(size_text.split(';').next().unwrap_or("").trim(), 16)
            .map_err(|_| "bad chunk size")?;
        body = &body[line_end + 2..];
        if size == 0 {
            return Ok(out);
        }
        if size.checked_add(2).is_none_or(|needed| body.len() < needed) {
            return Err("truncated chunk".to_owned());
        }
        out.extend_from_slice(&body[..size]);
        body = &body[size + 2..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dechunks_a_chunked_body() {
        assert_eq!(
            dechunk(b"5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n").unwrap(),
            b"hello world"
        );
    }

    #[test]
    fn a_huge_chunk_size_is_an_error_not_a_panic() {
        assert!(dechunk(b"ffffffffffffffff\r\nhi\r\n").is_err());
    }

    #[test]
    fn only_the_status_code_counts_as_success() {
        assert!(parse_response(b"HTTP/1.1 500 x 200\r\n\r\n{}").is_err());
    }

    #[test]
    fn an_answer_shorter_than_its_length_is_cut_short() {
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nab").is_err());
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nab").is_ok());
    }

    #[test]
    fn refuses_a_truncated_chunk() {
        assert!(dechunk(b"9\r\nhi\r\n").is_err());
    }

    #[test]
    fn reads_a_plain_reply_and_refuses_an_error_status() {
        assert_eq!(
            parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}").unwrap(),
            b"{}"
        );
        assert!(parse_response(b"HTTP/1.1 500 Internal Server Error\r\n\r\nboom").is_err());
    }

    #[test]
    fn the_request_carries_the_schema_and_ends_with_the_user_request() {
        let planner = LlamaPlanner {
            address: "127.0.0.1:1".into(),
        };
        let body: Value =
            serde_json::from_str(&planner.request_body("list files", "~", None)).unwrap();
        assert_eq!(
            body["response_format"]["json_schema"]["schema"]["title"],
            "Action plan, version 3"
        );
        let last = body["messages"].as_array().unwrap().last().unwrap();
        assert!(
            last["content"]
                .as_str()
                .unwrap()
                .ends_with("Request: list files")
        );
    }

    #[test]
    fn the_schema_goes_out_with_its_keys_in_the_order_of_the_file() {
        let planner = LlamaPlanner {
            address: "127.0.0.1:1".into(),
        };
        let body = planner.request_body("x", "~", None);
        let at = |needle: &str| body.find(needle).unwrap_or_else(|| panic!("{needle}"));
        // The grammar follows property order: a name comes before where to look for it.
        assert!(at(r#""name": { "$ref""#) < at(r#""in": { "$ref""#));
        assert!(at(r#""kind": { "enum""#) > at(r#""in": { "$ref""#));
    }

    #[test]
    fn the_request_tells_the_model_the_current_directory_after_the_static_prompt() {
        let planner = LlamaPlanner {
            address: "127.0.0.1:1".into(),
        };
        let body: Value =
            serde_json::from_str(&planner.request_body("list", "~/Downloads", None)).unwrap();
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(
            messages[1]["content"],
            "Current directory: ~/Downloads\nRequest: list"
        );
        assert!(
            !messages[0]["content"]
                .as_str()
                .unwrap()
                .contains("~/Downloads\nRequest")
        );
    }

    #[test]
    fn a_retry_adds_the_refused_output_and_the_error() {
        let planner = LlamaPlanner {
            address: "127.0.0.1:1".into(),
        };
        let retry = Retry {
            previous_output: "{bad",
            error: "not JSON",
        };
        let body: Value =
            serde_json::from_str(&planner.request_body("x", "~", Some(&retry))).unwrap();
        let messages = body["messages"].clone();
        assert_eq!(messages.as_array().unwrap().len(), 4);
        assert!(
            messages[3]["content"]
                .as_str()
                .unwrap()
                .contains("not JSON")
        );
    }

    #[test]
    fn the_reply_request_has_no_schema_and_no_tools_and_a_small_limit() {
        let body: Value =
            serde_json::from_str(&LlamaSpeaker::request_body("the system", "the facts")).unwrap();
        assert!(body.get("response_format").is_none());
        assert!(body.get("tools").is_none() && body.get("grammar").is_none());
        assert_eq!(body["max_tokens"], REPLY_MAX_TOKENS);
        assert_eq!(body["messages"][0]["content"], "the system");
        assert_eq!(body["messages"][1]["content"], "the facts");
    }

    #[test]
    fn a_server_that_never_answers_ends_the_reply_at_the_timeout_not_never() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let silent = std::thread::spawn(move || {
            // Accepts and says nothing until the test is over.
            let held = listener.accept();
            std::thread::sleep(Duration::from_secs(30));
            drop(held);
        });
        let (done, result) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let speaker = LlamaSpeaker {
                address,
                timeout: Duration::from_millis(300),
            };
            let _ = done.send(speaker.reply("s", "f"));
        });
        let answer = result
            .recv_timeout(Duration::from_secs(10))
            .expect("the reply gave up at its timeout");
        assert!(answer.is_err(), "{answer:?}");
        drop(silent);
    }

    #[test]
    fn nothing_listening_is_an_error_at_once() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        drop(listener);
        let error = LlamaSpeaker::new(&address).reply("s", "f").unwrap_err();
        assert!(error.contains("cannot connect"), "{error}");
    }

    #[test]
    fn a_server_that_drips_bytes_is_cut_at_the_total_timeout() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            for _ in 0..200 {
                if stream.write_all(b" ").is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        let (done, result) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let speaker = LlamaSpeaker {
                address,
                timeout: Duration::from_millis(400),
            };
            let _ = done.send(speaker.reply("s", "f"));
        });
        let answer = result
            .recv_timeout(Duration::from_secs(5))
            .expect("the reply gave up at the total timeout");
        assert!(answer.unwrap_err().contains("too long"));
    }
}
