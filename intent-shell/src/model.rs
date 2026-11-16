//! The model client: Bonsai 8B behind a local `llama-server` (S1), with the
//! output constrained by the same schema file the broker validates against.
//!
//! HTTP is spoken by hand over `std::net`: the server is on this machine, so
//! there is no TLS, and no dependency is worth a few dozen lines.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use serde_json::{Value, json};

use crate::ports::{Planner, Retry};

const SCHEMA: &str = include_str!("../../docs/schemas/action-plan.schema.json");

/// Static text first and the request last, so `cache_prompt` reuses the prefix (ADR 0011).
const SYSTEM_PROMPT: &str = r#"You turn the user's request into one JSON document and output nothing else.
A plan: {"kind":"plan","version":1,"summary":"...","steps":[{"id":"s1","action":"...","args":{...},"depends_on":[],"elevate":false}]}
A question, only when the request cannot be done without guessing: {"kind":"clarify","version":1,"question":"..."}
Actions and their args:
fs.list {path}; fs.stat {path}; fs.read {path}; fs.find_largest {path,count}; fs.write {path,content,mode: create|overwrite|append}; fs.move {from,to}; fs.delete {path}; process.run {program,argv}
user.manage {operation: add|remove|add_to_group|remove_from_group,user,group or null}, system.configure {key,value} and soul.propose_change {rationale,new_text} have no elevate field.
Rules: ids are s1, s2, ...; depends_on lists the step ids a step needs; elevate is true only when the user asks for administrator rights.
To use the paths found by an earlier fs.list or fs.find_largest step, write the path as {"from_step":"s1"} and put s1 in depends_on.
Paths may start with ~. "here" and "this directory" mean the working directory given in the request.
Do not ask a question when the request is clear."#;

const GENERATION_TIMEOUT: Duration = Duration::from_secs(600);

pub struct LlamaPlanner {
    /// `host:port` of `llama-server`.
    pub address: String,
    pub working_directory: String,
}

impl LlamaPlanner {
    fn request_body(&self, request: &str, retry: Option<&Retry>) -> Value {
        let schema: Value = serde_json::from_str(SCHEMA).unwrap_or(Value::Null);
        let mut messages = vec![
            json!({"role": "system", "content": SYSTEM_PROMPT}),
            json!({"role": "user", "content": format!("Working directory: {}\nRequest: {request}", self.working_directory)}),
        ];
        if let Some(retry) = retry {
            messages.push(json!({"role": "assistant", "content": retry.previous_output}));
            messages.push(json!({"role": "user", "content": format!("That document was refused: {}. Reply with a corrected document.", retry.error)}));
        }
        json!({
            "temperature": 0,
            "max_tokens": 1024,
            "cache_prompt": true,
            "messages": messages,
            "response_format": {"type": "json_schema", "json_schema": {"name": "action_plan", "schema": schema}},
        })
    }
}

impl Planner for LlamaPlanner {
    fn plan(&self, request: &str, retry: Option<&Retry>) -> Result<String, String> {
        let body = self.request_body(request, retry).to_string();
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
            working_directory: "/work".into(),
        };
        let body = planner.request_body("list files", None);
        assert_eq!(
            body["response_format"]["json_schema"]["schema"]["title"],
            "Action plan, version 1"
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
    fn a_retry_adds_the_refused_output_and_the_error() {
        let planner = LlamaPlanner {
            address: "127.0.0.1:1".into(),
            working_directory: "/work".into(),
        };
        let retry = Retry {
            previous_output: "{bad",
            error: "not JSON",
        };
        let messages = planner.request_body("x", Some(&retry))["messages"].clone();
        assert_eq!(messages.as_array().unwrap().len(), 4);
        assert!(
            messages[3]["content"]
                .as_str()
                .unwrap()
                .contains("not JSON")
        );
    }
}
