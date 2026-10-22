//! Morphine's effects, in the order `FUN_1800ff630` runs them on the
//! voices' sum: overdrive, chorus, delay, reverb and an eight-band
//! equaliser.
//!
//! The chorus and the delay are both made of the engine's `CTapeDelay`
//! (`FUN_1800e9810`): a tape whose write head moves at a speed an LFO
//! bends, each sample written with the time it was written at, and a read
//! head that plays the last pass of the tape at the time the write head is
//! at now. The delay is one pass of the tape; the LFO bends the pitch.
//! The reverb is eight damped combs and four allpasses a channel after a
//! 50 Hz highpass (`FUN_1801247d0`); the equaliser RBJ's shelves and peaks
//! (`FUN_1800dfa60`).

use crate::morphine_state::PARAMS;
use std::f64::consts::PI;

fn flush(v: f32) -> f32 {
    if v < 1e-9 && v > -1e-9 { 0.0 } else { v }
}

/// One `CTapeDelay`.
#[derive(Debug, Clone, Default)]
struct Tape {
    /// The samples written, with the time each was written at in passes
    /// of the tape, oldest first, as a ring.
    times: Vec<f64>,
    values: Vec<f32>,
    head: usize,
    len: usize,
    /// The oldest entry the read head may still need.
    cursor: usize,
    /// Passes done, and where the write head is in this one.
    pass: f64,
    now: f64,
    /// The time the write head starts at (`this+0x60`).
    offset: f64,
    /// The LFO's phase and step (`+0x30`, `+0x38`) and depth (`+0x78`).
    phase: f64,
    rate: f64,
    depth: f32,
    /// The write head's step a sample, smoothed to its target (`+0x50`,
    /// `+0x48`), and the samples on the tape (`+0x28`).
    step: f64,
    target: f64,
    size: u32,
    feedback: f32,
    gain: f32,
    gain_target: f32,
}

impl Tape {
    fn new(size: u32, offset: f64) -> Self {
        let size = size.max(4);
        let cap = 2 * (size as usize + 256) + 16;
        let mut tape = Tape {
            times: vec![0.0; cap],
            values: vec![0.0; cap],
            size,
            offset,
            ..Tape::default()
        };
        tape.stretch(0.5);
        tape.reset();
        tape
    }

    /// The tape's length as a fraction of its room (`FUN_1800e9bb0`).
    fn stretch(&mut self, s: f64) {
        let s = s.clamp(0.0, 0.99);
        let n = f64::from(self.size);
        let two = 2.0 / n;
        self.target = 1.0 / ((s * (1.0 - two) + two) * n);
    }

    fn reset(&mut self) {
        self.now = self.offset;
        self.pass = 0.0;
        self.step = self.target;
        self.gain = 0.0;
        self.phase = 0.0;
        self.head = 0;
        self.len = 0;
        self.cursor = 0;
    }

    fn at(&self, i: usize) -> usize {
        (self.head + i) % self.times.len()
    }

    fn run(&mut self, x: f32) -> f32 {
        if self.times.is_empty() {
            return 0.0;
        }
        let x = flush(x);
        let ph = self.phase;
        let tri = if ph >= 0.5 {
            1.0 - (2.0 * ph - 1.0)
        } else {
            ph + ph
        };
        let next = ph + self.rate;
        self.phase = if next >= 1.0 {
            next - f64::from(next as i32)
        } else {
            next
        };
        let speed = ((3.0 - (tri + tri)) * tri * f64::from(self.depth) * tri + 1.0).max(1.0);
        // The read head: the last pass at the time the write head is at.
        let want = self.pass - 1.0 + self.now;
        let mut read = 0.0;
        while self.cursor < self.len {
            let i = self.at(self.cursor);
            let t = self.times[i];
            // FL keeps each pass with the sample that crossed into the
            // next one at its end, so the read head may use the first
            // sample past the pass, but none after it.
            if t >= self.pass
                && (self.cursor == 0 || self.times[self.at(self.cursor - 1)] >= self.pass)
            {
                break;
            }
            if t >= want {
                let (t0, v0) = if self.cursor > 0 {
                    let j = self.at(self.cursor - 1);
                    (self.times[j] - (self.pass - 1.0), self.values[j])
                } else {
                    (0.0, 0.0)
                };
                let t1 = (t - (self.pass - 1.0)) as f32;
                let t0 = t0 as f32;
                let u = if t0 < t1 {
                    (self.now - f64::from(t0)) / f64::from(t1 - t0)
                } else {
                    0.0
                };
                let r = (f64::from(self.values[i] - v0) * u + f64::from(v0)) as f32;
                read = if r < 1e-9 && r > -1e-9 { 0.0 } else { r };
                break;
            }
            self.cursor += 1;
        }
        // No slower than a pass over the whole tape (`this+0x68`, 1 / size).
        let step = (speed * self.step).min(0.2).max(1.0 / f64::from(self.size));
        let write = read * self.feedback + x;
        let mut now = step + self.now;
        if now >= 1.0 {
            now -= 1.0;
            self.pass += 1.0;
        }
        self.now = now;
        if self.len == self.times.len() {
            self.head = self.at(1);
            self.len -= 1;
            self.cursor = self.cursor.saturating_sub(1);
        }
        let i = self.at(self.len);
        self.times[i] = self.pass + now;
        self.values[i] = write;
        self.len += 1;
        let out = read * self.gain;
        let g = (f64::from(self.gain_target - self.gain) * 0.01 + f64::from(self.gain)) as f32;
        self.gain = flush(g);
        self.step += (self.target - self.step) * 0.005;
        out
    }
}

/// A two-pole filter as `FUN_1800df680` designs a highpass and
/// `FUN_1800df780` runs it.
#[derive(Debug, Clone, Copy, Default)]
struct Highpass {
    c: [f32; 3],
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Highpass {
    fn new(hz: f32, q: f32, rate: f32) -> Self {
        let w = (f64::from(hz) * 2.0 * PI / f64::from(rate)) as f32;
        let (s, c) = w.sin_cos();
        let d = 1.0 / (s + q);
        Highpass {
            c: [
                flush((c + 1.0) / (s / q + 2.0)),
                flush(c * d * q * -2.0),
                flush((q - s) * d),
            ],
            ..Highpass::default()
        }
    }

    fn run(&mut self, x: f32) -> f32 {
        let x = flush(x);
        let [c0, c1, c2] = self.c;
        let y = flush((((x - (self.x1 + self.x1)) + self.x2) * c0 - self.y1 * c1) - c2 * self.y2);
        self.x2 = self.x1;
        self.y2 = self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }
}

/// One channel of the reverb.
#[derive(Debug, Clone, Default)]
struct Verb {
    combs: [(Vec<f32>, usize, f32); 8],
    passes: [(Vec<f32>, usize); 4],
    filter: Highpass,
}

/// The lengths of the combs and allpasses at 44.1 kHz over 1.1, left and
/// right (`FUN_180124e50`).
const LEFT: [f64; 12] = [
    1016.0, 1288.0, 1177.0, 1456.0, 1322.0, 1591.0, 1457.0, 1717.0, 249.0, 210.0, 198.0, 145.0,
];
const RIGHT: [f64; 12] = [
    1216.0, 1088.0, 1377.0, 1256.0, 1522.0, 1391.0, 1657.0, 1517.0, 244.0, 220.0, 178.0, 155.0,
];

impl Verb {
    fn new(lengths: &[f64; 12], rate: f64) -> Self {
        let scale = rate * 2.494_331_065_759_637_3e-5;
        let len = |l: f64| ((l * scale) as i32).max(1) as usize;
        Verb {
            combs: std::array::from_fn(|i| (vec![0.0; len(lengths[i])], 0, 0.0)),
            passes: std::array::from_fn(|i| (vec![0.0; len(lengths[8 + i])], 0)),
            filter: Highpass::new(50.0, 1.0, rate as f32),
        }
    }

    fn clear(&mut self) {
        for (buf, at, store) in &mut self.combs {
            buf.fill(0.0);
            *at = 0;
            *store = 0.0;
        }
        for (buf, at) in &mut self.passes {
            buf.fill(0.0);
            *at = 0;
        }
        self.filter.x1 = 0.0;
        self.filter.x2 = 0.0;
        self.filter.y1 = 0.0;
        self.filter.y2 = 0.0;
    }

    fn run(&mut self, x: f32, mix: f32, feedback: f32, damp: f32) -> f32 {
        let input = self.filter.run(x);
        let mut sum = 0.0;
        for (buf, at, store) in &mut self.combs {
            if buf.is_empty() {
                continue;
            }
            if *at >= buf.len() {
                *at = 0;
            }
            let s = flush((buf[*at] - *store) * damp + *store);
            *store = s;
            buf[*at] = s * feedback + input;
            *at += 1;
            sum += s;
        }
        let mut y = sum;
        for (buf, at) in &mut self.passes {
            if buf.is_empty() {
                continue;
            }
            if *at >= buf.len() {
                *at = 0;
            }
            let b = flush(buf[*at]);
            buf[*at] = b * 0.5 + y;
            *at += 1;
            y = b - y;
        }
        y * mix + x
    }
}

/// One band of the equaliser: 0 low shelf, 1 peak, 2 high shelf.
#[derive(Debug, Clone, Copy, Default)]
struct Band {
    b: [f32; 3],
    a: [f32; 2],
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Band {
    /// RBJ's filters with `A = 10^(dB / 20)` and a bandwidth of an octave
    /// (`FUN_1800dfa60`).
    fn design(kind: u8, hz: f32, db: f32, rate: f32) -> Self {
        let a = 10f32.powf(db * 0.05);
        // FL's own constant (`DAT_180177208`), a little short of 2 pi.
        #[allow(clippy::approx_constant)]
        let w = ((f64::from(hz) * 6.283_185) / f64::from(rate)) as f32;
        let w = w.clamp(0.0, 3.0);
        let sin = w.sin();
        let sh = ((1.0 * 0.346_573_6 * f64::from(w)) / f64::from(sin)).sinh();
        let beta = a.sqrt() * (sh + sh) as f32;
        let alpha = sin * (sh + sh) as f32 * 0.5;
        let cos = w.cos();
        let (b0, b1, b2, a0, a1, a2) = match kind {
            0 => {
                let (am, ap) = (a - 1.0, a + 1.0);
                let s = sin * beta;
                (
                    ((ap - am * cos) + s) * a,
                    (a + a) * (am - ap * cos),
                    ((ap - am * cos) - s) * a,
                    s + (am * cos + ap),
                    am * -2.0 - (ap * cos + ap * cos),
                    (am * cos + ap) - s,
                )
            }
            2 => {
                let (am, ap) = (a - 1.0, a + 1.0);
                let s = sin * beta;
                let up = am * cos + ap;
                let down = ap - am * cos;
                (
                    (s + up) * a,
                    (ap * cos + am) * a * -2.0,
                    (up - s) * a,
                    down + s,
                    (am - ap * cos) + (am - ap * cos),
                    down - s,
                )
            }
            _ => (
                alpha * a + 1.0,
                cos * -2.0,
                1.0 - alpha * a,
                alpha / a + 1.0,
                cos * -2.0,
                1.0 - alpha / a,
            ),
        };
        let n = 1.0 / a0;
        Band {
            b: [flush(b0 * n), flush(b1 * n), flush(b2 * n)],
            a: [flush(a1 * n), flush(a2 * n)],
            ..Band::default()
        }
    }

    fn run(&mut self, x: f32) -> f32 {
        let x = flush(x);
        let y = flush(
            ((self.x1 * self.b[1] + x * self.b[0] + self.b[2] * self.x2) - self.y1 * self.a[0])
                - self.a[1] * self.y2,
        );
        self.x2 = self.x1;
        self.y2 = self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }
}

/// The effects section.
#[derive(Debug, Clone, Default)]
pub struct Fx {
    rate: f64,
    drive: f32,
    drive_now: f32,
    chorus_on: bool,
    chorus: Vec<Tape>,
    delay_on: bool,
    delay: Vec<Tape>,
    reverb_on: bool,
    verb: Vec<Verb>,
    mix: f32,
    feedback: f32,
    damp: f32,
    eq_on: bool,
    eq: [[Band; 8]; 2],
    gains: [f32; 8],
}

impl Fx {
    /// Make the lines for a rate (`FUN_1800ff570`).
    pub fn set_rate(&mut self, rate: f64) {
        self.rate = rate;
        // 0.02 as an f32 widened (`DAT_180177088`): 881 samples at 44.1 kHz.
        let n = rate * f64::from(0.02f32);
        self.chorus = vec![
            Tape::new((n * 1.1) as u32, 0.0),
            Tape::new((n * 0.854) as u32, 0.467),
            Tape::new(n as u32, 0.778),
        ];
        let size = (rate + rate) as u32;
        self.delay = vec![Tape::new(size, 0.0), Tape::new(size, 0.7)];
        self.verb = vec![Verb::new(&LEFT, rate), Verb::new(&RIGHT, rate)];
        self.gains = [f32::NAN; 8];
    }

    /// Take the parameters (`FUN_180111ec0` cases 6 to 0x1d).
    pub fn set_params(&mut self, p: &[f32; PARAMS], rate: f64) {
        if self.chorus.is_empty() {
            return;
        }
        self.drive = p[6] * p[6] * p[6] * 16.0;
        let depth = f64::from(p[7] * p[7] * p[7]).clamp(0.0, 1.0) as f32;
        let wet = -(p[8] * p[8]);
        let speed = f64::from(p[9]) * 5.0 + 0.1;
        for (tape, factor) in self.chorus.iter_mut().zip([0.896, 1.0, 0.689]) {
            tape.depth = depth;
            tape.gain_target = wet;
            tape.rate = (speed * factor).max(0.0) / rate;
        }
        // The switch only gates the chorus (`FUN_1800f2c90`): its tapes
        // keep what they held.
        self.chorus_on = p[10] != 0.0;
        // The delay (`FUN_1800f3080` and the setters beside it).
        let t = f64::from(p[11]) * 0.95 + 0.05;
        let t = t * t * 0.5;
        let s = (f64::from(p[12]) - 0.5) * t;
        self.delay[0].stretch((t + s).clamp(0.05, 1.0));
        self.delay[1].stretch((t - s).clamp(0.05, 1.0));
        let depth = (f64::from(p[13] * p[13]) * 0.02).clamp(0.0, 1.0) as f32;
        let feedback = (f64::from(p[14]) * 0.84) as f32;
        for (tape, hz) in self.delay.iter_mut().zip([1.0, 0.876]) {
            tape.depth = depth;
            tape.feedback = feedback;
            tape.gain_target = p[15] * p[15];
            tape.rate = hz / rate;
        }
        let on = p[16] != 0.0;
        if on != self.delay_on {
            self.delay_on = on;
            self.delay.iter_mut().for_each(Tape::reset);
        }
        // The reverb (`FUN_180124d70`, `FUN_180124d00`, `FUN_180124cd0`).
        self.feedback = (f64::from(p[17].clamp(0.0, 1.0).powf(0.25)) * 0.67 + 0.3) as f32;
        self.damp = ((f64::from(p[18].clamp(0.0, 1.0).powf(0.25)) * 0.8 + 0.2) as f32).min(1.0);
        let m = p[19].clamp(0.0, 1.0);
        self.mix = (f64::from(m * m * m) * 0.2) as f32;
        let on = p[20] != 0.0;
        if on != self.reverb_on {
            self.reverb_on = on;
            self.verb.iter_mut().for_each(Verb::clear);
        }
        // The equaliser: `12 d |d|` dB, d = 2 v - 1 (`FUN_1800f7db0`), at
        // 62.5 Hz to 8 kHz an octave apart (`FUN_1800f7ba0`).
        for band in 0..8 {
            let d = f64::from(p[21 + band]) * 2.0 - 1.0;
            let db = (d * d.abs() * 12.0) as f32;
            if db != self.gains[band] {
                self.gains[band] = db;
                let hz = (250.0 * 2f64.powi(band as i32 - 2)) as f32;
                let kind = match band {
                    0 => 0,
                    7 => 2,
                    _ => 1,
                };
                for channel in &mut self.eq {
                    let old = channel[band];
                    let mut new = Band::design(kind, hz, db, rate as f32);
                    (new.x1, new.x2, new.y1, new.y2) = (old.x1, old.x2, old.y1, old.y2);
                    channel[band] = new;
                }
            }
        }
        let on = p[29] != 0.0;
        if on != self.eq_on {
            self.eq_on = on;
            for channel in &mut self.eq {
                for band in channel.iter_mut() {
                    (band.x1, band.x2, band.y1, band.y2) = (0.0, 0.0, 0.0, 0.0);
                }
            }
        }
    }

    pub fn reset(&mut self) {
        self.drive_now = 0.0;
        self.chorus.iter_mut().for_each(Tape::reset);
        self.delay.iter_mut().for_each(Tape::reset);
        self.verb.iter_mut().for_each(Verb::clear);
        for channel in &mut self.eq {
            for band in channel.iter_mut() {
                (band.x1, band.x2, band.y1, band.y2) = (0.0, 0.0, 0.0, 0.0);
            }
        }
    }

    /// Seconds the effects ring on after the voices stop.
    pub fn tail(&self) -> f64 {
        let mut t = 0.0f64;
        if self.delay_on {
            t = t.max(8.0);
        }
        if self.reverb_on {
            t = t.max(4.0);
        }
        if self.chorus_on {
            t = t.max(0.05);
        }
        t
    }

    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        // The overdrive (`FUN_1800ff800`).
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let d =
                (f64::from(self.drive - self.drive_now) * 0.01 + f64::from(self.drive_now)) as f32;
            self.drive_now = d;
            if d < 1e-9 && d > -1e-9 {
                self.drive_now = 0.0;
                continue;
            }
            if f64::from(d) <= 0.01 {
                continue;
            }
            let m = 1.0 - d * 0.0625;
            let m = m * m;
            let m = m * m;
            let makeup = (f64::from(m * m) * 0.7 + 0.3) as f32;
            let shape = |x: f32| {
                let x = flush(x);
                if x >= 0.0 {
                    1.0 - 10f32.powf(-(d * x)) * (1.0 - x)
                } else {
                    10f32.powf(d * x) * (x + 1.0) - 1.0
                }
            };
            *l = makeup * shape(*l);
            *r = makeup * shape(*r);
        }
        if self.chorus_on {
            for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                let x = *l + *r;
                let a = self.chorus[0].run(x);
                let b = self.chorus[2].run(x);
                let c = self.chorus[1].run(x);
                *l += c + a;
                *r += c + b;
            }
        }
        if self.delay_on {
            for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                *l += self.delay[0].run(*l);
                *r += self.delay[1].run(*r);
            }
        }
        if self.reverb_on {
            let (mix, feedback, damp) = (self.mix, self.feedback, self.damp);
            for l in left.iter_mut() {
                *l = self.verb[0].run(*l, mix, feedback, damp);
            }
            for r in right.iter_mut() {
                *r = self.verb[1].run(*r, mix, feedback, damp);
            }
        }
        if self.eq_on {
            for band in 0..8 {
                for l in left.iter_mut() {
                    *l = self.eq[0][band].run(*l);
                }
                for r in right.iter_mut() {
                    *r = self.eq[1][band].run(*r);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    #[test]
    fn a_flat_equaliser_band_passes_a_tone_as_it_is() {
        let mut band = Band::design(1, 1000.0, 0.0, 44_100.0);
        let x = testing::sine(1000.0 / 44_100.0, 8820);
        let y: Vec<f32> = x.iter().map(|v| band.run(*v)).collect();
        let g = testing::partial(&y[4410..], 1000.0 / 44_100.0);
        assert!((g - 1.0).abs() < 1e-3, "{g}");
    }

    #[test]
    fn a_peak_of_six_db_doubles_its_centre_twice_over() {
        // A = 10^(6 / 20), and the peak's gain is A at its centre: A^2 in
        // the cookbook's terms.
        let mut band = Band::design(1, 1000.0, 6.0, 44_100.0);
        let x = testing::sine(1000.0 / 44_100.0, 8820);
        let y: Vec<f32> = x.iter().map(|v| band.run(*v)).collect();
        let g = testing::partial(&y[4410..], 1000.0 / 44_100.0);
        assert!((g - 10f64.powf(12.0 / 20.0)).abs() < 0.01, "{g}");
    }

    #[test]
    fn a_still_tape_delays_by_one_pass() {
        // 1000 samples of room at half stretch: a pass of 501 samples.
        let mut tape = Tape::new(1000, 0.0);
        tape.gain_target = 1.0;
        tape.gain = 1.0;
        let mut out = Vec::new();
        for n in 0..2000 {
            out.push(tape.run(if n == 10 { 1.0 } else { 0.0 }));
        }
        let peak =
            out.iter().enumerate().fold(
                (0, 0.0f32),
                |m, (i, v)| if v.abs() > m.1 { (i, v.abs()) } else { m },
            );
        assert!((peak.0 as i32 - 511).abs() <= 2, "{peak:?}");
        assert!(peak.1 > 0.5);
    }

    #[test]
    fn a_steady_signal_comes_back_whole_across_the_seams_of_the_passes() {
        // FL keeps the sample that crossed into the next pass at the end of
        // each, so the read head never runs off the end of one.
        let mut tape = Tape::new(1000, 0.0);
        tape.gain_target = 1.0;
        tape.gain = 1.0;
        let out: Vec<f32> = (0..3000).map(|_| tape.run(1.0)).collect();
        assert!(out[600..].iter().all(|v| (v - 1.0).abs() < 1e-6));
    }

    #[test]
    fn a_tape_never_runs_slower_than_one_pass_over_its_room() {
        let mut tape = Tape::new(100, 0.0);
        tape.stretch(0.99);
        tape.step = 0.0;
        tape.run(0.0);
        assert!((tape.now - 0.01).abs() < 1e-12, "{}", tape.now);
    }

    #[test]
    fn the_chorus_tapes_are_sized_as_fl_sizes_them() {
        let mut fx = Fx::default();
        fx.set_rate(44_100.0);
        let sizes: Vec<u32> = fx.chorus.iter().map(|t| t.size).collect();
        assert_eq!(sizes, [970, 753, 881]);
        assert!(fx.delay.iter().all(|t| t.size == 88_200));
    }

    #[test]
    fn the_overdrive_keeps_a_quiet_signal_and_bends_a_loud_one() {
        let mut fx = Fx::default();
        fx.set_rate(44_100.0);
        let mut p = [0.0; PARAMS];
        p[6] = 0.5;
        fx.set_params(&p, 44_100.0);
        let (mut l, mut r) = (vec![0.9f32; 2000], vec![-0.9f32; 2000]);
        fx.process(&mut l, &mut r);
        assert!(l[1999] > 0.0 && l[1999] < 1.0);
        assert!((l[1999] + r[1999]).abs() < 1e-6);
    }
}
