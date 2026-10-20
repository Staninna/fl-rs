//! 3x Osc: three oscillators that read FL's wavetables, and the instrument
//! tab's envelopes, LFOs and filter after them, as `docs/plugins/3x-osc.md`,
//! `docs/plugins/host-voice.md` and the listings behind them have it.
//!
//! # The state
//!
//! Little-endian i32 after a version, in one of six lengths:
//!
//! | version | bytes | what it holds |
//! |---|---|---|
//! | none | 44 | shape, coarse and fine of each oscillator and the two mix levels |
//! | 10 | 84 | the first 20 parameters |
//! | 11 | 88 | and the three invert flags and the AM flag, a byte each |
//! | 12 | 92 | and parameter 20 before the flags |
//! | 13 | 93 | and the HQ flag |
//! | 14 | 460 | and the instrument tab: five envelope and LFO blocks and the filter |
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0, 7, 14 Panning | -64 to 64 | 0 |
//! | 1, 8, 15 Shape | 0 to 6 | 0 |
//! | 2, 9, 16 Coarse, semitones | -24 to 24 | 0, -12, -24 |
//! | 3, 10, 17 Fine, cents | -100 to 100 | 0 |
//! | 4, 11, 18 Stereo phase offset | -64 to 64 | 0 |
//! | 5, 12, 19 Stereo detune, cents | -50 to 50 | 0 |
//! | 6, 13 Mix level of oscillator 2 and 3 | 0 to 128 | 64, 32 |
//! | 20 Stereo phase randomness | 0 to 64 | 0 |
//! | 23 to 107 The instrument tab's five blocks, 17 words each | by word | |
//! | 110, 111 The tab's cutoff and resonance | 0 to 256 | 256, 0 |
//! | 113 The tab's filter type | 0 to 7 | 0 |
//!
//! 21, 22, 108, 109 and 112 are nobody's.
//!
//! FL does not hold a state to the ranges, and its own presets go past
//! them. Only a shape or a filter type that is none is turned down.
//!
//! # The oscillators
//!
//! The shapes are FL's host tables of 16384 points, made as FLEngine makes
//! them: a sine, a triangle, a square, a saw, a rounded saw, noise and, for
//! shape 6, the channel's sample, in whose place FL reads the saw when the
//! channel has none.
//!
//! A phase is 32 bits and moves by
//! `round(2^32 * 523.25 / rate * 2^(cents / 1200))` a sample, where the
//! cents are the note's, 100 a key from key 60, and the oscillator's coarse
//! and fine. Every sample is `table[phase >> 18] * gain` with nothing
//! between two points. The right side has its own phases, which start
//! `offset / 128` of a turn ahead and up to `random / 128` of a turn either
//! way, and its own pitch, the stereo detune higher. A noise oscillator
//! moves by a step that is drawn again every tick.
//!
//! The levels: `m3 = mix3 / 128` is oscillator 3's, `(1 - m3) * mix2 / 128`
//! oscillator 2's and the rest of `1 - m3` oscillator 1's. With AM on,
//! oscillator 3 is not added: the sum of the other two, which then share 1,
//! is multiplied by `1 - sample`. Each oscillator's two gains are its level
//! through the circular pan law.
//!
//! # The instrument tab
//!
//! The envelopes and LFOs step once a tick of the song, and their times
//! are whole ticks: `(1001^(v / 65536) - 1) * 0.128 * PPQ`, times
//! `tempo / 120` unless the block's flag says the times follow the tempo.
//! That is `(1001^(v / 65536) - 1) * 0.064` seconds at any PPQ, to the
//! nearest tick, and 12.288 times the curve at 96. What a tick's step
//! comes to holds until the next tick: the gains and the Fast LP's
//! coefficient get there in a straight line of no more than
//! `0.001 * 44100 / rate` a sample, the rest at once.
//!
//! The volume envelope scales the note, the mod X and mod Y blocks move the
//! filter's cutoff and resonance, the pitch block moves the pitch by its
//! amounts in cents, and the pan block has an LFO alone. The filter is one
//! of eight: Fast LP, the RBJ low-pass, band-pass, high-pass and notch, the
//! low-pass twice, and a state-variable low-pass once and twice. The note
//! goes through the circular pan law once more with its velocity.
//!
//! A note off starts the releases. A note whose volume envelope is off, as
//! a new plugin's is, fades in 10 ms.
//!
//! # HQ
//!
//! A state with the HQ flag reads tables made for the pitch instead: each
//! shape cut down to the harmonics that fit, a table every quarter octave,
//! read between two points. The doc has not confirmed how FL's engine
//! makes them, so [`Generator::fidelity`] says so.
//!
//! What is not FL's:
//!
//! - The custom shape. No host hands a generator its channel's sample, so
//!   the saw plays, and fidelity says so.
//! - FL steps the envelopes on the song's ticks. Here the ticks run from
//!   when the plugin was made or last reset. They are as long as the
//!   song's, by [`Generator::set_ppq`], and 96 to a beat until it is told.
//! - A note past [`VOICES`] takes the place of the oldest. A slide note
//!   sounds like any other: FL's host sends none, and moves the pitch of
//!   what sounds instead, which a voice takes on its next tick
//!   ([`Generator::set_note_pitch`]), as FL's reads `FinalLevels.Pitch`.
//! - FL's box filter for its offline render mode is not built.
//! - FL draws its random numbers from a seed it shares with everything
//!   else. Here each plugin starts from 0.
//!
//! Where this differs from the doc, the listings have it this way: the
//! envelope times are ticks and not samples, since the number the doc took
//! for the sample rate is a quarter of the PPQ; the AM depth and every
//! oscillator's gain carry the pan law's 0.7071 at the centre; the rounded
//! saw is `1 - 2 * 0.997^(16384 x)`; and the host's tables are read from
//! FLEngine's generator at `0x9cdfc0`.

use crate::fruit_kick::Articulation;
use crate::{Fidelity, Generator, NoteOn, StateError, knob_value, state_version};
use std::f64::consts::{FRAC_PI_4, PI, TAU};
use std::sync::OnceLock;

/// The plugin's event 201 name.
pub const NAME: &str = "3x Osc";

/// The notes it sounds at once. One more takes the place of the oldest.
pub const VOICES: usize = 64;

/// The points of a host table.
const TABLE: usize = 16_384;

/// `2^32 * 523.2511`: the increment at 0 cents, times the rate.
const BASE: f64 = 2_247_346_493_527.166;

/// The ticks to a beat the envelopes count in until the host says.
const PPQ: f64 = 96.0;

/// The range of each parameter's wheel.
const RANGES: [(i32, i32); 21] = [
    (-64, 64),
    (0, 6),
    (-24, 24),
    (-100, 100),
    (-64, 64),
    (-50, 50),
    (0, 128),
    (-64, 64),
    (0, 6),
    (-24, 24),
    (-100, 100),
    (-64, 64),
    (-50, 50),
    (0, 128),
    (-64, 64),
    (0, 6),
    (-24, 24),
    (-100, 100),
    (-64, 64),
    (-50, 50),
    (0, 64),
];
const DEFAULTS: [i32; 21] = [
    0, 0, 0, 0, 0, 0, 64, 0, 0, -12, 0, 0, 0, 32, 0, 0, -24, 0, 0, 0, 0,
];

/// Where a block of the instrument tab has each of its 17 numbers.
const FLAGS: usize = 0;
const ENABLED: usize = 1;
const PREDELAY: usize = 2;
const ATTACK: usize = 3;
const HOLD: usize = 4;
const DECAY: usize = 5;
const SUSTAIN: usize = 6;
const RELEASE: usize = 7;
const AMOUNT: usize = 8;
const LFO_PREDELAY: usize = 9;
const LFO_ATTACK: usize = 10;
const LFO_AMOUNT: usize = 11;
const LFO_SPEED: usize = 12;
const LFO_SHAPE: usize = 13;
const TENSIONS: usize = 14;

/// The blocks of the tab, in the order the state has them.
const VOLUME: usize = 1;
const MOD_X: usize = 2;
const MOD_Y: usize = 3;
const PITCH: usize = 4;

/// The parameter the instrument tab's start at. FL's tab handler
/// (`0x7378a0`) takes 23 to 113: `0x73ad35` hands 23 to `0x72c440`, which
/// lays the five blocks out 17 words apiece from there and puts the
/// cutoff, the resonance and the filter type 2, 3 and 5 past them. 21,
/// 22, 108, 109 and 112 are nobody's: FL keeps nothing for them.
const TAB_PARAM: usize = 23;
const TAB_BLOCKS_END: usize = TAB_PARAM + 5 * 17;
const TAB_CUTOFF: usize = TAB_BLOCKS_END + 2;
const TAB_RESONANCE: usize = TAB_BLOCKS_END + 3;
const TAB_FILTER: usize = TAB_BLOCKS_END + 5;
/// FL's NumParams.
const PARAMS: usize = TAB_BLOCKS_END + 6;

/// How far each block's envelope and LFO amounts go either way: FL
/// rescales words 8 and 11 by the table at `0x790a60` (`0x737820`) and not
/// by their wheels' -128 to 128.
const TAB_AMOUNTS: [i32; 5] = [64, 128, 128, 128, 2400];

/// The range of word `word` of block `block` of the instrument tab, from
/// the wheels of the tab's form (Tags 0x1000 + word) that `0x7364a0` lists.
/// Word 0, the flags, has no control, so FL stores what it is given.
/// Word 1 is the "use envelope" box.
fn tab_range(block: usize, word: usize) -> (i32, i32) {
    match word {
        FLAGS => (0, 1 << 30),
        ENABLED => (0, 1),
        SUSTAIN => (0, 128),
        AMOUNT | LFO_AMOUNT => (-TAB_AMOUNTS[block], TAB_AMOUNTS[block]),
        LFO_SPEED => (200, 65_536),
        LFO_SHAPE => (0, 2),
        TENSIONS.. => (-128, 128),
        _ => (100, 65_536),
    }
}

/// The range of a parameter, and `None` for one FL keeps nothing for.
fn param_range(index: usize) -> Option<(i32, i32)> {
    match index {
        0..21 => Some(RANGES[index]),
        TAB_PARAM..TAB_BLOCKS_END => {
            let at = index - TAB_PARAM;
            Some(tab_range(at / 17, at % 17))
        }
        TAB_CUTOFF | TAB_RESONANCE => Some((0, 256)),
        TAB_FILTER => Some((0, 7)),
        _ => None,
    }
}

/// A block's flags: its times are ticks whatever the tempo is, the same
/// for its LFO, a block that is off holds 1 and not 0, and the LFO's phase
/// is the plugin's and not the note's.
const FIXED_TICKS: i32 = 1;
const LFO_FIXED_TICKS: i32 = 2;
const FULL_WHEN_OFF: i32 = 4;
const GLOBAL_LFO: i32 = 0x20;

/// The stages of an envelope, as FL numbers them.
const WAIT: u8 = 0;
const RISE: u8 = 1;
const HELD: u8 = 2;
const FALL: u8 = 3;
const SUSTAINED: u8 = 4;
const RELEASED: u8 = 6;
const OVER: u8 = 9;

/// An envelope under this is over.
const TINY: f32 = 5.960_464_5e-8;

/// The tab of a new plugin, and of every state older than version 14.
fn new_tab() -> Articulation {
    let block = [
        0, 0, 100, 20_000, 20_000, 30_000, 50, 20_000, 0, 100, 20_000, 0, 32_950, 0, 0, 0, 0,
    ];
    let mut blocks = [block; 5];
    blocks[VOLUME][FLAGS] = FULL_WHEN_OFF;
    blocks[VOLUME][TENSIONS + 2] = -101;
    Articulation {
        blocks,
        cutoff: 256,
        resonance: 0,
        filter: 0,
    }
}

/// The six tables FL's host makes when it starts: sine, triangle, square,
/// saw, rounded saw and noise.
#[derive(Debug, PartialEq)]
struct HostTables {
    shapes: [Vec<f32>; 6],
}

/// The Mersenne Twister FL fills its noise table from.
struct Twister {
    state: [u32; 624],
    at: usize,
}

impl Twister {
    fn new(seed: u32) -> Self {
        let mut state = [seed; 624];
        for k in 1..624 {
            let last = state[k - 1];
            state[k] = (last ^ (last >> 30))
                .wrapping_mul(0x6c07_8965)
                .wrapping_add(k as u32);
        }
        Twister { state, at: 624 }
    }

    fn next(&mut self) -> u32 {
        if self.at == 624 {
            for k in 0..624 {
                let y = (self.state[k] & 0x8000_0000) | (self.state[(k + 1) % 624] & 0x7fff_ffff);
                let twist = if y & 1 == 0 { 0 } else { 0x9908_b0df };
                self.state[k] = self.state[(k + 397) % 624] ^ (y >> 1) ^ twist;
            }
            self.at = 0;
        }
        let mut y = self.state[self.at];
        self.at += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^ (y >> 18)
    }
}

impl HostTables {
    fn new() -> Self {
        let points = TABLE as f64;
        let sine = |i: usize| ((2 * i) as f64 * PI / points).sin() as f32;
        // Up from -1 through the first half and back, a quarter turn on,
        // so that the table starts at 0.
        let mut triangle = vec![0.0; TABLE];
        for i in 0..TABLE {
            let half = (TABLE / 2) as f64;
            let value = match i < TABLE / 2 {
                true => (4 * i) as f64 / points - 1.0,
                false => 1.0 - 4.0 * (i as f64 - half) / points,
            };
            triangle[(i + 0x3000) % TABLE] = value as f32;
        }
        let square = |i: usize| if i < TABLE / 2 { 1.0 } else { -1.0 };
        let saw = |i: usize| ((2 * i) as f64 / points - 1.0) as f32;
        // 1 - 2 * 0.997^i in 16 bits, then 1 from 7600 on and -1 for the
        // last sixth.
        let mut rounded = vec![1.0; TABLE];
        let mut left = 32_767.0f32;
        for point in &mut rounded[..7600] {
            let whole = f64::from(32_767.0 - left * 2.0) as i64 as i16;
            *point = (f64::from(whole) / 32_767.0) as f32;
            left *= 0.997;
        }
        rounded[13_653..].fill(-1.0);
        let mut twister = Twister::new(19_650_218);
        let scale = 2.0 / f64::from(u32::MAX);
        let noise = |_| (f64::from(twister.next()) * scale - 1.0) as f32;
        HostTables {
            shapes: [
                (0..TABLE).map(sine).collect(),
                triangle,
                (0..TABLE).map(square).collect(),
                (0..TABLE).map(saw).collect(),
                rounded,
                (0..TABLE).map(noise).collect(),
            ],
        }
    }

    /// The tables, made once.
    fn get() -> &'static HostTables {
        static TABLES: OnceLock<HostTables> = OnceLock::new();
        TABLES.get_or_init(HostTables::new)
    }
}

/// One of the host's tables, for the other plugins that read them: 0 the
/// sine, 1 the triangle, 2 the square, 3 the saw, 4 the rounded saw and 5
/// the noise, each 16384 points long, so a 32-bit phase reads one at
/// `phase >> 18`. The first call makes the tables, which asks for memory,
/// so a plugin calls this as it is made.
pub(crate) fn host_table(shape: usize) -> &'static [f32] {
    &HostTables::get().shapes[shape.min(5)]
}

/// A shape of the HQ engine at `x` of a turn: 1 the triangle, 2 the
/// square, 3 the saw and 4 the rounded saw.
fn shape_at(shape: usize, x: f32) -> f32 {
    match shape {
        1 if x < 0.25 => x * 4.0,
        1 if x < 0.75 => 1.0 - (x - 0.25) * 4.0,
        1 => (x - 0.75) * 4.0 - 1.0,
        2 if x < 0.5 => 1.0,
        2 => -1.0,
        3 => (x + x) - 1.0,
        4 if f64::from(x) >= 5.0 / 6.0 => -1.0,
        4 if x >= 0.463_867_2 => 1.0,
        4 => {
            let gone = 1.0 - 0.997f64.powf(f64::from(x) * 16_384.0);
            ((gone + gone) - 1.0) as f32
        }
        _ => 0.0,
    }
}

/// The levels the HQ engine has of each shape, four to the octave, and the
/// points of each.
const MIP_LEVELS: usize = 44;
const MIP_POINTS: usize = 32_768;

/// The HQ engine's tables: for the four shapes of [`shape_at`], every
/// level with two points more for the read past its end.
#[derive(Debug, PartialEq)]
struct MipTables {
    levels: Vec<Vec<f32>>,
}

/// A fast Fourier transform in place, of a length that is a power of two.
/// `inverse` turns it round and leaves the scale to the caller.
fn fft(re: &mut [f64], im: &mut [f64], inverse: bool) {
    let len = re.len();
    let mut j = 0;
    for i in 1..len {
        let mut bit = len >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut span = 2;
    while span <= len {
        let turn = if inverse { TAU } else { -TAU } / span as f64;
        let (step_re, step_im) = (turn.cos(), turn.sin());
        for start in (0..len).step_by(span) {
            let (mut w_re, mut w_im) = (1.0, 0.0);
            for k in start..start + span / 2 {
                let other = k + span / 2;
                let (t_re, t_im) = (
                    re[other] * w_re - im[other] * w_im,
                    re[other] * w_im + im[other] * w_re,
                );
                (re[other], im[other]) = (re[k] - t_re, im[k] - t_im);
                (re[k], im[k]) = (re[k] + t_re, im[k] + t_im);
                (w_re, w_im) = (
                    w_re * step_re - w_im * step_im,
                    w_re * step_im + w_im * step_re,
                );
            }
        }
        span *= 2;
    }
}

impl MipTables {
    fn new() -> Self {
        let mut levels = Vec::with_capacity(4 * MIP_LEVELS);
        for shape in 1..=4 {
            // 2048 points of the shape, with straight lines between them.
            let coarse: Vec<f32> = (0..2048)
                .map(|i| shape_at(shape, (f64::from(i) / 2048.0) as f32))
                .collect();
            let mut re: Vec<f64> = (0..MIP_POINTS)
                .map(|i| {
                    let (a, b) = (coarse[i / 16], coarse[(i / 16 + 1) % 2048]);
                    f64::from(a) + f64::from(b - a) * (i % 16) as f64 / 16.0
                })
                .collect();
            let mut im = vec![0.0; MIP_POINTS];
            fft(&mut re, &mut im, false);
            for level in 0..MIP_LEVELS {
                // The first 1024 * 2^(-level / 4) harmonics.
                let kept = (1024.0 * (-(level as f64) / 4.0).exp2()) as usize;
                let (mut cut_re, mut cut_im) = (re.clone(), im.clone());
                cut_re[kept + 1..MIP_POINTS - kept].fill(0.0);
                cut_im[kept + 1..MIP_POINTS - kept].fill(0.0);
                fft(&mut cut_re, &mut cut_im, true);
                let mut table: Vec<f32> = cut_re
                    .iter()
                    .map(|value| (value / MIP_POINTS as f64) as f32)
                    .collect();
                table.extend_from_within(..2);
                levels.push(table);
            }
        }
        MipTables { levels }
    }

    /// The tables, made the first time a state asks for HQ.
    fn get() -> &'static MipTables {
        static TABLES: OnceLock<MipTables> = OnceLock::new();
        TABLES.get_or_init(MipTables::new)
    }

    fn level(&self, shape: usize, level: usize) -> &[f32] {
        &self.levels[(shape - 1) * MIP_LEVELS + level]
    }
}

/// Delphi's `Random(range)`, which FL draws a note's phases and a noise
/// oscillator's steps with.
fn random(seed: &mut u32, range: u32) -> u32 {
    *seed = seed.wrapping_mul(0x0808_8405).wrapping_add(1);
    ((u64::from(*seed) * u64::from(range)) >> 32) as u32
}

/// The left and right gain of FL's circular pan law for a pan from -1 to
/// 1: `cos` and `sin` of `(1 + pan) * pi / 4`, times `vol`.
pub fn pan_law(pan: f32, vol: f32) -> [f32; 2] {
    let angle = (1.0 + f64::from(pan.clamp(-1.0, 1.0))) * FRAC_PI_4;
    let vol = f64::from(vol);
    [(angle.cos() * vol) as f32, (angle.sin() * vol) as f32]
}

/// What the phase moves by a sample at this many cents and this rate.
pub fn increment(cents: f32, rate: f32) -> u32 {
    let base = (BASE / f64::from(rate)) as f32;
    let exact = f64::from(base) * (f64::from(cents) / 1200.0).exp2();
    exact.round_ties_even() as i64 as u32
}

/// The ticks an envelope time of the tab comes to, where `unit` is the
/// ticks of `1001^(v / 65536) - 1 = 1`. Up to 100 is no time at all.
pub fn ticks(value: i32, unit: f64) -> i32 {
    if value < 101 {
        return 0;
    }
    let along = (f64::from(value) / 65_536.0) as f32;
    let curve = ((f64::from(along) * 1001f64.ln()).exp() - 1.0) as f32;
    (f64::from(curve) * unit).round_ties_even() as i32
}

/// The bend of one stretch of an envelope: `k` from its tension knob,
/// `1 / |k|` and `ln(|k| + 1)`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Tension {
    k: f32,
    a: f32,
    b: f32,
}

impl Tension {
    /// `sign(t) * (6347^|t / 128| - 1) * 0.1` for a knob from -128 to 128.
    fn of(knob: i32) -> Self {
        if knob == 0 {
            return Tension::default();
        }
        let size = f64::from((knob as f32 * (1.0 / 128.0)).abs());
        let k = ((size * 6347f64.ln()).exp() - 1.0) as f32 * 0.1;
        let k = if knob < 0 { -k } else { k };
        let size = f64::from(k.abs());
        Tension {
            k,
            a: (1.0 / size) as f32,
            b: (size + 1.0).ln() as f32,
        }
    }

    /// The level at `value` of the way up a straight stretch.
    fn shape(&self, value: f32) -> f32 {
        let (value, a, b) = (f64::from(value), f64::from(self.a), f64::from(self.b));
        if self.k == 0.0 {
            value as f32
        } else if self.k > 0.0 {
            (1.0 - (((1.0 - value) * b).exp() - 1.0) * a) as f32
        } else {
            (((value * b).exp() - 1.0) * a) as f32
        }
    }

    /// How far up the straight stretch a level is.
    fn unshape(&self, level: f32) -> f32 {
        let (level, size, b) = (f64::from(level), f64::from(self.k.abs()), f64::from(self.b));
        if self.k == 0.0 {
            level as f32
        } else if self.k > 0.0 {
            (1.0 - ((1.0 - level) * size + 1.0).ln() / b) as f32
        } else {
            ((level * size + 1.0).ln() / b) as f32
        }
    }
}

/// One block of the tab, worked out for a tempo.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Block {
    enabled: bool,
    full_when_off: bool,
    global_lfo: bool,
    /// The attack knob is at no time at all.
    no_attack: bool,
    /// Ticks.
    predelay: i32,
    hold: i32,
    /// What a tick adds to the straight envelope in each stretch.
    attack: f32,
    decay: f32,
    release: f32,
    sustain: f32,
    tensions: [Tension; 3],
    /// How far the envelope and the LFO move their target: of 128, and for
    /// the pitch as they are, in cents.
    amount: f32,
    lfo_amount: f32,
    cents: f32,
    lfo_cents: f32,
    lfo_predelay: i32,
    lfo_attack: f32,
    /// What a tick adds to the LFO's 32-bit phase.
    lfo_step: u32,
    lfo_shape: usize,
}

impl Block {
    fn of(raw: &[i32; 17], tempo: f64, ppq: f64) -> Self {
        let unit = ppq / 4.0 / 48.0 * 24_576.0 / 1000.0;
        let unit_for = |fixed: i32| match raw[FLAGS] & fixed {
            0 => unit * tempo / 120.0,
            _ => unit,
        };
        let (env, lfo) = (unit_for(FIXED_TICKS), unit_for(LFO_FIXED_TICKS));
        let over = |value: i32, unit: f64| (1.0 / f64::from(ticks(value, unit) + 1)) as f32;
        let of_128 = |value: i32| value as f32 * (1.0 / 128.0);
        Block {
            enabled: raw[ENABLED] != 0,
            full_when_off: raw[FLAGS] & FULL_WHEN_OFF != 0,
            global_lfo: raw[FLAGS] & GLOBAL_LFO != 0,
            no_attack: raw[ATTACK] == 100,
            predelay: ticks(raw[PREDELAY], env),
            hold: ticks(raw[HOLD], env),
            attack: over(raw[ATTACK], env),
            decay: -over(raw[DECAY], env),
            release: -over(raw[RELEASE], env),
            sustain: of_128(raw[SUSTAIN]),
            tensions: std::array::from_fn(|k| Tension::of(raw[TENSIONS + k])),
            amount: of_128(raw[AMOUNT]),
            lfo_amount: of_128(raw[LFO_AMOUNT]),
            cents: raw[AMOUNT] as f32,
            lfo_cents: raw[LFO_AMOUNT] as f32,
            lfo_predelay: ticks(raw[LFO_PREDELAY], lfo),
            lfo_attack: over(raw[LFO_ATTACK], lfo),
            lfo_step: (0x1_0000_0000u64 / ticks(raw[LFO_SPEED], lfo).max(3) as u64) as u32,
            lfo_shape: raw[LFO_SHAPE].clamp(0, 2) as usize,
        }
    }
}

/// One envelope and LFO of a note.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Envelope {
    stage: u8,
    /// The ticks the stage has lasted, for the two that are counted.
    count: i32,
    /// The straight envelope, and what its stretch's bend makes of it.
    value: f32,
    level: f32,
    lfo_phase: u32,
    /// The ticks the LFO still waits, how far it has faded in, and what
    /// it is at.
    lfo_waits: i32,
    lfo_fade: f32,
    lfo: f32,
}

impl Envelope {
    fn new(block: &Block) -> Self {
        let mut envelope = Envelope {
            lfo_waits: block.lfo_predelay,
            ..Envelope::default()
        };
        if block.enabled && block.predelay >= 1 {
            envelope.stage = WAIT;
        } else {
            envelope.stage = RISE;
            let full = match block.enabled {
                true => block.no_attack,
                false => block.full_when_off,
            };
            if full {
                envelope.value = 1.0;
                envelope.stage = if block.hold < 1 { FALL } else { HELD };
            }
        }
        envelope.level = envelope.value;
        envelope
    }

    /// One tick on. `global` is the phase of an LFO the notes share.
    fn step(&mut self, block: &Block, global: u32, shapes: &HostTables) {
        if block.enabled {
            match self.stage {
                WAIT => {
                    self.count += 1;
                    if block.predelay <= self.count {
                        self.stage = RISE;
                    }
                }
                RISE => {
                    self.value += block.attack;
                    if 1.0 <= self.value {
                        self.value = 1.0;
                        (self.stage, self.count) = if block.hold < 1 {
                            (FALL, self.count)
                        } else {
                            (HELD, 0)
                        };
                    }
                    self.level = block.tensions[0].shape(self.value);
                }
                HELD => {
                    self.count += 1;
                    if block.hold <= self.count {
                        self.stage = FALL;
                    }
                }
                FALL => {
                    self.value += block.decay;
                    self.level = block.tensions[1].shape(self.value);
                    if self.level <= block.sustain {
                        self.level = block.sustain;
                        self.value = block.tensions[1].unshape(self.level);
                        (self.stage, self.count) = (SUSTAINED, 0);
                    }
                }
                RELEASED => {
                    self.value += block.release;
                    self.level = block.tensions[2].shape(self.value);
                }
                _ => {}
            }
            if self.stage > HELD && self.level < TINY {
                (self.level, self.stage) = (0.0, OVER);
            }
        }
        if self.lfo_waits != 0 {
            self.lfo_waits -= 1;
            return;
        }
        self.lfo_phase = match block.global_lfo {
            true => global,
            false => self.lfo_phase.wrapping_add(block.lfo_step),
        };
        self.lfo = shapes.shapes[block.lfo_shape][(self.lfo_phase >> 18) as usize];
        if self.lfo_fade != 1.0 {
            self.lfo_fade += block.lfo_attack;
            if 1.0 < self.lfo_fade {
                self.lfo_fade = 1.0;
            }
            self.lfo *= self.lfo_fade;
        }
    }

    /// The note off: the release starts from the level the envelope is at.
    fn release(&mut self, block: &Block) {
        if self.stage <= RELEASED {
            self.value = block.tensions[2].unshape(self.level);
            (self.stage, self.count) = (RELEASED, 0);
        }
    }
}

/// The filter's kinds: FL's Fast LP, the four biquads and the
/// state-variable low-pass.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Fast,
    Biquad(u8),
    Svf,
}

/// The kind of each of the eight filter types, and whether it runs twice.
const FILTERS: [(Kind, bool); 8] = [
    (Kind::Fast, false),
    (Kind::Biquad(1), false),
    (Kind::Biquad(2), false),
    (Kind::Biquad(3), false),
    (Kind::Biquad(4), false),
    (Kind::Biquad(1), true),
    (Kind::Svf, false),
    (Kind::Svf, true),
];

/// The Fast LP's coefficient and what it keeps of its speed, for a cutoff
/// and a resonance from 0 to 1 and `s = 44100 / rate`:
///
/// ```text
/// f  = min(1, (601^cut - 1) / 600 * s)
/// fb = 1 - (601^(1 - 0.625 res) - 1) / 600
/// ```
pub fn fast_lp(cut: f32, res: f32, s: f32) -> (f32, f32) {
    let curve =
        |x: f32| ((f64::from(x * 601f32.ln()).exp() - 1.0) * f64::from(1.0f32 / 600.0)) as f32;
    let f = if cut <= 0.0 {
        0.0
    } else if cut < 1.0 {
        (curve(cut) * s).min(1.0)
    } else {
        1.0
    };
    let fb = if res <= 0.0 {
        0.0
    } else {
        1.0 - curve(1.0 - res.min(1.0) * 0.625)
    };
    (f, fb)
}

/// The biquads' angle a sample and their Q:
///
/// ```text
/// w = clamp(((51^cut - 1) * 0.0596 + 0.02) * s, 0.02 s, 3)
/// q = (51^(0.625 res) - 1) * 0.4 + 0.1
/// ```
pub fn biquad_wq(cut: f32, res: f32, s: f32) -> (f32, f32) {
    let ln51 = 51f32.ln();
    let grown = |x: f32| f64::from(x).exp() - 1.0;
    let floor = (f64::from(s) * 0.02) as f32;
    let w = ((grown(cut * ln51) * f64::from(0.0596f32) + 0.02) * f64::from(s)) as f32;
    let w = if w < floor { floor } else { w.min(3.0) };
    let q = if res <= 0.0 {
        0.1
    } else {
        (grown(res.min(1.0) * (ln51 * 0.625)) * f64::from(0.4f32) + 0.1) as f32
    };
    (w, q)
}

/// The coefficients `[b0, b1, b2, a1, a2]` of biquad `kind` at an angle
/// and a Q: 1 the low-pass, 2 the band-pass, 3 the high-pass and 4 the
/// notch of the RBJ cookbook.
pub fn biquad(kind: u8, w: f32, q: f32) -> [f32; 5] {
    let (sin, cos) = w.sin_cos();
    let (alpha, cos) = (f64::from(sin / (q * 2.0)), f64::from(cos));
    let n = 1.0 / (alpha + 1.0);
    let a1 = (n * cos * -2.0) as f32;
    let a2 = (n * (1.0 - alpha)) as f32;
    let [b0, b1, b2] = match kind {
        1 => {
            let b1 = (n * (1.0 - cos)) as f32;
            [b1 * 0.5, b1, b1 * 0.5]
        }
        2 => [(n * alpha) as f32, 0.0, (n * -alpha) as f32],
        3 => {
            let b1 = (n * -(cos + 1.0)) as f32;
            [-b1 * 0.5, b1, -b1 * 0.5]
        }
        _ => [n as f32, a1, n as f32],
    };
    [b0, b1, b2, a1, a2]
}

/// The state-variable filter's coefficient and its damping:
///
/// ```text
/// f = min(1, (26^cut - 1) * 0.04 * s)
/// q = 1 - 0.8 res, no lower than 0.2
/// ```
pub fn svf(cut: f32, res: f32, s: f32) -> (f32, f32) {
    let f = if cut <= 0.0 {
        0.0
    } else {
        let grown = ((f64::from(cut) * 26f64.ln()).exp() - 1.0) as f32;
        (grown * 0.04 * s).min(1.0)
    };
    let q = if res <= 0.0 {
        1.0
    } else if res < 1.0 {
        1.0 - res * 0.8
    } else {
        0.2
    };
    (f, q)
}

/// What a sample adds to a value that is headed for `target` from `from`
/// over `len` samples, at no more than `limit` a sample, and where that
/// gets it.
fn ramp(target: f32, from: f32, limit: f32, len: usize) -> (f32, f32) {
    let mut step = target - from;
    if step == 0.0 {
        return (0.0, target);
    }
    step /= len as f32;
    if limit < step.abs() {
        step = limit.copysign(step);
        return (step, from + step * len as f32);
    }
    (step, target)
}

/// The filter of one note.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Filter {
    kind: Kind,
    twice: bool,
    /// A Fast LP that is wide open does nothing until it first is not.
    active: bool,
    /// The Fast LP's coefficient now, what a sample adds to it, where that
    /// gets it by the next tick, and what it keeps of its speed.
    f: f64,
    f_step: f32,
    f_end: f32,
    fb: f32,
    /// The level and the speed of the Fast LP, left and right.
    fast: [[f64; 2]; 2],
    coefs: [f32; 5],
    /// Two inputs and two outputs back, left and right, of each stage.
    biquads: [[[f32; 4]; 2]; 2],
    svf_f: f32,
    svf_q: [f32; 2],
    /// The low and the band of each stage, left and right.
    svfs: [[[f32; 2]; 2]; 2],
}

impl Filter {
    fn new(kind: Kind, twice: bool) -> Self {
        Filter {
            kind,
            twice,
            active: kind != Kind::Fast,
            f: 1.0,
            f_step: 0.0,
            f_end: 1.0,
            fb: 0.0,
            fast: [[0.0; 2]; 2],
            coefs: [0.0; 5],
            biquads: [[[0.0; 4]; 2]; 2],
            svf_f: 0.0,
            svf_q: [1.0; 2],
            svfs: [[[0.0; 2]; 2]; 2],
        }
    }

    /// Take the cutoff and the resonance of the `len` samples to the next
    /// tick. What the note's gain is to be multiplied by comes back.
    fn set(&mut self, cut: f32, res: f32, s: f32, first: bool, len: usize) -> f32 {
        match self.kind {
            Kind::Fast => {
                let (f, fb) = fast_lp(cut, res, s);
                self.fb = fb;
                if f != 1.0 || fb != 0.0 {
                    self.active = true;
                }
                let from = if first { f } else { self.f_end };
                (self.f_step, self.f_end) = ramp(f, from, s * 0.001, len);
                self.f = f64::from(from);
                1.0
            }
            Kind::Biquad(kind) => {
                let (w, q) = biquad_wq(cut, res, s);
                self.coefs = biquad(kind, w, q);
                match self.twice {
                    true => (1.0 - f64::from(q) * (3.2 - f64::from(w)) * 0.046_875) as f32,
                    false => 1.0,
                }
            }
            Kind::Svf => {
                let (f, q) = svf(cut, res, s);
                self.svf_f = f;
                self.svf_q = [q, q + (1.0 - q) * 0.25];
                1.0
            }
        }
    }

    /// One frame through.
    fn run(&mut self, frame: [f32; 2]) -> [f32; 2] {
        if !self.active {
            return frame;
        }
        let mut out = frame;
        match self.kind {
            Kind::Fast => {
                for (side, state) in self.fast.iter_mut().enumerate() {
                    let [level, speed] = state;
                    *speed += (f64::from(frame[side]) - *level) * self.f;
                    *level += *speed;
                    *speed *= f64::from(self.fb);
                    out[side] = *level as f32;
                }
                self.f += f64::from(self.f_step);
            }
            Kind::Biquad(_) => {
                let [b0, b1, b2, a1, a2] = self.coefs;
                let stages = if self.twice { 2 } else { 1 };
                for stage in &mut self.biquads[..stages] {
                    for (side, state) in stage.iter_mut().enumerate() {
                        let [x1, x2, y1, y2] = *state;
                        let x = out[side];
                        let y = x * b0 + (((x2 * b2 + x1 * b1) - y1 * a1) - y2 * a2);
                        *state = [x, x1, y, y1];
                        out[side] = y;
                    }
                }
            }
            Kind::Svf => {
                let stages = if self.twice { 2 } else { 1 };
                for (stage, q) in self.svfs[..stages].iter_mut().zip(self.svf_q) {
                    for (side, state) in stage.iter_mut().enumerate() {
                        let [low, band] = *state;
                        let low = band * self.svf_f + low;
                        let band = (-(band * q + low) + out[side]) * self.svf_f + band;
                        *state = [low, band];
                        out[side] = low;
                    }
                }
            }
        }
        out
    }
}

/// What an oscillator reads on one side until the next tick.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
enum Source {
    /// A host table, by its number.
    #[default]
    Silent,
    Table(usize),
    /// The HQ engine's sine, its noise, a shape worked out at the phase,
    /// and a shape's table of a level.
    Sine,
    Noise,
    Shape(usize),
    Mip(usize, usize),
}

/// One oscillator, as the parameters have it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Osc {
    cents: i32,
    /// With its sign turned for the invert flag.
    level: f32,
    gains: [f32; 2],
    /// What the right side's phase starts ahead by.
    offset: u32,
    detune: i32,
    /// The host table it reads: the saw for the custom shape.
    shape: usize,
    custom: bool,
}

/// What the state comes to at a rate and a tempo.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Derived {
    oscs: [Osc; 3],
    am: bool,
    hq: bool,
    /// The right side is rendered and is not a copy of the left.
    stereo: bool,
    /// The range a note's phases are drawn from.
    random: u32,
    blocks: [Block; 5],
    filter: (Kind, bool),
    cutoff: f32,
    resonance: f32,
    rate: f32,
    /// `44100 / rate`.
    s: f32,
    /// One over the rate, as the HQ engine has it.
    period: f32,
}

/// The 10 ms a note fades out by at this rate: half a turn of a cosine,
/// from 1 down to 0.
fn fade_out(rate: f32) -> Vec<f32> {
    let len = ((f64::from(rate) * 0.01).round_ties_even() as usize).max(1);
    let step = (PI / (len + 1) as f64) as f32;
    let fade = |n: usize| (((n + 1) as f32 * step).cos() + 1.0) * 0.5;
    (0..len).map(fade).collect()
}

impl Derived {
    fn of(plugin: &ThreeOsc) -> Self {
        let p = &plugin.params;
        let of_128 = |value: i32| value as f32 * (1.0 / 128.0);
        let m3 = of_128(p[13]);
        let rest = if plugin.am { 1.0 } else { 1.0 - m3 };
        let second = rest * p[6] as f32 * (1.0 / 128.0);
        let levels = [rest - second, second, m3];
        let oscs: [Osc; 3] = std::array::from_fn(|k| {
            let at = 7 * k;
            let level = match plugin.invert[k] {
                true => -levels[k],
                false => levels[k],
            };
            Osc {
                cents: p[at + 2].wrapping_mul(100).wrapping_add(p[at + 3]),
                level,
                gains: pan_law(p[at] as f32 * (1.0 / 64.0), level),
                offset: p[at + 4].wrapping_mul(0x1ff_ffff) as u32,
                detune: p[at + 5],
                shape: if p[at + 1] < 6 { p[at + 1] as usize } else { 3 },
                custom: p[at + 1] >= 6,
            }
        });
        let stereo =
            |osc: &Osc, pan: i32| osc.shape == 5 || osc.offset != 0 || osc.detune != 0 || pan != 0;
        let tab = &plugin.tab;
        Derived {
            stereo: p[20] != 0 || (0..3).any(|k| stereo(&oscs[k], p[7 * k])),
            oscs,
            am: plugin.am,
            hq: plugin.hq,
            random: (p[20] as u32) << 18,
            blocks: std::array::from_fn(|k| Block::of(&tab.blocks[k], plugin.tempo, plugin.ppq)),
            filter: FILTERS[tab.filter.clamp(0, 7) as usize],
            cutoff: tab.cutoff as f32 * (1.0 / 256.0),
            resonance: tab.resonance as f32 * (1.0 / 256.0),
            rate: plugin.rate,
            s: (44_100.0 / f64::from(plugin.rate)) as f32,
            period: 1.0 / plugin.rate,
        }
    }
}

/// One note.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Voice {
    /// The host's number for it.
    id: u32,
    /// The note's own cents from key 60, pan, gain and filter offsets.
    pitch: f32,
    pan: f32,
    vol: f32,
    cut: f32,
    res: f32,
    /// Left and right of each oscillator.
    phases: [[u32; 2]; 3],
    steps: [[u32; 2]; 3],
    sources: [[Source; 2]; 3],
    stereo: bool,
    envelopes: [Envelope; 5],
    filter: Filter,
    /// The gains now, what a sample adds to them, and where that gets
    /// them by the next tick.
    gains: [f32; 2],
    gain_steps: [f32; 2],
    gain_ends: [f32; 2],
    /// It has played nothing yet.
    first: bool,
    /// A tick has passed since it last took its envelopes' levels.
    stale: bool,
    ticked: bool,
    /// How far into the fade out it is, once that has started.
    fade: Option<usize>,
}

impl Voice {
    fn new(note: &NoteOn, derived: &Derived, seed: &mut u32) -> Self {
        let mut phases = [[0u32; 2]; 3];
        for (phase, osc) in phases.iter_mut().zip(&derived.oscs) {
            phase[1] = osc.offset;
            if derived.random != 0 {
                for side in phase {
                    let drawn = random(seed, derived.random).wrapping_sub(derived.random >> 1);
                    *side = side.wrapping_add(drawn.wrapping_mul(0x100));
                }
            }
        }
        let (kind, twice) = derived.filter;
        Voice {
            id: note.voice,
            pitch: (f32::from(note.key) - 60.0) * 100.0 + note.fine_pitch,
            pan: note.pan,
            vol: note.velocity,
            cut: note.mod_x,
            res: note.mod_y,
            phases,
            steps: [[0; 2]; 3],
            sources: [[Source::Silent; 2]; 3],
            stereo: derived.stereo,
            envelopes: std::array::from_fn(|k| Envelope::new(&derived.blocks[k])),
            filter: Filter::new(kind, twice),
            gains: [0.0; 2],
            gain_steps: [0.0; 2],
            gain_ends: [0.0; 2],
            first: true,
            stale: true,
            ticked: false,
            fade: None,
        }
    }

    /// Start the releases. A note with no volume envelope fades out.
    fn release(&mut self, derived: &Derived) {
        for (envelope, block) in self.envelopes.iter_mut().zip(&derived.blocks).skip(VOLUME) {
            envelope.release(block);
        }
        if !derived.blocks[VOLUME].enabled {
            self.fade.get_or_insert(0);
        }
    }

    /// What an oscillator reads at a step of its phase.
    fn source(osc: &Osc, step: u32, derived: &Derived) -> Source {
        if osc.level.to_bits() == 0 {
            return Source::Silent;
        }
        if !derived.hq {
            return Source::Table(osc.shape);
        }
        match osc.shape {
            0 => Source::Sine,
            5 => Source::Noise,
            shape => {
                let turns = step as f32 * (1.0 / 4_294_967_296.0);
                if f64::from(turns) < 5e-4 {
                    return Source::Shape(shape);
                }
                let hz = f64::from(turns / derived.period);
                let level = (hz / 8.1758).log2() * 4.0 - 5.0;
                let top = (MIP_LEVELS - 2) as f64;
                Source::Mip(shape, level.clamp(0.0, top) as usize)
            }
        }
    }

    /// Take what the envelopes are at for the `len` samples to the next
    /// tick: the pitch, the gains and the filter.
    fn begin(&mut self, derived: &Derived, globals: &[u32; 5], seed: &mut u32, len: usize) {
        let shapes = HostTables::get();
        let starts_full = self.envelopes[VOLUME].stage > RISE;
        if self.first || self.ticked {
            for (k, envelope) in self.envelopes.iter_mut().enumerate() {
                envelope.step(&derived.blocks[k], globals[k], shapes);
            }
            if self.envelopes[VOLUME].stage == OVER {
                self.fade.get_or_insert(0);
            }
        }
        let [pan, volume, mod_x, mod_y, pitch] = &self.envelopes;
        let blocks = &derived.blocks;
        let pan_mod = pan.lfo * blocks[0].lfo_amount * 2.0;
        let level = (volume.level * (volume.lfo * blocks[VOLUME].lfo_amount + 1.0)).max(0.0);
        let moved = |envelope: &Envelope, block: &Block| {
            envelope.level * block.amount + envelope.lfo * block.lfo_amount
        };
        let cut = self.cut + derived.cutoff + moved(mod_x, &blocks[MOD_X]);
        let res = self.res + derived.resonance + moved(mod_y, &blocks[MOD_Y]);
        let cents =
            self.pitch + pitch.level * blocks[PITCH].cents + pitch.lfo * blocks[PITCH].lfo_cents;

        self.stereo |= derived.stereo;
        let sides = if self.stereo { 2 } else { 1 };
        for side in 0..sides {
            for (k, osc) in derived.oscs.iter().enumerate() {
                let step = if osc.shape == 5 {
                    random(seed, 0x9_9326) + 0x9_9326
                } else {
                    let detune = if side == 1 { osc.detune as f32 } else { 0.0 };
                    increment(cents + osc.cents as f32 + detune, derived.rate)
                };
                self.steps[k][side] = step;
                self.sources[k][side] = Voice::source(osc, step, derived);
            }
        }

        let scale = self.filter.set(cut, res, derived.s, self.first, len);
        let ends = pan_law(self.pan + pan_mod, self.vol * level).map(|gain| gain * scale);
        let from = match (self.first, starts_full) {
            (true, true) => ends,
            (true, false) => [0.0; 2],
            (false, _) => self.gain_ends,
        };
        for side in 0..2 {
            (self.gain_steps[side], self.gain_ends[side]) =
                ramp(ends[side], from[side], derived.s * 0.001, len);
        }
        self.gains = from;
        (self.first, self.stale, self.ticked) = (false, false, false);
    }

    /// The next sample of one oscillator on one side, before its gain.
    fn read(source: Source, phase: u32, mips: Option<&MipTables>, noise: &mut u32) -> f32 {
        match source {
            Source::Silent => 0.0,
            Source::Table(shape) => HostTables::get().shapes[shape][(phase >> 18) as usize],
            Source::Sine => (phase as f32 * (TAU / 4_294_967_296.0) as f32).sin(),
            Source::Noise => {
                *noise = noise.wrapping_mul(0x0bb3_8435).wrapping_add(0x3619_636b);
                *noise as i32 as f32 * (1.0 / 2_147_483_648.0)
            }
            Source::Shape(shape) => shape_at(shape, phase as f32 * (1.0 / 4_294_967_296.0)),
            Source::Mip(shape, level) => {
                let Some(mips) = mips else {
                    return 0.0;
                };
                let table = mips.level(shape, level);
                let at = phase as f32 * (1.0 / 4_294_967_296.0) * MIP_POINTS as f32;
                let point = (at as usize).min(MIP_POINTS);
                let along = at - point as f32;
                table[point] + (table[point + 1] - table[point]) * along
            }
        }
    }

    /// Add the next frames of the note to a block. Whether the note is
    /// over comes back.
    fn render(
        &mut self,
        derived: &Derived,
        fade: &[f32],
        mips: Option<&MipTables>,
        noise: &mut u32,
        left: &mut [f32],
        right: &mut [f32],
    ) -> bool {
        let sides = if self.stereo { 2 } else { 1 };
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            if self.fade.is_some_and(|at| at >= fade.len()) {
                return true;
            }
            let mut frame = [0.0f32; 2];
            for (side, sum) in frame.iter_mut().enumerate().take(sides) {
                for (k, osc) in derived.oscs.iter().enumerate() {
                    let phase = self.phases[k][side];
                    self.phases[k][side] = phase.wrapping_add(self.steps[k][side]);
                    let source = self.sources[k][side];
                    if source == Source::Silent {
                        continue;
                    }
                    let sample = Voice::read(source, phase, mips, noise) * osc.gains[side];
                    if derived.am && k == 2 {
                        *sum -= *sum * sample;
                    } else {
                        *sum += sample;
                    }
                }
            }
            if !self.stereo {
                frame[1] = frame[0];
                for phase in &mut self.phases {
                    phase[1] = phase[0];
                }
            }
            let mut frame = self.filter.run(frame);
            if let Some(at) = &mut self.fade {
                frame = frame.map(|value| value * fade[*at]);
                *at += 1;
            }
            *l += frame[0] * self.gains[0];
            *r += frame[1] * self.gains[1];
            self.gains[0] += self.gain_steps[0];
            self.gains[1] += self.gain_steps[1];
        }
        self.fade.is_some_and(|at| at >= fade.len())
    }
}

/// 3x Osc. A new one is at 44.1 kHz and 140 beats a minute with FL's
/// defaults: three sines, an octave apart.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreeOsc {
    params: [i32; 21],
    /// The invert flag of each oscillator.
    invert: [bool; 3],
    am: bool,
    hq: bool,
    tab: Articulation,
    rate: f32,
    tempo: f64,
    /// The song's ticks to a beat.
    ppq: f64,
    derived: Derived,
    /// The 10 ms a note with no volume envelope fades out by.
    fade: Vec<f32>,
    /// In the order they started, and never more than [`VOICES`].
    voices: Vec<Voice>,
    /// What the phases and the noise steps are drawn from, and the HQ
    /// engine's noise.
    seed: u32,
    noise: u32,
    /// The samples to the next tick, and the part of a sample the tick
    /// after it is later by.
    to_tick: usize,
    tick_part: f64,
    /// The phase of each block's LFO for the notes that share it.
    globals: [u32; 5],
    mips: Option<&'static MipTables>,
}

impl Default for ThreeOsc {
    fn default() -> Self {
        let mut plugin = ThreeOsc {
            params: DEFAULTS,
            invert: [false; 3],
            am: false,
            hq: false,
            tab: new_tab(),
            rate: 44_100.0,
            tempo: 140.0,
            ppq: PPQ,
            derived: Derived {
                oscs: [Osc::default(); 3],
                am: false,
                hq: false,
                stereo: false,
                random: 0,
                blocks: [Block::default(); 5],
                filter: FILTERS[0],
                cutoff: 1.0,
                resonance: 0.0,
                rate: 44_100.0,
                s: 1.0,
                period: 1.0 / 44_100.0,
            },
            fade: fade_out(44_100.0),
            voices: Vec::with_capacity(VOICES),
            seed: 0,
            noise: 0,
            to_tick: 0,
            tick_part: 0.0,
            globals: [0; 5],
            mips: None,
        };
        HostTables::get();
        plugin.derived = Derived::of(&plugin);
        plugin
    }
}

impl ThreeOsc {
    /// The instrument tab: the state's, or a new plugin's.
    pub fn articulation(&self) -> &Articulation {
        &self.tab
    }

    /// The invert flags of the three oscillators, the AM flag and the HQ
    /// flag.
    pub fn flags(&self) -> ([bool; 3], bool, bool) {
        (self.invert, self.am, self.hq)
    }

    /// The samples of a tick of the song.
    fn tick_len(&self) -> f64 {
        (f64::from(self.rate) * 60.0 / (self.tempo * self.ppq)).max(1.0)
    }

    fn derive(&mut self) {
        self.derived = Derived::of(self);
    }
}

impl Generator for ThreeOsc {
    /// The notes that sound play on with the new state. A state with the
    /// HQ flag makes the HQ tables the first time one is taken.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let version = state_version(bytes, 460)?;
        let expected = match version {
            ..10 => 44,
            10 => 84,
            11 => 88,
            12 => 92,
            13 => 93,
            14 => 460,
            _ => return Err(StateError::Version { found: version }),
        };
        if bytes.len() != expected {
            return Err(StateError::Length {
                expected,
                found: bytes.len(),
            });
        }
        let word =
            |at: usize| i32::from_le_bytes(*bytes[4 * at..].first_chunk().unwrap_or(&[0; 4]));
        let mut params = [0; 21];
        let mut flags = [0u8; 5];
        if version < 10 {
            // The shape, the coarse and the fine of each oscillator, with
            // the mix level of the second and the third before theirs.
            let places = [1, 2, 3, 6, 8, 9, 10, 13, 15, 16, 17];
            for (at, param) in places.into_iter().enumerate() {
                params[param] = word(at);
            }
        } else {
            let count = if version < 12 { 20 } else { 21 };
            for (param, value) in params.iter_mut().enumerate().take(count) {
                *value = word(param + 1);
            }
            let rest = &bytes[4 + 4 * count..];
            let saved = rest.len().min(flags.len());
            flags[..saved].copy_from_slice(&rest[..saved]);
        }
        for param in [1, 8, 15] {
            if !(0..=6).contains(&params[param]) {
                return Err(StateError::Range {
                    param,
                    value: params[param],
                });
            }
        }
        let tab = match bytes.get(96..).and_then(|tab| tab.first_chunk::<364>()) {
            Some(tab) => Articulation::read(tab),
            None => new_tab(),
        };
        if !(0..=7).contains(&tab.filter) {
            // The filter type comes after the 114 parameters FL numbers.
            return Err(StateError::Range {
                param: 114,
                value: tab.filter,
            });
        }
        self.params = params;
        self.invert = [flags[0] != 0, flags[1] != 0, flags[2] != 0];
        self.am = flags[3] != 0;
        self.hq = flags[4] & 1 != 0;
        self.tab = tab;
        if self.hq {
            self.mips = Some(MipTables::get());
        }
        self.derive();
        Ok(())
    }

    /// The notes that sound keep their phases and move at the new rate
    /// from the next tick on.
    fn set_sample_rate(&mut self, hz: f32) {
        self.rate = hz;
        self.fade = fade_out(hz);
        self.derive();
    }

    /// The envelope times that do not follow the tempo are as many
    /// seconds at the new one, from the next tick on.
    fn set_tempo(&mut self, bpm: f64) {
        if bpm.is_finite() && bpm > 0.0 {
            self.tempo = bpm;
            self.derive();
        }
    }

    /// The envelope times are as many seconds in the new ticks, to the
    /// nearest of them, from the next tick on. A PPQ of 0 is not taken.
    fn set_ppq(&mut self, ppq: u16) {
        if ppq > 0 {
            self.ppq = f64::from(ppq);
            self.derive();
        }
    }

    fn note_on(&mut self, note: NoteOn) {
        if self.voices.len() == VOICES {
            self.voices.remove(0);
        }
        self.voices
            .push(Voice::new(&note, &self.derived, &mut self.seed));
    }

    fn note_off(&mut self, voice: u32) {
        for sounding in self.voices.iter_mut().filter(|v| v.id == voice) {
            sounding.release(&self.derived);
        }
    }

    fn all_notes_off(&mut self) {
        for sounding in &mut self.voices {
            sounding.release(&self.derived);
        }
    }

    /// The voice reads the note's pitch as its tick starts, where the
    /// oscillators' steps are worked out again.
    fn set_note_pitch(&mut self, voice: u32, pitch: f32) {
        for sounding in self.voices.iter_mut().filter(|v| v.id == voice) {
            sounding.pitch = pitch;
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        let mut at = 0;
        while at < len {
            if self.to_tick == 0 {
                self.tick_part += self.tick_len();
                self.to_tick = self.tick_part as usize;
                self.tick_part -= self.to_tick as f64;
                for (global, block) in self.globals.iter_mut().zip(&self.derived.blocks) {
                    *global = global.wrapping_add(block.lfo_step);
                }
                for voice in &mut self.voices {
                    (voice.stale, voice.ticked) = (true, true);
                }
            }
            let run = (len - at).min(self.to_tick);
            let (derived, fade, mips) = (&self.derived, &self.fade[..], self.mips);
            let (seed, noise, globals) = (&mut self.seed, &mut self.noise, &self.globals);
            let (to_tick, block) = (self.to_tick, at..at + run);
            self.voices.retain_mut(|voice| {
                if voice.stale {
                    voice.begin(derived, globals, seed, to_tick);
                }
                let (l, r) = (&mut left[block.clone()], &mut right[block.clone()]);
                !voice.render(derived, fade, mips, noise, l, r)
            });
            at += run;
            self.to_tick -= run;
        }
    }

    /// The 21 of the plugin's own, then the instrument tab's from 23 on:
    /// five blocks of 17 words, then the cutoff at 110, the resonance at
    /// 111 and the filter type at 113.
    fn param_count(&self) -> usize {
        PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        let tab = &self.tab;
        let value = match index {
            0..21 => self.params[index],
            TAB_CUTOFF => tab.cutoff,
            TAB_RESONANCE => tab.resonance,
            TAB_FILTER => tab.filter,
            TAB_PARAM..TAB_BLOCKS_END => {
                let at = index - TAB_PARAM;
                tab.blocks[at / 17][at % 17]
            }
            _ => 0,
        };
        value as f32
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let Some(range) = param_range(index) else {
            return;
        };
        let value = knob_value(value, range);
        match index {
            0..21 => self.params[index] = value,
            TAB_CUTOFF => self.tab.cutoff = value,
            TAB_RESONANCE => self.tab.resonance = value,
            TAB_FILTER => self.tab.filter = value,
            _ => {
                let at = index - TAB_PARAM;
                self.tab.blocks[at / 17][at % 17] = value;
            }
        }
        self.derive();
    }

    /// The ticks start over with the notes cut.
    fn reset(&mut self) {
        self.voices.clear();
        (self.to_tick, self.tick_part) = (0, 0.0);
        self.globals = [0; 5];
    }

    /// The 10 ms fade, after the volume envelope's whole release where
    /// that is on, and the tick the release may wait for.
    fn tail(&self) -> usize {
        let volume = &self.derived.blocks[VOLUME];
        let mut ticks = 1.0;
        if volume.enabled {
            ticks += (-1.0 / f64::from(volume.release)).ceil() + 1.0;
        }
        (ticks * self.tick_len()).ceil() as usize + self.fade.len() + 1
    }

    fn fidelity(&self) -> Fidelity {
        let heard = |osc: &Osc| osc.custom && osc.level.to_bits() != 0;
        if self.hq {
            Fidelity::Approximate("3x Osc HQ tables")
        } else if self.derived.oscs.iter().any(heard) {
            Fidelity::Approximate("3x Osc custom shape")
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
            color: 0,
            porta: false,
        }
    }

    /// The defaults with these parameters moved.
    fn knobs(moved: &[(usize, i32)]) -> [i32; 21] {
        let mut params = DEFAULTS;
        for (index, value) in moved {
            params[*index] = *value;
        }
        params
    }

    /// Oscillator 1 alone, with this shape.
    fn alone(shape: i32) -> [i32; 21] {
        knobs(&[(1, shape), (6, 0), (13, 0)])
    }

    /// A version 14 state: the parameters, the invert, AM and HQ flags and
    /// the instrument tab.
    fn state(params: [i32; 21], flags: [u8; 5], tab: &Articulation) -> Vec<u8> {
        let mut bytes = testing::state(&[&[14][..], &params].concat());
        bytes.extend(flags);
        bytes.extend([0; 3]);
        for block in &tab.blocks {
            bytes.extend(testing::state(block));
        }
        bytes.extend(testing::state(&[
            0,
            0,
            tab.cutoff,
            tab.resonance,
            0,
            tab.filter,
        ]));
        assert_eq!(bytes.len(), 460);
        bytes
    }

    fn with(params: [i32; 21]) -> ThreeOsc {
        let mut osc = ThreeOsc::default();
        osc.set_state(&state(params, [0; 5], &new_tab())).unwrap();
        osc
    }

    fn with_tab(params: [i32; 21], tab: &Articulation) -> ThreeOsc {
        let mut osc = ThreeOsc::default();
        osc.set_state(&state(params, [0; 5], tab)).unwrap();
        osc
    }

    fn play(osc: &mut ThreeOsc, len: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut left, mut right) = (vec![0.0; len], vec![0.0; len]);
        osc.process(&mut left, &mut right);
        (left, right)
    }

    /// The left side of a note of this key, `len` samples long.
    fn tone(osc: &mut ThreeOsc, key: i16, len: usize) -> Vec<f32> {
        osc.note_on(note(1, key));
        play(osc, len).0
    }

    /// The turns a sample of an oscillator at this many cents.
    fn turns(cents: f32) -> f64 {
        f64::from(increment(cents, 44_100.0)) / 4_294_967_296.0
    }

    /// How many samples near `around` are a whole number of periods.
    fn whole(turns: f64, around: usize) -> usize {
        ((around as f64 * turns).floor() / turns).round() as usize
    }

    /// The frequency of a steady tone, in turns a sample, from its first
    /// crossing to its last.
    fn pitch(sound: &[f32]) -> f64 {
        let mut found = Vec::new();
        for (n, pair) in sound.windows(2).enumerate() {
            let (a, b) = (f64::from(pair[0]), f64::from(pair[1]));
            if a != 0.0 && a.signum() != b.signum() {
                found.push(n as f64 + a / (a - b));
            }
        }
        (found.len() - 1) as f64 / 2.0 / (found[found.len() - 1] - found[0])
    }

    fn rms(sound: &[f32]) -> f64 {
        let sum: f64 = sound.iter().map(|v| f64::from(*v).powi(2)).sum();
        (sum / sound.len() as f64).sqrt()
    }

    fn db(ratio: f64) -> f64 {
        20.0 * ratio.log10()
    }

    #[test]
    fn a_note_follows_the_pitch_the_host_moves_it_to() {
        crate::testing::follows_note_pitch(|| Box::new(ThreeOsc::default()), 44_100.0);
    }
    #[test]
    fn key_60_is_523_25_hz_and_a_new_plugin_is_three_sines_an_octave_apart() {
        // 0 cents is 523.25 Hz of a 32-bit phase.
        assert!((turns(0.0) * RATE - 523.251).abs() < 1e-3);
        let mut osc = with(alone(0));
        assert_eq!(osc.fidelity(), Fidelity::Exact);
        let sound = tone(&mut osc, 60, 44_100);
        let hz = pitch(&sound) * RATE;
        assert!((hz / 523.251 - 1.0).abs() < 0.005, "{hz} Hz");
        assert!((hz / 523.251 - 1.0).abs() < 1e-4, "{hz} Hz");
        // The level is 1, through the pan law twice: 0.7071 for the
        // oscillator's pan and 0.7071 for the note's.
        let len = whole(turns(0.0), 40_000);
        assert!((partial(&sound[..len], turns(0.0)) - 0.5).abs() < 1e-3);
        // An octave up and a fifth down.
        for (key, cents) in [(72, 1200.0), (53, -700.0)] {
            let hz = pitch(&tone(&mut with(alone(0)), key, 20_000)) * RATE;
            let wanted = 523.251 * 2f64.powf(cents / 1200.0);
            assert!((hz / wanted - 1.0).abs() < 1e-4, "{hz} Hz");
        }
        // The fine pitch of the note and the oscillator's own knobs.
        let mut osc = with(knobs(&[(2, 7), (3, -50), (6, 0), (13, 0)]));
        osc.note_on(NoteOn {
            fine_pitch: 25.0,
            ..note(1, 60)
        });
        let hz = pitch(&play(&mut osc, 20_000).0) * RATE;
        assert!((hz / (523.251 * 2f64.powf(675.0 / 1200.0)) - 1.0).abs() < 1e-4);

        // The defaults: sines at 0, -12 and -24 semitones. Oscillator 3
        // has 32 of 128, and the other two share the rest evenly.
        let mut osc = ThreeOsc::default();
        assert_eq!(osc.params, DEFAULTS);
        let low = turns(-2400.0);
        let len = whole(low, 40_000);
        let sound = tone(&mut osc, 60, len);
        for (cents, level) in [(0.0, 0.375), (-1200.0, 0.375), (-2400.0, 0.25)] {
            let found = partial(&sound, turns(cents));
            assert!((found - level * 0.5).abs() < 2e-3, "{found} at {cents}");
        }
        let hz = pitch(&sound) * RATE;
        assert!(hz > 120.0, "{hz}");
    }

    #[test]
    fn the_shapes_have_the_harmonics_of_their_names() {
        let base = turns(0.0);
        let len = whole(base, 60_000);
        // The size of a harmonic against the first, in dB.
        let harmonic = |shape: i32, n: f64| {
            let sound = tone(&mut with(alone(shape)), 60, len);
            db(partial(&sound, base * n) / partial(&sound, base))
        };
        assert!(harmonic(0, 3.0) < -40.0 && harmonic(0, 2.0) < -40.0);
        // A square has a third of the first at the third, a triangle a
        // ninth, and neither has a second. A saw has half at the second.
        assert!(
            (harmonic(2, 3.0) + 9.54).abs() < 1.0,
            "{}",
            harmonic(2, 3.0)
        );
        assert!(
            (harmonic(1, 3.0) + 19.08).abs() < 1.0,
            "{}",
            harmonic(1, 3.0)
        );
        assert!(harmonic(2, 2.0) < -40.0 && harmonic(1, 2.0) < -40.0);
        assert!((harmonic(3, 2.0) + 6.02).abs() < 0.5 && (harmonic(3, 3.0) + 9.54).abs() < 0.5);
        // The rounded saw has every harmonic, and the custom shape is the
        // saw, which is not what FL plays for a channel with a sample.
        assert!(harmonic(4, 2.0) > -20.0 && harmonic(4, 3.0) > -20.0);
        let (mut custom, mut saw) = (with(alone(6)), with(alone(3)));
        assert_eq!(tone(&mut custom, 60, 500), tone(&mut saw, 60, 500));
        assert_eq!(
            custom.fidelity(),
            Fidelity::Approximate("3x Osc custom shape")
        );
        // A custom shape that is not heard is not counted.
        let quiet = with(knobs(&[(1, 0), (8, 6), (6, 0), (13, 0)]));
        assert_eq!(quiet.fidelity(), Fidelity::Exact);
        // Noise is no tone: it is as loud as a table of noise is.
        let noise = tone(&mut with(alone(5)), 60, 20_000);
        assert!((rms(&noise) / 0.5 - 3f64.sqrt().recip()).abs() < 0.03);
    }

    #[test]
    fn fl_s_tables_are_made_as_flengine_makes_them() {
        let tables = HostTables::get();
        let [sine, triangle, square, saw, rounded, noise] = &tables.shapes;
        assert!(tables.shapes.iter().all(|table| table.len() == TABLE));
        assert_eq!((sine[0], sine[4096], sine[12_288]), (0.0, 1.0, -1.0));
        assert_eq!(
            (triangle[0], triangle[4096], triangle[12_288]),
            (0.0, 1.0, -1.0)
        );
        assert_eq!(
            (triangle[2048], triangle[8192], triangle[14_336]),
            (0.5, 0.0, -0.5)
        );
        assert_eq!((square[0], square[8191], square[8192]), (1.0, 1.0, -1.0));
        assert_eq!((saw[0], saw[8192], saw[4096]), (-1.0, 0.0, -0.5));
        // 1 - 2 * 0.997^n up to 7600, 1 to five sixths of the way, then -1.
        assert_eq!(
            (rounded[0], rounded[7600], rounded[13_652]),
            (-1.0, 1.0, 1.0)
        );
        assert_eq!((rounded[13_653], rounded[16_383]), (-1.0, -1.0));
        for n in [1usize, 100, 231, 1000, 7599] {
            let wanted = 1.0 - 2.0 * 0.997f64.powi(n as i32);
            assert!((f64::from(rounded[n]) - wanted).abs() < 1e-4, "{n}");
            assert!((f64::from(shape_at(4, n as f32 / 16_384.0)) - wanted).abs() < 1e-6);
        }
        // The engine's shapes are the host's.
        for (shape, table) in [(1, triangle), (2, square), (3, saw)] {
            for n in (0..TABLE).step_by(97) {
                assert_eq!(
                    shape_at(shape, n as f32 / 16_384.0),
                    table[n],
                    "{shape} {n}"
                );
            }
        }
        // The noise is a Mersenne Twister's, from -1 to 1.
        assert_eq!(Twister::new(5489).next(), 3_499_211_612);
        assert!(noise.iter().all(|value| (-1.0..=1.0).contains(value)));
        let mean: f64 = noise.iter().map(|v| f64::from(*v)).sum::<f64>() / TABLE as f64;
        assert!(mean.abs() < 0.02 && (rms(noise) - 3f64.sqrt().recip()).abs() < 0.01);
    }

    #[test]
    fn the_right_side_has_its_own_pitch_and_phase() {
        // Stereo detune: the right side is that many cents higher.
        for cents in [50, -50, 13] {
            let mut osc = with(knobs(&[(5, cents), (6, 0), (13, 0)]));
            osc.note_on(note(1, 60));
            let (left, right) = play(&mut osc, 30_000);
            let ratio = pitch(&right) / pitch(&left);
            let wanted = 2f64.powf(f64::from(cents) / 1200.0);
            assert!((ratio / wanted - 1.0).abs() < 1e-4, "{ratio} for {cents}");
            assert!((pitch(&left) * RATE / 523.251 - 1.0).abs() < 1e-4);
        }
        // Stereo phase offset: 32 of 128 is a quarter of a turn ahead.
        let mut osc = with(knobs(&[(4, 32), (6, 0), (13, 0)]));
        osc.note_on(note(1, 60));
        let (left, right) = play(&mut osc, 100);
        assert_eq!(left[0], 0.0);
        assert!((right[0] - 0.5).abs() < 1e-3, "{}", right[0]);
        // With none of them the right side is the left.
        let mut osc = with(alone(3));
        osc.note_on(note(1, 60));
        let (left, right) = play(&mut osc, 100);
        assert_eq!(left, right);
        assert!(!osc.derived.stereo);
        // The oscillator's pan is the pan law's: all the way left.
        let mut osc = with(knobs(&[(0, -64), (6, 0), (13, 0)]));
        osc.note_on(note(1, 60));
        let (left, right) = play(&mut osc, 2000);
        assert!(rms(&right) < 1e-6 && (rms(&left) - 0.5).abs() < 0.01);

        // Phase randomness: each note starts somewhere else, by up to
        // 64 of 128 of a turn either way.
        let mut osc = with(knobs(&[(1, 3), (6, 0), (13, 0), (20, 64)]));
        assert!(osc.derived.stereo);
        let mut starts = Vec::new();
        for id in 0..8 {
            osc.reset();
            osc.note_on(note(id, 60));
            let phases = osc.voices[0].phases[0];
            assert_ne!(phases[0], phases[1]);
            starts.push(phases[0]);
        }
        starts.sort_unstable();
        starts.dedup();
        assert_eq!(starts.len(), 8);
        assert!(starts.iter().all(|phase| phase % 256 == 0));
        // A range of 1 is a 128th of a turn either way.
        let mut osc = with(knobs(&[(20, 1)]));
        for id in 0..8 {
            osc.note_on(note(id, 60));
            let phase = osc.voices[id as usize].phases[0][0] as i32;
            assert!(phase.unsigned_abs() <= 1 << 25, "{phase}");
        }
    }

    #[test]
    fn am_multiplies_the_other_two_by_one_less_oscillator_3() {
        // Oscillator 1 at full level, and oscillator 3 two octaves under
        // it at full depth.
        let params = knobs(&[(6, 0), (13, 128)]);
        let mut osc = ThreeOsc::default();
        osc.set_state(&state(params, [0, 0, 0, 1, 0], &new_tab()))
            .unwrap();
        assert_eq!(osc.flags(), ([false; 3], true, false));
        let levels = osc.derived.oscs.map(|osc| osc.level);
        assert_eq!(levels, [1.0, 0.0, 1.0]);
        let (carrier, low) = (turns(0.0), turns(-2400.0));
        let len = whole(low, 60_000);
        let sound = tone(&mut osc, 60, len);
        // The depth is oscillator 3's gain: its level through the pan law.
        let depth = std::f64::consts::FRAC_1_SQRT_2;
        assert!((partial(&sound, carrier) - 0.5).abs() < 2e-3);
        for side in [carrier + low, carrier - low] {
            let found = partial(&sound, side);
            assert!((found - 0.5 * depth / 2.0).abs() < 2e-3, "{found}");
        }
        // Oscillator 3 itself is not heard.
        assert!(partial(&sound, low) < 2e-3);
        // Without AM the three levels add up to 1, and an invert flag
        // turns its oscillator over.
        let mut osc = ThreeOsc::default();
        osc.set_state(&state(DEFAULTS, [0, 1, 0, 0, 0], &new_tab()))
            .unwrap();
        assert_eq!(osc.derived.oscs.map(|osc| osc.level), [0.375, -0.375, 0.25]);
        let half = with(knobs(&[(6, 128), (13, 64)]));
        assert_eq!(half.derived.oscs.map(|osc| osc.level), [0.0, 0.5, 0.5]);
    }

    /// The tab with these numbers moved: the block, the place and the
    /// value.
    fn tab(moved: &[(usize, usize, i32)]) -> Articulation {
        let mut tab = new_tab();
        for (block, place, value) in moved {
            tab.blocks[*block][*place] = *value;
        }
        tab
    }

    /// The loudest sample of each 256 of a sound.
    fn peaks(sound: &[f32]) -> Vec<f32> {
        let peak = |chunk: &[f32]| chunk.iter().fold(0.0f32, |peak, v| peak.max(v.abs()));
        sound.chunks(256).map(peak).collect()
    }

    #[test]
    fn the_attack_takes_the_time_curve_s_ticks() {
        // (1001^(v / 65536) - 1) * 0.064 seconds, at any tempo.
        assert_eq!(ticks(100, 12.288), 0);
        assert_eq!(ticks(65_536, 12.288), 12_288);
        assert_eq!(ticks(20_000, 12.288), 89);
        for (tempo, value) in [(140.0, 30_000), (90.0, 30_000), (140.0, 20_000)] {
            let seconds = (1001f64.powf(f64::from(value) / 65_536.0) - 1.0) * 0.064;
            let tab = tab(&[
                (VOLUME, ENABLED, -1),
                (VOLUME, ATTACK, value),
                (VOLUME, HOLD, 60_000),
            ]);
            let mut osc = with_tab(alone(0), &tab);
            osc.set_tempo(tempo);
            let len = (seconds * RATE * 1.3) as usize;
            let sound = peaks(&tone(&mut osc, 60, len));
            // The first time the level is over a half and over 0.99 of
            // the 0.5 it gets to.
            let reach = |level: f32| {
                let at = sound.iter().position(|peak| *peak >= 0.5 * level).unwrap();
                at as f64 * 256.0 / RATE
            };
            let (half, full) = (reach(0.5), reach(0.99));
            assert!(
                (half / (seconds * 0.5) - 1.0).abs() < 0.05,
                "{half} of {seconds}"
            );
            assert!(
                (full / (seconds * 0.99) - 1.0).abs() < 0.05,
                "{full} of {seconds}"
            );
            assert!((sound[sound.len() - 2] - 0.5).abs() < 0.01);
        }
        // A block whose flag is set counts ticks: the same ticks at 70
        // beats a minute are twice as long as at 140.
        let fixed = |tempo: f64| {
            let tab = tab(&[
                (VOLUME, FLAGS, 5),
                (VOLUME, ENABLED, -1),
                (VOLUME, ATTACK, 20_000),
            ]);
            let mut osc = with_tab(alone(0), &tab);
            osc.set_tempo(tempo);
            (osc.derived.blocks[VOLUME].attack, osc.tick_len())
        };
        assert_eq!(fixed(140.0).0, fixed(70.0).0);
        assert_eq!(fixed(140.0).0, 1.0 / 90.0);
        assert_eq!(fixed(70.0).1, 2.0 * fixed(140.0).1);
        assert_eq!(fixed(140.0).1, 196.875);
    }

    #[test]
    fn the_ppq_sets_the_ticks_of_an_attack() {
        // The unit of the time curve is 0.128 ticks a PPQ, so an attack
        // has half the ticks at 48 that it has at 96, each twice as long:
        // the same seconds, to the nearest tick. That holds whether or
        // not the block's times follow the tempo.
        for flags in [0, 5] {
            let at = |ppq: Option<u16>| {
                let tab = tab(&[
                    (VOLUME, FLAGS, flags),
                    (VOLUME, ENABLED, -1),
                    (VOLUME, ATTACK, 40_000),
                ]);
                let mut osc = with_tab(alone(0), &tab);
                osc.set_tempo(120.0);
                if let Some(ppq) = ppq {
                    osc.set_ppq(ppq);
                }
                let ticks = (1.0 / f64::from(osc.derived.blocks[VOLUME].attack)).round() - 1.0;
                (ticks as i32, osc.tick_len())
            };
            let (coarse, fine) = (at(Some(48)), at(Some(96)));
            assert_eq!(fine, (ticks(40_000, 12.288), 229.6875));
            assert_eq!(coarse, (ticks(40_000, 6.144), 459.375));
            assert_eq!((fine.0, coarse.0), (821, 411));
            // A plugin that is not told counts 96 to a beat, and a PPQ of
            // 0 leaves it as it was.
            assert_eq!(at(None), fine);
            assert_eq!(at(Some(0)), fine);
        }
    }

    #[test]
    fn an_envelope_goes_through_its_stages_a_tick_at_a_time() {
        let shapes = HostTables::get();
        let raw = [
            0, -1, 5000, 9000, 8000, 12_000, 64, 10_000, 0, 100, 100, 0, 32_950, 0, 0, 0, 0,
        ];
        let block = Block::of(&raw, 120.0, PPQ);
        let count = |value: i32| ticks(value, 12.288);
        assert_eq!((block.predelay, block.hold), (count(5000), count(8000)));
        assert_eq!(block.attack, 1.0 / (count(9000) + 1) as f32);
        assert_eq!(block.decay, -1.0 / (count(12_000) + 1) as f32);
        assert_eq!(block.sustain, 0.5);
        let mut envelope = Envelope::new(&block);
        let mut stages = vec![(envelope.stage, 0)];
        let mut levels = Vec::new();
        for tick in 1..200 {
            envelope.step(&block, 0, shapes);
            levels.push(envelope.level);
            if stages.last().unwrap().0 != envelope.stage {
                stages.push((envelope.stage, tick));
            }
        }
        // The wait, the rise in equal steps to 1, the hold, the fall to
        // the sustain level.
        let rise = count(5000) + count(9000) + 1;
        let fall = rise + count(8000);
        let sustain = fall + (count(12_000) + 1 + 1) / 2;
        assert_eq!(
            stages,
            [
                (WAIT, 0),
                (RISE, count(5000)),
                (HELD, rise),
                (FALL, fall),
                (SUSTAINED, sustain)
            ]
        );
        assert_eq!(levels[count(5000) as usize], block.attack);
        assert_eq!((levels[rise as usize], levels[199 - 1]), (1.0, 0.5));
        // The release starts from where the envelope is and ends at 0.
        envelope.release(&block);
        let mut left = 0;
        while envelope.stage != OVER {
            envelope.step(&block, 0, shapes);
            left += 1;
        }
        assert_eq!(left, (count(10_000) + 1 + 1) / 2);
        assert_eq!(envelope.level, 0.0);

        // A tension bends a stretch between the same ends, and what it
        // makes of a level is undone for the release.
        for knob in [-128, -101, -40, 37, 128] {
            let tension = Tension::of(knob);
            let k = (6347f64.powf(f64::from(knob.abs()) / 128.0) - 1.0) * 0.1;
            assert!((f64::from(tension.k.abs()) / k - 1.0).abs() < 1e-5);
            assert_eq!(tension.k > 0.0, knob > 0);
            assert!(tension.shape(0.0).abs() < 1e-5 && (tension.shape(1.0) - 1.0).abs() < 1e-5);
            let middle = f64::from(tension.shape(0.5));
            let wanted = match knob > 0 {
                true => 1.0 - ((0.5 * (k + 1.0).ln()).exp() - 1.0) / k,
                false => ((0.5 * (k + 1.0).ln()).exp() - 1.0) / k,
            };
            assert!((middle - wanted).abs() < 1e-5, "{middle} for {knob}");
            assert_eq!(middle > 0.5, knob > 0);
            for value in [0.1f32, 0.5, 0.9] {
                let back = tension.unshape(tension.shape(value));
                assert!((back - value).abs() < 1e-3, "{back} for {value} at {knob}");
            }
        }
        assert_eq!(Tension::of(0).shape(0.3), 0.3);

        // An envelope that is off holds 0, or 1 for the volume's flag,
        // and one with no attack starts at 1.
        let off = Block::of(&new_tab().blocks[0], 140.0, PPQ);
        let volume = Block::of(&new_tab().blocks[VOLUME], 140.0, PPQ);
        assert_eq!(
            (Envelope::new(&off).level, Envelope::new(&volume).level),
            (0.0, 1.0)
        );
        let mut sudden = raw;
        (sudden[PREDELAY], sudden[ATTACK]) = (100, 100);
        let envelope = Envelope::new(&Block::of(&sudden, 120.0, PPQ));
        assert_eq!((envelope.level, envelope.stage), (1.0, HELD));
        // The LFO turns once in the ticks of its speed, no fewer than 3.
        assert_eq!(
            volume.lfo_step,
            (0x1_0000_0000u64 / ticks(32_950, 14.336) as u64) as u32
        );
        let mut fast = raw;
        fast[LFO_SPEED] = 100;
        assert_eq!(Block::of(&fast, 120.0, PPQ).lfo_step, 0x5555_5555);
    }

    #[test]
    fn the_pitch_envelope_and_the_lfos_move_what_they_are_on() {
        // An octave of pitch envelope with no attack: the note starts an
        // octave up and holds there.
        let moved = tab(&[
            (PITCH, ENABLED, -1),
            (PITCH, ATTACK, 100),
            (PITCH, AMOUNT, 1200),
        ]);
        let sound = tone(&mut with_tab(alone(0), &moved), 60, 8000);
        let hz = pitch(&sound) * RATE;
        assert!((hz / 1046.502 - 1.0).abs() < 1e-3, "{hz} Hz");
        // With the block off its amount does nothing.
        let moved = tab(&[(PITCH, AMOUNT, 1200)]);
        let hz = pitch(&tone(&mut with_tab(alone(0), &moved), 60, 8000)) * RATE;
        assert!((hz / 523.251 - 1.0).abs() < 1e-3, "{hz} Hz");

        // A square LFO on the volume that turns once in 104 ticks: half
        // as much again for the first half of a turn, half for the second.
        let wobble = tab(&[
            (VOLUME, LFO_AMOUNT, 64),
            (VOLUME, LFO_SPEED, 20_000),
            (VOLUME, LFO_SHAPE, 2),
            (VOLUME, LFO_PREDELAY, 100),
            (VOLUME, LFO_ATTACK, 100),
        ]);
        let mut osc = with_tab(alone(2), &wobble);
        assert_eq!(ticks(20_000, 14.336), 104);
        osc.note_on(note(1, 60));
        let mut levels = Vec::new();
        for _ in 0..4 {
            // A quarter of a turn on, then half a turn each time.
            let quarters = if levels.is_empty() { 1.0 } else { 2.0 };
            play(&mut osc, (26.0 * 196.875 * quarters) as usize);
            levels.push(osc.voices[0].gain_ends[0] / std::f32::consts::FRAC_1_SQRT_2);
        }
        assert_eq!(levels, [1.5, 0.5, 1.5, 0.5]);
        // The gain gets there by no more than 0.001 a sample: 0.197 a
        // tick, where the LFO asks for 0.7071 at once.
        let mut osc = with_tab(alone(2), &wobble);
        osc.note_on(note(1, 60));
        play(&mut osc, (52.5 * 196.875) as usize);
        let fell = 1.5 * std::f32::consts::FRAC_1_SQRT_2 - osc.voices[0].gain_ends[0];
        assert!((fell - 0.197).abs() < 0.002, "{fell}");
        // A sine LFO on the pan moves the two sides against each other.
        let swing = tab(&[(0, LFO_AMOUNT, 64), (0, LFO_SPEED, 20_000)]);
        let mut osc = with_tab(alone(0), &swing);
        osc.note_on(note(1, 60));
        let (left, right) = play(&mut osc, 60_000);
        let (left, right) = (peaks(&left), peaks(&right));
        let widest = left.iter().zip(&right).map(|(l, r)| (l - r).abs());
        assert!(widest.fold(0.0, f32::max) > 0.5);
    }

    #[test]
    fn every_filter_type_takes_away_what_it_is_named_for() {
        // A sine two octaves up, at 2093 Hz, and what is left of it.
        let left = |filter: i32, cutoff: i32, resonance: i32, coarse: i32| {
            let mut tab = new_tab();
            (tab.filter, tab.cutoff, tab.resonance) = (filter, cutoff, resonance);
            let params = knobs(&[(2, coarse), (6, 0), (13, 0)]);
            let sound = tone(&mut with_tab(params, &tab), 60, 30_000);
            rms(&sound[20_000..]) / (0.5 * std::f64::consts::FRAC_1_SQRT_2)
        };
        // The low-passes with the cutoff a tenth of the way up.
        for filter in [0, 1, 5, 6, 7] {
            let low = left(filter, 26, 0, 24);
            assert!(db(low) < -12.0, "type {filter}: {} dB", db(low));
            let open = left(filter, 256, 0, 24);
            assert!(db(open) > -7.0, "type {filter}: {} dB", db(open));
        }
        // Twice takes away more than once.
        assert!(left(5, 26, 0, 24) < left(1, 26, 0, 24) * 0.2);
        assert!(left(7, 26, 0, 24) < left(6, 26, 0, 24) * 0.2);
        // A narrow band-pass far below the tone.
        assert!(db(left(2, 26, 256, 24)) < -12.0);
        // The high-pass at the top, under a tone two octaves down.
        assert!(db(left(3, 256, 0, -24)) < -12.0);
        assert!(db(left(3, 0, 0, 24)) > -3.0);
        // The notch on the tone: w = 0.2984 at a cutoff of 113.
        assert!(db(left(4, 113, 0, 24)) < -30.0);
        assert!(db(left(4, 26, 256, 24)) > -1.0);
        // A new plugin's Fast LP is wide open and is not run.
        let mut osc = with(alone(3));
        osc.note_on(note(1, 60));
        play(&mut osc, 10);
        assert!(!osc.voices[0].filter.active);
        // The note's mod X moves the cutoff, as the channel's knob does.
        let mut osc = with(alone(0));
        osc.note_on(NoteOn {
            mod_x: -0.9,
            ..note(1, 84)
        });
        let sound = play(&mut osc, 30_000).0;
        assert!(db(rms(&sound[20_000..]) / 0.3536) < -12.0);

        // The doc's formulas.
        let near = |found: f32, wanted: f64| (f64::from(found) - wanted).abs() < 1e-5;
        let (f, fb) = fast_lp(0.5, 0.5, 1.0);
        assert!(near(f, (601f64.powf(0.5) - 1.0) / 600.0));
        assert!(near(fb, 1.0 - (601f64.powf(0.6875) - 1.0) / 600.0));
        assert_eq!(
            (fast_lp(0.0, 0.0, 1.0), fast_lp(1.0, 0.0, 1.0)),
            ((0.0, 0.0), (1.0, 0.0))
        );
        assert!(near(
            fast_lp(0.9, 2.0, 1.0).1,
            1.0 - (601f64.powf(0.375) - 1.0) / 600.0
        ));
        assert!(near(
            fast_lp(0.5, 0.0, 0.5).0,
            (601f64.powf(0.5) - 1.0) / 1200.0
        ));
        let (w, q) = biquad_wq(0.5, 0.5, 1.0);
        assert!(near(w, (51f64.powf(0.5) - 1.0) * 0.0596 + 0.02));
        assert!(near(q, (51f64.powf(0.3125) - 1.0) * 0.4 + 0.1));
        assert_eq!(biquad_wq(0.0, 0.0, 1.0), (0.02, 0.1));
        assert_eq!(biquad_wq(1.0, 0.0, 1.0).0, 3.0);
        // The Q runs from 0.1 to 4.37.
        let top = (51f64.powf(0.625) - 1.0) * 0.4 + 0.1;
        assert!(near(biquad_wq(1.0, 1.0, 1.0).1, top) && (top - 4.37).abs() < 0.01);
        assert!(near(biquad_wq(0.0, 0.0, 0.5).0, 0.01));
        let (f, q) = svf(0.5, 0.5, 1.0);
        assert!(near(f, (26f64.sqrt() - 1.0) * 0.04) && near(q, 0.6));
        assert_eq!(
            (svf(0.0, 0.0, 1.0), svf(1.0, 1.0, 1.0)),
            ((0.0, 1.0), (1.0, 0.2))
        );
        // The low-pass of the cookbook.
        let (sin, cos) = 0.3f64.sin_cos();
        let alpha = sin / 1.4;
        let [b0, b1, b2, a1, a2] = biquad(1, 0.3, 0.7);
        assert!(near(b1, (1.0 - cos) / (1.0 + alpha)) && b0 == b1 * 0.5 && b2 == b0);
        assert!(near(a1, -2.0 * cos / (1.0 + alpha)) && near(a2, (1.0 - alpha) / (1.0 + alpha)));
        let [b0, b1, b2, ..] = biquad(2, 0.3, 0.7);
        assert!(near(b0, alpha / (1.0 + alpha)) && b1 == 0.0 && b2 == -b0);
        let [b0, b1, b2, ..] = biquad(3, 0.3, 0.7);
        assert!(near(b1, -(1.0 + cos) / (1.0 + alpha)) && b0 == -b1 * 0.5 && b2 == b0);
        let [b0, b1, b2, a1, _] = biquad(4, 0.3, 0.7);
        assert!(near(b0, 1.0 / (1.0 + alpha)) && b1 == a1 && b2 == b0);
        // A value gets to its target in a straight line, no faster than
        // the limit.
        assert_eq!(ramp(1.0, 0.0, 0.001, 2000), (0.0005, 1.0));
        assert_eq!(ramp(1.0, 0.0, 0.001, 100), (0.001, 0.1));
        assert_eq!(ramp(0.0, 1.0, 0.001, 100), (-0.001, 0.9));
        assert_eq!(ramp(0.25, 0.25, 0.001, 100), (0.0, 0.25));
    }

    #[test]
    fn every_length_of_state_loads_and_no_other() {
        let params: [i32; 21] = [
            -4, 6, 7, -3, 5, -7, 65, 6, 3, 12, 13, -50, 5, 72, -8, 1, -12, 3, -15, -7, 33,
        ];
        let words = |version: i32, count: usize| {
            testing::state(&[&[version][..], &params[..count]].concat())
        };
        let mut osc = ThreeOsc::default();
        // 44 bytes: three oscillators' shape, coarse and fine, with the
        // mix levels between them, and no version.
        osc.set_state(&testing::state(&[6, 7, -3, 65, 3, 12, 13, 72, 1, -12, 3]))
            .unwrap();
        assert_eq!(
            osc.params,
            [
                0, 6, 7, -3, 0, 0, 65, 0, 3, 12, 13, 0, 0, 72, 0, 1, -12, 3, 0, 0, 0
            ]
        );
        // Version 10: 20 parameters. 11: and the four flags.
        let mut twenty = params;
        twenty[20] = 0;
        osc.set_state(&words(10, 20)).unwrap();
        assert_eq!(
            (osc.params, osc.flags()),
            (twenty, ([false; 3], false, false))
        );
        osc.set_state(&[words(11, 20), vec![1, 0, 1, 1]].concat())
            .unwrap();
        assert_eq!(
            (osc.params, osc.flags()),
            (twenty, ([true, false, true], true, false))
        );
        // 12: parameter 20 too. 13: and the HQ flag.
        osc.set_state(&[words(12, 21), vec![0, 1, 0, 0]].concat())
            .unwrap();
        assert_eq!(
            (osc.params, osc.flags()),
            (params, ([false, true, false], false, false))
        );
        assert_eq!(osc.articulation(), &new_tab());
        // 14: and the tab.
        let mut tab = tab(&[
            (VOLUME, ENABLED, -1),
            (PITCH, AMOUNT, -700),
            (0, LFO_SHAPE, 2),
        ]);
        (tab.cutoff, tab.resonance, tab.filter) = (26, 168, 7);
        osc.set_state(&state(params, [0, 0, 1, 0, 0], &tab))
            .unwrap();
        assert_eq!(
            (osc.params, osc.flags()),
            (params, ([false, false, true], false, false))
        );
        assert_eq!(osc.articulation(), &tab);
        assert_eq!(osc.derived.filter, (Kind::Svf, true));
        // An older state after it has a new plugin's tab again.
        osc.set_state(&words(10, 20)).unwrap();
        assert_eq!(osc.articulation(), &new_tab());
        assert_eq!(osc.derived.oscs[1].cents, 1213);
        assert_eq!(osc.derived.oscs[1].offset, (-50i32 * 0x1ff_ffff) as u32);

        let before = osc.clone();
        let length = |expected, found| Err(StateError::Length { expected, found });
        assert_eq!(osc.set_state(&[1, 2, 3]), length(460, 3));
        assert_eq!(osc.set_state(&words(10, 21)), length(84, 88));
        assert_eq!(osc.set_state(&words(11, 20)), length(88, 84));
        assert_eq!(osc.set_state(&words(12, 20)), length(92, 84));
        assert_eq!(
            osc.set_state(&[words(13, 21), vec![0; 4]].concat()),
            length(93, 92)
        );
        assert_eq!(
            osc.set_state(&[words(14, 21), vec![0; 4]].concat()),
            length(460, 92)
        );
        assert_eq!(osc.set_state(&testing::state(&[3; 12])), length(44, 48));
        for version in [15, i32::MAX] {
            let found = osc.set_state(&[words(version, 21), vec![0; 4]].concat());
            assert_eq!(found, Err(StateError::Version { found: version }));
        }
        // A shape and a filter type there is none of.
        let mut wrong = params;
        wrong[8] = 7;
        let found = osc.set_state(&state(wrong, [0; 5], &new_tab()));
        assert_eq!(found, Err(StateError::Range { param: 8, value: 7 }));
        tab.filter = 8;
        let found = osc.set_state(&state(params, [0; 5], &tab));
        assert_eq!(
            found,
            Err(StateError::Range {
                param: 114,
                value: 8
            })
        );
        assert_eq!(osc, before);
        // FL's own presets go past the wheels: Accordeon 2 has 256 of
        // phase randomness.
        let mut far = params;
        far[20] = 256;
        osc.set_state(&state(far, [0; 5], &new_tab())).unwrap();
        assert_eq!(osc.derived.random, 256 << 18);
    }

    #[test]
    fn a_note_off_fades_in_10_ms_or_runs_the_release() {
        // A new plugin has no volume envelope.
        let mut osc = with(alone(2));
        assert_eq!(osc.fade.len(), 441);
        assert_eq!(osc.tail(), 197 + 441 + 1);
        osc.note_on(note(7, 60));
        let held = play(&mut osc, 1000).0;
        assert!((held[999].abs() - 0.5).abs() < 1e-6);
        osc.note_off(3);
        assert!(osc.voices[0].fade.is_none());
        osc.note_off(7);
        let faded = play(&mut osc, 600).0;
        // Half a cosine down: half way after half the time.
        assert!((faded[219].abs() - 0.25).abs() < 0.01, "{}", faded[219]);
        assert!(faded[100].abs() > faded[300].abs() && faded[440].abs() < 1e-4);
        assert_eq!(faded[441..], [0.0; 159]);
        assert!(osc.voices.is_empty());

        // With the envelope on, the release takes its ticks from the
        // sustain level down, and the note is over inside the tail.
        let moved = tab(&[
            (VOLUME, ENABLED, -1),
            (VOLUME, ATTACK, 100),
            (VOLUME, HOLD, 100),
            (VOLUME, DECAY, 100),
            (VOLUME, SUSTAIN, 128),
            (VOLUME, RELEASE, 20_000),
            (VOLUME, TENSIONS + 2, 0),
        ]);
        let mut osc = with_tab(alone(2), &moved);
        let release = ticks(20_000, 14.336) + 1;
        assert_eq!(release, 105);
        let tail = osc.tail();
        assert_eq!(tail, (107.0f64 * 196.875).ceil() as usize + 442);
        osc.note_on(note(1, 60));
        play(&mut osc, 3000);
        osc.all_notes_off();
        let sound = peaks(&play(&mut osc, tail).0);
        assert!(osc.voices.is_empty());
        // Half way down after half the release.
        let half = (f64::from(release) * 196.875 / 2.0 / 256.0) as usize;
        assert!((sound[half] - 0.25).abs() < 0.02, "{}", sound[half]);
        assert!(sound[half / 2] > 0.33 && sound[half * 3 / 2] < 0.17);
        // A reset cuts what sounds.
        osc.note_on(note(1, 60));
        osc.reset();
        assert_eq!(play(&mut osc, 4).0, [0.0; 4]);
    }

    #[test]
    fn velocity_and_pan_are_the_note_s_gains_and_notes_add_up() {
        let one = tone(&mut with(alone(3)), 60, 300);
        let mut osc = with(alone(3));
        osc.note_on(NoteOn {
            velocity: 0.5,
            pan: -1.0,
            ..note(1, 60)
        });
        let (left, right) = play(&mut osc, 300);
        // All the way left is the whole gain on the left: 1 where the
        // centre has 0.7071.
        for n in 0..300 {
            let wanted = one[n] * 0.5 * std::f32::consts::SQRT_2;
            assert!((left[n] - wanted).abs() < 1e-6 && right[n].abs() < 1e-7);
        }
        assert_eq!(pan_law(0.0, 2.0)[0], pan_law(0.0, 2.0)[1]);
        assert!((pan_law(0.0, 2.0)[0] - std::f32::consts::SQRT_2).abs() < 1e-6);
        assert!((pan_law(5.0, 1.0)[1] - 1.0).abs() < 1e-6 && pan_law(1.0, 1.0)[0].abs() < 1e-7);
        // A second note 300 samples behind the first.
        let mut osc = with(alone(3));
        let mut first = tone(&mut osc, 60, 300);
        osc.note_on(note(2, 60));
        first.extend(play(&mut osc, 300).0);
        let alone = tone(&mut with(alone(3)), 60, 600);
        for n in 0..300 {
            assert!((first[300 + n] - (alone[300 + n] + alone[n])).abs() < 1e-6);
        }
        // A 65th note takes the place of the first.
        let mut osc = ThreeOsc::default();
        let room = osc.voices.capacity();
        for id in 0..=VOICES as u32 {
            osc.note_on(note(id, 60));
        }
        assert_eq!((osc.voices.len(), osc.voices[0].id), (VOICES, 1));
        assert_eq!(osc.voices.capacity(), room);
    }

    #[test]
    fn the_blocks_can_fall_anywhere() {
        let mut moved = tab(&[
            (VOLUME, ENABLED, -1),
            (VOLUME, ATTACK, 9000),
            (MOD_X, ENABLED, -1),
            (MOD_X, AMOUNT, -100),
            (0, LFO_AMOUNT, 50),
            (PITCH, LFO_AMOUNT, 30),
        ]);
        (moved.cutoff, moved.resonance) = (200, 100);
        let params = knobs(&[(1, 3), (5, 13), (8, 5), (15, 2), (20, 20)]);
        let start = |filter: i32| {
            let mut tab = moved;
            tab.filter = filter;
            let mut osc = with_tab(params, &tab);
            osc.set_sample_rate(48_000.0);
            osc.note_on(note(1, 57));
            osc
        };
        for filter in [0, 5, 7] {
            let mut osc = start(filter);
            let (whole, _) = play(&mut osc, 3000);
            osc.note_off(1);
            let (rest, _) = play(&mut osc, 1500);
            assert!(rms(&whole) > 0.01 && whole.iter().all(|v| v.is_finite()));
            for block in [1, 64, 197, 1000] {
                let mut osc = start(filter);
                let mut sound = Vec::new();
                while sound.len() < 4500 {
                    if sound.len() == 3000 {
                        osc.note_off(1);
                    }
                    let to_off = 3000usize.saturating_sub(sound.len());
                    let len = block.min(4500 - sound.len());
                    let len = if to_off > 0 { len.min(to_off) } else { len };
                    sound.extend(play(&mut osc, len).0);
                }
                let wanted = [&whole[..], &rest[..]].concat();
                assert_eq!(
                    testing::bits(&sound),
                    testing::bits(&wanted),
                    "{filter} {block}"
                );
            }
        }
    }

    #[test]
    fn hq_reads_tables_cut_down_to_the_pitch_and_says_it_is_not_confirmed() {
        // A saw three octaves up, at 4186 Hz. Its sixth harmonic is over
        // half the rate and comes back down at 18984 Hz.
        let saw = |hq: u8| {
            let mut osc = ThreeOsc::default();
            let params = knobs(&[(1, 3), (6, 0), (13, 0)]);
            osc.set_state(&state(params, [0, 0, 0, 0, hq], &new_tab()))
                .unwrap();
            osc
        };
        let (mut plain, mut hq) = (saw(0), saw(1));
        assert_eq!(plain.fidelity(), Fidelity::Exact);
        assert_eq!(hq.fidelity(), Fidelity::Approximate("3x Osc HQ tables"));
        assert_eq!(hq.flags(), ([false; 3], false, true));
        let base = turns(3600.0);
        let len = whole(base, 40_000);
        let (plain, hq) = (tone(&mut plain, 96, len), tone(&mut hq, 96, len));
        let folded = 1.0 - 6.0 * base;
        assert!(
            partial(&plain, folded) > 0.03,
            "{}",
            partial(&plain, folded)
        );
        assert!(partial(&hq, folded) < 0.003, "{}", partial(&hq, folded));
        // The harmonics that fit are as loud in both.
        for n in [1.0, 2.0, 4.0] {
            let (a, b) = (partial(&plain, base * n), partial(&hq, base * n));
            assert!((b / a - 1.0).abs() < 0.1, "{b} for {a} at {n}");
        }
        // The pitch is the same, and a sine is a sine.
        let mut osc = saw(1);
        let hz = pitch(&tone(&mut osc, 60, 20_000)) * RATE;
        assert!((hz / 523.251 - 1.0).abs() < 0.005, "{hz} Hz");
        let mut osc = ThreeOsc::default();
        osc.set_state(&state(alone(0), [0, 0, 0, 0, 1], &new_tab()))
            .unwrap();
        let sound = tone(&mut osc, 60, whole(turns(0.0), 40_000));
        assert!((partial(&sound, turns(0.0)) - 0.5).abs() < 1e-3);
        // The tables: 44 levels of four shapes, the lowest with all its
        // harmonics and level 40, for 20 kHz, with one.
        let mips = MipTables::get();
        assert_eq!(mips.levels.len(), 176);
        assert!(
            mips.levels
                .iter()
                .all(|table| table.len() == MIP_POINTS + 2)
        );
        let (full, top) = (mips.level(2, 0), mips.level(2, 40));
        assert!((full[8192] - 1.0).abs() < 0.1 && (full[24_576] + 1.0).abs() < 0.1);
        let third = partial(&top[..MIP_POINTS], 3.0 / MIP_POINTS as f64);
        let first = partial(&top[..MIP_POINTS], 1.0 / MIP_POINTS as f64);
        assert!(
            (first - 4.0 / PI).abs() < 0.01 && third < 1e-6,
            "{first} {third}"
        );
    }

    #[test]
    fn the_parameters_are_the_21_wheels_inside_their_ranges() {
        let mut osc = crate::generator_by_name("3x osc").unwrap();
        assert_eq!(osc.param_count(), 114);
        let params: Vec<f32> = (0..22).map(|index| osc.param(index)).collect();
        let defaults: Vec<f32> = DEFAULTS.iter().map(|value| *value as f32).collect();
        assert_eq!(params[..21], defaults[..]);
        assert_eq!(params[21], 0.0);
        osc.set_param(1, 9.0);
        osc.set_param(2, -99.0);
        osc.set_param(20, 12.4);
        osc.set_param(21, 5.0);
        assert_eq!(
            (osc.param(1), osc.param(2), osc.param(20)),
            (6.0, -24.0, 12.0)
        );
        // A wheel is heard by the notes that sound, from the next tick.
        let mut osc = with(alone(0));
        osc.note_on(note(1, 60));
        play(&mut osc, 4000);
        osc.set_param(2, 12.0);
        play(&mut osc, 400);
        let hz = pitch(&play(&mut osc, 8000).0) * RATE;
        assert!((hz / 1046.502 - 1.0).abs() < 1e-3, "{hz} Hz");
    }

    #[test]
    fn the_instrument_tab_s_parameters_follow_from_23() {
        let mut osc = ThreeOsc::default();
        let tab = new_tab();
        // The blocks, 17 words apiece, then the filter.
        let word = |block: usize, word: usize| TAB_PARAM + 17 * block + word;
        assert_eq!(
            osc.param(word(VOLUME, ATTACK)),
            tab.blocks[VOLUME][ATTACK] as f32
        );
        assert_eq!(osc.param(word(PITCH, TENSIONS + 2)), 0.0);
        assert_eq!(osc.param(word(VOLUME, TENSIONS + 2)), -101.0);
        assert_eq!(
            (osc.param(110), osc.param(111), osc.param(113)),
            (256.0, 0.0, 0.0)
        );
        // Each to its wheel's range, and the amounts to their block's.
        for (index, value, kept) in [
            (word(0, PREDELAY), 0.0, 100.0),
            (word(0, LFO_SPEED), 0.0, 200.0),
            (word(MOD_X, SUSTAIN), 999.0, 128.0),
            (word(0, AMOUNT), -999.0, -64.0),
            (word(PITCH, AMOUNT), 9999.0, 2400.0),
            (word(PITCH, LFO_AMOUNT), -9999.0, -2400.0),
            (word(MOD_Y, LFO_SHAPE), 5.0, 2.0),
            (word(0, ENABLED), 3.0, 1.0),
            (110, 300.0, 256.0),
            (111, -1.0, 0.0),
            (113, 9.0, 7.0),
        ] {
            osc.set_param(index, value);
            assert_eq!(osc.param(index), kept, "{index}");
        }
        assert_eq!(osc.tab.blocks[PITCH][AMOUNT], 2400);
        assert_eq!(
            (osc.tab.cutoff, osc.tab.resonance, osc.tab.filter),
            (256, 0, 7)
        );
        // FL keeps nothing for these.
        for index in [21, 22, 108, 109, 112, 114] {
            osc.set_param(index, 50.0);
            assert_eq!(osc.param(index), 0.0, "{index}");
        }

        // The cutoff is heard by a note that sounds.
        let mut osc = with(alone(3));
        osc.note_on(note(1, 72));
        play(&mut osc, 4000);
        let open = rms(&play(&mut osc, 4000).0);
        osc.set_param(TAB_CUTOFF, 20.0);
        play(&mut osc, 2000);
        let shut = rms(&play(&mut osc, 4000).0);
        assert!(shut < open * 0.3, "{shut} {open}");
    }

    #[test]
    fn every_preset_and_every_state_in_fl_s_projects_loads() {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, 3x Osc's states skipped");
            return;
        };
        let presets = testing::every_preset(&root, NAME);
        let ours = |saved: &testing::Saved| saved.plugin.eq_ignore_ascii_case(NAME);
        let projects: Vec<_> = testing::projects(&root).into_iter().filter(ours).collect();
        assert_eq!((presets.len(), projects.len()), (181, 152));
        let mut lengths = std::collections::BTreeMap::new();
        let mut fidelity = std::collections::BTreeMap::new();
        for saved in presets.iter().chain(&projects) {
            let mut osc = crate::generator_by_name(&saved.plugin).unwrap();
            osc.set_sample_rate(48_000.0);
            osc.set_tempo(128.0);
            if let Err(error) = osc.set_state(&saved.state) {
                panic!("{}: {error}", saved.file);
            }
            // A state with a version has the parameters after it.
            if saved.state.len() >= 84 {
                for index in 0..20 {
                    let at = 4 + 4 * index;
                    let word = i32::from_le_bytes(saved.state[at..at + 4].try_into().unwrap());
                    assert_eq!(osc.param(index), word as f32, "{}", saved.file);
                }
            }
            for (id, key) in [(1, 48), (2, 60), (3, 67)] {
                osc.note_on(note(id, key));
            }
            let (mut left, mut right) = (vec![0.0; 4096], vec![0.0; 4096]);
            osc.process(&mut left, &mut right);
            osc.all_notes_off();
            osc.process(&mut left, &mut right);
            let sound = left.iter().chain(&right);
            assert!(
                sound.clone().all(|v| v.is_finite() && v.abs() < 16.0),
                "{}",
                saved.file
            );
            *lengths.entry(saved.state.len()).or_insert(0) += 1;
            let said = match osc.fidelity() {
                Fidelity::Exact => "exact",
                Fidelity::Approximate(what) => what,
            };
            *fidelity.entry(said).or_insert(0) += 1;
        }
        assert_eq!(
            lengths.into_iter().collect::<Vec<_>>(),
            [(44, 1), (84, 82), (88, 70), (92, 121), (93, 2), (460, 57)]
        );
        assert_eq!(fidelity.values().sum::<i32>(), 333);
        assert_eq!(fidelity.get("3x Osc HQ tables"), Some(&13));
    }
}
