//! MiniSynth: FL Mobile's subtractive synth inside FL's wrapper for its
//! mobile plugins, as `docs/plugins/minisynth.md` reads it out of
//! `MiniSynth_x64.dll`.
//!
//! # The state
//!
//! `SSLF`, the length of what follows as a u32, then the synth's chunk:
//! the magic `99 'FLS' 0d 0a 1a 0a`, a `HEAD` block with the preset name in
//! UTF-16, and a `DATA` block of `0rev` and the 43 parameters as f32, each
//! from 0 to 1. A `DATA` block too short for all 43 leaves the rest as they
//! were, as FL's loader does.
//!
//! # Parameters
//!
//! FL moves parameter `i` with a whole number from 0 to 256, which the
//! wrapper divides by 256. [`Generator::param`] gives `int(256 v)` back.
//!
//! | # | name | what it sets |
//! |---|---|---|
//! | 0 | Voices Mode | `int(2 v)`: poly, mono, lead (mono legato) |
//! | 1 | Transpose | `int(24 (v - 0.5))` semitones |
//! | 2 | Slide Time | glide `0.9 (1 - v)^4 + 0.01` of the way a block |
//! | 3, 4 | MW Dest, MW Amount | the mod wheel, which FL never moves here |
//! | 5 | OSC Kind | `int(9 v)`: saw, saw+saw, pulse, sqr+saw, sqr+sqr, supersaw, bell, deaf saw, saw spread, saw spread 2 |
//! | 6 | OSC Mod | the kind's width, detune or spread |
//! | 7 | OSC Noise | `v^2` |
//! | 8 to 11 | FLT Freq, Peak, Kbd, Vel | the ladder's cutoff and feedback |
//! | 12 to 16 | EGF Atk, Dec, Sus, Rel, Amnt | the filter's envelope |
//! | 17 to 20 | LFO Shape, Dest, Rate, Amnt | sine, saw, square to filter, pitch or OSC Mod |
//! | 21 to 24 | AMP Atk, Dec, Sus, Rel | the amplitude's envelope |
//! | 25 to 27 | DIST, Overdrive, Decimator | the distortion |
//! | 28 to 32 | DELAY, Time, Fdbk, Filter, Mix | the tempo delay |
//! | 33 to 36 | CHORUS, ModRate, ModDepth, Mix | the chorus |
//! | 37 to 42 | PHASER, ModSpeed, ModWidth, Fdbk, Mix, Freq | the phaser |
//!
//! # FL's file
//!
//! The oscillator is 1024 band-limited tables of one wave, which the synth
//! adds up from the 1024 amplitudes and phases in
//! `Minisynth/oscdata.dat` beside the DLL. [`Generator::set_resource_dir`]
//! reads it. Without it the synth is silent and [`Generator::fidelity`]
//! says so. The tables are built once and shared.
//!
//! # A note
//!
//! The wrapper hands the synth MIDI: the key, and a velocity byte
//! `int(127 log21(10 v + 1))`, of which a 0 is a note off. A second note on
//! a key lets go of the first. The note's pan and velocity gain, by FL's
//! circular pan law, its fine pitch and its mod X and Y go to the key's
//! slot, which the voice playing the key reads as its gains and as offsets
//! to its pitch, cutoff and resonance.
//!
//! The synth renders in blocks of 32 frames: the LFO, each voice's pitch,
//! oscillator and filter are worked out once a block, the rest per sample.
//! It counts the song's beats from the start, as FL's song position, for
//! the LFO and the phaser, both of which follow it while FL plays.

use crate::{Fidelity, Generator, NoteOn, StateError};
use std::f64::consts::TAU;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

/// The name FL's projects give the plugin in event 201.
pub const NAME: &str = "MiniSynth";

/// The parameters the state holds and FL can move.
pub const PARAMS: usize = 43;

/// The file of the oscillator's harmonics, under the generators' directory.
pub const OSC_DATA: &str = "Minisynth/oscdata.dat";

/// FL's polyphony for the plugin.
const VOICES: usize = 99;
/// Frames the synth renders at a time.
const BLOCK: usize = 32;
/// Points in one table, and the tables there are.
const TABLE: usize = 2048;
const TABLES: usize = 1024;
const KEYS: usize = 128;

/// What `FUN_1800234b0` gives each parameter before a state comes.
pub const DEFAULTS: [f32; PARAMS] = {
    let mut d = [0.0f32; PARAMS];
    d[0] = 0.375;
    let halves = [1, 16, 20, 30, 32, 35, 36, 42];
    let mut i = 0;
    while i < halves.len() {
        d[halves[i]] = 0.5;
        i += 1;
    }
    let ones = [8, 13, 14, 15, 22, 23, 31, 41];
    i = 0;
    while i < ones.len() {
        d[ones[i]] = 1.0;
        i += 1;
    }
    d[19] = 0.25;
    d[34] = 0.25;
    d[24] = 0.07;
    d[29] = 0.7;
    d[38] = 0.112_222_22;
    d[39] = 0.4;
    d[40] = 0.1;
    d
};

/// The LFO's and the phaser's periods in beats, by `int(11 v)`.
const RATES: [f64; 12] = [
    0.25, 0.375, 0.5, 0.75, 1.0, 2.0, 4.0, 8.0, 16.0, 64.0, 128.0, 256.0,
];

/// The two generators that run on the synth's one global seed.
fn seed_step(x: u32) -> u32 {
    x.wrapping_mul(0xb464_b2f9).wrapping_add(0x9d6f_2492)
}

fn noise_step(x: u32) -> u32 {
    x.wrapping_mul(0x0bb3_8435).wrapping_add(0x3619_636b)
}

/// `FUN_1800229c0`: an envelope's step towards its target, per sample.
fn env_rate(v: f64, attack: bool) -> f32 {
    if v >= 0.001_000_000_047_497_451_3 {
        let x = v * 0.95 + 0.05;
        let d = x * x * x * x * if attack { 500.0 } else { 50.0 };
        if d >= 0.001 {
            return (0.0002 / d) as f32;
        }
    }
    1.0
}

/// A whole number of FL's from a float, as `cvttsd2si` to 64 bits and the
/// low 32 kept.
fn wrap(v: f64) -> u32 {
    (v as i64) as u32
}

/// The 1024 tables, each 2048 points of the harmonics from the first up to
/// `1024 - index`, from `oscdata.dat`'s amplitudes and phases.
fn build_tables(data: &[u8]) -> Option<Vec<f32>> {
    if data.len() < 8 * TABLES {
        return None;
    }
    let float = |i: usize| f32::from_le_bytes(data[4 * i..4 * i + 4].try_into().unwrap());
    let mut tables = vec![0.0f32; TABLE * TABLES];
    let mut sum = vec![0.0f32; TABLE];
    for h in 0..TABLES {
        let amp = f64::from(float(h));
        let phase = f64::from(float(TABLES + h));
        let step = f64::from((h + 1) as f32) * 0.003_067_961_575_771_282_3;
        for (j, point) in sum.iter_mut().enumerate() {
            let angle = f64::from(j as f32) * step + phase;
            *point = (angle.cos() * amp + f64::from(*point)) as f32;
        }
        let at = (TABLES - 1 - h) * TABLE;
        tables[at..at + TABLE].copy_from_slice(&sum);
    }
    Some(tables)
}

/// The tables for this file, built once while any synth holds them.
fn tables(path: &Path) -> Option<Arc<[f32]>> {
    static BUILT: Mutex<Vec<(PathBuf, Weak<[f32]>)>> = Mutex::new(Vec::new());
    let mut built = BUILT.lock().unwrap_or_else(|e| e.into_inner());
    built.retain(|(_, tables)| tables.strong_count() > 0);
    if let Some(tables) = built
        .iter()
        .find(|(known, _)| known == path)
        .and_then(|(_, tables)| tables.upgrade())
    {
        return Some(tables);
    }
    let tables: Arc<[f32]> = build_tables(&std::fs::read(path).ok()?)?.into();
    built.push((path.to_path_buf(), Arc::downgrade(&tables)));
    Some(tables)
}

/// What the wrapper keeps of the last note on a key (`FUN_180003dd0`).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Slot {
    left: f32,
    right: f32,
    semitones: f32,
    cutoff: f32,
    resonance: f32,
}

const EMPTY: Slot = Slot {
    left: 1.0,
    right: 1.0,
    semitones: 0.0,
    cutoff: 0.0,
    resonance: 0.0,
};

/// The parameters as the synth keeps them once set (`FUN_180023c20`).
#[derive(Debug, Clone, Copy)]
struct Patch {
    poly: bool,
    legato: bool,
    transpose: f32,
    glide: f32,
    kind: i32,
    osc_mod: f32,
    noise: f32,
    cutoff: f32,
    peak: f32,
    keyboard: f32,
    velocity: f32,
    egf_attack: f32,
    egf_decay: f32,
    egf_sustain: f32,
    egf_release: f32,
    egf_amount: f32,
    lfo_shape: i32,
    lfo_dest: i32,
    lfo_rate: f64,
    lfo_amount: f32,
    amp_attack: f32,
    amp_decay: f32,
    amp_sustain: f32,
    amp_release: f32,
    delay_on: bool,
    delay_time: f64,
    chorus_on: bool,
    phaser_on: bool,
    phaser_rate: f32,
}

impl Default for Patch {
    fn default() -> Self {
        Patch {
            poly: false,
            legato: false,
            transpose: 0.0,
            glide: 0.0,
            kind: 0,
            osc_mod: 0.0,
            noise: 0.0,
            cutoff: 0.0,
            peak: 0.0,
            keyboard: 0.0,
            velocity: 0.0,
            egf_attack: 0.0,
            egf_decay: 0.0,
            egf_sustain: 0.0,
            egf_release: 0.0,
            egf_amount: 0.0,
            lfo_shape: 0,
            lfo_dest: 0,
            lfo_rate: 0.0,
            lfo_amount: 0.0,
            amp_attack: 0.0,
            amp_decay: 0.0,
            amp_sustain: 0.0,
            amp_release: 0.0,
            delay_on: false,
            delay_time: 0.0,
            chorus_on: false,
            phaser_on: false,
            phaser_rate: 0.0,
        }
    }
}

/// One of the 99 voices (`CMiniSynthVoice`, 0x220 bytes).
#[derive(Debug, Clone)]
struct Voice {
    /// +0x50, `None` for a free voice.
    note: Option<u8>,
    /// +0x54, the velocity byte over 127.
    velocity: f32,
    /// +0x48, the count of the note on that took it.
    age: u32,
    /// +0x4c, the key the pitch glides along.
    glide: f32,
    /// +0xa8, +0xc8, +0xe8: up to 8 oscillators' phase, step and gain.
    phase: [u32; 8],
    step: [u32; 8],
    gain: [f32; 8],
    /// +0x108.
    count: usize,
    /// The table oscillator 0's pitch picks (+0x68), as an index.
    table: usize,
    /// +0x110: the ladder's four stages.
    stages: [f64; 4],
    /// +0x130: the ladder over a sample, 4 rows by 5 columns.
    ladder: [f64; 20],
    /// +0x1d0..+0x1e4: the filter's and the amplitude's envelope, each a
    /// level, a target and a step.
    filter_env: [f32; 3],
    amp_env: [f32; 3],
    /// +0x1f0 and +0x1f8.
    pitch: f64,
    freq: f64,
    /// +0x200, the DC the output loses.
    dc: f32,
    /// +0x204 and +0x20c: the gains' targets and where they are.
    target: [f32; 2],
    level: [f32; 2],
    /// +0x214, the slow random drift of pitch and cutoff.
    drift: f32,
    /// +0x218.
    released: bool,
}

impl Voice {
    fn new() -> Self {
        Voice {
            note: None,
            velocity: 0.0,
            age: 0,
            glide: 0.0,
            phase: [0; 8],
            step: [0; 8],
            gain: [0.0; 8],
            count: 0,
            table: 0,
            stages: [0.0; 4],
            ladder: [0.0; 20],
            filter_env: [0.0; 3],
            amp_env: [0.0; 3],
            pitch: 0.0,
            freq: 0.0,
            dc: 0.0,
            target: [0.0; 2],
            level: [-1.0; 2],
            drift: 0.0,
            released: true,
        }
    }

    /// `FUN_180021260`: free the voice and give it new phases.
    fn kill(&mut self, seed: &mut u32) {
        for phase in &mut self.phase {
            for _ in 0..128 {
                *seed = seed_step(*seed);
            }
            *phase = *seed;
        }
        self.note = None;
        self.velocity = 0.0;
        self.stages = [0.0; 4];
        self.filter_env[0] = 0.0;
        self.amp_env[0] = 0.0;
        self.released = true;
        self.level = [-1.0; 2];
    }

    /// `FUN_180021330`.
    fn start(&mut self, note: u8, velocity: f32, legato: bool, patch: &Patch) {
        self.velocity = velocity;
        self.note = Some(note);
        if legato && !self.released {
            return;
        }
        self.filter_env = [0.0, 4.0, patch.egf_attack];
        self.amp_env[1] = 4.0;
        self.amp_env[2] = patch.amp_attack;
        self.released = false;
    }

    /// `FUN_1800213a0`.
    fn release(&mut self, patch: &Patch) {
        self.released = true;
        self.filter_env[1] = 0.0;
        self.filter_env[2] = patch.egf_release;
        self.amp_env[1] = 0.0;
        self.amp_env[2] = patch.amp_release;
    }
}

/// What a block's voices read of the synth beside the patch.
struct Shared<'a> {
    patch: &'a Patch,
    slots: &'a [Slot; KEYS],
    lfo: f32,
    rate: f64,
}

impl Voice {
    /// `FUN_180022250`: the once-a-block work. False when the voice is
    /// freed by it.
    fn control(&mut self, s: &Shared, seed: &mut u32) -> bool {
        let Some(note) = self.note else {
            return false;
        };
        let patch = s.patch;
        let slot = s.slots[usize::from(note)];
        let mut velocity = self.velocity;
        if self.amp_env[0] < 1e-5 && self.amp_env[1] == 0.0 {
            velocity = 0.0;
        }
        self.target = [velocity * slot.left, velocity * slot.right];
        for (level, target) in self.level.iter_mut().zip(self.target) {
            if *level < 0.0 {
                *level = target;
            }
        }
        if self.released && velocity == 0.0 && self.level[0] < 1e-5 && self.level[1] < 1e-5 {
            self.kill(seed);
            return false;
        }
        let old = *seed;
        *seed = noise_step(*seed);
        let random = (f64::from(old) * 4.656_612_874_161_595e-10 - 1.0) as f32;
        self.drift = (random - self.drift) * 0.01 + self.drift;
        let drift = self.drift * 0.08;
        self.glide = (f32::from(note) - self.glide) * patch.glide + self.glide;
        let mut pitch = f64::from(self.glide + patch.transpose + drift);
        if patch.lfo_dest == 1 {
            pitch += f64::from(s.lfo * 12.0);
        }
        pitch += f64::from(slot.semitones);
        self.pitch = pitch;
        self.freq = 2f64.powf((pitch - 57.0) * 0.083_333_333_333_333_33) * 220.0;
        self.oscillators(s);
        self.filter(s, slot);
        true
    }

    /// `FUN_180021d10`.
    fn oscillators(&mut self, s: &Shared) {
        let patch = s.patch;
        let mut m = patch.osc_mod;
        if patch.lfo_dest == 2 {
            m += s.lfo * 12.0;
        }
        let semis = ((m * 24.0 - 12.0) as i32) as f32;
        let ratio = f64::from(2f32.powf(semis * 0.083_333_336 + 0.006_250_000_6));
        let f = self.freq;
        let full = 4_294_967_295.0;
        let mut freqs = [f; 8];
        let mut gains = [0.5f32; 8];
        let p0 = self.phase[0];
        let count;
        match patch.kind {
            0 => {
                count = 3;
                gains[..3].copy_from_slice(&[1.0, -1.0, 1.0]);
                self.phase[2] = p0;
                let delta = wrap(f64::from(1.0 - m.abs()) * full);
                self.phase[1] = p0.wrapping_add(delta);
                if delta == 0 {
                    self.count = 1;
                    return self.steps(&freqs, &gains, s.rate);
                }
            }
            1 => {
                count = 2;
                freqs[1] = f * ratio;
                gains[..2].copy_from_slice(&[1.0, 1.0]);
            }
            2 => {
                count = 2;
                gains[..2].copy_from_slice(&[1.0, -1.0]);
                self.phase[1] = p0.wrapping_add(wrap(f64::from(1.0 - m.abs()) * full));
            }
            3 => {
                count = 3;
                freqs[2] = f * ratio;
                gains[..3].copy_from_slice(&[1.0, -1.0, 1.0]);
                self.phase[1] = p0.wrapping_add(0x7fff_ffff);
            }
            4 => {
                count = 4;
                freqs[2] = f * ratio;
                freqs[3] = f * ratio;
                gains[..4].copy_from_slice(&[1.0, -1.0, 1.0, -1.0]);
                self.phase[1] = p0.wrapping_sub(0x7000_0001);
                self.phase[3] = self.phase[2].wrapping_sub(0x7000_0001);
            }
            5 => {
                count = 8;
                for (j, freq) in freqs.iter_mut().enumerate() {
                    *freq = ((j as f64 * 0.0125 - 0.05) * f64::from(m) + 1.0) * f;
                }
            }
            6 => {
                count = 6;
                let spread = (m * 0.5).abs();
                let mut at = 0u32;
                for j in 0..6 {
                    self.phase[j] = p0.wrapping_add(at);
                    at = at.wrapping_add(wrap(f64::from(spread) * full));
                }
            }
            7 => {
                count = 2;
                gains[..2].copy_from_slice(&[1.0, 1.0]);
                self.phase[1] = p0.wrapping_add(wrap(f64::from(1.0 - m.abs()) * full));
            }
            8 | 9 => {
                count = 3;
                let d = f64::from(m) * 0.01;
                let low = if patch.kind == 8 { 0.5 } else { 1.5 };
                freqs[0] = (1.0 - d) * f * low;
                freqs[2] = (f + f) * (d + 1.0);
                gains[..3].copy_from_slice(&[1.0, 1.0, 1.0]);
            }
            _ => {
                self.count = 0;
                return;
            }
        }
        self.count = count;
        self.steps(&freqs, &gains, s.rate);
    }

    fn steps(&mut self, freqs: &[f64; 8], gains: &[f32; 8], rate: f64) {
        for j in 0..self.count {
            self.gain[j] = gains[j];
            self.step[j] = wrap(freqs[j] / rate * 4_294_967_295.0);
        }
        let f = freqs[0] as f32;
        let rate = rate as f32;
        self.table = if rate * 0.000_488_281_25 < f {
            (1023 - ((0.5 / f) * rate) as i32).clamp(0, 1023) as usize
        } else {
            0
        };
    }

    /// `FUN_1800213e0`: the ladder's cutoff and feedback, and the ladder
    /// run 4096 times over a sample, by twelve squarings.
    fn filter(&mut self, s: &Shared, slot: Slot) {
        let patch = s.patch;
        let mut octaves = (f64::from(self.velocity) - 0.5) * f64::from(patch.velocity) * 14.0;
        octaves += (self.pitch - 48.0) * f64::from(patch.keyboard) * 0.083_333_333_333_333_33;
        octaves += 4.0;
        octaves += f64::from(self.drift) * 0.103 + f64::from(patch.cutoff) * 10.3;
        if patch.lfo_dest == 0 {
            octaves += f64::from(s.lfo) * 4.0;
        }
        let base = 2f64.powf(f64::from(slot.cutoff) + octaves);
        let scale = 6.25 / (4096.0 * s.rate);
        let sweep = 2f64.powf(f64::from(patch.egf_amount) * 10.0);
        let hz = ((sweep * base - base) * f64::from(self.filter_env[0]) + base).min(20_000.0);
        let g = (scale * hz).clamp(0.0, 1.0);
        let res = (f64::from(slot.resonance) + f64::from(patch.peak)).clamp(0.0, 1.0);
        let k = res * -4.050_000_190_734_863;
        let h = 1.0 - g;
        let (g2, g3) = (g * g, g * g * g);
        self.ladder = [
            h,
            h * g,
            h * g2,
            h * g3,
            0.0,
            h,
            h * g,
            h * g2,
            0.0,
            0.0,
            h,
            h * g,
            k * g,
            k * g2,
            k * g3,
            k * g3 * g + h,
            g,
            g2,
            g3,
            g3 * g,
        ];
        for _ in 0..12 {
            let a = self.ladder;
            for r in 0..4 {
                for c in 0..4 {
                    self.ladder[4 * c + r] = (0..4).map(|k| a[4 * k + r] * a[4 * c + k]).sum();
                }
                self.ladder[16 + r] =
                    (0..4).map(|k| a[4 * k + r] * a[16 + k]).sum::<f64>() + a[16 + r];
            }
        }
    }

    /// `FUN_180022440`'s loop: add `left.len()` samples.
    fn render(
        &mut self,
        tables: &[f32],
        patch: &Patch,
        seed: &mut u32,
        left: &mut [f32],
        right: &mut [f32],
    ) {
        let table = &tables[self.table * TABLE..(self.table + 1) * TABLE];
        let m = self.ladder;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let old = *seed;
            *seed = noise_step(*seed);
            let mut sum = 0.0f64;
            for j in 0..self.count {
                let at = self.phase[j].wrapping_add(*seed & 0x3f_ffff) >> 21;
                self.phase[j] = self.phase[j].wrapping_add(self.step[j]);
                sum += f64::from(table[at as usize] * self.gain[j]);
            }
            let unit = f64::from(old) * 2.328_306_437_080_797_4e-10;
            let noise = ((unit + unit) - 1.0) as f32 * patch.noise;
            let amp = self.amp_env[0];
            let x = ((f64::from(noise) + sum) * 0.200_000_002_980_232_24
                + 0.069_999_999_850_988_37)
                * f64::from(amp);
            let [s0, s1, s2, s3] = self.stages;
            let y = x * m[19] + s3 * m[15] + s2 * m[11] + s0 * m[3] + s1 * m[7];
            let n1 = s3 * m[13] + x * m[17] + s2 * m[9] + s0 * m[1] + s1 * m[5];
            let n0 = s3 * m[12] + x * m[16] + s2 * m[8] + s0 * m[0] + s1 * m[4];
            let n2 = s3 * m[14] + x * m[18] + s2 * m[10] + s0 * m[2] + s1 * m[6];
            self.stages = [n0 / (n0 * n0 + 1.0), n1, n2, y];
            let mut out = y * 4.999_999_925_494_195;
            self.dc = ((out - f64::from(self.dc)) * 0.004_999_999_888_241_291 + f64::from(self.dc))
                as f32;
            out += f64::from(-self.dc);
            let [level, target, rate] = self.filter_env;
            self.filter_env[0] = (target - level) * rate + level;
            let [level, target, rate] = self.amp_env;
            self.amp_env[0] = (target - level) * rate + level;
            for (level, target) in self.level.iter_mut().zip(self.target) {
                *level = (target - *level) * 0.005 + *level;
            }
            if self.filter_env[0] > 1.0 {
                self.filter_env = [1.0, patch.egf_sustain, patch.egf_decay];
            }
            if self.amp_env[0] > 1.0 {
                self.amp_env = [1.0, patch.amp_sustain, patch.amp_decay];
            }
            *l = (f64::from(self.level[0]) * out + f64::from(*l)) as f32;
            *r = (f64::from(self.level[1]) * out + f64::from(*r)) as f32;
        }
    }
}

/// The distortion's sample and hold, the delay, the chorus and the
/// phaser, with their buffers.
#[derive(Debug, Clone, Default)]
struct Effects {
    hold: i32,
    held: [f32; 2],
    delay: Vec<f32>,
    delay_at: usize,
    delay_lp: f32,
    delay_reset: bool,
    chorus: Vec<f32>,
    chorus_at: usize,
    chorus_phase: [f32; 2],
    chorus_reset: bool,
    phaser: [f32; 12],
    phaser_feedback: [f32; 2],
    phaser_centre: f32,
    phaser_phase: f32,
    phaser_reset: bool,
}

/// FL's MiniSynth.
pub struct MiniSynth {
    params: [f32; PARAMS],
    patch: Patch,
    preset: String,
    voices: Vec<Voice>,
    /// +0x3ae4: the velocity each key is held at.
    held: [f32; KEYS],
    slots: [Slot; KEYS],
    /// The host's note on each key, which FL's wrapper keeps per key.
    keys: [Option<u32>; KEYS],
    /// +0x3ce4 and +0x3ce8.
    count: u32,
    last: u8,
    /// +0x3a3c and +0x3a40.
    lfo: f32,
    lfo_phase: f64,
    /// FUN_180079cf4, the one seed of every voice.
    seed: u32,
    /// +0x40c4: frames since a voice sounded.
    quiet: u32,
    effects: Effects,
    rate: f64,
    tempo: f64,
    /// The song's position in beats.
    beats: f64,
    tables: Option<Arc<[f32]>>,
    looked: bool,
}

impl Default for MiniSynth {
    fn default() -> Self {
        let mut synth = MiniSynth {
            params: DEFAULTS,
            patch: Patch::default(),
            preset: String::new(),
            voices: vec![Voice::new(); VOICES],
            held: [0.0; KEYS],
            slots: [EMPTY; KEYS],
            keys: [None; KEYS],
            count: 1,
            last: 0,
            lfo: 0.0,
            lfo_phase: 0.0,
            seed: 0,
            quiet: u32::MAX,
            effects: Effects::default(),
            rate: 44_100.0,
            tempo: 120.0,
            beats: 0.0,
            tables: None,
            looked: false,
        };
        for voice in &mut synth.voices {
            voice.kill(&mut synth.seed);
        }
        for (index, value) in DEFAULTS.into_iter().enumerate() {
            synth.apply(index, value);
        }
        synth.size_buffers();
        synth
    }
}

impl MiniSynth {
    /// The preset name the state holds.
    pub fn preset(&self) -> &str {
        &self.preset
    }

    /// The 43 parameters as floats from 0 to 1.
    pub fn params(&self) -> &[f32; PARAMS] {
        &self.params
    }

    /// `FUN_180023c20`: one parameter, clamped to 0..1.
    fn apply(&mut self, index: usize, value: f32) {
        let v = if value <= 0.0 { 0.0 } else { value.min(1.0) };
        self.params[index] = v;
        let p = &mut self.patch;
        match index {
            0 => match (v + v) as i32 {
                0 => (p.poly, p.legato) = (true, false),
                1 => (p.poly, p.legato) = (false, false),
                _ => (p.poly, p.legato) = (false, true),
            },
            1 => p.transpose = ((v - 0.5) * 24.0) as i32 as f32,
            2 => {
                let x = (1.0 - v) * (1.0 - v);
                p.glide = (f64::from(x * x) * 0.9 + 0.01) as f32;
            }
            5 => p.kind = (v * 9.0) as i32,
            6 => p.osc_mod = v,
            7 => p.noise = v * v,
            8 => p.cutoff = v,
            9 => p.peak = v * 0.99,
            10 => p.keyboard = v,
            11 => p.velocity = v,
            12 => p.egf_attack = env_rate(f64::from(v), true),
            13 => p.egf_decay = env_rate(f64::from(v), false),
            14 => p.egf_sustain = v,
            15 => p.egf_release = env_rate(f64::from(v), false),
            16 => p.egf_amount = (v + v) - 1.0,
            17 => p.lfo_shape = (v + v) as i32,
            18 => p.lfo_dest = (v + v) as i32,
            19 => p.lfo_rate = RATES[((v * 11.0) as usize).min(11)],
            20 => p.lfo_amount = (v + v) - 1.0,
            21 => p.amp_attack = env_rate(f64::from(v), true),
            22 => p.amp_decay = env_rate(f64::from(v), false),
            23 => p.amp_sustain = v,
            24 => p.amp_release = env_rate(f64::from(v) * 0.95 + 0.05, false) * 10.0,
            28 => {
                let on = v != 0.0;
                self.effects.delay_reset |= on != p.delay_on;
                p.delay_on = on;
            }
            29 => p.delay_time = (f64::from((v * 3.0) as i32) + 1.0) * 0.25,
            33 => {
                let on = v != 0.0;
                self.effects.chorus_reset |= on != p.chorus_on;
                p.chorus_on = on;
            }
            37 => {
                let on = v != 0.0;
                self.effects.phaser_reset |= on != p.phaser_on;
                p.phaser_on = on;
            }
            38 => p.phaser_rate = RATES[((v * 11.0) as usize).min(11)] as f32,
            _ => {}
        }
    }

    /// `FUN_180023310`: the delay holds 1.5 s and the chorus 10 ms.
    fn size_buffers(&mut self) {
        let fx = &mut self.effects;
        fx.delay = vec![0.0; (self.rate * 1.5) as usize];
        fx.chorus = vec![0.0; (self.rate * 0.01) as usize];
        fx.delay_reset = true;
        fx.chorus_reset = true;
    }

    /// `FUN_1800248c0` after a new rate: every key let go, every voice
    /// freed, the effects started over.
    fn start_over(&mut self) {
        self.held = [0.0; KEYS];
        self.keys = [None; KEYS];
        self.lfo_phase = 0.0;
        for voice in &mut self.voices {
            voice.kill(&mut self.seed);
        }
        let fx = &mut self.effects;
        fx.hold = 0;
        fx.held = [0.0; 2];
        fx.delay_reset = true;
        fx.chorus_reset = true;
        fx.phaser_reset = true;
        self.quiet = u32::MAX;
    }

    /// `FUN_180024520`: a key at a velocity from 0 to 1, where 0 lets it
    /// go.
    fn key(&mut self, key: u8, velocity: f32) {
        let velocity = velocity.clamp(0.0, 1.0);
        let k = usize::from(key);
        self.held[k] = velocity;
        let patch = self.patch;
        if velocity == 0.0 {
            for v in 0..VOICES {
                if self.voices[v].note != Some(key) {
                    continue;
                }
                if !patch.poly && patch.legato {
                    let below = (0..k).rev().find(|&i| self.held[i] != 0.0);
                    let above = (k + 1..KEYS).find(|&i| self.held[i] != 0.0);
                    if let Some(other) = below.or(above) {
                        let voice = &mut self.voices[v];
                        voice.note = Some(other as u8);
                        voice.velocity = self.held[other];
                        self.last = other as u8;
                        continue;
                    }
                }
                self.voices[v].release(&patch);
            }
            return;
        }
        let pick = if patch.poly {
            let free = (0..VOICES).rev().find(|&v| self.voices[v].note.is_none());
            free.unwrap_or_else(|| {
                let mut pick = 0;
                let mut oldest = u32::MAX;
                for (v, voice) in self.voices.iter().enumerate() {
                    if voice.age <= oldest {
                        pick = v;
                        oldest = voice.age;
                    }
                }
                pick
            })
        } else {
            0
        };
        self.count = self.count.wrapping_add(1);
        let others = (0..KEYS).any(|i| i != k && self.held[i] != 0.0);
        let voice = &mut self.voices[pick];
        voice.age = self.count;
        voice.glide = f32::from(self.last);
        if patch.legato && !others {
            voice.glide = f32::from(key);
        }
        voice.start(key, velocity, patch.legato, &patch);
        self.last = key;
    }

    /// `FUN_1800249b0`: the LFO's value for the block, then its phase on.
    fn lfo(&mut self, beats: f64) {
        let p = &self.patch;
        match p.lfo_shape {
            0 => self.lfo = (self.lfo_phase * TAU).sin() as f32,
            1 => self.lfo = ((1.0 - self.lfo_phase) * 2.0 - 1.0) as f32,
            2 => self.lfo = f32::from(u8::from(0.5 > self.lfo_phase)) * 2.0 - 1.0,
            _ => {}
        }
        let amount = p.lfo_amount;
        if amount <= 1.0 {
            if amount >= -1.0 {
                self.lfo *= amount * amount;
                if amount < 0.0 {
                    self.lfo = -self.lfo;
                }
            } else {
                self.lfo = -self.lfo;
            }
        }
        let at = beats / p.lfo_rate;
        self.lfo_phase = at - f64::from(at as i32);
    }

    /// One block of at most [`BLOCK`] frames (`FUN_180025a40`'s loop).
    fn block(&mut self, left: &mut [f32], right: &mut [f32], sounded: &mut bool) {
        let frames = left.len();
        let mut l = [0.0f32; BLOCK];
        let mut r = [0.0f32; BLOCK];
        let (l, r) = (&mut l[..frames], &mut r[..frames]);
        self.lfo(self.beats);
        self.beats += frames as f64 / self.rate * self.tempo * (1.0 / 60.0);
        if let Some(tables) = &self.tables {
            let shared = Shared {
                patch: &self.patch,
                slots: &self.slots,
                lfo: self.lfo,
                rate: self.rate,
            };
            for voice in &mut self.voices {
                if voice.note.is_none() {
                    continue;
                }
                *sounded = true;
                if voice.control(&shared, &mut self.seed) {
                    voice.render(tables, &self.patch, &mut self.seed, l, r);
                }
            }
        }
        if *sounded {
            self.quiet = 0;
        } else if self.quiet != u32::MAX {
            self.quiet = self.quiet.wrapping_add(frames as u32);
        }
        if self.quiet < (self.rate * 0.5) as i32 as u32 {
            self.distortion(l, r);
            self.delay(l, r);
            self.phaser(l, r);
            self.chorus(l, r);
            *sounded = true;
        }
        for (out, add) in left.iter_mut().zip(l.iter()) {
            *out += add;
        }
        for (out, add) in right.iter_mut().zip(r.iter()) {
            *out += add;
        }
    }

    /// `FUN_180025150`.
    fn distortion(&mut self, left: &mut [f32], right: &mut [f32]) {
        if self.params[25] == 0.0 {
            return;
        }
        let od = self.params[26];
        let drive = od * od * 20.0;
        let x = 1.0 - drive * 0.0625;
        let x = x * x;
        let x = x * x;
        let gain = (f64::from(x * x) * 0.7 + 0.3) as f32;
        let dec = self.params[27];
        let hold = ((dec * dec).clamp(0.0, 1.0) * 64.0) as i32;
        let shape = |s: f32| {
            if s >= 0.0 {
                1.0 - 10f32.powf(-(s * drive)) * (1.0 - s)
            } else {
                10f32.powf(s * drive) * (s + 1.0) - 1.0
            }
        };
        let fx = &mut self.effects;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let mut a = gain * shape(*l);
            let mut b = gain * shape(*r);
            if hold != 0 {
                let mut count = fx.hold;
                if count as f32 > 0.0 {
                    [a, b] = fx.held;
                } else {
                    fx.held = [a, b];
                    count += hold;
                }
                fx.hold = (count as f32 - 1.0) as i32;
            }
            *l = a;
            *r = b;
        }
    }

    /// `FUN_1800257c0`.
    fn delay(&mut self, left: &mut [f32], right: &mut [f32]) {
        let fx = &mut self.effects;
        if !self.patch.delay_on || fx.delay.is_empty() {
            return;
        }
        if fx.delay_reset {
            fx.delay_reset = false;
            fx.delay.fill(0.0);
            fx.delay_at = 0;
            fx.delay_lp = 0.0;
        }
        let filter = self.params[31].clamp(0.0, 1.0) * 0.99 + 0.01;
        let feedback = self.params[30];
        let mix = self.params[32];
        let mut time = self.patch.delay_time * 40.0 / self.tempo;
        while time > 1.0 {
            time *= 0.5;
        }
        let size = fx.delay.len();
        let len = (((size - 1) as f64 * time) as i32).clamp(1, size as i32) as usize;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            if fx.delay_at >= len {
                fx.delay_at = 0;
            }
            let back = f64::from(feedback * fx.delay[fx.delay_at]) + 1e-30;
            fx.delay_lp = ((back - f64::from(fx.delay_lp)) * f64::from(filter)
                + f64::from(fx.delay_lp)) as f32;
            let echo = fx.delay_lp;
            let out = echo * mix * mix;
            fx.delay[fx.delay_at] = *r + *l + echo;
            if f64::from(out.abs()) > 1e-5 {
                self.quiet = 0;
            }
            *l += out;
            *r += out;
            fx.delay_at += 1;
        }
    }

    /// `FUN_180024b50`: six allpasses a side, swept by a cosine.
    fn phaser(&mut self, left: &mut [f32], right: &mut [f32]) {
        let fx = &mut self.effects;
        if !self.patch.phaser_on {
            return;
        }
        if fx.phaser_reset {
            fx.phaser_reset = false;
            fx.phaser = [0.0; 12];
            fx.phaser_feedback = [0.0; 2];
            fx.phaser_centre = -1.0;
        }
        let hi = (0.5 / self.rate * 40_000.0) as f32;
        let lo = (0.5 / self.rate * 100.0) as f32;
        let width = self.params[39].powf(3.0) * 0.5;
        let feedback = (f64::from(self.params[40].powf(0.333_333_34)) * 0.98) as f32;
        let centre = (self.params[42] * self.params[42]).max(1e-5);
        let mix = self.params[41].max(1e-5);
        if fx.phaser_centre < 0.0 {
            fx.phaser_centre = centre;
        }
        let at = (self.beats / f64::from(self.patch.phaser_rate)) as f32;
        fx.phaser_phase = at - (at as i32) as f32;
        let sweep = (f64::from(fx.phaser_phase) * TAU).cos();
        let up = ((sweep + 1.0) * f64::from(width)) as f32;
        let down = ((1.0 - sweep) * f64::from(width)) as f32;
        let span = hi - lo;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let c = fx.phaser_centre;
            let fl = ((c + up) * span + lo).clamp(1e-5, 0.499);
            let fr = ((c + down) * span + lo).clamp(1e-5, 0.499);
            fx.phaser_centre = (f64::from(centre - c) * 0.001 + f64::from(c)) as f32;
            let ins = [
                (f64::from(*l) + 1e-30) as f32 + feedback * fx.phaser_feedback[0],
                (f64::from(*r) + 1e-30) as f32 + feedback * fx.phaser_feedback[1],
            ];
            let coefs = [(1.0 - fl) / (fl + 1.0), (1.0 - fr) / (fr + 1.0)];
            for side in 0..2 {
                let a = coefs[side];
                let mut u = ins[side];
                for z in &mut fx.phaser[side * 6..side * 6 + 6] {
                    let v = *z - a * u;
                    *z = v * a + u;
                    u = v;
                }
                fx.phaser_feedback[side] = u;
            }
            *l += fx.phaser_feedback[0] * mix;
            *r += fx.phaser_feedback[1] * mix;
        }
    }

    /// `FUN_1800253f0`: two taps on 10 ms of the sum of the sides.
    fn chorus(&mut self, left: &mut [f32], right: &mut [f32]) {
        let fx = &mut self.effects;
        if !self.patch.chorus_on || fx.chorus.is_empty() {
            return;
        }
        if fx.chorus_reset {
            fx.chorus_reset = false;
            fx.chorus.fill(0.0);
            fx.chorus_at = 0;
            fx.chorus_phase = [0.62, 0.1];
        }
        let n = fx.chorus.len();
        let depth = self.params[35] * 0.9 * 0.4 + 0.04;
        let mix = self.params[36] * self.params[36];
        let speed = self.params[34];
        let hz = speed * speed * 4.0 + 0.2;
        let step = [
            (f64::from(hz) * (1.0 / self.rate)) as f32,
            (f64::from(hz + 0.05) * (1.0 / self.rate)) as f32,
        ];
        let tap = |buf: &[f32], from: usize, pos: f32| {
            let i = pos as i32;
            let part = pos - i as f32;
            let mut a = from + i as usize;
            if a >= n {
                a -= n;
            }
            let mut b = a + 1;
            if b >= n {
                b -= n;
            }
            (1.0 - part) * buf[a] + part * buf[b]
        };
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            if fx.chorus_at >= n {
                fx.chorus_at = 0;
            }
            let w = fx.chorus_at;
            fx.chorus[w] = *l + *r;
            let s = ((f64::from(fx.chorus_phase[0]) * TAU).sin() * f64::from(depth) + 0.5) as f32;
            let one = tap(&fx.chorus, w, n as f32 * depth * (s * s));
            let s = ((f64::from(fx.chorus_phase[1]) * TAU).sin() * f64::from(depth) + 0.5) as f32;
            let two = tap(&fx.chorus, w, s);
            for (phase, step) in fx.chorus_phase.iter_mut().zip(step) {
                let next = *phase + step;
                *phase = next - (next as i32) as f32;
            }
            *l += one * mix;
            *r += two * mix;
            fx.chorus_at += 1;
        }
    }
}

/// The 43 parameters a state holds, where it holds them.
fn parse(bytes: &[u8]) -> Result<(String, Vec<f32>), StateError> {
    let short = |expected| StateError::Length {
        expected,
        found: bytes.len(),
    };
    if bytes.len() < 8 || &bytes[..4] != b"SSLF" {
        return Err(short(8));
    }
    let len = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let mut chunk = bytes.get(8..8 + len).ok_or(short(8 + len))?;
    // A VST bank header before it is skipped (FUN_180007700).
    if chunk.len() > 0x3c && &chunk[..4] == b"CcnK" && &chunk[8..12] == b"FPCh" {
        chunk = &chunk[0x3c..];
    }
    if chunk.len() < 0x16 || chunk[..8] != *b"\x99FLS\r\n\x1a\n" {
        return Err(short(8 + 0x16));
    }
    let chars = usize::from(u16::from_le_bytes([chunk[0x14], chunk[0x15]]));
    let name_end = 0x16 + 2 * chars;
    let name = chunk.get(0x16..name_end).ok_or(short(8 + name_end))?;
    let name: Vec<u16> = name
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    let data = chunk
        .get(name_end..name_end + 8)
        .ok_or(short(8 + name_end + 8))?;
    let mut left = i32::from_le_bytes(data[4..8].try_into().unwrap());
    let mut values = Vec::new();
    for index in 0..PARAMS {
        left -= 4;
        if left <= 3 {
            break;
        }
        let at = name_end + 0xc + 4 * index;
        let value = chunk.get(at..at + 4).ok_or(short(8 + at + 4))?;
        values.push(f32::from_le_bytes(value.try_into().unwrap()));
    }
    Ok((String::from_utf16_lossy(&name), values))
}

impl Generator for MiniSynth {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let (preset, values) = parse(bytes)?;
        self.preset = preset;
        for (index, value) in values.into_iter().enumerate() {
            self.apply(index, value);
        }
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        let rate = f64::from(hz);
        if rate > 0.0 && rate != self.rate {
            self.rate = rate;
            self.size_buffers();
        }
        self.start_over();
    }

    fn set_tempo(&mut self, bpm: f64) {
        if bpm > 0.0 {
            self.tempo = bpm;
        }
    }

    fn set_resource_dir(&mut self, dir: &Path) {
        self.looked = true;
        self.tables = tables(&dir.join(OSC_DATA));
    }

    fn note_on(&mut self, note: NoteOn) {
        if note.slide {
            return;
        }
        let key = note.key.clamp(0, KEYS as i16 - 1) as u8;
        let k = usize::from(key);
        if self.keys[k].is_some_and(|voice| voice != note.voice) {
            self.key(key, 0.0);
        }
        self.keys[k] = Some(note.voice);
        let (left, right) = crate::sampler::circular(note.pan, note.velocity);
        self.slots[k] = Slot {
            left,
            right,
            semitones: (f64::from(note.fine_pitch) * 0.01) as f32,
            cutoff: note.mod_x,
            resonance: note.mod_y,
        };
        self.key(key, crate::bassdrum::velocity_level(note.velocity));
    }

    fn note_off(&mut self, voice: u32) {
        for k in 0..KEYS {
            if self.keys[k] == Some(voice) {
                self.keys[k] = None;
                self.key(k as u8, 0.0);
            }
        }
    }

    fn all_notes_off(&mut self) {
        for k in 0..KEYS {
            if self.keys[k].take().is_some() {
                self.key(k as u8, 0.0);
            }
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let mut sounded = false;
        for (l, r) in left.chunks_mut(BLOCK).zip(right.chunks_mut(BLOCK)) {
            self.block(l, r, &mut sounded);
        }
    }

    fn param_count(&self) -> usize {
        PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        self.params
            .get(index)
            .map_or(0.0, |v| ((v * 256.0) as i32) as f32)
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if index < PARAMS {
            let whole = value.round().clamp(i32::MIN as f32, i32::MAX as f32) as i32;
            self.apply(index, (f64::from(whole) * 0.003_906_25) as f32);
        }
    }

    fn reset(&mut self) {
        self.start_over();
        self.beats = 0.0;
        self.lfo = 0.0;
    }

    fn tail(&self) -> usize {
        // The release falls by its step a sample to 1e-5, then the gains
        // follow and the effects run half a second on, longer while the
        // delay still rings.
        let rate = f64::from(self.patch.amp_release).clamp(1e-9, 1.0);
        let release = if rate >= 1.0 {
            1.0
        } else {
            (1e-5f64).ln() / (1.0 - rate).ln()
        };
        let mut frames = release + 2500.0 + self.rate * 0.5;
        if self.patch.delay_on {
            let feedback = f64::from(self.params[30]).clamp(0.0, 0.999);
            let repeats = if feedback > 0.0 {
                ((1e-5f64).ln() / feedback.ln()).min(200.0)
            } else {
                1.0
            };
            frames += repeats * self.rate * 1.5;
        }
        frames.min(self.rate * 120.0) as usize
    }

    fn fidelity(&self) -> Fidelity {
        if self.tables.is_none() {
            Fidelity::Approximate("MiniSynth oscdata.dat not available")
        } else {
            Fidelity::Exact
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    fn note(voice: u32, key: i16, velocity: f32) -> NoteOn {
        NoteOn {
            voice,
            key,
            velocity,
            pan: 0.0,
            fine_pitch: 0.0,
            release: 0.5,
            mod_x: 0.0,
            mod_y: 0.0,
            slide: false,
            porta: false,
        }
    }

    /// A state as FL saves it.
    fn state(name: &str, params: &[f32]) -> Vec<u8> {
        let name: Vec<u16> = name.encode_utf16().collect();
        let mut chunk = b"\x99FLS\r\n\x1a\nHEAD".to_vec();
        chunk.extend((name.len() as u32 * 2 + 6).to_le_bytes());
        chunk.extend([1, 0, 1, 0]);
        chunk.extend((name.len() as u16).to_le_bytes());
        chunk.extend(name.iter().flat_map(|c| c.to_le_bytes()));
        chunk.extend(b"DATA");
        chunk.extend((4 * params.len() as u32 + 4).to_le_bytes());
        chunk.extend(b"0rev");
        chunk.extend(params.iter().flat_map(|v| v.to_le_bytes()));
        let mut bytes = b"SSLF".to_vec();
        bytes.extend((chunk.len() as u32).to_le_bytes());
        bytes.extend(chunk);
        bytes
    }

    /// An `oscdata.dat` of the first harmonic alone, as a sine.
    fn sine_data() -> Vec<u8> {
        let mut floats = vec![0.0f32; 2 * TABLES];
        floats[0] = 1.0;
        floats[TABLES] = -std::f32::consts::FRAC_PI_2;
        floats.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    fn with_sine() -> MiniSynth {
        MiniSynth {
            tables: Some(build_tables(&sine_data()).unwrap().into()),
            ..MiniSynth::default()
        }
    }

    fn play(synth: &mut MiniSynth, frames: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
        for (l, r) in left.chunks_mut(512).zip(right.chunks_mut(512)) {
            synth.process(l, r);
        }
        (left, right)
    }

    #[test]
    fn a_state_reads_back_its_name_and_parameters() {
        let mut params = DEFAULTS;
        params[11] = 0.25;
        params[5] = 2.0;
        let mut synth = MiniSynth::default();
        synth.set_state(&state("Lead", &params)).unwrap();
        assert_eq!(synth.preset(), "Lead");
        assert_eq!(synth.params()[11], 0.25);
        // FL's controls keep a parameter within 0..1.
        assert_eq!(synth.params()[5], 1.0);
        assert_eq!(synth.param(11), 64.0);
        synth.set_param(11, 128.0);
        assert_eq!(synth.params()[11], 0.5);
        synth.set_param(11, 1e9);
        assert_eq!(synth.param(11), 256.0);
        synth.set_param(11, f32::MIN);
        assert_eq!(synth.param(11), 0.0);
        assert_eq!(synth.param(PARAMS), 0.0);
        // A short DATA block leaves the rest alone.
        synth.set_param(11, 64.0);
        synth.set_state(&state("Short", &[1.0, 0.5])).unwrap();
        assert_eq!(synth.params()[0], 1.0);
        assert_eq!(synth.params()[11], 0.25);
        assert!(synth.set_state(b"SSLF\x04\0\0\0abcd").is_err());
        assert!(synth.set_state(&state("x", &DEFAULTS)[..30]).is_err());
        assert_eq!(synth.preset(), "Short");
    }

    #[test]
    fn the_parameters_take_the_shapes_the_dll_gives_them() {
        let mut synth = MiniSynth::default();
        let p = synth.patch;
        assert!(p.poly && !p.legato);
        assert_eq!(p.transpose, 0.0);
        assert_eq!(p.glide, 0.91);
        assert_eq!(p.lfo_rate, 0.5);
        assert_eq!(p.phaser_rate, 0.375);
        assert_eq!(p.delay_time, 0.75);
        // 0.0002 / (0.95 v + 0.05)^4 / 50 for a decay, / 500 for an
        // attack, and a step of 1 for the least.
        assert_eq!(env_rate(1.0, false), 0.000_004);
        assert_eq!(env_rate(1.0, true), 0.000_000_4);
        assert_eq!(env_rate(0.0, true), 1.0);
        synth.apply(0, 1.0);
        assert!(!synth.patch.poly && synth.patch.legato);
        synth.apply(1, 0.0);
        assert_eq!(synth.patch.transpose, -12.0);
        synth.apply(19, 1.0);
        assert_eq!(synth.patch.lfo_rate, 256.0);
    }

    #[test]
    fn the_tables_add_up_the_harmonics_below_their_index() {
        let mut data = sine_data();
        // The second harmonic at half the first, in cosine phase.
        data[4..8].copy_from_slice(&0.5f32.to_le_bytes());
        let tables = build_tables(&data).unwrap();
        let top = &tables[(TABLES - 1) * TABLE..];
        let next = &tables[(TABLES - 2) * TABLE..(TABLES - 1) * TABLE];
        for j in [0, 100, 512, 1500] {
            let x = TAU * j as f64 / TABLE as f64;
            assert!((f64::from(top[j]) - x.sin()).abs() < 1e-6, "{j}");
            let both = x.sin() + 0.5 * (2.0 * x).cos();
            assert!((f64::from(next[j]) - both).abs() < 1e-6, "{j}");
            assert_eq!(tables[j], next[j]);
        }
    }

    #[test]
    fn the_ladder_passes_a_constant_without_feedback() {
        let mut synth = with_sine();
        synth.apply(9, 0.0);
        let mut voice = Voice::new();
        voice.note = Some(60);
        voice.velocity = 0.5;
        voice.pitch = 48.0;
        let shared = Shared {
            patch: &synth.patch,
            slots: &synth.slots,
            lfo: 0.0,
            rate: 44_100.0,
        };
        voice.filter(&shared, EMPTY);
        let m = voice.ladder;
        // Every stage's row of the ladder over a sample, input column
        // and all, adds to 1 without feedback.
        for r in 0..4 {
            let row: f64 = (0..5).map(|c| m[4 * c + r]).sum();
            assert!((row - 1.0).abs() < 1e-9, "{r} {row}");
        }
        // FLT Freq 1 is 2^(4 + 10.3) Hz, over 20 kHz, so 20 kHz: the
        // first stage moves most of the way to the input in a sample.
        assert!(m[16] > 0.9 && m[16] < 1.0, "{}", m[16]);
        // A peak feeds the last stage back.
        let open = voice.ladder;
        synth.apply(9, 1.0);
        let shared = Shared {
            patch: &synth.patch,
            slots: &synth.slots,
            lfo: 0.0,
            rate: 44_100.0,
        };
        voice.filter(&shared, EMPTY);
        assert_ne!(voice.ladder[12], open[12]);
        assert_eq!(open[12], 0.0);
    }

    #[test]
    fn a_note_sounds_at_its_key_and_stops_after_its_release() {
        let mut synth = with_sine();
        let mut params = DEFAULTS;
        // Saw kind with OSC Mod 1 is one oscillator.
        params[6] = 1.0;
        params[9] = 0.0;
        params[24] = 0.0;
        synth.set_state(&state("", &params)).unwrap();
        synth.set_sample_rate(44_100.0);
        synth.note_on(note(1, 69, 1.0));
        let (left, right) = play(&mut synth, 44_100);
        assert!(left.iter().chain(&right).all(|v| v.is_finite()));
        // 440 Hz, give or take the drift and the dither.
        let tail = &left[4410..44_100];
        let at = testing::partial(tail, 440.0 / 44_100.0);
        let off = testing::partial(tail, 470.0 / 44_100.0);
        assert!(at > 0.05 && at > 20.0 * off, "{at} {off}");
        assert_eq!(left, right);
        synth.note_off(1);
        play(&mut synth, 44_100);
        assert!(synth.voices.iter().all(|v| v.note.is_none()));
    }

    #[test]
    fn a_second_note_on_a_key_lets_the_first_go_and_lead_returns_to_a_held_key() {
        let mut synth = with_sine();
        synth.note_on(note(1, 60, 1.0));
        synth.note_on(note(2, 60, 1.0));
        assert_eq!(synth.voices.iter().filter(|v| !v.released).count(), 1);
        // The old note's off is no longer the key's.
        synth.note_off(1);
        assert_eq!(synth.voices.iter().filter(|v| !v.released).count(), 1);
        synth.reset();
        synth.apply(0, 1.0);
        synth.note_on(note(1, 60, 1.0));
        synth.note_on(note(2, 64, 1.0));
        assert_eq!(synth.voices[0].note, Some(64));
        synth.note_off(2);
        assert_eq!(synth.voices[0].note, Some(60));
        assert!(!synth.voices[0].released);
        synth.note_off(1);
        assert!(synth.voices[0].released);
        // A velocity that makes a byte of 0 is a note off.
        synth.reset();
        synth.note_on(note(3, 60, 0.0));
        assert!(synth.voices.iter().all(|v| v.note.is_none()));
    }

    #[test]
    fn the_distortion_at_no_drive_passes_the_samples() {
        let mut synth = MiniSynth::default();
        synth.params[25] = 1.0;
        let mut l = [0.5, -0.25, 0.0];
        let mut r = [-0.5, 0.75, 1.0];
        synth.distortion(&mut l, &mut r);
        assert_eq!(l, [0.5, -0.25, 0.0]);
        assert_eq!(r, [-0.5, 0.75, 1.0]);
        // Drive 5 bends towards 1 at a gain of 0.7 (1 - 5 / 16)^8 + 0.3,
        // and the decimator holds.
        synth.params[26] = 0.5;
        synth.params[27] = 0.5;
        let mut l = [0.5; 40];
        let mut r = [0.5; 40];
        l[1] = 0.0;
        synth.distortion(&mut l, &mut r);
        let shaped = 1.0 - 10f32.powf(-2.5) * 0.5;
        let gain = (f64::from(0.6875f32.powi(8)) * 0.7 + 0.3) as f32;
        assert!((l[0] - gain * shaped).abs() < 1e-6, "{}", l[0]);
        assert_eq!(l[1], l[0]);
    }

    #[test]
    fn without_fl_s_file_it_is_silent_and_says_so() {
        let mut synth = MiniSynth::default();
        synth.set_resource_dir(Path::new("/nowhere"));
        assert!(matches!(synth.fidelity(), Fidelity::Approximate(_)));
        synth.note_on(note(1, 60, 1.0));
        let (left, _) = play(&mut synth, 2048);
        assert!(left.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn every_state_fl_ships_loads_and_plays() {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, MiniSynth's states skipped");
            return;
        };
        let dir = root.join("../Plugins/Fruity/Generators");
        let mut saved = testing::every_preset(&root, NAME);
        let presets = saved.len();
        saved.extend(
            testing::projects(&root)
                .into_iter()
                .filter(|saved| saved.plugin.eq_ignore_ascii_case(NAME)),
        );
        for saved in &saved {
            let mut synth = crate::generator_by_name(&saved.plugin).unwrap();
            synth.set_resource_dir(&dir);
            synth.set_sample_rate(44_100.0);
            if let Err(error) = synth.set_state(&saved.state) {
                panic!("{}: {error}", saved.file);
            }
            synth.note_on(note(1, 60, 1.0));
            let (mut left, mut right) = (vec![0.0; 22_050], vec![0.0; 22_050]);
            for (l, r) in left.chunks_mut(512).zip(right.chunks_mut(512)) {
                synth.process(l, r);
            }
            assert!(
                left.iter()
                    .chain(&right)
                    .all(|v| v.is_finite() && v.abs() < 8.0),
                "{}",
                saved.file
            );
            assert!(left.iter().any(|v| *v != 0.0), "{}", saved.file);
            assert_eq!(synth.fidelity(), Fidelity::Exact, "{}", saved.file);
        }
        eprintln!("{presets} presets, {} in projects", saved.len() - presets);
        assert!(saved.len() - presets >= 12);
    }
}
