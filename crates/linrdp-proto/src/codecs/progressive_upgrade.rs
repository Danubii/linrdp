//! Checked progressive refinement with one RAW/SRL reader and SRL state per component.
use crate::desktop::{Error, Result};
fn bad() -> Error {
    Error("invalid progressive refinement stream".into())
}
struct Bits<'a> {
    data: &'a [u8],
    at: usize,
}
impl Bits<'_> {
    fn read(&mut self, n: usize) -> Result<i64> {
        if n > 30 || n > self.data.len() * 8 - self.at {
            return Err(bad());
        }
        let mut value = 0;
        for _ in 0..n {
            value = (value << 1) | i64::from((self.data[self.at / 8] >> (7 - self.at % 8)) & 1);
            self.at += 1;
        }
        Ok(value)
    }
    fn finish(&self, extra: usize) -> Result<()> {
        if self.data.len() - self.at.div_ceil(8) > extra {
            Err(bad())
        } else {
            Ok(())
        }
    }
}
struct Srl<'a> {
    bits: Bits<'a>,
    kp: usize,
    zeros: usize,
    unary: bool,
}
impl Srl<'_> {
    fn next(&mut self, count: usize) -> Result<i64> {
        if self.zeros != 0 {
            self.zeros -= 1;
            return Ok(0);
        }
        let k = self.kp / 8;
        if !self.unary {
            if self.bits.read(1)? == 0 {
                self.zeros = (1 << k) - 1;
                self.kp = (self.kp + 4).min(80);
                return Ok(0);
            }
            self.zeros = self.bits.read(k)? as usize;
            self.unary = true;
            if self.zeros != 0 {
                self.zeros -= 1;
                return Ok(0);
            }
        }
        self.unary = false;
        let negative = self.bits.read(1)? != 0;
        self.kp = self.kp.saturating_sub(6);
        let mut magnitude = 1;
        let max = (1i64 << count) - 1;
        while magnitude < max && self.bits.read(1)? == 0 {
            magnitude += 1;
        }
        Ok(if negative { -magnitude } else { magnitude })
    }
}
/// Band arrays use physical order HL1,LH1,HH1,HL2,LH2,HH2,HL3,LH3,HH3,LL3.
/// Bit positions include base plus progressive quantization; reconstruction shifts by current - 1.
#[allow(clippy::too_many_arguments)]
pub fn upgrade(
    srl: &[u8],
    raw: &[u8],
    prev_bits: &[u8; 10],
    curr_bits: &[u8; 10],
    extrapolate: bool,
    coeff: &mut [i16; 4096],
    sign: &mut [i8; 4096],
) -> Result<()> {
    if srl.len() > 65535
        || raw.len() > 65535
        || (0..10).any(|i| curr_bits[i] == 0 || prev_bits[i] > 30 || curr_bits[i] > prev_bits[i])
    {
        return Err(bad());
    }
    let sizes = if extrapolate {
        [1023, 1023, 961, 272, 272, 256, 72, 72, 64, 81]
    } else {
        [1024, 1024, 1024, 256, 256, 256, 64, 64, 64, 64]
    };
    let mut srl = Srl {
        bits: Bits { data: srl, at: 0 },
        kp: 8,
        zeros: 0,
        unary: false,
    };
    let mut raw = Bits { data: raw, at: 0 };
    let mut offset = 0;
    for band in 0..10 {
        let count = usize::from(prev_bits[band] - curr_bits[band]);
        if count != 0 {
            for index in offset..offset + sizes[band] {
                let delta = if band == 9 || sign[index] != 0 {
                    let value = raw.read(count)?;
                    if band != 9 && sign[index] < 0 {
                        -value
                    } else {
                        value
                    }
                } else {
                    let value = srl.next(count)?;
                    sign[index] = value.signum() as i8;
                    value
                };
                let value = i64::from(coeff[index]) + (delta << (curr_bits[band] - 1));
                coeff[index] = i16::try_from(value).map_err(|_| bad())?;
            }
        }
        offset += sizes[band];
    }
    raw.finish(0)?;
    // SRL permits one trailing byte after byte alignment (MS-RDPEGFX SRL finishing).
    srl.bits.finish(1)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn raw_reader_continues_across_bands_and_applies_sign() {
        let mut coefficients = [0; 4096];
        let mut sign = [1; 4096];
        sign[1] = -1;
        let mut prev = [1; 10];
        prev[0] = 2;
        prev[1] = 2;
        let raw = [vec![255; 128], vec![0; 128]].concat();
        upgrade(
            &[],
            &raw,
            &prev,
            &[1; 10],
            false,
            &mut coefficients,
            &mut sign,
        )
        .unwrap();
        assert_eq!(coefficients[0], 1);
        assert_eq!(coefficients[1], -1);
        assert!(coefficients[2..1024].iter().all(|&n| n == 1));
        assert!(coefficients[1024..].iter().all(|&n| n == 0));
    }
    #[test]
    fn srl_state_and_reader_continue_across_bands() {
        let mut coefficients = [0; 4096];
        let mut sign = [1; 4096];
        sign[0] = 0;
        sign[1024] = 0;
        let mut prev = [1; 10];
        prev[0] = 2;
        prev[1] = 2;
        // Initial k=1: 1,0,0 => positive one. kp becomes2,k=0:1,1 =>negative one.
        upgrade(
            &[0b10011000],
            &[0; 256],
            &prev,
            &[1; 10],
            false,
            &mut coefficients,
            &mut sign,
        )
        .unwrap();
        assert_eq!(coefficients[0], 1);
        assert_eq!(coefficients[1024], -1);
        assert_eq!(sign[0], 1);
        assert_eq!(sign[1024], -1);
    }
    #[test]
    fn ll3_uses_positive_raw_refinement_for_both_layouts() {
        for extrapolate in [false, true] {
            let start: usize = if extrapolate { 4015 } else { 4032 };
            let mut coefficients = [-5; 4096];
            let mut sign = [-1; 4096];
            let mut prev = [2; 10];
            prev[9] = 3;
            let raw = vec![255; (4096 - start).div_ceil(8)];
            upgrade(
                &[],
                &raw,
                &prev,
                &[2; 10],
                extrapolate,
                &mut coefficients,
                &mut sign,
            )
            .unwrap();
            assert!(coefficients[..start].iter().all(|&n| n == -5));
            assert!(coefficients[start..].iter().all(|&n| n == -3));
        }
    }
    #[test]
    fn rejects_truncation_bad_positions_and_coefficient_overflow() {
        let prev = [2; 10];
        let current = [1; 10];
        assert!(
            upgrade(
                &[],
                &[],
                &prev,
                &current,
                false,
                &mut [0; 4096],
                &mut [1; 4096]
            )
            .is_err()
        );
        assert!(
            upgrade(
                &[],
                &[],
                &current,
                &prev,
                false,
                &mut [0; 4096],
                &mut [1; 4096]
            )
            .is_err()
        );
        assert!(
            upgrade(
                &[],
                &[],
                &prev,
                &[0; 10],
                false,
                &mut [0; 4096],
                &mut [1; 4096]
            )
            .is_err()
        );
        assert!(
            upgrade(
                &[],
                &[255; 512],
                &prev,
                &current,
                false,
                &mut [i16::MAX; 4096],
                &mut [1; 4096]
            )
            .is_err()
        );
        assert!(
            upgrade(
                &[],
                &[0],
                &current,
                &current,
                false,
                &mut [0; 4096],
                &mut [1; 4096]
            )
            .is_err()
        );
        assert!(
            upgrade(
                &[0; 2],
                &[],
                &current,
                &current,
                false,
                &mut [0; 4096],
                &mut [1; 4096]
            )
            .is_err()
        );
    }
}
