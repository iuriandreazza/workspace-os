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
6. Open the pull request and record the review outcome in it. The user merges.
7. Mark the item done here and update the notes that describe the files touched.

Status values: `done`, `next` (ready to start), `blocked` (waiting on the items
listed), `later` (not designed yet).

## Tracks and order

| Track | What | Can start |
|---|---|---|
| K | Kernel | now, one item at a time |
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
| K1 | Exception handling: save and restore context, decode the syndrome, report synchronous exceptions, enter EL1 when started at EL2 or EL3 | A boot that takes a deliberate breakpoint and resumes; boots at EL2 and EL3 | next |
| K2 | Device tree: keep the pointer from the boot protocol or find the blob at the start of RAM, read memory size and the UART node, stop hard-coding them in `board.rs` | Boots with two different memory sizes and prints each | blocked: K1 |
| K3 | Physical frame allocator fed by the device tree memory map | Allocate, free and reuse frames at boot; exhausting memory is reported, not a hang | blocked: K2 |
| K4 | MMU: kernel page tables, higher-half mapping, write-xor-execute | A write to kernel code and a jump into data each end in a reported fault | blocked: K3 |
| K5 | Kernel heap allocator | Boot-time allocation stress test | blocked: K4 |
| K6 | Interrupts: interrupt controller and generic timer | A boot that counts timer ticks | blocked: K5 |
| K7 | Kernel threads, context switch, round-robin scheduler. Mechanism and policy kept apart (ADR 0002) | Two threads interleave output in a checked order | blocked: K6 |
| K8 | Userspace: EL0, one address space per process, system call entry, first user program | A user program prints through a system call; an illegal access kills only that program | blocked: K7 |
| K9 | IPC and capabilities. ADR first: message model, capability representation, what the kernel checks | Two user programs exchange a message; a call without the capability is refused | blocked: K8 |
| K10 | Move the UART driver out of the kernel into a userspace server | The banner is printed by the server; killing the server does not stop the kernel | blocked: K9 |
| K11 | Multi-core bring-up through PSCI | Four cores run threads | later |

K10 is the point where the system is a microkernel in practice (ADR 0001).

## Track S: intent shell with a local SLM

The model is Bonsai 8B from PrismML (see "Model notes"). This track starts on
the development machine, outside the kernel: the kernel cannot host a model
runtime before it has userspace, memory management and a filesystem.

| Item | Session | Done when | Status |
|---|---|---|---|
| S1 | Run Bonsai 8B locally with `llama.cpp` (`llama-server`). Measure tokens per second, time to first token and memory at 8k and 32k context. Record one tool call end to end | Numbers and the tool-call transcript recorded in `docs/`, with the exact commands | next |
| S2 | Action plan format. ADR first: the structured actions the model may emit, the capability each one needs, and how output is constrained to the format | ADR accepted; a schema file exists | blocked: S1 |
| S3 | Shell prototype on the host: natural language to plan, deterministic capability broker that validates it, dry-run and confirmation, a log of every model decision (ADR 0002) | A session can turn "list the largest files here" into a validated plan and run it | blocked: S2 |
| S4 | Evaluation set: intents with expected plans. Measure Bonsai 8B, and compare with at least one alternative that has native tool calling | Accuracy table in `docs/`; decision to keep or change the model | blocked: S3 |
| S5 | Deterministic fallback shell: the same actions typed as commands, with no model loaded | Every action in the format is reachable without the model | blocked: S3 |
| S6 | Bridge into the OS: the shell runs on the host and talks to the kernel over a virtual serial channel | A plan produced on the host is executed by a user program in the OS | blocked: S5, K10 |
| S7 | Run inference inside the OS | To be designed | later |

Host-side code must not be built for the bare-metal target. Decide in S3
where it lives: `.cargo/config.toml` currently sets the kernel target for the
whole workspace.

## Track R: research and articles

| Item | Session | Done when | Status |
|---|---|---|---|
| R1 | Prior-art survey with verified references: learned policies in kernels, LLM-as-OS proposals, reuse of Linux drivers on other kernels | An annotated list where every claim has a source that was actually read | next |
| R2 | Article 1: booting a Rust microkernel, and what two rounds of adversarial review found in 60 lines of boot code | Draft reviewed by the user | blocked: R1 |
| R3 | Article 2: models as a control plane over a deterministic core | Draft reviewed by the user | blocked: R1, S4 |

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
| D4 | Package format and registry: signature, provenance, trust level; local conversions marked unverified | blocked: D3 |
| D5 | Registry-side conversion service | later |
| D6 | Driver hot swap from generic to converted | blocked: D4 |

## Track C: compatibility

| Item | Session | Status |
|---|---|---|
| C1 | Linux personality: load a static aarch64 Linux "hello world" and serve the system calls it makes | blocked: K8 |
| C2 | Wider system call coverage, driven by a static BusyBox | blocked: C1, a filesystem server |
| C3 | Lightweight virtual machine as fallback | later |
| C4 | x86_64 port, then binary translation | later |

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

## Open items outside the sessions

- Legal review of the licence boundary for converted drivers and model
  weights (ADR 0005).
- The hardware the OS should run on first after QEMU.
