//! Native first-desktop viewer. Window/event handling stays on the main thread.
use crate::{session, tls};
mod input;
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
struct Display {
    width: usize,
    height: usize,
    pixels: Vec<u32>,
    revision: u64,
    active: bool,
    error: Option<String>,
    status: String,
}

pub fn run(
    connection: &mut rustls::ClientConnection,
    stream: &mut TcpStream,
    protocol: SecurityProtocol,
    account: &str,
    identity: sspi::AuthIdentity,
    host: &str,
) -> Result<(), Error> {
    let (server, user) = session::connect_channels(connection, stream, protocol)?;
    let mut state = Session::new(user, server.io_channel)?;
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
    let mut window = Window::new(
        "LinRDP — Connecting",
        1024,
        768,
        WindowOptions {
            resize: true,
            ..WindowOptions::default()
        },
    )?;
    window.set_target_fps(60);
    let shared = Mutex::new(Display {
        width: 1024,
        height: 768,
        pixels: vec![0; 1024 * 768],
        revision: 0,
        active: false,
        error: None,
        status: "Connecting".into(),
    });
    let stop = AtomicBool::new(false);
    let shutdown = stream.try_clone()?;
    let (sender, receiver) = mpsc::sync_channel::<Vec<Input>>(128);
    std::thread::scope(|scope| -> Result<(), Error> {
        let shared = &shared;
        let stop = &stop;
        let worker = scope.spawn(move || {
            let result = receive(connection, stream, &mut state, shared, stop, &receiver);
            // Release only input actually sent, including when the UI closes.
            if let Ok(Some(packet)) = state.input(&[Input::ReleaseAll])
                && let Ok(packet) = data::encode(&packet)
            {
                let _ = tls::write_plaintext(connection, stream, &packet);
            }
            if let Err(error) = result {
                shared.lock().unwrap().error = Some(error.to_string());
            }
        });
        let result = (|| -> Result<(), Error> {
            let mut pixels = vec![0; 1024 * 768];
            let mut width = 1024;
            let mut height = 768;
            let mut revision = 0;
            let mut shown = false;
            let mut controller = input::Controller::attach(&mut window)?;
            let mut rendered = Vec::new();
            let mut rendered_size = (0, 0);
            let mut rendered_revision = u64::MAX;
            while window.is_open() {
                let ready;
                {
                    let frame = shared.lock().unwrap();
                    if let Some(error) = &frame.error {
                        return Err(error.clone().into());
                    }
                    if revision == 0 {
                        window.set_title(&format!("LinRDP — {}", frame.status));
                    }
                    if frame.revision != revision {
                        pixels.clone_from(&frame.pixels);
                        width = frame.width;
                        height = frame.height;
                        revision = frame.revision;
                        window.set_title(&format!("LinRDP — {host} — {width}×{height}"));
                    }
                    ready = frame.active && revision != 0;
                }
                let size = window.get_size();
                if size.0 == 0 || size.1 == 0 {
                    window.update();
                    let events = controller.poll(&mut window, false, (width, height))?;
                    if !events.is_empty() {
                        sender
                            .try_send(events)
                            .map_err(|_| "input queue unavailable")?;
                    }
                    continue;
                }
                if size != rendered_size || revision != rendered_revision {
                    input::Viewport::new(size, (width, height)).render(
                        &pixels,
                        (width, height),
                        size,
                        &mut rendered,
                    )?;
                    rendered_size = size;
                    rendered_revision = revision;
                }
                window.update_with_buffer(&rendered, size.0, size.1)?;
                let events = controller.poll(&mut window, ready, (width, height))?;
                if !events.is_empty() {
                    sender.try_send(events).map_err(
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
        worker.join().map_err(|_| "desktop worker panicked")?;
        let _ = shutdown.shutdown(Shutdown::Both);
        result
    })
}

fn receive(
    connection: &mut rustls::ClientConnection,
    stream: &mut TcpStream,
    state: &mut Session,
    shared: &Mutex<Display>,
    stop: &AtomicBool,
    input: &Receiver<Vec<Input>>,
) -> Result<(), Error> {
    let mut pending = Vec::new();
    let mut bytes = [0u8; 16384];
    let mut last_phase = state.phase;
    let mut updates = state.revision;
    let mut deadline = Instant::now() + Duration::from_secs(30);
    let mut partial_since = None;
    while !stop.load(Ordering::Relaxed) {
        // A bounded channel preserves input ordering without blocking the UI.
        for events in input.try_iter().take(128) {
            if stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            if let Some(packet) = state.input(&events)? {
                tls::write_plaintext(connection, stream, &data::encode(&packet)?)?;
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
        match tls::read_chunk(connection, stream, &mut bytes) {
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
                        state.receive(data::decode(&pending[..length])?)?
                    } else {
                        state.receive_fastpath(&pending[..length])?;
                        Vec::new()
                    };
                    for message in state.notifications.drain(..) {
                        println!("{message}");
                        shared.lock().unwrap().status = message;
                    }
                    for reply in replies {
                        tls::write_plaintext(connection, stream, &data::encode(&reply)?)?;
                    }
                    pending.drain(..length);
                    partial_since = if pending.is_empty() {
                        None
                    } else {
                        Some(Instant::now())
                    };
                    if state.phase != last_phase {
                        println!("Desktop phase: {:?}.", state.phase);
                        last_phase = state.phase;
                        shared.lock().unwrap().active = state.phase == Phase::Active;
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
                    if state.revision != updates && state.framebuffer.updates > 0 {
                        let mut frame = shared.lock().unwrap();
                        frame.width = usize::from(state.framebuffer.width);
                        frame.height = usize::from(state.framebuffer.height);
                        state.copy_display(&mut frame.pixels);
                        frame.revision += 1;
                        frame.active = state.phase == Phase::Active;
                        updates = state.revision;
                    }
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut
                        | io::ErrorKind::WouldBlock
                        | io::ErrorKind::Interrupted
                ) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
