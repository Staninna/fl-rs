//! Fruity Slicer: one sample file cut into slices, a slice a key, as
//! `docs/plugins/fruity-slicer.md` has it.
//!
//! # The state
//!
//! A version (FL 2026 writes 16), the loop's length in beats and its
//! tempo, the pitch and time knobs of the stretcher and its method, the
//! attack and decay of every slice in ms, the sample's path, the slices
//! (a name, a start frame, a key, a float and whether it plays reversed),
//! the switches, the rate the starts were counted at and, from version 16
//! on, the plugin's own articulation: FL's five channel envelopes.
//!
//! # The sample
//!
//! FL loads the file and works on a copy of it once, as the state is
//! loaded: it reverses the reversed slices, declicks the end of every slice
//! when "declick" is on, stretches the whole when the time or pitch knob is
//! off its centre, and fades every slice in and out over the attack and
//! the decay. Here that happens when the host hands the file over.
//!
//! # A note
//!
//! The key picks the slice: slice `i` sits on its own key when it has one
//! and on the root key plus `i` when not, with the root 60 an octave lower
//! for every 12 slices that would run past key 124. The slice plays from
//! its start for its length, or to the end of the sample with "play to
//! end", at the file's own pitch moved by the note's fine pitch, through
//! FL's pan law with the note's velocity and pan. A note off starts the
//! volume envelope's release, and when that is down (or at once, with the
//! envelope off) a 10 ms declick ends the voice.
//!
//! # What is not FL's
//!
//! - FL stretches with zplane's Elastique, which is not rebuilt. A
//!   stretched slice plays unstretched here, cut or padded with silence to
//!   its stretched length.
//! - The volume envelope's release runs frame by frame. FL steps it once a
//!   tick and ramps the gain between the steps.
//! - The other four envelopes, the LFOs and the attack, hold and decay of
//!   the volume envelope are read and not run, and fidelity says so when a
//!   state has them other than as FL leaves them.
//! - FL's resampler is not read: a 4-point Hermite curve stands in between
//!   frames.

use crate::menvelope::Reader;
use crate::sampler::{self, Bank, Wave};
use crate::{Fidelity, Generator, NoteOn, SampleFile, StateError};
use std::f64::consts::PI;

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Slicer";

/// The version FL 2026 writes.
const WRITTEN: i32 = 16;

/// The most slices that sound at once.
const VOICES: usize = 256;

/// How many keys a slice can sit on: 0 to 124.
const KEYS: usize = 125;

/// The size of the articulation block, five envelopes of 0x44 bytes and
/// four words.
const ARTICULATION: usize = 0x16c;

/// One slice as the state has it.
#[derive(Debug, Clone, PartialEq)]
pub struct Slice {
    pub name: String,
    /// The first frame, counted at the state's rate.
    pub start: i32,
    /// The key the slice sits on, -1 for its place after the root.
    pub key: i32,
    /// FL writes -1.0 and does not read it back for playing.
    pub unknown: f32,
    pub reversed: bool,
}

/// One slice of the sample as FL leaves it after loading: where it starts
/// and how many frames it has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Region {
    start: usize,
    len: usize,
}

/// A slice that sounds.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Voice {
    note: u32,
    pos: f64,
    stop: f64,
    step: f64,
    gains: (f32, f32),
    /// Where the release is: 1 at the note off, 0 when it is down. `None`
    /// while the note is held.
    release: Option<f64>,
    /// How many frames of the declick have gone, once it has started.
    declick: Option<usize>,
}

/// The release of the volume envelope, as FL reads it out of the
/// articulation block.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Release {
    /// The knob, 0 to 65536.
    time: i32,
    /// The tension knob, -128 to 128.
    tension: i32,
    /// Bit 0 of the envelope's flags: the time is in beats.
    synced: bool,
}

#[derive(Debug, Clone)]
pub struct FruitySlicer {
    pub version: i32,
    /// The loop's length in beats and its tempo.
    pub beats: f32,
    pub tempo: f32,
    /// The stretcher's pitch knob in cents and its time knob, 10000 to
    /// the doubling.
    pub pitch: i32,
    pub time: i32,
    pub method: i32,
    /// Each slice's fade in and fade out, ms.
    pub attack: i32,
    pub decay: i32,
    pub path: String,
    pub slices: Vec<Slice>,
    pub animate: bool,
    pub root: i32,
    pub play_to_end: bool,
    /// The rate the starts were counted at, 0 when the state does not say.
    pub counted_at: i32,
    pub auto_dump: bool,
    pub declick: bool,
    pub auto_fit: bool,
    pub view: i32,
    pub articulation: Option<Vec<u8>>,
    bank: Bank,
    /// The sample as FL plays it: reversed, declicked, stretched and faded.
    wave: Option<Wave>,
    regions: Vec<Region>,
    keys: [i32; KEYS],
    voices: Vec<Voice>,
    rate: f32,
    bpm: f64,
    ppq: u16,
}

impl Default for FruitySlicer {
    fn default() -> Self {
        FruitySlicer {
            version: WRITTEN,
            beats: 4.0,
            tempo: 140.0,
            pitch: 0,
            time: 0,
            method: 2,
            attack: 0,
            decay: 0,
            path: String::new(),
            slices: Vec::new(),
            animate: false,
            root: 60,
            play_to_end: false,
            counted_at: 0,
            auto_dump: true,
            declick: true,
            auto_fit: true,
            view: 0,
            articulation: None,
            bank: Bank::default(),
            wave: None,
            regions: Vec::new(),
            keys: [-1; KEYS],
            voices: Vec::new(),
            rate: 44_100.0,
            bpm: 120.0,
            ppq: 96,
        }
    }
}

/// A string as the state holds one: a byte of length (0xff: a u32 of
/// length follows) and that many bytes of UTF-8. Control characters at its
/// end go (`FUN_0074b2d0`).
fn string(reader: &mut Reader<'_>) -> Result<String, StateError> {
    let len = match reader.u8()? {
        0xff => reader.u32()? as usize,
        len => usize::from(len),
    };
    let bytes = reader.bytes(len)?;
    let text = String::from_utf8_lossy(bytes);
    Ok(text
        .trim_end_matches(|c: char| (c as u32) < 0x20)
        .to_owned())
}

/// Delphi's `Round`: halves go to the even neighbour.
fn round(x: f64) -> i64 {
    x.round_ties_even() as i64
}

/// Half a cosine from 0 at `x = 0` to 1 at `x = 1`: FL's fade of kind 4.
fn cosine(x: f64) -> f64 {
    ((1.0 - x) * PI).cos().mul_add(0.5, 0.5)
}

impl FruitySlicer {
    fn read(bytes: &[u8]) -> Result<Self, StateError> {
        let mut reader = Reader::new(bytes);
        let version = reader.i32()?;
        if version > WRITTEN {
            return Err(StateError::Version { found: version });
        }
        let mut plugin = FruitySlicer {
            version,
            pitch: 0,
            time: 0,
            method: 0,
            ..FruitySlicer::default()
        };
        let beats = reader.bytes(4)?;
        let tempo = reader.bytes(4)?;
        let word = |b: &[u8]| i32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        let float = |b: &[u8]| f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        plugin.beats = if version < 12 {
            word(beats) as f32
        } else {
            float(beats)
        };
        if version < 7 {
            plugin.beats *= 4.0;
        }
        plugin.tempo = if version < 8 {
            word(tempo) as f32
        } else {
            float(tempo)
        };
        if version >= 9 {
            plugin.pitch = reader.i32()?;
            plugin.time = reader.i32()?;
            plugin.method = reader.i32()?;
        }
        plugin.attack = reader.i32()?;
        plugin.decay = reader.i32()?;
        plugin.path = string(&mut reader)?;
        let count = reader.i32()?;
        let count = usize::try_from(count).map_err(|_| StateError::Length {
            expected: reader.at,
            found: bytes.len(),
        })?;
        for _ in 0..count.min(bytes.len()) {
            let name = string(&mut reader)?;
            let start = reader.i32()?;
            let mut slice = Slice {
                name,
                start,
                key: -1,
                unknown: -1.0,
                reversed: false,
            };
            if version >= 13 {
                slice.key = reader.i32()?;
                slice.unknown = reader.f32()?;
                slice.reversed = reader.u8()? != 0;
            } else if version > 5 {
                slice.reversed = reader.u8()? != 0;
            }
            plugin.slices.push(slice);
        }
        if plugin.slices.len() != count {
            return Err(StateError::Length {
                expected: bytes.len() + 1,
                found: bytes.len(),
            });
        }
        plugin.animate = reader.u8()? != 0;
        plugin.root = if version < 2 { 60 } else { reader.i32()? };
        plugin.play_to_end = version >= 3 && reader.u8()? != 0;
        plugin.counted_at = if version < 4 { 0 } else { reader.i32()? };
        let mut fit = false;
        if version >= 5 {
            plugin.auto_dump = reader.u8()? != 0;
            if version < 9 {
                fit = reader.u8()? != 0;
            }
        }
        let method = if (6..=9).contains(&version) {
            i32::from(reader.u8()?)
        } else {
            0
        };
        if fit && version < 9 {
            plugin.time = 10_000;
            plugin.method = method;
        }
        plugin.declick = version >= 10 && reader.u8()? != 0;
        plugin.auto_fit = if version < 11 {
            true
        } else {
            reader.u8()? != 0
        };
        plugin.view = if version < 14 { 0 } else { reader.i32()? };
        if version < 15 && plugin.method > 1 {
            plugin.method += 2;
        }
        let rest = bytes.len() - reader.at;
        if rest >= ARTICULATION {
            plugin.articulation = Some(reader.bytes(ARTICULATION)?.to_vec());
        }
        reader.end()?;
        plugin.bank.place(&plugin.path);
        Ok(plugin)
    }

    /// The word `index` of envelope `envelope` in the articulation block.
    fn envelope(&self, envelope: usize, index: usize) -> Option<i32> {
        let at = envelope * 0x44 + index * 4;
        let bytes = self.articulation.as_deref()?.get(at..at + 4)?;
        Some(i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// The volume envelope's release, when the envelope is on
    /// (`FUN_007579d0`: with it off a note off goes straight to the
    /// declick).
    fn release(&self) -> Option<Release> {
        if self.envelope(1, 1)? == 0 {
            return None;
        }
        Some(Release {
            time: self.envelope(1, 7)?,
            tension: self.envelope(1, 16)?,
            synced: self.envelope(1, 0)? & 1 != 0,
        })
    }

    /// Whether the articulation does more than FL's default: an envelope
    /// other than the volume one on, or the volume one with an attack, a
    /// hold, a decay or a sustain below full, or an LFO.
    fn articulated(&self) -> bool {
        if self.articulation.is_none() {
            return false;
        }
        let on = |e: usize| self.envelope(e, 1).unwrap_or(0) != 0;
        if (0..5).filter(|e| *e != 1).any(on) {
            return true;
        }
        if !on(1) {
            return false;
        }
        let word = |i: usize| self.envelope(1, i).unwrap_or(0);
        word(3) > 100 || word(4) > 100 || word(5) > 100 || word(6) != 128 || word(11) != 0
    }

    /// The release in ticks (`FUN_00755cf0` with the time unit of
    /// `FUN_00755d80`), and the length of a tick in frames.
    fn release_ticks(&self, release: Release) -> (f64, f64) {
        let ppq = f64::from(self.ppq.max(1));
        let mut unit = f64::from(self.ppq / 4) / 48.0 * 24_576.0 / 1000.0;
        if !release.synced {
            unit = unit * self.bpm / 120.0;
        }
        let ticks = if release.time < 0x65 {
            0
        } else {
            let curve = (f64::from(release.time) * (1.0 / 65_536.0)) as f32;
            let curve = (f64::from(curve) * 1001f64.ln()).exp();
            round(f64::from((curve - 1.0) as f32) * unit)
        };
        let tick = f64::from(self.rate) * 60.0 / (self.bpm.max(1.0) * ppq);
        (ticks as f64, tick)
    }

    /// The tension of the release (`FUN_00755c30`, `FUN_005f83c0`) and the
    /// level it gives at `x`, 1 at the note off and 0 when it is down
    /// (`FUN_00755830`, state 6).
    fn level(tension: i32, x: f64) -> f64 {
        if tension == 0 {
            return x;
        }
        let size = (f64::from((tension.abs() as f32) * (1.0 / 128.0)) * 6347f64.ln()).exp();
        let t = ((size - 1.0) as f32 * 0.1) as f64;
        let (inverse, bend) = (1.0 / t, (t + 1.0).ln());
        if tension > 0 {
            1.0 - (((1.0 - x) * bend).exp() - 1.0) * inverse
        } else {
            ((x * bend).exp() - 1.0) * inverse
        }
    }

    /// The copy of the sample FL plays and its slices, made as the state
    /// loads (`FUN_007686d0` and the functions it calls).
    fn prepare(&mut self) {
        self.wave = None;
        self.regions.clear();
        self.keys = [-1; KEYS];
        let Some(file) = self.bank.wave(0) else {
            return;
        };
        let rate = file.rate;
        let mut sides = vec![file.left.clone()];
        if let Some(right) = &file.right {
            sides.push(right.clone());
        }
        let total = file.len();
        // The starts were counted at the rate the state names.
        let mut starts: Vec<i64> = self
            .slices
            .iter()
            .map(|s| {
                if self.counted_at != 0 {
                    round(f64::from(s.start) * f64::from(rate) / f64::from(self.counted_at))
                } else {
                    i64::from(s.start)
                }
            })
            .collect();
        // A slice that starts past the end goes, with every one after it
        // (`FUN_00769900`).
        if let Some(past) = starts.iter().position(|s| *s >= total as i64) {
            starts.truncate(past);
        }
        if total > 0 && starts.is_empty() {
            starts.push(0);
        }
        let lengths = |starts: &[i64], total: i64| -> Vec<i64> {
            (0..starts.len())
                .map(|i| match starts.get(i + 1) {
                    Some(next) => (next - starts[i]).max(1),
                    None => total - starts[i],
                })
                .collect()
        };
        let lens = lengths(&starts, total as i64);
        let count = starts.len();
        let mut root = 60;
        while root - 1 + count as i32 >= KEYS as i32 && root >= 12 {
            root -= 12;
        }
        for i in 0..count {
            let key = self.slices.get(i).map_or(-1, |s| s.key);
            let key = if key < 0 { root + i as i32 } else { key };
            self.keys[key.clamp(0, KEYS as i32 - 1) as usize] = i as i32;
        }
        let span = |s: i64, l: i64| -> Option<(usize, usize)> {
            let s = usize::try_from(s).ok()?;
            let l = usize::try_from(l).ok()?;
            (l > 0).then_some((s, l))
        };
        // Reversed slices (`FUN_00769cd0`).
        for (i, slice) in self.slices.iter().enumerate().take(count) {
            if let Some((s, l)) = span(starts[i], lens[i]).filter(|_| slice.reversed) {
                for side in &mut sides {
                    let end = (s + l).min(side.len());
                    if s < end {
                        side[s..end].reverse();
                    }
                }
            }
        }
        if self.declick {
            for i in 0..count {
                if let Some((s, l)) = span(starts[i], lens[i]) {
                    declick(&mut sides, rate, s, l);
                }
            }
        }
        let (mut starts, mut lens) = (starts, lens);
        let mut total = total;
        if total > 0 && (self.time != 0 || self.pitch != 0) {
            // `FUN_0076aa20`: the stretched sample is the old one times
            // 2^(time / 10000) long, every slice in proportion.
            let ratio =
                f64::from((f64::from(self.time as f32 / 10_000.0) * 2f64.ln()).exp() as f32);
            let stretched = round(total as f64 * ratio).max(0) as usize;
            let new_starts: Vec<i64> = starts.iter().map(|s| round(*s as f64 * ratio)).collect();
            let new_lens = lengths(&new_starts, stretched as i64);
            let mut out = vec![vec![0.0; stretched]; sides.len()];
            for i in 0..count {
                let (Some((from, had)), Some((to, has))) =
                    (span(starts[i], lens[i]), span(new_starts[i], new_lens[i]))
                else {
                    continue;
                };
                let n = had.min(has);
                for (side, old) in out.iter_mut().zip(&sides) {
                    let n = n
                        .min(old.len().saturating_sub(from))
                        .min(side.len().saturating_sub(to));
                    side[to..to + n].copy_from_slice(&old[from..from + n]);
                }
            }
            sides = out;
            starts = new_starts;
            lens = new_lens;
            total = stretched;
            // Elastique's slices are declicked again (`FUN_0076a790`).
            if (self.method > 1 || self.pitch != 0) && self.declick {
                for i in 0..count {
                    if let Some((s, l)) = span(starts[i], lens[i]) {
                        declick(&mut sides, rate, s, l);
                    }
                }
            }
        }
        // The fades of every slice (`FUN_0076a830`).
        if self.attack != 0 || self.decay != 0 {
            let per_ms = (f64::from(rate) / 1000.0) as f32;
            let attack = round(f64::from(self.attack as f32 * per_ms)).max(0) as usize;
            let decay = round(f64::from(self.decay as f32 * per_ms)).max(0) as usize;
            for i in 0..count {
                let Some((s, l)) = span(starts[i], lens[i]) else {
                    continue;
                };
                fade(&mut sides, rate, s, attack.min(l), 2, 0.0, 1.0, 0.0);
                let n = decay.min(l);
                fade(&mut sides, rate, s + l - n, n, 3, 1.0, 0.0, 10.0);
            }
        }
        self.regions = starts
            .iter()
            .zip(&lens)
            .map(|(s, l)| Region {
                start: (*s).clamp(0, total as i64) as usize,
                len: (*l).max(0) as usize,
            })
            .collect();
        let right = (sides.len() > 1).then(|| sides.pop().unwrap_or_default());
        let left = sides.pop().unwrap_or_default();
        self.wave = Some(Wave { left, right, rate });
    }
}

/// FL's declicker at the end of a slice (`FUN_0060d020`, `FUN_0060cdd0`):
/// the zeros at its end are passed over, and over the last 28 ms before
/// them a one-pole tracker whose rate sweeps from 2 Hz to 22050 Hz takes
/// the signal out while the last ms fades along half a cosine.
fn declick(sides: &mut [Vec<f32>], rate: u32, start: usize, len: usize) {
    let rate = f64::from(rate);
    let span = round(rate * f64::from(0.04f32) * f64::from(0.7f32)).max(0) as usize;
    for side in sides.iter_mut() {
        let end = (start + len).min(side.len());
        if end <= start {
            continue;
        }
        let mut count = end - start;
        while count > 0 && side[start + count - 1] == 0.0 {
            count -= 1;
        }
        let n = span.min(count);
        if n == 0 {
            continue;
        }
        let first = start + count - n;
        let sweep = 22_050f64.ln() / (2f64.ln() * n as f64) * 2f64.ln();
        let floor = 2.0 / rate;
        let fade = (round(rate * 0.001) as usize).min(n);
        let flat = n - fade;
        let step = 1.0 / (fade + 1) as f64;
        let (mut dc, mut gain) = (0.0f64, 1.0f64);
        for i in 1..=n {
            let x = &mut side[first + i - 1];
            let k = (floor * (i as f64 * sweep).exp()).min(1.0);
            dc += k * (f64::from(*x) - dc);
            if i > flat {
                gain = ((i - flat) as f64 * step * PI).cos().mul_add(0.5, 0.5);
            }
            *x = ((f64::from(*x) - dc) * gain) as f32;
        }
    }
}

/// A fade of `len` frames from `start` (`FUN_00607ed0`), from gain `from`
/// to `to`. A fade of 50 ms or less is half a cosine whatever its kind.
#[allow(clippy::too_many_arguments)]
fn fade(
    sides: &mut [Vec<f32>],
    rate: u32,
    start: usize,
    len: usize,
    kind: u8,
    from: f32,
    to: f32,
    bend: f64,
) {
    if len == 0 {
        return;
    }
    let ms = len as f64 * 1000.0 / f64::from(rate);
    let kind = if ms <= 50.0 { 4 } else { kind };
    let step = 1.0 / (len + 1) as f64;
    let falling = to < from;
    for j in 0..len {
        let x = (j + 1) as f64 * step;
        let g = match kind {
            3 => {
                let x = if falling { 1.0 - x } else { x };
                let base = f64::from(bend as f32 + 1.0).ln();
                let g = f64::from(((x as f32 as f64 * base).exp() - 1.0) as f32) * (1.0 / bend);
                if falling { 1.0 - g } else { g }
            }
            4 => cosine(x),
            _ => x,
        };
        let gain = g * f64::from(to - from) + f64::from(from);
        for side in sides.iter_mut() {
            if let Some(v) = side.get_mut(start + j) {
                *v = (f64::from(*v) * gain) as f32;
            }
        }
    }
}

impl Generator for FruitySlicer {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let read = Self::read(bytes)?;
        *self = FruitySlicer {
            rate: self.rate,
            bpm: self.bpm,
            ppq: self.ppq,
            voices: Vec::with_capacity(VOICES),
            ..read
        };
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        self.rate = hz;
    }

    fn set_tempo(&mut self, bpm: f64) {
        self.bpm = bpm;
    }

    fn set_ppq(&mut self, ppq: u16) {
        self.ppq = ppq;
    }

    fn note_on(&mut self, note: NoteOn) {
        if note.slide {
            return;
        }
        let Some(wave) = &self.wave else {
            return;
        };
        let Some(slice) = usize::try_from(note.key)
            .ok()
            .and_then(|k| self.keys.get(k))
            .and_then(|i| usize::try_from(*i).ok())
        else {
            return;
        };
        let Some(region) = self.regions.get(slice) else {
            return;
        };
        let len = if self.play_to_end {
            wave.len().saturating_sub(region.start)
        } else {
            region.len
        };
        // `FUN_00768230`: the voice plays `len - 1` frames, and at least
        // one, past the first.
        let len = len.saturating_sub(1).max(1) + 1;
        let step = sampler::step(f64::from(note.fine_pitch), wave.rate, self.rate);
        if self.voices.len() == VOICES {
            self.voices.remove(0);
        }
        self.voices.push(Voice {
            note: note.voice,
            pos: region.start as f64,
            stop: (region.start + len) as f64,
            step,
            gains: sampler::circular(note.pan, note.velocity),
            release: None,
            declick: None,
        });
    }

    fn note_off(&mut self, voice: u32) {
        let enveloped = self.release().is_some();
        for v in self.voices.iter_mut().filter(|v| v.note == voice) {
            if enveloped {
                v.release.get_or_insert(1.0);
            } else {
                v.declick.get_or_insert(0);
            }
        }
    }

    fn all_notes_off(&mut self) {
        let enveloped = self.release().is_some();
        for v in &mut self.voices {
            if enveloped {
                v.release.get_or_insert(1.0);
            } else {
                v.declick.get_or_insert(0);
            }
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        let Some(wave) = &self.wave else {
            return;
        };
        let release = self.release();
        let (ticks, tick) = release.map_or((0.0, 1.0), |r| self.release_ticks(r));
        let fall = 1.0 / ((ticks + 1.0) * tick.max(1.0));
        let tension = release.map_or(0, |r| r.tension);
        let dk = (round(10.0 * 0.001 * f64::from(self.rate)).max(1)) as usize;
        let dk_step = PI / (dk + 1) as f64;
        for voice in self.voices.iter_mut() {
            for n in 0..len {
                if voice.pos >= voice.stop {
                    break;
                }
                let mut gain = 1.0f64;
                if let Some(x) = voice.release.as_mut() {
                    let level = Self::level(tension, *x);
                    if level <= 5.960_464_5e-8 {
                        voice.declick.get_or_insert(0);
                        gain = 0.0;
                    } else {
                        gain = level;
                    }
                    *x -= fall;
                }
                if let Some(done) = voice.declick.as_mut() {
                    *done += 1;
                    if *done > dk {
                        voice.pos = f64::INFINITY;
                        break;
                    }
                    gain *= (*done as f64 * dk_step).cos().mul_add(0.5, 0.5);
                }
                let (l, r) = wave.frame(voice.pos);
                left[n] += l * voice.gains.0 * gain as f32;
                right[n] += r * voice.gains.1 * gain as f32;
                voice.pos += voice.step;
            }
        }
        self.voices.retain(|voice| voice.pos < voice.stop);
    }

    fn param(&self, _index: usize) -> f32 {
        0.0
    }

    fn set_param(&mut self, _index: usize, _value: f32) {}

    fn reset(&mut self) {
        self.voices.clear();
    }

    /// The whole sample an octave down, and the release.
    fn tail(&self) -> usize {
        let Some(wave) = &self.wave else {
            return 1;
        };
        let frames = wave.len() as f64 / f64::from(wave.rate) * f64::from(self.rate) * 2.0;
        let release = self.release().map_or(0.0, |r| {
            let (ticks, tick) = self.release_ticks(r);
            (ticks + 1.0) * tick
        });
        (frames + release + f64::from(self.rate) * 0.01) as usize + 1
    }

    fn fidelity(&self) -> Fidelity {
        if self.bank.lacking() {
            return Fidelity::Approximate(
                "Fruity Slicer: its sample file is missing, and its slices are silent",
            );
        }
        if self.time != 0 || self.pitch != 0 {
            return Fidelity::Approximate(
                "Fruity Slicer: Elastique's stretch is left out, and slices play unstretched",
            );
        }
        if self.articulated() {
            return Fidelity::Approximate(
                "Fruity Slicer: its envelopes and LFOs are left out but for the volume release",
            );
        }
        if self.bank.resampled(self.rate) {
            return Fidelity::Approximate(
                "Fruity Slicer: a Hermite curve stands in for FL's resampler",
            );
        }
        if self.release().is_some() {
            return Fidelity::Approximate(
                "Fruity Slicer: the volume release runs frame by frame, not in FL's ticks",
            );
        }
        Fidelity::Exact
    }

    fn sample_paths(&self) -> Vec<String> {
        self.bank.paths()
    }

    fn set_sample(&mut self, path: &str, file: Option<SampleFile<'_>>) {
        if self.bank.find(path).is_none() {
            return;
        }
        self.bank.set(path, file);
        self.voices.clear();
        self.prepare();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    const RATE: u32 = 44_100;

    /// Settings of a test state.
    #[derive(Clone)]
    struct Setup {
        starts: Vec<(i32, bool)>,
        attack: i32,
        decay: i32,
        declick: bool,
        to_end: bool,
        time: i32,
        articulation: Option<Vec<u8>>,
    }

    impl Default for Setup {
        fn default() -> Self {
            Setup {
                starts: vec![(0, false), (100, false), (250, false)],
                attack: 0,
                decay: 0,
                declick: false,
                to_end: false,
                time: 0,
                articulation: None,
            }
        }
    }

    fn short(bytes: &mut Vec<u8>, text: &str) {
        bytes.push(text.len() as u8);
        bytes.extend(text.as_bytes());
    }

    /// A version 16 state as FL writes one.
    fn state(setup: &Setup) -> Vec<u8> {
        let mut bytes = 16i32.to_le_bytes().to_vec();
        bytes.extend(8f32.to_le_bytes());
        bytes.extend(120f32.to_le_bytes());
        for word in [0, setup.time, 4, setup.attack, setup.decay] {
            bytes.extend(word.to_le_bytes());
        }
        short(&mut bytes, "loop.wav");
        bytes.extend((setup.starts.len() as i32).to_le_bytes());
        for (start, reversed) in &setup.starts {
            short(&mut bytes, "Kick");
            bytes.extend(start.to_le_bytes());
            bytes.extend((-1i32).to_le_bytes());
            bytes.extend((-1f32).to_le_bytes());
            bytes.push(u8::from(*reversed));
        }
        bytes.push(1);
        bytes.extend(60i32.to_le_bytes());
        bytes.push(u8::from(setup.to_end));
        bytes.extend((RATE as i32).to_le_bytes());
        bytes.push(1);
        bytes.push(u8::from(setup.declick));
        bytes.push(1);
        bytes.extend(0i32.to_le_bytes());
        if let Some(block) = &setup.articulation {
            bytes.extend(block);
        }
        bytes
    }

    /// The articulation block FL's projects hold: the volume envelope on
    /// with a release of 6500 and a tension of -101, the rest off.
    fn articulation() -> Vec<u8> {
        let off = [
            0, 0, 100, 20000, 20000, 30000, 50, 20000, 0, 100, 20000, 0, 32950, 0, 0, 0, 0,
        ];
        let volume = [
            4, -1, 100, 100, 100, 100, 128, 6500, 0, 100, 20000, 0, 32950, 0, 0, 0, -101,
        ];
        let mut words: Vec<i32> = Vec::new();
        for e in 0..5 {
            words.extend(if e == 1 { volume } else { off });
        }
        words.extend([0, 0, 256, 0, 0, 0]);
        testing::state(&words)
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
            porta: false,
        }
    }

    fn slicer(setup: &Setup, file: &[f32]) -> FruitySlicer {
        let mut slicer = FruitySlicer::default();
        slicer.set_sample_rate(RATE as f32);
        slicer.set_state(&state(setup)).unwrap();
        assert_eq!(slicer.sample_paths(), ["loop.wav"]);
        slicer.set_sample(
            "loop.wav",
            Some(SampleFile {
                left: file,
                right: None,
                rate: RATE,
            }),
        );
        slicer
    }

    fn play(slicer: &mut FruitySlicer, len: usize) -> Vec<f32> {
        let (mut left, mut right) = (vec![0.0; len], vec![0.0; len]);
        slicer.process(&mut left, &mut right);
        assert_eq!(left, right);
        left
    }

    fn ramp(len: usize) -> Vec<f32> {
        (0..len).map(|i| i as f32 / 1000.0).collect()
    }

    const CENTRE: f32 = std::f32::consts::FRAC_1_SQRT_2;

    #[test]
    fn each_key_from_60_plays_its_slice_once_at_the_file_s_pitch() {
        let file = ramp(400);
        let mut slicer = slicer(&Setup::default(), &file);
        for (key, from, to) in [(60, 0, 100), (61, 100, 250), (62, 250, 400)] {
            slicer.note_on(note(1, key));
            let out = play(&mut slicer, 500);
            let expected: Vec<f32> = (0..500)
                .map(|n| {
                    if n < to - from {
                        file[from + n] * CENTRE
                    } else {
                        0.0
                    }
                })
                .collect();
            assert_eq!(out, expected, "key {key}");
            assert!(slicer.voices.is_empty());
        }
        // Keys without a slice are silent.
        for key in [59, 63, 127] {
            slicer.note_on(note(1, key));
            assert!(slicer.voices.is_empty());
        }
        assert_eq!(slicer.fidelity(), Fidelity::Exact);
    }

    #[test]
    fn play_to_end_runs_on_past_the_slice() {
        let file = ramp(400);
        let setup = Setup {
            to_end: true,
            ..Setup::default()
        };
        let mut slicer = slicer(&setup, &file);
        slicer.note_on(note(1, 61));
        let out = play(&mut slicer, 500);
        assert!(out[299] > 0.0 && out[300] == 0.0);
    }

    #[test]
    fn many_slices_start_an_octave_down_and_keys_of_their_own_win() {
        let starts: Vec<(i32, bool)> = (0..70).map(|i| (i * 10, false)).collect();
        let setup = Setup {
            starts,
            ..Setup::default()
        };
        let slicer = slicer(&setup, &ramp(700));
        // 60 - 1 + 70 runs past 124, so the slices start at 48.
        assert_eq!(slicer.keys[48], 0);
        assert_eq!(slicer.keys[117], 69);
        assert_eq!(slicer.keys[47], -1);
        assert_eq!(slicer.keys[118], -1);
    }

    #[test]
    fn a_note_off_ends_the_slice_with_a_10_ms_cosine() {
        let file = vec![1.0; 10_000];
        let mut slicer = slicer(&Setup::default(), &file);
        slicer.note_on(note(7, 62));
        let held = play(&mut slicer, 100);
        assert!(held.iter().all(|v| (*v - CENTRE).abs() < 1e-6));
        slicer.note_off(7);
        let out = play(&mut slicer, 600);
        // 441 frames at 44100 Hz, frame i of them at (1 + cos(i pi / 442)) / 2.
        for (i, v) in out.iter().enumerate().take(441) {
            let g = ((i + 1) as f64 * PI / 442.0).cos().mul_add(0.5, 0.5);
            assert!((f64::from(*v) - g * f64::from(CENTRE)).abs() < 1e-6, "{i}");
        }
        assert!(out[441..].iter().all(|v| *v == 0.0));
        assert!(slicer.voices.is_empty());
    }

    #[test]
    fn the_volume_envelope_s_release_comes_before_the_declick() {
        let file = vec![1.0; 40_000];
        let setup = Setup {
            articulation: Some(articulation()),
            ..Setup::default()
        };
        let mut slicer = slicer(&setup, &file);
        slicer.set_tempo(120.0);
        slicer.set_ppq(96);
        // 6500 is (1001^(6500/65536) - 1) * 12.288 = 12.1, so 12 ticks,
        // and the release is down after 13 ticks of 229.6875 frames.
        let release = slicer.release().unwrap();
        let (ticks, tick) = slicer.release_ticks(release);
        assert_eq!(ticks, 12.0);
        assert_eq!(tick, 229.6875);
        slicer.note_on(note(1, 62));
        play(&mut slicer, 10);
        slicer.note_off(1);
        let out = play(&mut slicer, 4_000);
        // The tension of -101 bends it down fast: (100.95^x - 1) / 99.95.
        let x = 1.0 - 1000.0 / (13.0 * tick);
        let level = FruitySlicer::level(-101, x);
        assert!((level - (100.95f64.powf(x) - 1.0) / 99.95).abs() < 1e-3);
        assert!((f64::from(out[1000]) - level * f64::from(CENTRE)).abs() < 1e-4);
        assert!(out.windows(2).all(|w| w[1] <= w[0]));
        assert!(out[3_000..].iter().all(|v| *v == 0.0));
        assert!(slicer.voices.is_empty());
    }

    #[test]
    fn reversed_slices_play_backwards_and_fades_shape_the_ends() {
        let setup = Setup {
            starts: vec![(0, true), (100, false)],
            ..Setup::default()
        };
        let mut reversed = slicer(&setup, &ramp(400));
        reversed.note_on(note(1, 60));
        let out = play(&mut reversed, 100);
        assert!((out[0] - 0.099 * CENTRE).abs() < 1e-6);
        assert_eq!(out[99], 0.0);
        // A 1 ms attack and decay: 44 frames of half a cosine at each end.
        let file = vec![1.0; 1000];
        let setup = Setup {
            attack: 1,
            decay: 1,
            starts: vec![(0, false), (500, false)],
            ..Setup::default()
        };
        let slicer = slicer(&setup, &file);
        let wave = slicer.wave.as_ref().unwrap();
        for j in 0..44 {
            let rise = cosine((j + 1) as f64 / 45.0);
            assert!((f64::from(wave.left[j]) - rise).abs() < 1e-6, "{j}");
            assert!((f64::from(wave.left[499 - j]) - rise).abs() < 1e-6, "{j}");
            assert!((f64::from(wave.left[500 + j]) - rise).abs() < 1e-6, "{j}");
        }
        assert_eq!(wave.left[250], 1.0);
    }

    #[test]
    fn the_declicker_takes_a_slice_s_end_down_to_nothing() {
        let file = vec![0.5; 4000];
        let setup = Setup {
            declick: true,
            starts: vec![(0, false), (2000, false)],
            ..Setup::default()
        };
        let slicer = slicer(&setup, &file);
        let wave = slicer.wave.as_ref().unwrap();
        // 28 ms is 1235 frames: before them the slice is as it was.
        assert_eq!(wave.left[2000 - 1236], 0.5);
        assert!(wave.left[2000 - 1235] < 0.5);
        assert_eq!(wave.left[1999], 0.0);
        assert!(wave.left[1990].abs() < 0.01);
    }

    #[test]
    fn a_stretched_loop_plays_its_slices_unstretched_in_their_new_places() {
        let setup = Setup {
            time: 10_000,
            ..Setup::default()
        };
        let slicer = slicer(&setup, &ramp(400));
        assert_eq!(slicer.wave.as_ref().unwrap().len(), 800);
        assert_eq!(
            slicer.regions[1],
            Region {
                start: 200,
                len: 300
            }
        );
        assert_eq!(slicer.wave.as_ref().unwrap().left[200], 0.1);
        assert_eq!(slicer.wave.as_ref().unwrap().left[350], 0.0);
        assert!(matches!(slicer.fidelity(), Fidelity::Approximate(_)));
    }

    #[test]
    fn a_state_fl_does_not_write_is_turned_down() {
        let mut slicer = FruitySlicer::default();
        let mut bytes = state(&Setup::default());
        assert!(slicer.set_state(&bytes[..bytes.len() - 1]).is_err());
        bytes[0] = 17;
        assert_eq!(
            slicer.set_state(&bytes),
            Err(StateError::Version { found: 17 })
        );
    }

    #[test]
    fn every_state_in_fl_s_files_loads_and_plays() {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, Fruity Slicer's states skipped");
            return;
        };
        let mut saved = testing::every_preset(&root, NAME);
        saved.extend(testing::projects(&root));
        let file: Vec<f32> = (0..400_000)
            .map(|i| ((i % 97) as f32 - 48.0) / 50.0)
            .collect();
        let mut states = 0;
        for saved in saved {
            if !saved.plugin.eq_ignore_ascii_case(NAME) {
                continue;
            }
            let mut slicer = crate::generator_by_name(&saved.plugin).unwrap();
            slicer.set_sample_rate(48_000.0);
            if let Err(error) = slicer.set_state(&saved.state) {
                panic!("{}: {error}", saved.file);
            }
            for path in slicer.sample_paths() {
                slicer.set_sample(
                    &path,
                    Some(SampleFile {
                        left: &file,
                        right: Some(&file),
                        rate: 44_100,
                    }),
                );
            }
            for key in 0..128 {
                slicer.note_on(note(key as u32, key));
            }
            let (mut left, mut right) = (vec![0.0; 2048], vec![0.0; 2048]);
            slicer.process(&mut left, &mut right);
            for key in 0..128 {
                slicer.note_off(key);
            }
            slicer.process(&mut left, &mut right);
            assert!(
                left.iter().chain(&right).all(|v| v.is_finite()),
                "{}",
                saved.file
            );
            states += 1;
        }
        eprintln!("Fruity Slicer: {states} states");
        assert!(states >= 13, "{states}");
    }
}
