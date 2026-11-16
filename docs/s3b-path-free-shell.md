---
title: "S3b: path-free resources and the terminal screen"
type: report
status: done
created: 2026-11-16
updated: 2026-11-16
tags: [slm, shell, resolver, broker, screen]
---

# S3b: path-free resources and the terminal screen

Roadmap item S3b, decided in ADR 0014. The person never navigates directories
and the shell has no current directory: a plan names things with references,
deterministic code resolves them before the broker, and the broker judges the
real resource.

## Run it

Same as S3 (`docs/s3-intent-shell-prototype.md`, "Run it"): start `llama-server`,
then `./scripts/intent-shell.sh` (add `--administrator` to be allowed to confirm
root actions). `[d]` shows the details view of a plan. In a script, the answers
are echoed so a transcript shows them.

On this host the places live under `$HOME`: `downloads` is `~/Downloads`, and
so on, and a place that does not exist is not found. `everywhere` is `$HOME`.

## What changed

```text
request ─► model (output constrained by the schema, version 2)
        ─► parse + semantic rules        a Spec: an action whose resources are references
        ─► resolver                      reference ─► real locations (or: not found, ambiguous)
        │     └─ ambiguous: the person chooses, the pick is logged, the broker judges it
        ─► broker: capability ∩ user permissions, root, protected root     as in S3
        ─► screen ─► confirm ─► root panel per root action ─► execute ─► log
```

- `reference.rs`: the reference types, and `Spec`, the action as the model
  proposed it. `Action` (real locations only) is built from a `Spec` only by
  the resolver and by the `from_step` expansion, so an unresolved reference
  cannot reach `judge` or the host.
- `resolve.rs`: places, name search (whole name ignoring case, otherwise names
  that contain it, at most 20 candidates), new items, the person's choices. It
  never asks whether the action is allowed.
- `label.rs`: friendly names, `"report.pdf" (Downloads › "taxes")`. Two
  locations never share a label; a name that looks like a place stays quoted.
- `screen.rs`: header, table, notes, summary, details view, root panel and the
  choice question, all pure functions with golden tests.
- `host.rs`: `UnixHost` maps places to folders, searches only where the user
  may list, does not follow symlinks, and says when it could not look
  everywhere. Its errors name the item, not where it is.
- The plan format is version 2 (`docs/schemas/`): `path` and `from_step`-by-path
  are gone, references replace them, and 12 examples were added.

## Findings along the way

- **The shell sent the schema with its keys sorted.** `model.rs` parsed the
  schema into a `serde_json::Value`, whose maps are sorted, and the grammar
  `llama-server` builds follows the order of the properties. With the new
  `name`, `in`, `kind` the model was forced to write `in`, `kind`, `name` and
  produced junk (`"name":"}},"`). The schema now goes out as the file's own
  text, and a test pins the order. S3 had the same defect, with less to lose.
- **`pattern` on a name breaks the grammar.** `^[^/]{1,255}$` made the server
  stop honouring the schema for a request, and reply with a 500. The schema
  bounds the length and the rule that a name has no separator and is not `.` or
  `..` is code (`check_rules`, mirrored in `validate.py`).
- **A place at the filesystem root cannot reach the root as a target.** A
  destination inside a place is the place joined with a name, and a search
  returns children. The root is reachable only if a host lists `/` itself, so
  the protection is tested there too.

## Tests and the gate

`./scripts/check.sh` runs 179 tests for `intent-shell`, none needing the model,
including ambiguity (the person picks, cancels, a pick outside the candidates),
a name that matches nothing, more than 20 matches, a visible file the user
may not read or delete, a reference that tries to reach what a direct path
would be refused, names with separators or `..`, a host that lies about
matches, the search logged before it runs, and the screen.

Mutation check, by hand: each change was applied, the whole suite run, the
source restored and its timestamp refreshed (a stale timestamp makes the next build reuse the mutant). Every mutant was killed. Two of them were also run through `./scripts/check.sh`: it stopped at "unit tests" with the failing tests.

| Mutation | Failing tests |
|---|---|
| first candidate chosen instead of reporting the ambiguity | 8 |
| names may contain separators | 4 |
| not found becomes an empty success | 3 |
| the search is not logged | 1 |
| the host's matching is trusted | 1 |
| a pick outside the candidates is accepted | 1 |
| an exact match does not win over "contains" | 1 |
| more than 20 matches become a choice | 1 |
| `.` and `..` accepted as names | 3 |
| resolved resources are not judged | 24 |
| protected root dropped | 2 |
| user permission check dropped | 11 |
| capability check dropped | 2 |
| the root panel hides the exact action | 3 |
| the plan screen shows real locations | 2 |
| labels that collide are not separated | 1 |
| a `folder` reference matches files | 1 |
| a place is found by naming it | 1 |
| the search note is dropped | 1 |
| matches are not deduplicated | 1 |
| the name limit counts characters, not bytes | 1 |
| a place may be overwritten | 1 |
| a place may be removed or moved | 2 |
| a folder may be moved into itself | 2 |
| a new item may take an existing name | 1 |
| an unfinished search resolves silently | 1 |

## What the model did (for S4, not fixed here)

The eight intents of the S3 report, with "here" and `DIR` turned into "my
downloads" or left as they were, Bonsai 8B, temperature 0, grammar-constrained.
Every output was valid and none held a path.

- "list the largest files here" and "show the 3 largest files": `fs.find_largest`
  in `home` (the place the model picks for "no place"), with `count` 10 and 3.
- "delete the two largest files in my downloads": the search and the delete
  by reference, shown as three actions. "find the 2 largest files ... and
  delete them": only the search.
- "find the 2 largest ... then run stat on each": the search and a `from_step`
  `fs.stat`, expanded to the two real files. S3 had invented literal paths.
- "create a file notes.txt ...": `fs.write` on a `name`, not a `new_name`, so
  the resolver reports that nothing was found. Adding a sentence to the prompt
  did not change it.
- "add the user bia to the group staff": no valid plan, nothing ran.
- "delete the largest file ..., as administrator": the search with `elevate`,
  held for root; the delete was not planned.

## Limits and follow-ups

- `everywhere` is `$HOME` on this host, so system files are out of reach until
  the OS maps it differently. This is the adapter's choice (ADR 0014).
- The resolver walks the tree for a name breadth first: at most 100,000
  entries, with a note when it stops, so on a large home a deep match can still
  be missed and is reported as not found. There is no index.
- Table columns are aligned by characters, not by terminal cell width, so wide
  characters misalign a row. Escaping and uniqueness do not depend on it.
- The log records real locations. It is private to the user, as in S3.
- A symlink matches by its own name and is not resolved to its target, so a
  root confirmation shows the link, not where it leads. `access(2)` follows it.
- A search that stopped early (the entry cap or an unreadable folder) never
  resolves silently, even to a single match: the person confirms it.
- Names that differ only by Unicode normalisation (NFC and NFD) or look-alike
  letters can look the same in a choice list, and the list has no details view: the
  real location is visible only after the pick, in the details view and the
  root panel.
- A name stored in a different Unicode normalisation than the model wrote it
  (NFD against NFC, common for downloaded files on macOS) is not found: telling
  them apart needs a normalisation table, which is a dependency this item did not add.
- The decision log is not protected from a delete that names it or a folder above
  it; this matters once deletes run. The screen does not say whether a deleted
  item is a folder.
- A symlink to a folder is found as a file, not as a folder. `exists` and `same`
  look at metadata during resolution and are not logged; only searches and reads are.
- `allows` ignores the sticky bit on the folder that holds an item, and the
  schema lets `fs.write` name a folder. Both only matter once writes run.
- Two steps of one plan are compared by location only: deleting a folder and
  acting on something inside it in the same plan is not flagged.
- The schema is compared with the examples by `validate.py` by hand, not in the
  gate (ADR 0011); the Rust parser is stricter than the schema for `2.0`. A name is bounded in
  characters by the schema and in bytes by a rule (`NAME_MAX`).
- A search runs again after each choice the person makes, so a plan with
  several ambiguities searches several times.
- S4 reuses the eight intents above as its first cases. S5 types the
  references as flags (`--in downloads --name x`), as ADR 0014 says.
