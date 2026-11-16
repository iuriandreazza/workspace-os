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
   where no model is loaded. The profiles offered are:
   - direct and technical: short, precise answers;
   - warm and didactic: explains what it is doing and why;
   - formal and discreet: professional, speaks only when needed;
   - good-humoured: light, with a marked character, still precise.
2. **One SOUL per machine.** The SOUL belongs to the system, not to a user
   account: it is the same AI for every user of the machine.
3. **A personality per user, built on the SOUL.** For each user the AI may
   have a personality: the way that one SOUL shows itself to that person. A
   personality uses the SOUL and cannot contradict it. Each user shapes
   their own, without root authority; the AI may propose adjustments, and
   only that user approves them. A personality may vary:
   - the tone of writing, humour and formality;
   - the spoken voice;
   - the language;
   - the level of detail of the answers;
   - the name that user calls the AI by.
4. **The AI may propose changes to its own SOUL, and only a user with root
   authority can approve them.** A change is a proposal shown in full and
   accepted or rejected. Every applied change is kept in a history and can be
   undone. A user without root authority cannot change the SOUL. Root
   authority is defined in ADR 0007.

## The limit that makes this safe

The SOUL and the voice govern presentation. They never govern authority.

- Nothing written in the SOUL can grant, widen or remove a capability. The
  capability broker validates every action against the user's capabilities
  and does not read the SOUL.
- A SOUL change is itself an action in the plan format, with its own
  capability, held only by users with root authority, so it goes through the
  broker like any other action.
- A personality is presentation too: it cannot grant a capability either, and
  one user's personality never applies to another user.
- The deterministic fallback shell has no persona. The system stays fully
  usable without the model, the SOUL or the voice (ADR 0002).
- The same request must lead to the same validated actions under any SOUL
  and any personality. This is tested, not assumed.

## Alternatives considered

| Topic | Chosen | Not chosen, and why |
|---|---|---|
| What "voice" covers | Speech and writing tone | Writing tone only: simpler, but the system would have no spoken presence. Speech only: leaves the writing style undefined |
| How the SOUL is defined | Interview plus presets | Presets only: predictable but impersonal. Free text: most freedom, and the easiest way to smuggle in instructions about what the AI may do |
| Change after installation | The AI proposes, a user with root authority approves | Only the user edits: safer, but the AI cannot adapt. Fixed until reinstall: stable and inflexible. Any user approves: one account could reshape the AI everyone else uses |
| Scope | One SOUL per machine, with a personality per user on top | One SOUL per user: more personal, but the machine loses a single identity. One SOUL and nothing per user: every user gets exactly the same manner |

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

- With a different name per user, what name the system itself goes by: in
  the SOUL, on the login screen, and when it speaks to several people.
- How the system knows which user is speaking, so that it answers with that
  user's personality and voice. The voiceprint enrolled for root confirmation
  (ADR 0007) is the natural candidate; recognising a speaker to pick a
  personality needs far less certainty than confirming a root action.
- Which local speech models to use, and their licences (ADR 0005).
