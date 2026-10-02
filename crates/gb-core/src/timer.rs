//! DIV/TIMA/TMA/TAC (0xFF04-0xFF07).

pub struct Timer {
    /// Internal 16-bit divider; DIV (0xFF04) is the upper byte.
    div: u16,
}

impl Timer {
    pub fn new() -> Self {
        Timer { div: 0xABCC }
    }

    /// Advance one M-cycle (4 T-cycles). Returns IF bits to raise.
    pub fn tick(&mut self) -> u8 {
        self.div = self.div.wrapping_add(4);
        0
    }

    /// Internal divider value (used by the bus for the APU frame sequencer).
    pub fn div_counter(&self) -> u16 {
        self.div
    }

    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            0xFF04 => (self.div >> 8) as u8,
            _ => 0xFF,
        }
    }

    pub fn write(&mut self, addr: u16, val: u8) {
        let _ = (addr, val);
    }
}

impl Default for Timer {
    fn default() -> Self {
        Self::new()
    }
}
