---
title: "K2b research: what the boot environments provide and how Limine hands over"
type: research
status: done
created: 2026-11-15
updated: 2026-11-15
tags: [research, boot, bootloader, uefi, virtualbox, qemu, limine, rust]
---

# K2b research: what the boot environments provide and how Limine hands over

Input to ADR 0012 (the handoff between bootloader and kernel). Everything here
was read in this session on 2026-11-15. Source links are pinned to the commit
that was current when read, so they keep pointing at the text that was read.
Anything not read is marked **unverified**.

Pinned commits:

| Repository | Commit |
|---|---|
| VirtualBox (`VirtualBox/virtualbox`, mirror of Oracle's source) | `b6bea72111fcff48f31c57c9f66365b1d3301bb9` |
| Limine (`limine-bootloader/limine`) | `7b8ca7dc2c8685e042c1f1bfa25cd3db4f7d54ee` |
| Limine protocol (`limine-bootloader/limine-protocol`) | `877c2d4c50e170575f8b8e7fbf7b8fa89b1dc157` |
| EDK2 (`tianocore/edk2`) | `8628578dacd38b327d4f9acf820c6914f4b3a48f` |
| `uefi-rs` (`rust-osdev/uefi-rs`) | `c214394e721946fc01d313961bab9356001586ad` |
| `r-efi` | `c0371abfefa6e779867c378f62f750747e320ca9` |

Short names used below for the most cited files:

- `vbox-config`: [`src/VBox/Main/src-client/ConsoleImplConfigArmV8.cpp`](https://github.com/VirtualBox/virtualbox/blob/b6bea72111fcff48f31c57c9f66365b1d3301bb9/src/VBox/Main/src-client/ConsoleImplConfigArmV8.cpp)
- `vbox-desc`: [`include/VBox/platforms/vbox-armv8.h`](https://github.com/VirtualBox/virtualbox/blob/b6bea72111fcff48f31c57c9f66365b1d3301bb9/include/VBox/platforms/vbox-armv8.h)
- `limine-proto`: [`PROTOCOL.md`](https://github.com/limine-bootloader/limine-protocol/blob/877c2d4c50e170575f8b8e7fbf7b8fa89b1dc157/PROTOCOL.md)

## 1. VirtualBox on an Apple Silicon Mac

### What is documented

- Arm guests run on Arm hosts only; an x86 guest on an Arm host is not
  supported ([User Guide 7.2, "Arm Host Limitations"](https://download.virtualbox.org/virtualbox/7.2.18/UserManual.pdf),
  section 2.1.2). The guest table for Arm64 hosts lists Windows 11 as the
  supported guest under Premier Support (section 1.3.3). Nothing in the
  manual lists a custom OS; booting ours is outside what Oracle documents.
- "All Arm VMs require UEFI" (same manual, section 5.22). Firmware is chosen
  with `VBoxManage modifyvm "VM name" --firmware efi`.
- Release 7.2.0 notes: "Arm VMs: ACPI now available" and "Removed IO-APIC from
  Arm VM settings"; release 7.2.10: "Devices/EFI: Fixed booting issue when ARM
  VM had less than 1024 MiB of RAM assigned"
  ([Changelog 7.2](https://www.virtualbox.org/wiki/Changelog-7.2)).
- **Serial ports.** The manual says: "Serial ports are not available on Arm
  VMs" (section 5.18). The source contradicts it, see below. This is the most
  important discrepancy for the gate and has to be tried by hand.
- Disk images: VirtualBox runs VDI, VMDK and VHD disks. A raw disk image
  is converted with `VBoxManage convertfromraw <input> <output> --format=VDI|VMDK|VHD`
  (section 15.13 of the same manual). So the build output is a raw GPT image,
  and a conversion step produces what VirtualBox attaches.

### What the source shows (the machine itself)

All from `vbox-config` unless another file is named. This is the code that
builds the virtual Arm machine, so it is stronger evidence than the manual,
but it is a snapshot of `main`: a released VirtualBox can differ.

- **Firmware.** The machine always uses the EDK2 build `VBoxEFI-arm64.fd`
  (`eFwType = FirmwareType_EFI64` is hard-coded; a comment notes the firmware
  type is not yet queried). It is a ROM at physical address 0, 64 MiB, followed
  by a 64 MiB window with a CFI flash for NVRAM at `0x04000000`.
- **RAM.** "The base RAM will start at 128MiB" (`0x08000000`) and go up to
  4 GiB minus 512 MiB; RAM beyond that is a second region at 4 GiB. This is
  **not** QEMU's layout, where RAM starts at `0x40000000`.
- **Hardware description.** Both an FDT and, when `ACPIEnabled` is set in the
  VM's firmware settings, ACPI tables are generated. They live with a 64 KiB
  descriptor at the very end of the 4 GiB space, in a 2 MiB ROM region
  (`vbox-desc`, `VBOXPLATFORMARMV8_PHYS_ADDR = 4GiB - 64KiB`). The descriptor
  has a magic (`V8OX`), a version and the offsets of the FDT, the ACPI tables,
  the XSDP, the RAM, the MMIO regions and the firmware ROM.
- **Interrupt controller and CPUs.** GICv3 (`ArchRev = GICV3`), optionally with
  an ITS; PSCI with the `hvc` conduit (kernel start and `cpu_on` for K11);
  generic timer; a 24 MHz fixed clock node.
- **Serial.** An `arm-pl011` device exists, but only for serial port 0 and
  only if that port is enabled in the VM settings
  (`serialPort->COMGETTER(Enabled)`); the base address is allocated by a
  resource manager, so it is not fixed. The FDT node is `pl011@<base>` with
  `compatible = "arm,pl011", "arm,primecell"`, and the device is added to the
  ACPI tables too. A `chosen` / `stdout-path` node exists but is inside `#if 0`,
  so **no `stdout-path` is provided**: the kernel has to find the UART by
  `compatible`, as `board.rs` already does.
- **Other devices:** PL031 RTC, PL061 GPIO, a TPM at `0xfed40000`, a PCIe ECAM
  root bus, `qemu-fw-cfg` (with ramfb) in some configurations.
- **Boot disk.** The firmware is EDK2, so it follows the UEFI default boot
  path (section 5 below).

### Does the firmware hand the FDT to the OS? **Not established.**

- The firmware reads the FDT from the descriptor and keeps it in a HOB
  ([`PlatformPeiLib.c`](https://github.com/VirtualBox/virtualbox/blob/b6bea72111fcff48f31c57c9f66365b1d3301bb9/src/VBox/Devices/EFI/Firmware/ArmVirtPkg/Library/PlatformPeiLib/PlatformPeiLib.c)).
  It installs the ACPI tables from the XSDP
  ([`VBoxAcpiPlatform.c`](https://github.com/VirtualBox/virtualbox/blob/b6bea72111fcff48f31c57c9f66365b1d3301bb9/src/VBox/Devices/EFI/Firmware/VBoxPkg/VBoxAcpiPlatformDxe/VBoxAcpiPlatform.c)).
- I did not find which driver, if any, publishes the FDT to the OS as an EFI
  configuration table (`EFI_DTB_TABLE_GUID`) on this firmware, nor whether it
  is published while ACPI is on. The VirtualBox firmware's platform
  description file for Arm could not be resolved to the drivers it includes.
- I did not check whether the ACPI tables VirtualBox builds include an SPCR
  table (the ACPI description of the console UART).
- **Consequence:** the bootloader must look for both an FDT configuration table
  and an ACPI RSDP, and the first job of the code part of K2b is to print the
  configuration table GUIDs under each firmware. The kernel today reads only
  the device tree (ADR 0010).

## 2. QEMU `virt` with EDK2

- Machine: [QEMU `virt` documentation](https://www.qemu.org/docs/master/system/arm/virt.html).
  Flash at `0x0`, "RAM starts at `0x40000000`", PL011 UART(s), GIC selectable
  by `gic-version`, RTC, fw_cfg. The machine generates a device tree; ACPI is
  controlled by `acpi=on|off|auto`. The page warns that device addresses may
  change between QEMU versions, so guests should read the device tree.
- Firmware here: Homebrew's QEMU 11.1.3 ships EDK2 builds
  (`share/qemu/edk2-aarch64-code.fd`, found on this machine).
- **ACPI or device tree.** EDK2's QEMU platform decides in
  [`PlatformHasAcpiDtDxe.c`](https://github.com/tianocore/edk2/blob/8628578dacd38b327d4f9acf820c6914f4b3a48f/OvmfPkg/PlatformHasAcpiDtDxe/PlatformHasAcpiDtDxe.c):
  it exposes ACPI when QEMU provides the fw_cfg file `etc/table-loader`
  (and `PcdForceNoAcpi` is false); otherwise it exposes the device tree. The
  comment in that file notes the OS would be unbootable if neither is
  exposed. It is used by
  [`ArmVirtQemuFvMain.fdf.inc`](https://github.com/tianocore/edk2/blob/8628578dacd38b327d4f9acf820c6914f4b3a48f/ArmVirtPkg/ArmVirtQemuFvMain.fdf.inc).
  So with default QEMU options the firmware exposes ACPI, and the device tree
  path is the exception. What I did **not** establish: whether the DTB is
  also published as a configuration table in the ACPI case, and whether
  `-machine virt,acpi=off` removes `etc/table-loader` (**unverified**,
  plausible). The code part has to try both.
- **Serial.** PL011. EDK2's `ArmVirtPkg` code reads the UART address from the
  device tree by looking for `compatible = "arm,pl011"` (a call to
  `FdtSerialGetPorts` in the ArmVirtPkg platform library, seen in a search
  result of the pinned tree; the exact file was not noted).
- **Direct kernel load (the existing gate).** QEMU puts the DTB at the start
  of RAM when the kernel image does not overlap it; this is what ADR 0010 and
  `link.ld` rely on. It does not apply when a UEFI firmware starts us.

## 3. How Limine does each step

Source: [`limine-proto`](https://github.com/limine-bootloader/limine-protocol/blob/877c2d4c50e170575f8b8e7fbf7b8fa89b1dc157/PROTOCOL.md)
for the contract, and the bootloader's own files at the pinned commit for the
mechanism.

### Mechanism, in UEFI

1. **Memory map.** [`common/mm/pmm.s2.c`](https://github.com/limine-bootloader/limine/blob/7b8ca7dc2c8685e042c1f1bfa25cd3db4f7d54ee/common/mm/pmm.s2.c),
   `init_memmap`: calls `GetMemoryMap` once with a tiny buffer to learn the size,
   allocates with slack, calls it again, converts the firmware's
   descriptors into its own fixed-size entries, and checks the descriptor size
   it was given, because that is what the walk strides by. It allocates the
   kernel and its structures from this map, not from firmware pools.
2. **Leaving boot services.** [`common/lib/misc.c`](https://github.com/limine-bootloader/limine/blob/7b8ca7dc2c8685e042c1f1bfa25cd3db4f7d54ee/common/lib/misc.c),
   `efi_leave_boot_services`: fetches the memory map and calls
   `ExitBootServices` with its key; if the call fails because an allocation
   changed the map, it fetches again and retries; after success it masks
   interrupts and rebuilds its own copy of the map, because after the exit no
   firmware call that allocates is allowed.
3. **Device tree.** Same file: if no `dtb_path` is configured, it scans
   `gST->configuration_table` for `EFI_DTB_TABLE_GUID`, copies the blob into
   its own allocation and later removes the `memory@` nodes from it
   ([`limine.c`](https://github.com/limine-bootloader/limine/blob/7b8ca7dc2c8685e042c1f1bfa25cd3db4f7d54ee/common/protos/limine.c),
   DTB feature), because the memory map is the authority.
4. **Kernel load.** [`common/protos/limine.c`](https://github.com/limine-bootloader/limine/blob/7b8ca7dc2c8685e042c1f1bfa25cd3db4f7d54ee/common/protos/limine.c)
   loads an ELF, applies a slide when it is relocatable, builds the page
   tables, and for aarch64 enters through `enter_in_el1` / `enter_in_el2`.
5. **Where the kernel lives.** In the protocol the kernel image is physically
   contiguous, mapped in the higher half at or above `0xffffffff80000000`;
   there is no fixed physical address.

### What the protocol hands over (aarch64)

- **Requests and responses.** The kernel embeds request structures (ID,
  revision, a response pointer) in its image; the loader scans the image for
  them and fills the response pointers, leaving a pointer `NULL` if a request
  is unsupported. A **base revision** tag (0 to 6 at the pinned commit)
  covers behaviour outside any request; a loader that does not support the
  requested revision leaves the tag untouched, and the kernel has to fail
  itself. Revisions 0 to 5 are deprecated.
- **Memory map.** Entries are `{base, length, type}`, sorted, with types
  Usable, Reserved, ACPI reclaimable, ACPI NVS, Bad memory, Bootloader
  reclaimable, Executable and modules, Framebuffer and (base revision 4 and
  later) Reserved-mapped. Usable and bootloader-reclaimable entries are
  4 KiB aligned and never overlap anything. EFI types map as: LoaderCode/Data
  and BootServicesCode/Data to bootloader-reclaimable, ConventionalMemory to
  usable, ACPIReclaim and ACPINVS to their own types, runtime services to
  reserved(-mapped), anything else to reserved.
- **Device tree and ACPI.** The DTB feature returns a pointer, in
  bootloader-reclaimable memory, without `memory@` nodes (and `/chosen` should
  be ignored). The RSDP feature returns an address and gives no response when
  ACPI is absent.
- **State at entry (aarch64).** Little-endian AArch64, at EL1, or from base
  revision 6 at EL2 with VHE when the loader runs there. All of `DAIF` masked.
  **MMU and caches on**; 4 KiB granule; `TTBR1_EL1` holds the loader's higher-half
  tables, mapping the kernel and a direct map of memory; `MAIR_EL1.Attr0` is
  Normal write-back; `VBAR_EL1` undefined; `SP` at the top of a stack of at
  least 64 KiB in bootloader-reclaimable memory; all general registers zero;
  `X30` is zero. Boot services are exited.
- **Other features that exist** and that we may need later: framebuffer, EFI
  system table, EFI memory map, SMBIOS, modules (initial ramdisk), command
  line, MP (secondary cores), entropy, bootloader performance.

### What Limine gives that we would have to build or give up

- Needs: a kernel in the higher half with its own request section; page tables
  from the loader; the kernel must run with the MMU on from its first
  instruction. Our kernel does not do this (K4 is where it gets an MMU).
- Gives: a tested memory map conversion, a tested exit sequence, MP bring-up,
  framebuffer, modules and a proven ABI.

## 4. The kernel as it is today

From `kernel/src/arch/aarch64/boot.s`, `link.ld` and `board.rs`:

- Entered with the MMU off; sets up its own `SCTLR_EL1` assuming "the MMU and the
  caches are off" and cleans nothing.
- Linked at fixed `0x40200000` ("2 MiB above the start of RAM" on QEMU).
- `x0` is saved in `x19`; the code overwrites `x1` and `x2` afterwards.
- **Parks every core whose MPIDR affinity is not zero.** Under UEFI only the
  firmware's boot core arrives at the kernel and nothing says its affinity is 0.
- `board::discover` tries `x0`, then the start of RAM at `0x40000000`. That
  fallback is wrong on VirtualBox, where RAM starts at `0x08000000`.
- Memory size comes from the device tree `memory` node; under UEFI the
  authoritative source is the firmware memory map (the DTB may include
  regions that firmware services or ACPI occupy).

Consequences for the VirtualBox target, derived from the facts above:

- `0x40200000` is RAM in VirtualBox only if the VM has at least about
  `0x40200000 - 0x08000000 = 0x38200000` bytes (about 898 MiB) of RAM, and
  EDK2 may already use it. This matches the 7.2.10 fix for VMs below 1024 MiB
  as a sign that small Arm VMs are fragile; a fixed address is a real risk.

## 5. Entering the kernel: the documented contract for Linux

[`Documentation/arch/arm64/booting.rst`](https://raw.githubusercontent.com/torvalds/linux/master/Documentation/arch/arm64/booting.rst)
(read on `master` on 2026-11-15; not pinned):

- `x0` is the physical address of the DTB; `x1`, `x2`, `x3` must be zero.
- The MMU must be off. The image's address range must be cleaned to the PoC;
  the instruction cache may be on or off but must hold no stale entries for
  the image.
- All interrupts masked. Non-secure, at EL2 (recommended) or EL1, all CPUs at
  the same level.
- The image is placed at `text_offset` from a 2 MiB aligned base in usable RAM.

This is the de facto standard way to enter an aarch64 kernel with the MMU off.
It is what `boot.s` already assumes and what any Linux we load in K2d expects;
it also means the bootloader will have to turn the MMU off for Linux anyway.

## 6. UEFI state, disk layout and default path

- **Default path.** The UEFI specification (section 3.5.1.1) names the fallback
  loader on removable media `\EFI\BOOT\BOOT<arch>.EFI`; `BOOTAA64.EFI` on
  aarch64 and `BOOTX64.EFI` on x86_64. I could not open uefi.org (HTTP 403).
  This comes from search results quoting the specification and from
  Limine's own usage notes, which tell users to place `BOOTX64.EFI` and
  `BOOTAA64.EFI` in `/EFI/BOOT` of a FAT-formatted system partition
  ([`USAGE.md`](https://github.com/limine-bootloader/limine/blob/7b8ca7dc2c8685e042c1f1bfa25cd3db4f7d54ee/USAGE.md)).
  Treat the exact specification text as **unverified**; Limine's practice is
  verified.
- **Disk.** A GPT disk with an EFI system partition (FAT, partition type GUID
  `C12A7328-F81F-11D2-BA4B-00A0C93EC93B`; **unverified**, from memory)
  holding `\EFI\BOOT\BOOTAA64.EFI` and our kernel file. For VirtualBox the raw
  image goes through `VBoxManage convertfromraw`.
- **CPU state while the application runs (AArch64).** UEFI runs applications
  with the MMU and caches on and memory identity-mapped; the boot services
  also own the interrupt state. **Unverified**: I could not open the
  specification chapter, and this is the claim `boot.s`'s comment contradicts
  ("the boot protocol guarantees that they were off already"). The comment
  describes direct loading and says nothing about UEFI. The design must not
  depend on the exact state: the bootloader turns the MMU off itself.

## 7. Rust for a UEFI application

- Targets `aarch64-unknown-uefi` and `x86_64-unknown-uefi` (and
  `i686-unknown-uefi`) are tier 2; PE32+ output, `no_std` through
  cross-compilation, `efiapi` calling convention, `abort` on panic, FP and
  SIMD disabled by default, no dynamic linking, single address space and one
  CPU ([Rust platform support: UEFI](https://doc.rust-lang.org/rustc/platform-support/unknown-uefi.html)).
- Options:

| Option | Licence (read) | Compatible with GPL-2.0-only (ADR 0005)? | Notes |
|---|---|---|---|
| [`uefi`](https://github.com/rust-osdev/uefi-rs/blob/c214394e721946fc01d313961bab9356001586ad/Cargo.toml) (`uefi-rs`) 0.41.0 | workspace `MIT OR Apache-2.0` (a `LICENSE` file is present, as `LICENSE-APACHE` and `LICENSE-MIT`) | Yes, if we elect **MIT**. Apache-2.0 alone is not compatible with GPL-2.0-only. | High-level safe API, boot services, memory map. Its `exit_boot_services` is `unsafe`, retries once on a stale key and returns the final map; afterwards boot services and the crate's logger and allocator are unusable ([docs.rs](https://docs.rs/uefi/latest/uefi/boot/fn.exit_boot_services.html)). Needs Rust 1.91 or later (`rust-version`). |
| [`r-efi`](https://github.com/r-efi/r-efi/blob/c0371abfefa6e779867c378f62f750747e320ca9/Cargo.toml) 7.2.0 | `MIT OR Apache-2.0 OR LGPL-2.1-or-later` | Yes, elect MIT | Raw bindings only: all unsafe, and the protocol wrappers are ours. |
| Own bindings | ours | Yes | Full control, full cost; the structs come from the UEFI specification. |

- Not done here: the licences of the transitive dependencies of `uefi` and of
  `uefi-raw`. This is a requirement for the code session, with the
  results kept next to the manifest.
- The firmware calls a PE entry point with the image handle and the system
  table in registers; nothing in the Rust target hides that.

## 8. What this means for the handoff

1. The two firmwares disagree on RAM base (`0x40000000` against `0x08000000`)
   and the UART base is allocated, so the kernel cannot assume either; it has
   to read both from what the bootloader hands over.
2. The memory map has to come from the firmware, not from the device tree.
3. It is not established that both firmwares publish a DTB under UEFI, and ACPI
   may be the only description. The handoff carries a DTB pointer and an RSDP
   pointer, either may be absent, and the bootloader must say so.
4. On VirtualBox the manual says there is no serial port on Arm, the source says
   there is one if enabled. If it does not work, there is no way to see the
   kernel's output on VirtualBox without a framebuffer (K2e), so the handoff
   should have room to describe one later.
5. The kernel has a fixed load address and parks cores by MPIDR affinity. Both
   conflict with a UEFI start and are handled in the ADR.
6. x86_64 cannot enter long mode with paging off, so the CPU state at entry
   cannot be the same on both architectures. The structure can be the same.
