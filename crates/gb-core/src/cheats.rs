//! Cheat codes: Game Genie (ROM read patches) and GameShark (RAM writes applied once per frame).
//!
//! Cheats are host settings, not machine state: they are not part of save states and survive `load_state`.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheatError {
    /// Not a recognised Game Genie / GameShark format.
    Format(String),
    /// Well-formed but the address is outside what the code type may touch.
    Address(u16),
}

impl fmt::Display for CheatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CheatError::Format(s) => write!(
                f,
                "unrecognised cheat code '{s}' (Game Genie: ABC-DEF-GHI or ABC-DEF, GameShark: 01VVLLHH)"
            ),
            CheatError::Address(a) => write!(f, "cheat address {a:#06X} is not patchable"),
        }
    }
}

impl std::error::Error for CheatError {}

/// Replaces the ROM byte at `addr` with `value` (only when the original equals `compare`, if given).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GameGenie {
    pub addr: u16,
    pub value: u8,
    pub compare: Option<u8>,
}

/// Writes `value` to `addr` (A000-DFFF) once per frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GameShark {
    pub addr: u16,
    pub value: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cheat {
    GameGenie(GameGenie),
    GameShark(GameShark),
}

impl Cheat {
    /// Parse `ABC-DEF-GHI` / `ABC-DEF` (Game Genie) or `01VVLLHH` (GameShark), case-insensitive.
    pub fn parse(code: &str) -> Result<Cheat, CheatError> {
        let bad = || CheatError::Format(code.to_string());
        let code = code.trim();
        let digits: Vec<u8> = code
            .chars()
            .filter(|&c| c != '-')
            .map(|c| c.to_digit(16).map(|d| d as u8))
            .collect::<Option<_>>()
            .ok_or_else(bad)?;
        let dashes = code.matches('-').count();
        match (digits.len(), dashes) {
            (8, 0) => {
                if digits[0] != 0 || digits[1] != 1 {
                    return Err(bad());
                }
                let value = digits[2] << 4 | digits[3];
                let addr = u16::from(digits[6]) << 12
                    | u16::from(digits[7]) << 8
                    | u16::from(digits[4]) << 4
                    | u16::from(digits[5]);
                if !(0xA000..=0xDFFF).contains(&addr) {
                    return Err(CheatError::Address(addr));
                }
                Ok(Cheat::GameShark(GameShark { addr, value }))
            }
            (6, 1) | (9, 2) => {
                // Dashes only after digits 3 (and 6).
                let expected: String = code
                    .chars()
                    .enumerate()
                    .filter(|&(i, _)| i == 3 || i == 7)
                    .map(|(_, c)| c)
                    .collect();
                if expected.chars().any(|c| c != '-') || code.len() != digits.len() + dashes {
                    return Err(bad());
                }
                let value = digits[0] << 4 | digits[1];
                let addr = (u16::from(digits[5]) << 12
                    | u16::from(digits[2]) << 8
                    | u16::from(digits[3]) << 4
                    | u16::from(digits[4]))
                    ^ 0xF000;
                if addr > 0x7FFF {
                    return Err(CheatError::Address(addr));
                }
                let compare = (digits.len() == 9).then(|| (digits[6] << 4 | digits[8]).rotate_right(2) ^ 0xBA);
                Ok(Cheat::GameGenie(GameGenie { addr, value, compare }))
            }
            _ => Err(bad()),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Cheats {
    genie: Vec<GameGenie>,
    shark: Vec<GameShark>,
}

impl Cheats {
    pub fn add(&mut self, cheat: Cheat) {
        match cheat {
            Cheat::GameGenie(g) => self.genie.push(g),
            Cheat::GameShark(s) => self.shark.push(s),
        }
    }

    pub fn clear(&mut self) {
        self.genie.clear();
        self.shark.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.genie.is_empty() && self.shark.is_empty()
    }

    #[inline]
    pub fn has_genie(&self) -> bool {
        !self.genie.is_empty()
    }

    /// The byte the CPU sees for a ROM read of `addr` that returned `original`.
    pub fn patch_rom(&self, addr: u16, original: u8) -> u8 {
        for g in &self.genie {
            if g.addr == addr && g.compare.is_none_or(|c| c == original) {
                return g.value;
            }
        }
        original
    }

    pub fn shark(&self) -> &[GameShark] {
        &self.shark
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_genie_nine_digit_with_compare() {
        // Known code: 00A-17B-C49 -> value 0x00, addr = 0x7B1A ^ ... computed below.
        let c = Cheat::parse("00A-17B-C49").unwrap();
        let Cheat::GameGenie(g) = c else { panic!() };
        // digits: 0,0,A,1,7,B,C,4,9 -> addr = (B<<12 | A<<8 | 1<<4 | 7) ^ F000 = 0xBA17 ^ 0xF000 = 0x4A17
        assert_eq!((g.addr, g.value), (0x4A17, 0x00));
        // compare = ror((C<<4)|9, 2) ^ BA = ror(0xC9,2)=0x72 ^ 0xBA = 0xC8
        assert_eq!(g.compare, Some(0xC8));
    }

    #[test]
    fn game_genie_six_digit_and_case() {
        let Cheat::GameGenie(g) = Cheat::parse("fb1-c4e").unwrap() else {
            panic!()
        };
        // value FB, addr = (E<<12 | 1<<8 | C<<4 | 4) ^ F000 = 0xE1C4 ^ 0xF000 = 0x11C4
        assert_eq!((g.addr, g.value, g.compare), (0x11C4, 0xFB, None));
    }

    #[test]
    fn game_shark_parse_and_limits() {
        assert_eq!(
            Cheat::parse("0163D1C0").unwrap(),
            Cheat::GameShark(GameShark {
                addr: 0xC0D1,
                value: 0x63
            })
        );
        assert_eq!(
            Cheat::parse("010000C1").unwrap(),
            Cheat::GameShark(GameShark { addr: 0xC100, value: 0 })
        );
        assert!(matches!(Cheat::parse("01FF0080"), Err(CheatError::Address(0x8000)))); // VRAM is not cheatable
        assert!(matches!(Cheat::parse("01FF00E0"), Err(CheatError::Address(_))));
        assert!(matches!(Cheat::parse("02FF00C0"), Err(CheatError::Format(_))));
    }

    #[test]
    fn rejects_malformed() {
        for bad in [
            "",
            "abc",
            "00A-17B-C4",
            "00A17B-C49",
            "00A-17B-C49-1",
            "00G-17B-C49",
            "0163D1C",
            "0163D1C00",
            "00A-17BC49",
        ] {
            assert!(Cheat::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn genie_patch_and_compare() {
        let mut c = Cheats::default();
        c.add(Cheat::GameGenie(GameGenie {
            addr: 0x1234,
            value: 0x99,
            compare: Some(0x42),
        }));
        assert_eq!(c.patch_rom(0x1234, 0x42), 0x99);
        assert_eq!(c.patch_rom(0x1234, 0x43), 0x43); // wrong bank/original: not patched
        assert_eq!(c.patch_rom(0x1235, 0x42), 0x42);
    }
}
