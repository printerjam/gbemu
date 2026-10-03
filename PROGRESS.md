# Progress

## Current state

| # | Milestone | Status | Evidence |
|---|---|---|---|
| 1 | CPU passes cpu_instrs | DONE | blargg cpu_instrs 11/11 + combined (gbtest blargg 19/19) |
| 2 | PPU passes dmg-acid2 | DONE | gbtest acid2 1/1 pixel-exact; mooneye ppu/ 12/12 |
| 3 | MBC1/2/3/5 | MBC1/2/5 DONE, MBC3 RTC unit-tested | mooneye-mbc 28/28; rtc3test / mbc3-tester pending |
| 4 | Homebrew playable in window | in verification | Tobu Tobu Girl (MIT) reaches gameplay with scripted input (`homebrew/`, not committed) |
| 5 | APU | in progress | dmg_sound 11/12 singles on main (03-trigger regressed after merge) |
| 6 | Terminal renderer | built, unverified on real ROM | `gbterm` |
| 7 | Save states | todo | |
| 8 | CGB | todo | |
| 9 | WASM | todo | |

## Scoreboard (`cargo run --release -p gb-runner -- --roms roms`)

| Suite | Passed | Total |
|---|---|---|
| blargg | 19 | 19 |
| mooneye | 64 | 66 |
| mooneye-mbc | 28 | 28 |
| acid2 | 1 | 1 |

**SCORE: 112/114**

Failing:
| boot_hwio-dmgABCmgb | FAIL | regs B,C,D,E,H,L = 42 42 42 42 42 42 |
| serial/boot_sclk_align-dmgABCmgb | FAIL | regs B,C,D,E,H,L = 42 42 42 42 42 42 |

Unscored `blargg-extra`: 15/22 (dmg_sound 12/13, oam_bug 3/9).

## Tools

- `gbtest` scoreboard; `gbtest run <rom>`; `gbtest trace <rom> --doctor` + `scripts/doctor.sh N` (Gameboy Doctor logs in `out/doctor/`).
- `gbemu <rom>` desktop (minifb+cpal); `gbterm <rom>` terminal.

## Notes / decisions

- Timer post-boot internal divider is 0xABC8 (only value passing mooneye boot_div-dmgABCmgb under tick-then-access).
- TAC write falling-edge check uses an empirical +8 T look-ahead (timer/rapid_toggle).
- PPU is a scanline renderer with dot-accurate mode timing; pixel FIFO not started.
- `roms/` is write-protected (it was wiped once mid-session; refetch with scripts/fetch-roms.sh).

## Log

- 2026-10-02: skeleton + plan; test ROMs v7.0.
- 2026-10-03: wave 1 merged (CPU, timer, PPU, MBCs, APU, runner, desktop, gbterm, disasm/trace). Score 112/114. EI;HALT dispatch fix (found by Tobu crash).

## Next

- Fix dmg_sound 03-trigger regression; unused/boot hwio masks; serial clock alignment; oam_bug.
- Verify gbterm and desktop keyboard/saves; save states; CGB; WASM.
