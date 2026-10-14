//! EQUO: a drawn curve over 31 band passes a third of an octave apart,
//! each with its own level and pan, as `docs/plugins/equo.md` has it.
//!
//! The state is 4139 bytes in version 3 and 4143 in version 4: the version
//! as a little-endian i32, then each of the 31 bands' three targets
//! (volume, pan, send) as an f32 value and an i32 mode, the nine banks of
//! curves as f32 `[bank][band][target]`, the five knobs as i32, the
//! bandwidth, oversampling, send insert, shown target and shown bank as
//! i32, three switches as bytes, and in version 4 one more i32 for the
//! editor. FL leaves a state older than version 3 alone, and so does
//! this.
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Band shift | -12000 to 12000 | 0 |
//! | 1 Band mix | -128 to 128 | 128 |
//! | 2 Morph | 0 to 7000 | 0 |
//! | 3 Smooth | 0 to 1024 | 170 |
//! | 4 Volume | 0 to 256 | 128 |
//! | 5 to 841 a bank's band | 0 to 2^30 | 2^29 |
//!
//! A curve value runs from 0 to 2, 1 being flat. For a level it is the gain
//! `(3^v - 1) / 2`, which is 1 at 1 and 4 at 2; for a pan it is `v - 1`.
//! The level and pan of a band come from banks 1 to 8: the morph knob over
//! 1000 picks two banks next to each other and goes between them on the
//! curve's own scale, the shift knob slides the curve along the bands by
//! up to 30 of them, and the mix knob takes each value towards 1 (at 0)
//! or past it to its mirror (at -128). The ninth bank, the master curve,
//! then scales each level and adds to each pan.
//!
//! Band `i` is a cookbook band pass at `20 * 2^(i / 3)` Hz and unity in its
//! centre, the bandwidth wheel over 300 octaves wide, in double precision.
//! A band at or over half the rate is off, and one over 11050 Hz runs at
//! twice the rate, each frame held for two and each two averaged after.
//! Every band's output is added to the main bus at its level times
//! `10^(-3/20)` over the bandwidth wheel over 100, panned by FL's
//! balance law. In add mode the input is on the bus first and each band
//! adds its level less one. Every gain moves to its new value in a line
//! over a block, at most 0.001 a sample at 44.1 kHz, and the volume knob's
//! `(11^v - 1) / 10` comes last. With oversampling on, all of it runs
//! between the halves of FL's 2x allpass resampler.
//!
//! Once a song tick, each band's level and pan move towards their new
//! values by FL's one-pole smoother, which comes to within 1% in
//! `(11^s - 1) * 40000` samples for the smooth knob at `s`. After a fifth
//! of a second with no sample over 10^-6, the plugin passes its input on.
//!
//! A state with the bandwidth wheel outside 80 to 500 or oversampling other
//! than 0 or 1, which FL's controls cannot save, is turned down as
//! parameter 842 or 843 out of range.
//!
//! What is not FL's. [`Effect::fidelity`] says so for the first, where a
//! state asks for it: the send to another insert is not routed, so a state
//! with a send loses that output, and send only mode passes the input on.
//! A loaded state starts with every smoother and gain where it goes, where
//! FL's glides there from a flat curve, and the ticks are counted here
//! from the tempo and PPQ, at this plugin's own block boundaries.

use crate::{Effect, Fidelity, StateError, knob_value, state_version};
use dsp::biquad::Coefficients64;
use std::f64::consts::{LN_2, LN_10, TAU};

/// The plugin's event 201 name.
pub const NAME: &str = "EQUO";

const BANDS: usize = 31;
/// Volume, pan and send.
const TARGETS: usize = 3;
const VOLUME: usize = 0;
const PAN: usize = 1;
const SEND: usize = 2;
/// Banks 1 to 8 and the master curve.
const BANKS: usize = 9;
const MASTER: usize = 8;
const KNOBS: usize = 5;
const PARAMS: usize = KNOBS + BANKS * BANDS * TARGETS;

const RANGES: [(i32, i32); KNOBS] = [
    (-12_000, 12_000),
    (-128, 128),
    (0, 7000),
    (0, 1024),
    (0, 256),
];
const DEFAULTS: [i32; KNOBS] = [0, 128, 0, 170, 128];
/// A band parameter is its value in 2^29ths.
const BAND_SCALE: f64 = 536_870_912.0;
const BAND_RANGE: (i32, i32) = (0, 1 << 30);

/// The bandwidth wheel, which no parameter moves.
const BANDWIDTH: (i32, i32) = (80, 500);
const DEFAULT_BANDWIDTH: i32 = 150;

/// How long a state of each version is.
const LEN_3: usize = 4139;
const LEN_4: usize = 4143;

/// A target's modes. 0 is on.
const OFF: i32 = 1;
const LOCKED: i32 = 2;

/// A block whose samples are none of them over this is silence.
const SILENCE: f32 = 1e-6;
/// What FL's DSP units take as nothing: 2^-24.
const TINY: f32 = 5.960_464_5e-8;

/// How many frames go through at a time.
const CHUNK: usize = 256;

/// The two paths of FL's 2x resampler, from the DLL: three first-order
/// allpasses each, at the lower rate.
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

/// A curve value as the gain or the pan it is: FL's `FUN_006ccb70`.
fn to_gain(target: usize, value: f32) -> f32 {
    if target == PAN {
        value - 1.0
    } else {
        let grown = (f64::from(value) * 3f64.ln()).exp();
        (f64::from((grown - 1.0) as f32) * 0.5) as f32
    }
}

/// A gain or a pan as the curve value it comes from: `FUN_006ccc00`.
fn to_curve(target: usize, gain: f32) -> f32 {
    if target == PAN {
        gain + 1.0
    } else {
        let x = (f64::from(gain) / 0.5) as f32 + 1.0;
        (f64::from(x).ln() / 3f64.ln()) as f32
    }
}

/// The volume knob's gain.
fn level(knob: i32) -> f32 {
    let x = knob as f32 * (1.0 / 128.0);
    if x == 1.0 {
        1.0
    } else {
        let grown = f64::from(x * (11f64.ln() as f32)).exp();
        ((grown - 1.0) * f64::from(0.1f32)) as f32
    }
}

/// The smoother's pole for the smooth knob at `knob` and a tick of `tick`
/// samples. None where it does not smooth.
fn smoothing(knob: i32, tick: f32) -> Option<f64> {
    let x = knob.unsigned_abs() as f32 * (1.0 / 1024.0);
    let grown = (f64::from(x) * 11f64.ln()).exp();
    let time = f64::from((f64::from((grown - 1.0) as f32) * 40_000.0 / f64::from(tick)) as f32);
    (time > 0.0).then(|| (0.01f64.ln() / time.max(1.0)).exp())
}

/// FL's balance law: the side the pan points away from comes down.
fn pan_law(pan: f32, level: f32) -> [f32; 2] {
    let level64 = f64::from(level);
    if pan.is_sign_negative() {
        [level, (level64 * (f64::from(pan) + 1.0)) as f32]
    } else {
        [(level64 * (1.0 - f64::from(pan))) as f32, level]
    }
}

/// FL's line to `target` over `frames`: where it starts and its step a
/// frame, no steeper than `limit`. `now` is where it ends.
fn ramp(now: &mut f32, target: f32, limit: f32, frames: usize) -> (f32, f32) {
    let start = *now;
    let mut step = target - start;
    if step != 0.0 {
        *now = target;
        let n = frames as f32;
        step /= n;
        if limit < step.abs() {
            step = limit.copysign(step);
            *now = start + step * n;
            if now.abs() < TINY {
                *now = 0.0;
                step = -start / n;
            }
        }
    }
    (start, step)
}

/// Add `x` to `out`, both stereo frames side by side, at a gain a side
/// that starts at `start` and moves by `step` a frame. The gains are
/// summed as FL's SSE loop sums them: the even frames and the odd ones
/// each in their own lane, two steps at a time, eight frames to a round,
/// and the frames past the last round one step at a time.
fn add_ramp(x: &[f32], out: &mut [f32], start: [f32; 2], step: [f32; 2]) {
    let frames = x.len() / 2;
    let mut even = [start[0] + 0.0, start[1] + 0.0];
    let mut odd = [start[0] + step[0], start[1] + step[1]];
    let twice = [step[0] + step[0], step[1] + step[1]];
    let rounds = frames / 8;
    for pair in 0..rounds * 4 {
        let at = pair * 4;
        out[at] += x[at] * even[0];
        out[at + 1] += x[at + 1] * even[1];
        out[at + 2] += x[at + 2] * odd[0];
        out[at + 3] += x[at + 3] * odd[1];
        for side in 0..2 {
            even[side] += twice[side];
            odd[side] += twice[side];
        }
    }
    for frame in rounds * 8..frames {
        out[frame * 2] += x[frame * 2] * even[0];
        out[frame * 2 + 1] += x[frame * 2 + 1] * even[1];
        even[0] += step[0];
        even[1] += step[1];
    }
}

/// [`add_ramp`] for the output's gain: in place, the same on both sides.
fn scale_ramp(out: &mut [f32], start: f32, step: f32) {
    let frames = out.len() / 2;
    let mut even = start + 0.0;
    let mut odd = start + step;
    let twice = step + step;
    let rounds = frames / 8;
    for pair in 0..rounds * 4 {
        let at = pair * 4;
        out[at] *= even;
        out[at + 1] *= even;
        out[at + 2] *= odd;
        out[at + 3] *= odd;
        even += twice;
        odd += twice;
    }
    for frame in rounds * 8..frames {
        out[frame * 2] *= even;
        out[frame * 2 + 1] *= even;
        even += step;
    }
}

/// One first-order allpass of the resampler, a side each.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Allpass {
    before: [f64; 2],
    out: [f64; 2],
}

impl Allpass {
    fn step(&mut self, side: usize, x: f64, c: f64) -> f64 {
        let y = self.before[side] + (x - self.out[side]) * c;
        self.before[side] = x;
        self.out[side] = y;
        y
    }
}

/// One direction of FL's 2x resampler for both sides.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct HalfBand {
    paths: [[Allpass; 3]; 2],
}

impl HalfBand {
    fn path(&mut self, path: usize, side: usize, x: f64) -> f64 {
        let stages = self.paths[path].iter_mut().zip(PATHS[path]);
        stages.fold(x, |x, (stage, c)| stage.step(side, x, c))
    }

    /// Two frames in `wide` for each in `input`, stereo side by side.
    fn up(&mut self, input: &[f32], wide: &mut [f32]) {
        for (frame, out) in input
            .as_chunks::<2>()
            .0
            .iter()
            .zip(wide.as_chunks_mut::<4>().0)
        {
            for side in 0..2 {
                let x = f64::from(frame[side]);
                out[side + 2] = self.path(0, side, x) as f32;
                out[side] = self.path(1, side, x) as f32;
            }
        }
        self.flush();
    }

    /// One frame in `out` for each two in `wide`.
    fn down(&mut self, wide: &[f32], out: &mut [f32]) {
        for (pair, frame) in wide
            .as_chunks::<4>()
            .0
            .iter()
            .zip(out.as_chunks_mut::<2>().0)
        {
            for side in 0..2 {
                let first = self.path(0, side, f64::from(pair[side]));
                let second = self.path(1, side, f64::from(pair[side + 2]));
                frame[side] = ((second + first) * 0.5) as f32;
            }
        }
        self.flush();
    }

    /// FL zeroes an allpass's output under 2^-24 after every block.
    fn flush(&mut self) {
        for stage in self.paths.as_flattened_mut() {
            for out in &mut stage.out {
                if out.abs() < f64::from(TINY) {
                    *out = 0.0;
                }
            }
        }
    }
}

/// One of a band's targets.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Target {
    /// The value the state keeps, which a locked band plays.
    value: f32,
    mode: i32,
    /// Where the curves put it, and where the smoother has it.
    goal: f32,
    now: f32,
    smoothed: f64,
    /// The gains a side the last block ended at.
    gains: [f32; 2],
}

/// One band pass and what it plays at.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Band {
    /// Whether it runs, whether it is at or over half the rate, and
    /// whether it runs at twice the rate.
    on: bool,
    above: bool,
    wide: bool,
    filter: Coefficients64,
    /// `x[n-1]`, `x[n-2]`, `y[n-1]`, `y[n-2]` of each side.
    state: [[f64; 4]; 2],
    targets: [Target; TARGETS],
}

impl Band {
    /// One sample of one side through the band pass. FL's loop has no
    /// `b1`, which is 0.
    fn step(&mut self, side: usize, x: f32) -> f32 {
        let c = &self.filter;
        let s = &mut self.state[side];
        let x = f64::from(x);
        let y = x * c.b0 + ((s[1] * c.b2 - s[3] * c.a2) - s[2] * c.a1);
        *s = [x, s[0], y, s[2]];
        y as f32
    }

    /// Stereo frames side by side from `input` into `out`.
    fn filter(&mut self, input: &[f32], out: &mut [f32]) {
        for (x, y) in input
            .as_chunks::<2>()
            .0
            .iter()
            .zip(out.as_chunks_mut::<2>().0.iter_mut())
        {
            y[0] = self.step(0, x[0]);
            y[1] = self.step(1, x[1]);
        }
    }

    /// The same at twice the rate: each frame held for two, and each two
    /// out averaged.
    fn filter_wide(&mut self, input: &[f32], out: &mut [f32]) {
        for (x, y) in input
            .as_chunks::<2>()
            .0
            .iter()
            .zip(out.as_chunks_mut::<2>().0.iter_mut())
        {
            let first = [self.step(0, x[0]), self.step(1, x[1])];
            let second = [self.step(0, x[0]), self.step(1, x[1])];
            y[0] = (f64::from(first[0] + second[0]) * 0.5) as f32;
            y[1] = (f64::from(first[1] + second[1]) * 0.5) as f32;
        }
    }
}

/// EQUO. A new one has FL's defaults: every curve flat, which passes
/// everything at about 0 dB.
#[derive(Debug, Clone, PartialEq)]
pub struct Equo {
    knobs: [i32; KNOBS],
    banks: Box<[[[f32; TARGETS]; BANDS]; BANKS]>,
    bandwidth: i32,
    /// 1 runs everything at twice the rate.
    oversample: i32,
    /// The insert the sends go to, from 0; -1 for none.
    send: i32,
    /// The target and the bank the editor shows.
    shown_target: i32,
    shown_bank: i32,
    send_only: bool,
    add_mode: bool,
    master: bool,
    /// Version 4's word for the editor.
    editor: i32,
    bands: [Band; BANDS],
    /// The project's rate in Hz.
    rate: i32,
    tempo: f64,
    ppq: u16,
    /// What the knobs come to: the shift in bands, the mix's pull towards
    /// 1, and the morph in banks.
    shift: f32,
    mix: f32,
    morph: f32,
    /// What each band's level is scaled by.
    norm: f32,
    /// The output's gain, and where its line ended.
    gain: f32,
    gain_now: f32,
    smoothing: Option<f64>,
    /// How many frames to the next tick.
    until_tick: f64,
    /// How many frames of silence the plugin waits for, and how many are
    /// left: under minus a block, it passes its input on.
    tail: i32,
    quiet: i32,
    /// The next block's gains start where they go.
    reset: bool,
    up: HalfBand,
    down: HalfBand,
    input: Vec<f32>,
    wide: Vec<f32>,
    main: Vec<f32>,
    band: Vec<f32>,
    out: Vec<f32>,
}

impl Default for Equo {
    fn default() -> Self {
        let target = |goal: f32| Target {
            value: 1.0,
            mode: 0,
            goal,
            now: goal,
            smoothed: f64::from(goal),
            gains: [0.0; 2],
        };
        let band = Band {
            on: false,
            above: false,
            wide: false,
            filter: Coefficients64::IDENTITY,
            state: [[0.0; 4]; 2],
            targets: [target(1.0), target(0.0), target(1.0)],
        };
        let mut plugin = Equo {
            knobs: DEFAULTS,
            banks: Box::new([[[1.0; TARGETS]; BANDS]; BANKS]),
            bandwidth: DEFAULT_BANDWIDTH,
            oversample: 0,
            send: -1,
            shown_target: 0,
            shown_bank: 0,
            send_only: false,
            add_mode: false,
            master: false,
            editor: 0,
            bands: [band; BANDS],
            rate: 44_100,
            tempo: 140.0,
            ppq: 96,
            shift: 0.0,
            mix: 0.0,
            morph: 0.0,
            norm: 1.0,
            gain: 1.0,
            gain_now: 1.0,
            smoothing: None,
            until_tick: 0.0,
            tail: 0,
            quiet: 0,
            reset: true,
            up: HalfBand::default(),
            down: HalfBand::default(),
            input: vec![0.0; 2 * CHUNK],
            wide: vec![0.0; 4 * CHUNK],
            main: vec![0.0; 4 * CHUNK],
            band: vec![0.0; 4 * CHUNK],
            out: vec![0.0; 2 * CHUNK],
        };
        plugin.load();
        plugin
    }
}

impl Equo {
    /// 2 with oversampling on, else 1.
    fn os(&self) -> usize {
        1 << self.oversample
    }

    /// The rate the bands run at.
    fn inner_rate(&self) -> i32 {
        self.rate * self.os() as i32
    }

    /// The steepest a gain may move in a frame at the bands' rate:
    /// `TAudioRenderer`'s field 0x34.
    fn limit(&self) -> f32 {
        ((44_100.0 / f64::from(self.inner_rate())) * 0.001) as f32
    }

    fn samples_per_tick(&self) -> f32 {
        (f64::from(self.rate) * 60.0 / (self.tempo * f64::from(self.ppq))) as f32
    }

    /// FL's `FUN_006ce050`: the bands' centres and filters, from the
    /// bandwidth and the rate.
    fn design(&mut self) {
        let width = (f64::from(self.bandwidth) / 100.0) as f32;
        let octaves = width / 3.0;
        let down = (LN_10 * 0.05) as f32 * -3.0;
        self.norm = f64::from(down).exp() as f32 / width;
        let rate = self.inner_rate();
        let half = (f64::from(rate) / 2.0) as f32;
        let held = (f64::from(self.os() as i32 * 0x5654) / 2.0) as f32;
        let radians = 1.0 / f64::from(rate) * TAU;
        for (i, band) in self.bands.iter_mut().enumerate() {
            let mut centre = ((i as f64 / 3.0 * LN_2).exp() * 20.0) as f32;
            band.above = half <= centre;
            if band.above {
                continue;
            }
            band.wide = centre > held;
            if band.wide {
                centre *= 0.5;
            }
            let w = f64::from(centre) * radians;
            let (sin, cos) = w.sin_cos();
            let x = LN_2 / 2.0 * f64::from(octaves) * w / sin;
            let sinh = (x.exp() - (-x).exp()) / 2.0;
            band.filter = Coefficients64::band_pass(cos, sin * sinh);
        }
        self.activate();
    }

    /// `FUN_006cdf70`: which bands run.
    fn activate(&mut self) {
        let send = self.send;
        for band in &mut self.bands {
            let on = |target: usize| band.targets[target].mode != OFF;
            band.on = (on(VOLUME) || on(PAN) || (on(SEND) && send >= 0)) && !band.above;
        }
    }

    /// A curve value of `bank` at `band` with the mix knob's pull, as a
    /// gain or a pan.
    fn mixed(&self, bank: usize, band: usize, target: usize) -> f32 {
        let value = self.banks[bank][band.min(BANDS - 1)][target];
        to_gain(target, value + (1.0 - value) * self.mix)
    }

    /// `FUN_006cd980`: bank `bank` at the place `whole + part` along the
    /// bands.
    fn across(&self, bank: usize, whole: usize, part: f32, target: usize) -> f32 {
        let at = self.mixed(bank, whole, target);
        if part != 0.0 {
            at + (self.mixed(bank, whole + 1, target) - at) * part
        } else {
            at
        }
    }

    /// `FUN_006cda50`: where the curves put a band's target.
    fn goal(&self, band: usize, target: usize) -> f32 {
        let t = &self.bands[band].targets[target];
        match t.mode {
            OFF => to_gain(target, 1.0),
            LOCKED => to_gain(target, t.value),
            _ => {
                let place = band as f32 - self.shift;
                let place = if place > 0.0 { place } else { 0.0 };
                let whole = f64::from(place) as i32;
                let part = place - whole as f32;
                let bank = f64::from(self.morph) as i32;
                let between = self.morph - bank as f32;
                let (whole, bank) = (whole as usize, bank as usize);
                let mut value = self.across(bank, whole, part, target);
                if between != 0.0 {
                    let next = self.across(bank + 1, whole, part, target);
                    let from = to_curve(target, value);
                    let to = to_curve(target, next);
                    value = to_gain(target, from + (to - from) * between);
                }
                let master = to_gain(target, self.banks[MASTER][band][target]);
                if target == PAN {
                    let sum = value + master;
                    sum.clamp(-1.0, 1.0)
                } else {
                    value * master
                }
            }
        }
    }

    /// `FUN_006ce220`: the knobs, and every target's goal.
    fn aim(&mut self) {
        self.shift = (f64::from(self.knobs[0]) * 0.0025) as f32;
        self.mix = 1.0 - self.knobs[1] as f32 * (1.0 / 128.0);
        self.morph = (f64::from(self.knobs[2]) * 0.001) as f32;
        for band in 0..BANDS {
            for target in 0..TARGETS {
                self.bands[band].targets[target].goal = self.goal(band, target);
            }
        }
    }

    /// What a state or a rate asks to be worked out again: FL's restore,
    /// less what only the editor sees. The smoothers start where they go.
    fn load(&mut self) {
        self.tail = (f64::from(self.rate) / 5.0).round_ties_even() as i32;
        self.up = HalfBand::default();
        self.down = HalfBand::default();
        self.design();
        self.aim();
        self.gain = level(self.knobs[4]);
        self.smoothing = smoothing(self.knobs[3], self.samples_per_tick());
        for band in &mut self.bands {
            band.state = [[0.0; 4]; 2];
            for target in &mut band.targets {
                target.now = target.goal;
                target.smoothed = f64::from(target.goal);
            }
        }
        self.reset = true;
    }

    /// FL's `NewTick`: each target a step nearer its goal.
    fn tick_smoothers(&mut self) {
        for band in &mut self.bands {
            for target in &mut band.targets {
                target.now = match self.smoothing {
                    Some(pole) => {
                        let goal = f64::from(target.goal);
                        target.smoothed = goal + (target.smoothed - goal) * pole;
                        target.smoothed as f32
                    }
                    None => target.goal,
                };
            }
        }
    }

    /// One block of at most [`CHUNK`] frames: FL's `Eff_Render`.
    fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len();
        let silent = left.iter().chain(right.iter()).all(|v| v.abs() <= SILENCE);
        if silent {
            self.quiet = self.quiet.saturating_sub(n as i32);
        } else {
            self.quiet = self.tail;
        }
        if self.quiet <= -(n as i32) {
            self.reset = true;
            return;
        }
        for (frame, (l, r)) in self
            .input
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .zip(left.iter().zip(right.iter()))
        {
            frame[0] = *l;
            frame[1] = *r;
        }
        let os = self.os();
        let m = n * os;
        let limit = self.limit();
        let source: &[f32] = if os == 2 {
            self.up.up(&self.input[..2 * n], &mut self.wide[..2 * m]);
            &self.wide[..2 * m]
        } else {
            &self.input[..2 * n]
        };
        let main = &mut self.main[..2 * m];
        if self.add_mode {
            main.copy_from_slice(source);
        } else {
            main.fill(0.0);
        }
        let band_out = &mut self.band[..2 * m];
        for band in &mut self.bands {
            if !band.on {
                continue;
            }
            if band.wide {
                band.filter_wide(source, band_out);
            } else {
                band.filter(source, band_out);
            }
            let volume = band.targets[VOLUME];
            if self.send_only || volume.mode == OFF {
                continue;
            }
            let mut gains = pan_law(band.targets[PAN].now, volume.now * self.norm);
            if self.add_mode {
                gains = gains.map(|g| g - self.norm);
            }
            let target = &mut band.targets[VOLUME];
            if self.reset {
                target.gains = gains;
            }
            let (l0, ls) = ramp(&mut target.gains[0], gains[0], limit, m);
            let (r0, rs) = ramp(&mut target.gains[1], gains[1], limit, m);
            if ls == 0.0 && rs == 0.0 {
                if l0.to_bits() != 0 || r0.to_bits() != 0 {
                    for (y, x) in main
                        .as_chunks_mut::<2>()
                        .0
                        .iter_mut()
                        .zip(band_out.as_chunks::<2>().0.iter())
                    {
                        y[0] += x[0] * l0;
                        y[1] += x[1] * r0;
                    }
                }
            } else {
                add_ramp(band_out, main, [l0, r0], [ls, rs]);
            }
        }
        let out = &mut self.out[..2 * n];
        if self.send_only {
            out.copy_from_slice(&self.input[..2 * n]);
        } else if os == 2 {
            self.down.down(&self.main[..2 * m], out);
        } else {
            out.copy_from_slice(&self.main[..2 * n]);
        }
        if self.reset {
            self.gain_now = self.gain;
            self.reset = false;
        }
        let (start, step) = ramp(&mut self.gain_now, self.gain, limit / os as f32, n);
        if step == 0.0 {
            if start.to_bits() == 0 {
                out.fill(0.0);
            } else if start != 1.0 {
                for v in out.iter_mut() {
                    *v *= start;
                }
            }
        } else {
            scale_ramp(out, start, step);
        }
        for (frame, (l, r)) in out
            .as_chunks::<2>()
            .0
            .iter()
            .zip(left.iter_mut().zip(right.iter_mut()))
        {
            *l = frame[0];
            *r = frame[1];
        }
    }

    /// Read a state, or say why not.
    fn read(bytes: &[u8]) -> Result<Saved, StateError> {
        let version = state_version(bytes, LEN_4)?;
        if version < 3 {
            return Err(StateError::Version { found: version });
        }
        let expected = if version == 3 { LEN_3 } else { LEN_4 };
        if bytes.len() != expected {
            return Err(StateError::Length {
                expected,
                found: bytes.len(),
            });
        }
        let mut words = bytes[4..].as_chunks::<4>().0.iter().copied();
        let mut word = || words.next().unwrap_or_default();
        let mut items = [[(0.0f32, 0i32); TARGETS]; BANDS];
        for band in &mut items {
            for item in band {
                *item = (f32::from_le_bytes(word()), i32::from_le_bytes(word()));
            }
        }
        let mut banks = Box::new([[[0.0f32; TARGETS]; BANDS]; BANKS]);
        for value in banks.as_flattened_mut().as_flattened_mut() {
            *value = f32::from_le_bytes(word());
        }
        let mut knobs = [0; KNOBS];
        for (param, knob) in knobs.iter_mut().enumerate() {
            let value = i32::from_le_bytes(word());
            let (min, max) = RANGES[param];
            if value < min || value > max {
                return Err(StateError::Range { param, value });
            }
            *knob = value;
        }
        let [oversample, bandwidth, send, shown_target, shown_bank] =
            [(); 5].map(|_| i32::from_le_bytes(word()));
        if !(0..=1).contains(&oversample) {
            return Err(StateError::Range {
                param: PARAMS + 1,
                value: oversample,
            });
        }
        if bandwidth < BANDWIDTH.0 || bandwidth > BANDWIDTH.1 {
            return Err(StateError::Range {
                param: PARAMS,
                value: bandwidth,
            });
        }
        let at = 4 + 4 * (BANDS * TARGETS * 2 + BANKS * BANDS * TARGETS + KNOBS + 5);
        let switches = &bytes[at..at + 3];
        let editor = bytes
            .get(at + 3..at + 7)
            .map_or(0, |w| i32::from_le_bytes(w.try_into().unwrap()));
        Ok(Saved {
            items,
            banks,
            knobs,
            oversample,
            bandwidth,
            send,
            shown_target,
            shown_bank,
            switches: [switches[0] != 0, switches[1] != 0, switches[2] != 0],
            editor,
        })
    }

    /// The bank a band parameter of the editor's bank also sets the kept
    /// value for: the master curve with the master button down.
    fn edited_bank(&self) -> usize {
        if self.master {
            MASTER
        } else {
            self.shown_bank.clamp(0, MASTER as i32 - 1) as usize
        }
    }
}

/// What a state holds.
struct Saved {
    items: [[(f32, i32); TARGETS]; BANDS],
    banks: Box<[[[f32; TARGETS]; BANDS]; BANKS]>,
    knobs: [i32; KNOBS],
    oversample: i32,
    bandwidth: i32,
    send: i32,
    shown_target: i32,
    shown_bank: i32,
    /// Send only, add mode, master.
    switches: [bool; 3],
    editor: i32,
}

impl Effect for Equo {
    /// The resampler and the filters start empty and every gain at its
    /// goal: FL's would glide from what it last played.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let saved = Self::read(bytes)?;
        for (band, items) in self.bands.iter_mut().zip(saved.items) {
            for (target, (value, mode)) in band.targets.iter_mut().zip(items) {
                target.value = value;
                target.mode = mode;
            }
        }
        self.banks = saved.banks;
        self.knobs = saved.knobs;
        self.oversample = saved.oversample;
        self.bandwidth = saved.bandwidth;
        self.send = saved.send;
        self.shown_target = saved.shown_target;
        self.shown_bank = saved.shown_bank;
        [self.send_only, self.add_mode, self.master] = saved.switches;
        self.editor = saved.editor;
        self.load();
        Ok(())
    }

    /// FL counts the rate in whole Hz. A rate that is not a number of 1 Hz
    /// or more is not taken.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz >= 1.0 {
            self.rate = hz.round() as i32;
            self.load();
        }
    }

    /// The tick's length sets how fast the smoother goes. This allocates
    /// nothing.
    fn set_tempo(&mut self, bpm: f64) {
        if bpm.is_finite() && bpm > 0.0 {
            self.tempo = bpm;
            self.smoothing = smoothing(self.knobs[3], self.samples_per_tick());
        }
    }

    fn set_ppq(&mut self, ppq: u16) {
        if ppq > 0 {
            self.ppq = ppq;
            self.smoothing = smoothing(self.knobs[3], self.samples_per_tick());
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        let mut done = 0;
        while done < len {
            if self.until_tick <= 0.0 {
                self.tick_smoothers();
                self.until_tick += f64::from(self.samples_per_tick()).max(1.0);
            }
            let next = self.until_tick.ceil() as usize;
            let n = CHUNK.min(len - done).min(next.max(1));
            self.render(&mut left[done..done + n], &mut right[done..done + n]);
            self.until_tick -= n as f64;
            done += n;
        }
    }

    fn param_count(&self) -> usize {
        PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        if index < KNOBS {
            self.knobs[index] as f32
        } else if index < PARAMS {
            let (bank, target, band) = place(index);
            (f64::from(self.banks[bank][band][target]) * BAND_SCALE).round_ties_even() as f32
        } else {
            0.0
        }
    }

    /// A band parameter moves a band of a bank whose kept target is on:
    /// FL's own does nothing for one that is off or locked.
    fn set_param(&mut self, index: usize, value: f32) {
        if index < KNOBS {
            self.knobs[index] = knob_value(value, RANGES[index]);
            match index {
                3 => self.smoothing = smoothing(self.knobs[3], self.samples_per_tick()),
                4 => self.gain = level(self.knobs[4]),
                _ => self.aim(),
            }
        } else if index < PARAMS {
            let (bank, target, band) = place(index);
            let edited = bank == self.edited_bank();
            let kept = &mut self.bands[band].targets[target];
            if kept.mode == 0 {
                let value = (f64::from(knob_value(value, BAND_RANGE)) / BAND_SCALE) as f32;
                if edited {
                    kept.value = value;
                }
                self.banks[bank][band][target] = value;
            }
            self.aim();
        }
    }

    /// FL's flush: the filters emptied, each gain at its goal at once.
    fn reset(&mut self) {
        for band in &mut self.bands {
            band.state = [[0.0; 4]; 2];
            for target in &mut band.targets {
                target.now = target.goal;
            }
        }
        self.reset = true;
        self.until_tick = 0.0;
    }

    fn fidelity(&self) -> Fidelity {
        if self.send >= 0 {
            Fidelity::Approximate("EQUO's send to another insert is not routed")
        } else {
            Fidelity::Exact
        }
    }
}

/// The bank, target and band of a band parameter: banks of 93, each three
/// runs of 31 bands.
fn place(index: usize) -> (usize, usize, usize) {
    let at = index - KNOBS;
    (
        at / (BANDS * TARGETS),
        at % (BANDS * TARGETS) / BANDS,
        at % BANDS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    /// A state with every curve at `curve(band)` for the level, flat pan,
    /// and these knobs.
    fn state(curve: impl Fn(usize) -> f32, knobs: [i32; 5], bandwidth: i32) -> Vec<u8> {
        let mut bytes = 4i32.to_le_bytes().to_vec();
        for band in 0..BANDS {
            for target in 0..TARGETS {
                let value = if target == VOLUME { curve(band) } else { 1.0 };
                bytes.extend(value.to_le_bytes());
                bytes.extend(0i32.to_le_bytes());
            }
        }
        for bank in 0..BANKS {
            for band in 0..BANDS {
                for target in 0..TARGETS {
                    let flat = bank == MASTER || target != VOLUME;
                    let value = if flat { 1.0 } else { curve(band) };
                    bytes.extend(value.to_le_bytes());
                }
            }
        }
        for knob in knobs {
            bytes.extend(knob.to_le_bytes());
        }
        for word in [0, bandwidth, -1, 0, 0] {
            bytes.extend(word.to_le_bytes());
        }
        bytes.extend([0, 0, 0]);
        bytes.extend(1i32.to_le_bytes());
        assert_eq!(bytes.len(), LEN_4);
        bytes
    }

    /// The level in dB a tone at `hz` comes out at, once it has settled.
    fn gain_db(effect: &mut Equo, hz: f64) -> f64 {
        let turns = hz / 48_000.0;
        let tone = testing::sine(turns, 48_000);
        let (mut left, mut right) = (tone.clone(), tone);
        effect.process(&mut left, &mut right);
        // Half a second in, a whole number of the tone's periods.
        let periods = (24_000.0 * turns).floor();
        let tail = &left[24_000..24_000 + (periods / turns).round() as usize];
        20.0 * testing::partial(tail, turns).log10()
    }

    fn equo(bytes: &[u8]) -> Equo {
        let mut effect = Equo::default();
        effect.set_sample_rate(48_000.0);
        effect.set_state(bytes).unwrap();
        effect
    }

    #[test]
    fn the_curve_scales_run_both_ways() {
        assert_eq!(to_gain(VOLUME, 1.0), 1.0);
        assert!((to_gain(VOLUME, 2.0) - 4.0).abs() < 1e-6);
        assert_eq!(to_gain(VOLUME, 0.0), 0.0);
        assert_eq!(to_gain(PAN, 0.25), -0.75);
        for v in [0.0, 0.3, 1.0, 1.7, 2.0] {
            assert!(
                (to_curve(VOLUME, to_gain(VOLUME, v)) - v).abs() < 1e-6,
                "{v}"
            );
        }
        assert_eq!(level(128), 1.0);
        assert_eq!(level(0), 0.0);
        assert!((level(256) - 12.0).abs() < 1e-5);
    }

    #[test]
    fn the_bands_are_a_third_of_an_octave_apart_from_20_hz() {
        let effect = equo(&state(|_| 1.0, DEFAULTS, 150));
        let centre = |i: usize| (i as f64 / 3.0 * LN_2).exp() * 20.0;
        assert!((centre(30) - 20_480.0).abs() < 1e-6);
        // At 48 kHz every band is under half the rate, and the top three
        // run at twice it.
        let wide: Vec<bool> = effect.bands.iter().map(|b| b.wide).collect();
        assert_eq!(wide.iter().filter(|w| **w).count(), 3);
        assert!(wide[28] && !wide[27]);
        assert!(effect.bands.iter().all(|b| b.on && !b.above));
        // 10^(-3/20) / 1.5.
        assert!((effect.norm - 0.471_96).abs() < 1e-5);
    }

    #[test]
    fn a_flat_curve_passes_the_middle_within_half_a_db() {
        let mut effect = equo(&state(|_| 1.0, DEFAULTS, 150));
        for hz in [100.0, 315.0, 1000.0, 1250.0] {
            let db = gain_db(&mut effect, hz);
            assert!(db.abs() < 0.5, "{hz} Hz at {db} dB");
        }
    }

    #[test]
    fn a_band_at_zero_cuts_its_centre() {
        // Band 17 is at 1016 Hz: with it and its neighbours at 0, a
        // kilohertz goes; with them at 2, it comes up by about 12 dB.
        let cut = |band: usize| if (16..=18).contains(&band) { 0.0 } else { 1.0 };
        let mut effect = equo(&state(cut, DEFAULTS, 150));
        assert!(gain_db(&mut effect, 1016.0) < -12.0);
        assert!(gain_db(&mut effect, 100.0).abs() < 0.5);
        let boost = |band: usize| if (16..=18).contains(&band) { 2.0 } else { 1.0 };
        let mut effect = equo(&state(boost, DEFAULTS, 150));
        let db = gain_db(&mut effect, 1016.0);
        assert!(db > 9.0 && db < 12.5, "{db}");
    }

    #[test]
    fn the_mix_knob_flattens_and_mirrors_the_curve() {
        let cut = |band: usize| if (16..=18).contains(&band) { 0.0 } else { 1.0 };
        let mut flat = equo(&state(cut, [0, 0, 0, 170, 128], 150));
        assert!(gain_db(&mut flat, 1016.0).abs() < 0.5);
        let mut mirrored = equo(&state(cut, [0, -128, 0, 170, 128], 150));
        assert!(gain_db(&mut mirrored, 1016.0) > 9.0);
    }

    #[test]
    fn the_shift_moves_the_curve_up() {
        // 400 is one band: the cut at bands 16 to 18 moves to 17 to 19.
        let cut = |band: usize| if (16..=18).contains(&band) { 0.0 } else { 1.0 };
        let effect = equo(&state(cut, [400, 128, 0, 170, 128], 150));
        let level = |band: usize| effect.bands[band].targets[VOLUME].goal;
        assert_eq!(
            [level(16), level(17), level(19), level(20)],
            [1.0, 0.0, 0.0, 1.0]
        );
        let effect = equo(&state(cut, [200, 128, 0, 170, 128], 150));
        let half = effect.bands[16].targets[VOLUME].goal;
        assert!((half - 0.5).abs() < 1e-6, "{half}");
    }

    #[test]
    fn the_morph_goes_between_banks_on_the_curve_s_scale() {
        let mut bytes = state(|_| 1.0, [0, 128, 500, 170, 128], 150);
        // Bank 2's level at band 0 is 2: halfway on the curve is 1.5.
        let at = 4 + BANDS * TARGETS * 8 + (BANDS * TARGETS) * 4;
        bytes[at..at + 4].copy_from_slice(&2.0f32.to_le_bytes());
        let effect = equo(&bytes);
        let goal = effect.bands[0].targets[VOLUME].goal;
        assert!((goal - to_gain(VOLUME, 1.5)).abs() < 1e-6, "{goal}");
        assert_eq!(effect.bands[1].targets[VOLUME].goal, 1.0);
    }

    #[test]
    fn the_balance_law_turns_down_the_far_side() {
        assert_eq!(pan_law(0.0, 0.5), [0.5, 0.5]);
        assert_eq!(pan_law(0.5, 1.0), [0.5, 1.0]);
        assert_eq!(pan_law(-1.0, 1.0), [1.0, 0.0]);
    }

    #[test]
    fn a_line_is_no_steeper_than_its_limit() {
        let mut now = 0.0;
        assert_eq!(ramp(&mut now, 1.0, 0.001, 100), (0.0, 0.001));
        assert!((now - 0.1).abs() < 1e-6);
        let mut now = 0.5;
        assert_eq!(ramp(&mut now, 0.5, 0.001, 100), (0.5, 0.0));
        // The lanes add up to the same line.
        let x = vec![1.0; 2 * 19];
        let mut out = vec![0.0; 2 * 19];
        add_ramp(&x, &mut out, [0.0, 1.0], [0.25, -0.25]);
        for (frame, pair) in out.as_chunks::<2>().0.iter().enumerate() {
            assert_eq!(*pair, [0.25 * frame as f32, 1.0 - 0.25 * frame as f32]);
        }
    }

    #[test]
    fn silence_is_passed_on_after_a_fifth_of_a_second() {
        // Every band at 0 passes nothing, until the plugin sleeps. It
        // starts asleep, and a sound wakes it.
        let mut effect = equo(&state(|_| 0.0, DEFAULTS, 150));
        let whisper = |len: usize| vec![1e-6f32; len];
        let (mut left, mut right) = (whisper(100), whisper(100));
        effect.process(&mut left, &mut right);
        assert_eq!(left, whisper(100));
        let mut tone = testing::sine(0.01, 4800);
        effect.process(&mut tone.clone(), &mut tone);
        assert!(tone.iter().all(|v| *v == 0.0));
        let (mut left, mut right) = (whisper(12_000), whisper(12_000));
        effect.process(&mut left, &mut right);
        assert!(left[..9600].iter().all(|v| *v == 0.0));
        assert_eq!(left[11_000..], whisper(1000)[..]);
    }

    #[test]
    fn oversampling_keeps_the_middle_flat() {
        let mut bytes = state(|_| 1.0, DEFAULTS, 150);
        let at = 4 + 4 * (BANDS * TARGETS * 2 + BANKS * BANDS * TARGETS + KNOBS);
        bytes[at..at + 4].copy_from_slice(&1i32.to_le_bytes());
        let mut effect = equo(&bytes);
        assert_eq!(effect.os(), 2);
        assert!(gain_db(&mut effect, 1000.0).abs() < 0.5);
    }

    #[test]
    fn a_state_is_read_or_turned_down() {
        let bytes = state(|_| 1.0, DEFAULTS, 150);
        assert!(Equo::default().set_state(&bytes).is_ok());
        assert_eq!(
            Equo::default().set_state(&bytes[..LEN_3]),
            Err(StateError::Length {
                expected: LEN_4,
                found: LEN_3
            })
        );
        let mut three = bytes[..LEN_3].to_vec();
        three[0] = 3;
        assert!(Equo::default().set_state(&three).is_ok());
        three[0] = 2;
        assert_eq!(
            Equo::default().set_state(&three),
            Err(StateError::Version { found: 2 })
        );
        let mut loud = bytes.clone();
        let at = 4 + 4 * (BANDS * TARGETS * 2 + BANKS * BANDS * TARGETS + 4);
        loud[at..at + 4].copy_from_slice(&257i32.to_le_bytes());
        assert_eq!(
            Equo::default().set_state(&loud),
            Err(StateError::Range {
                param: 4,
                value: 257
            })
        );
    }

    #[test]
    fn a_band_parameter_moves_its_bank() {
        let mut effect = Equo::default();
        assert_eq!(effect.param_count(), 842);
        assert_eq!(effect.param(5), BAND_SCALE as f32);
        // Bank 1, the pan of band 2.
        let index = KNOBS + BANDS + 2;
        effect.set_param(index, 0.0);
        assert_eq!(effect.banks[0][2][PAN], 0.0);
        assert_eq!(effect.bands[2].targets[PAN].goal, -1.0);
        assert_eq!(place(PARAMS - 1), (MASTER, SEND, BANDS - 1));
    }

    #[test]
    fn every_state_in_fl_s_files_loads_and_plays() {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, EQUO's states skipped");
            return;
        };
        let projects = testing::projects(&root).into_iter();
        let presets = testing::every_preset(&root, NAME).into_iter();
        let mut run = 0;
        for saved in projects.chain(presets) {
            if !saved.plugin.eq_ignore_ascii_case(NAME) {
                continue;
            }
            let mut effect = Equo::default();
            effect.set_sample_rate(44_100.0);
            effect.set_state(&saved.state).unwrap();
            let mut left = testing::sine(0.01, 8192);
            let mut right = testing::sine(0.037, 8192);
            for (l, r) in left.chunks_mut(300).zip(right.chunks_mut(300)) {
                effect.process(l, r);
            }
            let sound = left.iter().chain(&right);
            assert!(
                sound.clone().all(|v| v.is_finite() && v.abs() < 16.0),
                "{}",
                saved.file
            );
            assert_eq!(effect.fidelity(), Fidelity::Exact, "{}", saved.file);
            run += 1;
        }
        // 10 in the projects, 10 presets.
        assert_eq!(run, 20);
    }

    #[test]
    fn the_render_test_s_curve_lifts_1_khz_and_cuts_5_khz() {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, EQUO's render test skipped");
            return;
        };
        let saved = testing::projects(&root.join("System/Render tests"))
            .into_iter()
            .find(|s| s.plugin == NAME)
            .unwrap();
        // The 3x Osc's insert holds the preset "Random bank": eight random
        // curves, the first of them playing, every knob at its default.
        let mut effect = equo(&saved.state);
        assert_eq!(effect.knobs, DEFAULTS);
        let mut flat = equo(&state(|_| 1.0, DEFAULTS, 150));
        let mut lift = |hz: f64| gain_db(&mut effect, hz) - gain_db(&mut flat, hz);
        // The curve is +10 dB at band 17 and -7 dB at band 24. The bands
        // overlap, so what plays is the curve blurred: +8.6 and -2.3 dB.
        assert!((lift(1016.0) - 8.6).abs() < 0.3);
        assert!((lift(5120.0) + 2.3).abs() < 0.3);
    }
}
