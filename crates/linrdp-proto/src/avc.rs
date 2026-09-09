//! Stateful, bounded AVC420 decoding. The wire payload is Annex B H.264.
use crate::desktop::{Error, Result};
use openh264::{decoder::Decoder, formats::YUVSource};
use std::collections::HashMap;

const MAX_SURFACES: usize = 64;
const MAX_INPUT: usize = 16 * 1024 * 1024;
const MAX_PIXELS: usize = 16_777_216;
const MAX_DIMENSION: usize = 8192;
const MAX_NATIVE_BUDGET: usize = 256 * 1024 * 1024;

pub struct Frame {
    pub width: usize,
    pub height: usize,
    /// Packed 0x00RRGGBB pixels, in row order.
    pub pixels: Vec<u32>,
}

#[derive(Default)]
pub struct DecoderPool {
    decoders: HashMap<u16, Decoder>,
    budgets: HashMap<u16, usize>,
    surface_bounds: HashMap<u16, (usize, usize)>,
}
impl DecoderPool {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn remove(&mut self, surface_id: u16) {
        self.decoders.remove(&surface_id);
        self.budgets.remove(&surface_id);
        self.surface_bounds.remove(&surface_id);
    }

    pub fn clear(&mut self) {
        self.decoders.clear();
        self.budgets.clear();
        self.surface_bounds.clear();
    }

    pub fn decode(&mut self, surface_id: u16, data: &[u8]) -> Result<Option<Frame>> {
        self.decode_bounded(surface_id, MAX_DIMENSION, MAX_DIMENSION, data)
    }

    /// Bind coded picture allocations to the advertised graphics surface before native decode.
    pub fn decode_bounded(
        &mut self,
        surface_id: u16,
        width: usize,
        height: usize,
        data: &[u8],
    ) -> Result<Option<Frame>> {
        let result = self.prepare_and_decode(surface_id, width, height, data);
        if result.is_err() {
            // Preflight errors also invalidate reference state.
            self.remove(surface_id);
        }
        result
    }
    fn prepare_and_decode(
        &mut self,
        surface_id: u16,
        width: usize,
        height: usize,
        data: &[u8],
    ) -> Result<Option<Frame>> {
        if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
            return Err(bad("invalid AVC surface dimensions"));
        }
        if self
            .surface_bounds
            .get(&surface_id)
            .is_some_and(|&bounds| bounds != (width, height))
        {
            // A changed surface starts a fresh stream; stale SPS cannot allocate against old bounds.
            self.remove(surface_id);
        }
        let budget = validate_annex_b(data, width, height)?;
        if let Some(budget) = budget {
            // Keep the high-water estimate: parameter sets/reference buffers may survive a smaller SPS.
            let budget = budget.max(self.budgets.get(&surface_id).copied().unwrap_or(0));
            let used: usize = self
                .budgets
                .iter()
                .filter(|(id, _)| **id != surface_id)
                .map(|(_, size)| *size)
                .sum();
            if budget > MAX_NATIVE_BUDGET || used > MAX_NATIVE_BUDGET - budget {
                return Err(bad("AVC reference-picture memory budget exceeded"));
            }
            self.budgets.insert(surface_id, budget);
        }
        if !self.decoders.contains_key(&surface_id) {
            if budget.is_none() {
                return Err(bad("AVC stream must begin with a sequence parameter set"));
            }
            if self.decoders.len() >= MAX_SURFACES {
                return Err(bad("too many AVC decoder surfaces"));
            }
            let decoder =
                Decoder::new().map_err(|e| bad(&format!("AVC decoder initialization: {e}")))?;
            self.decoders.insert(surface_id, decoder);
            self.surface_bounds.insert(surface_id, (width, height));
        }
        let frame = self.decode_inner(surface_id, data)?;
        if frame.as_ref().is_some_and(|f| {
            f.width > width.div_ceil(16) * 16 || f.height > height.div_ceil(32) * 32
        }) {
            return Err(bad("decoded AVC dimensions exceed the surface"));
        }
        Ok(frame)
    }

    fn decode_inner(&mut self, surface_id: u16, data: &[u8]) -> Result<Option<Frame>> {
        let decoder = self.decoders.get_mut(&surface_id).unwrap();
        let Some(yuv) = decoder
            .decode(data)
            .map_err(|e| bad(&format!("invalid AVC frame: {e}")))?
        else {
            return Ok(None);
        };
        let (width, height) = yuv.dimensions();
        dimensions(width, height)?;
        let pixels = rdp_rgb(&yuv);
        Ok(Some(Frame {
            width,
            height,
            pixels,
        }))
    }
}

/// MS-RDPEGFX 3.3.8.3.1 uses full-range BT.709, regardless of video-library
/// display defaults. OpenH264's write_rgb8 instead uses limited-range BT.601.
fn rdp_rgb(yuv: &impl YUVSource) -> Vec<u32> {
    let (width, height) = yuv.dimensions();
    let (ys, us, vs) = yuv.strides();
    let mut output = vec![0; width * height];
    for row in 0..height {
        let y = &yuv.y()[row * ys..row * ys + width];
        let u = &yuv.u()[row / 2 * us..];
        let v = &yuv.v()[row / 2 * vs..];
        let target = &mut output[row * width..(row + 1) * width];
        for x in 0..width {
            let luma = i32::from(y[x]) << 8;
            let cb = i32::from(u[x / 2]) - 128;
            let cr = i32::from(v[x / 2]) - 128;
            let red = ((luma + 403 * cr) >> 8).clamp(0, 255) as u32;
            let green = ((luma - 48 * cb - 120 * cr) >> 8).clamp(0, 255) as u32;
            let blue = ((luma + 475 * cb) >> 8).clamp(0, 255) as u32;
            target[x] = (red << 16) | (green << 8) | blue;
        }
    }
    output
}

fn bad(message: &str) -> Error {
    Error(message.into())
}
fn dimensions(width: usize, height: usize) -> Result<()> {
    if width == 0
        || height == 0
        || width > MAX_DIMENSION
        || height > MAX_DIMENSION
        || width.checked_mul(height).is_none_or(|n| n > MAX_PIXELS)
    {
        return Err(bad("AVC dimensions exceed the desktop limit"));
    }
    Ok(())
}

fn start_code(data: &[u8], from: usize) -> Option<(usize, usize)> {
    let mut i = from;
    while i + 3 <= data.len() {
        if data[i..].starts_with(&[0, 0, 1]) {
            return Some((i, 3));
        }
        if data[i..].starts_with(&[0, 0, 0, 1]) {
            return Some((i, 4));
        }
        i += 1;
    }
    None
}

fn validate_annex_b(data: &[u8], width: usize, height: usize) -> Result<Option<usize>> {
    if data.is_empty() || data.len() > MAX_INPUT {
        return Err(bad("invalid AVC payload size"));
    }
    let (mut start, mut prefix) =
        start_code(data, 0).ok_or_else(|| bad("AVC requires Annex B start codes"))?;
    if data[..start].iter().any(|b| *b != 0) {
        return Err(bad("unexpected bytes before AVC start code"));
    }
    let mut budget = None;
    let mut count = 0;
    loop {
        count += 1;
        if count > 4096 {
            return Err(bad("too many AVC NAL units"));
        }
        let next = start_code(data, start + prefix);
        let end = next.map_or(data.len(), |(position, _)| position);
        let nal = &data[start + prefix..end];
        if nal.is_empty() || nal[0] & 0x80 != 0 {
            return Err(bad("invalid AVC NAL header"));
        }
        match nal[0] & 0x1f {
            7 => {
                let info = validate_sps(&nal[1..])?;
                if info.width > width.div_ceil(16) * 16 || info.height > height.div_ceil(32) * 32 {
                    return Err(bad("AVC coded dimensions exceed the surface"));
                }
                // 4:2:0 DPB plus working pictures, motion vectors and alignment allowance.
                let estimate = info.width * info.height * (info.references + 6) * 3;
                budget = Some(budget.unwrap_or(0usize).saturating_add(estimate));
            }
            1 | 5 | 6 | 8 | 9 | 10 | 11 | 12 => {}
            _ => return Err(bad("unsupported AVC NAL unit type")),
        }
        let Some((position, length)) = next else {
            break;
        };
        start = position;
        prefix = length;
    }
    Ok(budget)
}

/// Read only a bounded SPS prefix: picture dimensions precede optional VUI data.
struct Sps {
    width: usize,
    height: usize,
    references: usize,
}
fn validate_sps(data: &[u8]) -> Result<Sps> {
    if data.len() > 4096 {
        return Err(bad("AVC sequence parameter set is too large"));
    }
    let mut rbsp = Vec::with_capacity(data.len());
    let mut zeros = 0;
    for &byte in data {
        if zeros == 2 && byte == 3 {
            zeros = 0;
            continue;
        }
        rbsp.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    let mut bits = Bits {
        bytes: &rbsp,
        offset: 0,
    };
    let profile = bits.read(8)?;
    bits.read(16)?; // constraints and level
    if bits.ue()? > 31 {
        return Err(bad("invalid AVC sequence parameter set ID"));
    }
    if matches!(
        profile,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    ) {
        if bits.ue()? != 1 || bits.ue()? != 0 || bits.ue()? != 0 {
            return Err(bad("AVC420 requires 8-bit 4:2:0 video"));
        }
        bits.read(1)?;
        if bits.read(1)? != 0 {
            for index in 0..8 {
                if bits.read(1)? != 0 {
                    let mut last = 8i64;
                    let mut next = 8i64;
                    for _ in 0..if index < 6 { 16 } else { 64 } {
                        if next != 0 {
                            next = (last + bits.se()? + 256).rem_euclid(256);
                        }
                        if next != 0 {
                            last = next;
                        }
                    }
                }
            }
        }
    } else if !matches!(profile, 66 | 77 | 88) {
        return Err(bad("unsupported AVC profile"));
    }
    if bits.ue()? > 12 {
        return Err(bad("invalid AVC frame number width"));
    }
    match bits.ue()? {
        0 => {
            if bits.ue()? > 12 {
                return Err(bad("invalid AVC picture order width"));
            }
        }
        1 => {
            bits.read(1)?;
            bits.se()?;
            bits.se()?;
            let cycle = bits.ue()?;
            if cycle > 255 {
                return Err(bad("invalid AVC picture order cycle"));
            }
            for _ in 0..cycle {
                bits.se()?;
            }
        }
        2 => {}
        _ => return Err(bad("invalid AVC picture order type")),
    }
    let references = bits.ue()? as usize;
    if references > 16 {
        return Err(bad("too many AVC reference pictures"));
    }
    bits.read(1)?;
    let width_mbs = u64::from(bits.ue()?) + 1;
    let height_maps = u64::from(bits.ue()?) + 1;
    let frame_only = bits.read(1)?;
    if frame_only == 0 {
        bits.read(1)?;
    }
    let width = width_mbs * 16;
    let height = height_maps * 16 * u64::from(2 - frame_only);
    if width > MAX_DIMENSION as u64
        || height > MAX_DIMENSION as u64
        || width * height > MAX_PIXELS as u64
    {
        return Err(bad("AVC coded dimensions exceed the desktop limit"));
    }
    dimensions(width as usize, height as usize)?;
    bits.read(1)?;
    if bits.read(1)? != 0 {
        let crop_x = (u64::from(bits.ue()?) + u64::from(bits.ue()?)) * 2;
        let crop_y =
            (u64::from(bits.ue()?) + u64::from(bits.ue()?)) * 2 * u64::from(2 - frame_only);
        if crop_x >= width || crop_y >= height {
            return Err(bad("invalid AVC picture crop"));
        }
    }
    Ok(Sps {
        width: width as usize,
        height: height as usize,
        references,
    })
}

struct Bits<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl Bits<'_> {
    fn read(&mut self, count: usize) -> Result<u32> {
        if count > 32 || self.offset + count > self.bytes.len() * 8 {
            return Err(bad("truncated AVC sequence parameter set"));
        }
        let mut value = 0;
        for _ in 0..count {
            value = (value << 1)
                | u32::from((self.bytes[self.offset / 8] >> (7 - self.offset % 8)) & 1);
            self.offset += 1;
        }
        Ok(value)
    }
    fn ue(&mut self) -> Result<u32> {
        let mut zeros = 0;
        while self.read(1)? == 0 {
            zeros += 1;
            if zeros > 31 {
                return Err(bad("AVC exponential Golomb value overflow"));
            }
        }
        Ok(((1u32 << zeros) - 1) + self.read(zeros)?)
    }
    fn se(&mut self) -> Result<i64> {
        let value = i64::from(self.ue()?);
        Ok(if value & 1 == 1 {
            (value + 1) / 2
        } else {
            -value / 2
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const IDR: &[u8] = include_bytes!("../tests/fixtures/avc/red-idr.h264");
    const P: &[u8] = include_bytes!("../tests/fixtures/avc/red-p.h264");

    fn red(frame: Frame) {
        assert_eq!((frame.width, frame.height), (32, 32));
        assert_eq!(frame.pixels.len(), 1024);
        for pixel in frame.pixels {
            assert_eq!(pixel >> 24, 0);
            assert!((pixel >> 16) & 255 >= 245, "{pixel:06x}");
            assert!((pixel >> 8) & 255 <= 8, "{pixel:06x}");
            assert!(pixel & 255 <= 8, "{pixel:06x}");
        }
    }
    #[test]
    fn decodes_idr_and_predictive_frame_with_persistent_state() {
        let mut pool = DecoderPool::new();
        red(pool.decode(1, IDR).unwrap().unwrap());
        red(pool.decode(1, P).unwrap().unwrap());
    }
    #[test]
    fn surfaces_have_independent_reference_state_and_cleanup() {
        let mut pool = DecoderPool::new();
        red(pool.decode(1, IDR).unwrap().unwrap());
        assert!(pool.decode(2, P).is_err());
        red(pool.decode(2, IDR).unwrap().unwrap());
        pool.remove(1);
        assert!(pool.decode(1, P).is_err());
        red(pool.decode(2, P).unwrap().unwrap());
        pool.clear();
        assert!(pool.decoders.is_empty());
        assert!(pool.decode(2, P).is_err());
    }
    #[test]
    fn rejects_malformed_or_oversized_payloads_before_decoder_creation() {
        let mut pool = DecoderPool::new();
        for input in [
            &[][..],
            &[1, 2, 3],
            &[0, 0, 1],
            &[0, 0, 1, 0xff],
            &[0, 0, 1, 0x67],
            &[0, 0, 1, 0x6f],
        ] {
            assert!(pool.decode(1, input).is_err());
            assert!(pool.decoders.is_empty());
        }
        assert!(pool.decode(1, &vec![0; MAX_INPUT + 1]).is_err());
        assert!(pool.decoders.is_empty());
    }
    #[test]
    fn surface_limits_and_aggregate_reference_budget_precede_native_decode() {
        let mut pool = DecoderPool::new();
        assert!(pool.decode_bounded(1, 4, 4, IDR).is_err());
        assert!(pool.decoders.is_empty());
        red(pool.decode_bounded(1, 32, 32, IDR).unwrap().unwrap());
        assert!(pool.decode_bounded(1, 32, 32, &[0, 0, 1, 0x67]).is_err());
        assert!(pool.decoders.is_empty());
        assert!(pool.budgets.is_empty());
        red(pool.decode_bounded(1, 32, 32, IDR).unwrap().unwrap());
        assert!(pool.decode_bounded(1, 16, 16, P).is_err());
        assert!(pool.decoders.is_empty());
        let mut packet = vec![0, 0, 1, 0x67];
        packet.extend(sps(256, 256, 16));
        assert!(pool.decode_bounded(1, 4096, 4096, &packet).is_err());
        assert!(pool.decoders.is_empty());
        // Simulate other live contexts consuming the pool's conservative native budget.
        pool.budgets.insert(2, MAX_NATIVE_BUDGET);
        assert!(pool.decode_bounded(1, 32, 32, IDR).is_err());
        assert!(pool.decoders.is_empty());
        pool.remove(2);
        red(pool.decode_bounded(1, 32, 32, IDR).unwrap().unwrap());
    }
    fn sps(width_mbs: u32, height_mbs: u32, refs: u32) -> Vec<u8> {
        fn ue(bits: &mut Vec<bool>, n: u32) {
            let n = u64::from(n) + 1;
            let len = 64 - n.leading_zeros();
            bits.extend(std::iter::repeat_n(false, len as usize - 1));
            for i in (0..len).rev() {
                bits.push(n & (1 << i) != 0);
            }
        }
        let mut bits = Vec::new();
        for b in [66u8, 0, 30] {
            for i in (0..8).rev() {
                bits.push(b & (1 << i) != 0);
            }
        }
        for n in [0, 0, 0, 0, refs] {
            ue(&mut bits, n);
        }
        bits.push(false);
        ue(&mut bits, width_mbs - 1);
        ue(&mut bits, height_mbs - 1);
        bits.extend([true, true, false, false, true]);
        while bits.len() % 8 != 0 {
            bits.push(false);
        }
        let rbsp: Vec<u8> = bits
            .as_chunks::<8>()
            .0
            .iter()
            .map(|bits| bits.iter().fold(0u8, |a, b| (a << 1) | u8::from(*b)))
            .collect();
        let mut escaped = vec![];
        let mut zeros = 0;
        for b in rbsp {
            if zeros == 2 && b <= 3 {
                escaped.push(3);
                zeros = 0;
            }
            escaped.push(b);
            zeros = if b == 0 { zeros + 1 } else { 0 };
        }
        escaped
    }
    #[test]
    fn validates_coded_dimensions_and_reference_budget_before_native_decode() {
        assert!(validate_sps(&sps(2, 2, 1)).is_ok());
        assert!(validate_sps(&sps(513, 1, 1)).is_err());
        assert!(validate_sps(&sps(512, 512, 1)).is_err());
        assert!(validate_sps(&sps(2, 2, 17)).is_err());
        let mut packet = vec![0, 0, 1, 0x67];
        packet.extend(sps(513, 1, 1));
        let mut pool = DecoderPool::new();
        assert!(pool.decode(1, &packet).is_err());
        assert!(pool.decoders.is_empty());
    }
    #[test]
    fn decoder_surface_count_is_bounded() {
        let mut pool = DecoderPool::new();
        for id in 0..MAX_SURFACES as u16 {
            red(pool.decode(id, IDR).unwrap().unwrap());
        }
        assert!(pool.decode(MAX_SURFACES as u16, IDR).is_err());
        pool.remove(0);
        red(pool.decode(MAX_SURFACES as u16, IDR).unwrap().unwrap());
    }
}

#[cfg(test)]
mod color_tests {
    use super::*;
    use openh264::formats::YUVSlices;
    #[test]
    fn rdp_full_range_709_preserves_black_white_and_normative_colors() {
        for (y, u, v, expected) in [
            (0, 128, 128, 0x000000),
            (255, 128, 128, 0xffffff),
            (16, 128, 128, 0x101010),
            (235, 128, 128, 0xebebeb),
            (53, 99, 255, 0xfc0000),
            (182, 29, 12, 0x00fe00),
            (17, 255, 116, 0x0000fc),
        ] {
            let ys = [y; 4];
            let us = [u];
            let vs = [v];
            let image = YUVSlices::new((&ys, &us, &vs), (2, 2), (2, 1, 1));
            assert_eq!(rdp_rgb(&image), [expected; 4]);
        }
    }
    #[test]
    fn conversion_respects_independent_padded_plane_strides() {
        let y = [0, 16, 99, 235, 255, 99];
        let u = [128, 99];
        let v = [128, 99, 99];
        let image = YUVSlices::new((&y, &u, &v), (2, 2), (3, 2, 3));
        assert_eq!(rdp_rgb(&image), [0, 0x101010, 0xebebeb, 0xffffff]);
    }
}
