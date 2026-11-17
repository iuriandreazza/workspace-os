---
title: "S3c: conversational terminal interface"
type: report
status: done
created: 2026-11-16
updated: 2026-11-16
tags: [shell, tui, ratatui, ux]
---

# S3c: conversational terminal interface

> Since S3e (`docs/s3e-directories-and-tree.md`, ADR 0018) the transcripts below
> that mention `everywhere` or a location shown as `(Downloads)` are historical:
> `everywhere` is `under_home` and locations are written relative to the
> current directory or to `~`, with the tree view for listings and choices.

Roadmap item S3c, decided in ADR 0015. The intent shell opens a chat-style
screen on a terminal: the header on top, the history in the middle, the input
line fixed at the bottom. Only the terminal adapter changed: `screen.rs`, the
resolver, the broker and `Shell` are as in S3b.

## Run it

```bash
./scripts/intent-shell.sh                      # the interface, on a terminal
./scripts/intent-shell.sh --plain              # the line-by-line adapter
./scripts/intent-shell.sh --transcript out.txt # also write the history as text
```

`--plain` is also what runs when stdin or stdout is not a terminal, or `TERM`
is unset or `dumb`, so piped transcripts and the live test work as before.

| Key | Where | Does |
|---|---|---|
| Enter | input | send the request |
| PgUp / PgDn, mouse wheel, Ctrl-Home / Ctrl-End | anywhere | scroll the history |
| y / d / n, Esc | plan | run the steps marked ok / show the details / cancel |
| Up / Down, Enter, Esc | ambiguous reference | select, choose, cancel |
| y / n, Esc | root panel | confirm this exact action / refuse. Only `y` confirms |
| Ctrl-D (idle), Ctrl-C | | quit; the history is printed to the terminal |

## What asks, and what does not (ADR 0016)

A plan runs without the plan question when every action that could run only
reads (`fs.list`, `fs.stat`, `fs.read`, `fs.find_largest`) or adds
(`fs.write` with mode `create` or `append`). The result then shows the plan table
and a line saying it ran without asking, and the log records `plan_auto_approved`
before anything runs. Overwrite, move, delete, `process.run`, user, system and
SOUL changes, and anything held for root still ask, and one such action makes the
whole plan ask. The rule is `Action::needs_confirmation`, a `match` without a
wildcard, so a new action cannot be added without classifying it.

## How it is built

```text
main thread: terminal, state, drawing        worker thread: Shell::handle
  key ─► State::on_key ─► Effect               Planner wrapper ─► Event::Busy
  Event ─► State::on_event                     ChannelConfirm  ─► Event::Ask*, waits for Reply
```

- `tui/state.rs`: the interface as data. Keys and events in, an effect out,
  no terminal: every rule (only `y` confirms, a selection stays in its list,
  a root action is never confirmed unless it is shown whole) is a unit test.
- `tui/view.rs`: draws the state with `ratatui`; tested against `TestBackend`.
- `tui/wrap.rs`: hard wrapping by display width. Nothing is cut off, because the
  root panel must show the whole exact action (ADR 0007). When the terminal is
  too small for it, the panel says so, and only `n` works.
- Keys typed before a question is drawn are discarded, so a keystroke meant for
  something else cannot answer it. Characters that `printable` would escape
  cannot be typed at all.
- Answer keys are ignored for 0.7 s after a question is drawn (2 s for a root action), because a terminal
  without the kitty keyboard protocol reports a held or double-tapped \`y\` as
  fresh presses.
- Pasted text only stays out of the answers if the terminal honours bracketed
  paste (most do; a multiplexer that does not delivers a paste as key presses,
  and the quiet gap above is then the only guard).
- An answer needs a plain key press: Ctrl-, Alt- or repeated keys never answer, and
  a paste (bracketed) only fills the request line. Quitting at a question is a
  refusal and is logged; Ctrl-C is refused while a step runs. SIGTERM and SIGHUP
  restore the terminal. \`--transcript\` is written with mode 0600.
- The plan arrives whole from the model, so the block grows by stage (request,
  wait, plan, results), not by token.

## Dependencies and licences

One direct dependency, `ratatui` 0.30 (MIT) with default features off and the
`crossterm` backend (MIT, re-exported), optional behind the `tui` feature. The
deterministic shell (S5) and the recovery mode build with `--no-default-features`
and link none of it; `scripts/check.sh` builds and tests both ways. It adds 66
crates to the interface build, checked with `cargo metadata`:

| Licence expression | Crates |
|---|---|
| MIT, or dual with Apache-2.0 / Unlicense / BSL-1.0 / LLVM exception (each usable under MIT or another GPL-2.0-compatible term) | 64 |
| Zlib (`foldhash`) | 1 |
| (MIT OR Apache-2.0) AND Unicode-3.0 (`unicode-ident`, compile-time only) | 1 |
| GPL-2.0-only (this crate) | 1 |

Zlib and the Unicode licence are permissive and GPL-2.0 compatible. No built
crate is Apache-2.0 only or GPL-3.0 only.

## A real session

Recorded in `tmux` (100 by 30) against Bonsai 8B, with a `HOME` holding a
`Downloads` and a `Documents` folder that both have a `report.pdf`. Screens are
shown as printed; the blank lines at the bottom of each are trimmed.

```text
### 1. waiting for the model
 intent shell ──────────────────────────────────────────────
  user  "iuriandreazza" (administrator)   model  "127.0.0.1:8089"   log  "decisions.jsonl"
 ───────────────────────────────────────────────────────────────
you ▸ find the largest file in downloads























────────────────────────────────────────────────────────────────────────────────────────────────────
⠇ asking the model… 0s
PgUp/Dn scroll · ^C quit

### 2. the plan, asking to confirm
 intent shell ──────────────────────────────────────────────
  user  "iuriandreazza" (administrator)   model  "127.0.0.1:8089"   log  "decisions.jsonl"
 ───────────────────────────────────────────────────────────────
you ▸ find the largest file in downloads


  Step  What                                Needs    Verdict
  s1    Find the largest file in Downloads  fs.read  ok

  1 steps, 1 actions: 1 ok, 0 held for root, 0 refused.  The model says: "Finding the largest file
in downloads"

















────────────────────────────────────────────────────────────────────────────────────────────────────
[y] run the steps marked ok   [d] details   [n] cancel

### 3. ambiguous reference, selection
 intent shell ──────────────────────────────────────────────
  user  "iuriandreazza" (administrator)   model  "127.0.0.1:8089"   log  "decisions.jsonl"
 ───────────────────────────────────────────────────────────────
  1 steps, 1 actions: 1 ok, 0 held for root, 0 refused.  The model says: "Finding the largest file
in downloads"



  Step  What                                Needs    Verdict
  s1    Find the largest file in Downloads  fs.read  ok
          ↳ exact: fs.find_largest
path="/private/tmp/claude-501/-Users-iuriandreazza-desenvolvimento-workspace-os--claude-worktrees-ni
fty-mclaren-76f132/452c4a9d-ab12-48f8-baa0-d30f0858aa91/scratchpad/home2/Downloads" count=1

  1 steps, 1 actions: 1 ok, 0 held for root, 0 refused.  The model says: "Finding the largest file
in downloads"


▸ confirmed

s1 Find the largest file in Downloads -> done
    "big.iso" (Downloads)  3000000 bytes

you ▸ show details of report.pdf, search everywhere

Step s1: file "report.pdf" in everywhere matches more than one thing. Which one?
────────────────────────────────────────────────────────────────────────────────────────────────────
▸ 1. "report.pdf" (Documents)
  2. "report.pdf" (Downloads)
↑/↓ select · Enter choose · Esc cancel

### 4. second option selected
 intent shell ──────────────────────────────────────────────
  user  "iuriandreazza" (administrator)   model  "127.0.0.1:8089"   log  "decisions.jsonl"
 ───────────────────────────────────────────────────────────────
  1 steps, 1 actions: 1 ok, 0 held for root, 0 refused.  The model says: "Finding the largest file
in downloads"



  Step  What                                Needs    Verdict
  s1    Find the largest file in Downloads  fs.read  ok
          ↳ exact: fs.find_largest
path="/private/tmp/claude-501/-Users-iuriandreazza-desenvolvimento-workspace-os--claude-worktrees-ni
fty-mclaren-76f132/452c4a9d-ab12-48f8-baa0-d30f0858aa91/scratchpad/home2/Downloads" count=1

  1 steps, 1 actions: 1 ok, 0 held for root, 0 refused.  The model says: "Finding the largest file
in downloads"


▸ confirmed

s1 Find the largest file in Downloads -> done
    "big.iso" (Downloads)  3000000 bytes

you ▸ show details of report.pdf, search everywhere

Step s1: file "report.pdf" in everywhere matches more than one thing. Which one?
────────────────────────────────────────────────────────────────────────────────────────────────────
  1. "report.pdf" (Documents)
▸ 2. "report.pdf" (Downloads)
↑/↓ select · Enter choose · Esc cancel

### 5. root confirmation panel
 intent shell ──────────────────────────────────────────────
  user  "iuriandreazza" (administrator)   model  "127.0.0.1:8089"   log  "decisions.jsonl"
 ───────────────────────────────────────────────────────────────
▸ chose "report.pdf" (Downloads)


  Step  What                                                           Needs    Verdict
  s1    Show details of "report.pdf" (Downloads)                       fs.read  ok
  s2    Find the 1 largest files in folder "everywhere" in everywhere           REFUSED
          ↳ nothing found for folder "everywhere" in everywhere

  2 steps, 2 actions: 1 ok, 0 held for root, 1 refused.  The model says: "Show details of
report.pdf and search everywhere"


▸ cancelled

Not confirmed. Nothing ran.

you ▸ delete big.iso in downloads, as administrator


  Step  What                          Needs      Verdict
  s1    Delete "big.iso" (Downloads)  fs.delete  ROOT, held

  1 steps, 1 actions: 0 ok, 1 held for root, 0 refused.  The model says: "Delete big.iso from
downloads as administrator"

────────────────────────────────────────────────────────────────────────────────────────────────────
[y] run the steps marked ok   [d] details   [n] cancel

### 6. root panel after confirming the plan
 intent shell ──────────────────────────────────────────────
  user  "iuriandreazza" (administrator)   model  "127.0.0.1:8089"   log  "decisions.jsonl"
 ───────────────────────────────────────────────────────────────
report.pdf and search everywhere"


▸ cancelled

Not confirmed. Nothing ran.

you ▸ delete big.iso in downloads, as administrator


  Step  What                          Needs      Verdict
  s1    Delete "big.iso" (Downloads)  fs.delete  ROOT, held

  1 steps, 1 actions: 0 ok, 1 held for root, 0 refused.  The model says: "Delete big.iso from
downloads as administrator"


┌ ROOT ACTION ─────────────────────────────────────────────────────────────────────────────────────┐
│╔═ ROOT CONFIRMATION ═ asked of "iuriandreazza" ═ step s1                                         │
│║ Action  Delete "big.iso" (Downloads)                                                            │
│║ Exact   fs.delete                                                                               │
│path="/private/tmp/claude-501/-Users-iuriandreazza-desenvolvimento-workspace-os--claude-worktrees-│
│nifty-mclaren-76f132/452c4a9d-ab12-48f8-baa0-d30f0858aa91/scratchpad/home2/Downloads/big.iso"     │
│║ Needs   fs.delete, as root. Confirms this one action only.                                      │
│╚═                                                                                                │
│[y] confirm this exact action   [n] refuse                                                        │
└──────────────────────────────────────────────────────────────────────────────────────────────────┘

### 7. after the root confirmation
 intent shell ──────────────────────────────────────────────
  user  "iuriandreazza" (administrator)   model  "127.0.0.1:8089"   log  "decisions.jsonl"
 ───────────────────────────────────────────────────────────────
you ▸ delete big.iso in downloads, as administrator


  Step  What                          Needs      Verdict
  s1    Delete "big.iso" (Downloads)  fs.delete  ROOT, held

  1 steps, 1 actions: 0 ok, 1 held for root, 0 refused.  The model says: "Delete big.iso from
downloads as administrator"


▸ confirmed

╔═ ROOT CONFIRMATION ═ asked of "iuriandreazza" ═ step s1
║ Action  Delete "big.iso" (Downloads)
║ Exact   fs.delete
path="/private/tmp/claude-501/-Users-iuriandreazza-desenvolvimento-workspace-os--claude-worktrees-ni
fty-mclaren-76f132/452c4a9d-ab12-48f8-baa0-d30f0858aa91/scratchpad/home2/Downloads/big.iso"
║ Needs   fs.delete, as root. Confirms this one action only.
╚═

▸ root action confirmed

s1 Delete "big.iso" (Downloads) -> FAILED
    fs.delete is not executable in this prototype
────────────────────────────────────────────────────────────────────────────────────────────────────
▸
Enter send · PgUp/Dn scroll · ^D quit

```

`fs.delete` is not executable in the prototype (as in S3), so the last request
ends in a failed step after the root confirmation; the confirmation itself is
what this report shows.
