//! The kernel heap (ADR 0029): the allocator of `kernelheap` in a window of
//! its own in the upper half, grown on demand with frames from `physmem`
//! mapped read and write, never executable.

use alloc::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::fmt::{self, Write};
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicBool, Ordering, compiler_fence};

use kernelheap::{AllocError, FreeError, Heap};
use kernelvm::{HEAP_BASE, HEAP_WINDOW, PAGE_SIZE, Permissions};
use physmem::FrameAllocator;

use crate::arch::mmu;
use crate::log::klog;

/// The heap grows by at least this many pages at a time, so a run of small
/// requests does not walk the page tables once each.
const MIN_GROWTH_PAGES: usize = 16;
const WINDOW_PAGES: usize = (HEAP_WINDOW / PAGE_SIZE) as usize;

struct State {
    heap: Heap,
    frames: FrameAllocator<'static>,
    pages: usize,
}

/// One lock around the whole heap.
///
/// Today it only detects re-entry. The kernel has one core, no interrupts and
/// no threads, so the one way to get inside twice is a handler that allocates
/// while the code it interrupted was inside the allocator: a bug to report, not
/// to wait on. What each later item must do (ADR 0029):
///
/// - K6, interrupts: handlers must not allocate, or this lock masks interrupts
///   while held (save and restore `DAIF`).
/// - K7, threads: the same masking, because a tick must not switch away from a
///   thread that holds the lock, or a sleeping lock once a scheduler exists.
/// - Memory ordering: compiler fences keep the compiler from moving the heap's
///   accesses out of the critical section; there is no hardware ordering, and
///   none is needed with one core and no interrupts. Whoever makes an interrupt able to
///   interleave with the allocator replaces `with` as a whole (an atomic
///   `swap` or `compare_exchange` with acquire, a release store, and masking),
///   not just the flag.
/// - Panics: if the closure panics the flag stays set, and an allocation on the
///   panic path would hit the re-entry assertion. The panic handler must not
///   allocate (it does not today), and K6 must keep it so.
/// - K11, cores: a spin lock on `compare_exchange`. A plain load and store
///   cannot exclude a second core. Not done now because exclusive accesses to
///   memory that is Device or non-cacheable may never succeed on real cores
///   (the direct path runs uncached), and `memory.rs` avoids them for the same
///   reason.
struct Locked<T> {
    busy: AtomicBool,
    value: UnsafeCell<T>,
}

// SAFETY: access goes through `with`, which refuses a second entry. With one
// core and no interrupts that is exclusion; K6, K7 and K11 must change `with`
// before they exist (see the type's documentation).
unsafe impl<T: Send> Sync for Locked<T> {}

impl<T> Locked<T> {
    const fn new(value: T) -> Self {
        Self {
            busy: AtomicBool::new(false),
            value: UnsafeCell::new(value),
        }
    }

    fn with<R>(&self, use_value: impl FnOnce(&mut T) -> R) -> R {
        // A plain load and store, not `swap`: see the type's documentation.
        assert!(
            !self.busy.load(Ordering::Relaxed),
            "kernel heap entered while it was in use"
        );
        self.busy.store(true, Ordering::Relaxed);
        compiler_fence(Ordering::SeqCst);
        // SAFETY: the flag above is set, so this is the only reference to the
        // value until it is cleared below.
        let result = use_value(unsafe { &mut *self.value.get() });
        compiler_fence(Ordering::SeqCst);
        self.busy.store(false, Ordering::Relaxed);
        result
    }
}

pub struct KernelHeap {
    state: Locked<Option<State>>,
}

impl KernelHeap {
    pub const fn new() -> Self {
        Self {
            state: Locked::new(None),
        }
    }
}

#[global_allocator]
static HEAP: KernelHeap = KernelHeap::new();

#[derive(Debug)]
pub enum InitError {
    /// No frame, or no table frame, for the first pages.
    NoMemory,
    AlreadyInitialised,
}

impl fmt::Display for InitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoMemory => "no frame left for the first pages of the heap",
            Self::AlreadyInitialised => "the heap was initialised twice",
        })
    }
}

/// Takes ownership of the frame allocator, maps the heap's first pages and
/// starts the allocator. Nothing may allocate before this: a request until
/// then is refused.
pub fn init(frames: FrameAllocator<'static>) -> Result<(), InitError> {
    HEAP.state.with(|state| {
        if state.is_some() {
            return Err(InitError::AlreadyInitialised);
        }
        let mut frames = frames;
        let pages = mmu::grow_heap(&mut frames, HEAP_BASE, MIN_GROWTH_PAGES);
        if pages == 0 {
            return Err(InitError::NoMemory);
        }
        let mut heap = Heap::empty();
        // SAFETY: `grow_heap` mapped `pages` pages at `HEAP_BASE`, read and
        // write, and nothing else is ever mapped or used there.
        unsafe { heap.init(HEAP_BASE as *mut u8, pages * PAGE_SIZE as usize) }
            .expect("a mapped, aligned, non-empty range");
        *state = Some(State {
            heap,
            frames,
            pages,
        });
        Ok(())
    })
}

impl State {
    /// Adds pages until a request that was refused for `needed` more bytes can
    /// fit, or says it cannot: no more window, or fewer frames than that.
    /// Checked before mapping anything, so an impossible request (a huge size)
    /// does not take every frame on its way to being refused. The frames the
    /// page tables need come from the same allocator, so they count: one level
    /// 3 table per 512 pages, and up to three more where the range starts and
    /// ends.
    fn grow(&mut self, needed: usize) -> bool {
        let required = needed.div_ceil(PAGE_SIZE as usize);
        let room = WINDOW_PAGES - self.pages;
        let frames_needed = required.saturating_add(required.div_ceil(512) + 3);
        if required > room || frames_needed > self.frames.free_frames() {
            return false;
        }
        let wanted = required.max(MIN_GROWTH_PAGES).min(room);
        let added = mmu::grow_heap(
            &mut self.frames,
            HEAP_BASE + self.pages as u64 * PAGE_SIZE,
            wanted,
        );
        if added == 0 {
            return false;
        }
        // SAFETY: `grow_heap` mapped `added` pages read and write right after
        // the heap's end, which is where `pages` pages from `HEAP_BASE` end.
        unsafe { self.heap.extend(added * PAGE_SIZE as usize) }
            .expect("the pages just mapped follow the heap");
        self.pages += added;
        true
    }

    fn alloc(&mut self, layout: Layout) -> Result<NonNull<u8>, AllocError> {
        loop {
            match self.heap.alloc(layout) {
                Err(AllocError::OutOfMemory { needed }) if self.grow(needed) => {}
                other => return other,
            }
        }
    }
}

/// Allocates, or says why not. The allocator the compiler calls turns a refusal
/// into a null pointer; kernel code that must survive one calls this.
pub fn try_alloc(layout: Layout) -> Result<NonNull<u8>, AllocError> {
    HEAP.state.with(|state| match state {
        Some(state) => state.alloc(layout),
        None => Err(AllocError::OutOfMemory {
            needed: layout.size(),
        }),
    })
}

/// Returns a block, or says what is wrong with the pointer.
///
/// # Safety
/// As [`kernelheap::Heap::dealloc`]: `layout` is the one the block was
/// allocated with.
pub unsafe fn try_free(pointer: NonNull<u8>, layout: Layout) -> Result<(), FreeError> {
    HEAP.state.with(|state| match state {
        // SAFETY: the caller's promise about `layout`.
        Some(state) => unsafe { state.heap.dealloc(pointer, layout) },
        None => Err(FreeError::Outside),
    })
}

// SAFETY: `alloc` returns blocks the heap carved out of memory it owns, apart
// from every other live block and aligned as asked, or null; `dealloc` takes
// them back. A zero-sized request is the caller's contract violation and is
// refused with null rather than served.
unsafe impl GlobalAlloc for KernelHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        try_alloc(layout).map_or(ptr::null_mut(), NonNull::as_ptr)
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        let Some(pointer) = NonNull::new(pointer) else {
            return;
        };
        // SAFETY: `GlobalAlloc` promises the layout is the one given to `alloc`.
        if let Err(error) = unsafe { try_free(pointer, layout) } {
            // A bad free is a bug in whoever freed: report it, never go on.
            panic!("kernel heap: {error} ({pointer:p}, {layout:?})");
        }
    }
}

#[derive(Clone, Copy)]
pub struct Stats {
    pub size: usize,
    pub used: usize,
    pub free_blocks: usize,
    pub pages: usize,
    pub free_frames: usize,
}

#[cfg(feature = "selftest-heap-exhaustion")]
impl Stats {
    /// Why the heap cannot grow any more, when it cannot: whichever of the
    /// window and the frames would run out first.
    fn limit(&self) -> &'static str {
        if WINDOW_PAGES - self.pages <= self.free_frames {
            "the window is full"
        } else {
            "no frame is left"
        }
    }
}

pub fn stats() -> Stats {
    HEAP.state.with(|state| {
        let state = state.as_ref().expect("the heap is not initialised");
        Stats {
            size: state.heap.size(),
            used: state.heap.used_bytes(),
            free_blocks: state.heap.free_blocks(),
            pages: state.pages,
            free_frames: state.frames.free_frames(),
        }
    })
}

struct Random(u64);

impl Random {
    fn below(&mut self, limit: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % limit
    }
}

const STRESS_SLOTS: usize = 192;
const STRESS_ROUNDS: usize = 6000;

/// Exercises the heap through the interface the rest of the kernel will use and
/// panics if it misbehaves: many allocations of mixed sizes and alignments with
/// their contents checked, some big enough to make the heap grow, everything
/// freed in a scrambled order and merged back into one block; then the pages it
/// mapped are read back from the live tables, and invalid requests and a double
/// free are refused. Bounded work: it does not grow with the size of RAM.
pub fn selftest(mut out: impl Write) {
    use alloc::vec::Vec;

    let before = stats();
    assert_eq!(before.used, 0, "heap in use before its self-test");

    // The blocks of up to 300 KB make the heap grow to about 440 pages: only
    // on a machine with the frames for that, so a very small one still boots.
    let big_blocks = before.free_frames >= 2048;
    let mut random = Random(0x2545_f491_4f6c_dd1d);
    let mut slots: [Option<(Vec<u8>, u8)>; STRESS_SLOTS] = [const { None }; STRESS_SLOTS];
    let (mut allocations, mut live_bytes, mut peak) = (0usize, 0usize, 0usize);
    for round in 0..STRESS_ROUNDS {
        let slot = &mut slots[random.below(STRESS_SLOTS as u64) as usize];
        if let Some((buffer, fill)) = slot.take() {
            assert!(
                buffer.iter().all(|&byte| byte == fill),
                "heap contents changed under a live block (round {round})"
            );
            live_bytes -= buffer.len();
            continue;
        }
        let length = match random.below(40) {
            0 if big_blocks => 20_000 + random.below(300_000) as usize,
            0 => 1 + random.below(2000) as usize,
            1..=9 => 1 + random.below(8192) as usize,
            _ => 1 + random.below(300) as usize,
        };
        let mut buffer = Vec::new();
        buffer.try_reserve_exact(length).unwrap_or_else(|_| {
            panic!(
                "heap refused {length} bytes, {} frames free",
                stats().free_frames
            )
        });
        let fill = 1 + random.below(255) as u8;
        buffer.resize(length, fill);
        live_bytes += length;
        peak = peak.max(live_bytes);
        allocations += 1;
        *slot = Some((buffer, fill));
    }
    for (buffer, fill) in slots.iter().flatten() {
        assert!(buffer.iter().all(|byte| byte == fill), "contents changed");
    }
    drop(slots);
    let after = stats();
    assert_eq!(after.used, 0, "the heap leaked after everything was freed");
    assert_eq!(
        after.free_blocks, 1,
        "the freed blocks did not merge into one"
    );
    klog!(
        Ok,
        out,
        "heap: stress: {allocations} allocations, {} KiB live at the peak, grown to {} KiB in {} pages, all freed, {} free block",
        peak >> 10,
        after.size >> 10,
        after.pages,
        after.free_blocks
    );

    check_mapping(after.pages, &mut out);

    let refused = |layout: Layout, expected: AllocError| {
        assert_eq!(try_alloc(layout).err(), Some(expected), "{layout:?}");
    };
    refused(
        Layout::from_size_align(0, 8).expect("a layout"),
        AllocError::ZeroSize,
    );
    refused(
        Layout::from_size_align(64, 8192).expect("a layout"),
        AllocError::AlignmentTooLarge,
    );
    // On a machine with more frames than the window holds these are refused for
    // the window instead; the boots with less RAM are the ones that reach the
    // frame checks, the table-frame one included.
    // Past the window, and past the frames the machine has left once the free
    // heap is counted (it is all one block, so it merges with what growth
    // adds): the first must be refused before growth takes every frame on its
    // way to failing.
    let more_than_frames = Layout::from_size_align(
        (after.free_frames + after.pages + 16) * PAGE_SIZE as usize,
        8,
    )
    .expect("a layout");
    // Fewer pages than there are frames, but not once the page tables that
    // map them are counted: refused too, without taking a single frame.
    let without_tables = Layout::from_size_align(
        (after.free_frames + after.pages - 1) * PAGE_SIZE as usize,
        8,
    )
    .expect("a layout");
    let past_window = Layout::from_size_align(1 << 50, 8).expect("a layout");
    for huge in [more_than_frames, without_tables, past_window] {
        assert!(
            matches!(try_alloc(huge), Err(AllocError::OutOfMemory { .. })),
            "{huge:?} was not refused"
        );
        let unchanged = stats();
        assert_eq!(
            (unchanged.size, unchanged.used, unchanged.free_frames),
            (after.size, 0, after.free_frames),
            "a refused request changed the heap or took frames"
        );
    }
    klog!(
        Ok,
        out,
        "heap: refused zero size, alignment 8192 and oversized requests, and nothing changed"
    );

    // An empty heap starts on a page, so every alignment would hold by luck:
    // a small block in front makes the next free address a misaligned one.
    let pad_layout = Layout::from_size_align(16, 16).expect("a layout");
    let pad = try_alloc(pad_layout).expect("a small block");
    for align in [16, 64, 256, 4096] {
        let layout = Layout::from_size_align(100, align).expect("a layout");
        let block = try_alloc(layout).expect("an aligned block");
        assert_eq!(
            block.as_ptr() as usize % align,
            0,
            "alignment {align} not kept"
        );
        // SAFETY: allocated just above with this layout.
        assert_eq!(unsafe { try_free(block, layout) }, Ok(()));
    }
    // SAFETY: allocated above with `pad_layout`.
    assert_eq!(unsafe { try_free(pad, pad_layout) }, Ok(()));
    klog!(
        Ok,
        out,
        "heap: blocks aligned to 16, 64, 256 and 4096 bytes"
    );

    let layout = Layout::from_size_align(100, 8).expect("a layout");
    let block = try_alloc(layout).expect("a small block");
    // SAFETY: the block was just allocated with this layout.
    assert_eq!(unsafe { try_free(block, layout) }, Ok(()));
    // SAFETY: the block was freed above; freeing it again is the very error
    // this checks is refused, and the heap sees it before touching memory.
    let second = unsafe { try_free(block, layout) };
    assert_eq!(
        second,
        Err(FreeError::Overlap),
        "a double free was accepted"
    );
    klog!(Ok, out, "heap: double free refused");
}

/// Reads the heap's pages back from the live tables: each is read and write,
/// never executable, maps a frame of its own, and the pages on either side of
/// the mapped part do not translate.
fn check_mapping(pages: usize, mut out: impl Write) {
    use alloc::vec::Vec;

    let mut frames: Vec<u64> = Vec::new();
    frames
        .try_reserve_exact(pages)
        .expect("room to list the heap's frames");
    for page in 0..pages as u64 {
        let (frame, permissions) = mmu::upper_page(HEAP_BASE + page * PAGE_SIZE)
            .unwrap_or_else(|| panic!("heap page {page} is not mapped"));
        assert_eq!(
            permissions,
            Some(Permissions::ReadWrite),
            "heap page {page} is not read and write, never executable"
        );
        frames.push(frame);
    }
    frames.sort_unstable();
    assert!(
        frames.windows(2).all(|pair| pair[0] != pair[1]),
        "two heap pages share a frame"
    );
    drop(frames);
    assert!(
        mmu::upper_page(HEAP_BASE - PAGE_SIZE).is_none(),
        "the page below the heap is mapped"
    );
    assert!(
        mmu::upper_page(HEAP_BASE + pages as u64 * PAGE_SIZE).is_none(),
        "the page above the heap is mapped"
    );
    klog!(
        Ok,
        out,
        "heap: {pages} pages mapped read write and never executed at {HEAP_BASE:#x}, the pages around them unmapped"
    );
}

/// Takes memory until the heap refuses, reports why, then frees it all and
/// checks the heap works again.
#[cfg(feature = "selftest-heap-exhaustion")]
pub fn exhaust(mut out: impl Write) {
    use alloc::vec::Vec;

    const BLOCK: usize = 1 << 20;
    let layout = Layout::from_size_align(BLOCK, 4096).expect("a layout");
    // The list of blocks lives in the heap's own memory, so it is taken
    // before the heap fills and never grows.
    let mut blocks: Vec<NonNull<u8>> = Vec::new();
    blocks
        .try_reserve_exact(WINDOW_PAGES * PAGE_SIZE as usize / BLOCK)
        .expect("room for the list of blocks");
    let error = loop {
        match try_alloc(layout) {
            Ok(block) => blocks.push(block),
            Err(error) => break error,
        }
    };
    let full = stats();
    assert!(matches!(error, AllocError::OutOfMemory { .. }));
    klog!(
        Ok,
        out,
        "heap: exhausted after {} MiB in blocks of 1 MiB: {error}; {}",
        blocks.len(),
        full.limit()
    );
    klog!(
        Info,
        out,
        "heap: {} KiB mapped in {} pages, {} frames left",
        full.size >> 10,
        full.pages,
        full.free_frames
    );
    for block in blocks.drain(..) {
        // SAFETY: each block was allocated above with `layout`.
        assert_eq!(unsafe { try_free(block, layout) }, Ok(()));
    }
    let freed = stats();
    // What stays in use is the list of blocks itself.
    let list = (blocks.capacity() * size_of::<NonNull<u8>>()).next_multiple_of(16);
    assert_eq!(freed.used, list, "blocks were lost");
    let again = try_alloc(layout).expect("the heap is not usable after freeing everything");
    // SAFETY: allocated just above with `layout`.
    assert_eq!(unsafe { try_free(again, layout) }, Ok(()));
    klog!(
        Ok,
        out,
        "heap: usable again after freeing everything, {} free block",
        stats().free_blocks
    );
}
