use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use linrdp_proto::negotiation::{PROBE_REQUEST, Response, confirm_length, decode_confirm};

const TIMEOUT: Duration = Duration::from_secs(5);
const HELP: &str = "LinRDP — early development\n\nUsage: linrdp probe <host> [port]\n       linrdp --help\n       linrdp --version\n\nProbe RDP security negotiation (default port: 3389).\nUse an unbracketed IPv6 address with the port as a separate argument.\nNo TLS handshake, credentials, login or desktop session is performed.";

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("linrdp: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    if args.is_empty() || matches!(args.as_slice(), [flag] if flag == "--help" || flag == "-h") {
        println!("{HELP}");
        return Ok(());
    }
    if matches!(args.as_slice(), [flag] if flag == "--version" || flag == "-V") {
        println!("linrdp {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args[0] != "probe" || !(2..=3).contains(&args.len()) {
        return Err(format!("invalid arguments\n\n{HELP}").into());
    }
    let host = &args[1];
    if host.is_empty() || host.starts_with('-') {
        return Err("expected a hostname or IP address".into());
    }
    let port = args
        .get(2)
        .map(|value| value.parse::<u16>())
        .transpose()?
        .unwrap_or(3389);
    if port == 0 {
        return Err("port must be between 1 and 65535".into());
    }
    // System DNS resolution is outside our TCP deadline.
    let addresses: Vec<_> = (host.as_str(), port).to_socket_addrs()?.collect();
    if addresses.is_empty() {
        return Err("hostname resolved to no addresses".into());
    }
    let deadline = Instant::now() + TIMEOUT;
    let mut last_error = None;
    for address in addresses {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match TcpStream::connect_timeout(&address, remaining) {
            Ok(mut stream) => {
                let response = exchange(&mut stream)?;
                println!("Server {address} selected {}.", response.protocol);
                println!(
                    "Negotiation only: server identity, TLS and login have NOT been verified."
                );
                return Ok(());
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(format!(
        "could not connect: {}",
        last_error.map_or_else(
            || "connection timed out".to_owned(),
            |error| error.to_string()
        )
    )
    .into())
}

fn exchange(stream: &mut TcpStream) -> Result<Response, Box<dyn std::error::Error>> {
    stream.set_write_timeout(Some(TIMEOUT))?;
    stream.write_all(&PROBE_REQUEST)?;
    let deadline = Instant::now() + TIMEOUT;
    let mut packet = [0u8; 19];
    read_before(stream, &mut packet[..4], deadline)?;
    let length = confirm_length([packet[0], packet[1], packet[2], packet[3]])?;
    read_before(stream, &mut packet[4..length], deadline)?;
    Ok(decode_confirm(&packet[..length])?)
}

fn read_before(
    stream: &mut TcpStream,
    mut buffer: &mut [u8],
    deadline: Instant,
) -> std::io::Result<()> {
    while !buffer.is_empty() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "negotiation timed out",
            ));
        }
        stream.set_read_timeout(Some(remaining))?;
        match stream.read(buffer) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "server closed during negotiation",
                ));
            }
            Ok(count) => buffer = &mut buffer[count..],
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn exchanges_with_a_fragmented_loopback_peer() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut peer, _) = listener.accept().unwrap();
            peer.set_read_timeout(Some(TIMEOUT)).unwrap();
            let mut request = [0; 19];
            peer.read_exact(&mut request).unwrap();
            assert_eq!(
                request,
                [
                    3, 0, 0, 19, 14, 0xe0, 0, 0, 0, 0, 0, 1, 0, 8, 0, 11, 0, 0, 0
                ]
            );
            for byte in [3, 0, 0, 19, 14, 0xd0, 0, 0, 0, 0, 0, 2, 0, 8, 0, 2, 0, 0, 0] {
                peer.write_all(&[byte]).unwrap();
            }
        });
        let response = exchange(&mut TcpStream::connect(address).unwrap()).unwrap();
        assert_eq!(
            response.protocol,
            linrdp_proto::negotiation::SecurityProtocol::CredSsp
        );
        server.join().unwrap();
    }

    #[test]
    fn rejects_eof_and_oversized_response_before_reading_body() {
        for reply in [vec![3, 0], vec![3, 0, 255, 255]] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut peer, _) = listener.accept().unwrap();
                peer.set_read_timeout(Some(TIMEOUT)).unwrap();
                let mut request = [0; 19];
                peer.read_exact(&mut request).unwrap();
                peer.write_all(&reply).unwrap();
            });
            assert!(exchange(&mut TcpStream::connect(address).unwrap()).is_err());
            server.join().unwrap();
        }
    }

    #[test]
    fn read_deadline_is_enforced() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (_peer, _) = listener.accept().unwrap();
        let error = read_before(&mut client, &mut [0], Instant::now()).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    }

    #[test]
    fn invalid_cli_arguments_fail_before_network_access() {
        for args in [
            vec!["connect"],
            vec!["probe"],
            vec!["probe", "localhost", "0"],
            vec!["probe", "localhost", "65536"],
            vec!["probe", "--password"],
        ] {
            assert!(run(args.into_iter().map(str::to_owned).collect()).is_err());
        }
    }
}
