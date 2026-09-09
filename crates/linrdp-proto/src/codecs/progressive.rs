//! Progressive RFX state management (MS-RDPEGFX 2.2.4.2).
//! Region DWT selection, first-pass signs, fixed-point color and difference
//! references are kept separate; none can be inferred from CONTEXT flags.
use crate::desktop::{Error, Result};
use ironrdp_graphics::{dwt, dwt_extrapolate, rlgr};
use ironrdp_pdu::codecs::rfx::{
    EntropyAlgorithm,
    progressive::{
        ComponentCodecQuant, ProgressiveBlock, ProgressiveRegion, ProgressiveTile,
        decode_progressive_stream,
    },
};
use std::collections::BTreeMap;
fn bad(s: impl Into<String>) -> Error {
    Error(s.into())
}
pub struct DecodedTile {
    pub region_index: usize,
    pub x_idx: u16,
    pub y_idx: u16,
    pub pixels: Vec<u8>,
}
struct Tile {
    coeff: Box<[[i16; 4096]; 3]>,
    sign: Box<[[i8; 4096]; 3]>,
    bits: [[u8; 10]; 3],
    ready: bool,
    extrapolate: bool,
    active_context: Option<u32>,
}
impl Default for Tile {
    fn default() -> Self {
        Self {
            coeff: Box::new([[0; 4096]; 3]),
            sign: Box::new([[0; 4096]; 3]),
            bits: [[0; 10]; 3],
            ready: false,
            extrapolate: false,
            active_context: None,
        }
    }
}
#[derive(Default)]
struct Context {
    flags: u8,
    tiles: BTreeMap<(u16, u16), Tile>,
}
#[derive(Default)]
pub struct ProgressiveDecoder {
    context: Context,
}
impl ProgressiveDecoder {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn has_reference(&self) -> bool {
        !self.context.tiles.is_empty()
    }
    pub fn delete_context(&mut self, id: u32) {
        for tile in self.context.tiles.values_mut() {
            if tile.active_context == Some(id) {
                tile.active_context = None;
            }
        }
    }
    pub fn decode_bitmap(
        &mut self,
        id: u32,
        width: u16,
        height: u16,
        data: &[u8],
    ) -> Result<Vec<DecodedTile>> {
        if width == 0
            || height == 0
            || width > 8192
            || height > 8192
            || width as usize * height as usize > 16_777_216
        {
            return Err(bad("progressive surface dimensions exceed limit"));
        }
        preflight(data)?;
        let blocks =
            decode_progressive_stream(data).map_err(|e| bad(format!("Progressive block: {e}")))?;
        // The current-frame DWT reference belongs to the surface, and must
        // survive changes/deletion of transient bitmap encoding contexts.
        let ctx = &mut self.context;
        let mut out = Vec::new();
        let mut region_index = 0;
        for block in blocks {
            match block {
                ProgressiveBlock::Context(c) => {
                    if c.flags & !1 != 0 {
                        return Err(bad("unsupported progressive context flags"));
                    }
                    ctx.flags = c.flags;
                }
                ProgressiveBlock::Region(region) => {
                    if region.flags & !1 != 0 {
                        return Err(bad("unsupported progressive region flags"));
                    }
                    let extrapolate = region.flags & 1 != 0;
                    for encoded in &region.tiles {
                        let (x, y) = match encoded {
                            ProgressiveTile::Simple(t) => (t.x_idx, t.y_idx),
                            ProgressiveTile::First(t) => (t.x_idx, t.y_idx),
                            ProgressiveTile::Upgrade(t) => (t.x_idx, t.y_idx),
                        };
                        if x as usize * 64 >= width as usize || y as usize * 64 >= height as usize {
                            return Err(bad("progressive tile outside surface"));
                        }
                        let tile = ctx.tiles.entry((x, y)).or_default();
                        match encoded {
                            ProgressiveTile::Simple(t) => first(
                                tile,
                                &region,
                                [t.quant_idx_y, t.quant_idx_cb, t.quant_idx_cr],
                                255,
                                t.flags,
                                [t.y_data, t.cb_data, t.cr_data],
                                extrapolate,
                            )?,
                            ProgressiveTile::First(t) => first(
                                tile,
                                &region,
                                [t.quant_idx_y, t.quant_idx_cb, t.quant_idx_cr],
                                t.quality,
                                t.flags,
                                [t.y_data, t.cb_data, t.cr_data],
                                extrapolate,
                            )?,
                            ProgressiveTile::Upgrade(t) => {
                                if !tile.ready {
                                    return Err(bad("progressive upgrade without reference tile"));
                                }
                                if tile.extrapolate != extrapolate {
                                    return Err(bad("progressive DWT mode changed during upgrade"));
                                }
                                let bits = bit_positions(
                                    &region,
                                    [t.quant_idx_y, t.quant_idx_cb, t.quant_idx_cr],
                                    t.quality,
                                )?;
                                let srl = [t.y_srl_data, t.cb_srl_data, t.cr_srl_data];
                                let raw = [t.y_raw_data, t.cb_raw_data, t.cr_raw_data];
                                for c in 0..3 {
                                    super::progressive_upgrade::upgrade(
                                        srl[c],
                                        raw[c],
                                        &tile.bits[c],
                                        &bits[c],
                                        extrapolate,
                                        &mut tile.coeff[c],
                                        &mut tile.sign[c],
                                    )?;
                                }
                                tile.bits = bits;
                            }
                        }
                        tile.active_context = Some(id);
                    }
                    // A region can expose pixels from tiles sent previously.
                    // Reconstruct only the tiles covered by this region, retaining
                    // its own clipping masks when handing them to the surface.
                    let mut visible = std::collections::BTreeSet::new();
                    for rect in &region.rects {
                        let right = rect.x as usize + rect.width as usize;
                        let bottom = rect.y as usize + rect.height as usize;
                        if right > width as usize || bottom > height as usize {
                            return Err(bad("progressive region outside surface"));
                        }
                        if rect.width == 0 || rect.height == 0 {
                            continue;
                        }
                        for y in rect.y as usize / 64..bottom.div_ceil(64) {
                            for x in rect.x as usize / 64..right.div_ceil(64) {
                                visible.insert((x as u16, y as u16));
                            }
                        }
                    }
                    if out.len() + visible.len() > 4096 {
                        return Err(bad("progressive reconstructed tile limit"));
                    }
                    for (x, y) in visible {
                        let tile =
                            ctx.tiles.get(&(x, y)).filter(|t| t.ready).ok_or_else(|| {
                                bad("progressive region without a reference tile")
                            })?;
                        out.push(DecodedTile {
                            region_index,
                            x_idx: x,
                            y_idx: y,
                            pixels: reconstruct(tile)?,
                        });
                    }
                    region_index += 1;
                }
                _ => {}
            }
        }
        Ok(out)
    }
}
fn preflight(mut data: &[u8]) -> Result<()> {
    if data.len() > 32 * 1024 * 1024 {
        return Err(bad("progressive stream exceeds limit"));
    }
    let mut blocks = 0;
    let mut tiles = 0usize;
    let mut rectangles = 0usize;
    while !data.is_empty() {
        blocks += 1;
        if blocks > 4096 || data.len() < 6 {
            return Err(bad("progressive block framing limit"));
        }
        let kind = u16::from_le_bytes(data[..2].try_into().unwrap());
        let len = u32::from_le_bytes(data[2..6].try_into().unwrap()) as usize;
        if len < 6 || len > data.len() {
            return Err(bad("truncated progressive block"));
        }
        if kind == 0xccc4 {
            if len < 18 {
                return Err(bad("truncated progressive region"));
            }
            rectangles += u16::from_le_bytes(data[7..9].try_into().unwrap()) as usize;
            tiles += u16::from_le_bytes(data[12..14].try_into().unwrap()) as usize;
            if tiles > 4096 || rectangles > 4096 {
                return Err(bad("progressive region allocation limit"));
            }
        }
        data = &data[len..];
    }
    Ok(())
}
fn bit_positions(
    region: &ProgressiveRegion<'_>,
    indices: [u8; 3],
    quality: u8,
) -> Result<[[u8; 10]; 3]> {
    let prog = if quality == 255 {
        [ComponentCodecQuant::LOSSLESS; 3]
    } else {
        let q = region
            .quant_prog_vals
            .get(quality as usize)
            .ok_or_else(|| bad("progressive quality index outside table"))?;
        [q.y_quant, q.cb_quant, q.cr_quant]
    };
    let mut bits = [[0; 10]; 3];
    for c in 0..3 {
        let base = region
            .quant_vals
            .get(indices[c] as usize)
            .ok_or_else(|| bad("progressive quantization index outside table"))?;
        for (band, bit) in bits[c].iter_mut().enumerate() {
            let p = prog[c].for_band(band);
            if p > 8 {
                return Err(bad("progressive quantization exceeds 8"));
            }
            let n = base.for_band(band) + p;
            if n == 0 || n > 23 {
                return Err(bad("invalid progressive bit position"));
            }
            *bit = n;
        }
    }
    Ok(bits)
}
fn first(
    tile: &mut Tile,
    region: &ProgressiveRegion<'_>,
    indices: [u8; 3],
    quality: u8,
    flags: u8,
    data: [&[u8]; 3],
    extrapolate: bool,
) -> Result<()> {
    // MS-RDPEGFX 2.2.4.2.1.5.4: the seven high flag bits MUST be ignored.
    let difference = flags & 1 != 0;
    if difference && (!tile.ready || tile.extrapolate != extrapolate) {
        return Err(bad(format!(
            "progressive difference without matching reference: ready={}, previous_dwt={}, incoming_dwt={extrapolate}",
            tile.ready, tile.extrapolate
        )));
    }
    let bits = bit_positions(region, indices, quality)?;
    let lengths = band_lengths(extrapolate);
    for c in 0..3 {
        let mut decoded = [0i16; 4096];
        rlgr::decode(EntropyAlgorithm::Rlgr1, data[c], &mut decoded)
            .map_err(|e| bad(format!("Progressive RLGR: {e}")))?;
        for (sign, value) in tile.sign[c].iter_mut().zip(&decoded) {
            *sign = value.signum() as i8;
        }
        let ll = 4096 - lengths[9];
        for i in ll + 1..4096 {
            decoded[i] = decoded[i].wrapping_add(decoded[i - 1]);
        }
        let mut pos = 0;
        for (band, len) in lengths.iter().enumerate() {
            for (target, source) in tile.coeff[c][pos..pos + len]
                .iter_mut()
                .zip(&decoded[pos..pos + len])
            {
                let value = source.wrapping_shl((bits[c][band] - 1) as u32);
                *target = if difference {
                    target.wrapping_add(value)
                } else {
                    value
                };
            }
            pos += len;
        }
    }
    tile.bits = bits;
    tile.ready = true;
    tile.extrapolate = extrapolate;
    Ok(())
}
fn band_lengths(extrapolate: bool) -> [usize; 10] {
    if extrapolate {
        [1023, 1023, 961, 272, 272, 256, 72, 72, 64, 81]
    } else {
        [1024, 1024, 1024, 256, 256, 256, 64, 64, 64, 64]
    }
}
fn reconstruct(tile: &Tile) -> Result<Vec<u8>> {
    let mut spatial = tile.coeff.clone();
    let mut scratch = [0i16; 4096];
    for c in 0..3 {
        if tile.extrapolate {
            dwt_extrapolate::decode(&mut spatial[c], &mut scratch);
        } else {
            dwt::decode(&mut spatial[c], &mut scratch);
        }
    }
    let mut pixels = vec![0; 4096 * 4];
    // RFX uses signed 11.5 fixed-point YCbCr. Widen before the offset and
    // matrix products so malformed extreme coefficients cannot overflow.
    for (i, pixel) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let y = (i64::from(spatial[0][i]) + 4096) * 65536;
        let cb = i64::from(spatial[1][i]);
        let cr = i64::from(spatial[2][i]);
        *pixel = [
            ((y + 91915 * cr) >> 21).clamp(0, 255) as u8,
            ((y - 46818 * cr - 22526 * cb) >> 21).clamp(0, 255) as u8,
            ((y + 115992 * cb) >> 21).clamp(0, 255) as u8,
            255,
        ];
    }
    Ok(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironrdp_pdu::codecs::rfx::{
        RfxRectangle,
        progressive::{ProgressiveContextPdu, TileSimple, encode_progressive_stream},
    };
    fn wire(extrapolate: bool, difference: bool) -> Vec<u8> {
        let mut coeff = [0i16; 4096];
        coeff[4096 - band_lengths(extrapolate)[9]] = 32;
        let mut y = vec![0; 32768];
        let len = rlgr::encode(EntropyAlgorithm::Rlgr1, &coeff, &mut y).unwrap();
        y.truncate(len);
        let mut chroma = vec![0; 32768];
        let len = rlgr::encode(EntropyAlgorithm::Rlgr1, &[0; 4096], &mut chroma).unwrap();
        chroma.truncate(len);
        let q = ComponentCodecQuant {
            ll3: 1,
            hl3: 1,
            lh3: 1,
            hh3: 1,
            hl2: 1,
            lh2: 1,
            hh2: 1,
            hl1: 1,
            lh1: 1,
            hh1: 1,
        };
        encode_progressive_stream(&[
            ProgressiveBlock::Context(ProgressiveContextPdu {
                context_id: 0,
                tile_size: 64,
                flags: 1,
            }),
            ProgressiveBlock::Region(ProgressiveRegion {
                tile_size: 64,
                flags: u8::from(extrapolate),
                rects: vec![RfxRectangle {
                    x: 0,
                    y: 0,
                    width: 64,
                    height: 64,
                }],
                quant_vals: vec![q],
                quant_prog_vals: vec![],
                tiles: vec![ProgressiveTile::Simple(TileSimple {
                    quant_idx_y: 0,
                    quant_idx_cb: 0,
                    quant_idx_cr: 0,
                    x_idx: 0,
                    y_idx: 0,
                    flags: 0x80 | u8::from(difference),
                    y_data: &y,
                    cb_data: &chroma,
                    cr_data: &chroma,
                    tail_data: &[],
                })],
            }),
        ])
        .unwrap()
    }
    #[test]
    fn golden_fixed_point_and_difference_reference() {
        for extrapolate in [false, true] {
            let mut decoder = ProgressiveDecoder::new();
            for (difference, gray) in [(false, 129), (true, 130), (false, 129)] {
                let frames = decoder
                    .decode_bitmap(1, 64, 64, &wire(extrapolate, difference))
                    .unwrap();
                assert_eq!(frames.len(), 1);
                for pixel in frames[0].pixels.as_chunks::<4>().0 {
                    assert_eq!(
                        *pixel,
                        [gray, gray, gray, 255],
                        "extrapolate={extrapolate},difference={difference}"
                    );
                }
            }
        }
    }
    #[test]
    fn golden_first_upgrade_preserves_cumulative_difference_reference() {
        use ironrdp_pdu::codecs::rfx::progressive::{
            ProgressiveCodecQuant, TileFirst, TileUpgrade,
        };
        for extrapolate in [false, true] {
            let source = wire(extrapolate, false);
            let mut blocks = decode_progressive_stream(&source).unwrap();
            let region = blocks
                .iter_mut()
                .find_map(|b| {
                    if let ProgressiveBlock::Region(r) = b {
                        Some(r)
                    } else {
                        None
                    }
                })
                .unwrap();
            let mut coeff = [0i16; 4096];
            coeff[4096 - band_lengths(extrapolate)[9]] = 1;
            let mut y = vec![0; 32768];
            let n = rlgr::encode(EntropyAlgorithm::Rlgr1, &coeff, &mut y).unwrap();
            y.truncate(n);
            let chroma = match &region.tiles[0] {
                ProgressiveTile::Simple(t) => t.cb_data,
                _ => unreachable!(),
            };
            region.quant_vals[0] = ComponentCodecQuant {
                ll3: 6,
                hl3: 6,
                lh3: 6,
                hh3: 6,
                hl2: 6,
                lh2: 6,
                hh2: 6,
                hl1: 6,
                lh1: 6,
                hh1: 6,
            };
            let mut prog = ComponentCodecQuant::LOSSLESS;
            prog.ll3 = 1;
            region.quant_prog_vals = vec![ProgressiveCodecQuant {
                quality: 50,
                y_quant: prog,
                cb_quant: ComponentCodecQuant::LOSSLESS,
                cr_quant: ComponentCodecQuant::LOSSLESS,
            }];
            region.tiles = vec![ProgressiveTile::First(TileFirst {
                quant_idx_y: 0,
                quant_idx_cb: 0,
                quant_idx_cr: 0,
                x_idx: 0,
                y_idx: 0,
                flags: 0,
                quality: 0,
                y_data: &y,
                cb_data: chroma,
                cr_data: chroma,
                tail_data: &[],
            })];
            let first = encode_progressive_stream(&blocks).unwrap();
            let region = blocks
                .iter_mut()
                .find_map(|b| {
                    if let ProgressiveBlock::Region(r) = b {
                        Some(r)
                    } else {
                        None
                    }
                })
                .unwrap();
            if let ProgressiveTile::First(t) = &mut region.tiles[0] {
                t.flags = 1;
            }
            let difference = encode_progressive_stream(&blocks).unwrap();
            let count = band_lengths(extrapolate)[9];
            let mut raw = vec![255; count.div_ceil(8)];
            if !count.is_multiple_of(8) {
                *raw.last_mut().unwrap() <<= 8 - count % 8;
            }
            let region = blocks
                .iter_mut()
                .find_map(|b| {
                    if let ProgressiveBlock::Region(r) = b {
                        Some(r)
                    } else {
                        None
                    }
                })
                .unwrap();
            region.tiles = vec![ProgressiveTile::Upgrade(TileUpgrade {
                quant_idx_y: 0,
                quant_idx_cb: 0,
                quant_idx_cr: 0,
                x_idx: 0,
                y_idx: 0,
                quality: 255,
                y_srl_data: &[],
                y_raw_data: &raw,
                cb_srl_data: &[],
                cb_raw_data: &[],
                cr_srl_data: &[],
                cr_raw_data: &[],
            })];
            let upgrade = encode_progressive_stream(&blocks).unwrap();
            for diff in [false, true] {
                let mut decoder = ProgressiveDecoder::new();
                decoder.decode_bitmap(1, 64, 64, &first).unwrap();
                if diff {
                    decoder.decode_bitmap(1, 64, 64, &difference).unwrap();
                }
                let decoded = decoder.decode_bitmap(1, 64, 64, &upgrade).unwrap();
                let gray = if diff { 133 } else { 131 };
                for pixel in decoded[0].pixels.as_chunks::<4>().0 {
                    assert_eq!(*pixel, [gray, gray, gray, 255]);
                }
            }
        }
    }
    #[test]
    fn difference_reference_survives_encoding_context_changes() {
        let mut decoder = ProgressiveDecoder::new();
        assert!(
            decoder
                .decode_bitmap(1, 64, 64, &wire(false, true))
                .is_err()
        );
        decoder
            .decode_bitmap(1, 64, 64, &wire(false, false))
            .unwrap();
        decoder.delete_context(1);
        let frames = decoder
            .decode_bitmap(2, 64, 64, &wire(false, true))
            .unwrap();
        assert_eq!(&frames[0].pixels[..4], &[130, 130, 130, 255]);
    }
}
