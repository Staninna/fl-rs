//! FL's linear-phase convolver: the context `mydispatcher` in
//! `engine_x64.dll` (Frequency Splitter's folder) makes for op 0 and runs a
//! frame at a time for op 4. Maximus asks for one a band; Parametric EQ 2
//! asks for one too. `docs/plugins/maximus.md` section 5.8 maps it with
//! its addresses.
//!
//! A context takes a magnitude on the `N / 2 + 1` bins of an FFT of `N`,
//! makes a filter of it (the zero-phase impulse, `N / 4` frames late, under
//! a half sine window `N / 2` long) and runs the sound through it by
//! overlap-add in FFT blocks of `N / 2`, `3 N / 4` frames late. A new
//! magnitude goes to the idle one of two filters, which is primed with the
//! last `N` frames and faded in over a set time while the other fades out.
//!
//! The FFTs are IPP's in FL, made with `IPP_FFT_NODIV_BY_ANY`: here a
//! radix-2 one in floats that scales neither way either, so the sums are
//! FL's but for the rounding of the butterflies.

use std::f64::consts::TAU;

/// A complex number in floats.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct C32 {
    re: f32,
    im: f32,
}

impl C32 {
    fn mul(self, other: C32) -> C32 {
        C32 {
            re: self.re * other.re - self.im * other.im,
            im: self.re * other.im + self.im * other.re,
        }
    }
}

/// An unscaled radix-2 FFT of one size, both ways.
#[derive(Debug, Clone, PartialEq, Default)]
struct Fft {
    /// `e^(-2 pi i k / n)` for the first half of the turn.
    turns: Vec<C32>,
}

impl Fft {
    fn new(n: usize) -> Self {
        let turns = (0..n / 2)
            .map(|k| {
                let (sin, cos) = (-TAU * k as f64 / n as f64).sin_cos();
                C32 {
                    re: cos as f32,
                    im: sin as f32,
                }
            })
            .collect();
        Fft { turns }
    }

    /// The transform of `data` in place; `inverse` turns the other way.
    fn run(&self, data: &mut [C32], inverse: bool) {
        let n = data.len();
        let mut j = 0;
        for i in 1..n {
            let mut bit = n >> 1;
            while j & bit != 0 {
                j ^= bit;
                bit >>= 1;
            }
            j |= bit;
            if i < j {
                data.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let half = len / 2;
            let step = n / len;
            for start in (0..n).step_by(len) {
                for k in 0..half {
                    let mut turn = self.turns[k * step];
                    if inverse {
                        turn.im = -turn.im;
                    }
                    let a = data[start + k];
                    let b = data[start + k + half].mul(turn);
                    data[start + k] = C32 {
                        re: a.re + b.re,
                        im: a.im + b.im,
                    };
                    data[start + k + half] = C32 {
                        re: a.re - b.re,
                        im: a.im - b.im,
                    };
                }
            }
            len <<= 1;
        }
    }
}

/// One filter of a context (`FUN_1801e3040`, 0xa8 bytes): the filter's
/// spectrum, a block of input a side and the overlap-added output a side.
#[derive(Debug, Clone, PartialEq, Default)]
struct Convolver {
    /// The filter's spectrum (`+0x38`).
    spectrum: Vec<C32>,
    /// The block filling up, its upper half always 0 (`+0x10`, `+0x18`).
    input: [Vec<C32>; 2],
    /// What comes out over the next block and the tail after it (`+0x90`,
    /// `+0x98`).
    output: [Vec<f32>; 2],
    /// The place in the block (`+0xa0`).
    at: usize,
}

impl Convolver {
    fn make_room(&mut self, n: usize) {
        self.spectrum.resize(n, C32::default());
        for side in 0..2 {
            self.input[side].resize(n, C32::default());
            self.output[side].resize(n, 0.0);
        }
    }

    /// `FUN_1801e2a50`: the place and both blocks back to 0.
    fn clear(&mut self) {
        self.at = 0;
        for side in 0..2 {
            self.input[side].fill(C32::default());
            self.output[side].fill(0.0);
        }
    }

    /// `FUN_1801e2c90` with `FUN_1801e1960`: the filter for the magnitudes
    /// on bins `0..=n/2`. The magnitudes times the spectrum of a click at
    /// `n / 2`, mirrored, transformed back and over `n`: the zero-phase
    /// impulse, `n / 2` late. Its `n / 2` taps from `n / 4` under
    /// `sin(i pi / (n/2 - 1))`, and nothing after them.
    fn design(&mut self, magnitudes: &[f32], n: usize, fft: &Fft, work: &mut [C32]) {
        let (half, quarter) = (n / 2, n / 4);
        let work = &mut work[..n];
        work.fill(C32::default());
        work[half].re = 1.0;
        fft.run(work, false);
        for k in 0..=half {
            work[k].re *= magnitudes[k];
            work[k].im *= magnitudes[k];
        }
        for k in 1..half {
            work[n - k].re *= magnitudes[k];
            work[n - k].im *= magnitudes[k];
        }
        fft.run(work, true);
        let per_n = 1.0 / n as f32;
        let step = 1.0 / (half - 1) as f32;
        let pi = std::f32::consts::PI;
        for (i, tap) in self.spectrum[..n].iter_mut().enumerate() {
            *tap = if i < half {
                let impulse = per_n * work[quarter + i].re;
                let window = (((i as f32 * step) * pi) as f64).sin() as f32;
                C32 {
                    re: impulse * window,
                    im: 0.0,
                }
            } else {
                C32::default()
            };
        }
        fft.run(&mut self.spectrum[..n], false);
    }

    /// `FUN_1801e2ad0`: a frame in, the frame the filter has for it out.
    /// Every `n / 2` frames the block goes through the filter and is
    /// added onto the output's tail.
    fn frame(&mut self, frame: [f32; 2], n: usize, fft: &Fft, work: &mut [C32]) -> [f32; 2] {
        let half = n / 2;
        let at = self.at;
        let mut out = [0.0; 2];
        for side in 0..2 {
            self.input[side][at].re = frame[side];
            out[side] = self.output[side][at];
        }
        self.at += 1;
        if self.at == half {
            self.at = 0;
            let per_n = 1.0 / n as f32;
            for side in 0..2 {
                let work = &mut work[..n];
                work.copy_from_slice(&self.input[side][..n]);
                fft.run(work, false);
                for (bin, tap) in work.iter_mut().zip(&self.spectrum[..n]) {
                    *bin = bin.mul(*tap);
                }
                fft.run(work, true);
                let output = &mut self.output[side][..n];
                output.copy_within(half..n, 0);
                output[half..].fill(0.0);
                for (out, bin) in output.iter_mut().zip(work.iter()) {
                    *out += per_n * bin.re;
                }
            }
        }
        out
    }
}

/// One context: the magnitude asked for, two filters and the last `n`
/// frames, and the crossfade between the filters.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LinearPhase {
    /// The FFT size (`+0`), 0 before the first op 2.
    n: usize,
    /// A magnitude came that the filters do not have yet (`+4`).
    fresh: bool,
    /// The magnitude (`+8`), `n / 2 + 1` bins.
    kernel: Vec<f32>,
    filters: [Convolver; 2],
    /// The last `n` frames a side (`+0x20`, `+0x28`) and where the next
    /// goes (`+0x44`).
    history: [Vec<f32>; 2],
    at: usize,
    /// The rate and the crossfade's length in seconds (`+0x30`, `+0x34`).
    rate: i32,
    window: f32,
    /// How far the crossfade is and how long it is, in frames (`+0x38`,
    /// `+0x3c`).
    faded: i32,
    fade_len: i32,
    /// The filter that is not playing, or that fades out (`+0x40`).
    idle: usize,
    fft: Fft,
    work: Vec<C32>,
}

impl Default for LinearPhase {
    /// Op 0: a context at 44.1 kHz with a crossfade of 0.1 s, then op 2
    /// for an FFT of 256.
    fn default() -> Self {
        let mut context = LinearPhase {
            n: 0,
            fresh: false,
            kernel: Vec::new(),
            filters: Default::default(),
            history: Default::default(),
            at: 0,
            rate: 44_100,
            window: f32::from_bits(0x3dcc_cccd),
            faded: 0,
            fade_len: 0x1000,
            idle: 0,
            fft: Fft::default(),
            work: Vec::new(),
        };
        context.set_size(256);
        context
    }
}

impl LinearPhase {
    pub(crate) fn size(&self) -> usize {
        self.n
    }

    /// Op 2 (`FUN_1801c8850`): for a new size, a zero magnitude, both
    /// filters made from it, empty history and no crossfade. The same size
    /// changes nothing. This allocates.
    pub(crate) fn set_size(&mut self, n: usize) {
        if self.n == n {
            return;
        }
        self.n = n;
        self.kernel.clear();
        self.kernel.resize(n / 2 + 1, 0.0);
        self.idle = 0;
        self.at = 0;
        self.faded = self.fade_len;
        self.fft = Fft::new(n);
        self.work.clear();
        self.work.resize(n, C32::default());
        for filter in &mut self.filters {
            *filter = Convolver::default();
            filter.make_room(n);
            filter.design(&self.kernel, n, &self.fft, &mut self.work);
        }
        for side in &mut self.history {
            side.clear();
            side.resize(n, 0.0);
        }
        self.fresh = false;
    }

    /// What op 0 and op 2 leave, at the size this context has, without
    /// allocating: a zero magnitude in both filters, no history, no
    /// crossfade, 44.1 kHz and 0.1 s.
    pub(crate) fn restart(&mut self) {
        self.fresh = false;
        self.kernel.fill(0.0);
        for filter in &mut self.filters {
            filter.clear();
            filter.spectrum.fill(C32::default());
        }
        for side in &mut self.history {
            side.fill(0.0);
        }
        self.at = 0;
        self.idle = 0;
        self.rate = 44_100;
        self.window = f32::from_bits(0x3dcc_cccd);
        self.fade_len = 0x1000;
        self.faded = self.fade_len;
    }

    /// Ops 5 and 6: the rate and the crossfade's seconds, for the next
    /// crossfade.
    pub(crate) fn set_rate(&mut self, rate: i32, window: f32) {
        self.rate = rate;
        self.window = window;
    }

    /// Op 0xe: the crossfade is over, and a new magnitude would be taken.
    pub(crate) fn settled(&self) -> bool {
        self.faded >= self.fade_len
    }

    /// Op 3: a new magnitude, `n / 2 + 1` bins, taken at the next frame
    /// once the crossfade is over.
    pub(crate) fn set_kernel(&mut self, magnitudes: &[f32]) {
        let bins = self.kernel.len();
        self.kernel.copy_from_slice(&magnitudes[..bins]);
        self.fresh = true;
    }

    /// Op 0xd: the history and both filters' blocks emptied; the
    /// magnitude and the crossfade stay.
    pub(crate) fn clear(&mut self) {
        for side in &mut self.history {
            side.fill(0.0);
        }
        for filter in &mut self.filters {
            filter.clear();
        }
    }

    /// Op 4 (`FUN_1801c8a70`): one frame through the playing filter, or
    /// through both while one fades into the other.
    pub(crate) fn frame(&mut self, frame: [f32; 2]) -> [f32; 2] {
        let n = self.n;
        if self.fresh && self.faded >= self.fade_len {
            self.fresh = false;
            self.faded = 0;
            self.fade_len = (self.rate as f32 * self.window) as i32;
            let idle = &mut self.filters[self.idle];
            idle.clear();
            idle.design(&self.kernel, n, &self.fft, &mut self.work);
            for _ in 0..n {
                let past = [self.history[0][self.at], self.history[1][self.at]];
                idle.frame(past, n, &self.fft, &mut self.work);
                self.at += 1;
                if self.at == n {
                    self.at = 0;
                }
            }
            self.idle = usize::from(self.idle == 0);
        }
        self.history[0][self.at] = frame[0];
        self.history[1][self.at] = frame[1];
        self.at += 1;
        if self.at == n {
            self.at = 0;
        }
        if self.faded < self.fade_len {
            let a = self.filters[0].frame(frame, n, &self.fft, &mut self.work);
            let b = self.filters[1].frame(frame, n, &self.fft, &mut self.work);
            let t = self.faded as f32 / self.fade_len as f32;
            let to_a = if self.idle == 0 { 1.0 - t } else { t };
            let to_b = 1.0 - to_a;
            self.faded += 1;
            [b[0] * to_b + a[0] * to_a, b[1] * to_b + a[1] * to_a]
        } else {
            let playing = usize::from(self.idle == 0);
            self.filters[playing].frame(frame, n, &self.fft, &mut self.work)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    #[test]
    fn the_fft_scales_neither_way() {
        let fft = Fft::new(16);
        let mut data: Vec<C32> = (0..16)
            .map(|k| C32 {
                re: (k as f32 * 0.37).sin(),
                im: 0.0,
            })
            .collect();
        let before = data.clone();
        fft.run(&mut data, false);
        fft.run(&mut data, true);
        for (a, b) in data.iter().zip(&before) {
            assert!((a.re / 16.0 - b.re).abs() < 1e-5 && (a.im / 16.0).abs() < 1e-5);
        }
    }

    /// A context settled on a flat magnitude: a click comes out `3 n / 4`
    /// frames late, as the windowed click it is.
    #[test]
    fn a_flat_magnitude_is_a_delay_of_three_quarters() {
        let mut context = LinearPhase::default();
        context.set_size(512);
        context.set_rate(48_000, 0.01);
        context.set_kernel(&[1.0; 257]);
        let mut out = Vec::new();
        for k in 0..4000 {
            let x = if k == 1000 { 1.0 } else { 0.0 };
            out.push(context.frame([x, -x]));
        }
        // The crossfade of 480 frames is over by the click.
        assert!(context.settled());
        for (k, [l, r]) in out.iter().enumerate() {
            let want = if k == 1000 + 384 { 1.0 } else { 0.0 };
            assert!((l - want).abs() < 1e-4, "{k}: {l}");
            assert_eq!(*r, -l);
        }
    }

    /// The crossfade in: from the zero filter of op 2 to the new one,
    /// linearly over `rate * window` frames.
    #[test]
    fn a_new_magnitude_fades_in_over_its_window() {
        let mut context = LinearPhase::default();
        context.set_size(256);
        context.set_rate(1000, 0.5);
        context.set_kernel(&[0.5; 129]);
        let tone = testing::sine(0.05, 2000);
        let out: Vec<f32> = tone.iter().map(|x| context.frame([*x; 2])[0]).collect();
        assert_eq!(context.fade_len, 500);
        // Past the fade and the delay, a tone at half its level.
        let level = testing::partial(&out[1000..2000], 0.05);
        assert!((level - 0.5).abs() < 1e-3, "{level}");
        // Halfway through the fade, a quarter, 192 frames late.
        let at = 250;
        let want = 0.5 * (at as f32 / 500.0) * tone[at - 192];
        assert!((out[at] - want).abs() < 2e-3, "{} {want}", out[at]);
    }
}
