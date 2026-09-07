//! Bounded static virtual channels over enhanced-security MCS.
use crate::desktop::{Error, Result};
fn bad(s: &str) -> Error {
    Error(s.into())
}
pub const MAX_MESSAGE: usize = 4 * 1024 * 1024;
pub fn indication(p: &[u8]) -> Result<(u16, &[u8])> {
    if p.len() < 7 || p[0] != 0x68 || p[5] & 0x3f != 0x30 {
        return Err(bad("invalid channel indication"));
    }
    u16::from_be_bytes([p[1], p[2]])
        .checked_add(1001)
        .ok_or_else(|| bad("server user ID overflow"))?;
    let (n, h) = if p[6] & 128 == 0 {
        (usize::from(p[6]), 7)
    } else {
        if p.len() < 8 || p[6] & 64 != 0 {
            return Err(bad("unsupported MCS fragment"));
        }
        ((usize::from(p[6] & 63) << 8) | usize::from(p[7]), 8)
    };
    if p.len() != h + n {
        return Err(bad("channel indication length mismatch"));
    }
    Ok((u16::from_be_bytes([p[3], p[4]]), &p[h..]))
}
#[derive(Default)]
pub struct Channel {
    pending: Vec<u8>,
    total: Option<usize>,
}
impl Channel {
    pub fn is_partial(&self) -> bool {
        self.total.is_some()
    }
    pub fn receive(&mut self, p: &[u8]) -> Result<Option<Vec<u8>>> {
        if p.len() < 8 {
            return Err(bad("short virtual channel header"));
        }
        let size = u32::from_le_bytes(p[..4].try_into().unwrap()) as usize;
        let flags = u32::from_le_bytes(p[4..8].try_into().unwrap());
        if size > MAX_MESSAGE || flags & !0x13 != 0 {
            return Err(bad("unsupported virtual channel flags or size"));
        }
        if flags & 1 != 0 {
            if self.total.is_some() {
                return Err(bad("overlapping channel fragments"));
            }
            self.total = Some(size);
        }
        if self.total != Some(size) || self.pending.len() + p.len() - 8 > size {
            return Err(bad("invalid virtual channel fragment"));
        }
        self.pending.extend_from_slice(&p[8..]);
        if flags & 2 != 0 {
            if self.pending.len() != size {
                return Err(bad("incomplete virtual channel message"));
            }
            self.total = None;
            return Ok(Some(std::mem::take(&mut self.pending)));
        }
        if self.pending.len() >= size {
            return Err(bad("missing last virtual channel fragment"));
        }
        Ok(None)
    }
}
pub fn send(user: u16, channel: u16, message: &[u8]) -> Result<Vec<Vec<u8>>> {
    send_with_protocol(user, channel, message, true)
}
/// DRDYNVC expects its command header first at the server application endpoint.
/// Do not request forwarding of the enclosing static-channel header.
pub fn send_dvc(user: u16, channel: u16, message: &[u8]) -> Result<Vec<Vec<u8>>> {
    if message.len() > 1600 {
        return Err(bad("DVC message exceeds transport limit"));
    }
    send_with_protocol(user, channel, message, false)
}
fn send_with_protocol(
    user: u16,
    channel: u16,
    message: &[u8],
    show_protocol: bool,
) -> Result<Vec<Vec<u8>>> {
    if user < 1001 || channel < 1001 || message.is_empty() || message.len() > MAX_MESSAGE {
        return Err(bad("invalid outgoing channel message"));
    }
    let mut packets = Vec::new();
    let chunks = message.chunks(1600);
    let count = chunks.len();
    for (i, chunk) in chunks.enumerate() {
        let mut p = vec![0x64];
        p.extend((user - 1001).to_be_bytes());
        p.extend(channel.to_be_bytes());
        p.push(0x70);
        let len = chunk.len() + 8;
        if len < 128 {
            p.push(len as u8);
        } else {
            p.extend(((len as u16) | 0x8000).to_be_bytes());
        }
        p.extend((message.len() as u32).to_le_bytes());
        p.extend(
            ((if show_protocol { 0x10 } else { 0 })
                | (if i == 0 { 1u32 } else { 0 })
                | (if i + 1 == count { 2 } else { 0 }))
            .to_le_bytes(),
        );
        p.extend(chunk);
        packets.push(p);
    }
    Ok(packets)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragments_reassemble_and_reject_interleaving() {
        let bytes = vec![42; 5000];
        let packets = send(1004, 1005, &bytes).unwrap();
        let mut c = Channel::default();
        let mut result = None;
        for mut p in packets {
            p[0] = 0x68;
            result = c.receive(indication(&p).unwrap().1).unwrap();
        }
        assert_eq!(result, Some(bytes));
        let first = [10, 0, 0, 0, 1, 0, 0, 0, 1];
        assert!(c.receive(&first).unwrap().is_none());
        assert!(c.receive(&first).is_err());
    }
    #[test]
    fn rejects_bad_lengths_flags_and_limits() {
        let mut c = Channel::default();
        assert!(c.receive(&[0; 7]).is_err());
        assert!(c.receive(&[1, 0, 0, 0, 3, 0, 0, 0]).is_err());
        assert!(
            Channel::default()
                .receive(&[1, 0, 0, 0, 0, 0, 0, 0, 0])
                .is_err()
        );
        assert!(send(1004, 1005, &vec![0; MAX_MESSAGE + 1]).is_err());
    }
    #[test]
    fn server_virtual_channel_priority_can_differ_from_graphics() {
        for priority in [0x30, 0x70, 0xb0, 0xf0] {
            let packet = [0x68, 0, 1, 3, 0xec, priority, 1, 42];
            assert_eq!(indication(&packet).unwrap(), (1004, &[42][..]));
        }
        assert!(indication(&[0x68, 0, 1, 3, 0xec, 0x40, 1, 42]).is_err());
        assert!(indication(&[0x68, 255, 255, 3, 0xec, 0x70, 1, 42]).is_err());
    }
}
