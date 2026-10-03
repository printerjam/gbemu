//! DIV/TIMA/TMA/TAC (0xFF04-0xFF07).
//!
//! TIMA counts falling edges of `(div bit selected by TAC) & TAC.enable`, so
//! writes to DIV/TAC can cause spurious increments. An overflow leaves TIMA at
//! 0 for one M-cycle, then reloads TMA and raises the interrupt.

use crate::irq;
use serde::{Deserialize, Serialize};

/// Empirical (gambatte tima/tc00_late_tc01*): a TAC write checks the newly selected clock bit
/// against the divider one M-cycle earlier than the tick-then-write model implies.
const NEW_CLOCK_LEAD: u16 = 4;

#[derive(Serialize, Deserialize)]
pub struct Timer {
    /// Internal 16-bit divider; DIV (0xFF04) is the upper byte.
    div: u16,
    tima: u8,
    tma: u8,
    tac: u8,
    /// TIMA overflowed on the last tick; reload happens on the next one.
    overflow_pending: bool,
    /// The reload happened on the last tick (TIMA writes ignored, TMA writes propagate).
    reloading: bool,
    /// IF bits raised by a register write, collected by the bus right after the write.
    write_irq: u8,
}

impl Timer {
    pub fn new() -> Self {
        Self::with_div(0xABC8)
    }

    /// Timer whose internal divider starts at `div` (post-boot phase differs per model).
    pub fn with_div(div: u16) -> Self {
        Timer {
            div,
            tima: 0,
            tma: 0,
            tac: 0,
            overflow_pending: false,
            reloading: false,
            write_irq: 0,
        }
    }

    fn signal(&self) -> bool {
        self.signal_at(self.div)
    }

    fn signal_at(&self, div: u16) -> bool {
        let bit = match self.tac & 3 {
            0 => 9,
            1 => 3,
            2 => 5,
            _ => 7,
        };
        self.tac & 4 != 0 && div & (1 << bit) != 0
    }

    fn increment(&mut self) {
        let (v, overflow) = self.tima.overflowing_add(1);
        self.tima = v;
        if overflow {
            self.overflow_pending = true;
        }
    }

    /// Increment caused by a DIV/TAC write (falling-edge glitch). An overflow here reloads TIMA and
    /// raises the interrupt right away instead of one M-cycle later.
    fn increment_from_write(&mut self) {
        self.increment();
        if self.overflow_pending {
            self.overflow_pending = false;
            self.reloading = true;
            self.tima = self.tma;
            self.write_irq |= irq::TIMER;
        }
    }

    /// IF bits raised by the last register write.
    pub fn take_write_irq(&mut self) -> u8 {
        std::mem::take(&mut self.write_irq)
    }

    /// Advance one M-cycle (4 T-cycles). Returns IF bits to raise.
    pub fn tick(&mut self) -> u8 {
        let mut irqs = 0;
        self.reloading = false;
        if self.overflow_pending {
            self.overflow_pending = false;
            self.reloading = true;
            self.tima = self.tma;
            irqs = irq::TIMER;
        }
        let before = self.signal();
        self.div = self.div.wrapping_add(4);
        if before && !self.signal() {
            self.increment();
        }
        irqs
    }

    /// Internal divider value (used by the bus for the APU frame sequencer).
    pub fn div_counter(&self) -> u16 {
        self.div
    }

    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            0xFF04 => (self.div >> 8) as u8,
            0xFF05 => self.tima,
            0xFF06 => self.tma,
            _ => 0xF8 | self.tac,
        }
    }

    pub fn write(&mut self, addr: u16, val: u8) {
        match addr {
            0xFF04 => {
                let before = self.signal();
                self.div = 0;
                if before {
                    self.increment_from_write();
                }
            }
            0xFF05 => {
                if !self.reloading {
                    self.overflow_pending = false;
                    self.tima = val;
                }
            }
            0xFF06 => {
                self.tma = val;
                if self.reloading {
                    self.tima = val;
                }
            }
            _ => {
                let before = self.signal();
                self.tac = val & 7;
                if before && !self.signal_at(self.div.wrapping_sub(NEW_CLOCK_LEAD)) {
                    self.increment_from_write();
                }
            }
        }
    }
}

impl Default for Timer {
    fn default() -> Self {
        Self::new()
    }
}
