# workspace-os

A study turned into an operating system: a Rust microkernel with models as the
control plane. A small language model is the user-facing intent shell, and
purpose-trained models tune kernel policy, both on top of a small deterministic
core.

## Status

The kernel boots on aarch64 under QEMU, enters EL1 whether it is started at
EL1, EL2 or EL3, and prints a banner on the serial console. Secondary cores are
parked. A synchronous exception taken in the kernel saves the processor
context, is decoded from `ESR_EL1` and, for a breakpoint, returns to the
interrupted code. Any other exception ends in a kernel panic that describes it, as long as
the kernel stack is valid (guard pages come with the MMU).

The kernel reads the whole memory map and the reserved regions from the device
tree and manages physical memory in 4 KiB frames. A UEFI bootloader loads it from a
disk image (QEMU and VirtualBox on Apple silicon) and hands over the memory map.

Next milestone: the MMU, the base for isolated processes. The full plan is in [docs/roadmap.md](docs/roadmap.md).

## Requirements

- `rustup`. The Rust version, target and components are pinned in
  `rust-toolchain.toml` and installed automatically.
- QEMU with `qemu-system-aarch64`, and AArch64 UEFI firmware for the bootloader boot of the gate (Homebrew's QEMU ships it; on Debian and Ubuntu install `qemu-efi-aarch64`)
- `python3`, standard library only: the licence check and the disk image tool use it.

## Usage

```bash
git config core.hooksPath .githooks   # once per clone: enables the git hooks
cargo build                           # build the kernel
cargo run                             # boot it in QEMU (exit with Ctrl-A then X)
./scripts/check.sh                    # format, licences, lint, unit tests and boot smoke test
./scripts/build-disk-image.sh         # bootable UEFI disk image; see docs/booting-the-image.md
./scripts/intent-shell.sh             # the intent shell, on the host (needs llama-server); --plain for the line adapter; --no-model types commands with no model (S5); starts in $HOME, understands paths and "go to ~/Downloads", shows listings as a tree (docs/s3e-directories-and-tree.md)
```

## Checks

`./scripts/check.sh` is the single gate for a change. The hooks in `.githooks/`
run it before every commit and merge commit, and CI runs it on every pull
request and on `main`.

The hooks refuse to run while the working tree has unstaged changes or
untracked files, because the checks read the working tree. Rebases and
cherry-picks do not run the hooks; CI covers those.

`main` is protected: changes land through a pull request with the CI check
passing.

## Layout

| Path | Purpose |
|---|---|
| `kernel/src/main.rs` | Kernel entry point and panic handler |
| `kernel/src/arch/` | Architecture-specific code (boot assembly, exception vectors, linker script) |
| `kernel/src/memory_image.rs` | Boot-time self-test of `.bss` and `.data` |
| `kernel/src/memory.rs` | Builds the frame allocator from the device tree and the kernel layout; boot-time self-test |
| `physmem/` | Allocation-free physical frame allocator with host unit tests |
| `kernel/src/board.rs` | Reads memory and UART from the device tree; QEMU `virt` fallback location of the blob |
| `devicetree/` | Minimal, allocation-free device tree reader with host unit tests |
| `kernel/src/drivers/` | Device drivers that live in the kernel for now |
| `bootloader/` | UEFI application: boot menu, loads the kernel from the EFI system partition and enters it with a `BootInfo`, and starts Linux or another system's EFI loader from the entries of `\WSOS.CFG` (built for `aarch64-unknown-uefi`) |
| `bootinfo/` | The handoff structure, memory map conversion, ELF placement, identity map page tables, the boot menu logic, the entry file parser and the PE check, shared by bootloader and kernel, with host unit tests |
| `docs/adr/` | Architecture decision records |

## Decisions

- [ADR 0001: Own microkernel written in Rust](docs/adr/0001-own-microkernel-in-rust.md)
- [ADR 0002: Models as control plane over a deterministic core](docs/adr/0002-models-as-control-plane.md)
- [ADR 0003: Model-assisted driver conversion and package registry](docs/adr/0003-driver-conversion-and-package-registry.md)
- [ADR 0004: aarch64 as the first target architecture](docs/adr/0004-aarch64-first.md)
- [ADR 0005: GPL-2.0 licence](docs/adr/0005-gpl2-licence.md)
- [ADR 0006: System persona, the SOUL and the voice, defined at installation](docs/adr/0006-system-persona-soul-and-voice.md)
- [ADR 0007: Linux-style user permissions over kernel capabilities](docs/adr/0007-linux-style-user-permissions.md)
- [ADR 0008: Own UEFI bootloader, written with Limine as the reference](docs/adr/0008-own-uefi-bootloader.md)
- [ADR 0009: A small model in the bootloader detects and decides how to load kernels](docs/adr/0009-boot-model-detects-kernels.md)
- [ADR 0010: Own minimal device tree reader, with no allocation](docs/adr/0010-own-device-tree-reader.md)
- [ADR 0013: Boot handoff structure between the bootloader and the kernel](docs/adr/0013-boot-handoff-structure.md)
- [ADR 0017: MPL-2.0 dependencies in a GPL-2.0-only system](docs/adr/0017-mpl2-dependencies.md)

## Licence

[GNU General Public License v2.0 only](LICENSE) (`GPL-2.0-only`).
