//! `--input-script` parser: lines of `frame button down|up`, `#` comments.

use gb_core::Button;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event {
    pub frame: u64,
    pub button: Button,
    pub pressed: bool,
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
            return Err(err("expected `frame button down|up`"));
        };
        let frame = frame.parse().map_err(|_| err("bad frame number"))?;
        let button = parse_button(button).ok_or_else(|| err("unknown button"))?;
        let pressed = match action {
            "down" => true,
            "up" => false,
            _ => return Err(err("expected down or up")),
        };
        events.push(Event { frame, button, pressed });
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
                button: Button::Start,
                pressed: true
            }
        );
        assert!(!ev[1].pressed);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse("1 turbo down").is_err());
        assert!(parse("1 a sideways").is_err());
        assert!(parse("x a down").is_err());
    }
}
