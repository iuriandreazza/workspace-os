---
title: "S4: evaluation set and model comparison"
type: report
status: in progress
created: 2026-11-16
updated: 2026-11-16
tags: [slm, bonsai, evaluation, tool-calling, llama-cpp]
---

# S4: evaluation set and model comparison

Roadmap item S4. This file is written in two steps. The set, the scoring and
the decision rule below were fixed and committed **before any model was run**,
so the rule cannot be bent to fit the numbers. The results and the decision
are added after them.

## The set

`eval/intents.json` holds 68 intents: 58 in English and 10 in Brazilian
Portuguese (the owner's language; ADR 0006 gives each user a language). Eight
are the intents of the S3 and S3b reports, which ADR 0014 says S4 reuses. Every
action of ADR 0011 appears at least twice, and every form of reference of
ADR 0014 does: a place, a name with a `kind`, a `new_name`, a folder as a
destination and `from_step`.

| Category | n | What it asks of the model |
|---|---:|---|
| `list` | 6 | Read a place, or a folder by name |
| `largest` | 8 | Choose the place and the count, from digits or words, with and without a place |
| `stat-read` | 6 | Name a file, and say whether it is a file, a folder or either |
| `write` | 6 | `new_name` against `name`, and the mode: create, overwrite, append |
| `move` | 5 | A place, a folder or a new name as the destination, and a chain through `from_step` |
| `delete` | 7 | Delete by name, through a chain, and refuse to delete a whole place |
| `chain` | 3 | Steps that depend on each other, and steps that do not |
| `elevate` | 5 | Ask for root only when the person asks for it |
| `process` | 3 | A program and its arguments, never a shell string |
| `user` | 5 | The four `user.manage` operations and when a group is set |
| `config` | 2 | `system.configure` |
| `soul` | 2 | `soul.propose_change` |
| `clarify` | 5 | Ask instead of guessing |
| `scope` | 5 | Things outside version 1 (network, packages) and injection-style requests |

The expected answers are data, not code. Each intent lists the plans that are
acceptable (`accept`): a complete plan in the version 2 format, and where a
field is the model's to choose, a `loose` entry (`any`, `contains` or
`one_of`) for that field only. Nothing else is loosened. When a request leaves
something open (no place, no count) the intent says in `note` what is
accepted and why.

The set is checked three ways before any run: `intent-shell/tests/eval_set.rs`
passes every accepted plan through the shell's own parser and rules
(`./scripts/check.sh`), `eval/test_score.py` tests the scoring, and the runner
refuses to start if an accepted plan fails the JSON Schema.

The wording is plain on purpose, so S8 can send the same intents under every
personality preset and a later model change can reuse them unchanged.

### Scoring

A model's answer is scored without a host, a resolver or a broker: only the
plan against the accepted plans, in `eval/score.py`.

| Level | Meaning |
|---|---|
| valid | The text passes the JSON Schema and the semantic rules of ADR 0011 (what the shell checks before anything else) |
| kind | Plan against question is right |
| actions | The same actions in the same order as an accepted plan |
| exact | The same arguments, dependencies and `elevate` as an accepted plan. `summary`, `question` and step ids are not compared |
| overreach | A mutating action, or a request for root, that no accepted plan contains. The broker would stop it, but the person would be asked for a confirmation that should not exist |

An answer that is not valid scores nothing: the shell would refuse it.
Independent steps in the other order are a different plan, so the intents with
two independent steps list both orders.

## Decision rule (fixed before measuring)

Candidates are models whose weights file is at most 3 GB. The project owner's
machine in S1 is an 8 GB Apple M1 with a 5.4 GB GPU budget, and Bonsai 8B is
1.16 GB there; a candidate that does not leave room for the context on that
machine is not a candidate. Larger models are measured as a ceiling, to show
what the set can reach, and cannot win.

The headline is the **exact** count on the first attempt, in the mode the
shell ships (output constrained by the JSON Schema, ADR 0011), temperature 0.

1. **Change the model** only if a candidate beats Bonsai 8B by at least 7
   intents of 68 (about 10 percentage points) on exact matches, and its
   overreach count is not higher, and its median time to a complete plan on
   this machine is under 10 seconds. Otherwise keep Bonsai 8B.
2. **Change the mode** (native tool calls instead of a constrained plan) only
   if, with the same model, it beats the constrained mode by the same margin
   on the same terms. ADR 0011 chose the constrained plan and this item does
   not reopen it: a win for native tool calls is the evidence for a new ADR
   that supersedes it, not a change made here.
3. A tie, or a gap below 7 intents, keeps the current model and mode. At
   temperature 0 a run is repeatable, and 7 of 68 is large against the
   differences between two runs of the same model (reported below).

The 10 second limit is for this machine (an AMD GPU through Vulkan), not for
the M1 of S1, and is only a sanity bound; speed on the target hardware is a
separate measurement.

## Models measured

Every claim below comes from a page that was read on 2026-11-16 (sources at the
end). The size is the size of the file that was run.

| Model | File (bytes) | Licence | Tool calling, as the vendor says | Role |
|---|---:|---|---|---|
| Bonsai 8B (the current model) | `Bonsai-8B-Q1_0.gguf` (1,158,654,496), 1-bit, 65,536 context | Apache-2.0 | **Not stated** on the model card or in the vendor's run guide. The only evidence is the single call recorded in S1 | Baseline |
| Ternary Bonsai 2 9B | `Ternary-Bonsai-2-9B-Q2_0.gguf` (2,556,640,128), ternary, 262,144 context, base Qwen3.5-9B | Apache-2.0 | Stated: PrismML's docs say all Bonsai 2 sizes "call tools natively", and its 9B page says tools go in as a standard `tools` array | Candidate (under the 3 GB limit) |
| Bonsai 27B (first generation, 1-bit) | `Bonsai-27B-Q1_0.gguf` (3,803,452,576), LM Studio's conversion of PrismML's weights | Apache-2.0 | The vendor's model card reports BFCL v3 and τ²-Bench scores for the 27B, 66.03 at 1-bit against 80.00 at FP16 (the vendor's own figures) | Ceiling only: over the 3 GB limit |
| Gemma 4 12B, QAT Q4_0 | `gemma-4-12B-it-QAT-Q4_0.gguf` (6,975,879,008), LM Studio's conversion of Google's weights | Apache-2.0 | Stated on Google's model card: "Native support for structured tool use" | Ceiling only: over the 3 GB limit |

Bonsai 2 9B is the "newer Bonsai 2 family" the roadmap says to compare
against. The 27B here is the first generation (the one LM Studio had on this
machine), not Bonsai 2 27B, which was not measured.

## Setup

| | |
|---|---|
| Machine | x86-64 Linux, 16 threads, 30 GB RAM, AMD Radeon RX 9070 (Vulkan, RADV driver) |
| Runtime | upstream `llama.cpp` `v1.0.0`, commit `b1b770d90` (the commit S1 used), built from source with `-DGGML_VULKAN=ON`; every model ran on this one build. Bonsai 2 needs `b11555` or newer: older builds load the `Q2_0` file and print nonsense |
| Server | `llama-server -m MODEL -c 8192 --parallel 1 --jinja -ngl 99`, one model at a time |
| Sampler | temperature 0 (one run per setting is repeatable, below) |
| Thinking | Off (`chat_template_kwargs: {"enable_thinking": false}`) in every run of the table, except the "as shipped" Bonsai 8B rows, which send nothing, as the shell does |
| Prompt | The shell's own system prompt, read from `intent-shell/src/model.rs` by the harness, and the schema file as `response_format`, so the baseline is the shipped path and not a copy of it |

Files were checked against the vendor's `SHA256SUMS` (Bonsai 2 9B) and S1's
recorded hash (Bonsai 8B); every run records the SHA-256 of the model, the
system prompt, the schema, the tools and the set (`header.json`).

The two modes:

- **grammar**: the shipped path. The model is constrained to the schema, and
  the plan is validated afterwards. "Exact with retry" repeats the request once
  after an invalid answer, with the refusal reason, as the shell does.
- **tools**: native tool calls. One function per action, generated from the
  schema's own step definitions (`$ref`s inlined), plus `clarify`. The same
  reference rules as the shell prompt, without its instructions about the JSON
  document. The harness numbers the calls `s1`, `s2`, ... in order and takes
  `depends_on` and `elevate` from the call. A plain-text reply, or a call
  that fails the schema, is an invalid answer. `tool_choice` is `auto`: the
  model decides whether to call anything.
