---
title: "ADR 0036: A SOUL change is a root-approved diff, kept in a history that can be undone"
type: adr
status: accepted
created: 2026-11-26
updated: 2026-11-26
tags: [soul, plan, broker, root, history, undo, shell]
---

# ADR 0036: A SOUL change is a root-approved diff, kept in a history that can be undone

## Status

**Accepted by the project owner on 2026-11-26** (the six departures were put to them and answered "Aceitar"). It fills what ADR 0006 point 4 and ADR 0011 ("Proposing a change
to the SOUL") leave open for roadmap item S10: where the history lives, its
format, how undo works, what the approver sees, and two small additions to the
plan format. The decisions the project owner must accept are
the ones in the tables below.

**It partially supersedes ADR 0011** ("Proposing a change to the SOUL": the
pending proposal that any user's AI records) and **refines ADR 0028 section 5**
(the root panel carries SOUL text to `Confirm`; the broker still never reads
a SOUL). Both are spelled out below.

## Context

ADR 0006: the AI may propose changes to its own SOUL; only a user with root
authority approves; the full diff is shown; every applied change is kept in a
history and can be undone. ADR 0011 has the action `soul.propose_change`
(capability `soul.propose`) and says the model never writes the diff, the
system computes it. Today the action parses and is judged, but nothing runs it
(`host.execute` says "not executable"). ADR 0007 defines root authority as
membership of the administrators group, confirmed per action, and ADR 0032
accepted that on the host prototype it is the `--administrator` flag.

## Decision

### 1. Actions

| Action | Args | Capability | Root | Asked |
|---|---|---|---|---|
| `soul.propose_change` | `rationale`, `new_text` (as today) | `soul.propose` | **always** (was: never) | the plan, then the root panel |
| `soul.undo_change` (new) | none | `soul.propose` | always | the plan, then the root panel |
| `soul.history` (new) | none | `soul.propose` | never | no: it only reads |

The plan schema stays at **version 3**. The document shape of
`soul.propose_change` does not change, only the root rule, which lives in the
broker's action table and not in the document; the two new actions are added
to the action list, and ADR 0011 says a new action is an amendment (a bump is
for changing the document of an existing one). The deterministic
shell (ADR 0022) gets `soul propose_change`, `soul undo_change` and
`soul history`, so every path is reachable with no model (ADR 0002).

Undo is a plan action, not a hidden command, because it changes the SOUL and
must go through the same broker, panel and log as any change. The model may
emit it; it can do no more than propose, as for a change.

### 2. Who approves

"Root authority" is `User.administrator` of the session, the rule S9 already
uses. The broker decides it from the action table (`RootRule::Always`), as for
`user.manage`:

- an administrator: the step is held (`ROOT, held`) and the plan screen is
  asked first, then the root panel of section 3;
- anyone else: the step is **refused at review** with "root authority needs an
  administrator, and you are not one". Nothing is asked and nothing changes.
  The full text the model wrote is in the decision log (ADR 0002).

**The broker never reads the SOUL.** Its verdict for these actions depends only
on the capability and on `administrator`, so the same request gives the same
verdicts, `review` events and `describe()` under any SOUL (ADR 0006, ADR 0028
section 5: the tests that compare them stay and cover these actions).
`describe()` carries the text the model proposed and never the current file, so it does not vary with the SOUL. Every
result that depends on the file (stale, nothing changes, unreadable SOUL,
damaged history, lock held, the effect on a personality) happens when the step
runs and is a failed step with a `soul_change` event, never a broker verdict.
The one refinement of ADR 0028 section 5: the root panel, built at that moment
by the session core, is what carries SOUL text to `Confirm`. The planner, the
resolver and the broker still never receive it.

The write itself needs an `Approval`, a value the core can only build from a
confirmation given by an administrator. The store refuses a write without one,
so a bug in the broker still cannot write the SOUL (two layers, each tested).

The limit ADR 0032 accepted stays: `--administrator` is a flag, not
authentication. S11b and S12 are what make it real.

### 3. What the approver sees

One root panel per step, in the existing root-confirmation form, whose body is
the **whole diff**: every line of the current file and of the new file, each
with a fixed prefix (`  ` kept, `- ` removed, `+ ` added), then counts the code
computed. Rules:

- The text that is written is the proposed text rewritten by the SOUL parser
  (`Soul::parse` then `Soul::to_text`, ADR 0028), never the model's bytes. The
  file rules of ADR 0028 apply to the proposal as to a file: at most 2048
  bytes, UTF-8, a leading byte-order mark ignored (the plan allows 16384 bytes
  of `new_text`, so the limit is checked here). A proposal that fails any of
  it is refused with the message and never reaches the panel. A proposal whose
  canonical text equals the current file byte for byte is refused as "no
  change". So comments, odd spacing and unknown keys the model wrote
  do not exist in what is approved or written.
- The current side is the file's real bytes, so what is lost from a hand-edited
  file (comments) shows up as removed lines.
- Every line passes the same escaping as other model text (`printable`), and
  the frame characters (`║ ╔ ╚ ═ ↳`) in it are escaped too. Untrusted text is
  cut into pieces of at most 64 characters, never inside an escape, each
  behind a prefix the code draws (`↳` marks a continuation). A character that
  may take more than one terminal cell (anything outside ASCII and U+00A0 to U+024F, the Latin letters) is escaped, so a piece is as wide as it has characters (the Latin letters are assumed narrow, as terminals draw them unless set to draw East Asian ambiguous-width characters wide; in such a terminal a line can still wrap inside a piece, a limit this ADR accepts), so a terminal never wraps it and a wrapped
  piece never starts a line that looks like the panel's. A line starting with
  `+` or `-` inside the file is shown after its own prefix. The panel ends
  with the line the code draws. When the two files differ only in line endings
  or the final newline, the panel says so, because the line diff is empty.
- The model's `rationale` is shown once, quoted and labelled as the model's
  words, never as part of the diff.
- An undo shows the same panel, from the current file to the file it restores.
- A panel that does not fit the terminal cannot be confirmed (the existing
  rule of the interface, ADR 0015, which limits height as well as width). A
  SOUL is at most 2048 bytes, but blank and comment lines are allowed in a
  hand-edited one, so its diff can be about two thousand lines: on a short
  terminal approval is impossible and `--plain` (or a taller terminal) is the
  way. In `--plain` only the end of a long panel stays above the question, so
  the panel repeats the counts of the diff just after the last diff line.
  Refusing stays possible.
- What the change does to the approver's own personality (section 5).

### 4. Applying

All of it under an exclusive `flock(LOCK_EX | LOCK_NB)` on a dedicated lock
file, `.lock` inside the history directory (never on a directory descriptor,
which not every system honours; the lock goes with the process, so a crash
leaves nothing to clean). So two sessions
at once never interleave and a held lock is a refusal ("another session is
changing the SOUL"), never a wait.

1. Read the SOUL and settle what a dead process left (section 6; settling needs the text). The SOUL must exist (first use is ADR
   0032's interview) and be readable; the panel was built from exactly this
   text. If the text on disk now is not the text the panel showed, refuse:
   "the SOUL changed since you saw it; nothing was changed".
2. Write the new SOUL to a temporary file beside the SOUL (`fsync`), then
   the history entry as `<seq>.pending`, naming that file by its inode, `fsync`
   it and the history directory (so the entry cannot vanish while the swap
   below survives), then `rename` the temporary file over the SOUL and `fsync`
   the directory. A reader sees the old file or the new file, never half of
   one.
3. Commit the history entry (section 6).

The SOUL path is resolved once, links followed, and that real path is used
for the write, the history directory name and the lock; a link that leads
nowhere is refused. The path the shell reads is the one changed, so the S8a
`voice-profile` (read as format 0) is rewritten in place as format 1. Mode
0644 like S9. The running session keeps the persona it started with; the new SOUL
is read at the next start.

### 5. Personalities that the new SOUL contradicts

ADR 0028 defines "contradict" and says what a shell does with a personality
that does: at the default path it is dropped, with a message, and the SOUL
stands alone; named with `--personality` it stops the shell. This ADR adds
nothing to that and decides:

- **The SOUL wins.** ADR 0006: a personality never contradicts the SOUL.
- **No personality file is ever written by a SOUL change.** A personality
  belongs to its user, and the shell cannot reach other users' files. A user
  whose personality stops being valid is told at their next start, by the
  existing message, and edits it.
- **Undo restores validity** with no work: the personality file was never touched.
- **The panel says what it can know:** whether the approver's own personality
  file (the one this shell would load) stays valid against the new SOUL, or
  the exact reason it would be dropped. It cannot say it for other users.
- A shell started with `--personality FILE` that the new SOUL contradicts
  stops with the ADR 0028 error until the flag is removed or the file changed.
  Accepted, because that is the rule the person chose when they named it.

### 6. History

| Question | Decision |
|---|---|
| Where | A directory beside the SOUL file: `<SOUL file name>.history/` (`soul.history/` for the default). It is derived from the path the SOUL was read from, so `--soul FILE` has its own |
| What is an entry | One file per applied change, `00000001.json`, never rewritten. Fields: `seq`, `kind` (`change` or `undo`), `at` (Unix seconds), `by` (the confirming user's name), `rationale` (a change) or `reverts` (the `seq` an undo reverts), `file_id` (kept for the crash rule), `before` and `after` (the whole files; a SOUL is at most 2048 bytes, so a snapshot is cheaper and simpler than a patch) |
| Crash safety | The new SOUL is first written to a temporary file beside the SOUL, and the entry (which names that file by its inode, `file_id`: a device number may change between boots, an inode of a file that stays does not) is written complete and synced as `<seq>.pending`; then the temporary file is renamed over the SOUL, then the entry is renamed to `<seq>.json`. A history file therefore only ever describes a change that happened. A leftover `.pending` is settled by the next operation under the lock: if the SOUL equals its `after` **and is the very file the entry names** it is committed (the process died after the swap; a SOUL someone else wrote with the same text is not it); otherwise it is deleted (it never took effect, and the decision log has the proposal) |
| Undo | Reverts the most recent change **not already reverted**: walk the entries in order, a change pushes, an undo pops the `seq` it names. Two undos go back two changes. An undo entry itself cannot be undone, so there is no redo; to get a version back, propose it again. Undo writes the entry's `before` text verbatim as a new entry of kind `undo`, after checking that it still parses as a SOUL (a tampered history cannot write arbitrary text) |
| Refusals | No entries: "there is no change to undo". The SOUL on disk is not the `after` of the entry to revert (it was changed outside the history): refused, naming the change that cannot be undone. Entries unreadable, a `seq` missing or repeated, an undo that names the wrong entry: "the history is damaged", names the entry, changes nothing. A missing directory is an empty history, not damage |
| Listing | `soul.history` skips `.lock` and prints every readable entry up to the first problem (seq, kind, who, when, rationale as a quoted line), then a WARNING line that says what is wrong. Each entry is two lines: a short fixed row (number, kind, time, the name cut at 32 characters) and, indented, the reason cut at 40 characters or `reverts change N`. Messages never put untrusted text on a line longer than a terminal: what a message quotes (a parser's complaint about a proposal, the rationale in a step line) is cut at 60 or 36 characters and on a line of its own, and the messages about a damaged history quote nothing from the file, only entry numbers and a short word of ours ("it is not valid JSON"). Listing and undo first settle a leftover `.pending` when no writer holds the lock, so a change whose process died after the swap can be seen and undone at once |

### 7. The decision log

ADR 0002 logs every model decision. Beside the existing events (`review`,
`plan_confirmation`, `root_confirmation`), a SOUL change adds one event after
the attempt: `soul_change` with `kind`, `by`, `seq`, `outcome` (`applied`, or
the refusal such as `stale`, `locked`, `invalid`, `no_change`, `unavailable`, `nothing_to_undo`, `declined`, `not_administrator`, `failed`, `damaged`). The event with outcome `writing` and the file about to be written is recorded **before** the write, and the shell writes nothing when the log cannot take it (ADR 0002); a log that fails after the write is reported by the result event, with the change done and,
when applied, the file written. The SOUL holds no secret (it is read by every
user), so the file is logged whole; nothing the person typed as a password
exists in this flow.

### 8. The model's prompt

The planner prompt gains one line that says what `new_text` is: the whole file,
`key: value` lines of ADR 0028 format 1, with the keys and the preset names.
The model is **not** told the current SOUL: the planner never receives a SOUL
(ADR 0028 section 5, tested), so it writes a complete SOUL blind and the person
sees what it costs in the diff. A model that proposes a SOUL without the name
the machine has will show that as a removed line; the person refuses it. Giving
the model the current text for this one action is a later decision.

## The user name

The name of the person comes from the environment (`USER` on the host) and is
not ours to trust. The panel header, the history and the entry file hold it
escaped like the other untrusted text and cut at 32 characters with an
ellipsis (the stored name is the cut one, so showing it again changes nothing); the decision log keeps it whole, as JSON. A write whose history
entry would be larger than the history accepts (64 KiB) is refused before
anything is written, so the writer never makes a file the reader refuses.

Whatever a history file holds is untrusted too: the listing cuts the name and
the rationale it reads, and the text of a message about a damaged file, to a
short escaped piece. History files are opened without blocking and only if they
are regular files, and so is the lock file (opened without following links), so a named pipe planted in the directory cannot hold the lock or freeze a listing.

## What changes in ADR 0011

"Emitting the action only records a pending proposal. Any user's AI may do it"
becomes: any user's AI may emit it, and the broker refuses it at review for a
user without root authority. There is no stored queue of pending proposals: a
proposal lives as long as the plan that carries it. A queue of proposals an
administrator reads later is a larger feature with its own staleness rules and
is left out until someone needs it (YAGNI). ADR 0011's status line gains
"partially superseded by ADR 0036" once this is accepted.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| Undo as a shell command outside the plan | One fewer action | A path to change the SOUL that skips the broker, the panel and the log |
| Redo (undo of an undo) | Familiar from editors | Two undos would not go back two changes, which is what people expect from a history of a persona |
| Patches in the history instead of snapshots | Smaller | A SOUL is at most 2048 bytes; a patch chain breaks at the first damaged entry |
| One history file, appended | One file | A crash can leave a torn last line, and rewriting it is the half-written state this ADR forbids |
| Write the SOUL, then the history | Fewer steps | A crash between them leaves an applied change with no history |
| Persistent pending proposals | Non-administrators' proposals reach an administrator | See "What changes in ADR 0011" |
| Rewrite the contradicting personalities | No user sees a message | Edits files that belong to other users |

## Consequences

- S11-2 changes `soul.rs` and the personality after this; the history stores
  whole files, so a later format number changes nothing here.
- S11b (voice confirmation) and S12 (installer) replace the `--administrator`
  flag with real authentication; the `Approval` value is where they plug in.
- The model prompt changes: the S4 evaluation was not re-run for this line.
- A SOUL change takes effect at the next start of each shell.
- `docs/s10-soul-evolution.md` reports what was built and measured.
