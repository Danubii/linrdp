//! Explicit per-invocation certificate trust for RDP hosts without SANs.
//! A pin replaces CA/name validation, never TLS handshake signature validation.

use std::{str::FromStr, sync::Arc};

use ring::digest::{SHA256, digest};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, ClientConfig, DigitallySignedStruct, Error, SignatureScheme};
use x509_cert::der::Decode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fingerprint([u8; 32]);

impl FromStr for Fingerprint {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        const INVALID: &str = "expected a SHA-256 certificate fingerprint: 64 hex digits or 32 colon/hyphen-separated bytes";
        let compact = if value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()) {
            value.to_owned()
        } else if value.len() == 95 {
            let separator = value.as_bytes()[2];
            if !matches!(separator, b':' | b'-') {
                return Err(INVALID);
            }
            for (index, byte) in value.bytes().enumerate() {
                if (index % 3 == 2 && byte != separator)
                    || (index % 3 != 2 && !byte.is_ascii_hexdigit())
                {
                    return Err(INVALID);
                }
            }
            value
                .bytes()
                .filter(|b| *b != separator)
                .map(char::from)
                .collect()
        } else {
            return Err(INVALID);
        };
        let mut bytes = [0; 32];
        for (index, output) in bytes.iter_mut().enumerate() {
            *output =
                u8::from_str_radix(&compact[index * 2..index * 2 + 2], 16).map_err(|_| INVALID)?;
        }
        Ok(Self(bytes))
    }
}

pub fn config(name: ServerName<'static>, fingerprint: Fingerprint) -> Arc<ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(PinnedCertificate {
        name,
        fingerprint,
        provider: provider.clone(),
    });
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("ring supports TLS 1.2/1.3")
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    config.resumption = rustls::client::Resumption::disabled();
    Arc::new(config)
}

#[derive(Debug)]
struct PinnedCertificate {
    name: ServerName<'static>,
    fingerprint: Fingerprint,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for PinnedCertificate {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        _ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        if server_name != &self.name {
            return Err(Error::General(
                "certificate pin belongs to another destination".into(),
            ));
        }
        if digest(&SHA256, end_entity.as_ref()).as_ref() != self.fingerprint.0 {
            return Err(Error::General(
                "server certificate SHA-256 fingerprint does not match the explicit pin".into(),
            ));
        }
        let certificate = x509_cert::Certificate::from_der(end_entity.as_ref())
            .map_err(|_| Error::InvalidCertificate(CertificateError::BadEncoding))?;
        let validity = certificate.tbs_certificate.validity;
        if now.as_secs() < validity.not_before.to_unix_duration().as_secs() {
            return Err(Error::InvalidCertificate(CertificateError::NotValidYet));
        }
        if now.as_secs() > validity.not_after.to_unix_duration().as_secs() {
            return Err(Error::InvalidCertificate(CertificateError::Expired));
        }
        // Trust is the exact certificate selected by the user. Possession of
        // its private key is proven by the TLS signature callbacks below.
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
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
    ) -> Result<HandshakeSignatureValid, Error> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::pki_types::PrivatePkcs8KeyDer;
    use rustls::{ServerConfig, ServerConnection};
    use std::net::{TcpListener, TcpStream};
    use std::time::{Duration, Instant};

    #[derive(Debug)]
    struct FixtureCertificate(Arc<rustls::sign::CertifiedKey>);
    impl rustls::server::ResolvesServerCert for FixtureCertificate {
        fn resolve(
            &self,
            _: rustls::server::ClientHello<'_>,
        ) -> Option<Arc<rustls::sign::CertifiedKey>> {
            Some(self.0.clone())
        }
    }

    fn attempt(
        tls12: bool,
        mismatched_pin: bool,
        date: i32,
        wrong_key: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // No SAN, matching the Windows certificate compatibility case.
        let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        if date < 0 {
            params.not_before = rcgen::date_time_ymd(2000, 1, 1);
            params.not_after = rcgen::date_time_ymd(2001, 1, 1);
        } else if date > 0 {
            params.not_before = rcgen::date_time_ymd(2090, 1, 1);
            params.not_after = rcgen::date_time_ymd(2091, 1, 1);
        }
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = params.self_signed(&key).unwrap();
        let mut fingerprint = Fingerprint(
            digest(&SHA256, cert.der().as_ref())
                .as_ref()
                .try_into()
                .unwrap(),
        );
        if mismatched_pin {
            fingerprint.0[0] ^= 1;
        }
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
        // A custom resolver deliberately permits mismatched key/cert fixtures,
        // so the client must actually verify the handshake signature.
        let resolver = FixtureCertificate(Arc::new(rustls::sign::CertifiedKey::new(
            vec![cert.der().clone()],
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
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut conn = ServerConnection::new(Arc::new(server_config)).unwrap();
            while conn.is_handshaking() {
                if conn.complete_io(&mut stream).is_err() {
                    break;
                }
            }
        });
        let name = ServerName::try_from("test-host.example").unwrap();
        let result = super::super::handshake(
            &mut stream,
            name.clone(),
            config(name, fingerprint),
            Instant::now() + Duration::from_secs(2),
        );
        drop(stream);
        server.join().unwrap();
        result.map(|_| ())
    }

    #[test]
    fn pins_certificates_without_san_on_tls12_and_tls13() {
        for tls12 in [true, false] {
            attempt(tls12, false, 0, false).unwrap();
        }
    }

    #[test]
    fn rejects_a_changed_certificate() {
        assert!(
            attempt(false, true, 0, false)
                .unwrap_err()
                .to_string()
                .contains("fingerprint does not match")
        );
    }

    #[test]
    fn rejects_expired_and_future_certificates_even_when_pinned() {
        for date in [-1, 1] {
            let error = attempt(false, false, date, false).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains(if date < 0 { "Expired" } else { "NotValidYet" }),
                "{error}"
            );
        }
    }

    #[test]
    fn rejects_wrong_private_key_on_both_tls_versions() {
        for tls12 in [true, false] {
            let error = attempt(tls12, false, 0, true).unwrap_err();
            assert!(error.to_string().contains("BadSignature"), "{error}");
        }
    }

    #[test]
    fn parses_only_complete_consistent_sha256_values() {
        let plain = "aB".repeat(32);
        let colon = vec!["AB"; 32].join(":");
        let hyphen = vec!["ab"; 32].join("-");
        for value in [plain, colon, hyphen] {
            assert_eq!(
                value.parse::<Fingerprint>().unwrap(),
                Fingerprint([0xab; 32])
            );
        }
        for value in [
            "".into(),
            "00".repeat(31),
            "00".repeat(33),
            "gg".repeat(32),
            format!(" {}", "00".repeat(32)),
            vec!["00"; 32].join(":").replacen(':', "-", 1),
        ] {
            assert!(value.parse::<Fingerprint>().is_err());
        }
    }

    #[test]
    fn verifier_cannot_be_reused_for_another_destination() {
        let verifier = PinnedCertificate {
            name: ServerName::try_from("one.example").unwrap(),
            fingerprint: Fingerprint([0; 32]),
            provider: Arc::new(rustls::crypto::ring::default_provider()),
        };
        assert!(
            verifier
                .verify_server_cert(
                    &CertificateDer::from(vec![]),
                    &[],
                    &ServerName::try_from("two.example").unwrap(),
                    &[],
                    UnixTime::now()
                )
                .unwrap_err()
                .to_string()
                .contains("another destination")
        );
    }
}
