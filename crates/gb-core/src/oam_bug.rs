//! DMG OAM corruption bug (Pan Docs "OAM Corruption Bug").
//!
//! While the PPU scans OAM (mode 2) it reads one 8-byte row per M-cycle. A CPU access to
//! 0xFE00-0xFEFF, or a 16-bit increment/decrement of a register holding such an address, during that
//! window corrupts the row being scanned using the row before it. OAM is a 16-bit-wide memory, so the
//! patterns work on little-endian words.

/// Number of 8-byte rows in OAM.
pub const ROWS: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Write, or increment/decrement without a read (including a write coinciding with one).
    Write,
    /// Plain read.
    Read,
    /// Read in the same M-cycle as an increment/decrement (`ld a,[hli]`, first read of `pop`).
    ReadInc,
}

fn word(oam: &[u8; 0xA0], row: usize, w: usize) -> u16 {
    let i = row * 8 + w * 2;
    u16::from_le_bytes([oam[i], oam[i + 1]])
}

fn set_word(oam: &mut [u8; 0xA0], row: usize, w: usize, v: u16) {
    let i = row * 8 + w * 2;
    oam[i..i + 2].copy_from_slice(&v.to_le_bytes());
}

fn copy_row(oam: &mut [u8; 0xA0], from: usize, to: usize) {
    oam.copy_within(from * 8..from * 8 + 8, to * 8);
}

/// Apply the corruption for an access while the PPU is scanning `row`.
pub fn corrupt(oam: &mut [u8; 0xA0], row: usize, kind: Kind) {
    if row == 0 || row >= ROWS {
        return;
    }
    if kind == Kind::ReadInc && (4..ROWS - 1).contains(&row) {
        let a = word(oam, row - 2, 0);
        let b = word(oam, row - 1, 0);
        let c = word(oam, row, 0);
        let d = word(oam, row - 1, 2);
        set_word(oam, row - 1, 0, (b & (a | c | d)) | (a & c & d));
        copy_row(oam, row - 1, row);
        copy_row(oam, row - 1, row - 2);
    }
    let a = word(oam, row, 0);
    let b = word(oam, row - 1, 0);
    let c = word(oam, row - 1, 2);
    let first = match kind {
        Kind::Write => ((a ^ c) & (b ^ c)) ^ c,
        Kind::Read | Kind::ReadInc => b | (a & c),
    };
    set_word(oam, row, 0, first);
    oam.copy_within((row - 1) * 8 + 2..(row - 1) * 8 + 8, row * 8 + 2);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oam() -> [u8; 0xA0] {
        let mut o = [0u8; 0xA0];
        for (i, b) in o.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(37).wrapping_add(11);
        }
        o
    }

    #[test]
    fn write_pattern_and_row_copy() {
        let mut o = oam();
        let orig = o;
        corrupt(&mut o, 3, Kind::Write);
        let (a, b, c) = (word(&orig, 3, 0), word(&orig, 2, 0), word(&orig, 2, 2));
        assert_eq!(word(&o, 3, 0), ((a ^ c) & (b ^ c)) ^ c);
        assert_eq!(o[3 * 8 + 2..3 * 8 + 8], orig[2 * 8 + 2..2 * 8 + 8]);
        assert_eq!(o[..3 * 8], orig[..3 * 8]);
        assert_eq!(o[4 * 8..], orig[4 * 8..]);
    }

    #[test]
    fn first_row_is_immune() {
        let mut o = oam();
        let orig = o;
        corrupt(&mut o, 0, Kind::Write);
        assert_eq!(o, orig);
    }

    #[test]
    fn read_inc_corrupts_previous_row_then_reads() {
        let mut o = oam();
        let orig = o;
        corrupt(&mut o, 6, Kind::ReadInc);
        let (a, b, c, d) = (
            word(&orig, 4, 0),
            word(&orig, 5, 0),
            word(&orig, 6, 0),
            word(&orig, 5, 2),
        );
        let glitched = (b & (a | c | d)) | (a & c & d);
        assert_eq!(word(&o, 5, 0), glitched);
        assert_eq!(o[4 * 8..5 * 8], o[5 * 8..6 * 8]);
        // Current row: normal read corruption of the copied data.
        let (a2, b2, c2) = (word(&o, 6, 0), word(&o, 5, 0), word(&o, 5, 2));
        let _ = (a2, c2);
        assert_eq!(word(&o, 6, 0), b2 | (c & c2));
    }
}
