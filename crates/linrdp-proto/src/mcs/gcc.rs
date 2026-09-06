use super::{Error, Reader, ServerSettings, Settings};
use crate::negotiation::SecurityProtocol;

const T124_OID: &[u8] = &[0, 5, 0, 20, 124, 0, 1];

fn per_length(out: &mut Vec<u8>, value: usize) {
    if value < 128 {
        out.push(value as u8);
    } else {
        out.extend_from_slice(&((value as u16) | 0x8000).to_be_bytes());
    }
}

pub(super) fn request(settings: Settings, protocol: SecurityProtocol) -> Vec<u8> {
    let mut core = vec![0; 216];
    core[..4].copy_from_slice(&[1, 0xc0, 216, 0]);
    core[4..8].copy_from_slice(&0x00080004u32.to_le_bytes());
    core[8..10].copy_from_slice(&settings.width.to_le_bytes());
    core[10..12].copy_from_slice(&settings.height.to_le_bytes());
    core[12..16].copy_from_slice(&[1, 0xca, 3, 0xaa]);
    core[16..20].copy_from_slice(&settings.keyboard_layout.to_le_bytes());
    // Product build is our initial diagnostic build, not an impersonated OS.
    core[20..24].copy_from_slice(&1u32.to_le_bytes());
    for (i, ch) in "LinRDP".encode_utf16().enumerate() {
        core[24 + 2 * i..26 + 2 * i].copy_from_slice(&ch.to_le_bytes());
    }
    core[56..60].copy_from_slice(&4u32.to_le_bytes()); // enhanced keyboard
    core[64..68].copy_from_slice(&12u32.to_le_bytes());
    core[132..136].copy_from_slice(&[1, 0xca, 1, 0]);
    core[140..144].copy_from_slice(&[16, 0, 2, 0]); // 16-bit color only
    core[144] = 5; // RNS_UD_CS_SUPPORT_ERRINFO_PDU | SUPPORT_STATUSINFO_PDU
    let selected: u32 = match protocol {
        SecurityProtocol::Tls => 1,
        SecurityProtocol::CredSsp => 2,
        SecurityProtocol::CredSspEarlyAuth => 8,
    };
    core[212..216].copy_from_slice(&selected.to_le_bytes());
    // TLS provides encryption; zero legacy encryption methods.
    core.extend_from_slice(&[2, 0xc0, 12, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    if settings.clipboard {
        core.extend_from_slice(&[3, 0xc0, 20, 0, 1, 0, 0, 0]);
        core.extend_from_slice(b"cliprdr\0");
        core.extend_from_slice(&0xc0000000u32.to_le_bytes()); // initialized, encrypt under legacy security
    } else {
        core.extend_from_slice(&[3, 0xc0, 8, 0, 0, 0, 0, 0]);
    }
    let mut conference = vec![0, 8, 0, 0x10, 0, 1, 0xc0, 0];
    conference.extend_from_slice(b"Duca");
    per_length(&mut conference, core.len());
    conference.extend_from_slice(&core);
    let mut gcc = T124_OID.to_vec();
    per_length(&mut gcc, conference.len());
    gcc.extend_from_slice(&conference);
    gcc
}

pub(super) fn response(bytes: &[u8], requested: u32) -> Result<ServerSettings, Error> {
    let mut r = Reader(bytes);
    r.expect(T124_OID)?;
    r.per_length()?; // Ignored ConnectData::connectPDU length per MS-RDPBCGR.
    r.expect(&[0x14])?; // conferenceCreateResponse with userData
    r.be16()?; // node ID is ignored
    let tag_length = r.per_length()?;
    if tag_length == 0 || tag_length > 4 {
        return Err(Error("invalid GCC tag"));
    }
    r.take(tag_length)?;
    r.byte()?; // conference result ignored by RDP
    r.expect(&[1, 0xc0, 0])?; // one user-data set, four-byte H.221 key
    r.expect(b"McDn")?;
    let length = r.per_length()?;
    let blocks = r.take(length)?;
    r.end()?;
    server_blocks(blocks, requested)
}

fn server_blocks(bytes: &[u8], requested: u32) -> Result<ServerSettings, Error> {
    let mut r = Reader(bytes);
    let mut core = None;
    let mut security = false;
    let mut network = None;
    let mut clipboard_channel = None;
    while !r.0.is_empty() {
        let kind = r.le16()?;
        let length = usize::from(r.le16()?)
            .checked_sub(4)
            .ok_or(Error("invalid GCC block length"))?;
        let mut block = Reader(r.take(length)?);
        match kind {
            0x0c01 => {
                if core.is_some() || ![4, 8, 12].contains(&length) {
                    return Err(Error("invalid or duplicate server core"));
                }
                let version = block.le32()?;
                if version < 0x00080004 {
                    return Err(Error("unsupported server version"));
                }
                let echoed = if length >= 8 { block.le32()? } else { 0 };
                if echoed != requested {
                    return Err(Error("server changed requested security protocols"));
                }
                let flags = if length == 12 { block.le32()? } else { 0 };
                // We did not advertise channel-join skipping.
                if flags & 8 != 0 {
                    return Err(Error("unrequested channel-join skipping"));
                }
                core = Some((version, flags));
            }
            0x0c02 => {
                if security || length != 8 || block.le32()? != 0 || block.le32()? != 0 {
                    return Err(Error("invalid legacy security data on TLS connection"));
                }
                security = true;
            }
            0x0c03 => {
                if network.is_some() || ![4, 8].contains(&length) {
                    return Err(Error("invalid or duplicate server network data"));
                }
                let channel = block.le16()?;
                let count = block.le16()?;
                if channel < 1001 || count > 1 || length != if count == 0 { 4 } else { 8 } {
                    return Err(Error("invalid server channel assignment"));
                }
                if count == 1 {
                    let clip = block.le16()?;
                    if clip < 1001 || clip == channel {
                        return Err(Error("invalid clipboard channel"));
                    }
                    clipboard_channel = Some(clip);
                    block.le16()?; // odd channel count padding
                }
                network = Some(channel);
            }
            _ => return Err(Error("unsupported server GCC block")),
        }
        block.end()?;
    }
    let (version, early_capability_flags) = core.ok_or(Error("missing server core"))?;
    if !security {
        return Err(Error("missing server security"));
    }
    Ok(ServerSettings {
        version,
        early_capability_flags,
        io_channel: network.ok_or(Error("missing server network"))?,
        clipboard_channel,
    })
}
