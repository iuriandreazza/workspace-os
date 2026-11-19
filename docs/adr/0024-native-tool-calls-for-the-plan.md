---
title: "ADR 0024: Native tool calls to produce the plan, for models that do better with them"
type: adr
status: proposed
created: 2026-11-19
updated: 2026-11-19
tags: [slm, shell, plan, tool-calling, llama-cpp, evaluation]
---

# ADR 0024: Native tool calls to produce the plan, for models that do better with them

## Status

**Proposed, for the project owner to decide.** If accepted, it supersedes only
the "Constraining the output" part of ADR 0011 (how the model is made to emit
the plan), and the status line of ADR 0011 gains "and ADR 0024". Everything else
in ADR 0011 stands: the schema, the semantic rules, the broker's order of checks,
root and confirmation, the SOUL action, and what the deterministic shell reaches.
ADR 0002 stands: the model proposes, deterministic code decides. Nothing in this
ADR changes what the broker allows; it changes which text the planner asks the
model for.

## Context

ADR 0011 chose to constrain the model's output with the plan's JSON Schema, which
`llama-server` turns into a grammar, and kept native tool calls "as a comparison
for S4". S4 made the comparison (`docs/s4-model-evaluation.md`, PR 27: 89 intents
on plan schema version 3, one commit, temperature 0). What it found, as exact
plans out of 89 (a plan equal to an accepted plan; nothing was executed):

| Model | Constrained plan (shipped) | Native tool calls | Tool calls, dependencies derived by the harness |
|---|---:|---:|---:|
| Bonsai 8B (in use) | 19 | 1 (88 answers invalid) | 35 |
| Ternary Bonsai 2 9B, thinking on / off | 42 / 40 | 52 | 53 |
| Bonsai 27B, first generation (ceiling) | 28 | 58 | 53 |
| Gemma 4 12B (ceiling) | 56 | 47 | 52 |

- **The mode matters more than the model for the Bonsai family**: the same
  weights gain 10 to 16 exact plans (8B, 9B) and 30 (27B), and the overreach
  falls for the 8B and the 27B. Gemma 4 12B goes the other way: in tools mode its
  answers for the plans that search and then act stop after the search.
- **The first protocol failed for a reason that is not about tools.** It asked the
  model to number its calls and write `depends_on`. Bonsai 8B wrote
  `depends_on: ["s1"]` on its first call in 81 of 89 answers. With the dependencies
  worked out by the harness from the `from_step` references, the same model gets
  35. That variant was added after the first one failed; it was not pre-registered.
- **The cost is the prompt**: 5,200 to 6,000 tokens for the Bonsai models (1,600
  to 2,200 for Gemma) against 700 constrained, because every tool carries the
  schema's reference definitions inlined. `cache_prompt` reuses the prefix, so the
  cost was small there; the first request after a start is not cached. At the
  100 tokens per second of prompt processing that S1 measured on the 8 GB M1, 6,000
  tokens would be about a minute. That is an estimate, not a measurement.
- **What was not measured:** a loop in which the results of a call go back to the
  model (the harness takes one reply as the whole plan); any model on the M1; a
  retry in tools mode (the harness does not retry there); a smaller tool
  definition.
- **Each way has its own failure.** The constrained mode: a refused plan (the model
  appends a `session.cd` to a plan: 7 of the 8B's 12 invalid answers). The tools
  mode: a call the server cannot parse (Gemma 4 wrote `{"path":{"path":{...` until
  the server stopped at 64 levels and answered HTTP 400, 3 answers in 178).

## Decision

If the owner accepts, the shell gets a second way to obtain the plan from a model
and a setting that says which way each model uses. The constrained plan stays,
as the default and as the way for a model that does better with it.

### 1. One plan, whichever way it was obtained

The planner returns the same document text it returns today (`Planner::plan`,
`intent-shell/src/ports.rs`). In tools mode it builds that document from the
model's calls; from `parse_checked` onwards nothing changes: schema, semantic
rules, resolver, broker, confirmation, log. There is still one path to execution
(ADR 0011, ADR 0002). The log keeps what the model really sent (the calls) as well
as the document built from them: every document the model produced is logged,
valid or not.

### 2. The tools come from the schema

One function per action, generated from the schema file's own step definitions
with the `$ref`s inlined, plus `clarify`. The schema stays the one artifact that
the model, the shell and `docs/schemas/validate.py` read. A new action in the
schema is a new tool the same day.

### 3. The model does not write `depends_on`; code derives it

A call's dependencies are the steps its arguments refer to with `from_step`. The
tools have no `depends_on`. The model numbers its calls `s1`, `s2`, ... in the
order it makes them and refers to an earlier call as `{"from_step":"s1"}`.
Consequence: in tools mode a dependency that is only about order, with no data
passed, cannot be written. Today that is no loss, because the broker resolves
every step against the filesystem as it is before any step runs, so a step that
needs another to have run first cannot work anyway. If a later ADR needs ordering
without data, it adds an argument.

### 4. Nothing is repaired

The conversion repairs nothing and guesses nothing: a missing `elevate` where the
action has one, an unknown tool, a `clarify` mixed with other calls, arguments
that are not an object, or an argument the schema does not have, make the answer
invalid, and the existing path applies (one retry with the reason, then the shell
says it could not make a plan and points to the deterministic shell; ADR 0011).
Whether a reply with no call at all (prose) is invalid or becomes a question is
open (below). A call the server itself cannot parse (HTTP 400 for a call nested
without end) is an invalid answer from the model, not a failing server.

### 5. The setting is per model

The shell records, for each model it knows, which way to ask. A model with no
entry is asked the way it is today. The same setting holds what else the request
needs for that model (for the Ternary Bonsai 2 family, that thinking is off:
`chat_template_kwargs {"enable_thinking": false}`; whether to adopt that model is
the decision of S4's report, not of this ADR).

### 6. The prompt shares its rules

The reference rules of the system prompt (how to name a thing, the current
directory, `session.cd` alone) are written once and used by both ways. Tools mode
replaces the instructions about the JSON document with a short preamble. The tool
definitions are made smaller than S4's 5,200 to 6,000 tokens before this is
implemented: what has to be inlined, what can be shortened (descriptions, the many
alternatives of a reference), measured against the evaluation set.

### 7. The evaluation set decides

`eval/run_eval.py --mode tools` is the reference for the conversion of calls to a
document (`eval/modes.py`, `document_from_calls`). The Rust conversion is held to
it with shared fixtures: tool-call messages and the documents they must give, as
data under `docs/schemas/examples/`, tested by both. A model uses tools mode only
if, on the S4 set with the final prompt and tools, it beats its constrained mode
by at least 7 exact plans with no more overreach (the rule S4 used), and the first
request is acceptable on the 8 GB M1.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| **A. Keep the constrained plan for every model (ADR 0011 as it is)** | Nothing to build. A valid shape every time. A small prompt (700 tokens). The retry is already written | Leaves 10 to 16 exact plans on the table for the Bonsai models. Its commonest refusal is a rule the model breaks (`session.cd` with other steps) |
| **B. A second way, set per model (this ADR)** | Uses each model where it does better (Bonsai: tools; Gemma: constrained). Everything below the planner stays one path. The schema stays the one artifact | Two ways to maintain and test. A bigger prompt in tools mode, and a slow first request on the M1 until the tools are made smaller. The derived dependencies cannot say "only in this order" |
| C. Tool calls for every model | One way | Gemma 4 12B loses 4 to 9 exact plans in tools mode; a model with no tool template cannot be used |
| D. Tool calls with the results fed back to the model (a loop) | Natural for a plan that searches and then acts; Gemma's problem may go away | Not measured. The model would see file names and sizes, which is data it can be misled by (ADR 0011: text read by the model is data); the plan could no longer be shown whole before anything runs. A different ADR, probably with S7 |
| E. A hand-written grammar for the plan | More control than the schema | A second artifact that drifts from the schema (ADR 0011 already rejected it) |

**Recommendation: B**, gated by the conditions in 7 and by the open questions
below. A is the fallback if the tools cannot be made small enough for the M1.

## Consequences

- The Rust shell grows a conversion of calls to a document and a per-model
  setting; the schema, the resolver, the broker, the confirmation and the screens
  do not change.
- `docs/schemas/examples/` gains fixtures of tool calls and the documents they
  give; the reference validator and the Rust test both read them.
- The log records the calls as well as the document; its shape for decisions does
  not change.
- A new action is a schema change and nothing else, as before: the tools follow it.
- The deterministic shell (ADR 0022) is not touched: it types the same actions
  and has no model.

## Open questions

- **A reply with no tool call.** Invalid and retried, or taken as a question
  (`clarify`, at most 300 characters)? S4 scored it invalid; it happened 0 to 7
  times in 89 answers per run, most for Bonsai 8B. Taking prose as a question
  gives the model one more way to put text in front of the person; the text would
  be shown as the model's, as `clarify` is.
- **How small the tools can be.** S4 did not try. The measure is in 6 and 7.
- **The M1.** Memory, the first request, the `llama.cpp` build (Metal), and
  whether the server builds the tool grammar the same way there.
- **A loop.** Whether a plan that searches and then acts is better made by feeding
  the search back (option D). Out of this ADR.
- **A model that stops after the search.** Gemma 4 12B in tools mode plans the
  search and stops; the per-model setting is what handles it for now.
