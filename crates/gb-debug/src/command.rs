//! Debugger command language (shared by the `:` prompt and `--script`).

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `break ADDR`
    Break(u16),
    /// `delete ADDR`: remove a breakpoint.
    Delete(u16),
    /// `watch ADDR`
    Watch(u16),
    /// `unwatch ADDR`
    Unwatch(u16),
    /// `set REG=VAL`
    Set(Reg, u16),
    /// `poke ADDR VAL`
    Poke(u16, u8),
    /// `trace on [FILE]` / `trace off`
    Trace(Option<String>),
    /// `step [N]`
    Step(u32),
    /// `next`: step over CALL/RST.
    Next,
    /// `frame`: run to the next frame boundary.
    Frame,
    /// `continue [FRAMES]`: run until a stop condition (script mode caps at FRAMES).
    Continue(Option<u32>),
    /// `ret`: run until the current function returns.
    Ret,
    Regs,
    /// `x [ADDR] [LEN]`: hex dump.
    Examine(Option<u16>, u16),
    /// `dis [ADDR] [COUNT]`
    Disasm(Option<u16>, u16),
    /// `goto ADDR`: move the memory view.
    Goto(u16),
    Serial,
    Breaks,
    Help,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reg {
    A,
    F,
    B,
    C,
    D,
    E,
    H,
    L,
    Af,
    Bc,
    De,
    Hl,
    Sp,
    Pc,
    Ime,
}

pub fn parse_num(s: &str) -> Result<u32, String> {
    let t = s.trim();
    let r = if let Some(h) = t.strip_prefix('$').or_else(|| t.strip_prefix("0x")) {
        u32::from_str_radix(h, 16)
    } else {
        t.parse()
    };
    r.map_err(|_| format!("bad number '{s}'"))
}

fn addr(s: &str) -> Result<u16, String> {
    let n = parse_num(s)?;
    u16::try_from(n).map_err(|_| format!("address out of range '{s}'"))
}

fn parse_reg(s: &str) -> Result<Reg, String> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "a" => Reg::A,
        "f" => Reg::F,
        "b" => Reg::B,
        "c" => Reg::C,
        "d" => Reg::D,
        "e" => Reg::E,
        "h" => Reg::H,
        "l" => Reg::L,
        "af" => Reg::Af,
        "bc" => Reg::Bc,
        "de" => Reg::De,
        "hl" => Reg::Hl,
        "sp" => Reg::Sp,
        "pc" => Reg::Pc,
        "ime" => Reg::Ime,
        _ => return Err(format!("unknown register '{s}'")),
    })
}

impl Reg {
    pub fn is_16bit(self) -> bool {
        matches!(self, Reg::Af | Reg::Bc | Reg::De | Reg::Hl | Reg::Sp | Reg::Pc)
    }
}

/// Parse one command line. `Ok(None)` for blank lines and `#` comments.
pub fn parse(line: &str) -> Result<Option<Command>, String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return Ok(None);
    }
    let mut it = line.split_whitespace();
    let name = it.next().unwrap().to_ascii_lowercase();
    let args: Vec<&str> = it.collect();
    let need = |n: usize| -> Result<(), String> {
        if args.len() < n {
            Err(format!("'{name}' needs {n} argument(s)"))
        } else {
            Ok(())
        }
    };
    let cmd = match name.as_str() {
        "break" | "b" => {
            need(1)?;
            Command::Break(addr(args[0])?)
        }
        "delete" | "del" | "unbreak" => {
            need(1)?;
            Command::Delete(addr(args[0])?)
        }
        "watch" | "w" => {
            need(1)?;
            Command::Watch(addr(args[0])?)
        }
        "unwatch" => {
            need(1)?;
            Command::Unwatch(addr(args[0])?)
        }
        "set" => {
            // `set a=$12`, `set a = $12`, `set a $12`
            let joined = args.join(" ");
            let (r, v) = joined
                .split_once('=')
                .map(|(r, v)| (r.trim().to_string(), v.trim().to_string()))
                .or_else(|| joined.split_once(' ').map(|(r, v)| (r.to_string(), v.trim().to_string())))
                .ok_or("usage: set REG=VALUE")?;
            let reg = parse_reg(&r)?;
            let val = parse_num(&v)?;
            let max = if reg.is_16bit() { 0xFFFF } else { 0xFF };
            if val > max {
                return Err(format!("value {val:#x} too large for {r}"));
            }
            Command::Set(reg, val as u16)
        }
        "poke" => {
            need(2)?;
            let v = parse_num(args[1])?;
            Command::Poke(addr(args[0])?, u8::try_from(v).map_err(|_| "poke value must fit a byte")?)
        }
        "trace" => match args.first().map(|s| s.to_ascii_lowercase()).as_deref() {
            Some("on") => Command::Trace(Some(args.get(1).copied().unwrap_or("trace.log").to_string())),
            Some("off") => Command::Trace(None),
            _ => return Err("usage: trace on [FILE] | trace off".into()),
        },
        "step" | "s" => Command::Step(args.first().map_or(Ok(1), |a| parse_num(a))?),
        "next" | "n" => Command::Next,
        "frame" | "f" => Command::Frame,
        "continue" | "c" => Command::Continue(args.first().map(|a| parse_num(a)).transpose()?),
        "ret" | "r" => Command::Ret,
        "regs" | "reg" => Command::Regs,
        "x" | "mem" => Command::Examine(
            args.first().map(|a| addr(a)).transpose()?,
            args.get(1).map_or(Ok(64), |a| parse_num(a).map(|n| n.min(0x10000) as u16))?,
        ),
        "dis" | "disasm" => Command::Disasm(
            args.first().map(|a| addr(a)).transpose()?,
            args.get(1).map_or(Ok(8), |a| parse_num(a).map(|n| n.min(1000) as u16))?,
        ),
        "goto" | "g" => {
            need(1)?;
            Command::Goto(addr(args[0])?)
        }
        "serial" => Command::Serial,
        "breaks" | "info" => Command::Breaks,
        "help" | "?" => Command::Help,
        "quit" | "q" | "exit" => Command::Quit,
        _ => return Err(format!("unknown command '{name}' (try help)")),
    };
    Ok(Some(cmd))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Command {
        parse(s).unwrap().unwrap()
    }

    #[test]
    fn numbers_in_all_bases() {
        assert_eq!(p("break $C000"), Command::Break(0xC000));
        assert_eq!(p("break 0x150"), Command::Break(0x150));
        assert_eq!(p("break 256"), Command::Break(256));
    }

    #[test]
    fn set_forms() {
        assert_eq!(p("set a=$12"), Command::Set(Reg::A, 0x12));
        assert_eq!(p("set hl = $C000"), Command::Set(Reg::Hl, 0xC000));
        assert_eq!(p("set pc $150"), Command::Set(Reg::Pc, 0x150));
        assert!(parse("set a=$100").is_err());
        assert!(parse("set q=1").is_err());
    }

    #[test]
    fn poke_and_watch() {
        assert_eq!(p("poke $C000 $FF"), Command::Poke(0xC000, 0xFF));
        assert!(parse("poke $C000 $1FF").is_err());
        assert!(parse("poke $C000").is_err());
        assert_eq!(p("watch $FF40"), Command::Watch(0xFF40));
        assert!(parse("watch $10000").is_err());
    }

    #[test]
    fn trace_and_flow() {
        assert_eq!(p("trace on"), Command::Trace(Some("trace.log".into())));
        assert_eq!(p("trace on /tmp/x"), Command::Trace(Some("/tmp/x".into())));
        assert_eq!(p("trace off"), Command::Trace(None));
        assert_eq!(p("step"), Command::Step(1));
        assert_eq!(p("s 5"), Command::Step(5));
        assert_eq!(p("continue 10"), Command::Continue(Some(10)));
        assert_eq!(p("x $C000"), Command::Examine(Some(0xC000), 64));
    }

    #[test]
    fn blank_comment_and_errors() {
        assert_eq!(parse("  ").unwrap(), None);
        assert_eq!(parse("# hi").unwrap(), None);
        assert!(parse("bogus").is_err());
        assert!(parse("break").is_err());
    }
}
