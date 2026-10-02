//! SM83 CPU. Every memory access goes through [`CpuBus`], which advances the
//! rest of the machine by one M-cycle per access, so instruction timing falls
//! out of the access pattern.

/// What the CPU sees of the machine. Implemented by [`crate::bus::Bus`];
/// tests may implement it with a flat-memory mock.
pub trait CpuBus {
    /// Advance one M-cycle, then read `addr`.
    fn read(&mut self, addr: u16) -> u8;
    /// Advance one M-cycle, then write `val` to `addr`.
    fn write(&mut self, addr: u16, val: u8);
    /// Advance one M-cycle without a memory access (internal delay).
    fn tick(&mut self);
    /// `IE & IF & 0x1F`: interrupts requested and enabled.
    fn pending_interrupts(&self) -> u8;
    /// Clear the given bit(s) in IF (interrupt acknowledged by dispatch).
    fn ack_interrupt(&mut self, mask: u8);
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Registers {
    pub a: u8,
    pub f: u8,
    pub b: u8,
    pub c: u8,
    pub d: u8,
    pub e: u8,
    pub h: u8,
    pub l: u8,
    pub sp: u16,
    pub pc: u16,
}

impl Registers {
    /// DMG (CPU ABC) register state after the boot ROM hands over at 0x0100.
    pub fn post_boot_dmg() -> Self {
        Registers { a: 0x01, f: 0xB0, b: 0x00, c: 0x13, d: 0x00, e: 0xD8, h: 0x01, l: 0x4D, sp: 0xFFFE, pc: 0x0100 }
    }
}

pub struct Cpu {
    pub regs: Registers,
    pub ime: bool,
    pub halted: bool,
}

impl Cpu {
    pub fn new() -> Self {
        Cpu { regs: Registers::post_boot_dmg(), ime: false, halted: false }
    }

    /// Execute one instruction (or one interrupt dispatch, or one idle
    /// M-cycle while halted). Returns the opcode executed, `None` when no
    /// instruction ran (halted idle cycle or interrupt dispatch). CB-prefixed
    /// instructions return `Some(0xCB)`.
    pub fn step(&mut self, bus: &mut impl CpuBus) -> Option<u8> {
        let _ = bus;
        unimplemented!("SM83 core not written yet")
    }
}

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}
