//! The three-band EQ of a mixer insert, as `FLEngine_x64.dll` has it:
//! `docs/mixer-insert.md` has the addresses.
//!
//! Each band is an RBJ biquad with its coefficients in f64: a low shelf,
//! a peak and a high shelf, run one after the other on the insert's bus
//! after its effect slots and before its volume and pan. The filter is
//! direct form I with its state in f64, and each band rounds what it puts
//! out to f32.

use model::EqBand;

/// What a band is, as the insert constructor `FUN_01286f60` sets it at
/// band `+0x10`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Type 6.
    LowShelf,
    /// Type 7.
    Peak,
    /// Type 8.
    HighShelf,
}

/// How long a band whose gain goes to 0 runs on: a fifth of a second,
/// `sample_rate / 5` (`FUN_012c6b60`, stored at `0x193fab0`).
fn tail(sample_rate: u32) -> i64 {
    i64::from(sample_rate / 5)
}

/// The frequency of a band, in Hz, for its raw 0..=65536:
/// `(101^(raw / 65536) - 1) * 159.9 + 10`, in the floats `FUN_0127b390`
/// has it in. 10 Hz at 0 and 16 kHz at 65536.
pub(crate) fn frequency_hz(raw: i32) -> f32 {
    let g = (f64::from(raw) * (1.0 / 65_536.0)) as f32;
    let grown = (f64::from(g) * 101f64.ln()).exp();
    ((f64::from((grown - 1.0) as f32)) * 159.9 + 10.0) as f32
}

/// One band: its settings, its coefficients and its state.
#[derive(Debug, Clone, Copy)]
struct Band {
    kind: Kind,
    /// The raw values from the band's 225 records or its automation.
    raw: EqBand,
    /// Gain in dB, `raw * 0.01` as f32.
    gain: f32,
    /// Frequency in Hz, kept under `0.4999` of the sample rate.
    frequency: f32,
    /// Width, `raw / 65536` as f32.
    width: f32,
    /// What is left of the band's run: -1 for as long as it has a gain,
    /// the frames left of its tail once the gain went to 0, and 0 when it
    /// is still.
    left: i64,
    /// b0, b1, b2, a1, a2.
    coefficients: [f64; 5],
    /// x1, x2, y1, y2, left and right side by side.
    state: [[f64; 2]; 4],
}

impl Band {
    fn new(kind: Kind) -> Self {
        Band {
            kind,
            raw: EqBand {
                gain: 0,
                frequency: 0,
                width: 0,
            },
            gain: 0.0,
            frequency: 0.0,
            width: 0.0,
            left: 0,
            coefficients: [1.0, 0.0, 0.0, 0.0, 0.0],
            state: [[0.0; 2]; 4],
        }
    }

    /// Take the band's raw values in and work its coefficients out, as
    /// `FUN_0127b390` and `FUN_0069d0e0` do. A band with a gain runs for
    /// good, and one without runs on for `tail` frames: FL starts that tail
    /// on any change to a band without a gain.
    fn set(&mut self, raw: EqBand, sample_rate: u32, tail: i64) {
        self.raw = raw;
        self.frequency = frequency_hz(raw.frequency);
        self.width = (f64::from(raw.width) * (1.0 / 65_536.0)) as f32;
        self.gain = raw.gain as f32 * 0.01;
        self.left = if raw.gain == 0 { tail } else { -1 };
        self.design(sample_rate);
    }

    /// `FUN_0069d0e0` and `FUN_0069cb10`: the RBJ cookbook's shelves and
    /// peak, with a shelf's slope `0.1 + 1.9 * width` and a peak's Q
    /// `0.1 + 0.0999 * (1001^width - 1)`.
    fn design(&mut self, sample_rate: u32) {
        let top = (f64::from(sample_rate) * 0.4999) as f32;
        if self.frequency > top {
            self.frequency = top;
        }
        let step = 1.0 / f64::from(sample_rate) * std::f64::consts::TAU;
        let (sin, cos) = (f64::from(self.frequency) * step).sin_cos();
        let a = 10f64.powf(f64::from(self.gain) * 0.025);
        let c = &mut self.coefficients;
        match self.kind {
            Kind::Peak => {
                let q = (f64::from(self.width) * f64::from(1000f32 + 1.0).ln()).exp() - 1.0;
                let q = f64::from(q as f32);
                let alpha = sin / ((q * 0.0999 + 0.1) * 2.0);
                let norm = 1.0 / (alpha / a + 1.0);
                let a1 = norm * cos * -2.0;
                *c = [
                    norm * (alpha * a + 1.0),
                    a1,
                    norm * (1.0 - alpha * a),
                    a1,
                    norm * (1.0 - alpha / a),
                ];
            }
            Kind::LowShelf | Kind::HighShelf => {
                let slope = f64::from(self.width) * 1.9 + 0.1;
                let beta = ((a * a + 1.0) / slope - (a - 1.0) * (a - 1.0)).sqrt();
                let tilt = (a - 1.0) * cos;
                let lift = beta * sin;
                if self.kind == Kind::LowShelf {
                    let norm = 1.0 / (a + 1.0 + tilt + lift);
                    *c = [
                        norm * a * (((a + 1.0) - tilt) + lift),
                        norm * a * 2.0 * ((a - 1.0) - (a + 1.0) * cos),
                        norm * a * (((a + 1.0) - tilt) - lift),
                        norm * ((a - 1.0) + (a + 1.0) * cos) * -2.0,
                        norm * ((a + 1.0 + tilt) - lift),
                    ];
                } else {
                    let norm = 1.0 / (((a + 1.0) - tilt) + lift);
                    *c = [
                        norm * a * (a + 1.0 + tilt + lift),
                        norm * a * -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                        norm * a * ((a + 1.0 + tilt) - lift),
                        norm * ((a - 1.0) - (a + 1.0) * cos) * 2.0,
                        norm * (((a + 1.0) - tilt) - lift),
                    ];
                }
            }
        }
    }

    /// Run a block through the band in place: `0x69d500`, the SSE2 form
    /// of FL's stereo biquad, then `0x6a6160` on the state, which zeroes
    /// what is under 2^-24.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let [b0, b1, b2, a1, a2] = self.coefficients;
        let [mut x1, mut x2, mut y1, mut y2] = self.state;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let x = [f64::from(*l), f64::from(*r)];
            let mut y = [0.0; 2];
            for side in 0..2 {
                let rest = x1[side] * b1 - y2[side] * a2 + x2[side] * b2 - y1[side] * a1;
                y[side] = x[side] * b0 + rest;
            }
            (x2, x1, y2, y1) = (x1, x, y1, y);
            *l = y[0] as f32;
            *r = y[1] as f32;
        }
        self.state = [x1, x2, y1, y2];
        let floor = f64::from(f32::EPSILON / 2.0);
        for value in self.state.iter_mut().flatten() {
            if value.abs() < floor {
                *value = 0.0;
            }
        }
    }
}

/// An insert's EQ.
#[derive(Debug, Clone)]
pub(crate) struct InsertEq {
    bands: [Band; 3],
    sample_rate: u32,
}

impl InsertEq {
    /// The EQ with the bands an insert's 225 records give it, as a song
    /// starts: the bands with a gain run, the others are still.
    pub fn new(bands: [EqBand; 3], sample_rate: u32) -> Self {
        let kinds = [Kind::LowShelf, Kind::Peak, Kind::HighShelf];
        let mut eq = InsertEq {
            bands: kinds.map(Band::new),
            sample_rate,
        };
        for (band, raw) in eq.bands.iter_mut().zip(bands) {
            // FL's reset at the start of play (`FUN_0127b530`) stops the
            // tails the load started.
            band.set(raw, sample_rate, 0);
        }
        eq
    }

    /// Whether any band runs.
    pub fn running(&self) -> bool {
        self.bands.iter().any(|band| band.left != 0)
    }

    /// Set one raw value of a band from the next block on, by its control
    /// number less 0x1FD0: the band is the low 3 bits, 0 to 2, and the
    /// bits above say gain, frequency or width, as the REC dispatcher
    /// splits it. A number that names no band changes nothing.
    pub fn set_control(&mut self, offset: u16, value: i32) {
        let tail = tail(self.sample_rate);
        let (band, field) = (usize::from(offset & 7), offset >> 3);
        let Some(b) = self.bands.get_mut(band) else {
            return;
        };
        let mut raw = b.raw;
        match field {
            0 => raw.gain = value,
            1 => raw.frequency = value,
            2 => raw.width = value,
            _ => return,
        }
        b.set(raw, self.sample_rate, tail);
    }

    /// Run a block through the bands that run, low shelf first, as
    /// `FUN_01276d00` does. A band's tail is counted down by the block,
    /// and a band whose tail is over is still from then on, with its state
    /// cleared.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let frames = left.len() as i64;
        for band in &mut self.bands {
            if band.left == 0 {
                continue;
            }
            band.process(left, right);
            if band.left > 0 {
                band.left -= frames;
                if band.left <= 0 {
                    band.left = 0;
                    band.state = [[0.0; 2]; 4];
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn band(gain: i32, frequency: i32, width: i32) -> EqBand {
        EqBand {
            gain,
            frequency,
            width,
        }
    }

    /// FL's bands as a new insert has them, with these gains.
    fn eq(gains: [i32; 3]) -> InsertEq {
        let frequencies = [5777, 33_145, 55_825];
        let bands = [0, 1, 2].map(|b| band(gains[b], frequencies[b], 17_500));
        InsertEq::new(bands, RATE)
    }

    /// The level in dB a sine at `hz` comes out at, once the EQ has
    /// settled, left side.
    fn level_db(eq: &mut InsertEq, hz: f64) -> f64 {
        let n = RATE as usize;
        let sine = |i: usize| (std::f64::consts::TAU * hz * i as f64 / f64::from(RATE)).sin();
        let mut left: Vec<f32> = (0..n).map(|i| sine(i) as f32).collect();
        let mut right = left.clone();
        for (l, r) in left.chunks_mut(512).zip(right.chunks_mut(512)) {
            eq.process(l, r);
        }
        let tail = &left[n / 2..];
        let peak = tail.iter().fold(0f32, |m, v| m.max(v.abs()));
        20.0 * f64::from(peak).log10()
    }

    #[test]
    fn the_frequency_runs_from_10_hz_to_16_khz_through_fl_s_three_bands() {
        assert_eq!(frequency_hz(0), 10.0);
        assert!((frequency_hz(65_536) - 16_000.0).abs() < 0.01);
        for (raw, hz) in [(5777, 90.0), (33_145, 1500.0), (55_825, 8000.0)] {
            let got = frequency_hz(raw);
            assert!((got - hz).abs() < hz * 0.005, "{raw}: {got}");
        }
    }

    #[test]
    fn bands_without_a_gain_are_still_and_change_nothing() {
        let mut flat = eq([0, 0, 0]);
        assert!(!flat.running());
        let mut left = [0.25f32, -1.0, 0.5];
        let mut right = left;
        flat.process(&mut left, &mut right);
        assert_eq!(left, [0.25, -1.0, 0.5]);
    }

    #[test]
    fn each_band_lifts_its_own_part_of_the_spectrum() {
        // The low shelf: 18 dB at the bottom, nothing at the top.
        assert!((level_db(&mut eq([1800, 0, 0]), 5.0) - 18.0).abs() < 0.1);
        assert!(level_db(&mut eq([1800, 0, 0]), 12_000.0).abs() < 0.1);
        // The peak: its gain at its frequency, nothing far from it.
        assert!((level_db(&mut eq([0, -1200, 0]), 1500.0) + 12.0).abs() < 0.01);
        assert!(level_db(&mut eq([0, -1200, 0]), 30.0).abs() < 0.05);
        // The high shelf: its gain at the top, nothing at the bottom.
        assert!((level_db(&mut eq([0, 0, 600]), 20_000.0) - 6.0).abs() < 0.2);
        assert!(level_db(&mut eq([0, 0, 600]), 50.0).abs() < 0.05);
    }

    #[test]
    fn a_band_s_coefficients_are_the_cookbook_s() {
        // A peak with FL's width 17500, a Q of 0.632, at 1.5 kHz, +6 dB.
        let mut peak = Band::new(Kind::Peak);
        peak.set(band(600, 33_145, 17_500), RATE, 0);
        let q = 0.1 + 0.0999 * (1001f64.powf(17_500.0 / 65_536.0) - 1.0);
        assert!((q - 0.632).abs() < 1e-3);
        let w = std::f64::consts::TAU * f64::from(peak.frequency) / f64::from(RATE);
        let alpha = w.sin() / (2.0 * q);
        let a = 10f64.powf(6.0 / 40.0);
        let a0 = 1.0 + alpha / a;
        let want = [
            (1.0 + alpha * a) / a0,
            -2.0 * w.cos() / a0,
            (1.0 - alpha * a) / a0,
            -2.0 * w.cos() / a0,
            (1.0 - alpha / a) / a0,
        ];
        for (got, want) in peak.coefficients.iter().zip(want) {
            assert!((got - want).abs() < 1e-6, "{got} {want}");
        }
    }

    #[test]
    fn a_frequency_past_the_top_stops_just_under_half_the_rate() {
        let mut low = Band::new(Kind::HighShelf);
        low.set(band(600, 65_536, 17_500), 22_050, 0);
        assert_eq!(low.frequency, (22_050.0 * 0.4999) as f32);
        assert!(low.coefficients.iter().all(|c| c.is_finite()));
    }

    #[test]
    fn a_band_set_to_0_db_runs_on_for_a_fifth_of_a_second_and_then_stops() {
        let mut eq = eq([0, 0, 0]);
        eq.set_control(0, 1800);
        assert!(eq.running());
        let mut left = [1.0f32; 512];
        let mut right = left;
        eq.process(&mut left, &mut right);
        // Back to 0 dB: the band runs on, ringing out, for 9600 frames.
        eq.set_control(0, 0);
        let mut blocks = 0;
        while eq.running() {
            let (mut left, mut right) = ([0.0f32; 512], [0.0f32; 512]);
            eq.process(&mut left, &mut right);
            blocks += 1;
        }
        assert_eq!(blocks, 9600usize.div_ceil(512));
        assert_eq!(eq.bands[0].state, [[0.0; 2]; 4]);
        // A frequency or width set on a band without a gain starts a tail
        // too, as FL's does.
        eq.set_control(8 + 1, 20_000);
        assert!(eq.running());
        // A number past the third band changes nothing.
        let before = eq.bands.map(|b| b.raw);
        eq.set_control(3, 1800);
        eq.set_control(24, 1800);
        assert_eq!(eq.bands.map(|b| b.raw), before);
    }

    #[test]
    fn the_state_under_2_to_the_minus_24_is_zeroed_after_a_block() {
        let mut eq = eq([1800, 0, 0]);
        let (mut left, mut right) = ([1e-9f32; 4], [0.0f32; 4]);
        eq.process(&mut left, &mut right);
        assert!(eq.bands[0].state.iter().flatten().all(|v| *v == 0.0));
    }
}
