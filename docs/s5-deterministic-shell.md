---
title: "S5: deterministic fallback shell"
type: report
status: done
created: 2026-11-16
updated: 2026-11-17
tags: [shell, commands, broker, recovery, no-model]
---

# S5: deterministic fallback shell

Roadmap item S5, with the command syntax in ADR 0022. The same actions of the
plan format (ADR 0011, references of ADR 0014, paths and `session.cd` of ADR 0018) are typed as commands with no
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
fs list --path ~/Downloads/films
fs read --name budget.csv --under-home --kind file
session cd --path ~/Downloads
cd ~/Downloads; ls; rm films/old.mkv          # short words for the three most typed
fs write --new-name notes.txt --in-path ~/Documents --content 'two words' --mode append
fs move --name a.txt --in documents --kind file --to-name archive --to-in documents
fs find_largest --in downloads --count 2; fs delete --from-step s1
process run --program tar -- -czf backup.tgz docs
user manage --operation add_to_group --user bia --group staff
```

The flags are the arguments of the schema with `_` written as `-`. References
are `--in P` (a place), `--path P` (a path as typed), `--name N --in P|--in-path P
[--kind ...]`, `--name N --under-home [--kind ...]`, `--new-name N --in P|--in-path P`
and `--from-step sN`. `--after sN` orders steps. The command layer expands
nothing (no `~`, globs or `$`): a `--path` goes to the format as typed and the
resolver expands the `~` and cleans `.` and `..`, then the broker judges where it
lands. The defaults are `--kind any` where an action only reads (`fs write` on a
name, `fs delete` and the source of `fs move` require `--kind`; a `--path` has no
kind) and `--mode create`. The full grammar is in ADR 0022.

`cd PATH`, `ls [PATH]` and `rm PATH` expand to `session cd --path`, `fs list --path`
(the current directory when left out) and `fs delete --path` before parsing. ADR 0018
also names `tree --depth N`; it is not provided, because the format has no depth to
carry (ADR 0022, section 5). The prompt of the line adapter shows the directory
(`command ~/Downloads> `), and the header of the interface follows a `cd`, as for a
model session.

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
- A typed plan carries `Origin::Typed`; the plan screen then says `Typed commands:`
  where a model plan says `The model says:`. Nothing else on the screen changes.
- The log records one `command` event (the line, the current directory, the document
  it became, whether it was accepted and the error) where a model plan records `request` and
  `model_output`. A typed command is never logged as a model decision. From
  `plan_accepted` on the events are the same.
- `Setup.planner` is an `Option`: `None` is this shell. The conversational
  interface runs it too (`State::for_entry`: a submitted line is "working", not
  "asking the model", so quitting waits for the step as for any other).

## Tests and the gate

`./scripts/check.sh` runs them in both configurations (with and without the
interface). `command_tests.rs`, none needing a model:

- every plan example of the format (`docs/schemas/examples/valid`) is produced by
  a command line with identical steps, and a test fails when an example has no
  line or the other way round;
- every action in the schema's `step` list (now with `session.cd`) has a command and no command is
  invented (the schema is read in the test), and `--elevate` is accepted exactly
  where the root rule says a step may ask for root;
- the paths, `--in-path`, `--under-home` and `session cd`: about 30 refused lines
  (a `--path` is alone, `--in` and `--in-path` are two ways to say where, a lone
  `--in-path`, no kind where one is required, a NUL, an empty path, 4097 bytes), a
  path and a root deletion judged by the broker as for the model, and a typed `cd`
  that changes where the next line resolves, is logged, and stays put for a file,
  a missing folder or a line that mixes it with other steps;
- the short words equal their long forms, refuse a missing or second path and
  refuse `rm -rf x`, so a shell habit never deletes a file named `-rf`;
- quoting, escapes and about 35 malformed lines, each refused with the reason, and
  19 more that are well formed as words and refused either by the command layer or,
  behind it, by the format (a count of 0, a name with `..`, a group on `add`, a
  cycle, `elevate` on `user.manage`, a 17th step);
- the same scenario typed and as the model's document gives the same log events
  (apart from the author's), the same questions, the same root panels, the same
  executed actions and the same outcome: a read that runs without asking, a
  delete that asks, a refused delete, a root action held for a non-administrator,
  a root panel for an administrator, an ambiguous name and the person's choice;
  a name not found, a declined plan and a dead log are tested on the typed side;
- the shell itself (not only the helper) refuses a typed line that breaks a
  semantic rule of the format, and the forms each argument takes are compared
  with the schema, as `help` prints them;
- `tests/no_model.rs` runs the real binary with `--no-model --plain`: a read runs,
  a bad line is refused, a delete asks, root needs an administrator and gets its
  own panel, `--model` with `--no-model` is refused, and the log holds `command`
  and no model event;
- `tests/model_session.rs` runs the real binary with a model (a fake
  `llama-server` on a local port): a request reaches the model and is not read as a
  command, so the `Option` around the planner cannot break the model path unseen;
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
| the shell checks a typed plan with `parse` instead of `parse_checked` | the semantic-rules test |
| a session with no model still reads requests | the interface and binary tests |
| the interface waits for a model under no model | the state test |
| a session with a model reads commands, or uses the no-model planner, or loses its planner in `main` | `tests/model_session.rs` |
| `--in-path` written as a place (after the merge with S3e) | 2 tests |
| the short word `rm` takes `-rf` as a path | 1 test |
| the name of `cd` is not forced to a folder | 2 tests |
| `--under-home` ignored | 3 tests |
| `ls` with no path lists the home, not the current directory | 1 test |

## Limits and follow-ups

- The interface drops characters it would escape (`printable`), so a name with a
  no-break space or a variation selector cannot be typed or pasted there (a tab
  or a newline can, as `"\t"` and `"\n"`),
  and a paste is edited without a warning. That is the S3c rule for every line,
  here as for a request to a model, and not new; the line adapter takes them. A
  `\u{..}` escape in the grammar, or a paste that refuses what it would alter, is a
  follow-up.
- A value that ends the line with an escaped space loses it, because the line is
  trimmed before it is parsed; quote the value.
- The plan screen does not say whether a resolved name is a file or a folder; a
  follow-up for `screen.rs`.

- The host adapter executes reads only (ADR 0016, "Consequences"), so a delete,
  write, move, run, account or SOUL command reaches the confirmation and then
  reports `... is not executable in this prototype`. That is the host's state,
  not the shell's: every such command is parsed, resolved, judged and confirmed
  exactly as the model's.
- The syntax targets plan version 3 (S3e). The branch was written against version 2
  and merged with S3e; the merge brought `--path`, `--in-path`, `--under-home`,
  `session cd` and the short words in, and the gate tests that compare the table
  with the schema are what showed what was missing.
- `tree` and `--depth` (ADR 0018) are not typeable: the format has no depth.
- A line is one request: a plan of several steps is written on one line with `;`.
  A long `fs write` content is a long line.
- No history, completion or line editing in the line adapter; the interface has
  the input line only.
- Recovery mode as a boot entry is K13; this item gives it the shell to start.
