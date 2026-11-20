use std::alloc::{Layout as StdLayout, alloc as std_alloc, dealloc as std_dealloc};
use std::collections::BTreeMap;

use super::*;

/// A page-aligned buffer standing in for mapped heap memory.
struct Arena {
    base: *mut u8,
    length: usize,
    layout: StdLayout,
}

impl Arena {
    fn new(length: usize) -> Self {
        let layout = StdLayout::from_size_align(length, 4096).unwrap();
        // SAFETY: a non-zero layout.
        let base = unsafe { std_alloc(layout) };
        assert!(!base.is_null());
        Self {
            base,
            length,
            layout,
        }
    }

    fn heap(&self, length: usize) -> Heap {
        assert!(length <= self.length);
        let mut heap = Heap::empty();
        // SAFETY: the arena outlives the heap in every test and nothing else uses it.
        unsafe { heap.init(self.base, length) }.unwrap();
        heap
    }
}

impl Drop for Arena {
    fn drop(&mut self) {
        // SAFETY: allocated in `new` with this layout.
        unsafe { std_dealloc(self.base, self.layout) };
    }
}

fn layout(size: usize, align: usize) -> Layout {
    Layout::from_size_align(size, align).unwrap()
}

fn free(heap: &mut Heap, pointer: NonNull<u8>, layout: Layout) {
    // SAFETY: tests free what they allocated, with its layout.
    unsafe { heap.dealloc(pointer, layout) }.unwrap();
}

#[test]
fn a_fresh_heap_is_one_free_block() {
    let arena = Arena::new(64 * 1024);
    let heap = arena.heap(64 * 1024);
    assert_eq!(heap.size(), 64 * 1024);
    assert_eq!(heap.free_bytes(), 64 * 1024);
    assert_eq!(heap.used_bytes(), 0);
    assert_eq!(heap.free_blocks(), 1);
}

#[test]
fn allocations_are_inside_the_heap_aligned_and_do_not_overlap() {
    let arena = Arena::new(64 * 1024);
    let mut heap = arena.heap(64 * 1024);
    let mut taken: Vec<(usize, usize)> = Vec::new();
    for (size, align) in [(1, 1), (7, 8), (16, 16), (100, 64), (4000, 4096), (33, 32)] {
        let l = layout(size, align);
        let pointer = heap.alloc(l).unwrap().as_ptr() as usize;
        assert_eq!(pointer % align, 0);
        assert!(pointer >= arena.base as usize);
        assert!(pointer + size <= arena.base as usize + 64 * 1024);
        for &(start, length) in &taken {
            assert!(pointer + size <= start || start + length <= pointer);
        }
        taken.push((pointer, size));
    }
}

#[test]
fn a_freed_block_is_reused_at_once() {
    let arena = Arena::new(4096);
    let mut heap = arena.heap(4096);
    let l = layout(100, 8);
    let first = heap.alloc(l).unwrap();
    free(&mut heap, first, l);
    assert_eq!(heap.alloc(l).unwrap(), first);
}

#[test]
fn freeing_everything_merges_into_one_block_in_any_order() {
    for order in [[0, 1, 2, 3], [3, 2, 1, 0], [1, 3, 0, 2], [2, 0, 3, 1]] {
        let arena = Arena::new(4096);
        let mut heap = arena.heap(4096);
        let l = layout(200, 8);
        let blocks: Vec<_> = (0..4).map(|_| heap.alloc(l).unwrap()).collect();
        for index in order {
            free(&mut heap, blocks[index], l);
        }
        assert_eq!(heap.free_blocks(), 1, "order {order:?}");
        assert_eq!(heap.free_bytes(), 4096);
    }
}

#[test]
fn fragmentation_is_visible_and_heals() {
    let arena = Arena::new(4096);
    let mut heap = arena.heap(4096);
    let l = layout(64, 8);
    let blocks: Vec<_> = (0..8).map(|_| heap.alloc(l).unwrap()).collect();
    for index in [0, 2, 4, 6] {
        free(&mut heap, blocks[index], l);
    }
    // Four holes of 64 and the tail: a bigger request skips the holes.
    assert_eq!(heap.free_blocks(), 5);
    let big = layout(128, 8);
    let pointer = heap.alloc(big).unwrap().as_ptr() as usize;
    assert!(
        pointer >= blocks[7].as_ptr() as usize + 64,
        "took a hole too small for it"
    );
    for index in [1, 3, 5, 7] {
        free(&mut heap, blocks[index], l);
    }
    // SAFETY: allocated just above with `big`.
    unsafe { heap.dealloc(NonNull::new(pointer as *mut u8).unwrap(), big) }.unwrap();
    assert_eq!(heap.free_blocks(), 1);
}

#[test]
fn alignment_returns_the_front_and_the_tail_to_the_list() {
    let arena = Arena::new(16 * 1024);
    let mut heap = arena.heap(16 * 1024);
    let _first = heap.alloc(layout(16, 16)).unwrap();
    let l = layout(100, 4096);
    let pointer = heap.alloc(l).unwrap();
    assert_eq!(pointer.as_ptr() as usize % 4096, 0);
    free(&mut heap, pointer, l);
    assert_eq!(heap.used_bytes(), 16);
    assert_eq!(heap.free_blocks(), 1);
}

#[test]
fn zero_size_large_alignment_and_overflow_are_errors() {
    let arena = Arena::new(4096);
    let mut heap = arena.heap(4096);
    assert_eq!(heap.alloc(layout(0, 1)), Err(AllocError::ZeroSize));
    assert_eq!(
        heap.alloc(layout(16, 8192)),
        Err(AllocError::AlignmentTooLarge)
    );
    let huge = Layout::from_size_align(isize::MAX as usize - 4096, 1).unwrap();
    assert!(matches!(
        heap.alloc(huge),
        Err(AllocError::OutOfMemory { .. })
    ));
    // Rounding a size close to usize::MAX would wrap. `Layout` caps sizes at
    // isize::MAX, so the largest that rounds up past it still fits in usize,
    // and the heap refuses it as exhausted.
    let edge = Layout::from_size_align(isize::MAX as usize, 1).unwrap();
    assert!(matches!(
        heap.alloc(edge),
        Err(AllocError::OutOfMemory { .. })
    ));
    assert_eq!(heap.free_bytes(), 4096, "a refusal changed the heap");
}

#[test]
fn exhaustion_reports_what_extend_must_add_and_extend_fixes_it() {
    let arena = Arena::new(8192);
    let mut heap = arena.heap(4096);
    let l = layout(6000, 8);
    let AllocError::OutOfMemory { needed } = heap.alloc(l).unwrap_err() else {
        panic!("expected exhaustion");
    };
    assert!((6000 - 4096..6000).contains(&needed));
    // SAFETY: the arena has the next 4096 bytes.
    unsafe { heap.extend(4096) }.unwrap();
    assert_eq!(heap.size(), 8192);
    assert!(heap.alloc(l).is_ok());
}

#[test]
fn extend_merges_with_a_free_tail_and_keeps_a_used_one_apart() {
    let arena = Arena::new(8192);
    let mut heap = arena.heap(4096);
    // SAFETY: the arena has the next 4096 bytes.
    unsafe { heap.extend(4096) }.unwrap();
    assert_eq!(heap.free_blocks(), 1);

    let mut heap = arena.heap(4096);
    let l = layout(4096, 16);
    let whole = heap.alloc(l).unwrap();
    // SAFETY: as above.
    unsafe { heap.extend(4096) }.unwrap();
    assert_eq!(heap.free_blocks(), 1);
    free(&mut heap, whole, l);
    assert_eq!(heap.free_blocks(), 1);
    assert_eq!(heap.free_bytes(), 8192);
}

#[test]
fn bad_frees_are_refused_and_change_nothing() {
    let arena = Arena::new(4096);
    let mut heap = arena.heap(4096);
    let l = layout(64, 8);
    let pointer = heap.alloc(l).unwrap();
    let before = (heap.free_bytes(), heap.free_blocks());
    // SAFETY: each call is one the heap must refuse before touching memory.
    unsafe {
        assert_eq!(
            heap.dealloc(NonNull::new(arena.base.add(8)).unwrap(), l),
            Err(FreeError::Unaligned)
        );
        let outside = NonNull::new(arena.base.add(4096)).unwrap();
        assert_eq!(heap.dealloc(outside, l), Err(FreeError::Outside));
        let straddling = NonNull::new(arena.base.add(4096 - 16)).unwrap();
        assert_eq!(heap.dealloc(straddling, l), Err(FreeError::Outside));
        let below = NonNull::new(arena.base.sub(16)).unwrap();
        assert_eq!(heap.dealloc(below, l), Err(FreeError::Outside));
        // Free memory: the tail was never allocated.
        let tail = NonNull::new(arena.base.add(2048)).unwrap();
        assert_eq!(heap.dealloc(tail, l), Err(FreeError::Overlap));
    }
    assert_eq!((heap.free_bytes(), heap.free_blocks()), before);
    free(&mut heap, pointer, l);
    // SAFETY: a double free, which the heap sees as overlapping.
    assert_eq!(unsafe { heap.dealloc(pointer, l) }, Err(FreeError::Overlap));
    assert_eq!(heap.free_blocks(), 1);
}

#[test]
fn init_and_extend_refuse_bad_input() {
    let arena = Arena::new(4096);
    let mut heap = Heap::empty();
    // SAFETY: refused before use.
    unsafe {
        assert_eq!(heap.extend(16), Err(ExtendError::NotInitialised));
        assert_eq!(
            heap.init(arena.base.add(8), 64),
            Err(ExtendError::BadLength)
        );
        assert_eq!(heap.init(arena.base, 0), Err(ExtendError::BadLength));
        assert_eq!(heap.init(arena.base, 20), Err(ExtendError::BadLength));
        assert_eq!(
            heap.init(core::ptr::null_mut(), 64),
            Err(ExtendError::BadLength)
        );
        assert_eq!(
            heap.init(arena.base, usize::MAX & !15),
            Err(ExtendError::Overflow)
        );
    }
    let mut heap = arena.heap(64);
    // SAFETY: refused before use.
    unsafe {
        assert_eq!(heap.extend(0), Err(ExtendError::BadLength));
        assert_eq!(heap.extend(8), Err(ExtendError::BadLength));
        assert_eq!(heap.extend(usize::MAX & !15), Err(ExtendError::Overflow));
    }
    assert_eq!(heap.size(), 64);
}

struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, limit: u64) -> u64 {
        self.next() % limit
    }
}

/// Random allocations and frees against a model that records every live block:
/// blocks stay inside the heap, aligned, apart, and keep their contents; the
/// counters match the model; at the end everything merges back.
#[test]
fn random_use_matches_a_model() {
    for seed in 1..=40u64 {
        let capacity = 256 * 1024;
        let arena = Arena::new(capacity);
        let mut heap = arena.heap(capacity / 4);
        let mut random = Random(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        let mut live: BTreeMap<usize, (Layout, u8)> = BTreeMap::new();
        let mut model_used = 0usize;

        for step in 0..3000 {
            if random.below(5) < 3 {
                let size = 1 + random.below(3000) as usize;
                let align = 1usize << random.below(8);
                let l = layout(size, align);
                match heap.alloc(l) {
                    Ok(pointer) => {
                        let address = pointer.as_ptr() as usize;
                        assert_eq!(address % align, 0);
                        assert!(address >= arena.base as usize);
                        assert!(address + size <= arena.base as usize + heap.size());
                        if let Some((&below, &(l2, _))) = live.range(..=address).next_back() {
                            assert!(below + l2.size() <= address, "overlap below, step {step}");
                        }
                        if let Some((&above, _)) = live.range(address..).next() {
                            assert!(address + size <= above, "overlap above, step {step}");
                        }
                        let fill = random.below(255) as u8 + 1;
                        // SAFETY: the block is ours and `size` long.
                        unsafe { core::ptr::write_bytes(pointer.as_ptr(), fill, size) };
                        live.insert(address, (l, fill));
                        model_used += (size.max(GRANULE) + GRANULE - 1) & !(GRANULE - 1);
                    }
                    Err(AllocError::OutOfMemory { needed }) => {
                        if random.below(2) == 0 {
                            let more = ((needed + 4095) & !4095).min(arena.length - heap.size());
                            if more > 0 {
                                // SAFETY: still inside the arena.
                                unsafe { heap.extend(more) }.unwrap();
                            }
                        }
                    }
                    Err(other) => panic!("unexpected {other:?}"),
                }
            } else if let Some((&address, &(l, fill))) = live
                .iter()
                .nth(random.below(live.len().max(1) as u64) as usize)
            {
                // SAFETY: a live block of ours.
                let bytes = unsafe { core::slice::from_raw_parts(address as *const u8, l.size()) };
                assert!(
                    bytes.iter().all(|&b| b == fill),
                    "contents changed, step {step}"
                );
                free(&mut heap, NonNull::new(address as *mut u8).unwrap(), l);
                live.remove(&address);
                model_used -= (l.size().max(GRANULE) + GRANULE - 1) & !(GRANULE - 1);
            }
            assert_eq!(heap.used_bytes(), model_used, "seed {seed}, step {step}");
        }
        for (address, (l, fill)) in std::mem::take(&mut live) {
            // SAFETY: a live block of ours.
            let bytes = unsafe { core::slice::from_raw_parts(address as *const u8, l.size()) };
            assert!(bytes.iter().all(|&b| b == fill));
            free(&mut heap, NonNull::new(address as *mut u8).unwrap(), l);
        }
        assert_eq!(heap.used_bytes(), 0);
        assert_eq!(heap.free_blocks(), 1, "seed {seed}: not coalesced");
    }
}

/// Whatever the heap looks like, extending by what `OutOfMemory` reports makes
/// the refused request fit.
#[test]
fn extending_by_the_reported_amount_always_makes_the_request_fit() {
    let arena = Arena::new(1 << 20);
    let mut random = Random(7);
    let mut heap = arena.heap(4096);
    let mut live = Vec::new();
    for _ in 0..400 {
        let l = layout(1 + random.below(5000) as usize, 1usize << random.below(13));
        let pointer = match heap.alloc(l) {
            Ok(pointer) => pointer,
            Err(AllocError::OutOfMemory { needed }) => {
                let length = needed.next_multiple_of(GRANULE);
                // SAFETY: the arena is a megabyte and the test stays below it.
                unsafe { heap.extend(length) }.unwrap();
                let again = heap.alloc(l);
                assert!(again.is_ok(), "{l:?} did not fit after adding {length}");
                again.unwrap()
            }
            Err(other) => panic!("{other:?}"),
        };
        live.push((pointer, l));
        if random.below(2) == 0 {
            let (pointer, l) = live.swap_remove(random.below(live.len() as u64) as usize);
            free(&mut heap, pointer, l);
        }
    }
}

#[test]
fn aligned_blocks_behind_a_small_one_leave_no_residue() {
    let arena = Arena::new(64 * 1024);
    let mut heap = arena.heap(64 * 1024);
    let pad_layout = layout(16, 16);
    let pad = heap.alloc(pad_layout).unwrap();
    for align in [16, 64, 256, 4096] {
        let l = layout(100, align);
        let block = heap.alloc(l).unwrap();
        assert_eq!(block.as_ptr() as usize % align, 0);
        free(&mut heap, block, l);
        assert_eq!(heap.used_bytes(), 16, "align {align}");
    }
    free(&mut heap, pad, pad_layout);
    assert_eq!(heap.used_bytes(), 0);
    assert_eq!(heap.free_blocks(), 1);
}

fn tables(pages: usize) -> usize {
    pages + pages.div_ceil(512) + 3
}

#[test]
fn growth_is_refused_when_the_window_or_the_frames_with_their_tables_fall_short() {
    // Past the window.
    assert_eq!(growth_pages(101, 16, 64, 10_000, 100, tables), None);
    // The frames cover the pages but not the tables that map them.
    assert_eq!(growth_pages(1000, 16, 64, 1000, 10_000, tables), None);
    assert_eq!(
        growth_pages(1000, 16, 64, tables(1000) - 1, 10_000, tables),
        None
    );
    assert_eq!(
        growth_pages(1000, 16, 0, tables(1000), 10_000, tables),
        Some(1000)
    );
    // No frames at all.
    assert_eq!(growth_pages(1, 16, 64, 0, 10_000, tables), None);
}

#[test]
fn the_minimum_growth_never_eats_the_reserve() {
    // Plenty of frames: the minimum is added.
    assert_eq!(growth_pages(1, 16, 64, 10_000, 10_000, tables), Some(16));
    // Few frames: only the request, not the extra pages.
    assert_eq!(growth_pages(1, 16, 64, 6, 10_000, tables), Some(1));
    // Just above the reserve: some of the extra.
    let free = tables(1) + 64 + 5;
    assert_eq!(growth_pages(1, 16, 64, free, 10_000, tables), Some(6));
    // A big request needs no extra and takes what it needs, reserve or not.
    assert_eq!(
        growth_pages(300, 16, 64, tables(300), 10_000, tables),
        Some(300)
    );
    // The extra stays inside the window.
    assert_eq!(growth_pages(1, 16, 0, 10_000, 4, tables), Some(4));
}
