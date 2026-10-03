//! Memory map, interrupt flags, OAM DMA, and the per-M-cycle clock that drives
//! every peripheral.

use crate::apu::Apu;
use crate::cartridge::Cartridge;
use crate::cpu::CpuBus;
use crate::joypad::Joypad;
use crate::oam_bug;
use crate::ppu::Ppu;
use crate::serial::Serial;
use crate::timer::Timer;
use crate::Model;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Dma {
    /// Last value written to 0xFF46.
    reg: u8,
    /// Transfer in progress: next byte index (0..160) and source base.
    active: Option<(u8, u16)>,
    /// Requested transfer and M-cycles until it takes over.
    pending: Option<(u8, u16)>,
}

/// CGB VRAM DMA (FF51-FF55).
#[derive(Serialize, Deserialize)]
struct Hdma {
    src: u16,
    dst: u16,
    /// Blocks remaining minus one (7 bits); 0x7F once finished.
    len: u8,
    /// HBlank transfer in progress.
    active: bool,
}

/// Internal divider value when the CGB boot ROM hands over (mooneye boot_div-cgbABCDE).
const CGB_POST_BOOT_DIV: u16 = 0x2674;

/// M-cycles a speed switch keeps the CPU stopped: 2^17 CPU clocks in either direction (age-cgb spsw-tima).
const SPEED_SWITCH_M_CYCLES: u32 = 0x8000;

#[derive(Serialize, Deserialize)]
pub struct Bus {
    pub cart: Cartridge,
    pub ppu: Ppu,
    pub apu: Apu,
    pub timer: Timer,
    pub serial: Serial,
    pub joypad: Joypad,
    /// Eight 4 KiB banks; bank 0 at 0xC000, `svbk` (1-7) at 0xD000.
    wram: Vec<u8>,
    svbk: u8,
    /// The divider has been written since power-on: from then on the model counter and the hardware counter agree,
    /// so the DMG serial-edge look-ahead no longer applies (gambatte serial/div_write_*).
    div_synced: bool,
    cgb: bool,
    double_speed: bool,
    /// KEY1 bit 0: speed switch armed for the next STOP.
    key1_armed: bool,
    /// Alternates in double speed so DMA/cartridge run every second M-cycle.
    ds_phase: bool,
    hdma: Hdma,
    /// FF56 (infrared port) and FF72-FF75 scratch registers.
    rp: u8,
    undoc: [u8; 4],
    #[serde(with = "crate::state::bytes")]
    hram: [u8; 0x7F],
    /// IF (0xFF0F), low 5 bits.
    pub int_flag: u8,
    /// IE (0xFFFF).
    pub int_enable: u8,
    dma: Dma,
    /// T-cycles elapsed since power-on.
    pub cycles: u64,
}

impl Bus {
    pub fn new(cart: Cartridge) -> Self {
        Self::with_ppu(cart, Ppu::new(), false)
    }

    /// CGB hardware; the cartridge decides between CGB mode and DMG compatibility mode.
    pub fn new_cgb(cart: Cartridge) -> Self {
        let ppu = Ppu::new_cgb(cart.header().cgb_supported());
        let mut bus = Self::with_ppu(cart, ppu, true);
        bus.timer = Timer::with_div(CGB_POST_BOOT_DIV);
        bus.apu.set_cgb(true);
        bus.serial.set_cgb(true);
        bus
    }

    pub fn model(&self) -> Model {
        if self.cgb {
            Model::Cgb
        } else {
            Model::Dmg
        }
    }

    pub fn double_speed(&self) -> bool {
        self.double_speed
    }

    fn with_ppu(cart: Cartridge, ppu: Ppu, cgb: bool) -> Self {
        Bus {
            cart,
            ppu,
            apu: Apu::new(),
            timer: Timer::new(),
            serial: Serial::new(),
            joypad: Joypad::new(),
            wram: vec![0; 0x8000],
            svbk: 1,
            div_synced: false,
            cgb,
            double_speed: false,
            key1_armed: false,
            ds_phase: false,
            hdma: Hdma {
                src: 0,
                dst: 0,
                len: 0x7F,
                active: false,
            },
            rp: 0,
            undoc: [0; 4],
            hram: [0; 0x7F],
            int_flag: 0x01,
            int_enable: 0x00,
            dma: Dma {
                reg: 0xFF,
                active: None,
                pending: None,
            },
            cycles: 0,
        }
    }

    /// Advance every peripheral by one M-cycle of the CPU clock, then let a pending
    /// HBlank DMA block steal the bus.
    pub fn tick_m(&mut self) {
        self.tick_cycle();
        let hblank = self.ppu.take_hblank();
        if hblank && self.hdma.active {
            self.hdma_block();
        }
    }

    /// One M-cycle of the CPU clock. In double speed the PPU/APU still run at the normal
    /// rate (2 dots per M-cycle); OAM DMA and the cartridge clock run every second M-cycle.
    fn tick_cycle(&mut self) {
        let dots = if self.double_speed { 2 } else { 4 };
        self.cycles += dots as u64;
        let div_before = self.timer.div_counter();
        let mut irqs = self.timer.tick();
        self.frame_sequencer_edge(div_before);
        // The divider as modelled here lags the hardware counter by one M-cycle on DMG (see Timer::new),
        // so the serial clock edge is taken 4 T ahead (mooneye serial/boot_sclk_align).
        let lag = if self.cgb || self.div_synced { 0 } else { 4 };
        irqs |= self
            .serial
            .clock(div_before.wrapping_add(lag), self.timer.div_counter().wrapping_add(lag));
        irqs |= self.ppu.tick(dots);
        self.apu.tick(dots);
        self.int_flag |= irqs;
        if self.double_speed {
            self.ds_phase = !self.ds_phase;
            if self.ds_phase {
                return;
            }
        }
        self.cart.tick();
        self.tick_dma();
    }

    /// APU frame sequencer steps on the falling edge of DIV bit 12 (bit 13 in double speed).
    fn frame_sequencer_edge(&mut self, div_before: u16) {
        let mask = if self.double_speed { 0x2000 } else { 0x1000 };
        if div_before & mask != 0 && self.timer.div_counter() & mask == 0 {
            self.apu.frame_sequencer_step();
        }
    }

    fn tick_dma(&mut self) {
        if let Some((index, src)) = self.dma.active {
            let val = self.dma_source_read(src + index as u16);
            self.ppu.write_oam_dma(index, val);
            self.dma.active = if index < 159 { Some((index + 1, src)) } else { None };
        }
        if let Some((delay, src)) = self.dma.pending {
            if delay == 0 {
                self.dma.pending = None;
                self.dma.active = Some((0, src));
            } else {
                self.dma.pending = Some((delay - 1, src));
            }
        }
    }

    /// Copy one 16-byte HDMA/GDMA block. The CPU is stalled for 8 M-cycles
    /// (16 in double speed) while the PPU and timers keep running.
    fn hdma_block(&mut self) {
        let m_cycles = if self.double_speed { 16 } else { 8 };
        for i in 0..m_cycles {
            self.tick_cycle();
            for j in 0..16 / m_cycles {
                let n = (i * (16 / m_cycles) + j) as u16;
                let val = self.peek(self.hdma.src.wrapping_add(n));
                self.ppu
                    .dma_write_vram(0x8000 | (self.hdma.dst.wrapping_add(n) & 0x1FFF), val);
            }
        }
        self.hdma.src = self.hdma.src.wrapping_add(16);
        self.hdma.dst = self.hdma.dst.wrapping_add(16) & 0x1FF0;
        self.hdma.len = self.hdma.len.wrapping_sub(1) & 0x7F;
        if self.hdma.len == 0x7F {
            self.hdma.active = false;
        }
        self.ppu.take_hblank();
    }

    fn write_hdma_control(&mut self, val: u8) {
        if self.hdma.active && val & 0x80 == 0 {
            // Cancel; the length bits take the written value.
            self.hdma.active = false;
            self.hdma.len = val & 0x7F;
            return;
        }
        self.hdma.len = val & 0x7F;
        if val & 0x80 == 0 {
            self.hdma.active = true;
            while self.hdma.active {
                self.hdma_block();
            }
        } else {
            self.hdma.active = true;
            if self.ppu.in_hblank() {
                self.hdma_block();
            }
        }
    }

    fn wram_index(&self, addr: u16) -> usize {
        let off = (addr & 0x1FFF) as usize;
        if off < 0x1000 {
            off
        } else {
            off - 0x1000 + (self.svbk.max(1) as usize) * 0x1000
        }
    }

    fn cgb_read(&self, addr: u16) -> u8 {
        if !self.cgb {
            return 0xFF;
        }
        let cgb_mode = self.cgb_mode();
        match addr {
            0xFF4F | 0xFF68..=0xFF6C => self.ppu.read_reg(addr),
            0xFF72 | 0xFF73 => self.undoc[(addr - 0xFF72) as usize],
            0xFF75 => 0x8F | self.undoc[3],
            0xFF76 | 0xFF77 => 0x00,
            // Unmapped in DMG compatibility mode.
            _ if !cgb_mode => 0xFF,
            0xFF4D => 0x7E | (self.double_speed as u8) << 7 | self.key1_armed as u8,
            0xFF55 => (!self.hdma.active as u8) << 7 | self.hdma.len,
            0xFF56 => self.rp | 0x3E,
            0xFF70 => 0xF8 | self.svbk,
            0xFF74 => self.undoc[2],
            _ => 0xFF,
        }
    }

    /// CGB hardware with a CGB cartridge (as opposed to DMG compatibility mode).
    fn cgb_mode(&self) -> bool {
        self.cgb && self.cart.header().cgb_supported()
    }

    fn cgb_write(&mut self, addr: u16, val: u8) {
        if !self.cgb {
            return;
        }
        let cgb_mode = self.cgb_mode();
        match addr {
            0xFF4F | 0xFF68..=0xFF6C => {
                self.ppu.write_reg(addr, val);
            }
            0xFF72 | 0xFF73 => self.undoc[(addr - 0xFF72) as usize] = val,
            0xFF75 => self.undoc[3] = val & 0x70,
            _ if !cgb_mode => {}
            0xFF4D => self.key1_armed = val & 1 != 0,
            0xFF51 => self.hdma.src = (self.hdma.src & 0x00FF) | (val as u16) << 8,
            0xFF52 => self.hdma.src = (self.hdma.src & 0xFF00) | (val & 0xF0) as u16,
            0xFF53 => self.hdma.dst = (self.hdma.dst & 0x00FF) | ((val & 0x1F) as u16) << 8,
            0xFF54 => self.hdma.dst = (self.hdma.dst & 0xFF00) | (val & 0xF0) as u16,
            0xFF55 => self.write_hdma_control(val),
            0xFF56 => self.rp = val & 0xC1,
            0xFF70 => self.svbk = val & 7,
            0xFF74 => self.undoc[2] = val,
            _ => {}
        }
    }

    /// STOP: perform an armed CGB speed switch.
    fn speed_switch(&mut self) {
        if !self.cgb || !self.key1_armed {
            return;
        }
        self.key1_armed = false;
        self.double_speed = !self.double_speed;
        self.ds_phase = false;
        self.timer.reset_div_speed_switch();
        self.int_flag |= self.timer.take_write_irq();
        for _ in 0..SPEED_SWITCH_M_CYCLES {
            self.tick_cycle();
        }
    }

    fn dma_source_read(&self, addr: u16) -> u8 {
        // Sources 0xE000-0xFFFF see the WRAM echo, not OAM/IO.
        let addr = if addr >= 0xE000 { addr - 0x2000 } else { addr };
        self.peek(addr)
    }

    fn dma_blocks_oam(&self) -> bool {
        self.dma.active.is_some()
    }

    /// Side-effect-free read (no clock advance); also used by debuggers.
    pub fn peek(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x7FFF => self.cart.read_rom(addr),
            0x8000..=0x9FFF => self.ppu.read_vram(addr),
            0xA000..=0xBFFF => self.cart.read_ram(addr),
            0xC000..=0xFDFF => self.wram[self.wram_index(addr)],
            0xFE00..=0xFE9F => {
                if self.dma_blocks_oam() {
                    0xFF
                } else {
                    self.ppu.read_oam(addr)
                }
            }
            0xFEA0..=0xFEFF => 0x00,
            0xFF00 => self.joypad.read(),
            0xFF01..=0xFF02 => self.serial.read(addr),
            0xFF04..=0xFF07 => self.timer.read(addr),
            0xFF0F => 0xE0 | self.int_flag,
            0xFF10..=0xFF3F => self.apu.read(addr),
            0xFF46 => self.dma.reg,
            0xFF40..=0xFF4B => self.ppu.read_reg(addr),
            0xFF4C..=0xFF7F => self.cgb_read(addr),
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize],
            0xFFFF => self.int_enable,
            _ => 0xFF,
        }
    }

    /// Write without clock advance.
    pub fn poke(&mut self, addr: u16, val: u8) {
        match addr {
            0x0000..=0x7FFF => self.cart.write_rom(addr, val),
            0x8000..=0x9FFF => self.ppu.write_vram(addr, val),
            0xA000..=0xBFFF => self.cart.write_ram(addr, val),
            0xC000..=0xFDFF => {
                let i = self.wram_index(addr);
                self.wram[i] = val;
            }
            0xFE00..=0xFE9F => {
                if !self.dma_blocks_oam() {
                    self.ppu.write_oam(addr, val)
                }
            }
            0xFF00 => self.joypad.write(val),
            0xFF01..=0xFF02 => self.serial.write(addr, val),
            0xFF04..=0xFF07 => {
                self.div_synced |= addr == 0xFF04;
                let before = self.timer.div_counter();
                self.timer.write(addr, val);
                self.frame_sequencer_edge(before);
                self.int_flag |= self.timer.take_write_irq();
                self.int_flag |= self.serial.clock(before, self.timer.div_counter());
            }
            0xFF0F => self.int_flag = val & 0x1F,
            0xFF10..=0xFF3F => self.apu.write(addr, val),
            0xFF46 => {
                self.dma.reg = val;
                self.dma.pending = Some((1, (val as u16) << 8));
            }
            0xFF40..=0xFF4B => {
                self.ppu.write_reg(addr, val);
                // Register writes can raise STAT immediately (LYC/STAT/LCDC).
                self.int_flag |= self.ppu.take_irq();
            }
            0xFF4C..=0xFF7F => self.cgb_write(addr, val),
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize] = val,
            0xFFFF => self.int_enable = val,
            _ => {}
        }
    }
}

impl Bus {
    /// OAM corruption bug: `addr` is on the address bus (access or IDU) in this M-cycle.
    fn oam_bug(&mut self, addr: u16, kind: oam_bug::Kind) {
        if (0xFE00..0xFF00).contains(&addr) {
            if let Some(row) = self.ppu.oam_scan_row() {
                oam_bug::corrupt(self.ppu.oam_mut(), row, kind);
            }
        }
    }
}

impl CpuBus for Bus {
    fn read(&mut self, addr: u16) -> u8 {
        self.tick_m();
        self.oam_bug(addr, oam_bug::Kind::Read);
        self.peek(addr)
    }

    fn read_idu(&mut self, addr: u16) -> u8 {
        self.tick_m();
        self.oam_bug(addr, oam_bug::Kind::ReadInc);
        self.peek(addr)
    }

    fn write(&mut self, addr: u16, val: u8) {
        self.tick_m();
        self.oam_bug(addr, oam_bug::Kind::Write);
        self.poke(addr, val);
    }

    fn double_speed(&self) -> bool {
        self.double_speed
    }

    fn tick_idu(&mut self, addr: u16) {
        self.tick_m();
        self.oam_bug(addr, oam_bug::Kind::Write);
    }

    fn tick(&mut self) {
        self.tick_m();
    }

    fn stop(&mut self) {
        self.speed_switch();
    }

    fn pending_interrupts(&self) -> u8 {
        self.int_enable & self.int_flag & 0x1F
    }

    fn ack_interrupt(&mut self, mask: u8) {
        self.int_flag &= !mask;
    }
}
