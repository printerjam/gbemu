//! Cartridge: header parsing and memory bank controllers.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CartError {
    TooSmall,
    Unsupported(u8),
}

impl fmt::Display for CartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CartError::TooSmall => write!(f, "ROM smaller than a cartridge header"),
            CartError::Unsupported(t) => write!(f, "unsupported cartridge type 0x{t:02X}"),
        }
    }
}

impl std::error::Error for CartError {}

pub struct Cartridge {
    rom: Vec<u8>,
    ram: Vec<u8>,
    rom_bank: u8,
}

impl Cartridge {
    pub fn from_rom(rom: Vec<u8>) -> Result<Self, CartError> {
        if rom.len() < 0x150 {
            return Err(CartError::TooSmall);
        }
        Ok(Cartridge {
            rom,
            ram: vec![0; 0x2000],
            rom_bank: 1,
        })
    }

    pub fn title(&self) -> String {
        self.rom[0x134..0x144]
            .iter()
            .take_while(|&&b| b != 0)
            .map(|&b| b as char)
            .collect()
    }

    /// `addr` in 0x0000..=0x7FFF.
    pub fn read_rom(&self, addr: u16) -> u8 {
        let off = if addr < 0x4000 {
            addr as usize
        } else {
            self.rom_bank as usize * 0x4000 + (addr as usize - 0x4000)
        };
        self.rom.get(off % self.rom.len()).copied().unwrap_or(0xFF)
    }
    /// MBC register writes (`addr` in 0x0000..=0x7FFF).
    pub fn write_rom(&mut self, addr: u16, val: u8) {
        if (0x2000..0x4000).contains(&addr) {
            self.rom_bank = (val & 0x1F).max(1);
        }
    }
    /// `addr` in 0xA000..=0xBFFF.
    pub fn read_ram(&self, addr: u16) -> u8 {
        self.ram[(addr - 0xA000) as usize]
    }
    pub fn write_ram(&mut self, addr: u16, val: u8) {
        self.ram[(addr - 0xA000) as usize] = val;
    }
    /// Advance one M-cycle (MBC3 RTC).
    pub fn tick(&mut self) {}

    pub fn has_battery(&self) -> bool {
        false
    }
    /// Battery-backed state to persist (.sav contents).
    pub fn save_data(&self) -> Vec<u8> {
        self.ram.clone()
    }
    pub fn load_save_data(&mut self, data: &[u8]) {
        let n = data.len().min(self.ram.len());
        self.ram[..n].copy_from_slice(&data[..n]);
    }
}
