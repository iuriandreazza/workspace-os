---
title: "S4: evaluation set and model comparison"
type: report
status: done
created: 2026-11-16
updated: 2026-11-16
tags: [slm, bonsai, evaluation, tool-calling, llama-cpp]
---

# S4: evaluation set and model comparison

Roadmap item S4: an evaluation set of intents with expected plans, Bonsai 8B
measured on it, and at least one alternative with native tool calling.

## Result in short

- **Decision: keep Bonsai 8B, and open the change of mode.**
  Asking for native tool calls (with the dependencies worked out by the
  harness) took Bonsai 8B from 28 to 44 exact plans out of 67. Ternary
  Bonsai 2 9B gets 34 in the current constrained mode as the shell sends it (38
  with thinking off), and 47 in native tool calls: its lead over Bonsai 8B is
  3 intents there, under the threshold of 7 fixed before the run. Changing the mode needs a new ADR that
  supersedes the output-constraint part of ADR 0011 (follow-up below).
- **Rule 1 (change the model) is not met** by the request the shell sends
  today: Bonsai 2 9B gets 34 exact plans against 28, a gap of 6 where 7 was
  required. The gap would be 10 (38 against 28) with thinking switched off,
  which is a change to the request. Both figures are one run at temperature 0
  and the shell's limit of 1,024 output tokens; the gap is close to the 4
  intents by which the same model moves between sampler settings.
- No small model is good enough to act unattended. The best candidate under
  3 GB gets 47 of 67 plans exactly right (70%); a "model" that only ever asks
  a question gets 15 of 67 (22%). The broker and the confirmation stay the
  safety, as ADR 0002 and ADR 0007 say.

## The set

`eval/intents.json` holds 67 intents: 57 in English and 10 in Brazilian
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
| `write` | 5 | `new_name` against `name`, and the mode: create, overwrite, append |
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
`one_of`) for that field only. Nothing else is loosened. Where a request leaves
something open (no place, no count) the intent says in `note` what is accepted
and why. The wording is plain on purpose, so S8 can send the same intents under
every personality preset and a later model change can reuse them unchanged
(`run_eval.py --prompt-extra FILE` appends a preset to the system prompt).

The set is checked before any run: `intent-shell/tests/eval_set.rs` passes
every accepted plan through the shell's own parser and rules
(`./scripts/check.sh`), `eval/test_score.py` and `eval/test_modes.py` test the
scoring, the prompts and the conversion of tool calls, and the runner refuses
to start if an accepted plan fails the JSON Schema.

### Scoring

A model's answer is scored without a host, a resolver or a broker: only the
plan against the accepted plans, in `eval/score.py`.

| Level | Meaning |
|---|---|
| valid | The text passes the JSON Schema, the semantic rules of ADR 0011 and the two things only the shell's parser refuses (a fraction where an integer is expected, a trailing newline in an identifier, a lone surrogate) |
| kind | Plan against question is right |
| actions | The same actions in the same order as an accepted plan (dependent steps are put in dependency order first, as the shell does) |
| exact | The same arguments, dependencies and `elevate` as an accepted plan. `summary`, `question` and step ids are not compared |
| overreach | A mutating action, or a request for root, that no accepted plan contains |
| off target | A mutating step whose arguments, with the steps it reads from, are not those of an accepted plan: deleting `other.zip` for `old.zip`, or deleting 100 files for two. An upper bound: another way to name the same item also counts |

An answer that is not valid scores nothing: the shell would refuse it. Overreach
and off target are read from the answer the shell would act on (the retry, when
the first answer was invalid and the retry is valid); valid, kind, actions and
exact are the first attempt. A harmless extra read-only step (listing a place
before deleting from it) makes a plan not exact.

## Decision rule (fixed before measuring, commit `53ab082`)

Candidates are models whose weights file is at most 3 GB. The project owner's
machine in S1 is an 8 GB Apple M1 with a 5.4 GB GPU budget, and Bonsai 8B is
1.16 GB there; a candidate that does not leave room for the context on that
machine is not a candidate. Larger models are measured as a ceiling, to show
what the set can reach, and cannot win.

The headline is the **exact** count on the first attempt, in the mode the
shell ships (output constrained by the JSON Schema, ADR 0011), temperature 0.

1. **Change the model** only if a candidate beats Bonsai 8B by at least 7
   intents on exact matches, and its overreach count is not higher, and its
   median time to a complete plan on this machine is under 10 seconds.
   Otherwise keep Bonsai 8B.
2. **Change the mode** (native tool calls instead of a constrained plan) only
   if, with the same model, it beats the constrained mode by the same margin
   on the same terms. ADR 0011 chose the constrained plan and this item does
   not reopen it: a win for native tool calls is the evidence for a new ADR
   that supersedes it, not a change made here.
3. A tie, or a gap below 7 intents, keeps the current model and mode.

The 10 second limit is for this machine (an AMD GPU through Vulkan), not for
the M1 of S1, and is only a sanity bound.

### What changed after the rule was fixed

The text above is as committed, except for four edits: "7 of 68" intents is now
7 intents (the set has 67, so 10.4 percentage points, and the parenthesis "about
10 percentage points" was dropped); the third sentence of rule 3 ("At temperature
0 a run is repeatable, and 7 of 68 is large against the differences between two
runs of the same model (reported below)") was dropped, because the report shows
differences of up to 4 between sampler settings; and a clause saying speed on
the target hardware is a separate measurement was dropped. Four
review rounds (below) changed the set and the harness; each change and its
reason:

- **`write-then-read` removed** (68 to 67 intents). Its only accepted plan
  creates `notes.txt` and reads it back, and the broker resolves every step
  against the filesystem as it is before any step runs, so the read of a file
  that does not exist yet is blocked. A plan the shell cannot run must not be
  the right answer. This was removed after the first model results were seen,
  by a reviewer's finding, not to favour a model: in the first round no
  temperature 0 run got it exact (one sampled run of Bonsai 2 9B did).
- **`write-create-no-place`**: an `everywhere` alternative was dropped and then
  put back, so the set is as frozen: the prompt says to use `everywhere` when
  no place is given.
- **Scoring**: `valid` also applies the checks only the shell's parser makes
  (no stored answer fails them). Overreach is as defined when frozen (by action). "Off target"
  was added because the review showed deleting the wrong file, or too many
  files through a wider search, did not count. Dependent steps in reverse
  order are put in dependency order before comparing. Levels are computed again
  from the stored answers every time, so these changes applied to every run
  without asking a model again.
- **Native tool calls, two variants.** The first harness asked the model to
  number its own calls and list `depends_on` ("tools"). Bonsai 8B wrote
  `depends_on: ["s1"]` on its first call in 50 of 67 answers, so 12 plans were
  exact: that measures following a numbering convention, not tool calling. A
  variant was added after seeing this, in which the model is not asked for
  `depends_on` and the harness derives it from the `from_step` references
  ("tools-derived"). Both are reported; the derived one was not pre-registered.
- **Everything was run again** from one commit (`f787c8d`, clean tree, the
  committed `eval/run-matrix.sh`) after the last change that could alter an
  answer or a score. Later commits only add robustness (`complete` check,
  incomplete-run warning, handling of malformed server replies) and leave the
  request, the prompts and the scoring as measured.

## Models measured

Every claim below comes from a page that was read on 2026-11-16 (sources at the
end). The size is the size of the file that was run.

| Model | File (bytes) | Licence | Tool calling, as the vendor says | Role |
|---|---:|---|---|---|
| Bonsai 8B (the current model) | `Bonsai-8B-Q1_0.gguf` (1,158,654,496), 1-bit, 65,536 context | Apache-2.0 | No statement on the model card or in the vendor's run guide; the card's benchmark table has a BFCL column, 65.7 for Bonsai 8B (the vendor's own figure). The other evidence is the single call recorded in S1 | Baseline |
| Ternary Bonsai 2 9B | `Ternary-Bonsai-2-9B-Q2_0.gguf` (2,556,640,128), ternary, 262,144 context, base Qwen3.5-9B | Apache-2.0 | Stated: PrismML's docs say all Bonsai 2 sizes "call tools natively", and its 9B page says tools go in as a standard `tools` array | Candidate (under the 3 GB limit) |
| Bonsai 27B (first generation, 1-bit) | `Bonsai-27B-Q1_0.gguf` (3,803,452,576), LM Studio's conversion of PrismML's weights | Apache-2.0 | The vendor's model card reports, for the 27B, BFCL v3 70.72 and τ²-Bench 61.34 at 1-bit against 77.10 and 82.90 at FP16 (the vendor's own figures) | Ceiling only: over the 3 GB limit |
| Gemma 4 12B, QAT Q4_0 | `gemma-4-12B-it-QAT-Q4_0.gguf` (6,975,879,008), LM Studio's conversion of Google's weights | Apache-2.0 | Stated on Google's model card: "Native support for structured tool use" | Ceiling only: over the 3 GB limit |

Bonsai 2 9B is the "newer Bonsai 2 family" the roadmap says to compare
against. The 27B here is the first generation (the one LM Studio had on this
machine), not Bonsai 2 27B, which was not measured.

## Setup

| | |
|---|---|
| Machine | x86-64 Linux, 16 threads, 30 GB RAM, AMD Radeon RX 9070 (Vulkan, RADV driver) |
| Runtime | upstream `llama.cpp` `v1.0.0`, commit `b1b770d90` (the commit S1 used), built from source with `-DGGML_VULKAN=ON`; every model ran on this one build. Bonsai 2 needs `b11555` or newer: older builds load the `Q2_0` file and print nonsense |
| Server | `llama-server -m MODEL -c 8192 --parallel 1 --jinja -ngl 99`, one model at a time (16384 for the thinking run) |
| Sampler | temperature 0, one request per intent; one retry in the constrained mode, as the shell does |
| Thinking | Off (`chat_template_kwargs: {"enable_thinking": false}`) in every row, except the rows marked "as shipped" and "thinking", which send nothing, as the shell does today |
| Prompt | The shell's own system prompt, read from `intent-shell/src/model.rs` by the harness, and the schema file as `response_format`, so the baseline is the shipped path and not a copy of it |

Files were checked against the vendor's `SHA256SUMS` (Bonsai 2 9B) and S1's
recorded hash (Bonsai 8B). Every run directory in `eval/results/` has the raw
answers and a `header.json` with the commit, the SHA-256 of the model, the
server binary, the chat template, the system prompt, the schema, the tools and
the set.

The modes:

- **grammar**: the shipped path. The model is constrained to the schema, and
  the plan is validated afterwards.
- **tools**: native tool calls. One function per action, generated from the
  schema's own step definitions (`$ref`s inlined), plus `clarify`; the same
  reference rules as the shell prompt, without its instructions about the JSON
  document. The harness numbers the calls `s1`, `s2`, ... in order. The model
  fills `depends_on` and `elevate`. `tool_choice` is `auto`. A plain-text reply
  is an invalid answer, nothing is repaired, and tools mode is not retried.
- **tools-derived**: as tools, but the tools have no `depends_on` and the
  prompt does not mention it; the harness sets it from the `from_step`
  references.

## Results

All figures are the number of intents out of 67. Every run had zero failed
requests.

### Accuracy

| Run | valid | kind | actions | exact | overreach | off target |
|---|---:|---:|---:|---:|---:|---:|
| *always answer with a question* | – | – | – | 15 | 0 | – |
| *always list the home folder* | – | – | – | 2 | 0 | – |
| **Bonsai 8B, grammar, as shipped** | 64 | 53 | 37 | **28** | 13 | 18 |
| Bonsai 8B, grammar, thinking off | 64 | 53 | 37 | 28 | 13 | 18 |
| Bonsai 8B, tools | 13 | 12 | 12 | 12 | 1 | 1 |
| **Bonsai 8B, tools-derived** | 64 | 57 | 51 | **44** | 5 | 10 |
| Bonsai 2 9B, grammar, as shipped (thinking on) | 56 | 53 | 35 | 34 | 5 | 10 |
| **Bonsai 2 9B, grammar, thinking off** | 66 | 58 | 40 | **38** | 13 | 16 |
| Bonsai 2 9B, tools | 64 | 59 | 49 | 46 | 3 | 4 |
| **Bonsai 2 9B, tools-derived** | 64 | 59 | 51 | **47** | 4 | 5 |
| Bonsai 2 9B, tools, thinking on | 64 | 63 | 48 | 46 | 1 | 1 |
| *ceiling:* Bonsai 27B (1st gen), grammar | 65 | 59 | 46 | 38 | 14 | 21 |
| *ceiling:* Bonsai 27B, tools | 67 | 66 | 61 | 55 | 2 | 5 |
| *ceiling:* Bonsai 27B, tools-derived | 66 | 65 | 60 | 54 | 2 | 5 |
| *ceiling:* Gemma 4 12B, grammar | 66 | 64 | 57 | 56 | 4 | 6 |
| *ceiling:* Gemma 4 12B, tools | 64 | 59 | 38 | 32 | 4 | 5 |
| *ceiling:* Gemma 4 12B, tools-derived | 59 | 56 | 40 | 34 | 4 | 5 |

The retry of the constrained mode added at most one exact plan to any run
(Bonsai 27B: 38 to 39); no other run changed, so the retry column is left out
(`python3 eval/score.py table eval/results/*` prints it).

Sampling, Bonsai 2 9B with the vendor's non-thinking sampler at temperature
0.7 (one sample each, so not repeatable): grammar 34 exact (against 38 at
temperature 0), tools 46 (against 46). With thinking on in tools mode (the
vendor's default, temperature 1.0, up to 6,000 tokens): 46. The differences
between these settings are 4 intents or fewer.

Repeatability: three constrained runs of Bonsai 8B (as shipped, a repeat of it,
and thinking off) gave byte-identical answers.

### By category (exact plans)

| Category | n | 8B grammar | 8B tools-derived | 9B grammar | 9B tools-derived | 27B tools | Gemma grammar |
|---|---:|---:|---:|---:|---:|---:|---:|
| chain | 3 | 1 | 2 | 2 | 2 | 2 | 3 |
| clarify | 5 | 0 | 0 | 0 | 1 | 5 | 5 |
| config | 2 | 0 | 2 | 1 | 2 | 2 | 2 |
| delete | 7 | 4 | 5 | 3 | 5 | 5 | 4 |
| elevate | 5 | 1 | 3 | 2 | 2 | 2 | 5 |
| largest | 8 | 8 | 8 | 7 | 8 | 8 | 8 |
| list | 6 | 5 | 6 | 5 | 6 | 6 | 5 |
| move | 5 | 1 | 2 | 1 | 2 | 3 | 4 |
| process | 3 | 3 | 2 | 3 | 3 | 3 | 3 |
| scope | 5 | 0 | 1 | 0 | 1 | 4 | 2 |
| soul | 2 | 0 | 1 | 2 | 2 | 2 | 2 |
| stat-read | 6 | 2 | 5 | 4 | 5 | 6 | 4 |
| user | 5 | 1 | 5 | 5 | 5 | 5 | 5 |
| write | 5 | 2 | 2 | 3 | 3 | 2 | 4 |

### Speed and prompt size (this machine, not the M1)

| Run | median request | 95th percentile | prompt tokens | generation tok/s |
|---|---:|---:|---:|---:|
| Bonsai 8B, grammar | 0.43 s | 0.77 s | 475 | 157 |
| Bonsai 8B, tools-derived | 0.30 s | 0.46 s | 3,150 | 155 |
| Bonsai 2 9B, grammar, thinking off | 1.22 s | 1.97 s | 485 | 118 |
| Bonsai 2 9B, grammar, as shipped | 1.95 s | 17.13 s | 483 | 120 |
| Bonsai 2 9B, tools-derived | 0.60 s | 0.87 s | 3,263 | 114 |
| Bonsai 27B, tools | 1.53 s | 2.87 s | 3,836 | 52 |
| Gemma 4 12B, grammar | 2.33 s | 3.92 s | 524 | 64 |

The prompt is the system prompt, the schema or the tools, and the request. The
constrained mode sends about 480 tokens (the schema is not in the prompt: the
server turns it into a grammar); native tool calls send 3,100 to 3,800 for the Bonsai models (1,300 to 1,800 for
Gemma, whose template renders the tools more compactly), because every tool
carries its inlined reference definitions. At the 100 tokens per
second of prompt processing that S1 measured on the M1, 3,150 tokens would be
about 30 seconds for the first request, and `cache_prompt` would reuse them
afterwards. That is an estimate from S1's rate, not a measurement on the M1,
and a more compact tool definition is part of the follow-up.

### What the models get wrong

- **Asking and refusing.** The Bonsai models almost never ask: `clarify` and
  `scope` together are 10 intents, and Bonsai 8B gets 0 of them in the
  constrained mode and 1 with native tool calls; Bonsai 2 9B gets 0 and 2. In
  the constrained mode Bonsai 8B answers "delete it" with a delete, "install
  firefox" with `system.configure` and `process.run`, and "give me
  administrator rights" with `system.configure`; Bonsai 2 9B answers the last
  with `user.manage`. Those are the overreach counts. The 27B with tool calls (9 of the 10; 8 with derived dependencies) and
  Gemma 4 12B constrained (7 of the 10) ask far more often.
- **Mode, not size, moves Bonsai.** The same weights, asked through native tool
  calls instead of the constrained plan, gain 16 intents (8B), 9 (9B) and 16
  (27B) and halve or better the overreach. Gemma 4 12B goes the other way: 56
  in the constrained mode, 32 to 34 with tool calls. The effect depends on the model. Gemma's tool answers often stop after one
  exploratory step (a `fs_list` before the delete), which a one-shot harness
  scores as wrong; whether it would do better in a loop that feeds results
  back was not measured.
- **Thinking and the shell's token limit do not mix.** Bonsai 2 9B with the
  template's default (thinking on) in the constrained mode: all 67 answers
  carried a reasoning trace, 11 ran out of the shell's 1,024-token limit before
  the JSON (all 11 stopped for length) and are invalid, and the median time is
  1.95 s with a 95th percentile of 17 s. The vendor says to allow at least 8,192
  output tokens with thinking on and to avoid temperature 0 there; neither was
  tried in the constrained mode. The one thinking run with a larger limit (tools
  mode, 6,000 tokens, temperature 1.0) used at most 623 tokens and got 46.
- **The intents the S3 and S3b reports recorded as wrong.** `delete-largest-two` (search, then delete through `from_step`) is exact only for Bonsai
  2 9B and Gemma 4 12B in the constrained mode; Bonsai 8B misses it in every
  mode, and so do the 9B and the 27B with native tool calls (they plan the
  search and stop). `write-create` (`name` instead of `new_name`, which the S3b
  resolver reports as not found) is still wrong for Bonsai 8B in every mode and
  for the 27B with tool calls, and right for Bonsai 2 9B. "Add the user bia to
  the group staff" fails validation in the constrained mode for Bonsai 8B (no
  group; also `user-remove-group` and `user-add-group-pt`) and is exact for
  Bonsai 8B with tool calls, for the 9B constrained and with tool calls, for
  the 27B with tool calls, and for Gemma constrained.

## Decision

Applying the rule fixed before measuring:

1. **Model, in the shipped mode.** As the shell sends the request today,
   Bonsai 2 9B beats Bonsai 8B by 6 exact plans (34 against 28): less than 7.
   Rule 1 is not met and rule 3 keeps the current model. With thinking
   switched off the gap is 10 (38 against 28), overreach 13 against 13 and
   median 1.22 s, which would meet it; that needs `enable_thinking: false` in
   the request, a change to `intent-shell/src/model.rs` that this item does not
   make. The result is close: one run per setting, the sampler settings of the
   same model move it by up to 4 intents, and the larger token limit the vendor
   recommends for thinking was not tried in this mode.
2. **Mode.** Native tool calls beat the constrained mode by at least 7 for
   Bonsai 2 9B (46 and 47 against 38) and for Bonsai 27B (55 and 54 against
   38), with less overreach each time. For Bonsai 8B they do so only with the
   variant that was not pre-registered (44 against 28 with derived
   dependencies; 12 against 28 with the first protocol, which measures the
   numbering convention). For Gemma 4 12B it goes the other way. Rule 2 is met
   for the Bonsai family, and is the evidence for a new ADR, not a change made
   here.
3. **Both together (not in the rule).** In native tool calls with derived
   dependencies, Bonsai 2 9B beats Bonsai 8B by 3 (47 against 44), under the
   threshold of 7.

**Keep Bonsai 8B, and open the mode change.** The model decision follows rule
1 and rule 3. The mode recommendation follows rule 2 and its caveat on Bonsai 8B
above. Other reasons, none of them measured against the rule:

- the mode's gain for the model already in use is 16, with no new weights; the
  swap's best case is 10 and needs a change of request;
- once the mode changes, the 9B's lead is 3, below the threshold;
- the 9B is 2.56 GB against 1.16 GB and was not run on the 8 GB M1 of S1 at
  all. Its `Q2_0` file needs `llama.cpp` build `b11555` or newer; the build used
  here (`v1.0.0`) is build 12359, the same commit S1 recorded for Homebrew's
  build on the M1, so that is not an obstacle there, but Metal was not tried.

If the owner would rather change the model now, the swap is Ternary Bonsai 2
9B, `Q2_0` file, with `enable_thinking: false` sent on every request.

### Follow-ups (not started)

- **A new ADR** superseding the output-constraint choice of ADR 0011 for native
  tool calls with dependencies derived from `from_step` references: the tools
  built from the schema, a tool prompt smaller than today's 3,100 tokens, plain-text
  replies treated as a question, a retry in tools mode, and what happens with a
  model (Gemma 4 12B) that does better constrained. Then re-measure the 8B and
  9B inside it. This needs a roadmap row (here called S4b), which this item does
  not add.
- The weak spots are the same in every small model: asking instead of acting,
  and refusing what version 1 does not offer. The prompt in `model.rs` never
  says network and packages are out of version 1; that is a prompt change (S8
  is also touching the prompt) and `run_eval.py --prompt-extra` is the way to
  test it against this set.
- `docs/s1-bonsai-8b-local-run.md` and the roadmap's model notes say Bonsai 8B
  does tool calls; the vendor page does not, and this item measured it at 12 of
  67 exact with the first protocol and 44 with derived dependencies.

## Reproduce

Build `llama.cpp` at the commit S1 used (Vulkan; SPIRV-Headers is needed and
was installed into a local prefix here):

```bash
git clone --depth 1 --branch v1.0.0 https://github.com/ggml-org/llama.cpp.git
cd llama.cpp
cmake -B build -DGGML_VULKAN=ON -DCMAKE_BUILD_TYPE=Release -DLLAMA_CURL=OFF
cmake --build build --config Release -j10 --target llama-server
./build/bin/llama-server --version   # commit b1b770d
```

The models (2.56 GB for Bonsai 2 9B; Bonsai 8B, Bonsai 27B and Gemma 4 12B
were already in LM Studio's folder):

```bash
mkdir -p ~/models/bonsai-2-9b && cd ~/models/bonsai-2-9b
curl -L -O https://huggingface.co/prism-ml/Ternary-Bonsai-2-9B-gguf/resolve/main/Ternary-Bonsai-2-9B-Q2_0.gguf
echo "1f884aa56aacd2bbbb2e4b7abaead2b1102b74a9c75a23fc33dab916813a384d  Ternary-Bonsai-2-9B-Q2_0.gguf" | sha256sum -c
```

Every measurement, one server at a time (about half an hour on this machine; a rerun skips complete runs):

```bash
LLAMA_SERVER=/path/to/llama.cpp/build/bin/llama-server \
BONSAI_8B=/path/Bonsai-8B-Q1_0.gguf \
BONSAI_2_9B=~/models/bonsai-2-9b/Ternary-Bonsai-2-9B-Q2_0.gguf \
BONSAI_1_27B=/path/Bonsai-27B-Q1_0.gguf \
GEMMA_4_12B=/path/gemma-4-12B-it-QAT-Q4_0.gguf \
./eval/run-matrix.sh bonsai8b bonsai2 bonsai27b gemma sampling thinking
```

One run by hand against a server already up, and the tables from the stored
answers (no model needed):

```bash
uv run --with jsonschema python eval/run_eval.py --url http://127.0.0.1:8089 \
  --mode grammar --label my-run --model-file MODEL.gguf --no-thinking --retry
python3 -I eval/score.py table eval/results/*
python3 -I eval/score.py categories eval/results/bonsai8b-grammar eval/results/bonsai2-9b-tools-derived
python3 -I eval/score.py timing eval/results/*
python3 -I eval/score.py compare eval/results/bonsai8b-grammar eval/results/bonsai2-9b-grammar
python3 -I eval/score.py failures eval/results/bonsai8b-grammar
python3 -I eval/score.py baselines
```

The checks that run in `./scripts/check.sh`: `python3 -I eval/test_score.py`,
`python3 -I eval/test_modes.py` and `intent-shell/tests/eval_set.rs`.

## Review

Every pull request that adds harness code is reviewed locally and
adversarially (`CLAUDE.md`). Four rounds were run, each by a fresh reviewer
that got the diff and the ADRs and not this report's reasoning. The finding counts are as the reviewers reported them; the answers of the first rounds were not kept in the repository.

| Round | Findings | What was done |
|---|---:|---|
| 1 | 11 | An unrunnable expected plan removed; overreach split in two; clarify and tool calls no longer repaired; strictness the shell has added; dependency order; retry time; the 9B measured as shipped; commit and hashes in the header; baselines. Accepted: a plain-text reply counts as invalid in tools mode, and some intents accept only a question |
| 2 | 13 | Derived-dependencies variant; off target through the search a delete reads from; dotted tool names; lone surrogates; matrix guards. Accepted: the scope and soul/config intents compare only what their notes state |
| 3 | 15 | Retry failures and retry overreach counted; derived prompt without `depends_on`; matrix resume and server identity check; `RecursionError`. Accepted: the retry text is the schema's message rather than the parser's; tool descriptions carry small hints the shell prompt lacks |
| 4 | 7 | No scoring bug. Incomplete runs flagged and not skipped; `grep -F`; a weak test fixed; malformed server replies caught. Accepted: the columns have two bases (first attempt, and the answer the shell acts on), written under "Scoring" |

A fifth round fact-checked this report against the stored answers and reviewed the code added since the measurement; its findings (a wrong run time, the framing of rule 1, the BFCL figures, the llama.cpp build, a false sentence about `user-add-group`, and gaps in the completeness check and in the guard for malformed replies) are fixed in this report and in `eval/`.

## Not verified, and limits

- **Not verified:** the vendors' benchmark figures; that Bonsai 8B supports
  tool calling (no vendor page says so); Bonsai 2 9B on Metal, on the 8 GB M1
  and its memory including the context cache (nothing here measured memory);
  whether the `llama.cpp` of Homebrew or LM Studio runs the Bonsai 2 `Q2_0`
  file; the `PTQ1_0` and `PQ2_0` packings, the drafters, and Bonsai 2 27B, 4B
  and 2B; Gemma 4's parameter count beyond its name; the licence review that
  ADR 0005 asks for the weights.
- **The set:** 67 intents, one wording each, one run per setting at
  temperature 0 (repeatable for the runs of the table; the two at 0.7 are one
  sample). The `scope` intents measure a limit the shipped prompt does not state.
  `config` and `soul` intents compare the action, not the key or the text.
  `elevate-read` reads `hosts` in `everywhere`, which resolves to the home
  folder on this host. A plain-text answer in tools mode is scored as invalid
  even when it is a sensible question, and tools mode is not retried while the
  shell retries. The retry message is the schema's text, not the parser's. An
  "always ask" answer scores 15 of 67, so read every number against that.
- **The machine:** an x86-64 desktop with an AMD GPU. Speed and prompt cost on
  the 8 GB M1 are estimates from S1, not measurements.

## Sources

Read on 2026-11-16.

- [PrismML docs, Bonsai 2 introduction](https://docs.prismml.com/models/bonsai-8b.md) (served the Bonsai 2 page for this address), [formats and runtime support](https://docs.prismml.com/download/formats.md), [Ternary Bonsai 2 9B](https://docs.prismml.com/models/bonsai-2-9b.md)
- [Bonsai 8B model card](https://huggingface.co/prism-ml/Bonsai-8B-gguf), [Bonsai 27B model card](https://huggingface.co/prism-ml/Bonsai-27B-gguf), [LM Studio's Bonsai 27B conversion](https://huggingface.co/lmstudio-community/Bonsai-27B-GGUF), [Ternary Bonsai 2 9B model card](https://huggingface.co/prism-ml/Ternary-Bonsai-2-9B-gguf), [its file list and `SHA256SUMS`](https://huggingface.co/prism-ml/Ternary-Bonsai-2-9B-gguf/tree/main)
- [Bonsai 1 run guide](https://github.com/PrismML-Eng/Bonsai-demo/blob/main/Bonsai1_README.md)
- [Gemma 4 12B QAT model card](https://huggingface.co/google/gemma-4-12B-it-qat-q4_0-unquantized), [LM Studio's conversion](https://huggingface.co/lmstudio-community/gemma-4-12B-it-QAT-GGUF)
- [llama.cpp](https://github.com/ggml-org/llama.cpp) `v1.0.0`
