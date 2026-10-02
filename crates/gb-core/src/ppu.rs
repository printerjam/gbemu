//! PPU: VRAM, OAM, LCD registers (0xFF40-0xFF4B except 0xFF46), rendering.

use crate::{SCREEN_HEIGHT, SCREEN_WIDTH};

pub struct Ppu {
    vram: Box<[u8; 0x2000]>,
    oam: [u8; 0xA0],
    regs: [u8; 12],
    framebuffer: Vec<u32>,
    frame_ready: bool,
}

impl Ppu {
    pub fn new() -> Self {
        Ppu {
            vram: Box::new([0; 0x2000]),
            oam: [0; 0xA0],
            regs: [0; 12],
            framebuffer: vec![0x00FF_FFFF; SCREEN_WIDTH * SCREEN_HEIGHT],
            frame_ready: false,
        }
    }

    /// Advance `dots` T-cycles (4 per M-cycle on DMG). Returns IF bits to raise
    /// (`irq::VBLANK`, `irq::STAT`).
    pub fn tick(&mut self, dots: u32) -> u8 {
        let _ = dots;
        0
    }

    /// `addr` in 0x8000..=0x9FFF. CPU view (0xFF while blocked in mode 3).
    pub fn read_vram(&self, addr: u16) -> u8 {
        self.vram[(addr & 0x1FFF) as usize]
    }
    pub fn write_vram(&mut self, addr: u16, val: u8) {
        self.vram[(addr & 0x1FFF) as usize] = val;
    }
    /// `addr` in 0xFE00..=0xFE9F. CPU view (0xFF while blocked in modes 2/3).
    pub fn read_oam(&self, addr: u16) -> u8 {
        self.oam[(addr - 0xFE00) as usize]
    }
    pub fn write_oam(&mut self, addr: u16, val: u8) {
        self.oam[(addr - 0xFE00) as usize] = val;
    }
    /// OAM DMA write; bypasses mode-based CPU blocking.
    pub fn write_oam_dma(&mut self, index: u8, val: u8) {
        self.oam[index as usize] = val;
    }
    /// `addr` in 0xFF40..=0xFF4B (never 0xFF46; the bus owns DMA).
    pub fn read_reg(&self, addr: u16) -> u8 {
        self.regs[(addr - 0xFF40) as usize]
    }
    pub fn write_reg(&mut self, addr: u16, val: u8) {
        self.regs[(addr - 0xFF40) as usize] = val;
    }

    /// 160x144 pixels, row-major, 0x00RRGGBB. DMG shades are exactly
    /// 0xFFFFFF, 0xAAAAAA, 0x555555, 0x000000 (test-suite reference palette).
    pub fn framebuffer(&self) -> &[u32] {
        &self.framebuffer
    }

    /// True once per completed frame (entering VBlank); clears the flag.
    pub fn take_frame_ready(&mut self) -> bool {
        std::mem::take(&mut self.frame_ready)
    }
}

impl Default for Ppu {
    fn default() -> Self {
        Self::new()
    }
}
