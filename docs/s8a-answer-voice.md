---
title: "S8a: the voice of the answers"
type: report
status: done
created: 2026-11-19
updated: 2026-11-19
tags: [shell, soul, persona, model, ux]
---

# S8a: the voice of the answers

> The profile file described below is now the SOUL file (ADR 0028, `docs/s8-soul-format.md`): a one-word file still loads, as format 0, and `--voice-profile` is an alias of `--soul`.

Roadmap item S8a, decided in ADR 0021 (accepted 2026-11-16, with the user's three
decisions). After every result the intent shell asks the model for a short
reply in the tone of a voice profile, and shows it under the result. The table,
verdicts, tree, details view and root panel are the same deterministic text as
before; the reply is one more block, marked on every line, and it is never read
for anything.

## Run it

```bash
./scripts/intent-shell.sh                        # replies on, profile from the file or the default
./scripts/intent-shell.sh --no-reply             # exactly the output of S5
./scripts/intent-shell.sh --voice-profile FILE   # a profile file of your choice
```

The profile file holds one word, one of the four presets of ADR 0006:
`direct-technical`, `warm-didactic`, `formal-discreet`, `good-humoured` (case
and surrounding blanks do not matter). The default path is
`~/.config/workspace-os/voice-profile`. The file is read at most 256 bytes and never blocks (a named pipe is an empty file). Nothing else is accepted, not even a byte-order mark: a SOUL in free
text is what ADR 0006 refuses, so a file with anything more than the preset's
name is not a profile.

| The file | What happens |
|---|---|
| Absent at the default path | "direct and technical", no message |
| Named with `--voice-profile` and absent | the shell does not start, and says which file |
| Unreadable | the default profile, and one message naming the file and the system's error |
| Readable but not exactly a preset name | the default profile, and one message naming the file, what it holds (cut at 40 characters, escaped) and the four valid names |

Every start with a model and replies on records one `voice_profile` event
(`profile`, `source`: `default`, `file` or `default_after_problem`, and the
problem). With `--no-reply` or `--no-model` the file is not read at all.

## Where the reply sits

```text
request ─► plan ─► broker ─► (confirm) ─► execute ─► Outcome ─┬─► result on screen (as before)
                                                              └─► reply_to(Outcome) ─► log `reply` ─► marked block on screen
```

`reply::reply_to` takes the finished `Outcome`, the request text, a `Speaker`
and the log. It takes no `Shell`, `Confirm`, `Host`, `Planner` or `Authority`,
so nothing it reads or writes can reach the broker, a question or an action:
that is true by its signature, not by a test of behaviour. Both interfaces call
it after the result is on screen (`plain.rs`; `tui::serve`, which sends the
outcome, then `OutcomeAwaitingReply` (the result and `saying…` at once), then `Said(reply)`). A reply never delays or
alters the result; it only delays the next prompt.

`ports::Speaker` is the port. `model::LlamaSpeaker` is its adapter: the same
`llama-server`, no `response_format`, no tools, `max_tokens` 200, a 30 s deadline
for the connection and the answer together (the connection is cut at 5 s inside it).
The planner prompt, the grammar and the plan
schema are not touched (ADR 0024 and S4b own them). Connecting now has a limit
for the planner too, a transport change only.

## What the model is told

A fixed system prompt (`reply::system_prompt`) with the profile's one line
filled in by code, and a digest of the outcome (`reply::facts`):

```text
request: "delete notes.txt in my downloads"
result: every step was refused; nothing ran
step s1: "Delete \"Downloads\"" -> refused: "\"Downloads\" is a whole place and cannot be removed, moved or overwritten"
```

It has the request, the kind of outcome, one line per step with the friendly
name the screen shows and done / failed / not run / the verdict, and the number
of items a listing found. It has no file content, no output text, no host error
text (a failure is only `failed`), no summary written by the model, and no real
location beyond what the friendly screen already shows (the screen writes
the arguments of `process.run`, a setting's value and a SOUL rationale as given,
and so do the facts). Every piece of text
that came from the person, a file or the model is JSON-quoted and capped at 200
characters, so a name with a newline cannot start a new fact line. Tests
check the sentinel content of a read and the real home are absent from what the
scripted model received.

No reply is asked for after `help`, a typed line that is not a command, or a
log that could not be written, and none with no model (the deterministic shell).

## What is shown, and what cannot be forged

The model's text is cleaned (`reply::clean`): all whitespace collapsed to one
space, escaped like all model text (`printable`), then cut at 400 characters (not
bytes), so escaping cannot grow it. It is then wrapped by `reply::block`, not by the screen, to the
actual width, and **every** line, the title and each continuation, starts with
`  » `. The line adapter counts every non-ASCII character as two cells, so a wide
character its width table does not know cannot wrap a line into an unmarked one. No line of the deterministic screens starts with that. `»` (U+00BB) is now a
reserved character: `printable`, which escapes all text from files, the machine
and the model, writes it as `\u00bb`, so no text from a file can contain the mark,
and a long line the screen or the terminal wraps cannot leave a row that starts
like a reply. The cost: a file name with a `»` shows escaped, and a `»` cannot be
typed in the interface. The history printed when the interface closes wraps a reply to the terminal's own width, for the same reason (`--transcript` files use the same text). A test wraps hostile file content at four widths; another
walks the result, the plan screens, the details view and the root panel. A hostile reply
that imitates a verdict table, a root panel, a command or a plan in JSON is
therefore text after a mark and nothing else.

Plain mode prints it after the result; with no reply it prints nothing at all,
not even a blank line. The interface shows the result and `saying…` in one step and accepts no typing until the reply or its failure arrives (no request can start in between) and leaves
at once on Ctrl-C then, as it does while the model plans, because nothing is
running. Colours and headers are S3d.

## The decision log

```json
{"event":"reply","by":"model","decided":false,"profile":"warm-didactic","text":"...","elapsed_ms":2276}
{"event":"reply_failed","by":"model","decided":false,"profile":"warm-didactic","error":"...","elapsed_ms":30001}
```

It is the last event of the request. No event the code decided carries `by`.
The text is logged as the model wrote it (cut at 2000 characters), before the
reply is shown: if the log cannot be written the reply is not shown, as with
every model output (ADR 0002).

## Degradation

No model (`--no-model`): no reply, S5 as it was. A server error, a refused
connection, a timeout or an empty reply: no reply, one `reply_failed` event,
and the screen and transcript are those of `--no-reply`. The end-to-end test
compares the bytes.

## Tests and the gate

All of these run in `./scripts/check.sh`.

| Test | What it pins |
|---|---|
| `the_same_request_gives_the_same_decisions_under_every_profile_with_the_reply_on_or_off` | eight scenarios (ran unasked, confirmed, declined, root, refused, question, no plan, a read) times four profiles, reply on and off: outcome, rendered text, log minus the reply events, actions run, screens and root panels shown, planner calls: all equal |
| `the_reply_is_logged_last_and_apart_from_what_the_code_decided` | last event, `by: model`, `decided: false`; no decision event has `by` |
| `what_the_model_is_given_is_...` and the host failure, refusal and hostile name tests | the digest |
| `a_hostile_reply_changes_no_decision_...`, `a_reply_that_is_a_plan_...`, `no_line_of_the_deterministic_screens_...` | seven hostile texts; no extra planner call, no extra action; every line marked; no control character |
| `a_model_that_cannot_be_reached_or_fails_or_writes_nothing_...`, `a_reply_that_cannot_be_logged_...` | degradation |
| profile file tests (absent, named and absent, each preset, unknown and free text and binary and huge, a directory) | defined behaviour and the message |
| `model.rs`: no schema, no tools; a server that never answers, or drips bytes, ends at the timeout (bounded, it cannot hang the gate); nothing listening is an error | the adapter |
| `tui` state and `serve` tests | order result, saying, reply; none changes nothing; leaving; no reply with no model; every wrapped line marked at widths 12, 24 and 40 |
| `tests/reply_session.rs` | the real binary against a fake server: the output without the marked lines is byte for byte the `--no-reply` output; a 500 on the reply gives exactly that output; a reply that is a plan or a command is only text; the profile file picks the voice; a bad one says so |

Mutations, each applied to the code and shown to fail the gate (then undone):

 1. marks only the first line of a reply: fails `a_hostile_reply_changes_no_decision_runs_nothing_and_cannot_pass_for_the_screen`, `every_line_of_a_wrapped_reply_is_marked_and_fits`, `every_wrapped_line_of_a_reply_on_a_narrow_screen_is_marked`
 2. no cap on the reply: fails `the_text_is_one_line_capped_in_characters_and_escaped`
 3. newlines kept in the reply: fails `a_model_that_cannot_be_reached_or_fails_or_writes_nothing_leaves_only_todays_output`, `the_text_is_one_line_capped_in_characters_and_escaped`, `the_reply_comes_after_the_result_marked_and_the_rest_is_byte_for_byte_the_output_without_it`, `the_result_is_sent_first_and_the_reply_after_it_with_saying_in_between`
 4. reply not escaped: fails `a_hostile_reply_changes_no_decision_runs_nothing_and_cannot_pass_for_the_screen`, `the_text_is_one_line_capped_in_characters_and_escaped`
 5. file content in the facts: fails `what_the_model_is_given_is_the_outcome_in_friendly_names_and_nothing_from_the_machine`
 6. reply shown even when not logged: fails `a_reply_that_cannot_be_logged_is_not_shown`
 7. reply not marked as the model's: fails `a_model_that_cannot_be_reached_or_fails_or_writes_nothing_leaves_only_todays_output`, `the_reply_is_logged_last_and_apart_from_what_the_code_decided`, `the_reply_comes_after_the_result_marked_and_the_rest_is_byte_for_byte_the_output_without_it`
 8. reply marked as decided: fails `the_reply_is_logged_last_and_apart_from_what_the_code_decided`, `the_reply_comes_after_the_result_marked_and_the_rest_is_byte_for_byte_the_output_without_it`
 9. unknown profile silently the default: fails `an_unknown_profile_or_free_text_gives_the_default_and_a_message_that_says_why`, `the_profile_file_chooses_the_voice_and_a_bad_one_says_so_and_uses_the_default`
10. facts after a dead log: fails `there_is_nothing_to_say_after_a_listing_of_commands_a_bad_line_or_a_dead_log`
11. request not quoted in the facts: fails `a_hostile_file_name_reaches_the_model_only_as_a_quoted_string`, `what_the_model_is_given_is_the_outcome_in_friendly_names_and_nothing_from_the_machine`
12. lines one cell too wide: fails `every_line_of_a_wrapped_reply_is_marked_and_fits`, `every_wrapped_line_of_a_reply_on_a_narrow_screen_is_marked`
13. named missing profile is not an error: fails `a_profile_file_the_person_names_that_does_not_exist_stops_the_shell_with_a_message`, `a_profile_file_the_person_named_and_that_is_missing_is_an_error_naming_it`
14. no timeout on the reply: fails `a_server_that_never_answers_ends_the_reply_at_the_timeout_not_never`
15. --no-reply ignored: fails `replies_need_a_model_and_are_on_unless_turned_off`, `the_reply_comes_after_the_result_marked_and_the_rest_is_byte_for_byte_the_output_without_it`, `with_a_model_a_request_goes_to_the_model_and_is_not_read_as_a_command`
16. --no-reply flag does nothing: fails `replies_are_on_by_default_and_the_profile_file_defaults_to_the_config_directory`, `the_reply_comes_after_the_result_marked_and_the_rest_is_byte_for_byte_the_output_without_it`, `with_a_model_a_request_goes_to_the_model_and_is_not_read_as_a_command`
17. blank line printed when there is no reply: fails `a_server_error_on_the_reply_leaves_exactly_the_output_of_a_session_without_replies`
18. an empty reply block when there is none: fails `a_reply_is_a_block_under_the_result_and_none_changes_nothing`
19. reply asked with no model: fails `with_no_model_there_is_no_reply_even_if_a_speaker_is_given`
20. saying not abandoned on leaving: fails `leaving_abandons_a_wait_for_the_model_or_for_a_reply_but_never_a_running_step`
21. reply sent before the result: fails `with_no_model_there_is_no_reply_even_if_a_speaker_is_given`, `without_a_reply_the_events_are_those_of_today_and_a_failed_one_only_ends_the_wait`
22. host error text in the facts: fails `a_failure_of_the_host_and_a_refusal_are_told_without_the_host_text_and_with_the_reason`
23. the line adapter wraps with the screen's width table: fails `a_wide_character_the_width_table_does_not_know_cannot_wrap_into_an_unmarked_line`
24. cap before escaping: fails `escaping_cannot_grow_the_reply_past_its_cap`
25. timeout per read, not for the whole answer: fails `a_server_that_drips_bytes_is_cut_at_the_total_timeout`
26. profile file read to its end: the test on `/dev/zero` aborts the test run (out of memory)
27. machine text (an I/O error) in a refusal given to the model: fails `a_refusal_that_carries_machine_text_reaches_the_model_as_a_fixed_sentence`
28. the profile file opened blocking: fails `a_named_pipe_with_no_writer_is_an_empty_profile_not_a_hang`
29. the result sent without the wait for the reply: fails `the_result_is_sent_first_and_the_reply_after_it_with_saying_in_between`
30. a late reply ends any wait: fails `no_request_can_start_between_the_result_and_its_reply_and_a_late_reply_cannot_end_a_wait`
31. the notice about a bad profile not shown in the interface: fails `a_notice_is_the_first_thing_in_the_history_and_none_adds_nothing`
32. `»` not reserved: fails `text_from_a_file_cannot_contain_the_mark_so_no_wrapped_line_can_start_like_a_reply`
33. a reply asked for after leaving at a question: fails `leaving_at_a_question_does_not_wait_for_a_reply_to_the_refusal`
34. the reason a step was not run taken from the host's text: fails `a_step_that_was_not_run_is_told_to_the_model_in_the_codes_words_not_the_hosts`
35. a reply asked for after the input ended at a question (line adapter): fails `when_the_input_ends_at_a_question_no_reply_is_asked_for`
36. a line that is not valid text taken for the end of the input: fails `a_line_that_is_not_text_at_a_question_is_not_the_end_of_the_input`
37. a reply that throws the reader back to the bottom of the history: fails `a_reply_does_not_move_the_person_who_is_reading_back`
38. the end of the input remembered for the next request: fails `the_end_of_the_input_is_forgotten_for_the_next_request`
39. a reply that shifts the text the reader is looking at: fails `a_reply_does_not_move_the_person_who_is_reading_back`
40. the profile path unescaped in a message: fails `the_path_in_a_profile_message_is_escaped`
41. a reply printed at exit wrapped to another width than the terminal's: fails `a_reply_printed_when_the_interface_closes_is_wrapped_to_the_terminal`


Two mutations the task names cannot be written: "reply before the confirm or
the execute" and "reply text passed to `Confirm`, the broker or the planner",
because `reply_to` has no access to any of them. That is the design, and the
signature is the guard.

## A real session (Bonsai 8B, `./scripts/intent-shell-live-test.sh`)

The live test `the_model_writes_a_short_reply_in_each_profile_and_decides_nothing`
runs three requests against the real model, asks for the reply in each of the
four profiles for the same result, and asserts that the log gained only the
reply event each time.

```text
[~] > delete notes.txt in my downloads

  in ~

  Step  What                Needs      Verdict
  s1    Delete "Downloads"  fs.delete  REFUSED
          ↳ "Downloads" is a whole place and cannot be removed, moved or overwritten

  1 steps, 1 actions: 0 ok, 0 held for root, 1 refused.  The model says: "Delete notes.txt from my downloads"

  No step can run. Nothing ran.
(the result took 4077 ms)
--- direct-technical (+3479 ms)
  » Reply from the model (words only: it decided and ran nothing):
  » You requested to delete "notes.txt" in "my downloads". The system refused
  » the deletion because "Downloads" is a whole place and cannot be removed.
--- warm-didactic (+4917 ms)
  » Reply from the model (words only: it decided and ran nothing):
  » You cannot delete notes.txt because it is located in your Downloads folder,
  » which is a whole directory and cannot be removed or modified. Please check
  » if you have the correct path or file name.
--- formal-discreet (+5239 ms)
  » Reply from the model (words only: it decided and ran nothing):
  » The request to delete "notes.txt" in "Downloads" was refused because
  » "Downloads" is a whole place and cannot be removed, moved, or overwritten.
  » No action was taken as the system does not support deleting entire
  » directories.
--- good-humoured (+5273 ms)
  » Reply from the model (words only: it decided and ran nothing):
  » You tried to delete the whole folder named "Downloads," but it's not
  » possible — it's like trying to remove a room from a house. Let's see if we
  » can help with something else instead!
```

What a reply adds to a request: 1.2 to 2.1 s on the last run (machine quiet),
against 2.1 to 2.9 s for the planning and result of the same requests; 2.3 to
6.7 s on an earlier run while other work used the model. The reply costs about
as much as the plan; ADR 0021 expected seconds. The result is on screen first.

What the transcript also shows, honestly: the 1-bit 8B model follows the voice
loosely and sometimes gets a detail wrong (it says "1 item" for a ranking, or
writes in the first person). That is the case ADR 0021 designs for: the reply is
words under a result that is correct, marked as the model's. The prompt asks for
no numbers or names that are not in the facts; a better model, or a longer
prompt tuned with the S4 evaluation, is later work.

`a_path_and_a_relative_path_resolve_against_the_session_directory` in the same
file fails on `main` too (checked on a clean checkout of `origin/main`): the
model writes an absolute path with a `~` inside it. That is the planner, the
subject of ADR 0024 and S4b, and is not changed here.

## Limits and follow-ups

- The machine name of ADR 0021 decision 3 is not in this item. It belongs to the
  SOUL file (S8), and a free-text name field would be the free-text SOUL that
  ADR 0006 rejects until S8 decides its format. The prompt says "a computer's
  assistant".
- The "short length option" the ADR mentions is not added: the cap is fixed at 400
  characters and two sentences. S3d's `/commands` or S8 can add it.
- Per-user personality, the interview and the evolution of the SOUL are S8, S9 and S10.
- An address given as a host name is resolved by the system's resolver, which no
  deadline here can interrupt; the default `127.0.0.1:8089` is not affected.
- A reply is also asked for after "no plan", as ADR 0021 says, even when the
  server hung: after the planner's own wait the reply may wait up to 30 s more.
- Every request pays for its reply: a server that is slow or hung only for free
  text makes each request wait up to 30 s (then `reply_failed` is logged), with
  no back-off after a failure.
- The reply is one call per request with a fixed timeout of 30 s; the interface
  blocks typing while it waits. Streaming the reply would remove the wait and is
  not done.
