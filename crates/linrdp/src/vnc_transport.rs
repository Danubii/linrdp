//! Bounded RFB authentication and VeNCrypt 0.2 transport negotiation.
use openssl::ssl::{SslConnector, SslMethod, SslVerifyMode, SslVersion};
use std::{
    error::Error,
    io::{self, IsTerminal, Write},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};
use vnc::{PixelFormat, VncClient, VncEncoding};
use zeroize::Zeroizing;
type Result<T> = std::result::Result<T, Box<dyn Error>>;
trait Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + Sync> Transport for T {}
type Stream = Box<dyn Transport>;
const DEADLINE: Duration = Duration::from_secs(15);
fn bad(s: impl Into<String>) -> Box<dyn Error> {
    io::Error::new(io::ErrorKind::InvalidData, s.into()).into()
}
async fn read<const N: usize>(stream: &mut Stream) -> Result<[u8; N]> {
    let mut bytes = [0; N];
    timeout(DEADLINE, stream.read_exact(&mut bytes)).await??;
    Ok(bytes)
}
async fn write(stream: &mut Stream, data: &[u8]) -> Result<()> {
    timeout(DEADLINE, stream.write_all(data)).await??;
    Ok(())
}
async fn reason(stream: &mut Stream) -> Result<String> {
    let len = u32::from_be_bytes(read(stream).await?) as usize;
    if len > 4096 {
        return Err(bad("VNC error message exceeds limit"));
    }
    let mut bytes = vec![0; len];
    timeout(DEADLINE, stream.read_exact(&mut bytes)).await??;
    Ok(String::from_utf8_lossy(&bytes)
        .chars()
        .filter(|c| !c.is_control())
        .collect())
}
async fn security_result(stream: &mut Stream, version: u8) -> Result<()> {
    if u32::from_be_bytes(read(stream).await?) != 0 {
        let detail = if version >= 8 {
            reason(stream).await?
        } else {
            "authentication failed".into()
        };
        return Err(bad(format!("VNC authentication: {detail}")));
    }
    Ok(())
}
fn password() -> Result<Zeroizing<String>> {
    Ok(Zeroizing::new(rpassword::prompt_password(
        "VNC password: ",
    )?))
}
fn username(user: Option<&str>) -> Result<String> {
    if let Some(user) = user {
        return Ok(user.into());
    }
    eprint!("VNC username: ");
    io::stderr().flush()?;
    let mut name = String::new();
    io::stdin().read_line(&mut name)?;
    let name = name.trim_end_matches(['\r', '\n']).to_owned();
    if name.is_empty() || name.len() > 1024 {
        return Err(bad("VNC username must contain 1–1024 bytes"));
    }
    Ok(name)
}
async fn vnc_auth(stream: &mut Stream) -> Result<()> {
    let challenge = read::<16>(stream).await?;
    let secret = password()?;
    let response = Zeroizing::new(vnc::client::password_response(&challenge, &secret));
    write(stream, &response).await
}
async fn plain_auth(stream: &mut Stream, user: Option<&str>) -> Result<()> {
    let user = username(user)?;
    let secret = password()?;
    if user.len() > 1024 || secret.len() > 4096 {
        return Err(bad("VNC credential length exceeds limit"));
    }
    write(stream, &(user.len() as u32).to_be_bytes()).await?;
    write(stream, &(secret.len() as u32).to_be_bytes()).await?;
    write(stream, user.as_bytes()).await?;
    write(stream, secret.as_bytes()).await
}
fn subtype(offered: &[u32]) -> Result<u32> {
    [262,261,260,259,258,257].into_iter().find(|n|offered.contains(n)).ok_or_else(||bad(format!("Unsupported VeNCrypt subtypes {offered:?}; enable X509Plain, X509Vnc, X509None, TLSPlain, TLSVnc or TLSNone. Unencrypted Plain is refused.")))
}
fn may_approve_certificate_error(code: i32) -> bool {
    // OpenSSL X509_V_ERR: self-signed chain, local issuer, unverifiable leaf,
    // hostname mismatch and IP mismatch. Validity errors are intentionally absent.
    matches!(code, 18 | 19 | 20 | 21 | 62 | 64)
}
async fn vencrypt(mut stream: Stream, host: &str, user: Option<&str>) -> Result<Stream> {
    let version = read::<2>(&mut stream).await?;
    if version < [0, 2] {
        return Err(bad("VeNCrypt 0.2 or newer is required"));
    }
    write(&mut stream, &[0, 2]).await?;
    if read::<1>(&mut stream).await?[0] != 0 {
        return Err(bad("Server rejected VeNCrypt 0.2"));
    }
    let count = read::<1>(&mut stream).await?[0] as usize;
    if count == 0 {
        return Err(bad("Server offered no VeNCrypt subtypes"));
    }
    let mut bytes = vec![0; count * 4];
    timeout(DEADLINE, stream.read_exact(&mut bytes)).await??;
    let offered: Vec<u32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|v| u32::from_be_bytes(*v))
        .collect();
    let chosen = subtype(&offered)?;
    write(&mut stream, &chosen.to_be_bytes()).await?;
    if read::<1>(&mut stream).await?[0] != 1 {
        return Err(bad("Server could not initialize VeNCrypt TLS"));
    }
    let verified = chosen >= 260;
    let mut builder = SslConnector::builder(SslMethod::tls_client())?;
    builder.set_min_proto_version(Some(SslVersion::TLS1_2))?;
    let deferred = Arc::new(AtomicBool::new(false));
    if verified {
        let deferred = Arc::clone(&deferred);
        builder.set_verify_callback(SslVerifyMode::PEER, move |valid, context| {
            if valid {
                return true;
            }
            let allowed = may_approve_certificate_error(context.error().as_raw());
            if allowed {
                deferred.store(true, Ordering::Relaxed);
            }
            allowed
        });
        builder.set_default_verify_paths()?;
    } else {
        builder.set_verify(SslVerifyMode::NONE);
        builder.set_max_proto_version(Some(SslVersion::TLS1_2))?;
        builder.set_cipher_list("ADH:AECDH:@SECLEVEL=0")?;
        eprintln!(
            "VNC: encrypted anonymous TLS; the server's identity is not verified (VeNCrypt TLS)."
        );
    }
    let connector = builder.build();
    let mut config = connector.configure()?;
    if !verified {
        config.set_verify_hostname(false);
    }
    let ssl = config.into_ssl(host)?;
    let mut tls = tokio_openssl::SslStream::new(ssl, stream)?;
    timeout(DEADLINE, Pin::new(&mut tls).connect()).await??;
    if verified {
        let cert = tls
            .ssl()
            .peer_certificate()
            .ok_or_else(|| bad("VNC TLS server supplied no certificate"))?;
        let now = openssl::asn1::Asn1Time::days_from_now(0)?;
        if cert.not_before().compare(&now)?.is_gt() || cert.not_after().compare(&now)?.is_lt() {
            return Err(bad("VNC server certificate is expired or not yet valid"));
        }
        if deferred.load(Ordering::Relaxed) {
            let fingerprint = cert.digest(openssl::hash::MessageDigest::sha256())?;
            let fingerprint = fingerprint
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(":");
            eprintln!(
                "VNC certificate cannot be verified for {host}.\nSHA-256: {fingerprint}\nValid from: {}\nValid until: {}",
                cert.not_before(),
                cert.not_after()
            );
            if !io::stdin().is_terminal() {
                return Err(bad("Certificate approval requires an interactive terminal"));
            }
            eprint!("Trust this certificate for this connection? [y/N] ");
            io::stderr().flush()?;
            let mut answer = String::new();
            io::stdin().read_line(&mut answer)?;
            if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                return Err(bad("VNC certificate was not approved"));
            }
            // Approval is bound to this already-established TLS connection's
            // certificate; no second connection can substitute another key.
            eprintln!("VNC: encrypted TLS with this certificate approved for this session.");
        } else {
            eprintln!("VNC: certificate-verified TLS (VeNCrypt X509).");
        }
    }
    stream = Box::new(tls);
    match chosen {
        258 | 261 => vnc_auth(&mut stream).await?,
        259 | 262 => plain_auth(&mut stream, user).await?,
        257 | 260 => {}
        _ => unreachable!(),
    }
    Ok(stream)
}
/// Negotiate the strongest implemented security type offered without retrying
/// a weaker type after TLS or authentication failure.
pub async fn connect(host: &str, port: u16, user: Option<&str>) -> Result<VncClient> {
    let tcp = timeout(DEADLINE, TcpStream::connect((host, port))).await??;
    tcp.set_nodelay(true)?;
    let mut stream: Stream = Box::new(tcp);
    let banner = read::<12>(&mut stream).await?;
    let version = match &banner {
        b"RFB 003.003\n" => 3,
        b"RFB 003.007\n" => 7,
        b"RFB 003.008\n" => 8,
        _ => return Err(bad("Unsupported RFB protocol version")),
    };
    write(&mut stream, &banner).await?;
    let selected = if version == 3 {
        u32::from_be_bytes(read(&mut stream).await?)
    } else {
        let count = read::<1>(&mut stream).await?[0] as usize;
        if count == 0 {
            return Err(bad(reason(&mut stream).await?));
        }
        let mut offered = vec![0; count];
        timeout(DEADLINE, stream.read_exact(&mut offered)).await??;
        let selected=[19,2,1].into_iter().find(|n|offered.contains(n)).ok_or_else(||bad(format!("Unsupported VNC security types {offered:?}; enable VeNCrypt, VNC authentication, or None on the server.")))?;
        write(&mut stream, &[selected]).await?;
        u32::from(selected)
    };
    match selected {
        0 => return Err(bad(reason(&mut stream).await?)),
        19 => {
            stream = vencrypt(stream, host, user).await?;
            security_result(&mut stream, version).await?;
        }
        2 => {
            eprintln!("VNC: unencrypted connection with traditional VNC password authentication.");
            vnc_auth(&mut stream).await?;
            security_result(&mut stream, version).await?;
        }
        1 => {
            eprintln!("VNC: unencrypted connection without authentication.");
            if version == 8 {
                security_result(&mut stream, version).await?;
            }
        }
        _ => return Err(bad(format!("Unsupported VNC security type {selected}"))),
    }
    let encodings = vec![
        VncEncoding::Zrle,
        VncEncoding::CopyRect,
        VncEncoding::Raw,
        VncEncoding::QemuExtendedKeyEventPseudo,
        VncEncoding::ExtendedDesktopSizePseudo,
        VncEncoding::DesktopSizePseudo,
        VncEncoding::LastRectPseudo,
    ];
    Ok(timeout(
        DEADLINE,
        VncClient::from_authenticated_stream(stream, true, Some(PixelFormat::bgra()), encodings),
    )
    .await??)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn subtype_prefers_verified_and_refuses_plain() {
        assert_eq!(subtype(&[257, 258, 259, 260, 261, 262]).unwrap(), 262);
        assert_eq!(subtype(&[258, 259]).unwrap(), 259);
        assert!(subtype(&[256]).is_err());
        assert!(subtype(&[]).is_err());
    }
    #[test]
    fn certificate_approval_excludes_validity_errors() {
        assert!(may_approve_certificate_error(18));
        assert!(may_approve_certificate_error(62));
        assert!(!may_approve_certificate_error(9));
        assert!(!may_approve_certificate_error(10));
    }
    #[test]
    fn failure_messages_are_bounded_and_strip_terminal_controls() {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let (client, mut peer) = tokio::io::duplex(64);
            let mut client: Stream = Box::new(client);
            peer.write_all(&4097u32.to_be_bytes()).await.unwrap();
            assert!(
                reason(&mut client)
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("exceeds limit")
            );
            peer.write_all(&4u32.to_be_bytes()).await.unwrap();
            peer.write_all(b"a\x1bb\n").await.unwrap();
            assert_eq!(reason(&mut client).await.unwrap(), "ab");
            peer.write_all(&8u32.to_be_bytes()).await.unwrap();
            drop(peer);
            assert!(reason(&mut client).await.is_err());
        });
    }
    #[test]
    fn legacy_authentication_failure_does_not_read_a_reason() {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let (client, mut peer) = tokio::io::duplex(64);
            let mut client: Stream = Box::new(client);
            peer.write_all(&1u32.to_be_bytes()).await.unwrap();
            assert!(
                security_result(&mut client, 7)
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("authentication failed")
            );
        });
    }
}
