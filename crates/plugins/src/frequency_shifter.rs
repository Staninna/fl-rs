//! Frequency Shifter: a Hilbert pair and a quadrature oscillator that move
//! every partial of the input up or down by the same number of hertz, or a
//! ring modulator, as `docs/plugins/frequency-shifter.md` reads it from the
//! plugin's own `Engine_x64.dll`.
//!
//! The state is 52 bytes: a little-endian i32 version, 4 (FL reads any
//! under 5), the ten parameters as i32, a word of window flags and the
//! sidechain input's number.
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Mix | 0 to 10000 | 10000 |
//! | 1 Type: HQ shifter, shifter, ring mod | 0 to 2 | 1 |
//! | 2 Frequency | -70000 to 70000 | 0 |
//! | 3 L/R phase | 0 to 10000 | 0 |
//! | 4, 5 Left and right shape | 0 to 10000 | 10000 |
//! | 6 Feedback | 0 to 10000 | 5000 |
//! | 7 Stereo | 0 to 10000 | 10000 |
//! | 8 Range: 20 kHz, 200 Hz, tempo | 0 to 2 | 1 |
//! | 9 Start phase | 0 to 10000 | 5000 |
//!
//! The shell hands each wheel to the engine as its place between its ends,
//! from 0 to 1. The engine runs one frame at a time:
//!
//! 1. The mix, the shapes, the feedback, the stereo wheel and the two
//!    phases glide to where they were set, by `1 - 0.1^(200 / rate)` of the
//!    way each frame, and the frequency by `1 - 0.1^(10 / rate)`. The
//!    engine starts from its own defaults and the frequency from the bottom
//!    of its wheel. FL's mixer runs from the moment a project is open, so
//!    those glides have landed by the time a song plays: a state taken
//!    here lands them at once, and only a parameter moved later glides.
//! 2. `tanh` of the feedback times the last frame's wet output is added to
//!    the input, which goes through a high-pass at 20 Hz and a low-pass at
//!    20 kHz (designed for 44.1 kHz whatever the rate, as in FL).
//! 3. The ring modulator multiplies that by a sine, or by the sidechain.
//!    The shifters split it into two signals 90 degrees apart with two
//!    chains of four allpass sections, and mix them with a cosine and a
//!    sine to keep the upper sideband (shape 1), the lower (shape 0) or a
//!    blend. The HQ shifter first splits the input with 16th-order
//!    Linkwitz-Riley crossovers it designs again every 32 frames, so that
//!    what would fold over 0 Hz or over 22.05 kHz is crossfaded out, and
//!    sends the dry signal through the same crossovers to keep it in phase.
//! 4. The wet and the dry are mixed, and the stereo wheel blends each side
//!    with the other.
//!
//! In the tempo range the oscillator turns once in a whole number of 192nds
//! of a beat, from a quarter of a beat to 32 beats, and starts in step with
//! the song when it plays.
//!
//! Every sum is taken in the order the engine takes it, lane by lane. The
//! CRT's `sinf`, `sin`, `tanhf`, `powf` and `pow` are Rust's.

use crate::{Effect, Sidechain, StateError, knob_value, state_version};
use std::f64::consts::TAU;

/// The plugin's event 201 name.
pub const NAME: &str = "Frequency Shifter";

/// The range of each parameter's wheel.
const RANGES: [(i32, i32); 10] = [
    (0, 10_000),
    (0, 2),
    (-70_000, 70_000),
    (0, 10_000),
    (0, 10_000),
    (0, 10_000),
    (0, 10_000),
    (0, 10_000),
    (0, 2),
    (0, 10_000),
];
/// The wheels' defaults, which the shell hands the engine when it is made.
const DEFAULTS: [i32; 10] = [10_000, 1, 0, 0, 10_000, 10_000, 5000, 10_000, 1, 5000];

/// How long the state is, and the newest version FL reads.
const LEN: usize = 52;
const VERSION: i32 = 4;

/// What the engine sets its parameters to when it is made: 1 for the mix,
/// 0 for the type, the L/R phase and the range, and a half for the rest.
const ENGINE_DEFAULTS: [f32; 10] = [1.0, 0.0, 0.5, 0.0, 0.5, 0.5, 0.5, 0.5, 0.0, 0.5];
/// What the glides take a power of: a tenth, a bit under.
const TENTH: f64 = f64::from_bits(0x3fb9_9999_9999_9998);
/// The rate the engine is made at.
const START_RATE: f64 = 44_100.0;

/// The parameters that glide, in the order the engine moves them.
const GLIDING: [usize; 7] = [0, 4, 5, 6, 7, 3, 9];

/// Where the type and the range switch, as the engine compares a wheel's
/// place.
const THIRD: f32 = 0.3333;
const TWO_THIRDS: f32 = 0.6666;
/// How much of an octave the frequency wheel's half spans in the 20 kHz
/// and the 200 Hz range: 0.1 Hz times 2 to these is the end.
const OCTAVES: f32 = f32::from_bits(0x418c_e08b);
const OCTAVES_200: f32 = f32::from_bits(0x412f_73da);
/// Two pi and a half pi as f32.
const TURN: f32 = f32::from_bits(0x40c9_0fdb);
const QUARTER: f32 = f32::from_bits(0x3fc9_0fdb);
/// The feedback wheel's span: from -1.005 to 1.005.
const FEEDBACK: f32 = f32::from_bits(0x3f80_a3d7);
/// The crossfade between the HQ shifter's bands takes 500ths of a second.
const FADE: f32 = f32::from_bits(0x43f9_ffff);
/// The HQ shifter's crossovers: 22050 Hz less the shift, and the shift,
/// each kept between these.
const FOLD: f32 = 22_050.0;
const LOWEST: f32 = 20.0;
const HIGHEST: f32 = 20_000.0;
/// The Qs of an 8th-order Butterworth filter's sections, which each band
/// runs twice.
const QS: [f64; 4] = [
    0.509_795_606_136_322,
    0.601_344_883_441_925,
    0.899_976_193_904_876_7,
    2.562_915_325_164_795,
];
/// The two allpass chains: the first comes out a frame late.
const LATE: [u32; 4] = [0x3ef5_7409, 0x3f60_4fdb, 0x3f7a_024c, 0x3f7f_5c1c];
const ON_TIME: [u32; 4] = [0x3e25_a405, 0x3f3b_a7c8, 0x3f72_026f, 0x3f7d_97e8];

/// A filter the engine designs, after the cookbook: a low-pass or a
/// high-pass at `hz` with quality `q`, as `b0, b1, b2` and the feedback
/// terms negated.
fn design(high: bool, hz: f64, rate: f64, q: f64) -> [f32; 5] {
    let (sin, cos) = (hz * TAU / rate).sin_cos();
    let alpha = 0.5 / q * sin;
    let (b0, b1) = if high {
        ((cos + 1.0) * 0.5, -(cos + 1.0))
    } else {
        ((1.0 - cos) * 0.5, 1.0 - cos)
    };
    let k = 1.0 / (alpha + 1.0);
    let a1 = cos * -2.0;
    let a2 = 1.0 - alpha;
    [
        (k * b0) as f32,
        (k * b1) as f32,
        (k * b0) as f32,
        -((k * a1) as f32),
        -((k * a2) as f32),
    ]
}

/// One of the engine's scalar filters on the input.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Biquad {
    c: [f32; 5],
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Biquad {
    fn run(&mut self, x: f32) -> f32 {
        let [b0, b1, b2, a1, a2] = self.c;
        let y = ((a2 * self.y2 + a1 * self.y1) + b2 * self.x2) + (b0 * x + b1 * self.x1);
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// Four lanes of a section of the HQ shifter's crossovers.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Section {
    c: [[f32; 4]; 5],
    x1: [f32; 4],
    x2: [f32; 4],
    y1: [f32; 4],
    y2: [f32; 4],
}

/// One of the five crossover banks: eight sections, lanes 0 and 1 a
/// low-pass and lanes 2 and 3 a high-pass. A bank runs no section until it
/// has been designed.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Bank {
    sections: [Section; 8],
    len: usize,
}

impl Bank {
    /// Design the low-pass at `low` and the high-pass at `high`: four
    /// Butterworth sections each, then the same four again.
    fn design(&mut self, low: f32, high: f32, rate: f64) {
        for (n, q) in QS.iter().enumerate() {
            let lows = design(false, f64::from(low), rate, *q);
            let highs = design(true, f64::from(high), rate, *q);
            for (c, (l, h)) in self.sections[n].c.iter_mut().zip(lows.iter().zip(&highs)) {
                *c = [*l, *l, *h, *h];
            }
            self.sections[n + 4].c = self.sections[n].c;
        }
        self.len = 8;
    }

    fn run(&mut self, mut x: [f32; 4]) -> [f32; 4] {
        for s in &mut self.sections[..self.len] {
            let [b0, b1, b2, a1, a2] = s.c;
            let mut y = [0.0; 4];
            for n in 0..4 {
                y[n] = (((s.x1[n] * b1[n] + x[n] * b0[n]) + b2[n] * s.x2[n]) + s.y1[n] * a1[n])
                    + a2[n] * s.y2[n];
            }
            s.x2 = s.x1;
            s.x1 = x;
            s.y2 = s.y1;
            s.y1 = y;
            x = y;
        }
        x
    }

    /// A stereo pair through both halves, the two summed: an allpass with
    /// the crossover's phase.
    fn run_pair(&mut self, l: f32, r: f32) -> [f32; 2] {
        let y = self.run([l, r, l, r]);
        [y[0] + y[2], y[1] + y[3]]
    }
}

/// A second-order allpass section in four lanes: `y = c (x + y[n-2]) -
/// x[n-2]`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Allpass {
    c: [f32; 4],
    x1: [f32; 4],
    x2: [f32; 4],
    y1: [f32; 4],
    y2: [f32; 4],
}

impl Allpass {
    fn new(c: u32) -> Self {
        Allpass {
            c: [f32::from_bits(c); 4],
            ..Allpass::default()
        }
    }

    fn run(&mut self, x: [f32; 4]) -> [f32; 4] {
        let mut y = [0.0; 4];
        for n in 0..4 {
            y[n] = (x[n] + self.y2[n]) * self.c[n] - self.x2[n];
        }
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// The Hilbert pair: two chains whose outputs are 90 degrees apart over
/// most of the band, the first held back a frame.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Hilbert {
    late: [Allpass; 4],
    held: [f32; 4],
    on_time: [Allpass; 4],
}

impl Default for Hilbert {
    fn default() -> Self {
        Hilbert {
            late: LATE.map(Allpass::new),
            held: [0.0; 4],
            on_time: ON_TIME.map(Allpass::new),
        }
    }
}

impl Hilbert {
    fn run(&mut self, x: [f32; 4]) -> ([f32; 4], [f32; 4]) {
        let late = self.late.iter_mut().fold(x, |x, stage| stage.run(x));
        let out = std::mem::replace(&mut self.held, late);
        let on_time = self.on_time.iter_mut().fold(x, |x, stage| stage.run(x));
        (out, on_time)
    }

    fn clear(&mut self) {
        *self = Hilbert::default();
    }
}

/// `Engine_x64.dll`'s object, one field for each the process reads.
#[derive(Debug, Clone, PartialEq)]
struct Engine {
    rate: f64,
    /// Where each parameter was set, and where the gliding ones are now.
    target: [f32; 10],
    now: [f32; 10],
    wet: f32,
    dry: f32,
    hq: bool,
    shifter: bool,
    /// The frequency wheel, where it was set and where it is now.
    to: f32,
    freq: f32,
    lr: f32,
    shape: [f32; 2],
    unshape: [f32; 2],
    feedback: f32,
    stereo: f32,
    tempo: bool,
    narrow: bool,
    start: f32,
    /// Whether the phase starts again at the next frame, and whether the
    /// song was playing at the last.
    restart: bool,
    playing: bool,
    glide: f32,
    glide_freq: f32,
    fade_step: f32,
    per_frame: f64,
    per_frame_f32: f32,
    /// The song's place in beats at this frame and the next.
    at: f64,
    next: f64,
    step: f64,
    phase: f64,
    fade: f32,
    fade_by: f32,
    count: u32,
    last: [f32; 2],
    high_pass: [Biquad; 2],
    low_pass: [Biquad; 2],
    banks: [Bank; 5],
    hilbert: Hilbert,
}

impl Engine {
    fn new() -> Self {
        let mut engine = Engine {
            rate: START_RATE,
            target: ENGINE_DEFAULTS,
            now: ENGINE_DEFAULTS,
            wet: 0.0,
            dry: 0.0,
            hq: false,
            shifter: false,
            to: 0.0,
            freq: 0.0,
            lr: 0.0,
            shape: [0.0; 2],
            unshape: [0.0; 2],
            feedback: 0.0,
            stereo: 0.0,
            tempo: false,
            narrow: false,
            start: 0.0,
            restart: false,
            playing: false,
            glide: 0.0,
            glide_freq: 0.0,
            fade_step: 0.0,
            per_frame: 0.0,
            per_frame_f32: 0.0,
            at: 0.0,
            next: 0.0,
            step: 0.0,
            phase: 0.0,
            fade: 1.0,
            fade_by: 0.0,
            count: 0,
            last: [0.0; 2],
            high_pass: [Biquad::default(); 2],
            low_pass: [Biquad::default(); 2],
            banks: [Bank::default(); 5],
            hilbert: Hilbert::default(),
        };
        engine.set_rate(START_RATE);
        for (index, value) in ENGINE_DEFAULTS.iter().enumerate() {
            engine.derive(index, *value);
        }
        engine
    }

    fn set_rate(&mut self, rate: f64) {
        self.rate = rate;
        let high = design(true, 20.0, rate, 0.5);
        // The low-pass reads a rate the engine never sets but at birth.
        let low = design(false, 20_000.0, START_RATE, 0.5);
        for side in 0..2 {
            self.high_pass[side].c = high;
            self.low_pass[side].c = low;
        }
        self.fade_step = FADE / rate as f32;
        self.glide = (1.0 - TENTH.powf(200.0 / rate)) as f32;
        self.glide_freq = (1.0 - TENTH.powf(10.0 / rate)) as f32;
        self.per_frame = 1.0 / rate;
        self.per_frame_f32 = self.per_frame as f32;
    }

    /// What a parameter's place means, as the engine works it out.
    fn derive(&mut self, index: usize, v: f32) {
        match index {
            0 => {
                let twice = v + v;
                self.wet = if twice >= 0.0 { twice.min(1.0) } else { 0.0 };
                let less = 2.0 - twice;
                self.dry = if less >= 0.0 { less.min(1.0) } else { 0.0 };
            }
            1 => {
                // The engine asks `v < 0.6666 && v < 0.3333`.
                self.hq = v < THIRD;
                self.shifter = v < TWO_THIRDS;
            }
            2 => self.to = v,
            3 => self.lr = v * 0.25,
            4 | 5 => {
                self.shape[index - 4] = v;
                self.unshape[index - 4] = 1.0 - v;
            }
            6 => self.feedback = (v * FEEDBACK + v * FEEDBACK) - FEEDBACK,
            7 => self.stereo = (1.0 - v) * 0.5,
            8 => {
                let tempo = TWO_THIRDS <= v;
                if tempo != self.tempo {
                    self.tempo = tempo;
                    self.restart = true;
                }
                self.narrow = (THIRD..TWO_THIRDS).contains(&v);
            }
            9 => self.start = v + 0.5,
            _ => {}
        }
    }

    /// Dispatch 3: the type, the frequency and the range take at once, the
    /// others glide there.
    fn set(&mut self, index: usize, v: f32) {
        self.target[index] = v;
        if matches!(index, 1 | 2 | 8) {
            self.derive(index, v);
        }
    }

    /// Let every glide land where it is going.
    fn land(&mut self) {
        for index in GLIDING {
            self.now[index] = self.target[index];
            self.derive(index, self.target[index]);
        }
        self.freq = self.to;
    }

    fn glide(&mut self) {
        for index in GLIDING {
            let (now, to) = (self.now[index], self.target[index]);
            if now != to {
                let mut moved = (to - now) * self.glide + now;
                if (moved - to).abs() < 1e-5 {
                    moved = to;
                }
                self.now[index] = moved;
                self.derive(index, moved);
            }
        }
    }

    /// The shift in Hz for the frequency wheel's place.
    fn shift(&self, v: f32) -> f32 {
        let octaves = if self.narrow { OCTAVES_200 } else { OCTAVES };
        if v < 0.5 {
            let below = (0.5 - v) + (0.5 - v);
            if below == 0.0 {
                -0.0
            } else {
                2f32.powf(octaves * below) * -0.1
            }
        } else {
            let above = ((v - 0.5) + v) - 0.5;
            if above == 0.0 {
                0.0
            } else {
                2f32.powf(above * octaves) * 0.1
            }
        }
    }

    /// One frame. `side` is the sidechain's, when the ring modulator has
    /// one; `at` and `next` the song's place in beats at this frame and the
    /// next, or -1 when it is not playing; `bpm` the tempo.
    fn frame(
        &mut self,
        l: f32,
        r: f32,
        side: Option<[f32; 2]>,
        at: f64,
        next: f64,
        bpm: f64,
    ) -> [f32; 2] {
        let playing = at >= 0.0;
        if !self.playing && playing {
            self.restart = true;
            self.at = at;
            self.playing = true;
            self.next = next;
        } else {
            self.playing = playing;
            if playing {
                self.at = at;
                self.next = next;
            } else {
                let beats = bpm * (1.0 / 60.0) * self.per_frame;
                self.at += beats;
                self.next = self.at + beats;
            }
        }
        self.glide();
        let off = self.to - self.freq;
        self.freq = if off.abs() > 1e-4 {
            off * self.glide_freq + self.freq
        } else {
            self.to
        };
        let dry_in = [l, r];
        let mut input = [0.0; 2];
        for (side_n, x) in [l, r].into_iter().enumerate() {
            let x = (self.feedback * self.last[side_n]).tanh() + x;
            let x = self.high_pass[side_n].run(x);
            input[side_n] = self.low_pass[side_n].run(x);
        }
        let hz = if self.tempo {
            let v = self.freq;
            let period = if !self.shifter {
                2f64.powf(f64::from((1.0 - v) * 7.0)) * 48.0
            } else if v >= 0.5 {
                2f64.powf(f64::from(7.0 - (v - 0.5) * 14.0)) * 48.0
            } else {
                2f64.powf(f64::from(7.0 - (0.5 - v) * 14.0)) * -48.0
            };
            let period = f64::from(period as i32) * (1.0 / 192.0);
            if self.restart {
                self.restart = false;
                if period > 0.0 {
                    let x = f64::from((self.at / period) as f32 + self.start);
                    self.phase = x - f64::from(x as i32);
                } else {
                    let t = f64::from(((-1.0 / period) * self.at) as f32);
                    let x = (1.0 - (t - f64::from(t as i32))) + f64::from(self.start);
                    self.phase = x - f64::from(x as i32);
                }
            }
            self.step = f64::from(((self.next - self.at) / period) as f32);
            if self.step == 0.0 {
                0.0
            } else {
                (1.0 / (self.step * self.rate)) as f32
            }
        } else {
            if self.restart {
                self.restart = false;
                self.phase = f64::from(self.start);
            }
            let hz = if self.shifter {
                self.shift(self.freq)
            } else if self.freq == 0.0 {
                0.0
            } else {
                let octaves = if self.narrow { OCTAVES_200 } else { OCTAVES };
                2f32.powf(self.freq * octaves) * 0.1
            };
            self.step = f64::from(hz * self.per_frame_f32);
            hz
        };
        let (wet, [dry_l, dry_r]) = if self.shifter {
            self.shifted(input, dry_in, hz)
        } else {
            let wet = match side {
                Some([sl, sr]) => [input[0] * sl, input[1] * sr],
                None => {
                    let p = self.phase as f32;
                    let left = f64::from(p * TURN).sin() * f64::from(input[0]);
                    let mut q = p + self.lr;
                    q -= (q as i32) as f32;
                    let right = f64::from(q * TURN).sin() * f64::from(input[1]);
                    let phase = self.step + self.phase;
                    self.phase = phase - f64::from(phase as i32);
                    [left as f32, right as f32]
                }
            };
            (wet, dry_in)
        };
        self.last = wet;
        let left = wet[0] * self.wet + self.dry * dry_l;
        let right = wet[1] * self.wet + self.dry * dry_r;
        let other = 1.0 - self.stereo;
        [
            self.stereo * right + other * left,
            other * right + left * self.stereo,
        ]
    }

    /// The shifters' wet signal, and the dry one the HQ shifter puts
    /// through its crossovers.
    fn shifted(&mut self, input: [f32; 2], dry: [f32; 2], hz: f32) -> ([f32; 2], [f32; 2]) {
        let [l, r] = input;
        let (pair, dry) = if self.hq {
            if self.count == 0 {
                self.fade_by = if hz >= 0.0 {
                    self.fade_step
                } else {
                    -self.fade_step
                };
                let shift = hz.abs();
                let fold = FOLD - shift;
                let fold = if fold >= LOWEST {
                    fold.min(HIGHEST)
                } else {
                    LOWEST
                };
                let shift = if shift >= LOWEST {
                    shift.min(HIGHEST)
                } else {
                    LOWEST
                };
                let rate = self.rate;
                self.banks[0].design(fold, shift, rate);
                self.banks[1].design(shift, shift, rate);
                self.banks[2].design(fold, fold, rate);
                self.banks[3].design(fold, fold, rate);
                self.banks[4].design(shift, shift, rate);
            }
            self.count = (self.count + 1) % 32;
            let split = self.banks[0].run([l, r, l, r]);
            let [p0, p1] = self.banks[1].run_pair(split[0], split[1]);
            let [q0, q1] = self.banks[2].run_pair(split[2], split[3]);
            let [d0, d1] = self.banks[3].run_pair(dry[0], dry[1]);
            let dry = self.banks[4].run_pair(d0, d1);
            let (on, off) = (self.fade, 1.0 - self.fade);
            let pair = [
                p0 * on + q0 * off,
                p1 * on + q1 * off,
                q0 * on + p0 * off,
                q1 * on + p1 * off,
            ];
            let fade = self.fade + self.fade_by;
            self.fade = if fade >= 0.0 { fade.min(1.0) } else { 0.0 };
            (pair, dry)
        } else {
            ([l, r, l, r], dry)
        };
        let (a, b) = self.hilbert.run(pair);
        let p = self.phase as f32;
        let left = p * TURN;
        let mut q = p + self.lr;
        q -= (q as i32) as f32;
        let right = q * TURN;
        let sine = |x: f32| f64::from(x).sin() as f32;
        let s = [
            sine(left),
            sine(left + QUARTER),
            sine(right),
            sine(right + QUARTER),
        ];
        let phase = (self.step + 1000.0) + self.phase;
        self.phase = phase - f64::from(phase as i32);
        let wet_l = (b[2] * s[1] + a[2] * s[0]) * self.unshape[0]
            + (b[0] * s[1] - a[0] * s[0]) * self.shape[0];
        let wet_r = (b[3] * s[3] + a[3] * s[2]) * self.unshape[1]
            + (b[1] * s[3] - a[1] * s[2]) * self.shape[1];
        ([wet_l, wet_r], dry)
    }

    /// Clear what the frames so far left behind.
    fn clear(&mut self) {
        for filter in self.high_pass.iter_mut().chain(&mut self.low_pass) {
            *filter = Biquad {
                c: filter.c,
                ..Biquad::default()
            };
        }
        for bank in &mut self.banks {
            for s in &mut bank.sections {
                *s = Section {
                    c: s.c,
                    ..Section::default()
                };
            }
        }
        self.hilbert.clear();
        self.last = [0.0; 2];
        self.playing = false;
    }
}

/// Frequency Shifter: the shell's state and the song's place around the
/// engine.
#[derive(Debug, Clone, PartialEq)]
pub struct FrequencyShifter {
    params: [i32; 10],
    /// The sidechain input the state chose, 0 for none.
    sidechain: usize,
    side: Sidechain,
    engine: Box<Engine>,
    hz: u32,
    bpm: f64,
    ppq: u16,
    /// The song's place in frames at the start of the next block, while it
    /// plays, and the tick the last [`Effect::tick`] was at.
    song: Option<u64>,
    tick: Option<u64>,
}

impl Default for FrequencyShifter {
    fn default() -> Self {
        let mut plugin = FrequencyShifter {
            params: DEFAULTS,
            sidechain: 0,
            side: Sidechain::default(),
            engine: Box::new(Engine::new()),
            hz: 44_100,
            bpm: 120.0,
            ppq: 96,
            song: None,
            tick: None,
        };
        for index in 0..DEFAULTS.len() {
            plugin.send(index);
        }
        plugin
    }
}

impl FrequencyShifter {
    /// Hand a parameter to the engine as its wheel's place.
    fn send(&mut self, index: usize) {
        let (min, max) = RANGES[index];
        let place = f64::from(self.params[index] - min) * (1.0 / f64::from(max - min));
        self.engine.set(index, place.clamp(0.0, 1.0) as f32);
    }

    /// Frames to a tick, as the shell counts them for four steps a beat.
    fn tick_frames(&self) -> f32 {
        let ticks_a_step = i32::from(self.ppq) / 4;
        (f64::from(self.hz as i32 * 15) / (self.bpm * f64::from(ticks_a_step))) as f32
    }

    fn ring_mod(&self) -> bool {
        self.params[1] == 2
    }
}

impl Effect for FrequencyShifter {
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
        let words = bytes.as_chunks::<4>().0;
        for (param, word) in self.params.iter_mut().zip(&words[1..11]) {
            *param = i32::from_le_bytes(*word);
        }
        self.sidechain = usize::try_from(i32::from_le_bytes(words[12])).unwrap_or(0);
        for index in 0..self.params.len() {
            self.send(index);
        }
        self.engine.land();
        Ok(())
    }

    /// FL counts the rate in whole Hz. A rate that is not a number of 1 Hz
    /// or more is not taken. The filters keep what they hold.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz >= 1.0 {
            self.hz = hz.round() as u32;
            self.engine.set_rate(f64::from(self.hz));
        }
    }

    fn set_tempo(&mut self, bpm: f64) {
        if bpm > 0.0 && bpm.is_finite() {
            self.bpm = bpm;
        }
    }

    fn set_ppq(&mut self, ppq: u16) {
        if ppq >= 4 {
            self.ppq = ppq;
        }
    }

    /// FL hands the engine the song's place at every frame while it plays,
    /// from its clock in frames. This counts the frames from the tick where
    /// the song started or moved to.
    fn tick(&mut self, ppq_tick: u64, tempo: f64) {
        let follows = self
            .tick
            .is_some_and(|tick| ppq_tick == tick || ppq_tick == tick + 1);
        self.tick = Some(ppq_tick);
        if !follows || self.song.is_none() {
            let tempo = if tempo > 0.0 { tempo } else { self.bpm };
            let ms = ppq_tick as f64 * 60_000.0 / (tempo * f64::from(self.ppq));
            let frames = ms * f64::from(self.hz) * f64::from(0.001f32);
            self.song = Some(frames.round().max(0.0) as u64);
        }
    }

    fn follows_song(&self) -> bool {
        true
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let tick_frames = self.tick_frames();
        let ppq = f32::from(self.ppq);
        let ring_side = self.ring_mod() && self.sidechain > 0;
        for (n, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
            let (at, next) = match self.song {
                Some(song) => {
                    let place = |frame: u64| f64::from(frame as i32 as f32 / tick_frames / ppq);
                    let frame = song + n as u64;
                    (place(frame), place(frame + 1))
                }
                None => (-1.0, -1.0),
            };
            let side = if ring_side { self.side.frame(n) } else { None };
            [*l, *r] = self.engine.frame(*l, *r, side, at, next, self.bpm);
        }
        if let Some(song) = &mut self.song {
            *song += left.len().min(right.len()) as u64;
        }
        self.side.clear();
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

    /// The song stops: the filters empty, and the phase starts in step
    /// with the song again when it plays.
    fn reset(&mut self) {
        self.engine.clear();
        self.song = None;
        self.tick = None;
        self.side.clear();
    }

    fn sidechain_input(&self) -> usize {
        self.sidechain
    }

    fn set_sidechain(&mut self, index: usize, left: &[f32], right: &[f32]) {
        if index > 0 && index == self.sidechain {
            self.side.keep(left, right);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{partial, sine, state};

    /// A state of version 4 with these parameters.
    fn bytes(params: [i32; 10], sidechain: i32) -> Vec<u8> {
        let mut words = vec![4];
        words.extend(params);
        words.extend([0, sidechain]);
        state(&words)
    }

    /// A plugin at 48 kHz with these parameters, run for two seconds so
    /// that every glide has settled.
    fn settled(params: [i32; 10]) -> FrequencyShifter {
        let mut plugin = FrequencyShifter::default();
        plugin.set_sample_rate(48_000.0);
        plugin.set_state(&bytes(params, 0)).unwrap();
        let (mut l, mut r) = (vec![0.0; 96_000], vec![0.0; 96_000]);
        plugin.process(&mut l, &mut r);
        plugin
    }

    /// The left output for a sine that turns `turns` times a frame.
    fn run(plugin: &mut FrequencyShifter, turns: f64, len: usize) -> Vec<f32> {
        let mut l = sine(turns, len);
        let mut r = l.clone();
        plugin.process(&mut l, &mut r);
        l
    }

    #[test]
    fn shifts_a_tone_up_by_the_wheel() {
        // 200 Hz range, the wheel at its top: 200 Hz. 1 kHz in, 1.2 kHz out.
        let mut params = DEFAULTS;
        params[2] = 70_000;
        for kind in [0, 1] {
            params[1] = kind;
            let mut plugin = settled(params);
            let out = run(&mut plugin, 1000.0 / 48_000.0, 48_000);
            let tail = &out[24_000..];
            let up = partial(tail, 1200.0 / 48_000.0);
            let down = partial(tail, 800.0 / 48_000.0);
            let stays = partial(tail, 1000.0 / 48_000.0);
            assert!(up > 0.9 && up < 1.1, "{kind}: {up}");
            assert!(down < 0.01 && stays < 0.01, "{kind}: {down} {stays}");
        }
    }

    /// The wheel's place for a shift of `hz` in the 20 kHz range.
    fn wheel(hz: f64) -> i32 {
        let place = (hz.abs() / 0.1).log2() / f64::from(OCTAVES);
        (place * 70_000.0).round().copysign(hz) as i32
    }

    #[test]
    fn the_wheel_s_ends_are_20_khz_and_200_hz() {
        let engine = Engine::new();
        assert_eq!(engine.shift(0.5), 0.0);
        assert!((engine.shift(1.0) - 20_000.0).abs() < 0.1);
        assert!((engine.shift(0.0) + 20_000.0).abs() < 0.1);
        let mut narrow = Engine::new();
        narrow.set(8, 0.5);
        assert!((narrow.shift(1.0) - 200.0).abs() < 0.01);
        assert!((narrow.shift(0.75) - 200f32.sqrt() * 0.1f32.sqrt()).abs() < 0.01);
    }

    #[test]
    fn a_shape_of_0_keeps_the_lower_sideband_and_a_negative_shift_goes_down() {
        let mut params = DEFAULTS;
        params[1] = 1;
        params[8] = 0;
        params[2] = wheel(300.0);
        params[4] = 0;
        let mut plugin = settled(params);
        let tail = run(&mut plugin, 1000.0 / 48_000.0, 48_000).split_off(24_000);
        assert!(partial(&tail, 700.0 / 48_000.0) > 0.95);
        assert!(partial(&tail, 1300.0 / 48_000.0) < 0.01);
        params[2] = wheel(-300.0);
        params[4] = 10_000;
        let mut plugin = settled(params);
        let tail = run(&mut plugin, 1000.0 / 48_000.0, 48_000).split_off(24_000);
        assert!(partial(&tail, 700.0 / 48_000.0) > 0.95);
        assert!(partial(&tail, 1300.0 / 48_000.0) < 0.01);
    }

    #[test]
    fn the_hq_shifter_drops_what_would_fold_over_0_hz() {
        // 100 Hz moved down 300 Hz comes out at 200 Hz from the plain
        // shifter, and not from the HQ one.
        let mut params = DEFAULTS;
        params[8] = 0;
        params[2] = wheel(-300.0);
        let mut folded = |kind| {
            params[1] = kind;
            let mut plugin = settled(params);
            let tail = run(&mut plugin, 100.0 / 48_000.0, 48_000).split_off(24_000);
            partial(&tail, 200.0 / 48_000.0)
        };
        assert!(folded(1) > 0.9);
        assert!(folded(0) < 1e-3);
    }

    #[test]
    fn the_ring_modulator_makes_both_sidebands() {
        let mut params = DEFAULTS;
        params[1] = 2;
        params[8] = 1;
        // 2^(0.5 * 10.965784) / 10: about 4.46 Hz... take the top, 200 Hz.
        params[2] = 70_000;
        let mut plugin = settled(params);
        let tail = run(&mut plugin, 1000.0 / 48_000.0, 48_000).split_off(24_000);
        for hz in [800.0, 1200.0] {
            let side = partial(&tail, hz / 48_000.0);
            assert!((side - 0.5).abs() < 0.01, "{hz}: {side}");
        }
        assert!(partial(&tail, 1000.0 / 48_000.0) < 0.01);
    }

    #[test]
    fn the_ring_modulator_multiplies_by_its_sidechain() {
        let mut plugin = FrequencyShifter::default();
        plugin.set_sample_rate(48_000.0);
        let mut params = DEFAULTS;
        params[1] = 2;
        plugin.set_state(&bytes(params, 3)).unwrap();
        assert_eq!(plugin.sidechain_input(), 3);
        let mut quiet = FrequencyShifter::default();
        quiet.set_sample_rate(48_000.0);
        quiet.set_state(&bytes(params, 3)).unwrap();
        let input = sine(1000.0 / 48_000.0, 4800);
        let side = vec![0.5; 4800];
        let (mut l, mut r) = (input.clone(), input.clone());
        let (mut ql, mut qr) = (input.clone(), input);
        for at in (0..4800).step_by(480) {
            plugin.set_sidechain(3, &side[at..at + 480], &side[at..at + 480]);
            plugin.process(&mut l[at..at + 480], &mut r[at..at + 480]);
            quiet.set_sidechain(3, &[0.0; 480], &[0.0; 480]);
            quiet.process(&mut ql[at..at + 480], &mut qr[at..at + 480]);
        }
        // The wet half is the filtered input at half its size: the
        // difference from a sidechain of silence.
        let wet: Vec<f32> = l.iter().zip(&ql).map(|(a, b)| a - b).collect();
        let tail = &wet[2400..];
        let level = partial(tail, 1000.0 / 48_000.0);
        assert!((level - 0.5).abs() < 0.01, "{level}");
    }

    #[test]
    fn a_mix_of_0_passes_the_input_once_it_has_glided_there() {
        let mut params = DEFAULTS;
        params[0] = 0;
        params[2] = 30_000;
        params[1] = 1;
        let mut plugin = settled(params);
        let input = sine(0.0123, 4800);
        assert_eq!(run(&mut plugin, 0.0123, 4800), input);
        // The HQ shifter puts the dry signal through its crossovers, which
        // only turn its phase.
        params[1] = 0;
        let mut plugin = settled(params);
        for turns in [50.0 / 48_000.0, 0.01, 0.2] {
            let out = run(&mut plugin, turns, 48_000);
            let level = partial(&out[24_000..], turns);
            assert!((level - 1.0).abs() < 0.003, "{turns}: {level}");
        }
    }

    #[test]
    fn a_stereo_wheel_at_0_makes_both_sides_alike() {
        let mut params = DEFAULTS;
        params[7] = 0;
        params[2] = 20_000;
        params[3] = 4000;
        let mut plugin = settled(params);
        let mut l = sine(0.01, 4800);
        let mut r = sine(0.017, 4800);
        plugin.process(&mut l, &mut r);
        assert_eq!(l, r);
    }

    #[test]
    fn the_filters_are_the_cookbook_s() {
        let rate = 48_000.0;
        let low = design(false, 1000.0, rate, std::f64::consts::FRAC_1_SQRT_2);
        let num = [low[0], low[1], low[2]].map(f64::from);
        let den = [1.0, -f64::from(low[3]), -f64::from(low[4])];
        let at = |hz: f64| crate::testing::response(&num, &den, hz / rate);
        assert!((at(1000.0) - 0.5f64.sqrt()).abs() < 1e-4);
        assert!((at(10.0) - 1.0).abs() < 1e-4);
        // A bank's low-pass and high-pass at one place sum to an allpass.
        let mut bank = Bank::default();
        bank.design(1000.0, 1000.0, rate);
        for turns in [0.001, 1000.0 / rate, 0.05, 0.3] {
            let input = sine(turns, 48_000);
            let out: Vec<f32> = input.iter().map(|x| bank.run_pair(*x, *x)[0]).collect();
            let level = partial(&out[24_000..], turns);
            assert!((level - 1.0).abs() < 1e-3, "{turns}: {level}");
        }
    }

    #[test]
    fn the_hilbert_pair_is_a_quarter_turn_apart() {
        let mut hilbert = Hilbert::default();
        for turns in [100.0 / 48_000.0, 0.02, 0.3] {
            let input = sine(turns, 48_000);
            let mut pairs = input.iter().map(|x| hilbert.run([*x; 4]));
            let pairs: Vec<(f32, f32)> = pairs.by_ref().map(|(a, b)| (a[0], b[0])).collect();
            for (a, b) in &pairs[24_000..] {
                let size = f64::from(*a).hypot(f64::from(*b));
                assert!((size - 1.0).abs() < 0.01, "{turns}: {size}");
            }
        }
    }

    #[test]
    fn in_the_tempo_range_the_oscillator_keeps_time_with_the_song() {
        // The wheel at its top: a turn every quarter of a beat, 8 Hz at
        // 120 BPM, and the start phase at its middle starts it at 0.
        let mut params = DEFAULTS;
        params[1] = 2;
        params[2] = 70_000;
        params[8] = 2;
        let mut plugin = FrequencyShifter::default();
        plugin.set_sample_rate(48_000.0);
        plugin.set_tempo(120.0);
        plugin.set_ppq(96);
        plugin.set_state(&bytes(params, 0)).unwrap();
        let input = sine(1000.0 / 48_000.0, 96_000);
        let (mut l, mut r) = (input.clone(), input);
        // 250 frames a tick.
        for (tick, (l, r)) in l.chunks_mut(250).zip(r.chunks_mut(250)).enumerate() {
            plugin.tick(tick as u64, 120.0);
            plugin.process(l, r);
        }
        let tail = &l[48_000..];
        for hz in [992.0, 1008.0] {
            let side = partial(tail, hz / 48_000.0);
            assert!((side - 0.5).abs() < 0.01, "{hz}: {side}");
        }
    }

    #[test]
    fn full_feedback_stays_bounded() {
        let mut params = DEFAULTS;
        params[6] = 10_000;
        params[2] = 25_000;
        let mut plugin = settled(params);
        let out = run(&mut plugin, 0.01, 48_000);
        assert!(out.iter().all(|v| v.is_finite() && v.abs() < 8.0));
    }

    #[test]
    fn a_state_is_52_bytes_of_version_4_or_under() {
        let mut plugin = FrequencyShifter::default();
        let mut state = bytes(DEFAULTS, 0);
        state[0] = 5;
        assert_eq!(
            plugin.set_state(&state),
            Err(StateError::Version { found: 5 })
        );
        state[0] = 4;
        assert_eq!(
            plugin.set_state(&state[..48]),
            Err(StateError::Length {
                expected: 52,
                found: 48
            })
        );
        state[12..16].copy_from_slice(&(-123i32).to_le_bytes());
        plugin.set_state(&state).unwrap();
        assert_eq!(plugin.param(2), -123.0);
        assert_eq!(plugin.param_count(), 10);
    }

    #[test]
    fn every_state_in_fl_s_projects_plays() {
        let Some(root) = crate::testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, the states in FL's projects skipped");
            return;
        };
        let mut played = 0;
        for saved in crate::testing::projects(&root) {
            if !saved.plugin.eq_ignore_ascii_case(NAME) {
                continue;
            }
            let mut plugin = FrequencyShifter::default();
            plugin.set_sample_rate(44_100.0);
            plugin.set_state(&saved.state).unwrap();
            let mut l = sine(0.01, 44_100);
            let mut r = sine(0.013, 44_100);
            for (tick, (l, r)) in l.chunks_mut(300).zip(r.chunks_mut(300)).enumerate() {
                plugin.tick(tick as u64, 140.0);
                plugin.process(l, r);
            }
            let sound = l.iter().chain(&r);
            assert!(
                sound.clone().all(|v| v.is_finite() && v.abs() < 4.0),
                "{}",
                saved.file
            );
            assert!(
                sound.map(|v| v.abs()).fold(0.0, f32::max) > 0.1,
                "{}",
                saved.file
            );
            played += 1;
        }
        assert_eq!(played, 5);
    }
}
