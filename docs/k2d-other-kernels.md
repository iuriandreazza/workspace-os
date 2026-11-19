---
title: "K2d: loading other kernels"
type: report
status: done
created: 2026-11-19
updated: 2026-11-19
tags: [boot, bootloader, uefi, linux, chainload, k2d]
---

# K2d: loading other kernels

Roadmap item K2d: the bootloader starts Linux through its EFI entry (with a
command line and an initial ramdisk) and the EFI loader of another system,
from entries written by hand. Mechanism only: no detection and no model
(ADR 0009, track B). Our own entries (Normal, Recovery) are untouched and need
nothing new. The format of the entries is ADR 0025 (proposed: the user accepts
or not when reviewing the pull request).

## What was checked in the sources

Every point below was read on 2026-11-19. What could not be read is listed at
the end.

### Linux through its EFI entry

- On arm64 the kernel `Image` is itself a PE/COFF image with an EFI stub linked
  in; "the Image itself masquerades as a PE/COFF image", and there is no
  compressed-kernel support at that level
  ([EFI stub, kernel docs](https://docs.kernel.org/admin-guide/efi-stub.html)).
  Distributions often ship a compressed variant (an EFI "zboot" PE: a small
  PE whose payload is gzip). The Alpine kernel used in the gate is one: its
  header is `MZ`, `zimg`, `gzip` (checked by reading the file), a PE32+ image
  for machine `0xaa64` with subsystem 10 (EFI application). Both forms start
  the same way, through `LoadImage` and `StartImage`.
- The arm64 header has the magic `ARM\x64` and a field holding the offset of
  the PE header, which is how firmware finds the EFI entry
  ([booting arm64 Linux](https://docs.kernel.org/arch/arm64/booting.html)).
- **Command line.** The stub reads the image's `LoadOptions` (UCS-2) and
  converts them to the kernel's command line; the text comes from the
  `LoadedImage` protocol of the started image (`efi_convert_cmdline` in
  [`efi-stub-helper.c`](https://github.com/torvalds/linux/blob/master/drivers/firmware/efi/libstub/efi-stub-helper.c)).
  The loader therefore sets `LoadOptions` and `LoadOptionsSize` on the child's
  `LoadedImage` before `StartImage`. The conversion is bounded by
  `COMMAND_LINE_SIZE`, hence the 1023-character limit of the entry parser, and
  it reads ASCII, hence "printable ASCII only".
- **Initrd.** The stub first looks for the vendor media device path
  `LINUX_EFI_INITRD_MEDIA_GUID` (`5568e427-68fc-4f3d-ac74-ca555231cc68`, from
  [`include/linux/efi.h`](https://github.com/torvalds/linux/blob/master/include/linux/efi.h);
  the x86 boot document names the same GUID,
  [section "PE/COFF entry point"](https://docs.kernel.org/arch/x86/boot.html)),
  finds the handle that carries it, and calls `LoadFile2` on it: first with a
  null buffer to learn the size (the handle must answer `EFI_BUFFER_TOO_SMALL`
  and set the size), then with memory it allocated (`efi_load_initrd_dev_path`
  in the same file). Only when no handle has the path does it fall back to the
  `initrd=` option. So the loader installs, on one new handle, the device path
  protocol (that vendor media node, then the end node) and `LoadFile2`.
  `bootinfo::initrd::reply` is that handshake, tested on the host.
- An initrd must lie in the 1 GiB-aligned window the arm64 boot document
  describes; the stub allocates it, not us, so that is its concern.

### Chainloading another system's EFI loader

- A loader is a PE image: `LoadImage` then `StartImage`. Limine's chainload
  does it this way: it reads the image, builds the file path from the
  partition's device handle and a media file-path node, loads the image from
  memory, **sets the loaded image's device handle, file path and load options**
  on the child, and starts it; when the child returns, Limine returns with the
  same status
  ([`common/protos/chainload.c`](https://github.com/limine-bootloader/limine/blob/trunk/common/protos/chainload.c),
  read; the licence is BSD-2-Clause, [`COPYING`](https://github.com/limine-bootloader/limine/blob/trunk/COPYING)).
  No Limine code is copied here; the order of steps was the reference. The
  device handle and file path matter: another system's loader finds its own
  files (a boot manager's configuration, for example) relative to where it
  was loaded from.
- Here the device path given to `LoadImage` is the device path of the partition
  the bootloader started from, followed by a file-path node; EDK2 sets
  the child's device handle from it (the probe in the gate shows it; another
  firmware was not tried). The loader gives `LoadImage` the bytes it
  read and checked, so what was checked is what starts.
- **Another partition** needs the partition to be named in the entry (its GPT
  partition GUID) and its `SimpleFileSystem` found among the handles by the
  hard-drive node of its device path. It is not built in this item: the gate
  has no image with a second partition, and code that no boot runs is not
  accepted here. ADR 0025 records it.

### Not read

- The UEFI specification pages for `LoadImage`, `StartImage` and
  `EFI_LOAD_FILE2_PROTOCOL` (uefi.org answered 403 to the fetch). What the code
  relies on from the specification comes from the Linux and Limine sources
  above and from the documentation of the `uefi` crate, and is exercised by the
  gate on EDK2 only.
- The PGP signature of the Alpine tarball (see below).

## What the bootloader does

`bootloader/src/other.rs`; the logic is in `bootinfo`:

- `bootinfo::config` parses `\WSOS.CFG` (ADR 0025). `bootinfo::pe` checks the
  image: `MZ`, a PE signature at `e_lfanew`, machine `0xaa64`, a PE32+ optional
  header, subsystem 10, an entry point inside the image, and each section's
  file data inside the file. `bootinfo::initrd` answers the `LoadFile2`
  handshake. `bootinfo::menu` has the entry kinds.
- A refusal (file missing, not a PE image, wrong machine, subsystem or cut
  file, an initrd that is declared and absent or empty, a firmware step that
  failed) prints `cannot start <label>: <reason>` and draws the menu again with
  the countdown stopped.
- The image's buffer is given back to the firmware once `LoadImage` copied it,
  and the initrd's after the image returns; the protocol handle is removed.
- A test application, `chainprobe` (`bootloader/probe/chainprobe.rs`), stands in
  for another system's loader: it prints that it started and the file path
  the bootloader gave it, then returns.

## The Linux kernel of the test

The gate must start a real Linux on aarch64 under QEMU, on macOS and on the
Ubuntu x86_64 runner, with nothing large committed.

| Option | For | Against |
|---|---|---|
| A distribution kernel, pinned and checked (chosen) | 10 MB, one `curl`, the same bytes on both hosts, no toolchain, an unmodified kernel with the stock EFI stub | Needs the network the first time (then it is cached); trusts the distribution's build, and the pin is only as good as where it was taken from; a distribution may one day drop the file |
| Build a minimal kernel from source | The configuration is ours; no third-party binary | Needs an aarch64 cross toolchain on both hosts and 10 to 30 minutes of CPU the first time (the CI runner has no cache for it); a build that differs by compiler version; a larger gate with more to break |

Chosen: the `vmlinuz-virt` of **Alpine Linux 3.24.4 netboot** for aarch64
(Linux 6.18.60-0-virt), SHA-256
`cd05fed9e617b5df93dc62ece9b78c422fd94e9b432a2f7c092714b139d5eef4`, 10,412,544
bytes, from `dl-cdn.alpinelinux.org/alpine/v3.24/releases/aarch64/netboot-3.24.4/`.

- **Where the pin comes from.** Alpine publishes a SHA-256 only for the whole
  netboot tarball (`alpine-netboot-3.24.4-aarch64.tar.gz.sha256`:
  `9d10f909...745e`). The tarball was downloaded once (433 MB), its hash was
  compared with that file's, and `boot/vmlinuz-virt` was extracted from it and
  hashed: the value above. The gate never fetches the tarball. The PGP
  signature next to it was not verified.
- **Reproducible.** The URL names a release, and Alpine keeps old releases on
  its mirror (3.20.0 and 3.23.0 answered 200 on the same day). If it ever goes,
  `scripts/fetch-test-kernel.sh` fails and says so; update the pin in the same
  change as the test strings that read the kernel's release.
- **Cache.** `${XDG_CACHE_HOME:-$HOME/.cache}/workspace-os/`; the file is checked
  with `hashlib` (macOS and Debian differ in `shasum` and `sha256sum`) every
  time, not only when downloaded. `WSOS_LINUX_KERNEL=/path` points at a file
  for a machine without network; it is checked too.
- **Never silent.** A download that fails or a hash that differs ends the script
  with an error and so the gate. There is no switch that skips the Linux boots.
- **Licence.** The kernel is GPL-2.0, like this project, and a test input: it is
  fetched, never committed or redistributed by this repository.

## Boots added to the gate

All in `scripts/boot-smoke-test.sh`, on QEMU `virt` with EDK2, driving the menu
through the serial console. The assertions were confirmed by mutation
(the pull request lists each mutation and what it made fail), except the paths
named under "Not in the gate" in the limits below.

| Boot | What it proves |
|---|---|
| `uefi-linux` | Entry 3 starts Linux: `Linux version 6.18.60-0-virt`, a machine line, a `Kernel command line:` equal to the entry's text (so `LoadOptions` arrive whole), no `Freeing initrd memory`, and none of our own kernel's lines |
| `uefi-linux-initrd` | The same with an initrd of 1,003,520 bytes (a cpio archive, 245 whole pages, with an `/init` that is not a program): the kernel prints `Freeing initrd memory` with exactly that many KiB, no unpacking failure, and `Failed to execute /init`, which only an initrd that arrived intact can show; with no initrd it says there is no `/init` |
| `uefi-others` | Six refusals in a row on one boot, each with its own message and a menu that comes back with no countdown (`back to the menu`, `autoboot stopped`), none starting anything; then the chainload of `chainprobe` (started, device handle set, the right file path, `returned SUCCESS`) and the menu again; then `1` starts our kernel |
| `uefi-bad-config` | A `\WSOS.CFG` with a label the console cannot print (a character outside the Basic Multilingual Plane, which panicked the bootloader before it was refused) on line 3: the line is named, only Normal and Recovery are listed, and the countdown still starts Normal |
| `uefi-with-config` | A good file: the entry is listed third and Normal still starts by the timeout |

The initrd archive is generated by the script (no download), and the
kernel's `Freeing initrd memory` figure is exact only for a size that is a whole
number of pages, which is why it is one.

## Limits

- Only the EFI system partition the bootloader started from (see above).
- Only EDK2 under QEMU was run. VirtualBox's firmware and real machines were
  not, and neither was a Linux boot with ACPI (the bootloader is for our
  kernel's device-tree boot; Linux gets whatever the firmware publishes, which
  under `acpi=off` is the device tree).
- Linux with the kernel's own `initrd=` command-line option was not tested;
  the supported way is the `initrd` key.
- The gate's Linux boots end in a kernel panic for lack of a root filesystem,
  by design; what is checked is the start.
- Not in the gate: everything that runs after an image *returns* to the
  bootloader with Linux (it always ends in a panic, so none does): removing the
  initrd protocol, freeing the initrd, the unload after a refused start; the
  freeing of the image's buffer on a refusal (nothing reads memory back); a
  `linux` entry with no `cmdline` (no load options set); a refusal for an empty
  file, a chainloaded loader that
  returns an error status (same code path as the success the gate runs), and
  that the padding file of the initrd arrived intact (the `/init` and the
  archive's headers did, as the kernel found them).
- `LoadImage` that answers `SECURITY_VIOLATION` loads the image without starting
  it; the `uefi` crate drops that handle, so the loaded copy stays until the
  machine resets. Only a start that fails that way is unloaded here; the status alone cannot tell
  that from an image that ran and returned the same status, and a firmware that
  does not validate handles would then see a second unload of a finished image
  (EDK2 answers `INVALID_PARAMETER`).
- If the firmware refused to uninstall the initrd protocol after Linux's stub
  returned, a second Linux entry could find the stale handle. The stub reads
  the protocol with `HandleProtocol`, which does not block an uninstall, and
  EDK2 uninstalls it here; no firmware that refuses was seen.
- The firmware's watchdog (five minutes) is armed again when an entry is chosen,
  so an interactive loader left idle that long is reset, as for any boot
  application.
- A Linux boot under emulation on the CI runner is slower than on Apple
  silicon; the job's timeout was raised for it.
