//! `gbterm`: Game Boy in a truecolor terminal (Unicode upper-half-block rendering).

mod input;
mod render;

use crossterm::event::{
    self, Event, KeyEventKind, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{cursor, execute};
use gb_core::GameBoy;
use input::{Action, Input};
use render::{Encoder, Palette};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const USAGE: &str =
    "usage: gbterm <rom.gb> [--palette gray|dmg-green|pocket] [--model dmg|cgb|auto] [--exit-after-frames N] \
[--dump-frame-ansi FILE]

keys: arrows/WASD d-pad, X/K = A, Z/J = B, Enter = Start, Backspace/Shift+Tab = Select, P pause, Q/Esc quit
       F1 or [ = save state, F2 or ] = load state (slot 1: <rom>.ss1)";

/// Real DMG refresh rate.
const FPS: f64 = 59.7275;

struct Args {
    rom: PathBuf,
    palette: Palette,
    model: Option<gb_core::Model>,
    exit_after: Option<u64>,
    dump: Option<PathBuf>,
}

fn fail(msg: &str) -> ! {
    eprintln!("gbterm: {msg}\n{USAGE}");
    std::process::exit(2);
}

fn parse_args() -> Args {
    let mut rom = None;
    let mut a = Args {
        rom: PathBuf::new(),
        palette: Palette::Gray,
        model: None,
        exit_after: None,
        dump: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = |f: &str| it.next().unwrap_or_else(|| fail(&format!("{f} needs a value")));
        match arg.as_str() {
            "--palette" => {
                let v = val("--palette");
                a.palette = Palette::parse(&v).unwrap_or_else(|| fail(&format!("unknown palette '{v}'")));
            }
            "--model" => {
                let v = val("--model");
                a.model = gb_core::Model::parse_choice(&v).unwrap_or_else(|e| fail(&e));
            }
            "--exit-after-frames" => {
                let v = val("--exit-after-frames");
                a.exit_after = Some(v.parse().unwrap_or_else(|_| fail(&format!("bad frame count '{v}'"))));
            }
            "--dump-frame-ansi" => a.dump = Some(val("--dump-frame-ansi").into()),
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            s if !s.starts_with("--") && rom.is_none() => rom = Some(PathBuf::from(s)),
            s => fail(&format!("unknown argument '{s}'")),
        }
    }
    a.rom = rom.unwrap_or_else(|| fail("missing <rom>"));
    a
}

static KITTY_PUSHED: AtomicBool = AtomicBool::new(false);
static TERM_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Undo everything `enter_terminal` did. Idempotent; safe to call from a panic hook.
fn restore_terminal() {
    if !TERM_ACTIVE.swap(false, Ordering::SeqCst) {
        return;
    }
    let mut out = std::io::stdout();
    if KITTY_PUSHED.swap(false, Ordering::SeqCst) {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(out, cursor::Show, LeaveAlternateScreen);
    let _ = terminal::disable_raw_mode();
    let _ = out.flush();
}

/// Returns whether key release events are available.
fn enter_terminal() -> std::io::Result<bool> {
    terminal::enable_raw_mode()?;
    TERM_ACTIVE.store(true, Ordering::SeqCst);
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        prev(info);
    }));
    let mut out = std::io::stdout();
    execute!(
        out,
        EnterAlternateScreen,
        cursor::Hide,
        terminal::Clear(terminal::ClearType::All)
    )?;
    let mut releases = false;
    if terminal::supports_keyboard_enhancement().unwrap_or(false) {
        let flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
            | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES;
        if execute!(out, PushKeyboardEnhancementFlags(flags)).is_ok() {
            KITTY_PUSHED.store(true, Ordering::SeqCst);
            releases = true;
        }
    }
    Ok(releases)
}

fn sav_path(rom: &Path) -> PathBuf {
    rom.with_extension("sav")
}

fn save_battery(gb: &GameBoy, path: &Path) {
    if gb.cartridge().has_battery() {
        if let Err(e) = std::fs::write(path, gb.cartridge().save_data()) {
            eprintln!("gbterm: cannot write {}: {e}", path.display());
        }
    }
}

fn state_path(rom: &Path) -> PathBuf {
    rom.with_extension("ss1")
}

fn save_slot(gb: &GameBoy, rom: &Path) -> String {
    let path = state_path(rom);
    match std::fs::write(&path, gb.save_state()) {
        Ok(()) => format!("state saved to {}", path.display()),
        Err(e) => format!("cannot write {}: {e}", path.display()),
    }
}

/// Returns the notice text and whether a state was loaded.
fn load_slot(gb: &mut GameBoy, rom: &Path) -> (String, bool) {
    let path = state_path(rom);
    match std::fs::read(&path) {
        Err(e) => (format!("cannot read {}: {e}", path.display()), false),
        Ok(data) => match gb.load_state(&data) {
            Ok(()) => ("state loaded".into(), true),
            Err(e) => (format!("load failed: {e}"), false),
        },
    }
}

fn truncate(s: &str, cols: usize) -> String {
    s.chars().take(cols).collect()
}

fn main() {
    let args = parse_args();
    let rom = std::fs::read(&args.rom).unwrap_or_else(|e| fail(&format!("{}: {e}", args.rom.display())));
    let mut gb = GameBoy::with_model_choice(rom, args.model)
        .unwrap_or_else(|e| fail(&format!("{}: cartridge error: {e:?}", args.rom.display())));
    let sav = sav_path(&args.rom);
    if gb.cartridge().has_battery() {
        if let Ok(data) = std::fs::read(&sav) {
            gb.cartridge_mut().load_save_data(&data);
        }
    }
    let title = gb.cartridge().title();

    let interactive = std::io::stdout().is_terminal() && std::io::stdin().is_terminal();
    if !interactive && args.exit_after.is_none() {
        fail("stdin/stdout is not a terminal; use --exit-after-frames for headless runs");
    }

    let mut input = Input::new(false);
    if interactive {
        match enter_terminal() {
            Ok(releases) => input = Input::new(releases),
            Err(e) => {
                restore_terminal();
                fail(&format!("cannot initialise terminal: {e}"));
            }
        }
    }

    let result = run(&mut gb, &args, &title, interactive, input);
    restore_terminal();
    save_battery(&gb, &sav);
    if let Some(path) = &args.dump {
        // Fit to the real terminal when interactive; headless dumps use the native 160x72 cells
        // (crossterm's size() falls back to 80x24 via tput instead of failing without a tty).
        let (cols, rows) = if interactive {
            terminal::size()
                .map(|(c, r)| (c as usize, r as usize))
                .unwrap_or((usize::MAX, usize::MAX))
        } else {
            (usize::MAX, usize::MAX)
        };
        let (w, h) = render::fit(cols, rows);
        let cells = render::to_cells(gb.framebuffer(), w, h, args.palette);
        if let Err(e) = std::fs::write(path, render::render_plain(&cells, w, h)) {
            eprintln!("gbterm: cannot write {}: {e}", path.display());
            std::process::exit(1);
        }
    }
    if let Err(e) = result {
        eprintln!("gbterm: {e}");
        std::process::exit(1);
    }
}

fn run(gb: &mut GameBoy, args: &Args, title: &str, interactive: bool, mut input: Input) -> std::io::Result<()> {
    let period = Duration::from_secs_f64(1.0 / FPS);
    let mut enc = Encoder::default();
    let mut out = std::io::stdout();
    let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
    let mut audio = Vec::new();
    let (mut paused, mut quit) = (false, false);
    let mut frame: u64 = 0;
    let mut next = Instant::now();
    let (mut fps_start, mut fps_frames, mut fps) = (Instant::now(), 0u32, 0.0f64);
    let mut last_status = String::new();
    let mut notice: Option<(String, Instant)> = None;

    while !quit && args.exit_after.is_none_or(|n| frame < n) {
        for (b, pressed) in input.changes(frame) {
            gb.set_button(b, pressed);
        }
        if !paused {
            gb.run_frame();
        }
        audio.clear();
        gb.drain_audio(&mut audio);
        frame += 1;

        if notice
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() > Duration::from_secs(3))
        {
            notice = None;
        }
        if interactive {
            let (cols, rows) = terminal::size()
                .map(|(c, r)| (c as usize, r as usize))
                .unwrap_or((80, 24));
            let (w, h) = render::fit(cols, rows.saturating_sub(1));
            let cells = render::to_cells(gb.framebuffer(), w, h, args.palette);
            buf.clear();
            buf.extend_from_slice(b"\x1b[?2026h");
            enc.encode(&cells, w, h, (cols - w) / 2, 0, &mut buf);
            fps_frames += 1;
            if fps_start.elapsed() >= Duration::from_secs(1) {
                fps = fps_frames as f64 / fps_start.elapsed().as_secs_f64();
                fps_frames = 0;
                fps_start = Instant::now();
            }
            let status = truncate(
                &format!(
                    "{title} | {fps:5.1} fps{} | q quit, p pause, F1/[ save, F2/] load{}",
                    if paused { " | PAUSED" } else { "" },
                    notice.as_ref().map_or(String::new(), |(n, _)| format!(" | {n}"))
                ),
                cols,
            );
            if status != last_status {
                buf.extend_from_slice(format!("\x1b[0m\x1b[{};1H{status}\x1b[K", rows).as_bytes());
                last_status = status;
            }
            buf.extend_from_slice(b"\x1b[0m\x1b[?2026l");
            if buf.len() > "\x1b[?2026h\x1b[0m\x1b[?2026l".len() {
                out.write_all(&buf)?;
                out.flush()?;
            }

            // Pace: wait out the rest of the frame, handling input as it arrives.
            next += period;
            let now = Instant::now();
            if next + Duration::from_millis(100) < now {
                next = now; // fell behind; do not try to catch up
            }
            loop {
                let now = Instant::now();
                if now >= next {
                    break;
                }
                if !event::poll(next - now)? {
                    break;
                }
                match event::read()? {
                    Event::Key(k) => match input::map_key(&k) {
                        Some(Action::Quit) if k.kind != KeyEventKind::Release => quit = true,
                        Some(Action::Pause) if k.kind == KeyEventKind::Press => paused = !paused,
                        Some(Action::SaveState) if k.kind == KeyEventKind::Press => {
                            notice = Some((save_slot(gb, &args.rom), Instant::now()));
                        }
                        Some(Action::LoadState) if k.kind == KeyEventKind::Press => {
                            let (text, loaded) = load_slot(gb, &args.rom);
                            if loaded {
                                for (b, pressed) in input.resync(frame) {
                                    gb.set_button(b, pressed);
                                }
                            }
                            notice = Some((text, Instant::now()));
                        }
                        Some(Action::Button(b)) => input.key(b, k.kind, frame),
                        _ => {}
                    },
                    Event::Resize(..) => {
                        enc.invalidate();
                        last_status.clear();
                        execute!(out, terminal::Clear(terminal::ClearType::All))?;
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(())
}
