//! Fruity Delay Bank: eight delay units in a row, each with its own gains,
//! feedback and grain, as `docs/plugins/fruity-delay-bank.md` has it.
//!
//! The state is 792 bytes: a version of 15 or 16, 133 little-endian i32,
//! and 32 bytes for each unit. The i32 are four parameters for the whole
//! plugin, sixteen for each of the eight units, and the oversampling
//! switch:
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Dry, 1 Wet, 2 Input, 3 Feedback | 0 to 128 | 128 |
//! | 4 Input panning | -128 to 128 | 0 |
//! | 5 Input volume | -160 to 160 | 128 |
//! | 6, 7, 8 Input filter cutoff, resonance, gain | 0 to 65536, 65536, 256 | 33145, 17500, 128 |
//! | 9 Feedback panning | -128 to 128 | 0 |
//! | 10 Feedback volume | -128 to 128 | 64 |
//! | 11 Feedback stereo separation | -128 to 128 | 0 |
//! | 12 Time | 0 to 7680 | 1440 |
//! | 13 Time stereo offset | -512 to 512 | 0 |
//! | 14, 15, 16 Feedback filter cutoff, resonance, gain | as 6, 7, 8 | |
//! | 17 Output panning | -128 to 128 | 0 |
//! | 18 Output volume | -160 to 160 | 128 |
//! | 19 To next delay | -160 to 160 | 0 |
//!
//! Unit `u` has its sixteen at `4 + 16 * u`. A unit's 32 bytes are what
//! its switches are at: the two filters' modes and types, whether the
//! input filter is after the delay, the feedback mode, the grain's
//! division and shape, whether the unit is on, whether its first echo is
//! left alone, and whether its time is in beats.
//!
//! A unit takes the plugin's input through its input gains and, where the
//! unit before sends to it, that unit's output. That goes into a delay
//! line, whose output comes back into it through the feedback gains. What
//! leaves the line goes through the grain and the output gains into the
//! wet sum, beside the dry input.
//!
//! Each unit has two filters of up to three alike stages: one on its input,
//! before the line or after the grain, and one on what the line feeds
//! back. They are the cookbook's low pass, band pass, notch, high pass,
//! shelves and peak, designed and run as FL does (`FUN_006debc0`,
//! `FUN_005dc180`, `FUN_005dc9a0`).
//!
//! With the 2x oversampling on, the input goes up through FL's half band
//! ([`dsp::halfband`]), the units run at twice the song's rate (their
//! times, grains, filters and glides all worked out for it, as FL tells
//! its renderer the doubled rate), and the sum comes down through the half
//! band again. FL reports no latency for it, and neither does this.
//!
//! Not built: the bend of the grain's shape. A state that has it says so
//! in [`Effect::fidelity`].

use crate::parts::{Matrix, glide, offset_frames, stereo_gains};
use crate::{Effect, Fidelity, StateError, knob_gain, knob_value, state_version};
use dsp::biquad::{Biquad64, Coefficients64};
use dsp::halfband::{MAX_STAGES, Oversampler, WIDEST};
use std::f64::consts::TAU;

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Delay Bank";

const UNITS: usize = 8;
/// The parameters for the whole plugin, and for one unit.
const GLOBALS: usize = 4;
const EACH: usize = 16;
/// The i32 the state has, the last of them the oversampling switch.
const SAVED: usize = GLOBALS + EACH * UNITS + 1;
const OVERSAMPLING: usize = SAVED - 1;
/// The parameters a host can move: the last unit has no "to next".
const PARAMS: usize = GLOBALS + EACH * UNITS - 1;

/// How long the state is, where its i32 end, and how long a unit's
/// switches are.
const LEN: usize = 792;
const SWITCHES_AT: usize = 4 + 4 * SAVED;
const SWITCHES: usize = 32;

/// The range of each parameter of the plugin and of a unit.
const RANGES: [(i32, i32); GLOBALS + EACH] = [
    (0, 128),
    (0, 128),
    (0, 128),
    (0, 128),
    (-128, 128),
    (-160, 160),
    (0, 65_536),
    (0, 65_536),
    (0, 256),
    (-128, 128),
    (-128, 128),
    (-128, 128),
    (0, 7680),
    (-512, 512),
    (0, 65_536),
    (0, 65_536),
    (0, 256),
    (-128, 128),
    (-160, 160),
    (-160, 160),
];
const DEFAULTS: [i32; GLOBALS + EACH] = [
    128, 128, 128, 128, 0, 128, 33_145, 17_500, 0, 0, 64, 0, 1440, 0, 33_145, 17_500, 0, 0, 128, 0,
];

/// A unit's parameters, counted from the unit's first.
const IN_PAN: usize = 0;
const IN_VOL: usize = 1;
const IN_FILTER: usize = 2;
const FB_PAN: usize = 5;
const FB_VOL: usize = 6;
const FB_SEP: usize = 7;
const TIME: usize = 8;
const OFFSET: usize = 9;
const FB_FILTER: usize = 10;
const OUT_PAN: usize = 13;
const OUT_VOL: usize = 14;
const NEXT: usize = 15;

/// The feedback modes.
const OFF: i32 = 0;
const INVERTED: i32 = 2;
const PING_PONG: i32 = 3;

/// What FL makes 0 of in a delay line: a size under 2^-24.
const TINY: f32 = 5.960_464_5e-8;

/// A volume wheel that goes under 0: the knob curve on its size, with
/// its sign.
fn signed_gain(value: i32) -> f32 {
    let size = value.abs() as f32 * (1.0 / 128.0);
    let gain = if size == 1.0 {
        1.0
    } else {
        knob_gain(f64::from(size)) as f32
    };
    gain * value.signum() as f32
}

/// The range of a parameter by its index.
fn range(index: usize) -> (i32, i32) {
    if index < GLOBALS {
        RANGES[index]
    } else {
        RANGES[GLOBALS + (index - GLOBALS) % EACH]
    }
}

/// A pan and a volume as four gains that glide to where they are put.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Gain {
    pan: f32,
    volume: f32,
    /// The gains the pan and the volume make, worked out by the next
    /// block when `moved` says they are stale, and the gains in use.
    target: [f32; 4],
    current: [f32; 4],
    moved: bool,
}

impl Gain {
    /// The gains for a block's first frame and what to add every frame.
    fn ramp(&mut self, cap: f32, len: usize) -> ([f32; 4], [f32; 4]) {
        if self.moved {
            self.target = stereo_gains(self.pan, self.volume, 0.0);
            self.moved = false;
        }
        ramp(&mut self.current, self.target, cap, len)
    }
}

/// Glide four gains to their targets over a block.
fn ramp(current: &mut [f32; 4], target: [f32; 4], cap: f32, len: usize) -> ([f32; 4], [f32; 4]) {
    let (mut start, mut step) = ([0.0; 4], [0.0; 4]);
    for k in 0..4 {
        (start[k], step[k]) = glide(&mut current[k], target[k], cap, len);
    }
    (start, step)
}

/// The grain's window for a shape of 0: a raised cosine, its two halves
/// measured on their own.
fn window(len: usize) -> Vec<f32> {
    let last = len.saturating_sub(1);
    let half = last / 2;
    let rise = (std::f64::consts::PI / half as f64) as f32;
    let fall = (std::f64::consts::PI / (last - half) as f64) as f32;
    let value = |k: usize| {
        let angle = if k < half {
            rise * k as f32
        } else {
            (f64::from(fall * (k - half) as f32) + std::f64::consts::PI) as f32
        };
        let value = (0.5 - f64::from(angle).cos() * 0.5) as f32;
        // A window of one or two samples divides by 0 in FL.
        if value.is_finite() { value } else { 1.0 }
    };
    (0..len).map(value).collect()
}

/// The designer's kind for each filter mode of the state (`0x7344e0`): off,
/// low pass, band pass, notch, high pass, low shelf, peak, high shelf.
const KINDS: [i32; 8] = [0, 1, 2, 3, 4, 6, 7, 8];
const LOW_PASS: i32 = 1;
const BAND_PASS: i32 = 2;
const NOTCH: i32 = 3;
const HIGH_PASS: i32 = 4;
const LOW_SHELF: i32 = 6;
const PEAK: i32 = 7;
const HIGH_SHELF: i32 = 8;

/// The most stages a filter has room for: `0x178` bytes, three of `0x78`
/// and the filter's own fields.
const STAGES: usize = 3;

/// `float(base^x - 1)` as FL works it, `Exp(double(float(x)) * Ln(base))`,
/// for a wheel's curve.
fn curve(x: f64, base: f64) -> f64 {
    f64::from(((f64::from(x as f32) * base.ln()).exp() - 1.0) as f32)
}

/// FL's `Power` (`FUN_0045d6b0`): `Exp(y * Ln(x))`.
fn power(x: f64, y: f64) -> f64 {
    (y * x.ln()).exp()
}

/// `FUN_005dc180`: the cookbook's coefficients for a designer kind, from
/// the amplitude `a`, the kind's alpha (for a shelf, `2 sqrt(A) alpha` over
/// the sine), the sine and the cosine of the corner, normalised to
/// `a0 = 1`, in FL's order of operations.
fn cookbook(kind: i32, a: f64, alpha: f64, sin: f64, cos: f64) -> Coefficients64 {
    let one = |n: f64| Coefficients64 {
        b0: 0.0,
        b1: 0.0,
        b2: 0.0,
        a1: n * cos * -2.0,
        a2: n * (1.0 - alpha),
    };
    match kind {
        LOW_PASS => {
            let n = 1.0 / (alpha + 1.0);
            let b1 = n * (1.0 - cos);
            let b0 = b1 * 0.5;
            Coefficients64 {
                b0,
                b1,
                b2: b0,
                ..one(n)
            }
        }
        BAND_PASS => {
            let n = 1.0 / (alpha + 1.0);
            Coefficients64 {
                b0: n * alpha,
                b1: 0.0,
                b2: n * -alpha,
                ..one(n)
            }
        }
        NOTCH => {
            let n = 1.0 / (alpha + 1.0);
            let c = one(n);
            Coefficients64 {
                b0: n,
                b1: c.a1,
                b2: n,
                ..c
            }
        }
        HIGH_PASS => {
            let n = 1.0 / (alpha + 1.0);
            let b1 = n * -(cos + 1.0);
            let b0 = -b1 * 0.5;
            Coefficients64 {
                b0,
                b1,
                b2: b0,
                ..one(n)
            }
        }
        PEAK => {
            let n = 1.0 / (alpha / a + 1.0);
            let a1 = n * cos * -2.0;
            Coefficients64 {
                b0: n * (alpha * a + 1.0),
                b1: a1,
                b2: n * (1.0 - alpha * a),
                a1,
                a2: n * (1.0 - alpha / a),
            }
        }
        LOW_SHELF => {
            let tilt = (a - 1.0) * cos;
            let slope = alpha * sin;
            let n = 1.0 / (a + 1.0 + tilt + slope);
            Coefficients64 {
                b0: n * a * (((a + 1.0) - tilt) + slope),
                b1: n * a * 2.0 * ((a - 1.0) - (a + 1.0) * cos),
                b2: n * a * (((a + 1.0) - tilt) - slope),
                a1: n * ((a - 1.0) + (a + 1.0) * cos) * -2.0,
                a2: n * ((a + 1.0 + tilt) - slope),
            }
        }
        HIGH_SHELF => {
            let tilt = (a - 1.0) * cos;
            let slope = alpha * sin;
            let n = 1.0 / (((a + 1.0) - tilt) + slope);
            Coefficients64 {
                b0: n * a * (a + 1.0 + tilt + slope),
                b1: n * a * -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                b2: n * a * ((a + 1.0 + tilt) - slope),
                a1: n * ((a - 1.0) - (a + 1.0) * cos) * 2.0,
                a2: n * (((a + 1.0) - tilt) - slope),
            }
        }
        _ => Coefficients64::IDENTITY,
    }
}

/// One of a unit's two filters (`+0x68` the input's, `+0x1e0` the
/// feedback's): up to three alike stages in a row.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Filter {
    /// The designer's kind, 0 for none, and how many stages run.
    kind: i32,
    stages: usize,
    coefficients: Coefficients64,
    /// Each stage's state, a side at a time.
    state: [[Biquad64; 2]; STAGES],
}

impl Default for Filter {
    fn default() -> Self {
        Filter {
            kind: 0,
            stages: 0,
            coefficients: Coefficients64::IDENTITY,
            state: Default::default(),
        }
    }
}

impl Filter {
    /// `FUN_006debc0` for a mode, a type (stages less one) and the three
    /// wheels: cutoff, resonance or width, and gain. `step` is the
    /// renderer's `44100 / SR` times `2 pi`, so a cutoff counts in parts of
    /// 44.1 kHz.
    ///
    /// The spread is what `FUN_006dfc10` keeps beside the stages from
    /// their count `n`: `1 / n` for the Q of a pass or the amplitude of a
    /// peak or shelf, and `n` for a band pass's width, `1 / n` for a
    /// notch's. FL works it out when the unit is flushed, which a state
    /// load does before it sets the wheels, so it is the count's here.
    fn design(&mut self, mode: i32, kind_of_type: i32, wheels: [i32; 3], step: f64) {
        let kind = usize::try_from(mode)
            .ok()
            .and_then(|m| KINDS.get(m))
            .copied();
        self.kind = kind.unwrap_or(0);
        self.stages = usize::try_from(kind_of_type + 1).map_or(0, |n| n.min(STAGES));
        let n = kind_of_type.saturating_add(1);
        let several = kind_of_type > 0;
        let per_stage = f64::from((1.0 / f64::from(n)) as f32);
        let width_spread = if self.kind == NOTCH {
            f64::from(1.0 / n as f32)
        } else {
            f64::from(n as f32)
        };
        let cut = f64::from(wheels[0]) * (1.0 / 65_536.0);
        let res = f64::from(wheels[1]) * (1.0 / 65_536.0);
        // The corner's sine and cosine, from a cutoff in parts of 44.1 kHz
        // and the offset its kind adds, held to `0..2.8495` radians.
        let corner = |fc: f64, offset: f64| {
            let w = (fc + offset) * step;
            let w = if w <= 2.849_517_146_113_191 {
                w.max(0.0)
            } else {
                2.849_517_146_113_191
            };
            let (sin, cos) = w.sin_cos();
            (w, sin, cos)
        };
        // The cutoff of a band, a notch, a high pass, a peak and a shelf:
        // `(1501^c - 1) * 0.0004`, held under `top`.
        let narrow = |top: f64| {
            if cut > 0.0 {
                (curve(cut, 1501.0) * 0.000_399_933_342_123_404_15).min(top)
            } else {
                0.0
            }
        };
        // The Q of a pass: 0.1 to 50, shared out over the stages.
        let pass_q = || {
            let mut q = if res <= 0.0 {
                0.1
            } else if res < 1.0 {
                curve(res, 2001.0) * 0.024_950_001_388_788_223 + 0.1
            } else {
                50.0
            };
            if several {
                q = power(q, per_stage);
            }
            q
        };
        // The amplitude of a peak or a shelf from the gain wheel: 0.1 at
        // 0, 1 at 128, 2 at 256, shared out over the stages.
        let amplitude = || {
            let g = f64::from(wheels[2] as f32 * (1.0 / 128.0));
            let g = f64::from(((g * 1.111_111_164_093_017_6f64.ln()).exp() - 1.0) as f32);
            let a = g * 8.1 + 0.1;
            if several { power(a, per_stage) } else { a }
        };
        let (a, alpha, sin, cos) = match self.kind {
            LOW_PASS | HIGH_PASS => {
                let (fc, offset) = if self.kind == LOW_PASS {
                    let fc = if cut > 0.0 {
                        curve(cut, 1501.0) * 0.000_664_666_644_297_540_2
                    } else {
                        0.0
                    };
                    (fc, 0.003)
                } else {
                    (narrow(0.6001), 0.0001)
                };
                let k = 0.5 / pass_q();
                let (_, sin, cos) = corner(fc, offset);
                (0.0, sin * k, sin, cos)
            }
            BAND_PASS | NOTCH => {
                let width = if res <= 0.0 {
                    0.01
                } else if res < 1.0 {
                    curve(res, 101.0) * 0.119_899_995_625_019_07 + 0.01
                } else {
                    12.0
                };
                let (w, sin, cos) = corner(narrow(1.0001), 0.0001);
                let s = ((width * width_spread * w) / sin).sinh();
                (0.0, sin * s, sin, cos)
            }
            PEAK => {
                let r = 1.0 - res;
                let q = if r <= 0.0 {
                    0.1
                } else if r < 1.0 {
                    curve(r, 2001.0) * 0.049_949_999_898_672_104 + 0.1
                } else {
                    100.0
                };
                let a = amplitude();
                let (_, sin, cos) = corner(narrow(1.0001), 0.0001);
                (a, sin / (q * 2.0), sin, cos)
            }
            LOW_SHELF | HIGH_SHELF => {
                let r = 1.0 - res;
                let slope = if r <= 0.0 {
                    0.1
                } else if r < 1.0 {
                    curve(r, 3.0) * 0.449_999_988_079_071 + 0.1
                } else {
                    1.0
                };
                let a = amplitude();
                let beta = a.sqrt() * ((a + 1.0 / a) * (1.0 / slope - 1.0) + 2.0).sqrt();
                let (_, sin, cos) = corner(narrow(1.0001), 0.0001);
                (a, beta, sin, cos)
            }
            _ => {
                self.coefficients = Coefficients64::IDENTITY;
                return;
            }
        };
        self.coefficients = cookbook(self.kind, a, alpha, sin, cos);
    }

    /// `FUN_006e1320`: a frame through each stage in turn, in doubles,
    /// back to a float between them (`FUN_005dc9a0`).
    fn run(&mut self, frame: (f32, f32)) -> (f32, f32) {
        let c = &self.coefficients;
        let (mut l, mut r) = frame;
        for [left, right] in &mut self.state[..self.stages] {
            l = left.process(c, f64::from(l)) as f32;
            r = right.process(c, f64::from(r)) as f32;
        }
        (l, r)
    }

    /// The kernel's flush of what is too small for a double to hold, at
    /// the end of a block.
    fn flush(&mut self) {
        for side in self.state.as_flattened_mut() {
            side.flush();
        }
    }

    /// `FUN_006dfc10`'s clear: every stage starts from nothing.
    fn clear(&mut self) {
        self.state = Default::default();
    }
}

/// One delay unit.
#[derive(Debug, Clone, PartialEq, Default)]
struct Unit {
    /// What the unit's 32 bytes have: the filters' modes and types, the
    /// input filter after the delay, the feedback mode, the grain, the
    /// unit on, the first echo left alone, the time in beats.
    filters: [[i32; 2]; 2],
    post: bool,
    /// The input filter and the feedback filter.
    bands: [Filter; 2],
    mode: i32,
    grain_division: i32,
    grain_shape: i32,
    enabled: bool,
    first: bool,
    beats: bool,

    input: Gain,
    output: Gain,
    /// The input and the output wheels' own gains, before the plugin's
    /// Input and Wet are on them, and the gain to the next unit.
    in_volume: f32,
    out_volume: f32,
    next: f32,
    /// The feedback's gains as the last block worked them out, and the
    /// ones in use.
    fb_target: [f32; 4],
    fb_current: [f32; 4],

    /// The delay in frames, how much later each side goes in, the line
    /// and where it is read. A unit with no delay has no line.
    delay: usize,
    late: [usize; 2],
    line: Vec<[f32; 2]>,
    at: usize,
    /// The grain's window, how long it is before rounding, and where in
    /// it the unit is.
    grain: Vec<f32>,
    grain_period: f64,
    grain_phase: f64,
}

impl Unit {
    /// Whether the unit has a delay line: it is on and its feedback mode
    /// is not "off".
    fn delays(&self) -> bool {
        self.enabled && self.mode != OFF
    }

    /// The delay time and the grain, from the two time wheels.
    fn time(&mut self, time: i32, offset: i32, rate: f32, tempo: f32) {
        let late = offset_frames(offset, f64::from(rate));
        let mut frames = (time * rate as i32) as f32;
        if self.beats {
            frames = (f64::from(frames) * 0.031_25 / f64::from(tempo)) as f32;
        } else {
            frames /= 3840.0;
        }
        let delay = (frames.round_ties_even() as usize).max(1);
        if self.delays() {
            // A line of another length starts empty.
            if self.line.len() != delay + late + 1 {
                self.line = vec![[0.0; 2]; delay + late + 1];
                self.at = 0;
            }
            self.delay = delay;
            self.late = if offset > 0 { [0, late] } else { [late, 0] };
        } else {
            self.line = Vec::new();
        }

        let period = f64::from(frames / self.grain_division as f32);
        let len = period.round_ties_even();
        if self.grain_division < 1 || !self.enabled || len.is_nan() || len < 1.0 {
            self.grain = Vec::new();
        } else {
            self.grain_period = period;
            if self.grain.len() != len as usize {
                self.grain = window(len as usize);
            }
        }
    }

    /// What FL's flush does to a unit: the line and the grain start
    /// over, and the gains are where they were last worked out to be.
    fn flush(&mut self) {
        for band in &mut self.bands {
            band.clear();
        }
        self.line.fill([0.0; 2]);
        self.grain_phase = 0.0;
        self.input.current = self.input.target;
        self.output.current = self.output.target;
        self.fb_current = self.fb_target;
    }
}

/// What the units keep for the length of a block: their gains' glides
/// (input, output and feedback for each), where each grain is and how
/// long it has to go, and the block's length at the units' rate.
#[derive(Debug, Clone, Copy, Default)]
struct Block {
    ramps: [([f32; 4], [f32; 4]); UNITS * 3],
    grain_left: [usize; UNITS],
    grain_at: [usize; UNITS],
    len: usize,
}

/// Fruity Delay Bank. A new one has FL's defaults: the first unit on,
/// three steps of delay, ping-pong off, half of every echo fed back.
#[derive(Debug, Clone, PartialEq)]
pub struct DelayBank {
    params: [i32; SAVED],
    /// The song's rate.
    rate: f32,
    tempo: f32,
    units: [Unit; UNITS],
    /// The stage up and the stage down (`0x21d8`, `0x21e0`).
    oversampler: Oversampler,
    /// The plugin's four wheels as gains.
    dry: f32,
    wet: f32,
    input: f32,
    feedback: f32,
}

impl Default for DelayBank {
    fn default() -> Self {
        let mut params = [0; SAVED];
        params[..GLOBALS].copy_from_slice(&DEFAULTS[..GLOBALS]);
        for unit in params[GLOBALS..SAVED - 1].chunks_mut(EACH) {
            unit.copy_from_slice(&DEFAULTS[GLOBALS..]);
        }
        let mut plugin = DelayBank {
            params,
            rate: 44_100.0,
            tempo: 140.0,
            units: Default::default(),
            oversampler: Oversampler::default(),
            dry: 1.0,
            wet: 1.0,
            input: 1.0,
            feedback: 1.0,
        };
        for unit in &mut plugin.units {
            unit.mode = 1;
            unit.beats = true;
        }
        plugin.units[0].enabled = true;
        plugin.set();
        plugin
    }
}

impl DelayBank {
    /// A unit's parameter.
    fn of(&self, unit: usize, which: usize) -> i32 {
        self.params[GLOBALS + EACH * unit + which]
    }

    /// How many times the rate is doubled: the state's switch
    /// (`FUN_006dff00`).
    fn stages(&self) -> usize {
        self.params[OVERSAMPLING] as usize
    }

    /// The rate the units run at, which FL gives its renderer: the song's
    /// times the oversampling.
    fn unit_rate(&self) -> f32 {
        self.rate * (1 << self.stages()) as f32
    }

    /// Work out everything the parameters, the switches, the rate and
    /// the tempo make. The gains are left for the next block to glide to.
    fn set(&mut self) {
        let plain = |value: i32| {
            let p = value as f32 * (1.0 / 128.0);
            if p == 1.0 {
                1.0
            } else {
                knob_gain(f64::from(p)) as f32
            }
        };
        self.dry = plain(self.params[0]);
        self.wet = plain(self.params[1]);
        self.input = plain(self.params[2]);
        self.feedback = self.params[3] as f32 * (1.0 / 128.0);
        for at in 0..UNITS {
            let of = |which: usize| self.of(at, which);
            let (in_pan, in_vol) = (of(IN_PAN), of(IN_VOL));
            let (out_pan, out_vol, next) = (of(OUT_PAN), of(OUT_VOL), of(NEXT));
            let (time, offset) = (of(TIME), of(OFFSET));
            let wheels = |first: usize| [of(first), of(first + 1), of(first + 2)];
            let (in_wheels, fb_wheels) = (wheels(IN_FILTER), wheels(FB_FILTER));
            let (input, wet, rate, tempo) = (self.input, self.wet, self.unit_rate(), self.tempo);
            let unit = &mut self.units[at];
            // The renderer's `44100 / SR` (`+0x20`) times `2 pi`.
            let step = 44_100.0 / f64::from(rate) * TAU;
            for (band, wheels) in [in_wheels, fb_wheels].into_iter().enumerate() {
                let [mode, kind] = unit.filters[band];
                unit.bands[band].design(mode, kind, wheels, step);
            }
            unit.in_volume = signed_gain(in_vol);
            unit.out_volume = signed_gain(out_vol);
            unit.next = signed_gain(next);
            let gains = [
                (&mut unit.input, in_pan, unit.in_volume * input),
                (&mut unit.output, out_pan, unit.out_volume * wet),
            ];
            for (gain, pan, volume) in gains {
                let pan = pan as f32 * (1.0 / 128.0);
                if gain.pan != pan || gain.volume != volume {
                    (gain.pan, gain.volume, gain.moved) = (pan, volume, true);
                }
            }
            unit.time(time, offset, rate, tempo);
        }
    }

    /// The feedback's four gains for a unit, as FL works them out for
    /// every block.
    fn feedback_gains(&self, at: usize) -> [f32; 4] {
        let pan = self.of(at, FB_PAN) as f32 * (1.0 / 128.0);
        let mut volume = self.of(at, FB_VOL) as f32 * (1.0 / 128.0);
        let separation = self.of(at, FB_SEP) as f32 * (1.0 / 128.0);
        if separation < 0.0 {
            volume *= separation * 0.5 + 1.0;
        }
        let [ll, rr, rl, lr] = stereo_gains(pan, volume * self.feedback, separation);
        if self.units[at].mode == PING_PONG {
            // Each side feeds the other.
            [lr, rl, rr, ll]
        } else {
            [ll, rr, rl, lr]
        }
    }

    /// One frame through the units, `frame` frames into a block of
    /// `block.len` at the units' rate: the dry part and every unit's
    /// output.
    fn frame(
        &mut self,
        block: &mut Block,
        frame: usize,
        (dry_left, dry_right): (f32, f32),
    ) -> (f32, f32) {
        let mut wet = (dry_left * self.dry, dry_right * self.dry);
        let mut before = (0.0, 0.0);
        let mut send = 0.0;
        for (at, unit) in self.units.iter_mut().enumerate() {
            if !unit.enabled {
                send = 0.0;
                continue;
            }
            let [input, output, feedback] = [0, 1, 2].map(|k| {
                let (start, step) = block.ramps[at * 3 + k];
                std::array::from_fn::<f32, 4, _>(|g| start[g] + step[g] * frame as f32)
            });
            let mut x = Matrix::mix(&input, dry_left, dry_right);
            if send != 0.0 {
                x.0 += before.0 * send;
                x.1 += before.1 * send;
            }
            // The input filter's place (`+0x358`): none with its mode
            // at 0, else before the line or after the grain.
            let in_filter = unit.filters[0][0] != 0;
            if in_filter && !unit.post {
                x = unit.bands[0].run(x);
            }
            if !unit.line.is_empty() {
                let size = unit.line.len();
                let write = (unit.at + unit.delay) % size;
                unit.line[(write + unit.late[0]) % size][0] += x.0;
                unit.line[(write + unit.late[1]) % size][1] += x.1;
                let [out_left, out_right] = unit.line[unit.at];
                // The feedback filter works on what leaves the line,
                // after the first echo is taken from it.
                let mut heard = (out_left, out_right);
                if unit.filters[1][0] > 0 {
                    heard = unit.bands[1].run(heard);
                }
                let mut back = Matrix::mix(&feedback, heard.0, heard.1);
                if back.0.abs() < TINY {
                    back.0 = 0.0;
                }
                if back.1.abs() < TINY {
                    back.1 = 0.0;
                }
                x = if unit.first {
                    (out_left, out_right)
                } else {
                    back
                };
                unit.line[unit.at] = [0.0; 2];
                unit.line[write][0] += back.0;
                unit.line[write][1] += back.1;
                unit.at = (unit.at + 1) % size;
                if unit.mode == INVERTED {
                    x = (x.1, x.0);
                }
            }
            if !unit.grain.is_empty() {
                if block.grain_left[at] == 0 {
                    while unit.grain_period <= unit.grain_phase {
                        unit.grain_phase -= unit.grain_period;
                    }
                    let phase = unit.grain_phase.round_ties_even() as usize;
                    block.grain_at[at] = phase % unit.grain.len();
                    block.grain_left[at] =
                        (unit.grain.len() - block.grain_at[at]).min(block.len - frame);
                    unit.grain_phase += block.grain_left[at] as f64;
                }
                let shape = unit.grain[block.grain_at[at]];
                x = (shape * x.0, shape * x.1);
                block.grain_at[at] += 1;
                block.grain_left[at] -= 1;
            }
            if in_filter && unit.post {
                x = unit.bands[0].run(x);
            }
            let out = Matrix::mix(&output, x.0, x.1);
            wet.0 += out.0;
            wet.1 += out.1;
            before = x;
            send = unit.next;
        }
        wet
    }

    /// What of the state this does not do, if anything.
    fn missing(&self) -> Option<&'static str> {
        let mut on = self.units.iter().filter(|unit| unit.enabled);
        if on.any(|u| !u.grain.is_empty() && u.grain_shape != 0) {
            Some("Delay Bank's grain shape is not built: the grain is a raised cosine")
        } else {
            None
        }
    }
}

impl Effect for DelayBank {
    /// FL reads a version of 15 or more and leaves the plugin alone for
    /// an older one. Before 16 the time wheel counted in tens.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let version = state_version(bytes, LEN)?;
        if version < 15 {
            return Err(StateError::Version { found: version });
        }
        if bytes.len() != LEN {
            return Err(StateError::Length {
                expected: LEN,
                found: bytes.len(),
            });
        }
        let mut params = [0; SAVED];
        for (param, word) in bytes[4..SWITCHES_AT].as_chunks::<4>().0.iter().enumerate() {
            let mut value = i32::from_le_bytes(*word);
            if param < OVERSAMPLING {
                let unit = param >= GLOBALS && (param - GLOBALS) % EACH == TIME;
                if unit && version < 16 {
                    value = value.saturating_mul(10);
                }
            }
            // FL takes the switch unchecked as the oversamplers' stage
            // count (`FUN_006dff00`); its button sets 0 or 1. Past the
            // stages the half band has room for, the state is refused.
            let (min, max) = if param == OVERSAMPLING {
                (0, MAX_STAGES as i32)
            } else {
                range(param)
            };
            if value < min || value > max {
                return Err(StateError::Range { param, value });
            }
            params[param] = value;
        }
        self.params = params;
        let switches = bytes[SWITCHES_AT..].as_chunks::<SWITCHES>().0;
        for (unit, bytes) in self.units.iter_mut().zip(switches) {
            let word = |at: usize| i32::from_le_bytes(*bytes[at..].first_chunk::<4>().unwrap());
            unit.filters = [[word(0), word(4)], [word(8), word(12)]];
            unit.post = bytes[16] != 0;
            unit.mode = word(17);
            unit.grain_division = word(21);
            unit.grain_shape = word(25);
            unit.enabled = bytes[29] != 0;
            unit.first = bytes[30] != 0;
            unit.beats = bytes[31] != 0;
        }
        // FL builds the oversampler again (`FUN_006dff00`), sets the units
        // up, flushes them, and only then takes the parameters: the gains
        // glide to them from where they were.
        self.oversampler.reset();
        for unit in &mut self.units {
            unit.flush();
        }
        self.set();
        Ok(())
    }

    /// The delay times are worked out for the rate, and the oversampler
    /// is built again (`FUN_006dff00`). A rate that is not a number above
    /// 0 is not taken.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz > 0.0 {
            self.rate = hz;
            self.oversampler.reset();
            self.set();
            for unit in &mut self.units {
                unit.flush();
            }
        }
    }

    /// The times of the units that count in beats follow the tempo.
    fn set_tempo(&mut self, bpm: f64) {
        if bpm.is_finite() && bpm > 0.0 && bpm as f32 != self.tempo {
            self.tempo = bpm as f32;
            self.set();
        }
    }

    /// `Eff_Render` (`0x006e1380`): up when the oversampling is on, the
    /// units, and down.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        if len == 0 {
            return;
        }
        let stages = self.stages();
        // The units see a block `2^stages` times as long, and the most a
        // gain moves a frame is for their rate, as FL's renderer has it.
        let mut block = Block {
            len: len << stages,
            ..Block::default()
        };
        let cap = 0.001 * 44_100.0 / self.unit_rate();
        for at in 0..UNITS {
            if !self.units[at].enabled {
                continue;
            }
            let feedback = self.feedback_gains(at);
            let unit = &mut self.units[at];
            block.ramps[at * 3] = unit.input.ramp(cap, block.len);
            block.ramps[at * 3 + 1] = unit.output.ramp(cap, block.len);
            if !unit.line.is_empty() {
                unit.fb_target = feedback;
                block.ramps[at * 3 + 2] = ramp(&mut unit.fb_current, feedback, cap, block.len);
            }
        }

        let (left, right) = (&mut left[..len], &mut right[..len]);
        if stages == 0 {
            for (frame, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
                (*l, *r) = self.frame(&mut block, frame, (*l, *r));
            }
        } else {
            let mut oversampler = self.oversampler;
            for (n, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
                let mut wide = [[0.0; 2]; WIDEST];
                oversampler.up(stages, [*l, *r], &mut wide);
                for (k, frame) in wide[..1 << stages].iter_mut().enumerate() {
                    let (l, r) = self.frame(&mut block, (n << stages) + k, (frame[0], frame[1]));
                    *frame = [l, r];
                }
                [*l, *r] = oversampler.down(stages, &mut wide);
            }
            oversampler.flush();
            self.oversampler = oversampler;
        }
        for unit in &mut self.units {
            for band in &mut unit.bands {
                band.flush();
            }
        }
    }

    fn param_count(&self) -> usize {
        PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        if index < PARAMS {
            self.params[index] as f32
        } else {
            0.0
        }
    }

    /// A time wheel that moves makes a line of another length, which asks
    /// for memory here and never in [`Effect::process`].
    fn set_param(&mut self, index: usize, value: f32) {
        if index < PARAMS {
            self.params[index] = knob_value(value, range(index));
            self.set();
        }
    }

    /// What FL's flush does: the lines empty, the grains at their start,
    /// and every gain where the last block worked it out to be. The
    /// oversampler keeps what it has (`FUN_006dfe30` leaves it).
    fn reset(&mut self) {
        for unit in &mut self.units {
            unit.flush();
        }
    }

    fn fidelity(&self) -> Fidelity {
        self.missing()
            .map_or(Fidelity::Exact, Fidelity::Approximate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{presets, state};
    use dsp::halfband::HalfBand;

    /// What a unit's 32 bytes are: its switches.
    #[derive(Clone, Copy)]
    struct Switches {
        mode: i32,
        grain: i32,
        enabled: bool,
        first: bool,
        beats: bool,
        in_filter: i32,
    }

    const ON: Switches = Switches {
        mode: 1,
        grain: 0,
        enabled: true,
        first: true,
        beats: false,
        in_filter: 0,
    };
    const NOT_ON: Switches = Switches {
        enabled: false,
        ..ON
    };

    /// A state of version 16 with these units on and these parameters
    /// moved from the defaults.
    fn saved(units: &[Switches], moves: &[(usize, i32)]) -> Vec<u8> {
        let mut params = DelayBank::default().params;
        for (index, value) in moves {
            params[*index] = *value;
        }
        let mut bytes = state(&[16]);
        bytes.extend(state(&params));
        for at in 0..UNITS {
            let unit = units.get(at).copied().unwrap_or(NOT_ON);
            bytes.extend(state(&[unit.in_filter, 0, 0, 0]));
            bytes.push(0);
            bytes.extend(state(&[unit.mode, unit.grain, 0]));
            bytes.extend([unit.enabled, unit.first, unit.beats].map(u8::from));
        }
        assert_eq!(bytes.len(), LEN);
        bytes
    }

    /// A unit's parameter by its place in the unit.
    fn at(unit: usize, which: usize) -> usize {
        GLOBALS + EACH * unit + which
    }

    /// A plugin with a state, after the three blocks its gains take to
    /// come up from nothing.
    fn with(units: &[Switches], moves: &[(usize, i32)]) -> DelayBank {
        let mut plugin = DelayBank::default();
        plugin.set_sample_rate(48_000.0);
        plugin.set_state(&saved(units, moves)).unwrap();
        for _ in 0..3 {
            plugin.process(&mut [0.0; 512], &mut [0.0; 512]);
        }
        plugin
    }

    /// A click on the left, or on both sides, through the plugin in
    /// blocks of 512: where each side is not 0, and what it is there.
    fn click(plugin: &mut DelayBank, sides: [f32; 2], len: usize) -> [Vec<(usize, f32)>; 2] {
        let (mut left, mut right) = (vec![0.0; len], vec![0.0; len]);
        (left[0], right[0]) = (sides[0], sides[1]);
        for (l, r) in left.chunks_mut(512).zip(right.chunks_mut(512)) {
            plugin.process(l, r);
        }
        [left, right].map(|side| {
            let heard = side.iter().copied().enumerate();
            heard.filter(|(_, v)| *v != 0.0).collect()
        })
    }

    #[test]
    fn the_defaults_are_the_wheels_of_fl_s_window() {
        let plugin = DelayBank::default();
        assert_eq!(plugin.param_count(), 131);
        let unit: Vec<f32> = (4..20).map(|index| plugin.param(index)).collect();
        assert_eq!(
            unit,
            DEFAULTS[4..].iter().map(|v| *v as f32).collect::<Vec<_>>()
        );
        assert_eq!(plugin.param(at(7, TIME)), 1440.0);
        assert_eq!(plugin.param(131), 0.0);
        assert_eq!((plugin.dry, plugin.wet, plugin.feedback), (1.0, 1.0, 1.0));
        // The first unit is on, at three steps of 140 BPM.
        assert!(plugin.units[0].enabled && !plugin.units[1].enabled);
        assert_eq!(plugin.units[0].delay, 14_175);
        assert_eq!(plugin.latency(), 0);
        assert_eq!(plugin.fidelity(), Fidelity::Exact);
    }

    #[test]
    fn with_no_feedback_each_line_gives_one_echo_at_its_delay() {
        // Two units, 0.375 s and 0.1875 s, their first echoes left alone
        // and nothing fed back.
        let moves = [(at(0, FB_VOL), 0), (at(1, FB_VOL), 0), (at(1, TIME), 720)];
        let mut plugin = with(&[ON, ON], &moves);
        assert_eq!(
            (plugin.units[0].delay, plugin.units[1].delay),
            (18_000, 9000)
        );
        let [left, right] = click(&mut plugin, [1.0, 1.0], 48_000);
        assert_eq!(left, [(0, 1.0), (9000, 1.0), (18_000, 1.0)]);
        assert_eq!(right, left);
    }

    #[test]
    fn the_feedback_scales_every_echo_and_the_first_one_too_unless_it_is_left() {
        // Half of what leaves the line goes back in.
        let mut plugin = with(&[ON], &[]);
        let [left, _] = click(&mut plugin, [1.0, 1.0], 60_000);
        assert_eq!(
            left,
            [(0, 1.0), (18_000, 1.0), (36_000, 0.5), (54_000, 0.25)]
        );
        // With the first echo not left alone, the output is what goes
        // back: the first echo has the feedback's gain already.
        let processed = Switches { first: false, ..ON };
        let mut plugin = with(&[processed], &[]);
        let [left, _] = click(&mut plugin, [1.0, 1.0], 60_000);
        assert_eq!(
            left,
            [(0, 1.0), (18_000, 0.5), (36_000, 0.25), (54_000, 0.125)]
        );
        // The plugin's Feedback wheel is on top of the unit's.
        let mut plugin = with(&[ON], &[(3, 64)]);
        let [left, _] = click(&mut plugin, [1.0, 1.0], 40_000);
        assert_eq!(left, [(0, 1.0), (18_000, 1.0), (36_000, 0.25)]);
    }

    #[test]
    fn ping_pong_crosses_the_sides_and_inverted_swaps_the_output() {
        let pong = Switches {
            mode: PING_PONG,
            ..ON
        };
        let mut plugin = with(&[pong], &[]);
        let [left, right] = click(&mut plugin, [1.0, 0.0], 60_000);
        assert_eq!(left, [(0, 1.0), (18_000, 1.0), (54_000, 0.25)]);
        assert_eq!(right, [(36_000, 0.5)]);
        let inverted = Switches {
            mode: INVERTED,
            ..ON
        };
        let mut plugin = with(&[inverted], &[]);
        let [left, right] = click(&mut plugin, [1.0, 0.0], 40_000);
        assert_eq!(left, [(0, 1.0)]);
        assert_eq!(right, [(18_000, 1.0), (36_000, 0.5)]);
        // With the feedback mode off there is no line: the unit's input
        // is its output.
        let off = Switches { mode: OFF, ..ON };
        let mut plugin = with(&[off], &[]);
        let [left, _] = click(&mut plugin, [1.0, 1.0], 20_000);
        assert_eq!(left, [(0, 2.0)]);
    }

    #[test]
    fn the_time_is_in_seconds_or_in_beats_and_one_side_can_be_late() {
        // 1920 to a beat: 1440 is three steps, 16 800 frames at 150 BPM.
        let beats = Switches { beats: true, ..ON };
        let mut plugin = with(&[beats], &[(at(0, FB_VOL), 0)]);
        plugin.set_tempo(150.0);
        assert_eq!(plugin.units[0].delay, 14_400);
        let [left, _] = click(&mut plugin, [1.0, 1.0], 20_000);
        assert_eq!(left, [(0, 1.0), (14_400, 1.0)]);
        // An offset above 0 holds the right side back, one under 0 the
        // left: 256 is 15.3 ms.
        for (offset, late) in [(256, 1), (-256, 0)] {
            let moves = [(at(0, FB_VOL), 0), (at(0, OFFSET), offset)];
            let mut plugin = with(&[ON], &moves);
            let sides = click(&mut plugin, [1.0, 1.0], 20_000);
            assert_eq!(sides[1 - late], [(0, 1.0), (18_000, 1.0)]);
            assert_eq!(sides[late], [(0, 1.0), (18_000 + 735, 1.0)]);
        }
    }

    #[test]
    fn a_unit_sends_its_output_to_the_next_and_the_wheels_are_knob_curves() {
        // The second unit hears nothing of the input, and all of what
        // the first puts out.
        let moves = [
            (at(0, FB_VOL), 0),
            (at(1, FB_VOL), 0),
            (at(0, NEXT), 128),
            (at(1, IN_VOL), 0),
            (at(1, TIME), 720),
        ];
        let mut plugin = with(&[ON, ON], &moves);
        let [left, _] = click(&mut plugin, [1.0, 1.0], 30_000);
        assert_eq!(left, [(0, 1.0), (18_000, 1.0), (27_000, 1.0)]);
        // Dry off, wet and the output wheel as the knob curve, the input
        // wheel under 0 turned over.
        let half = knob_gain(0.5) as f32;
        let moves = [
            (0, 0),
            (1, 64),
            (at(0, FB_VOL), 0),
            (at(0, IN_VOL), -128),
            (at(0, OUT_VOL), 64),
        ];
        let mut plugin = with(&[ON], &moves);
        let [left, _] = click(&mut plugin, [1.0, 1.0], 20_000);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].0, 18_000);
        assert!((left[0].1 + half * half).abs() < 1e-6, "{}", left[0].1);
        // A pan all the way right takes the left side's echo away.
        let mut plugin = with(&[ON], &[(at(0, FB_VOL), 0), (at(0, OUT_PAN), 128)]);
        let [left, right] = click(&mut plugin, [1.0, 1.0], 20_000);
        assert!(left.len() == 2 && left[1].1.abs() < 1e-6, "{left:?}");
        assert!(right.len() == 2 && right[1].1 > 1.0, "{right:?}");
    }

    #[test]
    fn the_gains_come_up_from_nothing_after_a_state() {
        let mut plugin = DelayBank::default();
        plugin.set_sample_rate(44_100.0);
        plugin.set_state(&saved(&[ON], &[])).unwrap();
        // A thousandth a frame at 44.1 kHz: a block of 512 gets half way.
        plugin.process(&mut [0.0; 512], &mut [0.0; 512]);
        assert!((plugin.units[0].input.current[0] - 0.512).abs() < 1e-6);
        plugin.process(&mut [0.0; 512], &mut [0.0; 512]);
        assert_eq!(plugin.units[0].input.current, [1.0, 1.0, 0.0, 0.0]);
        // A flush leaves them there and empties the line.
        plugin.process(&mut [0.5; 64], &mut [0.5; 64]);
        plugin.reset();
        assert_eq!(plugin.units[0].input.current[0], 1.0);
        assert!(plugin.units[0].line.iter().all(|frame| *frame == [0.0; 2]));
    }

    #[test]
    fn the_grain_is_a_raised_cosine_a_division_of_the_delay_long() {
        let window = window(9);
        assert_eq!(window[0], 0.0);
        assert!((window[4] - 1.0).abs() < 1e-6);
        assert!((window[2] - 0.5).abs() < 1e-6 && (window[6] - 0.5).abs() < 1e-6);
        assert!(window[8].abs() < 1e-6);
        // Four grains to a delay of 18 000 frames.
        let grainy = Switches { grain: 4, ..ON };
        let plugin = with(&[grainy], &[]);
        assert_eq!(plugin.units[0].grain.len(), 4500);
        // A level that stands still comes out of the line in the
        // window's shape.
        let mut plugin = with(&[grainy], &[(0, 0), (at(0, FB_VOL), 0)]);
        let (mut left, mut right) = (vec![1.0; 40_000], vec![1.0; 40_000]);
        plugin.process(&mut left, &mut right);
        let from = 18_000 + 4500 - (18_000 + 1536) % 4500;
        assert!(left[from].abs() < 1e-6, "{}", left[from]);
        assert!((left[from + 2250] - 1.0).abs() < 1e-5);
        assert!((left[from + 1125] - 0.5).abs() < 1e-3);
    }

    #[test]
    fn a_state_with_what_is_not_built_says_so() {
        // The filters are built.
        let filtered = Switches { in_filter: 4, ..ON };
        let mut plugin = with(&[filtered], &[]);
        assert_eq!(plugin.fidelity(), Fidelity::Exact);
        // So is the oversampling, the state's last i32: a stage count,
        // which FL's button sets to 0 or 1 and its loader takes as it is.
        let mut bytes = saved(&[ON], &[]);
        bytes[SWITCHES_AT - 4] = 1;
        plugin.set_state(&bytes).unwrap();
        assert_eq!((plugin.stages(), plugin.fidelity()), (1, Fidelity::Exact));
        bytes[SWITCHES_AT - 4] = 2;
        plugin.set_state(&bytes).unwrap();
        assert_eq!((plugin.stages(), plugin.fidelity()), (2, Fidelity::Exact));
        assert_eq!(plugin.unit_rate(), 4.0 * plugin.rate);
        // The grain's bend is not.
        let shaped = Switches { grain: 2, ..ON };
        let mut bytes = saved(&[shaped], &[]);
        bytes[SWITCHES_AT + 25..SWITCHES_AT + 29].copy_from_slice(&64i32.to_le_bytes());
        plugin.set_state(&bytes).unwrap();
        assert!(matches!(plugin.fidelity(), Fidelity::Approximate(text) if text.contains("grain")));
    }

    /// A plugin at `hz` with this state, not run yet.
    fn fresh(hz: f32, units: &[Switches], moves: &[(usize, i32)]) -> DelayBank {
        let mut plugin = DelayBank::default();
        plugin.set_sample_rate(hz);
        plugin.set_state(&saved(units, moves)).unwrap();
        plugin
    }

    #[test]
    fn oversampled_the_units_run_at_twice_the_rate_between_the_half_bands() {
        // The plugin at 44.1 kHz with the oversampling on is the input up
        // through FL's half band, the plugin at 88.2 kHz without it on
        // blocks twice as long, and the half band down, to the bit: the
        // units' times, filters, grains and glides are worked out for the
        // doubled rate.
        let filtered = Switches {
            in_filter: 1,
            grain: 3,
            first: false,
            mode: 3,
            ..ON
        };
        let units = [filtered, ON];
        let moves = [
            (at(0, IN_FILTER), 20_000),
            (at(0, OFFSET), 100),
            (at(0, NEXT), 90),
            (at(1, TIME), 700),
        ];
        let mut both = moves.to_vec();
        both.push((OVERSAMPLING, 1));
        let mut over = fresh(44_100.0, &units, &both);
        let mut plain = fresh(88_200.0, &units, &moves);
        assert_eq!(over.latency(), 0);
        assert_eq!(over.units[0].delay, plain.units[0].delay);
        assert_eq!(over.units[0].grain, plain.units[0].grain);
        assert_eq!(over.units[0].late, plain.units[0].late);
        let input: Vec<f32> = (0..20_000)
            .map(|n| ((n * 7919 % 1009) as f32 / 1009.0 - 0.5) * 0.8)
            .collect();
        let (mut up, mut down) = ([HalfBand::default(); 2], [HalfBand::default(); 2]);
        for block in input.chunks(500) {
            let sides = [block.to_vec(), block.iter().map(|x| -x).collect::<Vec<_>>()];
            let [mut left, mut right] = sides.clone();
            over.process(&mut left, &mut right);
            let mut wide = [vec![0.0; 1000], vec![0.0; 1000]];
            for (side, x) in sides.iter().enumerate() {
                up[side].up(x, &mut wide[side]);
                up[side].flush();
            }
            let [wl, wr] = &mut wide;
            plain.process(wl, wr);
            for (side, got) in [left, right].iter().enumerate() {
                let mut want = vec![0.0; 500];
                down[side].down(&wide[side], &mut want);
                down[side].flush();
                assert_eq!(*got, want, "side {side}");
            }
        }
    }

    #[test]
    fn oversampled_an_echo_is_where_and_as_loud_as_it_is_without() {
        // One echo of a click after 0.375 s, the first one left alone.
        // At twice the rate the line is twice as long; out of the half
        // bands the echo is the click again, under three frames late,
        // with the dry click as late.
        let moves = [(at(0, FB_VOL), 0), (OVERSAMPLING, 1)];
        let mut plugin = fresh(44_100.0, &[ON], &moves);
        assert_eq!(plugin.units[0].delay, 33_075);
        assert_eq!(fresh(44_100.0, &[ON], &moves[..1]).units[0].delay, 16_538);
        // The units' gains come up from 0 over 23 ms first.
        for _ in 0..4 {
            plugin.process(&mut [0.0; 512], &mut [0.0; 512]);
        }
        let mut left = vec![0.0; 20_000];
        let mut right = vec![0.0; 20_000];
        left[0] = 1.0;
        right[0] = 1.0;
        for (l, r) in left.chunks_mut(512).zip(right.chunks_mut(512)) {
            plugin.process(l, r);
        }
        let dry: f32 = left[..100].iter().sum();
        let echo: f32 = left[16_400..16_700].iter().sum();
        assert!((dry - 1.0).abs() < 1e-3, "{dry}");
        assert!((echo - 1.0).abs() < 1e-3, "{echo}");
        let peak = |range: std::ops::Range<usize>| {
            range
                .max_by(|a, b| left[*a].abs().total_cmp(&left[*b].abs()))
                .unwrap()
        };
        assert!((1..4).contains(&peak(0..100)), "{}", peak(0..100));
        let at = peak(16_400..16_700);
        assert!((16_538..16_542).contains(&at), "{at}");
        assert!(right == left);
    }

    /// The gain of coefficients at `turns` a sample.
    fn gain(c: &Coefficients64, turns: f64) -> f64 {
        let w = TAU * turns;
        let (z1, z2) = ((-w).sin_cos(), (-2.0 * w).sin_cos());
        let num = (c.b0 + c.b1 * z1.1 + c.b2 * z2.1, c.b1 * z1.0 + c.b2 * z2.0);
        let den = (1.0 + c.a1 * z1.1 + c.a2 * z2.1, c.a1 * z1.0 + c.a2 * z2.0);
        (num.0.hypot(num.1)) / (den.0.hypot(den.1))
    }

    /// `FUN_005dc180` is the cookbook: each kind has its gain where the
    /// cookbook puts it.
    #[test]
    fn the_designer_is_the_cookbook() {
        let w = 0.3f64;
        let (sin, cos) = w.sin_cos();
        let turns = w / TAU;
        let alpha = sin / (2.0 * 0.9);
        let at =
            |kind: i32, a: f64, alpha: f64, f: f64| gain(&cookbook(kind, a, alpha, sin, cos), f);
        assert!((at(LOW_PASS, 0.0, alpha, 0.0) - 1.0).abs() < 1e-12);
        assert!(at(LOW_PASS, 0.0, alpha, 0.5) < 1e-12);
        assert!(at(HIGH_PASS, 0.0, alpha, 0.0) < 1e-12);
        assert!((at(HIGH_PASS, 0.0, alpha, 0.5) - 1.0).abs() < 1e-12);
        assert!((at(BAND_PASS, 0.0, alpha, turns) - 1.0).abs() < 1e-12);
        assert!(at(NOTCH, 0.0, alpha, turns) < 1e-9);
        // A peak of amplitude 1.5 is 1.5^2 at its centre, and a shelf is
        // that far from the other end.
        assert!((at(PEAK, 1.5, alpha, turns) - 2.25).abs() < 1e-9);
        let beta = 1.5f64.sqrt() * ((1.5 + 1.0 / 1.5) * (1.0 / 0.8 - 1.0) + 2.0f64).sqrt();
        assert!((at(LOW_SHELF, 1.5, beta, 0.0) - 2.25).abs() < 1e-9);
        assert!((at(LOW_SHELF, 1.5, beta, 0.5) - 1.0).abs() < 1e-9);
        assert!((at(HIGH_SHELF, 1.5, beta, 0.5) - 2.25).abs() < 1e-9);
        assert!((at(HIGH_SHELF, 1.5, beta, 0.0) - 1.0).abs() < 1e-9);
    }

    /// The wheels as `FUN_006debc0` reads them, at 44.1 kHz where a
    /// cutoff is in parts of the rate.
    #[test]
    fn the_wheels_make_the_corner_and_the_width() {
        let step = TAU;
        let mut filter = Filter::default();
        // A low pass at its default cutoff: `(1501^0.506 - 1) * 0.00066`
        // of 44.1 kHz and 0.003 more, 1.29 kHz, -3 dB there at the Q of
        // its wheel at 0, which is 0.1 here: well down.
        filter.design(1, 0, [33_145, 0, 128], step);
        assert_eq!((filter.kind, filter.stages), (LOW_PASS, 1));
        let fc = curve(33_145.0 / 65_536.0, 1501.0) * 0.000_664_666_644_297_540_2 + 0.003;
        assert!((fc * 44_100.0 - 1290.0).abs() < 10.0, "{}", fc * 44_100.0);
        let q = 0.1;
        let want = q; // a low pass is Q at its corner
        assert!((gain(&filter.coefficients, fc) - want).abs() < 1e-6);
        // At the top of the resonance wheel the Q is 50.
        filter.design(1, 0, [33_145, 65_536, 128], step);
        assert!((gain(&filter.coefficients, fc) - 50.0).abs() < 1e-3);
        // Three stages share the Q out: each has 50^(1/3).
        filter.design(1, 2, [33_145, 65_536, 128], step);
        assert_eq!(filter.stages, 3);
        let each = power(50.0, f64::from(1.0f32 / 3.0));
        assert!((gain(&filter.coefficients, fc) - each).abs() < 1e-3);
        // A peak at 128 on its gain wheel does nothing, and at 256 it is
        // twice the amplitude: four times the gain at its centre.
        filter.design(6, 0, [30_000, 30_000, 128], step);
        assert_eq!(filter.kind, PEAK);
        let centre = curve(30_000.0 / 65_536.0, 1501.0) * 0.000_399_933_342_123_404_15 + 0.0001;
        assert!((gain(&filter.coefficients, centre) - 1.0).abs() < 1e-6);
        filter.design(6, 0, [30_000, 30_000, 256], step);
        assert!((gain(&filter.coefficients, centre) - 4.0).abs() < 1e-3);
        // A notch has nothing at its centre, a band pass all of it.
        filter.design(3, 0, [30_000, 30_000, 128], step);
        assert!(gain(&filter.coefficients, centre) < 1e-9);
        filter.design(2, 2, [30_000, 30_000, 128], step);
        assert!((gain(&filter.coefficients, centre) - 1.0).abs() < 1e-9);
        // Mode 0 is no filter.
        filter.design(0, 0, [30_000, 30_000, 128], step);
        assert_eq!(filter.coefficients, Coefficients64::IDENTITY);
    }

    /// The input filter before the line and after it, and the feedback
    /// filter on each echo.
    #[test]
    fn the_filters_shape_the_echoes() {
        // An echo of a tone through a unit whose input is low passed at
        // its default cutoff: low tones pass, high ones do not.
        let tone = |plugin: &mut DelayBank, hz: f64| {
            let turns = hz / 48_000.0;
            let mut left = crate::testing::sine(turns, 48_000);
            let mut right = left.clone();
            for (l, r) in left.chunks_mut(512).zip(right.chunks_mut(512)) {
                plugin.process(l, r);
            }
            crate::testing::partial(&left[24_000..], turns)
        };
        // The Dry wheel at 0: only the echoes.
        let dry = (0, 0);
        let low = Switches { in_filter: 1, ..ON };
        let mut plugin = with(&[low], &[dry, (at(0, IN_FILTER + 1), 30_000)]);
        let passed = tone(&mut plugin, 200.0);
        let mut plugin = with(&[low], &[dry, (at(0, IN_FILTER + 1), 30_000)]);
        let stopped = tone(&mut plugin, 12_000.0);
        assert!(passed > 0.5 && stopped < 0.05, "{passed} {stopped}");
        // The feedback filter: a high pass in the line takes a low tone
        // out of every echo, the first one processed too.
        let processed = Switches { first: false, ..ON };
        let mut bytes = saved(&[processed], &[dry]);
        bytes[SWITCHES_AT + 8..SWITCHES_AT + 12].copy_from_slice(&4i32.to_le_bytes());
        let mut plugin = DelayBank::default();
        plugin.set_sample_rate(48_000.0);
        plugin.set_state(&bytes).unwrap();
        assert_eq!(plugin.units[0].bands[1].kind, HIGH_PASS);
        assert_eq!(plugin.fidelity(), Fidelity::Exact);
        let level = tone(&mut plugin, 40.0);
        let mut plain = with(&[processed], &[dry]);
        let unfiltered = tone(&mut plain, 40.0);
        assert!(
            level < 0.1 * unfiltered && unfiltered > 0.1,
            "{level} {unfiltered}"
        );
        // A flush empties the filters.
        plugin.reset();
        assert!(plugin.units[0].bands[1].state == Filter::default().state);
    }

    #[test]
    fn a_state_of_another_version_length_or_range_is_turned_down() {
        let mut plugin = DelayBank::default();
        let mut times: Vec<(usize, i32)> = (1..UNITS).map(|u| (at(u, TIME), 144)).collect();
        times.push((at(0, TIME), 768));
        let good = saved(&[ON], &times);
        assert_eq!(plugin.set_state(&good), Ok(()));
        assert_eq!(plugin.param(at(0, TIME)), 768.0);
        // Version 15 counted the time in tens.
        let mut older = good.clone();
        older[0] = 15;
        assert_eq!(plugin.set_state(&older), Ok(()));
        assert_eq!(plugin.param(at(0, TIME)), 7680.0);
        older[0] = 14;
        assert_eq!(
            plugin.set_state(&older),
            Err(StateError::Version { found: 14 })
        );
        assert_eq!(
            plugin.set_state(&good[..700]),
            Err(StateError::Length {
                expected: 792,
                found: 700
            })
        );
        let wrong = saved(&[ON], &[(at(2, OFFSET), 513)]);
        assert_eq!(
            plugin.set_state(&wrong),
            Err(StateError::Range {
                param: at(2, OFFSET),
                value: 513
            })
        );
        // The oversampling takes as many stages as the half band has.
        for value in [-1, 5] {
            let wrong = saved(&[ON], &[(OVERSAMPLING, value)]);
            assert_eq!(
                plugin.set_state(&wrong),
                Err(StateError::Range {
                    param: OVERSAMPLING,
                    value
                })
            );
        }
        // A wheel moves inside its range, and the last unit's "to next"
        // is no parameter.
        plugin.set_param(at(0, TIME), 9000.0);
        assert_eq!(plugin.param(at(0, TIME)), 7680.0);
        plugin.set_param(131, 5.0);
        assert_eq!(plugin.params[131], 0);
    }

    #[test]
    fn fl_s_presets_load_and_run() {
        let Some(presets) = presets(NAME) else {
            return;
        };
        assert_eq!(presets.len(), 21);
        let mut exact = 0;
        for preset in &presets {
            let mut plugin = DelayBank::default();
            plugin.set_sample_rate(48_000.0);
            plugin.set_tempo(140.0);
            plugin.set_state(&preset.state).unwrap();
            exact += usize::from(plugin.fidelity() == Fidelity::Exact);
            let [left, right] = click(&mut plugin, [0.5, -0.5], 48_000);
            let sound = left.iter().chain(&right);
            assert!(
                sound.clone().all(|(_, v)| v.is_finite() && v.abs() < 8.0),
                "{}",
                preset.file
            );
        }
        assert!(exact > 0 && exact < presets.len(), "{exact}");
        let default = presets.iter().find(|p| p.file == "Default").unwrap();
        let mut plugin = DelayBank::default();
        plugin.set_state(&default.state).unwrap();
        // But for the last unit's "to next", which is no parameter and
        // is -1 in the file.
        assert_eq!(
            plugin.params[..PARAMS],
            DelayBank::default().params[..PARAMS]
        );
        assert_eq!(plugin.params[PARAMS], -1);
        assert!(plugin.units[0].enabled && plugin.units[0].beats && !plugin.units[0].first);
    }
}
