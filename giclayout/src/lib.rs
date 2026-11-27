//! How a GICv3's redistributors are laid out and found (ADR 0035).
//!
//! The kernel reads the registers; this crate holds the arithmetic around
//! them, so that it is tested on the host with a fake region: the packing of
//! `MPIDR_EL1` into the affinity a redistributor carries, and the walk over the
//! frames of a region for the one that belongs to a core.

#![no_std]

/// The size of the two pages of a redistributor without virtual LPIs: the
/// frame that holds its control registers and the one for SGIs and PPIs.
pub const FRAME: usize = 0x20000;
const TYPE: usize = 0x08;
const PERIPHERAL_ID_2: usize = 0xffe8;
const TYPE_VIRTUAL_LPIS: u64 = 1 << 1;
const TYPE_LAST: u64 = 1 << 4;

/// The affinity `GICR_TYPER` holds in bits 63:32, from `MPIDR_EL1`: Aff3 in bits
/// 31:24, then Aff2, Aff1 and Aff0 (the bits 23:0 of the register).
pub fn affinity(mpidr: u64) -> u32 {
    ((((mpidr >> 32) & 0xff) << 24) | (mpidr & 0x00ff_ffff)) as u32
}

/// Whether `peripheral_id_2` says the frame it was read from belongs to a GICv3
/// or a GICv4: a slot that reads as zero, or holds anything else, is not a
/// redistributor and must not be matched by its (zero) affinity.
fn is_redistributor(peripheral_id_2: u32) -> bool {
    matches!((peripheral_id_2 >> 4) & 0xf, 3 | 4)
}

/// The offset, inside a region of `region_bytes`, of the redistributor whose
/// affinity is `affinity`. The redistributors follow one another from the start
/// of the region, each `FRAME` long (twice that with virtual LPIs) until one
/// says it is the last. `read64` and `read32` read the region at an offset.
/// `None` when no frame matches, or one is not a redistributor at all.
pub fn find_redistributor(
    region_bytes: usize,
    affinity: u32,
    read64: impl Fn(usize) -> u64,
    read32: impl Fn(usize) -> u32,
) -> Option<usize> {
    let mut frame = 0usize;
    while frame.checked_add(FRAME)? <= region_bytes {
        if !is_redistributor(read32(frame + PERIPHERAL_ID_2)) {
            return None;
        }
        let typer = read64(frame + TYPE);
        if (typer >> 32) as u32 == affinity {
            return Some(frame);
        }
        if typer & TYPE_LAST != 0 {
            return None;
        }
        frame += if typer & TYPE_VIRTUAL_LPIS != 0 {
            2 * FRAME
        } else {
            FRAME
        };
    }
    None
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    /// One slot: its affinity, whether it is the last, whether it has virtual
    /// LPIs (and so is twice as long), and the architecture revision it reads.
    struct Slot {
        affinity: u32,
        last: bool,
        virtual_lpis: bool,
        architecture: u32,
    }

    fn slot(affinity: u32, last: bool, virtual_lpis: bool) -> Slot {
        Slot {
            affinity,
            last,
            virtual_lpis,
            architecture: 3,
        }
    }

    /// The region's reads, as a map from offset to value, with zero elsewhere
    /// (a slot nobody wrote reads as zero).
    fn region(
        slots: &[Slot],
    ) -> (
        usize,
        impl Fn(usize) -> u64 + use<>,
        impl Fn(usize) -> u32 + use<>,
    ) {
        let mut words: Vec<(usize, u64)> = Vec::new();
        let mut at = 0;
        for slot in slots {
            let typer = u64::from(slot.affinity) << 32
                | if slot.last { TYPE_LAST } else { 0 }
                | if slot.virtual_lpis {
                    TYPE_VIRTUAL_LPIS
                } else {
                    0
                };
            words.push((at + TYPE, typer));
            words.push((at + PERIPHERAL_ID_2, u64::from(slot.architecture << 4)));
            at += if slot.virtual_lpis { 2 * FRAME } else { FRAME };
        }
        let words64 = words.clone();
        (
            at,
            move |offset| {
                words64
                    .iter()
                    .find(|(o, _)| *o == offset)
                    .map_or(0, |(_, value)| *value)
            },
            move |offset| {
                words
                    .iter()
                    .find(|(o, _)| *o == offset)
                    .map_or(0, |(_, value)| *value as u32)
            },
        )
    }

    #[test]
    fn affinity_packs_aff3_above_the_other_three() {
        assert_eq!(affinity(0x0000_0000_8000_0003), 3);
        assert_eq!(affinity(0x0000_0000_8001_0203), 0x0001_0203);
        assert_eq!(affinity(0x0000_0001_8000_0000), 0x0100_0000);
        // Bits that are not an affinity field are dropped.
        assert_eq!(affinity(0xffff_ff00_c0ff_ffff), 0x00ff_ffff);
    }

    #[test]
    fn the_first_slot_is_found_at_offset_zero() {
        let (bytes, read64, read32) = region(&[slot(0, true, false)]);
        assert_eq!(find_redistributor(bytes, 0, read64, read32), Some(0));
    }

    #[test]
    fn a_later_slot_is_found_by_its_affinity() {
        let slots = [
            slot(0, false, false),
            slot(1, false, false),
            slot(0x0100_0002, true, false),
        ];
        let (bytes, read64, read32) = region(&slots);
        assert_eq!(find_redistributor(bytes, 1, &read64, &read32), Some(FRAME));
        assert_eq!(
            find_redistributor(bytes, 0x0100_0002, &read64, &read32),
            Some(2 * FRAME)
        );
    }

    #[test]
    fn slots_with_virtual_lpis_are_twice_as_long() {
        let slots = [
            slot(0, false, true),
            slot(1, false, false),
            slot(2, true, true),
        ];
        let (bytes, read64, read32) = region(&slots);
        assert_eq!(
            find_redistributor(bytes, 1, &read64, &read32),
            Some(2 * FRAME)
        );
        assert_eq!(
            find_redistributor(bytes, 2, &read64, &read32),
            Some(3 * FRAME)
        );
    }

    #[test]
    fn a_core_that_has_no_slot_is_not_found() {
        let slots = [slot(0, false, false), slot(1, true, false)];
        let (bytes, read64, read32) = region(&slots);
        assert_eq!(find_redistributor(bytes, 2, &read64, &read32), None);
    }

    #[test]
    fn the_walk_stops_at_the_end_of_the_region_without_a_last_slot() {
        let slots = [slot(0, false, false), slot(1, false, false)];
        let (bytes, read64, read32) = region(&slots);
        assert_eq!(find_redistributor(bytes, 7, &read64, &read32), None);
    }

    #[test]
    fn a_slot_that_is_not_a_redistributor_is_never_matched() {
        // Zero affinity and zero everywhere else: what a hole in the region reads.
        let hole = Slot {
            architecture: 0,
            ..slot(0, true, false)
        };
        let (bytes, read64, read32) = region(&[hole]);
        assert_eq!(find_redistributor(bytes, 0, read64, read32), None);
        let v2 = Slot {
            architecture: 2,
            ..slot(0, true, false)
        };
        let (bytes, read64, read32) = region(&[v2]);
        assert_eq!(find_redistributor(bytes, 0, read64, read32), None);
    }

    #[test]
    fn a_gicv4_slot_is_a_redistributor() {
        let v4 = Slot {
            architecture: 4,
            ..slot(5, true, false)
        };
        let (bytes, read64, read32) = region(&[v4]);
        assert_eq!(find_redistributor(bytes, 5, read64, read32), Some(0));
    }

    #[test]
    fn a_region_shorter_than_a_frame_holds_nothing() {
        assert_eq!(find_redistributor(FRAME - 1, 0, |_| 0, |_| 0), None);
        assert_eq!(find_redistributor(0, 0, |_| 0, |_| 0), None);
    }
}
