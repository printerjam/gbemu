//! CGB features exercised through hand-assembled ROMs (no external test ROMs needed).
use gb_core::{GameBoy, Model};

/// 32 KiB ROM with `code` at 0x0100 and the CGB-enhanced flag set.
fn rom(code: &[u8], cgb_flag: u8) -> Vec<u8> {
    let mut r = vec![0u8; 0x8000];
    r[0x100..0x100 + code.len()].copy_from_slice(code);
    r[0x143] = cgb_flag;
    r
}

fn run_until_halt_loop(gb: &mut GameBoy, steps: usize) {
    for _ in 0..steps {
        gb.step();
    }
}

#[test]
fn model_follows_header_and_override() {
    assert_eq!(GameBoy::new(rom(&[], 0x80)).unwrap().model(), Model::Cgb);
    assert_eq!(GameBoy::new(rom(&[], 0x00)).unwrap().model(), Model::Dmg);
    assert_eq!(
        GameBoy::with_model(rom(&[], 0x00), Model::Cgb).unwrap().model(),
        Model::Cgb
    );
    assert_eq!(
        GameBoy::with_model(rom(&[], 0x80), Model::Dmg).unwrap().model(),
        Model::Dmg
    );
}

#[test]
fn speed_switch_doubles_cpu_clock_relative_to_ppu() {
    // ld a,1 ; ldh (4D),a ; stop ; nop ; jr -2
    let code = [0x3E, 0x01, 0xE0, 0x4D, 0x10, 0x00, 0x00, 0x18, 0xFE];
    let mut gb = GameBoy::new(rom(&code, 0x80)).unwrap();
    assert!(!gb.bus.double_speed());
    run_until_halt_loop(&mut gb, 5);
    assert!(gb.bus.double_speed());
    assert_eq!(gb.bus.peek(0xFF4D) & 0x81, 0x80, "KEY1: double speed, no longer armed");
    // In double speed an M-cycle is 2 dots: 1000 steps of `jr` (3 M-cycles) = 6000 dots.
    let before = gb.cycles();
    run_until_halt_loop(&mut gb, 1000);
    assert_eq!(gb.cycles() - before, 1000 * 3 * 2);
}

#[test]
fn stop_without_armed_switch_keeps_speed() {
    let code = [0x10, 0x00, 0x00, 0x18, 0xFE];
    let mut gb = GameBoy::new(rom(&code, 0x80)).unwrap();
    run_until_halt_loop(&mut gb, 10);
    assert!(!gb.bus.double_speed());
}

#[test]
fn gdma_copies_blocks_to_selected_vram_bank_and_reports_done() {
    // WRAM 0xC000 pattern -> VRAM bank 1 0x8000, 2 blocks; then read FF55.
    let code = [
        0xAF, 0xE0, 0x40, // LCD off so VRAM is never locked
        0x3E, 0x01, 0xE0, 0x4F, // VBK = 1
        0x3E, 0xC0, 0xE0, 0x51, // src hi
        0xAF, 0xE0, 0x52, // src lo = 0
        0x3E, 0x80, 0xE0, 0x53, // dst hi = 0x80 (masked to 0x00 -> 0x8000)
        0xAF, 0xE0, 0x54, // dst lo
        0x3E, 0x01, 0xE0, 0x55, // GDMA, 2 blocks
        0xF0, 0x55, 0x47, // ld b, (FF55)
        0x18, 0xFE,
    ];
    let mut gb = GameBoy::new(rom(&code, 0x80)).unwrap();
    for i in 0..32u16 {
        gb.bus.poke(0xC000 + i, 0x40 + i as u8);
    }
    run_until_halt_loop(&mut gb, 16);
    assert_eq!(gb.registers().b, 0xFF, "transfer finished");
    assert_eq!(gb.bus.peek(0x8000), 0x40, "bank 1 selected for the copy");
    assert_eq!(gb.bus.peek(0x801F), 0x5F);
    gb.bus.poke(0xFF4F, 0);
    assert_eq!(gb.bus.peek(0x8000), 0x00, "bank 0 untouched");
}

#[test]
fn wram_banking_via_svbk() {
    let mut gb = GameBoy::new(rom(&[], 0x80)).unwrap();
    gb.bus.poke(0xFF70, 2);
    gb.bus.poke(0xD000, 0x22);
    gb.bus.poke(0xFF70, 3);
    gb.bus.poke(0xD000, 0x33);
    gb.bus.poke(0xC000, 0x11);
    assert_eq!(gb.bus.peek(0xD000), 0x33);
    gb.bus.poke(0xFF70, 2);
    assert_eq!(gb.bus.peek(0xD000), 0x22);
    gb.bus.poke(0xFF70, 0);
    assert_eq!(gb.bus.peek(0xD000), 0x00, "bank 0 selects bank 1");
    assert_eq!(gb.bus.peek(0xE000), 0x11, "echo of bank 0");
}

#[test]
fn dmg_ignores_cgb_registers() {
    let mut gb = GameBoy::with_model(rom(&[], 0x80), Model::Dmg).unwrap();
    gb.bus.poke(0xFF70, 3);
    gb.bus.poke(0xFF4F, 1);
    assert_eq!(gb.bus.peek(0xFF70), 0xFF);
    assert_eq!(gb.bus.peek(0xFF4F), 0xFF);
}
