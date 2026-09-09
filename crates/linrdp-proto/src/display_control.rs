//! MS-RDPEDYC versions 1–2 with display control and optional graphics endpoints.
use crate::desktop::{Error, Result};
const NAME: &[u8] = b"Microsoft::Windows::RDS::DisplayControl\0";
const GRAPHICS_NAME: &[u8] = b"Microsoft::Windows::RDS::Graphics\0";
const MAX_GRAPHICS_MESSAGE: usize = 16 * 1024 * 1024;
#[derive(Debug, PartialEq, Eq)]
pub enum GraphicsEvent {
    Opened,
    Data(Vec<u8>),
    Closed,
}
fn bad(s: &str) -> Error {
    Error(s.into())
}
fn number(b: &mut &[u8], width: u8) -> Result<u32> {
    let n = match width {
        0 => 1,
        1 => 2,
        2 => 4,
        _ => return Err(bad("invalid DVC integer width")),
    };
    if b.len() < n {
        return Err(bad("truncated DVC integer"));
    }
    let mut v = [0; 4];
    v[..n].copy_from_slice(&b[..n]);
    *b = &b[n..];
    Ok(u32::from_le_bytes(v))
}
fn header(kind: u8, id: u32) -> Vec<u8> {
    let (width, n) = if id <= 255 {
        (0, 1)
    } else if id <= 65535 {
        (1, 2)
    } else {
        (2, 4)
    };
    let mut b = vec![kind << 4 | width];
    b.extend_from_slice(&id.to_le_bytes()[..n]);
    b
}
#[derive(Default)]
pub struct DisplayControl {
    negotiated: bool,
    graphics_enabled: bool,
    graphics_id: Option<u32>,
    graphics_fragment: Vec<u8>,
    graphics_total: Option<usize>,
    graphics_events: Vec<GraphicsEvent>,
    id: Option<u32>,
    area: Option<u64>,
    fragment: Vec<u8>,
    total: Option<usize>,
}
impl DisplayControl {
    pub fn with_graphics() -> Self {
        Self {
            graphics_enabled: true,
            ..Self::default()
        }
    }
    pub fn take_graphics(&mut self) -> Vec<GraphicsEvent> {
        std::mem::take(&mut self.graphics_events)
    }
    /// Encode a graphics message into uncompressed DVC PDUs, each at most 1600 bytes.
    pub fn send_graphics(&self, data: &[u8]) -> Result<Vec<Vec<u8>>> {
        let id = self
            .graphics_id
            .ok_or_else(|| bad("graphics channel is not open"))?;
        if data.is_empty() || data.len() > MAX_GRAPHICS_MESSAGE {
            return Err(bad("invalid graphics message size"));
        }
        let continuation = header(3, id);
        if continuation.len() + data.len() <= 1600 {
            let mut p = continuation;
            p.extend_from_slice(data);
            return Ok(vec![p]);
        }
        let mut first = header(2, id);
        let (width, length) = if data.len() <= 65535 { (1, 2) } else { (2, 4) };
        first[0] |= width << 2;
        first.extend_from_slice(&(data.len() as u32).to_le_bytes()[..length]);
        let count = 1600 - first.len();
        first.extend_from_slice(&data[..count]);
        let mut packets = vec![first];
        for chunk in data[count..].chunks(1600 - continuation.len()) {
            let mut p = continuation.clone();
            p.extend_from_slice(chunk);
            packets.push(p);
        }
        Ok(packets)
    }
    fn graphics_data(&mut self, cmd: u8, width: u8, mut b: &[u8]) -> Result<()> {
        if cmd == 2 {
            if self.graphics_total.is_some() {
                return Err(bad("overlapping graphics DVC fragments"));
            }
            let total = number(&mut b, width)? as usize;
            if total == 0 || total > MAX_GRAPHICS_MESSAGE {
                return Err(bad("invalid graphics message size"));
            }
            self.graphics_total = Some(total);
        }
        if let Some(total) = self.graphics_total {
            if b.is_empty() || self.graphics_fragment.len() + b.len() > total {
                return Err(bad("invalid graphics DVC fragment size"));
            }
            self.graphics_fragment.extend_from_slice(b);
            if self.graphics_fragment.len() == total {
                self.graphics_events
                    .push(GraphicsEvent::Data(std::mem::take(
                        &mut self.graphics_fragment,
                    )));
                self.graphics_total = None;
            }
        } else {
            if b.is_empty() {
                return Err(bad("empty graphics DVC message"));
            }
            self.graphics_events.push(GraphicsEvent::Data(b.to_vec()));
        }
        Ok(())
    }
    pub fn ready(&self) -> bool {
        self.id.is_some() && self.area.is_some()
    }
    pub fn partial(&self) -> bool {
        self.total.is_some() || self.graphics_total.is_some()
    }
    /// Decode one reassembled static-channel payload; returns DVC replies.
    pub fn receive(&mut self, p: &[u8]) -> Result<Vec<Vec<u8>>> {
        if p.is_empty() || p.len() > 1600 {
            return Err(bad("invalid DVC PDU size"));
        }
        let cmd = p[0] >> 4;
        let mut b = &p[1..];
        if cmd == 5 {
            if self.negotiated || p[0] & 3 != 0 || b.len() < 3 || b[0] != 0 {
                return Err(bad("invalid DVC capabilities"));
            }
            let version = u16::from_le_bytes([b[1], b[2]]);
            if !matches!((version, p.len()), (1, 4) | (2 | 3, 12)) {
                return Err(bad("invalid DVC capability version or length"));
            }
            self.negotiated = true;
            // Version 2 keeps both endpoints uncompressed.
            return Ok(vec![vec![0x50, 0, version.min(2) as u8, 0]]);
        }
        if !self.negotiated {
            return Err(bad("DVC message before capabilities"));
        }
        let id = number(&mut b, p[0] & 3)?;
        match cmd {
            1 => {
                if b.is_empty()
                    || b.len() > 256
                    || !b.ends_with(&[0])
                    || b[..b.len() - 1].contains(&0)
                {
                    return Err(bad("invalid DVC channel name"));
                }
                if self.id == Some(id) || self.graphics_id == Some(id) {
                    return Err(bad("duplicate DVC channel identifier"));
                }
                let display = b == NAME && self.id.is_none();
                let graphics =
                    self.graphics_enabled && b == GRAPHICS_NAME && self.graphics_id.is_none();
                let accepted = display || graphics;
                if display {
                    self.id = Some(id);
                }
                if graphics {
                    self.graphics_id = Some(id);
                    self.graphics_events.push(GraphicsEvent::Opened);
                }
                let mut reply = header(1, id);
                reply.extend((if accepted { 0u32 } else { 0xc00000bb }).to_le_bytes());
                Ok(vec![reply])
            }
            2 | 3 => {
                if self.graphics_id == Some(id) {
                    self.graphics_data(cmd, (p[0] >> 2) & 3, b)?;
                    return Ok(Vec::new());
                }
                if self.id != Some(id) {
                    return Err(bad("data for an unopened DVC"));
                }
                if cmd == 2 {
                    if self.total.is_some() {
                        return Err(bad("overlapping DVC fragments"));
                    }
                    let total = number(&mut b, (p[0] >> 2) & 3)? as usize;
                    // The only server message on this endpoint is a 20-byte CAPS PDU.
                    if total != 20 {
                        return Err(bad("invalid display capability message size"));
                    }
                    self.total = Some(total);
                }
                if let Some(total) = self.total {
                    if b.is_empty() || self.fragment.len() + b.len() > total {
                        return Err(bad("invalid DVC fragment size"));
                    }
                    self.fragment.extend_from_slice(b);
                    if self.fragment.len() == total {
                        let complete = std::mem::take(&mut self.fragment);
                        self.total = None;
                        self.caps(&complete)?;
                    }
                } else {
                    self.caps(b)?;
                }
                Ok(Vec::new())
            }
            4 => {
                if !b.is_empty() {
                    return Err(bad("trailing DVC close data"));
                }
                if self.id == Some(id) {
                    self.id = None;
                    self.area = None;
                    self.fragment.clear();
                    self.total = None;
                }
                if self.graphics_id == Some(id) {
                    self.graphics_id = None;
                    self.graphics_fragment.clear();
                    self.graphics_total = None;
                    self.graphics_events.push(GraphicsEvent::Closed);
                }
                Ok(vec![header(4, id)])
            }
            _ => Err(bad("unsupported DVC command")),
        }
    }
    fn caps(&mut self, b: &[u8]) -> Result<()> {
        if b.len() != 20 {
            return Err(bad("invalid display capabilities length"));
        }
        let v: Vec<_> = b
            .as_chunks::<4>()
            .0
            .iter()
            .map(|v| u32::from_le_bytes(*v))
            .collect();
        if v[0] != 5 || v[1] != 20 || v[2] == 0 || v[3] == 0 || v[4] == 0 {
            return Err(bad("invalid display capabilities"));
        }
        let area = u64::from(v[2])
            .checked_mul(u64::from(v[3]))
            .and_then(|n| n.checked_mul(u64::from(v[4])))
            .ok_or_else(|| bad("display area overflow"))?;
        self.area = Some(area.min(16_777_216));
        Ok(())
    }
    /// Odd widths round down as required by MS-RDPEDISP. Invalid sizes retain scaling.
    pub fn size(&self, width: usize, height: usize) -> Option<(u16, u16)> {
        if !(200..=8192).contains(&width) || !(200..=8192).contains(&height) {
            return None;
        }
        let width = width & !1;
        (width as u64 * height as u64 <= self.area?).then_some((width as u16, height as u16))
    }
    pub fn layout(&self, width: u16, height: u16) -> Result<Vec<u8>> {
        if self.size(width.into(), height.into()) != Some((width, height)) {
            return Err(bad("display layout outside negotiated limits"));
        }
        let mut p = header(
            3,
            self.id.ok_or_else(|| bad("display channel is not open"))?,
        );
        for v in [
            2u32,
            56,
            40,
            1,
            1,
            0,
            0,
            width.into(),
            height.into(),
            0,
            0,
            0,
            100,
            100,
        ] {
            p.extend(v.to_le_bytes());
        }
        Ok(p)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn create(c: &mut DisplayControl, id: u32, name: &[u8]) -> Result<Vec<Vec<u8>>> {
        let mut p = header(1, id);
        p.extend(name);
        c.receive(&p)
    }
    fn graphics(id: u32) -> DisplayControl {
        let mut c = DisplayControl::with_graphics();
        c.receive(&[0x50, 0, 1, 0]).unwrap();
        create(&mut c, id, GRAPHICS_NAME).unwrap();
        assert_eq!(c.take_graphics(), [GraphicsEvent::Opened]);
        c
    }
    #[test]
    fn graphics_fragmentation_round_trips_all_id_and_length_widths() {
        for id in [1, 300, 70_000] {
            for len in [1, 1595, 1598, 1600, 65_535, 65_536, MAX_GRAPHICS_MESSAGE] {
                let mut c = graphics(id);
                let data: Vec<_> = (0..len).map(|i| i as u8).collect();
                let packets = c.send_graphics(&data).unwrap();
                for p in &packets {
                    assert!(p.len() <= 1600);
                    c.receive(p).unwrap();
                }
                assert!(!c.partial());
                assert_eq!(c.take_graphics(), [GraphicsEvent::Data(data)]);
            }
        }
    }
    #[test]
    fn interleaved_endpoints_close_and_reopen_independently() {
        let mut c = graphics(1);
        create(&mut c, 300, NAME).unwrap();
        let caps = caps();
        let mut first = header(2, 300);
        first.push(20);
        first.extend(&caps[..7]);
        c.receive(&first).unwrap();
        let data = vec![42; 5000];
        let packets = c.send_graphics(&data).unwrap();
        c.receive(&packets[0]).unwrap();
        let mut last = header(3, 300);
        last.extend(&caps[7..]);
        c.receive(&last).unwrap();
        assert!(c.ready());
        assert!(c.partial());
        for p in &packets[1..] {
            c.receive(p).unwrap();
        }
        assert_eq!(c.take_graphics(), [GraphicsEvent::Data(data)]);
        c.receive(&packets[0]).unwrap();
        c.receive(&header(4, 1)).unwrap();
        assert!(c.ready());
        assert!(!c.partial());
        assert_eq!(c.take_graphics(), [GraphicsEvent::Closed]);
        create(&mut c, 1, GRAPHICS_NAME).unwrap();
        c.receive(&header(4, 300)).unwrap();
        assert!(!c.ready());
        let p = c.send_graphics(b"still open").unwrap();
        c.receive(&p[0]).unwrap();
        assert_eq!(
            c.take_graphics(),
            [
                GraphicsEvent::Opened,
                GraphicsEvent::Data(b"still open".to_vec())
            ]
        );
        create(&mut c, 300, NAME).unwrap();
        c.receive(&last).unwrap_err(); // A previous fragmented capability does not survive close.
    }
    #[test]
    fn graphics_rejects_duplicate_ids_unknown_channels_and_bad_fragments() {
        let mut c = open();
        let rejected = create(&mut c, 1, GRAPHICS_NAME).unwrap();
        assert_eq!(&rejected[0][2..], &0xc00000bbu32.to_le_bytes());
        let mut c = graphics(1);
        assert!(create(&mut c, 1, NAME).is_err());
        create(&mut c, 300, NAME).unwrap();
        assert!(create(&mut c, 300, GRAPHICS_NAME).is_err());
        assert!(c.send_graphics(&[]).is_err());
        assert!(c.send_graphics(&vec![0; MAX_GRAPHICS_MESSAGE + 1]).is_err());
        for size in [0u32, MAX_GRAPHICS_MESSAGE as u32 + 1, u32::MAX] {
            let mut p = vec![0x28, 1];
            p.extend(size.to_le_bytes());
            p.push(0);
            assert!(c.receive(&p).is_err());
        }
        assert!(c.receive(&[0x30, 1]).is_err());
        assert!(c.receive(&[0x30, 2, 0]).is_err());
        assert!(c.receive(&vec![0; 1601]).is_err());
        c.receive(&[0x20, 1, 2, 7]).unwrap();
        assert!(c.receive(&[0x20, 1, 2, 8]).is_err());
        assert!(c.receive(&[0x30, 1, 8, 9]).is_err());
        c.receive(&header(4, 1)).unwrap();
        create(&mut c, 1, GRAPHICS_NAME).unwrap();
        c.receive(&[0x20, 1, 2, 7]).unwrap();
        c.receive(&[0x30, 1, 8]).unwrap();
        assert!(!c.partial());
        assert_eq!(
            c.take_graphics().last(),
            Some(&GraphicsEvent::Data(vec![7, 8]))
        );
    }
    fn open() -> DisplayControl {
        let mut c = DisplayControl::default();
        assert_eq!(
            c.receive(&[0x50, 0, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0]).unwrap(),
            [vec![0x50, 0, 2, 0]]
        );
        let mut p = header(1, 300);
        p.extend(NAME);
        assert_eq!(c.receive(&p).unwrap(), [vec![0x11, 44, 1, 0, 0, 0, 0]]);
        c
    }
    fn caps() -> Vec<u8> {
        [5u32, 20, 1, 4096, 4096]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect()
    }
    #[test]
    fn caps_and_monitor_layout_match_wire_fields() {
        let mut c = open();
        assert!(!c.ready());
        let mut p = header(3, 300);
        p.extend(caps());
        c.receive(&p).unwrap();
        assert_eq!(c.size(1281, 800), Some((1280, 800)));
        let p = c.layout(1280, 800).unwrap();
        assert_eq!(&p[..3], &[0x31, 44, 1]);
        let v: Vec<_> = p[3..]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|v| u32::from_le_bytes(*v))
            .collect();
        assert_eq!(v, [2, 56, 40, 1, 1, 0, 0, 1280, 800, 0, 0, 0, 100, 100]);
        assert!(c.size(8192, 8192).is_none());
        assert!(c.layout(1281, 800).is_err());
    }
    #[test]
    fn fragmented_caps_close_and_unknown_channels() {
        let mut c = open();
        let mut p = header(1, 4);
        p.extend(b"unsupported\0");
        assert_eq!(
            &c.receive(&p).unwrap()[0][2..],
            &0xc00000bbu32.to_le_bytes()
        );
        let caps = caps();
        let mut p = header(2, 300);
        p.push(20);
        p.extend(&caps[..7]);
        c.receive(&p).unwrap();
        assert!(c.partial());
        let mut p = header(3, 300);
        p.extend(&caps[7..]);
        c.receive(&p).unwrap();
        assert!(c.ready());
        c.receive(&header(4, 300)).unwrap();
        assert!(!c.ready());
        assert!(c.receive(&p).is_err());
    }
    #[test]
    fn rejects_bad_sequences_lengths_and_overflow() {
        assert!(
            DisplayControl::default()
                .receive(&[0x10, 1, b'x', 0])
                .is_err()
        );
        let mut c = open();
        let mut p = header(3, 300);
        p.extend(caps());
        for n in 0..p.len() {
            assert!(open().receive(&p[..n]).is_err());
        }
        p[11..].fill(255);
        assert!(c.receive(&p).is_err());
        assert!(c.receive(&[0x23, 1]).is_err());
    }
}
