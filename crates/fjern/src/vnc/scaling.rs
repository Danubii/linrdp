//! Cached full-window bilinear scaling, matching minifb's fixed-point mapping.
use super::{Canvas, MAX_PIXELS, Result, invalid};

#[derive(Default)]
pub(super) struct Scaler {
    dimensions: (usize, usize, usize, usize),
    source: Vec<u32>,
    pixels: Vec<u32>,
    x: Vec<(usize, usize, u32)>,
    y: Vec<(usize, usize, u32)>,
    dirty: Vec<bool>,
    rendered: usize,
}

fn axis(source: usize, target: usize) -> Vec<(usize, usize, u32)> {
    let step = if target > 1 {
        ((source - 1) << 16) / (target - 1)
    } else {
        0
    };
    (0..target)
        .map(|n| {
            let fixed = n * step;
            let at = fixed >> 16;
            (at, (at + 1).min(source - 1), ((fixed >> 8) & 255) as u32)
        })
        .collect()
}

fn blend(a: u32, b: u32, c: u32, d: u32, fx: u32, fy: u32) -> u32 {
    let weights = [
        (256 - fx) * (256 - fy),
        fx * (256 - fy),
        (256 - fx) * fy,
        fx * fy,
    ];
    let mut pixel = 0;
    for shift in [0, 8, 16, 24] {
        let value = (((a >> shift) & 255) * weights[0]
            + ((b >> shift) & 255) * weights[1]
            + ((c >> shift) & 255) * weights[2]
            + ((d >> shift) & 255) * weights[3]
            + 32768)
            >> 16;
        pixel |= value << shift;
    }
    pixel
}

impl Scaler {
    pub(super) fn render(&mut self, canvas: &Canvas, size: (usize, usize)) -> Result<&[u32]> {
        let (w, h) = size;
        if w == 0 || h == 0 || w > 8192 || h > 8192 || w * h > MAX_PIXELS {
            return Err(invalid("VNC window exceeds supported dimensions"));
        }
        let dimensions = (canvas.width, canvas.height, w, h);
        let reset = dimensions != self.dimensions;
        if reset {
            self.dimensions = dimensions;
            self.source.resize(canvas.pixels.len(), 0);
            self.pixels.resize(w * h, 0);
            self.x = axis(canvas.width, w);
            self.y = axis(canvas.height, h);
            self.dirty.resize(canvas.height, true);
        }
        for (y, (old, new)) in self
            .source
            .chunks_mut(canvas.width)
            .zip(canvas.pixels.chunks(canvas.width))
            .enumerate()
        {
            self.dirty[y] = reset || old != new;
            if self.dirty[y] {
                old.copy_from_slice(new);
            }
        }
        self.rendered = 0;
        for (row, &(y, ny, fy)) in self.y.iter().enumerate() {
            // Both interpolation neighbors participate in invalidation.
            if !self.dirty[y] && !self.dirty[ny] {
                continue;
            }
            let top = &self.source[y * canvas.width..(y + 1) * canvas.width];
            let bottom = &self.source[ny * canvas.width..(ny + 1) * canvas.width];
            for (dst, &(x, nx, fx)) in self.pixels[row * w..(row + 1) * w].iter_mut().zip(&self.x) {
                *dst = blend(top[x], top[nx], bottom[x], bottom[nx], fx, fy);
            }
            self.rendered += w;
        }
        Ok(&self.pixels)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Independent scalar reference using the original fixed-point equations.
    fn reference(c: &Canvas, (w, h): (usize, usize)) -> Vec<u32> {
        let sx = if w > 1 {
            ((c.width - 1) << 16) / (w - 1)
        } else {
            0
        };
        let sy = if h > 1 {
            ((c.height - 1) << 16) / (h - 1)
        } else {
            0
        };
        (0..h)
            .flat_map(|row| {
                (0..w).map(move |col| {
                    let x = col * sx;
                    let y = row * sy;
                    let fx = (x >> 8) & 255;
                    let fy = (y >> 8) & 255;
                    let mut out = 0;
                    for shift in [0, 8, 16, 24] {
                        let sample = |xx: usize, yy: usize| {
                            ((c.pixels[yy * c.width + xx] >> shift) & 255) as usize
                        };
                        let a = x >> 16;
                        let b = y >> 16;
                        let nx = (a + 1).min(c.width - 1);
                        let ny = (b + 1).min(c.height - 1);
                        let value = (sample(a, b) * (256 - fx) * (256 - fy)
                            + sample(nx, b) * fx * (256 - fy)
                            + sample(a, ny) * (256 - fx) * fy
                            + sample(nx, ny) * fx * fy
                            + 32768)
                            >> 16;
                        out |= (value as u32) << shift;
                    }
                    out
                })
            })
            .collect()
    }
    #[test]
    fn cached_scaling_matches_reference_after_damage_and_resize() {
        let mut scaler = Scaler::default();
        for (sw, sh) in [(1, 1), (3, 5), (17, 9)] {
            let mut c = Canvas {
                width: sw,
                height: sh,
                pixels: (0..sw * sh)
                    .map(|n| (n as u32).wrapping_mul(0x1a2b3c4d))
                    .collect(),
            };
            for size in [(1, 1), (2, 7), (31, 13), (5, 3)] {
                assert_eq!(scaler.render(&c, size).unwrap(), reference(&c, size));
                scaler.render(&c, size).unwrap();
                assert_eq!(scaler.rendered, 0);
                c.pixels[(sh / 2) * sw] ^= 0xffffff;
                assert_eq!(scaler.render(&c, size).unwrap(), reference(&c, size));
            }
        }
    }
    #[test]
    fn small_damage_only_rescales_neighboring_rows() {
        let mut c = Canvas {
            width: 100,
            height: 100,
            pixels: vec![0; 10000],
        };
        let mut s = Scaler::default();
        s.render(&c, (200, 200)).unwrap();
        c.pixels[5050] = 0xffffff;
        assert_eq!(s.render(&c, (200, 200)).unwrap(), reference(&c, (200, 200)));
        assert!(s.rendered > 0 && s.rendered < 2000);
    }

    #[test]
    #[ignore = "release CPU benchmark"]
    fn benchmark_vnc_scaled_damage() {
        let mut c = Canvas {
            width: 1920,
            height: 1080,
            pixels: vec![0x123456; 1920 * 1080],
        };
        let size = (1280, 720);
        let mut s = Scaler::default();
        s.render(&c, size).unwrap();
        let mut samples = [Vec::new(), Vec::new()];
        for batch in 0..6 {
            for kind in if batch % 2 == 0 { [0, 1] } else { [1, 0] } {
                let start = std::time::Instant::now();
                for frame in 0..50 {
                    c.pixels[(frame * 19 % 1080) * 1920 + 10] ^= 0xffffff;
                    if kind == 0 {
                        std::hint::black_box(reference(&c, size));
                    } else {
                        std::hint::black_box(s.render(&c, size).unwrap());
                    }
                }
                samples[kind].push(start.elapsed().as_secs_f64() * 20.0);
                assert_eq!(s.render(&c, size).unwrap(), reference(&c, size));
            }
        }
        for times in &mut samples {
            times.sort_by(f64::total_cmp);
        }
        eprintln!(
            "1080p -> 720p sparse damage: full scalar reference {:.3} ms, cached {:.3} ms",
            samples[0][3], samples[1][3]
        );
    }
}
