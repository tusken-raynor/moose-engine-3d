/// Converts an f32 to IEEE 754 half-precision bits, rounding to nearest even.
/// Values beyond the f16 range become infinity.
pub(crate) fn f32_to_f16_bits(x: f32) -> u16 {
    let bits = x.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let mant = bits & 0x7f_ffff;

    if exp == 0xff {
        return sign | 0x7c00 | if mant != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00;
    }
    if e <= 0 {
        // Subnormal result: value = m * 2^-24.
        if e < -10 {
            return sign;
        }
        let m = mant | 0x80_0000;
        let shift = (14 - e) as u32;
        return sign | round_shift(m, shift) as u16;
    }
    // Rounding may carry into the exponent, which is still correct (up to infinity).
    sign | round_shift(((e as u32) << 23) | mant, 13) as u16
}

fn round_shift(value: u32, shift: u32) -> u32 {
    let kept = value >> shift;
    let rem = value & ((1 << shift) - 1);
    let half = 1 << (shift - 1);
    if rem > half || (rem == half && kept & 1 == 1) {
        kept + 1
    } else {
        kept
    }
}

/// Converts IEEE 754 half-precision bits to an f32 (exact; every f16 value fits).
pub(crate) fn f16_bits_to_f32(h: u16) -> f32 {
    let sign = ((h & 0x8000) as u32) << 16;
    let exp = ((h >> 10) & 0x1f) as u32;
    let mant = (h & 0x3ff) as u32;
    let bits = match (exp, mant) {
        (0, 0) => sign,
        (0, _) => {
            // Subnormal: shift the mantissa up until its leading bit becomes the implicit one.
            let (mut e, mut m) = (127 - 15 + 1, mant);
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            sign | (e << 23) | ((m & 0x3ff) << 13)
        }
        (0x1f, _) => sign | 0x7f80_0000 | (mant << 13),
        _ => sign | ((exp + 127 - 15) << 23) | (mant << 13),
    };
    f32::from_bits(bits)
}

#[cfg(test)]
mod tests {
    use super::f16_bits_to_f32;
    use super::f32_to_f16_bits as h;

    #[test]
    fn decode_round_trips_every_value() {
        for bits in 0..=u16::MAX {
            let x = f16_bits_to_f32(bits);
            if x.is_nan() {
                assert_eq!(bits & 0x7c00, 0x7c00);
                assert_ne!(bits & 0x3ff, 0);
            } else {
                assert_eq!(h(x), bits, "{bits:#06x} -> {x}");
            }
        }
        assert_eq!(f16_bits_to_f32(0x3c00), 1.0);
        assert_eq!(f16_bits_to_f32(0x0001), 2f32.powi(-24));
    }

    #[test]
    fn known_values() {
        assert_eq!(h(0.0), 0x0000);
        assert_eq!(h(-0.0), 0x8000);
        assert_eq!(h(1.0), 0x3c00);
        assert_eq!(h(0.5), 0x3800);
        assert_eq!(h(-2.0), 0xc000);
        assert_eq!(h(0.1), 0x2e66);
        assert_eq!(h(65504.0), 0x7bff);
        assert_eq!(h(65520.0), 0x7c00); // rounds up to infinity
        assert_eq!(h(1e-7), 0x0002); // subnormal
        assert_eq!(h(2.0f32.powi(-24)), 0x0001); // smallest subnormal
        assert_eq!(h(1e-9), 0x0000);
        assert_eq!(h(f32::INFINITY), 0x7c00);
        assert_eq!(h(f32::NAN) & 0x7c00, 0x7c00);
        assert_ne!(h(f32::NAN) & 0x3ff, 0);
    }
}
