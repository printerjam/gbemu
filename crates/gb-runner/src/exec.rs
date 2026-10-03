//! Running a single test ROM to a verdict.

use crate::image::{diff_pixels, load_png, Image};
use crate::suites::{Detect, GambatteExpect, TestCase};
use gb_core::{GameBoy, CLOCK_HZ, CYCLES_PER_FRAME, SCREEN_HEIGHT, SCREEN_WIDTH};
use std::cell::RefCell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Pass,
    Fail(String),
}

pub struct TestResult {
    pub outcome: Outcome,
    pub elapsed: Duration,
    /// Final framebuffer, kept for failing and screenshot-based tests.
    pub framebuffer: Option<Vec<u32>>,
}

thread_local! {
    static PANIC_INFO: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Install a panic hook that records the message per-thread instead of printing it.
pub fn install_quiet_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "panic".to_string()
        };
        let loc = info
            .location()
            .map(|l| format!(" ({}:{})", l.file(), l.line()))
            .unwrap_or_default();
        PANIC_INFO.with(|p| *p.borrow_mut() = format!("{msg}{loc}"));
    }));
}

pub fn run_case(case: &TestCase, wall_limit: Duration) -> TestResult {
    let start = Instant::now();
    let mut fb_out = None;
    let r = catch_unwind(AssertUnwindSafe(|| execute(case, wall_limit, &mut fb_out)));
    let outcome = match r {
        Ok(o) => o,
        Err(_) => Outcome::Fail(format!("panic: {}", PANIC_INFO.with(|p| p.borrow().clone()))),
    };
    let keep =
        matches!(outcome, Outcome::Fail(_)) || matches!(case.detect, Detect::Acid2 { .. } | Detect::Screen { .. });
    TestResult {
        outcome,
        elapsed: start.elapsed(),
        framebuffer: if keep { fb_out } else { None },
    }
}

fn execute(case: &TestCase, wall_limit: Duration, fb_out: &mut Option<Vec<u32>>) -> Outcome {
    let rom = match std::fs::read(&case.rom) {
        Ok(r) => r,
        Err(e) => return Outcome::Fail(format!("read {}: {e}", case.rom.display())),
    };
    let mut gb = match GameBoy::with_model_choice(rom, case.model) {
        Ok(g) => g,
        Err(e) => return Outcome::Fail(format!("cartridge: {e:?}")),
    };
    let limit = (case.seconds * CLOCK_HZ as f64) as u64;
    let outcome = match &case.detect {
        Detect::Blargg { reference } => run_blargg(&mut gb, limit, wall_limit, reference.as_deref()),
        Detect::Mooneye => run_mooneye(&mut gb, limit, wall_limit),
        Detect::Acid2 { reference } => run_acid2(&mut gb, limit, wall_limit, reference),
        Detect::Screen { reference, buttons } => run_screen(&mut gb, limit, wall_limit, reference, buttons),
        Detect::Microtest => run_microtest(&mut gb, limit, wall_limit),
        Detect::MooneyeEd => run_mooneye_ed(&mut gb, limit, wall_limit),
        Detect::Gambatte(expect) => run_gambatte(&mut gb, expect),
    };
    *fb_out = Some(gb.framebuffer().to_vec());
    outcome
}

pub fn serial_tail(gb: &GameBoy) -> String {
    let s = String::from_utf8_lossy(gb.serial_output());
    let s: String = s
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    let s = s.trim();
    let chars: Vec<char> = s.chars().collect();
    let start = chars.len().saturating_sub(60);
    chars[start..].iter().collect()
}

/// Calls `step` for every CPU step, `check` once per frame of emulated time. Returns early with the first
/// `Some` from either.
fn drive<T>(
    gb: &mut GameBoy,
    limit: u64,
    wall_limit: Duration,
    mut on_step: impl FnMut(&mut GameBoy, Option<u8>) -> Option<T>,
    mut per_frame: impl FnMut(&mut GameBoy) -> Option<T>,
) -> Result<T, &'static str> {
    let wall = Instant::now();
    let mut next = gb.cycles() + CYCLES_PER_FRAME as u64;
    loop {
        let op = gb.step();
        if let Some(v) = on_step(gb, op) {
            return Ok(v);
        }
        if gb.cycles() >= next {
            next = gb.cycles() + CYCLES_PER_FRAME as u64;
            if let Some(v) = per_frame(gb) {
                return Ok(v);
            }
            if gb.cycles() >= limit {
                return Err("timeout");
            }
            if wall.elapsed() > wall_limit {
                return Err("wall-clock timeout");
            }
        }
    }
}

fn timeout_msg(gb: &GameBoy, kind: &str) -> String {
    format!(
        "{kind} after {:.1}s emulated, serial: \"{}\"",
        gb.cycles() as f64 / CLOCK_HZ as f64,
        serial_tail(gb)
    )
}

fn blargg_memory_result(gb: &GameBoy) -> Option<Outcome> {
    let bus = &gb.bus;
    if bus.peek(0xA001) != 0xDE || bus.peek(0xA002) != 0xB0 || bus.peek(0xA003) != 0x61 {
        return None;
    }
    let code = bus.peek(0xA000);
    if code == 0x80 {
        return None; // still running
    }
    if code == 0 {
        return Some(Outcome::Pass);
    }
    let text: String = (0xA004u16..0xA004 + 200)
        .map(|a| bus.peek(a))
        .take_while(|&b| b != 0)
        .map(|b| {
            if b.is_ascii_graphic() || b == b' ' {
                b as char
            } else {
                ' '
            }
        })
        .collect();
    Some(Outcome::Fail(format!("code {code:02X}: {}", text.trim())))
}

fn run_blargg(gb: &mut GameBoy, limit: u64, wall: Duration, reference: Option<&std::path::Path>) -> Outcome {
    let reference: Option<Image> = reference.and_then(|p| load_png(p).ok());
    let mut seen = 0usize;
    let mut frames = 0u32;
    let r = drive(
        gb,
        limit,
        wall,
        |_, _| None,
        |gb| {
            frames += 1;
            let out = gb.serial_output();
            if out.len() != seen {
                seen = out.len();
                let s = String::from_utf8_lossy(out);
                if s.contains("Passed") {
                    return Some(Outcome::Pass);
                }
                if s.contains("Failed") {
                    return Some(Outcome::Fail(format!("serial: \"{}\"", serial_tail(gb))));
                }
            }
            if let Some(o) = blargg_memory_result(gb) {
                return Some(o);
            }
            if let Some(img) = &reference {
                if frames.is_multiple_of(10) && diff_pixels(gb.framebuffer(), img, SCREEN_WIDTH, SCREEN_HEIGHT) == 0 {
                    return Some(Outcome::Pass);
                }
            }
            None
        },
    );
    match r {
        Ok(o) => o,
        Err(kind) => {
            if let Some(img) = &reference {
                let d = diff_pixels(gb.framebuffer(), img, SCREEN_WIDTH, SCREEN_HEIGHT);
                if d == 0 {
                    return Outcome::Pass;
                }
                return Outcome::Fail(format!("{} ({d} px differ)", timeout_msg(gb, kind)));
            }
            Outcome::Fail(timeout_msg(gb, kind))
        }
    }
}

fn run_mooneye(gb: &mut GameBoy, limit: u64, wall: Duration) -> Outcome {
    let r = drive(
        gb,
        limit,
        wall,
        |gb, op| (op == Some(0x40)).then(|| mooneye_verdict(gb)),
        |_| None,
    );
    match r {
        Ok(o) => o,
        Err(kind) => Outcome::Fail(timeout_msg(gb, kind)),
    }
}

fn mooneye_verdict(gb: &GameBoy) -> Outcome {
    let r = gb.registers();
    let got = [r.b, r.c, r.d, r.e, r.h, r.l];
    if got == [3, 5, 8, 13, 21, 34] {
        Outcome::Pass
    } else {
        Outcome::Fail(format!(
            "regs B,C,D,E,H,L = {}",
            got.iter().map(|v| format!("{v:02X}")).collect::<Vec<_>>().join(" ")
        ))
    }
}

fn run_acid2(gb: &mut GameBoy, limit: u64, wall: Duration, reference: &std::path::Path) -> Outcome {
    let img = match load_png(reference) {
        Ok(i) => i,
        Err(e) => return Outcome::Fail(e),
    };
    let r = drive(gb, limit, wall, |_, op| (op == Some(0x40)).then_some(()), |_| None);
    if let Err(kind) = r {
        return Outcome::Fail(timeout_msg(gb, kind));
    }
    for _ in 0..5 {
        gb.run_frame();
    }
    match diff_pixels(gb.framebuffer(), &img, SCREEN_WIDTH, SCREEN_HEIGHT) {
        0 => Outcome::Pass,
        d => Outcome::Fail(format!("{d} px differ")),
    }
}

fn run_screen(
    gb: &mut GameBoy,
    limit: u64,
    wall: Duration,
    reference: &std::path::Path,
    buttons: &[gb_core::Button],
) -> Outcome {
    let img = match load_png(reference) {
        Ok(i) => i,
        Err(e) => return Outcome::Fail(e),
    };
    let mut frame = 0u64;
    let r = drive(
        gb,
        limit,
        wall,
        |_, _| None,
        |gb| {
            frame += 1;
            if frame >= 30 {
                let t = frame - 30;
                if let Some(&b) = buttons.get((t / 10) as usize) {
                    match t % 10 {
                        0 => gb.set_button(b, true),
                        5 => gb.set_button(b, false),
                        _ => {}
                    }
                }
            }
            let done = frame >= 30 + 10 * buttons.len() as u64 + 10;
            (done && diff_pixels(gb.framebuffer(), &img, SCREEN_WIDTH, SCREEN_HEIGHT) == 0).then_some(Outcome::Pass)
        },
    );
    match r {
        Ok(o) => o,
        Err(kind) => {
            let d = diff_pixels(gb.framebuffer(), &img, SCREEN_WIDTH, SCREEN_HEIGHT);
            Outcome::Fail(format!("{kind} after {frame} frames, {d} px differ"))
        }
    }
}

/// GBMicrotest: 0xFF82 holds 0x01 on pass and 0xFF on fail.
fn run_microtest(gb: &mut GameBoy, limit: u64, wall: Duration) -> Outcome {
    let r = drive(
        gb,
        limit,
        wall,
        |_, _| None,
        |gb| match gb.bus.peek(0xFF82) {
            0x01 => Some(Outcome::Pass),
            0xFF => Some(Outcome::Fail(format!(
                "result {:02X}, expected {:02X}",
                gb.bus.peek(0xFF80),
                gb.bus.peek(0xFF81)
            ))),
            _ => None,
        },
    );
    match r {
        Ok(o) => o,
        Err(kind) => Outcome::Fail(format!("{kind}: no result at 0xFF82")),
    }
}

/// wilbertpol mooneye: the ROM finishes by executing the undefined opcode 0xED.
fn run_mooneye_ed(gb: &mut GameBoy, limit: u64, wall: Duration) -> Outcome {
    let r = drive(
        gb,
        limit,
        wall,
        |gb, op| (op == Some(0xED)).then(|| mooneye_verdict(gb)),
        |_| None,
    );
    match r {
        Ok(o) => o,
        Err(kind) => Outcome::Fail(timeout_msg(gb, kind)),
    }
}

/// Glyphs of gambatte's hex result display: one byte per row, bit 7 = leftmost pixel, set = black.
const HEX_TILES: [[u8; 8]; 16] = [
    [0x00, 0x7F, 0x41, 0x41, 0x41, 0x41, 0x41, 0x7F],
    [0x00, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08],
    [0x00, 0x7F, 0x01, 0x01, 0x7F, 0x40, 0x40, 0x7F],
    [0x00, 0x7F, 0x01, 0x01, 0x3F, 0x01, 0x01, 0x7F],
    [0x00, 0x41, 0x41, 0x41, 0x7F, 0x01, 0x01, 0x01],
    [0x00, 0x7F, 0x40, 0x40, 0x7E, 0x01, 0x01, 0x7E],
    [0x00, 0x7F, 0x40, 0x40, 0x7F, 0x41, 0x41, 0x7F],
    [0x00, 0x7F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x10],
    [0x00, 0x3E, 0x41, 0x41, 0x3E, 0x41, 0x41, 0x3E],
    [0x00, 0x7F, 0x41, 0x41, 0x7F, 0x01, 0x01, 0x7F],
    [0x00, 0x08, 0x22, 0x41, 0x7F, 0x41, 0x41, 0x41],
    [0x00, 0x7E, 0x41, 0x41, 0x7E, 0x41, 0x41, 0x7E],
    [0x00, 0x3E, 0x41, 0x40, 0x40, 0x40, 0x41, 0x3E],
    [0x00, 0x7E, 0x41, 0x41, 0x41, 0x41, 0x41, 0x7E],
    [0x00, 0x7F, 0x40, 0x40, 0x7F, 0x40, 0x40, 0x7F],
    [0x00, 0x7F, 0x40, 0x40, 0x7F, 0x40, 0x40, 0x40],
];

pub fn hex_matches(fb: &[u32], digits: &str) -> bool {
    digits.chars().enumerate().all(|(i, c)| {
        let tile = &HEX_TILES[c.to_digit(16).unwrap_or(0) as usize];
        (0..8).all(|y| {
            (0..8).all(|x| {
                // Gambatte compares with the low three bits of each channel masked off.
                let px = fb[y * SCREEN_WIDTH + i * 8 + x] & 0xF8F8F8;
                px == if tile[y] & (0x80 >> x) != 0 { 0 } else { 0xF8F8F8 }
            })
        })
    })
}

/// Gambatte runs every ROM for 16 frames; audio tests look at output changes during the last one.
fn run_gambatte(gb: &mut GameBoy, expect: &GambatteExpect) -> Outcome {
    let frame = CYCLES_PER_FRAME as u64;
    while gb.cycles() < 15 * frame {
        gb.step();
    }
    let before = gb.bus.apu.level_changes();
    while gb.cycles() < 16 * frame {
        gb.step();
    }
    match expect {
        GambatteExpect::Hex(digits) => {
            if hex_matches(gb.framebuffer(), digits) {
                Outcome::Pass
            } else {
                Outcome::Fail(format!("screen does not show {digits}"))
            }
        }
        GambatteExpect::Audio(want_sound) => {
            let changes = gb.bus.apu.level_changes() - before;
            if (changes > 0) == *want_sound {
                Outcome::Pass
            } else if *want_sound {
                Outcome::Fail("expected audio, got silence".into())
            } else {
                Outcome::Fail(format!("expected silence, output changed {changes}x"))
            }
        }
        GambatteExpect::Png(path) => match load_png(path) {
            Ok(img) => match diff_pixels(gb.framebuffer(), &img, SCREEN_WIDTH, SCREEN_HEIGHT) {
                0 => Outcome::Pass,
                d => Outcome::Fail(format!("{d} px differ")),
            },
            Err(e) => Outcome::Fail(e),
        },
    }
}
