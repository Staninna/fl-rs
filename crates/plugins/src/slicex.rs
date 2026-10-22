//! Slicex: two decks, each a sample cut into regions, every region a
//! slice a key plays, as `docs/plugins/slicex.md` has it.
//!
//! # The state
//!
//! A version (FL writes 0x60003 and reads it as 6), the plugin's eight
//! knobs, 47 bytes of settings and three words, then each deck: whether its
//! sample is a file (and the file's path) or carried in the state as a
//! whole WAV file, 0x4c bytes of the deck's settings, its four envelopes and
//! its slices of 17 words each. Eight articulator groups follow, each five
//! words (the filter) and six articulators.
//!
//! # The slices
//!
//! A sample's regions are its WAV file's cue points, their lengths from the
//! `ltxt` records and their keys from the `tlst` chunk. A region without a
//! key goes on the key its number counts up from a base: 60 for up to 65
//! regions, an octave lower for every 12 above 54. Slice `i` holds the
//! settings of region `i`.
//!
//! # A note
//!
//! The note's colour picks the deck and its key the slices. Each plays its
//! region from its start (or its sample start), at the file's own pitch
//! moved by the note's fine pitch, the slice's and the plugin's pitch, at
//! the slice's volume times the note's velocity and the plugin's volume.
//! Once a control tick (48 samples at 44.1 kHz) the voice's articulators
//! move on and its gains are worked out again, and the gains ramp to them
//! over the tick. The articulators of the slice's groups move its pan and
//! volume and, with the group's filter on, the filter's cutoff and
//! resonance. A note off lets the articulators go; once they are all over
//! (at once, when none runs) FL's declicker fades the voice out over 20 ms
//! and the voice ends. A region that plays out does the same from the frame
//! it ends on, its last frame held.
//!
//! # What is not FL's
//!
//! - The player is FL's interpolator ([`dsp::interpolator`]) on the
//!   region's frames alone, at the quality the host asks for, taken to be
//!   FL's default render quality (`sampler::RENDER_POINTS`).
//! - The speed and sample start articulators, the articulators' maps, cut
//!   groups, time stretching and the layer modes that pick one of several
//!   slices on a key are left out, and [`Generator::fidelity`] says so when
//!   a state asks for them.
//! - FL reads a file's regions itself. Here the plugin reads them from the
//!   file under FL's install, which the host names with
//!   [`Generator::set_resource_dir`].

use crate::autogun::{Biquad, ramp, stereo_pan};
use crate::harmor_art::{EnvRun, Envelope};
use crate::menvelope::{Class, Lfo, LfoRun, MEnvelope, Reader};
use crate::sampler::{self, Bank, Wave};
use crate::{Fidelity, Generator, NoteOn, SampleFile, StateError};
use dsp::interpolator::Interpolator;
use std::f64::consts::{LN_2, PI, TAU};
use std::path::{Component, Path, PathBuf};

/// The plugin's event 201 name.
pub const NAME: &str = "Slicex";

/// The most slices that sound at once.
pub const VOICES: usize = 256;

/// The version FL 2026 writes.
const WRITTEN: i32 = 0x60003;

/// A slice's 17 words as a new slice has them (`FUN_00887420`).
const DEFAULT_SLICE: [i32; 17] = [
    0, 0, 0, 8000, 0, 12800, 128, 128, 0, 12800, 0, 0, 12800, 0, 0, 0, 12800,
];

/// The articulator groups, and the parameters each has: its three knobs
/// and the four envelope and four LFO knobs of its first five
/// articulators (`FUN_0087f320`).
const GROUPS: usize = 8;
const GROUP_PARAMS: usize = 43;

/// The knobs, then 43 parameters a group (`ProcessParam` takes 0x160).
pub const PARAMS: usize = 8 + GROUPS * GROUP_PARAMS;

/// The ranges of the knobs' controls (the editor's DFM: `MainVolSlider`
/// 0..16000, `MainRandSlider` 0..256, `MainLFOSlider` -128..128,
/// `MainPitchSlider` -1200..1200, `ModXWheel` and `ModYWheel` 0..512).
/// Knobs 0 and 1 have no control; `ProcessParam` turns a value for knob 1
/// into a switch.
const KNOB_RANGES: [(i32, i32); 8] = [
    (0, 1),
    (0, 1),
    (0, 16_000),
    (0, 256),
    (-128, 128),
    (-1200, 1200),
    (0, 512),
    (0, 512),
];

/// A group's knobs: `FEnvWheel`, `FCutWheel` and `FResWheel`.
const HEAD_RANGE: (i32, i32) = (0, 256);

/// An articulator's knobs (`TMEnvEditor`'s DFM): attack, decay, sustain and
/// release, then the LFO's speed, tension, skew and pulse width.
const ART_RANGES: [(i32, i32); 8] = [
    (0, 256),
    (0, 256),
    (-128, 128),
    (0, 256),
    (512, 65_536),
    (-128, 128),
    (-128, 128),
    (-112, 112),
];

/// The articulators of a group by what they move: pan, volume, filter
/// cutoff, filter resonance, speed and sample start. Pan, the filter and
/// the sample start add (`DAT_0087dc7a`), volume and speed multiply, and
/// a voice starts volume at 1 and the rest at 0 (`DAT_00883960`).
const ITEMS: usize = 6;
const MULTIPLIES: [bool; ITEMS] = [false, true, false, false, true, false];
const STARTS_AT: [f64; ITEMS] = [0.0, 1.0, 0.0, 0.0, 0.0, 0.0];

/// The most filter stages a voice has room for.
const STAGES: usize = 3;

/// Voice states as FL numbers them: playing, let go, fading out, over.
const PLAYING: i32 = 1;
const RELEASED: i32 = 5;
const ENDING: i32 = 9;
const OVER: i32 = 10;

/// One region of a sample: where it starts, where it ends (the last frame,
/// `None` for up to the next region), its name and its key.
#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    pub start: i64,
    pub end: Option<i64>,
    pub name: String,
    /// -1 for none.
    pub key: i32,
}

/// One slice's settings, the 17 words of the state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slice(pub [i32; 17]);

impl Default for Slice {
    fn default() -> Self {
        Slice(DEFAULT_SLICE)
    }
}

/// `FUN_008874a0`'s scales.
const OVER_128: f32 = 0.007_812_5;
const OVER_256: f32 = 0.003_906_25;
const OVER_6400: f32 = 0.000_156_25;
const OVER_12800: f32 = 7.812_5e-5;

impl Slice {
    /// -1 on the left to 1 on the right (word 2 / 6400).
    pub fn pan(&self) -> f32 {
        self.0[2] as f32 * OVER_6400
    }

    /// The volume: word 3 over 8000, 1 there and `(3^x - 1) / 2` elsewhere.
    pub fn volume(&self) -> f32 {
        let x = (f64::from(self.0[3]) * 0.000_125) as f32;
        if x == 1.0 {
            return 1.0;
        }
        let e = (f64::from(x * 3f32.ln())).exp();
        ((e - 1.0) * 0.5) as f32
    }

    /// How much of its amp group's articulators the slice takes (word 5).
    fn amp_amount(&self) -> f32 {
        self.0[5] as f32 * OVER_12800
    }

    /// The filter's cutoff and resonance, 0 to 1 (words 6 and 7 / 256).
    fn cutoff(&self) -> f32 {
        self.0[6] as f32 * OVER_256
    }

    fn resonance(&self) -> f32 {
        self.0[7] as f32 * OVER_256
    }

    /// How much of its filter group's articulators the slice takes (word 9).
    fn filter_amount(&self) -> f32 {
        self.0[9] as f32 * OVER_12800
    }

    /// Cents (word 10).
    pub fn pitch(&self) -> f32 {
        self.0[10] as f32
    }

    /// The start delay in beats and the sample start, 0 to 1 (words 13
    /// and 14 / 12800).
    fn delay(&self) -> f32 {
        self.0[13] as f32 * OVER_12800
    }

    fn start(&self) -> f32 {
        self.0[14] as f32 * OVER_12800
    }

    /// The groups of the slice's amp (pan and volume), filter, speed and
    /// sample start articulators (words 4, 8, 11 and 15), `None` for none.
    fn groups(&self) -> [Option<usize>; 4] {
        [4, 8, 11, 15].map(|w| usize::try_from(self.0[w]).ok().filter(|g| *g < GROUPS))
    }

    /// The group of each of the six articulators a voice of the slice
    /// runs, when `filtered(g)` says whether group `g` has its filter on
    /// (`TriggerVoice` links the filter's two only then).
    fn links(&self, filtered: impl Fn(usize) -> bool) -> [Option<usize>; ITEMS] {
        let [amp, filter, speed, start] = self.groups();
        let filter = filter.filter(|g| filtered(*g));
        [amp, amp, filter, filter, speed, start]
    }
}

/// One deck.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Deck {
    /// The sample's path; the file, when the state does not carry it.
    pub path: String,
    /// The sample is a file and not in the state.
    pub linked: bool,
    pub settings: Vec<u8>,
    envelopes: Vec<MEnvelope>,
    pub slices: Vec<Slice>,
    pub regions: Vec<Region>,
    /// The regions are known: read from the carried file, or from the
    /// linked one.
    pub marked: bool,
    wave: Option<usize>,
}

impl Deck {
    /// The last frame of a region (`FUN_0062a6d0`): its own end, or the
    /// frame before the next region that starts after it, or the last of
    /// the sample.
    fn end(&self, index: usize, frames: i64) -> i64 {
        let region = &self.regions[index];
        if let Some(end) = region.end {
            return end;
        }
        let later = self.regions[index + 1..]
            .iter()
            .find(|r| r.start > region.start);
        later.map_or(frames - 1, |r| r.start - 1)
    }

    /// The slice settings of region `index`.
    fn slice(&self, index: usize) -> Slice {
        self.slices.get(index).copied().unwrap_or_default()
    }
}

/// One articulator group as the state has it: five words (the filter
/// envelope amount, cutoff and resonance knobs, the filter's type and its
/// stages, 0 for off) and six articulators.
#[derive(Debug, Clone, PartialEq)]
pub struct Articulator {
    pub head: [i32; 5],
    pub items: Vec<Item>,
}

/// One articulator (`TGroupArticulator`): a base, envelope and LFO
/// amounts, an envelope, an LFO and the mod X, mod Y, velocity and random
/// maps.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub knobs: [f32; 3],
    envelope: MEnvelope,
    lfo: MEnvelope,
    maps: Vec<MEnvelope>,
}

impl Item {
    /// Whether it moves its target at all.
    fn active(&self) -> bool {
        self.envelope.active || self.lfo.active
    }

    /// Whether its maps leave a value as it is: flat at 0.5 for one that
    /// adds and at 1 for one that multiplies.
    fn plain(&self, multiplies: bool) -> bool {
        let neutral = if multiplies { 1.0 } else { 0.5 };
        let flat = |map: &MEnvelope| map.flat().is_some_and(|y| (y - neutral).abs() < 1e-6);
        self.maps.iter().all(flat)
    }
}

/// An articulator made ready to run.
#[derive(Debug, Clone, PartialEq)]
struct Part {
    multiplies: bool,
    env_amount: f32,
    lfo_amount: f32,
    envelope: Envelope,
    lfo: Lfo,
    env_on: bool,
    lfo_on: bool,
}

/// A group made ready: its six articulators, the cutoff and resonance its
/// knobs add (`(knob - 128) / 256`), and its filter's type and stages less
/// one, -1 when the filter is off.
#[derive(Debug, Clone, PartialEq)]
struct Group {
    parts: Vec<Part>,
    cutoff: f32,
    resonance: f32,
    kind: u8,
    last: i32,
}

impl Group {
    fn filtered(&self) -> bool {
        self.last >= 0
    }
}

/// FL's declicker as Slicex sets it up (`FUN_008854c0` ->
/// `FUN_00876710(2, 4 ms, 1, 20 ms)`, tables by `FUN_00876030`, run by
/// `FUN_00876880`). The attack takes a share of a voice's first frame off
/// its first 4 ms; the release is a one-pole high pass whose cutoff rises
/// over 20 ms to the top of the band, with a half cosine fade over its
/// last millisecond.
#[derive(Debug, Clone, PartialEq, Default)]
struct Declicker {
    attack: Vec<f32>,
    gain: Vec<f32>,
    cut: Vec<f32>,
}

impl Declicker {
    fn new(rate: f32) -> Self {
        let sr = f64::from(rate.round() as i32);
        let frames = |ms: f64| ((ms * 0.001 * sr).round_ties_even() as i64).max(1) as usize;
        let n = frames(4.0);
        let step = 1.0 / (n as f64 + 1.0);
        let fade = |i: usize, step: f64| (((i as f64 * step * PI).cos() + 1.0) * 0.5) as f32;
        let attack = (1..=n).map(|i| fade(i, step)).collect();
        let n = frames(20.0);
        let m = n.min((sr * 0.001).round_ties_even() as usize);
        let step = 1.0 / (m as f64 + 1.0);
        let from = n - m;
        let rise = 22_050f64.ln() / (2f64.ln() * n as f64);
        let base = 2.0 / sr;
        let mut gain = Vec::with_capacity(n);
        let mut cut = Vec::with_capacity(n);
        let mut level = 1.0;
        for i in 1..=n {
            cut.push((base * (i as f64 * rise * LN_2).exp()).min(1.0) as f32);
            if i > from {
                level = fade(i - from, step);
            }
            gain.push(level);
        }
        Declicker { attack, gain, cut }
    }

    /// The release's length in frames.
    fn release_len(&self) -> usize {
        self.gain.len()
    }

    /// Run over a voice's interleaved frames in place.
    fn run(&self, state: &mut Declick, frames: &mut [f32]) {
        let len = frames.len() / 2;
        if state.attack < self.attack.len() {
            if state.attack == 0 {
                state.first = [-frames[0], -frames[1]];
            }
            let n = (self.attack.len() - state.attack).min(len);
            for (k, pair) in frames[..2 * n]
                .as_chunks_mut::<2>()
                .0
                .iter_mut()
                .enumerate()
            {
                let share = self.attack[state.attack + k];
                pair[0] += share * state.first[0];
                pair[1] += share * state.first[1];
            }
            state.attack += n;
        }
        let Some(mut at) = state.release else {
            return;
        };
        let mut from = 0;
        if state.delay > 0 {
            from = state.delay.min(len);
            state.delay -= from;
            if from >= len {
                return;
            }
        }
        let last = self.release_len() - 1;
        for pair in frames[2 * from..].as_chunks_mut::<2>().0 {
            let i = at.min(last);
            let (gain, cut) = (self.gain[i], self.cut[i]);
            at += 1;
            for (x, dc) in pair.iter_mut().zip(&mut state.dc) {
                *dc += cut * (*x - *dc);
                *x = (*x - *dc) * gain;
            }
        }
        state.release = Some(at);
    }
}

/// Where a voice is in the declicker (voice+0xfc).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Declick {
    /// Frames of the attack gone, `usize::MAX` for none.
    attack: usize,
    first: [f32; 2],
    /// Frames of the release gone, once it has started, and the frames it
    /// waits first.
    release: Option<usize>,
    delay: usize,
    dc: [f32; 2],
}

impl Declick {
    /// `FUN_00876830`: the attack runs for a voice that starts past its
    /// region's first frame.
    fn new(attack: bool) -> Self {
        Declick {
            attack: if attack { 0 } else { usize::MAX },
            first: [0.0; 2],
            release: None,
            delay: 0,
            dc: [0.0; 2],
        }
    }

    /// Start the release `delay` frames into the next run, and end the
    /// attack.
    fn release(&mut self, delay: usize) {
        if self.release.is_none() {
            self.attack = usize::MAX;
            self.release = Some(0);
            self.delay = delay;
        }
    }
}

/// Where a voice is in one articulator.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Run {
    group: usize,
    env: EnvRun,
    lfo: Option<LfoRun>,
    value: f64,
}

impl Run {
    fn over(&self) -> bool {
        self.env.over() && self.lfo.is_none_or(|lfo| lfo.over())
    }
}

/// One slice that is sounding (`TriggerVoice`'s 0x97c bytes).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Voice {
    note: u32,
    wave: usize,
    /// The region's first frame in the file and how many frames it has.
    start: usize,
    len: usize,
    /// Where the player is, counted from the region's start, and how fast
    /// the file goes by at the voice's pitch: the file's rate over the
    /// output's, less than 0 for a reversed slice.
    pos: f64,
    speed: f64,
    reader: Interpolator,
    /// The slice's pitch less the key's, and the note's pitch from key 60
    /// (`FinalLevels.Pitch`), in cents.
    cents: f32,
    pitch: f32,
    /// The slice's pan, volume, amp amount, cutoff, resonance and filter
    /// amount, and the note's pan and volume.
    pan: f32,
    volume: f32,
    amp: f32,
    cutoff: f32,
    resonance: f32,
    filter_amount: f32,
    note_pan: f32,
    note_volume: f32,
    runs: [Option<Run>; ITEMS],
    /// The filter's group, when it is on.
    filter: Option<usize>,
    stages: [Biquad; STAGES],
    state: i32,
    /// Samples to the next control tick: less than 0 before the first.
    countdown: i32,
    /// Samples before the voice starts.
    delay: i32,
    /// The gains (left, right, right into left, left into right) the last
    /// tick aimed at, where their ramp ends, where it is and its step.
    target: [f32; 4],
    end: [f32; 4],
    now: [f32; 4],
    step: [f32; 4],
    first: bool,
    declick: Declick,
}

/// What a voice reads of the plugin as it runs.
struct Setup<'a> {
    groups: &'a [Group],
    declicker: &'a Declicker,
    bank: &'a Bank,
    tick: i32,
    rates: [f64; 2],
    cap: f32,
    volume: f32,
    cents: f32,
    scale: f64,
}

/// Slicex.
#[derive(Debug, Clone, PartialEq)]
pub struct Slicex {
    /// The eight knobs: volume (word 2, 12800 for unity), randomness, LFO
    /// level (128 for full), pitch in cents (word 5), mod X and mod Y.
    pub knobs: [i32; 8],
    pub settings: Vec<u8>,
    words: [i32; 3],
    pub decks: [Deck; 2],
    pub articulators: Vec<Articulator>,
    groups: Vec<Group>,
    bank: Bank,
    /// The slices each key plays on each deck: deck and region.
    keys: [Vec<Vec<(usize, usize)>>; 2],
    voices: Vec<Voice>,
    rate: f32,
    tempo: f32,
    /// The control tick, the articulators' rates (half seconds and beats a
    /// tick) and the steepest a gain ramps a sample.
    tick: i32,
    rates: [f64; 2],
    cap: f32,
    declicker: Declicker,
    /// A voice's frames of one tick, interleaved.
    work: Vec<f32>,
    /// FL's `Plugins/Fruity/Generators`, for the regions of linked files.
    resources: Option<PathBuf>,
    /// A linked file whose regions could not be read.
    unmarked: bool,
}

impl Default for Slicex {
    fn default() -> Self {
        let mut plugin = Slicex {
            knobs: [0, 0, 12800, 256, 128, 0, 256, 256],
            settings: vec![0; 0x2f],
            words: [0; 3],
            decks: [Deck::default(), Deck::default()],
            articulators: Vec::new(),
            groups: Vec::new(),
            bank: Bank::default(),
            keys: [vec![Vec::new(); 128], vec![Vec::new(); 128]],
            voices: Vec::with_capacity(VOICES),
            rate: 44_100.0,
            tempo: 120.0,
            tick: 48,
            rates: [0.0; 2],
            cap: 0.0,
            declicker: Declicker::default(),
            work: Vec::new(),
            resources: None,
            unmarked: false,
        };
        plugin.set_sample_rate(44_100.0);
        plugin
    }
}

/// A Delphi short string as Slicex writes one: a length byte, or 0xff and a
/// u32 length, then the letters. Control characters at its end go.
fn string(reader: &mut Reader<'_>) -> Result<String, StateError> {
    let mut len = u32::from(reader.u8()?);
    if len == 0xff {
        len = reader.u32()?;
    }
    let bytes = reader.bytes(len as usize)?;
    let text = sampler::text(bytes, false);
    Ok(text
        .trim_end_matches(|c: char| (c as u32) < 0x20)
        .to_owned())
}

fn words<const N: usize>(reader: &mut Reader<'_>) -> Result<[i32; N], StateError> {
    let mut out = [0; N];
    for word in &mut out {
        *word = reader.i32()?;
    }
    Ok(out)
}

/// FL's knob curve, `(11^x - 1) / 10` and 1 at 1, kept to the sign of
/// `x` (`FUN_0087efe0`, with its single precision `ln 11` and `0.1`).
fn knob_curve(x: f32) -> f32 {
    let size = x.abs();
    let level = if size == 1.0 {
        1.0
    } else {
        let e = f64::from(size * 11f32.ln()).exp();
        ((e - 1.0) * f64::from(0.1f32)) as f32
    };
    level * if x == 0.0 { 0.0 } else { x.signum() }
}

/// A WAV file's audio, with FL's view of its regions. `None` for a file
/// that is not PCM or float.
fn decode_wav(bytes: &[u8]) -> Option<(Wave, Vec<Region>)> {
    if bytes.get(..4)? != b"RIFF" || bytes.get(8..12)? != b"WAVE" {
        return None;
    }
    let mut format = None;
    let mut data: Option<&[u8]> = None;
    for (id, body) in riff(bytes) {
        match id {
            b"fmt " => format = Some(body),
            b"data" => data = Some(body),
            _ => {}
        }
    }
    let format = format?;
    let tag = u16::from_le_bytes([*format.first()?, *format.get(1)?]);
    let channels = usize::from(u16::from_le_bytes([*format.get(2)?, *format.get(3)?]));
    let rate = u32::from_le_bytes(format.get(4..8)?.try_into().ok()?);
    let bits = u16::from_le_bytes([*format.get(14)?, *format.get(15)?]);
    let tag = if tag == 0xfffe {
        u16::from_le_bytes([*format.get(24)?, *format.get(25)?])
    } else {
        tag
    };
    let width = usize::from(bits / 8);
    if channels == 0 || width == 0 {
        return None;
    }
    let data = data?;
    let frames = data.len() / (width * channels);
    let read = |at: usize| -> f32 {
        let b = &data[at..at + width];
        match (tag, bits) {
            (1, 8) => (f32::from(b[0]) - 128.0) / 128.0,
            (1, 16) => f32::from(i16::from_le_bytes([b[0], b[1]])) / 32_768.0,
            (1, 24) => (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0,
            (1, 32) => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
            (3, 32) => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            (3, 64) => f64::from_le_bytes(b.try_into().unwrap_or([0; 8])) as f32,
            _ => 0.0,
        }
    };
    if !matches!((tag, bits), (1, 8 | 16 | 24 | 32) | (3, 32 | 64)) {
        return None;
    }
    let side = |channel: usize| -> Vec<f32> {
        (0..frames)
            .map(|f| read((f * channels + channel) * width))
            .collect()
    };
    let wave = Wave {
        left: side(0),
        right: (channels > 1).then(|| side(1)),
        rate,
    };
    Some((wave, regions(bytes)))
}

/// The chunks of a RIFF file, each id and body.
fn riff(bytes: &[u8]) -> impl Iterator<Item = (&[u8], &[u8])> {
    let mut at = 12;
    std::iter::from_fn(move || {
        let id = bytes.get(at..at + 4)?;
        let size = u32::from_le_bytes(bytes.get(at + 4..at + 8)?.try_into().ok()?) as usize;
        let body = bytes.get(at + 8..(at + 8).saturating_add(size).min(bytes.len()))?;
        at = at + 8 + size + (size & 1);
        Some((id, body))
    })
}

/// A RIFF file's regions: its cue points in the order the chunk lists
/// them, each with the name its `labl` gives, the length its `ltxt` gives
/// and the key its `tlst` gives.
fn regions(bytes: &[u8]) -> Vec<Region> {
    let u32_at = |b: &[u8], at: usize| {
        b.get(at..at + 4)
            .map_or(0, |w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
    };
    let mut cues: Vec<(u32, Region)> = Vec::new();
    let mut names = Vec::new();
    let mut lengths = Vec::new();
    let mut keys = Vec::new();
    for (id, body) in riff(bytes) {
        match id {
            b"cue " => {
                let count = u32_at(body, 0) as usize;
                for k in 0..count.min(body.len() / 24) {
                    let at = 4 + 24 * k;
                    let region = Region {
                        start: i64::from(u32_at(body, at + 20)),
                        end: None,
                        name: String::new(),
                        key: -1,
                    };
                    cues.push((u32_at(body, at), region));
                }
            }
            b"LIST" if body.get(..4) == Some(b"adtl") => {
                let mut at = 4;
                while let Some(sub) = body.get(at..at + 8) {
                    let size = u32_at(sub, 4) as usize;
                    let Some(inner) = body.get(at + 8..(at + 8 + size).min(body.len())) else {
                        break;
                    };
                    match &sub[..4] {
                        b"labl" => names.push((
                            u32_at(inner, 0),
                            sampler::text(inner.get(4..).unwrap_or_default(), false),
                        )),
                        b"ltxt" => lengths.push((u32_at(inner, 0), u32_at(inner, 4))),
                        _ => {}
                    }
                    at += 8 + size + (size & 1);
                }
            }
            b"LIST" if body.get(..4) == Some(b"tlst") => {
                let count = u32_at(body, 4) as usize;
                for k in 0..count {
                    let at = 8 + 24 * k;
                    if let Some(entry) = body.get(at..at + 24) {
                        keys.push((u32_at(entry, 4) + 1, i32::from(entry[13])));
                    }
                }
            }
            _ => {}
        }
    }
    for (id, region) in &mut cues {
        if let Some((_, name)) = names.iter().find(|(n, _)| n == id) {
            region.name.clone_from(name);
        }
        if let Some((_, len)) = lengths
            .iter()
            .find(|(n, _)| n == id)
            .filter(|(_, l)| *l > 0)
        {
            region.end = Some(region.start + i64::from(*len) - 1);
        }
        if let Some((_, key)) = keys.iter().find(|(n, _)| n == id) {
            region.key = *key;
        }
    }
    cues.into_iter().map(|(_, region)| region).collect()
}

/// The key the regions without one count up from (`FUN_0062ab40`).
fn base_key(regions: usize) -> i32 {
    let n = regions as i32;
    if n - 0x41 < 1 {
        return 60;
    }
    (60 - (n - 0x36) / 12 * 12).max(0)
}

/// Walk `parts` down from `root`, matching names without regard to case.
fn find(root: &Path, parts: &[&str]) -> Option<PathBuf> {
    let mut at = root.to_path_buf();
    for part in parts {
        if !matches!(
            Path::new(part).components().next(),
            Some(Component::Normal(_))
        ) {
            return None;
        }
        let exact = at.join(part);
        at = if exact.exists() {
            exact
        } else {
            let entries = std::fs::read_dir(&at).ok()?.filter_map(Result::ok);
            let same =
                |e: &std::fs::DirEntry| e.file_name().to_string_lossy().eq_ignore_ascii_case(part);
            entries.into_iter().find(same)?.path()
        };
    }
    at.is_file().then_some(at)
}

/// The file a state's path names under FL's install: the placeholders in
/// front go, and the rest is looked for under `Data` and under the install.
fn locate(install: &Path, path: &str) -> Option<PathBuf> {
    let path = path.replace('\\', "/");
    let mut parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    while parts
        .first()
        .is_some_and(|p| p.len() > 2 && p.starts_with('%') && p.ends_with('%'))
    {
        parts.remove(0);
    }
    let data = install.join("Data");
    find(&data, &parts)
        .or_else(|| {
            parts
                .split_first()
                .filter(|(f, _)| f.eq_ignore_ascii_case("data"))
                .and_then(|(_, rest)| find(&data, rest))
        })
        .or_else(|| find(install, &parts))
}

/// A frame of a region as FL's player reads it, the region's last frame
/// held past its end.
fn frame(wave: &Wave, start: usize, len: usize, pos: f64, reader: &Interpolator) -> (f32, f32) {
    let region = start..start + len;
    if reader.speed() == 1.0 && pos.fract() == 0.0 && pos >= len as f64 && len > 0 {
        return wave.read_in(region, (len - 1) as f64, reader);
    }
    wave.read_in(region, pos, reader)
}

/// `FUN_00883cd0`: a voice filter's stages from its cutoff and resonance,
/// 0 to 1: the cutoff on an exponential curve, the resonance as a Q (low
/// and high pass, rooted over the stages) or a bandwidth in octaves (band
/// pass and notch, times the stages, or over them for the notch). `scale`
/// is 44100 over the rate.
fn design(stages: &mut [Biquad], kind: u8, cut: f64, res: f64, scale: f64) {
    let grow = |x: f64, base: f64| ((f64::from(x as f32) * base.ln()).exp() - 1.0) as f32;
    let count = stages.len() as f64;
    let turn = scale * TAU;
    let at = |f: f64, offset: f64| ((f + offset) * turn).clamp(0.0, 2.849_517_146_113_191);
    match kind {
        0 | 3 => {
            let (factor, offset, top) = if kind == 0 {
                (0.000_664_666_644_297_540_2, 0.003, f64::INFINITY)
            } else {
                (0.000_399_933_342_123_404_15, 0.0001, 0.6001)
            };
            let f = if cut > 0.0 {
                (f64::from(grow(cut, 1501.0)) * factor).min(top)
            } else {
                0.0
            };
            let mut q = if res <= 0.0 {
                0.1
            } else if res >= 1.0 {
                50.0
            } else {
                f64::from(grow(res, 2001.0)) * 0.024_950_001_388_788_223 + 0.1
            };
            if stages.len() > 1 {
                q = (f64::from(q as f32).ln() * f64::from((1.0 / count) as f32)).exp() as f32
                    as f64;
            }
            let q = 0.5 / q;
            let (sin, cos) = at(f, offset).sin_cos();
            for stage in stages {
                stage.design(i32::from(kind) + 1, 0.0, sin * q, sin, cos);
            }
        }
        1 | 2 => {
            let f = if cut > 0.0 {
                (f64::from(grow(cut, 1501.0)) * 0.000_399_933_342_123_404_15).min(1.0001)
            } else {
                0.0
            };
            let bw = if res <= 0.0 {
                0.01
            } else if res >= 1.0 {
                12.0
            } else {
                f64::from(grow(res, 101.0)) * 0.119_899_995_625_019_07 + 0.01
            };
            let width = if kind == 2 { 1.0 / count } else { count };
            let width = f64::from((width * (LN_2 / 2.0)) as f32);
            let w = at(f, 0.0001);
            let (sin, cos) = w.sin_cos();
            let alpha = if sin > 0.0 {
                sin * (bw * width * w / sin).sinh()
            } else {
                0.0
            };
            for stage in stages {
                stage.design(i32::from(kind) + 1, 0.0, alpha, sin, cos);
            }
        }
        _ => {}
    }
}

impl Slicex {
    fn read(bytes: &[u8]) -> Result<Self, StateError> {
        let mut reader = Reader::new(bytes);
        let mut version = reader.i32()?;
        if version > WRITTEN {
            return Err(StateError::Version { found: version });
        }
        if version > 0xffff {
            version >>= 16;
        }
        let mut plugin = Slicex {
            knobs: words(&mut reader)?,
            ..Slicex::default()
        };
        plugin.settings = reader
            .bytes(if version < 2 { 0x2e } else { 0x2f })?
            .to_vec();
        plugin.words = words(&mut reader)?;
        for deck in &mut plugin.decks {
            let linked = version >= 2 && reader.u8()? != 0;
            if linked || version > 2 {
                deck.path = string(&mut reader)?;
            }
            deck.linked = linked;
            if !linked {
                let len = reader.i32()?;
                if len > 0 {
                    let file = reader.bytes(len as usize)?;
                    if let Some((wave, regions)) = decode_wav(file) {
                        deck.regions = regions;
                        deck.wave = Some(plugin.bank.hold(wave));
                    }
                    deck.marked = true;
                }
            } else {
                deck.wave = plugin.bank.place(&deck.path);
            }
            deck.settings = reader.bytes(0x4c)?.to_vec();
            deck.envelopes = (0..4)
                .map(|_| MEnvelope::read(&mut reader, Class::Map))
                .collect::<Result<_, _>>()?;
            let count = reader.i32()?;
            let count = usize::try_from(count).map_err(|_| StateError::Length {
                expected: reader.at,
                found: bytes.len(),
            })?;
            deck.slices = (0..count)
                .map(|_| words::<17>(&mut reader).map(Slice))
                .collect::<Result<_, _>>()?;
        }
        for _ in 0..GROUPS {
            let head = words::<5>(&mut reader)?;
            let mut items = Vec::with_capacity(ITEMS);
            for _ in 0..ITEMS {
                let _version = reader.i32()?;
                let knobs = [reader.f32()?, reader.f32()?, reader.f32()?];
                let envelope = MEnvelope::read(&mut reader, Class::Envelope)?;
                let lfo = MEnvelope::read(&mut reader, Class::Lfo)?;
                let maps = (0..4)
                    .map(|_| MEnvelope::read(&mut reader, Class::Map))
                    .collect::<Result<_, _>>()?;
                items.push(Item {
                    knobs,
                    envelope,
                    lfo,
                    maps,
                });
            }
            plugin.articulators.push(Articulator { head, items });
        }
        reader.end()?;
        plugin.map_keys();
        plugin.make_groups();
        Ok(plugin)
    }

    /// The slices on each key of each deck (`FUN_008855b0`): every region
    /// with a key of its own, of both decks unless the decks are kept
    /// apart, and then the deck's others from the base key up where the
    /// key is free.
    fn map_keys(&mut self) {
        let apart = self.settings.get(0x2a).is_some_and(|b| *b != 0);
        for own in 0..2 {
            let mut keys = vec![Vec::new(); 128];
            for (d, deck) in self.decks.iter().enumerate() {
                if apart && d != own {
                    continue;
                }
                for (k, region) in deck.regions.iter().enumerate() {
                    if (0..128).contains(&region.key) {
                        keys[region.key as usize].push((d, k));
                    }
                }
            }
            let regions = &self.decks[own].regions;
            let base = base_key(regions.len());
            for (k, region) in regions.iter().enumerate() {
                let key = base + k as i32;
                if region.key == -1 && (0..128).contains(&key) && keys[key as usize].is_empty() {
                    keys[key as usize].push((own, k));
                }
            }
            self.keys[own] = keys;
        }
    }

    /// The groups made ready from the state's articulators.
    fn make_groups(&mut self) {
        self.groups = self
            .articulators
            .iter()
            .map(|art| {
                let parts = art.items.iter().enumerate().map(|(i, item)| {
                    let centre = if MULTIPLIES[i] { 0.0 } else { 0.5 };
                    let envelope = Envelope::new(&item.envelope, centre);
                    Part {
                        multiplies: MULTIPLIES[i],
                        env_amount: item.knobs[1],
                        lfo_amount: item.knobs[2],
                        env_on: envelope.active,
                        envelope,
                        lfo: Lfo::new(&item.lfo),
                        lfo_on: item.lfo.active,
                    }
                });
                Group {
                    parts: parts.collect(),
                    cutoff: 0.0,
                    resonance: 0.0,
                    kind: 0,
                    last: -1,
                }
            })
            .collect();
        self.refresh();
    }

    /// What the knobs and the groups' words make of the groups, as
    /// `ProcessParam` leaves them once a state has loaded and every
    /// parameter has been set again (`FUN_0087efe0` and `ProcessParam`'s
    /// group branch): every LFO amount is the LFO level knob's, and the
    /// filter cutoff articulator's envelope amount is the group's first
    /// knob over 128. This allocates nothing.
    fn refresh(&mut self) {
        let level = knob_curve(self.knobs[4] as f32 * OVER_128);
        for (group, art) in self.groups.iter_mut().zip(&self.articulators) {
            let [amount, cutoff, resonance, kind, stages] = art.head;
            for part in &mut group.parts {
                part.lfo_amount = level;
            }
            if let Some(part) = group.parts.get_mut(2) {
                part.env_amount = amount as f32 * OVER_128;
                part.lfo_amount = part.env_amount * level;
            }
            group.cutoff = (cutoff - 0x80) as f32 * OVER_256;
            group.resonance = (resonance - 0x80) as f32 * OVER_256;
            group.kind = kind as u8;
            group.last = (stages.min(STAGES as i32)) - 1;
            if group.kind > 3 {
                group.last = -1;
            }
        }
    }

    /// The plugin's volume: knob 2 over 12800, through FL's knob curve.
    fn volume(&self) -> f32 {
        knob_curve(self.knobs[2] as f32 * OVER_12800)
    }

    /// The control tick (`FUN_008853e0`: 48 samples at 44.1 kHz, or 4
    /// with the fine setting, rounded to 4), the articulators' rates
    /// (`FUN_00885300`) and the gains' steepest ramp (the host's +0x38).
    fn retime(&mut self) {
        let sr = self.rate.round() as i32;
        let base = if self.settings.get(41).is_some_and(|b| *b != 0) {
            4
        } else {
            48
        };
        let x = f64::from(base * sr) / 44_100.0;
        self.tick = (((x / 4.0).round_ties_even() as i32) * 4).max(4);
        let unit = f64::from(self.tick * base) / (x * 96.0);
        let per_tick = unit * 96.0 / 44_100.0;
        self.rates = [per_tick * 2.0, per_tick * f64::from(self.tempo) / 60.0];
        self.cap = ((44_100.0 / f64::from(sr)) * 0.001) as f32 * 10.0;
    }

    /// The regions of a linked deck, read from its file.
    fn mark(&mut self, path: &str) {
        let Some(install) = self
            .resources
            .as_deref()
            .and_then(|dir| dir.parent()?.parent()?.parent())
        else {
            return;
        };
        let Some(file) = locate(install, path) else {
            return;
        };
        let Ok(bytes) = std::fs::read(file) else {
            return;
        };
        for deck in self.decks.iter_mut().filter(|d| d.linked && d.path == path) {
            deck.regions = regions(&bytes);
            deck.marked = true;
        }
        self.map_keys();
    }

    /// The slices a state can play: each deck's that have a region and a
    /// sample.
    fn playable(&self) -> impl Iterator<Item = Slice> + '_ {
        self.decks
            .iter()
            .filter(|d| d.wave.is_some())
            .flat_map(|d| (0..d.regions.len()).map(|k| d.slice(k)))
    }

    /// The range of a parameter's control.
    fn range(index: usize) -> Option<(i32, i32)> {
        if index < 8 {
            return Some(KNOB_RANGES[index]);
        }
        let k = (index - 8) % GROUP_PARAMS;
        (index < PARAMS).then(|| {
            if k < 3 {
                HEAD_RANGE
            } else {
                ART_RANGES[(k - 3) % 8]
            }
        })
    }

    /// Start a voice of region `k` of deck `d` (`TriggerVoice`).
    fn trigger(&mut self, note: &NoteOn, d: usize, k: usize, share: f32, reversed: bool) {
        let deck = &self.decks[d];
        let Some(place) = deck.wave else {
            return;
        };
        let Some(wave) = self.bank.wave(place) else {
            return;
        };
        let frames = wave.len() as i64;
        let start = deck.regions[k].start.clamp(0, frames);
        let end = deck.end(k, frames).clamp(start, frames);
        // FL's player is given `end - start + 1` frames: the last sounds.
        let len = ((end + 1).min(frames) - start).max(0) as usize;
        let last = end - start;
        let slice = deck.slice(k);
        let groups = &self.groups;
        let filtered = |g: usize| groups.get(g).is_some_and(Group::filtered);
        let links = slice.links(filtered);
        let mut runs = [None; ITEMS];
        for (i, link) in links.iter().enumerate() {
            let Some(group) = link.and_then(|g| groups.get(g).map(|grp| (g, grp))) else {
                continue;
            };
            let part = &group.1.parts[i];
            runs[i] = Some(Run {
                group: group.0,
                env: part
                    .envelope
                    .start(self.rates[usize::from(part.envelope.tempo)]),
                lfo: part
                    .lfo_on
                    .then(|| part.lfo.start(self.rates[usize::from(part.lfo.tempo())])),
                value: STARTS_AT[i],
            });
        }
        let filter = links[2].filter(|g| filtered(*g));
        let sr = self.rate.round() as i32;
        let delay = slice.delay();
        let delay = if delay <= 0.0 {
            0
        } else {
            (f64::from(delay * sr as f32 * 60.0 / self.tempo)).round_ties_even() as i32
        };
        // The sample start: up to 100 ms of the region, or all of it with
        // the setting at +0x1318.
        let from = f64::from(slice.start());
        let mut offset = if from <= 0.0 {
            0.0
        } else if self.settings.get(40).is_some_and(|b| *b != 0) {
            from.min(1.0) * (last + 1) as f64
        } else {
            from * (f64::from(sr) * 0.1).min((last + 1) as f64)
        };
        if reversed {
            offset = last as f64 - offset;
        }
        let pos = offset.clamp(0.0, len as f64);
        // The voice keeps the speed as an f32 (+0xb4): the file's rate
        // times the unit's 1 / rate.
        let speed = f64::from((f64::from(wave.rate) * (1.0 / f64::from(sr))) as f32);
        if self.voices.len() == VOICES {
            self.voices.remove(0);
        }
        self.voices.push(Voice {
            note: note.voice,
            wave: place,
            start: start as usize,
            len,
            pos,
            speed: if reversed { -speed } else { speed },
            reader: sampler::reader(speed),
            cents: slice.pitch() + ((60 - i32::from(note.key)) * 100) as f32,
            pitch: (i32::from(note.key) - 60) as f32 * 100.0 + note.fine_pitch,
            pan: slice.pan(),
            volume: slice.volume() * share,
            amp: slice.amp_amount(),
            cutoff: slice.cutoff(),
            resonance: slice.resonance(),
            filter_amount: slice.filter_amount(),
            note_pan: note.pan,
            note_volume: note.velocity,
            runs,
            filter,
            stages: [Biquad::default(); STAGES],
            state: PLAYING,
            countdown: -self.tick,
            delay,
            target: [0.0; 4],
            end: [0.0; 4],
            now: [0.0; 4],
            step: [0.0; 4],
            first: true,
            declick: Declick::new(pos > 0.0),
        });
    }
}

impl Voice {
    /// A control tick (`FUN_008843d0`'s first part): the articulators move
    /// on, a voice let go of whose articulators are all over starts its
    /// release, and the gains are aimed anew.
    fn tick(&mut self, setup: &Setup<'_>) {
        let dt = if self.countdown < 0 { 0.0 } else { 1.0 };
        self.countdown = setup.tick;
        if self.state > 4 {
            self.state = ENDING;
        }
        for (i, run) in self.runs.iter_mut().enumerate() {
            let Some(run) = run else {
                continue;
            };
            let part = &setup.groups[run.group].parts[i];
            run.value = STARTS_AT[i];
            if part.env_on {
                let rate = setup.rates[usize::from(part.envelope.tempo)];
                part.envelope.advance(&mut run.env, dt, rate);
                let out = f64::from(run.env.out);
                run.value = if part.multiplies {
                    run.value * out
                } else {
                    run.value + out * f64::from(part.env_amount)
                };
            }
            if let Some(lfo) = &mut run.lfo {
                part.lfo
                    .advance(lfo, dt, setup.rates[usize::from(part.lfo.tempo())]);
                let swing = f64::from(lfo.out) * f64::from(part.lfo_amount);
                run.value = if part.multiplies {
                    run.value * (swing * 2.0 + 1.0)
                } else {
                    run.value + swing
                };
            }
            if self.state > 4 && !run.over() {
                self.state = RELEASED;
            }
        }
        if self.state > 8 {
            self.declick.release(0);
        }
        let value = |i: usize| self.runs[i].map_or(STARTS_AT[i], |run| run.value);
        let amp = f64::from(self.amp);
        let pan = (f64::from(self.pan + self.note_pan) + value(0) * 2.0 * amp) as f32;
        let level = self.volume * self.note_volume * setup.volume;
        let volume = (f64::from(level) * ((value(1) - 1.0) * amp + 1.0)) as f32;
        self.target = stereo_pan(pan.clamp(-1.0, 1.0), volume);
        if self.first {
            self.end = self.target;
            self.first = false;
        }
        for g in 0..4 {
            ramp(
                &mut self.step[g],
                &mut self.end[g],
                &mut self.now[g],
                self.target[g],
                setup.cap,
                setup.tick,
            );
        }
    }

    /// Render the voice's part of a block and add it to `left` and
    /// `right` (`FUN_008843d0`).
    fn render(&mut self, setup: &Setup<'_>, work: &mut [f32], left: &mut [f32], right: &mut [f32]) {
        if self.state >= OVER {
            return;
        }
        let Some(wave) = setup.bank.wave(self.wave) else {
            self.state = OVER;
            return;
        };
        let len = left.len();
        let mut at = 0;
        if self.delay > 0 {
            self.delay -= len as i32;
            if self.delay >= 0 {
                return;
            }
            at = (len as i32 + self.delay) as usize;
            self.delay = 0;
        }
        while at < len {
            if self.countdown < 1 {
                self.tick(setup);
            }
            // A tick is never longer than the buffer, unless the rate fell
            // under a sounding voice.
            let n = (self.countdown as usize).min(len - at).min(work.len() / 2);
            // FUN_006200d0, with the unit's `ln 2 / 12 / 100`.
            let cents = self.cents + self.pitch + setup.cents;
            let step = (f64::from(cents) * (LN_2 / 12.0 / 100.0)).exp() * self.speed;
            if step != self.reader.speed() {
                self.reader.set_speed(step);
            }
            let frames = &mut work[..2 * n];
            let mut short = None;
            for (k, pair) in frames.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                if self.pos < 0.0 {
                    self.pos = 0.0;
                }
                if short.is_none() && step > 0.0 && self.pos >= self.len as f64 {
                    short = Some(k);
                }
                let (l, r) = frame(wave, self.start, self.len, self.pos, &self.reader);
                pair[0] = l;
                pair[1] = r;
                self.pos += step;
            }
            if self.state < ENDING {
                if let Some(k) = short {
                    self.state = ENDING;
                    self.declick.release(k);
                } else if self.pos <= 0.0 {
                    self.state = ENDING;
                    self.declick.release(0);
                }
            }
            if let Some(g) = self.filter {
                let group = &setup.groups[g];
                let value = |i: usize| self.runs[i].map_or(0.0, |run| run.value);
                let amount = f64::from(self.filter_amount);
                let cut = f64::from(self.cutoff) + (f64::from(group.cutoff) + value(2)) * amount;
                let res =
                    f64::from(self.resonance) + (f64::from(group.resonance) + value(3)) * amount;
                let stages = &mut self.stages[..=group.last as usize];
                design(stages, group.kind, cut, res, setup.scale);
                for stage in stages {
                    stage.run(frames);
                }
            }
            setup.declicker.run(&mut self.declick, frames);
            self.mix(frames, &mut left[at..at + n], &mut right[at..at + n]);
            self.countdown -= n as i32;
            at += n;
        }
        if self.state >= ENDING
            && self
                .declick
                .release
                .is_some_and(|r| r >= setup.declicker.release_len())
        {
            self.state = OVER;
        }
    }

    /// Add frames at the voice's gains, which ramp by their step a frame
    /// (the mixer at `0x91b5c0`, `FUN_00635d10`): the left and right gains
    /// and, with the pan off the centre, the part of each side sent across.
    fn mix(&mut self, frames: &[f32], left: &mut [f32], right: &mut [f32]) {
        let [mut ll, mut rr, mut rl, mut lr] = self.now;
        let [dll, drr, drl, dlr] = self.step;
        for ((pair, l), r) in frames
            .as_chunks::<2>()
            .0
            .iter()
            .zip(left.iter_mut())
            .zip(right.iter_mut())
        {
            *l += pair[0] * ll + pair[1] * rl;
            *r += pair[1] * rr + pair[0] * lr;
            ll += dll;
            rr += drr;
            rl += drl;
            lr += dlr;
        }
        let n = left.len() as f32;
        for (now, step) in self.now.iter_mut().zip(self.step) {
            *now += step * n;
        }
    }

    /// Voice_Release (`FUN_00883970`): the articulators let go, and a
    /// voice still waiting to start ends.
    fn release(&mut self, groups: &[Group], rates: [f64; 2]) {
        if self.state < RELEASED {
            self.state = RELEASED;
        }
        for (i, run) in self.runs.iter_mut().enumerate() {
            let Some(run) = run else {
                continue;
            };
            let part = &groups[run.group].parts[i];
            part.envelope.release(&mut run.env, 1.0);
            if let Some(lfo) = &mut run.lfo {
                part.lfo.release(lfo, rates[usize::from(part.lfo.tempo())]);
            }
        }
        if self.delay > 0 {
            self.state = OVER;
        }
    }
}

impl Generator for Slicex {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let read = Self::read(bytes)?;
        *self = Slicex {
            rate: self.rate,
            tempo: self.tempo,
            resources: self.resources.clone(),
            ..read
        };
        let rate = self.rate;
        self.set_sample_rate(rate);
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        self.rate = hz;
        self.retime();
        self.declicker = Declicker::new(hz);
        self.work = vec![0.0; 2 * self.tick as usize];
    }

    fn set_tempo(&mut self, bpm: f64) {
        if bpm > 0.0 {
            self.tempo = bpm as f32;
            self.retime();
        }
    }

    fn set_resource_dir(&mut self, dir: &Path) {
        self.resources = Some(dir.to_path_buf());
    }

    fn note_on(&mut self, note: NoteOn) {
        if note.slide {
            return;
        }
        let deck = usize::from(note.color & 1);
        let reversed = note.color > 13;
        let Some(entries) = usize::try_from(note.key)
            .ok()
            .and_then(|k| self.keys[deck].get(k))
        else {
            return;
        };
        let layers = entries.len();
        let mode = self.settings.first().copied().unwrap_or(0);
        let share = if mode == 1 && layers > 1 {
            (1.0 / layers as f64) as f32
        } else {
            1.0
        };
        for at in 0..layers {
            let (d, k) = self.keys[deck][note.key as usize][at];
            self.trigger(&note, d, k, share, reversed);
        }
    }

    fn note_off(&mut self, voice: u32) {
        let Slicex {
            voices,
            groups,
            rates,
            ..
        } = self;
        for v in voices.iter_mut().filter(|v| v.note == voice) {
            v.release(groups, *rates);
        }
    }

    fn all_notes_off(&mut self) {
        let Slicex {
            voices,
            groups,
            rates,
            ..
        } = self;
        for v in voices.iter_mut() {
            v.release(groups, *rates);
        }
    }

    fn set_note_pitch(&mut self, voice: u32, pitch: f32) {
        for v in self.voices.iter_mut().filter(|v| v.note == voice) {
            v.pitch = pitch;
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        let setup = Setup {
            groups: &self.groups,
            declicker: &self.declicker,
            bank: &self.bank,
            tick: self.tick,
            rates: self.rates,
            cap: self.cap,
            volume: self.volume(),
            cents: self.knobs[5] as f32,
            scale: f64::from((44_100.0 / f64::from(self.rate.round() as i32)) as f32),
        };
        for voice in &mut self.voices {
            voice.render(&setup, &mut self.work, &mut left[..len], &mut right[..len]);
        }
        self.voices.retain(|voice| voice.state < OVER);
    }

    fn param_count(&self) -> usize {
        PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        if index < 8 {
            return self.knobs[index] as f32;
        }
        if index >= PARAMS {
            return 0.0;
        }
        let (g, k) = ((index - 8) / GROUP_PARAMS, (index - 8) % GROUP_PARAMS);
        let Some(art) = self.articulators.get(g) else {
            return 0.0;
        };
        if k < 3 {
            return art.head[k] as f32;
        }
        let (item, j) = ((k - 3) / 8, (k - 3) % 8);
        let item = &art.items[item];
        if j < 4 {
            item.envelope.knobs[j] as f32
        } else {
            item.lfo.knobs[j - 4] as f32
        }
    }

    /// `ProcessParam` (`FUN_0087e4f0`): knobs 2 to 5 work the volume,
    /// randomness, LFO level and pitch out again, a group's knobs its
    /// filter, and an articulator's its envelope or LFO. This allocates
    /// nothing.
    fn set_param(&mut self, index: usize, value: f32) {
        let Some((low, high)) = Self::range(index) else {
            return;
        };
        let v = f64::from(value)
            .clamp(f64::from(low), f64::from(high))
            .round_ties_even() as i32;
        if index < 8 {
            self.knobs[index] = v;
            self.refresh();
            return;
        }
        let (g, k) = ((index - 8) / GROUP_PARAMS, (index - 8) % GROUP_PARAMS);
        let (Some(art), Some(group)) = (self.articulators.get_mut(g), self.groups.get_mut(g))
        else {
            return;
        };
        if k < 3 {
            art.head[k] = v;
            self.refresh();
            return;
        }
        let (i, j) = ((k - 3) / 8, (k - 3) % 8);
        let (item, part) = (&mut art.items[i], &mut group.parts[i]);
        if j < 4 {
            item.envelope.knobs[j] = v;
            part.envelope.set_knobs(item.envelope.knobs);
        } else {
            item.lfo.knobs[j - 4] = v;
            part.lfo.set_knobs(item.lfo.knobs);
        }
    }

    fn reset(&mut self) {
        self.voices.clear();
    }

    /// The longest region at the slowest the knobs can play it, and the
    /// declicker's release.
    fn tail(&self) -> usize {
        let mut longest = 0f64;
        for deck in &self.decks {
            let Some(wave) = deck.wave.and_then(|p| self.bank.wave(p)) else {
                continue;
            };
            let frames = wave.len() as i64;
            for k in 0..deck.regions.len() {
                let length = (deck.end(k, frames) - deck.regions[k].start).max(0) as f64;
                longest = longest.max(length / f64::from(wave.rate) * 4.0);
            }
        }
        (longest * f64::from(self.rate)) as usize + self.declicker.release_len() + 1
    }

    fn fidelity(&self) -> Fidelity {
        if self.bank.lacking() {
            return Fidelity::Approximate(
                "Slicex: a sample file is missing, and its slices are silent",
            );
        }
        if self.unmarked
            || self
                .decks
                .iter()
                .any(|d| d.linked && d.wave.is_some() && !d.marked)
        {
            return Fidelity::Approximate("Slicex: a linked file's regions could not be read");
        }
        let filtered = |g: usize| self.groups.get(g).is_some_and(Group::filtered);
        let mut moved = false;
        let mut mapped = false;
        for slice in self.playable() {
            for (i, link) in slice.links(filtered).iter().enumerate() {
                let Some(item) = link.and_then(|g| self.articulators.get(g)?.items.get(i)) else {
                    continue;
                };
                moved |= i >= 4 && item.active();
                mapped |= !item.plain(MULTIPLIES[i]);
            }
        }
        if moved {
            return Fidelity::Approximate(
                "Slicex: its speed and sample start articulators are left out",
            );
        }
        if mapped {
            return Fidelity::Approximate(
                "Slicex: its articulators' modulation, velocity and random maps are left out",
            );
        }
        if self.playable().any(|s| s.0[0] > 0) {
            return Fidelity::Approximate("Slicex: its cut groups are left out");
        }
        let layered = self.keys.iter().flatten().any(|k| k.len() > 1);
        if self.settings.first().is_some_and(|m| *m > 1) && layered {
            return Fidelity::Approximate("Slicex: a layer mode that picks one slice is left out");
        }
        Fidelity::Exact
    }

    fn sample_paths(&self) -> Vec<String> {
        self.bank.paths()
    }

    fn set_sample(&mut self, path: &str, file: Option<SampleFile<'_>>) {
        let found = file.is_some();
        self.bank.set(path, file);
        let linked = self
            .decks
            .iter()
            .any(|d| d.linked && d.path == path && !d.marked);
        if found && linked {
            self.mark(path);
            self.unmarked |= self
                .decks
                .iter()
                .any(|d| d.linked && d.path == path && !d.marked);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menvelope::tests::stream;
    use crate::testing;
    use dsp::curve::Point;

    /// A WAV file of 16-bit mono frames with these cue points, `ltxt`
    /// lengths and `tlst` keys.
    fn wav(frames: &[i16], cues: &[u32], lengths: &[(u32, u32)], keys: &[(u32, u8)]) -> Vec<u8> {
        let chunk = |id: &[u8], body: &[u8]| {
            let mut out = id.to_vec();
            out.extend((body.len() as u32).to_le_bytes());
            out.extend(body);
            if body.len() % 2 == 1 {
                out.push(0);
            }
            out
        };
        let mut fmt = Vec::new();
        for (value, width) in [(1u32, 2), (1, 2), (44_100, 4), (88_200, 4), (2, 2), (16, 2)] {
            fmt.extend(&value.to_le_bytes()[..width]);
        }
        let data: Vec<u8> = frames.iter().flat_map(|f| f.to_le_bytes()).collect();
        let mut cue = (cues.len() as u32).to_le_bytes().to_vec();
        for (i, at) in cues.iter().enumerate() {
            for word in [i as u32 + 1, *at, u32::from_le_bytes(*b"data"), 0, 0, *at] {
                cue.extend(word.to_le_bytes());
            }
        }
        let mut adtl = b"adtl".to_vec();
        for (id, len) in lengths {
            let mut body = id.to_le_bytes().to_vec();
            body.extend(len.to_le_bytes());
            body.extend(b"rgn ");
            body.extend([0; 8]);
            adtl.extend(chunk(b"ltxt", &body));
        }
        adtl.extend(chunk(b"labl", b"\x01\0\0\0Kick\0"));
        let mut tlst = b"tlst".to_vec();
        tlst.extend((keys.len() as u32).to_le_bytes());
        for (id, key) in keys {
            tlst.extend(b"cue ");
            tlst.extend((id - 1).to_le_bytes());
            tlst.extend(1u32.to_le_bytes());
            tlst.extend([0xff, *key, 0x90, 0]);
            tlst.extend([0; 8]);
        }
        let mut body = b"WAVE".to_vec();
        body.extend(chunk(b"fmt ", &fmt));
        body.extend(chunk(b"data", &data));
        body.extend(chunk(b"cue ", &cue));
        body.extend(chunk(b"LIST", &adtl));
        if !keys.is_empty() {
            body.extend(chunk(b"LIST", &tlst));
        }
        chunk(b"RIFF", &body)
    }

    fn at(x: f64, y: f64) -> Point {
        Point {
            x,
            y,
            tension: 0.0,
            mode: 0,
        }
    }

    fn flat() -> Vec<u8> {
        stream(
            6,
            3,
            &[at(0.0, 0.5), at(8.0, 0.5)],
            [0, -1, -1, -1, -1],
            &[],
        )
    }

    /// What a test state puts in group 0 beside the defaults: its five
    /// words, and a volume envelope (in half seconds) that is on.
    struct Arts<'a> {
        head: [i32; 5],
        volume: Option<&'a [Point]>,
    }

    const PLAIN: Arts<'static> = Arts {
        head: [128, 128, 128, 0, 0],
        volume: None,
    };

    /// A state carrying `file` on deck A with these slice settings, and an
    /// empty deck B.
    fn state(file: &[u8], slices: &[[i32; 17]], volume: i32, pitch: i32) -> Vec<u8> {
        state_with(file, slices, volume, pitch, &PLAIN)
    }

    fn state_with(
        file: &[u8],
        slices: &[[i32; 17]],
        volume: i32,
        pitch: i32,
        arts: &Arts<'_>,
    ) -> Vec<u8> {
        let mut bytes = WRITTEN.to_le_bytes().to_vec();
        for word in [0, 0, volume, 256, 128, pitch, 256, 256] {
            bytes.extend(word.to_le_bytes());
        }
        bytes.extend([0; 0x2f]);
        bytes.extend([0; 12]);
        for (deck, file) in [(0, file), (1, &[][..])] {
            bytes.push(0);
            let path = if deck == 0 {
                &b"%FLStudioData%\\Loops\\a.wav"[..]
            } else {
                b""
            };
            bytes.push(path.len() as u8);
            bytes.extend(path);
            bytes.extend((file.len() as i32).to_le_bytes());
            bytes.extend(file);
            bytes.extend([0; 0x4c]);
            for _ in 0..4 {
                bytes.extend(flat());
            }
            let slices = if deck == 0 { slices } else { &[] };
            bytes.extend((slices.len() as i32).to_le_bytes());
            for slice in slices {
                for word in slice {
                    bytes.extend(word.to_le_bytes());
                }
            }
        }
        for g in 0..GROUPS {
            let head = if g == 0 { arts.head } else { PLAIN.head };
            for word in head {
                bytes.extend(word.to_le_bytes());
            }
            for (i, multiplies) in MULTIPLIES.iter().enumerate() {
                bytes.extend(3i32.to_le_bytes());
                for knob in [0.0f32, 1.0, 1.0] {
                    bytes.extend(knob.to_le_bytes());
                }
                let neutral = if *multiplies { 1.0 } else { 0.5 };
                let point = [at(0.0, neutral)];
                let envelope = match arts.volume {
                    Some(points) if g == 0 && i == 1 => {
                        stream(1, 3, points, [0, 1, -1, -1, -1], &[128, 128, 0, 128])
                    }
                    _ => stream(1, 3, &point, [0, 0, -1, -1, -1], &[128, 128, 0, 128]),
                };
                bytes.extend(envelope);
                bytes.extend(stream(
                    2,
                    3,
                    &point,
                    [0, 0, -1, -1, -1],
                    &[40_000, 0, 0, 0, 0],
                ));
                for _ in 0..4 {
                    bytes.extend(stream(3, 3, &point, [0, 0, -1, -1, -1], &[]));
                }
            }
        }
        bytes
    }

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

    fn play(sx: &mut Slicex, len: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut left, mut right) = (vec![0.0; len], vec![0.0; len]);
        sx.process(&mut left, &mut right);
        (left, right)
    }

    fn ramp(len: usize) -> Vec<i16> {
        (0..len).map(|n| n as i16 * 10 + 10).collect()
    }

    fn sample(frame: i16) -> f32 {
        f32::from(frame) / 32_768.0
    }

    /// What the declicker's release makes of a frame held from where it
    /// starts.
    fn faded(sx: &Slicex, held: f32) -> Vec<f32> {
        let d = &sx.declicker;
        let mut dc = 0.0;
        (0..d.release_len())
            .map(|i| {
                dc += d.cut[i] * (held - dc);
                (held - dc) * d.gain[i]
            })
            .collect()
    }

    #[test]
    fn each_key_from_60_plays_its_region_and_fades_its_held_last_frame() {
        let frames = ramp(300);
        let file = wav(&frames, &[0, 100, 200], &[], &[]);
        let mut sx = Slicex::default();
        sx.set_state(&state(&file, &[DEFAULT_SLICE; 3], 12_800, 0))
            .unwrap();
        assert!(sx.sample_paths().is_empty());
        assert_eq!(sx.decks[0].regions.len(), 3);
        assert_eq!(sx.decks[0].regions[0].name, "Kick");
        assert_eq!(sx.fidelity(), Fidelity::Exact);
        sx.note_on(note(1, 61));
        // FL hands its player the frames from the region's first to its
        // last, and the last sounds too. Past it the player holds the last
        // frame, and the declicker's release fades that out over 20 ms.
        let release = sx.declicker.release_len();
        let (left, right) = play(&mut sx, 100 + release + 30);
        for n in 0..100 {
            assert_eq!(left[n], sample(frames[100 + n]), "{n}");
            assert_eq!(left[n], right[n]);
        }
        let fade = faded(&sx, sample(frames[199]));
        assert_eq!(&left[100..100 + release], &fade[..]);
        assert!(left[100 + release..].iter().all(|v| v.abs() < 1e-7));
        assert!(sx.voices.is_empty());
        // The last region ends with the file, and key 63 has none.
        sx.note_on(note(2, 62));
        let (left, _) = play(&mut sx, 101);
        assert_eq!(left[99], sample(frames[299]));
        assert_eq!(left[100], faded(&sx, sample(frames[299]))[0]);
        sx.reset();
        sx.note_on(note(3, 63));
        assert!(play(&mut sx, 10).0.iter().all(|v| *v == 0.0));
        assert!(sx.tail() >= 100 + release);
    }

    #[test]
    fn a_note_off_fades_the_slice_out_from_the_next_control_tick() {
        let file = wav(&[16_384; 5000], &[0], &[], &[]);
        let mut sx = Slicex::default();
        sx.set_state(&state(&file, &[DEFAULT_SLICE], 12_800, 0))
            .unwrap();
        assert_eq!(sx.tick, 48);
        sx.note_on(note(7, 60));
        assert!(play(&mut sx, 100).0.iter().all(|v| *v == 0.5));
        sx.note_off(7);
        // The voice's ticks fall on 0, 48, 96 and 144: the release starts
        // 44 frames into this block.
        let (left, _) = play(&mut sx, 2000);
        assert!(left[..44].iter().all(|v| *v == 0.5));
        let fade = faded(&sx, 0.5);
        assert_eq!(&left[44..44 + fade.len()], &fade[..]);
        assert!(sx.voices.is_empty());
    }

    #[test]
    fn the_declicker_takes_the_first_frame_off_and_fades_with_a_rising_high_pass() {
        let d = Declicker::new(44_100.0);
        assert_eq!((d.attack.len(), d.release_len()), (176, 882));
        let fade = |i: f64, n: f64| (((i / (n + 1.0)) * PI).cos() + 1.0) * 0.5;
        assert!((f64::from(d.attack[0]) - fade(1.0, 176.0)).abs() < 1e-7);
        // The gain is 1 but for the release's last millisecond.
        assert!(d.gain[..838].iter().all(|g| *g == 1.0));
        assert!((f64::from(d.gain[838]) - fade(1.0, 44.0)).abs() < 1e-7);
        assert!((f64::from(d.gain[881]) - fade(44.0, 44.0)).abs() < 1e-7);
        // The one-pole's coefficient rises from 2/44100 by 22050 times,
        // to 1 at 44.1 kHz.
        let first = 2.0 / 44_100.0 * (22_050f64.ln() / 882.0).exp();
        assert!((f64::from(d.cut[0]) - first).abs() < 1e-9);
        assert!((d.cut[881] - 1.0).abs() < 1e-6);
        // A voice that starts past its first frame fades that frame in.
        let mut state = Declick::new(true);
        let mut frames = vec![1.0; 400];
        d.run(&mut state, &mut frames);
        assert!((frames[0] - (1.0 - d.attack[0])).abs() < 1e-7);
        assert!((frames[351] - (1.0 - d.attack[175])).abs() < 1e-7);
        assert!(frames[352..].iter().all(|v| *v == 1.0));
        // The release can wait for the frame a region ends on.
        let mut state = Declick::new(false);
        state.release(3);
        let mut frames = vec![1.0; 10];
        d.run(&mut state, &mut frames);
        assert!(frames[..6].iter().all(|v| *v == 1.0));
        assert!(frames[6] < 1.0);
    }

    #[test]
    fn a_volume_envelope_shapes_the_slice_and_its_end_ends_it_once_let_go() {
        let file = wav(&[16_384; 30_000], &[0], &[], &[]);
        // From 1 to 0 over half a half second: 11025 samples.
        let points = [at(0.0, 1.0), at(0.5, 0.0)];
        let arts = Arts {
            head: PLAIN.head,
            volume: Some(&points),
        };
        let mut sx = Slicex::default();
        sx.set_state(&state_with(&file, &[DEFAULT_SLICE], 12_800, 0, &arts))
            .unwrap();
        assert_eq!(sx.fidelity(), Fidelity::Exact);
        sx.note_on(note(1, 60));
        let (left, right) = play(&mut sx, 12_000);
        assert_eq!(left[0], 0.5);
        // The gains ramp to each tick's value of the envelope.
        let half = left[11_025 / 2];
        assert!((half - 0.25).abs() < 0.01, "{half}");
        assert!(left[11_100..].iter().all(|v| v.abs() < 1e-6));
        assert_eq!(left, right);
        // Not let go, the voice plays on silent; let go, it ends.
        assert_eq!(sx.voices.len(), 1);
        sx.note_off(1);
        play(&mut sx, 2000);
        assert!(sx.voices.is_empty());
    }

    #[test]
    fn a_group_s_filter_runs_on_its_slices() {
        let file = wav(&[16_384; 20_000], &[0], &[], &[]);
        let level = |kind: i32| {
            let arts = Arts {
                head: [128, 128, 128, kind, 1],
                volume: None,
            };
            let mut sx = Slicex::default();
            sx.set_state(&state_with(&file, &[DEFAULT_SLICE], 12_800, 0, &arts))
                .unwrap();
            assert!(sx.groups[0].filtered());
            sx.note_on(note(1, 60));
            play(&mut sx, 8000).0[7999]
        };
        // A low pass lets the file's constant level through, and a high
        // pass takes it away.
        assert!((level(0) - 0.5).abs() < 1e-4);
        assert!(level(3).abs() < 1e-4);
    }

    #[test]
    fn parameters_follow_processparam_and_its_controls_ranges() {
        let ramp: Vec<i16> = (0..1000).map(|n| n as i16).collect();
        let file = wav(&ramp, &[0], &[], &[]);
        let mut sx = Slicex::default();
        sx.set_state(&state(&file, &[DEFAULT_SLICE], 12_800, 0))
            .unwrap();
        assert_eq!(sx.param_count(), 352);
        sx.set_param(5, 5000.0);
        assert_eq!(sx.param(5), 1200.0);
        sx.set_param(2, f32::MIN);
        assert_eq!(sx.param(2), 0.0);
        sx.set_param(2, 12_800.4);
        assert_eq!(sx.param(2), 12_800.0);
        // Group 1's cutoff knob, and the LFO speed of its third articulator.
        let group = 8 + GROUP_PARAMS;
        sx.set_param(group + 1, 300.0);
        assert_eq!(
            (sx.param(group + 1), sx.articulators[1].head[1]),
            (256.0, 256)
        );
        assert_eq!(sx.groups[1].cutoff, 0.5);
        sx.set_param(group + 3 + 2 * 8 + 4, 1.0);
        assert_eq!(sx.param(group + 3 + 2 * 8 + 4), 512.0);
        assert_eq!(sx.param(group + 3), 128.0);
        sx.set_param(PARAMS, 1.0);
        assert_eq!(sx.param(PARAMS), 0.0);
        // The pitch knob plays an octave up.
        sx.note_on(note(1, 60));
        let (left, _) = play(&mut sx, 10);
        let source: Vec<f32> = ramp.iter().map(|v| sample(*v)).collect();
        let want = sampler::reader(2.0).frame(&source, 6, 0.0);
        assert!((left[3] - want).abs() < 1e-9, "{}", left[3] * 32_768.0);
    }

    #[test]
    fn the_colour_picks_the_deck_and_the_last_two_play_it_backwards() {
        let frames = ramp(600);
        let file = wav(&frames, &[0, 500], &[], &[]);
        let mut sx = Slicex::default();
        sx.set_state(&state(&file, &[DEFAULT_SLICE; 2], 12_800, 0))
            .unwrap();
        // Deck B has nothing.
        let mut other = note(1, 60);
        other.color = 1;
        sx.note_on(other);
        assert!(play(&mut sx, 10).0.iter().all(|v| *v == 0.0));
        // Backwards from the region's last frame, which the attack fades in.
        let mut back = note(2, 60);
        back.color = 14;
        sx.note_on(back);
        let (left, _) = play(&mut sx, 300);
        let attack = sx.declicker.attack[0];
        assert!((left[0] - sample(frames[499]) * (1.0 - attack)).abs() < 1e-5);
        assert!((left[200] - sample(frames[299])).abs() < 1e-5);
    }

    #[test]
    fn a_region_s_length_and_key_come_from_the_file() {
        let frames = ramp(300);
        let file = wav(&frames, &[0, 100], &[(1, 50)], &[(2, 36)]);
        let mut sx = Slicex::default();
        sx.set_state(&state(&file, &[DEFAULT_SLICE; 2], 12_800, 0))
            .unwrap();
        let regions = &sx.decks[0].regions;
        assert_eq!((regions[0].end, regions[1].key), (Some(49), 36));
        sx.note_on(note(1, 60));
        let (left, _) = play(&mut sx, 60);
        assert_eq!(left[49], sample(frames[49]));
        assert_eq!(left[50], faded(&sx, sample(frames[49]))[0]);
        sx.reset();
        sx.note_on(note(2, 36));
        assert_eq!(play(&mut sx, 4).0[0], sample(frames[100]));
        // The base key falls an octave for every 12 regions above 54.
        assert_eq!(
            (base_key(65), base_key(66), base_key(78), base_key(200)),
            (60, 48, 36, 0)
        );
    }

    #[test]
    fn volumes_pitches_and_pans_are_the_slice_s_and_the_knobs() {
        let frames = vec![16_384i16; 2000];
        let file = wav(&frames, &[0], &[], &[]);
        let mut loud = DEFAULT_SLICE;
        loud[3] = 16_000;
        let mut sx = Slicex::default();
        sx.set_state(&state(&file, &[loud], 25_600, 0)).unwrap();
        assert_eq!(Slice(loud).volume(), 4.0);
        assert_eq!(Slice(DEFAULT_SLICE).volume(), 1.0);
        let mut quiet = note(1, 60);
        quiet.velocity = 0.5;
        sx.note_on(quiet);
        let want = 0.5 * 4.0 * 12.0 * 0.5;
        assert!((play(&mut sx, 4).0[0] - want).abs() < 1e-5);
        // An octave of fine pitch and the plugin's pitch knob.
        let ramp: Vec<i16> = (0..1000).map(|n| n as i16).collect();
        let file = wav(&ramp, &[0], &[], &[]);
        let mut sx = Slicex::default();
        sx.set_state(&state(&file, &[DEFAULT_SLICE], 12_800, 600))
            .unwrap();
        let mut high = note(1, 60);
        high.fine_pitch = 600.0;
        sx.note_on(high);
        let (left, _) = play(&mut sx, 10);
        // Twice the speed: FL's interpolator low-passes the ramp.
        let source: Vec<f32> = ramp.iter().map(|v| sample(*v)).collect();
        let want = sampler::reader(2.0).frame(&source, 6, 0.0);
        assert!((left[3] - want).abs() < 1e-9, "{}", left[3] * 32_768.0);
        assert!((left[3] - 6.0 / 32_768.0).abs() < 1e-6);
        // Hard left takes the right side away and sends it across.
        let mut sx = Slicex::default();
        sx.set_state(&state(&file, &[DEFAULT_SLICE], 12_800, 0))
            .unwrap();
        let mut left_note = note(1, 60);
        left_note.pan = -1.0;
        sx.note_on(left_note);
        let (left, right) = play(&mut sx, 8);
        let x = sample(ramp[5]);
        assert!((left[5] - x * std::f32::consts::SQRT_2).abs() < 1e-7);
        assert!(right[5].abs() < 1e-9);
    }

    #[test]
    fn a_state_fl_does_not_write_is_turned_down() {
        let file = wav(&ramp(10), &[0], &[], &[]);
        let good = state(&file, &[DEFAULT_SLICE], 12_800, 0);
        let mut sx = Slicex::default();
        sx.set_state(&good).unwrap();
        let kept = sx.clone();
        let mut newer = good.clone();
        newer[..4].copy_from_slice(&0x60004i32.to_le_bytes());
        assert_eq!(
            sx.set_state(&newer),
            Err(StateError::Version { found: 0x60004 })
        );
        for len in [3, 40, good.len() - 1] {
            assert!(sx.set_state(&good[..len]).is_err(), "{len}");
        }
        let mut long = good.clone();
        long.push(0);
        assert!(sx.set_state(&long).is_err());
        assert_eq!(sx, kept);
    }

    #[test]
    fn every_preset_and_every_state_in_fl_s_projects_loads() {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, Slicex's states skipped");
            return;
        };
        let generators = root.parent().unwrap().join("Plugins/Fruity/Generators");
        let mut saved = testing::every_preset(&root, NAME);
        let presets = saved.len();
        saved.extend(testing::projects(&root));
        let (mut states, mut slices, mut linked) = (0, 0, 0);
        let mut said = std::collections::BTreeMap::new();
        for saved in saved {
            if !saved.plugin.eq_ignore_ascii_case(NAME) {
                continue;
            }
            assert!(crate::generator_by_name(&saved.plugin).is_some());
            let mut sx = Slicex::default();
            sx.set_resource_dir(&generators);
            sx.set_sample_rate(48_000.0);
            if let Err(error) = sx.set_state(&saved.state) {
                panic!("{}: {error}", saved.file);
            }
            let named = sx.sample_paths();
            for path in &named {
                sx.set_sample(
                    path,
                    Some(SampleFile {
                        left: &[0.3; 400_000],
                        right: None,
                        rate: 44_100,
                    }),
                );
            }
            // Every linked file's regions are found under the install.
            assert!(!sx.unmarked, "{}", saved.file);
            linked += named.len();
            slices += sx.decks.iter().map(|d| d.regions.len()).sum::<usize>();
            // Every parameter takes back what it says.
            for index in 0..PARAMS {
                let value = sx.param(index);
                sx.set_param(index, value);
                assert_eq!(sx.param(index), value, "{} {index}", saved.file);
            }
            for key in 0..128 {
                sx.note_on(note(key as u32, key));
            }
            let (mut left, mut right) = ([0.0; 256], [0.0; 256]);
            sx.process(&mut left, &mut right);
            sx.all_notes_off();
            sx.process(&mut left, &mut right);
            assert!(
                left.iter().chain(&right).all(|v| v.is_finite()),
                "{}",
                saved.file
            );
            *said.entry(format!("{:?}", sx.fidelity())).or_insert(0) += 1;
            states += 1;
        }
        eprintln!(
            "Slicex: {presets} presets, {states} states, {slices} regions, {linked} linked files, {said:?}"
        );
        assert_eq!(states, 50);
    }
}
