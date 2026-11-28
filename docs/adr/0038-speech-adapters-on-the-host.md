---
title: "ADR 0038: Speech on the host: capture without a crate, the voice limited to speech, no auto-send yet"
type: adr
status: proposed
created: 2026-11-27
updated: 2026-11-27
tags: [speech, voice, audio, shell, amendment]
---

# ADR 0038: Speech on the host: capture without a crate, the voice limited to speech, no auto-send yet

## Status

**Proposed (2026-11-27), for the user to accept.** It amends ADR 0031 in three
places, adds two decisions it did not take, and supersedes nothing else. ADR 0031 stays accepted; where it and this
ADR differ, this ADR is the one that was built in S11-2
(`docs/s11-2-speech.md`).

## Context

S11-2 built ADR 0031. Running it showed three places where the text of the ADR
and the code that was reviewed differ, and a reader of the ADR alone would be
misled.

## Decision

1. **Capture without a crate (ADR 0031, section 4).** The ADR says capture is
   "a small module behind the port, over crates that pass the gate
   (`coreaudio-sys` ...)". On macOS the capture is the AudioQueue API of the
   system's AudioToolbox, declared by hand in `audio.rs` (about forty items, each
   `unsafe` block with its `// SAFETY:` reason). No dependency is added.
   `coreaudio-sys` needs a binding generator for forty declarations, and `cpal` and
   `hound` stay refused by ADR 0005 (Apache-2.0 only; `rodio` is dual-licensed but
   needs `cpal`). The port is unchanged: another
   adapter can replace this one, and Linux has none yet (the shell says so).
2. **The voice reaches only the speech call (ADR 0031, section 3; this adds a
   decision the ADR did not take).** The
   recogniser is told the language of the replies (the personality's `language`,
   else the first language of the SOUL), never the language of the chosen voice.
   A person who speaks another language than the first the SOUL lists sets
   `language` in the personality.
3. **`--auto-send` is not part of S11-2 (ADR 0031, section 5, point 2).** The
   transcript waits for Enter and there is no option to send it at once. The
   option stays an opt-in for a later item, and when it exists the plan still
   goes through every confirmation.

Also recorded here, because ADR 0031 is silent on them and the code depends on
them: Piper answers an unknown voice with its default voice, so the voice is
checked against `GET /voices` before every utterance; `POST /download` is never
called; a Piper install needs a path of fewer than 160 bytes to its
`espeak-ng-data`.

## Consequences

- The capture has not run against a microphone the development process may
  use (no permission was granted); the hand test in `docs/s11-2-speech.md` is
  what still proves it.
- A later Linux adapter chooses between `alsa`, `pipewire` and a hand-written
  binding with the licence check of ADR 0005, as the ADR 0031 text pictured.
- Accepting this ADR needs nothing from the code; rejecting item 2 or 3 would
  need a change.

## Decisions for the user

1. Accept the hand-written capture and no auto-send for S11-2.
2. Accept that the voice does not choose the recogniser's language.
