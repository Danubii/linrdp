//! Native first-desktop viewer. Window/event handling stays on the main thread.
use crate::{session, tls};
use linrdp_proto::{
    data,
    desktop::{Phase, Session, frame_length},
    negotiation::SecurityProtocol,
};
use minifb::{ScaleMode, Window, WindowOptions};
use std::{
    io,
    net::{Shutdown, TcpStream},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
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
            scale_mode: ScaleMode::AspectRatioStretch,
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
    std::thread::scope(|scope| -> Result<(), Error> {
        let worker = scope.spawn(|| {
            if let Err(error) = receive(connection, stream, &mut state, &shared, &stop) {
                shared.lock().unwrap().error = Some(error.to_string());
            }
        });
        let result = (|| -> Result<(), Error> {
            let mut pixels = vec![0; 1024 * 768];
            let mut width = 1024;
            let mut height = 768;
            let mut revision = 0;
            let mut shown = false;
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
                        window
                            .set_title(&format!("LinRDP — {host} — {width}×{height} — View only"));
                    }
                    ready = frame.active && revision != 0;
                }
                window.update_with_buffer(&pixels, width, height)?;
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
        let _ = shutdown.shutdown(Shutdown::Both);
        worker.join().map_err(|_| "desktop worker panicked")?;
        result
    })
}

fn receive(
    connection: &mut rustls::ClientConnection,
    stream: &mut TcpStream,
    state: &mut Session,
    shared: &Mutex<Display>,
    stop: &AtomicBool,
) -> Result<(), Error> {
    let mut pending = Vec::new();
    let mut bytes = [0u8; 16384];
    let mut last_phase = state.phase;
    let mut updates = state.revision;
    let mut deadline = Instant::now() + Duration::from_secs(30);
    let mut partial_since = None;
    while !stop.load(Ordering::Relaxed) {
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
