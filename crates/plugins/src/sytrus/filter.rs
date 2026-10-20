//! Sytrus's filters as a voice runs them: the design of every type once a
//! control tick (`0x86ebe0`) and the kernels once a render call
//! (`0x870820`), with the glides of the state variable filter's and the
//! ladders' coefficients and gains, the quality switch's 2x rate and the
//! alternate switch. Section 13 of `docs/plugins/sytrus.md`.

use super::{Filter, STAGES, curve, power};
use crate::parts::glide;
use dsp::biquad::{Biquad64, Coefficients64};
use std::f64::consts::{FRAC_PI_2, LN_2, PI, TAU};

/// The types, as `+0x170` numbers them. 11 is a four pole ladder with
/// drive and 12 a phaser.
pub(super) const SVF: i32 = 0;
const LADDER_LOW: i32 = 1;
const LADDER_HIGH: i32 = 2;
const LOW: i32 = 3;
const BAND: i32 = 4;
const STOP: i32 = 5;
const HIGH: i32 = 6;
const ALL: i32 = 7;
const LOW_SHELF: i32 = 8;
const PEAK: i32 = 9;
const HIGH_SHELF: i32 = 10;
pub(super) const LADDER: i32 = 11;
pub(super) const PHASER: i32 = 12;

/// How the earlier stages of a detuned row are tuned up: their cutoff to
/// this power (`0x8fd2a0`).
const TUNING: [f64; 3] = [1.0, 0.5, 0.75];
/// The most phaser allpasses: three stages and five more, in pairs.
const ALLPASSES: usize = 16;

/// The state variable filter's coefficient by its cutoff, at the rate and
/// at twice it (`0x86ea60`, `plugin+0x61d0`): `2 sin(asin(x) 44100 / rate)`
/// for `x = (i / 2048 + 0.01) / 2`, to where it reaches 1.175.
#[derive(Debug, Clone, PartialEq, Default)]
pub(super) struct Tables {
    svf: [Vec<f32>; 2],
}

impl Tables {
    /// How many steps the table at the rate (0) or at twice it (1) has.
    #[cfg(test)]
    pub(super) fn len(&self, twice: usize) -> usize {
        self.svf[twice].len()
    }

    /// The tables at this rate. This allocates.
    pub(super) fn new(rate: f32) -> Self {
        let mut scale = 44_100.0 / f64::from(rate);
        let mut svf = [Vec::new(), Vec::new()];
        for table in &mut svf {
            for i in 0..4096 {
                let x = ((f64::from(i) * (1.0 / 2048.0) + 0.01) * 0.5) as f32;
                let x = x.min(1.0);
                let angle = f64::from(x).asin() as f32;
                let v = (f64::from(angle) * scale).sin() * 2.0;
                if v >= 1.175 {
                    table.push(1.175);
                    break;
                }
                table.push(v as f32);
            }
            scale *= 0.5;
        }
        Tables { svf }
    }
}

/// One stage of a row.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Stage {
    /// The state variable filter's and the ladders' two coefficients (the
    /// frequency, +8, and the damping or keep, +0xc): what the tick aims
    /// at and where they are (+0x58, +0x5c).
    aim: [f32; 2],
    now: [f32; 2],
    /// The state variable filter's low and band, or a ladder's low and
    /// speed.
    state: [f64; 2],
    biquad: Biquad64,
    coefficients: Coefficients64,
    /// The four pole ladder: its poles and last output, the pole's
    /// coefficient, the feedback, the make-up gain and the drive.
    poles: [f32; 5],
    pole: f32,
    feedback: f32,
    gain: f32,
    drive: f64,
}

impl Stage {
    fn rest() -> Self {
        Stage {
            aim: [0.5, 1.0],
            now: [0.5, 1.0],
            state: [0.0; 2],
            biquad: Biquad64::default(),
            coefficients: Coefficients64::IDENTITY,
            poles: [0.0; 5],
            pole: 0.0,
            feedback: 0.0,
            gain: 0.0,
            drive: 0.0,
        }
    }
}

/// A filter as one voice runs it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Run {
    stage: [Stage; STAGES],
    /// The phaser's allpasses, its coefficient, feedback and state.
    allpass: [f32; ALLPASSES],
    pairs: usize,
    coefficient: f32,
    feedback: f32,
    held: f32,
    /// The state variable filter's low, band and high gains: where they
    /// are and what the tick aims at.
    gains: [f32; 3],
    gain_aims: [f32; 3],
    /// The volume ahead of the filter.
    pub(super) volume: f32,
    pub(super) volume_aim: f32,
    /// The waveshaper's mix, and what the tick aims it at.
    pub(super) mix: f32,
    pub(super) mix_target: f32,
    /// The row runs at twice the rate (+0x188), and its scale (+0x184).
    twice: bool,
    scale: f32,
}

impl Run {
    pub(super) fn rest() -> Self {
        Run {
            stage: [Stage::rest(); STAGES],
            allpass: [0.0; ALLPASSES],
            pairs: 0,
            coefficient: 0.0,
            feedback: 0.0,
            held: 0.0,
            gains: [0.0; 3],
            gain_aims: [0.0; 3],
            volume: 0.0,
            volume_aim: 0.0,
            mix: 0.0,
            mix_target: 0.0,
            twice: false,
            scale: 1.0,
        }
    }

    /// Ready a row for a note (`0x86af20`): the stages at rest with the
    /// damping a state variable filter waits at (1) or a ladder's (0.1),
    /// the phaser's allpasses, and the 2x rate for the quality switch up
    /// to 176.4 kHz or for type 11 up to 88.2 kHz.
    pub(super) fn start(filter: &Filter, rate: f32) -> Self {
        let mut run = Run::rest();
        let damping = if matches!(filter.kind, LADDER_LOW | LADDER_HIGH) {
            0.1
        } else {
            1.0
        };
        for stage in &mut run.stage {
            stage.aim = [0.5, damping];
            stage.now = stage.aim;
        }
        let hz = rate.round() as i32;
        run.twice = (filter.quality && hz <= 176_399) || (filter.kind == LADDER && hz <= 88_199);
        run.scale = (44_100.0 / f64::from(rate)) as f32;
        if run.twice {
            run.scale *= 0.5;
        }
        let order = filter.order.max(0) as usize;
        run.pairs = if filter.alternate { order + 5 } else { order }.min(ALLPASSES / 2);
        run
    }

    /// Work the row out for a tick (`0x86ebe0`, and the gains of
    /// `0x870820`): `cutoff` and `resonance` with their articulators,
    /// `arts` the low (or gain, or drive), band and high articulators.
    /// `first` takes every coefficient and gain as it is.
    pub(super) fn tick(
        &mut self,
        filter: &Filter,
        (cutoff, resonance): (f32, f32),
        arts: [f32; 3],
        tables: &Tables,
        first: bool,
    ) {
        let stages = filter.stages;
        let last = stages.saturating_sub(1);
        let spread = if filter.kind == LADDER {
            (1.0 / (last as f64 / 4.0 + 1.0)) as f32
        } else {
            (1.0 / (last + 1) as f64) as f32
        };
        let tuning = TUNING[last.min(2)] as f32;
        let detune = filter.detune;
        let detuned = |x: f64| if detune { power(x, tuning) } else { x };
        let c = f64::from(cutoff);
        let mut r = f64::from(resonance);
        let alternate = filter.alternate;
        let scale = f64::from(self.scale);
        // A shelf's or a peak's gain, and the ladder's drive, take the low
        // slider with its articulator (`0x870820`).
        let low = filter.low as f32 * arts[0];
        match filter.kind {
            SVF => {
                for k in 0..3 {
                    let g = filter.levels[k] * arts[k];
                    self.gain_aims[k] = if last > 0 {
                        if g > 0.0 {
                            power(f64::from(g), spread) as f32
                        } else {
                            0.0
                        }
                    } else {
                        g
                    };
                }
                let mut fc = if c > 0.0 {
                    curve(26.0, c) * 0.039_599_999_785_423_28
                } else {
                    0.0
                };
                let table = &tables.svf[usize::from(self.twice)];
                for stage in self.stage[..stages].iter_mut().rev() {
                    let at = (fc * 2048.0).round_ties_even() as i64;
                    let at = at.clamp(0, table.len() as i64 - 1) as usize;
                    stage.aim[0] = table[at];
                    fc = detuned(fc);
                }
                let mut damping = if r <= 0.0 {
                    1.0
                } else if r < 1.0 {
                    1.0 - r * 0.800_000_011_920_929
                } else {
                    0.2
                };
                if alternate {
                    damping *= damping;
                }
                self.stage[0].aim[1] = damping as f32;
            }
            LADDER_LOW | LADDER_HIGH => {
                if alternate {
                    r *= (c + 1.0) * 0.5;
                }
                let mut fc = if c > 0.0 {
                    (curve(76.0, c) * 0.013_24).min(1.6)
                } else {
                    0.0
                };
                let mut q = if r <= 0.0 {
                    1.0
                } else if r < 1.0 {
                    curve(1001.0, f64::from(r as f32)) * 0.049 + 1.0
                } else {
                    50.0
                };
                if last > 0 {
                    q = power(q, spread);
                }
                let scale = scale * FRAC_PI_2;
                for stage in self.stage[..stages].iter_mut().rev() {
                    let cos = ((fc + 0.007) * scale).min(0.4 * PI).cos();
                    stage.aim[0] = (2.0 - cos * 2.0) as f32;
                    let fall = cos - 1.0;
                    let keep = (-fall * fall * fall).sqrt() * f64::from(std::f32::consts::SQRT_2);
                    let keep = (keep / (q * fall) + 1.0) as f32;
                    stage.aim[1] = if f64::from(keep) < 0.1 { 0.1 } else { keep };
                    fc = detuned(fc);
                }
            }
            LOW..=HIGH_SHELF => self.biquads(filter, c, r, low, (spread, scale)),
            LADDER => self.ladder(filter, c, r, low, (spread, scale)),
            PHASER => {
                let fc = if c > 0.0 {
                    curve(151.0, c) * 0.006_666_000_001_132_488
                } else {
                    0.0
                };
                let feedback = if r <= 0.0 {
                    0.0
                } else if r < 1.0 {
                    let x = ((r * 10.0) as f32 + 1.0) as f64;
                    f64::from((x.ln() / 11f64.ln()) as f32 * 0.9)
                } else {
                    0.9
                };
                let w = ((fc + 1e-4) * f64::from(self.scale)).min(1.0);
                self.feedback = feedback as f32;
                let a = ((1.0 - w) / (w + 1.0)) as f32;
                self.coefficient = if a.is_subnormal() { 0.0 } else { a };
            }
            _ => {}
        }
        if first {
            self.gains = self.gain_aims;
            for stage in &mut self.stage {
                stage.now = stage.aim;
            }
        }
    }

    /// Types 3 to 10: one of the cookbook's biquads a stage, by the
    /// frequency and the Q curves of each.
    fn biquads(
        &mut self,
        filter: &Filter,
        c: f64,
        mut r: f64,
        low: f32,
        (spread, scale): (f32, f64),
    ) {
        let kind = filter.kind;
        let stages = filter.stages;
        let last = stages.saturating_sub(1);
        let detune = filter.detune;
        let tuning = TUNING[last.min(2)] as f32;
        let detuned = |x: f64| if detune { power(x, tuning) } else { x };
        if filter.alternate {
            r *= (c + 1.0) * 0.5;
        }
        let (reach, offset, most) = match kind {
            LOW => (0.000_664_666_644_297_540_2, 0.003, f64::INFINITY),
            HIGH => (0.000_399_933_342_123_404_15, 1e-4, 0.6001),
            _ => (0.000_399_933_342_123_404_15, 1e-4, 1.0001),
        };
        let mut fc = if c > 0.0 {
            (curve(1501.0, c) * reach).min(most)
        } else {
            0.0
        };
        // A shelf's or a peak's gain: `(8^(low / 64) - 1) / 8 + 1 / 8`.
        let x = f64::from((f64::from(low) * 0.015_625) as f32);
        let mut a = f64::from((curve(8.0, x) * 0.125 + 0.125) as f32);
        if last > 0 && kind >= LOW_SHELF {
            a = power(a, spread);
        }
        let shape = match kind {
            BAND | STOP => {
                if r <= 0.0 {
                    0.01
                } else if r < 1.0 {
                    curve(101.0, f64::from(r as f32)) * 0.119_899_995_625_019_07 + 0.01
                } else {
                    12.0
                }
            }
            LOW_SHELF | HIGH_SHELF => {
                let r = 1.0 - r;
                if r <= 0.0 {
                    0.1
                } else if r < 1.0 {
                    curve(3.0, f64::from(r as f32)) * 0.449_999_988_079_071_04 + 0.1
                } else {
                    1.0
                }
            }
            PEAK => {
                let r = 1.0 - r;
                if r <= 0.0 {
                    0.1
                } else if r < 1.0 {
                    curve(2001.0, f64::from(r as f32)) * 0.049_949_999_898_672_104 + 0.1
                } else {
                    100.0
                }
            }
            _ => {
                let q = if r <= 0.0 {
                    0.1
                } else if r < 1.0 {
                    curve(2001.0, f64::from(r as f32)) * 0.024_950_001_388_788_223 + 0.1
                } else {
                    50.0
                };
                if last > 0 { power(q, spread) } else { q }
            }
        };
        // The bandwidth of types 4 and 5: the order (its reciprocal for 5)
        // times ln 2 / 2 (`+0x180`, `0x90e500`).
        let order = filter.order.max(1) as f32;
        let width = if kind == STOP { 1.0 / order } else { order };
        let width = f64::from((f64::from(width) * (LN_2 / 2.0)) as f32);
        let scale = scale * TAU;
        for stage in self.stage[..stages].iter_mut().rev() {
            let w = ((fc + offset) * scale).clamp(0.0, 2.849_517_146_113_191);
            let (sin, cos) = w.sin_cos();
            stage.coefficients = match kind {
                LOW => Coefficients64::low_pass(cos, sin * (0.5 / shape)),
                HIGH => Coefficients64::high_pass(cos, sin * (0.5 / shape)),
                ALL => Coefficients64::all_pass(cos, sin * (0.5 / shape)),
                BAND => Coefficients64::band_pass(cos, sin * (shape * width * w / sin).sinh()),
                STOP => Coefficients64::notch(cos, sin * (shape * width * w / sin).sinh()),
                PEAK => Coefficients64::peak(cos, sin / (shape * 2.0), a),
                _ => {
                    let beta = a.sqrt() * ((a + 1.0 / a) * (1.0 / shape - 1.0) + 2.0).sqrt();
                    if kind == LOW_SHELF {
                        Coefficients64::low_shelf(cos, beta * sin, a)
                    } else {
                        Coefficients64::high_shelf(cos, beta * sin, a)
                    }
                }
            };
            fc = detuned(fc);
        }
    }

    /// Type 11, the four pole ladder with drive: Love Philter's Mango, with
    /// Sytrus's 0.6 x min(drive, 1) in the feedback.
    fn ladder(&mut self, filter: &Filter, c: f64, r: f64, low: f32, (spread, scale): (f32, f64)) {
        let stages = filter.stages;
        let last = stages.saturating_sub(1);
        let detune = filter.detune;
        let tuning = TUNING[last.min(2)] as f32;
        let detuned = |x: f64| if detune { power(x, tuning) } else { x };
        let mut fc = if c > 0.0 {
            (curve(201.0, c) * 0.014_949_999_749_660_492).min(3.0)
        } else {
            0.0
        };
        let drive = curve(251.0, f64::from(low * (1.0 / 128.0))) * 0.12;
        let driven = drive >= 0.001;
        let thin = if driven { 1.0 / drive } else { 1.0 };
        let mut feedback = 0.0;
        if r > 0.0 {
            feedback = f64::from((drive as f32).min(1.0)) * 0.6 + 1.9;
            if r < 1.0 {
                feedback *= r;
            }
        }
        if last > 0 {
            feedback = power(feedback + 1.0, spread) - 1.0;
        }
        let scale = scale * 0.5;
        for stage in self.stage[..stages].iter_mut().rev() {
            let g = ((fc + 0.01) * scale).min(0.65);
            let g2 = g * g;
            let turn = ((g * 1.873 * g2 + g2 * 0.4955) - g * 0.649) + 0.9988;
            let mut pole = 1.0 - (-PI * turn * g).exp();
            stage.feedback = (-feedback * (g2 * -3.9364 + g * 1.8409 + 0.9968)) as f32;
            if driven {
                stage.drive = drive * 2.0;
                pole *= thin;
                let heat = (thin * 4.0).tanh() as f32;
                if filter.alternate {
                    stage.feedback *= heat;
                }
                let open = f64::from(1.0 - stage.feedback);
                stage.gain = (f64::from(1.0 / heat) * 0.5 * open) as f32;
            } else {
                stage.drive = 0.0;
                stage.gain = (f64::from(1.0 - stage.feedback) * 0.5) as f32;
            }
            stage.pole = pole as f32;
            fc = detuned(fc);
        }
    }

    /// Run `buffer` through the row, in place, as a render call does: the
    /// volume glides to its aim first, the row runs at twice the rate on
    /// every sample twice over when the quality asks (`0x62c6d0`) and is
    /// brought back by the mean of each pair (`0x62d280`). `cap` is
    /// `TAudioRenderer+0x38`, `work` room for twice the frames.
    pub(super) fn run(&mut self, filter: &Filter, buffer: &mut [f32], work: &mut [f32], cap: f32) {
        let len = buffer.len();
        if len == 0 {
            return;
        }
        let (mut v, step) = glide(&mut self.volume, self.volume_aim, cap, len);
        for x in buffer.iter_mut() {
            *x *= v;
            v += step;
        }
        if filter.stages == 0 {
            return;
        }
        if self.twice {
            let work = &mut work[..2 * len];
            for (pair, x) in work.as_chunks_mut::<2>().0.iter_mut().zip(buffer.iter()) {
                *pair = [*x, *x];
            }
            self.kernels(filter, work, cap);
            for (x, pair) in buffer.iter_mut().zip(work.as_chunks::<2>().0) {
                *x = ((f64::from(pair[0] + pair[1])) * 0.5) as f32;
            }
        } else {
            self.kernels(filter, buffer, cap);
        }
    }

    fn kernels(&mut self, filter: &Filter, buffer: &mut [f32], cap: f32) {
        let n = buffer.len();
        let stages = filter.stages;
        match filter.kind {
            SVF => {
                let mut gains = [(0.0, 0.0); 3];
                for k in 0..3 {
                    gains[k] = glide(&mut self.gains[k], self.gain_aims[k], cap, n);
                }
                let mut f = [(0.0, 0.0); STAGES];
                for (stage, f) in self.stage[..stages].iter_mut().zip(&mut f) {
                    *f = glide(&mut stage.now[0], stage.aim[0], cap, n);
                }
                let stage0 = &mut self.stage[0];
                let (damping, damping_step) = glide(&mut stage0.now[1], stage0.aim[1], cap, n);
                for (k, stage) in self.stage[..stages].iter_mut().enumerate() {
                    let (mut q, q_step) = if k == 0 {
                        (damping, damping_step)
                    } else {
                        (stage.aim[1], 0.0)
                    };
                    // The alternate switch: the damping over the frequency.
                    if filter.alternate {
                        let most = f[k].0.max(stage.now[0]);
                        q = if k == 0 { q * self.scale } else { self.scale } / most;
                    }
                    let (mut fq, f_step) = f[k];
                    let [mut lo, mut band] = stage.state;
                    let (mut g, steps) = (gains.map(|g| g.0), gains.map(|g| g.1));
                    for x in buffer.iter_mut() {
                        lo += band * f64::from(fq);
                        let input = f64::from(*x);
                        let high = input - (band * f64::from(q) + lo);
                        band += high * f64::from(fq);
                        let mut y = lo as f32 * g[0];
                        y += band as f32 * g[1];
                        y += high as f32 * g[2];
                        *x = y;
                        q += q_step;
                        fq += f_step;
                        for (g, s) in g.iter_mut().zip(&steps) {
                            *g += s;
                        }
                    }
                    stage.state = [flushed(lo), flushed(band)];
                }
            }
            LADDER_LOW | LADDER_HIGH => {
                let high = filter.kind == LADDER_HIGH;
                for stage in &mut self.stage[..stages] {
                    let (mut g, g_step) = glide(&mut stage.now[0], stage.aim[0], cap, n);
                    let (mut k, k_step) = glide(&mut stage.now[1], stage.aim[1], cap, n);
                    let [mut lo, mut speed] = stage.state;
                    for x in buffer.iter_mut() {
                        let input = f64::from(*x);
                        speed += (input - lo) * f64::from(g);
                        lo += speed;
                        *x = if high { (input - lo) as f32 } else { lo as f32 };
                        g += g_step;
                        speed *= f64::from(k);
                        k += k_step;
                    }
                    stage.state = [flushed(lo), flushed(speed)];
                }
            }
            LOW..=HIGH_SHELF => {
                for stage in &mut self.stage[..stages] {
                    stage.biquad.process_block(&stage.coefficients, buffer);
                    stage.biquad.flush();
                }
            }
            LADDER => {
                for stage in &mut self.stage[..stages] {
                    let (pole, feedback, gain, drive) =
                        (stage.pole, stage.feedback, stage.gain, stage.drive);
                    let poles = &mut stage.poles;
                    for x in buffer.iter_mut() {
                        let last = poles[3];
                        if drive == 0.0 {
                            poles[0] += ((poles[4] * feedback + *x) - poles[0]) * pole;
                            poles[1] += (poles[0] - poles[1]) * pole;
                            poles[2] += (poles[1] - poles[2]) * pole;
                            poles[3] += (poles[2] - poles[3]) * pole;
                        } else {
                            let hot = (f64::from(poles[4] * feedback + *x) * drive) as f32;
                            let mut hot = saturate(hot);
                            for state in &mut poles[..4] {
                                let own = saturate((f64::from(*state) * drive) as f32);
                                *state = (f64::from(*state)
                                    + f64::from(hot - own) * f64::from(pole))
                                    as f32;
                                hot = saturate((f64::from(*state) * drive) as f32);
                            }
                        }
                        poles[4] = last + poles[3];
                        *x = gain * poles[4];
                    }
                    for state in poles.iter_mut() {
                        if state.abs() < 5.960_464_5e-8 {
                            *state = 0.0;
                        }
                    }
                }
            }
            PHASER => {
                let (a, fb) = (self.coefficient, self.feedback);
                let mut s = self.held;
                let pairs = self.pairs;
                for x in buffer.iter_mut() {
                    s = s * fb + *x;
                    for pair in self.allpass[..2 * pairs].as_chunks_mut::<2>().0 {
                        let y = s * -a + pair[0];
                        pair[0] = s + y * a;
                        let z = y * -a + pair[1];
                        pair[1] = y + z * a;
                        s = z;
                    }
                    *x = s;
                }
                self.held = s;
                for state in &mut self.allpass {
                    if state.abs() < 5.960_464_5e-8 {
                        *state = 0.0;
                    }
                }
            }
            _ => {}
        }
    }
}

/// The ladder's saturator: `x (a^2 + 3a + 6) / (a^3 + 3a^2 + 6a + 12)`
/// with `a` the size of `x` (`0x6275c0`).
fn saturate(x: f32) -> f32 {
    let size = x.abs();
    let poly = (3.0 + size) * size + 6.0;
    x * poly / (size * poly + 12.0)
}

/// A filter's state, with a number too small to hold in full made 0.
fn flushed(state: f64) -> f64 {
    if state.is_subnormal() { 0.0 } else { state }
}
