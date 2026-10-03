//! Windowed DMG frontend.

mod audio;
mod pad;
mod script;

use audio::Audio;
use gb_core::{Button, GameBoy, Rewind, CLOCK_HZ, CYCLES_PER_FRAME, SCREEN_HEIGHT, SCREEN_WIDTH};
use minifb::{Key, KeyRepeat, Scale, ScaleMode, Window, WindowOptions};
use script::Action;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

const USAGE: &str =
    "usage: gbemu <rom.gb> [--scale N] [--palette gray|dmg-green|pocket] [--model dmg|cgb|auto] [--mute]\n\
    \x20             [--screenshot-at-frame N --screenshot out.png] [--exit-after-frames N]\n\
    \x20             [--input-script FILE] [--cheat CODE]...\n\
    keys: F1..F4 save state slot 1..4, Shift+F1..F4 load it (<rom>.ss1..ss4), hold Q to rewind";

const SAVE_INTERVAL: Duration = Duration::from_secs(5);

struct Args {
    rom: PathBuf,
    scale: usize,
    palette: [u32; 4],
    model: Option<gb_core::Model>,
    mute: bool,
    screenshot_at: Option<u64>,
    screenshot: Option<PathBuf>,
    exit_after: Option<u64>,
    input_script: Option<PathBuf>,
    cheats: Vec<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    let mut rom = None;
    let mut a = Args {
        rom: PathBuf::new(),
        scale: 4,
        palette: palette("gray").unwrap(),
        model: None,
        mute: false,
        screenshot_at: None,
        screenshot: None,
        exit_after: None,
        input_script: None,
        cheats: Vec::new(),
    };
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or(format!("{name} needs a value"));
        match arg.as_str() {
            "--scale" => {
                a.scale = value("--scale")?.parse().map_err(|_| "bad --scale")?;
                if scale_enum(a.scale).is_none() {
                    return Err("--scale must be one of 1, 2, 4, 8, 16, 32".into());
                }
            }
            "--palette" => {
                let name = value("--palette")?;
                a.palette = palette(&name).ok_or(format!("unknown palette {name}"))?;
            }
            "--model" => a.model = gb_core::Model::parse_choice(&value("--model")?)?,
            "--mute" => a.mute = true,
            "--screenshot-at-frame" => {
                a.screenshot_at = Some(value(&arg)?.parse().map_err(|_| "bad --screenshot-at-frame")?)
            }
            "--screenshot" => a.screenshot = Some(value(&arg)?.into()),
            "--exit-after-frames" => a.exit_after = Some(value(&arg)?.parse().map_err(|_| "bad --exit-after-frames")?),
            "--cheat" => {
                let code = value("--cheat")?;
                gb_core::Cheat::parse(&code).map_err(|e| e.to_string())?;
                a.cheats.push(code);
            }
            "--input-script" => a.input_script = Some(value(&arg)?.into()),
            "-h" | "--help" => return Err(String::new()),
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            _ => {
                if rom.replace(PathBuf::from(&arg)).is_some() {
                    return Err("more than one ROM given".into());
                }
            }
        }
    }
    a.rom = rom.ok_or("no ROM given")?;
    if a.screenshot_at.is_some() && a.screenshot.is_none() {
        return Err("--screenshot-at-frame requires --screenshot".into());
    }
    Ok(a)
}

/// Shades from lightest to darkest.
fn palette(name: &str) -> Option<[u32; 4]> {
    Some(match name {
        "gray" => [0xFFFFFF, 0xAAAAAA, 0x555555, 0x000000],
        "dmg-green" => [0x9BBC0F, 0x8BAC0F, 0x306230, 0x0F380F],
        "pocket" => [0xC4CFA1, 0x8B956D, 0x4D533C, 0x1F1F1F],
        _ => return None,
    })
}

fn scale_enum(n: usize) -> Option<Scale> {
    Some(match n {
        1 => Scale::X1,
        2 => Scale::X2,
        4 => Scale::X4,
        8 => Scale::X8,
        16 => Scale::X16,
        32 => Scale::X32,
        _ => return None,
    })
}

fn recolor(src: &[u32], pal: &[u32; 4], dst: &mut [u32]) {
    for (d, &s) in dst.iter_mut().zip(src) {
        *d = match s & 0xFF {
            0xFF => pal[0],
            0xAA => pal[1],
            0x55 => pal[2],
            _ => pal[3],
        };
    }
}

fn save_screenshot(path: &Path, pixels: &[u32]) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), SCREEN_WIDTH as u32, SCREEN_HEIGHT as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    let rgb: Vec<u8> = pixels
        .iter()
        .flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8])
        .collect();
    writer.write_image_data(&rgb).map_err(|e| e.to_string())
}

/// Persists battery RAM, remembering the last written contents.
struct Saver {
    path: PathBuf,
    last: Vec<u8>,
}

impl Saver {
    fn flush(&mut self, gb: &GameBoy) {
        if !gb.cartridge().has_battery() {
            return;
        }
        let data = gb.cartridge().save_data();
        if data == self.last {
            return;
        }
        let tmp = self.path.with_extension("sav.tmp");
        match std::fs::write(&tmp, &data).and_then(|_| std::fs::rename(&tmp, &self.path)) {
            Ok(()) => self.last = data,
            Err(e) => eprintln!("gbemu: cannot write {}: {e}", self.path.display()),
        }
    }
}

fn power_on(
    rom: &[u8],
    model: Option<gb_core::Model>,
    sav: &Path,
    sample_rate: u32,
    cheats: &[String],
) -> Result<(GameBoy, Saver), String> {
    let mut gb = GameBoy::with_model_choice(rom.to_vec(), model).map_err(|e| e.to_string())?;
    gb.set_sample_rate(sample_rate);
    for c in cheats {
        gb.add_cheat(c).map_err(|e| e.to_string())?;
    }
    let mut last = Vec::new();
    if gb.cartridge().has_battery() {
        if let Ok(data) = std::fs::read(sav) {
            gb.cartridge_mut().load_save_data(&data);
        }
        last = gb.cartridge().save_data();
    }
    Ok((
        gb,
        Saver {
            path: sav.to_path_buf(),
            last,
        },
    ))
}

/// Fixed button order used for held-state arrays.
const BUTTONS: [Button; 8] = [
    Button::Right,
    Button::Left,
    Button::Up,
    Button::Down,
    Button::A,
    Button::B,
    Button::Select,
    Button::Start,
];

fn index_of(b: Button) -> usize {
    BUTTONS.iter().position(|&x| x == b).unwrap()
}

/// Window for either the normal (resizable) or the "fullscreen" mode.
///
/// minifb has no fullscreen API and no way to query the screen size, so fullscreen is a borderless
/// topmost window at the largest power-of-two scale that fits a 16:9 screen whose width is taken
/// from a throwaway `Scale::FitScreen` probe (which only considers width), centred on that screen.
fn make_window(title: &str, scale: usize, fullscreen: bool) -> Result<Window, String> {
    let mut opts = WindowOptions {
        scale: scale_enum(scale).unwrap(),
        scale_mode: ScaleMode::AspectRatioStretch,
        resize: true,
        ..WindowOptions::default()
    };
    let mut pos = None;
    if fullscreen {
        let probe_opts = WindowOptions {
            borderless: true,
            scale: Scale::FitScreen,
            ..WindowOptions::default()
        };
        let probe = Window::new(title, SCREEN_WIDTH, SCREEN_HEIGHT, probe_opts).map_err(|e| e.to_string())?;
        let screen_w = probe.get_size().0;
        drop(probe);
        let screen_h = screen_w * 9 / 16;
        // Largest scale enum value (powers of two) fitting both dimensions.
        let s = [32, 16, 8, 4, 2, 1]
            .into_iter()
            .find(|s| SCREEN_WIDTH * s <= screen_w && SCREEN_HEIGHT * s <= screen_h)
            .unwrap_or(1);
        opts = WindowOptions {
            borderless: true,
            topmost: true,
            scale: scale_enum(s).unwrap(),
            scale_mode: ScaleMode::AspectRatioStretch,
            ..WindowOptions::default()
        };
        pos = Some(((screen_w - SCREEN_WIDTH * s) / 2, (screen_h - SCREEN_HEIGHT * s) / 2));
    }
    let mut window = Window::new(title, SCREEN_WIDTH, SCREEN_HEIGHT, opts).map_err(|e| e.to_string())?;
    window.set_background_color(0, 0, 0);
    if let Some((x, y)) = pos {
        window.set_position(x as isize, y as isize);
    }
    Ok(window)
}

const SLOT_KEYS: [Key; 4] = [Key::F1, Key::F2, Key::F3, Key::F4];

fn state_path(rom: &Path, slot: u8) -> PathBuf {
    rom.with_extension(format!("ss{slot}"))
}

fn save_slot(gb: &GameBoy, rom: &Path, slot: u8) {
    let path = state_path(rom, slot);
    match std::fs::write(&path, gb.save_state()) {
        Ok(()) => eprintln!("gbemu: saved state slot {slot} to {}", path.display()),
        Err(e) => eprintln!("gbemu: cannot write {}: {e}", path.display()),
    }
}

/// Returns true when a state was loaded.
fn load_slot(gb: &mut GameBoy, rom: &Path, slot: u8) -> bool {
    let path = state_path(rom, slot);
    match std::fs::read(&path) {
        Err(e) => eprintln!("gbemu: cannot read {}: {e}", path.display()),
        Ok(data) => match gb.load_state(&data) {
            Ok(()) => {
                eprintln!("gbemu: loaded state slot {slot}");
                return true;
            }
            Err(e) => eprintln!("gbemu: {}: {e}", path.display()),
        },
    }
    false
}

const KEYMAP: [(Key, Button); 9] = [
    (Key::Right, Button::Right),
    (Key::Left, Button::Left),
    (Key::Up, Button::Up),
    (Key::Down, Button::Down),
    (Key::X, Button::A),
    (Key::Z, Button::B),
    (Key::Enter, Button::Start),
    (Key::Backspace, Button::Select),
    (Key::RightShift, Button::Select),
];

/// After a state load the joypad holds the saved button state; release everything so the next input
/// poll re-applies whatever is actually held now.
fn resync_keys(gb: &mut GameBoy, applied: &mut [bool; 8]) {
    for b in BUTTONS {
        gb.set_button(b, false);
    }
    *applied = [false; 8];
}

fn run(args: Args) -> Result<(), String> {
    let rom = std::fs::read(&args.rom).map_err(|e| format!("{}: {e}", args.rom.display()))?;
    let events = match &args.input_script {
        Some(p) => script::parse(&std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?)?,
        None => Vec::new(),
    };
    let mut next_event = 0;

    let audio = if args.mute {
        None
    } else {
        match Audio::new() {
            Ok(a) => Some(a),
            Err(e) => {
                eprintln!("gbemu: audio disabled: {e}");
                None
            }
        }
    };
    let sample_rate = audio.as_ref().map_or(44_100, |a| a.sample_rate);
    let sav = args.rom.with_extension("sav");
    let (mut gb, mut saver) = power_on(&rom, args.model, &sav, sample_rate, &args.cheats)?;

    let title = gb.cartridge().title();
    let title = if title.trim().is_empty() {
        "gbemu".to_string()
    } else {
        title
    };
    let mut window = make_window(&title, args.scale, false)?;
    let mut fullscreen = false;
    let mut pad = pad::Pad::new();
    let mut fps_frames = 0u32;
    let mut fps_since = Instant::now();
    let mut fps_text = String::new();
    let mut shown_title = String::new();

    let mut shades = vec![0u32; SCREEN_WIDTH * SCREEN_HEIGHT];
    let mut samples = Vec::new();
    let mut applied = [false; 8];
    let mut frames: u64 = 0;
    let mut paused = false;
    let mut rewind = Rewind::default();
    let frame_dur = Duration::from_secs_f64(CYCLES_PER_FRAME as f64 / CLOCK_HZ as f64);
    let mut deadline = Instant::now();
    let mut last_save = Instant::now();
    let audio_debug = std::env::var_os("GBEMU_AUDIO_DEBUG").is_some();
    let (mut dbg_at, mut dbg_frames) = (Instant::now(), 0u64);

    while window.is_open() && !window.is_key_down(Key::Escape) {
        if window.is_key_pressed(Key::P, KeyRepeat::No) {
            paused = !paused;
        }
        if window.is_key_pressed(Key::R, KeyRepeat::No) {
            saver.flush(&gb);
            (gb, saver) = power_on(&rom, args.model, &sav, sample_rate, &args.cheats)?;
            rewind.clear();
            applied = [false; 8];
            frames = 0;
            next_event = 0;
        }
        if window.is_key_pressed(Key::F11, KeyRepeat::No) {
            fullscreen = !fullscreen;
            window = make_window(&title, args.scale, fullscreen)?;
        }
        let pad_held = pad.as_mut().map_or([false; 8], |p| p.poll());
        let mut want = pad_held;
        for (key, button) in KEYMAP {
            want[index_of(button)] |= window.is_key_down(key);
        }
        for (i, &down) in want.iter().enumerate() {
            if down != applied[i] {
                applied[i] = down;
                gb.set_button(BUTTONS[i], down);
            }
        }
        let shift = window.is_key_down(Key::LeftShift) || window.is_key_down(Key::RightShift);
        for (i, key) in SLOT_KEYS.iter().enumerate() {
            if window.is_key_pressed(*key, KeyRepeat::No) {
                let slot = i as u8 + 1;
                if !shift {
                    save_slot(&gb, &args.rom, slot);
                } else if load_slot(&mut gb, &args.rom, slot) {
                    rewind.clear();
                    resync_keys(&mut gb, &mut applied);
                }
            }
        }
        if window.is_key_pressed(Key::F12, KeyRepeat::No) {
            let mut path = args.rom.clone();
            let n = (0..)
                .find(|n| !path.with_extension(format!("{n}.png")).exists())
                .unwrap();
            path.set_extension(format!("{n}.png"));
            match save_screenshot(&path, &shades) {
                Ok(()) => eprintln!("gbemu: screenshot {}", path.display()),
                Err(e) => eprintln!("gbemu: screenshot failed: {e}"),
            }
        }

        let fast = window.is_key_down(Key::Tab);
        if fps_since.elapsed() >= Duration::from_millis(500) {
            fps_text = format!("{:.1} fps", fps_frames as f64 / fps_since.elapsed().as_secs_f64());
            fps_frames = 0;
            fps_since = Instant::now();
        }
        let mut wanted = format!("{title} - {fps_text}");
        if fast {
            wanted.push_str(" [fast]");
        }
        if paused {
            wanted = format!("{title} [paused]");
        }
        if wanted != shown_title {
            window.set_title(&wanted);
            shown_title = wanted;
        }

        if paused {
            window.update();
            std::thread::sleep(Duration::from_millis(16));
            deadline = Instant::now();
            continue;
        }

        let mut rewound = false;
        while next_event < events.len() && events[next_event].frame <= frames {
            let e = events[next_event];
            match e.action {
                Action::Button { button, pressed } => gb.set_button(button, pressed),
                Action::Save(slot) => save_slot(&gb, &args.rom, slot),
                Action::Load(slot) => {
                    if load_slot(&mut gb, &args.rom, slot) {
                        rewind.clear();
                    }
                }
                Action::Rewind(n) => {
                    for _ in 0..n {
                        if !rewind.pop_into(&mut gb) {
                            break;
                        }
                    }
                    rewound = true;
                }
            }
            next_event += 1;
        }

        if window.is_key_down(Key::Q) {
            // Hold to rewind: each emulated frame steps back one snapshot (a few frames of game time).
            rewind.pop_into(&mut gb);
        } else if !rewound {
            rewind.frame(&gb);
            gb.run_frame();
        }
        frames += 1;
        fps_frames += 1;
        if gb.model() == gb_core::Model::Cgb {
            shades.copy_from_slice(gb.framebuffer());
        } else {
            recolor(gb.framebuffer(), &args.palette, &mut shades);
        }
        window
            .update_with_buffer(&shades, SCREEN_WIDTH, SCREEN_HEIGHT)
            .map_err(|e| e.to_string())?;

        samples.clear();
        gb.drain_audio(&mut samples);
        let mut audio_driven = false;
        if let Some(a) = &audio {
            if !samples.is_empty() && !fast {
                a.push(&samples);
                audio_driven = true;
            }
        }

        if audio_debug && dbg_at.elapsed() >= Duration::from_secs(1) {
            let secs = dbg_at.elapsed().as_secs_f64();
            let (fill, played, under) = audio.as_ref().map_or((0, 0, 0), |a| {
                (
                    a.fill_frames(),
                    a.stats.played.load(std::sync::atomic::Ordering::Relaxed),
                    a.stats.underrun.load(std::sync::atomic::Ordering::Relaxed),
                )
            });
            eprintln!(
                "audio: {:.2} fps, ring fill {fill} frames, device consumed {played}, underrun {under}",
                (frames - dbg_frames) as f64 / secs
            );
            dbg_at = Instant::now();
            dbg_frames = frames;
        }
        if args.screenshot_at == Some(frames) {
            let path = args.screenshot.as_ref().unwrap();
            save_screenshot(path, &shades)?;
            eprintln!("gbemu: screenshot {} at frame {frames}", path.display());
        }
        if args.exit_after.is_some_and(|n| frames >= n) {
            break;
        }

        if last_save.elapsed() >= SAVE_INTERVAL {
            saver.flush(&gb);
            last_save = Instant::now();
        }

        if fast {
            deadline = Instant::now();
        } else if audio_driven {
            // Keep the ring near its target fill; bounded so a dead device cannot stall us.
            let a = audio.as_ref().unwrap();
            let give_up = Instant::now() + Duration::from_millis(100);
            while a.fill_frames() > a.target_frames && Instant::now() < give_up {
                std::thread::sleep(Duration::from_millis(1));
            }
            deadline = Instant::now();
        } else {
            deadline += frame_dur;
            let now = Instant::now();
            if deadline > now {
                std::thread::sleep(deadline - now);
            } else if now - deadline > frame_dur * 5 {
                deadline = now;
            }
        }
    }
    saver.flush(&gb);
    Ok(())
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("gbemu: {e}");
            }
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("gbemu: {e}");
            ExitCode::FAILURE
        }
    }
}
