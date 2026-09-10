use crate::{PixelFormat, Rect, VncError, VncEvent};
use std::future::Future;
use tokio::io::{AsyncRead, AsyncReadExt};
use tracing::error;

use super::{uninit_vec, zlib::ZlibReader};

fn read_run_length(reader: &mut ZlibReader, remaining: usize) -> Result<usize, VncError> {
    let mut run_length_part;
    let mut run_length = 1;
    loop {
        run_length_part = reader.read_u8()?;
        run_length += run_length_part as usize;
        if run_length > remaining {
            return Err(VncError::InvalidImageData);
        }
        if 255 != run_length_part {
            break;
        }
    }
    Ok(run_length)
}

fn copy_true_color(
    reader: &mut ZlibReader,
    pixels: &mut Vec<u8>,
    pad: bool,
    compressed_bpp: usize,
    bpp: usize,
) -> Result<(), VncError> {
    let mut buf = [255; 4];
    std::io::Read::read_exact(
        reader,
        &mut buf[pad as usize..pad as usize + compressed_bpp],
    )?;
    pixels.extend_from_slice(&buf[..bpp]);
    Ok(())
}

fn copy_indexed(
    palette: &[u8],
    pixels: &mut Vec<u8>,
    bpp: usize,
    index: u8,
) -> Result<(), VncError> {
    let start = index as usize * bpp;
    let color = palette
        .get(start..start + bpp)
        .ok_or(VncError::InvalidImageData)?;
    pixels.extend_from_slice(color);
    Ok(())
}

pub struct Decoder {
    decompressor: Option<flate2::Decompress>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    fn format() -> PixelFormat {
        PixelFormat::try_from([32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0]).unwrap()
    }

    #[tokio::test]
    async fn palette_row_padding_and_valid_runs_decode_exactly() {
        let mut cases = Vec::new();
        for (palette_size, indices, expected) in [
            (2u8, vec![0x40, 0xa0], vec![0, 1, 0, 1, 0, 1]),
            (3, vec![0x18, 0x90], vec![0, 1, 2, 2, 1, 0]),
            (5, vec![0x01, 0x20, 0x43, 0x20], vec![0, 1, 2, 4, 3, 2]),
        ] {
            let mut payload = vec![palette_size];
            for i in 0..palette_size {
                payload.extend_from_slice(&[i, i, i]);
            }
            payload.extend(indices);
            cases.push((payload, expected));
        }
        cases.push((vec![128, 7, 7, 7, 5], vec![7; 6]));
        cases.push((
            vec![130, 0, 0, 0, 1, 1, 1, 128, 2, 129, 2],
            vec![0, 0, 0, 1, 1, 1],
        ));
        for (payload, expected) in cases {
            let mut encoder = flate2::Compress::new(flate2::Compression::fast(), true);
            let bytes = packet(&mut encoder, &payload);
            Decoder::new()
                .decode(
                    &format(),
                    &Rect {
                        x: 0,
                        y: 0,
                        width: 3,
                        height: 2,
                    },
                    &mut bytes.as_slice(),
                    &|event| {
                        let VncEvent::RawImage(_, actual) = event else {
                            panic!()
                        };
                        let expected: Vec<u8> =
                            expected.iter().flat_map(|n| [*n, *n, *n, 255]).collect();
                        assert_eq!(actual, expected);
                        std::future::ready(Ok(()))
                    },
                )
                .await
                .unwrap();
        }
    }
    fn packet(compressor: &mut flate2::Compress, data: &[u8]) -> Vec<u8> {
        let mut compressed = Vec::with_capacity(data.len() * 2 + 128);
        compressor
            .compress_vec(data, &mut compressed, flate2::FlushCompress::Sync)
            .unwrap();
        let mut packet = (compressed.len() as u32).to_be_bytes().to_vec();
        packet.extend(compressed);
        packet
    }
    #[tokio::test]
    async fn persistent_stream_and_tile_edges_preserve_pixels() {
        let mut encoder = flate2::Compress::new(flate2::Compression::fast(), true);
        let mut decoder = Decoder::new();
        for color in [[1, 2, 3], [4, 5, 6]] {
            let packet = packet(&mut encoder, &[1, color[0], color[1], color[2], 1, 9, 8, 7]);
            let events = Mutex::new(Vec::new());
            decoder
                .decode(
                    &format(),
                    &Rect {
                        x: 3,
                        y: 5,
                        width: 65,
                        height: 2,
                    },
                    &mut packet.as_slice(),
                    &|event| {
                        events.lock().unwrap().push(event);
                        std::future::ready(Ok(()))
                    },
                )
                .await
                .unwrap();
            let events = events.into_inner().unwrap();
            assert_eq!(events.len(), 2);
            for (index, event) in events.into_iter().enumerate() {
                let VncEvent::RawImage(r, bytes) = event else {
                    panic!()
                };
                assert_eq!(
                    (r.x, r.y, r.width, r.height),
                    if index == 0 {
                        (3, 5, 64, 2)
                    } else {
                        (67, 5, 1, 2)
                    }
                );
                let expected = if index == 0 {
                    [color[0], color[1], color[2], 255]
                } else {
                    [9, 8, 7, 255]
                };
                assert_eq!(bytes, expected.repeat(usize::from(r.width) * 2));
            }
        }
    }
    #[tokio::test]
    async fn malformed_payloads_fail_without_panicking_or_emitting_tiles() {
        // Invalid packed palette index; oversized true-color run; oversized
        // indexed run; truncated raw tile. Each tile is only one pixel.
        for payload in [
            vec![3, 0, 0, 0, 1, 1, 1, 2, 2, 2, 0xc0],
            vec![128, 1, 2, 3, 1],
            vec![130, 1, 2, 3, 4, 5, 6, 128, 1],
            vec![0, 1],
        ] {
            let mut compressor = flate2::Compress::new(flate2::Compression::fast(), true);
            let bytes = packet(&mut compressor, &payload);
            assert!(Decoder::new()
                .decode(
                    &format(),
                    &Rect {
                        x: 0,
                        y: 0,
                        width: 1,
                        height: 1
                    },
                    &mut bytes.as_slice(),
                    &|_| {
                        panic!("invalid tile emitted");
                        #[allow(unreachable_code)]
                        std::future::ready(Ok(()))
                    }
                )
                .await
                .is_err());
        }
        let oversized = (64u32 * 1024 * 1024 + 1).to_be_bytes();
        assert!(Decoder::new()
            .decode(
                &format(),
                &Rect {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1
                },
                &mut oversized.as_slice(),
                &|_| std::future::ready(Ok(()))
            )
            .await
            .is_err());
    }

    #[tokio::test]
    async fn true_color_tiles_cross_buffer_boundaries() {
        let mut encoder = flate2::Compress::new(flate2::Compression::fast(), true);
        let mut decoder = Decoder::new();
        for round in 0..3u8 {
            let mut payload = Vec::new();
            for _ in 0..4 {
                payload.push(0);
                for n in 0..4096 {
                    payload.extend_from_slice(&[n as u8, round, (n / 64) as u8]);
                }
            }
            let bytes = packet(&mut encoder, &payload);
            let count = std::sync::atomic::AtomicUsize::new(0);
            decoder
                .decode(
                    &format(),
                    &Rect {
                        x: 0,
                        y: 0,
                        width: 128,
                        height: 128,
                    },
                    &mut bytes.as_slice(),
                    &|event| {
                        let VncEvent::RawImage(_, pixels) = event else {
                            panic!()
                        };
                        for (n, p) in pixels.chunks_exact(4).enumerate() {
                            assert_eq!(p, [n as u8, round, (n / 64) as u8, 255]);
                        }
                        count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        std::future::ready(Ok(()))
                    },
                )
                .await
                .unwrap();
            assert_eq!(count.load(std::sync::atomic::Ordering::Relaxed), 4);
        }
    }
}

impl Decoder {
    pub fn new() -> Self {
        Self {
            decompressor: Some(flate2::Decompress::new(true)),
        }
    }

    pub async fn decode<S, F, Fut>(
        &mut self,
        format: &PixelFormat,
        rect: &Rect,
        input: &mut S,
        output_func: &F,
    ) -> Result<(), VncError>
    where
        S: AsyncRead + Unpin,
        F: Fn(VncEvent) -> Fut,
        Fut: Future<Output = Result<(), VncError>>,
    {
        if rect.width == 0
            || rect.height == 0
            || rect.width > 8192
            || rect.height > 8192
            || rect.x.checked_add(rect.width).is_none()
            || rect.y.checked_add(rect.height).is_none()
            || usize::from(rect.width) * usize::from(rect.height) > 16 * 1024 * 1024
            || ![8, 16, 32].contains(&format.bits_per_pixel)
            || [format.red_shift, format.green_shift, format.blue_shift]
                .iter()
                .any(|s| *s >= 32)
        {
            return Err(VncError::InvalidImageData);
        }
        let data_len = input.read_u32().await? as usize;
        if data_len > 64 * 1024 * 1024 {
            return Err(VncError::InvalidImageData);
        }
        let mut zlib_data = uninit_vec(data_len);
        input.read_exact(&mut zlib_data).await?;
        let decompressor = self.decompressor.take().unwrap();
        let mut reader = ZlibReader::new(decompressor, &zlib_data);

        let bpp = format.bits_per_pixel as usize / 8;
        let pixel_mask = ((format.red_max as u32) << format.red_shift)
            | ((format.green_max as u32) << format.green_shift)
            | ((format.blue_max as u32) << format.blue_shift);

        let (compressed_bpp, alpha_at_first) =
            if format.bits_per_pixel == 32 && format.true_color_flag > 0 && format.depth <= 24 {
                if pixel_mask & 0x000000ff == 0 {
                    // rgb at the most significant bits
                    // if format.big_endian_flag is set
                    // then decompressed data is excepted to be [rgb.0, rgb.1, rgb.2, alpha]
                    // otherwise the decompressed data should be [alpha, rgb.0, rgb.1, rgb.2]
                    (3, format.big_endian_flag == 0)
                } else if pixel_mask & 0xff000000 == 0 {
                    // rgb at the least significant bits
                    // if format.big_endian_flag is set
                    // then decompressed data should be [alpha, rgb.0, rgb.1, rgb.2]
                    // otherwise the decompressed data should be [rgb.0, rgb.1, rgb.2, alpha]
                    (3, format.big_endian_flag > 0)
                } else {
                    (4, false)
                }
            } else {
                (bpp, false)
            };
        let mut palette = Vec::with_capacity(128 * bpp);

        let mut y = 0;
        while y < rect.height {
            let height = if y + 64 > rect.height {
                rect.height - y
            } else {
                64
            };
            let mut x = 0;
            while x < rect.width {
                let width = if x + 64 > rect.width {
                    rect.width - x
                } else {
                    64
                };
                let pixel_count = height as usize * width as usize;

                let control = reader.read_u8()?;
                let is_rle = control & 0x80 > 0;
                let palette_size = control & 0x7f;
                palette.truncate(0);

                for _ in 0..palette_size {
                    copy_true_color(
                        &mut reader,
                        &mut palette,
                        alpha_at_first,
                        compressed_bpp,
                        bpp,
                    )?
                }

                let mut pixels = Vec::with_capacity(pixel_count * bpp);
                match (is_rle, palette_size) {
                    (false, 0) => {
                        // True Color pixels
                        for _ in 0..pixel_count {
                            copy_true_color(
                                &mut reader,
                                &mut pixels,
                                alpha_at_first,
                                compressed_bpp,
                                bpp,
                            )?
                        }
                    }
                    (false, 1) => {
                        // Color fill
                        for _ in 0..pixel_count {
                            copy_indexed(&palette, &mut pixels, bpp, 0)?
                        }
                    }
                    (false, 2..=16) => {
                        // Indexed pixels
                        let bits_per_index = match palette_size {
                            2 => 1,
                            3..=4 => 2,
                            5..=16 => 4,
                            _ => unreachable!(),
                        };
                        let mut encoded = reader.read_u8()?;
                        let mask = (1 << bits_per_index) - 1;

                        for y in 0..height {
                            let mut shift = 8 - bits_per_index;
                            for _ in 0..width {
                                if shift < 0 {
                                    shift = 8 - bits_per_index;
                                    encoded = reader.read_u8()?;
                                }
                                let idx = (encoded >> shift) & mask;

                                copy_indexed(&palette, &mut pixels, bpp, idx)?;
                                shift -= bits_per_index;
                            }
                            if shift < 8 - bits_per_index && y < height - 1 {
                                encoded = reader.read_u8()?;
                            }
                        }
                    }
                    (true, 0) => {
                        // True Color RLE
                        let mut count = 0;
                        let mut pixel = Vec::new();
                        while count < pixel_count {
                            pixel.truncate(0);
                            copy_true_color(
                                &mut reader,
                                &mut pixel,
                                alpha_at_first,
                                compressed_bpp,
                                bpp,
                            )?;
                            let run_length = read_run_length(&mut reader, pixel_count - count)?;
                            for _ in 0..run_length {
                                pixels.extend(&pixel)
                            }
                            count += run_length;
                        }
                    }
                    (true, 2..=127) => {
                        // Indexed RLE
                        let mut count = 0;
                        while count < pixel_count {
                            let control = reader.read_u8()?;
                            let longer_than_one = control & 0x80 > 0;
                            let index = control & 0x7f;
                            let run_length = if longer_than_one {
                                read_run_length(&mut reader, pixel_count - count)?
                            } else {
                                1
                            };
                            for _ in 0..run_length {
                                copy_indexed(&palette, &mut pixels, bpp, index)?;
                            }
                            count += run_length;
                        }
                    }
                    (x, y) => {
                        error!("ZRLE subencoding error {:?}", (x, y));
                        return Err(VncError::InvalidImageData);
                    }
                }
                output_func(VncEvent::RawImage(
                    Rect {
                        x: rect.x + x,
                        y: rect.y + y,
                        width,
                        height,
                    },
                    pixels,
                ))
                .await?;
                x += width;
            }
            y += height;
        }

        self.decompressor = Some(reader.into_inner()?);

        Ok(())
    }
}
