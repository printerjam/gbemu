//! `gbtest bench`: headless speed measurement.

use crate::{parse, usage_error, value};
use gb_core::{GameBoy, CLOCK_HZ, CYCLES_PER_FRAME};
use std::time::Instant;

pub fn run(mut args: impl Iterator<Item = String>) {
    let mut rom: Option<String> = None;
    let mut frames = 3000u64;
    let mut model = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--frames" => frames = parse(&value(&mut args, &a), &a),
            "--model" => {
                model = gb_core::Model::parse_choice(&value(&mut args, &a)).unwrap_or_else(|e| usage_error(&e))
            }
            _ if !a.starts_with("--") && rom.is_none() => rom = Some(a),
            _ => usage_error(&format!("bench: unknown argument '{a}'")),
        }
    }
    let rom = rom.unwrap_or_else(|| usage_error("bench: missing <rom>"));
    let data = std::fs::read(&rom).unwrap_or_else(|e| usage_error(&format!("{rom}: {e}")));
    let mut gb = GameBoy::with_model_choice(data, model)
        .unwrap_or_else(|e| usage_error(&format!("{rom}: cartridge error {e:?}")));
    gb.set_sample_rate(48_000);
    let mut audio = Vec::new();
    let start = Instant::now();
    for _ in 0..frames {
        gb.run_frame();
        audio.clear();
        gb.drain_audio(&mut audio);
    }
    let secs = start.elapsed().as_secs_f64();
    let fps = frames as f64 / secs;
    let real_fps = CLOCK_HZ as f64 / CYCLES_PER_FRAME as f64;
    println!(
        "{rom}: {frames} frames in {secs:.3}s = {fps:.0} frames/s = {:.0}x real time ({:.2} Mcycles/s)",
        fps / real_fps,
        gb.cycles() as f64 / secs / 1e6
    );
}
