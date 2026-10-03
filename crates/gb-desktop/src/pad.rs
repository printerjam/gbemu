//! Optional gamepad input (cargo feature `gamepad`, via gilrs).

#[cfg(feature = "gamepad")]
use gb_core::Button;

/// Which Game Boy buttons the gamepad is holding, indexed like [`crate::BUTTONS`].
#[cfg(feature = "gamepad")]
pub struct Pad {
    gilrs: gilrs::Gilrs,
    held: [bool; 8],
}

#[cfg(feature = "gamepad")]
impl Pad {
    pub fn new() -> Option<Pad> {
        match gilrs::Gilrs::new() {
            Ok(gilrs) => Some(Pad {
                gilrs,
                held: [false; 8],
            }),
            Err(e) => {
                eprintln!("gbemu: gamepad support disabled: {e}");
                None
            }
        }
    }

    /// Drain pending events; returns the current held state.
    pub fn poll(&mut self) -> [bool; 8] {
        use gilrs::{Axis, EventType};
        while let Some(ev) = self.gilrs.next_event() {
            match ev.event {
                EventType::ButtonPressed(b, _) | EventType::ButtonReleased(b, _) => {
                    let down = matches!(ev.event, EventType::ButtonPressed(..));
                    if let Some(i) = button_index(b) {
                        self.held[i] = down;
                    }
                }
                EventType::AxisChanged(axis, v, _) => {
                    let (neg, pos) = match axis {
                        Axis::LeftStickX => (Button::Left, Button::Right),
                        // gilrs reports stick-up as positive Y.
                        Axis::LeftStickY => (Button::Down, Button::Up),
                        _ => continue,
                    };
                    self.held[crate::index_of(neg)] = v < -0.5;
                    self.held[crate::index_of(pos)] = v > 0.5;
                }
                EventType::Disconnected => self.held = [false; 8],
                _ => {}
            }
        }
        self.held
    }
}

#[cfg(feature = "gamepad")]
fn button_index(b: gilrs::Button) -> Option<usize> {
    use gilrs::Button as G;
    Some(crate::index_of(match b {
        G::DPadRight => Button::Right,
        G::DPadLeft => Button::Left,
        G::DPadUp => Button::Up,
        G::DPadDown => Button::Down,
        // Nintendo layout: east is A, south is B.
        G::East => Button::A,
        G::South => Button::B,
        G::Start => Button::Start,
        G::Select => Button::Select,
        _ => return None,
    }))
}

/// Stand-in when built without the `gamepad` feature: never holds anything.
#[cfg(not(feature = "gamepad"))]
pub struct Pad;

#[cfg(not(feature = "gamepad"))]
impl Pad {
    pub fn new() -> Option<Pad> {
        None
    }

    pub fn poll(&mut self) -> [bool; 8] {
        [false; 8]
    }
}
