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
