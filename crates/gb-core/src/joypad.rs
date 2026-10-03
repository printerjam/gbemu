//! Joypad (P1, 0xFF00).

use crate::irq;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Button {
    Right,
    Left,
    Up,
    Down,
    A,
    B,
    Select,
    Start,
}

pub struct Joypad {
    /// P1 bits 4-5 (0 = group selected).
    select: u8,
    /// Pressed buttons, 1 = pressed. Low nibble d-pad, high nibble actions.
    pressed: u8,
}

impl Joypad {
    pub fn new() -> Self {
        Joypad {
            select: 0x00,
            pressed: 0,
        }
    }

    pub fn read(&self) -> u8 {
        0xC0 | self.select | (!self.lines() & 0x0F)
    }

    pub fn write(&mut self, val: u8) {
        self.select = val & 0x30;
    }

    /// Update a button. Returns IF bits to raise (a selected line going low).
    pub fn set_button(&mut self, button: Button, pressed: bool) -> u8 {
        let before = self.lines();
        let bit = match button {
            Button::Right => 0x01,
            Button::Left => 0x02,
            Button::Up => 0x04,
            Button::Down => 0x08,
            Button::A => 0x10,
            Button::B => 0x20,
            Button::Select => 0x40,
            Button::Start => 0x80,
        };
        if pressed {
            self.pressed |= bit;
        } else {
            self.pressed &= !bit;
        }
        if self.lines() & !before != 0 {
            irq::JOYPAD
        } else {
            0
        }
    }

    /// Active-high input lines P10-P13 as seen through the current selection.
    fn lines(&self) -> u8 {
        let mut l = 0;
        if self.select & 0x10 == 0 {
            l |= self.pressed & 0x0F;
        }
        if self.select & 0x20 == 0 {
            l |= self.pressed >> 4;
        }
        l
    }
}

impl Default for Joypad {
    fn default() -> Self {
        Self::new()
    }
}
