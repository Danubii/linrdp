//! Native first-desktop viewer. Window/event handling stays on the main thread.
use crate::{session, tls};
mod input;
mod presentation;
mod resize;
mod transport;
use linrdp_proto::{
    data,
    desktop::{Input, Phase, Session, frame_length},
    negotiation::SecurityProtocol,
};
use minifb::{Window, WindowOptions};
use std::{
    io,
    net::{Shutdown, TcpStream},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    time::{Duration, Instant},
};

type Error = Box<dyn std::error::Error>;
#[derive(Default)]
struct Display {
    width: usize,
    height: usize,
    pixels: linrdp_proto::desktop::Snapshot,
    revision: u64,
    pending: bool,
    active: bool,
    error: Option<String>,
    status: String,
    epoch: u64,
    window_size: (usize, usize),
}

pub fn run(
    connection: &mut rustls::ClientConnection,
    stream: &mut TcpStream,
    protocol: SecurityProtocol,
    account: &str,
    identity: sspi::AuthIdentity,
    host: &str,
    settings: linrdp_proto::mcs::Settings,
) -> Result<(), Error> {
    stream.set_nodelay(true)?;
    let (server, user) = session::connect_channels(connection, stream, protocol, settings)?;
    let mut state = Session::new(user, server.io_channel)?;
    let mut clipboard = if settings.clipboard {
        server.static_channels[0].map(|channel| crate::clipboard::Clipboard::new(user, channel))
    } else {
        None
    };
    let mut resize = if settings.dynamic_resolution || settings.h264 {
        server.static_channels[usize::from(settings.clipboard)].map(|channel| {
            if settings.h264 {
                resize::Resize::with_graphics(user, channel, settings.dynamic_resolution)
            } else {
                resize::Resize::new(user, channel)
            }
        })
    } else {
        None
    };
    let clipboard_focus = clipboard.as_ref().map(|c| c.focused());
    let (domain, username) = crate::nla::account(account)?;
    let info = state.client_info(
        domain,
        username,
        identity.password.as_ref(),
        stream.local_addr()?.ip(),
    )?;
    let packet = zeroize::Zeroizing::new(data::encode(&info)?);
    tls::write_plaintext(connection, stream, &packet)?;
    drop(packet);
    drop(info);
    drop(identity);
    println!("Client Info sent; waiting for licensing and desktop activation.");
    let (initial_width, initial_height) =
        (usize::from(settings.width), usize::from(settings.height));
    let mut window = Window::new(
        "Fjern — Connecting",
        initial_width,
        initial_height,
        WindowOptions {
            resize: true,
            ..WindowOptions::default()
        },
    )?;
    // Poll input at 120 Hz; unchanged desktops do not submit pixel buffers.
    window.set_target_fps(120);
    let shared = Mutex::new(Display {
        width: initial_width,
        height: initial_height,
        pixels: Default::default(),
        revision: 0,
        pending: false,
        active: false,
        error: None,
        status: "Connecting".into(),
        epoch: 0,
        window_size: (initial_width, initial_height),
    });
    let stop = AtomicBool::new(false);
    let shutdown = stream.try_clone()?;
    let mut transport = transport::Transport::new(stream.try_clone()?)?;
    let (sender, receiver) = mpsc::sync_channel::<(u64, Vec<Input>)>(128);
    std::thread::scope(|scope| -> Result<(), Error> {
        let shared = &shared;
        let stop = &stop;
        let worker = scope.spawn(move || {
            let result = receive(
                connection,
                &mut transport,
                &mut state,
                shared,
                stop,
                &receiver,
                &mut Channels {
                    clipboard: &mut clipboard,
                    resize: &mut resize,
                },
            );
            // Release only input actually sent, including when the UI closes.
            if let Ok(Some(packet)) = state.input(&[Input::ReleaseAll])
                && let Ok(packet) = data::encode(&packet)
            {
                let _ = transport.write_plaintext(connection, &packet);
            }
            if let Err(error) = result {
                shared.lock().unwrap().error = Some(error.to_string());
            }
        });
        let result = (|| -> Result<(), Error> {
            let mut pixels = linrdp_proto::desktop::Snapshot::default();
            pixels.pixels.resize(initial_width * initial_height, 0);
            let mut width = initial_width;
            let mut height = initial_height;
            let mut revision = 0;
            let mut shown = false;
            let mut controller = input::Controller::attach(&mut window)?;
            let mut input_epoch = 0;
            let mut rendered = Vec::new();
            let mut rendered_size = (0, 0);
            let mut rendered_revision = u64::MAX;
            let mut rendered_remote = (0, 0);
            while window.is_open() {
                let ready;
                {
                    let mut frame = shared.lock().unwrap();
                    frame.window_size = window.get_size();
                    if input_epoch != frame.epoch {
                        controller.reset();
                        input_epoch = frame.epoch;
                    }
                    if let Some(error) = &frame.error {
                        return Err(error.clone().into());
                    }
                    if revision == 0 {
                        window.set_title(&format!("Fjern — {}", frame.status));
                    }
                    if frame.revision != revision {
                        frame.take_pixels(&mut pixels);
                        width = frame.width;
                        height = frame.height;
                        revision = frame.revision;
                        if !shown || (width, height) != rendered_remote {
                            window.set_title(&format!("Fjern — {host} — {width}×{height}"));
                        }
                    }
                    ready = frame.active && revision != 0;
                }
                let size = window.get_size();
                if size.0 == 0 || size.1 == 0 {
                    if let Some(focused) = &clipboard_focus {
                        focused.store(false, Ordering::Relaxed);
                    }
                    window.update();
                    let events = controller.poll(&mut window, false, (width, height))?;
                    if !events.is_empty() {
                        sender
                            .try_send((input_epoch, events))
                            .map_err(|_| "input queue unavailable")?;
                    }
                    continue;
                }
                if size != rendered_size || revision != rendered_revision || window.needs_redraw() {
                    if size == (width, height) {
                        window.update_with_buffer(&pixels.pixels, width, height)?;
                    } else {
                        input::Viewport::new(size, (width, height)).render(
                            &pixels.pixels,
                            (width, height),
                            size,
                            &mut rendered,
                        )?;
                        window.update_with_buffer(&rendered, size.0, size.1)?;
                    }
                    rendered_size = size;
                    rendered_remote = (width, height);
                    rendered_revision = revision;
                } else {
                    // Keep focus, resize and input events moving without uploading
                    // an identical desktop to the compositor.
                    window.update();
                }
                if let Some(focused) = &clipboard_focus {
                    focused.store(ready && window.is_active(), Ordering::Relaxed);
                }
                let events = controller.poll(&mut window, ready, (width, height))?;
                if !events.is_empty() {
                    sender.try_send((input_epoch, events)).map_err(
                        |_| "input queue unavailable; disconnecting to avoid lost key releases",
                    )?;
                }
                if ready && !shown {
                    println!(
                        "First remote bitmap displayed: {width}x{height}. Close the window to disconnect; the remote account is not signed out."
                    );
                    shown = true;
                }
            }
            Ok(())
        })();
        stop.store(true, Ordering::Relaxed);
        let _ = shutdown.shutdown(Shutdown::Read);
        worker.join().map_err(|_| "desktop worker panicked")?;
        let _ = shutdown.shutdown(Shutdown::Both);
        if result.is_err()
            && let Some(error) = &shared.lock().unwrap().error
        {
            return Err(error.clone().into());
        }
        result
    })
}

struct Channels<'a> {
    clipboard: &'a mut Option<crate::clipboard::Clipboard>,
    resize: &'a mut Option<resize::Resize>,
}
fn receive(
    connection: &mut rustls::ClientConnection,
    stream: &mut transport::Transport,
    state: &mut Session,
    shared: &Mutex<Display>,
    stop: &AtomicBool,
    input: &Receiver<(u64, Vec<Input>)>,
    channels: &mut Channels<'_>,
) -> Result<(), Error> {
    let mut pending = Vec::new();
    let mut bytes = [0u8; 16384];
    let mut last_phase = state.phase;
    let mut updates = state.revision;
    let mut staging = linrdp_proto::desktop::Snapshot::default();
    let mut deadline = Instant::now() + Duration::from_secs(30);
    let mut partial_since = None;
    let mut batch = presentation::Batch::default();
    while !stop.load(Ordering::Relaxed) {
        if let Some(resize) = channels.resize.as_mut() {
            let desired = shared.lock().unwrap().window_size;
            if let Some(packets) = resize.poll(
                Instant::now(),
                desired,
                (state.framebuffer.width, state.framebuffer.height),
                state.phase == Phase::Active,
                state.framebuffer.updates > 0,
            )? {
                if let Some(packet) = state.input(&[Input::ReleaseAll])? {
                    stream.write_plaintext(connection, &data::encode(&packet)?)?;
                }
                {
                    let mut frame = shared.lock().unwrap();
                    frame.epoch += 1;
                    frame.active = false;
                }
                for packet in packets {
                    stream.write_plaintext(connection, &data::encode(&packet)?)?;
                }
            }
            shared.lock().unwrap().active =
                state.phase == Phase::Active && state.framebuffer.updates > 0 && !resize.waiting();
        }
        if let Some(clipboard) = channels.clipboard.as_mut() {
            for packet in clipboard.poll().map_err(|e| e.to_string())? {
                stream.write_plaintext(connection, &data::encode(&packet)?)?;
            }
        }
        // A bounded channel preserves input ordering without blocking the UI.
        for (epoch, events) in input.try_iter().take(128) {
            if epoch != shared.lock().unwrap().epoch
                || channels.resize.as_ref().is_some_and(|r| r.waiting())
            {
                continue;
            }
            if stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            if let Some(packet) = state.input(&events)? {
                stream.write_plaintext(connection, &data::encode(&packet)?)?;
            }
        }
        if state.phase == Phase::Active
            && state.framebuffer.updates == 0
            && Instant::now() > deadline
        {
            return Err("Windows activated the session but did not send a bitmap before the first-frame deadline".into());
        }
        if state.phase != Phase::Active && Instant::now() > deadline {
            return Err("desktop activation timed out".into());
        }
        if partial_since.is_some_and(|started: Instant| started.elapsed() > Duration::from_secs(10))
        {
            return Err("incomplete desktop packet timed out".into());
        }
        let before = state.revision;
        let graphics_before = channels.resize.as_ref().map(|r| r.graphics_revision());
        let mut idle = false;
        match stream.read_chunk(connection, &mut bytes, batch.read_budget()) {
            Ok(0) => return Err("server closed the desktop connection".into()),
            Ok(count) => {
                if pending.is_empty() {
                    partial_since = Some(Instant::now());
                }
                pending.extend_from_slice(&bytes[..count]);
                if pending.len() > u16::MAX as usize + bytes.len() {
                    return Err("desktop receive buffer exceeded limit".into());
                }
                while let Some(length) = frame_length(&pending)? {
                    if pending.len() < length {
                        break;
                    }
                    let replies = if pending[0] == 3 {
                        let payload = data::decode(&pending[..length])?;
                        if payload.first() == Some(&0x68) {
                            let (channel, body) = linrdp_proto::channel::indication(payload)?;
                            if let Some(clipboard) = channels
                                .clipboard
                                .as_mut()
                                .filter(|c| c.channel() == channel)
                            {
                                clipboard.receive(body).map_err(|e| e.to_string())?
                            } else if let Some(resize) =
                                channels.resize.as_mut().filter(|r| r.channel == channel)
                            {
                                resize.receive(body, state)?
                            } else {
                                state.receive(payload)?
                            }
                        } else {
                            state.receive(payload)?
                        }
                    } else {
                        state.receive_fastpath(&pending[..length])?;
                        Vec::new()
                    };
                    for message in state.notifications.drain(..) {
                        println!("{message}");
                        shared.lock().unwrap().status = message;
                    }
                    for reply in replies {
                        stream.write_plaintext(connection, &data::encode(&reply)?)?;
                    }
                    pending.drain(..length);
                    partial_since = if pending.is_empty() {
                        None
                    } else {
                        Some(Instant::now())
                    };
                    if state.phase != last_phase {
                        println!("Desktop phase: {:?}.", state.phase);
                        {
                            let mut frame = shared.lock().unwrap();
                            if last_phase == Phase::Active {
                                frame.epoch += 1;
                            }
                            frame.active = state.phase == Phase::Active
                                && state.framebuffer.updates > 0
                                && !channels.resize.as_ref().is_some_and(|r| r.waiting());
                        }
                        last_phase = state.phase;
                        deadline = Instant::now()
                            + Duration::from_secs(if state.phase == Phase::Active {
                                90
                            } else {
                                30
                            });
                        if state.phase == Phase::Finalizing {
                            updates = 0;
                        }
                    }
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut
                        | io::ErrorKind::WouldBlock
                        | io::ErrorKind::Interrupted
                ) =>
            {
                idle = error.kind() != io::ErrorKind::Interrupted;
            }
            Err(error) => return Err(error.into()),
        }
        if state.revision != before {
            let complete =
                graphics_before != channels.resize.as_ref().map(|r| r.graphics_revision());
            batch.changed(Instant::now(), complete);
        }
        let partial = !pending.is_empty() || state.bitmap_fragment_pending();
        let slot_pending = shared.lock().unwrap().pending;
        if batch.due(Instant::now(), idle, partial, slot_pending)
            && presentation::publish(
                state,
                shared,
                &mut staging,
                &mut updates,
                !channels.resize.as_ref().is_some_and(|r| r.waiting()),
            )
        {
            batch.published();
        }
    }
    Ok(())
}
