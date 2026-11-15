---
title: "ADR 0002: Models as control plane over a deterministic core"
type: adr
status: accepted
created: 2026-11-15
updated: 2026-11-15
tags: [ai, slm, policy, architecture]
---

# ADR 0002: Models as control plane over a deterministic core

## Context

The goal is an OS operated by models instead of only deterministic programs.
Kernel mechanism (context switch, page tables, interrupt handling) has a budget
of nanoseconds to microseconds and must be verifiable; model inference is
slower and probabilistic.

## Decision

Use two kinds of model, both outside the trusted core:

| | Intent shell | Policy plane |
|---|---|---|
| Model | Small language model (SLM) | Purpose-trained models, not language models |
| Role | Turns natural language into a structured action plan and orchestrates programs | Tunes scheduling, memory, I/O and power policy from kernel telemetry |
| Latency budget | Hundreds of milliseconds to seconds | Microseconds, or asynchronous batched decisions |
| Fallback | Classic deterministic shell | Fixed heuristic |

Rules that hold for both:

1. The model proposes, deterministic code decides. The intent shell never
   executes anything directly: a capability broker validates each action. The
   policy plane only adjusts parameters within limits enforced by the kernel.
2. The system boots and works with no model loaded.
3. Every model decision is logged, for debugging and retraining.

## Alternatives considered

- A model inside the kernel mechanism: rejected. It is too slow, not
  deterministic and cannot be verified; an error there corrupts memory.

## Consequences

- The kernel needs a telemetry interface and a bounded policy interface.
- A capability broker is required before the intent shell can act.
- Model quality affects performance and usability, never integrity.
