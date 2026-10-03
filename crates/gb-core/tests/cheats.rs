//! Cheats against a synthetic ROM: Game Genie patches ROM reads, GameShark writes WRAM every frame.
use gb_core::GameBoy;

fn rom() -> Vec<u8> {
    let mut rom = vec![0u8; 0x8000];
    rom[0x100..0x103].copy_from_slice(&[0x00, 0xC3, 0x50]); // NOP; JP $0150
    rom[0x103] = 0x01;
    rom[0x150..0x153].copy_from_slice(&[0xC3, 0x50, 0x01]); // spin
    rom[0x4A17] = 0x42;
    rom
}

/// Inverse of the Game Genie address/compare encoding, to build a code for a known byte.
fn genie_code(value: u8, addr: u16, compare: u8) -> String {
    let a = addr ^ 0xF000;
    let c = (compare ^ 0xBA).rotate_left(2);
    format!(
        "{:X}{:X}{:X}-{:X}{:X}{:X}-{:X}{:X}{:X}",
        value >> 4,
        value & 15,
        (a >> 8) & 15,
        (a >> 4) & 15,
        a & 15,
        a >> 12,
        c >> 4,
        0,
        c & 15
    )
}

#[test]
fn game_genie_patches_rom_reads_only_when_compare_matches() {
    let mut gb = GameBoy::new(rom()).unwrap();
    assert_eq!(gb.bus.peek(0x4A17), 0x42);
    gb.add_cheat(&genie_code(0x99, 0x4A17, 0x41)).unwrap(); // wrong compare value
    assert_eq!(gb.bus.peek(0x4A17), 0x42);
    gb.add_cheat(&genie_code(0x99, 0x4A17, 0x42)).unwrap();
    assert_eq!(gb.bus.peek(0x4A17), 0x99);
    assert_eq!(gb.bus.peek(0x4A18), 0x00);
    gb.clear_cheats();
    assert_eq!(gb.bus.peek(0x4A17), 0x42);
}

#[test]
fn game_genie_survives_load_state_and_affects_cpu_fetches() {
    let mut gb = GameBoy::new(rom()).unwrap();
    gb.add_cheat(&genie_code(0x18, 0x0150, 0xC3)).unwrap(); // JP -> JR
    let state = gb.save_state();
    gb.load_state(&state).unwrap();
    assert_eq!(gb.bus.peek(0x0150), 0x18);
    for _ in 0..20 {
        gb.step();
    }
    assert!(
        gb.registers().pc > 0x153,
        "patched JR left the spin loop, pc={:#X}",
        gb.registers().pc
    );
}

#[test]
fn game_shark_sets_wram_each_frame() {
    let mut gb = GameBoy::new(rom()).unwrap();
    gb.add_cheat("0163D1C0").unwrap();
    assert_eq!(gb.bus.peek(0xC0D1), 0);
    gb.run_frame();
    assert_eq!(gb.bus.peek(0xC0D1), 0x63);
    gb.bus.poke(0xC0D1, 0);
    gb.run_frame();
    assert_eq!(gb.bus.peek(0xC0D1), 0x63);
    assert!(gb.add_cheat("nonsense").is_err());
}
