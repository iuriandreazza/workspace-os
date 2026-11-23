//! Physical frame allocator.
//!
//! It needs no heap: the caller lends it a slice of words, and the allocator
//! keeps two bitmaps in it, one bit per frame of a window of physical memory.
//!
//! - `available`: the frame is RAM, is not reserved and is not handed out.
//! - `allocated`: the frame was handed out by [`FrameAllocator::alloc`] and not
//!   returned yet. Only such a frame may be freed, so a frame the kernel image
//!   or the device tree occupies can never become available by a stray `free`.
//!
//! The map is built in two independent steps, [`Builder::add_memory`] and
//! [`Builder::reserve`], that may come in any order: a reservation always wins.
//! Every range is untrusted (it comes from the device tree): sizes of zero,
//! overlaps, ranges outside the window and ranges that wrap the address space
//! are all safe, and can only reduce what is available.
//!
//! Frames are handed out lowest address first, so the result is deterministic.

#![cfg_attr(not(test), no_std)]

use core::fmt;
use core::ops::Range;

pub const FRAME_SIZE: u64 = 4096;
const BITS: usize = u64::BITS as usize;

/// Words of storage that manage a window of `frames` frames.
pub const fn storage_words(frames: usize) -> usize {
    2 * frames.div_ceil(BITS)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreeError {
    Unaligned,
    OutOfRange,
    NotAllocated,
}

impl fmt::Display for FreeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unaligned => "address is not frame aligned",
            Self::OutOfRange => "address is outside the managed window",
            Self::NotAllocated => "frame is not allocated",
        })
    }
}

/// The window of frames the bitmaps cover, shared by the builder and the
/// allocator.
#[derive(Clone, Copy)]
struct Window {
    base: u64,
    frames: usize,
}

impl Window {
    fn end(&self) -> u64 {
        // `frames` is capped in `Builder::new` so that this cannot overflow.
        self.base + self.frames as u64 * FRAME_SIZE
    }

    /// The frames of the window between two physical addresses. Addresses
    /// outside the window are clamped to its edges; a reversed pair is empty.
    fn frames_between(&self, start: u64, end: u64) -> Range<usize> {
        let first = start.clamp(self.base, self.end());
        let last = end.clamp(self.base, self.end());
        let first = ((first - self.base) / FRAME_SIZE) as usize;
        let last = ((last - self.base) / FRAME_SIZE) as usize;
        first..last.max(first)
    }
}

fn align_down(address: u64) -> u64 {
    address & !(FRAME_SIZE - 1)
}

/// Rounds up, saturating at the last frame boundary of the address space.
fn align_up(address: u64) -> u64 {
    address
        .checked_next_multiple_of(FRAME_SIZE)
        .unwrap_or(align_down(u64::MAX))
}

/// Sets or clears a range of bits a word at a time: the ranges come from the
/// device tree, so the cost of one must not grow with its length bit by bit.
fn set_bits(bitmap: &mut [u64], frames: Range<usize>, value: bool) {
    if frames.is_empty() {
        return;
    }
    let (first_word, last_word) = (frames.start / BITS, (frames.end - 1) / BITS);
    for (offset, bits) in bitmap[first_word..=last_word].iter_mut().enumerate() {
        let word = first_word + offset;
        let low = if word == first_word {
            frames.start % BITS
        } else {
            0
        };
        let high = if word == last_word {
            (frames.end - 1) % BITS + 1
        } else {
            BITS
        };
        let width = high - low;
        let mask = if width == BITS {
            u64::MAX
        } else {
            ((1 << width) - 1) << low
        };
        if value {
            *bits |= mask;
        } else {
            *bits &= !mask;
        }
    }
}

/// The bits of `word` that belong to the frames in `frames`.
fn word_mask(frames: &Range<usize>, word: usize) -> u64 {
    let first = word * BITS;
    let low = frames.start.saturating_sub(first).min(BITS);
    let high = frames.end.saturating_sub(first).min(BITS);
    if high <= low {
        return 0;
    }
    let width = high - low;
    let ones = if width == BITS {
        u64::MAX
    } else {
        (1u64 << width) - 1
    };
    ones << low
}

fn count_bits(bitmap: &[u64]) -> usize {
    bitmap.iter().map(|word| word.count_ones() as usize).sum()
}

/// Collects the memory map: what is RAM and what is reserved.
pub struct Builder<'a> {
    window: Window,
    memory: &'a mut [u64],
    reserved: &'a mut [u64],
    dropped_bytes: u64,
}

impl<'a> Builder<'a> {
    /// Manages frames from `window_base` (rounded up to a frame) for as long as
    /// `storage` allows: `storage.len() / 2` words of 64 frames each. Memory
    /// outside that window cannot be managed and is counted as dropped.
    pub fn new(window_base: u64, storage: &'a mut [u64]) -> Self {
        let base = align_up(window_base);
        let (memory, reserved) = storage.split_at_mut(storage.len() / 2);
        let reserved = &mut reserved[..memory.len()];
        let room = usize::try_from((u64::MAX - base) / FRAME_SIZE).unwrap_or(usize::MAX);
        let frames = (memory.len() * BITS).min(room);
        memory.fill(0);
        reserved.fill(0);
        Self {
            window: Window { base, frames },
            memory,
            reserved,
            dropped_bytes: 0,
        }
    }

    /// Marks the frames wholly inside `[base, base + size)` as RAM. A partial
    /// frame at either end is not usable and is left out.
    pub fn add_memory(&mut self, base: u64, size: u64) {
        let end = base.saturating_add(size);
        let (window_start, window_end) = (self.window.base, self.window.end());
        let inside = end.clamp(window_start, window_end) - base.clamp(window_start, window_end);
        self.dropped_bytes = self.dropped_bytes.saturating_add((end - base) - inside);
        let frames = self.window.frames_between(align_up(base), align_down(end));
        set_bits(self.memory, frames, true);
    }

    /// Marks every frame touched by `[base, base + size)` as not available.
    /// A partial frame counts: the whole frame is kept out.
    pub fn reserve(&mut self, base: u64, size: u64) {
        if size == 0 {
            return;
        }
        let end = base.saturating_add(size);
        let frames = self.window.frames_between(align_down(base), align_up(end));
        set_bits(self.reserved, frames, true);
    }

    pub fn build(self) -> FrameAllocator<'a> {
        let memory_frames = count_bits(self.memory);
        let mut reserved_frames = 0;
        for (memory, reserved) in self.memory.iter_mut().zip(self.reserved.iter()) {
            reserved_frames += (*memory & reserved).count_ones() as usize;
            *memory &= !reserved;
        }
        // The same words now record what has been handed out: nothing yet.
        self.reserved.fill(0);
        FrameAllocator {
            window: self.window,
            available: self.memory,
            allocated: self.reserved,
            free_frames: memory_frames - reserved_frames,
            cursor: 0,
            memory_frames,
            reserved_frames,
            dropped_bytes: self.dropped_bytes,
        }
    }
}

pub struct FrameAllocator<'a> {
    window: Window,
    available: &'a mut [u64],
    allocated: &'a mut [u64],
    free_frames: usize,
    /// No word below this one holds an available frame.
    cursor: usize,
    memory_frames: usize,
    reserved_frames: usize,
    dropped_bytes: u64,
}

impl FrameAllocator<'_> {
    /// Hands out the lowest available frame, or `None` when memory is
    /// exhausted. The frame's contents are whatever was left in it.
    pub fn alloc(&mut self) -> Option<u64> {
        for word in self.cursor..self.available.len() {
            let available = self.available[word];
            if available == 0 {
                continue;
            }
            let bit = available.trailing_zeros() as usize;
            self.available[word] &= !(1 << bit);
            self.allocated[word] |= 1 << bit;
            self.free_frames -= 1;
            self.cursor = word;
            return Some(self.window.base + ((word * BITS + bit) as u64) * FRAME_SIZE);
        }
        self.cursor = self.available.len();
        None
    }

    /// Hands out the lowest available frame wholly inside `range`, or `None`
    /// when there is none. Everything else is as [`alloc`](Self::alloc), and
    /// the frames outside `range` are left as they are.
    pub fn alloc_in(&mut self, range: Range<u64>) -> Option<u64> {
        let frames = self
            .window
            .frames_between(align_up(range.start), align_down(range.end));
        if frames.is_empty() {
            return None;
        }
        for word in frames.start / BITS..=(frames.end - 1) / BITS {
            let available = self.available[word] & word_mask(&frames, word);
            if available == 0 {
                continue;
            }
            let bit = available.trailing_zeros() as usize;
            self.available[word] &= !(1 << bit);
            self.allocated[word] |= 1 << bit;
            self.free_frames -= 1;
            return Some(self.window.base + ((word * BITS + bit) as u64) * FRAME_SIZE);
        }
        None
    }

    /// Returns a frame that [`alloc`](Self::alloc) handed out. Anything else
    /// (a frame never handed out, one already freed, a reserved frame, an
    /// address outside the window or not on a frame boundary) is refused and
    /// changes nothing.
    pub fn free(&mut self, address: u64) -> Result<(), FreeError> {
        if !address.is_multiple_of(FRAME_SIZE) {
            return Err(FreeError::Unaligned);
        }
        if address < self.window.base || address >= self.window.end() {
            return Err(FreeError::OutOfRange);
        }
        let index = ((address - self.window.base) / FRAME_SIZE) as usize;
        let (word, bit) = (index / BITS, index % BITS);
        if self.allocated[word] & (1 << bit) == 0 {
            return Err(FreeError::NotAllocated);
        }
        self.allocated[word] &= !(1 << bit);
        self.available[word] |= 1 << bit;
        self.free_frames += 1;
        self.cursor = self.cursor.min(word);
        Ok(())
    }

    /// Makes the reserved frames of `range` available, except those touched by
    /// a range of `keep`, and returns how many it released. This is for memory
    /// the boot protocol hands over once the kernel no longer reads it
    /// (ADR 0030): everything built into the allocator as reserved stays
    /// reserved until now.
    ///
    /// The allocator does not remember which frames are RAM, so the caller
    /// promises that `range` is. A frame that is already available or
    /// allocated is left as it is, so releasing a range twice releases nothing
    /// the second time, and a frame in use is never given out again. Frames
    /// outside the window are not managed and are ignored. A partial frame at
    /// either end of `range` is not released; one that a `keep` range touches
    /// even in part is kept.
    pub fn release(&mut self, range: Range<u64>, keep: &[Range<u64>]) -> usize {
        let frames = self
            .window
            .frames_between(align_up(range.start), align_down(range.end));
        if frames.is_empty() {
            return 0;
        }
        // The frames a `keep` range touches, partly or not, as frame numbers
        // of the window.
        let window = self.window;
        let kept =
            |keep: &Range<u64>| window.frames_between(align_down(keep.start), align_up(keep.end));
        let mut released = 0;
        for word in frames.start / BITS..=(frames.end - 1) / BITS {
            let mut mask = word_mask(&frames, word);
            for keep in keep {
                mask &= !word_mask(&kept(keep), word);
            }
            // Reserved: neither available nor handed out.
            let reserved = mask & !self.available[word] & !self.allocated[word];
            self.available[word] |= reserved;
            released += reserved.count_ones() as usize;
        }
        self.free_frames += released;
        self.reserved_frames -= released.min(self.reserved_frames);
        self.cursor = self.cursor.min(frames.start / BITS);
        released
    }

    pub fn free_frames(&self) -> usize {
        self.free_frames
    }

    /// Frames of RAM in the map, reserved ones included.
    pub fn memory_frames(&self) -> usize {
        self.memory_frames
    }

    /// Frames of RAM that a reservation keeps out.
    pub fn reserved_frames(&self) -> usize {
        self.reserved_frames
    }

    /// Bytes of RAM that fell outside the window and cannot be managed.
    pub fn dropped_bytes(&self) -> u64 {
        self.dropped_bytes
    }

    /// The physical addresses the bitmaps cover.
    pub fn window(&self) -> Range<u64> {
        self.window.base..self.window.end()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    const F: u64 = FRAME_SIZE;
    const BASE: u64 = 0x4000_0000;

    fn storage(frames: usize) -> Vec<u64> {
        vec![u64::MAX; storage_words(frames)]
    }

    /// Frames handed out until exhaustion.
    fn drain(allocator: &mut FrameAllocator) -> Vec<u64> {
        std::iter::from_fn(|| allocator.alloc()).collect()
    }

    #[test]
    fn set_bits_matches_a_bit_by_bit_fill() {
        for start in [0, 1, 63, 64, 65, 127, 128, 200] {
            for end in [0, 1, 63, 64, 65, 128, 129, 192, 255, 256] {
                for value in [false, true] {
                    let mut fast = vec![if value { 0 } else { u64::MAX }; 4];
                    let mut slow = fast.clone();
                    set_bits(&mut fast, start..end, value);
                    for index in start..end {
                        let (word, bit) = (index / BITS, index % BITS);
                        if value {
                            slow[word] |= 1 << bit;
                        } else {
                            slow[word] &= !(1 << bit);
                        }
                    }
                    assert_eq!(fast, slow, "{start}..{end} {value}");
                }
            }
        }
    }

    #[test]
    fn dirty_storage_is_cleared() {
        let mut words = storage(128);
        let allocator = Builder::new(BASE, &mut words).build();
        assert_eq!(allocator.free_frames(), 0);
        assert_eq!(allocator.memory_frames(), 0);
    }

    #[test]
    fn hands_out_every_frame_once_lowest_first() {
        let mut words = storage(256);
        let mut builder = Builder::new(BASE, &mut words);
        builder.add_memory(BASE, 100 * F);
        let mut allocator = builder.build();
        assert_eq!(allocator.free_frames(), 100);
        let frames = drain(&mut allocator);
        let expected: Vec<u64> = (0..100).map(|i| BASE + i * F).collect();
        assert_eq!(frames, expected);
        assert_eq!(allocator.free_frames(), 0);
        assert_eq!(allocator.alloc(), None);
        assert_eq!(allocator.alloc(), None);
    }

    #[test]
    fn reserved_frames_are_never_handed_out_whatever_the_order() {
        for reserve_first in [false, true] {
            let mut words = storage(256);
            let mut builder = Builder::new(BASE, &mut words);
            if reserve_first {
                builder.reserve(BASE + 10 * F, 5 * F);
            }
            builder.add_memory(BASE, 64 * F);
            if !reserve_first {
                builder.reserve(BASE + 10 * F, 5 * F);
            }
            let mut allocator = builder.build();
            assert_eq!(allocator.memory_frames(), 64);
            assert_eq!(allocator.reserved_frames(), 5);
            assert_eq!(allocator.free_frames(), 59);
            let frames = drain(&mut allocator);
            assert_eq!(frames.len(), 59);
            assert!(
                frames
                    .iter()
                    .all(|f| !(BASE + 10 * F..BASE + 15 * F).contains(f))
            );
        }
    }

    #[test]
    fn partial_frames_round_the_safe_way() {
        let mut words = storage(64);
        let mut builder = Builder::new(BASE, &mut words);
        // Memory loses its partial edges: 1 byte past a boundary to 1 byte short of one.
        builder.add_memory(BASE + 1, 10 * F - 2);
        // A one-byte reservation takes its whole frame.
        builder.reserve(BASE + 3 * F + 17, 1);
        // A reservation ending one byte into a frame takes that frame too.
        builder.reserve(BASE + 6 * F, F + 1);
        let mut allocator = builder.build();
        let frames = drain(&mut allocator);
        let expected: Vec<u64> = [1, 2, 4, 5, 8].iter().map(|i| BASE + i * F).collect();
        assert_eq!(frames, expected);
    }

    #[test]
    fn hostile_ranges_cannot_corrupt_the_map() {
        let mut words = storage(128);
        let mut builder = Builder::new(BASE, &mut words);
        builder.add_memory(BASE, 32 * F);
        // Zero sizes, overlaps, duplicates and ranges far outside the window.
        builder.add_memory(BASE, 0);
        builder.add_memory(BASE + 8 * F, 8 * F);
        builder.add_memory(BASE, 32 * F);
        builder.add_memory(0, 0x1000);
        builder.add_memory(u64::MAX - F, 10 * F);
        builder.add_memory(u64::MAX, u64::MAX);
        builder.reserve(BASE, 0);
        builder.reserve(0, F);
        builder.reserve(u64::MAX - 10, 1000);
        builder.reserve(BASE + 4 * F, 4 * F);
        builder.reserve(BASE + 6 * F, 4 * F);
        builder.reserve(BASE + 4 * F, 6 * F);
        let mut allocator = builder.build();
        assert_eq!(allocator.memory_frames(), 32);
        assert_eq!(allocator.reserved_frames(), 6);
        assert_eq!(drain(&mut allocator).len(), 26);
    }

    #[test]
    fn only_reserved_frames_that_are_memory_are_counted() {
        let mut words = storage(256);
        let mut builder = Builder::new(BASE, &mut words);
        builder.add_memory(BASE, 10 * F);
        builder.add_memory(BASE + 20 * F, 10 * F);
        // Two frames of RAM, then the hole, then two frames of the second bank.
        builder.reserve(BASE + 8 * F, 14 * F);
        builder.reserve(BASE + 100 * F, 5 * F);
        let allocator = builder.build();
        assert_eq!(allocator.memory_frames(), 20);
        assert_eq!(allocator.reserved_frames(), 4);
        assert_eq!(allocator.free_frames(), 16);
    }

    #[test]
    fn a_reservation_across_the_whole_address_space_leaves_nothing() {
        let mut words = storage(64);
        let mut builder = Builder::new(BASE, &mut words);
        builder.add_memory(BASE, 64 * F);
        builder.reserve(0, u64::MAX);
        let mut allocator = builder.build();
        assert_eq!(allocator.free_frames(), 0);
        assert_eq!(allocator.alloc(), None);
    }

    #[test]
    fn memory_outside_the_window_is_dropped_and_counted() {
        let mut words = storage(64);
        let mut builder = Builder::new(BASE, &mut words);
        // 100 frames asked for, 64 fit.
        builder.add_memory(BASE, 100 * F);
        // Entirely below and entirely above the window.
        builder.add_memory(BASE - 4 * F, 4 * F);
        builder.add_memory(BASE + 1000 * F, 2 * F);
        let allocator = builder.build();
        assert_eq!(allocator.memory_frames(), 64);
        assert_eq!(allocator.dropped_bytes(), (36 + 4 + 2) * F);
        assert_eq!(allocator.window(), BASE..BASE + 64 * F);
    }

    #[test]
    fn window_at_the_top_of_the_address_space_does_not_overflow() {
        for base in [u64::MAX, u64::MAX - F, align_down(u64::MAX) - 3 * F] {
            let mut words = storage(64);
            let mut builder = Builder::new(base, &mut words);
            builder.add_memory(base, u64::MAX);
            builder.reserve(base, u64::MAX);
            let mut allocator = builder.build();
            let window = allocator.window();
            assert!(window.start <= window.end);
            let _ = drain(&mut allocator);
            assert_eq!(allocator.free(u64::MAX), Err(FreeError::Unaligned));
        }
    }

    #[test]
    fn unaligned_window_base_rounds_up() {
        let mut words = storage(64);
        let mut builder = Builder::new(BASE + 1, &mut words);
        builder.add_memory(BASE, 4 * F);
        let mut allocator = builder.build();
        assert_eq!(allocator.window().start, BASE + F);
        assert_eq!(
            drain(&mut allocator),
            vec![BASE + F, BASE + 2 * F, BASE + 3 * F]
        );
    }

    #[test]
    fn empty_storage_manages_nothing() {
        for len in [0, 1] {
            let mut words = vec![u64::MAX; len];
            let mut builder = Builder::new(BASE, &mut words);
            builder.add_memory(BASE, 64 * F);
            builder.reserve(BASE, F);
            let mut allocator = builder.build();
            assert_eq!(allocator.alloc(), None);
            assert_eq!(allocator.free(BASE), Err(FreeError::OutOfRange));
        }
    }

    fn small() -> (Vec<u64>, u64) {
        (storage(128), BASE)
    }

    #[test]
    fn freed_frames_are_reused_lowest_first() {
        let (mut words, base) = small();
        let mut builder = Builder::new(base, &mut words);
        builder.add_memory(base, 8 * F);
        let mut allocator = builder.build();
        let frames = drain(&mut allocator);
        assert_eq!(frames.len(), 8);
        assert_eq!(allocator.free(base + 5 * F), Ok(()));
        assert_eq!(allocator.free(base + 2 * F), Ok(()));
        assert_eq!(allocator.free_frames(), 2);
        assert_eq!(allocator.alloc(), Some(base + 2 * F));
        assert_eq!(allocator.alloc(), Some(base + 5 * F));
        assert_eq!(allocator.alloc(), None);
    }

    #[test]
    fn reuse_works_across_words() {
        let mut words = storage(256);
        let mut builder = Builder::new(BASE, &mut words);
        builder.add_memory(BASE, 200 * F);
        let mut allocator = builder.build();
        drain(&mut allocator);
        assert_eq!(allocator.free(BASE + 130 * F), Ok(()));
        assert_eq!(allocator.alloc(), Some(BASE + 130 * F));
        assert_eq!(allocator.free(BASE + 3 * F), Ok(()));
        assert_eq!(allocator.alloc(), Some(BASE + 3 * F));
    }

    #[test]
    fn free_refuses_anything_that_was_not_handed_out() {
        let (mut words, base) = small();
        let mut builder = Builder::new(base, &mut words);
        builder.add_memory(base, 8 * F);
        builder.reserve(base + F, F);
        let mut allocator = builder.build();
        let before = allocator.free_frames();
        // Never handed out: available, reserved, and not RAM at all.
        assert_eq!(allocator.free(base), Err(FreeError::NotAllocated));
        assert_eq!(allocator.free(base + F), Err(FreeError::NotAllocated));
        assert_eq!(allocator.free(base + 100 * F), Err(FreeError::NotAllocated));
        assert_eq!(allocator.free(base + 1), Err(FreeError::Unaligned));
        assert_eq!(allocator.free(base - F), Err(FreeError::OutOfRange));
        assert_eq!(allocator.free(base + 128 * F), Err(FreeError::OutOfRange));
        assert_eq!(allocator.free(u64::MAX - F + 1), Err(FreeError::OutOfRange));
        assert_eq!(allocator.free_frames(), before);

        let a = allocator.alloc().unwrap();
        assert_eq!(allocator.free(a), Ok(()));
        assert_eq!(allocator.free(a), Err(FreeError::NotAllocated));
        assert_eq!(allocator.free_frames(), before);
        // The refused double free must not have made `a` available twice.
        let frames = drain(&mut allocator);
        let unique: BTreeSet<u64> = frames.iter().copied().collect();
        assert_eq!(unique.len(), frames.len());
    }

    #[test]
    fn random_operations_match_a_simple_model() {
        let mut words = storage(512);
        let mut builder = Builder::new(BASE, &mut words);
        builder.add_memory(BASE + 3 * F, 200 * F);
        builder.add_memory(BASE + 300 * F, 150 * F);
        builder.reserve(BASE + 20 * F, 30 * F);
        builder.reserve(BASE + 340 * F, 3 * F);
        let mut allocator = builder.build();

        let mut available: BTreeSet<u64> = (3..203).chain(300..450).map(|i| BASE + i * F).collect();
        for reserved in (20..50).chain(340..343) {
            available.remove(&(BASE + reserved * F));
        }
        let mut held: BTreeSet<u64> = BTreeSet::new();
        assert_eq!(allocator.free_frames(), available.len());

        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        for _ in 0..20_000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            match state % 3 {
                0 | 1 => {
                    let expected = available.first().copied();
                    assert_eq!(allocator.alloc(), expected);
                    if let Some(frame) = expected {
                        available.remove(&frame);
                        held.insert(frame);
                    }
                }
                _ => {
                    // Mostly a held frame, sometimes a stranger.
                    let pick = (state >> 8) % 600;
                    let address = BASE + pick * F + if state.is_multiple_of(7) { 8 } else { 0 };
                    let result = allocator.free(address);
                    if held.remove(&address) {
                        assert_eq!(result, Ok(()));
                        available.insert(address);
                    } else {
                        assert!(result.is_err(), "freed {address:#x} that was not held");
                    }
                }
            }
            assert_eq!(allocator.free_frames(), available.len());
        }
    }

    #[test]
    fn released_frames_are_handed_out_and_counted() {
        let mut words = storage(256);
        let mut builder = Builder::new(BASE, &mut words);
        builder.add_memory(BASE, 200 * F);
        builder.reserve(BASE + 10 * F, 100 * F);
        let mut allocator = builder.build();
        assert_eq!(allocator.free_frames(), 100);
        assert_eq!(allocator.reserved_frames(), 100);
        // The reserved range spans three words (frames 10 to 109).
        assert_eq!(allocator.release(BASE + 10 * F..BASE + 110 * F, &[]), 100);
        assert_eq!(allocator.free_frames(), 200);
        assert_eq!(allocator.reserved_frames(), 0);
        let handed = drain(&mut allocator);
        assert_eq!(handed.len(), 200);
        assert!(handed.contains(&(BASE + 10 * F)) && handed.contains(&(BASE + 109 * F)));
    }

    #[test]
    fn release_keeps_what_the_keep_ranges_touch_even_in_part() {
        let mut words = storage(256);
        let mut builder = Builder::new(BASE, &mut words);
        builder.add_memory(BASE, 200 * F);
        builder.reserve(BASE, 200 * F);
        let mut allocator = builder.build();
        assert_eq!(allocator.free_frames(), 0);
        // Frames 5 (whole), 63 and 64 (one byte of each) and 130 to 140.
        let keep = [
            BASE + 5 * F..BASE + 6 * F,
            BASE + 63 * F + F - 1..BASE + 64 * F + 1,
            BASE + 130 * F..BASE + 141 * F,
        ];
        let released = allocator.release(BASE..BASE + 200 * F, &keep);
        assert_eq!(released, 200 - 1 - 2 - 11);
        let handed = drain(&mut allocator);
        for frame in [5, 63, 64, 130, 135, 140] {
            assert!(!handed.contains(&(BASE + frame * F)), "frame {frame}");
        }
        for frame in [0, 4, 6, 62, 65, 129, 141, 199] {
            assert!(handed.contains(&(BASE + frame * F)), "frame {frame}");
        }
    }

    #[test]
    fn releasing_twice_or_over_frames_in_use_changes_nothing() {
        let mut words = storage(256);
        let mut builder = Builder::new(BASE, &mut words);
        builder.add_memory(BASE, 128 * F);
        builder.reserve(BASE + 64 * F, 64 * F);
        let mut allocator = builder.build();
        let first = allocator.alloc().unwrap();
        // Over free frames, an allocated one and reserved ones.
        assert_eq!(allocator.release(BASE..BASE + 128 * F, &[]), 64);
        assert_eq!(allocator.release(BASE..BASE + 128 * F, &[]), 0);
        assert_eq!(
            allocator.free(first),
            Ok(()),
            "the frame in use was still in use"
        );
        assert_eq!(allocator.free(first), Err(FreeError::NotAllocated));
        assert_eq!(allocator.free_frames(), 128);
    }

    #[test]
    fn release_ignores_what_is_outside_the_window_or_partial() {
        // A window of 64 frames from BASE.
        let mut words = storage(64);
        let mut builder = Builder::new(BASE, &mut words);
        builder.add_memory(BASE, 64 * F);
        builder.reserve(BASE, 64 * F);
        let mut allocator = builder.build();
        // Entirely outside the window; empty; a partial frame at both ends.
        assert_eq!(allocator.release(BASE - 8 * F..BASE, &[]), 0);
        assert_eq!(allocator.release(BASE + 64 * F..BASE + 80 * F, &[]), 0);
        assert_eq!(allocator.release(BASE + 5 * F..BASE + 5 * F, &[]), 0);
        assert_eq!(allocator.release(BASE + 20..BASE + F + 20, &[]), 0);
        assert_eq!(allocator.release(BASE + F + 1..BASE + 3 * F, &[]), 1);
        // Starts below the window and ends inside: frames 0 to 3, frame 1
        // already released.
        assert_eq!(allocator.release(BASE - 8 * F..BASE + 4 * F, &[]), 3);
        // Starts inside and ends past the window: the rest.
        assert_eq!(allocator.release(BASE + 4 * F..BASE + 100 * F, &[]), 60);
        assert_eq!(allocator.free_frames(), 64);
    }

    #[test]
    fn release_lowers_the_cursor_so_the_lowest_frame_comes_first() {
        let mut words = storage(256);
        let mut builder = Builder::new(BASE, &mut words);
        builder.add_memory(BASE, 200 * F);
        builder.reserve(BASE, 10 * F);
        let mut allocator = builder.build();
        drain(&mut allocator);
        assert_eq!(allocator.alloc(), None);
        assert_eq!(allocator.release(BASE..BASE + 10 * F, &[]), 10);
        assert_eq!(allocator.alloc(), Some(BASE));
    }

    #[test]
    fn a_hostile_release_cannot_wrap_or_overflow() {
        let mut words = storage(64);
        let mut builder = Builder::new(BASE, &mut words);
        builder.add_memory(BASE, 64 * F);
        builder.reserve(BASE, 64 * F);
        let mut allocator = builder.build();
        assert_eq!(allocator.release(u64::MAX - F..u64::MAX, &[]), 0);
        let everything = 0..u64::MAX;
        assert_eq!(
            allocator.release(0..u64::MAX, core::slice::from_ref(&everything)),
            0
        );
        assert_eq!(allocator.free_frames(), 0);
        assert_eq!(allocator.release(0..u64::MAX, &[]), 64);
    }

    #[test]
    fn alloc_in_takes_the_lowest_frame_of_the_range_and_nothing_else() {
        let mut words = storage(256);
        let mut builder = Builder::new(BASE, &mut words);
        builder.add_memory(BASE, 200 * F);
        builder.reserve(BASE + 70 * F, 2 * F);
        let mut allocator = builder.build();
        let range = BASE + 68 * F..BASE + 140 * F;
        // Frames 68, 69, then 72 on: 70 and 71 are reserved.
        assert_eq!(allocator.alloc_in(range.clone()), Some(BASE + 68 * F));
        assert_eq!(allocator.alloc_in(range.clone()), Some(BASE + 69 * F));
        assert_eq!(allocator.alloc_in(range.clone()), Some(BASE + 72 * F));
        assert_eq!(allocator.free_frames(), 200 - 2 - 3);
        // The lowest frame overall is still the lowest of the allocator.
        assert_eq!(allocator.alloc(), Some(BASE));
        let mut taken = 3;
        while allocator.alloc_in(range.clone()).is_some() {
            taken += 1;
        }
        assert_eq!(taken, 72 - 2);
        assert_eq!(allocator.alloc_in(range), None);
        // Partial frames at the ends are not in the range; an empty or
        // reversed range, or one outside the window, has no frame.
        assert_eq!(allocator.alloc_in(BASE + 1..BASE + F), None);
        assert_eq!(allocator.alloc_in(BASE + 5 * F..BASE + 5 * F), None);
        let (high, low) = (BASE + 9 * F, BASE + 3 * F);
        assert_eq!(allocator.alloc_in(high..low), None);
        assert_eq!(allocator.alloc_in(0..F), None);
        // What it handed out can be returned like any frame.
        assert_eq!(allocator.free(BASE + 69 * F), Ok(()));
        assert_eq!(
            allocator.alloc_in(BASE + 68 * F..BASE + 100 * F),
            Some(BASE + 69 * F)
        );
    }
}
