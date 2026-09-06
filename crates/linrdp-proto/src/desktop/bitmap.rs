use super::*;

pub struct Framebuffer {
    pub width: u16,
    pub height: u16,
    pub pixels: Vec<u32>,
    pub updates: u64,
}
impl Framebuffer {
    pub(super) fn validate_size(width: u16, height: u16) -> Result<()> {
        if width == 0
            || height == 0
            || width > 8192
            || height > 8192
            || usize::from(width) * usize::from(height) > 16_777_216
        {
            return Err(bad("desktop or bitmap dimensions exceed limit"));
        }
        Ok(())
    }
    pub fn new(width: u16, height: u16) -> Result<Self> {
        Self::validate_size(width, height)?;
        Ok(Self {
            width,
            height,
            pixels: vec![0; usize::from(width) * usize::from(height)],
            updates: 0,
        })
    }
    pub fn update(&mut self, data: &[u8]) -> Result<()> {
        let mut r = Cursor(data);
        match r.u16()? {
            3 => {
                r.take(2)?;
                return r.end();
            }
            1 => {}
            _ => {
                return Err(bad(
                    "server sent unnegotiated drawing orders or palette output",
                ));
            }
        }
        let count = r.u16()?;
        if usize::from(count) > r.0.len() / 18 {
            return Err(bad("invalid bitmap rectangle count"));
        }
        let mut expanded_pixels = 0usize;
        for _ in 0..count {
            let left = r.u16()?;
            let top = r.u16()?;
            let right = r.u16()?;
            let bottom = r.u16()?;
            let width = r.u16()?;
            let height = r.u16()?;
            let bpp = r.u16()?;
            let flags = r.u16()?;
            let len = usize::from(r.u16()?);
            let mut bytes = Cursor(r.take(len)?);
            Self::validate_size(width, height)?;
            expanded_pixels += usize::from(width) * usize::from(height);
            if expanded_pixels > 16_777_216 {
                return Err(bad("bitmap update expansion exceeds limit"));
            }
            if bpp != 16
                || flags & !0x0401 != 0
                || left > right
                || top > bottom
                || right >= self.width
                || bottom >= self.height
                || right - left + 1 > width
                || bottom - top + 1 > height
            {
                return Err(bad("invalid bitmap rectangle bounds, flags or color depth"));
            }
            let compressed = flags & 1 != 0;
            let mut decoded = Vec::new();
            let stride;
            let pixels = if compressed {
                if flags & 0x0400 == 0 {
                    if bytes.u16()? != 0 {
                        return Err(bad("invalid bitmap compression header"));
                    }
                    let body = usize::from(bytes.u16()?);
                    bytes.u16()?;
                    bytes.u16()?;
                    if body != bytes.0.len() {
                        return Err(bad("bitmap compression length mismatch"));
                    }
                }
                ironrdp_graphics::rle::decompress_16_bpp(
                    bytes.0,
                    &mut decoded,
                    usize::from(width),
                    usize::from(height),
                )
                .map_err(|e| Error(format!("invalid compressed bitmap: {e}")))?;
                stride = usize::from(width) * 2;
                if decoded.len() != stride * usize::from(height) {
                    return Err(bad("decompressed bitmap size mismatch"));
                }
                decoded.as_slice()
            } else {
                if flags != 0 {
                    return Err(bad("compression flags on raw bitmap"));
                }
                stride = (usize::from(width) * 2).div_ceil(4) * 4;
                if bytes.0.len() != stride * usize::from(height) {
                    return Err(bad("raw bitmap size mismatch"));
                }
                bytes.0
            };
            for y in 0..usize::from(bottom - top + 1) {
                let row = (usize::from(height) - 1 - y) * stride;
                let dest = (usize::from(top) + y) * usize::from(self.width) + usize::from(left);
                for x in 0..usize::from(right - left + 1) {
                    let off = row + x * 2;
                    let n = u16::from_le_bytes([pixels[off], pixels[off + 1]]);
                    let red = u32::from((n >> 11) & 31);
                    let green = u32::from((n >> 5) & 63);
                    let blue = u32::from(n & 31);
                    self.pixels[dest + x] = (((red << 3) | (red >> 2)) << 16)
                        | (((green << 2) | (green >> 4)) << 8)
                        | (blue << 3)
                        | (blue >> 2);
                }
            }
            self.updates = self.updates.saturating_add(1);
        }
        r.end()
    }
}
