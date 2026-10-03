# Architecture

How gbemu works, for a reader who knows Rust but not Game Boy internals. `PLAN.md` holds the original contracts,
`PROGRESS.md` the current scoreboard, `docs/DEVLOG.md` the story of how it was built.

## Crate map

| Crate | Binary | Role |
|---|---|---|
| `gb-core` | – | The whole machine. Pure logic: no I/O, no clock, deterministic. Dependencies: `serde` + `bincode` (save states) only, so it also builds for `wasm32`. |
| `gb-desktop` | `gbemu` | Window (minifb), audio (cpal), battery saves, save states, rewind, cheats, input scripts. |
| `gb-term` | `gbterm` | Truecolor terminal renderer (upper-half-block cells, diff-encoded), same hotkeys. |
| `gb-runner` | `gbtest` | Headless test-ROM runner and scoreboard, `run`, `trace` (Gameboy Doctor format), `bench`. |
| `gb-debug` | `gbdbg` | TUI debugger with a scriptable command mode. |
| `gb-wasm` | – | `wasm-bindgen` wrapper; `web/` is the browser frontend (PWA, touch, gamepad, cheats). |

Inside `gb-core` (`crates/gb-core/src`):

| Module | Content |
|---|---|
| `cpu.rs` | SM83 CPU: fetch/decode/execute, interrupt dispatch, HALT, EI delay. Talks to the machine only through the `CpuBus` trait. |
| `bus.rs` | `Bus`: memory map, IF/IE, WRAM/HRAM, OAM DMA, CGB HDMA and speed switch, and the per-M-cycle clock that drives every peripheral. Implements `CpuBus`. |
| `timer.rs`, `serial.rs`, `joypad.rs` | DIV/TIMA, serial port, P1 register. |
| `ppu.rs`, `oam_bug.rs` | Video; the DMG OAM corruption bug. |
| `apu.rs` | Four sound channels, frame sequencer, mixing and resampling. |
| `cartridge.rs` | Header parsing, ROM-only/MBC1/MBC1M/MBC2/MBC3(+RTC, MBC30)/MBC5, battery saves. |
| `gameboy.rs` | `GameBoy` = `Cpu` + `Bus`; the API every frontend uses (`step`, `run_frame`, `framebuffer`, `set_button`, `drain_audio`, `save_state`, `add_cheat`, ...). |
| `state.rs`, `rewind.rs` | Save states and the rewind ring. |
| `cheats.rs` | Game Genie and GameShark. |
| `disasm.rs` | Table-driven disassembler used by `trace` and `gbdbg`. |

## Timing model: one M-cycle per memory access

The CPU never counts cycles. An *M-cycle* is 4 T-cycles (clock ticks at 4.194304 MHz); the CPU's instruction timing is
just the number of bus accesses it performs. Every `CpuBus::read`, `write` or `tick` (an internal delay cycle) first
advances the **whole** machine by one M-cycle, and only then performs the access:

```mermaid
sequenceDiagram
    participant CPU
    participant Bus
    participant Per as Timer / Serial / PPU / APU / DMA / Cart
    Note over CPU: LD A,(HL)   (2 M-cycles)
    CPU->>Bus: read(PC)   // opcode fetch
    Bus->>Per: tick_m(): advance 4 T-cycles
    Per-->>Bus: IF bits to raise
    Bus-->>CPU: opcode byte
    CPU->>Bus: read(HL)   // operand
    Bus->>Per: tick_m(): advance 4 T-cycles
    Per-->>Bus: IF bits to raise
    Bus-->>CPU: data byte
    Note over CPU: instruction ends; next step() checks IME & (IE & IF)
```

Consequences:

- Instruction timing is not a table: `LD A,(HL)` takes two M-cycles because it makes two accesses; `INC HL` makes a
  `tick` for the 16-bit ALU cycle. blargg's `instr_timing` and `mem_timing` and mooneye's timing tests check exactly
  this.
- Because the clock advances *before* the access, a read sees the world as it is at the end of its M-cycle, so
  register reads (LY, DIV, IF, STAT) observe peripheral state with the correct phase.
- Peripherals expose `tick(..) -> u8`: the IF bits they want raised. The bus ORs them into IF. Reads and writes take
  absolute addresses.
- `Bus::peek`/`poke` access memory without advancing the clock (debuggers, DMA source reads, cheats).
- Double speed (CGB) halves the dot count per M-cycle for PPU and APU (2 instead of 4) while the CPU, timer and OAM
  DMA keep one step per M-cycle.

`GameBoy::run_frame` loops `step()` until the PPU reports a finished frame (or one frame's worth of cycles passed
with the LCD off).

## Interrupts, HALT and the quirks we hit

Five sources (VBlank, STAT, Timer, Serial, Joypad) set bits in IF (0xFF0F); IE (0xFFFF) masks them; the CPU's IME
flag gates dispatch. `Cpu::step` runs one instruction, one interrupt dispatch, or one idle HALT cycle.

- **Dispatch** is 5 M-cycles: two internal delay cycles, push PC high byte, push PC low byte, and one more delay before the jump. The vector is chosen
  *after the high-byte push*, because that push can land on IE (0xFFFF) when SP is 0x0000 (**ie_push**, mooneye): if the
  push cleared the enabled bit, dispatch is cancelled and PC becomes 0x0000.
- **IF acknowledge after the low push (normal speed).** The serviced IF bit is cleared after the low-byte push, so a
  push that lands on IF itself (SP = 0xFF10) cannot resurrect the bit (gambatte `irq_precedence/late_if_via_sp_if`
  and the `late_retrigger` family). In double speed the early acknowledge matches the tests better; that acknowledge
  point is still an open item (`CpuBus::double_speed()` selects it).
- **EI delay.** `EI` enables IME only after the *next* instruction. `ei_pending` implements that; `DI` right after
  cancels it.
- **EI;HALT.** If `EI` is followed by `HALT` while an interrupt is already pending, the interrupt is dispatched
  immediately after `HALT` is fetched and the pushed return address is the `HALT` itself, so the interrupt handler
  returns into the `HALT` again. This was found by a real game (Tobu Tobu Girl crashed) and is handled by
  `halt_bug` in `dispatch_interrupt`.
- **HALT bug.** `HALT` with IME=0 and an interrupt pending does not halt, and the next byte is fetched twice
  (PC fails to advance once).
- **STOP** is a two-byte instruction with no low-power state here; on CGB it performs the speed switch when one is armed (KEY1), otherwise it does nothing.
- **OAM bug.** On DMG, a 16-bit inc/dec or an access to 0xFE00-0xFEFF while the PPU scans OAM corrupts OAM rows.
  That is why `CpuBus` has `read_idu`/`tick_idu` (the increment/decrement unit puts its address on the bus); the
  logic is in `oam_bug.rs`.

### Timer and serial

`timer.rs` models DIV as a free-running 16-bit counter whose upper byte is DIV. TIMA counts *falling edges* of
`(DIV bit selected by TAC) & TAC.enable`, so writing DIV or TAC can cause spurious increments. A TIMA overflow reads as
0 for one M-cycle, then reloads from TMA and raises the interrupt. Serial's 8192 Hz internal clock is also derived from
a DIV bit (bit 8, bit 3 for the CGB fast clock), so a transfer started mid-period is aligned to the divider.

## PPU

> This section describes the PPU as of the scanline renderer. The pixel FIFO is being landed by the PPU agent;
> this section is to be updated after that merge.

`Ppu::tick(dots)` advances a dot-level state machine: 456 dots per line, 154 lines (144 visible + 10 VBlank). Each
visible line goes through mode 2 (OAM scan, 80 dots), mode 3 (drawing, 172+ dots depending on SCX, window and sprites)
and mode 0 (HBlank). STAT (0xFF41) mirrors the mode and the LYC comparison and has its own interrupt line, which
rises on the OR of the enabled sources (the "STAT blocking" rule). The CPU's VRAM/OAM access is blocked in the modes
that hardware blocks it, with slightly different windows for reads and writes on DMG.

Currently each line is **rendered in one go when mode 3 ends** (`render_line`): background, window and up to ten
sprites per line are composed from one snapshot of the registers. Mode-3 *length* is computed from SCX, window start
and sprite fetch penalties, so STAT timing is dot-accurate even though pixel production is not. The consequence:
CPU writes to SCX/SCY/LCDC/BGP *during* mode 3 have no per-pixel effect, which is what the gambatte
`scx_during_m3`, `scy`, `bgtiledata`, `bgtilemap` screenshot groups and the mealybug tests measure. A pixel-FIFO
renderer (per-dot fetcher and pixel pipeline) is the planned fix.

Framebuffer: 160x144 `u32` as `0x00RRGGBB`. On DMG the four shades are exactly `0xFFFFFF/0xAAAAAA/0x555555/0x000000`
so screenshots compare bit-exact against the test suites' references; frontends recolor.

## APU

`apu.rs` models the four channels (two square waves with sweep/envelope on channel 1, wave, noise), the 512 Hz frame
sequencer (length, sweep, envelope clocks), NR5x mixing and the DAC/power behaviour tests care about (register read
masks, length counters surviving power-off on DMG, wave RAM access timing).

Sample pipeline: each channel keeps a countdown in T-cycles to its next waveform event. `Apu::tick` jumps from event to
event (and to output-sample boundaries), accumulating the mixed level over each constant stretch. The output is therefore
a **box-filtered downsample of the 4.194304 MHz signal** to the host sample rate (`set_sample_rate`), with no aliasing
from naively picking samples. A DC-blocking high-pass (modelling the output capacitor, DMG charge factor) runs on
the result; samples are stereo interleaved `f32` collected with `drain_audio`. The bus clocks the frame sequencer from
the divider (DIV bit 12 falling edge, bit 13 in double speed), which is why a DIV write can advance it.

## Cartridge

`Cartridge::from_rom` parses the header (title, 0x147 type, 0x148 ROM size, 0x149 RAM size, 0x143 CGB flag, header
checksum) and picks a mapper:

- **ROM-only (+RAM)**, **MBC1** (5-bit + 2-bit banking, mode 0/1 affecting 0x0000 and RAM banking, bank 0 -> 1 on the
  5-bit register only; 1 MiB carts with the Nintendo logo at 0x40104 are detected as **MBC1M** multicarts with 4-bit
  wiring), **MBC2** (built-in 512x4-bit RAM, upper nibble reads 1, address bit 8 selects ROM bank vs RAM enable),
  **MBC3** (RAM banks plus RTC registers; MBC30 widens the ROM bank register to 8 bits and RAM banks to 8 for ROMs
  over 2 MiB), **MBC5** (9-bit ROM bank, bank 0 allowed, rumble bit masked off the RAM bank).
- ROM bank numbers wrap by the real ROM size; disabled or missing RAM reads `0xFF`.
- **RTC:** `Cartridge::tick()` is called once per M-cycle and counts emulated time (1 s = 1,048,576 M-cycles), so
  the clock is deterministic and tied to emulation speed. Latching (0 -> 1 write to 0x6000), halt bit and day-carry
  are implemented; writing seconds resets the sub-second counter.
- **Battery saves:** `save_data()` is the RAM, plus for MBC3 timer carts the common 48-byte footer (5 live + 5 latched
  registers as `u32`, then a 64-bit unix time). Wall-clock catch-up happens only through
  `load_save_data_at(data, now)`, because `gb-core` never reads the clock itself.

## Save states and rewind

`GameBoy::save_state() -> Vec<u8>` / `load_state(&[u8]) -> Result<(), StateError>` (`state.rs`).

```
"GBST" | version u32 LE | ROM header checksum u8 | ROM global checksum u16 LE | ROM length u32 LE | CRC-32 u32 LE | bincode payload
```

- Every stateful core struct derives `Serialize`/`Deserialize`; **new fields must be serializable**. Arrays longer
  than 32 bytes use the `state::bytes` / `boxed_bytes` helpers.
- **Not stored:** ROM bytes and ROM-derived cartridge fields (taken from the running cartridge, which is how a state
  is refused for another ROM), pending audio, and host settings (DMG palette, audio sample rate, cheats).
- **Versioning:** `STATE_VERSION` must be bumped whenever any serialized layout changes (currently 3); a mismatch
  returns `StateError::Version`. Other errors: `BadMagic`, `WrongRom`, `Corrupt` (CRC), `Decode`. A failed load leaves the
  machine untouched.
- **Encoding:** bincode with fixed-width integers (chosen over postcard's varints for rewind, see below).

**Rewind** (`rewind.rs`) keeps a bounded ring of snapshots: one every 4 frames, 300 of them by default (about 20 s). Only the
newest snapshot is stored in full; each older one is stored as the XOR against its younger neighbour with zero-run-length
encoding. Because integers are fixed-width, consecutive snapshots stay byte-aligned and most bytes XOR to zero.
Measured (release): about 0.5 KB and 40 us per snapshot on dmg-acid2 and cpu_instrs.

## Color Game Boy additions

`Model::{Dmg, Cgb}` (`--model`, default follows header byte 0x143 bit 7). On CGB the core adds: banked VRAM (2) and
WRAM (8), BG/OBJ palette RAM with auto-increment, BG attributes (palette, bank, flips, priority), CGB sprite rules,
general and HBlank DMA (HDMA), double-speed mode toggled by STOP + KEY1, the CGB post-boot register state, compat-mode
IO gating for DMG games on a CGB, wave RAM and length-counter differences in the APU, and the CGB serial fast clock.
Reaching double speed changes how many dots a M-cycle is worth (see above) and moves the APU frame-sequencer tap.

## Frontends

All frontends only call the `GameBoy` API and never touch core internals beyond that.

- `gbemu` / `gbterm`: battery RAM written to `<rom>.sav` every few seconds and on exit; `<rom>.ss1..4` save states;
  hold-to-rewind; `--cheat CODE`; `--model`. `gbemu` also has `--input-script` (per-frame button, `save`/`load`/`rewind`
  actions) for automation, screenshots, fast-forward and optional gamepad support.
- Web (`gb-wasm` + `web/`): the browser owns timing (`requestAnimationFrame` at 59.7275 fps with catch-up limit),
  audio goes to an AudioWorklet (ScriptProcessor fallback), saves and states live in `localStorage`, plus per-ROM
  cheats, standard-mapping gamepads, a touch d-pad and PWA offline support (service worker).

## Tests and the scoreboard workflow

- **Unit and integration tests** (`cargo test --release`): per-module tests in `gb-core`, plus `tests/` (ROM-driven
  tests skip when `roms/` is absent; state, rewind, cheats, CGB integration tests).
- **`gbtest`** is the ROM runner. Suites (see `crates/gb-runner/src/suites.rs`, `extra.rs`): the scored set
  (`blargg`, `mooneye`, `mooneye-mbc`, `acid2`, `mbc3`, `cgb`) and unscored wide suites (`blargg-extra`, `gbmicrotest`,
  `gambatte` DMG and CGB, `age`, `same-suite`, `mealybug`, `scribbltests`, `turtle-tests`, `bully`, `strikethrough`,
  `little-things`, `mooneye-wilbertpol`, `mooneye-extra`, `cgb-extra`, ...). Verdicts: serial text (blargg),
  `LD B,B` with the Fibonacci register signature (mooneye), exact screenshot comparison (acid2, rtc3test, mbc3-tester,
  gambatte PNGs), a hex-digit pattern on screen (gambatte `_out` tests), APU output probes (gambatte audio tests).
  Test ROMs come from c-sp/game-boy-test-roms (`scripts/fetch-roms.sh` into `roms/`).
- **Baseline guard:** `scripts/scoreboard.sh` runs every suite, compares per-test results with
  `scoreboard-baseline.json`, exits 1 if any previously passing test now fails or vanishes, and lists newly passing ones;
  `--update` rewrites the baseline.
- **Gameboy Doctor:** `gbtest trace <rom> --doctor` prints the reference log format (state before each instruction,
  LY forced to 0x90); `scripts/doctor.sh N` diffs it against the truth log of blargg cpu_instrs individual ROM N and
  prints the first diverging line with context.
- **Bench:** `gbtest bench <rom> [--frames N]` and `scripts/bench.sh` report frames per second and the multiple of
  real time.
- **CI:** `.github/workflows/ci.yml` runs rustfmt, clippy with `-D warnings`, the workspace tests and `scripts/scoreboard.sh`; `pages.yml` deploys the web build.
