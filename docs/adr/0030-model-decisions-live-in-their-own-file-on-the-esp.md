---
title: "ADR 0030: The boot model's decisions live in their own file on the ESP"
type: adr
status: proposed
created: 2026-11-23
updated: 2026-11-23
tags: [boot, bootloader, uefi, model, amendment]
---

# ADR 0030: The boot model's decisions live in their own file on the ESP

## Context

ADR 0027 says the boot model writes its decisions as entries in the format of
ADR 0025, to a file of its own, and that an amendment of ADR 0025 must say how
the loader reads that file and how the entries share the menu. ADR 0025 said the
installer and the model "write the same file" as a person; ADR 0027 reverses
that, because the model would overwrite what a person wrote. This ADR is that
amendment. It changes nothing else in ADR 0025.

## Decision

1. **A second file.** The loader reads `\WSOS.DEC` from the root of the same
   ESP, after `\WSOS.CFG`. A person owns `\WSOS.CFG`; the model is the only
   writer of `\WSOS.DEC`. The model never writes `\WSOS.CFG`.
2. **Same entry syntax.** `\WSOS.DEC` uses the entry syntax of ADR 0025
   (`entry`, `linux`, `cmdline`, `initrd`, `chainload`) and the same parser, with
   one addition: a `disk <fingerprint>` line before the first entry. The
   fingerprint is computed from the disk's layout as ADR 0009 point 2 requires;
   its exact definition is B3's. An entry from a file whose fingerprint does not
   match the current disk is not shown.
3. **Seven entries in total.** The menu holds the two built-in entries, then the
   entries of `\WSOS.CFG`, then those of `\WSOS.DEC`, up to seven file entries
   in all (ADR 0025 counted `\WSOS.CFG` alone). When the seven are taken, the
   entries of `\WSOS.DEC` are the ones dropped, with one line saying so.
4. **A bad `\WSOS.DEC` never blocks anything.** A file with an error, a missing
   file or a fingerprint mismatch is ignored with one message naming the cause;
   the built-in entries, the entries of `\WSOS.CFG` and the countdown continue,
   as for a bad `\WSOS.CFG`.
5. **Every entry is checked again at load time**, by the PE check of ADR 0025,
   whoever wrote it. The model's entries get no extra trust.

## Not decided here

A device tree key and a kernel or loader on another partition (XBOOTLDR) stay
unsupported, as ADR 0027 states. B3 amends again only if the corpus of B2 shows
they are needed.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| The model appends to `\WSOS.CFG` | One file, one parser | It overwrites or reorders what a person wrote; an error in the model's part hides the person's entries |
| A second file (chosen) | A person's file is never touched; the model's file can be deleted to reset it; the same parser serves both | One more file read in the boot path, bounded and failing closed |
| UEFI variables for the decisions | No file on the ESP | The same objections ADR 0025 gave to `Boot####` variables |

## Consequences

- `bootinfo` reads two files, with one parser and one limit of seven.
- The fingerprint check, and what else `\WSOS.DEC` records, are specified by B3
  with tests on the host.
- ADR 0025 stays accepted; this ADR amends it and ADR 0027 points here.
