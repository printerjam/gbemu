//! `gbtest trace`: per-instruction CPU trace, optionally in Gameboy Doctor format.

use crate::{parse, usage_error, value};
use gb_core::bus::Bus;
use gb_core::cpu::CpuBus;
use gb_core::disasm::disassemble;
use gb_core::GameBoy;
use std::io::{BufWriter, Write};

/// Doctor mode: Gameboy Doctor's reference logs were made on an emulator whose LY (0xFF44)
/// always reads 0x90 (and whose IF read has no upper bits), so the CPU sees those values instead.
/// Implemented as a `CpuBus` wrapper; the core is untouched.
struct DoctorBus<'a>(&'a mut Bus);

impl CpuBus for DoctorBus<'_> {
    fn read(&mut self, addr: u16) -> u8 {
        let v = self.0.read(addr);
        match addr {
            0xFF44 => 0x90,
            // The reference emulator returns IF without the unused upper bits (hardware reads them as 1).
            0xFF0F => v & 0x1F,
            _ => v,
        }
    }
    fn write(&mut self, addr: u16, val: u8) {
        self.0.write(addr, val)
    }
    fn tick(&mut self) {
        self.0.tick()
    }
    fn pending_interrupts(&self) -> u8 {
        self.0.pending_interrupts()
    }
    fn ack_interrupt(&mut self, mask: u8) {
        self.0.ack_interrupt(mask)
    }
}

pub fn run(mut args: impl Iterator<Item = String>) {
    let mut rom: Option<String> = None;
    let (mut steps, mut doctor) = (1_000_000u64, false);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--steps" => steps = parse(&value(&mut args, &a), &a),
            "--doctor" => doctor = true,
            _ if !a.starts_with("--") && rom.is_none() => rom = Some(a),
            _ => usage_error(&format!("trace: unknown argument '{a}'")),
        }
    }
    let rom = rom.unwrap_or_else(|| usage_error("trace: missing <rom>"));
    let data = std::fs::read(&rom).unwrap_or_else(|e| usage_error(&format!("{rom}: {e}")));
    let mut gb = GameBoy::new(data).unwrap_or_else(|e| usage_error(&format!("{rom}: cartridge error {e:?}")));
    let mut out = BufWriter::new(std::io::stdout().lock());
    let mut executed = 0u64;
    let mut carry: Option<String> = None;
    while executed < steps {
        let r = gb.cpu.regs;
        let pc = r.pc;
        let bus = &gb.bus;
        let line = if doctor {
            let m = |i: u16| bus.peek(pc.wrapping_add(i));
            format!(
                "A:{:02X} F:{:02X} B:{:02X} C:{:02X} D:{:02X} E:{:02X} H:{:02X} L:{:02X} SP:{:04X} PC:{:04X} PCMEM:{:02X},{:02X},{:02X},{:02X}",
                r.a, r.f, r.b, r.c, r.d, r.e, r.h, r.l, r.sp, pc, m(0), m(1), m(2), m(3)
            )
        } else {
            let (text, len) = disassemble(|a| bus.peek(a), pc);
            let mut bytes = String::new();
            for i in 0..len as u16 {
                bytes.push_str(&format!("{:02X} ", bus.peek(pc.wrapping_add(i))));
            }
            format!(
                "{pc:04X}  {bytes:<9} {text:<18} A:{:02X} F:{:02X} B:{:02X} C:{:02X} D:{:02X} E:{:02X} H:{:02X} L:{:02X} SP:{:04X} IME:{} CY:{}",
                r.a, r.f, r.b, r.c, r.d, r.e, r.h, r.l, r.sp, gb.cpu.ime as u8, gb.bus.cycles
            )
        };
        // Interrupt dispatch and halted idle cycles are not instructions. Reference logs show the state
        // before the dispatch merged with the first handler instruction, so a non-instruction step
        // carries its pre-state line over to the next executed instruction.
        let ran = if doctor {
            gb.cpu.step(&mut DoctorBus(&mut gb.bus))
        } else {
            gb.step()
        };
        if ran.is_none() {
            carry.get_or_insert(line);
        } else {
            executed += 1;
            let line = carry.take().unwrap_or(line);
            if writeln!(out, "{line}").is_err() {
                return; // closed pipe (e.g. `| head`)
            }
        }
    }
    let _ = out.flush();
}
