---
title: "Booting the disk image"
type: guide
status: active
created: 2026-11-16
updated: 2026-11-19
tags: [boot, bootloader, uefi, qemu, virtualbox, aarch64, linux]
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
bootloader stops with "the firmware offers ACPI only". A good boot prints the
bootloader's banner and summary, then the kernel's banner and
`entry: bootloader, boot info 1.0` (the gate checks the details).

## The boot menu

The bootloader stops on a text menu before it loads anything (ADR 0007,
ADR 0008). It is plain text on the firmware console, which is the serial line
as well as the screen:

```
  boot  info  boot menu: arrows and Enter, or a number from 1 to 9
        > 1  Normal
          2  Recovery
  boot  info  autoboot in 3 s, starting Normal
```

- **Normal** starts the kernel with `boot_mode` 0, **Recovery** with 1 (ADR
  0013). The kernel reports the mode it received in its `entry: bootloader`
  line, for example `mode 1 (recovery)`. What recovery does once it runs is
  K13's work; today the same kernel starts with the other mode.
- **Up** and **Down** move the selection and **Enter** starts it. A digit starts
  the entry with that number (1 to 9) at once.
- Any key stops the countdown, so the menu then waits for a person. With no
  key, Normal starts after three seconds, so an unattended machine boots.
- A key pressed before the menu is drawn is dropped, so it cannot choose an
  entry nobody saw: press it once the menu is on the console.
- A byte on the console that nobody means (a terminal server announcing
  itself, say) also stops the countdown when it is a valid character, and then the machine waits for
  a person. An unattended machine on a noisy serial line therefore needs the
  firmware's boot order, not this menu. The firmware's watchdog is off while
  the menu waits and is armed again once an entry is chosen.
- The menu only appends lines to the console. The graphical menu (K2e) draws
  next to it and does not replace it, because the gate reads this one.

The list of entries is data (`bootinfo::menu::BUILT_IN` and `\WSOS.CFG`, below), and the menu logic
(`bootinfo/src/menu.rs`) does not know what an entry means, so a list made by
the boot model (ADR 0009) takes its place without changing the menu.

To type on the console of a QEMU run, use `-nographic` as below: the terminal
you start QEMU in is the serial console. `./scripts/check.sh` types on the
serial line through a socket instead, once the menu has been drawn, and starts
each entry in turn.

## Other systems: Linux and chainloading

Besides our kernel the menu can start Linux and the EFI loader of another
system (K2d, ADR 0008, ADR 0025). Their entries are written by hand in
`\WSOS.CFG`, a text file in the root of the EFI system partition. Without it
the menu holds Normal and Recovery only; with it the entries follow them,
numbered from 3 (seven at most, so each has a digit).

```
# a line that starts with a hash is a comment
entry Alpine Linux
linux \VMLINUZ
cmdline console=ttyAMA0 earlycon
initrd \INITRD.IMG

entry Another system
chainload \EFI\OTHER\BOOTAA64.EFI
```

- `entry <label>` opens an entry; `linux`, `cmdline`, `initrd` and `chainload`
  belong to it. An entry has `linux` or `chainload`, not both; `cmdline` and
  `initrd` only go with `linux`. A path is on the EFI system partition, starts
  with a backslash and has no `.` or `..` part. The command line is printable
  ASCII, at most 1023 characters.
- A file with an error does not stop the boot: the bootloader prints the line
  (`\WSOS.CFG, line 3: unknown key: the built-in entries only`) and shows
  Normal and Recovery only.
- Before it starts an entry the bootloader reads the image and checks that it
  is a PE image for aarch64 with the EFI application subsystem, and that the
  initrd it names exists. Any failure prints one line
  (`cannot start Alpine Linux: Linux image \VMLINUZ is not on the EFI system
  partition`, `... is not a valid EFI image: a PE image for x86_64 (machine
  0x8664), not aarch64`, `initrd \INITRD.IMG is not on ...`) and the menu
  comes back **with no countdown**, so a failing default is never retried by
  itself.
- A chainloaded loader that returns (a declined boot, an error) also brings the
  menu back, with the status it returned.

### Try Linux by hand under QEMU

`make-disk-image.py` takes `--file \ESP\PATH=LOCAL` for every extra file;
names must be upper-case 8.3 (`VMLINUZ`, `INITRD.IMG`), the only ones it writes. The
Linux kernel the gate uses is a pinned Alpine release; the script fetches and
checks it, and prints where the cached file is:

```bash
linux="$(./scripts/fetch-test-kernel.sh)"
printf 'entry Alpine Linux\nlinux \\VMLINUZ\ncmdline console=ttyAMA0 earlycon\n' > /tmp/WSOS.CFG
cargo build -p kernel && cargo build -p bootloader --target aarch64-unknown-uefi
python3 -I scripts/make-disk-image.py target/linux.img \
  target/aarch64-unknown-uefi/debug/bootloader.efi \
  target/aarch64-unknown-none-softfloat/debug/kernel \
  --file '\WSOS.CFG=/tmp/WSOS.CFG' --file "\\VMLINUZ=$linux"
qemu-system-aarch64 -machine virt,acpi=off -cpu cortex-a72 -m 512M -nographic \
  -bios "$(brew --prefix)/share/qemu/edk2-aarch64-code.fd" \
  -drive file=target/linux.img,format=raw,if=virtio
```

Press `3` on the menu. Linux prints its first lines and, with no root
filesystem, ends in `Kernel panic ... Unable to mount root fs`: that is the
expected end of this test. Add `initrd \INITRD.IMG` to the entry and an image
(`--file '\INITRD.IMG=...'`) to give it an initial ramdisk; the kernel then
prints `Freeing initrd memory`. To chainload, add
`chainload \EFI\TEST\PROBE.EFI` and put `chainprobe.efi` from
`target/aarch64-unknown-uefi/debug/` there: it says where it was loaded from
and returns to the menu.

Sources and the reasons for each choice are in `docs/k2d-other-kernels.md`.

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
`>>`, `fail`), then a summary box; the kernel draws its own box and logs lines
that start with `kernel` and the same kind of tag. The kernel's `entry:
bootloader, boot info 1.0, mode 0 (normal)` line shows how it was entered and
which entry was chosen, and `booted on aarch64 in EL1` is the
banner. Leave QEMU with Ctrl-A then X.

Add `-accel hvf -cpu host` to run with the hypervisor instead of emulation.
It boots through the EL1 path (tried by hand in K2b-3, with a start at EL2 it
fails: the host does not allow nested virtualization); the gate uses
emulation.

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

> **Since K6** the kernel drives VirtualBox's GICv3 (ADR 0035) and counts timer
> ticks here: the serial file shows `interrupts: gicv3, distributor 0xfcd30000,
> redistributors 0xfcd40000`, `timer: virtual, 24000000 Hz` and 20 ticks.
> Run with the commands below on 2026-11-26. The first K6 build drove a GICv2
> only and stopped here with `the controller is a GICv3, which the kernel does not
> drive`; that is not what this section's image does any more.

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
  the kernel can be loaded elsewhere: the higher half of K4 (ADR 0019) separates
  the virtual address from the physical one but keeps the physical one fixed.
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

## When the bootloader stops

Before it leaves the boot services, the bootloader says why on the serial
console and returns to the firmware after five seconds. A failure after that
point (an invalid final memory map, an exhausted table pool) is written to the
UART and the machine then waits: there is no console left to return to, so it
needs a reset. The messages the gate provokes:

| Message | Cause |
|---|---|
| `\KERNEL.ELF is not on the EFI system partition` | The image has no kernel at that path. |
| `the kernel needs 0xA..0xB and the firmware refused it` | The firmware holds the kernel's load address (VirtualBox with 1024 MiB). The firmware's memory map is printed just before. |
| `the kernel is linked at address 0` | The kernel image asks to be loaded at page zero. |
| `the firmware offers ACPI only (rsdp 0x...)` | ACPI is on and no device tree is published: turn ACPI off. |

The entries of other systems do not end the boot: their refusals are in "Other systems" above and bring the menu back.

## What was not verified

- `-accel kvm` boots. `-accel hvf -cpu host` was run by hand on Apple silicon
  (K2b-3) with the default EL1 start; it cannot start the bootloader at EL2
  there. These runs are not in the gate.
- A real Arm machine.
- UEFI firmware other than QEMU's EDK2 and VirtualBox's.
- The boot menu on VirtualBox. The VirtualBox section above was run before the
  menu existed; its serial port is output only there, so the menu would count
  down and start Normal.
- A key typed on a screen console (a graphical window or a real keyboard). The
  gate types on the serial line only; the same firmware input protocol serves
  both, but only the serial path was run.
