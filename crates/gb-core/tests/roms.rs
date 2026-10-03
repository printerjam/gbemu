//! ROM-driven regression tests; skipped when `roms/` is absent.
use gb_core::GameBoy;
use std::path::PathBuf;

fn rom(rel: &str) -> Option<Vec<u8>> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../roms").join(rel);
    std::fs::read(p).ok()
}

fn blargg_serial(rel: &str, max_cycles: u64) -> Option<String> {
    let mut gb = GameBoy::new(rom(rel)?).unwrap();
    while gb.cycles() < max_cycles {
        gb.step();
        let out = gb.serial_output();
        if out.ends_with(b"Passed\n") || out.ends_with(b"Passed all tests\n") || out.windows(6).any(|w| w == b"Failed")
        {
            break;
        }
    }
    Some(String::from_utf8_lossy(gb.serial_output()).into_owned())
}

#[test]
fn cpu_instrs_individual() {
    let Some(_) = rom("blargg/cpu_instrs/cpu_instrs.gb") else {
        return;
    };
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../roms/blargg/cpu_instrs/individual");
    for e in std::fs::read_dir(dir).unwrap() {
        let name = e.unwrap().file_name().into_string().unwrap();
        if name.ends_with(".gb") {
            let out = blargg_serial(&format!("blargg/cpu_instrs/individual/{name}"), 4_194_304 * 60).unwrap();
            assert!(out.contains("Passed"), "{name}: {out}");
        }
    }
}

#[test]
fn cpu_instrs_combined() {
    if let Some(out) = blargg_serial("blargg/cpu_instrs/cpu_instrs.gb", 4_194_304 * 90) {
        assert!(out.contains("Passed all tests"), "{out}");
    }
}

fn roms_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../roms")
}

fn collect(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut v: Vec<_> = rd.map(|e| e.unwrap().path()).collect();
    v.sort();
    for p in v {
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|e| e == "gb") {
            out.push(p);
        }
    }
}

/// Blargg result: serial text or the 0xA000 memory protocol. Returns Ok(pass).
fn run_blargg(path: &std::path::Path) -> bool {
    let mut gb = GameBoy::new(std::fs::read(path).unwrap()).unwrap();
    while gb.cycles() < 4_194_304 * 60 {
        gb.step();
        let out = gb.serial_output();
        if out.windows(6).any(|w| w == b"Passed") {
            return true;
        }
        if out.windows(6).any(|w| w == b"Failed") {
            return false;
        }
        let b = &gb.bus;
        if b.peek(0xA001) == 0xDE && b.peek(0xA002) == 0xB0 && b.peek(0xA003) == 0x61 {
            let r = b.peek(0xA000);
            if r != 0x80 {
                return r == 0;
            }
        }
    }
    false
}

fn run_mooneye(path: &std::path::Path) -> bool {
    run_mooneye_exit(path, 0x40)
}

fn run_mooneye_exit(path: &std::path::Path, exit: u8) -> bool {
    let mut gb = GameBoy::new(std::fs::read(path).unwrap()).unwrap();
    while gb.cycles() < 4_194_304 * 30 {
        if gb.step() == Some(exit) {
            let r = gb.registers();
            return [r.b, r.c, r.d, r.e, r.h, r.l] == [3, 5, 8, 13, 21, 34];
        }
    }
    false
}

fn applies_to_dmg(name: &str) -> bool {
    let stem = name.trim_end_matches(".gb");
    match stem.rsplit_once('-') {
        Some((_, suf)) => matches!(suf, "GS" | "dmgABC" | "dmgABCmgb"),
        None => true,
    }
}

#[test]
#[ignore]
fn scoreboard() {
    let root = roms_dir();
    let mut all = Vec::new();
    for d in [
        "blargg/instr_timing",
        "blargg/mem_timing",
        "blargg/mem_timing-2",
        "blargg/cpu_instrs",
    ] {
        collect(&root.join(d), &mut all);
    }
    all.push(root.join("blargg/halt_bug.gb"));
    let (mut pass, mut total) = (0, 0);
    for p in &all {
        total += 1;
        if run_blargg(p) {
            pass += 1;
        } else {
            println!("BLARGG FAIL {}", p.strip_prefix(&root).unwrap().display());
        }
    }
    println!("blargg {pass}/{total}");
    let mut all = Vec::new();
    collect(&root.join("mooneye-test-suite/acceptance"), &mut all);
    let (mut pass, mut total) = (0, 0);
    for p in &all {
        let rel = p
            .strip_prefix(root.join("mooneye-test-suite/acceptance"))
            .unwrap()
            .to_owned();
        let s = rel.to_string_lossy();
        if s.starts_with("ppu/")
            || s.starts_with("serial/")
            || s.contains("boot_hwio")
            || s.contains("boot_regs") && false
        {
            continue;
        }
        if !applies_to_dmg(p.file_name().unwrap().to_str().unwrap()) {
            continue;
        }
        total += 1;
        if run_mooneye(p) {
            pass += 1;
        } else {
            println!("MOONEYE FAIL {s}");
        }
    }
    println!("mooneye {pass}/{total}");
}
