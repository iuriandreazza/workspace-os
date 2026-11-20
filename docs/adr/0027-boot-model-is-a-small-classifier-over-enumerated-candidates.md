---
title: "ADR 0027: The boot model is a small trained classifier over enumerated candidates"
type: adr
status: proposed
created: 2026-11-20
updated: 2026-11-20
tags: [boot, bootloader, model, uefi, inference, licence, aarch64]
---

# ADR 0027: The boot model is a small trained classifier over enumerated candidates

## Context

ADR 0009 puts a small model in the bootloader that detects kernels and decides
how to load them, and leaves open "language model or purpose-trained
classifier, and its size". Roadmap item B1-1 researched the options
(`docs/b1-boot-model-research.md`); this ADR settles them (item B1-2) and
measures what the decision costs to run in the UEFI environment, which the
research could not do.

This ADR does not reopen ADR 0009, ADR 0008 or ADR 0002. It fills in the points
ADR 0009 left open, inside the four limits it states (our system boots without
the model; decisions are stored; there is a fallback; the loader checks the
format before jumping). It also **narrows two of its statements**, which the
project owner should accept knowingly: ADR 0009 says the model decides "the
parameters to pass", and here the model only chooses among parameter lines that
already exist on the disk; and the enumeration of section 2 has structural
limits (a depth and a cap) that code, not the model, applies.

Status is `proposed`: the decision needs the project owner's acceptance, and
the accuracy of any model is B2's to show, not this ADR's.

## Decision

### 1. Kind and size: a purpose-trained classifier, within a budget

The boot model is a **classifier trained for this purpose**, not a language
model. It reads one candidate at a time and outputs scores for a fixed set of
closed labels. Nothing it outputs is text.

The shape that was measured (below) is the **reference shape**, not a promise
about the trained model: a byte-level network over a window of 512 positions,
each embedded in 32 numbers, one dense layer of 256, one of 128 and 32 output
scores; int8 weights and activations, 32-bit sums; 4,239,360 weights (4.0 MiB)
and 4,231,168 multiply-accumulates per candidate. B2 may change the shape
(including a tree ensemble) as long as it stays inside this **budget**:

| Budget | Limit | Why |
|---|---|---|
| Weights | at most 16 MiB | four times the reference; the whole model is read from the ESP and kept in RAM |
| Work per candidate | at most 16 million int8 multiply-accumulates | four times the reference |
| Candidates per decision | at most 256 | a hard cap; see "Inputs" |
| Scratch memory | at most 2 MiB beyond the weights | allocated once, before the first candidate |

Worst case at the budget, **derived** from the measured throughput (about
1.1 G multiply-accumulates per second on TCG and 24 to 30 G on HVF, below):
4.1 G multiply-accumulates, about 3.8 s on TCG and about 0.2 s on HVF. The
measured reference shape on a disk of 16 candidates takes 62 ms on TCG and
2.8 ms on HVF. Those two are **warm** figures: after a 1 s warm-up, on the
same weights and the same headers. A real boot runs the model once, on cores
that were mostly idle and with cold caches; the one earlier run without warm-up
took 5.9 ms for the first disk on HVF against 2.8 ms later, so the first
decision can take about twice as long (not measured on TCG). The worst case at
the budget is a warm figure as well. A disk of 16 candidates is a small disk;
at the cap of 256 candidates the reference shape takes about 1.0 s on TCG
(256 times 3.87 ms) and 44 ms on HVF, warm.

### 2. Inputs: enumerated candidates, a window of bytes and a name

**Enumerating the candidates is the model's input, not a scanner that overrules
the model.** ADR 0009 lists what the model reads: "the partition layout, file
names and paths, and the headers of candidate files". Listing the partitions
and the files of the EFI system partition is how the firmware hands that
material over; it takes no decision. The enumeration:

- applies no filter by name, size, extension or content, so no rule decides
  what looks like a kernel (the structural limits are the cap of 256
  candidates, in the order the firmware lists them, and the loader says so on
  screen when it is reached; a disk with more than 256 files listed before its
  kernel, such as a large Windows ESP, is not fully seen, and the model never
  scored what the cap cut: a stated limit that code, not the model, imposes);
- lists the GPT entries (type GUID, name) and the regular files of the ESP and
  of an XBOOTLDR partition (the latter read for entry text only, see section 3), to a depth of three directories;
- gives the model, for each candidate, a window of 512 positions: up to 64
  bytes of its path or partition name followed by the first bytes of its
  content (the arm64 Linux `Image` header is 64 bytes, and a PE header is
  found through its own header fields; `docs/b1-boot-model-research.md`,
  section 1). The exact windows (for example a second window at a partition offset for
  superblocks) are B2's to choose within the budget.

Only FAT is read, as the firmware reads it. No filesystem driver is added: the
known drivers are GPL-3.0-or-later (ADR 0005) and the research found no need
yet. A disk whose Linux lives on a root filesystem the firmware cannot read is
found only if it has a kernel on the ESP; this is a stated limit, not a defect
to fix here. A kernel on an XBOOTLDR partition is not bootable either until
ADR 0025 is amended (section 3).

### 3. The typed decision

For every candidate the model outputs closed labels, about 32 scores in the
reference shape (the labels below need 8 + 4 + 6 + 16 = 34; the two rows of
difference are 256 multiply-accumulates and do not change the time): **system family** (an enum fixed by B2's corpus), **protocol**, **role**
and a **group**:

| Field | Closed set |
|---|---|
| protocol | our kernel (ADR 0013), Linux through its EFI stub, chain-load of another EFI loader (ADR 0025), none |
| role | kernel image, initial ramdisk, device tree, EFI loader, entry text, other |
| group | one of 16 slots; candidates with the same slot on the same disk belong to one entry |

An entry is assembled from one group: the kernel image (or loader), at most one
initrd, at most one device tree. **Parameters are never generated.** They are
the `options` line, copied verbatim, of the candidate the model labelled
"entry text" in the same group (a Boot Loader Specification `.conf` file); with
none, the command line is empty. Linux entered directly with the MMU off stays
out of scope (K2d).

The decision is written as entries in the format of ADR 0025 (`entry`, `linux`,
`cmdline`, `initrd`, `chainload`). The same parser, the same format checks and
the same menu then apply, and the entries the model writes are limited to the
seven the menu has room for. The built-in entries for our own kernel stay first
and never depend on the model (ADR 0009, point 1).

**Not expressible in ADR 0025 today:** a device tree (there is no key for it),
a kernel or loader on another partition such as XBOOTLDR (ADR 0025 supports the
ESP only), and our own kernel as a model-written entry (it is built in). Until
an ADR that amends ADR 0025 adds them, the loader ignores the `device tree`
role, and XBOOTLDR is read only to take an `options` line for an entry whose
files are on the ESP; B3 writes that amendment if the corpus shows it is needed.

### 4. Validation before the jump

Deterministic code validates every decision (ADR 0002, ADR 0009 points 3 and
4); it never repairs one:

1. Every label decodes through an exhaustive `match`; a score vector that names
   no valid class, a group with two kernels, a protocol with no kernel image or
   an index outside the candidate list is rejected.
2. Every path comes from the enumeration and is opened again at load time.
3. The format check of the chosen protocol runs on the bytes that will be
   loaded: ELF and the handoff checks of ADR 0013 for our kernel; the PE check
   of ADR 0025 for the stub and for the chain-load; the arm64 `Image` magic
   (`0x644d5241` at offset 56, `Documentation/arch/arm64/booting.rst`) where
   the image is not a PE file.
4. A decision that fails ends as one line on the menu naming the check that
   failed, and the countdown stops; nothing is started (ADR 0009, point 4).

What the checks cannot see: a decision that is well-formed, passes every check
and is still wrong, such as starting a different valid kernel, or a Linux with an
empty command line and no console. The cost is a wrong but valid boot, which the
menu lets the user correct (ADR 0009, open question on corrections; the
entries of the model are plain text a person can overrule in `\WSOS.CFG`). A
disk crafted to steer the model can steer more than the choice of a kernel:
the initrd, the loader to chain-load and the `options` text are taken from
files on the ESP (the `options` line, also from XBOOTLDR), for example a
`.conf` file with `options init=/bin/sh`, or an extra initrd placed in front of
a genuine kernel. The model adds no file that is not on a partition it was given, but
it makes whatever a writable partition holds bootable from the menu; the
firmware trusts the ESP, and a removable disk's partition is not trusted just
because it is present. Which disks are enumerated, protecting the ESP and
signing the weights are B4's; until then B3 enumerates only the disk the
bootloader started from.

### 5. Inference in the UEFI environment

- **Where.** In the bootloader, before `ExitBootServices`, so the firmware reads
  the disk and no driver is needed. Never after the handoff.
- **CPU and memory.** One core, no operating system. Weights and scratch come
  from `AllocatePages` as `LOADER_DATA`, once, with no allocation while a
  decision runs. The kernel's load range at `0x40200000` (ADR 0013) is reserved
  **before** the weights are allocated, so that the weights cannot take it; the
  pages are given back or left reclaimable at the handoff. The measurement
  allocated 4.0 MiB of weights for the classifier and 128 MiB plus 14 MiB for the larger
  alternative, in a 512 MiB machine, with no failure.
- **Floating point and SIMD.** The inference is integer arithmetic, but the
  compiler vectorises it with NEON, which `aarch64-unknown-uefi` enables by
  default (`rustc --print cfg` shows `target_feature="neon"`; B1-1, section 4).
  On QEMU's EDK2 the probe, running at EL1, read `CPACR_EL1 = 0x300000`
  (`FPEN` = `0b11`) and `FPCR = 0`, and ran NEON and fp32 code with no trap, as
  UEFI 2.10, section 2.3.6, requires. The compiler may already emit NEON
  anywhere in the bootloader (B1-1, section 4; no disassembly was read), so the
  bootloader may already depend on this. The kernel stays soft-float
  (`.cargo/config.toml`) and is not affected. On x86-64 (K14) the build needs a
  hard-float target; not decided here.
- **Time.** The firmware arms a **5 minute watchdog** before it starts a boot
  option and resets the machine when it fires (UEFI 2.10, 7.5.1, read from the
  Wayback snapshot of `07_Services_Boot_Services.html`, `20251207195607`). The
  worst case at the budget is 3.8 s on TCG, so the bootloader does not need to
  disable it; the measuring program does, because a series of any length must be measured without a reset.
  Inference runs before the menu, so it is under this watchdog (the menu
  disables and re-arms it itself, `bootloader/src/menu.rs`). The 3.8 s is inference only: reading the weights and the headers through the
  firmware (not measured here, and slow on a slow USB or SD device) comes on top,
  and B3 must measure it. **A deadline of 30 s** from the start of the weight
  read, checked between reads and between two candidates (each takes a few
  milliseconds on TCG at the reference shape), ends a run that overruns and
  takes the fallback of ADR 0009, point 3, well before the watchdog could
  reset the machine into the same inference again.
- **Runtime.** Hand-written Rust, about a hundred lines: an int8 matrix-vector
  product with sixteen independent lanes, a shift-and-clamp to int8 and an
  embedding lookup. No library, no `alloc`, no dependency. Every known runtime
  was ruled out in the research for `std`, a C library, a pre-release backend
  or a licence (B1-1, sections 4 and 6); this one is under our control and
  small enough to review.

### 6. Licences

- **Code.** Ours, GPL-2.0-only (ADR 0005). No new dependency, so
  `scripts/check-licences.py` has nothing new to accept.
- **Weights.** Ours, trained in B2, proposed under GPL-2.0-only like the code.
  The corpus they are trained on (headers of kernel images and EFI loaders of
  other systems) has its provenance recorded by B2. Whether a model trained on
  those files inherits their licences is the open legal question B1-1 listed
  (section 6); this ADR does not settle it and B2 may not release weights before
  it is answered.
- **Not used.** Magika (Apache-2.0 only, code cannot be linked, and its labels
  have no kernel image class), TensorFlow Lite for Microcontrollers
  (Apache-2.0 only), `efifs` (GPL-3.0-or-later) and the Llama 3.2 weights
  (a community licence, not read at source).

### 7. Where the weights live

In a file on the ESP, `\WSOS.MDL`, not inside the `.efi`:

- training a new model does not rebuild or re-sign the bootloader;
- the licence boundary stays clear (code and data are different files);
- a missing or unreadable file means no model: the bootloader offers the last
  decisions that led to a boot and the built-in entries (ADR 0009, points 1
  and 3).

The file starts with a magic, a format version, the shape and the length, and
ends with a CRC-32 against corruption. A CRC does not protect against
tampering: signing the weights is B4, and until B4 the model is as trusted as
the ESP (ADR 0009, Consequences). This item commits no weights and no large
file: the measurement uses synthetic weights. B2 produces the file and a script
with a pinned version and SHA-256 (like `scripts/fetch-test-kernel.sh`), and
that script, not this ADR, makes the gate fail when the file is missing.

## Measurement

### What was run

`bootloader/probe/inferprobe.rs` is a UEFI application of its own (a third
binary of the `bootloader` package, next to `chainprobe`), not part of the
bootloader and never loaded by it. It runs, in a release build, on a disk image
where it is the only program (`scripts/measure-boot-model.py`):

- **`classifier-candidate` and `classifier-disk`**: the reference shape of
  section 1, for one candidate and for 16 candidates (a disk with a kernel, an
  initrd, a loader and a few other files per system).
- **`slm-int8-token`**: the alternative of the next section, one decoding step of
  a language model with the shapes of SmolLM2-135M (30 layers, hidden 576,
  intermediate 1536, 3 key-value heads of 64, vocabulary 49152): every
  projection of every layer and the output projection, 134,479,872 int8
  weights (128 MiB) and as many multiply-accumulates.
- **`slm-fp32-layer`**: one layer of the same shapes in fp32 (3,538,944 weights),
  to see what fp32 costs next to int8.

**No trained weights exist** (B2 comes after this item), so every number is for
**a workload with the same shape and size as the candidate, with weights from a
fixed pseudo-random generator**. Time does not depend on the values of the
weights: the loops have no data-dependent branch. The SmolLM2 workload omits
attention (about 10 million multiply-accumulates at 300 tokens of context, 8 %
more), the normalisations, rotary embeddings and sampling. The checksums the
program prints show the work ran and, being integer or fixed-order fp32
arithmetic, are **identical on TCG and HVF**.

### How

Host: Apple M1 (4 performance and 4 efficiency cores), 8 cores, 8 GiB, macOS
27.2. QEMU 11.1.3 (Homebrew). Firmware: Homebrew `edk2-aarch64-code.fd`,
EDK2 stable 202408 (the version the firmware prints). `rustc` 1.100.0. The
probe runs at EL1 (the firmware did not start it at EL2), and its generic
timer counter runs at 62.5 MHz under TCG and 24 MHz under HVF.

```bash
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
python3 -I scripts/measure-boot-model.py --accel hvf --accel tcg --series 9 --reps 21
```

which builds the probe and runs, per series, this QEMU command line (TCG shown;
HVF replaces `-accel tcg -cpu cortex-a72` with `-accel hvf -cpu host`):

```bash
cargo build --release --target-dir target -p bootloader --target aarch64-unknown-uefi --bin inferprobe
python3 -I scripts/make-disk-image.py infer.img target/aarch64-unknown-uefi/release/inferprobe.efi --file '\INFER.CFG=INFER.CFG'   # INFER.CFG: reps 21, slm 1, warmup_ms 1000
qemu-system-aarch64 -machine virt,acpi=off -m 512M -nodefaults -display none -serial stdio -monitor none \
  -accel tcg -cpu cortex-a72 -bios /opt/homebrew/share/qemu/edk2-aarch64-code.fd \
  -drive file=infer.img,format=raw,if=virtio
```

Per series: one boot; a 1 s unmeasured warm-up of each workload; then 21
repetitions timed with the counter (`CNTVCT_EL0`); the median, quartiles and
extremes of the repetitions are printed with each repetition. The script reports
the **median over series of the median of each series**, and drops a series when
the load average at either end of the boot is above 4, when any workload's
interquartile range exceeds 10 % of its median, or when the program's clock and
the host's disagree by more than 10 %. The load limit is half the eight cores
(the M1 has four performance cores). The clock check detects a skewed counter,
not interference: a vCPU that the host did not schedule delays both clocks
alike. The rule does not remove a steady disturbance that stays under those
limits: the load average lags, a constant extra load gives a small spread, and
the number of other QEMU processes is printed but not used to drop a series. Memory: 512 MiB. One core is
used by the guest. The machine is shared: another session ran its own QEMU gate
during part of the HVF run (recorded as "other qemu" in the series lines).

### Results (microseconds; 9 series of 21 repetitions each)

| Workload | TCG, cortex-a72 (9 of 9 series kept) | HVF, host CPU (7 of 9 series kept) |
|---|---:|---:|
| classifier, one candidate | 3,870 (series medians 3,856 to 4,000) | 172 (171 to 173) |
| classifier, 16 candidates (one disk) | 61,980 (61,682 to 63,937) | 2,772 (2,759 to 2,816) |
| SLM int8, one token | 126,977 (125,846 to 129,850) | 4,430 (4,413 to 4,777) |
| SLM fp32, one layer | 19,710, or 29,468 in 3 of 9 series | 357 (346 to 369) |

Load average at the start of the series was 1.9 to 2.5 on TCG and 2.2 to 4.0 on
HVF. The two HVF series dropped had the widest spread in the fp32 layer and the
int8 token (IQR 32 % and 15 % of the median), with the load at 3.1 to 3.6. The
program's clock agrees with the host's: the host time of a series divided by
the counter's was 1.00 to 1.03 (TCG) and 1.00 (HVF).

A second run of the same command after the review fixes (the counter read lost
its `nomem` promise), at a busier moment (load 3.0 to 4.0), kept 4 of 9 HVF
series and 7 of 9 TCG series and gave the same picture: HVF 180, 2,798, 358 and
4,671 us, TCG 3,878, 62,850, 29,310 (19,692 in the quietest series) and 127,762
us, in the order of the table. The table is the quieter first run, taken with commit 3e0c67a's probe (before
the `nomem` change); the second run used the probe of the later commits, whose
code is the final one. Rerunning the documented command on the final commit
gives numbers within about 5 % of the table, not identical to it.

**Derived (arithmetic on the numbers above, not measured):**

| | TCG | HVF |
|---|---:|---:|
| throughput, int8 | 1.1 G MAC/s | 24 to 30 G MAC/s |
| the language model's decision: 300 prompt tokens and 60 generated, 360 token steps, no batching of the prompt | about 46 s | about 1.6 s |
| the language model in fp32, one token (30 layers and the output projection, scaled from the one-layer time) | not computed (the fp32 layer is bimodal) | about 13 ms taking the one-layer time at face value; a rough indicator only: the layer (14 MB) is run repeatedly, the real model streams 513 MiB, and the figure is neither a floor nor a ceiling |
| memory of the language model's weights | 128 MiB int8, 513 MiB fp32 | same |

### What each number is, and what it is not

- **Neither is hardware time.** No number here is the time on a real machine.
- **TCG** emulates the guest instruction by instruction on the host; it models
  no cache and no TLB (ADR 0019, "Known limits") and its relative costs are not
  those of any physical core. What it shows is that even through emulation the
  classifier decides a disk in 62 ms and the language model needs about 46 s,
  a ratio of about 700 to 1 that is close to the ratio of the work (360 token
  steps of 134 million multiply-accumulates against 16 candidates of 4.2
  million). It runs on every runner, so it is the one the gate uses.
- **HVF** runs the guest at native speed on the M1's cores, with its large
  caches. It is a **best case**, faster than most boards a bootloader will meet.
  It does not tell how a smaller core or a slower memory behaves; the 4 MiB
  classifier fits the M1's caches, the 128 MiB model does not, and the
  difference between them on a board with a slower memory is not measured.
- The compiler targets `+v8a`, without the dot-product instructions the M1 and
  recent cores have, so the int8 numbers are not the best a tuned kernel would
  reach.
- **Not measured**: reading the weights from the ESP (128 MiB through the
  firmware's FAT driver and virtio is a cost of its own, not timed here), the
  enumeration of the disk and the read of 512 bytes of each candidate, the
  tokenisation and the grammar of a language model, attention, any x86-64 or
  real aarch64 board, and **accuracy**, which is B2's.
- **Two measurement effects found on the way.** (1) On HVF the first 20 or so
  repetitions of the first run ran twice as slow as the rest (the classifier
  went from 5.9 to 2.8 ms over one series, an idle core ramping up); the
  1 s warm-up exists because of that. (2) On TCG the fp32 layer takes either
  19.5 or about 29 to 30 ms, constant within a boot and different between boots (6 and 3
  of 9); the integer workloads did not do this. The cause was not investigated.
  The fp32 figures are therefore the least reliable, and the decision does not
  rest on them.
- **Interference.** Another session's gate ran QEMU at the same time during
  earlier attempts: a TCG series taken when the load average rose from 3 to 13
  gave a language-model token of 147 ms (median over series) with one series at
  368 ms, against 127 ms at a quiet machine. The script now waits for the load
  to settle and drops disturbed series, and those attempts are not in the table.
- **The gate checks the program, not the numbers.** `./scripts/check.sh` builds
  the probe in release, boots it once on TCG with two repetitions, no warm-up
  and every workload, and requires the four checksums to equal the ones in
  `scripts/measure-boot-model.py` and the repetitions to be two (so the
  configuration was read); it fails if the arithmetic
  changed, if the firmware is missing, or if the program does not finish. It
  uses `-cpu cortex-a72`, not `-cpu max` (ADR 0026).

## Alternatives considered

| Option | For | Against |
|---|---|---|
| A language model, SmolLM2-135M class, int8, with its output constrained to a grammar | Reads a listing as text; can generalise to layouts nobody labelled; Apache-2.0 weights exist (B1-1, section 3) | Measured: 128 MiB of weights (does not fit the 64 MiB image the tooling writes today), 127 ms a token on TCG and 4.4 ms on HVF, so about 46 s and 1.6 s a decision (derived); a grammar engine and a tokeniser to write and review in the root of trust; a grammar guarantees syntax, not that a path exists; free-text parameters; a stronger model than the closed decision needs |
| A purpose-trained classifier over enumerated candidates (**chosen**) | Every output is a closed label, so validation is exhaustive and total; 4 MiB, 62 ms on TCG and 2.8 ms on HVF a disk (measured); an inference engine small enough to read in one sitting; no tokeniser, no grammar, no free text | Needs a labelled corpus (B2); cannot invent a command line; sees only what the enumeration gives it (FAT, partitions); a new kind of input (a layout never seen) is a wrong label, not a reasoned guess |
| A tree ensemble over bytes | Smaller and faster still; trainers (LightGBM, `linfa`, MIT) exist | Size and time were not read or measured (B1-1, section 3); hand-made features would be the rule-based scanner ADR 0009 rules out, and raw bytes are weak for trees. **Kept open for B2** as a comparison inside the same budget and the same decision type |
| Magika | Trained, small, fast (vendor claims) | Apache-2.0-only code cannot be linked (ADR 0005); no label for a kernel image or an initrd (B1-1, section 1) |
| No model, signatures and standard entries (ADR 0009, first alternative) | Deterministic | Already rejected by ADR 0009; not reopened here |

The language model remains possible later: the typed decision, the validation
and the entry format do not depend on how the labels are produced. A new ADR
that supersedes this one would swap the producer.

## Consequences

- One new binary in the `bootloader` package, `inferprobe`, and one script,
  `scripts/measure-boot-model.py`. The bootloader's own source, the kernel and
  the boot protocols do not change. The gate gains one step, and `check.sh`
  builds the probe in release, which compiles the `uefi` crate once more.
- B2 starts from this budget and this decision type: it must produce labels
  for the fields of section 3, a model within the budget, a `\WSOS.MDL` file and
  a script that fetches it with a pinned version and SHA-256, and report the
  accuracy on a held-out part of the corpus.
- B3 writes the enumeration, the loader of `\WSOS.MDL`, the validation of
  section 4 and the entries; it measures again on the real code, on at least
  one machine that is not an M1 under QEMU if one is at hand, and shows the
  model's boot time next to the stored-decision boot (ADR 0009, point 2).
- The memory the classifier needs is 4.0 MiB of weights in the reference shape and at most
  18 MiB at the budget; the language model would have added 128 MiB to a bootloader that
  already runs in 256 MiB in the gate and needs 2048 MiB on VirtualBox (`docs/booting-the-image.md`).

## Known limits

- Every time is for a stand-in workload with synthetic weights, on one machine,
  under emulation or virtualisation. None is hardware time and none says that a
  trained model of this shape decides correctly.
- The reference shape is a plausible one, chosen before any corpus exists. If B2
  needs a larger window, more outputs or a deeper network, the budget (four
  times the reference) is what limits it, and the budget is this ADR's to
  change, with new measurements.
- The enumeration reads only FAT. A Linux whose kernel is on ext4 or another
  filesystem the firmware cannot read is not found. A filesystem driver is a
  separate decision with its own licence question (ADR 0005, ADR 0008).
- The effect of the firmware not enabling FP and SIMD was not tested: the
  firmware used here enables them. A NEON instruction on a firmware that did not
  would fault in the firmware's handler. The bootloader may already depend on this.
- A "valid but wrong" decision (section 4) is not caught by the loader.
- The cause of the bimodal fp32 time on TCG is unknown.
- The weights' licensing and the integrity of the weights are open (section 6
  and B4).
