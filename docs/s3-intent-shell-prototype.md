---
title: "S3: intent shell prototype on the host"
type: report
status: done
created: 2026-11-15
updated: 2026-11-15
tags: [slm, shell, broker, capabilities, prototype]
---

# S3: intent shell prototype on the host

Roadmap item S3. The model proposes, deterministic code decides (ADR 0002):
`intent-shell` turns a request into a plan in the ADR 0011 format, the broker
validates every action, the person sees what would run and confirms, and every
decision is logged.

## Run it

Start the model (the weights are in `~/models/bonsai-8b/`, see S1 for how they
got there):

```bash
llama-server -m ~/models/bonsai-8b/Bonsai-8B-Q1_0.gguf -c 8192 --parallel 1 --port 8089 --jinja -ngl 99
```

Then, in another terminal, from the repository root:

```bash
./scripts/intent-shell.sh                 # an ordinary user
./scripts/intent-shell.sh --administrator # may confirm root actions
```

Options: `--model HOST:PORT` (default `127.0.0.1:8089`) and `--log FILE`
(default `~/.local/state/workspace-os/intent-shell/decisions.jsonl`). The
shell reads a request per line; it asks `[y/N]` before running the plan and
again for each root action.

The test that needs the real model is outside the gate, because CI has no
model:

```bash
./scripts/intent-shell-live-test.sh       # INTENT_SHELL_MODEL=host:port to override
```

It checks the plumbing (constrained output reaches the broker, declining runs
nothing), not plan quality.

## What it does

```text
request ─► model (llama-server, output constrained by the schema file)
        ─► parse + semantic rules      ─ fail: one retry with the error, then
        │                                "could not make a plan", nothing runs
        ─► broker: capability ∩ user permissions, root, expansion of references
        ─► rendering of the exact actions ─► confirm plan ─► confirm each root action
        ─► execute ─► every step above appended to the decision log
```

Code is in `intent-shell/src/`: `plan.rs` (format, rules), `broker.rs`,
`session.rs` (the pipeline), and adapters behind the traits of `ports.rs`:
`host.rs` (Unix permissions through `access(2)`, real read actions),
`model.rs` (llama-server client), `log.rs` (JSON lines).

Behaviour worth knowing, all covered by tests that need no model:

- A document that fails the schema or a semantic rule runs nothing, not even
  its valid steps. A *step* the broker refuses is different: it and its
  dependents are refused, and independent steps still run, but only after the
  person confirms the plan as the broker rendered it, refusals included.
- Root is decided by the broker. `user.manage` and `system.configure` are
  always held; `elevate: true` holds the others. A step that fails the user's
  permissions without `elevate` is refused, never promoted. A user who is not
  an administrator cannot elevate. Confirming the plan does not confirm a root
  step: each is asked on its own, shown exactly, and logged with who confirmed.
- A `from_step` reference is expanded before confirmation: the broker runs the
  read-only producer (`fs.list`, `fs.find_largest`) itself, then shows one
  concrete action per real path. It never runs a held or mutating step for
  this, and refuses to expand more than 100 paths or a reference to a step
  whose paths are not known yet. A plain string such as `"s1.path"` is a
  literal path.
- What is shown is the broker's rendering with every string JSON-quoted, so a
  control character in a path or content cannot rewrite the terminal. The
  model's `summary` appears only labelled as the model's claim.
- The shell runs nothing while the decision log cannot be written: the plan is
  logged before the broker may read anything, each read that expands a
  reference is logged just before it runs, and every action is logged before
  it runs and after.
- Inside one step that expands to several actions, each action is decided on
  its own: a refused path is labelled in the rendering and skipped, the others
  run once the plan is confirmed, and steps that depend on the step are
  blocked.

### What really executes

Only `fs.list`, `fs.stat`, `fs.read` and `fs.find_largest`. Every other action
in the format is validated, rendered and confirmed like the rest, then
reported as "not executable in this prototype". Nothing here writes, deletes,
starts a program, or touches accounts, and there is no `sudo`: a confirmed
root action of a read runs with the user's own privileges on the host. The
second factor of ADR 0007 (the person's password) is not implemented.

### Open items of ADR 0011 settled here

- `process.run` registry: empty (`broker::PROGRAM_REGISTRY`), so every
  `process.run` is refused until programs are vetted.
- `system.configure` keys: none accepted (`broker::CONFIG_KEYS`).

## Design decisions

**Where host code lives.** A new workspace member `intent-shell/`, built with
`--target "$host_target"` exactly like `devicetree/` in `scripts/check.sh`.
`.cargo/config.toml` keeps the bare-metal target for everything else, and the
workspace gets `default-members = ["kernel", "devicetree"]` so a bare
`cargo build` and `cargo run` still mean the kernel (checked: `cargo run` boots
and prints the banner). Alternatives: a second workspace (two lockfiles, two
gates) or a `[target]` override per package (does not exist in Cargo).
`scripts/intent-shell.sh` supplies the host target for running it.

**One schema, two readings.** The model is constrained by
`docs/schemas/action-plan.schema.json` (embedded with `include_str!`); the
broker re-reads the output with typed `serde` parsing instead of a JSON Schema
crate, which would add many dependencies for one file. ADR 0011 asks for the
same artifact on both sides, so this is a deviation, contained by a test that
runs every example next to the schema through the Rust parser: valid ones
pass, `schema-*` ones fail the structural half, `semantic-*` ones pass it and
fail the rules. A change to the schema that the Rust side ignores breaks that
test only if an example shows it; a new field needs a new example.

**Dependencies.** `serde`, `serde_json` and `libc`, plus their dependencies
(`itoa`, `memchr`, `proc-macro2`, `quote`, `syn`, `unicode-ident`, `zmij`,
`serde_core`, `serde_derive`). Every licence is MIT or dual MIT/Apache-2.0
(used under MIT), `memchr` is `Unlicense OR MIT`, `zmij` is MIT and
`unicode-ident` is `(MIT OR Apache-2.0) AND Unicode-3.0` (a permissive data
licence). Nothing is Apache-only, so ADR 0005 holds. HTTP to `llama-server` is
written by hand over `std::net`: the server is local, so no TLS crate (and
none of the licences that come with one).

**Layering.** `plan`, `broker` and `session` depend only on the traits in
`ports.rs` (`Host`, `Planner`, `Confirm`, `DecisionLog`); the terminal, the
filesystem, the model and the log are adapters. The pipeline tests inject
fakes for all four.

## Tests and the gate

`./scripts/check.sh` now also runs clippy and 60 tests for `intent-shell`,
none needing the model. They include one test per refusal class (no
capability, beyond the user's permissions, root without confirmation, a
non-administrator elevating, malformed output, rule violations) and the 27
schema examples.

Mutation check, done by hand (no mutation tool was installed): each change
below was applied to the source, the tests were run, and the source restored.
Every mutant was killed.

| Mutation | Failing tests |
|---|---|
| capability check dropped | 1 |
| user permission check dropped | 3 |
| root hold dropped | 6 |
| a non-administrator may elevate | 2 |
| a refused dependency does not block dependents | 1 |
| semantic rules skipped | 1 |
| answer to a root confirmation ignored | 1 |
| plan confirmation ignored | 1 |

With the permission check weakened to a condition that never holds,
`./scripts/check.sh` exited with status 101 at "unit tests" and three tests
failed.

## What the model did (for S4, not fixed here)

Bonsai 8B, temperature 0, grammar-constrained, through the prompt in
`model.rs`. The structure was always valid; the meaning often was not. Ten
requests were tried by hand, so this is an observation, not a measurement:

- "list the largest files here" (the S3 done criterion) became
  `fs.find_largest` with the right directory and `count: 1`: valid, runnable,
  and only the single largest file. Another run wrote the path as
  `/Users/<name>/`, the working directory cut short.
- "delete the two largest files in DIR" became only `fs.list`: no delete, no
  reference. "find the 2 largest files in DIR and delete them" did the same.
  "show the 3 largest files" became `fs.list`.
- "find the 2 largest files, then run stat on each" produced `fs.find_largest`
  and two `fs.stat` steps with invented literal paths (`DIR/1`, `DIR/2`)
  instead of `from_step`. The broker refused both (no such path to read), and
  the first step still ran after confirmation.
- "create a file notes.txt with the text hello" became an `fs.move` from the
  working directory to the target directory: wrong action, shown and not
  confirmed.
- "add the user bia to the group staff" failed validation twice (a group
  operation without a group, per the semantic rule) and ended in "could not
  make a plan" with nothing run.
- "delete the largest file ..., as administrator" set `elevate` but planned
  `fs.list`; the broker held it for root confirmation, and it ran only after
  the second `y`.

No run produced a `from_step` reference, so the expansion path is tested only
with canned plans. Nothing the model got wrong ran anything the person did not
see first, which is the point of the broker; the quality is S4's problem.

## Limits and follow-ups

- Time of check against time of use: the permission check and the action are
  not atomic. On the host the OS still enforces the real permissions at run
  time, so a race can fail an action but not widen it.
- A path that does not exist is reported as "your permissions do not allow",
  because `access(2)` failing is all the check sees. Misleading, not unsafe.
- A search that could not look everywhere (unreadable directories, or the
  100,000-entry cap) says so as a `NOTE` next to the plan and next to the
  result, because a reference expands from it.
- Escaping covers the known invisible, blank and bidirectional characters, not
  look-alike letters from other scripts: two names that differ only by such
  letters still look the same in the confirmation.
- Reads are capped (64 KiB for `fs.read`, 100,000 entries for
  `fs.find_largest`); `fs.read` refuses anything that is not a regular file,
  and `fs.list` has no cap on entries.
- The log and its directory are private to the user (0600, 0700), also when
  the log file already existed.
- The parser is stricter than the schema in two harmless ways: it refuses
  `3.0` where the schema's `integer` accepts it, and `"s1\n"` as a step id.
- A step that depends on a read nobody references cannot itself be expanded
  before confirmation, so a longer chain through such a step is refused
  (`CannotExpand`) instead of run: strict, not unsafe.
- The log records the user's name and the full request text, locally. It is
  not rotated.
- S5 (typed commands into the same plan) and S4 (evaluation) are next.
