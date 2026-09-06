//! Clipboard virtual channel wire types (MS-RDPECLIP).
use crate::desktop::{Error, Result};
fn bad(s: &str) -> Error {
    Error(s.into())
}
pub const TEXT: u32 = 13;
pub const FILES: u32 = 0xc001;
pub const EFFECT: u32 = 0xc002;
pub const MAX_FILES: usize = 512;
pub const MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub fn packet(kind: u16, flags: u16, body: &[u8]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend(kind.to_le_bytes());
    p.extend(flags.to_le_bytes());
    p.extend((body.len() as u32).to_le_bytes());
    p.extend(body);
    p
}
pub fn parse(p: &[u8]) -> Result<(u16, u16, &[u8])> {
    if p.len() < 8 || p.len() > crate::channel::MAX_MESSAGE {
        return Err(bad("invalid clipboard PDU size"));
    }
    let length = u32at(p, 4)? as usize;
    // MS-RDPECLIP 2.2.1 implementation note: Windows may append four
    // bytes outside dataLen. They are padding, never clipboard contents.
    if length != p.len() - 8 && length.checked_add(4) != Some(p.len() - 8) {
        return Err(bad(&format!(
            "clipboard PDU {} length mismatch: declared {length}, received {}",
            u16at(p, 0)?,
            p.len() - 8
        )));
    }
    Ok((u16at(p, 0)?, u16at(p, 2)?, &p[8..8 + length]))
}

pub fn capabilities() -> Vec<u8> {
    packet(7, 0, &[1, 0, 0, 0, 1, 0, 12, 0, 2, 0, 0, 0, 14, 0, 0, 0])
}
pub fn capabilities_flags(b: &[u8]) -> Result<u32> {
    let count = u16at(b, 0)?;
    if count > 16 || b.len() < 4 {
        return Err(bad("invalid clipboard capabilities"));
    }
    let mut at = 4;
    let mut flags = None;
    for _ in 0..count {
        let kind = u16at(b, at)?;
        let len = usize::from(u16at(b, at + 2)?);
        if len < 4 || at + len > b.len() {
            return Err(bad("bad clipboard capability length"));
        }
        if kind == 1 {
            if len != 12 || flags.is_some() {
                return Err(bad("invalid general clipboard capability"));
            }
            flags = Some(u32at(b, at + 8)?);
        }
        at += len;
    }
    if at != b.len() {
        return Err(bad("trailing clipboard capabilities"));
    }
    Ok(flags.unwrap_or(0))
}
pub fn formats(files: bool) -> Vec<u8> {
    let mut b = Vec::new();
    for (id, name) in if files {
        vec![
            (FILES, "FileGroupDescriptorW"),
            (EFFECT, "Preferred DropEffect"),
        ]
    } else {
        vec![(TEXT, "")]
    } {
        b.extend(id.to_le_bytes());
        b.extend(utf16(name));
    }
    packet(2, 0, &b)
}
pub fn parse_formats(b: &[u8], long: bool, ascii: bool) -> Result<Vec<(u32, String)>> {
    let mut at = 0;
    let mut out = Vec::new();
    while at < b.len() {
        if out.len() >= 256 {
            return Err(bad("too many clipboard formats"));
        }
        let id = u32at(b, at)?;
        at += 4;
        let name = if long {
            let start = at;
            while u16at(b, at)? != 0 {
                at += 2;
                if at - start > 1024 {
                    return Err(bad("clipboard format name too long"));
                }
            }
            let s = unicode(&b[start..at + 2])?;
            at += 2;
            s
        } else {
            let end = at + 32;
            let field = b
                .get(at..end)
                .ok_or_else(|| bad("short clipboard format name"))?;
            at = end;
            if ascii {
                String::from_utf8(field.iter().copied().take_while(|v| *v != 0).collect())
                    .map_err(|_| bad("invalid clipboard format name"))?
            } else {
                let len = field
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .position(|v| *v == [0, 0])
                    .map_or(32, |n| n * 2 + 2);
                unicode(&field[..len])?
            }
        };
        out.push((id, name));
    }
    Ok(out)
}
pub fn utf16(s: &str) -> Vec<u8> {
    s.encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect()
}
pub fn unicode(b: &[u8]) -> Result<String> {
    if b.len() < 2 || !b.len().is_multiple_of(2) || !b.ends_with(&[0, 0]) {
        return Err(bad("invalid clipboard UTF-16"));
    }
    let units: Vec<_> = b[..b.len() - 2]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|v| u16::from_le_bytes([v[0], v[1]]))
        .collect();
    if units.contains(&0) {
        return Err(bad("embedded clipboard string terminator"));
    }
    String::from_utf16(&units).map_err(|_| bad("invalid clipboard Unicode"))
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDescriptor {
    pub name: String,
    pub directory: bool,
    pub size: u64,
}
pub fn safe_name(name: &str) -> Result<Vec<&str>> {
    if name.is_empty() || name.encode_utf16().count() > 259 {
        return Err(bad("invalid clipboard filename length"));
    }
    let parts: Vec<_> = name.split('\\').collect();
    for p in &parts {
        let base = p.split('.').next().unwrap().to_ascii_uppercase();
        if p.is_empty()
            || *p == "."
            || *p == ".."
            || p.ends_with([' ', '.'])
            || p.chars().any(|c| c < ' ' || "/:*?\"<>|".contains(c))
            || matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (base.len() == 4
                && (base.starts_with("COM") || base.starts_with("LPT"))
                && matches!(base.as_bytes()[3], b'1'..=b'9'))
        {
            return Err(bad("unsafe clipboard filename"));
        }
    }
    Ok(parts)
}
pub fn encode_files(files: &[FileDescriptor]) -> Result<Vec<u8>> {
    if files.is_empty() || files.len() > MAX_FILES {
        return Err(bad("invalid file count"));
    }
    let mut b = (files.len() as u32).to_le_bytes().to_vec();
    for f in files {
        safe_name(&f.name)?;
        if f.size > MAX_BYTES {
            return Err(bad("clipboard file exceeds limit"));
        }
        let mut d = vec![0; 592];
        d[..4].copy_from_slice(&0x44u32.to_le_bytes());
        d[36..40].copy_from_slice(&(if f.directory { 0x10u32 } else { 0x80 }).to_le_bytes());
        d[64..68].copy_from_slice(&((f.size >> 32) as u32).to_le_bytes());
        d[68..72].copy_from_slice(&(f.size as u32).to_le_bytes());
        let n = utf16(&f.name);
        d[72..72 + n.len()].copy_from_slice(&n);
        b.extend(d);
    }
    Ok(b)
}
pub fn decode_files(b: &[u8]) -> Result<Vec<FileDescriptor>> {
    let count = u32at(b, 0)? as usize;
    if count == 0 || count > MAX_FILES || b.len() != 4 + 592 * count {
        return Err(bad("invalid file descriptor list"));
    }
    let mut names = std::collections::BTreeSet::new();
    let mut total = 0u64;
    let mut files = Vec::new();
    for d in b[4..].as_chunks::<592>().0 {
        let flags = u32at(d, 0)?;
        let directory = flags & 4 != 0 && u32at(d, 36)? & 0x10 != 0;
        if !directory && flags & 0x40 == 0 {
            return Err(bad("missing clipboard file size"));
        }
        let size = if directory {
            0
        } else {
            (u64::from(u32at(d, 64)?) << 32) | u64::from(u32at(d, 68)?)
        };
        total = total
            .checked_add(size)
            .ok_or_else(|| bad("file size overflow"))?;
        if total > MAX_BYTES {
            return Err(bad("clipboard transfer exceeds size limit"));
        }
        let name = &d[72..];
        let n = name
            .as_chunks::<2>()
            .0
            .iter()
            .position(|v| *v == [0, 0])
            .ok_or_else(|| bad("unterminated file name"))?;
        let name = unicode(&name[..n * 2 + 2])?;
        safe_name(&name)?;
        if !names.insert(name.to_lowercase()) {
            return Err(bad("duplicate clipboard file name"));
        }
        files.push(FileDescriptor {
            name,
            directory,
            size,
        });
    }
    Ok(files)
}
#[derive(Clone, Copy, Debug)]
pub struct FileRequest {
    pub stream: u32,
    pub index: u32,
    pub size_only: bool,
    pub offset: u64,
    pub count: u32,
}
pub fn file_request(r: FileRequest) -> Vec<u8> {
    let mut b = Vec::new();
    for n in [
        r.stream,
        r.index,
        if r.size_only { 1 } else { 2 },
        r.offset as u32,
        (r.offset >> 32) as u32,
        r.count,
    ] {
        b.extend(n.to_le_bytes());
    }
    packet(8, 0, &b)
}
pub fn parse_file_request(b: &[u8]) -> Result<FileRequest> {
    if b.len() != 24 {
        return Err(bad("unsupported clipboard file request"));
    }
    let flags = u32at(b, 8)?;
    if flags != 1 && flags != 2 {
        return Err(bad("invalid clipboard file request flags"));
    }
    Ok(FileRequest {
        stream: u32at(b, 0)?,
        index: u32at(b, 4)?,
        size_only: flags == 1,
        offset: u64::from(u32at(b, 12)?) | (u64::from(u32at(b, 16)?) << 32),
        count: u32at(b, 20)?,
    })
}
pub fn u32at(b: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        b.get(at..at + 4)
            .ok_or_else(|| bad("truncated clipboard field"))?
            .try_into()
            .unwrap(),
    ))
}
fn u16at(b: &[u8], at: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(
        b.get(at..at + 2)
            .ok_or_else(|| bad("truncated clipboard field"))?
            .try_into()
            .unwrap(),
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formats_and_capabilities_match_vectors() {
        assert_eq!(
            capabilities_flags(parse(&capabilities()).unwrap().2).unwrap(),
            14
        );
        assert_eq!(
            parse_formats(parse(&formats(true)).unwrap().2, true, false).unwrap()[0],
            (FILES, "FileGroupDescriptorW".into())
        );
        assert_eq!(
            packet(4, 0, &13u32.to_le_bytes()),
            [4, 0, 0, 0, 4, 0, 0, 0, 13, 0, 0, 0]
        );
    }
    #[test]
    fn file_descriptors_roundtrip_and_reject_paths() {
        let files = vec![FileDescriptor {
            name: "folder\\hello.txt".into(),
            size: 123,
            directory: false,
        }];
        let b = encode_files(&files).unwrap();
        assert_eq!(decode_files(&b).unwrap(), files);
        for name in [
            "../secret",
            "..\\secret",
            "C:\\secret",
            "/etc/passwd",
            "a\\..\\b",
            "CON.txt",
            "x.",
            "a\\\\b",
        ] {
            assert!(safe_name(name).is_err(), "{name}");
        }
        for end in 0..b.len() {
            assert!(decode_files(&b[..end]).is_err());
        }
    }
    #[test]
    fn unicode_and_range_requests_are_bounded() {
        assert_eq!(unicode(&utf16("æøå😀")).unwrap(), "æøå😀");
        assert!(unicode(&[0, 0, 0, 0]).is_err());
        let r = FileRequest {
            stream: 7,
            index: 2,
            size_only: false,
            offset: 0x123456789,
            count: 65536,
        };
        let p = file_request(r);
        let r2 = parse_file_request(parse(&p).unwrap().2).unwrap();
        assert_eq!(r2.offset, r.offset);
        assert!(parse_file_request(&[0; 28]).is_err());
    }
    #[test]
    fn windows_padding_is_excluded_from_file_contents() {
        let original = packet(9, 1, &[1, 0, 0, 0, 42]);
        let mut padded = original.clone();
        padded.extend([9, 8, 7, 6]);
        assert_eq!(parse(&padded).unwrap(), (9, 1, &[1, 0, 0, 0, 42][..]));
        for count in [1, 2, 3, 5, 6, 7, 8] {
            let mut bad = original.clone();
            bad.extend(vec![0; count]);
            assert!(parse(&bad).is_err());
        }
        for length in 0..original.len() {
            assert!(parse(&original[..length]).is_err());
        }
    }
}
