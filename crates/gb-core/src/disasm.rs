//! SM83 disassembler for debugging and tracing. Pure function of a memory reader.

const R8: [&str; 8] = ["B", "C", "D", "E", "H", "L", "(HL)", "A"];
const RR: [&str; 4] = ["BC", "DE", "HL", "SP"];
const RR_STACK: [&str; 4] = ["BC", "DE", "HL", "AF"];
const CC: [&str; 4] = ["NZ", "Z", "NC", "C"];
const ALU: [&str; 8] = ["ADD A,", "ADC A,", "SUB ", "SBC A,", "AND ", "XOR ", "OR ", "CP "];
const ROT: [&str; 8] = ["RLC", "RRC", "RL", "RR", "SLA", "SRA", "SWAP", "SRL"];
const ACC_OPS: [&str; 8] = ["RLCA", "RRCA", "RLA", "RRA", "DAA", "CPL", "SCF", "CCF"];
/// `(HL+)`/`(HL-)`-style indirect operands for opcodes 0x02/0x0A/0x12/0x1A/... by `p`.
const IND: [&str; 4] = ["(BC)", "(DE)", "(HL+)", "(HL-)"];

/// Operand tokens embedded (lowercase) in a template; they determine the instruction length.
/// `d8` imm8, `d16` imm16, `a16` absolute address, `a8` $FF00+imm8, `r8` relative jump target, `s8` signed imm8.
const TOKENS: [(&str, u8); 6] = [("d16", 2), ("a16", 2), ("d8", 1), ("a8", 1), ("r8", 1), ("s8", 1)];

/// Mnemonic template for an unprefixed opcode; `None` for illegal opcodes.
fn base_template(op: u8) -> Option<String> {
    let (x, y, z) = ((op >> 6) as usize, ((op >> 3) & 7) as usize, (op & 7) as usize);
    let (p, q) = (y >> 1, y & 1);
    let s = match (x, z) {
        (0, 0) => match y {
            0 => "NOP".into(),
            1 => "LD (a16),SP".into(),
            2 => "STOP d8".into(),
            3 => "JR r8".into(),
            _ => format!("JR {},r8", CC[y - 4]),
        },
        (0, 1) if q == 0 => format!("LD {},d16", RR[p]),
        (0, 1) => format!("ADD HL,{}", RR[p]),
        (0, 2) if q == 0 => format!("LD {},A", IND[p]),
        (0, 2) => format!("LD A,{}", IND[p]),
        (0, 3) if q == 0 => format!("INC {}", RR[p]),
        (0, 3) => format!("DEC {}", RR[p]),
        (0, 4) => format!("INC {}", R8[y]),
        (0, 5) => format!("DEC {}", R8[y]),
        (0, 6) => format!("LD {},d8", R8[y]),
        (0, _) => ACC_OPS[y].into(),
        (1, _) if y == 6 && z == 6 => "HALT".into(),
        (1, _) => format!("LD {},{}", R8[y], R8[z]),
        (2, _) => format!("{}{}", ALU[y], R8[z]),
        (_, 0) => match y {
            0..=3 => format!("RET {}", CC[y]),
            4 => "LDH (a8),A".into(),
            5 => "ADD SP,s8".into(),
            6 => "LDH A,(a8)".into(),
            _ => "LD HL,SPs8".into(),
        },
        (_, 1) if q == 0 => format!("POP {}", RR_STACK[p]),
        (_, 1) => ["RET", "RETI", "JP HL", "LD SP,HL"][p].into(),
        (_, 2) => match y {
            0..=3 => format!("JP {},a16", CC[y]),
            4 => "LD (C),A".into(),
            5 => "LD (a16),A".into(),
            6 => "LD A,(C)".into(),
            _ => "LD A,(a16)".into(),
        },
        (_, 3) => match y {
            0 => "JP a16".into(),
            1 => "CB".into(),
            6 => "DI".into(),
            7 => "EI".into(),
            _ => return None,
        },
        (_, 4) if y < 4 => format!("CALL {},a16", CC[y]),
        (_, 5) if q == 0 => format!("PUSH {}", RR_STACK[p]),
        (_, 5) if p == 0 => "CALL a16".into(),
        (_, 6) => format!("{}d8", ALU[y]),
        (_, 7) => format!("RST ${:02X}", y * 8),
        _ => return None,
    };
    Some(s)
}

fn cb_mnemonic(op: u8) -> String {
    let (x, y, z) = (op >> 6, (op >> 3) & 7, (op & 7) as usize);
    match x {
        0 => format!("{} {}", ROT[y as usize], R8[z]),
        1 => format!("BIT {y},{}", R8[z]),
        2 => format!("RES {y},{}", R8[z]),
        _ => format!("SET {y},{}", R8[z]),
    }
}

/// Disassemble the instruction at `pc`. Returns the text and its length in bytes.
/// Illegal opcodes render as `DB $xx` (length 1).
pub fn disassemble(read: impl Fn(u16) -> u8, pc: u16) -> (String, u8) {
    let op = read(pc);
    let Some(mut text) = base_template(op) else {
        return (format!("DB ${op:02X}"), 1);
    };
    if op == 0xCB {
        return (cb_mnemonic(read(pc.wrapping_add(1))), 2);
    }
    let mut len = 1u8;
    for (tok, n) in TOKENS {
        if let Some(i) = text.find(tok) {
            let b1 = read(pc.wrapping_add(1));
            let operand = match tok {
                "d16" | "a16" => format!("${:04X}", u16::from_le_bytes([b1, read(pc.wrapping_add(2))])),
                "d8" => format!("${b1:02X}"),
                "a8" => format!("$FF{b1:02X}"),
                "r8" => format!("${:04X}", pc.wrapping_add(2).wrapping_add(b1 as i8 as u16)),
                _ => format!("{:+}", b1 as i8),
            };
            text.replace_range(i..i + tok.len(), &operand);
            len += n;
            break;
        }
    }
    (text, len)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dis(bytes: &[u8], pc: u16) -> (String, u8) {
        let bytes = bytes.to_vec();
        disassemble(
            move |a| bytes.get(a.wrapping_sub(pc) as usize).copied().unwrap_or(0),
            pc,
        )
    }

    #[test]
    fn lengths_of_every_opcode() {
        const ILLEGAL: [u8; 11] = [0xD3, 0xDB, 0xDD, 0xE3, 0xE4, 0xEB, 0xEC, 0xED, 0xF4, 0xFC, 0xFD];
        const LEN3: [u8; 15] = [
            0x01, 0x11, 0x21, 0x31, 0x08, 0xC2, 0xC3, 0xC4, 0xCA, 0xCC, 0xCD, 0xD2, 0xD4, 0xDA, 0xDC,
        ];
        const LEN2: [u8; 24] = [
            0x06, 0x0E, 0x16, 0x1E, 0x26, 0x2E, 0x36, 0x3E, 0x18, 0x20, 0x28, 0x30, 0x38, 0xC6, 0xCE, 0xD6, 0xDE, 0xE0,
            0xE6, 0xE8, 0xEE, 0xF0, 0xF6, 0xF8,
        ];
        for op in 0..=255u8 {
            let want = if LEN3.contains(&op) || op == 0xEA || op == 0xFA {
                3
            } else if LEN2.contains(&op) || matches!(op, 0x10 | 0xCB | 0xFE) {
                2
            } else {
                1
            };
            let (text, len) = dis(&[op, 0x12, 0x34], 0x1000);
            assert_eq!(len, want, "opcode {op:02X} -> {text}");
            assert!(!text.is_empty());
            assert_eq!(text.starts_with("DB "), ILLEGAL.contains(&op), "opcode {op:02X}");
        }
        for op in 0..=255u8 {
            let (text, len) = dis(&[0xCB, op], 0);
            assert_eq!(len, 2);
            assert!(!text.contains('$') && !text.starts_with("DB"), "{text}");
        }
    }

    #[test]
    fn no_unresolved_tokens() {
        for op in 0..=255u8 {
            let (text, _) = dis(&[op, 0x12, 0x34], 0x1000);
            for (tok, _) in TOKENS {
                assert!(!text.contains(tok), "opcode {op:02X}: {text}");
            }
        }
    }

    #[test]
    fn mnemonic_samples() {
        let cases: &[(&[u8], &str)] = &[
            (&[0x00], "NOP"),
            (&[0x10, 0x00], "STOP $00"),
            (&[0x76], "HALT"),
            (&[0x08, 0x34, 0x12], "LD ($1234),SP"),
            (&[0x2A], "LD A,(HL+)"),
            (&[0x32], "LD (HL-),A"),
            (&[0x02], "LD (BC),A"),
            (&[0x36, 0x7F], "LD (HL),$7F"),
            (&[0x01, 0x34, 0x12], "LD BC,$1234"),
            (&[0x41], "LD B,C"),
            (&[0x46], "LD B,(HL)"),
            (&[0x86], "ADD A,(HL)"),
            (&[0x97], "SUB A"),
            (&[0xAF], "XOR A"),
            (&[0xFE, 0x90], "CP $90"),
            (&[0xE0, 0x44], "LDH ($FF44),A"),
            (&[0xF0, 0x40], "LDH A,($FF40)"),
            (&[0xE2], "LD (C),A"),
            (&[0xFA, 0x00, 0xC0], "LD A,($C000)"),
            (&[0xEA, 0x00, 0xC0], "LD ($C000),A"),
            (&[0xE8, 0xFE], "ADD SP,-2"),
            (&[0xF8, 0x05], "LD HL,SP+5"),
            (&[0xF9], "LD SP,HL"),
            (&[0xE9], "JP HL"),
            (&[0xC3, 0x50, 0x01], "JP $0150"),
            (&[0xCA, 0x50, 0x01], "JP Z,$0150"),
            (&[0xCD, 0x00, 0x20], "CALL $2000"),
            (&[0xC4, 0x00, 0x20], "CALL NZ,$2000"),
            (&[0xC0], "RET NZ"),
            (&[0xD9], "RETI"),
            (&[0xFF], "RST $38"),
            (&[0xC7], "RST $00"),
            (&[0xF5], "PUSH AF"),
            (&[0xC1], "POP BC"),
            (&[0xF3], "DI"),
            (&[0xFB], "EI"),
            (&[0x27], "DAA"),
            (&[0x39], "ADD HL,SP"),
            (&[0x3C], "INC A"),
            (&[0x35], "DEC (HL)"),
            (&[0x13], "INC DE"),
            (&[0xD3], "DB $D3"),
            (&[0xCB, 0x37], "SWAP A"),
            (&[0xCB, 0x7E], "BIT 7,(HL)"),
            (&[0xCB, 0x86], "RES 0,(HL)"),
            (&[0xCB, 0xFF], "SET 7,A"),
            (&[0xCB, 0x19], "RR C"),
        ];
        for (bytes, want) in cases {
            assert_eq!(dis(bytes, 0x0200).0, *want, "{bytes:02X?}");
        }
    }

    #[test]
    fn relative_jump_targets() {
        assert_eq!(dis(&[0x20, 0xFE], 0x0100).0, "JR NZ,$0100");
        assert_eq!(dis(&[0x18, 0x05], 0x0100).0, "JR $0107");
        assert_eq!(dis(&[0x38, 0x80], 0x0100).0, "JR C,$0082");
        assert_eq!(dis(&[0x18, 0xFE], 0x0000).0, "JR $0000");
        assert_eq!(dis(&[0x18, 0x7F], 0xFFF0).0, "JR $0071"); // wraps
    }
}
