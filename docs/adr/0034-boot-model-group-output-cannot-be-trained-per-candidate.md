---
title: "ADR 0034: The boot model's group output cannot be learned one candidate at a time"
type: adr
status: proposed
created: 2026-11-23
updated: 2026-11-23
tags: [boot, bootloader, model, amendment, entries]
---

# ADR 0034: The boot model's group output cannot be learned one candidate at a time

## Context

ADR 0027, section 3, gives the model four closed outputs for every candidate: system
family, protocol, role and a **group**, "one of 16 slots; candidates with the same slot
on the same disk belong to one entry". The model reads one candidate window at a time
(section 2) and the groups are meant to assemble a kernel with its initrd, device tree
and entry text.

B2 trained that model (`docs/b2-boot-model-corpus-and-training.md`). Family, protocol and
role are functions of one candidate's window and train. The group is not: which slot an
entry gets is a property of the disk (how many entries it holds and how they are numbered),
not of any one file. Whatever numbering is used is
relative to the other files of the disk: the same `\boot\vmlinuz-virt` is slot 0 on a disk with one entry and slot 3
on a disk where three others sort before it, and nothing in its 512 bytes says which. The corpus generator numbers
entries in a random order, so the measurements below show the consequence of that, **not an empirical property of real
disks**; the argument rests on the structure, and a deterministic numbering was not trained.

This ADR changes no decision of ADR 0027 except that one output. It is `proposed`: it
changes how entries are assembled, which touches ADR 0009 ("no rule based scanner runs
beside the model to overrule it"), and that is the project owner's call.

## What B2 measured

All on the held-out `test` releases of the report, 17,198 candidates in 1,500 disks.

| Measure | Result |
|---|---|
| Group accuracy of the trained model (random numbering), files that belong to an entry only | 25.3 %. Counting every candidate it is 65.6 %, but that is the share of candidates in no entry, which are all given slot 15 |
| Entry windows (training set) seen with more than one slot | 270 of 7,274 (3.7 %), with identical bytes and name; a low count, because nearly every window carries a random version string in its path |
| Tree ensemble, same output | 57.7 % over every candidate, no better than the share of slot 15 |
| A pair head on the trunk's 128 features (is this file in the same entry as that kernel?) | precision 61.8 %, recall 38.4 %, 231 of 1,245 disks with every pair right |
| The rule "same directory", with the true roles given | precision 71.1 %, recall 75.8 %, 458 of 1,245 disks with every pair right |

The pair head and the directory rule are baselines, not designs. Neither is good enough to
build the loader on: entry text in the Boot Loader Specification layout lives in
`\loader\entries`, away from the kernel it names, and several layouts keep two kernels in
one directory.

## Decision proposed

1. **The group output is not used.** `\WSOS.MDL` keeps the 34-score layout (changing it
   would change the file for nothing), the group slice is trained with a low weight and
   B3 ignores it. ADR 0027's budget is unaffected.
2. **Entries are assembled by code, from the model's labels.** For every kernel image or
   loader the model labels, B3 attaches the initrd, device tree and entry text that its
   own text points at: the `linux`, `initrd` and `devicetree` lines of a Boot Loader
   Specification entry name paths exactly, and where there is no entry text, the files in
   the same directory with the same version suffix. The model decides what each file *is*;
   code decides which files go together, and the loader's checks (ADR 0027, section 4)
   still run on the result.
3. **ADR 0009's sentence about a rule-based scanner is read as being about detection.**
   Pairing files the model has already labelled is assembly, not a second detector that
   can overrule the model.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| Keep the 16-slot group as specified | Nothing to change | Measured 25 % on the files that matter; a wrong slot puts a kernel with the wrong initrd |
| Replace the group with a pair score trained jointly with the trunk | Keeps assembly inside the model | Not measured here: the baseline on frozen trunk features failed, a joint model needs its own corpus of pairs, its own validation and 256 candidates make up to tens of thousands of pairs |
| Assembly by code from the model's labels (proposed) | Deterministic, testable on the host, exact where the entry text names paths | A rule beside the model, which the owner must accept; layouts with two kernels in one directory need the version suffix rule, and neither was validated at corpus scale |
| Generate paths instead of choosing among candidates | Would not need pairing | Rejected by ADR 0027 itself: free text is what the typed decision avoids |

## Consequences

- If accepted, B3 specifies the assembly rule with tests on the host (BLS parsing and the
  same-directory rule) and measures it on layouts whose entry text names the files of the disk (the B2 generator
  does not write that yet) and, where one is at hand, on real disks. B2's report is the evidence that the rule is needed.
- If rejected, ADR 0027 stands as written and its group output has no known way to be
  trained; B3 cannot use it.
- The group slice stays in the file so a later model can use it without a format change.
