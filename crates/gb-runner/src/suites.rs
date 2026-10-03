//! Built-in manifest of test suites and ROM discovery.

use gb_core::Model;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub enum Detect {
    /// Serial "Passed"/"Failed", blargg's 0xA000 memory protocol, or screenshot match.
    Blargg { reference: Option<PathBuf> },
    /// `LD B,B` with the Fibonacci register signature.
    Mooneye,
    /// `LD B,B`, then a few frames, then pixel-exact comparison.
    Acid2 { reference: PathBuf },
}

#[derive(Clone, Debug)]
pub struct TestCase {
    pub suite: &'static str,
    pub name: String,
    pub rom: PathBuf,
    pub detect: Detect,
    /// Emulated time limit in seconds.
    pub seconds: f64,
    /// Hardware to emulate; `None` = whatever the cartridge header asks for.
    pub model: Option<Model>,
}

pub struct SuiteInfo {
    pub name: &'static str,
    /// Counts towards `SCORE:`.
    pub scored: bool,
}

pub const SUITES: &[SuiteInfo] = &[
    SuiteInfo {
        name: "blargg",
        scored: true,
    },
    SuiteInfo {
        name: "mooneye",
        scored: true,
    },
    SuiteInfo {
        name: "mooneye-mbc",
        scored: true,
    },
    SuiteInfo {
        name: "acid2",
        scored: true,
    },
    SuiteInfo {
        name: "cgb",
        scored: true,
    },
    SuiteInfo {
        name: "blargg-extra",
        scored: false,
    },
];

pub fn is_scored(suite: &str) -> bool {
    SUITES.iter().any(|s| s.name == suite && s.scored)
}

/// Recursively collect `*.gb` files, sorted.
pub fn walk_gb(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    fn rec(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                rec(&p, out);
            } else if p.extension().is_some_and(|e| e == "gb") {
                out.push(p);
            }
        }
    }
    rec(dir, &mut out);
    out
}

/// Whether a mooneye ROM (by file stem) applies to a DMG-ABC.
///
/// Suffix after the last `-` names the models: uppercase letters are model families
/// (`G` = DMG/MGB, `S` = SGB, `A` = AGB, `C` = CGB, `M` = MGB), lowercase-prefixed
/// forms name specific revisions (`dmgABC`, `dmgABCmgb`, `dmg0`, `mgb`, `sgb2`, `cgb...`).
/// No (recognised) suffix means all models.
pub fn mooneye_applies_to_dmg(stem: &str) -> bool {
    let Some((_, suffix)) = stem.rsplit_once('-') else {
        return true;
    };
    if let Some(rest) = suffix.strip_prefix("dmg") {
        return rest.starts_with(['A', 'B', 'C']);
    }
    if ["mgb", "sgb", "cgb", "agb"].iter().any(|p| suffix.starts_with(p)) {
        return false;
    }
    if !suffix.is_empty() && suffix.chars().all(|c| "GSMAC".contains(c)) {
        return suffix.contains('G');
    }
    true // not a model suffix
}

fn rel_name(root: &Path, p: &Path) -> String {
    let r = p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/");
    r.strip_suffix(".gb").unwrap_or(&r).to_string()
}

/// Build the full manifest rooted at `roms`.
pub fn discover(roms: &Path, long: bool) -> Vec<TestCase> {
    let mut cases = Vec::new();
    let mooneye_secs = if long { 120.0 } else { 20.0 };
    // Howto's DMG time, with margin (a passing test exits early, so margin only costs time on failure).
    let margin = |s: f64| s * 1.5 + 1.0;

    // blargg (scored)
    let b = roms.join("blargg");
    let blargg = |cases: &mut Vec<TestCase>, suite: &'static str, rom: PathBuf, reference: Option<&str>, secs: f64| {
        let reference = reference.map(|r| rom.with_file_name(r)).filter(|r| r.exists());
        cases.push(TestCase {
            suite,
            name: rel_name(&b, &rom),
            rom,
            detect: Detect::Blargg { reference },
            seconds: secs,
            model: None,
        });
    };
    for p in walk_gb(&b.join("cpu_instrs/individual")) {
        blargg(&mut cases, "blargg", p, None, 20.0);
    }
    blargg(
        &mut cases,
        "blargg",
        b.join("cpu_instrs/cpu_instrs.gb"),
        Some("cpu_instrs-dmg-cgb.png"),
        margin(55.0),
    );
    blargg(
        &mut cases,
        "blargg",
        b.join("instr_timing/instr_timing.gb"),
        Some("instr_timing-dmg-cgb.png"),
        margin(1.0),
    );
    for p in walk_gb(&b.join("mem_timing/individual")) {
        blargg(&mut cases, "blargg", p, None, margin(3.0));
    }
    blargg(
        &mut cases,
        "blargg",
        b.join("mem_timing/mem_timing.gb"),
        Some("mem_timing-dmg-cgb.png"),
        margin(3.0),
    );
    blargg(
        &mut cases,
        "blargg",
        b.join("mem_timing-2/mem_timing.gb"),
        Some("mem_timing-dmg-cgb.png"),
        margin(4.0),
    );
    blargg(
        &mut cases,
        "blargg",
        b.join("halt_bug.gb"),
        Some("halt_bug-dmg-cgb.png"),
        margin(2.0),
    );

    // blargg-extra (not scored)
    for p in walk_gb(&b.join("dmg_sound/rom_singles")) {
        blargg(&mut cases, "blargg-extra", p, None, 15.0);
    }
    blargg(
        &mut cases,
        "blargg-extra",
        b.join("dmg_sound/dmg_sound.gb"),
        Some("dmg_sound-dmg.png"),
        margin(36.0),
    );
    for p in walk_gb(&b.join("oam_bug/rom_singles")) {
        blargg(&mut cases, "blargg-extra", p, None, margin(21.0));
    }
    blargg(
        &mut cases,
        "blargg-extra",
        b.join("oam_bug/oam_bug.gb"),
        Some("oam_bug-dmg.png"),
        margin(21.0),
    );
    cases.retain(|c| c.rom.is_file());

    // mooneye
    let m = roms.join("mooneye-test-suite");
    let acc = m.join("acceptance");
    for p in walk_gb(&acc) {
        let stem = p.file_stem().unwrap().to_string_lossy().into_owned();
        if mooneye_applies_to_dmg(&stem) {
            cases.push(TestCase {
                suite: "mooneye",
                name: rel_name(&acc, &p),
                rom: p,
                detect: Detect::Mooneye,
                seconds: mooneye_secs,
                model: None,
            });
        }
    }
    let emu = m.join("emulator-only");
    for p in walk_gb(&emu) {
        let rel = rel_name(&emu, &p);
        if ["mbc1/", "mbc2/", "mbc5/"].iter().any(|d| rel.starts_with(d)) {
            cases.push(TestCase {
                suite: "mooneye-mbc",
                name: rel,
                rom: p,
                detect: Detect::Mooneye,
                seconds: mooneye_secs,
                model: None,
            });
        }
    }

    // acid2
    let a = roms.join("dmg-acid2");
    let rom = a.join("dmg-acid2.gb");
    if rom.is_file() {
        cases.push(TestCase {
            suite: "acid2",
            name: "dmg-acid2".into(),
            rom,
            detect: Detect::Acid2 {
                reference: a.join("dmg-acid2-dmg.png"),
            },
            seconds: 10.0,
            model: None,
        });
    }
    // cgb: CGB-model runs (blargg DMG ROMs run in CGB compatibility mode)
    let cgb_case = |cases: &mut Vec<TestCase>, name: String, rom: PathBuf, detect: Detect, seconds: f64| {
        if rom.is_file() {
            cases.push(TestCase {
                suite: "cgb",
                name,
                rom,
                detect,
                seconds,
                model: Some(Model::Cgb),
            });
        }
    };
    let ca = roms.join("cgb-acid2");
    cgb_case(
        &mut cases,
        "cgb-acid2".into(),
        ca.join("cgb-acid2.gbc"),
        Detect::Acid2 {
            reference: ca.join("cgb-acid2.png"),
        },
        10.0,
    );
    for p in walk_gb(&b.join("cpu_instrs/individual")) {
        cgb_case(
            &mut cases,
            rel_name(&b, &p),
            p,
            Detect::Blargg { reference: None },
            20.0,
        );
    }
    for (rel, reference, secs) in [
        ("cpu_instrs/cpu_instrs.gb", "cpu_instrs-dmg-cgb.png", margin(55.0)),
        ("instr_timing/instr_timing.gb", "instr_timing-dmg-cgb.png", margin(1.0)),
        ("mem_timing/mem_timing.gb", "mem_timing-dmg-cgb.png", margin(3.0)),
        ("mem_timing-2/mem_timing.gb", "mem_timing-dmg-cgb.png", margin(4.0)),
        ("halt_bug.gb", "halt_bug-dmg-cgb.png", margin(2.0)),
        (
            "interrupt_time/interrupt_time.gb",
            "interrupt_time-cgb.png",
            margin(2.0),
        ),
    ] {
        let rom = b.join(rel);
        let reference = Some(rom.with_file_name(reference)).filter(|r| r.exists());
        cgb_case(&mut cases, rel_name(&b, &rom), rom, Detect::Blargg { reference }, secs);
    }
    for p in walk_gb(&b.join("mem_timing/individual")) {
        cgb_case(
            &mut cases,
            rel_name(&b, &p),
            p,
            Detect::Blargg { reference: None },
            margin(3.0),
        );
    }
    cases
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_suffix_filtering() {
        for ok in [
            "div_write",
            "unused_hwio-GS",
            "di_timing-GS",
            "boot_div-dmgABCmgb",
            "boot_regs-dmgABC",
            "boot_hwio-dmgABCmgb",
            "boot_sclk_align-dmgABCmgb",
            "intr_2_0_timing",
            "rom_1Mb",
        ] {
            assert!(mooneye_applies_to_dmg(ok), "{ok} should apply");
        }
        for bad in [
            "boot_div-S",
            "boot_div2-S",
            "boot_div-dmg0",
            "boot_hwio-dmg0",
            "boot_regs-dmg0",
            "boot_regs-mgb",
            "boot_regs-sgb",
            "boot_regs-sgb2",
            "foo-A",
            "foo-C",
            "foo-cgb",
            "foo-cgbABCDE",
            "foo-agb",
            "foo-S",
        ] {
            assert!(!mooneye_applies_to_dmg(bad), "{bad} should be excluded");
        }
    }
}
