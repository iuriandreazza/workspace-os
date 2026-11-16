---
title: "ADR 0009: A small model in the bootloader detects and decides how to load kernels"
type: adr
status: accepted
created: 2026-11-15
updated: 2026-11-15
tags: [boot, bootloader, model, linux, multi-boot]
---

# ADR 0009: A small model in the bootloader detects and decides how to load kernels

## Context

The bootloader (ADR 0008) should not be limited to our kernel. It should find
and start other systems on the machine: Linux and others.

Finding kernels can be done with fixed rules, since each format has an exact
signature and systems announce themselves in standard places. The decision
taken here is to use a model instead, in line with the purpose of the
project: a model operating the system where deterministic programs used to.

## Decision

A small model runs inside the bootloader. It detects the kernels present on
the machine and decides how each one is loaded.

- **Small.** Smaller than the model of the intent shell (Bonsai 8B). Whether
  it is a language model or a purpose-trained classifier is settled by the
  research in roadmap item B1.
- **What it reads:** the partition layout, file names and paths, and the
  headers of candidate files.
- **What it decides, for each kernel found:** what system it is, which boot
  protocol applies, which companion files go with it (such as an initial
  ramdisk), and the parameters to pass.
- **What it does not do:** the loading itself. The bootloader implements a
  fixed set of boot protocols in ordinary code; the model chooses among them
  and fills in their inputs.

## How this fits ADR 0002

ADR 0002 says the system boots and works with no model loaded, and that a
model proposes while deterministic code decides. This decision puts a model
in the boot path, so the limits are stated here instead of left implicit.

1. **Our own system still boots without the model.** The installer writes
   the entry for our kernel when it installs it. The model is needed to find
   other kernels and new ones, not to start the system it belongs to.
2. **Decisions are remembered.** A decision is stored with a fingerprint of
   the disk contents it was made from. While nothing relevant changes, boot
   uses the stored decision and does not run the model, so two boots of the
   same disk behave the same.
3. **Fallback.** If the model is missing, fails or runs out of time, the
   bootloader offers the last decisions that led to a successful boot.
4. **The loader checks what it is told.** Before jumping, the loading code
   confirms that the file really is in the format the chosen protocol needs.
   A wrong decision ends in an error on the menu, not in a jump into data.

Within those limits the model is the one that detects and decides. No rule
based scanner runs beside it to overrule it.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| No model in the bootloader: detection by signatures and standard entries, natural language only in the shell after boot | Deterministic and verifiable in the root of trust; nothing new to build | Leaves the first stage of the system outside the idea the project is about |
| Advisory model: rules detect, the model only names and orders the entries | Keeps detection exact | The model adds an inference engine to the bootloader for little effect |
| The model detects and decides (chosen) | The boot stage follows the same idea as the rest of the system; can recognise layouts no rule was written for | A probabilistic component in the root of trust; needs its own inference engine; a wrong decision can leave a system unbootable until corrected |

## Consequences

- An inference engine has to run in the UEFI environment: no standard
  library, no GPU, memory from the firmware. None of the usual model
  runtimes work there.
- The model's weights become part of the boot chain. Someone who can replace
  them can change what the machine boots, so they need the same integrity
  protection as the bootloader itself.
- A training corpus is needed: disk layouts and kernel images from several
  systems.
- Boot time grows whenever the disk changed and the model has to run.
- Accuracy has to be measured on that corpus before the model is trusted,
  and the measurement belongs in the gate.

## Open questions

- Language model or purpose-trained classifier, and its size.
- Which boot protocols the loader implements first.
- How the weights are verified at boot.
- What the user sees, and can correct, when a decision is wrong.
