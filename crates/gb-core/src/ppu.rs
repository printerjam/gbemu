//! PPU: VRAM, OAM, LCD registers (0xFF40-0xFF4B except 0xFF46), rendering.
//!
//! Dot-based state machine (456 dots per line, 154 lines). Each line is
//! rendered in one go when mode 3 ends (scanline renderer); mode 3 length is
//! computed from SCX, window start and sprite fetch penalties.

use crate::irq;
use crate::{SCREEN_HEIGHT, SCREEN_WIDTH};

const DOTS_PER_LINE: u16 = 456;
const LINES: u8 = 154;
/// Dots at the start of a line during which the visible mode and LYC flag
/// still reflect the previous line.
const LINE_PREFIX: u16 = 4;
const MODE3_START: u16 = 80;
const MODE3_BASE_LEN: u16 = 172;

const LCDC: usize = 0;
const STAT: usize = 1;
const SCY: usize = 2;
const SCX: usize = 3;
const LYC: usize = 5;
const BGP: usize = 7;
const OBP0: usize = 8;
const OBP1: usize = 9;
const WY: usize = 10;
const WX: usize = 11;

pub const DEFAULT_PALETTE: [u32; 4] = [0x00FF_FFFF, 0x00AA_AAAA, 0x0055_5555, 0x0000_0000];

#[derive(Clone, Copy, Default)]
struct Sprite {
    y: u8,
    x: u8,
    tile: u8,
    flags: u8,
}

pub struct Ppu {
    vram: Box<[u8; 0x2000]>,
    oam: [u8; 0xA0],
    regs: [u8; 12],
    framebuffer: Vec<u32>,
    frame_ready: bool,
    palette: [u32; 4],

    /// Current line (raw; LY register applies the line-153 quirk).
    ly: u8,
    /// Dot within the line, 0..456.
    dot: u16,
    /// Mode visible in STAT bits 0-1.
    mode: u8,
    /// Mode as seen by the STAT interrupt logic (leads `mode` at line start).
    irq_mode: u8,
    lyc_flag: bool,
    stat_line: bool,
    pending_irq: u8,
    /// Dot at which mode 3 ends on the current line.
    mode0_dot: u16,
    sprites: [Sprite; 10],
    sprite_count: usize,
    wy_triggered: bool,
    window_line: u8,
    /// Dots elapsed while the LCD is off (to keep producing blank frames).
    off_dots: u32,
}

impl Ppu {
    pub fn new() -> Self {
        let mut regs = [0u8; 12];
        regs[LCDC] = 0x91;
        regs[BGP] = 0xFC;
        regs[OBP0] = 0xFF;
        regs[OBP1] = 0xFF;
        let mut ppu = Ppu {
            vram: Box::new([0; 0x2000]),
            oam: [0; 0xA0],
            regs,
            framebuffer: vec![DEFAULT_PALETTE[0]; SCREEN_WIDTH * SCREEN_HEIGHT],
            frame_ready: false,
            palette: DEFAULT_PALETTE,
            // DMG-ABC post-boot: STAT reads 0x85 (mode 1, LY reads 0 via the
            // line 153 quirk, LYC=0 matches).
            ly: 153,
            dot: 400,
            mode: 1,
            irq_mode: 1,
            lyc_flag: true,
            stat_line: false,
            pending_irq: 0,
            mode0_dot: 0,
            sprites: [Sprite::default(); 10],
            sprite_count: 0,
            wy_triggered: false,
            window_line: 0,
            off_dots: 0,
        };
        ppu.update_stat_line();
        ppu.pending_irq = 0;
        ppu
    }

    /// Replace the four output shades (index = DMG color 0..3). Default is
    /// exact grayscale; affects only lines rendered afterwards.
    pub fn set_palette(&mut self, palette: [u32; 4]) {
        self.palette = palette;
    }

    fn lcd_on(&self) -> bool {
        self.regs[LCDC] & 0x80 != 0
    }

    fn ly_reg(&self) -> u8 {
        if self.ly == 153 && self.dot >= LINE_PREFIX {
            0
        } else {
            self.ly
        }
    }

    /// Value LYC is compared against right now, if any.
    fn compare_ly(&self) -> Option<u8> {
        if self.ly == 153 {
            Some(if self.dot < LINE_PREFIX { 153 } else { 0 })
        } else if self.dot < LINE_PREFIX && self.ly != 0 {
            None
        } else {
            Some(self.ly)
        }
    }

    fn stat_conditions(&self, sel: u8) -> bool {
        (self.lyc_flag && sel & 0x40 != 0)
            || (self.irq_mode == 0 && sel & 0x08 != 0)
            || (self.irq_mode == 1 && sel & 0x10 != 0)
            || (self.irq_mode == 2 && sel & 0x20 != 0)
    }

    /// Recompute LYC flag and the OR'ed STAT line; raise IRQ on rising edge.
    fn update_stat_line(&mut self) {
        if !self.lcd_on() {
            self.lyc_flag = false;
            self.stat_line = false;
            return;
        }
        self.lyc_flag = self.compare_ly() == Some(self.regs[LYC]);
        let line = self.stat_conditions(self.regs[STAT]);
        if line && !self.stat_line {
            self.pending_irq |= irq::STAT;
        }
        self.stat_line = line;
    }

    /// Advance `dots` T-cycles (4 per M-cycle on DMG). Returns IF bits to raise
    /// (`irq::VBLANK`, `irq::STAT`).
    pub fn tick(&mut self, dots: u32) -> u8 {
        if !self.lcd_on() {
            self.off_dots += dots;
            if self.off_dots >= crate::CYCLES_PER_FRAME {
                self.off_dots -= crate::CYCLES_PER_FRAME;
                self.frame_ready = true;
            }
        } else {
            for _ in 0..dots {
                self.step_dot();
            }
        }
        std::mem::take(&mut self.pending_irq)
    }

    fn step_dot(&mut self) {
        self.dot += 1;
        if self.dot == DOTS_PER_LINE {
            self.dot = 0;
            self.ly = if self.ly == LINES - 1 { 0 } else { self.ly + 1 };
            if self.ly == 0 {
                self.wy_triggered = false;
                self.window_line = 0;
            }
            if self.ly <= 144 {
                self.irq_mode = 2;
            }
        } else if self.dot == LINE_PREFIX {
            if self.ly < 144 {
                self.mode = 2;
            } else if self.ly == 144 {
                self.mode = 1;
                self.irq_mode = 1;
                self.pending_irq |= irq::VBLANK;
                self.frame_ready = true;
            }
        } else if self.ly < 144 {
            if self.dot == MODE3_START {
                self.start_mode3();
            } else if self.dot == self.mode0_dot {
                self.render_line();
                self.mode = 0;
                self.irq_mode = 0;
            }
        }
        self.update_stat_line();
    }

    fn sprite_height(&self) -> u8 {
        if self.regs[LCDC] & 0x04 != 0 {
            16
        } else {
            8
        }
    }

    fn window_starts_here(&self) -> bool {
        self.regs[LCDC] & 0x21 == 0x21 && self.wy_triggered && self.regs[WX] <= 166
    }

    /// OAM scan result, mode 3 length.
    fn start_mode3(&mut self) {
        self.mode = 3;
        self.irq_mode = 3;
        if self.regs[LCDC] & 0x20 != 0 && self.ly == self.regs[WY] {
            self.wy_triggered = true;
        }
        // OAM scan.
        let h = self.sprite_height() as i16;
        let mut n = 0;
        for i in 0..40 {
            let o = i * 4;
            let row = self.ly as i16 + 16 - self.oam[o] as i16;
            if (0..h).contains(&row) {
                self.sprites[n] = Sprite {
                    y: self.oam[o],
                    x: self.oam[o + 1],
                    tile: self.oam[o + 2],
                    flags: self.oam[o + 3],
                };
                n += 1;
                if n == 10 {
                    break;
                }
            }
        }
        self.sprite_count = n;
        // DMG priority: lower X first, then OAM order (stable sort).
        self.sprites[..n].sort_by_key(|s| s.x);

        let scx = self.regs[SCX] as u16;
        let mut len = MODE3_BASE_LEN + (scx & 7);
        if self.window_starts_here() {
            len += 6;
        }
        if self.regs[LCDC] & 0x02 != 0 {
            let mut last_tile = u16::MAX;
            for s in &self.sprites[..n] {
                if s.x >= 168 {
                    continue;
                }
                let pos = s.x as u16 + scx;
                let tile = pos / 8;
                if tile != last_tile {
                    last_tile = tile;
                    len += 5 - (pos % 8).min(5);
                }
                len += 6;
            }
        }
        self.mode0_dot = MODE3_START + len;
    }

    #[inline]
    fn tile_row(&self, tile: u8, row: u8, sprite: bool) -> (u8, u8) {
        let base = if sprite || self.regs[LCDC] & 0x10 != 0 {
            tile as usize * 16
        } else {
            (0x1000 + (tile as i8 as i32) * 16) as usize
        };
        let a = base + row as usize * 2;
        (self.vram[a], self.vram[a + 1])
    }

    fn render_line(&mut self) {
        let ly = self.ly;
        let lcdc = self.regs[LCDC];
        let mut bg = [0u8; SCREEN_WIDTH]; // color ids
        if lcdc & 0x01 != 0 {
            let scx = self.regs[SCX] as usize;
            let y = ly.wrapping_add(self.regs[SCY]) as usize;
            let map = if lcdc & 0x08 != 0 { 0x1C00 } else { 0x1800 } + (y / 8) * 32;
            let mut cache = (usize::MAX, 0u8, 0u8);
            for (x, out) in bg.iter_mut().enumerate() {
                let px = (scx + x) & 0xFF;
                let col = px / 8;
                if cache.0 != col {
                    let (lo, hi) = self.tile_row(self.vram[map + col], (y & 7) as u8, false);
                    cache = (col, lo, hi);
                }
                let bit = 7 - (px & 7);
                *out = ((cache.1 >> bit) & 1) | (((cache.2 >> bit) & 1) << 1);
            }
            if self.window_starts_here() {
                let wx = self.regs[WX] as i32 - 7;
                let map = if lcdc & 0x40 != 0 { 0x1C00 } else { 0x1800 } + (self.window_line as usize / 8) * 32;
                let row = self.window_line & 7;
                let mut cache = (usize::MAX, 0u8, 0u8);
                for x in wx.max(0) as usize..SCREEN_WIDTH {
                    let px = (x as i32 - wx) as usize;
                    let col = px / 8;
                    if cache.0 != col {
                        let (lo, hi) = self.tile_row(self.vram[map + col], row, false);
                        cache = (col, lo, hi);
                    }
                    let bit = 7 - (px & 7);
                    bg[x] = ((cache.1 >> bit) & 1) | (((cache.2 >> bit) & 1) << 1);
                }
                self.window_line += 1;
            }
        }

        let bgp = self.regs[BGP];
        let row_out = ly as usize * SCREEN_WIDTH;
        for x in 0..SCREEN_WIDTH {
            self.framebuffer[row_out + x] = self.palette[((bgp >> (bg[x] * 2)) & 3) as usize];
        }

        if lcdc & 0x02 != 0 {
            let h = self.sprite_height();
            // Draw lowest priority first so higher priority overwrites.
            let mut buf = [0u8; SCREEN_WIDTH]; // bit7 = present, bit6 = bg priority, bit5 = palette, 0..1 = color
            for s in self.sprites[..self.sprite_count].iter().rev() {
                let mut row = (ly as i16 + 16 - s.y as i16) as u8;
                if s.flags & 0x40 != 0 {
                    row = h - 1 - row;
                }
                let tile = if h == 16 { s.tile & 0xFE } else { s.tile };
                let (lo, hi) = self.tile_row(tile, row, true);
                for i in 0..8u8 {
                    let sx = s.x as i16 - 8 + i as i16;
                    if !(0..SCREEN_WIDTH as i16).contains(&sx) {
                        continue;
                    }
                    let bit = if s.flags & 0x20 != 0 { i } else { 7 - i };
                    let c = ((lo >> bit) & 1) | (((hi >> bit) & 1) << 1);
                    if c != 0 {
                        buf[sx as usize] = 0x80 | (s.flags & 0x80) >> 1 | (s.flags & 0x10) << 1 | c;
                    }
                }
            }
            for x in 0..SCREEN_WIDTH {
                let p = buf[x];
                if p & 0x80 == 0 || (p & 0x40 != 0 && bg[x] != 0) {
                    continue;
                }
                let pal = self.regs[if p & 0x20 != 0 { OBP1 } else { OBP0 }];
                self.framebuffer[row_out + x] = self.palette[((pal >> ((p & 3) * 2)) & 3) as usize];
            }
        }
    }

    fn vram_blocked(&self) -> bool {
        self.lcd_on() && self.mode == 3
    }
    fn oam_blocked(&self) -> bool {
        self.lcd_on() && self.mode >= 2
    }

    /// `addr` in 0x8000..=0x9FFF. CPU view (0xFF while blocked in mode 3).
    pub fn read_vram(&self, addr: u16) -> u8 {
        if self.vram_blocked() {
            0xFF
        } else {
            self.vram[(addr & 0x1FFF) as usize]
        }
    }
    pub fn write_vram(&mut self, addr: u16, val: u8) {
        if !self.vram_blocked() {
            self.vram[(addr & 0x1FFF) as usize] = val;
        }
    }
    /// `addr` in 0xFE00..=0xFE9F. CPU view (0xFF while blocked in modes 2/3).
    pub fn read_oam(&self, addr: u16) -> u8 {
        if self.oam_blocked() {
            0xFF
        } else {
            self.oam[(addr - 0xFE00) as usize]
        }
    }
    pub fn write_oam(&mut self, addr: u16, val: u8) {
        if !self.oam_blocked() {
            self.oam[(addr - 0xFE00) as usize] = val;
        }
    }
    /// OAM DMA write; bypasses mode-based CPU blocking.
    pub fn write_oam_dma(&mut self, index: u8, val: u8) {
        self.oam[index as usize] = val;
    }
    /// `addr` in 0xFF40..=0xFF4B (never 0xFF46; the bus owns DMA).
    pub fn read_reg(&self, addr: u16) -> u8 {
        let i = (addr - 0xFF40) as usize;
        match i {
            STAT => {
                let mode = if self.lcd_on() { self.mode } else { 0 };
                0x80 | (self.regs[STAT] & 0x78) | ((self.lyc_flag as u8) << 2) | mode
            }
            4 => {
                if self.lcd_on() {
                    self.ly_reg()
                } else {
                    0
                }
            }
            _ => self.regs[i],
        }
    }
    pub fn write_reg(&mut self, addr: u16, val: u8) {
        let i = (addr - 0xFF40) as usize;
        match i {
            LCDC => {
                let was_on = self.lcd_on();
                self.regs[LCDC] = val;
                let on = self.lcd_on();
                if was_on && !on {
                    self.ly = 0;
                    self.dot = 0;
                    self.mode = 0;
                    self.irq_mode = 0;
                    self.off_dots = 0;
                    self.wy_triggered = false;
                    self.window_line = 0;
                    self.framebuffer.fill(self.palette[0]);
                    self.update_stat_line();
                } else if !was_on && on {
                    // Line 0 after LCD-on has no OAM scan: mode 0 until mode 3.
                    self.ly = 0;
                    self.dot = LINE_PREFIX;
                    self.mode = 0;
                    self.irq_mode = 0;
                    self.update_stat_line();
                }
            }
            STAT => {
                if self.lcd_on() {
                    // DMG quirk: during the write all STAT sources read as
                    // enabled for one cycle.
                    let blip = self.stat_conditions(0x78);
                    if blip && !self.stat_line {
                        self.pending_irq |= irq::STAT;
                    }
                    self.stat_line = blip;
                }
                self.regs[STAT] = val & 0x78;
                self.update_stat_line();
            }
            4 => {}
            LYC => {
                self.regs[LYC] = val;
                self.update_stat_line();
            }
            _ => self.regs[i] = val,
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn lcd_on_ppu() -> Ppu {
        let mut p = Ppu::new();
        p.write_reg(0xFF40, 0);
        p.write_reg(0xFF40, 0x91);
        p
    }

    /// Run until LY reg == ly and dot == d.
    fn run_to(p: &mut Ppu, ly: u8, d: u16) -> u8 {
        let mut irq = 0;
        for _ in 0..200_000 {
            if p.ly == ly && p.dot == d {
                return irq;
            }
            irq |= p.tick(1);
        }
        panic!("never reached");
    }

    #[test]
    fn post_boot_stat() {
        let p = Ppu::new();
        assert_eq!(p.read_reg(0xFF41), 0x85);
        assert_eq!(p.read_reg(0xFF44), 0);
    }

    #[test]
    fn line_timing_and_vblank() {
        let mut p = lcd_on_ppu();
        let irq = run_to(&mut p, 144, 0);
        assert_eq!(irq & irq::VBLANK, 0);
        assert_eq!(p.tick(4), irq::VBLANK);
        assert_eq!(p.read_reg(0xFF41) & 3, 1);
        assert!(p.take_frame_ready());
        run_to(&mut p, 0, 0);
        assert_eq!(p.dot, 0);
    }

    #[test]
    fn mode_sequence_on_line() {
        let mut p = lcd_on_ppu();
        run_to(&mut p, 1, 10);
        assert_eq!(p.read_reg(0xFF41) & 3, 2);
        run_to(&mut p, 1, 100);
        assert_eq!(p.read_reg(0xFF41) & 3, 3);
        assert_eq!(p.read_vram(0x8000), 0xFF);
        assert_eq!(p.read_oam(0xFE00), 0xFF);
        run_to(&mut p, 1, 300);
        assert_eq!(p.read_reg(0xFF41) & 3, 0);
        assert_eq!(p.mode0_dot, 80 + 172);
        p.write_vram(0x8000, 7);
        assert_eq!(p.read_vram(0x8000), 7);
    }

    #[test]
    fn mode3_length_scx_and_sprites() {
        let mut p = lcd_on_ppu();
        p.write_reg(0xFF43, 5);
        p.write_reg(0xFF40, 0x93);
        p.write_oam_dma(0, 16 + 20);
        p.write_oam_dma(1, 8 + 3); // x=3 -> offset (3+8... pos=11+5=16) tile aligned
        run_to(&mut p, 20, 100);
        // pos = 11 + 5 = 16 -> offset 0 -> 5 + 6
        assert_eq!(p.mode0_dot, 80 + 172 + 5 + 11);
    }

    #[test]
    fn lcd_off_state() {
        let mut p = lcd_on_ppu();
        run_to(&mut p, 50, 100);
        p.write_reg(0xFF40, 0x11);
        assert_eq!(p.read_reg(0xFF44), 0);
        assert_eq!(p.read_reg(0xFF41) & 3, 0);
        assert_eq!(p.read_vram(0x8000), 0);
        assert_eq!(p.read_oam(0xFE00), 0);
        assert_eq!(p.tick(1000), 0);
        assert_eq!(p.read_reg(0xFF44), 0);
    }

    #[test]
    fn ly_153_quirk() {
        let mut p = lcd_on_ppu();
        run_to(&mut p, 153, 2);
        assert_eq!(p.read_reg(0xFF44), 153);
        run_to(&mut p, 153, 8);
        assert_eq!(p.read_reg(0xFF44), 0);
    }

    #[test]
    fn lyc_stat_irq_once_per_match() {
        let mut p = lcd_on_ppu();
        p.write_reg(0xFF45, 5);
        p.write_reg(0xFF41, 0x40);
        let irq = run_to(&mut p, 5, 10);
        assert_eq!(irq & irq::STAT, irq::STAT);
        assert_ne!(p.read_reg(0xFF41) & 4, 0);
        let irq = run_to(&mut p, 6, 10);
        assert_eq!(irq & irq::STAT, 0);
        assert_eq!(p.read_reg(0xFF41) & 4, 0);
    }

    #[test]
    fn stat_blocking_or_line() {
        let mut p = lcd_on_ppu();
        // mode 0 + mode 2 sources: mode 2 -> 3 -> 0 produces irq at mode 0 only
        // when line was low; mode 0 follows mode 3 (low) so both fire.
        p.write_reg(0xFF41, 0x28);
        let irq = run_to(&mut p, 2, 0);
        assert_eq!(irq & irq::STAT, irq::STAT);
        // hblank then next line mode 2: line stays high only if sources overlap.
        let mut p = lcd_on_ppu();
        p.write_reg(0xFF41, 0x08);
        p.write_reg(0xFF45, 3);
        p.write_reg(0xFF41, 0x48);
        let mut count = 0;
        for _ in 0..(456 * 5) {
            if p.tick(1) & irq::STAT != 0 {
                count += 1;
            }
        }
        assert!(count > 0);
    }

    fn setup_tile(p: &mut Ppu, tile: usize, rows: [(u8, u8); 8]) {
        for (r, (lo, hi)) in rows.iter().enumerate() {
            p.vram[tile * 16 + r * 2] = *lo;
            p.vram[tile * 16 + r * 2 + 1] = *hi;
        }
    }

    #[test]
    fn bg_scroll_and_palette() {
        let mut p = lcd_on_ppu();
        // tile 1: left pixel color 3, others 0.
        setup_tile(&mut p, 1, [(0x80, 0x80); 8]);
        p.vram[0x1800 + 1] = 1; // map (col 1, row 0)
        run_to(&mut p, 1, 0);
        run_to(&mut p, 1, 300);
        // line 0 rendered: pixel x=8 black
        assert_eq!(p.framebuffer()[8], 0);
        assert_eq!(p.framebuffer()[9], 0x00FF_FFFF);
        p.write_reg(0xFF43, 8);
        run_to(&mut p, 2, 300);
        assert_eq!(p.framebuffer()[160 * 2], 0);
    }

    #[test]
    fn sprite_priority_and_flip() {
        let mut p = lcd_on_ppu();
        p.write_reg(0xFF40, 0x93);
        p.write_reg(0xFF48, 0xE4);
        setup_tile(&mut p, 2, [(0x80, 0x00); 8]); // leftmost px color 1
        p.write_oam_dma(0, 16);
        p.write_oam_dma(1, 8 + 4);
        p.write_oam_dma(2, 2);
        p.write_oam_dma(3, 0x20); // x flip
        run_to(&mut p, 1, 0);
        run_to(&mut p, 1, 300);
        assert_eq!(p.framebuffer()[4 + 7], 0x00AA_AAAA);
        assert_eq!(p.framebuffer()[4], 0x00FF_FFFF);
    }

    #[test]
    fn window_line_counter_only_when_drawn() {
        let mut p = lcd_on_ppu();
        p.write_reg(0xFF40, 0xF1); // window on, map 9C00 for window
        p.vram[0x1800..0x1C00].fill(1);
        setup_tile(&mut p, 0, [(0xFF, 0xFF); 8]);
        p.write_reg(0xFF4A, 2);
        p.write_reg(0xFF4B, 7);
        run_to(&mut p, 5, 300);
        // window drew on lines 2..=5 -> counter 4
        assert_eq!(p.window_line, 4);
        assert_eq!(p.framebuffer()[2 * 160], 0);
        assert_eq!(p.framebuffer()[1 * 160], 0x00FF_FFFF);
    }
}
