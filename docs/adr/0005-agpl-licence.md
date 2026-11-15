---
title: "ADR 0005: AGPL-3.0 licence"
type: adr
status: accepted
created: 2026-11-15
updated: 2026-11-15
tags: [licence, agpl, gpl, drivers]
---

# ADR 0005: AGPL-3.0 licence

## Context

The repository is public and had no licence, so nobody had permission to use
the code. Drivers are planned to be converted from Linux (ADR 0003), and Linux
is licensed GPL-2.0-only.

## Decision

License the OS under the GNU Affero General Public License, version 3 only
(`AGPL-3.0-only`).

"Version 3 only" is the conservative choice: the copyright holder can add
"or later" in the future, but cannot take it back for versions already
released.

## Consequences

- Anyone who distributes the OS, or offers it to users over a network, has to
  publish their modified source under the same licence.
- **GPL-2.0-only code cannot be combined with AGPL-3.0 code in one program.**
  The two licences are incompatible. A driver converted from Linux is a
  derivative of GPL-2.0-only code, so it cannot be relicensed as AGPL-3.0 or
  linked into AGPL-3.0 code.
- The microkernel design is what makes this workable: each driver is a
  separate program that talks to the rest of the system over IPC, so a
  converted driver can stay GPL-2.0 while the OS is AGPL-3.0.

## Open questions

- **Driver support library.** Whatever a converted driver links against
  (the native driver API runtime) is combined with GPL-2.0-only code, so it
  cannot be AGPL-3.0. It needs a GPL-2.0-compatible licence, for example a
  permissive one. Not decided yet.
- **Legal confirmation.** Whether IPC is a sufficient boundary between
  separate works is a legal judgement. It needs review by a lawyer before
  converted drivers are distributed.
