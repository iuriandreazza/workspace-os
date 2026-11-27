pub mod gicv2;
pub mod gicv3;
pub mod pl011;

/// IDs 1020 to 1023 are not interrupts: they say nothing is pending for this
/// core (1023) or that what is pending is not for this world. Nothing is ended.
const FIRST_SPECIAL_ID: u32 = 1020;
const FIRST_LPI_ID: u32 = 8192;

/// What an acknowledge of an interrupt controller returned. The whole value
/// goes back to end the interrupt: for a GICv2 SGI it also holds the core that
/// sent it.
#[derive(Clone, Copy)]
pub struct Acknowledged {
    raw: u32,
    id: u32,
}

impl Acknowledged {
    /// Reads `raw`, as an acknowledge register returned it, with `id_mask`
    /// covering the bits that hold the ID: `None` for a special ID.
    pub fn from_register(raw: u32, id_mask: u32) -> Option<Self> {
        let id = raw & id_mask;
        (!(FIRST_SPECIAL_ID..FIRST_LPI_ID).contains(&id)).then_some(Self { raw, id })
    }

    pub fn id(self) -> u32 {
        self.id
    }

    pub fn raw(self) -> u32 {
        self.raw
    }
}
