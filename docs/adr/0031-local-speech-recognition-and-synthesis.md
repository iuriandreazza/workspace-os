---
title: "ADR 0031: Local speech: whisper.cpp to listen, Piper to speak, one voice per personality"
type: adr
status: accepted
created: 2026-11-23
updated: 2026-11-23
tags: [speech, voice, asr, tts, personality, licence, shell, security]
---

# ADR 0031: Local speech: whisper.cpp to listen, Piper to speak, one voice per personality

## Status

**Accepted (2026-11-23). Amended by ADR 0038 (proposed, 2026-11-27)** in
its section 4 (capture without a crate, loopback IP addresses only), its section 5
point 2 (`--auto-send` is not built) and the language the recogniser is told.
Until ADR 0038 is accepted, read this ADR with it. It answers the open question of ADR 0006 ("which local speech
models to use, and their licences") and gives ADR 0028 the spoken-voice key
that format 1 left out. The evidence is in `docs/s11-speech-research.md`; this
ADR does not repeat it. It extends ADR 0028 (the personality file gains format
2 with one key) and changes nothing else in it. It does not touch ADR 0002,
0006, 0007 or 0021.

## Context

ADR 0006 gives the AI a spoken voice that belongs to each user's personality,
and says speech "covers both directions, speaking and listening, and runs
locally". S11 has to turn a spoken request into a validated plan and speak the
answer back, with no network, and two users must be able to hear two different
voices. Voice confirmation of root actions is S11b and is out of scope here.

Two licence rules shape the choice (research, section 1):

- ADR 0005 allows only GPL-2.0-compatible code to be linked. Apache-2.0-only
  and GPL-3.0 code cannot be. The audio crates `cpal` and `hound` are
  Apache-2.0 only, and every Brazilian Portuguese synthesis engine found
  depends on espeak-ng, which is GPL-3.0-or-later. ADR 0008 already accepts a
  GPL-3.0 program "only as a separate program".
- A speech voice carries the licence of the recordings it was trained on, and
  Piper's own voices often descend from a research-only corpus (research,
  section 3). A voice's name is not enough to know its licence.

## Decision

### 1. Listening: whisper.cpp with Whisper `small`, as a separate process

Recognition is `whisper-server` from whisper.cpp (MIT) with the multilingual
Whisper `small` model quantised q5_1 (about 190 MB), listening on
`127.0.0.1`. Whisper `base` q5_1 (about 60 MB) is the setting for a machine
with little memory. The shell sends one WAV file per utterance and receives
text. It is not streaming: the user speaks, stops, and the utterance is
transcribed (a request is a few seconds, and the measured real-time factor of
`small` on the M1 was 0.12 to 0.15).

It is a separate process for the same reason `llama-server` is: the shell has
no C++ build, and the engine can be replaced. whisper.cpp is MIT, so linking it
later (for example through the `whisper-rs` crate, `Unlicense`) stays possible
under ADR 0005 if this boundary becomes a cost.

### 2. Speaking: Piper, as a separate process, with a closed list of voices

Synthesis is the Piper HTTP server (`piper1-gpl`, GPL-3.0, with espeak-ng,
GPL-3.0-or-later), run as its own process on `127.0.0.1` and never linked into
the shell, as ADR 0008 reads GRUB. The shell sends text and receives a WAV.

The voices the shell accepts are a **closed list kept in code**, and an entry
is added only with all of these recorded in the commit that adds it:

1. the `MODEL_CARD` of the voice, read from a pinned commit of
   `rhasspy/piper-voices`, and its `Training` chain walked down to a voice
   trained from scratch, every dataset in the chain CC0, public domain, CC BY,
   CC BY-SA, Apache-2.0 or a custom licence that allows use;
2. the SHA-256 of the `.onnx` file;
3. the attribution text, when a chain needs it.

A voice whose chain reaches the Lessac corpus, or any non-commercial
recordings, is refused. The sherpa-onnx copies of the pt-BR voices carry the
old chain and are refused too, for the same name: **the file is what is
checked, not the name**.

The list at the time of this ADR, from the research:

| Id (the value in the personality) | Language | Piper voice | Chain |
|---|---|---|---|
| `cadu` | pt-BR | `pt_BR-cadu-medium` | CC0, from `rdh` (CC0) |
| `jeff` | pt-BR | `pt_BR-jeff-medium` | CC0, from `rdh` (CC0) |
| `faber` | pt-BR | `pt_BR-faber-medium` | CC0, from `hifi_tts_92` (CC BY 4.0, attribution owed) |
| `joe` | en-US | `en_US-joe-medium` | CC0, from `rdh` (CC0) |

Distinct voices are distinct files: Piper's voices are single-speaker, so two
users hear two voices by having two entries. Voice cloning and reference-audio
models are **not** part of the system, even where their licence allows them
(research, section 6, point 4).

The voice files are data installed by the user next to the models (ADR 0005:
weights carry their own licence and are not part of the code), into the
user's data directory, not committed to this repository. The licence notice of
a release lists each shipped voice with its chain and attribution.

### 3. How a voice is named in the personality (extends ADR 0028)

ADR 0028 section 4 reserved the spoken voice for "format 2, which an S8 shell
rejects instead of ignoring". This ADR defines format 2 of the **personality
file** only; the SOUL file stays format 1.

- A personality of format 2 has the keys of format 1 plus one more, `voice`.
  Its value is one id from the closed list of section 2 (an unknown word makes
  the file invalid, like every other value in ADR 0028 section 2). A format 1
  personality is still read as before and has no voice.
- **Contradiction** (ADR 0028 section 3 gains a third rule): a personality
  contradicts the SOUL when its `voice` is a voice whose language is not allowed
  by the SOUL's `languages` (unless the SOUL says `any`), or when it does not
  match the personality's own `language` where both are set. Same handling as
  the other two rules: rejected whole, with a message naming the field.
- **A personality with no `voice`** is spoken with the first voice of the list
  for the reply language (the personality's `language`, else the first language
  of the SOUL), chosen by code, never by the model. Two users who set nothing
  hear the same voice; two users who set different ids hear different voices.
- **Who is speaking** is the logged-in user (the session), so the personality,
  and so the voice, is the one read for that user. No speaker recognition is
  involved; recognising a speaker to pick a personality, the open question in
  ADR 0006, stays with S11b's voiceprint and is not needed here.
- The voice reaches only the text-to-speech call. It is never put in a
  prompt, and the resolver, broker, planner and `Confirm` never see it
  (ADR 0028 section 5, extended: a test compares every action, verdict and
  decision-log event under every voice, as it does under every preset).

### 4. How the shell talks to the speech components

Two ports next to the existing ones (`ports.rs`): a transcriber (WAV in, text
out) and a speaker (text in, WAV out), each with a loopback HTTP adapter for
the two servers above, and a fake for tests. Two more ports, audio capture and
audio playback, hide the platform:

- **Playback:** the host's own player on a temporary file in a directory only
  the user can open (`afplay` on macOS, present on the development machine;
  `aplay` or `pw-play` on Linux, not verified).
- **Capture:** a small module behind the port, over crates that pass the gate
  (`coreaudio-sys` MIT on macOS, `alsa` or `pipewire` on Linux). `cpal`,
  `hound` and `rodio` are not used. The WAV header is written by hand.
- **Later, inside the OS**, which has no audio driver (S12): the two audio ports
  are the only adapters that change; the speech components keep the same
  text-and-WAV contract, as userspace services.

New shell options, all off by default: `--listen` (push-to-talk), `--speak`,
`--whisper HOST:PORT`, `--tts HOST:PORT`. The shell does not start the two
servers; the user does, as with `llama-server`.

The flow of a spoken request:

```text
push-to-talk ─► capture ─► transcriber ─► TRANSCRIPT shown in the input line
                                              │ the user presses Enter
                                              ▼
                typed path: planner ─► resolver ─► broker ─► confirm ─► execute ─► OUTCOME
                                                                                      │
                                           reply (ADR 0021) ─► screen, and, with --speak, ─► speaker ─► playback
```

### 5. The boundary (each line is tested)

1. **The transcript is untrusted text.** It is shown, goes down exactly the
   typed path, and is logged as input like typed text, with a mark saying it
   came from speech. It never reaches `Confirm`, and a confirmation is never
   taken from audio: in S11 every confirmation, root or not, is typed
   (S11b changes the root case only, and only with ADR 0007's three factors).
2. **By default the transcript waits for Enter.** The user reads what was
   heard before it is sent. An `--auto-send` option sends it at once; the plan
   still goes through every confirmation.
3. **Only the ADR 0021 reply is spoken**, after the same escaping and length
   cap as the screen (about 400 characters). The table, verdicts, the root panel
   and every confirmation are never spoken in their place. A root action's exact
   display stays text.
4. **The system does not hear itself.** Capture is off while it is speaking and
   until playback ends; listening is push-to-talk, never always on.
5. **Speech off changes nothing.** Without `--listen` and `--speak` no speech
   code runs, no process is contacted and the output is byte-identical to
   today's. With no model there is no reply, so nothing is spoken; the typed
   shell, the deterministic shell (S5) and recovery mode work as before and
   never load speech.
6. **A failure of speech is not an error of the request.** A server that is down,
   a timeout or an empty transcript says so in one line and leaves the typed
   input working.

### 6. What the done condition needs from S11-2

A spoken request becomes a validated plan and the answer is spoken, with no
network (every endpoint is loopback, and a test asserts the adapters accept
only loopback addresses), and two users hear two voices (two personality files
with two ids give two different `voice` values at the speaker port). The
real-audio check (a real microphone and speaker) is a manual step recorded in
the report; the automated tests use the fakes.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| **whisper.cpp small + Piper, separate processes (chosen)** | Both measured or read to the licence; the only recogniser run here covers pt-BR and English; Piper voices can be checked file by file; engines are replaceable | Two servers to run; GPL-3.0 only stays acceptable while Piper is a separate process; Piper could not be run on the development machine in the research (see below) |
| Kokoro-82M for synthesis | Apache-2.0 weights with a simple licence; three pt-BR voices in one model | pt-BR still needs espeak-ng (GPL-3.0+), so also a separate process; weak on short utterances (the typical reply); no runtime verified on the development machine |
| Voice cloning (Chatterbox, Qwen3-TTS) | Any voice for any user | Hands a voice-forging tool to every user, against the threat model of ADR 0007; large (about 2 GB); Chatterbox is retired |
| sherpa-onnx as the one runtime for both | One process; streaming recognisers | Apache-2.0 only, so never linked; failed on the retrained Piper voices and on Portuguese Kokoro in 2.0.3 |
| Parakeet for recognition | Lower vendor WER | Not measured; European Portuguese training data; GPU-oriented runtime; Apache-2.0 runtime |
| Moonshine | Streaming | No Portuguese recognition |
| Rust crates (`cpal`, `hound`) for audio | Least code | Apache-2.0 only: fail `scripts/check-licences.py` and ADR 0005 |
| Always listening | Hands-free | Ambient audio becomes input; the system would hear itself |
| Auto-send by default | Faster | The user cannot correct a mis-hearing before the planner sees it |

## Consequences

- S11-2 starts by running one Piper voice and one `whisper-server` request on
  the development machine and **measuring** them: the research has no synthesis
  figure, because the `piper-tts` 1.8.0 macOS wheel stopped at espeak-ng
  start-up and sherpa-onnx could not read the retrained voices. If Piper cannot
  run on the host at all, this ADR is revisited, not worked around.
- The personality file gets format 2 and the code a closed voice list; the
  contradiction check gains a rule. S9 writes format 1 and is not affected.
- A release distributes voices, so it carries the notice of each chain and the
  CC BY 4.0 attribution for `faber`. The legal question Piper itself declines
  to answer (whether a trained voice is a derivative of its recordings) joins
  the legal review already open in the roadmap.
- Memory: Whisper `small` measured 356 MiB peak RSS (GPU buffers not counted)
  next to Bonsai 8B at 8k (1303 MB footprint, 2256 MB RSS, S1) on a 5461 MiB
  Metal budget; one run, and a cold load took 289 s under memory pressure. The
  recogniser should be started after the model and may be unloaded when idle.
- Two small servers, one process each, are an operational cost; the shell can
  run them from a helper script later, not in S11.

## Known limits

- No synthesis speed or memory has been measured (research, section 7).
- Recognition accuracy on Brazilian Portuguese was checked on two synthetic
  sentences only.
- Capture on macOS and Linux and the microphone permission have not been tried.
- Only four voices, one speaker each; a user cannot add a voice without a code
  change and a licence reading. This is intended.
- No streaming: the user speaks, then waits for the transcript.
- With no model there is no reply to speak.
- Nothing about audio inside the OS is decided (S12).

## Decisions for the user

1. Accept whisper.cpp `small` and Piper as separate processes (sections 1 and 2).
2. Accept the closed voice list, with `faber`'s CC BY 4.0 attribution
   requirement, or drop `faber` and keep `cadu`, `jeff` and `joe`.
3. Accept `voice` as format 2 of the personality and the new contradiction rule
   (section 3).
4. Accept "transcript waits for Enter, confirmations always typed" (section 5)
   as the S11 rule, with `--auto-send` as an opt-in.
