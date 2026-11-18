---
title: "ADR 0023: Path rules (NUL, length) live in code, not in the grammar"
type: adr
status: accepted
created: 2026-11-17
updated: 2026-11-17
tags: [shell, schema, grammar, paths]
---

# ADR 0023: Path rules (NUL, length) live in code, not in the grammar

## Context

ADR 0018, section 4, says: "The grammar bounds path length and rejects NUL;
`..` handling is a rule in code, as for names, not a `pattern`." While
implementing S3e (`docs/s3e-directories-and-tree.md`) the grammar was tried
as written: a `pattern` that excludes NUL (`[^\u0000]`) in the path string made
`llama-server` answer HTTP 500 when it converted the schema to a grammar, so no
plan could be generated at all.

## Decision

Two rules about a path are enforced by deterministic code, not by the schema
the model is constrained with:

- **NUL**: a path containing NUL is refused by `check_rules` in `plan.rs` (the
  semantic rules run after parsing) and by `docs/schemas/validate.py`.
- **Length**: a path of more than 4096 bytes is refused by the same two places.
  The schema keeps no `pattern` on a path.

Nothing else of ADR 0018 changes. A path with NUL or above the limit is still
refused before the resolver runs; only the layer that refuses it moves from the
grammar to the plan check. This supersedes only the sentence of ADR 0018
section 4 quoted above.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| **Rules in code (this ADR)** | Works with `llama-server`; the same place that already holds the `..` and name rules; one definition, tested, with invalid examples that run in the gate and in `validate.py` | The model is not constrained away from emitting NUL; it is retried with the refusal |
| A grammar `pattern` | Refused at generation time | HTTP 500 from `llama-server`: unusable today |
| `maxLength` only in the schema | Cheap, supported | Counts characters, not bytes; NUL still open; two places say the limit |

## Consequences

- The schema in `docs/schemas/action-plan.schema.json` is looser than the code
  on paths; the code is the authority and `validate.py` mirrors it.
- If a future `llama-server` handles the pattern, the rule can be added to the
  grammar as well; the code check stays, because it is the one that is tested.
- The model learns of a refused path through the retry message, as for the
  other semantic rules.
