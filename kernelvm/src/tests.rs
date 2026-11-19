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
    )
    .unwrap()
}

fn no_alias() -> Range<u64> {
    0..0
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
    let l2 = arena.table(root).0[level_index(virtual_address, 1)] & ADDRESS_MASK;
    arena.table(l2).0[level_index(virtual_address, 2)] & ADDRESS_MASK
}

#[test]
fn indexes_follow_the_bits_of_the_address() {
    let address = KERNEL_OFFSET + 0x4021_d000;
    assert_eq!(level_index(address, 1), 1);
    assert_eq!(level_index(address, 2), 1);
    assert_eq!(level_index(address, 3), 0x1d);
    assert_eq!(level_index(0x7f_ffff_f000, 1), 511);
    assert_eq!(level_index(0x7f_ffff_f000, 3), 511);
}

#[test]
fn canonical_addresses_are_the_two_halves() {
    assert!(is_canonical(0));
    assert!(is_canonical(0x7f_ffff_ffff));
    assert!(!is_canonical(0x80_0000_0000));
    assert!(is_canonical(KERNEL_OFFSET));
    assert!(is_canonical(u64::MAX));
    assert!(!is_canonical(KERNEL_OFFSET - 1));
    assert!(!is_canonical(0x0001_0000_0000_0000));
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
    let ok = |t: (u64, u64), r: (u64, u64), d: (u64, u64), s: (u64, u64)| {
        ImageLayout::new(t.0..t.1, r.0..r.1, d.0..d.1, s.0..s.1)
    };
    assert!(ok((0, 4096), (4096, 8192), (8192, 12288), (16384, 20480)).is_ok());
    assert_eq!(
        ok((0, 4096), (4096, 8192), (8192, 12288), (16384 + 1, 20480)),
        Err(LayoutError::Unaligned)
    );
    assert_eq!(
        ok((0, 8192), (4096, 8192), (8192, 12288), (16384, 20480)),
        Err(LayoutError::Misordered)
    );
    assert_eq!(
        ok((0, 0), (0, 0), (0, 0), (16384, 20480)),
        Err(LayoutError::Misordered)
    );
    assert_eq!(
        ok((0, 4096), (4096, 8192), (8192, 12288), (12288, 20480)),
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
    assert!(translate(&arena, root, layout.stack.end + KERNEL_OFFSET).is_none());
    assert!(translate(&arena, root, layout.text.start + KERNEL_OFFSET - 1).is_none());
    assert!(translate(&arena, root, 0x0001_0000_0000_0000).is_none());
}

#[test]
fn the_upper_half_audits_clean_and_counts_each_kind() {
    let mut arena = Arena::new(8);
    let root = upper(&mut arena);
    let layout = layout();
    let size = |r: &Range<u64>| r.end - r.start;
    let report = audit(&arena, root, KERNEL_OFFSET, TYPES, &no_alias());
    assert!(report.is_clean());
    assert_eq!(report.text, size(&layout.text));
    assert_eq!(report.read_only, size(&layout.rodata));
    assert_eq!(report.read_write, size(&layout.data) + size(&layout.stack));
    assert_eq!(report.device, 0);
}

#[test]
fn a_page_that_is_writable_and_executable_is_found() {
    let mut arena = Arena::new(8);
    let root = upper(&mut arena);
    let at = KERNEL_OFFSET + BASE;
    let table = page_table(&arena, root, at);
    arena.table_mut(table).0[level_index(at, 3)] &= !READ_ONLY;
    let report = audit(&arena, root, KERNEL_OFFSET, TYPES, &no_alias());
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
    let report = audit(&arena, root, KERNEL_OFFSET, TYPES, &no_alias());
    assert_eq!(report.user_reachable, Some(at));
    let table = page_table(&arena, root, at);
    arena.table_mut(table).0[level_index(at, 3)] |= UNPRIVILEGED_EXECUTE_NEVER | USER_ACCESSIBLE;
    let report = audit(&arena, root, KERNEL_OFFSET, TYPES, &no_alias());
    assert_eq!(report.user_reachable, Some(at));
}

fn ram_window(arena: &mut Arena, regions: &[(u64, u64)], hole: Range<u64>) -> u64 {
    let mut mapper = Mapper::new(arena, TYPES).unwrap();
    for &(start, end) in regions {
        map_ram_region(&mut mapper, start..end, &hole).unwrap();
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

    let report = audit(&arena, root, 0, TYPES, &span);
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
    let report = audit(&arena, root, 0, TYPES, &span);
    assert!(report.aliased.is_some());
    assert!(!report.is_clean());
}

#[test]
fn aligned_ram_uses_blocks_and_edges_use_pages() {
    let mut arena = Arena::new(16);
    // 6 MiB aligned to 2 MiB: three blocks, so root, level 2 table only.
    ram_window(&mut arena, &[(0x4000_0000, 0x4060_0000)], 0..0);
    assert_eq!(arena.tables.len(), 2);
    let mut arena = Arena::new(16);
    ram_window(&mut arena, &[(0x4000_1000, 0x4000_3000)], 0..0);
    assert_eq!(arena.tables.len(), 3, "two pages need a level 3 table");
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
    let beyond = 0x80_0000_0000..0x80_0020_0000;
    assert_eq!(
        map_ram_region(&mut mapper, beyond, &(0..0)),
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
    let report = audit(&arena, root, 0, TYPES, &no_alias());
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
        mapper.map_range(0x80_0000_0000, 0, PAGE_SIZE, Permissions::Text),
        Err(MapError::NotCanonical)
    );
    // A range that starts in the lower half and crosses into the hole.
    assert_eq!(
        mapper.map_range(0x7f_ffff_f000, 0, 2 * PAGE_SIZE, Permissions::Text),
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
    arena.table_mut(root).0[0] = 0x2; // neither valid nor a table
    arena.table_mut(root).0[1] = leaf(&TYPES, GIGABYTE, Permissions::ReadWrite, 2);
    arena.table_mut(root).0[2] = leaf(&TYPES, 2 * GIGABYTE, Permissions::ReadWrite, 2);
    let report = audit(&arena, root, 0, TYPES, &no_alias());
    assert_eq!(report.malformed, 1);
    assert_eq!(report.read_write, 2 * GIGABYTE);
    let found = translate(&arena, root, 2 * GIGABYTE + 0x1234).unwrap();
    assert_eq!(found.output, 2 * GIGABYTE + 0x1234);
}

#[test]
fn the_register_fields_match_the_architecture() {
    // T0SZ 25, IRGN0 and ORGN0 write-back write-allocate, SH0 inner, TG0 4 KiB.
    assert_eq!(TCR_LOWER_HALF, 0x3519);
    // The same fields at 16 and up, with TG1 = 0b10.
    assert_eq!(TCR_UPPER_HALF, 0xb519_0000);
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
    assert_eq!(TCR_BOTH_HALVES, 0xb519_3519);
}
