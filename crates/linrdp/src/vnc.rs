//! Plain RFB desktop frontend. Protocol decoding is provided by vnc-rs.
use minifb::{InputCallback, Key, MouseButton, MouseMode, Window, WindowOptions};
use std::{
    collections::BTreeMap,
    error::Error,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use vnc::{Rect, VncEvent, X11Event};
type Result<T> = std::result::Result<T, Box<dyn Error>>;
const MAX_PIXELS: usize = 16 * 1024 * 1024;
fn invalid(s: &str) -> Box<dyn Error> {
    std::io::Error::new(std::io::ErrorKind::InvalidData, s).into()
}
#[derive(Default)]
struct Canvas {
    width: usize,
    height: usize,
    pixels: Vec<u32>,
}
impl Canvas {
    fn resize(&mut self, w: usize, h: usize) -> Result<()> {
        if w == 0 || h == 0 || w > 8192 || h > 8192 || w * h > MAX_PIXELS {
            return Err(invalid("VNC desktop exceeds supported dimensions"));
        }
        self.width = w;
        self.height = h;
        self.pixels = vec![0; w * h];
        Ok(())
    }
    fn check(&self, r: Rect) -> Result<()> {
        if r.width == 0
            || r.height == 0
            || usize::from(r.x) + usize::from(r.width) > self.width
            || usize::from(r.y) + usize::from(r.height) > self.height
        {
            return Err(invalid("VNC rectangle outside desktop"));
        }
        Ok(())
    }
    fn event(&mut self, event: VncEvent) -> Result<bool> {
        match event {
            VncEvent::SetResolution(s) => self.resize(s.width.into(), s.height.into())?,
            VncEvent::RawImage(r, data) => {
                self.check(r)?;
                if data.len() != usize::from(r.width) * usize::from(r.height) * 4 {
                    return Err(invalid("VNC pixel length mismatch"));
                }
                for (row, bytes) in data.chunks(usize::from(r.width) * 4).enumerate() {
                    let dst = (usize::from(r.y) + row) * self.width + usize::from(r.x);
                    for (i, p) in bytes.as_chunks::<4>().0.iter().enumerate() {
                        self.pixels[dst + i] = u32::from_le_bytes(*p) & 0xffffff;
                    }
                }
            }
            VncEvent::Copy(dst, src) => {
                self.check(src)?;
                self.check(dst)?;
                if (src.width, src.height) != (dst.width, dst.height) {
                    return Err(invalid("VNC CopyRect size mismatch"));
                }
                let rows: Vec<_> = (0..usize::from(src.height))
                    .flat_map(|y| {
                        let at = (usize::from(src.y) + y) * self.width + usize::from(src.x);
                        self.pixels[at..at + usize::from(src.width)].iter().copied()
                    })
                    .collect();
                for (y, row) in rows.chunks(usize::from(dst.width)).enumerate() {
                    let at = (usize::from(dst.y) + y) * self.width + usize::from(dst.x);
                    self.pixels[at..at + row.len()].copy_from_slice(row);
                }
            }
            VncEvent::Error(e) => return Err(invalid(&format!("VNC connection: {e}"))),
            VncEvent::JpegImage(..) | VncEvent::SetPixelFormat(_) => {
                return Err(invalid("Unexpected VNC image encoding"));
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}
#[derive(Default)]
struct Keyboard {
    held: BTreeMap<Key, u32>,
    events: Vec<X11Event>,
    overflow: bool,
    pending_text: Option<u32>,
}
impl Keyboard {
    fn emit(&mut self, keycode: u32, down: bool) {
        if self.events.len() < 1024 {
            self.events.push(X11Event::KeyEvent((keycode, down).into()));
        } else {
            self.overflow = true;
        }
    }
    fn flush_text(&mut self) {
        if let Some(code) = self.pending_text.take() {
            // Some layout symbols have no minifb Key identity. Deliver their text
            // as a complete stroke, without assigning it to an unrelated held key.
            self.emit(code, true);
            self.emit(code, false);
        }
    }
    fn key(&mut self, key: Key, down: bool) {
        if down {
            let shift =
                self.held.contains_key(&Key::LeftShift) || self.held.contains_key(&Key::RightShift);
            if let Some(mut code) = keysym(key, shift) {
                if code < 0xff00 {
                    if let Some(text) = self.pending_text.take() {
                        code = text;
                    }
                } else {
                    self.flush_text();
                }
                let code = *self.held.entry(key).or_insert(code);
                self.emit(code, true);
            } else {
                self.flush_text();
            }
        } else {
            self.flush_text();
            if let Some(code) = self.held.remove(&key) {
                self.emit(code, false);
            }
        }
    }
    fn text(&mut self, scalar: u32) {
        self.flush_text();
        if char::from_u32(scalar).is_none_or(char::is_control)
            || [
                Key::LeftCtrl,
                Key::RightCtrl,
                Key::LeftAlt,
                Key::RightAlt,
                Key::LeftSuper,
                Key::RightSuper,
            ]
            .iter()
            .any(|k| self.held.contains_key(k))
        {
            return;
        }
        // Wayland and X11 invoke add_char BEFORE set_key_state for a press.
        self.pending_text = Some(if scalar <= 255 {
            scalar
        } else {
            0x01000000 | scalar
        });
    }
    fn drain(&mut self) -> Vec<X11Event> {
        self.flush_text();
        std::mem::take(&mut self.events)
    }
    fn release(&mut self) {
        self.pending_text = None;
        let keys = std::mem::take(&mut self.held);
        for code in keys.into_values() {
            self.emit(code, false);
        }
    }
}
struct Callback(Arc<Mutex<Keyboard>>);
impl InputCallback for Callback {
    fn add_char(&mut self, scalar: u32) {
        if let Ok(mut k) = self.0.lock() {
            k.text(scalar);
        }
    }
    fn set_key_state(&mut self, key: Key, state: bool) {
        if let Ok(mut k) = self.0.lock() {
            k.key(key, state);
        }
    }
}
fn keysym(k: Key, shift: bool) -> Option<u32> {
    use Key::*;
    let n = k as u32;
    if (A..=Z).contains(&k) {
        return Some(n - A as u32 + if shift { b'A' } else { b'a' } as u32);
    }
    if (Key0..=Key9).contains(&k) {
        return Some(if shift {
            b")!@#$%^&*("[n as usize] as u32
        } else {
            b'0' as u32 + n
        });
    }
    if (F1..=F15).contains(&k) {
        return Some(0xffbe + n - F1 as u32);
    }
    if (NumPad0..=NumPad9).contains(&k) {
        return Some(0xffb0 + n - NumPad0 as u32);
    }
    Some(match k {
        Backspace => 0xff08,
        Tab => 0xff09,
        Enter => 0xff0d,
        Escape => 0xff1b,
        Delete => 0xffff,
        Home => 0xff50,
        Left => 0xff51,
        Up => 0xff52,
        Right => 0xff53,
        Down => 0xff54,
        PageUp => 0xff55,
        PageDown => 0xff56,
        End => 0xff57,
        Insert => 0xff63,
        Menu => 0xff67,
        Pause => 0xff13,
        LeftShift => 0xffe1,
        RightShift => 0xffe2,
        LeftCtrl => 0xffe3,
        RightCtrl => 0xffe4,
        CapsLock => 0xffe5,
        LeftAlt => 0xffe9,
        RightAlt => 0xffea,
        LeftSuper => 0xffeb,
        RightSuper => 0xffec,
        NumLock => 0xff7f,
        ScrollLock => 0xff14,
        NumPadDot => 0xffae,
        NumPadSlash => 0xffaf,
        NumPadAsterisk => 0xffaa,
        NumPadMinus => 0xffad,
        NumPadPlus => 0xffab,
        NumPadEnter => 0xff8d,
        Space => 32,
        Apostrophe => {
            if shift {
                34
            } else {
                39
            }
        }
        Backquote => {
            if shift {
                126
            } else {
                96
            }
        }
        Backslash => {
            if shift {
                124
            } else {
                92
            }
        }
        Comma => {
            if shift {
                60
            } else {
                44
            }
        }
        Equal => {
            if shift {
                43
            } else {
                61
            }
        }
        LeftBracket => {
            if shift {
                123
            } else {
                91
            }
        }
        Minus => {
            if shift {
                95
            } else {
                45
            }
        }
        Period => {
            if shift {
                62
            } else {
                46
            }
        }
        RightBracket => {
            if shift {
                125
            } else {
                93
            }
        }
        Semicolon => {
            if shift {
                58
            } else {
                59
            }
        }
        Slash => {
            if shift {
                63
            } else {
                47
            }
        }
        _ => return None,
    })
}
fn pointer(x: f32, y: f32, size: (usize, usize), canvas: &Canvas) -> (u16, u16) {
    (
        ((x.max(0.0) as usize * canvas.width / size.0.max(1)).min(canvas.width - 1)) as u16,
        ((y.max(0.0) as usize * canvas.height / size.1.max(1)).min(canvas.height - 1)) as u16,
    )
}
fn button(b: MouseButton) -> u8 {
    match b {
        MouseButton::Left => 1,
        MouseButton::Middle => 2,
        MouseButton::Right => 4,
    }
}
/// Connect using RFB authentication and the server's negotiated transport.
pub fn run(host: &str, port: u16, user: Option<&str>) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let client = runtime.block_on(crate::vnc_transport::connect(host, port, user))?;
    let result = (|| -> Result<()> {
        let mut canvas = Canvas::default();
        let first = runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(15), client.recv_event()).await
        });
        // The initial event is generated during ServerInit.
        canvas.event(first??)?;
        if canvas.pixels.is_empty() {
            return Err(invalid("VNC server did not provide a desktop size"));
        }
        let mut window = Window::new(
            "LinRDP — VNC",
            canvas.width,
            canvas.height,
            WindowOptions {
                resize: true,
                ..WindowOptions::default()
            },
        )?;
        window.set_target_fps(120);
        let keyboard = Arc::new(Mutex::new(Keyboard::default()));
        window.set_input_callback(Box::new(Callback(keyboard.clone())));
        let mut mask = 0u8;
        let mut position = (0, 0);
        let mut last_refresh = Instant::now();
        let mut active = true;
        let mut rendered_size = (0, 0);
        let mut changed = true;
        while window.is_open() {
            for _ in 0..64 {
                match runtime.block_on(client.poll_event())? {
                    Some(event) => {
                        changed |= canvas.event(event)?;
                    }
                    None => break,
                }
            }
            let size = window.get_size();
            if changed || rendered_size != size || window.needs_redraw() {
                window.update_with_buffer(&canvas.pixels, canvas.width, canvas.height)?;
                rendered_size = size;
                changed = false;
            } else {
                window.update();
            }
            let focused = window.is_active();
            if active && !focused {
                keyboard
                    .lock()
                    .map_err(|_| invalid("VNC keyboard lock failed"))?
                    .release();
                mask = 0;
                runtime.block_on(
                    client.input(X11Event::PointerEvent((position.0, position.1, 0).into())),
                )?;
            }
            active = focused;
            let events = {
                let mut keys = keyboard
                    .lock()
                    .map_err(|_| invalid("VNC keyboard lock failed"))?;
                if keys.overflow {
                    return Err(invalid("VNC keyboard event queue overflow"));
                }
                if !focused {
                    keys.release();
                }
                keys.drain()
            };
            for event in events {
                if focused || matches!(&event,X11Event::KeyEvent(key) if !key.down) {
                    runtime.block_on(client.input(event))?;
                }
            }
            if let Some(edges) = window.take_mouse_button_events() {
                if focused {
                    for edge in edges {
                        position = pointer(edge.x, edge.y, window.get_size(), &canvas);
                        if edge.down {
                            mask |= button(edge.button)
                        } else {
                            mask &= !button(edge.button)
                        }
                        runtime.block_on(client.input(X11Event::PointerEvent(
                            (position.0, position.1, mask).into(),
                        )))?;
                    }
                }
            } else {
                mask = 0;
                runtime.block_on(
                    client.input(X11Event::PointerEvent((position.0, position.1, 0).into())),
                )?;
            }
            if focused {
                if let Some((x, y)) = window.get_mouse_pos(MouseMode::Clamp) {
                    let next = pointer(x, y, window.get_size(), &canvas);
                    if next != position {
                        position = next;
                        runtime.block_on(client.input(X11Event::PointerEvent(
                            (position.0, position.1, mask).into(),
                        )))?;
                    }
                }
                if let Some((_, scroll)) = window.get_scroll_wheel() {
                    let bit = if scroll > 0.0 { 8 } else { 16 };
                    for _ in 0..(scroll.abs().ceil() as usize).min(16) {
                        runtime.block_on(client.input(X11Event::PointerEvent(
                            (position.0, position.1, mask | bit).into(),
                        )))?;
                        runtime.block_on(client.input(X11Event::PointerEvent(
                            (position.0, position.1, mask).into(),
                        )))?;
                    }
                }
            }
            if last_refresh.elapsed() >= Duration::from_millis(16) {
                runtime.block_on(client.input(X11Event::Refresh))?;
                last_refresh = Instant::now();
            }
        }
        let events = {
            let mut keys = keyboard
                .lock()
                .map_err(|_| invalid("VNC keyboard lock failed"))?;
            keys.release();
            keys.drain()
        };
        for event in events {
            runtime.block_on(client.input(event))?;
        }
        runtime
            .block_on(client.input(X11Event::PointerEvent((position.0, position.1, 0).into())))?;
        Ok(())
    })();
    let _ = runtime.block_on(client.close());
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_and_copies_overlapping_rectangles() {
        let mut c = Canvas::default();
        c.resize(3, 2).unwrap();
        c.pixels = vec![1, 2, 3, 4, 5, 6];
        c.event(VncEvent::Copy(
            Rect {
                x: 1,
                y: 0,
                width: 2,
                height: 2,
            },
            Rect {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
            },
        ))
        .unwrap();
        assert_eq!(c.pixels, [1, 1, 2, 4, 4, 5]);
        assert!(c.resize(8192, 8192).is_err());
        assert!(
            c.event(VncEvent::RawImage(
                Rect {
                    x: 0,
                    y: 0,
                    width: 4,
                    height: 1
                },
                vec![]
            ))
            .is_err()
        );
    }
    #[test]
    fn shift_tab_and_releases_preserve_original_keysyms() {
        let mut k = Keyboard::default();
        k.key(Key::LeftShift, true);
        k.key(Key::A, true);
        k.key(Key::Tab, true);
        k.key(Key::LeftShift, false);
        k.key(Key::A, false);
        k.release();
        let actual: Vec<_> = k
            .events
            .iter()
            .map(|e| match e {
                X11Event::KeyEvent(e) => (e.keycode, e.down),
                _ => panic!(),
            })
            .collect();
        assert_eq!(
            actual,
            [
                (0xffe1, true),
                (65, true),
                (0xff09, true),
                (0xffe1, false),
                (65, false),
                (0xff09, false)
            ]
        );
    }
    #[test]
    fn layout_text_replaces_printable_press_and_matches_release() {
        let mut keyboard = Keyboard::default();
        keyboard.text('æ' as u32);
        keyboard.key(Key::A, true);
        keyboard.key(Key::A, false);
        let actual: Vec<_> = keyboard
            .events
            .iter()
            .map(|event| match event {
                X11Event::KeyEvent(key) => (key.keycode, key.down),
                _ => panic!(),
            })
            .collect();
        assert_eq!(actual, [(0xe6, true), (0xe6, false)]);
    }
    #[test]
    fn native_text_before_key_does_not_change_previous_held_key() {
        let mut k = Keyboard::default();
        k.text('a' as u32);
        k.key(Key::A, true);
        k.text('B' as u32);
        k.key(Key::B, true);
        k.key(Key::A, false);
        k.key(Key::B, false);
        let actual: Vec<_> = k
            .drain()
            .iter()
            .map(|e| match e {
                X11Event::KeyEvent(e) => (e.keycode, e.down),
                _ => panic!(),
            })
            .collect();
        assert_eq!(actual, [(97, true), (66, true), (97, false), (66, false)]);
    }
    #[test]
    fn unmapped_unicode_is_a_complete_stroke_at_drain() {
        let mut k = Keyboard::default();
        k.key(Key::A, true);
        k.text('界' as u32);
        let actual: Vec<_> = k
            .drain()
            .iter()
            .map(|e| match e {
                X11Event::KeyEvent(e) => (e.keycode, e.down),
                _ => panic!(),
            })
            .collect();
        assert_eq!(
            actual,
            [(97, true), (0x0100754c, true), (0x0100754c, false)]
        );
        assert_eq!(k.held.get(&Key::A), Some(&97));
    }
    #[test]
    fn scaled_pointer_is_clamped() {
        let mut c = Canvas::default();
        c.resize(100, 200).unwrap();
        assert_eq!(pointer(25.0, 50.0, (50, 100), &c), (50, 100));
        assert_eq!(pointer(1000.0, -2.0, (50, 100), &c), (99, 0));
    }
}
