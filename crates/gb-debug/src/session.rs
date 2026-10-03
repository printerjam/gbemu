//! Debugger state and execution control, independent of any UI.

use crate::command::{Command, Reg};
use gb_core::bus::Bus;
use gb_core::cpu::CpuBus;
use gb_core::disasm::disassemble;
use gb_core::{GameBoy, CYCLES_PER_FRAME};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufWriter, Write};

/// Why execution stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    Breakpoint(u16),
    Watch { addr: u16, old: u8, new: u8, pc: u16 },
    /// The requested step/next/ret/frame completed.
    Done,
    /// Cycle budget exhausted without hitting anything.
    Cap,
}

#[derive(Clone, Copy)]
enum Goal {
    Step,
    Over { target: u16, sp: u16 },
    Ret { sp: u16 },
    Frame,
    Run,
}

/// Gameboy Doctor logs assume LY always reads 0x90 (same as `gbtest trace --doctor`).
struct DoctorBus<'a>(&'a mut Bus);

impl CpuBus for DoctorBus<'_> {
    fn read(&mut self, addr: u16) -> u8 {
        let v = self.0.read(addr);
        if addr == 0xFF44 {
            0x90
        } else {
            v
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

const STEP_CAP: u64 = 4 * CYCLES_PER_FRAME as u64;
const LONG_CAP: u64 = 600 * CYCLES_PER_FRAME as u64;

pub struct Session {
    pub gb: GameBoy,
    pub breaks: BTreeSet<u16>,
    /// Watched address -> last seen value.
    pub watches: BTreeMap<u16, u8>,
    pub mem_addr: u16,
    pub quit: bool,
    trace: Option<BufWriter<File>>,
}

pub fn flags(f: u8) -> String {
    let c = |bit: u8, ch: char| if f & bit != 0 { ch } else { '-' };
    format!("{}{}{}{}", c(0x80, 'Z'), c(0x40, 'N'), c(0x20, 'H'), c(0x10, 'C'))
}

impl Session {
    pub fn new(gb: GameBoy) -> Self {
        Session {
            gb,
            breaks: BTreeSet::new(),
            watches: BTreeMap::new(),
            mem_addr: 0,
            quit: false,
            trace: None,
        }
    }

    pub fn pc(&self) -> u16 {
        self.gb.cpu.regs.pc
    }

    pub fn peek(&self, a: u16) -> u8 {
        self.gb.bus.peek(a)
    }

    pub fn tracing(&self) -> bool {
        self.trace.is_some()
    }

    /// `(text, len)` of the instruction at `addr`.
    pub fn disasm(&self, addr: u16) -> (String, u8) {
        disassemble(|a| self.peek(a), addr)
    }

    /// One formatted disassembly line, with breakpoint / PC markers.
    pub fn disasm_line(&self, addr: u16) -> String {
        let (text, len) = self.disasm(addr);
        let bytes: Vec<String> = (0..len as u16).map(|i| format!("{:02X}", self.peek(addr.wrapping_add(i)))).collect();
        let bp = if self.breaks.contains(&addr) { '*' } else { ' ' };
        let cur = if addr == self.pc() { '>' } else { ' ' };
        format!("{bp}{cur}{addr:04X}  {:<8} {text}", bytes.join(" "))
    }

    /// Address of the instruction ending right before `addr`, found by trial decoding.
    pub fn prev_instruction(&self, addr: u16) -> u16 {
        for back in 1..=3u16 {
            let start = addr.wrapping_sub(back);
            if self.disasm(start).1 as u16 == back {
                return start;
            }
        }
        addr.wrapping_sub(1)
    }

    /// Start address such that decoding forward lands exactly on `pc` after about `before` lines.
    pub fn disasm_start(&self, pc: u16, before: usize) -> u16 {
        for span in (before..=before * 3).rev() {
            let mut a = pc.wrapping_sub(span as u16);
            let start = a;
            let mut n = 0;
            while a != pc && n < span + 1 {
                a = a.wrapping_add(self.disasm(a).1 as u16);
                n += 1;
                if (a.wrapping_sub(start) as usize) > span {
                    break;
                }
            }
            if a == pc && n >= before.saturating_sub(2) {
                return start;
            }
        }
        pc
    }

    pub fn regs_lines(&self) -> Vec<String> {
        let r = &self.gb.cpu.regs;
        let p = |a| self.peek(a);
        vec![
            format!(
                "AF {:02X}{:02X}  BC {:02X}{:02X}  DE {:02X}{:02X}  HL {:02X}{:02X}",
                r.a, r.f, r.b, r.c, r.d, r.e, r.h, r.l
            ),
            format!("SP {:04X}  PC {:04X}  flags {}", r.sp, r.pc, flags(r.f)),
            format!(
                "IME {}  HALT {}  cycles {}",
                self.gb.cpu.ime as u8, self.gb.cpu.halted as u8, self.gb.cycles()
            ),
            format!(
                "LY {:02X}  STAT {:02X}  LCDC {:02X}  IF {:02X}  IE {:02X}",
                p(0xFF44),
                p(0xFF41),
                p(0xFF40),
                p(0xFF0F) & 0x1F,
                p(0xFFFF)
            ),
        ]
    }

    pub fn hex_lines(&self, addr: u16, len: u16) -> Vec<String> {
        (0..len.div_ceil(16))
            .map(|row| {
                let base = addr.wrapping_add(row * 16);
                let bytes: Vec<u8> = (0..16u16).map(|i| self.peek(base.wrapping_add(i))).collect();
                let hex: Vec<String> = bytes.iter().map(|b| format!("{b:02X}")).collect();
                let asc: String = bytes.iter().map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { '.' }).collect();
                format!("{base:04X}  {} {}  {asc}", hex[..8].join(" "), hex[8..].join(" "))
            })
            .collect()
    }

    pub fn serial_text(&self) -> String {
        String::from_utf8_lossy(self.gb.serial_output()).into_owned()
    }

    // ------------------------------------------------------------ execution

    /// One CPU step; returns the opcode if an instruction ran. Writes a trace line if tracing.
    fn step_cpu(&mut self) -> Option<u8> {
        if self.trace.is_none() {
            return self.gb.step();
        }
        let r = self.gb.cpu.regs;
        let bus = &self.gb.bus;
        let m = |i: u16| bus.peek(r.pc.wrapping_add(i));
        let line = format!(
            "A:{:02X} F:{:02X} B:{:02X} C:{:02X} D:{:02X} E:{:02X} H:{:02X} L:{:02X} SP:{:04X} PC:{:04X} PCMEM:{:02X},{:02X},{:02X},{:02X}",
            r.a, r.f, r.b, r.c, r.d, r.e, r.h, r.l, r.sp, r.pc, m(0), m(1), m(2), m(3)
        );
        let ran = self.gb.cpu.step(&mut DoctorBus(&mut self.gb.bus));
        if ran.is_some() {
            if let Some(t) = &mut self.trace {
                let _ = writeln!(t, "{line}");
            }
        }
        ran
    }

    fn exec(&mut self, goal: Goal, budget: u64) -> Stop {
        let start = self.gb.cycles();
        loop {
            if self.gb.cycles() - start >= budget {
                return if matches!(goal, Goal::Frame) { Stop::Done } else { Stop::Cap };
            }
            let sp_before = self.gb.cpu.regs.sp;
            let pc_before = self.pc();
            let op = self.step_cpu();
            let pc = self.pc();
            for (&addr, old) in self.watches.iter_mut() {
                let new = self.gb.bus.peek(addr);
                if new != *old {
                    let stop = Stop::Watch { addr, old: *old, new, pc: pc_before };
                    *old = new;
                    // Refresh the remaining watches so one stop reports one change set.
                    return self.refresh_watches(stop);
                }
            }
            let done = match goal {
                Goal::Step => op.is_some(),
                Goal::Over { target, sp } => pc == target && self.gb.cpu.regs.sp == sp,
                Goal::Ret { sp } => {
                    let sp_after = self.gb.cpu.regs.sp;
                    matches!(op, Some(0xC9 | 0xD9 | 0xC0 | 0xC8 | 0xD0 | 0xD8))
                        && sp_after == sp_before.wrapping_add(2)
                        && sp_after > sp
                }
                Goal::Frame | Goal::Run => false,
            };
            if done {
                return Stop::Done;
            }
            if self.breaks.contains(&pc) {
                return Stop::Breakpoint(pc);
            }
        }
    }

    fn refresh_watches(&mut self, stop: Stop) -> Stop {
        let bus = &self.gb.bus;
        for (&a, v) in self.watches.iter_mut() {
            *v = bus.peek(a);
        }
        stop
    }

    /// Run one UI chunk (a frame's worth of cycles) for continue mode.
    pub fn run_chunk(&mut self) -> Stop {
        self.exec(Goal::Run, CYCLES_PER_FRAME as u64)
    }

    pub fn step(&mut self) -> Stop {
        self.exec(Goal::Step, STEP_CAP)
    }

    pub fn step_over(&mut self) -> Stop {
        let pc = self.pc();
        let op = self.peek(pc);
        let len = match op {
            0xCD | 0xC4 | 0xCC | 0xD4 | 0xDC => 3,
            _ if op & 0xC7 == 0xC7 => 1,
            _ => return self.step(),
        };
        let goal = Goal::Over { target: pc.wrapping_add(len), sp: self.gb.cpu.regs.sp };
        self.exec(goal, LONG_CAP)
    }

    pub fn run_to_ret(&mut self) -> Stop {
        let goal = Goal::Ret { sp: self.gb.cpu.regs.sp };
        self.exec(goal, LONG_CAP)
    }

    pub fn run_frame(&mut self) -> Stop {
        self.exec(Goal::Frame, CYCLES_PER_FRAME as u64)
    }

    pub fn describe(&self, stop: &Stop) -> String {
        let at = format!("{}", self.disasm_line(self.pc()).trim_start_matches([' ', '*', '>']));
        match stop {
            Stop::Breakpoint(a) => format!("breakpoint hit at ${a:04X}: {at}"),
            Stop::Watch { addr, old, new, pc } => {
                format!("watch ${addr:04X} changed {old:02X} -> {new:02X} (written by instruction at ${pc:04X}) next: {at}")
            }
            Stop::Done => format!("stopped: {at}"),
            Stop::Cap => format!("cycle budget exhausted: {at}"),
        }
    }

    // ------------------------------------------------------------- commands

    fn set_reg(&mut self, reg: Reg, v: u16) {
        let r = &mut self.gb.cpu.regs;
        let (hi, lo) = ((v >> 8) as u8, v as u8);
        match reg {
            Reg::A => r.a = lo,
            Reg::F => r.f = lo & 0xF0,
            Reg::B => r.b = lo,
            Reg::C => r.c = lo,
            Reg::D => r.d = lo,
            Reg::E => r.e = lo,
            Reg::H => r.h = lo,
            Reg::L => r.l = lo,
            Reg::Af => (r.a, r.f) = (hi, lo & 0xF0),
            Reg::Bc => (r.b, r.c) = (hi, lo),
            Reg::De => (r.d, r.e) = (hi, lo),
            Reg::Hl => (r.h, r.l) = (hi, lo),
            Reg::Sp => r.sp = v,
            Reg::Pc => r.pc = v,
            Reg::Ime => self.gb.cpu.ime = v != 0,
        }
    }

    pub fn toggle_break(&mut self, addr: u16) -> bool {
        if !self.breaks.remove(&addr) {
            self.breaks.insert(addr);
            true
        } else {
            false
        }
    }

    /// Execute a command and return its output lines. `continue` runs to a stop or a frame cap.
    pub fn run_command(&mut self, cmd: Command) -> Vec<String> {
        match cmd {
            Command::Break(a) => {
                self.breaks.insert(a);
                vec![format!("breakpoint set at ${a:04X}")]
            }
            Command::Delete(a) => vec![if self.breaks.remove(&a) {
                format!("breakpoint at ${a:04X} removed")
            } else {
                format!("no breakpoint at ${a:04X}")
            }],
            Command::Watch(a) => {
                let v = self.peek(a);
                self.watches.insert(a, v);
                vec![format!("watching ${a:04X} (now {v:02X})")]
            }
            Command::Unwatch(a) => vec![if self.watches.remove(&a).is_some() {
                format!("watch ${a:04X} removed")
            } else {
                format!("no watch at ${a:04X}")
            }],
            Command::Set(r, v) => {
                self.set_reg(r, v);
                self.regs_lines()
            }
            Command::Poke(a, v) => {
                self.gb.bus.poke(a, v);
                // A debugger poke is not a program-visible change.
                if let Some(w) = self.watches.get_mut(&a) {
                    *w = self.gb.bus.peek(a);
                }
                vec![format!("${a:04X} = {:02X}", self.peek(a))]
            }
            Command::Trace(Some(path)) => match File::create(&path) {
                Ok(f) => {
                    self.trace = Some(BufWriter::new(f));
                    vec![format!("tracing (Gameboy Doctor format) to {path}")]
                }
                Err(e) => vec![format!("cannot open {path}: {e}")],
            },
            Command::Trace(None) => {
                if let Some(mut t) = self.trace.take() {
                    let _ = t.flush();
                }
                vec!["trace off".into()]
            }
            Command::Step(n) => {
                let mut stop = Stop::Done;
                for _ in 0..n.max(1) {
                    stop = self.step();
                    if stop != Stop::Done {
                        break;
                    }
                }
                vec![self.describe(&stop)]
            }
            Command::Next => {
                let s = self.step_over();
                vec![self.describe(&s)]
            }
            Command::Frame => {
                let s = self.run_frame();
                vec![self.describe(&s)]
            }
            Command::Continue(frames) => {
                let cap = frames.map_or(3000, |f| f.max(1)) as u64 * CYCLES_PER_FRAME as u64;
                let s = self.exec(Goal::Run, cap);
                vec![self.describe(&s)]
            }
            Command::Ret => {
                let s = self.run_to_ret();
                vec![self.describe(&s)]
            }
            Command::Regs => self.regs_lines(),
            Command::Examine(a, len) => {
                let a = a.unwrap_or(self.mem_addr);
                self.hex_lines(a, len)
            }
            Command::Disasm(a, n) => {
                let mut a = a.unwrap_or(self.pc());
                (0..n)
                    .map(|_| {
                        let l = self.disasm_line(a);
                        a = a.wrapping_add(self.disasm(a).1 as u16);
                        l
                    })
                    .collect()
            }
            Command::Goto(a) => {
                self.mem_addr = a & 0xFFF0;
                vec![format!("memory view at ${:04X}", self.mem_addr)]
            }
            Command::Serial => vec![self.serial_text()],
            Command::Breaks => {
                let mut v: Vec<String> = self.breaks.iter().map(|a| format!("break ${a:04X}")).collect();
                v.extend(self.watches.iter().map(|(a, val)| format!("watch ${a:04X} (={val:02X})")));
                if v.is_empty() {
                    v.push("no breakpoints or watches".into());
                }
                v
            }
            Command::Help => HELP.lines().map(String::from).collect(),
            Command::Quit => {
                self.quit = true;
                vec![]
            }
        }
    }
}

pub const HELP: &str = "commands: break A | delete A | watch A | unwatch A | set REG=V | poke A V | trace on [F]|off
          step [N] | next | frame | continue [FRAMES] | ret | regs | x [A] [LEN] | dis [A] [N]
          goto A | serial | breaks | quit        (numbers: $HEX, 0xHEX or decimal)
keys: s step  n over  f frame  Space run/stop  r run-to-ret  b toggle bp at cursor  w watch  g goto
      Up/Down cursor  PgUp/PgDn memory  : command  q quit";
