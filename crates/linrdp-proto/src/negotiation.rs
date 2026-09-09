//! Initial TPKT/X.224 negotiation, MS-RDPBCGR sections 2.2.1.1–2.2.1.2.
//! <https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/>
//!
//! This module describes negotiation only; it does not implement TLS or CredSSP.

use std::fmt;

/// Diagnostic offer: TLS, CredSSP and CredSSP with early authorization.
/// No credentials follow this request in the current client.
pub const PROBE_REQUEST: [u8; 19] = [
    3, 0, 0, 19, // TPKT: version, reserved, big-endian total length
    14, 0xe0, 0, 0, 0, 0, 0, // X.224 connection request, class 0
    1, 0, 8, 0, // RDP_NEG_REQ: type, flags, little-endian length
    0x0b, 0, 0, 0, // TLS | HYBRID | HYBRID_EX
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityProtocol {
    Tls,
    CredSsp,
    CredSspEarlyAuth,
}

impl fmt::Display for SecurityProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Tls => "TLS",
            Self::CredSsp => "CredSSP (NLA)",
            Self::CredSspEarlyAuth => "CredSSP (NLA) with early authorization",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Response {
    pub protocol: SecurityProtocol,
    /// Capability flags are retained for subsequent session setup.
    pub flags: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    VncServer,
    Malformed(&'static str),
    LegacySecurity,
    UnofferedProtocol(u32),
    ServerFailure(u32),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::VncServer => f.write_str("the server speaks VNC (RFB), not RDP. Select VNC in the connection screen or use linrdp vnc <host> [port]"),
            Self::Malformed(reason) => write!(f, "invalid RDP negotiation: {reason}"),
            Self::LegacySecurity => {
                f.write_str("server selected legacy RDP security; refusing downgrade")
            }
            Self::UnofferedProtocol(value) => write!(
                f,
                "server selected an unoffered security protocol: {value:#x}"
            ),
            Self::ServerFailure(code) => {
                let reason = match code {
                    1 => "server requires TLS or does not support the requested authentication",
                    2 => "server only allows legacy RDP security",
                    3 => "server has no usable authentication certificate",
                    4 => "inconsistent security negotiation flags",
                    5 => "server requires CredSSP (NLA)",
                    6 => "server requires TLS with certificate-based client authentication",
                    7 => "server requires Entra ID authentication",
                    _ => "unknown server failure",
                };
                write!(f, "RDP negotiation failed ({code:#x}): {reason}")
            }
        }
    }
}

impl std::error::Error for Error {}

/// Validate a connection-confirm header before allocating or reading its body.
/// This phase accepts only a bare 11-byte confirm or a 19-byte negotiation PDU.
pub fn confirm_length(header: [u8; 4]) -> Result<usize, Error> {
    if header == *b"RFB " {
        return Err(Error::VncServer);
    }
    if header[0] != 3 || header[1] != 0 {
        return Err(Error::Malformed("invalid TPKT version or reserved byte"));
    }
    let length = usize::from(u16::from_be_bytes([header[2], header[3]]));
    if !matches!(length, 11 | 19) {
        return Err(Error::Malformed("unexpected connection-confirm length"));
    }
    Ok(length)
}

/// Decode exactly one response to `PROBE_REQUEST`, rejecting security downgrades.
pub fn decode_confirm(packet: &[u8]) -> Result<Response, Error> {
    let header = packet
        .get(..4)
        .ok_or(Error::Malformed("truncated TPKT header"))?;
    let length = confirm_length([header[0], header[1], header[2], header[3]])?;
    if packet.len() != length {
        return Err(Error::Malformed("TPKT length does not match packet"));
    }
    if usize::from(packet[4]) + 5 != length || packet[5] != 0xd0 || packet[10] != 0 {
        return Err(Error::Malformed(
            "expected an X.224 class 0 connection confirm",
        ));
    }
    // The destination reference must match the request's zero source reference.
    if packet[6..8] != [0, 0] {
        return Err(Error::Malformed("unexpected X.224 destination reference"));
    }
    if length == 11 {
        return Err(Error::LegacySecurity);
    }
    if packet[13..15] != [8, 0] {
        return Err(Error::Malformed("invalid negotiation structure length"));
    }
    let value = u32::from_le_bytes([packet[15], packet[16], packet[17], packet[18]]);
    match packet[11] {
        2 => {
            let protocol = match value {
                0 => return Err(Error::LegacySecurity),
                1 => SecurityProtocol::Tls,
                2 => SecurityProtocol::CredSsp,
                8 => SecurityProtocol::CredSspEarlyAuth,
                _ => return Err(Error::UnofferedProtocol(value)),
            };
            Ok(Response {
                protocol,
                flags: packet[12],
            })
        }
        3 if packet[12] == 0 => Err(Error::ServerFailure(value)),
        _ => Err(Error::Malformed(
            "invalid negotiation type or failure flags",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Synthetic MS-RDPBCGR connection confirm selecting HYBRID (NLA).
    const CONFIRM: [u8; 19] = [
        3, 0, 0, 19, 14, 0xd0, 0, 0, 0x12, 0x34, 0, 2, 1, 8, 0, 2, 0, 0, 0,
    ];

    #[test]
    fn accepts_each_offered_protocol_and_retains_flags() {
        for (value, protocol) in [
            (1, SecurityProtocol::Tls),
            (2, SecurityProtocol::CredSsp),
            (8, SecurityProtocol::CredSspEarlyAuth),
        ] {
            let mut packet = CONFIRM;
            packet[15] = value;
            assert_eq!(decode_confirm(&packet), Ok(Response { protocol, flags: 1 }));
        }
    }

    #[test]
    fn rejects_every_truncation_and_trailing_data() {
        for length in 0..CONFIRM.len() {
            assert!(decode_confirm(&CONFIRM[..length]).is_err());
        }
        let mut packet = CONFIRM.to_vec();
        packet.push(0);
        assert!(decode_confirm(&packet).is_err());
    }

    #[test]
    fn validates_framing_and_structure() {
        for (offset, value) in [
            (0, 2),
            (1, 1),
            (3, 18),
            (4, 13),
            (5, 0xe0),
            (6, 1),
            (10, 1),
            (11, 1),
            (13, 7),
            (14, 1),
        ] {
            let mut packet = CONFIRM;
            packet[offset] = value;
            assert!(
                matches!(decode_confirm(&packet), Err(Error::Malformed(_))),
                "offset {offset}"
            );
        }
    }

    #[test]
    fn refuses_legacy_and_unoffered_security() {
        let legacy = [3, 0, 0, 11, 6, 0xd0, 0, 0, 0, 0, 0];
        assert_eq!(decode_confirm(&legacy), Err(Error::LegacySecurity));
        for value in [0u32, 3, 4, 16, u32::MAX] {
            let mut packet = CONFIRM;
            packet[15..19].copy_from_slice(&value.to_le_bytes());
            let error = if value == 0 {
                Error::LegacySecurity
            } else {
                Error::UnofferedProtocol(value)
            };
            assert_eq!(decode_confirm(&packet), Err(error));
        }
    }

    #[test]
    fn preserves_known_and_unknown_server_failure_codes() {
        for code in [1u32, 2, 3, 4, 5, 6, 7, 0x12345678] {
            let mut packet = CONFIRM;
            packet[11] = 3;
            packet[12] = 0;
            packet[15..19].copy_from_slice(&code.to_le_bytes());
            assert_eq!(decode_confirm(&packet), Err(Error::ServerFailure(code)));
            packet[12] = 1;
            assert!(matches!(decode_confirm(&packet), Err(Error::Malformed(_))));
        }
    }

    #[test]
    fn bounds_all_possible_tpkt_lengths() {
        for length in 0..=u16::MAX {
            let [hi, lo] = length.to_be_bytes();
            assert_eq!(
                confirm_length([3, 0, hi, lo]).is_ok(),
                matches!(length, 11 | 19)
            );
        }
    }
}

#[cfg(test)]
mod service_detection_tests {
    use super::*;
    #[test]
    fn vnc_banner_has_an_actionable_error() {
        assert_eq!(confirm_length(*b"RFB "), Err(Error::VncServer));
        assert!(Error::VncServer.to_string().contains("linrdp vnc"));
        assert!(matches!(confirm_length(*b"HTTP"), Err(Error::Malformed(_))));
    }
}
