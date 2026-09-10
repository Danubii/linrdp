//! One-shot certificate discovery without granting server identity trust.

use super::{DeadlineTransport, pin::Fingerprint};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{
    CertificateError, ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme,
};
use std::{
    fmt,
    net::TcpStream,
    sync::{Arc, Mutex},
    time::Instant,
};
use x509_cert::der::Decode;

type Error = Box<dyn std::error::Error>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CertificateDetails {
    pub fingerprint: Fingerprint,
    pub subject: String,
    pub issuer: String,
    pub valid_from: String,
    pub valid_until: String,
}

/// Completes one TLS handshake solely to inspect the peer certificate.
///
/// CA and server-name trust are intentionally not checked. Certificate DER,
/// validity, and TLS handshake signatures are checked. The unverified TLS
/// connection and any buffered application data are always discarded.
pub fn discover(
    stream: &mut TcpStream,
    name: ServerName<'static>,
    deadline: Instant,
) -> Result<CertificateDetails, Error> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let captured = Arc::new(Mutex::new(None));
    let verifier = Arc::new(DiscoveryVerifier {
        provider: provider.clone(),
        captured: captured.clone(),
    });
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    config.resumption = rustls::client::Resumption::disabled();
    let mut connection = ClientConnection::new(Arc::new(config), name)?;
    let mut transport = DeadlineTransport { stream, deadline };
    while connection.is_handshaking() {
        connection.complete_io(&mut transport)?;
    }
    drop(connection);
    captured
        .lock()
        .unwrap()
        .take()
        .ok_or_else(|| "TLS peer did not provide a certificate".into())
}

#[derive(Debug)]
struct DiscoveryVerifier {
    provider: Arc<CryptoProvider>,
    captured: Arc<Mutex<Option<CertificateDetails>>>,
}

impl ServerCertVerifier for DiscoveryVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let certificate = x509_cert::Certificate::from_der(end_entity.as_ref())
            .map_err(|_| rustls::Error::InvalidCertificate(CertificateError::BadEncoding))?;
        let validity = certificate.tbs_certificate.validity;
        if now.as_secs() < validity.not_before.to_unix_duration().as_secs() {
            return Err(rustls::Error::InvalidCertificate(
                CertificateError::NotValidYet,
            ));
        }
        if now.as_secs() > validity.not_after.to_unix_duration().as_secs() {
            return Err(rustls::Error::InvalidCertificate(CertificateError::Expired));
        }
        *self.captured.lock().unwrap() = Some(CertificateDetails {
            fingerprint: Fingerprint::for_der(end_entity.as_ref()),
            subject: safe(certificate.tbs_certificate.subject),
            issuer: safe(certificate.tbs_certificate.issuer),
            valid_from: safe(validity.not_before),
            valid_until: safe(validity.not_after),
        });
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn safe(value: impl fmt::Display) -> String {
    let value = value.to_string();
    let mut output = String::with_capacity(value.len().min(256));
    let mut kept = 0;
    for character in value.chars() {
        if character.is_control()
            || matches!(character, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        {
            continue;
        }
        if kept == 256 {
            break;
        }
        output.push(character);
        kept += 1;
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::pki_types::PrivatePkcs8KeyDer;
    use rustls::{ServerConfig, ServerConnection};
    use std::{io::Write, net::TcpListener, time::Duration};

    #[derive(Debug)]
    struct Fixture(Arc<rustls::sign::CertifiedKey>);
    impl rustls::server::ResolvesServerCert for Fixture {
        fn resolve(
            &self,
            _: rustls::server::ClientHello<'_>,
        ) -> Option<Arc<rustls::sign::CertifiedKey>> {
            Some(self.0.clone())
        }
    }

    fn attempt(
        tls12: bool,
        date: i32,
        wrong_key: bool,
        malformed: bool,
    ) -> (Result<CertificateDetails, Error>, Fingerprint) {
        let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "Windows test host");
        if date < 0 {
            params.not_before = rcgen::date_time_ymd(2000, 1, 1);
            params.not_after = rcgen::date_time_ymd(2001, 1, 1);
        } else if date > 0 {
            params.not_before = rcgen::date_time_ymd(2090, 1, 1);
            params.not_after = rcgen::date_time_ymd(2091, 1, 1);
        }
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = params.self_signed(&key).unwrap();
        let leaf = if malformed {
            CertificateDer::from(vec![1, 2, 3])
        } else {
            cert.der().clone()
        };
        let expected = Fingerprint::for_der(leaf.as_ref());
        let signing_key = if wrong_key {
            rcgen::KeyPair::generate().unwrap()
        } else {
            key
        };
        let provider = rustls::crypto::ring::default_provider();
        let signer = provider
            .key_provider
            .load_private_key(PrivatePkcs8KeyDer::from(signing_key.serialize_der()).into())
            .unwrap();
        let resolver = Fixture(Arc::new(rustls::sign::CertifiedKey::new(
            vec![leaf],
            signer,
        )));
        let versions = if tls12 {
            vec![&rustls::version::TLS12]
        } else {
            vec![&rustls::version::TLS13]
        };
        let server_config = ServerConfig::builder_with_protocol_versions(&versions)
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(resolver));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut connection = ServerConnection::new(Arc::new(server_config)).unwrap();
            while connection.is_handshaking() {
                if connection.complete_io(&mut stream).is_err() {
                    return;
                }
            }
            connection
                .writer()
                .write_all(b"application-secret")
                .unwrap();
            let _ = connection.complete_io(&mut stream);
        });
        let result = discover(
            &mut stream,
            ServerName::try_from("untrusted.example").unwrap(),
            Instant::now() + Duration::from_secs(2),
        );
        drop(stream);
        server.join().unwrap();
        (result, expected)
    }

    #[test]
    fn discovers_exact_leaf_on_tls12_and_tls13_without_returning_connection() {
        for tls12 in [true, false] {
            let (result, expected) = attempt(tls12, 0, false, false);
            let details = result.unwrap();
            assert_eq!(details.fingerprint, expected);
            assert!(details.subject.contains("Windows test host"));
            assert!(!details.valid_from.is_empty());
            assert!(!details.valid_until.is_empty());
        }
    }

    #[test]
    fn rejects_wrong_handshake_signature_on_both_tls_versions() {
        for tls12 in [true, false] {
            assert!(
                attempt(tls12, 0, true, false)
                    .0
                    .unwrap_err()
                    .to_string()
                    .contains("BadSignature")
            );
        }
    }

    #[test]
    fn rejects_expired_future_and_malformed_certificates() {
        assert!(
            attempt(false, -1, false, false)
                .0
                .unwrap_err()
                .to_string()
                .contains("Expired")
        );
        assert!(
            attempt(false, 1, false, false)
                .0
                .unwrap_err()
                .to_string()
                .contains("NotValidYet")
        );
        assert!(
            attempt(false, 0, false, true)
                .0
                .unwrap_err()
                .to_string()
                .contains("BadEncoding")
        );
    }

    #[test]
    fn fingerprint_formats_are_public_and_unambiguous() {
        let fingerprint = Fingerprint::for_der(b"certificate");
        let displayed = fingerprint.to_string();
        assert_eq!(displayed.len(), 95);
        assert_eq!(displayed.bytes().filter(|byte| *byte == b':').count(), 31);
    }

    #[test]
    fn display_fields_cannot_inject_controls_or_unicode_directionality() {
        assert_eq!(
            safe("Vært\n\u{202e}navn\u{2067}.example\u{2069}"),
            "Værtnavn.example"
        );
    }
}
