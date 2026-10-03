//! Save-state tests. ROM-driven ones are skipped when `roms/` is absent.
use gb_core::{GameBoy, Model, StateError};
use std::path::PathBuf;

fn rom(rel: &str) -> Option<Vec<u8>> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../roms").join(rel);
    std::fs::read(p).ok()
}

#[derive(Debug, PartialEq)]
struct Observation {
    framebuffer: Vec<u32>,
    regs: gb_core::cpu::Registers,
    cycles: u64,
    serial: Vec<u8>,
    ram: Vec<u8>,
    audio: Vec<f32>,
}

fn observe(gb: &mut GameBoy) -> Observation {
    let mut audio = Vec::new();
    gb.drain_audio(&mut audio);
    Observation {
        framebuffer: gb.framebuffer().to_vec(),
        regs: *gb.registers(),
        cycles: gb.cycles(),
        serial: gb.serial_output().to_vec(),
        ram: gb.cartridge().save_data(),
        audio,
    }
}

fn run_frames(gb: &mut GameBoy, n: usize) {
    for _ in 0..n {
        gb.run_frame();
    }
}

/// Run `n` frames, snapshot, run `m` more (A); restore into the same machine and into a fresh one, run `m` (B, C).
fn check_determinism(rom_bytes: Vec<u8>, n: usize, m: usize) {
    check_determinism_model(rom_bytes, None, n, m);
}

fn check_determinism_model(rom_bytes: Vec<u8>, model: Option<Model>, n: usize, m: usize) {
    let mut gb = GameBoy::with_model_choice(rom_bytes.clone(), model).unwrap();
    gb.set_sample_rate(44100);
    run_frames(&mut gb, n);
    let state = gb.save_state();
    let mut sink = Vec::new();
    gb.drain_audio(&mut sink);
    run_frames(&mut gb, m);
    let a = observe(&mut gb);

    gb.load_state(&state).unwrap();
    run_frames(&mut gb, m);
    assert_eq!(a, observe(&mut gb), "same machine after load_state");

    let mut fresh = GameBoy::with_model_choice(rom_bytes, model).unwrap();
    fresh.set_sample_rate(44100);
    fresh.load_state(&state).unwrap();
    run_frames(&mut fresh, m);
    assert_eq!(a, observe(&mut fresh), "fresh machine after load_state");
}

#[test]
fn deterministic_after_load_acid2() {
    let Some(r) = rom("dmg-acid2/dmg-acid2.gb") else { return };
    check_determinism(r, 20, 30);
}

#[test]
fn deterministic_after_load_cgb_acid2() {
    let Some(r) = rom("cgb-acid2/cgb-acid2.gbc") else { return };
    check_determinism(r, 20, 30);
}

#[test]
fn deterministic_after_load_cgb_compat_mode() {
    let Some(r) = rom("dmg-acid2/dmg-acid2.gb") else { return };
    check_determinism_model(r, Some(Model::Cgb), 20, 30);
}

#[test]
fn deterministic_after_load_cgb_hdma_and_banks() {
    // SameSuite hdma_mode0 leaves HDMA, VRAM/WRAM banking and palette state in play.
    let Some(r) = rom("same-suite/dma/hdma_mode0.gb") else { return };
    check_determinism(r, 3, 5);
}

#[test]
fn deterministic_after_load_blargg_serial() {
    let Some(r) = rom("blargg/cpu_instrs/individual/06-ld r,r.gb") else {
        return;
    };
    check_determinism(r, 100, 300);
}

#[test]
fn deterministic_after_load_mbc_ram() {
    let Some(r) = rom("mooneye-test-suite/emulator-only/mbc1/ram_64kb.gb") else {
        return;
    };
    check_determinism(r, 3, 10);
}

#[test]
fn rejects_other_rom_and_garbage() {
    let (Some(a), Some(b)) = (
        rom("dmg-acid2/dmg-acid2.gb"),
        rom("blargg/cpu_instrs/individual/06-ld r,r.gb"),
    ) else {
        return;
    };
    let mut ga = GameBoy::new(a).unwrap();
    run_frames(&mut ga, 5);
    let state = ga.save_state();
    let mut gbb = GameBoy::new(b).unwrap();
    run_frames(&mut gbb, 5);
    let before = gbb.save_state();
    assert_eq!(gbb.load_state(&state), Err(StateError::WrongRom));
    assert_eq!(gbb.save_state(), before, "failed load must not modify the machine");
    assert_eq!(gbb.load_state(b"nope"), Err(StateError::BadMagic));
    assert_eq!(gbb.load_state(&[]), Err(StateError::BadMagic));
}

/// Synthetic MBC3+RTC+RAM cartridge: MBC registers, RAM, RTC and CPU state all survive a snapshot.
fn mbc3_rom() -> Vec<u8> {
    let mut rom = vec![0u8; 8 * 0x4000];
    for b in 0..8 {
        rom[b * 0x4000 + 0x3FFF] = b as u8;
    }
    rom[0x100..0x103].copy_from_slice(&[0x00, 0xC3, 0x50]); // NOP; JP $0150
    rom[0x103] = 0x01;
    rom[0x150..0x153].copy_from_slice(&[0xC3, 0x50, 0x01]); // spin
    rom[0x147] = 0x10;
    rom[0x148] = 2;
    rom[0x149] = 3;
    rom
}

#[test]
fn restores_mbc3_registers_ram_and_rtc() {
    let mut gb = GameBoy::new(mbc3_rom()).unwrap();
    gb.bus.poke(0x0000, 0x0A);
    gb.bus.poke(0x2000, 5);
    gb.bus.poke(0x4000, 0x08); // select RTC seconds
    gb.bus.poke(0xA000, 17);
    gb.bus.poke(0x4000, 2);
    gb.bus.poke(0xA123, 0x5A);
    for _ in 0..1000 {
        gb.step();
    }
    let state = gb.save_state();
    let (ram, pc, bank_byte) = (gb.cartridge().save_data(), gb.registers().pc, gb.bus.peek(0x7FFF));
    assert_eq!(bank_byte, 5);

    gb.bus.poke(0x2000, 1);
    gb.bus.poke(0x4000, 2);
    gb.bus.poke(0xA123, 0);
    gb.bus.poke(0x0000, 0);
    for _ in 0..777 {
        gb.step();
    }
    gb.load_state(&state).unwrap();
    assert_eq!(gb.cartridge().save_data(), ram);
    assert_eq!(gb.registers().pc, pc);
    assert_eq!(gb.bus.peek(0x7FFF), 5);
    assert_eq!(gb.bus.peek(0xA123), 0x5A); // RAM still enabled, bank 2 still selected
    gb.bus.poke(0x4000, 0x08);
    gb.bus.poke(0x6000, 0);
    gb.bus.poke(0x6000, 1);
    assert_eq!(gb.bus.peek(0xA000), 17); // live RTC seconds restored
}

#[test]
fn restore_keeps_host_settings() {
    let mut gb = GameBoy::new(mbc3_rom()).unwrap();
    gb.set_sample_rate(48000);
    gb.bus.ppu.set_palette([1, 2, 3, 4]);
    let state = gb.save_state();
    gb.set_sample_rate(22050);
    gb.bus.ppu.set_palette([9, 8, 7, 6]);
    gb.load_state(&state).unwrap();
    assert_eq!(gb.bus.ppu.palette(), [9, 8, 7, 6]);
    assert_eq!(gb.bus.apu.sample_rate(), 22050);
}

#[test]
fn rejects_corrupt_and_truncated() {
    let mut gb = GameBoy::new(mbc3_rom()).unwrap();
    let state = gb.save_state();
    let mut bad = state.clone();
    let last = bad.len() - 1;
    bad[last] ^= 0x01;
    assert_eq!(gb.load_state(&bad), Err(StateError::Corrupt));
    assert!(gb.load_state(&state[..state.len() - 10]).is_err());
    let mut other_version = state.clone();
    other_version[4] ^= 0xFF;
    assert!(matches!(gb.load_state(&other_version), Err(StateError::Version { .. })));
    gb.load_state(&state).unwrap();
}
