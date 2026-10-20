//! What Fruity Parametric EQ 2 borrows from the C++ `engine_x64.dll` in
//! Frequency Splitter's folder: the engine's 2x resampler.
//! `docs/plugins/fruity-parametric-eq-2.md` maps it, with its addresses.
//!
//! Every sum here is in the order the DLL does it, so that it comes out
//! the same to the last bit.

use std::f64::consts::{FRAC_PI_2, PI};

/// How many taps the resampler's low pass has.
pub(crate) const TAPS: usize = 176;

/// How many frames late the engine's resampler leaves the sound: its centre
/// tap, 89 samples at twice the rate, less the half sample the zero that
/// follows each one is late.
pub(crate) const RESAMPLER_LATENCY: usize = 44;

/// The engine's half-band low pass (`mydispatcher` op 10): a sinc of half
/// the band, halved, under a half sine window, rounded to floats twice as
/// the DLL rounds it.
pub(crate) fn half_band() -> [f32; TAPS] {
    let mut taps = [0.0f32; TAPS];
    for (i, tap) in taps.iter_mut().enumerate() {
        let x = (i as f64 - 89.0) * FRAC_PI_2;
        let sinc = if x == 0.0 { 1.0 } else { x.sin() / x };
        *tap = (sinc * 0.5) as f32;
    }
    for (i, tap) in taps.iter_mut().enumerate() {
        let window = (i as f64 * (PI / 176.0)).sin();
        *tap = (f64::from(*tap) * window) as f32;
    }
    taps
}

/// One side of the engine's 2x decimator (ops 10 and 12): two samples at
/// twice the rate in, one out. A ring of 256 floats that is written
/// backwards, newest first, and read against the taps in two runs either
/// side of its wrap.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Decimator {
    ring: [f32; 256],
    /// Where the next sample is written.
    at: usize,
}

impl Default for Decimator {
    fn default() -> Self {
        Decimator {
            ring: [0.0; 256],
            at: 0,
        }
    }
}

impl Decimator {
    /// Take the even and the odd sample of a pair and give the filter's
    /// output at the odd one.
    pub(crate) fn step(&mut self, taps: &[f32; TAPS], even: f32, odd: f32) -> f32 {
        const MASK: usize = 255;
        self.ring[self.at] = even;
        self.at = self.at.wrapping_sub(1) & MASK;
        self.ring[self.at] = odd;
        let start = self.at;
        let end = (start + TAPS).min(256);
        let mut out = 0.0f32;
        dot(&self.ring[start..end], &taps[..end - start], &mut out);
        if start + TAPS > 256 {
            let rest = start + TAPS - 256;
            dot(&self.ring[..rest], &taps[end - start..], &mut out);
        }
        self.at = self.at.wrapping_sub(1) & MASK;
        out
    }
}

/// The engine's dot product (`FUN_1801e0fe0`): four running sums over the
/// whole groups of four, added to `out` one after the other, then the rest
/// one by one.
fn dot(samples: &[f32], taps: &[f32], out: &mut f32) {
    let whole = samples.len() / 4 * 4;
    let mut lanes = [0.0f32; 4];
    for (s, t) in samples[..whole].chunks_exact(4).zip(taps.chunks_exact(4)) {
        for lane in 0..4 {
            lanes[lane] += s[lane] * t[lane];
        }
    }
    let mut sum = *out;
    for lane in lanes {
        sum += lane;
    }
    for (s, t) in samples[whole..].iter().zip(&taps[whole..]) {
        sum += s * t;
    }
    *out = sum;
}
