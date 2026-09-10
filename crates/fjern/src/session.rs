//! Basic settings and mandatory channel setup on an authenticated connection.
use crate::tls;
use linrdp_proto::{data, mcs, negotiation::SecurityProtocol};
use std::net::TcpStream;

type Error = Box<dyn std::error::Error>;

trait Transport {
    fn send(&mut self, payload: &[u8]) -> Result<(), Error>;
    fn receive(&mut self) -> Result<Vec<u8>, Error>;
}

struct TlsTransport<'a> {
    connection: &'a mut rustls::ClientConnection,
    stream: &'a mut TcpStream,
}
impl Transport for TlsTransport<'_> {
    fn send(&mut self, payload: &[u8]) -> Result<(), Error> {
        tls::write_plaintext(self.connection, self.stream, &data::encode(payload)?)
    }
    fn receive(&mut self) -> Result<Vec<u8>, Error> {
        let packet = tls::read_data(self.connection, self.stream)?;
        Ok(data::decode(&packet)?.to_vec())
    }
}

pub fn run(
    connection: &mut rustls::ClientConnection,
    stream: &mut TcpStream,
    protocol: SecurityProtocol,
) -> Result<(), Error> {
    let (server, user) = setup(
        &mut TlsTransport { connection, stream },
        protocol,
        mcs::Settings::default(),
    )?;
    println!(
        "MCS/GCC settings accepted: server version {:#010x}; user channel {user}, I/O channel {} joined.",
        server.version, server.io_channel
    );
    println!(
        "Session probe only: requested 1024x768, 16-bit color; no desktop activation, graphics or input performed."
    );
    Ok(())
}

pub fn connect_channels(
    connection: &mut rustls::ClientConnection,
    stream: &mut TcpStream,
    protocol: SecurityProtocol,
    settings: mcs::Settings,
) -> Result<(mcs::ServerSettings, u16), Error> {
    setup(&mut TlsTransport { connection, stream }, protocol, settings)
}

fn setup(
    transport: &mut impl Transport,
    protocol: SecurityProtocol,
    settings: mcs::Settings,
) -> Result<(mcs::ServerSettings, u16), Error> {
    transport.send(&mcs::connect_initial(settings, protocol)?)?;
    let server = mcs::connect_response(&transport.receive()?, 0x0b)?;
    if usize::from(settings.clipboard) + usize::from(settings.dynamic_resolution || settings.h264)
        != server.static_channels.iter().flatten().count()
    {
        return Err("server static channel count mismatch".into());
    }
    transport.send(mcs::ERECT_DOMAIN)?;
    transport.send(mcs::ATTACH_USER)?;
    let user = mcs::attach_confirm(&transport.receive()?)?;
    if user == server.io_channel {
        return Err("server assigned the same user and I/O channel".into());
    }
    if server.static_channels.contains(&Some(user)) {
        return Err("static channel overlaps user channel".into());
    }
    for channel in [
        Some(user),
        Some(server.io_channel),
        server.static_channels[0],
        server.static_channels[1],
    ]
    .into_iter()
    .flatten()
    {
        transport.send(&mcs::join_request(user, channel)?)?;
        mcs::join_confirm(&transport.receive()?, user, channel)?;
    }
    Ok((server, user))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    // Independently encoded enhanced-security response, no virtual channels.
    fn response() -> Vec<u8> {
        vec![
            0x7f, 0x66, 0x58, 10, 1, 0, 2, 1, 0, 0x30, 24, 2, 1, 34, 2, 1, 3, 2, 1, 0, 2, 1, 1, 2,
            1, 0, 2, 1, 1, 2, 1, 127, 2, 1, 2, 4, 54, 0, 5, 0, 20, 124, 0, 1, 42, 0x14, 0x76, 10,
            1, 1, 0, 1, 0xc0, 0, b'M', b'c', b'D', b'n', 32, 1, 12, 12, 0, 4, 0, 8, 0, 11, 0, 0, 0,
            2, 12, 12, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3, 12, 8, 0, 0xeb, 3, 0, 0,
        ]
    }
    struct Peer {
        replies: VecDeque<Vec<u8>>,
        sent: Vec<Vec<u8>>,
    }
    impl Transport for Peer {
        fn send(&mut self, payload: &[u8]) -> Result<(), Error> {
            self.sent.push(payload.to_vec());
            Ok(())
        }
        fn receive(&mut self) -> Result<Vec<u8>, Error> {
            self.replies
                .pop_front()
                .ok_or_else(|| "unexpected extra read".into())
        }
    }
    fn peer() -> Peer {
        Peer {
            replies: VecDeque::from([
                response(),
                vec![0x2e, 0, 0, 6],
                vec![0x3e, 0, 0, 6, 3, 0xef, 3, 0xef],
                vec![0x3e, 0, 0, 6, 3, 0xeb, 3, 0xeb],
            ]),
            sent: Vec::new(),
        }
    }
    #[test]
    fn joins_only_user_and_io_channels_in_order() {
        let mut peer = peer();
        let (server, user) = setup(
            &mut peer,
            SecurityProtocol::CredSspEarlyAuth,
            mcs::Settings::default(),
        )
        .unwrap();
        assert_eq!((server.io_channel, user), (1003, 1007));
        assert!(peer.replies.is_empty());
        assert_eq!(
            &peer.sent[1..],
            &[
                vec![4, 1, 0, 1, 0],
                vec![0x28],
                vec![0x38, 0, 6, 3, 0xef],
                vec![0x38, 0, 6, 3, 0xeb],
            ]
        );
    }
    #[test]
    fn rejected_or_mismatched_replies_stop_further_transmission() {
        for (reply, offset) in [(0, 5), (1, 1), (2, 5), (3, 7)] {
            let mut peer = peer();
            peer.replies[reply][offset] ^= 1;
            assert!(
                setup(
                    &mut peer,
                    SecurityProtocol::CredSspEarlyAuth,
                    mcs::Settings::default()
                )
                .is_err()
            );
            assert_eq!(peer.sent.len(), [1, 3, 4, 5][reply]);
        }
    }
    #[test]
    fn clipboard_is_joined_only_when_requested_and_returned() {
        let mut response = response();
        let n = response.len();
        response[2] += 4;
        let octets = response
            .windows(4)
            .position(|b| b == [4, 54, 0, 5])
            .unwrap();
        response[octets + 1] += 4;
        let gcc = response
            .windows(3)
            .position(|b| b == [1, 42, 0x14])
            .unwrap();
        response[gcc + 1] += 4;
        let data = response.windows(4).position(|b| b == b"McDn").unwrap();
        response[data + 4] += 4;
        response[n - 6] = 12;
        response[n - 2] = 1;
        response.extend([0xec, 3, 0, 0]);
        let mut p = peer();
        p.replies[0] = response.clone();
        p.replies.push_back(vec![0x3e, 0, 0, 6, 3, 0xec, 3, 0xec]);
        let settings = mcs::Settings {
            clipboard: true,
            ..Default::default()
        };
        let (server, _) = setup(&mut p, SecurityProtocol::CredSspEarlyAuth, settings).unwrap();
        assert_eq!(server.static_channels, [Some(1004), None]);
        assert_eq!(p.sent.last().unwrap(), &[0x38, 0, 6, 3, 0xec]);
        let mut p = peer();
        p.replies[0] = response;
        assert!(
            setup(
                &mut p,
                SecurityProtocol::CredSspEarlyAuth,
                mcs::Settings::default()
            )
            .is_err()
        );
        assert_eq!(p.sent.len(), 1);
        let mut p = peer();
        assert!(setup(&mut p, SecurityProtocol::CredSspEarlyAuth, settings).is_err());
        assert_eq!(p.sent.len(), 1);
    }
    #[test]
    fn dynamic_transport_is_joined_with_or_without_clipboard() {
        for clipboard in [false, true] {
            let mut p = peer();
            let packet = &mut p.replies[0];
            // Both one channel plus padding and two channels add four bytes.
            packet[2] += 4;
            let data = packet.windows(4).position(|b| b == b"McDn").unwrap();
            packet[data + 4] += 4;
            let n = packet.len();
            packet[n - 6] = 12;
            packet[n - 2] = 1 + u8::from(clipboard);
            packet.extend([0xec, 3]);
            packet.extend(if clipboard { [0xed, 3] } else { [0, 0] });
            p.replies.push_back(vec![0x3e, 0, 0, 6, 3, 0xec, 3, 0xec]);
            if clipboard {
                p.replies.push_back(vec![0x3e, 0, 0, 6, 3, 0xed, 3, 0xed]);
            }
            let settings = mcs::Settings {
                clipboard,
                dynamic_resolution: true,
                ..Default::default()
            };
            let (server, _) = setup(&mut p, SecurityProtocol::CredSsp, settings).unwrap();
            assert_eq!(
                server.static_channels[usize::from(clipboard)],
                Some(if clipboard { 1005 } else { 1004 })
            );
            assert!(p.replies.is_empty());
            assert_eq!(p.sent[5], [0x38, 0, 6, 3, 0xec]);
            if clipboard {
                assert_eq!(p.sent[6], [0x38, 0, 6, 3, 0xed]);
            }
        }
    }
}
