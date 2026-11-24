//! Sawer: a vintage saw synth of up to 24 voices, each a sampled saw with
//! a sub saw, sync and noise into a four-pole ladder, run six times over
//! the host's rate and brought down by a biquad, rebuilt from Sawer's
//! `engine_x64.dll` (`docs/plugins/sawer.md`). `Sawer_x64.dll` beside it is
//! the panel only.
//!
//! The engine is Image-Line's `CSynthesizer` frame (as Sakura's is), with a
//! `SawerSynth` holding the knobs and a list of `CSawerGenerator` voices.
//! The saw is FL's `data.saw`, 8192 f32 beside the DLL, read through
//! [`Generator::set_resource_dir`]. The voice path is ported as the DLL
//! computes it, its one-voice loop (`0x180036710`) and its four-voice SSE
//! loop (`0x180035160`) apart, and checked against the DLL under unicorn
//! (`.scratch/notes/sawer-emu/`). The chorus, phaser, delay and reverb are
//! not built yet: a state that turns one on plays without it and says so
//! in [`Generator::fidelity`].

use crate::generator::{Generator, NoteOn};
use crate::sakura_fx::{HighPass, sin_cos_f64};
use crate::{Fidelity, StateError};
use std::path::Path;
use std::sync::Arc;

pub const NAME: &str = "Sawer";
/// `SSLF`, the chunk's length, `Swr1`, 96 f32, 96 bytes of the engine's
/// own and the program's name.
pub const STATE_LEN: usize = 620;
/// The engine's parameters.
pub const PARAMS: usize = 96;
/// The saw, under `Plugins/Fruity/Generators`.
pub const DATA: &str = "Sawer/data.saw";

const VOICES: usize = 24;
const SAW: usize = 8192;
/// The voices run at six times the host's rate.
const OVER: usize = 6;
/// The frames one engine render takes: FL's wrapper (`FUN_180004190`)
/// cuts the host's block to its buffer, 64 frames.
const BLOCK: usize = 64;
const TINY: f32 = 1e-9;

/// The two random generators the engine keeps (`0x180086e30`,
/// `0x180086e34`) as they stand when FL has made one Sawer and given it a
/// sample rate (`emu`).
const SEED: u32 = 0x0026_d808;

/// `x * 0x0bb38435 + 0x3619636b`, the noise.
fn step(x: u32) -> u32 {
    x.wrapping_mul(0x0bb3_8435).wrapping_add(0x3619_636b)
}

/// `x * 0x77a73631 + 0xf6ff3a94`, the phases of a voice made over.
fn step2(x: u32) -> u32 {
    x.wrapping_mul(0x77a7_3631).wrapping_add(0xf6ff_3a94)
}

/// A draw as a fraction of 2^32.
fn unit(x: u32) -> f32 {
    x as f32 * f32::from_bits(0x2f80_0000)
}

fn flush(x: f32) -> f32 {
    if x < TINY && x > -TINY { 0.0 } else { x }
}

/// The runtime's `powf` and `pow`: in f64, rounded once.
fn powf(base: f32, exponent: f32) -> f32 {
    crate::sakura_fx::powf(base, exponent)
}

/// `FUN_180031ff0`: `max(50 v^5, 0.00101)` seconds.
fn fifth(v: f32) -> f64 {
    let d = f64::from(v);
    (d * d * d * d * 50.0 * d).max(0.00101)
}

/// `FUN_180031fc0`: `v` across `lo..=hi`, truncated and kept inside.
fn span(v: f32, lo: i32, hi: i32) -> i32 {
    (((hi - lo) as f32 * v + lo as f32) as i32).clamp(lo, hi)
}

/// What the knobs set in `SawerSynth` (`0x180032090`), by the engine's
/// offsets.
#[derive(Debug, Clone, Copy)]
struct Knobs {
    level: f32,          // 0x3900
    pan: f32,            // 0x3904
    attack: f32,         // 0x3a30
    decay: f32,          // 0x3a34
    sustain: f32,        // 0x3a38
    release: f32,        // 0x3a3c
    transpose: f32,      // 0x39d0
    fine: f32,           // 0x39d4
    mono: bool,          // 0x38e0
    glide_mode: u8,      // 0x38d4
    glide: f32,          // 0x38d0
    octaver: bool,       // 0x38e1
    unison_retrig: bool, // 0x3ac4
    unison: i32,         // 0x38f0
    unison_offset: f32,  // 0x3ac8
    detune: f32,         // 0x38f8
    unison_pan: f32,     // 0x38fc
    sub_phase: f32,      // 0x39d8
    sub_ratio: f32,      // 0x39dc
    sub_level: f32,      // 0x39e0
    sub_detune: f32,     // 0x39e4
    sub_retrig: bool,    // 0x39ec
    sub_invert: f32,     // 0x39e8
    sub_square: bool,    // 0x39ed
    sync: f32,           // 0x39f0
    sync_on: bool,       // 0x39f4
    noise: f32,          // 0x39f8
    cutoff: f32,         // 0x39fc
    resonance: f32,      // 0x3a00
    key_track: f32,      // 0x3a04
    vel_track: f32,      // 0x3a08
    mix_to: [f32; 4],    // 0x3a20
    egf: [f32; 4],       // 0x3a40: attack, decay, sustain, release
    egf_amount: f32,     // 0x3a50
    egf_dest: i32,       // 0x3a54
    lfo_shape: i32,      // 0x3aac
    lfo_speed: f64,      // 0x3aa0
    lfo_division: f32,   // 0x3aa8
    lfo_sync: bool,      // 0x3ab6
    lfo_attack: f32,     // 0x3ab8
    lfo_release: f32,    // 0x3abc
    lfo_retrig: bool,    // 0x3ab5
    lfo_invert: bool,    // 0x3ab4
    lfo_amount: f32,     // 0x3ab0
    lfo_dest: i32,       // 0x3ac0
    polyphony: i32,      // 0x38e4
    bend: f32,           // 0x38dc
    matrix_amount: [f32; 6],
    matrix_dest: [i16; 6],
    matrix_source: [i16; 6],
    chorus: bool,
    phaser: bool,
    delay: bool,
    reverb: bool,
}

impl Default for Knobs {
    fn default() -> Self {
        Knobs {
            level: 1.0,
            pan: 0.5,
            attack: 0.0,
            decay: 0.0,
            sustain: 0.0,
            release: 0.0,
            transpose: -12.0,
            fine: 0.0,
            mono: false,
            glide_mode: 0,
            glide: 1e-4,
            octaver: false,
            unison_retrig: false,
            unison: 1,
            unison_offset: 0.0,
            detune: 0.1,
            unison_pan: 0.0,
            sub_phase: 0.0,
            sub_ratio: 0.0,
            sub_level: 0.0,
            sub_detune: 0.0,
            sub_retrig: false,
            sub_invert: 0.0,
            sub_square: false,
            sync: 0.0,
            sync_on: false,
            noise: 0.0,
            cutoff: 0.0,
            resonance: 0.0,
            key_track: 0.0,
            vel_track: 0.0,
            mix_to: [0.0; 4],
            egf: [0.0; 4],
            egf_amount: 0.0,
            egf_dest: 0,
            lfo_shape: 0,
            lfo_speed: 0.0,
            lfo_division: 1.0,
            lfo_sync: false,
            lfo_attack: 0.0,
            lfo_release: 0.0,
            lfo_retrig: false,
            lfo_invert: false,
            lfo_amount: 0.0,
            lfo_dest: 0,
            polyphony: 24,
            bend: 0.0,
            matrix_amount: [0.0; 6],
            matrix_dest: [0; 6],
            matrix_source: [0; 6],
            chorus: false,
            phaser: false,
            delay: false,
            reverb: false,
        }
    }
}

/// One `CSawerGenerator` (0xf8 bytes), by the engine's offsets.
#[derive(Debug, Clone, Copy, Default)]
struct Voice {
    key: i32,          // 0x10
    unison: i8,        // 0x14
    count: i8,         // 0x15
    random: f32,       // 0x18
    velocity: f32,     // 0x20
    released: bool,    // 0x24
    glide_rate: f32,   // 0x2c
    note: f32,         // 0x30
    age: u32,          // 0x34
    let_go: f64,       // 0x48
    time: f64,         // 0x50
    phase: u32,        // 0x58
    inc: u32,          // 0x5c
    sub_phase: u32,    // 0x60
    sub_inc: u32,      // 0x64
    sync_phase: u32,   // 0x68
    sync_inc: u32,     // 0x6c
    sub_offset: u32,   // 0x70
    gain: [f32; 2],    // 0x74
    gain_to: [f32; 2], // 0x7c
    stage: [f32; 3],   // 0x84..0x8c
    y: f32,            // 0x90
    y_last: f32,       // 0x94
    feedback: f32,     // 0x98
    cut: f32,          // 0x9c
    cut_to: f32,       // 0xa0
    res: f32,          // 0xa4
    attack: f32,       // 0xa8
    decay: f32,        // 0xac
    release: f32,      // 0xb0
    sustain: f32,      // 0xb4
    amp_to: f32,       // 0xb8
    amp: f32,          // 0xbc
    amp_rate: f32,     // 0xc0
    f_attack: f32,     // 0xc4
    f_decay: f32,      // 0xc8
    f_release: f32,    // 0xcc
    f_sustain: f32,    // 0xd0
    fenv_to: f32,      // 0xd4
    fenv: f32,         // 0xd8
    fenv_rate: f32,    // 0xdc
    fenv_cut: f32,     // 0xe0
    drift_to: f32,     // 0xe4
    drift: f32,        // 0xe8
    drift_count: i32,  // 0xec
    noise: f32,        // 0xf0
}

/// The plugin.
pub struct Sawer {
    params: [f32; PARAMS],
    extra: [u8; 96],
    program: [u8; 128],
    knobs: Knobs,
    voices: [Voice; VOICES],
    /// The host's note on each key, and the note's pan gains and pitch.
    owner: [Option<u32>; 128],
    key_gain: [[f32; 128]; 2],
    key_pitch: [f32; 128],
    key_velocity: [f32; 128],
    held: [u8; 128],
    last_key: i32,
    ages: u32,
    rng: u32,
    rng2: u32,
    rate: f64,
    tempo: f64,
    /// The mix of the filter's four taps, now (`0x3a10`).
    mix: [f32; 4],
    /// `0x3b20`: where in the six oversamples the voices are.
    sub_step: i32,
    /// `0x3a5c`: the most the filter's step takes.
    cut_max: f32,
    /// `0x3a58`: base samples between voice updates.
    chunk: i32,
    down: [f32; 3],
    down_state: [[f32; 4]; 2],
    dc: HighPass,
    lfo_phase: f64,
    lfo_held: f32,
    lfo_value: f32,
    gains: [f32; 2],
    velocity_source: f32,
    saw: Option<Arc<[f32]>>,
    looked: bool,
    over: [Vec<f32>; 2],
}

impl Default for Sawer {
    fn default() -> Self {
        let mut synth = Sawer {
            params: [0.0; PARAMS],
            extra: [0xff; 96],
            program: [0; 128],
            knobs: Knobs::default(),
            voices: [Voice {
                key: -1,
                ..Voice::default()
            }; VOICES],
            owner: [None; 128],
            key_gain: [[std::f32::consts::FRAC_1_SQRT_2; 128]; 2],
            key_pitch: [0.0; 128],
            key_velocity: [0.0; 128],
            held: [0; 128],
            last_key: -1,
            ages: 0,
            rng: SEED,
            rng2: 2222,
            rate: 0.0,
            tempo: 120.0,
            mix: [0.0; 4],
            sub_step: 0,
            cut_max: 0.0,
            chunk: 1,
            down: [0.0; 3],
            down_state: [[0.0; 4]; 2],
            dc: HighPass::default(),
            lfo_phase: 0.0,
            lfo_held: 0.0,
            lfo_value: 0.0,
            gains: [0.0; 2],
            velocity_source: 0.0,
            saw: None,
            looked: false,
            over: [vec![0.0; BLOCK * OVER], vec![0.0; BLOCK * OVER]],
        };
        synth.program[..8].copy_from_slice(b"<noname>");
        for (i, v) in DEFAULTS.iter().enumerate() {
            synth.set(i, *v);
        }
        synth.set_rate(44_100.0);
        synth
    }
}

/// The parameters of a new instance (`emu`, read from the engine's table
/// after its constructor).
const DEFAULTS: [f32; PARAMS] = [
    0.5, 0.5, 0.0, 1.0, 1.0, 0.0, 0.5, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.25, 0.0, 1.0, 0.5,
    0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.0, 0.0,
    0.0, 0.5, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.25, 0.3, 0.0, 0.3, 0.2, 0.7, 0.0, 0.6,
    0.0, 0.4, 0.4, 0.3, 0.25, 0.0, 1.0, 0.5, 0.0, 0.5, 0.5, 0.5, 0.5, 0.0, 0.0, 0.5, 0.0, 1.0, 8.0,
    2.0, 0.0, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
    0.0,
];

/// Reads `data.saw`: 8192 f32, then the same less itself half a turn on
/// (`0x180031680`), for the square sub.
fn read_saw(path: &Path) -> Option<Arc<[f32]>> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() < 4 * SAW {
        return None;
    }
    let mut t = vec![0.0f32; 2 * SAW];
    for (i, v) in t[..SAW].iter_mut().enumerate() {
        *v = f32::from_le_bytes(bytes[4 * i..4 * i + 4].try_into().unwrap());
    }
    for i in 0..SAW {
        t[SAW + i] = t[i] - t[(i + SAW / 2) % SAW];
    }
    Some(t.into())
}

impl Sawer {
    /// A parameter's value with the matrix on it (`0x180037910`), kept to
    /// 0..1.
    fn modulated(&self, n: usize, v: f32) -> f32 {
        if n >= 0x49 {
            return v;
        }
        let k = &self.knobs;
        let mut v = v;
        for s in 0..6 {
            if k.matrix_source[s] != 0 && k.matrix_dest[s] == n as i16 {
                let source = match k.matrix_source[s] {
                    1 => self.velocity_source,
                    _ => 0.0,
                };
                v += source * k.matrix_amount[s];
            }
        }
        v.clamp(0.0, 1.0)
    }

    /// `0x180032020`: store a parameter and set what it sets.
    fn set(&mut self, n: usize, raw: f32) {
        if n >= PARAMS {
            return;
        }
        self.params[n] = raw;
        let v = self.modulated(n, raw);
        let k = &mut self.knobs;
        match n {
            0 => k.level = powf(10.0, v * 2.4 - f32::from_bits(0x3f66_6667)),
            1 => k.pan = v.clamp(0.0, 1.0),
            2 => k.attack = fifth(v) as f32,
            3 => k.decay = fifth(v) as f32,
            4 => k.sustain = v,
            5 => k.release = (fifth(v) * 0.35 + 0.02) as f32,
            6 => k.transpose = span(v, -24, 24) as f32 - 12.0,
            7 => k.fine = (v + v) - 1.0,
            8 => {
                if k.mono != (v != 0.0) {
                    k.mono = v != 0.0;
                    self.all_keys_off();
                }
            }
            9 | 10 => {
                let (time, key) = (self.params[9], self.params[10]);
                self.knobs.glide_mode = match (time != 0.0, key != 0.0) {
                    (false, true) => 0,
                    (false, false) => 1,
                    (true, true) => 2,
                    (true, false) => 3,
                };
            }
            11 => {
                let g = v * v * v;
                k.glide = if f64::from(g) < 1e-4 {
                    f32::from_bits(0x38d1_b717)
                } else {
                    g
                };
            }
            12 => k.octaver = v != 0.0,
            13 => k.unison_retrig = v != 0.0,
            14 => {
                k.unison = [1, 2, 4, 8][span(v, 0, 3) as usize].min(8);
                k.unison_offset = self.params[15] * -0.25;
                self.unison_offset();
            }
            15 => {
                k.detune = v * 0.5;
                k.unison_offset = v * -0.25;
                self.unison_offset();
            }
            16 => k.unison_pan = v.clamp(0.0, 1.0),
            17 => k.sub_phase = v,
            18 => {
                k.sub_ratio = match (v * 8.0) as i32 {
                    i32::MIN..=0 => 0.25,
                    1 => f32::from_bits(0x3eaa_aaab),
                    2 => 0.5,
                    3 => f32::from_bits(0x3f2a_aaab),
                    4 => 1.0,
                    5 => 1.5,
                    6 => 2.0,
                    7 => 3.0,
                    _ => 4.0,
                }
            }
            19 => k.sub_level = v * v,
            20 => k.sub_detune = v * v,
            21 => k.sub_retrig = v != 0.0,
            22 => k.sub_invert = if v != 0.0 { -1.0 } else { 1.0 },
            23 => k.sub_square = v != 0.0,
            24 => k.sync = v * 4.0,
            25 => k.sync_on = v != 0.0,
            26 => k.noise = v,
            27 => k.cutoff = v,
            28 => k.resonance = v,
            29 => k.key_track = v,
            30 => k.vel_track = v,
            31 => {
                let mode = span(v, 0, 3);
                let tiny = f32::from_bits(0x3586_37bd);
                k.mix_to = [tiny; 4];
                match mode {
                    0 => k.mix_to[0] = 1.0,
                    1 => k.mix_to[1] = 1.0,
                    2 => k.mix_to[2] = 1.0,
                    _ => k.mix_to[3] = f32::from_bits(0x3f19_999a),
                }
            }
            32 => k.egf[0] = fifth(v) as f32,
            33 => k.egf[1] = fifth(v) as f32,
            34 => k.egf[2] = v * v,
            35 => k.egf[3] = fifth(v) as f32,
            36 => k.egf_amount = v,
            37 => k.egf_dest = span(v, 0, 3),
            38 => k.lfo_shape = span(v, 0, 3),
            39 => {
                k.lfo_speed = f64::from(v * v * v * 10.0);
                k.lfo_division = [1.0, 2.0, 3.0, 4.0, 8.0, 12.0, 16.0, 32.0, 64.0, 128.0]
                    [span(v, 0, 9) as usize];
            }
            40 => k.lfo_sync = v != 0.0,
            41 => k.lfo_attack = v * v * v * 20.0 + 0.01,
            42 => k.lfo_release = v * v * v * 20.0 + 0.01,
            43 => k.lfo_retrig = v != 0.0,
            44 => k.lfo_invert = v != 0.0,
            45 => k.lfo_amount = v,
            46 => k.lfo_dest = ((v * 6.0) as i32).clamp(0, 6),
            47 => k.chorus = v != 0.0,
            51 => k.phaser = v != 0.0,
            57 => k.delay = v != 0.0,
            65 => k.reverb = v != 0.0,
            0x4b => {
                let n = ((v + 1.0) * 4.0) as i32;
                k.polyphony = if (1..=24).contains(&n) { n } else { 24 };
            }
            0x4c => k.bend = (v as i32) as f32,
            0x4e..=0x53 => {
                let d = (v + v) - 1.0;
                let mut a = d * d;
                if d < 0.0 {
                    a = -a;
                }
                k.matrix_amount[n - 0x4e] = a;
            }
            0x54..=0x59 => k.matrix_dest[n - 0x54] = (v as i32) as i16,
            0x5a..=0x5f => k.matrix_source[n - 0x5a] = (v as i32) as i16,
            _ => {}
        }
    }

    fn unison_offset(&mut self) {
        if self.params[14] < 0.25 {
            self.knobs.unison_offset = 0.0;
        }
    }

    /// `0x38c8`: the pitch every voice adds.
    fn pitch_offset(&self) -> f32 {
        self.knobs.fine + self.knobs.transpose + self.knobs.unison_offset
    }

    /// The sample rate (`0x180037480`, and `0x180023140` before it): every
    /// voice made over, the filters designed again.
    fn set_rate(&mut self, rate: f64) {
        self.rate = rate;
        for i in 0..VOICES {
            self.kill_voice(i);
            self.voices[i].note = 48.0;
        }
        self.last_key = -1;
        self.gains = [self.master(false), self.master(true)];
        let r = rate as f32;
        self.dc = HighPass::new(4.0, 1.0, r);
        self.chunk = 2 - (rate * -0.001) as i32;
        self.cut_max = (121_026.0 / rate) as f32 * f32::from_bits(0x3e2a_aaab);
        let (s, c) = sin_cos_f64(16_755.160_819_145_56 / rate);
        let a0 = ((1.0 - c) / (s * 0.125 + 2.0)) as f32;
        let k = 1.0 / (s + 8.0);
        let b1 = (c * -16.0 * k) as f32;
        let b2 = ((8.0 - s) * k) as f32;
        self.down = [flush(a0), flush(b1), flush(b2)];
        self.mix = self.knobs.mix_to;
        self.lfo_phase = 0.0;
        self.down_state = [[0.0; 4]; 2];
        self.dc.clear();
    }

    /// `FUN_180031130`: a voice ended, made over twice (once in
    /// `FUN_180024db0` and once after it).
    fn kill_voice(&mut self, i: usize) {
        self.reset_voice(i);
        self.reset_voice(i);
    }

    /// `FUN_180031070`: a voice made over, its phases drawn anew.
    fn reset_voice(&mut self, i: usize) {
        let v = &mut self.voices[i];
        v.key = -1;
        v.drift_count = 0;
        v.time = 0.0;
        v.stage = [0.0; 3];
        v.y = 0.0;
        v.y_last = 0.0;
        v.feedback = 0.0;
        v.cut = 0.0;
        v.cut_to = 0.0;
        v.res = 0.0;
        v.amp_to = 0.0;
        v.amp = 0.0;
        v.amp_rate = 0.0;
        v.fenv_to = 0.0;
        v.fenv = 0.0;
        v.fenv_rate = 0.0;
        v.gain = [0.0; 2];
        v.gain_to = [0.0; 2];
        v.inc = 0;
        v.sub_inc = 0;
        v.sync_inc = 0;
        let a = step2(self.rng);
        v.phase = a & 0xff_ffff;
        let b = step2(a);
        v.sub_phase = b & 0xff_ffff;
        self.rng = step2(b);
        v.sync_phase = self.rng & 0xff_ffff;
    }

    /// `FUN_180023fa0`: the master level and pan for one side.
    fn master(&self, right: bool) -> f32 {
        let k = &self.knobs;
        let g = k.level * 1.0 * 0.5;
        // The host's pan (`0x1290`) is the centre: its terms add to 0.
        let host = 0.5f32;
        let p = (((host - 0.5) + host) - 0.5) + (k.pan + k.pan);
        let p = p.clamp(0.0, 2.0);
        if g <= 0.0 {
            return 0.0;
        }
        if right {
            if p < 1.0 { g * p } else { g }
        } else if 1.0 < p {
            g * (2.0 - p)
        } else {
            g
        }
    }

    fn all_keys_off(&mut self) {
        for key in 0..128 {
            self.key_off(key);
        }
    }

    /// `FUN_180024e10`: a voice's pitch in semitones.
    fn pitch_of(&self, v: &Voice) -> f32 {
        let k = &self.knobs;
        let octave = if k.octaver && v.unison & 1 != 0 {
            12.0
        } else {
            0.0
        };
        let mut spread = 0.0;
        if v.count > 1 {
            spread =
                (v.unison as i32 as f32 * k.detune) / (v.count as i32 - 1) as f32 - k.detune * 0.5;
        }
        let key = self.key_pitch.get(v.key as usize).copied().unwrap_or(0.0);
        // The pitch wheel (`0x1468`) and the voice's arpeggio offset (`+0x28`)
        // read 0: FL's piano roll moves neither.
        ((((self.pitch_offset() + v.note) + (spread + key)) + octave) + v.random)
            + k.bend * 0.0
            + 0.0
    }

    /// `FUN_180024eb0`: a voice's gain on one side.
    fn side(&self, v: &Voice, right: bool) -> f32 {
        let mut f = 0.0;
        if v.count > 1 {
            f = v.unison as i32 as f32 / (v.count as i32 - 1) as f32;
            f = (f + f) - 1.0;
        }
        f *= self.knobs.unison_pan;
        let key = (v.key as usize).min(127);
        if right {
            (1.0 - f).clamp(0.0, 1.0) * self.key_gain[1][key] * 0.5
        } else {
            (f + 1.0).clamp(0.0, 1.0) * self.key_gain[0][key] * 0.5
        }
    }

    /// `FUN_180023370` with a velocity: unison voices for the key.
    fn key_on(&mut self, key: i32, velocity: f32) {
        self.held[key as usize] = (velocity * 127.0) as i32 as u8;
        if self.knobs.polyphony == 0 {
            return;
        }
        if self.knobs.mono {
            self.mono_on(key, velocity);
            return;
        }
        self.rng2 = step(self.rng2);
        let random = unit(self.rng2) - 1.0;
        let mut from = key as f32;
        if 0.0 <= self.last_key as f32 {
            from = self.last_key as f32;
        }
        if self.knobs.glide_mode == 1 || self.knobs.glide_mode == 3 {
            from = key as f32;
            let mut best = 0x80;
            for k in 0..128 {
                if self.held[k] != 0 && k as i32 != key && (key - k as i32).abs() < best {
                    best = (key - k as i32).abs();
                    from = k as f32;
                }
            }
        }
        let limit = self.knobs.polyphony as usize;
        for u in 0..self.knobs.unison {
            let mut taken = None;
            let (mut oldest, mut oldest_free) = (0, 0);
            for i in 0..limit.min(VOICES) {
                let v = &self.voices[i];
                if v.key < 0 {
                    taken = Some(i);
                    break;
                }
                if !v.released {
                    if v.age < self.voices[oldest].age {
                        oldest = i;
                    }
                } else if v.age < self.voices[oldest_free].age {
                    oldest_free = i;
                }
            }
            let i = taken.unwrap_or(if self.voices[oldest_free].released {
                oldest_free
            } else {
                oldest
            });
            self.start_voice(i, key, u, from, random, velocity);
        }
        self.last_key = key;
    }

    /// The fields `FUN_180023370` sets on a voice it takes for a key.
    fn start_voice(&mut self, i: usize, key: i32, u: i32, from: f32, random: f32, velocity: f32) {
        let k = self.knobs;
        let rate = self.rate;
        let age = self.ages;
        self.ages = self.ages.wrapping_add(1);
        let v = &mut self.voices[i];
        v.age = age;
        v.key = key;
        v.unison = u as i8;
        v.count = k.unison as i8;
        v.random = (random + random) * 0.0;
        v.velocity = powf(velocity, 1.0);
        v.note = from;
        v.released = false;
        if f64::from(k.glide) < 0.001 {
            v.note = key as f32;
        }
        v.glide_rate = glide_rate(&k, key, v.note, rate);
        self.begin(i);
    }

    /// `FUN_180030ea0`: a voice's envelopes start.
    fn begin(&mut self, i: usize) {
        let k = self.knobs;
        if k.lfo_retrig {
            self.lfo_phase = 0.0;
        }
        let v = &mut self.voices[i];
        v.time = 0.0;
        v.let_go = -1.0;
        let small = f32::from_bits(0x3a83_126f);
        v.attack = if k.attack < 0.001 { small } else { k.attack };
        v.decay = if k.decay < 0.001 { small } else { k.decay };
        v.sustain = k.sustain;
        v.f_attack = if k.egf[0] < 0.001 { small } else { k.egf[0] };
        v.f_decay = if k.egf[1] < 0.001 { small } else { k.egf[1] };
        v.f_sustain = k.egf[2];
        if k.unison_retrig {
            v.sync_phase = 0;
            v.phase = 0;
        }
        if k.sub_retrig {
            v.sub_phase = (v.phase as f32 * k.sub_ratio) as i64 as u32;
        }
        v.fenv = 0.0;
    }

    /// The release of every voice on the key (`FUN_180023370` with no
    /// velocity, `FUN_180030ff0`).
    fn key_off(&mut self, key: i32) {
        self.held[key as usize] = 0;
        if self.knobs.mono {
            self.mono_off(key);
            return;
        }
        for i in 0..VOICES {
            if self.voices[i].key == key {
                self.voices[i].released = true;
                self.release_voice(i);
            }
        }
    }

    fn release_voice(&mut self, i: usize) {
        let k = &self.knobs;
        let v = &mut self.voices[i];
        let small = f32::from_bits(0x3c23_d70a);
        v.release = if f64::from(k.release) < 0.01 {
            small
        } else {
            k.release
        };
        v.f_release = if f64::from(k.egf[3]) < 0.01 {
            small
        } else {
            k.egf[3]
        };
        v.let_go = 0.0;
        v.released = true;
    }

    /// `FUN_180023960`: the unison voices at the head of the list move to
    /// the newest key, and start again only if they were let go of.
    fn mono_on(&mut self, key: i32, velocity: f32) {
        self.rng2 = step(self.rng2);
        let random = unit(self.rng2) - 1.0;
        let mut from = key as f32;
        if 0.0 <= self.last_key as f32 {
            from = self.last_key as f32;
        }
        let head = self.voices[0];
        if (self.knobs.glide_mode == 1 || self.knobs.glide_mode == 3)
            && (head.key < 0 || head.released)
        {
            from = key as f32;
        }
        let k = self.knobs;
        for u in 0..(k.unison.max(0) as usize).min(VOICES) {
            let was = self.voices[u];
            let age = self.ages;
            self.ages = self.ages.wrapping_add(1);
            let rate = self.rate;
            let v = &mut self.voices[u];
            v.age = age;
            v.key = key;
            v.random = (random + random) * 0.0;
            v.released = false;
            v.unison = u as i8;
            v.count = k.unison as i8;
            v.glide_rate = glide_rate(&k, key, v.note, rate);
            if was.key < 0 || was.released {
                v.velocity = powf(velocity, 1.0);
                v.note = from;
                if f64::from(k.glide) < 0.001 {
                    v.note = key as f32;
                }
                self.begin(u);
            }
        }
        self.last_key = key;
    }

    fn mono_off(&mut self, key: i32) {
        let mut best = 0x80;
        let mut to = None;
        for k in 0..128 {
            if self.held[k] != 0 && (key - k as i32).abs() < best {
                best = (key - k as i32).abs();
                to = Some(k as i32);
            }
        }
        for i in 0..VOICES {
            if self.voices[i].key != key {
                continue;
            }
            match to {
                Some(k) => {
                    let age = self.ages;
                    self.ages = self.ages.wrapping_add(1);
                    let v = &mut self.voices[i];
                    v.age = age;
                    v.key = k;
                    v.glide_rate = (12.0 / (self.rate * f64::from(self.knobs.glide))) as f32;
                    self.last_key = k;
                }
                None => {
                    self.voices[i].released = true;
                    self.release_voice(i);
                }
            }
        }
    }

    /// `FUN_1800305d0`: a voice's rates for the next `n` base samples.
    /// False when the voice ends.
    fn update(&mut self, i: usize, n: i32) -> bool {
        let k = self.knobs;
        let lfo_value = self.lfo_value;
        let rate = self.rate;
        // `FUN_180024d10`: the glide.
        {
            let v = &mut self.voices[i];
            let to = v.key as f32;
            let mut c = v.note;
            let mut done = false;
            if c < to {
                c += n as f32 * v.glide_rate;
                v.note = c;
                if to < c {
                    v.note = to;
                    done = true;
                }
            }
            if !done && to < c {
                c -= n as f32 * v.glide_rate;
                v.note = c;
                if c < to {
                    v.note = to;
                }
            }
        }
        let v = self.voices[i];
        let t = v.time;
        let amt = k.lfo_amount;
        let mut lfo = ((t / f64::from(k.lfo_attack)) as f32).min(1.0) * lfo_value;
        if v.let_go >= 0.0 {
            let r = ((v.let_go / f64::from(k.lfo_release)) as f32).min(1.0);
            lfo *= 1.0 - r;
        }
        let w = lfo * amt;
        let mut extra = 0.0f32;
        let x8 = if k.lfo_dest == 0 {
            let x = w * amt;
            extra = x + x;
            x
        } else {
            amt * w
        };
        self.rng = step(self.rng);
        let mut v = self.voices[i];
        if v.drift_count == 0 {
            let r = self.rng as f32 * f32::from_bits(0x2f80_0000);
            v.drift_to = ((r + r) - 1.0) * f32::from_bits(0x3c23_d70a);
            v.drift_count = 1;
        } else {
            v.drift_count += 1;
            if v.drift_count > 0x400 {
                v.drift_count = 0;
            }
        }
        let prev = if t == 0.0 { 0.0 } else { v.drift };
        let fenv_now = v.fenv;
        let drift = (f64::from(v.drift_to - prev) * 0.0001 + f64::from(prev)) as f32;
        v.drift = drift;
        let egf = k.egf_amount;
        self.voices[i] = v;
        let mut gl = self.side(&v, false);
        let mut gr = self.side(&v, true);
        let h = lfo * 0.5;
        if k.lfo_dest == 5 {
            let g = powf(10.0, -((h + 0.5) * amt));
            gr *= g;
            gl *= g;
        }
        if k.lfo_dest == 6 {
            let h2 = h * amt;
            gl *= 0.5 - h2;
            gr *= h2 + 0.5;
        }
        let mut v = self.voices[i];
        v.gain_to = [gl, gr];
        let mut noise = k.noise;
        if k.lfo_dest == 4 {
            noise += x8;
        }
        let noise = noise.clamp(0.0, 1.0);
        v.noise = noise * noise * f32::from_bits(0x3f33_3333);
        let mut sub = k.sub_phase;
        if k.egf_dest == 2 {
            sub += egf * fenv_now;
        }
        if k.lfo_dest == 2 {
            sub += w;
        }
        let sub = sub.clamp(0.0, 1.0);
        v.sub_offset = (f64::from(sub) * 8_371_830.285 + 16_777.215) as i64 as u32;
        let mut sync = 0.0f64;
        if k.sync_on {
            sync = f64::from(k.sync);
            if k.egf_dest == 1 {
                sync = (sync + f64::from(egf * fenv_now) * 4.0).clamp(0.0, 4.0);
            }
            if k.lfo_dest == 1 {
                sync = (sync + f64::from(amt * amt * lfo) * 4.0).clamp(0.0, 4.0);
            }
        }
        self.voices[i] = v;
        let pitch = self.pitch_of(&v);
        let hz =
            (2f64.powf(f64::from((pitch - 57.0) * f32::from_bits(0x3daa_aaab))) * 220.0) as f32;
        let mut f = f64::from(hz) * 2f64.powf(f64::from(drift) + sync);
        if k.egf_dest == 3 {
            let semis = (egf * 24.0) as i32 as f32 * fenv_now * f32::from_bits(0x3daa_aaab);
            f *= f64::from(powf(2.0, semis));
        }
        if k.lfo_dest == 3 {
            f *= f64::from(powf(2.0, x8 * amt * amt));
        }
        let inv = 1.0 / (rate * 6.0);
        let v = &mut self.voices[i];
        v.inc = ((inv * f).clamp(0.0, 0.49) * 16_777_215.0) as i64 as u32;
        let sub_f = (f64::from(k.sub_detune * 3.0) + f64::from(k.sub_ratio) * f) * inv;
        v.sub_inc = (sub_f.clamp(0.0, 0.49) * 16_777_215.0) as i64 as u32;
        if k.sync_on {
            let s = 2f64.powf(-sync) * f * inv;
            v.sync_inc = (s.clamp(0.0, 0.49) * 16_777_215.0) as i64 as u32;
        } else {
            v.sync_phase = 0;
            v.sync_inc = 0;
        }
        let v = self.voices[i];
        let keyed = (self.pitch_of(&v) - 36.0) * k.key_track * f32::from_bits(0x3daa_aaab);
        let mut c = f64::from(keyed) + (f64::from(k.cutoff) * 9.3 + 5.0);
        let vel = (v.velocity * 8.0 - 4.0) * k.vel_track + extra + v.drift;
        c += f64::from(vel);
        let fc = 2f64.powf(c) as f32;
        let v = &mut self.voices[i];
        v.cut_to = ((1.0 / 3.0 / (rate * 6.0)) * f64::from(fc * 6.0)) as f32;
        if v.time == 0.0 {
            v.cut = v.cut_to;
        }
        let mut res = (f64::from(k.resonance) * 3.99) as f32;
        if f64::from(res) > 3.99 {
            res = f32::from_bits(0x407f_5c28);
        }
        v.res = res;
        v.fenv_cut = if k.egf_dest == 0 {
            egf * 320.0 * egf * (egf * egf)
        } else {
            0.0
        };
        // The envelopes.
        let dt = f64::from(n) * 2.267_573_696_145_124_8e-5;
        let sr = rate as f32 as f64;
        let rel = v.let_go;
        let coef;
        let mut target = 0.0f32;
        if rel > -1.0 {
            let rt = f64::from(v.release);
            if rel > rt {
                v.gain_to = [0.0; 2];
                if rel > rt + 0.1 {
                    return false;
                }
            }
            v.let_go = rel + dt;
            coef = 8.82 / (rt * sr);
        } else if v.time < f64::from(v.attack) {
            target = v.velocity * 5.0;
            coef = 8.82 / (f64::from(v.attack) * 39.441_975 * sr);
        } else {
            target = v.velocity * v.sustain;
            coef = 8.82 / (f64::from(v.decay) * sr);
        }
        v.amp_to = target;
        v.amp_rate = coef as f32;
        if v.amp_to < f32::from_bits(0x33d6_bf95) {
            v.amp_to = f32::from_bits(0x33d6_bf95);
        }
        let rel = v.let_go;
        let c;
        if rel > -1.0 {
            c = f64::from(v.f_release);
            v.fenv_to = 0.0;
            if rel > c {
                v.fenv_rate = 0.0;
                return true;
            }
        } else if v.time < f64::from(v.f_attack) {
            v.fenv_to = 5.0;
            c = f64::from(v.f_attack) * 39.441_975;
        } else {
            c = f64::from(v.f_decay);
            v.fenv_to = v.f_sustain;
        }
        v.fenv_rate = (8.82 / (c * sr)) as f32;
        if v.fenv_to < f32::from_bits(0x33d6_bf95) {
            v.fenv_to = f32::from_bits(0x33d6_bf95);
        }
        v.time += dt;
        true
    }

    /// `FUN_180036710`: one voice for `n` oversamples, put into (`first`)
    /// or added to the oversampled sides.
    fn run_one(&mut self, i: usize, at: usize, n: usize, first: bool) {
        let saw = match &self.saw {
            Some(saw) => saw.clone(),
            None => return,
        };
        let sub_gain = self.knobs.sub_invert * self.knobs.sub_level;
        let sub_table = if self.knobs.sub_square { SAW } else { 0 };
        let k36 = f32::from_bits(0x4330_6667) / self.rate as f32;
        let cut_max = self.cut_max;
        let mut v = self.voices[i];
        let mut c = ((v.fenv_cut * v.fenv + 1.0) * v.cut).min(cut_max);
        let r = self.rng;
        self.rng = step(r);
        let mut noise = (unit(r) - 0.5) * v.noise;
        let mut jitter = 0.0f32;
        let [over_l, over_r] = &mut self.over;
        for j in 0..n {
            if self.sub_step == 0 {
                let r = self.rng;
                self.rng = step(r);
                self.sub_step = 1;
                c = ((v.fenv_cut * v.fenv + 1.0) * v.cut).min(cut_max);
                for m in 0..4 {
                    self.mix[m] = (self.knobs.mix_to[m] - self.mix[m]) * k36 + self.mix[m];
                }
                let x = unit(r) - 0.5;
                noise = x * v.noise;
                jitter = x * f32::from_bits(0x3d75_c28f);
                v.cut += (v.cut_to - v.cut) * k36;
                v.fenv = ((v.fenv_to - v.fenv) * v.fenv_rate + v.fenv).min(1.0);
                v.amp = ((v.amp_to - v.amp) * v.amp_rate + v.amp).min(1.0);
                v.gain[1] += (v.gain_to[1] - v.gain[1]) * k36;
                v.gain[0] += (v.gain_to[0] - v.gain[0]) * k36;
            } else {
                self.sub_step += 1;
                if self.sub_step == 6 {
                    self.sub_step = 0;
                }
            }
            let main = (v.phase >> 11) as usize & (SAW - 1);
            let sub = (v.sub_offset.wrapping_add(v.sub_phase) >> 11) as usize & (SAW - 1);
            let mut x = saw[sub_table + sub] * sub_gain;
            let sync = v.sync_inc.wrapping_add(v.sync_phase);
            let wrapped = sync & 0xff00_0000 != 0;
            v.sync_phase = if wrapped { 0 } else { sync };
            let p = if wrapped { 0 } else { v.phase }.wrapping_add(v.inc);
            v.phase = if p & 0xff00_0000 == 0 { p } else { 0 };
            let s = if wrapped { 0 } else { v.sub_phase }.wrapping_add(v.sub_inc);
            v.sub_phase = if s & 0xff00_0000 == 0 { s } else { 0 };
            x += saw[main];
            x *= jitter + 1.0;
            let x = (f64::from(x) - 0.35) as f32;
            let x = (x + noise) * v.amp;
            let out = ladder(&mut v, x, c, false);
            let m = &self.mix;
            let y = out[0];
            let o =
                ((out[1] - y) * m[3] + (y - v.stage[2]) * m[2]) + (y * m[0] + v.stage[1] * m[1]);
            let (l, r) = (o * v.gain[0], o * v.gain[1]);
            if first {
                over_l[at + j] = l;
                over_r[at + j] = r;
            } else {
                over_l[at + j] += l;
                over_r[at + j] += r;
            }
        }
        self.voices[i] = v;
    }

    /// `FUN_180035160`: four voices at once, as the SSE code adds them.
    fn run_four(&mut self, lanes: [usize; 4], at: usize, n: usize, first: bool) {
        let saw = match &self.saw {
            Some(saw) => saw.clone(),
            None => return,
        };
        let sub_gain = self.knobs.sub_invert * self.knobs.sub_level;
        let sub_table = if self.knobs.sub_square { SAW } else { 0 };
        let k79 = (176.400_008_378_550_4 / self.rate) as f32;
        let cut_max = self.cut_max;
        let mut v = lanes.map(|i| self.voices[i]);
        let mut c = [0.0f32; 4];
        for l in 0..4 {
            c[l] = ((v[l].fenv_cut * v[l].fenv + 1.0) * v[l].cut).min(cut_max);
        }
        let mut noise = [0.0f32; 4];
        let mut jitter = [0.0f32; 4];
        let mut r = self.rng;
        for l in 0..4 {
            let x = unit(r) - 0.5;
            r = step(r);
            jitter[l] = f32::from_bits(0x3d75_c28f) * x;
            noise[l] = x * v[l].noise;
        }
        self.rng = r;
        let mut mix = self.mix;
        let [over_l, over_r] = &mut self.over;
        for j in 0..n {
            if self.sub_step == 0 {
                mix = self.mix;
                for ((now, to), was) in self.mix.iter_mut().zip(self.knobs.mix_to).zip(mix) {
                    *now = (to - was) * k79 + was;
                }
                let mut r = self.rng;
                for l in 0..4 {
                    let x = unit(r) - 0.5;
                    r = step(r);
                    jitter[l] = f32::from_bits(0x3d75_c28f) * x;
                    noise[l] = x * v[l].noise;
                }
                for l in 0..4 {
                    let w = &mut v[l];
                    c[l] = ((w.fenv_cut * w.fenv + 1.0) * w.cut).min(cut_max);
                    w.cut += (w.cut_to - w.cut) * k79;
                    let f = w.fenv + (w.fenv_to - w.fenv) * w.fenv_rate;
                    w.fenv = if 1.0 < f { 1.0 } else { f };
                    let a = w.amp + (w.amp_to - w.amp) * w.amp_rate;
                    w.amp = if 1.0 < a { 1.0 } else { a };
                }
                self.rng = r;
                self.sub_step = 1;
                for w in &mut v {
                    w.gain[0] += (w.gain_to[0] - w.gain[0]) * k79;
                }
                for w in &mut v {
                    w.gain[1] += (w.gain_to[1] - w.gain[1]) * k79;
                }
            } else {
                self.sub_step += 1;
                if self.sub_step == 6 {
                    self.sub_step = 0;
                }
            }
            let mut o = [0.0f32; 4];
            for l in 0..4 {
                let w = &mut v[l];
                let main = (w.phase >> 11) as usize & (SAW - 1);
                let sub = (w.sub_offset.wrapping_add(w.sub_phase) >> 11) as usize & (SAW - 1);
                let x = saw[sub_table + sub] * sub_gain + saw[main];
                let sync = w.sync_inc.wrapping_add(w.sync_phase);
                let wrapped = sync & 0xff00_0000 != 0;
                w.sync_phase = if wrapped { 0 } else { sync };
                let p = if wrapped { 0 } else { w.phase }.wrapping_add(w.inc);
                w.phase = if p & 0xff00_0000 == 0 { p } else { 0 };
                let s = if wrapped { 0 } else { w.sub_phase }.wrapping_add(w.sub_inc);
                w.sub_phase = if s & 0xff00_0000 == 0 { s } else { 0 };
                let x = (((x * jitter[l] + x) + f32::from_bits(0xbeb3_3333)) + noise[l]) * w.amp;
                let out = ladder(w, x, c[l], true);
                let y = out[0];
                o[l] = ((y * mix[0] + w.stage[1] * mix[1]) + (y - w.stage[2]) * mix[2])
                    + (out[1] - y) * mix[3];
            }
            let l = ((o[1] * v[1].gain[0] + o[0] * v[0].gain[0]) + o[2] * v[2].gain[0])
                + o[3] * v[3].gain[0];
            let r = ((o[1] * v[1].gain[1] + o[0] * v[0].gain[1]) + o[2] * v[2].gain[1])
                + o[3] * v[3].gain[1];
            if first {
                over_l[at + j] = l;
                over_r[at + j] = r;
            } else {
                over_l[at + j] += l;
                over_r[at + j] += r;
            }
        }
        for (l, i) in lanes.iter().enumerate() {
            self.voices[*i] = v[l];
        }
    }

    /// `Gen_Render` (`0x180034ba0`) for up to [`BLOCK`] frames.
    fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len();
        let k = self.knobs;
        // The LFO, once a block.
        let ph = self.lfo_phase;
        let mut value = match k.lfo_shape {
            0 => {
                let mut f = (ph + ph) as f32;
                if 1.0 < f {
                    f = 2.0 - f;
                }
                (f + f) - 1.0
            }
            1 => ((ph + ph) - 1.0) as f32,
            2 => {
                if 0.5 <= ph {
                    -1.0
                } else {
                    1.0
                }
            }
            _ => self.lfo_held,
        };
        let speed = if !k.lfo_sync {
            (1.0 / self.rate) * k.lfo_speed
        } else {
            self.tempo * (0.066_666_666_666_666_67 / f64::from(k.lfo_division)) / self.rate
        };
        let ph = speed * n as f64 + ph;
        self.lfo_phase = ph;
        if 1.0 < ph {
            self.lfo_phase = ph - 1.0;
            let r = unit(self.rng);
            self.lfo_held = (r + r) - 1.0;
        }
        if k.lfo_invert {
            value = -value;
        }
        self.lfo_value = value;
        // The voices in the engine's order: sounding ones under the
        // polyphony first.
        let mut order = [0usize; VOICES];
        let mut count = 0;
        let limit = k.polyphony.max(0) as usize;
        let mut rest = [0usize; VOICES];
        let mut rest_n = 0;
        let mut idle = [0usize; VOICES];
        let mut idle_n = 0;
        let mut seen = 0;
        for i in 0..VOICES {
            if self.voices[i].key < 0 {
                idle[idle_n] = i;
                idle_n += 1;
            } else if seen < limit {
                order[count] = i;
                count += 1;
                seen += 1;
            } else {
                rest[rest_n] = i;
                rest_n += 1;
            }
        }
        let active = count;
        order[count..count + rest_n].copy_from_slice(&rest[..rest_n]);
        count += rest_n;
        order[count..count + idle_n].copy_from_slice(&idle[..idle_n]);
        let samples = n * OVER;
        let chunk = self.chunk.max(1) as usize;
        if active > 0 {
            let mut left_over = active as i32;
            let mut g = 0;
            let mut first = true;
            while left_over > 0 {
                if left_over == 1 {
                    let i = order[g];
                    let mut done = 0;
                    while done < n {
                        let m = chunk.min(n - done);
                        if !self.update(i, m as i32) {
                            self.kill_voice(i);
                        }
                        self.run_one(i, done * OVER, m * OVER, first);
                        done += m;
                    }
                    break;
                }
                let lanes = [order[g], order[g + 1], order[g + 2], order[g + 3]];
                let mut done = 0;
                while done < n {
                    let m = chunk.min(n - done);
                    for i in lanes {
                        if self.voices[i].key >= 0 && !self.update(i, m as i32) {
                            self.kill_voice(i);
                        }
                    }
                    self.run_four(lanes, done * OVER, m * OVER, first);
                    done += m;
                }
                g += 4;
                first = false;
                left_over -= 4;
            }
            // `FUN_180036f50`: down to the host's rate.
            let [a0, b1, b2] = self.down;
            for (side, out) in [(0usize, &mut *left), (1, &mut *right)] {
                let over = &self.over[side][..samples];
                let s = &mut self.down_state[side];
                for (frame, i) in out.iter_mut().zip(over.chunks_exact(OVER)) {
                    let (a, b) = (s[0], s[1]);
                    // The engine's sums in pairs, as its unrolled loop adds
                    // them; the right side's are the left's turned about.
                    let sums = [
                        (b + b) + (i[0] + a),
                        (i[1] + b) + (i[0] + i[0]),
                        (i[1] + i[1]) + (i[0] + i[2]),
                        (i[3] + i[1]) + (i[2] + i[2]),
                        (i[3] + i[3]) + (i[2] + i[4]),
                        (i[4] + i[4]) + (i[5] + i[3]),
                    ];
                    let (mut y2, mut y1) = (s[2], s[3]);
                    for sum in sums {
                        let y = (sum * a0 - y1 * b1) - b2 * y2;
                        y2 = y1;
                        y1 = y;
                    }
                    *s = [i[4], i[5], y2, y1];
                    *frame = y1;
                }
            }
        } else {
            self.sub_step = 0;
            left.fill(0.0);
            right.fill(0.0);
        }
        // The DC highpass, one filter for both sides in turn.
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            *l = self.dc.run(*l);
            *r = self.dc.run(*r);
        }
        // `FUN_180023220`: the master level, moving 0.5% a sample.
        let to = [self.master(false), self.master(true)];
        let k5 = f32::from_bits(0x3ba3_d70a);
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            *l *= self.gains[0];
            *r *= self.gains[1];
            self.gains[0] = (to[0] - self.gains[0]) * k5 + self.gains[0];
            self.gains[1] = (to[1] - self.gains[1]) * k5 + self.gains[1];
        }
    }
}

/// The glide's rate (`FUN_180023370`): a semitone in `12 / rate / time`
/// for the time modes, the distance in that time for the others.
fn glide_rate(k: &Knobs, key: i32, note: f32, rate: f64) -> f32 {
    if k.glide_mode <= 1 {
        (12.0 / (rate * f64::from(k.glide))) as f32
    } else {
        (f64::from((key as f32 - note).abs()) / (rate * f64::from(k.glide))) as f32
    }
}

/// The ladder, three passes with the last stage's feedback saturated:
/// the output and the input after the last pass.
fn ladder(v: &mut Voice, x: f32, c: f32, _lanes: bool) -> [f32; 2] {
    let sat = |y: f32| {
        let p = y.max(0.0);
        let q = p * p;
        y / (q * q * 5.0 + 1.0)
    };
    let res = v.res;
    let [mut s1, mut s2, mut s3] = v.stage;
    s1 += ((x - v.feedback * res) - s1) * c;
    s2 += (s1 - s2) * c;
    s3 += (s2 - s3) * c;
    let y1 = sat((s3 - v.y) * c + v.y);
    s1 += ((x - (v.y_last * 0.5 + y1 * 0.5) * res) - s1) * c;
    s2 += (s1 - s2) * c;
    s3 += (s2 - s3) * c;
    let y2 = sat((s3 - y1) * c + y1);
    let half = y2 * 0.5;
    let x = x - (y1 * 0.5 + half) * res;
    s1 += (x - s1) * c;
    s2 += (s1 - s2) * c;
    s3 += (s2 - s3) * c;
    let y3 = sat((s3 - y2) * c + y2);
    v.stage = [s1, s2, s3];
    v.y = y3;
    v.y_last = y3;
    v.feedback = y3 * 0.5 + half;
    [y3, x]
}

impl Generator for Sawer {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        if bytes.len() != STATE_LEN {
            return Err(StateError::Length {
                expected: STATE_LEN,
                found: bytes.len(),
            });
        }
        if &bytes[..4] != b"SSLF" || &bytes[8..12] != b"1rwS" {
            return Err(StateError::Version {
                found: i32::from_le_bytes(bytes[8..12].try_into().unwrap()),
            });
        }
        for n in 0..PARAMS {
            let at = 12 + 4 * n;
            let v = f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
            self.set(n, v);
        }
        self.extra
            .copy_from_slice(&bytes[12 + 4 * PARAMS..12 + 4 * PARAMS + 96]);
        self.program.copy_from_slice(&bytes[STATE_LEN - 128..]);
        for n in 0x4e..PARAMS {
            self.set(n, self.params[n]);
        }
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        if hz > 0.0 && f64::from(hz) != self.rate {
            self.set_rate(f64::from(hz));
        }
    }

    fn set_tempo(&mut self, bpm: f64) {
        if bpm > 0.0 {
            self.tempo = bpm;
        }
    }

    fn set_resource_dir(&mut self, dir: &Path) {
        self.looked = true;
        self.saw = read_saw(&dir.join(DATA));
    }

    fn note_on(&mut self, note: NoteOn) {
        if note.slide {
            return;
        }
        let key = note.key.clamp(0, 127);
        let slot = key as usize;
        if self.owner[slot].is_some_and(|owner| owner != note.voice) {
            self.key_off(i32::from(key));
        }
        self.owner[slot] = Some(note.voice);
        let (left, right) = crate::sampler::circular(note.pan, 1.0);
        self.key_gain[0][slot] = left;
        self.key_gain[1][slot] = right;
        self.key_pitch[slot] = note.fine_pitch * 0.01;
        let level = crate::bassdrum::velocity_level(note.velocity);
        self.key_velocity[slot] = level;
        if level > 0.0 {
            // `FUN_180037c50`: the matrix reads the velocity first.
            self.velocity_source = level;
            for s in 0..6 {
                let d = self.knobs.matrix_dest[s];
                if self.knobs.matrix_source[s] != 0 && d >= 0 {
                    self.set(d as usize, self.params[d as usize]);
                }
            }
            self.key_on(i32::from(key), level);
        } else {
            self.key_off(i32::from(key));
        }
    }

    fn note_off(&mut self, voice: u32) {
        if let Some(key) = self.owner.iter().position(|o| *o == Some(voice)) {
            self.owner[key] = None;
            self.key_off(key as i32);
        }
    }

    fn all_notes_off(&mut self) {
        for key in 0..128 {
            if self.owner[key].take().is_some() {
                self.key_off(key as i32);
            }
        }
    }

    fn set_note_pitch(&mut self, voice: u32, pitch: f32) {
        if let Some(key) = self.owner.iter().position(|o| *o == Some(voice)) {
            self.key_pitch[key] = pitch * 0.01 - key as f32 + 60.0;
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let frames = left.len().min(right.len());
        let mut done = 0;
        let (mut l, mut r) = ([0.0f32; BLOCK], [0.0f32; BLOCK]);
        while done < frames {
            let n = (frames - done).min(BLOCK);
            self.render(&mut l[..n], &mut r[..n]);
            for i in 0..n {
                left[done + i] += l[i];
                right[done + i] += r[i];
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

    fn set_param(&mut self, index: usize, value: f32) {
        self.set(index, value);
    }

    fn reset(&mut self) {
        for i in 0..VOICES {
            self.reset_voice(i);
        }
        self.owner = [None; 128];
    }

    fn tail(&self) -> usize {
        let release = f64::from(self.knobs.release.max(self.knobs.egf[3])) + 0.2;
        (release * self.rate) as usize + 4096
    }

    fn fidelity(&self) -> Fidelity {
        let k = &self.knobs;
        if self.saw.is_none() {
            Fidelity::Approximate("Sawer data.saw not available")
        } else if k.chorus || k.phaser || k.delay || k.reverb {
            Fidelity::Approximate("Sawer's chorus, phaser, delay and reverb are not built")
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

    /// A synth with FL's `data.saw`, when the corpus names an install.
    fn synth() -> Option<Sawer> {
        let root = testing::corpus()?;
        let mut synth = Sawer::default();
        synth.set_resource_dir(&root.join("../Plugins/Fruity/Generators"));
        synth.saw.as_ref()?;
        Some(synth)
    }

    /// Events as the emulator harness takes them, `on:<key>:<frame>` and
    /// `off:<key>:<frame>`, each note's voice its key; blocks of 512 that
    /// start again at every event.
    fn render_events(synth: &mut Sawer, events: &str, frames: usize) -> [Vec<f32>; 2] {
        let mut list: Vec<(usize, bool, i16)> = events
            .split_whitespace()
            .map(|e| {
                let p: Vec<&str> = e.split(':').collect();
                (p[2].parse().unwrap(), p[0] == "on", p[1].parse().unwrap())
            })
            .collect();
        list.sort_by_key(|(at, on, _)| (*at, *on));
        let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
        let mut done = 0;
        let mut next = 0;
        while done < frames {
            while next < list.len() && list[next].0 <= done {
                let (_, on, key) = list[next];
                if on {
                    synth.note_on(note(key as u32, key));
                } else {
                    synth.note_off(key as u32);
                }
                next += 1;
            }
            let mut n = 512.min(frames - done);
            if next < list.len() {
                n = n.min(list[next].0 - done);
            }
            synth.process(&mut left[done..done + n], &mut right[done..done + n]);
            done += n;
        }
        [left, right]
    }

    #[test]
    fn the_render_test_plays_a_saw() {
        let Some(root) = testing::corpus() else {
            return;
        };
        let Some(mut synth) = synth() else {
            return;
        };
        let saved = testing::projects(&root.join("System/Render tests"));
        let ours: Vec<_> = saved.iter().filter(|s| s.plugin == NAME).collect();
        assert_eq!(ours.len(), 1);
        synth.set_state(&ours[0].state).unwrap();
        assert_eq!(synth.fidelity(), Fidelity::Exact);
        let [left, right] = render_events(&mut synth, "on:60:0 off:60:6000", 12_000);
        assert!(left.iter().chain(&right).all(|v| v.is_finite()));
        let peak = left.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((0.05..2.0).contains(&peak), "{peak}");
    }

    /// Bit for bit what `engine_x64.dll` plays for the render test's state
    /// under the harness in `.scratch/notes/sawer-emu` (built, polyphony 0,
    /// 44.1 kHz, the state; key 60 for 4000 frames in blocks of 64, let go
    /// at 3000): every sample's bits folded together, and three of them.
    #[test]
    fn plays_the_render_test_as_the_engine_dll_does() {
        let Some(root) = testing::corpus() else {
            return;
        };
        let Some(mut synth) = synth() else {
            return;
        };
        let saved = testing::projects(&root.join("System/Render tests"));
        let ours: Vec<_> = saved.iter().filter(|s| s.plugin == NAME).collect();
        synth.set_state(&ours[0].state).unwrap();
        let [left, right] = render_events(&mut synth, "on:60:0 off:60:3000", 4000);
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for (l, r) in left.iter().zip(&right) {
            for bits in [l.to_bits(), r.to_bits()] {
                hash = (hash ^ u64::from(bits)).wrapping_mul(0x0100_0000_01b3);
            }
        }
        let at = |n: usize| [left[n].to_bits(), right[n].to_bits()];
        assert_eq!(at(500), [0xbbeb_d46a, 0xbbea_cd31]);
        assert_eq!(at(2000), [0xbe25_3495, 0xbe24_ffbc]);
        assert_eq!(at(3500), [0x3d2e_e0a3, 0x3d2e_8e81]);
        assert_eq!(hash, 0xbdf7_2710_af28_0828);
    }

    #[test]
    fn follows_note_pitch() {
        let Some(root) = testing::corpus() else {
            return;
        };
        let dir = root.join("../Plugins/Fruity/Generators");
        testing::follows_note_pitch(
            || {
                let mut synth = Sawer::default();
                synth.set_resource_dir(&dir);
                Box::new(synth)
            },
            44_100.0,
        );
    }

    /// Every Sawer state FL ships loads and plays a note that stays
    /// finite.
    #[test]
    fn every_shipped_state_loads_and_plays() {
        let Some(root) = testing::corpus() else {
            return;
        };
        let mut saved = testing::every_preset(&root, NAME);
        let ours = |saved: &testing::Saved| saved.plugin.eq_ignore_ascii_case(NAME);
        saved.extend(testing::projects(&root).into_iter().filter(ours));
        assert!(saved.len() > 400, "{}", saved.len());
        for saved in saved {
            let mut synth = synth().unwrap();
            if let Err(error) = synth.set_state(&saved.state) {
                panic!("{}: {error}", saved.file);
            }
            let [left, right] = render_events(
                &mut synth,
                "on:60:0 on:64:100 off:60:2048 off:64:2048",
                4096,
            );
            let all = || left.iter().chain(&right);
            assert!(all().all(|v| v.is_finite()), "{}", saved.file);
            let peak = all().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(peak < 8.0, "{peak} in {}", saved.file);
        }
    }

    /// Writes what the port plays for the harness in
    /// `.scratch/notes/sawer-emu` to compare with the DLL.
    #[test]
    fn dump_for_dll() {
        let Ok(path) = std::env::var("SAWER_STATE") else {
            return;
        };
        let frames: usize = std::env::var("SAWER_FRAMES").unwrap().parse().unwrap();
        let events = std::env::var("SAWER_EVENTS").unwrap();
        let mut synth = synth().unwrap();
        synth.set_sample_rate(44_100.0);
        synth.set_state(&std::fs::read(path).unwrap()).unwrap();
        let [left, right] = render_events(&mut synth, &events, frames);
        let out: String = left
            .iter()
            .zip(&right)
            .map(|(l, r)| format!("{l:?} {r:?}\n"))
            .collect();
        std::fs::write(std::env::var("SAWER_OUT").unwrap(), out).unwrap();
    }
}
