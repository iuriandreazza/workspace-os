---
title: "ADR 0016: Run without asking when nothing is changed or removed"
type: adr
status: accepted
created: 2026-11-16
updated: 2026-11-16
tags: [shell, broker, confirmation, ux]
---

# ADR 0016: Run without asking when nothing is changed or removed

## Context

ADR 0011 (step 6) and S3 make the intent shell show every plan and ask the
person before anything runs. After using the conversational interface (ADR
0015) the user decided that asking before a harmless step is friction: a plan
that only looks at things, or only adds something new, should just run.
ADR 0002 still holds: the model proposes, deterministic code decides, and every
decision is logged. ADR 0007 still holds: a root action is always confirmed,
one by one, with the exact action shown.

## Decision

The plan confirmation of ADR 0011 step 6 is skipped when **every action that
could run** (every step not blocked, every item not refused) is one of:

| Action | Why it needs no question |
|---|---|
| `fs.list`, `fs.stat`, `fs.read`, `fs.find_largest` | Read only |
| `fs.write` with mode `create` | Adds a file that does not exist; the broker already refuses to create over an existing one |
| `fs.write` with mode `append` | Adds to a file; does not remove what was there |

A **hidden path** (any component starting with a dot: `.zshrc`, `.ssh/key`)
always asks, even for the actions above, whether it is read, created or
appended to. Startup files and credentials are hidden files, and appending a
line to one is persistent code execution or a leaked key, not a harmless add.

The plan is confirmed, as today, when **any** such action is one of:
`fs.write` with mode `overwrite`, `fs.move`, `fs.delete`, `process.run`,
`user.manage`, `system.configure`, `soul.propose_change`, or anything held for
root. One destructive action in a plan confirms the whole plan, because the
person approves it as a unit.

The classification is a method on `Action` (`needs_confirmation`), decided by
code from the action table, never by the model: the model cannot mark a step as
safe. A new action defaults to needing confirmation, because the `match` has no
wildcard.

What does not change:

- The broker still judges every action (capability, the user's permissions,
  protected root). Skipping the question never skips the verdict.
- A refused step, a blocked step and the choice of an ambiguous reference
  behave as before. The choice is still the person's.
- A root action still gets its own panel and its own confirmation (ADR 0007).
- The decision log records `plan_auto_approved` in place of `plan_confirmation`,
  before anything runs, so the log still says why it ran.
- The result shows the plan table (the same one a confirmation shows) and a line
  saying it ran without asking, so the person sees what was done.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| **A. Skip only for read, create and append (chosen)** | Nothing the person had is lost or changed; the boundary is a closed list in one `match` | `append` and `create` can still fill a disk or add a file the person did not want; they can be removed by a later, confirmed delete |
| B. Skip only for read-only actions | Strictest; nothing is written without asking | Asks before "create notes.txt", which the user considers harmless |
| C. Never skip (S3b, ADR 0011) | Every effect is seen first | The friction the user asked to remove |
| D. A per-user setting | Each person chooses | A policy knob in the trusted path, and a default still has to be chosen |

## Consequences

- When the host gains writes (it executes only reads today), `append` must use
  `O_NOFOLLOW` and `create` must use `create_new`, so a symlink cannot redirect
  an unasked append and a file that appears after the review is never
  overwritten. This is a condition of enabling writes, with its own test.
- An unasked `fs.read` shows file content in the terminal and writes a
  truncated copy to the decision log. The user's permissions bound it (ADR 0007)
  and hidden paths ask, but a readable non-hidden file with secrets in it is
  read without a question. Accepted by the user's decision above; redacting
  read output in the log is a possible later change.

- `session.rs` decides from the review whether to ask; the terminal adapters do
  not change, and a plan that is not asked about simply produces no question.
- Tests pin both sides: each auto action runs without a question, and each
  destructive action, root action and mixed plan still asks.
- ADR 0011 step 6 stays accepted for plans that are asked about; this ADR
  supersedes it only for the actions in the table.
