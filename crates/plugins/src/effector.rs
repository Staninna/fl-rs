//! Effector: twelve performance effects in one plugin, one of them on at a
//! time, as `docs/plugins/effector.md` has it.
//!
//! The DSP is in the `engine_x64.dll` beside the plugin, a C++ plugin of
//! Image-Line's own FX framework. Its state is the framework's chunk: the
//! bytes `SSLF`, the length of what follows, the bytes `FX01` in a
//! little-endian word, 256 bytes of program name and the twelve parameters
//! as f32 from 0 to 1.
//!
//! | parameter | default | what it does |
//! |---|---|---|
//! | 0 Effect Kind | 21/22 (11) | `trunc(11 v + 0.5)`: which effect is on |
//! | 1 FX Bypass | 1 (on) | any value but 0 turns every effect off |
//! | 2 Wet Mix | 1 | the effect's share of the output |
//! | 3, 4 Param X, Y | 0.5 | the two knobs, whose meaning each effect gives |
//! | 5 Mod Rate | 1/9 | one of 1/8, 1/4, 3/8, 1/2, 3/4, 1, 2, 4, 8 beats a cycle |
//! | 6 Tempo Reduce | 0 | the cycle shrinks by `(1 - v)^2` |
//! | 7, 8 Mod Amount X, Y | 0.5 | `(2v - 1)` squared with its sign: how far the LFO moves X and Y |
//! | 9 Mod Shape | 0 | sine, saw or square |
//! | 10 Input Level | 1 | `v^2` |
//! | 11 Output Gain | 0.5 | `10^(2.4 (v - 0.5))`, 24 dB each way |
//!
//! The effects the shipped projects use are built: 0 distortion, 1 lo-fi,
//! 2 flanger, 6 reverb, 8 the trance gate the projects call a ducker, and
//! 10 the vowel filter. The other six, 3, 4, 5, 7, 9 and 11, pass the input
//! through and say so in [`Effect::fidelity`].
//!
//! FL's host hands the plugin a parameter as a whole number from 0 to 256,
//! which it divides by 256. [`Effect::param`] and [`Effect::set_param`]
//! count in those.

use crate::{Effect, Fidelity, StateError, knob_value};
use std::f64::consts::TAU;

/// The plugin's event 201 name.
pub const NAME: &str = "Effector";

/// The parameters, as FL numbers them.
const PARAMS: usize = 12;
const KIND: usize = 0;
const BYPASS: usize = 1;
const WET: usize = 2;
const X: usize = 3;
const Y: usize = 4;
const RATE: usize = 5;
const REDUCE: usize = 6;
const MOD_X: usize = 7;
const MOD_Y: usize = 8;
const SHAPE: usize = 9;
const INPUT: usize = 10;
const OUTPUT: usize = 11;

/// What a new instance starts with (`FUN_180017920`): effect 11 and the
/// effects bypassed, so the plugin plays the input until a project says
/// otherwise.
const DEFAULTS: [f32; PARAMS] = [
    f32::from_bits(0x3f74_5d17),
    1.0,
    1.0,
    0.5,
    0.5,
    f32::from_bits(0x3de3_8e39),
    0.0,
    0.5,
    0.5,
    0.0,
    1.0,
    0.5,
];

/// The whole numbers FL's host counts a parameter in: 256 is 1.
const STEPS: f32 = 256.0;

/// The state's tags and how long it is: 8 bytes of tag and length, then
/// `FX01`, 256 bytes of name and the parameters.
const MAGIC: &[u8; 4] = b"SSLF";
const FX01: u32 = 0x4658_3031;
const NAME_BYTES: usize = 256;
const LEN: usize = 8 + 4 + NAME_BYTES + 4 * PARAMS;

/// The effects, numbered as the Effect Kind wheel counts them.
const DISTORTION: usize = 0;
const LOFI: usize = 1;
const FLANGER: usize = 2;
const REVERB: usize = 6;
const GATE: usize = 8;
const VOWEL: usize = 10;
const KINDS: usize = 12;

/// The Mod Rate wheel's steps, in beats a cycle.
const BEATS: [f32; 9] = [0.125, 0.25, 0.375, 0.5, 0.75, 1.0, 2.0, 4.0, 8.0];

/// The most frames one pass of the effects takes: the reverb copies its
/// input into buffers of this size on the stack in FL.
const CHUNK: usize = 1024;

/// The one-pole smoothers' steps: the wet mix, the fades and the input
/// level move 1 % of the way a frame, the output gain 0.5 %.
const STEP: f32 = 0.01;
const OUT_STEP: f32 = 0.005;

/// A bypassed effect whose fade is under this starts over.
const SILENT: f32 = 1e-4;

/// What the band filters and the tape delay flush to 0.
const TINY: f32 = 1e-9;

fn flush(x: f32) -> f32 {
    if x < TINY && x > -TINY { 0.0 } else { x }
}

/// `x` inside 0..1 the way FL's code clamps: a NaN stays a NaN.
fn unit(x: f32) -> f32 {
    let x = if x <= 0.0 { 0.0 } else { x };
    if x >= 1.0 { 1.0 } else { x }
}

/// What is left of `x` over its whole part, which is how FL wraps a phase.
fn frac(x: f64) -> f64 {
    x - f64::from(x as i32)
}

/// One band of the vowel filter (`FUN_180014520` and `FUN_180014680`): a
/// band-pass biquad whose skirts fall at 6 dB an octave and whose peak is
/// `q`, the cookbook's constant-skirt form.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Band {
    freq: f32,
    q: f32,
    hz: f32,
    b0: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Default for Band {
    /// As FL builds one: 100 Hz, a `q` of 1, at 44.1 kHz.
    fn default() -> Self {
        let mut band = Band {
            freq: 0.0,
            q: 0.0,
            hz: 0.0,
            b0: 0.0,
            a1: 0.0,
            a2: 0.0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        };
        band.tune(100.0, 1.0, 44_100.0);
        band
    }
}

impl Band {
    /// The coefficients, worked out again when one of the three moved. The
    /// sums are in f64 and each coefficient is rounded to f32 twice, as in
    /// FL.
    fn tune(&mut self, freq: f32, q: f32, hz: f32) {
        if freq == self.freq && q == self.q && hz == self.hz {
            return;
        }
        (self.freq, self.q, self.hz) = (freq, q, hz);
        let w = f64::from(freq) * TAU / f64::from(hz);
        let (sin, cos) = w.sin_cos();
        let norm = 1.0 / ((0.5 / f64::from(q)) * sin + 1.0);
        let b0 = (f64::from((sin * 0.5) as f32) * norm) as f32;
        let a1 = (f64::from((cos + cos) as f32) * norm) as f32;
        let a2 = (f64::from((1.0 - sin / f64::from(q + q)) as f32) * norm) as f32;
        (self.b0, self.a1, self.a2) = (flush(b0), flush(a1), flush(a2));
    }

    fn run(&mut self, x: f32) -> f32 {
        let x = flush(x);
        let y = flush(((x - self.x2) * self.b0 + self.y1 * self.a1) - self.a2 * self.y2);
        (self.x2, self.x1) = (self.x1, x);
        (self.y2, self.y1) = (self.y1, y);
        y
    }

    fn clear(&mut self) {
        (self.x1, self.x2, self.y1, self.y2) = (0.0, 0.0, 0.0, 0.0);
    }
}

/// One turn of a [`Tape`]: the samples written in it, each with its place
/// on the turn. Entries 7, 8 and 9 hold the last three of the turn before,
/// a turn back, and the turn's own start at 10.
#[derive(Debug, Clone, PartialEq)]
struct Turn {
    values: Vec<f32>,
    places: Vec<f64>,
    count: usize,
    read: usize,
}

impl Turn {
    fn new(len: usize) -> Self {
        Turn {
            values: vec![0.0; len],
            places: vec![0.0; len],
            count: 0,
            read: 0,
        }
    }

    fn clear(&mut self) {
        self.values.fill(0.0);
        self.places.fill(0.0);
        (self.count, self.read) = (0, 0);
    }
}

/// FL's `CTapeDelay` (`FUN_180017f80`, `FUN_1800181c0`): a tape loop that
/// turns once in the delay time. Each frame the head moves on by the speed
/// and writes the input with its place; the sample read is the one written
/// a turn before at the same place, found in the other turn's list and
/// interpolated between its two neighbours. A change of time changes the
/// speed, which glides 0.5 % a frame, so the delay bends like tape.
#[derive(Debug, Clone, PartialEq)]
struct Tape {
    /// The longest delay, in samples.
    len: i32,
    turns: [Turn; 2],
    /// Which of the two turns is being written.
    writing: usize,
    target: f64,
    speed: f64,
    place: f64,
    /// The slowest speed: a turn in `len` samples.
    slowest: f64,
    feedback: f32,
    /// A one-pole low-pass and one high-pass on the feedback path; the
    /// flanger leaves the first open and the second shut.
    low_k: f32,
    high_k: f32,
    low: f32,
    high: f32,
}

impl Tape {
    fn new(len: i32) -> Self {
        let len = len.max(4);
        let room = len as usize + 256;
        let mut tape = Tape {
            len,
            turns: [Turn::new(room), Turn::new(room)],
            writing: 0,
            target: 0.0,
            speed: 0.0,
            place: 0.0,
            slowest: 1.0 / f64::from(len),
            feedback: 0.0,
            low_k: 1.0,
            high_k: 0.0,
            low: 0.0,
            high: 0.0,
        };
        tape.set_time(0.5);
        tape.clear();
        tape
    }

    /// `t` from 0 to 1 of the way from 2 samples to the longest delay
    /// (`FUN_180018520`).
    fn set_time(&mut self, t: f64) {
        let shortest = 2.0 / f64::from(self.len);
        let t = if t <= 0.0 { 0.0 } else { t };
        self.target = 1.0 / ((t * (1.0 - shortest) + shortest) * f64::from(self.len));
    }

    /// `FUN_180018120`.
    fn clear(&mut self) {
        self.place = 0.0;
        self.speed = self.target;
        (self.low, self.high) = (0.0, 0.0);
        for turn in &mut self.turns {
            turn.clear();
        }
    }

    /// Write `x` and give what the head read, before the feedback filters.
    fn run(&mut self, x: f32) -> f32 {
        let x = flush(x);
        let read = &mut self.turns[1 - self.writing];
        let mut back = 0.0;
        if read.count != 0 {
            while read.read <= read.count {
                let at = 10 + read.read;
                let to = read.places[at];
                if self.place <= to {
                    let (from, to) = (read.places[at - 1] as f32, to as f32);
                    let frac = if from < to {
                        (self.place - f64::from(from)) / f64::from(to - from)
                    } else {
                        0.0
                    };
                    let (v0, v1) = (read.values[at - 1], read.values[at]);
                    let value = (f64::from(v1 - v0) * frac + f64::from(v0)) as f32;
                    if value >= TINY || value <= -TINY {
                        back = value;
                    }
                    break;
                }
                read.read += 1;
            }
        }
        self.low = (back - self.low) * self.low_k + self.low;
        self.high = (self.low - self.high) * self.high_k + self.high;
        let written = (self.low - self.high) * self.feedback + x;
        let speed = self.speed.min(0.9).max(self.slowest);
        self.place += speed;
        if self.place >= 1.0 {
            let [a, b] = &mut self.turns;
            let (write, read) = if self.writing == 0 { (a, b) } else { (b, a) };
            for k in 7..10 {
                write.values[k] = read.values[k + read.count];
                write.places[k] = read.places[k + read.count] - 1.0;
            }
            write.values[10 + write.count] = written;
            write.places[10 + write.count] = self.place;
            write.read = 0;
            self.writing = 1 - self.writing;
            self.place -= 1.0;
            self.turns[self.writing].count = 0;
        }
        let write = &mut self.turns[self.writing];
        write.values[10 + write.count] = written;
        write.places[10 + write.count] = self.place;
        write.count += 1;
        self.speed += (self.target - self.speed) * 0.005;
        back
    }
}

/// The plate lines' lengths at the largest room, in 20000ths of the rate
/// (`FUN_18001c090`): four input diffusers, then two halves of a tank, each
/// an allpass, a delay, an allpass and a delay.
const LINES: [i32; 12] = [
    142, 107, 379, 277, 672, 4453, 1800, 3720, 908, 4217, 2656, 3163,
];

/// Where the outputs tap the tank, in the same 20000ths: the left side's
/// seven and the right side's seven (`FUN_18001dae0`).
const TAPS_L: [i32; 7] = [266, 2974, 1913, 2496, 1690, 187, 1566];
const TAPS_R: [i32; 7] = [353, 3627, 1728, 3173, 1811, 335, 621];

/// The rate the Effector's reverb is tuned for, whatever the song's: the
/// unit tells it 44100 (`FUN_180018a50`).
const PLATE_HZ: f32 = 44_100.0;

/// What FL fills the reverb's lines with when it clears them.
const DENORMAL: f32 = 1e-30;

/// The reverb's 18 parameters as its preset 0 has them (`FUN_18001bff0`),
/// and how each becomes a whole number: `floor(v * scale + offset)`.
const PLATE_DEFAULTS: [u32; 18] = [
    0x3f5b_83f4,
    0x3c99_e459,
    0x3e25_ed09,
    0,
    0x3efd_6a05,
    0x3f80_0000,
    0x3e92_4925,
    0x3e76_474b,
    0x3d90_1495,
    0x3e25_ed09,
    0x3f5b_83f4,
    0x3f46_3b97,
    0x3f12_8bdc,
    0x3f00_0000,
    0,
    0,
    0,
    0,
];
const PLATE_SCALES: [(f32, f32); 18] = [
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

/// The reverb's parameters, as `FUN_18001d3f0` names them. 11 is the
/// early reflections' level, which the Effector mutes.
const INPUT_DB: usize = 0;
const LOW_CUT: usize = 1;
const HIGH_CUT: usize = 2;
const PREDELAY: usize = 3;
const ROOM: usize = 4;
const DIFFUSION: usize = 5;
const BASS: usize = 6;
const CROSSOVER: usize = 7;
const DECAY: usize = 8;
const DAMPING: usize = 9;
const DRY_DB: usize = 10;
const REVERB_DB: usize = 12;
const WIDTH: usize = 13;
const MUTE_INPUT: usize = 14;
const MUTE_DRY: usize = 15;
const MUTE_ER: usize = 16;
const MUTE_REVERB: usize = 17;

/// A level in tenths of a dB, as the reverb counts them: -36.1 dB and
/// under is silence (`FUN_18001dab0`).
fn tenths_db(level: i32) -> f32 {
    if level < -360 {
        0.0
    } else {
        10f32.powf(level as f32 * 0.005)
    }
}

/// The one-pole low-pass coefficient the reverb works out for a corner in
/// hundreds of Hz: 1 at 221, which is off.
fn one_pole(hundreds: i32) -> f32 {
    if (hundreds as f32) < 221.0 {
        let c = 2.0 - (hundreds as f32 * f32::from_bits(0x441d_1463) / PLATE_HZ).cos();
        1.0 - (c - (c * c - 1.0).sqrt())
    } else {
        1.0
    }
}

/// FL's `fxReverb`, the reverb behind effect 6: the same plate as Fruity
/// Reeverb 2's engine, without its modulation, with its own gains for the
/// input diffusers and a dry path. Its early reflections are left out: the
/// Effector mutes them.
#[derive(Debug, Clone, PartialEq)]
struct Plate {
    ints: [i32; 18],
    tuned: bool,
    cleared_room: i32,
    lines: [Vec<f32>; 12],
    len: [usize; 12],
    at: [usize; 12],
    pre: Vec<f32>,
    pre_len: usize,
    pre_at: usize,
    taps_l: [usize; 7],
    taps_r: [usize; 7],
    input: f32,
    low_k: f32,
    high_k: f32,
    diffuse: [f32; 2],
    tank: [f32; 2],
    decay: f32,
    bass: f32,
    cross: f32,
    damp: f32,
    dry: f32,
    wet_to: f32,
    /// The input high cut, and the low cut's allpass output and input.
    low: f32,
    cut: [f32; 2],
    /// Each half's crossover input and output, and damping state.
    cross_a: [f32; 2],
    cross_b: [f32; 2],
    damp_a: f32,
    damp_b: f32,
    wet: f32,
}

impl Default for Plate {
    fn default() -> Self {
        let hz = PLATE_HZ as i32;
        let lines = LINES.map(|t| vec![0.0; (t * hz / 20_000).max(1) as usize]);
        let mut plate = Plate {
            ints: [0; 18],
            tuned: false,
            cleared_room: 0,
            len: lines.each_ref().map(Vec::len),
            lines,
            at: [0; 12],
            pre: vec![0.0; (hz * 250 / 1000).max(1) as usize],
            pre_len: 1,
            pre_at: 0,
            taps_l: [0; 7],
            taps_r: [0; 7],
            input: 0.0,
            low_k: 0.0,
            high_k: 1.0,
            diffuse: [0.0; 2],
            tank: [0.0; 2],
            decay: 0.0,
            bass: 0.0,
            cross: 0.0,
            damp: 1.0,
            dry: 0.0,
            wet_to: 0.0,
            low: 0.0,
            cut: [0.0; 2],
            cross_a: [0.0; 2],
            cross_b: [0.0; 2],
            damp_a: 0.0,
            damp_b: 0.0,
            wet: -1000.0,
        };
        for (param, bits) in PLATE_DEFAULTS.iter().enumerate() {
            plate.set(param, f32::from_bits(*bits));
        }
        plate.clear();
        plate
    }
}

impl Plate {
    /// One parameter from 0 to 1 (`FUN_18001cc30`). It takes effect at the
    /// next [`Plate::run`].
    fn set(&mut self, param: usize, v: f32) {
        let (scale, offset) = PLATE_SCALES[param];
        let int = (v * scale + offset).floor() as i32;
        if self.ints[param] != int {
            self.ints[param] = int;
            self.tuned = false;
        }
    }

    /// `FUN_18001ca20`: the lines filled with the anti-denormal value and
    /// the filters emptied.
    fn clear(&mut self) {
        for line in &mut self.lines {
            line.fill(DENORMAL);
        }
        self.pre.fill(DENORMAL);
        self.low = 0.0;
        self.cut = [0.0; 2];
        self.cross_a = [0.0; 2];
        self.cross_b = [0.0; 2];
        (self.damp_a, self.damp_b) = (0.0, 0.0);
        self.wet = -1000.0;
        self.tuned = false;
    }

    /// `FUN_18001dae0`: lengths, taps and coefficients from the whole
    /// numbers.
    fn tune(&mut self) {
        let ints = self.ints;
        let hz = PLATE_HZ;
        let rate = hz as i32;
        self.input = if ints[MUTE_INPUT] != 0 {
            0.0
        } else {
            tenths_db(ints[INPUT_DB])
        };
        let low_cut = ints[LOW_CUT] as f32;
        self.low_k = if low_cut > 19.0 {
            let t = ((hz * 0.5).min(low_cut) * std::f32::consts::PI / hz).tan();
            (t - 1.0) / (t + 1.0)
        } else {
            0.0
        };
        self.high_k = one_pole(ints[HIGH_CUT]);
        self.pre_len = ((rate * ints[PREDELAY]) / 1000).max(1) as usize;
        self.pre_at %= self.pre_len;
        let scale = ((ints[ROOM] + 15) * rate) / 115;
        for (n, t) in LINES.iter().enumerate() {
            self.len[n] = ((t * scale) / 20_000).max(1) as usize;
            self.at[n] %= self.len[n];
        }
        for k in 0..7 {
            self.taps_l[k] = ((scale * TAPS_L[k]) / 20_000) as usize;
            self.taps_r[k] = ((scale * TAPS_R[k]) / 20_000) as usize;
        }
        if ints[ROOM] != self.cleared_room {
            self.cleared_room = ints[ROOM];
            for (line, len) in self.lines.iter_mut().zip(self.len) {
                line[..len].fill(0.0);
            }
        }
        let lens = self.len;
        let len = |n: usize| lens[n] as f32;
        let d = ints[DIFFUSION] as f32 * 0.01;
        if d == 0.0 {
            (self.diffuse, self.tank) = ([0.0; 2], [0.0; 2]);
        } else {
            let per = 1.0 / hz;
            let k = f32::from_bits(0xc05d_0c55);
            self.diffuse = [
                ((f32::from_bits(0xc245_5d4c) / d) * (len(1) + len(0)) * per).exp(),
                ((f32::from_bits(0xc1fb_30ec) / d) * (len(2) + len(3)) * per).exp(),
            ];
            self.tank = [
                ((len(4) + len(8)) * k / hz * (f32::from_bits(0x4036_db6e) / d)).exp(),
                (k / hz * (len(6) + len(10)) * (f32::from_bits(0x3ffa_fafb) / d)).exp(),
            ];
        }
        let seconds = ints[DECAY] as f32 * 0.1;
        let loop_ = (len(9) + len(7) + len(5) + len(11)) / hz * f32::from_bits(0xc0dd_0c55);
        self.decay = (loop_ / seconds).exp().powf(0.25);
        let bass = ((10.0 / (ints[BASS] as f32 * seconds)) * loop_)
            .exp()
            .powf(0.25);
        let r = bass / self.decay;
        self.bass = (r - 1.0) * 0.5;
        let r = r.min(1.0);
        let t = (ints[CROSSOVER] as f32 * std::f32::consts::PI / hz).tan();
        self.cross = (t - r) / (r + t);
        self.damp = one_pole(ints[DAMPING]);
        self.dry = if ints[MUTE_DRY] != 0 {
            0.0
        } else {
            tenths_db(ints[DRY_DB])
        };
        self.wet_to = if ints[MUTE_REVERB] != 0 {
            0.0
        } else {
            tenths_db(ints[REVERB_DB]) * 0.3
        };
        self.tuned = true;
    }

    /// Add the reverb of `input` to `out` (`FUN_18001e9f0`).
    fn run(&mut self, input: [&[f32]; 2], out: [&mut [f32]; 2]) {
        if !self.tuned {
            self.tune();
        }
        let g = self.decay;
        let rest = 1.0 - g;
        let w6 = rest * 0.6 + g;
        let w25 = rest * 0.25 + g;
        let w7 = rest * 0.7 + g;
        let w2 = rest * 0.2 + g;
        let w5 = rest * 0.5 + g;
        let width = self.ints[WIDTH] as f32 * 0.01;
        let [g1, g2] = self.diffuse;
        let [t1, t2] = self.tank;
        let [out_l, out_r] = out;
        let frames = input[0]
            .iter()
            .zip(input[1])
            .zip(out_l.iter_mut().zip(out_r));
        for ((l, r), (out_l, out_r)) in frames {
            let xl = self.input * *l;
            let xr = self.input * *r;
            self.low = (((xr + xl) * 0.5 + 1e-30) - self.low) * self.high_k + self.low;
            let lp = self.low;
            let ap = (lp - self.cut[0]) * self.low_k + self.cut[1];
            self.cut = [ap, lp];
            let x = if self.low_k != 0.0 {
                (lp - ap) * 0.5 + 1e-30
            } else {
                lp
            };
            let v0 = self.allpass(0, x, g1, g1);
            let v1 = self.allpass(1, v0, g2, g1);
            let v2 = self.allpass(2, v1, g1, g1);
            let v3 = self.allpass(3, v2, g2, g1);
            let pd = self.pre[self.pre_at] + 1e-20;
            self.pre[self.pre_at] = v3;
            self.pre_at = self.pre_at.checked_sub(1).unwrap_or(self.pre_len - 1);

            // The first half reads the second's last delay.
            let c = self.lines[11][self.at[11]] * g;
            let apx = (c - self.cross_a[1]) * self.cross + self.cross_a[0];
            self.cross_a = [c, apx];
            let y = (((apx + c) * self.bass + c + pd) - self.damp_a) * self.damp + self.damp_a;
            self.damp_a = y;
            let t = self.allpass(4, y, t1, t1);
            let d = self.delay(5, t) * g;
            let u = self.allpass(6, d, t2, t2);
            self.delay(7, u);

            // And the second the first's.
            let c = self.lines[7][self.at[7]] * g;
            let apx = (c - self.cross_b[1]) * self.cross + self.cross_b[0];
            self.cross_b = [c, apx];
            let y = (((apx + c) * self.bass + c + pd) - self.damp_b) * self.damp + self.damp_b;
            self.damp_b = y;
            let t = self.allpass(8, y, t1, t1);
            let d = self.delay(9, t) * g;
            let u = self.allpass(10, d, t2, t2);
            self.delay(11, u);

            if self.wet < -99.0 {
                self.wet = self.wet_to;
            }
            self.wet = (self.wet_to - self.wet) * 0.001 + self.wet;
            let tap = |line: usize, tap: usize| -> f32 {
                let at = self.at[line] + tap;
                let len = self.len[line];
                self.lines[line][if at < len { at } else { at - len }]
            };
            let (tl, tr) = (self.taps_l, self.taps_r);
            let left = self.wet
                * ((((tap(9, tl[0]) - w25 * tap(9, tl[1])) - w7 * tap(10, tl[2]))
                    - w2 * tap(11, tl[3]))
                    + w6 * tap(5, tl[4])
                    + tap(6, tl[5])
                    + w5 * tap(7, tl[6]));
            let right = self.wet
                * (((((w25 * tap(5, tr[1]) + tap(5, tr[0])) - w7 * tap(6, tr[2]))
                    - w2 * tap(7, tr[3]))
                    - w6 * tap(9, tr[4]))
                    + tap(10, tr[5])
                    + w5 * tap(11, tr[6]));
            let mid = right + left;
            let side = (left - right) * width;
            *out_l += xl * self.dry + side + mid;
            *out_r += xr * self.dry + (mid - side);
        }
    }

    /// A Schroeder allpass on line `n` that takes `g` on the way in and
    /// `h` on the way back, and moves the line on.
    fn allpass(&mut self, n: usize, x: f32, g: f32, h: f32) -> f32 {
        let at = self.at[n];
        let v = self.lines[n][at] - x * g;
        self.lines[n][at] = v * h + x;
        self.at[n] = at.checked_sub(1).unwrap_or(self.len[n] - 1);
        v
    }

    /// The sample `len` frames old on line `n`, swapped for `x`.
    fn delay(&mut self, n: usize, x: f32) -> f32 {
        let at = self.at[n];
        let old = self.lines[n][at];
        self.lines[n][at] = x;
        self.at[n] = at.checked_sub(1).unwrap_or(self.len[n] - 1);
        old
    }
}

/// What one effect keeps besides what they all have.
#[derive(Debug, Clone, PartialEq)]
enum Body {
    /// Effects 3, 4, 5, 7, 9 and 11: not built.
    None,
    /// The distortion's two tone low-passes.
    Distortion {
        low: [f32; 2],
    },
    /// The lo-fi's countdown to the next sample and the samples it holds.
    LoFi {
        count: f32,
        held: [f32; 2],
    },
    Flanger(Box<[Tape; 2]>),
    Reverb(Box<Plate>),
    /// The gate's smoothed envelope.
    Gate {
        duck: f32,
    },
    /// Eight bands a side.
    Vowel(Box<[[Band; 8]; 2]>),
}

/// One of the twelve effects (`FUN_180018590`, 0xb28 bytes in FL).
#[derive(Debug, Clone, PartialEq)]
struct Unit {
    kind: usize,
    hz: f32,
    x: f32,
    y: f32,
    /// The LFO's cycle in beats, and in seconds at the tempo.
    beats: f32,
    seconds: f32,
    mod_x: f32,
    mod_y: f32,
    shape: u8,
    bypassed: bool,
    /// The fade in and out an effect with a tail makes when it is turned
    /// on and off.
    fade: f32,
    phase: f64,
    step: f64,
    wet: f32,
    wet_now: f32,
    body: Body,
}

impl Unit {
    fn new(kind: usize) -> Self {
        let body = match kind {
            DISTORTION => Body::Distortion { low: [0.0; 2] },
            LOFI => Body::LoFi {
                count: 0.0,
                held: [0.0; 2],
            },
            FLANGER => Body::Flanger(Box::new([Tape::new(4), Tape::new(4)])),
            REVERB => Body::Reverb(Box::default()),
            GATE => Body::Gate { duck: 0.0 },
            VOWEL => Body::Vowel(Box::default()),
            _ => Body::None,
        };
        let mut unit = Unit {
            kind,
            hz: 0.0,
            x: 0.0,
            y: 0.0,
            beats: 1.0,
            seconds: 0.0,
            mod_x: 0.0,
            mod_y: 0.0,
            shape: 0,
            bypassed: false,
            fade: 0.0,
            phase: 0.0,
            step: 0.0,
            wet: 0.0,
            wet_now: -1.0,
            body,
        };
        unit.set_rate(44_100.0);
        unit
    }

    /// `FUN_180018a50`: a new rate builds the delay lines again, and every
    /// call empties them.
    fn set_rate(&mut self, hz: f32) {
        if hz != 0.0 && hz != self.hz {
            self.hz = hz;
            if let Body::Flanger(tapes) = &mut self.body {
                let len = (hz * 0.005) as i32;
                **tapes = [Tape::new(len), Tape::new(len)];
            }
        }
        match &mut self.body {
            Body::None => {}
            Body::Distortion { low } => *low = [0.0; 2],
            Body::LoFi { count, held } => (*count, *held) = (0.0, [0.0; 2]),
            Body::Flanger(tapes) => tapes.iter_mut().for_each(Tape::clear),
            Body::Reverb(plate) => plate.clear(),
            Body::Gate { duck } => *duck = 0.0,
            Body::Vowel(bands) => bands.iter_mut().flatten().for_each(Band::clear),
        }
        self.phase = 0.0;
        self.wet_now = -1.0;
        self.fade = 0.0;
    }

    /// The LFO's next value, from -1 to 1 (`FUN_180017ec0`).
    fn lfo(&mut self) -> f32 {
        let before = self.phase;
        self.phase = frac(self.phase + self.step);
        match self.shape {
            0 => (before * TAU).sin() as f32,
            1 => (1.0 - (self.phase + self.phase)) as f32,
            _ => {
                if before > 0.5 {
                    -1.0
                } else {
                    1.0
                }
            }
        }
    }

    /// The LFO's value for a block that reads it once, with the phase moved
    /// on over the rest of the block.
    fn block_lfo(&mut self, frames: usize) -> f32 {
        let value = self.lfo();
        let end = (frames as f64 - 1.0) * self.step + self.phase;
        self.phase = frac(end);
        value
    }

    /// X and Y moved by the LFO, each inside 0..1.
    fn moved(&self, lfo: f32) -> (f32, f32) {
        (
            unit(lfo * self.mod_x + self.x),
            unit(lfo * self.mod_y + self.y),
        )
    }

    /// `FUN_180018c20`.
    fn run(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        tempo: f32,
        scratch: &mut [Vec<f32>; 2],
    ) {
        if !(self.hz >= 1.0 && tempo >= 1.0) {
            return;
        }
        if self.wet_now < 0.0 {
            self.wet_now = self.wet;
        }
        self.seconds = (self.beats * 60.0) / tempo;
        self.step = if self.seconds == 0.0 {
            0.0
        } else {
            f64::from(1.0 / (self.seconds * self.hz))
        };
        match self.kind {
            DISTORTION => self.distortion(left, right),
            LOFI => self.lofi(left, right),
            FLANGER => self.flanger(left, right),
            REVERB => self.reverb(left, right, scratch),
            GATE => self.gate(left, right),
            VOWEL => self.vowel(left, right),
            _ => {}
        }
    }

    /// Move the wet mix and the fade on by a frame, and give both.
    fn smooth(&mut self) -> (f32, f32) {
        self.wet_now = (self.wet - self.wet_now) * STEP + self.wet_now;
        let on = if self.bypassed { 0.0 } else { 1.0 };
        self.fade = (on - self.fade) * STEP + self.fade;
        (self.wet_now, self.fade)
    }

    /// After a block: a bypassed effect that has faded out starts over.
    fn settle(&mut self) {
        if self.bypassed && self.fade < SILENT {
            self.set_rate(0.0);
        }
    }

    /// Effect 0 (`FUN_180019860`): a low-pass at Y splits the input; the
    /// band under it goes through four soft clippers in a row, made up to
    /// unity at 0.5, and the band over it passes. X is the drive. The LFO
    /// moves neither.
    fn distortion(&mut self, left: &mut [f32], right: &mut [f32]) {
        let Body::Distortion { low } = &mut self.body else {
            return;
        };
        if self.bypassed {
            return;
        }
        let drive = ((f64::from(self.x) * 0.95 + f64::from(0.05f32)) as f32).max(0.0);
        let drive = drive.min(1.0);
        let t = self.y * 0.9 + 0.1;
        let tone = (t * t) * (t * t);
        let k = drive * drive * 2048.0;
        let shape = |x: f32| -> f32 {
            let a = x / (x.abs() * k + 1.0);
            let a = a / (a * a * k + 1.0);
            let a = a / ((a * a * a).abs() * k + 1.0);
            a / (a * k * a * (a * a) + 1.0)
        };
        let half = 0.5 / (k * 0.5 + 1.0);
        let half = half / (half * half * k + 1.0);
        let half = half / ((half * half * half).abs() * k + 1.0);
        let gain = 0.5 / (half / (half * k * half * (half * half) + 1.0));
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            self.wet_now = (self.wet - self.wet_now) * STEP + self.wet_now;
            let w = self.wet_now;
            let xl = *l;
            low[0] = (xl - low[0]) * tone + low[0];
            let outl = shape(low[0]) * gain + (xl - low[0]);
            let xr = *r;
            low[1] = (xr - low[1]) * tone + low[1];
            let outr = shape(low[1]) * gain + (xr - low[1]);
            *l = (1.0 - w) * xl + w * outl;
            *r = (1.0 - w) * xr + w * outr;
        }
    }

    /// Effect 1 (`FUN_180019b60`): sample and hold for `128 X^3 + 1`
    /// frames, and steps of `(2048 Y^2 + 1) / 65536` toward 0.
    fn lofi(&mut self, left: &mut [f32], right: &mut [f32]) {
        if self.bypassed {
            return;
        }
        let lfo = self.block_lfo(left.len());
        let (x, y) = self.moved(lfo);
        let Body::LoFi { count, held } = &mut self.body else {
            return;
        };
        let steps = 65_536.0 / (y * y * 2048.0 + 1.0);
        let hold = x * x * x * 128.0 + 1.0;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let mut c = *count;
            if c <= 0.0 {
                c += hold;
                held[1] = ((steps * *r) as i32) as f32 * (1.0 / steps);
                held[0] = ((steps * *l) as i32) as f32 * (1.0 / steps);
            }
            *count = c - 1.0;
            self.wet_now = (self.wet - self.wet_now) * STEP + self.wet_now;
            let w = self.wet_now;
            *l = (1.0 - w) * *l + w * held[0];
            *r = (1.0 - w) * *r + w * held[1];
        }
    }

    /// Effect 2 (`FUN_180019d60`): the inverted input through a 5 ms tape
    /// delay with feedback, added to the input. X^2 is the delay, Y the
    /// feedback up to -0.96, and the LFO moves both every frame.
    fn flanger(&mut self, left: &mut [f32], right: &mut [f32]) {
        if self.bypassed && self.fade == 0.0 {
            return;
        }
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let (w, fade) = self.smooth();
            let lfo = self.lfo();
            let time = unit(self.x * self.x + lfo * self.mod_x);
            let feedback = unit(lfo * self.mod_y + self.y) * -0.96;
            let Body::Flanger(tapes) = &mut self.body else {
                return;
            };
            for tape in tapes.iter_mut() {
                tape.set_time(f64::from(time));
                tape.feedback = feedback;
            }
            let back_l = tapes[0].run(-(*l * fade));
            let back_r = tapes[1].run(-(*r * fade));
            *l += back_l * w;
            *r += back_r * w;
        }
        self.settle();
    }

    /// Effect 6 (`FUN_18001a130`): the plate reverb added to the input. X
    /// is the decay, `0.75 X^3` of 20 s, and the level; Y^2 the damping.
    fn reverb(&mut self, left: &mut [f32], right: &mut [f32], scratch: &mut [Vec<f32>; 2]) {
        if self.bypassed && self.fade == 0.0 {
            return;
        }
        let frames = left.len();
        let lfo = self.block_lfo(frames);
        let (x, y) = self.moved(lfo);
        let [wet_l, wet_r] = scratch;
        for n in 0..frames {
            let (w, fade) = self.smooth();
            wet_r[n] = fade * w * right[n];
            wet_l[n] = fade * w * left[n];
        }
        let Body::Reverb(plate) = &mut self.body else {
            return;
        };
        let decay = x * x * (x * 0.75);
        plate.set(DAMPING, y * y);
        plate.set(DECAY, decay);
        plate.set(REVERB_DB, decay * 0.3 + 0.5);
        plate.set(HIGH_CUT, 1.0);
        plate.set(LOW_CUT, 0.1);
        plate.set(DIFFUSION, 1.0);
        plate.set(ROOM, 1.0);
        plate.set(PREDELAY, 0.0);
        plate.set(DRY_DB, 0.0);
        plate.set(MUTE_ER, 1.0);
        plate.run([&wet_l[..frames], &wet_r[..frames]], [left, right]);
        self.settle();
    }

    /// Effect 8 (`FUN_180019f90`): a gain that ducks once a cycle. It
    /// opens over the first 1 % of the cycle and closes along
    /// `1 - k t^4` with `k = 50 (1 - X)^2 + 1`, down to `Y^2`. Neither the
    /// LFO's modulation nor the wet mix touches it.
    fn gate(&mut self, left: &mut [f32], right: &mut [f32]) {
        if self.bypassed || self.beats == 0.0 {
            return;
        }
        let Body::Gate { duck } = &mut self.body else {
            return;
        };
        let (x, y) = (unit(self.x), unit(self.y));
        let depth = 1.0 - y * y;
        let k = (1.0 - x) * (1.0 - x) * 50.0 + 1.0;
        let start = f64::from(0.01f32);
        let span = f64::from_bits(0x3ff0_295f_aaa3_fae2);
        let rise = f64::from_bits(0x4059_0000_0960_0004);
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let phase = self.phase;
            self.wet_now = (self.wet - self.wet_now) * STEP + self.wet_now;
            let open = if start <= phase {
                let t = ((phase - start) * span) as f32;
                let t = t * t;
                1.0 - (t * t * k).min(1.0)
            } else {
                (phase * rise) as f32
            };
            self.phase = frac(phase + self.step);
            *duck = (open - *duck) * STEP + *duck;
            let gain = 1.0 - *duck * depth;
            *l *= gain;
            *r *= gain;
        }
    }

    /// Effect 10 (`FUN_18001b140`): eight band-passes of `q` 12 a side,
    /// summed. X slides through three vowels, Y scales every formant by
    /// `2^(1.86 Y - 1)`.
    fn vowel(&mut self, left: &mut [f32], right: &mut [f32]) {
        if self.bypassed && self.fade == 0.0 {
            return;
        }
        let lfo = self.block_lfo(left.len());
        let (x, y) = self.moved(lfo);
        let y = y * 0.93;
        let scale = 2f32.powf(((y - 0.5) + y) - 0.5);
        let at = x * 3.0;
        let mut index = at as i32;
        let frac = at - index as f32;
        // The weights of the three tables, A, U and I.
        let (a, u, i) = match index {
            0 => (frac, 1.0 - frac, 0.0),
            1 => (1.0 - frac, 0.0, frac),
            2 => (0.0, frac, 1.0 - frac),
            _ => {
                index = 0;
                (frac, 1.0 - frac, 0.0)
            }
        };
        let gain = 1.0 - index as f32 * 0.125;
        let freqs: [f32; 8] =
            std::array::from_fn(|k| ((a * VOWEL_A[k] + u * VOWEL_U[k]) + i * VOWEL_I[k]) * scale);
        let hz = self.hz;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let (w, fade) = self.smooth();
            let mix = fade * w;
            let Body::Vowel(bands) = &mut self.body else {
                return;
            };
            let (mut sum_l, mut sum_r) = (0.0, 0.0);
            for (k, freq) in freqs.iter().enumerate() {
                bands[0][k].tune(*freq, 12.0, hz);
                bands[1][k].tune(*freq, 12.0, hz);
                sum_l += bands[0][k].run(*l) * gain;
                sum_r += bands[1][k].run(*r) * gain;
            }
            *l = sum_l * mix + (1.0 - mix) * *l;
            *r = (1.0 - mix) * *r + sum_r * mix;
        }
        self.settle();
    }
}

/// The vowel filter's formants in Hz: three vowels, each four of its own
/// and four it shares (`0x1800586a0`).
const VOWEL_A: [f32; 8] = [
    700.0, 1250.0, 2500.0, 3500.0, 4200.0, 8500.0, 10_000.0, 12_000.0,
];
const VOWEL_U: [f32; 8] = [
    250.0, 700.0, 2500.0, 3500.0, 4200.0, 8500.0, 10_000.0, 12_000.0,
];
const VOWEL_I: [f32; 8] = [
    250.0, 2500.0, 2700.0, 3500.0, 4200.0, 8500.0, 10_000.0, 12_000.0,
];

/// The plugin: the FX framework's shell (`FUN_180017a00`) around the
/// effects' host (`FUN_18001b7f0`).
#[derive(Debug, Clone, PartialEq)]
pub struct Effector {
    params: [f32; PARAMS],
    hz: f32,
    tempo: f32,
    /// How far the song has played, in beats, for the LFOs to start from
    /// when the Mod Rate moves.
    beats: f64,
    kind: usize,
    bypass: bool,
    rate: f32,
    reduce: f32,
    input: f32,
    input_now: [f32; 2],
    output: f32,
    output_now: f32,
    units: Vec<Unit>,
    scratch: [Vec<f32>; 2],
}

impl Default for Effector {
    fn default() -> Self {
        let mut plugin = Effector {
            params: DEFAULTS,
            hz: 44_100.0,
            tempo: 120.0,
            beats: 0.0,
            kind: 0,
            bypass: false,
            rate: 0.25,
            reduce: 0.0,
            input: 1.0,
            input_now: [-1.0; 2],
            output: 1.0,
            output_now: -1.0,
            units: (0..KINDS).map(Unit::new).collect(),
            scratch: [vec![0.0; CHUNK], vec![0.0; CHUNK]],
        };
        for (param, value) in DEFAULTS.iter().enumerate() {
            plugin.apply(param, *value);
        }
        plugin
    }
}

impl Effector {
    /// Which effect the Effect Kind wheel picks.
    pub fn kind(&self) -> usize {
        self.kind
    }

    /// One parameter from 0 to 1 (`FUN_180017600`).
    fn apply(&mut self, param: usize, value: f32) {
        let value = unit(value);
        let value = if value.is_nan() { 0.0 } else { value };
        self.params[param] = value;
        match param {
            KIND => {
                let kind = (value * 11.0 + 0.5).clamp(0.0, 11.0) as usize;
                self.kind = kind;
                self.bypass_units();
            }
            BYPASS => {
                self.bypass = value != 0.0;
                self.bypass_units();
            }
            WET => self.units.iter_mut().for_each(|u| u.wet = value),
            X => self.units.iter_mut().for_each(|u| u.x = value),
            Y => self.units.iter_mut().for_each(|u| u.y = value),
            RATE => {
                let step = ((value * 8.0 + 0.5) as i32).max(0) as usize;
                self.rate = BEATS[step.min(8)];
                self.align();
            }
            REDUCE => self.reduce = value,
            MOD_X | MOD_Y => {
                let v = value + value - 1.0;
                let amount = if v < 0.0 { -(v * v) } else { v * v };
                for u in &mut self.units {
                    if param == MOD_X {
                        u.mod_x = amount;
                    } else {
                        u.mod_y = amount;
                    }
                }
            }
            SHAPE => {
                let shape = ((value + value + 0.5) as i32).clamp(0, 2) as u8;
                self.units.iter_mut().for_each(|u| u.shape = shape);
            }
            INPUT => self.input = value * value,
            OUTPUT => self.output = 10f32.powf((value - 0.5) * 2.4),
            _ => {}
        }
    }

    /// Every effect but the one picked is bypassed, and that one too when
    /// FX Bypass is on (`FUN_18001bb30`).
    fn bypass_units(&mut self) {
        let (kind, bypass) = (self.kind, self.bypass);
        for (n, u) in self.units.iter_mut().enumerate() {
            u.bypassed = n != kind || bypass;
        }
    }

    /// `FUN_180016d40`: each LFO's phase put where the song is.
    fn align(&mut self) {
        let at = f64::from(self.beats as f32 * 4.0);
        for u in &mut self.units {
            if u.beats > 0.0 {
                u.phase = frac((0.25 / f64::from(u.beats)) * at);
            }
        }
    }

    fn run(&mut self, left: &mut [f32], right: &mut [f32]) {
        // Two smoothers with one target, each frame scaled by where they
        // were before it.
        let input = self.input;
        for state in &mut self.input_now {
            if *state < 0.0 {
                *state = input;
            }
        }
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let [a, b] = &mut self.input_now;
            *l *= *a;
            *r *= *b;
            *a = (input - *a) * STEP + *a;
            *b = (input - *b) * STEP + *b;
        }
        let reduce = 1.0 - self.reduce;
        let beats = (reduce * reduce * self.rate).max(1.0 / 512.0);
        for u in &mut self.units {
            u.beats = beats.max(0.0);
            u.run(left, right, self.tempo, &mut self.scratch);
        }
        if self.output_now < 0.0 {
            self.output_now = self.output;
        }
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            self.output_now = (self.output - self.output_now) * OUT_STEP + self.output_now;
            *l *= self.output_now;
            *r *= self.output_now;
        }
        let seconds = left.len() as f64 / f64::from(self.hz);
        self.beats += seconds * f64::from(self.tempo) / 60.0;
    }
}

impl Effect for Effector {
    /// FL reads the parameters one by one and stops where the bytes do.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let wrong = || StateError::Length {
            expected: LEN,
            found: bytes.len(),
        };
        if bytes.len() < 8 || &bytes[..4] != MAGIC {
            return Err(wrong());
        }
        let len = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        let chunk = bytes.get(8..8 + len).filter(|c| c.len() >= 4 + NAME_BYTES);
        let chunk = chunk.ok_or_else(wrong)?;
        let tag = u32::from_le_bytes(chunk[..4].try_into().unwrap());
        if tag != FX01 {
            return Err(StateError::Version { found: tag as i32 });
        }
        let words = chunk[4 + NAME_BYTES..].as_chunks::<4>().0;
        for (param, word) in words.iter().take(PARAMS).enumerate() {
            self.apply(param, f32::from_le_bytes(*word));
        }
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz >= 1.0 {
            self.hz = hz;
            for u in &mut self.units {
                u.set_rate(hz);
            }
            self.reset();
        }
    }

    fn set_tempo(&mut self, bpm: f64) {
        self.tempo = bpm as f32;
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        for (l, r) in left[..len]
            .chunks_mut(CHUNK)
            .zip(right[..len].chunks_mut(CHUNK))
        {
            self.run(l, r);
        }
    }

    fn param_count(&self) -> usize {
        PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        self.params
            .get(index)
            .map_or(0.0, |value| (value * STEPS).round())
    }

    /// As FL's wrapper does, a value the parameter already has changes
    /// nothing: the LFOs only resync when the Mod Rate moves.
    fn set_param(&mut self, index: usize, value: f32) {
        if index < PARAMS {
            let value = knob_value(value, (0, STEPS as i32)) as f32 / STEPS;
            if value != self.params[index] {
                self.apply(index, value);
            }
        }
    }

    /// Every effect empties its lines and starts its LFO and its fade
    /// again, as when FL is given a rate.
    fn reset(&mut self) {
        for u in &mut self.units {
            u.set_rate(0.0);
        }
        self.input_now = [-1.0; 2];
        self.output_now = -1.0;
        self.beats = 0.0;
    }

    fn fidelity(&self) -> Fidelity {
        let built = [DISTORTION, LOFI, FLANGER, REVERB, GATE, VOWEL];
        if self.bypass || built.contains(&self.kind) {
            Fidelity::Exact
        } else {
            Fidelity::Approximate("Effector's effects 3, 4, 5, 7, 9 and 11 pass the input through")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{corpus, every_preset, projects, response, sine};

    /// A state as FL saves it, with these parameters.
    fn bytes(params: &[f32]) -> Vec<u8> {
        let mut chunk = FX01.to_le_bytes().to_vec();
        chunk.extend([0; NAME_BYTES]);
        chunk.extend(params.iter().flat_map(|v| v.to_le_bytes()));
        let mut bytes = MAGIC.to_vec();
        bytes.extend((chunk.len() as u32).to_le_bytes());
        bytes.extend(chunk);
        bytes
    }

    /// The plugin at 44.1 kHz and 120 BPM with effect `kind` on and these
    /// parameters over the defaults.
    fn with(kind: usize, params: &[(usize, f32)]) -> Effector {
        let mut values = DEFAULTS;
        values[KIND] = kind as f32 / 11.0;
        values[BYPASS] = 0.0;
        for (param, value) in params {
            values[*param] = *value;
        }
        let mut plugin = Effector::default();
        plugin.set_sample_rate(44_100.0);
        plugin.set_tempo(120.0);
        plugin.set_state(&bytes(&values)).unwrap();
        assert_eq!(plugin.kind(), kind);
        plugin
    }

    fn run(plugin: &mut Effector, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.chunks_mut(256).zip(right.chunks_mut(256)) {
            plugin.process(l, r);
        }
    }

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0, |m, v| m.max(v.abs()))
    }

    fn rms(samples: &[f32]) -> f64 {
        let sum: f64 = samples.iter().map(|v| f64::from(*v).powi(2)).sum();
        (sum / samples.len() as f64).sqrt()
    }

    #[test]
    fn a_new_effector_is_bypassed_and_passes_its_input() {
        let mut plugin = Effector::default();
        assert_eq!(plugin.param_count(), 12);
        assert_eq!(plugin.kind(), 11);
        assert_eq!(plugin.param(BYPASS), 256.0);
        assert_eq!(plugin.param(KIND), 244.0);
        assert_eq!(plugin.fidelity(), Fidelity::Exact);
        let tone = sine(0.01, 4096);
        let (mut l, mut r) = (tone.clone(), tone.clone());
        plugin.process(&mut l, &mut r);
        assert_eq!(l, tone);
        assert_eq!(r, tone);
    }

    #[test]
    fn the_host_counts_a_parameter_in_256ths() {
        let mut plugin = Effector::default();
        plugin.set_param(KIND, 128.0);
        assert_eq!(plugin.kind(), 6);
        plugin.set_param(BYPASS, 0.0);
        assert_eq!(plugin.param(BYPASS), 0.0);
        plugin.set_param(OUTPUT, 1e9);
        assert_eq!(plugin.param(OUTPUT), 256.0);
        assert!((plugin.output - 10f32.powf(1.2)).abs() < 1e-4);
        plugin.set_param(KIND, 3.0 * 256.0 / 11.0);
        assert_eq!(plugin.kind(), 3);
        assert!(matches!(plugin.fidelity(), Fidelity::Approximate(_)));
    }

    #[test]
    fn a_state_that_is_not_fl_s_chunk_is_turned_down() {
        let mut plugin = Effector::default();
        let mut wrong = bytes(&DEFAULTS);
        wrong[8] = 0;
        assert!(matches!(
            plugin.set_state(&wrong),
            Err(StateError::Version { .. })
        ));
        assert!(plugin.set_state(&bytes(&DEFAULTS)[..100]).is_err());
        assert!(plugin.set_state(b"SSLF").is_err());
        // A state with three parameters sets those three.
        plugin.set_state(&bytes(&[0.0, 0.0, 0.25])).unwrap();
        assert_eq!((plugin.kind(), plugin.bypass), (0, false));
        assert_eq!(plugin.param(WET), 64.0);
        assert_eq!(plugin.param(X), 128.0);
    }

    #[test]
    fn the_mod_rate_wheel_steps_through_nine_lengths_and_the_lfo_has_three_shapes() {
        let mut plugin = Effector::default();
        for (step, beats) in BEATS.iter().enumerate() {
            plugin.apply(RATE, step as f32 / 8.0);
            assert_eq!(plugin.rate, *beats);
        }
        let mut u = Unit::new(LOFI);
        u.step = 0.25;
        let sine: Vec<f32> = (0..4).map(|_| u.lfo()).collect();
        assert!((sine[0]).abs() < 1e-6 && (sine[1] - 1.0).abs() < 1e-6);
        assert!((sine[3] + 1.0).abs() < 1e-6);
        u.shape = 1;
        assert_eq!(u.lfo(), 0.5);
        u.shape = 2;
        assert_eq!([u.lfo(), u.lfo(), u.lfo()], [1.0, 1.0, -1.0]);
    }

    #[test]
    fn the_output_and_input_gains_ramp_to_their_settings() {
        let mut plugin = with(GATE, &[(OUTPUT, 1.0), (INPUT, 0.5), (Y, 1.0)]);
        let (mut l, mut r) = (vec![1.0; 4096], vec![1.0; 4096]);
        run(&mut plugin, &mut l, &mut r);
        // The gate at a Y of 1 does nothing; the rest is 0.25 * 10^1.2.
        let gain = 0.25 * 10f32.powf(1.2);
        assert!((l[4095] - gain).abs() < 1e-3 * gain, "{}", l[4095]);
        assert!((l[0] - gain).abs() < 1e-3 * gain);
    }

    #[test]
    fn the_distortion_is_made_up_to_unity_at_one_half() {
        // A tone wheel at 1 keeps the whole input in the clipped band.
        let mut plugin = with(DISTORTION, &[(X, 1.0), (Y, 1.0)]);
        let (mut l, mut r) = (vec![0.5; 512], vec![-0.5; 512]);
        run(&mut plugin, &mut l, &mut r);
        assert!((l[511] - 0.5).abs() < 1e-4, "{}", l[511]);
        assert!((r[511] + 0.5).abs() < 1e-4);
        // And clips: twice the input comes out at much less than twice.
        let (mut l, mut r) = (vec![1.0; 512], vec![0.05; 512]);
        run(&mut plugin, &mut l, &mut r);
        assert!(l[511] > 0.5 && l[511] < 0.6, "{}", l[511]);
        // A small signal gets the make-up gain.
        assert!(r[511] > 0.4, "{}", r[511]);
    }

    #[test]
    fn the_lofi_holds_a_sample_for_128_x_cubed_frames() {
        let mut plugin = with(LOFI, &[(X, 1.0), (Y, 0.0), (MOD_X, 0.5), (MOD_Y, 0.5)]);
        let ramp: Vec<f32> = (0..1024).map(|n| n as f32 / 2048.0).collect();
        let (mut l, mut r) = (ramp.clone(), ramp.clone());
        run(&mut plugin, &mut l, &mut r);
        // Holds of 129 frames, each the ramp where it started, to 1/65536.
        for n in 0..1024 {
            let held = ramp[n - n % 129];
            assert!((l[n] - held).abs() <= 1.0 / 65_536.0, "{n}");
        }
        // Y at 1 leaves steps of 2049/65536.
        let mut plugin = with(LOFI, &[(X, 0.0), (Y, 1.0), (MOD_X, 0.5), (MOD_Y, 0.5)]);
        let (mut l, mut r) = (vec![0.1; 64], vec![-0.1; 64]);
        run(&mut plugin, &mut l, &mut r);
        let step = 2049.0 / 65_536.0;
        assert!((l[63] - 3.0 * step).abs() < 1e-6, "{}", l[63]);
        assert!((r[63] + 3.0 * step).abs() < 1e-6);
    }

    #[test]
    fn the_tape_delays_by_its_time_between_2_samples_and_its_length() {
        for (t, delay) in [(0.0, 3), (0.5, 112), (1.0, 221)] {
            let mut tape = Tape::new(220);
            tape.set_time(t);
            tape.clear();
            let out: Vec<f32> = (0..400)
                .map(|n| tape.run(if n == 10 { 1.0 } else { 0.0 }))
                .collect();
            let at = (0..400).max_by(|a, b| out[*a].total_cmp(&out[*b])).unwrap();
            assert_eq!(at, 10 + delay, "{t}");
            assert!(out[at] > 0.99, "{t}: {}", out[at]);
        }
    }

    #[test]
    fn the_flanger_adds_the_inverted_delay() {
        let mut plugin = with(FLANGER, &[(X, 0.0), (Y, 0.0), (MOD_X, 0.5), (MOD_Y, 0.5)]);
        // Once it has faded in, a step comes out as itself and, two
        // samples on, cancels.
        let (mut l, mut r) = (vec![0.0; 4096], vec![0.0; 4096]);
        l[3000..].fill(1.0);
        run(&mut plugin, &mut l, &mut r);
        assert!((l[3001] - 1.0).abs() < 1e-3, "{}", l[3001]);
        assert!(l[3010].abs() < 1e-3, "{}", l[3010]);
        assert_eq!(peak(&r), 0.0);
    }

    #[test]
    fn a_vowel_band_peaks_at_12_on_its_formant() {
        let mut band = Band::default();
        band.tune(1000.0, 12.0, 44_100.0);
        let (b0, a1, a2) = (f64::from(band.b0), f64::from(band.a1), f64::from(band.a2));
        let gain = |hz: f64| response(&[b0, 0.0, -b0], &[1.0, -a1, a2], hz / 44_100.0);
        assert!((gain(1000.0) - 12.0).abs() < 0.01, "{}", gain(1000.0));
        assert!(gain(10_000.0) < 1.5, "{}", gain(10_000.0));
        assert!(gain(100.0) < 1.5);
    }

    #[test]
    fn the_vowel_filter_rings_at_its_formants() {
        let mut plugin = with(VOWEL, &[(X, 0.0), (Y, 0.5), (MOD_X, 0.5), (MOD_Y, 0.5)]);
        // X at 0 is the U table; Y at 0.5 scales it by 2^-0.07.
        let scale = 2f32.powf(0.93 - 1.0);
        let at = |hz: f32| f64::from(hz * scale) / 44_100.0;
        let tone = |turns: f64| -> f64 {
            let mut plugin = plugin.clone();
            let input = sine(turns, 44_100);
            let (mut l, mut r) = (input.clone(), input);
            run(&mut plugin, &mut l, &mut r);
            rms(&l[22_050..]) * 2f64.sqrt()
        };
        let formant = tone(at(250.0));
        let between = tone(at(450.0));
        assert!(formant > 8.0, "{formant}");
        assert!(between < formant / 4.0, "{between}");
        plugin.reset();
    }

    #[test]
    fn the_gate_ducks_once_a_cycle_down_to_y_squared() {
        // A beat a cycle at 120 BPM: 22050 frames. X at 0 closes the
        // curve fast, `1 - 51 t^4`, so the gate is open for most of it.
        let mut plugin = with(GATE, &[(X, 0.0), (Y, 0.5), (RATE, 5.0 / 8.0), (MOD_X, 0.5)]);
        let (mut l, mut r) = (vec![1.0; 88_200], vec![1.0; 88_200]);
        run(&mut plugin, &mut l, &mut r);
        let last = &l[66_150..];
        // Open at the end of the cycle, down near 0.25 early in it.
        assert!(peak(last) > 0.99, "{}", peak(last));
        let low = last.iter().fold(1.0f32, |m, v| m.min(*v));
        assert!((low - 0.25).abs() < 0.02, "{low}");
        // And the same a cycle on.
        for n in 44_100..66_150 {
            assert!((l[n] - l[n + 22_050]).abs() < 1e-3, "{n}");
        }
    }

    #[test]
    fn the_reverb_decays_at_its_setting() {
        let mut plate = Plate::default();
        plate.set(DECAY, 0.75 * 0.125);
        plate.tune();
        let seconds = plate.ints[DECAY] as f32 * 0.1;
        let lens = [5, 7, 9, 11].map(|n| plate.len[n]).iter().sum::<usize>() as f32;
        // Four delays a loop come to -60 dB in the decay time.
        let expected = 0.001f32.powf(lens / PLATE_HZ / seconds / 4.0);
        assert!((plate.decay - expected).abs() < 1e-5);
        // An impulse dies away and stays finite.
        let mut plugin = with(REVERB, &[(X, 0.5), (Y, 0.5), (MOD_X, 0.5), (MOD_Y, 0.5)]);
        let mut l = vec![0.0; 88_200];
        let mut r = vec![0.0; 88_200];
        l[8000] = 1.0;
        run(&mut plugin, &mut l, &mut r);
        let early = rms(&r[9000..20_000]);
        let late = rms(&r[70_000..88_200]);
        assert!(early > 1e-4, "{early}");
        assert!(late < early / 10.0, "{early} {late}");
        assert!(l.iter().chain(&r).all(|v| v.is_finite()));
    }

    #[test]
    fn every_state_in_fl_s_files_loads_and_plays() {
        let Some(root) = corpus() else {
            eprintln!("FL_RS_CORPUS unset, Effector's states skipped");
            return;
        };
        let projects: Vec<_> = projects(&root)
            .into_iter()
            .filter(|s| s.plugin.eq_ignore_ascii_case(NAME))
            .collect();
        let presets = every_preset(&root, NAME);
        assert_eq!((projects.len(), presets.len()), (27, 3));
        let mut kinds = [0; KINDS];
        for saved in projects.iter().chain(&presets) {
            let mut plugin = Effector::default();
            plugin.set_sample_rate(48_000.0);
            plugin.set_tempo(128.0);
            plugin.set_state(&saved.state).unwrap();
            assert_eq!(plugin.fidelity(), Fidelity::Exact, "{}", saved.file);
            if !plugin.bypass {
                kinds[plugin.kind()] += 1;
            }
            let (mut l, mut r) = (sine(0.01, 96_000), sine(0.0037, 96_000));
            for x in l.iter_mut().chain(r.iter_mut()) {
                *x *= 0.5;
            }
            run(&mut plugin, &mut l, &mut r);
            let sound = l.iter().chain(&r);
            assert!(sound.clone().all(|v| v.is_finite()), "{}", saved.file);
            let loud = peak(&l).max(peak(&r));
            assert!(loud > 0.02 && loud < 4.0, "{}: {loud}", saved.file);
        }
        // Reverb, gate, vowel, lo-fi, distortion, flanger; the presets are
        // bypassed.
        let mut expected = [0; KINDS];
        for (kind, n) in [(6, 11), (8, 8), (10, 4), (1, 2), (0, 1), (2, 1)] {
            expected[kind] = n;
        }
        assert_eq!(kinds, expected);
    }
}
