---
title: "ADR 0021: The voice of the answers, written by the model from the SOUL"
type: adr
status: proposed
created: 2026-11-16
updated: 2026-11-16
tags: [shell, soul, persona, model, ux]
---

# ADR 0021: The voice of the answers, written by the model from the SOUL

## Context

ADR 0006 gives the system a SOUL and a voice, and says they govern
presentation, never authority. Today the intent shell has no voice: it prints a
fixed table and fixed result lines, plus one line, "The model says: ...", that
repeats the plan summary. After using the shell the user found the output
"simple and without personality" and asked for answers with character.

The commands to define the SOUL (interview, change, history, undo) are S9 and
S10 and wait. This ADR only decides how an answer gets its voice, using the
SOUL as it exists in S8 (a preset). ADR 0002 stands: the system works with no
model, and the model never decides.

## Decision (proposed)

### 1. A reply, written by the model after the facts are known

After a request ends (executed, declined, refused, no plan, a clarification
answered), the shell may ask the model for a short **reply** in the voice of the
SOUL. The model receives the validated outcome, not the other way around:

```text
request ─► plan ─► broker ─► (confirm) ─► execute ─► OUTCOME ─► reply (model, SOUL) ─► screen
```

- The reply is **free text** with a hard length cap (about 400 characters) and
  a timeout. It is shown under the deterministic result, never instead of it.
- The table, the verdicts, the notes, the tree, the root panel and every
  confirmation stay deterministic text. The reply cannot change, hide or
  replace them, and it is never parsed for anything.
- No model, a model error or a timeout shows no reply: the output is exactly
  today's. A missing voice is not an error.
- The reply is untrusted text like any model text: it passes through the
  same escaping (`printable`) and is shown on one clearly marked block.
- The call has no tools and no plan grammar. It cannot start an action.

### 2. What the model is given

The outcome in a compact, fixed form: the request, the verdicts, what ran
and what failed, counts, and short names of results (the friendly names the
screen already shows). Not file contents, not real locations, not the full
log. The prompt is a fixed template with the SOUL's fields filled in by code.

### 3. What a SOUL is, for now (S8, minimal)

A preset chosen from the four of ADR 0006 (direct and technical, warm and
didactic, formal and discreet, good-humoured), plus the machine name. The code
turns the preset into a few fixed prompt lines. **No free-text SOUL**: ADR 0006
rejects it because free text is the easiest way to smuggle in instructions. The
preset is read from a file in the user's config directory; absent, the shell
uses "direct and technical". Per-user personality and the commands to change
the SOUL are S8's remainder, S9 and S10.

### 4. What it may never do (tested)

- The same request yields the same validated actions, the same verdicts and
  the same log events under every preset and with the reply on or off. The
  reply call happens after execution, and a test runs the pipeline under all
  four presets and compares them.
- A reply never reaches `Confirm`, the broker or the log's decision events; it
  is logged as `reply` with the text, after the outcome.
- Text in the outcome that came from files (names) is the only way the world
  reaches the reply prompt; it is quoted as data, and the reply is capped and
  escaped, so a hostile file name cannot add to the screen anything but a line
  of marked text.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| **A. Free-text reply written by the model after the outcome (recommended)** | Real personality; cannot affect actions because it runs after them; degrades to today's output | A second model call per request (seconds at about 100 tok/s); the model sees names from the outcome |
| B. Only rewrite the "The model says" line | Cheapest, least risk | Little personality; the line is the plan summary, before anything ran |
| C. Fixed phrases per preset | Works with no model, predictable | Repetitive; no personality about what actually happened |
| D. Let the planning call also write the reply | One call | The model would write the reply before the result exists, and mixes presentation into the plan grammar the broker validates |

## Consequences

- A latency decision for the interface: the reply arrives after the result, so
  the interface shows the result at once and the reply as it comes ("saying…").
  Plain mode prints it after the result.
- A reply setting (on, off) and a short length option belong to the shell
  options, and the live test covers one reply against the real model.
- S8's remainder (per-user personality), S9 and S10 build on the same prompt
  template and need no change to this decision.

## Questions for the user

1. Option A (a reply after every outcome)? Or only after executed plans?
2. Default preset when no SOUL file exists: direct and technical?
3. Should the reply be on by default, with `--no-reply` to turn it off?
