---
title: "B2: corpus and training of the boot model"
type: report
status: done
created: 2026-11-23
updated: 2026-11-23
tags: [boot, bootloader, model, corpus, training, evaluation, b2]
---

# B2: corpus and training of the boot model

Roadmap item B2: disk layouts and kernel images from several systems, the training run
and an evaluation harness on the host, with the accuracy on a held-out part of the corpus.
The model is the classifier of ADR 0027. Everything is under `boot-model/` and is host code
that nothing else depends on.

## Result

A model of the reference shape (512 window positions, embedding 32, dense 256 and 128, 34
scores, int8 weights and activations) was trained and exported as `\WSOS.MDL`. It is inside
the budget of ADR 0027: 4,239,616 weights (4.04 MiB), 4,231,424 multiply-accumulates per
candidate (the probe measured 4,231,168 with 32 scores, so 256 more, 0.006 %), a file of
4,239,660 bytes.

On the **test** set, releases that no decision of the method was taken on (see "The split"),
17,198 candidates on 1,500 generated disks, with the model run in the integer arithmetic the
bootloader will use:

| Output | Accuracy |
|---|---:|
| system family | 98.70 % |
| protocol | 97.16 % |
| role | 96.29 % |
| all three right on a candidate | 95.11 % |
| group | not learnable per candidate, see below |
| disks where every candidate is exact | 829 of 1,500 (55 %) |
| disks with no safety error (below) | 1,219 of 1,500 (81 %) |

The two cases that matter most for safety, on the same set (the metric counts a non-kernel called a kernel, and our kernel not called our kernel; a kernel of another system given the wrong protocol is a wrong label that the loader's format check, ADR 0027 section 4, is there to catch, and is not in these counts):

- A candidate that is **not a kernel image labelled as one**: 0 of 14,434.
- **Our own kernel missed**: 0 of 294.

Those two zeros hold with the file names the templates give. (A second run with the same seeds trained different weights and got 95.03 % on all three; see "Reproduce". The tables are from the first, the one whose file is described under "Weights".) They do **not** hold when the
names lie or the model meets a distribution it was not trained on (next table), and that is
the main thing this report has to say.

| Harder test | all three | false kernels | our kernel missed |
|---|---:|---:|---:|
| test, ordinary names | 95.11 % | 0 / 14,434 | 0 / 294 |
| test-renamed: every file of an entry has a random name | 87.87 % | 9 / 6,043 | 1 / 129 |
| test-decoy: files that are not kernels sit under the names of one (`\vmlinuz`, `\KERNEL.ELF`, `\EFI\BOOT\BOOTAA64.EFI`) | 90.64 % | 145 / 6,794 (2.1 % of the non-kernels; about 12 % of the 1,209 decoys) | 0 / 129 |
| distributions never seen in training (Fedora, Rocky, AlmaLinux, openSUSE all left out) | 69.54 % | 407 / 8,507 (4.8 %) | n/a |

What the numbers do not say:

- **The intervals are too narrow.** The 17,198 test candidates are 6,189 distinct windows
  and only 24 distinct kernel windows and 11 initrd files. A candidate repeats whenever a disk repeats a file. The Wilson interval in
  `render_report.py` treats candidates as independent and so understates the error; read the
  per-system and per-release rows instead of the interval.
- **Validation is 100 %.** The validation releases (listed below) were used while the method
  was developed, share the file-name templates with training and are near releases of the
  same distributions. 100 % there says the model fits what it has seen, not that it
  generalises. The test set is lower because it holds file formats the training set barely
  has (below).

## What the model gets wrong

On the test set, kernel recall is 76.9 %: 342 kernels called "other" and 296 called "initrd".
No other role has a recall below 100 %, and every error of the other outputs follows from
those (the second table of the report lists the confusion matrices).

- **Kernels in a format the training set has once.** Alpine 3.18 and older ship the `virt`
  and `lts` kernels, and Rocky and AlmaLinux ship every kernel, as a bare gzip stream, not
  a PE file. Under ADR 0027, section 4 the loader refuses such an image, so the label is
  "kernel, protocol none" (`catalogue.refine`). The training set has one such file (AlmaLinux
  9.4, a single release of a single distribution). The model calls most of the test ones "other"
  or "initrd". The family errors (223 of 17,198) are mostly the same files.
- **Names that look like another role.** Alma and Rocky kernels at `\<machine>\<version>\linux`
  are called an initrd.
- **A distribution it has not seen is much worse** (last table above): on Fedora, Rocky,
  AlmaLinux and openSUSE after training without any of them, initrd recall is 0.0 % (their
  initrds are xz streams and the training set holds only gzip ones), kernel recall 55.9 % and 406 data files are called kernels. (Entry text recall is also 0.0 % there, but that is by construction: the only generator of entry text
  is the Fedora family, which the fold removes, so the training set had none.) A boot model that meets a new
  distribution will be wrong, and wrong in the direction the safety rule fears. The loader's
  format check (ADR 0027, section 4) is what stops a false kernel from being started; the model
  does not.
- **It reads names.** Trained on the name alone it gets 95.98 % on the test set, on the bytes alone
  94.76 %, both together 95.11 %. On bytes alone the protocol is 100 % and the role 94.8 %: every error is a kernel called "other", no initrd was taken for another file. That is partly because the negatives are easy: the junk files are made by `synthetic.py` and a generated gzip file is not a real initrd, and partitions and Alpine rootfs files fill most of the 14,434 non-kernels. The "0 false kernels" of the headline is against those negatives, and a harder set of real compressed files was not built. The name carries the rest of the decision, and a file with a kernel's name and no kernel's header is called a kernel about 12 % of the time: 145 false kernels among about 1,209 decoy files of the decoy set (the 2.1 % in the table is the share of all 6,794 non-kernel candidates of that set).

## Label set

The closed sets of ADR 0027, section 3, in the order of the scores (`labels.py`, part of the
`\WSOS.MDL` contract). Eight family slots are reserved by the ADR; the corpus contains four.

| Output | Classes |
|---|---|
| family (8) | `none`, `ours`, `linux`, `bsd`; four slots unused: there is no Windows, macOS or other system in the corpus, so the model is not trained to say them |
| protocol (4) | `none`, `ours` (ADR 0013), `linux` (EFI stub), `chainload` |
| role (6) | `other`, `kernel`, `initrd`, `dtb`, `loader`, `entry` (Boot Loader Specification entry text) |
| group (16) | slot of the entry on that disk, 15 meaning "no entry" |

The labelling rule (`catalogue.py`): kernel, initrd, device tree, EFI loader and entry text carry
the family of their system; everything else is `none`/`none`/`other`. Protocol is the protocol of
the entry the file belongs to; an EFI loader is `chainload`; a kernel the loader cannot start
(a BSD kernel, a Linux kernel that is a bare gzip stream) is protocol `none`; our own bootloader is
`ours`/`none`/`other`, because the loader that is running is not an entry of its own menu. GPT
partitions are candidates too (ADR 0027, section 2) and are always `none`/`none`/`other`: the decision
table has nothing to decide about one.

## The candidate window: the contract B3 implements

`window.py`, with tests. 512 positions for every candidate:

- bytes 0 to 63: the **last 64 bytes of the path** as the firmware reports it (UCS-2 units narrowed
  to a byte, `?` above 0x7f, backslashes, leading backslash, the case the volume stores), **right-aligned**
  with zeros on the left;
- bytes 64 to 511: the **first 448 bytes of the file**, zeros after the end of a shorter file.

For a GPT entry: the partition name narrowed (at most 36 units) in the name slot, then the 16 bytes of
the type GUID as stored on disk, then the first 432 bytes of the partition. Content always starts at
offset 64, so the arm64 Linux magic and the PE fields land at fixed positions. Most templates use long names (`vmlinuz-virt`); only a few use 8.3 uppercase ones (`VMLINUZ`, `INITRD`, `KERNEL.ELF`), the only form `scripts/make-disk-image.py` can write. B3's own test disks, which carry 8.3 names, are therefore partly out of the distribution of the name half of the window. The window does not reach an ext4 or Btrfs superblock; a second window for that was not built.

## The corpus

All of it is rebuilt from pinned downloads and from our own repository; **nothing third-party is
committed**. The repository holds the pins (`boot-model/corpus/pins.json`: URL, size, SHA-256, licence and
how it was verified, 555 entries, and `ours-pins.json`, 18). The files live in
`~/.cache/workspace-os/boot-model` (or `WSOS_CORPUS_CACHE`), 208 MB without the builds of our own
kernels (555 MB more). Downloads are the first 4 KiB of a file by HTTP Range request, except
archives of at most 100 MB (the Debian netboot tarballs, Alpine minirootfs and OpenBSD `BOOTAA64.EFI`),
which are fetched whole and checked against the SHA-256 the publisher lists next to them (Debian
`SHA256SUMS`, Alpine `.sha256`, OpenBSD `SHA256`). The first 4 KiB of a large file has no
published checksum: those 4 KiB are pinned by a hash made at first fetch (**trust on first use**) and
`fetch.py fetch` detects a later change, not a bad first download.

| System | Releases (train / validation / test) | Files of the entry kinds | Source and licence |
|---|---|---|---|
| Alpine Linux, aarch64 netboot | 3.19.0 to 3.22.8 (8) / 3.23.0 to 3.24.4 (4) / 3.18.12 | vmlinuz, initramfs, dtbs of `virt`, `lts`, `rpi`; config, System.map, modloop as `other` | `dl-cdn.alpinelinux.org`, GPL-2.0-only (Linux), mixed |
| Debian installer, arm64 | bookworm, bullseye / trixie / buster | linux, initrd.gz, GRUB and shim EFI, GRUB modules, device trees | `deb.debian.org`, `archive.debian.org` dated directories, GPL-2.0-only, GPL-3.0-or-later (GRUB), mixed |
| Fedora Server | 40, 41, 43 / 44 / 39, 42 | vmlinuz (PE zboot), initrd.img (xz), shim, GRUB, `mmaa64.efi`, `grub.cfg` | `dl.fedoraproject.org`, `archives.fedoraproject.org`, GPL-2.0-only, GPL-3.0-or-later, BSD-2-Clause |
| AlmaLinux | 9.4 / none / 9.3 | vmlinuz (gzip), initrd.img, shim, GRUB | `repo.almalinux.org/vault` |
| Rocky Linux | none / 9.4 / 8.9, 9.3 | same | `dl.rockylinux.org/vault` |
| openSUSE Leap, ports aarch64 | 15.3 to 15.5 / 15.6 / 15.2 | linux, initrd | `download.opensuse.org` |
| OpenBSD, arm64 | 7.7 / 7.8 / 7.6, 7.9, 8.0 | `bsd`, `bsd.mp`, `bsd.rd`, `BOOTAA64.EFI` | `cdn.openbsd.org`, `ftp.eu.openbsd.org` (7.6), ISC/BSD |
| our own kernel and bootloader | K0, K1, K3 / K4, K5 / K2, K4b (ELF, debug and release), bootloader at two, one and one commits | `KERNEL.ELF`, `BOOTAA64.EFI` | this repository, GPL-2.0-only, built by `build_ours.py` |
| Alpine minirootfs, aarch64 | 3.19.9, 3.21.10 / 3.24.4 / 3.16.9, 3.18.12 | 5 releases of real binaries, shared libraries and scripts (hard negatives: ELF files that are not kernels) | `dl-cdn.alpinelinux.org`, mixed |
| QEMU `virt` device trees (`dumpdtb`) | Cortex-A53, A57 / A72 / `max` | 128 trees from QEMU 11.1.3, which are GPL-2.0-only | generated |
| generated junk | seeds 1, 2 and 3 | text, PNG, BMP, zip, tar, gzip, xz, random, zeros, PDF, scripts, a stand-in for `\WSOS.MDL` | `synthetic.py` |

1,998 assets: 985 train, 492 validation, 521 test. Not downloaded: Ubuntu (no reachable arm64 netboot
directory was found), Fedora 45, any Windows or macOS file (no licence to download them),
FreeBSD and NetBSD (no small loader or kernel without a 1 GB image; asking would have crossed
the size rule), openSUSE Tumbleweed (it rotates). Largest single download: the Debian netboot
tarball, 56 MB; total for the corpus about 208 MB.

**What is generated, not captured.** No real disk image was used. The path of each file comes from
templates that follow the packaging of each system (`layouts.py`): where Debian, Fedora, Alpine,
openSUSE, OpenBSD and our image put a kernel, an initrd, a loader. They are the author's reading of
those conventions. Entry text in the Boot Loader Specification format is written from the specification,
not copied from a machine, and the text a generated disk carries does not name that disk's files (random machine id and version), so a rule that reads those lines was not exercised, because no image carrying `loader/entries` was downloaded. A disk is the list
of candidates the enumeration of ADR 0027 will produce, with the window function of this report; the
enumeration itself does not exist before B3. The first 64 bytes of a path are the model's most useful
input and they come from templates the model was also trained on, which is why validation is perfect and
why the renamed and decoy sets are in this report.

## The split

Releases are held out whole, in two sets fixed at different times:

- **validation**: held out from the start (the newest release of each distribution, the K4 and K5 kernels,
  the Cortex-A72 trees), and **used while developing the method**. The first full run, trained
  without regularisation, got about 71 % on a held-out set of those releases and showed the network memorising the compressed bytes of an
  initrd. Three choices answer that and were made looking at validation: a cosine learning rate, a small
  weight (0.05) on the group head, and two augmentations (zero the content of a quarter of the windows and
  the name of 15 %, and replace the tail of high-entropy windows by fresh random bytes, which keeps the
  network from memorising a compressed stream). Hyperparameters were not otherwise tuned: 12 epochs, batch
  128, Adam at 1e-3.
- **test**: releases pinned only **after** those choices were frozen. Nothing in the method was chosen
  looking at them, with two disclosed exceptions, both after the first full run on it:
  (1) the generator drew two entries with the same path on one disk, a bug in the generator
  that does not depend on the model, fixed and the run repeated; (2) the first run labelled a Linux kernel
  that is a bare gzip stream as `linux`, and reading the errors showed those labels disagree with ADR 0027,
  section 4 (the loader refuses a non-PE Linux image). They became protocol `none` (`catalogue.refine`) and
  the run was repeated. The first run had 98.28 % on the test set; the corrected labels make the task harder
  and the result 95.11 %. Nothing else was changed after the test set was first read.

**How leakage was prevented.** No release is in two splits (`test_corpus.py` checks it). Junk files are
generated from different seeds for each split. The same bytes can still occur in two releases, because a
publisher ships one file twice (for instance a device tree that did not change): `render_report.py` and the
run report the **novel windows**, the test windows that no training window equals byte for byte. They are
76 % of the test candidates, and the model gets 93.57 % of them right on all three labels, against 95.11 %
on all of the set, so the leaked windows do not account for the result. QEMU trees are split by CPU model (their first 448 bytes are nearly identical across models, so their 100 % recall is not evidence of generalisation: the exact-equality check does not catch near-duplicates),
our own kernels by commit (a kernel at two commits differs by a few header words: the "ours" result mostly
says that this linker layout is recognised, from 10 distinct windows). A split by release does not make a
distribution new: a new release of Debian is still Debian. The last table of the result is the
leave-distributions-out view.

## Training stack

| Piece | Licence | Shipped? |
|---|---|---|
| numpy 2.5.5 | BSD-3-Clause | no, training and evaluation on the host only |
| scikit-learn 1.9.1 | BSD-3-Clause | no, only the tree and pair comparisons |
| `uv`, Python 3.12.13 | MIT/Apache-2.0, PSF | no |
| `boot-model/*.py` | GPL-2.0-only (the repository) | host tooling |
| `\WSOS.MDL` | proposed GPL-2.0-only (ADR 0027, section 6) | **not committed, see "Weights"** |

ADR 0005 applies to what is shipped or linked: nothing above is. `scripts/check-licences.py` has
nothing new to accept; no Cargo dependency was added.

The model is trained in float with numpy (no framework): embedding of each byte (256 x 32), the
512 embeddings laid end to end, a dense layer of 256 with ReLU, one of 128 with ReLU, 34 scores, a
softmax cross-entropy per output, Adam. It is then quantised after training (post-training, no
quantisation-aware training): embedding scale chosen from its range, a power-of-two shift between
layers calibrated on 4,000 training windows, weights clipped at the 99.99th percentile. The file
holds the two shifts (13 and 8). The headline numbers come from the **integer** forward pass
(`training.int_forward`, exact in float64), the arithmetic the bootloader will run; the float model
is within 0.4 points of it on every set. The pure Python `mdl.forward` and the numpy integer path were checked
to give the same 34 scores on three test windows in the run; the gate tests `mdl.forward` against
hand-computed values and the file round trip, without numpy.

## The comparison ADR 0027 kept open: a tree ensemble

`HistGradientBoostingClassifier` over the 512 raw bytes (60 rounds, 15 leaves, one ensemble per output, no tuning),
same data, same test set:

| Output | MLP (int8) | Trees |
|---|---:|---:|
| family | 98.70 % | 93.47 % |
| protocol | 97.16 % | 93.46 % |
| role | 96.29 % | 95.13 % |

The four ensembles hold 42,064 nodes (about 673 KB at 16 bytes a node, an estimate: no inference code for them exists) and need
about 10,500 comparisons a candidate in the worst case against 4.2 million multiply-accumulates for the
network, so they fit the budget with a large margin and are two to three orders of magnitude faster.
They are 1.2 to 5.2 points less accurate here (role 1.2, protocol 3.7, family 5.2). The network stays the model of ADR 0027; the trees are an
option only if a later corpus shows the gap closing. Trees were not tuned and the comparison is one run.

## Inside the budget, with the existing probe

The shape is the one `bootloader/probe/inferprobe.rs` measured (512 x 32, 256, 128, 32 scores) with two
more scores, so the times of ADR 0027 apply: about 3.9 ms for a candidate on TCG and 0.17 ms on HVF, 62 ms and
2.8 ms for a disk of 16 candidates (measured by B1-2, not repeated here, because a trained model runs the
same loops: there is no data-dependent branch). The extra 256 multiply-accumulates are 0.006 %. The gate
still runs `scripts/measure-boot-model.py --check`. `mdl.within_budget` checks the weight bytes (limit 16 MiB)
and the multiply-accumulates (limit 16 million) of any shape the exporter writes, and refuses a shape whose 32-bit sums could overflow or whose shift is outside 0 to 31 (a Python integer does not wrap, the loader's does), with tests at the limits. Not
measured: a trained file read from an ESP by the firmware, on a machine that is not the M1, or the model in
the bootloader (B3).

## The group output

The group is a per-disk numbering of entries, not a property of one file, so a model that reads
one candidate at a time cannot learn it, by construction. The generator numbers the entries of a disk in a random
order, which makes that visible (the same `\boot\vmlinuz-virt` is slot 0 here and 3 there); **this is a demonstration
of a structural property, not a finding about real disks**. Any numbering a real enumerator could produce (by
directory order, by path) is relative to the other files of the disk and has the same property, but none was trained
here. The numbers below are the consequence of the random numbering, and the 270 windows with several slots is a low
count because nearly every window carries a random version string in its path. Of 7,274 entry windows in the training set, 270 (3.7 %) occur
with more than one slot even with identical bytes and name. Accuracy on the files of entries is 25.3 %, and 65.6 %
overall only because the share of "no entry" is 54 %. A tree ensemble does no better (57.7 %), a pair head on the
trunk's features gets precision 61.8 % and recall 38.4 %, and the rule "same directory", given the true roles, 71.1 % and
75.8 %. ADR 0034 (`proposed`) records this and a way out: the model labels what a file is, code attaches
the companions, and the group is not used.

## Reproduce

The gate does not download the corpus or train. These commands do, from a clean checkout, with `cargo`,
`qemu-system-aarch64` (for the device trees), `python3` and `uv`:

```bash
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
python3 -I boot-model/fetch.py fetch                        # downloads and verifies the pinned corpus
python3 -I boot-model/build_ours.py                         # builds our kernel and bootloader at 10 commits (see below)
export OPENBLAS_NUM_THREADS=1 OMP_NUM_THREADS=1 VECLIB_MAXIMUM_THREADS=1
uv run --no-project --with numpy==2.5.5 --with scikit-learn==1.9.1 python boot-model/run_b2.py --out target/boot-model
python3 -I boot-model/render_report.py target/boot-model/results.json
python3 -I boot-model/mutation_check.py                     # the tests of the gate notice each of 59 breakages
uv run --no-project --with numpy==2.5.5 python boot-model/test_training.py   # the numpy path equals the pure Python reference
```

`fetch.py pin` regenerates `pins.json` from the mirrors (use it only to extend the corpus). The default seed
is 20261123 (`--seed`); the disks, the junk and the QEMU trees come from it. The run took about 17 minutes on an Apple M1,
the training 145 s. The corpus fingerprint of the numbers here is `9f3c19ee…` (`results.json`). Two things limit reproduction:

- **The weights are not byte-reproducible.** Two runs with the same seeds, the same data and single-threaded BLAS gave
  models with different SHA-256 (`cbcf1168…`, the one evaluated here, and `37996262…`) and test accuracies of 95.11 % and 95.03 %.
  The cause was not isolated (a numerical library on macOS is the suspect). Expect a spread of about a tenth of a point, not identical weights.
- **Our own binaries depend on the compiler.** `build_ours.py` rebuilds the kernels and bootloaders at the commits it lists
  and pins their first 4 KiB; with another `rustc` the pins change, and so does the fingerprint.

## Weights

`\WSOS.MDL` is **not committed or published**. ADR 0027, section 6 says B2 may not release weights until the licence question of
a model trained on the headers of other systems' kernels and loaders is answered, and it is not. The file of this report is
`target/boot-model/WSOS.MDL` (SHA-256 `cbcf1168c42faa93ce6b89c77b20b4d4cc6c6d778f17a17a1e0169d61cb5ef8e`, 4,239,660 bytes) in
the author's checkout; the commands above make another. ADR 0027, section 7 also asks for a script that fetches the weights with a pinned
SHA-256 and fails the gate when the file is missing. That cannot be built now: there is no place to fetch from while the weights cannot be
released, and a gate step that needs them would fail CI. **The owner has to decide** whether the weights are to be released (and where), or
whether B3 trains in its own gate.

## What could not be verified

- **Real disks.** No captured disk image and no real ESP were read. Names and directories come from templates, so the numbers are for those
  layouts. The enumeration of the bootloader (B3) may produce paths this report did not generate.
- **The hardware.** The int8 forward pass was checked against the pure Python reference, not against the Rust code that will run it (it does not exist yet).
  The shifts and clamping of the reference follow `inferprobe.rs`; B3 should add a test that its loader reproduces `mdl.forward` on a file written by `mdl.write`.
- **The number of kernels.** The test set has 24 distinct kernel files; per-release rows are 1 to 4 files each. Intervals computed over candidates are too narrow.
- **Whether a release is independent.** Several Alpine releases share a kernel version (6.12.115 in 3.21 and 3.22; 6.18.60 in 3.23 and 3.24).
  The novel-window figure says the leak is not the cause, not that it is zero.
- **Windows, macOS, BSD other than OpenBSD, Ubuntu, Arch, non-FAT filesystems, UKI images, ACPI-only machines.** None is in the corpus. A Windows `bootmgfw.efi` would
  be labelled by a model that has never seen one.
- **Unseen distributions** are the weak point (69.5 % there, 4.8 % false kernels), measured once with four distributions held out.
- **Timing of this model**, which is derived from B1-2.
- **Other trainers and shapes.** A larger window, QAT, a second window at a partition offset and the tree ensemble tuned were not tried.
- **Reproducibility of the weights** (above).
