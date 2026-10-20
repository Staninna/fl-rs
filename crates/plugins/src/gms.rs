//! GMS, the Groove Machine Synth: FL Groove's synth in its plugin wrapper,
//! as `docs/plugins/gms.md` reads it out of `GMS_x64.dll`. The effects
//! after it are in [`crate::gms_fx`].
//!
//! # The state
//!
//! `SSLF`, the length of what follows as a u32, then the synth's chunk: the
//! magic `Syn4` (`Syn3` in the presets, `S4DE` from the demo), seven
//! 512-byte paths of which the first three are the oscillators' waves,
//! three i32 (not in `Syn3`), the 212 parameters as f32 from 0 to 1, and a
//! 512-byte preset name. A chunk that ends early leaves the parameters past
//! it as they were.
//!
//! # A note
//!
//! Three wavetable oscillators, each a WAV from `Synth Waves` made into 512
//! band-limited tables, up to 16 unison voices with detune and stereo
//! spread, noise, and a modulation of oscillator 1 by oscillator 2 (hard
//! sync, phase or ring). Then an amp envelope, two decay envelopes and two
//! LFOs, each to one of four places, and a three-slot modulation matrix.
//! The voices run through a four-pole ladder with a soft clip, four times a
//! sample, and the sum through the effects and the channel's level and pan.
//! The table reads are dithered by FL's noise generator; it is one for
//! every plugin in FL's process and starts at 0 here, so the dither and the
//! noise are FL's in kind and not sample for sample.
//!
//! What is not FL's:
//!
//! - The pitch wheel and the matrix's second and third sources are 0: no
//!   host here sends them.
//! - An LFO synced to the song reads the song's position from the frames
//!   played since the plugin was made or reset.
//! - Slide notes sound nothing.
//! - The effects say in [`Generator::fidelity`] which of them are not built.

// Limits are `max` then `min` in the binary's order, which turns a NaN into
// the lower limit where `clamp` would keep it.
#![allow(clippy::manual_clamp)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use crate::bassdrum::velocity_level;
use crate::gms_fx::{Chain, DELAY, FLANGER, PANNER, REVERB, TRANCER, UNITS};
use crate::sampler::circular;
use crate::{Fidelity, Generator, NoteOn, StateError};

/// The plugin's event 201 name.
pub const NAME: &str = "GMS";

/// How many parameters the state holds.
pub const PARAMS: usize = 212;

/// Where the waves are, under the directory
/// [`Generator::set_resource_dir`] is handed.
pub const WAVES: &str = "GMS/Synth Waves";

/// FL makes 99 voices.
const VOICES: usize = 99;

/// The most unison voices a note has.
const UNISON: usize = 16;

/// The most frames FL's wrapper renders at once.
const BLOCK: usize = 512;

/// A wave's tables: one for each harmonic count from 1 to 512, each a
/// turn in 1024 points.
const LEVELS: usize = 512;
const POINTS: usize = 1024;

const KEYS: usize = 128;

/// The chunk's magic numbers: `Syn3`, `Syn4` and the demo's `S4DE`.
const SYN3: u32 = 0x5379_6e33;
const SYN4: u32 = 0x5379_6e34;
const S4DE: u32 = 0x5334_4445;

/// How long a path or the name is.
const TEXT: usize = 0x200;

/// What a parameter starts at, by FL's default getter (`FUN_1800f1bd0`).
fn default_param(index: usize) -> f32 {
    match index {
        0 => 0.75,
        3 => 0.444_444_45,
        5 | 6 | 12 | 22 | 23 | 200 => 1.0,
        7 => 0.1,
        42 => 0.25,
        205 => 0.166_666_67,
        197 | 199 | 202 | 204 | 206 => 0.0,
        1 | 2 | 8 | 44 | 55..=60 | 198 | 201 | 203 | 207..=211 => 0.5,
        117..=136 => 0.5,
        137..=146 => 0.556_666_7,
        177..=186 => 0.5,
        _ => 0.0,
    }
}

/// `cvttss2si`: a float to an i32, and `i32::MIN` for what does not fit.
fn trunc(x: f32) -> i32 {
    if x.is_nan() || !(-2_147_483_648.0..2_147_483_648.0).contains(&x) {
        i32::MIN
    } else {
        x as i32
    }
}

/// `FUN_1800ef900`: decibels to a gain, nothing from -80 down.
fn db(x: f32) -> f32 {
    if x < -80.0 { 0.0 } else { powf(10.0, x * 0.05) }
}

fn powf(base: f32, exponent: f32) -> f32 {
    f64::from(base).powf(f64::from(exponent)) as f32
}

/// 0 to 1, as `maxss` then `minss` have it.
fn unit(x: f32) -> f32 {
    let x = if x > 0.0 { x } else { 0.0 };
    if x < 1.0 { x } else { 1.0 }
}

/// FL's noise generator (`FUN_1800ef8c0`), and its value as a float from 0
/// to 1 (`FUN_1800ef8e0`).
#[derive(Debug, Clone, Copy, Default)]
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(0x0bb3_8435).wrapping_add(0x3619_636b);
        self.0
    }

    fn unit(&mut self) -> f32 {
        self.next() as f32 * 2.328_306_4e-10
    }
}

/// `FUN_1800efa80`: a synced rate as a length in beats.
fn beats(rate: f32) -> f32 {
    match trunc(rate * 9.0) {
        i if i < 1 => 0.125,
        1 => 0.25,
        2 => 0.375,
        3 => 0.5,
        4 => 0.75,
        6 => 2.0,
        7 => 4.0,
        i if i > 7 => 8.0,
        _ => 1.0,
    }
}

/// `FUN_1800efb10`: an LFO's rate in hertz. Synced, it can set the phase
/// to the song's position.
fn lfo_hz(rate: f32, sync: f32, tempo: f32, pos: f32, phase: Option<&mut f32>) -> f32 {
    let mode = trunc(sync + sync);
    if mode == 0 {
        let v = 1.0 - rate;
        let v = if v <= 0.0 { 0.0 } else { v };
        let v = if 1.0 <= v { 1.0 } else { v };
        return (f64::from(v * v * 25.0) + 0.01) as f32;
    }
    let mut len = beats(rate);
    if mode == 2 {
        len *= 4.0;
    }
    if let Some(phase) = phase
        && 0.0 <= pos
    {
        *phase = (pos * 0.25) / len;
    }
    if 0.0 < tempo {
        let period = (len * 60.0) / tempo;
        if period != 0.0 {
            return 1.0 / period;
        }
    }
    0.0
}

/// `FUN_1800ef9f0`: an LFO's shape at a phase.
fn lfo_shape(shape: i32, phase: f32) -> f32 {
    match shape {
        0 => (f64::from(phase) * std::f64::consts::TAU).sin() as f32,
        1 => {
            let v = phase + phase;
            if 1.0 < v { 2.0 - v } else { v }
        }
        2 => 1.0 - phase,
        _ => {
            if phase < 0.5 {
                1.0
            } else {
                0.0
            }
        }
    }
}

/// `FUN_1800ef930`: a frequency as a 32-bit phase step.
fn phase_step(freq: f32, rate: f32) -> u32 {
    let f = if freq > 0.0 { freq } else { 0.0 };
    let top = rate * 0.4999;
    let f = if f < top { f } else { top };
    (f64::from(f / rate) * 4_294_967_295.0) as i64 as u32
}

/// `FUN_1800ef960`: a phase moved on by `turns`, in FL's fixed point where
/// a quarter turn is about 2^30.
fn phase_add(phase: u32, turns: f64) -> u32 {
    let v = (turns * 4_194_303.0 + 268_435_392.0) as i64 as u32;
    (v << 8).wrapping_add(phase)
}

/// `FUN_1800ef980`: the table a frequency reads, the one with as many
/// harmonics as fit under half the rate.
fn table_level(freq: f32, rate: f32) -> usize {
    if rate * (1.0 / 1024.0) < freq {
        let v = trunc((0.5 / freq) * rate);
        v.clamp(0, LEVELS as i32 - 1) as usize
    } else {
        LEVELS - 1
    }
}

/// `FUN_1800ef810`: whether a matrix slot's destination number is this
/// parameter.
fn targets(param: usize, dest: i32) -> bool {
    let to = match dest {
        1 => 12,
        2 => 13,
        4 => 50,
        6 => 20,
        7 => 22,
        8 | 12 => 16,
        10 => 21,
        11 => 23,
        14 => 34,
        15 => 28,
        17 => 35,
        18 => 29,
        20 => 4,
        21 => 5,
        _ => return false,
    };
    param == to
}

/// The ladder's cutoff table (built in `FUN_1800f0b20`): for each of 65536
/// steps of the cutoff, the stages' coefficient and the resonance gain.
fn ladder_table() -> &'static [(f32, f32)] {
    static TABLE: OnceLock<Vec<(f32, f32)>> = OnceLock::new();
    TABLE.get_or_init(|| {
        (0..65_536)
            .map(|i| {
                let g = powf(2.0, i as f32 * 0.000_149_133_69) * 0.000_5;
                let k = (f64::from(g * 7.5 + 5.0) * 0.756_525) as f32;
                (g, k)
            })
            .collect()
    })
}

/// `FUN_1800dfa30`: a WAV read between two points with a cubic of four,
/// one point late as FL reads it. Past the end is silence.
fn hermite(wave: &[f32], at: usize, frac: f32) -> f32 {
    if at >= wave.len() {
        return 0.0;
    }
    let p = |k: usize| wave.get(at + k).copied().unwrap_or(0.0);
    let (p0, p1, p2, p3) = (p(0), p(1), p(2), p(3));
    let c1 = (p2 - p0) * 0.5;
    let c3 = (p3 - p0) * 0.5 + (p1 - p2) * 1.5;
    (((((p0 - p1) + c1) - c3) + c3 * frac) * frac + c1) * frac + p1
}

/// `FUN_1800f1570`: a single-cycle wave as 512 tables. It is resampled to
/// 1024 points, taken apart into harmonics (IPP's forward transform, scaled
/// by 1/N) and summed again as cosines, one more harmonic a table.
fn build_tables(wave: &[f32]) -> Vec<f32> {
    let len = wave.len() as f64;
    let mut points = [0.0f32; POINTS];
    for (i, point) in points.iter_mut().enumerate() {
        let pos = i as f64 * len * 0.000_976_562_5;
        let at = pos as i32;
        let frac = (pos - f64::from(at)) as f32;
        *point = hermite(wave, at.max(0) as usize, frac);
    }
    let cos: Vec<f64> = (0..POINTS)
        .map(|n| (std::f64::consts::TAU * n as f64 / POINTS as f64).cos())
        .collect();
    let mut magnitude = [0.0f32; LEVELS + 1];
    let mut phase = [0.0f32; LEVELS + 1];
    for k in 1..=LEVELS {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (n, x) in points.iter().enumerate() {
            let at = (k * n) % POINTS;
            re += f64::from(*x) * cos[at];
            im -= f64::from(*x) * cos[(at + POINTS * 3 / 4) % POINTS];
        }
        re /= POINTS as f64;
        im /= POINTS as f64;
        magnitude[k] = re.hypot(im) as f32;
        phase[k] = im.atan2(re) as f32;
    }
    let mut tables = vec![0.0f32; LEVELS * POINTS];
    let mut sum = [0.0f32; POINTS];
    for h in 0..LEVELS {
        let amp = f64::from(magnitude[h + 1]);
        let ph = f64::from(phase[h + 1]);
        let w = f64::from(h as f32 + 1.0) * 0.006_135_923_151_542_565;
        for (j, s) in sum.iter_mut().enumerate() {
            let x = (j as f32) as f64 * w + ph;
            *s = (x.cos() * amp + f64::from(*s)) as f32;
        }
        tables[h * POINTS..(h + 1) * POINTS].copy_from_slice(&sum);
    }
    tables
}

/// The first channel of a WAV file as floats: PCM of 8 to 32 bits or
/// float. `None` for anything else.
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
                let width = usize::from(bits / 8);
                let frame = usize::from(channels) * width;
                if frame == 0 {
                    return None;
                }
                let first = |f: &[u8]| -> Option<f32> {
                    Some(match (tag, bits) {
                        (1, 8) => (f32::from(f[0]) - 128.0) / 128.0,
                        (1, 16) => f32::from(i16::from_le_bytes([f[0], f[1]])) / 32_768.0,
                        (1, 24) => {
                            (i32::from_le_bytes([0, f[0], f[1], f[2]]) >> 8) as f32 / 8_388_608.0
                        }
                        (1, 32) => {
                            i32::from_le_bytes([f[0], f[1], f[2], f[3]]) as f32 / 2_147_483_648.0
                        }
                        (3, 32) => f32::from_le_bytes([f[0], f[1], f[2], f[3]]),
                        _ => return None,
                    })
                };
                return body.chunks_exact(frame).map(first).collect();
            }
            _ => {}
        }
        at = at.checked_add(8 + size + (size & 1))?;
    }
}

/// The file a state's path names, as FL looks for it in its own `Synth
/// Waves`: the last part of the path, with the wave FL renamed under its
/// new name.
fn wave_file(path: &str) -> &str {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    if name.eq_ignore_ascii_case("Devils Horn.wav") {
        "Double Spike.wav"
    } else {
        name
    }
}

/// The file of this name in a directory, read; case does not count.
fn read_named(dir: &Path, name: &str) -> Option<Vec<u8>> {
    let entries = std::fs::read_dir(dir).ok()?;
    let found = entries.flatten().find(|entry| {
        entry
            .file_name()
            .to_str()
            .is_some_and(|n| n.eq_ignore_ascii_case(name))
    })?;
    std::fs::read(found.path()).ok()
}

/// One of FL's 99 voices (`GMSynthVoice`).
#[derive(Debug, Clone)]
struct Voice {
    active: bool,
    released: bool,
    key: i32,
    counter: u32,
    target: i32,
    pitch: f32,
    velocity: f32,
    env_amount: [f32; 2],
    lfo_amount: [f32; 2],
    /// Oscillator 1, the sync master, oscillator 2 and oscillator 3, for
    /// each unison voice: phases and steps.
    phase: [[u32; UNISON]; 4],
    step: [[u32; UNISON]; 4],
    /// Left and right for oscillators 1, 2, 3 and the noise, each unison
    /// voice.
    gain: [[f32; UNISON]; 8],
    random_start: bool,
    level: [usize; 3],
    amp: f32,
    amp_target: f32,
    amp_coef: f32,
    amp_decay: f32,
    amp_sustain: f32,
    env: [f32; 2],
    env_target: [f32; 2],
    env_coef: [f32; 2],
    env_decay: [f32; 2],
    res: f32,
    cut_offset: f32,
    /// The ladder's four stages, left and right.
    ladder: [[f32; 4]; 2],
    loud: f32,
    loud_target: f32,
    lfo_value: [f32; 2],
    left: Box<[f32; BLOCK]>,
    right: Box<[f32; BLOCK]>,
    cut: Box<[f32; BLOCK]>,
    out: Box<[f32; BLOCK]>,
}

impl Voice {
    fn new() -> Self {
        Voice {
            active: false,
            released: false,
            key: -1,
            counter: 0,
            target: 0,
            pitch: 0.0,
            velocity: 0.0,
            env_amount: [0.0; 2],
            lfo_amount: [0.0; 2],
            phase: [[0; UNISON]; 4],
            step: [[0; UNISON]; 4],
            gain: [[0.0; UNISON]; 8],
            random_start: false,
            level: [LEVELS - 1; 3],
            amp: 0.0,
            amp_target: 0.0,
            amp_coef: 0.0,
            amp_decay: 0.0,
            amp_sustain: 0.0,
            env: [0.0; 2],
            env_target: [0.0; 2],
            env_coef: [0.0; 2],
            env_decay: [0.0; 2],
            res: 0.0,
            cut_offset: 0.0,
            ladder: [[0.0; 4]; 2],
            loud: 0.0,
            loud_target: 0.0,
            lfo_value: [0.0; 2],
            left: Box::new([0.0; BLOCK]),
            right: Box::new([0.0; BLOCK]),
            cut: Box::new([0.0; BLOCK]),
            out: Box::new([0.0; BLOCK]),
        }
    }

    /// `FUN_1800fe890`: the voice stops, and its unison phases start over
    /// at random.
    fn kill(&mut self, rng: &mut Rng) {
        self.active = false;
        self.released = false;
        self.key = -1;
        self.ladder = [[0.0; 4]; 2];
        self.loud = -1.0;
        self.loud_target = 1.0;
        self.amp = 0.0;
        self.env = [0.0; 2];
        for u in 0..UNISON {
            let r = rng.next();
            self.phase[1][u] = r;
            self.phase[0][u] = r;
            for osc in [2, 3] {
                for _ in 0..30 {
                    rng.next();
                }
                self.phase[osc][u] = rng.next();
            }
            for _ in 0..30 {
                rng.next();
            }
        }
    }
}

/// A table of the oscillators' and where it reads.
fn read(tables: Option<&[f32]>, level: usize, phase: u32, rng: &mut Rng) -> f32 {
    let at = ((rng.next() & 0x3f_ffff).wrapping_add(phase) >> 22) as usize;
    tables.map_or(0.0, |t| t[level * POINTS + at])
}

/// GMS. A new one is at 44.1 kHz and 120 beats a minute with FL's
/// defaults and no waves.
#[derive(Debug, Clone)]
pub struct Gms {
    params: [f32; PARAMS],
    preset: String,
    paths: [String; 3],
    slots: [i32; 3],
    tables: [Option<Arc<Vec<f32>>>; 3],
    lost: bool,
    dir: Option<PathBuf>,
    rate: f64,
    tempo: f64,
    pos: f64,
    rng: Rng,
    voices: Vec<Voice>,
    /// For each key: the left and right gains and the pitch offset of its
    /// last note, and the host's number for the note held on it.
    keys: [(f32, f32, f32); KEYS],
    held: [Option<u32>; KEYS],
    velocity: f32,
    counter: u32,
    last_pitch: f32,
    lfo_phase: [f32; 2],
    lfo_step: [f32; 2],
    cut_raw: f32,
    cut: f32,
    modulation: f32,
    filter: i32,
    offset: u32,
    kind: i32,
    unison: usize,
    env_dest: [i32; 2],
    lfo_dest: [i32; 2],
    lfo_kind: [i32; 2],
    gains: [f32; 2],
    chain: Chain,
    sounding: bool,
    out: [Box<[f32; BLOCK]>; 2],
    cut_base: Box<[f32; BLOCK]>,
    mod_base: Box<[f32; BLOCK]>,
    mod_now: Box<[f32; BLOCK]>,
    amp_now: Box<[f32; BLOCK]>,
    lfo_now: [Box<[f32; BLOCK]>; 2],
}

impl Default for Gms {
    fn default() -> Self {
        let mut rng = Rng::default();
        let voices = (0..VOICES)
            .map(|_| {
                let mut voice = Voice::new();
                voice.kill(&mut rng);
                voice
            })
            .collect();
        let mut gms = Gms {
            params: std::array::from_fn(default_param),
            preset: String::new(),
            paths: Default::default(),
            slots: [0; 3],
            tables: Default::default(),
            lost: false,
            dir: None,
            rate: 44_100.0,
            tempo: 120.0,
            pos: 0.0,
            rng,
            voices,
            keys: [(0.0, 0.0, 0.0); KEYS],
            held: [None; KEYS],
            velocity: 0.0,
            counter: 1,
            last_pitch: -1.0,
            lfo_phase: [0.0; 2],
            lfo_step: [0.0; 2],
            cut_raw: 0.0,
            cut: -1.0,
            modulation: -1.0,
            filter: 0,
            offset: 0,
            kind: 0,
            unison: 1,
            env_dest: [0; 2],
            lfo_dest: [0; 2],
            lfo_kind: [0; 2],
            gains: [-1.0; 2],
            chain: Chain::new(),
            sounding: false,
            out: [Box::new([0.0; BLOCK]), Box::new([0.0; BLOCK])],
            cut_base: Box::new([0.0; BLOCK]),
            mod_base: Box::new([0.0; BLOCK]),
            mod_now: Box::new([0.0; BLOCK]),
            amp_now: Box::new([0.0; BLOCK]),
            lfo_now: [Box::new([0.0; BLOCK]), Box::new([0.0; BLOCK])],
        };
        gms.restart();
        gms
    }
}

impl Gms {
    /// The preset name the state holds.
    pub fn preset(&self) -> &str {
        &self.preset
    }

    /// The parameters as the state holds them.
    pub fn params(&self) -> &[f32; PARAMS] {
        &self.params
    }

    /// The paths of the oscillators' waves, as the state holds them.
    pub fn waves(&self) -> &[String; 3] {
        &self.paths
    }

    fn raw(&self, index: usize) -> f32 {
        self.params[index]
    }

    /// `FUN_1800f50c0`: a parameter with the modulation matrix on it. The
    /// first slot's source is the note's velocity; the other two have none
    /// here.
    fn get(&self, index: usize) -> f32 {
        let mut v = self.raw(index);
        for (amount, dest, source) in [(198, 199, self.velocity), (201, 202, 0.0), (203, 204, 0.0)]
        {
            let to = self.raw(dest);
            if to != 0.0 && targets(index, trunc(to)) {
                let a = self.raw(amount);
                v += (((a - 0.5) + a) - 0.5) * source;
                if v <= 0.0 {
                    v = 0.0;
                }
                if 1.0 <= v {
                    v = 1.0;
                }
            }
        }
        v
    }

    /// `FUN_1800f12d0`: what a new rate starts over.
    fn restart(&mut self) {
        self.gains = [-1.0; 2];
        self.chain.set_rate(self.rate as f32);
        for voice in &mut self.voices {
            voice.kill(&mut self.rng);
        }
        self.lfo_phase = [0.0; 2];
        self.last_pitch = -1.0;
        self.counter = 1;
        self.cut = -1.0;
        self.modulation = -1.0;
    }

    /// Read the three waves from the directory FL keeps them in.
    fn load_waves(&mut self) {
        self.lost = false;
        let mut made: Vec<(String, Arc<Vec<f32>>)> = Vec::new();
        for slot in 0..3 {
            let path = self.paths[slot].clone();
            if path.is_empty() {
                continue;
            }
            let name = wave_file(&path).to_owned();
            if let Some((_, tables)) = made.iter().find(|(n, _)| n.eq_ignore_ascii_case(&name)) {
                self.tables[slot] = Some(tables.clone());
                continue;
            }
            let wave = self
                .dir
                .as_ref()
                .and_then(|dir| read_named(&dir.join(WAVES), &name))
                .and_then(|bytes| read_wav(&bytes));
            match wave {
                Some(wave) if !wave.is_empty() => {
                    let tables = Arc::new(build_tables(&wave));
                    made.push((name, tables.clone()));
                    self.tables[slot] = Some(tables);
                }
                _ => self.lost = true,
            }
        }
    }

    /// `FUN_1800f5250` and `FUN_1800f53e0`: a voice for the key.
    fn start(&mut self, key: usize, velocity: f32) {
        self.velocity = velocity;
        let pick = if self.get(40) == 0.0 {
            let free = self.voices.iter().rposition(|v| !v.active);
            free.or_else(|| {
                let mut oldest = None;
                let mut least = 0xefff_ffffu32;
                for (i, v) in self.voices.iter().enumerate() {
                    if v.counter < least {
                        oldest = Some(i);
                        least = v.counter;
                    }
                }
                oldest
            })
        } else {
            Some(0)
        };
        let Some(index) = pick else {
            return;
        };
        let target = key as i32 - trunc(self.get(2) * -48.0 + 24.0);
        let target = target + (trunc(self.get(44) * -4.0) + 2) * -12;
        let rate = self.rate;
        let p = |i| self.get(i);
        let level = p(8) * p(8);
        let (attack, decay) = (p(4), p(5) * p(5));
        let amp_attack =
            (8.82 / ((f64::from(attack * attack * attack * 50.0) + 0.001) * rate)) as f32;
        let amp_decay = (8.82 / ((f64::from(decay * decay * 50.0) + 0.001) * rate)) as f32;
        let sustain = p(6);
        let env = |a: f32, d: f32| {
            let d = d * 0.8 + 0.2;
            let attack = (8.82 / ((f64::from(a * a * a * a * a * 300.0) + 0.001) * rate)) as f32;
            let fall = d * 70.0;
            let decay = (8.82 / ((f64::from(d * d * d * d * fall) + 0.001) * rate)) as f32;
            (attack, decay)
        };
        let envs = [env(p(20), p(22)), env(p(21), p(23))];
        let slide = p(11) != 0.0 && self.last_pitch != -1.0;
        let together = p(47) != 0.0;
        let spread = p(46) * 4.0;
        let random = !together && p(53) != 0.0;
        let retrig = [p(26) != 0.0, p(27) != 0.0];
        let counter = self.counter;
        self.counter = self.counter.wrapping_add(1);
        let last = self.last_pitch;
        let voice = &mut self.voices[index];
        voice.key = key as i32;
        voice.target = target;
        voice.velocity = velocity;
        voice.counter = counter;
        voice.released = false;
        voice.active = true;
        voice.lfo_value = [0.0; 2];
        voice.loud_target = level;
        if voice.loud < 0.0 {
            voice.loud = level;
        }
        voice.amp_coef = amp_attack;
        voice.amp_decay = amp_decay;
        voice.amp_sustain = sustain;
        voice.amp_target = 5.0;
        for (e, (attack, decay)) in envs.iter().enumerate() {
            voice.env_coef[e] = *attack;
            voice.env_target[e] = 5.0;
            voice.env[e] = 0.0;
            voice.env_decay[e] = *decay;
        }
        if slide {
            voice.pitch = last;
        }
        if together {
            for u in 0..UNISON {
                let phase = phase_add(0, f64::from(u as f32 * spread));
                for osc in &mut voice.phase {
                    osc[u] = phase;
                }
            }
            voice.random_start = false;
        } else {
            voice.random_start = random;
        }
        for (lfo, again) in retrig.iter().enumerate() {
            if *again {
                self.lfo_phase[lfo] = 0.0;
            }
        }
        self.last_pitch = target as f32;
    }

    /// `FUN_1800f58e0`: every voice of the key lets go.
    fn release(&mut self, key: usize) {
        let r = (f64::from(self.get(7)) * 0.55 + 0.1) as f32;
        let coef = (8.82 / ((f64::from(r * r * r * 25.0) + 0.001) * self.rate)) as f32;
        for voice in &mut self.voices {
            if voice.key == key as i32 {
                voice.released = true;
                voice.amp_coef = coef;
                voice.amp_target = 0.0;
            }
        }
    }

    /// `FUN_1800f5f00`: the block's settings, the effects' among them.
    fn setup(&mut self) {
        let tempo = self.tempo as f32;
        let pos = (self.pos * 4.0) as f32;
        let free = [self.get(26) == 0.0, self.get(27) == 0.0];
        let rate = self.rate;
        let (a, b, c) = (self.get(34), self.get(36), self.get(35));
        let d = self.get(37);
        let mut phase = self.lfo_phase[0];
        let hz = lfo_hz(a, b, tempo, pos, free[0].then_some(&mut phase));
        self.lfo_step[0] = (f64::from(hz) / rate) as f32;
        // FL's second LFO syncs the first one's phase.
        let hz = lfo_hz(c, d, tempo, pos, free[1].then_some(&mut phase));
        self.lfo_phase[0] = phase;
        self.lfo_step[1] = (f64::from(hz) / rate) as f32;
        for i in 0..UNITS {
            let on = self.get(147 + i) != 0.0;
            let x = unit_clamp(self.get(117 + i));
            let y = unit_clamp(self.get(127 + i));
            let lfo = self.get(177 + i);
            let shape = trunc(self.get(157 + i) * 3.0).clamp(0, 4);
            let mut length = beats(self.get(137 + i));
            if self.get(167 + i) != 0.0 {
                length *= 4.0;
            }
            let unit = &mut self.chain.units[i];
            unit.bypass = !on;
            if on {
                unit.x = x;
                unit.y = y;
                let v = (lfo + lfo) - 1.0;
                unit.amount = if v < 0.0 { -(v * v) } else { v * v };
                unit.shape = shape;
                unit.length = if length < 0.0 { 0.0 } else { length };
                let at = if unit.length <= 0.0 {
                    unit.phase
                } else {
                    (0.25 / f64::from(unit.length)) * self.pos * 4.0
                };
                unit.phase = at - f64::from(at as i32);
            }
        }
        let master = self.get(206) != 0.0;
        let bands: [f32; 5] = std::array::from_fn(|b| unit_clamp(self.get(207 + b)));
        let unit = &mut self.chain.units[TRANCER];
        unit.master = master;
        if master {
            unit.bands = bands;
        }
    }

    /// `FUN_1800f68f0`: each sounding voice's pitch, tables, unison and
    /// gains for the block.
    fn prepare(&mut self) {
        let unison = (1 - trunc(self.get(41) * -15.0)).clamp(1, UNISON as i32) as usize;
        self.unison = unison;
        let detune = {
            let d = (f64::from(self.get(42)) * 0.4) as f32;
            d * d
        };
        let sq = |x: f32| x * x;
        let a = sq(self.get(48));
        let b = sq(self.get(49));
        let c = sq(self.get(45));
        let w1 = ((1.0 - a) * (1.0 - b)) * (1.0 - c);
        let w2 = ((1.0 - b) * a) * (1.0 - c);
        let w3 = (1.0 - c) * b;
        self.cut_raw = self.raw(12);
        if 0.0 > self.cut {
            self.cut = self.cut_raw;
        }
        self.filter = trunc(self.get(15) + self.get(15)) & 0xff;
        self.offset = phase_add(0, f64::from(self.get(52) + self.get(52)));
        self.kind = trunc(self.get(51) + self.get(51));
        let rate = self.rate as f32;
        for v in 0..VOICES {
            if !self.voices[v].active {
                continue;
            }
            self.velocity = self.voices[v].velocity;
            let signed = |x: f32, flip: f32| if flip != 0.0 { -(x * x) } else { x * x };
            let env_amount = [
                signed(self.get(16), self.get(18)),
                signed(self.get(17), self.get(19)),
            ];
            let lfo_amount = [
                signed(self.get(28), self.get(30)),
                signed(self.get(29), self.get(31)),
            ];
            {
                let voice = &mut self.voices[v];
                voice.env_amount = env_amount;
                voice.lfo_amount = lfo_amount;
                if voice.amp_target == 0.0 && 1e-4 > voice.amp {
                    voice.kill(&mut self.rng);
                    continue;
                }
            }
            let tracking = self.get(14);
            let pitch = self.voices[v].pitch;
            let kbd = if tracking != 0.0 {
                tracking * (pitch - 57.0) * 0.007_812_5
            } else {
                0.0
            };
            let mut loud = db(self.get(8) * 50.0 - 25.0) * 200.0;
            if self.get(200) != 0.0 {
                loud *= self.voices[v].velocity;
            }
            let cut_offset = (self.get(12) - self.cut_raw) + kbd;
            let res = self.get(13);
            let key = self.voices[v].key;
            let target = key - trunc(self.get(2) * -48.0 + 24.0);
            let target = target + (trunc(self.get(44) * -4.0) + 2) * -12;
            let glide = self.get(11) != 0.0;
            let pitch = if glide { pitch } else { target as f32 };
            let flag = |dest: i32, want: i32| if dest == want { 1.0f32 } else { 0.0 };
            let (env, lfo) = (self.voices[v].env, self.voices[v].lfo_value);
            let (env_dest, lfo_dest) = (self.env_dest, self.lfo_dest);
            let e1 = |want| flag(env_dest[0], want) * env_amount[0] * env[0];
            let e2 = |want| flag(env_dest[1], want) * env_amount[1] * env[1];
            let l1 = |want| flag(lfo_dest[0], want) * lfo[0];
            let l2 = |want| flag(lfo_dest[1], want) * lfo[1];
            let bend = trunc(self.get(205) * 12.0) as f32 * 0.0;
            let mut p = bend + pitch;
            p += (e2(2) + e1(2)) * 24.0 + (l2(2) + l1(2)) * 12.0;
            p -= 57.0;
            p += self.keys[key.clamp(0, 127) as usize].2;
            let osc = |semis: f32, cents: f32| {
                let s = trunc(semis * 72.0 - 36.0) as f32;
                let c = trunc(cents * 100.0 - 50.0);
                (f64::from(c) * 0.01 + f64::from(s)) as f32
            };
            let off1 = osc(self.get(55), self.get(56));
            let off2 = osc(self.get(57), self.get(58));
            let mut modf = 1.0;
            if self.kind == 0 {
                let m = self.get(50) + e1(1);
                let m = m + ((l2(1) + l1(1)) + e2(1));
                modf = unit(m) * 3.0 + 1.0;
            }
            let off3 = osc(self.get(59), self.get(60));
            let base = powf(2.0, p * 0.083_333_336) * 220.0;
            let f1 = powf(2.0, off1 * 0.083_333_336) * base;
            let f2 = powf(2.0, off2 * 0.083_333_336) * base;
            let f3 = powf(2.0, off3 * 0.083_333_336) * base;
            let f0 = modf * f1;
            let stereo = self.get(43);
            let (kl, kr, _) = self.keys[key.clamp(0, 127) as usize];
            let voice = &mut self.voices[v];
            voice.loud_target = loud;
            voice.cut_offset = cut_offset;
            voice.res = res;
            voice.target = target;
            voice.pitch = pitch;
            voice.level = [
                table_level(f0, rate),
                table_level(f2, rate),
                table_level(f3, rate),
            ];
            let norm = 0.005 / ((unison - 1) as f32 * 0.15 + 1.0);
            for u in 0..UNISON {
                let d = if unison > 1 {
                    powf(2.0, ((u as f32) / ((unison - 1) as f32) - 0.5) * detune)
                } else {
                    1.0
                };
                voice.step[0][u] = phase_step(f0 * d, rate);
                voice.step[1][u] = phase_step(d * f1, rate);
                voice.step[2][u] = phase_step(d * f2, rate);
                voice.step[3][u] = phase_step(d * f3, rate);
                let mut s = if u & 1 == 1 { -1.0 } else { 1.0 };
                if unison & 1 == 1 && u == 0 {
                    s = 0.0;
                }
                let s = s * stereo;
                let gl = unit(1.0 - s) * kl;
                let gr = unit(s + 1.0) * kr;
                voice.gain[0][u] = gl * w1 * norm;
                voice.gain[1][u] = gr * w1 * norm;
                voice.gain[2][u] = gl * w2 * norm;
                voice.gain[3][u] = gr * w2 * norm;
                voice.gain[4][u] = gl * w3 * norm;
                voice.gain[5][u] = gr * w3 * norm;
                voice.gain[6][u] = gl * c * norm;
                voice.gain[7][u] = gr * c * norm;
                if voice.random_start {
                    voice.phase[0][u] = 0;
                    voice.phase[2][u] = 0;
                    let count = self.rng.next().wrapping_add(1) & 0x1ff;
                    let mut lead = 0;
                    if count != 0 {
                        lead = count.wrapping_mul(voice.step[0][u]);
                        voice.phase[2][u] = count.wrapping_mul(voice.step[2][u]);
                        voice.phase[0][u] = lead;
                    }
                    voice.phase[1][u] = lead;
                }
            }
            voice.random_start = false;
        }
    }

    /// `FUN_1800f7530`: the voices' envelopes, LFOs and oscillators for the
    /// block, before the ladder.
    fn sound(&mut self, frames: usize) {
        let kind = self.kind;
        let invert = self.get(54) != 0.0;
        let slide = {
            let t = self.get(10);
            let t = t * t * t;
            (8.82 / ((f64::from(t + t) + 0.001) * self.rate)) as f32
        };
        let target_mod = self.get(50) * self.get(50);
        if self.modulation < 0.0 {
            self.modulation = target_mod;
        }
        let mut first = true;
        for v in 0..VOICES {
            if !self.voices[v].active {
                continue;
            }
            if first {
                first = false;
                for i in 0..frames {
                    self.cut = (self.cut_raw - self.cut) * 0.01 + self.cut;
                    self.cut_base[i] = self.cut;
                    self.modulation = (target_mod - self.modulation) * 0.01 + self.modulation;
                    self.mod_base[i] = self.modulation;
                    for l in 0..2 {
                        self.lfo_now[l][i] = lfo_shape(self.lfo_kind[l], self.lfo_phase[l]);
                        let p = self.lfo_step[l] + self.lfo_phase[l];
                        self.lfo_phase[l] = p - trunc(p) as f32;
                    }
                }
            }
            self.sounding = true;
            let (env_dest, lfo_dest) = (self.env_dest, self.lfo_dest);
            let voice = &mut self.voices[v];
            voice.left[..frames].fill(0.0);
            voice.right[..frames].fill(0.0);
            let (mut e, mut t, mut c) = (voice.env, voice.env_target, voice.env_coef);
            let (mut a, mut at, mut ac) = (voice.amp, voice.amp_target, voice.amp_coef);
            let mut loud = voice.loud;
            for i in 0..frames {
                for k in 0..2 {
                    e[k] += (t[k] - e[k]) * c[k];
                }
                a += (at - a) * ac;
                loud += (voice.loud_target - loud) * 0.01;
                voice.pitch = (voice.target as f32 - voice.pitch) * slide + voice.pitch;
                for k in 0..2 {
                    if trunc(e[k]) != 0 {
                        c[k] = voice.env_decay[k];
                        e[k] = 1.0;
                        t[k] = 0.0;
                    }
                }
                if trunc(a) != 0 {
                    ac = voice.amp_decay;
                    at = voice.amp_sustain;
                    a = 1.0;
                }
                voice.out[i] = loud;
                let mut l1 = self.lfo_now[0][i] * voice.lfo_amount[0];
                if env_dest[0] == 3 {
                    l1 *= e[0];
                }
                let mut l2 = self.lfo_now[1][i] * voice.lfo_amount[1];
                if env_dest[1] == 3 {
                    l2 *= e[1];
                }
                voice.lfo_value = [l1, l2];
                if kind == 1 || kind == 2 {
                    let mut m = self.mod_base[i];
                    if env_dest[0] == 1 {
                        m += e[0] * voice.env_amount[0];
                    }
                    if env_dest[1] == 1 {
                        m += e[1] * voice.env_amount[1];
                    }
                    if lfo_dest[0] == 1 {
                        m += l1;
                    }
                    if lfo_dest[1] == 1 {
                        m += l2;
                    }
                    if m <= 0.0 {
                        m = 0.0;
                    }
                    self.mod_now[i] = m * if kind == 1 { 15.0 } else { 3.0 };
                }
                let mut g = a * a;
                if lfo_dest[0] == 3 {
                    g += l1;
                }
                if lfo_dest[1] == 3 {
                    g += l2;
                }
                if g <= 0.0 {
                    g = 0.0;
                }
                self.amp_now[i] = g;
                let mut cut = self.cut_base[i] + voice.cut_offset;
                if env_dest[0] == 0 {
                    cut += e[0] * voice.env_amount[0];
                }
                if env_dest[1] == 0 {
                    cut += e[1] * voice.env_amount[1];
                }
                if lfo_dest[0] == 0 {
                    cut += l1;
                }
                if lfo_dest[1] == 0 {
                    cut += l2;
                }
                voice.cut[i] = cut;
            }
            voice.env = e;
            voice.env_target = t;
            voice.env_coef = c;
            voice.amp = a;
            voice.amp_target = at;
            voice.amp_coef = ac;
            voice.loud = loud;
            let tables: [Option<&[f32]>; 3] =
                std::array::from_fn(|k| self.tables[k].as_deref().map(Vec::as_slice));
            let rng = &mut self.rng;
            for u in 0..self.unison {
                let g: [f32; 8] = std::array::from_fn(|k| voice.gain[k][u]);
                for i in 0..frames {
                    let at2 = self.offset.wrapping_add(voice.phase[2][u]);
                    let mut o2 = read(tables[1], voice.level[1], at2, rng);
                    if invert {
                        o2 = -o2;
                    }
                    let o1 = match kind {
                        1 => {
                            let at = phase_add(voice.phase[0][u], f64::from(o2 * self.mod_now[i]));
                            read(tables[0], voice.level[0], at, rng)
                        }
                        0 => {
                            let old = voice.phase[1][u];
                            let new = old.wrapping_add(voice.step[1][u]);
                            voice.phase[1][u] = new;
                            if new < old {
                                voice.phase[0][u] = new;
                            }
                            read(tables[0], voice.level[0], voice.phase[0][u], rng)
                        }
                        _ => {
                            let o = read(tables[0], voice.level[0], voice.phase[0][u], rng);
                            if kind == 2 {
                                o * (1.0 - o2 * self.mod_now[i])
                            } else {
                                o
                            }
                        }
                    };
                    let o3 = read(tables[2], voice.level[2], voice.phase[3][u], rng);
                    let noise = rng.unit() - 0.5;
                    voice.left[i] =
                        (o3 * g[4] + (noise * g[6] + voice.left[i])) + (o1 * g[0] + o2 * g[2]);
                    voice.right[i] =
                        ((noise * g[7] + voice.right[i]) + o3 * g[5]) + (o1 * g[1] + o2 * g[3]);
                    for osc in [0, 2, 3] {
                        voice.phase[osc][u] = voice.phase[osc][u].wrapping_add(voice.step[osc][u]);
                    }
                }
            }
            for i in 0..frames {
                voice.left[i] *= self.amp_now[i];
                voice.right[i] *= self.amp_now[i];
            }
        }
    }

    /// `FUN_1800f32f0`: one block of at most [`BLOCK`] frames into
    /// [`Gms::out`].
    fn block(&mut self, frames: usize) {
        let vol = db(self.raw(0) * 80.0 - 60.0);
        let pan = self.raw(1);
        let [left, right] = &mut self.out;
        left[..frames].fill(0.0);
        right[..frames].fill(0.0);
        self.sounding = false;
        self.setup();
        if self.voices.iter().any(|v| v.active) {
            let dest = |x: f32| trunc(x * 3.0);
            self.env_dest = [dest(self.get(24)), dest(self.get(25))];
            self.lfo_dest = [dest(self.get(38)), dest(self.get(39))];
            self.lfo_kind = [dest(self.get(32)), dest(self.get(33))];
            self.prepare();
            self.sound(frames);
        }
        // The ladder, voice by voice in FL's order.
        let table = ladder_table();
        let kind = self.filter;
        let [left, right] = &mut self.out;
        for voice in self.voices.iter_mut().filter(|v| v.active) {
            let res = voice.res;
            for i in 0..frames {
                let at = trunc(voice.cut[i] * 65_536.0).clamp(0, 65_535) as usize;
                let (g, k) = table[at];
                let amp = voice.out[i];
                for (side, input, out) in [
                    (0, voice.left[i], &mut left[i]),
                    (1, voice.right[i], &mut right[i]),
                ] {
                    let y = &mut voice.ladder[side];
                    let mut s = 0.0;
                    for _ in 0..4 {
                        let u = input - k * res * y[3];
                        s = u / (u * u * 0.04 + 1.0);
                        y[0] += (s - y[0]) * g;
                        y[1] += (y[0] - y[1]) * g;
                        y[2] += (y[1] - y[2]) * g;
                        y[3] += (y[2] - y[3]) * g;
                    }
                    match kind {
                        0 => *out += y[3] * amp,
                        1 => *out += (y[2] - y[3]) * amp,
                        2 => *out += (s - y[3]) * amp,
                        _ => {}
                    }
                }
            }
        }
        let tempo = self.tempo as f32;
        let any = self.chain.run(
            &mut left[..frames],
            &mut right[..frames],
            tempo,
            self.sounding,
        );
        let tr = (unit(pan) * vol).max(1e-4);
        let tl = (unit(1.0 - pan) * vol).max(1e-4);
        if !any {
            self.gains = [tl, tr];
        } else {
            let mut now = self.gains[0];
            if self.gains[0] < 0.0 {
                self.gains[0] = tl;
                now = tl;
            }
            let read = self.gains[1];
            if read < 0.0 {
                self.gains[0] = tr;
                now = tr;
            }
            if now < 1e-4 {
                self.gains[0] = 0.0;
            }
            if read < 1e-4 {
                self.gains[1] = 0.0;
            }
            for i in 0..frames {
                self.gains[0] = (tl - self.gains[0]) * 0.01 + self.gains[0];
                self.gains[1] = (tr - self.gains[1]) * 0.01 + self.gains[1];
                left[i] *= self.gains[0];
                right[i] *= self.gains[1];
            }
        }
        self.pos += self.tempo * (1.0 / 60.0) * frames as f64 * (1.0 / self.rate);
    }
}

fn unit_clamp(x: f32) -> f32 {
    let x = if x <= 0.0 { 0.0 } else { x };
    if 1.0 <= x { 1.0 } else { x }
}

impl Generator for Gms {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let Some(head) = bytes.first_chunk::<8>() else {
            return Err(StateError::Length {
                expected: 8,
                found: bytes.len(),
            });
        };
        let word = |at: usize| u32::from_le_bytes(head[at..at + 4].try_into().unwrap());
        if &head[..4] != b"SSLF" {
            return Err(StateError::Version {
                found: word(0) as i32,
            });
        }
        let len = word(4) as usize;
        let wanted = 4 + 7 * TEXT;
        let Some(chunk) = bytes.get(8..8 + len).filter(|c| c.len() >= wanted) else {
            return Err(StateError::Length {
                expected: 8 + len.max(wanted),
                found: bytes.len(),
            });
        };
        let magic = u32::from_le_bytes(chunk[..4].try_into().unwrap());
        if ![SYN3, SYN4, S4DE].contains(&magic) {
            return Err(StateError::Version {
                found: magic as i32,
            });
        }
        let text = |b: &[u8]| -> String {
            let end = b.iter().position(|c| *c == 0).unwrap_or(b.len());
            b[..end].iter().map(|c| char::from(*c)).collect()
        };
        for slot in 0..3 {
            self.paths[slot] = text(&chunk[4 + slot * TEXT..4 + (slot + 1) * TEXT]);
        }
        let mut rest = &chunk[wanted..];
        if magic != SYN3 && rest.len() >= 12 {
            for slot in 0..3 {
                self.slots[slot] =
                    i32::from_le_bytes(rest[slot * 4..slot * 4 + 4].try_into().unwrap());
            }
            rest = &rest[12..];
        }
        let count = (rest.len() / 4).min(PARAMS);
        for (param, word) in self.params.iter_mut().zip(rest.as_chunks::<4>().0) {
            *param = f32::from_le_bytes(*word);
        }
        rest = &rest[count * 4..];
        if count == PARAMS && rest.len() >= TEXT {
            self.preset = text(&rest[..TEXT])
                .trim_end_matches(|c: char| (c as u32) < 0x20)
                .to_owned();
        }
        self.load_waves();
        Ok(())
    }

    /// The voices and the effects start over, as FL's do.
    fn set_sample_rate(&mut self, hz: f32) {
        self.rate = f64::from(hz);
        self.restart();
    }

    fn set_tempo(&mut self, bpm: f64) {
        self.tempo = bpm;
    }

    fn set_resource_dir(&mut self, dir: &Path) {
        if self.dir.as_deref() != Some(dir) {
            self.dir = Some(dir.to_path_buf());
            if self.paths.iter().any(|p| !p.is_empty()) {
                self.load_waves();
            }
        }
    }

    fn note_on(&mut self, note: NoteOn) {
        if note.slide {
            return;
        }
        let key = note.key.clamp(0, KEYS as i16 - 1) as usize;
        let (left, right) = circular(note.pan, note.velocity);
        self.keys[key] = (left, right, (f64::from(note.fine_pitch) * 0.01) as f32);
        let velocity = velocity_level(note.velocity);
        if velocity == 0.0 {
            self.held[key] = None;
            self.velocity = 0.0;
            self.release(key);
            return;
        }
        self.held[key] = Some(note.voice);
        self.start(key, velocity);
    }

    fn note_off(&mut self, voice: u32) {
        if let Some(key) = self.held.iter().position(|h| *h == Some(voice)) {
            self.held[key] = None;
            self.release(key);
        }
    }

    fn all_notes_off(&mut self) {
        for key in 0..KEYS {
            if self.held[key].take().is_some() {
                self.release(key);
            }
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let mut done = 0;
        while done < left.len() {
            let frames = (left.len() - done).min(BLOCK);
            self.block(frames);
            for i in 0..frames {
                left[done + i] += self.out[0][i];
                right[done + i] += self.out[1][i];
            }
            done += frames;
        }
    }

    fn param_count(&self) -> usize {
        PARAMS
    }

    /// The parameter as FL's automation has it: `int(256 v)`.
    fn param(&self, index: usize) -> f32 {
        self.params
            .get(index)
            .map_or(0.0, |value| (value * 256.0) as i32 as f32)
    }

    /// `int / 256`, 0 to 1.
    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(param) = self.params.get_mut(index) {
            let value = f64::from(value.round().clamp(-1e9, 1e9) as i32) * (1.0 / 256.0);
            *param = (value as f32).max(0.0).min(1.0);
        }
    }

    fn reset(&mut self) {
        self.held = [None; KEYS];
        self.pos = 0.0;
        self.restart();
    }

    /// The release from full to under 0.0001, and what the delay and the
    /// reverb hold after it.
    fn tail(&self) -> usize {
        let r = (f64::from(self.get(7)) * 0.55 + 0.1) as f32;
        let coef = 8.82 / ((f64::from(r * r * r * 25.0) + 0.001) * self.rate);
        let release = (1e-4f64).ln() / (1.0 - coef.min(0.5)).ln();
        let mut seconds = 0.5;
        if self.get(147 + DELAY) != 0.0 {
            seconds += 1.5 * 8.0;
        }
        if self.get(147 + REVERB) != 0.0 {
            let x = self.get(117 + REVERB);
            let rt = ((x * x * x * 0.75 * 199.0 + 1.5).floor() * 0.1) as f64;
            seconds += rt;
        }
        (release + seconds * self.rate).ceil() as usize
    }

    fn fidelity(&self) -> Fidelity {
        if self.lost {
            Fidelity::Approximate("GMS waves not found")
        } else if self.chain.missing
            || [FLANGER, PANNER, TRANCER]
                .iter()
                .any(|i| self.get(147 + i) != 0.0)
        {
            Fidelity::Approximate("GMS Flanger, Panner and Trancer left out")
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
            velocity: 0.78,
            pan: 0.0,
            fine_pitch: 0.0,
            release: 0.5,
            mod_x: 0.0,
            mod_y: 0.0,
            slide: false,
            porta: false,
        }
    }

    /// A `Syn4` state as FL saves it.
    fn state(waves: [&str; 3], params: &[f32; PARAMS]) -> Vec<u8> {
        let mut chunk = SYN4.to_le_bytes().to_vec();
        for k in 0..7 {
            let mut path = [0u8; TEXT];
            if let Some(w) = waves.get(k) {
                path[..w.len()].copy_from_slice(w.as_bytes());
            }
            chunk.extend(path);
        }
        chunk.extend([0u8; 12]);
        for v in params {
            chunk.extend(v.to_le_bytes());
        }
        let mut name = [2u8; TEXT];
        name[..4].copy_from_slice(b"Test");
        name[4] = 0;
        chunk.extend(name);
        let mut bytes = b"SSLF".to_vec();
        bytes.extend((chunk.len() as u32).to_le_bytes());
        bytes.extend(chunk);
        bytes
    }

    fn play(gms: &mut Gms, frames: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
        for (l, r) in left.chunks_mut(256).zip(right.chunks_mut(256)) {
            gms.process(l, r);
        }
        (left, right)
    }

    /// A saw as a 1024-point 16-bit WAV.
    fn saw_wav() -> Vec<u8> {
        let mut data = Vec::new();
        for n in 0..1024 {
            let v = (n as f32 / 512.0 - 1.0) * 30_000.0;
            data.extend((v as i16).to_le_bytes());
        }
        let mut bytes = b"RIFF".to_vec();
        bytes.extend(((36 + data.len()) as u32).to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16u32.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(44_100u32.to_le_bytes());
        bytes.extend(88_200u32.to_le_bytes());
        bytes.extend(2u16.to_le_bytes());
        bytes.extend(16u16.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend((data.len() as u32).to_le_bytes());
        bytes.extend(data);
        bytes
    }

    /// A directory laid out as FL's `Plugins/Fruity/Generators`, with a saw
    /// for a wave.
    fn install() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gms-test-{}", std::process::id()));
        let waves = dir.join(WAVES);
        std::fs::create_dir_all(&waves).unwrap();
        std::fs::write(waves.join("Saw.wav"), saw_wav()).unwrap();
        dir
    }

    #[test]
    fn a_state_reads_back_its_waves_name_and_parameters() {
        let mut params: [f32; PARAMS] = std::array::from_fn(default_param);
        params[12] = 0.25;
        let mut gms = Gms::default();
        let path = "C:\\Program Files\\GMS\\Synth Waves\\Saw.wav";
        gms.set_state(&state([path, "", "Devils Horn.wav"], &params))
            .unwrap();
        assert_eq!(gms.preset(), "Test");
        assert_eq!(gms.params(), &params);
        assert_eq!(gms.waves()[0], path);
        assert_eq!(wave_file(path), "Saw.wav");
        assert_eq!(wave_file(&gms.waves()[2]), "Double Spike.wav");
        assert_eq!(gms.param(12), 64.0);
        // No resource directory: the waves are missing, and say so.
        assert!(matches!(gms.fidelity(), Fidelity::Approximate(_)));
        let mut bad = state(["", "", ""], &params);
        bad[8] = b'1';
        assert!(gms.set_state(&bad).is_err());
        assert!(gms.set_state(&state(["", "", ""], &params)[..100]).is_err());
    }

    #[test]
    fn a_sine_makes_tables_of_one_harmonic_at_half_its_height() {
        let sine: Vec<f32> = (0..1024)
            .map(|n| (std::f32::consts::TAU * n as f32 / 1024.0).sin())
            .collect();
        let tables = build_tables(&sine);
        for level in [0, 7, 511] {
            let t = &tables[level * POINTS..(level + 1) * POINTS];
            let peak = t.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!((peak - 0.5).abs() < 2e-3, "{level} {peak}");
        }
        // A saw keeps more of its harmonics in the higher tables.
        let saw: Vec<f32> = (0..1024).map(|n| n as f32 / 512.0 - 1.0).collect();
        let tables = build_tables(&saw);
        let power = |level: usize| {
            let t = &tables[level * POINTS..(level + 1) * POINTS];
            testing::partial(t, 5.0 / 1024.0)
        };
        assert!(power(0) < 1e-4);
        assert!(power(10) > 0.05);
    }

    #[test]
    fn a_note_sounds_lets_go_and_stops() {
        let dir = install();
        let mut params: [f32; PARAMS] = std::array::from_fn(default_param);
        params[7] = 0.1;
        params[12] = 0.8;
        let mut gms = Gms::default();
        gms.set_resource_dir(&dir);
        gms.set_state(&state(["Saw.wav", "", ""], &params)).unwrap();
        assert_eq!(gms.fidelity(), Fidelity::Exact);
        gms.note_on(note(1, 57));
        let (left, right) = play(&mut gms, 22_050);
        assert!(left.iter().chain(&right).all(|v| v.is_finite()));
        let peak = left.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.05 && peak < 4.0, "{peak}");
        // A centred note on a centred channel: both sides alike once the
        // right gain, which FL starts at 0, has caught up.
        assert_ne!(left[..64], right[..64]);
        let apart = left[4096..].iter().zip(&right[4096..]);
        assert!(apart.map(|(l, r)| (l - r).abs()).fold(0.0, f32::max) < 1e-6);
        // A4 at 220 Hz: the saw's fundamental is there.
        let tone = testing::partial(&left[11_025..11_025 + 4410], 220.0 / 44_100.0);
        assert!(tone > 0.01, "{tone}");
        gms.note_off(1);
        let (left, _) = play(&mut gms, 88_200);
        assert!(left[80_000..].iter().all(|v| v.abs() < 1e-5));
        assert!(gms.voices.iter().all(|v| !v.active));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_ladder_closes_with_the_cutoff() {
        let dir = install();
        let level = |cutoff: f32| {
            let mut params: [f32; PARAMS] = std::array::from_fn(default_param);
            params[12] = cutoff;
            let mut gms = Gms::default();
            gms.set_resource_dir(&dir);
            gms.set_state(&state(["Saw.wav", "", ""], &params)).unwrap();
            gms.note_on(note(1, 69));
            let (left, _) = play(&mut gms, 8192);
            left[4096..].iter().map(|v| v * v).sum::<f32>()
        };
        let (open, shut) = (level(1.0), level(0.1));
        assert!(shut < open * 0.05, "{open} {shut}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn mono_takes_one_voice_and_poly_the_last_free_one() {
        let mut params: [f32; PARAMS] = std::array::from_fn(default_param);
        params[40] = 1.0;
        let mut gms = Gms::default();
        gms.set_state(&state(["", "", ""], &params)).unwrap();
        gms.note_on(note(1, 60));
        gms.note_on(note(2, 64));
        assert_eq!(gms.voices.iter().filter(|v| v.active).count(), 1);
        assert!(gms.voices[0].active);
        params[40] = 0.0;
        let mut gms = Gms::default();
        gms.set_state(&state(["", "", ""], &params)).unwrap();
        gms.note_on(note(1, 60));
        gms.note_on(note(2, 64));
        assert!(gms.voices[VOICES - 1].active && gms.voices[VOICES - 2].active);
    }

    #[test]
    fn every_state_fl_ships_loads_and_plays() {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, GMS's states skipped");
            return;
        };
        let dir = root.join("../Plugins/Fruity/Generators");
        let saved: Vec<_> = testing::projects(&root)
            .into_iter()
            .filter(|saved| saved.plugin.eq_ignore_ascii_case(NAME))
            .collect();
        for saved in &saved {
            let mut gms = crate::generator_by_name(&saved.plugin).unwrap();
            gms.set_sample_rate(44_100.0);
            gms.set_resource_dir(&dir);
            if let Err(error) = gms.set_state(&saved.state) {
                panic!("{}: {error}", saved.file);
            }
            for (voice, key) in [(1, 48), (2, 55), (3, 60)] {
                gms.note_on(note(voice, key));
            }
            let (mut left, mut right) = (vec![0.0; 44_100], vec![0.0; 44_100]);
            for (l, r) in left.chunks_mut(512).zip(right.chunks_mut(512)) {
                gms.process(l, r);
            }
            let peak = left
                .iter()
                .chain(&right)
                .fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(
                left.iter().chain(&right).all(|v| v.is_finite()) && peak < 8.0,
                "{}: {peak}",
                saved.file
            );
            assert!(peak > 0.01, "{}: {peak}", saved.file);
            assert_eq!(gms.fidelity(), Fidelity::Exact, "{}", saved.file);
            eprintln!("{}: peak {peak:.3}", saved.file);
        }
        // Five demo songs and the render test hold ten.
        assert_eq!(saved.len(), 10);
    }
}
