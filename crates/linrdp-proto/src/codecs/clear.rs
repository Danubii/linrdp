//! ClearCodec decoder adapted from ironrdp-graphics 0.9.0 (MIT).
//! Copyright IronRDP contributors; see LICENSE-IRONRDP-MIT.
//! Local changes: correct single-palette RLEX framing and reject invalid suites.
//!
//! ClearCodec is a mandatory lossless codec for EGFX that uses three-layer
//! compositing (residual BGR RLE, bands with V-bar caching, subcodecs) to
//! efficiently encode text, UI elements, and icons.

use ironrdp_graphics::clearcodec::{FullVBar, GlyphCache, GlyphEntry, ShortVBar, VBarCache};

/// Glyph cache size as u16 for index arithmetic. GLYPH_CACHE_SIZE=4000 fits in u16.
const GLYPH_CACHE_WRAP: u16 = 4_000;

use ironrdp_core::{DecodeResult, ReadCursor, ensure_size, invalid_field_err};
use ironrdp_pdu::codecs::clearcodec::{Band, ShortVBarCacheMiss};
use ironrdp_pdu::codecs::clearcodec::{
    ClearCodecBitmapStream, CompositePayload, FLAG_GLYPH_INDEX, SubcodecId, VBar,
    decode_residual_layer, decode_subcodec_layer,
};

/// ClearCodec decoder maintaining persistent cache state across frames.
pub struct ClearCodecDecoder {
    vbar_cache: VBarCache,
    glyph_cache: GlyphCache,
}

impl ClearCodecDecoder {
    pub fn new() -> Self {
        Self {
            vbar_cache: VBarCache::new(),
            glyph_cache: GlyphCache::new(),
        }
    }

    /// Decode a ClearCodec bitmap stream into BGRA pixel data.
    ///
    /// The output buffer is `width * height * 4` bytes in BGRA format.
    /// The caller is responsible for compositing the result onto the target
    /// surface at the destination rectangle.
    ///
    /// **Alpha contract:** ClearCodec is lossless on the three color channels
    /// (B, G, R) per MS-RDPEGFX 2.2.4.1. The wire format does not transmit
    /// alpha; this decoder fills the alpha byte of every output pixel with
    /// `0xFF` unconditionally. Callers that need to preserve alpha across the
    /// network must transport it separately.
    pub fn decode(&mut self, data: &[u8], width: u16, height: u16) -> DecodeResult<Vec<u8>> {
        let mut src = ReadCursor::new(data);
        let stream = ClearCodecBitmapStream::decode(&mut src)?;

        // Handle cache reset
        if stream.is_cache_reset() {
            self.vbar_cache.reset();
        }

        // Validate glyph index range per spec: 0..3999 inclusive
        if let Some(idx) = stream.glyph_index
            && idx >= GLYPH_CACHE_WRAP
        {
            return Err(invalid_field_err!(
                "glyphIndex",
                "glyph index out of range 0-3999"
            ));
        }

        let w = usize::from(width);
        let h = usize::from(height);
        let pixel_count = w
            .checked_mul(h)
            .ok_or_else(|| invalid_field_err!("dimensions", "width * height overflow"))?;

        // Handle glyph hit: return cached pixel data
        if stream.is_glyph_hit() {
            let glyph_index = stream
                .glyph_index
                .ok_or_else(|| invalid_field_err!("flags", "GLYPH_HIT without GLYPH_INDEX"))?;
            let entry = self
                .glyph_cache
                .get(glyph_index)
                .ok_or_else(|| invalid_field_err!("glyphIndex", "glyph cache miss on hit"))?;
            // Glyphs are linear pixel streams; equal-area rectangles may reshape them.
            // MS-RDPEGFX 4.1.1.5 explicitly permits 2x8 -> 4x4 -> 8x2.
            if entry.pixels.len() != pixel_count * 4 {
                return Err(invalid_field_err!(
                    "glyphIndex",
                    "cached glyph pixel count mismatch"
                ));
            }
            return Ok(entry.pixels.clone());
        }

        // Cap allocation to prevent OOM from adversarial dimensions.
        // MS-RDPEGFX caps surfaces at 32767x32767; the spec does not
        // mandate a separate tile cap. We cap each tile dimension at
        // 8192 (supports 8K displays at 7680x4320 plus headroom). The
        // per-dimension form rather than a per-pixel-count form is
        // important because the original pixel-count cap (8192*8192
        // = 67M) accepted degenerate aspect ratios like 63961x771
        // (49M pixels, under cap) that allocate ~197MB from a few
        // attacker-controlled bytes. Capping each axis directly
        // rejects implausible tile shapes regardless of total area.
        const MAX_DECODE_DIM: u16 = 8192;
        if width == 0
            || height == 0
            || width > MAX_DECODE_DIM
            || height > MAX_DECODE_DIM
            || pixel_count > 16_777_216
        {
            return Err(invalid_field_err!(
                "dimensions",
                "width or height exceeds 8192-pixel decoder limit"
            ));
        }

        // Decode composite payload
        let mut output = vec![0u8; pixel_count * 4];

        if let Some(ref composite) = stream.composite {
            self.decode_composite(composite, &mut output, width, height)?;
        }

        // Store in glyph cache if applicable (area <= 1024 pixels)
        if stream.flags & FLAG_GLYPH_INDEX != 0
            && let Some(glyph_index) = stream.glyph_index
            && pixel_count <= 1024
        {
            self.glyph_cache.store(
                glyph_index,
                GlyphEntry {
                    width,
                    height,
                    pixels: output.clone(),
                },
            );
        }

        Ok(output)
    }

    fn decode_composite(
        &mut self,
        composite: &CompositePayload<'_>,
        output: &mut [u8],
        width: u16,
        _height: u16,
    ) -> DecodeResult<()> {
        let w = usize::from(width);

        // Layer 1: Residual (BGR RLE) - fills the entire output.
        // Cap pixel writes to the output buffer size to prevent CPU-spin DoS
        // from adversarial run_length values (FreeRDP CVE GHSA-32q9-m5qr-9j2v).
        if !composite.residual_data.is_empty() {
            let segments = decode_residual_layer(composite.residual_data)?;
            let max_offset = output.len();
            let mut offset = 0;
            for seg in &segments {
                let pixels_remaining = (max_offset.saturating_sub(offset)) / 4;
                let effective_run = u32::try_from(pixels_remaining)
                    .unwrap_or(u32::MAX)
                    .min(seg.run_length);
                for _ in 0..effective_run {
                    output[offset] = seg.blue;
                    output[offset + 1] = seg.green;
                    output[offset + 2] = seg.red;
                    output[offset + 3] = 0xFF; // Alpha
                    offset += 4;
                }
                if offset >= max_offset {
                    break;
                }
            }
        }

        // Layer 2: Bands (V-bar cached columns) - composite on top
        if !composite.bands_data.is_empty() {
            let bands = decode_bands_layer(composite.bands_data)?;
            for band in &bands {
                let band_height = band.y_end - band.y_start + 1;
                for (col_offset, vbar) in band.vbars.iter().enumerate() {
                    let x = usize::from(band.x_start) + col_offset;
                    if x >= w {
                        continue;
                    }

                    let full_vbar = self.resolve_vbar(
                        vbar,
                        band_height,
                        band.blue_bkg,
                        band.green_bkg,
                        band.red_bkg,
                    )?;

                    // Blit the full V-bar column into the output
                    let pixel_rows = full_vbar.pixels.len() / 3;
                    for row in 0..pixel_rows {
                        let y = usize::from(band.y_start) + row;
                        let dst_offset = (y * w + x) * 4;
                        let src_offset = row * 3;
                        if dst_offset + 3 < output.len() && src_offset + 2 < full_vbar.pixels.len()
                        {
                            output[dst_offset] = full_vbar.pixels[src_offset];
                            output[dst_offset + 1] = full_vbar.pixels[src_offset + 1];
                            output[dst_offset + 2] = full_vbar.pixels[src_offset + 2];
                            output[dst_offset + 3] = 0xFF;
                        }
                    }
                }
            }
        }

        // Layer 3: Subcodecs - composite on top
        if !composite.subcodec_data.is_empty() {
            let subcodecs = decode_subcodec_layer(composite.subcodec_data)?;
            for sub in &subcodecs {
                self.decode_subcodec_region(sub, output, width)?;
            }
        }

        Ok(())
    }

    fn resolve_vbar(
        &mut self,
        vbar: &VBar<'_>,
        band_height: u16,
        bg_blue: u8,
        bg_green: u8,
        bg_red: u8,
    ) -> DecodeResult<FullVBar> {
        match vbar {
            VBar::CacheHit { index } => {
                let cached = self
                    .vbar_cache
                    .get_vbar(*index)
                    .ok_or_else(|| invalid_field_err!("vbarIndex", "V-bar cache miss on hit"))?;
                Ok(cached.clone())
            }
            VBar::ShortCacheHit { index, y_on } => {
                let cached_short = self.vbar_cache.get_short_vbar(*index).ok_or_else(|| {
                    invalid_field_err!("shortVbarIndex", "short V-bar cache miss on hit")
                })?;
                // Create a modified short vbar with the y_on from this reference
                let modified = ShortVBar {
                    y_on: *y_on,
                    pixel_count: cached_short.pixel_count,
                    pixels: cached_short.pixels.clone(),
                };
                let full = VBarCache::reconstruct_full_vbar(
                    &modified,
                    band_height,
                    bg_blue,
                    bg_green,
                    bg_red,
                );
                // Store reconstructed full V-bar in cache
                self.vbar_cache.store_vbar(full.clone());
                Ok(full)
            }
            VBar::ShortCacheMiss(miss) => {
                let short = ShortVBar {
                    y_on: miss.y_on,
                    pixel_count: miss.y_off_delta,
                    pixels: miss.pixel_data.to_vec(),
                };
                // Store in short V-bar cache
                self.vbar_cache.store_short_vbar(short.clone());
                // Reconstruct and store full V-bar
                let full = VBarCache::reconstruct_full_vbar(
                    &short,
                    band_height,
                    bg_blue,
                    bg_green,
                    bg_red,
                );
                self.vbar_cache.store_vbar(full.clone());
                Ok(full)
            }
        }
    }

    #[expect(clippy::unused_self)]
    fn decode_subcodec_region(
        &self,
        sub: &ironrdp_pdu::codecs::clearcodec::Subcodec<'_>,
        output: &mut [u8],
        surface_width: u16,
    ) -> DecodeResult<()> {
        let sw = usize::from(surface_width);
        let sh = output.len() / (sw * 4).max(1);

        let x_end = usize::from(sub.x_start) + usize::from(sub.width);
        let y_end = usize::from(sub.y_start) + usize::from(sub.height);
        if x_end > sw || y_end > sh {
            return Err(invalid_field_err!(
                "subcodec",
                "region exceeds surface bounds"
            ));
        }

        match sub.codec_id {
            SubcodecId::Raw => {
                let w = usize::from(sub.width);
                let h = usize::from(sub.height);
                let expected =
                    w.checked_mul(h)
                        .and_then(|v| v.checked_mul(3))
                        .ok_or_else(|| {
                            invalid_field_err!("bitmapData", "raw subcodec dimensions overflow")
                        })?;
                if sub.bitmap_data.len() < expected {
                    return Err(invalid_field_err!(
                        "bitmapData",
                        "raw subcodec data too short"
                    ));
                }
                for row in 0..h {
                    for col in 0..w {
                        let x = usize::from(sub.x_start) + col;
                        let y = usize::from(sub.y_start) + row;
                        let src_idx = (row * w + col) * 3;
                        let dst_idx = (y * sw + x) * 4;
                        output[dst_idx] = sub.bitmap_data[src_idx];
                        output[dst_idx + 1] = sub.bitmap_data[src_idx + 1];
                        output[dst_idx + 2] = sub.bitmap_data[src_idx + 2];
                        output[dst_idx + 3] = 0xFF;
                    }
                }
            }
            SubcodecId::Rlex => {
                let rlex = decode_rlex(sub.bitmap_data)?;
                let w = usize::from(sub.width);
                let region_pixels = usize::from(sub.width) * usize::from(sub.height);
                let palette_len = rlex.palette.len();
                let mut px = 0usize;

                for seg in &rlex.segments {
                    if usize::from(seg.start_index) >= palette_len {
                        return Err(invalid_field_err!(
                            "rlex",
                            "start_index exceeds palette size"
                        ));
                    }
                    if usize::from(seg.stop_index) >= palette_len {
                        return Err(invalid_field_err!(
                            "rlex",
                            "stop_index exceeds palette size"
                        ));
                    }

                    let color = &rlex.palette[usize::from(seg.start_index)];
                    for _ in 0..seg.run_length {
                        if px >= region_pixels {
                            return Err(invalid_field_err!(
                                "rlex",
                                "run exceeds region pixel count"
                            ));
                        }
                        let x = usize::from(sub.x_start) + px % w;
                        let y = usize::from(sub.y_start) + px / w;
                        let dst_idx = (y * sw + x) * 4;
                        output[dst_idx] = color[0];
                        output[dst_idx + 1] = color[1];
                        output[dst_idx + 2] = color[2];
                        output[dst_idx + 3] = 0xFF;
                        px += 1;
                    }

                    for palette_idx in seg.start_index..=seg.stop_index {
                        if px >= region_pixels {
                            return Err(invalid_field_err!(
                                "rlex",
                                "suite exceeds region pixel count"
                            ));
                        }
                        let color = &rlex.palette[usize::from(palette_idx)];
                        let x = usize::from(sub.x_start) + px % w;
                        let y = usize::from(sub.y_start) + px / w;
                        let dst_idx = (y * sw + x) * 4;
                        output[dst_idx] = color[0];
                        output[dst_idx + 1] = color[1];
                        output[dst_idx + 2] = color[2];
                        output[dst_idx + 3] = 0xFF;
                        px += 1;
                    }
                }
                if px != region_pixels {
                    return Err(invalid_field_err!("rlex", "incomplete region"));
                }
            }
            SubcodecId::NsCodec => {
                let w = usize::from(sub.width);
                let h = usize::from(sub.height);
                let pixels = super::nsc::decode(sub.bitmap_data, w, h)
                    .map_err(|_| invalid_field_err!("nscodec", "invalid NSCodec subcodec"))?;
                for row in 0..h {
                    let dest =
                        ((usize::from(sub.y_start) + row) * sw + usize::from(sub.x_start)) * 4;
                    output[dest..dest + w * 4]
                        .copy_from_slice(&pixels[row * w * 4..(row + 1) * w * 4]);
                }
            }
        }

        Ok(())
    }
}

impl Default for ClearCodecDecoder {
    fn default() -> Self {
        Self::new()
    }
}

/// Every RLEX segment has one packed index/depth byte, including one-color palettes.
/// MS-RDPEGFX 2.2.4.1.1.3.1.3; the suite always emits its final palette entry.
fn decode_rlex(data: &[u8]) -> DecodeResult<ironrdp_pdu::codecs::clearcodec::RlexData> {
    use ironrdp_core::ensure_size;
    use ironrdp_pdu::codecs::clearcodec::{RlexData, RlexSegment};
    let mut src = ReadCursor::new(data);
    ensure_size!(in: src, size: 1);
    let count = src.read_u8();
    if count == 0 || count > 127 {
        return Err(invalid_field_err!("rlex", "invalid palette count"));
    }
    ensure_size!(in: src, size: usize::from(count) * 3);
    let mut palette = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        palette.push([src.read_u8(), src.read_u8(), src.read_u8()]);
    }
    let bits = 8 - (count - 1).leading_zeros();
    let mask = (1u16 << bits) - 1;
    let mut segments = Vec::new();
    while !src.is_empty() {
        ensure_size!(in: src, size: 2);
        let packed = src.read_u8();
        let stop = (u16::from(packed) & mask) as u8;
        let depth = packed >> bits;
        if stop >= count || depth > stop {
            return Err(invalid_field_err!("rlex", "invalid palette suite"));
        }
        let mut run = u32::from(src.read_u8());
        if run == 255 {
            ensure_size!(in: src, size: 2);
            run = u32::from(src.read_u16());
            if run == 65535 {
                ensure_size!(in: src, size: 4);
                run = src.read_u32();
            }
        }
        segments.push(RlexSegment {
            start_index: stop - depth,
            stop_index: stop,
            run_length: run,
        });
    }
    Ok(RlexData { palette, segments })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironrdp_pdu::codecs::clearcodec::{Subcodec, SubcodecId};

    #[test]
    fn single_palette_keeps_packed_byte_and_emits_run_plus_suite() {
        let bytes = [1, 10, 20, 30, 0, 3];
        let rlex = decode_rlex(&bytes).unwrap();
        assert_eq!(rlex.segments.len(), 1);
        assert_eq!(rlex.segments[0].run_length, 3);
        // Reproduces the upstream incompatibility without any desktop capture.
        assert_eq!(
            ironrdp_pdu::codecs::clearcodec::decode_rlex(&bytes)
                .unwrap()
                .segments
                .len(),
            2
        );
        let sub = Subcodec {
            x_start: 0,
            y_start: 0,
            width: 2,
            height: 2,
            codec_id: SubcodecId::Rlex,
            bitmap_data: &bytes,
        };
        let mut output = vec![0; 16];
        ClearCodecDecoder::new()
            .decode_subcodec_region(&sub, &mut output, 2)
            .unwrap();
        assert_eq!(output, [10, 20, 30, 255].repeat(4));
    }
    #[test]
    fn rejects_invalid_suite_truncation_and_incomplete_region() {
        assert!(decode_rlex(&[1, 10, 20, 30, 1, 0]).is_err());
        assert!(decode_rlex(&[1, 10, 20, 30, 0]).is_err());
        let sub = Subcodec {
            x_start: 0,
            y_start: 0,
            width: 2,
            height: 2,
            codec_id: SubcodecId::Rlex,
            bitmap_data: &[1, 10, 20, 30, 0, 2],
        };
        assert!(
            ClearCodecDecoder::new()
                .decode_subcodec_region(&sub, &mut [0; 16], 2)
                .is_err()
        );
    }
}

/// Decode all bands from the bands layer data.
fn decode_bands_layer<'a>(data: &'a [u8]) -> DecodeResult<Vec<Band<'a>>> {
    let mut bands = Vec::new();
    let mut src = ReadCursor::new(data);

    while !src.is_empty() {
        let band = decode_single_band(&mut src)?;
        bands.push(band);
    }

    Ok(bands)
}

fn decode_single_band<'a>(src: &mut ReadCursor<'a>) -> DecodeResult<Band<'a>> {
    ensure_size!(ctx: "ClearCodecBand", in: src, size: 11);

    let x_start = src.read_u16();
    let x_end = src.read_u16();
    let y_start = src.read_u16();
    let y_end = src.read_u16();
    let blue_bkg = src.read_u8();
    let green_bkg = src.read_u8();
    let red_bkg = src.read_u8();

    // Validate band height
    let height = y_end
        .checked_sub(y_start)
        .and_then(|h| h.checked_add(1))
        .ok_or_else(|| invalid_field_err!("yEnd", "yEnd < yStart"))?;

    if height > 52 {
        return Err(invalid_field_err!("bandHeight", "band height exceeds 52"));
    }

    if x_end < x_start {
        return Err(invalid_field_err!("xEnd", "xEnd < xStart"));
    }

    // `x_end - x_start` is at most u16::MAX (when x_end = u16::MAX and
    // x_start = 0), so the `+ 1` would overflow u16. Cast to usize first.
    let column_count = usize::from(x_end - x_start) + 1;
    let mut vbars = Vec::with_capacity(column_count);

    for _ in 0..column_count {
        let vbar = decode_vbar(src, height)?;
        vbars.push(vbar);
    }

    Ok(Band {
        x_start,
        x_end,
        y_start,
        y_end,
        blue_bkg,
        green_bkg,
        red_bkg,
        vbars,
    })
}

fn decode_vbar<'a>(src: &mut ReadCursor<'a>, band_height: u16) -> DecodeResult<VBar<'a>> {
    ensure_size!(ctx: "VBar", in: src, size: 2);
    let first_word = src.read_u16();

    // Top bit set: full V-bar cache hit
    if first_word & 0x8000 != 0 {
        let index = first_word & 0x7FFF;
        return Ok(VBar::CacheHit { index });
    }

    // Bit 14 set (bit 15 clear): short V-bar cache hit
    if first_word & 0x4000 != 0 {
        let index = first_word & 0x3FFF;
        ensure_size!(ctx: "ShortVBarCacheHit", in: src, size: 1);
        let y_on = src.read_u8();
        return Ok(VBar::ShortCacheHit { index, y_on });
    }

    // MS-RDPEGFX SHORT_VBAR_CACHE_MISS stores y_on in the first byte,
    // followed by the six-bit y_off. Fields do not straddle the byte boundary.
    let y_on = (first_word & 0xff) as u8;
    let y_off = ((first_word >> 8) & 0x3f) as u8;

    if y_off < y_on {
        return Err(invalid_field_err!(
            "shortVBarCacheMiss",
            "shortVBarYOff < shortVBarYOn"
        ));
    }

    if u16::from(y_off) > band_height {
        return Err(invalid_field_err!(
            "shortVBarCacheMiss",
            "shortVBarYOff exceeds band height"
        ));
    }

    let pixel_count = y_off - y_on;
    let pixel_byte_count = usize::from(pixel_count) * 3;
    ensure_size!(ctx: "ShortVBarCacheMiss", in: src, size: pixel_byte_count);
    let pixel_data = src.read_slice(pixel_byte_count);

    Ok(VBar::ShortCacheMiss(ShortVBarCacheMiss {
        y_on,
        y_off_delta: pixel_count,
        pixel_data,
    }))
}

#[cfg(test)]
mod band_tests {
    use super::*;
    #[test]
    fn short_vbar_miss_uses_little_endian_byte_fields_and_populates_caches() {
        let mut bytes = vec![0, 3]; // y_on=0, y_off=3; upstream misreads as12,0.
        bytes.extend([1, 2, 3, 4, 5, 6, 7, 8, 9]);
        let vbar = decode_vbar(&mut ReadCursor::new(&bytes), 3).unwrap();
        let mut decoder = ClearCodecDecoder::new();
        let full = decoder.resolve_vbar(&vbar, 3, 0, 0, 0).unwrap();
        assert_eq!(full.pixels, bytes[2..]);
        let hit = decoder
            .resolve_vbar(&VBar::CacheHit { index: 0 }, 3, 0, 0, 0)
            .unwrap();
        assert_eq!(hit.pixels, full.pixels);
        let short_hit = decoder
            .resolve_vbar(&VBar::ShortCacheHit { index: 0, y_on: 1 }, 4, 10, 11, 12)
            .unwrap();
        assert_eq!(
            short_hit.pixels,
            [vec![10, 11, 12], bytes[2..].to_vec()].concat()
        );
    }
    #[test]
    fn short_vbar_truncations_and_bad_offsets_fail() {
        assert!(decode_vbar(&mut ReadCursor::new(&[3, 2]), 3).is_err());
        assert!(decode_vbar(&mut ReadCursor::new(&[0, 4]), 3).is_err());
        assert!(decode_vbar(&mut ReadCursor::new(&[0, 3, 1, 2, 3]), 3).is_err());
        assert!(decode_bands_layer(&[0; 10]).is_err());
    }
}

#[cfg(test)]
mod nsc_tests {
    use super::*;
    #[test]
    fn nscodec_composites_into_clearcodec_rectangle_and_retains_row_order() {
        // Four raw NSCodec Y pixels, neutral Co/Cg, omitted opaque alpha.
        let mut nsc = Vec::new();
        for n in [4u32, 4, 4, 0] {
            nsc.extend(n.to_le_bytes());
        }
        nsc.extend([1, 0, 0, 0]);
        nsc.extend([10, 20, 30, 40]);
        nsc.extend([0; 8]);
        let mut sub = Vec::new();
        for n in [1u16, 1, 2, 2] {
            sub.extend(n.to_le_bytes());
        }
        sub.extend((nsc.len() as u32).to_le_bytes());
        sub.push(1); // NSCodec
        sub.extend(nsc);
        let mut clear = vec![0, 0];
        clear.extend(0u32.to_le_bytes()); // residual layer
        clear.extend(0u32.to_le_bytes()); // bands layer
        clear.extend((sub.len() as u32).to_le_bytes());
        clear.extend(sub);
        let pixels = ClearCodecDecoder::new().decode(&clear, 4, 4).unwrap();
        for (x, y, gray) in [(1usize, 1usize, 10), (2, 1, 20), (1, 2, 30), (2, 2, 40)] {
            let offset = (y * 4 + x) * 4;
            assert_eq!(&pixels[offset..offset + 4], &[gray, gray, gray, 255]);
        }
        assert_eq!(&pixels[..4], &[0; 4]);
    }
}

#[cfg(test)]
mod glyph_tests {
    use super::*;
    #[test]
    fn cached_glyph_can_be_reshaped_with_identical_pixel_count() {
        // Cache a 2x8 linear grayscale ramp in slot4 through a real cache-miss packet.
        let mut residual = Vec::new();
        let mut expected = Vec::new();
        for gray in 0u8..16 {
            residual.extend([gray, gray, gray, 1]);
            expected.extend([gray, gray, gray, 255]);
        }
        let mut miss = vec![1, 0, 4, 0];
        miss.extend((residual.len() as u32).to_le_bytes());
        miss.extend(0u32.to_le_bytes());
        miss.extend(0u32.to_le_bytes());
        miss.extend(residual);
        let mut decoder = ClearCodecDecoder::new();
        assert_eq!(decoder.decode(&miss, 2, 8).unwrap(), expected);
        let hit = [3, 1, 4, 0];
        assert_eq!(decoder.decode(&hit, 4, 4).unwrap(), expected);
        assert_eq!(decoder.decode(&hit, 8, 2).unwrap(), expected);
        assert_eq!(decoder.decode(&hit, 16, 1).unwrap(), expected);
        assert!(decoder.decode(&hit, 4, 3).is_err());
        assert!(decoder.decode(&hit, 4, 5).is_err());
    }
}
