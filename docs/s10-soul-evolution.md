---
title: "S10: SOUL evolution, the root-approved diff, history and undo"
type: report
status: done
created: 2026-11-26
updated: 2026-11-26
tags: [shell, soul, root, history, undo, broker]
---

# S10: SOUL evolution

Roadmap item S10, under ADR 0006 point 4, ADR 0011 ("Proposing a change to the
SOUL"), ADR 0007 (root authority) and ADR 0036 (accepted 2026-11-26), which
holds every decision below. The AI proposes a change to its own SOUL as an
action of the plan; it is applied only after a user with root authority has
approved the whole diff; every applied change is in a history and can be undone.

## Run it

```bash
./scripts/intent-shell.sh --administrator                    # ask: "make your tone formal and discreet"
./scripts/intent-shell.sh --administrator --no-model         # the same, typed
./scripts/check.sh                                           # the gate, with every test below
INTENT_SHELL_MODEL=127.0.0.1:8099 ./scripts/intent-shell-live-test.sh   # the live test, with a model up
```

Typed, with no model (ADR 0022):

```text
soul propose_change --rationale "shorter answers" --new-text "format: 1\npreset: direct-technical\nlanguages: any\n"
soul history
soul undo_change
```

## What was built

| Piece | Where | What it does |
|---|---|---|
| Two actions | `action.rs`, `plan.rs`, `command.rs`, schema | `soul.undo_change` and `soul.history`, both without arguments. `soul.propose_change` and `soul.undo_change` are root `always`; `soul.history` only reads. Schema version stays 3 |
| What is approved | `soul_change.rs` | The proposal is rewritten by the SOUL parser (`canonical`), checked by the file rules (2048 bytes, grammar), refused if equal to the file in place; the diff is every line of both files, each marked; `undo_target` picks the latest change no undo reverted |
| The panel | `screen::soul_panel` | The whole diff behind marks the code draws, the model's reason quoted and labelled, what the change does to the approver's own personality, and a footer the code draws. Escaped line by line, frame characters and characters that may be wide escaped, untrusted text cut in 64-character pieces behind prefixes the code draws |
| The write | `soul_files.rs` | `flock` on `soul.history/.lock`; history entry written complete as `NNNNNNNN.pending`, the SOUL swapped by `rename`, the entry committed as `NNNNNNNN.json`; a leftover is settled by the next write; the base is checked again under the lock |
| Approval | `Approval::from_root` | A value only an administrator's confirmation makes; the store cannot write without one |
| The session | `Shell::change_soul` | Prepare, show, ask, write, and one `soul_change` log event for every end |

## The four cases of the criterion

Each is a test in `intent-shell/src/soul_change_tests.rs`, in the gate, and each
was confirmed by breaking the behaviour (below).

| Case | Test | Result |
|---|---|---|
| Refused without approval | `a_proposed_change_is_refused_without_approval` | The panel is shown, the administrator says no: the step is "not run", the SOUL bytes are unchanged, no history |
| Refused when approved by a user without root authority | `a_proposed_change_approved_by_a_user_without_root_authority_is_refused` | The broker refuses at review, nobody is asked, the SOUL and the history are unchanged even though the fake person says yes to everything |
| Applied with a root approval | `a_proposed_change_approved_with_root_authority_is_applied` | The file is the canonical text; entry 1 holds who, why, before and after; the panel held every line of the diff; the log holds the file written |
| Undone from the history | `an_applied_change_is_undone_from_the_history` | The file is as it was; entry 2 is an undo of 1 |

## Decisions the tests hold

| Question | Decision and the test that holds it |
|---|---|
| Two undos | Go back two changes (`two_undos_go_back_two_changes_and_a_third_has_nothing_to_undo`). An undo is never undone, so a third says "no change to undo" (`an_undo_is_not_itself_undone_and_a_change_after_it_can_be`) |
| A SOUL that changed while the approver looked | The write is refused (`a_soul_that_changed_while_the_approver_looked_is_not_overwritten`); the same at the store (`a_write_checks_the_base_it_was_approved_on`) |
| Two sessions at once | One wins, the other is "stale" or "locked", the history has one entry (`two_sessions_changing_the_same_soul_leave_one_change_and_a_consistent_history`, `a_held_lock_is_a_refusal_and_not_a_wait`) |
| A process that dies | After the swap: the next write commits the entry. Before it: the entry is deleted. A torn entry and a stray temporary file are swept (three tests) |
| Damaged or edited history | Five kinds of damage stop the listing, an undo and a change, and change nothing; a history entry that would restore text that is not a SOUL is refused; a SOUL edited outside the history cannot be undone from it; a missing directory is an empty history |
| Text posing as the panel | Hostile lines in the current file and in the model's reason cannot make a line that starts like the panel's frame or an escape or a tab (`nothing_in_the_files_or_the_models_words_can_pass_for_the_panel`); comments in the proposal never reach the panel or the file (`the_approver_sees_the_canonical_text_never_the_models_bytes`) |
| Personalities | The SOUL wins; no personality file is written; the panel says whether the approver's own stays valid; undo restores validity without work (`the_panel_names_what_the_change_does_to_the_approvers_personality`) |
| The broker and the SOUL | Verdicts and `review` events are the same under any SOUL (`the_broker_gives_the_same_verdicts_and_log_under_any_soul`) |
| What the log holds | The whole proposal, even when refused (`the_log_holds_the_whole_proposal_even_when_it_is_refused`); the file written when applied; the outcome word for every refusal |
| Links, the S8a file | A link is followed and stays a link; one that leads nowhere is refused; the one-word file is rewritten in place as format 1 |

## Mutation checks

Every row: the line was broken, the named tests failed, the line was restored.

| Broken | Failed |
|---|---|
| `soul.propose_change` root rule `Never` | 24 tests, among them the non-administrator case |
| The broker lets a non-administrator through | the non-administrator case and the log test |
| `Approval::from_root` for anyone | `only_an_administrator_can_make_an_approval` |
| No base check at the write | the stale case and `a_write_checks_the_base_it_was_approved_on` |
| The model's bytes written instead of the canonical text | `the_approver_sees_the_canonical_text_never_the_models_bytes` and the no-change test |
| The "not confirmed" branch skipped | `a_proposed_change_is_refused_without_approval` |
| Undo ignores which changes were undone | three undo tests |
| A `.pending` always deleted, and always committed | the after-swap test, the before-swap test |
| No lock | `a_held_lock_is_a_refusal_and_not_a_wait` and the two-sessions test |
| Panel lines not escaped | the hostile-text test |
| No size limit, no "no change" check, diff without kept lines, a fixed personality line, no base check on undo, no gap check or number check in the history | one named test each |

## With the real model

`llama-server` was available, with Bonsai 8B, started by this session on port
8099 and stopped afterwards; nothing on 8089 was touched. The live test asks "change your SOUL so that you are
formal and discreet, and keep the name Atlas" for a machine whose SOUL is
`warm-didactic`, named Atlas, twice: declining at the panel and approving it.

1. **First prompt:** the model wrote `new_text` as `key: value\nformality: ...`
   (it copied the placeholder) and added an `soul.undo_change` step nobody asked for.
   The proposal was refused with "the first line must be format: 1" and the undo
   did not run. The SOUL stayed as it was.
2. **After the prompt gained a concrete example and "undo only when the user asks to go back"**
   the model wrote a valid file. Panel as shown, copied from the run:

```text
╔═ ROOT CONFIRMATION ═ SOUL change ═ asked of "tester" ═ step s1
║ Action  Replace the SOUL
║ The model's reason, in its words and not part of the change: "Change SOUL to be formal and discreet with the name Atlas."
║ The whole file, line by line: 1 removed (-), 1 added (+), 3 kept
║   format: 1
║ - preset: warm-didactic
║ + preset: formal-discreet
║   name: Atlas
║   languages: any
║ Personality  you have no personality file, so nothing of yours changes
║ Needs   soul.propose, as root. This is written exactly as shown, and the new SOUL is read at the next start.
╚═
```

Declining left the file untouched and created no history; approving wrote history
entry 1. One sample on one model is not a measurement: the S4 evaluation was
**not** re-run for the prompt change, and the model never sees the current SOUL,
so it keeps the name only because the request says so.

## Limits

- `--administrator` is a flag, not authentication (ADR 0032). Anyone who can run the shell with the flag can approve; S11b and S12 are what change that.
- The host prototype keeps the SOUL in the user's own `~/.config`, so the user can edit the file by hand with no panel. The history refuses an undo after such an edit rather than hide it. The root-owned system path comes with the installer (ADR 0028).
- No stored queue of proposals: a non-administrator's proposal is refused at review and stays in the log (ADR 0036, "What changes in ADR 0011").
- A diff that is long does not fit a short terminal, and the interface then offers only to refuse; `--plain` or a taller terminal is the way.
- A running session keeps the persona it started with; the new SOUL is read at the next start.
- The model writes a proposal blind. A proposal that drops the name shows as a removed line, and the person refuses it.
- The pre-commit gate takes about eight minutes, because the boot smoke tests run in it.
