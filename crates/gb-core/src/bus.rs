//! Memory map, interrupt flags, OAM DMA, and the per-M-cycle clock that drives
//! every peripheral.

use crate::apu::Apu;
use crate::cartridge::Cartridge;
use crate::cpu::CpuBus;
use crate::joypad::Joypad;
use crate::ppu::Ppu;
use crate::serial::Serial;
use crate::timer::Timer;

struct Dma {
    /// Last value written to 0xFF46.
    reg: u8,
    /// Transfer in progress: next byte index (0..160) and source base.
    active: Option<(u8, u16)>,
    /// Requested transfer and M-cycles until it takes over.
    pending: Option<(u8, u16)>,
}

pub struct Bus {
    pub cart: Cartridge,
    pub ppu: Ppu,
    pub apu: Apu,
    pub timer: Timer,
    pub serial: Serial,
    pub joypad: Joypad,
    wram: Box<[u8; 0x2000]>,
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
        Bus {
            cart,
            ppu: Ppu::new(),
            apu: Apu::new(),
            timer: Timer::new(),
            serial: Serial::new(),
            joypad: Joypad::new(),
            wram: Box::new([0; 0x2000]),
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

    /// Advance every peripheral by one M-cycle (4 T-cycles).
    pub fn tick_m(&mut self) {
        self.cycles += 4;
        let div_before = self.timer.div_counter();
        let mut irqs = self.timer.tick();
        if div_before & 0x1000 != 0 && self.timer.div_counter() & 0x1000 == 0 {
            self.apu.frame_sequencer_step();
        }
        irqs |= self.serial.tick();
        irqs |= self.ppu.tick(4);
        self.apu.tick(4);
        self.cart.tick();
        self.int_flag |= irqs;
        self.tick_dma();
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
            0xC000..=0xDFFF => self.wram[(addr - 0xC000) as usize],
            0xE000..=0xFDFF => self.wram[(addr - 0xE000) as usize],
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
            0xC000..=0xDFFF => self.wram[(addr - 0xC000) as usize] = val,
            0xE000..=0xFDFF => self.wram[(addr - 0xE000) as usize] = val,
            0xFE00..=0xFE9F => {
                if !self.dma_blocks_oam() {
                    self.ppu.write_oam(addr, val)
                }
            }
            0xFF00 => self.joypad.write(val),
            0xFF01..=0xFF02 => self.serial.write(addr, val),
            0xFF04..=0xFF07 => self.timer.write(addr, val),
            0xFF0F => self.int_flag = val & 0x1F,
            0xFF10..=0xFF3F => self.apu.write(addr, val),
            0xFF46 => {
                self.dma.reg = val;
                self.dma.pending = Some((0, (val as u16) << 8));
            }
            0xFF40..=0xFF4B => self.ppu.write_reg(addr, val),
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize] = val,
            0xFFFF => self.int_enable = val,
            _ => {}
        }
    }
}

impl CpuBus for Bus {
    fn read(&mut self, addr: u16) -> u8 {
        self.tick_m();
        self.peek(addr)
    }

    fn write(&mut self, addr: u16, val: u8) {
        self.tick_m();
        self.poke(addr, val);
    }

    fn tick(&mut self) {
        self.tick_m();
    }

    fn pending_interrupts(&self) -> u8 {
        self.int_enable & self.int_flag & 0x1F
    }

    fn ack_interrupt(&mut self, mask: u8) {
        self.int_flag &= !mask;
    }
}
