//! SM83 CPU. Every memory access goes through [`CpuBus`], which advances the
//! rest of the machine by one M-cycle per access, so instruction timing falls
//! out of the access pattern.

use serde::{Deserialize, Serialize};

/// What the CPU sees of the machine. Implemented by [`crate::bus::Bus`];
/// tests may implement it with a flat-memory mock.
pub trait CpuBus {
    /// Advance one M-cycle, then read `addr`.
    fn read(&mut self, addr: u16) -> u8;
    /// Advance one M-cycle, then write `val` to `addr`.
    fn write(&mut self, addr: u16, val: u8);
    /// Advance one M-cycle without a memory access (internal delay).
    fn tick(&mut self);
    /// Like [`CpuBus::read`], but a 16-bit register holding `addr` is incremented/decremented in the
    /// same M-cycle (`ld a,[hli]`, opcode fetch, first read of `pop`). Only matters for the OAM bug.
    fn read_idu(&mut self, addr: u16) -> u8 {
        self.read(addr)
    }
    /// Like [`CpuBus::tick`], but the increment/decrement unit puts `addr` (the register value before
    /// the operation) on the address bus (`inc rr`, the `dec sp` of a push). Only matters for the OAM bug.
    fn tick_idu(&mut self, _addr: u16) {
        self.tick();
    }
    /// CGB double-speed mode is active (the interrupt acknowledge point is only calibrated for normal speed).
    fn double_speed(&self) -> bool {
        false
    }
    /// Called before every instruction, interrupt dispatch or halted idle cycle: the point where a pending
    /// HBlank DMA block takes the bus.
    fn instruction_boundary(&mut self) {}
    /// STOP executed. The bus performs a CGB speed switch if one is armed.
    fn stop(&mut self) {}
    /// `IE & IF & 0x1F`: interrupts requested and enabled.
    fn pending_interrupts(&self) -> u8;
    /// Clear the given bit(s) in IF (interrupt acknowledged by dispatch).
    fn ack_interrupt(&mut self, mask: u8);
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registers {
    pub a: u8,
    pub f: u8,
    pub b: u8,
    pub c: u8,
    pub d: u8,
    pub e: u8,
    pub h: u8,
    pub l: u8,
    pub sp: u16,
    pub pc: u16,
}

impl Registers {
    /// CGB (CPU-CGB) state after the boot ROM: `cgb_game` = cartridge uses CGB features,
    /// otherwise the boot ROM leaves the DMG-compatibility values.
    pub fn post_boot_cgb(cgb_game: bool) -> Self {
        Registers {
            a: 0x11,
            f: 0x80,
            b: 0x00,
            c: 0x00,
            d: if cgb_game { 0xFF } else { 0x00 },
            e: if cgb_game { 0x56 } else { 0x08 },
            h: 0x00,
            l: if cgb_game { 0x0D } else { 0x7C },
            sp: 0xFFFE,
            pc: 0x0100,
        }
    }

    /// DMG (CPU ABC) register state after the boot ROM hands over at 0x0100.
    pub fn post_boot_dmg() -> Self {
        Registers {
            a: 0x01,
            f: 0xB0,
            b: 0x00,
            c: 0x13,
            d: 0x00,
            e: 0xD8,
            h: 0x01,
            l: 0x4D,
            sp: 0xFFFE,
            pc: 0x0100,
        }
    }
}

const FZ: u8 = 0x80;
const FN: u8 = 0x40;
const FH: u8 = 0x20;
const FC: u8 = 0x10;

#[derive(Serialize, Deserialize)]
pub struct Cpu {
    pub regs: Registers,
    pub ime: bool,
    pub halted: bool,
    /// EI executed; IME turns on after the next instruction.
    ei_pending: bool,
    /// HALT with IME=0 and an interrupt pending: next fetch doesn't advance PC.
    halt_bug: bool,
    /// Illegal opcode executed: the CPU is hung.
    locked: bool,
}

impl Cpu {
    pub fn new() -> Self {
        Self::with_registers(Registers::post_boot_dmg())
    }

    pub fn with_registers(regs: Registers) -> Self {
        Cpu {
            regs,
            ime: false,
            halted: false,
            ei_pending: false,
            halt_bug: false,
            locked: false,
        }
    }

    /// Execute one instruction (or one interrupt dispatch, or one idle
    /// M-cycle while halted). Returns the opcode executed, `None` when no
    /// instruction ran (halted idle cycle or interrupt dispatch). CB-prefixed
    /// instructions return `Some(0xCB)`.
    pub fn step(&mut self, bus: &mut impl CpuBus) -> Option<u8> {
        bus.instruction_boundary();
        if self.locked {
            bus.tick();
            return None;
        }
        if self.halted {
            if bus.pending_interrupts() == 0 {
                bus.tick();
                return None;
            }
            self.halted = false;
        }
        if self.ime && bus.pending_interrupts() != 0 {
            self.dispatch_interrupt(bus);
            return None;
        }
        let enable_ime = self.ei_pending;
        self.ei_pending = false;
        let op = self.fetch(bus);
        self.execute(bus, op);
        if enable_ime && op != 0xF3 {
            self.ime = true;
        }
        Some(op)
    }

    fn dispatch_interrupt(&mut self, bus: &mut impl CpuBus) {
        self.ime = false;
        self.ei_pending = false;
        bus.tick();
        bus.tick_idu(self.regs.sp);
        // EI;HALT with an interrupt pending: the halt bug makes the return address the HALT itself.
        if self.halt_bug {
            self.halt_bug = false;
            self.regs.pc = self.regs.pc.wrapping_sub(1);
        }
        let pc = self.regs.pc;
        self.regs.sp = self.regs.sp.wrapping_sub(1);
        bus.write(self.regs.sp, (pc >> 8) as u8);
        // IE may have been modified by the push; re-evaluate which source wins.
        let pending = bus.pending_interrupts();
        let source = (pending != 0).then(|| pending.trailing_zeros() as u16);
        let vector = source.map_or(0x0000, |bit| 0x0040 + bit * 8);
        // Normal speed: the IF bit is cleared after the low byte went out, so a push landing on IF (SP=0xFF10)
        // cannot resurrect it (gambatte irq_precedence/late_if_via_sp_if, *_late_retrigger_2). In double speed
        // the early acknowledge matches *_ds_1 instead; the finer-grained ds acknowledge point is not modelled.
        let early = bus.double_speed();
        if let (Some(bit), true) = (source, early) {
            bus.ack_interrupt(1 << bit);
        }
        self.regs.sp = self.regs.sp.wrapping_sub(1);
        bus.write(self.regs.sp, pc as u8);
        if let (Some(bit), false) = (source, early) {
            bus.ack_interrupt(1 << bit);
        }
        bus.tick();
        self.regs.pc = vector;
    }

    fn fetch(&mut self, bus: &mut impl CpuBus) -> u8 {
        let v = if self.halt_bug {
            bus.read(self.regs.pc)
        } else {
            bus.read_idu(self.regs.pc)
        };
        if self.halt_bug {
            self.halt_bug = false;
        } else {
            self.regs.pc = self.regs.pc.wrapping_add(1);
        }
        v
    }

    fn fetch16(&mut self, bus: &mut impl CpuBus) -> u16 {
        let lo = self.fetch(bus) as u16;
        let hi = self.fetch(bus) as u16;
        lo | hi << 8
    }

    /// Internal cycle (`dec sp` on the address bus), then the two stack writes.
    fn push(&mut self, bus: &mut impl CpuBus, v: u16) {
        bus.tick_idu(self.regs.sp);
        self.regs.sp = self.regs.sp.wrapping_sub(1);
        bus.write(self.regs.sp, (v >> 8) as u8);
        self.regs.sp = self.regs.sp.wrapping_sub(1);
        bus.write(self.regs.sp, v as u8);
    }

    fn pop(&mut self, bus: &mut impl CpuBus) -> u16 {
        let lo = bus.read_idu(self.regs.sp) as u16;
        self.regs.sp = self.regs.sp.wrapping_add(1);
        let hi = bus.read(self.regs.sp) as u16;
        self.regs.sp = self.regs.sp.wrapping_add(1);
        lo | hi << 8
    }

    fn bc(&self) -> u16 {
        (self.regs.b as u16) << 8 | self.regs.c as u16
    }
    fn de(&self) -> u16 {
        (self.regs.d as u16) << 8 | self.regs.e as u16
    }
    fn hl(&self) -> u16 {
        (self.regs.h as u16) << 8 | self.regs.l as u16
    }
    fn set_bc(&mut self, v: u16) {
        self.regs.b = (v >> 8) as u8;
        self.regs.c = v as u8;
    }
    fn set_de(&mut self, v: u16) {
        self.regs.d = (v >> 8) as u8;
        self.regs.e = v as u8;
    }
    fn set_hl(&mut self, v: u16) {
        self.regs.h = (v >> 8) as u8;
        self.regs.l = v as u8;
    }

    /// 16-bit register by the usual encoding: 0=BC 1=DE 2=HL 3=SP.
    fn rr(&self, i: u8) -> u16 {
        match i {
            0 => self.bc(),
            1 => self.de(),
            2 => self.hl(),
            _ => self.regs.sp,
        }
    }
    fn set_rr(&mut self, i: u8, v: u16) {
        match i {
            0 => self.set_bc(v),
            1 => self.set_de(v),
            2 => self.set_hl(v),
            _ => self.regs.sp = v,
        }
    }

    /// 8-bit operand by encoding: B C D E H L (HL) A.
    fn get_r(&mut self, bus: &mut impl CpuBus, i: u8) -> u8 {
        match i {
            0 => self.regs.b,
            1 => self.regs.c,
            2 => self.regs.d,
            3 => self.regs.e,
            4 => self.regs.h,
            5 => self.regs.l,
            6 => bus.read(self.hl()),
            _ => self.regs.a,
        }
    }
    fn set_r(&mut self, bus: &mut impl CpuBus, i: u8, v: u8) {
        match i {
            0 => self.regs.b = v,
            1 => self.regs.c = v,
            2 => self.regs.d = v,
            3 => self.regs.e = v,
            4 => self.regs.h = v,
            5 => self.regs.l = v,
            6 => bus.write(self.hl(), v),
            _ => self.regs.a = v,
        }
    }

    fn cond(&self, i: u8) -> bool {
        match i {
            0 => self.regs.f & FZ == 0,
            1 => self.regs.f & FZ != 0,
            2 => self.regs.f & FC == 0,
            _ => self.regs.f & FC != 0,
        }
    }

    fn alu(&mut self, op: u8, v: u8) {
        let a = self.regs.a;
        let carry = (self.regs.f & FC != 0) as u8;
        match op {
            0 | 1 => {
                let c = if op == 1 { carry } else { 0 };
                let r = a as u16 + v as u16 + c as u16;
                let h = (a & 0xF) + (v & 0xF) + c > 0xF;
                self.regs.a = r as u8;
                self.regs.f = flags(r as u8 == 0, false, h, r > 0xFF);
            }
            2 | 3 | 7 => {
                let c = if op == 3 { carry } else { 0 };
                let r = (a as u16).wrapping_sub(v as u16).wrapping_sub(c as u16);
                let h = (a & 0xF) < (v & 0xF) + c;
                if op != 7 {
                    self.regs.a = r as u8;
                }
                self.regs.f = flags(r as u8 == 0, true, h, r > 0xFF);
            }
            4 => {
                self.regs.a = a & v;
                self.regs.f = flags(self.regs.a == 0, false, true, false);
            }
            5 => {
                self.regs.a = a ^ v;
                self.regs.f = flags(self.regs.a == 0, false, false, false);
            }
            _ => {
                self.regs.a = a | v;
                self.regs.f = flags(self.regs.a == 0, false, false, false);
            }
        }
    }

    fn inc8(&mut self, v: u8) -> u8 {
        let r = v.wrapping_add(1);
        self.regs.f = (self.regs.f & FC) | flags(r == 0, false, v & 0xF == 0xF, false);
        r
    }

    fn dec8(&mut self, v: u8) -> u8 {
        let r = v.wrapping_sub(1);
        self.regs.f = (self.regs.f & FC) | flags(r == 0, true, v & 0xF == 0, false);
        r
    }

    /// SP + signed immediate with the weird low-byte flags.
    fn sp_plus(&mut self, e: u8) -> u16 {
        let sp = self.regs.sp;
        let r = sp.wrapping_add(e as i8 as u16);
        self.regs.f = flags(
            false,
            false,
            (sp & 0xF) + (e as u16 & 0xF) > 0xF,
            (sp & 0xFF) + e as u16 > 0xFF,
        );
        r
    }

    fn execute(&mut self, bus: &mut impl CpuBus, op: u8) {
        match op {
            0x00 => {}
            0x10 => {
                // STOP: 2 bytes, no low-power state; on CGB it performs an armed speed switch.
                self.fetch(bus);
                bus.stop();
            }
            0x76 => {
                if bus.pending_interrupts() != 0 {
                    if !self.ime {
                        self.halt_bug = true;
                    }
                } else {
                    self.halted = true;
                }
            }
            0x01 | 0x11 | 0x21 | 0x31 => {
                let v = self.fetch16(bus);
                self.set_rr(op >> 4, v);
            }
            0x02 | 0x12 | 0x22 | 0x32 => {
                let addr = match op >> 4 {
                    0 => self.bc(),
                    1 => self.de(),
                    _ => self.hl(),
                };
                bus.write(addr, self.regs.a);
                match op >> 4 {
                    2 => self.set_hl(addr.wrapping_add(1)),
                    3 => self.set_hl(addr.wrapping_sub(1)),
                    _ => {}
                }
            }
            0x0A | 0x1A | 0x2A | 0x3A => {
                let addr = match op >> 4 {
                    0 => self.bc(),
                    1 => self.de(),
                    _ => self.hl(),
                };
                self.regs.a = if op >> 4 >= 2 {
                    bus.read_idu(addr)
                } else {
                    bus.read(addr)
                };
                match op >> 4 {
                    2 => self.set_hl(addr.wrapping_add(1)),
                    3 => self.set_hl(addr.wrapping_sub(1)),
                    _ => {}
                }
            }
            0x03 | 0x13 | 0x23 | 0x33 => {
                let i = op >> 4;
                bus.tick_idu(self.rr(i));
                self.set_rr(i, self.rr(i).wrapping_add(1));
            }
            0x0B | 0x1B | 0x2B | 0x3B => {
                let i = op >> 4;
                bus.tick_idu(self.rr(i));
                self.set_rr(i, self.rr(i).wrapping_sub(1));
            }
            0x09 | 0x19 | 0x29 | 0x39 => {
                bus.tick();
                let hl = self.hl();
                let v = self.rr(op >> 4);
                let r = hl as u32 + v as u32;
                self.regs.f = (self.regs.f & FZ) | flags(false, false, (hl & 0xFFF) + (v & 0xFFF) > 0xFFF, r > 0xFFFF);
                self.set_hl(r as u16);
            }
            0x04 | 0x0C | 0x14 | 0x1C | 0x24 | 0x2C | 0x34 | 0x3C => {
                let i = op >> 3;
                let v = self.get_r(bus, i);
                let r = self.inc8(v);
                self.set_r(bus, i, r);
            }
            0x05 | 0x0D | 0x15 | 0x1D | 0x25 | 0x2D | 0x35 | 0x3D => {
                let i = op >> 3;
                let v = self.get_r(bus, i);
                let r = self.dec8(v);
                self.set_r(bus, i, r);
            }
            0x06 | 0x0E | 0x16 | 0x1E | 0x26 | 0x2E | 0x36 | 0x3E => {
                let v = self.fetch(bus);
                self.set_r(bus, op >> 3, v);
            }
            0x07 => {
                let a = self.regs.a;
                self.regs.a = a.rotate_left(1);
                self.regs.f = flags(false, false, false, a & 0x80 != 0);
            }
            0x0F => {
                let a = self.regs.a;
                self.regs.a = a.rotate_right(1);
                self.regs.f = flags(false, false, false, a & 1 != 0);
            }
            0x17 => {
                let a = self.regs.a;
                self.regs.a = a << 1 | (self.regs.f & FC != 0) as u8;
                self.regs.f = flags(false, false, false, a & 0x80 != 0);
            }
            0x1F => {
                let a = self.regs.a;
                self.regs.a = a >> 1 | ((self.regs.f & FC != 0) as u8) << 7;
                self.regs.f = flags(false, false, false, a & 1 != 0);
            }
            0x27 => {
                let mut a = self.regs.a;
                let f = self.regs.f;
                let mut carry = f & FC != 0;
                if f & FN == 0 {
                    if carry || a > 0x99 {
                        a = a.wrapping_add(0x60);
                        carry = true;
                    }
                    if f & FH != 0 || a & 0xF > 9 {
                        a = a.wrapping_add(6);
                    }
                } else {
                    if carry {
                        a = a.wrapping_sub(0x60);
                    }
                    if f & FH != 0 {
                        a = a.wrapping_sub(6);
                    }
                }
                self.regs.a = a;
                self.regs.f = flags(a == 0, f & FN != 0, false, carry);
            }
            0x2F => {
                self.regs.a = !self.regs.a;
                self.regs.f |= FN | FH;
            }
            0x37 => self.regs.f = (self.regs.f & FZ) | FC,
            0x3F => self.regs.f = (self.regs.f & FZ) | ((self.regs.f & FC) ^ FC),
            0x08 => {
                let addr = self.fetch16(bus);
                bus.write(addr, self.regs.sp as u8);
                bus.write(addr.wrapping_add(1), (self.regs.sp >> 8) as u8);
            }
            0x18 | 0x20 | 0x28 | 0x30 | 0x38 => {
                let e = self.fetch(bus) as i8;
                if op == 0x18 || self.cond((op >> 3) & 3) {
                    bus.tick();
                    self.regs.pc = self.regs.pc.wrapping_add(e as u16);
                }
            }
            0x40..=0x7F => {
                let v = self.get_r(bus, op & 7);
                self.set_r(bus, (op >> 3) & 7, v);
            }
            0x80..=0xBF => {
                let v = self.get_r(bus, op & 7);
                self.alu((op >> 3) & 7, v);
            }
            0xC6 | 0xCE | 0xD6 | 0xDE | 0xE6 | 0xEE | 0xF6 | 0xFE => {
                let v = self.fetch(bus);
                self.alu((op >> 3) & 7, v);
            }
            0xC0 | 0xC8 | 0xD0 | 0xD8 => {
                bus.tick();
                if self.cond((op >> 3) & 3) {
                    self.regs.pc = self.pop(bus);
                    bus.tick();
                }
            }
            0xC9 | 0xD9 => {
                self.regs.pc = self.pop(bus);
                bus.tick();
                if op == 0xD9 {
                    self.ime = true;
                }
            }
            0xC1 | 0xD1 | 0xE1 => {
                let v = self.pop(bus);
                self.set_rr((op >> 4) & 3, v);
            }
            0xF1 => {
                let v = self.pop(bus);
                self.regs.a = (v >> 8) as u8;
                self.regs.f = v as u8 & 0xF0;
            }
            0xC5 | 0xD5 | 0xE5 => {
                let v = self.rr((op >> 4) & 3);
                self.push(bus, v);
            }
            0xF5 => {
                let v = (self.regs.a as u16) << 8 | self.regs.f as u16;
                self.push(bus, v);
            }
            0xC2 | 0xCA | 0xD2 | 0xDA => {
                let addr = self.fetch16(bus);
                if self.cond((op >> 3) & 3) {
                    bus.tick();
                    self.regs.pc = addr;
                }
            }
            0xC3 => {
                let addr = self.fetch16(bus);
                bus.tick();
                self.regs.pc = addr;
            }
            0xE9 => self.regs.pc = self.hl(),
            0xC4 | 0xCC | 0xD4 | 0xDC => {
                let addr = self.fetch16(bus);
                if self.cond((op >> 3) & 3) {
                    let pc = self.regs.pc;
                    self.push(bus, pc);
                    self.regs.pc = addr;
                }
            }
            0xCD => {
                let addr = self.fetch16(bus);
                let pc = self.regs.pc;
                self.push(bus, pc);
                self.regs.pc = addr;
            }
            0xC7 | 0xCF | 0xD7 | 0xDF | 0xE7 | 0xEF | 0xF7 | 0xFF => {
                let pc = self.regs.pc;
                self.push(bus, pc);
                self.regs.pc = (op & 0x38) as u16;
            }
            0xE0 => {
                let n = self.fetch(bus);
                bus.write(0xFF00 | n as u16, self.regs.a);
            }
            0xF0 => {
                let n = self.fetch(bus);
                self.regs.a = bus.read(0xFF00 | n as u16);
            }
            0xE2 => bus.write(0xFF00 | self.regs.c as u16, self.regs.a),
            0xF2 => self.regs.a = bus.read(0xFF00 | self.regs.c as u16),
            0xEA => {
                let addr = self.fetch16(bus);
                bus.write(addr, self.regs.a);
            }
            0xFA => {
                let addr = self.fetch16(bus);
                self.regs.a = bus.read(addr);
            }
            0xE8 => {
                let e = self.fetch(bus);
                bus.tick();
                bus.tick();
                self.regs.sp = self.sp_plus(e);
            }
            0xF8 => {
                let e = self.fetch(bus);
                bus.tick();
                let v = self.sp_plus(e);
                self.set_hl(v);
            }
            0xF9 => {
                bus.tick();
                self.regs.sp = self.hl();
            }
            0xF3 => {
                self.ime = false;
                self.ei_pending = false;
            }
            0xFB => self.ei_pending = true,
            0xCB => {
                let op = self.fetch(bus);
                self.execute_cb(bus, op);
            }
            // D3 DB DD E3 E4 EB EC ED F4 FC FD
            _ => self.locked = true,
        }
    }

    fn execute_cb(&mut self, bus: &mut impl CpuBus, op: u8) {
        let i = op & 7;
        let bit = (op >> 3) & 7;
        let v = self.get_r(bus, i);
        let carry_in = (self.regs.f & FC != 0) as u8;
        match op >> 6 {
            0 => {
                let (r, c) = match bit {
                    0 => (v.rotate_left(1), v & 0x80 != 0),
                    1 => (v.rotate_right(1), v & 1 != 0),
                    2 => (v << 1 | carry_in, v & 0x80 != 0),
                    3 => (v >> 1 | carry_in << 7, v & 1 != 0),
                    4 => (v << 1, v & 0x80 != 0),
                    5 => (((v as i8) >> 1) as u8, v & 1 != 0),
                    6 => (v.rotate_left(4), false),
                    _ => (v >> 1, v & 1 != 0),
                };
                self.regs.f = flags(r == 0, false, false, c);
                self.set_r(bus, i, r);
            }
            1 => {
                self.regs.f = (self.regs.f & FC) | flags(v & (1 << bit) == 0, false, true, false);
            }
            2 => self.set_r(bus, i, v & !(1 << bit)),
            _ => self.set_r(bus, i, v | (1 << bit)),
        }
    }
}

#[inline]
fn flags(z: bool, n: bool, h: bool, c: bool) -> u8 {
    (z as u8) << 7 | (n as u8) << 6 | (h as u8) << 5 | (c as u8) << 4
}

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Flat 64 KiB memory with IF/IE modelled; counts M-cycles.
    struct Mock {
        mem: Vec<u8>,
        iflag: u8,
        ie: u8,
        cycles: u32,
    }

    impl Mock {
        fn new(prog: &[(u16, &[u8])]) -> Self {
            let mut mem = vec![0u8; 0x10000];
            for (addr, bytes) in prog {
                mem[*addr as usize..*addr as usize + bytes.len()].copy_from_slice(bytes);
            }
            Mock {
                mem,
                iflag: 0,
                ie: 0,
                cycles: 0,
            }
        }
    }

    impl CpuBus for Mock {
        fn read(&mut self, addr: u16) -> u8 {
            self.cycles += 1;
            self.mem[addr as usize]
        }
        fn write(&mut self, addr: u16, val: u8) {
            self.cycles += 1;
            self.mem[addr as usize] = val;
        }
        fn tick(&mut self) {
            self.cycles += 1;
        }
        fn pending_interrupts(&self) -> u8 {
            self.iflag & self.ie & 0x1F
        }
        fn ack_interrupt(&mut self, mask: u8) {
            self.iflag &= !mask;
        }
    }

    /// `EI; HALT` with an interrupt already pending: EI's delay means the interrupt is taken right
    /// after HALT (halt bug), the return address is the HALT itself, so after the handler returns
    /// the CPU halts again (Pan Docs: "ei halt" halt-bug case).
    #[test]
    fn ei_halt_with_pending_interrupt_returns_to_halt() {
        let mut bus = Mock::new(&[
            (0x0100, &[0xFB, 0x76, 0x00, 0x00]), // EI; HALT; NOP; NOP
            (0x0040, &[0x04, 0xD9]),             // INC B; RETI
        ]);
        bus.ie = 0x01;
        bus.iflag = 0x01;
        let mut cpu = Cpu::new();
        cpu.regs.b = 0;
        cpu.regs.pc = 0x0100;
        cpu.regs.sp = 0xFFFE;
        assert_eq!(cpu.step(&mut bus), Some(0xFB));
        assert_eq!(cpu.step(&mut bus), Some(0x76));
        assert!(!cpu.halted, "pending interrupt: HALT does not halt");
        assert!(cpu.ime, "IME is on after the instruction following EI");
        // Dispatch pushes the address of the HALT, not the instruction after it.
        assert_eq!(cpu.step(&mut bus), None);
        assert_eq!(cpu.regs.pc, 0x0040);
        assert_eq!(bus.mem[0xFFFC], 0x01);
        assert_eq!(bus.mem[0xFFFD], 0x01);
        assert_eq!(cpu.step(&mut bus), Some(0x04));
        assert_eq!(cpu.step(&mut bus), Some(0xD9));
        assert_eq!(cpu.regs.pc, 0x0101);
        // Back at HALT with nothing pending: halts, handler never runs again.
        assert_eq!(cpu.step(&mut bus), Some(0x76));
        assert!(cpu.halted);
        for _ in 0..8 {
            assert_eq!(cpu.step(&mut bus), None);
        }
        assert_eq!(cpu.regs.b, 1);
        assert_eq!(cpu.regs.pc, 0x0102);
    }

    /// HALT with IME=0 and a pending interrupt: no halt, and the next opcode byte is read twice.
    #[test]
    fn halt_bug_repeats_next_byte() {
        let mut bus = Mock::new(&[(0x0100, &[0x76, 0x04, 0x00])]); // HALT; INC B; NOP
        bus.ie = 0x04;
        bus.iflag = 0x04;
        let mut cpu = Cpu::new();
        cpu.regs.b = 0;
        cpu.regs.pc = 0x0100;
        assert_eq!(cpu.step(&mut bus), Some(0x76));
        assert_eq!(cpu.step(&mut bus), Some(0x04));
        assert_eq!(cpu.step(&mut bus), Some(0x04));
        assert_eq!(cpu.regs.b, 2);
        assert_eq!(cpu.regs.pc, 0x0102);
    }
}
