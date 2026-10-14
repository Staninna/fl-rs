//! Harmor's articulators at runtime (`docs/plugins/harmor.md` section
//! 3.3): `TOpArticulator` over the shared curve classes.
//!
//! An articulator is a base value, an envelope, an LFO and nine maps,
//! put together by its [`Mode`]: they multiply, add, or multiply by twice
//! their value. Flat maps fold into the base once (`FUN_00898200`), the
//! random and unison maps once a voice (`FUN_00897b50`), and the rest are
//! read every control tick (`FUN_00897d90`), whose result a one-pole then
//! smooths.
//!
//! The envelope runner is Harmor's own: it is `menvelope.rs`'s [`Track`]
//! with what Harmor adds, kept per voice. The four knobs and the global
//! envelope scales (articulators 34 to 37) live in the run, a sustain
//! offset lifts the sustain points, a release carries the offset over so
//! the release starts where the envelope is, and the attack can be jumped
//! into. The LFO is `menvelope.rs`'s.
//!
//! [`Track`]: crate::menvelope::Track

use crate::harmor_state::{ArtState, Mode};
use crate::menvelope::{Lfo, LfoRun, MEnvelope};
use dsp::curve::Curve;

/// Stages, numbered as FL numbers them.
const ATTACK: i32 = 1;
const DECAY: i32 = 3;
const HELD: i32 = 5;
const RELEASE: i32 = 6;
const OVER: i32 = 9;

/// A loop of no length is left after this many turns in one tick.
const TURNS: usize = 4096;

/// Steps a baked map has over its unit.
const MAP_STEPS: usize = 256;

/// An envelope made ready, for every voice of a part.
#[derive(Debug, Clone, PartialEq)]
pub struct Envelope {
    curve: Curve,
    lengths: Vec<f32>,
    /// Each point's value less the centre, and how much of the sustain
    /// offset it takes.
    values: Vec<f32>,
    weights: Vec<f32>,
    /// The values a lifted point is kept between, less the centre.
    low: f32,
    high: f32,
    decay: i32,
    loop_start: i32,
    loop_end: i32,
    last: i32,
    stuck: bool,
    /// The attack, decay and release knobs as scales and the sustain knob
    /// as an offset.
    knobs: [f32; 4],
    pub active: bool,
    /// `x` counts beats, and half seconds otherwise.
    pub tempo: bool,
    /// One run for every voice (flag 2), which this does not share.
    pub shared: bool,
}

/// Where a voice is in an [`Envelope`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnvRun {
    stage: i32,
    held: i32,
    point: usize,
    phase: f64,
    step: f64,
    from: f32,
    rise: f32,
    /// Attack, decay, sustain offset and release, with the global scales.
    scales: [f32; 4],
    pub out: f32,
}

impl EnvRun {
    pub fn over(&self) -> bool {
        self.stage >= OVER
    }

    /// Scale the attack, as VelToAttScale does at a trigger.
    pub fn scale_attack(&mut self, by: f32) {
        self.scales[0] *= by;
    }

    fn idle() -> Self {
        EnvRun {
            stage: OVER,
            held: OVER,
            point: 0,
            phase: 0.0,
            step: 0.0,
            from: 0.0,
            rise: 0.0,
            scales: [1.0, 1.0, 0.0, 1.0],
            out: 0.0,
        }
    }
}

impl Envelope {
    pub fn new(envelope: &MEnvelope, centre: f64) -> Self {
        let points = &envelope.points;
        let count = points.len() as i32;
        let mut last = envelope.loop_end;
        if last < 0 && envelope.loop_start >= 0 {
            last = count - 1;
        }
        let place = |index: i32| -> f64 {
            let upto = usize::try_from(index + 1).unwrap_or(0).min(points.len());
            points[..upto].iter().map(|point| point.x.max(0.0)).sum()
        };
        let stuck = last >= 0
            && envelope.loop_start >= 0
            && place(last) - place(envelope.loop_start) <= 1e-9;
        // The sustain knob lifts the points after the decay marker up to
        // the loop end (the weights FL keeps per point are not read).
        let weights = (0..count)
            .map(|i| {
                if envelope.loop_end >= 0 && i > envelope.decay && i <= envelope.loop_end {
                    1.0
                } else {
                    0.0
                }
            })
            .collect();
        let [a, d, s, r] = envelope.knobs;
        Envelope {
            curve: Curve::new(points),
            lengths: points.iter().map(|p| p.x.max(0.0) as f32).collect(),
            values: points.iter().map(|p| (p.y - centre) as f32).collect(),
            weights,
            low: -centre as f32,
            high: (1.0 - centre) as f32,
            decay: envelope.decay,
            loop_start: envelope.loop_start,
            loop_end: envelope.loop_end,
            last,
            stuck,
            knobs: [
                a as f32 / 128.0,
                d as f32 / 128.0,
                s as f32 / 128.0,
                r as f32 / 128.0,
            ],
            active: envelope.active && !points.is_empty(),
            tempo: envelope.flags & 1 != 0,
            shared: envelope.flags & 2 != 0,
        }
    }

    /// Point `i`'s value with the sustain offset on it (`FUN_0079fef0`).
    fn value(&self, i: usize, offset: f32) -> f32 {
        let v = self.values[i];
        if offset == 0.0 || self.weights[i] == 0.0 {
            v
        } else {
            (v + self.weights[i] * offset).clamp(self.low, self.high)
        }
    }

    /// A run at the first point.
    pub fn start(&self, rate: f64) -> EnvRun {
        let mut run = EnvRun::idle();
        run.scales = self.knobs;
        let (Some(first), true) = (self.lengths.first(), self.active) else {
            return run;
        };
        run.stage = if self.decay < 0 { DECAY } else { ATTACK };
        run.held = run.stage;
        if *first == 0.0 {
            (run.step, run.phase) = (1.0, 1.0);
        } else {
            run.step = rate / f64::from(*first);
        }
        run.from = self.value(0, run.scales[2]);
        run.out = run.from;
        run
    }

    /// Put the global scales on a run, on a voice's first tick: attack,
    /// decay and release times and the sustain offset.
    pub fn scale(&self, run: &mut EnvRun, globals: [f32; 4]) {
        run.scales[0] *= globals[0];
        run.scales[1] *= globals[1];
        run.scales[2] += globals[2];
        run.scales[3] *= globals[3];
    }

    /// Jump `share` of the way into the attack (`FUN_007a2580`).
    pub fn jump(&self, run: &mut EnvRun, share: f32, rate: f64) {
        if self.decay < 0 || share <= 0.0 || rate <= 0.0 {
            return;
        }
        let upto = (self.decay as usize + 1).min(self.lengths.len());
        let place: f64 = self.lengths[..upto].iter().map(|l| f64::from(*l)).sum();
        let ticks = place * f64::from(run.scales[0]) * f64::from(share) / rate;
        self.advance(run, ticks, rate);
    }

    /// Let go (`FUN_007a09f0`): a run at or before the loop end moves to
    /// it, with the offset carried over so the release starts where the
    /// run is.
    pub fn release(&self, run: &mut EnvRun, scale: f32) {
        if run.stage >= RELEASE {
            return;
        }
        run.scales[3] *= scale;
        if self.loop_end >= 0 && (self.loop_end as usize) < self.values.len() {
            let end = self.loop_end as usize;
            let now = self.value(end, run.scales[2]);
            run.scales[2] += run.out - now;
            // The carried offset reaches the points the loop end has.
            run.point = end;
            run.from = run.out;
            run.rise = 0.0;
            (run.phase, run.step) = (1.0, 1.0);
        }
        run.stage = RELEASE;
        run.held = RELEASE;
    }

    /// Move a run on by `dt` ticks. This allocates nothing.
    pub fn advance(&self, run: &mut EnvRun, dt: f64, rate: f64) {
        let count = self.values.len();
        if run.point >= count || run.stage >= OVER {
            return;
        }
        run.phase += dt * run.step;
        let mut turns = 0;
        while run.phase >= 1.0 {
            let left = if run.step == 0.0 {
                dt
            } else {
                (run.phase - 1.0) / run.step
            };
            run.from = if run.stage >= RELEASE && run.point as i32 == self.loop_end {
                run.out
            } else {
                self.value(run.point, run.scales[2])
            };
            let at = run.point as i32;
            let done = if at < self.last || self.last < 0 || run.stage > HELD {
                if self.decay <= at && self.decay >= 0 && run.held < DECAY {
                    run.stage = run.stage.max(DECAY);
                    run.held = DECAY;
                }
                if run.point + 1 < count {
                    run.point += 1;
                    false
                } else {
                    run.stage = OVER;
                    true
                }
            } else if self.loop_start < 0 {
                true
            } else {
                run.held = 4;
                run.point = (self.loop_start as usize + 1).min(count - 1);
                self.stuck
            };
            run.rise = self.value(run.point, run.scales[2]) - run.from;
            turns += 1;
            if done || turns > TURNS {
                (run.phase, run.step) = (0.0, 0.0);
                run.rise = 0.0;
                break;
            }
            let released = run.stage >= RELEASE && self.loop_end >= 0;
            let scale = if released {
                run.scales[3]
            } else if run.stage > 2 {
                run.scales[1]
            } else {
                run.scales[0]
            };
            let length = f64::from(self.lengths[run.point]) * f64::from(scale);
            if length <= 0.0 {
                run.step = 1.0;
                run.phase = left + 1.0;
            } else {
                run.step = rate / length;
                run.phase = left * run.step;
            }
        }
        run.out = if run.rise == 0.0 {
            run.from
        } else {
            run.from + self.curve.progress(run.point, run.phase) as f32 * run.rise
        };
    }
}

/// A map baked over its unit, less its centre.
#[derive(Debug, Clone, PartialEq)]
pub struct Map {
    table: Vec<f32>,
}

impl Map {
    fn new(map: &MEnvelope, centre: f64) -> Self {
        Map {
            table: map.table(MAP_STEPS, centre),
        }
    }

    /// The map at `x` from 0 to 1, between its steps.
    pub fn at(&self, x: f32) -> f32 {
        let at = (x.clamp(0.0, 1.0) * MAP_STEPS as f32).max(0.0);
        let i = (at as usize).min(MAP_STEPS - 1);
        let frac = at - i as f32;
        self.table[i] + (self.table[i + 1] - self.table[i]) * frac
    }
}

/// Which map is which, in the stream's order.
pub const RANDOM: usize = 5;
pub const UNISON: usize = 6;

/// One articulator made ready.
#[derive(Debug, Clone, PartialEq)]
pub struct Articulator {
    pub mode: Mode,
    /// What the flat maps come to.
    base: f32,
    pub envelope: Envelope,
    pub lfo: Lfo,
    lfo_on: bool,
    /// The maps read every tick: keyboard, velocity, mod X, Y, Z, held
    /// index and mod X speed. The random and unison ones are read once.
    maps: [Option<Map>; 9],
    /// What is left of the way to the value after a tick; 1 holds it.
    pub smoothing: f32,
}

/// What every articulator of a voice reads in a tick.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Inputs {
    /// Ticks to move on by: 0 on the first.
    pub dt: f64,
    /// The global LFO speed and amount (articulators 39 and 38).
    pub speed: f64,
    pub lfo_amount: f32,
    /// Where the voice is on each map, 0 to 1.
    pub at: [f32; 9],
    /// x units a tick: half seconds, beats.
    pub rates: [f64; 2],
}

/// Where one voice is in an [`Articulator`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArtRun {
    pub env: EnvRun,
    pub lfo: Option<LfoRun>,
    /// The base with the voice's random and unison maps.
    base: f32,
    /// The value before smoothing, and after.
    pub value: f32,
    pub out: f32,
}

impl Articulator {
    pub fn new(state: &ArtState, index: usize) -> Self {
        let mode = Mode::of(index);
        let centre = mode.centre();
        let mut base = if mode == Mode::Add { 0.0 } else { 1.0 };
        let mut maps: [Option<Map>; 9] = Default::default();
        for (k, map) in state.maps.iter().enumerate() {
            match map.flat() {
                // A random map that is switched off does nothing.
                Some(_) if k == RANDOM && !map.active => {}
                Some(level) => base = combine(mode, base, (f64::from(level) - centre) as f32),
                None if k == RANDOM && !map.active => {}
                None => maps[k] = Some(Map::new(map, centre)),
            }
        }
        Articulator {
            mode,
            base,
            envelope: Envelope::new(&state.envelope, centre),
            lfo: Lfo::new(&state.lfo),
            lfo_on: state.lfo.active,
            maps,
            smoothing: 0.0,
        }
    }

    /// A run for a new voice (`FUN_00897b50`, `FUN_00897c70`): `random`
    /// is the voice's random value (none when voice randomness is off) and
    /// `unison` its place among the unison voices, both 0 to 1.
    pub fn start(&self, random: Option<f32>, unison: f32, rates: [f64; 2]) -> ArtRun {
        let mut base = self.base;
        if let (Some(map), Some(random)) = (&self.maps[RANDOM], random) {
            base = combine(self.mode, base, map.at(random));
        }
        if let Some(map) = &self.maps[UNISON] {
            base = combine(self.mode, base, map.at(unison));
        }
        let rate = rates[usize::from(self.envelope.tempo)];
        let lfo = self
            .lfo_on
            .then(|| self.lfo.start(rates[usize::from(self.lfo.tempo())]));
        ArtRun {
            env: self.envelope.start(rate),
            lfo,
            base,
            value: base,
            out: base,
        }
    }

    /// One tick (`FUN_00897d90`); on the first, the value is taken as it
    /// is (`FUN_00898120`).
    pub fn tick(&self, run: &mut ArtRun, inputs: &Inputs, first: bool) {
        let mut acc = run.base;
        if self.envelope.active {
            let rate = inputs.rates[usize::from(self.envelope.tempo)];
            self.envelope.advance(&mut run.env, inputs.dt, rate);
            let e = run.env.out;
            acc = match self.mode {
                Mode::Add => acc + e,
                Mode::Multiply => acc * e,
                Mode::Double => acc * e * 2.0,
            };
        }
        if let Some(lfo) = &mut run.lfo {
            let rate = inputs.rates[usize::from(self.lfo.tempo())] * inputs.speed;
            self.lfo.advance(lfo, inputs.dt, rate);
            let l = lfo.out * inputs.lfo_amount;
            acc = match self.mode {
                Mode::Add => acc + l,
                _ => acc * (2.0 * l + 1.0),
            };
        }
        for (k, map) in self.maps.iter().enumerate() {
            if k == RANDOM || k == UNISON {
                continue;
            }
            if let Some(map) = map {
                acc = combine(self.mode, acc, map.at(inputs.at[k]));
            }
        }
        run.value = acc;
        run.out = if first {
            acc
        } else {
            acc + (run.out - acc) * self.smoothing
        };
    }
}

fn combine(mode: Mode, acc: f32, value: f32) -> f32 {
    match mode {
        Mode::Multiply => acc * value,
        Mode::Add => acc + value,
        Mode::Double => acc * value * 2.0,
    }
}

/// The smoothing an articulator's wheel (0..1001) gives at a control
/// tick of `tick` samples (`FUN_008980b0`).
pub fn smoothing(wheel: i32, rate: f64, tick: f64) -> f32 {
    if wheel >= 1001 {
        return 1.0;
    }
    let ticks = ((0.009 * f64::from(wheel)).exp2() - 1.0) * rate / (tick * 51.1);
    if ticks > 0.0 {
        (0.01f64.ln() / ticks).exp() as f32
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harmor_state::idle;
    use crate::menvelope::ENVELOPE_KNOBS;
    use dsp::curve::Point;

    fn adsr(points: &[(f64, f64)], decay: i32, loop_end: i32) -> MEnvelope {
        let mut e = idle(1, 0.0, ENVELOPE_KNOBS);
        e.points = points.iter().map(|(x, y)| Point::new(*x, *y)).collect();
        e.arp = vec![0; points.len()];
        e.active = true;
        e.decay = decay;
        e.loop_end = loop_end;
        e
    }

    #[test]
    fn an_envelope_rises_holds_and_releases_from_where_it_is() {
        // Up in 1, down to 0.5 in 1, hold, release to 0 in 1.
        let env = Envelope::new(
            &adsr(&[(0.0, 0.0), (1.0, 1.0), (1.0, 0.5), (1.0, 0.0)], 1, 2),
            0.0,
        );
        let mut run = env.start(0.25);
        env.advance(&mut run, 2.0, 0.25);
        assert!((run.out - 0.5).abs() < 1e-6, "{}", run.out);
        env.advance(&mut run, 2.0, 0.25);
        assert!((run.out - 1.0).abs() < 1e-6);
        env.advance(&mut run, 20.0, 0.25);
        assert!((run.out - 0.5).abs() < 1e-6);
        assert!(!run.over());
        // Let go half way down the decay: the release starts there.
        let mut early = env.start(0.25);
        env.advance(&mut early, 6.0, 0.25);
        assert!((early.out - 0.75).abs() < 1e-6, "{}", early.out);
        env.release(&mut early, 1.0);
        env.advance(&mut early, 2.0, 0.25);
        assert!((early.out - 0.375).abs() < 1e-6, "{}", early.out);
        env.advance(&mut early, 3.0, 0.25);
        assert!(early.over());
        assert_eq!(early.out, 0.0);
    }

    #[test]
    fn the_sustain_offset_lifts_the_hold() {
        let mut e = adsr(&[(0.0, 0.0), (1.0, 1.0), (1.0, 0.5), (1.0, 0.0)], 1, 2);
        e.knobs = [128, 128, 64, 128];
        let env = Envelope::new(&e, 0.0);
        let mut run = env.start(1.0);
        env.advance(&mut run, 5.0, 1.0);
        assert!((run.out - 1.0).abs() < 1e-6, "{}", run.out);
    }

    #[test]
    fn a_jump_skips_into_the_attack() {
        let env = Envelope::new(&adsr(&[(0.0, 0.0), (1.0, 1.0), (1.0, 0.5)], 1, -1), 0.0);
        let mut run = env.start(0.1);
        env.jump(&mut run, 0.5, 0.1);
        assert!((run.out - 0.5).abs() < 1e-5, "{}", run.out);
    }

    #[test]
    fn smoothing_is_ten_seconds_at_1000_and_holds_at_1001() {
        assert_eq!(smoothing(1001, 44_100.0, 88.0), 1.0);
        assert_eq!(smoothing(0, 44_100.0, 88.0), 0.0);
        let k = smoothing(1000, 44_100.0, 88.0);
        let ticks = 10.0 * 44_100.0 / 88.0;
        assert!((f64::from(k).powf(ticks) - 0.01).abs() < 2e-3);
    }
}
