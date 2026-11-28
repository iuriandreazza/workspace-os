//! The two speech servers as seen from the shell (ADR 0031, section 4):
//! `whisper-server` to listen and Piper's HTTP server to speak, each on this
//! machine and nowhere else.
//!
//! The HTTP is the same hand-written client as the model's (`model::exchange`).
//! An address is accepted only when it is an IP address on the loopback
//! interface: a name could resolve to another machine, and the speech
//! components are local by decision (no audio leaves the machine).

use std::net::SocketAddr;
use std::time::Duration;

use serde_json::{Value, json};

use crate::model::exchange;
use crate::ports::{Synthesizer, Transcriber};

/// An utterance is at most 30 seconds, and the model runs at about a seventh of
/// real time. The interface waits for this long and no longer, with a hung server.
pub const TRANSCRIBE_TIMEOUT: Duration = Duration::from_secs(30);
/// A reply is at most about 400 characters; the server answered in a fraction of a second.
const SYNTHESIZE_TIMEOUT: Duration = Duration::from_secs(30);
/// The list of voices, and whether anything answers at all.
const QUICK_TIMEOUT: Duration = Duration::from_secs(5);
const BOUNDARY: &str = "workspace-os-speech-7d1c9f3a";

/// `host:port` of a speech server, once it is known to be on this machine.
pub fn require_loopback(address: &str) -> Result<String, String> {
    let parsed: SocketAddr = address.parse().map_err(|_| {
        format!(
            "{address} is not an IP address and port such as 127.0.0.1:8178 (names are refused: the speech servers must be on this machine)"
        )
    })?;
    if parsed.ip().is_loopback() {
        Ok(address.to_owned())
    } else {
        Err(format!(
            "{address} is not on this machine: the speech servers must listen on 127.0.0.1 or ::1"
        ))
    }
}

/// `whisper-server`'s `/inference`: one WAV file in, the words out.
pub struct WhisperClient {
    address: String,
    /// How long one transcription may take, the connection included.
    timeout: Duration,
}

impl WhisperClient {
    pub fn new(address: &str) -> Result<Self, String> {
        Ok(Self {
            address: require_loopback(address)?,
            timeout: TRANSCRIBE_TIMEOUT,
        })
    }

    pub fn with_timeout(self, timeout: Duration) -> Self {
        Self { timeout, ..self }
    }

    fn form(wav: &[u8], language: Option<&str>) -> Result<Vec<u8>, String> {
        if wav
            .windows(BOUNDARY.len())
            .any(|w| w == BOUNDARY.as_bytes())
        {
            return Err("the audio cannot be sent".to_owned());
        }
        let field = |name: &str, value: &str| {
            format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
        };
        let mut body = format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"utterance.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
        )
        .into_bytes();
        body.extend_from_slice(wav);
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(field("response_format", "json").as_bytes());
        body.extend_from_slice(field("temperature", "0.0").as_bytes());
        // `pt-BR` is told to the server as `pt`: it knows languages, not regions.
        if let Some(base) = language.and_then(|tag| tag.split('-').next()) {
            body.extend_from_slice(field("language", base).as_bytes());
        }
        body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
        Ok(body)
    }
}

impl WhisperClient {
    /// Whether anything answers: the server's front page. The shell starts
    /// either way and says so, because the server may be started later.
    pub fn ping(&self) -> Result<(), String> {
        exchange(
            &self.address,
            "GET",
            "/",
            "text/plain",
            &[],
            self.timeout.min(QUICK_TIMEOUT),
        )
        .map(|_| ())
        .map_err(|e| {
            format!(
                "the speech recogniser at {} does not answer: {e}",
                self.address
            )
        })
    }
}

impl Transcriber for WhisperClient {
    fn transcribe(&self, wav: &[u8], language: Option<&str>) -> Result<String, String> {
        let body = Self::form(wav, language)?;
        let content_type = format!("multipart/form-data; boundary={BOUNDARY}");
        let answer = exchange(
            &self.address,
            "POST",
            "/inference",
            &content_type,
            &body,
            self.timeout,
        )?;
        let value: Value = serde_json::from_slice(&answer)
            .map_err(|e| format!("the recogniser's answer is not JSON: {e}"))?;
        value["text"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| "the recogniser's answer has no text".to_owned())
    }
}

/// Piper's HTTP server: `GET /voices` and `POST /synthesize`. Never `/download`,
/// which fetches a voice from the network.
pub struct PiperClient {
    address: String,
    /// How long one synthesis may take, the connection included.
    timeout: Duration,
}

impl PiperClient {
    pub fn new(address: &str) -> Result<Self, String> {
        Ok(Self {
            address: require_loopback(address)?,
            timeout: SYNTHESIZE_TIMEOUT,
        })
    }

    pub fn with_timeout(self, timeout: Duration) -> Self {
        Self { timeout, ..self }
    }
}

/// A RIFF/WAVE file, so that what the player is given is audio and not an error page.
fn is_wav(bytes: &[u8]) -> bool {
    bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE"
}

impl Synthesizer for PiperClient {
    fn voices(&self) -> Result<Vec<String>, String> {
        let answer = exchange(
            &self.address,
            "GET",
            "/voices",
            "text/plain",
            &[],
            self.timeout.min(QUICK_TIMEOUT),
        )?;
        let value: Value = serde_json::from_slice(&answer)
            .map_err(|e| format!("the speech server's voice list is not JSON: {e}"))?;
        let voices = value
            .as_object()
            .ok_or("the speech server's voice list is not an object")?;
        Ok(voices.keys().cloned().collect())
    }

    fn synthesize(&self, voice: &str, text: &str) -> Result<Vec<u8>, String> {
        let body = json!({"text": text, "voice": voice}).to_string();
        let answer = exchange(
            &self.address,
            "POST",
            "/synthesize",
            "application/json",
            body.as_bytes(),
            self.timeout,
        )?;
        if is_wav(&answer) {
            Ok(answer)
        } else {
            Err("the speech server did not answer with audio".to_owned())
        }
    }
}
