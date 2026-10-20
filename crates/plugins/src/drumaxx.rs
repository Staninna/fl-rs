//! Drumaxx: sixteen of Drumpad's struck membranes with a mixer, cut groups
//! and a pattern sequencer, as `docs/plugins/drumaxx.md` reads them out of
//! the `engine_x64.dll` beside the plugin's `Drumaxx_x64.dll`. Each pad is
//! one [`Voice`] of [`crate::drumpad_voice`], whose arithmetic the two
//! engines share instruction for instruction; this is `DMSynth` around
//! them.
//!
//! # The state
//!
//! `SSLF`, the chunk's length as a u32, then the chunk: the magic `DMS4`
//! (`4SMD` on disk; `DMS3` to `DMS5` and the demo's `DMD3` to `DMD5` read
//! alike), 256 bytes of kit name when the chunk is longer than 260, the 720
//! parameters as f32, sixteen patterns of 0x402 bytes, sixteen pad names of
//! 128 bytes, a cut group byte for each pad and an output track for each
//! pad as a 7-bit varint. A `DMS3` chunk's Retrigger parameters are a third
//! of what they were.
//!
//! Pad `n`'s parameter `p` is parameter `44 n + p`:
//!
//! | `p` | what |
//! |---|---|
//! | 0 | volume: a gain of `2 v^2` |
//! | 1 | pan: `1 - v` of the gain left, `v` right |
//! | 2, 3 | mute, solo: not 0 is on |
//! | 4 | the key that strikes the pad, as a number |
//! | 5 to 42 | Drumpad's 38 parameters, `p - 5` there |
//! | 43 | not read |
//!
//! Drumpad's parameters 0 and 1, Noise repeat and Retrigger, are read as
//! parameters 5 and 6 of the plugin for every pad: pad 0's.
//!
//! The 16 that follow the pads: 705 the master volume, `10^(1.6 v) / 4`,
//! halved once more at the output; 706 the swing; 707 the EQ and 708 to 715
//! its eight bands, `16 (v - 0.5)` dB each; 716 the limiter, 717 its drive
//! `10^(1.5 v)` and 718 its release `3 v^2 + 2` a second. 704 and 719 are
//! not read.
//!
//! # Notes and patterns
//!
//! A note on a pattern's key starts the pattern from its first step, and a
//! note off on that key stops it. A note strikes every pad whose key it
//! is, unmuted, and soloed when any pad is, at the note's
//! `int(127 log21(10 velocity + 1)) / 127`; that number is what a velocity
//! slot blends by. Striking a pad cuts every other pad in its cut group:
//! their hold ends there. A pattern runs in sixteenths of `rate 15 / tempo`
//! frames, the even ones `1 + swing / 2` of that and the odd ones
//! `1 - swing / 2`, each cut to whole frames, and strikes the pads whose
//! cell on the step has its high bit at the cell's low seven bits over 127.
//!
//! The pads take their gains and pitch from the key slot of their own key,
//! or of the pattern's key while it runs, filled as Drumpad's are.
//!
//! What is not FL's:
//!
//! - A pad routed to another mixer track (an output track not 0) plays on
//!   the plugin's own output. No shipped state routes one; fidelity says
//!   so when one does.
//! - A new Drumaxx starts from Drumpad's defaults on every pad, where FL's
//!   loads its `dmkit` resource. Every shipped state sets all of them.
//! - `DMS1` and `DMS2` states, which FL converts, are turned down.
//! - Each instance's noise starts where the first instance's does in FL.

// Limits are `max` then `min` in the binary's order, which turns a NaN into
// the lower limit where `clamp` would keep it.
#![allow(clippy::manual_clamp)]

use crate::bassdrum::velocity_level;
use crate::drumpad::{DEFAULTS as PAD_DEFAULTS, seed};
use crate::drumpad_voice::{self, Patch, RATE, Voice};
use crate::sampler::circular;
use crate::{Fidelity, Generator, NoteOn, StateError};

/// The plugin's event 201 name.
pub const NAME: &str = "Drumaxx";

/// The pads, and the parameters each has.
pub const PADS: usize = 16;
pub const PAD_PARAMS: usize = 44;

/// How many parameters there are: the pads', then sixteen of the kit's.
pub const PARAMS: usize = 720;

/// The kit's parameters.
const MASTER: usize = 0x2c1;
const SWING: usize = 0x2c2;
const EQ_ON: usize = 0x2c3;
const EQ_GAINS: usize = 0x2c4;
const LIMITER_ON: usize = 0x2cc;
const DRIVE: usize = 0x2cd;
const RELEASE: usize = 0x2ce;

/// The keys FL keeps the gains of.
const KEYS: usize = 128;

/// The patterns, the bytes each takes and its steps for each pad.
const PATTERNS: usize = 16;
const PATTERN_BYTES: usize = 0x402;
const STEPS: usize = 64;

/// The chunk magics read here: `DMS3` to `DMS5` and `DMD3` to `DMD5`.
const MAGICS: [u32; 6] = [
    0x444d_5333,
    0x444d_5334,
    0x444d_5335,
    0x444d_4433,
    0x444d_4434,
    0x444d_4435,
];
const DMS3: [u32; 2] = [0x444d_5333, 0x444d_4433];

/// How long the kit name is, and the pad names after the patterns.
const NAME_LEN: usize = 256;
const PAD_NAMES: usize = 0x800;

/// The most frames the pads are mixed in at once (`FUN_180034990`).
const MIX: usize = 512;

/// The eight EQ bands: `80 2^b` Hz, two octaves wide, a low shelf, six
/// peaks and a high shelf (`FUN_180033910`).
const BANDS: usize = 8;

/// The 2 pi the biquads are tuned with: the binary's, to seven digits.
#[allow(clippy::approx_constant)]
const BIQUAD_TAU: f64 = 6.283_185;

/// The output gain's glide a frame (`0x1800782c0`).
const OUT_GLIDE: f32 = 0.005;

/// A key's slot: the gains and pitch of the last note on it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Key {
    note: u32,
    left: f32,
    right: f32,
    semitones: f32,
}

/// The kinds of RBJ biquad the EQ runs (`FUN_18001bed0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    LowShelf,
    Peak,
    HighShelf,
}

/// One EQ band of one side: `FUN_18001bed0` tunes it at the host's rate,
/// `FUN_18001c270` runs it, flushing what goes in and comes out under 1e-9
/// to 0.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Band {
    kind: Kind,
    freq: f32,
    set: Option<(f32, f32)>,
    /// `b0, b1, b2, a1, a2`, over `a0`.
    c: [f32; 5],
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

fn flush(value: f32) -> f32 {
    if value < 1e-9 && value > -1e-9 {
        0.0
    } else {
        value
    }
}

impl Band {
    fn new(band: usize) -> Self {
        Band {
            kind: match band {
                0 => Kind::LowShelf,
                7 => Kind::HighShelf,
                _ => Kind::Peak,
            },
            freq: 80.0 * (1 << band) as f32,
            set: None,
            c: [1.0, 0.0, 0.0, 0.0, 0.0],
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    /// Tune for `gain` dB at `rate`, with the bandwidth of 2 octaves. The
    /// gain is `10^(dB / 20)` where the cookbook has `/ 40`, as in the
    /// voices' filters.
    fn tune(&mut self, gain: f32, rate: f32) {
        if self.set == Some((gain, rate)) {
            return;
        }
        self.set = Some((gain, rate));
        let a = 10f32.powf(gain * 0.05);
        let w = ((f64::from(self.freq) * BIQUAD_TAU / f64::from(rate)) as f32)
            .min(3.0)
            .max(0.0);
        let sin = w.sin();
        let s2 = {
            let arg = 2.0 * 0.346_573_6 * f64::from(w) / f64::from(sin);
            let sinh = arg.sinh();
            (sinh + sinh) as f32
        };
        let root = a.sqrt() * s2;
        let alpha = sin * s2 * 0.5;
        let cos = w.cos();
        let less = a - 1.0;
        let more = a + 1.0;
        let s = sin * root;
        let (b0, b1, b2, a0, a1, a2) = match self.kind {
            Kind::LowShelf => {
                let less_cos = less * cos;
                let more_cos = more * cos;
                let minus = more - less_cos;
                let plus = less_cos + more;
                (
                    (minus + s) * a,
                    (a + a) * (less - more_cos),
                    (minus - s) * a,
                    s + plus,
                    less * -2.0 - (more_cos + more_cos),
                    plus - s,
                )
            }
            Kind::Peak => {
                let up = alpha * a;
                let down = alpha / a;
                let b1 = cos * -2.0;
                (up + 1.0, b1, 1.0 - up, down + 1.0, b1, 1.0 - down)
            }
            Kind::HighShelf => {
                let less_cos = less * cos;
                let more_cos = more * cos;
                let plus = less_cos + more;
                let minus = more - less_cos;
                (
                    (s + plus) * a,
                    (more_cos + less) * a * -2.0,
                    (plus - s) * a,
                    minus + s,
                    {
                        let a1 = less - more_cos;
                        a1 + a1
                    },
                    minus - s,
                )
            }
        };
        let inverse = 1.0 / a0;
        self.c = [b0, b1, b2, a1, a2].map(|c| flush(inverse * c));
    }

    fn run(&mut self, x: f32) -> f32 {
        let x = flush(x);
        let [b0, b1, b2, a1, a2] = self.c;
        let y = flush(((self.x1 * b1 + x * b0 + b2 * self.x2) - self.y1 * a1) - a2 * self.y2);
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }

    fn clear(&mut self) {
        (self.x1, self.x2, self.y1, self.y2) = (0.0, 0.0, 0.0, 0.0);
    }
}

/// The limiter of one side (`FUN_180035840`): a gain that jumps down to
/// keep a sample within 1 and comes back to 1 on the fourth power of a
/// ramp.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Limiter {
    ramp: f32,
    floor: f32,
}

impl Limiter {
    const RESET: Limiter = Limiter {
        ramp: 0.0,
        floor: 1.0,
    };

    fn run(&mut self, x: f32, drive: f32, step: f64) -> f32 {
        let x = x * drive;
        let squared = self.ramp * self.ramp;
        let mut gain = squared * squared * (1.0 - self.floor) + self.floor;
        if (gain * x).abs() > 1.0 {
            self.ramp = 0.0;
            gain = 1.0 / x.abs();
            self.floor = gain;
        }
        let y = gain * x;
        self.ramp = (f64::from(self.ramp) + step) as f32;
        if self.ramp > 1.0 {
            self.ramp = 1.0;
        }
        y
    }
}

/// The pad parameters as Drumpad's 38, for [`Patch::of`]: `p` of the pad
/// is `p - 5` there, but for Noise repeat and Retrigger, which are the
/// plugin's parameters 5 and 6 whatever the pad (`FUN_18002c970`).
fn drumpad_params(params: &[f32; PARAMS], pad: usize) -> [f32; drumpad_voice::PARAMS] {
    let mut out = [0.0; drumpad_voice::PARAMS];
    let at = pad * PAD_PARAMS + 5;
    out.copy_from_slice(&params[at..at + drumpad_voice::PARAMS]);
    out[0] = params[5];
    out[1] = params[6];
    out
}

/// What a new Drumaxx is set to here: Drumpad's defaults on every pad,
/// struck by keys 36 to 51, and the kit's controls at their middles.
fn defaults() -> [f32; PARAMS] {
    let mut params = [0.0; PARAMS];
    for pad in 0..PADS {
        let at = pad * PAD_PARAMS;
        params[at] = 0.5;
        params[at + 1] = 0.5;
        params[at + 4] = (36 + pad) as f32;
        params[at + 5..at + 5 + PAD_DEFAULTS.len()].copy_from_slice(&PAD_DEFAULTS);
    }
    params[5] = 0.0;
    params[6] = 0.0;
    params[MASTER] = 0.72;
    params[EQ_GAINS..EQ_GAINS + BANDS].fill(0.5);
    params[RELEASE] = 0.5;
    params
}

/// Drumaxx. A new one is at 44.1 kHz and 120 beats a minute.
#[derive(Debug, Clone)]
pub struct Drumaxx {
    params: [f32; PARAMS],
    kit: String,
    /// The patterns as FL holds them: each 1024 cells, 64 steps of pad 0
    /// then of pad 1 and on, then its last step and its key.
    patterns: Vec<u8>,
    groups: [u8; PADS],
    outputs: [u32; PADS],
    rate: f32,
    tempo: f64,
    voices: Vec<Voice>,
    /// For each key: what its last note left, whether a note holds it, its
    /// velocity, and FL's flag that keeps it (`+0xa20`).
    notes: [Option<Key>; KEYS],
    held: [bool; KEYS],
    kept: [bool; KEYS],
    /// The pattern that runs, its key, whether it starts over at the next
    /// block (`+0x6340`), and the flag that keeps its key while the pads
    /// ring on after it stops (`+0x6359`).
    pattern: Option<usize>,
    pattern_key: i8,
    restart: bool,
    ringing: bool,
    /// Frames to the next step and the step (`+0x6348`, `+0x6350`).
    countdown: i64,
    step: i32,
    eq: [[Band; BANDS]; 2],
    limiter: [Limiter; 2],
    /// The output gain of each side and what it moves to.
    out_gain: [f32; 2],
    main: [Vec<f32>; 2],
    routed: bool,
}

impl Default for Drumaxx {
    fn default() -> Self {
        let rate = 44_100.0;
        let mut patterns = vec![0u8; PATTERNS * PATTERN_BYTES];
        for (index, pattern) in patterns
            .as_chunks_mut::<PATTERN_BYTES>()
            .0
            .iter_mut()
            .enumerate()
        {
            pattern[0x400] = 15;
            pattern[0x401] = 60 + index as u8;
        }
        Drumaxx {
            params: defaults(),
            kit: String::new(),
            patterns,
            groups: [0; PADS],
            outputs: [0; PADS],
            rate,
            tempo: 120.0,
            voices: (0..PADS).map(|pad| Voice::whole(rate, seed(pad))).collect(),
            notes: [None; KEYS],
            held: [false; KEYS],
            kept: [false; KEYS],
            pattern: None,
            pattern_key: 0,
            restart: true,
            ringing: false,
            countdown: 0,
            step: 0,
            eq: [
                std::array::from_fn(Band::new),
                std::array::from_fn(Band::new),
            ],
            limiter: [Limiter::RESET; 2],
            out_gain: [0.0; 2],
            main: [vec![0.0; MIX], vec![0.0; MIX]],
            routed: false,
        }
    }
}

impl Drumaxx {
    /// The kit name the state holds.
    pub fn kit(&self) -> &str {
        &self.kit
    }

    /// The parameters, as the state holds them.
    pub fn params(&self) -> &[f32; PARAMS] {
        &self.params
    }

    fn pad_param(&self, pad: usize, p: usize) -> f32 {
        self.params[pad * PAD_PARAMS + p]
    }

    /// The key that strikes a pad, as FL truncates it.
    fn pad_key(&self, pad: usize) -> i32 {
        self.pad_param(pad, 4) as i32
    }

    fn any_solo(&self) -> bool {
        (0..PADS).any(|pad| self.pad_param(pad, 3) != 0.0)
    }

    /// Whether a pad may sound: unmuted, and soloed if any pad is.
    fn open(&self, pad: usize, solo: bool) -> bool {
        self.pad_param(pad, 2) == 0.0 && (!solo || self.pad_param(pad, 3) != 0.0)
    }

    fn keep(&mut self, key: i32, on: bool) {
        if let Some(kept) = usize::try_from(key).ok().and_then(|k| self.kept.get_mut(k)) {
            *kept = on;
        }
    }

    fn is_kept(&self, key: i32) -> bool {
        usize::try_from(key)
            .ok()
            .and_then(|k| self.kept.get(k))
            .is_some_and(|kept| *kept)
    }

    /// One byte of the patterns as FL lays them out; past the end, 0.
    fn pattern_byte(&self, at: usize) -> u8 {
        self.patterns.get(at).copied().unwrap_or(0)
    }

    /// Strike a pad at `velocity` (`FUN_180035490`): every other pad in
    /// its cut group is cut, then the pad's voice starts.
    fn strike(&mut self, pad: usize, velocity: f32) {
        let group = self.groups[pad];
        for other in 0..PADS {
            if other != pad && self.groups[other] != 0 && self.groups[other] == group {
                self.voices[other].cut = true;
            }
        }
        let patch = Patch::of(&drumpad_params(&self.params, pad), velocity);
        self.voices[pad].trigger(patch);
    }

    /// The synth's note on (`FUN_180034e00`), the key's velocity over 0.
    fn key_on(&mut self, key: i32, velocity: f32) {
        for pattern in 0..PATTERNS {
            let pattern_key = self.pattern_byte(pattern * PATTERN_BYTES + 0x401) as i8;
            if i32::from(pattern_key) == key {
                self.pattern_key = pattern_key;
                self.pattern = Some(pattern);
                self.restart = true;
                self.keep(key, true);
            }
        }
        let solo = self.any_solo();
        for pad in 0..PADS {
            let pad_key = self.pad_key(pad);
            let strike = pad_key == key && self.open(pad, solo);
            if strike {
                self.strike(pad, velocity);
            }
            self.keep(pad_key, strike);
        }
    }

    /// The synth's note off: only the key of the pattern does anything.
    fn key_off(&mut self, key: i32) {
        if key != i32::from(self.pattern_key) {
            return;
        }
        if self.pattern.is_some() {
            self.ringing = true;
        }
        self.pattern = None;
        self.restart = true;
    }

    /// Mix `n` frames of the pads into `left` and `right` from `at`
    /// (`FUN_180034990`): the pads into the plugin's bus, the bus through
    /// the EQ and the limiter while any pad on it sounds, then the output
    /// gain.
    fn mix(&mut self, left: &mut [f32], right: &mut [f32], at: usize, n: usize, gain: [f32; 2]) {
        let mut done = 0;
        while done < n {
            let len = (n - done).min(MIX);
            let [main_left, main_right] = &mut self.main;
            main_left[..len].fill(0.0);
            main_right[..len].fill(0.0);
            let mut any = false;
            for (pad, voice) in self.voices.iter_mut().enumerate() {
                any |= voice.active;
                if voice.active && self.outputs[pad] != 0 {
                    self.routed = true;
                }
                voice.play(&mut main_left[..len], &mut main_right[..len], self.rate);
            }
            if any {
                self.bus(len);
            } else {
                for side in &mut self.eq {
                    side.iter_mut().for_each(Band::clear);
                }
                self.limiter = [Limiter::RESET; 2];
            }
            let from = at + done;
            for i in 0..len {
                if any {
                    left[from + i] += self.main[0][i] * self.out_gain[0];
                    right[from + i] += self.main[1][i] * self.out_gain[1];
                }
                for (now, to) in self.out_gain.iter_mut().zip(gain) {
                    *now += (to - *now) * OUT_GLIDE;
                }
            }
            done += len;
        }
    }

    /// The EQ and the limiter on `len` frames of the bus (`FUN_180035840`).
    fn bus(&mut self, len: usize) {
        let eq = self.params[EQ_ON] != 0.0;
        let limit = self.params[LIMITER_ON] != 0.0;
        for band in 0..BANDS {
            let gain = (self.params[EQ_GAINS + band] - 0.5) * 16.0;
            for side in &mut self.eq {
                side[band].tune(gain, self.rate);
            }
        }
        let release = self.params[RELEASE];
        let step = f64::from(release * release * 3.0 + 2.0) / f64::from(self.rate);
        let drive = 10f32.powf(self.params[DRIVE] * 1.5);
        for side in 0..2 {
            for i in 0..len {
                let mut x = self.main[side][i];
                if eq {
                    for band in self.eq[side].iter_mut().rev() {
                        x = band.run(x);
                    }
                }
                if limit {
                    x = self.limiter[side].run(x, drive, step);
                }
                self.main[side][i] = x;
            }
        }
    }

    /// The output gain of each side, from the master volume
    /// (`FUN_180025510` with FL's default CC 7 and 10 and pan).
    fn output_gain(&self) -> [f32; 2] {
        let master = 10f32.powf(self.params[MASTER] * 1.6) * 0.25;
        let gain = master * 0.5;
        if gain <= 0.0 { [0.0; 2] } else { [gain; 2] }
    }

    /// Silence every pad, as FL's reset of each voice does.
    fn silence(&mut self) {
        for voice in &mut self.voices {
            voice.clear();
        }
    }

    fn reset_bus(&mut self) {
        for side in &mut self.eq {
            side.iter_mut().for_each(Band::clear);
        }
        self.limiter = [Limiter::RESET; 2];
    }
}

/// Read a 7-bit varint as the loader does: low groups first, a clear high
/// bit ends it. Says what it read and how many bytes.
fn varint(bytes: &[u8]) -> (u32, usize) {
    let mut value = 0u32;
    for (i, byte) in bytes.iter().enumerate() {
        let shift = (7 * i) as u32;
        let part = u32::from(byte & 0x7f).checked_shl(shift).unwrap_or(0);
        value = value.wrapping_add(part);
        if byte & 0x80 == 0 {
            return (value, i + 1);
        }
    }
    (value, bytes.len())
}

impl Generator for Drumaxx {
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
        let Some(chunk) = bytes.get(8..8 + len).filter(|chunk| chunk.len() >= 4) else {
            return Err(StateError::Length {
                expected: 8 + len.max(4),
                found: bytes.len(),
            });
        };
        let magic = u32::from_le_bytes(chunk[..4].try_into().unwrap());
        if !MAGICS.contains(&magic) {
            return Err(StateError::Version {
                found: magic as i32,
            });
        }
        let mut rest = &chunk[4..];
        let mut kit = None;
        if rest.len() > NAME_LEN {
            let name = &rest[..NAME_LEN];
            let end = name.iter().position(|b| *b == 0).unwrap_or(NAME_LEN);
            kit = Some(name[..end].iter().map(|b| char::from(*b)).collect());
            rest = &rest[NAME_LEN..];
        }
        let need = PARAMS * 4 + PATTERNS * PATTERN_BYTES;
        if rest.len() < need {
            return Err(StateError::Length {
                expected: 8 + len - rest.len() + need,
                found: bytes.len(),
            });
        }
        let mut params = [0.0f32; PARAMS];
        for (param, word) in params.iter_mut().zip(rest.as_chunks::<4>().0) {
            *param = f32::from_le_bytes(*word);
        }
        rest = &rest[PARAMS * 4..];
        // On disk a pattern is its last step and key, then its cells; FL
        // holds the cells first.
        let mut patterns = vec![0u8; PATTERNS * PATTERN_BYTES];
        for (held, saved) in patterns
            .as_chunks_mut::<PATTERN_BYTES>()
            .0
            .iter_mut()
            .zip(rest.as_chunks::<PATTERN_BYTES>().0)
        {
            held[..0x400].copy_from_slice(&saved[2..]);
            held[0x400] = saved[0];
            held[0x401] = saved[1];
        }
        rest = &rest[PATTERNS * PATTERN_BYTES..];
        if rest.len() >= PAD_NAMES {
            rest = &rest[PAD_NAMES..];
        }
        let mut groups = self.groups;
        for (group, byte) in groups.iter_mut().zip(rest) {
            *group = *byte;
        }
        rest = &rest[rest.len().min(PADS)..];
        let mut outputs = [0u32; PADS];
        for output in &mut outputs {
            let (value, used) = varint(rest);
            *output = value;
            rest = &rest[used..];
        }
        if DMS3.contains(&magic) {
            for pad in 0..PADS {
                let at = pad * PAD_PARAMS + 6;
                params[at] = (params[at] * (1.0 / 3.0)).max(0.0).min(1.0);
            }
        }
        if let Some(kit) = kit {
            self.kit = kit;
        }
        self.params = params;
        self.patterns = patterns;
        self.groups = groups;
        self.outputs = outputs;
        self.reset_bus();
        Ok(())
    }

    /// The voices are cut, as FL cuts them.
    fn set_sample_rate(&mut self, hz: f32) {
        self.rate = hz;
        for voice in &mut self.voices {
            voice.set_rate(hz);
        }
        self.silence();
    }

    fn set_tempo(&mut self, bpm: f64) {
        self.tempo = bpm;
    }

    fn note_on(&mut self, note: NoteOn) {
        let key = note.key.clamp(0, KEYS as i16 - 1) as usize;
        // FL lets go of a note the key had before the new one starts.
        if self.notes[key].is_some() {
            self.key_off(key as i32);
        }
        let (left, right) = circular(note.pan, note.velocity);
        self.notes[key] = Some(Key {
            note: note.voice,
            left,
            right,
            semitones: (f64::from(note.fine_pitch) * 0.01) as f32,
        });
        let velocity = velocity_level(note.velocity);
        self.held[key] = velocity != 0.0;
        if velocity == 0.0 {
            self.key_off(key as i32);
        } else {
            self.key_on(key as i32, velocity);
        }
    }

    fn note_off(&mut self, voice: u32) {
        for key in 0..KEYS {
            if self.held[key] && self.notes[key].is_some_and(|note| note.note == voice) {
                self.held[key] = false;
                self.key_off(key as i32);
            }
        }
    }

    fn all_notes_off(&mut self) {
        for key in 0..KEYS {
            if self.held[key] {
                self.held[key] = false;
                self.key_off(key as i32);
            }
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let frames = left.len().min(right.len());
        // FUN_180003dd0's fill: every key gets the gains and pitch of the
        // key with a note at or below it, or of the lowest such key.
        let mut keys = [(0.0f32, 0.0f32, 0.0f32); KEYS];
        let mut last = None;
        for key in 0..KEYS {
            if let Some(note) = self.notes[key] {
                let now = (note.left, note.right, note.semitones);
                if last.is_none() {
                    keys[..key].fill(now);
                }
                last = Some(now);
            }
            keys[key] = last.unwrap_or_default();
        }
        // FUN_180034280: the pattern's key is kept while it runs, and
        // after it stops while the pads ring on.
        let ringing = self.voices.iter().any(|voice| voice.active);
        let pattern_key = i32::from(self.pattern_key);
        if self.pattern.is_some() || (self.ringing && ringing) {
            self.keep(pattern_key, true);
        }
        if self.ringing {
            self.ringing = ringing;
        }
        for pad in 0..PADS {
            if !self.voices[pad].active {
                continue;
            }
            let pad_key = self.pad_key(pad);
            self.keep(pad_key, true);
            let slot = if self.is_kept(pattern_key) {
                pattern_key
            } else {
                pad_key
            };
            let (l, r, semitones) = usize::try_from(slot)
                .ok()
                .and_then(|slot| keys.get(slot))
                .copied()
                .unwrap_or_default();
            let volume = {
                let v = self.pad_param(pad, 0);
                v * v + v * v
            };
            let pan = self.pad_param(pad, 1);
            let voice = &mut self.voices[pad];
            voice.bend = 2f32.powf(semitones.max(-12.0).min(12.0) * 0.083_333_336);
            voice.target = ((1.0 - pan) * volume * l, pan * volume * r);
        }
        if self.restart {
            self.restart = false;
            self.countdown = 0;
            self.step = 0;
        }
        let gain = self.output_gain();
        match self.pattern {
            None => self.mix(left, right, 0, frames, gain),
            Some(pattern) => {
                let swing = (self.params[SWING] * 0.5).max(0.0).min(0.5);
                let solo = self.any_solo();
                let base = pattern * PATTERN_BYTES;
                let last = i32::from(self.pattern_byte(base + 0x400));
                let sixteenth = f64::from(self.rate) * 15.0 / self.tempo;
                let mut at = 0;
                while at < frames {
                    if self.countdown <= 0 {
                        let swing = f64::from(swing);
                        let length = if self.step & 1 == 0 {
                            sixteenth * (swing + 1.0)
                        } else {
                            sixteenth * (1.0 - swing)
                        };
                        // A step of no frames would never end; FL's tempo
                        // never asks for one.
                        self.countdown = ((length + self.countdown as f64) as i64).max(1);
                        let step = self.step.max(0).min(STEPS as i32) as usize;
                        for pad in 0..PADS {
                            let cell = self.pattern_byte(base + step + pad * STEPS);
                            if cell & 0x80 != 0 && self.open(pad, solo) {
                                self.strike(pad, f32::from(cell & 0x7f) * (1.0 / 127.0));
                            }
                        }
                        self.step += 1;
                        if self.step > last {
                            self.step = 0;
                        }
                    }
                    let n = (self.countdown as usize).min(frames - at);
                    self.mix(left, right, at, n, gain);
                    self.countdown -= n as i64;
                    at += n;
                }
            }
        }
        // FUN_1800048d0: a key lets go once nothing keeps or holds it.
        for key in 0..KEYS {
            if !self.kept[key] && !self.held[key] {
                self.notes[key] = None;
            }
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

    /// `int / 256`, 0 to 1 (`FUN_1800034b0`). Turning the EQ or the
    /// limiter on or off starts it from silence (`FUN_180033770`).
    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(param) = self.params.get_mut(index) {
            let value = f64::from(value.round().clamp(-1e9, 1e9) as i32) * (1.0 / 256.0);
            *param = (value as f32).max(0.0).min(1.0);
            match index {
                EQ_ON => self.eq.iter_mut().flatten().for_each(Band::clear),
                LIMITER_ON => self.limiter = [Limiter::RESET; 2],
                _ => {}
            }
        }
    }

    fn reset(&mut self) {
        self.silence();
        self.notes = [None; KEYS];
        self.held = [false; KEYS];
        self.kept = [false; KEYS];
        self.pattern = None;
        self.ringing = false;
        self.restart = true;
        self.reset_bus();
    }

    /// The longest a pad can sound, as Drumpad's [`Generator::tail`]
    /// works it out, for the pad that sounds longest.
    fn tail(&self) -> usize {
        (0..PADS)
            .map(|pad| {
                let params = drumpad_params(&self.params, pad);
                let ends = [0.0, 1.0].map(|x| Patch::of(&params, x));
                let hold = ends.iter().map(|p| f64::from(p.hold)).fold(0.0, f64::max);
                let pitch = ends.iter().map(|p| p.pitch).fold(2.0, f64::min);
                let samples = (hold * f64::from(RATE) + 9_300.0).max(19_000.0) + 950.0 + 1_100.0;
                let step = f64::from(RATE / self.rate) * pitch * 0.5;
                (samples / step).ceil() as usize
            })
            .max()
            .unwrap_or(0)
    }

    fn fidelity(&self) -> Fidelity {
        if self.routed || self.outputs.iter().any(|output| *output != 0) {
            Fidelity::Approximate("Drumaxx pads routed to other mixer tracks")
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

    /// A state as FL saves it: name, parameters, patterns on disk, pad
    /// names, cut groups and outputs.
    fn state(
        params: &[f32; PARAMS],
        patterns: &[[u8; PATTERN_BYTES]],
        groups: [u8; 16],
    ) -> Vec<u8> {
        let mut chunk = MAGICS[1].to_le_bytes().to_vec();
        let mut name = [0u8; NAME_LEN];
        name[..4].copy_from_slice(b"Test");
        chunk.extend(name);
        for value in params {
            chunk.extend(value.to_le_bytes());
        }
        for pattern in patterns {
            chunk.extend(pattern);
        }
        chunk.extend([0u8; PAD_NAMES]);
        chunk.extend(groups);
        chunk.extend([0u8; PADS]);
        let mut bytes = b"SSLF".to_vec();
        bytes.extend((chunk.len() as u32).to_le_bytes());
        bytes.extend(chunk);
        bytes
    }

    /// Sixteen empty patterns on disk: last step 15, keys 60 to 75.
    fn empty_patterns() -> Vec<[u8; PATTERN_BYTES]> {
        (0..PATTERNS)
            .map(|i| {
                let mut pattern = [0u8; PATTERN_BYTES];
                pattern[0] = 15;
                pattern[1] = 60 + i as u8;
                pattern
            })
            .collect()
    }

    fn play(drum: &mut Drumaxx, frames: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
        for (l, r) in left.chunks_mut(512).zip(right.chunks_mut(512)) {
            drum.process(l, r);
        }
        (left, right)
    }

    fn short_hits() -> [f32; PARAMS] {
        let mut params = defaults();
        for pad in 0..PADS {
            // Hold 0.024 s.
            params[pad * PAD_PARAMS + 33] = 0.2;
        }
        params
    }

    #[test]
    fn a_state_reads_its_kit_parameters_patterns_and_groups() {
        let mut params = short_hits();
        params[44 + 9] = 0.25;
        let mut patterns = empty_patterns();
        patterns[2][2 + 5] = 0x80 | 100;
        let mut groups = [0u8; 16];
        groups[3] = 2;
        let mut drum = Drumaxx::default();
        drum.set_state(&state(&params, &patterns, groups)).unwrap();
        assert_eq!(drum.kit(), "Test");
        assert_eq!(drum.params()[44 + 9], 0.25);
        // FL holds the cells first and the last step and key after them.
        let base = 2 * PATTERN_BYTES;
        assert_eq!(drum.patterns[base + 5], 0x80 | 100);
        assert_eq!(drum.patterns[base + 0x400], 15);
        assert_eq!(drum.patterns[base + 0x401], 62);
        assert_eq!(drum.groups[3], 2);
        assert_eq!(drum.param(44 + 9), 64.0);
        // A DMS3 state has its Retrigger in thirds.
        let mut old = state(&params, &patterns, groups);
        old[8] = b'3';
        params[6] = 3.0;
        params[44 + 6] = 1.5;
        let mut three = state(&params, &patterns, groups);
        three[8] = b'3';
        drum.set_state(&three).unwrap();
        assert_eq!(drum.params()[6], 1.0);
        assert_eq!(drum.params()[44 + 6], 0.5);
        // DMS2 and a short chunk are turned down.
        old[8] = b'2';
        assert!(drum.set_state(&old).is_err());
        assert!(drum.set_state(&old[..400]).is_err());
    }

    #[test]
    fn a_note_strikes_the_pads_on_its_key_and_nothing_else() {
        let mut drum = Drumaxx::default();
        drum.set_state(&state(&short_hits(), &empty_patterns(), [0; 16]))
            .unwrap();
        // Key 40 is pad 4's.
        drum.note_on(note(1, 40, 1.0));
        assert!(drum.voices[4].active);
        assert_eq!(drum.voices.iter().filter(|v| v.active).count(), 1);
        let (left, right) = play(&mut drum, 44_100);
        assert!(left.iter().chain(&right).all(|v| v.is_finite()));
        let peak = left.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.001 && peak < 2.0, "{peak}");
        assert!(left[30_000..].iter().all(|v| *v == 0.0));
        // A key no pad has strikes none.
        let mut drum = Drumaxx::default();
        drum.note_on(note(1, 20, 1.0));
        assert!(drum.voices.iter().all(|v| !v.active));
        // A muted pad does not sound, and a soloed one silences the rest.
        let mut params = short_hits();
        params[4 * PAD_PARAMS + 2] = 1.0;
        params[5 * PAD_PARAMS + 3] = 1.0;
        drum.set_state(&state(&params, &empty_patterns(), [0; 16]))
            .unwrap();
        drum.note_on(note(2, 40, 1.0));
        drum.note_on(note(3, 42, 1.0));
        drum.note_on(note(4, 41, 1.0));
        let active: Vec<usize> = (0..PADS).filter(|p| drum.voices[*p].active).collect();
        assert_eq!(active, [5]);
    }

    #[test]
    fn a_pad_cuts_the_others_in_its_group() {
        let mut groups = [0u8; 16];
        groups[0] = 1;
        groups[1] = 1;
        let mut params = defaults();
        params[33] = 1.0;
        params[PAD_PARAMS + 33] = 1.0;
        let mut drum = Drumaxx::default();
        drum.set_state(&state(&params, &empty_patterns(), groups))
            .unwrap();
        drum.note_on(note(1, 36, 1.0));
        assert!(!drum.voices[0].cut);
        drum.note_on(note(2, 37, 1.0));
        assert!(drum.voices[0].cut && !drum.voices[1].cut);
    }

    #[test]
    fn a_pattern_steps_in_sixteenths_while_its_key_is_held() {
        let mut patterns = empty_patterns();
        // Pattern 0 strikes pad 2 on steps 0 and 4, pad 3 on step 1.
        patterns[0][2 + 2 * STEPS] = 0x80 | 127;
        patterns[0][2 + 2 * STEPS + 4] = 0x80 | 64;
        patterns[0][2 + 3 * STEPS + 1] = 0x80 | 127;
        let mut drum = Drumaxx::default();
        drum.set_state(&state(&short_hits(), &patterns, [0; 16]))
            .unwrap();
        drum.set_tempo(120.0);
        drum.note_on(note(1, 60, 1.0));
        assert_eq!(drum.pattern, Some(0));
        // A sixteenth at 120 is 5512.5 frames, 5512 as FL cuts it.
        let mut left = vec![0.0; 512];
        let mut right = vec![0.0; 512];
        drum.process(&mut left, &mut right);
        assert!(drum.voices[2].active && !drum.voices[3].active);
        assert_eq!(drum.countdown, 5512 - 512);
        for _ in 0..10 {
            drum.process(&mut left, &mut right);
        }
        assert!(drum.voices[3].active);
        // The note off stops it.
        drum.note_off(1);
        assert_eq!(drum.pattern, None);
    }

    #[test]
    fn the_eq_and_the_limiter_hold_a_hot_bus_down() {
        let mut limiter = Limiter::RESET;
        let out: Vec<f32> = (0..100)
            .map(|i| limiter.run(if i == 10 { 4.0 } else { 0.5 }, 1.0, 0.0001))
            .collect();
        assert!(out.iter().all(|v| v.abs() <= 1.0 + 1e-6));
        assert!((out[10] - 1.0).abs() < 1e-6);
        assert!(out[11] < 0.5 && out[11] > 0.0);
        // A band's gain at its frequency: twice its dB, as the voices'.
        let mut band = Band::new(3);
        band.tune(4.0, 44_100.0);
        let [b0, b1, b2, a1, a2] = band.c.map(f64::from);
        let gain = testing::response(&[b0, b1, b2], &[1.0, a1, a2], 640.0 / 44_100.0);
        assert!((20.0 * gain.log10() - 8.0).abs() < 0.1, "{gain}");
        let mut low = Band::new(0);
        low.tune(4.0, 44_100.0);
        let [b0, b1, b2, a1, a2] = low.c.map(f64::from);
        let bottom = testing::response(&[b0, b1, b2], &[1.0, a1, a2], 1.0 / 44_100.0);
        let top = testing::response(&[b0, b1, b2], &[1.0, a1, a2], 0.4);
        assert!((20.0 * bottom.log10() - 8.0).abs() < 0.2, "{bottom}");
        assert!((20.0 * top.log10()).abs() < 0.2, "{top}");
    }

    #[test]
    fn every_state_fl_ships_loads_and_plays() {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, Drumaxx's states skipped");
            return;
        };
        let mut saved = testing::every_preset(&root, NAME);
        let presets = saved.len();
        saved.extend(
            testing::projects(&root)
                .into_iter()
                .filter(|saved| saved.plugin.eq_ignore_ascii_case(NAME)),
        );
        for saved in &saved {
            let mut drum = crate::generator_by_name(&saved.plugin).unwrap();
            if let Err(error) = drum.set_state(&saved.state) {
                panic!("{}: {error}", saved.file);
            }
            drum.set_tempo(130.0);
            for (voice, key) in (36..52).enumerate() {
                drum.note_on(note(voice as u32, key, 0.8));
            }
            // And the first pattern.
            let mut read = Drumaxx::default();
            read.set_state(&saved.state).unwrap();
            drum.note_on(note(99, i16::from(read.patterns[0x401] as i8), 0.8));
            let (mut left, mut right) = (vec![0.0; 16_384], vec![0.0; 16_384]);
            for (l, r) in left.chunks_mut(512).zip(right.chunks_mut(512)) {
                drum.process(l, r);
            }
            assert!(
                left.iter()
                    .chain(&right)
                    .all(|v| v.is_finite() && v.abs() < 8.0),
                "{}",
                saved.file
            );
            assert!(left.iter().any(|v| *v != 0.0), "{}", saved.file);
            assert_eq!(drum.fidelity(), Fidelity::Exact, "{}", saved.file);
        }
        // 135 presets and the render test.
        assert!(presets >= 135, "{presets}");
        assert_eq!(saved.len() - presets, 1);
    }
}
