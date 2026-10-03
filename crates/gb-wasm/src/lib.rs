//! Browser bindings: a thin wrapper that turns `GameBoy` into something JS can drive.
//!
//! The framebuffer is exposed as RGBA bytes in wasm memory (`frame_ptr`/`frame_len`) so JS can wrap it in an
//! `ImageData` without copying across the boundary each frame. Audio is drained into a reusable buffer
//! (`audio_ptr`/`audio_len` after `run_frame`).

use gb_core::{Button, GameBoy, Model, Rewind, SCREEN_HEIGHT, SCREEN_WIDTH};
use wasm_bindgen::prelude::*;

/// Shades from lightest to darkest, same tables as the desktop and terminal frontends.
const PALETTES: [[u32; 4]; 3] = [
    [0xFFFFFF, 0xAAAAAA, 0x555555, 0x000000], // gray
    [0x9BBC0F, 0x8BAC0F, 0x306230, 0x0F380F], // dmg-green
    [0xC4CFA1, 0x8B956D, 0x4D533C, 0x1F1F1F], // pocket
];

fn button_from_code(code: u8) -> Option<Button> {
    Some(match code {
        0 => Button::Right,
        1 => Button::Left,
        2 => Button::Up,
        3 => Button::Down,
        4 => Button::A,
        5 => Button::B,
        6 => Button::Select,
        7 => Button::Start,
        _ => return None,
    })
}

fn fnv1a(data: &[u8]) -> u32 {
    data.iter()
        .fold(0x811C_9DC5u32, |h, &b| (h ^ b as u32).wrapping_mul(0x0100_0193))
}

#[wasm_bindgen]
pub struct Emulator {
    gb: GameBoy,
    rgba: Vec<u8>,
    audio: Vec<f32>,
    palette: usize,
    rom_hash: u32,
    rewind: Rewind,
}

#[wasm_bindgen]
impl Emulator {
    /// Power on with a ROM image. `model` is `"auto"` (follow the cartridge header), `"dmg"` or `"cgb"`.
    /// Throws a string error for unsupported/invalid cartridges or an unknown model.
    #[wasm_bindgen(constructor)]
    pub fn new(rom: &[u8], model: &str) -> Result<Emulator, JsError> {
        let rom_hash = fnv1a(rom);
        let choice = Model::parse_choice(model).map_err(|e| JsError::new(&e))?;
        let gb = GameBoy::with_model_choice(rom.to_vec(), choice).map_err(|e| JsError::new(&e.to_string()))?;
        let mut emu = Emulator {
            gb,
            rgba: vec![0xFF; SCREEN_WIDTH * SCREEN_HEIGHT * 4],
            audio: Vec::new(),
            palette: 0,
            rom_hash,
            rewind: Rewind::default(),
        };
        emu.convert_frame();
        Ok(emu)
    }

    pub fn set_sample_rate(&mut self, hz: u32) {
        self.gb.set_sample_rate(hz);
    }

    /// `0` gray, `1` dmg-green, `2` pocket. Takes effect on the next converted frame.
    pub fn set_palette(&mut self, index: usize) {
        self.palette = index.min(PALETTES.len() - 1);
        self.convert_frame();
    }

    /// Button codes: 0 Right, 1 Left, 2 Up, 3 Down, 4 A, 5 B, 6 Select, 7 Start.
    pub fn set_button(&mut self, code: u8, pressed: bool) {
        if let Some(b) = button_from_code(code) {
            self.gb.set_button(b, pressed);
        }
    }

    /// Emulate one video frame, refresh the RGBA frame and collect the audio produced.
    pub fn run_frame(&mut self) {
        self.rewind.frame(&self.gb);
        self.gb.run_frame();
        self.convert_frame();
        self.audio.clear();
        self.gb.drain_audio(&mut self.audio);
    }

    /// Step back one rewind snapshot (a few frames of game time) and refresh the frame. Returns false when
    /// there is no history left. Audio is not produced while rewinding.
    pub fn rewind_step(&mut self) -> bool {
        let ok = self.rewind.pop_into(&mut self.gb);
        if ok {
            self.convert_frame();
        }
        self.audio.clear();
        self.gb.drain_audio(&mut self.audio);
        self.audio.clear();
        ok
    }

    /// `"dmg"` or `"cgb"`: the model actually running (after resolving `auto`).
    pub fn model(&self) -> String {
        match self.gb.model() {
            Model::Cgb => "cgb",
            Model::Dmg => "dmg",
        }
        .to_string()
    }

    pub fn frame_ptr(&self) -> *const u8 {
        self.rgba.as_ptr()
    }

    pub fn frame_len(&self) -> usize {
        self.rgba.len()
    }

    /// Stereo interleaved f32 samples produced by the last `run_frame`.
    pub fn audio_ptr(&self) -> *const f32 {
        self.audio.as_ptr()
    }

    /// Number of f32 values (2 per stereo frame).
    pub fn audio_len(&self) -> usize {
        self.audio.len()
    }

    pub fn title(&self) -> String {
        self.gb.cartridge().title()
    }

    /// FNV-1a of the ROM image, for keying saves.
    pub fn rom_hash(&self) -> u32 {
        self.rom_hash
    }

    pub fn has_battery(&self) -> bool {
        self.gb.cartridge().has_battery()
    }

    /// Battery RAM (+RTC footer) to persist. `now_secs` is the unix time (for MBC3 RTC carts).
    pub fn save_data(&mut self, now_secs: f64) -> Vec<u8> {
        self.gb.cartridge_mut().set_unix_time(now_secs as u64);
        self.gb.cartridge().save_data()
    }

    pub fn load_save_data(&mut self, data: &[u8], now_secs: f64) {
        self.gb.cartridge_mut().load_save_data_at(data, now_secs as u64);
    }

    /// Snapshot of the complete machine (see `GameBoy::save_state`).
    pub fn save_state(&self) -> Vec<u8> {
        self.gb.save_state()
    }

    /// Restore a snapshot from the same ROM. Throws on mismatch/corruption and leaves the machine untouched.
    pub fn load_state(&mut self, data: &[u8]) -> Result<(), JsError> {
        self.gb.load_state(data).map_err(|e| JsError::new(&e.to_string()))?;
        self.convert_frame();
        Ok(())
    }

    /// Bytes sent over the serial port so far (handy for blargg-style test ROMs in the browser).
    pub fn serial_output(&self) -> String {
        String::from_utf8_lossy(self.gb.serial_output()).into_owned()
    }

    pub fn width(&self) -> usize {
        SCREEN_WIDTH
    }

    pub fn height(&self) -> usize {
        SCREEN_HEIGHT
    }
}

impl Emulator {
    fn convert_frame(&mut self) {
        let cgb = self.gb.model() == Model::Cgb;
        let table = PALETTES[self.palette];
        for (px, out) in self.gb.framebuffer().iter().zip(self.rgba.as_chunks_mut::<4>().0) {
            let rgb = match px & 0xFF_FFFF {
                other if cgb => other,
                0xFFFFFF => table[0],
                0xAAAAAA => table[1],
                0x555555 => table[2],
                0x000000 => table[3],
                other => other,
            };
            *out = [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 0xFF];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rom() -> Vec<u8> {
        let mut r = vec![0u8; 0x8000];
        r[0x100] = 0x18; // JR -2: spin
        r[0x101] = 0xFE;
        r
    }

    #[test]
    fn frame_is_rgba_and_palette_applies() {
        let mut e = Emulator::new(&rom(), "auto").ok().unwrap();
        e.run_frame();
        assert_eq!(e.frame_len(), 160 * 144 * 4);
        assert!(e.rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 0xFF));
        e.set_palette(1);
        assert_eq!(&e.rgba[..3], &[0x9B, 0xBC, 0x0F]);
        e.set_palette(99);
        assert_eq!(e.palette, 2);
    }

    #[test]
    fn state_roundtrip_through_wrapper() {
        let mut e = Emulator::new(&rom(), "auto").ok().unwrap();
        e.run_frame();
        let st = e.save_state();
        e.run_frame();
        assert!(e.load_state(&st).is_ok());
        assert_eq!(e.save_state(), st);
    }

    #[test]
    fn rewind_returns_to_an_earlier_snapshot() {
        let mut e = Emulator::new(&rom(), "auto").ok().unwrap();
        for _ in 0..8 {
            e.run_frame();
        }
        let before = e.save_state();
        e.run_frame(); // snapshot of `before` is taken at the start of this frame
        assert!(e.rewind_step());
        assert_eq!(e.save_state(), before);
    }

    #[test]
    fn audio_accumulates_per_frame_at_sample_rate() {
        let mut e = Emulator::new(&rom(), "auto").ok().unwrap();
        e.set_sample_rate(48_000);
        e.run_frame();
        e.run_frame();
        // one frame ~= 800 stereo frames at 48 kHz
        let n = e.audio_len();
        assert!((1500..=1700).contains(&n), "{n}");
    }
}
