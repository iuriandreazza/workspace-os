//! Physical memory: the frame allocator, built from the device tree memory map
//! minus everything that is already in use.

use alloc::vec::Vec;
use core::fmt::{self, Write};
use core::ops::Range;
use core::sync::atomic::{AtomicBool, Ordering};

use physmem::{Builder, FRAME_SIZE, FrameAllocator, FreeError, storage_words};

use crate::arch;
use crate::board::{Board, Entry};
use crate::log::klog;

/// The allocator manages a window of this many frames (4 GiB), counted from the
/// lowest RAM address. Its bitmaps live in the kernel's `.bss`, so they are
/// part of the image and reserved with it. RAM beyond the window is reported
/// and left unused.
const WINDOW_FRAMES: usize = 1 << 20;
const STORAGE_WORDS: usize = storage_words(WINDOW_FRAMES);

static mut FRAME_STORAGE: [u64; STORAGE_WORDS] = [0; STORAGE_WORDS];
static STORAGE_TAKEN: AtomicBool = AtomicBool::new(false);

/// A device tree with more ranges than this is refused instead of processed:
/// each range costs time proportional to the window, and a hostile blob can
/// hold tens of thousands of them. Real trees hold a handful.
const MAX_RANGES: usize = 4096;

/// The self-test checks this many frames at the start of each range the device
/// tree reserves; the fixed ranges are checked whole.
const SAMPLED_FRAMES_PER_RANGE: u64 = 256;

pub enum InitError {
    DeviceTree(devicetree::Error),
    NoUsableMemory,
    TooManyRanges,
}

impl fmt::Display for InitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DeviceTree(error) => write!(f, "{error}"),
            Self::NoUsableMemory => f.write_str("no usable memory left after the reservations"),
            Self::TooManyRanges => f.write_str("the device tree holds too many memory ranges"),
        }
    }
}

impl From<devicetree::Error> for InitError {
    fn from(error: devicetree::Error) -> Self {
        Self::DeviceTree(error)
    }
}

fn take_storage() -> &'static mut [u64] {
    // A plain load and store, not `swap`: that compiles to an exclusive
    // load/store loop, which may never succeed on real cores while the MMU is
    // off and memory is Device. Only the boot core runs, so nothing races.
    assert!(
        !STORAGE_TAKEN.load(Ordering::Relaxed),
        "frame allocator storage taken twice"
    );
    STORAGE_TAKEN.store(true, Ordering::Relaxed);
    // SAFETY: the flag above lets this run once, so this is the only reference
    // ever made to `FRAME_STORAGE`, and it lives for the rest of the boot.
    unsafe { &mut *core::ptr::addr_of_mut!(FRAME_STORAGE) }
}

/// What the kernel itself occupies, which no device tree says.
fn fixed_reservations(board: &Board) -> [(&'static str, Range<u64>); 4] {
    let widen = |range: Range<usize>| range.start as u64..range.end as u64;
    let blob = &board.device_tree;
    [
        ("kernel image", widen(arch::kernel_image())),
        ("boot stack", widen(arch::boot_stack())),
        ("exception stack", widen(arch::exception_stack())),
        (
            "device tree",
            blob.base..blob.base.saturating_add(blob.size),
        ),
    ]
}

pub fn init(board: &Board) -> Result<FrameAllocator<'static>, InitError> {
    let mut lowest = u64::MAX;
    let mut ranges = 0;
    board.for_each_memory_region(|region| {
        ranges += 1;
        if region.size > 0 {
            lowest = lowest.min(region.base);
        }
    })?;
    board.for_each_reserved_region(|_| ranges += 1)?;
    if ranges > MAX_RANGES {
        return Err(InitError::TooManyRanges);
    }
    if lowest == u64::MAX {
        return Err(InitError::NoUsableMemory);
    }

    let mut builder = Builder::new(lowest, take_storage());
    board.for_each_memory_region(|region| builder.add_memory(region.base, region.size))?;
    board.for_each_reserved_region(|region| builder.reserve(region.base, region.size))?;
    for (_, range) in fixed_reservations(board) {
        builder.reserve(range.start, range.end - range.start);
    }

    let frames = builder.build();
    if frames.free_frames() == 0 {
        return Err(InitError::NoUsableMemory);
    }
    Ok(frames)
}

pub fn report(frames: &FrameAllocator, board: &Board, mut out: impl Write) {
    for (name, range) in fixed_reservations(board) {
        klog!(
            Info,
            out,
            "reserved {name} {:#x}..{:#x}",
            range.start,
            range.end
        );
    }
    let mut declared = 0;
    // A device tree that parsed for `init` parses again; a failure shows as zero.
    let _ = board.for_each_reserved_region(|_| declared += 1);
    let source = match board.entry {
        Entry::Direct => "the device tree",
        Entry::Bootloader { .. } => "the boot map and device tree",
    };
    klog!(Info, out, "reserved by {source}: {declared} regions");
    if frames.dropped_bytes() > 0 {
        klog!(
            Info,
            out,
            "{} MiB of memory lies outside the allocator window and is unused",
            frames.dropped_bytes() >> 20
        );
    }
    klog!(
        Info,
        out,
        "frames: {} in memory map, {} reserved, {} free",
        frames.memory_frames(),
        frames.reserved_frames(),
        frames.free_frames()
    );
}

/// Calls `visit` with each frame address of `range` that falls in `window`.
fn for_each_frame_in(range: Range<u64>, window: Range<u64>, mut visit: impl FnMut(u64)) {
    let start = range.start.max(window.start) & !(FRAME_SIZE - 1);
    let end = range.end.min(window.end);
    let mut address = start;
    while address < end {
        visit(address);
        address += FRAME_SIZE;
    }
}

/// Exercises the allocator on the real memory map and panics if it misbehaves:
/// drains it, checks that nothing reserved was handed out, returns everything,
/// and reuses a frame. The allocator ends as it began.
pub fn selftest(frames: &mut FrameAllocator, board: &Board, mut out: impl Write) {
    let free_at_start = frames.free_frames();

    let mut handed_out = 0;
    while frames.alloc().is_some() {
        handed_out += 1;
    }
    assert_eq!(
        handed_out, free_at_start,
        "drained a different number of frames"
    );
    assert_eq!(
        frames.free_frames(),
        0,
        "drained allocator reports free frames"
    );
    assert!(
        frames.alloc().is_none(),
        "exhausted allocator handed out a frame"
    );
    klog!(
        Ok,
        out,
        "frames: exhausted after {handed_out} allocations, next request refused"
    );

    // A reserved frame must not have been handed out: freeing it would succeed.
    let window = frames.window();
    let mut check_reserved = |range: Range<u64>| {
        for_each_frame_in(range, window.clone(), |address| {
            assert!(
                frames.free(address).is_err(),
                "frame {address:#x} was reserved but handed out"
            );
        });
    };
    for (_, range) in fixed_reservations(board) {
        check_reserved(range);
    }
    let reserved_by_tree = board.for_each_reserved_region(|region| {
        let start = region.base.max(window.start);
        let end = region.base.saturating_add(region.size);
        check_reserved(start..end.min(start.saturating_add(SAMPLED_FRAMES_PER_RANGE * FRAME_SIZE)));
    });
    assert!(
        reserved_by_tree.is_ok(),
        "device tree reservations vanished"
    );

    // Everything handed out comes back, and a frame that was not is refused.
    let mut returned = 0;
    for_each_frame_in(window.clone(), window, |address| {
        if frames.free(address).is_ok() {
            returned += 1;
        }
    });
    assert_eq!(
        returned, handed_out,
        "returned a different number of frames"
    );
    assert_eq!(frames.free_frames(), free_at_start, "frames were lost");
    klog!(
        Ok,
        out,
        "frames: returned {returned}, {} free again",
        frames.free_frames()
    );

    let first = frames.alloc().expect("no frame after returning all");
    let second = frames.alloc().expect("only one frame available");
    assert_ne!(first, second, "the same frame was handed out twice");
    assert_eq!(frames.free(first), Ok(()));
    assert_eq!(frames.alloc(), Some(first), "a freed frame was not reused");
    klog!(Ok, out, "frames: reused {first:#x} after freeing it");

    assert_eq!(frames.free(second), Ok(()));
    assert_eq!(frames.free(second), Err(FreeError::NotAllocated));
    assert_eq!(frames.free(first + 1), Err(FreeError::Unaligned));
    assert_eq!(frames.free(first), Ok(()));
    assert_eq!(frames.free_frames(), free_at_start, "frames were lost");
    klog!(Ok, out, "frames: double free refused");
}

/// What to hand to the allocator (ADR 0033), worked out before the heap is
/// locked: both lists allocate.
pub struct Reclaim {
    /// The BootloaderReclaimable regions.
    ranges: Vec<Range<u64>>,
    /// What stays reserved inside them: the kernel itself and whatever the
    /// device tree reserves.
    keep: Vec<Range<u64>>,
    released: usize,
}

impl Reclaim {
    /// The BootloaderReclaimable memory (boot info, memory map, device tree
    /// copy, the bootloader's tables, former boot services memory), which
    /// nothing may read any more. The caller has copied what it keeps
    /// (`Board::into_owned`) and dropped the lower half, whose tables were in
    /// this memory.
    pub fn plan(board: &Board) -> Self {
        let widen = |range: Range<usize>| range.start as u64..range.end as u64;
        let mut keep: Vec<Range<u64>> = [
            arch::kernel_image(),
            arch::boot_stack(),
            arch::exception_stack(),
        ]
        .into_iter()
        .map(widen)
        .collect();
        let tree = board.for_each_tree_reservation(|region| {
            keep.push(region.base..region.base.saturating_add(region.size));
        });
        assert!(tree.is_ok(), "device tree reservations vanished");
        let mut ranges = Vec::new();
        board.for_each_reclaimable_region(|region| {
            ranges.push(region.base..region.base.saturating_add(region.size));
        });
        Self {
            ranges,
            keep,
            released: 0,
        }
    }

    /// Gives the frames to the allocator. Allocates nothing.
    pub fn release(&mut self, frames: &mut FrameAllocator, mut out: impl Write) {
        let free_before = frames.free_frames();
        self.released = self
            .ranges
            .iter()
            .map(|range| frames.release(range.clone(), &self.keep))
            .sum();
        assert_eq!(
            frames.free_frames(),
            free_before + self.released,
            "the allocator's count of free frames does not follow what it released"
        );
        if self.ranges.is_empty() {
            klog!(
                Info,
                out,
                "frames: no bootloader memory to reclaim (direct load)"
            );
        } else {
            klog!(
                Ok,
                out,
                "frames: reclaimed {} frames ({} KiB) of {} bootloader memory regions, {} free now",
                self.released,
                self.released * FRAME_SIZE as usize / 1024,
                self.ranges.len(),
                frames.free_frames()
            );
        }
    }
}

/// The words of a frame the self-test writes: a few across the page, so a
/// frame that something else still uses shows in at least one.
const PATTERN_OFFSETS: [usize; 9] = [0, 512, 1024, 1536, 2048, 2560, 3072, 3584, 4088];

fn pattern(frame: u64, word: usize) -> u64 {
    frame.rotate_left(17) ^ 0x5eed_0000_0000_0000 ^ word as u64
}

/// Takes every frame that `reclaim` released, writes a pattern in each through
/// the linear map, reads it back, and gives the frames back (ADR 0033). A frame
/// that two owners shared, or that was handed out twice, or that the linear map
/// does not reach, shows here; and the caller digests the hand-over again
/// afterwards, which a board that still pointed into the memory would fail.
pub fn reclaim_selftest(frames: &mut FrameAllocator, reclaimed: &Reclaim, mut out: impl Write) {
    if reclaimed.ranges.is_empty() {
        return;
    }
    let mut used = 0usize;
    for range in &reclaimed.ranges {
        while let Some(frame) = frames.alloc_in(range.clone()) {
            for (word, &offset) in PATTERN_OFFSETS.iter().enumerate() {
                let at = arch::pointer_to(frame + offset as u64) as *mut u64;
                // SAFETY: the frame was just handed out, so nothing else uses
                // it, and the linear map covers every frame of RAM the allocator
                // manages, read and write.
                unsafe { at.write_volatile(pattern(frame, word)) };
            }
            used += 1;
        }
    }
    assert_eq!(
        used, reclaimed.released,
        "the allocator handed out a different number of reclaimed frames than it released"
    );
    let mut verified = 0usize;
    for range in &reclaimed.ranges {
        for_each_frame_in(range.clone(), range.clone(), |frame| {
            // Only a frame this test took is allocated: a kept one never was.
            if frames.free(frame).is_err() {
                return;
            }
            for (word, &offset) in PATTERN_OFFSETS.iter().enumerate() {
                let at = arch::pointer_to(frame + offset as u64) as *const u64;
                // SAFETY: as above; the frame is only read.
                let found = unsafe { at.read_volatile() };
                assert_eq!(
                    found,
                    pattern(frame, word),
                    "reclaimed frame {frame:#x} changed while the test held it"
                );
            }
            verified += 1;
        });
    }
    assert_eq!(
        verified, used,
        "a reclaimed frame the test took was not returned"
    );
    klog!(
        Ok,
        out,
        "frames: reclaimed memory used: {used} frames allocated, written and read back through the linear map, then freed"
    );
}

/// RAM the allocator does not manage (the part of a big machine beyond its
/// window) is still in the linear map (ADR 0033), and nothing else in a boot
/// touches it. If the highest free frame of the machine lies outside the
/// allocator's window, write a pattern in it through the linear map, read it
/// back. On a machine of more than 512 GiB it is
/// the one place the boot shows that the 48-bit regime reaches what a 39-bit
/// one could not.
pub fn linear_map_selftest(board: &Board, window: Range<u64>, mut out: impl Write) {
    let mut highest: Option<u64> = None;
    let usable = board.for_each_usable_region(|region| {
        let end = region.base.saturating_add(region.size) & !(FRAME_SIZE - 1);
        if end > window.end && end - FRAME_SIZE >= region.base {
            highest = highest.max(Some(end - FRAME_SIZE));
        }
    });
    assert!(usable.is_ok(), "the memory regions vanished");
    let Some(frame) = highest else {
        return;
    };
    // A frame the device tree reserves is not free.
    let mut reserved = false;
    let _ = board.for_each_tree_reservation(|region| {
        reserved |=
            region.base < frame + FRAME_SIZE && frame < region.base.saturating_add(region.size);
    });
    if reserved {
        klog!(
            Info,
            out,
            "linear map: the highest free frame {frame:#x} is reserved by the device tree, not tested"
        );
        return;
    }
    let at = arch::pointer_to(frame) as *mut u64;
    // SAFETY: the frame is RAM nothing uses (outside the allocator's window and
    // not reserved), and the linear map covers it read and write. It is written
    // before it is read, so memory that was never initialised is not read.
    let found = unsafe {
        at.write_volatile(pattern(frame, 0));
        at.read_volatile()
    };
    assert_eq!(
        found,
        pattern(frame, 0),
        "the linear map read other memory than it wrote at {frame:#x}"
    );
    klog!(
        Ok,
        out,
        "linear map: frame {frame:#x} ({} GiB up), outside the allocator's window, written and read back at {:#x}",
        frame >> 30,
        at as usize
    );
}
