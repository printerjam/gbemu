//! Rewind tests; ROM-driven ones are skipped when `roms/` is absent.
use gb_core::{GameBoy, Rewind};
use std::path::PathBuf;
use std::time::Instant;

fn rom(rel: &str) -> Option<Vec<u8>> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../roms").join(rel);
    std::fs::read(p).ok()
}

fn frame_digest(gb: &GameBoy) -> (Vec<u32>, u64, gb_core::cpu::Registers) {
    (gb.framebuffer().to_vec(), gb.cycles(), *gb.registers())
}

/// Rewinding walks back through exactly the machine states that were snapshotted, and replaying from a
/// rewound state reproduces the original frames.
fn check_rewind(rom_bytes: Vec<u8>, frames: usize, interval: u32) {
    let mut gb = GameBoy::new(rom_bytes).unwrap();
    gb.set_sample_rate(44100);
    let mut rw = Rewind::new(interval, 1000);
    let mut states = Vec::new(); // (full state, digest) per snapshot
    let mut digests = Vec::new(); // digest after each frame
    for f in 0..frames {
        if f % interval as usize == 0 {
            states.push((gb.save_state(), frame_digest(&gb)));
        }
        rw.frame(&gb);
        gb.run_frame();
        digests.push(frame_digest(&gb));
    }
    assert_eq!(rw.len(), states.len());

    // Walk back through all snapshots, newest first.
    let mut gb2 = gb;
    for (state, digest) in states.iter().rev() {
        assert!(rw.pop_into(&mut gb2));
        assert_eq!(&frame_digest(&gb2), digest);
        assert_eq!(
            &gb2.save_state(),
            state,
            "state after rewind is bit-identical to the snapshot"
        );
    }
    assert!(!rw.pop_into(&mut gb2));
    assert!(rw.is_empty());

    // Rewind to the middle and replay: same frames as the first time.
    let mut gb = GameBoy::new(rom_bytes_of(&gb2)).unwrap();
    gb.set_sample_rate(44100);
    let mut rw = Rewind::new(interval, 1000);
    for _ in 0..frames {
        rw.frame(&gb);
        gb.run_frame();
    }
    let back = 5.min(rw.len() - 1);
    for _ in 0..back {
        assert!(rw.pop_into(&mut gb));
    }
    // The machine is now at snapshot index len-1 taken at frame `idx * interval`.
    let idx = rw.len();
    let start = idx * interval as usize;
    for (k, expected) in digests.iter().enumerate().skip(start).take(interval as usize * 6) {
        gb.run_frame();
        assert_eq!(&frame_digest(&gb), expected, "replayed frame {k}");
    }
}

fn rom_bytes_of(gb: &GameBoy) -> Vec<u8> {
    gb.cartridge().rom_bytes().to_vec()
}

#[test]
fn rewind_acid2() {
    let Some(r) = rom("dmg-acid2/dmg-acid2.gb") else { return };
    check_rewind(r, 40, 4);
}

#[test]
fn rewind_blargg_serial_and_interval_one() {
    let Some(r) = rom("blargg/cpu_instrs/individual/06-ld r,r.gb") else {
        return;
    };
    check_rewind(r, 120, 1);
}

#[test]
fn capacity_drops_oldest() {
    let Some(r) = rom("dmg-acid2/dmg-acid2.gb") else { return };
    let mut gb = GameBoy::new(r).unwrap();
    let mut rw = Rewind::new(1, 5);
    let mut cycles = Vec::new();
    for _ in 0..12 {
        rw.frame(&gb);
        cycles.push(gb.cycles());
        gb.run_frame();
    }
    assert_eq!(rw.len(), 5);
    for expect in cycles.iter().rev().take(5) {
        assert!(rw.pop_into(&mut gb));
        assert_eq!(gb.cycles(), *expect);
    }
    assert!(!rw.pop_into(&mut gb));
}

/// Prints bytes per snapshot and time per snapshot/restore. Run with `--ignored --nocapture` in release.
#[test]
#[ignore]
fn cost() {
    for name in ["dmg-acid2/dmg-acid2.gb", "blargg/cpu_instrs/cpu_instrs.gb"] {
        let Some(r) = rom(name) else { continue };
        let mut gb = GameBoy::new(r).unwrap();
        gb.set_sample_rate(44100);
        let mut rw = Rewind::default();
        let n = 600;
        let mut push_time = std::time::Duration::ZERO;
        for _ in 0..n {
            for _ in 0..4 {
                gb.run_frame();
            }
            let t = Instant::now();
            rw.push(&gb);
            push_time += t.elapsed();
        }
        let t = Instant::now();
        let full = gb.save_state().len();
        let save = t.elapsed();
        let t = Instant::now();
        for _ in 0..50 {
            rw.pop_into(&mut gb);
        }
        println!(
            "{name}: snapshots={} used={} KiB ({} B/snapshot avg, full state {} B); push {:?} avg; pop {:?} avg; save_state {:?}",
            rw.len(),
            rw.bytes_used() / 1024,
            rw.bytes_used() / rw.len().max(1),
            full,
            push_time / n,
            t.elapsed() / 50,
            save
        );
    }
}
