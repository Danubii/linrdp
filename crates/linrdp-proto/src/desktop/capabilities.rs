use super::*;
#[derive(Clone, Copy)]
pub(super) struct Demand {
    pub share: u32,
    pub width: u16,
    pub height: u16,
    pub refresh: bool,
    pub bpp: u16,
}

pub(super) fn demand(bytes: &[u8]) -> Result<Demand> {
    let mut r = Cursor(bytes);
    let share = r.u32()?;
    let source_len = usize::from(r.u16()?);
    let caps_len = usize::from(r.u16()?);
    r.take(source_len)?;
    let mut caps = Cursor(r.take(caps_len)?);
    let count = caps.u16()?;
    caps.u16()?;
    if count > 64 {
        return Err(bad("too many server capabilities"));
    }
    let mut bitmap = None;
    let mut general = false;
    let mut refresh = false;
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..count {
        let kind = caps.u16()?;
        if !seen.insert(kind) {
            return Err(bad("duplicate server capability"));
        }
        let len = usize::from(caps.u16()?)
            .checked_sub(4)
            .ok_or_else(|| bad("invalid capability length"))?;
        let mut c = Cursor(caps.take(len)?);
        match kind {
            1 => {
                if len < 20 {
                    return Err(bad("short general capability"));
                }
                refresh = c.0[18] != 0;
                general = true;
            }
            2 => {
                if len != 24 {
                    return Err(bad("invalid bitmap capability length"));
                }
                let bpp = c.u16()?;
                if !matches!(bpp, 16 | 24 | 32) {
                    return Err(bad("server selected an unsupported bitmap color depth"));
                }
                c.take(6)?;
                let width = c.u16()?;
                let height = c.u16()?;
                Framebuffer::validate_size(width, height)?;
                bitmap = Some((width, height, bpp));
            }
            _ => {}
        }
    }
    caps.end()?;
    r.u32()?;
    r.end()?; // sessionId
    if !general {
        return Err(bad("missing general capability"));
    }
    let (width, height, bpp) = bitmap.ok_or_else(|| bad("missing bitmap capability"))?;
    Ok(Demand {
        share,
        width,
        height,
        refresh,
        bpp,
    })
}

pub(super) fn confirm(user: u16, server: u16, d: Demand) -> Result<Vec<u8>> {
    let mut caps = Vec::new();
    let mut count = 0;
    let mut add = |kind: u16, body: &[u8]| {
        u16le(&mut caps, kind);
        u16le(&mut caps, (body.len() + 4) as u16);
        caps.extend(body);
        count += 1;
    };
    // Unix/native X server, protocol 0x200, no bulk compression.
    let mut general = vec![0; 20];
    general[..6].copy_from_slice(&[4, 0, 7, 0, 0, 2]);
    general[10..12].copy_from_slice(&0x0401u16.to_le_bytes()); // fast-path output and omitted bitmap compression headers
    add(1, &general);
    let mut bitmap = vec![0; 24];
    bitmap[..8].copy_from_slice(&[16, 0, 1, 0, 1, 0, 1, 0]);
    bitmap[..2].copy_from_slice(&d.bpp.to_le_bytes());
    bitmap[8..10].copy_from_slice(&d.width.to_le_bytes());
    bitmap[10..12].copy_from_slice(&d.height.to_le_bytes());
    bitmap[14..18].copy_from_slice(&[1, 0, 1, 0]);
    bitmap[20] = 1;
    add(2, &bitmap);
    let mut order = vec![0; 84];
    order[20..24].copy_from_slice(&[1, 0, 20, 0]);
    order[26] = 1;
    order[30] = 10; // NEGOTIATEORDERSUPPORT | ZEROBOUNDSDELTASSUPPORT; all orderSupport entries are zero
    add(3, &order);
    add(5, &[0, 0, 0, 0, 2, 0, 2, 0]); // control
    add(7, &[0; 8]); // activation
    add(8, &[1, 0, 20, 0]); // Required 24-bit color pointer cache
    add(9, &[0; 4]); // share
    add(10, &[0; 4]); // no color-table cache
    let mut input = vec![0; 84];
    input[0] = 1; // scancodes; no fast-path input
    input[4..8].copy_from_slice(&0x409u32.to_le_bytes());
    input[8] = 4;
    input[16] = 12;
    add(13, &input);
    add(14, &[1, 0, 0, 0]); // font list
    add(15, &[0; 4]); // no brush support
    // Static virtual channels use the default 1600-byte chunks without compression.
    add(20, &[0, 0, 0, 0, 0x40, 0x06, 0, 0]);
    let mut b = Vec::new();
    u32le(&mut b, d.share);
    u16le(&mut b, server);
    u16le(&mut b, 7);
    u16le(&mut b, (caps.len() + 4) as u16);
    // Keep the seven-byte source descriptor used by the existing wire layout.
    b.extend(b"Fjern\0\0");
    u16le(&mut b, count);
    u16le(&mut b, 0);
    b.extend(caps);
    share_control(user, 3, &b)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn demand_with_depth(bpp: u16) -> Vec<u8> {
        let mut caps = vec![];
        for n in [2u16, 0, 1, 24] {
            u16le(&mut caps, n);
        }
        caps.extend([0; 20]);
        u16le(&mut caps, 2);
        u16le(&mut caps, 28);
        let mut bitmap = [0; 24];
        bitmap[..2].copy_from_slice(&bpp.to_le_bytes());
        bitmap[8..10].copy_from_slice(&1024u16.to_le_bytes());
        bitmap[10..12].copy_from_slice(&768u16.to_le_bytes());
        caps.extend(bitmap);
        let mut packet = vec![];
        u32le(&mut packet, 1);
        u16le(&mut packet, 0);
        u16le(&mut packet, caps.len() as u16);
        packet.extend(caps);
        u32le(&mut packet, 0);
        packet
    }
    #[test]
    fn negotiates_supported_bitmap_depth_without_downgrading() {
        for bpp in [16u16, 24, 32] {
            let selected = demand(&demand_with_depth(bpp)).unwrap();
            assert_eq!(selected.bpp, bpp);
            let reply = confirm(1002, 1001, selected).unwrap();
            // Share control header, confirm header, source descriptor, capability count,
            // and general capability precede the bitmap capability.
            let bitmap_offset = 6 + 10 + 7 + 4 + 24;
            assert_eq!(&reply[bitmap_offset..bitmap_offset + 4], &[2, 0, 28, 0]);
            assert_eq!(
                &reply[bitmap_offset + 4..bitmap_offset + 6],
                &bpp.to_le_bytes()
            );
        }
        assert!(demand(&demand_with_depth(8)).is_err());
    }
}
