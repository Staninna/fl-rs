//! An operator's table in the frequency domain, as `TSytrusShape` works
//! it with IPP's real FFTs (`ippsFFTInit_R_32f` with the forward transform
//! divided by N): the harmonics filter wheel (`0x847900`) and the
//! band-limited copies a note picks by its pitch (`0x86ace0`, `0x8471d0`).
//! Section 11 of `docs/plugins/sytrus.md`.

use crate::three_osc::fft;
use std::f64::consts::PI;

/// The band-limited copies' size: `2^12` (`0x848980(14, 12)`).
pub(super) const SIZE: usize = 4096;
/// Their highest harmonic.
const HALF: usize = SIZE / 2;
/// How many copies an operator keeps.
const COPIES: usize = 32;
/// No copy: the note reads the whole table.
pub(super) const WHOLE: u8 = u8::MAX;

/// Numbers under 2^-24 are 0 (`0x628bc0`).
fn flush(x: f32) -> f32 {
    if x.abs() < 5.960_464_5e-8 { 0.0 } else { x }
}

/// The harmonic the filter wheel (0..128) cuts from (`0x869c90`):
/// `(1e10^(w x 0.35 + 0.65) - 1) x 1.6384e-6` with `w` the wheel over 128,
/// to the nearest.
pub(super) fn cutoff(wheel: i32) -> i64 {
    let w = wheel as f32 * (1.0 / 128.0);
    let x = w * 0.35 + 0.65;
    let grown = ((f64::from(x) * 1e10f64.ln()).exp() - 1.0) as f32;
    (f64::from(grown) * 1.6384e-6).round_ties_even() as i64
}

/// Lanczos's sigma for harmonic `k` of a cut at `top`, as both the filter
/// and the band limit weight the harmonics under their cut when the
/// global switch 0x4f6 is on.
fn sigma(k: usize, top: usize) -> f64 {
    let scale = (top as f64 / PI) as f32;
    let step = (PI / top as f64) as f32;
    f64::from(scale) * f64::from(k as f32 * step).sin() / k as f64
}

/// Run the filter wheel over a table of `N` samples (and one more, the
/// first again) in place: zero every harmonic from the cut up and, with
/// `smooth`, weight the ones under it by [`sigma`] (`0x847900`). Returns
/// the harmonics 0 to 2048 as the cut left them (before the sigma), each
/// over `N`, which the band-limited copies are made from.
pub(super) fn filter(table: &mut [f32], wheel: i32, smooth: bool) -> Vec<[f32; 2]> {
    let n = table.len() - 1;
    let mut re: Vec<f64> = table[..n].iter().map(|x| f64::from(*x)).collect();
    let mut im = vec![0.0; n];
    fft(&mut re, &mut im, false);
    let scale = 1.0 / n as f64;
    for (r, i) in re.iter_mut().zip(&mut im) {
        *r = f64::from((*r * scale) as f32);
        *i = f64::from((*i * scale) as f32);
    }
    let half = n / 2;
    let cut = cutoff(wheel);
    let cuts = cut <= half as i64;
    if cuts {
        for k in cut.max(0) as usize..=half {
            re[k] = 0.0;
            im[k] = 0.0;
        }
    }
    let spectrum = (0..=HALF.min(half))
        .map(|k| [re[k] as f32, if k == HALF { 0.0 } else { im[k] as f32 }])
        .collect();
    if cuts {
        let top = (cut - 1).max(0) as usize;
        if smooth && top > 0 {
            for k in 1..=top {
                let s = f64::from(sigma(k, top) as f32);
                re[k] *= s;
                im[k] *= s;
            }
        }
        let mut out_re = vec![0.0; n];
        let mut out_im = vec![0.0; n];
        hermitian(&re[..=half], &im[..=half], &mut out_re, &mut out_im);
        fft(&mut out_re, &mut out_im, true);
        for (x, y) in table.iter_mut().zip(&out_re) {
            *x = flush(*y as f32);
        }
        table[n] = table[0];
    }
    spectrum
}

/// Fill a whole spectrum from its harmonics 0 to N / 2, the rest their
/// mirror image, for an inverse transform with a real result.
fn hermitian(re: &[f64], im: &[f64], out_re: &mut [f64], out_im: &mut [f64]) {
    let n = out_re.len();
    let half = n / 2;
    for k in 0..=half {
        out_re[k] = re[k];
        out_im[k] = if k == 0 || k == half { 0.0 } else { im[k] };
        if k > 0 && k < half {
            out_re[n - k] = re[k];
            out_im[n - k] = -im[k];
        }
    }
}

/// The harmonic limits a note at `hz` reads its copy with (`0x86ace0`):
/// whole up to `18000 + (nyquist - 18000) / 4` Hz and none past `18000 +
/// 3 (nyquist - 18000) / 4`, the Nyquist frequency 22050 with the global
/// switch 0x4f6 and half the rate without. `None` when the whole table is
/// under them.
pub(super) fn limits(hz: f32, rate: f32, fixed: bool) -> Option<(i32, i32)> {
    let hz = if hz <= 0.1 { 0.1 } else { hz };
    let per = 1.0 / hz;
    let nyquist = if fixed {
        22_050.0
    } else {
        f64::from(rate * 0.5)
    };
    let d = (nyquist - 18_000.0) * 0.25;
    let low = ((d + 18_000.0) * f64::from(per)) as i32 + 1;
    let high = ((d * 3.0 + 18_000.0) * f64::from(per)) as i32 + 1;
    (low as usize <= HALF).then_some((low, high))
}

/// One copy and how many voices read it.
#[derive(Debug, Clone, PartialEq)]
struct Copy {
    limits: (i32, i32),
    users: u32,
    table: Vec<f32>,
}

/// An operator's band-limited copies (`TSytrusShape+0x850`), and the
/// room to make one without allocating.
#[derive(Debug, Clone, PartialEq, Default)]
pub(super) struct Copies {
    copies: Vec<Copy>,
    re: Vec<f64>,
    im: Vec<f64>,
}

impl Copies {
    /// Room for [`COPIES`] copies. This allocates.
    pub(super) fn new() -> Self {
        Copies {
            copies: (0..COPIES)
                .map(|_| Copy {
                    limits: (-1, -1),
                    users: 0,
                    table: vec![0.0; SIZE + 1],
                })
                .collect(),
            re: vec![0.0; SIZE],
            im: vec![0.0; SIZE],
        }
    }

    /// The copy for these limits, made from `spectrum` when no copy has
    /// them, and one more user of it. With every copy in use the one with
    /// the nearest limits stands in. This allocates nothing.
    pub(super) fn take(&mut self, limits: (i32, i32), spectrum: &[[f32; 2]], smooth: bool) -> u8 {
        if self.copies.is_empty() || spectrum.len() <= HALF {
            return WHOLE;
        }
        let found = self.copies.iter().position(|c| c.limits == limits);
        let index = match found {
            Some(index) => index,
            None => match self.copies.iter().position(|c| c.users == 0) {
                Some(index) => {
                    self.make(index, limits, spectrum, smooth);
                    index
                }
                None => {
                    let near = |c: &Copy| (c.limits.0 - limits.0).abs();
                    let best = self.copies.iter().enumerate().min_by_key(|(_, c)| near(c));
                    best.map_or(0, |(index, _)| index)
                }
            },
        };
        self.copies[index].users += 1;
        index as u8
    }

    /// One user fewer.
    pub(super) fn give_back(&mut self, index: u8) {
        if let Some(copy) = self.copies.get_mut(usize::from(index)) {
            copy.users = copy.users.saturating_sub(1);
        }
    }

    /// Nobody reads any copy.
    pub(super) fn release_all(&mut self) {
        for copy in &mut self.copies {
            copy.users = 0;
        }
    }

    pub(super) fn table(&self, index: u8) -> Option<&[f32]> {
        self.copies.get(usize::from(index)).map(|c| &c.table[..])
    }

    /// Make a copy (`0x8471d0`): the harmonics weighted 1 under the low
    /// limit, falling in a line to 0 at the high one (kept to 2048), 0
    /// above, and by [`sigma`] of the high limit with `smooth`, then back
    /// to 4096 samples.
    fn make(&mut self, index: usize, limits: (i32, i32), spectrum: &[[f32; 2]], smooth: bool) {
        let (low, high) = limits;
        let high = (high as usize).min(HALF);
        let low = low as usize;
        let weight = |k: usize| -> f32 {
            let mut w = if k < low {
                1.0
            } else if k <= high {
                let count = (high + 1 - low) as f32;
                let step = (-1.0 / f64::from(count)) as f32;
                (step + 1.0) + step * (k - low) as f32
            } else {
                0.0
            };
            if smooth && k >= 1 && k <= high && high > 0 {
                let scale = (high as f64 / PI) as f32;
                let turn = f64::from(k as f32 * (PI / high as f64) as f32).sin();
                w = ((f64::from(w * scale) * turn) / k as f64) as f32;
            }
            w
        };
        let (re, im) = (&mut self.re, &mut self.im);
        re.fill(0.0);
        im.fill(0.0);
        for k in 0..=HALF {
            let w = weight(k);
            let [r, i] = spectrum[k];
            let (r, i) = (f64::from(r * w), f64::from(i * w));
            re[k] = r;
            im[k] = if k == 0 || k == HALF { 0.0 } else { i };
            if k > 0 && k < HALF {
                re[SIZE - k] = r;
                im[SIZE - k] = -i;
            }
        }
        fft(re, im, true);
        let copy = &mut self.copies[index];
        for (x, y) in copy.table.iter_mut().zip(re.iter()) {
            *x = flush(*y as f32);
        }
        copy.table[SIZE] = copy.table[0];
        copy.limits = limits;
        copy.users = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::partial;

    /// A table of the first `count` harmonics of a saw, `n` samples and
    /// the first again.
    fn saw(n: usize, count: usize) -> Vec<f32> {
        let mut table: Vec<f32> = (0..n)
            .map(|i| {
                let x = i as f64 / n as f64;
                (1..=count)
                    .map(|h| (std::f64::consts::TAU * h as f64 * x).sin() / h as f64)
                    .sum::<f64>() as f32
            })
            .collect();
        table.push(table[0]);
        table
    }

    #[test]
    fn the_filter_wheel_cuts_the_harmonics_from_its_cut_up() {
        // The wheel at 128 cuts at harmonic 16384 and leaves everything.
        assert_eq!(cutoff(128), 16_384);
        assert_eq!(cutoff(0), 5);
        let mut table = saw(16_384, 40);
        let before = table.clone();
        let spectrum = filter(&mut table, 128, false);
        assert_eq!(table, before);
        // Harmonic 3 of a saw at 1/3 is i x -1/6 over N.
        assert!(
            (spectrum[3][1] + 1.0 / 6.0).abs() < 1e-5,
            "{:?}",
            spectrum[3]
        );
        // The wheel at 0 keeps harmonics 1 to 4.
        let spectrum = filter(&mut table, 0, false);
        let n = 16_384;
        assert!((partial(&table[..n], 4.0 / n as f64) - 0.25).abs() < 1e-3);
        assert!(partial(&table[..n], 5.0 / n as f64) < 1e-4);
        assert_eq!(spectrum[5], [0.0, 0.0]);
        // With the sigma the fourth is down to sin(pi 4 / 4)... 0, the
        // first to (4 / pi) sin(pi / 4).
        let mut smooth = saw(16_384, 40);
        filter(&mut smooth, 0, true);
        let first = partial(&smooth[..n], 1.0 / n as f64);
        assert!(
            (first - 4.0 / PI * (PI / 4.0).sin()).abs() < 1e-3,
            "{first}"
        );
    }

    #[test]
    fn a_note_reads_a_copy_without_the_harmonics_past_its_nyquist() {
        // At 44.1 kHz a note at 1 kHz keeps 19 harmonics whole and none
        // from 22 up; one at 9 Hz keeps them all.
        assert_eq!(limits(1000.0, 44_100.0, false), Some((20, 22)));
        assert_eq!(limits(9.0, 44_100.0, false), None);
        let mut table = saw(16_384, 200);
        let spectrum = filter(&mut table, 128, false);
        let mut copies = Copies::new();
        let index = copies.take((20, 22), &spectrum, false);
        let again = copies.take((20, 22), &spectrum, false);
        assert_eq!(index, again);
        let copy = copies.table(index).unwrap();
        assert_eq!(copy.len(), SIZE + 1);
        let at = |h: f64| partial(&copy[..SIZE], h / SIZE as f64);
        assert!((at(1.0) - 1.0).abs() < 1e-3, "{}", at(1.0));
        assert!((at(19.0) - 1.0 / 19.0).abs() < 1e-4);
        // Harmonic 21 is a third of the way down the line from 20 to 22.
        assert!((at(21.0) - 1.0 / 21.0 / 3.0).abs() < 1e-4, "{}", at(21.0));
        assert!(at(22.0) < 1e-5 && at(40.0) < 1e-5);
        // Given back twice, the copy is free for other limits.
        copies.give_back(index);
        copies.give_back(index);
        let other = copies.take((40, 44), &spectrum, false);
        assert_eq!(copies.copies[usize::from(other)].limits, (40, 44));
    }
}
