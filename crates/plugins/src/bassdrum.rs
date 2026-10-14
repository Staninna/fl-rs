//! BassDrum: a kick made of two swept oscillators, a click and a layer of
//! sampled noise, as `docs/plugins/bassdrum.md` and the listings of its
//! `engine_x64.dll` have it.
//!
//! # The state
//!
//! 1384 bytes: `SSLF`, the length of what follows as a u32, then the
//! engine's own chunk. That is its magic, 256 bytes of preset name, 1024
//! bytes of the path of the noise sample and the 23 parameters as f32, each
//! from 0 to 1. A chunk too short for the path or for all 23 leaves what it
//! lacks at the defaults, as FL's loader does.
//!
//! | parameter | what the voice gets | default |
//! |---|---|---|
//! | 0 Output level | `10^(2 (v - 0.5))` | 0.5 |
//! | 1 Sampling ratio | `2^(4 (v - 0.5))` | 0.5 |
//! | 2 Duration | `v^3` seconds | 0.5 |
//! | 3 Sub bass | `v^2` | 0 |
//! | 4 Drive | `v` | 0 |
//! | 5 Attack | `v` | 0 |
//! | 6 Base | `2^(4 v + 4)` Hz | 0.1 |
//! | 7 Peak | `100 v^3` times Base | 0.4 |
//! | 8 Slide time | `20 v^3 + 0.001` | 0.4 |
//! | 9, 10, 11 Variation base, peak, slide | `2 v - 1` octaves | 0.5 |
//! | 12 Variation phase | `v` | 0 |
//! | 13 Click cut | `v` | 0.5 |
//! | 14 Click frequency | `2^(14 v)` Hz | 0.5 |
//! | 15 Click amount | `v^5` | 0 |
//! | 16, 17, 18 Noise A, D, S | `v` | 0, 1, 1 |
//! | 19 Noise start | `v` | 0.5 |
//! | 20 Noise pitch | `v` | 0.5 |
//! | 21 Noise mix | `v` | 0 |
//! | 22 Noise filter | `v` | 1 |
//!
//! A parameter here is the float the state holds and not a wheel's whole
//! number: the doc has not read the wheels' ranges.
//!
//! # FL's files
//!
//! Two things the sound is made of are files of FL's and not in this
//! repository. [`Generator::set_resource_dir`] reads both:
//!
//! - [`WAVE`], one turn of the oscillators' wave in 1024 floats. It is near
//!   a sine and is not one. Without it a sine stands in.
//! - The noise sample the state names, by its file name under
//!   [`NOISE_DIR`]: FL looks there too when the path the state holds is of
//!   another machine. Without it the noise layer is silent.
//!
//! [`Generator::fidelity`] says which is lacking.
//!
//! # A note
//!
//! The key picks one of 128 voices and nothing else: every key sounds the
//! same, and a key that is sounding starts over. A note off does nothing,
//! a hit plays itself out. It keeps the knobs it started with, but for the
//! Sampling ratio, which is read as it plays. The note's gain is `v / 127` with
//! `v = int(127 * log21(10 * velocity + 1))`, which is 100 for the 100 of a
//! new note. Its pan is not read.
//!
//! The voice makes its samples at 44.1 kHz, whatever the host's rate is.
//! Every one of them, with `wave(x)` the wave at `x` turns, read between
//! two points:
//!
//! 1. The click is the last quarter of the wave, once, at the Click
//!    frequency and times `25 * amount`. It is added to the phase the two
//!    oscillators are read at.
//! 2. Each oscillator has an `e` that starts at 1 and loses
//!    `2e-4 / slide` of itself a sample. Its phase moves by
//!    `(e^4 * peak + 1) * base / 44100`, no more than half a turn. The
//!    second one has Base, Peak and Slide moved by the three Variation
//!    knobs and starts `phase^2 * 350` samples late.
//! 3. The noise sample is read at `2^(4 (pitch - 0.5))` samples a sample,
//!    through four one-pole lowpasses of `filter^2`, times `2 mix^2` and the
//!    16th power of an envelope that rises to 0.99 and falls to S. Noise
//!    start holds back the oscillators or the noise by up to 4410 samples.
//! 4. The amplitude is a straight fall over the duration through
//!    `7 x / (7 x + 1)`, smoothed by a one-pole of 0.005, times the square
//!    of the attack's rise.
//!
//! The samples are read at `ratio * 44100 / rate` a frame between two of
//! them. Then every frame: a one-pole lowpass of 0.7 splits it, four
//! dividers driven by `2560 * drive^4` saturate what is above, a resonant
//! highpass at 42 Hz with `q = 6 sub^2 + 0.5` follows, and the result is
//! times the gain and 0.4. A voice stops once more than `0.0232 * rate`
//! frames in a row are under 0.00001.
//!
//! What is not FL's:
//!
//! - FL stops a voice at the end of the block it went silent in, and here
//!   it stops on the frame. What is lost is under 0.00001.
//! - A 16-bit noise sample is divided by 32767, as the loader's listing
//!   has it. Other formats than 16-bit PCM and 32-bit float are not read.
//!
//! Where this differs from the doc, the listings have it this way: the
//! chunk's magic is `2kiK` or `1kiK` on disk, the oscillators' table is
//! the file `sounddata` and no sine, the saturator is scaled by what it
//! makes of 0.5 and not of 1, and the velocity is the `log21` above.

use crate::{Fidelity, Generator, NoteOn, StateError};
use std::f64::consts::TAU;
use std::path::{Path, PathBuf};

/// The plugin's event 201 name.
pub const NAME: &str = "BassDrum";

/// Where one turn of the oscillators' wave is, from the directory
/// [`Generator::set_resource_dir`] is handed.
pub const WAVE: &str = "BassDrum/sounddata";

/// Where the noise samples are, from the same directory.
pub const NOISE_DIR: &str = "BassDrum/Noise Samples";

/// How long FL writes the state.
pub const STATE_LEN: usize = 1384;

/// How many parameters there are, and how many keys.
const PARAMS: usize = 23;
const KEYS: usize = 128;

const DEFAULTS: [f32; PARAMS] = [
    0.5, 0.5, 0.5, 0.0, 0.0, 0.0, 0.1, 0.4, 0.4, 0.5, 0.5, 0.5, 0.0, 0.5, 0.5, 0.0, 0.0, 1.0, 1.0,
    0.5, 0.5, 0.0, 1.0,
];

/// Where in the engine's chunk the name ends and the path starts, and
/// where the path ends and the parameters start.
const PATH_AT: usize = 0x104;
const PARAMS_AT: usize = 0x504;

/// The rate the voices make their samples at, and one sample of it.
const INNER_RATE: f32 = 44_100.0;
const INNER_STEP: f32 = 1.0 / INNER_RATE;

/// The level the noise envelope turns round at.
const NOISE_TOP: f32 = 0.99;

/// A frame under this is silent.
const SILENT: f32 = 1e-5;

/// FL takes a number nearer to 0 than this for 0 in its highpass.
const TINY: f32 = 1e-9;

/// The inner samples the smoothed amplitude takes to be gone, once the
/// fall is over: 0.995 a sample down to a billionth.
const SMOOTH_TAIL: f64 = 4200.0;

/// What the knobs come to, as FL keeps them for its voices.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stored {
    /// The gain of the Output level knob.
    pub output: f32,
    /// How many inner samples a frame at 44.1 kHz moves by.
    pub ratio: f32,
    /// Seconds.
    pub duration: f32,
    pub sub: f32,
    pub drive: f32,
    pub attack: f32,
    /// Hz.
    pub base: f32,
    /// How many times Base the sweep starts above Base.
    pub peak: f32,
    pub slide: f32,
    /// Octaves, for the second oscillator.
    pub var_base: f32,
    pub var_peak: f32,
    pub var_slide: f32,
    pub var_phase: f32,
    pub click_cut: f32,
    /// Hz.
    pub click_freq: f32,
    pub click_amount: f32,
    pub noise_attack: f32,
    pub noise_decay: f32,
    pub noise_sustain: f32,
    pub noise_start: f32,
    pub noise_pitch: f32,
    pub noise_mix: f32,
    pub noise_filter: f32,
}

impl Stored {
    /// From the 23 parameters as the state holds them.
    pub fn of(params: &[f32; PARAMS]) -> Self {
        let p = params.map(|value| value.clamp(0.0, 1.0));
        let two_to = |x: f64| x.exp2() as f32;
        Stored {
            output: 10f64.powf(2.0 * (f64::from(p[0]) - 0.5)) as f32,
            ratio: two_to(f64::from((p[1] - 0.5) * 4.0)),
            duration: p[2] * p[2] * p[2],
            sub: p[3] * p[3],
            drive: p[4],
            attack: p[5],
            base: two_to(f64::from(p[6]) * 4.0 + 4.0),
            peak: p[7] * p[7] * p[7] * 100.0,
            slide: p[8] * p[8] * p[8] * 20.0 + 0.001,
            var_base: p[9] * 2.0 - 1.0,
            var_peak: p[10] * 2.0 - 1.0,
            var_slide: p[11] * 2.0 - 1.0,
            var_phase: p[12],
            click_cut: p[13],
            click_freq: two_to(f64::from(p[14]) * 14.0),
            click_amount: p[15].powi(5),
            noise_attack: p[16],
            noise_decay: p[17],
            noise_sustain: p[18],
            noise_start: p[19],
            noise_pitch: p[20],
            noise_mix: p[21],
            noise_filter: p[22],
        }
    }
}

/// The gain FL makes of a note's velocity gain: the velocity as a whole
/// number of 127, by `log21(10 * gain + 1)`, over 127.
pub fn velocity_level(gain: f32) -> f32 {
    let velocity = (gain * 10.0 + 1.0).ln() * 0.328_458_73 * 127.0;
    // A NaN counts as 0.
    (velocity as i32).clamp(0, 127) as f32 * (1.0 / 127.0)
}

/// The wave at `turns`, read between the two points either side. The last
/// point is followed by the first.
fn wave_at(wave: &[f32], turns: f32) -> f32 {
    let mut part = turns - (turns as i32) as f32;
    if part < 0.0 {
        part += 1.0;
    }
    let last = wave.len() - 1;
    let at = wave.len() as f32 * part;
    let point = (at as usize).min(last);
    let along = at - point as f32;
    let next = if point == last { 0 } else { point + 1 };
    along * wave[next] + (1.0 - along) * wave[point]
}

/// The noise sample at a point and `along` the way to the next. Past its
/// end there is nothing.
fn noise_at(sample: &[f32], point: usize, along: f32) -> f32 {
    match sample.get(point) {
        Some(value) => {
            let next = sample.get(point + 1).copied().unwrap_or(0.0);
            (1.0 - along) * value + along * next
        }
        None => 0.0,
    }
}

/// The four dividers the drive saturates with.
fn saturate(value: f32, drive: f32) -> f32 {
    let value = value / (value.abs() * drive + 1.0);
    let value = value / (value * value * drive + 1.0);
    let value = value / ((value * value * value).abs() * drive + 1.0);
    value / (drive * value * value * (value * value) + 1.0)
}

fn flush(value: f32) -> f32 {
    if value < TINY && -TINY < value {
        0.0
    } else {
        value
    }
}

/// The highpass the Sub bass knob makes resonant:
///
/// ```text
/// w = 2 pi hz / rate
/// b = (1 + cos w) / (2 + sin w / q)
/// a1 = -2 q cos w / (q + sin w)
/// a2 = (q - sin w) / (q + sin w)
/// y = (x - 2 x1 + x2) b - a1 y1 - a2 y2
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct SubFilter {
    b: f32,
    a1: f32,
    a2: f32,
    /// The input a frame ago and two frames ago, and the output.
    x: [f32; 2],
    y: [f32; 2],
}

impl SubFilter {
    fn set(&mut self, hz: f32, q: f32, rate: f32) {
        let w = (f64::from(hz) * TAU / f64::from(rate)) as f32;
        let (sin, cos) = w.sin_cos();
        let under = 1.0 / (sin + q);
        self.b = flush((cos + 1.0) / (sin / q + 2.0));
        self.a1 = flush(cos * under * (q * -2.0));
        self.a2 = flush((q - sin) * under);
    }

    fn clear(&mut self) {
        (self.x, self.y) = ([0.0; 2], [0.0; 2]);
    }

    fn process(&mut self, x: f32) -> f32 {
        let x = flush(x);
        let y = (x - (self.x[0] + self.x[0]) + self.x[1]) * self.b
            - self.y[0] * self.a1
            - self.a2 * self.y[1];
        let y = flush(y);
        self.x = [x, self.x[0]];
        self.y = [y, self.y[0]];
        y
    }
}

/// One swept oscillator.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Sweep {
    /// In turns.
    phase: f32,
    /// What is left of the sweep, from 1 down.
    e: f32,
    /// What `e` loses of itself a sample.
    decay: f32,
    /// How many times the base the sweep starts above it.
    peak: f32,
    /// The turns a sample at the base frequency.
    base: f32,
}

impl Sweep {
    /// One sample on, and the wave at the phase with `bend` added.
    fn next(&mut self, wave: &[f32], bend: f32) -> f32 {
        self.e -= self.e * self.decay;
        let e = self.e;
        let step = (e * e * e * e * self.peak + 1.0) * self.base;
        let step = if step <= 0.0 { 0.0 } else { step.min(0.5) };
        self.phase += step;
        if 1.0 < self.phase {
            self.phase -= 1.0;
        }
        wave_at(wave, self.phase + bend)
    }
}

/// The voice of one key.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Voice {
    active: bool,
    sweeps: [Sweep; 2],
    /// The samples the second oscillator still waits.
    second_waits: i32,
    /// Where in the wave the click is, in turns, and what a sample moves
    /// it by. The click is over at 1.
    click_at: f32,
    click_step: f32,
    click_gain: f32,
    /// Where in the noise sample the voice is, and what a sample moves it
    /// by.
    noise_at: f64,
    noise_step: f64,
    noise_mix: f32,
    noise_filter: f32,
    noise_poles: [f32; 4],
    /// Over 0, the samples the oscillators wait for the noise. Under 0,
    /// the samples the noise waits for them.
    start_delay: f32,
    /// The noise envelope, where it is headed and how fast.
    env: f32,
    env_target: f32,
    env_coef: f32,
    /// The same for after the envelope has been at its top.
    decay_target: f32,
    decay_coef: f32,
    /// The fall of the amplitude, from 1 to 0 by `amp_step` a sample, and
    /// the smoothed gain made of it.
    amp: f32,
    amp_step: f32,
    amp_smooth: f32,
    attack: f32,
    attack_coef: f32,
    /// The saturator's drive, and one over what it makes of 0.5.
    drive: f32,
    norm: f32,
    gain: f32,
    /// The lowpass the frame is split by.
    split: f32,
    sub: SubFilter,
    /// The frames in a row that were silent.
    silent: i32,
    /// The two inner samples a frame is read between, whether there are
    /// any yet, and how far from the first the next frame is.
    window: [f32; 2],
    primed: bool,
    at: f64,
}

impl Voice {
    /// Start over with what the knobs come to, for a wave of `points`
    /// points and a host at `rate`. What the filters and the resampler
    /// hold stays, as it does in FL for a key that is sounding.
    fn start(&mut self, s: &Stored, level: f32, points: usize, rate: f32) {
        let two_to = |octaves: f32| 2f64.powf(f64::from(octaves)) as f32;
        let per_second = |seconds: f32| {
            (1.0 / (f64::from(seconds * INNER_RATE) + f64::from(INNER_RATE) * 0.001)) as f32
        };
        // FL's increments are for a table of 1024 points.
        let k = 1024.0 / points as f32;

        self.active = true;
        self.silent = 0;
        self.amp = 1.0;
        self.amp_smooth = 1.0;
        self.amp_step = if 0.0 < s.duration {
            per_second(s.duration)
        } else {
            1.0
        };
        let drive = s.drive * s.drive;
        self.drive = drive * drive * 2560.0;
        self.norm = 1.0 / saturate(0.5, self.drive);
        self.gain = s.output * level;
        let attack = s.attack * s.attack;
        let attack = attack * attack;
        self.attack = 0.0;
        self.attack_coef = 1.0 / (attack * attack * 100_000.0 + 1.0);

        let peak = k * s.peak;
        let first = Sweep {
            phase: 0.25,
            e: 1.0,
            decay: (2e-4 / f64::from(s.slide)) as f32,
            peak,
            base: s.base * k * INNER_STEP,
        };
        let second = Sweep {
            base: two_to(s.var_base) * s.base * k * INNER_STEP,
            peak: two_to(s.var_peak) * peak,
            decay: (2e-4 / f64::from(two_to(s.var_slide) * s.slide)) as f32,
            ..first
        };
        self.sweeps = [first, second];
        self.second_waits = (s.var_phase * s.var_phase * 350.0) as i32;

        self.click_at = f64::from(s.click_cut).powf(0.125) as f32 * 0.25 + 0.75;
        self.click_gain = s.click_amount * 25.0;
        self.click_step = s.click_freq * INNER_STEP;

        self.sub.set(42.0, s.sub * 6.0 + 0.5, rate);
        self.sub.clear();

        self.noise_at = 0.0;
        let start = ((s.noise_start - 0.5) + s.noise_start) - 0.5;
        self.start_delay = start * start * start * 4410.0;
        self.noise_step = ((f64::from(s.noise_pitch) - 0.5) * 4.0).exp2();
        let mix = s.noise_mix * s.noise_mix;
        self.noise_mix = mix + mix;
        let decay = s.noise_decay * s.noise_decay * s.noise_decay;
        self.decay_coef = per_second(decay * decay);
        self.decay_target = s.noise_sustain;
        if 1e-4 <= s.noise_attack {
            let attack = s.noise_attack * s.noise_attack * s.noise_attack;
            self.env = 0.0;
            self.env_target = 1.0;
            self.env_coef = INNER_STEP / (attack * attack);
        } else {
            self.env = NOISE_TOP;
            self.env_target = s.noise_sustain;
            self.env_coef = self.decay_coef;
        }
        self.noise_filter = s.noise_filter * s.noise_filter;
    }

    /// The next sample of the click and the two oscillators.
    fn body(&mut self, wave: &[f32]) -> f32 {
        let mut bend = 0.0;
        if self.click_at < 1.0 {
            bend = wave_at(wave, self.click_at) * self.click_gain;
            self.click_at += self.click_step;
        }
        let mut body = self.sweeps[0].next(wave, bend);
        if self.second_waits < 1 {
            body += self.sweeps[1].next(wave, bend);
        } else {
            self.second_waits -= 1;
        }
        body
    }

    /// The next sample of the noise layer.
    fn noise(&mut self, sample: &[f32]) -> f32 {
        let at = self.noise_at;
        let along = (at - f64::from(at as i32)) as f32;
        let mut value = noise_at(sample, at as usize, along);
        self.noise_at = at + self.noise_step;
        for pole in &mut self.noise_poles {
            value = (value - *pole) * self.noise_filter + *pole;
            *pole = value;
        }
        let mut env = (self.env_target - self.env) * self.env_coef + self.env;
        if NOISE_TOP < env {
            self.env_target = self.decay_target;
            self.env_coef = self.decay_coef;
            env = NOISE_TOP;
        }
        self.env = env;
        let power = env * env * env * env;
        let power = power * power;
        value * self.noise_mix * power * power
    }

    /// The next sample of the sound, before its amplitude.
    fn mixed(&mut self, wave: &[f32], noise: Option<&[f32]>) -> f32 {
        let waiting = self.start_delay;
        let mut value = 0.0;
        let mut noise_due = true;
        if waiting <= 0.0 {
            value = self.body(wave);
            if waiting < 0.0 {
                noise_due = false;
                self.start_delay = (waiting + 1.0).min(0.0);
            }
        }
        if noise_due {
            if let Some(sample) = noise {
                value += self.noise(sample);
            }
            if 0.0 < self.start_delay {
                self.start_delay = (self.start_delay - 1.0).max(0.0);
            }
        }
        value
    }

    /// The next sample at 44.1 kHz.
    fn inner(&mut self, wave: &[f32], noise: Option<&[f32]>) -> f32 {
        let value = self.mixed(wave, noise);
        let mut target = 0.0;
        let mut amp = 0.0;
        if self.amp_step < 1.0 && 0.0 < self.amp {
            target = self.amp * 7.0 / (self.amp * 7.0 + 1.0);
            amp = self.amp - self.amp_step;
        }
        self.amp = amp;
        self.amp_smooth = (target * 0.005 - self.amp_smooth * 0.005) + self.amp_smooth;
        let attack = self.attack;
        self.attack = (1.0 - attack) * self.attack_coef + attack;
        attack * attack * self.amp_smooth * value
    }

    /// What the split, the drive and the highpass make of a frame.
    fn post(&mut self, frame: f32) -> f32 {
        let low = (frame - self.split) * 0.7 + self.split;
        self.split = low;
        let high = saturate(frame - low, self.drive);
        self.sub.process(high * self.norm + low) * self.gain * 0.4
    }

    /// The next frame, where a frame is `step` inner samples. The voice
    /// stops once more than `limit` frames in a row were silent.
    fn frame(&mut self, step: f64, limit: i32, wave: &[f32], noise: Option<&[f32]>) -> f32 {
        if !self.primed {
            self.window = [self.inner(wave, noise), self.inner(wave, noise)];
            self.primed = true;
        }
        while 1.0 <= self.at {
            self.window = [self.window[1], self.inner(wave, noise)];
            self.at -= 1.0;
        }
        let along = self.at as f32;
        let read = (1.0 - along) * self.window[0] + along * self.window[1];
        self.at += step;
        let out = self.post(read);
        if out.abs() < SILENT {
            self.silent += 1;
            if limit < self.silent {
                *self = Voice::default();
            }
        } else {
            self.silent = 0;
        }
        out
    }
}

/// One channel of a WAV file as floats: 16-bit PCM or 32-bit float.
/// `None` for anything else.
fn read_wav(bytes: &[u8]) -> Option<Vec<f32>> {
    let word = |at: usize| Some(u16::from_le_bytes(*bytes.get(at..)?.first_chunk()?));
    let long = |at: usize| Some(u32::from_le_bytes(*bytes.get(at..)?.first_chunk()?) as usize);
    if bytes.get(..4)? != b"RIFF" || bytes.get(8..12)? != b"WAVE" {
        return None;
    }
    let mut at = 12;
    let mut format = None;
    loop {
        let size = long(at + 4)?;
        let body = bytes.get(at + 8..)?;
        let body = &body[..size.min(body.len())];
        match bytes.get(at..at + 4)? {
            b"fmt " => format = Some((word(at + 8)?, word(at + 10)?, word(at + 22)?)),
            b"data" => {
                let (tag, channels, bits) = format?;
                let frame = usize::from(channels) * usize::from(bits / 8);
                let first = |frame: &[u8]| match (tag, bits) {
                    (1, 16) => {
                        Some(f32::from(i16::from_le_bytes(*frame.first_chunk()?)) / 32_767.0)
                    }
                    (3, 32) => Some(f32::from_le_bytes(*frame.first_chunk()?)),
                    _ => None,
                };
                return body.chunks_exact(frame.max(1)).map(first).collect();
            }
            _ => {}
        }
        at = at.checked_add(8 + size + (size & 1))?;
    }
}

/// The file a path names, whichever way its slashes go.
fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// The file of this name in a directory, read. Case does not count, as it
/// does not where FL runs.
fn read_named(dir: &Path, name: &str) -> Option<Vec<u8>> {
    if name.is_empty() {
        return None;
    }
    let entries = std::fs::read_dir(dir).ok()?;
    let found = entries.flatten().find(|entry| {
        entry
            .file_name()
            .to_str()
            .is_some_and(|file| file.eq_ignore_ascii_case(name))
    })?;
    std::fs::read(found.path()).ok()
}

/// BassDrum. A new one is at 44.1 kHz with FL's defaults and none of FL's
/// files.
#[derive(Debug, Clone, PartialEq)]
pub struct BassDrum {
    params: [f32; PARAMS],
    /// The path of the noise sample, as the state has it.
    noise_path: String,
    rate: f32,
    /// FL's `Plugins/Fruity/Generators`, once a host has said where it is.
    dir: Option<PathBuf>,
    /// One turn of a sine, for where FL's wave is lacking.
    sine: Vec<f32>,
    wave: Option<Vec<f32>>,
    noise: Option<Vec<f32>>,
    /// One for every key.
    voices: Vec<Voice>,
}

impl Default for BassDrum {
    fn default() -> Self {
        let sine = |point: usize| (TAU * point as f64 / 1024.0).sin() as f32;
        BassDrum {
            params: DEFAULTS,
            noise_path: String::new(),
            rate: INNER_RATE,
            dir: None,
            sine: (0..1024).map(sine).collect(),
            wave: None,
            noise: None,
            voices: vec![Voice::default(); KEYS],
        }
    }
}

impl BassDrum {
    /// The path of the noise sample the state names. Empty for a new
    /// plugin.
    pub fn noise_path(&self) -> &str {
        &self.noise_path
    }

    /// The noise sample of the state, from FL's directory. None where the
    /// host has named no directory or the file is not in it.
    fn find_noise(&self) -> Option<Vec<f32>> {
        let dir = self.dir.as_ref()?.join(NOISE_DIR);
        read_wav(&read_named(&dir, file_name(&self.noise_path))?)
    }

    /// The inner samples a frame moves by: the Sampling ratio, kept
    /// between a quarter and four, for the host's rate.
    fn step(&self) -> f64 {
        let ratio = f64::from(Stored::of(&self.params).ratio).clamp(0.25, 4.0);
        ratio * (f64::from(INNER_RATE) / f64::from(self.rate))
    }

    /// The most silent frames in a row a voice plays on through.
    fn limit(&self) -> i32 {
        (self.rate * 0.023_219_954) as i32
    }

    fn stop(&mut self) {
        self.voices.fill(Voice::default());
    }
}

impl Generator for BassDrum {
    /// The notes that sound keep the knobs they started with. This reads
    /// the state's noise sample where the host has named FL's directory.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let length = |expected: usize| StateError::Length {
            expected,
            found: bytes.len(),
        };
        let version = |magic: &[u8; 4]| StateError::Version {
            found: i32::from_le_bytes(*magic),
        };
        let (head, rest) = bytes.split_first_chunk::<8>().ok_or(length(STATE_LEN))?;
        let (magic, size) = head.split_at(4);
        if magic != b"SSLF" {
            return Err(version(magic.first_chunk().unwrap_or(&[0; 4])));
        }
        let size = u32::from_le_bytes(*size.first_chunk().unwrap_or(&[0; 4])) as usize;
        if size == 0 || rest.len() != size {
            return Err(length(STATE_LEN));
        }
        // A chunk that went through a VST host has a header of 60 bytes.
        let chunk = match rest.starts_with(b"CcnK") {
            true => rest.get(60..).ok_or(length(STATE_LEN))?,
            false => rest,
        };
        let magic = chunk.first_chunk::<4>().ok_or(length(STATE_LEN))?;
        // "Kik2" and "Kik1", the high byte last. The magics of a demo's
        // save are turned down, as an FL that is paid for does not write
        // them.
        if !matches!(magic, b"2kiK" | b"1kiK") {
            return Err(version(magic));
        }

        let mut params = DEFAULTS;
        let mut path = String::new();
        if let Some(saved) = chunk.get(PATH_AT..PARAMS_AT) {
            let text = saved.split(|byte| *byte == 0).next().unwrap_or(&[]);
            path = text.iter().map(|byte| char::from(*byte)).collect();
            let words = chunk[PARAMS_AT..].as_chunks::<4>().0;
            for (param, word) in words.iter().take(PARAMS).enumerate() {
                let value = f32::from_le_bytes(*word);
                if !value.is_finite() {
                    return Err(StateError::Range {
                        param,
                        value: i32::from_le_bytes(*word),
                    });
                }
                params[param] = value.clamp(0.0, 1.0);
            }
        }
        self.params = params;
        self.noise_path = path;
        self.noise = self.find_noise();
        Ok(())
    }

    /// Cuts every note, as FL's change of rate does.
    fn set_sample_rate(&mut self, hz: f32) {
        self.rate = hz;
        self.stop();
    }

    /// Reads the wave and the state's noise sample out of FL's directory.
    /// A wave that was read stays where the directory has none.
    fn set_resource_dir(&mut self, dir: &Path) {
        self.dir = Some(dir.to_path_buf());
        let wave = std::fs::read(dir.join(WAVE)).ok();
        if let Some(wave) = wave.and_then(|bytes| read_wav(&bytes)) {
            // The read at a point needs the point after it.
            if wave.len() >= 2 {
                self.wave = Some(wave);
                self.stop();
            }
        }
        self.noise = self.find_noise();
    }

    /// The voice of the note's key starts over. A velocity that comes to
    /// 0 of 127 starts nothing.
    fn note_on(&mut self, note: NoteOn) {
        let level = velocity_level(note.velocity);
        if level <= 0.0 {
            return;
        }
        let stored = Stored::of(&self.params);
        let points = self.wave.as_ref().unwrap_or(&self.sine).len();
        let key = note.key.clamp(0, KEYS as i16 - 1) as usize;
        self.voices[key].start(&stored, level, points, self.rate);
    }

    /// A hit plays itself out.
    fn note_off(&mut self, _voice: u32) {}

    fn all_notes_off(&mut self) {}

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let (step, limit) = (self.step(), self.limit());
        let wave = self.wave.as_deref().unwrap_or(&self.sine);
        let noise = self.noise.as_deref();
        for voice in self.voices.iter_mut().filter(|voice| voice.active) {
            for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                let value = voice.frame(step, limit, wave, noise);
                *l += value;
                *r += value;
                if !voice.active {
                    break;
                }
            }
        }
    }

    fn param_count(&self) -> usize {
        PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).copied().unwrap_or(0.0)
    }

    /// From 0 to 1, as the state holds it, and not rounded. A NaN counts
    /// as 0. The next note hears it, and the Sampling ratio is heard at
    /// once.
    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(param) = self.params.get_mut(index) {
            *param = if value.is_nan() {
                0.0
            } else {
                value.clamp(0.0, 1.0)
            };
        }
    }

    fn reset(&mut self) {
        self.stop();
    }

    /// As long as a whole hit of the knobs as they are: the fall of the
    /// amplitude, the ringing of the highpass down to a millionth and the
    /// silent frames before a voice stops.
    fn tail(&self) -> usize {
        let stored = Stored::of(&self.params);
        let fall = f64::from(stored.duration) * f64::from(INNER_RATE) + 44.1 + SMOOTH_TAIL;
        let mut sub = SubFilter::default();
        sub.set(42.0, stored.sub * 6.0 + 0.5, self.rate);
        let radius = f64::from(sub.a2).max(0.0).sqrt();
        let ring = if radius > 0.0 && radius < 1.0 {
            1e-6f64.ln() / radius.ln()
        } else {
            0.0
        };
        (fall / self.step() + ring) as usize + self.limit().max(0) as usize + 2
    }

    fn fidelity(&self) -> Fidelity {
        let noise_heard = self.params[21] > 0.0 && !self.noise_path.is_empty();
        if self.wave.is_none() {
            Fidelity::Approximate("BassDrum wave not available")
        } else if noise_heard && self.noise.is_none() {
            Fidelity::Approximate("BassDrum noise sample not available")
        } else {
            Fidelity::Exact
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, partial};

    const RATE: f64 = 44_100.0;

    fn note(voice: u32, key: i16) -> NoteOn {
        NoteOn {
            voice,
            key,
            velocity: 1.0,
            pan: 0.0,
            fine_pitch: 0.0,
            release: 0.5,
            mod_x: 0.0,
            mod_y: 0.0,
            slide: false,
            porta: false,
        }
    }

    /// The defaults with these knobs moved.
    fn knobs(moved: &[(usize, f32)]) -> [f32; PARAMS] {
        let mut params = DEFAULTS;
        for (index, value) in moved {
            params[*index] = *value;
        }
        params
    }

    /// A state as FL writes it, but for the magic and for how many of the
    /// parameters it has.
    fn state(magic: &[u8; 4], name: &str, path: &str, params: &[f32]) -> Vec<u8> {
        let padded = |text: &str, len: usize| {
            let mut bytes = text.as_bytes().to_vec();
            bytes.resize(len, 0);
            bytes
        };
        let mut chunk = magic.to_vec();
        chunk.extend(padded(name, 256));
        chunk.extend(padded(path, 1024));
        chunk.extend(params.iter().flat_map(|value| value.to_le_bytes()));
        let mut bytes = b"SSLF".to_vec();
        bytes.extend((chunk.len() as u32).to_le_bytes());
        bytes.extend(chunk);
        bytes
    }

    fn with(params: [f32; PARAMS]) -> BassDrum {
        let mut drum = BassDrum::default();
        drum.set_state(&state(b"2kiK", "a kick", "", &params))
            .unwrap();
        drum
    }

    fn play(drum: &mut BassDrum, len: usize) -> Vec<f32> {
        let (mut left, mut right) = (vec![0.0; len], vec![0.0; len]);
        drum.process(&mut left, &mut right);
        assert_eq!(left, right);
        left
    }

    /// A voice that has just started with these knobs, at 44.1 kHz.
    fn voice(params: [f32; PARAMS]) -> Voice {
        let mut voice = Voice::default();
        voice.start(&Stored::of(&params), 1.0, 1024, 44_100.0);
        voice
    }

    fn sine() -> Vec<f32> {
        BassDrum::default().sine
    }

    /// Where the sound goes through 0, in samples, by a straight line
    /// between the two samples either side.
    fn crossings(sound: &[f32]) -> Vec<f64> {
        let mut found = Vec::new();
        for (n, pair) in sound.windows(2).enumerate() {
            let (a, b) = (f64::from(pair[0]), f64::from(pair[1]));
            if a != 0.0 && a.signum() != b.signum() {
                found.push(n as f64 + a / (a - b));
            }
        }
        found
    }

    /// The frequency of a steady tone, in turns a sample, from its first
    /// crossing to its last.
    fn pitch(sound: &[f32]) -> f64 {
        let found = crossings(sound);
        (found.len() - 1) as f64 / 2.0 / (found[found.len() - 1] - found[0])
    }

    /// The doc's sweep at 44.1 kHz: the phase of every sample in turns,
    /// not wrapped. `peak` is how many times `base` the sweep starts above
    /// it.
    fn sweep(base: f64, peak: f64, slide: f64, len: usize) -> Vec<f64> {
        let (mut e, mut phase) = (1.0f64, 0.25);
        let phases = (0..len).map(|_| {
            e -= e * 2e-4 / slide;
            phase += ((e.powi(4) * peak + 1.0) * base / RATE).min(0.5);
            phase
        });
        phases.collect()
    }

    #[test]
    fn the_state_is_a_magic_a_name_a_path_and_23_floats() {
        let path = "%FLStudioFactoryData%/Plugins/Fruity/Generators/BassDrum/Noise \
                    Samples/Hard Clap.wav";
        let params: [f32; PARAMS] = std::array::from_fn(|index| index as f32 / 22.0);
        let bytes = state(b"2kiK", "Bassdrum 01", path, &params);
        assert_eq!(bytes.len(), STATE_LEN);
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 1376);

        let mut drum = BassDrum::default();
        assert_eq!(drum.params, DEFAULTS);
        assert_eq!(drum.noise_path(), "");
        drum.set_state(&bytes).unwrap();
        assert_eq!(drum.params, params);
        assert_eq!(drum.noise_path(), path);
        // The projects have the older magic and the path of the machine
        // they were saved on.
        let old = "C:\\Program Files\\Image-Line\\FL Studio 12\\Noise Samples\\Evolving 2.wav";
        drum.set_state(&state(b"1kiK", "Kick", old, &DEFAULTS))
            .unwrap();
        assert_eq!(drum.params, DEFAULTS);
        assert_eq!(file_name(drum.noise_path()), "Evolving 2.wav");
        assert_eq!(file_name(path), "Hard Clap.wav");
        assert_eq!(file_name("Soft Clap.wav"), "Soft Clap.wav");
        assert_eq!(file_name(""), "");

        // A float outside 0 to 1 is kept inside, as FL's setter does.
        let wide = knobs(&[(0, 1.5), (6, -0.25)]);
        drum.set_state(&state(b"2kiK", "", "", &wide)).unwrap();
        assert_eq!((drum.params[0], drum.params[6]), (1.0, 0.0));
        // A chunk a VST host wrapped has 60 bytes before the magic.
        let plain = state(b"2kiK", "", "", &params);
        let mut wrapped = b"SSLF".to_vec();
        wrapped.extend((1376u32 + 60).to_le_bytes());
        wrapped.extend(b"CcnK");
        wrapped.extend([0; 56]);
        wrapped.extend(&plain[8..]);
        drum.set_state(&wrapped).unwrap();
        assert_eq!(drum.params, params);
    }

    #[test]
    fn a_short_chunk_keeps_the_defaults_it_lacks_and_a_wrong_state_is_turned_down() {
        let moved = knobs(&[(0, 0.25), (6, 0.75), (22, 0.5)]);
        let whole = state(b"2kiK", "name", "noise.wav", &moved);
        // The chunk cut to `len` bytes, with its length put right.
        let cut = |len: usize| {
            let mut bytes = whole[..8 + len].to_vec();
            bytes[4..8].copy_from_slice(&(len as u32).to_le_bytes());
            bytes
        };
        let mut drum = with(knobs(&[(3, 1.0)]));
        // Seven of the 23 floats and half of the eighth.
        drum.set_state(&cut(PARAMS_AT + 30)).unwrap();
        assert_eq!(drum.params, knobs(&[(0, 0.25), (6, 0.75)]));
        assert_eq!(drum.noise_path(), "noise.wav");
        // Not all of the path: the name alone, which is not kept.
        for len in [PARAMS_AT - 1, PATH_AT, 4] {
            drum.set_state(&cut(len)).unwrap();
            assert_eq!((drum.params, drum.noise_path()), (DEFAULTS, ""), "{len}");
        }

        let mut drum = with(moved);
        let before = drum.clone();
        let length = |found| {
            Err(StateError::Length {
                expected: STATE_LEN,
                found,
            })
        };
        assert_eq!(drum.set_state(&[]), length(0));
        assert_eq!(drum.set_state(&whole[..7]), length(7));
        // The length the state says is not the length it has.
        assert_eq!(drum.set_state(&whole[..STATE_LEN - 1]), length(1383));
        let long = [&whole[..], &[0]].concat();
        assert_eq!(drum.set_state(&long), length(1385));
        assert_eq!(drum.set_state(&cut(0)), length(8));
        assert_eq!(drum.set_state(&cut(3)), length(11));
        // Not the wrapper's magic, not the engine's, and a demo's save.
        let version = |magic: &[u8; 4]| {
            Err(StateError::Version {
                found: i32::from_le_bytes(*magic),
            })
        };
        let mut other = whole.clone();
        other[..4].copy_from_slice(b"FLSS");
        assert_eq!(drum.set_state(&other), version(b"FLSS"));
        for magic in [b"Kik2", b"3kiK", b"ED2K", b"ED1K"] {
            let bytes = state(magic, "", "", &DEFAULTS);
            assert_eq!(drum.set_state(&bytes), version(magic));
        }
        // A float that is no number.
        let bytes = state(b"2kiK", "", "", &knobs(&[(4, f32::NAN)]));
        let found = drum.set_state(&bytes).unwrap_err();
        assert!(matches!(found, StateError::Range { param: 4, .. }));
        assert_eq!(drum, before);
    }

    #[test]
    fn the_knobs_come_to_what_the_doc_s_table_has() {
        let near = |found: f32, wanted: f64| (f64::from(found) / wanted - 1.0).abs() < 1e-6;
        let new = Stored::of(&DEFAULTS);
        // 0 dB, a ratio of 1, an eighth of a second and 2^4.4 Hz.
        assert_eq!((new.output, new.ratio, new.duration), (1.0, 1.0, 0.125));
        assert!(near(new.base, 21.112_126));
        assert!(near(new.peak, 6.4) && near(new.slide, 1.281));
        assert_eq!((new.var_base, new.var_peak, new.var_slide), (0.0, 0.0, 0.0));
        assert_eq!(new.click_freq, 128.0);
        assert_eq!(
            (new.sub, new.drive, new.attack, new.click_amount),
            (0.0, 0.0, 0.0, 0.0)
        );

        // Base runs from 16 to 256 Hz and the click from 1 Hz to 16384.
        let ends = [Stored::of(&[0.0; PARAMS]), Stored::of(&[1.0; PARAMS])];
        assert_eq!((ends[0].base, ends[1].base), (16.0, 256.0));
        assert_eq!((ends[0].click_freq, ends[1].click_freq), (1.0, 16_384.0));
        assert!(near(ends[0].output, 0.1) && near(ends[1].output, 10.0));
        assert_eq!((ends[0].ratio, ends[1].ratio), (0.25, 4.0));
        assert_eq!((ends[0].peak, ends[1].peak), (0.0, 100.0));
        assert!(near(ends[0].slide, 0.001) && near(ends[1].slide, 20.001));
        assert_eq!((ends[0].var_base, ends[1].var_slide), (-1.0, 1.0));

        // The doc's reading of Bassdrum 01.
        let preset = Stored::of(&knobs(&[(0, 0.514), (1, 0.548), (6, 0.305), (15, 0.5)]));
        assert!((20.0 * preset.output.log10() - 0.56).abs() < 0.01);
        assert!((preset.ratio - 1.142).abs() < 0.001);
        assert!((preset.base - 37.27).abs() < 0.01);
        assert_eq!(preset.click_amount, 1.0 / 32.0);
        let half = Stored::of(&[0.5; PARAMS]);
        assert_eq!((half.duration, half.sub, half.peak), (0.125, 0.25, 12.5));
    }

    #[test]
    fn the_sweep_starts_at_base_times_one_plus_peak_and_settles_to_base() {
        // Base at 256 Hz, the sweep from 2.7 times above it, a long hit.
        let params = knobs(&[(2, 1.0), (6, 1.0), (7, 0.3), (8, 0.5)]);
        let stored = Stored::of(&params);
        let (base, peak) = (f64::from(stored.base), f64::from(stored.peak));
        let slide = f64::from(stored.slide);
        assert!((base - 256.0).abs() < 1e-9 && (peak - 2.7).abs() < 1e-6);
        assert!((slide - 2.501).abs() < 1e-6);
        let start = base * (1.0 + peak);
        assert!((start - 947.2).abs() < 1e-3);

        // The voice's own samples go through 0 where the doc's phase is a
        // whole number of half turns.
        let len = 40_000;
        let model = sweep(base, peak, slide, len);
        let (mut voice, wave) = (voice(params), sine());
        let inner: Vec<f32> = (0..len).map(|_| voice.inner(&wave, None)).collect();
        let found = crossings(&inner);
        assert!(found.len() > 400, "{} crossings", found.len());
        for (k, time) in found.iter().enumerate() {
            let n = time.floor() as usize;
            let phase = model[n] + (model[n + 1] - model[n]) * (time - n as f64);
            let wanted = 0.5 * (k + 1) as f64;
            assert!((phase - wanted).abs() < 0.01, "{phase} at crossing {k}");
        }
        // The first step of the phase is the start frequency, less what
        // one sample of the slide takes off it.
        let first = (model[0] - 0.25) * RATE;
        assert!((first / start - 1.0).abs() < 1e-3, "{first} Hz");

        // What the plugin plays: the first half turn is within 3 % of the
        // start frequency. The highpass is still settling there, which
        // moves the crossings.
        let mut drum = with(params);
        drum.note_on(note(1, 60));
        let sound = play(&mut drum, len);
        let found = crossings(&sound);
        let early = 0.5 / (found[1] - found[0]) * RATE;
        assert!((early / start - 1.0).abs() < 0.03, "{early} Hz");
        // The slide time is 2.501 / 2e-4 samples. After one of it the
        // sweep is e^-4 of the way from the base, and after two it is at
        // the base.
        let time = slide / 2e-4;
        assert!((time - 12_505.0).abs() < 1e-3);
        let around = |from: f64, to: f64| {
            let (from, to) = (from as usize, to as usize);
            pitch(&sound[from..to]) * RATE
        };
        let after_one = around(time - 300.0, time + 300.0);
        let wanted = base * (1.0 + peak * (-4.0f64).exp());
        assert!((after_one / wanted - 1.0).abs() < 0.005, "{after_one} Hz");
        let settled = around(2.0 * time, len as f64);
        assert!((settled / base - 1.0).abs() < 0.005, "{settled} Hz");
        // It never rises on the way.
        let turns: Vec<f64> = found.windows(3).map(|w| w[2] - w[0]).collect();
        assert!(turns.windows(2).all(|pair| pair[1] > pair[0] - 0.2));
    }

    #[test]
    fn the_click_bends_the_first_quarter_turn_and_nothing_after_it() {
        let wave = sine();
        let hit = |amount: f32, cut: f32| {
            // A click at 2^7 Hz over a body at 256 Hz that does not sweep.
            let params = knobs(&[(2, 1.0), (6, 1.0), (7, 0.0), (13, cut), (15, amount)]);
            let mut voice = voice(params);
            let sound: Vec<f32> = (0..4000).map(|_| voice.mixed(&wave, None)).collect();
            sound
        };
        let (plain, clicked) = (hit(0.0, 0.0), hit(1.0, 0.0));
        // With no cut the click is a quarter of a turn of 128 Hz, from
        // the wave's lowest point up to 0: 86 samples and a bit.
        let last = (0.25f64 * RATE / 128.0) as usize;
        assert_eq!(last, 86);
        assert_eq!(
            testing::bits(&plain[last + 1..]),
            testing::bits(&clicked[last + 1..])
        );
        // Inside it the phase is bent by up to 25 turns, which is a
        // burst of crossings where the plain body has one.
        let (bent, straight) = (crossings(&clicked[..=last]), crossings(&plain[..=last]));
        assert_eq!(straight.len(), 1);
        assert!(bent.len() > 20, "{} crossings", bent.len());
        // The first sample is the body at a quarter turn and one step,
        // read 25 turns of the wave's lowest point back.
        let step = 256.0 / RATE;
        let bend = 25.0 * (TAU * 0.75).sin();
        let wanted = 2.0 * (TAU * (0.25 + step + bend)).sin();
        assert!(
            (f64::from(clicked[0]) - wanted).abs() < 1e-3,
            "{}",
            clicked[0]
        );

        // Half the amount is a 32nd of the bend.
        let half = hit(0.5, 0.0);
        let wanted = 2.0 * (TAU * (0.25 + step + bend / 32.0)).sin();
        assert!((f64::from(half[0]) - wanted).abs() < 1e-3, "{}", half[0]);
        // The cut starts the click later in the quarter: cut^(1/8) of
        // the way, so all the way at 1, where there is no click left.
        assert_eq!(testing::bits(&hit(1.0, 1.0)), testing::bits(&plain));
        let cut = hit(1.0, 0.5);
        let left = ((1.0 - 0.5f64.powf(0.125)) * 0.25 * RATE / 128.0).ceil() as usize;
        assert_eq!(left, 8);
        assert_ne!(cut[left - 1].to_bits(), plain[left - 1].to_bits());
        assert_eq!(testing::bits(&cut[left..]), testing::bits(&plain[left..]));
    }

    #[test]
    fn the_second_oscillator_is_moved_by_the_variation_knobs_and_starts_late() {
        // A wave that is 1 all the way round counts the oscillators.
        let ones = [1.0f32; 1024];
        let count = |phase: f32| {
            let mut voice = voice(knobs(&[(12, phase)]));
            let sound: Vec<f32> = (0..400).map(|_| voice.mixed(&ones, None)).collect();
            sound.iter().position(|value| *value == 2.0).unwrap()
        };
        // phase^2 * 350 samples.
        assert_eq!((count(0.0), count(1.0), count(0.5)), (0, 350, 87));

        // No sweep, so that the two are steady tones: the base at 256 Hz
        // and the second an octave up, half an octave down and with it.
        let wave = sine();
        for (knob, octaves) in [(1.0, 1.0), (0.25, -0.5), (0.5, 0.0)] {
            let params = knobs(&[(6, 1.0), (7, 0.0), (9, knob)]);
            let mut voice = voice(params);
            let sound: Vec<f32> = (0..44_100).map(|_| voice.mixed(&wave, None)).collect();
            let second = 256.0 * 2f64.powf(octaves) / RATE;
            let both = if octaves == 0.0 { 2.0 } else { 1.0 };
            assert!(
                (partial(&sound, 256.0 / RATE) - both).abs() < 0.02,
                "{knob}"
            );
            assert!((partial(&sound, second) - both).abs() < 0.02, "{knob}");
        }

        // Variation peak and slide: the second sweep starts 2^peak times
        // as far above its base and takes 2^slide times as long.
        let params = knobs(&[(6, 0.5), (7, 0.5), (8, 0.5), (10, 1.0), (11, 0.0)]);
        let voice = voice(params);
        let [first, second] = voice.sweeps;
        assert_eq!(second.base, first.base);
        assert_eq!(second.peak, first.peak * 2.0);
        assert!((second.decay / first.decay - 2.0).abs() < 1e-6);
        assert_eq!((first.phase, first.e), (0.25, 1.0));
        assert_eq!(first.peak, 12.5);
        assert!((f64::from(first.base) - 64.0 / RATE).abs() < 1e-9);
        assert!((f64::from(first.decay) - 2e-4 / 2.501).abs() < 1e-10);
    }

    #[test]
    fn the_amplitude_falls_through_the_knee_and_rises_with_the_attack() {
        let ones = [1.0f32; 1024];
        for (duration, attack) in [(0.5, 0.0), (0.3, 0.0), (0.5, 0.3), (0.0, 0.0)] {
            let mut voice = voice(knobs(&[(2, duration), (5, attack)]));
            let seconds = f64::from(duration).powi(3);
            let step = if seconds > 0.0 {
                1.0 / (seconds * RATE + 44.1)
            } else {
                1.0
            };
            let rise = 1.0 / (f64::from(attack).powi(8) * 1e5 + 1.0);
            let (mut amp, mut smooth, mut risen) = (1.0f64, 1.0f64, 0.0f64);
            for n in 0..12_000 {
                let found = f64::from(voice.inner(&ones, None));
                let mut target = 0.0;
                if step < 1.0 && amp > 0.0 {
                    target = 7.0 * amp / (7.0 * amp + 1.0);
                    amp -= step;
                } else {
                    amp = 0.0;
                }
                smooth += (target - smooth) * 0.005;
                // Two oscillators of a wave that is 1.
                let wanted = risen * risen * smooth * 2.0;
                risen += (1.0 - risen) * rise;
                // The voice counts the fall down in floats.
                assert!(
                    (found - wanted).abs() < 1e-3,
                    "{found} for {wanted} at {n} of {duration}"
                );
            }
            // The fall is over after the duration and a millisecond.
            assert_eq!(voice.amp, 0.0);
        }
        // The first sample is silent: the attack rises from 0, and with
        // the knob at 0 it is there a sample later. The smoothed gain
        // starts at 1 and has hardly moved.
        let mut voice = voice(DEFAULTS);
        assert_eq!(voice.inner(&ones, None), 0.0);
        assert!((voice.inner(&ones, None) - 2.0).abs() < 0.01);
    }

    /// A voice of these knobs, and its samples before the amplitude with a
    /// wave that is `wave` all the way round over this noise sample.
    fn layered(params: [f32; PARAMS], wave: f32, noise: &[f32], len: usize) -> Vec<f32> {
        let mut voice = voice(params);
        let wave = [wave; 1024];
        (0..len).map(|_| voice.mixed(&wave, Some(noise))).collect()
    }

    #[test]
    fn the_noise_layer_is_the_sample_times_the_16th_power_of_its_envelope() {
        let ones = vec![1.0f32; 30_000];
        // No attack: the envelope starts at 0.99 and falls to S. Mix 0.5
        // is a gain of 0.5 and filter 1 lets everything through.
        for (attack, decay, sustain) in [(0.0, 0.5, 0.5), (0.0, 0.3, 0.9), (0.4, 0.5, 0.2)] {
            let params = knobs(&[(16, attack), (17, decay), (18, sustain), (21, 0.5)]);
            let found = layered(params, 0.0, &ones, 20_000);
            let fall = 1.0 / (f64::from(decay).powi(6) * RATE + 44.1);
            let (mut env, mut target, mut coef) = (0.99, f64::from(sustain), fall);
            if attack > 0.0 {
                (env, target, coef) = (0.0, 1.0, 1.0 / (RATE * f64::from(attack).powi(6)));
            }
            let mut turned = None;
            for (n, found) in found.iter().enumerate() {
                env += (target - env) * coef;
                if env > 0.99 {
                    (env, target, coef) = (0.99, f64::from(sustain), fall);
                    turned.get_or_insert(n);
                }
                let wanted = 0.5 * env.powi(16);
                assert!(
                    (f64::from(*found) - wanted).abs() < 2e-4,
                    "{found} for {wanted} at {n} of {attack}"
                );
            }
            // An attack of 0.4 has a time constant of 0.4^6 seconds, and
            // 0.99 is 4.6 of them away.
            if attack > 0.0 {
                let wanted = 100f64.ln() * RATE * f64::from(attack).powi(6);
                assert!((turned.unwrap() as f64 - wanted).abs() < 3.0, "{turned:?}");
            } else {
                assert_eq!(turned, None);
            }
            assert!((f64::from(found[19_999]) / 0.5 - f64::from(sustain).powi(16)).abs() < 0.01);
        }
        // Without a sample there is no layer, whatever the mix is.
        let mut voice = voice(knobs(&[(21, 1.0)]));
        assert_eq!(voice.mixed(&[0.0; 1024], None), 0.0);
        // The mix is 2 mix^2: twice the sample at the top of the knob.
        let loud = layered(knobs(&[(21, 1.0), (17, 1.0)]), 0.0, &ones, 1);
        assert!((f64::from(loud[0]) - 2.0 * 0.99f64.powi(16)).abs() < 1e-3);
    }

    #[test]
    fn the_noise_is_read_at_its_pitch_through_four_lowpasses() {
        // A sample that counts its own samples, held at full level.
        let ramp: Vec<f32> = (0..4000).map(|n| n as f32).collect();
        let level = 2.0 * 0.99f64.powi(16);
        for (pitch, step) in [
            (0.5, 1.0),
            (0.75, 2.0),
            (0.25, 0.5),
            (0.5625, 2f64.powf(0.25)),
        ] {
            let params = knobs(&[(17, 1.0), (18, 1.0), (20, pitch), (21, 1.0)]);
            let found = layered(params, 0.0, &ramp, 1000);
            for (n, found) in found.iter().enumerate().skip(1) {
                let wanted = n as f64 * step * level;
                // The envelope has moved a hair from 0.99 by the end.
                assert!(
                    (f64::from(*found) / wanted - 1.0).abs() < 1e-3,
                    "{found} for {wanted} at {n} of {pitch}"
                );
            }
        }
        // Past the end of the sample there is nothing.
        let short = [1.0f32; 10];
        let params = knobs(&[(17, 1.0), (18, 1.0), (21, 1.0)]);
        let found = layered(params, 0.0, &short, 20);
        assert!(found[..10].iter().all(|value| *value > 1.0));
        assert_eq!(found[10..], [0.0; 10]);

        // Filter 0.5 is four one-poles of 0.25 in a row.
        let ones = vec![1.0f32; 100];
        let found = layered(
            knobs(&[(17, 1.0), (18, 1.0), (21, 1.0), (22, 0.5)]),
            0.0,
            &ones,
            60,
        );
        let mut poles = [0.0f64; 4];
        for (n, found) in found.iter().enumerate() {
            let mut value = 1.0;
            for pole in &mut poles {
                *pole += (value - *pole) * 0.25;
                value = *pole;
            }
            assert!(
                (f64::from(*found) / level - value).abs() < 1e-3,
                "sample {n}"
            );
        }
        // Filter 0 lets nothing through.
        let shut = layered(knobs(&[(21, 1.0), (22, 0.0)]), 0.0, &ones, 60);
        assert_eq!(shut, [0.0; 60]);
    }

    #[test]
    fn noise_start_holds_back_the_oscillators_or_the_noise() {
        // The oscillators are 2 and the noise is 8.5: 5 times a mix of 2
        // and 0.99^16.
        let tens = vec![5.0f32; 30_000];
        let hit = |start: f32| {
            let params = knobs(&[(17, 1.0), (18, 1.0), (19, start), (21, 1.0)]);
            layered(params, 1.0, &tens, 6000)
        };
        // In the middle both start together.
        let together = hit(0.5);
        assert!(together.iter().all(|value| *value > 10.0));
        // All the way up, the oscillators wait 4410 samples for the noise.
        let late_body = hit(1.0);
        assert!(late_body[..4410].iter().all(|v| *v > 8.0 && *v < 9.0));
        assert!(late_body[4410..].iter().all(|v| *v > 10.0));
        // All the way down, the noise waits as long for them.
        let late_noise = hit(0.0);
        assert_eq!(late_noise[..4410], [2.0; 4410]);
        assert!(late_noise[4410..].iter().all(|v| *v > 10.0));
        // (2 (start - 0.5))^3 * 4410: an eighth of it at three quarters.
        let part = hit(0.75);
        assert_eq!(part.iter().position(|v| *v > 10.0), Some(552));
        assert!(part[..552].iter().all(|v| *v > 8.0 && *v < 9.0));
        let part = hit(0.25);
        assert_eq!(part.iter().position(|v| *v > 10.0), Some(552));
        assert_eq!(part[..552], [2.0; 552]);
    }

    /// The gain of the highpass at `hz`, once it has settled.
    fn sub_gain(q: f32, hz: f64) -> f64 {
        let mut filter = SubFilter::default();
        filter.set(42.0, q, 44_100.0);
        let tone = testing::sine(hz / RATE, 88_200);
        let out: Vec<f32> = tone.iter().map(|value| filter.process(*value)).collect();
        partial(&out[44_100..], hz / RATE)
    }

    #[test]
    fn the_highpass_peaks_near_42_hz_with_the_sub_bass_up() {
        // Sub bass all the way up: q = 6 * 1 + 0.5.
        let mut wanted = SubFilter::default();
        wanted.set(42.0, 6.5, 44_100.0);
        assert_eq!(voice(knobs(&[(3, 1.0)])).sub, wanted);
        let gains: Vec<(f64, f64)> = (20..100)
            .map(|hz| (f64::from(hz), sub_gain(6.5, f64::from(hz))))
            .collect();
        let (peak, gain) = gains
            .iter()
            .copied()
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
        assert!((41.0..=44.0).contains(&peak), "the peak is at {peak} Hz");
        // The poles are those of a filter of half the q.
        assert!((3.0..3.5).contains(&gain), "{gain}");
        assert!(sub_gain(6.5, 21.0) < 0.4 && sub_gain(6.5, 84.0) < 1.5);
        assert!((sub_gain(6.5, 2000.0) - 1.0).abs() < 0.01);

        // The knob at 0 is a q of a half: a low cut with no peak.
        let mut plain = SubFilter::default();
        plain.set(42.0, 0.5, 44_100.0);
        assert_eq!(voice(DEFAULTS).sub, plain);
        let gains: Vec<f64> = (20..100).map(|hz| sub_gain(0.5, f64::from(hz))).collect();
        assert!(gains.windows(2).all(|pair| pair[1] > pair[0]));
        assert!(gains[0] < 0.2 && gains[79] < 0.9);
        assert!((sub_gain(0.5, 4000.0) - 1.0).abs() < 0.01);

        // The coefficients are the doc's.
        let w = TAU * 42.0 / RATE;
        let (sin, cos) = w.sin_cos();
        let near = |found: f32, wanted: f64| (f64::from(found) - wanted).abs() < 1e-6;
        assert!(near(wanted.b, (1.0 + cos) / (2.0 + sin / 6.5)));
        assert!(near(wanted.a1, -2.0 * 6.5 * cos / (6.5 + sin)));
        assert!(near(wanted.a2, (6.5 - sin) / (6.5 + sin)));
    }

    #[test]
    fn the_drive_saturates_what_is_above_the_split_and_is_scaled_by_a_half() {
        // The four dividers, and that 0.5 comes out as 0.5 times the scale.
        for drive in [0.0f32, 0.3, 1.0] {
            let d = f64::from(drive).powi(4) * 2560.0;
            let chain = |h: f64| {
                let h = h / (h.abs() * d + 1.0);
                let h = h / (h * h * d + 1.0);
                let h = h / (h.powi(3).abs() * d + 1.0);
                h / (h.powi(4) * d + 1.0)
            };
            let mut voice = voice(knobs(&[(4, drive)]));
            assert!((f64::from(voice.drive) - d).abs() < 1e-3 * d.max(1.0));
            assert!((f64::from(voice.norm) * chain(0.5) - 1.0).abs() < 1e-5);
            for high in [-1.5f32, -0.2, 0.01, 0.5, 2.0] {
                let found = f64::from(saturate(high, voice.drive));
                assert!((found - chain(f64::from(high))).abs() < 1e-6);
            }
            // A frame after silence: 0.7 of it is under the split, and
            // what is above goes through the dividers. The highpass lets
            // a first frame through as it is, but for its `b`.
            let frame = 0.8f32;
            let low = 0.7 * f64::from(frame);
            let wanted = (chain(f64::from(frame) - low) / chain(0.5) + low)
                * f64::from(voice.sub.b)
                * f64::from(voice.gain)
                * 0.4;
            let found = f64::from(voice.post(frame));
            assert!((found - wanted).abs() < 1e-5, "{found} for {wanted}");
        }
        // With no drive what is above the split is doubled.
        assert_eq!(voice(DEFAULTS).norm, 2.0);
        // The gain is the Output level and the velocity.
        let mut voice = Voice::default();
        voice.start(&Stored::of(&knobs(&[(0, 1.0)])), 0.5, 1024, 44_100.0);
        assert!((voice.gain - 5.0).abs() < 1e-5);
    }

    #[test]
    fn the_sampling_ratio_and_the_host_s_rate_are_a_resampler_s() {
        // A steady 128 Hz at the voice's own 44.1 kHz.
        let params = |ratio: f32| knobs(&[(1, ratio), (2, 1.0), (6, 0.75), (7, 0.0)]);
        let hz = |ratio: f32, rate: f32| {
            let mut drum = with(params(ratio));
            drum.set_sample_rate(rate);
            drum.note_on(note(1, 60));
            let sound = play(&mut drum, (rate * 0.4) as usize);
            pitch(&sound[sound.len() / 4..]) * f64::from(rate)
        };
        // At a ratio of 1 the pitch is the same at every rate.
        for rate in [44_100.0, 48_000.0, 88_200.0, 22_050.0] {
            let found = hz(0.5, rate);
            assert!((found / 128.0 - 1.0).abs() < 0.002, "{found} Hz at {rate}");
        }
        // The knob reads the samples 2^(4 (v - 0.5)) times as fast.
        for (ratio, times) in [
            (0.75, 2.0),
            (0.25, 0.5),
            (1.0, 4.0),
            (0.0, 0.25),
            (0.625, 2f64.sqrt()),
        ] {
            let found = hz(ratio, 44_100.0);
            assert!((found / (128.0 * times) - 1.0).abs() < 0.002, "{found} Hz");
        }
        // And is heard at once by a note that sounds.
        let mut drum = with(params(0.5));
        drum.note_on(note(1, 60));
        play(&mut drum, 4000);
        let before = pitch(&play(&mut drum, 4000)) * RATE;
        drum.set_param(1, 0.75);
        let after = pitch(&play(&mut drum, 4000)) * RATE;
        assert!((before / 128.0 - 1.0).abs() < 0.01 && (after / 256.0 - 1.0).abs() < 0.01);

        // At a ratio of 1 and 44.1 kHz a frame is a sample of the voice:
        // the two it is read between are the sample and the next.
        let mut voice = voice(DEFAULTS);
        let wave = sine();
        let mut twin = voice;
        let inner: Vec<f32> = (0..50).map(|_| twin.inner(&wave, None)).collect();
        let mut post = self::voice(DEFAULTS);
        for value in &inner[..48] {
            let wanted = post.post(*value);
            assert_eq!(voice.frame(1.0, 1024, &wave, None), wanted);
        }
        // At half the step a frame in between is half way.
        let mut voice = self::voice(DEFAULTS);
        let mut post = voice;
        for n in 0..90 {
            let (a, b) = (inner[n / 2], inner[n / 2 + 1]);
            let read = if n % 2 == 0 { a } else { 0.5 * a + 0.5 * b };
            assert_eq!(voice.frame(0.5, 1024, &wave, None), post.post(read), "{n}");
        }
    }

    #[test]
    fn a_voice_stops_after_1024_silent_frames_and_inside_the_tail() {
        for (params, rate) in [
            (DEFAULTS, 44_100.0),
            (knobs(&[(3, 1.0)]), 44_100.0),
            (knobs(&[(1, 0.2), (2, 0.8), (3, 0.6), (4, 0.7)]), 48_000.0),
            (knobs(&[(2, 0.0)]), 44_100.0),
        ] {
            let mut drum = with(params);
            drum.set_sample_rate(rate);
            // 1024 frames at 44.1 kHz, and as long at any rate.
            let limit = (f64::from(rate) * 1024.0 / RATE) as usize;
            assert_eq!(drum.limit() as usize, limit);
            drum.note_on(note(1, 60));
            drum.note_off(1);
            drum.all_notes_off();
            let tail = drum.tail();
            let sound = play(&mut drum, tail);
            assert!(drum.voices.iter().all(|voice| !voice.active), "{tail}");
            // The last frame it played, the 1025 silent ones before it
            // and the loud one before those.
            let mut twin = with(params);
            twin.set_sample_rate(rate);
            twin.note_on(note(1, 60));
            let mut played = 0;
            while twin.voices[60].active {
                play(&mut twin, 1);
                played += 1;
            }
            assert!(played <= tail && played > limit + 2, "{played} of {tail}");
            let quiet = &sound[played - limit - 1..played];
            assert!(quiet.iter().all(|value| value.abs() < 1e-5));
            assert!(sound[played - limit - 2].abs() >= 1e-5);
            assert!(sound[played..].iter().all(|value| *value == 0.0));
            // A note off did nothing to it: it was heard for a while.
            assert!(sound[..played / 4].iter().any(|value| value.abs() > 0.05));
        }
        // The default is an eighth of a second and its tail under one.
        let drum = BassDrum::default();
        assert!((10_000..30_000).contains(&drum.tail()), "{}", drum.tail());
        // Twice the rate is twice the frames, and a quarter of the ratio
        // four times.
        let mut fast = BassDrum::default();
        fast.set_sample_rate(88_200.0);
        assert!(fast.tail().abs_diff(2 * drum.tail()) < 8);
        let slow = with(knobs(&[(1, 0.0)]));
        assert!(slow.tail() > 2 * drum.tail());
        // A reset cuts what sounds.
        let mut drum = BassDrum::default();
        drum.note_on(note(1, 60));
        play(&mut drum, 10);
        drum.reset();
        assert_eq!(play(&mut drum, 4), [0.0; 4]);
    }

    #[test]
    fn the_velocity_is_the_gain_and_the_key_picks_the_voice_and_nothing_else() {
        // 127 * log21(10 v + 1), cut off to a whole number.
        assert_eq!(velocity_level(1.0), 100.0 / 127.0);
        assert_eq!(velocity_level(2.0), 1.0);
        assert_eq!(velocity_level(5.0), 1.0);
        assert_eq!(velocity_level(0.0), 0.0);
        assert_eq!(velocity_level(f32::NAN), 0.0);
        // The host's gain for a velocity of 64 is (11^0.64 - 1) / 10.
        let half = ((0.64 * 11f64.ln()).exp() - 1.0) / 10.0;
        assert_eq!(velocity_level(half as f32), 64.0 / 127.0);

        let hit = |note: NoteOn| {
            let mut drum = BassDrum::default();
            drum.note_on(note);
            play(&mut drum, 3000)
        };
        let one = hit(note(1, 60));
        assert!(one.iter().any(|value| value.abs() > 0.3));
        // Every key sounds the same, and so does every pan and pitch.
        let other = NoteOn {
            pan: -1.0,
            fine_pitch: 50.0,
            mod_x: 1.0,
            ..note(2, 97)
        };
        assert_eq!(testing::bits(&hit(other)), testing::bits(&one));
        assert_eq!(hit(note(1, -5)), one);
        assert_eq!(hit(note(1, 300)), one);
        // A louder note is the same sound at its gain.
        let loud = hit(NoteOn {
            velocity: 2.0,
            ..note(1, 60)
        });
        for (loud, one) in loud.iter().zip(&one) {
            assert!((loud - one * 1.27).abs() < 1e-5);
        }
        // A velocity that comes to nothing starts nothing.
        let none = hit(NoteOn {
            velocity: 0.0001,
            ..note(1, 60)
        });
        assert_eq!(none, [0.0; 3000]);

        // Two keys are two voices and add up. The same key again starts
        // its one voice over.
        let mut drum = BassDrum::default();
        drum.note_on(note(1, 36));
        let first = play(&mut drum, 500);
        assert_eq!(first, one[..500]);
        drum.note_on(note(2, 37));
        let both = play(&mut drum, 500);
        for n in 0..500 {
            assert_eq!(both[n], one[500 + n] + one[n]);
        }
        drum.note_on(note(3, 36));
        assert_eq!(drum.voices.iter().filter(|voice| voice.active).count(), 2);
        let again = play(&mut drum, 300);
        let mut twin = BassDrum::default();
        twin.note_on(note(1, 36));
        play(&mut twin, 1000);
        twin.note_on(note(1, 36));
        let alone = play(&mut twin, 300);
        for n in 0..300 {
            assert!(
                (again[n] - (alone[n] + one[500 + n])).abs() < 1e-5,
                "frame {n}"
            );
        }
        // A voice that starts over keeps what its filters hold, so it
        // is not quite a first hit.
        assert_ne!(alone, one[..300]);
        assert!((alone[40] - one[40]).abs() < 0.2);
    }

    #[test]
    fn the_blocks_can_fall_anywhere() {
        let params = knobs(&[(1, 0.6), (3, 0.7), (4, 0.5), (12, 0.4), (15, 0.8)]);
        let whole = {
            let mut drum = with(params);
            drum.set_sample_rate(48_000.0);
            drum.note_on(note(1, 60));
            play(&mut drum, 9000)
        };
        assert!(whole.iter().any(|value| value.abs() > 0.1));
        for block in [1, 64, 513, 4000] {
            let mut drum = with(params);
            drum.set_sample_rate(48_000.0);
            drum.note_on(note(1, 60));
            let mut sound = Vec::new();
            while sound.len() < 9000 {
                sound.extend(play(&mut drum, block.min(9000 - sound.len())));
            }
            assert_eq!(testing::bits(&sound), testing::bits(&whole), "{block}");
        }
        // The sound is added to what the block holds.
        let mut drum = with(params);
        drum.set_sample_rate(48_000.0);
        drum.note_on(note(1, 60));
        let (mut left, mut right) = (vec![1.0; 200], vec![-1.0; 200]);
        drum.process(&mut left, &mut right);
        for n in 0..200 {
            assert_eq!((left[n], right[n]), (1.0 + whole[n], whole[n] - 1.0));
        }
    }

    #[test]
    fn the_parameters_are_the_23_floats_from_0_to_1() {
        let mut drum = crate::generator_by_name("bassdrum").unwrap();
        assert_eq!(drum.param_count(), 23);
        let params: Vec<f32> = (0..24).map(|index| drum.param(index)).collect();
        assert_eq!(params[..23], DEFAULTS);
        assert_eq!(params[23], 0.0);
        drum.set_param(6, 0.3);
        drum.set_param(7, 7.0);
        drum.set_param(8, -1.0);
        drum.set_param(9, f32::NAN);
        drum.set_param(23, 0.5);
        let moved = [drum.param(6), drum.param(7), drum.param(8), drum.param(9)];
        assert_eq!(moved, [0.3, 1.0, 0.0, 0.0]);
        // The next note hears a knob, and the one that sounds does not.
        let mut drum = BassDrum::default();
        drum.note_on(note(1, 60));
        let voice = drum.voices[60];
        drum.set_param(0, 1.0);
        play(&mut drum, 1);
        assert_eq!(drum.voices[60].gain, voice.gain);
        drum.note_on(note(1, 61));
        assert!((drum.voices[61].gain / voice.gain - 10.0).abs() < 1e-4);
    }

    /// A WAV file of these 16-bit samples, `channels` to a frame.
    fn wav_16(samples: &[i16], channels: u16) -> Vec<u8> {
        let data: Vec<u8> = samples.iter().flat_map(|v| v.to_le_bytes()).collect();
        wav(1, channels, 16, &data)
    }

    fn wav(tag: u16, channels: u16, bits: u16, data: &[u8]) -> Vec<u8> {
        let mut bytes = b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0".to_vec();
        bytes.extend(tag.to_le_bytes());
        bytes.extend(channels.to_le_bytes());
        bytes.extend([0; 8]);
        bytes.extend((channels * bits / 8).to_le_bytes());
        bytes.extend(bits.to_le_bytes());
        // A chunk the reader has no use for, of an odd length.
        bytes.extend(b"LIST\x03\0\0\0abc\0data");
        bytes.extend((data.len() as u32).to_le_bytes());
        bytes.extend(data);
        bytes
    }

    #[test]
    fn a_wav_file_is_read_as_its_first_channel() {
        let mono = read_wav(&wav_16(&[0, 32_767, -32_767, 16_384], 1)).unwrap();
        assert_eq!(mono, [0.0, 1.0, -1.0, 16_384.0 / 32_767.0]);
        let stereo = read_wav(&wav_16(&[1, 2, 3, 4, 5, 6], 2)).unwrap();
        assert_eq!(stereo, [1.0 / 32_767.0, 3.0 / 32_767.0, 5.0 / 32_767.0]);
        let floats: Vec<u8> = [0.25f32, -2.0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        assert_eq!(read_wav(&wav(3, 1, 32, &floats)).unwrap(), [0.25, -2.0]);
        // 8 bits, no RIFF, no data and a file that stops early.
        assert_eq!(read_wav(&wav(1, 1, 8, &[1, 2, 3])), None);
        assert_eq!(read_wav(b"RIFX\0\0\0\0WAVE"), None);
        let whole = wav_16(&[1, 2, 3], 1);
        assert_eq!(read_wav(&whole[..40]), None);
        assert_eq!(read_wav(&whole[..whole.len() - 3]).unwrap().len(), 1);
        assert_eq!(read_wav(&[]), None);
    }

    /// A directory laid out as FL's `Plugins/Fruity/Generators`, with a
    /// wave and a noise sample that are not FL's. It is gone when this is
    /// dropped.
    struct Install(PathBuf);

    impl Install {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("fl-rs-{name}-{}", std::process::id()));
            let noise = dir.join(NOISE_DIR);
            std::fs::create_dir_all(&noise).unwrap();
            let turn: Vec<u8> = (0..64)
                .flat_map(|n| if n < 32 { 1.0f32 } else { -1.0 }.to_le_bytes())
                .collect();
            std::fs::write(dir.join(WAVE), wav(3, 1, 32, &turn)).unwrap();
            std::fs::write(noise.join("Hard Clap.wav"), wav_16(&[32_767; 9000], 1)).unwrap();
            Install(dir)
        }
    }

    impl Drop for Install {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn the_wave_and_the_noise_sample_are_read_from_fl_s_directory() {
        let install = Install::new("bassdrum-files");
        let lacking = Fidelity::Approximate("BassDrum wave not available");
        let no_noise = Fidelity::Approximate("BassDrum noise sample not available");
        let path = "C:\\FL Studio 12\\Plugins\\Noise Samples\\hard clap.WAV";
        let noisy = state(b"1kiK", "", path, &knobs(&[(21, 0.5)]));

        // Without the files a sine stands in and there is no noise.
        let mut drum = BassDrum::default();
        assert_eq!(drum.fidelity(), lacking);
        drum.set_resource_dir(Path::new("/nowhere/Plugins/Fruity/Generators"));
        assert_eq!(drum.fidelity(), lacking);
        drum.set_state(&noisy).unwrap();
        assert_eq!((drum.fidelity(), &drum.noise), (lacking, &None));
        drum.note_on(note(1, 60));
        let sine = play(&mut drum, 2000);
        assert!(sine.iter().any(|value| value.abs() > 0.3));

        // The directory first and the state after, and the other way.
        let mut drum = BassDrum::default();
        drum.set_resource_dir(&install.0);
        assert_eq!(drum.fidelity(), Fidelity::Exact);
        assert_eq!(drum.wave.as_ref().unwrap().len(), 64);
        drum.set_state(&noisy).unwrap();
        assert_eq!(drum.fidelity(), Fidelity::Exact);
        let mut other = BassDrum::default();
        other.set_state(&noisy).unwrap();
        other.note_on(note(1, 60));
        other.set_resource_dir(&install.0);
        // A new wave cuts what sounds.
        assert!(!other.voices[60].active);
        assert_eq!(other, drum);
        assert_eq!(drum.noise.as_ref().unwrap()[..], [1.0; 9000]);

        // The square of the file is what the oscillators read now, and
        // the noise is under it.
        drum.note_on(note(1, 60));
        let square = play(&mut drum, 2000);
        assert_ne!(square, sine);
        let mut quiet = BassDrum::default();
        quiet.set_resource_dir(&install.0);
        quiet
            .set_state(&state(b"1kiK", "", path, &DEFAULTS))
            .unwrap();
        quiet.note_on(note(1, 60));
        assert_ne!(play(&mut quiet, 2000), square);

        // A sample the directory does not have is lacking where the mix
        // would be heard, and not otherwise.
        let gone = "%FLStudioFactoryData%/Plugins/Noise Samples/Soft Clap.wav";
        drum.set_state(&state(b"2kiK", "", gone, &knobs(&[(21, 0.5)])))
            .unwrap();
        assert_eq!((drum.fidelity(), &drum.noise), (no_noise, &None));
        drum.set_param(21, 0.0);
        assert_eq!(drum.fidelity(), Fidelity::Exact);
        drum.set_state(&state(b"2kiK", "", "", &knobs(&[(21, 0.5)])))
            .unwrap();
        assert_eq!(drum.fidelity(), Fidelity::Exact);
    }

    /// FL's directory of generators, where `FL_RS_CORPUS` names an
    /// install's `Data`.
    fn fl_s_directory() -> Option<(PathBuf, PathBuf)> {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, BassDrum's files and states skipped");
            return None;
        };
        let dir = root.join("../Plugins/Fruity/Generators");
        Some((root, dir))
    }

    #[test]
    fn fl_s_wave_is_1024_points_near_a_sine_and_its_noise_samples_are_read() {
        let Some((_, dir)) = fl_s_directory() else {
            return;
        };
        let mut drum = BassDrum::default();
        drum.set_resource_dir(&dir);
        assert_eq!(drum.fidelity(), Fidelity::Exact, "no {WAVE}");
        let wave = drum.wave.clone().unwrap();
        assert_eq!(wave.len(), 1024);
        // It goes through 0 twice a turn, as a sine does, and is up to
        // 0.31 away from one.
        assert_eq!(crossings(&[&wave[..], &wave[..2]].concat()).len(), 2);
        let off = wave.iter().zip(&drum.sine).map(|(a, b)| (a - b).abs());
        let off = off.fold(0.0, f32::max);
        assert!((0.25..0.35).contains(&off), "{off}");
        let peak = wave.iter().fold(0.0f32, |peak, v| peak.max(v.abs()));
        assert!((0.99..=1.0).contains(&peak), "{peak}");

        for name in [
            "Evolving 1",
            "Evolving 2",
            "Hard Clap",
            "H-Freq Metal",
            "Lo-Fi Crunch",
            "Low Bit",
            "Metallic",
            "Soft Clap",
        ] {
            let path = format!("D:\\Image-Line\\Noise Samples\\{name}.wav");
            drum.set_state(&state(b"1kiK", "", &path, &knobs(&[(21, 0.5)])))
                .unwrap();
            assert_eq!(drum.fidelity(), Fidelity::Exact, "{name}");
            let noise = drum.noise.as_ref().unwrap();
            assert!(noise.len() > 19_000, "{name}");
            assert!(noise.iter().all(|value| value.abs() <= 1.0001), "{name}");
        }
    }

    #[test]
    fn every_preset_and_every_state_in_fl_s_projects_loads_and_plays_out() {
        let Some((root, dir)) = fl_s_directory() else {
            return;
        };
        let mut magics = std::collections::BTreeMap::new();
        let mut saved = testing::every_preset(&root, NAME);
        assert_eq!(saved.len(), 56);
        let ours = |saved: &testing::Saved| saved.plugin.eq_ignore_ascii_case(NAME);
        saved.extend(testing::projects(&root).into_iter().filter(ours));
        assert_eq!(saved.len(), 56 + 5);
        for saved in saved {
            assert_eq!(saved.state.len(), STATE_LEN, "{}", saved.file);
            let mut drum = crate::generator_by_name(&saved.plugin).unwrap();
            // Without FL's files it says what it lacks.
            if let Err(error) = drum.set_state(&saved.state) {
                panic!("{}: {error}", saved.file);
            }
            assert_ne!(drum.fidelity(), Fidelity::Exact, "{}", saved.file);
            drum.set_resource_dir(&dir);
            assert_eq!(drum.fidelity(), Fidelity::Exact, "{}", saved.file);
            // The state's floats are the plugin's parameters.
            for index in 0..PARAMS {
                let at = 8 + PARAMS_AT + 4 * index;
                let word = f32::from_le_bytes(saved.state[at..at + 4].try_into().unwrap());
                assert_eq!(drum.param(index), word, "{}", saved.file);
            }
            // A hit is heard, stays in bounds and is over inside the tail.
            drum.note_on(note(1, 60));
            let tail = drum.tail();
            let (mut left, mut right) = (vec![0.0; tail], vec![0.0; tail]);
            drum.process(&mut left, &mut right);
            let peak = left.iter().fold(0.0f32, |peak, v| peak.max(v.abs()));
            assert!(left.iter().all(|v| v.is_finite()), "{}", saved.file);
            assert!((0.05..16.0).contains(&peak), "{peak} in {}", saved.file);
            let (mut left, mut right) = ([0.0; 64], [0.0; 64]);
            drum.process(&mut left, &mut right);
            assert_eq!(left, [0.0; 64], "{} sounds on past {tail}", saved.file);
            let magic = String::from_utf8_lossy(&saved.state[8..12]).into_owned();
            *magics.entry(magic).or_insert(0) += 1;
        }
        // The 5 projects and 4 of the presets have the older magic.
        let found: Vec<(&str, i32)> = magics.iter().map(|(k, v)| (k.as_str(), *v)).collect();
        assert_eq!(found, [("1kiK", 9), ("2kiK", 52)]);
    }
}
