//! Verified TLS transport using rustls's standard certificate verifier.
//! https://docs.rs/rustls/0.23/rustls/client/struct.ClientConfig.html

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use rustls::pki_types::{CertificateDer, ServerName, pem::PemObject};
use rustls::{ClientConfig, ClientConnection, RootCertStore};

type Error = Box<dyn std::error::Error>;

/// An explicit PEM file replaces system roots for this connection only.
pub fn config(ca_file: Option<&Path>) -> Result<Arc<ClientConfig>, Error> {
    let mut roots = RootCertStore::empty();
    if let Some(path) = ca_file {
        for certificate in CertificateDer::pem_file_iter(path)? {
            roots.add(certificate?)?;
        }
    } else {
        let result = rustls_native_certs::load_native_certs();
        for error in result.errors {
            eprintln!("linrdp: warning: could not load some system certificates: {error}");
        }
        for certificate in result.certs {
            roots.add(certificate)?;
        }
    }
    if roots.is_empty() {
        return Err(
            "no trusted certificates were loaded; provide a trusted PEM file with --ca".into(),
        );
    }
    Ok(config_with_roots(roots))
}

fn config_with_roots(roots: RootCertStore) -> Arc<ClientConfig> {
    Arc::new(
        ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    )
}

/// The supplied hostname/IP is checked against the certificate SAN, not reverse DNS.
/// The deadline covers all handshake reads and writes, including partial records.
pub fn handshake(
    stream: &mut TcpStream,
    server_name: ServerName<'static>,
    config: Arc<ClientConfig>,
    deadline: Instant,
) -> Result<ClientConnection, Error> {
    let mut connection = ClientConnection::new(config, server_name)?;
    let mut transport = DeadlineTransport { stream, deadline };
    while connection.is_handshaking() {
        connection.complete_io(&mut transport)?;
    }
    Ok(connection)
}

struct DeadlineTransport<'a> {
    stream: &'a mut TcpStream,
    deadline: Instant,
}

impl DeadlineTransport<'_> {
    fn prepare(&self) -> io::Result<()> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "TLS handshake timed out",
            ));
        }
        self.stream.set_read_timeout(Some(remaining))?;
        self.stream.set_write_timeout(Some(remaining))
    }
}

impl Read for DeadlineTransport<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.prepare()?;
        self.stream.read(buffer)
    }
}

impl Write for DeadlineTransport<'_> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.prepare()?;
        self.stream.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.prepare()?;
        self.stream.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::pki_types::PrivatePkcs8KeyDer;
    use rustls::{ServerConfig, ServerConnection};
    use std::net::TcpListener;
    use std::time::Duration;

    fn attempt(
        name: &str,
        trusted: bool,
        expired: bool,
        tls12: bool,
    ) -> Result<ClientConnection, Error> {
        let mut params =
            rcgen::CertificateParams::new(vec!["localhost".into(), "127.0.0.1".into()]).unwrap();
        if expired {
            params.not_before = rcgen::date_time_ymd(2000, 1, 1);
            params.not_after = rcgen::date_time_ymd(2001, 1, 1);
        }
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = params.self_signed(&key).unwrap();
        let mut roots = RootCertStore::empty();
        if trusted {
            roots.add(cert.der().clone()).unwrap();
        }
        let versions = if tls12 {
            vec![&rustls::version::TLS12]
        } else {
            vec![&rustls::version::TLS13]
        };
        let server_config = ServerConfig::builder_with_protocol_versions(&versions)
            .with_no_client_auth()
            .with_single_cert(
                vec![cert.der().clone()],
                PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
            )
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut connection = ServerConnection::new(Arc::new(server_config)).unwrap();
            while connection.is_handshaking() {
                if connection.complete_io(&mut stream).is_err() {
                    break;
                }
            }
        });
        let result = handshake(
            &mut client,
            ServerName::try_from(name.to_owned()).unwrap(),
            config_with_roots(roots),
            Instant::now() + Duration::from_secs(2),
        );
        drop(client);
        server.join().unwrap();
        result
    }

    #[test]
    fn verifies_dns_and_ip_names_with_tls12_and_tls13() {
        for tls12 in [true, false] {
            for name in ["localhost", "127.0.0.1"] {
                let connection = attempt(name, true, false, tls12).unwrap();
                assert_eq!(
                    connection.protocol_version(),
                    Some(if tls12 {
                        rustls::ProtocolVersion::TLSv1_2
                    } else {
                        rustls::ProtocolVersion::TLSv1_3
                    })
                );
            }
        }
    }

    fn certificate_error(error: Error) -> rustls::CertificateError {
        let io_error = error.downcast_ref::<io::Error>().expect("TLS IO error");
        let tls_error = io_error
            .get_ref()
            .unwrap()
            .downcast_ref::<rustls::Error>()
            .unwrap();
        match tls_error {
            rustls::Error::InvalidCertificate(reason) => reason.clone(),
            other => panic!("expected certificate rejection, got {other:?}"),
        }
    }

    #[test]
    fn rejects_untrusted_certificate() {
        assert!(matches!(
            certificate_error(attempt("localhost", false, false, false).unwrap_err()),
            rustls::CertificateError::UnknownIssuer
        ));
    }

    #[test]
    fn rejects_wrong_hostname_even_with_trusted_certificate() {
        assert!(matches!(
            certificate_error(attempt("wrong.example", true, false, false).unwrap_err()),
            rustls::CertificateError::NotValidForName
                | rustls::CertificateError::NotValidForNameContext { .. }
        ));
    }

    #[test]
    fn rejects_expired_certificate() {
        assert!(matches!(
            certificate_error(attempt("localhost", true, true, false).unwrap_err()),
            rustls::CertificateError::Expired | rustls::CertificateError::ExpiredContext { .. }
        ));
    }

    #[test]
    fn times_out_when_peer_never_answers() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (_peer, _) = listener.accept().unwrap();
        let error = handshake(
            &mut client,
            ServerName::try_from("localhost").unwrap(),
            config_with_roots(RootCertStore::empty()),
            Instant::now() + Duration::from_millis(50),
        )
        .unwrap_err();
        let error = error.downcast_ref::<io::Error>().unwrap();
        assert!(matches!(
            error.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
        ));
    }
}
