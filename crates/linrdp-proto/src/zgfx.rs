//! Bounded MS-RDPEGFX 3.1.9.1 RDP 8.0 bulk decompression.
//! Every decoded byte, including raw segments and runs, enters the shared history.
use crate::desktop::{Error, Result};
const HISTORY: usize = 2_500_000;
const SEGMENT: usize = 65_535;
const MESSAGE: usize = 16 * 1024 * 1024;
fn bad() -> Error {
    Error("invalid or oversized ZGFX stream".into())
}
fn take<'a>(b: &mut &'a [u8], n: usize) -> Result<&'a [u8]> {
    let (a, rest) = b.split_at_checked(n).ok_or_else(bad)?;
    *b = rest;
    Ok(a)
}
struct Bits<'a> {
    data: &'a [u8],
    at: usize,
    end: usize,
}
impl Bits<'_> {
    fn read(&mut self, n: usize) -> Result<usize> {
        if n > 24 || n > self.end - self.at {
            return Err(bad());
        }
        let mut value = 0;
        for _ in 0..n {
            value = (value << 1) | usize::from((self.data[self.at / 8] >> (7 - self.at % 8)) & 1);
            self.at += 1;
        }
        Ok(value)
    }
}
/// Persistent decoder. A malformed stream poisons this instance until it is replaced.
/// This prevents reuse of partially updated history after an error.
pub struct Decoder {
    history: Vec<u8>,
    position: usize,
    failed: bool,
}
impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}
impl Decoder {
    pub fn new() -> Self {
        Self {
            history: vec![0; HISTORY],
            position: 0,
            failed: false,
        }
    }
    pub fn decompress(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        if self.failed {
            return Err(bad());
        }
        let result = self.decode(data);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn decode(&mut self, mut data: &[u8]) -> Result<Vec<u8>> {
        if data.len() > MESSAGE {
            return Err(bad());
        }
        let descriptor = take(&mut data, 1)?[0];
        let mut output = Vec::new();
        match descriptor {
            0xe0 => self.segment(data, &mut output)?,
            0xe1 => {
                let count = u16::from_le_bytes(take(&mut data, 2)?.try_into().unwrap());
                let expected = u32::from_le_bytes(take(&mut data, 4)?.try_into().unwrap()) as usize;
                if count == 0 || expected > MESSAGE {
                    return Err(bad());
                }
                for _ in 0..count {
                    let size = u32::from_le_bytes(take(&mut data, 4)?.try_into().unwrap()) as usize;
                    self.segment(take(&mut data, size)?, &mut output)?;
                    if output.len() > expected {
                        return Err(bad());
                    }
                }
                if !data.is_empty() || output.len() != expected {
                    return Err(bad());
                }
            }
            _ => return Err(bad()),
        }
        Ok(output)
    }
    fn byte(&mut self, value: u8, output: &mut Vec<u8>) {
        self.history[self.position] = value;
        self.position = (self.position + 1) % HISTORY;
        output.push(value);
    }
    fn bytes(&mut self, data: &[u8], output: &mut Vec<u8>) {
        let first = data.len().min(HISTORY - self.position);
        self.history[self.position..self.position + first].copy_from_slice(&data[..first]);
        self.history[..data.len() - first].copy_from_slice(&data[first..]);
        self.position = (self.position + data.len()) % HISTORY;
        output.extend_from_slice(data);
    }
    fn room(output: &[u8], start: usize, count: usize) -> Result<()> {
        if count > SEGMENT - (output.len() - start) || count > MESSAGE - output.len() {
            return Err(bad());
        }
        Ok(())
    }
    fn segment(&mut self, mut data: &[u8], output: &mut Vec<u8>) -> Result<()> {
        let flags = take(&mut data, 1)?[0];
        if !matches!(flags, 0x04 | 0x24) || data.len() > SEGMENT + 1000 {
            return Err(bad());
        }
        let start = output.len();
        if flags == 0x04 {
            Self::room(output, start, data.len())?;
            self.bytes(data, output);
            return Ok(());
        }
        let (&unused, encoded) = data.split_last().ok_or_else(bad)?;
        if unused > 7 || encoded.len() * 8 < usize::from(unused) {
            return Err(bad());
        }
        let mut bits = Bits {
            data: encoded,
            at: 0,
            end: encoded.len() * 8 - usize::from(unused),
        };
        while bits.at < bits.end {
            let available = (bits.end - bits.at).min(9);
            let saved = bits.at;
            let prefix = bits.read(available)? << (9 - available);
            bits.at = saved;
            let token = PREFIXES[prefix];
            if token == 255 {
                return Err(bad());
            }
            let (_, width, value_bits, base) = TOKENS[usize::from(token)];
            bits.read(width)?;
            if value_bits == 0 {
                Self::room(output, start, 1)?;
                self.byte(base as u8, output);
            } else if value_bits == 255 {
                let value = bits.read(8)? as u8;
                Self::room(output, start, 1)?;
                self.byte(value, output);
            } else {
                let distance = base + bits.read(value_bits)?;
                if distance == 0 {
                    let count = bits.read(15)?;
                    bits.read((8 - bits.at % 8) % 8)?;
                    Self::room(output, start, count)?;
                    if count > (bits.end - bits.at) / 8 {
                        return Err(bad());
                    }
                    let at = bits.at / 8;
                    self.bytes(&encoded[at..at + count], output);
                    bits.at += count * 8;
                } else {
                    if distance > HISTORY {
                        return Err(bad());
                    }
                    let mut ones = 0;
                    while bits.read(1)? != 0 {
                        ones += 1;
                        if ones > 14 {
                            return Err(bad());
                        }
                    }
                    let count = if ones == 0 {
                        3
                    } else {
                        (1 << (ones + 1)) + bits.read(ones + 1)?
                    };
                    Self::room(output, start, count)?;
                    for _ in 0..count {
                        let value = self.history[(self.position + HISTORY - distance) % HISTORY];
                        self.byte(value, output);
                    }
                }
            }
        }
        Ok(())
    }
}
// (Huffman prefix, prefix bits, distance bits / literal marker, distance base / literal).
// Constants are the wire table in MS-RDPEGFX 3.1.9.1.2, not an encoder-specific tree.
const TOKENS: [(usize, usize, usize, usize); 40] = [
    (0b0, 1, 255, 0),
    (0b11000, 5, 0, 0),
    (0b11001, 5, 0, 1),
    (0b110100, 6, 0, 2),
    (0b110101, 6, 0, 3),
    (0b110110, 6, 0, 255),
    (0b1101110, 7, 0, 4),
    (0b1101111, 7, 0, 5),
    (0b1110000, 7, 0, 6),
    (0b1110001, 7, 0, 7),
    (0b1110010, 7, 0, 8),
    (0b1110011, 7, 0, 9),
    (0b1110100, 7, 0, 10),
    (0b1110101, 7, 0, 11),
    (0b1110110, 7, 0, 58),
    (0b1110111, 7, 0, 59),
    (0b1111000, 7, 0, 60),
    (0b1111001, 7, 0, 61),
    (0b1111010, 7, 0, 62),
    (0b1111011, 7, 0, 63),
    (0b1111100, 7, 0, 64),
    (0b1111101, 7, 0, 128),
    (0b11111100, 8, 0, 12),
    (0b11111101, 8, 0, 56),
    (0b11111110, 8, 0, 57),
    (0b11111111, 8, 0, 102),
    (0b10001, 5, 5, 0),
    (0b10010, 5, 7, 32),
    (0b10011, 5, 9, 160),
    (0b10100, 5, 10, 672),
    (0b10101, 5, 12, 1696),
    (0b101100, 6, 14, 5792),
    (0b101101, 6, 15, 22176),
    (0b1011100, 7, 18, 54944),
    (0b1011101, 7, 20, 317088),
    (0b10111100, 8, 20, 1365664),
    (0b10111101, 8, 21, 2414240),
    (0b101111100, 9, 22, 4511392),
    (0b101111101, 9, 23, 8705696),
    (0b101111110, 9, 24, 17094304),
];
// A fixed nine-bit lookup avoids scanning the Huffman table for each output byte.
const PREFIXES: [u8; 512] = {
    let mut table = [255; 512];
    let mut token = 0;
    while token < TOKENS.len() {
        let (prefix, width, _, _) = TOKENS[token];
        let mut i = prefix << (9 - width);
        let end = (prefix + 1) << (9 - width);
        while i < end {
            table[i] = token as u8;
            i += 1;
        }
        token += 1;
    }
    table
};
#[cfg(test)]
mod tests {
    use super::*;
    use ironrdp_graphics::zgfx::{Compressor, wrap_compressed, wrap_uncompressed};
    #[test]
    fn literals_matches_and_persistent_history_round_trip() {
        let mut encoder = Compressor::new();
        let mut decoder = Decoder::new();
        for data in [
            vec![1],
            vec![101; 60000],
            (0..65000).map(|i| (i % 251) as u8).collect(),
            vec![101; 60000],
        ] {
            let compressed = encoder.compress(&data).unwrap();
            let wire = wrap_compressed(&compressed);
            assert_eq!(decoder.decompress(&wire).unwrap(), data);
        }
    }
    #[test]
    fn raw_multipart_and_raw_run_populate_history() {
        let mut decoder = Decoder::new();
        let data = vec![19; 200_000];
        assert_eq!(decoder.decompress(&wrap_uncompressed(&data)).unwrap(), data);
        // Match distance one, length three, including a reference into prior raw data.
        assert_eq!(
            decoder
                .decompress(&[0xe0, 0x24, 0b10001000, 0b01000000, 5])
                .unwrap(),
            vec![19; 3]
        );
        // Distance zero, raw length three, align to byte, then raw bytes.
        assert_eq!(
            decoder
                .decompress(&[0xe0, 0x24, 0x88, 0, 1, 0x80, b'A', b'B', b'C', 0])
                .unwrap(),
            b"ABC"
        );
    }
    #[test]
    fn truncated_invalid_and_expanding_streams_fail_without_panics() {
        for wire in [
            vec![],
            vec![0xe0],
            vec![0xe0, 0x24],
            vec![0xe0, 0x24, 255],
            vec![0xe0, 0x24, 0, 9],
            vec![0xe0, 0x24, 0b11000000, 7],
            vec![0xe0, 0x24, 0x8f, 0xff, 0xff, 0xff, 0xff, 0],
            vec![0xe0, 0x24, 0xbf, 0xff, 0xff, 0xff, 0xff, 0],
            vec![0xe1, 1, 0, 1, 0, 0, 0, 255, 255, 255, 255],
            vec![0xe1, 0, 0, 0, 0, 0, 0],
            vec![0xe1, 1, 0, 0, 0, 0, 2, 1, 0, 0, 0, 4],
        ] {
            let mut decoder = Decoder::new();
            assert!(decoder.decompress(&wire).is_err(), "{wire:x?}");
            assert!(
                decoder.decompress(&[0xe0, 4, 1]).is_err(),
                "failed state must stay poisoned"
            );
        }
        let mut wire = vec![0xe0, 4];
        wire.extend(vec![0; SEGMENT + 1]);
        assert!(Decoder::new().decompress(&wire).is_err());
        let valid = wrap_uncompressed(b"hello");
        let mut multipart = vec![0xe1, 1, 0, 5, 0, 0, 0];
        multipart.extend(6u32.to_le_bytes());
        multipart.extend(&valid[1..]);
        for n in 0..multipart.len() {
            assert!(Decoder::new().decompress(&multipart[..n]).is_err());
        }
        assert_eq!(Decoder::new().decompress(&multipart).unwrap(), b"hello");
        multipart.push(0);
        assert!(Decoder::new().decompress(&multipart).is_err());
    }
    #[test]
    fn compressed_mutations_and_truncations_are_checked() {
        let wire = wrap_compressed(&Compressor::new().compress(&vec![42; 60000]).unwrap());
        for n in 0..wire.len() {
            let _ = Decoder::new().decompress(&wire[..n]);
        }
        let mut decoder = Decoder::new();
        for byte in [0, 1, 7, 8, 0x7f, 0xff] {
            for index in 2..wire.len() {
                let mut changed = wire.clone();
                changed[index] = byte;
                decoder.failed = false;
                let _ = decoder.decompress(&changed);
            }
        }
    }
}
