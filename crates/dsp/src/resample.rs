//! Fractional-position sample readers for pitch shifting a stored sample.
//! Each reads a source buffer at a non-integer position; the sampler voice
//! advances the position by its pitch ratio per output sample.

/// Nearest-sample read. Aliases badly; only a reference point.
#[inline]
pub fn read_nearest(src: &[f32], pos: f64) -> f32 {
    src.get(pos.round() as usize).copied().unwrap_or(0.0)
}

/// Linear interpolation between the two neighbouring samples.
#[inline]
pub fn read_linear(src: &[f32], pos: f64) -> f32 {
    let i = pos.floor();
    let frac = (pos - i) as f32;
    let i = i as usize;
    let a = src.get(i).copied().unwrap_or(0.0);
    let b = src.get(i + 1).copied().unwrap_or(0.0);
    a + (b - a) * frac
}

/// 4-point Hermite (Catmull-Rom) interpolation. Out-of-range taps read 0.
#[inline]
pub fn read_hermite(src: &[f32], pos: f64) -> f32 {
    let i = pos.floor();
    let t = (pos - i) as f32;
    let i = i as isize;
    let at = |k: isize| {
        usize::try_from(i + k)
            .ok()
            .and_then(|j| src.get(j))
            .copied()
            .unwrap_or(0.0)
    };
    let (y0, y1, y2, y3) = (at(-1), at(0), at(1), at(2));
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * t + c2) * t + c1) * t + y1
}

/// Windowed-sinc read with `half_taps` taps each side (Hann window).
/// Highest quality here; cost grows with `half_taps`.
pub fn read_sinc(src: &[f32], pos: f64, half_taps: usize) -> f32 {
    let i = pos.floor() as isize;
    let frac = (pos - pos.floor()) as f32;
    let n = half_taps as isize;
    let mut acc = 0.0f32;
    let mut wsum = 0.0f32;
    for k in -n + 1..=n {
        let j = i + k;
        let Some(s) = usize::try_from(j).ok().and_then(|j| src.get(j)) else {
            continue;
        };
        let x = k as f32 - frac;
        let sinc = if x.abs() < 1e-6 {
            1.0
        } else {
            let px = std::f32::consts::PI * x;
            px.sin() / px
        };
        let w = 0.5 + 0.5 * (std::f32::consts::PI * x / half_taps as f32).cos();
        let c = sinc * w;
        acc += s * c;
        wsum += c;
    }
    if wsum.abs() > 1e-6 { acc / wsum } else { acc }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_positions_return_the_sample() {
        let src = [0.0, 1.0, 2.0, 3.0, 4.0];
        for (i, &v) in src.iter().enumerate() {
            let p = i as f64;
            assert_eq!(read_nearest(&src, p), v);
            assert_eq!(read_linear(&src, p), v);
            assert!((read_hermite(&src, p) - v).abs() < 1e-6);
        }
        assert!((read_sinc(&src, 2.0, 8) - 2.0).abs() < 1e-4);
    }

    #[test]
    fn midpoints_interpolate() {
        let src = [0.0, 1.0, 2.0, 3.0];
        assert_eq!(read_linear(&src, 1.5), 1.5);
        assert!((read_hermite(&src, 1.5) - 1.5).abs() < 1e-6);
    }

    #[test]
    fn sinc_reproduces_a_low_sine_between_samples() {
        let sr = 48_000.0f64;
        let f = 440.0f64;
        let src: Vec<f32> = (0..4800)
            .map(|i| (2.0 * std::f64::consts::PI * f * i as f64 / sr).sin() as f32)
            .collect();
        let mut worst = 0.0f32;
        for i in 100..4700 {
            let p = i as f64 + 0.37;
            let want = (2.0 * std::f64::consts::PI * f * p / sr).sin() as f32;
            worst = worst.max((read_sinc(&src, p, 16) - want).abs());
        }
        assert!(worst < 1e-3, "worst error {worst}");
    }

    #[test]
    fn out_of_range_is_silent() {
        let src = [1.0, 1.0];
        assert_eq!(read_linear(&src, 5.0), 0.0);
        assert_eq!(read_hermite(&src, -3.0), 0.0);
        assert_eq!(read_sinc(&src, 50.0, 4), 0.0);
    }
}
