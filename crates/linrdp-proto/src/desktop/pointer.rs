use super::*;

#[derive(Default)]
pub(super) struct Pointer {
    cache: [Option<Image>; 20],
    active: Option<usize>,
    position: Option<(u16, u16)>,
}
struct Image {
    width: usize,
    height: usize,
    hot_x: usize,
    hot_y: usize,
    xor: Vec<u8>,
    and: Vec<u8>,
}
impl Pointer {
    pub fn update(&mut self, bytes: &[u8]) -> Result<()> {
        let mut r = Cursor(bytes);
        let kind = r.u16()?;
        r.u16()?;
        match kind {
            1 => {
                let value = r.u32()?;
                if value != 0 && value != 0x7f00 {
                    return Err(bad("invalid system pointer"));
                }
                self.active = None;
            }
            3 => {
                self.position = Some((r.u16()?, r.u16()?));
            }
            6 => {
                let index = usize::from(r.u16()?);
                let hot_x = usize::from(r.u16()?);
                let hot_y = usize::from(r.u16()?);
                let width = usize::from(r.u16()?);
                let height = usize::from(r.u16()?);
                let and_len = usize::from(r.u16()?);
                let xor_len = usize::from(r.u16()?);
                if index >= 20
                    || width == 0
                    || height == 0
                    || width > 32
                    || height > 32
                    || hot_x >= width
                    || hot_y >= height
                    || xor_len != (width * 3).div_ceil(2) * 2 * height
                    || and_len != width.div_ceil(16) * 2 * height
                {
                    return Err(bad("invalid color pointer dimensions or cache slot"));
                }
                let xor = r.take(xor_len)?.to_vec();
                let and = r.take(and_len)?.to_vec();
                if r.0.len() == 1 {
                    r.byte()?;
                }
                self.cache[index] = Some(Image {
                    width,
                    height,
                    hot_x,
                    hot_y,
                    xor,
                    and,
                });
                self.active = Some(index);
            }
            7 => {
                let index = usize::from(r.u16()?);
                if index >= 20 || self.cache[index].is_none() {
                    return Err(bad("uncached remote pointer"));
                }
                self.active = Some(index);
            }
            _ => return Err(bad("unnegotiated pointer format")),
        }
        r.end()
    }
    pub fn overlay(&self, pixels: &mut [u32], width: usize, height: usize) {
        let (Some(index), Some((x, y))) = (self.active, self.position) else {
            return;
        };
        let Some(image) = &self.cache[index] else {
            return;
        };
        for py in 0..image.height {
            for px in 0..image.width {
                let dx = i32::from(x) + px as i32 - image.hot_x as i32;
                let dy = i32::from(y) + py as i32 - image.hot_y as i32;
                if dx < 0 || dy < 0 || dx >= width as i32 || dy >= height as i32 {
                    continue;
                }
                let row = image.height - 1 - py;
                let xor_offset = row * (image.width * 3).div_ceil(2) * 2 + px * 3;
                let rgb = &image.xor[xor_offset..xor_offset + 3];
                let rgb = u32::from(rgb[0]) | (u32::from(rgb[1]) << 8) | (u32::from(rgb[2]) << 16);
                let mask =
                    image.and[row * image.width.div_ceil(16) * 2 + px / 8] & (0x80 >> (px % 8));
                let at = dy as usize * width + dx as usize;
                pixels[at] = (if mask != 0 { pixels[at] } else { 0 }) ^ rgb;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn color_pointer_cache_masks_and_clipping() {
        let mut p = Pointer::default();
        // One red pixel; 24-bit XOR row padded to 2-byte boundary, AND row to 16 bits.
        p.update(&[
            6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 0, 2, 0, 4, 0, 0, 0, 255, 0, 0, 0,
        ])
        .unwrap();
        p.update(&[3, 0, 0, 0, 0, 0, 0, 0]).unwrap();
        let mut pixels = vec![0xffffff; 4];
        p.overlay(&mut pixels, 2, 2);
        assert_eq!(pixels, [0xff0000, 0xffffff, 0xffffff, 0xffffff]);
        p.update(&[7, 0, 0, 0, 0, 0]).unwrap();
        assert!(p.update(&[7, 0, 0, 0, 1, 0]).is_err());
        assert!(p.update(&[7, 0, 0, 0, 20, 0]).is_err());
        p.update(&[3, 0, 0, 0, 255, 255, 255, 255]).unwrap();
        p.overlay(&mut pixels, 2, 2);
        p.update(&[1, 0, 0, 0, 0, 0, 0, 0]).unwrap();
    }
}
