//! Hyper Chorus: four modulated delay lines a side, or eight, then a
//! low-pass, a high-pass and a dry/wet mix, as `docs/plugins/hyper-chorus.md`
//! has it.
//!
//! The plugin is a Delphi shell around a C++ engine, `Engine_x64.dll`
//! beside it ("QuadrachorEngine"), which does all the sound. The shell
//! turns each wheel into a number from 0 to 1 and hands it to the engine.
//!
//! The state is 52 bytes: a little-endian i32 version, 2, the eleven
//! parameters as i32 and the visualiser's gain as i32. FL leaves a state
//! alone whose version is 3 or more, and so does this.
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Delay | 0 to 1000 | 400 |
//! | 1 Feedback | 0 to 1000 | 500 |
//! | 2 Mod rate | 0 to 1000 | 400 |
//! | 3 Mod amount | 0 to 1000 | 400 |
//! | 4 Movement | 0 to 1000 | 300 |
//! | 5 Move rate | 0 to 1000 | 300 |
//! | 6 Delay lines, 4 or 8 | 0 to 1 | 0 |
//! | 7 Low pass | 7140 to 22000 | 22000 |
//! | 8 High pass | 120 to 10900 | 120 |
//! | 9 Mix | 0 to 1000 | 500 |
//! | 10 LFO phase difference | 0 to 1000 | 100 |
//!
//! A wheel at `n` is `v = (n - min) / (max - min)` to the engine. Each line
//! has its own ranges for the delay, the rate and depth of its LFO and the
//! rate of its movement LFO, and `v` goes along each range. A line reads
//! its delay plus a parabolic sine times delay times depth, by a four-point
//! cubic, and feeds its output back into itself. The movement LFO pans
//! each line between the left input and the right one at equal power. The
//! lines' outputs are summed at -5 dB, or with eight lines at -11 dB, then
//! go through a low-pass and a high-pass biquad with a Q of 0.5 and come
//! out under the input by an equal-power mix.
//!
//! A value told to the engine lands at the next sample. A value told again
//! before that sample glides there from the first one instead, 90% of the
//! way in 50 ms. So a state loaded into a new plugin glides from the
//! defaults, as in FL.

use crate::{Effect, StateError, knob_value, state_version};

/// The plugin's event 201 name.
pub const NAME: &str = "Hyper Chorus";

/// How long the state is, and the newest version FL reads.
const LEN: usize = 52;
const VERSION: i32 = 2;

/// The range of each parameter's wheel.
const RANGES: [(i32, i32); 11] = [
    (0, 1000),
    (0, 1000),
    (0, 1000),
    (0, 1000),
    (0, 1000),
    (0, 1000),
    (0, 1),
    (7140, 22_000),
    (120, 10_900),
    (0, 1000),
    (0, 1000),
];

/// The engine's own defaults as the shell's constructor rounds them onto
/// the wheels: 0.4, 0.5, 0.4, 0.4, 0.3, 0.3, 0, 1, 0, 0.5 and 0.1.
const DEFAULTS: [i32; 11] = [400, 500, 400, 400, 300, 300, 0, 22_000, 120, 500, 100];

/// The visualiser's gain slider, which the state keeps and the sound does
/// not hear.
const GAIN: (i32, i32) = (-120, 120);

/// Each line's ranges, from the engine's table at 0x18002ba10: the delay
/// in ms, the LFO's rate in Hz, the depth before it is squared, and the
/// movement LFO's rate in Hz, each as low and high. Lines 4 to 7 play only
/// with eight lines on.
const LINES: [[f32; 8]; 8] = [
    [2.0, 6.0, 1.5, 2.47, 20.0, 80.0, 0.33, 3.56],
    [3.5, 8.25, 1.57, 3.41, 20.0, 80.0, 0.29, 2.66],
    [6.5, 10.0, 0.38, 3.64, 20.0, 80.0, 0.26, 3.78],
    [5.0, 12.8, 1.29, 3.68, 20.0, 80.0, 0.14, 2.98],
    [7.1, 16.6, 3.41, 5.03, 20.0, 80.0, 0.44, 5.06],
    [9.1, 15.1, 2.47, 5.37, 20.0, 80.0, 0.66, 8.08],
    [8.3, 16.8, 3.01, 6.02, 20.0, 80.0, 0.93, 6.02],
    [9.6, 17.7, 3.3, 6.5, 20.0, 80.0, 0.41, 10.22],
];

/// How long each line's ring is: the write place is a u16.
const RING: usize = 1 << 16;

/// The pan gains' scale, near the square root of 2.
const PAN: f32 = f32::from_bits(0x3fb5_0bf3);
/// What the summed lines are taken at: -5 dB with four lines, -11 dB with
/// eight.
const FOUR: f32 = f32::from_bits(0x3f0f_f594);
const EIGHT: f32 = f32::from_bits(0x3e8f_f594);
/// The low-pass is `714 * 2^(LOW v)` Hz and the high-pass `12 * 2^(HIGH v)`.
const LOW: f32 = f32::from_bits(0x409e_4102);
const HIGH: f32 = f32::from_bits(0x40d0_2a30);

/// A gliding value nearer its target than this is put there.
const NEAR: f32 = 1e-4;

/// The MSVC runtime's `rand()`.
fn rand(seed: &mut u32) -> u32 {
    *seed = seed.wrapping_mul(214_013).wrapping_add(2_531_011);
    (*seed >> 16) & 0x7fff
}

/// Four numbers from 0 to 1 from `rand()`, as the engine starts its LFOs.
fn phases(seed: &mut u32) -> [f32; 4] {
    [0; 4].map(|_| ((f64::from(rand(seed)) * (1.0 / 32_767.0)) as f32).clamp(0.0, 1.0))
}

/// What is left of `x` over its whole part, as the engine takes it.
fn frac(x: f32) -> f32 {
    x - (x as i32) as f32
}

/// The LFO shape at `x`, twice the phase: a parabola up over the first
/// half of the cycle and one down over the second, from -0.25 to 0.25.
fn shape(x: f32) -> f32 {
    let low = x.min(1.0);
    let high = x.max(1.0) - 1.0;
    (low - low * low) - (high - high * high)
}

/// Four delay lines a side and their LFOs: the engine's `FUN_180002920`.
#[derive(Debug, Clone, PartialEq)]
struct Quad {
    /// Which row of [`LINES`] the first line is.
    row: usize,
    rate: f32,
    /// The last output of each line, left then right.
    out: [f32; 8],
    /// In samples.
    delay: [f32; 4],
    feedback: [f32; 4],
    /// The depth, as a part of the delay.
    depth: [f32; 4],
    /// Where each LFO is, in cycles, and what a sample adds.
    phase: [f32; 4],
    step: [f32; 4],
    /// How far ahead the right side's LFO is, in cycles.
    ahead: [f32; 4],
    /// The movement LFO: where it is, what a sample adds, how far it pans.
    turn: [f32; 4],
    turn_step: [f32; 4],
    movement: [f32; 4],
    /// Eight rings: the left side's four, then the right side's.
    rings: Vec<f32>,
    write: u16,
}

impl Quad {
    fn new(row: usize, seed: &mut u32) -> Self {
        Quad {
            row,
            rate: 44_100.0,
            out: [0.0; 8],
            delay: [0.0; 4],
            feedback: [0.0; 4],
            depth: [0.0; 4],
            phase: phases(seed),
            step: [0.0; 4],
            ahead: [0.0; 4],
            turn: phases(seed),
            turn_step: [0.0; 4],
            movement: [0.0; 4],
            rings: vec![0.0; 8 * RING],
            write: 0,
        }
    }

    /// Take parameter `param` at `v`: `FUN_1800024e0`.
    fn set(&mut self, param: usize, v: f32) {
        let rate = self.rate;
        let along = |line: usize, at: usize| {
            let range = &LINES[self.row + line];
            (range[at + 1] - range[at]) * v + range[at]
        };
        for line in 0..4 {
            match param {
                0 => self.delay[line] = along(line, 0) * 0.001 * rate,
                1 => self.feedback[line] = v * 0.7,
                2 => self.step[line] = along(line, 2) / rate,
                3 => {
                    let depth = along(line, 4);
                    self.depth[line] = depth * depth * 5e-5;
                }
                4 => self.movement[line] = v * 0.499,
                5 => self.turn_step[line] = along(line, 6) / rate,
                10 => self.ahead[line] = v * v * v * 0.25,
                _ => {}
            }
        }
    }

    /// The ring of line `line` at `at`.
    fn get(&self, line: usize, at: u16) -> f32 {
        self.rings[line * RING + usize::from(at)]
    }

    /// Line `line` read `delay` samples behind the sample before the one
    /// just written, between four samples.
    fn read(&self, line: usize, delay: f32) -> f32 {
        let whole = delay as i32;
        let at = self.write.wrapping_sub(whole as u16);
        let y0 = self.get(line, at);
        let y1 = self.get(line, at.wrapping_sub(1));
        let y2 = self.get(line, at.wrapping_sub(2));
        let y3 = self.get(line, at.wrapping_sub(3));
        let c1 = (y2 - y0) * 0.5;
        let a = (y1 - y2) + c1;
        let c3 = (y3 - y1) * 0.5 + (a + (y1 - y2));
        let t = delay - whole as f32;
        ((t * c3 - (c3 + a)) * t + c1) * t + y1
    }

    /// One sample in, each line's output in [`Quad::out`].
    fn tick(&mut self, left: f32, right: f32) {
        let mut back = [[0.0; 2]; 4];
        let mut gains = [[0.0; 2]; 4];
        for line in 0..4 {
            let phase = self.phase[line];
            let ahead = frac(phase + self.ahead[line]);
            let turn = self.turn[line];
            let pan = shape(turn + turn) * 4.0 * self.movement[line] + 0.5;
            let swing = self.delay[line] * self.depth[line];
            back[line] =
                [phase, ahead].map(|phase| shape(phase + phase) * 4.0 * swing + self.delay[line]);
            gains[line] = [(1.0 - pan).sqrt() * PAN, pan.sqrt() * PAN];
        }
        let write = usize::from(self.write);
        for (side, input) in [left, right].into_iter().enumerate() {
            for (line, gain) in gains.iter().enumerate() {
                let n = side * 4 + line;
                let fed = self.out[n] * self.feedback[line];
                self.rings[n * RING + write] = gain[side] * input + fed;
            }
            for (line, back) in back.iter().enumerate() {
                self.out[side * 4 + line] = self.read(side * 4 + line, back[side]);
            }
        }
        self.write = self.write.wrapping_add(1);
        for line in 0..4 {
            self.phase[line] = frac(self.step[line] + self.phase[line]);
            self.turn[line] = frac(self.turn_step[line] + self.turn[line]);
        }
    }

    /// The engine's reset of the first four lines: the rings and the
    /// outputs emptied, the LFOs where they are.
    fn clear(&mut self) {
        self.out = [0.0; 8];
        self.rings.fill(0.0);
        self.write = 0;
    }
}

/// A biquad's coefficients as the engine keeps them: `b0, b1, b2` and the
/// feedback ones with their sign turned, `-a1, -a2`, over `a0`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Biquad([f32; 5]);

impl Biquad {
    /// `FUN_180003030` for type 1, low-pass, or type 2, high-pass: the
    /// cookbook's with `alpha = sin w`, a Q of 0.5.
    fn new(high: bool, hz: f64, rate: f64) -> Self {
        let w = hz * std::f64::consts::TAU * (1.0 / rate);
        let (sin, cos) = w.sin_cos();
        let norm = 1.0 / (sin + 1.0);
        let (b0, b1) = if high {
            ((cos + 1.0) * 0.5, -(cos + 1.0))
        } else {
            ((1.0 - cos) * 0.5, 1.0 - cos)
        };
        Biquad([
            (norm * b0) as f32,
            (norm * b1) as f32,
            (norm * b0) as f32,
            -((norm * (cos * -2.0)) as f32),
            -((norm * (1.0 - sin)) as f32),
        ])
    }
}

/// One side of a biquad in direct form I: `x1, x2, y1, y2`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Past([f32; 4]);

impl Past {
    /// The next output for `x`. The engine adds `b0 x` and `b1 x1` in one
    /// order for the low-pass and the other for the high-pass.
    fn step(&mut self, q: &Biquad, x: f32, b0_first: bool) -> f32 {
        let [x1, x2, y1, y2] = self.0;
        let [b0, b1, b2, a1, a2] = q.0;
        let head = a1 * y1 + a2 * y2 + b2 * x2;
        let y = if b0_first {
            head + b0 * x + b1 * x1
        } else {
            head + b1 * x1 + b0 * x
        };
        self.0 = [x, x1, y, y1];
        y
    }
}

/// A parameter the engine is gliding: its number, where it is, where it
/// goes.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Glide {
    param: usize,
    now: f32,
    target: f32,
}

/// Hyper Chorus. A new one has FL's defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct HyperChorus {
    params: [i32; 11],
    gain: i32,
    rate: f64,
    /// What a sample of a glide moves of what is left.
    glide: f32,
    /// The engine's values, as it was last told them.
    told: [f32; 11],
    glides: [Option<Glide>; 11],
    /// How many samples the high-pass fades over, and where the fade is.
    fade_len: i32,
    fade_step: f32,
    fade: i32,
    eight: bool,
    low: Biquad,
    high: Biquad,
    lows: [Past; 2],
    highs: [Past; 2],
    dry: f32,
    wet: f32,
    quads: [Quad; 2],
}

impl Default for HyperChorus {
    /// The engine as it comes from its constructor, then told the shell's
    /// defaults.
    fn default() -> Self {
        let mut seed = 1;
        let first = Quad::new(0, &mut seed);
        let second = Quad::new(4, &mut seed);
        let mut plugin = HyperChorus {
            params: DEFAULTS,
            gain: 0,
            rate: 44_100.0,
            glide: 1.0,
            told: [0.4, 0.5, 0.4, 0.4, 0.3, 0.3, 0.0, 1.0, 0.0, 0.5, 0.1],
            glides: [None; 11],
            fade_len: 441,
            fade_step: 1.0 / 441.0,
            fade: 0,
            eight: false,
            low: Biquad::default(),
            high: Biquad::default(),
            lows: [Past::default(); 2],
            highs: [Past::default(); 2],
            dry: 1.0,
            wet: 0.0,
            quads: [first, second],
        };
        plugin.send_all();
        plugin
    }
}

impl HyperChorus {
    /// The shell's ProcessParam: a wheel at `n` told to the engine as 0..1.
    fn send(&mut self, param: usize) {
        let (min, max) = RANGES[param];
        let scale = 1.0 / f64::from(max - min);
        let v = (f64::from(self.params[param] - min) * scale).clamp(0.0, 1.0);
        self.tell(param, v as f32);
    }

    fn send_all(&mut self) {
        for param in 0..RANGES.len() {
            self.send(param);
        }
    }

    /// The engine's set-parameter call: the value kept, and a glide to it
    /// queued for the next sample. A glide already queued for the
    /// parameter keeps where it is and takes the new target.
    fn tell(&mut self, param: usize, v: f32) {
        self.told[param] = v;
        if let Some(glide) = self.glides.iter_mut().flatten().find(|g| g.param == param) {
            glide.target = v;
            return;
        }
        if let Some(free) = self.glides.iter_mut().find(|g| g.is_none()) {
            *free = Some(Glide {
                param,
                now: v,
                target: v,
            });
        }
    }

    /// One sample of every glide, the last queued first: `FUN_180001740`'s
    /// head. A glide that has arrived is put at its target and dropped.
    fn glide(&mut self) {
        let mut at = self.glides.iter().take_while(|g| g.is_some()).count();
        while at > 0 {
            at -= 1;
            let Some(mut g) = self.glides[at] else {
                continue;
            };
            let left = g.target - g.now;
            if left.abs() >= NEAR {
                g.now += left * self.glide;
                self.glides[at] = Some(g);
                self.apply(g.param, g.now);
            } else {
                self.apply(g.param, g.target);
                self.glides.copy_within(at + 1.., at);
                self.glides[RANGES.len() - 1] = None;
            }
        }
    }

    /// Where a value lands in the engine: `FUN_180001ee0`.
    fn apply(&mut self, param: usize, v: f32) {
        match param {
            6 => self.eight = v >= 0.5,
            7 => {
                let hz = 2f32.powf(LOW * v) * 714.0;
                self.low = Biquad::new(false, f64::from(hz), self.rate);
            }
            8 => {
                let hz = 2f32.powf(HIGH * v) * 12.0;
                self.high = Biquad::new(true, f64::from(hz), self.rate);
            }
            9 => {
                let w = if v >= 0.5 {
                    let t = 1.0 - (((v - 0.5) + v) - 0.5);
                    1.0 - t * t * 0.5
                } else {
                    (v + v) * (v + v) * 0.5
                };
                self.wet = w.sqrt();
                self.dry = (1.0 - w).sqrt();
            }
            _ => {
                for quad in &mut self.quads {
                    quad.set(param, v);
                }
            }
        }
    }

    /// The high-pass run, its output faded in by where the fade is:
    /// `FUN_180001d20`.
    fn faded(&mut self, wet: [f32; 2]) -> [f32; 2] {
        let g = self.fade as f32 * self.fade_step;
        let mut out = [0.0; 2];
        for side in 0..2 {
            let y = self.highs[side].step(&self.high, wet[side], false);
            out[side] = wet[side] * (1.0 - g) + y * g;
        }
        out
    }

    /// The engine's per-sample body after the glides.
    fn frame(&mut self, left: f32, right: f32) -> [f32; 2] {
        self.quads[0].tick(left, right);
        let a = &self.quads[0].out;
        let (sum_l, sum_r) = (a[0] + a[1] + a[2] + a[3], a[4] + a[5] + a[6] + a[7]);
        let wet = if self.eight {
            self.quads[1].tick(left, right);
            let b = &self.quads[1].out;
            [
                (sum_l + b[3] + b[2] + b[1] + b[0]) * EIGHT,
                (sum_r + b[7] + b[6] + b[5] + b[4]) * EIGHT,
            ]
        } else {
            [sum_l * FOUR, sum_r * FOUR]
        };
        let low = self.low;
        let wet = [0, 1].map(|side| self.lows[side].step(&low, wet[side], false));
        let wet = if self.told[8] == 0.0 {
            if self.fade > 0 {
                let out = self.faded(wet);
                self.fade -= 1;
                out
            } else {
                wet
            }
        } else if self.fade < self.fade_len {
            let out = self.faded(wet);
            self.fade += 1;
            out
        } else {
            let high = self.high;
            [0, 1].map(|side| self.highs[side].step(&high, wet[side], true))
        };
        [
            left * self.dry + wet[0] * self.wet,
            right * self.dry + wet[1] * self.wet,
        ]
    }
}

impl Effect for HyperChorus {
    /// The lines and the LFOs stay as they are, as in FL. Every parameter
    /// is told to the engine again and glides there from where it was.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let version = state_version(bytes, LEN)?;
        if version > VERSION {
            return Err(StateError::Version { found: version });
        }
        if bytes.len() != LEN {
            return Err(StateError::Length {
                expected: LEN,
                found: bytes.len(),
            });
        }
        let mut words = [0; 12];
        for (word, bytes) in words.iter_mut().zip(bytes[4..].as_chunks::<4>().0) {
            *word = i32::from_le_bytes(*bytes);
        }
        let mut params = [0; 11];
        for (param, value) in words.into_iter().take(11).enumerate() {
            let (min, max) = RANGES[param];
            if value < min || value > max {
                return Err(StateError::Range { param, value });
            }
            params[param] = value;
        }
        let gain = words[11];
        if gain < GAIN.0 || gain > GAIN.1 {
            return Err(StateError::Range {
                param: 11,
                value: gain,
            });
        }
        self.params = params;
        self.gain = gain;
        self.send_all();
        Ok(())
    }

    /// The engine counts the rate as a float. A rate that is not a number
    /// of 1 Hz or more is not taken.
    ///
    /// FL's engine keeps the delays, the LFO rates and the filters of the
    /// rate it was last told its parameters at. This one tells them again.
    fn set_sample_rate(&mut self, hz: f32) {
        if !(hz.is_finite() && hz >= 1.0) {
            return;
        }
        let rate = f64::from(hz);
        self.rate = rate;
        for quad in &mut self.quads {
            quad.rate = rate as f32;
        }
        self.fade_len = ((rate * 0.01) as i32).max(1);
        self.fade_step = 1.0 / self.fade_len as f32;
        self.glide = (1.0 - 0.099_999_999_999_999_98f64.powf(1.0 / (rate * 0.05))) as f32;
        for param in 0..RANGES.len() {
            let queued = self.glides.iter().flatten().any(|g| g.param == param);
            if !queued {
                self.tell(param, self.told[param]);
            }
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            self.glide();
            [*l, *r] = self.frame(*l, *r);
        }
    }

    fn param_count(&self) -> usize {
        RANGES.len()
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |value| *value as f32)
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(range) = RANGES.get(index) {
            self.params[index] = knob_value(value, *range);
            self.send(index);
        }
    }

    /// The engine's reset: the filters and the first four lines emptied
    /// and the high-pass fade back at its start. The other four lines keep
    /// what they hold, as in FL.
    fn reset(&mut self) {
        self.lows = [Past::default(); 2];
        self.highs = [Past::default(); 2];
        self.fade = 0;
        self.quads[0].clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{every_preset, presets, projects, response, state};

    /// A state of version 2 with these parameters and a visual gain of 0.
    fn bytes(params: [i32; 11]) -> Vec<u8> {
        let mut bytes = state(&[VERSION]);
        bytes.extend(state(&params));
        bytes.extend(state(&[0]));
        bytes
    }

    /// A plugin with this state whose glides have arrived. With no rate
    /// told, a glide goes the whole way in one sample.
    fn settled(params: [i32; 11]) -> HyperChorus {
        let mut plugin = HyperChorus::default();
        plugin.set_state(&bytes(params)).unwrap();
        plugin.process(&mut [0.0; 2], &mut [0.0; 2]);
        assert!(plugin.glides.iter().all(Option::is_none));
        plugin
    }

    /// The glide queued for a parameter, if there is one.
    fn gliding(plugin: &HyperChorus, param: usize) -> Option<Glide> {
        plugin
            .glides
            .iter()
            .flatten()
            .find(|g| g.param == param)
            .copied()
    }

    #[test]
    fn the_wheels_reach_the_engine_as_the_doc_has_them() {
        let plugin = settled([500, 1000, 0, 1000, 1000, 1000, 1, 22_000, 120, 500, 1000]);
        let [first, second] = &plugin.quads;
        // Line 0's delay runs 2 to 6 ms, and the middle is 4 ms at 44.1 kHz.
        assert!((first.delay[0] - 176.4).abs() < 1e-3);
        // Line 7's is 9.6 to 17.7 ms.
        assert!((second.delay[3] - 13.65 * 44.1).abs() < 1e-3);
        assert!((first.feedback[2] - 0.7).abs() < 1e-7);
        // The lowest LFO rate of line 2 is 0.38 Hz.
        assert!((first.step[2] * 44_100.0 - 0.38).abs() < 1e-5);
        // The depth at full is 80 squared over 20000.
        assert!((first.depth[1] - 0.32).abs() < 1e-6);
        assert!((first.movement[0] - 0.499).abs() < 1e-7);
        assert!((second.turn_step[3] * 44_100.0 - 10.22).abs() < 1e-4);
        assert_eq!(first.ahead, [0.25; 4]);
        assert!(plugin.eight);
        // 22000 on the low-pass wheel is 22 kHz, and 120 on the high-pass
        // is the bottom, where it is off.
        let hz = 2f32.powf(LOW) * 714.0;
        assert!((hz - 22_000.0).abs() < 0.1);
        assert_eq!(plugin.low, Biquad::new(false, f64::from(hz), 44_100.0));
        assert_eq!(plugin.told[8], 0.0);
        assert_eq!(plugin.dry, 0.5f32.sqrt());
        assert_eq!(plugin.wet, 0.5f32.sqrt());
    }

    #[test]
    fn the_filter_wheels_run_on_a_log_scale() {
        let mut plugin = settled([400, 500, 400, 400, 300, 300, 0, 14_570, 5510, 500, 100]);
        // Half way along each wheel is the middle of each range in octaves.
        let low = (714.0f64 * 22_000.0).sqrt();
        let high = (12.0f64 * 1090.0).sqrt();
        let near = |q: Biquad, high: bool, hz: f64| {
            let want = Biquad::new(high, hz, 44_100.0);
            q.0.iter().zip(want.0).all(|(a, b)| (a - b).abs() < 1e-4)
        };
        assert!(near(plugin.low, false, low));
        assert!(near(plugin.high, true, high));
        plugin.set_param(7, 7140.0);
        plugin.set_param(8, 10_900.0);
        plugin.process(&mut [0.0; 2], &mut [0.0; 2]);
        assert!(near(plugin.low, false, 714.0));
        assert!(near(plugin.high, true, 1090.0));
    }

    #[test]
    fn the_filters_are_the_cookbook_s_with_a_q_of_a_half() {
        for (high, hz) in [
            (false, 1000.0),
            (true, 1000.0),
            (false, 5000.0),
            (true, 50.0),
        ] {
            let q = Biquad::new(high, hz, 48_000.0);
            let [b0, b1, b2, a1, a2] = q.0.map(f64::from);
            let gain = |at: f64| response(&[b0, b1, b2], &[1.0, -a1, -a2], at / 48_000.0);
            let (pass, stop) = if high {
                (24_000.0, 0.0)
            } else {
                (0.0, 24_000.0)
            };
            assert!((gain(pass) - 1.0).abs() < 1e-5, "{hz}");
            assert!(gain(stop) < 1e-5, "{hz}");
            // A Q of 0.5 is 6 dB down at the corner.
            assert!((gain(hz) - 0.5).abs() < 1e-5, "{hz}");
        }
    }

    #[test]
    fn the_lfo_is_a_parabolic_sine() {
        let at = |phase: f32| shape(phase + phase) * 4.0;
        assert_eq!(at(0.0), 0.0);
        assert_eq!(at(0.25), 1.0);
        assert_eq!(at(0.5), 0.0);
        assert_eq!(at(0.75), -1.0);
        assert_eq!(at(0.125), 0.75);
        assert_eq!(at(0.875), -0.75);
    }

    #[test]
    fn a_line_reads_its_delay_a_sample_late_by_a_four_point_cubic() {
        let mut seed = 1;
        let mut quad = Quad::new(0, &mut seed);
        quad.delay = [10.0, 20.0, 30.5, 40.25];
        // Movement 0 pans each line to the middle, where both inputs come
        // in at 1.4144 times the square root of a half.
        let gain = 0.5f32.sqrt() * PAN;
        let mut lines = vec![[0.0; 4]; 50];
        for (n, out) in lines.iter_mut().enumerate() {
            quad.tick(if n == 0 { 1.0 } else { 0.0 }, 0.0);
            out.copy_from_slice(&quad.out[..4]);
        }
        let line = |k: usize| -> Vec<f32> { lines.iter().map(|o| o[k] / gain).collect() };
        let at = |k: usize, n: usize| line(k)[n];
        // Whole delays: the impulse alone, delay + 1 samples on.
        assert!((at(0, 11) - 1.0).abs() < 1e-6);
        assert_eq!(line(0).iter().filter(|v| **v != 0.0).count(), 1);
        assert!((at(1, 21) - 1.0).abs() < 1e-6);
        // Half a sample: Catmull-Rom's weights at 0.5.
        let half = [-0.0625, 0.5625, 0.5625, -0.0625];
        for (n, want) in (30..34).zip(half) {
            assert!((at(2, n) - want).abs() < 1e-6, "{n}");
        }
        let t = 0.25f32;
        let quarter = [
            -0.5 * t * (1.0 - t) * (1.0 - t),
            1.0 - 2.5 * t * t + 1.5 * t * t * t,
            0.5 * t * (1.0 + 4.0 * t - 3.0 * t * t),
            -0.5 * t * t * (1.0 - t),
        ];
        for (n, want) in (40..44).zip(quarter) {
            assert!((at(3, n) - want).abs() < 1e-6, "{n}");
        }
        // The right lines heard nothing.
        assert!(quad.rings[4 * RING..].iter().all(|v| *v == 0.0));
    }

    #[test]
    fn the_feedback_comes_round_once_every_delay_and_two_samples() {
        let mut seed = 1;
        let mut quad = Quad::new(0, &mut seed);
        quad.delay = [10.0; 4];
        quad.feedback = [0.5; 4];
        let gain = 0.5f32.sqrt() * PAN;
        let mut heard = Vec::new();
        for n in 0..40 {
            quad.tick(if n == 0 { 1.0 } else { 0.0 }, 0.0);
            heard.push(quad.out[0] / gain);
        }
        let loud: Vec<(usize, f32)> = heard
            .iter()
            .enumerate()
            .filter(|(_, v)| v.abs() > 1e-9)
            .map(|(n, v)| (n, *v))
            .collect();
        assert_eq!(loud.len(), 3);
        for ((n, v), (want_n, want_v)) in loud.into_iter().zip([(11, 1.0), (23, 0.5), (35, 0.25)]) {
            assert_eq!(n, want_n);
            assert!((v - want_v).abs() < 1e-5, "{n}");
        }
    }

    #[test]
    fn the_movement_pans_each_line_between_the_inputs_at_equal_power() {
        let mut seed = 1;
        let mut quad = Quad::new(0, &mut seed);
        quad.movement = [0.499; 4];
        // A quarter cycle on, the movement is at its top: the line hears
        // the right input almost alone.
        quad.turn = [0.25, 0.75, 0.0, 0.125];
        quad.tick(1.0, 1.0);
        let left = |line: usize| quad.rings[line * RING];
        let right = |line: usize| quad.rings[(4 + line) * RING];
        assert!((left(0) - 0.001f32.sqrt() * PAN).abs() < 1e-5);
        assert!((right(0) - 0.999f32.sqrt() * PAN).abs() < 1e-5);
        assert!((left(1) - 0.999f32.sqrt() * PAN).abs() < 1e-5);
        assert!((left(2) - right(2)).abs() < 1e-7);
        for line in 0..4 {
            let power = left(line) * left(line) + right(line) * right(line);
            assert!((power - PAN * PAN).abs() < 1e-5, "{line}");
        }
    }

    #[test]
    fn the_lfos_start_at_the_msvc_runtime_s_first_random_numbers() {
        let plugin = HyperChorus::default();
        let numbers = [
            41, 18_467, 6334, 26_500, 19_169, 15_724, 11_478, 29_358, 26_962, 24_464, 5705, 28_145,
            23_281, 16_827, 9961, 491,
        ];
        let phases = numbers.map(|n| (f64::from(n) / 32_767.0) as f32);
        let [first, second] = &plugin.quads;
        let got = [first.phase, first.turn, second.phase, second.turn].concat();
        assert_eq!(got, phases);
    }

    #[test]
    fn the_mix_is_an_equal_power_crossfade() {
        for (mix, wet) in [
            (0, 0.0f32),
            (250, 0.125),
            (500, 0.5),
            (750, 0.875),
            (1000, 1.0),
        ] {
            let mut params = DEFAULTS;
            params[9] = mix;
            let plugin = settled(params);
            assert!((plugin.wet - wet.sqrt()).abs() < 1e-6, "{mix}");
            assert!((plugin.dry - (1.0 - wet).sqrt()).abs() < 1e-6, "{mix}");
        }
        // Dry alone is the input to the bit.
        let mut params = DEFAULTS;
        params[9] = 0;
        let mut plugin = settled(params);
        let (mut l, mut r) = ([0.3, -0.7, 0.1], [0.2, 0.9, -1.0]);
        plugin.process(&mut l, &mut r);
        assert_eq!((l, r), ([0.3, -0.7, 0.1], [0.2, 0.9, -1.0]));
    }

    #[test]
    fn a_value_told_twice_glides_ninety_percent_of_the_way_in_50_ms() {
        let mut plugin = HyperChorus::default();
        plugin.set_sample_rate(44_100.0);
        plugin.process(&mut [0.0; 9000], &mut [0.0; 9000]);
        assert!(plugin.glides.iter().all(Option::is_none));
        // Told once, a value lands at the next sample.
        plugin.set_param(9, 1000.0);
        assert_eq!(
            gliding(&plugin, 9).map(|g| (g.now, g.target)),
            Some((1.0, 1.0))
        );
        plugin.process(&mut [0.0], &mut [0.0]);
        assert_eq!((plugin.wet, gliding(&plugin, 9)), (1.0, None));
        // Told twice before a block, it glides from the first to the second.
        plugin.set_param(9, 1000.0);
        plugin.set_param(9, 0.0);
        assert_eq!(
            gliding(&plugin, 9).map(|g| (g.now, g.target)),
            Some((1.0, 0.0))
        );
        plugin.process(&mut [0.0; 2205], &mut [0.0; 2205]);
        let now = gliding(&plugin, 9).unwrap().now;
        assert!((now - 0.1).abs() < 1e-3, "{now}");
        plugin.process(&mut [0.0; 8000], &mut [0.0; 8000]);
        assert_eq!(gliding(&plugin, 9), None);
        assert_eq!(plugin.wet, 0.0);
        // A state loaded into a new plugin glides from the defaults.
        let mut fresh = HyperChorus::default();
        fresh.set_sample_rate(44_100.0);
        let mut params = DEFAULTS;
        params[0] = 1000;
        fresh.set_state(&bytes(params)).unwrap();
        assert_eq!(
            gliding(&fresh, 0).map(|g| (g.now, g.target)),
            Some((0.4, 1.0))
        );
    }

    #[test]
    fn the_high_pass_fades_in_over_10_ms_and_out_again() {
        let mut params = DEFAULTS;
        params[8] = 5000;
        let mut plugin = settled(params);
        plugin.set_sample_rate(48_000.0);
        assert_eq!(plugin.fade_len, 480);
        plugin.process(&mut [0.0; 479], &mut [0.0; 479]);
        assert_eq!(plugin.fade, 480);
        plugin.process(&mut [0.0; 100], &mut [0.0; 100]);
        assert_eq!(plugin.fade, 480);
        plugin.set_param(8, 120.0);
        plugin.process(&mut [0.0; 80], &mut [0.0; 80]);
        assert_eq!(plugin.fade, 400);
        // Off, a level that stays comes through the wet side whole.
        plugin.process(&mut [0.0; 400], &mut [0.0; 400]);
        assert_eq!(plugin.fade, 0);
    }

    #[test]
    fn a_new_rate_moves_the_delays_and_not_the_times() {
        let mut plugin = settled(DEFAULTS);
        plugin.set_sample_rate(96_000.0);
        plugin.process(&mut [0.0; 9000], &mut [0.0; 9000]);
        // Line 0's delay at 0.4 is 3.6 ms.
        assert!((plugin.quads[0].delay[0] - 3.6 * 96.0).abs() < 1e-3);
        assert!((plugin.quads[0].step[0] * 96_000.0 - (0.97 * 0.4 + 1.5)).abs() < 1e-4);
    }

    #[test]
    fn a_reset_empties_the_first_four_lines_and_leaves_the_others() {
        let mut params = DEFAULTS;
        params[6] = 1;
        let mut plugin = settled(params);
        plugin.process(&mut [0.5; 300], &mut [-0.5; 300]);
        assert!(plugin.quads[0].rings.iter().any(|v| *v != 0.0));
        let phase = plugin.quads[0].phase;
        plugin.reset();
        assert!(plugin.quads[0].rings.iter().all(|v| *v == 0.0));
        assert_eq!(plugin.quads[0].phase, phase);
        assert!(plugin.quads[1].rings.iter().any(|v| *v != 0.0));
        assert_eq!((plugin.lows, plugin.fade), ([Past::default(); 2], 0));
    }

    #[test]
    fn a_moved_wheel_asks_for_no_memory_and_is_the_same_plugin_as_its_state() {
        let mut plugin = HyperChorus::default();
        let memory = |p: &HyperChorus| [0, 1].map(|n| p.quads[n].rings.as_ptr());
        let before = memory(&plugin);
        let moves = [
            1000.4,
            -3.0,
            2000.0,
            999.6,
            17.0,
            0.0,
            0.6,
            0.0,
            f32::NAN,
            100.0,
            300.0,
        ];
        for (index, value) in moves.into_iter().enumerate() {
            plugin.set_param(index, value);
            plugin.process(&mut [0.25; 200], &mut [0.25; 200]);
        }
        plugin.set_param(11, 1.0);
        assert_eq!((plugin.param(11), plugin.param_count()), (0.0, 11));
        let params: Vec<f32> = (0..11).map(|i| plugin.param(i)).collect();
        let want = [1000, 0, 1000, 1000, 17, 0, 1, 7140, 120, 100, 300];
        assert_eq!(params, want.map(|v| v as f32));
        assert_eq!(memory(&plugin), before);
        let same = settled(want);
        plugin.process(&mut [0.0; 2], &mut [0.0; 2]);
        assert_eq!(plugin.told, same.told);
        assert_eq!(plugin.quads[0].delay, same.quads[0].delay);
    }

    #[test]
    fn a_state_the_plugin_does_not_write_is_turned_down() {
        let mut plugin = settled([10, 20, 30, 40, 50, 60, 1, 8000, 900, 700, 800]);
        let before = plugin.clone();
        for len in [3, 48, 56] {
            let mut wrong = bytes(DEFAULTS);
            wrong.resize(len, 0);
            assert_eq!(
                plugin.set_state(&wrong),
                Err(StateError::Length {
                    expected: 52,
                    found: len
                })
            );
        }
        for (param, value) in [(0, 1001), (6, 2), (7, 7139), (8, 10_901), (10, -1)] {
            let mut params = DEFAULTS;
            params[param] = value;
            assert_eq!(
                plugin.set_state(&bytes(params)),
                Err(StateError::Range { param, value })
            );
        }
        let mut loud = bytes(DEFAULTS);
        loud[48..].copy_from_slice(&121i32.to_le_bytes());
        assert_eq!(
            plugin.set_state(&loud),
            Err(StateError::Range {
                param: 11,
                value: 121
            })
        );
        let mut newer = bytes(DEFAULTS);
        newer[0] = 3;
        assert_eq!(
            plugin.set_state(&newer),
            Err(StateError::Version { found: 3 })
        );
        assert_eq!(plugin, before);
        // FL reads any version under 3.
        let mut older = bytes(DEFAULTS);
        older[0] = 1;
        older[48..].copy_from_slice(&(-120i32).to_le_bytes());
        assert_eq!(plugin.set_state(&older), Ok(()));
        assert_eq!((plugin.params, plugin.gain), (DEFAULTS, -120));
    }

    /// A tone on the left and another on the right, a second of each.
    fn run(plugin: &mut HyperChorus) -> (Vec<f32>, Vec<f32>) {
        let tone = |turns: f64| -> Vec<f32> { crate::testing::sine(turns, 48_000) };
        let (mut left, mut right) = (tone(0.01), tone(0.0037));
        for (l, r) in left.chunks_mut(512).zip(right.chunks_mut(512)) {
            plugin.process(l, r);
        }
        (left, right)
    }

    #[test]
    fn fl_s_presets_load_and_play_at_a_sane_level() {
        let Some(own) = presets(NAME) else { return };
        let all = every_preset(&crate::testing::corpus().unwrap(), NAME);
        assert_eq!(own.len(), 12);
        assert!(all.len() >= own.len());
        for preset in &all {
            let mut plugin = HyperChorus::default();
            plugin.set_sample_rate(48_000.0);
            plugin.set_state(&preset.state).unwrap();
            let (left, right) = run(&mut plugin);
            let peak = left.iter().chain(&right).fold(0f32, |m, v| m.max(v.abs()));
            assert!(
                peak.is_finite() && peak > 0.1 && peak < 4.0,
                "{} {peak}",
                preset.file
            );
        }
        // Default.fst is the middle of every wheel but the filters.
        let default = own.iter().find(|p| p.file == "Default").unwrap();
        let mut plugin = HyperChorus::default();
        plugin.set_state(&default.state).unwrap();
        assert_eq!(
            plugin.params,
            [500, 500, 500, 500, 500, 500, 0, 22_000, 120, 500, 500]
        );
    }

    #[test]
    fn every_hyper_chorus_in_fl_s_projects_plays_at_a_sane_level() {
        let Some(root) = crate::testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, the states in FL's projects skipped");
            return;
        };
        let mut played = 0;
        for saved in projects(&root) {
            if !saved.plugin.eq_ignore_ascii_case(NAME) {
                continue;
            }
            let mut plugin = HyperChorus::default();
            plugin.set_sample_rate(48_000.0);
            plugin.set_state(&saved.state).unwrap();
            let (left, right) = run(&mut plugin);
            let peak = left.iter().chain(&right).fold(0f32, |m, v| m.max(v.abs()));
            assert!(
                peak.is_finite() && peak > 0.1 && peak < 4.0,
                "{} {peak}",
                saved.file
            );
            played += 1;
        }
        assert_eq!(played, 20);
    }
}
