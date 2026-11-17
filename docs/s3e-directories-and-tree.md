---
title: "S3e: directories, navigation and the tree view"
type: report
status: done
created: 2026-11-16
updated: 2026-11-16
tags: [slm, shell, resolver, broker, tree, schema]
---

# S3e: directories, navigation and the tree view

Roadmap item S3e, the implementation of ADR 0018. S3b (ADR 0014) had removed
paths and the current directory; after using the shell the person asked for
them back. A plan may now name a path next to the other references, the session
has a current directory, and listings, rankings and the choice of an ambiguous
reference are shown as a tree. The broker is still the only gate: naming
`/etc` gives access to nothing.

## Run it

As S3 and S3b (`docs/s3-intent-shell-prototype.md`, "Run it"): start
`llama-server`, then `./scripts/intent-shell.sh` (`--plain` for the line
adapter, `--administrator` to be allowed to confirm root actions).

```text
intent ~> list the folder ~/Downloads/films
intent ~> go to the Documents folder
intent ~/Documents> list the files in the current directory
```

The session starts in `$HOME`. The plain prompt shows the current directory;
every plan screen starts with `in <directory>`; the header shows where the
session started (it is printed once). The details view (`[d]`) and the root
panel show the exact absolute location.

## What changed, by ADR section

```text
request + current directory ─► model (output constrained by the schema, version 3)
        ─► parse + semantic rules        a Spec: references, now also paths and under_home
        ─► resolver (blind to permission)  path ─► real location; name ─► search; cd target
        ─► broker: capability ∩ user permissions, root, protected, not a directory, not found
        ─► screen (relative to the directory or ~) ─► confirm ─► execute ─► log (with the directory)
```

1. **Naming** (`reference.rs`, `plan.rs`, `resolve.rs`): `Direct::Path` is a
   path as typed (absolute, `~`, or relative to the current directory);
   `Direct::UnderHome` replaces `everywhere` (a search below the home, described
   as such: `file "x" under home`); `Named` and `New` take a `Within`, which is
   a place or a path. The resolver cleans `.` and `..` lexically (a `..` above
   the root is refused as not naming a location), expands `~`, and asks the
   host to resolve links. It does not look at permission or at whether the path
   exists; `session.cd` and every action are judged by the broker.
2. **Navigation** (`session.rs`): `SessionState { cwd }` is state of the
   session, one per session, created at the home. `Shell` takes it as `&mut`.
   `Review` carries the directory the plan was made under, the log records it
   (`request`, `review`, and a `cd` event with the old and new directory), and
   the model is told it in the request, next to the request and after the
   static prompt so the prompt cache still works.
3. **Tree** (`screen.rs::tree`, `host.rs::list`): a pure function from entries to
   lines, with depth 2, 200 lines, hidden entries only when asked, folders
   first then case-insensitive names, sizes right-aligned, and the marks
   `system`, `link ->`, `unreadable`, `protected`, `hidden, asks to open`,
   `⚠ asks` and `ROOT`. The same lines serve `--plain` (candidates numbered)
   and the interface (candidates selected with the arrows, context unmarked).
4. **Schema version 3** (`docs/schemas/`): `{"path": …}`, `{"under_home": true,
   "name", "kind"}`, `in` as a place or a path, `session.cd` (no `elevate`);
   `everywhere` is gone. 57 examples, each a test in the gate and in
   `validate.py`.
5. **Migration**: a version 2 document is refused with `unsupported version 2`
   and the model is shown that on its retry. The decision log is not versioned;
   old logs stay as they are and new events only add fields.

## Decisions of detail

These are choices the ADR left open, taken so that no decision of the ADR
changes.

- **`..` is cleaned lexically** (ADR 0018 decision 1), before links are
  resolved: `~/link/..` is `~`, not the parent of where `link` leads. What the
  broker judges, what the screen shows and what runs are all the same
  location, so this is a difference from the shell, not a hole.
- **Links.** The parent of a path is always resolved through links. The last
  component is followed when the step looks at, enters or writes to the thing
  (`list`, `stat`, `read`, `find_largest`, `cd`, `write`) and kept when the
  step removes or moves it (`delete`, `move` source), so the hidden-path rule
  of ADR 0016 and the root panel judge where the data is, and deleting a link
  still deletes the link. This holds for a typed path and for a name found by
  a search. A link that leads nowhere yet is followed by hand to where a write
  would create it. Paths that come from a listing (`from_step`) are not
  followed by the broker; `UnixHost` refuses a plain name that leads to a
  hidden place when it lists, ranks or reads it.
- **A path that names nothing** is "not found" when the first missing step is
  one the user may reach, and a permission refusal otherwise, so the answer
  does not tell whether something exists in a folder the user cannot enter.
- **Hidden entries** (ADR 0018: "only when asked"): `fs.list` has no flag, so
  hidden entries are counted in the tree header (`2 hidden`) and listed, marked
  `hidden, asks to open`, only when the listed folder is itself hidden. A
  hidden folder is never opened by a listing. A ranking (`find_largest`) of a
  plain folder does not look into hidden entries; before this item it did,
  without asking.
- **Folder sizes are not shown.** The ADR's example shows them; a recursive
  size would make every listing walk the whole subtree. Files show their size.
- **`session.cd` is a plan of its own.** Its directory takes effect for the
  next request (ADR: a plan carries the directory it was made under), so a plan
  that mixes it with other steps is refused with a message the model sees on
  its retry. It needs no capability and no `elevate`, never asks, is logged
  with the old and new directory, and is checked again when it runs.
- **The directory the model is told** is written both ways,
  `~/Downloads (/Users/ana/Downloads)`, in the request after the static
  prompt. With only `~` the 8B model invented homes (`/home/user/...`).
- **The header** shows the directory the session is in: the interface rebuilds
  it after every request; `--plain` shows it in the prompt
  (`intent ~/Documents>`), because its header is printed once.
- **Choice numbering** follows the rows of the tree, so the numbers go down
  the screen and the arrows move down the tree; the history of a choice says
  the location chosen, not only the leaf name.
- **Places are the real folders** they lead to (`place_roots` is canonical, as
  `home()` is), so a place that is a link is judged and shown where it leads.

## Findings along the way

- **`pattern` with a NUL breaks the grammar.** ADR 0018 section 4 says the
  grammar bounds a path and rejects NUL. `"pattern": "^[^\\u0000]+$"` made
  `llama-server` answer 500 ("output does not match the expected peg-native
  format") for some requests, the same way `pattern` on a name did in S3b. The
  schema bounds the length in characters; the NUL and the 4096 bytes are rules
  in code (`check_rules`, mirrored in `validate.py`, one example each). The
  decision of the ADR holds: a path with a NUL is refused. Only the layer
  that refuses it is the same as for names.
- **Bonsai 8B is weak with paths** (for S4, not fixed here). It writes
  `{"path":".},"}` for "this folder", invents `/home/user/...` when it is not
  told the real home, copies the example path of the prompt into an unrelated
  request, and picks `place` for `~/Downloads/films`. The prompt now says to
  write the current directory out in full instead of `.`, shows the real
  directory next to `~`, and uses a neutral example. With that, the
  transcripts below are right; the model still deletes "Downloads" when asked
  to delete a file in it, which the broker refuses (`a whole place and cannot
  be removed`).
- **The previous live test was stale.** It expected a declined or refused plan
  for "list the five largest files", but ADR 0016 runs read-only plans without
  asking. It now asks for a delete, and runs in a throwaway home.
- **Nine adversarial review rounds** found, among others, a hidden-path
  bypass through a link as the last component (list, ranking, write, move
  into, a name found by search, a dangling link), choice rows cut by the line
  limit, candidates numbered out of row order, a plan that mixes `cd` with
  other steps, a `cd` into something replaced while the person decided, a read
  of a hidden listing before the question, two tests sharing a directory, and
  a tree gather with no bound on a huge child. All were fixed with a test.
  Round 8 found nothing; round 9, on the last change only, found a path
  through a file reported as a permission problem, fixed in the next commit and
  re-read by a final round.

## Tests and the gate

`./scripts/check.sh` runs 315 tests for `intent-shell`, none needing the
model (it was 246). `intent-shell/src/directories_tests.rs` holds the ones of
this item; `host.rs` has the ones that need a real filesystem (links, the
tree, the cap).

| Area | What is tested |
|---|---|
| Every form of reference | A path absolute, `~`, relative, with `.`, `..` and a trailing `/`, a place, a name in a place and in a path, `under_home`, a new item in a path, a move into a path and to a new name, `cd` |
| Escapes | `..` above the root names nothing and never reaches the host; `~/../../etc/hosts` resolves, and the broker refuses it, and the refusal is on the screen; a link out of the home is judged at its target; what the user cannot reach is refused by permission, not "missing"; `/etc` may be named and listed |
| The directory | `cd` changes the next resolution and the plan screen; it is logged; it does not leak between sessions; it needs no capability and never asks; a file, nothing, or what cannot be entered is refused and the directory stays; it is a plan of its own; it is checked again when it runs |
| The tree | Folders first, then names ignoring case; depth 2 and the note about deeper folders; 200 lines and `… N more`; hidden counted and listed only when asked; `system`, `link ->`, `unreadable`, `protected`, `ROOT`, `⚠ asks`; the listing never feeds `from_step` |
| The choice | Candidates in their parent context, only they selectable, every one of up to 20 even when deep, numbered in row order, folders drawn as folders, `--plain` and the interface |
| Migration | A version 2 plan is refused with `unsupported version 2` and the model is told; `everywhere` is gone |
| Root | A root action on a relative path shows the exact absolute location, in the details view and the root panel, and not in the friendly view |
| Links and hidden paths | A typed path, a name and a move destination through links; a dangling link; a hidden link listed, ranked or read; a ranking skips hidden entries |

Mutation check, by hand: each change was applied, the whole suite run, the
source restored with `git checkout` and its timestamp refreshed. Every mutant
was killed. One was also run through `./scripts/check.sh`: it stopped at "unit
tests" (138 failures for version 3 read as version 2).

| Mutation | Failing tests |
|---|---|
| `~` expands to the working directory | 2 |
| a relative path ignores the session directory | 4 |
| `..` dropped instead of cleaned | 3 |
| `..` above the root clamped, not refused | 1 |
| a path to nothing is not "not found" | 2 |
| the last link of what is looked at is not followed | 1 |
| `cd` does not change the directory | 3 |
| the review resolves against the home | 4 |
| `cd` judged without checking it is a directory | 2 |
| `cd` needs a capability | 1 |
| version 2 accepted | 138 |
| the root panel shows only the friendly action | 4 |
| the plan screen omits the directory | 2 |
| tree: folders not first | 1 |
| tree: depth one too deep | 1 |
| tree: the line limit ignored | 2 |
| tree: hidden entries always shown | 1 |
| tree: no `system` mark | 1 |
| tree: `ROOT` mark dropped | 1 |
| tree: the deeper-folders note silent | 1 |
| choice: candidates cut at the line limit | 1 |
| a listing's tree feeds `from_step` | 1 |
| a hidden link is listed | 1 |
| a ranking looks into hidden entries | 1 |
| a move into a link judged at the link | 2 |
| candidates not renumbered in row order | 1 |
| `cd` not checked again when it runs | 1 |
| a read of what must ask runs before the question | 1 |
| `cd` mixed with other steps accepted | 2 |
| a `cd` leaks into the next session (a global) | 3 |
| a path below a file is not "not found" | 1 |

## The live test

With `llama-server` running (`docs/s3-intent-shell-prototype.md`):

```bash
./scripts/intent-shell-live-test.sh
```

It runs three tests in a throwaway home under `/tmp`, one at a time so the
transcripts do not interleave: the plumbing and a declined delete; a path and
a relative path resolving against the session directory; and a `cd` after
which the next request starts in the new directory. The transcript of the last
run is in the pull request. A live run depends on the model: the first
attempts failed for the reasons under "Findings".

## Limits and follow-ups

- Folder sizes are not shown (above); table columns and the tree are aligned
  by characters, not terminal cells, so wide characters misalign.
- `find_largest` still walks to 100,000 entries, as in S3.
- A listing of a folder with more than 5,000 entries shows the first the
  system returned and says so; `from_step` over more than 100 paths is refused
  as before.
- A destination that does not exist yet (`fs.write` create, `fs.move` to a new
  name) below a missing folder is refused as a missing write permission, not as
  a missing folder; writes do not run in this prototype.
- `UnixHost::same` compares by case when only one of two paths exists (S3b);
  on a case-sensitive volume this can refuse two steps that name `Foo` and
  `foo`. Pre-existing.
- The decision log records real locations, including the new `cwd` fields; it
  is private to the user, as before. Old logs stay readable: only fields were
  added.
- S4 measures the model on paths (the weaknesses above), with schema version 3.
  S5 types `cd`, `ls` and `tree --depth N`; the tree text already says
  "name a path to open it" until the flag exists.
- ADR 0018 says "use a path or `--depth`" in the tree example; `--depth` does
  not exist until S5, so the text says only the path.
