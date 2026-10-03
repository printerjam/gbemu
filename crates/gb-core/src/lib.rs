//! Game Boy (DMG) emulation core. Pure logic: no I/O, no external dependencies,
//! so it builds for native targets and wasm32 alike.

pub mod apu;
pub mod bus;
pub mod cartridge;
pub mod cheats;
pub mod cpu;
pub mod disasm;
pub mod gameboy;
pub mod joypad;
pub mod oam_bug;
pub mod ppu;
pub mod rewind;
pub mod serial;
pub mod state;
pub mod timer;

pub use cartridge::{CartError, Cartridge};
pub use cheats::{Cheat, CheatError};
pub use gameboy::GameBoy;
pub use joypad::Button;
pub use rewind::Rewind;
pub use state::StateError;

/// Hardware model being emulated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    Dmg,
    Cgb,
}

impl Model {
    /// Parse a `--model` argument: `auto` (follow the cartridge header) gives `None`.
    pub fn parse_choice(s: &str) -> Result<Option<Model>, String> {
        match s {
            "auto" => Ok(None),
            "dmg" => Ok(Some(Model::Dmg)),
            "cgb" => Ok(Some(Model::Cgb)),
            _ => Err(format!("unknown model '{s}' (expected dmg, cgb or auto)")),
        }
    }

    /// Model a cartridge asks for: CGB when header byte 0x143 has bit 7 set, else DMG.
    pub fn for_rom(rom: &[u8]) -> Model {
        if rom.get(0x143).is_some_and(|f| f & 0x80 != 0) {
            Model::Cgb
        } else {
            Model::Dmg
        }
    }
}

pub const SCREEN_WIDTH: usize = 160;
pub const SCREEN_HEIGHT: usize = 144;
/// T-cycles (dots) per second on DMG.
pub const CLOCK_HZ: u32 = 4_194_304;
/// T-cycles per video frame (154 lines * 456 dots).
pub const CYCLES_PER_FRAME: u32 = 70_224;

/// Interrupt bits as laid out in IF (0xFF0F) / IE (0xFFFF).
pub mod irq {
    pub const VBLANK: u8 = 0x01;
    pub const STAT: u8 = 0x02;
    pub const TIMER: u8 = 0x04;
    pub const SERIAL: u8 = 0x08;
    pub const JOYPAD: u8 = 0x10;
}
