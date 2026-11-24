//! PoiZone: a two-oscillator virtual analog of up to 64 voices with
//! unison, a four-pole filter, a trance gate, chorus and delay, rebuilt
//! from PoiZone's `engine_x64.dll` (`docs/plugins/poizone.md`).
//! `PoiZone_x64.dll` beside it is the panel only.
//!
//! The engine is Image-Line's `CSynthesizer` frame (as Sawer's is), with
//! a `PoiZoneSynth` holding the knobs and a list of 1024
//! `PoiZoneGenerator` voices. Each voice runs at eight times the host's
//! rate: two phase oscillators (saw, or a pulse made of the saw less
//! itself shifted), sync, ring and noise into a four-pole filter with a
//! soft clip on its input, brought down by a biquad. The voice path is
//! ported as the DLL computes it, its one-voice loop (`0x18002d340`) and
//! its four-voice SSE loop (`0x180031760`) apart, and checked against the
//! DLL under unicorn (`.scratch/notes/poizone-emu/`). The delay and the
//! arpeggiator are not built yet: a state that turns one on plays without
//! it and says so in [`Generator::fidelity`].

use crate::generator::{Generator, NoteOn};
use crate::sakura_fx::{Chorus, Stamped, powf, sin_cos_f32, step_for};
use crate::{Fidelity, StateError};

pub const NAME: &str = "PoiZone";
/// `SSLF`, the chunk's length, `4reV`, 156 f32 and the program's name.
pub const STATE_LEN: usize = 700;
/// The engine's parameters, 0x9c of them (74 on the panel, the rest its
/// controller links).
pub const PARAMS: usize = 156;

/// The voices the engine makes (`0x18002dca0`).
const VOICES: usize = 1024;
/// The voices run at eight times the host's rate.
const OVER: usize = 8;
/// The frames one engine render takes: FL's wrapper cuts the host's block
/// to its buffer, 64 frames.
const BLOCK: usize = 64;
const TINY: f32 = 1e-9;

/// `x * 0x0bb38435 + 0x3619636b`, each of the engine's three random
/// generators (`0x180081e20` noise and phases, `0x180081e24` the random
/// detune, `0x180081e28` the LFO's sample and hold).
fn step(x: u32) -> u32 {
    x.wrapping_mul(0x0bb3_8435).wrapping_add(0x3619_636b)
}

/// A draw as a fraction of 2^32, in f32.
fn unit(x: u32) -> f32 {
    x as f32 * f32::from_bits(0x2f80_0000)
}

fn flush(x: f32) -> f32 {
    if x < TINY && x > -TINY { 0.0 } else { x }
}

/// `FUN_18002e310`: `v^4` between two times in ms.
fn quartic(v: f32, lo: f32, hi: f32) -> f32 {
    let x = (v * v) * (v * v) * (hi - lo) + lo;
    x.max(lo).min(hi)
}

/// A choice from a knob: `int(v * n + small)`, kept to `0..=top`.
fn choice(v: f32, n: f32, small: u32, top: i32) -> i32 {
    ((v * n + f32::from_bits(small)) as i32).clamp(0, top)
}

/// One of the engine's envelopes (`FUN_180019f20` start, `FUN_18001a010`
/// a step, `FUN_18001a1b0`/`FUN_18001a190` the release), in f64 and in
/// steps of a voice update: an attack, a decay to the sustain and a
/// release, each bent by `1 - 10^(t c) (1 - t)` when its curve `c` is not
/// 0.
#[derive(Debug, Clone, Copy)]
struct Env {
    rate: f64,
    peak: f64,
    start: f64,
    attack: f64,
    decay: f64,
    sustain: f64,
    release: f64,
    stage: u8,
    t: f64,
    done: bool,
    value: f64,
    from: f64,
    curve: [f64; 3],
}

impl Default for Env {
    fn default() -> Self {
        Env {
            rate: 0.0,
            peak: 0.0,
            start: 0.0,
            attack: 0.0,
            decay: 0.0,
            sustain: 0.0,
            release: 0.0,
            stage: 0,
            t: 0.0,
            done: true,
            value: 0.0,
            from: 0.0,
            curve: [0.0; 3],
        }
    }
}

impl Env {
    #[allow(clippy::too_many_arguments)]
    fn start(&mut self, a: f64, d: f64, s: f64, curve: [f64; 3], rate: f64, peak: f64) {
        self.rate = rate;
        self.peak = peak;
        self.start = if self.done { 0.0 } else { self.value };
        let a = if a <= 0.001 { 0.001 } else { a };
        let d = if d <= 0.001 { 0.001 } else { d };
        self.curve = curve;
        self.done = false;
        self.stage = 0;
        self.sustain = peak * s;
        self.t = 0.0;
        self.attack = 1.0 / (a * rate);
        self.decay = 1.0 / (d * rate);
        self.set_release(0.0);
    }

    fn set_release(&mut self, r: f64) {
        if self.stage != 2 {
            let r = if r <= 0.001 { 0.001 } else { r };
            self.release = 1.0 / (r * self.rate);
        }
    }

    fn let_go(&mut self, r: f64) {
        self.set_release(r);
        if self.stage != 2 {
            self.from = self.value;
            self.stage = 2;
            self.t = 0.0;
        }
    }

    fn bend(t: f64, from: f64, to: f64, c: f64) -> f64 {
        if c == 0.0 {
            (to - from) * t + from
        } else {
            (1.0 - (10.0f64).powf(t * c) * (1.0 - t)) * (to - from) + from
        }
    }

    fn run(&mut self, n: i32) -> f32 {
        let n = f64::from(n);
        let v = match self.stage {
            0 => {
                let t = n * self.attack + self.t;
                self.t = t;
                if 1.0 <= t {
                    self.stage = 1;
                    self.t = 0.0;
                    self.peak
                } else {
                    Self::bend(t, self.start, self.peak, self.curve[0])
                }
            }
            1 => {
                let t = n * self.decay + self.t;
                self.t = t;
                if 1.0 <= t {
                    if self.sustain == 0.0 {
                        self.stage = 2;
                        self.done = true;
                    }
                    self.t = 1.0;
                    self.sustain
                } else {
                    Self::bend(t, self.peak, self.sustain, self.curve[1])
                }
            }
            2 => {
                let t = n * self.release + self.t;
                self.t = t;
                if 1.0 <= t {
                    self.done = true;
                    self.t = 1.0;
                    0.0
                } else {
                    Self::bend(t, self.from, 0.0, self.curve[2])
                }
            }
            _ => 0.0,
        };
        self.value = v;
        if v < 0.0 {
            self.value = 0.0;
            return 0.0;
        }
        v as f32
    }
}

/// The engine's LFO (`0x1800268b0`, run by `0x180026950`): a phase in
/// turns and five shapes, free or at a fraction of the tempo.
#[derive(Debug, Clone, Copy)]
struct Lfo {
    speed: f64,
    phase: f64,
    held: f64,
    shape: i32,
    retrig: bool,
    /// The tempo's division, or -1 when free.
    sync: f64,
}

impl Default for Lfo {
    fn default() -> Self {
        Lfo {
            speed: 1.0,
            phase: 0.0,
            held: 0.0,
            shape: 2,
            retrig: false,
            sync: -1.0,
        }
    }
}

impl Lfo {
    fn run(&mut self, rate: f64, tempo: f64, n: i32, rng: &mut u32) -> f32 {
        let mut speed = self.speed;
        if 0.0 < self.sync {
            speed = (0.066_666_666_666_666_67 / self.sync) * tempo;
        }
        let old = self.phase;
        let ph = (speed / rate) * f64::from(n) + old;
        let ph = ph - f64::from(ph as i32);
        self.phase = ph;
        let v = match self.shape {
            0 => ph,
            1 => {
                if ph < 0.5 {
                    1.0
                } else {
                    0.0
                }
            }
            2 => {
                let d = ph + ph;
                if 1.0 < d { 2.0 - d } else { d }
            }
            3 => (ph * std::f64::consts::TAU).cos(),
            4 => {
                if ph < old {
                    *rng = step(*rng);
                    self.held = f64::from(*rng) * 2.328_306_437_080_797_4e-10;
                }
                self.held
            }
            _ => 0.0,
        };
        v as f32
    }
}

/// The high shelf FL leaves on at the end (`CFilter` at `+0x80` and
/// `+0xbc` of the effects, `0x18001b7e0` design, `0x18001bb80` run):
/// 5 dB from 12 kHz, bandwidth 0.99.
#[derive(Debug, Clone, Copy, Default)]
struct Shelf {
    b: [f32; 5],
    s: [f32; 4],
}

impl Shelf {
    fn set(&mut self, rate: f32) {
        let a = powf(10.0, 5.0 * f32::from_bits(0x3d4c_cccd));
        let w = ((f64::from(12_000.0f32) * f64::from_bits(0x4019_21fb_3fa6_defc)) / f64::from(rate))
            as f32;
        let w = w.clamp(0.0, 3.0);
        let sn = (f64::from(w).sin()) as f32;
        let d = ((f64::from(0.99f32) * 0.346_573_6 * f64::from(w)) / f64::from(sn)).sinh();
        let dd = (d + d) as f32;
        let root = a.sqrt() * dd;
        let cs = (f64::from(w).cos()) as f32;
        let k = sn * root;
        let (am, ap) = (a - 1.0, a + 1.0);
        let (amc, apc) = (am * cs, ap * cs);
        let up = amc + ap;
        let down = ap - amc;
        let a0 = down + k;
        let b0 = (k + up) * a;
        let b2 = (up - k) * a;
        let b1 = (apc + am) * a * -2.0;
        let a1 = (am - apc) + (am - apc);
        let a2 = down - k;
        let inv = 1.0 / a0;
        self.b = [b0, b1, b2, a1, a2].map(|c| flush(inv * c));
    }

    fn run(&mut self, x: f32) -> f32 {
        let x = flush(x);
        let [b0, b1, b2, a1, a2] = self.b;
        let [x1, x2, y1, y2] = self.s;
        let y = flush((((x1 * b1 + x * b0) + b2 * x2) - y1 * a1) - a2 * y2);
        self.s = [x, x1, y, y1];
        y
    }
}

/// One of the cuts on a delay line (`0x18001b5f0` lowpass and
/// `0x18001b400` highpass designs, `0x18001b6f0`/`0x18001b500` runs): a
/// biquad from `sincosf` of the frequency, with a `q` of 1.
#[derive(Debug, Clone, Copy, Default)]
struct Cut {
    high: bool,
    b0: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Cut {
    fn set(&mut self, hz: f32, rate: f64) {
        let hz = if hz <= 10.0 { 10.0 } else { hz };
        let top = rate * 0.476_190_497_815_745_13;
        let hz = if top < f64::from(hz) { top as f32 } else { hz };
        let q = 1.0f32;
        let w = ((f64::from(hz) * std::f64::consts::TAU) / f64::from(rate as f32)) as f32;
        let (s, c) = sin_cos_f32(w);
        let k = 1.0 / (s + q);
        let top = if self.high { c + 1.0 } else { 1.0 - c };
        self.b0 = flush(top / (s / q + 2.0));
        self.a1 = flush((c * k) * (q * -2.0));
        self.a2 = flush((q - s) * k);
    }

    fn clear(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }

    fn run(&mut self, x: f32) -> f32 {
        let x = flush(x);
        let x1 = self.x1;
        let sum = if self.high {
            (x - (x1 + x1)) + self.x2
        } else {
            ((x1 + x1) + x) + self.x2
        };
        let y = flush((sum * self.b0 - self.y1 * self.a1) - self.a2 * self.y2);
        self.x2 = x1;
        self.y2 = self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }
}

/// A line of the delay (`CTapeDelay`, run by `0x1800256a0`): Sakura's
/// modulated line with the two cuts on what it reads back.
#[derive(Debug, Clone, Default)]
struct CutLine {
    line: Stamped,
    points: f64,
    least: f64,
    start: f64,
    lfo: f64,
    lfo_step: f64,
    depth: f32,
    target: f64,
    step: f64,
    feedback: f32,
    gain_target: f32,
    gain: f32,
    low: Cut,
    high: Cut,
}

impl CutLine {
    fn new(points: u32, start: f64) -> Self {
        let points = points.max(4);
        let mut line = CutLine {
            line: Stamped::new(points as usize),
            points: f64::from(points),
            least: 1.0 / f64::from(points),
            start,
            gain_target: 1.0,
            high: Cut {
                high: true,
                ..Cut::default()
            },
            ..CutLine::default()
        };
        line.place(0.5);
        line.clear();
        line
    }

    fn place(&mut self, position: f64) {
        self.target = step_for(position, self.points);
    }

    fn clear(&mut self) {
        self.line.phase = self.start;
        self.step = self.target;
        self.gain = 0.0;
        self.lfo = 0.0;
        self.line.clear();
        self.low.clear();
        self.high.clear();
    }

    fn run(&mut self, x: f32) -> f32 {
        let x = flush(x);
        let lfo = self.lfo;
        let tri = if lfo >= 0.5 {
            1.0 - (((lfo - 0.5) + lfo) - 0.5)
        } else {
            lfo + lfo
        };
        let next = lfo + self.lfo_step;
        let mut speed = (3.0 - (tri + tri)) * tri * f64::from(self.depth) * tri + 1.0;
        if speed <= 1.0 {
            speed = 1.0;
        }
        self.lfo = if next >= 1.0 {
            next - f64::from(next as i32)
        } else {
            next
        };
        let out = self.line.read();
        let out = self.low.run(out);
        let out = self.high.run(out);
        let step = (speed * self.step).min(0.2).max(self.least);
        let value = out * self.feedback + x;
        self.line.write(value, step);
        let result = out * self.gain;
        self.gain =
            flush((f64::from(self.gain_target - self.gain) * 0.01 + f64::from(self.gain)) as f32);
        self.step += (self.target - self.step) * 0.005;
        result
    }
}

/// The delay (`CDelay`, `0x180027fb0`): a line on each side, of up to two
/// seconds, its time a sixteenth of the tempo times one to eight, each
/// adding what it reads back to its side.
#[derive(Debug, Clone)]
struct Delay {
    on: bool,
    time: [f32; 2],
    feedback: f32,
    depth: f32,
    wet: f32,
    low: f32,
    high: f32,
    tempo: f64,
    rate: f64,
    lines: Vec<CutLine>,
}

impl Default for Delay {
    fn default() -> Self {
        Delay {
            on: false,
            time: [0.5, 0.5],
            feedback: 0.0,
            depth: 0.0,
            wet: 0.5,
            low: 1000.0,
            high: 10.0,
            tempo: 120.0,
            rate: 0.0,
            lines: Vec::new(),
        }
    }
}

impl Delay {
    fn set_sample_rate(&mut self, rate: f64) {
        self.rate = rate;
        let points = (rate + rate) as i64 as u32;
        let mut l = CutLine::new(points, 0.0);
        let mut r = CutLine::new(points, 0.7);
        l.lfo_step = 1.0 / rate;
        r.lfo_step = 0.876 / rate;
        self.lines = vec![l, r];
        self.set_wet(self.wet);
        self.set_depth(self.depth);
        self.set_feedback(self.feedback);
        self.set_time(0, self.time[0]);
        self.set_time(1, self.time[1]);
        self.set_low(self.low);
        self.set_high(self.high);
        self.clear();
    }

    fn clear(&mut self) {
        for line in &mut self.lines {
            line.clear();
        }
    }

    fn set_on(&mut self, v: f32) {
        self.clear();
        self.on = v != 0.0;
    }

    /// `0x1800280d0`: one to eight sixteenths of the tempo.
    fn set_time(&mut self, side: usize, v: f32) {
        self.time[side] = v;
        let tempo = self.tempo;
        if let Some(line) = self.lines.get_mut(side) {
            let sixteenths = f64::from((f64::from(v) * 7.0) as i32) + 1.0;
            let d = 2.0 / ((0.266_666_666_666_666_66 / sixteenths) * tempo);
            line.place(d.clamp(0.0, 1.0));
        }
    }

    fn set_feedback(&mut self, v: f32) {
        self.feedback = v;
        let f = (f64::from(v) * 0.84) as f32;
        for line in &mut self.lines {
            line.feedback = f;
        }
    }

    fn set_depth(&mut self, v: f32) {
        self.depth = v;
        let d = (f64::from(v * v) * 0.01) as f32;
        for line in &mut self.lines {
            line.depth = d.clamp(0.0, 1.0);
        }
    }

    fn set_wet(&mut self, v: f32) {
        self.wet = v;
        for line in &mut self.lines {
            line.gain_target = v * v;
        }
    }

    fn set_low(&mut self, hz: f32) {
        self.low = hz;
        let rate = self.rate;
        for line in &mut self.lines {
            line.low.set(hz, rate);
        }
    }

    fn set_high(&mut self, hz: f32) {
        self.high = hz;
        let rate = self.rate;
        for line in &mut self.lines {
            line.high.set(hz, rate);
        }
    }

    /// `0x1800285e0`, each block: the song's tempo, and the times again.
    fn set_tempo(&mut self, tempo: f64) {
        if tempo != self.tempo {
            self.tempo = tempo;
            self.set_time(0, self.time[0]);
            self.set_time(1, self.time[1]);
        }
    }

    fn run(&mut self, left: &mut [f32], right: &mut [f32]) {
        if !self.on || self.lines.len() < 2 {
            return;
        }
        let (l_line, r_line) = self.lines.split_at_mut(1);
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            *l += l_line[0].run(*l);
            *r += r_line[0].run(*r);
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Knobs {
    pulse_a: f32,     // 0x59ec
    pw_a: f32,        // 0x59f0
    sync: bool,       // 0x5a0c
    ring: bool,       // 0x5a0d
    pulse_b: f32,     // 0x59f4
    pw_b: f32,        // 0x59f8
    semis_b: i32,     // 0x59fc
    detune_b: f32,    // 0x5a00
    balance: f32,     // 0x5a04
    noise: f32,       // 0x5a08
    eg_amount: f32,   // 0x5a44
    eg: [f32; 4],     // 0x5a34, 0x5a38, 0x5a3c, 0x5a40
    level: f32,       // 0x3900
    fine: f32,        // 0x5a8c
    transpose: f32,   // 0x5a88
    tune: f32,        // 0x38c8
    cutoff: f32,      // 0x5a10
    reso: f32,        // 0x5a14
    key_track: f32,   // 0x5a1c
    vel_track: f32,   // 0x5a18
    amp: [f32; 4],    // 0x5a24 attack, 0x5a28 decay, 0x5a30 sustain, 0x5a2c release
    lfo_amount: f32,  // 0x5a78
    detune: f32,      // 0x38f8
    unison_pan: f32,  // 0x38fc
    mw_amount: f32,   // 0x5a84
    mono: bool,       // 0x38e0
    octaver: bool,    // 0x38e1
    glide: f32,       // 0x38d0
    glide_mode: u8,   // 0x38d4
    eg_dest: i32,     // 0x5a48
    filter_mode: i32, // 0x5a20
    lfo_dest: i32,    // 0x5a7c
    mw_dest: i32,     // 0x5a80
    arp: i32,         // 0x398c and the arpeggiator's mode
    unison: i32,      // 0x38f0
    polyphony: i32,   // 0x38e4
    bend_range: f32,  // 0x38dc
    delay: bool,      // the delay's switch
    gate_mix: f32,    // gate +0
    gate_smooth: f32, // gate +4
    gate_steps: [f32; 16],
    gate_speed: f64, // gate +0x50
}

impl Default for Knobs {
    fn default() -> Self {
        Knobs {
            pulse_a: 0.0,
            pw_a: 0.0,
            sync: false,
            ring: false,
            pulse_b: 0.0,
            pw_b: 0.0,
            semis_b: 0,
            detune_b: 0.0,
            balance: 0.0,
            noise: 0.0,
            eg_amount: 0.0,
            eg: [0.0; 4],
            level: 1.0,
            fine: 0.0,
            transpose: 0.0,
            tune: 0.0,
            cutoff: 0.0,
            reso: 0.0,
            key_track: 0.0,
            vel_track: 0.0,
            amp: [0.0; 4],
            lfo_amount: 0.0,
            detune: 0.0,
            unison_pan: 0.0,
            mw_amount: 0.0,
            mono: false,
            octaver: false,
            glide: 1e-4,
            glide_mode: 1,
            eg_dest: 0,
            filter_mode: 0,
            lfo_dest: 0,
            mw_dest: 0,
            arp: 0,
            unison: 1,
            polyphony: 64,
            bend_range: 1.0,
            delay: false,
            gate_mix: 0.0,
            gate_smooth: 0.0,
            gate_steps: [1.0; 16],
            gate_speed: 0.5,
        }
    }
}

/// The knobs the engine moves 4% a block toward where they are set
/// (`0x5a90..0x5aac`).
#[derive(Debug, Clone, Copy, Default)]
struct Smooth {
    pw_a: f32,
    pw_b: f32,
    balance: f32,
    noise: f32,
    lfo_amount: f32,
    eg_amount: f32,
    mw_amount: f32,
    cutoff: f32,
}

/// One `PoiZoneGenerator` (0x1f8 bytes), by the engine's offsets.
#[derive(Debug, Clone, Copy, Default)]
struct Voice {
    key: i32,        // 0x10
    unison: i8,      // 0x14
    count: i8,       // 0x15
    random: f32,     // 0x18
    velocity: f32,   // 0x20
    released: bool,  // 0x24
    glide_rate: f32, // 0x2c
    note: f32,       // 0x30
    age: u32,        // 0x34
    a: f32,          // 0x48
    a_step: f32,     // 0x4c
    a2: f32,         // 0x50
    ga: f32,         // 0x54
    ga_step: f32,    // 0x58
    b: f32,          // 0x5c
    b_step: f32,     // 0x60
    b2: f32,         // 0x64
    gb: f32,         // 0x68
    gb_step: f32,    // 0x6c
    x1: f32,         // 0x70
    s: [f32; 3],     // 0x74..0x7c
    y: [f32; 4],     // 0x80..0x8c
    fb: f32,         // 0x90
    fa: f32,         // 0x94
    res: f32,        // 0x98
    m4: f32,         // 0x9c
    m3: f32,         // 0xa0
    mx: f32,         // 0xa4
    noise: f32,      // 0xa8
    noise_step: f32, // 0xac
    amp: Env,        // 0xb0
    gl: f32,         // 0x130
    gl_step: f32,    // 0x134
    gr: f32,         // 0x138
    gr_step: f32,    // 0x13c
    sl: f32,         // 0x140
    sr: f32,         // 0x144
    dc: f32,         // 0x148
    eg: Env,         // 0x150
    lfo: Lfo,        // 0x1d0
}

/// The plugin.
pub struct PoiZone {
    params: [f32; PARAMS],
    program: [u8; 64],
    version: [u8; 4],
    knobs: Knobs,
    smooth: Smooth,
    voices: Vec<Voice>,
    /// The voices in the engine's order (`0x39c8`): sounding first.
    order: Vec<u16>,
    active: usize,
    resort: bool,
    owner: [Option<u32>; 128],
    key_gain: [[f32; 128]; 2],
    key_pitch: [f32; 128],
    held: [u8; 128],
    last_key: i32,
    ages: u32,
    rng: u32,
    rng2: u32,
    rng3: u32,
    rate: f64,
    tempo: f64,
    lfo: Lfo,
    down: [f32; 3],
    down_state: [[f32; 4]; 2],
    gate_pos: f64,
    gate_step: usize,
    gate_gain: f32,
    shelf: [Shelf; 2],
    chorus: Chorus,
    delay: Delay,
    gains: [f32; 2],
    over: [Vec<f32>; 2],
}

/// The parameters of a new instance (`emu`, read from the engine's table
/// after its constructor).
const DEFAULTS: [f32; 0x51] = [
    0.0,
    1.0,
    0.0,
    0.0,
    0.0,
    1.0,
    0.5,
    0.0,
    0.0,
    0.0,
    0.5,
    0.0,
    1.0,
    1.0,
    1.0,
    0.5,
    0.5,
    0.5,
    1.0,
    0.0,
    0.0,
    0.0,
    0.0,
    1.0,
    1.0,
    0.0,
    0.5,
    0.0,
    0.2,
    0.1,
    0.0,
    0.0,
    0.428_571_43,
    0.428_571_43,
    0.5,
    0.3,
    0.0,
    1.0,
    0.5,
    0.0,
    0.5,
    0.5,
    0.4,
    0.5,
    0.0,
    0.0,
    0.0,
    0.0,
    0.0,
    1.0,
    0.0,
    1.0,
    0.0,
    1.0,
    1.0,
    1.0,
    0.0,
    1.0,
    0.0,
    1.0,
    0.0,
    1.0,
    1.0,
    1.0,
    0.0,
    0.0,
    0.0,
    0.0,
    0.5,
    0.0,
    0.0,
    0.0,
    0.0,
    0.0,
    0.0,
    0.0,
    12.0,
    1.0,
    0.0,
    0.0,
    0.0,
];

impl Default for PoiZone {
    fn default() -> Self {
        // The constructor draws two phases for each voice and one spare
        // from the noise generator, which starts at 22222.
        let mut rng = 22_222u32;
        let mut voices = vec![
            Voice {
                key: -1,
                ..Voice::default()
            };
            VOICES
        ];
        for v in voices.iter_mut() {
            rng = step(rng);
            v.a = unit(rng).clamp(0.0, 1.0);
            rng = step(rng);
            v.b = unit(rng).clamp(0.0, 1.0);
            v.a2 = v.a;
            v.b2 = v.b;
        }
        rng = step(step(rng));
        let mut synth = PoiZone {
            params: [0.0; PARAMS],
            program: [0; 64],
            version: *b"4reV",
            knobs: Knobs::default(),
            smooth: Smooth::default(),
            voices,
            order: (0..VOICES as u16).collect(),
            active: 0,
            resort: true,
            owner: [None; 128],
            key_gain: [[std::f32::consts::FRAC_1_SQRT_2; 128]; 2],
            key_pitch: [0.0; 128],
            held: [0; 128],
            last_key: -1,
            ages: 0,
            rng,
            rng2: 2222,
            rng3: 2222,
            rate: 0.0,
            tempo: 120.0,
            lfo: Lfo::default(),
            down: [0.0; 3],
            down_state: [[0.0; 4]; 2],
            gate_pos: 0.0,
            gate_step: 0,
            gate_gain: 1.0,
            shelf: [Shelf::default(); 2],
            chorus: Chorus::default(),
            delay: Delay::default(),
            gains: [0.0; 2],
            over: [vec![0.0; BLOCK * OVER], vec![0.0; BLOCK * OVER]],
        };
        synth.program[..8].copy_from_slice(b"<noname>");
        synth.defaults();
        synth.set_rate(44_100.0);
        synth
    }
}

impl PoiZone {
    fn defaults(&mut self) {
        for (i, v) in DEFAULTS.iter().enumerate() {
            self.set(i, *v);
        }
        for i in 0x51..PARAMS {
            self.set(i, if i < 0x9b { -1.0 } else { 0.0 });
        }
    }

    /// `0x18002e3f0`: store a parameter and set what it sets.
    fn set(&mut self, n: usize, v: f32) {
        if n >= PARAMS {
            return;
        }
        self.params[n] = v;
        // The panel's ranges: knobs 0..1, the lists' indices 0..127. The
        // engine takes what it is given; a value past these would run its
        // smoothing to infinity.
        let v = if v.is_nan() {
            0.0
        } else if n < 75 {
            v.clamp(0.0, 1.0)
        } else if n <= 80 {
            v.clamp(0.0, 127.0)
        } else {
            v
        };
        let k = &mut self.knobs;
        let ms = 0.001f32;
        match n {
            0 => k.pulse_a = if v != 0.0 { 1.0 } else { 0.0 },
            1 => k.pw_a = v,
            2 => k.sync = v != 0.0,
            3 => k.ring = v != 0.0,
            4 => k.pulse_b = if v != 0.0 { 1.0 } else { 0.0 },
            5 => k.pw_b = v,
            6 => k.semis_b = -24 - (v * -48.0) as i32,
            7 => k.detune_b = v * 0.5,
            8 => k.balance = v,
            9 => k.noise = v,
            10 => k.eg_amount = (v + v) - 1.0,
            11 | 12 | 14 => k.eg[if n == 14 { 3 } else { n - 11 }] = quartic(v, 5.0, 50_000.0) * ms,
            13 => k.eg[2] = v,
            15 => k.level = powf(10.0, v * 1.5 - 1.0),
            16 => {
                k.fine = (v + v) - 1.0;
                k.tune = k.fine + k.transpose;
            }
            17 => {
                k.transpose = (-24 - (v * -48.0) as i32) as f32;
                k.tune = k.transpose + k.fine;
            }
            18 => k.cutoff = v,
            19 => k.reso = v,
            20 => k.key_track = v,
            21 => k.vel_track = v,
            22 => k.amp[0] = quartic(v, 1.0, 5000.0) * ms,
            23 => k.amp[1] = quartic(v, 5.0, 20_000.0) * ms,
            24 => k.amp[2] = (v * v).clamp(0.0, 1.0),
            25 => k.amp[3] = quartic(v, 15.0, 10_000.0) * ms,
            26 => k.lfo_amount = (v + v) - 1.0,
            27 => self.lfo.retrig = v != 0.0,
            28 => {
                let f = ((v * v) * v) * (v * v) * 30.0;
                self.lfo.speed = f64::from(f.clamp(0.0, 30.0));
            }
            29 => k.detune = v * 1.5,
            30 => k.unison_pan = v.clamp(0.0, 1.0),
            31 => {
                k.delay = v != 0.0;
                self.delay.set_on(v);
            }
            32 => self.delay.set_time(0, v),
            33 => self.delay.set_time(1, v),
            34 => self.delay.set_feedback(v),
            35 => self.delay.set_depth(v),
            36 => self.delay.set_high(powf(2.0, v * 10.0) * 10.0),
            37 => self
                .delay
                .set_low(powf(2.0, v * f32::from_bits(0x40d3_3333)) * 200.0),
            38 => self.delay.set_wet(v),
            39 => self.chorus.on = v != 0.0,
            40 => self.chorus.set_rate(v),
            41 => self.chorus.set_depth_f64(v),
            42 => self.chorus.set_wet(v),
            43 => k.mw_amount = (v + v) - 1.0,
            44 => {
                let mono = v != 0.0;
                if k.mono != mono {
                    k.mono = mono;
                    self.all_keys_off();
                }
            }
            45 => k.octaver = v != 0.0,
            46 => {
                let g = ((v * v) * v).clamp(0.0, 1.0);
                let g = g + g;
                k.glide = if f64::from(g) < 0.0001 {
                    f32::from_bits(0x38d1_b717)
                } else {
                    g
                };
            }
            47 => k.gate_mix = v.clamp(0.0, 1.0),
            48 => {
                let s = 1.0 - v.clamp(0.0, 1.0);
                k.gate_smooth = s * s * 0.01 + f32::from_bits(0x38d1_b717);
            }
            49..=64 => k.gate_steps[n - 49] = 1.0 - (v as i32) as f32,
            65 => k.eg_dest = choice(v, 4.0, 0x39d1_b717, 4),
            66 => k.filter_mode = choice(v + v, 1.0, 0x3951_b717, 2),
            67 => k.lfo_dest = choice(v, 5.0, 0x3a03_126e, 5),
            68 => self.lfo.shape = choice(v, 4.0, 0x39d1_b717, 4),
            69 => {
                let arp = choice(v, 4.0, 0x39d1_b717, 4);
                if (arp != 0) != (k.arp != 0) {
                    k.arp = arp;
                    self.all_keys_off();
                }
                self.knobs.arp = arp;
            }
            71 => k.mw_dest = choice(v, 4.0, 0x39d1_b717, 4),
            72 | 73 => {
                let (a, b) = (self.params[72] != 0.0, self.params[73] != 0.0);
                k.glide_mode = match (a, b) {
                    (false, false) => 0,
                    (false, true) => 2,
                    (true, false) => 1,
                    (true, true) => 3,
                };
            }
            75 => {
                let u = match v as i32 {
                    0 => 1,
                    1 => 2,
                    2 => 3,
                    3 => 4,
                    4 => 8,
                    5 => 12,
                    6 => 16,
                    _ => k.unison,
                };
                k.unison = u.min(16);
            }
            76 => {
                let p = v as i32 + 1;
                k.polyphony = if !(1..=64).contains(&p) { 64 } else { p };
            }
            78 => k.bend_range = (v as i32 + 1) as f32,
            79 => {
                if let Some(s) = [0.5, 1.0, 2.0, 3.0, 4.0, 8.0, 16.0].get(v as i32 as usize) {
                    k.gate_speed = *s;
                }
            }
            80 => {
                let i = (v as i32).clamp(0, 10) as usize;
                self.lfo.sync = [-1.0, 1.0, 2.0, 3.0, 4.0, 8.0, 12.0, 16.0, 32.0, 64.0, 128.0][i];
            }
            _ => {}
        }
    }

    fn set_rate(&mut self, rate: f64) {
        self.all_keys_off();
        self.rate = rate;
        // `FUN_1800240d0`: each voice is made over (`FUN_18002d2f0`) and
        // its note set to 48.
        for i in 0..VOICES {
            let v = &mut self.voices[i];
            v.x1 = 0.0;
            v.s = [0.0; 3];
            v.y = [0.0; 4];
            v.fb = 0.0;
            v.fa = 0.0;
            v.res = 0.0;
            v.m4 = 0.0;
            v.m3 = 0.0;
            v.mx = 0.0;
            v.a_step = 0.0;
            v.b_step = 0.0;
            v.ga = 0.0;
            v.gb = 0.0;
            v.noise = 0.0;
            self.kill_voice(i);
            self.voices[i].note = 48.0;
        }
        // `0x180033730`: the effects.
        for s in &mut self.shelf {
            s.set(rate as f32);
            s.s = [0.0; 4];
        }
        self.delay.set_sample_rate(rate);
        self.chorus.set_sample_rate(rate);
        // The engine hands the lines a depth of 1 when the rate changes.
        self.chorus.set_depth(1.0);
        self.chorus.clear();
        // The downsampler (`0x180033090`): a biquad at pi/12 of the 8x rate.
        let w = (rate * 0.261_799_387_799_149_4) / rate;
        let (s, c) = (w.sin(), w.cos());
        let inv = 1.0 / (s + 2.0);
        self.down = [
            ((1.0 - c) / (s * 0.5 + 2.0)) as f32,
            ((c * -4.0) * inv) as f32,
            ((2.0 - s) * inv) as f32,
        ]
        .map(flush);
        self.down_state = [[0.0; 4]; 2];
        self.lfo.phase = 0.0;
        let k = &self.knobs;
        self.smooth = Smooth {
            pw_a: k.pw_a,
            pw_b: k.pw_b,
            balance: k.balance,
            noise: k.noise,
            lfo_amount: k.lfo_amount,
            eg_amount: k.eg_amount,
            mw_amount: k.mw_amount,
            cutoff: k.cutoff,
        };
        self.gains = [self.master(), self.master()];
        self.last_key = -1;
    }

    /// `FUN_180024f10`: the master level; FL's volume and pan for the
    /// channel are 1 and the centre here, which leaves the pan out.
    fn master(&self) -> f32 {
        let g = self.knobs.level * 1.0 * 0.5;
        g.max(0.0)
    }

    fn all_keys_off(&mut self) {
        for key in 0..128 {
            self.key_off(key);
        }
    }

    /// `FUN_180025d90`: a voice's pitch in semitones.
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
        let bend = k.bend_range * 0.0;
        (((((k.tune + v.note) + (spread + key)) + octave) + v.random) + bend) + 0.0
    }

    /// `FUN_180025d50`: a voice's frequency in Hz.
    fn freq_of(&self, v: &Voice) -> f32 {
        let p = (self.pitch_of(v) - 57.0) * f32::from_bits(0x3daa_aaab);
        ((2.0f64).powf(f64::from(p)) * 220.0) as f32
    }

    /// `FUN_180025e30`: a voice's gain on one side.
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

    /// The nearest key held but `key` (the glide's modes 1 and 3).
    fn nearest_held(&self, key: i32) -> Option<i32> {
        let mut best = 0x80;
        let mut to = None;
        for k in 0..128 {
            if self.held[k] != 0 && k as i32 != key && (key - k as i32).abs() < best {
                best = (key - k as i32).abs();
                to = Some(k as i32);
            }
        }
        to
    }

    fn glide_rate(&self, key: i32, note: f32) -> f32 {
        let k = &self.knobs;
        if k.glide_mode <= 1 {
            (12.0 / (self.rate * f64::from(k.glide))) as f32
        } else {
            (f64::from((key as f32 - note).abs()) / (self.rate * f64::from(k.glide))) as f32
        }
    }

    fn draw_random(&mut self) -> f32 {
        self.rng2 = step(self.rng2);
        unit(self.rng2) - 1.0
    }

    /// `FUN_180024300` with a velocity: unison voices for the key.
    fn key_on(&mut self, key: i32, velocity: f32) {
        self.held[key as usize] = (velocity * 127.0) as i32 as u8;
        if self.knobs.polyphony == 0 {
            return;
        }
        if self.knobs.mono {
            self.mono_on(key, velocity);
            return;
        }
        let random = self.draw_random();
        let mut from = key;
        if 0.0 <= self.last_key as f32 {
            from = self.last_key;
        }
        let mut from = from as f32;
        if self.knobs.glide_mode == 1 || self.knobs.glide_mode == 3 {
            from = key as f32;
            if let Some(k) = self.nearest_held(key) {
                from = k as f32;
            }
        }
        let k = self.knobs;
        let limit = ((k.polyphony.min(64) * k.unison).max(0) as usize).min(VOICES);
        for u in 0..k.unison {
            let mut taken = None;
            let (mut oldest, mut oldest_free) = (0, 0);
            for i in 0..limit {
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
            let age = self.ages;
            self.ages = self.ages.wrapping_add(1);
            let v = &mut self.voices[i];
            v.age = age;
            v.key = key;
            v.unison = u as i8;
            v.count = k.unison as i8;
            v.random = (random + random) * 0.01;
            v.velocity = powf(velocity, 1.0);
            v.note = from;
            v.released = false;
            if f64::from(k.glide) < 0.001 {
                v.note = key as f32;
            }
            let note = v.note;
            self.voices[i].glide_rate = self.glide_rate(key, note);
            self.begin(i);
        }
        self.last_key = key;
    }

    /// `FUN_18002d0e0`: a voice's envelopes and LFO start.
    fn begin(&mut self, i: usize) {
        let k = self.knobs;
        let rate = self.rate * OVER as f64;
        let lfo = self.lfo;
        let v = &mut self.voices[i];
        let amp_curve = [0.0, -2.5, -2.5];
        v.amp.start(
            f64::from(k.amp[0]),
            f64::from(k.amp[1]),
            f64::from(k.amp[2]),
            amp_curve,
            rate,
            f64::from(v.velocity),
        );
        v.eg.done = true;
        v.eg.start(
            f64::from(k.eg[0]),
            f64::from(k.eg[1]),
            f64::from(k.eg[2]),
            [-2.0; 3],
            rate,
            1.0,
        );
        v.lfo = lfo;
        if v.lfo.retrig {
            v.lfo.phase = 0.0;
        }
        self.resort = true;
    }

    /// The release of every voice on the key (`FUN_180024300` with no
    /// velocity, `FUN_18002d210`).
    fn key_off(&mut self, key: i32) {
        self.held[key as usize] = 0;
        if self.knobs.polyphony == 0 {
            return;
        }
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
        v.amp.let_go(f64::from(k.amp[3]));
        v.eg.let_go(f64::from(k.eg[3]));
    }

    /// `FUN_1800248f0` with a velocity: the unison voices at the head of
    /// the list move to the newest key, and start again only if they were
    /// idle or let go of.
    fn mono_on(&mut self, key: i32, velocity: f32) {
        let random = self.draw_random();
        let mut from = key;
        if 0.0 <= self.last_key as f32 {
            from = self.last_key;
        }
        let mut from = from as f32;
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
            let v = &mut self.voices[u];
            v.age = age;
            v.key = key;
            v.random = (random + random) * 0.01;
            v.released = false;
            v.unison = u as i8;
            v.count = k.unison as i8;
            let note = v.note;
            let rate = self.glide_rate(key, note);
            let v = &mut self.voices[u];
            v.glide_rate = rate;
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
        let to = self.nearest_held(key);
        for i in 0..VOICES {
            if self.voices[i].key != key {
                continue;
            }
            match to {
                Some(k) => {
                    let age = self.ages;
                    self.ages = self.ages.wrapping_add(1);
                    let rate = (12.0 / (self.rate * f64::from(self.knobs.glide))) as f32;
                    let v = &mut self.voices[i];
                    v.age = age;
                    v.key = k;
                    v.glide_rate = rate;
                    self.last_key = k;
                }
                None => {
                    self.voices[i].released = true;
                    self.release_voice(i);
                }
            }
        }
    }

    /// `FUN_18002d280`: a voice stops.
    fn kill_voice(&mut self, i: usize) {
        let v = &mut self.voices[i];
        v.gl = 0.0;
        v.gl_step = 0.0;
        v.gr = 0.0;
        v.gr_step = 0.0;
        v.sl = 0.0;
        v.sr = 0.0;
        v.ga_step = 0.0;
        v.gb_step = 0.0;
        v.noise_step = 0.0;
        v.amp.done = true;
        v.eg.done = true;
        v.key = -1;
        self.resort = true;
    }

    /// `FUN_18002c580`: a voice's rates for the next `n` samples at 8x.
    fn update(&mut self, i: usize, n: i32) {
        // `FUN_180025c90`: the glide.
        {
            let v = &mut self.voices[i];
            let to = v.key as f32;
            let mut note = v.note;
            let mut clamp = false;
            if note < to {
                note += n as f32 * v.glide_rate;
                v.note = note;
                if to < note {
                    clamp = true;
                }
            }
            if !clamp && to < note {
                note -= n as f32 * v.glide_rate;
                v.note = note;
                if note < to {
                    clamp = true;
                }
            }
            if clamp {
                v.note = to;
            }
        }
        if self.voices[i].amp.done {
            self.kill_voice(i);
            return;
        }
        let k = self.knobs;
        let sm = self.smooth;
        let rate = self.rate * OVER as f64;
        let tempo = self.tempo as f32;
        let (eg, mut lfo) = {
            let mut rng3 = self.rng3;
            let v = &mut self.voices[i];
            let eg = v.eg.run(n);
            v.lfo.speed = self.lfo.speed;
            v.lfo.shape = self.lfo.shape;
            v.lfo.sync = self.lfo.sync;
            let lfo = v.lfo.run(rate, f64::from(tempo), n, &mut rng3);
            self.rng3 = rng3;
            (eg, lfo)
        };
        // The mod wheel (`0x126c`) reads 0.
        let wheel = 0.0f32;
        if k.mw_dest == 1 {
            lfo *= sm.mw_amount * wheel;
        }
        let amp = self.voices[i].amp.run(n) * 10.0;
        let mut pan = 0.5f32;
        if k.lfo_dest == 5 {
            pan = (lfo * sm.lfo_amount + 1.0) * 0.5;
        }
        let pan = pan.clamp(0.0, 1.0);
        let inv = 1.0 / n as f32;
        let vv = self.voices[i];
        let left = self.side(&vv, false);
        let right = self.side(&vv, true);
        // Oscillator A (`FUN_18002cb70`).
        let f0 = self.freq_of(&vv);
        let mut fa = f0;
        let mut pw = sm.pw_a;
        if (1..=2).contains(&k.eg_dest) {
            let semis = ((sm.eg_amount * 24.0) as i32 as f32 * eg) * f32::from_bits(0x3daa_aaab);
            fa = ((2.0f64).powf(f64::from(semis)) * f64::from(fa)) as f32;
        }
        if (3..=4).contains(&k.eg_dest) {
            pw += eg * sm.eg_amount;
        }
        if (1..=2).contains(&k.lfo_dest) {
            let a = sm.lfo_amount;
            let e = (a * a) * (a * a) * lfo * if a > 0.0 { 2.0 } else { -2.0 };
            fa *= powf(2.0, e);
        }
        if (3..=4).contains(&k.lfo_dest) {
            pw += lfo * sm.lfo_amount;
        }
        if k.mw_dest == 2 {
            let a = sm.mw_amount;
            fa *= powf(2.0, (a * a) * a * wheel * 2.0);
        } else if (3..=4).contains(&k.mw_dest) {
            pw += sm.mw_amount * wheel;
        }
        // Oscillator B (`FUN_18002ce40`).
        let mut fb = {
            let semis = (k.semis_b as f32 + k.detune_b) * f32::from_bits(0x3daa_aaab);
            (f64::from(f0) * (2.0f64).powf(f64::from(semis))) as f32
        };
        let mut pwb = sm.pw_b;
        if k.eg_dest == 2 {
            let semis = ((sm.eg_amount * 24.0) as i32 as f32 * eg) * f32::from_bits(0x3daa_aaab);
            fb = ((2.0f64).powf(f64::from(semis)) * f64::from(fb)) as f32;
        } else if k.eg_dest == 4 {
            pwb += eg * sm.eg_amount;
        }
        if k.lfo_dest == 2 {
            let a = sm.lfo_amount;
            let e = (a * a) * (a * a) * lfo * if a > 0.0 { 2.0 } else { -2.0 };
            fb *= powf(2.0, e);
        } else if k.lfo_dest == 4 {
            pwb += lfo * sm.lfo_amount;
        }
        if k.mw_dest == 4 {
            pwb += sm.mw_amount * wheel;
        }
        // The filter (`FUN_18002c8a0`).
        let pitch = self.pitch_of(&vv);
        let mut x = f64::from((pitch - 56.0) * k.key_track) * 0.0084
            + ((f64::from(vv.velocity) - 0.5) * f64::from(k.vel_track)) * 0.5;
        // FL's mod X (`0x620`) reads 0.
        x += 0.0;
        let signed = |x: f64, amount: f32, by: f32| {
            let d = f64::from(amount * amount * by);
            if amount > 0.0 { x + d } else { x - d }
        };
        if k.eg_dest == 0 {
            x = signed(x, sm.eg_amount, eg);
        }
        if k.lfo_dest == 0 {
            x = signed(x, sm.lfo_amount, lfo);
        }
        if k.mw_dest == 0 {
            x = signed(x, sm.mw_amount, wheel);
        }
        let x = (f64::from(sm.cutoff) * 1.1 + x).clamp(0.0, 0.91);
        let w = (2.0f64).powf(x * 10.0 + 6.0) / rate;
        let fa_coef = ((w * 3.6 - (w * 1.6) * w) - 1.0) as f32;
        let e = (1.386_249 - x * 1.523_350_549_450_549_3).exp() + 1.0;
        // FL's mod Y (`0x820`) reads 0.
        let r = f64::from(0.0 + k.reso).clamp(0.0, 1.0);
        let res = ((e * r) * 2.053_44) as f32;
        let (mut lp, mut bp, mut hp) = (0.0f32, 0.0f32, 0.0f32);
        match k.filter_mode {
            0 => lp = 1.0,
            1 => bp = 1.0,
            2 => hp = 1.0,
            _ => {}
        }
        let bp = bp * 3.0;
        let rate_half = rate * 0.5;
        let v = &mut self.voices[i];
        // The gains (`FUN_18002c580`).
        let gl = v.gl;
        v.gl_step = (left * amp * pan - gl) * inv;
        let gr = v.gr;
        v.gr_step = (right * amp * (1.0 - pan) - gr) * inv;
        v.gl = flush(gl);
        v.gl_step = flush(v.gl_step);
        v.gr = flush(gr);
        v.gr_step = flush(v.gr_step);
        v.sl = flush(v.sl);
        v.sr = flush(v.sr);
        let noise = v.noise;
        v.noise_step = flush((sm.noise - noise) * inv);
        v.noise = flush(noise);
        // Oscillator A's steps.
        let mut fa = fa.max(0.0);
        let pw = pw.clamp(0.0, 1.0);
        if f64::from(fa) > rate_half {
            fa = rate_half as f32;
        }
        let a_step = (f64::from(fa) / rate) as f32;
        v.a_step = a_step;
        let ga = v.ga;
        v.ga_step = flush(((1.0 - sm.balance) * (1.0 - sm.noise) - ga) / n as f32);
        v.ga = flush(ga);
        let a = v.a;
        let mut a2 = a - (0.99 - f64::from(pw) * 0.49) as f32;
        if a2 < 0.0 {
            a2 += 1.0;
        }
        v.a2 = flush(a2);
        v.a_step = flush(a_step);
        v.a = flush(a);
        // Oscillator B's steps.
        let mut fb = fb.max(0.0);
        let pwb = pwb.clamp(0.0, 1.0);
        if f64::from(fb) > rate_half {
            fb = rate_half as f32;
        }
        let b_step = (f64::from(fb) / rate) as f32;
        let gb = v.gb;
        v.gb_step = flush(((1.0 - sm.noise) * sm.balance - gb) / n as f32);
        v.gb = flush(gb);
        let b = v.b;
        let mut b2 = b - (0.99 - f64::from(pwb) * 0.49) as f32;
        if b2 < 0.0 {
            b2 += 1.0;
        }
        v.b2 = flush(b2);
        v.b_step = flush(b_step);
        v.b = flush(b);
        // The filter's coefficients.
        v.fa = fa_coef;
        v.fb = (fa_coef + 1.0) * 0.5;
        v.res = res;
        v.mx = hp;
        v.m3 = -bp;
        v.m4 = (lp - hp) + bp;
    }

    /// `0x18002d340`: one voice for `n` samples at 8x.
    fn run_one(&mut self, i: usize, n: usize, first: bool) {
        let k = self.knobs;
        let smooth = f32::from_bits(self.smooth_bits());
        let mut rng = self.rng;
        let v = &mut self.voices[i];
        let [l_out, r_out] = &mut self.over;
        for j in 0..n {
            let mut a = v.a - v.a_step;
            let mut a2 = v.a2 - v.a_step;
            let mut b = v.b - v.b_step;
            let mut b2 = v.b2 - v.b_step;
            rng = step(rng);
            let noise = ((f64::from(rng) * 2.328_306_437_080_797_4e-10) as f32 - 0.125) * v.noise;
            if a < 0.0 {
                a += 1.0;
            }
            if a2 < 0.0 {
                a2 += 1.0;
            }
            let mut wrap = 0.0;
            if b < 0.0 {
                b += 1.0;
                wrap = 1.0;
            }
            if b2 < 0.0 {
                b2 += 1.0;
            }
            if k.sync {
                let t = a * wrap;
                a2 -= t;
                a -= t;
            }
            v.a = a;
            v.a2 = a2;
            v.b = b;
            v.b2 = b2;
            let ob = b - b2 * k.pulse_b;
            let oa = a - a2 * k.pulse_a;
            let gb = v.gb * ob;
            let oa = if k.ring { ob * oa } else { oa };
            let mut x = ((v.ga * oa + gb) + noise) - 0.42;
            x -= v.y[3] * v.res;
            if x < 0.0 {
                x /= (x * x) * 3.0 + 1.0;
            }
            let (fa, fb) = (v.fa, v.fb);
            let y1 = (x + v.x1) * fb - fa * v.y[0];
            v.x1 = x;
            let y2 = (y1 + v.s[0]) * fb - fa * v.y[1];
            v.s[0] = y1;
            let y3 = (y2 + v.s[1]) * fb - fa * v.y[2];
            v.s[1] = y2;
            let y4 = (y3 + v.s[2]) * fb - fa * v.y[3];
            v.s[2] = y3;
            v.y = [y1, y2, y3, y4];
            let mut out = (y3 * v.m3 + y4 * v.m4) + x * v.mx;
            let dc = (out - v.dc) * f32::from_bits(0x38d1_b717) + v.dc;
            out -= dc;
            v.dc = flush(dc);
            v.gl += v.gl_step;
            v.gr += v.gr_step;
            v.noise += v.noise_step;
            v.ga += v.ga_step;
            v.gb += v.gb_step;
            v.sl = (v.gl - v.sl) * smooth + v.sl;
            v.sr = (v.gr - v.sr) * smooth + v.sr;
            let (l, r) = (out * v.sl, out * v.sr);
            if first {
                l_out[j] = l;
                r_out[j] = r;
            } else {
                l_out[j] += l;
                r_out[j] += r;
            }
        }
        self.rng = rng;
    }

    /// The engine's per-sample gain step (`0x59e8`): `rate * 6.8e-8`.
    fn smooth_bits(&self) -> u32 {
        ((self.rate * 6.802_721_088_435_375e-8) as f32).to_bits()
    }

    /// `0x180031760`: four voices for `n` samples at 8x, as the SSE loop
    /// sums them.
    fn run_four(&mut self, lanes: [usize; 4], n: usize, first: bool) {
        let k = self.knobs;
        let smooth = f32::from_bits(self.smooth_bits());
        let mut v = lanes.map(|i| self.voices[i]);
        let mut rng = self.rng;
        let [l_out, r_out] = &mut self.over;
        for j in 0..n {
            let mut ls = [0.0f32; 4];
            let mut rs = [0.0f32; 4];
            let mut draws = [0u32; 4];
            for d in draws.iter_mut() {
                rng = step(rng);
                *d = rng;
            }
            for (lane, v) in v.iter_mut().enumerate() {
                let noise = ((f64::from(draws[lane]) * 2.328_306_437_080_797_4e-10) as f32 - 0.125)
                    * v.noise;
                let mut a = v.a - v.a_step;
                let mut a2 = v.a2 - v.a_step;
                let mut b = v.b - v.b_step;
                let mut b2 = v.b2 - v.b_step;
                if a <= 0.0 {
                    a += 1.0;
                }
                if a2 <= 0.0 {
                    a2 += 1.0;
                }
                if b2 <= 0.0 {
                    b2 += 1.0;
                }
                let wrap = if b <= 0.0 { 1.0 } else { 0.0 };
                b += wrap;
                if k.sync {
                    let t = wrap * a;
                    a2 -= t;
                    a -= t;
                }
                v.a = a;
                v.a2 = a2;
                v.b = b;
                v.b2 = b2;
                let oa = a - k.pulse_a * a2;
                let ob = b - k.pulse_b * b2;
                let sum = if k.ring {
                    (ob * v.ga) * oa + ob * v.gb
                } else {
                    ob * v.gb + oa * v.ga
                };
                let mut x = noise + sum;
                x -= 0.42;
                let fbk = v.res * v.y[3];
                v.noise += v.noise_step;
                v.ga += v.ga_step;
                v.gb += v.gb_step;
                x -= fbk;
                let m = if x <= 0.0 { 1.0 } else { 0.0 };
                x /= ((m * 3.0) * x) * x + 1.0;
                let (fa, fb) = (v.fa, v.fb);
                let y1 = (x + v.x1) * fb - fa * v.y[0];
                v.x1 = x;
                let y2 = (y1 + v.s[0]) * fb - fa * v.y[1];
                v.s[0] = y1;
                let mx = v.mx * x;
                let y3 = (y2 + v.s[1]) * fb - fa * v.y[2];
                let y4 = (y3 + v.s[2]) * fb - fa * v.y[3];
                v.s[2] = y3;
                v.s[1] = y2;
                v.y = [y1, y2, y3, y4];
                let mut out = (mx + v.m3 * y3) + v.m4 * y4;
                v.gr += v.gr_step;
                v.dc += (out - v.dc) * f32::from_bits(0x38d1_b717);
                v.gl += v.gl_step;
                out -= v.dc;
                ls[lane] = out * v.sl;
                rs[lane] = out * v.sr;
                v.sl += (v.gl - v.sl) * smooth;
                v.sr += (v.gr - v.sr) * smooth;
            }
            let l = ((ls[1] + ls[0]) + ls[2]) + ls[3];
            let r = ((rs[1] + rs[0]) + rs[2]) + rs[3];
            if first {
                l_out[j] = l;
                r_out[j] = r;
            } else {
                l_out[j] += l;
                r_out[j] += r;
            }
        }
        self.rng = rng;
        // The loop hands the oscillators' gains back to the voices in the
        // lanes' reverse order (`0x180032857`..`0x1800328c2`): the first
        // voice takes the fourth's, and so on.
        for (lane, i) in lanes.iter().enumerate() {
            let back = v[3 - lane];
            self.voices[*i] = Voice {
                ga: back.ga,
                gb: back.gb,
                ..v[lane]
            };
        }
    }

    /// `FUN_180031260`: sounding voices first, each part in list order.
    fn sort(&mut self) {
        let mut n = 0;
        for i in 0..VOICES {
            if self.voices[i].key >= 0 {
                self.order[n] = i as u16;
                n += 1;
            }
        }
        self.active = n;
        for i in 0..VOICES {
            if self.voices[i].key < 0 {
                self.order[n] = i as u16;
                n += 1;
            }
        }
    }

    fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len();
        let k = self.knobs;
        let s = &mut self.smooth;
        let c = f32::from_bits(0x3d23_d70a);
        s.pw_a = (k.pw_a - s.pw_a) * c + s.pw_a;
        s.pw_b = (k.pw_b - s.pw_b) * c + s.pw_b;
        s.balance = (k.balance - s.balance) * c + s.balance;
        s.noise = (k.noise - s.noise) * c + s.noise;
        s.lfo_amount = (k.lfo_amount - s.lfo_amount) * c + s.lfo_amount;
        s.eg_amount = (k.eg_amount - s.eg_amount) * c + s.eg_amount;
        s.mw_amount = (k.mw_amount - s.mw_amount) * c + s.mw_amount;
        s.cutoff = (k.cutoff - s.cutoff) * c + s.cutoff;
        if self.resort {
            self.sort();
            self.resort = false;
        }
        let samples = n * OVER;
        let count = self.active;
        if count > 0 {
            let mut left_over = count as i32;
            let mut g = 0;
            let mut first = true;
            while left_over > 0 {
                if left_over < 2 {
                    let i = self.order[g] as usize;
                    self.update(i, samples as i32);
                    self.run_one(i, samples, first);
                    break;
                }
                let lanes = [0, 1, 2, 3].map(|j| self.order[(g + j).min(VOICES - 1)] as usize);
                for i in lanes {
                    if self.voices[i].key >= 0 {
                        self.update(i, samples as i32);
                    }
                }
                self.run_four(lanes, samples, first);
                first = false;
                g += 4;
                left_over -= 4;
            }
            // `FUN_1800328e0`: down to the host's rate.
            let [b0, a1, a2] = self.down;
            for (side, out) in [(0usize, &mut *left), (1, &mut *right)] {
                let over = &self.over[side][..samples];
                let st = &mut self.down_state[side];
                for (frame, i) in out.iter_mut().zip(over.chunks_exact(OVER)) {
                    let [mut x2, mut x1, mut y2, mut y1] = *st;
                    for &x in i {
                        let y = ((x + x2) + (x1 + x1)) * b0 - y1 * a1 - a2 * y2;
                        x2 = x1;
                        x1 = x;
                        y2 = y1;
                        y1 = y;
                    }
                    *frame = y1;
                    *st = [x2, x1, y2, y1].map(flush);
                }
            }
        } else {
            left.fill(0.0);
            right.fill(0.0);
        }
        // `FUN_180033a00`: the trance gate.
        let k = &self.knobs;
        if count == 0 {
            self.gate_pos = 0.0;
            self.gate_step = 0;
            self.gate_gain = 1.0 - k.gate_steps[0] * k.gate_mix;
        } else if f32::from_bits(0x3a83_126f) <= k.gate_mix {
            let mut speed = 1.0;
            if 0.0 < k.gate_speed {
                speed = (0.066_666_666_666_666_67 / k.gate_speed) * self.tempo;
            }
            for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                let p = speed / self.rate + self.gate_pos;
                self.gate_pos = p;
                if 1.0 < p {
                    self.gate_step = (self.gate_step + 1) & 15;
                    self.gate_pos = p - 1.0;
                }
                let to = 1.0 - k.gate_steps[self.gate_step] * k.gate_mix;
                let mut g = (to - self.gate_gain) * k.gate_smooth + self.gate_gain;
                if g < 1e-6 && g > -1e-6 {
                    g = 0.0;
                }
                self.gate_gain = g;
                *l *= g;
                *r *= g;
            }
        }
        // `FUN_180033790`: the shelf, the chorus and the delay.
        self.delay.set_tempo(self.tempo);
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            *l = self.shelf[0].run(*l);
            *r = self.shelf[1].run(*r);
        }
        self.chorus.run(left, right);
        self.delay.run(left, right);
        // `FUN_1800241b0`: the master level, moving 0.5% a sample.
        let to = self.master();
        let k5 = f32::from_bits(0x3ba3_d70a);
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            *l *= self.gains[0];
            *r *= self.gains[1];
            self.gains[0] = (to - self.gains[0]) * k5 + self.gains[0];
            self.gains[1] = (to - self.gains[1]) * k5 + self.gains[1];
        }
        // The LFO the voices start from, on at the host's rate.
        let mut rng3 = self.rng3;
        self.lfo.run(self.rate, self.tempo, n as i32, &mut rng3);
        self.rng3 = rng3;
    }
}

impl Generator for PoiZone {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        if bytes.len() != STATE_LEN {
            return Err(StateError::Length {
                expected: STATE_LEN,
                found: bytes.len(),
            });
        }
        let magic = &bytes[8..12];
        if &bytes[..4] != b"SSLF" || !matches!(magic, b"4reV" | b"3reV" | b"ED4V" | b"ED3V") {
            return Err(StateError::Version {
                found: i32::from_le_bytes(bytes[8..12].try_into().unwrap()),
            });
        }
        self.defaults();
        for n in 0..PARAMS {
            let at = 12 + 4 * n;
            let v = f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
            self.set(n, v);
        }
        self.version.copy_from_slice(magic);
        self.program.copy_from_slice(&bytes[STATE_LEN - 64..]);
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
        if level > 0.0 {
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
            if self.voices[i].key >= 0 {
                self.kill_voice(i);
            }
        }
        self.owner = [None; 128];
        self.held = [0; 128];
    }

    fn tail(&self) -> usize {
        let release = f64::from(self.knobs.amp[3]) + 0.2;
        (release * self.rate) as usize + 4096
    }

    fn fidelity(&self) -> Fidelity {
        let k = &self.knobs;
        if k.arp != 0 {
            Fidelity::Approximate("PoiZone's arpeggiator is not built")
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

    /// Events as the emulator harness takes them, `on:<key>:<frame>` and
    /// `off:<key>:<frame>`, each note's voice its key; blocks of 512 that
    /// start again at every event.
    fn render_events(synth: &mut PoiZone, events: &str, frames: usize) -> [Vec<f32>; 2] {
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

    fn render_test() -> Option<PoiZone> {
        let root = testing::corpus()?;
        let saved = testing::projects(&root.join("System/Render tests"));
        let ours: Vec<_> = saved.iter().filter(|s| s.plugin == NAME).collect();
        assert_eq!(ours.len(), 1);
        let mut synth = PoiZone::default();
        synth.set_state(&ours[0].state).unwrap();
        Some(synth)
    }

    #[test]
    fn the_voices_start_where_the_engine_draws_them() {
        let synth = PoiZone::default();
        assert_eq!(synth.rng, 425_342_704);
        assert_eq!(synth.voices[0].a, 0.933_465_24);
        assert_eq!(synth.voices[0].b, 0.603_863_66);
        assert_eq!(synth.voices[5].b, 0.470_823_26);
    }

    #[test]
    fn the_render_test_plays() {
        let Some(mut synth) = render_test() else {
            return;
        };
        assert_eq!(synth.fidelity(), Fidelity::Exact);
        let [left, right] = render_events(&mut synth, "on:60:0 off:60:6000", 12_000);
        assert!(left.iter().chain(&right).all(|v| v.is_finite()));
        let peak = left.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((0.05..2.0).contains(&peak), "{peak}");
    }

    fn fold(left: &[f32], right: &[f32]) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for (l, r) in left.iter().zip(right) {
            for bits in [l.to_bits(), r.to_bits()] {
                hash = (hash ^ u64::from(bits)).wrapping_mul(0x0100_0000_01b3);
            }
        }
        hash
    }

    /// Bit for bit what `engine_x64.dll` plays for the render test's state
    /// under the harness in `.scratch/notes/poizone-emu` (built, polyphony
    /// 0, 44.1 kHz, the state; key 60 for 4000 frames in blocks of 64, let
    /// go at 3000): every sample's bits folded together, and three of them.
    #[test]
    fn plays_the_render_test_as_the_engine_dll_does() {
        let Some(mut synth) = render_test() else {
            return;
        };
        let [left, right] = render_events(&mut synth, "on:60:0 off:60:3000", 4000);
        let at = |n: usize| [left[n].to_bits(), right[n].to_bits()];
        assert_eq!(at(500), [0x3e5c_d129, 0x3e5c_d129]);
        assert_eq!(at(2000), [0xbe09_3e6f, 0xbe09_3e6f]);
        assert_eq!(at(3500), [0xba67_ea93, 0xba67_ea93]);
        assert_eq!(fold(&left, &right), 0x4a95_8886_d7b2_4133);
    }

    /// The same as a chord of four with releases, which takes the
    /// engine's four-voice loop.
    #[test]
    fn plays_a_chord_as_the_engine_dll_does() {
        let Some(mut synth) = render_test() else {
            return;
        };
        let events = "on:60:0 on:64:100 on:67:700 off:60:2000 off:64:2500 off:67:3000 on:72:3300";
        let [left, right] = render_events(&mut synth, events, 6000);
        assert_eq!(fold(&left, &right), 0x0b0b_05fc_cc2d_4c5b);
    }

    #[test]
    fn follows_note_pitch() {
        testing::follows_note_pitch(|| Box::new(PoiZone::default()), 44_100.0);
    }

    /// Every PoiZone state FL ships loads and plays a note that stays
    /// finite.
    #[test]
    fn every_shipped_state_loads_and_plays() {
        let Some(root) = testing::corpus() else {
            return;
        };
        let mut saved = testing::every_preset(&root, NAME);
        let ours = |saved: &testing::Saved| saved.plugin.eq_ignore_ascii_case(NAME);
        saved.extend(testing::projects(&root).into_iter().filter(ours));
        assert!(saved.len() > 380, "{}", saved.len());
        for saved in saved {
            let mut synth = PoiZone::default();
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
    /// `.scratch/notes/poizone-emu` to compare with the DLL.
    #[test]
    fn dump_for_dll() {
        let Ok(path) = std::env::var("POIZONE_STATE") else {
            return;
        };
        let frames: usize = std::env::var("POIZONE_FRAMES").unwrap().parse().unwrap();
        let events = std::env::var("POIZONE_EVENTS").unwrap();
        let mut synth = PoiZone::default();
        let rate = std::env::var("POIZONE_RATE").map_or(44_100.0, |r| r.parse().unwrap());
        synth.set_sample_rate(rate);
        if let Ok(t) = std::env::var("POIZONE_TEMPO") {
            synth.set_tempo(t.parse().unwrap());
        }
        synth.set_state(&std::fs::read(path).unwrap()).unwrap();
        let [left, right] = render_events(&mut synth, &events, frames);
        let out: String = left
            .iter()
            .zip(&right)
            .map(|(l, r)| format!("{l:?} {r:?}\n"))
            .collect();
        std::fs::write(std::env::var("POIZONE_OUT").unwrap(), out).unwrap();
    }
}
