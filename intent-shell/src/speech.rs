//! Speech on the host (ADR 0031, roadmap S11-2): the closed list of voices, the
//! mouth that speaks a reply and the ear that turns audio into a line to edit.
//!
//! Everything here sits outside the decision path, like `reply`: the ear hands
//! the interface text that waits for Enter, and the mouth only ever receives
//! the cleaned reply of ADR 0021. Neither takes a `Shell`, a `Confirm`, a `Host`
//! or a `Planner`, so no audio and no voice can reach the resolver, the broker
//! or a confirmation. The platform and the two servers hide behind the traits
//! of `ports`; the real ones are in `audio` and `speech_http`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::{Value, json};

use crate::action::capped;
use crate::ports::{AudioCapture, AudioPlayback, DecisionLog, Synthesizer, Transcriber};
use crate::reply::{REPLY_LIMIT, escaped_line};

/// A voice of the closed list (ADR 0031, section 2): what the personality
/// names, the Piper voice it stands for and the file that was checked.
#[derive(Debug, PartialEq, Eq)]
pub struct SpokenVoice {
    /// The value of `voice` in a personality.
    pub id: &'static str,
    /// The language the voice speaks, as a tag of the personality's language key.
    pub language: &'static str,
    /// The name Piper loads it by: the `.onnx` file without its extension.
    pub piper: &'static str,
    /// SHA-256 of the `.onnx` file, from `rhasspy/piper-voices` at commit
    /// `e9c8a39444359388adaa94255becb37a5a2b8e51`.
    pub sha256: &'static str,
    /// What the notice of a release owes the recordings the voice descends from.
    pub attribution: Option<&'static str>,
}

/// The voices the shell accepts. An entry is added only with the reading of
/// its model card and the hash of its file in the commit that adds it.
pub const VOICES: [SpokenVoice; 4] = [
    SpokenVoice {
        id: "cadu",
        language: "pt-BR",
        piper: "pt_BR-cadu-medium",
        sha256: "d5a662e3f606d5a49dd29ba9ce90c304ae45dd9543f8b0c773bcb0de282499de",
        attribution: None,
    },
    SpokenVoice {
        id: "jeff",
        language: "pt-BR",
        piper: "pt_BR-jeff-medium",
        sha256: "82ddcdb01a0342f078bd89ce2815eda27185818d15e15f06fb9a72ac3bce94e1",
        attribution: None,
    },
    SpokenVoice {
        id: "faber",
        language: "pt-BR",
        piper: "pt_BR-faber-medium",
        sha256: "75e913eb463db0b6ac7ddf68d3e4c9ac19ace36dc3b710a81039b448352b8ffc",
        attribution: Some("Hi-Fi TTS (CC BY 4.0), the corpus the voice was trained from"),
    },
    SpokenVoice {
        id: "joe",
        language: "en-US",
        piper: "en_US-joe-medium",
        sha256: "abf4b998e34045cbd00eaf050de2de620f81ded6eb8b85a24b022a1c2b3f08d2",
        attribution: None,
    },
];

pub fn voice(id: &str) -> Option<&'static SpokenVoice> {
    VOICES.iter().find(|voice| voice.id == id)
}

/// The ids, for the messages.
pub fn voice_ids() -> String {
    VOICES.map(|voice| voice.id).join(", ")
}

/// Whether a voice that speaks `voice_language` suits `wanted`: the same
/// language, and the same region when both name one. `en` suits `en-US`; `pt`
/// suits `pt-BR`; `pt-PT` does not suit `pt-BR`.
pub fn language_fits(voice_language: &str, wanted: &str) -> bool {
    let split = |tag: &str| {
        let (base, region) = tag
            .split_once('-')
            .map_or((tag, None), |(b, r)| (b, Some(r)));
        (base.to_owned(), region.map(str::to_owned))
    };
    let ((have, have_region), (want, want_region)) = (split(voice_language), split(wanted));
    have == want
        && match (have_region, want_region) {
            (Some(a), Some(b)) => a == b,
            _ => true,
        }
}

/// The voice a personality with no `voice` is spoken with: the first of the
/// list for the reply language, chosen by code. With no language anywhere
/// (a SOUL that says `any` and a personality that names none) it is the first
/// voice of the list, because there is nothing to match.
pub fn default_voice(language: Option<&str>) -> &'static SpokenVoice {
    language
        .and_then(|wanted| {
            VOICES
                .iter()
                .find(|voice| language_fits(voice.language, wanted))
        })
        .unwrap_or(&VOICES[0])
}

/// What the reply says aloud: the cleaned reply and nothing else, once more
/// capped. `None` when there is nothing to say.
pub fn speakable(reply: &str) -> Option<String> {
    let text = capped(reply.trim(), REPLY_LIMIT);
    (!text.is_empty()).then_some(text)
}

/// How many utterances are queued or playing. The ear refuses to listen while
/// it is above zero: the system must not hear itself (ADR 0031, section 5).
#[derive(Clone, Default)]
pub struct Speaking(Arc<AtomicUsize>);

impl Speaking {
    pub fn begin(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    pub fn end(&self) {
        // Never below zero, even if an end arrives with no begin.
        let _ = self
            .0
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
    }

    pub fn active(&self) -> bool {
        self.0.load(Ordering::SeqCst) > 0
    }
}

/// Why something failed, as one escaped line the person can read: what a
/// server wrote in an error is not trusted, and an HTML page is not a line.
pub fn reason(why: &str) -> String {
    escaped_line(why, 200)
}

/// Speaks a reply with one voice. A failure is one line and never an error of
/// the request (ADR 0031, section 5, point 6).
pub struct Mouth {
    synthesizer: Box<dyn Synthesizer + Send>,
    playback: Box<dyn AudioPlayback + Send>,
    voice: &'static SpokenVoice,
}

impl Mouth {
    pub fn new(
        synthesizer: Box<dyn Synthesizer + Send>,
        playback: Box<dyn AudioPlayback + Send>,
        voice: &'static SpokenVoice,
    ) -> Self {
        Self {
            synthesizer,
            playback,
            voice,
        }
    }

    pub fn voice(&self) -> &'static SpokenVoice {
        self.voice
    }

    /// Synthesise and play `reply`. The server answers with its default voice
    /// when it does not know the one asked for, so the voice is checked to be
    /// installed first: two users must never hear one voice by accident.
    pub fn say(&self, reply: &str) -> Result<(), String> {
        let Some(text) = speakable(reply) else {
            return Ok(());
        };
        let installed = self
            .synthesizer
            .voices()
            .map_err(|e| format!("speech is off for this reply: {}", reason(&e)))?;
        if !installed.iter().any(|name| name == self.voice.piper) {
            return Err(format!(
                "speech is off for this reply: the voice {} ({}) is not installed on the speech server",
                self.voice.id, self.voice.piper
            ));
        }
        let wav = self
            .synthesizer
            .synthesize(self.voice.piper, &text)
            .map_err(|e| format!("speech is off for this reply: {}", reason(&e)))?;
        self.playback
            .play(&wav)
            .map_err(|e| format!("speech is off for this reply: {}", reason(&e)))
    }
}

/// The longest line a transcript may become. The input line of the interface
/// takes no more than a person would type for one request.
const TRANSCRIPT_LIMIT: usize = 500;

/// Turns the words of a recogniser into a line the person can read and edit:
/// one line, escaped, capped. It is untrusted text like a pasted one (ADR 0031,
/// section 5, point 1). `None` when nothing was said: the recogniser writes
/// a bracketed tag such as `[BLANK_AUDIO]` for silence.
pub fn transcript_line(raw: &str) -> Option<String> {
    let mut text = raw.trim();
    // Bracketed tags at either end are the recogniser's, wherever the speech sits
    // between them: `[Music] list the files [Music]`, `[ Silence ]`.
    loop {
        let before = text;
        if text.starts_with('[')
            && let Some(end) = text.find(']')
        {
            text = text[end + 1..].trim_start();
        }
        if text.ends_with(']')
            && let Some(start) = text.rfind('[')
        {
            text = text[..start].trim_end();
        }
        // A parenthesis is only a tag when it is the whole line: "(1) list" and
        // "the folder (downloads)" are speech.
        if text.starts_with('(') && text.ends_with(')') && text.matches(')').count() == 1 {
            text = "";
        }
        if text == before {
            break;
        }
    }
    (!text.is_empty()).then(|| escaped_line(text, TRANSCRIPT_LIMIT))
}

/// Records one utterance and turns it into a line. Push-to-talk: `start`, then
/// `finish`, and nothing is recorded in between by anything else.
pub struct Ear {
    capture: Box<dyn AudioCapture + Send>,
    transcriber: Box<dyn Transcriber + Send>,
    speaking: Speaking,
}

impl Ear {
    pub fn new(
        capture: Box<dyn AudioCapture + Send>,
        transcriber: Box<dyn Transcriber + Send>,
        speaking: Speaking,
    ) -> Self {
        Self {
            capture,
            transcriber,
            speaking,
        }
    }

    pub fn start(&mut self) -> Result<(), String> {
        if self.speaking.active() {
            return Err("the system is speaking: wait until it has finished".to_owned());
        }
        self.capture.start()
    }

    pub fn cancel(&mut self) {
        self.capture.cancel();
    }

    /// Stop recording and transcribe. `language` is the reply language, when known.
    pub fn finish(&mut self, language: Option<&str>) -> Result<String, String> {
        let wav = self.capture.finish()?;
        let raw = self
            .transcriber
            .transcribe(&wav, language)
            .map_err(|e| format!("the speech recogniser failed: {}", reason(&e)))?;
        transcript_line(&raw).ok_or_else(|| "nothing was heard".to_owned())
    }
}

/// Speaks replies one after another on a thread of its own, so that playing
/// one never holds the shell. A failure is reported as a line and nothing else.
pub struct Talker {
    queue: Sender<String>,
    speaking: Speaking,
}

impl Talker {
    pub fn spawn(
        mouth: Mouth,
        speaking: Speaking,
        report: impl Fn(String) + Send + 'static,
    ) -> Self {
        let (queue, replies) = channel::<String>();
        let done = speaking.clone();
        thread::spawn(move || {
            for reply in replies {
                let said = mouth.say(&reply);
                done.end();
                if let Err(why) = said {
                    report(why);
                }
            }
        });
        Self { queue, speaking }
    }

    /// Queue a reply. The microphone is closed from now until it has been played.
    pub fn say(&self, reply: String) {
        self.speaking.begin();
        if self.queue.send(reply).is_err() {
            self.speaking.end();
        }
    }
}

/// The decision log, shared by the worker that runs the shell and the thread
/// that listens, so a transcript is on record before it is shown (ADR 0002)
/// and the request that comes from it says so.
#[derive(Clone)]
pub struct SharedLog {
    log: Arc<Mutex<Box<dyn DecisionLog + Send>>>,
    /// The utterances heard since the last request, oldest first.
    heard: Arc<Mutex<Vec<String>>>,
}

impl SharedLog {
    pub fn new(log: Box<dyn DecisionLog + Send>) -> Self {
        Self {
            log: Arc::new(Mutex::new(log)),
            heard: Arc::default(),
        }
    }

    /// What the recogniser wrote, as an event. It is the model's output, not a decision.
    pub fn transcript(line: &str) -> Value {
        json!({"event": "transcript", "by": "model", "decided": false, "text": line})
    }
}

impl DecisionLog for SharedLog {
    fn record(&mut self, mut event: Value) -> Result<(), String> {
        if let Ok(mut heard) = self.heard.lock() {
            match event["event"].as_str() {
                Some("transcript") => {
                    heard.push(event["text"].as_str().unwrap_or_default().to_owned());
                }
                // A request that is one utterance, or several heard in a row, is
                // spoken; a line with typed words in it, or with a part taken
                // out, is not.
                Some("request") => {
                    let request = event["request"].as_str().unwrap_or_default();
                    let spoken = (0..heard.len()).any(|from| {
                        (from..heard.len()).any(|to| heard[from..=to].join(" ") == request)
                    });
                    heard.clear();
                    if spoken {
                        event["input"] = json!("speech");
                    }
                }
                _ => {}
            }
        }
        self.log
            .lock()
            .map_err(|_| "the decision log is unavailable".to_owned())?
            .record(event)
    }
}

/// Fakes of the four ports, in memory: the gate has no microphone, no speaker,
/// no whisper and no Piper. Public so the interface's tests use them too.
#[cfg(test)]
pub mod loopback {
    use std::sync::{Arc, Mutex};

    use serde_json::Value;

    use super::{AudioCapture, AudioPlayback, DecisionLog, Synthesizer, Transcriber};
    use crate::testing::MemoryLog;

    /// A log that a test can read while another thread writes to it.
    #[derive(Clone, Default)]
    pub struct SharedMemory(pub Arc<Mutex<MemoryLog>>);

    impl DecisionLog for SharedMemory {
        fn record(&mut self, event: Value) -> Result<(), String> {
            self.0.lock().unwrap().record(event)
        }
    }

    impl SharedMemory {
        pub fn events(&self) -> Vec<Value> {
            self.0.lock().unwrap().events.clone()
        }
    }

    /// A WAV that carries `text` and `voice` as its bytes, so a test can see
    /// what reached the synthesiser from what was played.
    pub fn tagged_wav(voice: &str, text: &str) -> Vec<u8> {
        format!("RIFFtestWAVE|{voice}|{text}").into_bytes()
    }

    #[derive(Clone, Default)]
    pub struct FakeSynthesizer {
        pub installed: Arc<Mutex<Vec<String>>>,
        pub calls: Arc<Mutex<Vec<(String, String)>>>,
        pub fails: bool,
        /// What a failure says, when it is not the plain one.
        pub says: Option<String>,
    }

    impl FakeSynthesizer {
        pub fn with(installed: &[&str]) -> Self {
            Self {
                installed: Arc::new(Mutex::new(
                    installed.iter().map(|v| (*v).to_owned()).collect(),
                )),
                ..Self::default()
            }
        }
    }

    impl Synthesizer for FakeSynthesizer {
        fn voices(&self) -> Result<Vec<String>, String> {
            Ok(self.installed.lock().unwrap().clone())
        }

        fn synthesize(&self, voice: &str, text: &str) -> Result<Vec<u8>, String> {
            self.calls
                .lock()
                .unwrap()
                .push((voice.to_owned(), text.to_owned()));
            if self.fails {
                return Err(self
                    .says
                    .clone()
                    .unwrap_or_else(|| "the speech server answered 500".to_owned()));
            }
            Ok(tagged_wav(voice, text))
        }
    }

    #[derive(Clone, Default)]
    pub struct FakePlayback {
        pub played: Arc<Mutex<Vec<Vec<u8>>>>,
    }

    impl AudioPlayback for FakePlayback {
        fn play(&self, wav: &[u8]) -> Result<(), String> {
            self.played.lock().unwrap().push(wav.to_vec());
            Ok(())
        }
    }

    #[derive(Clone, Default)]
    pub struct FakeCapture {
        pub state: Arc<Mutex<Vec<&'static str>>>,
        pub fails_to_start: bool,
    }

    impl AudioCapture for FakeCapture {
        fn start(&mut self) -> Result<(), String> {
            if self.fails_to_start {
                return Err("the microphone cannot be opened".to_owned());
            }
            self.state.lock().unwrap().push("start");
            Ok(())
        }

        fn finish(&mut self) -> Result<Vec<u8>, String> {
            self.state.lock().unwrap().push("finish");
            Ok(b"RIFFutteranceWAVE".to_vec())
        }

        fn cancel(&mut self) {
            self.state.lock().unwrap().push("cancel");
        }
    }

    #[derive(Clone, Default)]
    pub struct FakeTranscriber {
        pub heard: String,
        pub languages: Arc<Mutex<Vec<Option<String>>>>,
    }

    impl FakeTranscriber {
        pub fn hearing(text: &str) -> Self {
            Self {
                heard: text.to_owned(),
                ..Self::default()
            }
        }
    }

    impl Transcriber for FakeTranscriber {
        fn transcribe(&self, _wav: &[u8], language: Option<&str>) -> Result<String, String> {
            self.languages
                .lock()
                .unwrap()
                .push(language.map(str::to_owned));
            Ok(self.heard.clone())
        }
    }
}
