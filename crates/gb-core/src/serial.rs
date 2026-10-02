//! Serial port (SB 0xFF01, SC 0xFF02). No link partner: received bits are 1s.
//! Transmitted bytes are recorded so test ROMs (blargg) can report over serial.

use crate::irq;

/// Internal clock: 8192 Hz bit rate = 128 M-cycles per bit.
const M_CYCLES_PER_BIT: u16 = 128;

pub struct Serial {
    sb: u8,
    sc: u8,
    bits_left: u8,
    counter: u16,
    output: Vec<u8>,
}

impl Serial {
    pub fn new() -> Self {
        Serial { sb: 0, sc: 0x7E, bits_left: 0, counter: 0, output: Vec::new() }
    }

    /// Advance one M-cycle. Returns IF bits to raise.
    pub fn tick(&mut self) -> u8 {
        if self.bits_left == 0 {
            return 0;
        }
        self.counter += 1;
        if self.counter < M_CYCLES_PER_BIT {
            return 0;
        }
        self.counter = 0;
        self.sb = (self.sb << 1) | 1;
        self.bits_left -= 1;
        if self.bits_left == 0 {
            self.sc &= 0x7F;
            return irq::SERIAL;
        }
        0
    }

    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            0xFF01 => self.sb,
            _ => self.sc | 0x7E,
        }
    }

    pub fn write(&mut self, addr: u16, val: u8) {
        match addr {
            0xFF01 => self.sb = val,
            _ => {
                self.sc = val;
                // Only internally clocked transfers progress without a partner.
                if val & 0x81 == 0x81 {
                    self.output.push(self.sb);
                    self.bits_left = 8;
                    self.counter = 0;
                }
            }
        }
    }

    /// Every byte transmitted so far.
    pub fn output(&self) -> &[u8] {
        &self.output
    }
}

impl Default for Serial {
    fn default() -> Self {
        Self::new()
    }
}
