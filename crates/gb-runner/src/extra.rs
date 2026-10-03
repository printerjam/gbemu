//! Unscored suites: every collection beyond the scored core set, with DMG model filtering.
//! Suites ending in `-cgb` hold the CGB-only tests and run on the CGB model.

use crate::suites::{mooneye_applies_to_dmg, walk, Detect, GambatteExpect, TestCase};
use gb_core::{Button, Model};
use std::path::{Path, PathBuf};

struct Ctx<'a> {
    cases: &'a mut Vec<TestCase>,
}

impl Ctx<'_> {
    fn add(&mut self, suite: &'static str, name: String, rom: PathBuf, detect: Detect, seconds: f64) {
        self.cases.push(TestCase {
            suite,
            name,
            rom,
            detect,
            seconds,
            model: Some(if suite.ends_with("-cgb") {
                Model::Cgb
            } else {
                Model::Dmg
            }),
        });
    }
}

fn stem(p: &Path) -> String {
    p.file_stem().unwrap().to_string_lossy().into_owned()
}

fn rel(root: &Path, p: &Path) -> String {
    let r = p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/");
    r.rsplit_once('.').map_or(r.clone(), |(a, _)| a.to_string())
}

fn sibling(rom: &Path, name: &str) -> Option<PathBuf> {
    Some(rom.with_file_name(name)).filter(|p| p.is_file())
}

/// Gambatte `_out` expectation: what follows `marker` in the file name.
fn gambatte_expect(file_name: &str, marker: &str) -> Option<GambatteExpect> {
    let rest = &file_name[file_name.find(marker)? + marker.len()..];
    if rest.starts_with("audio0") {
        return Some(GambatteExpect::Audio(false));
    }
    if rest.starts_with("audio1") {
        return Some(GambatteExpect::Audio(true));
    }
    let hex: String = rest.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
    // An empty pattern would match any screen; never turn that into a pass.
    (!hex.is_empty()).then_some(GambatteExpect::Hex(hex))
}

fn gambatte(ctx: &mut Ctx, root: &Path) {
    let secs = 16.0 * gb_core::CYCLES_PER_FRAME as f64 / gb_core::CLOCK_HZ as f64;
    for rom in walk(root, &["gb", "gbc"]) {
        let file = rom.file_name().unwrap().to_string_lossy().into_owned();
        let stem = stem(&rom);
        let name = rel(root, &rom);
        // Mirrors gambatte's testrunner.cpp: which models expect which `_out` suffix.
        let (mut dmg, mut cgb) = (None, None);
        if file.contains("dmg08_cgb04c_out") {
            dmg = gambatte_expect(&file, "dmg08_cgb04c_out");
            cgb = dmg.clone();
        } else if file.contains("dmg08_out") {
            dmg = gambatte_expect(&file, "dmg08_out");
            if file.contains("cgb04c_out") {
                cgb = gambatte_expect(&file, "cgb04c_out");
            }
        } else if file.contains("_out") {
            cgb = gambatte_expect(&file, "_out");
        }
        if let Some(e) = dmg {
            ctx.add("gambatte", name.clone(), rom.clone(), Detect::Gambatte(e), secs);
        }
        if let Some(e) = cgb {
            ctx.add("gambatte-cgb", name.clone(), rom.clone(), Detect::Gambatte(e), secs);
        }
        let shared = sibling(&rom, &format!("{stem}_dmg08_cgb04c.png"));
        if let Some(png) = shared.clone().or_else(|| sibling(&rom, &format!("{stem}_dmg08.png"))) {
            ctx.add(
                "gambatte",
                format!("{name}:png"),
                rom.clone(),
                Detect::Gambatte(GambatteExpect::Png(png)),
                secs,
            );
        }
        if let Some(png) = shared.or_else(|| sibling(&rom, &format!("{stem}_cgb04c.png"))) {
            ctx.add(
                "gambatte-cgb",
                format!("{name}:png"),
                rom.clone(),
                Detect::Gambatte(GambatteExpect::Png(png)),
                secs,
            );
        }
    }
}

fn age(ctx: &mut Ctx, root: &Path) {
    for rom in walk(root, &["gb"]) {
        let stem = stem(&rom);
        let name = rel(root, &rom);
        let dmg_png = sibling(&rom, &format!("{stem}-dmgC.png"));
        if let Some(png) = dmg_png {
            // Screenshot test: the ROM is model-agnostic, the reference picks the model.
            ctx.add("age", name, rom, Detect::Acid2 { reference: png }, 10.0);
        } else if stem.contains("dmgC") {
            ctx.add("age", name, rom, Detect::Mooneye, 20.0);
        } else {
            ctx.add("age-cgb", name, rom, Detect::Mooneye, 20.0);
        }
    }
}

fn same_suite(ctx: &mut Ctx, root: &Path) {
    for rom in walk(root, &["gb"]) {
        let name = rel(root, &rom);
        let dmg = mooneye_applies_to_dmg(&stem(&rom))
            && !["apu/", "dma/", "sgb/", "ppu/blocking_bgpi_increase"]
                .iter()
                .any(|d| name.starts_with(d));
        ctx.add(
            if dmg { "same-suite" } else { "same-suite-cgb" },
            name,
            rom,
            Detect::Mooneye,
            10.0,
        );
    }
}

fn mealybug(ctx: &mut Ctx, root: &Path) {
    for rom in walk(root, &["gb"]) {
        let stem = stem(&rom);
        let name = rel(root, &rom);
        if let Some(png) = sibling(&rom, &format!("{stem}_dmg_blob.png")) {
            ctx.add("mealybug", name, rom, Detect::Acid2 { reference: png }, 10.0);
        } else if let Some(png) =
            sibling(&rom, &format!("{stem}_cgb_c.png")).or_else(|| sibling(&rom, &format!("{stem}_cgb_d.png")))
        {
            ctx.add("mealybug-cgb", name, rom, Detect::Acid2 { reference: png }, 10.0);
        }
    }
}

fn screens(ctx: &mut Ctx, roms: &Path) {
    use Button::*;
    // (suite, dir, rom file, reference candidates, buttons, seconds)
    type Entry<'a> = (&'static str, &'a str, &'a str, &'a [&'a str], Vec<Button>, f64);
    let entries: Vec<Entry> = vec![
        (
            "scribbltests",
            "scribbltests/lycscx",
            "lycscx.gb",
            &["lycscx-cgb-dmg.png"],
            vec![],
            2.0,
        ),
        (
            "scribbltests",
            "scribbltests/lycscy",
            "lycscy.gb",
            &["lycscy-cgb-dmg.png"],
            vec![],
            2.0,
        ),
        (
            "scribbltests",
            "scribbltests/palettely",
            "palettely.gb",
            &["palettely-dmg.png"],
            vec![],
            2.0,
        ),
        (
            "scribbltests",
            "scribbltests/scxly",
            "scxly.gb",
            &["scxly-dmg.png"],
            vec![],
            2.0,
        ),
        (
            "scribbltests",
            "scribbltests/statcount",
            "statcount-auto.gb",
            &["statcount_auto-cgb-dmg.png"],
            vec![],
            7.0,
        ),
        (
            "turtle-tests",
            "turtle-tests/window_y_trigger",
            "window_y_trigger.gb",
            &["window_y_trigger.png"],
            vec![],
            2.0,
        ),
        (
            "turtle-tests",
            "turtle-tests/window_y_trigger_wx_offscreen",
            "window_y_trigger_wx_offscreen.gb",
            &["window_y_trigger_wx_offscreen.png"],
            vec![],
            2.0,
        ),
        ("bully", "bully", "bully.gb", &["bully.png"], vec![], 2.0),
        (
            "strikethrough",
            "strikethrough",
            "strikethrough.gb",
            &["strikethrough-dmg.png"],
            vec![],
            2.0,
        ),
        (
            "little-things",
            "little-things-gb",
            "firstwhite.gb",
            &["firstwhite-dmg-cgb.png"],
            vec![],
            2.0,
        ),
        (
            "little-things",
            "little-things-gb",
            "tellinglys.gb",
            &["tellinglys-dmg.png"],
            vec![A, B, Select, Start, Right, Left, Up, Down],
            9.0,
        ),
    ];
    for (suite, dir, file, refs, buttons, seconds) in entries {
        let rom = roms.join(dir).join(file);
        let Some(reference) = refs.iter().map(|r| roms.join(dir).join(r)).find(|p| p.is_file()) else {
            continue;
        };
        if rom.is_file() {
            let name = rel(&roms.join(dir), &rom);
            ctx.add(suite, name, rom, Detect::Screen { reference, buttons }, seconds);
        }
    }
}

fn microtests(ctx: &mut Ctx, root: &Path) {
    for rom in walk(root, &["gb"]) {
        // Only ROMs that write the 0xFF82 result byte (`ldh ($82),a` / `ld ($FF82),a`) have an automatic verdict;
        // the rest were meant for logic analyzers.
        let bytes = std::fs::read(&rom).unwrap_or_default();
        let reports = bytes.windows(2).any(|w| w == [0xE0, 0x82]) || bytes.windows(3).any(|w| w == [0xEA, 0x82, 0xFF]);
        // is_if_set_during_ime0 needs ~380 ms; everything else finishes within two frames.
        let suite = if reports { "gbmicrotest" } else { "gbmicrotest-manual" };
        ctx.add(suite, rel(root, &rom), rom, Detect::Microtest, 0.6);
    }
}

/// wilbertpol naming matches mooneye's, except a bare `-dmg` suffix means the plain DMG.
fn wilbertpol_applies_to_dmg(stem: &str) -> bool {
    stem.ends_with("-dmg") || (!stem.starts_with("mgb_") && mooneye_applies_to_dmg(stem))
}

fn mooneye_family(ctx: &mut Ctx, roms: &Path) {
    let wp = roms.join("mooneye-test-suite-wilbertpol");
    for dir in ["acceptance", "emulator-only", "misc", "madness"] {
        for rom in walk(&wp.join(dir), &["gb"]) {
            if wilbertpol_applies_to_dmg(&stem(&rom)) {
                ctx.add("mooneye-wilbertpol", rel(&wp, &rom), rom, Detect::MooneyeEd, 20.0);
            }
        }
    }
    if let Some(png) = sibling(&wp.join("manual-only/sprite_priority.gb"), "sprite_priority-dmg.png") {
        // This older build never executes LD B,B; compare the screen as it settles.
        ctx.add(
            "mooneye-wilbertpol",
            "manual-only/sprite_priority".into(),
            wp.join("manual-only/sprite_priority.gb"),
            Detect::Screen {
                reference: png,
                buttons: vec![],
            },
            2.0,
        );
    }

    // Mooneye proper: misc/, madness/ and manual-only/ (acceptance and emulator-only are scored elsewhere).
    let m = roms.join("mooneye-test-suite");
    for dir in ["misc", "madness"] {
        for rom in walk(&m.join(dir), &["gb"]) {
            let suite = if wilbertpol_applies_to_dmg(&stem(&rom)) {
                "mooneye-extra"
            } else {
                "mooneye-cgb"
            };
            ctx.add(suite, rel(&m, &rom), rom, Detect::Mooneye, 20.0);
        }
    }
    let sp = m.join("manual-only/sprite_priority.gb");
    if let Some(png) = sibling(&sp, "sprite_priority-dmg.png") {
        ctx.add(
            "mooneye-extra",
            "manual-only/sprite_priority".into(),
            sp,
            Detect::Acid2 { reference: png },
            10.0,
        );
    }
}

pub fn discover(roms: &Path, cases: &mut Vec<TestCase>) {
    let mut ctx = Ctx { cases };
    microtests(&mut ctx, &roms.join("gbmicrotest"));
    gambatte(&mut ctx, &roms.join("gambatte"));
    age(&mut ctx, &roms.join("age-test-roms"));
    same_suite(&mut ctx, &roms.join("same-suite"));
    mealybug(&mut ctx, &roms.join("mealybug-tearoom-tests"));
    screens(&mut ctx, roms);
    mooneye_family(&mut ctx, roms);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gambatte_out_suffixes() {
        let hex = |f: &str, m: &str| match gambatte_expect(f, m) {
            Some(GambatteExpect::Hex(h)) => h,
            other => panic!("{other:?}"),
        };
        assert_eq!(hex("x_dmg08_cgb04c_outE1.gbc", "dmg08_cgb04c_out"), "E1");
        assert_eq!(
            hex("x_dmg08_out65766576_cgb04c_out657655AA.gbc", "dmg08_out"),
            "65766576"
        );
        assert_eq!(
            hex("x_dmg08_out65766576_cgb04c_out657655AA.gbc", "cgb04c_out"),
            "657655AA"
        );
        assert!(matches!(
            gambatte_expect("x_dmg08_outaudio0_cgb04c_outaudio1.gbc", "dmg08_out"),
            Some(GambatteExpect::Audio(false))
        ));
        assert!(matches!(
            gambatte_expect("x_dmg08_outaudio0_cgb04c_outaudio1.gbc", "cgb04c_out"),
            Some(GambatteExpect::Audio(true))
        ));
        // Empty digit string would match any screen: never a test.
        assert!(gambatte_expect("x_dmg08_out.gb", "dmg08_out").is_none());
    }

    #[test]
    fn hex_glyphs_match_only_their_digit() {
        let mut fb = vec![0xFFFFFFu32; 160 * 144];
        for (y, row) in HEX_ROWS_FOR_TEST.iter().enumerate() {
            for x in 0..8 {
                if row & (0x80 >> x) != 0 {
                    fb[y * 160 + x] = 0;
                }
            }
        }
        assert!(crate::exec::hex_matches(&fb, "2"));
        assert!(!crate::exec::hex_matches(&fb, "3"));
        assert!(!crate::exec::hex_matches(&vec![0xFFFFFFu32; 160 * 144], "2"));
    }

    /// Glyph "2" as drawn by gambatte's testrunner.
    const HEX_ROWS_FOR_TEST: [u8; 8] = [0x00, 0x7F, 0x01, 0x01, 0x7F, 0x40, 0x40, 0x7F];

    #[test]
    fn wilbertpol_model_suffixes() {
        assert!(wilbertpol_applies_to_dmg("boot_regs-dmg"));
        assert!(wilbertpol_applies_to_dmg("ly_lyc-GS"));
        assert!(wilbertpol_applies_to_dmg("div_timing"));
        assert!(!wilbertpol_applies_to_dmg("ly_lyc-C"));
        assert!(!wilbertpol_applies_to_dmg("boot_regs-mgb"));
        assert!(!wilbertpol_applies_to_dmg("mgb_oam_dma_halt_sprites"));
    }
}
