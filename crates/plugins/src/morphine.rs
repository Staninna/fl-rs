//! Morphine: FL's additive resynthesis synth, as `docs/plugins/morphine.md`
//! reads it out of `Morphine_x64.dll`.
//!
//! Four generators each play an analysed sound: a run of frames, each the
//! amplitude, pan and detune of 128 harmonics, read along at a rate and
//! looped. A point that walks a line over the square between them (the
//! morph line) weighs the four spectra into one, which a bank of 128
//! sine oscillators plays at the note's pitch under a master ADSR. Four
//! envelopes and a matrix of twelve routes modulate the rest. The voices
//! go through overdrive, chorus, delay, reverb and an eight-band
//! equaliser, then the master level.
//!
//! # Timing
//!
//! The engine works in control blocks of `int(0.0029 * rate)` samples.
//! Every block it moves the envelopes and the master ADSR on; every third
//! one it builds the spectrum anew and sets each oscillator ramping to it
//! over the next three blocks. The ADSR's level reaches the samples
//! through a Catmull-Rom curve over its last four block values, so the
//! amplitude trails the envelope by two blocks, as FL's does.
//!
//! Frames, envelope points and the morph line count time in hundredths of
//! a second at rate 1 (or 2.4 / tempo seconds when synced).
//!
//! # What is not FL's
//!
//! See [`Generator::fidelity`] and the doc: the noise layer, the effects
//! and the release-loop jump are approximated, unison and the per-note
//! tuning a host can send are left out.

use crate::morphine_fx::Fx;
use crate::morphine_noise::{NoiseBlock, NoiseRun, NoiseSample};
use crate::morphine_state::{self, Eg, Frames, PARAMS, PARTIALS, Program};
use crate::{Fidelity, Generator, NoteOn, StateError};
use std::f64::consts::PI;
use std::path::{Path, PathBuf};

/// The plugin's event 201 name.
pub const NAME: &str = "Morphine";

/// The most voices the engine sounds (`this+0x38e8`).
const VOICES: usize = 32;
/// Seconds in a control block (`DAT_180177030`).
const BLOCK: f64 = 0.0029;
/// Frames, points and the morph line count in hundredths of a second
/// (`DAT_1801772a0`), or `tempo * 0.41667` of them a second when synced
/// (`DAT_180177128`).
const UNITS: f64 = 100.0;
const SYNCED: f64 = 0.416_666_666_666_666_7;
/// No oscillator runs above this fraction of the rate (`DAT_180177130`).
const TOP: f64 = 0.4535;
/// A modulation destination nothing routes to (`FUN_1800fbbc0`).
const UNSET: f32 = -1000.0;
/// Below this an oscillator's gains count as silence (`DAT_180176fe0`).
const QUIET: f32 = 1e-6;

/// `2^x`, as the engine's `pow(2, x)` in f64.
fn exp2(x: f64) -> f64 {
    x.exp2()
}

/// Where `t` is in a loop from `start` to `end` (`FUN_1800eb970`, the
/// morph line's `FUN_180101e20`): mode 1 forwards, 2 backwards, 3 back
/// and forth, 0 none.
pub(crate) fn looped(t: f64, start: i32, end: i32, mode: i32) -> f64 {
    if mode == 0 {
        return t;
    }
    let e = f64::from(end);
    if end == start && t >= e {
        return e;
    }
    let len = end.wrapping_sub(start);
    match mode {
        1 if t > e => {
            let n = ((t - f64::from(start)) / f64::from(len)) as i32;
            t - f64::from(n.wrapping_mul(len))
        }
        2 if t > e => {
            let back = e - t;
            if back >= f64::from(start) {
                return back;
            }
            let c = ((back - e) as i32).wrapping_abs();
            let c = if len == 0 { c } else { c - c % len };
            back + f64::from(c)
        }
        3 if t >= e => {
            let d = t - e;
            let n = (d / f64::from(len)) as i32;
            let d = d - f64::from(n.wrapping_mul(len));
            if n & 1 != 0 {
                f64::from(start) + d
            } else {
                e - d
            }
        }
        _ => t,
    }
}

/// The segment `t` falls in, given each point's duration: the point, the
/// next one if there is one, and how far along. A time past the last
/// point stays on it.
fn segment(durations: impl Iterator<Item = i32>, len: usize, mut t: f64) -> (usize, usize, f64) {
    for (i, d) in durations.enumerate() {
        if i + 1 >= len {
            return (i, i, 0.0);
        }
        if t < f64::from(d) {
            return (i, i + 1, t / f64::from(d));
        }
        t -= f64::from(d);
    }
    (len.saturating_sub(1), len.saturating_sub(1), 0.0)
}

/// The value of an envelope's points at `t` (`FUN_1800f3e20`): straight
/// lines between them, the last held.
fn eg_value(eg: &Eg, t: f64) -> f32 {
    let (a, b, u) = segment(eg.points.iter().map(|p| p.duration), eg.points.len(), t);
    if a == b {
        return eg.points[a].value;
    }
    (f64::from(eg.points[a].value) * (1.0 - u) + f64::from(eg.points[b].value) * u) as f32
}

/// The rate a generator, envelope or the morph line runs at: `2^(8 (rate +
/// (velocity - 0.5) (2 vel_track - 1)) - 4 + (key - 48) / 12 (2 key_track -
/// 1))` (`FUN_18012cd10`, `FUN_1800fb9d0`, `FUN_180105df0`).
fn rate(rate: f32, vel_track: f32, key_track: f32, key: f32, velocity: f32) -> f64 {
    let key_track = f64::from(key_track - 0.5 + key_track - 0.5);
    let vel_track = f64::from(vel_track - 0.5 + vel_track - 0.5);
    exp2(
        (f64::from(velocity) - 0.5) * vel_track * 8.0 + f64::from(rate) * 8.0 - 4.0
            + f64::from(key - 48.0) * (1.0 / 12.0) * key_track,
    )
}

/// The master ADSR (`FUN_1800da760`, `FUN_1800da830`), run once a block.
#[derive(Debug, Clone, Copy, Default)]
struct Adsr {
    stage: u8,
    pos: f64,
    value: f64,
    from: f64,
    release_from: f64,
    attack: f64,
    decay: f64,
    sustain: f64,
    release: f64,
    finished: bool,
}

/// The decay and release bend: `1 - 10^(-2 x) (1 - x)` of the way
/// (`FUN_1800daa00` with the shape -2 `FUN_1800da760` sets).
fn bend(x: f64, from: f64, to: f64) -> f64 {
    (1.0 - 10f64.powf(x * -2.0) * (1.0 - x)) * (to - from) + from
}

impl Adsr {
    fn start(&mut self, rate: f64, attack: f64, decay: f64, sustain: f64) {
        self.from = if self.finished { 0.0 } else { self.value };
        self.attack = 1.0 / (attack.max(0.001) * rate);
        self.decay = 1.0 / (decay.max(0.001) * rate);
        self.sustain = sustain;
        self.finished = false;
        self.stage = 0;
        self.pos = 0.0;
        self.release = 1.0 / (0.001 * rate);
    }

    fn let_go(&mut self, rate: f64, release: f64) {
        if self.stage != 2 {
            self.release = 1.0 / (release.max(0.001) * rate);
            self.release_from = self.value;
            self.stage = 2;
            self.pos = 0.0;
        }
    }

    fn step(&mut self, n: usize) -> f32 {
        let n = n as f64;
        match self.stage {
            0 => {
                self.pos += n * self.attack;
                if self.pos >= 1.0 {
                    self.value = 1.0;
                    self.stage = 1;
                    self.pos = 0.0;
                } else {
                    self.value = (1.0 - self.from) * self.pos + self.from;
                }
            }
            1 => {
                self.pos += n * self.decay;
                if self.pos >= 1.0 {
                    if self.sustain == 0.0 {
                        self.stage = 2;
                        self.finished = true;
                    }
                    self.value = self.sustain;
                    self.pos = 1.0;
                } else {
                    self.value = bend(self.pos, 1.0, self.sustain);
                }
            }
            2 => {
                self.pos += n * self.release;
                if self.pos >= 1.0 {
                    self.finished = true;
                    self.value = 0.0;
                    self.pos = 1.0;
                } else {
                    self.value = bend(self.pos, self.release_from, 0.0);
                }
            }
            _ => self.value = 0.0,
        }
        if self.value < 0.0 {
            self.value = 0.0;
        }
        self.value as f32
    }
}

/// Where a voice is in one of the generators: its zone and its time
/// along the zone's frames (`FUN_1800eef20`).
#[derive(Debug, Clone, Copy, Default)]
struct Player {
    zone: usize,
    t: f64,
    released: bool,
    /// After a release with the release loop on: the frame the loop's end
    /// falls on, which plays as the frame the voice was in.
    marker: Option<usize>,
    from: usize,
}

#[derive(Debug, Clone, Copy, Default)]
struct EgRun {
    t: f64,
    released: bool,
    marker: Option<usize>,
    from: f32,
    gain: f32,
}

/// The oscillators of a voice (`CResynth`, `FUN_1801220c0`, the kernels
/// from `FUN_180115860`): each a magic circle `c -= k s; s += k c` whose
/// `k` and left and right gains ramp to their targets.
#[derive(Debug, Clone)]
struct Bank {
    phase: [f64; PARTIALS],
    c: [f32; PARTIALS],
    s: [f32; PARTIALS],
    k: [f32; PARTIALS],
    dk: [f32; PARTIALS],
    left: [f32; PARTIALS],
    right: [f32; PARTIALS],
    dleft: [f32; PARTIALS],
    dright: [f32; PARTIALS],
    on: [bool; PARTIALS],
    /// The level's last four block values and the curve through the middle
    /// two (`FUN_180122ae0`).
    history: [f32; 4],
    curve: [f32; 4],
    x: f32,
    dx: f32,
}

impl Default for Bank {
    fn default() -> Self {
        Bank {
            phase: [0.0; PARTIALS],
            c: [0.0; PARTIALS],
            s: [0.0; PARTIALS],
            k: [0.0; PARTIALS],
            dk: [0.0; PARTIALS],
            left: [0.0; PARTIALS],
            right: [0.0; PARTIALS],
            dleft: [0.0; PARTIALS],
            dright: [0.0; PARTIALS],
            on: [false; PARTIALS],
            history: [0.0; 4],
            curve: [0.0; 4],
            x: 0.0,
            dx: 0.0,
        }
    }
}

/// A value the engine flushes to 0.
fn tidy(v: f32) -> f32 {
    if v < 1e-9 && v > -1e-9 { 0.0 } else { v }
}

impl Bank {
    /// Push the level of the coming block, and draw the curve the samples
    /// of the block read their gain off.
    fn level(&mut self, level: f32, n: usize) {
        let [_, y0, y1, y2] = self.history;
        let y3 = level;
        let a1 = (y2 - y0) * 0.5;
        let a3 = (y1 - y2) * 1.5 + (y3 - y0) * 0.5;
        self.curve = [y1, a1, (y0 - y1) + a1 - a3, a3];
        self.history = [y0, y1, y2, y3];
        self.x = 0.0;
        self.dx = 1.0 / n as f32;
    }

    /// Set every oscillator ramping to the spectrum over `n` samples.
    #[allow(clippy::too_many_arguments)]
    fn aim(&mut self, spec: &Spectrum, n: usize, pitch: f64, pan: f32, rate: f64) {
        let top = rate * TOP;
        let per = 1.0 / rate;
        let steps = n as f32;
        for k in 0..PARTIALS {
            let p = ((pan + spec.pan[k] + 1.0) * 0.5).clamp(0.0, 1.0);
            let mut f = f64::from(spec.freq[k]) * pitch;
            if f <= 1.0 {
                f = 1.0;
            }
            let (mut right, mut left) = (p * spec.amp[k], (1.0 - p) * spec.amp[k]);
            if top < f {
                right = 0.0;
                left = 0.0;
            }
            let f = f.min(top);
            let coef = (PI * f * per).sin() as f32;
            let coef = coef + coef;
            let taper = ((1.0 - f / top) * 0.6 + 0.4) as f32;
            left *= taper;
            right *= taper;
            let old = self.phase[k];
            let next = per * f * n as f64 + old;
            self.phase[k] = if next >= 1.0 {
                next - f64::from(next as i32)
            } else {
                next
            };
            let (now_left, now_right) = if self.on[k] {
                (self.left[k], self.right[k])
            } else {
                (0.0, 0.0)
            };
            if left.abs() >= QUIET
                || right.abs() >= QUIET
                || now_left.abs() >= QUIET
                || now_right.abs() >= QUIET
            {
                if !self.on[k] {
                    let (s, c) = (old * 2.0 * PI).sin_cos();
                    self.c[k] = c as f32;
                    self.s[k] = s as f32;
                    self.k[k] = coef;
                    self.left[k] = 0.0;
                    self.right[k] = 0.0;
                    self.on[k] = true;
                }
                self.dleft[k] = tidy((left - now_left) / steps);
                self.dright[k] = tidy((right - now_right) / steps);
                self.dk[k] = tidy((coef - self.k[k]) / steps);
            } else {
                self.on[k] = false;
            }
        }
    }

    fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        let [a0, a1, a2, a3] = self.curve;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let (mut sum_l, mut sum_r) = (0.0f32, 0.0f32);
            for k in 0..PARTIALS {
                if !self.on[k] {
                    continue;
                }
                self.left[k] += self.dleft[k];
                self.right[k] += self.dright[k];
                let kk = self.k[k];
                self.c[k] -= kk * self.s[k];
                self.s[k] += kk * self.c[k];
                self.k[k] = kk + self.dk[k];
                sum_l += self.left[k] * self.c[k];
                sum_r += self.right[k] * self.c[k];
            }
            let x = self.x;
            let g = ((a3 * x + a2) * x + a1) * x + a0;
            self.x = x + self.dx;
            *l += sum_l * g;
            *r += sum_r * g;
        }
    }
}

/// One spectrum: amplitude, pan and frequency (in harmonics of the note)
/// of each partial.
#[derive(Debug, Clone)]
struct Spectrum {
    amp: [f32; PARTIALS],
    pan: [f32; PARTIALS],
    freq: [f32; PARTIALS],
}

impl Default for Spectrum {
    fn default() -> Self {
        Spectrum {
            amp: [0.0; PARTIALS],
            pan: [0.0; PARTIALS],
            freq: [0.0; PARTIALS],
        }
    }
}

#[derive(Debug, Clone)]
struct Voice {
    /// The key it plays, -1 when free.
    key: i32,
    age: u32,
    released: bool,
    /// The velocity through the curve.
    velocity: f32,
    /// The note the glide is at, and how far it moves a sample.
    glide: f32,
    glide_rate: f32,
    adsr: Adsr,
    done: u32,
    egs: [EgRun; 4],
    eg_out: [f32; 4],
    mods: [f32; 48],
    pitch: f32,
    pan: f32,
    players: [Player; 4],
    pwm: [[f64; PARTIALS]; 4],
    pwm_started: bool,
    morph_t: f64,
    /// Samples into the block, and the counter that spaces the spectra
    /// three blocks apart (`this+0xfc`, `this+0x100`).
    pos: usize,
    phase: i32,
    fresh: bool,
    bank: Bank,
    noise: NoiseRun,
}

impl Default for Voice {
    fn default() -> Self {
        Voice {
            key: -1,
            age: 0,
            released: false,
            velocity: 0.0,
            glide: 0.0,
            glide_rate: 0.0,
            adsr: Adsr {
                finished: true,
                ..Adsr::default()
            },
            done: 0,
            egs: [EgRun::default(); 4],
            eg_out: [0.0; 4],
            mods: [UNSET; 48],
            pitch: 1.0,
            pan: 0.0,
            players: [Player::default(); 4],
            pwm: [[0.0; PARTIALS]; 4],
            pwm_started: false,
            morph_t: 0.0,
            pos: 0,
            phase: -1,
            fresh: true,
            bank: Bank::default(),
            noise: NoiseRun::default(),
        }
    }
}

impl Voice {
    fn modded(&self, d: usize) -> f32 {
        let v = self.mods[d];
        if v < -999.9 { 0.0 } else { v }
    }

    /// A parameter with what the matrix sends its destination, kept in 0..1
    /// (`FUN_1800fbff0`).
    fn param(&self, base: f32, d: usize) -> f32 {
        (self.modded(d) + base).clamp(0.0, 1.0)
    }
}

/// Morphine.
pub struct Morphine {
    program: Program,
    params: [f32; PARAMS],
    rate: f32,
    tempo: f64,
    block: usize,
    voices: Vec<Voice>,
    clock: u32,
    /// The key last played, for the glide (`this+0x3990`).
    last_key: i32,
    /// The keys held, and which host voice holds each.
    held: [bool; 128],
    holder: [Option<u32>; 128],
    gens: [Spectrum; 4],
    spectrum: Spectrum,
    scratch_l: Vec<f32>,
    scratch_r: Vec<f32>,
    gain: f64,
    fx: Fx,
    /// FL's `Plugins/Fruity/Generators`, and the noise sample read from
    /// it.
    dir: Option<PathBuf>,
    noise: Option<NoiseSample>,
}

impl Default for Morphine {
    fn default() -> Self {
        let program = default_program();
        let params = program.params;
        let mut m = Morphine {
            program,
            params,
            rate: 44_100.0,
            tempo: 120.0,
            block: 127,
            voices: vec![Voice::default(); VOICES],
            clock: 0,
            last_key: -1,
            held: [false; 128],
            holder: [None; 128],
            gens: Default::default(),
            spectrum: Spectrum::default(),
            scratch_l: Vec::new(),
            scratch_r: Vec::new(),
            gain: 0.0,
            fx: Fx::default(),
            dir: None,
            noise: None,
        };
        m.set_sample_rate(44_100.0);
        m.gain = m.target_gain();
        m
    }
}

/// A program of silence, as the engine starts with: one silent frame in
/// each generator.
fn default_program() -> Program {
    use morphine_state::{EgPoint, Frame, Gen, MorphLine, MorphPoint, Route, Zone};
    let frames = Frames {
        frames: vec![Frame {
            duration: 100,
            gain: 1.0,
            amp: [0.0; PARTIALS],
            pan: [0.0; PARTIALS],
            freq: [0.0; PARTIALS],
        }],
        loop_start: 100,
        loop_end: 110,
        release_loop: false,
        loop_mode: 3,
        rate: 0.5,
        key_track: 0.5,
        vel_track: 0.5,
        sync: false,
        vel_sense: [1.0; PARTIALS],
        partial_gain: [1.0; PARTIALS],
    };
    let one = Gen {
        zones: vec![Zone {
            keys: 128,
            level: 1.0,
            frames,
        }],
    };
    let eg = Eg {
        points: vec![EgPoint {
            value: 1.0,
            duration: 100,
        }],
        loop_start: 100,
        loop_end: 110,
        bipolar: false,
    };
    let mut params = morphine_state::default_params();
    params[0] = 0.5;
    params[30] = 0.0;
    params[31] = 1.0;
    params[32] = 1.0;
    params[33] = 0.2;
    params[10] = 0.0;
    params[16] = 0.0;
    params[20] = 0.0;
    params[29] = 0.0;
    params[6] = 0.0;
    params[36] = 0.0;
    params[0x7c] = 15.0;
    Program {
        gens: vec![one.clone(), one.clone(), one.clone(), one],
        morph: MorphLine {
            points: vec![MorphPoint {
                x: 0.0,
                y: 1.0,
                duration: 100,
            }],
            loop_start: 100,
            loop_end: 110,
            solo: 0,
        },
        egs: vec![eg.clone(), eg.clone(), eg.clone(), eg],
        routes: [Route {
            source: 0,
            dest: 0,
            amount: 0.5,
        }; 12],
        params,
        noise: String::new(),
        name: String::new(),
    }
}

impl Morphine {
    /// The program the state loaded, for tests and tools.
    pub fn program(&self) -> &Program {
        &self.program
    }

    fn polyphony(&self) -> usize {
        let p = self.params[0x7c] as i32 + 1;
        if p < 1 || p > VOICES as i32 {
            VOICES
        } else {
            p as usize
        }
    }

    fn mono(&self) -> bool {
        self.params[0x7c] as i32 == 0
    }

    /// The output gain, `10^(2.4 v - 1.2) / 2` at the centre
    /// (`FUN_180111ec0` case 0, `FUN_1800e9160`).
    fn target_gain(&self) -> f64 {
        let level = 10f32.powf(self.params[0] * 2.4 - 1.2);
        f64::from(level * 0.5)
    }

    /// Semitones from the transpose and fine tune (`FUN_180111ec0` cases
    /// 4 and 5): `int(48 (t - 0.5))` and `int(200 (f - 0.5))` cents.
    fn transpose(&self) -> i32 {
        ((self.params[4] - 0.5) * 48.0) as i32
    }

    fn tuning(&self) -> f32 {
        let cents = ((self.params[5] - 0.5) * 200.0) as i32;
        (f64::from(cents) * 0.01 + f64::from(self.transpose())) as f32
    }

    fn units(&self, sync: bool) -> f64 {
        if sync {
            self.tempo * SYNCED / f64::from(self.rate)
        } else {
            UNITS / f64::from(self.rate)
        }
    }

    /// The modulation of a voice this block (`FUN_1800fb820`,
    /// `FUN_1800fb9d0`, `FUN_1800fbbc0`).
    fn modulate(&self, v: &mut Voice, n: usize) {
        let p = &self.params;
        for e in 0..4 {
            let base = 0x63 + 6 * e;
            let eg = &self.program.egs[e];
            let speed = rate(
                v.param(p[base + 2], 41 + 2 * e),
                p[base + 4],
                p[base + 3],
                v.key as f32,
                v.velocity,
            );
            let run = &mut v.egs[e];
            run.t += self.units(p[base + 1] >= 0.5) * speed * n as f64;
            let release_loop = p[base] >= 0.5;
            let mode = p[0x8c - e] as i32;
            let t = if run.released && release_loop {
                run.t
            } else {
                looped(run.t, eg.loop_start, eg.loop_end, mode)
            };
            let mut value = match run.marker {
                Some(marker) => marker_value(eg, t, marker, run.from),
                None => eg_value(eg, t),
            };
            if eg.bipolar {
                value = value * 2.0 - 1.0;
            }
            let amount = v.mods[40 + 2 * e];
            let amount = if amount < -999.9 { 1.0 } else { amount };
            v.eg_out[e] = value * run.gain * amount;
        }
        v.mods = [UNSET; 48];
        for route in &self.program.routes {
            let (s, d) = (route.source, route.dest);
            if s == 0 || d <= 0 || d >= 48 {
                continue;
            }
            let value = match s {
                1 => v.key as f32 * (1.0 / 120.0),
                2 => v.velocity,
                5..=8 => v.eg_out[(s - 5) as usize],
                // CC 7 (volume) is 1 and CC 10 (pan) 0.5 until a host
                // sends them; the wheel, aftertouch and the rest are 0.
                16 => 1.0,
                19 => 0.5,
                _ => 0.0,
            };
            let d = d as usize;
            let now = if v.mods[d] < -999.9 { 0.0 } else { v.mods[d] };
            let sent = (value * (route.amount + route.amount - 1.0)).clamp(-1.0, 1.0);
            v.mods[d] = sent + now;
        }
        v.pan = v.modded(2);
        let m = v.modded(3);
        v.pitch = 2f32.powf(m * m.abs());
    }

    /// The spectrum one generator gives this voice now, and its time moved
    /// on by `n` samples (`FUN_18012c850`).
    fn generate(&self, v: &mut Voice, g: usize, n: usize, out: &mut Spectrum) {
        let p = &self.params;
        let b = 0x2b + 10 * g;
        let generator = &self.program.gens[g];
        let Some(zone) = generator.zones.get(v.players[g].zone) else {
            *out = Spectrum::default();
            return;
        };
        let set = &zone.frames;
        let key = (v.key + self.transpose()) as f32;
        let speed = rate(
            v.param(p[b + 2], 0x0d + 4 * g),
            p[b + 3],
            p[b + 4],
            key,
            v.velocity,
        );
        let detune = v.param(p[b + 8], 0x0f + 4 * g);
        let pan = v.param(p[b + 9], 0x10 + 4 * g);
        let pwm_rate = v.param(p[b + 6], 0x0e + 4 * g);
        let mode = p[0x81 + 2 * g] as i32;
        let release_loop = p[b] >= 0.5;
        let player = &mut v.players[g];
        let t = if player.released && release_loop {
            player.t
        } else {
            looped(player.t, set.loop_start, set.loop_end, mode)
        };
        let frames = &set.frames;
        let (mut a, b2, u) = segment(frames.iter().map(|f| f.duration), frames.len(), t);
        if player.marker == Some(a) {
            a = player.from;
        }
        let zg = zone.level.cbrt();
        let (wa, wb, ga, gb) = if a == b2 {
            (zg, 0.0, zg * frames[a].gain, 0.0)
        } else {
            let (wa, wb) = ((1.0 - u) as f32, u as f32);
            (
                zg * wa,
                zg * wb,
                zg * (f64::from(frames[a].gain) * (1.0 - u)) as f32,
                zg * (f64::from(frames[b2].gain) * u) as f32,
            )
        };
        player.t += self.units(p[b + 1] >= 0.5) * speed * n as f64;
        let (detune, pan) = (detune * detune, pan * pan);
        let velocity = v.velocity - 1.0;
        let fa = &frames[a];
        let fb = &frames[b2];
        for k in 0..PARTIALS {
            let level = ga * fa.amp[k] + gb * fb.amp[k];
            out.amp[k] =
                (velocity * set.vel_sense[k] + 1.0) * level * level * level * set.partial_gain[k];
            out.pan[k] = pan * fa.pan[k] * wa + pan * fb.pan[k] * wb;
            out.freq[k] = (wa * fa.freq[k] + wb * fb.freq[k]) * detune + (k + 1) as f32;
        }
        // The partials' own LFO (`FUN_1800f8460`): mode 1 multiplies by
        // its cosine, 2 by one less it.
        let pwm = (p[0x82 + 2 * g] * 2.0 + 0.5) as i32;
        if pwm == 1 || pwm == 2 {
            let step = f64::from(pwm_rate * 0.25) / f64::from(self.rate) * n as f64;
            for k in 0..PARTIALS {
                let mut ph = f64::from(out.freq[k]) * step + v.pwm[g][k];
                if ph > 1.0 {
                    ph -= f64::from(ph as i32);
                }
                v.pwm[g][k] = ph;
                let w = (ph * 2.0 * PI).cos();
                let w = if pwm == 1 { w } else { 1.0 - w };
                out.amp[k] *= w as f32;
            }
        }
    }

    /// Build the voice's spectrum and aim its oscillators at it over `n`
    /// samples (`FUN_1801001c0`, `FUN_1801054b0`).
    fn respell(&mut self, i: usize, n: usize) {
        let mut v = std::mem::take(&mut self.voices[i]);
        // The glide (`FUN_1800e9d90`).
        let key = v.key as f32;
        let step = n as f32 * v.glide_rate;
        if v.glide < key {
            v.glide = (v.glide + step).min(key);
        } else if v.glide > key {
            v.glide = (v.glide - step).max(key);
        }
        let p = self.params;
        let line = &self.program.morph;
        let t = looped(v.morph_t, line.loop_start, line.loop_end, p[0x80] as i32);
        let (a, b, u) = segment(line.points.iter().map(|q| q.duration), line.points.len(), t);
        let (x, y) = if a == b {
            (line.points[a].x, line.points[a].y)
        } else {
            let mix = |p: f32, q: f32| (f64::from(p) * (1.0 - u) + f64::from(q) * u) as f32;
            (
                mix(line.points[a].x, line.points[b].x),
                mix(line.points[a].y, line.points[b].y),
            )
        };
        let x = v.param(x, 0x1e);
        let y = v.param(y, 0x1f);
        let mut w = [(1.0 - x) * y, x * y, x * (1.0 - y), (1.0 - x) * (1.0 - y)];
        let mut mix = [0.0f32; 4];
        for g in 0..4 {
            let m = v.param(p[0x57 + 3 * g], 0x20 + 2 * g);
            mix[g] = if p[0x59 + 3 * g] >= 0.5 { 0.0 } else { m * m };
        }
        if (1..=4).contains(&line.solo) {
            let s = (line.solo - 1) as usize;
            for g in 0..4 {
                w[g] = if g == s { 1.0 } else { 0.0 };
                mix[g] = if g == s { 1.0 } else { 0.0 };
            }
        }
        let mut gens = std::mem::take(&mut self.gens);
        for (g, out) in gens.iter_mut().enumerate() {
            self.generate(&mut v, g, n, out);
        }
        let spec = &mut self.spectrum;
        let pans: [f32; 4] = std::array::from_fn(|g| {
            let q = v.param(p[0x58 + 3 * g], 0x21 + 2 * g);
            q + q - 1.0
        });
        for k in 0..PARTIALS {
            spec.amp[k] = (0..4).map(|g| mix[g] * w[g] * gens[g].amp[k]).sum();
            spec.pan[k] = (0..4).map(|g| (pans[g] + gens[g].pan[k]) * w[g]).sum();
            spec.freq[k] = (0..4).map(|g| w[g] * gens[g].freq[k]).sum();
        }
        self.gens = gens;
        let speed = rate(
            v.param(p[0x54], 0x1d),
            p[0x56],
            p[0x55],
            (v.key + self.transpose()) as f32,
            v.velocity,
        );
        v.morph_t += speed * self.units(p[0x53] >= 0.5) * n as f64;
        // The note's pitch (`FUN_1800e9e90`, `FUN_1800e9e50`).
        let note = self.tuning() + v.glide;
        let hz = exp2(f64::from((note - 57.0) * (1.0 / 12.0))) * 220.0;
        let hz = f64::from(hz as f32) * f64::from(v.pitch);
        v.bank
            .aim(&self.spectrum, n, hz, v.pan, f64::from(self.rate));
        self.voices[i] = v;
    }

    /// Start voice `i` on `key` (`FUN_1800fffa0`).
    fn start(&mut self, i: usize, key: i32, velocity: f32, from: f32, legato: bool) {
        let rate = f64::from(self.rate);
        let glide = self.params[1].powi(3);
        let glide = if f64::from(glide) < 0.0001 {
            0.0001
        } else {
            glide
        };
        let mode = self.params[0x7d] as i32;
        self.clock += 1;
        let p = self.params;
        let transpose = self.transpose();
        let v = &mut self.voices[i];
        let was_free = v.key < 0;
        v.key = key;
        v.age = self.clock;
        let start = if f64::from(glide) < 0.001 {
            key as f32
        } else {
            from
        };
        v.glide_rate = if mode >= 2 {
            ((key as f32 - start).abs() as f64 / (rate * f64::from(glide))) as f32
        } else {
            (12.0 / (rate * f64::from(glide))) as f32
        };
        if legato {
            v.released = false;
            return;
        }
        v.glide = start;
        v.released = false;
        v.velocity = velocity.powf(p[2] * 5.0);
        // The envelopes start over, their gain set by the velocity
        // (`FUN_1800fbed0`, `FUN_1800f7880`).
        for e in 0..4 {
            let base = 0x63 + 6 * e;
            v.egs[e] = EgRun {
                gain: 1.0 - (1.0 - v.velocity) * p[base + 5],
                ..EgRun::default()
            };
            v.eg_out[e] = 0.0;
        }
        v.mods = [UNSET; 48];
        // The sums with the envelopes at 0.
        let routes = self.program.routes;
        let v = &mut self.voices[i];
        for route in &routes {
            let (s, d) = (route.source, route.dest);
            if s == 0 || d <= 0 || d >= 48 {
                continue;
            }
            let value = match s {
                1 => v.key as f32 * (1.0 / 120.0),
                2 => v.velocity,
                16 => 1.0,
                19 => 0.5,
                _ => 0.0,
            };
            let d = d as usize;
            let now = if v.mods[d] < -999.9 { 0.0 } else { v.mods[d] };
            v.mods[d] = (value * (route.amount + route.amount - 1.0)).clamp(-1.0, 1.0) + now;
        }
        let a = v.param(p[30], 4);
        let d = v.param(p[31], 5);
        let s = v.param(p[32], 7);
        let attack = (f64::from(a.sqrt() * a * a * a) * 15.0).max(0.003);
        let decay = f64::from(d * d * d * 30.0).max(0.003);
        v.adsr.start(rate, attack, decay, f64::from(s * s));
        v.done = 0;
        // The generators pick their zone by the transposed key.
        let zkey = key + transpose;
        for g in 0..4 {
            let zones = &self.program.gens[g].zones;
            let mut left = zkey;
            let mut zone = zones.len().saturating_sub(1);
            for (z, zn) in zones.iter().enumerate() {
                left -= zn.keys;
                if left < 0 {
                    zone = z;
                    break;
                }
            }
            v.players[g] = Player {
                zone,
                ..Player::default()
            };
            let b = 0x2b + 10 * g;
            if was_free || !v.pwm_started {
                let phase = f64::from(p[b + 7].clamp(0.0, 1.0));
                let free = p[b + 5] >= 0.5;
                for k in 0..PARTIALS {
                    v.pwm[g][k] = if free && v.pwm_started {
                        v.pwm[g][k]
                    } else {
                        let x = (k + 1) as f64 * phase;
                        if x >= 1.0 { x - f64::from(x as i32) } else { x }
                    };
                }
            }
        }
        v.pwm_started = true;
        v.morph_t = 0.0;
        v.noise.start();
        v.pos = 0;
        v.phase = -1;
        v.fresh = true;
        if was_free {
            v.bank = Bank::default();
        }
    }

    /// Let voice `i` go (`FUN_180100620`).
    fn release(&mut self, i: usize) {
        let p = self.params;
        let rate = f64::from(self.rate);
        let v = &mut self.voices[i];
        if v.released {
            return;
        }
        v.released = true;
        for e in 0..4 {
            let eg = &self.program.egs[e];
            let base = 0x63 + 6 * e;
            let mode = p[0x8c - e] as i32;
            let run = &mut v.egs[e];
            if p[base] >= 0.5 && mode != 0 {
                let t = looped(run.t, eg.loop_start, eg.loop_end, mode);
                let (marker, at) =
                    loop_end_point(eg.points.iter().map(|q| q.duration), eg.loop_end);
                if let Some(marker) = marker {
                    run.from = eg_value(eg, t);
                    run.marker = Some(marker);
                    run.t = f64::from(at);
                }
            }
            run.released = true;
        }
        for g in 0..4 {
            let b = 0x2b + 10 * g;
            let mode = p[0x81 + 2 * g] as i32;
            let player = &mut v.players[g];
            if let Some(zone) = self.program.gens[g].zones.get(player.zone) {
                let set = &zone.frames;
                if p[b] >= 0.5 && mode != 0 {
                    let t = looped(player.t, set.loop_start, set.loop_end, mode);
                    let (marker, at) =
                        loop_end_point(set.frames.iter().map(|f| f.duration), set.loop_end);
                    if let Some(marker) = marker {
                        player.from =
                            segment(set.frames.iter().map(|f| f.duration), set.frames.len(), t).0;
                        player.marker = Some(marker);
                        player.t = f64::from(at);
                    }
                }
            }
            player.released = true;
        }
        let r = v.param(p[33], 6);
        let release = f64::from(r * r * r * 10.0).max(0.003);
        v.adsr.let_go(rate, release);
    }

    /// Read the noise sample the program names from `Noise Samples`.
    fn load_noise(&mut self) {
        self.noise = None;
        let (Some(dir), name) = (&self.dir, &self.program.noise) else {
            return;
        };
        if name.is_empty() {
            return;
        }
        let dir = dir.join("Morphine").join("Noise Samples");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return;
        };
        let found = entries.flatten().find(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|file| file.eq_ignore_ascii_case(name))
        });
        if let Some(bytes) = found.and_then(|entry| std::fs::read(entry.path()).ok()) {
            self.noise = NoiseSample::read(&bytes);
        }
    }

    /// One block's control of voice `i`: modulation, level and noise.
    /// False when the voice has finished.
    fn control(&mut self, i: usize, n: usize) -> bool {
        let mut v = std::mem::take(&mut self.voices[i]);
        self.modulate(&mut v, n);
        if v.adsr.finished {
            v.done += 1;
        }
        let alive = v.done <= 1;
        if alive {
            let level = v.adsr.step(n) * 10f32.powf(v.modded(1));
            v.bank.level(level, n);
            if let Some(sample) = &self.noise {
                let p = &self.params;
                let b = NoiseBlock {
                    mix: v.param(p[34], 8),
                    pan: v.param(p[35], 9),
                    ratio: v.param(p[39], 10),
                    cutoff: v.param(p[40], 11),
                    res: v.param(p[41], 12),
                    key_track: p[42],
                    mute: p[36] != 0.0,
                    vel_sense: p[38],
                    loop_mode: p[0x7e] as i32,
                    filter_mode: p[0x7f] as i32,
                    mono: p[37] != 0.0,
                    voice_pan: v.pan,
                    velocity: v.velocity,
                    level,
                    note: f64::from(self.tuning() + v.glide),
                };
                v.noise.block(sample, &b, n, f64::from(self.rate));
            }
        }
        self.voices[i] = v;
        alive
    }

    fn render_voice(&mut self, i: usize, left: &mut [f32], right: &mut [f32]) {
        let block = self.block;
        if self.voices[i].phase < 0 {
            let mut v = std::mem::take(&mut self.voices[i]);
            self.modulate(&mut v, 1);
            self.voices[i] = v;
            self.respell(i, 1);
            let (mut l, mut r) = ([0.0f32], [0.0f32]);
            self.voices[i].bank.render(&mut l, &mut r);
            self.voices[i].phase = 0;
        }
        let mut done = 0;
        while done < left.len() {
            if self.voices[i].pos == 0 {
                if !self.control(i, block) {
                    self.voices[i].key = -1;
                    self.voices[i].bank = Bank::default();
                    return;
                }
                let v = &mut self.voices[i];
                let st = v.phase;
                let call = if st == 3 {
                    v.phase = 1;
                    true
                } else {
                    v.phase = st + 1;
                    st == 0
                };
                if call {
                    let n = if v.fresh { block } else { 3 * block };
                    self.respell(i, n);
                }
                let v = &mut self.voices[i];
                if v.fresh {
                    v.phase = 0;
                }
                v.fresh = false;
            }
            let v = &mut self.voices[i];
            let chunk = (left.len() - done).min(block - v.pos);
            v.bank.render(
                &mut left[done..done + chunk],
                &mut right[done..done + chunk],
            );
            if let Some(sample) = &self.noise {
                v.noise.render(
                    sample,
                    &mut left[done..done + chunk],
                    &mut right[done..done + chunk],
                );
            }
            v.pos = (v.pos + chunk) % block;
            done += chunk;
        }
    }

    fn key_on(&mut self, key: i32, velocity: f32) {
        self.held[key as usize] = true;
        let from = if self.last_key >= 0 {
            self.last_key as f32
        } else {
            key as f32
        };
        let mode = self.params[0x7d] as i32;
        if self.mono() {
            let v = &self.voices[0];
            let retrigger = v.key < 0 || v.released;
            let from = if (mode == 1 || mode == 3) && retrigger {
                key as f32
            } else {
                from
            };
            let from = if retrigger {
                from
            } else {
                self.voices[0].glide
            };
            self.start(0, key, velocity, from, !retrigger);
        } else {
            let count = self.polyphony();
            let from = if mode == 1 || mode == 3 {
                nearest_held(&self.held, key).map_or(key as f32, |k| k as f32)
            } else {
                from
            };
            let mut pick = None;
            let (mut oldest, mut oldest_released) = (0, None::<usize>);
            for (i, v) in self.voices[..count].iter().enumerate() {
                if v.key < 0 {
                    pick = Some(i);
                    break;
                }
                if v.released {
                    if oldest_released.is_none_or(|o| v.age < self.voices[o].age) {
                        oldest_released = Some(i);
                    }
                } else if v.age < self.voices[oldest].age {
                    oldest = i;
                }
            }
            let i = pick.or(oldest_released).unwrap_or(oldest);
            self.start(i, key, velocity, from, false);
        }
        self.last_key = key;
    }

    fn key_off(&mut self, key: i32) {
        self.held[key as usize] = false;
        if self.mono() {
            if self.voices[0].key == key {
                if let Some(back) = nearest_held(&self.held, key) {
                    let from = self.voices[0].glide;
                    let velocity = self.voices[0].velocity;
                    self.start(0, back, velocity, from, true);
                    self.last_key = back;
                } else {
                    self.release(0);
                }
            }
        } else {
            for i in 0..self.voices.len() {
                if self.voices[i].key == key && !self.voices[i].released {
                    self.release(i);
                }
            }
        }
    }
}

/// The frame or point the loop's end falls at, counting their durations
/// from the start, and where it starts (`FUN_1800ebc50`).
fn loop_end_point(durations: impl Iterator<Item = i32>, end: i32) -> (Option<usize>, i32) {
    let mut at = 0;
    for (i, d) in durations.enumerate() {
        if at >= end {
            return (Some(i), at);
        }
        at += d;
    }
    (None, at)
}

/// An envelope's value after its release jumped to `marker`: the segment
/// from it starts at the value the envelope was at.
fn marker_value(eg: &Eg, t: f64, marker: usize, from: f32) -> f32 {
    let (a, b, u) = segment(eg.points.iter().map(|p| p.duration), eg.points.len(), t);
    let va = if a == marker {
        from
    } else {
        eg.points[a].value
    };
    if a == b {
        return va;
    }
    (f64::from(va) * (1.0 - u) + f64::from(eg.points[b].value) * u) as f32
}

/// The held key nearest `key`, other than it (`FUN_1800e8b70`).
fn nearest_held(held: &[bool; 128], key: i32) -> Option<i32> {
    let mut best: Option<(i32, i32)> = None;
    for (k, on) in held.iter().enumerate() {
        let k = k as i32;
        if *on && k != key {
            let d = (key - k).abs();
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, k));
            }
        }
    }
    best.map(|(_, k)| k)
}

impl Generator for Morphine {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let program = morphine_state::read_state(bytes)?;
        self.params = program.params;
        self.program = program;
        self.load_noise();
        self.reset();
        self.fx.set_params(&self.params, f64::from(self.rate));
        self.gain = self.target_gain();
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        self.rate = hz;
        self.block = ((f64::from(hz) * BLOCK) as usize).max(1);
        self.scratch_l = vec![0.0; 4096];
        self.scratch_r = vec![0.0; 4096];
        self.fx.set_rate(f64::from(hz));
        self.fx.set_params(&self.params, f64::from(hz));
        self.reset();
    }

    fn set_tempo(&mut self, bpm: f64) {
        self.tempo = bpm;
    }

    fn set_resource_dir(&mut self, dir: &Path) {
        self.dir = Some(dir.to_path_buf());
        self.load_noise();
    }

    fn note_on(&mut self, note: NoteOn) {
        let pitch = i32::from(note.key) * 100 + note.fine_pitch as i32;
        let key = (pitch as f32 * 0.01) as i32;
        if !(0..128).contains(&key) {
            return;
        }
        let v = ((f64::from(note.velocity) * 10.0 + 1.0).ln() as f32 * 41.714_26).clamp(0.0, 127.0);
        let velocity = (v as i32) as f32 / 127.0;
        if self.holder[key as usize].is_some() {
            self.key_off(key);
        }
        self.holder[key as usize] = Some(note.voice);
        if velocity > 0.0 {
            self.key_on(key, velocity);
        }
    }

    fn note_off(&mut self, voice: u32) {
        if let Some(key) = self.holder.iter().position(|h| *h == Some(voice)) {
            self.holder[key] = None;
            self.key_off(key as i32);
        }
    }

    fn all_notes_off(&mut self) {
        for key in 0..128 {
            if self.holder[key].take().is_some() {
                self.key_off(key as i32);
            }
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        let mut done = 0;
        while done < len {
            let n = (len - done).min(self.scratch_l.len());
            let mut l = std::mem::take(&mut self.scratch_l);
            let mut r = std::mem::take(&mut self.scratch_r);
            l[..n].fill(0.0);
            r[..n].fill(0.0);
            for i in 0..self.voices.len() {
                if self.voices[i].key >= 0 {
                    self.render_voice(i, &mut l[..n], &mut r[..n]);
                }
            }
            self.fx.process(&mut l[..n], &mut r[..n]);
            let target = self.target_gain();
            for k in 0..n {
                left[done + k] += l[k] * self.gain as f32;
                right[done + k] += r[k] * self.gain as f32;
                self.gain += (target - self.gain) * 0.005;
            }
            self.scratch_l = l;
            self.scratch_r = r;
            done += n;
        }
    }

    fn param_count(&self) -> usize {
        0x8d
    }

    fn param(&self, index: usize) -> f32 {
        if index < 0x8d {
            self.params[index]
        } else {
            0.0
        }
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if index < 0x8d && value.is_finite() {
            self.params[index] = value;
            self.fx.set_params(&self.params, f64::from(self.rate));
        }
    }

    fn reset(&mut self) {
        for v in &mut self.voices {
            *v = Voice::default();
        }
        self.held = [false; 128];
        self.holder = [None; 128];
        self.last_key = -1;
        self.fx.reset();
    }

    fn tail(&self) -> usize {
        let r = self.params[33];
        let release = f64::from(r * r * r * 10.0).max(0.003);
        ((release + 0.05 + self.fx.tail()) * f64::from(self.rate)) as usize
    }

    fn fidelity(&self) -> Fidelity {
        Fidelity::Approximate("Morphine noise layer, effects and release-loop jump approximated")
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

    #[test]
    fn a_forward_loop_wraps_back_by_its_length() {
        assert_eq!(looped(15.0, 10, 20, 1), 15.0);
        assert_eq!(looped(25.0, 10, 20, 1), 15.0);
        assert_eq!(looped(25.0, 10, 20, 0), 25.0);
        // At the end of a loop of no length the time stays there.
        assert_eq!(looped(30.0, 20, 20, 1), 20.0);
    }

    #[test]
    fn a_ping_pong_loop_turns_at_both_ends() {
        assert_eq!(looped(22.0, 10, 20, 3), 18.0);
        assert_eq!(looped(32.0, 10, 20, 3), 12.0);
        assert_eq!(looped(42.0, 10, 20, 3), 18.0);
    }

    #[test]
    fn a_backward_loop_runs_from_its_end_down() {
        assert_eq!(looped(13.0, 0, 10, 2), 7.0);
        assert_eq!(looped(19.0, 0, 10, 2), 1.0);
    }

    #[test]
    fn a_segment_is_found_by_the_durations_before_it() {
        let d = [10, 20, 30];
        assert_eq!(segment(d.iter().copied(), 3, 5.0), (0, 1, 0.5));
        assert_eq!(segment(d.iter().copied(), 3, 20.0), (1, 2, 0.5));
        assert_eq!(segment(d.iter().copied(), 3, 100.0), (2, 2, 0.0));
    }

    #[test]
    fn rate_one_half_with_no_tracking_is_one() {
        assert!((rate(0.5, 0.5, 0.5, 60.0, 0.3) - 1.0).abs() < 1e-12);
        assert!((rate(0.625, 0.5, 0.5, 60.0, 0.3) - 2.0).abs() < 1e-12);
        // Full key tracking: an octave up doubles the rate.
        assert!((rate(0.5, 0.5, 1.0, 60.0, 0.3) - 2.0).abs() < 1e-12);
    }

    #[test]
    fn the_decay_bends_to_its_end() {
        assert_eq!(bend(0.0, 1.0, 0.5), 1.0);
        assert!((bend(1.0, 1.0, 0.5) - 0.5).abs() < 1e-12);
        // Faster than a line at first.
        assert!(bend(0.25, 1.0, 0.0) < 0.75);
    }

    #[test]
    fn the_level_curve_passes_through_its_middle_points() {
        let mut bank = Bank::default();
        for y in [0.2, 0.4, 0.6, 0.8] {
            bank.level(y, 4);
        }
        let [a0, a1, a2, a3] = bank.curve;
        assert!((a0 - 0.4).abs() < 1e-6);
        assert!((a0 + a1 + a2 + a3 - 0.6).abs() < 1e-6);
    }

    #[test]
    fn an_oscillator_sounds_at_its_frequency() {
        let rate = 48_000.0;
        let mut bank = Bank::default();
        let mut spec = Spectrum::default();
        spec.amp[0] = 1.0;
        spec.freq[0] = 1.0;
        bank.level(1.0, 1);
        bank.level(1.0, 1);
        bank.level(1.0, 1);
        bank.level(1.0, 4800);
        bank.aim(&spec, 1, 1000.0, 0.0, rate);
        let (mut l, mut r) = ([0.0f32; 1], [0.0f32; 1]);
        bank.render(&mut l, &mut r);
        bank.aim(&spec, 4800, 1000.0, 0.0, rate);
        let (mut l, mut r) = (vec![0.0f32; 4800], vec![0.0f32; 4800]);
        bank.render(&mut l, &mut r);
        let peak = testing::partial(&l, 1000.0 / rate);
        let taper = (1.0 - 1000.0 / (rate * TOP)) * 0.6 + 0.4;
        assert!((peak - 0.5 * taper).abs() < 0.01, "{peak}");
        assert_eq!(l, r);
    }

    #[test]
    fn the_default_program_is_silent_and_a_sine_program_sounds() {
        let mut synth = Morphine::default();
        synth.set_sample_rate(44_100.0);
        synth.note_on(note(1, 60));
        let (mut l, mut r) = (vec![0.0; 4096], vec![0.0; 4096]);
        synth.process(&mut l, &mut r);
        assert!(l.iter().all(|v| *v == 0.0));

        let mut synth = Morphine::default();
        synth.program.gens[0].zones[0].frames.frames[0].amp[0] = 1.0;
        synth.set_sample_rate(44_100.0);
        synth.note_on(note(1, 69));
        let (mut l, mut r) = (vec![0.0; 44_100], vec![0.0; 44_100]);
        synth.process(&mut l, &mut r);
        let tail = &l[22_050..];
        let peak = testing::partial(&tail[..22_000], 440.0 / 44_100.0);
        assert!(peak > 0.01, "{peak}");
        synth.note_off(1);
        let (mut l, mut r) = (vec![0.0; 44_100], vec![0.0; 44_100]);
        synth.process(&mut l, &mut r);
        assert!(l[40_000..].iter().all(|v| v.abs() < 1e-4));
    }

    #[test]
    fn every_state_fl_ships_loads_and_plays() {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, Morphine's states skipped");
            return;
        };
        let dir = root.join("../Plugins/Fruity/Generators");
        let mut saved = testing::every_preset(&root, NAME);
        let fsts = saved.len();
        saved.extend(
            testing::projects(&root)
                .into_iter()
                .filter(|saved| saved.plugin.eq_ignore_ascii_case(NAME)),
        );
        // Morphine's own presets are its chunk with no "SSLF".
        let presets = root.join("Patches/Plugin presets/Generators/Morphine");
        let mut mrps = Vec::new();
        let mut dirs = vec![presets];
        while let Some(d) = dirs.pop() {
            for entry in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("mrp"))
                {
                    mrps.push(path);
                }
            }
        }
        mrps.sort();
        let mut states: Vec<(String, Vec<u8>)> = saved
            .iter()
            .map(|s| (s.file.clone(), s.state.clone()))
            .collect();
        for path in &mrps {
            let chunk = std::fs::read(path).unwrap();
            let mut state = b"SSLF".to_vec();
            state.extend((chunk.len() as u32).to_le_bytes());
            state.extend(chunk);
            states.push((path.display().to_string(), state));
        }
        for (i, (file, state)) in states.iter().enumerate() {
            let mut synth = Morphine::default();
            synth.set_resource_dir(&dir);
            synth.set_sample_rate(44_100.0);
            if let Err(error) = synth.set_state(state) {
                panic!("{file}: {error}");
            }
            // Every project state, and every tenth preset, plays.
            if i >= saved.len() && !(i - saved.len()).is_multiple_of(10) {
                continue;
            }
            synth.note_on(note(1, 60));
            let (mut left, mut right) = (vec![0.0; 22_050], vec![0.0; 22_050]);
            for (l, r) in left.chunks_mut(512).zip(right.chunks_mut(512)) {
                synth.process(l, r);
            }
            synth.note_off(1);
            let (mut l2, mut r2) = (vec![0.0; 22_050], vec![0.0; 22_050]);
            for (l, r) in l2.chunks_mut(512).zip(r2.chunks_mut(512)) {
                synth.process(l, r);
            }
            let peak = left
                .iter()
                .chain(&right)
                .fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(
                left.iter()
                    .chain(&right)
                    .chain(&l2)
                    .chain(&r2)
                    .all(|v| v.is_finite() && v.abs() < 8.0),
                "{file}"
            );
            assert!(peak > 1e-4, "{file}: {peak}");
        }
        eprintln!(
            "{fsts} fst, {} in projects, {} mrp",
            saved.len() - fsts,
            mrps.len()
        );
        assert!(saved.len() - fsts >= 7);
        assert!(mrps.len() >= 300);
    }

    /// Print the level of a state's note over time: `MORPHINE_PROBE=<file>`.
    #[test]
    #[ignore]
    fn probe() {
        let Some(path) = std::env::var_os("MORPHINE_PROBE") else {
            return;
        };
        let state = std::fs::read(path).unwrap();
        let mut synth = Morphine::default();
        synth.set_sample_rate(44_100.0);
        synth.set_state(&state).unwrap();
        let key = std::env::var("MORPHINE_KEY").map_or(60, |k| k.parse().unwrap());
        synth.note_on(note(1, key));
        for block in 0..40 {
            if block == 30 {
                synth.note_off(1);
            }
            let (mut l, mut r) = (vec![0.0; 2205], vec![0.0; 2205]);
            synth.process(&mut l, &mut r);
            let peak = l.iter().chain(&r).fold(0.0f32, |m, v| m.max(v.abs()));
            let rms = (l.iter().map(|v| v * v).sum::<f32>() / l.len() as f32).sqrt();
            let v = &synth.voices[0];
            eprintln!(
                "{:5.2}s peak {peak:.4} rms {rms:.4} env {:.3} amp0 {:.4} morph {:.1} t0 {:.1}",
                block as f32 * 0.05,
                v.adsr.value,
                synth.spectrum.amp[0],
                v.morph_t,
                v.players[0].t
            );
        }
    }
}
