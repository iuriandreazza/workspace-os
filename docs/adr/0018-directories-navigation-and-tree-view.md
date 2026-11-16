---
title: "ADR 0018: Directories, navigation and the tree view of the intent shell"
type: adr
status: accepted
created: 2026-11-16
updated: 2026-11-16
tags: [shell, resolver, navigation, tree, ux, schema]
---

# ADR 0018: Directories, navigation and the tree view of the intent shell

## Context

ADR 0014 (S3b) removed paths from the intent shell: the model names a place, a
name inside a place or an item to create; deterministic code resolves it; the
person never sees a current directory. After using the shell (S3b, S3c) the
user concluded that this cannot hold: "report.pdf in everywhere" is ambiguous
even for a human, and the person ends up asking the shell where things are.
Directories and navigation come back, and files shown in the terminal are
displayed as a **tree**.

This ADR supersedes the parts of ADR 0014 that say the person never navigates
directories, the shell has no current directory, and the default view shows
names without locations (and the `everywhere` place as the way to search).
Everything else in ADR 0014 stands: the resolver runs before the broker and is
blind to permissions, the broker judges the real resource, an ambiguous match
is a choice the person makes, the root panel shows the exact action, and
`Action` is built only from resolved resources. ADR 0002, 0007, 0011, 0015
and 0016 stand (0016's hidden-path rule applies to every path, typed or found).

## Decisions

### 1. How a location is named

| Option | For | Against |
|---|---|---|
| A. The model writes real paths, `~` and relative paths (ADR 0011 as first written) | Simple, familiar, one reference type | A model can invent a path; every plan names a place by spelling; `..` and symlinks need rules; loses the closed vocabulary the grammar enforced |
| **B. Paths as one more reference, next to the existing ones (recommended)** | `{"path":"~/Downloads/films"}` for what the person typed or sees in the tree; `place`, `name`+`in` and `from_step` stay for what the model cannot know. The resolver canonicalises, the broker judges, nothing else changes | Two ways to say a thing; the model must be told when to use which |
| C. Only a current directory and bare names (`cd`, then `big.iso`) | Closest to a shell, smallest vocabulary | Every plan depends on hidden state; a plan built in one directory means another elsewhere; hard to review |

Under B a path is **absolute, `~`-relative or relative to the current
directory** (decision 2). The resolver lexically cleans it (no `..` that leaves
the root, no NUL, bounded length), then canonicalises through the host. The
result is a real location like any other, and from there the broker judges it
exactly as for a resolved name. `place` remains a shortcut and `everywhere` is
renamed `under_home` in the schema: a search below the person's home, shown as
such, never presented as a place.

User space and system space: the starting point is the person's home. A path
outside it (`/etc`, `/usr`) is allowed to be *named*, because the broker, not
the vocabulary, protects the system, and the tree marks it `system`. Root
actions keep their own panel with the exact location (ADR 0007).

### 2. Navigation: a current directory

| Option | For | Against |
|---|---|---|
| **A. Current directory is session state in the core `Shell` (recommended)** | One rule for the terminal, the interface and S5; testable without a terminal; the log records it | The core gains state; a plan built under one directory runs under it |
| B. State in each terminal adapter | Core stays stateless | Two adapters, two behaviours; S5 and the interface can disagree |

The current directory starts at the person's home, is a resolved real
location, and appears in the header and in every plan screen. Plans store the
directory they were made under and **the resolved locations, not the relative
text, are what gets confirmed and logged**, so a later `cd` cannot change a
reviewed plan. The model is told the current directory in the prompt (about 20
tokens). `cd` and `ls`/`tree` are actions of their own:

- `session.cd {target}`: changes only the session's directory. It needs the
  read permission of the user on the target and no capability; it never asks
  (ADR 0016), and is logged.
- `fs.list` is unchanged; the tree is how its result is shown (decision 3).

S5 types the same: `cd ~/Downloads`, `ls`, `tree --depth 2`, `rm films/old.mkv`.

### 3. The tree view

Listing results, `find_largest` results and the candidates of an ambiguous
reference are shown as a tree, produced by a pure function in `screen.rs` so it is
unit tested and the same text serves `--plain`, S5 and the interface.

```text
 ~/Downloads                                          3 folders · 2 hidden
 ├─ films/                                                       4.2 GB
 │  ├─ old.mkv                                         1.9 GB  ⚠ asks
 │  └─ new.mkv                                         2.3 GB
 ├─ iso/
 │  └─ disk.img                                        0.9 GB  ROOT
 ├─ notes.txt                                            5 KB
 ├─ .cache/                                      (hidden, asks to open)
 └─ … 12 more (use a path or --depth to open)
```

Rules:

- Folders first, then names (case-insensitive, then bytes), sizes right-aligned.
- Default depth 2 and at most 200 lines; beyond that, a single "… N more" line
  says how to ask for more. A bound is never silent.
- Marks: hidden (shown only when asked, always `asks`, ADR 0016), `unreadable`,
  `protected`, `system`, `link ->` (never followed in the tree), and `ROOT` where
  a plan would hold an action for root.
- In the **ambiguity choice**, the candidates appear in their parent context so
  the person tells `~/Documents/report.pdf` from `~/Downloads/report.pdf` by
  where they sit; only candidates are selectable.
- In the **ratatui interface** the tree is a history block like any other
  (scrollable), and the choice is selected with the arrows over the leaves. In
  **plain mode** leaves are numbered, as the choice is today.
- Default views show locations relative to the current directory or `~`; the
  exact absolute location stays in the details view and in the root panel.

### 4. The plan format (schema version 3)

- `version` becomes 3. The reference grammar gains `{"path": "<string>"}` and
  `{"under_home": true, "name": "..."}` replaces `in:"everywhere"`; `place` and
  `name`+`in` remain, where `in` may now also be a `path` reference.
- `session.cd` is added to the action table.
- A version 2 document is refused as version 1 was. The examples under
  `docs/schemas/examples/` and `validate.py` follow; each example stays a test.
- The grammar bounds path length and rejects NUL; `..` handling is a rule in
  code, as for names, not a `pattern`.

### 5. Migration

- Kept as they are: `Resolved`, the resolver's interface, the broker, the
  review, the screen's plan table and root panel, the interface, the decision
  log. Tests keep passing against the new resolver variants.
- New: the `path` resolver, the session directory, `session.cd`, the tree
  renderer, the prompt's current-directory line, S5's `cd`/`ls`/`tree`.
- Removed: the claim in `docs/s3b-path-free-shell.md` that no location is
  shown by default; that report is updated.

## Alternatives considered at the top level

| Option | For | Against |
|---|---|---|
| Keep ADR 0014 and only add the tree | No schema change | The ambiguity the user hit stays; the tree would show locations the model still cannot name |
| **Add paths and a current directory (this ADR)** | Matches how people and the model already think of files; ambiguity is resolved by where things sit | A schema version, session state, a bigger prompt |
| Go back to ADR 0011's paths only | Smallest design | Throws away the resolver and the closed vocabulary that S3b built and tested |

## Consequences

- A schema version and a prompt change: the model's habits need re-measuring on
  the same intents (S4).
- The core has state (the current directory); every confirmed plan carries the
  directory and resolved locations it was reviewed under.
- ADR 0014's "the person never sees paths" is gone, but ADR 0007's rule that a
  root confirmation shows the exact real location now holds on every screen
  that shows a location.

## Decisions taken by the user (2026-11-16)

The user chose the recommended option in every question:

1. Naming: paths as one more reference next to `place`, `name`+`in` and
   `from_step` (option B); `everywhere` becomes `under_home`; schema version 3.
2. Navigation: the current directory is session state in the core `Shell`
   (option A); screens show locations relative to it or to `~`, and the exact
   absolute location stays in the details view and the root panel.
3. System space: `/etc` and the rest may be named and appear in the tree marked
   `system`, with the broker as the only gate. Tree defaults: depth 2, 200 lines,
   hidden entries only when asked, folders first.

Implementation is a roadmap item of its own (a new session, code after this ADR).
