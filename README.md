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

Next milestone: device tree, memory allocation and the MMU, the base for
isolated processes. The full plan is in [docs/roadmap.md](docs/roadmap.md).

## Requirements

- `rustup`. The Rust version, target and components are pinned in
  `rust-toolchain.toml` and installed automatically.
- QEMU with `qemu-system-aarch64`

## Usage

```bash
git config core.hooksPath .githooks   # once per clone: enables the git hooks
cargo build                           # build the kernel
cargo run                             # boot it in QEMU (exit with Ctrl-A then X)
./scripts/check.sh                    # format, lint and boot smoke test
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
| `kernel/src/board.rs` | Board description (QEMU `virt`) |
| `kernel/src/drivers/` | Device drivers that live in the kernel for now |
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

## Licence

[GNU General Public License v2.0 only](LICENSE) (`GPL-2.0-only`).
