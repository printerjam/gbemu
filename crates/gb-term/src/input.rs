//! Key mapping and held-button tracking (with or without key-release events).

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use gb_core::Button;

/// Frames a key press stays "held" when the terminal cannot report key releases.
pub const HOLD_FRAMES: u64 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Button(Button),
    Quit,
    Pause,
    SaveState,
    LoadState,
}

pub fn map_key(k: &KeyEvent) -> Option<Action> {
    use Button::*;
    Some(match k.code {
        KeyCode::Up => Action::Button(Up),
        KeyCode::Down => Action::Button(Down),
        KeyCode::Left => Action::Button(Left),
        KeyCode::Right => Action::Button(Right),
        KeyCode::Enter => Action::Button(Start),
        KeyCode::Backspace | KeyCode::BackTab => Action::Button(Select),
        KeyCode::F(1) => Action::SaveState,
        KeyCode::F(2) => Action::LoadState,
        KeyCode::Esc => Action::Quit,
        KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => Action::Quit,
        KeyCode::Char(c) => match c.to_ascii_lowercase() {
            'w' => Action::Button(Up),
            's' => Action::Button(Down),
            'a' => Action::Button(Left),
            'd' => Action::Button(Right),
            'x' | 'k' => Action::Button(A),
            'z' | 'j' => Action::Button(B),
            'q' => Action::Quit,
            'p' => Action::Pause,
            '[' => Action::SaveState,
            ']' => Action::LoadState,
            _ => return None,
        },
        _ => return None,
    })
}

const ALL: [Button; 8] = [
    Button::Right,
    Button::Left,
    Button::Up,
    Button::Down,
    Button::A,
    Button::B,
    Button::Select,
    Button::Start,
];

fn idx(b: Button) -> usize {
    ALL.iter().position(|&x| x == b).unwrap()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Held {
    No,
    Until(u64),
    UntilRelease,
}

pub struct Input {
    /// True when the terminal reports key release events (kitty keyboard protocol).
    releases: bool,
    held: [Held; 8],
    applied: [bool; 8],
}

impl Input {
    pub fn new(releases: bool) -> Self {
        Input {
            releases,
            held: [Held::No; 8],
            applied: [false; 8],
        }
    }

    /// Record a key event for `button` at `frame`.
    pub fn key(&mut self, button: Button, kind: KeyEventKind, frame: u64) {
        let h = &mut self.held[idx(button)];
        *h = match kind {
            KeyEventKind::Release => Held::No,
            _ if self.releases => Held::UntilRelease,
            _ => Held::Until(frame + HOLD_FRAMES),
        };
    }

    /// Every button's desired state at `frame`, for re-applying after the machine state was replaced.
    pub fn resync(&mut self, frame: u64) -> Vec<(Button, bool)> {
        self.applied = [false; 8];
        let mut all: Vec<_> = ALL.iter().map(|&b| (b, false)).collect();
        for (b, pressed) in self.changes(frame) {
            all[idx(b)].1 = pressed;
        }
        all
    }

    /// Button state changes to apply at `frame` (button, pressed).
    pub fn changes(&mut self, frame: u64) -> Vec<(Button, bool)> {
        let mut out = Vec::new();
        for (i, &b) in ALL.iter().enumerate() {
            let want = match self.held[i] {
                Held::No => false,
                Held::UntilRelease => true,
                Held::Until(f) => frame < f,
            };
            if want != self.applied[i] {
                self.applied[i] = want;
                out.push((b, want));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventState;

    fn ev(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: m,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn key_map() {
        let n = KeyModifiers::NONE;
        assert_eq!(map_key(&ev(KeyCode::Char('X'), n)), Some(Action::Button(Button::A)));
        assert_eq!(map_key(&ev(KeyCode::Char('j'), n)), Some(Action::Button(Button::B)));
        assert_eq!(
            map_key(&ev(KeyCode::BackTab, KeyModifiers::SHIFT)),
            Some(Action::Button(Button::Select))
        );
        assert_eq!(map_key(&ev(KeyCode::Enter, n)), Some(Action::Button(Button::Start)));
        assert_eq!(
            map_key(&ev(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(Action::Quit)
        );
        assert_eq!(map_key(&ev(KeyCode::Char('c'), n)), None);
        assert_eq!(map_key(&ev(KeyCode::Esc, n)), Some(Action::Quit));
        assert_eq!(map_key(&ev(KeyCode::F(1), n)), Some(Action::SaveState));
        assert_eq!(map_key(&ev(KeyCode::Char(']'), n)), Some(Action::LoadState));
    }

    #[test]
    fn without_release_events_press_is_held_then_expires_and_repeat_refreshes() {
        let mut i = Input::new(false);
        i.key(Button::A, KeyEventKind::Press, 10);
        assert_eq!(i.changes(10), [(Button::A, true)]);
        assert!(i.changes(10 + HOLD_FRAMES - 1).is_empty());
        i.key(Button::A, KeyEventKind::Repeat, 16);
        assert!(i.changes(10 + HOLD_FRAMES).is_empty()); // refreshed
        assert_eq!(i.changes(16 + HOLD_FRAMES), [(Button::A, false)]);
    }

    #[test]
    fn with_release_events_held_until_release() {
        let mut i = Input::new(true);
        i.key(Button::Start, KeyEventKind::Press, 0);
        assert_eq!(i.changes(0), [(Button::Start, true)]);
        assert!(i.changes(1000).is_empty());
        i.key(Button::Start, KeyEventKind::Release, 1000);
        assert_eq!(i.changes(1000), [(Button::Start, false)]);
    }

    #[test]
    fn resync_reports_every_button_with_current_hold_state() {
        let mut i = Input::new(true);
        i.key(Button::A, KeyEventKind::Press, 0);
        i.changes(0);
        let all = i.resync(1);
        assert_eq!(all.len(), 8);
        assert!(all.contains(&(Button::A, true)));
        assert_eq!(all.iter().filter(|(_, p)| *p).count(), 1);
    }
}
