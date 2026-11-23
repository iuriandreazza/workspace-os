---
title: "ADR 0032: The SOUL is built at installation, and at the first use of a machine that has none"
type: adr
status: proposed
created: 2026-11-23
updated: 2026-11-23
tags: [soul, interview, installer, shell, first-run]
---

# ADR 0032: The SOUL is built at installation, and at the first use of a machine that has none

## Status

**Proposed.** The two rules in "Decision" are the user's, given in chat on
2026-11-23 ("during installation it must go through the guide, and if one
enters the intent shell and there is none, it must go through the process of
building a SOUL"). The details in "Decisions that are mine" are not yet
accepted by the user. It adds to ADR 0006 (the interview and the presets, and
that a SOUL is defined at installation) and ADR 0028 (the file the interview
writes). It supersedes nothing.

## Context

ADR 0006 says the SOUL is defined during installation by a guided interview or
a preset, with or without a model. S9 built that interview as a flow of the
host-prototype shell (`intent-shell --setup-soul`). There is no installer yet
(S12), and on the host a developer starts the shell directly. A machine that
has no SOUL today silently speaks as `direct-technical`, so nobody is ever
asked.

## Decision

1. **At installation** the installer runs the same interview, with the
   authority of the installer (root). The interview is one flow with one
   entry, `Task::run`; S12 calls it. Nothing about it is duplicated.
2. **When the intent shell starts on a machine that has no SOUL**, it builds
   one before the session, with the same interview, and then starts the
   session with it.

## Decisions that are mine (to accept or change)

| Case | Rule | Why |
|---|---|---|
| Who builds it | Only a user with root authority (`--administrator` on the host). Any other user starts with `direct-technical` and is told, at every start while there is none, that no SOUL is set and how a user with root authority sets it | ADR 0006 point 4 and ADR 0007: the SOUL is the machine's, and an ordinary user cannot write it |
| When "no SOUL" | No file at the default path and no S8a `voice-profile` beside it. A broken link at the SOUL path is a file that is there. A SOUL the person named with `--soul` is not "the machine's": if it is missing that is the error it already was | ADR 0028 (default path, legacy file) |
| Only when it would be read | When replies are on and there is a model. With `--no-model` or `--no-reply` nothing reads the SOUL, so nothing is asked (the deterministic shell stays usable and silent, ADR 0002) | ADR 0002, ADR 0021 |
| Only when someone can answer | Standard input and output are both a terminal. A script's lines are never taken for answers: it starts with `direct-technical`, as before | The existing scripted uses of the shell (tests, transcripts, pipes) must not change |
| What if it is not finished | Declined, cancelled, the input ending or a write error: the session starts with `direct-technical` and the interview is offered again the next time, because there is still no SOUL | Nothing is written without an explicit yes (S9) |
| In the interface | The interview runs on its own screen, ends with "Ctrl-D starts the shell", and the session opens after it | One flow, two adapters (ADR 0015) |
| Leaving | In the interface, Ctrl-C or Ctrl-D leaves the interview, which is declining it: the session then starts with the default, and the person leaves that too. With `--plain`, Ctrl-D (the input ending) declines it and Ctrl-C ends the shell, as it does in the plain session. A closed terminal or a termination signal during the interview screen ends the shell with status 1 | The interview is a step before the session, not the session |
| A named personality | `--personality FILE` on the command line means the person is using the shell, not setting it up: no interview, and the personality is checked against the SOUL there is (`direct-technical` when none) with the errors it always had | A personality that does not exist or contradicts the SOUL the interview would then write would fail only after the questions |
| What cannot be looked up | Only "not found" means no SOUL. A directory the user cannot read, or a path below a file, may hide one, so that is not first use and the shell's usual reporting of an unusable SOUL applies | A wrong "no SOUL" would interview over an existing SOUL's place |
| A transcript | `--transcript` needs the interface, and that is checked before anything is asked; the transcript holds the session, not the interview. Whether its path can be written is found when the session ends, as before | The need for an interface is known at the start; the rest is not this ADR's to move |
| After the first time | Never again: a SOUL exists, and changing it is S10 | ADR 0006 point 4 |

## Alternatives considered

| Option | For | Against |
|---|---|---|
| Interview for any user at first use | Nobody is left on the default | An ordinary user would write the machine's SOUL (ADR 0006 point 4) |
| Ask at every start while there is no SOUL, even without a model | Simplest rule | Nothing reads the SOUL without a model, and the deterministic shell would be interrupted for nothing (ADR 0002) |
| Take answers from a script's input | Scriptable | A script that expects the shell to read its lines as requests would have them consumed as answers |

## Consequences

- S12 (the first-boot flow) runs the interview as part of the installer; this
  ADR is what makes that a requirement and not a possibility.
- The host prototype has two ways in: `--setup-soul` on purpose, and the first
  start on a terminal. They share every line of the interview.
- An administrator who starts the shell with root authority only to run root
  actions is interviewed at every start until a SOUL exists. They can finish
  it in a minute with the preset path, or start with `--no-reply` or
  `--no-model`, which also turn the replies off. There is no flag that defers
  the question while keeping the replies, on purpose: a flag nobody needs is
  the first thing to remove (YAGNI).
- A machine whose only user lacks root authority keeps `direct-technical` and
  the one-line notice until a user with root authority sets it up.
