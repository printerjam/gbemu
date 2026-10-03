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

Unscored (all tracked by the baseline, 4085/6161 total): gambatte DMG 1346/1873, gambatte CGB 2066/3352,
gbmicrotest 347/482, mooneye-wilbertpol 78/96, same-suite-cgb 46/77, blargg-extra 21/22 (oam_bug 7 single overflows its
own text buffer), cgb-extra 6/7 (cgb-acid-hell 2 px), mealybug 4/24 (+0/7 CGB), age 3/14 (+3/33 CGB), mooneye-cgb 3/9,
scribbltests 5/5, turtle 2/2, little-things 1/2, bully 0/1, strikethrough 0/1.

## Tools

- `gbtest` scoreboard / `run` / `trace --doctor`; `scripts/doctor.sh N`; `scripts/scoreboard.sh [--update]`.
- `gbemu` desktop, `gbterm` terminal, `gbdbg` debugger TUI, `web/` browser build.
- Published: https://github.com/printerjam/gbemu, Pages https://printerjam.github.io/gbemu/

## Notes / decisions

- Timer post-boot divider 0xABC8 (DMG), 0x2674 (CGB) — calibrated to mooneye boot_div.
- TAC write falling-edge check uses an empirical +8 T look-ahead (timer/rapid_toggle).
- PPU: pixel FIFO (ppu/pipe.rs); fetch-step constants fitted to mealybug; mid-line window disable not modelled yet.
- Toolchain: Homebrew rust for the workspace; rustup (~/.cargo, wasm32 target, rustfmt/clippy components) for the web build.
- `roms/` is write-protected (it was wiped once mid-session; refetch with scripts/fetch-roms.sh).

- CGB speed switch: the pause is 2^17 CPU clocks (0x8000 M-cycles) in either direction (gambatte `Memory::stop`, age
  `spsw-tima`: 128 increments of the 4 KHz timer); DIV is reset at its start. OPEN CONFLICT: the TIMA immediate-increment
  sample on that reset is calibrated to gambatte `speedchange_tima0*` (4 KHz sampled one M-cycle before the reset);
  age `spsw-tima-cgbBC` wants every clock one M-cycle later. Both are CGB-C hardware results, so a 1 M-cycle error
  remains somewhere in the model (suspect: interrupt-dispatch/HBlank phase relative to the TIMA edge); gambatte wins (16 vs 1 tests).
- HDMA/GDMA: a transfer unit ends one M-cycle after its last byte; an HBlank block runs at the next instruction
  boundary (`CpuBus::instruction_boundary`); sources in VRAM / 0xE000+ read 0xFF; the destination counter is 16 bits wide and a
  transfer stops at its end. Remaining `hdma_start_2`/`hdma_late_*_2` boundary pairs want the block ~1 M-cycle earlier
  than our HBlank start: PPU mode-0 timing, not bus logic.
- HALT and HBlank DMA (`Bus::halt_changed`, gambatte haltHdmaState): blocks do not run while halted; on wake-up one runs if it was requested at HALT entry or an HBlank began during the halt. Net 0 on the scoreboard (swaps hdma_m3halt_m1unhalt_hdma5, late_m3halt_m2unhalt_*_1 and *_ly_*_6 in, boundary `_2`/`_5` partners out): the `_2` partners want the m0 request ~1 M-cycle earlier (PPU side).

## Log

- 2026-10-02: skeleton + plan; test ROMs v7.0.
- 2026-10-03: wave 1 merged (CPU, timer, PPU, MBCs, APU, runner, desktop, gbterm, disasm/trace). EI;HALT dispatch fix.
- 2026-10-03: debugger, save states, rewind, MBC30, CGB, WASM + Pages, CI. Published to GitHub.

- 2026-10-03: Core merge (mooneye 66/66, oam_bug, DIV-clocked serial) → 152/152; wide suites added; Pages verified live.
- 2026-10-03: accuracy wave 1: timer rework (TAC fit removed), OAM DMA bus conflicts, CGB speed switch (2^17 clocks) + HDMA, CGB APU (PCM12/34, SameSuite), pixel FIFO PPU, cheats, bench, PWA, docs. 3513 → 4085/6161; Tobu bench 15x real time.

## Next

- Accuracy wave: pixel FIFO (mealybug, cgb-acid-hell, age m3, gambatte scx_during_m3), gambatte DMG screenshot groups
  (~0 pass: suspect shared cause), timer/oamdma/stat-irq gambatte groups, CGB speed switch (age spsw, gambatte speedchange),
  same-suite APU.
