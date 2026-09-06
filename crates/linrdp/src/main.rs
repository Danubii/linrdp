mod credentials;
mod nla;
mod ntlm;
mod session;
mod tls;
mod viewer;

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use linrdp_proto::negotiation::{PROBE_REQUEST, Response, confirm_length, decode_confirm};

const TIMEOUT: Duration = Duration::from_secs(5);
const HELP: &str = "LinRDP — early development

Usage: linrdp probe <host> [port]
       linrdp tls <host> [port] [trust-option]
       linrdp nla-probe <host> [port] [trust-option]
       linrdp login <host> [port] --user <username|DOMAIN\\username> [trust-option]
       linrdp session-probe <host> [port] --user <username> [trust-option]
       linrdp connect <host> [port] --user <username> [trust-option]
       linrdp --help
       linrdp --version

Trust: --ca <pem-file> OR --cert-sha256 <fingerprint>; defaults to system trust.
Use an unbracketed IPv6 address with the port as a separate argument.
probe checks RDP negotiation; tls additionally verifies TLS.
nla-probe requests an NTLM challenge without credentials.
login prompts locally for a hidden password after TLS verification, then
attempts NTLM CredSSP once. session-probe continues with MCS/GCC and channel
setup after login, then disconnects. connect opens an interactive desktop window.";

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
    let options = Options::parse(&args)?;
    let host = &options.host;
    let port = options.port;
    // Validate the trust source and server name before any network access.
    let server_name = rustls::pki_types::ServerName::try_from(host.clone())?;
    let tls_config = if options.tls {
        Some(match options.fingerprint {
            Some(pin) => tls::pin::config(server_name.clone(), pin),
            None => tls::config(options.ca_file.as_deref())?,
        })
    } else {
        None
    };
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
                if let Some(config) = tls_config {
                    let mut connection =
                        tls::handshake(&mut stream, server_name, config, Instant::now() + TIMEOUT)
                            .map_err(|error| format!("TLS verification failed: {error}"))?;
                    println!(
                        "TLS verified for {host}: {:?}, {:?}.",
                        connection
                            .protocol_version()
                            .expect("completed handshake has a version"),
                        connection
                            .negotiated_cipher_suite()
                            .expect("completed handshake has a cipher")
                            .suite()
                    );
                    if options.fingerprint.is_some() {
                        println!(
                            "Explicit certificate pin, validity and TLS signature verified; CA/name validation replaced by the supplied pin."
                        );
                    } else {
                        println!("Certificate chain, validity and hostname/IP verified.");
                    }
                    if options.nla {
                        let identity = nla::run(
                            &mut connection,
                            &mut stream,
                            host,
                            response.protocol,
                            options.user.as_deref(),
                        )?;
                        if options.view {
                            viewer::run(
                                &mut connection,
                                &mut stream,
                                response.protocol,
                                options.user.as_deref().ok_or("connect requires --user")?,
                                identity.ok_or("connect requires credentials")?,
                                host,
                            )?;
                        }
                        if options.session {
                            session::run(&mut connection, &mut stream, response.protocol)?;
                        }
                    } else {
                        println!("TLS diagnostic only: no NLA/login performed.");
                    }
                } else {
                    println!(
                        "Negotiation only: server identity, TLS and login have NOT been verified."
                    );
                }
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

#[derive(Debug, PartialEq, Eq)]
struct Options {
    tls: bool,
    nla: bool,
    session: bool,
    view: bool,
    user: Option<String>,
    host: String,
    port: u16,
    ca_file: Option<std::path::PathBuf>,
    fingerprint: Option<tls::pin::Fingerprint>,
}

impl Options {
    fn parse(args: &[String]) -> Result<Self, Box<dyn std::error::Error>> {
        if args.len() < 2
            || !matches!(
                args[0].as_str(),
                "probe" | "tls" | "nla-probe" | "login" | "session-probe" | "connect"
            )
        {
            return Err(format!("invalid arguments\n\n{HELP}").into());
        }
        let mut options = Self {
            tls: args[0] != "probe",
            nla: matches!(
                args[0].as_str(),
                "nla-probe" | "login" | "session-probe" | "connect"
            ),
            session: args[0] == "session-probe",
            view: args[0] == "connect",
            user: None,
            host: args[1].clone(),
            port: 3389,
            ca_file: None,
            fingerprint: None,
        };
        if options.host.is_empty() || options.host.starts_with('-') {
            return Err("expected a hostname or IP address".into());
        }
        let mut rest = &args[2..];
        if let Some(value) = rest.first().filter(|value| !value.starts_with('-')) {
            options.port = value.parse()?;
            rest = &rest[1..];
        }
        if options.port == 0 {
            return Err("port must be between 1 and 65535".into());
        }
        while !rest.is_empty() {
            if rest.len() < 2 {
                return Err(format!("invalid arguments\n\n{HELP}").into());
            }
            let flag = &rest[0];
            let value = &rest[1];
            if value.is_empty() || value.starts_with('-') {
                return Err("missing option value".into());
            }
            match flag.as_str() {
                "--user"
                    if matches!(args[0].as_str(), "login" | "session-probe" | "connect")
                        && options.user.is_none() =>
                {
                    nla::account(value)?;
                    options.user = Some(value.clone());
                }
                "--ca"
                    if options.tls
                        && options.ca_file.is_none()
                        && options.fingerprint.is_none() =>
                {
                    options.ca_file = Some(value.into())
                }
                "--cert-sha256"
                    if options.tls
                        && options.ca_file.is_none()
                        && options.fingerprint.is_none() =>
                {
                    options.fingerprint = Some(value.parse()?)
                }
                _ => return Err(format!("invalid or duplicate option\n\n{HELP}").into()),
            }
            rest = &rest[2..];
        }
        if matches!(args[0].as_str(), "login" | "session-probe" | "connect")
            && options.user.is_none()
        {
            return Err("login, session-probe and connect require --user".into());
        }
        Ok(options)
    }
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
    fn parses_tls_with_explicit_trust_and_ipv6() {
        let args = ["tls", "::1", "3390", "--ca", "lab.pem"].map(str::to_owned);
        assert_eq!(
            Options::parse(&args).unwrap(),
            Options {
                tls: true,
                nla: false,
                session: false,
                view: false,
                user: None,
                host: "::1".into(),
                port: 3390,
                ca_file: Some("lab.pem".into()),
                fingerprint: None,
            }
        );
    }

    #[test]
    fn invalid_cli_arguments_fail_before_network_access() {
        for args in [
            vec!["login", "localhost"],
            vec!["login", "localhost", "--user", "user@example.com"],
            vec!["login", "localhost", "--user", "one", "--user", "two"],
            vec![
                "login",
                "localhost",
                "--user",
                "one",
                "--password",
                "unused-test-value",
            ],
            vec!["nla-probe", "localhost", "--user", "one"],
            vec!["probe", "localhost", "--cert-sha256", "bad"],
            vec!["tls", "localhost", "--cert-sha256", "bad"],
            vec![
                "tls",
                "localhost",
                "--ca",
                "lab.pem",
                "--cert-sha256",
                "bad",
            ],
            vec!["connect"],
            vec!["session-probe", "localhost"],
            vec![
                "session-probe",
                "localhost",
                "--user",
                "tester",
                "--password",
                "unused",
            ],
            vec!["probe"],
            vec!["probe", "localhost", "0"],
            vec!["probe", "localhost", "65536"],
            vec!["probe", "--password"],
            vec!["probe", "localhost", "--ca", "lab.pem"],
            vec!["tls", "localhost", "--ca"],
            vec!["tls", "localhost", "--insecure"],
            vec!["tls", "localhost", "--ca", "--insecure"],
            vec!["tls", "localhost", "3389", "unexpected"],
        ] {
            assert!(run(args.into_iter().map(str::to_owned).collect()).is_err());
        }
    }

    #[test]
    fn parses_login_and_probe_with_explicit_trust() {
        let args = [
            "login",
            "localhost",
            "3390",
            "--ca",
            "lab.pem",
            "--user",
            "LAB\\tester",
        ]
        .map(str::to_owned);
        let options = Options::parse(&args).unwrap();
        assert!(options.nla && options.tls);
        assert_eq!(options.user.as_deref(), Some("LAB\\tester"));
        assert_eq!(options.port, 3390);
        let options = Options::parse(&["nla-probe".into(), "localhost".into()]).unwrap();
        assert!(options.nla && options.tls && options.user.is_none());
        let options = Options::parse(&[
            "session-probe".into(),
            "localhost".into(),
            "--user".into(),
            "tester".into(),
        ])
        .unwrap();
        assert!(options.session && options.nla && options.tls);
        assert_eq!(options.user.as_deref(), Some("tester"));
    }
}
