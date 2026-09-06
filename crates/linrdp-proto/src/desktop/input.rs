//! Slow-path input over the authenticated session (MS-RDPBCGR 2.2.8.1.1.3).
use super::{Phase, Result, Session, bad, u16le};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Key {
        code: u8,
        extended: bool,
        down: bool,
    },
    Move {
        x: u16,
        y: u16,
    },
    Button {
        button: u8,
        down: bool,
        x: u16,
        y: u16,
    },
    Wheel {
        delta: i16,
    },
    ReleaseAll,
}

#[derive(Default)]
pub(super) struct Held {
    keys: BTreeSet<(u8, bool)>,
    buttons: BTreeSet<u8>,
    position: (u16, u16),
}
impl Session {
    /// Returns one MCS input packet. Empty or inactive input is discarded.
    pub fn input(&mut self, events: &[Input]) -> Result<Option<Vec<u8>>> {
        if events.len() > 256 {
            return Err(bad("too many input events"));
        }
        if self.phase != Phase::Active {
            return Ok(None);
        }
        let mut body = vec![0; 4];
        let mut count = 0u16;
        let mut emit = |kind: u16, flags: u16, a: u16, b: u16| {
            body.extend([0; 4]); // eventTime is ignored by the server.
            for n in [kind, flags, a, b] {
                u16le(&mut body, n);
            }
            count += 1;
        };
        for &event in events {
            match event {
                Input::Key {
                    code,
                    extended,
                    down,
                } => {
                    if code == 0 || code > 0x7f {
                        return Err(bad("invalid keyboard scancode"));
                    }
                    if down {
                        self.held.keys.insert((code, extended));
                    } else if !self.held.keys.remove(&(code, extended)) {
                        continue;
                    }
                    emit(
                        4,
                        if extended { 0x100 } else { 0 } | if down { 0 } else { 0x8000 },
                        code.into(),
                        0,
                    );
                }
                Input::Move { x, y } | Input::Button { x, y, .. } => {
                    if x >= self.framebuffer.width || y >= self.framebuffer.height {
                        return Err(bad("input outside remote desktop"));
                    }
                    self.held.position = (x, y);
                    let flags = if let Input::Button { button, down, .. } = event {
                        if !(1..=3).contains(&button) {
                            return Err(bad("unsupported mouse button"));
                        }
                        if down {
                            self.held.buttons.insert(button);
                        } else if !self.held.buttons.remove(&button) {
                            continue;
                        }
                        (0x1000 << (button - 1)) | if down { 0x8000 } else { 0 }
                    } else {
                        0x800
                    };
                    emit(0x8001, flags, x, y);
                }
                Input::Wheel { delta } => {
                    if !(-255..=255).contains(&delta) {
                        return Err(bad("wheel delta outside signed nine-bit range"));
                    }
                    if delta != 0 {
                        emit(0x8001, 0x200 | (delta as u16 & 0x1ff), 0, 0);
                    }
                }
                Input::ReleaseAll => {
                    for (code, extended) in std::mem::take(&mut self.held.keys) {
                        emit(4, 0x8000 | if extended { 0x100 } else { 0 }, code.into(), 0);
                    }
                    for button in std::mem::take(&mut self.held.buttons) {
                        emit(
                            0x8001,
                            0x1000 << (button - 1),
                            self.held.position.0,
                            self.held.position.1,
                        );
                    }
                }
            }
        }
        if count == 0 {
            return Ok(None);
        }
        body[..2].copy_from_slice(&count.to_le_bytes());
        self.data(28, &body).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn active() -> Session {
        let mut s = Session::new(1004, 1003).unwrap();
        s.phase = Phase::Active;
        s
    }
    #[test]
    fn keyboard_wire_vector_and_release_on_focus_loss() {
        let mut s = active();
        let p = s
            .input(&[Input::Key {
                code: 0x1d,
                extended: true,
                down: true,
            }])
            .unwrap()
            .unwrap();
        assert_eq!(
            &p[p.len() - 16..],
            &[1, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 1, 0x1d, 0, 0, 0]
        );
        let p = s.input(&[Input::ReleaseAll]).unwrap().unwrap();
        assert_eq!(
            &p[p.len() - 12..],
            &[0, 0, 0, 0, 4, 0, 0, 0x81, 0x1d, 0, 0, 0]
        );
        assert!(s.input(&[Input::ReleaseAll]).unwrap().is_none());
    }
    #[test]
    fn mouse_coordinates_buttons_and_negative_wheel() {
        let mut s = active();
        let p = s
            .input(&[
                Input::Button {
                    button: 1,
                    down: true,
                    x: 1023,
                    y: 767,
                },
                Input::Wheel { delta: -120 },
            ])
            .unwrap()
            .unwrap();
        assert_eq!(
            &p[p.len() - 24..p.len() - 12],
            &[0, 0, 0, 0, 1, 128, 0, 144, 255, 3, 255, 2]
        );
        assert_eq!(
            &p[p.len() - 12..],
            &[0, 0, 0, 0, 1, 128, 136, 3, 0, 0, 0, 0]
        );
        let p = s.input(&[Input::ReleaseAll]).unwrap().unwrap();
        assert_eq!(&p[p.len() - 6..], &[0, 16, 255, 3, 255, 2]);
        assert!(s.input(&[Input::Move { x: 1024, y: 0 }]).is_err());
        assert!(s.input(&[Input::Wheel { delta: 256 }]).is_err());
    }
    #[test]
    fn inactive_input_and_redundant_releases_are_not_sent() {
        let mut s = Session::new(1004, 1003).unwrap();
        assert!(s.input(&[Input::Move { x: 0, y: 0 }]).unwrap().is_none());
        s.phase = Phase::Active;
        assert!(
            s.input(&[Input::Key {
                code: 0x1e,
                extended: false,
                down: false
            }])
            .unwrap()
            .is_none()
        );
        assert!(s.input(&vec![Input::ReleaseAll; 257]).is_err());
    }
}
