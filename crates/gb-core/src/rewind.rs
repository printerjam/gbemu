//! Rewind: a bounded history of machine snapshots.
//!
//! Each snapshot is the fixed-width bincode payload of [`GameBoy`] (see `state`). Only the newest snapshot is
//! stored in full; every older one is stored as the XOR against its younger neighbour, zero-run-length encoded.
//! Consecutive frames differ in a few hundred bytes (CPU/PPU/timer registers, a little RAM, the changed part of
//! the framebuffer), so a delta is a few KB instead of ~100 KB.

use crate::gameboy::GameBoy;
use std::collections::VecDeque;

/// Default: snapshot every 4 frames, keep 300 of them (about 20 s of history).
pub const DEFAULT_INTERVAL: u32 = 4;
pub const DEFAULT_CAPACITY: usize = 300;

/// Zero runs shorter than this stay inside a literal run (a run token costs two bytes).
const MIN_ZERO_RUN: usize = 4;

pub struct Rewind {
    interval: u32,
    capacity: usize,
    frames_since: u32,
    /// Newest snapshot, in full.
    newest: Option<Vec<u8>>,
    /// `deltas[i]` turns snapshot `i + 1` into snapshot `i` (oldest first); the last one is relative to `newest`.
    deltas: VecDeque<Vec<u8>>,
    delta_bytes: usize,
}

impl Default for Rewind {
    fn default() -> Self {
        Self::new(DEFAULT_INTERVAL, DEFAULT_CAPACITY)
    }
}

impl Rewind {
    /// Snapshot every `interval` calls to [`Rewind::frame`], keeping at most `capacity` snapshots.
    pub fn new(interval: u32, capacity: usize) -> Self {
        Rewind {
            interval: interval.max(1),
            capacity: capacity.max(1),
            frames_since: 0,
            newest: None,
            deltas: VecDeque::new(),
            delta_bytes: 0,
        }
    }

    /// Number of snapshots available to rewind to.
    pub fn len(&self) -> usize {
        self.deltas.len() + self.newest.is_some() as usize
    }

    pub fn is_empty(&self) -> bool {
        self.newest.is_none()
    }

    /// Approximate memory held by the history.
    pub fn bytes_used(&self) -> usize {
        self.delta_bytes + self.newest.as_ref().map_or(0, Vec::len)
    }

    /// Frames between snapshots.
    pub fn interval(&self) -> u32 {
        self.interval
    }

    pub fn clear(&mut self) {
        self.newest = None;
        self.deltas.clear();
        self.delta_bytes = 0;
        self.frames_since = 0;
    }

    /// Call once per emulated frame; takes a snapshot every `interval`-th call (the first call snapshots).
    pub fn frame(&mut self, gb: &GameBoy) {
        if self.frames_since == 0 {
            self.push(gb);
        }
        self.frames_since = (self.frames_since + 1) % self.interval;
    }

    /// Snapshot `gb` now.
    pub fn push(&mut self, gb: &GameBoy) {
        let snap = gb.snapshot_payload();
        if let Some(prev) = self.newest.replace(snap) {
            let d = encode_delta(&prev, self.newest.as_deref().unwrap());
            self.delta_bytes += d.len();
            self.deltas.push_back(d);
            if self.len() > self.capacity {
                if let Some(old) = self.deltas.pop_front() {
                    self.delta_bytes -= old.len();
                }
            }
        }
    }

    /// Restore the newest snapshot into `gb` and drop it, so the next call goes one snapshot further back.
    /// Returns false when the history is empty. Afterwards the frame counter restarts, so a snapshot is taken at the
    /// next [`Rewind::frame`] call.
    pub fn pop_into(&mut self, gb: &mut GameBoy) -> bool {
        let Some(newest) = self.newest.take() else {
            return false;
        };
        if gb.restore_payload(&newest).is_err() {
            self.newest = Some(newest);
            return false;
        }
        if let Some(d) = self.deltas.pop_back() {
            self.delta_bytes -= d.len();
            self.newest = decode_delta(&newest, &d);
            if self.newest.is_none() {
                self.clear(); // corrupt delta: cannot happen for our own data, but never keep a broken chain
            }
        }
        self.frames_since = 0;
        true
    }
}

fn put_varint(out: &mut Vec<u8>, mut v: usize) {
    while v >= 0x80 {
        out.push(v as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn get_varint(data: &[u8], pos: &mut usize) -> Option<usize> {
    let mut v = 0usize;
    let mut shift = 0;
    loop {
        let b = *data.get(*pos)?;
        *pos += 1;
        v |= ((b & 0x7F) as usize) << shift;
        if b & 0x80 == 0 {
            return Some(v);
        }
        shift += 7;
        if shift > 56 {
            return None;
        }
    }
}

/// Delta that turns `newer` back into `older`: `len(older)`, then `(zero_run, literal_len, literal bytes)` tokens
/// over `older ^ newer` (bytes of `newer` past its end count as 0).
fn encode_delta(older: &[u8], newer: &[u8]) -> Vec<u8> {
    let n = older.len();
    let x = |i: usize| older[i] ^ newer.get(i).copied().unwrap_or(0);
    let mut out = Vec::with_capacity(256);
    put_varint(&mut out, n);
    let mut i = 0;
    while i < n {
        let zero_start = i;
        // Skip zeros, 8 bytes at a time while both buffers agree.
        while i + 8 <= n.min(newer.len()) && older[i..i + 8] == newer[i..i + 8] {
            i += 8;
        }
        while i < n && x(i) == 0 {
            i += 1;
        }
        let zeros = i - zero_start;
        let lit_start = i;
        // Literal run: up to the last nonzero byte before a zero run of MIN_ZERO_RUN (or the end).
        let mut end = i;
        let mut j = i;
        while j < n {
            if x(j) != 0 {
                end = j + 1;
            } else if j + 1 - end >= MIN_ZERO_RUN {
                break;
            }
            j += 1;
        }
        let lit_end = end;
        put_varint(&mut out, zeros);
        put_varint(&mut out, lit_end - lit_start);
        out.extend((lit_start..lit_end).map(x));
        i = lit_end;
    }
    out
}

fn decode_delta(newer: &[u8], delta: &[u8]) -> Option<Vec<u8>> {
    let mut pos = 0;
    let n = get_varint(delta, &mut pos)?;
    let mut older = Vec::with_capacity(n);
    while older.len() < n {
        let zeros = get_varint(delta, &mut pos)?;
        let lits = get_varint(delta, &mut pos)?;
        if older.len() + zeros + lits > n {
            return None;
        }
        let i = older.len();
        older.extend((i..i + zeros).map(|k| newer.get(k).copied().unwrap_or(0)));
        let lit = delta.get(pos..pos + lits)?;
        pos += lits;
        let i = older.len();
        older.extend(
            lit.iter()
                .enumerate()
                .map(|(k, &b)| b ^ newer.get(i + k).copied().unwrap_or(0)),
        );
    }
    Some(older)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u64) -> u64 {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        *seed >> 33
    }

    #[test]
    fn delta_roundtrip_random_and_edge_shapes() {
        let mut seed = 7;
        for case in 0..300 {
            let a_len = (lcg(&mut seed) % 200) as usize + (case % 3) * 700;
            let b_len = match case % 4 {
                0 => a_len,
                1 => a_len + (lcg(&mut seed) % 20) as usize,
                2 => a_len.saturating_sub((lcg(&mut seed) % 20) as usize),
                _ => (lcg(&mut seed) % 50) as usize,
            };
            let older: Vec<u8> = (0..a_len).map(|_| lcg(&mut seed) as u8).collect();
            let mut newer: Vec<u8> = older
                .iter()
                .copied()
                .chain((0..b_len).map(|_| lcg(&mut seed) as u8))
                .collect();
            newer.truncate(b_len);
            // sparse edits so zero runs of every length occur
            for _ in 0..(lcg(&mut seed) % 8) {
                if !newer.is_empty() {
                    let k = lcg(&mut seed) as usize % newer.len();
                    newer[k] ^= 0x5A;
                }
            }
            let d = encode_delta(&older, &newer);
            assert_eq!(decode_delta(&newer, &d).as_deref(), Some(&older[..]), "case {case}");
        }
        assert_eq!(decode_delta(&[1, 2, 3], &encode_delta(&[], &[1, 2, 3])), Some(vec![]));
        assert_eq!(
            decode_delta(&[], &encode_delta(&[9, 0, 0, 0, 0, 0, 0, 1], &[])),
            Some(vec![9, 0, 0, 0, 0, 0, 0, 1])
        );
    }

    #[test]
    fn identical_buffers_encode_tiny() {
        let a = vec![0xAB; 100_000];
        assert!(encode_delta(&a, &a).len() < 16);
    }

    #[test]
    fn rejects_garbage_delta() {
        assert_eq!(decode_delta(&[0; 4], &[5, 9, 9]), None);
        assert_eq!(decode_delta(&[0; 4], &[]), None);
    }
}
