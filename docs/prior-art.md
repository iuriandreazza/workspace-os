---
title: "Prior-art survey"
type: research
status: accepted
created: 2026-11-15
updated: 2026-11-15
tags: [research, prior-art, policy, llm, drivers, boot]
---

# Prior-art survey

Roadmap item R1. It records what others have built in the four areas the
project's ADRs touch, so that articles can say what is new and what is not.

## Method and limits

- Every entry in sections 1 to 4 was opened and read on 2026-11-15. The link
  is the page that was read.
- "Read" means different things, and each entry says which:
  - **abstract**: the arXiv, USENIX, ACM or publisher page with the abstract
    and metadata. The paper body was not read, so the results quoted are the
    authors' own headline claims and the experimental conditions are unknown.
  - **docs**: the project's own documentation page.
  - **paper**: the first page of the paper text itself (only for one entry).
- The pages were fetched through a tool that summarises them with a small
  model. Numbers and names below were taken from those summaries, not from a
  verbatim reading, so check a figure against the linked page before quoting
  it in an article.
- Search engines were used to locate sources only. Nothing found only in a
  search result is stated as fact; those items are in the unverified list.
- Not finding something is weak evidence. "No precedent found" below means a
  few searches returned nothing, not that nothing exists.

## 1. Learned and replaceable policies inside kernels

### sched_ext (Linux kernel documentation) — docs

[docs.kernel.org/scheduler/sched-ext.html](https://docs.kernel.org/scheduler/sched-ext.html)

- **Did:** a Linux scheduling class whose behaviour is defined by BPF
  programs implementing `struct sched_ext_ops`. It can be switched on and off
  at any time; with no BPF scheduler loaded, `SCHED_EXT` tasks behave as
  `SCHED_NORMAL`.
- **Safety:** on an internal error or a stalled runnable task, or when the
  scheduler program exits, the BPF scheduler is aborted and tasks return to
  the fair scheduler. A bypass mode with fixed time slices covers load,
  unload and error recovery.
- **Measured:** nothing; it is documentation.
- **Differs from this project:** it is the closest existing mechanism to
  ADR 0002's "swap the policy without touching the mechanism, with a fixed
  fallback". It lives inside Linux, with the BPF verifier as the bounding
  device. Here the bound would be the kernel's own policy interface (roadmap
  P1) in a microkernel with no BPF.

### ghOSt (SOSP 2021) — abstract

[research.google](https://research.google/pubs/ghost-fast-and-flexible-user-space-delegation-of-linux-scheduling/)
(Humphries et al., Google and Stanford)

- **Did:** delegates Linux scheduling decisions to user-space code. Policies
  can be written in any language and changed without rebooting the host;
  per-CPU and centralised models, run-to-completion and preemptive.
- **Measured (as claimed):** throughput and latency comparable to the
  kernel's default scheduler, on academic workloads and on Google Snap and
  Google Search. No figures were on the page read.
- **Differs:** a policy mechanism, not a learned policy: it makes any policy
  pluggable and says nothing about models. It supports the idea that policy
  can live outside the kernel at acceptable cost; this project would do the
  same through its own interface.

### LinnOS (OSDI 2020) — abstract

[usenix.org](https://www.usenix.org/conference/osdi20/presentation/hao)
(Hao et al., University of Chicago)

- **Did:** a light neural network inside the OS that predicts per-I/O SSD
  latency, so storage applications can avoid slow I/Os.
- **Measured (as claimed):** average I/O latency reduced by 9.6% to 79.6%
  against hedging and heuristics; inference accuracy 87% to 97%; inference
  overhead 4 to 6 microseconds per I/O.
- **Differs:** a purpose-trained model deciding a kernel I/O policy, which
  is the P track's idea (ADR 0002). It is the evidence that a small model
  can sit within a microsecond budget. It runs in a monolithic kernel with no
  stated bound or watchdog on its output on the page read.

### LAKE (ASPLOS 2023) — abstract

[ldos.utexas.edu/node/63](https://ldos.utexas.edu/node/63)
(Fingler et al.; the page lists one author, see the unverified note below)

- **Did:** a system that supports ML in kernel space and exposes accelerators
  such as GPUs there, with APIs for feature collection across layers and for
  handling when acceleration pays off. The authors propose replacing
  hand-tuned heuristics with ML in five kernel subsystems.
- **Measured (as claimed):** inference time of an ML I/O-latency predictor
  falls by up to 96% with acceleration; baseline and hardware were not on the
  page.
- **Differs:** it addresses the cost of running models in a kernel. This
  project keeps models outside the trusted core and asynchronous or batched
  (ADR 0002), a different answer to the same cost problem.

### SchedCP / sched-agent (arXiv 2025) — abstract

[arxiv.org/abs/2509.01245](https://arxiv.org/abs/2509.01245)
(Zheng, Hu, Zhang, Quinn; MLforSystem 2025)

- **Did:** SchedCP, an MCP server with a workload-analysis engine, a
  scheduler-policy repository and an execution verifier (static and dynamic
  analysis before deployment); and sched-agent, a multi-agent system that
  analyses workloads, writes eBPF scheduling policies and deploys them through
  sched_ext.
- **Measured (as claimed):** up to 1.79x performance improvement and 13x cost
  reduction against naive agentic approaches.
- **Differs:** the nearest published work to "a model operating a kernel
  policy". It uses a language model that writes policy code, deployed through
  a verified, replaceable interface on Linux. This project's P track instead
  trains a small model on kernel telemetry to tune parameters inside limits
  the kernel enforces, with a fixed heuristic and a watchdog as fallback.

## 2. LLM as operating system, and natural language to system actions

### AIOS: LLM Agent Operating System (COLM 2025) — abstract

[arxiv.org/abs/2403.16971](https://arxiv.org/abs/2403.16971)
(Mei et al., Rutgers)

- **Did:** a kernel for LLM-based agents that isolates LLM resources and
  services from agent applications and provides scheduling, context, memory,
  storage and access control, with an SDK.
- **Measured (as claimed):** execution up to 2.1x faster when serving agents
  built with different frameworks. Benchmark and baseline not on the page.
- **Differs:** the "OS" schedules agents and LLM calls on top of an existing
  operating system. It is not a kernel for hardware, and the model does not
  manage the machine's resources. In this project the kernel is
  deterministic and the model is a replaceable layer above and beside it.

### MemGPT: Towards LLMs as Operating Systems (2023) — abstract

[arxiv.org/abs/2310.08560](https://arxiv.org/abs/2310.08560)
(Packer et al., UC Berkeley)

- **Did:** virtual context management, borrowing the idea of hierarchical
  memory from operating systems to give a model the effect of a much larger
  context, with interrupts to manage control flow.
- **Measured (as claimed):** document analysis beyond the context window and
  multi-session conversational agents. No figures on the page read.
- **Differs:** the OS metaphor is applied to the model's own memory. Nothing
  here concerns operating a computer.

### OS-Copilot / FRIDAY (2024) — abstract

[arxiv.org/abs/2402.07456](https://arxiv.org/abs/2402.07456)
(Wu et al.)

- **Did:** a framework for generalist agents that operate web, code
  terminals, files, multimedia and third-party applications, and FRIDAY, an
  agent that accumulates skills across tasks.
- **Measured (as claimed):** on GAIA, FRIDAY beats previous methods by 35%;
  it also learns to control Excel and PowerPoint. Absolute scores not on the
  page.
- **Differs:** an agent with free-form code execution on a conventional OS.
  The project's shell emits typed actions that a capability broker checks
  against the user's permissions (ADR 0002, 0007); the model never executes
  anything itself.

### NL2Bash (2018) — abstract

[arxiv.org/abs/1802.08979](https://arxiv.org/abs/1802.08979)
(Lin, Wang, Zettlemoyer, Ernst)

- **Did:** a corpus of Bash commands with expert-written English
  descriptions, and semantic-parsing baselines to map English to Bash.
- **Measured:** baseline performance on that task; no figures on the page
  read.
- **Differs:** a pre-LLM dataset for natural language to a shell language.
  The project maps to a closed set of typed actions rather than to arbitrary
  shell text, and validates them before running.

## 3. Reusing Linux (and other) drivers in other kernels

### LeVasseur et al., OSDI 2004 — abstract

[usenix.org](https://www.usenix.org/conference/osdi-04/unmodified-device-driver-reuse-and-improved-system-dependability-virtual-machines)

- **Did:** runs each unmodified driver together with its original OS inside
  its own virtual machine. A prototype ran unmodified Linux network, disk and
  PCI drivers this way. Separate VMs isolate faulty or malicious drivers.
- **Measured (as claimed):** network performance within 3% to 8% of native
  Linux; about 0.12% extra CPU per additional VM.
- **Differs:** this is the "virtual machine" path that roadmap C3 and ADR
  0003 keep as a fallback. No model and no translation of the driver: the
  cost is running a whole guest OS for it.

### Genode DDE and the porting guide — docs

[genode.org porting guide](https://genode.org/documentation/developer-resources/porting_device_drivers)

- **Did:** runs Linux driver code in user land so that it believes it is in
  the Linux kernel, in three parts: back end, an environment emulating the
  Linux functions the driver uses, and a front end exposing a session
  interface (NIC, block). The guide says the older generic `dde_linux26`
  environment grew hard to maintain and was abandoned; the current approach is
  a small environment tailored to each driver, usually a few thousand lines.
  The page itself warns it is partly out of date.
- **Measured:** nothing.
- **Differs:** it is a hand-built compatibility layer, and its own
  maintenance history is the main evidence for the risk of a generic layer
  that tracks Linux internals. ADR 0003 avoids the generic layer by
  translating each driver once to a native API, with a model doing the
  translation work that Genode's developers do by hand.

### Rump kernels: Rump File Systems (USENIX 2009) — paper (first page)

[usenix.org/legacy](https://www.usenix.org/legacy/event/usenix09/tech/full_papers/kantee/kantee.pdf)
(Kantee)

- **Did:** runs unmodified NetBSD kernel file system code in userspace
  through a shim layer that emulates enough of the kernel, as a userspace
  server or as a library. A prototype for Linux showed Linux's jffs2 mounted
  as a userspace server on NetBSD.
- **Measured (as claimed):** ten disk-based file systems and others
  functional; performance for an estimated typical workload within ±5% of
  kernel performance.
- **Differs:** reuse across kernels with an emulation shim, for file systems
  rather than hardware drivers. It is a precedent for running foreign kernel
  code as isolated userspace servers, which is where this project runs
  drivers (ADR 0001).

### LinuxKPI (FreeBSD manual page) — docs

[man.freebsd.org](https://man.freebsd.org/cgi/man.cgi?query=linuxkpi&sektion=4)

- **Did:** a FreeBSD kernel module providing a limited subset of the Linux
  kernel programming interface so Linux drivers and kernel code compile on
  FreeBSD with little or no modification. The page lists InfiniBand,
  `drm-kmod` graphics and wireless drivers as users.
- **Measured:** nothing.
- **Differs:** an in-kernel compatibility layer in a monolithic kernel, so a
  driver fault is a kernel fault. It tracks Linux's API surface by hand.

### Linux Kernel Library, LKL (project README) — docs

[github.com/lkl/linux](https://github.com/lkl/linux)

- **Did:** compiles the Linux kernel into an object that applications link,
  as an architecture port (`arch/lkl`); the API follows the Linux system call
  interface. Stated uses: user-space programs that read or write Linux file
  systems or use the Linux network stack, kernel drivers for other operating
  systems that read Linux file systems, and bootloader support for Linux file
  systems. Supported hosts per the README: POSIX and Windows user-space
  applications.
- **Measured:** nothing.
- **Differs:** reuses Linux subsystems wholesale instead of translating
  them. Of interest for the C track (reading Linux file systems) rather than
  for hardware drivers.

### LLM-assisted code migration (nearest work found) — abstract

- [LLMigrate, arXiv 2503.23791](https://arxiv.org/abs/2503.23791): LLM-based
  C-to-Rust translation that splits modules into functions, translates each,
  and reintegrates them, using static analysis, compiler-guided translation
  and repair. Evaluated on three Linux kernel modules (math, sort, ramfs);
  the abstract says less than 15% of the target code needed modification,
  and that it beats GPT-4o used alone (exact numbers not on the page).
  Differs: language migration inside one kernel, not migration between
  kernels or drivers.
- [IoTMigrator, EMNLP Findings 2025](https://aclanthology.org/2025.findings-emnlp.1048/):
  multi-agent LLM migration of embedded IoT code between RIOT and Zephyr,
  measured on its own six-task benchmark by compile pass rate and task
  completeness (reported gains of at least 50.5% and 83.4% in pass rate on
  RIOT and Zephyr over a baseline). Differs: application-level IoT code, not
  hardware drivers.
- [KernelGPT, arXiv 2401.00563 (ASPLOS 2025)](https://arxiv.org/abs/2401.00563):
  an LLM synthesises syscall specifications for kernel fuzzing and refines
  them from validation feedback; reports 24 new Linux kernel bugs, 11 with
  CVEs. Differs: it reads driver code to produce specifications, not to
  produce a driver. It does show the generate, validate, refine loop that
  ADR 0003 relies on.

No work was found that uses a model to translate a Linux hardware driver to a
different kernel's driver API with differential testing. Two searches found
none; this is the weakest "nothing found" claim in the document and the one
to re-check before any article states it.

## 4. Models in the boot chain or in detecting installed systems

Nothing found in two searches. What was found is rule-based:

- [os-prober (README)](https://sources.debian.org/src/os-prober/latest/README/):
  scans every disk and prints one line per partition found. Detection is a
  set of separate test scripts under `/usr/lib/os-probes/`, run once per
  partition; each returns whether it recognised the system. The README says
  it identifies Linux systems and that working out how to boot them is the
  job of a different tool, `linux-boot-prober`. The README does not say what
  signals each script checks.
- [bootnext](https://github.com/TensorWorks/bootnext) (README): picks a UEFI
  boot entry by matching a case-insensitive regular expression against the
  description of each entry; the first match wins.

**Differs:** ADR 0009 replaces this scripted detection with a model that
decides what a system is, which protocol to use and which companion files go
with it, inside the bootloader. No published precedent for that was found.
os-prober is the baseline B2 should measure against: the same disks,
scripted detection versus the model.

## 5. What looks new here, and what others already did

Judged only from the entries above.

**Already done by others**

- Swapping scheduling policy without touching the mechanism, with a safe
  fallback to the default (sched_ext, ghOSt).
- A small learned model making a kernel policy decision within microseconds
  (LinnOS), and the cost of running models in kernels (LAKE).
- A language-model agent that writes and deploys scheduler policies through a
  verifier (SchedCP).
- An "OS" for LLM agents (AIOS) and natural language to Bash or computer use
  (NL2Bash, OS-Copilot).
- Reusing Linux drivers elsewhere through a compatibility layer (LinuxKPI,
  Genode DDE), through unmodified kernel code in userspace (rump kernels,
  LKL), and through virtual machines (LeVasseur et al.).
- LLM translation of kernel and embedded code across languages and between
  OSes (LLMigrate, IoTMigrator), and LLM generate, validate, refine loops
  against a kernel (KernelGPT).

**Appears new, pending the checks below**

- Combining these in one design: a microkernel whose policy interface is
  bounded by the kernel and whose policy models are trained for the purpose
  rather than generated as code (the P track), with the OS booting with no
  model loaded.
- Model-assisted translation of a Linux driver to a native microkernel driver
  API, with a package registry carrying provenance and trust levels
  (ADR 0003). No precedent found; see the caveat in section 3.
- A model inside the bootloader that detects kernels and chooses a boot
  protocol, with decisions stored against a disk fingerprint and a loader
  that validates formats before jumping (ADR 0009). No precedent found.
- A capability broker that bounds the intent shell by both the user's
  permissions and the action's capability (ADR 0007). The surveyed agent
  systems were not read in enough depth to say how they gate actions; AIOS
  lists access control, but its page does not describe how.

**Do not claim in articles until measured or checked**

- That the project's approach beats any of the above: nothing here is
  measured yet.
- Any statement that "no one has done" the three novel items above without
  repeating the search with more terms and in the paper bodies.
- Every performance figure above is an authors' headline claim from an
  abstract.

## 6. Unverified: recalled, no source opened in this session

These are works the author remembers or saw only as a search hit. Do not cite
them until a source is opened.

- Decima (learned cluster scheduling, SIGCOMM 2019).
- KML, a machine-learning framework for operating system kernels.
- "Machine Learning for Load Balancing in the Linux Kernel" (APSys 2020).
- LLAMA and other learned memory-allocation work.
- Learned page prefetching and cache-eviction policies through eBPF.
- Karpathy's "LLM OS" framing, and Anthropic's computer use and MCP as
  references for agents acting on a machine.
- Nooks, SUD, Microdrivers, Decaf drivers: driver isolation and
  transformation work.
- RevNIC and other driver-synthesis and binary-translation work.
- NDISwrapper (Windows drivers on Linux).
- Rump kernel material beyond the one paper read: the anykernel thesis and
  the "Rump Kernels: No OS? No Problem!" article.
- GRUB's default-entry behaviour (the Arch wiki page was blocked when opened).
- Rust for Linux as a route to safer drivers.
- Unikernel and library-OS projects (MirageOS, Unikraft) as relatives of
  rump kernels and LKL.
- The full LAKE author list. The Texas page read lists one author; ACM and
  MIT metadata seen only in a search result list seven.
- Search hits not opened: the Linux Journal article on SchedCP, the SchedCP
  results from later arXiv versions (a baseline Claude Code scheduler that
  worked in one of three tries, 2.11x p99 latency on schbench), and the
  "AIOS" HTML versions.
