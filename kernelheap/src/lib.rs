//! The kernel heap's allocator (ADR 0029): an address-ordered free list, first
//! fit, with coalescing.
//!
//! It knows nothing about frames, page tables or locks. It manages bytes the
//! caller has mapped and hands to it with [`Heap::init`] and [`Heap::extend`];
//! growing is the caller's decision, taken when `alloc` says `OutOfMemory`.
//! Every failure is a value: no request can panic, hang or corrupt the list.

#![cfg_attr(not(test), no_std)]

use core::alloc::Layout;
use core::ptr::NonNull;

/// Every block is a multiple of this and starts on it, which makes room for a
/// free block's header in any block and keeps every remainder usable.
pub const GRANULE: usize = 16;
/// The largest alignment served. Nothing in the kernel needs more: aligned
/// buffers are frames.
pub const MAX_ALIGN: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AllocError {
    ZeroSize,
    AlignmentTooLarge,
    /// Rounding the size or aligning the block overflows the address space.
    SizeOverflow,
    /// No free block fits. Adding `needed` bytes with `extend` (rounded up to a
    /// granule) makes the same request fit: the size, rounded, plus the room
    /// alignment may cost, less the free block that already ends the heap.
    OutOfMemory {
        needed: usize,
    },
}

impl core::fmt::Display for AllocError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ZeroSize => f.write_str("allocation of zero bytes"),
            Self::AlignmentTooLarge => write!(f, "alignment above {MAX_ALIGN}"),
            Self::SizeOverflow => f.write_str("allocation size overflows"),
            Self::OutOfMemory { needed } => write!(f, "heap exhausted, {needed} bytes needed"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FreeError {
    /// The block is not wholly inside the heap.
    Outside,
    /// The pointer is not on a granule.
    Unaligned,
    /// Part of the block is already free: a double free or a wrong size.
    Overlap,
}

impl core::fmt::Display for FreeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Outside => "free of memory outside the heap",
            Self::Unaligned => "free of an unaligned pointer",
            Self::Overlap => "free of memory that is already free",
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtendError {
    /// The heap has no memory yet: call `init` first.
    NotInitialised,
    /// Not a multiple of the granule, or zero.
    BadLength,
    /// The new end would wrap the address space.
    Overflow,
}

/// What a free block holds at its start.
#[repr(C)]
struct Node {
    size: usize,
    next: Option<NonNull<Node>>,
}

const _: () = assert!(core::mem::size_of::<Node>() <= GRANULE);
const _: () = assert!(core::mem::align_of::<Node>() <= GRANULE);

pub struct Heap {
    start: usize,
    end: usize,
    head: Option<NonNull<Node>>,
    free: usize,
}

// SAFETY: the heap owns the bytes it manages and the list inside them; nothing
// in it is tied to the thread that made it. Sharing it needs a lock, which the
// owner provides (`&mut self` everywhere).
unsafe impl Send for Heap {}

fn align_up(value: usize, align: usize) -> Option<usize> {
    Some(value.checked_add(align - 1)? & !(align - 1))
}

impl Heap {
    pub const fn empty() -> Self {
        Self {
            start: 0,
            end: 0,
            head: None,
            free: 0,
        }
    }

    /// Gives the heap its first `length` bytes at `start`.
    ///
    /// # Safety
    /// `[start, start + length)` must be readable and writable memory that
    /// nothing else uses for as long as the heap lives, and not null.
    /// `start` and `length` must be multiples of [`GRANULE`], `length`
    /// not zero, and the heap must not have been initialised yet.
    pub unsafe fn init(&mut self, start: *mut u8, length: usize) -> Result<(), ExtendError> {
        assert!(self.end == 0, "heap initialised twice");
        let base = start as usize;
        if base == 0 || length == 0 || !(base | length).is_multiple_of(GRANULE) {
            return Err(ExtendError::BadLength);
        }
        let end = base.checked_add(length).ok_or(ExtendError::Overflow)?;
        self.start = base;
        self.end = base;
        // SAFETY: the caller promises the range, which `extend` needs as is.
        unsafe { self.extend(end - base) }
    }

    /// Adds `length` bytes right after the heap's current end.
    ///
    /// # Safety
    /// The `length` bytes after [`Heap::end`] must be readable and writable
    /// memory that nothing else uses for as long as the heap lives.
    pub unsafe fn extend(&mut self, length: usize) -> Result<(), ExtendError> {
        if self.end == 0 {
            return Err(ExtendError::NotInitialised);
        }
        if length == 0 || !length.is_multiple_of(GRANULE) {
            return Err(ExtendError::BadLength);
        }
        let new_end = self.end.checked_add(length).ok_or(ExtendError::Overflow)?;
        let at = self.end;
        self.end = new_end;
        // SAFETY: the new bytes are the caller's promise, and they are not
        // in the list, which only holds addresses below the old end.
        unsafe { self.insert(at, length) }.expect("the bytes after the end cannot be free");
        Ok(())
    }

    /// The address just past the heap: where `extend` adds.
    pub fn end(&self) -> usize {
        self.end
    }

    /// Bytes the heap manages.
    pub fn size(&self) -> usize {
        self.end - self.start
    }

    /// Bytes in free blocks.
    pub fn free_bytes(&self) -> usize {
        self.free
    }

    pub fn used_bytes(&self) -> usize {
        self.size() - self.free
    }

    /// How many separate free blocks there are: 1 when everything is free and
    /// adjacent blocks have been merged, more when memory is fragmented.
    pub fn free_blocks(&self) -> usize {
        let mut count = 0;
        let mut cursor = self.head;
        while let Some(node) = cursor {
            count += 1;
            // SAFETY: list nodes are inside the heap, which the owner of the
            // `init` and `extend` promises is valid, and only this heap
            // writes them.
            cursor = unsafe { node.as_ref().next };
        }
        count
    }

    /// The size a block for `layout` takes, or why there is none.
    fn block_size(layout: Layout) -> Result<usize, AllocError> {
        if layout.size() == 0 {
            return Err(AllocError::ZeroSize);
        }
        if layout.align() > MAX_ALIGN {
            return Err(AllocError::AlignmentTooLarge);
        }
        align_up(layout.size().max(GRANULE), GRANULE).ok_or(AllocError::SizeOverflow)
    }

    pub fn alloc(&mut self, layout: Layout) -> Result<NonNull<u8>, AllocError> {
        let size = Self::block_size(layout)?;
        let align = layout.align().max(GRANULE);
        let mut previous: Option<NonNull<Node>> = None;
        let mut cursor = self.head;
        while let Some(node) = cursor {
            let at = node.as_ptr() as usize;
            // SAFETY: `node` is in the list, so it is a valid header.
            let (block, next) = unsafe { ((*node.as_ptr()).size, (*node.as_ptr()).next) };
            if let Some(start) = align_up(at, align)
                && let Some(end) = start.checked_add(size)
                && end <= at + block
            {
                let front = start - at;
                let tail = at + block - end;
                // The block leaves the list; what is left of it goes back in
                // the same place, so the list stays in address order.
                let mut link = next;
                if tail > 0 {
                    // SAFETY: `end` is inside the block just taken out, on a
                    // granule, with `tail` bytes before the block's end.
                    link = Some(unsafe { Self::write_node(end, tail, next) });
                }
                if front > 0 {
                    // SAFETY: `at` is the block's own start, `front` bytes
                    // long, on a granule.
                    link = Some(unsafe { Self::write_node(at, front, link) });
                }
                self.set_next(previous, link);
                self.free -= size;
                // SAFETY: `start` is inside the heap, so not null.
                return Ok(unsafe { NonNull::new_unchecked(start as *mut u8) });
            }
            previous = Some(node);
            cursor = next;
        }
        // Aligning may waste up to `align - GRANULE` bytes in front. A free
        // block that ends the heap merges with what `extend` adds.
        let tail = previous.map_or(0, |node| {
            // SAFETY: a list node.
            let block = unsafe { node.as_ref().size };
            if node.as_ptr() as usize + block == self.end {
                block
            } else {
                0
            }
        });
        let needed = size
            .checked_add(align - GRANULE)
            .ok_or(AllocError::SizeOverflow)?
            - tail.min(size + align - GRANULE);
        Err(AllocError::OutOfMemory { needed })
    }

    /// Returns a block.
    ///
    /// # Safety
    /// If `pointer` came from [`Heap::alloc`] on this heap and is not free,
    /// `layout` must be the layout it was allocated with. Anything else is
    /// refused with an error where the heap can see it (outside, unaligned,
    /// overlapping a free block), but a wrong `layout` for a live block cannot
    /// be seen and corrupts the heap.
    pub unsafe fn dealloc(
        &mut self,
        pointer: NonNull<u8>,
        layout: Layout,
    ) -> Result<(), FreeError> {
        let size = Self::block_size(layout).map_err(|_| FreeError::Outside)?;
        let at = pointer.as_ptr() as usize;
        if !at.is_multiple_of(GRANULE) {
            return Err(FreeError::Unaligned);
        }
        let end = at.checked_add(size).ok_or(FreeError::Outside)?;
        if at < self.start || end > self.end {
            return Err(FreeError::Outside);
        }
        // SAFETY: the block is inside the heap and, by the caller's promise
        // about `layout`, is memory the heap handed out.
        unsafe { self.insert(at, size) }
    }

    /// Puts `[at, at + size)` in the list, merged with its neighbours.
    ///
    /// # Safety
    /// The range is inside the heap's memory, on granules, and not in use.
    unsafe fn insert(&mut self, at: usize, size: usize) -> Result<(), FreeError> {
        let mut previous: Option<NonNull<Node>> = None;
        let mut cursor = self.head;
        while let Some(node) = cursor {
            if node.as_ptr() as usize >= at {
                break;
            }
            previous = Some(node);
            // SAFETY: a list node.
            cursor = unsafe { node.as_ref().next };
        }
        let end = at + size;
        if let Some(previous) = previous {
            // SAFETY: a list node.
            let previous_end = previous.as_ptr() as usize + unsafe { previous.as_ref().size };
            if previous_end > at {
                return Err(FreeError::Overlap);
            }
        }
        if let Some(next) = cursor
            && (next.as_ptr() as usize) < end
        {
            return Err(FreeError::Overlap);
        }

        // Merge with the block right after, then with the one right before.
        let (mut length, mut after) = (size, cursor);
        if let Some(next) = cursor
            && next.as_ptr() as usize == end
        {
            // SAFETY: a list node.
            let following = unsafe { next.as_ref() };
            length += following.size;
            after = following.next;
        }
        if let Some(before) = previous {
            // SAFETY: a list node.
            let before_end = before.as_ptr() as usize + unsafe { before.as_ref().size };
            if before_end == at {
                // SAFETY: a list node, written only through `&mut self`.
                unsafe {
                    (*before.as_ptr()).size += length;
                    (*before.as_ptr()).next = after;
                }
                self.free += size;
                return Ok(());
            }
        }
        // SAFETY: `[at, at + length)` is inside the heap, not in use, and
        // not in the list any more (the block after it was merged in).
        let node = unsafe { Self::write_node(at, length, after) };
        self.set_next(previous, Some(node));
        self.free += size;
        Ok(())
    }

    /// Writes a header at `address` and returns it.
    ///
    /// # Safety
    /// `address` is on a granule, with `size` bytes of heap memory not in use
    /// (or being handed back) behind it.
    unsafe fn write_node(
        address: usize,
        size: usize,
        next: Option<NonNull<Node>>,
    ) -> NonNull<Node> {
        let node = address as *mut Node;
        // SAFETY: by the caller's promise the bytes are the heap's and the
        // address is aligned for `Node`.
        unsafe {
            node.write(Node { size, next });
            NonNull::new_unchecked(node)
        }
    }

    fn set_next(&mut self, previous: Option<NonNull<Node>>, link: Option<NonNull<Node>>) {
        match previous {
            // SAFETY: a list node, written only through `&mut self`.
            Some(node) => unsafe { (*node.as_ptr()).next = link },
            None => self.head = link,
        }
    }
}

#[cfg(test)]
mod tests;
