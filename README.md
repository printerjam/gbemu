# gbemu

A Game Boy (DMG) emulator written from scratch in Rust: CPU, PPU, APU, timer, serial, joypad and the common memory
bank controllers, no emulator crates. `gb-core` has no I/O; its only dependencies are `serde` and `bincode` (save
states). Windowing, audio, terminal and PNG code live in the frontends and tools. Architecture and contracts:
`PLAN.md`; status and scoreboard: `PROGRESS.md`.

## Build

```sh
cargo build --release          # all binaries land in target/release/
cargo test --release           # unit + integration tests (ROM-driven ones skip when roms/ is absent)
```

Needs a recent stable Rust toolchain. `gbemu` links minifb (X11/Wayland on Linux) and cpal (ALSA development
headers on Linux, e.g. `libasound2-dev`); the other binaries only need a terminal.

## Binaries

| Binary | Crate | What it is |
|---|---|---|
| `gbemu` | `gb-desktop` | Windowed frontend with audio, battery saves, save states, rewind |
| `gbterm` | `gb-term` | Terminal frontend (truecolor, half-block rendering) |
| `gbtest` | `gb-runner` | Headless test-ROM runner, scoreboard, single-ROM runner, CPU tracer |
| `gbdbg` | `gb-debug` | TUI debugger with a scriptable command mode |

```sh
target/release/gbemu game.gb [--scale N] [--palette gray|dmg-green|pocket] [--mute]
                             [--screenshot-at-frame N --screenshot out.png] [--exit-after-frames N]
                             [--input-script FILE]
target/release/gbterm game.gb [--palette gray|dmg-green|pocket] [--exit-after-frames N] [--dump-frame-ansi FILE]
target/release/gbdbg game.gb [--script FILE|-]      # `help` inside lists the commands
```

Battery-backed cartridge RAM (and the MBC3 real-time clock) is stored next to the ROM as `<rom>.sav` and written
every few seconds and on exit.

### Keys

| | `gbemu` | `gbterm` |
|---|---|---|
| D-pad | arrow keys | arrow keys or W A S D |
| A / B | X / Z | X or K / Z or J |
| Start | Enter | Enter |
| Select | Backspace or Right Shift | Backspace or Shift+Tab |
| Pause | P | P |
| Quit | Esc | Q or Esc |
| Reset | R | |
| Fast-forward | hold Tab | |
| Screenshot | F12 (`<rom>.N.png`) | |
| Save state | F1..F4 (slots 1..4) | F1 or `[` (slot 1) |
| Load state | Shift+F1..F4 | F2 or `]` (slot 1) |
| Rewind | hold Q | hold R |

`gbterm` needs a terminal with truecolor support; key releases are only reported by terminals that implement the
kitty keyboard protocol, otherwise a pressed key counts as held for a few frames (repeat keeps it held).

### Save states

States are files next to the ROM: `<rom>.ss1` .. `<rom>.ss4`. A state contains the complete machine (CPU, PPU, APU,
timer, memory, cartridge RAM and bank registers, RTC) but not the ROM; it is refused when it belongs to another ROM
(header and global checksum, ROM length), has been corrupted (CRC-32), or was written by an incompatible version.
Display palette and audio sample rate are not part of a state. Library API: `GameBoy::save_state()` /
`GameBoy::load_state(&[u8])`.

### Rewind

Both frontends keep about 20 seconds of history (a snapshot every 4 frames, stored as compressed deltas, a few
hundred bytes each for typical scenes). Hold the rewind key to run time backwards at 4x; release to continue from
that point. Library API: `gb_core::Rewind`.

### Scripted input (`gbemu --input-script FILE`)

One action per line, `#` starts a comment; the frame number counts displayed frames:

```
120 start down     # press, then release, a button
125 start up
300 save 1         # same as F1
400 load 1
500 rewind 25      # step back 25 rewind snapshots, then stay on that frame
```

Combined with `--screenshot-at-frame` and `--exit-after-frames` this gives fully automated runs.

## Test ROMs and the scoreboard

The free test suites ([c-sp/game-boy-test-roms](https://github.com/c-sp/game-boy-test-roms), blargg, mooneye,
dmg-acid2, rtc3test, mbc3-tester and others) are not stored in the repository:

```sh
scripts/fetch-roms.sh           # downloads and unpacks into ./roms
target/release/gbtest           # scored suites: blargg, mooneye, mooneye-mbc, acid2, mbc3; prints SCORE: passed/total
target/release/gbtest --suite mooneye --filter timer     # a subset
target/release/gbtest --suite blargg-extra               # unscored suites (dmg_sound, oam_bug)
target/release/gbtest --markdown --screenshots out/shots # report and failing screenshots
target/release/gbtest run roms/dmg-acid2/dmg-acid2.gb --frames 60 --screenshot acid2.png
```

Mooneye ROMs pass when they execute `LD B,B` with B,C,D,E,H,L = 3,5,8,13,21,34; blargg ROMs when the serial output
says `Passed` (or the screen matches the reference); acid2, rtc3test and mbc3-tester by an exact pixel comparison
with the reference screenshot.

### CPU trace and Gameboy Doctor

```sh
target/release/gbtest trace game.gb --steps 100             # disassembly + registers per instruction
target/release/gbtest trace game.gb --doctor --steps 1000   # Gameboy Doctor log format (LY reads as 0x90)
scripts/doctor.sh 4                                         # diff blargg cpu_instrs/individual/04 against the truth log
```

`scripts/doctor.sh` downloads the truth logs from `robert/gameboy-doctor` into `out/doctor/` on first use and prints
the first diverging line with context.

## Layout

```
crates/gb-core      the emulator core: cpu, ppu, apu, timer, serial, joypad, bus, cartridge, state, rewind, disasm
crates/gb-desktop   gbemu
crates/gb-term      gbterm
crates/gb-runner    gbtest
crates/gb-debug     gbdbg
scripts/            fetch-roms.sh, doctor.sh
```

Supported cartridges: ROM only, MBC1 (including multicarts), MBC2, MBC3 (with RTC, up to MBC30 sizes), MBC5
(including rumble carts). Other mapper types are rejected with `CartError::Unsupported`.
