---
title: "ADR 0007: Linux-style user permissions over kernel capabilities"
type: adr
status: accepted
created: 2026-11-15
updated: 2026-11-15
tags: [permissions, users, root, capabilities, security]
---

# ADR 0007: Linux-style user permissions over kernel capabilities

## Context

The system needs a rule for what each user may do. ADR 0006 already depends
on one: only a user with root authority approves a change to the SOUL. The
kernel is a capability-based microkernel (ADR 0001), which says how a program
is allowed to use a resource but not who the people are.

Linux programs, which the system intends to run, and Linux drivers, which it
intends to convert, are written against the Linux model of users and
permissions.

## Decision

Use the Linux model for the user permission matrix:

- users and groups, identified by numeric ids;
- an owner, a group and read, write and execute bits on every file;
- a root user, id 0, with authority over the whole machine;
- an explicit, confirmed step for an ordinary user to act with root
  authority, in the style of `sudo`: only members of an administrators group
  may do it, they confirm with their own password, and every use is logged
  under the name of the person who did it. Nobody logs in as root directly.

The two models sit in different layers and do not replace each other:

| Layer | Model | Answers |
|---|---|---|
| Kernel | Capabilities (ADR 0001) | May this program use this object? |
| System servers | Linux users, groups and permission bits | May this person do this? |

The servers that own resources (the filesystem server first) keep the user
and group ids and the permission bits, and enforce them. A program started
for a user receives only capabilities that the user's permissions allow. The
kernel never learns what a user is.

"Root authority" in ADR 0006 means the root user of this model.

## What this does not cover: the AI acts with less than the user

In the Linux model every program a user runs has all of that user's rights.
That is too much for an AI that turns natural language into actions: one
misread sentence would carry the user's full authority.

So the intent shell runs with the user's permissions as an upper bound, and
the capability broker (ADR 0002) narrows it further for each action. What the
AI may do is the intersection of what the user may do and what the action's
capability allows, never the union.

## The AI and root authority

The AI may act with root authority, with confirmation:

- An action that needs root is marked as such in the plan, and the broker
  holds it until a person who is allowed to act as root confirms it.
- The confirmation shows the exact action that will run, not a summary
  written by the model.
- Confirmation is per action. There is no standing grant: the AI never keeps
  root authority between actions.
- Every root action taken through the AI is logged with who confirmed it.

The risk this accepts is habit: a person who confirms often stops reading.
Showing the exact action and never offering "always allow" are the
mitigations.

### Confirming a root action by voice

A root action can be confirmed by voice, with three things together:

1. **Voiceprint.** The speaker is checked against the voiceprint enrolled for
   a user who may act as root.
2. **Random challenge.** The person repeats a phrase that is different every
   time, so an old recording does not pass.
3. **Second factor.** The person's own password is still required. The voice
   is never the only barrier to root. A PIN or a physical key may be added
   later.

Why all three: a voiceprint alone is weak. A recording replays it, a cloned
voice imitates it, and this system has a loudspeaker and its own speech
synthesis, so audio it plays could try to confirm an action. The challenge
defeats recordings but not a voice cloned in real time; the second factor
covers that.

Rules that follow:

- The system never accepts as confirmation audio that it is playing itself.
- Voiceprints are biometric data: they are stored only on the machine,
  encrypted, and are never sent anywhere.
- Confirming by typing, with the second factor, always remains available, for
  people who cannot or do not want to use their voice, and for noisy places.

## Recovery

Recovery follows what Linux does. When no administrator can authenticate,
the machine is recovered by someone with physical access to it:

- the boot menu offers a recovery mode, or the machine is started from the
  installation media;
- that gives a root shell on the installed system, where a new administrator
  password is set;
- recovery mode runs the deterministic shell only. The AI, the SOUL and voice
  confirmation are not loaded, so recovery never depends on a model.

As on Linux, this means physical access to the machine is authority over it.
The defence Linux uses is disk encryption with a recovery key, and a
password on the boot menu: without the key the disk cannot be read from
recovery mode or from other media.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| Linux model over capabilities (chosen) | Familiar; what Linux programs and drivers expect; gives ADR 0006 its definition of root | Coarse on its own: a program has all of its user's rights. Needs the broker for the AI |
| Capabilities only, no users or groups | Fine-grained, one mechanism | Unfamiliar; every Linux program would need its permission checks translated |
| A new permission model | Could be designed around the AI from the start | Nothing to reuse, nothing to compare against, and the Linux personality would still need the Linux model |

## Consequences

- The filesystem server stores an owner, a group and permission bits, and
  the Linux personality maps its permission calls straight onto them.
- Access control lists, Linux capability bits and mandatory access control
  are left out until something needs them.
- The capability design (roadmap item K9) has to let a server tie a
  capability to the user it was issued for.

## Open questions

- Whether the disk is encrypted by default, with a recovery key created
  during installation. Without it, recovery mode lets anyone with physical
  access take over the machine and change the SOUL.
- Which local speaker-verification model to use, its error rates, and its
  licence (ADR 0005).
