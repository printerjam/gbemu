# gbemu — plan

Game Boy (DMG, later CGB) emulator in Rust, written from scratch. No emulator
crates; I/O crates (minifb, cpal, png) only in frontends/tools.

## Resuming (fresh context)

1. Read `PROGRESS.md` (scoreboard, current milestone, next step).
2. `scripts/fetch-roms.sh` if `roms/` is missing (gitignored, 172 MB).
3. `cargo run --release -p gb-runner -- --roms roms` prints the scoreboard.
4. `git log --oneline | head -30` for recent work.

## Layout

| Crate | Binary | Role |
|---|---|---|
| `crates/gb-core` | — | Emulation core. No deps, no I/O, wasm-friendly. |
| `crates/gb-runner` | `gbtest` | Headless test-ROM runner, scoreboard, screenshots. |
| `crates/gb-desktop` | `gbemu` | minifb window + cpal audio frontend. |
| `crates/gb-wasm` | — | wasm-bindgen cdylib wrapping `GameBoy` for the browser page in `web/`. |

## Core contracts (gb-core)

Timing model: **M-cycle stepped by memory access.** The CPU never counts
cycles itself; every `CpuBus::read`/`write`/`tick` advances the whole machine
by one M-cycle (4 T-cycles) *before* the access. Instruction timing therefore
equals the access pattern, which is what blargg instr/mem_timing and the
mooneye timing tests check.

- `cpu::CpuBus` — trait the CPU talks to (`read`, `write`, `tick`,
  `pending_interrupts`, `ack_interrupt`). `bus::Bus` implements it.
- `Cpu::step(&mut self, &mut impl CpuBus) -> Option<u8>` — one instruction /
  interrupt dispatch / halted idle cycle. Returns executed opcode (0xCB for
  CB-prefixed), `None` if no instruction ran. Test runners use `Some(0x40)`
  (`LD B,B`) as mooneye/acid2 exit.
- Peripherals expose `tick(..) -> u8` returning IF bits to raise; the bus ORs
  them into IF. Peripheral read/write take absolute addresses.
  - `Timer::tick()` per M-cycle; `div_counter()` is the 16-bit internal divider.
  - `Ppu::tick(dots)`; `read/write_vram`, `read/write_oam` (CPU view, mode
    blocking), `write_oam_dma`, `read/write_reg` (FF40-FF4B, never FF46),
    `framebuffer() -> &[u32]` (160x144, 0x00RRGGBB), `take_frame_ready()`.
  - `Apu::tick(t)`, `frame_sequencer_step()` (bus calls on DIV bit 12 falling
    edge), `read/write` FF10-FF3F, `set_sample_rate`, `drain_samples`
    (stereo interleaved f32).
  - `Serial` records transmitted bytes (`output()`), used by blargg detection.
  - `Cartridge::from_rom`, `read_rom/write_rom/read_ram/write_ram`, `tick()`
    (per M-cycle, RTC), `has_battery`, `save_data`, `load_save_data`.
- Bus owns memory map, IF/IE, WRAM/HRAM, OAM DMA (FF46), `cycles` (T-cycles).
- `GameBoy` (gameboy.rs) is the frontend API: `new(rom)`, `step`,
  `run_frame`, `framebuffer`, `set_button`, `registers`, `cycles`,
  `serial_output`, `set_sample_rate`, `drain_audio`, `cartridge(_mut)`.
- Save states (`state.rs`): `GameBoy::save_state() -> Vec<u8>` /
  `load_state(&[u8]) -> Result<(), StateError>` (postcard payload behind a
  `GBST` header: version, ROM header+global checksum, ROM length, CRC-32).
  **Every stateful core struct derives `Serialize`/`Deserialize`; new fields
  must be serializable** (arrays > 32 bytes use `#[serde(with =
  "crate::state::bytes")]` / `boxed_bytes`; ROM bytes and host settings such as
  the DMG palette and audio sample rate are `serde(skip)` and restored from the
  running machine). Bump `STATE_VERSION` whenever the layout changes.
- No boot ROM: machine starts in documented DMG-ABC post-boot state
  (A=01 F=B0 BC=0013 DE=00D8 HL=014D SP=FFFE PC=0100, DIV counter 0xABCC).
- DMG palette in framebuffer is exactly FFFFFF/AAAAAA/555555/000000 so
  screenshots compare bit-exact with test-suite references. Frontends may
  recolor.

## Scoreboard

`gbtest` runs these and prints pass/total per suite and overall:

- **blargg** (serial output contains `Passed` / screenshot match):
  cpu_instrs (11 individual + combined), instr_timing, mem_timing (3 + combined),
  mem_timing-2, halt_bug. Later: dmg_sound, oam_bug.
- **mooneye acceptance** (LD B,B exit, B/C/D/E/H/L = 3/5/8/13/21/34):
  every `acceptance/**/*.gb` that applies to DMG-ABC (no suffix, `-GS`,
  `-dmgABC`, `-dmgABCmgb`); plus `emulator-only/mbc1|mbc2|mbc5`.
- **dmg-acid2**: framebuffer == `dmg-acid2-dmg.png` after LD B,B.

## Milestones (done only when its test ROMs were run and pass)

1. CPU passes blargg cpu_instrs (all 11 + combined).
2. PPU passes dmg-acid2 (pixel-exact).
3. MBC1/2/3/5 (mooneye emulator-only mbc1/mbc2/mbc5 suites; MBC3 RTC unit-tested / rtc3test).
4. A free homebrew game playable in a window (input, frame pacing, battery saves).
5. APU (blargg dmg_sound; audible in the desktop frontend).
6. Terminal renderer (half-block truecolor).
7. Save states.
8. CGB support (cgb-acid2, CGB blargg variants).
9. WASM build (browser page).

Stretch: mooneye timing tests to 100%, FIFO PPU + mealybug, debugger TUI, rewind.

## Browser build (`crates/gb-wasm` + `web/`)

Homebrew's rustc has no `wasm32` std, so the wasm build uses a separate rustup toolchain that lives in
`~/.cargo/bin` (not on the default PATH; Homebrew's `cargo` keeps building everything else). One-time setup:

```sh
curl -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal \
    --default-toolchain stable -t wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129 --locked   # must equal the wasm-bindgen crate version
```

`scripts/build-web.sh` (add `--serve [PORT]` to also serve `web/`) builds `-p gb-wasm` with the `web` profile
(opt-level 3, fat LTO, panic=abort) and runs `wasm-bindgen --target web` into `web/pkg/` (git-ignored). The page is
plain ES modules, no bundler: ROM picker/drag&drop, canvas scaled with `image-rendering: pixelated`, keyboard +
touch pad, AudioWorklet output (ScriptProcessor fallback on insecure origins), 59.7275 Hz accumulator pacing,
battery saves and save states in `localStorage` keyed by `title:fnv1a(rom)`, palette select. AudioWorklet needs
a secure context (`localhost` counts).

`.github/workflows/pages.yml` publishes `web/` to GitHub Pages on every push to `main`: stable toolchain +
wasm32 target, `wasm-bindgen-cli` installed at the version read from `Cargo.lock` (cached), then the same
`scripts/build-web.sh`. In the repo settings, Pages source must be set to "GitHub Actions".

## Workflow

- Coordinator plans, integrates, verifies; subsystems delegated to subagents.
- Parallel agents work in git worktrees under `~/tmp/gbemu-wt/<name>` on
  branch `wt/<name>`, each owning specific files; coordinator merges into `main`.
- Small commits; `PROGRESS.md` updated with scoreboard after each milestone.
- Only free test ROMs / homebrew. Never commercial ROMs.
