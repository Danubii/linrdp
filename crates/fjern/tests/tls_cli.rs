//! Synthetic loopback fixtures, not an RDP server implementation.
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rustls::pki_types::PrivatePkcs8KeyDer;
use rustls::{ServerConfig, ServerConnection};

struct PemFile(PathBuf);

impl PemFile {
    fn new(contents: &str) -> Self {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("fjern-test-{}-{suffix}.pem", std::process::id()));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        file.write_all(contents.as_bytes()).unwrap();
        Self(path)
    }
}

impl Drop for PemFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn tls_command_upgrades_the_negotiated_socket_and_verifies_explicit_trust() {
    for trusted in [true, false] {
        trust_case(false, trusted);
    }
}

#[test]
fn tls_command_verifies_pins_without_san_and_reports_the_trust_mode() {
    for trusted in [true, false] {
        trust_case(true, trusted);
    }
}

fn trust_case(pinned: bool, trusted: bool) {
    let names = if pinned {
        vec![]
    } else {
        vec!["127.0.0.1".into()]
    };
    let cert = rcgen::generate_simple_self_signed(names).unwrap();
    let other = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();
    let pem = PemFile::new(&if trusted {
        cert.cert.pem()
    } else {
        other.cert.pem()
    });
    let pin = ring::digest::digest(
        &ring::digest::SHA256,
        if trusted {
            cert.cert.der().as_ref()
        } else {
            other.cert.der().as_ref()
        },
    )
    .as_ref()
    .iter()
    .map(|byte| format!("{byte:02x}"))
    .collect::<String>();
    let server_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.cert.der().clone()],
            PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der()).into(),
        )
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    // Bound accept so an early CLI failure cannot hang the test suite.
    listener.set_nonblocking(true).unwrap();
    let peer = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(error) => panic!("fixture accept failed: {error}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = [0; 19];
        stream.read_exact(&mut request).unwrap();
        assert_eq!(request, linrdp_proto::negotiation::PROBE_REQUEST);
        stream
            .write_all(&[3, 0, 0, 19, 14, 0xd0, 0, 0, 0, 0, 0, 2, 0, 8, 0, 2, 0, 0, 0])
            .unwrap();
        let mut tls = ServerConnection::new(Arc::new(server_config)).unwrap();
        while tls.is_handshaking() {
            if tls.complete_io(&mut stream).is_err() {
                break;
            }
        }
    });
    let mut command = Command::new(env!("CARGO_BIN_EXE_fjern"));
    command.args(["tls", "127.0.0.1", &port.to_string()]);
    if pinned {
        command.args(["--cert-sha256", &pin]);
    } else {
        command.arg("--ca").arg(&pem.0);
    }
    let output = command.output().unwrap();
    peer.join().unwrap();
    assert_eq!(
        output.status.success(),
        trusted,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.contains("TLS verified"), trusted);
    if trusted {
        assert!(stdout.contains("no NLA/login performed"));
        assert_eq!(stdout.contains("Explicit certificate pin"), pinned);
        assert_eq!(stdout.contains("hostname/IP verified"), !pinned);
    } else {
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("TLS verification failed"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn rejects_empty_and_malformed_trust_files_before_connecting() {
    for contents in [
        "",
        "not a certificate",
        "-----BEGIN CERTIFICATE-----\ninvalid!\n-----END CERTIFICATE-----\n",
    ] {
        let pem = PemFile::new(contents);
        let output = Command::new(env!("CARGO_BIN_EXE_fjern"))
            .args(["tls", "nonexistent.invalid", "--ca"])
            .arg(&pem.0)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("could not connect"));
        assert!(output.stdout.is_empty());
    }
}
