---
title: "ADR 0005: GPL-2.0 licence"
type: adr
status: accepted
created: 2026-11-15
updated: 2026-11-15
tags: [licence, gpl, drivers]
---

# ADR 0005: GPL-2.0 licence

## Context

The repository is public and had no licence, so nobody had permission to use
the code. Drivers are planned to be converted from Linux (ADR 0003), and Linux
is licensed GPL-2.0-only. A driver converted from Linux is a derivative of
GPL-2.0-only code.

## Decision

License the OS under the GNU General Public License, version 2 only
(`GPL-2.0-only`), the same licence as the Linux kernel.

"Version 2 only" is the conservative choice: the copyright holder can add
"or later" in the future, but cannot take it back for versions already
released.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| AGPL-3.0 | Also covers offering the OS to users over a network | Incompatible with GPL-2.0-only: converted drivers could not be linked with OS code, and the library they link against would need a different licence |
| GPL-2.0-only (chosen) | Same licence as the drivers' origin: converted drivers, their support library and the OS can be combined freely | No network clause; incompatible with GPL-3.0, AGPL-3.0 and Apache-2.0 code |

## Consequences

- Converted Linux drivers keep their original licence and can link against OS
  code. The licence question no longer depends on IPC being a boundary between
  separate works.
- Anyone who distributes the OS has to publish their modified source under the
  same licence.
- Third-party code brought into the OS must be GPL-2.0 compatible. Code that
  is GPL-3.0, AGPL-3.0 or Apache-2.0 only cannot be linked in. Rust crates
  dual-licensed MIT/Apache-2.0 are used under their MIT terms.
- Model weights shipped with the OS carry their own licences, which have to be
  checked case by case.
