//! Vintage Chorus: a bucket-brigade chorus after the Juno's, as
//! `docs/plugins/vintage-chorus.md` has it. FL's plugin is a Delphi shell
//! around a VST engine (`engine_x64.dll`, uniqueID `JUNO`); this is the
//! engine, driven the way the shell drives it.
//!
//! The state is 72 bytes: a little-endian i32 version, 1, and seventeen
//! parameters as i32. FL leaves a state alone whose version is 2 or more,
//! and so does this. The shell hands the engine each parameter as
//! `(value - min) / (max - min)`.
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Mode (I, II, edit) | 0 to 2 | 0 |
//! | 1 Link I + II | 0 to 1 | 0 |
//! | 2 Speed | 1 to 245760 | 49152 |
//! | 3 LFO type (triangle, sine) | 0 to 1 | 0 |
//! | 4 LR phase | 0 to 6000 | 3000 |
//! | 5, 6 Time 1, Time 2 | 0 to 6000 | 60, 3000 |
//! | 7 Mix | 0 to 6000 | 3000 |
//! | 8 Mono input | 0 to 1 | 0 |
//! | 9 Invert wet | 0 to 1 | 0 |
//! | 10 Feedback | 0 to 6000 | 0 |
//! | 11 High pass | 0 to 6000 | 3000 |
//! | 12 Gain | 0 to 6000 | 0 |
//! | 13 Noise | 0 to 6000 | 3000 |
//! | 14 Noise gate | 0 to 1 | 0 |
//! | 15 Tempo sync | 0 to 1 | 0 |
//! | 16 Start phase | 0 to 6000 | 0 |
//!
//! Modes I and II are fixed settings, a triangle at 0.4 or 0.6 Hz sweeping
//! the delay between 1.08 and 5.23 ms, and I + II a sine at 8 Hz between
//! 2.85 and 3.37 ms. The edit mode takes the wheels. Each side runs: the
//! input with feedback, a high-pass, two second-order low-passes at 7 kHz,
//! a delay line read by a four-point Hermite at a delay that glides to
//! where the LFO puts it, hiss through a peaking filter the LFO moves, a
//! one-pole low-pass at 7 kHz, and an equal-power mix with the input. Then
//! a noise gate and the gain, which glides like the mix.

use crate::{Effect, Fidelity, StateError, knob_value, state_version};

/// The plugin's event 201 name.
pub const NAME: &str = "Vintage Chorus";

const COUNT: usize = 17;

/// The range of each parameter's control.
const RANGES: [(i32, i32); COUNT] = [
    (0, 2),
    (0, 1),
    (1, 245_760),
    (0, 1),
    (0, 6000),
    (0, 6000),
    (0, 6000),
    (0, 6000),
    (0, 1),
    (0, 1),
    (0, 6000),
    (0, 6000),
    (0, 6000),
    (0, 6000),
    (0, 1),
    (0, 1),
    (0, 6000),
];
const DEFAULTS: [i32; COUNT] = [
    0, 0, 49_152, 0, 3000, 60, 3000, 3000, 0, 0, 0, 3000, 0, 3000, 0, 0, 0,
];

/// How long the state is, and the newest version FL reads.
const LEN: usize = 72;
const VERSION: i32 = 1;

/// The delay lines: 32768 samples whatever the rate.
const RING: usize = 0x8000;

/// The first noise seed of the first engine FL loads, and what each
/// engine after it adds to it.
const FIRST_SEED: i32 = 0x517d8;
const SEED_STEP: i32 = 0xb66ad;

/// The position the shell gives when the song is not playing.
const STOPPED: f64 = -1.0;

/// A first-order section: `y = a1 x1 + a0 x + b1 y1`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct OnePole {
    a0: f32,
    a1: f32,
    b1: f32,
}

impl OnePole {
    /// FL's high-pass at `tan(w / 2) = t`.
    fn high(t: f32) -> Self {
        let a0 = 1.0 / (t + 1.0);
        OnePole {
            a0,
            a1: -a0,
            b1: -(t - 1.0) / (t + 1.0),
        }
    }

    /// FL's low-pass at `tan(w / 2) = t`.
    fn low(t: f32) -> Self {
        let a = t / (t + 1.0);
        OnePole {
            a0: a,
            a1: a,
            b1: -(t - 1.0) / (t + 1.0),
        }
    }

    fn run(&self, x: f32, state: &mut [f32; 2]) -> f32 {
        let y = self.a1 * state[0] + self.a0 * x + self.b1 * state[1];
        *state = [x, y];
        y
    }
}

/// A second-order section: `y = b0 x + b1 x1 + b2 x2 + a1 y1 + a2 y2`,
/// with the feedback signs folded in.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Biquad {
    c: [f32; 5],
}

impl Biquad {
    /// `state` is `[x1, x2, y1, y2]`.
    fn run(&self, x: f32, state: &mut [f32; 4]) -> f32 {
        let [x1, x2, y1, y2] = *state;
        let c = &self.c;
        let y = c[1] * x1 + c[0] * x + c[2] * x2 + c[3] * y1 + c[4] * y2;
        *state = [x, x1, y, y1];
        y
    }

    /// The anti-alias low-pass of the delay line, at 7 kHz with Q 0.707.
    fn bucket(rate: f32) -> Self {
        let w = f64::from(43_982.297_f32 / rate);
        let (s, c) = (w.sin() as f32, w.cos() as f32);
        let alpha = s / 1.414;
        let n = 1.0 / (alpha + 1.0);
        let b0 = (1.0 - c) * 0.5 * n;
        Biquad {
            c: [b0, (1.0 - c) * n, b0, -(c * -2.0) * n, -(1.0 - alpha) * n],
        }
    }

    /// The hiss filter: a peak at 6.5 kHz of `a` from 1 to 3, with `s2`
    /// twice its sine and `c2` its cosine times -2.
    fn hiss(s2: f32, c2: f32, a: f32) -> Self {
        let n = 1.0 / (s2 / a + 1.0);
        Biquad {
            c: [
                (s2 * a + 1.0) * n,
                c2 * n,
                (1.0 - s2 * a) * n,
                -c2 * n,
                -(1.0 - s2 / a) * n,
            ],
        }
    }
}

/// The engine's own random numbers: a 32-bit LCG, over 2^31.
fn next(seed: &mut i32) -> f32 {
    *seed = seed.wrapping_mul(0x0bb3_8435).wrapping_add(0x3619_636b);
    *seed as f32 * f32::from_bits(0x3000_0000)
}

/// What is left of `x` over its whole part, as the engine takes it.
fn frac(x: f32) -> f32 {
    x - (x as i32) as f32
}

/// The engine's triangle, from 0 to 1, at 0.5 at the start of the cycle.
fn triangle(p: f32) -> f32 {
    if p < 0.25 {
        p + p + 0.5
    } else if p < 0.75 {
        1.5 - p - p
    } else {
        (p - 0.75) + (p - 0.75)
    }
}

/// One side's delay line, read by a four-point Hermite.
#[derive(Debug, Clone, PartialEq)]
struct Line {
    ring: Vec<f32>,
    write: u32,
}

impl Line {
    fn new() -> Self {
        Line {
            ring: vec![0.0; RING],
            write: 0,
        }
    }

    /// Put `x` in and read `delay` samples back from it.
    fn run(&mut self, x: f32, delay: f32) -> f32 {
        let mask = (RING - 1) as u32;
        self.ring[self.write as usize] = x;
        self.write = (self.write + 1) & mask;
        let whole = delay as i32;
        let f = delay - whole as f32;
        let at = self.write.wrapping_sub(whole as u32);
        let get = |back: u32| self.ring[(at.wrapping_sub(back) & mask) as usize];
        let (xm1, x0, x1, x2) = (get(0), get(1), get(2), get(3));
        let c = (x1 - xm1) * 0.5;
        let v = x0 - x1;
        let w = v + c;
        let a = (x2 - x0) * 0.5 + (w + v);
        let b = a + w;
        ((a * f - b) * f + c) * f + x0
    }
}

/// The engine, the VST's `TestPlug` and the object at its +0x138.
#[derive(Debug, Clone, PartialEq)]
struct Engine {
    /// The parameters as the engine has them, 0 to 1, and the mode.
    values: [f32; COUNT],
    mode: i32,

    mono: bool,
    sync: bool,
    start: f32,
    /// A cycle of the synced LFO, in beats.
    beats: f64,
    sine: bool,
    /// The LFO: what a sample adds to its phase, the phase, and how far on
    /// the right side reads it.
    step: f32,
    phase: f32,
    spread: f32,
    /// The delay's middle and its swing, in seconds, and the second time.
    center: f32,
    time_2: f32,
    depth: f32,
    /// pi over the rate, and the high-pass's corner.
    pi_rate: f32,
    high_hz: f32,
    high: OnePole,
    high_state: [[f32; 2]; 2],
    bucket: Biquad,
    bucket_state: [[[f32; 4]; 2]; 2],
    lines: [Line; 2],
    feedback: f32,
    /// What each line gave last, which is what is fed back.
    last: [f32; 2],
    hiss_level: f32,
    seeds: [i32; 2],
    hiss_count: i32,
    hiss: [Biquad; 2],
    hiss_state: [[f32; 4]; 2],
    low: OnePole,
    low_state: [[f32; 2]; 2],
    gate: bool,
    gate_threshold: f32,
    gate_hold: i32,
    gate_count: i32,
    gate_gain: f32,
    gate_fall: f32,
    gain_to: f32,
    polarity: f32,
    mix_to: f32,
    mix: f32,
    gain: f32,
    gain_spare: f32,
    /// How far a glide goes in a sample.
    glide: f32,
    /// The delays, in seconds, as they glide.
    delays: [f32; 2],
    resync: bool,
    last_position: f64,
    rate: f32,
    over_rate: f32,
    hiss_rate: f32,
}

impl Engine {
    /// A new engine, as `FUN_180019770` makes it. `n` is how many FL
    /// loaded before it, for the noise seeds.
    fn new(n: i32) -> Self {
        let seed = FIRST_SEED.wrapping_add(SEED_STEP.wrapping_mul(n));
        Engine {
            values: [0.0; COUNT],
            mode: 0,
            mono: true,
            sync: false,
            start: 0.0,
            beats: 1.0,
            sine: false,
            step: f32::from_bits(0x3718_2c9f),
            phase: 0.0,
            spread: 0.5,
            center: f32::from_bits(0x3a8d_8ec9),
            time_2: f32::from_bits(0x3bab_606b),
            depth: 0.0,
            pi_rate: f32::from_bits(0x3895_6592),
            high_hz: 100.0,
            high: OnePole::default(),
            high_state: [[0.0; 2]; 2],
            bucket: Biquad::default(),
            bucket_state: [[[0.0; 4]; 2]; 2],
            lines: [Line::new(), Line::new()],
            feedback: 0.0,
            last: [0.0; 2],
            hiss_level: 0.01,
            seeds: [seed, seed.wrapping_add(0x1099e)],
            hiss_count: 16,
            hiss: [Biquad::default(); 2],
            hiss_state: [[0.0; 4]; 2],
            low: OnePole::default(),
            low_state: [[0.0; 2]; 2],
            gate: false,
            gate_threshold: 0.0025,
            gate_hold: 440,
            gate_count: 440,
            gate_gain: 1.0,
            gate_fall: 0.1f64.powf(f64::from_bits(0x3f23_d079_ee39_4d44)) as f32,
            gain_to: 1.0,
            polarity: 1.0,
            mix_to: 0.5,
            mix: 0.0,
            gain: 0.0,
            gain_spare: 0.0,
            glide: 0.0,
            delays: [0.01; 2],
            resync: true,
            last_position: -1.0,
            rate: 44_100.0,
            over_rate: f32::from_bits(0x37be_37c6),
            hiss_rate: 44_100.0,
        }
    }

    /// `FUN_180019680`: the high-pass at `hz`.
    fn set_high(&mut self, hz: f32) {
        self.high_hz = hz;
        self.high = OnePole::high(f64::from(hz * self.pi_rate).tan() as f32);
    }

    /// `FUN_1800192e0`: what follows from the rate.
    fn set_rate(&mut self, hz: f32) {
        let rate = f64::from(hz);
        let fall = f64::from_bits(0x3fb9_9999_9999_9998).powf(1.0 / (rate * 0.05));
        self.hiss_count = 16;
        self.rate = hz;
        self.glide = (1.0 - fall) as f32;
        self.over_rate = 1.0 / hz;
        self.pi_rate = (1.0 / rate) as f32 * std::f32::consts::PI;
        self.set_high(50.0);
        self.low = OnePole::low(f64::from(self.pi_rate * 7000.0).tan() as f32);
        self.bucket = Biquad::bucket(hz);
        self.hiss_rate = hz;
    }

    /// The engine's `setParameter` (`FUN_18001b390`).
    fn set(&mut self, index: usize, v: f32) {
        self.values[index] = v;
        let custom = self.mode == 2;
        match index {
            0 | 1 => {
                self.mode = (self.values[0] * 3.0 * 0.9999) as i32;
                match self.mode {
                    0 | 1 if self.values[1] >= 0.5 => {
                        self.step = 8.0 / self.rate;
                        self.preset();
                        self.spread = f32::from_bits(0x3cea_61da);
                        self.sine = true;
                        self.center = f32::from_bits(0x3b3a_c711);
                        self.time_2 = f32::from_bits(0x3b5c_db38);
                        self.depth = f32::from_bits(0x3a08_509c);
                    }
                    0 | 1 => {
                        let hz = if self.mode == 0 { 0.4 } else { 0.6 };
                        self.step = hz / self.rate;
                        self.preset();
                        self.spread = 0.5;
                        self.sine = false;
                        self.center = f32::from_bits(0x3a8d_8ec9);
                        self.time_2 = f32::from_bits(0x3bab_606b);
                        self.depth = f32::from_bits(0x3b87_fcb9);
                    }
                    2 => {
                        for index in 2..COUNT {
                            self.set(index, self.values[index]);
                        }
                    }
                    _ => {}
                }
            }
            2 | 15 if custom => {
                let speed = self.values[2];
                self.sync = self.values[15] >= 0.5;
                self.resync = false;
                self.step = speed * speed * 10.0 / self.rate;
                let notes = 2f64.powf(f64::from((1.0 - speed) * 7.0));
                let ticks = ((notes - 1.0) * f64::from_bits(0x3ff0_2040_8000_0000)) as f32;
                let ticks = (ticks * 48.0 * 6143.0 / 6144.0 + 1.1) as i32;
                self.beats = f64::from(ticks) / 48.0 * 0.25;
            }
            3 if custom => self.sine = v >= 0.5,
            4 if custom => self.spread = v,
            5 if custom => {
                self.center = (v * 9.8 + 0.2) * 0.001;
                self.depth = self.time_2 - self.center;
            }
            6 if custom => {
                self.time_2 = (v * 9.8 + 0.2) * 0.001;
                self.depth = self.time_2 - self.center;
            }
            7 if custom => self.mix_to = v,
            8 => self.mono = v >= 0.5,
            9 if custom => self.polarity = if v < 0.5 { 1.0 } else { -1.0 },
            10 if custom => self.feedback = v * 0.63,
            11 if custom => self.set_high(v * v * 160.0 + 10.0),
            12 => self.gain_to = 10f64.powf(f64::from(v * 12.0 / 20.0)) as f32,
            13 => {
                (self.hiss_level, self.gate_threshold) = if v == 0.0 {
                    (0.0, 0.0)
                } else {
                    let db = f64::from((v * 50.0 - 90.0) / 20.0);
                    let level = 10f64.powf(db) as f32 * 0.2;
                    (level, level * 8.0)
                };
            }
            14 => self.gate = v >= 0.5,
            16 => self.start = v,
            _ => {}
        }
    }

    /// What modes I and II share.
    fn preset(&mut self) {
        self.mix_to = 0.5;
        self.polarity = 1.0;
        self.feedback = 0.0;
        self.set_high(0.5);
        self.sync = false;
        self.resync = false;
    }

    /// One frame (`FUN_1800186a0`), at `position` beats into the song, or
    /// [`STOPPED`], with the song's tempo.
    fn frame(&mut self, input: [f32; 2], position: f64, tempo: f64) -> [f32; 2] {
        let mut input = input;
        if self.mono {
            let both = (input[0] + input[1]) * 0.5;
            input = [both; 2];
        }
        if self.last_position < 0.0 && position >= 0.0 {
            self.resync = true;
        }
        self.last_position = position;
        if self.sync {
            if self.resync && position >= 0.0 {
                self.phase = frac((position / self.beats) as f32 + self.start);
            }
            self.step = (tempo / 60.0 / self.beats) as f32 * self.over_rate;
        }

        // The LFO, 0 to 1 a side.
        let right = frac(self.phase + self.spread);
        let lfo = if self.sine {
            let tau = std::f32::consts::TAU;
            [self.phase, right].map(|p| (p * tau).sin() * 0.5 + 0.5)
        } else {
            [triangle(self.phase), triangle(right)]
        };
        for (delay, lfo) in self.delays.iter_mut().zip(lfo) {
            *delay += (self.depth * lfo + self.center - *delay) * self.glide;
        }
        self.phase = frac(self.phase + self.step);

        let mut wet = [0.0; 2];
        for side in 0..2 {
            let x = self.feedback * self.last[side] + input[side];
            let x = self.high.run(x, &mut self.high_state[side]);
            let [first, second] = &mut self.bucket_state[side];
            let x = self.bucket.run(x, first);
            let x = self.bucket.run(x, second);
            let out = self.lines[side].run(x, self.delays[side] * self.rate);
            self.last[side] = out;
            wet[side] = out;
        }

        if self.hiss_level > 0.0 {
            self.hiss_count += 1;
            let noise = [next(&mut self.seeds[0]), next(&mut self.seeds[1])];
            if self.hiss_count >= 16 {
                self.hiss_count = 0;
                let w = f64::from(40_840.707_f32 / self.hiss_rate);
                let s2 = w.sin() as f32;
                let s2 = s2 + s2;
                let c2 = w.cos() as f32 * -2.0;
                self.hiss = lfo.map(|lfo| Biquad::hiss(s2, c2, lfo + lfo + 1.0));
            }
            for side in 0..2 {
                let hiss = self.hiss[side].run(noise[side], &mut self.hiss_state[side]);
                wet[side] += self.hiss_level * hiss;
            }
        }

        let wet = [0, 1].map(|side| self.low.run(wet[side], &mut self.low_state[side]));
        self.mix += (self.mix_to - self.mix) * self.glide;
        self.gain += (self.gain_to - self.gain) * self.glide;
        self.gain_spare += (self.gain_to - self.gain_spare) * self.glide;
        let dry = (1.0 - self.mix).sqrt();
        let level = self.mix.sqrt();
        let mut out = [0, 1].map(|side| wet[side] * level * self.polarity + input[side] * dry);
        if self.gate {
            if out[0].abs() >= self.gate_threshold || out[1].abs() >= self.gate_threshold {
                self.gate_count = self.gate_hold;
                self.gate_gain = 1.0;
            }
            if self.gate_count > 0 {
                self.gate_count -= 1;
            } else {
                self.gate_gain *= self.gate_fall;
            }
            out = out.map(|x| self.gate_gain * x);
        }
        out.map(|x| x * self.gain)
    }
}

/// Vintage Chorus. A new one has FL's defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct VintageChorus {
    params: [i32; COUNT],
    hz: f32,
    engine: Engine,
    tempo: f64,
    ppq: u16,
    /// Where the song is, in beats, while it plays.
    position: Option<f64>,
}

impl Default for VintageChorus {
    fn default() -> Self {
        let mut plugin = VintageChorus {
            params: DEFAULTS,
            hz: 44_100.0,
            engine: Engine::new(0),
            tempo: 120.0,
            ppq: 96,
            position: None,
        };
        plugin.engine.set_rate(plugin.hz);
        plugin.apply();
        plugin
    }
}

impl VintageChorus {
    /// What the shell hands the engine for a parameter: its place in its
    /// range, 0 to 1.
    fn value(index: usize, value: i32) -> f32 {
        let (min, max) = RANGES[index];
        let v = f64::from(value - min) * (1.0 / f64::from(max - min));
        v.clamp(0.0, 1.0) as f32
    }

    /// Every parameter to the engine, in order, as the shell does after a
    /// state or a new rate.
    fn apply(&mut self) {
        for (index, value) in self.params.into_iter().enumerate() {
            self.engine.set(index, Self::value(index, value));
        }
    }
}

impl Effect for VintageChorus {
    /// The lines and the LFO stay as they are, as in FL.
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
        let mut params = [0; COUNT];
        for (param, word) in bytes[4..].as_chunks::<4>().0.iter().enumerate() {
            let value = i32::from_le_bytes(*word);
            let (min, max) = RANGES[param];
            if value < min || value > max {
                return Err(StateError::Range { param, value });
            }
            params[param] = value;
        }
        self.params = params;
        self.apply();
        Ok(())
    }

    /// The engine works out its filters again and takes every parameter
    /// again. Its lines stay.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz >= 1.0 {
            self.hz = hz;
            self.engine.set_rate(hz);
            self.apply();
        }
    }

    fn set_tempo(&mut self, bpm: f64) {
        if bpm.is_finite() && bpm > 0.0 {
            self.tempo = bpm;
        }
    }

    fn set_ppq(&mut self, ppq: u16) {
        self.ppq = ppq.max(1);
    }

    fn tick(&mut self, ppq_tick: u64, tempo: f64) {
        self.set_tempo(tempo);
        self.position = Some(ppq_tick as f64 / f64::from(self.ppq));
    }

    /// Only a synced LFO wants the song's place.
    fn follows_song(&self) -> bool {
        self.engine.mode == 2 && self.engine.sync
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let step = self.tempo / 60.0 / f64::from(self.hz);
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let position = self.position.unwrap_or(STOPPED);
            [*l, *r] = self.engine.frame([*l, *r], position, self.tempo);
            if let Some(position) = &mut self.position {
                *position += step;
            }
        }
    }

    fn param_count(&self) -> usize {
        COUNT
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |value| *value as f32)
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(range) = RANGES.get(index) {
            self.params[index] = knob_value(value, *range);
            self.engine
                .set(index, Self::value(index, self.params[index]));
        }
    }

    /// FL's engine keeps its lines, its filters and its LFO through a
    /// flush: only the song stops.
    fn reset(&mut self) {
        self.position = None;
    }

    fn fidelity(&self) -> Fidelity {
        if self.engine.hiss_level > 0.0 {
            Fidelity::Approximate("hiss seeds of the first engine FL loads")
        } else if self.follows_song() {
            Fidelity::Approximate("tempo sync follows the host's ticks")
        } else {
            Fidelity::Exact
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{every_preset, presets, sine, state};

    fn bytes(params: [i32; COUNT]) -> Vec<u8> {
        let mut bytes = state(&[1]);
        bytes.extend(state(&params));
        bytes
    }

    fn with(params: [i32; COUNT]) -> VintageChorus {
        let mut plugin = VintageChorus::default();
        plugin.set_state(&bytes(params)).unwrap();
        plugin
    }

    /// The edit mode, no noise, the given wheels.
    fn custom(edits: &[(usize, i32)]) -> [i32; COUNT] {
        let mut params = DEFAULTS;
        params[0] = 2;
        params[13] = 0;
        for (index, value) in edits {
            params[*index] = *value;
        }
        params
    }

    fn run(plugin: &mut VintageChorus, l: &[f32], r: &[f32], block: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = (l.to_vec(), r.to_vec());
        for (l, r) in l.chunks_mut(block).zip(r.chunks_mut(block)) {
            plugin.process(l, r);
        }
        (l, r)
    }

    #[test]
    fn the_modes_are_the_doc_s_fixed_settings() {
        let one = with(DEFAULTS).engine;
        assert_eq!(one.mode, 0);
        assert!((one.step * 44_100.0 - 0.4).abs() < 1e-6);
        assert!(!one.sine && one.spread == 0.5);
        assert!((one.center - 0.001_08).abs() < 1e-6 && (one.depth - 0.004_15).abs() < 1e-6);
        assert_eq!(
            (one.mix_to, one.polarity, one.feedback, one.high_hz),
            (0.5, 1.0, 0.0, 0.5)
        );
        let mut params = DEFAULTS;
        params[0] = 1;
        let two = with(params).engine;
        assert!((two.step * 44_100.0 - 0.6).abs() < 1e-6);
        params[1] = 1;
        let both = with(params).engine;
        assert!((both.step * 44_100.0 - 8.0).abs() < 1e-5 && both.sine);
        assert!((both.center - 0.002_85).abs() < 1e-6 && (both.depth - 0.000_52).abs() < 1e-6);
        // The edit mode takes the wheels: 0.2 to 10 ms, 0 to 0.63 of
        // feedback, 10 to 170 Hz.
        let edit = with(custom(&[(5, 6000), (6, 0), (10, 3000), (11, 6000), (9, 1)])).engine;
        assert_eq!(edit.mode, 2);
        assert!((edit.center - 0.01).abs() < 1e-7 && (edit.time_2 - 0.0002).abs() < 1e-8);
        assert!((edit.depth + 0.0098).abs() < 1e-7);
        assert!((edit.feedback - 0.315).abs() < 1e-6 && edit.high_hz == 170.0);
        assert_eq!(edit.polarity, -1.0);
        // The speed's square law to 10 Hz.
        let fast = with(custom(&[(2, 245_760)])).engine;
        assert!((fast.step * 44_100.0 - 10.0).abs() < 1e-4);
    }

    #[test]
    fn gain_and_noise_follow_their_decibels_in_every_mode() {
        let e = with([0, 0, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 6000, 6000, 1, 0, 6000]).engine;
        assert!((f64::from(e.gain_to) - 10f64.powf(0.6)).abs() < 1e-5);
        assert!((f64::from(e.hiss_level) - 0.2 * 10f64.powf(-2.0)).abs() < 1e-8);
        assert_eq!(e.gate_threshold, e.hiss_level * 8.0);
        assert!(e.mono && e.gate && e.start == 1.0);
        let quiet = with(custom(&[(13, 0)])).engine;
        assert_eq!((quiet.hiss_level, quiet.gate_threshold), (0.0, 0.0));
    }

    #[test]
    fn the_lfo_shapes_run_from_0_to_1() {
        assert_eq!(triangle(0.0), 0.5);
        assert_eq!(triangle(0.25), 1.0);
        assert_eq!(triangle(0.5), 0.5);
        assert_eq!(triangle(0.75), 0.0);
        assert_eq!(triangle(0.875), 0.25);
        assert_eq!(frac(1.25), 0.25);
    }

    #[test]
    fn the_hermite_is_exact_on_a_cubic() {
        let mut line = Line::new();
        let f = |n: f32| 0.001 * n * n * n - 0.02 * n * n + 0.3 * n;
        for n in 0..40 {
            line.run(f(n as f32), 0.0);
        }
        // The next sample goes in at 40; 7.25 back is 32.75.
        let got = line.run(f(40.0), 7.25);
        assert!((got - f(32.75)).abs() < 1e-4, "{got} {}", f(32.75));
        // No delay is the sample that went in last but one... and with
        // whole delays the samples themselves.
        let got = line.run(f(41.0), 3.0);
        assert!((got - f(38.0)).abs() < 1e-4);
    }

    /// The edit mode's chain for one side in f64, with the delay standing
    /// still at `delay` seconds (glided there from 10 ms).
    fn model(input: &[f32], e: &Engine) -> Vec<f64> {
        let (k, rate) = (f64::from(e.glide), f64::from(e.rate));
        let one = |p: &OnePole, x: f64, s: &mut [f64; 2]| {
            let y = f64::from(p.a1) * s[0] + f64::from(p.a0) * x + f64::from(p.b1) * s[1];
            *s = [x, y];
            y
        };
        let two = |b: &Biquad, x: f64, s: &mut [f64; 4]| {
            let c = b.c.map(f64::from);
            let y = c[0] * x + c[1] * s[0] + c[2] * s[1] + c[3] * s[2] + c[4] * s[3];
            *s = [x, s[0], y, s[2]];
            y
        };
        let (mut hs, mut ls) = ([0.0; 2], [0.0; 2]);
        let (mut b1, mut b2) = ([0.0; 4], [0.0; 4]);
        let mut line = vec![0.0f64; 8];
        let (mut delay, mut last) = (0.01f64, 0.0f64);
        // The glides of the mix and the gain are the engine's, in f32: they
        // stop short of where they go, which leaves some of the input in.
        let (mut mix, mut gain) = (0.0f32, 0.0f32);
        let target = f64::from(e.center) + f64::from(e.depth) * 0.5;
        let mut out = Vec::new();
        for x in input {
            let x = f64::from(*x);
            delay += (target - delay) * k;
            let y = one(&e.high, f64::from(e.feedback) * last + x, &mut hs);
            let y = two(&e.bucket, two(&e.bucket, y, &mut b1), &mut b2);
            line.push(y);
            let d = delay * rate;
            let n = line.len() as f64 - 1.0 - d;
            let at = |i: f64| line[i.max(0.0) as usize];
            // Linear is close enough to the Hermite on a slow tone.
            let w = at(n.floor()) + (at(n.floor() + 1.0) - at(n.floor())) * (n - n.floor());
            last = w;
            let w = one(&e.low, w, &mut ls);
            mix += (e.mix_to - mix) * e.glide;
            gain += (e.gain_to - gain) * e.glide;
            let (level, dry) = (f64::from(mix.sqrt()), f64::from((1.0 - mix).sqrt()));
            out.push((w * level * f64::from(e.polarity) + x * dry) * f64::from(gain));
        }
        out
    }

    #[test]
    fn with_no_swing_it_is_the_doc_s_chain_late_by_the_delay() {
        // A triangle LFO at 0.5 from both sides: no swing when the two
        // times are one.
        for edits in [
            vec![(5, 1200), (6, 1200), (7, 6000), (11, 0)],
            vec![
                (5, 3000),
                (6, 3000),
                (7, 2000),
                (10, 4000),
                (9, 1),
                (12, 3000),
            ],
        ] {
            let mut plugin = with(custom(&edits));
            let tone = sine(0.003, 20_000);
            let wanted = model(&tone, &plugin.engine);
            let (l, r) = run(&mut plugin, &tone, &tone, 256);
            let worst = l
                .iter()
                .zip(&wanted)
                .map(|(g, w)| (f64::from(*g) - w).abs());
            let worst = worst.fold(0.0, f64::max);
            assert!(worst < 2e-4, "{edits:?} {worst}");
            assert_eq!(l, r);
        }
    }

    #[test]
    fn with_the_mix_at_0_the_input_comes_out_under_the_gain_s_glide() {
        let mut plugin = with(custom(&[(7, 0)]));
        let tone = sine(0.01, 10_000);
        let (l, _) = run(&mut plugin, &tone, &tone, 100);
        // The gain starts at 0 and glides to 1 a tenth at a time in 50 ms.
        let k = f64::from(plugin.engine.glide);
        let mut gain = 0.0;
        for (n, (got, x)) in l.iter().zip(&tone).enumerate() {
            gain += (1.0 - gain) * k;
            assert!((f64::from(*got) - f64::from(*x) * gain).abs() < 1e-5, "{n}");
        }
    }

    #[test]
    fn the_glides_reach_a_tenth_in_50_ms() {
        let e = VintageChorus::default().engine;
        let left = (1.0 - f64::from(e.glide)).powf(44_100.0 * 0.05);
        assert!((left - 0.1).abs() < 1e-4);
    }

    #[test]
    fn the_noise_is_the_engine_s_lcg_and_moves_only_when_heard() {
        let mut plugin = with(custom(&[(13, 0)]));
        let seeds = plugin.engine.seeds;
        assert_eq!(seeds, [0x517d8, 0x517d8 + 0x1099e]);
        run(&mut plugin, &[0.1; 100], &[0.1; 100], 50);
        assert_eq!(plugin.engine.seeds, seeds);
        let mut plugin = with(custom(&[(13, 6000), (7, 6000)]));
        let (l, r) = run(&mut plugin, &[0.0; 4000], &[0.0; 4000], 64);
        assert_ne!(plugin.engine.seeds, seeds);
        // Hiss at -40 dB times 0.2 under the filters: quiet, not silent,
        // and different on the two sides.
        let peak = l.iter().map(|v| v.abs()).fold(0.0, f32::max);
        assert!(peak > 1e-4 && peak < 0.05, "{peak}");
        assert_ne!(l, r);
        assert!(matches!(plugin.fidelity(), Fidelity::Approximate(_)));
    }

    #[test]
    fn the_gate_closes_after_its_hold_and_falls_20_db_in_150_ms() {
        let mut plugin = with(custom(&[(13, 3000), (14, 1), (7, 0)]));
        let e = &plugin.engine;
        let threshold = e.gate_threshold;
        assert!((f64::from(e.gate_fall).powf(0.15 * 44_100.0) - 0.1).abs() < 1e-3);
        // Loud for a while, then under the threshold.
        let mut input = vec![0.5f32; 20_000];
        input.extend(vec![threshold * 0.5; 10_000]);
        let (l, _) = run(&mut plugin, &input, &input, 128);
        let late = 20_000 + 440 + 6615;
        let ratio = l[late] / (threshold * 0.5);
        assert!((ratio - 0.1).abs() < 0.01, "{ratio}");
        assert!((l[20_000 + 400] / (threshold * 0.5) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn mono_input_takes_the_middle() {
        let mut plugin = with(custom(&[(8, 1), (7, 0)]));
        let (l, r) = run(&mut plugin, &[1.0; 40_000], &[0.0; 40_000], 500);
        assert_eq!(l, r);
        assert!((l[39_999] - 0.5).abs() < 1e-4);
    }

    #[test]
    fn sync_counts_a_cycle_in_beats_from_the_song_s_place() {
        let plugin = with(custom(&[(15, 1), (2, 245_760)]));
        assert!(plugin.follows_song());
        // The fastest is one tick of 48 to a quarter beat.
        assert_eq!(plugin.engine.beats, 0.25 / 48.0);
        let mut plugin = with(custom(&[(15, 1), (2, 1), (16, 1500)]));
        let beats = plugin.engine.beats;
        assert!(beats > 30.0, "{beats}");
        plugin.set_tempo(120.0);
        plugin.tick(96 * 4, 120.0);
        plugin.process(&mut [0.0; 10], &mut [0.0; 10]);
        let wanted = frac(((4.0 + 9.0 * 2.0 / 44_100.0) / beats) as f32 + 0.25);
        let phase = plugin.engine.phase;
        assert!(
            (phase - frac(wanted + plugin.engine.step)).abs() < 1e-5,
            "{phase}"
        );
        plugin.reset();
        assert!(plugin.position.is_none());
    }

    #[test]
    fn a_moved_wheel_is_the_same_plugin_as_its_state_and_asks_for_no_memory() {
        let mut plugin = VintageChorus::default();
        let memory = plugin.engine.lines[0].ring.as_ptr();
        let wanted = [
            2, 1, 1000, 1, 100, 6000, 0, 5000, 1, 1, 6000, 0, 6000, 0, 1, 0, 6000,
        ];
        for (index, value) in wanted.iter().enumerate() {
            plugin.set_param(index, *value as f32);
        }
        plugin.set_param(2, -5.0);
        plugin.set_param(2, 1000.4);
        plugin.set_param(17, 3.0);
        assert_eq!(plugin.param(17), 0.0);
        let mut same = VintageChorus::default();
        same.set_state(&bytes(wanted)).unwrap();
        assert_eq!(plugin.engine.values, same.engine.values);
        assert_eq!(plugin.engine.center, same.engine.center);
        assert_eq!(plugin.engine.depth, same.engine.depth);
        assert_eq!(plugin.engine.lines[0].ring.as_ptr(), memory);
        assert_eq!(plugin.param_count(), 17);
    }

    #[test]
    fn a_state_the_plugin_does_not_write_is_turned_down() {
        let mut plugin = with(custom(&[]));
        let before = plugin.clone();
        for len in [3, 68, 76] {
            let mut wrong = bytes(DEFAULTS);
            wrong.resize(len, 0);
            assert_eq!(
                plugin.set_state(&wrong),
                Err(StateError::Length {
                    expected: 72,
                    found: len
                })
            );
        }
        for (param, value) in [(0, 3), (2, 0), (7, 6001), (14, 2)] {
            let mut params = DEFAULTS;
            params[param] = value;
            assert_eq!(
                plugin.set_state(&bytes(params)),
                Err(StateError::Range { param, value })
            );
        }
        let mut newer = bytes(DEFAULTS);
        newer[0] = 2;
        assert_eq!(
            plugin.set_state(&newer),
            Err(StateError::Version { found: 2 })
        );
        assert_eq!(plugin, before);
    }

    #[test]
    fn a_new_rate_keeps_the_lines_and_times() {
        let mut plugin = with(custom(&[(5, 3000), (6, 3000)]));
        run(&mut plugin, &[0.3; 1000], &[0.3; 1000], 100);
        let line = plugin.engine.lines[0].clone();
        plugin.set_sample_rate(96_000.0);
        assert_eq!(plugin.engine.lines[0], line);
        assert!((plugin.engine.over_rate - 1.0 / 96_000.0).abs() < 1e-12);
        assert!((plugin.engine.center - (0.5 * 9.8 + 0.2) * 0.001).abs() < 1e-7);
        let before = plugin.clone();
        plugin.set_sample_rate(f32::NAN);
        assert_eq!(plugin, before);
    }

    #[test]
    fn fl_s_presets_load_and_play() {
        let Some(own) = presets(NAME) else { return };
        let all = every_preset(&crate::testing::corpus().unwrap(), NAME);
        assert!(!own.is_empty());
        for preset in &all {
            let mut plugin = VintageChorus::default();
            assert_eq!(plugin.set_state(&preset.state), Ok(()), "{}", preset.file);
            let (l, r) = run(&mut plugin, &sine(0.01, 6000), &sine(0.02, 6000), 512);
            assert!(
                l.iter().chain(&r).all(|v| v.is_finite() && v.abs() < 8.0),
                "{}",
                preset.file
            );
        }
    }
}
