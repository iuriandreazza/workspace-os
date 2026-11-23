use super::*;

const TYPES: MemoryTypes = MemoryTypes {
    normal_index: 1,
    device_index: 0,
};
const ARENA_BASE: u64 = 0x1000_0000;

struct Arena {
    tables: Vec<Table>,
    limit: usize,
}

impl Arena {
    fn new(limit: usize) -> Self {
        Self {
            tables: Vec::new(),
            limit,
        }
    }

    fn slot(address: u64) -> usize {
        ((address - ARENA_BASE) / PAGE_SIZE) as usize
    }
}

impl TableRead for Arena {
    fn table(&self, address: u64) -> &Table {
        &self.tables[Self::slot(address)]
    }
}

impl TableMemory for Arena {
    fn allocate(&mut self) -> Option<u64> {
        (self.tables.len() < self.limit).then(|| {
            self.tables.push(Table::EMPTY);
            ARENA_BASE + (self.tables.len() as u64 - 1) * PAGE_SIZE
        })
    }

    fn table_mut(&mut self, address: u64) -> &mut Table {
        &mut self.tables[Self::slot(address)]
    }
}

const BASE: u64 = 0x4020_0000;
fn layout() -> ImageLayout {
    ImageLayout::new(
        BASE..BASE + 0x1d000,
        BASE + 0x1d000..BASE + 0x22000,
        BASE + 0x22000..BASE + 0x64000,
        BASE + 0x65000..BASE + 0x75000,
        BASE + 0x76000..BASE + 0x7a000,
    )
    .unwrap()
}

/// An expectation with nothing forbidden, no Device range and no known ranges,
/// of a root that maps from `base`, by the offset `base` (the lower half).
fn open() -> Expectation<'static> {
    Expectation {
        types: TYPES,
        window: None,
        forbidden: &(0..0),
        device: &(0..0),
        known: None,
    }
}

/// The upper half the kernel builds: the image at its physical address plus
/// the offset.
fn upper(arena: &mut Arena) -> u64 {
    let mut mapper = Mapper::new(arena, TYPES).unwrap();
    layout().map(&mut mapper, KERNEL_OFFSET).unwrap();
    mapper.root()
}

/// The level 3 table that holds the page at `virtual_address`.
fn page_table(arena: &Arena, root: u64, virtual_address: u64) -> u64 {
    let l1 = arena.table(root).0[level_index(virtual_address, 0)] & ADDRESS_MASK;
    let l2 = arena.table(l1).0[level_index(virtual_address, 1)] & ADDRESS_MASK;
    arena.table(l2).0[level_index(virtual_address, 2)] & ADDRESS_MASK
}

/// Maps the pages of `length` bytes of registers at `base` to themselves as
/// Device memory, as the old lower window did.
fn map_device(mapper: &mut Mapper<'_, Arena>, base: u64, length: u64) -> Result<(), MapError> {
    let start = base & !(PAGE_SIZE - 1);
    let end = (base + length).next_multiple_of(PAGE_SIZE);
    mapper.map_range(start, start, end - start, Permissions::Device)
}

/// The window the image is audited in: its own pages, by `KERNEL_OFFSET`.
static IMAGE_WINDOW: Window = Window {
    virt: KERNEL_OFFSET + BASE..KERNEL_OFFSET + BASE + 0x7a000,
    offset: Some(KERNEL_OFFSET),
};

/// An expectation of the upper half's image window.
fn in_image(expectation: Expectation<'_>) -> Expectation<'_> {
    Expectation {
        window: Some(&IMAGE_WINDOW),
        ..expectation
    }
}

#[test]
fn indexes_follow_the_bits_of_the_address() {
    let address = KERNEL_OFFSET + 0x4021_d000;
    assert_eq!(level_index(address, 0), 511);
    assert_eq!(level_index(address, 1), 1);
    assert_eq!(level_index(address, 2), 1);
    assert_eq!(level_index(address, 3), 0x1d);
    assert_eq!(level_index(0xffff_ffff_ffff_f000, 0), 511);
    assert_eq!(level_index(0x7f_ffff_f000, 0), 0);
    assert_eq!(level_index(0x7f_ffff_f000, 1), 511);
    assert_eq!(level_index(0x7f_ffff_f000, 3), 511);
    assert_eq!(level_index(0x0000_7fff_ffff_f000, 0), 255);
    assert_eq!(level_index(LINEAR_BASE + (513 << 30), 0), 1);
}

#[test]
fn canonical_addresses_are_the_two_halves() {
    assert!(is_canonical(0));
    assert!(is_canonical(0x7f_ffff_ffff));
    assert!(is_canonical(0x80_0000_0000));
    assert!(is_canonical(0x0000_ffff_ffff_ffff));
    assert!(!is_canonical(0x0001_0000_0000_0000));
    assert!(is_canonical(UPPER_BASE));
    assert!(is_canonical(KERNEL_OFFSET));
    assert!(is_canonical(u64::MAX));
    assert!(!is_canonical(UPPER_BASE - 1));
}

#[test]
fn offset_translates_both_ways() {
    assert_eq!(virtual_of(0x4020_0000), 0xffff_ff80_4020_0000);
    assert_eq!(physical_of(virtual_of(0x1234_5000)), 0x1234_5000);
}

#[test]
fn mair_indexes_are_found_wherever_they_are() {
    assert_eq!(
        MemoryTypes::from_mair(DIRECT_ENTRY_MAIR),
        Some(MemoryTypes {
            normal_index: 1,
            device_index: 0
        })
    );
    // The firmware's MAIR_EL1: Device-nGnRnE at 0, Normal write-back at 3.
    let found = MemoryTypes::from_mair(0x0000_0000_ff44_0400).unwrap();
    assert_eq!((found.device_index, found.normal_index), (0, 3));
    assert_eq!(MemoryTypes::from_mair(0x4444_4444_4444_4444), None);
    assert_eq!(MemoryTypes::from_mair(0xffff_ffff_ffff_ffff), None);
}

#[test]
fn descriptors_carry_the_permissions_of_each_kind() {
    let bits = |permissions| leaf(&TYPES, 0x4020_0000, permissions, 3);
    let text = Leaf::decode(bits(Permissions::Text), 3);
    assert!(!text.writable && text.executable && !text.user_accessible);
    let rodata = Leaf::decode(bits(Permissions::ReadOnly), 3);
    assert!(!rodata.writable && !rodata.executable);
    let data = Leaf::decode(bits(Permissions::ReadWrite), 3);
    assert!(data.writable && !data.executable);
    let device = Leaf::decode(bits(Permissions::Device), 3);
    assert!(device.writable && !device.executable);
    assert_eq!(device.attribute_index, TYPES.device_index);
    assert_eq!(data.attribute_index, TYPES.normal_index);
    for permissions in [
        Permissions::Text,
        Permissions::ReadOnly,
        Permissions::ReadWrite,
        Permissions::Device,
    ] {
        let found = Leaf::decode(bits(permissions), 3);
        assert!(!found.user_executable && !found.user_accessible);
        assert_eq!(found.output, 0x4020_0000);
        assert_eq!(bits(permissions) & 3, 3, "a level 3 leaf is a page");
        assert_ne!(bits(permissions) & ACCESS_FLAG, 0);
    }
    assert_eq!(leaf(&TYPES, 0x4020_0000, Permissions::Text, 2) & 3, 1);
}

#[test]
fn blocks_keep_only_the_bits_of_their_size() {
    let block = leaf(&TYPES, 0x4030_1000, Permissions::ReadWrite, 2);
    assert_eq!(Leaf::decode(block, 2).output, 0x4020_0000);
}

#[test]
fn layout_is_checked() {
    let ok = |t: (u64, u64), r: (u64, u64), d: (u64, u64), s: (u64, u64), e: (u64, u64)| {
        ImageLayout::new(t.0..t.1, r.0..r.1, d.0..d.1, s.0..s.1, e.0..e.1)
    };
    let (t, r, d, s, e) = (
        (0, 4096),
        (4096, 8192),
        (8192, 12288),
        (16384, 20480),
        (24576, 28672),
    );
    assert!(ok(t, r, d, s, e).is_ok());
    assert_eq!(
        ok(t, r, d, (16384 + 1, 20480), e),
        Err(LayoutError::Unaligned)
    );
    assert_eq!(ok((0, 8192), r, d, s, e), Err(LayoutError::Misordered));
    assert_eq!(
        ok((0, 0), (0, 0), (0, 0), s, e),
        Err(LayoutError::Misordered)
    );
    assert_eq!(
        ok(t, r, d, (12288, 20480), e),
        Err(LayoutError::NoGuardPage)
    );
    // The exception stack needs its own guard page above the boot stack.
    assert_eq!(
        ok(t, r, d, s, (20480, 28672)),
        Err(LayoutError::NoGuardPage)
    );
}

#[test]
fn image_sections_get_their_own_permissions() {
    let mut arena = Arena::new(8);
    let root = upper(&mut arena);
    let at = |physical: u64| translate(&arena, root, physical + KERNEL_OFFSET).unwrap();
    let layout = layout();

    for (range, writable, executable) in [
        (&layout.text, false, true),
        (&layout.rodata, false, false),
        (&layout.data, true, false),
        (&layout.stack, true, false),
        (&layout.exception_stack, true, false),
    ] {
        for page in [range.start, range.end - PAGE_SIZE] {
            let found = at(page);
            assert_eq!(found.output, page);
            assert_eq!((found.writable, found.executable), (writable, executable));
        }
    }
    // Every byte of a page translates to the same offset in its frame.
    assert_eq!(
        at(layout.text.start + 0x123).output,
        layout.text.start + 0x123
    );
}

#[test]
fn the_guard_page_and_everything_around_the_image_is_unmapped() {
    let mut arena = Arena::new(8);
    let root = upper(&mut arena);
    let layout = layout();
    let guard = layout.data.end + KERNEL_OFFSET;
    assert!(translate(&arena, root, guard).is_none());
    assert!(translate(&arena, root, layout.stack.start + KERNEL_OFFSET - 1).is_none());
    // The page between the two stacks, and the one above the exception stack.
    assert!(translate(&arena, root, layout.stack.end + KERNEL_OFFSET).is_none());
    assert!(
        translate(
            &arena,
            root,
            layout.exception_stack.start + KERNEL_OFFSET - 1
        )
        .is_none()
    );
    assert!(translate(&arena, root, layout.exception_stack.end + KERNEL_OFFSET).is_none());
    assert!(translate(&arena, root, layout.text.start + KERNEL_OFFSET - 1).is_none());
    assert!(translate(&arena, root, 0x0001_0000_0000_0000).is_none());
}

#[test]
fn the_upper_half_audits_clean_and_counts_each_kind() {
    let mut arena = Arena::new(8);
    let root = upper(&mut arena);
    let layout = layout();
    let size = |r: &Range<u64>| r.end - r.start;
    let report = audit(&arena, root, UPPER_BASE, &in_image(open()));
    assert!(report.is_clean());
    assert_eq!(report.text, size(&layout.text));
    assert_eq!(report.read_only, size(&layout.rodata));
    assert_eq!(
        report.read_write,
        size(&layout.data) + size(&layout.stack) + size(&layout.exception_stack)
    );
    assert_eq!(report.device, 0);
}

#[test]
fn a_page_that_is_writable_and_executable_is_found() {
    let mut arena = Arena::new(8);
    let root = upper(&mut arena);
    let at = KERNEL_OFFSET + BASE;
    let table = page_table(&arena, root, at);
    arena.table_mut(table).0[level_index(at, 3)] &= !READ_ONLY;
    let report = audit(&arena, root, UPPER_BASE, &in_image(open()));
    assert_eq!(report.writable_executable, Some(at));
    assert!(!report.is_clean());
}

#[test]
fn a_page_for_el0_is_found() {
    let mut arena = Arena::new(8);
    let root = upper(&mut arena);
    let at = KERNEL_OFFSET + BASE + 0x22000;
    let table = page_table(&arena, root, at);
    arena.table_mut(table).0[level_index(at, 3)] &= !UNPRIVILEGED_EXECUTE_NEVER;
    let report = audit(&arena, root, UPPER_BASE, &in_image(open()));
    assert_eq!(report.user_reachable, Some(at));
    let table = page_table(&arena, root, at);
    arena.table_mut(table).0[level_index(at, 3)] |= UNPRIVILEGED_EXECUTE_NEVER | USER_ACCESSIBLE;
    let report = audit(&arena, root, UPPER_BASE, &in_image(open()));
    assert_eq!(report.user_reachable, Some(at));
}

fn ram_window(arena: &mut Arena, regions: &[(u64, u64)], hole: Range<u64>) -> u64 {
    let mut mapper = Mapper::new(arena, TYPES).unwrap();
    for &(start, end) in regions {
        map_ram_region(&mut mapper, start..end, &hole, 0).unwrap();
    }
    mapper.root()
}

#[test]
fn the_window_maps_ram_but_not_the_image_or_its_guard() {
    let mut arena = Arena::new(16);
    let ram = 0x4000_0000..0x4800_0000;
    let span = layout().span();
    let root = ram_window(&mut arena, &[(ram.start, ram.end)], span.clone());
    let at = |address: u64| translate(&arena, root, address);

    let below = at(0x4000_0000).unwrap();
    assert_eq!(below.output, 0x4000_0000);
    assert!(below.writable && !below.executable);
    for address in [
        span.start,
        span.start + 0x1000,
        span.end - 1,
        layout().data.end,
    ] {
        assert!(at(address).is_none(), "{address:#x} must not be mapped");
    }
    let after = at(span.end).unwrap();
    assert_eq!(after.output, span.end);
    assert!(after.writable && !after.executable);
    assert!(at(ram.end).is_none());
    assert!(at(ram.start - 1).is_none());

    let report = audit(
        &arena,
        root,
        0,
        &Expectation {
            forbidden: &span,
            ..open()
        },
    );
    assert!(report.is_clean());
    assert_eq!(
        report.read_write,
        ram.end - ram.start - (span.end - span.start)
    );
    assert_eq!(report.text, 0);
}

#[test]
fn an_alias_of_the_image_is_found() {
    let mut arena = Arena::new(16);
    let span = layout().span();
    let root = ram_window(&mut arena, &[(0x4000_0000, 0x4800_0000)], 0..0);
    let report = audit(
        &arena,
        root,
        0,
        &Expectation {
            forbidden: &span,
            ..open()
        },
    );
    assert!(report.aliased.is_some());
    assert!(!report.is_clean());
}

#[test]
fn aligned_ram_uses_blocks_and_edges_use_pages() {
    let mut arena = Arena::new(16);
    // 6 MiB aligned to 2 MiB: three blocks, so the root, a level 1 and a level
    // 2 table only.
    ram_window(&mut arena, &[(0x4000_0000, 0x4060_0000)], 0..0);
    assert_eq!(arena.tables.len(), 3);
    let mut arena = Arena::new(16);
    ram_window(&mut arena, &[(0x4000_1000, 0x4000_3000)], 0..0);
    assert_eq!(arena.tables.len(), 4, "two pages need a level 3 table");
}

#[test]
fn regions_are_widened_to_whole_pages_and_may_touch() {
    let mut arena = Arena::new(16);
    let root = ram_window(
        &mut arena,
        &[(0x4000_0100, 0x4000_0200), (0x4000_3f00, 0x4000_4001)],
        0..0,
    );
    assert!(translate(&arena, root, 0x4000_0000).is_some());
    assert!(translate(&arena, root, 0x4000_3000).is_some());
    assert!(translate(&arena, root, 0x4000_4000).is_some());
    assert!(translate(&arena, root, 0x4000_5000).is_none());
    assert!(translate(&arena, root, 0x4000_1000).is_none());
}

#[test]
fn ram_above_four_gib_is_mapped() {
    let mut arena = Arena::new(16);
    let root = ram_window(&mut arena, &[(0x1_0000_0000, 0x1_2000_0000)], 0..0);
    assert_eq!(
        translate(&arena, root, 0x1_1fff_f000).unwrap().output,
        0x1_1fff_f000
    );
}

#[test]
fn ram_beyond_the_regime_is_refused() {
    let mut arena = Arena::new(16);
    let mut mapper = Mapper::new(&mut arena, TYPES).unwrap();
    let beyond = 0x0001_0000_0000_0000..0x0001_0000_0020_0000;
    assert_eq!(
        map_ram_region(&mut mapper, beyond, &(0..0), 0),
        Err(MapError::NotCanonical)
    );
}

#[test]
fn device_registers_map_as_device_without_execute() {
    let mut arena = Arena::new(16);
    let mut mapper = Mapper::new(&mut arena, TYPES).unwrap();
    map_device(&mut mapper, 0x0900_0010, 0x1000).unwrap();
    let root = mapper.root();
    // 0x10 bytes into a page and 0x1000 long touch two pages.
    for address in [0x0900_0000, 0x0900_1000] {
        let found = translate(&arena, root, address).unwrap();
        assert_eq!(found.attribute_index, TYPES.device_index);
        assert!(found.writable && !found.executable);
    }
    assert!(translate(&arena, root, 0x0900_2000).is_none());
    let device = 0x0900_0000..0x0900_2000;
    let report = audit(
        &arena,
        root,
        0,
        &Expectation {
            device: &device,
            ..open()
        },
    );
    assert_eq!(report.device, 0x2000);
    assert!(report.is_clean());
}

#[test]
fn overlapping_maps_are_refused() {
    let mut arena = Arena::new(16);
    let mut mapper = Mapper::new(&mut arena, TYPES).unwrap();
    mapper
        .map_range(0x4000_0000, 0x4000_0000, BLOCK_SIZE, Permissions::ReadWrite)
        .unwrap();
    assert_eq!(
        mapper.map_range(0x4000_1000, 0x4000_1000, PAGE_SIZE, Permissions::Text),
        Err(MapError::Overlap)
    );
    assert_eq!(
        mapper.map_range(0x4000_0000, 0x5000_0000, BLOCK_SIZE, Permissions::Text),
        Err(MapError::Overlap)
    );
    mapper
        .map_range(0x4020_1000, 0x4020_1000, PAGE_SIZE, Permissions::ReadOnly)
        .unwrap();
    assert_eq!(
        mapper.map_range(0x4020_1000, 0x4020_1000, PAGE_SIZE, Permissions::Text),
        Err(MapError::Overlap)
    );
    // A block over a table of pages is an overlap, too.
    assert_eq!(
        mapper.map_range(0x4020_0000, 0x4020_0000, BLOCK_SIZE, Permissions::Text),
        Err(MapError::Overlap)
    );
}

#[test]
fn running_out_of_frames_is_reported() {
    let mut arena = Arena::new(2);
    let mut mapper = Mapper::new(&mut arena, TYPES).unwrap();
    assert_eq!(
        mapper.map_range(0x4000_0000, 0x4000_0000, PAGE_SIZE, Permissions::ReadWrite),
        Err(MapError::OutOfFrames)
    );
    assert_eq!(
        Mapper::new(&mut Arena::new(0), TYPES).err(),
        Some(MapError::OutOfFrames)
    );
}

#[test]
fn unaligned_and_non_canonical_ranges_are_refused() {
    let mut arena = Arena::new(16);
    let mut mapper = Mapper::new(&mut arena, TYPES).unwrap();
    assert_eq!(
        mapper.map_range(0x4000_0001, 0x4000_0000, PAGE_SIZE, Permissions::Text),
        Err(MapError::Unaligned)
    );
    assert_eq!(
        mapper.map_range(0x4000_0000, 0x4000_0000, 100, Permissions::Text),
        Err(MapError::Unaligned)
    );
    assert_eq!(
        mapper.map_range(0x0001_0000_0000_0000, 0, PAGE_SIZE, Permissions::Text),
        Err(MapError::NotCanonical)
    );
    // A range that starts in the lower half and crosses into the hole.
    assert_eq!(
        mapper.map_range(0x0000_ffff_ffff_f000, 0, 2 * PAGE_SIZE, Permissions::Text),
        Err(MapError::NotCanonical)
    );
    assert_eq!(
        mapper.map_range(u64::MAX - 0xfff, 0, 2 * PAGE_SIZE, Permissions::Text),
        Err(MapError::NotCanonical)
    );
    assert_eq!(
        mapper.map_range(0x4000_0000, 0, 0, Permissions::Text),
        Ok(())
    );
}

#[test]
fn a_malformed_entry_is_counted_and_a_gigabyte_block_is_read() {
    let mut arena = Arena::new(4);
    let root = arena.allocate().unwrap();
    let l1 = arena.allocate().unwrap();
    arena.table_mut(root).0[0] = table_descriptor(l1);
    arena.table_mut(root).0[1] = 0x2; // neither valid nor a table
    arena.table_mut(l1).0[0] = 0x2; // neither valid nor a table
    arena.table_mut(l1).0[1] = leaf(&TYPES, GIGABYTE, Permissions::ReadWrite, 2);
    arena.table_mut(l1).0[2] = leaf(&TYPES, 2 * GIGABYTE, Permissions::ReadWrite, 2);
    let report = audit(&arena, root, 0, &open());
    // A level 0 entry that is a block is as malformed as a level 1 entry that
    // is neither (a 4 KiB granule has no 512 GiB blocks).
    assert_eq!(report.malformed, 2);
    assert_eq!(report.read_write, 2 * GIGABYTE);
    let found = translate(&arena, root, 2 * GIGABYTE + 0x1234).unwrap();
    assert_eq!(found.output, 2 * GIGABYTE + 0x1234);
}

#[test]
fn the_register_fields_match_the_architecture() {
    // T0SZ 25, IRGN0 and ORGN0 write-back write-allocate, SH0 inner, TG0 4 KiB.
    assert_eq!(TCR_LOWER_HALF, 0x3519);
    // The same fields at 16 and up, with TG1 = 0b10 and T1SZ 16: 48 bits.
    assert_eq!(TCR_UPPER_HALF, 0xb510_0000);
    assert_eq!(TCR_LOWER_HALF & TCR_UPPER_HALF_MASK, 0);
    assert_eq!(TCR_UPPER_HALF & TCR_LOWER_HALF_MASK, 0);
    assert_eq!(TCR_EPD0 & TCR_LOWER_HALF_MASK, TCR_EPD0);
    assert_eq!(TCR_EPD1 & TCR_UPPER_HALF_MASK, TCR_EPD1);
    assert_eq!(SCTLR_WXN, 0x80000);
}

#[test]
fn the_pieces_the_boot_code_assembles_equal_what_leaf_builds() {
    let attribute = |index: u64| index << ATTRIBUTE_INDEX_SHIFT;
    let at = 0x4020_0000;
    let normal = TYPES.normal_index as u64;
    let device = TYPES.device_index as u64;
    assert_eq!(
        leaf(&TYPES, at, Permissions::Text, 3),
        at | attribute(normal) | NORMAL_PAGE_BITS | READ_ONLY
    );
    assert_eq!(
        leaf(&TYPES, at, Permissions::ReadOnly, 3),
        at | attribute(normal) | NORMAL_PAGE_BITS | READ_ONLY | PRIVILEGED_EXECUTE_NEVER
    );
    assert_eq!(
        leaf(&TYPES, at, Permissions::ReadWrite, 3),
        at | attribute(normal) | NORMAL_PAGE_BITS | PRIVILEGED_EXECUTE_NEVER
    );
    assert_eq!(
        leaf(&TYPES, at, Permissions::ReadWrite, 2),
        at | attribute(normal) | NORMAL_BLOCK_BITS
    );
    assert_eq!(
        leaf(&TYPES, 0x4000_0000, Permissions::Device, 2),
        0x4000_0000 | attribute(device) | DEVICE_BLOCK_BITS
    );
    assert_eq!(table_descriptor(at), at | TABLE_BITS);
    assert_eq!(TCR_BOTH_HALVES, 0xb510_3519);
}

#[test]
fn gigabyte_aligned_ram_uses_one_block_per_gigabyte() {
    let mut arena = Arena::new(16);
    // 4 GiB aligned to 1 GiB: four level 1 blocks and nothing below the level 1
    // table.
    let root = ram_window(&mut arena, &[(0x4000_0000, 0x1_4000_0000)], 0..0);
    assert_eq!(arena.tables.len(), 2);
    let found = translate(&arena, root, 0x1_3fff_f123).unwrap();
    assert_eq!(found.output, 0x1_3fff_f123);
    assert!(found.writable && !found.executable);
    let report = audit(&arena, root, 0, &open());
    assert_eq!(report.read_write, 4 << 30);
    assert!(report.is_clean());
    // A machine with 400 GiB of RAM needs two tables below the root, not hundreds.
    let mut arena = Arena::new(16);
    ram_window(
        &mut arena,
        &[(0x4000_0000, 0x4000_0000 + (400 << 30))],
        0..0,
    );
    assert_eq!(arena.tables.len(), 2);
}

#[test]
fn a_gigabyte_block_is_not_split_around_the_hole() {
    let mut arena = Arena::new(16);
    let span = layout().span();
    let root = ram_window(&mut arena, &[(0x4000_0000, 0x8000_0000)], span.clone());
    assert!(translate(&arena, root, span.start).is_none());
    assert!(translate(&arena, root, span.end).is_some());
    assert!(
        audit(
            &arena,
            root,
            0,
            &Expectation {
                forbidden: &span,
                ..open()
            }
        )
        .is_clean()
    );
}

#[test]
fn regions_that_share_a_page_map_it_once() {
    let mut arena = Arena::new(16);
    let root = ram_window(
        &mut arena,
        &[(0x4000_0000, 0x4000_0800), (0x4000_0800, 0x4000_1000)],
        0..0,
    );
    let found = translate(&arena, root, 0x4000_0000).unwrap();
    assert!(found.writable && !found.executable);
    // The same page asked for with other permissions is still a conflict.
    let mut mapper = Mapper::new(&mut arena, TYPES).unwrap();
    mapper
        .map_range(0x4000_0000, 0x4000_0000, PAGE_SIZE, Permissions::ReadWrite)
        .unwrap();
    assert_eq!(
        mapper.map_range(0x4000_0000, 0x4000_0000, PAGE_SIZE, Permissions::Text),
        Err(MapError::Overlap)
    );
}

#[test]
fn an_empty_region_maps_nothing() {
    let mut arena = Arena::new(16);
    let root = ram_window(&mut arena, &[(0x4000_0100, 0x4000_0100)], 0..0);
    assert!(translate(&arena, root, 0x4000_0000).is_none());
    assert_eq!(arena.tables.len(), 1);
}

#[test]
fn a_block_over_pages_an_earlier_region_mapped_is_split_not_refused() {
    let mut arena = Arena::new(16);
    // The first region ends inside the first page of the 2 MiB that the
    // second starts at, so that block is already a table when the second asks.
    let root = ram_window(
        &mut arena,
        &[(0x4000_0000, 0x4020_0800), (0x4020_0800, 0x8000_0000)],
        0..0,
    );
    for address in [0x4020_0000, 0x4020_1000, 0x4030_0000, 0x7fff_f000] {
        let found = translate(&arena, root, address).unwrap();
        assert_eq!(found.output, address);
        assert!(found.writable && !found.executable);
    }
    assert!(audit(&arena, root, 0, &open()).is_clean());
    // The same for a gigabyte: a region ends inside one a later region covers.
    let mut arena = Arena::new(16);
    let root = ram_window(
        &mut arena,
        &[(0x4000_0000, 0x4000_1800), (0x4000_1800, 0xc000_0000)],
        0..0,
    );
    assert_eq!(
        translate(&arena, root, 0xbfff_f000).unwrap().output,
        0xbfff_f000
    );
    assert_eq!(
        translate(&arena, root, 0x4000_1000).unwrap().output,
        0x4000_1000
    );
    // A conflicting permission under the table is still refused.
    let mut mapper = Mapper::new(&mut arena, TYPES).unwrap();
    mapper
        .map_range(0x4020_1000, 0x4020_1000, PAGE_SIZE, Permissions::Text)
        .unwrap();
    assert_eq!(
        mapper.map_range(0x4020_0000, 0x4020_0000, BLOCK_SIZE, Permissions::ReadWrite),
        Err(MapError::Overlap)
    );
}

#[test]
fn a_small_region_inside_a_mapped_block_is_accepted_when_identical() {
    let mut arena = Arena::new(16);
    let root = ram_window(
        &mut arena,
        &[
            (0x4000_0000, 0x8000_0000),
            (0x4800_0000, 0x4800_1000),
            (0x4020_0000, 0x4040_0000),
        ],
        0..0,
    );
    assert_eq!(arena.tables.len(), 2, "nothing was split");
    assert_eq!(
        translate(&arena, root, 0x4800_0800).unwrap().output,
        0x4800_0800
    );
    // The same place with another permission, or another output, is refused.
    let mut mapper = Mapper::new(&mut arena, TYPES).unwrap();
    mapper
        .map_range(0x4000_0000, 0x4000_0000, GIGABYTE, Permissions::ReadWrite)
        .unwrap();
    assert_eq!(
        mapper.map_range(0x4800_0000, 0x4800_0000, PAGE_SIZE, Permissions::Text),
        Err(MapError::Overlap)
    );
    assert_eq!(
        mapper.map_range(0x4800_0000, 0x5000_0000, PAGE_SIZE, Permissions::ReadWrite),
        Err(MapError::Overlap)
    );
}

// ---- The audit checks what each leaf maps and with which memory type ------

/// The expectation of the kernel's upper half: the image by section, nothing
/// else.
fn image_expectation(parts: &[(Range<u64>, Permissions)]) -> Expectation<'_> {
    in_image(Expectation {
        known: Some(parts),
        ..open()
    })
}

/// Changes the leaf of the page at `at` in the upper half.
fn tamper(arena: &mut Arena, root: u64, at: u64, change: impl FnOnce(u64) -> u64) {
    let table = page_table(arena, root, at);
    let slot = &mut arena.table_mut(table).0[level_index(at, 3)];
    *slot = change(*slot);
}

#[test]
fn the_image_audits_clean_against_its_sections() {
    let mut arena = Arena::new(8);
    let root = upper(&mut arena);
    let parts = layout().parts();
    let report = audit(&arena, root, UPPER_BASE, &image_expectation(&parts));
    assert!(report.is_clean(), "{report:?}");
}

#[test]
fn a_page_that_maps_another_frame_is_found() {
    let mut arena = Arena::new(8);
    let root = upper(&mut arena);
    let at = KERNEL_OFFSET + BASE + 0x23000;
    // One page further: still inside the data, so only the address is wrong.
    tamper(&mut arena, root, at, |d| d + PAGE_SIZE);
    let parts = layout().parts();
    let report = audit(&arena, root, UPPER_BASE, &image_expectation(&parts));
    assert_eq!(report.wrong_output, Some(at));
    assert_eq!(report.wrong_section, None);
    assert!(!report.is_clean());
}

#[test]
fn a_page_that_maps_the_wrong_section_is_found() {
    let mut arena = Arena::new(8);
    let root = upper(&mut arena);
    // The first page of `.data` made to map the first page of `.rodata`, with
    // the permissions of data: the totals stay as they are.
    let at = KERNEL_OFFSET + BASE + 0x22000;
    tamper(&mut arena, root, at, |d| d - 5 * PAGE_SIZE);
    let parts = layout().parts();
    let report = audit(&arena, root, UPPER_BASE, &image_expectation(&parts));
    assert_eq!(report.wrong_output, Some(at));
    assert_eq!(report.wrong_section, Some(at));
    assert_eq!(report.read_write, 0x42000 + 0x10000 + 0x4000);
}

#[test]
fn permissions_swapped_between_sections_are_found_though_the_totals_hold() {
    let mut arena = Arena::new(8);
    let root = upper(&mut arena);
    // Two pages exchange their permission bits: one text page becomes read
    // only, one read only page becomes text. The sizes by kind are unchanged.
    let (text_page, rodata_page) = (KERNEL_OFFSET + BASE, KERNEL_OFFSET + BASE + 0x1d000);
    let set_bits = |wanted: u64| {
        move |d: u64| {
            let bits = READ_ONLY | PRIVILEGED_EXECUTE_NEVER;
            (d & !bits) | wanted
        }
    };
    let before = {
        let parts = layout().parts();
        audit(&arena, root, UPPER_BASE, &image_expectation(&parts))
    };
    tamper(
        &mut arena,
        root,
        text_page,
        set_bits(READ_ONLY | PRIVILEGED_EXECUTE_NEVER),
    );
    tamper(&mut arena, root, rodata_page, set_bits(READ_ONLY));
    let parts = layout().parts();
    let report = audit(&arena, root, UPPER_BASE, &image_expectation(&parts));
    assert_eq!(
        (report.text, report.read_only),
        (before.text, before.read_only)
    );
    assert_eq!(report.wrong_section, Some(text_page));
}

#[test]
fn a_memory_type_that_is_not_normal_is_found_in_the_image() {
    for (name, index) in [("device", TYPES.device_index), ("unused", 5)] {
        let mut arena = Arena::new(8);
        let root = upper(&mut arena);
        let at = KERNEL_OFFSET + BASE + 0x10000;
        tamper(&mut arena, root, at, |d| {
            (d & !ATTRIBUTE_INDEX_MASK) | (u64::from(index) << ATTRIBUTE_INDEX_SHIFT)
        });
        let report = audit(&arena, root, UPPER_BASE, &in_image(open()));
        assert_eq!(report.wrong_memory_type, Some(at), "{name}");
        assert!(!report.is_clean());
    }
}

#[test]
fn the_device_range_is_device_memory_and_nothing_else_is() {
    let uart = 0x0900_0000..0x0900_1000;
    let expectation = Expectation {
        device: &uart,
        ..open()
    };
    let mut arena = Arena::new(16);
    let mut mapper = Mapper::new(&mut arena, TYPES).unwrap();
    map_device(&mut mapper, uart.start, 0x1000).unwrap();
    map_ram_region(&mut mapper, 0x4000_0000..0x4001_0000, &(0..0), 0).unwrap();
    let root = mapper.root();
    assert!(audit(&arena, root, 0, &expectation).is_clean());

    // The UART mapped as Normal memory.
    tamper(&mut arena, root, uart.start, |d| {
        (d & !ATTRIBUTE_INDEX_MASK) | (u64::from(TYPES.normal_index) << ATTRIBUTE_INDEX_SHIFT)
    });
    assert_eq!(
        audit(&arena, root, 0, &expectation).wrong_memory_type,
        Some(uart.start)
    );
    // Restored, and RAM mapped as Device instead.
    tamper(&mut arena, root, uart.start, |d| {
        (d & !ATTRIBUTE_INDEX_MASK) | (u64::from(TYPES.device_index) << ATTRIBUTE_INDEX_SHIFT)
    });
    tamper(&mut arena, root, 0x4000_8000, |d| {
        (d & !ATTRIBUTE_INDEX_MASK) | (u64::from(TYPES.device_index) << ATTRIBUTE_INDEX_SHIFT)
    });
    assert_eq!(
        audit(&arena, root, 0, &expectation).wrong_memory_type,
        Some(0x4000_8000)
    );
}

#[test]
fn a_window_page_that_maps_another_frame_is_found() {
    let mut arena = Arena::new(16);
    let root = ram_window(&mut arena, &[(0x4000_0000, 0x4001_0000)], 0..0);
    let at = 0x4000_3000;
    tamper(&mut arena, root, at, |d| d + PAGE_SIZE);
    let report = audit(&arena, root, 0, &open());
    assert_eq!(report.wrong_output, Some(at));
}

#[test]
fn a_leaf_outside_the_known_ranges_is_found() {
    let mut arena = Arena::new(8);
    let root = upper(&mut arena);
    let mut parts = layout().parts();
    parts[4].0 = parts[4].0.start..parts[4].0.end - PAGE_SIZE;
    let report = audit(&arena, root, UPPER_BASE, &image_expectation(&parts));
    assert_eq!(
        report.wrong_section,
        Some(KERNEL_OFFSET + layout().exception_stack.end - PAGE_SIZE)
    );
}

#[test]
fn leaf_permissions_are_read_back_from_the_bits() {
    for permissions in [
        Permissions::Text,
        Permissions::ReadOnly,
        Permissions::ReadWrite,
        Permissions::Device,
    ] {
        let found = Leaf::decode(leaf(&TYPES, 0x4000_0000, permissions, 3), 3);
        assert_eq!(found.permissions(TYPES), Some(permissions));
    }
    let both = leaf(&TYPES, 0, Permissions::ReadWrite, 3) & !PRIVILEGED_EXECUTE_NEVER;
    assert_eq!(Leaf::decode(both, 3).permissions(TYPES), None);
}

#[test]
fn the_heap_window_is_in_the_upper_half_and_clear_of_the_image() {
    let image = layout().span();
    let window = HEAP_BASE..HEAP_BASE + HEAP_WINDOW;
    assert!(is_canonical(window.start) && is_canonical(window.end - 1));
    assert!(window.start >= KERNEL_OFFSET);
    assert!(window.start > image.end + KERNEL_OFFSET + (1 << 30));
    assert!(HEAP_BASE.is_multiple_of(GIGABYTE));
}

#[test]
fn heap_pages_added_to_the_live_tables_are_read_write_never_executable() {
    let mut arena = Arena::new(64);
    let root = upper(&mut arena);
    let frames = [0x8000_0000u64, 0x8000_5000, 0x9000_0000];
    for (page, &frame) in frames.iter().enumerate() {
        let mut mapper = Mapper::resume(&mut arena, TYPES, root);
        let at = HEAP_BASE + page as u64 * PAGE_SIZE;
        mapper
            .map_range(at, frame, PAGE_SIZE, Permissions::ReadWrite)
            .unwrap();
        let leaf = translate(&arena, root, at).unwrap();
        assert_eq!(leaf.output, frame);
        assert_eq!(leaf.permissions(TYPES), Some(Permissions::ReadWrite));
    }
    // The pages around the mapped part are the guard pages.
    assert!(translate(&arena, root, HEAP_BASE - PAGE_SIZE).is_none());
    assert!(translate(&arena, root, HEAP_BASE + 3 * PAGE_SIZE).is_none());
    // The image is where it was.
    assert!(translate(&arena, root, virtual_of(BASE)).is_some());
    // The image's audit does not see the heap: its window is the image's own.
    assert!(audit(&arena, root, UPPER_BASE, &in_image(open())).is_clean());
    // The heap is a window of its own, where any frame is expected, and a root
    // with the heap in it has a stray unless the heap is listed (ADR 0029, 0033).
    let heap = Window {
        virt: HEAP_BASE..HEAP_BASE + HEAP_WINDOW,
        offset: None,
    };
    let report = audit(
        &arena,
        root,
        UPPER_BASE,
        &Expectation {
            window: Some(&heap),
            ..open()
        },
    );
    assert!(report.is_clean(), "{report:?}");
    assert_eq!(report.read_write, 3 * PAGE_SIZE);
    assert_eq!(
        strays(
            &arena,
            root,
            UPPER_BASE,
            core::slice::from_ref(&IMAGE_WINDOW.virt)
        ),
        (3, Some(HEAP_BASE))
    );
}

#[test]
fn growing_the_heap_cannot_remap_a_page_and_runs_out_of_table_frames_cleanly() {
    let mut arena = Arena::new(64);
    let root = upper(&mut arena);
    let mut mapper = Mapper::resume(&mut arena, TYPES, root);
    mapper
        .map_range(HEAP_BASE, 0x8000_0000, PAGE_SIZE, Permissions::ReadWrite)
        .unwrap();
    assert_eq!(
        mapper.map_range(HEAP_BASE, 0x8000_1000, PAGE_SIZE, Permissions::ReadWrite),
        Err(MapError::Overlap)
    );
    // No frame left for the tables a page 1 GiB away needs.
    let used = arena.tables.len();
    arena.limit = used;
    let mut mapper = Mapper::resume(&mut arena, TYPES, root);
    assert_eq!(
        mapper.map_range(
            HEAP_BASE + HEAP_WINDOW - PAGE_SIZE,
            0x8000_0000,
            PAGE_SIZE,
            Permissions::ReadWrite
        ),
        Err(MapError::OutOfFrames)
    );
}

#[test]
fn the_walked_descriptors_are_the_ones_the_mapping_wrote() {
    let mut arena = Arena::new(64);
    let root = upper(&mut arena);
    let mut mapper = Mapper::resume(&mut arena, TYPES, root);
    mapper
        .map_range(HEAP_BASE, 0x8000_0000, PAGE_SIZE, Permissions::ReadWrite)
        .unwrap();
    let walked = walked_descriptors(&arena, root, HEAP_BASE);
    let [Some(l0), Some(l1), Some(l2), Some(l3)] = walked else {
        panic!("a mapped page has four descriptors: {walked:?}");
    };
    assert_eq!(l0, root + 8 * level_index(HEAP_BASE, 0) as u64);
    let read = |address: u64| {
        arena.table(address & !(PAGE_SIZE - 1)).0[((address & (PAGE_SIZE - 1)) / 8) as usize]
    };
    assert_eq!(read(l0) & 3, 3);
    assert_eq!(read(l1) & 3, 3);
    assert_eq!(read(l2) & 3, 3);
    assert_eq!(read(l3) & ADDRESS_MASK, 0x8000_0000);
    // An unmapped address stops at the first empty descriptor.
    let nothing = walked_descriptors(&arena, root, HEAP_BASE + (32 << 30));
    assert!(nothing[1].is_some() && nothing[2].is_none());
    assert_eq!(walked_descriptors(&arena, root, 1 << 60), [None; 4]);
}

#[test]
fn mapping_pages_in_the_heap_window_takes_no_more_tables_than_the_formula_says() {
    for (start, pages) in [
        (0usize, 1usize),
        (511, 2),
        (0, 512),
        (1, 512),
        (300, 1000),
        (0, 5000),
    ] {
        let mut arena = Arena::new(10_000);
        let root = upper(&mut arena);
        let before = arena.tables.len();
        let mut mapper = Mapper::resume(&mut arena, TYPES, root);
        mapper
            .map_range(
                HEAP_BASE + start as u64 * PAGE_SIZE,
                0x8000_0000,
                pages as u64 * PAGE_SIZE,
                Permissions::ReadWrite,
            )
            .unwrap();
        let tables = arena.tables.len() - before;
        assert!(
            tables + pages <= frames_to_map(pages),
            "{pages} pages at {start} took {tables} tables, formula allows {}",
            frames_to_map(pages) - pages
        );
    }
}

// ---- The linear map, the devices and the windows (ADR 0033) --------------

static LINEAR_WINDOW: Window = Window {
    virt: LINEAR_BASE..LINEAR_BASE + LINEAR_SPAN,
    offset: Some(LINEAR_BASE),
};

static DEVICE_WINDOW_RANGE: Window = Window {
    virt: DEVICE_BASE..DEVICE_BASE + DEVICE_WINDOW,
    offset: None,
};

/// The upper half the kernel builds: image, linear map of `regions`, and the
/// device pages at `uart`; returns the root and where the UART landed.
fn full_upper(arena: &mut Arena, regions: &[(u64, u64)], uart: u64) -> (u64, u64) {
    let mut mapper = Mapper::new(arena, TYPES).unwrap();
    layout().map(&mut mapper, KERNEL_OFFSET).unwrap();
    for &(start, end) in regions {
        map_linear_region(&mut mapper, start..end, &layout().span()).unwrap();
    }
    let at = DeviceWindow::new().map(&mut mapper, uart, 0x1000).unwrap();
    (mapper.root(), at)
}

#[test]
fn the_layout_constants_keep_the_windows_apart() {
    assert_eq!(UPPER_BASE, 0xffff_0000_0000_0000);
    assert_eq!(LINEAR_BASE, UPPER_BASE);
    // The linear map ends below the last 512 GiB, where the image, the devices
    // and the heap are, so they never meet.
    const { assert!(LINEAR_BASE + LINEAR_SPAN <= KERNEL_OFFSET) };
    assert_eq!(level_index(LINEAR_BASE + LINEAR_SPAN - 1, 0), 255);
    assert_eq!(level_index(KERNEL_OFFSET, 0), 511);
    // Inside the last 512 GiB: the image's gigabytes, then the devices, then the heap.
    const { assert!(KERNEL_OFFSET + (1 << 30) * 2 <= DEVICE_BASE) };
    const { assert!(DEVICE_BASE + DEVICE_WINDOW < HEAP_BASE) };
    // The heap's last byte exists: its window does not wrap past 2^64.
    assert!(HEAP_BASE.checked_add(HEAP_WINDOW).is_some());
    assert_eq!(linear_of(0x4000_0000), LINEAR_BASE + 0x4000_0000);
}

#[test]
fn ram_above_512_gib_gets_a_linear_address_the_image_offset_could_not() {
    // `KERNEL_OFFSET + physical` overflows 2^64 from 512 GiB up.
    assert!(KERNEL_OFFSET.checked_add(600 << 30).is_none());
    let mut arena = Arena::new(64);
    // 520 GiB of RAM from 1 GiB: it ends at 521 GiB.
    let ram = 0x4000_0000..0x4000_0000 + (520 << 30);
    let (root, _) = full_upper(&mut arena, &[(ram.start, ram.end)], 0x0900_0000);
    for physical in [
        ram.start,
        0x7f_ffff_f000,
        0x80_0000_0000,
        ram.end - PAGE_SIZE,
    ] {
        let found = translate(&arena, root, linear_of(physical)).unwrap();
        assert_eq!(found.output, physical);
        assert!(found.writable && !found.executable);
    }
    assert!(translate(&arena, root, linear_of(ram.end)).is_none());
    assert!(translate(&arena, root, linear_of(0)).is_none());

    let report = audit(
        &arena,
        root,
        UPPER_BASE,
        &Expectation {
            window: Some(&LINEAR_WINDOW),
            forbidden: &layout().span(),
            ..open()
        },
    );
    assert!(report.is_clean(), "{report:?}");
    assert_eq!(
        report.read_write,
        (ram.end - ram.start) - (layout().span().end - layout().span().start)
    );
    // The image's frames are not writable through the linear map.
    assert!(translate(&arena, root, linear_of(layout().text.start)).is_none());
    assert!(translate(&arena, root, linear_of(layout().data.start)).is_none());
}

#[test]
fn a_machine_that_cannot_be_reached_by_the_linear_map_is_refused() {
    let mut arena = Arena::new(16);
    let mut mapper = Mapper::new(&mut arena, TYPES).unwrap();
    let last = LINEAR_SPAN - 0x20_0000..LINEAR_SPAN;
    assert_eq!(map_linear_region(&mut mapper, last, &(0..0)), Ok(()));
    let beyond = LINEAR_SPAN - 0x10_0000..LINEAR_SPAN + 0x10_0000;
    assert_eq!(
        map_linear_region(&mut mapper, beyond, &(0..0)),
        Err(MapError::BeyondLinearMap)
    );
    let far = 1 << 47 | 0x4000_0000;
    assert_eq!(
        map_linear_region(&mut mapper, far..far + 0x20_0000, &(0..0)),
        Err(MapError::BeyondLinearMap)
    );
    assert_eq!(map_linear_region(&mut mapper, 5..5, &(0..0)), Ok(()));
}

#[test]
fn the_image_window_the_linear_window_and_the_devices_audit_clean_apart() {
    let mut arena = Arena::new(64);
    let ram = 0x4000_0000..0x4800_0000;
    let (root, uart) = full_upper(&mut arena, &[(ram.start, ram.end)], 0x0900_0000);
    let parts = layout().parts();
    let image = audit(&arena, root, UPPER_BASE, &image_expectation(&parts));
    assert!(image.is_clean(), "{image:?}");
    assert_eq!(image.text, layout().text.end - layout().text.start);
    let device_range = 0x0900_0000..0x0900_1000;
    let devices = audit(
        &arena,
        root,
        UPPER_BASE,
        &Expectation {
            window: Some(&DEVICE_WINDOW_RANGE),
            device: &device_range,
            ..open()
        },
    );
    assert!(devices.is_clean(), "{devices:?}");
    assert_eq!(devices.device, PAGE_SIZE);
    assert_eq!(devices.read_write + devices.text + devices.read_only, 0);
    assert_eq!(uart, DEVICE_BASE);
    let windows = [
        IMAGE_WINDOW.virt.clone(),
        LINEAR_WINDOW.virt.clone(),
        DEVICE_WINDOW_RANGE.virt.clone(),
    ];
    assert_eq!(strays(&arena, root, UPPER_BASE, &windows), (0, None));
}

#[test]
fn a_leaf_outside_every_window_is_a_stray() {
    let mut arena = Arena::new(64);
    let (root, _) = full_upper(&mut arena, &[(0x4000_0000, 0x4800_0000)], 0x0900_0000);
    let windows = [
        IMAGE_WINDOW.virt.clone(),
        LINEAR_WINDOW.virt.clone(),
        DEVICE_WINDOW_RANGE.virt.clone(),
    ];
    // The heap's window is none of the three: a heap page is a stray until the
    // heap is in the list.
    let mut mapper = Mapper::resume(&mut arena, TYPES, root);
    mapper
        .map_range(HEAP_BASE, 0x8000_0000, PAGE_SIZE, Permissions::ReadWrite)
        .unwrap();
    assert_eq!(
        strays(&arena, root, UPPER_BASE, &windows),
        (1, Some(HEAP_BASE))
    );
    let mut with_heap = windows.to_vec();
    with_heap.push(HEAP_BASE..HEAP_BASE + HEAP_WINDOW);
    assert_eq!(strays(&arena, root, UPPER_BASE, &with_heap), (0, None));
}

#[test]
fn a_root_with_no_mappings_has_no_strays_and_one_leaf_below_is_found() {
    let mut arena = Arena::new(16);
    let root = Mapper::new(&mut arena, TYPES).unwrap().root();
    assert_eq!(strays(&arena, root, 0, &[]), (0, None));
    assert!(audit(&arena, root, 0, &open()).is_clean());
    // One identity page, as the old lower window held.
    let mut mapper = Mapper::resume(&mut arena, TYPES, root);
    mapper
        .map_range(0x0900_0000, 0x0900_0000, PAGE_SIZE, Permissions::Device)
        .unwrap();
    assert_eq!(strays(&arena, root, 0, &[]), (1, Some(0x0900_0000)));
}

#[test]
fn a_linear_leaf_that_maps_another_frame_is_found() {
    let mut arena = Arena::new(64);
    let (root, _) = full_upper(&mut arena, &[(0x4000_0000, 0x4010_0000)], 0x0900_0000);
    // A 2 MiB-aligned region of one block: make the block point elsewhere.
    let at = linear_of(0x4000_0000);
    let l1 = arena.table(root).0[level_index(at, 0)] & ADDRESS_MASK;
    let l2 = arena.table(l1).0[level_index(at, 1)] & ADDRESS_MASK;
    let l3 = arena.table(l2).0[level_index(at, 2)] & ADDRESS_MASK;
    arena.table_mut(l3).0[0] += PAGE_SIZE;
    let report = audit(
        &arena,
        root,
        UPPER_BASE,
        &Expectation {
            window: Some(&LINEAR_WINDOW),
            ..open()
        },
    );
    assert_eq!(report.wrong_output, Some(at));
}

#[test]
fn device_registers_land_in_the_window_with_a_guard_page_after_each() {
    let mut arena = Arena::new(64);
    let mut mapper = Mapper::new(&mut arena, TYPES).unwrap();
    let mut window = DeviceWindow::new();
    assert_eq!(window.next(), DEVICE_BASE);
    // 0x10 bytes into a page, 0x1000 long: two pages, the offset kept.
    let first = window.map(&mut mapper, 0x0900_0010, 0x1000).unwrap();
    assert_eq!(first, DEVICE_BASE + 0x10);
    assert_eq!(window.next(), DEVICE_BASE + 3 * PAGE_SIZE);
    let second = window.map(&mut mapper, 0x0801_0000, 0x10000).unwrap();
    assert_eq!(second, DEVICE_BASE + 3 * PAGE_SIZE);
    let root = mapper.root();
    for (address, output) in [
        (DEVICE_BASE, 0x0900_0000),
        (DEVICE_BASE + PAGE_SIZE, 0x0900_1000),
        (second, 0x0801_0000),
        (second + 15 * PAGE_SIZE, 0x0801_f000),
    ] {
        let found = translate(&arena, root, address).unwrap();
        assert_eq!(found.output, output);
        assert_eq!(found.attribute_index, TYPES.device_index);
        assert!(found.writable && !found.executable);
    }
    // The pages after each mapping are unmapped.
    assert!(translate(&arena, root, DEVICE_BASE + 2 * PAGE_SIZE).is_none());
    assert!(translate(&arena, root, second + 16 * PAGE_SIZE).is_none());
}

#[test]
fn the_device_window_refuses_what_does_not_fit_or_has_no_length() {
    let mut arena = Arena::new(64);
    let mut mapper = Mapper::new(&mut arena, TYPES).unwrap();
    let mut window = DeviceWindow::new();
    assert_eq!(
        window.map(&mut mapper, 0x0900_0000, 0),
        Err(MapError::Empty)
    );
    // One page less than the window, with its guard, fits; one byte more does not.
    let fits = DEVICE_WINDOW - 2 * PAGE_SIZE;
    assert_eq!(
        window.map(&mut mapper, 0x1000_0000, fits + PAGE_SIZE + 1),
        Err(MapError::WindowFull)
    );
    assert_eq!(window.next(), DEVICE_BASE, "a refusal maps nothing");
    window
        .map(&mut mapper, 0x1000_0000, fits + PAGE_SIZE)
        .unwrap();
    assert_eq!(window.next(), DEVICE_BASE + DEVICE_WINDOW);
    assert_eq!(
        window.map(&mut mapper, 0x0900_0000, 1),
        Err(MapError::WindowFull)
    );
}

#[test]
fn a_block_that_starts_before_the_window_is_not_judged_by_it() {
    let mut arena = Arena::new(64);
    let root = upper(&mut arena);
    let mut mapper = Mapper::resume(&mut arena, TYPES, root);
    // A 2 MiB block that overlaps the window but starts before it.
    mapper
        .map_range(HEAP_BASE, 0x8000_0000, BLOCK_SIZE, Permissions::ReadWrite)
        .unwrap();
    let window = Window {
        virt: HEAP_BASE + 0x1000..HEAP_BASE + 0x3000,
        offset: None,
    };
    let report = audit(
        &arena,
        root,
        UPPER_BASE,
        &Expectation {
            window: Some(&window),
            ..open()
        },
    );
    assert_eq!(report.read_write, 0, "{report:?}");
    // ... and it is a stray of any list of windows that does not start it.
    assert_eq!(
        strays(
            &arena,
            root,
            UPPER_BASE,
            &[IMAGE_WINDOW.virt.clone(), window.virt.clone()]
        ),
        (1, Some(HEAP_BASE))
    );
}

#[test]
fn a_hostile_device_length_is_a_full_window_not_an_overflow() {
    let mut arena = Arena::new(16);
    let mut mapper = Mapper::new(&mut arena, TYPES).unwrap();
    let mut window = DeviceWindow::new();
    assert_eq!(
        window.map(&mut mapper, 0x0900_0000, 0xffff_ffff_f000_0000),
        Err(MapError::NotCanonical)
    );
    assert_eq!(
        window.map(&mut mapper, 0x0900_0000, 1 << 40),
        Err(MapError::WindowFull)
    );
    assert_eq!(window.next(), DEVICE_BASE);
}

#[test]
fn a_device_beyond_48_physical_bits_is_refused() {
    let mut arena = Arena::new(16);
    let mut mapper = Mapper::new(&mut arena, TYPES).unwrap();
    let mut window = DeviceWindow::new();
    assert_eq!(
        window.map(&mut mapper, 0x0001_0000_0900_0000, 0x1000),
        Err(MapError::NotCanonical)
    );
    assert_eq!(window.next(), DEVICE_BASE);
}
