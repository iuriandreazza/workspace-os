#!/usr/bin/env bash
# Prints the path of the Linux kernel the boot smoke test starts through the
# bootloader (K2d), fetching it first if it is not in the cache. The kernel is
# a pinned Alpine Linux release, checked against a SHA-256 kept in this file,
# and is never committed: it lives in the cache directory.
#
#   scripts/fetch-test-kernel.sh
#
# It fails, loudly, if the kernel cannot be had or does not match: the gate
# must not pass without the Linux boot. On a machine with no network, put the
# file in the cache (or point WSOS_LINUX_KERNEL at it); it is checked either way.
#
# Why this kernel: docs/k2d-other-kernels.md, "The Linux kernel of the test".
# The file is the `vmlinuz-virt` of Alpine's netboot tarball, an arm64 Linux
# 6.18.60 with its EFI stub (a gzip "zboot" PE image), 10.4 MB. The same hash
# was checked inside the tarball whose SHA-256 Alpine publishes next to it.
set -euo pipefail

readonly VERSION="3.24.4"
readonly URL="https://dl-cdn.alpinelinux.org/alpine/v3.24/releases/aarch64/netboot-${VERSION}/vmlinuz-virt"
# What the kernel calls itself in its first line, which the boot smoke test reads.
readonly KERNEL_RELEASE="6.18.60-0-virt"
readonly SHA256="cd05fed9e617b5df93dc62ece9b78c422fd94e9b432a2f7c092714b139d5eef4"

cache="${XDG_CACHE_HOME:-$HOME/.cache}/workspace-os"
file="${WSOS_LINUX_KERNEL:-$cache/vmlinuz-virt-alpine-${VERSION}-aarch64}"

# sha256_of FILE: hashlib, because macOS has `shasum` and Debian `sha256sum`.
sha256_of() {
    python3 -I -c 'import hashlib, sys; print(hashlib.sha256(open(sys.argv[1], "rb").read()).hexdigest())' "$1"
}

die() {
    echo "fetch-test-kernel: $1" >&2
    exit 1
}

verified() {
    [[ -f "$file" && "$(sha256_of "$file")" == "$SHA256" ]]
}

if ! verified; then
    [[ -z "${WSOS_LINUX_KERNEL:-}" ]] || die "$file is missing or does not match $SHA256"
    command -v curl > /dev/null || die "curl not found, and $file is not cached"
    mkdir -p "$cache"
    download="$(mktemp "$cache/download.XXXXXX")"
    trap 'rm -f "$download"' EXIT
    echo "fetch-test-kernel: downloading $URL" >&2
    curl --fail --silent --show-error --location --retry 3 --output "$download" "$URL" \
        || die "cannot download $URL (no network? put the file at $file)"
    got="$(sha256_of "$download")"
    [[ "$got" == "$SHA256" ]] || die "$URL has SHA-256 $got, expected $SHA256"
    mv "$download" "$file"
fi
echo "$file"
