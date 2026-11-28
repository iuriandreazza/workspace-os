---
title: "S8: the SOUL file, the presets and the personality per user"
type: report
status: done
created: 2026-11-20
updated: 2026-11-20
tags: [shell, soul, persona, personality, format]
---

# S8: the SOUL file, the presets and the personality per user

Roadmap item S8, decided in ADR 0006 and ADR 0028 (accepted on 2026-11-22: the format is a
decision ADR 0006 and ADR 0021 do not cover; it supersedes only section 3 of
ADR 0021). The generalisation of S8a: the profile file of one word became the
SOUL file, and a second file, the personality, sits on top of it. The reply
(`reply::reply_to`) is still the only reader of either.

## Run it

```bash
./scripts/intent-shell.sh                                  # ~/.config/workspace-os/soul and /personality, if they exist
./scripts/intent-shell.sh --soul FILE --personality FILE   # files of your choice
./scripts/intent-shell.sh --voice-profile FILE             # alias of --soul, for the S8a name
./scripts/check.sh                                         # the gate, with every test below
./scripts/intent-shell-live-test.sh                        # the live test (needs llama-server and Bonsai 8B on 127.0.0.1:8089)
```

## The files

Both files are read only when replies are on and there is a model: with `--no-reply` or `--no-model` neither is read, even a named one, so a missing named file is not noticed. The SOUL's `languages` limits what a personality may ask for; with no personality it adds nothing to the prompt.

```text
# SOUL (one per machine)            # personality (one per user)
format: 1                           format: 1
preset: warm-didactic               name: Dex
name: Orion                         formality: casual
humour: none..light                 detail: brief
languages: en, pt-BR                language: pt-BR
```

Rules, all in `intent-shell/src/soul.rs` and written out in ADR 0028:

- UTF-8, at most 2048 bytes (read only that far; a pipe, a device or a
  directory is an invalid file, never a hang), one `key: value` per line, `#`
  lines and blank lines ignored, first line `format: 1`. An unknown key, a
  repeated key, a line without a colon, a wrong version: the whole file is invalid.
- Values are words of a closed list (`preset`, `formality`: casual, neutral,
  formal; `humour`: none, light, marked; `detail`: minimal, brief, explained), a
  name (1 to 24 letters, digits, space, `-`, `'`, `.`, two words at most) or
  language tags (`en`, `pt-BR`, up to 8; `any` in the SOUL).
- The preset gives each axis a default and the range a personality may use. The
  SOUL may narrow or move a range (`humour: none..light`); its default moves into it.
- **A personality contradicts the SOUL** when a level is outside the SOUL's
  range or its language is not in the SOUL's list. It is rejected whole, with
  every field named: `it contradicts the SOUL (formal-discreet): humour: marked
  is outside the SOUL's range none`. Nothing else can contradict, because no key
  exists for authority, capabilities, the preset or the SOUL's name.
- The S8a one-word file is format 0 (`preset: <word>`). Without `--soul`, the
  shell reads `soul`, and if there is none the old `voice-profile` beside it.

| Problem in | Named on the command line | At the default path |
|---|---|---|
| SOUL missing | the shell does not start (with replies on) | `direct-technical`, silently |
| SOUL unusable | `direct-technical` and one message | same |
| personality missing | the shell does not start | no personality, silently |
| personality unusable or contradicting | the shell does not start, with the message | the SOUL alone, one message, `personality` event `default_after_problem` |

The persona (SOUL and personality resolved, `soul::Persona`) fills the fixed
prompt of `reply::system_prompt`: the preset's line, one fixed sentence per
axis, the language tag and the two names as JSON-quoted labels. The file never
supplies a sentence. `detail: explained` asks the model for more but changes
neither the 400-character cap nor the cleaning. Startup logs a `voice_profile`
(the SOUL) and a `personality` event; neither is in the request's decision events.

## What the decision path never sees

`Persona` is a field of `reply::Voice` and of `Setup.reply`, nothing else. The
broker, the resolver, the planner and `Confirm` take no SOUL, no personality and
no persona, by their signatures.

## Tests (all run in `./scripts/check.sh`)

| Test | What it pins |
|---|---|
| `the_evaluation_intents_give_the_same_actions_verdicts_and_log_under_every_soul_and_personality` | every accepted plan of `eval/intents.json` (109 plans from 89 intents), as a normal user and an administrator, under 4 presets, the lowest and highest personality each preset allows, hostile names, with a model that says "Done." and one that echoes its whole prompt: outcome, rendered text, log, actions run, screens, root panels and planner calls equal the run with no voice. It checks that the comparison is not empty (plans ran, verdicts shown, root asked) |
| `the_format_is_closed_...`, `the_edges_of_every_range_...`, the contradiction tests | exact messages, every range edge |
| hostile tests | control sequences, `»`, lines imitating a verdict, the reply title, a plan in JSON, bidi and line-separator characters, null bytes, in every value, key and comment, and in the file as a whole: rejected with a message that is escaped and short, or (comments) dropped without effect; a name that reads like a verdict is a quoted label; what the model was told, echoed back, is one marked, escaped line of at most 400 characters |
| file tests | absent, named and absent, too large (2049 bytes) and exactly 2048, not UTF-8, directory, `/dev/zero`, `/dev/null`, a named pipe with no writer, an escaped path |
| `tests/reply_session.rs` (real binary) | the SOUL and personality shape the prompt and the result text is byte for byte the same; a contradiction at the default path is dropped with the field named; one named with `--personality` stops the shell; the S8a `voice-profile` still works |

Limits of the invariance test: it uses `FakeHost` with the Downloads and Documents
places of `pipeline_tests::files()`, not the intent's `cwd` and not the home of
`eval/intents.json`, so many plans end in a refusal or a question rather than a
run; those are verdicts too and are compared. The model is scripted: it says
nothing about what a real model writes.

## Mutations

Each applied, the whole `intent-shell` suite run, the failing tests listed (first three),
then undone (the source was compared with a backup afterwards):

1. level range check removed: fails `a_personality_at_the_default_path_that_contradicts_the_soul_is_dropped_with_the_field_and_the_reason`, `a_level_outside_the_souls_range_is_a_contradiction_that_names_the_field_and_the_limit`, `a_personality_at_the_default_path_that_cannot_be_used_is_dropped_and_the_soul_stands_alone` and more
2. language check removed: fails `a_personality_the_person_names_that_contradicts_the_soul_stops_the_shell`, `a_contradicting_personality_is_not_applied_in_part`, `a_language_the_soul_does_not_list_is_a_contradiction_unless_the_soul_says_any` and more
3. contradicting personality applied in part: fails `a_personality_at_the_default_path_that_contradicts_the_soul_is_dropped_with_the_field_and_the_reason`, `a_personality_the_person_names_that_contradicts_the_soul_stops_the_shell`, `a_contradicting_personality_is_not_applied_in_part` and more
4. unknown key accepted: fails `the_format_is_closed_and_versioned_and_every_error_says_where`
5. duplicate key accepted: fails `the_format_is_closed_and_versioned_and_every_error_says_where`
6. format version not checked: fails `a_personality_at_the_default_path_that_cannot_be_used_is_dropped_and_the_soul_stands_alone`, `the_format_is_closed_and_versioned_and_every_error_says_where`
7. name charset not checked: fails `a_name_is_short_and_made_of_letters_and_a_few_marks`
8. file size limit removed: fails `a_soul_that_cannot_be_used_gives_the_default_and_a_message_that_says_why`
9. comments not skipped: fails `a_soul_that_cannot_be_used_gives_the_default_and_a_message_that_says_why`, `hostile_text_in_comments_is_dropped_and_changes_nothing`, `the_file_of_s8a_is_format_zero_and_comments_and_blank_lines_are_ignored`
10. format 0 (S8a file) not read: fails `the_file_of_s8a_is_format_zero_and_comments_and_blank_lines_are_ignored`, `the_profile_file_chooses_the_voice_and_a_bad_one_says_so_and_uses_the_default`
11. named personality problem does not stop the shell: fails `a_personality_the_person_names_that_contradicts_the_soul_stops_the_shell`, `a_personality_at_the_default_path_that_cannot_be_used_is_dropped_and_the_soul_stands_alone`
12. file opened blocking: fails `a_directory_a_device_and_a_pipe_are_read_at_their_start_and_never_hang`
13. path in messages not escaped: fails `the_path_in_a_message_is_escaped`
14. rejected value not escaped: fails `hostile_text_in_either_file_is_rejected_with_a_message_that_cannot_forge_the_screen`
15. axis levels ignored in the prompt: fails `a_range_in_the_soul_moves_the_default_into_it`, `a_soul_holds_the_name_and_the_ranges_and_a_personality_the_nickname`
16. names not quoted in the prompt: fails `a_soul_holds_the_name_and_the_ranges_and_a_personality_the_nickname`, `the_soul_and_the_personality_files_shape_the_prompt_and_change_nothing_else`
17. default level not moved into the range: fails `a_range_in_the_soul_moves_the_default_into_it`
18. backwards range accepted: fails `the_format_is_closed_and_versioned_and_every_error_says_where`
19. language tag not validated: fails `the_format_is_closed_and_versioned_and_every_error_says_where`
20. soul default not used when personality leaves a level out: fails `a_soul_holds_the_name_and_the_ranges_and_a_personality_the_nickname`
21. a persona event in the decision log: fails `a_hostile_reply_changes_no_decision_runs_nothing_and_cannot_pass_for_the_screen`, `a_model_that_cannot_be_reached_or_fails_or_writes_nothing_leaves_only_todays_output`, `the_same_request_gives_the_same_decisions_under_every_profile_with_the_reply_on_or_off` and more
22. detail raises the reply cap: fails `escaping_cannot_grow_the_reply_past_its_cap`, `the_text_is_one_line_capped_in_characters_and_escaped`, `what_the_model_was_told_can_only_come_back_as_marked_escaped_text_of_a_fixed_length`
23. legacy voice-profile not found: fails `the_profile_file_chooses_the_voice_and_a_bad_one_says_so_and_uses_the_default`
24. --voice-profile alias removed: fails `a_profile_file_the_person_names_that_does_not_exist_stops_the_shell_with_a_message`, `replies_are_on_by_default_and_the_soul_and_personality_default_to_the_config_directory`
25. personality file not read: fails `a_personality_at_the_default_path_that_contradicts_the_soul_is_dropped_with_the_field_and_the_reason`, `a_personality_the_person_names_that_contradicts_the_soul_stops_the_shell`, `the_soul_and_the_personality_files_shape_the_prompt_and_change_nothing_else`
26. source of the personality not logged as dropped: fails `a_personality_at_the_default_path_that_contradicts_the_soul_is_dropped_with_the_field_and_the_reason`, `the_profile_file_chooses_the_voice_and_a_bad_one_says_so_and_uses_the_default`
27. `any` accepted as a language tag: fails `any_is_the_souls_word_for_no_limit_and_never_a_language`
28. byte-order mark kept: fails `a_byte_order_mark_at_the_start_of_either_file_is_ignored`
29. a broken SOUL not said first when a named personality fails: fails `a_named_personality_that_fails_against_a_soul_that_could_not_be_used_says_so_first`
30. precedence line removed from the prompt: fails `the_settings_are_told_to_take_precedence_over_the_preset_line`, `the_soul_and_the_personality_files_shape_the_prompt_and_change_nothing_else`
31. the SOUL's log event records the persona: fails `the_soul_and_the_personality_files_shape_the_prompt_and_change_nothing_else`

## A real session (Bonsai 8B)

`llama-server` and `~/models/bonsai-8b/Bonsai-8B-Q1_0.gguf` were present, a server
answered on 127.0.0.1:8089, and `./scripts/intent-shell-live-test.sh` ran
the three requests with the four presets and a fifth persona (the
warm-didactic SOUL "Orion" with a personality "Dex", `language: pt-BR`).
Three tests passed; `a_path_and_a_relative_path_resolve_against_the_session_directory`
failed as it does on `main` (planner, S8a report; ADR 0024 and S4b). The log gained only
its reply event each time (asserted).

```text
[~] > delete notes.txt in my downloads

  in ~

  Step  What                Needs      Verdict
  s1    Delete "Downloads"  fs.delete  REFUSED
          ↳ "Downloads" is a whole place and cannot be removed, moved or overwritten

  1 steps, 1 actions: 0 ok, 0 held for root, 1 refused.  The model says: "Delete notes.txt from my downloads"

  No step can run. Nothing ran.
(the result took 2594 ms)
--- direct-technical (+1738 ms)
  » Reply from the model (words only: it decided and ran nothing):
  » The system refused to delete "Downloads" as it is a directory. No action was
  » taken.
--- warm-didactic (+2872 ms)
  » Reply from the model (words only: it decided and ran nothing):
  » You asked to delete a file, but the system explained that "Downloads" is a
  » folder and cannot be removed. It's important to understand that the system
  » is protecting your files and folders, so you'll need to delete them manually
  » if you want to remove them.
--- formal-discreet (+2524 ms)
  » Reply from the model (words only: it decided and ran nothing):
  » The request to delete "Notes.txt" in "Downloads" was refused because
  » "Downloads" is a whole place and cannot be removed. Nothing ran as the
  » action was denied at the system level.
--- good-humoured (+2637 ms)
  » Reply from the model (words only: it decided and ran nothing):
  » You tried to delete a whole folder, but the system said it's not
  » possible—folders are like homes and can't be moved or erased. Let's keep
  » things simple and stick to what's actually doable!
--- warm-didactic + personality (pt-BR) (+2267 ms)
  » Reply from the model (words only: it decided and ran nothing):
  » You tried to delete a folder called "Downloads," but I can't do that because
  » it's a whole place and can't be removed, moved, or overwritten. Let me know
  » if there's something else I can help with!

```

The model follows the voice loosely: in the first request the pt-BR personality
produced a mixed Portuguese and English answer, in the second plain English. The
shell asks for the language; it cannot make a 1-bit 8B model comply, and the
reply is words under a result that is correct, marked as the model's.

## Review

Round 1 (a fresh subagent with the diff and the ADRs only; no confirmed finding on the broker, resolver, planner, verdicts or decision events; seven low findings):

| Finding | Outcome |
|---|---|
| A broken SOUL's message is lost when a named personality is then rejected against the fallback SOUL | Fixed: the SOUL's message is said first (mutation 29) |
| `any` accepted as a language tag; `languages: any, en` silently restricts to `en` | Fixed: `any` only alone in the SOUL (mutation 27) |
| The preset line can contradict an axis sentence in the prompt | Fixed: the prompt says the settings win (mutation 30) |
| A two-word name such as `Reply yes` is accepted, so "nothing is an instruction" is too strong | Accepted and the ADR wording softened: a name is words the model reads; the bound is that the model's output is only the marked reply. A narrower charset was not chosen: it would reject real names |
| A UTF-8 byte-order mark breaks both formats | Fixed: ignored at the start of a file (mutation 28) |
| The `voice_profile` event recorded the persona after the personality | Fixed: it records the SOUL alone (mutation 31) |
| The SOUL is in the user's `~/.config`, so the user can widen its ranges | Not a defect: the accepted deviation of ADR 0028; stated under limits |

Round 2 (fresh subagent, whole diff): nothing confirmed on the decision path or the layering; five low items:

| Finding | Outcome |
|---|---|
| A SOUL may widen a range beyond its preset (`direct-technical` with `humour: marked`), and a comment said "the most the preset allows" | Intended by ADR 0028 (the SOUL's own word); comment and this report now say "replace" |
| The SOUL's `languages` adds nothing to the prompt without a personality | Documented above; not changed (ADR 0028 section 3) |
| A name of only punctuation (`.`, `'`, `-`) is accepted | Fixed: a name needs a letter (test in `a_name_is_short_and_made_of_letters_and_a_few_marks`). Full-width and look-alike letters stay allowed: refusing them would refuse real names |
| The report said a missing named file stops the shell, but with `--no-reply` or `--no-model` neither file is read | Documented |
| `soul_path` used `exists()`, so a dangling `soul` link fell back to the old file silently | Fixed: a link that is there counts as present |

Round 3 (fresh subagent, whole diff): nothing on the decision path, the layering or the hostile-text handling; three low items:

| Finding | Outcome |
|---|---|
| A dangling link at the default SOUL or personality path was read as absent, silently | Fixed: it is a problem with a message (test `a_dangling_link_at_the_default_path_is_a_problem_not_an_absent_file`, confirmed by mutation: the test fails without the fix) |
| The SOUL's `languages` has no effect without a personality `language` | Same as round 2: documented, ADR 0028 section 3 |
| `scripts/intent-shell.sh` still described `--voice-profile` | Fixed: it now describes `--soul` and `--personality` |

Round 4 was not run: the round-3 fixes are one branch and one comment line.

## Not verified, limits

- A personality is per user in the file layout (`~/.config`), but the shell does
  not yet know who is speaking beyond the account that runs it. Who is speaking
  by voice is an open question of ADR 0006 (S11).
- The SOUL sits in the user's `~/.config`, not in a root-owned system path (ADR
  0028, accepted deviation). Anyone who can write it can change the voice, never
  an authority.
- Spoken voice is not a key of this format: it is `voice` in format 2 of the
  personality (ADR 0031, S11-2); the SOUL has none. A SOUL change by the AI and the
  history are S10; the interview is S9: S9 writes this format.
- The invariance test does not use the intents' `cwd`.
- The prompt asks for a language and a level of detail; the model's compliance is
  not tested beyond the live transcript above.
