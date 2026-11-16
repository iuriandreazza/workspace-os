---
title: "Booting the disk image"
type: guide
status: active
created: 2026-11-16
updated: 2026-11-16
tags: [boot, bootloader, uefi, qemu, virtualbox, aarch64]
---

# Booting the disk image

The UEFI bootloader (`bootloader/`, ADR 0008 and ADR 0013) and the kernel are
packaged in a disk image that firmware can start. This is how to build it and
run it in three places. Every command was run on the machine type named in its
section; the last section lists what was not tried.

## Build the image

```bash
./scripts/build-disk-image.sh                 # writes target/workspace-os.img
```

The script builds the kernel and the bootloader (`aarch64-unknown-uefi`) and
writes a 64 MiB GPT disk with one EFI system partition (FAT32) that holds
`\EFI\BOOT\BOOTAA64.EFI` (the bootloader) and `\KERNEL.ELF`. It needs `cargo`
and `python3` only. The image is a build product and is never committed.

Everything below needs **ACPI off** (`acpi=off` on QEMU, `--acpi off` on
VirtualBox). With ACPI on, both firmwares publish only ACPI tables and no
device tree; the kernel can read only the device tree (ADR 0010), so the
bootloader stops with "the firmware offers ACPI only". The expected serial
output is in `docs/adr/0013-boot-handoff-structure.md`, "Findings of K2b-2".

## The boot menu

The bootloader stops on a text menu before it loads anything (ADR 0007,
ADR 0008). It is plain text on the firmware console, which is the serial line
as well as the screen:

```
  boot  info  boot menu: arrows and Enter, or the number of an entry
        > 1  Normal
          2  Recovery
  boot  info  autoboot in 3 s, starting Normal
```

- **Normal** starts the kernel with `boot_mode` 0, **Recovery** with 1 (ADR
  0013). The kernel reports the mode it received in its `entry: bootloader`
  line, for example `mode 1 (recovery)`. What recovery does once it runs is
  K13's work; today the same kernel starts with the other mode.
- **Up** and **Down** move the selection and **Enter** starts it. A digit starts
  the entry with that number at once.
- Any key stops the countdown, so the menu then waits for a person. With no
  key, Normal starts after three seconds, so an unattended machine boots.
- The menu only appends lines to the console. The graphical menu (K2e) draws
  next to it and does not replace it, because the gate reads this one.

The list of entries is data (`bootinfo::menu::BUILT_IN`), and the menu logic
(`bootinfo/src/menu.rs`) does not know what an entry means, so a list made by
the boot model (ADR 0009) takes its place without changing the menu.

To type on the console of a QEMU run, use `-nographic` as below: the terminal
you start QEMU in is the serial console. `./scripts/check.sh` types on the
serial line through a socket instead, once the menu has been drawn, and starts
each entry in turn.

## QEMU on a Mac with Apple silicon

QEMU and its UEFI firmware come from Homebrew:

```bash
brew install qemu
```

```bash
qemu-system-aarch64 -machine virt,acpi=off -cpu cortex-a72 -m 512M \
  -nographic \
  -bios "$(brew --prefix)/share/qemu/edk2-aarch64-code.fd" \
  -drive file=target/workspace-os.img,format=raw,if=virtio
```

The firmware prints its own lines first. The bootloader draws a banner, the
boot menu and then logs lines that start with `boot` and a tag (`ok`, `info`,
`>>`, `fail`), then a summary box; the kernel draws its own box and logs lines that start with
`kernel` and the same kind of tag. The kernel's `entry: bootloader, boot info
1.0, mode 0 (normal)` line shows how it was entered and which entry was chosen, and `booted on aarch64 in EL1` is the
banner. Leave QEMU with Ctrl-A then X.

Add `-accel hvf -cpu host` to run with the hypervisor instead of emulation.
That combination was not tried; the gate uses emulation.

## QEMU emulating aarch64 on Linux x86_64

```bash
sudo apt-get install --yes qemu-system-arm qemu-efi-aarch64
```

```bash
qemu-system-aarch64 -machine virt,acpi=off -cpu cortex-a72 -m 512M \
  -nographic \
  -bios /usr/share/qemu-efi-aarch64/QEMU_EFI.fd \
  -drive file=target/workspace-os.img,format=raw,if=virtio
```

The output is the same as on the Mac. This is plain emulation (TCG), so the
firmware takes tens of seconds to reach the bootloader on a slow machine.
`./scripts/check.sh` runs this same boot on Ubuntu in CI, with the packages
above, so the packages and the path are the ones CI uses.

## VirtualBox on a Mac with Apple silicon

VirtualBox 7.2 runs Arm guests natively on Apple silicon. This section was run
with VirtualBox 7.2.22.

```bash
brew install --cask virtualbox        # asks for your password; reboot if macOS asks
```

```bash
VBoxManage convertfromraw target/workspace-os.img target/workspace-os.vdi --format VDI
VBoxManage createvm --name workspace-os --platform-architecture arm \
  --ostype Other_arm64 --register
VBoxManage modifyvm workspace-os --memory 2048 --firmware efi --acpi off \
  --graphicscontroller qemuramfb \
  --uart1 0x3f8 4 --uart-mode1 file "$PWD/target/workspace-os-serial.log"
VBoxManage storagectl workspace-os --name Disk --add virtio-scsi
VBoxManage storageattach workspace-os --storagectl Disk --port 0 --device 0 \
  --type hdd --medium target/workspace-os.vdi
VBoxManage startvm workspace-os --type headless
```

Read the output from the file the virtual serial port writes:

```bash
tr -d '\r' < target/workspace-os-serial.log | grep -a 'boot  \|kernel  '
```

Stop and remove the machine with:

```bash
VBoxManage controlvm workspace-os poweroff
VBoxManage unregistervm workspace-os --delete
```

Things that matter, each found by running it:

- **Memory: 2048 MiB.** The kernel is linked at `0x40200000` and the
  bootloader must get exactly that range (ADR 0013, "Load address"). VirtualBox
  puts its RAM at `0x08000000`, and its firmware reserves the top of the first
  part of RAM. With 1024 MiB that reservation covers `0x40200000` and the
  bootloader stops with the firmware's memory map printed and "the kernel's load
  address is not free RAM". 2048 MiB and 4096 MiB leave it free. This lasts until
  the kernel is relocatable (K4).
- **ACPI off** gives a device tree. With it on, VirtualBox's firmware publishes
  only an ACPI table.
- **The serial port is a PL011 at `0xffdde000`**, named by the device tree. The
  `--uart1` option is what makes VirtualBox create it and connect it to a file;
  the I/O base and IRQ values are accepted but the address comes from the
  firmware.
- The bootloader starts at EL1 here.

## Other hosts

- VirtualBox on an x86_64 host runs x86_64 guests only, so it cannot run this
  image. It waits for the x86_64 port (roadmap item K14).
- VirtualBox on Linux x86_64 is therefore not covered; use QEMU there.

## What was not verified

- `-accel hvf` and `-accel kvm` boots.
- A real Arm machine.
- UEFI firmware other than QEMU's EDK2 and VirtualBox's.
