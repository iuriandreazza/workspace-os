---
title: "ADR 0014: Path-free resources and the terminal screen of the intent shell"
type: adr
status: accepted
created: 2026-11-16
updated: 2026-11-16
tags: [slm, shell, plan, schema, resolver, broker, ux]
---

# ADR 0014: Path-free resources and the terminal screen of the intent shell

## Context

The S3 prototype and ADR 0011 name every resource by a filesystem path (the
`path` argument) and let the model say "here", meaning the shell's current
directory. After seeing the prototype, the user decided that the person who uses
the intent shell never navigates directories: they say what they want, not
where it is. The shell is agnostic to how the filesystem is organised and to
the difference between user space and system space, and it has no current
directory.

ADR 0002 still holds: the model proposes, deterministic code decides, and the
system works with no model. ADR 0007 still holds: the broker checks the
intersection of the user's permissions and the action's capability, and a root
confirmation shows the exact action.

## Decision

### The model names resources, never paths

A plan version 2 refers to a resource with a **reference**, one of:

| Reference | Meaning |
|---|---|
| `{"place":"downloads"}` | A well-known place, from a closed list |
| `{"name":"report.pdf","in":"downloads","kind":"file"}` | An existing item found by name inside a place (`in` may be `"everywhere"`; `kind` is `file`, `folder` or `any`) |
| `{"new_name":"notes.txt","in":"documents"}` | An item that does not exist yet, to be created in a place |
| `{"from_step":"s1"}` | Everything an earlier step produced, as before, now resources and not paths |

Places: `home`, `downloads`, `documents`, `desktop`, `pictures`, `music`,
`videos`, `everywhere`. `everywhere` means all the person's files wherever
they live; it is not a path and does not tell user space from system space.
Adding a place is an amendment to this ADR, because the list is part of the
schema the model is constrained by.

Which references an argument accepts is part of the schema: a place alone is
a valid target for `fs.list` and `fs.find_largest`, and never for
`fs.write`, `fs.move` or `fs.delete`, so a place can be read but not removed
or overwritten as a whole. A `name` is a single file name: it may not be
empty, `.` or `..`, and may not contain `/` or a NUL. There is no field in
which the model can write a path, and no current directory: "here" has no
meaning, and the model asks a `clarify` question or uses `everywhere`.

### What this supersedes in ADR 0011

ADR 0011 stays accepted for everything not listed here. Superseded by this ADR:

- the `path` and `pathOrRef` definitions of the schema, and the schema version
  (1 becomes 2; the shell refuses a version 1 document);
- the example under "Shape" that uses `~/Downloads`;
- the bullet that says a `from_step` reference is every *path* the step
  produced and that the user sees each real path;
- the **Arguments** column of the initial actions table (`path`, `from`,
  `to`): the action names, capabilities and root rules are unchanged;
- the paragraph that treats a plain string `"s1.path"` as a literal path: a
  plain string is no longer a valid reference;
- the S5 command syntax `--path ~/Downloads`.

### Who resolves a reference: deterministic code, before the broker

```text
model ─► parse + semantic rules ─► resolver ─► broker ─► screen, confirmation ─► execute
                                      │
                         places, name search, from_step
```

The **resolver** is deterministic code behind a port (`Catalog`). It turns
each reference into a concrete resource: a real location plus a friendly name.
The model never sees a real location and never chooses between candidates.
Where places live is the adapter's business: the host adapter maps
`downloads` to `~/Downloads`, the OS will map it to whatever its filesystem
server decides. The core does not know.

- A name search is bounded (the walk cap of S3), logs the search before it
  runs, refuses to run when the log cannot be written, reports an incomplete
  search as a note, and looks only where the user may list.
- Resolution is blind to whether the *action* is allowed. It answers "what is
  this?", never "may you?". The broker is the only authority.
- **Does not resolve.** No match is reported as "not found", with the
  reference in friendly terms. The step is refused, its dependents are
  blocked, and nothing is guessed or fuzzily corrected.
- **Ambiguous.** More than one match is not resolved either. See the decision
  below: the person chooses.
- A *resolved* reference is the only way an action gets a real location.
  In code, a model-facing step spec (with references) and an `Action` (real
  resources only) are different types, and only the resolver and the
  `from_step` expansion construct an `Action`.

### What the broker validates

Always the resolved resource, never the reference. After resolution the
broker runs exactly the checks of ADR 0011: capability, the user's
permissions on the real location, the protected filesystem root, root
holds. A place that maps to the filesystem root, a symlink, a name that
resolves to a file the user cannot touch: all reach the broker as real
locations and get the verdict a direct path would have got. Resolution is
not a way around it. Resolving and judging are two separate steps so that a
mistake in one cannot grant what only the other may.

### What the person sees

- Default: **friendly names**. A resource shows as its quoted name and where
  it is in terms of places: `"report.pdf" (Downloads)`, `"a.txt" (Documents ›
  taxes)`, `"hosts" (elsewhere)`. Two different resources never show alike:
  when they would, more of the parents are added until they differ. All names
  are escaped as in S3, and a name that looks like a location stays inside its
  quotes.
- **Details** (`d` in the plan view): the real location of every resource, next to
  its friendly name.
- **Root confirmation always shows the real location**, with no toggle,
  because ADR 0007 requires the exact action. The panel shows the friendly
  name, the real location and the action together.

### The screen

```text
 intent shell ─────────────────────────────────────────────────────────
  user  iuri (administrator)   model  Bonsai 8B   log  decisions.jsonl
 ───────────────────────────────────────────────────────────────────────
 intent> delete the three largest files in downloads

  Step  What                                      Needs   Verdict
  s1    Find the 3 largest files in Downloads     read    ok
  s2    Delete "big.iso" (Downloads)              delete  ok
        Delete "movie.mkv" (Downloads)            delete  ok
        Delete "disk.img" (Downloads)             delete  ROOT, held

  Notes
   · s1: 2 folders could not be searched; the result may be incomplete.

  3 steps: 3 ok, 1 held for root, 0 refused.  The model says: "…"
  [y] run   [d] details   [n] cancel   >
```

```text
 ╔═ ROOT CONFIRMATION ═══ asked of iuri ═════════════════════════════════
 ║ Action  Delete "disk.img" (Downloads)
 ║ Exact   fs.delete  /Users/iuri/Downloads/disk.img
 ║ Needs   fs.delete, as root. Confirms this one action only.
 ╚═ [y] confirm this exact action   [n] refuse   >
```

A header, a table with one line per concrete action and the broker's verdict
beside it, notes, one summary line, and a separate panel for each root
action. Nothing shows a current directory. The log shows as its file name
only; its full location is in the details view. The screen is produced by a
pure function in the core, so it is unit tested; the terminal only prints it.

### Deterministic shell (S5)

The same references are typed without the model:
`fs find_largest --in downloads --count 3`, `fs delete --name big.iso --in
downloads --kind file`, `fs write --new-name notes.txt --in documents`.
The mapping stays mechanical, and the parsed plan goes through the same
resolver, broker and screen. There is one path to execution.

## Decisions taken by the user (2026-11-16)

The user chose option A in all three, and approved the screen above.

### 1. Vocabulary of references

| Option | For | Against |
|---|---|---|
| **A. Closed places plus name lookup (above)** | The model fills a small enum and a single file name, which the grammar enforces. Easy to test, small prompt, nothing to leak. Works with no model (S5 flags) | The model must guess a file name. Anything not in a place or named is out of reach until a place is added |
| B. Free-text description resolved by a deterministic search ("the budget spreadsheet from last week") | Closest to speech | Ranking is a policy in the trusted path, hard to test, and an ambiguous result is the normal case. Not typeable in S5 |
| C. The shell gives the model a catalog of opaque ids | Exact, and no guessing | The catalog goes into the prompt (about 100 tok/s makes that slow), exposes names to the model, and goes stale between turns |

### 2. Ambiguous reference

| Option | For | Against |
|---|---|---|
| **A. The person chooses from a numbered list**, with the place and enough detail to tell them apart; the pick is logged, goes through the broker, and never goes back to the model | Least friction; the choice is the person's | One more prompt type in the terminal |
| B. Refuse and ask to rephrase | Simplest | Annoying, and rephrasing hits the same ambiguity |
| C. Send the candidates back to the model as a `clarify` | Natural wording | The model sees names, and decides what the person should decide |

### 3. How a name matches

| Option | For | Against |
|---|---|---|
| **A. Whole name, case-insensitive; if nothing matches, names that contain it. At most 20 candidates, more means "be more specific"** | Forgiving, and ambiguity is handled by decision 2 | More candidates to pick from |
| B. Whole name only | Never surprising | Fails whenever the model misremembers an extension |

## Consequences

- The schema changes to version 2 and the examples with it; the Rust parser
  and `validate.py` follow, and every example is a test.
- The schema bounds the length of a name; that it has no `/` and is not `.` or
  `..` is a rule in code, because giving the name a `pattern` made
  `llama-server` stop honouring the schema (`docs/s3b-path-free-shell.md`).
- The prompt loses "here", `~` and the working directory, and gains the
  places and the reference forms. The model's quality is S4's question; the
  shell is safe whatever it writes.
- A hidden file reachable only by name is found if its name matches. The
  resolver looks only where the user may list, so it does not reveal what
  the user could not see, but an unreadable file inside a listable directory
  resolves and the broker refuses it, saying so.
- The S3 report's ten intents are the comparison set for this item, because
  the S4 set does not exist yet. S4 reuses them.
- The log gains `resolution` and `choice` events.

## Open questions

- Places beyond the eight, and how the OS maps them when it has a filesystem
  server (K12).
- Whether `everywhere` needs an index instead of a bounded walk once the
  filesystem is large.
