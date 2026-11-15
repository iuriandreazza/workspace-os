---
title: "ADR 0006: System persona, the SOUL and the voice, defined at installation"
type: adr
status: accepted
created: 2026-11-15
updated: 2026-11-15
tags: [persona, soul, voice, installer, slm]
---

# ADR 0006: System persona, the SOUL and the voice, defined at installation

## Context

The intent shell (ADR 0002) is how the user talks to the operating system. The
AI behind it represents the system, so how it behaves should not be an
accident of the model: it should be chosen by the person who installs it.

## Decision

During installation the system defines two things for the AI that represents
it:

- **SOUL**: who the AI is. Its identity, values, and how it relates to the
  user.
- **Voice**: how it expresses itself, in two senses: the tone of what it
  writes, and the synthesised voice it speaks with. Speech covers both
  directions, speaking and listening, and runs locally.

How they are defined and kept:

1. **Guided interview plus presets.** At installation the AI asks a few
   questions and builds the SOUL from the answers. Someone who does not want
   the interview picks a ready-made profile. The presets also cover the case
   where no model is loaded.
2. **One per machine.** The SOUL belongs to the system, not to a user
   account: every user of the machine meets the same AI.
3. **The AI may propose changes to its own SOUL, and only the user's approval
   applies them.** A change is a proposal the user sees in full and accepts
   or rejects. Every applied change is kept in a history and can be undone.

## The limit that makes this safe

The SOUL and the voice govern presentation. They never govern authority.

- Nothing written in the SOUL can grant, widen or remove a capability. The
  capability broker validates every action against the user's capabilities
  and does not read the SOUL.
- A SOUL change is itself an action in the plan format, with its own
  capability, so it goes through the broker like any other action.
- The deterministic fallback shell has no persona. The system stays fully
  usable without the model, the SOUL or the voice (ADR 0002).
- The same request must lead to the same validated actions under any SOUL.
  This is tested, not assumed.

## Alternatives considered

| Topic | Chosen | Not chosen, and why |
|---|---|---|
| What "voice" covers | Speech and writing tone | Writing tone only: simpler, but the system would have no spoken presence. Speech only: leaves the writing style undefined |
| How the SOUL is defined | Interview plus presets | Presets only: predictable but impersonal. Free text: most freedom, and the easiest way to smuggle in instructions about what the AI may do |
| Change after installation | The AI proposes, the user approves | Only the user edits: safer, but the AI cannot adapt. Fixed until reinstall: stable and inflexible |
| Scope | One per machine | One per user: more personal, but the machine loses a single identity. Machine base plus per-user tweaks: more moving parts for little gain now |

## Consequences

- The installer needs a first-boot flow that can run the interview, with and
  without a model.
- The plan format (roadmap item S2) needs an action for proposing a SOUL
  change.
- Local speech synthesis and recognition become requirements, and with them
  audio drivers inside the OS. On the development machine they can be
  prototyped long before that.
- An AI that can propose changes to its own persona can drift, or be talked
  into proposing a change by text it reads. Approval by the user, the full
  diff shown, and the history with undo are the mitigations.

## Open questions

- On a machine with several users, who may approve a SOUL change.
- Which local speech models to use, and their licences (ADR 0005).
- What the presets are.
