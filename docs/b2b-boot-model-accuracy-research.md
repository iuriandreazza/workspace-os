---
title: "B2b research: making the boot model more accurate"
type: research
status: done
created: 2026-11-23
updated: 2026-11-23
tags: [research, boot, bootloader, model, accuracy, calibration, jev, b2, b3]
---

# B2b research: making the boot model more accurate

Input to the owner's decision on ADR 0034 and to the scope of roadmap item B3. It
changes no decision: ADR 0027 is untouched, and ADR 0034 (`proposed`) is not edited.
No ADR is added, because nothing here supersedes a recorded decision (see "Does any of
this need an ADR").

**Merge order.** The B2 report (`docs/b2-boot-model-corpus-and-training.md`) and ADR
0034 live on the branch of PR 45 (`feat/b2-boot-model-corpus-7ef2ff`), not on `main`.
Until PR 45 merges, the two files do not exist on `main` and a link to them is dead.
Every B2 number below is read from those two files on that branch.

**What is evidence here.** Three kinds of statement, kept apart:

- **Measured (B2)**: a number from the B2 report or ADR 0034.
- **Derived**: arithmetic on the budget of ADR 0027, shown in place.
- **Estimate**: my judgement of an expected gain. No experiment was run for this note
  (no real disk was read and no model was trained). Every estimate is a hypothesis for
  "What to measure next", never a finding.

## 1. What B2 measured, restated for this question

| Fact | Number (B2 report) |
|---|---|
| In-distribution test, role / protocol / family / all three | 96.29 / 97.16 / 98.70 / 95.11 % |
| Held-out distributions (Fedora, Rocky, AlmaLinux, openSUSE left out, one fold) | 69.54 % all three; 407 of 8,507 non-kernels called kernels (4.8 %); initrd recall 0.0 % (their initrds are xz, the training set holds only gzip ones: the stated "likely cause", not isolated) |
| Kernels in a format the training set has once (bare gzip, from one AlmaLinux release) | kernel recall on test 76.9 % (342 called "other", 296 called "initrd") |
| Names lie | renamed 87.87 %, 9 false kernels of 6,043; decoys 91.32 %, 131 of 1,023 decoy files called kernels (about 12.8 %) |
| Name alone / bytes alone / both | 95.98 / 94.76 / 95.11 % (ablation on the in-distribution test set only) |
| Float vs int8 | within 0.4 points on every set |
| Trees vs network | 1.2 to 5.2 points worse (role, protocol, family) |
| Run-to-run spread | two runs with the same seeds: 95.11 and 95.03 % all-three, and 1,219 vs 1,343 disks with no strict safety error; a model trained with the group loss at weight 1.0 reached 97.48 % |
| Group | 25.3 % on entry files (random numbering, by construction); pair head P 61.8 % / R 38.4 %; rule "same directory" with true roles P 71.1 % / R 75.8 % |

Four readings of that table drive everything that follows:

1. **The weak spot is coverage of formats and distributions, not capacity.** The zero
   initrd recall and the 76.9 % kernel recall both sit on formats the training set has
   once or never. A bigger network trained on the same corpus would fit them no better.
   QAT has little to gain either: the integer model is within 0.4 points of the float one.
2. **The model does not combine name and bytes well.** Name alone (95.98 %) is at least as
   good as name plus bytes (95.11 %) in distribution. That is consistent with the model
   leaning on names; whether names also cause the held-out failures is **a hypothesis**:
   the ablation was never run on the held-out fold.
3. **Differences smaller than the run-to-run spread are not gains.** The spread is at least
   0.1 point on all-three accuracy and far larger on counts that depend on a few errors
   (1,219 vs 1,343 disks). The held-out view is one fold of four chosen distributions, and
   the test set has 24 distinct kernel files, so one more release moves it.
4. **No assembly design can be measured on the current generator.** ADR 0034 states that
   the generated entry text names a random machine id and version, not the disk's files.
   The pair-head and directory baselines therefore include entry files whose text
   contradicts the layout. That has to be fixed before any option in section 3 is judged.

## 2. Question 1: raising accuracy on unseen distributions

The budget of ADR 0027: weights at most 16 MiB, at most 16 M int8 MACs per candidate, at
most 256 candidates per decision, at most 2 MiB of scratch beyond the weights. Reference
shape (as trained): 4.04 MiB, 4,231,424 MACs. Headroom: about 3.8 times on both.

### 2.1 Options and their cost against the budget

| Option | Weights | MACs per candidate | Scratch | Runtime code | Targets which B2 failure |
|---|---|---|---|---|---|
| **A. Format augmentation** (re-compress real initrd content as xz, zstd, lz4, bzip2, gzip; ship real kernels as raw, gzip and zstd streams; all derived on the host from files already in the corpus) | 0 | 0 | 0 | 0 | initrd recall 0 %; bare-gzip kernels; part of the 4.8 % |
| **B. More real releases** (more distributions and releases, so "unseen" shrinks) | 0 | 0 | 0 | 0 | the whole held-out gap; the 24-kernel interval problem |
| **C. Mixed-name augmentation** (kernel bytes under random and non-kernel names, non-kernel bytes under kernel-looking names, in training, with generator seeds and templates different from the evaluation decoys) | 0 | 0 | 0 | 0 | decoys 12.8 %, renamed 87.9 % |
| **D. Calibrated confidence with abstention** (integer margin between the top two scores; below a threshold the candidate is shown as unverified instead of used) | 0 | 0 (a compare per output) | 0 | a few lines | false kernels on a new distribution, at the price of missed kernels |
| **E. Extra scalar inputs: file size bucket** (directory-entry size, no read needed) | +8K (32 one-hot buckets x 256) | +8K | 0 | a few lines | kernel vs initrd vs dtb vs entry text when bytes are compressed |
| **F. Neighbour names in the window** (names of the other files in the same directory appended, window 512 to 1,024) | 8.0 MiB total | about 8.4 M | 0 | no new loops | kernels called "initrd" (`\<machine>\<version>\linux` beside an `initrd`) |
| **G. Ensemble of three reference-shape models**, disagreement as an abstention signal | 12.1 MiB | 12.7 M | 0 to 2 MiB | the same loops, three times | distribution shift; spread between runs |
| **H. Gated 1-D convolution over the 448 content bytes** (MalConv-like, kernel 8, 64 filters, stride 4) | below 0.1 MiB for the convolution | about 3.6 M (7.2 M at stride 2 gated, not recommended: close to the limit) | a few KiB | a new loop (convolution, max-pool) | field positions that move between layouts; unproven here |
| **I. Self-distillation** (host-side larger teacher; the shipped student keeps the reference shape) | 0 | 0 | 0 | 0 | soft labels on windows with no hand label; unseen formats |
| **J. Window larger than 512, content only** | grows with the window | grows with the window | 0 | 0 | no B2 evidence that the window is limiting |
| **K. Four or more models in an ensemble** | 16.2 MiB | 16.9 M | n/a | n/a | does not fit the weights or the work limit |

Two details of the arithmetic: the 1,024 window is 1,024 x 32 x 256 = 8.39 M weights and
MACs in the first layer; "four models" is 4 x 4.23 M = 16.9 M, which is over the 16 M
limit and over the 16 MiB weights limit at 4 x 4.04 MiB.

### 2.2 Notes on each option

**A. Format augmentation.** It is the only option whose target is a measured, specific
failure with a stated likely cause. The model has never seen an xz initrd; a window that
begins with the xz magic bytes is as new to it as a different distribution. Compressing
the same real cpio content with each compressor produces windows with real framing and
real high-entropy tails, and costs nothing at boot. Risk: a synthetic xz stream may differ
from the one a distribution ships (dictionary size, check type, a prepended early
microcode archive), so the check is on real held-out releases, not on the augmented
copies. The existing augmentation (zeroing a quarter of the windows, replacing the tail of
high-entropy windows by fresh random bytes) was designed to stop memorisation of a
compressed stream, which is the opposite of what the missing-format case needs.

**B. More real data.** Likely the largest single gain, and the same direction as A (A is a
cheap stand-in for B on one axis). Cost is on the host: download size, pins, time. One
open item belongs to ADR 0027, section 6, not to this note: the weights may not be
released until it is answered whether a model trained on headers of other systems'
kernels inherits their licences. More corpus makes that question more, not less,
pressing. B3 trains in its own gate or the owner releases weights; B2's report already
asks for that decision.

**C. Mixed-name augmentation.** The model trained with name dropout (the name zeroed in
15 % of windows) but never with a kernel under a false name or junk under a kernel's
name; those cases exist only in the evaluation sets. Training on them with different
generator seeds and templates keeps the evaluation sets honest (the decoy generator was
already fixed once after the test set was read; its decoys must stay evaluation-only,
with their own templates).

**D. Abstention.** Not an accuracy gain: it trades coverage for precision. The loader's
format check (ADR 0027, section 4) already stops a false kernel from being started, so
abstention matters for the wrong-but-valid boot and for the amount of noise on the menu.
What the literature says: softmax confidence is miscalibrated in general (Guo et al.,
2017, arXiv 1706.04599, temperature scaling as a one-parameter repair), becomes worse
under dataset shift (Ovadia et al., 2019, arXiv 1906.02530), and a selective-classification
risk-coverage curve is the right way to choose a threshold (Geifman and El-Yaniv, 2017,
arXiv 1705.08500). The consequence for this project: **validation (100 %) cannot
calibrate anything**; the temperature or margin must be fitted on a held-out-distributions
fold that is not the one reported. Report both directions: false kernels removed and
real kernels now missed. Whether the margin of this model separates its errors on a new
distribution is **unknown**: B2 measured accuracy, not confidence.

**E and F. Context inputs.** Both change the window contract (`window.py`, "the contract
B3 implements"). ADR 0027, section 2 leaves the exact windows to B2 "within the budget",
so no ADR is needed, but its enumeration "applies no filter by name, size, extension or
content": a size is an input to the model and filters nothing, which the B2b report
should state. The contract must be settled **before B3 starts**, or `\WSOS.MDL`'s format
version must carry it, because B3's loader builds the window. File size and the sibling
list are available from the directory read the enumeration already does. Hypothesis for
F: the Alma and Rocky kernels at `\<machine>\<version>\linux` called initrd might be
fixed by seeing an `initrd` sibling. Not measured.

**G and K. Ensembles.** Three models fit; four do not. Deep ensembles are the standard
way to get usable uncertainty under shift (Lakshminarayanan et al., 2017; **not
re-opened**, the arXiv page could not be fetched in this session, so the ID is
deliberately not given). Cost: three times the inference time, which is 3.9 ms x 3 per
candidate on TCG (derived from ADR 0027), still small. Gain over D alone: not known;
two same-seed runs differed by 0.08 point in distribution, so the spread within a
configuration is small, and disagreement on held-out distributions is what to measure.

**H. Convolution.** Position-invariant features could help when a layout shifts fields,
and the raw-bytes line of work is established (Raff et al., 2017, arXiv 1710.09435,
"Malware Detection by Eating a Whole EXE"). But kernel and initrd headers are
position-specific and the dense layer already reads them; B2's evidence says the failures
are coverage, and trees (a different bias) lost by 1.2 to 5.2 points. It adds a new loop
to the hand-written runtime of about a hundred lines. Low priority.

**I. Distillation.** A larger host-side model (nothing in the budget applies on the host)
as teacher of the budgeted student (Hinton et al., 2015, arXiv 1503.02531). Its value is
soft labels on windows nobody labelled, for instance unlabeled files from additional
releases. It needs a trustworthy teacher; with the same corpus, the teacher shares the
student's blind spots. It is useful after B, not instead of B.

**J. A larger window.** Compressed streams are high entropy past their first bytes, and
magic numbers sit in the first bytes already inside 448. Nothing in B2 says the window
length limits accuracy. Low.

**Other tools considered.** Outlier exposure (Hendrycks et al., 2018, arXiv 1812.04606)
trains the model to be uncertain on out-of-distribution inputs; it is the principled
version of "abstain on unknown formats" and uses the synthetic junk files B2 already
generates, at zero runtime cost: worth testing together with D. Domain-generalisation
benchmarks find plain empirical risk minimisation hard to beat when the model selection
is done honestly (Gulrajani and Lopez-Paz, 2020, arXiv 2007.01434), which is a reason
to prefer data (A, B) over exotic training objectives. Quantization-aware training
(Jacob et al., 2017, arXiv 1712.05877) is not worth the effort at a 0.4-point
float-to-int gap.

## 3. Question 2: the group problem

**Constraint on every learned design.** A learned design must output a **relation** ("this
file belongs to that kernel", or "to none"), never a slot or an index into a numbering.
Anything that outputs a number the disk assigns brings back the disk-relative problem
ADR 0034 identified, however much disk context the model sees.

Budget arithmetic: ADR 0027 states the work limit per candidate. A cost that is paid once
per disk is amortised over its candidates in the table below; that amortised reading is
**my interpretation**, not the text of ADR 0027. The absolute cost is also given, and
the 256-candidate cap bounds it. The throughput used is the measured 1.1 G MAC/s on TCG
and 24 to 29 G MAC/s on HVF (ADR 0027).

| Design | Weights | MACs per disk (256 candidates) | Per candidate, amortised | Scratch | Runtime code |
|---|---|---|---|---|---|
| **1. Code assembly from the model's labels** (ADR 0034) | 0 | 0 | 0 | 0 | a parser and a rule, tested on the host |
| **2. Learned scorer over relational features, code supplies the features** (same directory, shared prefix length, version-suffix match, whether the entry text names the path) | tens of weights, or a small tree set | at most 16 kernels x 240 others x about 150 MACs = 0.6 M; at the worst split of 256 candidates (128 x 128 pairs, 256 -> 64 -> 1) about 268 M | 2 K to about 1 M | pair scores, a few KiB | a small scorer; the features are code |
| **3. Pair head on trunk features** (the B2 baseline) | 128 x 2 -> 64 -> 1 head | as above | as above | | measured: P 61.8 %, R 38.4 %; worse than the directory rule, probably pessimistic |
| **4. Pointer or attention second pass** over the per-file 128-d features (each non-kernel file attends over kernels; argmax; no softmax at inference) (pointer networks, Vinyals et al., 2015, arXiv 1506.03134; permutation-invariant set models, Zaheer et al., 2017, arXiv 1703.06114 and Lee et al., 2018, arXiv 1810.00825) | Q and K projections 2 x 128 x 128 = 32 K | about 17 M (Q, K: 8.4 M; scores 256 x 256 x 128: 8.4 M) | about 66 K | 256 x 128 int8 features (32 KiB) plus scores (256 KiB at int32) | an attention loop and an argmax added to the roughly hundred lines of hand-written Rust that ADR 0027 promised |
| **5. Neighbours in the window** (option F) | as F | as F | about 8.4 M | | helps roles, not the group |
| **6. A deterministic slot** (rank of the path among the kernels) | 0 | 0 | 0 | | still a function of the whole disk; a single candidate cannot know it. Rejected |
| **7. A slot as a hash of the window** | 0 | 0 | 0 | | learnable, but collisions on a disk of several entries; rejected in ADR 0034 |

### 3.1 What each design buys

**1. Code assembly (ADR 0034).** Zero cost against every limit. It also rests on a
fact of ADR 0027 itself: section 3 already requires code to read the entry text to copy
its `options` line verbatim. The `linux`, `initrd` and `devicetree` lines of a Boot
Loader Specification entry name paths exactly and are read by the same parser, so the
objection "a rule beside the model" applies to a few more lines of a parser that must
exist anyway, not to a new component. Where there is no entry text (Alpine and similar),
same directory plus version suffix is the fallback, and its measured quality is the
weak part: P 71.1 %, R 75.8 %, 458 of 1,245 disks with every pair right, on a generator
whose entry text does not name the disk's files. That number says little about BLS
disks, where the text decides, and nothing yet about real disks. Limits: the owner must
read ADR 0009's sentence about a rule-based scanner as being about detection; grub.cfg
and syslinux configs are not BLS and parsing them would be a scanner proper, so they
are not proposed.

**2. Learned scorer over relational features.** B2's pair head failed because it saw two
trunk embeddings and no relation between them; its inputs could not say "same
directory". Giving the scorer the relations the code can compute, and learning only how
to weigh them, keeps "the model decides" in the loop at a cost of a few hundred weights.
It generalises the directory rule rather than replacing it. Gain over design 1: unknown;
it matters only on layouts where the rule is ambiguous (two kernels in one directory).
It needs the corpus fixed first (section 1, item 4).

**4. Pointer or attention second pass.** In principle the strongest, because it learns
which file belongs to which kernel from content (an initrd and its kernel share a
version string; a device tree names a board). The cost fits the budget easily (about
17 M MACs for the whole disk, about 66 K per candidate). The cost that matters is not
MACs: it needs a corpus of **real disk layouts with ground-truth relations**, which the
current generator does not produce, it adds an attention loop and an argmax to the
runtime that ADR 0027 promised would be about a hundred lines, and it can fit the
generator's layouts as the byte-level model fits the generator's junk (the "0 false
kernels" of the headline is against easy negatives). Do it only after design 1 or 2
is measured and found short.

**Recommendation for the group problem.** Design 1 now, with design 2 as the next step
if the measurements in section 6 show the rule short on ambiguous layouts. Design 4 is
the research option, not the B3 option.

## 4. Question 3: TypeSafe's Jev

Sources read on 2026-11-23, none of them primary legal advice. The blog post and the
documentation were summarised by a model reading the page; the legal pages were read the
same way and the key clause was re-read and quoted by a second pass. **Read the pages
themselves before relying on this section.** Dates on the pages disagree (the blog
shows two dates; the master customer agreement says "Last updated: Sep 23, 2026" and a
publication time of Nov 16, 2026), so which text is in force is **not known**.

- Blog: https://typesafe.ai/blog/introducing-system-one-models-and-jev
- Documentation: https://docs.typesafe.ai/models.md, https://docs.typesafe.ai/confidence.md,
  https://docs.typesafe.ai/introduction/machine-learning-primer.md
- Legal: https://typesafe.ai/legal/mca (Master Customer Agreement), https://typesafe.ai/privacy,
  https://typesafe.ai/terms

### 4.1 What it is

A hosted model that returns typed decisions with probabilities instead of text
("unstructured state in, typed probabilistic decisions out", blog). Trained with
"Reinforcement learning for calibrated decisions" (primer). Choices up to a cardinality
of 255; above that, "scoring independently then making an explicit choice" (blog).
Price listed at $0.042 per million input tokens, output free (blog and models page);
latency "70ms-500ms" end to end from the vendor's laptops (blog). The footer of every
legal page still says "early access". The service definition in the MCA is limited to
the vendor's hosted web interface and hosted API; **no page read mentions weights or
self-hosting** (not found, so not verified either way).

### 4.2 Can it run in the loader?

No. It is a network service, the boot model runs offline in UEFI (ADR 0027 and ADR 0009),
and no weights are offered. This was the starting answer and it holds.

### 4.3 Do its ideas transfer?

| Idea | Already in the design? | What to take |
|---|---|---|
| Typed outputs, closed sets | Yes: ADR 0027, section 3 (exhaustive `match`, a vector that names no class is rejected) | Nothing new |
| Calibrated probabilities | No: the model emits raw int scores | The goal (option D), not the method. The vendor gives a definition ("outcomes assigned 0.2 should occur about 20 % of the time", primer) and a confidence formula, (p_max - 1/n) / (1 - 1/n) (confidence page), but **publishes no calibration metric** (no ECE, Brier score or reliability curve in the pages read). The claim is asserted, not shown. Its own page says the confidence is "one reasonable way to summarize a distribution" |
| Parallel scoring of up to 255 candidates, two-stage choice above | Partly: ADR 0027 scores every candidate (cap 256) independently; the choice among candidates is made in code. The vendor's two-stage scheme (score independently, then choose) is the shape of designs 2 and 4 above | A confirmation of the shape, not new evidence |
| Workflow evals | Partly: B2 has an evaluation harness with ground truth from the generator | The discipline of a fixed set of end-to-end workflows rerun per model version. Their reference answers are a consensus of other models, which the post admits "biases answers"; ours are generator labels, with the weakness already in B2's report |

### 4.4 Host-side teacher or baseline?

**Teacher: no.** The MCA, section 2.3(b), prohibits the customer from using "the Services
or any Output ... to perform model distillation", which the page describes as including
training a model to imitate the output of the Services or developing a similar or
competing product (the quoted words are the exact ones; the rest is a paraphrase). Section
4.2 says the vendor "disclaims ownership of Output" and assigns to the customer
what rights it has, but the two clauses are read together: owning the output does not
license the use that 2.3(b) forbids. Labels from Jev used to train the boot model fall
inside the prohibition on any plain reading, so they are out, whatever their other merits.
A teacher, if wanted, is a model **of ours** (option I). This is not legal advice; a
signed agreement might differ from the public text, which is the only text read.

**Baseline: possible but low value, and conditional.** Scoring a sample of synthetic
windows (hex dumps of the generator's headers, never a real disk) and comparing with the
trained model would show whether a frontier model finds the task harder or easier. The
prohibition above is about training; comparison is not named. Conditions before any use:
the signed terms of the account, not the public pages (the website terms say a separate
agreement governs the services); no real disk contents ever, as the task says and as
section 4.1 and 4.3 of the MCA suggest (the vendor may process customer data "in
perpetuity" to derive Telemetry and may use Telemetry "without restriction"; the pages
also say retention periods are not fixed and the service is hosted in the United States);
and a cost the owner accepts. Expected value: low. A frontier model reading a hex dump
tells us little about a 4 MiB integer classifier, and the project's own held-out folds
are the stronger evidence.

**Not verified.** Whether any enterprise or on-premises arrangement exists; what zero data
retention means in practice (the form was not submitted); the actual calibration of the
outputs (no data published); the date of the MCA in force.

## 5. Ranking

Estimates, not measurements. "Gain" is the expected movement on the failures of section 1
**if the hypothesis in the right-hand column holds**; "risk" includes the risk that the
gain is smaller than the run-to-run spread of section 1.

| Rank | Option | Expected gain | Budget cost | Risk | Why here |
|---:|---|---|---|---|---|
| 0 | Fix the generator (entry text naming the disk's files), several held-out folds, several seeds | enables every measurement below | none | low | without it no gain below can be told from noise or an artifact of the generator |
| 1 | **A. Format augmentation** (compressors; kernel container formats) | high on initrd recall and bare-gzip kernels, the two largest measured failures | none | low to medium (synthetic framing) | direct answer to a measured failure with a stated cause; free at boot |
| 2 | **1. Code assembly** (ADR 0034, BLS lines plus the fallback rule) | high on assembly; makes the group problem moot | none | medium (owner's reading of ADR 0009; fallback rule not validated) | the only design with zero cost whose structure is understood |
| 3 | **B. More real releases** | high on the held-out gap | none at boot; host effort; licence question open | medium | the cleanest fix and the slowest; A buys time |
| 4 | **D. Abstention** (margin, fitted on a separate held-out fold), with **outlier exposure** | no accuracy gain; a lower rate of false kernels at a stated loss of recall | none | medium (margins may not separate errors under shift) | cheap safety net while A and B take effect |
| 5 | **C. Mixed-name augmentation** | medium on decoys and renamed | none | low | evaluation generators must stay separate |
| 6 | **E. File-size input** | low to medium on compressed or ambiguous files | negligible | low; changes the window contract | decide before B3 or version the file |
| 7 | **2. Learned scorer over relational features** | unknown; only where the directory rule is ambiguous | negligible | low | next step if design 1's rule is short |
| 8 | **F. Neighbour names** | unknown; might fix kernels called initrd | doubles the window (8.4 M MACs, 8 MiB) | medium; changes the window contract | measure once A and B are in |
| 9 | **G. Ensemble of three** | unknown; mainly an uncertainty signal | 3 x (12.7 M MACs, 12.1 MiB) | low | after D shows whether a single model's margin works |
| 10 | **I. Self-distillation** | low to medium, after B | none at boot | medium | needs a teacher not blind to the student's gaps |
| 11 | **4. Pointer or attention second pass** | possibly high on odd layouts | 17 M MACs per disk, a new loop in the runtime | high (needs real layouts with relations; overfits the generator) | research, not B3 |
| 12 | H, J, QAT, a tuned tree ensemble | low | | | no B2 evidence points there |
| not used | Jev as teacher | forbidden by MCA 2.3(b) as read | | | the project would breach the terms |

## 6. What B3 should do, and what to measure next

### What B3 should do

1. **Not wait for these improvements.** B3 is the loader, the inference loop in Rust,
   the stored decisions and the fallback; its accuracy comes from whichever
   `\WSOS.MDL` it is given. Its test must show that the Rust forward pass reproduces
   `mdl.forward` on a file written by `mdl.write` (B2's report asks for this).
2. **If ADR 0034 is accepted:** implement assembly by code: the entry-text parser that
   extracts `options` (ADR 0027, section 3) also reads `linux`, `initrd` and
   `devicetree`; the fallback is the same directory plus version suffix; the loader's
   checks run on the result. Tests are on the host.
   If ADR 0034 is rejected, B3 has no way to build an entry from the model's output and
   cannot proceed past the labels.
3. **Settle the window contract and the file header before B3 writes the loader.** If
   options E or F are wanted, they change the window; if D is wanted, a threshold has
   to live somewhere. The cheapest moment is now, while no weights are released. A
   version field in the `\WSOS.MDL` header (the file already starts with a magic, a
   format version and the shape) can carry both.
4. **Treat a low-margin or out-of-distribution label as "unverified"** on the menu (D),
   but only once the margin is shown to separate errors on a held-out fold.
5. **Do not use Jev.** If the owner wants a comparison, apply the conditions of 4.4.

### What to measure next (proposed item "B2b")

A host-only item, one session and one pull request, that comes **before** B3 trains
anything:

1. Fix the generator so entry text names the disk's real files; rerun the group
   baselines (directory rule, pair head) and report disks with every pair right.
2. Leave-distributions-out in **several folds** (each distribution family out once, not
   four at once), with **at least three seeds** per configuration, reporting the
   spread. A change counts as a gain only if it is larger than that spread.
3. Option A against that protocol: initrd recall (was 0.0 %), kernel recall (was
   55.9 % on the fold and 76.9 % on test), false kernels (was 4.8 %).
4. The name/bytes ablation **on the held-out folds**, to turn reading 2 of section 1 from
   hypothesis into finding. Then option C.
5. Risk-coverage curve and expected calibration error of the margin, on a fold that
   is not used for the final report, for the raw scores, with temperature scaling and
   with outlier exposure. Report the false kernels removed and the real kernels lost at
   each threshold.
6. Option E (size) and F (neighbour names) as single-factor ablations; G only if D
   shows a single model's margin is unreliable.
7. Statistics on **distinct windows and per release**, not per candidate (B2's report
   says the candidate-level intervals are too narrow).
8. No real disk. A real ESP is read only with the owner's explicit say-so, and its
   contents go into no external service.

## Does any of this need an ADR?

No. Nothing here supersedes a recorded decision.

- ADR 0034 stays `proposed`; this note is evidence for it, not a substitute.
- Options E and F change the window, which ADR 0027, section 2 leaves to B2.
- Abstention is a use of the typed decision of ADR 0027 and the fallback of ADR 0009.
- If the owner reads "applies no filter by size" in ADR 0027 as forbidding a size
  input, that reading needs an amendment before option E; I read it as describing
  candidate selection, not model inputs.
- Training on augmented or distilled data touches the open licence question of
  ADR 0027, section 6 no more than the corpus already does.

## What could not be verified

- No experiment was run: every "expected gain" is an estimate.
- Real disks: none read.
- Arithmetic of section 2.1 and 3 uses the shapes of ADR 0027 and the reference model;
  no inference code for options H, 2 or 4 exists, so their runtime cost beyond MACs is an
  estimate.
- Deep ensembles (Lakshminarayanan et al., 2017): the arXiv page could not be opened.
  The other papers were checked for title, first author, year and identifier on their
  arXiv pages (abstract only; their bodies were not read).
- Magika (Fratantonio et al., 2024, arXiv 2409.13768) reports an average F1 of about
  99 % over more than a hundred content types with a 1 MB model (abstract). Whether it
  has kernel or initrd classes is not in the abstract and not verified; ADR 0027,
  section 6 already rules out linking it for licence reasons.
- Jev: weights or self-hosting, zero data retention terms, the calibration of its
  probabilities, which version of the agreement is in force, and the exact scope of
  section 2.3(b) in a signed contract. All its pages were read through a summarising
  fetcher, and the early-access wording on the legal pages conflicts with a model
  page that lists a production version.
