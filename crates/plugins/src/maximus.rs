//! Maximus: FL's three-band maximizer, as `docs/plugins/maximus.md` has it.
//!
//! A two-way crossover splits the input into LOW, MID and HIGH. Each band
//! runs the same engine: a peak or RMS detector over a window, release
//! stages on the level, a drawn curve that turns the level into a gain,
//! attack stages on the gain, a lookahead delay on the sound, the gain,
//! then a saturator. The three bands sum into a MASTER band with the same
//! engine. The band mix can be blended against a delayed dry path.
//!
//! The state is a version word, 7 global and 4 x 13 band parameters as
//! i32, a spare copy of them for the A/B switch, a tail of what the display
//! shows (of which the solo switch, the oversampling order and the two
//! crossover flags matter here), from version 14 on nine more bytes and
//! five i32 of monitor settings, from 15 on one more i32, and then eight
//! drawn curves: a compression curve and an envelope curve for each band.
//! Versions 7 to 13 are 1342 bytes with the default curves, 14 is 1371 and
//! 15 is 1375. Soundgoodizer's presets are version 4, 1333 bytes.
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 LOW frequency, `30 * 533.3^(v / 65536)` Hz | 0 to 65536 | 19800 (200 Hz) |
//! | 1 LOW slope, 2 or 4 poles | 0 to 1 | 1 |
//! | 2 HIGH frequency | 0 to 65536 | 48065 (3 kHz) |
//! | 3 HIGH slope | 0 to 1 | 1 |
//! | 4 LMH lookahead delay, a time wheel | 0 to 10000 | 1115 (2 ms) |
//! | 5 LMH mix | 0 to 1000 | 1000 |
//! | 6 Low cut, `5 * 16^(v / 65536)` Hz, -1 off | -1 to 65536 | 32768 (20 Hz) |
//! | 7 + 13 b + 0 Mode: compress, bypass, mute, off | 0 to 3 | 0 |
//! | + 1 Pre gain, `(7^x - 1) / 6` | 0 to 2000 | 1000 |
//! | + 2 Post gain | 0 to 2000 | 1000 |
//! | + 3 Stereo separation | -1000 to 1000 | 0 |
//! | + 4 Saturation threshold | -1000 to 1000 | 0 |
//! | + 5 Saturation ceiling, `(7^x - 1) / 6` | 0 to 2000 | 1000 |
//! | + 6 Attack time | 0 to 10000 | 1115 |
//! | + 7 Release 2 time | 0 to 10000 | 0 |
//! | + 8 Attack and release 2 curve, stages | 1 to 8 | 2 |
//! | + 9 Release time | 0 to 10000 | 5000 |
//! | + 10 Release curve, stages | 1 to 8 | 3 |
//! | + 11 Sustain window | 0 to 10000 | 2882 |
//! | + 12 Sustain mode, peak or RMS | 0 to 1 | 0 |
//!
//! The time, gain and stage curves are Fruity Limiter's, and so are the
//! detectors and the lookahead. A curve's x is the level: `x = 0.5` is
//! 0 dBFS and `x = 1` is +12 dB, on `level = (3^(2x) - 1) / 2`; its y is
//! the output on the same scale, times the ceiling of 0.975. The curve is
//! read into a table of 32769 gains, one for every 1/8192 of level up to 4,
//! and the render looks the pre-gained level up in it. Where the curve
//! reaches the ceiling the table ends, and above it the gain is the curve's
//! last output over the level: a brickwall.
//!
//! The crossover is Linkwitz-Riley: a slope of 0 is one 2-pole section a
//! band with `Q = 0.5` and the LOW and HIGH bands inverted, a slope of 1
//! two Butterworth sections. The LOW band gets an allpass at the HIGH
//! frequency so the three sum flat. A state can ask for an FIR crossover
//! instead: windowed sincs of `0.004 * rate` taps that sum to a delta, and
//! half of that length of latency.
//!
//! The curves are the envelopes FL's plugins share, read by
//! [`crate::menvelope`] and drawn by [`dsp::curve`].
//!
//! Oversampling (an order of 1 in 19 shipped states) runs the crossover
//! and the three bands at twice the rate or more, between the stages of
//! FL's polyphase half band, the one Parametric EQ 2 has, and their times
//! are rounded at the project rate and multiplied. The master runs at the
//! project rate.
//!
//! The linear-phase crossover (four shipped states) draws each band's
//! magnitude from the wheels as an analytic -6 dB crossover and hands it
//! to a context of FL's `engine_x64.dll` convolver ([`crate::linear_phase`]),
//! which filters the band `3 N / 4` frames late and fades a new magnitude
//! in over a set time.
//!
//! What a state can ask for that is stood in for, which [`Effect::fidelity`]
//! says: an oversampling order past 3 or a linear-phase precision past 7,
//! which no shipped state has.

use crate::limiter::{Highest, Mean, coef, gain};
use crate::linear_phase::{LinearPhase, WINDOWS, fft_size};
use crate::menvelope::{Class, MEnvelope, Reader};
use crate::parts::Matrix;
use crate::{Effect, Fidelity, StateError, knob_value, state_version};
use dsp::biquad::{Biquad64, Coefficients64};
use dsp::curve::{Curve, Point};
use dsp::halfband::{Oversampler, WIDEST};
use std::f64::consts::{PI, TAU};

/// The plugin's event 201 name.
pub const NAME: &str = "Maximus";

const BANDS: usize = 4;
const MASTER: usize = 3;
const GLOBALS: usize = 7;
const PER_BAND: usize = 13;
/// How many parameters the plugin has.
pub const PARAMS: usize = GLOBALS + BANDS * PER_BAND;

/// The global parameters.
const LOW_FREQ: usize = 0;
const LOW_SLOPE: usize = 1;
const HIGH_FREQ: usize = 2;
const HIGH_SLOPE: usize = 3;
const LMH_DELAY: usize = 4;
const LMH_MIX: usize = 5;
const LOW_CUT: usize = 6;

/// A band's parameters, from its first.
const MODE: usize = 0;
const PRE: usize = 1;
const POST: usize = 2;
const SEPARATION: usize = 3;
const SAT_THRESHOLD: usize = 4;
const SAT_CEILING: usize = 5;
const ATTACK: usize = 6;
const RELEASE_2: usize = 7;
const ATTACK_STAGES: usize = 8;
const RELEASE: usize = 9;
const RELEASE_STAGES: usize = 10;
const WINDOW: usize = 11;
const ENV_MODE: usize = 12;

const BAND_RANGES: [(i32, i32); PER_BAND] = [
    (0, 3),
    (0, 2000),
    (0, 2000),
    (-1000, 1000),
    (-1000, 1000),
    (0, 2000),
    (0, 10_000),
    (0, 10_000),
    (1, 8),
    (0, 10_000),
    (1, 8),
    (0, 10_000),
    (0, 1),
];
const BAND_DEFAULTS: [i32; PER_BAND] = [0, 1000, 1000, 0, 0, 1000, 1115, 0, 2, 5000, 3, 2882, 0];

/// The range of each parameter's control.
const RANGES: [(i32, i32); PARAMS] = {
    let mut ranges = [(0, 65_536); PARAMS];
    ranges[LOW_SLOPE] = (0, 1);
    ranges[HIGH_SLOPE] = (0, 1);
    ranges[LMH_DELAY] = (0, 10_000);
    ranges[LMH_MIX] = (0, 1000);
    ranges[LOW_CUT] = (-1, 65_536);
    let mut i = GLOBALS;
    while i < PARAMS {
        ranges[i] = BAND_RANGES[(i - GLOBALS) % PER_BAND];
        i += 1;
    }
    ranges
};
const DEFAULTS: [i32; PARAMS] = {
    let mut defaults = [0; PARAMS];
    defaults[LOW_FREQ] = 19_800;
    defaults[LOW_SLOPE] = 1;
    defaults[HIGH_FREQ] = 48_065;
    defaults[HIGH_SLOPE] = 1;
    defaults[LMH_DELAY] = 1115;
    defaults[LMH_MIX] = 1000;
    defaults[LOW_CUT] = 32_768;
    let mut i = GLOBALS;
    while i < PARAMS {
        defaults[i] = BAND_DEFAULTS[(i - GLOBALS) % PER_BAND];
        i += 1;
    }
    defaults
};

/// The default compression curve: 1:1 up to 0 dBFS, then the ceiling.
const DEFAULT_CURVE: [(f64, f32, f32); 3] = [(0.0, 0.0, 0.0), (0.5, 0.5, 0.0), (0.5, 0.5, 0.0)];

/// The state: where the parameters start, and how long the block of
/// parameters, spare copy and display is in each version. Offsets inside
/// that block are the plugin's own field offsets less `0x12c`.
const PARAMS_AT: usize = 4;
const SHOWN_AT: usize = 0x1dc;
const SOLO_AT: usize = 0x1eb;
const OVERSAMPLE_AT: usize = 0x1f0;
const MID_FROM_INPUT_AT: usize = 0x1f8;
const FIR_AT: usize = 0x1f9;
/// Version 14 adds 29 bytes after the block, 15 four more.
const V14_EXTRA: usize = 29;
const V15_EXTRA: usize = 4;
/// How long the state is with the default curves, for a `Length` error.
const V15_LEN: usize = 1375;

/// How many entries the gain table has, one per 1/8192 of level to 4.0,
/// and the two ceilings a state can pick.
const TABLE: usize = 32_768;
const LEVEL_STEP: f64 = 1.0 / 8192.0;
const CEILINGS: [f64; 2] = [0.975, 1.0];
/// Under this a table entry counts as at the ceiling, and a stage's state
/// as nothing.
const EPSILON: f64 = 5.960_464_5e-8;

/// The most stages a curve wheel asks for.
const STAGES: usize = 8;

/// A block with no sample above this is silence.
const QUIET: f32 = 1e-6;

/// The highest oversampling order run as FL runs it: eight times. The
/// corpus and the presets ask for 0 or 1.
const MAX_ORDER: usize = 3;

/// The low cut's four sections: their `1 / (2 Q)` at 0.8 of the wheel's
/// frequency.
const LOW_CUT_ALPHAS: [f64; 4] = [0.197_711_84, 0.619_327_72, 0.676_844_54, 1.079_356_9];

/// The frequency a crossover wheel comes to: 30 Hz to 16 kHz.
fn crossover_hz(value: i32) -> f64 {
    30.0 * (f64::from(value) * (533.333_333_333_333_4f64.ln() / 65_536.0)).exp()
}

/// The frequency the low cut wheel comes to: 5 Hz to 80 Hz.
fn low_cut_hz(value: i32) -> f64 {
    5.0 * (f64::from(value) * (16f64.ln() / 65_536.0)).exp()
}

/// The curve through points as the stream has them: the width of the
/// stretch that ends at each, its height, that stretch's tension and its
/// shape. FL keeps a height as a single.
fn curve(points: &[(f64, f32, f32, u8)]) -> Curve {
    let points: Vec<Point> = points
        .iter()
        .map(|(x, y, tension, mode)| Point {
            x: *x,
            y: f64::from(*y),
            tension: *tension,
            mode: *mode,
        })
        .collect();
    Curve::new(&points)
}

/// A curve from a preset's points, which are all plain.
fn plain_curve(points: &[(f64, f32, f32)]) -> Curve {
    let points: Vec<(f64, f32, f32, u8)> = points.iter().map(|(x, y, t)| (*x, *y, *t, 0)).collect();
    curve(&points)
}

/// A curve read into gains: entry `i` is the curve's output at a level of
/// `i / 8192` over that level.
#[derive(Debug, Clone, PartialEq)]
struct Table {
    gains: Vec<f32>,
    /// The last entry the render reads from the table, the curve's output
    /// at the top of the table, and the curve's highest output over the
    /// ceiling.
    last: usize,
    top: f64,
    peak: f64,
}

impl Default for Table {
    fn default() -> Self {
        Table {
            gains: Vec::with_capacity(TABLE + 2),
            last: 0,
            top: 0.0,
            peak: 0.0,
        }
    }
}

impl Table {
    /// Read `curve` with its output scaled to `ceiling`.
    ///
    /// Where the output comes to the ceiling the table's last entry stays
    /// at the entry before, so the render's brickwall takes over from
    /// there. The entry after the last repeats it, for the render's
    /// interpolation.
    fn build(&mut self, curve: &Curve, ceiling: f64) {
        let ln3 = 3f64.ln();
        self.gains.clear();
        let (mut kept, mut peak, mut out) = (0, 0.0f64, 0.0);
        for i in 0..=TABLE {
            let level = i.max(1) as f64 * LEVEL_STEP;
            let x = (level * 2.0 + 1.0).ln() * (0.5 / ln3);
            let y = curve.value_at(x);
            out = ((y * 2.0 * ln3).exp() - 1.0) * ceiling * 0.5;
            let mut this = i;
            if (out - ceiling).abs() < EPSILON {
                out = ceiling;
                this = kept;
            }
            peak = peak.max(out);
            self.gains.push((out / level) as f32);
            kept = this;
        }
        self.gains.truncate(kept + 1);
        self.gains.push(self.gains[kept]);
        self.last = kept;
        self.top = out;
        self.peak = peak / ceiling;
    }

    /// The gain for a detector level with `pre` on it, and `post` after
    /// the curve.
    fn gain(&self, level: f32, pre: f32, post: f32) -> f32 {
        let place = level * (pre as f64 * 8192.25) as f32;
        let index = place as usize;
        if index > self.last {
            (self.top * f64::from(post)) as f32 / level
        } else {
            let (a, b) = (self.gains[index], self.gains[index + 1]);
            (a + (b - a) * (place - index as f32)) * pre * post
        }
    }
}

/// One-pole stages in a row, each keeping `rise` of its way to an input
/// above it and `fall` of its way to one below.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Chain {
    count: usize,
    rise: f64,
    fall: f64,
    state: [f64; STAGES],
}

impl Chain {
    fn run(&mut self, input: f32) -> f32 {
        let mut x = f64::from(input);
        for state in &mut self.state[..self.count] {
            let keep = if x > *state { self.rise } else { self.fall };
            *state = x + (*state - x) * keep;
            x = f64::from(*state as f32);
        }
        x as f32
    }

    /// What FL does after every block: a state too small to hear is 0.
    fn flush(&mut self) {
        for state in &mut self.state[..self.count] {
            if state.abs() < EPSILON {
                *state = 0.0;
            }
        }
    }
}

/// A ring of stereo frames held back.
#[derive(Debug, Clone, PartialEq, Default)]
struct Delay {
    frames: Vec<[f32; 2]>,
    at: usize,
}

impl Delay {
    fn start(&mut self, len: usize) {
        self.frames.clear();
        self.frames.resize(len, [0.0; 2]);
        self.at = 0;
    }

    fn push(&mut self, frame: [f32; 2]) -> [f32; 2] {
        if self.frames.is_empty() {
            return frame;
        }
        let out = std::mem::replace(&mut self.frames[self.at], frame);
        self.at = (self.at + 1) % self.frames.len();
        out
    }
}

/// One second-order section on a stereo signal, in double precision.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Section {
    coefficients: Coefficients64,
    state: [Biquad64; 2],
}

impl Section {
    fn run(&mut self, frame: [f32; 2]) -> [f32; 2] {
        let c = &self.coefficients;
        [
            self.state[0].process(c, f64::from(frame[0])) as f32,
            self.state[1].process(c, f64::from(frame[1])) as f32,
        ]
    }

    fn reset(&mut self) {
        self.state = [Biquad64::default(); 2];
    }

    fn flush(&mut self) {
        for state in &mut self.state {
            state.flush();
        }
    }
}

/// What a crossover section is: a low pass, a high pass or an allpass.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Low,
    High,
    All,
}

/// A section at `hz` with the Q of a slope index: `2^(-1 / (slope + 1))`,
/// 0.5 for one section a band and the Butterworth 0.707 for two.
fn section(kind: Kind, hz: f64, slope: i32, rate: f64) -> Section {
    let w = hz * TAU / rate;
    let (sin, cos) = w.sin_cos();
    let alpha = match slope {
        0 => sin,
        1 => sin * std::f64::consts::FRAC_1_SQRT_2,
        _ => sin / (0.5f64.powf(1.0 / f64::from(slope + 1)) * 2.0),
    };
    let coefficients = match kind {
        Kind::Low => Coefficients64::low_pass(cos, alpha),
        Kind::High => Coefficients64::high_pass(cos, alpha),
        Kind::All => Coefficients64::all_pass(cos, alpha),
    };
    Section {
        coefficients,
        state: [Biquad64::default(); 2],
    }
}

/// Run a stereo frame through sections in a row.
fn run(sections: &mut [Section], mut frame: [f32; 2]) -> [f32; 2] {
    for section in sections {
        frame = section.run(frame);
    }
    frame
}

/// A time wheel in frames at `rate`, unrounded, as `FUN_008415c0` works
/// it: `float(501^(float(v) * 1e-4f) - 1) * 2 ms`, the power by `exp` and
/// `ln` in doubles.
fn wheel_frames(value: i32, rate: f64) -> f64 {
    let x = f64::from(value as f32 * 1e-4f32);
    let ms = f64::from(((x * 501f64.ln()).exp() - 1.0) as f32) * 2.0;
    ms * 0.001 * rate
}

/// The linear-phase curves' quick log2 (`FUN_008477b0`): the exponent and
/// a cubic in doubles on one less the mantissa.
fn quick_log2(x: f32) -> f32 {
    let bits = x.to_bits();
    let exponent = (bits >> 23) as i32 - 127;
    let mantissa = (bits & 0x7f_ffff) as i32 as f32;
    let t = 1.0f32 - mantissa * f32::from_bits(0x3400_0000);
    let t = f64::from(t);
    let mut cubic = t * -0.155_650_1;
    cubic *= t;
    cubic *= t;
    let mut square = t * 0.106_315_7;
    square *= t;
    cubic -= square;
    cubic -= t * 0.739;
    cubic += 1.0;
    exponent as f32 + cubic as f32
}

/// The linear-phase curves' quick `2^x`: a cubic in doubles on the
/// fraction times the power of two of the whole part, one over that for a
/// negative `x`, 1 at 0.
fn quick_exp2(x: f32) -> f32 {
    let at = |x: f32| -> f32 {
        let whole = x as i32;
        let fraction = f64::from(x - whole as f32);
        let power = f64::from(f32::from_bits(((whole + 127) as u32) << 23));
        let cubic = fraction
            * (fraction * (fraction * 0.079_064_658_211 + 0.224_328_091_473) + 0.696_598_502_57)
            + 0.999_862_219_597;
        (power * cubic) as f32
    };
    if x > 0.0 {
        at(x)
    } else if x < 0.0 {
        1.0 / at(-x)
    } else {
        1.0
    }
}

/// One crossover curve of the linear-phase mode (`FUN_008477b0`): a low
/// pass at `hz`, or with `high` a high pass, -6 dB there and `slope` dB
/// an octave away from it on the stop side, one less the mirrored stop
/// curve on the pass side, at `f` Hz.
fn crossover_curve(hz: f32, slope: f32, f: f32, high: bool) -> f32 {
    let mut octaves = quick_log2(f / hz);
    if high {
        octaves = -octaves;
    }
    let level = |octaves: f32| -> f32 {
        let db = (-6.0 - f64::from(slope * octaves)) as f32;
        if -100.0 > db {
            0.0
        } else if db > 100.0 {
            2f32.powf(db * (1.0 / 6.0f32))
        } else {
            quick_exp2(db * (1.0 / 6.0f32))
        }
    };
    if octaves >= 0.0 {
        level(octaves)
    } else {
        (1.0 - f64::from(level(-octaves))) as f32
    }
}

/// What the linear-phase crossover is built from: the wheels as FL's
/// `FUN_00847e50` reads them, in floats.
#[derive(Debug, Clone, Copy, PartialEq)]
struct LinearSetting {
    low_hz: f32,
    high_hz: f32,
    low_slope: f32,
    high_slope: f32,
    /// The low cut's corner, when it is in the curves.
    low_cut: Option<f32>,
    /// LOW and HIGH switched off, which takes them out of MID's curve.
    low_off: bool,
    high_off: bool,
    /// Which of the three bands have a curve (an IIR crossover with
    /// sections, in FL's test); a band without one gets `2^(1/6)`.
    curved: [bool; 3],
    /// The FFT size and the band rate it is for.
    n: usize,
    rate: i32,
}

impl LinearSetting {
    /// The magnitude of band `b` on the bins `0..=n/2` into `into`
    /// (`FUN_00847e50`): the band's curve times the low cut's, which is
    /// left out from the first bin where it reaches 1; the bins over
    /// `0.4999` of the rate at 1; the last bin a copy of the one before.
    fn magnitudes(&self, b: usize, into: &mut [f32]) {
        let half = self.n / 2;
        let bin = (f64::from(self.rate) / self.n as f64) as f32;
        let top = (f64::from(self.rate) * 0.4999) as f32;
        let off = quick_exp2(f32::from_bits(0x3e2a_aaab));
        let mut cutting = self.low_cut.is_some();
        for (k, out) in into[..=half].iter_mut().enumerate() {
            let f = k as f32 * bin;
            let f = if 0.1f32 > f { 0.1f32 } else { f };
            if f > top {
                *out = 1.0;
                continue;
            }
            let band = if self.curved[b] {
                match b {
                    0 => crossover_curve(self.low_hz, self.low_slope, f, false),
                    1 if self.high_off => crossover_curve(self.low_hz, self.low_slope, f, true),
                    1 if self.low_off => crossover_curve(self.high_hz, self.high_slope, f, false),
                    1 => {
                        crossover_curve(self.low_hz, self.low_slope, f, true)
                            * crossover_curve(self.high_hz, self.high_slope, f, false)
                    }
                    _ => crossover_curve(self.high_hz, self.high_slope, f, true),
                }
            } else {
                off
            };
            let mut cut = 1.0;
            if let Some(hz) = self.low_cut
                && cutting
            {
                cut = crossover_curve(hz, 24.0, f, true);
                cutting = cut != 1.0;
            }
            *out = band * cut;
        }
        into[half] = into[half - 1];
    }
}

/// The FIR crossover: the three bands' finite impulse responses, all as
/// long, on one stereo history. Each channel's history holds every frame
/// twice, `len` apart, so the frames the taps read lie in one run.
#[derive(Debug, Clone, PartialEq, Default)]
struct Fir {
    taps: [Vec<f32>; 3],
    history: [Vec<f32>; 2],
    at: usize,
}

impl Fir {
    fn make_room(&mut self, longest: usize) {
        for taps in &mut self.taps {
            taps.reserve_exact(longest.saturating_sub(taps.len()));
        }
        for history in &mut self.history {
            history.reserve_exact((2 * longest).saturating_sub(history.len()));
        }
    }

    fn len(&self) -> usize {
        self.taps[0].len()
    }

    /// Take the bands' taps, LOW, MID and HIGH. The history goes on
    /// unless their length changes, when it starts from silence.
    fn set(&mut self, bands: [&[f64]; 3]) {
        let kept = self.len();
        for (taps, band) in self.taps.iter_mut().zip(bands) {
            taps.clear();
            taps.extend(band.iter().map(|tap| *tap as f32));
        }
        if self.len() != kept {
            self.reset();
        }
    }

    fn clear(&mut self) {
        for taps in &mut self.taps {
            taps.clear();
        }
        self.reset();
    }

    fn reset(&mut self) {
        let len = self.len();
        for history in &mut self.history {
            history.clear();
            history.resize(2 * len, 0.0);
        }
        self.at = 0;
    }

    /// Each band's output for the next frame. Every band and channel sums
    /// its taps in order from the newest frame, in doubles; the six sums
    /// run side by side.
    fn run(&mut self, frame: [f32; 2]) -> [[f32; 2]; 3] {
        let len = self.len();
        for (history, sample) in self.history.iter_mut().zip(frame) {
            history[self.at] = sample;
            history[self.at + len] = sample;
        }
        // Tap k reads the frame k before this one, at `at + len - k`.
        let window = self.at + 1..self.at + len + 1;
        let [left, right] = &self.history;
        let frames = left[window.clone()]
            .iter()
            .rev()
            .zip(right[window].iter().rev());
        let [low, mid, high] = &self.taps;
        let taps = low.iter().zip(mid).zip(high);
        let mut sums = [[0.0f64; 2]; 3];
        for (((low, mid), high), (left, right)) in taps.zip(frames) {
            let (left, right) = (f64::from(*left), f64::from(*right));
            for (sum, tap) in sums.iter_mut().zip([low, mid, high]) {
                let tap = f64::from(*tap);
                sum[0] += tap * left;
                sum[1] += tap * right;
            }
        }
        self.at = (self.at + 1) % len;
        sums.map(|[left, right]| [left as f32, right as f32])
    }
}

/// The taps of a windowed sinc low pass at `hz` over the rate, `len` of
/// them centred in a buffer of `total`. The window is the four-term
/// Blackman-Harris. FL does the sinc's distance from the centre in
/// singles.
fn sinc_low_pass(into: &mut [f64], total: usize, len: usize, cutoff: f64) {
    let cutoff = cutoff.min(0.5);
    let start = (total - len) / 2;
    let centre = (len - 1) / 2;
    for i in 0..len {
        let d = f64::from(i as f32 - (len - 1) as f32 * 0.5);
        into[start + i] = if i == centre {
            cutoff * 2.0
        } else {
            (TAU * cutoff * d).sin() / (d * PI)
        };
    }
}

/// Turn the taps of a low pass centred in `into` into a high pass: the
/// negative of them with one added at the centre.
fn invert(into: &mut [f64], total: usize) {
    for tap in into.iter_mut() {
        *tap = -*tap;
    }
    into[(total - 1) / 2] += 1.0;
}

/// Put the Blackman-Harris window of `len` points on the taps centred in a
/// buffer of `total`.
fn window(into: &mut [f64], total: usize, len: usize) {
    let start = (total - len) / 2;
    let step = TAU / (len - 1) as f64;
    for i in 0..len {
        let a = step * i as f64;
        let w = 0.35875 - 0.48829 * a.cos() + 0.14128 * (2.0 * a).cos() - 0.01168 * (3.0 * a).cos();
        into[start + i] *= w;
    }
}

/// The saturator of a band.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
enum Saturator {
    #[default]
    Off,
    /// The threshold wheel above 0: Fruity Limiter's knee, an exponential
    /// from `start` that never gets `span` above it.
    Soft { start: f32, span: f32, inverse: f32 },
    /// The wheel under 0: `ceiling * atan((|x| / ceiling)^k)^(1/k)`, scaled
    /// so that `expect` comes out at the ceiling, and the ceiling from
    /// there on. `k` runs from 20 at the wheel's -1 to 1 at its -1000. FL
    /// reads this off a table of 65537 points.
    Shaped {
        ceiling: f32,
        k: f64,
        expect: f32,
        norm: f64,
    },
}

impl Saturator {
    fn apply(&self, x: f32) -> f32 {
        match *self {
            Saturator::Off => x,
            Saturator::Soft {
                start,
                span,
                inverse,
            } => {
                let size = x.abs();
                if size > start {
                    let bent = f64::from(start)
                        + f64::from(span) * (1.0 - f64::from((start - size) * inverse).exp());
                    (bent as f32).copysign(x)
                } else {
                    x
                }
            }
            Saturator::Shaped {
                ceiling,
                k,
                expect,
                norm,
            } => {
                let size = f64::from(x.abs().min(expect) / ceiling);
                let bent = size.powf(k).atan().powf(1.0 / k) * norm * f64::from(ceiling);
                (bent as f32).copysign(x)
            }
        }
    }
}

/// One band: its settings, worked out from the parameters, and what the
/// frames so far left behind.
#[derive(Debug, Clone, PartialEq)]
struct Band {
    /// The mode as set, and the one it comes to after the solo switch:
    /// 0 compress, 1 pass with the post gain, 2 and 3 silent.
    mode: i32,
    effective: i32,
    pre: f32,
    post: f32,
    /// The sign the band goes into the sum with.
    sign: f32,
    delay: Delay,
    release: Chain,
    attack: Chain,
    rms_mode: bool,
    window: usize,
    peak: Highest,
    rms: Mean,
    table: Table,
    saturator: Saturator,
    matrix: Matrix,
    /// The gain the attack stages start from on a flush: pre, post and
    /// the curve at nothing.
    rest: f32,
}

impl Default for Band {
    fn default() -> Self {
        Band {
            mode: 0,
            effective: 0,
            pre: 1.0,
            post: 1.0,
            sign: 1.0,
            delay: Delay::default(),
            release: Chain::default(),
            attack: Chain::default(),
            rms_mode: false,
            window: 1,
            peak: Highest::default(),
            rms: Mean::default(),
            table: Table::default(),
            saturator: Saturator::Off,
            matrix: Matrix::new([1.0, 1.0, 0.0, 0.0]),
            rest: 1.0,
        }
    }
}

impl Band {
    fn make_room(&mut self, longest: usize) {
        self.delay
            .frames
            .reserve_exact(longest.saturating_sub(self.delay.frames.len()));
        self.peak.make_room(longest);
        self.rms.make_room(longest);
    }

    /// One frame through the band's engine, after the stereo separation:
    /// the sound held back and gained, and the sound held back alone. The
    /// band is in mode 0 or 1.
    fn run(&mut self, frame: [f32; 2]) -> ([f32; 2], [f32; 2]) {
        let mut gain = self.post;
        if self.effective == 0 {
            let level = if self.rms_mode {
                let mean = self.rms.push(frame[0] * frame[0] + frame[1] * frame[1]);
                (mean * 0.5).max(0.0).sqrt() as f32
            } else {
                self.peak.push(frame[0].abs().max(frame[1].abs()))
            };
            let level = self.release.run(level);
            gain = self.attack.run(self.table.gain(level, self.pre, self.post));
        }
        let dry = self.delay.push(frame);
        let wet = dry.map(|sample| self.saturator.apply(sample * gain));
        (wet, dry)
    }

    /// Forget the frames so far: FL's flush.
    fn flush(&mut self) {
        self.attack.state = [f64::from(self.rest); STAGES];
        self.release.state = [0.0; STAGES];
        self.delay.start(self.delay.frames.len());
        self.peak.start(self.window);
        self.rms.start(self.window);
        self.matrix.settle();
    }

    fn end_block(&mut self) {
        self.attack.flush();
        self.release.flush();
    }
}

/// What the display's part of the state and the version 14 flags hold
/// that the sound depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Switches {
    /// The band the display shows, and whether it is soloed.
    shown: i32,
    solo: bool,
    /// The oversampling order: 0 none, 1 twice, 2 four times.
    oversample: i32,
    /// The MID band is the input and not the crossover's middle.
    mid_from_input: bool,
    /// The crossover is the FIR one, or FL's linear-phase one.
    fir: bool,
    linear: bool,
    /// The linear-phase precision (`0x328`, the FFT is `512 << it`) and
    /// crossfade speed (`0x330`), from version 14.
    precision: i32,
    fade: i32,
    /// The curves' ceiling is 1.0 and not 0.975.
    whole_ceiling: bool,
}

impl Default for Switches {
    fn default() -> Self {
        Switches {
            shown: 3,
            solo: false,
            oversample: 0,
            mid_from_input: false,
            fir: false,
            linear: false,
            precision: 2,
            fade: 0,
            whole_ceiling: false,
        }
    }
}

/// Maximus. A new one has FL's defaults: three bands at 200 Hz and 3 kHz,
/// each a brickwall at the ceiling with 2 ms of lookahead, a 20 Hz low cut
/// and the master the same.
#[derive(Debug, Clone, PartialEq)]
pub struct Maximus {
    params: [i32; PARAMS],
    curves: [Curve; BANDS],
    switches: Switches,
    hz: f64,
    bands: [Band; BANDS],
    /// Which band the crossover's remainder goes to, which is the first
    /// that is not switched off.
    remainder: usize,
    /// The IIR crossover: the LOW band's sections, the MID band's high
    /// passes which the HIGH band's sections take over from, the MID
    /// band's low passes and the HIGH band's high passes.
    low: Vec<Section>,
    mid_high: Vec<Section>,
    mid_low: Vec<Section>,
    high: Vec<Section>,
    /// The FIR crossover.
    fir: Fir,
    /// The low cut's four sections, when it is on.
    low_cut: Vec<Section>,
    low_cut_on: bool,
    /// The halves that take the three bands to the band rate and back:
    /// FL's oversampler (`0x7cc` up, `0x7d4` down, `FUN_0083b340` and
    /// `FUN_0083b3e0`): `order` stages of the 2x polyphase half band
    /// (`FUN_0083ae10` up, `FUN_0083afe0` down, the 13 doubles at
    /// `0x008bb648`, which are Parametric EQ 2's) up before the crossover
    /// and as many down after the band sum. FL reports no latency for it.
    oversampler: Oversampler,
    /// The linear-phase crossover: one engine context a band (`0x834`),
    /// the inputs the magnitudes were last asked for with, which bands
    /// still have to send theirs (`0x120`), and room for one magnitude.
    contexts: [LinearPhase; 3],
    linear_dirty: [bool; 3],
    magnitudes: Vec<f32>,
    mix: f32,
    /// The frames the output is late by, and how many frames of silence
    /// put the plugin to sleep.
    latency: usize,
    hold: i64,
    countdown: i64,
    asleep: bool,
}

impl Default for Maximus {
    fn default() -> Self {
        let mut plugin = Maximus {
            params: DEFAULTS,
            curves: std::array::from_fn(|_| plain_curve(&DEFAULT_CURVE)),
            switches: Switches::default(),
            hz: 44_100.0,
            bands: std::array::from_fn(|_| Band::default()),
            remainder: 0,
            low: Vec::with_capacity(2 * STAGES),
            mid_high: Vec::with_capacity(STAGES),
            mid_low: Vec::with_capacity(STAGES),
            high: Vec::with_capacity(STAGES),
            fir: Fir::default(),
            low_cut: Vec::with_capacity(LOW_CUT_ALPHAS.len()),
            low_cut_on: false,
            oversampler: Oversampler::default(),
            contexts: Default::default(),
            linear_dirty: [false; 3],
            magnitudes: Vec::new(),
            mix: 1.0,
            latency: 0,
            hold: 0,
            countdown: 0,
            asleep: false,
        };
        plugin.make_room();
        plugin.rebuild_tables();
        plugin.reset();
        plugin
    }
}

/// The sign a band of this slope goes into the sum with: one section a
/// band sums flat with the outer bands inverted.
fn sign(slope: i32) -> f32 {
    if slope == 0 { -1.0 } else { 1.0 }
}

impl Maximus {
    /// Have the memory for the longest time a wheel can ask for at this
    /// rate and oversampling, a second, and for the longest FIR.
    fn make_room(&mut self) {
        let longest = (wheel_frames(10_000, self.hz).round_ties_even() as usize).max(1);
        let os = self.os();
        for (b, band) in self.bands.iter_mut().enumerate() {
            band.make_room(if b == MASTER { longest } else { longest * os });
        }
        self.fir.make_room(self.fir_len());
        // The engine's op 2, which FL sends when it first sends a
        // magnitude: done here, off the audio thread.
        if self.switches.linear {
            let n = self.linear_size();
            for context in &mut self.contexts {
                context.set_size(n);
            }
            self.magnitudes.resize(n / 2 + 1, 0.0);
        }
    }

    /// The linear-phase FFT size at the band rate.
    fn linear_size(&self) -> usize {
        fft_size(self.switches.precision.clamp(0, 7), self.band_hz() as i32)
    }

    /// The oversampling order the three bands run at, and what it
    /// multiplies the rate by. An order past [`MAX_ORDER`] runs at that.
    fn order(&self) -> usize {
        self.switches.oversample.clamp(0, MAX_ORDER as i32) as usize
    }

    fn os(&self) -> usize {
        1 << self.order()
    }

    /// The rate the crossover and the three bands run at (FL's band
    /// renderer, `0xcc`): the project rate times the oversampling.
    fn band_hz(&self) -> f64 {
        self.hz * self.os() as f64
    }

    /// How many taps the FIR crossover has: 4 ms at the band rate, odd,
    /// five at least.
    fn fir_len(&self) -> usize {
        ((self.band_hz() * 0.004).round_ties_even() as usize | 1).max(5)
    }

    fn band_params(&self, band: usize) -> &[i32] {
        let at = GLOBALS + PER_BAND * band;
        &self.params[at..at + PER_BAND]
    }

    /// Read every band's curve into its table, for a new state.
    fn rebuild_tables(&mut self) {
        let ceiling = CEILINGS[usize::from(self.switches.whole_ceiling)];
        for (band, curve) in self.bands.iter_mut().zip(&self.curves) {
            band.table.build(curve, ceiling);
        }
    }

    /// Work out everything the parameters and the switches come to, as FL
    /// does when a parameter moves or a state loads. A detector, a delay
    /// or a chain of stages whose length changes starts from nothing.
    fn update(&mut self) {
        self.resolve_modes();
        self.design_crossover();
        self.design_low_cut();
        self.mix = self.params[LMH_MIX] as f32 * 0.001;
        for b in 0..BANDS {
            self.tune_band(b);
        }
        let band_delay = (0..MASTER).map(|b| self.bands[b].delay.frames.len()).max();
        let fir_half = if self.fir.len() == 0 {
            0
        } else {
            (self.fir_len() - 1) / 2
        };
        // The bands' frames are at the band rate (`FUN_00841340`), and
        // the linear-phase crossover is `3 N / 4` of them late.
        let os = self.os();
        let master = self.bands[MASTER].delay.frames.len();
        self.latency = if self.switches.linear {
            band_delay.unwrap_or(0) / os + master + self.linear_size() * 3 / 4 / os
        } else {
            (fir_half + band_delay.unwrap_or(0)) / os + master
        };
        self.hold = self.latency as i64 + (1.5 * self.hz).round_ties_even() as i64;
    }

    /// `FUN_00847e50`, at the start of each block: each band that has a
    /// new magnitude to send sends it once its context has finished its
    /// crossfade (ops 0xe, 5, 6, 2, 3).
    fn send_magnitudes(&mut self) {
        let setting = self.linear_setting();
        let window = WINDOWS[self.switches.fade.clamp(0, 3) as usize];
        for b in 0..3 {
            if !self.linear_dirty[b] || !self.contexts[b].settled() {
                continue;
            }
            self.linear_dirty[b] = false;
            let bins = setting.n / 2 + 1;
            if self.magnitudes.len() < bins || self.contexts[b].size() != setting.n {
                continue;
            }
            setting.magnitudes(b, &mut self.magnitudes);
            let context = &mut self.contexts[b];
            context.set_rate(setting.rate, window);
            context.set_kernel(&self.magnitudes[..bins]);
        }
    }

    /// Each band's mode after the solo switch: with a band soloed, every
    /// other band is silent, but the master, which passes.
    fn resolve_modes(&mut self) {
        let solo = self.switches.solo && self.switches.shown != MASTER as i32;
        for b in 0..BANDS {
            let p: [i32; PER_BAND] = self.band_params(b).try_into().unwrap();
            let band = &mut self.bands[b];
            band.mode = p[MODE];
            band.effective = p[MODE];
            if solo && b as i32 != self.switches.shown {
                band.effective = band.effective.max(if b == MASTER { 1 } else { 2 });
            }
            band.pre = gain(p[PRE]);
            band.post = gain(p[POST]);
            band.matrix
                .aim(separation_gains(p[SEPARATION] as f32 * 0.001));
        }
    }

    /// The crossover for the frequencies, the slopes and the modes: a band
    /// that is switched off has no sections, and the frequencies are put
    /// in order when both slopes count.
    fn design_crossover(&mut self) {
        let mut low_hz = crossover_hz(self.params[LOW_FREQ]);
        let mut high_hz = crossover_hz(self.params[HIGH_FREQ]);
        let slope = |band: usize, index: usize| {
            if self.band_params(band)[MODE] == 3 {
                -1
            } else {
                self.params[index]
            }
        };
        let (low_slope, high_slope) = (slope(0, LOW_SLOPE), slope(2, HIGH_SLOPE));
        if low_slope >= 0 && high_slope >= 0 && high_hz < low_hz {
            std::mem::swap(&mut low_hz, &mut high_hz);
        }
        let off = (0..MASTER).position(|b| self.band_params(b)[MODE] != 3);
        self.remainder = off.unwrap_or(1);

        let fir = self.switches.fir && !self.switches.linear;
        let (low, mid_high, mid_low, high) = (
            self.sections(Kind::Low, low_hz, low_slope, low_slope + 1),
            self.sections(Kind::High, low_hz, low_slope, low_slope + 1),
            self.sections(Kind::Low, high_hz, high_slope, high_slope + 1),
            self.sections(Kind::High, high_hz, high_slope, high_slope + 1),
        );
        let mut low = low;
        if high_slope > 0 {
            low.push(section(Kind::All, high_hz, 0, self.band_hz()));
        }
        // A section keeps the state of the one in its place before, as
        // FL's do when a frequency moves.
        for (old, mut new) in [
            (&mut self.low, low),
            (&mut self.mid_high, mid_high),
            (&mut self.mid_low, mid_low),
            (&mut self.high, high),
        ] {
            for (section, kept) in new.iter_mut().zip(old.iter()) {
                section.state = kept.state;
            }
            *old = new;
        }
        self.bands[0].sign = if fir || self.switches.linear {
            1.0
        } else {
            sign(low_slope)
        };
        self.bands[1].sign = 1.0;
        self.bands[2].sign = if fir || self.switches.linear {
            1.0
        } else {
            sign(high_slope)
        };

        if fir {
            let total = self.fir_len();
            let (os, rate) = (self.os() as f64, self.band_hz());
            let taps = |slope: i32, hz: f64| -> usize {
                let n = (os * 27_500.0 * f64::from(slope + 1) / hz).round_ties_even() as usize | 1;
                n.max(5).min(total)
            };
            let (n_low, n_high) = (taps(low_slope, low_hz), taps(high_slope, high_hz));
            let (f_low, f_high) = (low_hz / rate, high_hz / rate);
            let mut buffer = vec![0.0; total];
            let mut other = vec![0.0; total];
            sinc_low_pass(&mut buffer, total, n_low, f_low);
            window(&mut buffer, total, n_low);
            let low_taps = buffer.clone();
            sinc_low_pass(&mut other, total, n_high, f_high);
            invert(&mut other, total);
            window(&mut other, total, n_high);
            for (a, b) in buffer.iter_mut().zip(&other) {
                *a += b;
            }
            invert(&mut buffer, total);
            self.fir.set([&low_taps, &buffer, &other]);
        } else {
            self.fir.clear();
        }
    }

    /// `count` sections of a kind at `hz` with a slope's Q, none for a
    /// count under 1.
    fn sections(&self, kind: Kind, hz: f64, slope: i32, count: i32) -> Vec<Section> {
        (0..count.max(0))
            .map(|_| section(kind, hz, slope, self.band_hz()))
            .collect()
    }

    /// The linear-phase crossover's inputs, as `FUN_00847e50` reads them:
    /// the wheels in floats (not put in order), the slope switches as 12
    /// or 24 dB an octave, the low cut when its wheel is over 0 with its
    /// corner pulled down by `round(log_1.3(max(f - 5, 1)))` Hz, and the
    /// FFT size for the precision (`0x328`) at the band rate.
    fn linear_setting(&self) -> LinearSetting {
        let ln2 = 2f64.ln();
        let wheel = |value: i32, top: f64, base: f64| -> f32 {
            let per = top.ln() / (ln2 * 65_536.0);
            ((f64::from(value) * per * ln2).exp() * base) as f32
        };
        let low_cut = (self.params[LOW_CUT] > 0).then(|| {
            let hz = wheel(self.params[LOW_CUT], 16.0, 5.0);
            let over = if 1.0f32 > hz - 5.0 { 1.0 } else { hz - 5.0 };
            let pull = (f64::from(over).ln() / 1.3f64.ln()).round_ties_even();
            hz - pull as f32
        });
        let slope = |value: i32| if value == 0 { 12.0 } else { 24.0 };
        let mode = |b: usize| self.band_params(b)[MODE];
        let (low_off, high_off) = (mode(0) == 3, mode(2) == 3);
        let rate = self.band_hz() as i32;
        LinearSetting {
            low_hz: wheel(self.params[LOW_FREQ], 533.333_333_333_333_4, 30.0),
            high_hz: wheel(self.params[HIGH_FREQ], 533.333_333_333_333_4, 30.0),
            low_slope: slope(self.params[LOW_SLOPE]),
            high_slope: slope(self.params[HIGH_SLOPE]),
            low_cut,
            low_off,
            high_off,
            curved: [!low_off, !(low_off && high_off), !high_off],
            n: fft_size(self.switches.precision, rate),
            rate,
        }
    }

    fn design_low_cut(&mut self) {
        let wheel = self.params[LOW_CUT];
        self.low_cut_on = wheel >= 0 && !self.switches.linear;
        let w = low_cut_hz(wheel) * 0.8 * TAU / self.hz;
        let (sin, cos) = w.sin_cos();
        for (k, alpha) in LOW_CUT_ALPHAS.iter().enumerate() {
            let coefficients = Coefficients64::high_pass(cos, sin * alpha);
            match self.low_cut.get_mut(k) {
                Some(section) => section.coefficients = coefficients,
                None => self.low_cut.push(Section {
                    coefficients,
                    state: [Biquad64::default(); 2],
                }),
            }
        }
    }

    /// A band's delay, detector, stages and saturator for its parameters:
    /// FL's `FUN_008415c0` and `FUN_00841cc0`.
    fn tune_band(&mut self, b: usize) {
        let p: [i32; PER_BAND] = self.band_params(b).try_into().unwrap();
        let hz = self.hz;
        // The master runs at the project rate, the three bands at the band
        // rate. FL rounds the delay and the window at the project rate and
        // multiplies them by the oversampling; the stage times it keeps
        // unrounded, at the band rate.
        let os = if b == MASTER { 1 } else { self.os() };
        let at = |value: i32| wheel_frames(value, hz).round_ties_even() as usize * os;
        let exact = |value: i32| wheel_frames(value, hz) * os as f64;
        let compressing = (0..MASTER).any(|k| self.bands[k].effective == 0);
        let band = &mut self.bands[b];
        let held = band.effective < 2
            && (b != MASTER || band.effective < 1)
            && (b == MASTER || compressing);
        let lookahead = if held {
            at(if b == MASTER {
                p[ATTACK]
            } else {
                self.params[LMH_DELAY]
            })
        } else {
            0
        };
        if band.delay.frames.len() != lookahead {
            band.delay.start(lookahead);
        }

        let root = |stages: i32| f64::from(stages).sqrt();
        let attack = if p[ATTACK] == 0 {
            lookahead as f64
        } else {
            exact(p[ATTACK])
        };
        // FL's 0.8 is a single's.
        let attack = attack * f64::from(0.8f32) / root(p[ATTACK_STAGES]);
        let release_2 = if p[RELEASE_2] == 0 {
            attack
        } else {
            exact(p[RELEASE_2]) * 2.0 / root(p[ATTACK_STAGES])
        };
        let count = p[ATTACK_STAGES] as usize;
        if band.attack.count != count {
            band.attack.state = [1.0; STAGES];
        }
        band.attack.count = count;
        band.attack.rise = coef(release_2);
        band.attack.fall = coef(attack);

        let window = lookahead.max(1).max(at(p[WINDOW]));
        let rms = p[ENV_MODE] != 0;
        if band.window != window || band.rms_mode != rms {
            band.window = window;
            band.rms_mode = rms;
            band.peak.start(window);
            band.rms.start(window);
        }

        let release = exact(p[RELEASE]);
        let count = if release > 0.0 {
            p[RELEASE_STAGES] as usize
        } else {
            0
        };
        if band.release.count != count {
            band.release.state = [0.0; STAGES];
        }
        band.release.count = count;
        band.release.rise = 0.0;
        band.release.fall = coef(release * 2.0 / root(p[RELEASE_STAGES]));

        band.rest = band.pre * band.post * band.table.gains[0];
        band.saturator = saturator(&p, band);
    }

    /// Take a preset: its parameters and the compression curve of each
    /// band, with the switches at their defaults.
    pub(crate) fn load_preset(&mut self, params: [i32; PARAMS], curves: [&[(f64, f32, f32)]; 4]) {
        self.params = params;
        self.curves = curves.map(plain_curve);
        self.switches = Switches::default();
        self.rebuild_tables();
        self.reset();
    }

    /// One frame of input into the three crossover bands.
    fn split(&mut self, frame: [f32; 2]) -> [[f32; 2]; 3] {
        if self.switches.linear {
            // Each band's context on the input; FL's MID band takes the
            // input itself after its context has run when 0x324 is set.
            let bands: [[f32; 2]; 3] = std::array::from_fn(|b| self.contexts[b].frame(frame));
            if self.switches.mid_from_input {
                return [bands[0], frame, bands[2]];
            }
            return bands;
        }
        if self.fir.len() != 0 {
            return self.fir.run(frame);
        }
        let low = run(&mut self.low, frame);
        let high_passed = run(&mut self.mid_high, frame);
        if self.switches.mid_from_input {
            // FL's MID band is the plain input here, and its high passes
            // run on a copy, so the HIGH band's start from the input too.
            let high = run(&mut self.high, frame);
            run(&mut self.mid_low, high_passed);
            return [low, frame, high];
        }
        let mid = run(&mut self.mid_low, high_passed);
        let high = run(&mut self.high, high_passed);
        [low, mid, high]
    }

    /// One frame at the project rate through the plugin: the low cut,
    /// up to the band rate, the three bands, down again, the master. Each
    /// band's separation gains move on by their step after each of its
    /// frames.
    fn frame(
        &mut self,
        frame: [f32; 2],
        gains: &mut [[f32; 4]; BANDS],
        steps: &[[f32; 4]; BANDS],
    ) -> [f32; 2] {
        let frame = if self.low_cut_on {
            run(&mut self.low_cut, frame)
        } else {
            frame
        };
        let order = self.order();
        let sum = if order == 0 {
            self.bands_frame(frame, gains, steps)
        } else {
            let mut wide = [[0.0f32; 2]; WIDEST];
            self.oversampler.up(order, frame, &mut wide);
            for frame in wide.iter_mut().take(1 << order) {
                *frame = self.bands_frame(*frame, gains, steps);
            }
            self.oversampler.down(order, &mut wide)
        };
        let master = &mut self.bands[MASTER];
        let gain = gains[MASTER];
        for (g, s) in gains[MASTER].iter_mut().zip(&steps[MASTER]) {
            *g += s;
        }
        if master.effective >= 2 {
            return [0.0; 2];
        }
        let (l, r) = Matrix::mix(&gain, sum[0], sum[1]);
        master.run([l, r]).0
    }

    /// One frame at the band rate through the crossover and the three
    /// bands: their sum.
    fn bands_frame(
        &mut self,
        frame: [f32; 2],
        gains: &mut [[f32; 4]; BANDS],
        steps: &[[f32; 4]; BANDS],
    ) -> [f32; 2] {
        let split = self.split(frame);
        let (mix, mix_on) = (self.mix, self.mix != 1.0);
        let linear = self.switches.linear;
        let mut sum = [0.0f32; 2];
        for b in 0..MASTER {
            let band = &mut self.bands[b];
            if band.effective >= 2 {
                continue;
            }
            let [l, r] = split[b];
            let (l, r) = Matrix::mix(&gains[b], l, r);
            let (wet, dry) = band.run([l, r]);
            let sign = band.sign;
            for k in 0..2 {
                let out = if mix_on {
                    wet[k] * mix * sign + dry[k] * (1.0 - mix) * sign
                } else if sign == -1.0 && !linear {
                    -wet[k]
                } else {
                    wet[k]
                };
                sum[k] += out;
            }
        }
        for (gain, step) in gains.iter_mut().zip(steps).take(MASTER) {
            for (g, s) in gain.iter_mut().zip(step) {
                *g += s;
            }
        }
        sum
    }

    /// Forget the frames so far, as FL's flush does.
    fn flush(&mut self) {
        for band in &mut self.bands {
            band.flush();
        }
        self.oversampler.reset();
        // FL's flush ends with op 0xd to each context (`FUN_00846440`).
        if self.switches.linear {
            for context in &mut self.contexts {
                context.clear();
            }
        }
        let sections = (self.low.iter_mut())
            .chain(&mut self.mid_high)
            .chain(&mut self.mid_low)
            .chain(&mut self.high)
            .chain(&mut self.low_cut);
        for section in sections {
            section.reset();
        }
        self.fir.reset();
    }

    /// Read a state's parameters, switches and curves. `None` where the
    /// bytes are not a state of this version.
    fn parse(bytes: &[u8], version: i32) -> Option<Parsed> {
        let mut params = DEFAULTS;
        let mut switches = Switches::default();
        let int = |at: usize| -> Option<i32> {
            Some(i32::from_le_bytes(*bytes.get(at..)?.first_chunk()?))
        };
        let curves_at = if version < 3 {
            // Six globals, the bands, the spare copy and a short display tail.
            let tail = if version < 2 { 0x15 } else { 0x16 };
            let block = 0x18 + 0xd0 + 0x18 + 0xd0 + tail;
            for (k, param) in params.iter_mut().take(GLOBALS - 1).enumerate() {
                *param = int(PARAMS_AT + 4 * k)?;
            }
            for k in 0..BANDS * PER_BAND {
                params[GLOBALS + k] = int(PARAMS_AT + 0x18 + 4 * k)?;
            }
            let display = PARAMS_AT + block - tail;
            switches.shown = int(display + 4)?;
            switches.solo = *bytes.get(display + 0x13)? != 0;
            PARAMS_AT + block
        } else {
            let block = match version {
                3 => 0x1ee,
                4 => 0x1f1,
                5 => 0x1f5,
                6 => 0x1f9,
                _ => 0x1fa,
            };
            // Versions 11 to 13 wrote one i32 more than FL reads today.
            let block = if (11..=13).contains(&version)
                && curves_end(bytes, PARAMS_AT + block) != Some(bytes.len())
            {
                block + 4
            } else {
                block
            };
            for (k, param) in params.iter_mut().enumerate() {
                *param = int(PARAMS_AT + 4 * k)?;
            }
            let byte = |at: usize| -> u8 { bytes.get(PARAMS_AT + at).copied().unwrap_or(0) };
            let partial = |at: usize| -> i32 {
                let mut word = [0u8; 4];
                for (k, byte_at) in word.iter_mut().zip(at..at + 4) {
                    *k = if byte_at < block { byte(byte_at) } else { 0 };
                }
                i32::from_le_bytes(word)
            };
            switches.shown = partial(SHOWN_AT);
            switches.solo = byte(SOLO_AT) != 0;
            switches.oversample = partial(OVERSAMPLE_AT);
            switches.mid_from_input = version >= 6 && byte(MID_FROM_INPUT_AT) != 0;
            switches.fir = version >= 7 && byte(FIR_AT) != 0;
            let mut at = PARAMS_AT + block;
            if version >= 14 {
                switches.precision = int(at)?;
                switches.fade = int(at + 4)?;
                let flags = *bytes.get(at + 8)?;
                switches.linear = flags & 1 != 0;
                switches.whole_ceiling = flags & 0x40 != 0;
                at += V14_EXTRA;
            }
            if version >= 15 {
                at += V15_EXTRA;
            }
            at
        };
        let mut reader = Reader {
            bytes,
            at: curves_at,
        };
        let mut curves: [Curve; BANDS] = std::array::from_fn(|_| Curve::default());
        for curve in &mut curves {
            // The compression curve, then the envelope curve the sound
            // does not depend on.
            *curve = read_curve(&MEnvelope::read(&mut reader, Class::Map).ok()?);
            MEnvelope::read(&mut reader, Class::Map).ok()?;
        }
        let at = reader.at;
        Some(Parsed {
            params,
            switches,
            curves,
            end: at,
        })
    }
}

/// What a state holds, and where its curves end.
struct Parsed {
    params: [i32; PARAMS],
    switches: Switches,
    curves: [Curve; BANDS],
    end: usize,
}

/// Where the eight curves that start at `at` end.
fn curves_end(bytes: &[u8], at: usize) -> Option<usize> {
    let mut reader = Reader { bytes, at };
    for _ in 0..2 * BANDS {
        MEnvelope::read(&mut reader, Class::Map).ok()?;
    }
    Some(reader.at)
}

/// The curve an envelope block draws, with its heights as the singles FL
/// keeps.
fn read_curve(envelope: &MEnvelope) -> Curve {
    let points: Vec<Point> = envelope
        .points
        .iter()
        .map(|point| Point {
            y: f64::from(point.y as f32),
            ..*point
        })
        .collect();
    Curve::new(&points)
}

/// The stereo separation matrix for a wheel from -1 to 1: above 0 the
/// sides are mixed towards mono by `(5^s - 1) / 4`, under 0 each side has
/// half that much of the other taken off, which widens.
fn separation_gains(separation: f32) -> [f32; 4] {
    if separation == 0.0 {
        return [1.0, 1.0, 0.0, 0.0];
    }
    let x = if separation > 0.0 {
        ((f64::from(separation) * 5f64.ln()).exp() - 1.0) as f32 * 0.25
    } else {
        separation
    };
    let x = x.min(1.0);
    [1.0 - x * 0.5, 1.0 - x * 0.5, x * 0.5, x * 0.5]
}

/// A band's saturator for its parameters, as FL's `FUN_00841cc0`: the
/// threshold wheel's sign picks the shape, and the level the band is
/// expected to peak at is what the curve can put out times the post gain,
/// or the ceiling if that is more.
fn saturator(p: &[i32; PER_BAND], band: &Band) -> Saturator {
    let threshold = p[SAT_THRESHOLD];
    if threshold == 0 {
        return Saturator::Off;
    }
    let ceiling = gain(p[SAT_CEILING]);
    let reach = if !band.rms_mode && band.mode < 1 {
        band.table.peak
    } else {
        4.0
    };
    let expect = (reach * f64::from(band.post)).max(f64::from(ceiling));
    if threshold > 0 {
        let start = (1.0 - threshold as f32 * 0.001) * ceiling;
        let mut span = ceiling - start;
        if span <= 0.0 {
            span = 1.525_878_9e-5;
        }
        Saturator::Soft {
            start,
            span,
            inverse: 1.0 / span,
        }
    } else {
        let hardness = (f64::from(threshold) * -0.1) as f32 + 1.0;
        let k = f64::from(20.0 - (f64::from(hardness).ln() / 101f64.ln()) as f32 * 19.0);
        let norm = if ceiling > 0.0 {
            1.0 / (expect / f64::from(ceiling)).powf(k).atan().powf(1.0 / k)
        } else {
            1.0
        };
        Saturator::Shaped {
            ceiling,
            k,
            expect: expect as f32,
            norm,
        }
    }
}

impl Effect for Maximus {
    /// FL reads nothing of a version under 1 and keeps what it has. A
    /// state whose curves do not end where the bytes do is turned down,
    /// but for the versions 11 to 13 that wrote one i32 more.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let version = state_version(bytes, V15_LEN)?;
        if version < 1 {
            return Ok(());
        }
        let version = if version >= 0x10000 {
            version >> 16
        } else {
            version
        };
        let Some(parsed) = Self::parse(bytes, version) else {
            return Err(StateError::Length {
                expected: V15_LEN,
                found: bytes.len(),
            });
        };
        if parsed.end != bytes.len() {
            return Err(StateError::Length {
                expected: parsed.end,
                found: bytes.len(),
            });
        }
        for (param, value) in parsed.params.iter().enumerate() {
            let (min, max) = RANGES[param];
            if *value < min || *value > max {
                return Err(StateError::Range {
                    param,
                    value: *value,
                });
            }
        }
        self.params = parsed.params;
        self.switches = parsed.switches;
        self.curves = parsed.curves;
        // The oversampling sets how long the bands' memory has to be.
        self.make_room();
        self.rebuild_tables();
        self.reset();
        Ok(())
    }

    /// The times, the filters and the memory are worked out for the rate,
    /// and the plugin starts from nothing. A rate that is not a number
    /// above 0 is not taken.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz > 0.0 {
            self.hz = f64::from(hz);
            self.make_room();
            self.reset();
        }
    }

    /// A block of silence counts down, and after the latency and a second
    /// and a half of them the plugin sleeps: it forgets what it held and
    /// passes its input on until a block has sound again.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        if len == 0 {
            return;
        }
        let (left, right) = (&mut left[..len], &mut right[..len]);
        if self.switches.linear {
            self.send_magnitudes();
        }
        let quiet = left.iter().chain(right.iter()).all(|v| v.abs() <= QUIET);
        self.countdown = if quiet {
            self.countdown.saturating_sub(len as i64)
        } else {
            self.hold
        };
        if self.countdown <= -(len as i64) {
            if !self.asleep {
                self.flush();
                self.asleep = true;
            }
            return;
        }
        self.asleep = false;
        // Each band's separation ramps over the block, the three bands'
        // over its frames at the band rate.
        let os = self.os();
        let mut ramps = [([1.0, 1.0, 0.0, 0.0], [0.0; 4]); BANDS];
        for (b, (ramp, band)) in ramps.iter_mut().zip(&mut self.bands).enumerate() {
            if band.effective < 2 {
                *ramp = band.matrix.ramp(if b == MASTER { len } else { len * os });
            }
        }
        let mut gains = ramps.map(|(from, _)| from);
        let steps = ramps.map(|(_, step)| step);
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            [*l, *r] = self.frame([*l, *r], &mut gains, &steps);
        }
        for band in &mut self.bands {
            band.end_block();
        }
        self.oversampler.flush_subnormal();
        let sections = (self.low.iter_mut())
            .chain(&mut self.mid_high)
            .chain(&mut self.mid_low)
            .chain(&mut self.high)
            .chain(&mut self.low_cut);
        for section in sections {
            section.flush();
        }
    }

    fn param_count(&self) -> usize {
        PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |value| *value as f32)
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(range) = RANGES.get(index) {
            self.params[index] = knob_value(value, *range);
            self.update();
            // A slope remakes the oversamplers and flushes the plugin,
            // which wakes on the next block with sound (`FUN_008485d0`,
            // `FUN_00846280`, 0x788 = 0xc0000001).
            // The four crossover globals ask for new linear-phase
            // magnitudes (`0x842933`, `0x842960`); the low cut and the
            // band modes are read with the next of them.
            if index <= HIGH_SLOPE && self.switches.linear {
                self.linear_dirty = [true; 3];
            }
            if index == LOW_SLOPE || index == HIGH_SLOPE {
                self.flush();
                self.countdown = -0x3fff_ffff;
                self.asleep = true;
            }
        }
    }

    /// The crossover's FIR half-length and the LMH delay, both at the
    /// band rate and so divided by the oversampling, and the master's
    /// attack, in frames.
    fn latency(&self) -> usize {
        self.latency
    }

    /// Two time wheels at their end and the longest FIR.
    fn max_latency(&self) -> usize {
        let wheel = wheel_frames(10_000, self.hz).round_ties_even() as usize;
        let crossover = if self.switches.linear {
            self.linear_size() * 3 / 4
        } else {
            (self.fir_len() - 1) / 2
        };
        (wheel * self.os() + crossover) / self.os() + wheel
    }

    fn reset(&mut self) {
        // The contexts as FL makes them, which then take a magnitude each.
        for context in &mut self.contexts {
            context.restart();
        }
        // FL's `FUN_00848930`: every band sends a magnitude after a load
        // (and here after a new rate, which reaches the same reset).
        self.linear_dirty = [self.switches.linear; 3];
        self.update();
        self.flush();
        self.countdown = self.hold;
        self.asleep = false;
    }

    fn fidelity(&self) -> Fidelity {
        if self.switches.oversample > MAX_ORDER as i32 {
            Fidelity::Approximate("Maximus oversampling past 8x runs at 8x")
        } else if self.switches.linear && !(0..=7).contains(&self.switches.precision) {
            Fidelity::Approximate("Maximus linear phase past its highest precision runs at it")
        } else {
            Fidelity::Exact
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, presets, state};
    use dsp::halfband::HalfBand;

    const HZ: f64 = 48_000.0;

    /// A version 15 state with these parameters and the default curves,
    /// as FL writes it: the parameters, their spare copy, the display's
    /// 34 bytes, the version 14 and 15 extras and eight curves.
    fn v15(params: [i32; PARAMS], curves: [&[(f64, f32, f32)]; 4]) -> Vec<u8> {
        let mut bytes = state(&[0xf0003]);
        bytes.extend(state(&params));
        bytes.extend(state(&params));
        let mut display = [0u8; 34];
        display[..4].copy_from_slice(&50i32.to_le_bytes());
        display[4..8].copy_from_slice(&3i32.to_le_bytes());
        display[12] = 1;
        bytes.extend(display);
        bytes.extend(state(&[2, 0]));
        bytes.push(6);
        bytes.extend(state(&[3, 1, 2, 2, 2, 1]));
        for curve in curves {
            for _ in 0..2 {
                bytes.extend(state(&[3, 3, curve.len() as i32]));
                for (width, y, tension) in curve {
                    bytes.extend(width.to_le_bytes());
                    bytes.extend(f64::from(*y).to_le_bytes());
                    bytes.extend(tension.to_le_bytes());
                    bytes.extend([0, 0, 0, 0]);
                }
                bytes.extend(state(&[0, -1, -1, -1, -1]));
            }
        }
        bytes
    }

    fn defaults() -> [&'static [(f64, f32, f32)]; 4] {
        [&DEFAULT_CURVE; 4]
    }

    fn at_48k(bytes: &[u8]) -> Maximus {
        let mut plugin = Maximus::default();
        plugin.set_sample_rate(HZ as f32);
        plugin.set_state(bytes).unwrap();
        plugin
    }

    /// The defaults with some parameters put elsewhere.
    fn moved(moves: &[(usize, i32)]) -> [i32; PARAMS] {
        let mut params = DEFAULTS;
        for (index, value) in moves {
            params[*index] = *value;
        }
        params
    }

    fn band(b: usize, k: usize) -> usize {
        GLOBALS + PER_BAND * b + k
    }

    /// Both sides get `input`, in blocks of 256, and the left comes back.
    fn run(plugin: &mut Maximus, input: &[f32]) -> Vec<f32> {
        let (mut left, mut right) = (input.to_vec(), input.to_vec());
        for (l, r) in left.chunks_mut(256).zip(right.chunks_mut(256)) {
            plugin.process(l, r);
        }
        assert!(left == right);
        left
    }

    fn db(gain: f64) -> f64 {
        20.0 * gain.log10()
    }

    #[test]
    fn the_wheels_are_the_doc_s_curves() {
        // 200 Hz and 3 kHz at the defaults, 30 Hz to 16 kHz over the wheel.
        assert!((crossover_hz(19_800) - 200.0).abs() < 0.1);
        assert!((crossover_hz(48_065) - 3000.0).abs() < 1.0);
        assert!((crossover_hz(0) - 30.0).abs() < 1e-9);
        assert!((crossover_hz(65_536) - 16_000.0).abs() < 1e-6);
        // The low cut: 20 Hz at the default, 5 to 80 Hz.
        assert!((low_cut_hz(32_768) - 20.0).abs() < 1e-9);
        assert!((low_cut_hz(0) - 5.0).abs() < 1e-9 && (low_cut_hz(65_536) - 80.0).abs() < 1e-9);
        // The separation: mono at 1000, wider under 0, unity at 0.
        assert_eq!(separation_gains(0.0), [1.0, 1.0, 0.0, 0.0]);
        let mono = separation_gains(1.0);
        assert!(mono.iter().all(|g| (g - 0.5).abs() < 1e-6));
        assert_eq!(separation_gains(-1.0), [1.5, 1.5, -0.5, -0.5]);
        let [ll, _, rl, _] = separation_gains(0.5);
        assert!((f64::from(rl) - (5f64.sqrt() - 1.0) / 8.0).abs() < 1e-6 && ll + rl == 1.0);
    }

    #[test]
    fn the_default_curve_is_a_brickwall_at_the_ceiling() {
        let curve = plain_curve(&DEFAULT_CURVE);
        // Points at x = 0, 0.5 and 1: 1:1 to 0 dBFS, then flat.
        assert_eq!(curve.value_at(0.25), 0.25);
        assert_eq!(curve.value_at(0.5), 0.5);
        assert_eq!(curve.value_at(0.75), 0.5);
        assert_eq!(curve.value_at(2.0), 0.5);
        let mut table = Table::default();
        table.build(&curve, 0.975);
        // Under 0 dBFS the gain is the ceiling: out = 0.975 * level.
        for i in [1usize, 100, 4096, 8191] {
            assert!((f64::from(table.gains[i]) - 0.975).abs() < 1e-5, "{i}");
        }
        // At 1.0 the output reaches the ceiling and the table ends there.
        assert_eq!(table.last, 8191);
        assert_eq!(table.gains.len(), 8193);
        assert!((table.top - 0.975).abs() < 1e-12 && (table.peak - 1.0).abs() < 1e-12);
        // Above the table the gain is the ceiling over the level.
        assert!((f64::from(table.gain(2.0, 1.0, 1.0)) - 0.4875).abs() < 1e-6);
        assert!((f64::from(table.gain(0.5, 1.0, 1.0)) - 0.975).abs() < 1e-5);
        // Pre gain scales the level that is looked up, post the output.
        assert!((f64::from(table.gain(0.5, 4.0, 1.0)) - 0.975 / 0.5).abs() < 1e-5);
        assert!((f64::from(table.gain(0.25, 1.0, 2.0)) - 1.95).abs() < 1e-5);
        // With the whole ceiling the brickwall is at 1.
        table.build(&curve, 1.0);
        assert_eq!(table.gains[4096], 1.0);
        assert_eq!(table.gain(4.0, 1.0, 1.0), 0.25);
    }

    #[test]
    fn a_tension_bends_a_stretch_between_the_same_ends() {
        // The curves are `dsp::curve`'s: FL's tension is
        // `k = 1000001^|t| - 1`, bent as `1 - ((k + 1)^(1 - v) - 1) / k`
        // above 0 and `((k + 1)^v - 1) / k` under it.
        for tension in [0.07f32, 0.19, -0.3, 0.9] {
            let bent = plain_curve(&[(0.0, 0.0, 0.0), (1.0, 1.0, tension)]);
            assert!(bent.value_at(0.0).abs() < 1e-6 && (bent.value_at(0.99999) - 1.0).abs() < 1e-3);
            let middle = bent.value_at(0.5);
            assert_eq!(middle > 0.5, tension > 0.0, "{tension}");
            let k = 1_000_001f64.powf(f64::from(tension.abs())) - 1.0;
            let want = if tension > 0.0 {
                1.0 - ((0.5 * (k + 1.0).ln()).exp() - 1.0) / k
            } else {
                ((0.5 * (k + 1.0).ln()).exp() - 1.0) / k
            };
            assert!((middle - want).abs() < 1e-5, "{tension}: {middle} {want}");
        }
        // A stretch of no width is skipped, and a curve ends flat.
        let gate = plain_curve(&[
            (0.0, 0.0, 0.0),
            (0.1, 0.0, 0.0),
            (0.1, 0.2, 0.0),
            (0.3, 0.5, 0.0),
            (0.5, 0.5, 0.0),
        ]);
        assert_eq!(gate.value_at(0.05), 0.0);
        assert!((gate.value_at(0.15) - 0.1).abs() < 1e-6);
        assert!((gate.value_at(0.35) - 0.35).abs() < 1e-6);
        let steps = plain_curve(&[
            (0.0, 0.0, 0.0),
            (0.5, 0.2, 0.0),
            (0.0, 0.8, 0.0),
            (0.5, 1.0, 0.0),
        ]);
        assert!(
            (steps.value_at(0.5) - 0.8).abs() < 1e-6
                && (steps.value_at(0.49999) - 0.2).abs() < 1e-4
        );
        // A height is kept as a single.
        let bytes: Vec<u8> = state(&[3, 3, 1])
            .into_iter()
            .chain(0f64.to_le_bytes())
            .chain(0.1f64.to_le_bytes())
            .chain([0; 28])
            .collect();
        let mut reader = Reader::new(&bytes);
        let envelope = MEnvelope::read(&mut reader, Class::Map).unwrap();
        assert_eq!(read_curve(&envelope).value_at(0.0), f64::from(0.1f32));
    }

    #[test]
    fn the_iir_crossover_sums_flat_and_the_fir_one_to_a_delta() {
        let mut plugin = Maximus::default();
        plugin.set_sample_rate(HZ as f32);
        // Every band passes with the gain at 1 and no delay: the sum is
        // the crossover alone.
        for b in 0..BANDS {
            plugin.set_param(band(b, MODE), 1.0);
        }
        plugin.set_param(LOW_CUT, -1.0);
        assert_eq!(plugin.latency(), 0);
        for turns in [0.001, 0.0042, 0.01, 0.0625, 0.1, 0.25, 0.4] {
            let input = testing::sine(turns, 48_000);
            let out = run(&mut plugin, &input);
            let level = testing::partial(&out[24_000..], turns);
            assert!(db(level).abs() < 0.1, "{turns}: {} dB", db(level));
        }
        // One section a band sums flat as well, with the outer bands
        // inverted.
        plugin.set_param(LOW_SLOPE, 0.0);
        plugin.set_param(HIGH_SLOPE, 0.0);
        assert_eq!(
            (
                plugin.bands[0].sign,
                plugin.bands[1].sign,
                plugin.bands[2].sign
            ),
            (-1.0, 1.0, -1.0)
        );
        for turns in [0.001, 0.0042, 0.01, 0.0625, 0.25] {
            let input = testing::sine(turns, 48_000);
            let out = run(&mut plugin, &input);
            let level = testing::partial(&out[24_000..], turns);
            assert!(
                db(level).abs() < 0.1,
                "slope 0 at {turns}: {} dB",
                db(level)
            );
        }
        // The FIR crossover sums to a delta, half its length late.
        let mut params = DEFAULTS;
        for b in 0..BANDS {
            params[band(b, MODE)] = 1;
        }
        params[LOW_CUT] = -1;
        let mut bytes = v15(params, defaults());
        bytes[PARAMS_AT + FIR_AT] = 1;
        let mut fir = at_48k(&bytes);
        assert!(fir.switches.fir && fir.fidelity() == Fidelity::Exact);
        assert_eq!((fir.fir_len(), fir.latency()), (193, 96));
        let mut input = vec![0.0; 400];
        input[10] = 1.0;
        let out = run(&mut fir, &input);
        for (n, v) in out.iter().enumerate() {
            let want = if n == 106 { 1.0 } else { 0.0 };
            assert!((v - want).abs() < 1e-6, "{n}: {v}");
        }
        // Its taps: the LOW band's low pass and the HIGH band's high pass
        // are each other's complement with the MID band.
        let sum: Vec<f32> = (0..193)
            .map(|k| fir.fir.taps.iter().map(|taps| taps[k]).sum())
            .collect();
        assert!(
            sum.iter()
                .enumerate()
                .all(|(k, v)| (v - if k == 96 { 1.0 } else { 0.0 }).abs() < 1e-6)
        );
    }

    #[test]
    fn a_band_clamps_a_sine_to_its_ceiling() {
        // A 1 kHz tone 6 dB over full scale into the MID band alone, with
        // LOW and HIGH off and the master passing: the band's brickwall
        // curve holds it at the ceiling once the attack has settled.
        let alone = [
            (band(0, MODE), 3),
            (band(2, MODE), 3),
            (band(MASTER, MODE), 1),
            (LOW_CUT, -1),
        ];
        let mut plugin = at_48k(&v15(moved(&alone), defaults()));
        let loud: Vec<f32> = testing::sine(1000.0 / HZ, 48_000)
            .iter()
            .map(|v| v * 2.0)
            .collect();
        let out = run(&mut plugin, &loud);
        let settled = &out[24_000..];
        let peak = settled.iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!(peak <= 0.975 + 1e-3, "{peak}");
        assert!(db(f64::from(peak) / 0.975).abs() < 0.1, "{peak}");
        let level = testing::partial(settled, 1000.0 / HZ);
        assert!(db(level / 0.975).abs() < 0.1, "{}", db(level));
        // With the master compressing as well, its curve under the
        // ceiling is the ceiling times the level: 0.975 squared.
        let mut plugin = at_48k(&v15(moved(&alone[..1]).map(|v| v), defaults()));
        plugin.set_param(band(2, MODE), 3.0);
        plugin.set_param(LOW_CUT, -1.0);
        let out = run(&mut plugin, &loud);
        let level = testing::partial(&out[24_000..], 1000.0 / HZ);
        assert!(db(level / (0.975 * 0.975)).abs() < 0.1, "{}", db(level));
        // The whole ceiling flag puts the brickwall at 1.
        let mut bytes = v15(moved(&alone), defaults());
        bytes[PARAMS_AT + 0x1fa + 8] = 0x40;
        let mut plugin = at_48k(&bytes);
        let out = run(&mut plugin, &loud);
        let level = testing::partial(&out[24_000..], 1000.0 / HZ);
        assert!(db(level).abs() < 0.1, "{}", db(level));
    }

    #[test]
    fn unity_settings_pass_sound_unchanged() {
        // Every band passing, no low cut, no delay: the IIR crossover and
        // the sum are the only thing in the way, and they are flat.
        let mut params = DEFAULTS;
        for b in 0..BANDS {
            params[band(b, MODE)] = 1;
        }
        params[LOW_CUT] = -1;
        let mut plugin = at_48k(&v15(params, defaults()));
        assert_eq!(plugin.latency(), 0);
        let input: Vec<f32> = (0..4800)
            .map(|n| ((n * 7919 % 1009) as f32 / 1009.0 - 0.5) * 0.8)
            .collect();
        let out = run(&mut plugin, &input);
        // The crossover's allpasses turn the phase, so compare levels: a
        // sine comes out at its level within 0.1 dB. FL's LOW band
        // allpass has a Q of 0.5 against the 0.707 of the MID and HIGH
        // sum, so the IIR sum is not flat to 1e-4, and FL's is not either.
        for turns in [0.01, 0.05, 0.2] {
            let input = testing::sine(turns, 48_000);
            let out = run(&mut plugin, &input);
            let level = testing::partial(&out[24_000..], turns);
            assert!(db(level).abs() < 0.1, "{turns}: {level}");
        }
        assert!(out.iter().all(|v| v.is_finite() && v.abs() < 1.0));
        // With the FIR crossover the sound is itself to 1e-4, 96 frames
        // late.
        let mut bytes = v15(params, defaults());
        bytes[PARAMS_AT + FIR_AT] = 1;
        let mut fir = at_48k(&bytes);
        let out = run(&mut fir, &input);
        for (n, v) in out.iter().enumerate().skip(96) {
            assert!((v - input[n - 96]).abs() < 1e-4, "{n}");
        }
        // A compressing band under its threshold changes nothing but the
        // ceiling's 0.975 and the delays: the LMH delay and the master's
        // attack.
        let mut plugin = at_48k(&v15(moved(&[(LOW_CUT, -1)]), defaults()));
        assert_eq!(plugin.latency(), 192);
        let quiet = testing::sine(0.02, 48_000);
        let out = run(&mut plugin, &quiet);
        let level = testing::partial(&out[24_000..], 0.02);
        assert!((level - 0.975 * 0.975).abs() < 2e-4, "{level}");
    }

    #[test]
    fn the_linear_phase_curves_are_the_doc_s() {
        // The quick log2 and 2^x are within a thousandth of the real ones.
        for x in [0.01f32, 0.3, 1.0, 1.5, 2.0, 7.3, 1000.0] {
            assert!((quick_log2(x) - x.log2()).abs() < 2e-3, "{x}");
        }
        for x in [-20.0f32, -3.5, -0.4, 0.0, 0.1667, 2.5] {
            let want = x.exp2();
            assert!((quick_exp2(x) / want - 1.0).abs() < 2e-4, "{x}");
        }
        // -6 dB at the corner, 12 or 24 dB an octave away on the stop
        // side, and the low and the high pass add up to 1.
        let at = |f: f32, slope: f32| crossover_curve(1000.0, slope, f, false);
        assert!((at(1000.0, 24.0) - 0.5).abs() < 3e-3);
        // FL's dB here are sixths of an octave of level: -18 is 2^-3.
        assert!((at(2000.0, 12.0) / 0.125 - 1.0).abs() < 0.01);
        assert!((at(4000.0, 24.0) * 512.0 - 1.0).abs() < 0.01);
        assert_eq!(at(1e5, 24.0), 0.0);
        for f in [20.0f32, 400.0, 999.0, 1000.0, 1500.0, 9000.0] {
            let sum = at(f, 24.0) + crossover_curve(1000.0, 24.0, f, true);
            assert!((sum - 1.0).abs() < 1e-6, "{f}: {sum}");
        }
        // The FFT size: 2048 at 48 kHz for the default precision, twice
        // that for each octave of rate, half under 44.1 kHz.
        assert_eq!(fft_size(2, 48_000), 2048);
        assert_eq!(fft_size(2, 96_000), 4096);
        assert_eq!(fft_size(2, 32_000), 1024);
        assert_eq!(fft_size(7, 192_000), 65_536);
        // The three bands' magnitudes add up to 1 but for where the LOW
        // band's pass meets the HIGH band's: at the default 200 Hz and
        // 3 kHz, flat to a thousandth. The low cut takes the bottom away.
        let mut bytes = v15(moved(&[(LOW_CUT, -1)]), defaults());
        bytes[PARAMS_AT + 0x1fa + 8] = 1;
        let plugin = at_48k(&bytes);
        let setting = plugin.linear_setting();
        assert_eq!(
            (setting.n, setting.rate, setting.low_cut),
            (2048, 48_000, None)
        );
        let mut bands = vec![vec![0.0f32; 1025]; 3];
        for (b, band) in bands.iter_mut().enumerate() {
            setting.magnitudes(b, band);
        }
        for k in 0..1025 {
            let sum: f32 = bands.iter().map(|band| band[k]).sum();
            assert!((sum - 1.0).abs() < 1e-3, "{k}: {sum}");
        }
        let mut cut = setting;
        cut.low_cut = Some(20.0);
        let mut low = vec![0.0f32; 1025];
        cut.magnitudes(0, &mut low);
        assert!(low[0] < 1e-3 && (low[10] - bands[0][10]).abs() < 1e-3);
    }

    #[test]
    fn the_linear_phase_crossover_passes_sound_three_quarters_of_its_fft_late() {
        // Every band passing, no low cut: after the first crossfade
        // (0.1 s) the bands add up to the input 1536 frames late.
        let mut params = DEFAULTS;
        for b in 0..BANDS {
            params[band(b, MODE)] = 1;
        }
        params[LOW_CUT] = -1;
        let mut bytes = v15(params, defaults());
        bytes[PARAMS_AT + 0x1fa + 8] = 1;
        let mut plugin = at_48k(&bytes);
        assert_eq!(
            (plugin.latency(), plugin.fidelity()),
            (1536, Fidelity::Exact)
        );
        let input: Vec<f32> = (0..24_000)
            .map(|n| ((n * 7919 % 1009) as f32 / 1009.0 - 0.5) * 0.5)
            .collect();
        let out = run(&mut plugin, &input);
        let error = (12_000..24_000)
            .map(|n| (out[n] - input[n - 1536]).abs())
            .fold(0.0f32, f32::max);
        assert!(error < 5e-3, "{error}");
        // Nothing comes out for the first block of 1024 frames, the
        // filters' tails before their centre are small, and the sound
        // fades in from the zero filter over those 0.1 s.
        assert!(out[..1024].iter().all(|v| *v == 0.0));
        assert!(out[1024..1536].iter().all(|v| v.abs() < 0.01));
        assert!((1600..4800).any(|n| out[n].abs() > 0.05));
        // A moved crossover sends new magnitudes, which fade in over
        // the window of the speed menu.
        plugin.set_param(LOW_CUT, 30_000.0);
        assert_eq!(plugin.linear_dirty, [false; 3]);
        plugin.set_param(LOW_FREQ, 30_000.0);
        assert_eq!(plugin.linear_dirty, [true; 3]);
        let out = run(&mut plugin, &input[..512]);
        assert!(out.iter().all(|v| v.is_finite()));
        assert_eq!(plugin.linear_dirty, [false; 3]);
        assert!(!plugin.contexts[0].settled());
        // With oversampling the FFT is twice as long at twice the rate:
        // as late in seconds.
        bytes[PARAMS_AT + OVERSAMPLE_AT] = 1;
        let plugin = at_48k(&bytes);
        assert_eq!((plugin.linear_size(), plugin.latency()), (4096, 1536));
    }

    #[test]
    fn the_oversampler_is_parametric_eq_2_s_half_band_a_frame_at_a_time() {
        // A frame at a time through order 1 is the block form FL runs,
        // to the bit, both ways.
        let input: Vec<f32> = (0..1000)
            .map(|n| ((n * 7919 % 1009) as f32 / 1009.0 - 0.5) * 0.8)
            .collect();
        let mut block_wide = vec![0.0f32; 2000];
        HalfBand::default().up(&input, &mut block_wide);
        let mut block_back = vec![0.0f32; 1000];
        HalfBand::default().down(&block_wide, &mut block_back);
        let mut over = Oversampler::default();
        for (n, x) in input.iter().enumerate() {
            let mut wide = [[0.0; 2]; WIDEST];
            over.up(1, [*x, -*x], &mut wide);
            assert_eq!(
                wide[..2],
                [
                    [block_wide[2 * n], -block_wide[2 * n]],
                    [block_wide[2 * n + 1], -block_wide[2 * n + 1]]
                ]
            );
            let mut pair = wide;
            pair[0] = [block_wide[2 * n]; 2];
            pair[1] = [block_wide[2 * n + 1]; 2];
            assert_eq!(over.down(1, &mut pair), [block_back[n]; 2]);
        }
        // Up and down through four or eight times passes a tone in the
        // band at its level and stops one in the top octave's image.
        for order in [2, 3] {
            let mut over = Oversampler::default();
            let tone = testing::sine(0.02, 24_000);
            let out: Vec<f32> = tone
                .iter()
                .map(|x| {
                    let mut wide = [[0.0; 2]; WIDEST];
                    over.up(order, [*x; 2], &mut wide);
                    over.down(order, &mut wide)[0]
                })
                .collect();
            let level = testing::partial(&out[12_000..], 0.02);
            assert!(db(level).abs() < 0.01, "{order}: {} dB", db(level));
        }
        // A tone just under the project's Nyquist comes up as itself and
        // its image at the band rate, the image 40 dB and more down.
        let mut over = Oversampler::default();
        let tone = testing::sine(0.3, 24_000);
        let mut wide_left = Vec::with_capacity(48_000);
        for x in &tone {
            let mut wide = [[0.0; 2]; WIDEST];
            over.up(1, [*x; 2], &mut wide);
            wide_left.extend([wide[0][0], wide[1][0]]);
        }
        let tone_level = testing::partial(&wide_left[24_000..], 0.15);
        let image = testing::partial(&wide_left[24_000..], 0.35);
        assert!(db(tone_level).abs() < 0.01, "{}", db(tone_level));
        assert!(db(image) < -40.0, "{}", db(image));
    }

    #[test]
    fn oversampling_runs_the_bands_at_the_band_rate() {
        let mut params = moved(&[(band(1, RELEASE), 4000), (band(1, WINDOW), 3000)]);
        params[band(3, RELEASE)] = 4000;
        let once = at_48k(&v15(params, defaults()));
        let mut bytes = v15(params, defaults());
        bytes[PARAMS_AT + OVERSAMPLE_AT] = 1;
        let twice = at_48k(&bytes);
        assert_eq!((twice.os(), twice.band_hz()), (2, 96_000.0));
        // The times are rounded at the project rate and doubled for the
        // three bands, not for the master.
        for b in 0..MASTER {
            let (a, b2) = (&once.bands[b], &twice.bands[b]);
            assert_eq!(b2.delay.frames.len(), 2 * a.delay.frames.len());
            assert_eq!(b2.window, 2 * a.window);
        }
        let mid = &twice.bands[1];
        assert_eq!(
            mid.release.fall,
            coef(2.0 * wheel_frames(4000, HZ) * 2.0 / 3f64.sqrt())
        );
        assert_eq!(twice.bands[MASTER], once.bands[MASTER]);
        // The crossover is designed at the band rate.
        assert_eq!(
            twice.low[0].coefficients,
            section(Kind::Low, crossover_hz(19_800), 1, 96_000.0).coefficients
        );
        // FL divides the bands' delay by the oversampling: the same
        // latency, 96 + 96 frames.
        assert_eq!((once.latency(), twice.latency()), (192, 192));
        // The FIR crossover is 4 ms at the band rate, its taps for a
        // frequency twice as many: 385 taps, 192 of them late at 96 kHz,
        // 96 at 48 kHz.
        let mut fir = bytes.clone();
        fir[PARAMS_AT + FIR_AT] = 1;
        let fir = at_48k(&fir);
        assert_eq!((fir.fir_len(), fir.latency()), (385, 96 + 96 + 96));
        assert!(fir.latency() <= fir.max_latency());

        // Every band passing: the crossover and the resampler in the way,
        // and both pass a tone at its level.
        let mut params = DEFAULTS;
        for b in 0..BANDS {
            params[band(b, MODE)] = 1;
        }
        params[LOW_CUT] = -1;
        let mut bytes = v15(params, defaults());
        bytes[PARAMS_AT + OVERSAMPLE_AT] = 1;
        let mut plugin = at_48k(&bytes);
        assert_eq!(plugin.latency(), 0);
        for turns in [0.01, 0.05, 0.2] {
            let out = run(&mut plugin, &testing::sine(turns, 48_000));
            let level = testing::partial(&out[24_000..], turns);
            assert!(db(level).abs() < 0.1, "{turns}: {} dB", db(level));
        }
        // The MID band alone clamps a 1 kHz sine 6 dB over to its
        // ceiling at twice the rate as well, with the master passing.
        let alone = [
            (band(0, MODE), 3),
            (band(2, MODE), 3),
            (band(MASTER, MODE), 1),
            (LOW_CUT, -1),
        ];
        let mut bytes = v15(moved(&alone), defaults());
        bytes[PARAMS_AT + OVERSAMPLE_AT] = 1;
        let mut plugin = at_48k(&bytes);
        let loud: Vec<f32> = testing::sine(1000.0 / HZ, 48_000)
            .iter()
            .map(|v| v * 2.0)
            .collect();
        let out = run(&mut plugin, &loud);
        assert!(out.iter().all(|v| v.is_finite()));
        let level = testing::partial(&out[24_000..], 1000.0 / HZ);
        assert!(db(level / 0.975).abs() < 0.1, "{}", db(level));
    }

    #[test]
    fn the_times_and_the_stages_are_the_limiter_s() {
        let plugin = at_48k(&v15(
            moved(&[
                (band(0, ATTACK_STAGES), 4),
                (band(0, RELEASE_2), 5000),
                (band(0, RELEASE_STAGES), 2),
            ]),
            defaults(),
        ));
        let low = &plugin.bands[0];
        // The LMH delay is 2 ms, the band's attack 2 ms over 0.8 / sqrt(4).
        assert_eq!(low.delay.frames.len(), 96);
        assert_eq!(low.attack.count, 4);
        assert_eq!(
            low.attack.fall,
            coef(wheel_frames(1115, HZ) * f64::from(0.8f32) / 2.0)
        );
        assert_eq!(low.attack.rise, coef(wheel_frames(5000, HZ) * 2.0 / 2.0));
        assert_eq!(low.release.count, 2);
        assert_eq!(low.release.rise, 0.0);
        assert_eq!(
            low.release.fall,
            coef(wheel_frames(5000, HZ) * 2.0 / 2f64.sqrt())
        );
        assert_eq!(
            low.window,
            wheel_frames(2882, HZ).round_ties_even() as usize
        );
        // Release 2 at 0 falls back on the attack, and no release means
        // no stages.
        let mid = &plugin.bands[1];
        assert_eq!(mid.attack.rise, mid.attack.fall);
        let none = at_48k(&v15(moved(&[(band(1, RELEASE), 0)]), defaults()));
        assert_eq!(none.bands[1].release.count, 0);
        // The master's delay is its own attack, and with no band
        // compressing the LMH delay goes.
        assert_eq!(plugin.bands[MASTER].delay.frames.len(), 96);
        assert_eq!(plugin.latency(), 192);
        let passing = at_48k(&v15(
            moved(&[(band(0, MODE), 1), (band(1, MODE), 1), (band(2, MODE), 1)]),
            defaults(),
        ));
        assert_eq!(
            (passing.bands[0].delay.frames.len(), passing.latency()),
            (0, 96)
        );
        assert_eq!(plugin.hold, 192 + 72_000);
        assert!(plugin.max_latency() >= plugin.latency());
    }

    #[test]
    fn the_saturators_are_the_doc_s() {
        // The soft one is Fruity Limiter's knee from the threshold.
        let plugin = at_48k(&v15(moved(&[(band(0, SAT_THRESHOLD), 500)]), defaults()));
        let Saturator::Soft {
            start,
            span,
            inverse,
        } = plugin.bands[0].saturator
        else {
            panic!("not soft");
        };
        assert!(
            (start - 0.5).abs() < 1e-6 && (span - 0.5).abs() < 1e-6 && (inverse - 2.0).abs() < 1e-5
        );
        let sat = plugin.bands[0].saturator;
        assert_eq!(sat.apply(0.3), 0.3);
        assert!((sat.apply(0.75) - (0.5 + 0.5 * (1.0 - (-0.5f32).exp()))).abs() < 1e-6);
        assert!(sat.apply(10.0) <= 1.0 && sat.apply(-10.0) >= -1.0);
        // The shaped one at -1000 is k = 1: an arctangent scaled so that
        // the expected peak, here the ceiling, comes out at the ceiling.
        let plugin = at_48k(&v15(moved(&[(band(0, SAT_THRESHOLD), -1000)]), defaults()));
        let Saturator::Shaped {
            ceiling,
            k,
            expect,
            norm,
        } = plugin.bands[0].saturator
        else {
            panic!("not shaped");
        };
        assert!((ceiling - 1.0).abs() < 1e-6 && (k - 1.0).abs() < 1e-6);
        assert!((expect - 1.0).abs() < 1e-6 && (norm - 1.0 / 1f64.atan()).abs() < 1e-9);
        let sat = plugin.bands[0].saturator;
        assert!((f64::from(sat.apply(1.0)) - 1.0).abs() < 1e-6);
        assert!((f64::from(sat.apply(0.5)) - 0.5f64.atan() / 1f64.atan()).abs() < 1e-6);
        assert_eq!(sat.apply(3.0), sat.apply(1.0));
        // At -1 the knee is nearly hard.
        let plugin = at_48k(&v15(moved(&[(band(0, SAT_THRESHOLD), -1)]), defaults()));
        let Saturator::Shaped { k, .. } = plugin.bands[0].saturator else {
            panic!("not shaped");
        };
        assert!((k - (20.0 - 1.1f64.ln() / 101f64.ln() * 19.0)).abs() < 1e-4);
        assert!(plugin.bands[0].saturator.apply(0.9) > 0.89);
        // Post gain raises what the band is expected to peak at.
        let plugin = at_48k(&v15(
            moved(&[(band(0, SAT_THRESHOLD), -1000), (band(0, POST), 2000)]),
            defaults(),
        ));
        let Saturator::Shaped { expect, .. } = plugin.bands[0].saturator else {
            panic!("not shaped");
        };
        assert!((expect - 8.0).abs() < 1e-5);
    }

    #[test]
    fn the_modes_and_the_solo_switch_route_the_bands() {
        // Mode 2 silences a band, mode 3 takes it out of the crossover:
        // with LOW and HIGH off the MID band is the whole input.
        let mut params = moved(&[
            (band(0, MODE), 3),
            (band(2, MODE), 3),
            (band(1, MODE), 1),
            (band(MASTER, MODE), 1),
            (LOW_CUT, -1),
        ]);
        let mut plugin = at_48k(&v15(params, defaults()));
        assert_eq!(plugin.remainder, 1);
        assert!(
            plugin.low.is_empty()
                && plugin.mid_high.is_empty()
                && plugin.mid_low.is_empty()
                && plugin.high.is_empty()
        );
        let input = testing::sine(0.01, 1000);
        assert!(run(&mut plugin, &input) == input);
        // Mute a band and its share is gone.
        params = moved(&[
            (band(1, MODE), 2),
            (band(0, MODE), 1),
            (band(2, MODE), 1),
            (band(MASTER, MODE), 1),
            (LOW_CUT, -1),
        ]);
        let mut plugin = at_48k(&v15(params, defaults()));
        let out = run(&mut plugin, &testing::sine(1000.0 / HZ, 48_000));
        // What is left is the LOW and HIGH bands' skirts at 1 kHz.
        assert!(testing::partial(&out[24_000..], 1000.0 / HZ) < 0.03);
        // A solo on the LOW band silences MID and HIGH and passes the
        // master.
        let mut bytes = v15(moved(&[(LOW_CUT, -1)]), defaults());
        bytes[PARAMS_AT + SHOWN_AT] = 0;
        bytes[PARAMS_AT + SOLO_AT] = 1;
        let plugin = at_48k(&bytes);
        assert_eq!(
            plugin.bands.iter().map(|b| b.effective).collect::<Vec<_>>(),
            [0, 2, 2, 1]
        );
        // Shown master: no solo.
        bytes[PARAMS_AT + SHOWN_AT] = 3;
        let plugin = at_48k(&bytes);
        assert_eq!(
            plugin.bands.iter().map(|b| b.effective).collect::<Vec<_>>(),
            [0, 0, 0, 0]
        );
    }

    #[test]
    fn the_lmh_mix_blends_the_bands_against_the_dry_sound() {
        // A mix of 0 with every band muted: the delayed dry sound comes
        // out with the sign of each band, which is +1 for the default
        // slopes. The master passes.
        let params = moved(&[(LMH_MIX, 0), (band(MASTER, MODE), 1), (LOW_CUT, -1)]);
        let mut plugin = at_48k(&v15(params, defaults()));
        let input = testing::sine(0.01, 48_000);
        let out = run(&mut plugin, &input);
        let level = testing::partial(&out[24_000..], 0.01);
        assert!((level - 1.0).abs() < 1e-3, "{level}");
        // The dry path is the band's own, so the crossover is in it.
        assert_eq!(plugin.latency(), 96);
    }

    #[test]
    fn the_low_cut_is_four_high_passes_at_0_8_of_the_wheel() {
        let plugin = at_48k(&v15(DEFAULTS, defaults()));
        assert!(plugin.low_cut_on && plugin.low_cut.len() == 4);
        let w = 16.0 * TAU / HZ;
        let (sin, cos) = w.sin_cos();
        assert_eq!(
            plugin.low_cut[2].coefficients,
            Coefficients64::high_pass(cos, sin * LOW_CUT_ALPHAS[2])
        );
        // Off at -1.
        let off = at_48k(&v15(moved(&[(LOW_CUT, -1)]), defaults()));
        assert!(!off.low_cut_on);
        // 20 Hz is well down at 10 Hz and flat at 200 Hz, with every band
        // passing.
        let mut params = moved(&[(LOW_CUT, 32_768)]);
        for b in 0..BANDS {
            params[band(b, MODE)] = 1;
        }
        let mut plugin = at_48k(&v15(params, defaults()));
        let out = run(&mut plugin, &testing::sine(10.0 / HZ, 96_000));
        assert!(db(testing::partial(&out[48_000..], 10.0 / HZ)) < -20.0);
        let out = run(&mut plugin, &testing::sine(200.0 / HZ, 96_000));
        assert!(db(testing::partial(&out[48_000..], 200.0 / HZ)).abs() < 0.2);
    }

    #[test]
    fn a_moved_slope_flushes_the_plugin_and_a_frequency_does_not() {
        let mut plugin = at_48k(&v15(DEFAULTS, defaults()));
        let tone = testing::sine(0.01, 2048);
        run(&mut plugin, &tone);
        plugin.set_param(LOW_FREQ, 30_000.0);
        assert!(!plugin.asleep);
        assert!(plugin.bands[1].delay.frames.iter().any(|f| f[0] != 0.0));
        plugin.set_param(HIGH_SLOPE, 0.0);
        assert!(plugin.asleep && plugin.countdown == -0x3fff_ffff);
        assert!(plugin.bands[1].delay.frames.iter().all(|f| *f == [0.0; 2]));
        assert!(
            plugin
                .low
                .iter()
                .all(|s| s.state == [Biquad64::default(); 2])
        );
        // Silence passes through asleep; sound wakes it.
        let mut quiet = [0.0f32; 64];
        plugin.process(&mut quiet.clone(), &mut quiet);
        assert!(plugin.asleep);
        run(&mut plugin, &tone[..256]);
        assert!(!plugin.asleep);
    }

    #[test]
    fn silence_puts_the_plugin_to_sleep_and_sound_wakes_it() {
        let mut plugin = at_48k(&v15(DEFAULTS, defaults()));
        run(&mut plugin, &testing::sine(0.01, 4800));
        assert!(!plugin.asleep);
        let hold = plugin.hold as usize;
        run(&mut plugin, &vec![0.0; hold]);
        assert!(!plugin.asleep);
        run(&mut plugin, &vec![0.0; 512]);
        assert!(plugin.asleep);
        // Asleep, the input passes as it is.
        let mut tiny = [1e-7, -1e-7];
        plugin.process(&mut tiny.clone(), &mut tiny);
        assert_eq!(tiny, [1e-7, -1e-7]);
        run(&mut plugin, &testing::sine(0.01, 256));
        assert!(!plugin.asleep && plugin.countdown == plugin.hold);
        let before = plugin.clone();
        plugin.process(&mut [], &mut []);
        assert_eq!(plugin, before);
    }

    #[test]
    fn states_of_every_version_load_and_wrong_ones_are_turned_down() {
        let bytes = v15(DEFAULTS, defaults());
        assert_eq!(bytes.len(), V15_LEN);
        let plugin = at_48k(&bytes);
        assert_eq!(plugin.params, DEFAULTS);
        assert_eq!(plugin.switches, Switches::default());
        assert_eq!(plugin.fidelity(), Fidelity::Exact);
        let mut default = Maximus::default();
        default.set_state(&bytes).unwrap();
        assert_eq!(default, Maximus::default());
        // The version word's high half is the version.
        let mut words = bytes.clone();
        words[..4].copy_from_slice(&15i32.to_le_bytes());
        assert_eq!(at_48k(&words), plugin);
        // A version 7 state: no extras.
        let mut v7 = bytes.clone();
        v7[..4].copy_from_slice(&7i32.to_le_bytes());
        v7.drain(PARAMS_AT + 0x1fa..PARAMS_AT + 0x1fa + 33);
        assert_eq!(v7.len(), 1342);
        let seven = at_48k(&v7);
        assert_eq!((seven.params, seven.switches.fir), (DEFAULTS, false));
        // Version 11 to 13 may have one i32 more before the curves.
        let mut v11 = v7.clone();
        v11[..4].copy_from_slice(&11i32.to_le_bytes());
        let eleven = at_48k(&v11);
        v11.splice(PARAMS_AT + 0x1fa..PARAMS_AT + 0x1fa, 1i32.to_le_bytes());
        assert_eq!(v11.len(), 1346);
        assert_eq!(at_48k(&v11), eleven);
        // The flags: linear phase and the whole ceiling.
        let mut flagged = bytes.clone();
        flagged[PARAMS_AT + 0x1fa + 8] = 0x41;
        let plugin = at_48k(&flagged);
        assert!(plugin.switches.linear && plugin.switches.whole_ceiling);
        assert_eq!(plugin.switches.precision, 2);
        assert_eq!(plugin.fidelity(), Fidelity::Exact);
        flagged[PARAMS_AT + 0x1fa..PARAMS_AT + 0x1fe].copy_from_slice(&9i32.to_le_bytes());
        assert_eq!(
            at_48k(&flagged).fidelity(),
            Fidelity::Approximate("Maximus linear phase past its highest precision runs at it")
        );
        let mut over = bytes.clone();
        over[PARAMS_AT + OVERSAMPLE_AT] = 1;
        let plugin = at_48k(&over);
        assert_eq!((plugin.order(), plugin.fidelity()), (1, Fidelity::Exact));
        over[PARAMS_AT + OVERSAMPLE_AT] = 4;
        assert_eq!(
            at_48k(&over).fidelity(),
            Fidelity::Approximate("Maximus oversampling past 8x runs at 8x")
        );
        // Wrong lengths and ranges.
        let mut plugin = at_48k(&bytes);
        let before = plugin.clone();
        let mut short = bytes.clone();
        short.pop();
        assert!(matches!(
            plugin.set_state(&short),
            Err(StateError::Length { .. })
        ));
        let mut long = bytes.clone();
        long.push(0);
        assert_eq!(
            plugin.set_state(&long),
            Err(StateError::Length {
                expected: 1375,
                found: 1376
            })
        );
        let bad = v15(moved(&[(band(2, ATTACK_STAGES), 9)]), defaults());
        assert_eq!(
            plugin.set_state(&bad),
            Err(StateError::Range {
                param: band(2, ATTACK_STAGES),
                value: 9
            })
        );
        assert_eq!(
            plugin.set_state(&[1, 0]),
            Err(StateError::Length {
                expected: 1375,
                found: 2
            })
        );
        assert_eq!(plugin, before);
        // A version under 1 leaves the plugin as it is.
        assert_eq!(plugin.set_state(&[0; 8]), Ok(()));
        assert_eq!(plugin, before);
    }

    #[test]
    fn a_moved_wheel_asks_for_no_memory() {
        let mut plugin = Maximus::default();
        plugin.set_sample_rate(48_000.0);
        let memory = |plugin: &Maximus| -> Vec<*const f32> {
            let mut places = Vec::new();
            for band in &plugin.bands {
                places.push(band.peak.levels.as_ptr());
                places.push(band.rms.values.as_ptr());
                places.push(band.delay.frames.as_ptr().cast());
                places.push(band.table.gains.as_ptr());
            }
            for taps in &plugin.fir.taps {
                places.push(taps.as_ptr());
            }
            for history in &plugin.fir.history {
                places.push(history.as_ptr());
            }
            places
        };
        let before = memory(&plugin);
        for wheel in [10_000.0, 0.0, 5000.0, 1115.0] {
            for index in [
                LMH_DELAY,
                band(0, WINDOW),
                band(MASTER, ATTACK),
                band(1, RELEASE),
            ] {
                plugin.set_param(index, wheel);
                plugin.process(&mut [0.5; 64], &mut [0.5; 64]);
            }
        }
        for index in [LOW_SLOPE, HIGH_SLOPE, band(0, MODE), band(2, MODE)] {
            for value in [0.0, 1.0, 3.0] {
                plugin.set_param(index, value);
                plugin.process(&mut [0.5; 64], &mut [0.5; 64]);
            }
        }
        assert_eq!(memory(&plugin), before);
        assert_eq!((plugin.param_count(), plugin.param(PARAMS)), (PARAMS, 0.0));
        plugin.set_param(band(1, PRE), 1e9);
        assert_eq!(plugin.param(band(1, PRE)), 2000.0);
    }

    #[test]
    fn every_state_fl_ships_loads_and_runs() {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, the states of {NAME} skipped");
            return;
        };
        let named = |saved: &testing::Saved| saved.plugin.eq_ignore_ascii_case(NAME);
        let projects: Vec<_> = testing::projects(&root).into_iter().filter(named).collect();
        let presets = presets(NAME).unwrap();
        // The doc's counts: 125 states in the projects and 41 presets,
        // the four Soundgoodizer ones among them.
        assert_eq!((projects.len(), presets.len()), (125, 41));
        let mut approximated = std::collections::BTreeMap::new();
        let mut stood_in = Vec::new();
        for saved in projects.iter().chain(&presets) {
            let mut plugin = Maximus::default();
            plugin.set_sample_rate(48_000.0);
            if let Err(error) = plugin.set_state(&saved.state) {
                panic!("{}: {error}", saved.file);
            }
            if let Fidelity::Approximate(what) = plugin.fidelity() {
                *approximated.entry(what).or_insert(0) += 1;
                stood_in.push(format!("{} ({what})", saved.file));
            }
            let (mut left, mut right) = (testing::sine(0.01, 2048), testing::sine(0.037, 2048));
            for (l, r) in left.chunks_mut(300).zip(right.chunks_mut(300)) {
                plugin.process(l, r);
            }
            let sound = left.iter().chain(&right);
            assert!(
                sound.clone().all(|v| v.is_finite() && v.abs() < 100.0),
                "{}",
                saved.file
            );
            assert!(plugin.latency() <= plugin.max_latency());
        }
        // 17 project states and two presets oversample, all of them at
        // order 1, and four project states ask for the linear-phase
        // crossover: all run as FL runs them.
        assert_eq!(
            approximated,
            std::collections::BTreeMap::new(),
            "{}",
            stood_in.join("; ")
        );
        eprintln!("{}", stood_in.join("; "));
        // The doc's readings of some presets.
        let named = |file: &str| {
            let preset = presets.iter().find(|p| p.file == file);
            at_48k(&preset.unwrap_or_else(|| panic!("no preset {file}")).state)
        };
        assert_eq!(named("Default").params, DEFAULTS);
        let gate = named("Gate");
        assert!((gate.curves[MASTER].value_at(0.05)).abs() < 1e-9);
        let rising = gate.curves[MASTER].value_at(0.25);
        assert!(rising > 0.2 && rising < 0.5, "{rising}");
        // "Distortion (no latency)": every band passes, with a shaped
        // saturator on three of them. Its version 4 block ends with one
        // byte of the oversampling word, which FL reads as 2x.
        let distortion = named("Distortion (no latency)");
        assert_eq!(distortion.latency(), 0);
        assert!(matches!(
            distortion.bands[0].saturator,
            Saturator::Shaped { .. }
        ));
        assert_eq!(distortion.switches.oversample, 1);
        let slam = named("Breakbeat slam");
        assert!(matches!(slam.bands[0].saturator, Saturator::Shaped { .. }));
        assert_eq!((slam.params[LOW_SLOPE], slam.bands[0].sign), (0, -1.0));
    }
}
