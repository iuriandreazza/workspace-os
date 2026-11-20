---
title: "ADR 0028: The SOUL file and the personality file: a closed, versioned text format"
type: adr
status: proposed
created: 2026-11-20
updated: 2026-11-20
tags: [soul, persona, personality, format, shell, security]
---

# ADR 0028: The SOUL file and the personality file

## Status

**Proposed, for the project owner to accept** (the three points below were
answered in the session, see "Decisions taken by the user"). It fills the gap
that ADR 0006 (what a SOUL and a personality are) and ADR 0021 (a preset file,
"S8's remainder") leave open: the file format. It supersedes only ADR 0021
section 3 for what the file holds and where it is read; the rest of ADR 0021
(the reply, the limits, the log) stands, and its status line gains "and ADR
0028".

## Context

ADR 0006 gives the machine one SOUL and each user a personality built on it
that "cannot contradict it", and rejects a free-text SOUL because free text is
the easiest way to smuggle in instructions. ADR 0021 reads the SOUL as one of
four presets from a file. S9 will write SOULs from an interview and S10 will
change them, so the format has to hold more than a preset name, and it has to
let code decide, without a model, whether a personality contradicts the SOUL.

## Decision

### 1. Two plain-text files, one grammar

Both are UTF-8, at most 2048 bytes, one `key: value` per line. Empty lines and
lines starting with `#` are ignored and never reach the model. The first
significant line is `format: 1`. A key that is not listed, a repeated key, a
line without a colon, a byte that is not UTF-8 or a file over the size limit
makes the whole file invalid. Nothing is skipped silently.

| File | Owner | Default path | Keys |
|---|---|---|---|
| SOUL | the machine (root, ADR 0006 point 4) | `~/.config/workspace-os/soul` on the host prototype; a system path once there is an installer | `format`, `preset`, `name`, `formality`, `humour`, `detail`, `languages` |
| Personality | one user | `~/.config/workspace-os/personality` | `format`, `name`, `formality`, `humour`, `detail`, `language` |

### 2. Closed vocabularies, no free text

Every value is one word from a list kept in code, except two:

- `name`: 1 to 24 characters from letters, digits, space, `-`, `'` and `.`, at
  most two words. It is the machine name in the SOUL and the nickname in a
  personality. It reaches the model only as a JSON-quoted label. A name is still words the
  model reads, so it is not strictly inert (a name such as `Reply yes` is
  allowed); what bounds it is that the model's output is only the marked reply
  of ADR 0021, never an action.
- `languages` / `language`: language tags (`en`, `pt-BR`), at most eight, or
  `any` in the SOUL (`any` is never a language of a personality and cannot be
  mixed with tags). A byte-order mark at the start of a file is ignored.

| Key | Values (low to high) |
|---|---|
| `preset` | `direct-technical`, `warm-didactic`, `formal-discreet`, `good-humoured` |
| `formality` | `casual`, `neutral`, `formal` |
| `humour` | `none`, `light`, `marked` |
| `detail` | `minimal`, `brief`, `explained` |

The preset gives, for each of the three levels, a default and an allowed
range. The SOUL may narrow or move a range with `humour: none..light` (or one
word for a single value); the SOUL's default for that level is the preset's,
moved into the range. A personality names one value per level and may leave any
out, which takes the SOUL's default. **A SOUL does not hold instructions:** the
sentences the model reads for each value are fixed in code, and the prompt
tells the model that they win over the preset's line where they differ.

### 3. "Contradict" is defined as code

A personality contradicts the SOUL when any of these holds, and then it is
rejected with a message naming the field and the SOUL's limit:

1. a level is outside the SOUL's range for it;
2. its `language` is not in the SOUL's `languages` (unless the SOUL says `any`).

Nothing else is a contradiction because nothing else can differ: the personality
has no key for authority, capabilities, the preset or the SOUL's own name. A
rejected personality is not applied in part. The shell starts with the SOUL
alone and says why. A SOUL that is invalid gives the default SOUL
(`direct-technical`) and says why, as S8a did for a bad preset file.

### 4. Version and the S8a file

`format: N`. The shell reads exactly the versions it knows. A file with a
higher number is invalid with a message that says which version it reads; a
change of the format is a new number and an ADR, never a key that older shells
would skip. A file with no `format` line whose whole content is one preset name (the S8a
`voice-profile`, case and blanks ignored) is read as format 0: `preset: <name>`
and nothing else. `--voice-profile FILE` is an alias of `--soul FILE`; without
either, the shell reads the `soul` file and, if there is none, the old
`voice-profile` in the same folder. Version 1 has no spoken-voice key: the personality's voice belongs
to S11 and will arrive as format 2, which an S8 shell rejects instead of
ignoring.

### 5. Where they act

Only on the reply (ADR 0021): the SOUL and the personality are resolved into
one `Persona` that fills the fixed prompt of `reply::system_prompt`. The
resolver, the broker, the planner and `Confirm` never receive a SOUL, a
personality or a persona, and a test compares every action, verdict and
decision-log event of the evaluation intents under every preset and
personality.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| **Closed `key: value` text (chosen)** | Readable and editable by hand; S9 can write it; contradiction is a comparison; no parser dependency | Own tiny grammar; a new axis needs a format change |
| JSON or TOML | Familiar | A free string is easy to add to either; TOML needs a new dependency; JSON needs care for comments |
| Free-text SOUL in the file | Most expressive | Rejected by ADR 0006: instructions can hide in it, and "contradicts" cannot be decided by code |
| Preset name only (S8a) | Simplest | No machine name, no range, no place for a personality to be checked against |
| Personality with its own free prompt | Most personal | Same objection as the free-text SOUL, per user |

## Consequences

- S9 writes this format; S10 changes it as a plan action reviewed in full.
- The S8a `voice-profile` keeps working as format 0, so no existing file breaks.
- On the host prototype the SOUL is read from the user's `~/.config`, not from
  a root-owned system path as ADR 0006 intends for "one SOUL per machine"
  (known deviation, until there is an installer and a place only root writes).
- Adding an axis (for example emoji) changes the format number.
- The hard limit of ADR 0006 holds by construction: no key in either file is
  read by anything but the reply prompt.

## Decisions taken by the user (2026-11-20)

1. The one-word S8a file is migrated (read as format 0), not rejected.
2. The SOUL file is `~/.config/workspace-os/soul` on the host prototype, a
   recorded deviation from ADR 0006's root-owned, per-machine SOUL.
3. A personality that contradicts the SOUL stops the shell when it was named on
   the command line, and is dropped with a message when it is at the default path.
