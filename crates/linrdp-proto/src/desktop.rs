//! First-desktop RDP profile (MS-RDPBCGR).
//! Enhanced security, valid-client licensing, bitmap output, no drawing orders.
use std::fmt;

mod bitmap;
mod capabilities;
mod fastpath;
mod input;
mod pointer;
pub use bitmap::Framebuffer;
pub use fastpath::frame_length;
pub use input::Input;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}
fn bad(message: &str) -> Error {
    Error(message.into())
}
type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Licensing,
    DemandActive,
    Finalizing,
    Active,
    Deactivated,
}

pub struct Session {
    held: input::Held,
    user: u16,
    channel: u16,
    share: u32,
    server: u16,
    pub phase: Phase,
    pub framebuffer: Framebuffer,
    synchronized: bool,
    cooperated: bool,
    granted: bool,
    pointer: pointer::Pointer,
    fragment: Option<(u8, Vec<u8>)>,
    pub revision: u64,
    refresh_supported: bool,
    pub notifications: Vec<String>,
}
impl Session {
    pub fn new(user: u16, channel: u16) -> Result<Self> {
        if user < 1001 || channel < 1001 || user == channel {
            return Err(bad("invalid session channels"));
        }
        Ok(Self {
            held: input::Held::default(),
            user,
            channel,
            share: 0,
            server: 0,
            phase: Phase::Licensing,
            framebuffer: Framebuffer::new(1024, 768)?,
            synchronized: false,
            cooperated: false,
            granted: false,
            pointer: pointer::Pointer::default(),
            fragment: None,
            revision: 0,
            refresh_supported: false,
            notifications: Vec::new(),
        })
    }

    /// Client Info travels only inside the previously authenticated TLS stream.
    pub fn client_info(
        &self,
        domain: &str,
        username: &str,
        password: &str,
        address: std::net::IpAddr,
    ) -> Result<zeroize::Zeroizing<Vec<u8>>> {
        let domain = utf16(domain, 50)?;
        let username = utf16(username, 512)?;
        let password = zeroize::Zeroizing::new(utf16(password, 512)?);
        let mut b = zeroize::Zeroizing::new(vec![0x40, 0, 0, 0]); // SEC_INFO_PKT, basic security header under TLS
        u32le(&mut b, 0); // Unicode codepage unused
        u32le(&mut b, 0x0009017b); // mouse, CAD disabled, autologon, Unicode, maximize, logon notify, Windows key, audio disabled
        for size in [
            domain.len() - 2,
            username.len() - 2,
            password.len() - 2,
            0,
            0,
        ] {
            u16le(&mut b, size as u16);
        }
        b.extend(domain);
        b.extend(username);
        b.extend_from_slice(&password);
        b.extend([0; 4]);
        u16le(&mut b, if address.is_ipv4() { 2 } else { 23 });
        let addr = utf16(&address.to_string(), 80)?;
        u16le(&mut b, addr.len() as u16);
        b.extend(addr);
        let dir = utf16("LinRDP", 512)?;
        u16le(&mut b, dir.len() as u16);
        b.extend(dir);
        // UTC timezone, no performance restrictions or reconnection cookie.
        b.extend([0; 180]);
        u16le(&mut b, 0);
        self.send(&b).map(zeroize::Zeroizing::new)
    }

    /// Decode one complete MCS SendDataIndication and return outbound MCS PDUs.
    pub fn receive(&mut self, payload: &[u8]) -> Result<Vec<Vec<u8>>> {
        let mut r = Cursor(payload);
        if r.0.first() == Some(&0x21) {
            return Err(bad("server disconnected the RDP session"));
        }
        r.expect(&[0x68])?;
        r.be16()?
            .checked_add(1001)
            .ok_or_else(|| bad("server user ID overflow"))?;
        if r.be16()? != self.channel {
            return Err(bad("unexpected MCS channel"));
        }
        r.expect(&[0x70])?; // high priority, unsegmented
        let length = r.per_length()?;
        let data = r.take(length)?;
        r.end()?;
        if self.phase == Phase::Licensing {
            license_valid_client(data)?;
            self.phase = Phase::DemandActive;
            return Ok(Vec::new());
        }
        let mut r = Cursor(data);
        let mut replies = Vec::new();
        while !r.0.is_empty() {
            let length = usize::from(r.u16()?);
            if length < 6 {
                return Err(bad("invalid Share Control length"));
            }
            let mut pdu = Cursor(r.take(length - 2)?);
            let kind = pdu.u16()?;
            let source = pdu.u16()?;
            if kind & 0xfff0 != 0x10 {
                return Err(bad("unsupported Share Control version"));
            }
            match kind & 15 {
                1 => {
                    if !matches!(self.phase, Phase::DemandActive | Phase::Deactivated) {
                        return Err(bad("unexpected Demand Active"));
                    }
                    let demand = capabilities::demand(pdu.0)?;
                    self.share = demand.share;
                    self.server = source;
                    self.refresh_supported = demand.refresh;
                    self.framebuffer = Framebuffer::new(demand.width, demand.height)?;
                    self.held = input::Held::default();
                    self.fragment = None;
                    self.pointer = pointer::Pointer::default();
                    self.synchronized = false;
                    self.cooperated = false;
                    self.granted = false;
                    replies.push(self.send(&capabilities::confirm(self.user, source, demand)?)?);
                    let mut sync = vec![1, 0];
                    u16le(&mut sync, source);
                    replies.push(self.data(31, &sync)?);
                    replies.push(self.data(20, &[4, 0, 0, 0, 0, 0, 0, 0])?);
                    replies.push(self.data(20, &[1, 0, 0, 0, 0, 0, 0, 0])?);
                    replies.push(self.data(39, &[0, 0, 0, 0, 3, 0, 50, 0])?);
                    self.phase = Phase::Finalizing;
                }
                6 => {
                    if !matches!(self.phase, Phase::Active | Phase::Finalizing) {
                        return Err(bad("unexpected deactivation"));
                    }
                    if pdu.u32()? != self.share {
                        return Err(bad("deactivation share mismatch"));
                    }
                    let len = usize::from(pdu.u16()?);
                    pdu.take(len)?;
                    pdu.end()?;
                    self.phase = Phase::Deactivated;
                }
                7 => {
                    let previous = self.phase;
                    self.share_data(pdu, source)?;
                    if previous == Phase::Finalizing && self.phase == Phase::Active {
                        replies.push(
                            self.data(28, &[1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])?,
                        );
                        if self.refresh_supported {
                            let mut area = vec![1, 0, 0, 0, 0, 0, 0, 0];
                            u16le(&mut area, self.framebuffer.width - 1);
                            u16le(&mut area, self.framebuffer.height - 1);
                            replies.push(self.data(33, &area)?);
                        }
                    }
                }
                _ => return Err(Error(format!("unsupported Share Control PDU {kind:#x}"))),
            }
        }
        Ok(replies)
    }

    fn share_data(&mut self, mut r: Cursor<'_>, source: u16) -> Result<()> {
        if !matches!(self.phase, Phase::Finalizing | Phase::Active) {
            return Err(bad("data before activation"));
        }
        if source != self.server || r.u32()? != self.share {
            return Err(bad("Share Data session mismatch"));
        }
        r.take(2)?; // padding, stream priority
        r.u16()?; // uncompressedLength is unreliable on some Windows control PDUs
        let kind = r.byte()?;
        if r.byte()? != 0 || r.u16()? != 0 {
            return Err(bad("server sent unnegotiated bulk compression"));
        }
        match kind {
            31 => {
                r.expect(&[1, 0])?;
                r.u16()?;
                r.end()?;
                self.synchronized = true;
            }
            20 => {
                let action = r.u16()?;
                r.u16()?;
                r.u32()?;
                r.end()?;
                match action {
                    4 => self.cooperated = true,
                    2 => self.granted = true,
                    _ => return Err(bad("invalid server control action or grant")),
                }
            }
            40 => {
                r.take(8)?;
                r.end()?;
                if !self.synchronized || !self.cooperated || !self.granted {
                    return Err(bad("font map before session control completed"));
                }
                self.phase = Phase::Active;
            }
            2 if self.phase == Phase::Active => {
                self.framebuffer.update(r.0)?;
                self.revision = self.revision.saturating_add(1);
            }
            47 => {
                let code = r.u32()?;
                r.end()?;
                if code != 0 {
                    return Err(Error(format!("server RDP error {code:#010x}")));
                }
            }
            27 => {
                self.pointer.update(r.0)?;
                self.revision = self.revision.saturating_add(1);
            }
            38 => {
                let info = r.u32()?;
                if info == 3 {
                    r.u16()?;
                    let fields = r.u32()?;
                    if fields & !3 != 0 {
                        return Err(bad("unknown extended logon fields"));
                    }
                    if fields & 1 != 0 {
                        let len = r.u32()? as usize;
                        r.take(len)?;
                    }
                    if fields & 2 != 0 {
                        if r.u32()? != 8 {
                            return Err(bad("invalid logon error field length"));
                        }
                        let kind = r.u32()?;
                        let detail = r.u32()?;
                        let message = match kind {
                            0xfffffffb => {
                                "Windows is showing a session contention dialog for another signed-in user"
                            }
                            0xfffffffa => "Windows is showing a remote-logon permission dialog",
                            0xfffffffc => "Windows is showing a session reconnection dialog",
                            0xfffffffd => "Windows is terminating the logon session",
                            0xfffffffe => "Windows logon is continuing",
                            0xffffffff => "Windows denied desktop logon",
                            _ => "Windows logon notification",
                        };
                        self.notifications
                            .push(format!("{message} ({kind:#010x}, detail {detail:#010x})"));
                    }
                    r.take(570)?;
                    r.end()?;
                }
            }
            54 => {
                let status = r.u32()?;
                r.end()?;
                self.notifications
                    .push(format!("Windows session status {status:#010x}"));
            }
            _ => return Err(Error(format!("unsupported Share Data PDU {kind}"))),
        }
        Ok(())
    }

    pub fn copy_display(&self, pixels: &mut Vec<u32>) {
        pixels.clone_from(&self.framebuffer.pixels);
        self.pointer.overlay(
            pixels,
            usize::from(self.framebuffer.width),
            usize::from(self.framebuffer.height),
        );
    }

    fn send(&self, data: &[u8]) -> Result<Vec<u8>> {
        if data.len() > 0x3fff {
            return Err(bad("outbound MCS payload too large"));
        }
        let mut out = vec![0x64];
        out.extend_from_slice(&(self.user - 1001).to_be_bytes());
        out.extend_from_slice(&self.channel.to_be_bytes());
        out.push(0x70);
        if data.len() < 128 {
            out.push(data.len() as u8);
        } else {
            out.extend_from_slice(&((data.len() as u16) | 0x8000).to_be_bytes());
        }
        out.extend_from_slice(data);
        Ok(out)
    }
    fn data(&self, kind: u8, body: &[u8]) -> Result<Vec<u8>> {
        let mut b = Vec::new();
        u32le(&mut b, self.share);
        b.extend([0, 1]);
        u16le(&mut b, (body.len() + 4) as u16);
        b.extend([kind, 0, 0, 0]);
        b.extend(body);
        self.send(&share_control(self.user, 7, &b)?)
    }
}

fn license_valid_client(data: &[u8]) -> Result<()> {
    let mut r = Cursor(data);
    let flags = r.u16()?;
    r.u16()?;
    if flags & !0x200 != 0x80 {
        return Err(bad("invalid licensing security header"));
    }
    let kind = r.byte()?;
    if r.byte()? & 15 != 3 {
        return Err(bad("unsupported licensing version"));
    }
    let size = usize::from(r.u16()?);
    if size != data.len() - 4 {
        return Err(bad("licensing length mismatch"));
    }
    if kind != 0xff {
        return Err(Error(format!(
            "licensing message {kind:#x} requires an RDS licensing exchange; currently only STATUS_VALID_CLIENT is supported"
        )));
    }
    let code = r.u32()?;
    let transition = r.u32()?;
    if code != 7 || transition != 2 {
        return Err(Error(format!(
            "server licensing denied: code {code:#x}, transition {transition}"
        )));
    }
    if r.u16()? != 4 {
        return Err(bad("invalid licensing error blob"));
    }
    let len = usize::from(r.u16()?);
    r.take(len)?;
    r.end()
}
fn utf16(text: &str, max: usize) -> Result<Vec<u8>> {
    if text.contains('\0') || text.len() > max {
        return Err(bad("invalid Client Info string"));
    }
    let bytes: Vec<u8> = text
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect();
    if bytes.len() > max {
        return Err(bad("Client Info string too long"));
    }
    Ok(bytes)
}
fn share_control(user: u16, kind: u16, body: &[u8]) -> Result<Vec<u8>> {
    let length = u16::try_from(body.len() + 6).map_err(|_| bad("Share Control PDU too large"))?;
    let mut b = Vec::new();
    u16le(&mut b, length);
    u16le(&mut b, 0x10 | kind);
    u16le(&mut b, user);
    b.extend(body);
    Ok(b)
}
fn u16le(out: &mut Vec<u8>, n: u16) {
    out.extend(n.to_le_bytes());
}
fn u32le(out: &mut Vec<u8>, n: u32) {
    out.extend(n.to_le_bytes());
}

struct Cursor<'a>(&'a [u8]);
impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.0.len() {
            return Err(bad("truncated RDP desktop PDU"));
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn be16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn expect(&mut self, b: &[u8]) -> Result<()> {
        if self.take(b.len())? == b {
            Ok(())
        } else {
            Err(bad("unexpected RDP desktop field"))
        }
    }
    fn end(self) -> Result<()> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(bad("trailing RDP desktop data"))
        }
    }
    fn per_length(&mut self) -> Result<usize> {
        let b = self.byte()?;
        if b < 128 {
            return Ok(usize::from(b));
        }
        if b & 0x40 != 0 {
            return Err(bad("fragmented MCS payload unsupported"));
        }
        Ok((usize::from(b & 63) << 8) | usize::from(self.byte()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn indication(body: &[u8]) -> Vec<u8> {
        let mut b = vec![0x68, 0, 1, 3, 0xeb, 0x70];
        if body.len() < 128 {
            b.push(body.len() as u8);
        } else {
            b.extend(((body.len() as u16) | 0x8000).to_be_bytes());
        }
        b.extend(body);
        b
    }
    fn licensed() -> Session {
        let mut s = Session::new(1004, 1003).unwrap();
        s.receive(&indication(&[
            0x80, 0, 0, 0, 0xff, 3, 16, 0, 7, 0, 0, 0, 2, 0, 0, 0, 4, 0, 0, 0,
        ]))
        .unwrap();
        s
    }
    fn demand() -> Vec<u8> {
        let mut b = vec![0xea, 3, 1, 0, 1, 0, 56, 0, 0, 2, 0, 0, 0]; // share, descriptor size, caps size, descriptor, count/pad
        b.extend([1, 0, 24, 0]);
        b.extend([0; 20]);
        b.extend([2, 0, 28, 0, 16, 0, 1, 0, 1, 0, 1, 0, 0, 4, 0, 3]);
        b.extend([0; 12]);
        b.extend([0; 4]);
        indication(&share_control(1002, 1, &b).unwrap())
    }
    fn server_data(kind: u8, body: &[u8]) -> Vec<u8> {
        let mut b = vec![0xea, 3, 1, 0, 0, 1];
        u16le(&mut b, (body.len() + 4) as u16);
        b.extend([kind, 0, 0, 0]);
        b.extend(body);
        indication(&share_control(1002, 7, &b).unwrap())
    }
    #[test]
    fn activation_requires_license_capabilities_and_server_control() {
        let mut s = licensed();
        assert_eq!(s.phase, Phase::DemandActive);
        let replies = s.receive(&demand()).unwrap();
        assert_eq!(replies.len(), 5);
        assert_eq!(s.phase, Phase::Finalizing);
        s.receive(&server_data(31, &[1, 0, 0xec, 3])).unwrap();
        s.receive(&server_data(20, &[4, 0, 0, 0, 0, 0, 0, 0]))
            .unwrap();
        s.receive(&server_data(20, &[2, 0, 0xec, 3, 0, 0, 0, 0]))
            .unwrap();
        s.receive(&server_data(40, &[0, 0, 0, 0, 3, 0, 4, 0]))
            .unwrap();
        assert_eq!(s.phase, Phase::Active);
        let mut s = licensed();
        s.receive(&demand()).unwrap();
        assert!(s.receive(&server_data(40, &[0; 8])).is_err());
    }
    #[test]
    fn rejects_failed_licensing_and_truncated_activation() {
        let mut s = Session::new(1004, 1003).unwrap();
        assert!(s.receive(&demand()).is_err());
        let license = [
            0x80, 0, 0, 0, 0xff, 3, 16, 0, 7, 0, 0, 0, 2, 0, 0, 0, 4, 0, 0, 0,
        ];
        for end in 0..license.len() {
            assert!(license_valid_client(&license[..end]).is_err());
        }
        let mut bad_license = license;
        bad_license[8] = 8;
        assert!(license_valid_client(&bad_license).is_err());
        let d = demand();
        for end in 0..d.len() {
            assert!(licensed().receive(&d[..end]).is_err());
        }
        let mut wrong = d.clone();
        wrong[4] ^= 1;
        assert!(licensed().receive(&wrong).is_err());
    }
    #[test]
    fn raw_bitmap_orientation_color_and_padding() {
        let mut f = Framebuffer::new(2, 2).unwrap();
        // Bottom row blue/white, top row red/green, RGB565 little endian.
        let b = [
            1, 0, 1, 0, 0, 0, 0, 0, 1, 0, 1, 0, 2, 0, 2, 0, 16, 0, 0, 0, 8, 0, 31, 0, 255, 255, 0,
            248, 224, 7,
        ];
        f.update(&b).unwrap();
        assert_eq!(f.pixels, [0xff0000, 0x00ff00, 0x0000ff, 0xffffff]);
        for end in 0..b.len() {
            assert!(Framebuffer::new(2, 2).unwrap().update(&b[..end]).is_err());
        }
        let mut bad = b;
        bad[8] = 2;
        assert!(f.update(&bad).is_err());
        let single = [
            1, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 0, 16, 0, 0, 0, 4, 0, 0, 248, 0, 0,
        ];
        f.update(&single).unwrap();
        assert_eq!(f.pixels[0], 0xff0000);
    }
    #[test]
    fn compressed_bitmap_matches_raw_and_rejects_overrun() {
        let mut f = Framebuffer::new(2, 2).unwrap();
        // Regular COLOR_IMAGE run of four literal RGB565 pixels.
        let mut b = vec![
            1, 0, 1, 0, 0, 0, 0, 0, 1, 0, 1, 0, 2, 0, 2, 0, 16, 0, 1, 4, 9, 0, 0x84, 31, 0, 255,
            255, 0, 248, 224, 7,
        ];
        f.update(&b).unwrap();
        assert_eq!(f.pixels, [0xff0000, 0x00ff00, 0x0000ff, 0xffffff]);
        b[22] = 0x85;
        assert!(f.update(&b).is_err());
        assert!(Framebuffer::new(8192, 8192).is_err());
    }
    #[test]
    fn client_info_is_bounded_and_uses_zeroizing_storage() {
        let s = Session::new(1004, 1003).unwrap();
        let info = s
            .client_info("D", "U", "P", "127.0.0.1".parse().unwrap())
            .unwrap();
        assert!(info.windows(4).any(|b| b == [0x40, 0, 0, 0]));
        assert!(
            s.client_info("D", &"a".repeat(512), "P", "::1".parse().unwrap())
                .is_err()
        );
        assert!(
            s.client_info("D", "a\0b", "P", "::1".parse().unwrap())
                .is_err()
        );
    }
}
