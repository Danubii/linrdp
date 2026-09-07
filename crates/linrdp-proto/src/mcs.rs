//! Minimal MCS/GCC basic settings and channel connection (MS-RDPBCGR 2.2.1.3–9).
//! Enhanced security with optional clipboard and dynamic-channel transport.

use crate::{data, negotiation::SecurityProtocol};
use std::fmt;

mod gcc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub &'static str);
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid MCS/GCC exchange: {}", self.0)
    }
}
impl std::error::Error for Error {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub width: u16,
    pub height: u16,
    pub keyboard_layout: u32,
    pub clipboard: bool,
    pub dynamic_resolution: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            width: 1024,
            height: 768,
            keyboard_layout: 0x409,
            clipboard: false,
            dynamic_resolution: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerSettings {
    pub version: u32,
    pub io_channel: u16,
    pub static_channels: [Option<u16>; 2],
    pub early_capability_flags: u32,
}

pub const ERECT_DOMAIN: &[u8] = &[4, 1, 0, 1, 0];
pub const ATTACH_USER: &[u8] = &[0x28];

/// Build MCS Connect Initial, without its TPKT/X.224 envelope.
pub fn connect_initial(settings: Settings, protocol: SecurityProtocol) -> Result<Vec<u8>, Error> {
    if !(200..=8192).contains(&settings.width) || !(200..=8192).contains(&settings.height) {
        return Err(Error("desktop dimensions must be between 200 and 8192"));
    }
    if u32::from(settings.width) * u32::from(settings.height) > 16_777_216 {
        return Err(Error("desktop exceeds pixel budget"));
    }
    let gcc = gcc::request(settings, protocol);
    let mut body = vec![4, 1, 1, 4, 1, 1, 1, 1, 0xff];
    for parameters in [
        [34, 2, 0, 1, 0, 1, 65535, 2],
        [1, 1, 1, 1, 0, 1, 1056, 2],
        [65535, 64535, 65535, 1, 0, 1, 65535, 2],
    ] {
        let mut values = Vec::new();
        for value in parameters {
            // T.125 RDP implementations encode these unsigned domain values
            // without a sign octet (see MS-RDPBCGR 4.1.3).
            let bytes = (value as u16).to_be_bytes();
            tlv(
                &mut values,
                &[2],
                if bytes[0] == 0 { &bytes[1..] } else { &bytes },
            );
        }
        tlv(&mut body, &[0x30], &values);
    }
    tlv(&mut body, &[4], &gcc);
    let mut result = Vec::new();
    tlv(&mut result, &[0x7f, 0x65], &body);
    Ok(result)
}

pub fn connect_response(payload: &[u8], requested_protocols: u32) -> Result<ServerSettings, Error> {
    if payload.len() > data::MAX_PAYLOAD_SIZE {
        return Err(Error("response too large"));
    }
    let mut outer = Reader(payload);
    let mut body = Reader(outer.ber(&[0x7f, 0x66])?);
    outer.end()?;
    if body.ber(&[0x0a])? != [0] {
        return Err(Error("server rejected MCS connection"));
    }
    // These values are ignored by RDP, but their structure is still bounded.
    body.integer()?;
    let mut domain = Reader(body.ber(&[0x30])?);
    for _ in 0..8 {
        domain.integer()?;
    }
    domain.end()?;
    body.expect(&[4])?;
    // MS-RDPBCGR 3.2.5.3.4 requires ignoring this user-data length.
    body.ber_length()?;
    gcc::response(body.0, requested_protocols)
}

pub fn attach_confirm(payload: &[u8]) -> Result<u16, Error> {
    let mut r = Reader(payload);
    r.expect(&[0x2e, 0])?;
    let user = r
        .be16()?
        .checked_add(1001)
        .ok_or(Error("user ID overflow"))?;
    r.end()?;
    Ok(user)
}

pub fn join_request(user: u16, channel: u16) -> Result<Vec<u8>, Error> {
    let user = user.checked_sub(1001).ok_or(Error("invalid user ID"))?;
    if channel < 1001 {
        return Err(Error("invalid channel ID"));
    }
    let mut result = vec![0x38];
    result.extend_from_slice(&user.to_be_bytes());
    result.extend_from_slice(&channel.to_be_bytes());
    Ok(result)
}

pub fn join_confirm(payload: &[u8], user: u16, channel: u16) -> Result<(), Error> {
    let expected = join_request(user, channel)?;
    let mut r = Reader(payload);
    r.expect(&[0x3e, 0])?;
    r.expect(&expected[1..])?;
    if r.be16()? != channel {
        return Err(Error("joined channel does not match request"));
    }
    r.end()
}

fn tlv(out: &mut Vec<u8>, tag: &[u8], value: &[u8]) {
    out.extend_from_slice(tag);
    let length = value.len();
    if length < 128 {
        out.push(length as u8);
    } else if length < 256 {
        out.extend_from_slice(&[0x81, length as u8]);
    } else {
        out.push(0x82);
        out.extend_from_slice(&(length as u16).to_be_bytes());
    }
    out.extend_from_slice(value);
}

struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], Error> {
        if count > self.0.len() {
            return Err(Error("truncated field"));
        }
        let (value, rest) = self.0.split_at(count);
        self.0 = rest;
        Ok(value)
    }
    fn expect(&mut self, expected: &[u8]) -> Result<(), Error> {
        if self.take(expected.len())? != expected {
            return Err(Error("unexpected field or result"));
        }
        Ok(())
    }
    fn byte(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }
    fn be16(&mut self) -> Result<u16, Error> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn le16(&mut self) -> Result<u16, Error> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn le32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn end(self) -> Result<(), Error> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(Error("unexpected trailing data"))
        }
    }
    fn ber_length(&mut self) -> Result<usize, Error> {
        let first = self.byte()?;
        if first < 128 {
            return Ok(usize::from(first));
        }
        let count = usize::from(first & 0x7f);
        if count == 0 || count > 2 {
            return Err(Error("unsupported BER length"));
        }
        let mut size = 0;
        for b in self.take(count)? {
            size = size * 256 + usize::from(*b);
        }
        Ok(size)
    }
    fn ber(&mut self, tag: &[u8]) -> Result<&'a [u8], Error> {
        self.expect(tag)?;
        let count = self.ber_length()?;
        self.take(count)
    }
    fn integer(&mut self) -> Result<u32, Error> {
        let bytes = self.ber(&[2])?;
        if bytes.is_empty() || bytes.len() > 4 {
            return Err(Error("invalid domain integer"));
        }
        Ok(bytes.iter().fold(0, |n, b| (n << 8) | u32::from(*b)))
    }
    fn per_length(&mut self) -> Result<usize, Error> {
        let first = self.byte()?;
        if first < 128 {
            return Ok(usize::from(first));
        }
        if first & 0x40 != 0 {
            return Err(Error("fragmented PER lengths unsupported"));
        }
        Ok((usize::from(first & 0x3f) << 8) | usize::from(self.byte()?))
    }
}
