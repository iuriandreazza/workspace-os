---
title: "S5: deterministic fallback shell"
type: report
status: done
created: 2026-11-16
updated: 2026-11-16
tags: [shell, commands, broker, recovery, no-model]
---

# S5: deterministic fallback shell

Roadmap item S5, with the command syntax in ADR 0022. The same actions of the
plan format (ADR 0011, references of ADR 0014) are typed as commands with no
model loaded. A typed line becomes the plan document a model would emit, and
from there it is the model's pipeline: resolver, broker, plan screen as dry run,
the question of ADR 0016, a panel for each root action, execution and the
decision log.

## Run it

```bash
./scripts/intent-shell.sh --no-model                    # on a terminal: the interface, header says `"none (commands only)"`
./scripts/intent-shell.sh --no-model --plain            # the line adapter; also what runs with no terminal
./scripts/intent-shell.sh --no-model --administrator    # may confirm root actions
cargo run --target "$(rustc -vV | sed -n 's/^host: //p')" -p intent-shell --no-default-features -- --no-model   # no interface linked (recovery mode, ADR 0007)
```

`--no-model` never opens a connection and never reads a model address.
`help` prints the commands, built from the table the parser reads.

## Syntax in one screen

```text
fs find_largest --in downloads --count 3
fs delete --name big.iso --in downloads --kind file
fs delete --name big.iso --in downloads --kind file --elevate
fs write --new-name notes.txt --in documents --content 'two words' [--mode create|overwrite|append]
fs move --name a.txt --in documents --to-name archive --to-in documents
fs find_largest --in downloads --count 2; fs delete --from-step s1       # steps with ';'
process run --program tar -- -czf backup.tgz docs
user manage --operation add_to_group --user bia --group staff
```

The flags are the arguments of the schema with `_` written as `-`. References
are `--in P` (a place), `--name N --in P [--kind ...]`, `--new-name N --in P`
and `--from-step sN`. `--after sN` orders steps. Nothing is expanded (no `~`,
globs or `$`), and a path cannot be written (ADR 0014). The two defaults are
`--kind any` (the widest kind: a name that matches a file and a folder is the person's choice) and `--mode create`. The full grammar is in ADR 0022.

## What shares code with the model path

```text
typed line ─► command.rs ─► plan JSON ─► plan::parse_checked ─┐
model text ─────────────────────────────► plan::parse_checked ─┤
                                                               ▼
                       Shell::carry_out: resolver ─► broker ─► screen ─► confirm ─► execute ─► log
```

- `command.rs` only writes the JSON document. It builds no plan value, so every
  structural and semantic rule of the format applies to a typed command as to the
  model's output, and the two cannot drift.
- `Shell::handle_command` is a second entry to the function the model entry
  also ends in (`carry_out`). It never calls the planner, and a test uses a
  planner that panics if asked.
- A typed plan carries `Origin::Typed`; the plan screen then says `You typed:`
  where a model plan says `The model says:`. Nothing else on the screen changes.
- The log records one `command` event (the line, the document it became, whether
  it was accepted and the error) where a model plan records `request` and
  `model_output`. A typed command is never logged as a model decision. From
  `plan_accepted` on the events are the same.
- `Setup.planner` is an `Option`: `None` is this shell. The conversational
  interface runs it too (`State::without_model`: a submitted line is "working", not
  "asking the model", so quitting waits for the step as for any other).

## Tests and the gate

`./scripts/check.sh` runs them in both configurations (with and without the
interface). `command_tests.rs`, none needing a model:

- every plan example of the format (`docs/schemas/examples/valid`) is produced by
  a command line with identical steps, and a test fails when an example has no
  line or the other way round;
- every action in the schema's `step` list has a command and no command is
  invented (the schema is read in the test), and `--elevate` is accepted exactly
  where the root rule says a step may ask for root;
- quoting, escapes and 33 malformed lines, each refused with the reason, and 20
  lines that parse as words and are refused by the format's own rules (a place
  to delete, a count of 0, a name with `..`, `elevate` on `user.manage`, 17 steps);
- the same scenario typed and as the model's document gives the same log events
  (apart from the author's), the same questions, the same root panels, the same
  executed actions and the same outcome: a read that runs without asking, a
  delete that asks, a refused delete, a root action held for a non-administrator,
  a root panel for an administrator, an ambiguous name and the person's choice,
  a name not found;
- a log that cannot be written stops a command before it runs anything, including
  a refused one.

Mutation check, by hand (each change applied, the suite run, the source restored):

| Mutation | Killed by |
|---|---|
| a typed plan is not marked as typed | the screen test |
| `--kind` defaults to `file` | the defaults test |
| `--mode` defaults to `overwrite` | the defaults and example tests |
| `--from-step` adds no dependency | 3 tests |
| the typed line is not logged | the log test |
| a backslash is not an escape | 2 tests |
| `system configure` takes `--elevate` | 2 tests |

## Limits and follow-ups

- The host adapter executes reads only (ADR 0016, "Consequences"), so a delete,
  write, move, run, account or SOUL command reaches the confirmation and then
  reports `... is not executable in this prototype`. That is the host's state,
  not the shell's: every such command is parsed, resolved, judged and confirmed
  exactly as the model's.
- The syntax targets plan version 2. ADR 0018 (paths, a current directory,
  `session.cd`, version 3) is an item of its own; ADR 0022 says how the same
  table grows (`--path`, `--under-home`, `session cd`, and the `cd`/`ls`/`tree`/`rm`
  words as aliases that make the same documents).
- A line is one request: a plan of several steps is written on one line with `;`.
  A long `fs write` content is a long line.
- No history, completion or line editing in the line adapter; the interface has
  the input line only.
- Recovery mode as a boot entry is K13; this item gives it the shell to start.
