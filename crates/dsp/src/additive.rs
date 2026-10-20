//! Additive synthesis: a bank of sines with their own frequency, phase and
//! level, and the lookup tables that shape a spectrum by where a partial
//! sits in frequency.
//!
//! The bank is the one Image-Line's additive synths build their voices on
//! (`TFastToneBank` in Harmless, read at `FUN_00746320` and the functions
//! it calls): every partial is a two-term recursion
//! `s[n] = 2 cos(w) s[n-1] - s[n-2]`, and its level moves to the level it
//! is given by a one-pole step every sample. The phase is kept in turns in
//! `f64` and never wrapped, as FL keeps it, so a partial stays where it
//! should be however long the note is.
//!
//! A partial's recursion runs on from call to call. It starts again from
//! its phase when its frequency is set, when it comes back from silence,
//! and when the samples it was given run out: `Round(f * 8192)` of them,
//! counted in whole calls (`FUN_00745730`, `FUN_007457c0`). It starts at
//! `sin(2 pi (phase - f))` and `sin(2 pi (phase - 2 f))`, so its first value
//! is the sine at its phase, and `2 cos w` is worked out again as
//! `2 sin(2 pi (f + 1/4))` when the frequency was set. The arguments are
//! the fraction of the turn by rounding to the nearest whole one, times
//! 2 pi, in `f32`.
//!
//! The partials sound in groups of eight, summed as FL's AVX kernel sums
//! them (`FUN_00745c00` for the first group, `FUN_00745ca0` for the rest,
//! taken on every processor with AVX): four sums, the first partial and
//! the fifth of a group in the first, and so on, each group added on as
//! `(low + sum) + high`, then the four added up left to right
//! (`FUN_00746270`). A level is kept as how far it is from its target,
//! which shrinks by the smoothing every sample.
//!
//! FL drops silent partials out of the groups every 512 samples by swapping
//! the last sounding one into their slot. Here the groups are made afresh
//! on every call from the partials that sound, in their order, so a group
//! can hold other partials than FL's after a partial falls silent.
//!
//! A [`ShapeTable`] is a curve of points looked up at `x * scale + offset`:
//! with `x` the log frequency of each partial it is a filter response laid
//! over the spectrum, and read round and round it is a comb or a phaser.

use std::f64::consts::TAU;

/// A partial whose level and target are both under this is not rendered.
/// It is the threshold FL's bank drops a partial at, 2^-24.
pub const SILENT: f32 = 5.960_464_5e-8;

/// How many partials sound side by side, as FL's groups hold them.
const GROUP: usize = 8;

/// The most samples one pass of the groups takes: the four sums of every
/// sample of the pass are held at once.
const PASS: usize = 256;

/// The samples a partial is given before it starts again from its phase,
/// per turn a sample of its frequency (`DAT_007e9ee0`).
const RESTART: f32 = 8192.0;

/// A bank of sines.
#[derive(Debug, Clone, PartialEq)]
pub struct SineBank {
    /// Turns, `f64` so they do not drift.
    phase: Vec<f64>,
    /// Turns a sample.
    freq: Vec<f64>,
    level: Vec<f32>,
    target: Vec<f32>,
    /// The recursion: the last sample and the one before, and `2 cos w`.
    s1: Vec<f32>,
    s2: Vec<f32>,
    coef: Vec<f32>,
    /// The samples left before the partial starts again from its phase.
    countdown: Vec<i32>,
    /// Whether the frequency was set since the partial last started, so
    /// that `2 cos w` is worked out again when it does.
    retuned: Vec<bool>,
    /// Whether the partial sounded in the last call.
    sounding: Vec<bool>,
    len: usize,
    /// What is left of the way to the target after a sample.
    smoothing: f32,
    /// The four sums of each sample of a pass.
    sums: Vec<[f32; 4]>,
}

/// The fraction of `turns` from the nearest whole turn, as an angle in
/// `f32` (`FUN_00745630`).
fn angle(turns: f64) -> f32 {
    ((turns - turns.round_ties_even()) * TAU) as f32
}

/// The sine of an angle in `f32` from -pi to pi, rounded to `f32`: folded
/// into a quarter turn either side of 0 and summed as its series to the
/// 21st power in `f64`, which is within 2^-60 of the sine there, so it
/// rounds as the sine does but where the sine lies that close to halfway
/// between two `f32`s.
fn sinf(angle: f32) -> f32 {
    use std::f64::consts::{FRAC_PI_2, PI};
    let x = f64::from(angle);
    let x = if x > FRAC_PI_2 {
        PI - x
    } else if x < -FRAC_PI_2 {
        -PI - x
    } else {
        x
    };
    let x2 = x * x;
    // 1/n! for the odd n from 21 down to 3, with their signs.
    const TERMS: [f64; 10] = [
        1.957_294_106_339_126e-20,
        -8.220_635_246_624_33e-18,
        2.811_457_254_345_521e-15,
        -7.647_163_731_819_816e-13,
        1.605_904_383_682_161_3e-10,
        -2.505_210_838_544_172e-8,
        2.755_731_922_398_589e-6,
        -1.984_126_984_126_984e-4,
        8.333_333_333_333_333e-3,
        -1.666_666_666_666_666_6e-1,
    ];
    let series = TERMS.iter().fold(0.0, |sum, term| sum * x2 + term);
    (x + x * x2 * series) as f32
}

impl SineBank {
    /// A bank of `capacity` partials, every one at 0 Hz, phase 0 and
    /// silent, with no smoothing.
    pub fn new(capacity: usize) -> Self {
        SineBank {
            phase: vec![0.0; capacity],
            freq: vec![0.0; capacity],
            level: vec![0.0; capacity],
            target: vec![0.0; capacity],
            s1: vec![0.0; capacity],
            s2: vec![0.0; capacity],
            coef: vec![0.0; capacity],
            countdown: vec![0; capacity],
            retuned: vec![false; capacity],
            sounding: vec![false; capacity],
            len: 0,
            smoothing: 0.0,
            sums: vec![[0.0; 4]; PASS],
        }
    }

    /// How many partials it can hold.
    pub fn capacity(&self) -> usize {
        self.phase.len()
    }

    /// How many partials sound, from the first. The rest are silenced.
    pub fn set_len(&mut self, len: usize) {
        let len = len.min(self.capacity());
        for k in len..self.len {
            self.level[k] = 0.0;
            self.target[k] = 0.0;
        }
        self.len = len;
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The part of the way to its target a level has still to go after a
    /// sample: 0 jumps, values near 1 glide.
    pub fn set_smoothing(&mut self, smoothing: f32) {
        self.smoothing = smoothing.clamp(0.0, 1.0);
    }

    /// Put partial `k` at this many turns. The partial is heard from there
    /// when it next starts again.
    pub fn set_phase(&mut self, k: usize, turns: f64) {
        self.phase[k] = turns;
    }

    pub fn phase(&self, k: usize) -> f64 {
        self.phase[k]
    }

    /// Set partial `k` to this many turns a sample. It starts again from
    /// its phase at the next call.
    pub fn set_freq(&mut self, k: usize, turns: f64) {
        self.freq[k] = turns;
        self.countdown[k] = 0;
        self.retuned[k] = true;
    }

    pub fn freq(&self, k: usize) -> f64 {
        self.freq[k]
    }

    /// The level partial `k` moves to.
    pub fn set_target(&mut self, k: usize, level: f32) {
        self.target[k] = level;
    }

    pub fn level(&self, k: usize) -> f32 {
        self.level[k]
    }

    /// Every partial at its target at once, as for a new note.
    pub fn jump(&mut self) {
        let len = self.len;
        self.level[..len].copy_from_slice(&self.target[..len]);
    }

    /// Every partial silent at once, to start again from its phase when it
    /// sounds.
    pub fn silence(&mut self) {
        self.level.fill(0.0);
        self.target.fill(0.0);
        self.sounding.fill(false);
    }

    /// Start partial `k` again from its phase.
    fn restart(&mut self, k: usize) {
        let (phase, freq) = (self.phase[k], self.freq[k]);
        let back = phase - freq;
        self.s1[k] = sinf(angle(back));
        self.s2[k] = sinf(angle(back - freq));
        if std::mem::take(&mut self.retuned[k]) {
            self.coef[k] = sinf(angle(freq + 0.25)) * 2.0;
        }
        self.countdown[k] = ((freq as f32) * RESTART).round_ties_even() as i32;
    }

    /// Add the next `out.len()` samples of the bank to `out`.
    pub fn render_add(&mut self, out: &mut [f32]) {
        let n = out.len();
        // Which partials sound, and their restarts, before any renders.
        for k in 0..self.len {
            let (level, target) = (self.level[k], self.target[k]);
            let sounds = level.abs() >= SILENT || target.abs() >= SILENT;
            if !sounds {
                self.level[k] = 0.0;
                self.sounding[k] = false;
                continue;
            }
            if !self.sounding[k] {
                self.countdown[k] = 0;
                self.sounding[k] = true;
            }
            self.countdown[k] = self.countdown[k].saturating_sub(n as i32);
            if self.countdown[k] < 1 {
                self.restart(k);
            }
        }
        // The groups, a pass of samples at a time.
        let mut group = [0; GROUP];
        for chunk in out.chunks_mut(PASS) {
            let len = chunk.len();
            // The first group sets the sums: its `low + 0` is `low`.
            self.sums[..len].fill([0.0; 4]);
            let mut found = 0;
            let mut any = false;
            for k in 0..self.len {
                if !self.sounding[k] {
                    continue;
                }
                group[found] = k;
                found += 1;
                if found == GROUP {
                    self.render_group(&group, len);
                    found = 0;
                    any = true;
                }
            }
            if found > 0 {
                self.render_group(&group[..found], len);
                any = true;
            }
            if any {
                for (out, sums) in chunk.iter_mut().zip(&self.sums[..len]) {
                    *out += ((sums[0] + sums[1]) + sums[2]) + sums[3];
                }
            }
        }
        // Every partial moves on, sounding or not (`FUN_00746160`).
        for k in 0..self.len {
            self.phase[k] += self.freq[k] * n as f64;
        }
    }

    /// Run the partials `ks`, eight at most, through `len` samples into
    /// [`SineBank::sums`]: partial `j` and partial `j + 4` of the group into
    /// sum `j`, as `(low + sum) + high`. A lane without a partial puts in a
    /// zero, which leaves a sum as it is.
    fn render_group(&mut self, ks: &[usize], len: usize) {
        let k_smooth = self.smoothing;
        // The low four lanes and the high four, so that each half runs as
        // one vector.
        let mut lanes = [Lanes::default(); 2];
        for (lane, &k) in ks.iter().enumerate() {
            let (half, j) = (&mut lanes[lane / 4], lane % 4);
            half.coef[j] = self.coef[k];
            half.s1[j] = self.s1[k];
            half.s2[j] = self.s2[k];
            half.target[j] = self.target[k];
            half.away[j] = self.level[k] - self.target[k];
            half.heard[j] = 1.0;
        }
        let [mut low, mut high] = lanes;
        for sums in &mut self.sums[..len] {
            let p_low = low.step(k_smooth);
            let p_high = high.step(k_smooth);
            for j in 0..4 {
                sums[j] = (p_low[j] + sums[j]) + p_high[j];
            }
        }
        for (lane, &k) in ks.iter().enumerate() {
            let (half, j) = if lane < 4 {
                (&low, lane)
            } else {
                (&high, lane - 4)
            };
            self.level[k] = half.away[j] + half.target[j];
            self.s1[k] = half.s1[j];
            self.s2[k] = half.s2[j];
        }
    }
}

/// Four lanes of a group: their recursions and levels.
#[derive(Clone, Copy, Default)]
struct Lanes {
    coef: [f32; 4],
    s1: [f32; 4],
    s2: [f32; 4],
    target: [f32; 4],
    /// How far each level is from its target.
    away: [f32; 4],
    /// 1 for a lane with a partial, 0 for one without.
    heard: [f32; 4],
}

impl Lanes {
    /// The next sample of each lane at its level, and the levels a sample
    /// on.
    #[inline(always)]
    #[allow(clippy::needless_range_loop)] // six arrays by the same lane
    fn step(&mut self, smoothing: f32) -> [f32; 4] {
        let mut out = [0.0; 4];
        for j in 0..4 {
            let s = self.s1[j] * self.coef[j] - self.s2[j];
            out[j] = s * (self.away[j] + self.target[j]) * self.heard[j];
            self.s2[j] = self.s1[j];
            self.s1[j] = s;
            self.away[j] *= smoothing;
        }
        out
    }
}

/// A curve of points, looked up at `x * scale + offset`.
///
/// The curve holds `len + 1` points, the last a copy of the first so a
/// lookup that goes round reads across the join. `centre`, `below` and
/// `above` are what the plugin that made the curve says of it: the point
/// its centre is at, and how far the curve reaches below and above its
/// centre in units of its own width (Harmless's filter ranges add them).
#[derive(Debug, Clone, PartialEq)]
pub struct ShapeTable {
    points: Vec<f32>,
    pub centre: f32,
    pub below: f32,
    pub above: f32,
    /// The points a turn takes, for a curve that is read round and round.
    pub period: f32,
}

impl ShapeTable {
    /// A curve of `len` points from `point(i)`, centred at
    /// `centre * (len - 1)`.
    pub fn new(
        len: usize,
        centre: f64,
        below: f32,
        above: f32,
        point: impl Fn(usize) -> f64,
    ) -> Self {
        let mut points: Vec<f32> = (0..len).map(|i| point(i) as f32).collect();
        points.push(points[0]);
        ShapeTable {
            points,
            centre: (centre * (len as f64 - 1.0)) as f32,
            below,
            above,
            period: len as f32,
        }
    }

    /// A curve whose points are set one by one, all 0 to start.
    pub fn zeroed(len: usize, centre: f64, below: f32, above: f32) -> Self {
        Self::new(len, centre, below, above, |_| 0.0)
    }

    /// How many points it has, without the copy at the end.
    pub fn len(&self) -> usize {
        self.points.len() - 1
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn points(&self) -> &[f32] {
        &self.points[..self.len()]
    }

    /// Set point `i`. The copy of the first point follows it.
    pub fn set(&mut self, i: usize, value: f32) {
        self.points[i] = value;
        if i == 0 {
            let last = self.points.len() - 1;
            self.points[last] = value;
        }
    }

    pub fn get(&self, i: usize) -> f32 {
        self.points[i]
    }

    /// Scale the points so their root mean square is `rms`. A silent curve
    /// stays silent.
    pub fn normalise_rms(&mut self, rms: f64) {
        let len = self.len();
        let sum: f64 = self.points[..len]
            .iter()
            .map(|p| f64::from(*p).powi(2))
            .sum();
        let now = (sum / len as f64).sqrt();
        if now > 0.0 {
            let gain = (rms / now) as f32;
            for p in &mut self.points {
                *p *= gain;
            }
        }
    }

    /// Points that come to less than 2^-24 become 0, as FL flushes them.
    pub fn flush(&mut self) {
        for p in &mut self.points {
            if p.abs() < SILENT {
                *p = 0.0;
            }
        }
    }

    /// The curve at `x * scale + offset`, between the two points either
    /// side, and the first or the last point outside it.
    pub fn clamped(&self, x: f32, scale: f32, offset: f32) -> f32 {
        let at = x * scale + offset;
        let i = (at - 0.5).round();
        if i < 0.0 {
            return self.points[0];
        }
        let last = self.len() - 1;
        let i = i as usize;
        if i >= last {
            return self.points[last];
        }
        let frac = at - i as f32;
        self.points[i] * (1.0 - frac) + self.points[i + 1] * frac
    }

    /// The curve at `x * scale + offset`, read round and round, between
    /// the two points either side. The length must be a power of two.
    pub fn wrapped(&self, x: f32, scale: f32, offset: f32) -> f32 {
        let at = x * scale + offset;
        let floor = (at - 0.5).round();
        let frac = at - floor;
        let i = (floor as i64 as usize) & (self.len() - 1);
        self.points[i] * (1.0 - frac) + self.points[i + 1] * frac
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_partial_is_a_sine_at_its_phase() {
        let mut bank = SineBank::new(4);
        bank.set_len(1);
        bank.set_freq(0, 0.01);
        bank.set_phase(0, 0.25);
        bank.set_target(0, 0.5);
        bank.jump();
        let mut out = vec![0.0; 300];
        for chunk in out.chunks_mut(64) {
            bank.render_add(chunk);
        }
        for (n, value) in out.iter().enumerate() {
            let want = 0.5 * (TAU * (0.25 + 0.01 * n as f64)).sin();
            assert!(
                (f64::from(*value) - want).abs() < 1e-4,
                "{n}: {value} {want}"
            );
        }
    }

    /// FL's AVX bank, slot for slot: `FUN_00745c00` and `FUN_00745ca0`
    /// over every group, then `FUN_00746270`.
    fn fl_sum(levels: &[f32], s1: &[f32], s2: &[f32], coef: &[f32], n: usize) -> Vec<f32> {
        let mut sums = vec![[0.0f32; 4]; n];
        for g in 0..levels.len().div_ceil(8) {
            let lane = |j: usize| (g * 8 + j < levels.len()).then_some(g * 8 + j);
            let (mut a, mut b) = ([0.0f32; 8], [0.0f32; 8]);
            for j in 0..8 {
                if let Some(k) = lane(j) {
                    (a[j], b[j]) = (s1[k], s2[k]);
                }
            }
            for sums in sums.iter_mut() {
                let mut p = [0.0f32; 8];
                for j in 0..8 {
                    if let Some(k) = lane(j) {
                        let s = a[j] * coef[k] - b[j];
                        p[j] = s * levels[k];
                        (b[j], a[j]) = (a[j], s);
                    }
                }
                for j in 0..4 {
                    sums[j] = if g == 0 {
                        p[j] + p[j + 4]
                    } else {
                        (p[j] + sums[j]) + p[j + 4]
                    };
                }
            }
        }
        sums.iter().map(|s| ((s[0] + s[1]) + s[2]) + s[3]).collect()
    }

    #[test]
    fn the_partials_add_up_as_fl_s_kernel_adds_them() {
        let count = 21;
        let mut bank = SineBank::new(32);
        bank.set_len(count);
        for k in 0..count {
            bank.set_freq(k, 0.003 * (k + 1) as f64);
            bank.set_phase(k, 0.1 * k as f64);
            bank.set_target(k, 1.0 / (k + 1) as f32);
        }
        bank.jump();
        let mut out = vec![0.0; 200];
        bank.render_add(&mut out);
        let start = |k: usize, back: f64| {
            let (phase, freq) = (0.1 * k as f64, 0.003 * (k + 1) as f64);
            sinf(angle(phase - freq - back * freq))
        };
        let s1: Vec<f32> = (0..count).map(|k| start(k, 0.0)).collect();
        let s2: Vec<f32> = (0..count).map(|k| start(k, 1.0)).collect();
        let coef: Vec<f32> = (0..count)
            .map(|k| sinf(angle(0.003 * (k + 1) as f64 + 0.25)) * 2.0)
            .collect();
        let levels: Vec<f32> = (0..count).map(|k| 1.0 / (k + 1) as f32).collect();
        assert_eq!(out, fl_sum(&levels, &s1, &s2, &coef, 200));
    }

    #[test]
    fn a_partial_runs_on_between_calls_until_its_samples_run_out() {
        let render = |chunk: usize| {
            let mut bank = SineBank::new(4);
            bank.set_len(2);
            bank.set_freq(0, 0.11);
            bank.set_freq(1, 0.037);
            bank.set_target(0, 0.5);
            bank.set_target(1, 0.25);
            bank.jump();
            let mut out = vec![0.0; 300];
            for part in out.chunks_mut(chunk) {
                bank.render_add(part);
            }
            out
        };
        // 0.037 turns a sample gives 303 samples before a restart, so no
        // partial starts again inside 300, however they are cut up.
        assert_eq!(render(300), render(37));
    }

    #[test]
    fn the_sine_rounds_as_the_maths_library_s_does() {
        let mut differ = 0;
        for i in 0..=2_000_000 {
            let angle = ((i as f64 / 1_000_000.0 - 1.0) * std::f64::consts::PI) as f32;
            differ += usize::from(sinf(angle) != (f64::from(angle).sin() as f32));
        }
        assert_eq!(differ, 0);
    }

    #[test]
    fn a_level_glides_to_its_target() {
        let mut bank = SineBank::new(1);
        bank.set_len(1);
        bank.set_freq(0, 0.25);
        bank.set_phase(0, 0.25);
        bank.set_smoothing(0.5);
        bank.set_target(0, 1.0);
        let mut out = vec![0.0; 8];
        bank.render_add(&mut out);
        // The peaks of a quarter-turn sine are every other sample, and the
        // level halves its way to 1 every sample.
        assert!((out[0] - 0.0).abs() < 1e-6);
        assert!((out[2] + 0.75).abs() < 1e-5);
        assert!((out[4] - 0.9375).abs() < 1e-5);
    }

    #[test]
    fn a_silent_bank_adds_nothing_and_keeps_time() {
        let mut bank = SineBank::new(2);
        bank.set_len(2);
        bank.set_freq(1, 0.1);
        let mut out = vec![0.25; 10];
        bank.render_add(&mut out);
        assert!(out.iter().all(|v| *v == 0.25));
        // A whole turn on: FL never wraps the phase.
        assert!((bank.phase(1) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_table_interpolates_and_clamps() {
        let table = ShapeTable::new(5, 0.0, 1.0, 0.0, |i| i as f64);
        assert_eq!(table.clamped(-3.0, 1.0, 0.0), 0.0);
        assert_eq!(table.clamped(2.5, 1.0, 0.0), 2.5);
        assert_eq!(table.clamped(9.0, 1.0, 0.0), 4.0);
        let round = ShapeTable::new(4, 0.0, 0.0, 0.0, |i| i as f64);
        // Past the last point it goes back to the first.
        assert_eq!(round.wrapped(3.5, 1.0, 0.0), 1.5);
        assert_eq!(round.wrapped(5.0, 1.0, 0.0), 1.0);
    }

    #[test]
    fn a_table_normalises_to_its_rms() {
        let mut table = ShapeTable::new(4, 0.0, 0.0, 0.0, |i| [1.0, -1.0, 1.0, -1.0][i]);
        table.normalise_rms(3.0);
        assert!(table.points().iter().all(|p| p.abs() == 3.0));
    }
}
