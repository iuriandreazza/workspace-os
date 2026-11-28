//! Speech (ADR 0031, roadmap S11-2): the closed list of voices, `voice` in a
//! personality of format 2, the ports with their fakes, the loopback adapters
//! against stand-in servers, and the boundary: a transcript is text that waits
//! for Enter, the system does not hear itself, only the cleaned reply is spoken
//! and a voice never reaches a prompt or a decision.

use std::net::TcpListener;
use std::sync::mpsc::channel;
use std::time::Duration;

use serde_json::{Value, json};

use crate::ports::{DecisionLog, Synthesizer, Transcriber};
use crate::reply::{self, REPLY_LIMIT};
use crate::soul::{Persona, Personality, Profile, Soul};
use crate::speech::loopback::{
    FakeCapture, FakePlayback, FakeSynthesizer, FakeTranscriber, HeldPlayback, SharedMemory, http,
    stub, tagged_wav,
};
use crate::speech::{
    Ear, Mouth, SharedLog, Speaking, Talker, VOICES, default_voice, language_fits, speakable,
    transcript_line, voice,
};
use crate::speech_http::{PiperClient, WhisperClient, require_loopback};

const SOUL: &str = "format: 1\npreset: warm-didactic\nlanguages: en, pt-BR\n";

fn persona_with(personality: &str) -> Persona {
    Soul::parse(SOUL)
        .unwrap()
        .with(&Personality::parse(personality).unwrap())
        .unwrap()
}

fn contradiction(soul: &str, personality: &str) -> String {
    Soul::parse(soul)
        .unwrap()
        .with(&Personality::parse(personality).unwrap())
        .unwrap_err()
}

// ---- the list and the personality ------------------------------------------

#[test]
fn the_list_is_closed_and_every_entry_records_a_hash_and_a_name() {
    assert_eq!(VOICES.map(|v| v.id), ["cadu", "jeff", "faber", "joe"]);
    for voice in &VOICES {
        assert_eq!(voice.sha256.len(), 64, "{}", voice.id);
        assert!(voice.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
        let language = voice.language.replace('-', "_");
        assert_eq!(voice.piper, format!("{language}-{}-medium", voice.id));
    }
    // Only the voice trained from a CC BY corpus owes an attribution.
    let owing: Vec<_> = VOICES.iter().filter(|v| v.attribution.is_some()).collect();
    assert_eq!(owing.len(), 1);
    assert_eq!(owing[0].id, "faber");
    assert!(voice("lessac").is_none() && voice("").is_none() && voice("CADU").is_none());
}

#[test]
fn a_voice_needs_format_two_and_a_name_from_the_list() {
    let personality = Personality::parse("format: 2\nvoice: jeff\nname: Dex\n").unwrap();
    let persona = Soul::parse(SOUL).unwrap().with(&personality).unwrap();
    assert_eq!(persona.spoken_voice().id, "jeff");
    // Format 1 is read as before and has no voice.
    assert!(Personality::parse("format: 1\nname: Dex\n").is_ok());
    for (text, message) in [
        ("format: 1\nvoice: jeff\n", "a voice needs \"format: 2\""),
        (
            "format: 2\nvoice: robot\n",
            "is not a voice (one of cadu, jeff, faber, joe)",
        ),
        (
            "format: 2\nvoice: jeff\nvoice: joe\n",
            "voice appears twice",
        ),
        ("format: 3\nvoice: jeff\n", "it reads 1 or 2"),
    ] {
        let error = Personality::parse(text).unwrap_err();
        assert!(error.contains(message), "{text:?}: {error}");
    }
    // The SOUL stays format 1 and rejects a voice.
    assert!(Soul::parse("format: 2\npreset: warm-didactic\n").is_err());
    assert!(Soul::parse("format: 1\npreset: warm-didactic\nvoice: jeff\n").is_err());
}

#[test]
fn a_voice_in_a_language_the_soul_does_not_allow_contradicts_it() {
    let narrow = "format: 1\npreset: warm-didactic\nlanguages: en\n";
    let error = contradiction(narrow, "format: 2\nvoice: cadu\n");
    assert!(
        error.contains("voice: cadu speaks pt-BR, which is not among the SOUL's languages en"),
        "{error}"
    );
    // `any` limits nothing, and `en` allows the en-US voice.
    for soul in ["format: 1\npreset: warm-didactic\nlanguages: any\n", narrow] {
        let allowed = Soul::parse(soul).unwrap();
        let personality = Personality::parse("format: 2\nvoice: joe\n").unwrap();
        assert_eq!(allowed.with(&personality).unwrap().spoken_voice().id, "joe");
    }
    // A voice of another language than any the SOUL allows is a contradiction,
    // even where the list has no voice of the SOUL's region.
    let spanish = "format: 1\npreset: warm-didactic\nlanguages: es\n";
    assert!(contradiction(spanish, "format: 2\nvoice: cadu\n").contains("voice: cadu"));
}

#[test]
fn a_voice_may_be_named_where_it_is_the_default_for_a_region_the_list_has_no_voice_for() {
    // `joe` is what a personality with no voice gets under `en-GB`, so naming it is no contradiction.
    let gb = Soul::parse("format: 1\npreset: warm-didactic\nlanguages: en-GB\n").unwrap();
    let named = gb
        .with(&Personality::parse("format: 2\nvoice: joe\n").unwrap())
        .unwrap();
    assert_eq!(named.spoken_voice().id, "joe");
    let portugal = Soul::parse("format: 1\npreset: warm-didactic\nlanguages: pt-PT\n").unwrap();
    let cadu = Personality::parse("format: 2\nlanguage: pt-PT\nvoice: cadu\n").unwrap();
    assert_eq!(portugal.with(&cadu).unwrap().spoken_voice().id, "cadu");
    // A voice of another language is still a contradiction.
    assert!(
        gb.with(&Personality::parse("format: 2\nvoice: cadu\n").unwrap())
            .is_err()
    );
    // And where the list has a voice for the region, only those fit: `en-US` has `joe` only.
    let us = Soul::parse("format: 1\npreset: warm-didactic\nlanguages: pt-BR\n").unwrap();
    assert!(
        us.with(&Personality::parse("format: 2\nvoice: joe\n").unwrap())
            .is_err()
    );
}

#[test]
fn a_voice_that_does_not_match_the_personalitys_own_language_contradicts_it() {
    let error = contradiction(SOUL, "format: 2\nlanguage: en\nvoice: cadu\n");
    assert!(
        error.contains("voice: cadu speaks pt-BR, not the personality's language en"),
        "{error}"
    );
    assert!(error.starts_with("it contradicts the SOUL (warm-didactic)"));
    let fits = persona_with("format: 2\nlanguage: pt-BR\nvoice: faber\n");
    assert_eq!(fits.spoken_voice().id, "faber");
    // Rejected whole: a personality that contradicts only in its voice sets nothing.
    assert!(
        Soul::parse(SOUL)
            .unwrap()
            .with(&Personality::parse("format: 2\nname: Dex\nlanguage: en\nvoice: cadu\n").unwrap())
            .is_err()
    );
}

#[test]
fn a_personality_with_no_voice_is_spoken_in_the_first_voice_of_the_reply_language() {
    assert!(language_fits("pt-BR", "pt"));
    assert!(language_fits("pt-BR", "pt-BR"));
    assert!(!language_fits("pt-BR", "pt-PT"));
    assert!(language_fits("en-US", "en"));
    assert!(!language_fits("en-US", "pt"));
    assert_eq!(default_voice(Some("pt-BR")).id, "cadu");
    assert_eq!(default_voice(Some("en")).id, "joe");
    assert_eq!(default_voice(Some("fr")).id, "cadu");
    // A region with no voice of its own gets a voice of the same language.
    assert_eq!(default_voice(Some("en-GB")).id, "joe");
    assert_eq!(default_voice(Some("pt-PT")).id, "cadu");
    assert_eq!(default_voice(None).id, "cadu");
    // The reply language is the personality's, else the first the SOUL allows.
    // The SOUL of these tests lists `en` first.
    assert_eq!(
        persona_with("format: 1\nname: Dex\n").spoken_voice().id,
        "joe"
    );
    let portuguese =
        Soul::parse("format: 1\npreset: warm-didactic\nlanguages: pt-BR, en\n").unwrap();
    assert_eq!(portuguese.persona().spoken_voice().id, "cadu");
    assert_eq!(
        persona_with("format: 1\nlanguage: en\n").spoken_voice().id,
        "joe"
    );
    let english = Soul::parse("format: 1\npreset: warm-didactic\nlanguages: en\n").unwrap();
    assert_eq!(english.persona().spoken_voice().id, "joe");
    let anything = Soul::parse("format: 1\npreset: warm-didactic\nlanguages: any\n").unwrap();
    assert_eq!(anything.persona().spoken_voice().id, "cadu");
}

#[test]
fn two_users_with_two_ids_are_spoken_to_in_two_voices_at_the_synthesiser() {
    let synthesizer = FakeSynthesizer::with(&["pt_BR-cadu-medium", "pt_BR-jeff-medium"]);
    let (ana, bruno) = (
        persona_with("format: 2\nvoice: cadu\n"),
        persona_with("format: 2\nvoice: jeff\n"),
    );
    for persona in [&ana, &bruno] {
        let mouth = Mouth::new(
            Box::new(synthesizer.clone()),
            Box::new(FakePlayback::default()),
            persona.spoken_voice(),
            Speaking::default(),
        );
        mouth.say("Done.").unwrap();
    }
    let calls = synthesizer.calls.lock().unwrap().clone();
    assert_eq!(
        calls,
        [
            ("pt_BR-cadu-medium".to_owned(), "Done.".to_owned()),
            ("pt_BR-jeff-medium".to_owned(), "Done.".to_owned()),
        ]
    );
}

#[test]
fn the_voice_never_reaches_a_prompt_the_facts_or_the_persona_the_log_records() {
    let plain = persona_with("format: 1\nname: Dex\nlanguage: pt-BR\n");
    for spoken in VOICES.iter().filter(|v| v.language == "pt-BR") {
        let with_voice = persona_with(&format!(
            "format: 2\nname: Dex\nlanguage: pt-BR\nvoice: {}\n",
            spoken.id
        ));
        assert_eq!(with_voice.spoken_voice().id, spoken.id);
        assert_eq!(
            reply::system_prompt(&with_voice),
            reply::system_prompt(&plain),
            "{}",
            spoken.id
        );
        assert_eq!(with_voice.describe(), plain.describe());
        assert_eq!(with_voice.voice_lines(), plain.voice_lines());
        let prompt = reply::system_prompt(&with_voice);
        assert!(!prompt.contains(spoken.piper) && !prompt.contains(spoken.id));
    }
}

// ---- the mouth --------------------------------------------------------------

fn mouth(synthesizer: &FakeSynthesizer, playback: &FakePlayback) -> Mouth {
    Mouth::new(
        Box::new(synthesizer.clone()),
        Box::new(playback.clone()),
        voice("cadu").unwrap(),
        Speaking::default(),
    )
}

#[test]
fn the_reply_is_synthesised_in_the_voice_and_played() {
    let (synthesizer, playback) = (
        FakeSynthesizer::with(&["pt_BR-cadu-medium"]),
        FakePlayback::default(),
    );
    mouth(&synthesizer, &playback)
        .say("  Pronto,   abri a pasta.  ")
        .unwrap();
    assert_eq!(
        synthesizer.calls.lock().unwrap().as_slice(),
        [(
            "pt_BR-cadu-medium".to_owned(),
            "Pronto,   abri a pasta.".to_owned()
        )]
    );
    assert_eq!(
        playback.played.lock().unwrap().as_slice(),
        [tagged_wav("pt_BR-cadu-medium", "Pronto,   abri a pasta.")]
    );
}

#[test]
fn a_voice_the_server_does_not_have_is_never_replaced_by_its_default() {
    // Piper answers an unknown voice with its default one; so the voice is checked first.
    let (synthesizer, playback) = (
        FakeSynthesizer::with(&["pt_BR-jeff-medium"]),
        FakePlayback::default(),
    );
    let error = mouth(&synthesizer, &playback).say("Done.").unwrap_err();
    assert!(
        error.contains("the voice cadu (pt_BR-cadu-medium) is not installed"),
        "{error}"
    );
    assert!(synthesizer.calls.lock().unwrap().is_empty());
    assert!(playback.played.lock().unwrap().is_empty());
}

#[test]
fn a_failure_of_speech_is_a_line_and_nothing_is_played() {
    let synthesizer = FakeSynthesizer {
        fails: true,
        ..FakeSynthesizer::with(&["pt_BR-cadu-medium"])
    };
    let playback = FakePlayback::default();
    let error = mouth(&synthesizer, &playback).say("Done.").unwrap_err();
    assert!(error.starts_with("speech is off for this reply"), "{error}");
    assert!(!error.contains('\n'));
    assert!(playback.played.lock().unwrap().is_empty());
}

#[test]
fn what_a_server_writes_in_an_error_reaches_the_person_as_one_escaped_capped_line() {
    let hostile = format!(
        "server answered 500: <html>\n\u{1b}[2J{}\n</html>",
        "x".repeat(5000)
    );
    let synthesizer = FakeSynthesizer {
        fails: true,
        says: Some(hostile),
        ..FakeSynthesizer::with(&["pt_BR-cadu-medium"])
    };
    let error = mouth(&synthesizer, &FakePlayback::default())
        .say("Done.")
        .unwrap_err();
    assert!(
        !error.contains('\n') && !error.contains('\u{1b}'),
        "{error:?}"
    );
    assert!(error.chars().count() < 300, "{}", error.len());
    assert!(error.contains("server answered 500"));
}

#[test]
fn the_recogniser_is_told_the_reply_language_and_never_the_language_of_the_voice() {
    // A SOUL that lists en first and a personality that picks a Portuguese voice:
    // the voice reaches only the speech call, so the language is still `en`.
    assert_eq!(
        persona_with("format: 2\nvoice: cadu\n").listen_language(),
        Some("en")
    );
    assert_eq!(
        persona_with("format: 1\nname: Dex\n").listen_language(),
        Some("en")
    );
    assert_eq!(
        persona_with("format: 1\nlanguage: pt-BR\n").listen_language(),
        Some("pt-BR")
    );
    assert_eq!(
        persona_with("format: 2\nlanguage: pt-BR\nvoice: cadu\n").listen_language(),
        Some("pt-BR")
    );
    let anything = Soul::parse("format: 1\npreset: warm-didactic\nlanguages: any\n").unwrap();
    assert_eq!(anything.persona().listen_language(), None);
    let voiced = anything
        .with(&Personality::parse("format: 2\nvoice: joe\n").unwrap())
        .unwrap();
    assert_eq!(voiced.listen_language(), None);
}

#[test]
fn nothing_is_said_for_an_empty_reply_and_no_more_than_the_cap_for_a_long_one() {
    assert_eq!(speakable("   "), None);
    let long = "a ".repeat(REPLY_LIMIT);
    assert!(speakable(&long).unwrap().chars().count() <= REPLY_LIMIT);
    let (synthesizer, playback) = (
        FakeSynthesizer::with(&["pt_BR-cadu-medium"]),
        FakePlayback::default(),
    );
    mouth(&synthesizer, &playback).say(" ").unwrap();
    assert!(synthesizer.calls.lock().unwrap().is_empty());
}

#[test]
fn the_microphone_stays_shut_from_the_moment_a_reply_is_queued_until_it_has_been_played() {
    let (started_tx, started) = channel();
    let (release_tx, release) = channel();
    let (reported_tx, reported) = channel();
    let speaking = Speaking::default();
    let mouth = Mouth::new(
        Box::new(FakeSynthesizer::with(&["pt_BR-cadu-medium"])),
        Box::new(HeldPlayback::new(started_tx, release)),
        voice("cadu").unwrap(),
        speaking.clone(),
    );
    let talker = Talker::spawn(mouth, move |why| {
        reported_tx.send(why).unwrap();
    });
    let capture = FakeCapture::default();
    let mut ear = Ear::new(
        Box::new(capture.clone()),
        Box::new(FakeTranscriber::hearing("x")),
        speaking.clone(),
    );
    assert!(ear.start().is_ok());
    ear.cancel();
    talker.say("Done.".to_owned());
    // Queued: shut already, before playback begins.
    assert!(speaking.active());
    assert!(ear.start().unwrap_err().contains("the system is speaking"));
    started.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(speaking.active() && ear.start().is_err());
    release_tx.send(()).unwrap();
    for _ in 0..200 {
        if !speaking.active() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!speaking.active());
    assert!(ear.start().is_ok());
    // The recording was started twice only: once before and once after.
    assert_eq!(
        capture.state.lock().unwrap().as_slice(),
        ["start", "cancel", "start"]
    );
    assert!(reported.try_recv().is_err());
}

#[test]
fn a_voice_that_is_behind_drops_replies_instead_of_queueing_them_without_end() {
    let (started_tx, started) = channel();
    let (release_tx, release) = channel();
    let (reported_tx, reported) = channel();
    let speaking = Speaking::default();
    let mouth = Mouth::new(
        Box::new(FakeSynthesizer::with(&["pt_BR-cadu-medium"])),
        Box::new(HeldPlayback::new(started_tx, release)),
        voice("cadu").unwrap(),
        speaking.clone(),
    );
    let talker = Talker::spawn(mouth, move |why| {
        reported_tx.send(why).unwrap();
    });
    for n in 0..5 {
        talker.say(format!("reply {n}"));
    }
    started.recv_timeout(Duration::from_secs(10)).unwrap();
    // Three are held (one playing, two waiting); the other two were dropped with a line.
    for _ in 0..2 {
        let why = reported.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(why.contains("the voice is behind"), "{why}");
    }
    assert!(reported.try_recv().is_err());
    for _ in 0..3 {
        release_tx.send(()).unwrap();
    }
    for _ in 0..300 {
        if !speaking.active() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!speaking.active());
}

#[test]
fn a_failure_while_speaking_is_reported_and_reopens_the_microphone() {
    let (reported_tx, reported) = channel();
    let speaking = Speaking::default();
    let synthesizer = FakeSynthesizer {
        fails: true,
        ..FakeSynthesizer::with(&["pt_BR-cadu-medium"])
    };
    let talker = Talker::spawn(
        Mouth::new(
            Box::new(synthesizer.clone()),
            Box::new(FakePlayback::default()),
            voice("cadu").unwrap(),
            speaking.clone(),
        ),
        move |why| reported_tx.send(why).unwrap(),
    );
    talker.say("Done.".to_owned());
    let why = reported.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(why.starts_with("speech is off for this reply"));
    for _ in 0..200 {
        if !speaking.active() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!speaking.active());
}

// ---- the ear ----------------------------------------------------------------

fn ear(capture: &FakeCapture, transcriber: &FakeTranscriber) -> Ear {
    Ear::new(
        Box::new(capture.clone()),
        Box::new(transcriber.clone()),
        Speaking::default(),
    )
}

#[test]
fn what_was_heard_is_one_escaped_line_with_the_reply_language_passed_on() {
    let transcriber = FakeTranscriber::hearing(" show the\nlargest files \u{1b}[2J in downloads ");
    let capture = FakeCapture::default();
    let mut ear = ear(&capture, &transcriber);
    ear.start().unwrap();
    let line = ear.finish(Some("pt-BR")).unwrap();
    assert!(!line.contains('\n') && !line.contains('\u{1b}'), "{line:?}");
    assert!(line.starts_with("show the largest files"), "{line}");
    assert_eq!(
        transcriber.languages.lock().unwrap().as_slice(),
        [Some("pt-BR".to_owned())]
    );
}

#[test]
fn silence_and_tags_are_nothing_heard_and_a_long_transcript_is_capped() {
    for raw in [
        "",
        "   ",
        "[BLANK_AUDIO][Music]",
        "[BLANK_AUDIO]",
        " (silêncio) ",
        "[ Silence ]",
    ] {
        assert_eq!(transcript_line(raw), None, "{raw:?}");
    }
    assert_eq!(
        transcript_line("open [the] folder").unwrap(),
        "open [the] folder"
    );
    // Tags around real speech are dropped; the speech is not.
    assert_eq!(
        transcript_line("[Music] list the largest files [Music]").unwrap(),
        "list the largest files"
    );
    assert_eq!(
        transcript_line("[BLANK_AUDIO] list downloads [Music]").unwrap(),
        "list downloads"
    );
    // Tags that touch each other are all dropped, a bracket with a digit is not.
    assert_eq!(transcript_line("[Music][2] list").unwrap(), "[2] list");
    assert_eq!(transcript_line("list [2][Music]").unwrap(), "list [2]");
    // A bracket is a tag only as a word of its own with no digit in it.
    for speech in [
        "set the array a[0]",
        "open file [1]",
        "[2] list the files",
        "read a[i]",
    ] {
        assert_eq!(transcript_line(speech).unwrap(), speech);
    }
    // A parenthesis is a tag only when it is the whole line.
    for speech in [
        "Show me the folder (downloads)",
        "(1) list the files",
        "(a) the first and (b)",
    ] {
        assert_eq!(transcript_line(speech).unwrap(), speech);
    }
    let long = "word ".repeat(1000);
    assert!(transcript_line(&long).unwrap().chars().count() <= 500);
    let transcriber = FakeTranscriber::hearing("[BLANK_AUDIO]");
    let mut ear = ear(&FakeCapture::default(), &transcriber);
    ear.start().unwrap();
    assert_eq!(ear.finish(None).unwrap_err(), "nothing was heard");
}

#[test]
fn a_microphone_that_cannot_be_opened_says_why_and_nothing_is_transcribed() {
    let capture = FakeCapture {
        fails_to_start: true,
        ..FakeCapture::default()
    };
    let transcriber = FakeTranscriber::hearing("x");
    let mut ear = ear(&capture, &transcriber);
    assert!(ear.start().unwrap_err().contains("cannot be opened"));
    assert!(transcriber.languages.lock().unwrap().is_empty());
}

// ---- the decision log -------------------------------------------------------

#[test]
fn a_request_that_is_the_transcript_as_heard_says_it_came_from_speech() {
    let memory = SharedMemory::default();
    let mut log = SharedLog::new(Box::new(memory.clone()));
    log.record(SharedLog::transcript("list downloads")).unwrap();
    log.record(json!({"event": "request", "request": "list downloads"}))
        .unwrap();
    // An edited line was typed; and a second request is not the same utterance.
    log.record(SharedLog::transcript("list downloads")).unwrap();
    log.record(json!({"event": "request", "request": "list documents"}))
        .unwrap();
    log.record(json!({"event": "request", "request": "list downloads"}))
        .unwrap();
    // Two utterances in one input line: the request is all of it, and was spoken.
    log.record(SharedLog::transcript("list")).unwrap();
    log.record(SharedLog::transcript("downloads")).unwrap();
    log.record(json!({"event": "request", "request": "list downloads"}))
        .unwrap();
    // Erased from the input line: the next request is not it, even if typed alike.
    log.record(SharedLog::transcript("delete the folder"))
        .unwrap();
    log.forget();
    log.record(json!({"event": "request", "request": "delete the folder"}))
        .unwrap();
    // An utterance thrown away and said again: the request is the second one.
    log.record(SharedLog::transcript("list files")).unwrap();
    log.record(SharedLog::transcript("list the files")).unwrap();
    log.record(json!({"event": "request", "request": "list the files"}))
        .unwrap();
    let events = memory.events();
    let inputs: Vec<_> = events
        .iter()
        .filter(|e| e["event"] == "request")
        .map(|e| e.get("input").cloned())
        .collect();
    assert_eq!(
        inputs,
        [
            Some(json!("speech")),
            None,
            None,
            Some(json!("speech")),
            None,
            Some(json!("speech"))
        ]
    );
    assert_eq!(events[0]["by"], "model");
    assert_eq!(events[0]["decided"], false);
}

// ---- the adapters, against servers made for the test ------------------------

fn wav(extra: &str) -> Vec<u8> {
    let mut bytes = b"RIFF\x24\0\0\0WAVEfmt ".to_vec();
    bytes.extend_from_slice(extra.as_bytes());
    bytes
}

#[test]
fn a_request_for_the_recogniser_is_one_multipart_file_and_its_text_comes_back() {
    let server = stub(vec![http(
        "200 OK",
        "application/json",
        br#"{"text":" Mostre o tamanho."}"#,
    )]);
    let client = WhisperClient::new(&server.address).unwrap();
    let text = client.transcribe(&wav("audio"), Some("pt-BR")).unwrap();
    assert_eq!(text, " Mostre o tamanho.");
    let request = String::from_utf8_lossy(&server.requests.lock().unwrap()[0]).into_owned();
    assert!(request.starts_with("POST /inference HTTP/1.1"), "{request}");
    assert!(request.contains("multipart/form-data; boundary="));
    assert!(request.contains("name=\"file\"; filename=\"utterance.wav\""));
    assert!(request.contains("RIFF") && request.contains("audio"));
    assert!(
        request.contains("name=\"language\"\r\n\r\npt\r\n"),
        "{request}"
    );
    assert!(request.contains("name=\"response_format\"\r\n\r\njson"));
}

#[test]
fn the_recogniser_failing_in_any_way_is_an_error_not_text() {
    for answer in [
        http("500 Internal Server Error", "text/plain", b"boom"),
        http("200 OK", "text/html", b"<html>not json</html>"),
        http("200 OK", "application/json", br#"{"error":"no model"}"#),
        b"HTTP/1.1 200 OK\r\nContent-Length: 40\r\n\r\n{\"text\":\"cut".to_vec(),
    ] {
        let server = stub(vec![answer]);
        let client = WhisperClient::new(&server.address).unwrap();
        assert!(client.transcribe(&wav("a"), None).is_err());
    }
}

#[test]
fn piper_lists_its_voices_and_synthesises_one_by_name() {
    let voices = json!({"pt_BR-cadu-medium": {}, "en_US-joe-medium": {}}).to_string();
    let audio = wav("speech");
    let server = stub(vec![
        http("200 OK", "application/json", voices.as_bytes()),
        http("200 OK", "audio/wav", &audio),
    ]);
    let client = PiperClient::new(&server.address).unwrap();
    let mut listed = client.voices().unwrap();
    listed.sort();
    assert_eq!(listed, ["en_US-joe-medium", "pt_BR-cadu-medium"]);
    assert_eq!(
        client.synthesize("pt_BR-cadu-medium", "Olá.").unwrap(),
        audio
    );
    let requests = server.requests.lock().unwrap().clone();
    assert!(String::from_utf8_lossy(&requests[0]).starts_with("GET /voices HTTP/1.1"));
    let asked = String::from_utf8_lossy(&requests[1]).into_owned();
    assert!(asked.starts_with("POST /synthesize HTTP/1.1"), "{asked}");
    let body: Value = serde_json::from_str(asked.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body, json!({"text": "Olá.", "voice": "pt_BR-cadu-medium"}));
}

#[test]
fn what_piper_answers_must_be_audio_and_whole() {
    for answer in [
        http("200 OK", "text/html", b"<html>405</html>"),
        http(
            "500 Internal Server Error",
            "text/plain",
            b"No text provided",
        ),
        // The connection closes before the promised length: a cut file is not played.
        {
            let mut cut = http("200 OK", "audio/wav", &wav("speech"));
            cut.truncate(cut.len() - 3);
            cut
        },
    ] {
        let server = stub(vec![answer]);
        let client = PiperClient::new(&server.address).unwrap();
        assert!(client.synthesize("pt_BR-cadu-medium", "x").is_err());
    }
}

#[test]
fn a_voice_missing_on_the_real_adapter_is_refused_before_anything_is_synthesised() {
    let voices = json!({"pt_BR-jeff-medium": {}}).to_string();
    let server = stub(vec![http("200 OK", "application/json", voices.as_bytes())]);
    let mouth = Mouth::new(
        Box::new(PiperClient::new(&server.address).unwrap()),
        Box::new(FakePlayback::default()),
        voice("cadu").unwrap(),
        Speaking::default(),
    );
    assert!(mouth.say("Done.").unwrap_err().contains("is not installed"));
    // Only the list was asked for: no synthesis request, so no default voice was used.
    assert_eq!(server.requests.lock().unwrap().len(), 1);
}

/// Nothing listens on port 1 of the loopback interface (a port bound and then
/// dropped could be taken by another test before the connection).
const NOTHING: &str = "127.0.0.1:1";

#[test]
fn nothing_listening_is_an_error_at_once_for_both_servers() {
    let whisper = WhisperClient::new(NOTHING).unwrap();
    assert!(
        whisper
            .transcribe(&wav("a"), None)
            .unwrap_err()
            .contains("cannot connect")
    );
    assert!(whisper.ping().unwrap_err().contains("does not answer"));
    let piper = PiperClient::new(NOTHING).unwrap();
    assert!(piper.voices().unwrap_err().contains("cannot connect"));
    assert!(
        piper
            .synthesize("pt_BR-cadu-medium", "x")
            .unwrap_err()
            .contains("cannot connect")
    );
}

/// A server that accepts and never answers, until the test is over.
fn silent() -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let held = std::thread::spawn(move || {
        let mut streams = Vec::new();
        while let Ok((stream, _)) = listener.accept() {
            streams.push(stream);
            if streams.len() >= 8 {
                break;
            }
        }
        std::thread::sleep(Duration::from_secs(20));
    });
    (address, held)
}

#[test]
fn a_server_that_never_reads_cannot_hold_a_recording_being_sent_past_the_timeout() {
    let (address, held) = silent();
    // 48 MB: far more than the buffers a loopback socket autotunes to, unlike the 960 KB of 30 s of audio.
    let recording = wav(&"x".repeat(48_000_000));
    let whisper = WhisperClient::new(&address)
        .unwrap()
        .with_timeout(Duration::from_millis(500));
    let error = whisper.transcribe(&recording, None).unwrap_err();
    // Said of the sending, not of the waiting for an answer.
    assert!(
        error.contains("did not read the request in time"),
        "{error}"
    );
    drop(held);
}

#[test]
fn a_server_that_never_answers_ends_each_call_at_its_timeout_and_the_microphone_reopens() {
    let (address, held) = silent();
    let short = Duration::from_millis(300);
    let started = std::time::Instant::now();
    let whisper = WhisperClient::new(&address).unwrap().with_timeout(short);
    assert!(
        whisper
            .transcribe(&wav("a"), None)
            .unwrap_err()
            .contains("too long")
    );
    assert!(whisper.ping().unwrap_err().contains("too long"));
    let piper = PiperClient::new(&address).unwrap().with_timeout(short);
    assert!(piper.voices().unwrap_err().contains("too long"));
    assert!(
        piper
            .synthesize("pt_BR-cadu-medium", "x")
            .unwrap_err()
            .contains("too long")
    );
    // Four calls at 300 ms each, not at the five seconds of a quick call.
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    // Through the mouth and its thread: one line, and the microphone is open again.
    let (reported_tx, reported) = channel();
    let speaking = Speaking::default();
    let talker = Talker::spawn(
        Mouth::new(
            Box::new(PiperClient::new(&address).unwrap().with_timeout(short)),
            Box::new(FakePlayback::default()),
            voice("cadu").unwrap(),
            speaking.clone(),
        ),
        move |why| reported_tx.send(why).unwrap(),
    );
    talker.say("Done.".to_owned());
    let why = reported.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(
        why.starts_with("speech is off for this reply") && why.contains("too long"),
        "{why}"
    );
    for _ in 0..200 {
        if !speaking.active() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!speaking.active());
    drop(held);
}

#[test]
fn only_an_address_on_this_machine_is_accepted_and_names_are_refused() {
    for good in ["127.0.0.1:8178", "127.0.0.2:5178", "[::1]:5178"] {
        assert_eq!(require_loopback(good).unwrap(), good);
        assert!(WhisperClient::new(good).is_ok() && PiperClient::new(good).is_ok());
    }
    for bad in [
        "localhost:8178",
        "example.com:80",
        "192.168.1.5:5178",
        "10.0.0.1:80",
        "0.0.0.0:5178",
        "[::ffff:127.0.0.1]:80",
        "8.8.8.8:53",
        "127.0.0.1",
        "",
        "http://127.0.0.1:80",
    ] {
        assert!(require_loopback(bad).is_err(), "{bad}");
        assert!(WhisperClient::new(bad).is_err() && PiperClient::new(bad).is_err());
    }
}

#[test]
fn the_default_persona_describes_itself_as_it_did_before_voices_existed() {
    let keys: Vec<String> = Persona::of(Profile::WarmDidactic)
        .describe()
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    let mut sorted = keys;
    sorted.sort();
    assert_eq!(
        sorted,
        [
            "detail",
            "formality",
            "humour",
            "language",
            "name",
            "nickname",
            "profile"
        ]
    );
}
