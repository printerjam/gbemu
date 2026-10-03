# Progress

## Current state

| # | Milestone | Status | Evidence |
|---|---|---|---|
| 1 | CPU passes cpu_instrs | DONE | blargg cpu_instrs 11/11 + combined (gbtest blargg 19/19); Gameboy Doctor logs match ROMs 1,3-11 (ROM 2: one-instruction IF-dispatch divergence under review) |
| 2 | PPU passes dmg-acid2 | DONE | gbtest acid2 1/1 pixel-exact; mooneye ppu/ 12/12 |
| 3 | MBC1/2/3/5 | DONE | mooneye-mbc 28/28; mbc3 suite 4/4 (rtc3test x3, mbc3-tester incl. MBC30) |
| 4 | Homebrew playable in window | verifying | Tobu Tobu Girl (MIT) reaches gameplay via scripted input on main; real-keyboard (xdotool) + battery round-trip pending |
| 5 | APU | verified, marks after M4 | dmg_sound 13/13; Tobu WAV capture: peak 25453, no clipping, DC -5; desktop ring buffer fed, 59.7 fps audio pacing, no underruns |
| 6 | Terminal renderer | verified render, marks after M4 | gbterm headless ANSI dump of dmg-acid2 decodes to 0 pixel diffs; interactive input check pending |
| 7 | Save states | verified, marks after M4 | 7 state integration tests (determinism, wrong-ROM, corruption); script save/load smoke |
| 8 | CGB | in progress | |
| 9 | WASM | in progress | |

## Scoreboard (`cargo run --release -p gb-runner -- --roms roms`)

| Suite | Passed | Total |
|---|---|---|
| blargg | 19 | 19 |
| mooneye | 64 | 66 |
| mooneye-mbc | 28 | 28 |
| acid2 | 1 | 1 |
| mbc3 | 4 | 4 |

**SCORE: 116/118**

Failing:
| boot_hwio-dmgABCmgb | FAIL | regs B,C,D,E,H,L = 42 42 42 42 42 42 |
| serial/boot_sclk_align-dmgABCmgb | FAIL | regs B,C,D,E,H,L = 42 42 42 42 42 42 |

Unscored `blargg-extra`: 16/22 (dmg_sound 13/13, oam_bug 3/9).

## Tools

- `gbtest` scoreboard; `gbtest run <rom>`; `gbtest trace <rom> --doctor` + `scripts/doctor.sh N` (Gameboy Doctor logs in `out/doctor/`).
- `gbemu <rom>` desktop (minifb+cpal); `gbterm <rom>` terminal; `gbdbg <rom>` debugger TUI (`--script` mode).

## Notes / decisions

- Timer post-boot internal divider is 0xABC8 (only value passing mooneye boot_div-dmgABCmgb under tick-then-access).
- TAC write falling-edge check uses an empirical +8 T look-ahead (timer/rapid_toggle).
- PPU is a scanline renderer with dot-accurate mode timing; pixel FIFO not started.
- `roms/` is write-protected (it was wiped once mid-session; refetch with scripts/fetch-roms.sh).

## Log

- 2026-10-02: skeleton + plan; test ROMs v7.0.
- 2026-10-03: wave 1 merged (CPU, timer, PPU, MBCs, APU, runner, desktop, gbterm, disasm/trace). Score 112/114. EI;HALT dispatch fix (found by Tobu crash).

- Later 2026-10-03: APU audio verified, debugger, save states (serde+postcard), MBC30, mbc3 suite. Score 116/118.

## Next

- Fix dmg_sound 03-trigger regression; unused/boot hwio masks; serial clock alignment; oam_bug.
- Verify gbterm and desktop keyboard/saves; save states; CGB; WASM.
