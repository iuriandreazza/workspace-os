---
title: "ADR 0004: aarch64 as the first target architecture"
type: adr
status: accepted
created: 2026-11-15
updated: 2026-11-15
tags: [architecture, aarch64, qemu]
---

# ADR 0004: aarch64 as the first target architecture

## Context

The kernel needs one architecture to start with. Development happens on an
ARM64 Mac.

## Decision

Target aarch64 first, on the QEMU `virt` machine. x86_64 comes second.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| aarch64 (chosen) | Runs at native speed in a VM on the development machine; simpler boot path without x86 legacy | Less teaching material; common PCs and most Linux and Windows binaries are x86_64 |
| x86_64 | More documentation for Rust kernels; most common target hardware; more binaries for the compatibility goal | Runs emulated on the development machine, which is slow for testing models |

## Consequences

- Architecture-specific code is isolated in `kernel/src/arch/aarch64`.
- Running x86_64 programs will need binary translation until the x86_64 port
  exists.

## Update, 2026-11-15: first real hardware

The first real machine the system should run on is an x86_64 PC. aarch64
stays the first architecture, developed and tested under QEMU, and the
x86_64 port moves earlier in the roadmap (item K14) because real hardware
depends on it. Until that port exists the system only runs under QEMU.
