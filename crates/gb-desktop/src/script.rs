//! `--input-script` parser: lines of `frame button down|up`, `frame save N` or `frame load N`
//! (state slots 1..=4), `#` comments.

use gb_core::Button;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Button { button: Button, pressed: bool },
    Save(u8),
    Load(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event {
    pub frame: u64,
    pub action: Action,
}

pub fn parse_button(name: &str) -> Option<Button> {
    Some(match name.to_ascii_lowercase().as_str() {
        "right" => Button::Right,
        "left" => Button::Left,
        "up" => Button::Up,
        "down" => Button::Down,
        "a" => Button::A,
        "b" => Button::B,
        "select" => Button::Select,
        "start" => Button::Start,
        _ => return None,
    })
}

/// Events sorted by frame (stable, so same-frame order is preserved).
pub fn parse(text: &str) -> Result<Vec<Event>, String> {
    let mut events = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap().trim();
        if line.is_empty() {
            continue;
        }
        let err = |msg: &str| format!("input script line {}: {msg}", n + 1);
        let parts: Vec<&str> = line.split_whitespace().collect();
        let [frame, button, action] = parts[..] else {
            return Err(err("expected `frame button down|up` or `frame save|load N`"));
        };
        let frame = frame.parse().map_err(|_| err("bad frame number"))?;
        let action = match button {
            "save" | "load" => {
                let slot: u8 = action.parse().map_err(|_| err("bad slot"))?;
                if !(1..=4).contains(&slot) {
                    return Err(err("slot must be 1..4"));
                }
                if button == "save" {
                    Action::Save(slot)
                } else {
                    Action::Load(slot)
                }
            }
            _ => Action::Button {
                button: parse_button(button).ok_or_else(|| err("unknown button"))?,
                pressed: match action {
                    "down" => true,
                    "up" => false,
                    _ => return Err(err("expected down or up")),
                },
            },
        };
        events.push(Event { frame, action });
    }
    events.sort_by_key(|e| e.frame);
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_sorts() {
        let ev = parse("125 start up\n# c\n120 Start down  # go\n").unwrap();
        assert_eq!(ev.len(), 2);
        assert_eq!(
            ev[0],
            Event {
                frame: 120,
                action: Action::Button {
                    button: Button::Start,
                    pressed: true
                }
            }
        );
        assert!(matches!(ev[1].action, Action::Button { pressed: false, .. }));
    }

    #[test]
    fn parses_state_slots() {
        let ev = parse("10 save 2\n5 load 4").unwrap();
        assert_eq!(ev[0].action, Action::Load(4));
        assert_eq!(ev[1].action, Action::Save(2));
        assert!(parse("1 save 0").is_err());
        assert!(parse("1 load 5").is_err());
        assert!(parse("1 save x").is_err());
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse("1 turbo down").is_err());
        assert!(parse("1 a sideways").is_err());
        assert!(parse("x a down").is_err());
    }
}
