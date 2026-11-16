---
title: "ADR 0022: Command syntax of the deterministic shell"
type: adr
status: proposed
created: 2026-11-16
updated: 2026-11-16
tags: [shell, commands, broker, recovery, no-model]
---

# ADR 0022: Command syntax of the deterministic shell

## Context

ADR 0002 says the system works with no model, and ADR 0007 says recovery mode
runs the deterministic shell only. ADR 0011 and ADR 0014 sketch how that shell
is typed: the action name, then flags that map mechanically from the action's
arguments (`fs find_largest --in downloads --count 3`), with the parsed plan
going through the same schema, rules, resolver and broker as a model plan.
They leave open what a mechanical mapping cannot say by itself: how two
references in one action are written (`fs.move`), how steps refer to each other
and to ordering, quoting, `process.run` arguments, defaults, how the mode is
entered, and what the person sees where the screen says "The model says".

This ADR extends ADR 0011 and ADR 0014. It supersedes, for the deterministic
shell only, three sentences of ADR 0011: "One command line parses into a one-step
plan; a script is a list of lines that parse into a plan" (a line is a plan of one to 16 steps separated by `;`, because the
interface has a single-line input and a line adapter has no way to say "more
follows") and "`$s1`-style references" (`--from-step s1`, because `$` invites
the expansion this shell must not have). The `--path` of ADR 0011 was already
superseded by ADR 0014. Nothing else in either ADR is reopened. It targets
the plan format as it is in the code (version 2). ADR 0018 moves the format to
version 3 (paths, a current directory, `session.cd`) in an item of its own; the
last section says how this syntax grows with it.

## Decision

ADR 0014 sketches `fs write --new-name notes.txt --in documents`. The schema
requires `content`, so the typed line adds `--content`.

### 1. One path: a command line becomes a plan document

```text
typed line ─► command.rs ─► plan JSON, version 2 ─► plan::parse_checked
                                                   ─► the model path from here on:
                                    resolver ─► broker ─► screen ─► confirm ─► execute ─► log
```

The command parser emits the same JSON document the model would, as text, and
that text goes through `parse_checked`. It never builds a plan value directly,
so every structural and semantic rule (which reference an argument accepts, `count` 1 to 100, a name has no separator,
dependencies and cycles, bounds)
applies to a typed command exactly as to the model, and cannot drift. The shell
from the document onward is one function shared by both entries: resolution,
the person's choice of an ambiguous reference, the broker's verdict, the plan
screen as dry run, the confirmation of ADR 0016, a separate confirmation of
each root action (ADR 0007), execution and the decision log. Typing a command
adds convenience, never reach (ADR 0002 rule 2): nothing the broker refuses
runs, and a command that asks for something the format cannot say is refused at
parsing.

The model's retry (ADR 0011) does not apply: a typed command that does not
parse is refused at once with the reason, and nothing runs.

### 2. Entering the mode

`intent-shell --no-model` starts the shell with no model: it does not read a
model address, never opens a connection, and treats every line as a command. It
works with the line adapter and with the conversational interface (ADR 0015);
the header says `model  "none (commands only)"`. Recovery mode starts it this way,
from a build without the interface (`--no-default-features`). There is no
automatic switch from a model that is down: ADR 0011 already says the shell
reports it could not make a plan and points here; the person restarts with
`--no-model`.

A session with a model does not read commands. `/commands` of the model
interface (S3d) are another feature and do not share this grammar.

### 3. Grammar

A line holds one plan: one to 16 steps separated by `;`.

```text
step      = group action {flag}           e.g.  fs delete --name big.iso --in downloads --kind file
group     = fs | process | user | system | soul
action    = the part of the action name after the dot:  fs.find_largest is `fs find_largest`
```

Typed lines also accept `help`, which prints the commands and flags, produced
from the same table the parser uses, and does nothing else.

**Tokens and quoting.** No expansion of any kind: no `~`, no globbing, no
variables, no command substitution, no `$`. Whitespace separates tokens; an
unquoted `;` ends a step. `'...'` is literal; `"..."` allows `\\`, `\"`, `\n`
and `\t` and refuses any other escape; outside quotes `\x` is a literal `x`.
Quoted and unquoted pieces next to each other make one token, and `""` is an
empty token. An unterminated quote or a trailing `\` is an error.

**Flags.** A flag is the name of an argument of the schema with `_` written as
`-`, after `--`, followed by its value. A flag appears at most once, except `--after`, which repeats and ignores a repeat of the same step. The
schema's argument is the only source: an unknown flag, a missing required flag,
a repeated flag or a value of the wrong type is an error naming the flag where the command layer sees it (the format's own
message, which names the argument, otherwise), and is never guessed. Flag values are always the next token. A value that starts with `--` must be
quoted or escaped (`'--x'`, `\--x`), or it is read as the next flag; a single `-`
is an ordinary character (`--count -1` is refused by the number check, not by
the tokenizer). `--elevate` takes no value. Nothing else is boolean.

| Action | Flags |
|---|---|
| `fs list` | reference |
| `fs stat`, `fs read`, `fs delete` | reference |
| `fs find_largest` | reference, `--count N` |
| `fs write` | reference, `--content TEXT`, `--mode create\|overwrite\|append` (default `create`) |
| `fs move` | source reference, destination as `--to-in`, `--to-name`, `--to-new-name` |
| `process run` | `--program NAME`, then `--` and every following token of the step is an argument |
| `user manage` | `--operation add\|remove\|add_to_group\|remove_from_group`, `--user NAME`, `--group NAME` (only for the group operations) |
| `system configure` | `--key KEY`, `--value TEXT` |
| `soul propose_change` | `--rationale TEXT`, `--new-text TEXT` |

Every `fs` and `process` action also takes `--elevate`, a request for root that
the broker decides (ADR 0011). `user manage`, `system configure` and `soul
propose_change` do not take it, as in the schema.

**References** are the three forms of ADR 0014 and `from_step`:

| Flags | Reference |
|---|---|
| `--in downloads` | `{"place":"downloads"}` |
| `--name big.iso --in downloads [--kind file\|folder\|any]` | `{"name":…,"in":…,"kind":…}`; `--kind` defaults to `any` where the action only reads; it is required for `fs write` on a name, `fs delete` and the source of `fs move` |
| `--new-name notes.txt --in documents` | `{"new_name":…,"in":…}` |
| `--from-step s1` | `{"from_step":"s1"}` |

An action takes only the forms its argument accepts in the schema (a place is
for `fs list`, `fs find_largest` and a move's destination, never for `fs
delete`); the command layer says which flags the action takes, `help` lists them
per action, and a test compares both with the schema. Mixing forms is an error. The reference of `fs move`'s destination uses the same
flags with `--to-` in front; its `--to-name` is always a folder, so `--to-kind`
does not exist. There is no flag in which a path can be written (ADR 0014).

**Steps.** Step ids are `s1`, `s2`, ... by position. A step depends on nothing
unless it says so: `--after s1` (repeatable, at most 15) adds a dependency, and
`--from-step s1` adds the dependency on `s1` itself, which the format requires.
Steps are independent otherwise (ADR 0011). The document's `summary`, which the
format requires, is the list of the typed action names, never user text, so
it cannot exceed its bound.

### 4. What the person sees and the log

The plan screen says `Typed commands:` followed by that list where a model plan says
`The model says:`, so the screen never attributes a typed command to a model.
Everything else on the screen is the same function. The log records
`command` (the typed line, the generated document, and whether it was accepted
with the error) where a model plan records `model_output`, and then the same
events: `plan_accepted`, `search`, `choice`, `review`, `plan_confirmation` or
`plan_auto_approved`, `root_confirmation`, `execute`, `result`. A typed
command is never logged as a model decision (ADR 0002 rule 3 concerns the
model, and the log is what retraining reads). The first write of a typed
command is its own record, and the shell stops before anything when the log
cannot be written. That holds for the recovery mode too (ADR 0002 rule 3 and
ADR 0011): it starts only where the decision log can be opened, and a recovery
boot has to give it a writable place (K13).

### 5. Growth with ADR 0018

When the format gains version 3, the same mechanical rule adds
`--path TEXT` as one more reference, `--under-home` for the renamed search, and
`session cd` for `session.cd`. ADR 0018 also names `cd`, `ls`, `tree` and `rm`
as the S5 words; they are shorthand for `session cd`, `fs list`, `fs list` shown
as a tree and `fs delete`, and arrive with that item, as aliases that produce
the same documents, so the mechanical table stays the only definition.

## Alternatives considered

| Topic | Chosen | Not chosen, and why |
|---|---|---|
| Where the command meets the core | Emit the plan JSON, run `parse_checked`, share the shell from the document on | A `Planner` that returns the JSON: no core change, but the model's retry would repeat a deterministic error, typed commands would be logged as `model_output`, and a typo would be reported as "the model could not be reached" |
| | | Building `Plan` values directly: avoids the text round trip, but copies the private structural rules of `plan.rs` and lets two readings of the format drift |
| Flags | Names from the schema, `_` as `-` | Positional arguments: shorter, but not mechanical, and a new argument would need a decision |
| Action words | `fs delete` | `fs.delete`: no ambiguity either, but ADR 0014's examples use two words, and the dotted form is the model's, not a person's |
| Several steps | `;` inside one line | A script of one step per line: needs a line adapter with state ("more to come") and cannot work in the single-line input of the interface. Many independent requests are already many lines |
| References between steps | `--from-step s1` | `$s1`: ADR 0011 hints at it, but `$` invites the expansion this shell must not have |
| Elevation | Explicit `--elevate`, decided by the broker | Prompting for root on refusal: it would promote a refused step, which ADR 0011 forbids |
| Defaults | `--mode create`, and `--kind any` only where an action reads | None: safer to read but unusable. `create` is the weakest mode (it refuses to overwrite). `any` is the widest kind, and the plan screen does not say whether a resolved name is a file or a folder, so a default there could make a folder delete look like a file delete. `fs write` on a name, `fs delete` and the source of `fs move` therefore require `--kind` (ADR 0014's example writes it too); a name that only reads defaults to `any`, and an ambiguity is the person's choice |

## Consequences

- The gate keeps the table honest: a test lists the actions in the schema and
  fails when one has no command, and another turns each example of the format
  into the commands that produce the same plan.
- `Shell` has a second entry that shares everything after the document with the
  model entry, so a change to confirmation, resolution or the log applies to
  both.
- A typed `fs.write` carries its content on the line, so a long content is
  impractical; the format's bound is 64 KiB and the line is the limit.
- The model's refusal text now says to restart with `--no-model` to type commands.

## Open questions

- `soul propose_change` is a command, because every action of the format is
  reachable (S5). In recovery mode no SOUL is loaded (ADR 0007), so it only
  records a pending proposal, which is all ADR 0011 lets the action do; applying
  one needs a root confirmation of the full diff and belongs to S10.

- Whether recovery mode (K13) starts this shell as a user program of the OS
  or the host binary stays a separate tool; this ADR fixes the grammar only.
- Command history and completion in the interface: later, as adapter features.
