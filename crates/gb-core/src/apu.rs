//! APU: sound registers 0xFF10-0xFF26 and wave RAM 0xFF30-0xFF3F.
//!
//! Channels keep a countdown (in T-cycles) to their next event; `tick` jumps
//! from event to event (and to output-sample boundaries), accumulating the
//! mixed level over each constant stretch, so the output is a box-filtered
//! downsample of the 4.194304 MHz signal.

use serde::{Deserialize, Serialize};

const CLOCK: u64 = 4_194_304;
const NEVER: u32 = u32::MAX;

/// Read-back OR masks for 0xFF10..=0xFF3F (wave RAM excluded).
const READ_MASK: [u8; 0x20] = [
    0x80, 0x3F, 0x00, 0xFF, 0xBF, // NR10-14
    0xFF, 0x3F, 0x00, 0xFF, 0xBF, // NR20-24
    0x7F, 0xFF, 0x9F, 0xFF, 0xBF, // NR30-34
    0xFF, 0xFF, 0x00, 0x00, 0xBF, // NR40-44
    0x00, 0x00, 0x70, // NR50-52
    0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, // unused
];

const DUTY: [[u8; 8]; 4] = [
    [0, 0, 0, 0, 0, 0, 0, 1],
    [1, 0, 0, 0, 0, 0, 0, 1],
    [1, 0, 0, 0, 0, 1, 1, 1],
    [0, 1, 1, 1, 1, 1, 1, 0],
];

#[derive(Clone, Default, Serialize, Deserialize)]
struct Envelope {
    volume: u8,
    timer: u8,
    /// Still doing automatic updates (cleared when the volume reaches its limit).
    active: bool,
}

impl Envelope {
    fn trigger(&mut self, nrx2: u8) {
        self.volume = nrx2 >> 4;
        self.timer = Self::period(nrx2);
        self.active = true;
    }
    /// NRx2 written while the channel plays ("zombie mode"): the volume is bumped depending on the old value.
    fn zombie_write(&mut self, old: u8, new: u8, cgb: bool) {
        let mut v = self.volume as i32;
        let invert = (old ^ new) & 8 != 0;
        if cgb {
            let tick = new & 7 != 0 && old & 7 == 0;
            if invert {
                // Fitted to SameSuite channel_*_volume: going to add mode costs 1 (2 if the old period was nonzero),
                // going to subtract mode costs 1 only when the envelope would also have ticked.
                v = if new & 8 != 0 {
                    16 - v - if old & 7 == 0 { 1 } else { 2 }
                } else {
                    16 - v - tick as i32
                };
            } else if tick || (new & 0xF == 8 && old & 0xF == 8) {
                v += if new & 8 != 0 { 1 } else { -1 };
            }
        } else {
            if old & 7 == 0 && self.active {
                v += 1;
            } else if old & 8 == 0 {
                v += 2;
            }
            if invert {
                v = 16 - v;
            }
        }
        self.volume = (v & 15) as u8;
    }
    fn period(nrx2: u8) -> u8 {
        match nrx2 & 7 {
            0 => 8,
            p => p,
        }
    }
    fn clock(&mut self, nrx2: u8) {
        if nrx2 & 7 == 0 {
            return;
        }
        self.timer = self.timer.saturating_sub(1);
        if self.timer == 0 {
            self.timer = Self::period(nrx2);
            if nrx2 & 8 != 0 {
                if self.volume < 15 {
                    self.volume += 1;
                }
                self.active = self.volume < 15;
            } else {
                if self.volume > 0 {
                    self.volume -= 1;
                }
                self.active = self.volume > 0;
            }
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Square {
    enabled: bool,
    timer: u32,
    pos: u8,
    /// Duty output bit latched when the position last advanced (duty changes apply from the next step).
    out: u8,
    len: u16,
    env: Envelope,
    // sweep (channel 1 only)
    shadow: u16,
    sweep_timer: u8,
    sweep_enabled: bool,
    negate_used: bool,
}

impl Square {
    fn new() -> Self {
        Square {
            enabled: false,
            timer: NEVER,
            pos: 0,
            out: 0,
            len: 0,
            env: Envelope::default(),
            shadow: 0,
            sweep_timer: 0,
            sweep_enabled: false,
            negate_used: false,
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct Apu {
    /// CGB (non-AGB) behaviour: free wave RAM access while playing, no retrigger corruption,
    /// length counters cleared by power-off and locked while off.
    #[serde(default)]
    cgb: bool,
    /// DIV bit that clocks the frame sequencer, sampled by the bus just before an NR52 write.
    #[serde(skip)]
    div_bit: bool,
    /// Powered on while the DIV bit was high: the first frame sequencer event is skipped.
    skip_fs: bool,
    #[serde(skip)]
    ds: bool,
    /// After such a power-on the sequencer behaves as if its next step were odd until the first event runs.
    #[serde(skip)]
    fs_quirk_odd: bool,
    /// Cycle counter value when the APU was last powered on: the 1 MHz channel clock phase restarts there.
    #[serde(skip)]
    power_on_at: u64,
    /// Number of times the mixed output level changed (test-harness probe: "was the output silent/constant").
    #[serde(skip)]
    level_changes: u64,
    #[serde(with = "crate::state::bytes")]
    regs: [u8; 0x30],
    #[serde(with = "crate::state::bytes")]
    wave_ram: [u8; 16],
    sample_rate: u32,
    power: bool,
    /// Next frame sequencer step to run (0..8).
    fs: u8,

    ch1: Square,
    ch2: Square,
    // wave
    ch3_enabled: bool,
    ch3_timer: u32,
    ch3_pos: u8,
    ch3_len: u16,
    ch3_sample: u8,
    // noise
    ch4_enabled: bool,
    ch4_timer: u32,
    ch4_len: u16,
    ch4_env: Envelope,
    ch4_lfsr: u16,

    /// Free-running T-cycle counter, used for wave RAM access timing.
    cycles: u64,
    ch3_last_read: u64,

    // output
    level_l: f32,
    level_r: f32,
    acc_l: f64,
    acc_r: f64,
    /// `cycles` at the last accumulator flush / start of the current output window.
    acc_from: u64,
    win_start: u64,
    until_sample: u32,
    hp_charge: f32,
    phase: u64,
    hp_l: f32,
    hp_r: f32,
    /// Undelivered audio is not machine state.
    #[serde(skip)]
    out: Vec<f32>,
}

impl Apu {
    pub fn new() -> Self {
        let mut apu = Apu {
            regs: [0; 0x30],
            wave_ram: [0; 16],
            sample_rate: 48_000,
            cgb: false,
            power_on_at: 0,
            div_bit: false,
            skip_fs: false,
            ds: false,
            fs_quirk_odd: false,
            level_changes: 0,
            power: true,
            fs: 0,
            ch1: Square::new(),
            ch2: Square::new(),
            ch3_enabled: false,
            ch3_timer: NEVER,
            ch3_pos: 0,
            ch3_len: 0,
            ch3_sample: 0,
            ch4_enabled: false,
            ch4_timer: NEVER,
            ch4_len: 0,
            ch4_env: Envelope::default(),
            ch4_lfsr: 0x7FFF,
            cycles: 0,
            ch3_last_read: 0,
            level_l: 0.0,
            level_r: 0.0,
            acc_l: 0.0,
            acc_r: 0.0,
            acc_from: 0,
            win_start: 0,
            until_sample: CLOCK.div_ceil(48_000) as u32,
            hp_charge: 0.999958f32.powf(CLOCK as f32 / 48_000.0),
            phase: 0,
            hp_l: 0.0,
            hp_r: 0.0,
            out: Vec::new(),
        };
        // Post-boot DMG state.
        apu.write(0xFF26, 0xF1);
        apu.write(0xFF11, 0xBF);
        apu.write(0xFF12, 0xF3);
        apu.write(0xFF25, 0xF3);
        apu.write(0xFF24, 0x77);
        apu.write(0xFF14, 0xBF); // trigger ch1 silently as the boot ROM does
        apu.write(0xFF1A, 0x7F);
        apu.write(0xFF1C, 0x9F);
        apu.write(0xFF21, 0x00);
        apu.update_levels();
        apu
    }

    // ---------------------------------------------------------------- timing

    /// Advance `t_cycles` T-cycles.
    pub fn tick(&mut self, t_cycles: u32) {
        self.ds = t_cycles < 4;
        let rate = self.sample_rate as u64;
        // A disabled square channel's frequency timer is frozen (its duty position is kept).
        let t1 = if self.ch1.enabled { self.ch1.timer } else { NEVER };
        let t2 = if self.ch2.enabled { self.ch2.timer } else { NEVER };
        // Fast path: no channel event and no output sample is due within this call.
        if t_cycles < self.until_sample && t_cycles < t1.min(t2).min(self.ch3_timer).min(self.ch4_timer) {
            self.cycles += t_cycles as u64;
            self.phase += rate * t_cycles as u64;
            self.until_sample -= t_cycles;
            if t1 != NEVER {
                self.ch1.timer -= t_cycles;
            }
            if t2 != NEVER {
                self.ch2.timer -= t_cycles;
            }
            if self.ch3_timer != NEVER {
                self.ch3_timer -= t_cycles;
            }
            if self.ch4_timer != NEVER {
                self.ch4_timer -= t_cycles;
            }
            return;
        }
        let mut remaining = t_cycles;
        while remaining > 0 {
            let (run1, run2) = (self.ch1.enabled, self.ch2.enabled);
            let chunk = remaining
                .min(self.until_sample)
                .min(if run1 { self.ch1.timer } else { NEVER })
                .min(if run2 { self.ch2.timer } else { NEVER })
                .min(self.ch3_timer)
                .min(self.ch4_timer);

            self.phase += rate * chunk as u64;
            self.until_sample -= chunk;
            self.cycles += chunk as u64;
            remaining -= chunk;

            let mut changed = false;
            if run1 && self.ch1.timer != NEVER {
                self.ch1.timer -= chunk;
                if self.ch1.timer == 0 {
                    self.ch1.timer = self.square_period(0);
                    self.ch1.pos = (self.ch1.pos + 1) & 7;
                    self.ch1.out = DUTY[(self.regs[0x01] >> 6) as usize][self.ch1.pos as usize];
                    changed = true;
                }
            }
            if run2 && self.ch2.timer != NEVER {
                self.ch2.timer -= chunk;
                if self.ch2.timer == 0 {
                    self.ch2.timer = self.square_period(5);
                    self.ch2.pos = (self.ch2.pos + 1) & 7;
                    self.ch2.out = DUTY[(self.regs[0x06] >> 6) as usize][self.ch2.pos as usize];
                    changed = true;
                }
            }
            if self.ch3_timer != NEVER {
                self.ch3_timer -= chunk;
                if self.ch3_timer == 0 {
                    self.ch3_timer = self.wave_period();
                    self.ch3_pos = (self.ch3_pos + 1) & 31;
                    self.ch3_sample = self.wave_nibble(self.ch3_pos);
                    self.ch3_last_read = self.cycles;
                    changed = true;
                }
            }
            if self.ch4_timer != NEVER {
                self.ch4_timer -= chunk;
                if self.ch4_timer == 0 {
                    self.ch4_timer = self.noise_period();
                    self.clock_lfsr();
                    changed = true;
                }
            }
            if changed {
                self.update_levels();
            }

            if self.phase >= CLOCK {
                self.phase -= CLOCK;
                self.emit_sample();
                self.until_sample = self.samples_until_next();
            }
        }
    }

    /// T-cycles until the output phase accumulator next wraps.
    fn samples_until_next(&self) -> u32 {
        (CLOCK - self.phase).div_ceil(self.sample_rate as u64).max(1) as u32
    }

    /// Fold the current output level into the running average up to the present cycle.
    fn flush_acc(&mut self) {
        let d = (self.cycles - self.acc_from) as f64;
        self.acc_l += self.level_l as f64 * d;
        self.acc_r += self.level_r as f64 * d;
        self.acc_from = self.cycles;
    }

    fn emit_sample(&mut self) {
        self.flush_acc();
        let n = (self.cycles - self.win_start).max(1) as f64;
        let l = (self.acc_l / n) as f32;
        let r = (self.acc_r / n) as f32;
        self.acc_l = 0.0;
        self.acc_r = 0.0;
        self.win_start = self.cycles;
        // DC-blocking high-pass (capacitor), DMG charge factor.
        let charge = self.hp_charge;
        let ol = l - self.hp_l;
        self.hp_l = l - ol * charge;
        let or = r - self.hp_r;
        self.hp_r = r - or * charge;
        self.out.push(ol.clamp(-1.0, 1.0));
        self.out.push(or.clamp(-1.0, 1.0));
    }

    /// Frame sequencer clock (512 Hz): called by the bus on the falling edge
    /// of DIV-counter bit 12.
    pub fn frame_sequencer_step(&mut self) {
        if !self.power {
            return;
        }
        if self.skip_fs {
            self.skip_fs = false;
            return;
        }
        self.fs_quirk_odd = false;
        let step = self.fs;
        self.fs = (self.fs + 1) & 7;
        if step & 1 == 0 {
            self.clock_lengths();
        }
        if step == 2 || step == 6 {
            self.clock_sweep();
        }
        if step == 7 {
            let (r1, r2, r4) = (self.regs[0x02], self.regs[0x07], self.regs[0x11]);
            if self.ch1.enabled {
                self.ch1.env.clock(r1);
            }
            if self.ch2.enabled {
                self.ch2.env.clock(r2);
            }
            if self.ch4_enabled {
                self.ch4_env.clock(r4);
            }
        }
        self.update_levels();
    }

    /// The next sequencer step does not clock the length counters (so enabling a length now clocks it once).
    fn fs_odd(&self) -> bool {
        self.fs & 1 == 1 || self.fs_quirk_odd
    }

    fn clock_lengths(&mut self) {
        if self.regs[0x04] & 0x40 != 0 && self.ch1.len > 0 {
            self.ch1.len -= 1;
            if self.ch1.len == 0 {
                self.ch1.enabled = false;
            }
        }
        if self.regs[0x09] & 0x40 != 0 && self.ch2.len > 0 {
            self.ch2.len -= 1;
            if self.ch2.len == 0 {
                self.ch2.enabled = false;
            }
        }
        if self.regs[0x0E] & 0x40 != 0 && self.ch3_len > 0 {
            self.ch3_len -= 1;
            if self.ch3_len == 0 {
                self.ch3_enabled = false;
            }
        }
        if self.regs[0x13] & 0x40 != 0 && self.ch4_len > 0 {
            self.ch4_len -= 1;
            if self.ch4_len == 0 {
                self.ch4_enabled = false;
            }
        }
    }

    // ----------------------------------------------------------------- sweep

    fn sweep_calc(&mut self) -> u16 {
        let nr10 = self.regs[0x00];
        let shift = nr10 & 7;
        let delta = self.ch1.shadow >> shift;
        if nr10 & 8 != 0 {
            self.ch1.negate_used = true;
            self.ch1.shadow.wrapping_sub(delta)
        } else {
            self.ch1.shadow + delta
        }
    }

    fn clock_sweep(&mut self) {
        if self.ch1.sweep_timer > 0 {
            self.ch1.sweep_timer -= 1;
        }
        if self.ch1.sweep_timer != 0 {
            return;
        }
        let nr10 = self.regs[0x00];
        let period = (nr10 >> 4) & 7;
        self.ch1.sweep_timer = if period == 0 { 8 } else { period };
        if self.ch1.sweep_enabled && period != 0 {
            let new = self.sweep_calc();
            if new > 2047 {
                self.ch1.enabled = false;
            } else if nr10 & 7 != 0 {
                self.ch1.shadow = new;
                self.regs[0x03] = new as u8;
                self.regs[0x04] = (self.regs[0x04] & !7) | (new >> 8) as u8;
                if self.sweep_calc() > 2047 {
                    self.ch1.enabled = false;
                }
            }
        }
    }

    // --------------------------------------------------------------- periods

    fn square_period(&self, base: usize) -> u32 {
        let f = self.regs[base + 3] as u32 | ((self.regs[base + 4] as u32 & 7) << 8);
        (2048 - f) * 4
    }

    /// Time to the first duty step after a trigger (CGB): the period plus two ticks (one if the channel was already
    /// running), rounded up to the next edge of the 1 MHz channel clock, which restarts at power-on. In double
    /// speed the extra ticks are dropped: SameSuite (CGB-E) wants them, Gambatte (CGB-C) does not, and we follow
    /// Gambatte there, as SameBoy does for pre-D revisions.
    fn square_start(&self, base: usize, active: bool) -> u32 {
        if !self.cgb {
            return self.square_period(base);
        }
        let t = self.square_period(base) + if active { 4 } else { 8 };
        let delta = (4 - ((self.cycles.wrapping_sub(self.power_on_at) as u32 + t) & 3)) % 4;
        if self.ds {
            t - 8 + delta
        } else {
            t + delta
        }
    }

    fn wave_period(&self) -> u32 {
        let f = self.regs[0x0D] as u32 | ((self.regs[0x0E] as u32 & 7) << 8);
        (2048 - f) * 2
    }

    fn noise_period(&self) -> u32 {
        let nr43 = self.regs[0x12];
        let div = match nr43 & 7 {
            0 => 8,
            d => d as u32 * 16,
        };
        div << (nr43 >> 4)
    }

    fn clock_lfsr(&mut self) {
        let nr43 = self.regs[0x12];
        if nr43 >> 4 >= 14 {
            return;
        }
        let x = (self.ch4_lfsr ^ (self.ch4_lfsr >> 1)) & 1;
        self.ch4_lfsr = (self.ch4_lfsr >> 1) | (x << 14);
        if nr43 & 8 != 0 {
            self.ch4_lfsr = (self.ch4_lfsr & !0x40) | (x << 6);
        }
    }

    fn wave_nibble(&self, pos: u8) -> u8 {
        let b = self.wave_ram[(pos >> 1) as usize];
        if pos & 1 == 0 {
            b >> 4
        } else {
            b & 0xF
        }
    }

    // ---------------------------------------------------------------- output

    fn dac(on: bool, digital: u8) -> f32 {
        if on {
            digital as f32 / 7.5 - 1.0
        } else {
            0.0
        }
    }

    /// Current 4-bit digital output of each channel (before the DAC), as exposed by PCM12/PCM34.
    fn digital(&self) -> [u8; 4] {
        let cgb = self.cgb;
        let sq = |c: &Square, nrx1: u8| {
            let bit = if cgb {
                c.out
            } else {
                DUTY[(nrx1 >> 6) as usize][c.pos as usize]
            };
            if c.enabled {
                bit * c.env.volume
            } else {
                0
            }
        };
        let d3 = if self.ch3_enabled {
            match (self.regs[0x0C] >> 5) & 3 {
                0 => 0,
                1 => self.ch3_sample,
                2 => self.ch3_sample >> 1,
                _ => self.ch3_sample >> 2,
            }
        } else {
            0
        };
        let d4 = if self.ch4_enabled && self.ch4_lfsr & 1 == 0 {
            self.ch4_env.volume
        } else {
            0
        };
        [sq(&self.ch1, self.regs[0x01]), sq(&self.ch2, self.regs[0x06]), d3, d4]
    }

    /// CGB PCM12 (0xFF76): channel 1 in the low nibble, channel 2 in the high nibble.
    pub fn pcm12(&self) -> u8 {
        let d = self.digital();
        d[0] | d[1] << 4
    }

    /// CGB PCM34 (0xFF77): channel 3 in the low nibble, channel 4 in the high nibble.
    pub fn pcm34(&self) -> u8 {
        let d = self.digital();
        d[2] | d[3] << 4
    }

    fn update_levels(&mut self) {
        self.flush_acc();
        if !self.power {
            if self.level_l != 0.0 || self.level_r != 0.0 {
                self.level_changes += 1;
            }
            self.level_l = 0.0;
            self.level_r = 0.0;
            return;
        }
        let d = self.digital();
        let dac_on = [
            self.regs[0x02] & 0xF8 != 0,
            self.regs[0x07] & 0xF8 != 0,
            self.regs[0x0A] & 0x80 != 0,
            self.regs[0x11] & 0xF8 != 0,
        ];
        let outs: [f32; 4] = std::array::from_fn(|i| Self::dac(dac_on[i], d[i]));
        let nr51 = self.regs[0x15];
        let nr50 = self.regs[0x14];
        let mix = |mask: u8, vol: u8| {
            let mut s = 0.0;
            for (i, o) in outs.iter().enumerate() {
                if mask & (1 << i) != 0 {
                    s += o;
                }
            }
            s * (vol + 1) as f32 / 8.0 / 4.0
        };
        let (r, l) = (mix(nr51 & 0xF, nr50 & 7), mix(nr51 >> 4, (nr50 >> 4) & 7));
        if r != self.level_r || l != self.level_l {
            self.level_changes += 1;
        }
        self.level_r = r;
        self.level_l = l;
    }

    // ------------------------------------------------------------- registers

    /// While the wave channel plays, wave RAM accesses hit the byte being
    /// fetched, and only in the cycle right after the fetch (DMG).
    fn wave_index(&self) -> Option<usize> {
        if !self.ch3_enabled {
            return Some(usize::MAX);
        }
        if self.cgb {
            return Some((self.ch3_pos >> 1) as usize);
        }
        if self.cycles - self.ch3_last_read < 2 {
            Some((self.ch3_pos >> 1) as usize)
        } else {
            None
        }
    }

    /// `addr` in 0xFF10..=0xFF3F.
    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            0xFF30..=0xFF3F => match self.wave_index() {
                Some(usize::MAX) => self.wave_ram[(addr - 0xFF30) as usize],
                Some(i) => self.wave_ram[i],
                None => 0xFF,
            },
            0xFF26 => {
                0x70 | (self.power as u8) << 7
                    | self.ch1.enabled as u8
                    | (self.ch2.enabled as u8) << 1
                    | (self.ch3_enabled as u8) << 2
                    | (self.ch4_enabled as u8) << 3
            }
            _ => {
                let i = (addr - 0xFF10) as usize;
                self.regs[i] | READ_MASK[i]
            }
        }
    }

    pub fn write(&mut self, addr: u16, val: u8) {
        if let 0xFF30..=0xFF3F = addr {
            match self.wave_index() {
                Some(usize::MAX) => self.wave_ram[(addr - 0xFF30) as usize] = val,
                Some(i) => self.wave_ram[i] = val,
                None => {}
            }
            return;
        }
        if addr == 0xFF26 {
            self.write_nr52(val);
            return;
        }
        let i = (addr - 0xFF10) as usize;
        if !self.power {
            // DMG: length counters stay writable while powered off (not on CGB).
            if self.cgb {
                return;
            }
            match i {
                0x01 => self.ch1.len = 64 - (val & 63) as u16,
                0x06 => self.ch2.len = 64 - (val & 63) as u16,
                0x0B => self.ch3_len = 256 - val as u16,
                0x10 => self.ch4_len = 64 - (val & 63) as u16,
                _ => {}
            }
            return;
        }
        let odd = self.fs_odd();
        match i {
            0x00 => {
                if self.regs[0] & 8 != 0 && val & 8 == 0 && self.ch1.negate_used {
                    self.ch1.enabled = false;
                }
                self.regs[i] = val;
            }
            0x01 => {
                self.regs[i] = val;
                self.ch1.len = 64 - (val & 63) as u16;
            }
            0x06 => {
                self.regs[i] = val;
                self.ch2.len = 64 - (val & 63) as u16;
            }
            0x0B => {
                self.regs[i] = val;
                self.ch3_len = 256 - val as u16;
            }
            0x10 => {
                self.regs[i] = val;
                self.ch4_len = 64 - (val & 63) as u16;
            }
            0x02 | 0x07 | 0x11 => {
                let old = self.regs[i];
                match i {
                    0x02 if self.ch1.enabled => self.ch1.env.zombie_write(old, val, self.cgb),
                    0x07 if self.ch2.enabled => self.ch2.env.zombie_write(old, val, self.cgb),
                    0x11 if self.ch4_enabled => self.ch4_env.zombie_write(old, val, self.cgb),
                    _ => {}
                }
                self.regs[i] = val;
                if val & 0xF8 == 0 {
                    match i {
                        0x02 => self.ch1.enabled = false,
                        0x07 => self.ch2.enabled = false,
                        _ => self.ch4_enabled = false,
                    }
                }
            }
            0x0A => {
                self.regs[i] = val;
                if val & 0x80 == 0 {
                    self.ch3_enabled = false;
                    self.ch3_sample = 0;
                }
            }
            0x04 | 0x09 | 0x0E | 0x13 => {
                let was = self.regs[i] & 0x40 != 0;
                self.regs[i] = val;
                let now = val & 0x40 != 0;
                let trigger = val & 0x80 != 0;
                if !was && now && odd {
                    let len = match i {
                        0x04 => &mut self.ch1.len,
                        0x09 => &mut self.ch2.len,
                        0x0E => &mut self.ch3_len,
                        _ => &mut self.ch4_len,
                    };
                    if *len > 0 {
                        *len -= 1;
                        if *len == 0 && !trigger {
                            match i {
                                0x04 => self.ch1.enabled = false,
                                0x09 => self.ch2.enabled = false,
                                0x0E => self.ch3_enabled = false,
                                _ => self.ch4_enabled = false,
                            }
                        }
                    }
                }
                if trigger {
                    match i {
                        0x04 => self.trigger_ch1(),
                        0x09 => self.trigger_ch2(),
                        0x0E => self.trigger_ch3(),
                        _ => self.trigger_ch4(),
                    }
                }
            }
            _ => {
                if i < 0x17 {
                    self.regs[i] = val;
                }
            }
        }
        self.update_levels();
    }

    fn write_nr52(&mut self, val: u8) {
        let on = val & 0x80 != 0;
        if on == self.power {
            return;
        }
        if !on {
            // Power off: clear registers and channel state (length counters
            // survive on DMG, wave RAM is untouched).
            for r in &mut self.regs[0..0x17] {
                *r = 0;
            }
            if self.cgb {
                self.ch1.len = 0;
                self.ch2.len = 0;
                self.ch3_len = 0;
                self.ch4_len = 0;
            }
            self.ch1.enabled = false;
            self.ch2.enabled = false;
            self.ch3_enabled = false;
            self.ch4_enabled = false;
            self.ch1.timer = NEVER;
            self.ch2.timer = NEVER;
            self.ch3_timer = NEVER;
            self.ch4_timer = NEVER;
            self.ch1.env = Envelope::default();
            self.ch2.env = Envelope::default();
            self.ch4_env = Envelope::default();
            self.ch1.sweep_enabled = false;
            self.ch3_sample = 0;
            self.power = false;
        } else {
            self.power = true;
            self.power_on_at = self.cycles;
            self.skip_fs = self.div_bit;
            self.fs_quirk_odd = self.div_bit;
            self.fs = 0;
            self.ch1.pos = 0;
            self.ch2.pos = 0;
            self.ch1.out = 0;
            self.ch2.out = 0;
            self.ch3_pos = 0;
        }
        self.update_levels();
    }

    // -------------------------------------------------------------- triggers

    fn trigger_ch1(&mut self) {
        let nr10 = self.regs[0x00];
        let dac = self.regs[0x02] & 0xF8 != 0;
        if self.ch1.len == 0 {
            self.ch1.len = 64;
            if self.regs[0x04] & 0x40 != 0 && self.fs_odd() {
                self.ch1.len -= 1;
            }
        }
        self.ch1.timer = self.square_start(0, self.ch1.enabled);
        if self.ch1.enabled {
            // Restarting an active channel re-evaluates the duty output immediately.
            self.ch1.out = DUTY[(self.regs[0x01] >> 6) as usize][self.ch1.pos as usize];
        }
        self.ch1.env.trigger(self.regs[0x02]);
        self.ch1.shadow = self.regs[0x03] as u16 | ((self.regs[0x04] as u16 & 7) << 8);
        let period = (nr10 >> 4) & 7;
        self.ch1.sweep_timer = if period == 0 { 8 } else { period };
        self.ch1.sweep_enabled = period != 0 || nr10 & 7 != 0;
        self.ch1.negate_used = false;
        self.ch1.enabled = dac;
        if nr10 & 7 != 0 && self.sweep_calc() > 2047 {
            self.ch1.enabled = false;
        }
    }

    fn trigger_ch2(&mut self) {
        let dac = self.regs[0x07] & 0xF8 != 0;
        if self.ch2.len == 0 {
            self.ch2.len = 64;
            if self.regs[0x09] & 0x40 != 0 && self.fs_odd() {
                self.ch2.len -= 1;
            }
        }
        self.ch2.timer = self.square_start(5, self.ch2.enabled);
        if self.ch2.enabled {
            self.ch2.out = DUTY[(self.regs[0x06] >> 6) as usize][self.ch2.pos as usize];
        }
        self.ch2.env.trigger(self.regs[0x07]);
        self.ch2.enabled = dac;
    }

    fn trigger_ch3(&mut self) {
        let dac = self.regs[0x0A] & 0x80 != 0;
        if self.ch3_len == 0 {
            self.ch3_len = 256;
            if self.regs[0x0E] & 0x40 != 0 && self.fs_odd() {
                self.ch3_len -= 1;
            }
        }
        // DMG quirk: retriggering right as the channel reads wave RAM
        // corrupts the first bytes.
        if !self.cgb && self.ch3_enabled && self.ch3_timer != NEVER && self.ch3_timer <= 2 {
            let p = (self.ch3_pos.wrapping_add(1) & 31) >> 1;
            if p < 4 {
                self.wave_ram[0] = self.wave_ram[p as usize];
            } else {
                let base = (p & !3) as usize;
                let (a, b) = self.wave_ram.split_at_mut(4);
                a.copy_from_slice(&b[base - 4..base]);
            }
        }
        self.ch3_pos = 0;
        self.ch3_timer = self.wave_period() + 6;
        self.ch3_enabled = dac;
    }

    fn trigger_ch4(&mut self) {
        let dac = self.regs[0x11] & 0xF8 != 0;
        if self.ch4_len == 0 {
            self.ch4_len = 64;
            if self.regs[0x13] & 0x40 != 0 && self.fs_odd() {
                self.ch4_len -= 1;
            }
        }
        self.ch4_timer = self.noise_period();
        self.ch4_env.trigger(self.regs[0x11]);
        self.ch4_lfsr = 0x7FFF;
        self.ch4_enabled = dac;
    }

    // ------------------------------------------------------------------ misc

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Count of mixed-output level changes since power-on; constant across an interval means silence/DC.
    pub fn level_changes(&self) -> u64 {
        self.level_changes
    }

    /// Tell the APU the state of the frame-sequencer DIV bit (called by the bus before NR52 writes).
    pub fn set_div_bit(&mut self, high: bool) {
        self.div_bit = high;
    }

    /// Select CGB (true) or DMG (false) APU behaviour.
    pub fn set_cgb(&mut self, cgb: bool) {
        self.cgb = cgb;
    }

    pub fn set_sample_rate(&mut self, hz: u32) {
        self.sample_rate = hz.max(1);
        self.phase = 0;
        self.acc_l = 0.0;
        self.acc_r = 0.0;
        self.acc_from = self.cycles;
        self.win_start = self.cycles;
        self.until_sample = self.samples_until_next();
        self.hp_charge = 0.999958f32.powf(CLOCK as f32 / self.sample_rate as f32);
    }

    /// Move generated samples (stereo interleaved L,R as f32 in -1.0..=1.0)
    /// into `out`.
    pub fn drain_samples(&mut self, out: &mut Vec<f32>) {
        out.append(&mut self.out);
    }
}

impl Default for Apu {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apu() -> Apu {
        let mut a = Apu::new();
        a.write(0xFF26, 0x00);
        a.write(0xFF26, 0x80);
        a
    }

    #[test]
    fn read_masks_after_power_cycle() {
        let a = apu();
        for (i, m) in READ_MASK.iter().enumerate().take(0x16) {
            assert_eq!(a.read(0xFF10 + i as u16), *m, "reg {:#x}", 0xFF10 + i);
        }
        assert_eq!(a.read(0xFF26), 0xF0);
    }

    #[test]
    fn registers_locked_while_off_but_wave_ram_and_length_writable() {
        let mut a = apu();
        a.write(0xFF26, 0);
        a.write(0xFF12, 0xF0);
        a.write(0xFF24, 0x77);
        a.write(0xFF30, 0x12);
        assert_eq!(a.read(0xFF12), 0);
        assert_eq!(a.read(0xFF24), 0);
        assert_eq!(a.read(0xFF30), 0x12);
        a.write(0xFF16, 0x3E); // length load while off
        assert_eq!(a.ch2.len, 2);
        a.write(0xFF26, 0x80);
        assert_eq!(a.ch2.len, 2, "length survives power cycle");
    }

    #[test]
    fn power_off_preserves_wave_ram_and_clears_regs() {
        let mut a = apu();
        a.write(0xFF30, 0xAB);
        a.write(0xFF24, 0x55);
        a.write(0xFF26, 0);
        a.write(0xFF26, 0x80);
        assert_eq!(a.read(0xFF24), 0);
        assert_eq!(a.read(0xFF30), 0xAB);
    }

    #[test]
    fn length_expiry_in_sequencer_steps() {
        let mut a = apu();
        a.write(0xFF17, 0xF0);
        a.write(0xFF16, 62); // len = 2
        a.write(0xFF19, 0xC0); // trigger + length enable
        assert_eq!(a.read(0xFF26) & 2, 2);
        a.frame_sequencer_step(); // step 0: clock -> 1
        assert_eq!(a.read(0xFF26) & 2, 2);
        a.frame_sequencer_step(); // 1
        a.frame_sequencer_step(); // 2: clock -> 0
        assert_eq!(a.read(0xFF26) & 2, 0);
    }

    #[test]
    fn enabling_length_in_first_half_clocks_extra() {
        let mut a = apu();
        a.write(0xFF17, 0xF0);
        a.write(0xFF16, 63); // len = 1
        a.write(0xFF19, 0x80);
        a.frame_sequencer_step(); // step 0 done; next doesn't clock length
        a.write(0xFF19, 0x40);
        assert_eq!(a.read(0xFF26) & 2, 0);
    }

    #[test]
    fn dac_off_disables_channel() {
        let mut a = apu();
        a.write(0xFF17, 0xF0);
        a.write(0xFF19, 0x80);
        assert_eq!(a.read(0xFF26) & 2, 2);
        a.write(0xFF17, 0x07);
        assert_eq!(a.read(0xFF26) & 2, 0);
        a.write(0xFF19, 0x80);
        assert_eq!(a.read(0xFF26) & 2, 0);
    }

    #[test]
    fn sweep_overflow_on_trigger_and_clock() {
        let mut a = apu();
        a.write(0xFF12, 0xF0);
        a.write(0xFF10, 0x01); // period 0, shift 1
        a.write(0xFF13, 0xFF);
        a.write(0xFF14, 0x87); // freq 0x7FF: overflow on trigger
        assert_eq!(a.read(0xFF26) & 1, 0);

        a.write(0xFF10, 0x11); // period 1, shift 1
        a.write(0xFF13, 0x00);
        a.write(0xFF14, 0x84); // freq 0x400
        assert_eq!(a.read(0xFF26) & 1, 1);
        for _ in 0..3 {
            a.frame_sequencer_step();
        } // step 2 clocks sweep: 0x400 + 0x200 = 0x600 ok, next calc 0x900 overflows
        assert_eq!(a.read(0xFF26) & 1, 0);
    }

    #[test]
    fn negate_clear_after_use_disables() {
        let mut a = apu();
        a.write(0xFF12, 0xF0);
        a.write(0xFF10, 0x19); // period 1, negate, shift 1
        a.write(0xFF14, 0x84);
        assert_eq!(a.read(0xFF26) & 1, 1);
        a.write(0xFF10, 0x11);
        assert_eq!(a.read(0xFF26) & 1, 0);
    }

    #[test]
    fn square_tone_frequency() {
        let mut a = apu();
        a.set_sample_rate(44_100);
        a.write(0xFF24, 0x77);
        a.write(0xFF25, 0x22); // ch2 both sides
        a.write(0xFF16, 0x80); // 50% duty
        a.write(0xFF17, 0xF0);
        // freq reg 1750 => period (2048-1750)*4*8 = 9536 T => ~439.8 Hz
        a.write(0xFF18, (1750 & 0xFF) as u8);
        a.write(0xFF19, 0x80 | (1750 >> 8) as u8);
        let secs = 2;
        for _ in 0..(CLOCK as u32 * secs / 4) {
            a.tick(4);
        }
        let mut out = Vec::new();
        a.drain_samples(&mut out);
        assert!((out.len() as i64 - 44_100 * 2 * secs as i64).abs() <= 4);
        let left: Vec<f32> = out.iter().step_by(2).copied().skip(4410).collect();
        let crossings = left.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        let secs_f = left.len() as f32 / 44_100.0;
        let hz = crossings as f32 / secs_f;
        let expect = CLOCK as f32 / 9536.0;
        assert!((hz - expect).abs() < 2.0, "hz {hz} expect {expect}");
        assert!(left.iter().all(|s| s.abs() <= 1.0));
    }

    #[test]
    fn pcm12_exposes_digital_square_output() {
        let mut a = Apu::new();
        a.set_cgb(true);
        a.write(0xFF26, 0x00);
        a.write(0xFF26, 0x80);
        a.write(0xFF16, 0x80); // 50% duty
        a.write(0xFF17, 0xF0); // volume 15
        a.write(0xFF18, 0xFC);
        a.write(0xFF19, 0x87); // period 16 T
        let mut seen = [false; 16];
        for _ in 0..64 {
            a.tick(4);
            let v = a.pcm12();
            assert_eq!(v & 0x0F, 0, "channel 1 is silent");
            seen[(v >> 4) as usize] = true;
        }
        assert!(seen[0] && seen[15], "output alternates between 0 and the volume");
        assert_eq!(a.pcm34(), 0);
    }

    #[test]
    fn powering_on_while_div_bit_is_high_skips_the_first_sequencer_event() {
        let run = |div_high: bool| {
            let mut a = Apu::new();
            a.write(0xFF26, 0x00);
            a.set_div_bit(div_high);
            a.write(0xFF26, 0x80);
            a.write(0xFF17, 0xF0);
            a.write(0xFF16, 63); // length 1
            a.write(0xFF19, 0xC0);
            a.frame_sequencer_step();
            a.read(0xFF26) & 2
        };
        assert_eq!(run(false), 0, "step 0 clocks the length");
        assert_eq!(run(true), 2, "the first event is skipped");
    }

    #[test]
    fn cgb_envelope_write_zombie_mode() {
        let mut a = Apu::new();
        a.set_cgb(true);
        a.write(0xFF26, 0x00);
        a.write(0xFF26, 0x80);
        a.write(0xFF17, 0x48); // volume 4, add mode, period 0
        a.write(0xFF19, 0x80);
        a.write(0xFF17, 0x49); // period 0 -> 1 while running: one extra tick (+1)
        assert_eq!(a.ch2.env.volume, 5);
        a.write(0xFF17, 0x41); // add -> subtract inverts: 16 - 5
        assert_eq!(a.ch2.env.volume, 11);
    }
}
