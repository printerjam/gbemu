//! Top-level machine: CPU + bus, and the API frontends use.

use crate::bus::Bus;
use crate::cartridge::{CartError, Cartridge};
use crate::cpu::{Cpu, Registers};
use crate::joypad::Button;
use crate::CYCLES_PER_FRAME;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct GameBoy {
    pub cpu: Cpu,
    pub bus: Bus,
}

impl GameBoy {
    /// Power on with `rom` inserted, skipping the boot ROM (post-boot DMG state).
    pub fn new(rom: Vec<u8>) -> Result<Self, CartError> {
        let cart = Cartridge::from_rom(rom)?;
        Ok(GameBoy {
            cpu: Cpu::new(),
            bus: Bus::new(cart),
        })
    }

    /// Execute one CPU step. See [`Cpu::step`] for the return value.
    pub fn step(&mut self) -> Option<u8> {
        self.cpu.step(&mut self.bus)
    }

    /// Run until the PPU completes a frame, or one frame's worth of cycles
    /// passes (LCD off).
    pub fn run_frame(&mut self) {
        let end = self.bus.cycles + CYCLES_PER_FRAME as u64;
        while self.bus.cycles < end {
            self.step();
            if self.bus.ppu.take_frame_ready() {
                break;
            }
        }
    }

    /// 160x144 pixels, 0x00RRGGBB.
    pub fn framebuffer(&self) -> &[u32] {
        self.bus.ppu.framebuffer()
    }

    pub fn set_button(&mut self, button: Button, pressed: bool) {
        self.bus.int_flag |= self.bus.joypad.set_button(button, pressed);
    }

    pub fn registers(&self) -> &Registers {
        &self.cpu.regs
    }

    /// T-cycles since power-on.
    pub fn cycles(&self) -> u64 {
        self.bus.cycles
    }

    /// Bytes sent over the serial port so far.
    pub fn serial_output(&self) -> &[u8] {
        self.bus.serial.output()
    }

    pub fn set_sample_rate(&mut self, hz: u32) {
        self.bus.apu.set_sample_rate(hz);
    }

    /// Append generated audio (stereo interleaved f32) to `out`.
    pub fn drain_audio(&mut self, out: &mut Vec<f32>) {
        self.bus.apu.drain_samples(out);
    }

    pub fn cartridge(&self) -> &Cartridge {
        &self.bus.cart
    }

    pub fn cartridge_mut(&mut self) -> &mut Cartridge {
        &mut self.bus.cart
    }
}
