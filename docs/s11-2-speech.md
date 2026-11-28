---
title: "S11-2: speech on the host, measurements and the hand test"
type: report
status: done
created: 2026-11-27
updated: 2026-11-27
tags: [shell, speech, voice, whisper, piper, personality, boundary]
---

# S11-2: speech on the host

Roadmap item S11-2, under ADR 0031. A spoken request becomes a validated plan
and the answer is spoken back, with no network; two users hear two voices; with
speech off the output is the output of today. This report says what was built,
what was measured on the development machine, what ADR 0031 did not say and
the code now does, and what only a person with a microphone and a speaker can
check.

Nothing in ADR 0031 failed on this machine. Three places where the ADR's text and the code differ (a capture without a crate, the voice not choosing the recogniser's language, no `--auto-send`) are in ADR 0038, proposed for the user to accept.

## What was built

| Part | Where |
|---|---|
| Four ports: `Transcriber`, `Synthesizer`, `AudioCapture`, `AudioPlayback`. (The ADR calls the second one "speaker"; `ports::Speaker` is already the model writing a reply, ADR 0021, so it is `Synthesizer`.) | `ports.rs` |
| The closed list of four voices, `language_fits` and the default voice (in `voices.rs`, a leaf that imports nothing, so `soul` does not depend on the speech adapters); what a reply says aloud (`speakable`), what a transcript becomes (`transcript_line`), `Mouth` (synthesise and play), `Ear` (record and transcribe), `Talker` (speaks on a thread), `Speaking` (the gate that shuts the microphone), `SharedLog` | `speech.rs` |
| Loopback adapters for `whisper-server` (`POST /inference`, multipart) and Piper (`GET /voices`, `POST /synthesize`); only IP addresses on the loopback interface are accepted | `speech_http.rs`, over `model::exchange` (the model's HTTP client, made to carry bytes and GET) |
| Playback with `afplay` on a file in a private directory; capture with the AudioQueue API of macOS | `audio.rs` |
| Format 2 of the personality: `voice`, and the third contradiction rule | `soul.rs` |
| Push-to-talk in the conversational interface: Ctrl-T records, Ctrl-T or Enter stops, the transcript fills the input line; replies spoken on a thread | `tui/state.rs`, `tui/mod.rs`, `tui/view.rs` |
| `--speak` in both interfaces, `--listen` in the conversational one; `--whisper` and `--tts` | `main.rs`, `plain.rs`, `setup.rs` |
| Fakes for the gate: in-memory ports in `speech::loopback`, and servers made for the test on `127.0.0.1:0` | `speech.rs`, `speech_tests.rs` |

No dependency was added. The capture declares about forty lines of the
AudioToolbox C API by hand: that does not need a binding generator
(`coreaudio-sys` brings `bindgen`), and `cpal` and `hound` are Apache-2.0 only,
which ADR 0005 refuses (`rodio` is MIT OR Apache-2.0 but needs `cpal`, so it
does not pass either). ADR 0031 said "crates that pass the gate"; with
none, the gate's licence check has nothing new to say.

Options, all off by default: `--listen`, `--speak`, `--whisper 127.0.0.1:PORT`
(default `127.0.0.1:8178`), `--tts 127.0.0.1:PORT` (default `127.0.0.1:5178`).
They are refused, not ignored, with `--no-model`, with `--speak --no-reply`,
with `--setup-soul`, with `--listen` and no conversational interface
(`--plain`, a pipe), and when `--whisper` or `--tts` has no `--listen` or
`--speak` to serve. `USAGE` changed.

### What ADR 0031 did not say

Found by running the servers, and now in the code:

1. **Piper answers an unknown voice with its default voice**, status 200, one
   log line (`Voice not found ... Using default voice`). So two users would
   hear one voice with no sign of it if a voice file were missing. `Mouth` asks
   `GET /voices` before every utterance and refuses, in one line, to speak when
   the voice of the personality is not loaded.
2. **`POST /download` fetches a voice from the network.** The adapter never
   calls it. Starting the server with `-m` naming a file that does not exist is
   an error (`Unable to find voice`), not a download; `whisper-server` also has
   one listening socket on `127.0.0.1` and no other (`lsof`). Start the server with `--host 127.0.0.1` and put the voices in
   `--data-dir` by hand.
3. The synthesis endpoint is `POST /synthesize` with `{"text", "voice"}`
   (the voice is the file name without `.onnx`, found in `--data-dir`), and
   empty text is a 500, so empty text is never sent.
4. Piper's server needs the voice's `.onnx` and `.onnx.json` side by side.
5. A transcript from `whisper-server` of silence is a bracketed tag
   (`[BLANK_AUDIO]`); it is "nothing was heard".
6. An error body from a server is untrusted text. It is reduced to one escaped
   line of at most 200 characters before it reaches the person.
7. A cut answer (the connection closes before `Content-Length`) is an error:
   a truncated WAV would otherwise be played as if whole. This applies to the
   model's HTTP client as well, which shares the code.
8. A personality with `voice` and `language: en-US` against a SOUL that lists
   `en` is a contradiction by the existing rule of ADR 0028 (the check is an
   exact tag match). The voice rule is looser, deliberately: `en` suits the
   `en-US` voice and `pt` the `pt-BR` ones, a different region does not.
9. A personality that names no language, under a SOUL that says `languages:
   any`, has no reply language. It is spoken with the first voice of the list
   (`cadu`, pt-BR). This is a choice the ADR left open; the alternative, saying
   nothing, makes a default install mute.

## First task: Piper and `whisper-server` on the development machine

Apple M1, 8 GB, macOS 27.2. Installed for this: `brew install whisper.cpp`
(1.9.6, MIT; with `ggml`, `llama.cpp`, `sdl2-compat`), `brew install cmake
ninja` (not needed in the end), the Whisper model `ggml-small-q5_1.bin`
(190,085,487 bytes, SHA-256 `ae85e4a9...411bb`, as in the research), the voice
`pt_BR-cadu-medium.onnx` (63,221,984 bytes, SHA-256 `d5a662e3...499de`, which
equals the hash in the research and the one Hugging Face lists), and
`piper-tts` 1.8.0 from PyPI in a virtual environment. Nothing is in the
repository.

### Why Piper failed in the research

The `piper-tts` 1.8.0 macOS wheel stops at start-up with `Error processing file
'/Users/runner/work/piper1-gpl/.../phontab': No such file or directory`. The
cause, measured: espeak-ng inside the wheel keeps the data directory in a
buffer of 160 bytes on macOS, and a longer path is silently replaced by the
path it was built with (the comment above `-DN_PATH_HOME` in the pinned
`CMakeLists.txt` says so). Running `espeakbridge.initialize` on the same data
directory through symbolic links of growing length:

| Length of the path to `espeak-ng-data` | Result |
|---|---|
| 120, 140, 150, 155, 158 | starts |
| 160, 165, 170, 180, 200 | the error above |

The research ran in a scratch directory whose path is far over 160, so this is
the likely cause of its failure (not verified: the research does not record the
path). The fix is the placement: **install Piper under a short path**, for
example `~/.cache/pv` (the whole path to `espeak-ng-data` below 160 bytes; mine
was 82). The voices and the model may be anywhere. This is a limit of the
wheel, not of ADR 0031.

### Numbers

Whisper, `small` q5_1, `whisper-server -m ggml-small-q5_1.bin --host 127.0.0.1
--port 8178 -l pt`, one 3.3 s WAV made with `say -v Luciana` ("Mostre o
tamanho da pasta de downloads."):

| | |
|---|---|
| Transcript | " Mostre o tamanho da pasta de Downloads." (exact) |
| First request | 1.31 to 1.36 s |
| Later requests | 0.44 to 0.47 s, about 0.14 of real time |
| Memory (`vmmap`, physical footprint) | 358 MB alone; 326 MB beside the model |
| Start (file cached) | up in 1 to 2 s |

Piper, `pt_BR-cadu-medium`, `python -m piper.http_server`, the text "Abri a
pasta de downloads. Ela tem quarenta e dois arquivos." (4.46 s of audio):

| | |
|---|---|
| First request | 0.17 to 0.22 s |
| Later requests | 0.13 to 0.14 s, about 0.03 of real time |
| A one-word reply | 0.04 s |
| Memory | 179 MB (peak 179 MB); 172 MB beside the model |
| Start | up in about 2 s |
| Sockets | one listening socket on `127.0.0.1:PORT`, nothing else (`lsof -nP -a -p PID -i`) |

All three together (Bonsai 8B at 8k as in S1: `llama-server -c 8192 --parallel
1 --jinja -ngl 99`, 1.2 GB footprint), started in that order, one request to
each:

| | |
|---|---|
| Used / free memory | 7.0 GB used, 1.2 GB free, "free percentage" 15% (before: 4.9 GB used, 32%) |
| Swap file | 6144 MB at the start, 8192 MB after |
| Latency | Whisper 0.44 to 1.31 s, Piper 0.13 to 0.17 s, a 16-token model answer 0.8 s |
| Errors | none; Whisper's and Piper's answers were the same as alone |

They fit and work together. The machine was already under pressure before
anything started (4.9 GB used and a 6 GB swap file with nothing of this
running), and the three leave it at 15% free: it works, but with no room for
anything else. Whisper `base` (the ADR's low-memory setting, 181 MiB) was not
re-measured. The recogniser is a server the user starts, so it can be stopped
when not needed.

The scripts that produced these numbers are in the appendix.

## The boundary, line by line

Each line of ADR 0031 section 5 and the test that holds it. Each test was
confirmed by mutation: the line was broken in the code, the gate failed, the
code was put back (the table of mutations is below).

| Line | Test |
|---|---|
| 1. The transcript is untrusted text, shown, on the typed path, logged with a mark | `speech_tests::what_was_heard_is_one_escaped_line_...`, `a_request_that_is_the_transcript_as_heard_says_it_came_from_speech`, `tui::tests::a_spoken_request_becomes_a_validated_plan_...` |
| 1. It never answers a question | `tui::state::tests::a_transcript_never_answers_a_question_and_never_enters_any_other_mode` |
| 2. It waits for Enter | `what_was_heard_waits_in_the_input_line_and_is_sent_only_by_enter` |
| 3. Only the reply is spoken | `a_root_panel_a_table_and_a_verdict_are_never_spoken_in_place_of_text`, `a_hostile_reply_is_spoken_only_as_the_cleaned_line` |
| 4. The system does not hear itself | `the_microphone_stays_shut_from_the_moment_a_reply_is_queued_until_it_has_been_played` |
| 5. Speech off changes nothing | `speech_is_off_by_default_and_asks_for_nothing`, the existing tests of events and logs, and the byte comparison below |
| 6. A failure of speech is a line | `a_failure_of_speech_is_a_line_...`, `what_a_server_writes_in_an_error_...`, `a_failure_while_speaking_is_reported_...`, `plain::tests::a_reply_is_spoken_only_with_a_mouth_...` |
| Voice only at the synthesiser | `the_voice_never_reaches_a_prompt_...` and the existing every-persona comparison of `soul_tests`, which now includes a persona for every voice |
| Loopback only | `only_an_address_on_this_machine_is_accepted_and_names_are_refused`, `an_address_that_is_not_on_this_machine_stops_the_shell_before_it_starts` |
| Two voices | `two_users_with_two_ids_are_spoken_to_in_two_voices_at_the_synthesiser`, `a_voice_the_server_does_not_have_is_never_replaced_by_its_default` |

`--auto-send` (ADR 0031, section 5, point 2, "an opt-in") is **not
implemented**: the item does not list it, and a transcript that is sent at once
is the one part of the boundary that cannot be undone by reading. Not done.

## Evidence that speech off changes nothing

The binary built from `main` (`4686859`) and the one from this branch were run
on the same scripted session (`--plain`, a stand-in model on `127.0.0.1` that
answers a plan and a reply, a temporary home with three files, the same log
file name in two directories), once with the model and once with `--no-model`:

| | with the model | `--no-model` |
|---|---|---|
| standard output | identical, 1064 bytes | identical, 947 bytes |
| error output | identical, empty | identical, empty |
| decision log (without `unix_time` and `elapsed_ms`) | identical, 10 events | identical, 6 events |

The model run includes the reply under the result. This compared the line
adapter only: the conversational interface was not compared byte for byte (it
needs a terminal); `tui::state::tests` pin what it draws, and the hint under
the input line is the old one unless `--listen` was given. The scripts are in
the appendix.

## End to end with the real Piper

`intent-shell --plain --speak` with the stand-in model, the real Piper server
and `afplay`, three runs (`e2e.sh`):

| Run | Result |
|---|---|
| personality `format: 2`, `voice: cadu`, voice installed | the reply was spoken (5.1 s for the whole run, 4 s of it audio); the log has `{"event":"speech","speak":true,"voice":"cadu"}` |
| `voice: jeff`, only `cadu` installed | the reply as text, then "speech is off for this reply: the voice jeff (pt_BR-jeff-medium) is not installed on the speech server"; nothing was spoken |
| Piper stopped | the reply as text, then "speech is off for this reply: cannot connect to 127.0.0.1:5178: Connection refused"; the shell goes on |

`--speak --tts example.com:5178` stops the shell before it starts ("names are
refused: the speech servers must be on this machine"), and so does `--listen`
with `--plain`.

## Mutations

Every new test was confirmed by breaking the behaviour it holds and running the
unit tests (the same ones the gate runs): each break below made the named test
fail, and the code was put back. The list is the second pass, after review round 1
(the first pass, 27 breaks, only recorded that something failed).

| Break | Failing test |
|---|---|
| personality-language rule never fires | `speech_tests::a_voice_that_does_not_match_the_personalitys_own_language_contradicts_it` |
| SOUL-language rule never fires | `speech_tests::a_voice_in_a_language_the_soul_does_not_allow_contradicts_it` |
| the voice is added to the prompt | `speech_tests::the_voice_never_reaches_a_prompt_the_facts_or_the_persona_the_log_records` |
| the voice is added to the persona the log records | `speech_tests::the_default_persona_describes_itself_as_it_did_before_voices_existed`; `speech_tests::the_voice_never_reaches_a_prompt_the_facts_or_the_persona_the_log_records` |
| the default voice ignores the language | `speech_tests::a_personality_with_no_voice_is_spoken_in_the_first_voice_of_the_reply_language` |
| no check that the voice is installed | `plain::tests::a_reply_is_spoken_only_with_a_mouth_and_a_failure_is_one_printed_line`; `speech_tests::a_voice_missing_on_the_real_adapter_is_refused_before_anything_is_synthesised` |
| the ear ignores that the system is speaking | `speech_tests::the_microphone_stays_shut_from_the_moment_a_reply_is_queued_until_it_has_been_played` |
| a queued reply does not shut the microphone | `speech_tests::the_microphone_stays_shut_from_the_moment_a_reply_is_queued_until_it_has_been_played`; `tui::tests::a_root_panel_a_table_and_a_verdict_are_never_spoken_in_place_of_text` |
| a played or failed reply never reopens the microphone | `speech_tests::a_failure_while_speaking_is_reported_and_reopens_the_microphone`; `speech_tests::the_microphone_stays_shut_from_the_moment_a_reply_is_queued_until_it_has_been_played` |
| transcript not escaped | `speech_tests::silence_and_tags_are_nothing_heard_and_a_long_transcript_is_capped`; `speech_tests::what_was_heard_is_one_escaped_line_with_the_reply_language_passed_on` |
| tags before the speech are kept | `speech_tests::silence_and_tags_are_nothing_heard_and_a_long_transcript_is_capped` |
| tags after the speech are kept | `speech_tests::silence_and_tags_are_nothing_heard_and_a_long_transcript_is_capped` |
| every request is marked as spoken | `speech_tests::a_request_that_is_the_transcript_as_heard_says_it_came_from_speech` |
| no request is marked as spoken | `speech_tests::a_request_that_is_the_transcript_as_heard_says_it_came_from_speech`; `tui::tests::a_spoken_request_becomes_a_validated_plan_and_the_answer_is_spoken_back` |
| any address is accepted | `speech_tests::only_an_address_on_this_machine_is_accepted_and_names_are_refused` |
| a name is resolved | `speech_tests::only_an_address_on_this_machine_is_accepted_and_names_are_refused` |
| a transcript is taken outside the wait for it | `tui::state::tests::a_held_key_does_not_stop_the_recording_it_started_and_escape_cancels_it`; `tui::state::tests::a_speech_note_neither_scrolls_nor_restarts_a_question_nor_cancels_a_warning` |
| a transcript is also pushed as a request | `tui::state::tests::what_was_heard_waits_in_the_input_line_and_is_sent_only_by_enter` |
| stop accepted at once after start | `tui::state::tests::a_held_key_does_not_stop_the_recording_it_started_and_escape_cancels_it` |
| Ctrl-T works in every mode | `tui::state::tests::a_held_key_does_not_stop_the_recording_it_started_and_escape_cancels_it`; `tui::state::tests::a_transcript_never_answers_a_question_and_never_enters_any_other_mode` |
| `--speak --no-reply` accepted | `tests::speech_flags_that_could_do_nothing_are_refused_not_ignored` |
| `--listen` with `--no-model` accepted | `tests::speech_flags_that_could_do_nothing_are_refused_not_ignored` |
| a cut HTTP answer is accepted | `model::tests::an_answer_shorter_than_its_length_is_cut_short`; `speech_tests::what_piper_answers_must_be_audio_and_whole` |
| the request is spoken in place of the reply | `tui::tests::a_root_panel_a_table_and_a_verdict_are_never_spoken_in_place_of_text`; `tui::tests::a_spoken_request_becomes_a_validated_plan_and_the_answer_is_spoken_back` |
| the failure of the voice is not printed (plain) | `model::tests::nothing_listening_is_an_error_at_once`; `plain::tests::a_reply_is_spoken_only_with_a_mouth_and_a_failure_is_one_printed_line` |
| the SOUL reads format 2 | `soul_tests::the_format_is_closed_and_versioned_and_every_error_says_where`; `soul_tests::a_soul_that_cannot_be_used_gives_the_default_and_a_message_that_says_why` |
| format 1 accepts `voice` | `soul_tests::the_format_is_closed_and_versioned_and_every_error_says_where`; `speech_tests::a_voice_needs_format_two_and_a_name_from_the_list` |
| the all-zero recording is accepted | `audio::tests::silence_and_a_blip_are_refused_with_a_reason_and_speech_is_wrapped` |
| error text is not escaped | `speech_tests::what_a_server_writes_in_an_error_reaches_the_person_as_one_escaped_capped_line` |
| `pt-BR` sent to the recogniser as is | `speech_tests::a_request_for_the_recogniser_is_one_multipart_file_and_its_text_comes_back` |
| anything is audio | `speech_tests::what_piper_answers_must_be_audio_and_whole` |
| a transcript replaces what was typed | `tui::state::tests::a_transcript_is_added_to_what_was_typed_not_in_place_of_it` |
| the hint hides nothing/shows no microphone key | `tui::state::tests::without_listen_there_is_no_microphone_key_and_the_hint_is_unchanged` |
| escaped_line does not escape | `reply_tests::a_hostile_reply_changes_no_decision_runs_nothing_and_cannot_pass_for_the_screen`; `reply_tests::the_text_is_one_line_capped_in_characters_and_escaped` |
| speech notes and late transcripts reset scroll, quiet gap and warning | `tui::state::tests::a_speech_note_neither_scrolls_nor_restarts_a_question_nor_cancels_a_warning` |
| utterances are not joined in the mark | `speech_tests::a_request_that_is_the_transcript_as_heard_says_it_came_from_speech` |
| the hung-server timeouts are ignored | `speech_tests::a_server_that_never_answers_ends_each_call_at_its_timeout_and_the_microphone_reopens` |
| a reply file name taken by a leftover is an error, not skipped | `audio::tests::a_name_taken_by_a_leftover_file_is_skipped_and_the_audio_is_written_whole` |
| a reply file name taken by a leftover is an error instead of skipped | `audio::tests::a_name_taken_by_a_leftover_file_is_skipped_and_the_audio_is_written_whole` |
| a speech note or transcript at a question asks the loop to discard pending keys | `tui::state::tests::a_speech_note_at_a_question_does_not_ask_the_loop_to_discard_pending_keys` |
| the microphone's thread keeps the event channel open after the worker ended | `tui::tests::the_microphones_thread_keeps_no_event_sender_alive_once_the_worker_has_ended` |
| a request that is any run of utterances is not marked as spoken | `speech_tests::a_request_that_is_the_transcript_as_heard_says_it_came_from_speech` |
| a parenthesis anywhere is a tag | `speech_tests::silence_and_tags_are_nothing_heard_and_a_long_transcript_is_capped` |
| an answer for an earlier recording is taken for the current one | `tui::state::tests::an_answer_for_an_earlier_recording_is_never_taken_for_the_current_one` |
| the microphone's thread acts on a stale stop | `tui::tests::the_microphones_thread_ignores_a_stop_or_a_cancel_for_an_earlier_recording` |
| the microphone's thread acts on a stale cancel | same |
| erasing what was heard is not told to the log | `tui::state::tests::erasing_what_was_heard_tells_the_microphone_to_forget_it_and_typing_does_not` |
| forgetting keeps what was heard | `speech_tests::a_request_that_is_the_transcript_as_heard_says_it_came_from_speech` |
| a bracket with a digit, or attached to a word, is a tag | `speech_tests::silence_and_tags_are_nothing_heard_and_a_long_transcript_is_capped` |
| the sending of a recording has no deadline | `speech_tests::a_server_that_never_reads_cannot_hold_a_recording_being_sent_past_the_timeout` |
| a region with no voice gets the first voice of the list instead of one of its language | `speech_tests::a_personality_with_no_voice_is_spoken_in_the_first_voice_of_the_reply_language` |
| tags that touch each other are kept | `speech_tests::silence_and_tags_are_nothing_heard_and_a_long_transcript_is_capped` |
| a closed sink still sends | `tui::tests::nothing_is_sent_through_a_closed_sink_and_the_sender_is_dropped` |
| the queue of replies to speak has no bound | `speech_tests::a_voice_that_is_behind_drops_replies_instead_of_queueing_them_without_end` |
| the microphone hints are cut off on a narrow terminal | `tui::view::tests::the_microphone_hints_wrap_on_a_narrow_terminal_and_the_old_ones_are_as_before` |
| the worker starts no thread that speaks | `tui::tests::the_speech_the_worker_starts_speaks_replies_and_listens_in_the_replies_language` |
| the recogniser language is lost between the setup and the microphone's thread | same, and `tests::the_recogniser_language_is_the_replies_and_reaches_the_setup` |
| the personality's voice is not the one the mouth is built with | `tests::the_voice_of_the_personality_is_the_one_the_mouth_is_built_with` |
| the microphone's thread does not forget an erased transcript | `tui::tests::what_the_microphones_thread_is_told_to_forget_no_longer_marks_a_request` |
| playing does not remove the audio a closed shell left | `audio::tests::the_audio_is_removed_whatever_the_player_did_and_a_stuck_player_is_killed` |
| a speech note moves the text being read | `tui::state::tests::a_speech_note_neither_scrolls_nor_restarts_a_question_nor_cancels_a_warning` |
| stale reply audio is kept | `audio::tests::a_reply_left_by_a_closed_shell_is_removed_and_a_recent_or_own_one_is_not` |
| a held Enter sends the transcript just heard | `tui::state::tests::a_key_still_held_from_stopping_the_recording_neither_sends_nor_restarts_it` |
| a held Ctrl-T restarts the recording | `tui::state::tests::a_key_still_held_from_stopping_the_recording_neither_sends_nor_restarts_it` |
| the voice chooses the recogniser's language | `speech_tests::the_recogniser_is_told_the_reply_language_and_never_the_language_of_the_voice` |
| the thread that speaks does not share the mouth's gate | `speech_tests::the_microphone_stays_shut_from_the_moment_a_reply_is_queued_until_it_has_been_played` |
| the event that gives the keyboard back is made before the reply is queued | `tui::tests::the_event_that_gives_the_keyboard_back_exists_only_once_the_reply_is_queued` |
| the audio directory accepts a directory of another user, one open to others, or a link | `audio::tests::the_audio_directory_must_be_a_real_directory_of_ours_that_nobody_else_can_open` |
| the audio file stays after a player that failed, or a stuck player is not killed | `audio::tests::the_audio_is_removed_whatever_the_player_did_and_a_stuck_player_is_killed` |
| `speech_setup` gives the ear another gate than the mouth's | `tests::the_ear_and_the_mouth_that_speech_setup_builds_share_one_gate` |

## Run it

Install under short paths (below 160 bytes to `espeak-ng-data`), once:

```bash
uv venv ~/.cache/pv/v
uv pip install --python ~/.cache/pv/v/bin/python piper-tts==1.8.0 flask
brew install whisper.cpp
```

Voices: the `.onnx` and `.onnx.json` of each voice of the list from
`https://huggingface.co/rhasspy/piper-voices/tree/e9c8a39444359388adaa94255becb37a5a2b8e51`
into one directory, then check each file against the hash in `voices.rs`
(`shasum -a 256 FILE`). The shell does not check them: it never sees the
files. The Whisper model is `ggml-small-q5_1.bin` from
`https://huggingface.co/ggerganov/whisper.cpp`.

Start the servers, each on loopback:

```bash
whisper-server -m ggml-small-q5_1.bin --host 127.0.0.1 --port 8178 -l pt
```

```bash
~/.cache/pv/v/bin/python -m piper.http_server -m VOICES/pt_BR-cadu-medium.onnx --data-dir VOICES --host 127.0.0.1 --port 5178
```

The shell, with the model as always (`docs/s3-intent-shell-prototype.md`):

```bash
./scripts/intent-shell.sh --speak --listen
```

Ctrl-T starts recording, Ctrl-T or Enter stops, Esc cancels. What was heard
appears in the input line; read it, edit it, press Enter. A reply is spoken as
soon as it is sent to the screen (in the line adapter, after it is printed). A personality of format 2 chooses the voice:

```text
format: 2
language: pt-BR
voice: jeff
```

## The hand test

What was run here, and what was not.

Run on the development machine, with the commands above:

- `whisper-server` with a real request, and Piper with real requests and
  `afplay` on the result (exit status 0): the numbers above.
- The three servers together, as in the table.
- The capture on this machine: the AudioQueue opens, runs and delivers
  buffers, but they are all zero. This process has no microphone permission,
  so the shell's message was seen for real: "the microphone gave only silence:
  the terminal may lack permission to use it (System Settings, Privacy &
  Security, Microphone)".
- `intent-shell --plain --speak` against the real model and the real Piper,
  see "End to end" below.

**Not run here, for the user to run** (it needs a person at the microphone and
the speaker):

1. Give the terminal the Microphone permission, start the two servers and
   `./scripts/intent-shell.sh --speak --listen`.
2. Press Ctrl-T, say "mostre os maiores arquivos da pasta de downloads", press
   Enter. Expected: the sentence in the input line, a note saying it was
   heard, and nothing sent until Enter.
3. Press Enter. Expected: the plan screen as for a typed request, the result,
   the reply under it, and the reply spoken (the voice may start a moment before
   or after the text is drawn).
4. While it speaks, press Ctrl-T. Expected: "the system is speaking: wait
   until it has finished".
5. Stop Piper and ask something again. Expected: the shell works, and one line
   under the reply says speech is off for it.
6. Confirm that nothing is heard while the reply plays (the loudspeaker near
   the microphone), and the accuracy of `small` on your own pt-BR speech: this
   report has none (the research measured two synthetic sentences).

## Not done, and limits

- `--auto-send` (above).
- Linux and other systems: the two audio adapters say they are macOS only; the
  rest of speech is portable. Neither `alsa` nor `pipewire` was tried.
- The hash of a voice is a record for the person who installs it; nothing at
  run time checks the file, because the shell never opens it.
- The hash of `en_US-joe-medium.onnx` in the list comes from the file listing of
  Hugging Face at the pinned commit, not from a download.
- A server that hangs holds the interface in "transcribing" for at most 30
  seconds (the timeout of `WhisperClient`), with no cancel but Ctrl-C; a hung
  Piper costs one line per reply, on the thread that speaks in the
  conversational interface, so the shell is not held there; in the line adapter
  (`--plain --speak`) the speaking is synchronous and holds the prompt for up to
  35 seconds (5 for the voice list, 30 for the synthesis), and playing holds it
  until the reply has ended (up to 180 seconds). Both are tested for the time they
  take, not for the holding.
- While a transcription runs (at most 30 s, the screen says so) no key but Ctrl-C
  does anything: the microphone's thread is busy with it, and a recording asked
  for in that time would start only after it. A hung `whisper-server` costs that
  wait, not an error of the shell.
- Ctrl-T is a toggle, not hold-to-talk (a terminal sends no key release): a key
  held for more than 0.7 s counts as the second press and stops the recording.
  What is lost at the end of a recording is at most the 20 ms buffer being
  filled; an asynchronous stop would flush it but its callbacks come after the
  queue must be disposed (measured by the reviewer with a C program, not by the
  shell: the capture has not run against a microphone the process may use).
  With `languages: any` and no voice the recogniser uses the language it was
  started with: start it with `-l auto` for that case, as the measured runs used
  `-l pt`.
- A recording stops growing after 30 seconds but stops only when the person
  presses a key; the screen says that only the first 30 s are kept, and what
  is said after is not transcribed. Leaving the shell while a reply is playing does not stop
  `afplay`.
- The recogniser is told the reply language (`pt` for `pt-BR`), which is the
  language the person is expected to speak; a person who speaks another one
  than the replies are written in gets a worse transcript.
- The mark `input: speech` on a request compares the whole line with one
  utterance, or several in a row, since the last request: a line that mixes typed
  and spoken words, or one with a part of an utterance deleted, is not marked (the `transcript` events are in the
  log either way), and a line typed to equal what was heard is.
- A SOUL that lists several languages and a personality with no `language`: the
  recogniser is told the first one (ADR 0031 section 3 takes the first language
  of the SOUL as the reply language). The voice is never consulted for it, since
  the voice reaches only the speech call. A person who speaks another of them
  sets `language` in the personality.
- The servers are found by a fixed loopback port, with no authentication. On a
  machine with several users, another user who binds `8178` or `5178` first
  receives the microphone audio or the reply text of a shell that has not
  checked. The shell only guarantees the audio does not leave the machine.
  Pick other ports with `--whisper` and `--tts`, and start the servers before
  the shell.
- A language the list has no voice for (`es`) is spoken with the first voice of
  the list, a pt-BR one; a region with no voice of its own (`en-GB`, `pt-PT`)
  gets the voice of its language that the list has (`joe`, `cadu`). A named voice is checked against the
  languages; an absent one is not, because there is nothing to compare.
- A reply is voiced without the screen's frame ("Reply from the model", the
  mark): audio has no equivalent, and the text of a reply is escaped, so an odd
  character is voiced as its escape. ADR 0031 asks for the same escaping and
  cap as the screen, and that is what is done.
- At most three replies wait to be spoken, the one playing included; a fourth
  is not spoken and says so. Nothing stops a reply that is playing.
- Leaving the shell while a reply is playing leaves `afplay` playing to the end
  of that reply, and its temporary file in the private directory; the next
  playback by the same user removes files older than five minutes.
- The capture adapter is hand-written FFI, where ADR 0031 section 4 pictured a
  crate "that passes the gate". The port is the decision; no crate passes that
  is not Apache-2.0-only or does not bring a binding generator. The FFI has not
  been run against a microphone the process may use: the hand test is the
  proof it still lacks.
- With speech off, three things still differ from today, none in a session that
  runs: the `USAGE` text lists the four new options; a personality file with a
  `voice` key, with a first line other than `format: 1`, or with a wrong key says
  so in the new words (format 2 and the new key list); and a model answer cut
  short reads as below. The byte comparison above covers valid files only.
- A truncated answer from the model server now reads "the server's answer was
  cut short" instead of "server reply is not JSON": the check lives in the HTTP
  code both share, and is a behaviour of today's output that changes (for a reply cut in the middle).
- No voice is cloned and no voice is downloaded by the shell.

## Appendix: the commands that were run

Paths: `$REPO` is this repository, `$SCRATCH` any empty directory. `whisper-server`,
`llama-server` and the model are installed as in the sections above.

### Path length of Piper's data directory

```bash
V=$HOME/.cache/pv/v/bin/python
D=$HOME/.cache/pv/v/lib/python3.12/site-packages/piper/espeak-ng-data
base=$HOME/.cache/lp
for n in 120 140 150 155 158 160 165 170 180 200; do
  pad=$(( n - ${#base} - 1 - 15 )); dir="$base/$(printf 'x%.0s' $(seq 1 $pad))"; mkdir -p "$dir"; ln -sfn $D "$dir/espeak-ng-data"
  full="$dir/espeak-ng-data"
  res=$($V -I -c "
from piper import espeakbridge as e
e.initialize('$full'); print('ok')" 2>&1 | tail -1 | cut -c1-10)
  echo "len=${#full} -> $res"
done
```

### All three servers together (`coresident.sh`)

```bash
#!/bin/zsh
S=$SCRATCH
OUT=$S/coresident.txt; : > $OUT
log(){ echo "$@" | tee -a $OUT; }
snap(){ log "-- $1"; sysctl -n vm.swapusage | tee -a $OUT; memory_pressure | rg "free percentage" | tee -a $OUT; vm_stat | rg "Swapouts|Pageouts" | tee -a $OUT; }
fp(){ vmmap -summary $1 2>/dev/null | rg "Physical footprint:" | sed "s/^/$2 /" | tee -a $OUT; }
snap baseline
t0=$(date +%s)
llama-server -m ~/models/bonsai-8b/Bonsai-8B-Q1_0.gguf -c 8192 --parallel 1 --port 8089 --jinja -ngl 99 --host 127.0.0.1 > $S/llama.log 2>&1 &
LP=$!
for i in $(seq 1 600); do curl -s -o /dev/null http://127.0.0.1:8089/health && break; sleep 1; done
log "llama ready after $(( $(date +%s)-t0 )) s"; fp $LP llama; snap after-llama
t0=$(date +%s)
whisper-server -m $S/whisper/ggml-small-q5_1.bin --host 127.0.0.1 --port 8178 -l pt > $S/whisper/server.log 2>&1 &
WP=$!
for i in $(seq 1 600); do curl -s -o /dev/null http://127.0.0.1:8178/ && break; sleep 1; done
log "whisper ready after $(( $(date +%s)-t0 )) s"
for n in 1 2 3; do curl -s -o /dev/null -w "whisper req$n %{time_total}s\n" http://127.0.0.1:8178/inference -F file=@$S/whisper/req.wav -F response_format=json -F language=pt | tee -a $OUT; done
fp $WP whisper
V=$HOME/.cache/pv/v/bin/python
$V -I -m piper.http_server -m $S/piper/pt_BR-cadu-medium.onnx --data-dir $S/piper --host 127.0.0.1 --port 5178 > $S/piper/server.log 2>&1 &
PP=$!
for i in $(seq 1 120); do curl -s -o /dev/null http://127.0.0.1:5178/ && break; sleep 1; done
for n in 1 2 3; do curl -s -X POST http://127.0.0.1:5178/synthesize -H 'Content-Type: application/json' -d '{"text":"Abri a pasta de downloads. Ela tem quarenta e dois arquivos."}' -o $S/piper/co$n.wav -w "piper req$n %{time_total}s\n" | tee -a $OUT; done
fp $PP piper
curl -s http://127.0.0.1:8089/v1/chat/completions -H 'Content-Type: application/json' -d '{"messages":[{"role":"user","content":"Say hi in four words."}],"max_tokens":16}' -o /dev/null -w "llama req %{time_total}s\n" | tee -a $OUT
snap all-three
kill $WP $PP $LP; log done
```

### A stand-in for the model (`stub_model.py`)

```python
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
PLAN = {"kind":"plan","version":3,"summary":"List downloads","steps":[{"id":"s1","action":"fs.list","args":{"target":{"place":"downloads"}},"depends_on":[],"elevate":False}]}
class H(BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        text = json.dumps(PLAN) if "response_format" in body else "Listei a pasta de downloads, voce tem tres arquivos."
        out = json.dumps({"choices":[{"message":{"content":text}}]}).encode()
        self.send_response(200); self.send_header("Content-Type","application/json"); self.send_header("Content-Length",str(len(out))); self.end_headers(); self.wfile.write(out)
    def log_message(self,*a): pass
HTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
```

### Speech off, byte for byte (`compare.sh`)

Needs a binary built from `main` in `$SCRATCH/mainwt-target` (a `git worktree` of
`4686859` built with `CARGO_TARGET_DIR`) and the one of this branch in `$REPO/target`.

```bash
#!/bin/bash
S=$SCRATCH
R=$REPO
OLD=$S/mainwt-target/aarch64-apple-darwin/debug/intent-shell; NEW=$R/target/aarch64-apple-darwin/debug/intent-shell
python3 -I $S/tools/stub_model.py 8099 & STUB=$!; sleep 1
cd $S/cmp; rm -f *.out *.err *.jsonl; rm -f logs-old/*.jsonl logs-new/*.jsonl; mkdir -p logs-old logs-new
for bin in old new; do
  H=$S/cmp/home-$bin; mkdir -p $H/Downloads; for f in a.txt b.txt c.bin; do echo data > $H/Downloads/$f; done
  B=$OLD; [ $bin = new ] && B=$NEW
  for mode in model nomodel; do
    if [ $mode = model ]; then extra=(--model 127.0.0.1:8099); in="list my downloads"; else extra=(--no-model); in="fs list --in downloads"; fi
    printf '%s\n\n' "$in" | HOME=$H USER=ana $B --plain --log $S/cmp/logs-$bin/$mode.jsonl "${extra[@]}" > $S/cmp/$bin-$mode.out 2> $S/cmp/$bin-$mode.err; echo "$bin $mode rc=$?"
  done
done
kill $STUB
norm(){ sed -E 's/"unix_time":[0-9]+/"unix_time":0/; s/"elapsed_ms":[0-9]+/"elapsed_ms":0/; s/home-(old|new)/HOME/g; s/logs-(old|new)/LOGS/g' "$1"; }
for mode in model nomodel; do
  norm old-$mode.out > o.out; norm new-$mode.out > n.out; cmp o.out n.out && echo "$mode stdout identical ($(wc -c < n.out) bytes)"
  norm old-$mode.err > o.err; norm new-$mode.err > n.err; cmp o.err n.err && echo "$mode stderr identical ($(wc -c < n.err) bytes)"
  norm logs-old/$mode.jsonl > o.log; norm logs-new/$mode.jsonl > n.log; cmp o.log n.log && echo "$mode log identical ($(wc -l < n.log) events)"
done
```

### End to end with the real Piper (`e2e.sh`)

```bash
#!/bin/bash
S=$SCRATCH; NEW=$REPO/target/aarch64-apple-darwin/debug/intent-shell
python3 -I $S/tools/stub_model.py 8099 & STUB=$!
($HOME/.cache/pv/v/bin/python -I -m piper.http_server -m $S/piper/pt_BR-cadu-medium.onnx --data-dir $S/piper --host 127.0.0.1 --port 5178 > $S/piper/server.log 2>&1 & echo $! > $S/piper/server.pid)
for i in $(seq 1 40); do curl -s -o /dev/null http://127.0.0.1:5178/ && break; sleep 1; done
H=$S/cmp/home-e2e; mkdir -p $H/Downloads; for f in a.txt b.txt c.bin; do echo data > $H/Downloads/$f; done
printf 'format: 2\nlanguage: pt-BR\nvoice: jeff\n' > $S/cmp/personality-jeff
printf 'format: 2\nlanguage: pt-BR\nvoice: cadu\n' > $S/cmp/personality-cadu
echo "=== cadu installed, personality cadu: should speak"; time (printf 'list my downloads\n' | HOME=$H USER=ana $NEW --plain --model 127.0.0.1:8099 --log $S/cmp/e2e.jsonl --speak --tts 127.0.0.1:5178 --personality $S/cmp/personality-cadu | tail -4)
echo "=== personality jeff, only cadu installed: should say speech is off"; (printf 'list my downloads\n' | HOME=$H USER=ana $NEW --plain --model 127.0.0.1:8099 --log $S/cmp/e2e.jsonl --speak --tts 127.0.0.1:5178 --personality $S/cmp/personality-jeff | tail -3)
echo "=== piper stopped: shell works, one line"; kill $(cat $S/piper/server.pid); sleep 1
(printf 'list my downloads\n' | HOME=$H USER=ana $NEW --plain --model 127.0.0.1:8099 --log $S/cmp/e2e.jsonl --speak --tts 127.0.0.1:5178 --personality $S/cmp/personality-cadu | tail -3)
echo "=== non-loopback tts"; HOME=$H $NEW --plain --model 127.0.0.1:8099 --speak --tts example.com:5178 </dev/null; echo rc=$?
echo "=== --listen with --plain"; HOME=$H $NEW --plain --model 127.0.0.1:8099 --listen </dev/null; echo rc=$?
kill $STUB
```
