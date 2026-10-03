//! Top-level machine: CPU + bus, and the API frontends use.

use crate::bus::Bus;
use crate::cartridge::{CartError, Cartridge};
use crate::cpu::{Cpu, Registers};
use crate::joypad::Button;
use crate::{Model, CYCLES_PER_FRAME};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct GameBoy {
    pub cpu: Cpu,
    pub bus: Bus,
}

impl GameBoy {
    /// Power on with `rom` inserted, skipping the boot ROM. Picks CGB when the cartridge
    /// header advertises CGB support (0x143 bit 7), DMG otherwise.
    pub fn new(rom: Vec<u8>) -> Result<Self, CartError> {
        let model = Model::for_rom(&rom);
        Self::with_model(rom, model)
    }

    /// Power on as `model`, or as the cartridge asks when `None` (see [`GameBoy::new`]).
    pub fn with_model_choice(rom: Vec<u8>, model: Option<Model>) -> Result<Self, CartError> {
        match model {
            Some(m) => Self::with_model(rom, m),
            None => Self::new(rom),
        }
    }

    /// Power on as `model`. A DMG-only cartridge on `Model::Cgb` runs in CGB
    /// compatibility mode.
    pub fn with_model(rom: Vec<u8>, model: Model) -> Result<Self, CartError> {
        let cart = Cartridge::from_rom(rom)?;
        let (cpu, bus) = match model {
            Model::Dmg => (Cpu::new(), Bus::new(cart)),
            Model::Cgb => {
                let cgb_game = cart.header().cgb_supported();
                (
                    Cpu::with_registers(Registers::post_boot_cgb(cgb_game)),
                    Bus::new_cgb(cart),
                )
            }
        };
        Ok(GameBoy { cpu, bus })
    }

    pub fn model(&self) -> Model {
        self.bus.model()
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
