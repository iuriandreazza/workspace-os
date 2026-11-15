---
title: "ADR 0001: Own microkernel written in Rust"
type: adr
status: accepted
created: 2026-11-15
updated: 2026-11-15
tags: [kernel, microkernel, rust]
---

# ADR 0001: Own microkernel written in Rust

## Context

The project aims at an operational OS where models take part in operating the
system. Models are probabilistic, so everything they influence needs a small,
deterministic core that enforces limits. Drivers are expected to be converted
from Linux by a model (see ADR 0003), so a faulty driver must not be able to
take the system down.

## Decision

Build our own microkernel in Rust. The kernel keeps only mechanism: address
spaces, threads, IPC, capabilities and interrupt routing. Drivers, filesystems,
network stack and foreign-ABI personalities run as isolated userspace servers.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| Linux as the base | Drivers and Linux ABI for free; policy models pluggable through `sched_ext`/eBPF; fastest path to results | Not the target architecture; the microkernel design is postponed |
| seL4 | Formally verified; capability model ready | Steep learning curve; its design decisions are inherited |
| Own microkernel in Rust (chosen) | Full control over the design; memory safety in the kernel; kernel work itself becomes article material | Months to years until stable; delays the work on the models |

## Consequences

- Driver and model faults are contained to a single process.
- Every hardware feature has to be built or converted; nothing comes for free.
- Architecture-specific code lives behind `kernel/src/arch` so a second
  architecture can be added without touching the core.
