//! Sakura: FL's physical model of strings, as `docs/plugins/sakura.md`
//! reads it out of `Sakura_x64.dll` and its `engine_x64.dll`.
//!
//! The shell is a Delphi panel; the engine is an MSVC `CSakuraSynth` in
//! FL's `CFLWrapper` frame, as BassDrum's is, and holds the state, the
//! voices and the sound.
//!
//! # The state
//!
//! `SSLF`, the length of the engine's chunk (0x24c), then `1rkS`, 114 f32
//! parameters in the engine's order and the program's name in 128 bytes.
//! Most parameters are knobs from 0 to 1; the eight slots of the mod
//! matrix keep their source and target as whole numbers, and parameter
//! 111 (the strings' length ratio) is a whole number too.
//!
//! # A voice
//!
//! The engine runs at 48 kHz and reads its sum out at the host's rate in
//! straight lines, 48 frames at a time. A note takes a free voice of 99,
//! or the oldest. It works out its two strings' lengths from the key:
//! `n = int(48000 / f)` for `f = 220 * 2^((key - 57) / 12)` times the
//! octave knob, split between the strings by the ratio. Every sample of
//! the engine:
//!
//! 1. The exciter: a noise sample held while the noise does not pass the
//!    noise rate (or a constant, with Exc Noise Level up), a resonant
//!    biquad highpass (Lo-Cut), and a four-pole ladder whose cutoff an
//!    attack-decay envelope opens (Hi-Cut), with the last stage saturated
//!    by `x / (drive x^2 + 1)`.
//! 2. The strings: four lines, two to a string, clocked at
//!    `f * length / 48000` steps a sample, so that a turn of the line is a
//!    period. Each step every line is fed the exciter less a one-pole
//!    follower of it, and gives back what it held through a two-pole
//!    lowpass (Damp) with a sharpness feedback and the saturation
//!    `x / (sat x^2 + 1)`, times the decay. The first line of each string
//!    goes out through its comb, the second feeds it.
//! 3. The output is the string pair read between two steps, panned by
//!    Stereo Sep and Str Mix, times the amplitude envelope.
//!
//! The mod matrix's eight slots add a source (envelope, LFO, mod wheel,
//! pitch wheel, aftertouch, velocity, key, release) times the square of
//! their amount to one of 15 targets, once a block.
//!
//! After the voices, on the host's rate: a highpass at 32 Hz, the eight
//! resonators, the chorus, the reverb, the delay and the master level
//! (`sakura_fx`).
//!
//! Run under an emulator, the engine DLL plays what this does to the bit
//! (`docs/plugins/sakura.md`). What is not FL's:
//!
//! - The engine's noise is one generator for every Sakura FL has loaded;
//!   here each has its own, starting where FL's first does.
//! - The runtime's `powf`, `expf`, `tanf` and `cosf` are taken as
//!   rounding once from f64, which their SSE2 builds do; the FMA3 builds a
//!   CPU with FMA runs were not read.
//! - The runtime's f64 `pow` (`0x180042ef0`) is Rust's, which parts from it
//!   in the last bit on about one argument in 700: a state whose pitch
//!   moves can drift from FL's by a step of the voice's f64 step.
//! - The voices' lines are as long as the lowest key needs with the state's
//!   octave and ratio; a parameter change that asks for longer lines than
//!   the state did gets the longest the voice has, and `fidelity` says so.
//! - The pitch wheel, mod wheel and aftertouch sources read 0: FL's piano
//!   roll sends none.

use crate::sakura_fx::{Chorus, Echo, HighPass, Plate, Resonators, powf};
use crate::sampler::circular;
use crate::{Fidelity, Generator, NoteOn, StateError};
use std::f64::consts::TAU;

/// The plugin's event 201 name.
pub const NAME: &str = "Sakura";

/// How long FL writes the state.
pub const STATE_LEN: usize = 596;

/// How many parameters the engine reads from a state.
pub const PARAMS: usize = 114;

/// The engine's own rate.
const INNER: f64 = 48_000.0;

/// How many voices the engine has (`0x180001230`).
const VOICES: usize = 99;

/// The most samples a line of the engine holds.
const LONGEST: usize = 48_000;

/// What the parameters start at (`0x18002f200`).
const DEFAULTS: [f32; PARAMS] = {
    let mut p = [0.0; PARAMS];
    let halves = [
        0, 1, 0x12, 0x13, 0x1d, 0x40, 0x44, 0x47, 0x48, 0x4b, 0x4c, 0x4d, 0x71,
    ];
    let mut i = 0;
    while i < halves.len() {
        p[halves[i]] = 0.5;
        i += 1;
    }
    let ones = [3, 4, 6, 0xb, 0x1f, 0x23, 0x24, 0x25, 0x50, 0x52, 0x54];
    let mut i = 0;
    while i < ones.len() {
        p[ones[i]] = 1.0;
        i += 1;
    }
    p[0x1b] = 0.2;
    p[0x43] = 0.2;
    p[0x41] = 0.6;
    p[0x42] = 0.24;
    p[0x45] = 0.35;
    p[0x49] = 0.4;
    p[0x4f] = 6.0;
    p[0x53] = 4.0;
    p[0x56] = 3.0;
    p[0x6f] = 3.0;
    p
};

/// Pi as the LFO takes it (`0x180077ec8`).
#[allow(clippy::approx_constant)]
const LFO_PI: f64 = 3.141_592_653;

/// The first of the mod matrix's 32 parameters: source, amount, invert
/// and target for each of eight slots.
const MATRIX: usize = 0x4f;

/// The targets the engine asks the matrix for.
mod target {
    pub const LFO: i32 = 0;
    pub const LEVEL: i32 = 1;
    pub const SEPARATION: i32 = 2;
    pub const PITCH: i32 = 3;
    pub const LO_CUT: i32 = 4;
    pub const HI_CUT: i32 = 5;
    pub const HI_CUT_ATTACK: i32 = 6;
    pub const HI_CUT_DECAY: i32 = 7;
    pub const DECAY_1: i32 = 8;
    pub const DECAY_2: i32 = 9;
    pub const DECAY_BOTH: i32 = 10;
    pub const DAMP_1: i32 = 11;
    pub const DAMP_2: i32 = 12;
    pub const DAMP_BOTH: i32 = 13;
    pub const NOISE_RATE: i32 = 14;
}

/// `x^5` as the engine works it out: `(x x)(x x) x`.
fn fifth(x: f32) -> f32 {
    let square = x * x;
    square * square * x
}

/// The engine's value of each parameter (`0x18002f4c0`), worked out from
/// the 0 to 1 the state keeps.
#[derive(Debug, Clone, Default)]
struct Knobs {
    level: f32,
    tune: f32,
    amp_attack: f32,
    amp_decay: f32,
    amp_sustain: f32,
    amp_release: f32,
    decay: [f32; 2],
    decay_invert: [bool; 2],
    damp_offset: [f32; 2],
    damp: [f32; 2],
    sharpness: [f32; 2],
    saturation: f32,
    mix: f32,
    separation: f32,
    comb_ratio: f32,
    comb_feedback: f32,
    comb_feedback_invert: bool,
    comb_level: f32,
    comb_level_invert: bool,
    lo_cut: f32,
    lo_res: f32,
    hi_attack: f32,
    hi_decay: f32,
    hi_sustain: f32,
    hi_cut: f32,
    hi_res: f32,
    noise_level: f32,
    noise_rate: f32,
    drive: f32,
    eg_attack: f32,
    eg_decay: f32,
    eg_sustain: f32,
    eg_release: f32,
    lfo_rate: f32,
    ratio: i32,
    octave: f32,
}

/// The engine's envelope (`0x180018910`, `0x1800189b0`): straight lines,
/// worked out once a block.
#[derive(Debug, Clone, Copy)]
struct Envelope {
    attack: f64,
    decay: f64,
    release: f64,
    sustain: f64,
    from: f64,
    stage: u8,
    along: f64,
    value: f64,
    release_from: f64,
    idle: bool,
}

impl Default for Envelope {
    fn default() -> Self {
        Envelope {
            attack: 0.0,
            decay: 0.0,
            release: 0.0,
            sustain: 0.0,
            from: 0.0,
            stage: 0,
            along: 0.0,
            value: 0.0,
            release_from: 0.0,
            idle: true,
        }
    }
}

impl Envelope {
    fn start(&mut self, attack: f64, decay: f64, sustain: f64) {
        self.from = if self.idle { 0.0 } else { self.value };
        self.sustain = sustain;
        self.idle = false;
        self.stage = 0;
        self.along = 0.0;
        self.attack = (1.0 / INNER) / attack.max(0.001);
        self.decay = (1.0 / INNER) / decay.max(0.001);
        self.release = 1.0 / (0.001 * INNER);
    }

    fn let_go(&mut self, release: f64) {
        if self.stage != 2 {
            self.release = 1.0 / (release.max(0.001) * INNER);
            self.release_from = self.value;
            self.stage = 2;
            self.along = 0.0;
        }
    }

    fn tick(&mut self, frames: usize) -> f32 {
        let frames = frames as f64;
        self.value = 0.0;
        let value = match self.stage {
            0 => {
                self.along += frames * self.attack;
                if self.along >= 1.0 {
                    self.stage = 1;
                    self.along = 0.0;
                    1.0
                } else {
                    (1.0 - self.from) * self.along + self.from
                }
            }
            1 => {
                self.along += frames * self.decay;
                if self.along >= 1.0 {
                    if self.sustain == 0.0 {
                        self.stage = 2;
                        self.idle = true;
                    }
                    self.along = 1.0;
                    self.sustain
                } else {
                    (self.sustain - 1.0) * self.along + 1.0
                }
            }
            _ => {
                self.along += frames * self.release;
                if self.along >= 1.0 {
                    self.idle = true;
                    self.along = 1.0;
                    0.0
                } else {
                    (0.0 - self.release_from) * self.along + self.release_from
                }
            }
        };
        self.value = value.max(0.0);
        self.value as f32
    }
}

/// A line of a voice: the samples, how many of them are used, and where
/// it is.
#[derive(Debug, Clone, Default)]
struct Ring {
    samples: Vec<f32>,
    len: usize,
    at: usize,
}

impl Ring {
    fn with(capacity: usize) -> Self {
        Ring {
            samples: vec![0.0; capacity.max(1)],
            len: 1,
            at: 0,
        }
    }

    /// The place to read and write now, going back to the start at the end.
    fn here(&mut self) -> usize {
        if self.at >= self.len {
            self.at = 0;
        }
        self.at
    }
}

/// One of the 99 voices (`CSakuraKey`, 0x300 bytes).
#[derive(Debug, Clone, Default)]
struct Voice {
    active: bool,
    /// Whether its lines have been written since they were cleared.
    played: bool,
    first: bool,
    age: u32,
    key: i32,
    velocity: f32,
    release: f32,
    envelope: Envelope,
    eg: f32,
    lfo: f32,
    frequency: f64,
    scale: f64,
    step: f64,
    along: f64,
    prev: [f32; 2],
    cur: [f32; 2],
    time: f64,
    released_at: f64,
    amp_rate: f32,
    amp_target: f32,
    amp: f32,
    noise_gate: f32,
    drive: f32,
    held: f32,
    noise_scale: f32,
    noise_offset: f32,
    hi_rate: f32,
    hi_target: f32,
    hi_env: f32,
    hi_cut: f32,
    hi_res: f32,
    ladder: [f32; 3],
    out: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
    b0: f32,
    a1: f32,
    a2: f32,
    saturation: f32,
    low: [f32; 4],
    y: [f32; 4],
    follow: [f32; 4],
    damp: [f32; 4],
    sharp: [f32; 4],
    gain_target: [f32; 4],
    gain: [f32; 4],
    mix_target: [f32; 4],
    mix: [f32; 4],
    follow_rate: [f32; 2],
    comb_level: f32,
    comb_feedback: f32,
    silent: i32,
    lines: [Ring; 4],
    combs: [Ring; 2],
}

impl Voice {
    fn with(capacity: [usize; 2]) -> Self {
        let mut voice = Voice {
            lines: [
                Ring::with(capacity[0]),
                Ring::with(capacity[0]),
                Ring::with(capacity[1]),
                Ring::with(capacity[1]),
            ],
            combs: [Ring::with(capacity[0]), Ring::with(capacity[1])],
            ..Voice::default()
        };
        voice.kill();
        voice
    }

    /// `0x18002d870`: silent, its lines cleared, its filters at rest.
    fn kill(&mut self) {
        self.active = false;
        self.first = false;
        self.envelope.idle = true;
        self.along = 0.0;
        self.prev = [0.0; 2];
        self.cur = [0.0; 2];
        self.hi_cut = 0.0;
        self.hi_res = 0.0;
        self.ladder = [0.0; 3];
        self.out = 0.0;
        self.hi_target = 0.0;
        self.hi_env = 0.0;
        self.hi_rate = 0.0;
        self.amp_target = 0.0;
        self.amp = 0.0;
        self.amp_rate = 0.0;
        (self.x2, self.x1, self.y2, self.y1) = (0.0, 0.0, 0.0, 0.0);
        (self.b0, self.a1, self.a2) = (0.0, 0.0, 0.0);
        for ring in self.lines.iter_mut().chain(self.combs.iter_mut()) {
            if self.played {
                ring.samples.fill(0.0);
            }
            ring.at = 0;
        }
        self.played = false;
        self.low = [0.0; 4];
        self.y = [0.0; 4];
        self.damp = [0.0; 4];
        self.sharp = [0.0; 4];
        self.gain_target = [0.0; 4];
        self.mix_target = [0.0; 4];
        self.gain = [0.0; 4];
        self.mix = [0.0; 4];
        self.follow = [0.0; 4];
        self.drive = 0.0;
        self.silent = -1;
        self.step = 0.0;
        self.eg = 0.0;
        self.lfo = 0.0;
    }

    /// `0x18002d830`: the envelope to its release, the release source to 1
    /// and the amplitude's release timed from now.
    fn let_go(&mut self, knobs: &Knobs) {
        self.release = 1.0;
        self.envelope.let_go(f64::from(knobs.eg_release));
        self.released_at = 0.0;
    }
}

/// The strings' lengths for a key (`0x18002d510`): the frequency, then the
/// first and second string's.
fn lengths(key: i32, knobs: &Knobs) -> (f64, usize, usize) {
    let mut frequency = 2f64.powf((f64::from(key as f32) - 57.0) * (1.0 / 12.0)) * 220.0;
    match (knobs.octave * 6.0) as i32 {
        6 => frequency *= 8.0,
        5 => frequency *= 4.0,
        4 => frequency += frequency,
        2 => frequency *= 0.5,
        1 => frequency *= 0.25,
        0 => frequency *= 0.125,
        _ => {}
    }
    let n = (INNER / frequency) as i32;
    let (first, base) = match knobs.ratio {
        0 => (n / 4 * 4, n / 4),
        1 => (n / 3 * 3, n / 3),
        2 => (n / 2 * 2, n / 2),
        4 => (n, n * 2),
        5 => (n, n * 3),
        _ => (n, n),
    };
    let second = if knobs.ratio == 6 {
        n.wrapping_mul(4)
    } else {
        base
    };
    let first = first.clamp(1, LONGEST as i32) as usize;
    let second = second.clamp(1, LONGEST as i32) as usize;
    (frequency, first, second)
}

/// Sakura with its state.
pub struct Sakura {
    params: [f32; PARAMS],
    name: String,
    knobs: Knobs,
    rate: f64,
    voices: Vec<Voice>,
    /// The longest each string's lines can be.
    capacity: [usize; 2],
    /// A note asked for longer lines than the voices have.
    short: bool,
    order: Vec<usize>,
    noise: u32,
    counter: u32,
    lfo_phase: f32,
    lfo_value: f32,
    /// The pan gains and pitch offset of the last note on each key.
    key_left: [f32; 128],
    key_right: [f32; 128],
    key_pitch: [f32; 128],
    /// The host's number of the note that holds each key.
    owner: [Option<u32>; 128],
    /// The engine's samples of a block, the last of the block before at 0.
    inner: [Vec<f32>; 2],
    position: f64,
    count: usize,
    high: [HighPass; 2],
    resonators: Resonators,
    chorus: Chorus,
    plate: Plate,
    echo: Echo,
}

impl Default for Sakura {
    fn default() -> Self {
        let mut sakura = Sakura {
            params: DEFAULTS,
            name: "<noname>".to_owned(),
            knobs: Knobs::default(),
            rate: 44_100.0,
            voices: Vec::new(),
            capacity: [0; 2],
            short: false,
            order: Vec::with_capacity(VOICES),
            noise: 0,
            counter: 0,
            lfo_phase: 0.0,
            lfo_value: 1.0,
            key_left: [0.0; 128],
            key_right: [0.0; 128],
            key_pitch: [0.0; 128],
            owner: [None; 128],
            inner: [Vec::new(), Vec::new()],
            position: 0.0,
            count: 0,
            high: [HighPass::default(); 2],
            resonators: Resonators::default(),
            chorus: Chorus::default(),
            plate: Plate::default(),
            echo: Echo::default(),
        };
        sakura.echo.tempo = 120.0;
        for (n, value) in DEFAULTS.iter().enumerate() {
            sakura.apply(n, *value);
        }
        sakura.set_sample_rate(44_100.0);
        sakura.size_voices();
        sakura
    }
}

impl Sakura {
    /// The program's name the state holds.
    pub fn program(&self) -> &str {
        &self.name
    }

    /// What `0x18002f4c0` does with a parameter.
    fn apply(&mut self, n: usize, v: f32) {
        self.params[n] = v;
        let k = &mut self.knobs;
        match n {
            0 => k.level = powf(10.0, v * 4.8 - f32::from_bits(0x3fe6_6667)),
            1 => {
                let cents = ((v * 198.0 - 99.0) as i32).clamp(-99, 99);
                k.tune = (f64::from(cents) * 0.01) as f32;
            }
            2 => k.amp_attack = (f64::from(fifth(v) * 35.0) + 0.001) as f32,
            3 => k.amp_decay = (f64::from(fifth(v) * 50.0) + 0.001) as f32,
            4 => k.amp_sustain = v * v,
            5 => k.amp_release = (f64::from(fifth(v) * 10.0) + 0.1) as f32,
            6 => k.decay[0] = v,
            7 => k.decay_invert[0] = v != 0.0,
            8 => k.damp_offset[0] = v * v,
            9 => k.damp[0] = v,
            10 => k.sharpness[0] = v * v,
            11 => k.decay[1] = v,
            12 => k.decay_invert[1] = v != 0.0,
            13 => k.damp_offset[1] = v * v,
            14 => k.damp[1] = v,
            15 => k.sharpness[1] = v * v,
            16 => k.saturation = v * v,
            17 => k.mix = v,
            18 => k.separation = v,
            19 => k.comb_ratio = v,
            20 => k.comb_feedback = v.sqrt(),
            21 => k.comb_feedback_invert = v != 0.0,
            22 => k.comb_level = v,
            23 => k.comb_level_invert = v != 0.0,
            24 => k.lo_cut = v,
            25 => k.lo_res = v,
            26 => k.hi_attack = v,
            27 => k.hi_decay = v,
            28 => k.hi_sustain = v,
            29 => k.hi_cut = v,
            30 => k.hi_res = v,
            31 => k.noise_level = 1.0 - v,
            32 => k.noise_rate = v,
            33 => k.drive = v * 1000.0 * v * (v * v),
            34 => k.eg_attack = (f64::from(v * v * v * 10.0) + 0.001) as f32,
            35 => k.eg_decay = (f64::from(fifth(v) * 50.0) + 0.001) as f32,
            36 => k.eg_sustain = v * v,
            37 => k.eg_release = (f64::from(fifth(v) * 50.0) + 0.1) as f32,
            38 => k.lfo_rate = (f64::from(v * v * v * 10.0) + 0.01) as f32,
            39..=46 => self.resonators.position[n - 39] = 1.0 - powf(v, 0.1),
            47..=54 => {
                self.resonators.feedback[n - 47] = (f64::from(v.sqrt()) * 0.9999) as f32;
            }
            55..=62 => self.resonators.switch(n - 55, v != 0.0),
            63 => self.echo.on = v != 0.0,
            64 => self.echo.time = v,
            65 => self.echo.feedback = v,
            66 => self.echo.speed = v,
            67 => self.echo.depth = v,
            68 => self.echo.blur = v,
            69 => self.echo.wet = v,
            70 => self.chorus.on = v != 0.0,
            71 => self.chorus.set_rate(v),
            72 => self.chorus.set_depth(v),
            73 => self.chorus.set_wet(v),
            74 => self.plate.on = v != 0.0,
            75 => self.plate.set(12, v * 0.999 + 0.001),
            76 => self.plate.set(8, v * v * (v * 0.6)),
            77 => self.plate.set(2, v * v * v),
            111 => k.ratio = v as i32,
            112 => self.echo.sync = v,
            113 => k.octave = v,
            _ => {}
        }
    }

    /// The matrix's sum for a target (`0x18002e470`). The LFO's amount and
    /// the level are 1 where no slot is routed to them.
    fn matrix(&self, voice: &Voice, to: i32) -> f32 {
        let mut sum = 0.0f32;
        let mut routed = false;
        for slot in 0..8 {
            let p = &self.params[MATRIX + 4 * slot..MATRIX + 4 * slot + 4];
            let source = p[0] as i32;
            let mut amount = p[1] * p[1];
            if p[2] != 0.0 {
                amount = -amount;
            }
            if to != p[3] as i32 {
                continue;
            }
            if source != 0 {
                routed = true;
            }
            match source {
                1 => sum += amount * voice.eg,
                2 => sum += amount * voice.lfo,
                6 => sum += amount * voice.velocity,
                7 => {
                    sum =
                        (f64::from(voice.key) * 0.0125 * f64::from(amount) + f64::from(sum)) as f32
                }
                8 => sum += amount * voice.release,
                // The wheels and aftertouch, at rest.
                3..=5 => sum += amount * 0.0,
                _ => {}
            }
        }
        if (to == target::LFO || to == target::LEVEL) && !routed {
            1.0
        } else {
            sum
        }
    }

    /// The lines' lengths for the state: what the lowest key needs.
    fn size_voices(&mut self) {
        let (_, first, second) = lengths(0, &self.knobs);
        let capacity = [first, second];
        if capacity != self.capacity || self.voices.len() != VOICES {
            self.capacity = capacity;
            self.voices = (0..VOICES).map(|_| Voice::with(capacity)).collect();
        }
        self.short = false;
    }

    fn stop(&mut self) {
        for voice in &mut self.voices {
            voice.kill();
            voice.age = 0;
        }
        self.counter = 0;
        self.lfo_phase = 0.0;
        for side in &mut self.inner {
            side.fill(0.0);
        }
        self.position = 0.0;
        self.count = 0;
        self.owner = [None; 128];
    }

    fn clear_effects(&mut self) {
        for high in &mut self.high {
            high.clear();
        }
        self.chorus.clear();
        self.plate.clear();
        self.resonators.clear();
        self.echo.clear();
    }

    /// `0x18002c700`: the highpass, the resonators, the chorus, the reverb
    /// and the delay.
    fn effects(&mut self, left: &mut [f32], right: &mut [f32]) {
        let [high_left, high_right] = &mut self.high;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            *l = high_left.run(*l);
            *r = high_right.run(*r);
        }
        self.resonators.run(left, right);
        self.chorus.run(left, right);
        self.plate.run(left, right);
        self.echo.run(left, right);
    }

    /// `0x180031740`, `0x18002d510`.
    fn start(&mut self, key: i32, velocity: f32) {
        let free = self.voices.iter().position(|voice| !voice.active);
        let n = match free {
            Some(n) => n,
            None => {
                let mut oldest = 0;
                for (n, voice) in self.voices.iter().enumerate() {
                    if voice.age <= self.voices[oldest].age {
                        oldest = n;
                    }
                }
                self.voices[oldest].kill();
                oldest
            }
        };
        let (frequency, first, second) = lengths(key, &self.knobs);
        let capacity = self.capacity;
        if first > capacity[0] || second > capacity[1] {
            self.short = true;
        }
        let k = &self.knobs;
        let voice = &mut self.voices[n];
        voice.key = key;
        voice.velocity = velocity;
        voice.released_at = -1.0;
        voice.time = 0.0;
        voice.release = 0.0;
        voice.first = true;
        voice.frequency = frequency;
        let first = first.min(capacity[0]);
        let second = second.min(capacity[1]);
        voice.lines[0].len = first;
        voice.lines[1].len = first;
        voice.lines[2].len = second;
        voice.lines[3].len = second;
        let inverse = 1.0 / first as f64;
        voice.scale = inverse * INNER;
        voice.follow_rate = [inverse as f32, (1.0 / second as f64) as f32];
        let comb = |len: usize, cap: usize| {
            ((len as f32 * k.comb_ratio) as i32).max(1).min(cap as i32) as usize
        };
        voice.combs[0].len = comb(first, LONGEST).min(capacity[0]);
        voice.combs[1].len = comb(second, LONGEST).min(capacity[1]);
        voice.envelope.start(
            f64::from(k.eg_attack),
            f64::from(k.eg_decay),
            f64::from(k.eg_sustain),
        );
        (voice.x2, voice.x1, voice.y2, voice.y1) = (0.0, 0.0, 0.0, 0.0);
        voice.active = true;
        voice.played = true;
        voice.age = self.counter;
        self.counter = self.counter.wrapping_add(1);
    }

    /// `0x180031830`: every voice of the key lets go.
    fn let_go(&mut self, key: i32) {
        let knobs = &self.knobs;
        for voice in &mut self.voices {
            if voice.active && voice.key == key {
                voice.let_go(knobs);
            }
        }
    }

    /// `0x18002da50`: what a voice works out once a block of `frames`
    /// engine samples.
    fn prepare(&mut self, n: usize, frames: usize) {
        if !self.voices[n].active {
            return;
        }
        let mut voice = std::mem::take(&mut self.voices[n]);
        let k = &self.knobs;
        let key = voice.key.clamp(0, 127) as usize;
        let s = k.saturation;
        voice.saturation = s * s * s * 100.0;
        voice.step = voice.frequency / voice.scale;
        let bend = self.matrix(&voice, target::PITCH);
        let octaves = (self.key_pitch[key] + k.tune) * 0.083_333_336 + bend * 0.166_666_67;
        voice.step *= 2f64.powf(f64::from(octaves));
        voice.eg = voice.envelope.tick(frames);
        voice.lfo = self.matrix(&voice, target::LFO) * self.lfo_value;
        let rate = (self.matrix(&voice, target::NOISE_RATE) + k.noise_rate).clamp(0.0, 1.0);
        voice.noise_gate = (f64::from(1.0 - rate).powf(0.25) * 0.996) as f32;
        voice.drive = k.drive;
        voice.noise_scale = k.noise_level * k.noise_level;
        voice.noise_offset = 1.0 - k.noise_level;
        let attack = (self.matrix(&voice, target::HI_CUT_ATTACK) + k.hi_attack).clamp(0.0, 1.0);
        let attack = attack * attack * 0.5;
        let decay = (self.matrix(&voice, target::HI_CUT_DECAY) + k.hi_decay).clamp(0.0, 1.0);
        let decay = (f64::from(decay * decay * decay * 3.0) + 0.001) as f32;
        if voice.released_at > -1.0 {
            voice.hi_rate = f32::from_bits(0x3b03_126e);
            voice.hi_target = 0.0;
        } else if f64::from(attack) >= voice.time {
            voice.hi_rate = if attack >= 0.001 {
                0.0002 / attack
            } else {
                1.0
            };
            voice.hi_target = 1.0;
        } else {
            voice.hi_rate = (0.0002 / f64::from(decay)) as f32;
            voice.hi_target = k.hi_sustain * k.hi_sustain;
        }
        let cut = self.matrix(&voice, target::HI_CUT) * 0.5 + k.hi_cut;
        voice.hi_cut = (cut * cut).clamp(0.0, 1.0);
        voice.hi_res = ((1.0 - voice.hi_cut) * k.hi_res * -10.0).max(-4.0);
        let q = k.lo_res * 10.0 + 1.0;
        let cut = (self.matrix(&voice, target::LO_CUT) * 0.5 + k.lo_cut).clamp(0.0, 1.0);
        let w = 2f64.powf(f64::from(cut) * 9.3 + 5.0) * (TAU / INNER);
        let (sin, cos) = crate::sakura_fx::sin_cos_f64(w);
        let dq = f64::from(q);
        let inverse = 1.0 / (dq + sin);
        voice.b0 = ((cos + 1.0) / (sin / dq + 2.0)) as f32;
        voice.a1 = (f64::from(q * -2.0) * cos * inverse) as f32;
        voice.a2 = ((dq - sin) * inverse) as f32;
        let both = self.matrix(&voice, target::DAMP_BOTH);
        let one = [target::DAMP_1, target::DAMP_2];
        let decays = [target::DECAY_1, target::DECAY_2];
        let both_decay_target = target::DECAY_BOTH;
        for string in 0..2 {
            let sharp = k.sharpness[string] * -0.5;
            let x = (self.matrix(&voice, one[string]) + (both + k.damp[string])).clamp(0.0, 1.0);
            let d = 50f64.powf(2.57 - f64::from(x * x * x) * f64::from_bits(0x3fec_c8b4_3958_1061))
                as f32;
            let c = (d * 4.535_147_3e-5).min(1.0);
            voice.damp[2 * string] = c;
            voice.sharp[2 * string] = (1.0 - c) * sharp;
            let c = ((1.0 - k.damp_offset[string]) * d * 4.535_147_3e-5).min(1.0);
            voice.damp[2 * string + 1] = c;
            voice.sharp[2 * string + 1] = (1.0 - c) * sharp;
            let x = (self.matrix(&voice, decays[string])
                + (self.matrix(&voice, both_decay_target) + k.decay[string]))
                .clamp(0.0, 1.0);
            let mut g = powf(x, 0.05);
            if k.decay_invert[string] {
                g = -g;
            }
            voice.gain_target[2 * string] = g;
            voice.gain_target[2 * string + 1] = g;
        }
        let spread = self.matrix(&voice, target::SEPARATION);
        let s = (spread + k.separation).clamp(0.0, 1.0);
        let level = powf(10.0, self.matrix(&voice, target::LEVEL) * 2.0);
        let f = (1.0 - k.mix) * level * 0.05;
        voice.mix_target[0] = (1.0 - s) * f * self.key_left[key];
        voice.mix_target[1] = f * s * self.key_right[key];
        let spread = self.matrix(&voice, target::SEPARATION);
        let s = (spread + (1.0 - k.separation)).clamp(0.0, 1.0);
        let level = powf(10.0, self.matrix(&voice, target::LEVEL) * 2.0);
        let f = level * k.mix * 0.05;
        voice.mix_target[2] = (1.0 - s) * f * self.key_left[key];
        voice.mix_target[3] = f * s * self.key_right[key];
        voice.comb_feedback = if k.comb_feedback_invert {
            -k.comb_feedback
        } else {
            k.comb_feedback
        };
        voice.comb_level = if k.comb_level_invert {
            -k.comb_level
        } else {
            k.comb_level
        };
        let dt = frames as f64 * (1.0 / INNER);
        let rate;
        if voice.released_at > -1.0 {
            if voice.released_at > f64::from(k.amp_release) {
                voice.kill();
                self.voices[n] = voice;
                return;
            }
            voice.amp_target = 0.0;
            voice.released_at += dt;
            rate = 8.82 / f64::from(k.amp_release * 48_000.0);
        } else if f64::from(k.amp_attack) > voice.time {
            rate = 4.658_742_367_744_009e-6 / f64::from(k.amp_attack);
            voice.amp_target = 5.0;
        } else {
            voice.amp_target = k.amp_sustain;
            rate = 8.82 / f64::from(k.amp_decay * 48_000.0);
        }
        voice.amp_rate = rate as f32;
        if voice.first {
            voice.gain = voice.gain_target;
            voice.mix = voice.mix_target;
        }
        voice.time += dt;
        self.voices[n] = voice;
    }

    /// The engine's samples of a block (`0x1800343e0`): every voice works
    /// out its block, then the sounding ones play four at a time, the
    /// free ones filling the last four up.
    fn render_inner(&mut self, frames: usize) {
        for n in 0..VOICES {
            self.prepare(n, frames);
        }
        self.order.clear();
        let voices = &self.voices;
        self.order.extend((0..VOICES).filter(|n| voices[*n].active));
        let sounding = self.order.len();
        self.order
            .extend((0..VOICES).filter(|n| !voices[*n].active));
        let [left, right] = &mut self.inner;
        left[1..=frames].fill(0.0);
        right[1..=frames].fill(0.0);
        let mut group = 0;
        while group < sounding {
            let lanes: [Option<usize>; 4] =
                std::array::from_fn(|lane| self.order.get(group + lane).copied());
            for frame in 1..=frames {
                let mut sum = [0.0f32; 2];
                let mut draws = [0.0f32; 4];
                for draw in &mut draws {
                    *draw = self.noise as f32 * 2.328_306_4e-10;
                    self.noise = self
                        .noise
                        .wrapping_mul(0x0bb3_8435)
                        .wrapping_add(0x3619_636b);
                }
                for (lane, n) in lanes.iter().enumerate() {
                    let Some(n) = *n else { continue };
                    let voice = &mut self.voices[n];
                    if draws[lane] > voice.noise_gate {
                        voice.held = draws[lane];
                    }
                    if voice.active {
                        let [l, r] = play(voice);
                        sum[0] += l;
                        sum[1] += r;
                    }
                }
                left[frame] += sum[0];
                right[frame] += sum[1];
            }
            for n in lanes.into_iter().flatten() {
                if self.voices[n].silent > 1000 {
                    self.voices[n].kill();
                }
            }
            group += 4;
        }
    }
}

/// One engine sample of a sounding voice (`0x180031960`, one lane).
fn play(voice: &mut Voice) -> [f32; 2] {
    // The exciter.
    let excitation = voice.noise_scale * voice.held + voice.noise_offset;
    let into = (excitation - 2.0 * voice.x1) + voice.x2;
    let y = into * voice.b0 - (voice.a2 * voice.y2 + voice.a1 * voice.y1);
    voice.y2 = voice.y1;
    voice.y1 = y;
    voice.x2 = voice.x1;
    voice.x1 = excitation;
    voice.hi_env += (voice.hi_target - voice.hi_env) * voice.hi_rate;
    let g = voice.hi_cut * voice.hi_env;
    let [s1, s2, s3] = &mut voice.ladder;
    *s1 += ((voice.hi_res * voice.out + y) - *s1) * g;
    *s2 += (*s1 - *s2) * g;
    *s3 += (*s2 - *s3) * g;
    let s4 = voice.out + (*s3 - voice.out) * g;
    voice.out = s4 / (s4 * s4 * voice.drive + 1.0);
    let exciter = voice.out;
    // The strings, as many steps as the pitch asks for.
    let phase = voice.step + voice.along;
    let steps = phase as i32;
    voice.along = phase - f64::from(steps);
    for _ in 0..steps {
        voice.prev = voice.cur;
        let at: [usize; 4] = std::array::from_fn(|n| voice.lines[n].here());
        let comb = [voice.combs[0].here(), voice.combs[1].here()];
        let held: [f32; 4] = std::array::from_fn(|n| voice.lines[n].samples[at[n]]);
        for n in 0..4 {
            let rate = voice.follow_rate[n / 2];
            voice.follow[n] += rate * (exciter - voice.follow[n]);
            let into = exciter - voice.follow[n];
            voice.low[n] +=
                ((voice.sharp[n] * voice.y[n] + held[n]) - voice.low[n]) * voice.damp[n];
            let t = voice.y[n] + (voice.low[n] - voice.y[n]) * voice.damp[n];
            voice.y[n] = t / (t * t * voice.saturation + 1.0);
            voice.lines[n].samples[at[n]] = voice.gain[n] * voice.y[n] + into;
            voice.lines[n].at += 1;
        }
        let first = voice.comb_level * voice.combs[0].samples[comb[0]] + held[0];
        let second = voice.comb_level * voice.combs[1].samples[comb[1]] + held[2];
        voice.cur = [
            voice.mix[2] * second + voice.mix[0] * first,
            voice.mix[3] * second + voice.mix[1] * first,
        ];
        let feedback = voice.comb_feedback;
        let c0 = &mut voice.combs[0].samples[comb[0]];
        *c0 = *c0 * feedback + held[1];
        let c1 = &mut voice.combs[1].samples[comb[1]];
        *c1 = *c1 * feedback + held[3];
        voice.combs[0].at += 1;
        voice.combs[1].at += 1;
        for n in 0..4 {
            voice.gain[n] += (voice.gain_target[n] - voice.gain[n]) * 0.05;
            voice.mix[n] += (voice.mix_target[n] - voice.mix[n]) * 0.001;
        }
    }
    let along = voice.along;
    let amp = f64::from(voice.amp);
    let out: [f32; 2] = std::array::from_fn(|side| {
        let (prev, cur) = (voice.prev[side], voice.cur[side]);
        ((f64::from(cur - prev) * along + f64::from(prev)) * amp) as f32
    });
    if out[0].abs() >= 1e-8 || out[1].abs() >= 1e-8 {
        voice.silent = 0;
    } else if voice.silent >= 0 {
        voice.silent += 1;
    }
    voice.amp = (voice.amp_target - voice.amp) * voice.amp_rate + voice.amp;
    if voice.amp > 1.0 {
        voice.amp = 1.0;
    }
    out
}

impl Generator for Sakura {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let length = StateError::Length {
            expected: STATE_LEN,
            found: bytes.len(),
        };
        let (head, rest) = bytes.split_first_chunk::<8>().ok_or(length)?;
        let (magic, size) = head.split_at(4);
        if magic != b"SSLF" {
            let found = i32::from_le_bytes(magic.try_into().unwrap_or([0; 4]));
            return Err(StateError::Version { found });
        }
        let size = u32::from_le_bytes(size.try_into().unwrap_or([0; 4])) as usize;
        if size == 0 || rest.len() != size {
            return Err(length);
        }
        // A chunk that went through a VST host has a header of 60 bytes.
        let chunk = match rest.starts_with(b"CcnK") {
            true => rest.get(60..).ok_or(length)?,
            false => rest,
        };
        let magic = chunk.first_chunk::<4>().ok_or(length)?;
        // "Skr1", the high byte first on disk; "Skr D" is a demo's save.
        if magic != b"1rkS" {
            return Err(StateError::Version {
                found: i32::from_le_bytes(*magic),
            });
        }
        let words = chunk[4..].as_chunks::<4>().0;
        let mut params = self.params;
        for (n, word) in words.iter().take(PARAMS).enumerate() {
            let value = f32::from_le_bytes(*word);
            if !value.is_finite() {
                return Err(StateError::Range {
                    param: n,
                    value: i32::from_le_bytes(*word),
                });
            }
            params[n] = value;
        }
        let name = chunk.get(4 + PARAMS * 4..).filter(|rest| rest.len() >= 128);
        self.name = match name {
            Some(name) => {
                let text = name[..128].split(|byte| *byte == 0).next().unwrap_or(&[]);
                text.iter().map(|byte| char::from(*byte)).collect()
            }
            None => self.name.clone(),
        };
        self.stop();
        self.clear_effects();
        for (n, value) in params.iter().enumerate() {
            self.apply(n, *value);
        }
        self.size_voices();
        Ok(())
    }

    /// Cuts every note and lays the effects' lines out for the rate, as
    /// FL's change of rate does.
    fn set_sample_rate(&mut self, hz: f32) {
        self.rate = f64::from(hz);
        let points = (48.0 * INNER / self.rate) as usize + 8;
        self.inner = [vec![0.0; points], vec![0.0; points]];
        let rate = self.rate;
        self.chorus.set_sample_rate(rate);
        self.plate.set_sample_rate(hz);
        self.plate.set(9, 1.0);
        self.plate.set(5, 1.0);
        self.resonators.set_sample_rate(rate);
        self.echo.set_sample_rate(rate);
        self.high = [HighPass::new(32.0, 1.0, hz); 2];
        self.stop();
        self.clear_effects();
    }

    /// The delay's sync reads it.
    fn set_tempo(&mut self, bpm: f64) {
        self.echo.tempo = bpm;
    }

    fn note_on(&mut self, note: NoteOn) {
        let key = note.key.clamp(0, 127);
        let slot = key as usize;
        if self.owner[slot].is_some_and(|owner| owner != note.voice) {
            self.let_go(i32::from(key));
        }
        self.owner[slot] = Some(note.voice);
        let (left, right) = circular(note.pan, 1.0);
        self.key_left[slot] = left;
        self.key_right[slot] = right;
        self.key_pitch[slot] = note.fine_pitch * 0.01;
        let level = crate::bassdrum::velocity_level(note.velocity);
        if level > 0.0 {
            self.start(i32::from(key), level);
        } else {
            self.let_go(i32::from(key));
        }
    }

    fn note_off(&mut self, voice: u32) {
        if let Some(key) = self.owner.iter().position(|owner| *owner == Some(voice)) {
            self.let_go(key as i32);
        }
    }

    fn all_notes_off(&mut self) {
        for key in 0..128 {
            if self.owner[key].is_some() {
                self.let_go(key as i32);
            }
        }
    }

    fn set_note_pitch(&mut self, voice: u32, pitch: f32) {
        if let Some(key) = self.owner.iter().position(|owner| *owner == Some(voice)) {
            self.key_pitch[key] = pitch * 0.01 - key as f32 + 60.0;
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let frames = left.len().min(right.len());
        if frames == 0 {
            return;
        }
        let (left, right) = (&mut left[..frames], &mut right[..frames]);
        self.lfo_value = (f64::from(self.lfo_phase) * LFO_PI * 2.0).cos() as f32;
        let phase = f64::from(frames as f32 * self.knobs.lfo_rate) * (1.0 / self.rate)
            + f64::from(self.lfo_phase);
        self.lfo_phase = phase as f32;
        if self.lfo_phase > 1.0 {
            self.lfo_phase -= 1.0;
        }
        let ratio = INNER / self.rate;
        let mut done = 0;
        // The engine's output, kept apart from what the host's block holds.
        // FL runs the effects once over its whole block; they hold no state
        // that a block's length changes, so they run here 48 frames at a
        // time with the engine.
        let mut out = [[0.0f32; 48]; 2];
        while done < frames {
            let n = (frames - done).min(48);
            let mut k = 0;
            while self.position < 0.0 && k < n {
                let at = self.count as f64 + self.position;
                let i = at as i32 as usize;
                let along = (at - i as f64) as f32;
                for (o, b) in out.iter_mut().zip(&self.inner) {
                    o[k] = (b[i + 1] - b[i]) * along + b[i];
                }
                self.position += ratio;
                k += 1;
            }
            let rest = n - k;
            if rest > 0 {
                let end = rest as f64 * ratio + self.position;
                let mut count = end as i32 as usize;
                if (count as f64) < end {
                    count += 1;
                }
                let count = count.min(self.inner[0].len() - 2);
                self.count = count;
                self.render_inner(count);
                for frame in k..n {
                    let i = self.position as i32 as usize;
                    let along = (self.position - i as f64) as f32;
                    for (o, b) in out.iter_mut().zip(&self.inner) {
                        o[frame] = (b[i + 1] - b[i]) * along + b[i];
                    }
                    self.position += ratio;
                }
                self.position -= count as f64;
                for side in &mut self.inner {
                    side[0] = side[count];
                }
            }
            let [l, r] = &mut out;
            let (l, r) = (&mut l[..n], &mut r[..n]);
            self.effects(l, r);
            let level = self.knobs.level;
            for i in 0..n {
                left[done + i] += l[i] * level;
                right[done + i] += r[i] * level;
            }
            done += n;
        }
    }

    fn param_count(&self) -> usize {
        PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).copied().unwrap_or(0.0)
    }

    /// As the state holds it, and not rounded. A NaN is not taken.
    fn set_param(&mut self, index: usize, value: f32) {
        if index < PARAMS && value.is_finite() {
            self.apply(index, value);
        }
    }

    fn reset(&mut self) {
        self.stop();
        self.clear_effects();
    }

    fn tail(&self) -> usize {
        let k = &self.knobs;
        let seconds = f64::from(k.amp_release) + 0.05;
        let effects = if self.plate.on || self.echo.on {
            12.0
        } else {
            0.0
        };
        ((seconds + effects) * self.rate) as usize + 1100
    }

    fn fidelity(&self) -> Fidelity {
        if self.short {
            Fidelity::Approximate("Sakura string longer than its voice holds")
        } else {
            Fidelity::Exact
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    fn note(voice: u32, key: i16) -> NoteOn {
        NoteOn {
            voice,
            key,
            ..NoteOn::default()
        }
    }

    /// One note on `key`, let go at `release`, in blocks of 512 that end
    /// where the note is let go, as the harness in
    /// `.scratch/notes/sakura-emu` plays the engine DLL.
    fn render(synth: &mut Sakura, key: i16, frames: usize, release: usize) -> [Vec<f32>; 2] {
        let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
        synth.note_on(note(1, key));
        let mut done = 0;
        while done < frames {
            if done == release {
                synth.note_off(1);
            }
            let mut end = (done + 512).min(frames);
            if done < release {
                end = end.min(release);
            }
            synth.process(&mut left[done..end], &mut right[done..end]);
            done = end;
        }
        [left, right]
    }

    /// A state of the length FL writes with these parameters and name.
    fn state_of(params: &[f32; PARAMS]) -> Vec<u8> {
        let mut state = b"SSLF".to_vec();
        state.extend((STATE_LEN as u32 - 8).to_le_bytes());
        state.extend(b"1rkS");
        for p in params {
            state.extend(p.to_le_bytes());
        }
        let mut name = [0u8; 128];
        name[..4].copy_from_slice(b"Test");
        state.extend(name);
        assert_eq!(state.len(), STATE_LEN);
        state
    }

    #[test]
    fn a_state_is_read_and_a_bad_one_refused() {
        let mut params = DEFAULTS;
        params[0] = 0.25;
        params[113] = 1.0;
        let mut synth = Sakura::default();
        synth.set_state(&state_of(&params)).unwrap();
        assert_eq!(synth.program(), "Test");
        assert_eq!(synth.param(0), 0.25);
        assert_eq!(synth.param(113), 1.0);
        assert_eq!(synth.fidelity(), Fidelity::Exact);

        let good = state_of(&DEFAULTS);
        let mut bad = good.clone();
        bad[..4].copy_from_slice(b"XXXX");
        assert!(matches!(
            synth.set_state(&bad),
            Err(StateError::Version { .. })
        ));
        let mut bad = good.clone();
        bad[8..12].copy_from_slice(b"2rkS");
        assert!(matches!(
            synth.set_state(&bad),
            Err(StateError::Version { .. })
        ));
        assert!(matches!(
            synth.set_state(&good[..100]),
            Err(StateError::Length { .. })
        ));
        let mut bad = good;
        bad[12 + 4 * 5..12 + 4 * 6].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(matches!(
            synth.set_state(&bad),
            Err(StateError::Range { param: 5, .. })
        ));
        // A refused state leaves the one before.
        assert_eq!(synth.param(0), 0.25);
    }

    /// The level knob is `10^(4.8 v - 1.8)`, with the engine's 1.8 an f32
    /// a step above the nearest one, as its 4.8 is: 0.375 is unity.
    #[test]
    fn the_level_knob_is_decibels_with_the_engine_s_constant() {
        let mut synth = Sakura::default();
        synth.apply(0, 0.375);
        assert_eq!(synth.knobs.level, 1.0);
        synth.apply(0, 0.0);
        assert!((synth.knobs.level - 10f32.powf(-1.8)).abs() < 1e-6);
        synth.apply(0, 1.0);
        assert!((synth.knobs.level - 1000.0).abs() < 0.01);
    }

    /// A key's string is `48000 / f` samples long, split between the two
    /// strings by the ratio, and the octave knob moves `f` by octaves.
    #[test]
    fn a_key_s_strings_are_a_period_long() {
        let mut knobs = Sakura::default().knobs;
        let (f, first, second) = lengths(69, &knobs);
        assert!((f - 440.0).abs() < 1e-9);
        assert_eq!((first, second), (109, 109));
        knobs.ratio = 4;
        assert_eq!(lengths(69, &knobs), (f, 109, 218));
        knobs.ratio = 0;
        // A quarter, and the first string a whole number of them.
        assert_eq!(lengths(69, &knobs), (f, 108, 27));
        knobs.ratio = 3;
        knobs.octave = 1.0;
        let (f, first, _) = lengths(69, &knobs);
        assert!((f - 3520.0).abs() < 1e-9);
        assert_eq!(first, 13);
        knobs.octave = 0.0;
        let (f, first, _) = lengths(69, &knobs);
        assert!((f - 55.0).abs() < 1e-9);
        assert_eq!(first, 872);
    }

    /// The envelope rises over its attack, falls to the sustain over its
    /// decay and to nothing over its release, in straight lines a block
    /// at a time.
    #[test]
    fn the_envelope_is_straight_lines() {
        let mut e = Envelope::default();
        e.start(0.01, 0.1, 0.25);
        // 480 samples of attack: half way after 240.
        let values: Vec<f32> = (0..5).map(|_| e.tick(48)).collect();
        assert!((values[4] - 0.5).abs() < 1e-6, "{values:?}");
        assert!(values.windows(2).all(|w| w[1] > w[0]));
        while e.stage == 0 {
            e.tick(48);
        }
        assert_eq!(e.value, 1.0);
        // 4800 of decay to a quarter: five eighths after half of it.
        for _ in 0..50 {
            e.tick(48);
        }
        assert!((e.value - 0.625).abs() < 1e-9, "{}", e.value);
        for _ in 0..60 {
            e.tick(48);
        }
        assert_eq!(e.value, 0.25);
        e.let_go(0.1);
        for _ in 0..50 {
            e.tick(48);
        }
        assert!((e.value - 0.125).abs() < 1e-9, "{}", e.value);
        for _ in 0..60 {
            e.tick(48);
        }
        assert_eq!(e.value, 0.0);
        assert!(e.idle);
    }

    /// A plucked string sounds at its key: the sound repeats every
    /// `44100 / 440` frames for A at 440 Hz.
    #[test]
    fn a_note_sounds_at_its_pitch() {
        let mut synth = Sakura::default();
        let [left, _] = render(&mut synth, 69, 8192, usize::MAX);
        let tail = &left[4096..];
        let peak = tail.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.01, "{peak}");
        let correlation = |lag: usize| -> f64 {
            let n = tail.len() - lag;
            (0..n)
                .map(|i| f64::from(tail[i]) * f64::from(tail[i + lag]))
                .sum::<f64>()
                / n as f64
        };
        let best = (60..140)
            .max_by(|a, b| correlation(*a).total_cmp(&correlation(*b)))
            .unwrap();
        assert!((99..=101).contains(&best), "{best}");
    }

    #[test]
    fn follows_note_pitch() {
        testing::follows_note_pitch(|| Box::new(Sakura::default()), 44_100.0);
    }

    /// Bit for bit what `engine_x64.dll`'s own `CSakuraSynth` plays for the
    /// render test's state under the harness in `.scratch/notes/sakura-emu`
    /// (key 60, 20000 frames at 44.1 kHz, let go at 9999): every sample's
    /// bits folded together, and three of them.
    #[test]
    fn plays_the_render_test_as_the_engine_dll_does() {
        let Some(root) = testing::corpus() else {
            return;
        };
        let saved = testing::projects(&root.join("System/Render tests"));
        let ours: Vec<_> = saved.iter().filter(|s| s.plugin == NAME).collect();
        assert_eq!(ours.len(), 1);
        let mut synth = Sakura::default();
        synth.set_sample_rate(44_100.0);
        synth.set_state(&ours[0].state).unwrap();
        let [left, right] = render(&mut synth, 60, 20_000, 9999);
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for (l, r) in left.iter().zip(&right) {
            for bits in [l.to_bits(), r.to_bits()] {
                hash = (hash ^ u64::from(bits)).wrapping_mul(0x0100_0000_01b3);
            }
        }
        let at = |n: usize| [left[n].to_bits(), right[n].to_bits()];
        assert_eq!(at(1000), [0xbf45_de71, 0xbf08_0233]);
        assert_eq!(at(5000), [0xbe3d_897d, 0xbea4_8975]);
        assert_eq!(at(12_000), [0xbc3b_c596, 0xbbc3_fc82]);
        assert_eq!(hash, 0x3772_88ed_838c_d379);
    }

    /// How many Sakura presets FL ships, the plugin database's among them.
    const PRESETS: usize = 504;

    /// Every Sakura state FL ships, in a preset or a project, loads as its
    /// floats say and plays a note that is heard and stays finite.
    #[test]
    fn every_shipped_state_loads_and_plays() {
        let Some(root) = testing::corpus() else {
            return;
        };
        let mut saved = testing::every_preset(&root, NAME);
        let presets = saved.len();
        let ours = |saved: &testing::Saved| saved.plugin.eq_ignore_ascii_case(NAME);
        saved.extend(testing::projects(&root).into_iter().filter(ours));
        assert_eq!((presets, saved.len()), (PRESETS, PRESETS + 5));
        for saved in saved {
            assert_eq!(saved.state.len(), STATE_LEN, "{}", saved.file);
            let mut synth = crate::generator_by_name(&saved.plugin).unwrap();
            if let Err(error) = synth.set_state(&saved.state) {
                panic!("{}: {error}", saved.file);
            }
            assert_eq!(synth.fidelity(), Fidelity::Exact, "{}", saved.file);
            for index in 0..PARAMS {
                let at = 12 + 4 * index;
                let word = f32::from_le_bytes(saved.state[at..at + 4].try_into().unwrap());
                assert_eq!(synth.param(index), word, "{}", saved.file);
            }
            synth.note_on(note(1, 60));
            let (mut left, mut right) = (vec![0.0; 8192], vec![0.0; 8192]);
            for (l, r) in left.chunks_mut(512).zip(right.chunks_mut(512)) {
                synth.process(l, r);
            }
            let all = || left.iter().chain(&right);
            assert!(all().all(|v| v.is_finite()), "{}", saved.file);
            let peak = all().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!((1e-6..16.0).contains(&peak), "{peak} in {}", saved.file);
        }
    }

    fn env<T: std::str::FromStr>(name: &str, default: T) -> T {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }

    #[test]
    fn dump_for_dll() {
        let Ok(path) = std::env::var("SAKURA_STATE") else {
            return;
        };
        let frames: usize = env("SAKURA_FRAMES", 4096);
        let key: i16 = env("SAKURA_KEY", 60);
        let release: usize = env("SAKURA_RELEASE", usize::MAX);
        let block: usize = env("SAKURA_BLOCK", 512);
        let rate: f32 = env("SAKURA_RATE", 44_100.0);
        let velocity: f32 = env::<f32>("SAKURA_VELOCITY", 100.0) / 100.0;
        let state = std::fs::read(path).unwrap();
        // The harness runs the DLL flushing results to zero (MXCSR 0x9f80).
        #[cfg(target_arch = "x86_64")]
        if std::env::var("SAKURA_FTZ").is_ok() {
            #[allow(deprecated, unsafe_code)]
            // SAFETY: only how results under f32's normal range round.
            unsafe {
                std::arch::x86_64::_mm_setcsr(0x9f80)
            };
        }
        let mut synth = Sakura::default();
        synth.set_sample_rate(rate);
        synth.set_state(&state).unwrap();
        eprintln!("HP {:?}", synth.high[0]);
        let mut events: Vec<(usize, bool, i16)> = std::env::var("SAKURA_EVENTS")
            .unwrap_or_default()
            .split_whitespace()
            .map(|event| {
                let parts: Vec<&str> = event.split(':').collect();
                (
                    parts[2].parse().unwrap(),
                    parts[0] == "on",
                    parts[1].parse().unwrap(),
                )
            })
            .collect();
        if events.is_empty() {
            events.push((0, true, key));
            if release != usize::MAX {
                events.push((release, false, key));
            }
        }
        events.sort();
        events.reverse();
        let mut out = String::new();
        let mut done = 0;
        while done < frames {
            while events.last().is_some_and(|event| event.0 <= done) {
                let (_, on, key) = events.pop().unwrap();
                if on {
                    synth.note_on(NoteOn {
                        velocity,
                        ..note(key as u32 + 1000, key)
                    });
                } else {
                    synth.note_off(key as u32 + 1000);
                }
            }
            // A block ends at the next event, as the harness's does.
            let mut n = block.min(frames - done);
            if let Some(event) = events.last() {
                n = n.min(event.0 - done);
            }
            let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
            synth.process(&mut l, &mut r);
            for i in 0..n {
                out += &format!("{:?} {:?}\n", l[i], r[i]);
            }
            done += n;
        }
        std::fs::write(std::env::var("SAKURA_OUT").unwrap(), out).unwrap();
    }
}
