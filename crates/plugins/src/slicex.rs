//! Slicex: two decks, each a sample cut into regions, every region a
//! slice a key plays, as `docs/plugins/slicex.md` has it.
//!
//! # The state
//!
//! A version (FL writes 0x60003 and reads it as 6), the plugin's eight
//! knobs, 47 bytes of settings and three words, then each deck: whether its
//! sample is a file (and the file's path) or carried in the state as a
//! whole WAV file, 0x4c bytes of the deck's settings, its four envelopes and
//! its slices of 17 words each. Eight articulators follow, each five words
//! and six envelope sets.
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
//! The key picks the slices. Each plays its region once from its start to
//! its end at the file's own pitch, moved by the note's fine pitch, the
//! slice's and the plugin's pitch, at the slice's volume times the note's
//! velocity and the plugin's volume. A note off does not stop it.
//!
//! # What is not FL's
//!
//! - The eight articulators (envelopes and LFOs on volume, pan, pitch and
//!   the rest) are read and not run. With them switched off, which is how
//!   nearly every state has them, FL plays a slice out as here.
//! - The resampler is FL's shared one, not read: a step of 1 copies frames,
//!   anything else is a 4-point Hermite curve.
//! - The declicker at a slice's ends, time stretching, the filter, the
//!   layer modes that pick one of several slices on a key, reversing and
//!   the start offset are left out.
//! - A pan other than the centre goes through FL's pan law without the part
//!   of each side that FL sends across.
//! - Every note plays on deck A.
//! - FL reads a file's regions itself. Here the plugin reads them from the
//!   file under FL's install, which the host names with
//!   [`Generator::set_resource_dir`].

use crate::menvelope::{Class, MEnvelope, Reader};
use crate::sampler::{self, Bank, Wave};
use crate::{Fidelity, Generator, NoteOn, SampleFile, StateError, knob_gain};
use std::path::{Component, Path, PathBuf};

/// The plugin's event 201 name.
pub const NAME: &str = "Slicex";

/// The most slices that sound at once.
pub const VOICES: usize = 256;

/// The version FL 2026 writes.
const WRITTEN: i32 = 0x60003;

/// A slice's 17 words as a new slice has them.
const DEFAULT_SLICE: [i32; 17] = [
    0, 0, 0, 8000, 0, 12800, 128, 128, 0, 12800, 0, 0, 12800, 0, 0, 0, 12800,
];

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

impl Slice {
    /// -1 on the left to 1 on the right (`FUN_008874a0`: word 2 / 6400).
    pub fn pan(&self) -> f32 {
        self.0[2] as f32 * 0.000_156_25
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

    /// Cents (word 10).
    pub fn pitch(&self) -> f32 {
        self.0[10] as f32
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

/// One of the eight articulators: five words and six sets of an envelope,
/// an LFO and four maps.
#[derive(Debug, Clone, PartialEq)]
pub struct Articulator {
    pub head: [i32; 5],
    pub items: Vec<Item>,
}

/// What an articulator does to one target (`TGroupArticulator`).
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
}

/// One slice that is sounding.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Voice {
    note: u32,
    wave: usize,
    /// Source frames: where the region starts, where it stops and where the
    /// voice is.
    pos: f64,
    stop: f64,
    step: f64,
    gains: (f32, f32),
}

/// Slicex.
#[derive(Debug, Clone, PartialEq)]
pub struct Slicex {
    /// The eight knobs: volume (word 2, 12800 for unity), pitch in cents
    /// (word 5), and the rest.
    pub knobs: [i32; 8],
    pub settings: Vec<u8>,
    words: [i32; 3],
    pub decks: [Deck; 2],
    pub articulators: Vec<Articulator>,
    bank: Bank,
    /// The slices each key plays on deck A: deck and region.
    keys: Vec<Vec<(usize, usize)>>,
    voices: Vec<Voice>,
    rate: f32,
    /// FL's `Plugins/Fruity/Generators`, for the regions of linked files.
    resources: Option<PathBuf>,
    /// A linked file whose regions could not be read.
    unmarked: bool,
}

impl Default for Slicex {
    fn default() -> Self {
        Slicex {
            knobs: [0, 0, 12800, 256, 128, 0, 256, 256],
            settings: vec![0; 0x2f],
            words: [0; 3],
            decks: [Deck::default(), Deck::default()],
            articulators: Vec::new(),
            bank: Bank::default(),
            keys: vec![Vec::new(); 128],
            voices: Vec::with_capacity(VOICES),
            rate: 44_100.0,
            resources: None,
            unmarked: false,
        }
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
        for _ in 0..8 {
            let head = words::<5>(&mut reader)?;
            let mut items = Vec::with_capacity(6);
            for _ in 0..6 {
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
        Ok(plugin)
    }

    /// The slices on each key of deck A (`FUN_008855b0`): every region with
    /// a key of its own, of both decks unless the decks are kept apart, and
    /// then deck A's others from the base key up where the key is free.
    fn map_keys(&mut self) {
        let mut keys = vec![Vec::new(); 128];
        let apart = self.settings.get(0x2a).is_some_and(|b| *b != 0);
        for (d, deck) in self.decks.iter().enumerate() {
            if apart && d != 0 {
                continue;
            }
            for (k, region) in deck.regions.iter().enumerate() {
                if (0..128).contains(&region.key) {
                    keys[region.key as usize].push((d, k));
                }
            }
        }
        let regions = &self.decks[0].regions;
        let base = base_key(regions.len());
        for (k, region) in regions.iter().enumerate() {
            let key = base + k as i32;
            if region.key == -1 && (0..128).contains(&key) && keys[key as usize].is_empty() {
                keys[key as usize].push((0, k));
            }
        }
        self.keys = keys;
    }

    /// The plugin's volume: knob 2 over 12800, through FL's knob curve.
    fn volume(&self) -> f32 {
        let x = self.knobs[2] as f32 / 12_800.0;
        if x == 1.0 {
            1.0
        } else {
            knob_gain(f64::from(x)) as f32
        }
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
}

impl Generator for Slicex {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let read = Self::read(bytes)?;
        *self = Slicex {
            rate: self.rate,
            resources: self.resources.clone(),
            ..read
        };
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        self.rate = hz;
    }

    fn set_resource_dir(&mut self, dir: &Path) {
        self.resources = Some(dir.to_path_buf());
    }

    fn note_on(&mut self, note: NoteOn) {
        if note.slide {
            return;
        }
        let key = i32::from(note.key);
        let Some(entries) = usize::try_from(key).ok().and_then(|k| self.keys.get(k)) else {
            return;
        };
        let layers = entries.len() as f32;
        let mode = self.settings.first().copied().unwrap_or(0);
        let master = self.volume();
        let global = self.knobs[5] as f32;
        for &(d, k) in entries {
            let deck = &self.decks[d];
            let Some(place) = deck.wave else {
                continue;
            };
            let Some(wave) = self.bank.wave(place) else {
                continue;
            };
            let frames = wave.len() as i64;
            let start = deck.regions[k].start.clamp(0, frames);
            let end = deck.end(k, frames).clamp(start, frames);
            let slice = deck.slice(k);
            let mut level = slice.volume();
            if mode == 1 && layers > 1.0 {
                level *= 1.0 / layers;
            }
            let volume = level * note.velocity * master;
            let pan = (slice.pan() + note.pan).clamp(-1.0, 1.0);
            let gains = if pan == 0.0 {
                (volume, volume)
            } else {
                sampler::circular(pan, volume * std::f32::consts::SQRT_2)
            };
            let cents = f64::from(slice.pitch() + note.fine_pitch + global);
            let speed = f64::from(wave.rate) / f64::from(self.rate);
            let step = if cents == 0.0 {
                speed
            } else {
                (cents * std::f64::consts::LN_2 / 1200.0).exp() * speed
            };
            if self.voices.len() == VOICES {
                self.voices.remove(0);
            }
            self.voices.push(Voice {
                note: note.voice,
                wave: place,
                pos: start as f64,
                stop: end as f64,
                step,
                gains,
            });
        }
    }

    /// A slice plays its region out.
    fn note_off(&mut self, _voice: u32) {}

    fn all_notes_off(&mut self) {}

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        let Slicex { bank, voices, .. } = self;
        for voice in voices.iter_mut() {
            let Some(wave) = bank.wave(voice.wave) else {
                voice.pos = f64::INFINITY;
                continue;
            };
            for n in 0..len {
                if voice.pos >= voice.stop {
                    break;
                }
                let (l, r) = wave.frame(voice.pos);
                left[n] += l * voice.gains.0;
                right[n] += r * voice.gains.1;
                voice.pos += voice.step;
            }
        }
        voices.retain(|voice| voice.pos < voice.stop);
    }

    fn param(&self, _index: usize) -> f32 {
        0.0
    }

    fn set_param(&mut self, _index: usize, _value: f32) {}

    fn reset(&mut self) {
        self.voices.clear();
    }

    /// The longest region at the slowest the knobs can play it.
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
        (longest * f64::from(self.rate)) as usize + 1
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
        let active = self
            .articulators
            .iter()
            .flat_map(|a| &a.items)
            .any(Item::active);
        if active {
            return Fidelity::Approximate("Slicex: its articulators are left out");
        }
        if self.settings.first().is_some_and(|m| *m > 1) && self.keys.iter().any(|k| k.len() > 1) {
            return Fidelity::Approximate("Slicex: a layer mode that picks one slice is left out");
        }
        Fidelity::Approximate("Slicex: FL's resampler and declicker are not read")
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

    fn flat() -> Vec<u8> {
        let points = [0.0, 8.0].map(|x| Point {
            x,
            y: 0.5,
            tension: 0.0,
            mode: 0,
        });
        stream(6, 3, &points, [0, -1, -1, -1, -1], &[])
    }

    /// A state carrying `file` on deck A with these slice settings, and an
    /// empty deck B.
    fn state(file: &[u8], slices: &[[i32; 17]], volume: i32, pitch: i32) -> Vec<u8> {
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
        for _ in 0..8 {
            for word in [128, 128, 128, 0, 0] {
                bytes.extend(i32::to_le_bytes(word));
            }
            for _ in 0..6 {
                bytes.extend(3i32.to_le_bytes());
                for knob in [0.0f32, 1.0, 1.0] {
                    bytes.extend(knob.to_le_bytes());
                }
                let point = [Point {
                    x: 0.0,
                    y: 0.5,
                    tension: 0.0,
                    mode: 0,
                }];
                bytes.extend(stream(
                    1,
                    3,
                    &point,
                    [0, 0, -1, -1, -1],
                    &[128, 128, 0, 128],
                ));
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

    #[test]
    fn each_key_from_60_plays_its_region_once_at_the_file_s_pitch() {
        let frames = ramp(300);
        let file = wav(&frames, &[0, 100, 200], &[], &[]);
        let mut sx = Slicex::default();
        sx.set_state(&state(&file, &[DEFAULT_SLICE; 3], 12_800, 0))
            .unwrap();
        assert!(sx.sample_paths().is_empty());
        assert_eq!(sx.decks[0].regions.len(), 3);
        assert_eq!(sx.decks[0].regions[0].name, "Kick");
        sx.note_on(note(1, 61));
        // A note off does not stop it.
        sx.note_off(1);
        // FL hands its player the region's length as its last frame less
        // its first, so the last frame does not sound.
        let (left, right) = play(&mut sx, 150);
        for n in 0..99 {
            let want = f32::from(frames[100 + n]) / 32_768.0;
            assert!((left[n] - want).abs() < 1e-6, "{n}");
            assert_eq!(left[n], right[n]);
        }
        assert!(left[99..].iter().all(|v| *v == 0.0));
        // The last region ends with the file, and key 63 has none.
        sx.note_on(note(2, 62));
        let (left, _) = play(&mut sx, 100);
        assert!((left[98] - f32::from(frames[298]) / 32_768.0).abs() < 1e-6 && left[99] == 0.0);
        sx.note_on(note(3, 63));
        assert!(play(&mut sx, 10).0.iter().all(|v| *v == 0.0));
        assert!(sx.tail() >= 100);
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
        assert!(left[48] != 0.0 && left[49..].iter().all(|v| *v == 0.0));
        sx.note_on(note(2, 36));
        assert!((play(&mut sx, 4).0[0] - f32::from(frames[100]) / 32_768.0).abs() < 1e-6);
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
        let want = 0.5 * 4.0 * knob_gain(2.0) as f32 * 0.5;
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
        assert!(
            (left[3] - 6.0 / 32_768.0).abs() < 1e-7,
            "{}",
            left[3] * 32_768.0
        );
        // Hard left takes the right side away.
        let mut sx = Slicex::default();
        sx.set_state(&state(&file, &[DEFAULT_SLICE], 12_800, 0))
            .unwrap();
        let mut left_note = note(1, 60);
        left_note.pan = -1.0;
        sx.note_on(left_note);
        let (left, right) = play(&mut sx, 8);
        assert!(left[5] > 0.0 && right[5].abs() < 1e-9);
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
        for saved in saved {
            if !saved.plugin.eq_ignore_ascii_case(NAME) {
                continue;
            }
            assert!(crate::generator_by_name(&saved.plugin).is_some());
            let mut sx = Slicex::default();
            sx.set_resource_dir(&generators);
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
            for key in 0..128 {
                sx.note_on(note(key as u32, key));
            }
            let (mut left, mut right) = ([0.0; 256], [0.0; 256]);
            sx.process(&mut left, &mut right);
            assert!(
                left.iter().chain(&right).all(|v| v.is_finite()),
                "{}",
                saved.file
            );
            states += 1;
        }
        eprintln!(
            "Slicex: {presets} presets, {states} states, {slices} regions, {linked} linked files"
        );
        assert_eq!(states, 50);
    }
}
