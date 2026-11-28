//! The closed list of spoken voices (ADR 0031, section 2) and the language
//! rules that go with it. Pure data and comparisons: nothing here does input or
//! output, and it imports nothing from the rest of the shell, so the SOUL and
//! the personality (`soul`) can name a voice without depending on the speech
//! adapters.

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

/// Whether the voice may be named for `wanted`: it speaks that language, or the
/// list has no voice of that region at all and this one speaks the same language
/// (the voice `default_voice` would choose). `joe` may be named under `en-GB`.
pub fn voice_may_speak(voice: &SpokenVoice, wanted: &str) -> bool {
    language_fits(voice.language, wanted)
        || (!VOICES.iter().any(|v| language_fits(v.language, wanted))
            && base_language(voice.language) == base_language(wanted))
}

fn base_language(tag: &str) -> &str {
    tag.split('-').next().unwrap_or_default()
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
                // No voice of that region: one of the same language, `en-GB` for `joe`.
                .or_else(|| {
                    VOICES
                        .iter()
                        .find(|voice| base_language(voice.language) == base_language(wanted))
                })
        })
        .unwrap_or(&VOICES[0])
}
