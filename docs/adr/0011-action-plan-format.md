---
title: "ADR 0011: Action plan format of the intent shell"
type: adr
status: accepted
created: 2026-11-15
updated: 2026-11-15
tags: [slm, shell, plan, schema, capabilities, root, soul]
---

# ADR 0011: Action plan format of the intent shell

## Context

ADR 0002 says the intent shell model turns natural language into a structured
action plan and a deterministic capability broker validates every action. ADR
0007 says the AI acts with the intersection of the user's permissions and the
action's capability, and that root actions are confirmed one by one. ADR 0006
says the AI may propose a change to the SOUL and only a user with root
authority approves it. This ADR fixes the format all of that runs on.

Measured in S1 (`docs/s1-bonsai-8b-local-run.md`): Bonsai 8B emits a valid
tool call through `llama.cpp` and its own chat template, at about 100 tok/s of
prompt processing. Anything we put in the prompt costs time, so the format
must be small.

## Decision

### Shape

The model emits one JSON document, either a plan or a question
(`docs/schemas/action-plan.schema.json`, version 1):

```json
{"kind":"plan","version":1,"summary":"Delete the three largest files in ~/Downloads","steps":[
 {"id":"s1","action":"fs.find_largest","args":{"path":"~/Downloads","count":3},"depends_on":[],"elevate":false},
 {"id":"s2","action":"fs.delete","args":{"path":{"from_step":"s1"}},"depends_on":["s1"],"elevate":false}]}
```

- One action is a plan with one step. A sequence is the list. Dependencies
  are `depends_on`: a step starts only after the steps it names finished
  successfully; steps with no dependency between them are independent.
- `{"kind":"clarify","question":...}` is how the model asks instead of
  guessing. It carries no action, so it can never run anything.
- A path argument of some actions may be `{"from_step":"sN"}`: every path
  that step produced (`fs.list` and `fs.find_largest` produce paths). The
  broker expands it into concrete actions before checking capabilities and
  before confirmation, so the user sees each real path.
- Limits: at most 16 steps; strings are length-bounded; `process.run` takes a
  program name from a registry plus an argument array, never a shell string.
- There is no free-form field the model can use to ask for more authority.
  `elevate` is a request, not a grant (below).

### Initial actions

| Action | Arguments | Capability | Root |
|---|---|---|---|
| `fs.list` | path | `fs.read` | on request |
| `fs.stat` | path | `fs.read` | on request |
| `fs.read` | path | `fs.read` | on request |
| `fs.find_largest` | path, count | `fs.read` | on request |
| `fs.write` | path, content, mode (create, overwrite, append) | `fs.write` | on request |
| `fs.move` | from, to | `fs.write` on both ends | on request |
| `fs.delete` | path | `fs.delete` | on request |
| `process.run` | program, argv | `process.exec` | on request |
| `user.manage` | operation (add, remove, add_to_group, remove_from_group), user, group | `user.admin` | always |
| `system.configure` | key, value | `system.config` | always |
| `soul.propose_change` | rationale, new_text | `soul.propose` | see below |

Not in version 1, on purpose: network, package installation, windows. Each
needs a server that does not exist yet. A new action is a new ADR or an
amendment, and a version bump if it changes an existing one.

### How the broker validates

Order, all in deterministic code (ADR 0002); the model's text is never
consulted again after parsing:

1. **Schema.** The document must validate against the schema.
2. **Semantic rules**, which JSON Schema cannot express: step ids are
   unique; `depends_on` names existing steps and has no cycle or self
   reference; a `from_step` reference names a step that is also in
   `depends_on` and produces paths; `group` is set exactly for the group
   operations of `user.manage`. A reference implementation of these rules
   sits in `docs/schemas/validate.py` and the broker must behave the same.
3. **Capability.** For each step the broker holds the action's capability
   from the table. The AI's session has a capability set (initially all of
   them) and the step is refused if its capability is not in that set.
4. **User permission.** The step is checked against the user's own
   permissions on the resources it names (ADR 0007: owner, group, bits). The
   step runs only if both 3 and 4 allow it: the intersection, never the
   union.
5. **Root.** See next section.
6. **Dry run, confirmation, log**, as S3 specifies. Every document the model
   produced, valid or not, and every decision of the broker is logged
   (ADR 0002).

A refused step stops the steps that depend on it. Independent steps still
run only if the user confirms the partial plan.

### Root and confirmation

Whether a step needs root is decided by the broker from the table, never by
the model:

- `always`: `user.manage` and `system.configure`. The schema does not even
  allow `elevate` on them.
- `on request`: the other actions run with the user's own permissions. If
  the model sets `elevate: true`, the broker holds the step until a person
  allowed to act as root (an administrators group member, ADR 0007) confirms
  it. Without `elevate`, a step that fails the user's permissions is refused,
  never silently promoted to root. A refused step can be repeated by the user
  with elevation.
- Confirmation is per step, shows the exact expanded action (paths, content,
  arguments) as the broker will run it, never the `summary`, and grants
  nothing beyond that step. There is no "always allow" and no standing grant
  (ADR 0007). Each root step is logged with who confirmed.

### Proposing a change to the SOUL

`soul.propose_change` carries the complete proposed SOUL text and a
rationale. The model never writes the diff: the system computes it from the
current SOUL and `new_text`, and that full diff is what a person sees.

ADR 0006 gives this action a capability held only by users with root
authority, and also lets the AI propose adjustments. Both hold with this
split:

- Emitting the action only records a pending proposal. It changes nothing.
  Any user's AI may do it (capability `soul.propose`).
- Applying it is not an action the model can emit. It is a step by the
  broker that requires root confirmation of the full diff, per proposal. A
  proposal not approved by someone with root authority has no effect, and
  approval by a user without root authority is refused.
- Every applied change goes to the history and can be undone (S10).

This reading of ADR 0006 is an interpretation made here; if the user reads it
differently the ADR changes, not ADR 0006.

### Constraining the output

The server enforces the schema while decoding: `llama-server` accepts a JSON
Schema (`response_format` of type `json_schema`) and turns it into a grammar,
so the decoder cannot emit an action that is not in the table or an argument
of the wrong type. The same schema file is used again by the broker, because
the broker must not trust what reached it. One artifact serves both.

The grammar hides the schema from the model, so the prompt still needs a
compact description of the actions (241 tokens in total with a short request in the measurement below) and the
rule for `from_step`. Put that static text first and the user's request last:
the prefix is then reused by `cache_prompt` (S1).

If the output does not validate (cut off by the token limit, or a semantic
rule fails):

1. Nothing runs, not even the steps that look valid.
2. One retry, with the broker's error message added to the conversation.
3. If that fails too, the shell says it could not make a plan and points to
   the deterministic shell. It never falls back to executing a guess.
4. All attempts are logged.

Measured on 2026-11-15 on the development machine (Bonsai 8B, Q1_0, `llama-server`
with the real schema, temperature 0, five intents): the schema converted
without error and 5 of 5 outputs were structurally valid. Quality was weak:
three plain requests were answered with `clarify`, and one plan wrote
`"s1.path"` as a plain string where `{"from_step":"s1"}` was meant. A plain
string is a literal path, so the broker must treat it as one and the
delete would target a file named `s1.path`; this is safe but wrong, which is
why the evaluation of S4 matters and why more prompt guidance or examples are
likely needed. It is five requests, not a benchmark.

### The deterministic shell reaches the same actions

S5 types the same actions as commands, with no model:

- The command is the action name and the arguments are named flags:
  `fs find_largest --path ~/Downloads --count 3`. The mapping is mechanical
  from the table, so a new action is reachable the day it is added.
- One command line parses into a one-step plan; a script is a list of lines
  that parse into a plan. `--elevate` sets `elevate`; `$s1`-style
  references and ordering are left to S5.
- The parsed plan then goes through the same schema, semantic rules and
  broker as a model plan. There is one path to execution, so the model adds
  convenience, never reach (ADR 0002 rule 2).

### Where host code lives

This ADR adds no Rust. The schema, the examples and the reference validator
are under `docs/schemas/`; the validator is a Python script run in isolation
(`uv run --with jsonschema python docs/schemas/validate.py`) and is not part of
`scripts/check.sh`. `.cargo/config.toml` sets the bare-metal target for the
whole workspace, so a host-side crate cannot be added to it unchanged. S3
decides how: a separate workspace outside the kernel's target, or a per-package
override. S3 also puts the schema check and the semantic rules into its own
tests, using the examples here.

## Alternatives considered

| Topic | Chosen | Not chosen, and why |
|---|---|---|
| Output constraint | JSON Schema turned into a grammar by `llama.cpp`, validated again by the broker | Hand-written GBNF: more control, but a second artifact that can drift from the schema. Native tool calls: proven in S1 and better for models trained on them, but nothing guarantees a valid call, it depends on each model's template, and a plan with dependencies fits badly. Kept as a comparison for S4 |
| Plan shape | List with ids and `depends_on` | A linear sequence only: simpler, but cannot say two steps are independent |
| First actions | Small core of 11 | Read only plus SOUL: too little to exercise root confirmation. Core plus network and packages: bigger prompt and more attack surface without servers to back it |
| Who says a step needs root | The broker, from the action table; the model may only ask | The model marks it: a model that forgets the mark would skip confirmation |
| Asking for clarification | Top-level `clarify`, no action | An action in the list: it could then be mixed with real steps |

## Consequences

- S3 implements the broker against this table and the schema, with the
  examples as tests.
- The prompt carries an action description of about 220 tokens plus the
  user's request; with a prefix that does not change, `cache_prompt` pays for
  it once.
- A plan limited to 16 steps and 100 expanded paths per reference (the second
  limit is the broker's) bounds what one confirmation can cover.
- Text read by the model from files or the web is data. A later planning
  turn that includes such text cannot widen any capability, because the
  broker does not read it; the risk that remains is a misleading plan, which
  confirmation shows in full.
- The model is not told what it may not do. It is only unable to emit it.
  Refusals come from the broker, and the shell says why.

## Open questions

- The registry of programs for `process.run`, and the keys `system.configure`
  accepts, are defined with the broker in S3.
- The size limit of `new_text` (16 KiB) is provisional until S8 defines the
  SOUL format.
- Whether the model's weak showing in the measurement above is fixed by the
  prompt, by examples, or needs another model: S4.
