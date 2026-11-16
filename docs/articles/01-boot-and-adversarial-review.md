---
title: "Article 1: Booting a Rust microkernel, and what adversarial review found in 63 lines of boot code"
type: article
status: draft
created: 2026-11-15
updated: 2026-11-16
tags: [article, kernel, boot, aarch64, review, testing]
---

# Booting a Rust microkernel, and what adversarial review found in 63 lines of boot code

> Draft for the author's review (roadmap item R2). Language: English. Format:
> scientific article, for general developers. Text in square brackets is a note
> to the author, not article text. Every factual claim is listed with its source
> in the table at the end; the commands for what I re-ran are in the appendix.
>
> Notes for the author, to settle before publishing:
>
> 1. **Title.** The roadmap row says "60 lines" and "two rounds". The first boot
>    code (`boot.s` plus `link.ld` at commit `b58b67f`) is 63 lines by
>    `wc -l`, comments and blanks included, and pull request #3 ran two rounds
>    on it. The article keeps that core and then extends to the later boot code
>    (K1, K2, K3, K2b-2), so the title says 63 and the roadmap wording is
>    unchanged.
> 2. **AI assistance.** Section 3 needs one or two sentences from you on how the
>    code itself was written. The repository cannot say: `CLAUDE.md` forbids
>    attribution in commits and pull requests.
> 3. **Garbled serial output.** I do not claim to have seen any (4.4). If you
>    did, add it.
> 4. **Hardware-only findings** (6.3) are reasoning that was accepted without a
>    hardware run. I did not open the Arm Architecture Reference Manual for
>    them. Say so, or open it and cite it.

## Abstract

I am building a microkernel in Rust for aarch64, run under QEMU, as the
deterministic core of an operating system in which language models act as a
control plane. Its first milestone was small: boot, print a banner on the
serial port, stay alive. The boot assembly and the linker script were 63 lines.
Before each pull request that changed the kernel or the bootloader was merged,
a reviewer that sees only the diff and the architecture decision records tried
to break it, and had to give a concrete failure scenario for every finding. Five
pull requests went through this process, with between one and six rounds each;
the first boot, merged before the process existed, was reviewed through the
first of them. This article reports what the reviewers found, starting with the
63 lines. The
recurring result is that the boot code was mostly not wrong in the obvious way;
the **tests** and **comments** around it were. I re-ran the original code and
show that its test passed with a deleted `.bss` clear, with a panic after the
banner and with an undefined instruction after the banner. I also report what
the process did not catch, which was found by running the code instead.

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

I do not claim novelty for the review idea, and I make no comparison with other
practices. The prior-art survey (`docs/prior-art.md`) covers kernel policy,
LLM-as-OS, driver reuse and boot models; it does not cover code-review
practice, and I did not survey the literature on it.

**Scope.** This article covers the kernel and bootloader pull requests: #2
(first boot), #3 (gate and review of the first boot), #7 (K1, exceptions), #9
(K2, device tree), #13 (K3, frame allocator) and #15 (K2b-2, UEFI bootloader).
The intent-shell pull requests (#14, #16, #17) were reviewed under the same
rule, and the documentation-only pull requests are exempt from it; they are out
of scope here, so the counts below are not the count of every review in the
repository.

## 2. The system under test

### 2.1 The 63 lines

The first kernel (pull request #2, commit `b58b67f`) had two files that carry
the boot logic. `boot.s` is 23 lines and is short enough to quote whole:

```asm
// Entry point. QEMU `virt` jumps here at EL1 with the MMU off.
.section .text.boot
.global _start
_start:
    adrp x0, __stack_top
    add  x0, x0, :lo12:__stack_top
    mov  sp, x0

    // Rust assumes zero-initialised statics.
    adrp x0, __bss_start
    add  x0, x0, :lo12:__bss_start
    adrp x1, __bss_end
    add  x1, x1, :lo12:__bss_end
1:
    cmp  x0, x1
    b.hs 2f
    str  xzr, [x0], #8
    b    1b
2:
    bl   kernel_main
3:
    wfe
    b    3b
```

`link.ld` is 40 lines: it places the image at `0x40080000`, then `.text`,
`.rodata`, `.data`, `.bss` and a 64 KiB boot stack. Together that is the 63
lines. `kernel_main` printed one line on a PL011 UART at a fixed address:
`workspace-os kernel: booted on aarch64`. The only test was a script that booted
QEMU and checked that this line appeared.

For scale: `boot.s` is 137 lines on `main` today (`wc -l`), because it now
enters EL1 from EL2 or EL3, parks cores, and has a second entry convention for
the bootloader.

### 2.2 The gate

A single script, `scripts/check.sh`, is the definition of "this change is
acceptable". It runs format, lint, host unit tests, a licence check and the
boot smoke test (`scripts/boot-smoke-test.sh`), and it is the only place checks
are defined: it runs locally, in tracked git hooks (`.githooks/pre-commit` and
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
5. The author verifies each finding against the code and either fixes it or
   rebuts it in writing with evidence.
6. Repeat with a new reviewer until a round produces no confirmed finding.
7. The outcome is recorded in the pull request description.

Two things about who did what, stated plainly. The reviewers were automated
agents, each started with a fresh context. The findings were then judged by the
author against the code. [Author: add one or two sentences on how the code
itself was written and how much of it used AI assistance. The sources in the
repository do not say, and this article should.]

A fix for a finding is accepted together with a gate: a change to the test
script that fails when the behaviour is broken. This was checked with
mutations: break the behaviour on purpose and confirm `check.sh` fails. The pull
requests record these mutations; the one in #13 reports "72 mutations, 53
killed, the survivors were equivalent or defence in depth". I found no script in
`scripts/` that automates them, and I did not re-run all of them. They were
reported by the author; I re-ran the ones marked as re-run below.

## 4. Scope and counts

| Pull request | Content | Rounds | Findings, as the description states them |
|---|---|---|---|
| #2 | First boot (K0): the 63 lines | none, merged before the process existed | none |
| #3 | Gate, hooks, CI, review policy; reviews the K0 kernel | 2, and a third "in progress" when the description was written | 12 + 10 = 22, none critical; 20 fixed, 1 not changed, 1 fixed in part (counted by me from its tables) |
| #7 | K1: exceptions and entry to EL1; also covers code on `main` without a clean round | 4 | 7 + 4 + 6 + 0 = 17 by the round counts; some rounds list rebuttals |
| #9 | K2: memory and UART from the device tree | 1 | 0 confirmed, plus minor observations |
| #13 | K3: physical frame allocator | 6 | confirmed in rounds 1, 2, 4 and 5; none in 3 (partial review) and 6 |
| #15 | K2b-2: UEFI bootloader, boot handoff, disk image | 5 | 11 in round 1; later rounds report mostly gaps in the gate |

Counts are as the descriptions state them. The description of #7 says round 2
had four findings but lists five items (four fixed, one rebutted); I report it
as written. The review of #3 therefore also covered the K0 kernel, and the
review of #7 covered code that had reached `main` without a clean round.

## 5. The 63 lines under review

The first two rounds, in pull request #3, reviewed the 63 lines and the tooling
around them. The sections below are the findings about the code and its gate.
Each gives the failure scenario, the code, and the test that now catches it.

**What I re-ran.** Everything marked *re-run* below I executed myself, on this
machine, in a scratch copy of the original commit, not from the pull request
text. The setup is in the appendix. The QEMU version was 10.0.13 (Debian);
the version the author used is not recorded, and behaviour such as the device
tree placement may depend on it.

### 5.1 The device tree that QEMU dropped without a message

K0 linked the kernel at `0x40080000`, with this comment: RAM starts at
`0x40000000`, "the first 512 KiB are left free because QEMU places the device
tree blob at the start of RAM".

The reviewer's finding (round 1, item 6) was that the comment was false: QEMU
dropped the device tree. After the fix the comment says that QEMU puts the
1 MiB blob at the start of RAM **only when the kernel image does not overlap
it**. The image started 512 KiB into RAM and the blob is 1 MiB, so the two
overlapped.

```diff
-    /* RAM on QEMU `virt` starts at 0x40000000; the first 512 KiB are left
-       free because QEMU places the device tree blob at the start of RAM. */
-    . = 0x40080000;
+    /* RAM on QEMU `virt` starts at 0x40000000. QEMU puts the device tree blob
+       (1 MiB) there, but only when the kernel image does not overlap it, so
+       the kernel is linked 2 MiB above the start of RAM. */
+    . = 0x40200000;
```

*Re-run*, with QEMU's monitor command `info roms` (result lines only; the ACPI
rows and the monitor's line editing are removed, and the scratch path in the
ROM name is shortened):

```
K0 as merged, b58b67f (linked at 0x40080000):
addr=0000000040080000 size=0x001f7c mem=ram name="…/kernel ELF program header segment 0"
addr=0000000040081f80 size=0x0007a5 mem=ram name="…/kernel ELF program header segment 1"
serial: workspace-os kernel: booted on aarch64

After the fix, d621ae8 (linked at 0x40200000):
addr=0000000040000000 size=0x100000 mem=ram name="dtb"
addr=0000000040200000 size=0x002184 mem=ram name="…/kernel ELF program header segment 0"
addr=0000000040202188 size=0x000915 mem=ram name="…/kernel ELF program header segment 1"
addr=0000000040202aa0 size=0x000010 mem=ram name="…/kernel ELF program header segment 2"
serial: workspace-os kernel: booted on aarch64
```

So on this QEMU the blob is absent from the list of loaded ROMs when the image
is linked at `0x40080000`, and present at `0x40000000` with size `0x100000`
(1 MiB) when it is linked at `0x40200000`. Nothing on QEMU's output said that
the blob was dropped, and the kernel printed its banner in both cases.

The device tree was not read by anything then, so no test could fail. Kernel
milestone K2 started reading it, and the finding now has a gate in three parts:

- K2 (pull request #9) boots with two memory sizes and checks the memory and UART
  lines on every boot. Its mutation "fallback address moved" fails the ELF boot.
- K3 (pull request #13) reserves the device tree blob in the frame allocator;
  its round 1 finding was that the smoke test passed with the blob unreserved,
  and the gate now checks the blob line and the reserved count.
- Every direct boot on `main` prints the reservation. As printed on `main` at
  `50d808e` (colour codes removed):

```
  kernel  info  reserved device tree 0x40000000..0x40100000
```

I also tried the original mistake on today's kernel. In a scratch copy of
`main` at `50d808e` I changed the link address back to `0x40080000` and booted
it the same way as the unmodified build. The unmodified build printed its
log. The relinked one printed **nothing** in 10 seconds, and `info roms` lists
no `dtb`. I did not attach a debugger. My reading is that the kernel found no
device tree, so it found no UART, and the console drops output until it finds
one (pull request #9 describes that behaviour). So the original K0 mistake no
longer looks like success: the boot is silent. I ran the smoke test script on that
build, and its first boot printed `FAIL (elf): boot banner not found within 10s`.

Placement came back once more, from a different direction. The `0x40200000`
address that fixed this finding is itself an assumption about the machine. When
the bootloader was run on VirtualBox (Arm, 7.2.22), the firmware reserved a
region that covers `0x40200000` on a 1024 MiB machine, and the VM needs 2048 MiB
(ADR 0013, "Findings of K2b-2"). That was found by running it, not by a
reviewer.

### 5.2 The test that passed with a broken kernel

The K0 smoke test waited for the banner and exited with `PASS`. I applied three
mutations to the original commit and ran its own script (`b58b67f`,
`scripts/boot-smoke-test.sh`, unchanged):

| Mutation of K0 | Result, as printed |
|---|---|
| `.bss` clear loop skipped (`b.hs 2f` replaced by `b 2f`) | `PASS: boot banner found on serial console` |
| `panic!("injected")` right after the banner | `PASS: boot banner found on serial console` |
| `udf #0` right after the banner | `PASS: boot banner found on serial console` |

All three are *re-run* results. This is the central fact of the article in
three lines: the gate for the 63 lines accepted a kernel with a deleted `.bss`
clear, a kernel that panics, and a kernel that takes an undefined-instruction
exception, because its only assertion was that one line had appeared.

The review's first two rounds each turned part of this into a gate:

- Round 1, item 3: the gate passed with a `panic` right after the banner. The
  fix made the smoke test wait one second after the banner and fail if QEMU
  exited or if "kernel panic" appeared.
- Round 2, item 1: the gate still passed with the kernel stuck in an exception
  loop after the banner. A hang prints nothing and QEMU keeps running, so
  "alive and silent" looks identical to "alive and fine". K0 had no exception
  vector table. The fix installs one whose every entry ends in a panic. This is
  the version in the second review commit (`7bb6141`); it has since been
  extended:

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

I ran the same style of mutation against the gate as it is on `main` at
`50d808e`, one at a time, in scratch copies:

| Mutation of today's kernel | Gate result, as printed |
|---|---|
| `.bss` clear loop skipped | `PASS (elf)`, then `FAIL (raw): boot banner not found within 10s` |
| `udf #0` right after the `resumed after breakpoint` log line | `FAIL (elf): kernel panicked after the boot banner` |
| parking branch (`cbnz x0, .Lpark`) replaced by `nop` | `PASS` for `elf`, `raw`, `mem-64` and `mem-512`, then `FAIL (smp): boot banner not found within 10s` |

The first row shows the point of 5.3: the ELF boot still passes with the
`.bss` loop deleted, and only the raw boot fails. I did not investigate why the
`raw` and `smp` failures appear as "banner not found" rather than as a more
specific message; the gate fails, which is what this check asks.

### 5.3 The `.bss` that QEMU zeroed by itself

The K0 boot code clears `.bss` in a loop (the quoted `boot.s` above), because
Rust assumes zero-initialised statics.

Round 1, item 4: nothing covered this. The reason is subtle. QEMU's ELF loader
zeroes `.bss` itself, as the script header states, so a kernel whose loop is
broken still sees zeroes. The `.bss` row of 5.2 is consistent with that: the ELF
boot passes without the loop, the raw boot does not. The fix had two parts. A
self-test in `kernel/src/memory_image.rs` reads a zero-initialised probe and an
initialised probe at boot and asserts on both:

```rust
static ZEROED_PROBE: AtomicU64 = AtomicU64::new(0);
static INITIALISED_PROBE: AtomicU64 = AtomicU64::new(INITIALISED_PATTERN);
// ...
assert!(zeroed == 0, ".bss was not cleared by the boot code");
```

and, in round 2, a second boot mode in the smoke test that bypasses the ELF
loader: it converts the kernel to a raw image with `llvm-objcopy`, loads it at
the entry address, and loads a file of `0xff` bytes over the `.bss` range
first (commit `7bb6141`).

```bash
check_boot raw \
    -device "loader,file=$raw_image,addr=$entry,cpu-num=0,force-raw=on" \
    -device "loader,file=$garbage,addr=$bss_start,force-raw=on"
```

The script's header says this outright: only the `raw` boot proves that the
boot code clears `.bss`.

One correction in the middle: the first version of the fix, committed for
round 1, had a header in the smoke test calling this a "known gap" to be fixed
later by a raw-image boot. The raw boot arrived in the round 2 commit. For one
commit, the repository admitted it had a hole.

### 5.4 Cores that nobody parked

K0 let every core that QEMU starts run `_start`, on one shared boot stack. The
reviewer (round 1, item 7): secondary cores were not parked. The fix is the
first thing `_start` does after masking interrupts (commit `d621ae8`):

```asm
    mrs  x0, mpidr_el1
    and  x0, x0, #0xffffff
    cbnz x0, .Lpark
```

Round 2, item 6: the parking was never exercised by the gate. The `smp` boot
uses `-machine secure=on -smp 4`; the script's own comment says that with
`secure=on` QEMU does not hold the secondary cores back, so only this boot
proves the boot code parks them. The mutation in 5.2 (the branch replaced by
`nop`) fails the gate on that boot.

I should be careful with this finding. The sources I have show the
**mechanism** (several cores sharing one stack and one UART, and the `SAFETY`
comment on the console that depends on there being one core) but not a recorded
observation of garbled serial output. I will not describe one. [Author: if you
did see interleaved output while debugging, say so here; it is not in the
repository or in the pull requests.]

The same finding has a limit I left in place. Round 2, item 7 pointed out that
the boot-core test ignores affinity level 3 and assumes the boot core has
affinity 0. It was not changed: it is not reproducible on QEMU `virt`, the only
supported board, and the assumption is now stated in `boot.s`. This is a real
portability limit, accepted knowingly.

### 5.5 A comment that was a claim

Round 2, item 8 of pull request #3: the comment in `memory_image.rs` was false
about `.data`. With 5.1 (the linker comment), that makes two comments in the
first review that stated a reason that was not true. Neither could be found by a
test; both were found by a reader that did not share the author's belief.

## 6. Later boot code

### 6.1 K1: exceptions and entry to EL1

K1 added the saved-context exception handler, entry to EL1 from EL1, EL2 or EL3,
and a self-test: take a breakpoint with distinct values in `x0`..`x30`, the
flags and `SP_EL0`, and check that every one came back. The gate also boots at
EL2, EL3 and EL3-with-EL2.

**A self-test that could not tell its own bug from success.** The PR #7
description records an honest failure of the process itself: the mutations "x19
not restored" and "`SPSR` not restored" were **not** caught on the first run. My
reading, from the comment in the code, is that the registers still held the
interrupted values when the handler returned, so a missing restore went unseen.
The fix, commit `77518d3`, scrubs everything the frame holds after saving it,
so the restore is the only source of the interrupted values. The current
`exceptions.s` has it:

```asm
.irp n, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29
    mov  x\n, xzr
.endr
    msr  sp_el0, xzr
    msr  spsr_el1, xzr
    msr  elr_el1, xzr
    bl   handle_sync_exception
```

After this, each of these mutations failed the gate: `ELR` not advanced, `x19`,
`x30` or `SPSR` not restored, `x29` and `x30` swapped, the stack reset dropped,
and others listed in the pull request. The "x29/x30 undistinguished" finding in
round 1 was the same lesson in miniature: a test that cannot tell two registers
apart cannot detect their being swapped. I did not re-run the original
"x19 not restored" run, so the explanation above is a reading, not an
observation.

**The breakpoint the compiler also emits.** The handler resumes after a
breakpoint so the kernel can test its own exception path. Round 1: any `BRK`
resumed. The Rust compiler lowers `abort` and `unreachable` to `brk #0x1`, so a
real bug would be silently stepped over. The fix restricts resumption to one
dedicated immediate (`exceptions.rs`, today):

```rust
/// The only BRK immediate the kernel resumes from. The compiler lowers
/// `abort` and `unreachable` to `brk #0x1`, and those must stay fatal.
const RESUMABLE_BREAKPOINT: u64 = 0x800;
// ...
if syndrome.class == EC_BRK_A64 && syndrome.iss & 0xffff == RESUMABLE_BREAKPOINT {
```

A later commit (`df80a56`) added the gate: a kernel built with the
`selftest-compiler-trap` feature executes `brk #0x1` and the smoke test requires
it to end in a kernel panic that names that immediate. The mutation "any BRK
resumes" fails it.

**The stack reset nobody tested.** The unhandled-exception path starts again
from the top of the boot stack, because the faulting stack may be the cause.
Round 2: the gate did not exercise this. A test that panics with a valid stack
passes even if the reset is removed. The test now sets `SP_EL1` to an invalid
value first and then takes a `BRK` on a vector entry that is not handled:

```rust
unsafe { asm!("mov x0, #0x10", "mov sp, x0", "msr spsel, #0", "brk #0", out("x0") _) };
```

The mutation "stack reset dropped" is caught.

### 6.2 K2: one round, no confirmed finding

Pull request #9 (K2, the kernel reads memory and the UART from the device tree)
is the counterpoint. Its description records one round with a fresh-context
reviewer that also fuzzed 3 million mutations of a real QEMU blob, and **0
confirmed findings**. The minor observations: a `SAFETY` comment claimed the
contents were validated, but the claimed size staying inside RAM was in fact
trusted from the loader (fixed in `9c95d32`; the comment is now explicit about
it); an enabled memory node with an unusable `reg` gives `MalformedReg`
(rebutted: reporting malformed firmware is deliberate and tested); the
RAM-start fallback is a blind read on other boards (QEMU-only by design, in ADR
0010); and no boot with a non-zero `x0` is in the gate (a known gap).

A reader who expects review to find something every time should note this. One
round with no confirmed finding is what the procedure's stop rule allows. The
change also came with 16 host tests that truncate the blob at every length and
flip a bit at every byte (PR #9). I draw no conclusion about why this round
found nothing.

### 6.3 K3 and K2b-2: findings about the gate, and about hardware

In pull requests #13 (K3, six rounds) and #15 (K2b-2, five rounds), much of
what the later rounds found concerned whether the gate could fail.

- **K3 round 1:** the smoke test passed with the device tree blob unreserved.
  **Round 2:** the gate passed with tree reservations dropped, or the blob shifted
  or shrunk; the cost per range grew bit by bit, so a hostile blob could stall
  the boot; and a memory node without `reg` became a panic, which the
  description marks as a regression. **Round 4:** the gate could not detect a wrong
  `__image_start`. All were fixed with a gate change or a test; the `memreserve`
  boot, which hands the kernel a tree with two `/memreserve/` entries, is the
  new boot from round 2.
- **K2b-2 round 1** (11 findings): the kernel hid a corrupt device tree copy and
  the gate did not check it; integer overflows in `validate` and
  `LoadPlan::span`; a missing `SPSel` and interrupts unmasked after
  `ExitBootServices` (the fix, commit `51ee06e`, selects `SP_EL1` and masks
  interrupts before leaving boot services). **Round 2:** no code defect;
  the gate did not check `boot_cpu`, `rsdp`, the EFI table or `x2|x3`. **Round
  3:** the gate did not prove that the EL1 path installs the bootloader's page
  tables (the kernel now prints `TTBR0_EL1` and the gate compares it). **Round
  4:** no code defect; gate gaps closed. **Round 5:** no code defect and no ADR
  violation; one gate hole: an EL2 handoff above 4 GiB, where a wrong `IPS`
  field "hung silently".

The last item is the same lesson as 5.2 on a harder case: a hang prints
nothing. It was closed with the `uefi-el2-high` boot (EL2, 3.5 GiB); the
mutation "EL2 without `IPS`" now fails the gate, per the pull request.

Two of these are hardware-reasoned. I report them as such.

- **K3 round 5:** `AtomicBool::swap` compiles to an exclusive load/store loop.
  The commit (`0004e75`) says that such a loop "may never succeed on real cores
  while the MMU is off and memory is Device". It replaced the `swap` with a
  plain load and a store; the pull request says there is no exclusive
  instruction in the binary. QEMU cannot show this, and no hardware run is
  recorded. This is the reviewer's reasoning, accepted. I did not open the Arm
  Architecture Reference Manual to check the rule it relies on.
- **K2b-2:** cache and TLB maintenance, and break-before-make on the EL1
  `TTBR0` switch, are **not covered by any test, by decision**: QEMU's
  translation does not model them (PR #15, and the limit in ADR 0013).

One more place where the process bent: after round 5 of #15, the author wrote
that "no sixth round was run for those two gate additions; each was confirmed by
mutation instead". The stop rule in `CLAUDE.md` says to repeat until a round
produces no confirmed finding. That was a decision to stop early, recorded in
the pull request.

### 6.4 What running found that review did not

The K2b-2 pull request lists "findings from running it". None of them came from
a reviewer, and each one invalidated an assumption the code or an ADR had made:

- QEMU's EDK2 and VirtualBox publish a device tree only with ACPI off. With
  ACPI on, both publish ACPI tables only, which the kernel does not read (ADR
  0010), so the bootloader stops with "the firmware offers ACPI only".
- On VirtualBox, RAM starts at `0x08000000`, not `0x40000000`, and the firmware
  reserves the kernel's load address on a 1024 MiB machine (see 5.1).
- VirtualBox's manual says Arm VMs have no serial port; with 7.2.22 a PL011
  works at `0xffdde000`.
- A byte was lost on VirtualBox's virtual serial port in one run, so the log
  reads `24 memry map entries`. The pull request records it as printed. The
  cause was not investigated.

This is the limit of local review that sees only a diff: it cannot know what
the firmware publishes.

## 7. The hooks

Several findings concerned the tooling that runs the gate, not the kernel. In
PR #3, round 1: the pre-commit hook did not exist in a new git worktree (it was
a husky hook). Round 2: a commit could pass the hook when a needed file was
untracked; merge commits skipped the hook; and the hook's message suggested a
plain `stash`, which unstages the commit. The fix was tracked hooks under
`.githooks/`, a `pre-merge-commit` hook, and a refusal when the working tree has
unstaged or untracked files:

```sh
if ! git diff --quiet || [ -n "$(git ls-files --others --exclude-standard)" ]; then
    echo "pre-commit: the working tree has unstaged changes or untracked files." >&2
```

The reason is in the comment: the checks read the working tree, so it must hold
exactly what is being committed.

## 8. What the system prints

The project's own rule since PR #11 is that a kernel pull request includes the
serial output of a real boot of its final commit. In that spirit, this is a boot
of `main` at `50d808e` on this machine, copied as printed (colour codes
removed), `-m 128M` by default.

Direct load, the same command the cargo runner uses (`qemu-system-aarch64
-machine virt -cpu cortex-a72 -nodefaults -display none -serial stdio -kernel
target/aarch64-unknown-none-softfloat/debug/kernel`):

```
  +----------------------------------------------------------+
  |  workspace-os  kernel 0.1.0   -  aarch64  -  EL1
  +----------------------------------------------------------+
  kernel  ok    booted on aarch64 in EL1
  kernel  ok    entry: direct load
  kernel  ok    memory 128 MiB at 0x40000000
  kernel  ok    uart pl011 at 0x9000000
  kernel  info  reserved kernel image 0x40200000..0x40260050
  kernel  info  reserved boot stack 0x40260050..0x40270050
  kernel  info  reserved device tree 0x40000000..0x40100000
  kernel  info  reserved by the device tree: 0 regions
  kernel  info  frames: 32768 in memory map, 369 reserved, 32399 free
  kernel  ok    frames: exhausted after 32399 allocations, next request refused
  kernel  ok    frames: returned 32399, 32399 free again
  kernel  ok    frames: reused 0x40100000 after freeing it
  kernel  ok    frames: double free refused
  +----------------------------------------------------------+
  | entry   direct load by the QEMU loader
  | memory  128 MiB
  | frames  32399 free of 32768
  | console PL011 at 0x9000000
  +----------------------------------------------------------+
  kernel  info  exception: breakpoint #0x800 at 0x4020b214
  kernel  ok    resumed after breakpoint
```

Two of these lines trace to findings above: the device tree reservation (5.1,
K3 round 1) and the breakpoint that is taken and resumed (6.1). The allocation
lines are K3's own boot self-test.

Through the UEFI bootloader (`-machine virt,acpi=off -m 256M -bios
/usr/share/AAVMF/AAVMF_CODE.fd`, disk image from
`scripts/build-disk-image.sh`; the boot-info section and the first memory map
lines are elided here):

```
  boot  ok    started at EL1
  boot  ok    kernel loaded at 0x40200000..0x40270050, entry 0x40200000
  boot  ok    device tree at 0x47ef8000 (1052672 bytes), acpi rsdp 0x0, pl011 at 0x9000000
  ...
  kernel  info  sctlr_el1 0x30d0198d
  kernel  info  mapping: runtime services 0xff
  kernel  info  mapping: kernel image 0xff, uart 0x00, boot info 0xff, tables at 0x4e6b5000
  kernel  ok    memory 250 MiB at 0x40000000
  kernel  ok    uart pl011 at 0x9000000
  kernel  info  reserved kernel image 0x40200000..0x40260050
  kernel  info  reserved boot stack 0x40260050..0x40270050
  kernel  info  reserved device tree 0x4e753000..0x4e854000
  kernel  info  reserved by the boot map and device tree: 10 regions
  kernel  info  frames: 64128 in memory map, 7202 reserved, 56926 free
  kernel  ok    frames: exhausted after 56926 allocations, next request refused
  kernel  ok    frames: returned 56926, 56926 free again
  kernel  ok    frames: reused 0x40000000 after freeing it
  kernel  ok    frames: double free refused
  ...
  kernel  info  exception: breakpoint #0x800 at 0x4020b214
  kernel  ok    resumed after breakpoint
```

[Author: the full 77-line output is saved in the session notes folder next to
this draft's hand-over note; decide whether to quote it whole. It includes the
bootloader's ASCII-art banner.]

On the same machine and commit, `scripts/boot-smoke-test.sh` printed 17 `PASS`
lines and no `FAIL`: `elf`, `raw`, `mem-64`, `mem-512`, `smp`, `el2`, `el3`,
`el3-el2`, `memreserve`, `uefi`, `uefi-768`, `uefi-el2`, `uefi-el2-high`,
`uefi-acpi-only`, `unhandled-class`, `unhandled-vector`, `compiler-trap`. I ran
that script alone, not `check.sh`; clippy, host tests and the licence check were
not re-run for this article.

All of it ran on QEMU (TCG) with the firmware shipped with Debian. The VirtualBox
runs in pull request #15 are the author's, recorded by hand; I did not repeat
them.

## 9. Discussion

**What kind of bug the reviewers found.** Of the findings above, few are
defects in the boot assembly itself (the unparked cores is the clear case;
the unmasked interrupts after leaving boot services and the `SPSel` omission in
K2b-2 are others). The resumable `BRK` and the stale syndrome are handler logic.
Most of the rest are tests that could not fail (`.bss`, the stuck kernel, the
register restore, the stack reset, the compiler trap, the unreserved blob, the
wrong `__image_start`, the silent EL2 hang), comments that were false (the
device tree gap, `.data`, a `SAFETY` claim), and tooling that checked the wrong
tree. That is a statement about this small code base and about these reviewers,
not a general result.

**The fresh context mattered in one specific way.** A reviewer that has not
read the author's reasoning could not inherit the belief that "QEMU puts the
device tree there". It read the comment as a claim and tested it. I did not run
a control without that isolation, so I cannot say how many findings it caused.

**Mutation testing was the useful half.** A finding was only closed when the
behaviour could be broken and the gate seen to fail. The `x19` and `SPSR` case
shows that a plausible-looking test failed this standard on its first run, and
the three K0 rows of 5.2 show that the first gate failed it for the whole
kernel.

**Rebuttals are part of the record.** Not every finding was accepted: the
reviewers' reports on hook behaviour during rebase and cherry-pick were
rebutted because the README documents it and CI plus branch protection cover
it, and the finding that `arch` imports `board` was rebutted because no ADR
forbids it and the panic handler already does the same. Whether those were the
right calls is for the reader; the reasons are in the pull requests.

## 10. Limitations and what review did not catch

- **Findings rebutted or left**: boot-core affinity assumed 0 (5.4); no test of
  IRQ and FIQ paths, because that needs an interrupt controller (K6); no
  release-mode boot; the hooks do not run on rebase or cherry-pick; a 512 GiB
  limit on the identity map, coarse memory types and other items recorded in
  ADR 0013 for K2b-2.
- **Documented limits**: a synchronous exception with an invalid `SP_EL1` loops
  silently until guard pages exist (K4); break-before-make and cache and TLB
  maintenance on the EL1 `TTBR0` switch are untested; the kernel's rejection
  paths for bad boot info and the bootloader's missing-file path are untested.
- **Everything ran on QEMU, plus two VirtualBox runs by the author.** Nothing in
  the gate says the boot path works on real hardware, and the exclusive-access
  and `IPS` findings are reasoning that no hardware run has confirmed.
- **I cannot tell what review missed.** By construction, the findings I know
  about are the ones that were found. A round that ends in "no confirmed
  finding" is the termination rule, not evidence of correctness. Section 6.4 is
  a list of things review did not find and running did.
- **The re-runs have limits.** One QEMU version, one machine, scratch copies of
  the commits, mutations of my own choosing. They show that the original gate
  accepted those kernels and that today's gate rejects them; they do not show
  that the gate rejects every broken kernel.
- **Counts are as stated in the pull request descriptions.** See section 4 for
  the inconsistency in #7 and the unfinished third round in #3.
- **No comparison.** Review cost was not measured and the same code was not
  developed without reviewers, so I make no claim of effectiveness or
  superiority over any other practice.

## 11. Conclusion

For a boot path this small, the most valuable review output was not a bug in
the assembly. It was the discovery that the checks could not fail where they
claimed to. The first gate accepted a kernel with a deleted `.bss` clear, a
panic, and an undefined instruction, and the gate on `main` rejected the equivalents I tried (5.2). The practical rules I take from this are: make every positive check
have an inverse that you have actually broken; do not trust the emulator to be
hostile (dirty the memory yourself, start more cores, start at the wrong
exception level, use more than one memory size); treat comments that explain a
design choice as claims to be tested; run the thing on a second platform, since
a diff review cannot see what firmware publishes; and keep the rebuttals next to
the fixes.

## Appendix: how I re-ran the K0 results

Everything was run in scratch directories outside the repository, with the
toolchain pinned by each commit's `rust-toolchain.toml`, QEMU 10.0.13 (Debian
`1:10.0.13+ds-0+deb13u1`).

```bash
git archive b58b67f | tar -x -C scratch/k0        # K0 as merged
git archive d621ae8 | tar -x -C scratch/k0fix     # after review round 1
(cd scratch/k0 && cargo build)
(sleep 2; echo "info roms"; sleep 1; echo quit) | qemu-system-aarch64 \
  -machine virt -cpu cortex-a72 -nodefaults -display none \
  -serial file:k0.serial -monitor stdio \
  -kernel scratch/k0/target/aarch64-unknown-none-softfloat/debug/kernel
```

`-nodefaults` is not in K0's own script at `b58b67f`; I added it to match the
current runner. For the K0 mutations, I edited a copy of `b58b67f` and ran its
own `scripts/boot-smoke-test.sh` (which does not use `-nodefaults`): in
`boot.s`, `b.hs 2f` to `b 2f`; in `main.rs`, `panic!("injected");` or
`unsafe { core::arch::asm!("udf #0") };` in place of `arch::halt()` after the
banner. For `main` at `50d808e`: `git archive HEAD`, the link address in
`link.ld` changed to `0x40080000` for 5.1, and `b.hs 2f` to `b 2f` (the `.bss`
loop), `cbnz x0, .Lpark` to `nop`, and an `udf #0` after the
`resumed after breakpoint` log line in `main.rs` for 5.2.

## Sources and where each claim can be checked

Repository: `iuriandreazza/workspace-os`. Paths are relative to its root; PR
numbers are on GitHub; commits are in `git log`. "Re-run" means I executed it
for this article; see the appendix.

| # | Claim | Where to check |
|---|---|---|
| 1 | The project is a Rust aarch64 microkernel run in QEMU, GPL-2.0-only; `main` protected with PR and CI required; repository public | ADR 0001, 0004, 0005, `CLAUDE.md` ("Checks"); `gh repo view iuriandreazza/workspace-os --json visibility` |
| 2 | Models as control plane, model proposes and deterministic code decides, system boots with no model | ADR 0002, "Decision", rules 1 and 2 |
| 3 | `check.sh` is the single definition of the gate and runs locally, in hooks and in CI; it runs format, lint, host tests, licence check and boot smoke test | `CLAUDE.md` "Checks"; `scripts/check.sh`; `.githooks/` |
| 4 | The review procedure (fresh-context subagent, diff and ADRs only, concrete failure scenario, rounds until none confirmed, recorded in PR) | `CLAUDE.md` "Review: always adversarial, always local" |
| 5 | The 63 lines: `boot.s` 23 lines, `link.ld` 40 lines at `b58b67f`; `boot.s` 137 lines on `main` | `git show b58b67f:kernel/src/arch/aarch64/boot.s \| wc -l` and `…/link.ld`; `wc -l kernel/src/arch/aarch64/boot.s` at `50d808e` |
| 6 | Scope: PRs #2, #3, #7, #9, #13, #15 are the kernel and bootloader PRs; #14, #16, #17 are the intent shell; #11 and #12 documentation | `gh pr list --state all`; PR titles and bodies |
| 7 | PR #3: two rounds, 12 and 10 findings, none critical; a third round in progress when the description was written; 20 fixed, round 2 item 7 not changed, round 2 item 9 count fixed and window not | `gh pr view 3` (tables counted by me) |
| 8 | PR #7: four rounds; findings 7, 4, 6 and none; rebuttals in rounds 1 to 3; round 2 says 4 but lists five items | `gh pr view 7` |
| 9 | PR #9: one round, 0 confirmed, 3M-mutation fuzz of a real QEMU blob, minor observations; mutations "fixed size", "fallback address moved" | `gh pr view 9`; `git show 9c95d32` |
| 10 | PR #13: six rounds and their findings; "72 mutations, 53 killed, the survivors were equivalent or defence in depth"; known blind spots | `gh pr view 13` |
| 11 | PR #15: five rounds and findings; mutations that fail the gate; items not covered by any test, by decision; no sixth round for the last two gate additions; findings from running; "memry" line | `gh pr view 15`; ADR 0013 "Findings of K2b-2" |
| 12 | K0 linked at `0x40080000` with a comment about a 512 KiB gap; fixed to `0x40200000` with the 1 MiB overlap explanation | `git show b58b67f:kernel/src/arch/aarch64/link.ld`; `git diff b58b67f d621ae8 -- kernel/src/arch/aarch64/link.ld` |
| 13 | **Re-run:** `info roms` for `b58b67f` (no `dtb`; image at `0x40080000`) and `d621ae8` (`dtb` at `0x40000000`, size `0x100000`; image at `0x40200000`); banner printed in both; QEMU 10.0.13 | Appendix |
| 14 | **Re-run:** `main` at `50d808e` relinked at `0x40080000` printed nothing in 10 s, `info roms` shows no `dtb`, and `scripts/boot-smoke-test.sh` printed `FAIL (elf): boot banner not found within 10s`; the unmodified build printed its log; the cause (no UART found) is my inference from PR #9's description that the console drops output until the UART is found | Appendix; `gh pr view 9` ("the console drops output until the UART is found") |
| 15 | Round 1 finding 6, "Linker comment was false: QEMU dropped the device tree"; `info roms` check noted in the test plan | `gh pr view 3` |
| 16 | K0's smoke test asserted only that the banner appeared and exited with `PASS` | `git show b58b67f:scripts/boot-smoke-test.sh` |
| 17 | **Re-run:** the K0 smoke test printed `PASS: boot banner found on serial console` with the `.bss` loop skipped, with `panic!` after the banner and with `udf #0` after the banner | Appendix |
| 18 | **Re-run:** gate on `main` at `50d808e`: `.bss` loop skipped fails `raw`; `udf #0` after the breakpoint log line fails `elf` with "kernel panicked after the boot banner"; parking branch replaced by `nop` fails `smp` | Appendix; output in the scratch directory of the session, not committed |
| 19 | QEMU's ELF loader zeroes `.bss`; raw boot over `0xff` garbage; `smp` boot with `secure=on` | `scripts/boot-smoke-test.sh` header and `check_boot raw/smp`; `git show 7bb6141 -- scripts/boot-smoke-test.sh` |
| 20 | Memory-image probes and assertion messages | `kernel/src/memory_image.rs` |
| 21 | The first fix called the `.bss` hole a "known gap"; the raw boot came in the next commit | `git show d621ae8 -- scripts/boot-smoke-test.sh`; `git show 7bb6141 -- scripts/boot-smoke-test.sh` |
| 22 | Round 1 item 3 (panic after banner passed), round 2 item 1 (exception loop passed); vector table and `unhandled_exception_entry` (version in `7bb6141`; current code uses `x4` and `x0` holds the entry index) | `gh pr view 3`; `git show 7bb6141:kernel/src/arch/aarch64/exceptions.s`; `exceptions.s` on `main` |
| 23 | Mutations caught: panic after banner, undefined instruction after banner, `.bss` loop that does not clear, secondary cores not parked | "Test plan" in `gh pr view 3` |
| 24 | Round 1 item 7 (cores not parked), round 2 item 6 (parking never exercised), round 2 item 7 (MPIDR affinity, not changed); the `_start` park snippet at `d621ae8`; the console `SAFETY` comment relies on parking | `gh pr view 3`; `git show d621ae8:kernel/src/arch/aarch64/boot.s`; `git show d621ae8 -- kernel/src/board.rs` |
| 25 | No observation of garbled serial output is recorded | Absence: searched the repository and PRs 2, 3, 7 (nothing found); author to confirm |
| 26 | Round 2 item 8: `memory_image.rs` comment false about `.data` | `gh pr view 3` |
| 27 | K1 self-test mutations x19 and `SPSR` missed on first run, fixed by scrubbing live state; full mutation list | `gh pr view 7` ("Mutation tests"); `git show 77518d3`; scrub code in `exceptions.s` |
| 28 | `brk #0x1` emitted by the compiler for `abort` and `unreachable`; resume restricted to `#0x800`; gate for the compiler trap | `exceptions.rs` (`RESUMABLE_BREAKPOINT`); `git show ac3f01b`; `git show df80a56` |
| 29 | Stack reset test uses an invalid `SP_EL1` | `git show e944dbe -- kernel/src/arch/aarch64/exceptions.rs`; `exceptions.rs` line 236 on `main` |
| 30 | K3 round findings (blob unreserved passes; reservations dropped; hostile blob stalls boot; node without `reg`; `__image_start`; exclusive loop) | `gh pr view 13` |
| 31 | Exclusive loop comment ("may never succeed on real cores while the MMU is off and memory is Device"); `swap` replaced by load and store; no hardware run recorded | `git show 0004e75`; `gh pr view 13`. Not checked against the Arm Architecture Reference Manual |
| 32 | K2b-2 round 1 fixes: corrupt copy reported, overflows, `SPSel` and interrupt masking before leaving boot services | `gh pr view 15`; `git show 51ee06e` |
| 33 | Round 5: EL2 handoff above 4 GiB with a wrong `IPS` hung silently; `uefi-el2-high` boot | `gh pr view 15`; `git show efff5f4`; `scripts/boot-smoke-test.sh` |
| 34 | VirtualBox firmware reserves `0x40200000` at 1024 MiB; VM needs 2048 MiB; ACPI-only publication; PL011 at `0xffdde000`; RAM at `0x08000000` | ADR 0013 "Findings of K2b-2"; `docs/k2b-bootloader-research.md`; `gh pr view 12` and `15` |
| 35 | Boot output of `main` at `50d808e`, direct and UEFI, and 17 `PASS` lines from `scripts/boot-smoke-test.sh` | **Re-run:** appendix; direct command as in `.cargo/config.toml` |
| 36 | Hook did not exist in a new worktree; untracked file, merge commits, stash message; the tracked-hooks fix | `gh pr view 3` (round 1 item 1, round 2 items 2 to 4); `.githooks/pre-commit`; `git show 7bb6141 -- .githooks` |
| 37 | Rebuttals: hooks on rebase and cherry-pick, `arch` importing `board`, no IRQ/FIQ test, no release boot | `gh pr view 7` |
| 38 | Known limit: invalid `SP_EL1` loops silently until guard pages (K4) | `gh pr view 7` ("Known limit"); header of `exceptions.s` |
| 39 | Pull request #2 was the first boot, merged before the review process; the rule of boot output in PR descriptions is from PR #11 | `gh pr view 2`; `gh pr view 11`; `CLAUDE.md` "Showing that it runs" |
| 40 | `docs/prior-art.md` does not cover code-review practice; no claim is made that the approach beats anything | `docs/prior-art.md`, sections 1 to 4 and "Do not claim in articles until measured or checked" |
| 41 | No script in `scripts/` automates the mutations; they were reported by the author | Absence: `ls scripts`; `rg -i mutation` finds the word only in two intent-shell docs |
