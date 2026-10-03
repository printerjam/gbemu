# Development log

A concise narrative of how gbemu was built. Dates are commit dates from `git log`; scores are the ones recorded in
`PROGRESS.md` and in commit messages at the time.

## Method

A coordinator agent owned `main`, `PLAN.md` and `PROGRESS.md`: it wrote the architecture and the shared contracts
(the `CpuBus` timing model, peripheral `tick`/read/write conventions, the `GameBoy` frontend API), then delegated
subsystems to parallel agents. Each agent worked in its own git worktree (`~/tmp/gbemu-wt/<name>`, branch
`wt/<name>`) and owned specific files (CPU/timer, PPU, APU, cartridge, runner, desktop, ...). Agents committed small
changes on their branch and the coordinator merged them into `main`; anything touching a file one did not own was kept
minimal and reported as a "contract change". Verification was ROM-driven: a milestone counted as done only when its
test ROMs had been run and passed. No emulator crates were used; I/O crates (minifb, cpal, png) live only in the
frontends and tools, and `gb-core` stays dependency-light.

## Timeline

**2026-10-02: skeleton, then the first wave.**
- Skeleton workspace with the core contracts, bus, serial, joypad and `PLAN.md`; test ROMs from c-sp/game-boy-test-roms
  v7.0.
- The CPU and timer landed and passed all 11 blargg `cpu_instrs` ROMs plus the combined ROM; APU channels, the desktop
  frontend, the `gbtest` runner, the cartridge module (MBC1/1M/2/3/5), the PPU (dot state machine, STAT/LYC interrupts,
  scanline renderer) and the terminal frontend followed the same day, along with a disassembler and
  `gbtest trace --doctor` for Gameboy Doctor logs.

**2026-10-03: wave 1 merged; score 112/114.**
- All first-wave branches merged. The scoreboard (blargg, mooneye acceptance for DMG, mooneye MBC suites, dmg-acid2)
  read 112/114; the two failures were `boot_hwio-dmgABCmgb` and `serial/boot_sclk_align-dmgABCmgb`, fixed shortly after
  (serial clock derived from the DIV counter, P1 post-boot value) for 114/114.
- Doctor logs: ROMs 1 and 3-11 matched line for line; ROM 2 diverged at one interrupt-dispatch timing quirk of the
  reference emulator (later reconciled when `trace` was changed to match the reference format).
- Same day: save states and rewind, the debugger (`gbdbg`), MBC30 plus the `mbc3` suite (rtc3test, mbc3-tester),
  CGB support (VRAM/WRAM banks, palettes, HDMA, double speed), the WASM build and web frontend, CI with a scoreboard
  baseline guard, GitHub Pages deployment, the DMG OAM corruption bug, and unscored "wide" suites (gambatte, age,
  mealybug, gbmicrotest, ...).
- Milestones (from `PROGRESS.md`): CPU, PPU, MBC, homebrew playable, APU (dmg_sound 13/13, cgb_sound 13/13),
  terminal renderer, save states, CGB (cgb suite 34/34) and WASM were all marked done; the live Pages build was checked
  in Chromium. Scored suites reached 152/152 after the "Core merge" (mooneye 66/66, oam_bug, DIV-clocked serial).

**Accuracy wave (after 152/152).** With the scored set green, the unscored wide suites became the target (baseline
3513/6161 at the start of the wave, 3590 after the timer rework, 3743 after the OAM DMA work, per commit messages).
Notable steps:
- Gambatte DMG screenshot groups scored ~0%. Investigation showed the runner was fine; the shared cause is the
  scanline renderer sampling registers once per line, so mid-mode-3 writes (`scx_during_m3`, `scy`, `bgtiledata`,
  `bgtilemap`) cannot show up. The pixel-FIFO PPU (`ppu/pipe.rs`) then replaced it: mealybug 1 → 4 of 24, +32 overall,
  with 18 window `late_reenable` tests lost (mid-line window disable is not modelled yet).
- Serial: the DMG edge look-ahead now applies only until DIV is first written (three gambatte serial tests).
- Interrupt dispatch: acknowledging the IF bit after the low-byte push at normal speed fixed `late_if_via_sp_if` and
  the `late_retrigger_2` family (+14 gambatte tests); double-speed acknowledge timing remains open.
- Timer rework: gambatte `tima` went from 80 to 114 of 121.
- OAM DMA bus conflicts (the CPU sees the DMA byte on the shared bus, writes collide); gambatte `oamdma` went from
  117 to 376 of 393.
- Tooling and features: `gbtest bench` and `scripts/bench.sh` (the first measurements showed 300-390 frames/s, i.e. 5-7x
  real time, with the PPU about 57% of sampled time), Game Genie/GameShark cheats, web cheats box, gamepad, touch
  d-pad and PWA offline support, and these docs.
- CGB speed switch: the pause is 2^17 CPU clocks (age spsw-tima, gambatte), not Pan Docs' 2050 M-cycles; HDMA blocks
  start at instruction boundaries and follow gambatte's HALT/HDMA rules.
- APU: PCM12/PCM34, SameBoy-model noise counter, staged sweep and CGB envelope (same-suite-cgb 9 → 68 of 77). Where
  CGB-C (gambatte) and CGB-E (SameSuite) disagree we model CGB-E.
- Perf: PPU bulk-advance through idle HBlank/VBlank and an event-free APU fast path; Tobu runs at about 15x real time.
- End of the session: 152/152 scored, 4115/6161 across every suite in the collection.

## Bugs worth remembering

- **Tobu's EI;HALT crash.** A real game (Tobu Tobu Girl, MIT licensed) crashed during play even though
  every test ROM passed. The cause was `EI` followed by `HALT` with an interrupt already pending: the interrupt must be
  dispatched with the `HALT` itself as the return address (halt bug through dispatch). The fix is in the CPU
  ("EI;HALT with pending interrupt returns to the HALT") with unit tests, found by playing, not by the suites.
- **The `roms/` wipe.** The test-ROM directory was wiped once in the middle of the session. It is symlinked read-only
  into the worktrees and write-protected since; `scripts/fetch-roms.sh` refetches it.
- **The TAC look-ahead.** To pass mooneye `timer/rapid_toggle` the timer first used an empirical +8 T look-ahead on TAC
  writes (recorded in `PROGRESS.md`'s notes, and as a `const AHEAD` in `timer.rs`). It fit the one test but not the
  gambatte `tima` group. The later timer rework removed the look-ahead and replaced it with a principled model:
  TIMA counts falling edges of the selected DIV bit, a write-induced overflow raises its interrupt immediately, and only a
  small, documented lead remains for the newly selected clock bit. Gambatte `tima` rose from 80 to 114 of 121.
- **The DIV phase calibrations.** The post-boot divider (0xABC8 on DMG, 0x2674 on CGB) is calibrated to mooneye
  `boot_div`; the serial clock then needed a 4 T look-ahead for `boot_sclk_align`. Gambatte's serial tests showed that
  look-ahead is only valid until DIV is first written, which led to the `div_synced` flag in the bus.
- **Gameboy Doctor and interrupts.** Doctor expects the interrupt dispatch to be folded into the next instruction's
  log line and LY pinned at 0x90, so `trace --doctor` does both rather than changing the core.
- **State-format churn.** Switching save states from postcard to bincode (fixed-width integers) was driven by rewind:
  with varints, a counter crossing a size boundary shifted every later byte and ruined the XOR deltas.

## Where things stand

See `PROGRESS.md` for the live numbers. Known open items at the time of writing: the pixel-FIFO PPU, double-speed
IF-acknowledge timing, remaining gambatte/mealybug/age groups, and performance (the per-dot PPU is the main hotspot).
