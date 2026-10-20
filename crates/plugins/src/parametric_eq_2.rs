//! Fruity Parametric EQ 2: seven bands of cookbook filters in series, as
//! `docs/plugins/fruity-parametric-eq-2.md` has it.
//!
//! The state is a little-endian i32 version, the plugin's 36 parameters as
//! i32, 36 more for the spare state of the A/B switch, and a tail of
//! switches that grew with the version: 305 bytes in version 2, 309 in 3,
//! 326 in 4, 330 in 5, 350 in 6 and 354 in 7 and 8. FL reads a version
//! above 8 as far as it knows it. This turns one down, since nothing says
//! how long it is.
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 to 6 Band level, 0.01 dB | -1800 to 1800 | 0 |
//! | 7 to 13 Band frequency | 0 to 65536 | 63 Hz to 6.3 kHz |
//! | 14 to 20 Band width | 0 to 65536 | 40124, 25412 x 5, 40124 |
//! | 21 to 27 Band type | 0 to 7 | low shelf, peak x 5, high shelf |
//! | 28 to 34 Band order | -3 to 3 | 0 |
//! | 35 Main level, 0.01 dB | -1800 to 1800 | 0 |
//!
//! A frequency is `20 * 1000^(v / 65536)` Hz. The types are Off, low pass,
//! band pass, high pass, notch, low shelf, peak and high shelf. What the
//! width means goes with the type: a Q of `(1 - w)^3 * 9.9 + 0.1` over the
//! square root of 2 for the two passes, `4.9 w + 0.1` octaves for band
//! pass, notch and peak, and a slope of `(1 - w) * 1.72 + 0.33` for the
//! shelves. An order of `o` is `|o| + 1` filters one after the other that
//! share the band's gain. With `o` above 0 they are alike, and with `o`
//! under 0, "Steep", FL's tables spread their Q, centres or slopes.
//!
//! The main level comes first, then every band that is on, each filter in
//! double precision with single precision samples between them. A peak or
//! a shelf at 0 dB runs for a fifth of a second and then drops out. After
//! a fifth of a second of silence the whole plugin sleeps and passes its
//! input on.
//!
//! With HQ on in its first mode the bands run at twice the rate, between
//! the two halves of a 2x resampler. The plugin's own is two paths of
//! three allpasses. The doc leaves its coefficients and the widths of the
//! steep peaks open. Both are read from the DLL here. A state can ask for
//! the engine's instead: each sample twice over and a 0 going up, and a
//! 176 tap FIR going down, 44 frames late
//! (`crate::parametric_eq_2_engine`).
//!
//! HQ in its second mode asks the engine for matched filters in place of
//! the cookbook's, fitted to the analog curve up to Nyquist, at the
//! project's rate (`crate::parametric_eq_2_engine`).
//!
//! One thing a state can ask for is stood in for, and
//! [`Effect::fidelity`] says when a state has it: linear phase runs the
//! same filters as they are, the same gains with another phase and no
//! delay.
//!
//! The parameter smoothing of version 7 on is not here either: a moved
//! parameter counts at once, as it does in FL with the smoothing off, and
//! every state FL's own projects hold has it off.

use crate::parametric_eq_2_engine::{self, Decimator, Matched, RESAMPLER_LATENCY, TAPS, half_band};
use crate::parts::Matrix;
use crate::{Effect, Fidelity, StateError, knob_value, read_state, state_version};
use dsp::biquad::{Biquad64, Coefficients64, bandwidth_alpha, shelf_beta};
use std::f64::consts::{FRAC_1_SQRT_2, LN_2, PI, TAU};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Parametric EQ 2";

const BANDS: usize = 7;
const PARAMS: usize = 5 * BANDS + 1;
/// Where each group of parameters starts.
const FREQ: usize = BANDS;
const WIDTH: usize = 2 * BANDS;
const KIND: usize = 3 * BANDS;
const ORDER: usize = 4 * BANDS;
const MAIN: usize = 5 * BANDS;

/// The range of each parameter's control.
const RANGES: [(i32, i32); PARAMS] = {
    let mut ranges = [(-1800, 1800); PARAMS];
    let mut band = 0;
    while band < BANDS {
        ranges[FREQ + band] = (0, 65536);
        ranges[WIDTH + band] = (0, 65536);
        ranges[KIND + band] = (0, 7);
        ranges[ORDER + band] = (-3, 3);
        band += 1;
    }
    ranges
};

#[rustfmt::skip]
const DEFAULTS: [i32; PARAMS] = [
    0, 0, 0, 0, 0, 0, 0,
    10923, 18204, 25486, 32768, 40050, 47332, 54613,
    40124, 25412, 25412, 25412, 25412, 25412, 40124,
    5, 6, 6, 6, 6, 6, 7,
    0, 0, 0, 0, 0, 0, 0,
    0,
];

/// Where the tail of the state starts, and what is in it at a fixed place.
const TAIL: usize = 4 + 8 * PARAMS;
const BITS: usize = TAIL + 25;
const ENABLED: usize = TAIL + 26;

/// How long the state of a version is.
fn state_len(version: i32) -> usize {
    match version {
        ..=2 => 305,
        3 => 309,
        4 => 326,
        5 => 330,
        6 => 350,
        _ => 354,
    }
}

/// The types a band can have, by their number in the state.
const OFF: i32 = 0;
const LOW_PASS: i32 = 1;
const BAND_PASS: i32 = 2;
const HIGH_PASS: i32 = 3;
const NOTCH: i32 = 4;
const LOW_SHELF: i32 = 5;
const PEAK: i32 = 6;

/// `ln 10 / 20`, which turns dB into a natural log. FL has it as a float.
const LN10_20: f32 = 0.115_129_255;

/// The highest corner a filter is worked out for, in radians a sample:
/// 0.999 pi, 0.7 pi for a peak and 0.8 pi for one filter of a steep peak.
/// FL holds them as floats.
const TOP: f64 = (0.999 * PI) as f32 as f64;
const TOP_PEAK: f64 = (0.7 * PI) as f32 as f64;
const TOP_STEEP_PEAK: f64 = (0.8 * PI) as f32 as f64;

/// FL's tables for the steep orders: a row for each number of filters,
/// and in it a column for each filter.
///
/// What a steep shelf's slope is multiplied by.
const STEEP_SLOPE: [[f32; 4]; 4] = [
    [1.0, 1.0, 1.0, 1.0],
    [2.630_447_1, 0.615_644_45, 1.0, 1.0],
    [5.161_228, 1.109_271, 0.509_867_3, 1.0],
    [10.319_123, 1.514_435_3, 0.694_032_85, 0.552_996_7],
];
/// What a steep pass's Q is multiplied by.
const STEEP_Q: [[f32; 4]; 4] = [
    [1.0, 1.0, 1.0, 1.0],
    [1.983_401_7, 0.749_214_65, 1.0, 1.0],
    [2.643_04, 1.081_518_7, 0.694_244_74, 1.0],
    [3.576_451_5, 1.141_732_8, 1.044_710_8, 0.655_118_6],
];
/// How far a steep peak's filters are off its centre, in widths of one.
const STEEP_OFFSET: [[f32; 4]; 4] = [
    [0.0, 0.0, 0.0, 0.0],
    [-0.25, 0.25, 0.0, 0.0],
    [-0.475, 0.0, 0.475, 0.0],
    [-0.775, -0.3, 0.3, 0.775],
];
/// What a steep peak's filters widen by: the inner ones of three and four
/// by half of pi.
const STEEP_WIDTH: [[f32; 4]; 4] = [
    [1.0, 1.0, 1.0, 1.0],
    [1.0, 1.0, 1.0, 1.0],
    [1.0, 1.570_796_4, 1.0, 1.0],
    [1.0, 1.570_796_4, 1.570_796_4, 1.0],
];
/// The part of the level, as an amplitude in dB, each filter of a steep
/// peak has. Filters that are alike have `1 / 2n`.
const STEEP_GAIN: [f32; 4] = [0.5, 0.321_25, 0.244_2, 0.214_445];

/// The frequency wheel: `20 * 1000^(v / 65536)` Hz, 20 Hz to 20 kHz.
fn hertz(freq: i32) -> f32 {
    let octaves = 1000f64.ln() / (65536.0 * LN_2);
    ((f64::from(freq) * octaves * LN_2).exp() * 20.0) as f32
}

/// The width wheel as a part of its way, 0 to 1.
fn width(value: i32) -> f32 {
    value as f32 * (1.0 / 65536.0)
}

/// The Q of a low or a high pass: `(1 - w)^3 * 9.9 + 0.1`. The filter has
/// it over the square root of 2, so that 1 is the flattest.
fn resonance(width: f32) -> f64 {
    let open = f64::from(1.0 - width);
    open * open * open * 9.9 + 0.1
}

/// The width of a band pass, a notch or a peak in octaves, 0.1 to 5.
fn octaves(width: f32) -> f64 {
    f64::from(width) * 4.9 + 0.1
}

/// The slope of a shelf, a third to 2.05.
fn shelf_slope(width: f32) -> f64 {
    f64::from(1.0 - width) * 1.719_298_245_614_035_3 + 0.333_333_333_333_333_3
}

/// The gain of a level in 0.01 dB, as FL works it out for the main level.
fn main_gain(level: i32) -> f32 {
    f64::from(level as f32 * 0.01 * LN10_20).exp() as f32
}

/// What a band pass widens by and a notch narrows by with `n` filters:
/// `10^(-3/20) * sqrt(n)`.
fn spread(n: usize) -> f32 {
    let ratio = f64::from(LN10_20 * 3.0).exp() as f32 / f64::from(LN10_20 * 6.0).exp() as f32;
    (f64::from(ratio) * (n as f64).sqrt()) as f32
}

/// The parameters of one band.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Knobs {
    level: i32,
    freq: i32,
    width: i32,
    kind: i32,
    order: i32,
}

impl Knobs {
    /// How many filters the band is.
    fn count(&self) -> usize {
        self.order.unsigned_abs() as usize + 1
    }

    /// The filters of a band that is not off, for a rate in Hz: the
    /// cookbook's, or with `matched` the engine's matched ones
    /// (`FUN_006ee6e0` into `FUN_006f1c80`). The ones past
    /// [`Knobs::count`] pass their input on.
    ///
    /// FL holds the level, the frequency and the width as floats, and
    /// comes back to a float here and there on the way. So does this.
    fn design(&self, rate: f64, matched: bool) -> [Coefficients64; 4] {
        let n = self.count();
        let row = n - 1;
        let steep = self.order < 0;
        let db = self.level as f32 * 0.01;
        let hz = hertz(self.freq);
        let width = width(self.width);
        let share = (1.0 / n as f64) as f32;
        let step = TAU / rate;
        // The corner in radians a sample, with its sine and cosine.
        let corner = |hz: f32, top: f64| {
            let w0 = (f64::from(hz) * step).min(top);
            (w0, w0.sin(), w0.cos())
        };
        // What the engine takes: the frequency over half the rate, the Q
        // and the gain, each as a float. The frequency is not held under
        // a top here.
        let engine = |kind: Matched, hz: f64, q: f64, db: f64| {
            parametric_eq_2_engine::matched(kind, ((hz / rate) * 2.0) as f32, q as f32, db as f32)
        };
        // The Q of a width in octaves: `1 / (2 sinh(ln 2 / 2 BW))`.
        let octave_q = |octaves: f64| 1.0 / ((LN_2 / 2.0 * octaves).sinh() * 2.0);

        std::array::from_fn(|i| {
            if i >= n {
                return Coefficients64::IDENTITY;
            }
            match self.kind {
                LOW_PASS | HIGH_PASS => {
                    let (_, sin, cos) = corner(hz, TOP);
                    let mut q = resonance(width);
                    if n > 1 {
                        q = f64::from(f64::from(q as f32).powf(f64::from(share)) as f32);
                    }
                    if steep {
                        q *= f64::from(STEEP_Q[row][i]);
                    }
                    if matched {
                        let kind = if self.kind == LOW_PASS {
                            Matched::LowPass
                        } else {
                            Matched::HighPass
                        };
                        return engine(kind, f64::from(hz), q * FRAC_1_SQRT_2, f64::from(db));
                    }
                    let alpha = sin * FRAC_1_SQRT_2 / q;
                    if self.kind == LOW_PASS {
                        Coefficients64::low_pass(cos, alpha)
                    } else {
                        Coefficients64::high_pass(cos, alpha)
                    }
                }
                BAND_PASS | NOTCH => {
                    let (w0, _, cos) = corner(hz, TOP);
                    if self.kind == BAND_PASS {
                        let wide = octaves(width) * f64::from(spread(n));
                        if matched {
                            let q = octave_q(wide);
                            return engine(Matched::BandPass, f64::from(hz), q, f64::from(db));
                        }
                        Coefficients64::band_pass(cos, bandwidth_alpha(w0, wide))
                    } else {
                        let narrow = octaves(width) * f64::from(1.0 / spread(n));
                        if matched {
                            let q = octave_q(narrow);
                            return engine(Matched::Notch, f64::from(hz), q, f64::from(db));
                        }
                        Coefficients64::notch(cos, bandwidth_alpha(w0, narrow))
                    }
                }
                PEAK => {
                    let (mut w0, _, mut cos) = corner(hz, TOP_PEAK);
                    let mut wide = octaves(width);
                    let mut centre = f64::from(hz);
                    let amplitude = if steep {
                        let off = f64::from(STEEP_OFFSET[row][i]) * wide * f64::from(share);
                        centre = f64::from(hz) * (off * LN_2).exp();
                        wide = wide * f64::from(share) * f64::from(STEEP_WIDTH[row][i]);
                        (w0, _, cos) = corner(centre as f32, TOP_STEEP_PEAK);
                        let mut part = f64::from(STEEP_GAIN[row]);
                        if n == 2 {
                            part -= (wide - 1.0) * 0.01;
                        }
                        f64::from(db) * part
                    } else {
                        f64::from(db) * f64::from(share) * 0.5
                    };
                    if matched {
                        // The level the filter has at its centre, twice
                        // the amplitude's.
                        return engine(Matched::Peak, centre, octave_q(wide), amplitude * 2.0);
                    }
                    let a = f64::from(f64::from(amplitude as f32 * LN10_20).exp() as f32);
                    Coefficients64::peak(cos, bandwidth_alpha(w0, wide), a)
                }
                _ => {
                    let (_, sin, cos) = corner(hz, TOP);
                    let a = f64::from(f64::from(db * share * 0.5 * LN10_20).exp() as f32);
                    let mut slope = shelf_slope(width);
                    if steep {
                        slope *= f64::from(STEEP_SLOPE[row][i]);
                    }
                    if matched {
                        let kind = if self.kind == LOW_SHELF {
                            Matched::LowShelf
                        } else {
                            Matched::HighShelf
                        };
                        let q = 1.0 / ((a + 1.0 / a) * (1.0 / slope - 1.0) + 2.0).sqrt();
                        return engine(kind, f64::from(hz), q, f64::from(db * share));
                    }
                    let beta_sin = shelf_beta(a, slope) * sin;
                    if self.kind == LOW_SHELF {
                        Coefficients64::low_shelf(cos, beta_sin, a)
                    } else {
                        Coefficients64::high_shelf(cos, beta_sin, a)
                    }
                }
            }
        })
    }
}

/// One band as it runs.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Band {
    /// 0 for a band that does nothing, -1 for one that runs, and for a
    /// peak or a shelf at 0 dB the frames it still runs for.
    tail: i32,
    /// How many of the filters the band has.
    count: usize,
    filters: [Coefficients64; 4],
    /// What each filter keeps of each side.
    state: [[Biquad64; 2]; 4],
}

impl Band {
    /// Run both sides through the band's filters, one filter after the
    /// other.
    fn run(&mut self, left: &mut [f32], right: &mut [f32]) {
        let filters = self.filters.iter().zip(&mut self.state);
        for (filter, [l, r]) in filters.take(self.count) {
            l.process_block(filter, left);
            r.process_block(filter, right);
            l.flush();
            r.flush();
        }
    }

    /// Count a peak or a shelf at 0 dB down by the frames of a block it
    /// has run, and drop it when it gets to 0. FL counts once a block,
    /// after the whole of it.
    fn count(&mut self, frames: usize) {
        if self.tail > 0 {
            let frames = i32::try_from(frames).unwrap_or(i32::MAX);
            self.tail = self.tail.saturating_sub(frames).max(0);
            if self.tail == 0 {
                self.state = Default::default();
            }
        }
    }
}

/// One first-order allpass of the resampler: `y = x1 + (x - y1) * c`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Allpass {
    before: f64,
    out: f64,
}

impl Allpass {
    fn step(&mut self, x: f64, c: f64) -> f64 {
        self.out = self.before + (x - self.out) * c;
        self.before = x;
        self.out
    }
}

/// The coefficients of the resampler's two paths, from the DLL. Together
/// the paths are a half-band low pass: going up the second path gives the
/// first of each two samples, and going down it takes the second.
const PATHS: [[f64; 3]; 2] = [
    [
        0.147_377_113_601_046_6,
        0.482_468_542_769_700_14,
        0.883_005_025_769_373_1,
    ],
    [
        0.039_151_597_734_460_045,
        0.302_646_848_328_493_4,
        0.674_615_918_546_963_9,
    ],
];

/// One side of FL's 2x resampler in one direction: two paths of three
/// allpasses at the lower rate. Maximus's oversampler is the same one
/// (`TPolyphase`, the same 13 doubles), a frame at a time.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct HalfBand {
    paths: [[Allpass; 3]; 2],
}

impl HalfBand {
    fn path(&mut self, path: usize, x: f64) -> f64 {
        let stages = self.paths[path].iter_mut().zip(PATHS[path]);
        stages.fold(x, |x, (stage, c)| stage.step(x, c))
    }

    /// Two samples in `wide` for each one in `input`.
    pub(crate) fn up(&mut self, input: &[f32], wide: &mut [f32]) {
        for (x, pair) in input.iter().zip(wide.as_chunks_mut::<2>().0) {
            let x = f64::from(*x);
            *pair = [self.path(1, x) as f32, self.path(0, x) as f32];
        }
        self.flush();
    }

    /// One sample in `out` for each two in `wide`.
    pub(crate) fn down(&mut self, wide: &[f32], out: &mut [f32]) {
        for (pair, y) in wide.as_chunks::<2>().0.iter().zip(out) {
            let first = self.path(0, f64::from(pair[0]));
            let second = self.path(1, f64::from(pair[1]));
            *y = ((second + first) * 0.5) as f32;
        }
        self.flush();
    }

    /// The two samples at twice the rate for one sample, as `up` gives
    /// them.
    pub(crate) fn up_one(&mut self, x: f32) -> [f32; 2] {
        let x = f64::from(x);
        [self.path(1, x) as f32, self.path(0, x) as f32]
    }

    /// The sample at the lower rate for two at twice it, as `down` gives
    /// it.
    pub(crate) fn down_one(&mut self, pair: [f32; 2]) -> f32 {
        let first = self.path(0, f64::from(pair[0]));
        let second = self.path(1, f64::from(pair[1]));
        ((second + first) * 0.5) as f32
    }

    pub(crate) fn flush(&mut self) {
        for stage in self.paths.as_flattened_mut() {
            for state in [&mut stage.before, &mut stage.out] {
                if state.is_subnormal() {
                    *state = 0.0;
                }
            }
        }
    }
}

/// One side of the engine's resampler: going up it puts each sample twice
/// over and a 0 after it, and going down it takes the pair through its
/// low pass.
#[derive(Debug, Clone, PartialEq, Default)]
struct EngineResampler {
    down: Decimator,
}

impl EngineResampler {
    /// Two samples in `wide` for each one in `input` (op 9).
    fn up(input: &[f32], wide: &mut [f32]) {
        for (x, pair) in input.iter().zip(wide.as_chunks_mut::<2>().0) {
            *pair = [x + x, 0.0];
        }
    }

    /// One sample in `out` for each two in `wide` (op 12).
    fn down(&mut self, taps: &[f32; TAPS], wide: &[f32], out: &mut [f32]) {
        for (pair, y) in wide.as_chunks::<2>().0.iter().zip(out) {
            *y = self.down.step(taps, pair[0], pair[1]);
        }
    }
}

/// How many frames go through the resampler at a time.
const CHUNK: usize = 128;

/// The plugin has not had sound for a fifth of a second.
const ASLEEP: i32 = i32::MIN;

/// A block under this on every sample is silence.
const SILENCE: f32 = 1e-6;

/// Fruity Parametric EQ 2. A new one has FL's defaults: seven bands at
/// 0 dB, which pass everything.
#[derive(Debug, Clone, PartialEq)]
pub struct ParametricEq2 {
    params: [i32; PARAMS],
    /// The other state of the A/B switch.
    spare: [i32; PARAMS],
    /// The HQ button, and which of its modes: 0 runs the bands at twice
    /// the rate, 1 asks for the matched filters.
    hq: bool,
    mode: u8,
    linear_phase: bool,
    /// Whether the engine's resampler is asked for in place of the
    /// plugin's own.
    engine_resampler: bool,
    /// A bit for each band that is switched on.
    enabled: i32,
    rate: i32,
    /// The main level.
    main: Matrix,
    bands: [Band; BANDS],
    /// The resampler of each side, up and down.
    up: [HalfBand; 2],
    down: [HalfBand; 2],
    /// The engine's resampler of each side, and its low pass.
    engine: [EngineResampler; 2],
    taps: [f32; TAPS],
    /// How many frames of silence are left before the plugin sleeps.
    awake: i32,
    /// The plugin has slept, and the main level jumps to where it is put
    /// when it wakes.
    woken: bool,
}

impl Default for ParametricEq2 {
    fn default() -> Self {
        let band = Band {
            tail: 0,
            count: 1,
            filters: [Coefficients64::IDENTITY; 4],
            state: Default::default(),
        };
        let mut plugin = ParametricEq2 {
            params: DEFAULTS,
            spare: DEFAULTS,
            hq: true,
            mode: 1,
            linear_phase: false,
            engine_resampler: false,
            enabled: 0x7f,
            rate: 44_100,
            main: Matrix::new([1.0, 1.0, 0.0, 0.0]),
            bands: [band; BANDS],
            up: Default::default(),
            down: Default::default(),
            engine: Default::default(),
            taps: half_band(),
            awake: ASLEEP,
            woken: false,
        };
        plugin.design_all();
        plugin
    }
}

impl ParametricEq2 {
    /// The parameters the A/B switch would bring back. Nothing here makes
    /// a sound with them.
    pub fn spare(&self) -> &[i32; 36] {
        &self.spare
    }

    /// Whether the bands run at twice the rate.
    fn oversampled(&self) -> bool {
        self.hq && self.mode == 0
    }

    /// Whether the bands are the engine's matched filters.
    fn matched(&self) -> bool {
        self.hq && self.mode == 1
    }

    /// Whether they get there by the engine's resampler.
    fn by_engine(&self) -> bool {
        self.oversampled() && self.engine_resampler
    }

    /// The rate the bands run at.
    fn band_rate(&self) -> i32 {
        if self.oversampled() {
            self.rate.saturating_mul(2)
        } else {
            self.rate
        }
    }

    fn knobs(&self, band: usize) -> Knobs {
        Knobs {
            level: self.params[band],
            freq: self.params[FREQ + band],
            width: self.params[WIDTH + band],
            kind: self.params[KIND + band],
            order: self.params[ORDER + band],
        }
    }

    /// Work a band's filters out from its parameters. What the filters
    /// hold stays.
    fn design(&mut self, band: usize) {
        let knobs = self.knobs(band);
        let rate = self.band_rate();
        let matched = self.matched();
        let band = &mut self.bands[band];
        band.count = knobs.count();
        if knobs.kind == OFF {
            band.tail = 0;
            band.filters = [Coefficients64::IDENTITY; 4];
            return;
        }
        // A peak or a shelf with no gain rings out and then drops out.
        band.tail = if knobs.kind >= LOW_SHELF && knobs.level == 0 {
            rate / 5
        } else {
            -1
        };
        band.filters = knobs.design(f64::from(rate), matched);
    }

    fn design_all(&mut self) {
        for band in 0..BANDS {
            self.design(band);
        }
    }

    fn is_on(&self, band: usize) -> bool {
        self.enabled >> band & 1 == 1
    }

    /// Whether any band changes the sound for good.
    fn shapes(&self) -> bool {
        (0..BANDS).any(|band| self.is_on(band) && self.bands[band].tail < 0)
    }

    /// Empty every filter and both resamplers.
    fn clear(&mut self) {
        for band in &mut self.bands {
            band.state = Default::default();
        }
        self.up = Default::default();
        self.down = Default::default();
        self.engine = Default::default();
    }

    /// What FL empties as it falls asleep (`FUN_006edbf0`): the bands that
    /// are counting down and its own resampler. A band that shapes the
    /// sound for good keeps what it holds, and so does the engine's
    /// resampler.
    fn sleep(&mut self) {
        for band in &mut self.bands {
            if band.tail > 0 {
                band.state = Default::default();
            }
        }
        self.up = Default::default();
        self.down = Default::default();
        self.woken = true;
    }

    /// The bands that run: on, and not dropped out.
    fn running(&self, band: usize) -> bool {
        self.bands[band].tail != 0 && self.is_on(band)
    }

    /// Run a stretch of a block through every band that runs, first band
    /// first.
    fn run(&mut self, left: &mut [f32], right: &mut [f32]) {
        for band in 0..BANDS {
            if self.running(band) {
                self.bands[band].run(left, right);
            }
        }
    }

    /// Count the bands down by the frames of a whole block, at the rate
    /// they run at.
    fn count(&mut self, frames: usize) {
        for band in 0..BANDS {
            if self.running(band) {
                self.bands[band].count(frames);
            }
        }
    }
}

impl Effect for ParametricEq2 {
    /// A state starts every filter from nothing and has its main level
    /// from the first sample on.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let version = state_version(bytes, state_len(8))?;
        if version > 8 {
            return Err(StateError::Version { found: version });
        }
        let expected = state_len(version);
        if bytes.len() != expected {
            return Err(StateError::Length {
                expected,
                found: bytes.len(),
            });
        }
        let params = read_state(&bytes[4..4 + 4 * PARAMS], &RANGES)?;
        let word = |at: usize| {
            let word = bytes[at..].first_chunk::<4>();
            word.map_or(0, |word| i32::from_le_bytes(*word))
        };
        for (index, spare) in self.spare.iter_mut().enumerate() {
            *spare = word(4 + 4 * (PARAMS + index));
        }
        let flags = word(TAIL);
        self.hq = flags & 1 == 1;
        self.mode = (flags >> 1) as u8;
        // Before version 4 there is no linear phase, FL's own resampler
        // is the one there is, and every band is on.
        let (bits, enabled) = if version >= 4 {
            (bytes[BITS], word(ENABLED))
        } else {
            (0, 0x7f)
        };
        self.linear_phase = bits & 1 == 1;
        self.engine_resampler = bits & 2 == 2;
        self.enabled = enabled;
        self.params = params;
        let main = main_gain(params[MAIN]);
        self.main.jump([main, main, 0.0, 0.0]);
        self.design_all();
        self.clear();
        Ok(())
    }

    /// The filters are worked out anew for the rate and start from
    /// nothing. A rate that is not a number of 1 Hz or more is not taken.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz >= 1.0 {
            self.rate = hz.round() as i32;
            self.design_all();
            self.clear();
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        if len == 0 {
            return;
        }
        let (left, right) = (&mut left[..len], &mut right[..len]);
        let frames = i32::try_from(len).unwrap_or(i32::MAX);

        let silent = left.iter().chain(right.iter()).all(|s| s.abs() < SILENCE);
        if !silent || self.linear_phase {
            self.awake = self.rate / 5;
        } else {
            self.awake = self.awake.saturating_sub(frames);
        }
        if self.awake <= -frames {
            if self.awake != ASLEEP {
                self.sleep();
                self.awake = ASLEEP;
            }
            return;
        }

        if self.woken {
            self.main.settle();
            self.woken = false;
        }
        self.main.apply(left, right);
        if !self.oversampled() {
            self.run(left, right);
            self.count(len);
            return;
        }
        let engine = self.engine_resampler;
        for (l, r) in left.chunks_mut(CHUNK).zip(right.chunks_mut(CHUNK)) {
            let (mut wide_l, mut wide_r) = ([0.0; 2 * CHUNK], [0.0; 2 * CHUNK]);
            let (wide_l, wide_r) = (&mut wide_l[..2 * l.len()], &mut wide_r[..2 * r.len()]);
            if engine {
                EngineResampler::up(l, wide_l);
                EngineResampler::up(r, wide_r);
            } else {
                self.up[0].up(l, wide_l);
                self.up[1].up(r, wide_r);
            }
            self.run(wide_l, wide_r);
            if engine {
                self.engine[0].down(&self.taps, wide_l, l);
                self.engine[1].down(&self.taps, wide_r, r);
            } else {
                self.down[0].down(wide_l, l);
                self.down[1].down(wide_r, r);
            }
        }
        self.count(2 * len);
    }

    fn param_count(&self) -> usize {
        PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |value| *value as f32)
    }

    /// The main level ramps to where it was put over the next block. A
    /// band's level, frequency or width moves its filters and leaves what
    /// they hold, and its type or order also empties them.
    fn set_param(&mut self, index: usize, value: f32) {
        let Some(range) = RANGES.get(index) else {
            return;
        };
        let value = knob_value(value, *range);
        if self.params[index] == value {
            return;
        }
        self.params[index] = value;
        if index == MAIN {
            let main = main_gain(value);
            self.main.aim([main, main, 0.0, 0.0]);
            return;
        }
        let band = index % BANDS;
        if index >= KIND {
            self.bands[band].state = Default::default();
        }
        self.design(band);
    }

    /// With no band that changes the sound, linear phase has nothing to
    /// differ in.
    fn fidelity(&self) -> Fidelity {
        if self.linear_phase && self.shapes() {
            Fidelity::Approximate("linear phase runs as the same filters with their own phase")
        } else {
            Fidelity::Exact
        }
    }

    /// 44 frames with the engine's resampler, which FL tells its host,
    /// and 0 otherwise. FL's convolver is `3N/4` frames late with linear
    /// phase on, and FL tells its host so. The filters that stand in for
    /// it here are not late, so a host has nothing to make up for.
    fn latency(&self) -> usize {
        if self.by_engine() {
            RESAMPLER_LATENCY
        } else {
            0
        }
    }

    /// Empties every filter and the resampler, ends a ramp of the main
    /// level and puts the plugin to sleep.
    fn reset(&mut self) {
        self.clear();
        self.main.settle();
        self.awake = ASLEEP;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{bits, corpus, partial, presets, projects, response, sine, state};

    /// HQ off, HQ at twice the rate and HQ with the matched filters, as
    /// the state's flags have them.
    const PLAIN: i32 = 0;
    const TWICE: i32 = 1;
    const MATCHED: i32 = 3;

    /// A state of this version, with the spare state and everything FL's
    /// window keeps at its defaults. `bits` is the byte of switches that
    /// versions 4 and up have, and `enabled` their bit for each band.
    fn saved(version: i32, params: &[i32; PARAMS], flags: i32, bits: u8, enabled: i32) -> Vec<u8> {
        let mut bytes = state(&[version]);
        bytes.extend(state(params));
        bytes.extend(state(&DEFAULTS));
        bytes.extend(state(&[flags, 0]));
        bytes.push(1);
        bytes.extend(state(&[2]));
        if version > 2 {
            bytes.extend(state(&[1]));
        }
        if version >= 4 {
            bytes.extend(state(&[2, 0]));
            bytes.push(bits);
            bytes.extend(state(&[enabled, 1]));
        }
        if version > 4 {
            bytes.extend(state(&[0]));
        }
        if version >= 6 {
            bytes.extend(state(&[3, 1, 2, 2, 2]));
        }
        if version > 6 {
            bytes.extend(state(&[0]));
        }
        bytes
    }

    fn with(bytes: &[u8]) -> ParametricEq2 {
        let mut plugin = ParametricEq2::default();
        plugin.set_state(bytes).unwrap();
        plugin
    }

    /// A version 8 state with every band on.
    fn eq(params: &[i32; PARAMS], flags: i32) -> ParametricEq2 {
        with(&saved(8, params, flags, 0x40, 0x7f))
    }

    /// Where the frequency wheel is for this many Hz.
    fn wheel(hz: f64) -> i32 {
        (65536.0 * (hz / 20.0).ln() / 1000f64.ln()).round() as i32
    }

    /// The default state with one band set and the others typed off.
    fn one_band(kind: i32, hz: f64, level: i32, width: i32, order: i32) -> [i32; PARAMS] {
        let mut params = DEFAULTS;
        params[KIND..ORDER].fill(OFF);
        params[0] = level;
        params[FREQ] = wheel(hz);
        params[WIDTH] = width;
        params[KIND] = kind;
        params[ORDER] = order;
        params
    }

    /// The gain in dB the plugin has at 44.1 kHz for a tone of a whole
    /// number of Hz, once it has settled. Both sides get the tone.
    fn measured(plugin: &mut ParametricEq2, hz: usize) -> f64 {
        let turns = hz as f64 / 44_100.0;
        let mut left: Vec<f32> = sine(turns, 2 * 44_100).iter().map(|v| v * 0.25).collect();
        let mut right = left.clone();
        for (l, r) in left.chunks_mut(512).zip(right.chunks_mut(512)) {
            plugin.process(l, r);
        }
        assert!(left == right);
        20.0 * (partial(&left[44_100..], turns) / 0.25).log10()
    }

    /// The gain in dB of the doc's peak filter at `hz`: the cookbook's,
    /// with `A = 10^(dB / 40)` and a width in octaves.
    fn doc_peak(centre: f64, octaves: f64, db: f64, rate: f64, hz: f64) -> f64 {
        let w0 = TAU * centre / rate;
        let a = 10f64.powf(db / 40.0);
        let alpha = w0.sin() * (LN_2 / 2.0 * octaves * w0 / w0.sin()).sinh();
        let num = [1.0 + alpha * a, -2.0 * w0.cos(), 1.0 - alpha * a];
        let den = [1.0 + alpha / a, -2.0 * w0.cos(), 1.0 - alpha / a];
        20.0 * response(&num, &den, hz / rate).log10()
    }

    /// The gain in dB of the analog peak at `hz`: `(s^2 + s A/Q + 1) /
    /// (s^2 + s/(A Q) + 1)` with `A = 10^(dB / 40)`.
    fn analog_peak(centre: f64, q: f64, db: f64, hz: f64) -> f64 {
        let a = 10f64.powf(db / 40.0);
        let w = hz / centre;
        let real = (1.0 - w * w) * (1.0 - w * w);
        let num = real + (w * a / q) * (w * a / q);
        let den = real + (w / (a * q)) * (w / (a * q));
        10.0 * (num / den).log10()
    }

    /// A signal with a bit of everything in it.
    fn noise(len: usize) -> Vec<f32> {
        let mut seed = 0x2545_f491u32;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed >> 8) as f32 / 8_388_608.0 - 1.0
        };
        (0..len).map(|_| next() * 0.5).collect()
    }

    fn running(plugin: &ParametricEq2) -> usize {
        plugin.bands.iter().filter(|band| band.tail != 0).count()
    }

    #[test]
    fn the_default_passes_everything_and_its_bands_drop_out() {
        let mut plugin = ParametricEq2::default();
        assert_eq!(plugin, eq(&DEFAULTS, MATCHED));
        assert_eq!(plugin.fidelity(), Fidelity::Exact);
        assert_eq!(plugin.param_count(), 36);
        let params: Vec<f32> = (0..36).map(|index| plugin.param(index)).collect();
        assert_eq!(params, DEFAULTS.map(|value| value as f32));
        assert_eq!((plugin.param(36), plugin.spare()), (0.0, &DEFAULTS));

        // Every band is a peak or a shelf at 0 dB. For a fifth of a second
        // they run, and what they do to a sample is under what a float
        // can tell. Then they are off, and the block is not touched.
        assert_eq!(running(&plugin), 7);
        let input = noise(3 * 8_820);
        let (mut left, mut right) = (input.clone(), input.iter().map(|v| -v).collect::<Vec<_>>());
        for (block, (l, r)) in left.chunks_mut(441).zip(right.chunks_mut(441)).enumerate() {
            assert_eq!(running(&plugin), if block < 20 { 7 } else { 0 }, "{block}");
            plugin.process(l, r);
        }
        for (got, want) in left.iter().zip(&input) {
            assert!((got - want).abs() < 1e-6);
        }
        assert_eq!(bits(&left[8_820..]), bits(&input[8_820..]));
        assert!(right.iter().zip(&input).all(|(got, want)| *got == -want));

        // A level moved off 0 and back runs its band again.
        plugin.set_param(3, 1.0);
        assert_eq!((running(&plugin), plugin.bands[3].tail), (1, -1));
        plugin.set_param(3, 0.0);
        assert_eq!(plugin.bands[3].tail, 8_820);

        // The same state with HQ at twice the rate drops its bands too,
        // and what is left is the resampler: unity at 1 kHz.
        let mut plugin = eq(&DEFAULTS, TWICE);
        assert_eq!(plugin.bands[0].tail, 17_640);
        assert!(measured(&mut plugin, 1_000).abs() < 1e-4);
        assert_eq!(running(&plugin), 0);
    }

    #[test]
    fn a_peak_of_6_db_at_1_khz_is_6_db_there_and_gone_a_decade_away() {
        // The default width of a peak is 2 octaves.
        let params = one_band(PEAK, 1_000.0, 600, 25412, 0);
        let centre = f64::from(hertz(params[FREQ]));
        assert!((centre - 1_000.0).abs() < 0.1);
        for (flags, rate) in [(PLAIN, 44_100.0), (TWICE, 88_200.0)] {
            let mut plugin = eq(&params, flags);
            let at = |hz: usize| measured(&mut eq(&params, flags), hz);
            let (low, mid, high) = (at(100), measured(&mut plugin, 1_000), at(10_000));
            assert!((mid - 6.0).abs() < 0.01, "{mid} dB with flags {flags}");
            assert!(low > 0.0 && low < 0.5, "{low} dB at 100 Hz");
            assert!(high > 0.0 && high < 0.5, "{high} dB at 10 kHz");
            // And what the doc's formula gives at the rate the band runs
            // at.
            for (got, hz) in [(low, 100.0), (mid, 1_000.0), (high, 10_000.0)] {
                let want = doc_peak(centre, 2.0, 6.0, rate, hz);
                assert!((got - want).abs() < 2e-3, "{hz} Hz: {got} and not {want}");
            }
        }
        // At twice the rate the top of the band is less squeezed: more of
        // the peak is left at 10 kHz.
        let plain = measured(&mut eq(&params, PLAIN), 10_000);
        let twice = measured(&mut eq(&params, TWICE), 10_000);
        assert!(twice > plain + 0.01, "{twice} against {plain}");
        // The matched filter is the analog peak at the rate, with no
        // squeeze to speak of: within 0.02 dB of it from 100 Hz to
        // 15 kHz.
        let q = 1.0 / ((LN_2 / 2.0 * 2.0f64).sinh() * 2.0);
        for hz in [100, 1_000, 4_000, 10_000, 15_000] {
            let got = measured(&mut eq(&params, MATCHED), hz);
            let want = analog_peak(centre, q, 6.0, hz as f64);
            assert!((got - want).abs() < 0.02, "{hz} Hz: {got} and not {want}");
        }
        assert!(plain < analog_peak(centre, q, 6.0, 10_000.0) - 0.02);

        // A cut is the boost upside down.
        let cut = one_band(PEAK, 1_000.0, -600, 25412, 0);
        assert!((measured(&mut eq(&cut, PLAIN), 1_000) + 6.0).abs() < 0.01);
        // The main level is on top of the bands, in 0.01 dB.
        let mut louder = params;
        louder[MAIN] = -350;
        assert!((measured(&mut eq(&louder, PLAIN), 1_000) - 2.5).abs() < 0.01);
    }

    #[test]
    fn the_wheels_are_the_doc_s_curves() {
        // 20 Hz to 20 kHz, and the default bands where the doc has them.
        assert_eq!((hertz(0), hertz(65536)), (20.0, 20_000.0));
        let bands = [63.0, 136.0, 294.0, 632.0, 1363.0, 2936.0, 6324.0];
        for (band, hz) in bands.into_iter().enumerate() {
            assert_eq!(hertz(DEFAULTS[FREQ + band]).round(), hz);
        }
        assert_eq!((wheel(20.0), wheel(20_000.0)), (0, 65536));

        // The default width of a low or a high pass, 36068, is a Q of 1,
        // which the filter has over the square root of 2.
        let default = ((1.0 - (1.0f64 / 11.0).cbrt()) * 65536.0).round();
        assert_eq!(default, 36068.0);
        assert!((resonance(width(36068)) - 1.0).abs() < 2e-5);
        assert_eq!((resonance(width(0)), resonance(width(65536))), (10.0, 0.1));
        // The other two defaults: 2 octaves and a slope of 1.
        assert!((octaves(width(25412)) - 2.0).abs() < 1e-5);
        assert!((octaves(width(0)) - 0.1).abs() < 1e-12 && octaves(width(65536)) == 5.0);
        assert!((shelf_slope(width(40124)) - 1.0).abs() < 1e-5);
        assert!((shelf_slope(width(65536)) - 1.0 / 3.0).abs() < 1e-12);
        assert!((shelf_slope(width(0)) - 2.0526).abs() < 1e-4);
        // The main level is 10^(v / 2000).
        assert_eq!(main_gain(0), 1.0);
        for level in [-1800, -350, 600, 1800] {
            let want = 10f64.powf(f64::from(level) / 2000.0);
            assert!((f64::from(main_gain(level)) - want).abs() < 1e-6 * want);
        }
        // 0.7079 times the square root of the number of filters.
        assert!((f64::from(spread(1)) - 0.7079).abs() < 5e-5);
        assert!((f64::from(spread(4)) - 2.0 * 0.7079).abs() < 1e-4);
    }

    #[test]
    fn a_steep_pass_spreads_its_q_as_the_tables_say() {
        // A cookbook low pass has the gain Q at its corner, and FL's has
        // the wheel's Q over the square root of 2. At the default Q of 1,
        // filters that are alike lose 3 dB each there, and the steep ones
        // lose 3 dB between them.
        let corner = |order: i32, kind: i32| {
            let params = one_band(kind, 1_000.0, 0, 36068, order);
            measured(&mut eq(&params, PLAIN), 1_000)
        };
        let by_table = |row: usize| {
            let q = STEEP_Q[row].map(f64::from);
            let filters = q.iter().take(row + 1).map(|q| q * FRAC_1_SQRT_2);
            20.0 * filters.product::<f64>().log10()
        };
        let three = 20.0 * FRAC_1_SQRT_2.log10();
        for kind in [LOW_PASS, HIGH_PASS] {
            for (order, want) in [
                (0, three),
                (1, 2.0 * three),
                (2, 3.0 * three),
                (3, 4.0 * three),
            ] {
                let got = corner(order, kind);
                assert!((got - want).abs() < 0.02, "order {order}: {got} dB");
            }
            for (order, row) in [(-1, 1), (-2, 2), (-3, 3)] {
                let got = corner(order, kind);
                assert!(
                    (got - by_table(row)).abs() < 0.02,
                    "order {order}: {got} dB"
                );
            }
        }
        // The doc's tables, to its three places.
        assert!((by_table(1) - 20.0 * (1.983 * 0.749 / 2.0f64).log10()).abs() < 0.01);
        assert!((by_table(3) + 3.11).abs() < 0.01 && (by_table(1) + 2.58).abs() < 0.01);
        assert!((corner(1, LOW_PASS) - corner(-1, LOW_PASS) + 3.44).abs() < 0.03);
        assert!((corner(3, LOW_PASS) - corner(-3, LOW_PASS) + 8.93).abs() < 0.03);

        // Steep or not, the slope far above the corner is the order's:
        // 24, 48 and 96 dB two octaves up for 2, 4 and 8 poles.
        let far = |order: i32| {
            let params = one_band(LOW_PASS, 1_000.0, 0, 36068, order);
            measured(&mut eq(&params, PLAIN), 4_000)
        };
        assert!((far(0) + 24.5).abs() < 0.1);
        for (order, want) in [(1, -49.1), (3, -98.2)] {
            assert!((far(order) - want).abs() < 0.1, "order {order}");
            assert!((far(-order) - far(order)).abs() < 0.1, "order {order}");
        }
        // And between the pass band and the corner the steep one holds
        // on: at 700 Hz it has lost 0.02 dB where eight poles alike have
        // lost 3.7.
        let near = |order: i32| {
            let params = one_band(LOW_PASS, 1_000.0, 0, 36068, order);
            measured(&mut eq(&params, PLAIN), 700)
        };
        assert!(near(-3).abs() < 0.03 && (near(3) + 3.73).abs() < 0.03);

        // The width wheel all the way down is a Q of 10: 17 dB of
        // resonance on one filter.
        let params = one_band(LOW_PASS, 1_000.0, 0, 0, 0);
        let got = measured(&mut eq(&params, PLAIN), 1_000);
        assert!(
            (got - 20.0 * (10.0 * FRAC_1_SQRT_2).log10()).abs() < 0.02,
            "{got}"
        );
    }

    #[test]
    fn a_band_s_filters_share_its_gain() {
        // A shelf of +9 dB has all of it at its own end, half of it at
        // the corner and none at the other end, whatever the order.
        for order in -3..=3 {
            let low = one_band(LOW_SHELF, 1_000.0, 900, 40124, order);
            let at = |hz: usize| measured(&mut eq(&low, PLAIN), hz);
            let (bottom, corner, top) = (at(20), at(1_000), at(15_000));
            assert!((bottom - 9.0).abs() < 0.01, "order {order}: {bottom}");
            assert!((corner - 4.5).abs() < 0.01, "order {order}: {corner}");
            assert!(top.abs() < 0.01, "order {order}: {top}");
            // The steep ones get there in less room: an octave above the
            // corner half a dB is left of a shelf of filters alike, and
            // under 0.15 dB of a steep one.
            let above = at(2_000);
            let most = if order < 0 { 0.15 } else { 0.6 };
            assert!(above > 0.0 && above < most, "order {order}: {above}");

            let high = one_band(7, 1_000.0, 900, 40124, order);
            let at = |hz: usize| measured(&mut eq(&high, PLAIN), hz);
            assert!((at(15_000) - 9.0).abs() < 0.01 && at(20).abs() < 0.01);
            assert!((at(1_000) - 4.5).abs() < 0.01);
        }

        // A peak of filters alike has its level in the centre to the
        // hundredth of a dB. A steep one is built from filters off the
        // centre, with gains from FL's table that come to the level
        // within 0.2 dB, and it is narrower: an octave out, half of the
        // level is left of the one and under a third of the other.
        for order in -3..=3 {
            let peak = one_band(PEAK, 1_000.0, 600, 25412, order);
            let at = |hz: usize| measured(&mut eq(&peak, PLAIN), hz);
            let (centre, octave) = (at(1_000), at(500));
            if order < 0 {
                assert!((centre - 6.0).abs() < 0.2, "order {order}: {centre}");
                assert!(octave > 1.0 && octave < 1.7, "order {order}: {octave}");
            } else {
                assert!((centre - 6.0).abs() < 0.01, "order {order}: {centre}");
                assert!((octave - 3.0).abs() < 0.02, "order {order}: {octave}");
            }
        }
    }

    #[test]
    fn a_band_pass_and_a_notch_are_at_their_centre_what_they_say() {
        for order in [0, 2, -3] {
            let band = one_band(BAND_PASS, 1_000.0, 0, 25412, order);
            let at = |hz: usize| measured(&mut eq(&band, PLAIN), hz);
            assert!(at(1_000).abs() < 0.01, "order {order}");
            assert!(at(100) < -12.0 && at(10_000) < -12.0, "order {order}");

            let notch = one_band(NOTCH, 1_000.0, 0, 25412, order);
            let at = |hz: usize| measured(&mut eq(&notch, PLAIN), hz);
            assert!(at(1_000) < -60.0, "order {order}: {}", at(1_000));
            assert!(
                at(100).abs() < 0.5 && at(10_000).abs() < 0.5,
                "order {order}"
            );
        }
        // One band pass 2 octaves wide by the wheel is 0.7079 of that
        // between its half-power points: 3 dB down 0.7079 octaves out.
        let band = one_band(BAND_PASS, 1_000.0, 0, 25412, 0);
        let edge = (1_000.0 * 2f64.powf(-0.7079)).round() as usize;
        let got = measured(&mut eq(&band, PLAIN), edge);
        assert!((got + 3.0).abs() < 0.05, "{got} dB at {edge} Hz");
    }

    #[test]
    fn every_version_of_the_state_loads_at_its_length() {
        let params = one_band(PEAK, 250.0, -420, 9000, -2);
        let lengths = [
            (2, 305),
            (3, 309),
            (4, 326),
            (5, 330),
            (6, 350),
            (7, 354),
            (8, 354),
        ];
        for (version, len) in lengths {
            let bytes = saved(version, &params, TWICE, 0x43, 0x7e);
            assert_eq!(bytes.len(), len, "version {version}");
            let plugin = with(&bytes);
            assert_eq!(plugin.params, params);
            assert!(plugin.hq && plugin.mode == 0);
            // The switches of version 4 are not in the older ones, where
            // every band is on.
            let newer = version >= 4;
            assert_eq!(plugin.linear_phase, newer, "version {version}");
            assert_eq!(plugin.engine_resampler, newer, "version {version}");
            assert_eq!(plugin.enabled, if newer { 0x7e } else { 0x7f });
        }
        // FL reads a version under 2 as one of 2.
        let mut ancient = saved(2, &params, PLAIN, 0, 0);
        ancient[..4].copy_from_slice(&1i32.to_le_bytes());
        assert_eq!(with(&ancient), with(&saved(2, &params, PLAIN, 0, 0)));
        // The HQ button is the lowest bit of its word and the mode the
        // rest.
        for (flags, hq, mode) in [(0, false, 0), (1, true, 0), (2, false, 1), (3, true, 1)] {
            let plugin = eq(&params, flags);
            assert_eq!((plugin.hq, plugin.mode), (hq, mode), "flags {flags}");
            assert_eq!(plugin.oversampled(), flags == 1);
        }
        // The spare state is kept as it is.
        let mut bytes = saved(8, &params, PLAIN, 0x40, 0x7f);
        bytes[4 + 4 * PARAMS..][..4].copy_from_slice(&(-430i32).to_le_bytes());
        assert_eq!(with(&bytes).spare()[0], -430);
    }

    #[test]
    fn a_state_the_plugin_does_not_write_is_turned_down() {
        let mut plugin = eq(&one_band(NOTCH, 60.0, 0, 100, 3), TWICE);
        let before = plugin.clone();
        let mut newer = saved(8, &DEFAULTS, MATCHED, 0x40, 0x7f);
        newer[..4].copy_from_slice(&9i32.to_le_bytes());
        assert_eq!(
            plugin.set_state(&newer),
            Err(StateError::Version { found: 9 })
        );
        for (version, expected) in [(2, 305), (3, 309), (6, 350), (8, 354)] {
            let bytes = saved(version, &DEFAULTS, MATCHED, 0x40, 0x7f);
            for wrong in [&bytes[..expected - 1], &[&bytes[..], &[0]].concat()] {
                assert_eq!(
                    plugin.set_state(wrong),
                    Err(StateError::Length {
                        expected,
                        found: wrong.len()
                    })
                );
            }
        }
        for bytes in [&[8, 0, 0][..], &[]] {
            assert_eq!(
                plugin.set_state(bytes),
                Err(StateError::Length {
                    expected: 354,
                    found: bytes.len()
                })
            );
        }
        for (param, value) in [
            (0, -1801),
            (6, 1801),
            (FREQ, -1),
            (FREQ + 6, 65537),
            (WIDTH + 3, 65537),
            (KIND, 8),
            (KIND + 6, -1),
            (ORDER, 4),
            (ORDER + 5, -4),
            (MAIN, 1801),
        ] {
            let mut params = DEFAULTS;
            params[param] = value;
            for version in [2, 8] {
                assert_eq!(
                    plugin.set_state(&saved(version, &params, MATCHED, 0x40, 0x7f)),
                    Err(StateError::Range { param, value })
                );
            }
        }
        assert_eq!(plugin, before);
    }

    #[test]
    fn what_is_stood_in_for_is_said() {
        let flat = DEFAULTS;
        let peak = one_band(PEAK, 1_000.0, 600, 25412, 0);
        let note = |plugin: &ParametricEq2| match plugin.fidelity() {
            Fidelity::Exact => "",
            Fidelity::Approximate(note) => note,
        };
        // The cookbook filters, at the rate or at twice it, are FL's.
        for flags in [PLAIN, TWICE, 2] {
            assert_eq!(eq(&peak, flags).fidelity(), Fidelity::Exact, "{flags}");
        }
        // So are the engine's matched ones.
        assert_eq!(eq(&peak, MATCHED).fidelity(), Fidelity::Exact);
        assert_eq!(eq(&flat, MATCHED).fidelity(), Fidelity::Exact);
        // Linear phase is not, where a band does something.
        let linear =
            |params: &[i32; PARAMS], flags: i32| with(&saved(8, params, flags, 0x41, 0x7f));
        assert!(note(&linear(&peak, PLAIN)).contains("linear phase"));
        assert!(note(&linear(&peak, MATCHED)).contains("linear phase"));
        assert_eq!(linear(&flat, PLAIN).fidelity(), Fidelity::Exact);
        // The engine's resampler is asked for by its bit, and it only
        // runs with HQ at twice the rate. It is FL's.
        let engine =
            |params: &[i32; PARAMS], flags: i32| with(&saved(8, params, flags, 0x42, 0x7f));
        assert_eq!(engine(&flat, TWICE).fidelity(), Fidelity::Exact);
        assert_eq!(engine(&peak, TWICE).fidelity(), Fidelity::Exact);
        assert_eq!(engine(&peak, PLAIN).fidelity(), Fidelity::Exact);
        assert_eq!(engine(&peak, MATCHED).fidelity(), Fidelity::Exact);
        // A moved level changes what is said.
        let mut plugin = with(&saved(8, &flat, PLAIN, 0x41, 0x7f));
        plugin.set_param(2, 150.0);
        assert!(note(&plugin).contains("linear phase"));
        let off = saved(8, &peak, PLAIN, 0x41, 0x7e);
        assert_eq!(with(&off).fidelity(), Fidelity::Exact);

        // The stand-in sounds as the filters it is made of do.
        let want = measured(&mut eq(&peak, PLAIN), 2_000);
        assert_eq!(measured(&mut linear(&peak, PLAIN), 2_000), want);
        let want = measured(&mut eq(&peak, MATCHED), 2_000);
        assert_eq!(measured(&mut linear(&peak, MATCHED), 2_000), want);
        // The two resamplers pass 2 kHz alike, to a hundredth of a dB.
        let twice = measured(&mut eq(&peak, TWICE), 2_000);
        let by_engine = measured(&mut engine(&peak, TWICE), 2_000);
        assert!((by_engine - twice).abs() < 0.01, "{by_engine} and {twice}");
    }

    #[test]
    fn a_band_that_is_off_does_nothing() {
        // Typed off, or switched off by its bit in versions 4 and up.
        let peak = one_band(PEAK, 1_000.0, 1200, 25412, 0);
        let mut typed = peak;
        typed[KIND] = OFF;
        let switched = saved(8, &peak, PLAIN, 0x40, 0x7e);
        for mut plugin in [eq(&typed, PLAIN), with(&switched)] {
            let input = noise(2_000);
            let (mut left, mut right) = (input.clone(), input.clone());
            plugin.process(&mut left, &mut right);
            assert_eq!((bits(&left), bits(&right)), (bits(&input), bits(&input)));
        }
        assert!((measured(&mut eq(&peak, PLAIN), 1_000) - 12.0).abs() < 0.01);
        // In version 3 there is no such bit.
        let mut plugin = with(&saved(3, &peak, PLAIN, 0x40, 0x7e));
        assert!((measured(&mut plugin, 1_000) - 12.0).abs() < 0.01);
    }

    #[test]
    fn a_moved_parameter_is_the_same_plugin_as_its_state() {
        let mut plugin = eq(&DEFAULTS, PLAIN);
        let moves = [
            (1, -420.4),
            (FREQ + 1, 70_000.0),
            (WIDTH + 1, 9000.2),
            (KIND + 1, 3.6),
            (ORDER + 1, -9.0),
            (KIND + 4, 0.0),
            (MAIN, 2000.0),
        ];
        for (index, value) in moves {
            plugin.set_param(index, value);
        }
        plugin.set_param(36, 1.0);
        assert_eq!(plugin.param(36), 0.0);
        let mut params = DEFAULTS;
        for (index, value) in [(1, -420), (FREQ + 1, 65536), (WIDTH + 1, 9000)] {
            params[index] = value;
        }
        for (index, value) in [
            (KIND + 1, NOTCH),
            (ORDER + 1, -3),
            (KIND + 4, OFF),
            (MAIN, 1800),
        ] {
            params[index] = value;
        }
        // The main level is on its way. One block gets it there.
        let mut loaded = eq(&params, PLAIN);
        assert_ne!(plugin, loaded);
        for plugin in [&mut plugin, &mut loaded] {
            plugin.process(&mut [0.25; 4], &mut [0.25; 4]);
            plugin.reset();
        }
        assert_eq!(plugin, loaded);

        // The main level alone, with every band typed off: a ramp from
        // the old gain to the new one over the block.
        let mut params = DEFAULTS;
        params[KIND..ORDER].fill(OFF);
        let mut plugin = eq(&params, PLAIN);
        plugin.set_param(MAIN, -602.0);
        let (mut left, mut right) = ([1.0; 4], [-1.0; 4]);
        plugin.process(&mut left, &mut right);
        let low = main_gain(-602);
        assert!((low - 0.5).abs() < 1e-4);
        for (n, got) in left.iter().enumerate() {
            assert!((got - (1.0 + (low - 1.0) * n as f32 / 4.0)).abs() < 1e-6);
        }
        plugin.process(&mut left, &mut right);
        assert_eq!(right[3], -left[3]);
        let (mut left, mut right) = ([1.0; 2], [1.0; 2]);
        plugin.process(&mut left, &mut right);
        assert_eq!(left, [low; 2]);

        // A new type empties the band's filters, and a new level leaves
        // what they hold.
        let ringing = || {
            let mut plugin = eq(&one_band(LOW_PASS, 200.0, 0, 0, 0), PLAIN);
            plugin.process(&mut [1.0; 64], &mut [1.0; 64]);
            plugin
        };
        let filled =
            |plugin: &ParametricEq2| plugin.bands[0].state != <[[Biquad64; 2]; 4]>::default();
        let mut plugin = ringing();
        plugin.set_param(WIDTH, 30_000.0);
        assert!(filled(&plugin));
        plugin.set_param(ORDER, 1.0);
        assert!(!filled(&plugin));
        let mut plugin = ringing();
        plugin.set_param(KIND, f32::from(HIGH_PASS as i8));
        assert!(!filled(&plugin));
    }

    #[test]
    fn a_fifth_of_a_second_of_silence_puts_the_plugin_to_sleep() {
        // A low pass at 20 Hz with a Q of 10 rings for a long time.
        let params = one_band(LOW_PASS, 20.0, 0, 0, 0);
        let mut plugin = eq(&params, PLAIN);
        let click = |plugin: &mut ParametricEq2| {
            let (mut left, mut right) = ([0.0; 512], [0.0; 512]);
            (left[0], right[0]) = (1.0, 1.0);
            plugin.process(&mut left, &mut right);
            left
        };
        click(&mut plugin);
        // 8820 frames of silence in blocks of 512: the 18th block still
        // rings, and from the 19th on the input is passed on.
        for block in 1..=24 {
            let (mut left, mut right) = ([0.0; 512], [0.0; 512]);
            plugin.process(&mut left, &mut right);
            let loudest = left.iter().fold(0.0f32, |peak, v| peak.max(v.abs()));
            if block <= 18 {
                assert!(loudest > 1e-5, "block {block}: {loudest}");
            } else {
                assert_eq!(loudest, 0.0, "block {block}");
            }
        }
        // It wakes with what the low pass still held as it fell asleep,
        // as FL empties only the bands that are counting down: the ring
        // goes on where it stopped.
        let held = plugin.bands[0].state;
        assert_ne!(held, <[[Biquad64; 2]; 4]>::default());
        let mut fresh = eq(&params, PLAIN);
        fresh.bands[0].state = held;
        assert_eq!(bits(&click(&mut plugin)), bits(&click(&mut fresh)));
        // Asleep, a peak at 0 dB that has just been put there is emptied.
        let mut plugin = eq(&one_band(PEAK, 1_000.0, 600, 25412, 0), PLAIN);
        plugin.process(&mut [0.5; 64], &mut [0.5; 64]);
        plugin.process(&mut [0.0; 8_820], &mut [0.0; 8_820]);
        plugin.set_param(0, 0.0);
        assert_eq!(plugin.bands[0].tail, 8_820);
        assert_ne!(plugin.bands[0].state, <[[Biquad64; 2]; 4]>::default());
        plugin.process(&mut [0.0; 64], &mut [0.0; 64]);
        assert_eq!(plugin.awake, ASLEEP);
        assert_eq!(plugin.bands[0].state, <[[Biquad64; 2]; 4]>::default());
        // A main level moved while it sleeps is there at once when it
        // wakes, with no ramp.
        plugin.set_param(MAIN, -602.0);
        let (mut left, mut right) = ([1.0; 4], [1.0; 4]);
        plugin.process(&mut left, &mut right);
        assert_eq!(left, [main_gain(-602); 4]);
        // What is under a millionth of full scale is silence too.
        let mut plugin = eq(&params, PLAIN);
        plugin.process(&mut [9e-7; 512], &mut [-9e-7; 512]);
        assert_eq!(plugin.awake, ASLEEP);
        plugin.process(&mut [2e-6; 512], &mut [0.0; 512]);
        assert_eq!(plugin.awake, 8_820);
        // An empty block and sides of two lengths are taken as they come.
        plugin.process(&mut [], &mut [0.5; 3]);
        let (mut left, mut right) = ([0.5; 3], [0.5; 1]);
        plugin.process(&mut left, &mut right);
        assert_eq!(left[1..], [0.5; 2]);
    }

    #[test]
    fn the_resampler_is_a_half_band_filter() {
        // Going up, a tone keeps its size and its image at the other end
        // of the new band is 80 dB down or more, up to 18 kHz. Going up
        // and down again is unity.
        for (hz, floor) in [
            (100usize, -100.0),
            (1_000, -100.0),
            (10_000, -100.0),
            (18_000, -80.0),
        ] {
            let turns = hz as f64 / 44_100.0;
            let input = sine(turns, 2 * 44_100);
            let mut wide = vec![0.0; 4 * 44_100];
            HalfBand::default().up(&input, &mut wide);
            let settled = &wide[2 * 44_100..];
            assert!(
                (partial(settled, turns / 2.0) - 1.0).abs() < 1e-4,
                "{hz} Hz"
            );
            let image = 20.0 * partial(settled, 0.5 - turns / 2.0).log10();
            assert!(image < floor, "{hz} Hz: image at {image} dB");
            let mut back = vec![0.0; 2 * 44_100];
            HalfBand::default().down(&wide, &mut back);
            assert!(
                (partial(&back[44_100..], turns) - 1.0).abs() < 1e-4,
                "{hz} Hz"
            );
        }
        // A level that stands still goes through both ways as it is.
        let mut wide = [0.0; 400];
        HalfBand::default().up(&[0.5; 200], &mut wide);
        assert!(wide[300..].iter().all(|v| (v - 0.5).abs() < 1e-6));
    }

    #[test]
    fn the_engine_s_resampler_is_its_half_band_fir_44_frames_late() {
        // The taps, to the bit, as the DLL builds them: a sinc of half
        // the band under a half sine, centred on tap 89. The taps an even
        // way from the centre are what is left of sin(k pi).
        let taps = half_band();
        for (tap, want) in [
            (0, 0x0000_0000u32),
            (1, 0xa1ba_bcd7),
            (2, 0xb908_eea8),
            (3, 0x2200_e8f5),
            (87, 0x23b3_be28),
            (88, 0x3ea2_f983),
            (89, 0x3eff_f58f),
            (90, 0x3ea2_deee),
            (91, 0x23b3_8385),
            (174, 0x390c_2779),
            (175, 0x212b_f3f9),
        ] {
            assert_eq!(taps[tap].to_bits(), want, "tap {tap}");
        }
        assert_eq!(taps[89], (0.5 * (89.0 * PI / 176.0).sin()) as f32);
        let sum: f64 = taps.iter().map(|tap| f64::from(*tap)).sum();
        assert!((sum - 0.999_841_4).abs() < 1e-7, "{sum}");

        // Up is each sample twice over and a 0, so with no band in the
        // way a click comes out as the odd taps doubled: the centre one
        // 44 frames on, the rest under 1e-15.
        let mut params = DEFAULTS;
        params[KIND..ORDER].fill(OFF);
        let mut plugin = with(&saved(8, &params, TWICE, 0x42, 0x7f));
        assert_eq!(plugin.latency(), 44);
        let (mut left, mut right) = ([0.0; 100], [0.0; 100]);
        (left[0], right[0]) = (1.0, -1.0);
        plugin.process(&mut left, &mut right);
        for (n, (l, r)) in left.iter().zip(&right).enumerate() {
            let want = if n < 88 { 2.0 * taps[2 * n + 1] } else { 0.0 };
            assert_eq!((l.to_bits(), *r), (want.to_bits(), -want), "frame {n}");
            if n != 44 {
                assert!(l.abs() < 1e-15, "frame {n}");
            }
        }
        assert_eq!(left[44], 2.0 * taps[89]);

        // A tone passes at its size below 18 kHz, and it is the same a
        // block at a time as in one go: the ring carries on.
        for hz in [100usize, 1_000, 10_000, 18_000] {
            let mut plugin = with(&saved(8, &params, TWICE, 0x42, 0x7f));
            let got = measured(&mut plugin, hz);
            assert!(got.abs() < 0.01, "{hz} Hz: {got} dB");
        }
        let input = noise(3_000);
        let mut whole = with(&saved(8, &params, TWICE, 0x42, 0x7f));
        let mut blocks = whole.clone();
        let (mut left, mut right) = (input.clone(), input.clone());
        whole.process(&mut left, &mut right);
        let (mut l2, mut r2) = (input.clone(), input.clone());
        for (l, r) in l2.chunks_mut(77).zip(r2.chunks_mut(77)) {
            blocks.process(l, r);
        }
        assert_eq!(bits(&left), bits(&l2));
        // A peak at twice the rate is the same filter by either way up.
        let peak = one_band(PEAK, 1_000.0, 600, 25412, 0);
        let mut plugin = with(&saved(8, &peak, TWICE, 0x42, 0x7f));
        let got = measured(&mut plugin, 1_000);
        assert!((got - 6.0).abs() < 0.01, "{got}");
        // The rest of HQ runs on time.
        for (flags, bits) in [(TWICE, 0x40), (PLAIN, 0x42), (MATCHED, 0x42)] {
            assert_eq!(with(&saved(8, &peak, flags, bits, 0x7f)).latency(), 0);
        }
    }

    #[test]
    fn fl_s_presets_load_as_the_doc_reads_them() {
        let Some(presets) = presets(NAME) else { return };
        let names: Vec<&str> = presets.iter().map(|preset| preset.file.as_str()).collect();
        let want = [
            "20Hz + 18kHz cut",
            "3 bands",
            "30Hz + 18kHz cut",
            "40Hz + 18kHz cut",
            "40Hz cut",
            "Default",
            "Kick",
            "Old Telephone",
            "Peaking only",
            "Quick Low Roll-Off",
            "Radio",
            "Tight Drum Sizzle",
            "Vocal Presence",
        ];
        assert_eq!(names, want);
        let load = |file: &str| {
            let preset = presets.iter().find(|preset| preset.file == file).unwrap();
            assert_eq!(preset.plugin, NAME);
            with(&preset.state)
        };
        // Version 6 at 350 bytes with HQ off, or version 8 at 354 with the
        // matched filters.
        for preset in &presets {
            let plugin = load(&preset.file);
            let old = preset.state.len() == 350;
            assert_eq!(preset.state.len(), if old { 350 } else { 354 });
            assert_eq!(
                (plugin.hq, plugin.mode),
                if old { (false, 0) } else { (true, 1) }
            );
            assert_eq!((plugin.enabled, plugin.linear_phase), (0x7f, false));
        }
        assert_eq!(load("Default"), ParametricEq2::default());

        // "40Hz cut": a high pass at a nominal 32 Hz with the default Q
        // of 1, Steep 8. Its 3 dB point is where it says.
        let cut = load("40Hz cut");
        let band = cut.knobs(0);
        assert_eq!((band.kind, band.order, band.width), (HIGH_PASS, -3, 36068));
        assert_eq!(hertz(band.freq).round(), 32.0);
        assert_eq!(cut.fidelity(), Fidelity::Exact);
        // "20Hz + 18kHz cut": the wheel all the way down is 20 Hz, and
        // the last band is a low pass at 18.7 kHz, both Steep 8.
        let both = load("20Hz + 18kHz cut");
        let (low, high) = (both.knobs(0), both.knobs(6));
        assert_eq!(
            (low.kind, low.order, hertz(low.freq)),
            (HIGH_PASS, -3, 20.0)
        );
        assert_eq!((high.kind, high.order), (LOW_PASS, -3));
        assert_eq!(hertz(high.freq).round(), 18_705.0);
        // "Kick": three peaks, and another state in the spare.
        let mut kick = load("Kick");
        assert_eq!(kick.params[KIND..ORDER], [6, 6, 6, 0, 0, 0, 0]);
        assert_eq!(kick.params[..3], [260, -270, 100]);
        assert_eq!((kick.spare()[0], kick.spare()[KIND]), (-430, HIGH_PASS));
        assert!((measured(&mut kick, 48) - 2.6).abs() < 0.1);
        // "Old Telephone" and its like ask for the matched filters.
        let telephone = load("Old Telephone");
        assert!(telephone.matched() && telephone.fidelity() == Fidelity::Exact);
    }

    #[test]
    fn every_state_in_fl_s_projects_is_filed_by_what_it_asks_for() {
        let Some(root) = corpus() else {
            eprintln!("FL_RS_CORPUS unset, the states in FL's projects skipped");
            return;
        };
        // How many states ask for what, and how many of them a stand-in
        // does not change, since no band of theirs does a thing.
        let mut counts = std::collections::BTreeMap::new();
        let mut total = 0;
        for saved in projects(&root) {
            if !saved.plugin.eq_ignore_ascii_case(NAME) {
                continue;
            }
            let plugin = with(&saved.state);
            let asks = if plugin.linear_phase {
                "linear phase"
            } else if plugin.hq && plugin.mode == 1 {
                "matched"
            } else if plugin.oversampled() && plugin.engine_resampler {
                "engine resampler"
            } else if plugin.oversampled() {
                "twice the rate"
            } else {
                "the rate"
            };
            let exact = plugin.fidelity() == Fidelity::Exact;
            *counts.entry((asks, exact)).or_insert(0) += 1;
            total += 1;
        }
        // 1130 of the 1138 are FL's own filters, and the 8 with linear
        // phase are stood in for.
        let want = [
            (("engine resampler", true), 204),
            (("linear phase", false), 8),
            (("matched", true), 61),
            (("the rate", true), 274),
            (("twice the rate", true), 591),
        ];
        assert_eq!(counts.into_iter().collect::<Vec<_>>(), want);
        assert_eq!(total, 1138);
    }
}
