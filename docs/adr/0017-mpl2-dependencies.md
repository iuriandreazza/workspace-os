---
title: "ADR 0017: MPL-2.0 dependencies in a GPL-2.0-only system"
type: adr
status: accepted
created: 2026-11-16
updated: 2026-11-16
tags: [licence, gpl, mpl, dependencies, bootloader]
---

# ADR 0017: MPL-2.0 dependencies in a GPL-2.0-only system

## Context

ADR 0005 licenses the OS under `GPL-2.0-only` and requires third-party code to
be GPL-2.0 compatible. It names the licences that cannot be linked in (GPL-3.0,
AGPL-3.0, Apache-2.0 alone) and how dual MIT/Apache-2.0 crates are used. It
does not mention the Mozilla Public License.

The UEFI bootloader (K2b-2, ADR 0013) depends on the `uefi` crate, and `uefi`
0.41.0 depends unconditionally on `ucs2` 0.3.3, which is `MPL-2.0`. Its code is
linked into the bootloader's `.efi`. The pull request that added it accepted
the dependency on the reading below, and recorded that ADR 0005 was silent.
This ADR records the rule so the next MPL dependency is not decided ad hoc.

What the MPL-2.0 text says, read in the `LICENSE` file shipped with `ucs2`
0.3.3:

- Section 1.12 lists GPL 2.0, LGPL 2.1 and AGPL 3.0, and later versions of
  them, as Secondary Licences.
- Section 3.3 allows a Larger Work that combines MPL code with code under a
  Secondary Licence to be distributed under that Secondary Licence, unless the
  MPL code is "Incompatible With Secondary Licenses".
- Section 1.5 defines that as code carrying the Exhibit B notice, or code that
  was released only under MPL 1.1 or earlier.
- Section 3.2 requires that whoever distributes an executable tells the
  recipients how to obtain the source of the MPL-covered files.

`ucs2` 0.3.3 carries no Exhibit B notice in its sources, and its manifest
declares `MPL-2.0`.

## Decision

A dependency under `MPL-2.0` may be linked into the OS, the bootloader and
their tools when all of these hold:

1. Its declared licence is MPL version 2.0. MPL 1.1 and earlier are refused.
2. Neither its sources nor its licence file apply the Exhibit B notice
   ("Incompatible With Secondary Licenses"). The check is made on the exact
   version in `Cargo.lock`, and again when that version changes.
3. Its files keep their MPL-2.0 licence. If we ever modify them, the modified
   files stay under MPL-2.0 and are published.
4. The pull request that adds it names the crate, the version and the result
   of check 2 in its description, and adds a row to the table below.

The combined binary is distributed under `GPL-2.0-only`, as section 3.3
permits.

When a release distributes binaries (a disk image with the bootloader), the
release states where the source of every MPL-2.0 dependency is obtained: the
crate name and version from `Cargo.lock`, on crates.io. GPL-2.0 already
requires the source of the whole work, so this is one more line in the same
notice, not a second mechanism.

MPL-2.0 dependencies accepted so far:

| Crate | Version | Linked into | Brought in by | Exhibit B notice |
|---|---|---|---|---|
| `ucs2` | 0.3.3 | `bootloader` | `uefi` 0.41.0 | absent |

## Alternatives considered

| Option | For | Against |
|---|---|---|
| Accept MPL-2.0 under the conditions above (chosen) | The licence text names GPL 2.0 as a Secondary Licence; keeps the `uefi` crate and its safe API; no code to write or maintain | One more licence to track per dependency; a source notice is owed when binaries are released |
| Refuse MPL; replace `uefi` with `uefi-raw` and our own wrappers | Only MIT code linked into the bootloader | `ucs2` is not optional in `uefi`, so the whole safe layer goes: more `unsafe` written by us around boot services, the memory map and file protocols, for a licence that is already compatible |
| Refuse MPL; fork `uefi` without `ucs2` | Keeps most of the safe API | A fork to maintain across `uefi` releases, to remove about 350 lines of string conversion |

## Consequences

- ADR 0005 stands. This ADR adds MPL-2.0 to the licences that can be linked
  in, under conditions; it does not change the licence of the OS.
- This is a reading of the licence text by the project, not legal advice. It
  joins the legal review already listed as an open item in the roadmap.
- The check is manual, done in review (CLAUDE.md lists licence problems among
  what the adversarial reviewer looks for). There is no automated licence
  check in `scripts/check.sh`; adding one is a separate piece of work.
- No binary is distributed today: the disk image is built locally and never
  committed. The source notice becomes due with the first released image.
- An upgrade of an MPL-2.0 dependency repeats check 2 and updates its row.
