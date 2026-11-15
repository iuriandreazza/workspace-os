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
  authority.

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

- How an ordinary user is granted root authority for one action: the
  mechanism and what the user has to confirm.
- Whether a confirmation given by voice is enough for a root action, given
  that each user has their own spoken voice (ADR 0006).
