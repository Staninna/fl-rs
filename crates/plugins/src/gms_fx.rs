//! GMS's chain of ten effects and its master section, as
//! `docs/plugins/gms.md` reads them out of `GMS_x64.dll`. Each effect is a
//! unit of FL Groove's (`FUN_1800ff500` dispatches on the unit's number);
//! [`crate::gms`] runs them in order on the synth's sum.
//!
//! Distortion, Crusher, Phaser, Low Pass, Hi Pass, Delay and Reverb are
//! rebuilt, and the five-band master EQ unit 9 carries. Flanger, Panner and
//! Trancer are not read: switched on they pass the sound through, and
//! [`Chain::missing`] says so.

// Limits are written as the binary orders them, which keeps a NaN where
// `clamp` would not.
#![allow(clippy::manual_clamp)]

/// FL's denormal guard: anything nearer 0 than this is 0.
const TINY: f32 = 1e-9;

/// What the units' level followers take as silence.
const QUIET: f32 = 1e-4;

/// How far a unit's wet level moves to its target in a sample.
const GLIDE: f32 = 0.01;

/// The units by number.
pub(crate) const DISTORTION: usize = 0;
pub(crate) const CRUSHER: usize = 1;
pub(crate) const FLANGER: usize = 2;
pub(crate) const PHASER: usize = 3;
pub(crate) const LOW_PASS: usize = 4;
pub(crate) const HI_PASS: usize = 5;
pub(crate) const DELAY: usize = 6;
pub(crate) const REVERB: usize = 7;
pub(crate) const PANNER: usize = 8;
pub(crate) const TRANCER: usize = 9;

/// How many units there are.
pub(crate) const UNITS: usize = 10;

fn flush(x: f32) -> f32 {
    if x < TINY && -TINY < x { 0.0 } else { x }
}

/// `FUN_18012a3f0`: sine and cosine of a float angle. The cosine is the
/// sine a quarter turn on, of the angle's size; a tiny angle takes the
/// series.
fn sincos(w: f32) -> (f32, f32) {
    let size = w.abs();
    if size < 0.000_122_070_31 {
        return (w, 1.0 - size * size * 0.5);
    }
    let quarter = size + std::f32::consts::FRAC_PI_2;
    (f64::from(w).sin() as f32, f64::from(quarter).sin() as f32)
}

/// One of FL Groove's biquads, low pass (`FUN_1800e4eb0`, `FUN_1800e4fb0`)
/// or high pass (`FUN_1800e4cc0`, `FUN_1800e4dc0`).
#[derive(Debug, Clone, Copy, Default)]
struct Biquad {
    high: bool,
    freq: f32,
    q: f32,
    rate: f32,
    b0: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Biquad {
    /// As the unit's constructor leaves it: 100 Hz, q 1, at 44.1 kHz.
    fn new(high: bool) -> Self {
        let mut filter = Biquad {
            high,
            ..Biquad::default()
        };
        filter.set(100.0, 1.0, 44_100.0);
        filter
    }

    fn set(&mut self, freq: f32, q: f32, rate: f32) {
        if freq == self.freq && q == self.q && rate == self.rate {
            return;
        }
        self.freq = freq;
        self.q = q;
        self.rate = rate;
        let w = (f64::from(freq) * std::f64::consts::TAU / f64::from(rate)) as f32;
        let (s, c) = sincos(w);
        let inv = 1.0 / (s + q);
        let top = if self.high { c + 1.0 } else { 1.0 - c };
        self.b0 = flush(top / (s / q + 2.0));
        self.a1 = flush(c * inv * (q * -2.0));
        self.a2 = flush((q - s) * inv);
    }

    fn clear(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }

    fn run(&mut self, x: f32) -> f32 {
        let x = flush(x);
        let ff = if self.high {
            (x - (self.x1 + self.x1)) + self.x2
        } else {
            self.x1 + self.x1 + x + self.x2
        };
        let y = flush((ff * self.b0 - self.y1 * self.a1) - self.a2 * self.y2);
        self.x2 = self.x1;
        self.y2 = self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }
}

/// A band of the master section: an RBJ shelf or peak (`FUN_1800e50a0`,
/// run by `FUN_1800e5440`).
#[derive(Debug, Clone, Copy)]
struct Band {
    kind: u8,
    rate: f32,
    gain: f32,
    freq: f32,
    width: f32,
    dirty: bool,
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Band {
    /// `FUN_1800e5050`: a 1 kHz peak at 44.1 kHz until it is set.
    fn new() -> Self {
        Band {
            kind: 1,
            rate: 44_100.0,
            gain: 0.0,
            freq: 1000.0,
            width: 1.0,
            dirty: true,
            b0: 0.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    fn set(&mut self, kind: u8, gain: f32, freq: f32, width: f32) {
        self.dirty |= kind != self.kind || gain != self.gain || freq != self.freq;
        self.dirty |= width != self.width;
        self.kind = kind;
        self.gain = gain;
        self.freq = freq;
        self.width = width;
    }

    /// The coefficients, when anything has moved.
    fn update(&mut self, rate: f32) {
        self.dirty |= rate != self.rate;
        self.rate = rate;
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let a = 10f64.powf(f64::from(self.gain * 0.05)) as f32;
        let w = (f64::from(self.freq) * SHORT_TAU / f64::from(rate)) as f32;
        let w = if w < 3.0 { w } else { 3.0 };
        let w = if w > 0.0 { w } else { 0.0 };
        let sn = f64::from(w).sin() as f32;
        let arg = f64::from(self.width) * 0.346_573_6 * f64::from(w) / f64::from(sn);
        let sinh2 = (arg.sinh() * 2.0) as f32;
        let root = a.sqrt() * sinh2;
        let alpha = sn * sinh2 * 0.5;
        let (b0, b1, b2, a0, a1, a2);
        match self.kind {
            0 => {
                let cs = f64::from(w).cos() as f32;
                let two = sn * root;
                let less = a - 1.0;
                let more = a + 1.0;
                let lc = less * cs;
                let low = more - lc;
                let high = lc + more;
                let mc = more * cs;
                a0 = two + high;
                b2 = (low - two) * a;
                b0 = (low + two) * a;
                a1 = less * -2.0 - (mc + mc);
                b1 = (a + a) * (less - mc);
                a2 = high - sn * root;
            }
            1 => {
                let up = alpha * a;
                let down = alpha / a;
                a0 = down + 1.0;
                b0 = up + 1.0;
                let cs = f64::from(w).cos() as f32;
                a1 = cs * -2.0;
                b1 = a1;
                a2 = 1.0 - down;
                b2 = 1.0 - up;
            }
            _ => {
                let cs = f64::from(w).cos() as f32;
                let two = sn * root;
                let less = a - 1.0;
                let more = a + 1.0;
                let lc = less * cs;
                let mc = more * cs;
                let high = lc + more;
                let low = more - lc;
                a0 = low + two;
                b0 = (two + high) * a;
                b2 = (high - two) * a;
                let d = less - mc;
                a1 = d + d;
                b1 = (mc + less) * a * -2.0;
                a2 = low - sn * root;
            }
        }
        let inv = 1.0 / a0;
        self.b0 = flush(inv * b0);
        self.b1 = flush(inv * b1);
        self.b2 = flush(inv * b2);
        self.a1 = flush(inv * a1);
        self.a2 = flush(inv * a2);
    }

    fn run(&mut self, x: f32) -> f32 {
        let x = flush(x);
        let y = ((self.x1 * self.b1 + x * self.b0 + self.b2 * self.x2) - self.y1 * self.a1)
            - self.a2 * self.y2;
        let y = flush(y);
        self.x2 = self.x1;
        self.y2 = self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }

    fn clear(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }
}

/// The turn the master section's bands take, to seven places as the
/// binary keeps it.
#[allow(clippy::approx_constant)]
const SHORT_TAU: f64 = 6.283_185;

/// The master section's bands: their centres, widths in octaves and kinds
/// (low shelf, three peaks, high shelf), from `FUN_1800ff500`'s stack.
const BAND_FREQ: [f32; 5] = [80.0, 220.0, 1200.0, 6000.0, 12_000.0];
const BAND_WIDTH: [f32; 5] = [1.0, 2.5, 2.8, 2.5, 2.0];
const BAND_KIND: [u8; 5] = [0, 1, 1, 1, 2];

/// One lap of the tape delay's recording: samples with the tape position
/// each was written at. Ten slots lead, the last three of the lap before.
#[derive(Debug, Clone, Default)]
struct Lap {
    samples: Vec<f32>,
    pos: Vec<f64>,
    count: usize,
    read: usize,
}

/// `MFXTapeDelay` (`FUN_1800feae0`, run by `FUN_1800fed00`): a loop of tape
/// as long as the delay, read one lap behind where it is written, so a
/// change of time changes the pitch of what is on it.
#[derive(Debug, Clone, Default)]
struct Tape {
    len: usize,
    gain: f32,
    feedback: f32,
    low: f32,
    high: f32,
    laps: [Lap; 2],
    write: usize,
    lp: f32,
    hp: f32,
    speed: f64,
    slowest: f64,
    pos: f64,
}

impl Tape {
    fn new(len: usize) -> Self {
        let len = len.max(4);
        let lap = || Lap {
            samples: vec![0.0; len + 256],
            pos: vec![0.0; len + 256],
            count: 0,
            read: 0,
        };
        Tape {
            len,
            low: 1.0,
            laps: [lap(), lap()],
            slowest: 1.0 / len as f64,
            ..Tape::default()
        }
    }

    fn clear(&mut self) {
        for lap in &mut self.laps {
            lap.samples.fill(0.0);
            lap.pos.fill(0.0);
        }
        self.pos = 0.0;
    }

    /// `FUN_1800fef90`: the time as a share of the tape, at most 0.99.
    fn set_time(&mut self, time: f64) {
        let time = if 0.99 <= time { 0.99 } else { time };
        let step = 1.0 / self.len as f64;
        let time = if time <= 0.0 { 0.0 } else { time };
        let step = step + step;
        self.speed = 1.0 / ((time * (1.0 - step) + step) * self.len as f64);
    }

    fn run(&mut self, x: f32) -> f32 {
        let cur = self.pos;
        let back = 1 - self.write;
        let mut out = 0.0f32;
        let lap = &mut self.laps[back];
        while lap.count != 0 {
            let i = lap.read;
            if lap.count < i {
                break;
            }
            let hi = lap.pos[i + 10];
            if cur <= hi {
                let lo = lap.pos[i + 9] as f32;
                let top = hi as f32;
                let frac = if lo < top {
                    (cur - f64::from(lo)) / f64::from(top - lo)
                } else {
                    0.0
                };
                let (a, b) = (lap.samples[i + 9], lap.samples[i + 10]);
                let v = (f64::from(b - a) * frac + f64::from(a)) as f32;
                if TINY <= v || v <= -TINY {
                    out = v;
                }
                break;
            }
            lap.read = i + 1;
        }
        let mut speed = self.speed;
        if 0.2 <= speed {
            speed = 0.2;
        }
        if speed <= self.slowest {
            speed = self.slowest;
        }
        let lp = ((out * self.feedback + x * self.gain) - self.lp) * self.low + self.lp;
        self.pos = speed + cur;
        self.lp = lp;
        let hp = (lp - self.hp) * self.high + self.hp;
        self.hp = hp;
        let w = lp - hp;
        if 1.0 <= speed + cur {
            let (front, rear) = self.laps.split_at_mut(1);
            let (wb, rb) = if self.write == 0 {
                (&mut front[0], &mut rear[0])
            } else {
                (&mut rear[0], &mut front[0])
            };
            let rc = rb.count;
            for k in [9, 8, 7] {
                wb.samples[k] = rb.samples[rc + k];
                wb.pos[k] = rb.pos[rc + k] - 1.0;
            }
            wb.samples[wb.count + 10] = w;
            wb.pos[wb.count + 10] = self.pos;
            wb.read = 0;
            self.write = back;
            self.pos -= 1.0;
            self.laps[back].count = 0;
        }
        let wb = &mut self.laps[self.write];
        let at = (wb.count + 10).min(wb.samples.len() - 1);
        wb.samples[at] = w;
        wb.pos[at] = self.pos;
        if wb.count + 11 < wb.samples.len() {
            wb.count += 1;
        }
        out
    }
}

/// Dattorro's input diffusers and tank lines, as `fxReverb` lays them out,
/// in samples at 29761 Hz.
const LINES: [i32; 12] = [
    142, 107, 379, 277, 672, 4453, 1800, 3720, 908, 4217, 2656, 3163,
];

/// The output taps: which line each reads and how far along, left side
/// first.
const TAPS: [(usize, i32); 14] = [
    (9, 266),
    (9, 2974),
    (10, 1913),
    (11, 2496),
    (5, 1690),
    (6, 187),
    (7, 1566),
    (5, 3627),
    (5, 353),
    (6, 1728),
    (7, 3173),
    (9, 1811),
    (10, 335),
    (11, 621),
];

/// The default preset `fxReverb` starts with (`FUN_180101530`), before GMS
/// sets its own.
const PLATE_DEFAULTS: [f32; 18] = [
    0.857_5, 0.018_78, 0.162, 0.0, 0.495, 1.0, 0.285_7, 0.240_5, 0.070_3, 0.162, 0.857_5, 0.774_4,
    0.572_5, 0.5, 0.0, 0.0, 0.0, 0.0,
];

/// How each of `fxReverb`'s parameters becomes the whole number it runs on
/// (`FUN_180102170`): `floor(v scale + offset)`.
const PLATE_SCALE: [(f32, f32); 18] = [
    (421.0, -360.5),
    (2981.0, 19.5),
    (216.0, 5.5),
    (250.0, 0.5),
    (99.0, 1.5),
    (100.0, 0.5),
    (28.0, 2.5),
    (1975.0, 25.5),
    (199.0, 1.5),
    (216.0, 5.5),
    (421.0, -360.5),
    (421.0, -360.5),
    (421.0, -360.5),
    (200.0, 0.5),
    (1.0, 0.5),
    (1.0, 0.5),
    (1.0, 0.5),
    (1.0, 0.5),
];

/// `FUN_180102ff0`: tenths of a decibel to a gain, nothing at -36.1 dB and
/// below.
fn tenths(db: i32) -> f32 {
    if db < -360 {
        0.0
    } else {
        10f64.powf(f64::from(db as f32 * 0.005)) as f32
    }
}

/// A one-pole lowpass coefficient for a frequency in hundreds of hertz, 1
/// from 221 up.
fn damping(hundreds: i32, rate: f32) -> f32 {
    if (hundreds as f32) < 221.0 {
        let c = f64::from(hundreds as f32 * 628.318_54 / rate).cos() as f32;
        let d = 2.0 - c;
        1.0 - (d - (d * d - 1.0).sqrt())
    } else {
        1.0
    }
}

/// `fxReverb`: a plate after Dattorro, with FL's own taps, a shelf and a
/// lowpass in the tank and whole-number parameters. Its early reflections
/// GMS keeps off and they are not built here.
#[derive(Debug, Clone, Default)]
struct Plate {
    rate: f32,
    values: [f32; 18],
    ints: [i32; 18],
    fresh: bool,
    size: i32,
    lines: Vec<Vec<f32>>,
    lens: [usize; 12],
    at: [usize; 12],
    pre: Vec<f32>,
    pre_len: usize,
    pre_at: usize,
    taps: [usize; 14],
    in_gain: f32,
    dry: f32,
    wet: f32,
    hp: f32,
    lp_in: f32,
    diffuse: [f32; 4],
    decay: f32,
    shelf: f32,
    allpass: f32,
    lp_tank: f32,
    width: f32,
    lp_state: f32,
    ap_x: f32,
    ap_y: f32,
    tank_lp: [f32; 2],
    tank_ap: [(f32, f32); 2],
    level: f32,
}

impl Plate {
    fn new(rate: f32) -> Self {
        let sr = rate as i32;
        let lines = LINES
            .iter()
            .map(|n| vec![0.0; ((sr * n) / 20_000).max(1) as usize])
            .collect();
        let pre = (sr * 250 / 1000).max(1) as usize;
        let mut plate = Plate {
            rate,
            values: PLATE_DEFAULTS,
            lines,
            pre: vec![0.0; pre],
            pre_len: 1,
            size: -1,
            level: -1000.0,
            ..Plate::default()
        };
        for (i, v) in PLATE_DEFAULTS.iter().enumerate() {
            plate.set(i, *v);
        }
        plate.fresh = true;
        plate
    }

    fn set(&mut self, index: usize, value: f32) {
        self.values[index] = value;
        let (scale, offset) = PLATE_SCALE[index];
        let int = (value * scale + offset).floor() as i32;
        if int != self.ints[index] {
            self.ints[index] = int;
            self.fresh = true;
        }
    }

    /// `FUN_180103020`, when a whole number has moved.
    fn update(&mut self) {
        if !self.fresh {
            return;
        }
        self.fresh = false;
        let p = self.ints;
        let rate = self.rate;
        let sr = rate as i32;
        self.in_gain = if p[14] != 0 { 0.0 } else { tenths(p[0]) };
        self.hp = if (p[1] as f32) > 19.0 {
            let f = (rate * 0.5).min(p[1] as f32);
            let x = f64::from(f * std::f32::consts::PI / rate).tan() as f32;
            (x - 1.0) / (x + 1.0)
        } else {
            0.0
        };
        self.lp_in = damping(p[2], rate);
        self.pre_len = ((sr * p[3]) / 1000).max(1).min(self.pre.len() as i32) as usize;
        self.pre_at %= self.pre_len;
        let span = ((p[4] + 15) * sr) / 115;
        for (k, n) in LINES.iter().enumerate() {
            let len = ((span * n) / 20_000).max(1) as usize;
            self.lens[k] = len.min(self.lines[k].len());
            self.at[k] %= self.lens[k];
        }
        for (k, (_, n)) in TAPS.iter().enumerate() {
            self.taps[k] = ((span * n) / 20_000).max(0) as usize;
        }
        if p[4] != self.size {
            self.size = p[4];
            for line in &mut self.lines {
                line.fill(0.0);
            }
        }
        let inv = 1.0 / rate;
        let m = p[5] as f32 * 0.01;
        let len = |k: usize| self.lens[k] as i32;
        if m == 0.0 {
            self.diffuse = [0.0; 4];
        } else {
            let e = |x: f32| f64::from(x).exp() as f32;
            self.diffuse[0] = e((len(1) + len(0)) as f32 * (-49.341_11 / m) * inv);
            self.diffuse[1] = e((-31.398_888 / m) * (len(2) + len(3)) as f32 * inv);
            let first = (len(4) + len(8)) as f32 * -3.453_877_7 / rate * (2.857_143 / m);
            self.diffuse[2] = e(first);
            let second = (-3.453_877_7 / rate) * (len(6) + len(10)) as f32 * (1.960_784_3 / m);
            self.diffuse[3] = e(second);
        }
        let rt = p[8] as f32 * 0.1;
        let lap = (len(9) + len(7) + len(5) + len(11)) as f32 / rate * -6.907_755_4;
        let decay = (f64::from(lap / rt).exp().powf(0.25)) as f32;
        self.decay = decay;
        let low = p[6] as f32 * rt;
        let ratio = (f64::from(10.0 / low * lap).exp().powf(0.25)) as f32 / decay;
        self.shelf = (ratio - 1.0) * 0.5;
        let r = ratio.min(1.0);
        let x = f64::from(p[7] as f32 * std::f32::consts::PI / rate).tan() as f32;
        self.allpass = (x - r) / (r + x);
        self.lp_tank = damping(p[9], rate);
        self.dry = if p[15] != 0 { 0.0 } else { tenths(p[10]) };
        self.wet = if p[17] != 0 { 0.0 } else { tenths(p[12]) * 0.3 };
        self.width = p[13] as f32 * 0.01;
    }

    /// `FUN_180101f60`: the lines filled with FL's anti-denormal value.
    fn clear(&mut self) {
        for line in &mut self.lines {
            line.fill(1e-30);
        }
        self.pre.fill(1e-30);
        self.lp_state = 0.0;
        self.ap_x = 0.0;
        self.ap_y = 0.0;
        self.tank_lp = [0.0; 2];
        self.tank_ap = [(0.0, 0.0); 2];
        self.level = -1000.0;
    }

    /// A diffusing allpass of FL's: `t = z - a x`, `z = b t + x`.
    fn diffuse(&mut self, k: usize, x: f32, a: f32, b: f32) -> f32 {
        let at = self.at[k];
        let t = self.lines[k][at] - a * x;
        self.lines[k][at] = b * t + x;
        self.step(k);
        t
    }

    fn step(&mut self, k: usize) {
        self.at[k] = if self.at[k] == 0 {
            self.lens[k] - 1
        } else {
            self.at[k] - 1
        };
    }

    fn tap(&self, k: usize) -> f32 {
        let (line, _) = TAPS[k];
        let mut at = self.taps[k] + self.at[line];
        if at >= self.lens[line] {
            at -= self.lens[line];
        }
        self.lines[line][at.min(self.lens[line] - 1)]
    }

    /// `FUN_180103f30`: add the reverb of `input` to `out`.
    fn run(&mut self, input: (&[f32], &[f32]), out: (&mut [f32], &mut [f32])) {
        self.update();
        let g = self.decay;
        let rest = 1.0 - g;
        let c37 = rest * 0.6 + g;
        let c38 = rest * 0.25 + g;
        let c39 = rest * 0.7 + g;
        let c52 = rest * 0.2 + g;
        let c40 = rest * 0.5 + g;
        let [d1, d2, dd1, dd2] = self.diffuse;
        let (left, right) = out;
        for i in 0..left.len() {
            let il = self.in_gain * input.0[i];
            let ir = self.in_gain * input.1[i];
            let mut m = (((ir + il) * 0.5 + 1e-30) - self.lp_state) * self.lp_in + self.lp_state;
            self.lp_state = m;
            let h = (m - self.ap_y) * self.hp + self.ap_x;
            self.ap_x = m;
            self.ap_y = h;
            if self.hp != 0.0 {
                m = (m - h) * 0.5 + 1e-30;
            }
            let a = self.diffuse(0, m, d1, d1);
            let b = self.diffuse(1, a, d2, d1);
            let c = self.diffuse(2, b, d1, d1);
            let e = self.diffuse(3, c, d2, d1);
            let mut v = self.pre[self.pre_at];
            self.pre[self.pre_at] = e;
            self.pre_at = if self.pre_at == 0 {
                self.pre_len - 1
            } else {
                self.pre_at - 1
            };
            v += 1e-20;
            // Two halves of the tank, each fed by the other's end.
            let mut feed = self.lines[11][self.at[11]] * g;
            for half in 0..2 {
                let (x1, y1) = self.tank_ap[half];
                let ap = (feed - y1) * self.allpass + x1;
                self.tank_ap[half] = (feed, ap);
                let lp = self.tank_lp[half];
                let u = (((ap + feed) * self.shelf + feed + v) - lp) * self.lp_tank + lp;
                self.tank_lp[half] = u;
                let base = if half == 0 { 4 } else { 8 };
                let w = self.diffuse(base, u, dd1, dd1);
                let at = self.at[base + 1];
                let y = self.lines[base + 1][at] * g;
                self.lines[base + 1][at] = w;
                self.step(base + 1);
                let z = self.diffuse(base + 2, y, dd2, dd2);
                let end = base + 3;
                let at = self.at[end];
                self.lines[end][at] = z;
                self.step(end);
                feed = self.lines[end][self.at[end]] * g;
            }
            let t: [f32; 14] = std::array::from_fn(|k| self.tap(k));
            if self.level < -99.0 {
                self.level = self.wet;
            }
            let target = self.wet;
            self.level = (target - self.level) * 0.001 + self.level;
            let wl = self.level
                * ((((((t[0] - c38 * t[1]) - c39 * t[2]) - c52 * t[3]) + c37 * t[4]) + t[5])
                    + c40 * t[6]);
            let wr = self.level
                * ((((((c38 * t[7] + t[8]) - c39 * t[9]) - c52 * t[10]) - c37 * t[11]) + t[12])
                    + c40 * t[13]);
            let s = wr + wl;
            let d = (wl - wr) * self.width;
            left[i] += (il * self.dry + d) + s;
            right[i] += ir * self.dry + (s - d);
        }
    }
}

/// One unit of the chain, with the settings `FUN_1800f5f00` gives it each
/// block.
#[derive(Debug, Clone)]
pub(crate) struct Unit {
    kind: usize,
    rate: f32,
    pub(crate) x: f32,
    pub(crate) y: f32,
    /// The LFO's length in beats.
    pub(crate) length: f32,
    seconds: f32,
    pub(crate) amount: f32,
    pub(crate) shape: i32,
    pub(crate) bypass: bool,
    pub(crate) phase: f64,
    step: f64,
    mix: f32,
    tail: i32,
    pub(crate) active: bool,
    filters: [Biquad; 6],
    hold: f32,
    held: (f32, f32),
    stages: [[f32; 6]; 2],
    last: [f32; 2],
    depth: f32,
    tapes: Option<Box<[Tape; 2]>>,
    plate: Option<Box<Plate>>,
    /// The master section, on unit 9 only: on, the five values and their
    /// bands for each side.
    pub(crate) master: bool,
    pub(crate) bands: [f32; 5],
    eq: [[Band; 2]; 5],
}

impl Unit {
    fn new(kind: usize) -> Self {
        let high = kind == HI_PASS;
        Unit {
            kind,
            rate: 0.0,
            x: 0.0,
            y: 0.0,
            length: 1.0,
            seconds: 0.0,
            amount: 0.0,
            shape: 0,
            bypass: false,
            phase: 0.0,
            step: 0.0,
            mix: 0.0,
            tail: 0,
            active: false,
            filters: [Biquad::new(high); 6],
            hold: 0.0,
            held: (0.0, 0.0),
            stages: [[0.0; 6]; 2],
            last: [0.0; 2],
            depth: -1.0,
            tapes: None,
            plate: None,
            master: false,
            bands: [0.5; 5],
            eq: [[Band::new(); 2]; 5],
        }
    }

    /// `FUN_1800ff2c0`: a new rate lays the lines out again; every unit's
    /// state starts over.
    fn set_rate(&mut self, rate: f32) {
        if rate != 0.0 && rate != self.rate {
            self.rate = rate;
            match self.kind {
                DELAY => {
                    let len = (rate * 1.5) as usize;
                    self.tapes = Some(Box::new([Tape::new(len), Tape::new(len)]));
                }
                // The unit makes its reverb after it has taken the first
                // rate, so the reverb keeps the 44.1 kHz it is made with
                // until the rate changes again.
                REVERB => {
                    let made = if self.plate.is_some() { rate } else { 44_100.0 };
                    self.plate = Some(Box::new(Plate::new(made)));
                }
                _ => {}
            }
        }
        if let Some(tapes) = &mut self.tapes {
            for tape in tapes.iter_mut() {
                tape.clear();
            }
        }
        if let Some(plate) = &mut self.plate {
            plate.clear();
        }
        for filter in &mut self.filters {
            filter.clear();
        }
        self.stages = [[0.0; 6]; 2];
        self.last = [0.0; 2];
        self.depth = -1.0;
        self.hold = 0.0;
        self.held = (0.0, 0.0);
        for band in &mut self.eq {
            for side in band {
                side.clear();
                side.dirty = true;
            }
        }
        self.phase = 0.0;
        self.mix = 0.0;
        self.tail = 0;
        self.active = false;
    }

    /// `FUN_1800fea00`: the unit's LFO, one step on. The sine reads the
    /// phase before the step, the others after it.
    fn lfo(&mut self) -> f32 {
        let next = self.step + self.phase;
        let frac = next - f64::from(next as i32);
        let old = self.phase;
        self.phase = frac;
        match self.shape {
            0 => ((old * std::f64::consts::TAU).sin() as f32) * self.amount,
            1 => {
                let v = frac as f32;
                let v = v + v;
                if 1.0 < v {
                    (2.0 - v) * self.amount
                } else {
                    v * self.amount
                }
            }
            2 => frac as f32 * self.amount,
            _ => {
                if 0.5 < frac as f32 {
                    self.amount
                } else {
                    0.0
                }
            }
        }
    }

    /// The LFO moved on by the rest of a block after one [`Unit::lfo`].
    fn skip(&mut self, frames: usize) {
        let next = (frames as i32 - 1) as f64 * self.step + self.phase;
        self.phase = next - f64::from(next as i32);
    }

    fn glide(&mut self) -> f32 {
        let on = if self.bypass { 0.0 } else { 1.0 };
        self.mix = (on - self.mix) * GLIDE + self.mix;
        self.mix
    }

    /// Whether a unit that fades runs this block.
    fn awake(&self) -> bool {
        !self.bypass || QUIET <= self.mix
    }

    /// `FUN_1800ff500`: one block through the unit.
    fn run(&mut self, left: &mut [f32], right: &mut [f32], tempo: f32) -> bool {
        let mut missing = false;
        if !(1.0 <= self.rate && 1.0 <= tempo) {
            return missing;
        }
        let seconds = self.length * 60.0;
        self.active = false;
        self.seconds = seconds / tempo;
        self.step = if self.seconds == 0.0 {
            0.0
        } else {
            f64::from(1.0 / (self.rate * self.seconds))
        };
        let frames = left.len();
        match self.kind {
            DISTORTION => self.distortion(left, right),
            CRUSHER => self.crusher(left, right),
            PHASER => self.phaser(left, right),
            LOW_PASS | HI_PASS => self.pass(left, right),
            DELAY => self.delay(left, right),
            REVERB => self.reverb(left, right),
            _ => {
                if !self.bypass {
                    missing = true;
                    self.active = true;
                }
            }
        }
        if self.master {
            self.active = true;
            for (b, band) in self.eq.iter_mut().enumerate() {
                let gain = (self.bands[b] - 0.5) * 24.0;
                let freq = BAND_FREQ[b].max(0.0);
                let width = BAND_WIDTH[b].max(QUIET);
                for side in band.iter_mut() {
                    side.set(BAND_KIND[b], gain, freq, width);
                    side.update(self.rate);
                }
                for i in 0..frames {
                    left[i] = band[0].run(left[i]);
                    right[i] = band[1].run(right[i]);
                }
            }
        }
        missing
    }

    fn distortion(&mut self, left: &mut [f32], right: &mut [f32]) {
        if !self.awake() {
            return;
        }
        self.active = true;
        let v = self.lfo() + self.x;
        let v = if v <= 0.0 { 0.0 } else { v };
        let v = if 1.0 <= v { 1.0 } else { v };
        self.skip(left.len());
        let k = v * v * v * v * 1024.0;
        let makeup = 0.5 / (1.0 / (k + 1.0));
        let freq = 2f64.powf(f64::from(self.y * 4.28 + 10.0)) as f32;
        self.filters[0].set(freq, 0.99, self.rate);
        self.filters[1].set(freq, 0.99, self.rate);
        for i in 0..left.len() {
            let mix = self.glide();
            let x = left[i];
            let y = self.filters[0].run((x / (x.abs() * k + 1.0)) * makeup);
            left[i] = y * mix + (1.0 - mix) * x;
            let x = right[i];
            let y = self.filters[1].run((x / (x.abs() * k + 1.0)) * makeup);
            right[i] = y * mix + (1.0 - mix) * x;
        }
    }

    fn crusher(&mut self, left: &mut [f32], right: &mut [f32]) {
        if !self.awake() {
            return;
        }
        self.active = true;
        let v = (1.0 - self.x) + self.lfo();
        let v = if v <= 0.0 { 0.0 } else { v };
        self.skip(left.len());
        let v = if 1.0 <= v { 1.0 } else { v };
        let period = v * v * 64.0 + 3.0;
        let freq = 2f64.powf(f64::from(self.y * 4.28 + 10.0)) as f32;
        self.filters[0].set(freq, 0.99, self.rate);
        self.filters[1].set(freq, 0.99, self.rate);
        for i in 0..left.len() {
            let mut count = self.hold;
            let l = if 0.0 < count {
                self.held.0
            } else {
                self.held = (left[i], right[i]);
                count = period;
                left[i]
            };
            self.hold = count - 1.0;
            let mix = self.glide();
            let y = self.filters[0].run(l);
            left[i] = (1.0 - mix) * left[i] + mix * y;
            let y = self.filters[1].run(self.held.1);
            right[i] = (1.0 - mix) * right[i] + mix * y;
        }
    }

    fn phaser(&mut self, left: &mut [f32], right: &mut [f32]) {
        if !self.awake() {
            return;
        }
        self.active = true;
        let half = f64::from(0.5 / self.rate);
        let hi = (half * 40_000.0) as f32;
        let lo = (half * 100.0) as f32;
        let feedback = (f64::from(self.y * 0.6).powf(f64::from(0.333_333_34f32)) as f32) * 0.98;
        let mut target = self.x * self.x;
        if target <= 1e-5 {
            target = 1e-5;
        }
        if self.depth < 0.0 {
            self.depth = target;
        }
        for i in 0..left.len() {
            let depth = self.depth;
            let mix = self.glide();
            let lfo = self.lfo();
            let mut w = (depth + lfo * depth * 0.5) * (hi - lo) + lo;
            if 0.499 <= w {
                w = 0.499;
            }
            if f64::from(w) < 0.001 {
                w = 0.001;
            }
            let a = (1.0 - w) / (w + 1.0);
            self.depth = (f64::from(target - depth) * 0.001 + f64::from(depth)) as f32;
            for (side, buf) in [(0, &mut *left), (1, &mut *right)] {
                let x = (buf[i] - 1e-6) + feedback * self.last[side];
                let s = &mut self.stages[side];
                let mut t = s[0] - a * x;
                s[0] = t * a + x;
                for z in s.iter_mut().skip(1) {
                    let next = *z - t * a;
                    *z = next * a + t;
                    t = next;
                }
                self.last[side] = t;
                buf[i] += t * mix;
            }
        }
    }

    fn pass(&mut self, left: &mut [f32], right: &mut [f32]) {
        if !self.awake() {
            return;
        }
        self.active = true;
        let v = self.lfo() + self.x;
        let v = if v > 0.0 { v } else { 0.0 };
        let q = self.y * self.y;
        let q = q + q + 1.0;
        let v = if v < 1.0 { v } else { 1.0 };
        self.skip(left.len());
        let freq = if self.kind == LOW_PASS {
            2f64.powf(f64::from(v * 8.28 + 6.0)) as f32
        } else {
            2f64.powf(f64::from(v * 7.424 + 5.0)) as f32
        };
        for filter in &mut self.filters {
            filter.set(freq, q, self.rate);
        }
        for i in 0..left.len() {
            let mix = self.glide();
            let mut l = left[i];
            for k in [2, 1, 0] {
                l = self.filters[k].run(l);
            }
            left[i] = (1.0 - mix) * left[i] + mix * l;
            let mut r = right[i];
            for k in [5, 4, 3] {
                r = self.filters[k].run(r);
            }
            right[i] = (1.0 - mix) * right[i] + mix * r;
        }
    }

    fn delay(&mut self, left: &mut [f32], right: &mut [f32]) {
        if self.bypass && self.tail < 0 {
            return;
        }
        let Some(mut tapes) = self.tapes.take() else {
            return;
        };
        self.active = true;
        let mut time = self.seconds * 0.666_666_7;
        if f64::from(time) < 0.01 {
            time = GLIDE;
        }
        let y2 = self.y + self.y;
        let feedback = self.x * self.x;
        let low = if 1.0 <= y2 { 1.0 } else { y2 };
        let high = if y2 - 1.0 <= 0.0 { 0.0 } else { y2 - 1.0 };
        for tape in tapes.iter_mut() {
            tape.low = low * low;
            tape.high = high * high;
            tape.feedback = feedback;
            tape.set_time(f64::from(time));
        }
        let hold = (self.rate * 1.5) as i32;
        for i in 0..left.len() {
            let gain = self.glide() * feedback;
            tapes[0].gain = gain;
            tapes[1].gain = gain;
            let l = tapes[0].run(left[i]);
            let r = tapes[1].run(right[i]);
            let loud = [l, r, left[i], right[i]].iter().any(|v| QUIET < v.abs());
            self.tail = if loud { hold } else { self.tail - 1 };
            left[i] += l;
            right[i] += r;
        }
        self.tapes = Some(tapes);
    }

    fn reverb(&mut self, left: &mut [f32], right: &mut [f32]) {
        if self.bypass && self.tail < 1 {
            return;
        }
        let Some(mut plate) = self.plate.take() else {
            return;
        };
        self.active = true;
        let frames = left.len();
        let mut wet_l = [0.0f32; 512];
        let mut wet_r = [0.0f32; 512];
        for i in 0..frames {
            let mix = self.glide();
            wet_l[i] = mix * left[i];
            wet_r[i] = mix * right[i];
        }
        let size = self.x * self.x * self.x * 0.75;
        plate.set(9, self.y * self.y);
        plate.set(8, size);
        plate.set(12, size * 0.3 + 0.5);
        plate.set(2, 1.0);
        plate.set(1, 0.1);
        plate.set(5, 1.0);
        plate.set(4, 1.0);
        plate.set(3, 0.0);
        plate.set(10, 0.0);
        plate.set(16, 1.0);
        plate.run((&wet_l[..frames], &wet_r[..frames]), (left, right));
        let hold = (self.rate * 0.25) as i32;
        for i in 0..frames {
            let loud = QUIET < left[i].abs() || QUIET < right[i].abs();
            self.tail = if loud { hold } else { self.tail - 1 };
        }
        self.plate = Some(plate);
    }
}

/// The ten units in their order.
#[derive(Debug, Clone)]
pub(crate) struct Chain {
    pub(crate) units: Vec<Unit>,
    /// Whether a unit that is not rebuilt was switched on.
    pub(crate) missing: bool,
}

impl Chain {
    pub(crate) fn new() -> Self {
        Chain {
            units: (0..UNITS).map(Unit::new).collect(),
            missing: false,
        }
    }

    pub(crate) fn set_rate(&mut self, rate: f32) {
        for unit in &mut self.units {
            unit.set_rate(rate);
        }
    }

    /// The block after the voices (`FUN_1800f32f0`): a unit hears it is
    /// off while nothing before it has sounded. Whether anything sounded.
    pub(crate) fn run(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        tempo: f32,
        voices: bool,
    ) -> bool {
        let mut any = voices;
        for unit in &mut self.units {
            if !any {
                unit.bypass = true;
            }
            self.missing |= unit.run(left, right, tempo);
            any |= unit.active;
        }
        any
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    #[test]
    fn the_biquads_pass_and_stop_where_their_corner_says() {
        for high in [false, true] {
            let mut f = Biquad::new(high);
            f.set(1000.0, 0.99, 44_100.0);
            let gain = |turns: f64| {
                let num: [f64; 3] = if high {
                    [1.0, -2.0, 1.0]
                } else {
                    [1.0, 2.0, 1.0]
                };
                let num = num.map(|c| c * f64::from(f.b0));
                let den = [1.0, f64::from(f.a1), f64::from(f.a2)];
                testing::response(&num, &den, turns)
            };
            // FL's b0 takes half the damping its poles do, so the pass band
            // stands at 2 (1 + s/q) / (s/q + 2) and not at 1.
            let w = std::f64::consts::TAU * 1000.0 / 44_100.0;
            let sq = w.sin() / 0.99;
            let flat = 2.0 * (1.0 + sq) / (sq + 2.0);
            let (pass, stop) = if high { (0.5, 0.0) } else { (0.0, 0.5) };
            let pass = if high { gain(pass) } else { gain(pass + 1e-9) };
            assert!((pass - flat).abs() < 1e-3, "{high} {pass} {flat}");
            assert!(
                gain(stop + if high { 1e-4 } else { -1e-4 }) < 1e-3,
                "{high}"
            );
            let corner = if high {
                gain(100.0 / 44_100.0)
            } else {
                gain(10_000.0 / 44_100.0)
            };
            assert!(corner < 0.02, "{high} {corner}");
        }
    }

    #[test]
    fn a_flat_band_changes_nothing_and_a_boost_lifts_its_centre() {
        let mut band = Band::new();
        band.set(1, 0.0, 1200.0, 2.8);
        band.update(44_100.0);
        let tone = testing::sine(1200.0 / 44_100.0, 4410);
        let out: Vec<f32> = tone.iter().map(|x| band.run(*x)).collect();
        assert!((testing::partial(&out[2205..], 1200.0 / 44_100.0) - 1.0).abs() < 1e-3);
        let mut band = Band::new();
        band.set(1, 6.0, 1200.0, 2.8);
        band.update(44_100.0);
        let out: Vec<f32> = tone.iter().map(|x| band.run(*x)).collect();
        // FL's A is 10^(dB / 20) where RBJ's is 10^(dB / 40), so the peak
        // lifts by A^2: 12 dB for 6.
        let lift = testing::partial(&out[2205..], 1200.0 / 44_100.0);
        assert!((lift - 10f64.powf(0.6)).abs() < 0.02, "{lift}");
    }

    #[test]
    fn the_tape_gives_back_what_it_took_a_delay_later() {
        let mut tape = Tape::new(1000);
        tape.gain = 1.0;
        tape.set_time(0.3);
        // A fresh tape has no lap before its first, which FL starts from
        // -1: let two laps go by first.
        for _ in 0..1000 {
            tape.run(0.0);
        }
        let mut out = Vec::new();
        for n in 0..2000 {
            out.push(tape.run(if n == 0 { 1.0 } else { 0.0 }));
        }
        let at = out.iter().position(|v| v.abs() > 1e-6).unwrap();
        // 0.3 of the tape with the two-sample floor: a lap is
        // (0.3 (1 - 0.002) + 0.002) 1000 samples. The impulse comes back
        // read between two of them.
        assert!((298..=302).contains(&at), "{at}");
        assert!(
            (out[at] + out[at + 1] - 1.0).abs() < 1e-3,
            "{:?}",
            &out[at..at + 2]
        );
        assert!(out[at + 2..].iter().all(|v| v.abs() < 1e-6));
    }

    #[test]
    fn the_plate_rings_on_and_dies_away() {
        let mut plate = Plate::new(44_100.0);
        // A decay of 1.1 s.
        plate.set(8, 0.05);
        plate.set(12, 0.725);
        plate.set(16, 1.0);
        plate.set(10, 0.0);
        let mut impulse = vec![0.0f32; 512];
        impulse[0] = 1.0;
        let silence = vec![0.0f32; 512];
        let mut energy = Vec::new();
        for block in 0..400 {
            let input = if block == 0 { &impulse } else { &silence };
            let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
            plate.run((input, input), (&mut l, &mut r));
            assert!(l.iter().chain(&r).all(|v| v.is_finite()));
            energy.push(l.iter().chain(&r).map(|v| v * v).sum::<f32>());
        }
        let early: f32 = energy[2..40].iter().sum();
        let late: f32 = energy[360..].iter().sum();
        assert!(early > 1e-4, "{early}");
        assert!(late < early * 1e-3, "{early} {late}");
    }

    #[test]
    fn a_unit_that_is_off_fades_out_and_stops() {
        let mut chain = Chain::new();
        chain.set_rate(44_100.0);
        let unit = &mut chain.units[DISTORTION];
        unit.x = 1.0;
        unit.y = 1.0;
        let tone = testing::sine(440.0 / 44_100.0, 512);
        let (mut l, mut r) = (tone.clone(), tone.clone());
        unit.run(&mut l, &mut r, 120.0);
        assert!(unit.active);
        assert_ne!(l, tone);
        unit.bypass = true;
        for _ in 0..4 {
            let (mut l, mut r) = (tone.clone(), tone.clone());
            unit.run(&mut l, &mut r, 120.0);
        }
        let (mut l, mut r) = (tone.clone(), tone.clone());
        unit.run(&mut l, &mut r, 120.0);
        assert!(!unit.active);
        assert_eq!(l, tone);
    }
}
