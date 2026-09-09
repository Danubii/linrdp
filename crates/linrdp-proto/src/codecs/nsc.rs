//! Checked MS-RDPNSC bitmap decoding for ClearCodec subcodec 1.
//! Plane sizes and RLE follow MS-RDPNSC 2.2.2; output is top-down BGRA.
use crate::desktop::{Error, Result};
const MAX_PIXELS: usize = 16 * 1024 * 1024;
fn bad() -> Error {
    Error("invalid or oversized NSCodec bitmap".into())
}
fn take<'a>(data: &mut &'a [u8], count: usize) -> Result<&'a [u8]> {
    let (head, rest) = data.split_at_checked(count).ok_or_else(bad)?;
    *data = rest;
    Ok(head)
}
fn plane(mut data: &[u8], size: usize) -> Result<Vec<u8>> {
    if data.len() == size {
        return Ok(data.to_vec());
    }
    if data.is_empty() || data.len() > size || size < 4 {
        return Err(bad());
    }
    let mut output = Vec::with_capacity(size);
    // The final four bytes are always literal EndData, even when identical.
    while output.len() < size - 4 {
        let value = take(&mut data, 1)?[0];
        if output.len() == size - 5 || data.first() != Some(&value) {
            output.push(value);
        } else {
            take(&mut data, 1)?;
            let first = take(&mut data, 1)?[0];
            let count = if first == 255 {
                u32::from_le_bytes(take(&mut data, 4)?.try_into().unwrap()) as usize
            } else {
                usize::from(first) + 2
            };
            if count < 2 || count > size - 4 - output.len() {
                return Err(bad());
            }
            output.resize(output.len() + count, value);
        }
    }
    output.extend_from_slice(take(&mut data, 4)?);
    if !data.is_empty() {
        return Err(bad());
    }
    Ok(output)
}
/// Decode one complete NSCodec stream, with explicit bounds supplied by its enclosing rectangle.
pub fn decode(mut data: &[u8], width: usize, height: usize) -> Result<Vec<u8>> {
    if width == 0
        || height == 0
        || width > 8192
        || height > 8192
        || width * height > MAX_PIXELS
        || data.len() > MAX_PIXELS * 4 + 20
    {
        return Err(bad());
    }
    let header = take(&mut data, 20)?;
    let counts: Vec<_> = header[..16]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|n| u32::from_le_bytes(*n) as usize)
        .collect();
    let loss = header[16];
    let subsampled = header[17];
    if !(1..=7).contains(&loss) || subsampled > 1 {
        return Err(bad());
    }
    // Reserved header bytes are ignored as specified.
    let y_stride = if subsampled != 0 {
        width.div_ceil(8) * 8
    } else {
        width
    };
    let chroma_size = if subsampled != 0 {
        y_stride / 2 * height.div_ceil(2)
    } else {
        width * height
    };
    let sizes = [y_stride * height, chroma_size, chroma_size, width * height];
    if (0..4).any(|i| counts[i] > sizes[i] || (i != 3 && counts[i] == 0))
        || counts.iter().sum::<usize>() != data.len()
    {
        return Err(bad());
    }
    let mut planes = Vec::with_capacity(4);
    for i in 0..4 {
        planes.push(if counts[i] == 0 {
            vec![255; sizes[i]]
        } else {
            plane(take(&mut data, counts[i])?, sizes[i])?
        });
    }
    let mut pixels = vec![0; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let p = y * width + x;
            let c = if subsampled != 0 {
                (y / 2) * (y_stride / 2) + x / 2
            } else {
                p
            };
            let luma = i16::from(planes[0][y * y_stride + x]);
            // Recover the signed 8-bit chroma, including the inverse transform's half shift.
            let co = i16::from((planes[1][c] << (loss - 1)) as i8);
            let cg = i16::from((planes[2][c] << (loss - 1)) as i8);
            pixels[p * 4] = (luma - co - cg).clamp(0, 255) as u8;
            pixels[p * 4 + 1] = (luma + cg).clamp(0, 255) as u8;
            pixels[p * 4 + 2] = (luma + co - cg).clamp(0, 255) as u8;
            pixels[p * 4 + 3] = planes[3][p];
        }
    }
    Ok(pixels)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn stream(planes: [&[u8]; 4], loss: u8, subsampled: u8) -> Vec<u8> {
        let mut data = Vec::new();
        for p in planes {
            data.extend((p.len() as u32).to_le_bytes());
        }
        data.extend([loss, subsampled, 0xa5, 0x5a]);
        for p in planes {
            data.extend(p);
        }
        data
    }
    #[test]
    fn raw_planes_preserve_order_signed_chroma_and_alpha() {
        let data = stream(
            [
                &[100, 200, 30, 40],
                &[10, 246, 0, 0],
                &[20, 236, 0, 0],
                &[1, 2, 3, 4],
            ],
            1,
            0,
        );
        assert_eq!(
            decode(&data, 2, 2).unwrap(),
            [
                70, 120, 90, 1, 230, 180, 210, 2, 30, 30, 30, 3, 40, 40, 40, 4
            ]
        );
    }
    #[test]
    fn color_loss_restores_signed_eight_bit_chroma_before_clamping() {
        let data = stream([&[99], &[34], &[55], &[]], 3, 0);
        assert_eq!(decode(&data, 1, 1).unwrap(), [255, 63, 15, 255]);
        let data = stream([&[128], &[1], &[3], &[]], 7, 0);
        assert_eq!(decode(&data, 1, 1).unwrap(), [128, 64, 255, 255]);
    }
    #[test]
    fn subsampling_uses_padded_rows_and_odd_height() {
        let y = [
            10, 20, 30, 99, 99, 99, 99, 99, 40, 50, 60, 99, 99, 99, 99, 99, 70, 80, 90, 99, 99, 99,
            99, 99,
        ];
        let co = [1, 2, 99, 99, 3, 4, 99, 99];
        let data = stream([&y, &co, &[0; 8], &[]], 2, 1);
        let actual = decode(&data, 3, 3).unwrap();
        let expected = [
            (10, 2),
            (20, 2),
            (30, 4),
            (40, 2),
            (50, 2),
            (60, 4),
            (70, 6),
            (80, 6),
            (90, 8),
        ];
        for (p, (y, c)) in actual.as_chunks::<4>().0.iter().zip(expected) {
            assert_eq!(*p, [y - c, y, y + c, 255]);
        }
    }
    #[test]
    fn short_long_runs_and_literal_tail() {
        assert_eq!(
            plane(&[9, 9, 4, 1, 2, 3, 4], 10).unwrap(),
            [9, 9, 9, 9, 9, 9, 1, 2, 3, 4]
        );
        let mut run = vec![7, 7, 255];
        run.extend(300u32.to_le_bytes());
        run.extend([1, 2, 3, 4]);
        let mut expected = vec![7; 300];
        expected.extend([1, 2, 3, 4]);
        assert_eq!(plane(&run, 304).unwrap(), expected);
        assert_eq!(
            plane(&[7, 7, 3, 8, 8, 8, 8, 8], 10).unwrap(),
            [7, 7, 7, 7, 7, 8, 8, 8, 8, 8]
        );
        let data = stream(
            [
                &[50, 50, 4, 50, 50, 50, 50],
                &[0, 0, 4, 0, 0, 0, 0],
                &[0, 0, 4, 0, 0, 0, 0],
                &[],
            ],
            1,
            0,
        );
        assert_eq!(decode(&data, 5, 2).unwrap(), [50, 50, 50, 255].repeat(10));
    }
    #[test]
    fn invalid_lengths_parameters_runs_and_truncations_are_rejected() {
        let valid = stream([&[1, 2, 3, 4], &[0; 4], &[0; 4], &[]], 1, 0);
        for n in 0..valid.len() {
            assert!(decode(&valid[..n], 2, 2).is_err());
        }
        for (i, value) in [(0, 255), (4, 0), (16, 0), (16, 8), (17, 2)] {
            let mut data = valid.clone();
            data[i] = value;
            assert!(decode(&data, 2, 2).is_err());
        }
        let mut extra = valid.clone();
        extra.push(0);
        assert!(decode(&extra, 2, 2).is_err());
        for (w, h) in [(0, 1), (1, 0), (8193, 1), (8192, 8192), (usize::MAX, 1)] {
            assert!(decode(&valid, w, h).is_err());
        }
        for bytes in [
            &[1, 1, 255, 255, 255, 255, 255][..],
            &[1, 1, 255, 0, 0, 0, 0],
            &[1, 1, 7, 0, 0, 0, 0],
            &[1, 1],
            &[1, 1, 0, 0],
        ] {
            assert!(plane(bytes, 10).is_err());
        }
    }
}
