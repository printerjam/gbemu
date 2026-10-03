# Progress

## Current state

All milestone checks in this table were run against `main`.

| # | Milestone | Status | Evidence |
|---|---|---|---|
| 1 | CPU passes cpu_instrs | DONE | blargg cpu_instrs 11/11 + combined; Gameboy Doctor logs match |
| 2 | PPU passes dmg-acid2 | DONE | acid2 pixel-exact; mooneye ppu/ 12/12 |
| 3 | MBC1/2/3/5 | DONE | mooneye-mbc 28/28; mbc3 suite 4/4 (rtc3test x3, mbc3-tester incl. MBC30) |
| 4 | Homebrew playable in window | DONE | Tobu Tobu Girl (MIT): real X key events (XTEST) title→gameplay, P/Tab/R/Esc/F12, F1/Shift+F1 live state round-trip, battery .sav write+load proven by option byte change |
| 5 | APU | DONE | dmg_sound 13/13; cgb_sound 13/13; Tobu WAV: peak 25453, no clipping, DC -5; desktop audio pacing 59.7 fps without underruns |
| 6 | Terminal renderer | DONE | gbterm ANSI dump of dmg-acid2 decodes to 0 pixel diffs; tmux-driven key input reaches Tobu gameplay |
| 7 | Save states | DONE | serde+bincode `GBST` v2; determinism/wrong-ROM/corruption tests (DMG+CGB); frontend hotkeys; rewind (≈500 B/snapshot) |
| 8 | CGB | DONE | cgb suite 34/34 (cgb-acid2 pixel-exact, blargg on CGB, cgb_sound); geometrix/brickster render in colour |
| 9 | WASM | DONE | live https://printerjam.github.io/gbemu/ checked in Chromium: dmg-acid2 canvas 0 px diff vs reference, geometrix in CGB colour, Tobu gameplay via keyboard, 60 fps, audio worklet running, no console errors |

## Scoreboard (`scripts/scoreboard.sh`, guards against regressions vs `scoreboard-baseline.json`)

| Suite | Passed | Total |
|---|---|---|
| blargg | 19 | 19 |
| mooneye | 66 | 66 |
| mooneye-mbc | 28 | 28 |
| acid2 | 1 | 1 |
| cgb | 34 | 34 |
| mbc3 | 4 | 4 |

**SCORE: 152/152**

Unscored (all tracked by the baseline, 3513/6161 total): cgb-extra 6/7 (cgb-acid-hell 2 px), blargg-extra 21/22
(oam_bug 7 single overflows its own text buffer), gbmicrotest 347/482, gambatte DMG 1027/1873, gambatte CGB 1855/3352,
mooneye-wilbertpol 78/96, mealybug 1/24 (+0/7 CGB), age 3/14 (+1/33 CGB), same-suite-cgb 9/77, scribbltests 5/5,
turtle 2/2, little-things 1/2, bully 0/1, strikethrough 0/1, mooneye-cgb 3/9.

## Tools

- `gbtest` scoreboard / `run` / `trace --doctor`; `scripts/doctor.sh N`; `scripts/scoreboard.sh [--update]`.
- `gbemu` desktop, `gbterm` terminal, `gbdbg` debugger TUI, `web/` browser build.
- Published: https://github.com/printerjam/gbemu, Pages https://printerjam.github.io/gbemu/

## Notes / decisions

- Timer post-boot divider 0xABC8 (DMG), 0x2674 (CGB) — calibrated to mooneye boot_div.
- TAC write falling-edge check uses an empirical +8 T look-ahead (timer/rapid_toggle).
- PPU: scanline renderer with dot-accurate mode timing; pixel FIFO not started (mealybug, cgb-acid-hell, age m3-* need it).
- Toolchain: Homebrew rust for the workspace; rustup (~/.cargo, wasm32 target, rustfmt/clippy components) for the web build.
- `roms/` is write-protected (it was wiped once mid-session; refetch with scripts/fetch-roms.sh).

## Log

- 2026-10-02: skeleton + plan; test ROMs v7.0.
- 2026-10-03: wave 1 merged (CPU, timer, PPU, MBCs, APU, runner, desktop, gbterm, disasm/trace). EI;HALT dispatch fix.
- 2026-10-03: debugger, save states, rewind, MBC30, CGB, WASM + Pages, CI. Published to GitHub.

- 2026-10-03: Core merge (mooneye 66/66, oam_bug, DIV-clocked serial) → 152/152; wide suites added; Pages verified live.

## Next

- Accuracy wave: pixel FIFO (mealybug, cgb-acid-hell, age m3, gambatte scx_during_m3), gambatte DMG screenshot groups
  (~0 pass: suspect shared cause), timer/oamdma/stat-irq gambatte groups, CGB speed switch (age spsw, gambatte speedchange),
  same-suite APU.
