//! The model client: Bonsai 8B behind a local `llama-server` (S1), with the
//! output constrained by the same schema file the broker validates against.
//!
//! HTTP is spoken by hand over `std::net`: the server is on this machine, so
//! there is no TLS, and no dependency is worth a few dozen lines.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use serde::Serialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};

use crate::ports::{Planner, Retry};

const SCHEMA: &str = include_str!("../../docs/schemas/action-plan.schema.json");

/// Static text first and the request last, so `cache_prompt` reuses the prefix (ADR 0011).
const SYSTEM_PROMPT: &str = r#"You turn the user's request into one JSON document and output nothing else.
A plan: {"kind":"plan","version":3,"summary":"...","steps":[{"id":"s1","action":"...","args":{...},"depends_on":[],"elevate":false}]}
A question, only when the request cannot be done without guessing: {"kind":"clarify","version":3,"question":"..."}
Name things with a reference, P being one of home, downloads, documents, desktop, pictures, music, videos:
a path {"path":"~/Music/jazz"}: absolute, starting with ~ (the home), or relative to the current directory;
a place {"place":P}; an existing item found by name {"name":"report.pdf","in":P,"kind":"file"} (kind: file, folder or any; in is a place P or a path {"path":"..."}); an existing item found by name anywhere below the home {"under_home":true,"name":"report.pdf","kind":"file"}; an item to create {"new_name":"notes.txt","in":P}; what an earlier step found {"from_step":"s1"} (put s1 in depends_on).
The current directory is given with the request. When the user writes a path, or means this folder, or a file in this folder, use a path: "this folder" is the current directory written out in full, and a file in it is {"path":"report.pdf"}. Use a place only when the user names one (my downloads). Use under_home only when the user does not say where. A name is a file name only, never a path. To create something that does not exist yet use new_name, never name.
Relative paths start at the current directory as it was when the request was made, for every step of the plan.
Actions and their args:
fs.list {target: place, path, name, under_home or from_step}; fs.stat {target: path, name, under_home or from_step}; fs.read {target: path, name, under_home or from_step}; fs.find_largest {target: place, path, name or under_home, count}; fs.write {target: path, name, under_home or new_name, content, mode: create|overwrite|append}; fs.move {from: path, name, under_home or from_step, to: place, path, folder name or new_name}; fs.delete {target: path, name, under_home or from_step}; process.run {program,argv}
session.cd {target: place, path or folder name}: changes the current directory for the next request.
user.manage {operation: add|remove|add_to_group|remove_from_group,user,group or null}, system.configure {key,value}, soul.propose_change {rationale,new_text} and session.cd have no elevate field.
Rules: ids are s1, s2, ...; depends_on lists the step ids a step needs; elevate is true only when the user asks for administrator rights.
A from_step reference only works after an fs.list or fs.find_largest step.
Do not ask a question when the request is clear."#;

const GENERATION_TIMEOUT: Duration = Duration::from_secs(600);

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

impl Planner for LlamaPlanner {
    fn plan(&self, request: &str, cwd: &str, retry: Option<&Retry>) -> Result<String, String> {
        let body = self.request_body(request, cwd, retry);
        let response = post(&self.address, "/v1/chat/completions", &body)?;
        let value: Value = serde_json::from_str(&response)
            .map_err(|e| format!("server reply is not JSON: {e}"))?;
        value["choices"][0]["message"]["content"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("server reply has no message: {response}"))
    }
}

fn post(address: &str, path: &str, body: &str) -> Result<String, String> {
    let mut stream =
        TcpStream::connect(address).map_err(|e| format!("cannot connect to {address}: {e}"))?;
    stream
        .set_read_timeout(Some(GENERATION_TIMEOUT))
        .map_err(|e| e.to_string())?;
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(body.as_bytes()))
        .map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).map_err(|e| e.to_string())?;
    parse_response(&raw)
}

fn parse_response(raw: &[u8]) -> Result<String, String> {
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
    let bytes = if head.contains("transfer-encoding: chunked") {
        dechunk(body)?
    } else {
        body.to_vec()
    };
    String::from_utf8(bytes).map_err(|e| e.to_string())
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
    fn refuses_a_truncated_chunk() {
        assert!(dechunk(b"9\r\nhi\r\n").is_err());
    }

    #[test]
    fn reads_a_plain_reply_and_refuses_an_error_status() {
        assert_eq!(
            parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}").unwrap(),
            "{}"
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
}
