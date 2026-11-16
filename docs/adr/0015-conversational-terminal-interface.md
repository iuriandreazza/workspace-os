---
title: "ADR 0015: Conversational terminal interface of the intent shell"
type: adr
status: accepted
created: 2026-11-16
updated: 2026-11-16
tags: [shell, tui, ratatui, ux, recovery]
---

# ADR 0015: Conversational terminal interface of the intent shell

## Context

ADR 0014 defined what the person sees (header, table with verdicts, notes,
summary, root panel, details, numbered choice) and left the terminal as a
line-by-line adapter (`Confirm` in `ports.rs`, printed by `main.rs`). The user
wants a chat-style session instead, like Claude Code in a terminal: an input
line at the bottom, a visible wait while the model generates, the plan
appearing as a block in the history, confirmations and ambiguous-reference
choices made by selection, and a scrollable history.

Constraints that stay: ADR 0002 (the system works with no model), ADR 0007
(a root confirmation shows the exact action; recovery mode runs the
deterministic shell only), ADR 0005 (dependencies GPL-2.0 compatible), and
roadmap S3c (only the terminal adapter changes; `screen.rs`, the resolver and
the broker stay as they are).

## Decision

### 1. Library: `ratatui` with `crossterm`

| Option | For | Against |
|---|---|---|
| **A. `ratatui` + `crossterm` (recommended)** | The de facto Rust TUI library; immediate-mode drawing from plain data fits `screen.rs` (text in, widgets out); `TestBackend` renders to a buffer for layout tests; crossterm is the portable backend (macOS, Linux). Both are MIT | Several transitive dependencies (to be listed and checked before they are added); crossterm is a terminal-only backend, so the OS needs its own adapter later |
| B. `crossterm` only, hand-drawn | Fewest dependencies | We would rebuild layout, wrapping and scrolling; more of our own code in the trusted path |
| C. `cursive` | Widgets and dialogs ready made | Retained-mode, callback-driven: it fights the blocking `Confirm` port and has no buffer backend as direct as `TestBackend`. Licence is MIT, no advantage |
| D. Line editor only (`rustyline`) | Tiny; keeps scrollback natively | No progress block, no selection with arrows, no live layout: not the requested interface |

Licences checked on crates.io on 2026-11-16: `ratatui` 0.30.2 is MIT and
`crossterm` 0.29.0 is MIT, both compatible with GPL-2.0-only (ADR 0005). Every
transitive crate is listed with its licence in the implementation PR, taken from
`cargo metadata`, and the PR is not opened with one that is not compatible.

### 2. What stays plain text

The terminal interface is an adapter around the existing core, never a
dependency of it.

- `screen.rs`, `Outcome::render`, the resolver and the broker still produce
  and take plain text and data. The interface only lays that text out.
- The current line adapter stays, as the **plain mode**. It is used with
  `--plain`, and automatically when stdin or stdout is not a terminal, so
  scripted transcripts and the live test keep working.
- `ratatui` and `crossterm` are an optional Cargo feature `tui` of the
  `intent-shell` crate, on by default. The deterministic shell (S5) and the
  recovery mode (ADR 0007) are built with `--no-default-features` and link no
  TUI code. The gate builds and tests both configurations.
- Nothing in the core imports `ratatui` or `crossterm`.

### 3. Mapping of the ADR 0014 screens

```text
┌ intent shell ─ user iuri ─ model 127.0.0.1:8089 ─ log decisions.jsonl ┐  header (screen::header)
│ you ▸ delete the biggest file in downloads                        │
│ ⠹ asking the model... 3s                                          │  history, scrolls
│ plan ▸ (screen::plan_screen as one block)                         │  PgUp/PgDn, Home/End
│  s1  Find the largest file in Downloads   fs.find_largest   ok    │
│  s2  Delete big.iso                       fs.delete      ROOT held│
│ ╔ ROOT ═════════════════════════════════════════════════════════╗ │  separate modal panel
│ ║ exact: fs.delete /Users/.../big.iso   [y] confirm  [n] refuse ║ │  (screen::root_panel)
│ ╚═══════════════════════════════════════════════════════════════╝ │
├────────────────────────────────────────────────────────────────────┤
│ ▸ type a request                                    Ctrl-D quit   │  fixed input line
└────────────────────────────────────────────────────────────────────┘
```

| ADR 0014 element | In the interface |
|---|---|
| Input line (`intent>`) | Fixed at the bottom; Enter sends, Ctrl-D quits, empty line ignored |
| Waiting for the model | A spinner and elapsed seconds in the history while `Planner::plan` runs (the call can take minutes) |
| Plan screen | One history block with the text of `screen::plan_screen(Friendly)`, added when the plan reaches review, then execution results appended as they finish |
| Confirm the plan | A bar under the block: `y` run the steps marked ok, `n` cancel, `d` toggle the details view (same text as today, real locations) |
| Ambiguous reference | A list with the question from `screen::question`; Up/Down select, Enter chooses, Esc cancels |
| Root action | A separate modal panel with the exact action, always with the real location; only `y` confirms, Esc and `n` refuse; no key repeats, no "always" |
| Result and notes | Appended to the history as text from `Outcome::render` |
| History | Scrollable with PageUp/PageDown and the mouse wheel; the full history is printed to the normal terminal when the program exits, so it stays in the scrollback |

Not in this item: streaming the model's tokens into the plan. The plan arrives
whole from the model, so the "growing" block grows by stage (request, wait,
plan, verdicts, results), not by token. Streaming would change `Planner` and is
a later decision.

How it runs: the terminal adapter stays synchronous. The interface owns the
terminal on the main thread; the shell (`Shell::handle`) runs on a worker
thread that builds its own host, planner and log. The worker's `Confirm` and
a `Planner` wrapper send events to the interface and block for the answer. The
core needs no async and no change to its traits.

Keys read in a terminal are text: input is escaped by the same `printable`
rules as everything shown, so a pasted escape sequence cannot move the cursor
or forge a panel (the invisible-character escapes of S3b apply to the history
as well).

### 4. Tests

- **Content**: unchanged. `screen.rs` and `Outcome::render` are tested as text.
- **Layout**: `ratatui`'s `TestBackend` renders the interface to a buffer, and
  tests compare it as text: header, history, input line at the bottom, scrolling
  on a short screen, a narrow screen, the root panel showing the real path, the
  selection highlight position.
- **Behaviour**: the key handling is a pure function `(State, Key) -> State +
  Answer` and is tested without a terminal: only `y` confirms, Esc refuses a root
  action, a selection never leaves the list, a non-terminal falls back to plain.
- **Gate**: `scripts/check.sh` clippy and tests for the crate with and without
  the `tui` feature.

### 5. Transcript for the pull request

A real session is recorded in `tmux` (a detached session with a fixed size,
driven with `send-keys`, read with `capture-pane -p`) against the real model,
for the same intents as S3b: a plan with a confirmation, an ambiguous choice
and a root confirmation. The pane text is what the pull request shows. As a
second record, `--transcript FILE` writes the history in plain text, the same
text that is printed on exit.

## Alternatives considered for the viewport

| Option | For | Against |
|---|---|---|
| **A. Alternate screen, own scrolling (recommended)** | Full control of layout, scrolling and mouse; the fixed input line is trivial | The history leaves the scrollback on exit, solved by printing it on exit |
| B. Inline viewport (history goes to the terminal's scrollback, only the input and live block are redrawn) | Native scrollback and copy, closest to Claude Code | Resizing and the selection/modal panels are fragile; the scrollable history is the terminal's, not ours, so it cannot be tested with `TestBackend` |

## Consequences

- `intent-shell` gains two direct dependencies under a feature, and their
  transitive crates. Their licences are listed in the PR.
- `main.rs` splits into the plain adapter and the interface adapter; the
  options (`--model`, `--log`, `--administrator`) are shared.
- A second consumer of `Confirm` shows the trait is the right seam: if the
  interface needs a method the plain adapter cannot give, that is a finding.
- Closing the terminal mid-confirmation counts as a refusal (as end of input
  does today), and the terminal is always restored, including on panic.

## Decisions taken by the user (2026-11-16)

The user approved the design above: `ratatui` with `crossterm`, the alternate
screen with its own scrolling, and the line adapter kept as plain mode.

Settled while implementing:

- `ratatui` re-exports `crossterm`, so the crate has one direct dependency
  (`ratatui`) and uses the `crossterm` version it was built against, with no
  version skew. The licences are the same two.
- Plain mode is also the default when `TERM` is unset or `dumb`.
