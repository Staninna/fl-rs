//! Harmor's global units on the FX bus (`docs/plugins/harmor.md` section
//! 4): the distortion (`FUN_008944a0`) and the front ends Harmor puts on
//! FL's shared chorus, delay and reverb classes ([`crate::fl_fx`]).

use crate::fl_fx::{
    Delay32, DelayKernel, FlangerSettings, FruityReverb, ReverbSettings, StereoFlanger,
};
use std::f64::consts::PI;

/// The halfband allpass chains (`FUN_00863aa0`, `FUN_00863c70`).
const CHAIN_A: [f64; 3] = [
    0.147_377_113_601_046_6,
    0.482_468_542_769_700_14,
    0.883_005_025_769_373_1,
];
const CHAIN_B: [f64; 3] = [
    0.039_151_597_734_460_045,
    0.302_646_848_328_493_4,
    0.674_615_918_546_963_9,
];

/// Three first-order allpasses `y = c (x - y1) + x1`, in f64.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Chain {
    x1: [f64; 3],
    y1: [f64; 3],
}

impl Chain {
    fn run(&mut self, c: &[f64; 3], x: f64) -> f64 {
        let mut x = x;
        for (k, c) in c.iter().enumerate() {
            let y = c * (x - self.y1[k]) + self.x1[k];
            self.x1[k] = x;
            self.y1[k] = y;
            x = y;
        }
        x
    }
}

/// One halfband stage, up and down, for one side.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Halfband {
    up: [Chain; 2],
    down: [Chain; 2],
}

/// The most the oversampling goes to.
const MAX_OS: usize = 8;
/// Frames a block can have.
const MAX_BLOCK: usize = 256;

/// An RBJ low pass, Q = 1/sqrt 2.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Biquad {
    fn low_pass(hz: f64, rate: f64) -> Self {
        let w = 2.0 * PI * hz / rate;
        let alpha = w.sin() / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
        let cos = w.cos();
        let a0 = 1.0 + alpha;
        let b1 = (1.0 - cos) / a0;
        Biquad {
            b: [b1 * 0.5, b1, b1 * 0.5],
            a: [-2.0 * cos / a0, (1.0 - alpha) / a0],
            z: [0.0; 2],
        }
    }

    fn run(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// The distortion's settings, from REC 789..793 and the mode.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DistSettings {
    pub mode: i32,
    pub drive: i32,
    pub asym: i32,
    pub out: i32,
    pub mix: i32,
    pub cut: i32,
    /// 0 average, 1 high, 2 perfect.
    pub precision: i32,
}

/// The distortion.
#[derive(Debug, Clone, PartialEq)]
pub struct Distortion {
    pub settings: DistSettings,
    os: usize,
    stages: usize,
    bands: [[Halfband; 3]; 2],
    drive: f32,
    wet: f32,
    mix: f32,
    bias: [f32; 2],
    /// Ribbon's B, sincrush's g, bitjam's c.
    extra: f32,
    smoothed: [f32; 3],
    coef: f32,
    low_pass: Option<[[Biquad; 2]; 2]>,
    dc: [[f32; 2]; 2],
    dc_a: f32,
    up: [[f32; MAX_BLOCK * MAX_OS]; 2],
    tmp: [f32; MAX_BLOCK * MAX_OS],
    fresh: bool,
}

fn level3(x: f64) -> f64 {
    if x == 1.0 {
        1.0
    } else {
        (3f64.powf(x) - 1.0) * 0.5
    }
}

impl Default for Distortion {
    fn default() -> Self {
        Distortion {
            settings: DistSettings::default(),
            os: 2,
            stages: 1,
            bands: Default::default(),
            drive: 1.0,
            wet: 1.0,
            mix: 0.0,
            bias: [0.0; 2],
            extra: 0.0,
            smoothed: [0.0; 3],
            coef: 0.0,
            low_pass: None,
            dc: [[0.0; 2]; 2],
            dc_a: 2.2835e-5,
            up: [[0.0; MAX_BLOCK * MAX_OS]; 2],
            tmp: [0.0; MAX_BLOCK * MAX_OS],
            fresh: true,
        }
    }
}

impl Distortion {
    /// Work the setter out (ProcessParam 0x315..0x319). Allocates nothing.
    pub fn set(&mut self, s: DistSettings, rate: f64) {
        self.settings = s;
        let stages = (s.precision + 1).clamp(1, 3) as usize;
        self.stages = stages;
        self.os = 1 << stages;
        let v = f64::from(s.drive);
        let b = f64::from(s.asym) / 500.0;
        let w = level3(f64::from(s.out) / 1000.0);
        let m = f64::from(s.mix) / 1000.0;
        let wet = w * (1.0 - m.abs());
        let k1 = (v / 100.0 - 4.0).exp2();
        let (drive, gain, bias, extra) = match s.mode {
            1..=3 => (PI * k1, wet / (PI * k1.tanh()), 0.75 * b * b, 0.0),
            4 => {
                let k = (v / 100.0 - 2.0).exp2();
                (
                    PI * k,
                    wet / (PI * (4.0 * k).tanh().powi(3)),
                    0.75 * b * b,
                    0.0,
                )
            }
            5 => {
                let k = (v / 40.0 - 5.0).exp2();
                (
                    k,
                    wet / ((k + 1.0).ln() * (v / 1000.0 + 1.0)),
                    0.75 * b * b,
                    0.0,
                )
            }
            6 => {
                let k = (v / 200.0 - 3.0).exp2();
                (PI * k, wet / (PI * k.tanh()), 0.75 * b * b, 0.0)
            }
            7 => {
                let k = PI * (v / 500.0 - 1.0).exp2();
                (k * k, wet / k.sqrt(), 0.75 * b * b, 0.0)
            }
            8 => {
                let d = PI * k1;
                (d, wet / (PI * k1.tanh()), 0.0, b * b * d)
            }
            9 => {
                let d = PI * (7.0 - 0.007 * v).exp2();
                let g = (f64::from(s.asym) / 250.0).exp2();
                (d, wet / (d * g), 0.0, g)
            }
            10 => {
                let d = (9.0 - 0.007 * v).exp2();
                (d, wet / d, b / d, 0.0)
            }
            _ => {
                let q = 1024.0 * (f64::from(s.asym) / 50.0).exp2();
                let c = (10.0 - 0.007 * v).exp2() / (q + 1.0).ln();
                (q, wet / q, 0.0, c)
            }
        };
        self.drive = drive as f32;
        self.wet = gain as f32;
        self.mix = m as f32;
        self.extra = extra as f32;
        // b > 0 biases both sides alike, b <= 0 the right the other way.
        let bias = bias as f32;
        self.bias = if b > 0.0 { [bias, bias] } else { [bias, -bias] };
        let hi_rate = rate * self.os as f64;
        self.coef = (0.01f64.ln() / (0.015 * hi_rate)).exp() as f32;
        self.low_pass = (s.cut != 1100).then(|| {
            let hz = (16.351_6 * (f64::from(s.cut) / 100.0).exp2()).min(0.4999 * hi_rate);
            let f = Biquad::low_pass(hz, hi_rate);
            [[f; 2]; 2]
        });
        self.dc_a = (2.2835e-5 * 44_100.0 / rate) as f32;
        if self.fresh {
            self.smoothed = [self.mix, self.drive, self.wet];
            self.fresh = false;
        }
    }

    fn shape(&self, x: f32) -> f32 {
        let x64 = f64::from(x);
        (match self.settings.mode {
            1 => x64.tanh(),
            2 => x64.tanh().exp() - 1.0,
            3 => (x64.exp() - 1.0).tanh(),
            4 => x64.tanh().powi(3),
            5 => x64.signum() * x64.abs().ln_1p(),
            6 => x64.powi(3).tanh().cbrt(),
            7 => x64.signum() * x64.abs().sqrt().sin(),
            8 => {
                let b = f64::from(self.extra);
                x64.tanh() * (x64 + b).tanh() * (x64 + 2.0 * b).tanh()
            }
            9 => x64.sin() + f64::from(self.extra) * x64,
            10 => x64.round(),
            _ => {
                let c = f64::from(self.extra);
                x64.signum() * ((x64.abs().ln_1p() * c).round() / c).exp_m1()
            }
        }) as f32
    }

    /// Distort a block in place. Allocates nothing.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if self.settings.mode <= 0 {
            return;
        }
        let len = left.len().min(right.len()).min(MAX_BLOCK);
        let os = self.os;
        let n = len * os;
        for (side, buf) in [&*left, &*right].into_iter().enumerate() {
            // Up: each stage doubles; A gives the odd sample, B the even.
            self.up[side][..len].copy_from_slice(&buf[..len]);
            let mut count = len;
            for stage in 0..self.stages {
                let band = &mut self.bands[side][stage];
                self.tmp[..count].copy_from_slice(&self.up[side][..count]);
                for i in 0..count {
                    let x = f64::from(self.tmp[i]);
                    let even = band.up[1].run(&CHAIN_B, x);
                    let odd = band.up[0].run(&CHAIN_A, x);
                    self.up[side][2 * i] = even as f32;
                    self.up[side][2 * i + 1] = odd as f32;
                }
                count *= 2;
            }
        }
        let k = self.coef;
        let aims = [self.mix, self.drive, self.wet];
        for i in 0..n {
            for (s, aim) in self.smoothed.iter_mut().zip(aims) {
                *s = aim + (*s - aim) * k;
            }
            let [m, d, w] = self.smoothed;
            for side in 0..2 {
                let x = self.up[side][i];
                let dry = x * m;
                let shaped = self.shape((x + self.bias[side]) * d);
                let mut y = dry + shaped * w;
                if let Some(lp) = &mut self.low_pass {
                    let a = lp[side][0].run(f64::from(y));
                    y = lp[side][1].run(a) as f32;
                }
                self.up[side][i] = y;
            }
        }
        for (side, buf) in [left, right].into_iter().enumerate() {
            let mut count = n;
            for stage in (0..self.stages).rev() {
                let band = &mut self.bands[side][stage];
                count /= 2;
                for i in 0..count {
                    let even = f64::from(self.up[side][2 * i]);
                    let odd = f64::from(self.up[side][2 * i + 1]);
                    let y =
                        0.5 * (band.down[0].run(&CHAIN_A, even) + band.down[1].run(&CHAIN_B, odd));
                    self.up[side][i] = y as f32;
                }
            }
            // DC block (FUN_0062a3c0).
            let [mut v, mut lp] = self.dc[side];
            for (out, x) in buf[..len].iter_mut().zip(&self.up[side]) {
                v += (x - lp) * self.dc_a;
                lp += v;
                v *= 0.96;
                *out = x - lp;
            }
            self.dc[side] = [v, lp];
        }
    }

    pub fn reset(&mut self) {
        self.bands = Default::default();
        self.dc = [[0.0; 2]; 2];
        if let Some(lp) = &mut self.low_pass {
            for f in lp.iter_mut().flatten() {
                f.z = [0.0; 2];
            }
        }
    }
}

/// Frames the chorus, delay and reverb work in, and the most a block of
/// the host's has here.
pub const FX_BLOCK: usize = 256;

/// Seconds of delay or predelay room: the longest a wheel reaches at 30
/// BPM.
const LONGEST: f64 = 4.0;

fn seconds_of(v: i32, tempo: f64, free: f64) -> f64 {
    if v < 0 {
        ((f64::from(-v) / free).exp2() - 1.0) / if free == 96.0 { 15.0 } else { 63.0 }
    } else {
        f64::from(v) * 0.3125 / tempo.max(1.0)
    }
}

/// RBJ high or low pass for the delay's filters, with alpha = sin w (Q
/// 0.5).
fn rbj(high: bool, hz: f64, rate: f64) -> Biquad {
    let w = 2.0 * PI * (hz.min(0.49 * rate)) / rate;
    let alpha = w.sin();
    let cos = w.cos();
    let a0 = 1.0 + alpha;
    let (b0, b1) = if high {
        ((1.0 + cos) / 2.0 / a0, -(1.0 + cos) / a0)
    } else {
        ((1.0 - cos) / 2.0 / a0, (1.0 - cos) / a0)
    };
    Biquad {
        b: [b0, b1, b0],
        a: [-2.0 * cos / a0, (1.0 - alpha) / a0],
        z: [0.0; 2],
    }
}

/// Harmor's global units after the voices (section 4.1).
pub struct GlobalFx {
    pub dist: Distortion,
    chorus: StereoFlanger,
    chorus_on: bool,
    chorus_dry: f32,
    delay: Delay32,
    delay_on: bool,
    delay_kernel: DelayKernel,
    delay_offset: [usize; 2],
    delay_in: [f32; 2],
    delay_gain: [f32; 2],
    delay_filters: [[Option<Biquad>; 2]; 2],
    delay_cap: f32,
    reverb: Option<FruityReverb>,
    reverb_on: bool,
    reverb_wet: f32,
    reverb_gain: f32,
    reverb_glide: f32,
    mono: [f32; FX_BLOCK],
    wet: [[f32; FX_BLOCK]; 2],
    rate: f64,
}

impl GlobalFx {
    pub fn new(rate: f64) -> Self {
        let mut chorus = StereoFlanger::default();
        chorus.make_room(rate);
        GlobalFx {
            dist: Distortion::default(),
            chorus,
            chorus_on: false,
            chorus_dry: 1.0,
            delay: Delay32::with_capacity((LONGEST * rate) as usize + 2 * FX_BLOCK + 2048),
            delay_on: false,
            delay_kernel: DelayKernel::Straight,
            delay_offset: [0; 2],
            delay_in: [0.0; 2],
            delay_gain: [0.0; 2],
            delay_filters: [[None; 2]; 2],
            delay_cap: 0.001,
            reverb: Some(FruityReverb::new(
                rate as i32,
                (LONGEST * rate) as usize + 1,
            )),
            reverb_on: false,
            reverb_wet: 0.0,
            reverb_gain: 0.0,
            reverb_glide: 0.0,
            mono: [0.0; FX_BLOCK],
            wet: [[0.0; FX_BLOCK]; 2],
            rate,
        }
    }

    /// Take the global parameters (REC 772..824 as `g`), the distortion
    /// mode and the precision. Allocates nothing.
    pub fn set(&mut self, g: &[i32], mode: i32, precision: i32, tempo: f64) {
        let rate = self.rate;
        self.dist.set(
            DistSettings {
                mode,
                drive: g[17],
                asym: g[18],
                out: g[19],
                mix: g[20],
                cut: g[21],
                precision,
            },
            rate,
        );
        // Reverb (FUN_00893470).
        let was = self.reverb_on;
        self.reverb_on = g[22] >= 1;
        let pitch = |v: i32| 16.351_6 * (f64::from(v) / 100.0).exp2();
        let hundred = |v: i32| {
            if v < 1050 {
                (pitch(v) * 0.01) as f32
            } else {
                0.0
            }
        };
        let settings = ReverbSettings {
            low_cut: if g[23] > 0 { pitch(g[23]) as f32 } else { 0.0 },
            high_cut: hundred(g[24]),
            predelay: (seconds_of(g[25], tempo, 64.0) * rate).round() as i32,
            room: g[26].clamp(1, 100),
            diffusion: g[27],
            decay: ((f64::from(g[28]) / 100.0).exp2() * 0.1 * rate) as f32,
            damping: hundred(g[29]),
            colour: g[30],
        };
        if let Some(reverb) = &mut self.reverb
            && *reverb.settings() != settings
        {
            reverb.set(settings);
        }
        let w = f64::from(g[31]) / 1000.0;
        self.reverb_wet = (w * w * w * 0.5) as f32;
        if self.reverb_on && !was {
            self.reverb_gain = self.reverb_wet;
        }
        self.reverb_glide = (0.01f64.ln() / (0.015 * rate)).exp() as f32;
        // Chorus (FUN_008937f0).
        let order = g[32].clamp(0, 9);
        let was = self.chorus_on;
        self.chorus_on = order > 0;
        let v = g[38];
        let w = f64::from(v.abs()) / 1000.0;
        self.chorus_dry = (1.0 - (5f64.powf(w) - 1.0) / 4.0) as f32;
        let wet = f64::from(v.signum()) * (1.0 - (5f64.powf(1.0 - w) - 1.0) / 4.0);
        if self.chorus_on {
            if !was {
                self.chorus.fresh();
            }
            let wheels = [order, g[33], g[34], g[35], g[36], g[37]];
            self.chorus
                .set(&FlangerSettings::from_wheels(wheels, wet as f32), rate);
        }
        // Delay (FUN_00892f90).
        let was = self.delay_on;
        self.delay_on = g[39] >= 1;
        if self.delay_on && !was {
            self.delay.fresh();
        }
        let mode = g[40];
        self.delay_kernel = match mode {
            1 => DelayKernel::Swapped,
            2 => DelayKernel::PingPong,
            _ => DelayKernel::Straight,
        };
        self.delay.set_feedback(g[41] as f32 / 1000.0);
        let time = seconds_of(g[42], tempo, 96.0);
        let frames = ((time * rate).round() as usize).max(1);
        let ms = ((f64::from(g[43].unsigned_abs()) / 64.0).exp2() - 1.0) * 1.960_78;
        let offset = (ms * 0.001 * rate).round() as usize;
        self.delay_offset = if g[43] <= 0 { [offset, 0] } else { [0, offset] };
        let max = self.delay.capacity().saturating_sub(offset + FX_BLOCK + 1);
        self.delay.set_length(offset + FX_BLOCK, frames.min(max));
        let x = f64::from(g[44]) / 1000.0;
        let vol = (x * x * x) as f32;
        let mut pan = g[45] as f32 / 128.0;
        if mode > 0 {
            pan = -pan;
        }
        let angle = (1.0 + f64::from(pan.clamp(-1.0, 1.0))) * PI / 4.0;
        let s = std::f64::consts::SQRT_2 * f64::from(vol);
        self.delay_gain = if pan == 0.0 {
            [vol, vol]
        } else {
            [(s * angle.cos()) as f32, (s * angle.sin()) as f32]
        };
        let low = (g[46] > 0).then(|| rbj(true, pitch(g[46]), rate));
        let high = (g[47] < 1050).then(|| rbj(false, pitch(g[47]), rate));
        let keep = |f: &mut Option<Biquad>, new: Option<Biquad>| {
            *f = match (f.take(), new) {
                (Some(old), Some(mut new)) => {
                    new.z = old.z;
                    Some(new)
                }
                (_, new) => new,
            }
        };
        for side in 0..2 {
            keep(&mut self.delay_filters[side][0], low);
            keep(&mut self.delay_filters[side][1], high);
        }
        self.delay_cap = (0.001 * 44_100.0 / rate) as f32;
        if self.delay_on && !was {
            // A new delay starts with its input level where it is set.
            self.delay_in = self.delay_gain;
        }
    }

    /// Whether unit `unit` of the FX order runs.
    pub fn runs(&self, unit: i32) -> bool {
        match unit {
            0 => self.dist.settings.mode > 0,
            1 => self.chorus_on,
            2 => self.delay_on,
            3 => self.reverb_on,
            _ => false,
        }
    }

    /// The FX order's unit 0 to 3 over a block of at most [`FX_BLOCK`]
    /// frames. Allocates nothing.
    pub fn unit(&mut self, unit: i32, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len()).min(FX_BLOCK);
        match unit {
            0 => self.dist.process(&mut left[..len], &mut right[..len]),
            1 if self.chorus_on => {
                let dry = self.chorus_dry;
                for i in 0..len {
                    let (wl, wr) = self.chorus.frame(left[i], right[i]);
                    left[i] = left[i] * dry + wl;
                    right[i] = right[i] * dry + wr;
                }
            }
            2 if self.delay_on => {
                for (side, buf) in [&*left, &*right].into_iter().enumerate() {
                    let gain = self.delay_gain[side];
                    let x = &mut self.wet[side];
                    for i in 0..len {
                        // The input level ramps 15 ms to 1 %.
                        self.delay_in[side] =
                            gain + (self.delay_in[side] - gain) * self.reverb_glide;
                        let mut y = f64::from(buf[i] * self.delay_in[side]);
                        for f in self.delay_filters[side].iter_mut().flatten() {
                            y = f.run(y);
                        }
                        x[i] = y as f32;
                    }
                    self.delay.add(side, &x[..len], self.delay_offset[side]);
                }
                let [wl, wr] = &mut self.wet;
                self.delay.render(
                    self.delay_kernel,
                    &mut wl[..len],
                    &mut wr[..len],
                    self.delay_cap,
                );
                for i in 0..len {
                    left[i] += wl[i];
                    right[i] += wr[i];
                }
            }
            3 if self.reverb_on => {
                let Some(reverb) = &mut self.reverb else {
                    return;
                };
                for i in 0..len {
                    self.mono[i] = left[i] + right[i];
                }
                let [wl, wr] = &mut self.wet;
                reverb.process(&self.mono[..len], &mut wl[..len], &mut wr[..len]);
                for i in 0..len {
                    self.reverb_gain =
                        self.reverb_wet + (self.reverb_gain - self.reverb_wet) * self.reverb_glide;
                    left[i] += wl[i] * self.reverb_gain;
                    right[i] += wr[i] * self.reverb_gain;
                }
            }
            _ => {}
        }
    }

    /// How long the delay and reverb ring on, in frames.
    pub fn tail(&self) -> usize {
        let mut tail = 0.0;
        if self.delay_on {
            let fb = f64::from(self.delay.feedback()).abs().min(0.999);
            let repeats = if fb > 0.0 {
                (0.001f64.ln() / fb.ln()).min(60.0)
            } else {
                0.0
            };
            tail += self.delay.delay() as f64 * (repeats + 1.0);
        }
        if self.reverb_on
            && let Some(reverb) = &self.reverb
        {
            let s = reverb.settings();
            tail += f64::from(s.decay) * 1.5 + f64::from(s.predelay);
        }
        if self.dist.settings.mode > 0 {
            tail += 0.53 * self.rate;
        }
        tail as usize
    }

    pub fn reset(&mut self) {
        self.dist.reset();
        self.chorus.reset();
        if let Some(reverb) = &mut self.reverb {
            reverb.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{partial, sine};

    fn settings(mode: i32, mix: i32) -> DistSettings {
        DistSettings {
            mode,
            drive: 400,
            asym: 0,
            out: 1000,
            mix,
            cut: 1100,
            precision: 0,
        }
    }

    #[test]
    fn a_dry_distortion_passes_a_low_sine() {
        let mut d = Distortion::default();
        d.set(settings(1, 1000), 48_000.0);
        let f = 480.0 / 48_000.0;
        let mut l = sine(f, 9600);
        let mut r = l.clone();
        for (a, b) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            d.process(a, b);
        }
        let level = partial(&l[4800..9600], f);
        assert!((level - 1.0).abs() < 0.02, "{level}");
    }

    #[test]
    fn a_wet_tanh_flattens_a_loud_sine_and_adds_odd_harmonics() {
        let mut d = Distortion::default();
        d.set(settings(1, 0), 48_000.0);
        let f = 480.0 / 48_000.0;
        let mut l: Vec<f32> = sine(f, 9600).iter().map(|v| v * 4.0).collect();
        let mut r = l.clone();
        for (a, b) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            d.process(a, b);
        }
        let tail = &l[4800..9600];
        let peak = tail.iter().fold(0f32, |m, v| m.max(v.abs()));
        let third = partial(tail, 3.0 * f);
        let second = partial(tail, 2.0 * f);
        assert!(peak < 1.0, "{peak}");
        assert!(third > 0.05, "{third}");
        assert!(second < third * 0.05, "{second} {third}");
    }

    #[test]
    fn the_reverb_rings_on_after_its_input() {
        let mut fx = GlobalFx::new(48_000.0);
        let mut g = crate::harmor_state::GLOBAL_DEFAULTS;
        g[22] = 1;
        g[26] = 50;
        g[27] = 100;
        g[28] = 400;
        g[31] = 1000;
        g[24] = 1050;
        g[29] = 1050;
        fx.set(&g, 0, 0, 120.0);
        assert!(fx.runs(3));
        let mut l = [0.0f32; 256];
        let mut r = [0.0f32; 256];
        l[0] = 1.0;
        r[0] = 1.0;
        fx.unit(3, &mut l, &mut r);
        let mut energy = 0.0;
        for _ in 0..40 {
            let (mut l, mut r) = ([0.0f32; 256], [0.0f32; 256]);
            fx.unit(3, &mut l, &mut r);
            energy += l.iter().chain(&r).map(|v| v * v).sum::<f32>();
        }
        assert!(energy > 1e-4, "{energy}");
        assert!(fx.tail() > 48_000);
    }

    #[test]
    fn the_delay_echoes_after_its_time() {
        let mut fx = GlobalFx::new(48_000.0);
        let mut g = crate::harmor_state::GLOBAL_DEFAULTS;
        g[39] = 1;
        g[41] = 0;
        // 120 BPM, 32 * 0.3125 / 120 s = 4000 frames.
        g[42] = 32;
        g[44] = 1000;
        g[47] = 1050;
        fx.set(&g, 0, 0, 120.0);
        let mut heard = Vec::new();
        for block in 0..20 {
            let (mut l, mut r) = ([0.0f32; 256], [0.0f32; 256]);
            if block == 0 {
                l[0] = 1.0;
                r[0] = 1.0;
            }
            fx.unit(2, &mut l, &mut r);
            heard.extend_from_slice(&l);
        }
        let at = heard
            .iter()
            .skip(1)
            .position(|v| v.abs() > 0.5)
            .map(|i| i + 1);
        assert_eq!(at, Some(4000));
    }
}
