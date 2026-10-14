//! Fruity Reeverb 2: a plate reverb after Dattorro with early reflections,
//! as `docs/plugins/fruity-reeverb-2.md` has it.
//!
//! The state comes in three forms. The newest is 66 bytes: a little-endian
//! i32 10001, the plugin's 15 parameters as i32, a byte for a predelay that
//! keeps time with the tempo and a byte for a mid/side input. The one
//! before is 58 bytes: 10000, the first 13 parameters and the two bytes.
//! The oldest, which FL's presets have, is 53 bytes with no version: the 13
//! parameters and a byte that FL does not use. The two older forms leave
//! the modulation at speed 684 and depth 0.
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Low cut, Hz; 19 is off | 19 to 3000 | 75 |
//! | 1 High cut, 100 Hz; 221 is off | 5 to 221 | 40 |
//! | 2 Predelay, 1/384 s or 1/192 beat | 0 to 384 | 0 |
//! | 3 Room size | 1 to 100 | 50 |
//! | 4 Diffusion, % | 0 to 100 | 100 |
//! | 5 Decay time, 0.1 s | 1 to 200 | 15 |
//! | 6 High damping, 100 Hz; 221 is off | 5 to 221 | 40 |
//! | 7 Bass multiplier, 0.01 | 20 to 300 | 100 |
//! | 8 Crossover, Hz | 25 to 2000 | 500 |
//! | 9 Stereo separation | -64 (apart) to 64 (merged) | 0 |
//! | 10 Dry level | 0 to 160 | 128 |
//! | 11 Early reflection level | 0 to 160 | 64 |
//! | 12 Wet level | 0 to 160 | 64 |
//! | 13 Modulation speed | 0 to 2048 | 684 |
//! | 14 Modulation depth | 0 to 2048 | 0 |
//!
//! The two sides of the input are added up, or taken off each other for a
//! mid/side input. That one signal goes through the high cut, the low cut
//! and four allpass diffusers. What the first diffuser gives also feeds a
//! line the early reflections are tapped off, eight a side. What the
//! fourth gives goes through the predelay, at least one sample long, into
//! the tank: two halves in a figure of eight, each a shelf, a damping low
//! pass, a delay an LFO moves, an allpass, a delay, an allpass and a delay.
//! Seven taps a side into the tank's lines are the wet sound. The output is
//! the input at the dry level, plus the reflections and the wet sound each
//! through a 2x2 mix that the separation sets.
//!
//! The twelve lines of the diffusers and the tank are as long as the doc's
//! table times `rate * (room + 15) / 115 / 20000`, so they grow with the
//! rate and the room.

use crate::fl_fx::{DOWN_60_DB, LINES, Line, TAP_BLEND, TAPS_LEFT, TAPS_RIGHT};
use crate::parts::Matrix;
use crate::{Effect, StateError, knob_gain, knob_value, read_state};
use std::f64::consts::PI;

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Reeverb 2";

/// The range of each parameter's wheel.
const RANGES: [(i32, i32); 15] = [
    (19, 3000),
    (5, 221),
    (0, 384),
    (1, 100),
    (0, 100),
    (1, 200),
    (5, 221),
    (20, 300),
    (25, 2000),
    (-64, 64),
    (0, 160),
    (0, 160),
    (0, 160),
    (0, 2048),
    (0, 2048),
];
const DEFAULTS: [i32; 15] = [75, 40, 0, 50, 100, 15, 40, 100, 500, 0, 128, 64, 64, 684, 0];

/// How long each form of the state is.
const OLD_LEN: usize = 53;
const V10000_LEN: usize = 58;
const V10001_LEN: usize = 66;

/// The modulation speed the two older forms get.
const OLD_SPEED: i32 = 684;

/// The low cut and the two high cuts at these are off.
const LOW_CUT_OFF: i32 = 19;
const HIGH_CUT_OFF: i32 = 221;

/// Where the eight early reflections a side are, before the room size
/// scales them, and which of them come upside down. The eighth of the left
/// is not played.
const EARLY_LEFT: [f64; 8] = [
    1102.0, 2904.0, 3784.0, 4873.0, 5878.0, 7470.0, 9062.0, 9263.0,
];
const EARLY_RIGHT: [f64; 8] = [
    837.0, 1423.0, 2865.0, 3938.0, 5278.0, 5978.0, 7532.0, 9263.0,
];
const EARLY_LEFT_SIGNS: [f32; 8] = [1.0, 1.0, -1.0, 1.0, 1.0, 1.0, 1.0, 0.0];
const EARLY_RIGHT_SIGNS: [f32; 8] = [1.0, -1.0, 1.0, 1.0, -1.0, 1.0, 1.0, -1.0];

/// The deepest the modulation moves its delay, in seconds.
const DEPTH: f64 = 200.0 / 44_100.0;

/// What a level wheel comes to with `x` the wheel over its unity.
fn level(x: f64) -> f32 {
    if x == 1.0 { 1.0 } else { knob_gain(x) as f32 }
}

/// The mix of a stereo pair at a level, for a separation of -1 to 1, as
/// `[LL, RR, RL, LR]`.
///
/// Above 0 each side gets up to half of the other, which ends in mono.
/// Under 0 it gets the other upside down, which widens. A mid/side input
/// turns the right output over.
fn spread(level: f32, separation: f64, mid_side: bool) -> [f32; 4] {
    let mut mix = [level, level, 0.0, 0.0];
    if separation != 0.0 {
        let amount = if separation > 0.0 {
            (5f64.powf(separation) - 1.0) * 0.25
        } else {
            separation
        };
        let crossed = (amount.min(1.0) * 0.5) as f32;
        mix = [
            level * (1.0 - crossed),
            level * (1.0 - crossed),
            level * crossed,
            level * crossed,
        ];
    }
    if mid_side {
        mix[1] = -mix[1];
        mix[3] = -mix[3];
    }
    mix
}

/// What a one-pole low pass takes of its way to its input, for a corner in
/// units of 100 Hz. 1 for a corner that is off.
fn low_pass(corner: i32, rate: f64) -> f32 {
    if corner == HIGH_CUT_OFF {
        return 1.0;
    }
    let corner = f64::from(corner).min(rate / 2.0 / 100.0);
    let c = 2.0 - (2.0 * PI * 100.0 * corner / rate).cos();
    (1.0 - (c - (c * c - 1.0).sqrt())) as f32
}

/// What the parameters, the two switches, the rate and the tempo come to.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Tune {
    /// The low cut's allpass coefficient, 0 for off, and what the high cut
    /// and the damping take of their way.
    low_cut: f32,
    high_cut: f32,
    damping: f32,
    /// How long the twelve lines are in frames, and how far back the taps
    /// are.
    lens: [usize; 12],
    taps_left: [usize; 7],
    taps_right: [usize; 7],
    /// The allpass gains: the four diffusers', and the tank's first and
    /// second.
    diffuser: f32,
    tank_first: f32,
    tank_second: f32,
    /// What each of the tank's four delays keeps, the shelf's amount and
    /// its allpass coefficient.
    feedback: f32,
    bass: f32,
    crossover: f32,
    /// What the taps 2 to 6 of each side weigh.
    weights: [f32; 5],
    /// How far back each early reflection is and how loud it is.
    early_left: [(usize, f32); 8],
    early_right: [(usize, f32); 8],
    /// The predelay in frames.
    predelay: usize,
    /// How far the LFO turns every frame, the depth it goes to in frames
    /// and what the depth keeps of its way there.
    lfo_step: f32,
    depth: f32,
    depth_keep: f32,
}

impl Tune {
    fn new(params: &[i32; 15], tempo_based: bool, rate: f64, tempo: f64) -> Self {
        let p = |index: usize| f64::from(params[index]);

        let low_cut = if params[0] == LOW_CUT_OFF {
            0.0
        } else {
            let t = (PI * p(0).min(rate / 2.0) / rate).tan();
            ((t - 1.0) / (t + 1.0)) as f32
        };

        // FL works the lengths out in whole numbers.
        let scale = (rate * (p(3) + 15.0) / 115.0).round() as u64;
        let scaled = |units: u64| (units * scale / 20_000) as usize;
        let lens = LINES.map(|units| scaled(units).max(1));
        let seconds = |lines: [usize; 2]| (lens[lines[0]] + lens[lines[1]]) as f64 / (2.0 * rate);

        let diffusion = p(4) * 0.01;
        let allpass = |lines: [usize; 2], spread: f64| {
            if diffusion == 0.0 {
                0.0
            } else {
                (DOWN_60_DB * seconds(lines) / (diffusion * spread)).exp() as f32
            }
        };

        let around = (lens[5] + lens[7] + lens[9] + lens[11]) as f64;
        let decay = p(5) * rate * 0.1;
        let feedback = (DOWN_60_DB * around / decay / 4.0).exp();
        let bass_feedback = (DOWN_60_DB * around / (decay * p(7) * 0.01) / 4.0).exp();
        let ratio = bass_feedback / feedback;
        let t = (PI * p(8) / rate).tan();
        let shelf = ratio.min(1.0);

        let reach = rate * (p(3) + 3.0) * 3.302e-7;
        let early = |k: usize, side: &[f64; 8], gain: f64| {
            let len = |units: f64| (units * reach).round_ties_even();
            let nearer = len(EARLY_LEFT[k]).min(len(EARLY_RIGHT[k]));
            let own = len(side[k]);
            let gain = if own > 0.0 { gain * nearer / own } else { 0.0 };
            (own as usize, gain as f32)
        };

        let predelay = if tempo_based {
            p(2) * rate * 0.3125 / tempo
        } else {
            p(2) * rate / 384.0
        };

        // The engine works the modulation out in f32 (`0x18000335e`).
        let speed = (p(13) / 2048.0) as f32;
        let hz = rate as f32;
        let feedback = feedback as f32;
        Tune {
            low_cut,
            high_cut: low_pass(params[1], rate),
            damping: low_pass(params[6], rate),
            lens,
            taps_left: TAPS_LEFT.map(scaled),
            taps_right: TAPS_RIGHT.map(scaled),
            diffuser: allpass([0, 1], 0.07),
            tank_first: allpass([4, 8], 0.35),
            tank_second: allpass([6, 10], 0.51),
            feedback,
            bass: ((ratio - 1.0) * 0.5) as f32,
            crossover: ((t - shelf) / (t + shelf)) as f32,
            weights: TAP_BLEND.map(|blend| (1.0 - feedback) * blend + feedback),
            early_left: std::array::from_fn(|k| early(k, &EARLY_LEFT, 0.5)),
            early_right: std::array::from_fn(|k| early(k, &EARLY_RIGHT, 0.36)),
            predelay: predelay.round_ties_even() as usize,
            lfo_step: (speed * speed * speed * 9.9 + 0.1) / hz,
            depth: (p(14) / 2048.0) as f32 * DEPTH as f32 * hz,
            depth_keep: 0.01f32.powf((1.0 / (f64::from(hz) * 0.1)) as f32),
        }
    }
}

/// A line as long as a power of two, read anywhere behind where it is
/// written.
#[derive(Debug, Clone, PartialEq, Default)]
struct Ring {
    samples: Vec<f32>,
    at: usize,
}

impl Ring {
    /// Have a power of two of at least `len` samples, all at 0.
    fn start(&mut self, len: usize) {
        self.samples.clear();
        self.samples.resize(len.next_power_of_two(), 0.0);
        self.at = 0;
    }

    fn write(&mut self, x: f32) {
        self.at = (self.at + 1) & (self.samples.len() - 1);
        self.samples[self.at] = x;
    }

    /// What was written `back` samples before the last one.
    fn read(&self, back: usize) -> f32 {
        self.samples[self.at.wrapping_sub(back) & (self.samples.len() - 1)]
    }

    /// What was written `back` samples before the last one, on a straight
    /// line between the two samples around it (`FUN_180001180`).
    fn read_between(&self, back: f32) -> f32 {
        let whole = back as usize;
        let (near, far) = (self.read(whole), self.read(whole + 1));
        let part = back - whole as f32;
        (1.0 - part) * near + part * far
    }
}

/// Fruity Reeverb 2. A new one has FL's defaults: a medium room that
/// rings for a second and a half.
#[derive(Debug, Clone, PartialEq)]
pub struct Reeverb2 {
    params: [i32; 15],
    tempo_based: bool,
    mid_side: bool,
    rate: f64,
    tempo: f64,
    tune: Tune,
    /// The input's level, and the mixes of the reflections and of the wet
    /// sound.
    dry: Matrix,
    early_mix: Matrix,
    wet_mix: Matrix,
    lines: [Line; 12],
    predelay: Line,
    early: Ring,
    /// The delay of each half that the LFO moves.
    swung: [Ring; 2],
    /// The high cut's state, and the low cut's: its allpass and the sample
    /// before.
    high: f32,
    low: [f32; 2],
    /// Each half's shelf, as the low cut's, and its damping.
    shelf: [[f32; 2]; 2],
    damped: [f32; 2],
    /// Where the LFO is in its turn, and how deep it goes just now.
    phase: f32,
    depth: f32,
}

impl Default for Reeverb2 {
    fn default() -> Self {
        let (rate, tempo) = (44_100.0, 120.0);
        let mut plugin = Reeverb2 {
            params: DEFAULTS,
            tempo_based: false,
            mid_side: false,
            rate,
            tempo,
            tune: Tune::new(&DEFAULTS, false, rate, tempo),
            dry: Matrix::new([0.0; 4]),
            early_mix: Matrix::new([0.0; 4]),
            wet_mix: Matrix::new([0.0; 4]),
            lines: Default::default(),
            predelay: Line::default(),
            early: Ring::default(),
            swung: Default::default(),
            high: 0.0,
            low: [0.0; 2],
            shelf: [[0.0; 2]; 2],
            damped: [0.0; 2],
            phase: 0.0,
            depth: 0.0,
        };
        plugin.make_room();
        plugin.predelay.start(1);
        plugin.aim();
        plugin.settle();
        plugin
    }
}

impl Reeverb2 {
    /// Have the lines' memory for this rate: the twelve at room size 100,
    /// a predelay of a second, the reflections' 0.3163 s and the deepest
    /// modulation. The predelay asks for more only where it keeps time
    /// with a tempo slow enough to make it longer than a second.
    fn make_room(&mut self) {
        let mut largest = DEFAULTS;
        (largest[2], largest[3]) = (RANGES[2].1, RANGES[3].1);
        let whole = Tune::new(&largest, false, self.rate, self.tempo);
        for (line, len) in self.lines.iter_mut().zip(whole.lens) {
            line.make_room(len);
        }
        self.predelay.make_room(whole.predelay);
        self.early.start((0.3163 * self.rate) as usize + 1);
        for ring in &mut self.swung {
            ring.start((DEPTH * self.rate) as usize + 4);
        }
        self.lines_anew();
    }

    /// Put the lines at the lengths the room asks for, empty.
    fn lines_anew(&mut self) {
        for (line, len) in self.lines.iter_mut().zip(self.tune.lens) {
            line.start(len);
        }
        self.early.samples.fill(0.0);
    }

    /// The three mixes the parameters ask for.
    fn aim(&mut self) {
        let p = |index: usize| f64::from(self.params[index]);
        let separation = p(9) / 64.0;
        let dry = level(p(10) / 128.0);
        self.dry.aim([dry, dry, 0.0, 0.0]);
        self.early_mix
            .aim(spread(level(p(11) / 256.0), separation, self.mid_side));
        self.wet_mix
            .aim(spread(level(p(12) / 256.0), separation, self.mid_side));
    }

    fn settle(&mut self) {
        for mix in [&mut self.dry, &mut self.early_mix, &mut self.wet_mix] {
            mix.settle();
        }
    }

    /// Take parameters that are inside their ranges, or a new tempo or
    /// switch. As in FL, another room size empties the lines, and another
    /// predelay empties everything.
    fn set(&mut self, params: [i32; 15]) {
        self.params = params;
        let tune = Tune::new(&params, self.tempo_based, self.rate, self.tempo);
        let (room, predelay) = (
            tune.lens != self.tune.lens,
            tune.predelay.max(1) != self.predelay.samples.len(),
        );
        self.tune = tune;
        if room {
            self.lines_anew();
        }
        if predelay {
            self.predelay.start(tune.predelay.max(1));
            self.clear();
        }
        self.aim();
    }

    /// What FL's reset does: every line and every filter at nothing.
    fn clear(&mut self) {
        for line in &mut self.lines {
            line.samples.fill(0.0);
        }
        self.predelay.samples.fill(0.0);
        self.early.samples.fill(0.0);
        for ring in &mut self.swung {
            ring.samples.fill(0.0);
        }
        self.high = 0.0;
        self.low = [0.0; 2];
        self.shelf = [[0.0; 2]; 2];
        self.damped = [0.0; 2];
    }

    /// One sample of the two sides added up through the network: the wet
    /// sound's two sides, and the early reflections' when they are asked
    /// for.
    fn frame(&mut self, input: f32, reflections: bool) -> ([f32; 2], [f32; 2]) {
        let tune = &self.tune;

        // The sine is of the phase before this frame's step, and the depth
        // jumps to its target once it is within 0.01 of it
        // (`0x180003420..0x1800034b4`).
        let before = self.phase;
        let phase = before + tune.lfo_step;
        self.phase = phase - phase.trunc();
        let gap = tune.depth - self.depth;
        self.depth = if gap.abs() < 0.01 {
            tune.depth
        } else {
            gap * (1.0 - tune.depth_keep) + self.depth + 1e-30
        };
        let wave = (f64::from(before) * std::f64::consts::TAU).sin() as f32;
        let swing = self.depth * ((wave + 1.0) * 0.5);

        // The high cut, then the low cut: half of what its allpass and
        // its input differ by.
        let x = input + 1e-30;
        self.high += (x - self.high) * tune.high_cut;
        let turned = (self.high - self.low[0]) * tune.low_cut + self.low[1];
        self.low = [turned, self.high];
        let mut x = if tune.low_cut != 0.0 {
            (self.high - turned) * 0.5 + 1e-30
        } else {
            self.high
        };

        // Every line steps right after its access, as FL's do. The first
        // diffuser feeds the reflections' line (`0x1800037f7`).
        let (diffusers, tank) = self.lines.split_at_mut(4);
        for (k, line) in diffusers.iter_mut().enumerate() {
            x = line.allpass(x, tune.diffuser);
            line.step();
            if k == 0 {
                self.early.write(x);
            }
        }
        // A predelay of 0 is a line of one sample.
        let fed = self.predelay.delay(x) + 1e-20;
        self.predelay.step();

        // Each half starts from the other half's last delay: the first
        // reads line 11 before the second writes it, the second reads line
        // 7 after the first has written it and stepped, one sample newer
        // (`0x180003a2c..0x180003a6a`).
        for half in 0..2 {
            let (first, second) = tank.split_at_mut(4);
            let (lines, other) = if half == 0 {
                (first, &second[3])
            } else {
                (second, &first[3])
            };
            let c = other.samples[other.at] * tune.feedback;
            let shelf = &mut self.shelf[half];
            let turned = (c - shelf[0]) * tune.crossover + shelf[1];
            *shelf = [turned, c];
            let mut y = (turned + c) * tune.bass + c + fed;
            y = (y - self.damped[half]) * tune.damping + self.damped[half];
            self.damped[half] = y;
            self.swung[half].write(y);
            let y = self.swung[half].read_between(swing);
            let v = lines[0].allpass(y, tune.tank_first);
            lines[0].step();
            let v = lines[1].delay(v) * tune.feedback;
            lines[1].step();
            let v = lines[2].allpass(v, tune.tank_second);
            lines[2].step();
            lines[3].delay(v);
            lines[3].step();
        }

        // The taps, read once every line has stepped, so a tap of `k` is
        // the sample written `k - 1` frames ago. Lines 5, 6 and 7 are the
        // tank's 1, 2 and 3 here, and 9, 10 and 11 its 5, 6 and 7. On the
        // right `w1` weighs the second tap (`0x180003cb8..0x180003d31`).
        let [w1, w2, w3, w4, w5] = tune.weights;
        let (l, r) = (&tune.taps_left, &tune.taps_right);
        let wet = [
            tank[5].tap(l[0])
                - w1 * tank[5].tap(l[1])
                - w3 * tank[6].tap(l[2])
                - w4 * tank[7].tap(l[3])
                + w2 * tank[1].tap(l[4])
                + tank[2].tap(l[5])
                + w5 * tank[3].tap(l[6]),
            w1 * tank[1].tap(r[1]) + tank[1].tap(r[0])
                - w3 * tank[2].tap(r[2])
                - w4 * tank[3].tap(r[3])
                - w2 * tank[5].tap(r[4])
                + tank[6].tap(r[5])
                + w5 * tank[7].tap(r[6]),
        ];

        let mut early = [0.0; 2];
        if reflections {
            let sides = [
                (&tune.early_left, &EARLY_LEFT_SIGNS),
                (&tune.early_right, &EARLY_RIGHT_SIGNS),
            ];
            for (sum, (taps, signs)) in early.iter_mut().zip(sides) {
                for ((back, gain), sign) in taps.iter().zip(signs) {
                    *sum += sign * gain * self.early.read(*back);
                }
            }
        }
        (wet, early)
    }
}

impl Effect for Reeverb2 {
    /// FL turns a version above 10001 down, and so does this. The oldest
    /// form leaves the tempo switch as it was.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let Some(first) = bytes.first_chunk::<4>() else {
            return Err(StateError::Length {
                expected: V10001_LEN,
                found: bytes.len(),
            });
        };
        let version = i32::from_le_bytes(*first);
        if version > 10001 {
            return Err(StateError::Version { found: version });
        }
        // Where the parameters start, how many there are, and how long
        // the state is.
        let (start, count, expected) = match version {
            10001 => (4, 15, V10001_LEN),
            10000 => (4, 13, V10000_LEN),
            _ => (0, 13, OLD_LEN),
        };
        if bytes.len() != expected {
            return Err(StateError::Length {
                expected,
                found: bytes.len(),
            });
        }
        let end = start + 4 * count;
        // The two parameters an older form does not have, then the ones
        // the state has over them.
        let mut words = [0u8; 60];
        words[52..56].copy_from_slice(&OLD_SPEED.to_le_bytes());
        words[..4 * count].copy_from_slice(&bytes[start..end]);
        let params = read_state(&words, &RANGES)?;
        if version >= 10000 {
            self.tempo_based = bytes[end] != 0;
            self.mid_side = bytes[end + 1] != 0;
        } else {
            self.mid_side = false;
        }
        self.set(params);
        self.settle();
        Ok(())
    }

    /// The lines are as long as the rate makes them, and start empty. This
    /// is where the plugin asks for their memory. A rate that is not a
    /// number of 1 or more is not taken.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz >= 1.0 {
            // FL has the rate as a whole number.
            self.rate = f64::from(hz).round();
            self.tune = Tune::new(&self.params, self.tempo_based, self.rate, self.tempo);
            self.make_room();
            self.predelay.start(self.tune.predelay.max(1));
            self.reset();
        }
    }

    /// A predelay that keeps time with the tempo is as long as the tempo
    /// makes it. A tempo that is not a number above 0 is not taken.
    fn set_tempo(&mut self, bpm: f64) {
        if bpm.is_finite() && bpm > 0.0 && bpm != self.tempo {
            self.tempo = bpm;
            self.set(self.params);
        }
    }

    /// After a level or the separation has moved, the mixes go from where
    /// the last block ended to where the parameters put them, in a
    /// straight line over this block.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        if len == 0 {
            return;
        }
        let silent = [0.0; 4];
        let reflections = self.early_mix != Matrix::new(silent);
        let (mut dry, dry_step) = self.dry.ramp(len);
        let (mut early_mix, early_step) = self.early_mix.ramp(len);
        let (mut wet_mix, wet_step) = self.wet_mix.ramp(len);
        for (l, r) in left[..len].iter_mut().zip(right[..len].iter_mut()) {
            let input = if self.mid_side { *l - *r } else { *l + *r };
            let (wet, early) = self.frame(input, reflections);
            let wet = Matrix::mix(&wet_mix, wet[0], wet[1]);
            let early = Matrix::mix(&early_mix, early[0], early[1]);
            *l = *l * dry[0] + early.0 + wet.0;
            *r = *r * dry[1] + early.1 + wet.1;
            for (mix, step) in [
                (&mut dry, &dry_step),
                (&mut early_mix, &early_step),
                (&mut wet_mix, &wet_step),
            ] {
                for (gain, step) in mix.iter_mut().zip(step) {
                    *gain += step;
                }
            }
        }
    }

    fn param_count(&self) -> usize {
        RANGES.len()
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |value| *value as f32)
    }

    /// Moving the room size or the predelay empties the lines, as in FL.
    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(range) = RANGES.get(index) {
            let mut params = self.params;
            params[index] = knob_value(value, *range);
            self.set(params);
        }
    }

    /// Empties the lines and the filters, puts the lines and the LFO back
    /// at their start and ends a ramp.
    fn reset(&mut self) {
        self.clear();
        for line in self.lines.iter_mut().chain([&mut self.predelay]) {
            line.at = 0;
        }
        for ring in self.swung.iter_mut().chain([&mut self.early]) {
            ring.at = 0;
        }
        self.phase = 0.0;
        self.depth = 0.0;
        self.settle();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{corpus, every_preset, partial, presets, sine, state};

    const HZ: f64 = 48_000.0;

    /// The newest form of the state.
    fn v10001(params: [i32; 15], tempo_based: bool, mid_side: bool) -> Vec<u8> {
        let mut bytes = state(&[10001]);
        bytes.extend(state(&params));
        bytes.extend([u8::from(tempo_based), u8::from(mid_side)]);
        bytes
    }

    fn at_48k(bytes: &[u8]) -> Reeverb2 {
        let mut plugin = Reeverb2::default();
        plugin.set_sample_rate(HZ as f32);
        plugin.set_state(bytes).unwrap();
        plugin
    }

    fn with(params: [i32; 15]) -> Reeverb2 {
        at_48k(&v10001(params, false, false))
    }

    /// The defaults with some parameters put elsewhere.
    fn moved(moves: &[(usize, i32)]) -> [i32; 15] {
        let mut params = DEFAULTS;
        for (index, value) in moves {
            params[*index] = *value;
        }
        params
    }

    /// The two cuts and the damping off and no dry sound: the network on
    /// its own. `early` and `wet` are the two level wheels.
    fn bare(early: i32, wet: i32) -> [i32; 15] {
        moved(&[(0, 19), (1, 221), (6, 221), (10, 0), (11, early), (12, wet)])
    }

    /// The plugin's two outputs for these two inputs, in blocks of 512.
    fn run(plugin: &mut Reeverb2, left: &[f32], right: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let (mut left, mut right) = (left.to_vec(), right.to_vec());
        for (l, r) in left.chunks_mut(512).zip(right.chunks_mut(512)) {
            plugin.process(l, r);
        }
        (left, right)
    }

    /// `len` frames out for one frame of 1 on the left.
    fn impulse(plugin: &mut Reeverb2, len: usize) -> (Vec<f32>, Vec<f32>) {
        let mut left = vec![0.0; len];
        left[0] = 1.0;
        run(plugin, &left, &vec![0.0; len])
    }

    /// The first frame that is louder than what FL adds to keep clear of
    /// numbers too small to hold.
    fn onset(side: &[f32]) -> usize {
        side.iter().position(|v| v.abs() > 1e-9).unwrap()
    }

    /// The frames of a side that are not 0, with what they are.
    fn hits(side: &[f32]) -> Vec<(usize, f32)> {
        let frames = side.iter().copied().enumerate();
        frames.filter(|(_, value)| value.abs() > 1e-9).collect()
    }

    /// How fast a side falls between two frames, in dB a second, from the
    /// power of the tenth of a second after each.
    fn fall(side: &[f32], from: usize, to: usize) -> f64 {
        let power = |at: usize| -> f64 {
            let squares = side[at..at + 4800].iter().map(|v| f64::from(*v).powi(2));
            10.0 * (squares.sum::<f64>() / 4800.0).log10()
        };
        (power(from) - power(to)) / ((to - from) as f64 / HZ)
    }

    fn db(gain: f64) -> f64 {
        20.0 * gain.log10()
    }

    #[test]
    fn the_lines_are_the_doc_s_table_scaled_by_the_rate_and_the_room() {
        // At room size 100 a line is its number over 20000 seconds long.
        let tune = Tune::new(&moved(&[(3, 100)]), false, 44_100.0, 120.0);
        let want = LINES.map(|units| (units * 44_100 / 20_000) as usize);
        assert_eq!(tune.lens, want);
        assert_eq!(tune.lens[5], 9818);
        assert!((tune.lens[5] as f64 / 44.1 - 222.6).abs() < 0.05);
        // Twice the rate is twice the frames, to the frame.
        let (slow, fast) = (with(DEFAULTS).tune, {
            let mut plugin = Reeverb2::default();
            plugin.set_sample_rate(96_000.0);
            plugin.tune
        });
        for (slow, fast) in slow.lens.iter().zip(fast.lens) {
            assert!(fast.abs_diff(2 * slow) <= 1, "{slow} and {fast}");
        }
        // The room scales them by (room + 15) / 115: room 17 is 28 % of
        // room 100, and room 1 is 14 %.
        for (room, part) in [(100, 1.0), (50, 65.0 / 115.0), (17, 0.278), (1, 0.139)] {
            let tune = with(moved(&[(3, room)])).tune;
            let scale = (HZ * f64::from(room + 15) / 115.0).round() as u64;
            assert_eq!(
                tune.lens,
                LINES.map(|units| (units * scale / 20_000) as usize)
            );
            let whole = 4453.0 * HZ / 20_000.0;
            assert!((tune.lens[5] as f64 / whole - part).abs() < 0.001, "{room}");
            // Every tap is inside the line it reads.
            let lines = [9, 9, 10, 11, 5, 6, 7];
            for (tap, line) in tune.taps_left.iter().zip(lines) {
                assert!(*tap < tune.lens[line], "room {room}");
            }
            let lines = [5, 5, 6, 7, 9, 10, 11];
            for (tap, line) in tune.taps_right.iter().zip(lines) {
                assert!(*tap < tune.lens[line], "room {room}");
            }
        }
        assert_eq!(
            with(DEFAULTS).tune.taps_left[0],
            (266 * 27_130 / 20_000) as usize
        );
    }

    #[test]
    fn the_coefficients_are_the_doc_s() {
        // "The Venue": low cut 400 Hz, high cut 6 kHz, diffusion 65 %,
        // decay 2.3 s, damping 3.7 kHz, bass x0.7, crossover 330 Hz.
        let venue = moved(&[
            (0, 400),
            (1, 60),
            (3, 60),
            (4, 65),
            (5, 23),
            (6, 37),
            (7, 70),
            (8, 330),
        ]);
        let tune = with(venue).tune;
        let close = |got: f32, want: f64| (f64::from(got) - want).abs() < 1e-6;
        let t = (PI * 400.0 / HZ).tan();
        assert!(close(tune.low_cut, (t - 1.0) / (t + 1.0)));
        let pole = |hz: f64| {
            let c = 2.0 - (2.0 * PI * hz / HZ).cos();
            1.0 - (c - (c * c - 1.0).sqrt())
        };
        assert!(close(tune.high_cut, pole(6000.0)) && close(tune.damping, pole(3700.0)));

        let lens = tune.lens.map(|len| len as f64);
        let ln = 0.001f64.ln();
        let allpass = |a: usize, b: usize, spread: f64| {
            (ln * (lens[a] + lens[b]) / (2.0 * HZ) / (0.65 * spread)).exp()
        };
        assert!(close(tune.diffuser, allpass(0, 1, 0.07)));
        assert!(close(tune.tank_first, allpass(4, 8, 0.35)));
        assert!(close(tune.tank_second, allpass(6, 10, 0.51)));

        // Four delays a trip, and 60 dB down in the decay time.
        let around = lens[5] + lens[7] + lens[9] + lens[11];
        let feedback = (ln * around / (2.3 * HZ)).exp().powf(0.25);
        assert!(close(tune.feedback, feedback));
        let trips = 2.3 * HZ / around;
        assert!((db(f64::from(tune.feedback).powf(4.0 * trips)) + 60.0).abs() < 1e-3);
        // The bass rings 0.7 times as long: its gain over the feedback is
        // what the shelf does to the lows.
        let ratio = (ln * around / (2.3 * HZ * 0.7)).exp().powf(0.25) / feedback;
        assert!(close(tune.bass, (ratio - 1.0) * 0.5) && ratio < 1.0);
        let t = (PI * 330.0 / HZ).tan();
        assert!(close(tune.crossover, (t - ratio) / (t + ratio)));
        // A bass that rings longer leaves the shelf's corner alone.
        let long = with(moved(&[(7, 150), (8, 298)])).tune;
        let t = (PI * 298.0 / HZ).tan();
        assert!(long.bass > 0.0 && close(long.crossover, (t - 1.0) / (t + 1.0)));
        // The tap weights go from their own number to 1 with the feedback.
        for (weight, blend) in tune.weights.iter().zip(TAP_BLEND) {
            let want = (1.0 - feedback) * f64::from(blend) + feedback;
            assert!(close(*weight, want));
        }

        // What is off: no low cut, cuts wide open, and no diffusion makes
        // the allpasses plain delays.
        let off = with(moved(&[(0, 19), (1, 221), (4, 0), (6, 221)])).tune;
        assert_eq!((off.low_cut, off.high_cut, off.damping), (0.0, 1.0, 1.0));
        assert_eq!(
            (off.diffuser, off.tank_first, off.tank_second),
            (0.0, 0.0, 0.0)
        );
        assert_eq!(with(DEFAULTS).tune.bass, 0.0);
    }

    #[test]
    fn the_lfo_turns_at_speed_cubed_times_9_9_plus_0_1_hz() {
        for (speed, hz) in [(0, 0.1), (684, 0.4688), (1024, 1.3375), (2048, 10.0)] {
            let tune = with(moved(&[(13, speed)])).tune;
            let want = (f64::from(speed) / 2048.0).powi(3) * 9.9 + 0.1;
            assert!(
                (f64::from(tune.lfo_step) * HZ - want).abs() < 1e-5,
                "{speed}"
            );
            assert!((want - hz).abs() < 1e-3, "{speed}");
        }
        // The depth is up to 200 samples at 44.1 kHz, and as long in time
        // at any rate. It gets there in a tenth of a second.
        let tune = with(moved(&[(14, 2048)])).tune;
        assert!((f64::from(tune.depth) - 200.0 / 44_100.0 * HZ).abs() < 1e-3);
        assert!((f64::from(tune.depth_keep).powf(0.1 * HZ) - 0.01).abs() < 1e-4);
        assert_eq!(with(moved(&[(14, 1024)])).tune.depth * 2.0, tune.depth);
        assert_eq!(with(DEFAULTS).tune.depth, 0.0);

        // With no depth the speed changes nothing, and with depth it does.
        let tail = |params: [i32; 15]| impulse(&mut with(params), 24_000).0;
        let still = tail(bare(0, 128));
        assert!(tail(moved_from(bare(0, 128), 13, 2048)) == still);
        let swung = tail(moved_from(bare(0, 128), 14, 1280));
        assert!(swung != still);
        // The swing moves the tank's delay and not what goes into it, so
        // the wet sound starts on the same frame.
        assert_eq!(onset(&swung), onset(&still));
    }

    fn moved_from(mut params: [i32; 15], index: usize, value: i32) -> [i32; 15] {
        params[index] = value;
        params
    }

    #[test]
    fn the_first_early_reflection_is_at_the_doc_s_tap() {
        // Room 50 at 48 kHz: a tap is its number times 48000 * 53 *
        // 3.302e-7. The first right one is at 837 and the first left one
        // at 1102, 14.6 and 19.3 ms.
        let reach = HZ * 53.0 * 3.302e-7;
        let (left_at, right_at) = ((1102.0 * reach).round(), (837.0 * reach).round());
        assert_eq!((left_at, right_at), (926.0, 703.0));
        assert!((left_at / 48.0 - 19.3).abs() < 0.05);
        let (left_at, right_at) = (left_at as usize, right_at as usize);

        // Only the reflections, at -12.7 dB.
        let level = (11f64.sqrt() - 1.0) / 10.0;
        assert!((db(level) + 12.7).abs() < 0.05);
        let mut plugin = with(bare(128, 0));
        let diffuser = f64::from(plugin.tune.diffuser);
        let (left, right) = impulse(&mut plugin, 12_000);
        // A diffuser lets its input through at once, at its gain upside
        // down. The reflections are tapped after the first, so the first
        // reflection is the impulse at -g, at the tap's delay.
        assert_eq!((onset(&left), onset(&right)), (left_at, right_at));
        let first = -diffuser * level;
        let nearer = right_at as f64 / left_at as f64;
        assert!((f64::from(left[left_at]) - first * 0.5 * nearer).abs() < 1e-6);
        assert!((f64::from(right[right_at]) - first * 0.36).abs() < 1e-6);

        // With no diffusion the diffusers are delays, and the impulse
        // comes out as the taps and nothing else: seven on the left and
        // eight on the right, each behind the first line.
        let mut plugin = with(moved_from(bare(128, 0), 4, 0));
        let lines = plugin.tune.lens[0];
        let (left, right) = impulse(&mut plugin, 12_000);
        let taps = |numbers: [f64; 8], signs: [f32; 8], gain: f64| -> Vec<(usize, f32)> {
            let all = (0..8).filter(|k| signs[*k] != 0.0);
            all.map(|k| {
                let own = (numbers[k] * reach).round();
                let near = own.min(
                    (EARLY_LEFT[k] * reach)
                        .round()
                        .min((EARLY_RIGHT[k] * reach).round()),
                );
                let size = gain * near / own * level;
                (lines + own as usize, signs[k] * size as f32)
            })
            .collect()
        };
        let want_left = taps(EARLY_LEFT, EARLY_LEFT_SIGNS, 0.5);
        let want_right = taps(EARLY_RIGHT, EARLY_RIGHT_SIGNS, 0.36);
        assert_eq!((want_left.len(), want_right.len()), (7, 8));
        for (got, want) in [(hits(&left), want_left), (hits(&right), want_right)] {
            assert_eq!(got.len(), want.len());
            for ((at, value), (want_at, want_value)) in got.into_iter().zip(want) {
                assert_eq!(at, want_at);
                assert!((value - want_value).abs() < 1e-6, "{at}: {value}");
            }
        }
        // The last tap is at 162 ms at 44.1 kHz, as the doc has it.
        assert!((9263.0f64 * 44_100.0 * 53.0 * 3.302e-7 / 44.1 - 162.0).abs() < 0.5);
    }

    #[test]
    fn the_wet_taps_hear_the_tank_as_fl_s_engine_reads_it() {
        // With no diffusion every allpass is a plain delay, and with the
        // cuts, the damping and the modulation off and a flat bass an
        // impulse goes round the tank as single hits. `FUN_180003270`
        // steps every line right after its access and reads the taps after
        // that, so a tap of `k` hears what was written `k - 1` frames
        // before, and a predelay of 0 is one frame.
        let mut plugin = with(moved_from(bare(0, 128), 4, 0));
        let t = plugin.tune;
        let (len, g) = (t.lens, f64::from(t.feedback));
        let [w1, w2, w3, w4, w5] = t.weights.map(f64::from);
        let level = (11f64.sqrt() - 1.0) / 10.0;
        // The frame each tank line is first written: the four diffusers
        // and the predelay's one, then the half's own lines.
        let into = len[..4].iter().sum::<usize>() + 1;
        let (l5, l9) = (into + len[4], into + len[8]);
        let (l6, l10) = (l5 + len[5], l9 + len[9]);
        let (l7, l11) = (l6 + len[6], l10 + len[10]);
        // Lines 5 and 9 hold the impulse, 6, 7, 10 and 11 it times g.
        let (l, r) = (t.taps_left, t.taps_right);
        let want_left = [
            (l9 + l[0], 1.0),
            (l9 + l[1], -w1),
            (l10 + l[2], -w3 * g),
            (l11 + l[3], -w4 * g),
            (l5 + l[4], w2),
            (l6 + l[5], g),
            (l7 + l[6], w5 * g),
        ];
        // On the right w1 weighs the second tap (`0x180003cb8..0x180003d31`).
        let want_right = [
            (l5 + r[0], 1.0),
            (l5 + r[1], w1),
            (l6 + r[2], -w3 * g),
            (l7 + r[3], -w4 * g),
            (l9 + r[4], -w2),
            (l10 + r[5], g),
            (l11 + r[6], w5 * g),
        ];
        // The second half reads line 7 after the first has written it and
        // stepped, so the impulse comes round into line 9 a frame sooner
        // than its length says. The first half reads line 11 before the
        // second writes it, a whole length later.
        let (round9, round5) = (l7 + len[7] - 1 + len[8], l11 + len[11] + len[4]);
        let (left, right) = impulse(&mut plugin, round9.max(round5) + 4000);
        let first_pass = |side: &[f32]| hits(&side[..round9.min(round5)]);
        for (side, want) in [(&left, want_left), (&right, want_right)] {
            let mut want = want.map(|(at, value)| (at - 1, value * level));
            want.sort_by_key(|(at, _)| *at);
            let got = first_pass(side);
            assert_eq!(got.len(), 7);
            for ((at, value), (want_at, want_value)) in got.into_iter().zip(want) {
                assert_eq!(at, want_at);
                assert!((f64::from(value) - want_value).abs() < 1e-6, "{at}");
            }
        }
        let again = g * g * level;
        assert!((f64::from(left[round9 + l[0] - 1]) - again).abs() < 1e-6);
        assert!((f64::from(right[round5 + r[0] - 1]) - again).abs() < 1e-6);
    }

    #[test]
    fn the_tail_falls_as_the_feedback_and_the_loop_s_length_say() {
        // The feedback is worked out for the four delays alone: 60 dB in
        // the decay time over 0.44 s a trip. A trip also goes through four
        // allpasses, which hold the sound back by their own length on
        // average, so it takes 1.39 times as long and the tail falls that
        // much slower than the wheel says. Late in the tail it is slower
        // still, since what is left there is what the allpasses hold
        // longest.
        let slowed = |tune: &Tune| {
            let sum = |lines: [usize; 4]| lines.iter().map(|l| tune.lens[*l]).sum::<usize>() as f64;
            let (delays, allpasses) = (sum([5, 7, 9, 11]), sum([4, 6, 8, 10]));
            (delays + allpasses) / delays
        };
        let mut falls = Vec::new();
        for decay in [15, 30] {
            let mut plugin = with(moved_from(bare(0, 128), 5, decay));
            let slowed = slowed(&plugin.tune);
            assert!((slowed - 1.388).abs() < 0.001);
            let (left, right) = impulse(&mut plugin, 111_000);
            // Between 0.2 s and 2.2 s.
            let wheel = 60.0 / (f64::from(decay) * 0.1);
            for side in [&left, &right] {
                let fall = fall(side, 9600, 105_600);
                assert!(
                    (fall * slowed / wheel - 1.0).abs() < 0.1,
                    "decay {decay}: {fall} dB/s and not {}",
                    wheel / slowed
                );
                // And not what the wheel alone says.
                assert!(fall < 0.75 * wheel);
            }
            falls.push(fall(&left, 9600, 105_600));
        }
        // Twice the decay time is about half the fall. The ratio comes out
        // at 1.90, since the longer tail has more of what the allpasses
        // hold longest in the window.
        assert!((falls[0] / falls[1] - 1.9).abs() < 0.05, "{falls:?}");
    }

    #[test]
    fn the_high_cut_takes_a_high_tone_out_of_the_tail() {
        // A tone at 10 kHz for a second, then nothing: 4800 periods.
        let turns = 10_000.0 / HZ;
        let mut tone = sine(turns, 48_000);
        tone.resize(57_600, 0.0);
        let heard = |cut: i32| {
            let mut plugin = with(moved_from(bare(0, 128), 1, cut));
            let (left, _) = run(&mut plugin, &tone, &tone);
            // The tone once the tank is full of it, and the tenth of a
            // second after it has stopped.
            let tail = left[48_000..52_800].iter().map(|v| f64::from(*v).powi(2));
            (
                partial(&left[43_200..48_000], turns),
                tail.sum::<f64>().sqrt(),
            )
        };
        let (open, open_tail) = heard(221);
        // At 1 kHz the cut is a one-pole low pass, which leaves 10 % of
        // 10 kHz.
        let (cut, cut_tail) = heard(10);
        let take = f64::from(low_pass(10, HZ));
        let c = 2.0 - (2.0 * PI * 1000.0 / HZ).cos();
        assert!((take - (1.0 - (c - (c * c - 1.0).sqrt()))).abs() < 1e-7);
        let want = crate::testing::response(&[take], &[1.0, take - 1.0], turns);
        assert!(want < 0.11 && want > 0.09, "{want}");
        assert!((cut / open / want - 1.0).abs() < 0.01, "{}", cut / open);
        assert!((cut_tail / open_tail / want - 1.0).abs() < 0.02);
        // A low tone passes.
        let low = 100.0 / HZ;
        let through = |cut: i32| {
            let mut plugin = with(moved_from(bare(0, 128), 1, cut));
            let (left, _) = run(&mut plugin, &sine(low, 48_000), &sine(low, 48_000));
            partial(&left[43_200..], low)
        };
        assert!((through(10) / through(221) - 1.0).abs() < 0.01);
    }

    #[test]
    fn the_predelay_moves_the_wet_sound_s_start_by_its_frames() {
        // A predelay of 0 is a line of one sample (`FUN_006d1d00`), so the
        // plain sound is already a frame late, and one of `n` frames moves
        // it `n - 1` more.
        let mut plain = with(bare(0, 128));
        assert_eq!((plain.tune.predelay, plain.predelay.samples.len()), (0, 1));
        let plain = impulse(&mut plain, 30_000).0;
        let start = onset(&plain);
        for (wheel, tempo) in [
            (27, None),
            (1, None),
            (384, None),
            (48, Some(120.0)),
            (48, Some(75.0)),
        ] {
            // 1/384 s a step, or 1/192 beat with the tempo switch.
            let want = match tempo {
                None => (f64::from(wheel) * HZ / 384.0).round() as usize,
                Some(bpm) => (f64::from(wheel) / 192.0 * 60.0 / bpm * HZ).round() as usize,
            };
            let state = v10001(moved_from(bare(0, 128), 2, wheel), tempo.is_some(), false);
            let mut plugin = Reeverb2::default();
            plugin.set_sample_rate(HZ as f32);
            plugin.set_tempo(tempo.unwrap_or(140.0));
            plugin.set_state(&state).unwrap();
            assert_eq!(plugin.tune.predelay, want, "predelay {wheel}");
            let late = impulse(&mut plugin, 30_000 + want).0;
            assert_eq!(onset(&late), start + want - 1, "predelay {wheel}");
            // And all of it is the same sound, that much later.
            assert!(late[..want - 1].iter().all(|v| v.abs() < 1e-9));
            for (late, plain) in late[want - 1..].iter().zip(&plain) {
                assert!((late - plain).abs() < 1e-9, "predelay {wheel}");
            }
        }
        assert_eq!((27.0f64 * HZ / 384.0).round(), 3375.0);
        // A quarter of a beat at 120 is an eighth of a second.
        assert_eq!(48.0 / 192.0 * 60.0 / 120.0 * HZ, 6000.0);

        // The early reflections are tapped off ahead of the predelay, and
        // stay where they are.
        let early = |wheel: i32| impulse(&mut with(moved_from(bare(128, 0), 2, wheel)), 9000).1;
        assert_eq!((onset(&early(0)), onset(&early(384))), (703, 703));
        assert!(early(0) == early(384));

        // A tempo moves a predelay that keeps time with it, and no other.
        let mut plugin = at_48k(&v10001(moved(&[(2, 48)]), true, false));
        plugin.set_tempo(60.0);
        assert_eq!(plugin.tune.predelay, 12_000);
        plugin.set_tempo(f64::NAN);
        plugin.set_tempo(0.0);
        assert_eq!(plugin.predelay.samples.len(), 12_000);
        let mut plugin = with(moved(&[(2, 48)]));
        plugin.set_tempo(60.0);
        assert_eq!(plugin.tune.predelay, 6000);
    }

    #[test]
    fn the_dry_and_the_wet_ends_of_the_mix_are_exact() {
        let (left, right) = (sine(0.0123, 6000), sine(0.0311, 6000));
        // Only the dry sound, at unity: the input to the bit.
        let mut plugin = with(moved(&[(10, 128), (11, 0), (12, 0)]));
        let (l, r) = run(&mut plugin, &left, &right);
        assert!(l == left && r == right);
        // The dry wheel is the knob curve over 128: +5.6 dB at its end,
        // where the doc's prose has +4.1.
        let mut plugin = with(moved(&[(10, 160), (11, 0), (12, 0)]));
        let top = (11f64.powf(1.25) - 1.0) / 10.0;
        assert!((db(top) - 5.6).abs() < 0.05);
        let (l, _) = run(&mut plugin, &left, &right);
        assert!(
            l.iter()
                .zip(&left)
                .all(|(got, sent)| *got == sent * top as f32)
        );
        // Nothing at all: silence.
        let mut plugin = with(moved(&[(10, 0), (11, 0), (12, 0)]));
        let (l, r) = run(&mut plugin, &left, &right);
        assert!(l.iter().chain(&r).all(|v| *v == 0.0));

        // Only the wet sound: none of the input is in it. It starts at
        // nothing, and is what the dry and the wet together are less the
        // input.
        let wet_only = moved(&[(10, 0), (11, 160), (12, 160)]);
        let (wet_l, wet_r) = run(&mut with(wet_only), &left, &right);
        assert!(
            wet_l[..64]
                .iter()
                .chain(&wet_r[..64])
                .all(|v| v.abs() < 1e-9)
        );
        assert!(wet_l[3000..].iter().any(|v| v.abs() > 0.01));
        let both = moved(&[(10, 128), (11, 160), (12, 160)]);
        let (both_l, both_r) = run(&mut with(both), &left, &right);
        for n in 0..6000 {
            assert!((both_l[n] - left[n] - wet_l[n]).abs() < 1e-6);
            assert!((both_r[n] - right[n] - wet_r[n]).abs() < 1e-6);
        }
        // The wet and the reflection wheels are the knob curve over 256:
        // -12.7 dB at 128 and -9.2 dB at their end.
        let level = |wheel: f64| (11f64.powf(wheel / 256.0) - 1.0) / 10.0;
        assert!((db(level(128.0)) + 12.7).abs() < 0.05 && (db(level(160.0)) + 9.2).abs() < 0.05);
        let quieter = moved(&[(10, 0), (11, 128), (12, 128)]);
        let (quiet_l, _) = run(&mut with(quieter), &left, &right);
        let ratio = (level(128.0) / level(160.0)) as f32;
        for n in 3000..6000 {
            assert!((quiet_l[n] - wet_l[n] * ratio).abs() < 1e-6);
        }
    }

    #[test]
    fn the_separation_mixes_the_two_sides_of_the_reverb() {
        assert_eq!(spread(0.5, 0.0, false), [0.5, 0.5, 0.0, 0.0]);
        // All the way merged: each side is half itself and half the other.
        assert_eq!(spread(0.5, 1.0, false), [0.25; 4]);
        // Half-way: (sqrt(5) - 1) / 4 of the way to that.
        let crossed = ((5f64.sqrt() - 1.0) * 0.25 * 0.5) as f32;
        assert_eq!(
            spread(1.0, 0.5, false),
            [1.0 - crossed, 1.0 - crossed, crossed, crossed]
        );
        // All the way apart: half of the other side upside down.
        assert_eq!(spread(0.5, -1.0, false), [0.75, 0.75, -0.25, -0.25]);
        assert_eq!(
            spread(1.0, -0.625, false),
            [1.3125, 1.3125, -0.3125, -0.3125]
        );
        // A mid/side input turns the right output over.
        assert_eq!(spread(0.5, 0.0, true), [0.5, -0.5, 0.0, 0.0]);
        assert_eq!(spread(0.5, 1.0, true), [0.25, -0.25, 0.25, -0.25]);

        // Through the plugin: merged, the two outputs are one, and apart
        // they differ more than as they come.
        let tail = |separation: i32| {
            let params = moved_from(bare(128, 128), 9, separation);
            impulse(&mut with(params), 24_000)
        };
        let (plain_l, plain_r) = tail(0);
        let (mono_l, mono_r) = tail(64);
        assert!(mono_l == mono_r && plain_l != plain_r);
        let (wide_l, wide_r) = tail(-64);
        for n in (0..24_000).step_by(97) {
            let mono = 0.5 * (plain_l[n] + plain_r[n]);
            assert!((mono_l[n] - mono).abs() < 1e-6);
            let side = plain_l[n] - plain_r[n];
            assert!((wide_l[n] - wide_r[n] - 2.0 * side).abs() < 1e-5);
            assert!((wide_l[n] + wide_r[n] - plain_l[n] - plain_r[n]).abs() < 1e-5);
        }

        // The reverb hears the two sides added up, or the left less the
        // right with the mid/side switch.
        let (left, right) = (sine(0.01, 4000), sine(0.017, 4000));
        let sum: Vec<f32> = left.iter().zip(&right).map(|(l, r)| l + r).collect();
        let less: Vec<f32> = left.iter().zip(&right).map(|(l, r)| l - r).collect();
        let silent = vec![0.0; 4000];
        let stereo = run(&mut with(bare(128, 128)), &left, &right);
        assert!(stereo == run(&mut with(bare(128, 128)), &sum, &silent));
        let mid_side = v10001(bare(128, 128), false, true);
        let (ms_l, ms_r) = run(&mut at_48k(&mid_side), &left, &right);
        let (want_l, want_r) = run(&mut with(bare(128, 128)), &less, &silent);
        assert!(ms_l == want_l);
        assert!(ms_r.iter().zip(&want_r).all(|(got, want)| *got == -*want));
    }

    #[test]
    fn a_moved_wheel_asks_for_no_memory_and_is_the_same_plugin_as_its_state() {
        let mut plugin = Reeverb2::default();
        plugin.set_sample_rate(HZ as f32);
        let memory = |plugin: &Reeverb2| -> Vec<*const f32> {
            let lines = plugin.lines.iter().map(|line| line.samples.as_ptr());
            let rings = [&plugin.early, &plugin.swung[0], &plugin.swung[1]];
            lines
                .chain(rings.map(|ring| ring.samples.as_ptr()))
                .chain([plugin.predelay.samples.as_ptr()])
                .collect()
        };
        // The reflections' line is the power of two over 0.3163 s, as the
        // doc has it: 16384 at 44.1 kHz.
        assert_eq!(plugin.early.samples.len(), 16_384);
        assert_eq!(Reeverb2::default().early.samples.len(), 16_384);
        plugin.set_param(2, 384.0);
        let before = memory(&plugin);
        for (room, predelay) in [(100.0, 0.0), (1.0, 384.0), (73.0, 100.0), (50.0, 27.0)] {
            plugin.set_param(3, room);
            plugin.set_param(2, predelay);
            plugin.set_param(14, 2048.0);
            plugin.process(&mut [0.5; 700], &mut [-0.25; 700]);
        }
        assert_eq!(memory(&plugin), before);

        let moves = [
            400.4, 60.0, 31.0, 60.0, 65.0, 999.0, 37.0, 70.0, 330.0, -40.0, 128.0, -5.0, 102.0,
            610.0, 1280.0,
        ];
        for (index, value) in moves.into_iter().enumerate() {
            plugin.set_param(index, value);
        }
        plugin.set_param(15, 1.0);
        assert_eq!((plugin.param(15), plugin.param_count()), (0.0, 15));
        let params = [
            400, 60, 31, 60, 65, 200, 37, 70, 330, -40, 128, 0, 102, 610, 1280,
        ];
        assert_eq!(
            std::array::from_fn::<f32, 15, _>(|i| plugin.param(i)),
            params.map(|v| v as f32)
        );
        plugin.reset();
        assert_eq!(plugin, with(params));
        // A moved level ramps over one block: the dry sound from 1 to 0.
        let mut plugin = with(moved(&[(11, 0), (12, 0)]));
        plugin.set_param(10, 0.0);
        let (mut left, mut right) = ([1.0; 4], [-1.0; 4]);
        plugin.process(&mut left, &mut right);
        assert_eq!(
            (left, right),
            ([1.0, 0.75, 0.5, 0.25], [-1.0, -0.75, -0.5, -0.25])
        );
        // A rate that is not one leaves the plugin as it is.
        let before = plugin.clone();
        plugin.set_sample_rate(f32::NAN);
        plugin.set_sample_rate(0.0);
        assert_eq!(plugin, before);
    }

    #[test]
    fn the_three_forms_load_and_others_are_turned_down() {
        let params = moved(&[(0, 400), (5, 23), (9, -40), (13, 610), (14, 1280)]);
        let newest = v10001(params, true, false);
        assert_eq!(newest.len(), 66);
        let plugin = at_48k(&newest);
        assert_eq!(
            (plugin.params, plugin.tempo_based, plugin.mid_side),
            (params, true, false)
        );
        let mut default = Reeverb2::default();
        default.set_state(&v10001(DEFAULTS, false, false)).unwrap();
        assert_eq!(default, Reeverb2::default());

        // 10000 has 13 parameters and the two switches, and the
        // modulation as it was before there were wheels for it.
        let mut older = state(&[10000]);
        older.extend(state(&params[..13]));
        older.extend([0, 1]);
        assert_eq!(older.len(), 58);
        let mut plugin = at_48k(&newest);
        plugin.set_state(&older).unwrap();
        assert_eq!(
            plugin.params,
            moved_from(moved_from(params, 13, 684), 14, 0)
        );
        assert!(!plugin.tempo_based && plugin.mid_side);

        // The oldest starts with its first parameter. Its last byte is
        // not read, the mid/side switch goes off and the tempo switch
        // stays.
        let mut oldest = state(&params[..13]);
        oldest.push(1);
        assert_eq!(oldest.len(), 53);
        let mut plugin = at_48k(&v10001(DEFAULTS, true, true));
        plugin.set_state(&oldest).unwrap();
        assert_eq!(
            plugin.params,
            moved_from(moved_from(params, 13, 684), 14, 0)
        );
        assert!(plugin.tempo_based && !plugin.mid_side);

        let before = plugin.clone();
        assert_eq!(
            plugin.set_state(&state(&[10002, 0])),
            Err(StateError::Version { found: 10002 })
        );
        for (bytes, expected) in [
            (&newest[..65], 66),
            (&older[..57], 58),
            (&oldest[..52], 53),
            (&[1, 2, 3][..], 66),
            (&[][..], 66),
        ] {
            assert_eq!(
                plugin.set_state(bytes),
                Err(StateError::Length {
                    expected,
                    found: bytes.len()
                })
            );
        }
        for (param, value) in [
            (0, 18),
            (1, 222),
            (3, 0),
            (5, 201),
            (9, -65),
            (12, 161),
            (14, 2049),
        ] {
            assert_eq!(
                plugin.set_state(&v10001(moved(&[(param, value)]), false, false)),
                Err(StateError::Range { param, value })
            );
        }
        assert_eq!(plugin, before);
    }

    #[test]
    fn fl_s_presets_load_as_the_doc_reads_them() {
        let Some(own) = presets(NAME) else { return };
        let all = every_preset(&corpus().unwrap(), NAME);
        // Six of the plugin's own and a mixer preset in the oldest form,
        // and the three of the plugin database in the one after.
        assert_eq!((own.len(), all.len()), (6, 10));
        let lengths = |len: usize| all.iter().filter(|p| p.state.len() == len).count();
        assert_eq!((lengths(53), lengths(58), lengths(66)), (7, 3, 0));
        let named = |file: &str| {
            let preset = all.iter().find(|p| p.file == file);
            at_48k(&preset.unwrap_or_else(|| panic!("no preset {file}")).state)
        };
        for preset in &all {
            let plugin = named(&preset.file);
            assert_eq!(plugin.params[13..], [684, 0], "{}", preset.file);
            assert!(!plugin.tempo_based && !plugin.mid_side, "{}", preset.file);
        }
        // The template is the defaults, and "Default" has its own levels.
        assert_eq!(named("Fruity Reeverb 2").params, DEFAULTS);
        let default = named("Default");
        assert_eq!(default.params, moved(&[(11, 128), (12, 87)]));
        let level = |wheel: i32| db((11f64.powf(f64::from(wheel) / 256.0) - 1.0) / 10.0);
        assert!((level(128) + 12.7).abs() < 0.05 && (level(87) + 18.0).abs() < 0.05);

        // The doc's table.
        let hall = named("Large Hall");
        assert_eq!(hall.params[..6], [19, 112, 18, 40, 100, 22]);
        assert_eq!(hall.tune.low_cut, 0.0);
        assert!((hall.tune.predelay as f64 / 48.0 - 47.0).abs() < 0.5);
        let venue = named("The Venue");
        assert_eq!(venue.params[..9], [400, 60, 31, 60, 65, 23, 37, 70, 330]);
        assert!((venue.tune.predelay as f64 / 48.0 - 81.0).abs() < 0.5);
        let cathedral = named("Cathedral");
        assert_eq!(
            cathedral.params[..9],
            [33, 21, 27, 90, 100, 45, 31, 150, 298]
        );
        assert!((cathedral.tune.predelay as f64 / 48.0 - 70.0).abs() < 0.5);
        assert_eq!(named("Mastering for strings").params, cathedral.params);
        let studio = named("Small Studio");
        assert_eq!(
            (studio.params[3], studio.params[5], studio.params[7]),
            (17, 5, 90)
        );
        assert_eq!(named("Drum Room").params[..6], [100, 36, 0, 35, 100, 6]);
    }
}
