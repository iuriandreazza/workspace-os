# workspace-os

A study turned into an operating system: a Rust microkernel with models as the
control plane. A small language model is the user-facing intent shell, and
purpose-trained models tune kernel policy, both on top of a small deterministic
core.

## Status

First milestone: the kernel boots on aarch64 under QEMU and prints a banner on
the serial console. Nothing else exists yet.

## Requirements

- Rust (stable) via `rustup`; the target in `rust-toolchain.toml` is installed
  automatically
- QEMU with `qemu-system-aarch64`

## Usage

```bash
cargo build                    # build the kernel
cargo run                      # boot it in QEMU (exit with Ctrl-A then X)
./scripts/boot-smoke-test.sh   # boot and assert the serial banner
```

## Layout

| Path | Purpose |
|---|---|
| `kernel/src/main.rs` | Kernel entry point and panic handler |
| `kernel/src/arch/` | Architecture-specific code (boot assembly, linker script) |
| `kernel/src/board.rs` | Board description (QEMU `virt`) |
| `kernel/src/drivers/` | Device drivers that live in the kernel for now |
| `docs/adr/` | Architecture decision records |

## Decisions

- [ADR 0001: Own microkernel written in Rust](docs/adr/0001-own-microkernel-in-rust.md)
- [ADR 0002: Models as control plane over a deterministic core](docs/adr/0002-models-as-control-plane.md)
- [ADR 0003: Model-assisted driver conversion and package registry](docs/adr/0003-driver-conversion-and-package-registry.md)
- [ADR 0004: aarch64 as the first target architecture](docs/adr/0004-aarch64-first.md)
- [ADR 0005: AGPL-3.0 licence](docs/adr/0005-agpl-licence.md)

## Licence

[GNU Affero General Public License v3.0 only](LICENSE) (`AGPL-3.0-only`).
