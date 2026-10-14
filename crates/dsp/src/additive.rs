//! Additive synthesis: a bank of sines with their own frequency, phase and
//! level, and the lookup tables that shape a spectrum by where a partial
//! sits in frequency.
//!
//! The bank is the one Image-Line's additive synths (Harmless, Harmor)
//! build their voices on: every partial is a two-term recursion
//! `s[n] = 2 cos(w) s[n-1] - s[n-2]` started from its phase, and its level
//! moves to the level it is given by a one-pole step every sample. The
//! phase is kept in turns in `f64`, so a partial stays where it should be
//! however long the note is.
//!
//! A [`ShapeTable`] is a curve of points looked up at `x * scale + offset`:
//! with `x` the log frequency of each partial it is a filter response laid
//! over the spectrum, and read round and round it is a comb or a phaser.

use std::f64::consts::TAU;

/// A partial whose level and target are both under this is not rendered.
/// It is the threshold FL's bank drops a partial at, 2^-24.
pub const SILENT: f32 = 5.960_464_5e-8;

/// A bank of sines.
#[derive(Debug, Clone, PartialEq)]
pub struct SineBank {
    /// Turns, `f64` so they do not drift.
    phase: Vec<f64>,
    /// Turns a sample.
    freq: Vec<f64>,
    level: Vec<f32>,
    target: Vec<f32>,
    len: usize,
    /// What is left of the way to the target after a sample.
    smoothing: f32,
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
            len: 0,
            smoothing: 0.0,
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

    /// Put partial `k` at this many turns.
    pub fn set_phase(&mut self, k: usize, turns: f64) {
        self.phase[k] = turns - turns.floor();
    }

    pub fn phase(&self, k: usize) -> f64 {
        self.phase[k]
    }

    /// Set partial `k` to this many turns a sample.
    pub fn set_freq(&mut self, k: usize, turns: f64) {
        self.freq[k] = turns;
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

    /// Every partial silent at once.
    pub fn silence(&mut self) {
        self.level.fill(0.0);
        self.target.fill(0.0);
    }

    /// Add the next `out.len()` samples of the bank to `out`.
    pub fn render_add(&mut self, out: &mut [f32]) {
        let n = out.len();
        let k_smooth = self.smoothing;
        for k in 0..self.len {
            let target = self.target[k];
            let mut level = self.level[k];
            let freq = self.freq[k];
            if level.abs() < SILENT && target.abs() < SILENT {
                self.level[k] = 0.0;
                self.advance(k, freq, n);
                continue;
            }
            // The recursion starts one and two samples back so that its
            // first value is the sine at the phase.
            let phase = self.phase[k];
            let coef = (2.0 * (TAU * freq).cos()) as f32;
            let mut s1 = (TAU * (phase - freq)).sin() as f32;
            let mut s2 = (TAU * (phase - 2.0 * freq)).sin() as f32;
            for sample in out.iter_mut() {
                let s = coef * s1 - s2;
                *sample += s * level;
                level = (level - target) * k_smooth + target;
                s2 = s1;
                s1 = s;
            }
            self.level[k] = level;
            self.advance(k, freq, n);
        }
    }

    fn advance(&mut self, k: usize, freq: f64, n: usize) {
        let phase = self.phase[k] + freq * n as f64;
        self.phase[k] = phase - phase.floor();
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
        assert!((bank.phase(1) - 0.0).abs() < 1e-9);
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
