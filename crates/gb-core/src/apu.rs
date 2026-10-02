//! APU: sound registers 0xFF10-0xFF26 and wave RAM 0xFF30-0xFF3F.

pub struct Apu {
    regs: [u8; 0x30],
    sample_rate: u32,
}

impl Apu {
    pub fn new() -> Self {
        Apu { regs: [0; 0x30], sample_rate: 48_000 }
    }

    /// Advance `t_cycles` T-cycles.
    pub fn tick(&mut self, t_cycles: u32) {
        let _ = t_cycles;
    }

    /// Frame sequencer clock (512 Hz): called by the bus on the falling edge
    /// of DIV-counter bit 12.
    pub fn frame_sequencer_step(&mut self) {}

    /// `addr` in 0xFF10..=0xFF3F.
    pub fn read(&self, addr: u16) -> u8 {
        self.regs[(addr - 0xFF10) as usize]
    }
    pub fn write(&mut self, addr: u16, val: u8) {
        self.regs[(addr - 0xFF10) as usize] = val;
    }

    pub fn set_sample_rate(&mut self, hz: u32) {
        self.sample_rate = hz;
    }

    /// Move generated samples (stereo interleaved L,R as f32 in -1.0..=1.0)
    /// into `out`.
    pub fn drain_samples(&mut self, out: &mut Vec<f32>) {
        let _ = out;
    }
}

impl Default for Apu {
    fn default() -> Self {
        Self::new()
    }
}
