#!/usr/bin/env bash
# Builds the kernel and the UEFI bootloader and writes a bootable disk image:
# a GPT disk whose EFI system partition holds the bootloader at
# \EFI\BOOT\BOOTAA64.EFI and the kernel at \KERNEL.ELF. The image is a build
# product, never committed.
#
#   scripts/build-disk-image.sh [OUTPUT]     (default: target/workspace-os.img)
set -euo pipefail

cd "$(dirname "$0")/.."

output="${1:-target/workspace-os.img}"
# Fixed target directory, as in the boot smoke test: with CARGO_TARGET_DIR set
# the files below would be stale.
cargo build --quiet --target-dir target -p kernel
cargo build --quiet --target-dir target -p bootloader --target aarch64-unknown-uefi

mkdir -p "$(dirname "$output")"
python3 -I scripts/make-disk-image.py "$output" \
    target/aarch64-unknown-uefi/debug/bootloader.efi \
    target/aarch64-unknown-none-softfloat/debug/kernel
echo "wrote $output"
