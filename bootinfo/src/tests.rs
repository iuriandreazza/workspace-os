use super::*;
use std::vec::Vec;

fn sample() -> BootInfo {
    let mut info = BootInfo::new();
    info.console_kind = CONSOLE_PL011;
    info.console_base = 0x0900_0000;
    info.boot_mode = MODE_RECOVERY;
    info.arch = ARCH_AARCH64;
    info.boot_cpu = 0x0301;
    info.memory_map = 0x1234_5000;
    info.memory_map_count = 7;
    info.device_tree = 0x4000_0000;
    info.device_tree_size = 0x1000;
    info.rsdp = 0x4fff_0000;
    info.kernel_base = 0x4020_0000;
    info.kernel_size = 0x30_0000;
    info.efi_system_table = 0x4ffe_0000;
    info
}

#[test]
fn layout_matches_the_adr_tables() {
    assert_eq!(size_of::<BootInfo>(), 120);
    assert_eq!(offset_of!(BootInfo, size), 12);
    assert_eq!(offset_of!(BootInfo, console_base), 24);
    assert_eq!(offset_of!(BootInfo, boot_mode), 32);
    assert_eq!(offset_of!(BootInfo, memory_map), 48);
    assert_eq!(offset_of!(BootInfo, memory_map_entry_size), 64);
    assert_eq!(offset_of!(BootInfo, device_tree), 72);
    assert_eq!(offset_of!(BootInfo, efi_system_table), 112);
    assert_eq!(size_of::<MemoryEntry>(), 24);
    assert_eq!(&MAGIC.to_le_bytes(), b"WSOSBOOT");
    assert_eq!(&ENTRY_MAGIC.to_be_bytes(), b"WSOSENTR");
}

#[test]
fn bytes_round_trip() {
    let info = sample();
    let bytes = info.to_bytes();
    assert_eq!(BootInfo::parse(&bytes), Ok(info));
}

#[test]
fn bytes_are_what_memory_holds() {
    // The kernel reads the bootloader's `#[repr(C)]` value from memory, so the
    // serialisation must be the same bytes.
    let info = sample();
    // SAFETY: `BootInfo` is plain integers; viewing its bytes is sound. The
    // reserved fields are zero in `sample`, so padding is not involved.
    let raw: [u8; 120] = unsafe { core::mem::transmute(info) };
    assert_eq!(raw, info.to_bytes());
}

#[test]
fn rejects_what_is_not_a_boot_info() {
    let good = sample().to_bytes();
    assert_eq!(BootInfo::parse(&good[..31]), Err(Error::TooSmall));

    let mut bad = good;
    bad[0] ^= 1;
    assert_eq!(BootInfo::parse(&bad), Err(Error::BadMagic));

    for size in [0u32, 31, 4097, 121] {
        let mut bad = good;
        bad[12..16].copy_from_slice(&size.to_le_bytes());
        assert_eq!(BootInfo::parse(&bad), Err(Error::BadSize), "size {size}");
    }
}

#[test]
fn unknown_major_version_still_names_the_console() {
    let mut bytes = sample().to_bytes();
    bytes[8..10].copy_from_slice(&2u16.to_le_bytes());
    bytes[10..12].copy_from_slice(&5u16.to_le_bytes());
    assert_eq!(
        BootInfo::parse(&bytes),
        Err(Error::UnsupportedVersion {
            major: 2,
            minor: 5,
            console_kind: CONSOLE_PL011,
            console_base: 0x0900_0000,
        })
    );
}

#[test]
fn a_newer_minor_version_and_trailing_fields_are_accepted() {
    let mut bytes = sample().to_bytes().to_vec();
    bytes[10..12].copy_from_slice(&9u16.to_le_bytes());
    bytes.extend_from_slice(&[0xAA; 40]);
    let size = bytes.len() as u32;
    bytes[12..16].copy_from_slice(&size.to_le_bytes());
    let parsed = BootInfo::parse(&bytes).unwrap();
    assert_eq!(parsed.version_minor, 9);
    assert_eq!(parsed.efi_system_table, sample().efi_system_table);
    assert_eq!(parsed.size, 160);
}

#[test]
fn a_field_past_size_takes_its_default() {
    let full = sample();
    // Declared size ends right after `rsdp`: later fields are absent.
    let size = offset_of!(BootInfo, kernel_base);
    let mut bytes = full.to_bytes();
    bytes[12..16].copy_from_slice(&(size as u32).to_le_bytes());
    let parsed = BootInfo::parse(&bytes[..size]).unwrap();
    assert_eq!(parsed.rsdp, full.rsdp);
    assert_eq!(parsed.kernel_base, 0);
    assert_eq!(parsed.kernel_size, 0);
    assert_eq!(parsed.efi_system_table, 0);

    // A field cut in half counts as absent, even though its first bytes are there.
    let size = offset_of!(BootInfo, kernel_base) + 4;
    bytes[12..16].copy_from_slice(&(size as u32).to_le_bytes());
    let parsed = BootInfo::parse(&bytes[..size]).unwrap();
    assert_eq!(parsed.kernel_base, 0);

    // Header only: every body field is absent.
    bytes[12..16].copy_from_slice(&32u32.to_le_bytes());
    let parsed = BootInfo::parse(&bytes[..32]).unwrap();
    assert_eq!(parsed.memory_map_count, 0);
    assert_eq!(parsed.console_base, full.console_base);
}

#[test]
fn no_flip_of_a_byte_panics() {
    let good = sample().to_bytes();
    for index in 0..good.len() {
        for bit in 0..8 {
            let mut bytes = good;
            bytes[index] ^= 1 << bit;
            let _ = BootInfo::parse(&bytes);
        }
    }
}

#[test]
fn from_address_checks_before_reading() {
    // SAFETY: the null and misaligned cases return before any read.
    unsafe {
        assert_eq!(BootInfo::from_address(0), Err(Error::NullPointer));
        assert_eq!(BootInfo::from_address(0x1001), Err(Error::Misaligned));
    }
    #[repr(align(8))]
    struct Aligned([u8; 120]);
    let aligned = Aligned(sample().to_bytes());
    // SAFETY: the buffer is a valid boot info of the size it declares.
    let read = unsafe { BootInfo::from_address(aligned.0.as_ptr() as usize) };
    assert_eq!(read, Ok(sample()));
}

fn entry(base: u64, pages: u64, kind: MemoryKind) -> MemoryEntry {
    MemoryEntry::new(base, pages * PAGE_SIZE, kind)
}

fn map_of(entries: &[MemoryEntry], stride: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    for entry in entries {
        bytes.extend_from_slice(&entry.to_bytes());
        bytes.resize(bytes.len() + stride - ENTRY_SIZE, 0xEE);
    }
    bytes
}

#[test]
fn efi_kinds_follow_the_adr_table() {
    use MemoryKind::*;
    let expected = [
        (0, Reserved),
        (1, BootloaderReclaimable),
        (2, BootloaderReclaimable),
        (3, BootloaderReclaimable),
        (4, BootloaderReclaimable),
        (5, ReservedMapped),
        (6, ReservedMapped),
        (7, Usable),
        (8, Reserved),
        (9, AcpiReclaimable),
        (10, AcpiNvs),
        (11, Reserved),
        (12, Reserved),
        (13, Reserved),
        (14, Reserved),
        (15, Reserved),
        (0x8000_0000, Reserved),
    ];
    for (efi, kind) in expected {
        assert_eq!(kind_from_efi(efi), kind, "EFI type {efi}");
    }
}

fn descriptor(efi_type: u32, start: u64, pages: u64) -> EfiDescriptor {
    EfiDescriptor {
        efi_type,
        physical_start: start,
        pages,
    }
}

#[test]
fn conversion_sorts_merges_and_drops_empty_descriptors() {
    let mut out = [MemoryEntry::default(); 8];
    let n = build_memory_map(
        [
            descriptor(7, 0x4000_4000, 4),
            descriptor(2, 0x4000_0000, 2),
            descriptor(4, 0x4000_2000, 2), // merges with the loader data before it
            descriptor(7, 0x4000_8000, 0), // empty
            descriptor(7, 0x4000_8000, 8), // merges with the conventional memory before it
            descriptor(11, 0x0900_0000, 1),
        ],
        &mut out,
    )
    .unwrap();
    assert_eq!(
        &out[..n],
        &[
            entry(0x0900_0000, 1, MemoryKind::Reserved),
            entry(0x4000_0000, 4, MemoryKind::BootloaderReclaimable),
            entry(0x4000_4000, 12, MemoryKind::Usable),
        ]
    );
}

#[test]
fn conversion_refuses_what_it_cannot_trust() {
    let mut out = [MemoryEntry::default(); 2];
    assert_eq!(
        build_memory_map(
            [descriptor(7, 0x1000, 4), descriptor(7, 0x2000, 4)],
            &mut out
        ),
        Err(BuildError::Overlap)
    );
    assert_eq!(
        build_memory_map([descriptor(7, 0x1001, 1)], &mut out),
        Err(BuildError::BadDescriptor)
    );
    assert_eq!(
        build_memory_map([descriptor(7, 0xffff_ffff_ffff_f000, 2)], &mut out),
        Err(BuildError::BadDescriptor)
    );
    assert_eq!(
        build_memory_map([descriptor(7, 0, u64::MAX)], &mut out),
        Err(BuildError::BadDescriptor)
    );
    assert_eq!(
        build_memory_map(
            [
                descriptor(7, 0x1000, 1),
                descriptor(1, 0x3000, 1),
                descriptor(0, 0x5000, 1),
            ],
            &mut out
        ),
        Err(BuildError::TooManyEntries)
    );
}

#[test]
fn the_kernel_is_carved_out_of_the_entry_that_holds_it() {
    let mut out = [MemoryEntry::default(); 4];
    let base = 0x4000_0000;
    out[0] = entry(base, 0x1000, MemoryKind::BootloaderReclaimable);
    let n = carve_kernel(&mut out, 1, base + 0x200 * PAGE_SIZE, 0x40 * PAGE_SIZE).unwrap();
    assert_eq!(
        &out[..n],
        &[
            entry(base, 0x200, MemoryKind::BootloaderReclaimable),
            entry(base + 0x200 * PAGE_SIZE, 0x40, MemoryKind::Kernel),
            entry(
                base + 0x240 * PAGE_SIZE,
                0x1000 - 0x240,
                MemoryKind::BootloaderReclaimable
            ),
        ]
    );

    // At the start of an entry, with an entry after it that must move along.
    let mut out = [MemoryEntry::default(); 4];
    out[0] = entry(base, 0x100, MemoryKind::BootloaderReclaimable);
    out[1] = entry(base + 0x100 * PAGE_SIZE, 0x100, MemoryKind::Usable);
    let n = carve_kernel(&mut out, 2, base, 0x10 * PAGE_SIZE).unwrap();
    assert_eq!(n, 3);
    assert_eq!(out[0], entry(base, 0x10, MemoryKind::Kernel));
    assert_eq!(
        out[1],
        entry(
            base + 0x10 * PAGE_SIZE,
            0xf0,
            MemoryKind::BootloaderReclaimable
        )
    );
    assert_eq!(
        out[2],
        entry(base + 0x100 * PAGE_SIZE, 0x100, MemoryKind::Usable)
    );

    // Across two entries, or beyond the map, or with no room.
    assert_eq!(
        carve_kernel(&mut out, 3, base + 0xf0 * PAGE_SIZE, 0x20 * PAGE_SIZE),
        Err(BuildError::KernelNotContained)
    );
    let mut full = [MemoryEntry::default(); 1];
    full[0] = entry(base, 0x100, MemoryKind::BootloaderReclaimable);
    assert_eq!(
        carve_kernel(&mut full, 1, base + PAGE_SIZE, PAGE_SIZE),
        Err(BuildError::TooManyEntries)
    );
}

#[test]
fn a_good_map_validates_and_the_view_reads_it_back() {
    let kernel = 0x4020_0000;
    let entries = [
        entry(0x4000_0000, 0x200, MemoryKind::BootloaderReclaimable),
        entry(kernel, 0x300, MemoryKind::Kernel),
        entry(0x4050_0000, 0x1000, MemoryKind::Usable),
    ];
    for stride in [24, 32] {
        let bytes = map_of(&entries, stride);
        let map = MemoryMap::from_bytes(&bytes, stride).unwrap();
        assert_eq!(map.len(), 3);
        assert_eq!(map.iter().collect::<Vec<_>>(), entries);
        assert_eq!(map.validate(kernel, 0x2f_f010), Ok(()));
    }
}

#[test]
fn a_bad_map_is_refused_with_the_reason() {
    let kernel = 0x4020_0000;
    let good = [
        entry(0x4000_0000, 0x200, MemoryKind::BootloaderReclaimable),
        entry(kernel, 0x300, MemoryKind::Kernel),
        entry(0x4050_0000, 0x1000, MemoryKind::Usable),
    ];
    let check = |entries: &[MemoryEntry], size: u64| {
        let bytes = map_of(entries, ENTRY_SIZE);
        MemoryMap::from_bytes(&bytes, ENTRY_SIZE)
            .unwrap()
            .validate(kernel, size)
    };
    let kernel_size = 0x30_0000;
    assert_eq!(check(&good, kernel_size), Ok(()));
    assert_eq!(check(&[], kernel_size), Err(MapError::Empty));

    let mut unsorted = good;
    unsorted.swap(0, 2);
    assert_eq!(check(&unsorted, kernel_size), Err(MapError::Unsorted));

    let mut overlap = good;
    overlap[2].base = 0x4050_0000 - 0x1000;
    assert_eq!(check(&overlap, kernel_size), Err(MapError::Overlap));

    let mut empty = good;
    empty[2].length = 0;
    assert_eq!(check(&empty, kernel_size), Err(MapError::EmptyEntry));

    let mut wraps = good;
    wraps[2].base = u64::MAX - 0xfff;
    assert_eq!(check(&wraps, kernel_size), Err(MapError::EmptyEntry));

    let mut unaligned = good;
    unaligned[2].base += 0x10;
    assert_eq!(check(&unaligned, kernel_size), Err(MapError::Misaligned));

    assert_eq!(check(&good[..1], kernel_size), Err(MapError::KernelEntry));
    assert_eq!(
        check(&good, kernel_size - 0x2000),
        Err(MapError::KernelEntry)
    );
    let mut moved = good;
    moved[1].base += PAGE_SIZE;
    moved[1].length -= PAGE_SIZE;
    assert_eq!(check(&moved, kernel_size), Err(MapError::KernelEntry));
    let mut two_kernels = good;
    two_kernels[2].kind = MemoryKind::Kernel as u32;
    assert_eq!(check(&two_kernels, kernel_size), Err(MapError::KernelEntry));
}

#[test]
fn the_view_refuses_bad_strides_and_sizes() {
    assert_eq!(
        MemoryMap::from_bytes(&[0; 48], 16).err(),
        Some(MapError::EntrySize)
    );
    assert_eq!(
        MemoryMap::from_bytes(&[0; 50], 24).err(),
        Some(MapError::TooLarge)
    );

    let mut info = sample();
    info.memory_map_entry_size = 8;
    // SAFETY: the entry size is refused before any memory is read.
    let result = unsafe { info.memory_map(|address| address as usize) }.err();
    assert_eq!(result, Some(Error::BadMemoryMap(MapError::EntrySize)));
    info.memory_map_entry_size = 24;
    info.memory_map_count = u64::MAX / 8;
    // SAFETY: the count is refused before any memory is read.
    let result = unsafe { info.memory_map(|address| address as usize) }.err();
    assert_eq!(result, Some(Error::BadMemoryMap(MapError::TooLarge)));
    info.memory_map_count = 2;
    info.memory_map = 0;
    // SAFETY: a null address is refused before any memory is read.
    let result = unsafe { info.memory_map(|address| address as usize) }.err();
    assert_eq!(result, Some(Error::BadMemoryMap(MapError::BadAddress)));
}

#[test]
fn the_map_is_read_at_the_address_the_caller_converts_to() {
    let entries = [
        MemoryEntry::new(0x4000_0000, 0x20_0000, MemoryKind::Usable),
        MemoryEntry::new(0x4020_0000, 0x4_0000, MemoryKind::Kernel),
    ];
    let bytes = map_of(&entries, ENTRY_SIZE);
    // A physical address that is not where the bytes are: the structure holds
    // `bytes`' address minus a linear base, and the kernel adds it back.
    const LINEAR_BASE: u64 = 0xffff_0000_0000_0000;
    let mut info = sample();
    info.memory_map_entry_size = ENTRY_SIZE as u32;
    info.memory_map_count = entries.len() as u64;
    info.memory_map = (bytes.as_ptr() as u64).wrapping_sub(LINEAR_BASE);
    // SAFETY: the conversion gives the address of `bytes`, which holds the
    // entries and outlives the map.
    let map =
        unsafe { info.memory_map(|physical| physical.wrapping_add(LINEAR_BASE) as usize) }.unwrap();
    assert_eq!(map.len(), 2);
    assert_eq!(map.iter().next().unwrap().base, 0x4000_0000);
    assert_eq!(map.iter().nth(1).unwrap().kind(), MemoryKind::Kernel);
    // The checks are on the physical address the structure holds.
    info.memory_map = 0x4000_0001;
    // SAFETY: a misaligned address is refused before the conversion is called.
    let refused = unsafe { info.memory_map(|_| unreachable!("a bad address is not converted")) };
    assert_eq!(
        refused.err(),
        Some(Error::BadMemoryMap(MapError::BadAddress))
    );
}

// ---- page tables ----

const MAIR: u64 = 0xffbb_4400; // index 0 device, 1 non-cacheable, 2 write-through, 3 write-back

fn attributes(bits: u8) -> TableAttributes {
    TableAttributes::from_mair(MAIR, bits).unwrap()
}

struct Walk {
    physical: u64,
    index: u64,
    execute_never: bool,
    access_flag: bool,
    /// Bits 9:8 of the descriptor: 3 inner shareable, 2 outer shareable.
    shareability: u64,
    level: u8,
}

/// Translates `address` through the tables the way the hardware would.
fn translate(pool: &[Table], root: u64, bits: u8, address: u64) -> Option<Walk> {
    let base = pool.as_ptr() as u64;
    let table_at =
        |descriptor: u64| &pool[(((descriptor & 0x0000_ffff_ffff_f000) - base) / 4096) as usize];
    let mut table = &pool[((root - base) / 4096) as usize];
    let mut level = if bits > 39 { 0 } else { 1 };
    loop {
        let shift = 12 + 9 * (3 - level as u32);
        let descriptor = table.0[((address >> shift) & 0x1ff) as usize];
        if descriptor & 1 == 0 {
            return None;
        }
        if level == 3 || descriptor & 2 == 0 {
            assert!(level == 3 && descriptor & 2 != 0 || level < 3 && descriptor & 2 == 0);
            let mask = (1u64 << shift) - 1;
            return Some(Walk {
                physical: (descriptor & 0x0000_ffff_ffff_f000 & !mask) | (address & mask),
                index: (descriptor >> 2) & 7,
                execute_never: descriptor >> 53 & 3 == 3,
                access_flag: descriptor & (1 << 10) != 0,
                shareability: (descriptor >> 8) & 3,
                level,
            });
        }
        table = table_at(descriptor);
        level += 1;
    }
}

fn pool(count: usize) -> Vec<Table> {
    std::vec![Table::EMPTY; count]
}

fn sample_map() -> Vec<MemoryEntry> {
    std::vec![
        entry(0x4000_0000, 0x200, MemoryKind::BootloaderReclaimable), // 2 MiB, aligned
        entry(0x4020_0000, 0x300, MemoryKind::Kernel), // 3 MiB: one block, then pages
        entry(0x4050_0000, 0x7aff, MemoryKind::Usable), // ends mid-block
        entry(0x4ff0_0000, 0x10, MemoryKind::ReservedMapped),
        entry(0x0900_0000, 1, MemoryKind::Reserved),
        entry(0x8_0000_0000, 0x100, MemoryKind::Usable),
    ]
}

#[test]
fn the_identity_map_maps_memory_normal_and_the_rest_device() {
    for bits in [39u8, 40, 48] {
        let entries = sample_map();
        let mut pool = pool(tables_needed(&entries));
        let root = build_identity_map(&mut pool, &entries, attributes(bits)).unwrap();

        let probe = |address: u64| translate(&pool, root, bits, address).unwrap();
        for entry in &entries {
            let normal = entry.kind().is_normal_memory();
            for address in [entry.base, entry.base + entry.length / 2, entry.end() - 1] {
                let walk = probe(address);
                assert_eq!(walk.physical, address, "identity at {address:#x}");
                assert_eq!(
                    walk.index,
                    if normal { 3 } else { 0 },
                    "type at {address:#x}"
                );
                assert_eq!(walk.execute_never, !normal, "xn at {address:#x}");
                // Without the access flag the first access faults; Normal
                // memory is inner shareable, Device outer shareable.
                assert!(walk.access_flag, "access flag at {address:#x}");
                assert_eq!(
                    walk.shareability,
                    if normal { 3 } else { 2 },
                    "shareability at {address:#x}"
                );
            }
        }
        // Memory where no entry says so is Device, never unmapped, up to 512 GiB.
        for address in [
            0,
            0x0800_0000,
            0x3fff_f000,
            0x4900_0000,
            0x7fff_0000,
            0x7f_ffff_f000,
        ] {
            let walk = probe(address);
            assert_eq!(walk.physical, address);
            assert_eq!((walk.index, walk.execute_never), (0, true), "{address:#x}");
        }
        // Just outside a memory entry's last page is still Device.
        assert_eq!(probe(0x4050_0000 + 0x7aff * 4096).index, 0);
        assert_eq!(probe(0x4020_0000 - 1).index, 3);
        // A 2 MiB aligned entry is mapped by blocks, an edge by pages.
        assert_eq!(probe(0x4000_0000).level, 2);
        assert_eq!(probe(0x4050_0000 + 0x7aff * 4096 - 1).level, 3);
        // Beyond 512 GiB nothing is mapped.
        if bits > 39 {
            assert!(translate(&pool, root, bits, 1 << 39).is_none());
        }
    }
}

#[test]
fn the_pool_bound_holds_and_a_smaller_pool_is_refused() {
    let entries = sample_map();
    let needed = tables_needed(&entries);
    let mut big = pool(needed);
    assert!(build_identity_map(&mut big, &entries, attributes(48)).is_ok());

    let mut small = pool(2);
    assert_eq!(
        build_identity_map(&mut small, &entries, attributes(48)),
        Err(PageError::PoolExhausted)
    );
}

#[test]
fn overlapping_or_out_of_range_memory_is_refused() {
    let mut overlap = std::vec![
        entry(0x4000_0000, 0x10, MemoryKind::Usable),
        entry(0x4000_8000, 0x10, MemoryKind::Usable),
    ];
    let mut tables = pool(16);
    assert_eq!(
        build_identity_map(&mut tables, &overlap, attributes(48)),
        Err(PageError::Overlap)
    );
    overlap.truncate(1);
    // Past what the regime translates: 2^48, and 2^39 where it has 39 bits.
    overlap[0].base = 1 << 48;
    assert_eq!(
        build_identity_map(&mut tables, &overlap, attributes(48)),
        Err(PageError::OutOfRange)
    );
    overlap[0].base = 1 << 39;
    assert_eq!(
        build_identity_map(&mut tables, &overlap, attributes(39)),
        Err(PageError::OutOfRange)
    );
    assert_eq!(
        build_identity_map(&mut tables, &[], attributes(30)),
        Err(PageError::UnsupportedAddressBits)
    );
    // A regime that covers 36 bits cannot hold memory at 64 GiB.
    let high = [entry(0x10_0000_0000, 1, MemoryKind::Usable)];
    assert_eq!(
        build_identity_map(&mut tables, &high, attributes(36)),
        Err(PageError::OutOfRange)
    );
}

#[test]
fn memory_above_512_gib_is_mapped_through_a_level_1_table_of_its_own() {
    // The machine of the item (ADR 0033): RAM from 1 GiB to 521 GiB, with
    // something at the very top, where a firmware allocates.
    let entries = std::vec![
        entry(0x0900_0000, 1, MemoryKind::Reserved),
        entry(0x4000_0000, 0x200, MemoryKind::Kernel),
        entry(0x4020_0000, 0x1000, MemoryKind::Usable),
        entry(0x81_0000_0000, 0x400, MemoryKind::Usable), // 516 GiB
        entry(0x81_ffc0_0000, 0x400, MemoryKind::BootloaderReclaimable), // the top
    ];
    for bits in [40u8, 48] {
        let mut tables = pool(tables_needed(&entries));
        let root = build_identity_map(&mut tables, &entries, attributes(bits)).unwrap();
        let probe = |address: u64| translate(&tables, root, bits, address);
        for address in [0x81_0000_0000, 0x81_ffc0_0000, 0x82_0000_0000 - 1] {
            let walk = probe(address).unwrap_or_else(|| panic!("{address:#x} unmapped at {bits}"));
            assert_eq!(walk.physical, address, "identity at {address:#x}");
            assert_eq!(walk.index, 3, "Normal at {address:#x}, {bits} bits");
            assert!(!walk.execute_never);
        }
        // Other addresses of the span are Device, as below 512 GiB.
        for address in [0x80_0000_0000, 0x80_8000_0000, 0x81_4000_0000] {
            let walk = probe(address).unwrap();
            assert_eq!((walk.physical, walk.index, walk.level), (address, 0, 1));
            assert!(walk.execute_never);
        }
        // Below 512 GiB nothing changed.
        assert_eq!(probe(0x4020_0000).unwrap().index, 3);
        assert_eq!(probe(0x0900_0000).unwrap().index, 0);
        // A span no region reaches is not made.
        assert!(probe(0x100_0000_0000).is_none());
    }
}

#[test]
fn the_pool_bound_covers_memory_that_spans_512_gib() {
    // 600 GiB from 1 GiB: it ends at 601 GiB, in the second span.
    let entries = std::vec![
        entry(0x4000_0000, 0x200, MemoryKind::Kernel),
        entry(0x4020_0000, (600 << 18) - 0x200, MemoryKind::Usable),
    ];
    let mut tables = pool(tables_needed(&entries));
    let root = build_identity_map(&mut tables, &entries, attributes(48)).unwrap();
    let probe = |address: u64| translate(&tables, root, 48, address).unwrap();
    assert_eq!(probe(0x95_ffff_f000).index, 3);
    assert_eq!(probe(0x80_0000_0000).index, 3);
    let mut short = pool(tables_needed(&entries) / 2);
    assert_eq!(
        build_identity_map(&mut short, &entries, attributes(48)),
        Err(PageError::PoolExhausted)
    );
}

#[test]
fn the_memory_types_come_from_the_live_mair() {
    // No Device-nGnRnE entry, and no write-back entry.
    assert!(TableAttributes::from_mair(0xffff_ffff_ffff_ffff, 48).is_none());
    assert!(TableAttributes::from_mair(0x4444_4444_4444_4400, 48).is_none());
    let found = TableAttributes::from_mair(0x0000_00ff_0000_0000, 48).unwrap();
    assert_eq!((found.normal_index, found.device_index), (4, 0));
}

// ---- ELF ----

fn elf_with(segments: &[(u32, u64, u64, u64, u64)], entry: u64) -> Vec<u8> {
    // (type, file offset, address, file size, memory size)
    let mut file = std::vec![0u8; 64 + 56 * segments.len()];
    file[..4].copy_from_slice(b"\x7fELF");
    file[4] = 2;
    file[5] = 1;
    file[6] = 1;
    file[16..18].copy_from_slice(&2u16.to_le_bytes());
    file[18..20].copy_from_slice(&183u16.to_le_bytes());
    file[24..32].copy_from_slice(&entry.to_le_bytes());
    file[32..40].copy_from_slice(&64u64.to_le_bytes());
    file[54..56].copy_from_slice(&56u16.to_le_bytes());
    file[56..58].copy_from_slice(&(segments.len() as u16).to_le_bytes());
    for (i, (kind, offset, address, file_size, memory_size)) in segments.iter().enumerate() {
        let at = 64 + 56 * i;
        file[at..at + 4].copy_from_slice(&kind.to_le_bytes());
        file[at + 8..at + 16].copy_from_slice(&offset.to_le_bytes());
        file[at + 24..at + 32].copy_from_slice(&address.to_le_bytes());
        file[at + 32..at + 40].copy_from_slice(&file_size.to_le_bytes());
        file[at + 40..at + 48].copy_from_slice(&memory_size.to_le_bytes());
    }
    file.resize(file.len() + 0x4000, 0);
    file
}

#[test]
fn an_elf_is_planned_with_its_bss_and_stack() {
    let file = elf_with(
        &[
            (1, 0x1000, 0x4020_0000, 0x1000, 0x1000),
            (1, 0x2000, 0x4020_1000, 0x800, 0x12000),
            (4, 0, 0, 0, 0),
        ],
        0x4020_0000,
    );
    let plan = elf::plan(&file).unwrap();
    assert_eq!(plan.base, 0x4020_0000);
    assert_eq!(plan.end, 0x4021_3000);
    assert_eq!(plan.memory_size(), 0x13000);
    assert_eq!(plan.span(), 0x13000);
    assert_eq!(plan.segments().len(), 2);
    assert_eq!(plan.entry, 0x4020_0000);
}

#[test]
fn an_elf_that_cannot_be_trusted_is_refused() {
    use elf::ElfError::*;
    let good = &[(1u32, 0x1000u64, 0x4020_0000u64, 0x1000u64, 0x2000u64)];
    let file = elf_with(good, 0x4020_0000);
    assert!(elf::plan(&file).is_ok());
    assert_eq!(elf::plan(&file[..10]), Err(TooSmall));

    let mut bad = file.clone();
    bad[4] = 1;
    assert_eq!(elf::plan(&bad), Err(NotElf64LittleEndian));
    let mut bad = file.clone();
    bad[18] = 62; // x86-64
    assert_eq!(elf::plan(&bad), Err(NotAarch64Executable));
    let mut bad = file.clone();
    bad[16] = 3; // ET_DYN
    assert_eq!(elf::plan(&bad), Err(NotAarch64Executable));

    assert_eq!(
        elf::plan(&elf_with(&[], 0x4020_0000)),
        Err(NoLoadableSegment)
    );
    assert_eq!(elf::plan(&elf_with(good, 0x4030_0000)), Err(BadEntry));
    assert_eq!(elf::plan(&elf_with(good, 0x4020_2000)), Err(BadEntry));
    assert_eq!(
        elf::plan(&elf_with(
            &[(1, 0x1000, 0x4020_0000, 0x3000, 0x2000)],
            0x4020_0000
        )),
        Err(BadSegment)
    );
    assert_eq!(
        elf::plan(&elf_with(
            &[(1, u64::MAX - 4, 0x4020_0000, 0x10, 0x20)],
            0x4020_0000
        )),
        Err(BadSegment)
    );
    assert_eq!(
        elf::plan(&elf_with(
            &[(1, 0x1000, u64::MAX - 8, 0x10, 0x20)],
            0x4020_0000
        )),
        Err(BadSegment)
    );
    assert_eq!(
        elf::plan(&elf_with(
            &[
                (1, 0x1000, 0x4020_0000, 0x10, 0x2000),
                (1, 0x1000, 0x4020_1000, 0x10, 0x10)
            ],
            0x4020_0000
        )),
        Err(Overlap)
    );
    assert_eq!(
        elf::plan(&elf_with(
            &[(1, 0x1000, 0x4020_0800, 0x10, 0x2000)],
            0x4020_0800
        )),
        Err(Unaligned)
    );
    let mut crowded = Vec::new();
    for i in 0..9 {
        crowded.push((1, 0x1000, 0x4020_0000 + 0x1000 * i as u64, 0x10, 0x10));
    }
    assert_eq!(
        elf::plan(&elf_with(&crowded, 0x4020_0000)),
        Err(TooManySegments)
    );

    // Program headers past the end of the file.
    let mut bad = file.clone();
    bad[32..40].copy_from_slice(&u64::MAX.to_le_bytes());
    assert_eq!(elf::plan(&bad), Err(BadProgramHeaders));
    let mut bad = file.clone();
    bad[54..56].copy_from_slice(&8u16.to_le_bytes());
    assert_eq!(elf::plan(&bad), Err(BadProgramHeaders));
}

#[test]
fn no_flip_of_an_elf_byte_panics() {
    let file = elf_with(&[(1, 0x1000, 0x4020_0000, 0x1000, 0x2000)], 0x4020_0000);
    for index in 0..64 + 56 {
        for bit in 0..8 {
            let mut bytes = file.clone();
            bytes[index] ^= 1 << bit;
            let _ = elf::plan(&bytes);
        }
    }
}

#[test]
fn huge_sizes_are_refused_not_overflowed() {
    let entries = [entry(0x4020_0000, 1, MemoryKind::Kernel)];
    let bytes = map_of(&entries, ENTRY_SIZE);
    let map = MemoryMap::from_bytes(&bytes, ENTRY_SIZE).unwrap();
    assert_eq!(
        map.validate(0x4020_0000, u64::MAX),
        Err(MapError::KernelEntry)
    );

    let file = elf_with(
        &[(1, 0x1000, 0xffff_ffff_ffff_f000, 0x10, 0xfff)],
        0xffff_ffff_ffff_f000,
    );
    assert_eq!(elf::plan(&file), Err(elf::ElfError::BadSegment));
}

#[test]
fn a_truncated_body_gets_the_default_entry_size_and_a_big_size_is_read() {
    let mut bytes = sample().to_bytes();
    let size = offset_of!(BootInfo, memory_map) as u32;
    bytes[12..16].copy_from_slice(&size.to_le_bytes());
    let parsed = BootInfo::parse(&bytes[..size as usize]).unwrap();
    assert_eq!(parsed.memory_map_entry_size, ENTRY_SIZE as u32);

    let mut big = sample().to_bytes().to_vec();
    big.resize(8192, 0);
    big[12..16].copy_from_slice(&8192u32.to_le_bytes());
    assert!(BootInfo::parse(&big).is_ok());
}

#[test]
fn entries_of_one_kind_with_a_gap_do_not_merge() {
    let mut out = [MemoryEntry::default(); 4];
    let n = build_memory_map(
        [
            descriptor(1, 0x1000, 1),
            descriptor(1, 0x3000, 1),
            descriptor(1, 0x4000, 1),
        ],
        &mut out,
    )
    .unwrap();
    assert_eq!(
        &out[..n],
        &[
            entry(0x1000, 1, MemoryKind::BootloaderReclaimable),
            entry(0x3000, 2, MemoryKind::BootloaderReclaimable),
        ]
    );
    // One page of overlap is an overlap.
    assert_eq!(
        build_memory_map(
            [descriptor(7, 0x1000, 2), descriptor(7, 0x2000, 2)],
            &mut out
        ),
        Err(BuildError::Overlap)
    );
}

const KERNEL_BASE: u64 = 0x4020_0000;
const KERNEL_SIZE: u64 = 0x30_0000;

fn handoff_info() -> BootInfo {
    let mut info = sample();
    info.boot_mode = MODE_NORMAL;
    info.kernel_base = KERNEL_BASE;
    info.kernel_size = KERNEL_SIZE;
    info
}

fn handoff_entries() -> Vec<MemoryEntry> {
    vec![
        entry(0x3000_0000, 0x10, MemoryKind::Reserved),
        entry(0x4000_0000, 0x200, MemoryKind::BootloaderReclaimable),
        entry(KERNEL_BASE, 0x300, MemoryKind::Kernel),
        entry(0x4050_0000, 0x1000, MemoryKind::Usable),
        entry(0x4200_0000, 8, MemoryKind::AcpiReclaimable),
        entry(0x4300_0000, 8, MemoryKind::AcpiNvs),
        entry(0x4400_0000, 8, MemoryKind::BadMemory),
        entry(0x5000_0000, 0x10, MemoryKind::ReservedMapped),
    ]
}

/// Runs the kernel's checks on `info` with `entries` as the map it points at.
fn run_check(
    info: BootInfo,
    entries: &[MemoryEntry],
) -> (Result<(HandoffSummary, BootInfo), Rejected>, bool) {
    let bytes = map_of(entries, ENTRY_SIZE);
    let mut map_read = false;
    let result = check(Ok(info), |_| {
        map_read = true;
        MemoryMap::from_bytes(&bytes, ENTRY_SIZE).map_err(Error::BadMemoryMap)
    })
    .map(|handoff| {
        (
            HandoffSummary {
                console: handoff.console,
                ram_lowest: handoff.ram_lowest,
                ram_total: handoff.ram_total,
                entries: handoff.map.len(),
            },
            handoff.info,
        )
    });
    (result, map_read)
}

#[derive(Debug, PartialEq, Eq)]
struct HandoffSummary {
    console: usize,
    ram_lowest: u64,
    ram_total: u64,
    entries: usize,
}

#[test]
fn a_good_handoff_is_accepted_and_reports_what_the_kernel_needs() {
    let info = handoff_info();
    let (result, _) = run_check(info, &handoff_entries());
    let (summary, accepted) = result.expect("accepted");
    assert_eq!(accepted, info);
    assert_eq!(
        summary,
        HandoffSummary {
            console: 0x0900_0000,
            // RAM is the usable, reclaimable and kernel entries, not the
            // reserved ones, whatever their address.
            ram_lowest: 0x4000_0000,
            ram_total: (0x200 + 0x300 + 0x1000 + 8) * PAGE_SIZE,
            entries: 8,
        }
    );
}

#[test]
fn a_header_that_is_not_a_boot_info_is_refused_without_a_console() {
    let mut bytes = handoff_info().to_bytes();
    bytes[0] ^= 1;
    let refused = check(BootInfo::parse(&bytes), |_| unreachable!("no map is read"));
    assert_eq!(
        refused.err(),
        Some(Rejected {
            error: HandoffError::Info(Error::BadMagic),
            console: None
        })
    );
    for (error, bytes) in [
        (Error::TooSmall, &bytes[..8]),
        (
            Error::BadSize,
            &{
                let mut short = handoff_info().to_bytes();
                short[12..16].copy_from_slice(&8u32.to_le_bytes());
                short
            }[..],
        ),
    ] {
        let refused = check(BootInfo::parse(bytes), |_| unreachable!());
        assert_eq!(
            refused.err().map(|r| (r.error, r.console)),
            Some((HandoffError::Info(error), None))
        );
    }
    // SAFETY: a null address is refused before anything is read.
    let parsed = unsafe { BootInfo::from_address(0) };
    let refused = check(parsed, |_| unreachable!());
    assert_eq!(
        refused.err().map(|r| r.error),
        Some(HandoffError::Info(Error::NullPointer))
    );
}

#[test]
fn an_unknown_version_is_refused_on_the_console_its_header_names() {
    let mut bytes = handoff_info().to_bytes();
    bytes[8..10].copy_from_slice(&2u16.to_le_bytes());
    let refused = check(BootInfo::parse(&bytes), |_| unreachable!());
    let refused = refused.err().expect("refused");
    assert_eq!(
        refused.error,
        HandoffError::Info(Error::UnsupportedVersion {
            major: 2,
            minor: 0,
            console_kind: CONSOLE_PL011,
            console_base: 0x0900_0000,
        })
    );
    assert_eq!(refused.console, Some(0x0900_0000));

    // A console the kernel cannot drive cannot report the refusal either.
    let mut bytes = handoff_info().to_bytes();
    bytes[8..10].copy_from_slice(&2u16.to_le_bytes());
    bytes[16..20].copy_from_slice(&CONSOLE_NONE.to_le_bytes());
    let refused = check(BootInfo::parse(&bytes), |_| unreachable!());
    assert_eq!(refused.err().map(|r| r.console), Some(None));
}

#[test]
fn a_console_the_kernel_cannot_drive_is_refused() {
    for (kind, base) in [
        (CONSOLE_NONE, 0x0900_0000),
        (CONSOLE_NS16550_MMIO, 0x0900_0000),
        (CONSOLE_NS16550_PORT, 0x3f8),
        (99, 0x0900_0000),
        (CONSOLE_PL011, 0),
    ] {
        let mut info = handoff_info();
        info.console_kind = kind;
        info.console_base = base;
        let (result, map_read) = run_check(info, &handoff_entries());
        assert_eq!(
            result.err(),
            Some(Rejected {
                error: HandoffError::UnsupportedConsole(kind),
                console: None
            }),
            "console {kind} at {base:#x}"
        );
        assert!(!map_read, "the map is not read after a refused console");
    }
}

#[test]
fn another_architecture_is_refused_on_the_console() {
    for arch in [ARCH_X86_64, 0, 7] {
        let mut info = handoff_info();
        info.arch = arch;
        let (result, map_read) = run_check(info, &handoff_entries());
        assert_eq!(
            result.err(),
            Some(Rejected {
                error: HandoffError::UnsupportedArch(arch),
                console: Some(0x0900_0000)
            })
        );
        assert!(!map_read);
    }
}

#[test]
fn a_bad_memory_map_is_refused_on_the_console() {
    let refuses = |info: BootInfo, entries: &[MemoryEntry], error: Error| {
        let (result, map_read) = run_check(info, entries);
        assert!(map_read);
        assert_eq!(
            result.err(),
            Some(Rejected {
                error: HandoffError::Info(error),
                console: Some(0x0900_0000)
            })
        );
    };
    let bad = |reason| Error::BadMemoryMap(reason);
    refuses(handoff_info(), &[], bad(MapError::Empty));

    let mut unsorted = handoff_entries();
    unsorted.swap(0, 3);
    refuses(handoff_info(), &unsorted, bad(MapError::Unsorted));

    let mut overlap = handoff_entries();
    overlap[3].base = 0x4040_0000;
    refuses(handoff_info(), &overlap, bad(MapError::Overlap));

    let mut unaligned = handoff_entries();
    unaligned[1].base += 0x10;
    refuses(handoff_info(), &unaligned, bad(MapError::Misaligned));

    // No RAM at all: a map of firmware regions only has no kernel entry.
    refuses(
        handoff_info(),
        &[entry(0x3000_0000, 0x10, MemoryKind::Reserved)],
        bad(MapError::KernelEntry),
    );

    // The kernel the structure describes is not the kernel entry of the map.
    let mut info = handoff_info();
    info.kernel_base += PAGE_SIZE;
    refuses(info, &handoff_entries(), bad(MapError::KernelEntry));
    let mut info = handoff_info();
    info.kernel_size = 0;
    refuses(info, &handoff_entries(), bad(MapError::KernelEntry));
}

#[test]
fn a_map_that_cannot_be_read_is_refused_on_the_console() {
    for error in [
        Error::BadMemoryMap(MapError::EntrySize),
        Error::BadMemoryMap(MapError::BadAddress),
        Error::BadMemoryMap(MapError::TooLarge),
    ] {
        let refused = check(Ok(handoff_info()), |_| Err(error));
        assert_eq!(
            refused.err(),
            Some(Rejected {
                error: HandoffError::Info(error),
                console: Some(0x0900_0000)
            })
        );
    }
}

#[test]
fn the_memory_map_view_of_a_structure_is_bounded_before_it_is_read() {
    // The reads `BootInfo::memory_map` refuses before it touches memory.
    let mut info = handoff_info();
    // SAFETY: every structure below is refused before the map is read.
    let refused = |info: &BootInfo| unsafe { info.memory_map(|address| address as usize) }.err();
    info.memory_map_count = u64::MAX;
    assert_eq!(
        refused(&info),
        Some(Error::BadMemoryMap(MapError::TooLarge))
    );
    info.memory_map_count = 1 << 40;
    assert_eq!(
        refused(&info),
        Some(Error::BadMemoryMap(MapError::TooLarge))
    );
    info = handoff_info();
    info.memory_map_entry_size = 16;
    assert_eq!(
        refused(&info),
        Some(Error::BadMemoryMap(MapError::EntrySize))
    );
    info = handoff_info();
    info.memory_map = 0;
    assert_eq!(
        refused(&info),
        Some(Error::BadMemoryMap(MapError::BadAddress))
    );
    info.memory_map = 0x1001;
    assert_eq!(
        refused(&info),
        Some(Error::BadMemoryMap(MapError::BadAddress))
    );
    info.memory_map = u64::MAX - 8;
    assert_eq!(
        refused(&info),
        Some(Error::BadMemoryMap(MapError::BadAddress))
    );
}

#[test]
fn the_acpi_root_pointer_is_handed_over_as_the_firmware_has_it() {
    // ACPI 2.0 wins when the firmware publishes both tables, zero is "none".
    assert_eq!(select_rsdp(Some(0x2000), Some(0x1000)), 0x2000);
    assert_eq!(select_rsdp(None, Some(0x1000)), 0x1000);
    assert_eq!(select_rsdp(Some(0x2000), None), 0x2000);
    assert_eq!(select_rsdp(None, None), 0);

    // From the bootloader's structure, through the bytes, to what the kernel
    // reports: QEMU's firmware never offers a device tree and ACPI together,
    // so this is the only test of a non-zero value.
    let mut info = handoff_info();
    info.rsdp = 0x4fff_0000;
    let (result, _) = run_check(
        BootInfo::parse(&info.to_bytes()).unwrap(),
        &handoff_entries(),
    );
    assert_eq!(result.expect("accepted").1.rsdp, 0x4fff_0000);
}
