//! Rounding that compiles inline. On baseline x86-64 (SSE2, no SSE4.1
//! `roundss`) `f32::round` is a call into the maths library (`roundf`),
//! made where a plugin rounds once per partial. This gives the same bits
//! from the rounding of a sum.
//!
//! `f64::floor` and `f64::round_ties_even` (`floor`, `rint`) are calls too,
//! in Harmor's partial bank and `additive`'s phase angle. Inline versions
//! of both kept every bit and timed no faster on the plugin bench
//! (glibc's are a dozen instructions), so they were left out.

/// Every `f32` this large or larger is a whole number.
const WHOLE_F32: f32 = 8_388_608.0; // 2^23

/// `x.round()`, to the bit: the nearest whole number, a half away from
/// zero, with the sign of `x` (so -0.3 gives -0.0). Infinities and whole
/// numbers come back as they are, and a NaN as a NaN, its payload aside
/// (nothing here reads one).
#[inline]
pub fn half_away(x: f32) -> f32 {
    let a = x.abs();
    // From 2^23 to 2^24 the floats are the whole numbers, so under 2^23
    // adding 2^23 rounds `a` to one, the even one at a half, and taking
    // 2^23 off again is exact; so is `a - near`. A half that went down to
    // the even one goes up instead. A NaN goes through the sums as a NaN.
    let near = (a + WHOLE_F32) - WHOLE_F32;
    let near = near + f32::from(u8::from(a - near == 0.5));
    // 2^23 and over, every float is whole already, and the sum above
    // would round it.
    let whole = if a >= WHOLE_F32 { a } else { near };
    whole.copysign(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn same(bits: u32) -> bool {
        let x = f32::from_bits(bits);
        let (got, want) = (half_away(x), x.round());
        got.to_bits() == want.to_bits() || (got.is_nan() && want.is_nan())
    }

    #[test]
    fn it_rounds_as_round_does_at_the_edges() {
        for x in [
            0.0,
            -0.0,
            0.3,
            -0.3,
            0.5,
            -0.5,
            0.499_999_97,
            -0.499_999_97,
            1.5,
            -1.5,
            2.5,
            -2.5,
            8_388_607.5,
            -8_388_607.5,
            8_388_608.0,
            16_777_217.0,
            f32::MAX,
            f32::MIN,
            f32::MIN_POSITIVE,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NAN,
        ] {
            assert!(same(f32::to_bits(x)), "{x}");
        }
        assert_eq!(half_away(-0.3).to_bits(), (-0.0f32).to_bits());
    }

    #[test]
    fn it_rounds_as_round_does_either_side_of_each_half() {
        // Every half up to 2^16 either way, and the floats either side.
        for k in -65_536..65_536 {
            let half = k as f32 + 0.5;
            let bits = half.to_bits();
            for b in [bits - 1, bits, bits + 1] {
                assert!(same(b), "{}", f32::from_bits(b));
            }
        }
    }

    #[test]
    fn it_rounds_as_round_does_over_a_sample_of_all_floats() {
        // A prime stride through every bit pattern touches each exponent
        // and sign many times over.
        let mut bits = 0u32;
        loop {
            assert!(same(bits), "{bits:#x}");
            match bits.checked_add(97) {
                Some(next) => bits = next,
                None => break,
            }
        }
    }

    /// Every `f32` there is, about four billion: `cargo test --release -p
    /// dsp -- --ignored every_float`.
    #[test]
    #[ignore]
    fn it_rounds_as_round_does_for_every_float() {
        std::thread::scope(|s| {
            for part in 0..16u32 {
                s.spawn(move || {
                    let start = part << 28;
                    for bits in start..=start | 0x0fff_ffff {
                        assert!(same(bits), "{bits:#x}");
                    }
                });
            }
        });
    }
}
