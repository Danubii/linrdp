//! TPKT/X.224 class 0 data framing for MCS session setup.
//! MS-RDPBCGR 2.2.1.3. This does not parse MCS or fast-path graphics.

use std::fmt;

pub const HEADER_SIZE: usize = 7;
pub const MAX_PAYLOAD_SIZE: usize = u16::MAX as usize - HEADER_SIZE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Error(pub &'static str);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid RDP data frame: {}", self.0)
    }
}

impl std::error::Error for Error {}

/// Determine the complete packet length without consuming a following packet.
/// Incomplete valid headers return None; invalid prefixes fail immediately.
pub fn frame_length(prefix: &[u8]) -> Result<Option<usize>, Error> {
    if prefix.first().is_some_and(|value| *value != 3) {
        return Err(Error("expected TPKT version 3"));
    }
    if prefix.get(1).is_some_and(|value| *value != 0) {
        return Err(Error("nonzero reserved byte"));
    }
    if prefix.len() < 4 {
        return Ok(None);
    }
    let length = usize::from(u16::from_be_bytes([prefix[2], prefix[3]]));
    if length < HEADER_SIZE {
        return Err(Error("packet shorter than data header"));
    }
    Ok(Some(length))
}

/// Wrap opaque MCS bytes in one unsegmented class 0 Data TPDU.
pub fn encode(payload: &[u8]) -> Result<Vec<u8>, Error> {
    if payload.len() > MAX_PAYLOAD_SIZE {
        return Err(Error("payload exceeds TPKT length limit"));
    }
    let length = (payload.len() + HEADER_SIZE) as u16;
    let mut packet = Vec::with_capacity(usize::from(length));
    packet.extend_from_slice(&[3, 0]);
    packet.extend_from_slice(&length.to_be_bytes());
    packet.extend_from_slice(&[2, 0xf0, 0x80]);
    packet.extend_from_slice(payload);
    Ok(packet)
}

/// Decode exactly one packet, borrowing its payload. MCS validation is separate.
pub fn decode(packet: &[u8]) -> Result<&[u8], Error> {
    let length = frame_length(packet)?.ok_or(Error("truncated TPKT header"))?;
    if packet.len() != length {
        return Err(Error("truncated packet or trailing data"));
    }
    if packet[4..7] != [2, 0xf0, 0x80] {
        return Err(Error("expected unsegmented class 0 Data TPDU"));
    }
    Ok(&packet[HEADER_SIZE..])
}

#[cfg(test)]
mod tests {
    use super::*;

    // Header assembled independently; MCS Erect Domain Request payload.
    const PACKET: &[u8] = &[3, 0, 0, 12, 2, 0xf0, 0x80, 4, 1, 0, 1, 0];

    #[test]
    fn matches_wire_vector_and_handles_fragmented_headers() {
        assert_eq!(encode(&[4, 1, 0, 1, 0]).unwrap(), PACKET);
        assert_eq!(decode(PACKET).unwrap(), &[4, 1, 0, 1, 0]);
        for end in 0..4 {
            assert_eq!(frame_length(&PACKET[..end]).unwrap(), None);
        }
        assert_eq!(frame_length(&PACKET[..4]).unwrap(), Some(12));
        let joined = [PACKET, PACKET].concat();
        assert_eq!(frame_length(&joined).unwrap(), Some(12));
        assert!(decode(&joined).is_err());
    }

    #[test]
    fn rejects_truncation_and_invalid_headers() {
        for end in 0..PACKET.len() {
            assert!(decode(&PACKET[..end]).is_err());
        }
        for index in [0, 1, 4, 5, 6] {
            let mut packet = PACKET.to_vec();
            packet[index] ^= 1;
            assert!(decode(&packet).is_err());
        }
        for length in 0..HEADER_SIZE as u16 {
            let [high, low] = length.to_be_bytes();
            assert!(frame_length(&[3, 0, high, low]).is_err());
        }
    }

    #[test]
    fn enforces_packet_size_bounds() {
        let payload = vec![0; MAX_PAYLOAD_SIZE];
        let packet = encode(&payload).unwrap();
        assert_eq!(packet.len(), usize::from(u16::MAX));
        assert_eq!(decode(&packet).unwrap(), payload);
        assert!(encode(&vec![0; MAX_PAYLOAD_SIZE + 1]).is_err());
        assert_eq!(decode(&encode(&[]).unwrap()).unwrap(), &[]);
    }
}
