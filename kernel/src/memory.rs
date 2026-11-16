//! Physical memory: the frame allocator, built from the device tree memory map
//! minus everything that is already in use.

use core::fmt::{self, Write};
use core::ops::Range;
use core::sync::atomic::{AtomicBool, Ordering};

use physmem::{Builder, FRAME_SIZE, FrameAllocator, FreeError, storage_words};

use crate::arch;
use crate::board::Board;

/// The allocator manages a window of this many frames (4 GiB), counted from the
/// lowest RAM address. Its bitmaps live in the kernel's `.bss`, so they are
/// part of the image and reserved with it. RAM beyond the window is reported
/// and left unused.
const WINDOW_FRAMES: usize = 1 << 20;
const STORAGE_WORDS: usize = storage_words(WINDOW_FRAMES);

static mut FRAME_STORAGE: [u64; STORAGE_WORDS] = [0; STORAGE_WORDS];
static STORAGE_TAKEN: AtomicBool = AtomicBool::new(false);

pub enum InitError {
    DeviceTree(devicetree::Error),
    NoUsableMemory,
}

impl fmt::Display for InitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DeviceTree(error) => write!(f, "{error}"),
            Self::NoUsableMemory => f.write_str("no usable memory left after the reservations"),
        }
    }
}

impl From<devicetree::Error> for InitError {
    fn from(error: devicetree::Error) -> Self {
        Self::DeviceTree(error)
    }
}

fn take_storage() -> &'static mut [u64] {
    assert!(
        !STORAGE_TAKEN.swap(true, Ordering::Relaxed),
        "frame allocator storage taken twice"
    );
    // SAFETY: the flag above lets this run once, so this is the only reference
    // ever made to `FRAME_STORAGE`, and it lives for the rest of the boot.
    unsafe { &mut *core::ptr::addr_of_mut!(FRAME_STORAGE) }
}

/// What the kernel itself occupies, which no device tree says.
fn fixed_reservations(board: &Board) -> [(&'static str, Range<u64>); 3] {
    let widen = |range: Range<usize>| range.start as u64..range.end as u64;
    let blob = &board.device_tree;
    [
        ("kernel image", widen(arch::kernel_image())),
        ("boot stack", widen(arch::boot_stack())),
        (
            "device tree",
            blob.base..blob.base.saturating_add(blob.size),
        ),
    ]
}

pub fn init(board: &Board) -> Result<FrameAllocator<'static>, InitError> {
    let mut lowest = u64::MAX;
    board.fdt.for_each_memory_region(|region| {
        if region.size > 0 {
            lowest = lowest.min(region.base);
        }
    })?;
    if lowest == u64::MAX {
        return Err(InitError::NoUsableMemory);
    }

    let mut builder = Builder::new(lowest, take_storage());
    board
        .fdt
        .for_each_memory_region(|region| builder.add_memory(region.base, region.size))?;
    board
        .fdt
        .for_each_reserved_region(|region| builder.reserve(region.base, region.size))?;
    for (_, range) in fixed_reservations(board) {
        builder.reserve(range.start, range.end - range.start);
    }

    let frames = builder.build();
    if frames.free_frames() == 0 {
        return Err(InitError::NoUsableMemory);
    }
    Ok(frames)
}

pub fn report(frames: &FrameAllocator, board: &Board, out: &mut impl Write) {
    for (name, range) in fixed_reservations(board) {
        let _ = writeln!(
            out,
            "workspace-os kernel: reserved {name} {:#x}..{:#x}",
            range.start, range.end
        );
    }
    let mut declared = 0;
    // A device tree that parsed for `init` parses again; a failure shows as zero.
    let _ = board.fdt.for_each_reserved_region(|_| declared += 1);
    let _ = writeln!(
        out,
        "workspace-os kernel: reserved by the device tree: {declared} regions"
    );
    if frames.dropped_bytes() > 0 {
        let _ = writeln!(
            out,
            "workspace-os kernel: {} MiB of memory lies outside the allocator window and is unused",
            frames.dropped_bytes() >> 20
        );
    }
    let _ = writeln!(
        out,
        "workspace-os kernel: frames: {} in memory map, {} reserved, {} free",
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
pub fn selftest(frames: &mut FrameAllocator, board: &Board, out: &mut impl Write) {
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
    let _ = writeln!(
        out,
        "workspace-os kernel: frames: exhausted after {handed_out} allocations, next request refused"
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
    let reserved_by_tree = board.fdt.for_each_reserved_region(|region| {
        check_reserved(region.base..region.base.saturating_add(region.size));
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
    let _ = writeln!(
        out,
        "workspace-os kernel: frames: returned {returned}, {} free again",
        frames.free_frames()
    );

    let first = frames.alloc().expect("no frame after returning all");
    let second = frames.alloc().expect("only one frame available");
    assert_ne!(first, second, "the same frame was handed out twice");
    assert_eq!(frames.free(first), Ok(()));
    assert_eq!(frames.alloc(), Some(first), "a freed frame was not reused");
    let _ = writeln!(
        out,
        "workspace-os kernel: frames: reused {first:#x} after freeing it"
    );

    assert_eq!(frames.free(second), Ok(()));
    assert_eq!(frames.free(second), Err(FreeError::NotAllocated));
    assert_eq!(frames.free(first + 1), Err(FreeError::Unaligned));
    assert_eq!(frames.free(first), Ok(()));
    assert_eq!(frames.free_frames(), free_at_start, "frames were lost");
    let _ = writeln!(out, "workspace-os kernel: frames: double free refused");
}
