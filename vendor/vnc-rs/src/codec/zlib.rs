/*
Copyright (c) 2016  whitequark <whitequark@whitequark.org>
Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:
The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.
THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
*/

use std::io::{BufReader, Read, Result};

struct InflateReader<'a> {
    decompressor: flate2::Decompress,
    input: &'a [u8],
}

pub struct ZlibReader<'a>(BufReader<InflateReader<'a>>);

impl<'a> ZlibReader<'a> {
    pub fn new(decompressor: flate2::Decompress, input: &'a [u8]) -> ZlibReader<'a> {
        Self(BufReader::with_capacity(
            32768,
            InflateReader {
                decompressor,
                input,
            },
        ))
    }

    pub fn into_inner(mut self) -> Result<flate2::Decompress> {
        // Consume sync-flush markers, but reject extra decoded payload.
        let mut extra = [0];
        if self.read(&mut extra)? != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "leftover decoded data",
            ));
        }
        let inner = self.0.into_inner();
        if inner.input.is_empty() {
            Ok(inner.decompressor)
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "leftover zlib byte data",
            ))
        }
    }

    pub fn read_u8(&mut self) -> std::io::Result<u8> {
        let mut buf = [0; 1];
        self.read_exact(&mut buf)?;
        Ok(buf[0])
    }
}

impl Read for ZlibReader<'_> {
    fn read(&mut self, output: &mut [u8]) -> Result<usize> {
        self.0.read(output)
    }
}

impl Read for InflateReader<'_> {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        let in_before = self.decompressor.total_in();
        let out_before = self.decompressor.total_out();
        let result =
            self.decompressor
                .decompress(self.input, output, flate2::FlushDecompress::None);
        let consumed = (self.decompressor.total_in() - in_before) as usize;
        let produced = (self.decompressor.total_out() - out_before) as usize;

        self.input = &self.input[consumed..];
        match result {
            Ok(flate2::Status::Ok) => Ok(produced),
            Ok(flate2::Status::BufError) => Ok(produced),
            Err(error) => Err(std::io::Error::new(std::io::ErrorKind::InvalidData, error)),
            Ok(flate2::Status::StreamEnd) => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "zlib stream end",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn compressed(bytes: &[u8]) -> Vec<u8> {
        let mut data = Vec::with_capacity(bytes.len() * 2 + 128);
        flate2::Compress::new(flate2::Compression::fast(), true)
            .compress_vec(bytes, &mut data, flate2::FlushCompress::Sync)
            .unwrap();
        data
    }
    #[test]
    fn buffering_checks_payload_end_and_truncation() {
        let source: Vec<u8> = (0..100000).map(|n| n as u8).collect();
        let data = compressed(&source);
        let mut reader = ZlibReader::new(flate2::Decompress::new(true), &data);
        let mut actual = vec![0; source.len()];
        reader.read_exact(&mut actual).unwrap();
        assert_eq!(actual, source);
        reader.into_inner().unwrap();
        assert!(ZlibReader::new(flate2::Decompress::new(true), &data)
            .into_inner()
            .is_err());
        let mut reader = ZlibReader::new(flate2::Decompress::new(true), &data[..data.len() / 2]);
        assert!(reader.read_exact(&mut actual).is_err());
    }
    #[test]
    #[ignore = "release CPU benchmark"]
    fn benchmark_buffered_inflate() {
        let source: Vec<u8> = (0..1920 * 1080 * 3).map(|n| (n * 17) as u8).collect();
        let data = compressed(&source);
        let mut times = [Vec::new(), Vec::new()];
        for batch in 0..6 {
            for kind in if batch % 2 == 0 { [0, 1] } else { [1, 0] } {
                let raw = InflateReader {
                    decompressor: flate2::Decompress::new(true),
                    input: &data,
                };
                let mut reader: Box<dyn Read> = if kind == 0 {
                    Box::new(raw)
                } else {
                    Box::new(BufReader::with_capacity(32768, raw))
                };
                let start = std::time::Instant::now();
                let mut pixel = [0; 3];
                for expected in source.chunks_exact(3) {
                    reader.read_exact(&mut pixel).unwrap();
                    assert_eq!(&pixel, expected);
                    std::hint::black_box(pixel);
                }
                times[kind].push(start.elapsed().as_secs_f64() * 1000.0);
            }
        }
        for t in &mut times {
            t.sort_by(f64::total_cmp);
        }
        eprintln!(
            "1080p inflate (3-byte reads): {:.3} -> {:.3} ms",
            times[0][3], times[1][3]
        );
    }
}
