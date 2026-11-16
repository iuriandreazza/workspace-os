---
title: "Roadmap"
type: roadmap
status: active
created: 2026-11-15
updated: 2026-11-15
tags: [roadmap, planning, kernel, slm, drivers]
---

# Roadmap

Each item below is one development session: one branch, one pull request.
The decisions behind this plan are in `docs/adr/`; the rules for every session
are in `CLAUDE.md`.

## How to run a session

1. Start a session in this repository and name the item, for example:
   "Do session K1 from `docs/roadmap.md`."
2. Read `CLAUDE.md`, the ADRs the item lists, and the item itself.
3. Create a branch from `main`. If the item says "ADR first", write and agree
   the ADR before any code.
4. Implement it together with the gate addition the item names. A session is
   not done until `./scripts/check.sh` fails when the new behaviour is broken.
5. Run the local adversarial review until a round has no confirmed finding.
6. Open the pull request and record the review outcome in it. For a change
   to the kernel or the bootloader, also paste the serial output of a real
   boot (`CLAUDE.md`, "Showing that it runs"). The user merges.
7. Mark the item done here and update the notes that describe the files touched.

Status values: `done`, `next` (ready to start), `blocked` (waiting on the items
listed), `later` (not designed yet).

## Tracks and order

| Track | What | Can start |
|---|---|---|
| K | Kernel and bootloader | now, one item at a time |
| B | Boot model | after K2c |
| S | Intent shell with a local SLM | now, in parallel with K, on the host |
| R | Research and articles | now, in parallel |
| P | Policy plane (purpose-trained models) | after K7 |
| D | Drivers and packages | after K10 |
| C | Compatibility with other systems | after K8 |

K is sequential: each item builds on the previous one. S and R do not touch
kernel files, so they can run in separate sessions at the same time as K.

## Track K: kernel

| Item | Session | Gate addition | Status |
|---|---|---|---|
| K0 | Boot on QEMU `virt`, serial banner, parked cores, vector table that panics, check script, hooks, CI | Three boots: ELF, raw image over dirtied `.bss`, four cores | done |
| K1 | Exception handling: save and restore context, decode the syndrome, report synchronous exceptions, enter EL1 when started at EL2 or EL3. Its adversarial review also covers what reached `main` without a clean round: the vector table, the raw and four-core boots, and the git hooks | A boot that takes a deliberate breakpoint and resumes; boots at EL2 and EL3 | done |
| K2 | Device tree: keep the pointer from the boot protocol or find the blob at the start of RAM, read memory size and the UART node, stop hard-coding them in `board.rs` | Boots with two different memory sizes and prints each. `boot.s` overwrites `x0`, the pointer from the boot protocol: save it first if it is wanted | done |
| K2b-1 | Own UEFI bootloader, part 1a (ADR 0008): research and the ADR for the handoff structure between bootloader and kernel. Result: `docs/k2b-bootloader-research.md` and ADR 0013 | Research with sources; ADR accepted | done |
| K2b-2 | Own UEFI bootloader, part 1b: a Rust UEFI application that loads the kernel from the EFI system partition, leaves boot services and jumps to it with the structure of ADR 0013, plus a bootable disk image. First, find out which tables (device tree, ACPI) each firmware publishes and whether VirtualBox's Arm serial port works (ADR 0013, "What is not yet known"). Edits `boot.s` and `board.rs`, which K3 also touches | The banner under QEMU with UEFI firmware, in the gate; the banner under VirtualBox on the ARM Mac, recorded by hand. Result: `bootloader/`, `bootinfo/`, `scripts/build-disk-image.sh`, `docs/booting-the-image.md` | done |
| K2c | Own UEFI bootloader, part 2: the boot menu, with the normal entry and the recovery entry (ADR 0007) | Each menu entry reaches the kernel with a different boot mode, checked in the gate | next |
| K2d | Loading other kernels, mechanism only (ADR 0008): Linux through its EFI entry with a command line and an initial ramdisk, and other systems by starting their own EFI loader. Entries written by hand for now | A Linux kernel started by our bootloader under QEMU prints its first lines | blocked: K2c |
| K2e | Graphical boot menu: draw on the firmware framebuffer, with a background image, a font and a theme the installer can set. The text menu over the serial console stays, because the gate reads it | To be designed | later |
| K3 | Physical frame allocator fed by the device tree memory map | Allocate, free and reuse frames at boot; exhausting memory is reported, not a hang | done |
| K4 | MMU: kernel page tables, higher-half mapping, write-xor-execute | A write to kernel code and a jump into data each end in a reported fault | next |
| K5 | Kernel heap allocator | Boot-time allocation stress test | blocked: K4 |
| K6 | Interrupts: interrupt controller and generic timer | A boot that counts timer ticks | blocked: K5 |
| K7 | Kernel threads, context switch, round-robin scheduler. Mechanism and policy kept apart (ADR 0002) | Two threads interleave output in a checked order | blocked: K6 |
| K8 | Userspace: EL0, one address space per process, system call entry, first user program | A user program prints through a system call; an illegal access kills only that program | blocked: K7 |
| K9 | IPC and capabilities. ADR first: message model, capability representation, what the kernel checks, and how a server ties a capability to the user it was issued for (ADR 0007) | Two user programs exchange a message; a call without the capability is refused | blocked: K8 |
| K10 | Move the UART driver out of the kernel into a userspace server | The banner is printed by the server; killing the server does not stop the kernel | blocked: K9 |
| K11 | Multi-core bring-up through PSCI | Four cores run threads | later |
| K12 | Filesystem server with an in-memory filesystem | A user program creates, reads and deletes a file through the server | blocked: K10 |
| K13 | Users and permissions in the Linux model (ADR 0007): users, groups, owner and permission bits enforced by the filesystem server, a root user with no direct login, `sudo`-style elevation for members of an administrators group, and a recovery mode from the boot menu that runs without the AI | A user is refused a file they have no permission for; an administrator gets it after confirming with their own password; a user outside the group cannot elevate; the kernel holds no notion of a user | blocked: K12 |
| K14 | x86_64 port (ADR 0004): bootloader for x86_64 UEFI (ADR 0008), boot, serial console, exceptions, memory management and context switch behind `kernel/src/arch`, with the same gate running on both architectures | Every boot in the gate passes on aarch64 and on x86_64 | blocked: K10 |
| K15 | x86_64 under VirtualBox and QEMU on a PC | The banner in a VirtualBox x86_64 guest, recorded by hand | blocked: K14 |

K10 is the point where the system is a microkernel in practice (ADR 0001).

The x86_64 port waits for K10 on purpose: porting earlier would make every
item up to K10 cost two implementations. After K14 every kernel item is done
on both architectures. On the ARM development machine x86_64 runs emulated,
so its gate is slower.

## Track B: boot model

A small model in the bootloader that detects kernels and decides how to load
them (ADR 0009). It builds on the bootloader items of track K.

| Item | Session | Done when | Status |
|---|---|---|---|
| B1 | Research, then ADR: language model or purpose-trained classifier and its size; the inputs it reads and the typed decision it returns; how inference runs in the UEFI environment; licences | ADR accepted, with a measured estimate of inference time in UEFI | blocked: K2c |
| B2 | Corpus and training, offline: disk layouts and kernel images from several systems, the training run, and an evaluation harness on the host | Accuracy on a held-out part of the corpus, recorded in `docs/` | blocked: B1 |
| B3 | Inference in the bootloader: the model decides, decisions are stored with a fingerprint of the disk, the last successful decisions are the fallback, and the loader checks the format before jumping | With the model: Linux and our kernel are found on a disk with no hand-written entries. Without the model: our kernel still boots. A deliberately wrong decision ends in an error on the menu | blocked: B2, K2d |
| B4 | Integrity of the model's weights at boot | Replaced weights are refused | blocked: B3 |

## Track S: intent shell with a local SLM

The model is Bonsai 8B from PrismML (see "Model notes"). This track starts on
the development machine, outside the kernel: the kernel cannot host a model
runtime before it has userspace, memory management and a filesystem.

| Item | Session | Done when | Status |
|---|---|---|---|
| S1 | Run Bonsai 8B locally with `llama.cpp` (`llama-server`). Measure tokens per second, time to first token and memory at 8k and 32k context. Record one tool call end to end | Numbers and the tool-call transcript recorded in `docs/s1-bonsai-8b-local-run.md`, with the exact commands | done |
| S2 | Action plan format. ADR first: the structured actions the model may emit (including proposing a SOUL change, ADR 0006), the capability each one needs, which ones need root and so a per-action confirmation (ADR 0007), and how output is constrained to the format | ADR accepted; a schema file exists | done |
| S3 | Shell prototype on the host: natural language to plan, deterministic capability broker that validates it against the user's permissions and the action's capability, never more than both allow (ADR 0007), dry-run and confirmation, a log of every model decision (ADR 0002) | A session can turn "list the largest files here" into a validated plan and run it. Report: `docs/s3-intent-shell-prototype.md` | done |
| S3b | Terminal screen and path-free resources. ADR first, superseding the path-based parts of ADR 0011: the person never navigates the filesystem and does not see userspace or system space. The model names things by description or query, a deterministic resolver turns them into concrete resources before the broker, and the broker still checks real resources. The default view shows names; the real paths sit behind a details view that stays mandatory before any root confirmation (ADR 0007). Then the screen: header with user, model and log, a per-step table with the verdict and capability of each action, notes on incomplete results, a summary line, a separate panel for each root action | The same intents as S4 give the same validated actions with no path in the default view; a root confirmation still shows the exact action; the ADR is accepted. Report: `docs/s3b-path-free-shell.md` | done |
| S3c | Conversational terminal interface with `ratatui`, in the style of a chat session: input line at the bottom, progress while the model generates, the plan as a growing block, confirmations and the choice of an ambiguous reference by selection, a scrollable history. ADR first: the library and its licence (ADR 0005), what stays text-only so the deterministic shell (S5) and the recovery mode (ADR 0007) work without it, and how the screens of ADR 0014 map onto it. Only the terminal adapter changes: `screen.rs`, the resolver and the broker stay as they are | The same plans, choices and root confirmations as S3b, driven through the new interface; the screen content is still tested as text; the ADR is accepted. Report: `docs/s3c-conversational-interface.md` | done |
| S3d | Colour and header of the interface, and `/commands`. Colour: a small palette for verdicts (ok, held, refused, root), the tree marks and the reply, with a no-colour fallback for `NO_COLOR` and `--plain`. Header (the ratatui interface, plain text in `--plain`): the model's name (read from the server, not `host:port`), the machine name (ADR 0006), the kernel version, battery percentage when the host has one, CPU and memory usage as small bars refreshed every few seconds. `/command` input in the interface (for example `/help`, `/clear`, `/model`, `/tree`, and later `/soul` for S9 and S10), parsed by code and never sent to the model; unknown commands say so. ADR first: what the header reads on each host, refresh cost, and which `/commands` exist | Header and palette snapshot-tested with `TestBackend`; a `/command` never reaches the planner; no battery on a desktop shows no battery; colour off with `NO_COLOR` | later |
| S4 | Evaluation set: intents with expected plans. Measure Bonsai 8B, and compare with at least one alternative that has native tool calling | Accuracy table in `docs/`; decision to keep or change the model | next |
| S5 | Deterministic fallback shell: the same actions typed as commands, with no model loaded | Every action in the format is reachable without the model | next |
| S6 | Bridge into the OS: the shell runs on the host and talks to the kernel over a virtual serial channel | A plan produced on the host is executed by a user program in the OS | blocked: S5, K10 |
| S7 | Run inference inside the OS | To be designed | later |
| S8 | SOUL format, presets and per-user personality (ADR 0006): what the single SOUL file holds, the four ready-made profiles, how a user's personality (tone, voice, language, level of detail, name) layers on the SOUL without contradicting it, and the shell loading both. The broker reads neither | The same evaluation intents produce the same validated actions under every preset and personality; a personality that contradicts the SOUL is rejected | blocked: S4 |
| S8a | Voice of the answers (ADR 0018): after each outcome the model writes a short reply in the tone of the SOUL, which in this item is one of the four presets read from a file; the table, verdicts, tree and root panel stay deterministic text | A scripted pipeline gives identical actions, verdicts and decision log under all four presets, with the reply on and off; no model, an error or a timeout shows the output as it is today; a live test with a real reply | blocked: ADR 0018 accepted |
| S9 | Guided interview that builds a SOUL from the user's answers; preset path with no model loaded | A SOUL produced by the interview loads in the shell; the preset path works with the model off | blocked: S8 |
| S10 | SOUL evolution: the AI proposes a change as an action in the plan format; it applies only after a user with root authority approves the full diff; history and undo | A proposed change is refused without approval and when approved by a user without root authority, applied with a root approval, and undone from the history | blocked: S8 |
| S11 | Speech on the host: local speech synthesis and recognition. The spoken voice belongs to each user's personality (ADR 0006), so two users can hear different voices. Research the models and their licences first | A spoken request becomes a validated plan and the answer is spoken back, with no network; two users get two different voices | blocked: S3, S8 |
| S11b | Voice confirmation of root actions (ADR 0007): voiceprint enrolment, a random challenge phrase, and the person's password as second factor; typed confirmation always available. Research the speaker-verification models, their error rates and licences first | A root action is refused for the wrong speaker, for a replayed recording, for audio the system itself is playing, and without the second factor; it passes with all three | blocked: S11, S10 |
| S12 | First-boot flow inside the OS that runs the interview | To be designed; needs an installer, userspace, storage and audio drivers | later |

Host-side code must not be built for the bare-metal target. It lives in the
`intent-shell` crate, built with `--target "$host_target"` like `devicetree`
and left out of the workspace's `default-members` (S3,
`docs/s3-intent-shell-prototype.md`).

## Track R: research and articles

| Item | Session | Done when | Status |
|---|---|---|---|
| R1 | Prior-art survey with verified references: learned policies in kernels, LLM-as-OS proposals, reuse of Linux drivers on other kernels, models in boot. Result: `docs/prior-art.md` | An annotated list where every claim has a source that was actually read | done |
| R2 | Article 1: booting a Rust microkernel, and what two rounds of adversarial review found in 60 lines of boot code | Draft reviewed by the user | next |
| R3 | Article 2: models as a control plane over a deterministic core | Draft reviewed by the user | blocked: S4 |

## Track P: policy plane

| Item | Session | Status |
|---|---|---|
| P1 | ADR: telemetry the kernel exports and the bounded interface a policy may use | blocked: K7 |
| P2 | Trace collection under QEMU for scheduling decisions | blocked: P1 |
| P3 | First purpose-trained model, trained offline on the traces, for one scheduler decision; fixed heuristic as fallback with a watchdog | blocked: P2 |
| P4 | Comparison against the heuristic on recorded workloads | blocked: P3 |

## Track D: drivers and packages

| Item | Session | Status |
|---|---|---|
| D1 | ADR: native driver API and driver server skeleton | blocked: K10 |
| D2 | Hand-written reference drivers for the generic classes: virtio block and virtio network | blocked: D1 |
| D3 | Conversion pipeline prototype, offline: one simple Linux driver through model translation, build and differential test in QEMU (ADR 0003) | blocked: D2 |
| D3b | Evaluate a discriminative model (Jev) for the decisions around the translation, on the registry side only: triaging which path a driver takes (translate, shim or virtual machine), and answering a checklist of typed questions about a converted driver before publication. It does not write the driver; the translation in D3 still needs a generative model. Matching hardware to its Linux driver needs no model: Linux drivers declare the identifiers they support | blocked: D3; only worth it once many drivers are converted |
| D4 | Package format and registry: signature, provenance, trust level; local conversions marked unverified | blocked: D3 |
| D5 | Registry-side conversion service | later |
| D6 | Driver hot swap from generic to converted | blocked: D4 |

## Track C: compatibility

| Item | Session | Status |
|---|---|---|
| C1 | Linux personality: load a static aarch64 Linux "hello world" and serve the system calls it makes | blocked: K8 |
| C2 | Wider system call coverage, driven by a static BusyBox | blocked: C1, K13 |
| C3 | Lightweight virtual machine as fallback | later |
| C4 | Binary translation, to run programs built for the other architecture | blocked: K14 |

## Model notes: Bonsai 8B

Facts as published by PrismML and checked on 2026-11-15; the benchmark figures
are the vendor's own.

- 1-bit weights, 8.2 billion parameters, about 1.15 GB as a GGUF file.
- Runs on upstream `llama.cpp` (CPU, Metal, CUDA, Vulkan). MLX on Apple
  Silicon needs PrismML's fork until their change is merged.
- Context window of 65,536 tokens. Published memory estimates including the
  context cache: about 2.5 GB at 8k tokens, 5.9 GB at 32k, 10.5 GB at 64k.
- Weights under Apache-2.0. They are data shipped next to the OS, not code
  linked into it; `llama.cpp` is MIT. Confirm with the licence review that
  ADR 0005 already asks for.
- Tool calling: the project owner reports that Bonsai 8B does it. The pages
  read on 2026-11-15 did not state it, so S1 records a working tool call.
  PrismML's newer Bonsai 2 family advertises native tool calling; S4 compares
  against it.

Sources: [PrismML announcement](https://prismml.com/news/bonsai-8b),
[model card](https://huggingface.co/prism-ml/Bonsai-8B-gguf),
[PrismML docs](https://docs.prismml.com/models/bonsai-8b.md),
[run guide](https://github.com/PrismML-Eng/Bonsai-demo/blob/main/Bonsai1_README.md).

## Model notes: Jev

From the Wikipedia article read on 2026-11-15; performance figures are the
vendor's own and nothing here was tested.

- Discriminative model from TypeSafe AI, in limited early access since
  September 2026. It answers typed questions (classification, regression)
  with a probability estimate and does not generate text or code.
- Proprietary, not open weights. The article does not say whether it can run
  locally; it describes an API with a limit of 64k tokens per input.
- Questions in one request are answered independently; a question that
  depends on another answer needs a second request.

What follows for this project:

- It fits the rule "the model proposes, deterministic code decides"
  (ADR 0002): a typed answer with a probability can be gated by a threshold.
- It cannot convert a driver, because conversion is code generation.
- Being proprietary and remote, it can sit in the registry-side conversion
  service (ADR 0003) but not in the OS image, which must work offline and
  without any model.

Source: [Jev (AI model)](https://en.wikipedia.org/wiki/Jev_(AI_model)).

## Open items outside the sessions

- Legal review of the licence boundary for converted drivers and model
  weights (ADR 0005), and of the reading of MPL-2.0 that lets `ucs2` be linked
  into the bootloader (ADR 0017).
- Disk encryption is optional at installation (ADR 0007). It has no roadmap
  item yet: it needs a block device driver and the installer first.
- Physical hardware: not a target yet; the system runs in QEMU and
  VirtualBox (ADR 0004).
