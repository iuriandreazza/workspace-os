---
title: "B1-1 research: the boot model, its inputs, its runtime and its licences"
type: research
status: done
created: 2026-11-16
updated: 2026-11-16
tags: [research, boot, bootloader, model, uefi, licence, inference]
---

# B1-1 research: the boot model, its inputs, its runtime and its licences

Roadmap item B1-1. Input to the ADR of item B1-2, which settles what this
document only lays out. It does not choose between a language model and a
classifier, and it does not reopen ADR 0009 (a model detects kernels and
decides how to load them) or ADR 0008.

## Method and limits

Every statement carries one of three labels:

- **source**: read in this session, with the link or command that was read.
- **derived**: arithmetic or reasoning from sourced facts; the inputs are
  named, and the result is not a measurement.
- **unverified**: recalled, summarised by someone else, or seen only in a
  search result. Do not build a decision on it.

How the sources were read:

- Text files and web pages were fetched verbatim with `curl` into a scratch
  directory and read there. Nothing downloaded was executed. Raw GitHub files
  were fetched from branch heads; the commits in the table below are the heads
  at the time of checking, a few minutes after the fetch, so a file may differ
  if a commit landed in between.
- One page (the Debian `os-prober` Linux probe) first came through a
  summarising fetch tool. It was then replaced by the verbatim script from
  Debian Sources, and only the verbatim text is cited.
- Search results were used to find sources, never as a source. Items seen only
  in a search result are in section 8.
- Not found is weak evidence. "No precedent found" means a few searches
  returned nothing.
- The Salsa (Debian GitLab) pages answered with a proof-of-work challenge.
  That was not bypassed; Debian Sources was used instead.
- Vendor and author claims (speeds, accuracy, "no libc") are marked as such.
- Nothing was run. Every timing below is either a published figure or derived
  from one. B1-2 measures.

Pinned commits (head at the time of checking) and versions:

| Source | Pin |
|---|---|
| Linux (`torvalds/linux`) | `141b164bbe186423e295b139e40e8e2d11cf1341` |
| Rust (`rust-lang/rust`) | `c99bff8cd92cfc7545121db13506095828082138` |
| `karpathy/llama2.c` | `350e04fe35433e6d2941dce5a1f53308f87058eb` |
| `ggml-org/llama.cpp` | `d998c676ff463b1b8ad94ad7c0b905e281422fad` |
| `ggml-org/ggml` | `d9fe7bff3d2bf4d077e25c654486d02c321734aa` |
| `google/magika` | `3a3089991f66ba60d8a115fde31422c62d429cf4` |
| `tracel-ai/burn` | `c1af5e17b39b3ef642f951e230407282a6f77304` |
| `huggingface/candle` | `cfcbd7186d13271843ce7eb41d25659bd0835850` |
| `sonos/tract` | `10d0303cab896804967495b2bba9469964ac10fc` |
| `hardrave/NIGHTRUN` | `cb1c2bbca1083a0029bb08c5596e80662a0338fb` |
| `pbatard/efifs` | `e64895a7ef3ea326ecd39eddd20973e1c8744e1d` |
| `limine-bootloader/limine` (`trunk`) | `033b59788039d5aec35ced33082f23388ab33cda` |
| `BayesWitnesses/m2cgen` | `9784632311986234032673cdbfd29fc4c5cb429d` |
| `microsoft/LightGBM` | `53350cd896c6fd76d2cc03354f452f18646830e9` |
| `rust-ml/linfa` | `3b411a4c985cdf6b83430d9c44f11e338d300c50` |
| `tensorflow/tflite-micro` | `2c13ceb07010b642785ffb20f02e82dabb2bb435` |
| `uapi-group/specifications` | `ebdf46e5bd0fb14df82638b81d760a4a0c556464` |
| `uefi` crate docs | docs.rs, `uefi` 0.41.0 |
| UEFI Specification 2.10 | Wayback snapshot `20251207195607` of `uefi.org/specs/UEFI/2.10/02_Overview.html` (the live site answers 403) |
| `os-prober` | Debian Sources, version 1.85 (`forky`/`sid`) |
| SmolLM2-135M-Instruct | Hugging Face `12fd25f77366fa6b3b4b768ec3050bf629380bac` |
| Qwen2.5-0.5B-Instruct | Hugging Face `7ae557604adf67be50417f59c2c2f167def9a775`; the base-model card was read from `raw/main` and is not pinned |

Short names: `booting.rst` is
[`Documentation/arch/arm64/booting.rst`](https://github.com/torvalds/linux/blob/141b164bbe186423e295b139e40e8e2d11cf1341/Documentation/arch/arm64/booting.rst);
`efi-stub.rst` is
[`Documentation/admin-guide/efi-stub.rst`](https://github.com/torvalds/linux/blob/141b164bbe186423e295b139e40e8e2d11cf1341/Documentation/admin-guide/efi-stub.rst);
`uefi-spec` is the 2.10 snapshot above.

## 1. What a bootloader can read from a disk in UEFI

What the model can be given as input depends on what the firmware lets the
bootloader read. This section is about that, not about the model.

### What the firmware reads

- **Only FAT, in practice.** `efi-stub.rst` (source): "It's not possible to
  execute bzImage.efi from the usual Linux file systems because EFI firmware
  doesn't have support for them." Limine's README lists its supported
  filesystems as FAT12/16/32 and ISO9660
  ([README](https://github.com/limine-bootloader/limine/blob/033b59788039d5aec35ced33082f23388ab33cda/README.md),
  source). The UEFI requirement that the firmware implement FAT is in
  chapter 13 of the specification; that chapter could not be fetched
  (**unverified**).
- **Raw block access exists.** The `uefi` crate's `proto::media` module lists
  `BlockIO`, `DiskIo`, a `partition` module (partition information protocol)
  and `fs` ([docs.rs](https://docs.rs/uefi/0.41.0/uefi/proto/media/index.html),
  source). So the partition table and the first bytes of any partition can be
  read without a filesystem driver; reading files inside a non-FAT filesystem
  needs a driver (below).
- **Filesystem drivers exist but carry a licence problem.** `efifs` is a set of
  standalone read-only EFI filesystem drivers based on GRUB's, **GPLv3+**, with
  some files GPLv2+
  ([README](https://github.com/pbatard/efifs/blob/e64895a7ef3ea326ecd39eddd20973e1c8744e1d/README.md),
  source). Whether it supports aarch64 and ext4 was only partly read: the
  build instructions name `aarch64` as an architecture (source); the list of
  filesystems was not read (**unverified**). Under ADR 0005, GPL-3.0-or-later
  cannot be linked into the bootloader; loaded as a separate driver image it
  would be a separate program, as ADR 0008 judges GRUB. Whether that holds is
  a legal question (section 6).

### What identifies a system without reading its filesystem

- **Partition type GUIDs.** The Discoverable Partitions Specification gives
  `c12a7328-f81f-11d2-ba4b-00a0c93ec93b` for the EFI System Partition (this
  confirms the value the K2b research marked "from memory"),
  `bc13c2ff-59e6-4262-a352-b275fd6f7172` for the Extended Boot Loader
  partition, and `b921b045-1df0-41c3-af44-4c6f280d3fae` for a Linux root
  partition on AArch64
  ([spec](https://github.com/uapi-group/specifications/blob/ebdf46e5bd0fb14df82638b81d760a4a0c556464/specs/discoverable_partitions_specification.md),
  source). A disk that follows this specification announces a Linux root with
  no filesystem read. Many disks do not follow it (**unverified**: no survey
  was read).
- **Where entries live (source,
  [Boot Loader Specification](https://github.com/uapi-group/specifications/blob/ebdf46e5bd0fb14df82638b81d760a4a0c556464/specs/boot_loader_specification.md)).**
  Type 1 entries are `*.conf` files in `/loader/entries/` of the boot
  partition. Their keys include `title`, `version`, `machine-id`, `linux`
  (kernel image), `initrd` (may repeat), `efi`, `options` (kernel parameters),
  `devicetree`, `devicetree-overlay`, `architecture` (for example `aa64`) and
  `uki`. The files named by `linux`, `initrd`, `efi` and `devicetree` must be
  on the same partition as the entry, with absolute paths. Type 2 entries are
  unified kernel images, single `.efi` files in `/EFI/Linux/`. On a disk that
  uses this layout, with `/boot` or the ESP in FAT, the entry names the
  kernel, the initrd, the device tree and the parameters (**derived**).
- **Linux arm64 image header (source, `booting.rst`).** A 64-byte header with
  `text_offset`, `image_size`, `flags` and, at the end, a `magic` of
  `0x644d5241` ("ARM\x64"); `res5` is the offset of a PE header when booting
  through EFI. `efi-stub.rst` says that on arm64 "the Image itself masquerades
  as a PE/COFF image", so the same file starts as a PE file and carries the
  arm64 magic. This is exact, fixed-offset information.
- **How an existing scanner decides (source, `os-prober` 1.85 scripts from
  Debian Sources, read verbatim).**
  - The Linux probe (`mounted/common/90linux-distro`) runs on a **mounted**
    root filesystem: it looks for `ld*.so*` under `lib*/` (with `/boot`
    present) or under `usr/lib*/`, then reads `etc/os-release` or
    `usr/lib/os-release`, then falls back to `etc/debian_version` and a list of
    `*-release` files. Its own comment calls the test "inaccurate".
  - The Microsoft probe for the ESP (`mounted/arm64/efi/20microsoft`) looks
    for `bootmgfw.efi` and a `bcd` file under a `microsoft/boot` directory,
    found through a helper (`item_in_dir`) whose case handling was not read,
    and prints "Windows Boot Manager".
  - The generic EFI probe (`mounted/arm64/05efi`) accepts only a FAT
    partition whose GPT type is the ESP GUID (or an MBR partition of type
    `0xef`), then runs the per-system probes under `mounted/efi/`.
  - The consequence (**derived**): the evidence a scanner uses for Linux is on
    the root filesystem, which the firmware cannot read; the evidence for
    Windows is on the ESP, which it can. A bootloader without a filesystem
    driver sees the ESP, the XBOOTLDR partition, the partition table and raw
    partition bytes. That is the realistic input space of B2's corpus, unless
    the driver question is decided otherwise.
- **Magika (source,
  [README](https://github.com/google/magika/blob/3a3089991f66ba60d8a115fde31422c62d429cf4/README.md)
  and the
  [`standard_v3_3` model page](https://github.com/google/magika/blob/3a3089991f66ba60d8a115fde31422c62d429cf4/assets/models/standard_v3_3/README.md)).**
  The model's label list has `elf`, `pebin`, `coff`, `iso`, `squashfs` and
  `gzip`, and no label for a Linux kernel image, an initrd or an EFI stub. The
  README says the full output space also depends on built-in heuristics and
  format resolvers, so this is a statement about the model's labels, not about
  everything Magika can report. Kernel image labels would have to come from our
  own corpus (B2).

### Not read

The x86 Linux boot header and protocol, the ELF and PE magic bytes as
specifications (only the arm64 header was read), the ext4 superblock layout,
the BSD and macOS boot files, and the Debian layout of `/boot`. All
**unverified**.

## 2. The typed decision

ADR 0009 fixes what the model decides: the system, the boot protocol, the
companion files, and the parameters. The protocols the loader can implement
are, from what has been read:

| Protocol | What the input of the decision must name | Source |
|---|---|---|
| Our kernel, entered with `BootInfo` | the kernel file; boot mode | ADR 0013 |
| Linux through its EFI stub | the image; zero or more `initrd=` paths; an optional `dtb=` path; a command line | `efi-stub.rst`: `initrd=` is the only stub-specific parameter, paths are absolute from the root of the ESP with backslashes, everything else goes to the kernel; `dtb=` overrides the device tree only as "a last resort" because firmware adds runtime data to it |
| Another EFI loader, chain-loaded | the loader file | BLS `efi` key; `os-prober` Microsoft probe |
| Linux entered directly, MMU off | the image, a device tree, parameters | `booting.rst`; ADR 0013 puts this in K2d's scope |

Rule 4 of ADR 0009 (the loader checks the format before jumping) can check
that a path exists and that the file has the header the protocol needs. It
cannot check that a command line makes sense.

### The crux, as a table of which fields are closed

This table is **derived** from ADR 0009 and the protocols above.

| Field | Closed set or free text | What validates it |
|---|---|---|
| system family | closed (an enum of the families the corpus has) | nothing; a label |
| display name | free text, but it exists in `os-release` or BLS `title` | nothing needed |
| protocol | closed (the table above) | the loader's format check |
| kernel path | closed if the input lists candidates; free text if the model writes it | the file exists and has the right header |
| initrd and dtb paths | same as the kernel path | the file exists |
| parameters | free text, unless taken whole from a BLS `options` line or left empty | syntax at most |

What this means for the choice (the two sides; B1-2 chooses):

- **A classifier** (a model that scores or picks among options) needs every
  field to be a choice among candidates. That holds for system, protocol and
  paths **if something enumerates the candidates first** (list the partitions,
  list the files of the ESP, find the images). It does not hold for
  `parameters`, except by choosing among strings that already exist on the
  disk. Enumeration is mechanism, not detection, but it is a rule-based step
  that feeds the model, and ADR 0009 says that no rule-based scanner runs
  beside the model to overrule it. The ADR must say whether feeding candidates
  counts as that. This document does not decide it.
- **A language model** can read a listing as text and write the decision, and
  its output can be forced into the typed shape. `llama.cpp`'s GBNF grammars
  "constrain model outputs", and JSON Schema converts to a grammar (source,
  [GBNF guide](https://github.com/ggml-org/llama.cpp/blob/d998c676ff463b1b8ad94ad7c0b905e281422fad/grammars/README.md)).
  A grammar guarantees syntax, not truth: the model can still name a path that
  does not exist or invent a parameter, and a grammar engine is more code in
  the bootloader (**derived**). Whether a grammar engine exists outside
  `llama.cpp` that fits UEFI was not researched.

## 3. Model kind and size: candidates

Sizes of memory are **derived** (parameters times bytes per parameter,
ignoring scales, embeddings tied or not, the key-value cache and activations).
Parameter counts are from the Hugging Face API responses, source.

| Candidate | Kind | Size | Memory by precision (derived) | Licence of the weights | Notes |
|---|---|---|---|---|---|
| Magika `standard_v3_3` | Trained classifier over file content | "a few MBs" (vendor) | n/a | Apache-2.0 for the project (README and `LICENSE`); the weights file was not examined separately | Vendor: about 5 ms per file on one CPU, ~99% average precision and recall on its own test set of ~100M files, 200+ labels; it reads "a limited subset" of the file. A Rust CLI with Python, JavaScript and Go bindings (README); its label set does not cover what we need |
| Own small trained classifier (decision tree, boosted trees, small neural network) | Trained by us on B2's corpus | Not known (**unverified**: no size was read or measured) | n/a | Ours; training tools are not linked | `m2cgen` (MIT) "transpiles trained statistical models into native code" and lists Rust as an output language; the names `DecisionTree` and `RandomForest` appear in its README. `LightGBM` (MIT) and `linfa` (MIT OR Apache-2.0, has decision trees) are possible trainers. Nothing here was tested |
| llama2.c "tinyllamas" | Small language model, Llama 2 architecture, fp32 | 260K, 15M, 42M, 110M parameters (README table) | fp32: ~1 MB, 60 MB, 168 MB, 440 MB | Not read (**unverified**) | Trained on TinyStories, a children's-story corpus; the README says nothing about instruction following. Useful as a size reference and as a source of a very small architecture; not as a ready decision model |
| SmolLM2-135M-Instruct | Small language model | 134,515,008 parameters (BF16) | BF16 ~257 MiB; int8 ~128 MiB; 4-bit ~64 MiB | Apache-2.0 (card and API metadata) | The card calls the family "lightweight enough to run on-device"; the 135M model was trained on 2T tokens. No claim about structured output was read |
| Qwen2.5-0.5B-Instruct | Small language model | 494,032,768 parameters (BF16); the card read was the base model's: 0.49B total, 0.36B non-embedding, 32,768-token context | BF16 ~942 MiB; int8 ~471 MiB; 4-bit ~236 MiB | Apache-2.0 per card metadata; the `LICENSE` file was not read | Larger than needed if the decision is closed-set |
| Bonsai 8B (ADR 0009's upper bound) | LM of the intent shell | 8.2B parameters, ~1.15 GB as GGUF (roadmap model notes) | ~1.15 GB for weights alone | Apache-2.0 (roadmap model notes) | ADR 0009 asks for something smaller |

**Timing, published and derived.** The only published figures for neural
inference inside a UEFI application are NightRun's (section 4):

- Llama 3.2 1B, Q8_0, QEMU/KVM, 8 cores, x86_64: prompt 52-56 tokens/s,
  generation ~20 tokens/s, 5.6 s from boot to chat. Self-reported, "single
  scripted runs that vary about 20% with host load".
- Granite 3B, Q4_K_M, a real Raspberry Pi 5 (8 GB): prompt 6.2 tokens/s,
  generation 3.0 tokens/s, with kernels the project calls "pre-sdot".

Scaling these by parameter count to a 135M model, naively (**derived**, wide
error, ignores that quantisation, embedding share and attention differ): about
400 tokens/s for the prompt from the 1B figure, and about 140 from the Pi
figure. A 300-token input would then take roughly 0.75 to 2 s, and 60 generated
tokens about 0.4 to 1 s. A classifier of Magika's class is claimed at ~5 ms,
two orders of magnitude lower. Neither number says what QEMU will do on the
machine B1-2 uses: see section 7.

## 4. Inference inside the UEFI environment

### The environment, from the specification (source)

UEFI Specification 2.10, section 2.3.6, AArch64 platforms:

- "During boot services only a single processor is used for execution. All
  secondary processors must be either powered off or held in a quiescent
  state."
- The processor runs at the highest non-secure EL available (EL2 or EL1), with
  the MMU and the instruction and data caches on and RAM identity-mapped.
- "All floating point traps and exceptions will be disabled (FPCR=0,
  CPACR_EL1.FPEN=11, CPTR_EL2.TFP=0). This implies that the FP unit will be
  enabled by default."
- "128 KiB or more of available stack space"; interrupts enabled, with no
  interrupt services other than the boot services timer.

On the x86 platforms (same page, source; the text read did not separate
IA-32 from x64): the floating-point control word must be
initialised to `0x027F`, the multimedia control word to `0x1F80`, and `CR0.EM`
and `CR0.TS` must be zero. That FP and SSE are therefore usable by a UEFI
application on x86 is **derived**; enabling SSE also depends on other state
that was not read.

A 2020 EDK2 patch by Ard Biesheuvel says that without enabling the VFP early
in `PrePeiCore` "we may end up triggering synchronous exception when code
attempts to use FP or SIMD registers, which is permitted by the spec"
([edk2-devel patch](https://patchew.org/EDK2/20200107092215.3271-1-ard.biesheuvel@linaro.org/20200107092215.3271-3-ard.biesheuvel@linaro.org),
source). That is about EDK2's own early startup code on one platform, not
about the state handed to applications; it shows that FP enablement in
firmware startup code was something that had to be done explicitly. The Rust
documentation separately warns that "not all firmwares correctly configure
those units" ([Rust platform support: UEFI](https://github.com/rust-lang/rust/blob/c99bff8cd92cfc7545121db13506095828082138/src/doc/rustc/src/platform-support/unknown-uefi.md),
source). The FP state was **not** examined on QEMU's EDK2 or VirtualBox's
firmware: this is for B1-2 to test.

### What the compiler does by default

The Rust page says MMX, SSE and "other FP-units are disabled by default". The
compiler itself, run with the project's pinned toolchain, says something
narrower:

```
$ rustc --version
rustc 1.100.0 (0ef1bc386 2026-11-09)
$ rustc --print cfg --target aarch64-unknown-uefi | rg 'target_feature|panic'
panic="abort"
target_feature="neon"
$ rustc --print cfg --target x86_64-unknown-uefi | rg target_feature
target_feature="fxsr"
$ RUSTC_BOOTSTRAP=1 rustc -Z unstable-options --print target-spec-json --target aarch64-unknown-uefi | rg features
  "features": "+v8a",
```

(source: the commands above, run in this repository's worktree). So on
`aarch64-unknown-uefi` the compiler may emit NEON and floating-point code by
default; on `x86_64-unknown-uefi` it may not emit SSE. The page's sentence
holds for x86_64 and does not match aarch64 (**derived**: `target_feature`
output proves the feature is enabled for codegen, not that any given function
uses it). The kernel is different: `.cargo/config.toml` builds it for
`aarch64-unknown-none-softfloat` on purpose (source: that file). The
bootloader is not bound by that rule today. NightRun, an x86_64 UEFI
application, says it needs "a custom hard-float target" for its binary
(source, its README), consistent with the x86 line above.

### Memory

- For `no_std`, the Rust page says `alloc` works "if an allocator is provided
  by the user", and lists `uefi-rs` as a crate that "implements allocators"
  (source, the Rust page above). The `std` port's `alloc` is built on
  `AllocatePool`/`FreePool` with `EfiLoaderData`, and the page notes that all
  memory must be dropped before `ExitBootServices` (same page, its `std`
  section). What the `uefi` crate's allocator calls underneath was not read
  (**unverified**).
- Our bootloader allocates kernel file buffers and the memory map as
  `LOADER_DATA` (source: `bootloader/src/main.rs`, `allocate_pages` and
  `memory_map` calls), which ADR 0013 converts to BootloaderReclaimable.
- **Derived risk, the one that matters for the design.** ADR 0013 requires the
  kernel at the fixed range at `0x40200000` through `AllocatePages` at an
  exact address, which fails if the range is taken, and notes that the
  loader's kernel file buffer assumes the firmware allocates top-down. A model
  of tens or hundreds of megabytes loaded before the kernel range is claimed
  could take that range. Weights and scratch memory must be allocated after the
  kernel's range is reserved, or released before. ADR 0013's findings also say
  VirtualBox needs 2048 MiB: the weights are extra on top of a machine that
  already needs that much.
- NightRun sizes its key-value cache and scratch up front and allocates
  nothing during generation (its README, self-reported). That is a design
  worth copying; it was not verified.

### Processors and time

- The specification says one processor (above). NightRun says it does
  "multi-core inference through firmware MP services". These do not conflict
  if MP services is a separate firmware protocol that starts other cores on
  request, but the MP services protocol belongs to the platform
  initialisation specification, which was not read (**unverified**). The Rust
  page also says "UEFI forbids running code on anything but the boot
  CPU/thread". Treat multi-core inference as unsupported until tested.
- The boot services watchdog timer: a long computation in a UEFI application
  may be reset by the firmware's default watchdog. This is recalled from the
  specification's boot services chapter and was not read (**unverified**).
  B1-2 should read that chapter and, if a watchdog applies, state its period
  next to the measured inference time.

### Porting existing engines

- **llama2.c** is "pure C with no dependencies" in its README, but `run.c`
  includes `stdio.h`, `stdlib.h`, `math.h`, `fcntl.h` and uses `fopen`, `mmap`,
  `malloc`, `expf`, `sqrtf`, `powf`, `cosf` and `sinf` (source, `run.c`). It
  needs a C library; "no dependencies" means no third-party libraries. It is
  fp32 for `run.c`; an int8 variant is `runq.c`.
- **llama.cpp / ggml** are MIT (source). Whether either builds freestanding for
  `aarch64-unknown-uefi` was not read. The README of `llama.cpp` was not read
  for portability claims (**unverified**).
- **Burn**, `no_std` backend: "Burn's core components support `no_std`" and
  "only the Flex backend can be used in a `no_std` environment" (source,
  [README](https://github.com/tracel-ai/burn/blob/c1af5e17b39b3ef642f951e230407282a6f77304/README.md));
  the workspace version is `0.23.0-pre.2`, a pre-release, `MIT OR Apache-2.0`.
  Its transitive dependencies were not read.
- **tract** and **candle**: their READMEs read here do not claim `no_std`
  (**derived**: absence in the README, not a negative proof). TensorFlow Lite
  for Microcontrollers is Apache-2.0 only (section 6).
- **Hand-written kernels** are the route both precedents took.

### Precedents for inference in UEFI (not for boot decisions)

| Project | What it shows | How it was read |
|---|---|---|
| [NightRun](https://github.com/hardrave/NIGHTRUN), Rust, MIT | A UEFI application boots into a chat with Llama 3.2 1B, Llama 3.2 3B, Granite 4.1 3B and Qwen3 4B; hand-written quantised kernels (AVX2 on x86_64, NEON on a Raspberry Pi 5), "no_std where it counts"; **stays inside boot services and never calls `ExitBootServices`**; greedy output pinned against `llama.cpp`; the model is copied into RAM with CRC-32 checks while it streams in; benchmarks above | The rendered README on GitHub, read in full. Self-description; its text says most of the code was written with a coding agent. The source files and `LICENSE` were not read |
| "OO" (`llm-baremetal`), freestanding C | Claims Mamba-2.8B and Llama 2 inference on x86_64 under UEFI "no libc, no malloc", with 1.3 tokens/s for Mamba-2.8B and a QEMU image | The Hugging Face model card only. The repository, the code and the licence were not read; the "first" claim is the author's |

Both are custom engines, which is consistent with ADR 0009's remark that none
of the usual runtimes works in UEFI. One implication of NightRun's design
(**derived**): a model run before `ExitBootServices` can use the firmware for
reading the disk, which our bootloader already does, so the model does not need
its own drivers.

No project was found that puts a model in the bootloader to decide what to
boot (same finding as `docs/prior-art.md`, section 4; one search in this session,
for neural inference in a UEFI application, surfaced only the two projects
above and a few unopened ones listed in section 8).

## 5. What B1-2 has to settle

Questions this research leaves open, for the ADR (it does not answer them):

1. **Kind of model.** Section 2's table: does feeding enumerated candidates
   count as the rule-based scanner ADR 0009 forbids? That decides whether a
   classifier is possible.
2. **Parameters.** Free text, taken from an existing line, or left empty.
3. **Input.** FAT and partition table only (the realistic case), or a
   filesystem driver (GPLv3+ `efifs`, or our own).
4. **Floating point.** Whether the bootloader uses the compiler's default
   NEON on aarch64, and what K14's x86_64 build needs (a hard-float target).
5. **Where the weights live.** A file on the ESP or inside the `.efi`; that
   affects the licence question and the integrity item B4.
6. **Memory order.** Reserve the kernel's range before loading the model.

## 6. Licences, under ADR 0005 and ADR 0017

ADR 0005: the OS is GPL-2.0-only; third-party code must be GPL-2.0
compatible; GPL-3.0, AGPL-3.0 and Apache-2.0-only code cannot be linked in;
dual MIT/Apache-2.0 crates are used under MIT; model weights have their own
licences, checked case by case. ADR 0017: MPL-2.0 code may be linked under four
conditions.

| Component | Declared licence (what was read) | Under the ADRs |
|---|---|---|
| llama2.c | MIT (`LICENSE`) | Linkable. Needs a C library, so it would be translated, not linked |
| llama.cpp, ggml | MIT (`LICENSE`) | Linkable if a build exists |
| Burn | `MIT OR Apache-2.0` (`Cargo.toml`); `LICENSE-MIT` read | Linkable under MIT; transitive crates not checked |
| tract | MIT or Apache-2.0 (README) | Linkable under MIT |
| candle | MIT and Apache-2.0 (README badges) | Linkable under MIT; the `LICENSE` files were not read |
| linfa | MIT or Apache-2.0 (README) | Linkable under MIT; training time only unless used at runtime |
| m2cgen, LightGBM | MIT (`LICENSE`) | Training and generation tools, not linked. Whether generated code carries the generator's licence was not read (**unverified**) |
| NightRun | MIT (its README); `LICENSE` not read | Source of design; reuse of code would need the file read |
| TFLite Micro | Apache-2.0 only (`LICENSE`, header read) | **Cannot be linked** (ADR 0005) |
| Magika | Apache-2.0 only (`LICENSE`) | **Code cannot be linked**; the model weights as data are the case-by-case question |
| efifs | GPLv3+, some files GPLv2+ (README) | **Cannot be linked** into the bootloader; a separate driver image is a legal question |
| Any MPL-2.0 crate read here | none | ADR 0017 is not triggered by anything in this document; `ucs2` already accepted stays as recorded |
| SmolLM2-135M-Instruct weights | Apache-2.0 (card) | Data; case by case |
| Qwen2.5-0.5B weights | Apache-2.0 (card metadata); `LICENSE` not read | Data; case by case |
| Llama 3.2 weights | "Llama Community License", from NightRun's README, not read at source | Not an open licence as far as is known (**unverified**) |
| tinyllamas weights | not read | **unverified** |

Open for the legal review that the roadmap already lists: whether weights
compiled into the GPL-2.0-only `.efi` differ from weights in a separate file
on the ESP; whether a model trained on a corpus of disk images and kernel
files from other systems inherits those files' licences. This is a reading of
licence texts by the project, not legal advice.

## 7. What B1-2 must measure

Roadmap item B1-2 asks for an inference time measured in the UEFI environment
under QEMU. The inputs that decide whether a number means anything:

- **The accelerator.** On the machine this research ran on, `uname -m` and
  `rustc -vV` say `x86_64`, and `qemu-system-aarch64 -accel help` (QEMU 10.0.13,
  Debian) lists one accelerator: `tcg` (source: those commands). An aarch64
  guest here is therefore emulated. Whether QEMU on the ARM Mac the project
  also uses can accelerate an aarch64 guest was not checked (**unverified**).
  The published NightRun figures are KVM on x86_64 hardware and a real Pi 5.
  None transfers; B1-2 reports the accelerator and the host with every number.
- Cold (first boot, model loaded from disk) and warm (decision stored,
  ADR 0009 rule 2) boots are different questions; only the first runs the
  model.
- The firmware's FP state, the watchdog, the memory order and the VirtualBox
  RAM size (sections 4 and 5).
- Accuracy is B2's. B1-2 measures time, not correctness.

## 8. Unverified, and what was not read

- The UEFI specification's chapter 13 (FAT requirement), chapter 7 (boot
  services, the watchdog timer, `AllocatePages`), the platform initialisation
  specification (MP services).
- `CPACR_EL1` as an architecture register: the spec text gives its required
  value, the Arm manual was not read.
- The x86 Linux boot header, ELF and PE magic bytes, ext4 superblock, BSD and
  macOS boot files, Debian's layout of `/boot`.
- The `LICENSE` files of Qwen, NightRun and candle; the licences of the OO
  repository, the tinyllamas weights and Llama 3.2; the list of filesystems of
  `efifs`; the transitive dependencies of Burn's `flex` backend.
- Whether `llama.cpp` or `ggml` builds freestanding; whether `m2cgen` output is
  free of the generator's licence; any structured-output behaviour of
  SmolLM2 or Qwen at this size; any survey of how many real disks follow the
  Discoverable Partitions Specification or the Boot Loader Specification.
- Search-only hits, not opened: a `linaro-uefi` list thread on whether the
  UEFI specification requires preserving NEON state across runtime service
  calls; the Hackster and CNX Software articles on NightRun; Unikraft's
  `app-llama2-c` and a `trholding` fork of llama2.c with bare-metal support;
  the `cognisoc/cllm` unikernel.
- Whether QEMU on an ARM Mac can accelerate an aarch64 guest.
- Every vendor and author figure: Magika's speed and accuracy, NightRun's and
  OO's throughput.

## Sources not repeated above

- [llama2.c README and `run.c`](https://github.com/karpathy/llama2.c/blob/350e04fe35433e6d2941dce5a1f53308f87058eb/README.md)
- [SmolLM2-135M-Instruct card](https://huggingface.co/HuggingFaceTB/SmolLM2-135M-Instruct/blob/12fd25f77366fa6b3b4b768ec3050bf629380bac/README.md),
  [Qwen2.5-0.5B card](https://huggingface.co/Qwen/Qwen2.5-0.5B), both read as
  `raw/main/README.md`; parameter counts from
  `huggingface.co/api/models/<id>`
- [SmolLM2 paper abstract](https://arxiv.org/abs/2502.02737) (the abstract
  describes the 1.7B model; the 135M figures come from the card)
- [os-prober 1.85 sources](https://sources.debian.org/src/os-prober/1.85/os-probes/)
- [OO model card](https://huggingface.co/djibydiop/llm-baremey)
- [m2cgen](https://github.com/BayesWitnesses/m2cgen/blob/9784632311986234032673cdbfd29fc4c5cb429d/README.md),
  [linfa](https://github.com/rust-ml/linfa/blob/3b411a4c985cdf6b83430d9c44f11e338d300c50/README.md),
  [TFLite Micro licence](https://github.com/tensorflow/tflite-micro/blob/2c13ceb07010b642785ffb20f02e82dabb2bb435/LICENSE)
