//! Dot-accurate pixel pipeline: BG/window fetcher, BG FIFO, sprite FIFO.
//!
//! One call of [`Ppu::pipe_cycle`] per dot of mode 3. Cycle 1 is the dot after the OAM scan
//! ends. The model, in the order each cycle does things:
//!
//! 1. The BG fetcher advances one step (tile index, low byte, high byte: two steps each) and, once
//!    its row is complete and the BG FIFO is empty, pushes 8 pixels and starts the next tile.
//! 2. The window starts when the pixel counter reaches WX (FIFO flushed, fetcher restarted: 6 dots).
//! 3. A sprite whose X matches the pixel counter stalls the pixel clock until the BG fetch has
//!    completed, then for 6 dots while its row is fetched and merged into the sprite FIFO.
//! 4. One pixel is popped from both FIFOs and mixed.
//!
//! The pixel counter `cx` counts pops from 0. The first 8 pops are the left margin (they exist so
//! sprites at OAM X < 8 can match) followed by `SCX & 7` discarded pixels; screen pixel 0 is pop
//! `8 + (SCX & 7)`. The FIFO is pre-loaded with 8 blank pixels and pops start on cycle 5, so the
//! first real tile is pushed on cycle 13: 12 dots of startup + 160 pixels = 172 dots.

use super::*;

/// Idle dots before the first pop.
const STARTUP: u16 = 4;
/// Fetcher steps per tile row.
const FETCH_STEPS: u8 = 6;
const SPRITE_STALL: u8 = 6;
/// The first sprite of a line costs 3 dots less than the following ones (measured against
/// mooneye `intr_2_mode0_timing_sprites`).
const FIRST_SPRITE_STALL: u8 = 6;
/// Dots mode 3 ends before the last pixel when any sprite was fetched.
const SPRITE_TAIL: u16 = 3;
/// `mode0_dot` while mode 3 is running (never matches a real dot).
/// Dots between a pixel leaving the FIFOs and its palette/priority lookup.
const MIX_DELAY: u16 = 7;
pub(super) const MODE3_RUNNING: u16 = 0xFF00;

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
struct BgPx {
    color: u8,
    /// CGB BG attributes (palette bits 0-2, priority bit 7).
    attr: u8,
}

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
struct ObjPx {
    /// 0 = transparent.
    color: u8,
    flags: u8,
    /// OAM index (CGB priority).
    idx: u8,
}

/// A popped pixel waiting for its colour lookup.
#[derive(Clone, Copy, Default, Serialize, Deserialize)]
struct Pending {
    bg: BgPx,
    obj: ObjPx,
    /// Screen X, or 0xFF for margin/discarded pixels (not displayed).
    x: u8,
    /// Dot (`cyc`) at which the pixel is mixed.
    due: u16,
    /// LCDC.0 / LCDC.1 are sampled at their own dots (`bg_en_due` / `obj_en_due`).
    bg_en_due: u16,
    obj_en_due: u16,
    bg_en: bool,
    obj_en: bool,
}

fn env_delay(name: &str, default: u16) -> u16 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

#[derive(Default, Serialize, Deserialize)]
pub(super) struct Pipe {
    pub active: bool,
    /// Dots since mode 3 started.
    cyc: u16,
    /// Ring head shared by both FIFOs.
    head: u8,
    bg: [BgPx; 8],
    bg_len: u8,
    obj: [ObjPx; 8],
    /// Fetcher progress within the current tile row (0..=6).
    prog: u8,
    /// Tiles pushed so far (BG column offset, or window column).
    k: u8,
    win: bool,
    win_drawn: bool,
    tile: u8,
    attr: u8,
    lo: u8,
    hi: u8,
    /// Pops so far (margin and discards included).
    cx: u16,
    /// `SCX & 7`, latched when mode 3 starts.
    s: u8,
    /// Next sprite (in `Ppu::sprites`, X order) that may still match.
    sprite_i: u8,
    first_sprite_done: bool,
    /// Dots left in a sprite stall, of which `wait` are spent finishing the BG fetch.
    stall: u8,
    wait: u8,
    group: (u8, u8),
    /// A sprite stall ended since the last BG push: the next tile fetch starts late.
    post_sprite: bool,
    fetch_hold: u8,
    /// Popped pixels waiting for their palette lookup (`MIX_DELAY` dots after the pop).
    delay: [Pending; 8],
    delay_head: u8,
    delay_len: u8,
}

impl Pipe {
    /// Pop count at which the last visible pixel has left the FIFOs.
    fn pixels_end(&self) -> u16 {
        8 + self.s as u16 + SCREEN_WIDTH as u16
    }
}

impl Ppu {
    pub(super) fn pipe_start(&mut self) {
        let s = self.regs[SCX] & 7;
        self.pipe = Pipe {
            active: true,
            bg_len: 8,
            s,
            ..Pipe::default()
        };
        self.mode0_dot = MODE3_RUNNING;
    }

    /// Colour the pixel whose delay has elapsed (at most one per dot).
    fn mix_due(&mut self) {
        if self.pipe.delay_len == 0 {
            return;
        }
        let lcdc = self.regs[LCDC];
        for k in 0..self.pipe.delay_len as usize {
            let e = &mut self.pipe.delay[(self.pipe.delay_head as usize + k) & 7];
            if e.bg_en_due == self.pipe.cyc {
                e.bg_en = lcdc & 1 != 0;
            }
            if e.obj_en_due == self.pipe.cyc {
                e.obj_en = lcdc & 2 != 0;
            }
        }
        let head = self.pipe.delay[self.pipe.delay_head as usize];
        if head.due > self.pipe.cyc {
            return;
        }
        self.pipe.delay_head = (self.pipe.delay_head + 1) & 7;
        self.pipe.delay_len -= 1;
        if head.x != 0xFF {
            let color = self.mix(head);
            self.framebuffer[self.ly as usize * SCREEN_WIDTH + head.x as usize] = color;
        }
    }

    /// Dots after the last pop while its pixels are still being coloured.
    pub(super) fn pipe_drain(&mut self) {
        self.pipe.cyc += 1;
        self.mix_due();
    }

    pub(super) fn pipe_draining(&self) -> bool {
        self.pipe.delay_len > 0
    }

    pub(super) fn pipe_cycle(&mut self) {
        self.pipe_cycle_inner();
        // With sprites on the line mode 3 ends SPRITE_TAIL dots before the last pixel.
        let p = &self.pipe;
        if p.active && p.first_sprite_done && self.mode0_dot == MODE3_RUNNING && !self.sprite_hit_ahead() {
            let pops_left = p.pixels_end() - p.cx;
            let dots_left = if p.stall > 0 {
                p.stall as u16 + pops_left - 1
            } else {
                pops_left
            };
            if dots_left <= SPRITE_TAIL {
                self.mode0_dot = MODE3_START + p.cyc;
            }
        }
    }

    /// A sprite will still match the pixel counter before the line ends.
    fn sprite_hit_ahead(&self) -> bool {
        let p = &self.pipe;
        (p.sprite_i as usize..self.sprite_count).any(|j| {
            let at = self.sprites[j].x as u16 + p.s as u16;
            at >= p.cx && at < p.pixels_end() && self.regs[LCDC] & 0x02 != 0
        })
    }

    fn pipe_cycle_inner(&mut self) {
        self.pipe.cyc += 1;
        self.mix_due();
        if self.pipe.cyc <= STARTUP {
            return;
        }
        if self.pipe.stall > 0 {
            self.pipe.stall -= 1;
            if self.pipe.wait > 0 {
                self.pipe.wait -= 1;
                self.fetch_step(false);
            }
            if self.pipe.stall > 0 {
                return;
            }
            self.merge_sprites();
            self.pipe.post_sprite = true;
            self.pop();
            return;
        }
        self.fetch_step(true);
        if self.pipe.bg_len == 0 {
            return;
        }
        if self.try_start_window() || self.try_sprite_hit() {
            return;
        }
        self.pop();
    }

    fn fetch_step(&mut self, push: bool) {
        if self.pipe.fetch_hold > 0 {
            self.pipe.fetch_hold -= 1;
            return;
        }
        if self.pipe.prog < FETCH_STEPS {
            self.pipe.prog += 1;
            let (ti, lo, hi) = (
                env_delay("GB_TI", 4) as u8,
                env_delay("GB_LO", 4) as u8,
                env_delay("GB_HI", 6) as u8,
            );
            if self.pipe.prog == ti {
                self.fetch_tile_index();
            }
            if self.pipe.prog == lo {
                self.pipe.lo = self.vram[self.fetch_addr()];
            }
            if self.pipe.prog == hi {
                self.pipe.hi = self.vram[self.fetch_addr() + 1];
            }
        }
        if push && self.pipe.prog >= FETCH_STEPS && self.pipe.bg_len == 0 {
            self.push_row();
            self.pipe.prog = 1;
            self.pipe.k = self.pipe.k.wrapping_add(1);
            if self.pipe.post_sprite {
                self.pipe.post_sprite = false;
                self.pipe.fetch_hold = env_delay("GB_HOLD", 0) as u8;
            }
        }
    }

    fn fetch_tile_index(&mut self) {
        let lcdc = self.regs[LCDC];
        let (map, col) = if self.pipe.win {
            let base = if lcdc & 0x40 != 0 { 0x1C00 } else { 0x1800 };
            (base + (self.window_line as usize / 8) * 32, self.pipe.k as usize & 31)
        } else {
            let y = self.ly.wrapping_add(self.regs[SCY]) as usize;
            let base = if lcdc & 0x08 != 0 { 0x1C00 } else { 0x1800 };
            (
                base + (y / 8) * 32,
                ((self.regs[SCX] >> 3) as usize + self.pipe.k as usize) & 31,
            )
        };
        self.pipe.tile = self.vram[map + col];
        self.pipe.attr = if self.cgb_mode() {
            self.vram[0x2000 + map + col]
        } else {
            0
        };
    }

    /// VRAM offset of the current tile row's low byte (high byte is the next one).
    fn fetch_addr(&self) -> usize {
        let line = if self.pipe.win {
            self.window_line
        } else {
            self.ly.wrapping_add(self.regs[SCY])
        };
        let mut row = line & 7;
        let attr = self.pipe.attr;
        if attr & 0x40 != 0 {
            row = 7 - row;
        }
        self.tile_addr(self.pipe.tile, row, false, (attr >> 3) & 1)
    }

    fn push_row(&mut self) {
        let Pipe { lo, hi, attr, head, .. } = self.pipe;
        for i in 0..8u8 {
            let bit = if attr & 0x20 != 0 { i } else { 7 - i };
            let color = ((lo >> bit) & 1) | (((hi >> bit) & 1) << 1);
            self.pipe.bg[((head + i) & 7) as usize] = BgPx { color, attr };
        }
        self.pipe.bg_len = 8;
    }

    /// Start the window if the pixel counter reached WX. Costs the cycle (the FIFO is flushed and
    /// the fetcher restarts on window tiles).
    fn try_start_window(&mut self) -> bool {
        let lcdc = self.regs[LCDC];
        if self.pipe.win || !self.wy_triggered || lcdc & 0x20 == 0 || (!self.cgb_mode() && lcdc & 1 == 0) {
            return false;
        }
        if self.pipe.cx != self.regs[WX] as u16 + 1 + self.pipe.s as u16 {
            return false;
        }
        let p = &mut self.pipe;
        p.win = true;
        p.win_drawn = true;
        p.k = 0;
        p.prog = 0;
        p.bg_len = 0;
        true
    }

    /// Detect sprites whose X matches the pixel counter and begin the stall for them.
    fn try_sprite_hit(&mut self) -> bool {
        let cx = self.pipe.cx;
        let s = self.pipe.s as u16;
        let n = self.sprite_count as u8;
        let mut i = self.pipe.sprite_i;
        while i < n && (self.sprites[i as usize].x as u16 + s) < cx {
            i += 1;
        }
        self.pipe.sprite_i = i;
        if i >= n || self.sprites[i as usize].x as u16 + s != cx || self.regs[LCDC] & 0x02 == 0 {
            return false;
        }
        let g0 = i;
        while i < n && self.sprites[i as usize].x as u16 + s == cx {
            i += 1;
        }
        let wait = FETCH_STEPS.saturating_sub(self.pipe.prog);
        let mut stall = wait;
        for _ in g0..i {
            stall += if self.pipe.first_sprite_done {
                SPRITE_STALL
            } else {
                FIRST_SPRITE_STALL
            };
            self.pipe.first_sprite_done = true;
        }
        self.pipe.sprite_i = i;
        self.pipe.group = (g0, i);
        self.pipe.wait = wait;
        self.pipe.stall = stall;
        true
    }

    /// Fetch the matched sprites' rows and merge them into the sprite FIFO.
    fn merge_sprites(&mut self) {
        let (g0, g1) = self.pipe.group;
        let cgb = self.cgb_mode();
        let oam_order = cgb && self.opri & 1 == 0;
        let h = self.sprite_height();
        for j in g0..g1 {
            let sp = self.sprites[j as usize];
            let mut row = (self.ly as i16 + 16 - sp.y as i16) as u8;
            if sp.flags & 0x40 != 0 {
                row = h - 1 - row;
            }
            let tile = if h == 16 { sp.tile & 0xFE } else { sp.tile };
            let bank = if cgb { (sp.flags >> 3) & 1 } else { 0 };
            let (lo, hi) = self.tile_row(tile, row, true, bank);
            for i in 0..8u8 {
                let bit = if sp.flags & 0x20 != 0 { i } else { 7 - i };
                let color = ((lo >> bit) & 1) | (((hi >> bit) & 1) << 1);
                if color == 0 {
                    continue;
                }
                let slot = &mut self.pipe.obj[((self.pipe.head + i) & 7) as usize];
                if slot.color == 0 || (oam_order && sp.idx < slot.idx) {
                    *slot = ObjPx {
                        color,
                        flags: sp.flags,
                        idx: sp.idx,
                    };
                }
            }
        }
    }

    /// Shift one pixel out of both FIFOs, mixing it onto the screen if it is visible.
    fn pop(&mut self) {
        let i = self.pipe.head as usize;
        let bg = self.pipe.bg[i];
        let obj = self.pipe.obj[i];
        self.pipe.obj[i] = ObjPx::default();
        self.pipe.head = (self.pipe.head + 1) & 7;
        self.pipe.bg_len -= 1;
        let cx = self.pipe.cx;
        self.pipe.cx += 1;
        let first_visible = 8 + self.pipe.s as u16;
        let x = cx.wrapping_sub(first_visible);
        let x = if x < SCREEN_WIDTH as u16 { x as u8 } else { 0xFF };
        let p = &mut self.pipe;
        let tail = ((p.delay_head + p.delay_len) & 7) as usize;
        p.delay[tail] = Pending {
            bg,
            obj,
            x,
            due: p.cyc + MIX_DELAY,
            bg_en_due: p.cyc + env_delay("GB_BGEN", 6),
            obj_en_due: p.cyc + env_delay("GB_OBJEN", 6),
            bg_en: true,
            obj_en: true,
        };
        p.delay_len += 1;
        if self.pipe.cx == self.pipe.pixels_end() {
            self.pipe.active = false;
            if self.mode0_dot == MODE3_RUNNING {
                self.mode0_dot = MODE3_START + self.pipe.cyc;
            }
            if self.pipe.win_drawn {
                self.window_line += 1;
            }
        }
    }

    fn mix(&self, e: Pending) -> u32 {
        let (bg, obj) = (e.bg, e.obj);
        let lcdc = self.regs[LCDC];
        let cgb = self.cgb_mode();
        let bg_id = if !cgb && !e.bg_en { 0 } else { bg.color };
        let obj_visible = e.obj_en && obj.color != 0;
        let bg_wins = if cgb {
            e.bg_en && (obj.flags & 0x80 != 0 || bg.attr & 0x80 != 0)
        } else {
            obj.flags & 0x80 != 0
        };
        if obj_visible && !(bg_wins && bg_id != 0) {
            if cgb {
                self.obj_rgb[(obj.flags & 7) as usize * 4 + obj.color as usize]
            } else {
                let pal = (obj.flags >> 4) & 1;
                let shade = ((self.regs[OBP0 + pal as usize] >> (obj.color * 2)) & 3) as usize;
                if self.compat {
                    self.obj_rgb[pal as usize * 4 + shade]
                } else {
                    self.palette[shade]
                }
            }
        } else if cgb {
            self.bg_rgb[(bg.attr & 7) as usize * 4 + bg_id as usize]
        } else {
            let shade = ((self.regs[BGP] >> (bg_id * 2)) & 3) as usize;
            if self.compat {
                self.bg_rgb[shade]
            } else {
                self.palette[shade]
            }
        }
    }
}
