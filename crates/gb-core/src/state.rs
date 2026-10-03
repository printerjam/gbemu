//! Save states: the whole machine encoded with bincode (fixed-width integers, so snapshots of a running machine keep their layout and delta-compress well; see `rewind`) behind a small checked header.
//!
//! Layout: `GBST` | version u32 LE | ROM header checksum (0x14D) u8 | ROM global checksum (0x14E-F) u16 LE |
//! ROM length u32 LE | payload CRC-32 u32 LE | bincode payload.
//!
//! Every stateful core struct derives `Serialize`/`Deserialize`; new fields must be serializable.
//! ROM bytes are never stored. Frontend settings (DMG palette, audio sample rate) survive a load.

use crate::gameboy::GameBoy;
use std::fmt;
use std::vec::Vec;

const MAGIC: &[u8; 4] = b"GBST";
/// Bump whenever the serialized layout of any core struct changes.
pub const STATE_VERSION: u32 = 4;
const HEADER_LEN: usize = 4 + 4 + 1 + 2 + 4 + 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateError {
    /// Not a save state (bad magic or truncated header).
    BadMagic,
    /// Written by an incompatible emulator version.
    Version { found: u32, expected: u32 },
    /// State belongs to a different ROM.
    WrongRom,
    /// Payload does not match its checksum.
    Corrupt,
    /// Payload could not be decoded or is inconsistent with the loaded cartridge.
    Decode,
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StateError::BadMagic => write!(f, "not a save state"),
            StateError::Version { found, expected } => {
                write!(f, "save state version {found} is not supported (expected {expected})")
            }
            StateError::WrongRom => write!(f, "save state is for a different ROM"),
            StateError::Corrupt => write!(f, "save state is corrupt (checksum mismatch)"),
            StateError::Decode => write!(f, "save state could not be decoded"),
        }
    }
}

impl std::error::Error for StateError {}

const CRC_TABLE: [u32; 256] = {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
};

/// CRC-32 (IEEE).
fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc = (crc >> 8) ^ CRC_TABLE[((crc ^ b as u32) & 0xFF) as usize];
    }
    !crc
}

/// Identity of the loaded ROM: header checksum, global checksum, length.
fn rom_identity(gb: &GameBoy) -> (u8, u16, u32) {
    let rom = gb.bus.cart.rom_bytes();
    (
        rom[0x14D],
        u16::from_le_bytes([rom[0x14E], rom[0x14F]]),
        rom.len() as u32,
    )
}

impl GameBoy {
    /// Raw encoded machine state without header or checksum, for in-memory snapshots of this same machine
    /// (rewind). Restore with [`GameBoy::restore_payload`].
    pub(crate) fn snapshot_payload(&self) -> Vec<u8> {
        bincode::serialize(self).expect("serializing to a Vec cannot fail")
    }

    /// Restore a payload produced by [`GameBoy::snapshot_payload`] on this machine (same ROM). Host settings and
    /// the ROM are kept. On error the machine is untouched.
    pub(crate) fn restore_payload(&mut self, payload: &[u8]) -> Result<(), StateError> {
        let mut new: GameBoy = bincode::deserialize(payload).map_err(|_| StateError::Decode)?;
        if !new.bus.cart.adopt_rom_from(&mut self.bus.cart) {
            return Err(StateError::Decode);
        }
        // Host settings are not machine state.
        new.bus.ppu.set_palette(self.bus.ppu.palette());
        if new.bus.apu.sample_rate() != self.bus.apu.sample_rate() {
            new.bus.apu.set_sample_rate(self.bus.apu.sample_rate());
        }
        new.bus.cheats = std::mem::take(&mut self.bus.cheats);
        *self = new;
        Ok(())
    }

    /// Snapshot the complete machine state.
    pub fn save_state(&self) -> Vec<u8> {
        let payload = self.snapshot_payload();
        let (hdr, global, len) = rom_identity(self);
        let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&STATE_VERSION.to_le_bytes());
        out.push(hdr);
        out.extend_from_slice(&global.to_le_bytes());
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&crc32(&payload).to_le_bytes());
        out.extend_from_slice(&payload);
        out
    }

    /// Restore a snapshot taken with [`GameBoy::save_state`] on the same ROM. On error the machine is untouched.
    pub fn load_state(&mut self, data: &[u8]) -> Result<(), StateError> {
        if data.len() < HEADER_LEN || &data[..4] != MAGIC {
            return Err(StateError::BadMagic);
        }
        let u32_at = |i: usize| u32::from_le_bytes(data[i..i + 4].try_into().unwrap());
        let version = u32_at(4);
        if version != STATE_VERSION {
            return Err(StateError::Version {
                found: version,
                expected: STATE_VERSION,
            });
        }
        let (hdr, global, len) = rom_identity(self);
        if data[8] != hdr || u16::from_le_bytes([data[9], data[10]]) != global || u32_at(11) != len {
            return Err(StateError::WrongRom);
        }
        let payload = &data[HEADER_LEN..];
        if crc32(payload) != u32_at(15) {
            return Err(StateError::Corrupt);
        }
        self.restore_payload(payload)
    }
}

/// `serde(with = ...)` helper for fixed byte arrays longer than serde's 32-element limit.
pub mod bytes {
    use serde::de::{Error, SeqAccess, Visitor};
    use serde::{Deserializer, Serializer};
    use std::fmt;
    use std::vec::Vec;

    pub fn serialize<S: Serializer, const N: usize>(v: &[u8; N], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(v)
    }

    struct Fixed<const N: usize>;

    impl<'de, const N: usize> Visitor<'de> for Fixed<N> {
        type Value = [u8; N];
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(f, "{N} bytes")
        }
        fn visit_bytes<E: Error>(self, v: &[u8]) -> Result<[u8; N], E> {
            v.try_into().map_err(|_| E::invalid_length(v.len(), &self))
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<[u8; N], A::Error> {
            let mut out = Vec::with_capacity(N);
            while let Some(b) = seq.next_element::<u8>()? {
                out.push(b);
            }
            self.visit_bytes(&out)
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>, const N: usize>(d: D) -> Result<[u8; N], D::Error> {
        d.deserialize_bytes(Fixed::<N>)
    }
}

/// Same as [`bytes`] for `Box<[u8; N]>`.
pub mod boxed_bytes {

    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer, const N: usize>(v: &[u8; N], s: S) -> Result<S::Ok, S::Error> {
        super::bytes::serialize(v, s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>, const N: usize>(d: D) -> Result<Box<[u8; N]>, D::Error> {
        super::bytes::deserialize::<D, N>(d).map(Box::new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_known_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }
}
