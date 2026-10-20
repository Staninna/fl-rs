//! Emphasis: FL's mastering limiter, as `docs/plugins/emphasis.md` has it.
//!
//! `Emphasis_x64.dll` is a shell: it keeps the parameters, turns the
//! wheels into the values of the limiter in `Shared/ildsp_x64.dll`
//! (`Limiter_CreateInstance`, a C++ class `Limiter`), and hands it the
//! sound. [`Engine`] is that class.
//!
//! The limiter has three gain stages in a row on each side. A slow one
//! with a release that follows the program (`GRAdaptiveRel`) works on the
//! linked level of the input after the input gain, its top pressed down by
//! the emphasis amount, behind a 50 ms lookahead. A fast brickwall stage
//! (`GRStatic8`) with the emphasis hardness as its times runs at the
//! oversampled rate. A last 4 ms stage, on the true peak when that is on,
//! keeps the output under the ceiling. In M/S mode an 8 ms stage on the
//! decoded sides sits before it. The modes Versatile and Loud move the
//! slow stage's attack and the amount block by block from the balance of
//! lows and highs in the input.
//!
//! The state is a version (10 now), the 13 parameters as i32, a meter
//! setting, from version 7 another, from 9 the low latency switch as a
//! byte and from 10 instant recovery and the DC blocker as two more.
//!
//! | parameter | range | to the limiter |
//! |---|---|---|
//! | 0 bypass | 0 to 1 | as is |
//! | 1 delta | 0 to 1 | as is |
//! | 2 envelope order | 1 to 4 | less 1: the slow stage has as many smoothers in a row, plus 1 |
//! | 3 true peak | 0 to 1 | as is |
//! | 4 channel mode, L/R or M/S | 0 to 1 | as is |
//! | 5 emphasis mode: Versatile, Loud, Transient, Steady | 0 to 3 | as is |
//! | 6 input gain | 0 to 10000 | `24 n` dB |
//! | 7 linking | 0 to 10000 | `n` |
//! | 8 emphasis amount | 0 to 10000 | `n` |
//! | 9 output ceiling | 0 to 10000 | `20 log10((11^n - 1) / 10)` dB, 0 at the top |
//! | 10 level matching | 0 to 1 | as is |
//! | 11 oversampling, off to 16x | 0 to 4 | as is: `2^v` times |
//! | 12 emphasis hardness | 0 to 9 | as is |
//!
//! `n` is the wheel's value over 10000.

use crate::emphasis_dsp::{
    Chain, DelayInt, DelayLerp, EPS, GrAdaptive, GrStatic, Highpass1, Lowpass1, Smoother1,
    TruePeak, pole,
};
use crate::{Effect, Fidelity, StateError, knob_value, state_version};

/// The name FL saves for the plugin in event 201.
pub const NAME: &str = "Emphasis";

/// The ranges of the 13 parameters.
const RANGES: [(i32, i32); 13] = [
    (0, 1),
    (0, 1),
    (1, 4),
    (0, 1),
    (0, 1),
    (0, 3),
    (0, 10_000),
    (0, 10_000),
    (0, 10_000),
    (0, 10_000),
    (0, 1),
    (0, 4),
    (0, 9),
];

/// FL's own "Default" preset.
const DEFAULTS: [i32; 13] = [0, 0, 1, 0, 0, 0, 0, 10_000, 10_000, 9_912, 0, 2, 7];

/// The three switches after the parameters: low latency, instant recovery
/// and the DC blocker, as the "Default" preset has them.
const DEFAULT_SWITCHES: [u8; 3] = [0, 1, 1];

/// The newest version FL reads.
const NEWEST: i32 = 10;

/// The longest block the limiter takes at once; a longer one is cut.
const MAX_BLOCK: usize = 512;

/// The limiter's parameter indexes for the three switches.
const LOW_LATENCY: u32 = 0x19;
const INSTANT: u32 = 0x1a;
const DC_BLOCK: u32 = 0x1b;

/// The length of a version's state.
fn length(version: i32) -> usize {
    match version {
        ..=6 => 60,
        7 | 8 => 64,
        9 => 65,
        _ => 67,
    }
}

/// `FUN_00706860`: the value the shell hands the limiter for a parameter
/// at this wheel value.
fn to_limiter(index: usize, raw: i32) -> f32 {
    // The wheels of 6 to 9 run from 0 to 10000.
    let n = (f64::from(raw) * 1e-4).clamp(0.0, 1.0) as f32;
    match index {
        2 => (raw - 1) as f32,
        6 => (24.0 * f64::from(n)) as f32,
        7 | 8 => n,
        9 => ceiling_db(n),
        _ => raw as f32,
    }
}

/// `FUN_00705110`: the ceiling wheel in dB, `-inf` at its bottom.
fn ceiling_db(n: f32) -> f32 {
    let gain = if n == 1.0 {
        1.0
    } else {
        // ln 11, worked out at startup into `.bss`.
        let ln11 = 11f64.ln() as f32;
        ((f64::from(n * ln11).exp() - 1.0) * f64::from(0.1f32)) as f32
    };
    if gain > 0.0 {
        // FUN_005dfeb0: Delphi's Log10, ln x times 1 / ln 10.
        (f64::from(gain).ln() * std::f64::consts::LOG10_E * 20.0) as f32
    } else {
        f32::NEG_INFINITY
    }
}

/// Two of a thing, one a side.
type Pair<T> = [T; 2];

fn pair<T>(make: impl Fn() -> T) -> Pair<T> {
    [make(), make()]
}

/// The four chains of halfbands of one side, for 2, 4, 8 and 16 times.
#[derive(Debug, Clone)]
struct Chains([Chain; 4]);

impl Chains {
    fn new() -> Self {
        Chains([2, 4, 8, 16].map(Chain::new))
    }

    fn get(&mut self, factor: usize) -> &mut Chain {
        &mut self.0[factor.trailing_zeros() as usize - 1]
    }

    fn clear(&mut self) {
        for chain in &mut self.0 {
            chain.clear();
        }
    }
}

/// The parameters as the limiter holds them, at `0xa02278` on.
#[derive(Debug, Clone, Copy)]
struct Values {
    bypass: f32,
    delta: f32,
    order: f32,
    true_peak: f32,
    channels: f32,
    mode: f32,
    gain_db: f32,
    link: f32,
    amount: f32,
    ceiling_db: f32,
    level_match: f32,
    oversampling: f32,
    hardness: f32,
    low_latency: f32,
    instant: f32,
    dc: f32,
}

/// The parameters that take effect only when the limiter can rebuild, as
/// it last took them (`0xa022ec` on).
#[derive(Debug, Clone, Copy)]
struct Taken {
    delta: f32,
    true_peak: f32,
    channels: f32,
    low_latency: f32,
    instant: f32,
}

/// The `Limiter` of `ildsp_x64.dll`.
#[derive(Debug, Clone)]
pub(crate) struct Engine {
    rate: f32,
    inv_rate: f32,
    v: Values,
    taken: Taken,
    /// `0xa02320`: the oversampling factor.
    factor: f32,
    /// `0xa022b4`: whether the mode is Versatile or Loud.
    program: bool,
    /// `0xa022b8` and `0xa022bc`: the input gain and its inverse.
    gain: f32,
    inv_gain: f32,
    /// `0xa022c0`: the output gain.
    out_gain: f32,
    /// `0xa022cc`: the slow stage's attack.
    attack: f32,
    /// `0xa022d4` and `0xa022d8`: where the level is pressed down and how
    /// hard.
    knee: f32,
    slope: f32,
    slow: Pair<GrAdaptive>,
    fast: Pair<GrStatic>,
    sides: Pair<GrStatic>,
    last: Pair<GrStatic>,
    look_slow: Pair<DelayLerp>,
    look_fast: Pair<DelayLerp>,
    look_sides: Pair<DelayInt>,
    look_last: Pair<DelayInt>,
    dry: Pair<DelayInt>,
    match_in: Pair<Lowpass1>,
    match_out: Pair<Lowpass1>,
    true_peak: Pair<TruePeak>,
    up: Pair<Chains>,
    down: Pair<Chains>,
    dry_up: Pair<Chains>,
    dry_down: Pair<Chains>,
    bypass_fade: Lowpass1,
    gain_smooth: Lowpass1,
    out_smooth: Lowpass1,
    /// The low and high levels of each side, for the program modes.
    lows: Pair<Lowpass1>,
    highs: Pair<Lowpass1>,
    split: Pair<Lowpass1>,
    target: Lowpass1,
    program_smooth: Smoother1,
    dc: Pair<Highpass1>,
    /// The fade out and back in around a rebuild.
    fade_a: Lowpass1,
    fade_b: Lowpass1,
    fade_on: bool,
    fade_count: u64,
    ready: bool,
    dirty: bool,
    work: Work,
}

/// The block buffers (`+0x40` to `+0xd8`).
#[derive(Debug, Clone)]
struct Work {
    level: Pair<Vec<f32>>,
    high_level: Pair<Vec<f32>>,
    high: Pair<Vec<f32>>,
    dry: Pair<Vec<f32>>,
    input: Pair<Vec<f32>>,
    output: Pair<Vec<f32>>,
    gain: Vec<f32>,
}

impl Work {
    fn new() -> Self {
        Work {
            level: pair(|| vec![0.0; MAX_BLOCK]),
            high_level: pair(|| vec![0.0; MAX_BLOCK * 16]),
            high: pair(|| vec![0.0; MAX_BLOCK * 16]),
            dry: pair(|| vec![0.0; MAX_BLOCK]),
            input: pair(|| vec![0.0; MAX_BLOCK]),
            output: pair(|| vec![0.0; MAX_BLOCK]),
            gain: vec![0.0; MAX_BLOCK],
        }
    }
}

impl Engine {
    /// The constructor `FUN_18016adc0`, the setup `FUN_180166970` at
    /// `rate`, `FUN_18016a630` and the parameters set again.
    pub(crate) fn new(rate: f32) -> Self {
        let frames = |seconds: f32| (rate * seconds) as usize + 16;
        let mut engine = Engine {
            rate,
            inv_rate: 1.0 / rate,
            // The constructor's calls of setParam.
            v: Values {
                bypass: 0.0,
                delta: 0.0,
                order: 0.0,
                true_peak: 0.0,
                channels: 0.0,
                mode: 0.0,
                gain_db: 0.0,
                link: 1.0,
                amount: 1.0,
                ceiling_db: 0.0,
                level_match: 0.0,
                oversampling: 2.0,
                hardness: 7.0,
                low_latency: 0.0,
                instant: 1.0,
                dc: 1.0,
            },
            taken: Taken {
                delta: 0.0,
                true_peak: 0.0,
                channels: 0.0,
                low_latency: 0.0,
                instant: 1.0,
            },
            factor: 4.0,
            program: true,
            gain: 1.0,
            inv_gain: 1.0,
            out_gain: 0.999_668_5,
            attack: 0.0,
            knee: 1.0,
            slope: 0.0,
            slow: pair(GrAdaptive::new),
            fast: pair(|| GrStatic::new(true)),
            sides: pair(|| GrStatic::new(false)),
            last: pair(|| GrStatic::new(true)),
            look_slow: pair(|| DelayLerp::new(frames(0.06))),
            look_fast: pair(|| DelayLerp::new(frames(0.52))),
            look_sides: pair(|| DelayInt::new(frames(0.04))),
            look_last: pair(|| DelayInt::new(frames(0.01))),
            dry: pair(|| DelayInt::new(frames(0.1))),
            match_in: pair(Lowpass1::default),
            match_out: pair(Lowpass1::default),
            true_peak: pair(TruePeak::new),
            up: pair(Chains::new),
            down: pair(Chains::new),
            dry_up: pair(Chains::new),
            dry_down: pair(Chains::new),
            bypass_fade: Lowpass1::default(),
            gain_smooth: Lowpass1::default(),
            out_smooth: Lowpass1::default(),
            lows: pair(Lowpass1::default),
            highs: pair(Lowpass1::default),
            split: pair(Lowpass1::default),
            target: Lowpass1::default(),
            program_smooth: Smoother1::default(),
            dc: pair(Highpass1::default),
            fade_a: Lowpass1::default(),
            fade_b: Lowpass1::default(),
            fade_on: true,
            fade_count: 0,
            ready: true,
            dirty: false,
            work: Work::new(),
        };
        engine.prepare();
        engine
    }

    /// `FUN_180166970` and `FUN_18016a630`, then `FUN_18016a400`.
    fn prepare(&mut self) {
        let rate = self.rate;
        let inv = self.inv_rate;
        let factor = self.factor.max(1.0) as u64 as f32;
        for ch in 0..2 {
            self.slow[ch].prepare(rate);
            self.fast[ch].set_rate(factor * rate);
            self.sides[ch].set_rate(rate);
            self.last[ch].set_rate(rate);
            self.look_slow[ch].set_rate(rate);
            self.look_fast[ch].set_rate(factor * rate);
            // FUN_18016a630.
            let ahead = (rate * 0.05).round();
            self.look_slow[ch].set_frames(ahead, inv);
            self.sides[ch].set_times(0.008, 0.008, 0.008);
            self.look_sides[ch].set_time(rate, 0.008);
            self.last[ch].set_times(0.004, 0.004, 0.004);
            self.look_last[ch].set_time(rate, 0.004);
            self.match_in[ch].tune(inv, 0.125);
            self.match_out[ch].tune(inv, 0.125);
            self.lows[ch].tune(inv, 5.0);
            self.highs[ch].tune(inv, 5.0);
            self.split[ch].tune(inv, 320.0);
            self.dc[ch].0.tune(inv, 5.0);
        }
        self.fade_a.tune(inv, 100.0);
        self.fade_b.tune(inv, 100.0);
        self.bypass_fade.tune(inv, 20.0);
        self.gain_smooth.tune(inv, 20.0);
        self.out_smooth.tune(inv, 20.0);
        self.target.tune(inv, 1000.0);
        let c = pole(inv, 0.05);
        self.program_smooth.attack = c;
        self.program_smooth.release = c;
        self.reapply();
    }

    /// `FUN_18016a400`: every parameter set again.
    fn reapply(&mut self) {
        let v = self.v;
        let order = [
            (0, v.bypass),
            (1, v.delta),
            (2, v.order),
            (3, v.true_peak),
            (4, v.channels),
            (5, v.mode),
            (6, v.gain_db),
            (7, v.link),
            (8, v.amount),
            (9, v.ceiling_db),
            (10, v.level_match),
            (11, v.oversampling),
            (12, v.hardness),
            (LOW_LATENCY, v.low_latency),
            (DC_BLOCK, v.dc),
            (INSTANT, v.instant),
        ];
        for (index, x) in order {
            self.set(index, x);
        }
    }

    /// The whole limiter as it is after a long run: the fade in done and
    /// the lookahead at its length. FL's mixer runs its plugins while the
    /// song is stopped, so they are so when a song plays.
    pub(crate) fn settle(&mut self) {
        self.fade_a.state = 1.0;
        self.fade_b.state = 1.0;
        self.fade_count = u64::MAX / 2;
        self.ready = false;
        let ms = 0.5 < self.taken.channels;
        self.gain_smooth.state = if ms { self.gain * 0.5 } else { self.gain };
        self.out_smooth.state = self.out_gain;
        self.bypass_fade.state = f32::from(u8::from(0.5 < self.v.bypass));
        for ch in 0..2 {
            self.look_slow[ch].now = self.look_slow[ch].target;
            self.look_fast[ch].now = self.look_fast[ch].target;
        }
    }

    /// `FUN_18016a560`: how many samples late the output is.
    pub(crate) fn latency(&self) -> usize {
        let mut late = 0;
        if self.taken.low_latency == 0.0 || self.taken.instant == 1.0 {
            late = self.look_slow[0].target as u64;
        }
        let factor = (self.factor as u64).max(1);
        late += self.look_fast[0].target as u64 / factor;
        if self.taken.channels == 1.0 {
            late += self.look_sides[0].delay as u64;
        }
        late as usize + self.look_last[0].delay
    }

    /// The tail of the cases that change the latency: the dry path is
    /// delayed to match, and the fade is let back in.
    fn relatch(&mut self) {
        let late = self.latency();
        for dry in &mut self.dry {
            dry.delay = late;
        }
        self.fade_on = true;
    }

    /// A case that can only take effect when the output is faded out: it
    /// marks the limiter to be rebuilt and fades out.
    fn not_ready(&mut self) {
        self.dirty = true;
        self.fade_on = false;
    }

    /// `FUN_180169150`, the limiter's setParam.
    pub(crate) fn set(&mut self, index: u32, x: f32) {
        let ready = self.ready;
        match index {
            0 => self.v.bypass = x,
            1 => {
                self.v.delta = x.round();
                if !ready {
                    return self.not_ready();
                }
                self.taken.delta = self.v.delta;
                self.set(9, self.v.ceiling_db);
                self.fade_on = true;
            }
            2 => {
                self.v.order = x.round();
                let stages = self.v.order as u64 as usize + 1;
                for slow in &mut self.slow {
                    slow.set_stages(stages);
                }
                self.set(8, self.v.amount);
                self.set(INSTANT, self.v.instant);
            }
            3 => {
                self.v.true_peak = x;
                if !ready {
                    return self.not_ready();
                }
                self.taken.true_peak = x;
                self.relatch();
            }
            4 => {
                self.v.channels = x;
                if !ready {
                    return self.not_ready();
                }
                self.taken.channels = x;
                let time = if x <= 0.5 {
                    0.008
                } else {
                    side_time(self.v.hardness)
                };
                for ch in 0..2 {
                    self.sides[ch].set_times(time, time, time);
                    self.look_sides[ch].set_time(self.rate, time);
                }
                self.relatch();
            }
            5 => {
                self.v.mode = x.round();
                self.program = self.v.mode <= 1.0;
                self.set(8, self.v.amount);
                self.set(6, self.v.gain_db);
            }
            6 => {
                self.v.gain_db = x;
                self.gain = (x * 0.115_129_25).exp();
                self.inv_gain = 1.0 / self.gain;
                let inv = (x * -0.115_129_25).exp();
                for slow in &mut self.slow {
                    slow.gain_db = x;
                    slow.inv_gain = inv;
                }
                self.set(9, self.v.ceiling_db);
            }
            7 => self.v.link = x,
            8 => {
                self.v.amount = x;
                let (emphasis, attack, release) = match self.v.mode as i32 {
                    0 => (0.0, 1.0, 1.0),
                    1 => (x, 0.0, 1.0),
                    2 => (0.0, x, x),
                    _ => (x, 0.0, x),
                };
                self.set(0x16, emphasis);
                self.set(0x14, attack);
                self.set(0x13, release);
            }
            9 => {
                self.v.ceiling_db = x;
                self.out_gain = if self.v.delta <= 0.5 {
                    (x * 0.115_129_25).exp().min(0.999_668_5)
                } else if self.v.level_match < 0.5 {
                    self.inv_gain
                } else {
                    1.0
                };
            }
            10 => {
                self.v.level_match = x.round();
                self.set(9, self.v.ceiling_db);
            }
            11 => {
                self.v.oversampling = x.round();
                if !ready {
                    return self.not_ready();
                }
                let factor = (1u32 << (self.v.oversampling as u32 & 31)) as f32;
                self.factor = factor;
                let rate = (factor * self.rate).max(44_100.0);
                for ch in 0..2 {
                    self.fast[ch].set_rate(rate);
                    self.look_fast[ch].set_rate(rate);
                }
                self.relatch();
            }
            12 => self.set_hardness(x),
            0x13 => {
                let mut y = 1.0 - x;
                if self.v.mode != 0.0 && self.v.mode != 1.0 {
                    y *= y;
                }
                let scale = y * y * 2.5 + 1.0;
                for slow in &mut self.slow {
                    slow.scale = scale;
                    slow.short = scale * 0.15;
                    slow.long = slow.short * 4.0 * scale;
                }
            }
            0x14 => {
                let t = ((x * 0.8 + 0.2) * std::f32::consts::LN_10).exp();
                let stages = self.v.order as u64 as usize;
                let stretch = [1.0, 0.414_213_57, 0.259_921_04, 0.189_207_12];
                let s = stretch[stages.min(3)];
                let attack = (1.0 - t) * -0.111_111_11 * s * 10.0 + 0.05;
                self.attack = attack;
                for slow in &mut self.slow {
                    slow.set_attack(attack);
                }
            }
            0x16 => {
                let l = (x * 10.0 + 1.0).ln();
                self.slope = 1.0 - l * 0.417_032_3;
                self.knee = (self.gain - 1.0) * self.slope + 1.0;
            }
            LOW_LATENCY => {
                self.v.low_latency = x;
                if !ready {
                    return self.not_ready();
                }
                self.taken.low_latency = x;
                self.relatch();
            }
            INSTANT => {
                let on = f32::from(u8::from(0.5 < x));
                self.v.instant = on;
                if !ready {
                    return self.not_ready();
                }
                self.taken.instant = on;
                let growth = 10.0 / (self.v.order + 1.0);
                for slow in &mut self.slow {
                    slow.growth = growth;
                }
                self.set(8, self.v.amount);
                for slow in &mut self.slow {
                    slow.instant = on != 0.0;
                }
                self.relatch();
            }
            DC_BLOCK => self.v.dc = f32::from(u8::from(0.5 < x)),
            _ => {}
        }
    }

    /// Case 12: the hardness sets the times of the fast stage, the
    /// lookaheads around it and the M/S stage.
    fn set_hardness(&mut self, x: f32) {
        self.v.hardness = x.round();
        let time = 2f32.powf(-x) * 0.032;
        let rate = self.rate;
        let ahead = (rate * time).floor();
        let total = (rate * 0.05).floor();
        let side = time.min(0.008);
        let index = (x as u64 as usize).min(9);
        let attack = ATTACK_STRETCH[index] * time;
        let factor = self.factor;
        for ch in 0..2 {
            self.look_slow[ch].set_frames(total - ahead, self.inv_rate);
            self.fast[ch].set_times(attack, time, time);
            let fast_inv = self.fast[ch].inv_rate;
            self.look_fast[ch].set_frames(ahead * factor, fast_inv);
            self.sides[ch].set_times(side, side, side);
            self.look_sides[ch].set_time(rate, side);
        }
        self.relatch_without_fade();
    }

    /// Case 12's tail: the latency, but no fade in.
    fn relatch_without_fade(&mut self) {
        let late = self.latency();
        for dry in &mut self.dry {
            dry.delay = late;
        }
    }

    /// `FUN_180166f40`: the buffers, the delays and the halfbands
    /// emptied.
    pub(crate) fn clear(&mut self) {
        for ch in 0..2 {
            self.look_slow[ch].clear();
            self.look_fast[ch].clear();
            self.look_sides[ch].clear();
            self.look_last[ch].clear();
            self.dry[ch].clear();
            self.up[ch].clear();
            self.down[ch].clear();
            self.dry_up[ch].clear();
            self.dry_down[ch].clear();
        }
    }

    /// Everything the song left behind forgotten, as a new instance.
    pub(crate) fn forget(&mut self) {
        self.clear();
        for ch in 0..2 {
            self.slow[ch].clear();
            self.fast[ch].clear();
            self.sides[ch].clear();
            self.last[ch].clear();
            self.match_in[ch].state = 0.0;
            self.match_out[ch].state = 0.0;
            self.lows[ch].state = 0.0;
            self.highs[ch].state = 0.0;
            self.split[ch].state = 0.0;
            self.dc[ch].0.state = 0.0;
            self.true_peak[ch].clear();
        }
        self.target.state = 0.0;
        self.program_smooth.state = 0.0;
        self.gain_smooth.state = 0.0;
        self.out_smooth.state = 0.0;
        self.bypass_fade.state = 0.0;
    }

    /// The program modes' look at the input (the first part of
    /// `FUN_180167330`).
    fn follow(&mut self, left: &[f32], right: &[f32]) {
        let floor = if self.taken.instant != 0.0 { 0.05 } else { 0.5 };
        let steep = if self.v.mode == 0.0 { 10.0 } else { 40.0 };
        let both_over = 0.9 < self.fast[1].over && 0.9 < self.fast[0].over;
        let (mut smooth, mut held, mut least) = (0.0, 0.0, 0.0);
        for (l, r) in left.iter().zip(right) {
            let mut ratio = [0.0f32; 2];
            for (ch, x) in [*l, *r].into_iter().enumerate() {
                let low = self.split[ch].step(x);
                let high = x - low;
                let low = self.lows[ch].step(low * low);
                let high = self.highs[ch].step(high * high);
                ratio[ch] = (1.0 / high.sqrt().max(EPS)) * low.sqrt();
            }
            let curve = ratio.map(|r| tanh(steep * (1.0 - tanh(r))));
            least = curve[0].min(curve[1]);
            let pick = if both_over {
                (least.sqrt() * self.attack).max(floor)
            } else {
                self.attack
            };
            smooth = self.target.step(pick);
            let push = if both_over { least.max(1e-6) } else { 1.0 };
            held = self.program_smooth.step(push).max(0.0);
        }
        if self.v.mode == 0.0 {
            for slow in &mut self.slow {
                slow.set_attack(smooth);
            }
            let room = ((24.0 - self.v.gain_db) * 0.041_666_668).min(1.0);
            self.set(0x16, (least.sqrt() * room).sqrt());
        } else {
            self.set(0x16, held.sqrt().sqrt());
        }
    }

    /// `FUN_180167330` on one block of up to [`MAX_BLOCK`] samples, in
    /// place. It walks the two sides by index, as the DLL does.
    #[allow(clippy::needless_range_loop)]
    fn block(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len();
        let mut w = std::mem::replace(&mut self.work, Work::empty());
        if self.v.dc > 0.0 {
            self.dc[0].run(left);
            self.dc[1].run(right);
        }
        if 0.5 < self.program as u8 as f32 {
            self.follow(left, right);
        }
        let factor = (self.factor as usize).max(1);
        let high_n = factor * n;
        {
            let ins: [&[f32]; 2] = [left, right];
            for ch in 0..2 {
                self.dry[ch].run(ins[ch], &mut w.dry[ch][..n]);
                if factor > 1 && 0.0 < self.v.delta {
                    let high = &mut w.high[ch][..high_n];
                    self.dry_up[ch].get(factor).up(&w.dry[ch][..n], high);
                    self.dry_down[ch]
                        .get(factor)
                        .down(high, &mut w.dry[ch][..n]);
                }
                w.input[ch][..n].copy_from_slice(ins[ch]);
            }
        }
        // The input gain, on the sides or on mid and side.
        let gain = &mut w.gain[..n];
        if self.taken.channels <= 0.5 {
            gain.fill(self.gain);
            self.gain_smooth.run(gain);
            for (o, g) in left.iter_mut().zip(gain.iter()) {
                *o *= g;
            }
            for (o, g) in right.iter_mut().zip(gain.iter()) {
                *o *= g;
            }
        } else {
            gain.fill(self.gain * 0.5);
            self.gain_smooth.run(gain);
            for i in 0..n {
                let (l, r) = (left[i], right[i]);
                left[i] = (l + r) * gain[i];
                right[i] = (l - r) * gain[i];
            }
        }
        // The linked level, pressed down over the knee.
        let link = self.v.link;
        let (l0, l1) = w.level.split_at_mut(1);
        let (level0, level1) = (&mut l0[0][..n], &mut l1[0][..n]);
        for i in 0..n {
            let (a, b) = (left[i].abs(), right[i].abs());
            let most = if a < b { b } else { a };
            level0[i] = (most - a) * link + a;
            level1[i] = (most - b) * link + b;
        }
        let (knee, slope) = (self.knee, self.slope);
        for v in level0.iter_mut().chain(level1.iter_mut()) {
            if knee < *v {
                *v = (*v - knee) * slope + knee;
            }
        }
        // The slow stage and the fast one, each side on its own.
        let mut outs: [&mut [f32]; 2] = [left, right];
        for ch in 0..2 {
            let level = &mut w.level[ch][..n];
            let out = &mut *outs[ch];
            self.slow[ch].run(level);
            if self.taken.low_latency == 0.0 || self.taken.instant != 0.0 {
                self.look_slow[ch].run(out);
            }
            for (o, g) in out.iter_mut().zip(level.iter()) {
                *o *= g;
            }
            if factor < 2 {
                for (l, o) in level.iter_mut().zip(out.iter()) {
                    *l = o.abs();
                }
                self.fast[ch].run(level);
                self.look_fast[ch].run(out);
                for (o, g) in out.iter_mut().zip(level.iter()) {
                    *o *= g;
                }
            } else {
                let high = &mut w.high[ch][..high_n];
                let high_level = &mut w.high_level[ch][..high_n];
                self.up[ch].get(factor).up(out, high);
                for (l, h) in high_level.iter_mut().zip(high.iter()) {
                    *l = h.abs();
                }
                self.fast[ch].run(high_level);
                self.look_fast[ch].run(high);
                for (h, g) in high.iter_mut().zip(high_level.iter()) {
                    *h *= g;
                }
                self.down[ch].get(factor).down(high, out);
            }
        }
        if 0.5 < self.taken.channels {
            for i in 0..n {
                let (m, s) = (outs[0][i], outs[1][i]);
                outs[0][i] = m + s;
                outs[1][i] = m - s;
            }
        }
        // The M/S stage and the last one.
        for ch in 0..2 {
            let level = &mut w.level[ch][..n];
            let out = &mut *outs[ch];
            if self.taken.channels == 1.0 {
                for (l, o) in level.iter_mut().zip(out.iter()) {
                    *l = o.abs();
                }
                self.sides[ch].run(level);
                self.look_sides[ch].run_in_place(out);
                for (o, g) in out.iter_mut().zip(level.iter()) {
                    *o *= g;
                }
            }
            if (self.taken.true_peak - 1.0).abs() >= EPS {
                for (l, o) in level.iter_mut().zip(out.iter()) {
                    *l = o.abs();
                }
            } else {
                self.true_peak[ch].run(out, level);
            }
            self.last[ch].run(level);
            self.look_last[ch].run_in_place(out);
            for (o, g) in out.iter_mut().zip(level.iter()) {
                *o *= g;
            }
        }
        // The ceiling.
        let gain = &mut w.gain[..n];
        gain.fill(self.out_gain);
        self.out_smooth.run(gain);
        for out in outs.iter_mut() {
            for (o, g) in out.iter_mut().zip(gain.iter()) {
                *o *= g;
            }
        }
        let bypass = 0.5 < self.v.bypass;
        if 0.5 < self.v.level_match {
            for ch in 0..2 {
                let out = &mut *outs[ch];
                let input = &w.input[ch][..n];
                let output = &mut w.output[ch][..n];
                for i in 0..n {
                    let a = self.match_in[ch].step(input[i] * input[i]).sqrt();
                    let b = self.match_out[ch].step(out[i] * out[i]).sqrt();
                    output[i] = a / b;
                }
                for (o, ratio) in out.iter_mut().zip(output.iter()) {
                    // FL's own leaves a NaN where both are silent.
                    *o *= ratio.min(2.0);
                }
            }
        }
        if (self.taken.delta - 1.0).abs() < EPS {
            for ch in 0..2 {
                for (o, d) in outs[ch].iter_mut().zip(&w.dry[ch][..n]) {
                    *o = d - *o;
                }
            }
        }
        // The bypass crossfade, and the fade around a rebuild.
        let to = f32::from(u8::from(bypass));
        let wait = self.rate * 0.1;
        for i in 0..n {
            let f = self.bypass_fade.step(to);
            for ch in 0..2 {
                let o = outs[ch][i];
                outs[ch][i] = (w.dry[ch][i] - o) * f + o;
            }
            if !bypass {
                self.fade_count = if self.fade_on {
                    self.fade_count.saturating_add(1)
                } else {
                    0
                };
                let open = f32::from(u8::from(wait <= self.fade_count as f32));
                let g = self.fade_b.step(self.fade_a.step(open));
                outs[0][i] *= g;
                outs[1][i] *= g;
            }
        }
        self.work = w;
        self.ready = self.fade_b.state < EPS;
        if self.dirty && self.ready {
            self.clear();
            self.reapply();
            self.dirty = false;
        }
    }

    /// The limiter on a block of any length.
    pub(crate) fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        for (l, r) in left[..len]
            .chunks_mut(MAX_BLOCK)
            .zip(right[..len].chunks_mut(MAX_BLOCK))
        {
            self.block(l, r);
        }
    }
}

impl Work {
    /// A stand-in while the buffers are lent out; asks for no memory.
    fn empty() -> Self {
        Work {
            level: [Vec::new(), Vec::new()],
            high_level: [Vec::new(), Vec::new()],
            high: [Vec::new(), Vec::new()],
            dry: [Vec::new(), Vec::new()],
            input: [Vec::new(), Vec::new()],
            output: [Vec::new(), Vec::new()],
            gain: Vec::new(),
        }
    }
}

/// How much longer than its hold the fast stage's attack is, by hardness
/// (`0x1803da9f0`).
const ATTACK_STRETCH: [f32; 10] = [
    3.0, 2.0, 1.5, 1.25, 1.125, 1.0625, 1.03125, 1.015_62, 1.007_81, 1.003_91,
];

/// The M/S stage's time by hardness, from 2 on (`0x1803da9c8`).
fn side_time(hardness: f32) -> f32 {
    const TIMES: [f32; 10] = [
        0.032, 0.016, 0.008, 0.004, 0.002, 0.001, 0.0005, 0.000_25, 0.000_125, 0.0,
    ];
    TIMES[(hardness as u64 as usize).clamp(2, 9)]
}

/// The rational tanh of `FUN_180167330`: a Pade approximant.
fn tanh(x: f32) -> f32 {
    let x2 = x * x;
    ((((x2 + 378.0) * x2 + 17_325.0) * x2 + 135_135.0) * x)
        / (((x2 * 28.0 + 3150.0) * x2 + 62_370.0) * x2 + 135_135.0)
}

/// Emphasis in an effect slot.
#[derive(Debug, Clone)]
pub struct Emphasis {
    params: [i32; 13],
    switches: [u8; 3],
    hz: f32,
    engine: Box<Engine>,
}

impl Default for Emphasis {
    /// FL's "Default" preset at 44.1 kHz.
    fn default() -> Self {
        Emphasis::with(DEFAULTS, DEFAULT_SWITCHES, 44_100.0)
    }
}

impl Emphasis {
    fn with(params: [i32; 13], switches: [u8; 3], hz: f32) -> Self {
        let mut plugin = Emphasis {
            params,
            switches,
            hz,
            engine: Box::new(Engine::new(hz)),
        };
        plugin.load();
        plugin
    }

    /// A new limiter given the parameters as the shell's state loader
    /// (`FUN_00706090`) gives them: each in turn, oversampling, hardness
    /// and level matching again, then the three switches.
    fn load(&mut self) {
        let mut engine = Engine::new(self.hz);
        for (index, raw) in self.params.iter().enumerate() {
            engine.set(index as u32, to_limiter(index, *raw));
        }
        for index in [11, 12, 10] {
            engine.set(index as u32, to_limiter(index, self.params[index]));
        }
        let [low_latency, instant, dc] = self.switches.map(f32::from);
        engine.set(INSTANT, instant);
        engine.set(DC_BLOCK, dc);
        engine.set(LOW_LATENCY, low_latency);
        engine.settle();
        *self.engine = engine;
    }
}

impl Effect for Emphasis {
    /// Versions up to 10. Before version 8 the envelope order was saved
    /// one lower, and FL adds one.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let version = state_version(bytes, length(NEWEST))?;
        if version > NEWEST {
            return Err(StateError::Version { found: version });
        }
        let expected = length(version);
        if bytes.len() != expected {
            return Err(StateError::Length {
                expected,
                found: bytes.len(),
            });
        }
        let mut params = [0; 13];
        for (index, word) in bytes[4..56].as_chunks::<4>().0.iter().enumerate() {
            params[index] = i32::from_le_bytes(*word);
        }
        if version < 8 {
            params[2] = (params[2] + 1).clamp(1, 4);
        }
        for (index, value) in params.iter().enumerate() {
            let (min, max) = RANGES[index];
            if *value < min || *value > max {
                return Err(StateError::Range {
                    param: index,
                    value: *value,
                });
            }
        }
        let mut switches = [0; 3];
        if version > 8 {
            switches[0] = bytes[64];
        }
        if version > 9 {
            switches[1] = bytes[65];
            switches[2] = bytes[66];
        }
        self.params = params;
        self.switches = switches;
        self.load();
        Ok(())
    }

    /// A new limiter at the rate. A rate that is not a number above 0 is
    /// not taken.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz > 0.0 {
            self.hz = hz;
            self.load();
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.engine.process(left, right);
    }

    fn param_count(&self) -> usize {
        RANGES.len()
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |value| *value as f32)
    }

    /// As FL's wheel moves it: a parameter that needs a rebuild fades the
    /// output out, rebuilds and fades it back in.
    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(range) = RANGES.get(index) {
            let raw = knob_value(value, *range);
            self.params[index] = raw;
            self.engine.set(index as u32, to_limiter(index, raw));
        }
    }

    fn reset(&mut self) {
        self.engine.forget();
    }

    /// FL's mixer reads the parameters in blocks: the program modes
    /// follow the input block by block.
    fn fidelity(&self) -> Fidelity {
        if self.params[5] <= 1 {
            Fidelity::Approximate(
                "Emphasis's Versatile and Loud modes follow the input once a block, and FL's blocks are not ours",
            )
        } else {
            Fidelity::Exact
        }
    }

    fn latency(&self) -> usize {
        self.engine.latency()
    }

    /// The 50 ms lookahead, the M/S and the last stage's.
    fn max_latency(&self) -> usize {
        (self.hz * 0.05) as usize + (self.hz * 0.008) as usize + (self.hz * 0.004) as usize + 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{presets, projects, sine, state};

    const HZ: f32 = 44_100.0;

    /// A version 10 state.
    fn v10(params: [i32; 13], switches: [u8; 3]) -> Vec<u8> {
        let mut bytes = state(&[10]);
        bytes.extend(state(&params));
        bytes.extend(state(&[0, 1]));
        bytes.extend(switches);
        bytes
    }

    fn with(params: [i32; 13], switches: [u8; 3]) -> Emphasis {
        let mut plugin = Emphasis::default();
        plugin.set_sample_rate(HZ);
        plugin.set_state(&v10(params, switches)).unwrap();
        plugin
    }

    fn moved(moves: &[(usize, i32)]) -> [i32; 13] {
        let mut params = DEFAULTS;
        for (index, value) in moves {
            params[*index] = *value;
        }
        params
    }

    fn run(plugin: &mut Emphasis, left: &[f32], right: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = (left.to_vec(), right.to_vec());
        for (a, b) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
            plugin.process(a, b);
        }
        (l, r)
    }

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    #[test]
    fn the_wheels_turn_into_the_limiter_s_values() {
        // The input gain is 24 dB at the top.
        assert_eq!(to_limiter(6, 10_000), 24.0);
        assert_eq!(to_limiter(6, 625), 1.5);
        // The ceiling is 0 dB at the top and -inf at the bottom, on the
        // gain knobs' curve between.
        assert_eq!(to_limiter(9, 10_000), 0.0);
        assert_eq!(to_limiter(9, 0), f32::NEG_INFINITY);
        let at = |n: f64| 20.0 * (((n * 11f64.ln()).exp() - 1.0) / 10.0).log10();
        assert!((f64::from(to_limiter(9, 9308)) - at(0.9308)).abs() < 1e-3);
        // The envelope order is one less.
        assert_eq!(to_limiter(2, 4), 3.0);
    }

    #[test]
    fn a_state_of_each_version_is_read_and_others_are_not() {
        let mut plugin = Emphasis::default();
        let mut bytes = v10(DEFAULTS, [1, 0, 1]);
        plugin.set_state(&bytes).unwrap();
        assert_eq!(plugin.switches, [1, 0, 1]);
        // Version 8 has no switches, and they are off.
        bytes.truncate(64);
        bytes[0] = 8;
        plugin.set_state(&bytes).unwrap();
        assert_eq!(plugin.switches, [0, 0, 0]);
        // Version 7 saved the order one lower.
        bytes[0] = 7;
        bytes[12..16].copy_from_slice(&0i32.to_le_bytes());
        plugin.set_state(&bytes).unwrap();
        assert_eq!(plugin.param(2), 1.0);
        bytes[0] = 11;
        assert!(plugin.set_state(&bytes).is_err());
        bytes[0] = 8;
        bytes.push(0);
        assert!(plugin.set_state(&bytes).is_err());
    }

    #[test]
    fn a_loud_tone_comes_out_under_the_ceiling() {
        // A 1 kHz tone 12 dB over full scale.
        let tone: Vec<f32> = sine(1000.0 / 44_100.0, 44_100)
            .iter()
            .map(|v| v * 4.0)
            .collect();
        for mode in 0..4 {
            for os in [0, 2] {
                for ms in [0, 1] {
                    let params = moved(&[(5, mode), (11, os), (4, ms), (3, 1)]);
                    let mut plugin = with(params, DEFAULT_SWITCHES);
                    let (l, r) = run(&mut plugin, &tone, &tone);
                    let ceiling = 10f32.powf(to_limiter(9, 9912) / 20.0);
                    let tail = &l[22_050..];
                    assert!(
                        peak(tail) <= ceiling * 1.01,
                        "mode {mode} os {os} ms {ms}: {}",
                        peak(tail)
                    );
                    assert!(peak(tail) > ceiling * 0.5, "mode {mode} os {os}");
                    assert!(r.iter().all(|v| v.is_finite()));
                }
            }
        }
    }

    #[test]
    fn a_quiet_tone_comes_out_late_by_the_latency_and_as_loud() {
        // Transient mode at no amount leaves a quiet tone as it is.
        let params = moved(&[(5, 2), (8, 0), (11, 0), (9, 10_000)]);
        let mut plugin = with(params, [0, 0, 0]);
        let late = plugin.latency();
        // 50 ms and the last stage's 4 ms.
        assert_eq!(late, 2205 + 176);
        let mut click = vec![0.0f32; 8192];
        click[100] = 0.25;
        let (l, _) = run(&mut plugin, &click, &click);
        let at =
            l.iter().enumerate().fold(
                (0, 0.0f32),
                |m, (i, v)| if v.abs() > m.1 { (i, v.abs()) } else { m },
            );
        assert_eq!(at.0, 100 + late);
        assert!((at.1 - 0.25 * 0.999_668_5).abs() < 1e-3, "{}", at.1);
    }

    #[test]
    fn delta_gives_what_the_limiter_took_away() {
        let tone: Vec<f32> = sine(0.01, 8192).iter().map(|v| v * 0.1).collect();
        let params = moved(&[(1, 1), (5, 2), (8, 0), (11, 0), (9, 10_000)]);
        let mut plugin = with(params, [0, 0, 0]);
        let (l, _) = run(&mut plugin, &tone, &tone);
        // Nothing is taken from a quiet tone at 0 dB gain.
        assert!(peak(&l[4096..]) < 1e-3, "{}", peak(&l[4096..]));
    }

    #[test]
    fn bypass_passes_the_input_late_by_the_latency() {
        let tone = sine(0.01, 8192);
        // The DC blocker works on the bypassed sound too.
        let mut plugin = with(moved(&[(0, 1), (11, 0)]), [0, 1, 0]);
        let late = plugin.latency();
        let (l, _) = run(&mut plugin, &tone, &tone);
        for i in 6000..8192 {
            assert!((l[i] - tone[i - late]).abs() < 1e-4, "{i}");
        }
    }

    #[test]
    fn a_rebuild_fades_out_and_back_in() {
        let tone: Vec<f32> = sine(0.01, 44_100).iter().map(|v| v * 0.1).collect();
        let mut plugin = with(moved(&[(11, 0)]), DEFAULT_SWITCHES);
        let (mut l, mut r) = (tone.clone(), tone.clone());
        plugin.process(&mut l[..4096], &mut r[..4096]);
        plugin.set_param(11, 2.0);
        assert!(plugin.engine.dirty);
        plugin.process(&mut l[4096..], &mut r[4096..]);
        assert!(!plugin.engine.dirty);
        assert_eq!(plugin.engine.factor, 4.0);
        // Out within a block, out for the 100 ms FL waits and back after.
        assert!(peak(&l[5200..9600]) < 1e-6);
        assert!(peak(&l[40_000..]) > 0.05);
    }

    #[test]
    fn every_preset_and_every_state_in_fl_s_projects_loads_and_plays() {
        let Some(saved) = presets("Emphasis") else {
            return;
        };
        assert_eq!(saved.len(), 21);
        let root = crate::testing::corpus().unwrap();
        let saved = saved.into_iter().chain(
            projects(&root)
                .into_iter()
                .filter(|s| s.plugin.eq_ignore_ascii_case(NAME)),
        );
        let tone: Vec<f32> = sine(0.013, 8192).iter().map(|v| v * 2.0).collect();
        let mut count = 0;
        for saved in saved {
            let mut plugin = Emphasis::default();
            plugin.set_sample_rate(HZ);
            plugin.set_state(&saved.state).unwrap();
            let (l, r) = run(&mut plugin, &tone, &tone);
            assert!(
                l.iter().chain(&r).all(|v| v.is_finite() && v.abs() <= 1.0),
                "{}",
                saved.file
            );
            count += 1;
        }
        assert_eq!(count, 26);
    }
}
