---
title: "Article 1: What adversarial review found in a small microkernel boot path"
type: article
status: draft
created: 2026-11-15
updated: 2026-11-15
tags: [article, kernel, boot, aarch64, review, testing]
---

# What adversarial review found in a small microkernel boot path

> Draft for the author's review (roadmap item R2). Language: English. Format:
> scientific article, for general developers. Sentences in square brackets are
> notes to the author, not article text. Every factual claim is listed with its
> source in the table at the end.

## Abstract

I am building a microkernel in Rust for aarch64, run under QEMU, as the
deterministic core of an operating system in which language models act as a
control plane. Its first milestone was small: boot, print a banner on the
serial port, stay alive. Before each pull request was merged I had it reviewed
by a reviewer that sees only the diff and the architecture decision records,
and that must give a concrete failure scenario for every finding. Two pull
requests went through this process, with two and four rounds. This article
reports what the reviewers found, the code before and after each fix, and the
mutation that the test suite now catches because of it. The recurring result
is that the boot code was mostly not wrong in the obvious way; the **tests**
and **comments** around it were. A test passed with a kernel that was broken,
and a comment gave a reason for a design choice that was false. I also report
what the process did not catch.

## 1. Introduction

A boot path is a good place to study review, because it is short, it runs
before anything that could report an error exists, and almost every mistake in
it looks like success on the development machine. An emulator is forgiving: it
loads the image for you, zeroes memory for you, and hands you a single core.

The project (`workspace-os`, GPL-2.0-only, public) rests on a few decisions
written as ADRs. The ones that matter here:

- ADR 0001: the kernel is an own microkernel in Rust. It keeps mechanism only;
  drivers and servers run in userspace.
- ADR 0002: models are the control plane over a deterministic core. The model
  proposes and deterministic code decides; the system must boot and work with
  no model loaded.
- ADR 0004 (aarch64 first) and ADR 0005 (GPL-2.0-only).

If the core is meant to be the thing that bounds probabilistic components, then
its own checks must not be probabilistic either. That is the motivation for
treating the test suite as part of the product and for reviewing it as hard as
the code.

I do not claim novelty for the review idea. [The prior-art survey
(`docs/prior-art.md`) covers kernel policy, LLM-as-OS, driver reuse and boot
models; it does not cover code-review practice, and I did not survey the
literature on it. This article therefore makes no claim of comparison.]

## 2. The system under test

The kernel at the end of the period reported here (milestones K0 and K1)
boots on QEMU `virt`, enters EL1 from EL1, EL2 or EL3, parks secondary cores,
handles synchronous exceptions, and prints a banner. Three files carry most of
the boot logic: `kernel/src/arch/aarch64/boot.s`, `exceptions.s` and `link.ld`.

A single script, `scripts/check.sh`, is the definition of "this change is
acceptable". It runs format, lint and a boot smoke test
(`scripts/boot-smoke-test.sh`), and it is the only place checks are defined:
it runs locally, in tracked git hooks (`.githooks/pre-commit` and
`pre-merge-commit`) and in CI on every pull request. `main` is protected: no
direct push, and the CI check must pass.

## 3. Method: the review procedure

The procedure is in `CLAUDE.md`. In summary:

1. Finish the change and make `./scripts/check.sh` pass.
2. Start a reviewer with a fresh context (a subagent). It receives the diff
   against the base branch and the ADRs, and **not** the author's reasoning,
   conclusions or a summary of what the change is meant to do.
3. The reviewer's job is to break the change. It looks for correctness bugs
   (above all in `unsafe`, assembly and linker scripts), assumptions that hold
   on QEMU but not elsewhere, concurrency and interrupt hazards, ADR and
   layering violations, checks that pass while the behaviour is broken,
   missing tests, and licence problems.
4. Every finding must state a concrete failure scenario. A finding without one
   is discarded.
5. I verify each finding against the code and either fix it or rebut it in
   writing with evidence.
6. Repeat with a new reviewer until a round produces no confirmed finding.
7. The outcome is recorded in the pull request description.

Two things about who did what, stated plainly. The reviewers were automated
agents, each started with a fresh context. The findings were then judged by me
against the code. [Author: add one or two sentences on how the code itself was
written and how much of it used AI assistance. The sources in the repository
do not say, and this article should.]

The fix for a finding is accepted only together with a gate: a change to the
test script that fails when the behaviour is broken. I checked that with
mutations: I broke the behaviour on purpose and confirmed `check.sh` failed.
The pull requests record these mutations. I did not find a script in the
repository that automates them; they were run by hand.

## 4. Results

Two pull requests were reviewed this way before this article:

| Pull request | Content | Rounds | Findings reported |
|---|---|---|---|
| #3 | Gate, hooks, CI, review policy, first boot fixes | 2 (a third was in progress when its description was written) | 12 + 10 = 22, none critical |
| #7 | K1: exceptions and entry to EL1 | 4 (last one with no confirmed finding) | 7 + 4 + 6 + 0 = 17 as stated by the round counts |

PR #3 describes its 22 findings as: 20 fixed, 1 not changed (finding 7 of
round 2, below) and 1 fixed in part (finding 9 of round 2). PR #7 lists
rebuttals in each of rounds 1 to 3.

The first boot (pull request #2) was written and merged before any of this.
The review of #3 therefore also covered the K0 kernel, and #7 covered code that
had reached `main` without a clean round.

The sections below are the findings I find most instructive. Each gives the
failure scenario, the code, and the test that now catches it.

### 4.1 The device tree that QEMU dropped without a message

K0 linked the kernel at `0x40080000`, with this comment: RAM starts at
`0x40000000`, "the first 512 KiB are left free because QEMU places the device
tree blob at the start of RAM".

The reviewer's finding (round 1, item 6) was that the comment was false: QEMU
dropped the device tree. After the fix the comment reads that QEMU puts the
1 MiB blob at the start of RAM **only when the kernel image does not overlap
it**. The image started 512 KiB into RAM and the blob is 1 MiB, so the two
overlapped, and QEMU dropped the blob.

```diff
-    /* RAM on QEMU `virt` starts at 0x40000000; the first 512 KiB are left
-       free because QEMU places the device tree blob at the start of RAM. */
-    . = 0x40080000;
+    /* RAM on QEMU `virt` starts at 0x40000000. QEMU puts the device tree blob
+       (1 MiB) there, but only when the kernel image does not overlap it, so
+       the kernel is linked 2 MiB above the start of RAM. */
+    . = 0x40200000;
```

The pull request records the check I used: QEMU's `info roms` lists the blob
as loaded.

Nothing in the kernel read the device tree yet, so no test could fail. A wrong comment
is the kind of thing a test cannot see.
**It has no automated gate today.** Roadmap item K2 (device tree) is where it
gets one: its gate boots with two different memory sizes and prints each. Until
then, only the manual `info roms` check covers it. I treat that as an open
weakness rather than a closed finding.

### 4.2 The `.bss` that QEMU zeroed by itself

The K0 boot code clears `.bss` in a loop, because Rust assumes zero-initialised
statics:

```asm
    adrp x0, __bss_start
    add  x0, x0, :lo12:__bss_start
    adrp x1, __bss_end
    add  x1, x1, :lo12:__bss_end
1:
    cmp  x0, x1
    b.hs 2f
    str  xzr, [x0], #8
    b    1b
```

Round 1, item 4: nothing covered this. The reason is subtle. QEMU's ELF loader
zeroes `.bss` itself, so a kernel whose loop is broken (or deleted) still sees
zeroes and still passes. The fix had two parts. A self-test in
`kernel/src/memory_image.rs` reads a zero-initialised probe and an initialised
probe at boot and asserts on both:

```rust
static ZEROED_PROBE: AtomicU64 = AtomicU64::new(0);
static INITIALISED_PROBE: AtomicU64 = AtomicU64::new(INITIALISED_PATTERN);
// ...
assert!(zeroed == 0, ".bss was not cleared by the boot code");
```

and, in round 2, a second boot mode in the smoke test that bypasses the ELF
loader: it converts the kernel to a raw image with `llvm-objcopy`, loads it at
the entry address, and loads a file of `0xff` bytes over the `.bss` range
first.

```bash
check_boot raw \
    -device "loader,file=$raw_image,addr=$entry,cpu-num=0,force-raw=on" \
    -device "loader,file=$garbage,addr=$bss_start,force-raw=on"
```

The script's header says this outright: only the `raw` boot proves that the
boot code clears `.bss`. The mutation that now fails the gate is "`.bss` loop
that does not clear", listed in PR #3.

One correction in the middle: the first version of the fix, committed for
round 1, had a header in the smoke test calling this a "known gap" to be fixed
later by a raw-image boot. The raw boot arrived in the round 2 commit. For one
commit, the repository admitted it had a hole.

### 4.3 The test that passed with a kernel stuck in an exception

Round 1, item 3: the gate passed with a `panic` right after the banner. The
fix made the smoke test wait one second after the banner and fail if QEMU
exited or if "kernel panic" appeared.

Round 2, item 1: the gate still passed with the kernel stuck in an exception
loop after the banner. A hang prints nothing and QEMU keeps running, so
"alive and silent" looks identical to "alive and fine". K0 had no exception
vector table. The fix installs one whose every entry ends in a panic:

```asm
unhandled_exception_entry:
    // The interrupted stack may be what faulted, and this path never returns,
    // so start again from the top of the boot stack.
    adrp x2, __stack_top
    add  x2, x2, :lo12:__stack_top
    mov  sp, x2

    mrs  x0, esr_el1
    mrs  x1, elr_el1
    b    unhandled_exception
```

Mutations recorded in PR #3 and caught by the gate after these fixes: "panic
after the banner" and "undefined instruction after the banner".

The general point is about what a smoke test asserts. "The banner appeared" is
a positive signal, and a kernel can emit it and then be broken. A useful test
also asserts the **absence** of the failure modes you can name (a panic marker,
a premature QEMU exit, a second banner) and makes unnamed ones loud, which is
what the vector table does.

### 4.4 Cores that nobody parked

K0 let every core that QEMU starts run `_start`, on one shared boot stack. The
reviewer (round 1, item 7): secondary cores were not parked. The fix is the
first thing `_start` does after masking interrupts:

```asm
    mrs  x0, mpidr_el1
    and  x0, x0, #0xffffff
    cbnz x0, .Lpark
```

Round 2, item 6: the parking was never exercised by the gate. The `smp` boot
uses `-machine secure=on -smp 4`; the script's own comment says that with
`secure=on` QEMU does not hold the secondary cores back, so only this boot
proves the boot code parks them. The mutation "secondary
cores not parked" is caught by it.

I should be careful with this finding. The sources I have show the
**mechanism** (several cores sharing one stack and one UART, and the
`SAFETY` comment on the console that depends on there being one core) but not
a recorded observation of garbled serial output. I will not describe one.
[Author: if you did see interleaved output while debugging, say so here; it is
not in the repository or in the pull requests.]

The same finding has a limit I left in place. Round 2, item 7 pointed out that
the boot-core test ignores affinity level 3 and assumes the boot core has
affinity 0. I did not change it: it is not reproducible on QEMU `virt`, the
only supported board, and the assumption is now stated in `boot.s`. This is a
real portability limit, accepted knowingly.

### 4.5 A self-test that could not tell its own bug from success

K1 added the saved-context exception handler and a self-test: take a
breakpoint with distinct values in `x0`..`x30`, the flags and `SP_EL0`, and
check that every one came back.

The PR #7 description records an honest failure of the process itself: the
mutations "x19 not restored" and "`SPSR` not restored" were **not** caught on
the first run. My reading, from the comment in the code, is that the registers still held the
interrupted values when the handler returned, so a missing restore went
unseen. The fix was to scrub everything the frame holds after saving it, so
the restore is the only source of the interrupted values:

```asm
.irp n, 1, 2, 3, ..., 29
    mov  x\n, xzr
.endr
    msr  sp_el0, xzr
    msr  spsr_el1, xzr
    msr  elr_el1, xzr
    bl   handle_sync_exception
```

(abbreviated; the register list is complete in `exceptions.s`). After this,
each of these mutations failed the gate: `ELR` not advanced, `x19`, `x30` or
`SPSR` not restored, `x29` and `x30` swapped, the stack reset dropped, and
others listed in the pull request. The "x29/x30 undistinguished" finding in
round 1 was the same lesson in miniature: a test that cannot tell two
registers apart cannot detect their being swapped.

### 4.6 The breakpoint the compiler also emits

The handler resumes after a breakpoint so the kernel can test its own
exception path. Round 1 of PR #7: any `BRK` resumed. The Rust compiler lowers
`abort` and `unreachable` to `brk #0x1`, so a real bug would be silently
stepped over. The fix restricts resumption to one dedicated immediate:

```rust
/// The only BRK immediate the kernel resumes from. The compiler lowers
/// `abort` and `unreachable` to `brk #0x1`, and those must stay fatal.
const RESUMABLE_BREAKPOINT: u64 = 0x800;
// ...
if syndrome.class == EC_BRK_A64 && syndrome.iss & 0xffff == RESUMABLE_BREAKPOINT {
```

A later commit added the gate: a kernel built with the `selftest-compiler-trap`
feature executes `brk #0x1` and the smoke test requires it to end in a kernel
panic that names that immediate. The mutation "any BRK resumes" fails it.

### 4.7 The stack reset nobody tested

The unhandled-exception path starts again from the top of the boot stack,
because the faulting stack may be the cause. Round 2 of PR #7: the gate did not
exercise this. A test that panics with a valid stack passes even if the reset is
removed. The test now sets `SP_EL1` to an invalid value first and then takes a
`BRK` on a vector entry that is not handled:

```rust
unsafe { asm!("mov x0, #0x10", "mov sp, x0", "msr spsel, #0", "brk #0", out("x0") _) };
```

The mutation "stack reset dropped" is caught.

### 4.8 The hooks

Several findings concerned the tooling that runs the gate, not the kernel.
Round 1 of PR #3: the pre-commit hook did not exist in a new git worktree
(it was a husky hook). Round 2: a commit could pass the hook when a needed
file was untracked; merge commits skipped the hook; and the hook's message
suggested a plain `stash`, which unstages the commit. The fix was tracked hooks
under `.githooks/`, a `pre-merge-commit` hook, and a refusal when the working
tree has unstaged or untracked files:

```sh
if ! git diff --quiet || [ -n "$(git ls-files --others --exclude-standard)" ]; then
    echo "pre-commit: the working tree has unstaged changes or untracked files." >&2
```

The reason is in the comment: the checks read the working tree, so it must hold
exactly what is being committed.

## 5. Discussion

**What kind of bug the reviewers found.** Of the findings above, few are
defects in the boot assembly itself (the unparked cores is the clear case; the
resumable `BRK` and the stale syndrome are handler logic). The others are
tests that could not fail (`.bss`, the stuck kernel, the register restore, the
stack reset, the compiler trap), a comment that was false (the device tree),
and tooling that checked the wrong tree. That is a statement about this small
code base and about these reviewers, not a general result.

**The fresh context mattered in one specific way.** A reviewer that has not
read my reasoning could not inherit my belief that "QEMU puts the device tree
there". It read the comment as a claim and tested it. I did not run a control
without that isolation, so I cannot say how many findings it caused.

**Mutation testing was the useful half.** A finding was only closed when I
could break the behaviour and see the gate fail. The `x19` and `SPSR` case
shows that a plausible-looking test failed this standard on its first run.

**Rebuttals are part of the record.** Not every finding was accepted: the
reviewers' reports on hook behaviour during rebase and cherry-pick were
rebutted because the README documents it and CI plus branch protection cover
it, and the finding that `arch` imports `board` was rebutted because no ADR
forbids it and the panic handler already does the same. Whether those were the
right calls is for the reader; the reasons are in the pull requests.

## 6. Limitations and what review did not catch

- **Findings I rebutted or left**: boot-core affinity assumed 0 (4.4); no test
  of IRQ and FIQ paths, because that needs an interrupt controller (K6); no
  release-mode boot; the hooks do not run on rebase or cherry-pick.
- **A documented known limit**: a synchronous exception with an invalid
  `SP_EL1` loops silently until guard pages exist (K4). The saved-context path
  trusts the stack pointer.
- **No automated gate for the device tree placement** (4.1) until K2.
- **Everything ran on QEMU only.** Nothing in the gate says the boot path works
  on other hardware; the code states several assumptions that hold on QEMU
  `virt`.
- **I cannot tell what review missed.** By construction, the findings I know
  about are the ones that were found. Four rounds ending in "no confirmed
  finding" is the termination rule, not evidence of correctness. The bugs that
  would surface on real hardware, or under interrupts, have not been looked for
  yet.
- **Counts are as stated in the pull request descriptions.** The description of
  PR #7 says round 2 had 4 findings but lists five items (four fixed, one
  rebutted); I report it as written. The description of PR #3 was written while
  a third round was in progress, and says so.
- **No comparison.** I did not measure review cost, or run the same code
  without reviewers, so I make no claim of effectiveness.

## 7. Conclusion

For a boot path this small, the most valuable review output was not a bug in
the assembly. It was the discovery that my checks could not fail where they
claimed to. The practical rules I take from this are: make every positive
check have an inverse that you have actually broken; do not trust the emulator
to be hostile (dirty the memory yourself, start more cores, start at the wrong
exception level); treat comments that explain a design choice as claims to be
tested; and keep the rebuttals next to the fixes.

The next kernel milestone (K2) reads the device tree, which gives finding 4.1
the automated gate it still lacks.

## Sources and where each claim can be checked

Repository: `iuriandreazza/workspace-os`. Paths are relative to its root; PR
numbers are on GitHub; commits are in `git log`.

| # | Claim | Where to check |
|---|---|---|
| 1 | The project is a Rust aarch64 microkernel run in QEMU, GPL-2.0-only; `main` protected with PR and CI required; repository public | ADR 0001, 0004, 0005, `CLAUDE.md` ("Checks"); `gh repo view iuriandreazza/workspace-os --json visibility` |
| 2 | Models as control plane, model proposes and deterministic code decides, system boots with no model | ADR 0002, "Decision", rules 1 and 2 |
| 3 | `check.sh` is the single definition of the gate and runs locally, in hooks and in CI | `CLAUDE.md` "Checks"; `scripts/check.sh`; `.githooks/` |
| 4 | The review procedure (fresh-context subagent, diff and ADRs only, concrete failure scenario, rounds until none confirmed, recorded in PR) | `CLAUDE.md` "Review: always adversarial, always local" |
| 5 | PR #3: two rounds, 12 and 10 findings, none critical; a third round in progress when the description was written | `gh pr view 3` |
| 6 | PR #7: four rounds; findings 7, 4, 6 and none; rebuttals listed in rounds 1 to 3 | `gh pr view 7` |
| 7 | PR #3 outcomes: 20 fixed, round 2 item 7 not changed, round 2 item 9 count fixed and window not | Tables in `gh pr view 3` (counted by me) |
| 8 | PR #7 round 2 says 4 findings but lists five items | `gh pr view 7` |
| 9 | K0 linked at `0x40080000` with a comment about a 512 KiB gap; fixed to `0x40200000` with the 1 MiB overlap explanation | `git show d621ae8 -- kernel/src/arch/aarch64/link.ld`; `git show b58b67f:kernel/src/arch/aarch64/link.ld` |
| 10 | Round 1 finding 6, "Linker comment was false: QEMU dropped the device tree"; `info roms` check | `gh pr view 3` |
| 11 | No automated device tree gate yet; K2's gate is two memory sizes | `docs/roadmap.md`, row K2 |
| 12 | `.bss` clear loop in `boot.s` | `kernel/src/arch/aarch64/boot.s` (`.Lel1`), original in `git show b58b67f:kernel/src/arch/aarch64/boot.s` |
| 13 | QEMU's ELF loader zeroes `.bss`; raw boot over `0xff` garbage; `smp` boot with `secure=on` | `scripts/boot-smoke-test.sh` header and `check_boot raw/smp`; `git show 7bb6141 -- scripts/boot-smoke-test.sh` |
| 14 | Memory-image probes and assertion messages | `kernel/src/memory_image.rs` |
| 15 | The first fix called the `.bss` hole a "known gap"; the raw boot came in the next commit | `git show d621ae8 -- scripts/boot-smoke-test.sh`; `git show 7bb6141 -- scripts/boot-smoke-test.sh` |
| 16 | Round 1 item 3 (panic after banner passed), round 2 item 1 (exception loop passed) | `gh pr view 3` |
| 17 | The vector table and `unhandled_exception_entry` added in the second review commit | `git show 7bb6141 -- kernel/src/arch/aarch64/exceptions.s` |
| 18 | Mutations caught: panic after banner, undefined instruction after banner, `.bss` loop that does not clear, secondary cores not parked | "Test plan" in `gh pr view 3` |
| 19 | Round 1 item 7 (cores not parked), round 2 item 6 (parking never exercised), round 2 item 7 (MPIDR affinity assumption, not changed) | `gh pr view 3`; `boot.s` comment on affinity 0 |
| 20 | The kernel console `SAFETY` comment relies on parking and masked interrupts | `git show d621ae8 -- kernel/src/board.rs` |
| 21 | No observation of garbled serial output is recorded | Absence: searched the repository and PRs 2, 3, 7 (nothing found); author to confirm |
| 22 | Mutations x19 and `SPSR` missed on first run, fixed by scrubbing live state | `gh pr view 7` ("Mutation tests"); `git show 77518d3`; scrub code in `exceptions.s` |
| 23 | Full list of K1 mutations caught | `gh pr view 7` |
| 24 | `brk #0x1` is emitted by the compiler for `abort` and `unreachable`; resume restricted to `#0x800`; gate for compiler trap | `git show ac3f01b -- kernel/src/arch/aarch64/exceptions.rs`; `git show df80a56`; `check_panic compiler-trap` in the smoke test |
| 25 | Stack reset test uses an invalid `SP_EL1` | `git show e944dbe -- kernel/src/arch/aarch64/exceptions.rs`; PR #7 round 2 |
| 26 | Hook did not exist in a new worktree; untracked file, merge commits, stash message; the tracked-hooks fix | `gh pr view 3` (round 1 item 1, round 2 items 2 to 4); `.githooks/pre-commit`, `git show 7bb6141 -- .githooks` |
| 27 | Rebuttals: hooks on rebase and cherry-pick, `arch` importing `board`, no IRQ/FIQ test, no release boot | `gh pr view 7` |
| 28 | Known limit: invalid `SP_EL1` loops silently until guard pages (K4) | `gh pr view 7` ("Known limit"); header of `exceptions.s` |
| 29 | Pull request #2 was the first boot, merged before the review process | `gh pr view 2`; `git log` |
| 30 | `docs/prior-art.md` does not cover code-review practice | `docs/prior-art.md`, section headings 1 to 4 |
| 31 | Mutations were run by hand; no script automates them in the repository | Absence: `ls scripts`; `rg mutation` finds nothing in the tree |
