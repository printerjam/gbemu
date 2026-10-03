//! Serial port (SB 0xFF01, SC 0xFF02). No link partner: received bits are 1s.
//! Transmitted bytes are recorded so test ROMs (blargg) can report over serial.

use crate::irq;
use serde::{Deserialize, Serialize};

/// The internal serial clock (8192 Hz) is DIV-counter bit 8: one bit is shifted on each falling edge
/// (512 T-cycles), so a transfer started mid-period is aligned to the divider. In double speed the
/// divider runs twice as fast, which doubles the rate automatically.
const CLOCK_BIT: u16 = 0x100;
/// CGB fast clock (SC bit 1, 262144 Hz): DIV-counter bit 3.
const FAST_CLOCK_BIT: u16 = 0x008;

#[derive(Serialize, Deserialize)]
pub struct Serial {
    sb: u8,
    sc: u8,
    bits_left: u8,
    cgb: bool,
    output: Vec<u8>,
}

impl Serial {
    pub fn new() -> Self {
        Serial {
            sb: 0,
            sc: 0x7E,
            bits_left: 0,
            cgb: false,
            output: Vec::new(),
        }
    }

    pub fn set_cgb(&mut self, cgb: bool) {
        self.cgb = cgb;
        self.sc = if cgb { 0x7C } else { 0x7E };
    }

    /// The divider counter changed from `div_before` to `div_after` (one M-cycle, or a DIV reset).
    /// Returns IF bits to raise.
    pub fn clock(&mut self, div_before: u16, div_after: u16) -> u8 {
        let bit = if self.cgb && self.sc & 2 != 0 {
            FAST_CLOCK_BIT
        } else {
            CLOCK_BIT
        };
        if self.bits_left == 0 || div_before & bit == 0 || div_after & bit != 0 {
            return 0;
        }
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
            _ => self.sc | if self.cgb { 0x7C } else { 0x7E },
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
