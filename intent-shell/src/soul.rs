//! The SOUL, the personality and the persona they make (ADR 0006, ADR 0028,
//! roadmap S8).
//!
//! Both files are a closed, versioned `key: value` text: every value is a word
//! from a list kept here, or a short name or a language tag, so nothing a file
//! holds is an instruction. Whether a personality contradicts the SOUL is a
//! comparison made here, in code. Nothing in this module is given to the
//! broker, the resolver, the planner or a confirmation: the persona only fills
//! the fixed prompt of the reply (`reply::system_prompt`).

use std::io::Read as _;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use serde_json::{Value, json};

use crate::action::{capped, printable};

/// The version of the file format this shell reads.
pub const FORMAT: u32 = 1;
/// Larger files are invalid; the file is read only this far, never to its end.
pub const MAX_BYTES: usize = 2048;
const MAX_LANGUAGES: usize = 8;
const NAME_LIMIT: usize = 24;
/// How much of a rejected value a message repeats.
const SHOWN_LIMIT: usize = 40;

/// The four ready-made profiles of ADR 0006.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    DirectTechnical,
    WarmDidactic,
    FormalDiscreet,
    GoodHumoured,
}

impl Profile {
    pub const ALL: [Self; 4] = [
        Self::DirectTechnical,
        Self::WarmDidactic,
        Self::FormalDiscreet,
        Self::GoodHumoured,
    ];

    /// What the file holds and the log records.
    pub fn id(self) -> &'static str {
        match self {
            Self::DirectTechnical => "direct-technical",
            Self::WarmDidactic => "warm-didactic",
            Self::FormalDiscreet => "formal-discreet",
            Self::GoodHumoured => "good-humoured",
        }
    }

    /// The fixed line the preset turns into. Code writes it, not the user.
    pub fn voice(self) -> &'static str {
        match self {
            Self::DirectTechnical => {
                "Direct and technical. Short, exact sentences. No greetings, no praise, no filler."
            }
            Self::WarmDidactic => {
                "Warm and didactic. Friendly and patient; explain in one plain sentence why it matters."
            }
            Self::FormalDiscreet => {
                "Formal and discreet. Polite, restrained, third person where natural, no exclamations."
            }
            Self::GoodHumoured => {
                "Good-humoured. Light and playful, at most one small joke, never at the person's expense."
            }
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        let wanted = text.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|profile| profile.id() == wanted)
    }

    /// Where the preset sits on each axis, by default.
    fn defaults(self) -> [u8; 3] {
        match self {
            Self::DirectTechnical => [1, 0, 0],
            Self::WarmDidactic => [1, 1, 2],
            Self::FormalDiscreet => [2, 0, 0],
            Self::GoodHumoured => [0, 2, 1],
        }
    }

    /// How far a personality may move on each axis, unless the SOUL names its own range.
    fn ranges(self) -> [(u8, u8); 3] {
        match self {
            Self::DirectTechnical => [(0, 1), (0, 0), (0, 1)],
            Self::WarmDidactic => [(0, 1), (0, 1), (1, 2)],
            Self::FormalDiscreet => [(1, 2), (0, 0), (0, 1)],
            Self::GoodHumoured => [(0, 1), (1, 2), (0, 2)],
        }
    }
}

/// What a personality may vary besides the name and the language (ADR 0006).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Formality,
    Humour,
    Detail,
}

impl Axis {
    pub const ALL: [Self; 3] = [Self::Formality, Self::Humour, Self::Detail];

    pub fn key(self) -> &'static str {
        match self {
            Self::Formality => "formality",
            Self::Humour => "humour",
            Self::Detail => "detail",
        }
    }

    /// The values, low to high.
    pub fn words(self) -> [&'static str; 3] {
        match self {
            Self::Formality => ["casual", "neutral", "formal"],
            Self::Humour => ["none", "light", "marked"],
            Self::Detail => ["minimal", "brief", "explained"],
        }
    }

    fn level(self, word: &str) -> Option<u8> {
        (0..3u8).find(|&level| self.words()[usize::from(level)] == word)
    }

    /// What the model is told for a value. Fixed text: the file never supplies it.
    fn sentence(self, level: u8) -> &'static str {
        let sentences = match self {
            Self::Formality => [
                "Formality: casual; relaxed, everyday wording.",
                "Formality: neutral; plain wording.",
                "Formality: formal; polite, complete sentences, no slang.",
            ],
            Self::Humour => [
                "Humour: none.",
                "Humour: at most one light touch.",
                "Humour: welcome and the character should show, but stay exact about the facts.",
            ],
            Self::Detail => [
                "Detail: minimal; only the outcome, in as few words as possible.",
                "Detail: brief; the outcome and the one thing that matters most.",
                "Detail: explained; the outcome and, in plain words, why.",
            ],
        };
        sentences[usize::from(level)]
    }

    fn named(self, level: u8) -> &'static str {
        self.words()[usize::from(level)]
    }
}

fn shown(text: &str) -> String {
    format!("\"{}\"", printable(&capped(text, SHOWN_LIMIT), false))
}

/// A name: the machine's in the SOUL, a nickname in a personality.
fn name(value: &str) -> Result<String, String> {
    let words: Vec<&str> = value.split(' ').collect();
    let fits = value.chars().any(char::is_alphabetic)
        && value.chars().count() <= NAME_LIMIT
        && words.len() <= 2
        && words.iter().all(|word| !word.is_empty())
        && value
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '\'' | '.'));
    if fits {
        Ok(value.to_owned())
    } else {
        Err(format!(
            "{} is not a name: up to {NAME_LIMIT} letters, digits, spaces, - ' or ., in at most two words",
            shown(value)
        ))
    }
}

/// `en`, `pt-BR`: two or three lowercase letters, then optionally two capitals.
fn language(value: &str) -> Result<String, String> {
    // `any` is the SOUL's word for no limit, never a language to write in.
    if value == "any" {
        return Err(format!(
            "{} is not a language tag such as en or pt-BR",
            shown(value)
        ));
    }
    let (base, region) = match value.split_once('-') {
        Some((base, region)) => (base, Some(region)),
        None => (value, None),
    };
    let base_ok = (2..=3).contains(&base.len()) && base.bytes().all(|b| b.is_ascii_lowercase());
    let region_ok =
        region.is_none_or(|r| r.len() == 2 && r.bytes().all(|b| b.is_ascii_uppercase()));
    if base_ok && region_ok {
        Ok(value.to_owned())
    } else {
        Err(format!(
            "{} is not a language tag such as en or pt-BR",
            shown(value)
        ))
    }
}

/// The `key: value` pairs of a file, after the `format` line, each with its line
/// number. Anything outside the grammar is an error: nothing is skipped.
fn pairs<'a>(text: &'a str, keys: &[&str]) -> Result<Vec<(usize, &'a str, &'a str)>, String> {
    let mut found: Vec<(usize, &str, &str)> = Vec::new();
    let mut versioned = false;
    for (index, line) in text.lines().enumerate() {
        let (number, line) = (index + 1, line.trim());
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            return Err(format!(
                "line {number}: {} is not \"key: value\"",
                shown(line)
            ));
        };
        let (key, value) = (key.trim(), value.trim());
        if !versioned {
            if key != "format" {
                return Err(format!(
                    "line {number}: the first line must be \"format: {FORMAT}\""
                ));
            }
            if value != FORMAT.to_string() {
                return Err(format!(
                    "line {number}: format {} is not one this shell reads (it reads {FORMAT})",
                    shown(value)
                ));
            }
            versioned = true;
            continue;
        }
        if !keys.contains(&key) {
            return Err(format!(
                "line {number}: {} is not a key here (the keys are {})",
                shown(key),
                keys.join(", ")
            ));
        }
        if found.iter().any(|(_, seen, _)| *seen == key) {
            return Err(format!("line {number}: {key} appears twice"));
        }
        found.push((number, key, value));
    }
    if versioned {
        Ok(found)
    } else {
        Err(format!("there is no \"format: {FORMAT}\" line"))
    }
}

/// `humour: none..light`, or one word for a single value.
fn range(axis: Axis, value: &str) -> Result<(u8, u8), String> {
    let (low, high) = value.split_once("..").unwrap_or((value, value));
    let (Some(low), Some(high)) = (axis.level(low.trim()), axis.level(high.trim())) else {
        return Err(format!(
            "{} is not {} (one of {}, or two of them as low..high)",
            shown(value),
            axis.key(),
            axis.words().join(", ")
        ));
    };
    if low > high {
        return Err(format!(
            "{} is backwards: the low end comes first",
            shown(value)
        ));
    }
    Ok((low, high))
}

fn range_words(axis: Axis, (low, high): (u8, u8)) -> String {
    if low == high {
        axis.named(low).to_owned()
    } else {
        format!("{}..{}", axis.named(low), axis.named(high))
    }
}

/// Who the AI is on this machine: the preset, how far a personality may move
/// from it, the name and the languages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Soul {
    preset: Profile,
    name: Option<String>,
    ranges: [(u8, u8); 3],
    /// `None` is `any`.
    languages: Option<Vec<String>>,
}

impl Soul {
    pub fn preset(preset: Profile) -> Self {
        Self {
            preset,
            name: None,
            ranges: preset.ranges(),
            languages: None,
        }
    }

    /// A file of format 1, or format 0: the one preset name the S8a file held.
    pub fn parse(text: &str) -> Result<Self, String> {
        if let Some(preset) = Profile::parse(text) {
            return Ok(Self::preset(preset));
        }
        let mut soul = Self::preset(Profile::DirectTechnical);
        let mut preset = None;
        let keys = [
            "preset",
            "name",
            "formality",
            "humour",
            "detail",
            "languages",
        ];
        let found = pairs(text, &keys)?;
        for &(number, key, value) in &found {
            let at = |message: String| format!("line {number}: {key}: {message}");
            match key {
                "preset" => {
                    preset = Some(Profile::parse(value).ok_or_else(|| {
                        at(format!(
                            "{} is not one of {}",
                            shown(value),
                            Profile::ALL.map(Profile::id).join(", ")
                        ))
                    })?);
                }
                "name" => soul.name = Some(name(value).map_err(at)?),
                "languages" => soul.languages = Self::languages(value).map_err(at)?,
                _ => {}
            }
        }
        let preset = preset.ok_or("there is no preset line")?;
        soul.preset = preset;
        soul.ranges = preset.ranges();
        for axis in Axis::ALL {
            if let Some(&(number, _, value)) = found.iter().find(|(_, key, _)| *key == axis.key()) {
                soul.ranges[axis as usize] = range(axis, value)
                    .map_err(|e| format!("line {number}: {}: {e}", axis.key()))?;
            }
        }
        Ok(soul)
    }

    fn languages(value: &str) -> Result<Option<Vec<String>>, String> {
        if value == "any" {
            return Ok(None);
        }
        let tags = value
            .split(',')
            .map(|tag| language(tag.trim()))
            .collect::<Result<Vec<_>, _>>()?;
        if tags.len() > MAX_LANGUAGES {
            return Err(format!("at most {MAX_LANGUAGES} languages"));
        }
        Ok(Some(tags))
    }

    fn default_level(&self, axis: Axis) -> u8 {
        let (low, high) = self.ranges[axis as usize];
        self.preset.defaults()[axis as usize].clamp(low, high)
    }

    /// The SOUL on its own, with no personality on top.
    pub fn persona(&self) -> Persona {
        Persona {
            preset: self.preset,
            machine_name: self.name.clone(),
            nickname: None,
            levels: Axis::ALL.map(|axis| self.default_level(axis)),
            language: None,
        }
    }

    /// The SOUL with a personality on top, or why the personality contradicts
    /// it: one reason for each field that does, each naming the field and the
    /// SOUL's limit.
    pub fn with(&self, personality: &Personality) -> Result<Persona, String> {
        let mut persona = self.persona();
        persona.nickname = personality.nickname.clone();
        let mut contradictions = Vec::new();
        for axis in Axis::ALL {
            let Some(level) = personality.levels[axis as usize] else {
                continue;
            };
            let allowed = self.ranges[axis as usize];
            if (allowed.0..=allowed.1).contains(&level) {
                persona.levels[axis as usize] = level;
            } else {
                contradictions.push(format!(
                    "{}: {} is outside the SOUL's range {}",
                    axis.key(),
                    axis.named(level),
                    range_words(axis, allowed)
                ));
            }
        }
        if let Some(tag) = &personality.language {
            match &self.languages {
                Some(allowed) if !allowed.contains(tag) => contradictions.push(format!(
                    "language: {tag} is not among the SOUL's languages {}",
                    allowed.join(", ")
                )),
                _ => persona.language = Some(tag.clone()),
            }
        }
        if contradictions.is_empty() {
            Ok(persona)
        } else {
            Err(format!(
                "it contradicts the SOUL ({}): {}",
                self.preset.id(),
                contradictions.join("; ")
            ))
        }
    }
}

/// What one user chose, before it is checked against the SOUL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Personality {
    nickname: Option<String>,
    levels: [Option<u8>; 3],
    language: Option<String>,
}

impl Personality {
    pub fn parse(text: &str) -> Result<Self, String> {
        let keys = ["name", "formality", "humour", "detail", "language"];
        let mut personality = Self {
            nickname: None,
            levels: [None; 3],
            language: None,
        };
        for (number, key, value) in pairs(text, &keys)? {
            let at = |message: String| format!("line {number}: {key}: {message}");
            match key {
                "name" => personality.nickname = Some(name(value).map_err(at)?),
                "language" => personality.language = Some(language(value).map_err(at)?),
                _ => {
                    let axis = Axis::ALL.into_iter().find(|axis| axis.key() == key);
                    if let Some(axis) = axis {
                        personality.levels[axis as usize] =
                            Some(axis.level(value).ok_or_else(|| {
                                at(format!(
                                    "{} is not one of {}",
                                    shown(value),
                                    axis.words().join(", ")
                                ))
                            })?);
                    }
                }
            }
        }
        Ok(personality)
    }
}

/// The SOUL and the personality, resolved. The only thing the reply prompt is
/// built from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Persona {
    pub preset: Profile,
    pub machine_name: Option<String>,
    pub nickname: Option<String>,
    levels: [u8; 3],
    pub language: Option<String>,
}

impl Persona {
    /// A preset with nothing else: the S8a profile.
    pub fn of(preset: Profile) -> Self {
        Soul::preset(preset).persona()
    }

    /// The voice lines of the prompt, one per line, all of them fixed text but
    /// the names and the language tag, which passed the checks above.
    pub fn voice_lines(&self) -> String {
        let mut lines = vec![format!("Voice: {}", self.preset.voice())];
        lines.push(
            "Where the settings below differ from the voice line, follow the settings.".to_owned(),
        );
        lines.extend(
            Axis::ALL
                .into_iter()
                .map(|axis| axis.sentence(self.levels[axis as usize]).to_owned()),
        );
        if let Some(tag) = &self.language {
            lines.push(format!(
                "Write the reply in the language with tag {}.",
                json!(tag)
            ));
        }
        if let Some(own) = &self.machine_name {
            lines.push(format!("Your name is {}.", json!(own)));
        }
        if let Some(nickname) = &self.nickname {
            lines.push(format!("The person calls you {}.", json!(nickname)));
        }
        lines.join("\n")
    }

    /// What the decision log records of the persona: the words, never free text.
    pub fn describe(&self) -> Value {
        let mut value = json!({"profile": self.preset.id()});
        for axis in Axis::ALL {
            value[axis.key()] = json!(axis.named(self.levels[axis as usize]));
        }
        value["language"] = json!(self.language);
        value["name"] = json!(self.machine_name);
        value["nickname"] = json!(self.nickname);
        value
    }
}

/// Where a file's content came from, for the log and for the person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// No file was asked for and none exists.
    Default,
    File,
    /// A file that exists but cannot be used: the fallback, and why.
    DefaultAfterProblem(String),
}

/// The persona of a session and where each of its two files came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub persona: Persona,
    /// The SOUL alone, which the `voice_profile` event records.
    pub soul_persona: Persona,
    pub soul: Source,
    pub personality: Source,
}

enum Read {
    Absent,
    Text(String),
    Problem(String),
}

/// The path as the messages write it: escaped, like every text the person did not type here.
fn shown_path(path: &Path) -> String {
    printable(&path.to_string_lossy(), false)
}

/// Only the start is read: a device or a huge file must not be read to its end.
/// Non-blocking, so a named pipe with no writer is an empty file, not a hang.
/// A file the person named that does not exist is an error: they asked for it.
fn read(path: &Path, what: &str, named_by_person: bool) -> Result<Read, String> {
    let bytes = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .and_then(|file| {
            let mut bytes = Vec::new();
            file.take(MAX_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .map(|_| bytes)
        });
    let at = shown_path(path);
    match bytes {
        Ok(bytes) if bytes.len() > MAX_BYTES => Ok(Read::Problem(format!(
            "the {what} {at} is larger than {MAX_BYTES} bytes"
        ))),
        // An editor may start the file with a byte-order mark; it is not text.
        Ok(bytes) => Ok(String::from_utf8(bytes).map_or_else(
            |_| Read::Problem(format!("the {what} {at} is not UTF-8 text")),
            |text| Read::Text(text.trim_start_matches('\u{feff}').to_owned()),
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if named_by_person {
                Err(format!("the {what} {at} does not exist"))
            } else {
                Ok(Read::Absent)
            }
        }
        Err(e) => Ok(Read::Problem(format!("cannot read the {what} {at}: {e}"))),
    }
}

/// The SOUL and the personality of this session, each as a path and whether the
/// person named it. A SOUL that is missing is `direct-technical`, silently; one
/// that is there but cannot be used is `direct-technical` with a message naming
/// the file, because a typo must not be mistaken for the voice that was wanted.
/// A personality the person named that cannot be used stops the shell; one at
/// the default path is dropped, with a message, and the SOUL stands alone.
pub fn load(soul: (&Path, bool), personality: (&Path, bool)) -> Result<Loaded, String> {
    let fallback = Soul::preset(Profile::DirectTechnical);
    let (soul_value, soul_source) = match read(soul.0, "SOUL", soul.1)? {
        Read::Absent => (fallback, Source::Default),
        Read::Problem(problem) => (
            fallback,
            Source::DefaultAfterProblem(format!("{problem}; using direct-technical")),
        ),
        Read::Text(text) => match Soul::parse(&text) {
            Ok(parsed) => (parsed, Source::File),
            Err(why) => (
                fallback,
                Source::DefaultAfterProblem(format!(
                    "the SOUL {}: {why}; using direct-technical",
                    shown_path(soul.0)
                )),
            ),
        },
    };
    let alone = soul_value.persona();
    let soul_persona = alone.clone();
    let soul_problem = match &soul_source {
        Source::DefaultAfterProblem(problem) => Some(problem.as_str()),
        _ => None,
    };
    let (persona, personality_source) = match read(personality.0, "personality", personality.1)? {
        Read::Absent => (alone, Source::Default),
        Read::Problem(problem) => dropped(problem, personality.1, alone, soul_problem)?,
        Read::Text(text) => match Personality::parse(&text).and_then(|p| soul_value.with(&p)) {
            Ok(persona) => (persona, Source::File),
            Err(why) => dropped(
                format!("the personality {}: {why}", shown_path(personality.0)),
                personality.1,
                alone,
                soul_problem,
            )?,
        },
    };
    Ok(Loaded {
        persona,
        soul_persona,
        soul: soul_source,
        personality: personality_source,
    })
}

/// A personality that cannot be used. When a SOUL that could not be used is why,
/// the person is told that first: the personality was checked against a SOUL they never wrote.
fn dropped(
    problem: String,
    named: bool,
    alone: Persona,
    soul_problem: Option<&str>,
) -> Result<(Persona, Source), String> {
    if named {
        return Err(match soul_problem {
            Some(soul) => format!("{soul}\n{problem}"),
            None => problem,
        });
    }
    Ok((
        alone,
        Source::DefaultAfterProblem(format!("{problem}; using the SOUL alone")),
    ))
}
