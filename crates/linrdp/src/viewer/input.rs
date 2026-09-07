use linrdp_proto::desktop::Input;
use minifb::{HasWindowHandle, InputCallback, Key, KeyRepeat, MouseButton, MouseMode, Window};
use std::{cell::RefCell, collections::BTreeSet, rc::Rc};

#[derive(Default)]
struct KeyQueue {
    events: Vec<(Key, bool)>,
    overflow: bool,
}
struct Callback(Rc<RefCell<KeyQueue>>);
impl InputCallback for Callback {
    fn add_char(&mut self, _: u32) {}
    fn set_key_state(&mut self, key: Key, down: bool) {
        let mut q = self.0.borrow_mut();
        if q.events.len() < 192 {
            q.events.push((key, down));
        } else {
            q.overflow = true;
        }
    }
}

/// The same centered viewport is used for rendering and mouse hit testing.
#[derive(Clone, Copy, Debug)]
pub(super) struct Viewport {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}
impl Viewport {
    pub fn new(window: (usize, usize), remote: (usize, usize)) -> Self {
        let scale = (window.0 as f64 / remote.0 as f64).min(window.1 as f64 / remote.1 as f64);
        let width = (remote.0 as f64 * scale).floor() as usize;
        let height = (remote.1 as f64 * scale).floor() as usize;
        Self {
            x: (window.0 - width) / 2,
            y: (window.1 - height) / 2,
            width,
            height,
        }
    }
    fn point(self, p: (f32, f32), remote: (usize, usize)) -> Option<(u16, u16)> {
        let (x, y) = (p.0 - self.x as f32, p.1 - self.y as f32);
        if !x.is_finite()
            || !y.is_finite()
            || x < 0.
            || y < 0.
            || x >= self.width as f32
            || y >= self.height as f32
        {
            return None;
        }
        Some((
            (x as usize * remote.0 / self.width) as u16,
            (y as usize * remote.1 / self.height) as u16,
        ))
    }
    pub fn render(
        self,
        source: &[u32],
        remote: (usize, usize),
        window: (usize, usize),
        out: &mut Vec<u32>,
    ) -> Result<(), &'static str> {
        let size = window
            .0
            .checked_mul(window.1)
            .filter(|n| *n <= 16_777_216)
            .ok_or("local window exceeds pixel limit")?;
        out.resize(size, 0);
        out.fill(0);
        for y in 0..self.height {
            let row = y * remote.1 / self.height * remote.0;
            let dst = (y + self.y) * window.0 + self.x;
            for x in 0..self.width {
                out[dst + x] = source[row + x * remote.0 / self.width];
            }
        }
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct Controller {
    queue: Rc<RefCell<KeyQueue>>,
    wayland: bool,
    focused: bool,
    keys: BTreeSet<Key>,
    ignored: BTreeSet<Key>,
    buttons: [bool; 3],
    ignored_buttons: [bool; 3],
    position: Option<(u16, u16)>,
    wheel: f32,
}
impl Controller {
    pub fn reset(&mut self) {
        *self.queue.borrow_mut() = KeyQueue::default();
        *self = Self {
            queue: self.queue.clone(),
            wayland: self.wayland,
            ..Self::default()
        };
    }
    pub fn attach(window: &mut Window) -> Result<Self, &'static str> {
        let handle = window
            .window_handle()
            .map_err(|_| "cannot identify native display backend")?;
        let wayland = matches!(
            handle.as_raw(),
            raw_window_handle::RawWindowHandle::Wayland(_)
        );
        let c = Self {
            wayland,
            ..Self::default()
        };
        window.set_input_callback(Box::new(Callback(c.queue.clone())));
        Ok(c)
    }
    pub fn poll(
        &mut self,
        window: &mut Window,
        ready: bool,
        remote: (usize, usize),
    ) -> Result<Vec<Input>, &'static str> {
        let focused = ready && window.is_active();
        let keys = window.get_keys().into_iter().collect();
        let mut transitions = {
            let mut q = self.queue.borrow_mut();
            if q.overflow {
                return Err("keyboard event queue overflow; disconnecting");
            }
            std::mem::take(&mut q.events)
        };
        for key in window.get_keys_pressed(KeyRepeat::Yes) {
            if repeatable(key)
                && self.keys.contains(&key)
                && !transitions.iter().any(|(k, _)| *k == key)
            {
                transitions.push((key, true));
            }
        }
        let position = window
            .get_unscaled_mouse_pos(MouseMode::Pass)
            .and_then(|p| Viewport::new(window.get_size(), remote).point(p, remote));
        let buttons = [MouseButton::Left, MouseButton::Right, MouseButton::Middle]
            .map(|b| window.get_mouse_down(b));
        // minifb 0.28 exposes raw Wayland axis distances (down positive),
        // but X11 exposes wheel steps (up positive). Use 15 axis units/step.
        let wheel = window
            .get_scroll_wheel()
            .map_or(0., |(_, y)| if self.wayland { -y / 15. } else { y });
        Ok(self.sample(focused, keys, transitions, position, buttons, wheel))
    }
    fn sample(
        &mut self,
        focused: bool,
        keys: BTreeSet<Key>,
        transitions: Vec<(Key, bool)>,
        position: Option<(u16, u16)>,
        buttons: [bool; 3],
        wheel: f32,
    ) -> Vec<Input> {
        let mut events = Vec::new();
        if !focused {
            if self.focused {
                events.push(Input::ReleaseAll);
            }
            *self = Self {
                queue: self.queue.clone(),
                wayland: self.wayland,
                ..Self::default()
            };
            return events;
        }
        if !self.focused {
            self.focused = true;
            self.ignored = keys;
            self.ignored_buttons = buttons;
            return events;
        }
        for (key, down) in transitions {
            if self.ignored.contains(&key) {
                if !down {
                    self.ignored.remove(&key);
                }
                continue;
            }
            if down {
                self.keys.insert(key);
            } else if !self.keys.remove(&key) {
                continue;
            }
            if let Some(event) = key_event(key, down) {
                events.push(event);
            }
        }
        if let Some((x, y)) = position {
            if self.position != position {
                events.push(Input::Move { x, y });
            }
            self.position = position;
        }
        for (i, button) in buttons.into_iter().enumerate() {
            if !button {
                self.ignored_buttons[i] = false;
            }
            let down = button && !self.ignored_buttons[i] && position.is_some();
            if down != self.buttons[i] {
                if let Some((x, y)) = self.position {
                    events.push(Input::Button {
                        button: i as u8 + 1,
                        down,
                        x,
                        y,
                    });
                }
                self.buttons[i] = down;
            }
            // Leaving the viewport cancels drags; reentry must not click again.
            if position.is_none() && button {
                self.ignored_buttons[i] = true;
            }
        }
        if position.is_some() && wheel.is_finite() {
            self.wheel = (self.wheel + wheel * 120.).clamp(-1200., 1200.);
            while self.wheel.abs() >= 1. {
                let delta = self.wheel.clamp(-255., 255.) as i16;
                events.push(Input::Wheel { delta });
                self.wheel -= f32::from(delta);
            }
        }
        events
    }
}
fn repeatable(key: Key) -> bool {
    !matches!(
        key,
        Key::LeftCtrl
            | Key::RightCtrl
            | Key::LeftShift
            | Key::RightShift
            | Key::LeftAlt
            | Key::RightAlt
            | Key::LeftSuper
            | Key::RightSuper
            | Key::CapsLock
            | Key::NumLock
            | Key::ScrollLock
    )
}
fn key_event(key: Key, down: bool) -> Option<Input> {
    use Key::*;
    let scan: u16 = match key {
        Escape => 0x01,
        Key1 => 0x02,
        Key2 => 0x03,
        Key3 => 0x04,
        Key4 => 0x05,
        Key5 => 0x06,
        Key6 => 0x07,
        Key7 => 0x08,
        Key8 => 0x09,
        Key9 => 0x0a,
        Key0 => 0x0b,
        Minus => 0x0c,
        Equal => 0x0d,
        Backspace => 0x0e,
        Tab => 0x0f,
        Q => 0x10,
        W => 0x11,
        E => 0x12,
        R => 0x13,
        T => 0x14,
        Y => 0x15,
        U => 0x16,
        I => 0x17,
        O => 0x18,
        P => 0x19,
        LeftBracket => 0x1a,
        RightBracket => 0x1b,
        Enter => 0x1c,
        LeftCtrl => 0x1d,
        A => 0x1e,
        S => 0x1f,
        D => 0x20,
        F => 0x21,
        G => 0x22,
        H => 0x23,
        J => 0x24,
        K => 0x25,
        L => 0x26,
        Semicolon => 0x27,
        Apostrophe => 0x28,
        Backquote => 0x29,
        LeftShift => 0x2a,
        Backslash => 0x2b,
        Z => 0x2c,
        X => 0x2d,
        C => 0x2e,
        V => 0x2f,
        B => 0x30,
        N => 0x31,
        M => 0x32,
        Comma => 0x33,
        Period => 0x34,
        Slash => 0x35,
        RightShift => 0x36,
        NumPadAsterisk => 0x37,
        LeftAlt => 0x38,
        Space => 0x39,
        CapsLock => 0x3a,
        F1 => 0x3b,
        F2 => 0x3c,
        F3 => 0x3d,
        F4 => 0x3e,
        F5 => 0x3f,
        F6 => 0x40,
        F7 => 0x41,
        F8 => 0x42,
        F9 => 0x43,
        F10 => 0x44,
        NumLock => 0x45,
        ScrollLock => 0x46,
        NumPad7 => 0x47,
        NumPad8 => 0x48,
        NumPad9 => 0x49,
        NumPadMinus => 0x4a,
        NumPad4 => 0x4b,
        NumPad5 => 0x4c,
        NumPad6 => 0x4d,
        NumPadPlus => 0x4e,
        NumPad1 => 0x4f,
        NumPad2 => 0x50,
        NumPad3 => 0x51,
        NumPad0 => 0x52,
        NumPadDot => 0x53,
        F11 => 0x57,
        F12 => 0x58,
        NumPadEnter => 0x11c,
        RightCtrl => 0x11d,
        NumPadSlash => 0x135,
        RightAlt => 0x138,
        Home => 0x147,
        Up => 0x148,
        PageUp => 0x149,
        Left => 0x14b,
        Right => 0x14d,
        End => 0x14f,
        Down => 0x150,
        PageDown => 0x151,
        Insert => 0x152,
        Delete => 0x153,
        LeftSuper => 0x15b,
        RightSuper => 0x15c,
        Menu => 0x15d,
        _ => return None,
    };
    Some(Input::Key {
        code: scan as u8,
        extended: scan & 0x100 != 0,
        down,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn callback_preserves_short_taps_and_bounds_event_storage() {
        let queue = Rc::new(RefCell::new(KeyQueue::default()));
        let mut callback = Callback(queue.clone());
        callback.set_key_state(Key::A, true);
        callback.set_key_state(Key::A, false);
        assert_eq!(queue.borrow().events, [(Key::A, true), (Key::A, false)]);
        for _ in 0..200 {
            callback.set_key_state(Key::A, true);
        }
        assert!(queue.borrow().overflow);
        assert_eq!(queue.borrow().events.len(), 192);
    }
    #[test]
    fn modifier_release_before_next_key_keeps_event_order() {
        assert!(!repeatable(Key::LeftShift));
        assert!(!repeatable(Key::CapsLock));
        assert!(repeatable(Key::A));
        let mut c = Controller {
            focused: true,
            ..Controller::default()
        };
        let transitions = vec![
            (Key::LeftCtrl, true),
            (Key::LeftCtrl, false),
            (Key::A, true),
            (Key::A, false),
        ];
        let expected: Vec<_> = transitions
            .iter()
            .map(|&(k, down)| key_event(k, down).unwrap())
            .collect();
        assert_eq!(
            c.sample(true, BTreeSet::new(), transitions, None, [false; 3], 0.),
            expected
        );
    }
    #[test]
    fn centered_viewport_maps_pixels_and_excludes_borders() {
        let v = Viewport::new((800, 800), (1024, 768));
        assert_eq!((v.x, v.y, v.width, v.height), (0, 100, 800, 600));
        assert_eq!(v.point((400., 400.), (1024, 768)), Some((512, 384)));
        assert_eq!(v.point((0., 99.), (1024, 768)), None);
        assert_eq!(v.point((800., 700.), (1024, 768)), None);
        let v = Viewport::new((6, 2), (2, 2));
        let mut out = Vec::new();
        v.render(&[1, 2, 3, 4], (2, 2), (6, 2), &mut out).unwrap();
        assert_eq!(out, [0, 0, 1, 2, 0, 0, 0, 0, 3, 4, 0, 0]);
    }
    #[test]
    fn matching_remote_uses_the_full_native_window() {
        let viewport = Viewport::new((1280, 720), (1280, 720));
        assert_eq!(
            (viewport.x, viewport.y, viewport.width, viewport.height),
            (0, 0, 1280, 720)
        );
        assert_eq!(viewport.point((0., 0.), (1280, 720)), Some((0, 0)));
        assert_eq!(
            viewport.point((1279., 719.), (1280, 720)),
            Some((1279, 719))
        );
    }
    #[test]
    fn focus_loss_releases_and_regaining_focus_ignores_held_keys() {
        let mut c = Controller::default();
        c.sample(true, BTreeSet::new(), vec![], Some((1, 2)), [false; 3], 0.);
        let e = c.sample(
            true,
            [Key::A, Key::LeftCtrl].into(),
            vec![(Key::LeftCtrl, true), (Key::A, true)],
            Some((1, 2)),
            [true, false, false],
            0.,
        );
        assert_eq!(e[0], key_event(Key::LeftCtrl, true).unwrap());
        assert_eq!(
            c.sample(false, BTreeSet::new(), vec![], None, [false; 3], 0.),
            [Input::ReleaseAll]
        );
        assert!(
            c.sample(true, [Key::A].into(), vec![], None, [false; 3], 0.)
                .is_empty()
        );
        assert!(
            c.sample(
                true,
                [Key::A].into(),
                vec![(Key::A, true)],
                None,
                [false; 3],
                0.
            )
            .is_empty()
        );
    }
    #[test]
    fn epoch_reset_clears_overflow_and_ignores_held_inputs() {
        let mut c = Controller {
            focused: true,
            keys: [Key::A].into(),
            buttons: [true, false, false],
            position: Some((5, 6)),
            wheel: 60.,
            ..Controller::default()
        };
        {
            let mut queue = c.queue.borrow_mut();
            queue.events.push((Key::A, false));
            queue.overflow = true;
        }

        c.reset();

        let queue = c.queue.borrow();
        assert!(queue.events.is_empty());
        assert!(!queue.overflow);
        drop(queue);
        assert!(!c.focused);
        assert!(c.keys.is_empty());
        assert_eq!(c.buttons, [false; 3]);
        assert_eq!(c.position, None);
        assert_eq!(c.wheel, 0.);
        assert!(
            c.sample(
                true,
                [Key::A].into(),
                vec![],
                Some((5, 6)),
                [true, false, false],
                0.,
            )
            .is_empty()
        );
        assert_eq!(c.ignored, [Key::A].into());
        assert_eq!(c.ignored_buttons, [true, false, false]);
    }
    #[test]
    fn short_taps_and_drag_exit_produce_releases() {
        let mut c = Controller {
            focused: true,
            ..Controller::default()
        };
        let e = c.sample(
            true,
            BTreeSet::new(),
            vec![(Key::A, true), (Key::A, false)],
            Some((5, 6)),
            [true, false, false],
            0.,
        );
        assert_eq!(
            &e[..2],
            &[
                key_event(Key::A, true).unwrap(),
                key_event(Key::A, false).unwrap()
            ]
        );
        assert_eq!(
            c.sample(
                true,
                BTreeSet::new(),
                vec![],
                None,
                [true, false, false],
                0.
            ),
            [Input::Button {
                button: 1,
                down: false,
                x: 5,
                y: 6
            }]
        );
        assert!(
            c.sample(
                true,
                BTreeSet::new(),
                vec![],
                Some((5, 6)),
                [true, false, false],
                0.
            )
            .is_empty()
        );
    }
}
