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
