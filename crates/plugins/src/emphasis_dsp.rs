//! The building blocks of Emphasis's limiter, the classes of
//! `Shared/ildsp_x64.dll` that `docs/plugins/emphasis.md` maps: one-pole
//! filters, exponential smoothers, cascaded peak holds, the gain stages
//! made of them, the delay lines, the polyphase IIR halfband filters of the
//! oversampling and the windowed-sinc FIR of the true-peak detector.
//!
//! Every class keeps its state in `f32` as the DLL does, and works out its
//! coefficients the way the DLL does, in `f64` where it calls `exp` and in
//! `f32` where it calls `expf`.

use std::f64::consts::{PI, TAU};

/// The smallest level the stages divide by: `0x1803e0230`.
pub(crate) const EPS: f32 = 1.19e-7;

/// `exp(-2 pi / (rate * time))`, the coefficient of a one-pole smoother
/// with this time constant, worked out in doubles.
pub(crate) fn pole(inv_rate: f32, time: f32) -> f32 {
    ((f64::from(inv_rate) * -TAU) / f64::from(time)).exp() as f32
}

/// `exp(-2 pi f / rate)`, the coefficient of a one-pole filter at `f` Hz.
pub(crate) fn pole_hz(inv_rate: f32, hz: f32) -> f32 {
    (f64::from(hz) * -TAU * f64::from(inv_rate)).exp() as f32
}

/// `Lowpass1`: `y = (y - x) c + x`.
#[derive(Debug, Clone, Default)]
pub(crate) struct Lowpass1 {
    pub(crate) coef: f32,
    pub(crate) state: f32,
}

impl Lowpass1 {
    pub(crate) fn tune(&mut self, inv_rate: f32, hz: f32) {
        self.coef = pole_hz(inv_rate, hz);
    }

    pub(crate) fn step(&mut self, x: f32) -> f32 {
        self.state = (self.state - x) * self.coef + x;
        self.state
    }

    pub(crate) fn run(&mut self, buf: &mut [f32]) {
        for v in buf {
            *v = self.step(*v);
        }
    }
}

/// `Highpass1`: the input less a `Lowpass1` of it.
#[derive(Debug, Clone, Default)]
pub(crate) struct Highpass1(pub(crate) Lowpass1);

impl Highpass1 {
    pub(crate) fn run(&mut self, buf: &mut [f32]) {
        for v in buf {
            *v -= self.0.step(*v);
        }
    }
}

/// `ExpSmoother1`: a one-pole smoother with one coefficient for a rising
/// input (`attack`) and one for a falling input (`release`).
#[derive(Debug, Clone, Default)]
pub(crate) struct Smoother1 {
    pub(crate) release: f32,
    pub(crate) attack: f32,
    pub(crate) state: f32,
}

impl Smoother1 {
    pub(crate) fn step(&mut self, x: f32) -> f32 {
        let c = if self.state < x {
            self.attack
        } else {
            self.release
        };
        self.state = (self.state - x) * c + x;
        self.state
    }
}

/// `ExpSmoother` and `ExpSmoother8`: `stages` of [`Smoother1`] in a row,
/// all on the same two coefficients. `step` is `2 pi k / rate`, where `k`
/// is `1 / sqrt(2^(1/stages) - 1)`, so that the row reaches its time as
/// one stage would.
#[derive(Debug, Clone)]
pub(crate) struct SmootherN {
    pub(crate) release: f32,
    pub(crate) attack: f32,
    pub(crate) states: [f32; 8],
    pub(crate) stages: usize,
    pub(crate) step: f32,
    pub(crate) attack_time: f32,
    pub(crate) release_time: f32,
}

impl SmootherN {
    pub(crate) fn new(stages: usize, attack_time: f32, release_time: f32) -> Self {
        SmootherN {
            release: 0.0,
            attack: 0.0,
            states: [0.0; 8],
            stages,
            step: 0.0,
            attack_time,
            release_time,
        }
    }

    /// `FUN_18016e820` for `ExpSmoother` (any number of stages) and the
    /// setup of `ExpSmoother8` (8 stages, `k` the constant 20.885 / 2 pi).
    pub(crate) fn tune(&mut self, inv_rate: f32, stages: usize) {
        self.stages = stages;
        let k = if stages == 8 {
            // 0x1803e0748, 2 pi / sqrt(2^(1/8) - 1).
            self.step = (f64::from(inv_rate) * 20.885_122_438_709_02) as f32;
            return self.coefs();
        } else {
            let p = 2f32.powf(1.0 / stages as f32);
            1.0 / (p - 1.0).sqrt()
        };
        self.step = (f64::from(inv_rate) * TAU * f64::from(k)) as f32;
        self.coefs();
    }

    pub(crate) fn coefs(&mut self) {
        self.attack = (-(self.step / self.attack_time)).exp();
        self.release = (-(self.step / self.release_time)).exp();
    }

    pub(crate) fn step(&mut self, mut x: f32) -> f32 {
        for state in &mut self.states[..self.stages] {
            let c = if *state < x {
                self.attack
            } else {
                self.release
            };
            x = (*state - x) * c + x;
            *state = x;
        }
        x
    }
}

/// `PeakHold8`: eight holds in a row, each keeping a peak for up to
/// `hold` samples, an eighth of the whole time.
#[derive(Debug, Clone, Default)]
pub(crate) struct PeakHold8 {
    pub(crate) hold: f32,
    counts: [u64; 8],
    held: [f32; 8],
}

impl PeakHold8 {
    /// `hold = max(time, 0) * 0.125 * rate`.
    pub(crate) fn tune(&mut self, rate: f32, time: f32) {
        self.hold = time.max(0.0) * 0.125 * rate;
    }

    pub(crate) fn step(&mut self, mut x: f32) -> f32 {
        for (count, held) in self.counts.iter_mut().zip(&mut self.held) {
            if *held <= x || self.hold <= *count as f32 {
                *count = 0;
            } else {
                *count += 1;
                x = *held;
            }
            *held = x;
        }
        x
    }

    fn clear(&mut self) {
        self.counts = [0; 8];
        self.held = [0.0; 8];
    }
}

/// The smoother of a [`GrStatic`]: eight stages (`GRStatic8`) or one
/// (`GRStatic1`).
#[derive(Debug, Clone)]
pub(crate) enum GrSmoother {
    Eight(SmootherN),
    One(Smoother1),
}

/// `GRStatic8` and `GRStatic1`: a gain stage. The level runs through a
/// [`PeakHold8`], is kept at 1 or above, is smoothed, and the gain is its
/// inverse. Beside that it counts the share of the block whose held level
/// is over 1.0019 (`0x1803e0688`), which Emphasis's program-dependent
/// modes read.
#[derive(Debug, Clone)]
pub(crate) struct GrStatic {
    pub(crate) rate: f32,
    pub(crate) inv_rate: f32,
    pub(crate) hold: PeakHold8,
    pub(crate) smoother: GrSmoother,
    pub(crate) attack_time: f32,
    pub(crate) hold_time: f32,
    pub(crate) release_time: f32,
    meter_hold: f32,
    meter_count: u64,
    meter_held: f32,
    /// The share of the last block over the line.
    pub(crate) over: f32,
}

impl GrStatic {
    /// As the constructors `FUN_18016be60` and `FUN_18016c050` leave it.
    pub(crate) fn new(eight: bool) -> Self {
        let smoother = if eight {
            GrSmoother::Eight(SmootherN::new(8, 0.01, 0.1))
        } else {
            GrSmoother::One(Smoother1::default())
        };
        GrStatic {
            rate: 0.0,
            inv_rate: 0.0,
            hold: PeakHold8::default(),
            smoother,
            attack_time: 0.01,
            hold_time: 0.05,
            release_time: 0.1,
            meter_hold: 0.0,
            meter_count: 0,
            meter_held: 0.0,
            over: 0.0,
        }
    }

    /// The setup at a rate (`FUN_18016fed0`, `FUN_18016f760`): the
    /// coefficients from the times.
    pub(crate) fn set_rate(&mut self, rate: f32) {
        self.rate = rate;
        self.inv_rate = 1.0 / rate;
        self.meter_hold = rate * 0.025;
        self.retune();
    }

    pub(crate) fn retune(&mut self) {
        let inv = self.inv_rate;
        match &mut self.smoother {
            GrSmoother::Eight(s) => {
                s.tune(inv, 8);
                s.attack_time = self.attack_time;
                s.release_time = self.release_time;
                s.attack = (-(s.step / self.attack_time)).exp();
                s.release = (-(s.step / self.release_time)).exp();
            }
            GrSmoother::One(s) => {
                s.attack = pole(inv, self.attack_time);
                s.release = pole(inv, self.release_time);
            }
        }
        self.hold.tune(self.rate, self.hold_time);
    }

    /// The times all at once, as `FUN_18016a630` and the parameters set
    /// them.
    pub(crate) fn set_times(&mut self, attack: f32, hold: f32, release: f32) {
        self.attack_time = attack;
        self.hold_time = hold;
        self.release_time = release;
        self.retune();
    }

    /// Levels in, gains out, in place.
    pub(crate) fn run(&mut self, buf: &mut [f32]) {
        let mut over = 0.0f32;
        for v in buf.iter() {
            let x = *v;
            if self.meter_held <= x || self.meter_hold <= self.meter_count as f32 {
                self.meter_count = 0;
                self.meter_held = x;
            } else {
                self.meter_count += 1;
            }
            over += f32::from(u8::from(1.001_867_4 <= self.meter_held));
        }
        self.over = over / buf.len() as f32;
        for v in buf.iter_mut() {
            let level = self.hold.step(*v).max(1.0);
            let smooth = match &mut self.smoother {
                GrSmoother::Eight(s) => s.step(level),
                GrSmoother::One(s) => s.step(level),
            };
            *v = (1.0 / smooth).min(1.0);
        }
    }

    pub(crate) fn clear(&mut self) {
        self.hold.clear();
        match &mut self.smoother {
            GrSmoother::Eight(s) => s.states = [0.0; 8],
            GrSmoother::One(s) => s.state = 0.0,
        }
        self.meter_count = 0;
        self.meter_held = 0.0;
    }
}

/// `GRAdaptiveRel`: the slow stage of Emphasis. The level is held over
/// 50 ms, smoothed by a row of `stages` smoothers whose release follows
/// the program, and the gain is the inverse.
#[derive(Debug, Clone)]
pub(crate) struct GrAdaptive {
    pub(crate) inv_rate: f32,
    pub(crate) hold: PeakHold8,
    /// `+0xf0`, for two stages and more.
    pub(crate) many: SmootherN,
    /// `+0x178`, for one stage.
    pub(crate) one: Smoother1,
    /// `+0x1d0`, a 20 Hz `Lowpass1` on the level.
    pub(crate) level: Lowpass1,
    /// `+0x220`: the gain the last sample came to, and `+0x224` the gain
    /// of the first stage then.
    last: f32,
    first: f32,
    /// `+0x228`: the first stage's gain a sample before that.
    first_before: f32,
    /// `+0x22c` (`0.15 scale`) and `+0x230` (`0.6 scale^2`): the two
    /// release times.
    pub(crate) short: f32,
    pub(crate) long: f32,
    /// `+0x234`: the input gain in dB, and `+0x238` its inverse as a
    /// factor.
    pub(crate) gain_db: f32,
    pub(crate) inv_gain: f32,
    /// `+0x23c`: how much the release grows with the gain reduction.
    pub(crate) growth: f32,
    /// `+0x240`: instant recovery, a release of 5 ms.
    pub(crate) instant: bool,
    /// `+0x244`: the attack time.
    pub(crate) attack: f32,
    /// `+0x24c`: the scale of the release times.
    pub(crate) scale: f32,
}

impl GrAdaptive {
    /// As `FUN_18016bb50` leaves it.
    pub(crate) fn new() -> Self {
        GrAdaptive {
            inv_rate: 0.0,
            hold: PeakHold8::default(),
            many: SmootherN::new(1, 0.01, 0.0),
            one: Smoother1::default(),
            level: Lowpass1::default(),
            last: 0.0,
            first: 0.0,
            first_before: 0.0,
            short: 0.05,
            long: 0.25,
            gain_db: 0.0,
            inv_gain: 1.0,
            growth: 10.0,
            instant: false,
            attack: 0.01,
            scale: 1.0,
        }
    }

    /// `FUN_18016efe0`: the setup at a rate.
    pub(crate) fn prepare(&mut self, rate: f32) {
        self.inv_rate = 1.0 / rate;
        self.many.tune(self.inv_rate, 1);
        self.set_attack(self.attack);
        self.level.tune(self.inv_rate, 20.0);
        self.short = self.scale * 0.15;
        self.long = self.short * 4.0 * self.scale;
        // FUN_18016a630: a hold of 50 ms.
        self.hold.hold = rate * 0.006_25;
    }

    /// The attack time of both smoothers.
    pub(crate) fn set_attack(&mut self, time: f32) {
        self.attack = time;
        self.many.attack_time = time;
        self.many.attack = ((-1.0 / time) * self.many.step).exp();
        self.one.attack = pole(self.inv_rate, time);
    }

    /// The number of stages (`FUN_18016e820`), with every stage taking
    /// the first one's state.
    pub(crate) fn set_stages(&mut self, stages: usize) {
        self.many.attack_time = self.attack;
        self.many.tune(self.inv_rate, stages);
        let first = self.many.states[0];
        for state in &mut self.many.states[1..] {
            *state = first;
        }
    }

    /// The release for this sample (the shared middle of both branches of
    /// `FUN_18016f1c0`).
    fn release(&mut self, level: f32, before: f32, prev: f32) -> f32 {
        if self.instant {
            return 0.005;
        }
        let x = self.inv_gain * level;
        let smooth = self.level.step(x).max(EPS);
        let gain = self.last.max(EPS);
        let over = (smooth / gain).max(EPS).ln() * 8.685_889;
        let reduction = gain.ln() * 8.685_889;
        let g = self.gain_db;
        let stretch = if reduction < g * -0.33 || over < g * -0.5 {
            (reduction - g * -0.33).abs().sqrt() * self.growth + 1.0
        } else {
            1.0
        };
        let pick = 6.0 - g < over && before < prev;
        let base = if pick { self.short } else { self.long };
        base * stretch
    }

    /// Levels in, gains out, in place.
    pub(crate) fn run(&mut self, buf: &mut [f32]) {
        for v in buf.iter_mut() {
            *v = self.hold.step(*v);
        }
        let neg_tau = f64::from(self.inv_rate) * -TAU;
        if self.many.stages == 1 {
            for v in buf.iter_mut() {
                let prev = self.first;
                let release = self.release(*v, self.first_before, prev);
                let x = v.max(1.0);
                self.one.release = (neg_tau / f64::from(release)).exp() as f32;
                let state = self.one.step(x).max(EPS);
                let gain = (1.0 / state).min(1.0);
                *v = gain;
                self.last = gain;
                self.first_before = prev;
                self.first = gain;
            }
        } else {
            for v in buf.iter_mut() {
                let prev = self.first;
                let release = self.release(*v, self.first_before, prev);
                let x = v.max(1.0);
                self.many.release_time = release;
                self.many.release = (-(self.many.step / release)).exp();
                let last = self.many.step(x).max(EPS);
                let gain = (1.0 / last).min(1.0);
                *v = gain;
                self.last = gain;
                self.first_before = prev;
                self.first = (1.0 / self.many.states[0].max(EPS)).min(1.0);
            }
        }
    }

    pub(crate) fn clear(&mut self) {
        self.hold.clear();
        self.many.states = [0.0; 8];
        self.one.state = 0.0;
        self.level.state = 0.0;
        self.last = 0.0;
        self.first = 0.0;
        self.first_before = 0.0;
    }
}

/// `DelayInt`: a delay of a whole number of samples.
#[derive(Debug, Clone, Default)]
pub(crate) struct DelayInt {
    buf: Vec<f32>,
    write: usize,
    pub(crate) delay: usize,
}

impl DelayInt {
    pub(crate) fn new(len: usize) -> Self {
        DelayInt {
            buf: vec![0.0; len.next_power_of_two()],
            write: 0,
            delay: 0,
        }
    }

    /// `trunc(max(0, rate * time))` samples.
    pub(crate) fn set_time(&mut self, rate: f32, time: f32) {
        self.delay = (rate * time).max(0.0) as usize;
    }

    pub(crate) fn run(&mut self, input: &[f32], out: &mut [f32]) {
        let mask = self.buf.len() - 1;
        for (o, i) in out.iter_mut().zip(input) {
            self.buf[self.write] = *i;
            *o = self.buf[self.write.wrapping_sub(self.delay) & mask];
            self.write = (self.write + 1) & mask;
        }
    }

    pub(crate) fn run_in_place(&mut self, buf: &mut [f32]) {
        let mask = self.buf.len() - 1;
        for v in buf {
            self.buf[self.write] = *v;
            *v = self.buf[self.write.wrapping_sub(self.delay) & mask];
            self.write = (self.write + 1) & mask;
        }
    }

    pub(crate) fn clear(&mut self) {
        self.buf.fill(0.0);
        self.write = 0;
    }
}

/// `DelayLerp`: a delay that glides to its length at 5 Hz and reads
/// between samples.
#[derive(Debug, Clone, Default)]
pub(crate) struct DelayLerp {
    buf: Vec<f32>,
    write: usize,
    /// `+0x100040`, the length in seconds.
    pub(crate) time: f32,
    /// `+0x100048`, the length it glides to, in samples.
    pub(crate) target: f32,
    /// `+0x100060`, where it is now.
    pub(crate) now: f32,
    glide: f32,
}

impl DelayLerp {
    pub(crate) fn new(len: usize) -> Self {
        DelayLerp {
            buf: vec![0.0; len.next_power_of_two()],
            ..DelayLerp::default()
        }
    }

    /// `FUN_18016d6f0`: the glide and the length from the time.
    pub(crate) fn set_rate(&mut self, rate: f32) {
        let inv = 1.0 / rate;
        self.glide = (f64::from(inv) * (-10.0 * PI)).exp() as f32;
        self.target = (rate * self.time).max(0.0) as u64 as f32;
    }

    /// The length in samples, and in seconds at `inv_rate` a sample.
    pub(crate) fn set_frames(&mut self, frames: f32, inv_rate: f32) {
        self.target = frames;
        self.time = frames * inv_rate;
    }

    pub(crate) fn run(&mut self, buf: &mut [f32]) {
        let mask = self.buf.len() - 1;
        let target = self.target;
        let near = target * 0.001;
        let whole = target.round();
        for v in buf {
            self.buf[self.write] = *v;
            self.now = (self.now - target) * self.glide + target;
            let at = if (self.now - target).abs() < near {
                whole
            } else {
                self.now
            };
            let floor = at as u64;
            let i = self.write.wrapping_sub(floor as usize) & mask;
            let a = self.buf[i];
            let b = self.buf[(i + 1) & mask];
            *v = (b - a) * (at - floor as f32) + a;
            self.write = (self.write + 1) & mask;
        }
    }

    pub(crate) fn clear(&mut self) {
        self.buf.fill(0.0);
        self.write = 0;
    }
}

/// One second-order allpass section in `z^-2` of a halfband branch.
#[derive(Debug, Clone, Copy, Default)]
struct Allpass {
    c: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Allpass {
    fn step(&mut self, x: f32) -> f32 {
        let y = (x - self.y2) * self.c + self.x2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// The coefficients of `FUN_180170630`, branch A and branch B, for 12, 6
/// and 4 sections.
const HALFBAND_12: ([f32; 6], [f32; 6]) = (
    [
        0.016_774_667,
        0.139_021_49,
        0.332_501_1,
        0.537_661_1,
        0.721_418_4,
        0.882_185_8,
    ],
    [
        0.065_013_19,
        0.230_941_3,
        0.436_494_24,
        0.632_961,
        0.803_780_85,
        0.959_968_75,
    ],
);
const HALFBAND_6: ([f32; 3], [f32; 3]) = (
    [0.060_297_392, 0.412_590_7, 0.772_715_6],
    [0.215_971_44, 0.604_358_6, 0.923_886_1],
);
const HALFBAND_4: ([f32; 2], [f32; 2]) =
    ([0.079_866_424, 0.545_353_65], [0.283_829_33, 0.834_411_9]);

/// A polyphase IIR halfband filter (`FUN_180170aa0`): `(A(x[n]) +
/// B(x[n - 1])) / 2`.
#[derive(Debug, Clone)]
pub(crate) struct Halfband {
    a: Vec<Allpass>,
    b: Vec<Allpass>,
    prev: f32,
}

impl Halfband {
    /// The one with this many sections: 12, 6 or 4.
    fn new(order: usize) -> Self {
        let make = |c: &[f32]| -> Vec<Allpass> {
            c.iter()
                .map(|c| Allpass {
                    c: *c,
                    ..Allpass::default()
                })
                .collect()
        };
        let (a, b) = match order {
            12 => (make(&HALFBAND_12.0), make(&HALFBAND_12.1)),
            6 => (make(&HALFBAND_6.0), make(&HALFBAND_6.1)),
            _ => (make(&HALFBAND_4.0), make(&HALFBAND_4.1)),
        };
        Halfband { a, b, prev: 0.0 }
    }

    pub(crate) fn step(&mut self, x: f32) -> f32 {
        let mut a = x;
        for s in &mut self.a {
            a = s.step(a);
        }
        let out = (a + self.prev) * 0.5;
        let mut b = x;
        for s in &mut self.b {
            b = s.step(b);
        }
        self.prev = b;
        out
    }

    fn clear(&mut self) {
        for s in self.a.iter_mut().chain(self.b.iter_mut()) {
            *s = Allpass {
                c: s.c,
                ..Allpass::default()
            };
        }
    }
}

/// One oversampling factor's chain of halfbands: 12 sections for 2x, then
/// 6, 4 and 4 (`FUN_18016c690` to `FUN_18016cce0`).
#[derive(Debug, Clone)]
pub(crate) struct Chain(Vec<Halfband>);

impl Chain {
    /// The chain for 2, 4, 8 or 16 times.
    pub(crate) fn new(factor: usize) -> Self {
        let orders = [12, 6, 4, 4];
        let stages = factor.trailing_zeros() as usize;
        Chain(orders[..stages].iter().map(|o| Halfband::new(*o)).collect())
    }

    /// `FUN_18016ab60`: each sample, times the factor, then a zero after
    /// every sample at each stage. `out` is `factor` times as long.
    pub(crate) fn up(&mut self, input: &[f32], out: &mut [f32]) {
        let factor = 1 << self.0.len();
        let mut work = [0.0f32; 16];
        for (x, frame) in input.iter().zip(out.chunks_exact_mut(factor)) {
            work[0] = x * factor as f32;
            let mut len = 1;
            for stage in &mut self.0 {
                for k in (0..len).rev() {
                    work[2 * k] = work[k];
                }
                for k in 0..len {
                    let s = work[2 * k];
                    work[2 * k] = stage.step(s);
                    work[2 * k + 1] = stage.step(0.0);
                }
                len *= 2;
            }
            frame.copy_from_slice(&work[..factor]);
        }
    }

    /// `FUN_18016ac90`: the last stage first, each keeping the first of
    /// every two samples. `input` is `factor` times as long as `out`.
    pub(crate) fn down(&mut self, input: &[f32], out: &mut [f32]) {
        let factor = 1 << self.0.len();
        let mut work = [0.0f32; 16];
        for (frame, o) in input.chunks_exact(factor).zip(out.iter_mut()) {
            work[..factor].copy_from_slice(frame);
            let mut len = factor;
            for stage in self.0.iter_mut().rev() {
                len /= 2;
                for k in 0..len {
                    let kept = stage.step(work[2 * k]);
                    stage.step(work[2 * k + 1]);
                    work[k] = kept;
                }
            }
            *o = work[0];
        }
    }

    pub(crate) fn clear(&mut self) {
        for stage in &mut self.0 {
            stage.clear();
        }
    }
}

/// The true-peak detector (`FUN_180172570` on the 4x `FUN_180172220`
/// design): a 385-tap windowed sinc interpolates four samples for every
/// one, and the level is the largest of them.
#[derive(Debug, Clone)]
pub(crate) struct TruePeak {
    taps: Vec<f32>,
    acc: Vec<f32>,
    at: usize,
}

/// The factor `FUN_180172c80` leaves at `+0`.
const TP_FACTOR: usize = 4;

impl TruePeak {
    pub(crate) fn new() -> Self {
        let factor = TP_FACTOR;
        let len = factor * 96 + 1;
        let taps = (0..len)
            .map(|i| {
                let t = f64::from(((1.0 / factor as f64) * i as f64 - 48.0) as f32);
                let sinc = if t.abs() >= 1e-6 {
                    (t * PI).sin() / (t * PI)
                } else {
                    1.0
                };
                let window = 0f64.max((i as f64 / (len - 1) as f64 * PI).sin());
                (window * sinc) as f32
            })
            .collect();
        let ring = len + 2 * factor;
        let ring = ring - ring % factor;
        TruePeak {
            taps,
            acc: vec![0.0; ring],
            at: 0,
        }
    }

    pub(crate) fn run(&mut self, input: &[f32], out: &mut [f32]) {
        let ring = self.acc.len();
        let len = self.taps.len();
        for (x, o) in input.iter().zip(out.iter_mut()) {
            // The taps from `at` on, wrapping round the ring once.
            let first = (ring - self.at).min(len);
            let (head, tail) = self.taps.split_at(first);
            for (a, h) in self.acc[self.at..self.at + first].iter_mut().zip(head) {
                *a += x * h;
            }
            for (a, h) in self.acc[..tail.len()].iter_mut().zip(tail) {
                *a += x * h;
            }
            for k in 0..TP_FACTOR {
                self.acc[(self.at + len + k) % ring] = 0.0;
            }
            let mut peak = 0.0f32;
            for k in 0..TP_FACTOR {
                peak = peak.max(self.acc[self.at + k].abs());
            }
            *o = peak;
            self.at = (self.at + TP_FACTOR) % ring;
        }
    }

    pub(crate) fn clear(&mut self) {
        self.acc.fill(0.0);
        self.at = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::sine;

    #[test]
    fn a_lowpass1_has_the_dll_s_coefficient_and_a_highpass1_blocks_dc() {
        let mut lp = Lowpass1::default();
        lp.tune(1.0 / 48_000.0, 1000.0);
        // exp(-2 pi 1000 / 48000)
        assert!((lp.coef - (-TAU / 48.0).exp() as f32).abs() < 1e-7);
        let mut hp = Highpass1::default();
        hp.0.tune(1.0 / 48_000.0, 5.0);
        // A constant dies away.
        let mut dc = vec![1.0f32; 48_000];
        hp.run(&mut dc);
        // A one-pole in f32 stops short by up to 2^-24 / (1 - c).
        assert!(dc[47_999].abs() < 1e-4);
    }

    #[test]
    fn a_row_of_stages_reaches_its_time_as_one_stage_would() {
        // k = 1 / sqrt(2^(1/n) - 1): 1 for one stage, 1.554 for two.
        let mut one = SmootherN::new(1, 0.01, 0.1);
        one.tune(1.0 / 48_000.0, 1);
        assert!((one.step - (TAU / 48_000.0) as f32).abs() < 1e-9);
        let mut two = SmootherN::new(2, 0.01, 0.1);
        two.tune(1.0 / 48_000.0, 2);
        let k = 1.0 / (2f64.sqrt() - 1.0).sqrt();
        assert!((f64::from(two.step) - TAU * k / 48_000.0).abs() < 1e-7);
        // 8 stages use the DLL's 20.885 = 2 pi / sqrt(2^(1/8) - 1).
        let k8 = TAU / (2f64.powf(0.125) - 1.0).sqrt();
        assert!((k8 - 20.885_122_438_709_02).abs() < 1e-6);
    }

    #[test]
    fn a_peak_hold8_keeps_a_peak_for_its_time_and_lets_it_go() {
        let mut hold = PeakHold8::default();
        // 8 stages of 10 samples.
        hold.tune(800.0, 0.1);
        assert_eq!(hold.hold, 10.0);
        let mut out = Vec::new();
        out.push(hold.step(1.0));
        for _ in 0..100 {
            out.push(hold.step(0.0));
        }
        assert!(out[..80].iter().all(|v| *v == 1.0));
        assert!(out[90..].iter().all(|v| *v == 0.0));
    }

    #[test]
    fn a_gain_stage_gives_the_inverse_of_a_steady_level_over_1() {
        let mut stage = GrStatic::new(true);
        stage.set_rate(48_000.0);
        stage.set_times(0.004, 0.004, 0.004);
        let mut levels = vec![2.0f32; 4800];
        stage.run(&mut levels);
        assert!((levels[4799] - 0.5).abs() < 1e-4);
        // All of the block is over the line.
        assert_eq!(stage.over, 1.0);
        let mut quiet = vec![0.5f32; 48_000];
        stage.run(&mut quiet);
        assert!((quiet[47_999] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn the_halfband_chains_pass_a_low_tone_through_at_its_level() {
        for factor in [2, 4, 8, 16] {
            let mut up = Chain::new(factor);
            let mut down = Chain::new(factor);
            let tone = sine(0.01, 4000);
            let mut high = vec![0.0; 4000 * factor];
            up.up(&tone, &mut high);
            let mut back = vec![0.0; 4000];
            down.down(&high, &mut back);
            let peak = back[2000..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!((peak - 1.0).abs() < 0.01, "{factor}x: {peak}");
            // The oversampled tone is as loud as the tone.
            let peak = high[2000 * factor..]
                .iter()
                .fold(0.0f32, |m, v| m.max(v.abs()));
            assert!((peak - 1.0).abs() < 0.01, "{factor}x up: {peak}");
        }
    }

    #[test]
    fn the_true_peak_of_a_tone_between_its_samples_is_its_amplitude() {
        // A quarter-rate tone a eighth of a turn off its samples peaks at
        // 0.707 on them but at 1 between.
        let tone: Vec<f32> = (0..1000)
            .map(|n| (TAU * (0.25 * n as f64 + 0.125)).sin() as f32)
            .collect();
        let sample_peak = tone.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((sample_peak - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3);
        let mut tp = TruePeak::new();
        let mut level = vec![0.0; 1000];
        tp.run(&tone, &mut level);
        let peak = level[200..].iter().fold(0.0f32, |m, v| m.max(*v));
        assert!(peak > 0.97 && peak < 1.03, "{peak}");
    }

    #[test]
    fn a_delay_lerp_glides_to_its_length() {
        let mut delay = DelayLerp::new(1024);
        delay.set_rate(1000.0);
        delay.set_frames(100.0, 1.0 / 1000.0);
        // Run long enough to arrive.
        let mut warm = vec![0.0; 5000];
        delay.run(&mut warm);
        let mut click = vec![0.0; 200];
        click[0] = 1.0;
        delay.run(&mut click);
        assert_eq!(click[100], 1.0);
        assert_eq!(click.iter().sum::<f32>(), 1.0);
    }
}
