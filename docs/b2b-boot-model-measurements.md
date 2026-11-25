---
title: "B2b: measurements on the boot model"
type: report
status: done
created: 2026-11-23
updated: 2026-11-24
tags: [boot, bootloader, model, measurement, accuracy, calibration, b2b]
---

# B2b: measurements on the boot model

Roadmap item B2b: host-only measurements on the model of ADR 0027, before B3 trains or loads anything. The
plan is section 6 of the research note of PR 47 (`docs/b2b-boot-model-accuracy-research.md` on the branch
`docs/boot-model-accuracy-research`; it was not merged when this was written, so it is not copied here). That note
ranked options by expected gain and measured none. This report measures them.

**What this report decides:** nothing. ADR 0027 is not edited and ADR 0034 stays `proposed`; the numbers are
evidence for the owner's decision, not a substitute for it. No real disk was read, nothing was sent to an
external service, no weights are committed (ADR 0027, section 6).

## Result

102 trainings of the B2 network (7 folds, 8 configurations, three seeds each, on the folds listed per step). Gains are claimed only
where the three seeds' ranges do not overlap and the means differ by a point; **everything else is within the
spread, and the spread is large** (initrd recall on `debian`: 6.9 to 43.1 % for the same data).

| Step | Question | Answer, measured |
|---|---|---|
| 1 | Does the group assembly of ADR 0034 hold on a generator whose text names the disk's files? | With the true roles: ADR 0034's combination P 95.6 % / R 95.6 %, **78.9 % of 1,244 disks entirely right** (directory rule alone 21.9 %, directory + version suffix 35.3 %, entry-text lines 29.4 %; pair head 153 to 295 disks). With the **model's roles** (in distribution): `base` R 71.7 ± 11.4 %, 51.6 ± 10.5 % of disks right; with the roles of the augmented models R 94.6 ± 1.7 %, 77.5 ± 2.4 %. The text rule is exact by construction of the generator |
| 2 | How big is the noise? | Large: initrd recall 0.0 % (Alpine), 4.4 % (B2 fold), 25.5 % (Debian), 91.6 % (EL) across folds and 6.9 to 43.1 % across seeds on one; B2's 0.0 % initrd recall on its fold is 4.4 ± 4.2 % over three seeds; false kernels 4.8 % (B2, candidates) are 5.3 ± 5.2 % per candidate, and per distinct file 0.2 ± 0.2 % over all files of the evaluation disks or 0.5 ± 0.4 % over the held-out family's files; on `debian` 10.1 ± 1.7 % of the held-out family's non-kernel files |
| 3 | Option A (formats) | **Gain** where the format is missing from training: initrd recall on the B2 fold 4.4 ± 4.2 → 36.9 ± 17.1 %; kernel recall in distribution 86.7 → 98.8 % (the test releases' bare gzip kernels: a format added after reading which failed, so tuned to this split, not out of sample). **No gain** pooled over the five families (kernel 69.7 → 70.6 %), none on Alpine's gzip initrds (0 → 2 %), none shown on the five bare-gzip kernels of `el` per release (ranges overlap, one seed in three finds them), and **lower means for the PE kernels of Fedora** (88.4 → 76.1 %, per-seed ranges overlap); false kernels of the held-out family not reduced (`debian` 10.1 → 16.0 %, `opensuse` 0.0 → 1.7 %) |
| 4a | Name or bytes? | On unseen families **bytes alone find kernels better than name and bytes** (pooled 69.7 → 89.4 %) and find almost no initrds (35.7 → 7.4 %); name alone finds few kernels (19.7 %) |
| 4b | Option C (mixed names) | Small gain on decoys in distribution (all-three 95.5 → 97.2 %, 0.1 point beyond the ranges, with near-twin decoy names in training); **no gain and a loss on `debian`: false kernels 10.1 → 47.2 % of the held-out family's non-kernel files** |
| 5 | Is the margin usable? | Raw scores saturate (probability indistinguishable from 1, so raw ECE 17.6 to 18.8 % is just the error rate); a temperature fitted on `opensuse` changes it to 14.6, 25.1 and 14.9 % on three other families, ranges overlapping the raw ones: **not shown to help or to carry over**. A threshold on one model's margin removes the false kernels only by losing 34 to 47 % of the real ones, and the thresholds of one quantile differ by 1.4 to 2.4 times between seeds (at the 10, 25 and 50 % quantiles). **Outlier exposure did not help** (`debian` false kernels 10.1 → 19.7 %, a loss). Three models' disagreement flags 63 % and 100 % of the false kernels of two folds at 13 % and 10.5 % of their real kernels, but 23.5 % (Alpine) and 100 % (OpenBSD) of the real kernels elsewhere |
| 6 | Options E and F | **E** (size): better means, within the spread (`debian` initrd 25.5 → 43.0 %). **F** (neighbour names): no gain on `el` (kernel 54.4 → 49.9 %). **G**: no gain in accuracy |

**What did not improve:** kernel recall on unseen families (pooled over five families A is within the spread; OE over
three; C, E and F, run on one to three folds, show no gain beyond it; the one gain is bytes alone, which finds kernels
but not initrds), the false kernels of the held-out family (the one reduction by the rule, name alone on `debian`, 10.1 → 2.7 %, is a network that calls almost nothing a kernel; C and outlier exposure raised those of `debian` by 37 and 10 points, A and the bytes-only network by 6 and 17,
within the spread), the margin as a safety net, and Alpine's gzip initrds. **What B2b changes for B3:** assembly by code (ADR 0034, still
`proposed`) is much better than any learned group head measured, but is only as good as the roles the model gives; the
roles on a family the corpus lacks are the weak point, and the one lever that targeted a measured cause (A) fixed that cause only.


## Method

### What was measured, and on what

Every number below comes from the B2 network (512 positions, embedding 32, dense 256 and 128, 34 int8 scores,
trained with the B2 recipe: 12 epochs, batch 128, Adam 1e-3 with cosine decay, the same two augmentations) run in
the integer arithmetic the bootloader will use. **Nothing in `window.py` or in the `\WSOS.MDL` format was changed**:
options E and F build a longer window inside the measurement code only (`boot-model/contexts.py`).

- **A run** is one training on one *fold* with one *configuration* and one *seed*: `boot-model/b2b.py`. 4,000
  generated training disks per run, as in B2 (the corrected generator makes about 42,000 to 51,000 candidates of them:
  more files per disk carry entry text and two kernels).
- **Folds** leave a family of distributions out of training whole, and evaluate on every release of that family:
  `alpine`, `debian`, `el` (Fedora, AlmaLinux and Rocky **together**: they share a layout, so leaving out one would
  leak the others), `opensuse`, `openbsd`; `b2four` is the fold of the B2 report (Fedora, AlmaLinux, Rocky and
  openSUSE out together); `indist` is B2's split by release. Our own kernels and the QEMU trees are always in
  training. Training uses the `train` split of the remaining families only, as the B2 fold did.
- **Seeds**: 0, 1 and 2 for every configuration reported. A seed changes the training disks and junk, the initial
  weights and the batch order. The **evaluation disks do not depend on the seed or the configuration**
  (800 generated disks per fold, 1,500 for `indist`), so runs are paired. A run that reached the cache before a
  fix to the generator is listed in "What could not be verified".
- **Unit of counting.** B2 counted candidates, and a kernel file seen on many generated disks under many paths
  counted many times (its test set held 24 distinct kernel files). Here a **file** is one distinct asset (same
  system, release and bytes); its rate is the share of its appearances that were right, and a fold's figure is the
  **mean of the file rates**. Candidate-level figures are given where they compare with B2, and labelled. Counts of
  distinct files are in the tables: **one distinct file is 9 to 100 points of recall** in most folds (the `el` fold
  holds 11 kernels and 11 initrds, `debian` 4 and 4, `openbsd` 3 kernels).
- **Two denominators.** The fold-level rates (`measure.py`) count every file of the evaluation disks, as B2 did,
  which includes negatives that are in training anyway (the Alpine rootfs, the QEMU trees, generated junk): on
  `debian` 1,779 non-kernel files, of which 1,060 are Debian's. They understate errors on unseen files (all-three 95
  to 99 % there is mostly easy negatives). Where it matters the tables are repeated **for the held-out family's files
  only** (`b2b_report.py heldout`), and the text says which one it quotes; the `el`, `debian` and `alpine` kernel and
  initrd recalls are the same under both, since every kernel and initrd of the evaluation disks is the family's.
- **Gain criterion, fixed before any run was read**: a configuration beats the baseline on a metric only if the
  three seeds' ranges (min to max) do not overlap, in the favourable direction. After the first tables showed
  differences of 0.0 against 0.2 points flagged as a loss by that rule, a materiality floor was added: the means
  must also differ by at least one point. The tables print the mean, the standard deviation and the range of the
  three seeds, and mark **gain** or **loss** only under that rule; everything else is within the spread.
- **Three seeds is a small sample.** A range of three values is a crude spread, not a confidence interval; "within
  spread" means "not shown", not "shown equal".

### What changed in the generator (step 1)

B2's entry text named a random machine id and version, not the disk's files (ADR 0034). Now (`layouts.py`,
`synthetic.py`):

- the `linux`, `initrd` and `devicetree` lines of an entry's text are the paths, with `/`, of the files of that
  entry on that disk; the text sits in `\loader\entries`, away from the kernel, as on a real volume;
- Enterprise Linux disks carry entry text with both layouts (`\<machine>\<version>\linux` and `\boot\vmlinuz-<v>`),
  Debian and openSUSE disks in 25 % of cases; Alpine and OpenBSD never (the fallback rule is the only one there);
- a system can hold **two kernel versions in one directory** (30 % of Debian, Enterprise Linux and openSUSE
  disks); on Alpine two flavours now share a directory when the layout style is one that has one;
- every candidate carries the whole file it was made from (`text`), so a rule reads the full entry text and a
  junk file that the model calls an entry is parsed as the junk it is, not skipped;
- two defects of the generated junk found by the review of this change were fixed: zip files carried the current
  time in their header (so no two runs were byte-identical), and junk files were at most 4 KiB while the real
  kernels they are told apart from are megabytes (which would have made a size input look like a detector).
  Generated files now take a size drawn log-uniformly from 512 B to 256 MiB, independent of the class.

The group rules (`boot-model/assembly.py`, tested in the gate) work on `(path, role, text)` per file, where the role
is the true one or the model's. A rule returns the `(kernel or loader, initrd or device tree or entry text)` pairs
it asserts, compared with the pairs that really belong to one entry.

## Step 1: the group baselines on the corrected generator

Test disks of the `indist` split (1,500 disks), the same releases as B2's test set. "True roles" gives each rule the
roles of the generator (the rule's ceiling); "predicted roles" gives it the roles of the models of the protocol below:
`base` (the B2 recipe) and `A` (with format augmentation, which repairs the one format hole of this split). Disks "with pairs" are disks that hold at least one kernel or loader and a
companion of the same entry. A disk counts as right only if the rule asserts exactly the true set of pairs.

B2's figures, on the old generator, for the same two baselines: the directory rule P 71.1 % / R 75.8 %, 458 of
1,245 disks with every pair right; the pair head P 61.8 % / R 38.4 %, 231 of 1,245. They are not comparable to the
table below: the generator, the share of disks with pairs and the definition of a disk "with pairs" changed (B2 counted
every disk that had a kernel-like file and a companion; the table counts disks with a true pair).

### Group rules, true roles (the ceiling of each rule; no model)

| Disks | Rule | Pair precision | Pair recall | Disks with pairs | Disks with every pair right | Spurious on pairless | Pairs missed, by companion | Pairs wrong, by companion |
|---|---|---:|---:|---:|---:|---:|---|---|
| all disks (1500) | same directory | 59.4 % | 67.8 % | 1244 | 272 (21.9 %) | 0 | {'entry': 889, 'dtb': 152} | {'initrd': 1493} |
| all disks (1500) | directory + version suffix | 93.9 % | 67.8 % | 1244 | 439 (35.3 %) | 0 | {'entry': 889, 'dtb': 152} | {'initrd': 143} |
| all disks (1500) | entry text lines | 100.0 % | 55.3 % | 1244 | 366 (29.4 %) | 0 | {'initrd': 1299, 'dtb': 143} | {} |
| all disks (1500) | ADR 0034 (entry text, else directory + suffix) | 95.6 % | 95.6 % | 1244 | 981 (78.9 %) | 0 | {'dtb': 143} | {'initrd': 143} |
| disks with entry text (621) | same directory | 64.2 % | 56.8 % | 621 | 0 (0.0 %) | 0 | {'entry': 889, 'dtb': 55} | {'initrd': 692} |
| disks with entry text (621) | directory + version suffix | 97.0 % | 56.8 % | 621 | 0 (0.0 %) | 0 | {'entry': 889, 'dtb': 55} | {'initrd': 39} |
| disks with entry text (621) | entry text lines | 100.0 % | 81.8 % | 621 | 366 (58.9 %) | 0 | {'initrd': 351, 'dtb': 46} | {} |
| disks with entry text (621) | ADR 0034 (entry text, else directory + suffix) | 98.2 % | 97.9 % | 621 | 542 (87.3 %) | 0 | {'dtb': 46} | {'initrd': 39} |
| disks with two kernels in one directory (467) | same directory | 44.0 % | 71.7 % | 382 | 26 (6.8 %) | 0 | {'entry': 289, 'dtb': 68} | {'initrd': 1151} |
| disks with two kernels in one directory (467) | directory + version suffix | 95.7 % | 71.7 % | 382 | 151 (39.5 %) | 0 | {'entry': 289, 'dtb': 68} | {'initrd': 41} |
| disks with two kernels in one directory (467) | entry text lines | 100.0 % | 46.3 % | 382 | 87 (22.8 %) | 0 | {'initrd': 615, 'dtb': 62} | {} |
| disks with two kernels in one directory (467) | ADR 0034 (entry text, else directory + suffix) | 96.7 % | 95.1 % | 382 | 298 (78.0 %) | 0 | {'dtb': 62} | {'initrd': 41} |

### Group rules, roles predicted by the model (3 seeds)

| Roles from | Disks | Rule | Pair precision | Pair recall | Disks with every pair right (of disks with pairs) |
|---|---|---|---|---|---|
| `base` | all disks | same directory | 54.9 ± 3.1 (51.8–58.0) | 50.2 ± 6.7 (44.1–57.3) | 15.2 ± 1.1 (14.1–16.3) |
| `base` | all disks | directory + version suffix | 90.9 ± 1.4 (89.7–92.4) | 50.2 ± 6.7 (44.1–57.3) | 23.4 ± 2.5 (21.0–25.9) |
| `base` | all disks | entry text lines | 100.0 ± 0.0 (100.0–100.0) | 42.6 ± 9.4 (34.4–52.9) | 21.8 ± 5.2 (17.2–27.5) |
| `base` | all disks | ADR 0034 (entry text, else directory + suffix) | 93.4 ± 1.2 (92.4–94.7) | 71.7 ± 11.4 (61.5–83.9) | 51.6 ± 10.5 (42.3–62.9) |
| `base` | disks with entry text | same directory | 60.0 ± 3.9 (56.2–64.0) | 42.8 ± 7.6 (36.0–51.0) | 0.0 ± 0.0 (0.0–0.0) |
| `base` | disks with entry text | directory + version suffix | 95.3 ± 0.9 (94.5–96.2) | 42.8 ± 7.6 (36.0–51.0) | 0.0 ± 0.0 (0.0–0.0) |
| `base` | disks with entry text | entry text lines | 100.0 ± 0.0 (100.0–100.0) | 63.0 ± 13.9 (50.9–78.2) | 43.6 ± 10.5 (34.5–55.1) |
| `base` | disks with entry text | ADR 0034 (entry text, else directory + suffix) | 97.2 ± 0.6 (96.7–97.8) | 74.5 ± 14.6 (61.6–90.3) | 56.5 ± 16.1 (42.7–74.2) |
| `base` | disks with two kernels in one directory | same directory | 39.7 ± 2.7 (36.9–42.1) | 52.3 ± 6.8 (45.6–59.2) | 5.1 ± 0.8 (4.5–6.0) |
| `base` | disks with two kernels in one directory | directory + version suffix | 92.1 ± 1.5 (90.8–93.8) | 52.3 ± 6.8 (45.6–59.2) | 25.6 ± 4.3 (21.2–29.8) |
| `base` | disks with two kernels in one directory | entry text lines | 100.0 ± 0.0 (100.0–100.0) | 35.8 ± 6.6 (29.3–42.5) | 15.5 ± 3.4 (12.3–19.1) |
| `base` | disks with two kernels in one directory | ADR 0034 (entry text, else directory + suffix) | 94.0 ± 1.3 (92.9–95.4) | 70.4 ± 10.1 (60.5–80.7) | 48.0 ± 10.2 (38.0–58.4) |
| `A` | all disks | same directory | 59.0 ± 0.8 (58.1–59.4) | 66.8 ± 1.7 (64.8–67.8) | 21.2 ± 1.2 (19.9–21.9) |
| `A` | all disks | directory + version suffix | 93.6 ± 0.5 (93.1–93.9) | 66.8 ± 1.7 (64.8–67.8) | 34.3 ± 1.7 (32.4–35.3) |
| `A` | all disks | entry text lines | 100.0 ± 0.0 (100.0–100.0) | 55.3 ± 0.0 (55.3–55.3) | 29.4 ± 0.0 (29.4–29.4) |
| `A` | all disks | ADR 0034 (entry text, else directory + suffix) | 95.4 ± 0.3 (95.0–95.6) | 94.6 ± 1.7 (92.6–95.6) | 77.5 ± 2.4 (74.7–78.9) |
| `A` | disks with entry text | same directory | 63.9 ± 0.6 (63.2–64.2) | 56.4 ± 0.7 (55.6–56.8) | 0.0 ± 0.0 (0.0–0.0) |
| `A` | disks with entry text | directory + version suffix | 96.9 ± 0.1 (96.7–97.0) | 56.4 ± 0.7 (55.6–56.8) | 0.0 ± 0.0 (0.0–0.0) |
| `A` | disks with entry text | entry text lines | 100.0 ± 0.0 (100.0–100.0) | 81.8 ± 0.0 (81.8–81.8) | 58.9 ± 0.0 (58.9–58.9) |
| `A` | disks with entry text | ADR 0034 (entry text, else directory + suffix) | 98.2 ± 0.1 (98.1–98.2) | 97.5 ± 0.7 (96.7–97.9) | 86.4 ± 1.5 (84.7–87.3) |
| `A` | disks with two kernels in one directory | same directory | 43.5 ± 0.8 (42.5–44.0) | 70.5 ± 2.0 (68.2–71.7) | 6.8 ± 0.0 (6.8–6.8) |
| `A` | disks with two kernels in one directory | directory + version suffix | 95.2 ± 0.7 (94.4–95.7) | 70.5 ± 2.0 (68.2–71.7) | 38.7 ± 1.4 (37.2–39.5) |
| `A` | disks with two kernels in one directory | entry text lines | 100.0 ± 0.0 (100.0–100.0) | 46.3 ± 0.0 (46.3–46.3) | 22.8 ± 0.0 (22.8–22.8) |
| `A` | disks with two kernels in one directory | ADR 0034 (entry text, else directory + suffix) | 96.4 ± 0.5 (95.8–96.7) | 93.9 ± 2.0 (91.6–95.1) | 76.5 ± 2.6 (73.6–78.0) |

### Pair head (logistic regression on the trunk's 128 features), true roles, per seed

| Seed | Pair precision | Pair recall | Disks with candidate pairs (B2's definition: every kernel-like file with every companion, true or not) | Disks with every pair right |
|---|---:|---:|---:|---:|
| 0 | 58.7 % | 25.1 % | 1265 | 153 |
| 1 | 53.6 % | 47.1 % | 1265 | 295 |
| 2 | 51.9 % | 28.7 % | 1265 | 192 |

Reading, with the limits that each carries:

- **The BLS-line rule is exact where there is text and says nothing elsewhere.** Precision is 100 % by construction:
  the generator writes the real paths of files that exist, never stale, relative or dangling ones, and never in a
  grub-style syntax. On a real disk a text can name a path on another volume (`/boot` on its own partition), so
  the ceiling is the generator's, not an estimate for real disks. Its recall (55 % of all pairs, 82 % on disks that
  carry text) is the share of pairs it can name.
- **The directory rule alone is poor, and the version suffix is what rescues it.** Same directory has precision
  59 % (44 % where two kernels share a directory): every initrd in the directory is paired with every kernel in it.
  Adding the suffix makes it 94 % (96 % with two kernels in a directory) at the same recall.
  Both rules miss every entry text (it lives in `\loader\entries`) and the device trees that sit in another
  directory (Debian puts them in `\usr\lib\linux-image-<v>`, Alpine in `\boot\dtbs-<flavour>`).
- **The combination ADR 0034 proposes** (text where a kernel is named by one, the directory-and-suffix fallback for
  the rest) reaches P 95.6 % / R 95.6 % with true roles and 78.9 % of the 1,244 disks entirely right; 87.3 % where
  there is text. The failures are of two kinds. The fallback cannot reach a device tree in another directory (143
  pairs: 110 Alpine files (69 in `\boot\dtbs-<flavour>`, 41 in `\dtbs-<flavour>`) and 33 Debian ones in `\usr\lib\linux-image-<v>`). And it asserts 143
  wrong pairs with an initrd: 113 pair a **loader** with the companions of its directory (on a Debian installer disk
  `grubaa64.efi` sits beside `linux` and `initrd.gz`), and 30 pair a true **kernel** of one system (OpenBSD's `bsd`,
  `bsd.mp`, `bsd.rd`; our `kernel.elf`) with another system's `\INITRD` or `\boot\initrd` in the same directory. The first kind is a
  property of treating a loader as kernel-like, as B2's pair experiment did (excluding loaders was not tried); the
  second, two systems sharing a directory, no directory-based rule can see.
- **With the roles the model predicts, `base` is much worse**: pair recall drops from 95.6 % to 71.7 ± 11.4 % and the
  disks entirely right from 78.9 % to 51.6 ± 10.5 % (range 42.3–62.9). Most of that is one format hole of this split: with
  the roles of the `A` models (augmented formats; role accuracy 99.5–100 %) the same rule gets recall 94.6 ± 1.7 % and
  **77.5 ± 2.4 %** of the disks right (ranges 92.6–95.6 and 74.7–78.9: apart from `base` by the report's rule). This is the
  in-distribution split, not a held-out family. **Assembly is only as good as the labelling**: on a family
  the model labels badly (steps 2 to 3) it would inherit those errors, which was not measured.
- **The pair head is no baseline worth keeping**: P 52 to 59 %, R 25 to 47 %, 153 to 295 disks right, with the
  seed spread larger than the gap to its B2 figure (the logistic regression did not converge in 2,000 iterations, as
  in B2). With true roles every rule above beats it on precision and on recall.
- Where two kernels share a directory (467 of the 1,500 disks) the suffix rule keeps its precision (95.7 %) and is
  the difference between 6.8 % of disks right (directory only) and 39.5 % (suffix); with text on top, 78.0 %.

What step 1 does not show: any figure about a real disk, a grub or syslinux configuration (ADR 0034 does not
propose parsing them), or whether `devicetree` lines of real entries use the paths the generator writes.

## Step 2: leave-distributions-out, several folds, three seeds

The baseline (`base`: the B2 recipe on the corrected generator), file level, mean ± standard deviation (min–max) of
three seeds. The `b2four` rows are the fold of the B2 report; `indist` is B2's split by release.

| Fold | Metric (files) | base |
|---|---|---|
| alpine | kernel_recall | 66.4 ± 8.9 (56.4–73.7) |
| alpine | initrd_recall | 0.0 ± 0.0 (0.0–0.0) |
| alpine | false_kernel | 0.0 ± 0.0 (0.0–0.0) |
| alpine | all_three | 97.2 ± 0.0 (97.2–97.2) |
| debian | kernel_recall | 94.5 ± 6.8 (86.9–100.0) |
| debian | initrd_recall | 25.5 ± 18.1 (6.9–43.1) |
| debian | false_kernel | 6.0 ± 1.0 (5.1–7.1) |
| debian | all_three | 82.7 ± 3.3 (80.3–86.5) |
| el | kernel_recall | 54.4 ± 12.4 (44.7–68.4) |
| el | initrd_recall | 91.6 ± 14.4 (75.0–100.0) |
| el | false_kernel | 0.2 ± 0.2 (0.0–0.4) |
| el | all_three | 99.0 ± 0.2 (98.8–99.2) |
| opensuse | kernel_recall | 100.0 ± 0.0 (100.0–100.0) |
| opensuse | initrd_recall | 25.7 ± 14.4 (13.3–41.6) |
| opensuse | false_kernel | 0.0 ± 0.0 (0.0–0.0) |
| opensuse | all_three | 99.6 ± 0.1 (99.6–99.7) |
| openbsd | kernel_recall | 33.3 ± 33.3 (0.0–66.7) |
| openbsd | initrd_recall | — |
| openbsd | false_kernel | 0.0 ± 0.0 (0.0–0.0) |
| openbsd | all_three | 97.7 ± 0.0 (97.7–97.7) |
| b2four | kernel_recall | 67.7 ± 4.5 (64.8–72.8) |
| b2four | initrd_recall | 4.4 ± 4.2 (0.8–9.1) |
| b2four | false_kernel | 0.2 ± 0.2 (0.0–0.4) |
| b2four | all_three | 97.9 ± 0.2 (97.7–98.1) |
| indist | kernel_recall | 86.7 ± 3.4 (83.3–90.0) |
| indist | initrd_recall | 100.0 ± 0.0 (100.0–100.0) |
| indist | false_kernel | 0.0 ± 0.0 (0.0–0.0) |
| indist | all_three | 99.9 ± 0.0 (99.8–99.9) |

Distinct files behind those figures are in the per-release table below (kernel and initrd files per release: one
for `debian`, `el` and `opensuse`; three kernels for `alpine` and for `openbsd`: `bsd`, `bsd.mp`, `bsd.rd`).
Per release, with the mean of the three seeds:

| Fold | Release | Kernel files | Kernel recall | Initrd files | Initrd recall | False kernels (non-kernel files) |
|---|---|---:|---:|---:|---:|---:|
| alpine | alpine/3.18.12 | 3 | 20.7 % | 3 | 0.0 % | 0.0 % (21) |
| alpine | alpine/3.19.0 | 3 | 75.9 % | 1 | 0.0 % | 0.2 % (21) |
| alpine | alpine/3.19.9 | 3 | 56.6 % | 1 | 0.0 % | 0.0 % (20) |
| alpine | alpine/3.20.0 | 3 | 65.1 % | 1 | 0.0 % | 0.0 % (19) |
| alpine | alpine/3.20.10 | 3 | 72.4 % | 1 | 0.0 % | 0.0 % (20) |
| alpine | alpine/3.21.0 | 3 | 69.2 % | 1 | 0.0 % | 0.0 % (20) |
| alpine | alpine/3.21.10 | 3 | 70.3 % | 1 | 0.0 % | 0.0 % (18) |
| alpine | alpine/3.22.0 | 3 | 70.6 % | 1 | 0.0 % | 0.0 % (19) |
| alpine | alpine/3.22.8 | 3 | 71.7 % | 1 | 0.0 % | 0.0 % (22) |
| alpine | alpine/3.23.0 | 3 | 78.3 % | 1 | 0.0 % | 0.0 % (21) |
| alpine | alpine/3.23.8 | 3 | 67.5 % | 1 | 0.0 % | 0.0 % (19) |
| alpine | alpine/3.24.0 | 3 | 66.5 % | 1 | 0.0 % | 0.0 % (22) |
| alpine | alpine/3.24.4 | 3 | 78.4 % | 1 | 0.0 % | 0.0 % (18) |
| debian | debian/bookworm-20230607+deb12u15 | 1 | 100.0 % | 1 | 32.2 % | 10.2 % (265) |
| debian | debian/bullseye-20210731+deb11u12 | 1 | 99.9 % | 1 | 26.5 % | 9.6 % (256) |
| debian | debian/buster-20190702+deb10u13 | 1 | 78.1 % | 1 | 26.4 % | 11.0 % (268) |
| debian | debian/trixie-20250803+deb13u8 | 1 | 100.0 % | 1 | 17.1 % | 9.4 % (271) |
| el | alma/9.3 | 1 | 18.8 % | 1 | 90.6 % | 0.5 % (224) |
| el | alma/9.4 | 1 | 15.3 % | 1 | 93.6 % | 0.6 % (196) |
| el | fedora/39 | 1 | 100.0 % | 1 | 82.2 % | 0.0 % (53) |
| el | fedora/40 | 1 | 95.4 % | 1 | 90.8 % | 0.0 % (87) |
| el | fedora/41 | 1 | 98.9 % | 1 | 99.3 % | 0.0 % (77) |
| el | fedora/42 | 1 | 100.0 % | 1 | 85.3 % | 0.0 % (80) |
| el | fedora/43 | 1 | 60.3 % | 1 | 94.1 % | 0.0 % (66) |
| el | fedora/44 | 1 | 75.6 % | 1 | 88.9 % | 0.0 % (65) |
| el | rocky/8.9 | 1 | 5.9 % | 1 | 98.5 % | 0.8 % (126) |
| el | rocky/9.3 | 1 | 16.4 % | 1 | 84.4 % | 0.9 % (131) |
| el | rocky/9.4 | 1 | 11.6 % | 1 | 100.0 % | 0.0 % (115) |
| opensuse | opensuse/15.2 | 1 | 100.0 % | 1 | 26.8 % | 0.0 % (43) |
| opensuse | opensuse/15.3 | 1 | 100.0 % | 1 | 7.6 % | 0.0 % (24) |
| opensuse | opensuse/15.4 | 1 | 100.0 % | 1 | 8.5 % | 0.0 % (30) |
| opensuse | opensuse/15.5 | 1 | 100.0 % | 1 | 9.3 % | 0.0 % (28) |
| opensuse | opensuse/15.6 | 1 | 100.0 % | 1 | 76.3 % | 0.0 % (22) |
| openbsd | openbsd/7.6 | 3 | 33.3 % | 0 | n/a % | 0.0 % (1) |
| openbsd | openbsd/7.7 | 3 | 33.3 % | 0 | n/a % | 0.0 % (1) |
| openbsd | openbsd/7.8 | 3 | 33.3 % | 0 | n/a % | 0.0 % (1) |
| openbsd | openbsd/7.9 | 3 | 33.3 % | 0 | n/a % | 0.0 % (1) |
| openbsd | openbsd/8.0 | 3 | 33.3 % | 0 | n/a % | 0.0 % (1) |

What the baseline shows, each with its spread:

- **The seed spread is large, and it is the yardstick.** Initrd recall on `debian` ranges from 6.9 to 43.1 % over
  three seeds with the same data; kernel recall on `el` from 44.7 to 68.4 %; on `openbsd` (three kernel files) from
  0 to 66.7 %. B2's figure of 0.0 % initrd recall on its fold came from one training; three give 0.8, 3.3 and 9.1 %
  (mean 4.4 ± 4.2) on the same fold with the corrected generator. Anything smaller than that is not a result.
- **The failures are not only "an unseen format".** Alpine's initrds (gzip, a format that is in training through
  Debian) are recognised 0.0 % of the time in all three seeds, in all 13 releases. The Alma and Rocky kernels (bare
  gzip) are recalled 6 to 19 % (five files), against 60 to 100 % for the Fedora kernels (a PE file); of the three
  OpenBSD kernels (`bsd`, `bsd.mp`, `bsd.rd`) a seed finds none, one or two.
  openSUSE's xz initrds are found in 8 to 27 % of cases in the four older releases and 76 % in 15.6, although xz is in
  training through Fedora, AlmaLinux and Rocky.
- **False kernels depend on what is counted.** Over all files of the evaluation disks they look rare (0.0 to 0.2 %,
  `debian` 6.0 ± 1.0 %); over the held-out family's own non-kernel files `debian` is **10.1 ± 1.7 %** (1,060 files:
  GRUB modules, device trees and installer files of the netboot tarballs) and the other families are 0.0 to 0.4 %
  (`openbsd`: 5 files). Per candidate `el` reaches 6.3 ± 5.9 % (0.0–11.8) because a few files appear on many disks.
  B2's 4.8 % on `b2four` was a candidate rate: 5.3 ± 5.2 % here, and per file 0.5 ± 0.4 % over the family's files.
  The figure depends on which files the model happens to call kernels in a given training.
- **All-three on the family's own files is below the diluted figure in all five folds (by 8 and 12 points for `alpine` and `debian`, 0.6 to 2 for the others)**: `alpine` 88.7 %, `debian` 71.0 ± 5.6 %,
  `b2four` 96.1 %, `el` 98.4 % and `opensuse` 97.5 %; `openbsd` **0.0 %** in every seed and every configuration, which is
  **by construction**: the family `bsd` exists only for OpenBSD, so with OpenBSD held out the label is never trained and
  no file of the fold can be right on all three. Pooled all-three rows that include `openbsd` use its diluted 97.7 %.
- **In distribution it is not 100 %.** Kernel recall on `indist` is 86.7 ± 3.4 % (the bare gzip kernels of the test releases:
  Alpine 3.18, AlmaLinux and Rocky, are a format the training set holds once).

Candidate-level figures of the same runs, for comparison with B2 (they weight a file by how many disks it is on):

| Fold | Metric (candidates) | base |
|---|---|---|
| alpine | kernel_recall | 65.3 ± 9.4 (54.8–72.8) |
| alpine | initrd_recall | 0.0 ± 0.0 (0.0–0.0) |
| alpine | false_kernel | 0.0 ± 0.1 (0.0–0.1) |
| alpine | all_three | 80.5 ± 0.4 (80.1–80.9) |
| debian | kernel_recall | 94.8 ± 6.4 (87.7–100.0) |
| debian | initrd_recall | 25.4 ± 18.2 (6.7–43.1) |
| debian | false_kernel | 5.7 ± 1.1 (4.7–6.9) |
| debian | all_three | 76.2 ± 3.3 (72.6–79.2) |
| el | kernel_recall | 39.2 ± 17.2 (27.7–59.0) |
| el | initrd_recall | 92.1 ± 13.6 (76.3–100.0) |
| el | false_kernel | 6.3 ± 5.9 (0.0–11.8) |
| el | all_three | 79.2 ± 4.9 (73.6–82.9) |
| opensuse | kernel_recall | 100.0 ± 0.0 (100.0–100.0) |
| opensuse | initrd_recall | 25.5 ± 14.5 (13.1–41.5) |
| opensuse | false_kernel | 0.1 ± 0.2 (0.0–0.3) |
| opensuse | all_three | 86.7 ± 2.0 (84.8–88.8) |
| openbsd | kernel_recall | 33.2 ± 33.3 (0.0–66.6) |
| openbsd | initrd_recall | — |
| openbsd | false_kernel | 0.0 ± 0.0 (0.0–0.0) |
| openbsd | all_three | 59.8 ± 0.0 (59.8–59.8) |
| b2four | kernel_recall | 53.5 ± 6.1 (48.7–60.3) |
| b2four | initrd_recall | 5.1 ± 5.5 (1.0–11.4) |
| b2four | false_kernel | 5.3 ± 5.1 (0.0–10.2) |
| b2four | all_three | 68.5 ± 2.7 (66.3–71.4) |

On the exact releases B2's fold used (the test split of the four families), per file:

| Fold | Metric (files) | base |
|---|---|---|
| b2four | kernel_recall | 54.8 ± 4.0 (50.1–57.2) |
| b2four | initrd_recall | 6.9 ± 6.9 (1.8–14.7) |
| b2four | false_kernel | 0.2 ± 0.2 (0.0–0.5) |
| b2four | all_three | 98.8 ± 0.0 (98.7–98.8) |

## Step 3: option A, format augmentation

`formats.py`. Re-encoded copies of every training release (two per release): the initrd as gzip (levels 6 and 9),
xz (crc32 and crc64, presets 6 and 9), bzip2, zstd (levels 3 and 19, with and without the content size) or lz4
(frame format, and the legacy format `lz4 -l` of the kernel), or left as it was; the kernel header followed by
aarch64 code in the same formats or left as it was. The payload is a `newc` cpio of the Alpine minirootfs
releases that are always in training (real executables and libraries), and only the first 4 KiB of the stream are
kept, as the corpus does for every file. An initrd's name takes the compressor's extension where it had one. The
list of formats (gzip, xz, bzip2, zstd, lz4) comes from the task, but the corpus was tabulated by magic
number first, and the failing files were read (the bare-gzip kernels were added to the formats after reading which failed): the augmentation is tuned to this split's failures (xz initrds: Fedora, AlmaLinux, Rocky, openSUSE; bare gzip kernels: AlmaLinux, Rocky and Alpine 3.18; all other
kernels PE or ELF; gzip initrds: Alpine and Debian) which is disclosed because it was read. Nothing is derived from
a held-out family's files (the initrd payload is the Alpine minirootfs, userland of the `alpine-rootfs` system, which is in training for every fold, including `alpine`).

File level, three seeds:

| Fold | Metric (files) | base | A |
|---|---|---|---|
| alpine | kernel_recall | 66.4 ± 8.9 (56.4–73.7) | 66.5 ± 10.1 (54.8–72.5) |
| alpine | initrd_recall | 0.0 ± 0.0 (0.0–0.0) | 2.3 ± 3.2 (0.0–6.0) |
| alpine | false_kernel | 0.0 ± 0.0 (0.0–0.0) | 0.0 ± 0.0 (0.0–0.1) |
| alpine | all_three | 97.2 ± 0.0 (97.2–97.2) | 97.3 ± 0.1 (97.2–97.3) |
| debian | kernel_recall | 94.5 ± 6.8 (86.9–100.0) | 93.5 ± 9.2 (82.9–99.3) |
| debian | initrd_recall | 25.5 ± 18.1 (6.9–43.1) | 32.7 ± 15.8 (19.0–49.9) |
| debian | false_kernel | 6.0 ± 1.0 (5.1–7.1) | 9.6 ± 4.9 (6.4–15.1) |
| debian | all_three | 82.7 ± 3.3 (80.3–86.5) | 81.1 ± 5.5 (75.0–85.8) |
| el | kernel_recall | 54.4 ± 12.4 (44.7–68.4) | 61.8 ± 7.1 (53.6–65.9) |
| el | initrd_recall | 91.6 ± 14.4 (75.0–100.0) | 82.8 ± 8.7 (74.8–92.1) |
| el | false_kernel | 0.2 ± 0.2 (0.0–0.4) | 0.3 ± 0.1 (0.1–0.4) |
| el | all_three | 99.0 ± 0.2 (98.8–99.2) | 99.0 ± 0.0 (99.0–99.0) |
| opensuse | kernel_recall | 100.0 ± 0.0 (100.0–100.0) | 100.0 ± 0.0 (100.0–100.0) |
| opensuse | initrd_recall | 25.7 ± 14.4 (13.3–41.6) | 31.6 ± 20.7 (8.6–48.8) |
| opensuse | false_kernel | 0.0 ± 0.0 (0.0–0.0) | 0.2 ± 0.1 (0.2–0.3) |
| opensuse | all_three | 99.6 ± 0.1 (99.6–99.7) | 99.6 ± 0.1 (99.5–99.7) |
| openbsd | kernel_recall | 33.3 ± 33.3 (0.0–66.7) | 31.1 ± 33.6 (0.0–66.7) |
| openbsd | initrd_recall | — | — **n/a** |
| openbsd | false_kernel | 0.0 ± 0.0 (0.0–0.0) | 0.0 ± 0.0 (0.0–0.0) |
| openbsd | all_three | 97.7 ± 0.0 (97.7–97.7) | 97.7 ± 0.0 (97.7–97.7) |
| b2four | kernel_recall | 67.7 ± 4.5 (64.8–72.8) | 65.9 ± 1.8 (64.8–68.0) |
| b2four | initrd_recall | 4.4 ± 4.2 (0.8–9.1) | 36.9 ± 17.1 (17.2–47.7) **gain** |
| b2four | false_kernel | 0.2 ± 0.2 (0.0–0.4) | 0.3 ± 0.3 (0.1–0.6) |
| b2four | all_three | 97.9 ± 0.2 (97.7–98.1) | 98.3 ± 0.1 (98.3–98.4) |
| Fold | Metric (files) | base | A |
|---|---|---|---|
| indist | kernel_recall | 86.7 ± 3.4 (83.3–90.0) | 98.8 ± 2.0 (96.5–100.0) **gain** |
| indist | initrd_recall | 100.0 ± 0.0 (100.0–100.0) | 100.0 ± 0.0 (100.0–100.0) |
| indist | false_kernel | 0.0 ± 0.0 (0.0–0.0) | 0.0 ± 0.0 (0.0–0.0) |
| indist | all_three | 99.9 ± 0.0 (99.8–99.9) | 100.0 ± 0.1 (99.9–100.0) |
| Metric (files; mean over the folds that have it, then seeds) | base | A |
|---|---|---|
| kernel_recall | 69.7 ± 11.2 (58.7–81.1) | 70.6 ± 9.9 (60.7–80.6) |
| initrd_recall | 35.7 ± 11.2 (23.8–46.1) | 37.3 ± 4.0 (32.8–40.1) |
| false_kernel | 1.2 ± 0.2 (1.0–1.5) | 2.0 ± 1.0 (1.4–3.1) |
| all_three | 95.3 ± 0.7 (94.7–96.0) | 95.0 ± 1.1 (93.7–95.9) |

Per release, bare-gzip kernels and xz initrds of the `el` fold (mean of three seeds), `base` against `A`:

| Release | Kernel recall `base` | Kernel recall `A` | Initrd recall `base` | Initrd recall `A` |
|---|---:|---:|---:|---:|
| alma/9.3 (bare gzip kernel, xz initrd) | 18.8 % | 54.8 % | 90.6 % | 85.8 % |
| alma/9.4 (bare gzip kernel, xz initrd) | 15.3 % | 50.4 % | 93.6 % | 84.9 % |
| rocky/8.9 (bare gzip kernel, xz initrd) | 5.9 % | 38.1 % | 98.5 % | 83.1 % |
| rocky/9.3 (bare gzip kernel, xz initrd) | 16.4 % | 48.4 % | 84.4 % | 99.5 % |
| rocky/9.4 (bare gzip kernel, xz initrd) | 11.6 % | 32.0 % | 100.0 % | 74.8 % |

And the B2 fold on the exact releases of the B2 report, candidate level:

| Fold | Metric (candidates) | base | A |
|---|---|---|---|
| b2four | kernel_recall | 39.0 ± 4.9 (33.3–42.3) | 45.9 ± 6.1 (42.2–52.9) |
| b2four | initrd_recall | 7.6 ± 7.7 (2.0–16.3) | 46.0 ± 20.4 (23.5–63.4) **gain** |
| b2four | false_kernel | 5.3 ± 5.2 (0.0–10.4) | 5.0 ± 5.0 (1.6–10.7) |
| b2four | all_three | 75.2 ± 2.7 (73.1–78.2) | 80.7 ± 2.2 (78.3–82.5) **gain** |

Result, against the targets the task named:

- **Initrd recall: a gain where the format was missing from training.** On `b2four`, where no xz initrd is left in
  training, 4.4 ± 4.2 % becomes 36.9 ± 17.1 % (ranges 0.8–9.1 against 17.2–47.7). At candidate level on B2's
  exact releases 7.6 → 46.0 %. It is far from solved: the best seed finds 48 % of the initrd files.
  Where xz is already in training (`el` keeps openSUSE's) there is nothing to gain and nothing gained (91.6 → 82.8 %,
  within the spread), and on `alpine`, whose initrds are gzip, it stays at 0.0 to 6.0 %: the failure there is
  not (only) the format. An untested rival cause: the Alpine userland of `alpine-rootfs` is in training for every fold, labelled as non-initrd.
- **Kernel recall: no pooled gain, and the bare-gzip result is not a gain by the rule.** Pooled over the five families
  69.7 → 70.6 % (within the spread). On `indist` 86.7 → 98.8 % (a gain by the rule; those are the bare gzip kernels
  of the test releases, a format picked after reading which failed). On the five Alma and Rocky kernels of `el` the
  means over three seeds go from 6 to 19 % to 32 to 55 % per release (table), **but the per-seed values overlap for
  every release** (for example `alma/9.3`: base 0.0 / 56.5 / 0.0 %, A 1.4 / 63.6 / 99.3 %): the base run of one seed
  already finds them, and A finds them in two seeds of three, so this is a hint, not a result. The `el` fold does not
  move (54.4 → 61.8 %, ranges overlap), and A has **lower means for the PE kernels of Fedora**, a format training already holds: mean
  recall over the six releases 88.4 → 76.1 % (but per seed base is 81.4 / 92.7 / 91.1 and A 81.8 / 66.6 / 79.8: the ranges overlap, so by the rule this
  is a hint of harm, not a loss) (`fedora/39` 100.0 → 85.0, `fedora/44` 75.6 → 58.8), an effect the fold figure
  hides.
- **False kernels: no reduction, a possible increase.** Over the held-out family's files: `debian` 10.1 ± 1.7 → 16.0 ± 8.1 %
  (ranges overlap), `opensuse` 0.0 → 1.7 ± 0.5 % (a loss by the rule); over all files, pooled 1.2 ± 0.2 → 2.0 ± 1.0 %. A copy whose name stays and whose bytes are a format that
  other (non-kernel) files also have, possibly makes the model less sure of what a compressed file is (a mechanism not tested).
- A's gain is measured on the fold that lacks the format (xz initrds); it is **coverage of a known gap**, and it was shaped by
  reading which formats failed. On the folds that did not lack a format (`debian`, `opensuse`, `openbsd`) there is
  no gain, and the false kernels of the held-out family are not lower. Whether it generalises to a format nobody listed is not shown.

The same, **on the held-out family's files only** (false kernels and all-three; kernel and initrd recall are as above):

| Fold | Metric (files of the held-out family only) | base | A | name | bytes |
|---|---|---|---|---|---|
| alpine | false_kernel (260 files) | 0.0 ± 0.0 (0.0–0.0) | 0.2 ± 0.0 (0.2–0.2) | 2.5 ± 2.2 (0.0–3.8) | 0.0 ± 0.0 (0.0–0.0) |
| alpine | all_three (299 files) | 88.7 ± 0.1 (88.6–88.9) | 89.2 ± 0.2 (88.9–89.4) | 65.0 ± 7.3 (57.1–71.6) **loss** | 93.2 ± 2.2 (90.6–94.6) **gain** |
| debian | false_kernel (1060 files) | 10.1 ± 1.7 (8.6–11.9) | 16.0 ± 8.1 (10.7–25.4) | 2.7 ± 0.8 (1.8–3.3) **gain** | 26.9 ± 18.6 (9.1–46.2) |
| debian | all_three (1064 files) | 71.0 ± 5.6 (67.1–77.4) | 68.5 ± 9.2 (58.3–76.3) | 82.5 ± 4.2 (78.8–87.1) **gain** | 67.1 ± 18.6 (50.3–87.1) |
| el | false_kernel (1220 files) | 0.4 ± 0.3 (0.0–0.7) | 0.4 ± 0.2 (0.2–0.6) | 0.0 ± 0.0 (0.0–0.0) | 0.7 ± 0.0 (0.7–0.7) |
| el | all_three (1231 files) | 98.4 ± 0.3 (98.0–98.6) | 98.4 ± 0.0 (98.3–98.4) | 17.1 ± 12.4 (2.9–26.0) **loss** | 87.8 ± 13.6 (72.1–96.6) **loss** |
| opensuse | false_kernel (147 files) | 0.0 ± 0.0 (0.0–0.1) | 1.7 ± 0.5 (1.3–2.2) **loss** | 0.4 ± 0.5 (0.1–0.9) | 0.0 ± 0.0 (0.0–0.0) |
| opensuse | all_three (152 files) | 97.5 ± 0.4 (97.1–97.9) | 97.6 ± 0.6 (96.9–97.9) | 2.4 ± 1.5 (1.3–4.1) **loss** | 96.9 ± 0.4 (96.7–97.4) |
| openbsd | false_kernel (5 files) | 0.0 ± 0.0 (0.0–0.0) | 0.0 ± 0.0 (0.0–0.0) | 0.0 ± 0.0 (0.0–0.0) | 100.0 ± 0.0 (100.0–100.0) **loss** |
| openbsd | all_three (20 files) | 0.0 ± 0.0 (0.0–0.0) | 0.0 ± 0.0 (0.0–0.0) | 0.0 ± 0.0 (0.0–0.0) | 0.0 ± 0.0 (0.0–0.0) |
| b2four | false_kernel (976 files) | 0.5 ± 0.4 (0.0–0.8) | 0.5 ± 0.5 (0.2–1.0) | — **n/a** | — **n/a** |
| b2four | all_three (992 files) | 96.1 ± 0.4 (95.7–96.4) | 96.9 ± 0.2 (96.8–97.1) | — **n/a** | — **n/a** |

## Step 4a: name and bytes, on the held-out folds

B2 ran this ablation only in distribution (name alone 95.98 %, bytes alone 94.76 %, both 95.11 %). Here the
network sees only the name (content zeroed) or only the bytes (name zeroed), in training and in evaluation, per fold.

| Fold | Metric (files) | base | name | bytes |
|---|---|---|---|---|
| alpine | kernel_recall | 66.4 ± 8.9 (56.4–73.7) | 22.8 ± 0.0 (22.8–22.8) **loss** | 94.9 ± 0.0 (94.9–94.9) **gain** |
| alpine | initrd_recall | 0.0 ± 0.0 (0.0–0.0) | 0.0 ± 0.0 (0.0–0.0) | 6.7 ± 6.7 (0.0–13.3) |
| alpine | false_kernel | 0.0 ± 0.0 (0.0–0.0) | 0.6 ± 0.5 (0.0–0.9) | 0.0 ± 0.0 (0.0–0.0) |
| alpine | all_three | 97.2 ± 0.0 (97.2–97.2) | 91.1 ± 1.8 (89.3–92.9) **loss** | 98.3 ± 0.5 (97.7–98.6) **gain** |
| debian | kernel_recall | 94.5 ± 6.8 (86.9–100.0) | 0.0 ± 0.0 (0.0–0.0) **loss** | 100.0 ± 0.0 (100.0–100.0) |
| debian | initrd_recall | 25.5 ± 18.1 (6.9–43.1) | 14.2 ± 10.4 (4.0–24.8) | 0.0 ± 0.0 (0.0–0.0) **loss** |
| debian | false_kernel | 6.0 ± 1.0 (5.1–7.1) | 1.6 ± 0.5 (1.1–2.0) **gain** | 16.1 ± 11.1 (5.5–27.5) |
| debian | all_three | 82.7 ± 3.3 (80.3–86.5) | 89.5 ± 2.5 (87.4–92.3) **gain** | 80.4 ± 11.1 (70.3–92.3) |
| el | kernel_recall | 54.4 ± 12.4 (44.7–68.4) | 36.6 ± 12.5 (29.1–51.0) | 54.5 ± 0.0 (54.5–54.5) |
| el | initrd_recall | 91.6 ± 14.4 (75.0–100.0) | 48.7 ± 1.1 (47.4–49.4) **loss** | 3.0 ± 5.2 (0.0–9.1) **loss** |
| el | false_kernel | 0.2 ± 0.2 (0.0–0.4) | 0.0 ± 0.0 (0.0–0.0) | 0.4 ± 0.0 (0.4–0.4) |
| el | all_three | 99.0 ± 0.2 (98.8–99.2) | 51.0 ± 7.3 (42.6–56.3) **loss** | 92.8 ± 8.0 (83.5–98.0) **loss** |
| opensuse | kernel_recall | 100.0 ± 0.0 (100.0–100.0) | 39.3 ± 4.7 (33.9–42.4) **loss** | 100.0 ± 0.0 (100.0–100.0) |
| opensuse | initrd_recall | 25.7 ± 14.4 (13.3–41.6) | 6.8 ± 3.6 (4.1–10.9) **loss** | 20.0 ± 0.0 (20.0–20.0) |
| opensuse | false_kernel | 0.0 ± 0.0 (0.0–0.0) | 0.1 ± 0.1 (0.0–0.1) | 0.1 ± 0.2 (0.0–0.3) |
| opensuse | all_three | 99.6 ± 0.1 (99.6–99.7) | 85.4 ± 0.2 (85.2–85.7) **loss** | 99.4 ± 0.2 (99.2–99.6) |
| openbsd | kernel_recall | 33.3 ± 33.3 (0.0–66.7) | 0.0 ± 0.0 (0.0–0.0) | 97.8 ± 3.8 (93.3–100.0) **gain** |
| openbsd | initrd_recall | — | — **n/a** | — **n/a** |
| openbsd | false_kernel | 0.0 ± 0.0 (0.0–0.0) | 0.0 ± 0.0 (0.0–0.0) | 0.7 ± 0.2 (0.6–0.9) |
| openbsd | all_three | 97.7 ± 0.0 (97.7–97.7) | 97.7 ± 0.0 (97.7–97.7) | 97.5 ± 0.2 (97.4–97.7) |
| Metric (files; mean over the folds that have it, then seeds) | base | name | bytes |
|---|---|---|---|
| kernel_recall | 69.7 ± 11.2 (58.7–81.1) | 19.7 ± 3.1 (17.2–23.3) **loss** | 89.4 ± 0.8 (88.6–89.9) **gain** |
| initrd_recall | 35.7 ± 11.2 (23.8–46.1) | 17.4 ± 2.0 (15.6–19.6) **loss** | 7.4 ± 0.8 (6.7–8.3) **loss** |
| false_kernel | 1.2 ± 0.2 (1.0–1.5) | 0.4 ± 0.1 (0.4–0.5) | 3.4 ± 2.2 (1.4–5.7) |
| all_three | 95.3 ± 0.7 (94.7–96.0) | 83.0 ± 1.3 (81.5–83.8) **loss** | 93.7 ± 1.1 (92.4–94.4) **loss** |

- **Reading 2 of the note is confirmed, and goes further: on two new families the name hurts kernels.** Bytes alone finds
  the kernels better than name and bytes together: pooled 69.7 ± 11.2 → 89.4 ± 0.8 % (gain by the rule), driven by `alpine`
  66.4 → 94.9 % and `openbsd` 33.3 → 97.8 % (`el` 54.4 → 54.5 %, `debian` 94.5 → 100.0 %, `opensuse` 100 → 100 %: no change beyond the
  spread; pooled all-three is lower, 95.3 → 93.7 %, a loss by the rule). Name alone finds almost none (19.7 %): paths of a family the model has not
  seen say nothing.
- **Bytes alone cannot find initrds**: 7.4 ± 0.8 % pooled against 35.7 ± 11.2 % with both (0.0 % on `debian`): a
  compressed stream looks like any other compressed file without its name. The two modalities are complementary and
  neither alone is the answer: bytes for kernels (they have a header), the name for initrds.
- **False kernels with bytes alone are not shown higher or lower** (pooled over all files 1.2 → 3.4 %, ranges overlap; the held-out family's own files:
  `debian` 10.1 → 26.9 ± 18.6 %, ranges overlapping, and the OpenBSD loader is called a kernel in every seed), and name
  alone has fewer (`debian` 2.7 %) only because it calls almost nothing a kernel (kernel recall 0.0 % there). Its all-three
  on `debian` is a gain (71.0 → 82.5 %) for the same reason: the Debian files are mostly not kernels.
- Two disclosures about these two networks: the up-front zeroing of one modality is followed by the usual 25 % content and
  15 % name dropout, so up to a quarter (bytes-only) or 15 % (name-only) of their training rows are blank with real labels,
  which slightly handicaps them; they were not re-run without it.
- **Consequence (a hypothesis, not tested here):** the 15 % name dropout of B2 is too little for kernels on a new
  family and the balance of the two inputs could be trained differently; option C below is one way, a per-class
  dropout another.

## Step 5: confidence of the margin

The fold used to fit is **`opensuse`**, which is not used for the final report of this step; thresholds and
temperatures fitted there are reported on `alpine`, `debian` and `el` (`openbsd` and `b2four` were not run for the
outlier-exposure configuration and `b2four` contains openSUSE). The role head only. The *margin* is the best score
minus the second best, in integer scores, which is what the loader could compare against a threshold. **Temperature
scaling does not change the ranking by margin**: the risk-coverage curve of the raw margin is the same with or
without it; the temperature changes only the probabilities (so the ECE) and where a threshold lands on them. The
temperature is fitted by the lowest negative log-likelihood on 4,000 candidates of `opensuse`; the integer scores
are large (T between 3,400 and 5,800), so the grid goes to 2^20.

Role head, candidate level; temperature fitted on `opensuse` (grid, lowest negative log-likelihood).

| Config | Fold | ECE raw scores (T = 1) | T fitted | ECE after temperature | Accuracy of the role |
|---|---|---|---|---|---|
| base | alpine | 18.4 ± 1.0 (17.7–19.6) % | 4871.0, 4096.0, 4871.0 | 14.6 ± 3.3 (11.8–18.2) % | 81.6 ± 1.0 (80.4–82.3) % |
| base | debian | 17.6 ± 1.4 (16.5–19.2) % | 4871.0, 4096.0, 4871.0 | 25.1 ± 9.4 (15.3–34.1) % | 82.4 ± 1.4 (80.9–83.5) % |
| base | el | 18.8 ± 6.4 (14.5–26.1) % | 4871.0, 4096.0, 4871.0 | 14.9 ± 2.9 (13.2–18.2) % | 81.2 ± 6.4 (73.9–85.5) % |
| OE | alpine | 23.1 ± 0.7 (22.3–23.5) % | 5792.6, 4096.0, 3444.3 | 10.3 ± 3.7 (7.3–14.4) % | 78.0 ± 2.0 (76.4–80.2) % |
| OE | debian | 20.8 ± 4.4 (15.9–24.0) % | 5792.6, 4096.0, 3444.3 | 18.7 ± 8.2 (10.8–27.2) % | 79.2 ± 4.3 (76.0–84.1) % |
| OE | el | 20.1 ± 1.6 (18.3–21.4) % | 5792.6, 4096.0, 3444.3 | 13.5 ± 2.5 (10.6–15.4) % | 80.5 ± 1.6 (78.6–81.7) % |

Threshold fitted on `opensuse`, reported on the other folds (config `base`; weights: one per distinct file).

| Fold | Quantile of the calibration fold's kernel margins | Threshold (seeds) | False kernels removed | Real kernels lost | False kernels left (files) |
|---|---|---|---|---|---|
| alpine | 0.00 | 4459, 2368, 124 | 0.0 ± 0.0 (0.0–0.0) % | 23.8 ± 24.8 (0.0–49.6) % | 0.0 |
| alpine | 0.10 | 8114, 5573, 3408 | 100.0 ± 0.0 (100.0–100.0) % | 46.4 ± 28.3 (15.8–71.5) % | 0.0 |
| alpine | 0.25 | 10588, 9048, 7472 | 100.0 ± 0.0 (100.0–100.0) % | 71.4 ± 24.8 (51.6–99.2) % | 0.0 |
| alpine | 0.50 | 11901, 10027, 8720 | 100.0 ± 0.0 (100.0–100.0) % | 73.8 ± 24.2 (52.3–100.0) % | 0.0 |
| debian | 0.00 | 4459, 2368, 124 | 64.5 ± 48.8 (8.2–94.7) % | 9.3 ± 11.5 (0.7–22.3) % | 33.6 |
| debian | 0.10 | 8114, 5573, 3408 | 99.4 ± 1.1 (98.1–100.0) % | 33.7 ± 35.9 (10.2–74.9) % | 0.6 |
| debian | 0.25 | 10588, 9048, 7472 | 100.0 ± 0.0 (100.0–100.0) % | 75.6 ± 22.8 (54.7–100.0) % | 0.0 |
| debian | 0.50 | 11901, 10027, 8720 | 100.0 ± 0.0 (100.0–100.0) % | 85.6 ± 24.9 (56.8–100.0) % | 0.0 |
| el | 0.00 | 4459, 2368, 124 | 63.2 ± 52.0 (26.4–100.0) % | 19.8 ± 17.2 (0.6–33.7) % | 2.0 |
| el | 0.10 | 8114, 5573, 3408 | 100.0 ± 0.0 (100.0–100.0) % | 46.9 ± 21.2 (25.0–67.4) % | 0.0 |
| el | 0.25 | 10588, 9048, 7472 | 100.0 ± 0.0 (100.0–100.0) % | 81.8 ± 20.0 (60.4–100.0) % | 0.0 |
| el | 0.50 | 11901, 10027, 8720 | 100.0 ± 0.0 (100.0–100.0) % | 86.5 ± 21.1 (62.2–100.0) % | 0.0 |

Threshold fitted on `opensuse`, reported on the other folds (config `OE`; weights: one per distinct file).

| Fold | Quantile of the calibration fold's kernel margins | Threshold (seeds) | False kernels removed | Real kernels lost | False kernels left (files) |
|---|---|---|---|---|---|
| alpine | 0.00 | 292, 4837, 1517 | 100.0 ± 0.0 (100.0–100.0) % | 9.6 ± 11.4 (1.0–22.5) % | 0.0 |
| alpine | 0.10 | 5470, 6494, 5639 | 100.0 ± 0.0 (100.0–100.0) % | 28.7 ± 5.2 (24.5–34.5) % | 0.0 |
| alpine | 0.25 | 9344, 11112, 8688 | 100.0 ± 0.0 (100.0–100.0) % | 51.4 ± 5.2 (46.0–56.5) % | 0.0 |
| alpine | 0.50 | 10816, 11840, 9971 | 100.0 ± 0.0 (100.0–100.0) % | 55.0 ± 7.6 (46.7–61.7) % | 0.0 |
| debian | 0.00 | 292, 4837, 1517 | 59.2 ± 46.5 (7.8–98.5) % | 5.8 ± 9.6 (0.0–16.9) % | 105.2 |
| debian | 0.10 | 5470, 6494, 5639 | 99.1 ± 1.1 (97.9–100.0) % | 32.1 ± 31.9 (10.4–68.7) % | 2.5 |
| debian | 0.25 | 9344, 11112, 8688 | 100.0 ± 0.0 (100.0–100.0) % | 44.6 ± 26.9 (17.1–70.9) % | 0.0 |
| debian | 0.50 | 10816, 11840, 9971 | 100.0 ± 0.0 (100.0–100.0) % | 63.7 ± 36.6 (26.9–100.0) % | 0.0 |
| el | 0.00 | 292, 4837, 1517 | 50.6 ± 50.0 (0.0–100.0) % | 15.2 ± 16.5 (0.2–32.9) % | 2.6 |
| el | 0.10 | 5470, 6494, 5639 | 94.3 ± 9.8 (83.0–100.0) % | 30.4 ± 16.3 (14.1–46.7) % | 0.3 |
| el | 0.25 | 9344, 11112, 8688 | 100.0 ± 0.0 (100.0–100.0) % | 56.7 ± 33.1 (31.2–94.1) % | 0.0 |
| el | 0.50 | 10816, 11840, 9971 | 100.0 ± 0.0 (100.0–100.0) % | 62.8 ± 31.7 (36.9–98.2) % | 0.0 |

- **The raw scores are saturated, not calibrated**: at T = 1 the integer scores (thousands) give a maximum probability of
  indistinguishable from 1 (median 1.0), so the raw ECE of 17.6 to 18.8 % is simply the error rate (role accuracy about 81 %) and says nothing
  about the ordering of confidences. Temperature
  fitted on `opensuse` gives 14.6 % (`alpine`), 14.9 % (`el`) and 25.1 % (`debian`): the ranges of the three seeds
  overlap the raw ones in every fold (`debian` 16.5–19.2 raw, 15.3–34.1 after), so by the rule none of the three is
  shown to improve or worsen (and the temperature, like the threshold, is fitted on a different training than the one it is applied to), and a temperature that transfers is **not shown**. Even at best a 15 % error remains.
  Outlier exposure's after-temperature ECE (10.3, 18.7, 13.5 %) is lower than base's in all three, also within the
  spread; the table supports neither "better" nor "no better".
- **The margin separates errors only with a large loss of kernels.** (Sample sizes: `debian` has 142 to 195 distinct false-kernel
  files per seed; `el` 8 and 7 files in two seeds and none in the third; `alpine` one file in one seed and none in the
  others, so its rows, and the means over seeds that have any, rest on a handful of files.) A threshold at the 10 % quantile of the
  calibration fold's kernel margins removes 99 to 100 % of the false kernels of `alpine`, `debian` and `el` and
  loses 33.7 ± 35.9 % (`debian`), 46.4 ± 28.3 % (`alpine`) and 46.9 ± 21.2 % (`el`) of the kernels the model found;
  at the 25 % quantile the loss is 71 to 82 %. The lowest threshold (the smallest margin of the calibration fold's kernels) loses 9 to 24 % of the
  kernels (`alpine` 23.8 %) and removes 63 to 65 % of the false kernels of `debian` and `el`, and 0 % on `alpine`
  where only a seed has a false kernel, a single file. The ranges between seeds are huge (`debian` at q = 0: 8 to 95 % removed): **the scale of the margin
  differs between trainings** (the threshold of the 10 %, 25 % and 50 % quantiles differs 2.4, 1.4 and 1.4 times between
  seeds; the lowest quantile, a sample minimum, 36 times). Note what was fitted on what: the threshold of seed *s* comes
  from the `opensuse` model of that seed, which is a **different training** (without openSUSE) from the one it is applied to
  (without `alpine`, `debian` or `el`), so the figures mix the cost of a margin with the shift of scale between
  trainings; they are not the figures of one shipped model with a threshold fitted on a held-out family. A threshold
  would have to be fitted on the weights that ship.
On the held-out family's own files, `base` against outlier exposure:

| Fold | Metric (files of the held-out family only) | base | OE |
|---|---|---|---|
| alpine | false_kernel (260 files) | 0.0 ± 0.0 (0.0–0.0) | 1.2 ± 1.1 (0.0–2.3) |
| alpine | all_three (299 files) | 88.7 ± 0.1 (88.6–88.9) | 79.3 ± 5.3 (74.3–84.9) **loss** |
| debian | false_kernel (1060 files) | 10.1 ± 1.7 (8.6–11.9) | 19.7 ± 7.7 (12.6–28.0) **loss** |
| debian | all_three (1064 files) | 71.0 ± 5.6 (67.1–77.4) | 66.1 ± 9.2 (58.8–76.4) |
| el | false_kernel (1220 files) | 0.4 ± 0.3 (0.0–0.7) | 0.4 ± 0.1 (0.3–0.5) |
| el | all_three (1231 files) | 98.4 ± 0.3 (98.0–98.6) | 98.4 ± 0.1 (98.3–98.5) |
| opensuse | false_kernel (147 files) | 0.0 ± 0.0 (0.0–0.1) | 0.0 ± 0.0 (0.0–0.0) |
| opensuse | all_three (152 files) | 97.5 ± 0.4 (97.1–97.9) | 97.4 ± 0.2 (97.2–97.6) |

- **Outlier exposure** (3,000 generated files in 16 formats that no training or evaluation negative uses, the role,
  family and protocol heads pulled to uniform on them, weight 0.5, 10 % of each batch): no reduction in false kernels
  (over all files, pooled over `alpine`, `debian` and `el`, 2.1 ± 0.4 → 4.1 ± 1.5 %, a **loss** by the rule; the held-out family's files: `debian` 10.1 → 19.7 ± 7.7 %, a loss, `alpine` 0.0 → 1.2 %), OE trains on 10 % more rows per step than the baseline,, no gain in kernel recall (71.8 → 73.0 %), a drop of
  all-three on `alpine` (97.2 → 94.8 %). Its ECE after temperature (10.3, 18.7, 13.5 %) is not shown to differ from base's (within the spread); its threshold table is as unstable as base's. It did not do what the note expected of it.
- **G, the three seeds as an ensemble** (sum of the integer scores; 3 x 4.04 MiB and 12.7 M multiply-accumulates,
  inside ADR 0027's budget): no gain in false kernels or kernel recall against the mean of the seeds
  (`debian` 6.0 → 6.0 %, `el` 54.4 → 49.6 %). Disagreement between the three models flags 62.7 % of the false kernels
  of `debian` and 100 % of those of `el`, and 13.1 % and 10.5 % of the real kernels there, but **23.5 % of the real
  kernels of `alpine` and 100 % of those of `openbsd`** (the table has no false kernel to flag on those folds): a
  signal on two folds, not on all. The sum of integer scores is also dominated by the model with the largest score scale,
  which the margin results above say varies between trainings. G was measured
  because the single margin turned out not to transfer between trainings (no trigger criterion was written down
  before the curves were computed, so this is a decision after the fact).

| Fold | Single model false kernel (files) | Ensemble false kernel | Single kernel recall | Ensemble kernel recall | Seeds disagree on the role: share of false kernels | ... of real kernels |
|---|---|---|---|---|---|---|
| alpine | 0.0 ± 0.0 (0.0–0.0) % | 0.0 % | 66.4 ± 8.9 (56.4–73.7) % | 63.9 % | n/a % | 23.5 % |
| debian | 6.0 ± 1.0 (5.1–7.1) % | 6.0 % | 94.5 ± 6.8 (86.9–100.0) % | 100.0 % | 62.7 % | 13.1 % |
| el | 0.2 ± 0.2 (0.0–0.4) % | 0.3 % | 54.4 ± 12.4 (44.7–68.4) % | 49.6 % | 100.0 % | 10.5 % |
| opensuse | 0.0 ± 0.0 (0.0–0.0) % | 0.0 % | 100.0 ± 0.0 (100.0–100.0) % | 100.0 % | n/a % | 0.0 % |
| openbsd | 0.0 ± 0.0 (0.0–0.0) % | 0.0 % | 33.3 ± 33.3 (0.0–66.7) % | 33.3 % | n/a % | 100.0 % |

## Step 4b: option C, mixed-name augmentation

In training, 25 % of the disks (of the same 4,000) are generated in a *mixed* mode: half of the files of every entry
are renamed to words and extensions unrelated to what they are (`.rom`, `.fw`, `.pak`, `.sys`), and one to three junk
files take kernel-looking names (`\\vmlinux`, `\\kernel8.img`, `\\Image.gz`, `\\boot\\vmlinuz-5.15.0-91-generic`, ...)
and stay labelled "other". None of those names or extensions is in the evaluation generators (the renamed set uses
hex names with `.bin`, `.dat`, `.img`; the decoy set has its own eight paths; a test checks the two lists are
disjoint). Run on `el` and `debian` (the folds with headroom) and on `indist`, where the renamed and decoy sets live.

| Fold | Metric (files) | base | C |
|---|---|---|---|
| el | kernel_recall | 54.4 ± 12.4 (44.7–68.4) | 60.2 ± 7.6 (55.3–68.9) |
| el | initrd_recall | 91.6 ± 14.4 (75.0–100.0) | 90.0 ± 15.1 (72.6–99.8) |
| el | false_kernel | 0.2 ± 0.2 (0.0–0.4) | 0.3 ± 0.1 (0.3–0.4) |
| el | all_three | 99.0 ± 0.2 (98.8–99.2) | 99.0 ± 0.2 (98.8–99.1) |
| debian | kernel_recall | 94.5 ± 6.8 (86.9–100.0) | 93.9 ± 8.4 (84.4–100.0) |
| debian | initrd_recall | 25.5 ± 18.1 (6.9–43.1) | 45.1 ± 14.7 (32.3–61.1) |
| debian | false_kernel | 6.0 ± 1.0 (5.1–7.1) | 28.2 ± 5.2 (24.9–34.2) **loss** |
| debian | all_three | 82.7 ± 3.3 (80.3–86.5) | 62.4 ± 2.6 (60.0–65.1) **loss** |

Decoy set (`indist`, files named like kernels that are not):

| Fold | Metric (files) | base | C |
|---|---|---|---|
| indist | kernel_recall | 86.9 ± 3.5 (83.5–90.4) | 87.7 ± 4.6 (82.7–91.8) |
| indist | initrd_recall | 100.0 ± 0.0 (100.0–100.0) | 100.0 ± 0.0 (100.0–100.0) |
| indist | false_kernel | 1.8 ± 0.3 (1.5–2.2) | 1.2 ± 0.5 (0.8–1.8) |
| indist | all_three | 95.5 ± 1.1 (94.2–96.2) | 97.2 ± 1.0 (96.3–98.2) **gain** |

Renamed set (`indist`, real kernels and initrds under hex names):

| Fold | Metric (files) | base | C |
|---|---|---|---|
| indist | kernel_recall | 74.9 ± 2.3 (73.5–77.5) | 77.0 ± 1.1 (75.9–78.0) |
| indist | initrd_recall | 66.1 ± 5.7 (62.8–72.7) | 83.2 ± 15.2 (70.5–100.0) |
| indist | false_kernel | 0.0 ± 0.0 (0.0–0.0) | 0.0 ± 0.1 (0.0–0.1) |
| indist | all_three | 98.8 ± 0.6 (98.2–99.2) | 98.9 ± 0.4 (98.6–99.4) |

On the held-out family's own files (`el`, `debian`), configurations `base`, `C` and `E`:

| Fold | Metric (files of the held-out family only) | base | C | E |
|---|---|---|---|---|
| el | false_kernel (1220 files) | 0.4 ± 0.3 (0.0–0.7) | 0.5 ± 0.0 (0.5–0.5) | 0.2 ± 0.2 (0.0–0.3) |
| el | all_three (1231 files) | 98.4 ± 0.3 (98.0–98.6) | 98.3 ± 0.3 (98.0–98.5) | 98.5 ± 0.2 (98.3–98.7) |
| debian | false_kernel (1060 files) | 10.1 ± 1.7 (8.6–11.9) | 47.2 ± 8.6 (41.7–57.1) **loss** | 8.7 ± 3.2 (6.0–12.2) |
| debian | all_three (1064 files) | 71.0 ± 5.6 (67.1–77.4) | 37.1 ± 4.3 (33.1–41.7) **loss** | 73.3 ± 2.5 (71.0–76.0) |

- **Where names lie (`indist`): a small gain on decoys, none shown on renamed files.** On the decoy set all-three
  goes 95.5 ± 1.1 → 97.2 ± 1.0 % (a gain by the rule, ranges 94.2–96.2 against 96.3–98.2) and false kernels
  1.8 → 1.2 % (within the spread). On the renamed set kernel recall 74.9 → 77.0 % and initrd recall 66.1 → 83.2 %
  (±15) are within the spread.
- **On unseen families: no gain, and a large loss on `debian`.** `el` is unchanged (kernel 54.4 → 60.2 %, within
  the spread). On `debian` false kernels go from 6.0 ± 1.0 % to **28.2 ± 5.2 %** over all files (**10.1 → 47.2 ± 8.6 %** over
  the family's own files) and all-three from 82.7 to 62.4 % (71.0 → 37.1 % on the family's files): both a loss, while initrd recall rises 25.5 → 45.1 % (within the spread). With the name made less reliable
  the model probably leans on the bytes, and the Debian files that look like kernels to it (GRUB modules, device trees,
  the netboot `linux` of other releases) are called kernels.
- A contamination to disclose: five of the eight evaluation decoy paths have a near twin in the training list
  (`\\boot\\vmlinuz-6.1.0-18-arm64` and `\\boot\\vmlinuz-5.15.0-91-generic`; `\\bsd` and `\\bsd.mp`; `\\grubaa64.efi` and
  `\\boot\\grub\\grubaa64.efi`; `\\boot\\Image` and `\\Image.gz`; `\\initrd.img` and `\\boot\\initramfs-linux.img`), so the lists
  are disjoint as strings only and the decoy gain, 0.1 point beyond the ranges, is closer to in-sample than it looks.
- C **did not** pass the test the note set it: it does not reduce the false kernels on unseen families and it can
  increase them a lot. It stays a candidate only for a configuration with a measured guard against that (see "What
  B3 should settle").

## Step 6: options E and F, single factors

Both change the window contract and neither touches `window.py`: `contexts.py` builds the longer window in the
measurement only. **E**: one more position after the 512 holding the size bucket (`bit_length` of the file size,
capped at 31; it shares the byte embedding, so it is not the 32 x 256 one-hot the note costed; +8,192 weights). **F**:
512 more positions: the names of up to 16 other files of the same directory, sorted, 32 bytes each (window 1,024;
the first layer doubles to 8.4 M weights and multiply-accumulates, inside ADR 0027's budget at 8.0 MiB and 8.4 M
MACs). Neither is a filter: the size is an input to the model, and ADR 0027's enumeration "applies no filter by
name, size, extension or content". They were run on `el` and `debian` (E) and on `el` only (F: a run takes twice
as long), three seeds each.

| Fold | Metric (files) | base | E | F |
|---|---|---|---|---|
| el | kernel_recall | 54.4 ± 12.4 (44.7–68.4) | 59.9 ± 5.7 (54.9–66.1) | 49.9 ± 1.9 (48.6–52.1) |
| el | initrd_recall | 91.6 ± 14.4 (75.0–100.0) | 96.2 ± 3.3 (93.7–100.0) | 90.0 ± 8.4 (82.6–99.0) |
| el | false_kernel | 0.2 ± 0.2 (0.0–0.4) | 0.1 ± 0.1 (0.0–0.2) | 0.2 ± 0.1 (0.1–0.3) |
| el | all_three | 99.0 ± 0.2 (98.8–99.2) | 99.1 ± 0.1 (99.0–99.2) | 99.1 ± 0.1 (99.0–99.1) |
| debian | kernel_recall | 94.5 ± 6.8 (86.9–100.0) | 99.2 ± 1.4 (97.6–100.0) | — **n/a** |
| debian | initrd_recall | 25.5 ± 18.1 (6.9–43.1) | 43.0 ± 3.9 (39.8–47.4) | — **n/a** |
| debian | false_kernel | 6.0 ± 1.0 (5.1–7.1) | 5.2 ± 1.9 (3.6–7.3) | — **n/a** |
| debian | all_three | 82.7 ± 3.3 (80.3–86.5) | 84.1 ± 1.5 (82.7–85.7) | — **n/a** |

- **E, the size: better means in both folds, never beyond the spread.** `debian` kernel recall 94.5 → 99.2 % and
  initrd recall 25.5 → 43.0 % (ranges 6.9–43.1 against 39.8–47.4 overlap), `el` initrd 91.6 → 96.2 %; the narrower
  seed spread of E (± 3.9 against ± 18.1 on `debian` initrd) is its most consistent effect. **A caveat that
  weakens even that**: the generated negatives have no real size; they were given sizes drawn log-uniformly over
  512 B to 256 MiB (the review of this change found that the first version, their true length of at most 4 KiB, would
  have let the network separate kernels from junk by size alone). Whether real negatives have that spread is an
  assumption; the size of the real files in the corpus is their pinned size.
- **F, neighbour names: no gain.** On `el`, kernel recall 54.4 → 49.9 %, initrd 91.6 → 90.0 %, false kernels 0.2 → 0.2 %.
  The hypothesis was that an Alma or Rocky kernel at `\\<machine>\\<version>\\linux` called "initrd" would be fixed
  by seeing an `initrd` beside it; the kernel recall of those releases did not move beyond the seed spread. It costs
  twice the first layer and twice the training time. Not measured on the other folds.
- **G** is in step 5.

## What B3 should settle (it was not changed here)

1. **The window contract and the `\WSOS.MDL` format version.** Nothing measured here justifies a longer window: F
   did not help and E did not beat the spread. If E is wanted later it needs one more position and a size source
   in the enumeration; deciding now, while no weights are released, is cheaper than versioning the file later.
2. **Which labelling errors B3 can live with.** Assembly (step 1) is only as good as the roles: with the model's own
   roles the BLS-line rule has precision 100 % but recall 43 ± 9 %, and the ADR 0034 combination 71.7 ± 11.4 % of the
   pairs and 51.6 ± 10.5 % of the disks entirely right. Whether a disk with a wrong or missing label is
   shown as unverified, falls back to the stored decision of an earlier boot, or is refused, is a B3 decision.
3. **Whether the loader treats a loader as kernel-like in the fallback rule.** 113 of the 143 wrong pairs of the ADR 0034
   combination (true roles) pair a loader with the initrd beside it; excluding loaders from the fallback was not measured and
   would leave the other 30 (two systems sharing a root directory).
4. **How to use the margin, if at all.** It did not transfer between trainings (thresholds of the same quantile
   differ by 1.4 to 2.4 times between seeds), so a threshold has to be fitted on the weights that ship, on a held-out family, and
   it costs 34 to 47 % of the real kernels at the 10 % quantile for removing the false ones, on folds with few false files (step 5). Disagreement of three models is a
   better signal on two folds and triples the work (inside the budget).
5. **The balance of name and bytes**, since bytes alone find unseen kernels (89 %) and the name is what finds initrds
   (7 % without it). Neither the 15 % name dropout of B2 nor option C is the answer; a per-class treatment or a
   second head is not measured.
6. **More real releases** (option B of the note) remains the one lever not tried here: the corpus is what limits
   every fold, and it comes with the licence question of ADR 0027, section 6.
7. A test that the Rust forward pass reproduces `mdl.forward`, already asked for by B2.

## What could not be verified

- **Real disks.** Nothing here reads one. The generator's templates are the author's reading of each system's layout,
  and the entry texts, the shares of disks with text (25 %, 40 %, 100 %, per system) and the two-kernel layouts are
  invented. Every rule and every model figure is for those layouts.
- **Seeds.** Three per configuration; the spread is the range of three numbers. A gain that is "within spread"
  may be real and a "gain" may be luck: the folds with one to three distinct files (`openbsd`, `debian`) move in
  steps of 33 points (`openbsd`, three kernels) and 25 points (`debian`, four kernels and four initrds).
- **Runs before two fixes.** The 21 `base` runs and about a dozen of the `A` runs (the cache's timestamps do not say exactly which) were trained before the generator stopped
  stamping the current time in the headers of generated zip files (four bytes of the window of about 8 % of the junk)
  and before the sizes of generated files were drawn independently of class; neither affects them beyond those
  four bytes (the size is read only by E), and all later runs include both. Evaluation sets were byte-identical across
  seeds and configurations only from that point on.
- **Held-out folds are not independent of the choices made.** The formats of A come from the task but were completed after reading which files failed (bare-gzip kernels), so A's gain on `indist` and the 77.5 % with its roles are tuned to this split and not out of sample; C's decoy names and E's size model were chosen after the review.
- **Calibration** was fitted on one fold (`opensuse`) and the tables show only the lowest four quantiles; a
  second calibration fold (the swap) was not run.
- **B2's own numbers** (95.1 % on test, 69.5 % and 4.8 % on its fold) were not reproduced; the `indist` and `b2four`
  rows are runs of the corrected generator, which makes more candidates per disk (47,663 training candidates for 4,000 disks in the `indist` seed-0 run).
- **Cache key.** A run is cached by fold, configuration and seed (and, since the last fix, by the number of disks and
  epochs), not by the code that produced it; that is how the runs before the two generator fixes stayed in the results.
- **Tests of the measurement code.** The gate tests the rules, the window extras, the metrics, the calibration arithmetic and
  the generator; the numpy side (training, the fold construction in `b2b.py`, the tables of `b2b_report.py`) has a few
  tests run by hand (`test_training.py`) and no mutation entries, so a silent leak of a held-out family into
  training would be caught only by the check in `test_training.py`, not by the gate.
- **Weights** are not committed and not released (ADR 0027, section 6); the 102 trained models were kept in a cache
  outside the repository and are not part of the result.

## Reproduce

```bash
export OPENBLAS_NUM_THREADS=1 OMP_NUM_THREADS=1 VECLIB_MAXIMUM_THREADS=1
U="uv run --no-project --with numpy==2.5.5 --with scikit-learn==1.9.1 --with zstandard --with lz4 python -I"
$U boot-model/b2b.py queue --jobs 4 --seeds 0 1 2 --configs base A --folds alpine debian el opensuse openbsd b2four indist
$U boot-model/b2b.py queue --jobs 4 --seeds 0 1 2 --configs name bytes --folds alpine debian el opensuse openbsd
$U boot-model/b2b.py queue --jobs 4 --seeds 0 1 2 --configs OE --folds alpine debian el opensuse
$U boot-model/b2b.py queue --jobs 4 --seeds 0 1 2 --configs C --folds el debian indist
$U boot-model/b2b.py queue --jobs 4 --seeds 0 1 2 --configs E --folds el debian
$U boot-model/b2b.py queue --jobs 4 --seeds 0 1 2 --configs F --folds el
$U boot-model/b2b_report.py step1                     # and: folds, releases, pooled, confidence, ensemble
```

A run takes 10 to 45 minutes on an 8 GB Apple machine depending on what else is running; the 102 runs of this report took
about eight hours of wall-clock time on one machine, which was also doing other work. The cache (`~/.cache/workspace-os/boot-model/b2b`, or under `WSOS_CORPUS_CACHE`)
holds the results and the scores of each run. `zstandard` and `lz4` (BSD-3-Clause) are host-only packages loaded for two
of the formats of option A; nothing shipped depends on them (ADR 0005). The gate tests the standard-library parts;
`test_training.py` (numpy) is run by hand, and `python3 -I boot-model/mutation_check.py` confirms the tests by mutation.
