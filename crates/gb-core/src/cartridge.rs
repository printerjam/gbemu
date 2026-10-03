//! Cartridge: header parsing and memory bank controllers (ROM-only, MBC1/1M, MBC2, MBC3+RTC, MBC5).

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CartError {
    TooSmall,
    Unsupported(u8),
}

impl fmt::Display for CartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CartError::TooSmall => write!(f, "ROM smaller than a cartridge header"),
            CartError::Unsupported(t) => write!(f, "unsupported cartridge type 0x{t:02X}"),
        }
    }
}

impl std::error::Error for CartError {}

/// Parsed cartridge header (0x0100..0x0150).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub title: String,
    /// 0x143: 0x80 = CGB-enhanced, 0xC0 = CGB-only.
    pub cgb_flag: u8,
    /// 0x147.
    pub cart_type: u8,
    /// 0x148.
    pub rom_size_code: u8,
    /// 0x149.
    pub ram_size_code: u8,
    /// 0x14D as stored in the ROM.
    pub header_checksum: u8,
    /// Checksum computed over 0x134..=0x14C.
    pub computed_checksum: u8,
}

impl Header {
    fn parse(rom: &[u8]) -> Header {
        let computed = rom[0x134..=0x14C]
            .iter()
            .fold(0u8, |x, &b| x.wrapping_sub(b).wrapping_sub(1));
        // CGB carts reuse the last title bytes for flags; stop the title before 0x143 when a CGB flag is present.
        let end = if rom[0x143] & 0x80 != 0 { 0x143 } else { 0x144 };
        Header {
            title: rom[0x134..end]
                .iter()
                .take_while(|&&b| b != 0)
                .map(|&b| b as char)
                .collect(),
            cgb_flag: rom[0x143],
            cart_type: rom[0x147],
            rom_size_code: rom[0x148],
            ram_size_code: rom[0x149],
            header_checksum: rom[0x14D],
            computed_checksum: computed,
        }
    }

    pub fn checksum_ok(&self) -> bool {
        self.header_checksum == self.computed_checksum
    }

    pub fn cgb_supported(&self) -> bool {
        self.cgb_flag & 0x80 != 0
    }

    pub fn cgb_only(&self) -> bool {
        self.cgb_flag == 0xC0
    }

    /// ROM size in bytes declared by the header.
    pub fn rom_size(&self) -> Option<usize> {
        match self.rom_size_code {
            0..=8 => Some(0x8000 << self.rom_size_code),
            0x52 => Some(72 * 0x4000),
            0x53 => Some(80 * 0x4000),
            0x54 => Some(96 * 0x4000),
            _ => None,
        }
    }

    /// External RAM size in bytes declared by the header (not counting MBC2's built-in RAM).
    pub fn ram_size(&self) -> usize {
        match self.ram_size_code {
            1 => 0x800,
            2 => 0x2000,
            3 => 0x8000,
            4 => 0x20000,
            5 => 0x10000,
            _ => 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    None,
    Mbc1,
    Mbc2,
    Mbc3,
    Mbc5,
}

const M_CYCLES_PER_SECOND: u32 = 1_048_576;
const RTC_FOOTER_LEN: usize = 48;

/// MBC3 real-time clock registers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RtcRegs {
    s: u8,
    m: u8,
    h: u8,
    dl: u8,
    /// bit0 = day bit 8, bit6 = halt, bit7 = day carry.
    dh: u8,
}

impl RtcRegs {
    fn halted(&self) -> bool {
        self.dh & 0x40 != 0
    }

    fn tick_second(&mut self) {
        if self.s == 59 {
            self.s = 0;
        } else {
            self.s = (self.s + 1) & 0x3F;
            return;
        }
        if self.m == 59 {
            self.m = 0;
        } else {
            self.m = (self.m + 1) & 0x3F;
            return;
        }
        if self.h == 23 {
            self.h = 0;
        } else {
            self.h = (self.h + 1) & 0x1F;
            return;
        }
        let days = ((self.dh as u16 & 1) << 8 | self.dl as u16) + 1;
        if days >= 512 {
            self.dh |= 0x80;
        }
        self.dl = days as u8;
        self.dh = (self.dh & 0xFE) | ((days >> 8) & 1) as u8;
    }

    fn advance_seconds(&mut self, n: u64) {
        if self.halted() || n == 0 {
            return;
        }
        let days = (self.dh as u64 & 1) << 8 | self.dl as u64;
        let total = days * 86400 + self.h as u64 * 3600 + self.m as u64 * 60 + self.s as u64 + n;
        let days = total / 86400;
        if days >= 512 {
            self.dh |= 0x80;
        }
        let days = days % 512;
        self.dl = days as u8;
        self.dh = (self.dh & 0xFE) | (days >> 8) as u8;
        self.h = (total / 3600 % 24) as u8;
        self.m = (total / 60 % 60) as u8;
        self.s = (total % 60) as u8;
    }

    fn to_u32s(self) -> [u32; 5] {
        [
            self.s as u32,
            self.m as u32,
            self.h as u32,
            self.dl as u32,
            self.dh as u32,
        ]
    }

    fn from_u32s(v: [u32; 5]) -> Self {
        RtcRegs {
            s: v[0] as u8 & 0x3F,
            m: v[1] as u8 & 0x3F,
            h: v[2] as u8 & 0x1F,
            dl: v[3] as u8,
            dh: v[4] as u8 & 0xC1,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct Rtc {
    live: RtcRegs,
    latched: RtcRegs,
    /// M-cycles accumulated within the current second.
    sub: u32,
    /// Last value written to the latch register.
    latch_state: u8,
    /// Unix time associated with the saved state (set by load / `set_unix_time`).
    unix_time: u64,
}

pub struct Cartridge {
    header: Header,
    kind: Kind,
    rom: Vec<u8>,
    ram: Vec<u8>,
    battery: bool,
    rumble: bool,
    multicart: bool,
    /// Number of 16 KiB ROM banks rounded up to a power of two, minus one.
    rom_mask: usize,
    ram_enabled: bool,
    /// MBC1: BANK1 (5 bit). MBC2/MBC3/MBC5: ROM bank register (low bits).
    rom_bank: u16,
    /// MBC1: BANK2 (2 bit). MBC3: RAM bank / RTC select. MBC5: RAM bank.
    aux: u8,
    /// MBC1 banking mode.
    mode: bool,
    rtc: Option<Rtc>,
}

const LOGO_START: [u8; 16] = [
    0xCE, 0xED, 0x66, 0x66, 0xCC, 0x0D, 0x00, 0x0B, 0x03, 0x73, 0x00, 0x83, 0x00, 0x0C, 0x00, 0x0D,
];

impl Cartridge {
    pub fn from_rom(mut rom: Vec<u8>) -> Result<Self, CartError> {
        if rom.len() < 0x150 {
            return Err(CartError::TooSmall);
        }
        let header = Header::parse(&rom);
        let t = header.cart_type;
        // (kind, ram, battery, timer, rumble)
        let (kind, has_ram, battery, timer, rumble) = match t {
            0x00 => (Kind::None, false, false, false, false),
            0x08 => (Kind::None, true, false, false, false),
            0x09 => (Kind::None, true, true, false, false),
            0x01 => (Kind::Mbc1, false, false, false, false),
            0x02 => (Kind::Mbc1, true, false, false, false),
            0x03 => (Kind::Mbc1, true, true, false, false),
            0x05 => (Kind::Mbc2, true, false, false, false),
            0x06 => (Kind::Mbc2, true, true, false, false),
            0x0F => (Kind::Mbc3, false, true, true, false),
            0x10 => (Kind::Mbc3, true, true, true, false),
            0x11 => (Kind::Mbc3, false, false, false, false),
            0x12 => (Kind::Mbc3, true, false, false, false),
            0x13 => (Kind::Mbc3, true, true, false, false),
            0x19 => (Kind::Mbc5, false, false, false, false),
            0x1A => (Kind::Mbc5, true, false, false, false),
            0x1B => (Kind::Mbc5, true, true, false, false),
            0x1C => (Kind::Mbc5, false, false, false, true),
            0x1D => (Kind::Mbc5, true, false, false, true),
            0x1E => (Kind::Mbc5, true, true, false, true),
            _ => return Err(CartError::Unsupported(t)),
        };
        let ram_len = match kind {
            Kind::Mbc2 => 512,
            _ if has_ram => header.ram_size(),
            _ => 0,
        };
        let multicart = kind == Kind::Mbc1 && rom.len() == 0x100000 && rom[0x40104..0x40104 + 16] == LOGO_START;
        let banks = rom.len().div_ceil(0x4000);
        rom.resize(banks * 0x4000, 0xFF);
        Ok(Cartridge {
            header,
            kind,
            rom,
            ram: vec![0; ram_len],
            battery,
            rumble,
            multicart,
            rom_mask: banks.next_power_of_two() - 1,
            ram_enabled: false,
            rom_bank: 1,
            aux: 0,
            mode: false,
            rtc: timer.then(Rtc::default),
        })
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn title(&self) -> String {
        self.header.title.clone()
    }

    /// True when the header advertises CGB support (flag 0x80 or 0xC0).
    pub fn cgb_flag(&self) -> u8 {
        self.header.cgb_flag
    }

    /// True when detected as an MBC1 multicart (MBC1M wiring).
    pub fn is_multicart(&self) -> bool {
        self.multicart
    }

    pub fn has_rtc(&self) -> bool {
        self.rtc.is_some()
    }

    pub fn has_rumble(&self) -> bool {
        self.rumble
    }

    pub fn ram_len(&self) -> usize {
        self.ram.len()
    }

    fn rom_at(&self, bank: usize, off: usize) -> u8 {
        self.rom
            .get((bank & self.rom_mask) * 0x4000 + off)
            .copied()
            .unwrap_or(0xFF)
    }

    /// `addr` in 0x0000..=0x7FFF.
    pub fn read_rom(&self, addr: u16) -> u8 {
        let off = addr as usize & 0x3FFF;
        let high = addr >= 0x4000;
        let bank = match self.kind {
            Kind::None => high as usize,
            Kind::Mbc1 => {
                let shift = if self.multicart { 4 } else { 5 };
                let b2 = (self.aux as usize) << shift;
                if high {
                    let b1 = self.rom_bank as usize & if self.multicart { 0x0F } else { 0x1F };
                    b2 | b1
                } else if self.mode {
                    b2
                } else {
                    0
                }
            }
            Kind::Mbc2 | Kind::Mbc3 | Kind::Mbc5 => {
                if high {
                    self.rom_bank as usize
                } else {
                    0
                }
            }
        };
        self.rom_at(bank, off)
    }

    /// MBC register writes (`addr` in 0x0000..=0x7FFF).
    pub fn write_rom(&mut self, addr: u16, val: u8) {
        match self.kind {
            Kind::None => {}
            Kind::Mbc1 => match addr >> 13 {
                0 => self.ram_enabled = val & 0xF == 0xA,
                1 => self.rom_bank = if val & 0x1F == 0 { 1 } else { (val & 0x1F) as u16 },
                2 => self.aux = val & 3,
                _ => self.mode = val & 1 != 0,
            },
            Kind::Mbc2 => {
                if addr < 0x4000 {
                    if addr & 0x100 == 0 {
                        self.ram_enabled = val & 0xF == 0xA;
                    } else {
                        let b = (val & 0xF) as u16;
                        self.rom_bank = if b == 0 { 1 } else { b };
                    }
                }
            }
            Kind::Mbc3 => match addr >> 13 {
                0 => self.ram_enabled = val & 0xF == 0xA,
                1 => {
                    // MBC30 (ROMs over 2 MiB) has an 8-bit ROM bank register, plain MBC3 has 7 bits.
                    let mask = if self.rom.len() > 0x20_0000 { 0xFF } else { 0x7F };
                    self.rom_bank = if val & mask == 0 { 1 } else { (val & mask) as u16 };
                }
                2 => self.aux = val,
                _ => {
                    if let Some(rtc) = &mut self.rtc {
                        if rtc.latch_state == 0 && val == 1 {
                            rtc.latched = rtc.live;
                        }
                        rtc.latch_state = val;
                    }
                }
            },
            Kind::Mbc5 => match addr >> 12 {
                0 | 1 => self.ram_enabled = val == 0x0A,
                2 => self.rom_bank = (self.rom_bank & 0x100) | val as u16,
                3 => self.rom_bank = (self.rom_bank & 0xFF) | ((val as u16 & 1) << 8),
                4 | 5 => self.aux = val & if self.rumble { 0x07 } else { 0x0F },
                _ => {}
            },
        }
    }

    /// Offset into `self.ram` for an address in 0xA000..=0xBFFF, if RAM is accessible there.
    fn ram_offset(&self, addr: u16) -> Option<usize> {
        if self.ram.is_empty() {
            return None;
        }
        let off = addr as usize & 0x1FFF;
        let bank = match self.kind {
            Kind::None | Kind::Mbc2 => 0,
            Kind::Mbc1 => {
                if self.mode {
                    self.aux as usize
                } else {
                    0
                }
            }
            Kind::Mbc3 | Kind::Mbc5 => self.aux as usize,
        };
        Some((bank * 0x2000 + off) % self.ram.len())
    }

    /// Highest selectable RAM bank: 3 on MBC3, 7 on MBC30.
    fn mbc3_max_ram_bank(&self) -> u8 {
        if self.rom.len() > 0x20_0000 {
            7
        } else {
            3
        }
    }

    fn rtc_select(&self) -> Option<u8> {
        if self.kind == Kind::Mbc3 && self.rtc.is_some() && (0x08..=0x0C).contains(&self.aux) {
            Some(self.aux)
        } else {
            None
        }
    }

    /// `addr` in 0xA000..=0xBFFF.
    pub fn read_ram(&self, addr: u16) -> u8 {
        if !self.ram_enabled {
            return 0xFF;
        }
        if self.kind == Kind::Mbc2 {
            return self.ram[addr as usize & 0x1FF] | 0xF0;
        }
        if let Some(sel) = self.rtc_select() {
            let r = &self.rtc.as_ref().unwrap().latched;
            return match sel {
                0x08 => r.s,
                0x09 => r.m,
                0x0A => r.h,
                0x0B => r.dl,
                _ => r.dh,
            };
        }
        if self.kind == Kind::Mbc3 && self.aux > self.mbc3_max_ram_bank() {
            return 0xFF;
        }
        match self.ram_offset(addr) {
            Some(o) => self.ram[o],
            None => 0xFF,
        }
    }

    pub fn write_ram(&mut self, addr: u16, val: u8) {
        if !self.ram_enabled {
            return;
        }
        if self.kind == Kind::Mbc2 {
            self.ram[addr as usize & 0x1FF] = val & 0xF;
            return;
        }
        if let Some(sel) = self.rtc_select() {
            let rtc = self.rtc.as_mut().unwrap();
            let r = &mut rtc.live;
            match sel {
                0x08 => {
                    r.s = val & 0x3F;
                    rtc.sub = 0;
                }
                0x09 => r.m = val & 0x3F,
                0x0A => r.h = val & 0x1F,
                0x0B => r.dl = val,
                _ => r.dh = val & 0xC1,
            }
            return;
        }
        if self.kind == Kind::Mbc3 && self.aux > self.mbc3_max_ram_bank() {
            return;
        }
        if let Some(o) = self.ram_offset(addr) {
            self.ram[o] = val;
        }
    }

    /// Advance one M-cycle (MBC3 RTC).
    #[inline]
    pub fn tick(&mut self) {
        if let Some(rtc) = &mut self.rtc {
            if !rtc.live.halted() {
                rtc.sub += 1;
                if rtc.sub >= M_CYCLES_PER_SECOND {
                    rtc.sub = 0;
                    rtc.live.tick_second();
                }
            }
        }
    }

    pub fn has_battery(&self) -> bool {
        self.battery
    }

    /// Set the unix time recorded in the RTC save footer (gb-core never reads the clock itself).
    pub fn set_unix_time(&mut self, now: u64) {
        if let Some(rtc) = &mut self.rtc {
            rtc.unix_time = now;
        }
    }

    /// Battery-backed state to persist (.sav contents): RAM, plus a 48-byte VBA/BGB RTC footer for MBC3 timer carts.
    pub fn save_data(&self) -> Vec<u8> {
        let mut out = self.ram.clone();
        if let Some(rtc) = &self.rtc {
            for v in rtc.live.to_u32s().into_iter().chain(rtc.latched.to_u32s()) {
                out.extend_from_slice(&v.to_le_bytes());
            }
            out.extend_from_slice(&rtc.unix_time.to_le_bytes());
        }
        out
    }

    /// Restore RAM (and RTC, when a footer is present) without wall-clock catch-up.
    pub fn load_save_data(&mut self, data: &[u8]) {
        self.load_inner(data, None);
    }

    /// Like `load_save_data`, but advances the RTC by the wall-clock time elapsed since the save (`now` is unix seconds).
    pub fn load_save_data_at(&mut self, data: &[u8], now: u64) {
        self.load_inner(data, Some(now));
    }

    fn load_inner(&mut self, data: &[u8], now: Option<u64>) {
        let n = data.len().min(self.ram.len());
        self.ram[..n].copy_from_slice(&data[..n]);
        if self.kind == Kind::Mbc2 {
            for b in &mut self.ram {
                *b &= 0xF;
            }
        }
        let ram_len = self.ram.len();
        let Some(rtc) = &mut self.rtc else { return };
        let footer = data.get(ram_len..).unwrap_or(&[]);
        if footer.len() < 44 {
            return;
        }
        let word = |i: usize| u32::from_le_bytes(footer[i * 4..i * 4 + 4].try_into().unwrap());
        rtc.live = RtcRegs::from_u32s(std::array::from_fn(word));
        rtc.latched = RtcRegs::from_u32s(std::array::from_fn(|i| word(i + 5)));
        rtc.unix_time = if footer.len() >= RTC_FOOTER_LEN {
            u64::from_le_bytes(footer[40..48].try_into().unwrap())
        } else {
            word(10) as u64
        };
        rtc.sub = 0;
        if let Some(now) = now {
            rtc.live.advance_seconds(now.saturating_sub(rtc.unix_time));
            rtc.unix_time = now;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ROM with `banks` 16 KiB banks; even offsets hold the bank's low byte, odd offsets the high byte.
    fn make_rom(cart_type: u8, banks: usize, ram_code: u8) -> Vec<u8> {
        let mut rom = vec![0u8; banks * 0x4000];
        for b in 0..banks {
            for i in (0..0x4000).step_by(2) {
                rom[b * 0x4000 + i] = b as u8;
                rom[b * 0x4000 + i + 1] = (b >> 8) as u8;
            }
        }
        rom[0x147] = cart_type;
        rom[0x148] = (banks / 2).trailing_zeros() as u8;
        rom[0x149] = ram_code;
        rom
    }

    fn cart(t: u8, banks: usize, ram: u8) -> Cartridge {
        Cartridge::from_rom(make_rom(t, banks, ram)).unwrap()
    }

    fn lo(c: &Cartridge, a: u16) -> u8 {
        c.read_rom(a)
    }

    #[test]
    fn header_and_errors() {
        let mut rom = make_rom(0x03, 4, 3);
        rom[0x134..0x13A].copy_from_slice(b"TETRIS");
        let sum = rom[0x134..=0x14C]
            .iter()
            .fold(0u8, |x, &b| x.wrapping_sub(b).wrapping_sub(1));
        rom[0x14D] = sum;
        let c = Cartridge::from_rom(rom).unwrap();
        let h = c.header();
        assert_eq!(c.title(), "TETRIS");
        assert!(h.checksum_ok());
        assert_eq!((h.cart_type, h.rom_size(), h.ram_size()), (3, Some(0x10000), 0x8000));
        assert!(c.has_battery());
        assert_eq!(Cartridge::from_rom(vec![0; 10]).err(), Some(CartError::TooSmall));
        assert_eq!(
            Cartridge::from_rom(make_rom(0xFC, 2, 0)).err(),
            Some(CartError::Unsupported(0xFC))
        );
    }

    #[test]
    fn rom_only_ram() {
        let mut c = cart(0x08, 2, 2);
        assert_eq!(lo(&c, 0x5000), 1);
        c.write_rom(0x2000, 0); // ignored
        assert_eq!(lo(&c, 0x5000), 1);
        c.write_ram(0xA000, 5);
        assert_eq!(c.read_ram(0xA000), 0xFF); // disabled
        let c2 = cart(0x00, 2, 0);
        assert_eq!(c2.read_ram(0xA000), 0xFF);
        assert!(!c2.has_battery());
    }

    #[test]
    fn mbc1_banking() {
        let mut c = cart(0x01, 64, 0); // 1 MiB, not multicart (no logo)
        assert_eq!(lo(&c, 0x5000), 1);
        c.write_rom(0x2000, 0);
        assert_eq!(lo(&c, 0x5000), 1); // 0 -> 1
        c.write_rom(0x2000, 0x1F);
        assert_eq!(lo(&c, 0x5000), 0x1F);
        c.write_rom(0x4000, 1);
        assert_eq!(lo(&c, 0x5000), 0x3F);
        assert_eq!(lo(&c, 0x1000), 0); // mode 0: bank 0 fixed
        c.write_rom(0x6000, 1);
        assert_eq!(lo(&c, 0x1000), 0x20); // mode 1: bank2 affects 0x0000 region
        c.write_rom(0x2000, 0x20); // low 5 bits 0 -> 1, bank2 kept
        assert_eq!(lo(&c, 0x5000), 0x21);
        c.write_rom(0x2000, 0xE3); // upper bits ignored
        assert_eq!(lo(&c, 0x5000), 0x23);
    }

    #[test]
    fn mbc1_wraps_by_rom_size() {
        let mut c = cart(0x01, 4, 0);
        c.write_rom(0x2000, 0x06);
        assert_eq!(lo(&c, 0x5000), 2);
        c.write_rom(0x2000, 0x04);
        assert_eq!(lo(&c, 0x5000), 0); // bank 4 wraps to 0 (no 0->1 after masking)
        c.write_rom(0x4000, 3);
        c.write_rom(0x6000, 1);
        assert_eq!(lo(&c, 0x1000), 0);
    }

    #[test]
    fn mbc1_ram_modes_and_enable() {
        let mut c = cart(0x03, 4, 3);
        c.write_ram(0xA000, 1);
        assert_eq!(c.read_ram(0xA000), 0xFF);
        c.write_rom(0x0000, 0x0A);
        c.write_ram(0xA000, 0x11);
        assert_eq!(c.read_ram(0xA000), 0x11);
        c.write_rom(0x4000, 2);
        assert_eq!(c.read_ram(0xA000), 0x11); // mode 0: always bank 0
        c.write_rom(0x6000, 1);
        assert_eq!(c.read_ram(0xA000), 0);
        c.write_ram(0xA000, 0x22);
        c.write_rom(0x6000, 0);
        assert_eq!(c.read_ram(0xA000), 0x11);
        c.write_rom(0x0000, 0x1A); // low nibble 0xA enables
        assert_eq!(c.read_ram(0xA000), 0x11);
        c.write_rom(0x0000, 0x00);
        assert_eq!(c.read_ram(0xA000), 0xFF);
    }

    #[test]
    fn mbc1_small_ram_mirrors() {
        let mut c = cart(0x02, 2, 1); // 2 KiB
        c.write_rom(0, 0x0A);
        c.write_ram(0xA000, 7);
        assert_eq!(c.read_ram(0xA800), 7);
    }

    #[test]
    fn mbc1_multicart() {
        let mut rom = make_rom(0x01, 64, 0);
        rom[0x40104..0x40104 + 16].copy_from_slice(&LOGO_START);
        let mut c = Cartridge::from_rom(rom).unwrap();
        assert!(c.is_multicart());
        c.write_rom(0x2000, 0x1F); // only 4 bits used
        assert_eq!(lo(&c, 0x5000), 0x0F);
        c.write_rom(0x4000, 1);
        assert_eq!(lo(&c, 0x5000), 0x1F); // bank2 << 4
        c.write_rom(0x6000, 1);
        assert_eq!(lo(&c, 0x1000), 0x10);
        c.write_rom(0x4000, 3);
        assert_eq!(lo(&c, 0x1000), 0x30);
    }

    #[test]
    fn mbc2_behaviour() {
        let mut c = cart(0x06, 16, 0);
        c.write_ram(0xA000, 0x0A);
        assert_eq!(c.read_ram(0xA000), 0xFF);
        c.write_rom(0x0000, 0x0A); // A8 clear: RAM enable
        c.write_rom(0x0100, 0x05); // A8 set: ROM bank
        assert_eq!(lo(&c, 0x5000), 5);
        c.write_rom(0x2100, 0); // 0 -> 1
        assert_eq!(lo(&c, 0x5000), 1);
        c.write_rom(0x2000, 0x07); // A8 clear: not a bank write
        assert_eq!(lo(&c, 0x5000), 1);
        c.write_rom(0x2000, 0x0A);
        c.write_ram(0xA005, 0xAB);
        assert_eq!(c.read_ram(0xA005), 0xFB);
        assert_eq!(c.read_ram(0xA205), 0xFB); // mirrored
        assert_eq!(c.read_ram(0xBE05), 0xFB);
        c.write_rom(0x0000, 0x00);
        assert_eq!(c.read_ram(0xA005), 0xFF);
        assert!(c.has_battery());
        assert_eq!(c.save_data().len(), 512);
        c.write_rom(0x2100, 0x0F);
        c.write_rom(0x2100, 0x10 | 3); // 4 bits only
        assert_eq!(lo(&c, 0x5000), 3);
    }

    #[test]
    fn mbc3_banking_and_ram() {
        let mut c = cart(0x13, 128, 3);
        c.write_rom(0x2000, 0);
        assert_eq!(lo(&c, 0x5000), 1);
        c.write_rom(0x2000, 0x7F);
        assert_eq!(lo(&c, 0x5000), 0x7F);
        c.write_rom(0x2000, 0x80 | 5); // 7 bits
        assert_eq!(lo(&c, 0x5000), 5);
        assert_eq!(lo(&c, 0x1000), 0);
        c.write_rom(0, 0x0A);
        for b in 0..4u8 {
            c.write_rom(0x4000, b);
            c.write_ram(0xA010, 0x40 + b);
        }
        c.write_rom(0x4000, 2);
        assert_eq!(c.read_ram(0xA010), 0x42);
        c.write_rom(0x4000, 5); // invalid select
        assert_eq!(c.read_ram(0xA010), 0xFF);
    }

    #[test]
    fn mbc30_wide_banks() {
        let mut c = cart(0x13, 256, 5);
        c.write_rom(0x2000, 0xC8);
        assert_eq!(lo(&c, 0x5000), 0xC8);
        c.write_rom(0, 0x0A);
        c.write_rom(0x4000, 6);
        c.write_ram(0xA000, 0x66);
        assert_eq!(c.read_ram(0xA000), 0x66);
        c.write_rom(0x4000, 4);
        assert_eq!(c.read_ram(0xA000), 0);
    }

    fn rtc_cart() -> Cartridge {
        let mut c = cart(0x10, 4, 2);
        c.write_rom(0, 0x0A);
        c
    }

    fn rtc_read(c: &mut Cartridge, reg: u8) -> u8 {
        c.write_rom(0x4000, reg);
        c.read_ram(0xA000)
    }

    fn latch(c: &mut Cartridge) {
        c.write_rom(0x6000, 0);
        c.write_rom(0x6000, 1);
    }

    fn run_seconds(c: &mut Cartridge, s: u32) {
        for _ in 0..(s as u64 * M_CYCLES_PER_SECOND as u64) {
            c.tick();
        }
    }

    #[test]
    fn rtc_latch_and_tick() {
        let mut c = rtc_cart();
        run_seconds(&mut c, 61);
        assert_eq!(rtc_read(&mut c, 8), 0); // not latched yet
        latch(&mut c);
        assert_eq!((rtc_read(&mut c, 8), rtc_read(&mut c, 9)), (1, 1));
        run_seconds(&mut c, 1);
        assert_eq!(rtc_read(&mut c, 8), 1); // latched value is frozen
        c.write_rom(0x6000, 1); // no 0 -> 1 edge
        assert_eq!(rtc_read(&mut c, 8), 1);
        latch(&mut c);
        assert_eq!(rtc_read(&mut c, 8), 2);
    }

    #[test]
    fn rtc_write_resets_subsecond_and_halt() {
        let mut c = rtc_cart();
        c.write_rom(0x4000, 8);
        for _ in 0..M_CYCLES_PER_SECOND - 1 {
            c.tick();
        }
        c.write_ram(0xA000, 10); // resets sub-second counter
        c.tick();
        latch(&mut c);
        assert_eq!(rtc_read(&mut c, 8), 10);
        run_seconds(&mut c, 1);
        latch(&mut c);
        assert_eq!(rtc_read(&mut c, 8), 11);
        // halt
        c.write_rom(0x4000, 0x0C);
        c.write_ram(0xA000, 0x40);
        run_seconds(&mut c, 5);
        latch(&mut c);
        assert_eq!(rtc_read(&mut c, 8), 11);
        assert_eq!(rtc_read(&mut c, 0x0C), 0x40);
    }

    #[test]
    fn rtc_carries_and_day_overflow() {
        let mut c = rtc_cart();
        c.write_rom(0x4000, 8);
        c.write_ram(0xA000, 59);
        c.write_rom(0x4000, 9);
        c.write_ram(0xA000, 59);
        c.write_rom(0x4000, 10);
        c.write_ram(0xA000, 23);
        c.write_rom(0x4000, 11);
        c.write_ram(0xA000, 0xFF);
        c.write_rom(0x4000, 12);
        c.write_ram(0xA000, 0x01); // day 511
        run_seconds(&mut c, 1);
        latch(&mut c);
        assert_eq!(rtc_read(&mut c, 8), 0);
        assert_eq!(rtc_read(&mut c, 9), 0);
        assert_eq!(rtc_read(&mut c, 10), 0);
        assert_eq!(rtc_read(&mut c, 11), 0);
        assert_eq!(rtc_read(&mut c, 12), 0x80); // carry set, day bit 8 cleared
        run_seconds(&mut c, 1);
        latch(&mut c);
        assert_eq!(rtc_read(&mut c, 12), 0x80); // carry is sticky
    }

    #[test]
    fn save_roundtrip() {
        let mut c = rtc_cart();
        c.write_rom(0x4000, 0);
        c.write_ram(0xA123, 0x77);
        run_seconds(&mut c, 3);
        latch(&mut c);
        c.set_unix_time(1_000);
        let sav = c.save_data();
        assert_eq!(sav.len(), 0x2000 + 48);
        let mut d = rtc_cart();
        d.load_save_data(&sav);
        d.write_rom(0x4000, 0);
        assert_eq!(d.read_ram(0xA123), 0x77);
        assert_eq!(rtc_read(&mut d, 8), 3);
        assert_eq!(d.save_data(), sav);
        // wall-clock catch-up: 1 minute later
        let mut e = rtc_cart();
        e.load_save_data_at(&sav, 1_060);
        latch(&mut e);
        assert_eq!((rtc_read(&mut e, 8), rtc_read(&mut e, 9)), (3, 1));
        // 32-bit timestamp footer variant
        let mut sav32 = sav[..0x2000 + 40].to_vec();
        sav32.extend_from_slice(&1_000u32.to_le_bytes());
        let mut f = rtc_cart();
        f.load_save_data_at(&sav32, 1_001);
        latch(&mut f);
        assert_eq!(rtc_read(&mut f, 8), 4);
    }

    #[test]
    fn ram_save_without_rtc() {
        let mut c = cart(0x03, 4, 2);
        c.write_rom(0, 0x0A);
        c.write_ram(0xA000, 9);
        let s = c.save_data();
        assert_eq!(s.len(), 0x2000);
        let mut d = cart(0x03, 4, 2);
        d.load_save_data(&s);
        d.write_rom(0, 0x0A);
        assert_eq!(d.read_ram(0xA000), 9);
    }

    #[test]
    fn mbc5_banking() {
        let mut c = cart(0x1B, 512, 4);
        c.write_rom(0x2000, 0);
        assert_eq!(lo(&c, 0x4000), 0); // bank 0 allowed
        c.write_rom(0x2000, 0x34);
        assert_eq!((lo(&c, 0x5000), lo(&c, 0x5001)), (0x34, 0));
        c.write_rom(0x3000, 1);
        assert_eq!((lo(&c, 0x5000), lo(&c, 0x5001)), (0x34, 1));
        c.write_rom(0x2000, 0xFF);
        c.write_rom(0x3000, 0xFF); // only bit 0 used
        assert_eq!((lo(&c, 0x5000), lo(&c, 0x5001)), (0xFF, 1));
        c.write_rom(0, 0x0A);
        for b in 0..16u8 {
            c.write_rom(0x4000, b);
            c.write_ram(0xA000, 0x80 + b);
        }
        c.write_rom(0x4000, 9);
        assert_eq!(c.read_ram(0xA000), 0x89);
        c.write_rom(0x0000, 0x1A); // exact 0x0A only on MBC5
        assert_eq!(c.read_ram(0xA000), 0xFF);
    }

    #[test]
    fn mbc5_wraps_and_rumble() {
        let mut c = cart(0x19, 8, 0);
        c.write_rom(0x2000, 0x0B);
        assert_eq!(lo(&c, 0x5000), 3);
        let mut r = cart(0x1E, 4, 4);
        r.write_rom(0, 0x0A);
        r.write_rom(0x4000, 0x0A); // bit 3 = rumble motor, RAM bank = 2
        r.write_ram(0xA000, 0x55);
        r.write_rom(0x4000, 0x02);
        assert_eq!(r.read_ram(0xA000), 0x55);
        assert!(r.has_rumble());
    }
}
