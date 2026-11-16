# workspace-os — instructions for Claude

A Rust microkernel with models as the control plane. Read `README.md` for the
overview and `docs/adr/` for the decisions already taken. Do not reopen a
decision recorded in an ADR; propose a new ADR that supersedes it.

Work is planned in `docs/roadmap.md`: one item is one session, one branch and
one pull request. When a session finishes an item, mark it done there.

## Commands

```bash
git config core.hooksPath .githooks   # once per clone: enables the git hooks
./scripts/check.sh                    # format, licences, lint, unit tests and boot smoke test: the full gate
./scripts/intent-shell.sh             # host-side intent shell (S3, S3b); see docs/s3b-path-free-shell.md
cargo run                             # boot the kernel in QEMU (Ctrl-A then X to exit)
```

`cargo`, `qemu-system-aarch64` and `python3` (standard library only, for the
licence check) must be on `PATH`.

## Checks

`scripts/check.sh` is the single definition of "this change is acceptable". It
runs in three places and must stay the only one:

- locally, on demand;
- in the git hooks under `.githooks/` (`pre-commit` and `pre-merge-commit`);
- in CI (`.github/workflows/ci.yml`) on every pull request and on `main`.

Add a new check to `scripts/check.sh`, never to a hook or the workflow
directly. Never bypass the hooks with `--no-verify`.

The hooks are tracked files, so they exist in every worktree once
`core.hooksPath` is set for the clone. If it is not set, ask the user to set
it; `git -c core.hooksPath=.githooks commit` applies it to a single command.
The hooks refuse to run while the working tree has unstaged changes or
untracked files, because the checks read the working tree.

`main` is protected on GitHub: no direct push, and a pull request needs the CI
check `Format, lint and boot` to pass.

## Showing that it runs

Every pull request that changes the kernel or the bootloader includes, in its
description, the serial output of a real boot of its final commit
(`cargo run`), copied as printed and not summarised. When the change adds
behaviour that only another boot shows (a different memory size, another
exception level, a boot that must end in a panic), include that boot too,
with the exact command.

Passing checks say the gate is green; the output shows the user what the
system actually does.

## Review: always adversarial, always local

Every pull request that changes code or tooling is reviewed locally and
adversarially before it is merged. Do not request remote or automated
reviewers (Copilot or others), and do not wait for them.

Procedure:

1. Finish the change and make `./scripts/check.sh` pass.
2. Start a reviewer with a fresh context (a subagent). Give it the diff against
   the base branch and the ADRs. Do not give it the author's reasoning,
   conclusions or a summary of what the change is supposed to do well.
3. The reviewer's job is to break the change, not to approve it. It looks for:
   - correctness bugs and undefined behaviour, above all in `unsafe` code,
     assembly and linker scripts;
   - hardware and boot assumptions that hold on QEMU but not elsewhere;
   - concurrency, interrupt and re-entrancy hazards;
   - violations of an ADR or of the layering (`arch` / `board` / `drivers`);
   - checks that can pass while the behaviour is broken, and missing tests;
   - licence problems in new dependencies (see ADR 0005).
4. Every finding states a concrete failure scenario: the input or state, and
   the wrong result. A finding without one is discarded.
5. The author verifies each finding against the code, then either fixes it or
   rebuts it in writing with evidence. "Works on my machine" is not evidence.
6. Repeat from step 2 with a new reviewer until a round produces no confirmed
   finding.
7. Record the outcome in the pull request description: rounds run, findings
   fixed, findings rebutted and why.

Pull requests that only touch documentation (`docs/`, `README.md`,
`CLAUDE.md`) are exempt.

## Git

- Semantic commit messages and branch names in English (`feat`, `fix`, `docs`,
  `refactor`, `chore`).
- No Claude attribution anywhere: no `Co-Authored-By` trailer in commits and
  no "Generated with Claude Code" line in pull request descriptions.
- One branch per piece of work; open a pull request against `main`.
- The user merges. Do not merge a pull request, even with the review closed
  and CI green; say that it is ready. Documentation-only pull requests are the
  exception.

## Code rules specific to this project

- The kernel holds mechanism only. Policy and models stay outside it (ADR 0002).
- Architecture-specific code lives under `kernel/src/arch/`, board-specific
  addresses under `kernel/src/board.rs`.
- Every `unsafe` block has a `// SAFETY:` comment stating why it is sound.
- The kernel is built soft-float and must not use FP or SIMD.
- New dependencies must be GPL-2.0 compatible (ADR 0005).
