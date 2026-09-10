//! Cached full-window bilinear scaling, matching minifb's fixed-point mapping.
use super::{Canvas, MAX_PIXELS, Result, invalid};

#[derive(Default)]
pub(super) struct Scaler {
    dimensions: (usize, usize, usize, usize),
    pixels: Vec<u32>,
    x: Vec<(usize, usize, u32)>,
    y: Vec<(usize, usize, u32)>,
    dirty: Vec<Option<(usize, usize)>>,
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
    // Two independent 32-bit lanes: each weighted channel fits in 24 bits,
    // so neither the sums nor rounding can carry into its neighbor.
    let pair = |a: u32, b: u32, c: u32, d: u32| {
        let lanes = |p: u32| u64::from(p & 255) | (u64::from((p >> 16) & 255) << 32);
        let sum = lanes(a) * u64::from(weights[0])
            + lanes(b) * u64::from(weights[1])
            + lanes(c) * u64::from(weights[2])
            + lanes(d) * u64::from(weights[3])
            + 0x0000_8000_0000_8000;
        let rounded = (sum >> 16) & 0x0000_00ff_0000_00ff;
        rounded as u32 | ((rounded >> 16) as u32 & 0x00ff_0000)
    };
    pair(a, b, c, d) | (pair(a >> 8, b >> 8, c >> 8, d >> 8) << 8)
}

impl Scaler {
    pub(super) fn damage(&mut self, x: usize, y: usize, width: usize, height: usize) {
        for row in self.dirty.iter_mut().skip(y).take(height) {
            *row = Some(match *row {
                Some((left, right)) => (left.min(x), right.max(x + width)),
                None => (x, x + width),
            });
        }
    }

    pub(super) fn render(&mut self, canvas: &Canvas, size: (usize, usize)) -> Result<&[u32]> {
        let (w, h) = size;
        if w == 0 || h == 0 || w > 8192 || h > 8192 || w * h > MAX_PIXELS {
            return Err(invalid("VNC window exceeds supported dimensions"));
        }
        let dimensions = (canvas.width, canvas.height, w, h);
        let reset = dimensions != self.dimensions;
        if reset {
            self.dimensions = dimensions;
            self.pixels.resize(w * h, 0);
            self.x = axis(canvas.width, w);
            self.y = axis(canvas.height, h);
            self.dirty = vec![Some((0, canvas.width)); canvas.height];
        }
        self.rendered = 0;
        for (row, &(y, ny, fy)) in self.y.iter().enumerate() {
            // Both interpolation neighbors participate in invalidation.
            let (left, right) = match (self.dirty[y], self.dirty[ny]) {
                (Some((a, b)), Some((c, d))) => (a.min(c), b.max(d)),
                (Some(span), None) | (None, Some(span)) => span,
                (None, None) => continue,
            };
            let start = self.x.partition_point(|&(_, nx, _)| nx < left);
            let end = self.x.partition_point(|&(x, _, _)| x < right);
            let top = &canvas.pixels[y * canvas.width..(y + 1) * canvas.width];
            let bottom = &canvas.pixels[ny * canvas.width..(ny + 1) * canvas.width];
            for (dst, &(x, nx, fx)) in self.pixels[row * w + start..row * w + end]
                .iter_mut()
                .zip(&self.x[start..end])
            {
                *dst = blend(top[x], top[nx], bottom[x], bottom[nx], fx, fy);
            }
            self.rendered += end - start;
        }
        self.dirty.fill(None);
        Ok(&self.pixels)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scalar_blend(a: u32, b: u32, c: u32, d: u32, fx: u32, fy: u32) -> u32 {
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

    #[test]
    fn packed_blend_matches_all_fraction_pairs() {
        for (a, b, c, d) in [
            (0, u32::MAX, 0x12345678, 0xfedcba98),
            (u32::MAX, u32::MAX, u32::MAX, u32::MAX),
            (0xff00ff00, 0x00ff00ff, 0xff00ff00, 0x00ff00ff),
        ] {
            for fx in 0..256 {
                for fy in 0..256 {
                    assert_eq!(blend(a, b, c, d, fx, fy), scalar_blend(a, b, c, d, fx, fy));
                }
            }
        }
    }

    #[test]
    #[ignore = "release CPU benchmark"]
    fn benchmark_vnc_blend() {
        let mut samples = [Vec::new(), Vec::new()];
        for batch in 0..6 {
            for kind in if batch % 2 == 0 { [0, 1] } else { [1, 0] } {
                let start = std::time::Instant::now();
                for n in 0..1920 * 1080u32 {
                    let (a, b, c, d, fx, fy) = std::hint::black_box((
                        n.wrapping_mul(1234567),
                        n,
                        !n,
                        0xffabcdef,
                        n & 255,
                        (n >> 8) & 255,
                    ));
                    std::hint::black_box(if kind == 0 {
                        scalar_blend(a, b, c, d, fx, fy)
                    } else {
                        blend(a, b, c, d, fx, fy)
                    });
                }
                samples[kind].push(start.elapsed().as_secs_f64() * 1000.0);
            }
        }
        for times in &mut samples {
            times.sort_by(f64::total_cmp);
        }
        eprintln!(
            "1080p blends: scalar {:.3} ms, paired {:.3} ms",
            samples[0][3], samples[1][3]
        );
    }
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
                scaler.damage(0, sh / 2, 1, 1);
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
        s.damage(50, 50, 1, 1);
        assert_eq!(s.render(&c, (200, 200)).unwrap(), reference(&c, (200, 200)));
        assert!(s.rendered > 0 && s.rendered <= 25);
    }

    #[test]
    fn accumulated_damage_copy_scroll_and_same_size_reset_match_reference() {
        let mut c = Canvas::default();
        c.resize(97, 71).unwrap();
        let mut scaler = Scaler::default();
        let size = (151, 103);
        scaler.render(&c, size).unwrap();
        for frame in 0..20 {
            for (x, y) in [(0, 0), (96, 70), (frame, frame + 9)] {
                c.pixels[y * c.width + x] ^= 0xabcdef;
                scaler.damage(x, y, 1, 1);
            }
            assert_eq!(scaler.render(&c, size).unwrap(), reference(&c, size));
            c.pixels.copy_within(97.., 0);
            scaler.damage(0, 0, 97, 70);
            assert_eq!(scaler.render(&c, size).unwrap(), reference(&c, size));
        }
        c.resize(97, 71).unwrap();
        scaler.damage(0, 0, 97, 71);
        assert_eq!(scaler.render(&c, size).unwrap(), reference(&c, size));
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
                    s.damage(10, frame * 19 % 1080, 1, 1);
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
