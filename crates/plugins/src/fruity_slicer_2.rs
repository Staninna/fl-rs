//! Fruity Slicer 2: one sample cut into up to 128 slices, each with its own
//! key, pitch, volume, pan, play mode, envelope and filter, as
//! `docs/plugins/fruity-slicer-2.md` reads it out of the plugin's
//! `engine_x64.dll`.
//!
//! Slicer 2 shares no code with the first Fruity Slicer. Its Delphi shell
//! is a panel and a sample editor; the sound is in a C++ engine of FL's
//! newer plugins (Kepler's kind), with its own state, its own voices and
//! its own read of the sample.
//!
//! # The state
//!
//! FL's wrapper writes a version and chunks; chunk 1 holds a 12-byte shell
//! header, the engine's 12-byte header and the engine's stream: "slc1", a
//! version (1 to 4), a count of tracks (the engine plays the first), the
//! sample's auto-slicing settings and root key, then per track its output,
//! volume and pan (FL's channel volume and pan as the host hands them
//! over), pitch, fine tune, stretch mode, cut, filter, the sample's path
//! (or, when the state embeds it, the sample itself), its tempo and its
//! slices.
//!
//! # A note
//!
//! FL's note is a MIDI note to the engine: its key and a velocity of
//! `int(ln(10 * gain + 1) * 41.714)` out of 127. Every slice on the key
//! starts a voice of the 16 (the oldest is taken when none is free): it
//! reads the sample from the slice's start to its end, to the sample's end
//! or backwards, as its mode says, through a four-point Hermite curve at
//! `2^(semitones / 12)` times the file's rate over the host's, the
//! semitones the track's and the slice's pitch and fine tune. Then the
//! slice's state-variable filter, its envelope (attack, hold, decay,
//! sustain, release), the velocity's curve and the slice's pan and volume;
//! the voices' sum goes through the track's filter and the track's volume
//! and pan.
//!
//! # What is not FL's
//!
//! - Elastique (stretch mode 2) is not rebuilt: such a track is resampled
//!   to the tempo as mode 1 does, so its pitch moves with the tempo.
//! - The engine's 128 outputs all go to the one stereo output here.
//! - FL's `FPE_MIDI_Vol` and `FPE_MIDI_Pan`: the track keeps the volume and
//!   pan the state saved, and the host puts the channel's on after.
//! - A note's pan and the engine's parameters for automation are not
//!   taken.

use crate::menvelope::Reader;
use crate::sampler::{Bank, Chunks, Wave};
use crate::{Fidelity, Generator, NoteOn, SampleFile, StateError};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Slicer 2";

/// The stream's versions the engine reads (`FUN_180150340`).
const VERSIONS: std::ops::RangeInclusive<i32> = 1..=4;

/// The most slices a track has (`FUN_18014d630`).
const SLICES: usize = 128;

/// The voices of a track (`FUN_18014ddd0`).
const VOICES: usize = 16;

/// The rate the engine works its envelope times out at, whatever the
/// host's (`FUN_18014bed0` sets it once, `FUN_18014c990` reads it).
const ENVELOPE_RATE: f64 = 44_100.0;

/// The curve of the velocity and the volume, before the volume's gain of
/// `10^0.3` (`FUN_18014c2d0`, `FUN_18014b450`).
fn velocity_curve(v: f32) -> f32 {
    (v * 0.984_35 * v * v - v * 0.231_573 * v) + v * 0.247_223
}

/// A volume knob's gain (`FUN_18014b450`).
fn volume_curve(v: f32) -> f32 {
    velocity_curve(v) * 10f64.powf(f64::from(0.3f32)) as f32
}

/// The engine's pan law (`FUN_18014c890`, `FUN_18014f5f0`): left and right
/// for a pan from 0 to 1, 1 a side in the centre.
fn pan_law(pan: f32) -> (f32, f32) {
    let p = pan.clamp(0.0, 1.0);
    let q = 1.0 - p;
    (
        (1.828 - q * 0.828) * q * 1.414_427_2,
        (1.828 - p * 0.828) * p * 1.414_427_2,
    )
}

/// `1 - 0.1^x`, as the engine makes its smoothing coefficients.
fn smoothing(x: f64) -> f32 {
    (1.0 - 0.099_999_999_999_999_98f64.powf(x)) as f32
}

/// The tension curve of the envelope's stages (`FUN_180151e10`,
/// `FUN_180151ef0`). The engine sets all three to a tension of -1 and
/// never moves them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Curve {
    k: f32,
    scale: f32,
    log: f32,
}

impl Curve {
    /// A tension from -1 to 1.
    pub fn new(tension: f32) -> Self {
        let sign = if tension > 0.0 { 1.0 } else { -1.0 };
        let grow = (6347f64.ln() * f64::from(tension.abs())).exp();
        let k = (grow - 1.0) as f32 * sign * 0.1;
        if k == 0.0 {
            return Curve {
                k,
                scale: 0.0,
                log: 0.0,
            };
        }
        Curve {
            k,
            scale: (1.0 / f64::from(k).abs()) as f32,
            log: f64::from(k.abs() + 1.0).ln() as f32,
        }
    }

    /// The curve at `x` from 0 to 1.
    pub fn at(&self, x: f32) -> f32 {
        if self.k == 0.0 {
            return x;
        }
        if self.k <= 0.0 {
            return ((f64::from(self.log * x).exp() - 1.0) * f64::from(self.scale)) as f32;
        }
        (1.0 - (f64::from((1.0 - x) * self.log).exp() - 1.0) * f64::from(self.scale)) as f32
    }
}

/// One slice as the state holds it (`FUN_18014b5a0`).
#[derive(Debug, Clone, PartialEq)]
pub struct Slice {
    pub name: String,
    /// The output it goes to, -1 for the track's.
    pub output: i32,
    /// Its key, -1 for the root key plus its place.
    pub key: i32,
    pub start: i32,
    pub end: i32,
    pub volume: f32,
    pub pan: f32,
    /// Semitones, -24 to 24.
    pub pitch: i32,
    /// From 0 to 1, 0.5 in tune: a semitone either way.
    pub fine: f32,
    /// Bit 0: a note off does not end it; bit 1: it plays to the sample's
    /// end; bit 2: it plays backwards.
    pub mode: i32,
    /// Seconds of fade before its end, playing forwards.
    pub fade: f32,
    /// 0 off, 1 lowpass, 2 highpass, 3 bandpass.
    pub filter: i32,
    pub cutoff: f32,
    pub resonance: f32,
    pub attack: f32,
    pub hold: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
}

impl Default for Slice {
    /// As `FUN_18014b510` makes one.
    fn default() -> Self {
        Slice {
            name: String::new(),
            output: -1,
            key: -1,
            start: 0,
            end: 0,
            volume: 0.7694,
            pan: 0.5,
            pitch: 0,
            fine: 0.5,
            mode: 0,
            fade: 0.0,
            filter: 0,
            cutoff: 1.0,
            resonance: 0.0,
            attack: 0.0,
            hold: 0.0,
            decay: 0.5,
            sustain: 1.0,
            release: 0.1,
        }
    }
}

/// The track: the sample, its slices and what goes on all of them
/// (`FUN_180150340`, `FUN_18014bbf0`).
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub output: i32,
    pub volume: f32,
    pub pan: f32,
    pub pitch: i32,
    pub fine: f32,
    /// 0 resampled, 1 resampled to the tempo, 2 Elastique.
    pub mode: i32,
    /// A note stops what sounds.
    pub cut: bool,
    pub filter: i32,
    pub cutoff: f32,
    pub resonance: f32,
    pub unknown_18: i32,
    /// The slice the panel shows.
    pub selected: i32,
    pub path: String,
    /// The loop's own tempo.
    pub tempo: f32,
    /// The slice the keys play chromatically, -1 for none.
    pub chromatic: i32,
    pub unknown_2c: f32,
    pub slices: Vec<Slice>,
    /// Version 4 on.
    pub unknown_380c: i32,
}

/// The state's head: the auto-slicer's settings and the root key.
#[derive(Debug, Clone, PartialEq)]
pub struct Head {
    pub version: i32,
    /// The state carries the sample's frames.
    pub embed: i32,
    pub unknown_14: i32,
    pub unknown_18: i32,
    pub unknown_1c: f32,
    pub unknown_20: u8,
    pub unknown_24: f32,
    pub root: i32,
}

/// The envelope of a voice (`FUN_180150bf0` and the stages after it).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Envelope {
    level: f32,
    phase: f64,
    from: f32,
    stage: u8,
    count: i32,
    attack: f64,
    hold: i32,
    decay: f64,
    sustain: f32,
    release: f64,
    quick: f64,
}

impl Envelope {
    fn new() -> Self {
        Envelope {
            level: 0.0,
            phase: 0.0,
            from: 0.0,
            stage: 7,
            count: 0,
            attack: 1.0,
            hold: 0,
            decay: 1.0,
            sustain: 0.0,
            release: 1.0,
            // `FUN_18014bed0`: 1 ms at the fixed rate.
            quick: f64::from(1.0 / (ENVELOPE_RATE as f32 * 0.001)),
        }
    }

    /// The times of a slice (`FUN_18014c990`).
    fn set(&mut self, slice: &Slice) {
        let time = |p: f32, unit: f64| -> f64 {
            let t = if p == 0.0 {
                0.0
            } else {
                2f64.powf(f64::from(p * 15.0)) * unit
            };
            if t as f32 == 0.0 {
                1.0
            } else {
                f64::from(1.0 / (ENVELOPE_RATE as f32 * t as f32))
            }
        };
        self.attack = time(slice.attack, 0.001);
        self.hold = (f64::from(slice.hold * 10.0) * ENVELOPE_RATE) as i32;
        self.decay = time(slice.decay, f64::from(0.001f32));
        self.sustain = slice.sustain;
        if self.stage == 4 {
            self.level = self.sustain;
            self.phase = f64::from(self.sustain);
        }
        self.release = time(slice.release, f64::from(0.001f32));
    }

    /// Back to the start of the attack (`FUN_18014c2d0`).
    fn start(&mut self) {
        self.level = 0.0;
        self.phase = 0.0;
        self.stage = 1;
        self.count = 0;
    }

    /// The release from a note off (`FUN_180150110`).
    fn release(&mut self) {
        if !(5..=7).contains(&self.stage) {
            self.from = self.level;
            self.phase = 1.0;
            self.stage = 5;
        }
    }

    /// The 1 ms release at a slice's end or a cut (`FUN_18014dd40`).
    fn stop(&mut self) {
        if !(6..=7).contains(&self.stage) {
            if self.stage != 5 {
                self.from = self.level;
                self.phase = 1.0;
            }
            self.stage = 6;
        }
    }

    /// One frame (`FUN_180150ee0` and its neighbours).
    fn tick(&mut self, curve: &Curve) {
        match self.stage {
            1 => {
                self.phase += self.attack;
                if self.phase >= 1.0 {
                    self.stage = 2;
                    self.count = 0;
                    self.level = 1.0;
                } else {
                    self.level = curve.at(self.phase as f32);
                }
            }
            2 => {
                self.count += 1;
                if self.count > self.hold {
                    self.stage = 3;
                    self.phase = 1.0;
                }
            }
            3 => {
                self.phase -= self.decay;
                if self.phase < 0.0 {
                    self.stage = 4;
                    self.level = self.sustain;
                } else {
                    self.level = curve.at(self.phase as f32) * (1.0 - self.sustain) + self.sustain;
                }
            }
            4 => {
                self.phase = f64::from(self.sustain);
                self.level = self.sustain;
            }
            5 | 6 => {
                self.phase -= if self.stage == 5 {
                    self.release
                } else {
                    self.quick
                };
                if self.phase < 0.0 {
                    self.phase = 0.0;
                }
                let level = curve.at(self.phase as f32) * self.from;
                self.level = level;
                if level < 1e-6 {
                    self.level = 0.0;
                    self.stage = 7;
                }
            }
            _ => {}
        }
    }
}

/// The engine's state-variable filter, in the trapezoidal form
/// (`FUN_18014c5d0`, `FUN_18014dee0`, the frame in `FUN_18014d180`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Svf {
    a: [f32; 3],
    m: [f32; 3],
    ic1: [f32; 2],
    ic2: [f32; 2],
}

impl Svf {
    /// The coefficients for a type, a cutoff and a resonance from 0 to 1.
    fn design(&mut self, kind: i32, cutoff: f32, resonance: f32, pi_t: f32) {
        if !(1..=3).contains(&kind) {
            return;
        }
        let hz = (2f64.powf(f64::from(cutoff * 10.583_333)) * 14.350_000_381_469_727) as f32;
        let g = f64::from(hz * pi_t).tan() as f32;
        let k = 1.0 - resonance * 0.7225 * 0.99;
        self.m = match kind {
            1 => [0.0, 0.0, 1.0],
            2 => [1.0, -k, -1.0],
            _ => [0.0, 1.0, 0.0],
        };
        let a1 = 1.0 / ((g + k) * g + 1.0);
        let a2 = a1 * g;
        self.a = [a1, a2, a2 * g];
    }

    fn run(&mut self, side: usize, x: f32) -> f32 {
        let [a1, a2, a3] = self.a;
        let (ic1, ic2) = (self.ic1[side], self.ic2[side]);
        let v3 = x - ic2;
        let v1 = a2 * v3 + ic1 * a1;
        let v2 = ic1 * a2 + ic2 + a3 * v3;
        self.ic1[side] = v1 + v1 - ic1;
        self.ic2[side] = v2 + v2 - ic2;
        self.m[1] * v1 + self.m[0] * x + self.m[2] * v2
    }
}

/// A voice (0x238 bytes in the engine, `FUN_18014bed0`).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Voice {
    /// The key whose note off releases it.
    key: i32,
    slice: usize,
    serial: u32,
    releasing: bool,
    /// Bit 0 of the slice's mode.
    keeps: bool,
    pos: f64,
    end: i32,
    forward: bool,
    step: f64,
    count: i32,
    /// Semitones from the slice's key, in chromatic play.
    offset: f32,
    /// Semitones of the note's fine pitch.
    bend: f32,
    velocity: f32,
    cutoff: f32,
    resonance: f32,
    filter: Svf,
    gains: (f32, f32),
    smoothed: (f32, f32),
    envelope: Envelope,
}

impl Voice {
    fn new() -> Self {
        Voice {
            key: -1,
            slice: 0,
            serial: 0,
            releasing: false,
            keeps: false,
            pos: 0.0,
            end: 0,
            forward: true,
            step: 0.0,
            count: 0,
            offset: 0.0,
            bend: 0.0,
            velocity: 0.0,
            cutoff: 0.5,
            resonance: 0.5,
            filter: Svf::default(),
            gains: (0.0, 0.0),
            smoothed: (0.0, 0.0),
            envelope: Envelope::new(),
        }
    }
}

/// The 4-point Hermite read of the engine (`FUN_18014cb90`), with the
/// zeros the engine keeps around a sample.
fn hermite(side: &[f32], i: usize, t: f32) -> f32 {
    let at = |k: usize| side.get(k).copied().unwrap_or(0.0);
    let before = if i == 0 { 0.0 } else { at(i - 1) };
    let (x0, x1, x2) = (at(i), at(i + 1), at(i + 2));
    let c1 = (x1 - before) * 0.5;
    let w = (x0 - x1) + c1;
    let a = (x2 - x0) * 0.5 + w + (x0 - x1);
    ((a * t - (a + w)) * t + c1) * t + x0
}

/// Fruity Slicer 2.
#[derive(Debug, Clone)]
pub struct FruitySlicer2 {
    pub head: Head,
    /// The tracks the state holds; the engine plays the first.
    pub tracks: Vec<Track>,
    embedded: Option<Wave>,
    bank: Bank,
    rate: f32,
    tempo: f64,
    pi_t: f32,
    /// Frames between the updates of the parameters.
    every: i32,
    /// The smoothing of the cutoffs, once an update.
    slow: f32,
    /// The smoothing of the gains, every frame.
    fast: f32,
    curve: Curve,
    voices: [Voice; VOICES],
    /// The sounding voices, in the order they started.
    active: [usize; VOICES],
    sounding: usize,
    serial: u32,
    held: Vec<(u32, i32)>,
    // The track's own.
    count: i32,
    cutoff: f32,
    resonance: f32,
    filter: Svf,
    gains: (f32, f32),
    smoothed: (f32, f32),
}

impl Default for FruitySlicer2 {
    fn default() -> Self {
        let mut plugin = FruitySlicer2 {
            head: Head {
                version: 4,
                embed: 0,
                unknown_14: 1,
                unknown_18: 16,
                unknown_1c: 0.75,
                unknown_20: 0,
                unknown_24: 0.5,
                root: 60,
            },
            tracks: Vec::new(),
            embedded: None,
            bank: Bank::default(),
            rate: 44_100.0,
            tempo: 120.0,
            pi_t: 0.0,
            every: 10,
            slow: 0.0,
            fast: 0.0,
            curve: Curve::new(-1.0),
            voices: [Voice::new(); VOICES],
            active: [0; VOICES],
            sounding: 0,
            serial: 0,
            held: Vec::with_capacity(128),
            count: 0,
            cutoff: 0.0,
            resonance: 0.0,
            filter: Svf::default(),
            gains: (1.0, 1.0),
            smoothed: (1.0, 1.0),
        };
        plugin.set_rate(44_100.0);
        plugin
    }
}

/// A string of the stream: an i32 length and that many bytes, the last a
/// NUL (`FUN_180148720` writes it).
fn string(reader: &mut Reader<'_>) -> Result<String, StateError> {
    let len = reader.i32()?;
    let len = usize::try_from(len).map_err(|_| StateError::Length {
        expected: reader.bytes.len() + 1,
        found: reader.bytes.len(),
    })?;
    let bytes = reader.bytes(len)?;
    let text = String::from_utf8_lossy(bytes);
    Ok(text.split('\0').next().unwrap_or_default().to_owned())
}

/// What a state holds.
struct Parsed {
    head: Head,
    tracks: Vec<Track>,
    embedded: Option<Wave>,
}

/// The engine's stream inside FL's wrapper chunk.
fn stream(bytes: &[u8]) -> Result<&[u8], StateError> {
    let short = StateError::Length {
        expected: 40,
        found: bytes.len(),
    };
    let first = bytes.first_chunk::<4>().ok_or(short)?;
    // FL's wrapper: a version of 1 and chunks; chunk 1 is the plugin's.
    let body = if u32::from_le_bytes(*first) == 1 {
        let mut body = None;
        for chunk in Chunks::new(&bytes[4..]) {
            let (id, chunk) = chunk?;
            if id == 1 && body.is_none() {
                body = Some(chunk);
            }
        }
        body.ok_or(short)?
    } else {
        bytes
    };
    // The shell's 12 bytes (0, the length, 0) and the engine's (1, 8, 0).
    body.get(24..).ok_or(short)
}

fn read(bytes: &[u8]) -> Result<Parsed, StateError> {
    let stream = stream(bytes)?;
    let mut reader = Reader::new(stream);
    let magic = reader.bytes(4)?;
    if magic != b"slc1" {
        return Err(StateError::Version {
            found: i32::from_le_bytes([magic[0], magic[1], magic[2], magic[3]]),
        });
    }
    let version = reader.i32()?;
    if !VERSIONS.contains(&version) {
        return Err(StateError::Version { found: version });
    }
    let count = reader.i32()?;
    let head = Head {
        version,
        embed: reader.i32()?,
        unknown_14: reader.i32()?,
        unknown_18: reader.i32()?,
        unknown_1c: reader.f32()?,
        unknown_20: reader.u8()?,
        unknown_24: reader.f32()?,
        root: reader.i32()?,
    };
    let mut tracks = Vec::new();
    let mut embedded = None;
    let too_long = StateError::Length {
        expected: stream.len() + 1,
        found: stream.len(),
    };
    for n in 0..count.max(0) {
        let mut track = Track {
            output: reader.i32()?,
            volume: reader.f32()?,
            pan: reader.f32()?,
            pitch: reader.i32()?,
            fine: reader.f32()?,
            mode: reader.i32()?,
            cut: reader.u8()? != 0,
            filter: reader.i32()?,
            cutoff: reader.f32()?,
            resonance: reader.f32()?,
            unknown_18: reader.i32()?,
            selected: reader.i32()?,
            path: string(&mut reader)?,
            tempo: 0.0,
            chromatic: -1,
            unknown_2c: 0.0,
            slices: Vec::new(),
            unknown_380c: 0,
        };
        let slices = usize::try_from(reader.i32()?).map_err(|_| too_long)?;
        if slices > SLICES {
            return Err(too_long);
        }
        track.tempo = reader.f32()?;
        track.chromatic = reader.i32()?;
        track.unknown_2c = reader.f32()?;
        if reader.i32()? == 1 {
            let frames = reader.i32()?;
            let channels = reader.i32()?;
            let rate = reader.i32()?;
            if frames > 0 {
                let frames = usize::try_from(frames).map_err(|_| too_long)?;
                let mut sides = Vec::new();
                for _ in 0..channels.max(0) {
                    let raw = reader.bytes(frames.checked_mul(4).ok_or(too_long)?)?;
                    let side: Vec<f32> = raw
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|b| f32::from_le_bytes(*b))
                        .collect();
                    sides.push(side);
                }
                if n == 0 && !sides.is_empty() {
                    let left = sides.remove(0);
                    let right = (channels == 2).then(|| sides.remove(0));
                    embedded = Some(Wave {
                        left,
                        right,
                        rate: u32::try_from(rate).unwrap_or(44_100),
                    });
                }
            }
        }
        for _ in 0..slices {
            let mut slice = Slice {
                output: reader.i32()?,
                key: reader.i32()?,
                start: reader.i32()?,
                end: reader.i32()?,
                volume: reader.f32()?,
                pan: reader.f32()?,
                pitch: reader.i32()?,
                fine: reader.f32()?,
                mode: i32::from(reader.u8()? as i8),
                fade: reader.f32()?,
                filter: reader.i32()?,
                cutoff: reader.f32()?,
                resonance: reader.f32()?,
                attack: reader.f32()?,
                hold: reader.f32()?,
                decay: reader.f32()?,
                sustain: reader.f32()?,
                release: reader.f32()?,
                ..Slice::default()
            };
            if version > 2 {
                slice.name = string(&mut reader)?;
            }
            track.slices.push(slice);
        }
        if version > 3 {
            track.unknown_380c = reader.i32()?;
        }
        tracks.push(track);
    }
    reader.end()?;
    Ok(Parsed {
        head,
        tracks,
        embedded,
    })
}

impl FruitySlicer2 {
    /// The rate's part (`FUN_18014f4a0`, `FUN_18014c180`).
    fn set_rate(&mut self, hz: f32) {
        self.rate = hz;
        let rate = f64::from(hz);
        self.pi_t = (std::f64::consts::PI / rate) as f32;
        self.every = ((rate / 4410.0) as i32).max(1);
        self.slow = smoothing(0.022_675_736_961_451_247);
        self.fast = smoothing(1.0 / (rate * 0.01));
    }

    /// The track's gains from its volume and pan (`FUN_18014f5f0`).
    fn track_gains(&mut self) {
        if let Some(track) = self.tracks.first() {
            let (l, r) = pan_law(track.pan);
            let v = volume_curve(track.volume);
            self.gains = (l * v, r * v);
        }
    }

    fn wave(&self) -> Option<&Wave> {
        self.embedded.as_ref().or_else(|| self.bank.wave(0))
    }

    /// A free voice, or the oldest (`FUN_18014ddd0`).
    fn take(&mut self) -> usize {
        let place = if self.sounding < VOICES {
            let used = &self.active[..self.sounding];
            let free = (0..VOICES).rev().find(|v| !used.contains(v));
            free.unwrap_or(0)
        } else {
            let oldest = (0..self.sounding)
                .min_by_key(|k| self.voices[self.active[*k]].serial)
                .unwrap_or(0);
            let voice = self.active[oldest];
            self.active.copy_within(oldest + 1..self.sounding, oldest);
            self.sounding -= 1;
            voice
        };
        self.serial = self.serial.wrapping_add(1);
        self.voices[place].serial = self.serial;
        place
    }

    /// Every sounding voice to its 1 ms release (`FUN_18014dd40`).
    fn stop_all(&mut self) {
        for k in 0..self.sounding {
            let v = &mut self.voices[self.active[k]];
            v.releasing = true;
            v.envelope.stop();
        }
    }

    /// A voice of a slice starts (`FUN_18014c2d0`).
    fn start(&mut self, slice: usize, velocity: f32, key: i32, offset: f32, bend: f32) {
        let Some(len) = self.wave().map(Wave::len) else {
            return;
        };
        let Some(track) = self.tracks.first() else {
            return;
        };
        // The fields a voice takes, copied: the slice's name stays put.
        let Some(s) = track.slices.get(slice).map(|s| Slice {
            name: String::new(),
            ..*s
        }) else {
            return;
        };
        let place = self.take();
        let len = i32::try_from(len).unwrap_or(i32::MAX);
        let (start, end) = match s.mode {
            0 | 1 => (s.start, s.end),
            2 | 3 => (s.start, len),
            4 | 5 => (s.end, s.start),
            6 | 7 => (s.end, 0),
            _ => (0, 0),
        };
        let start = start.max(0).min(len);
        let end = end.max(0).min(len);
        let v = &mut self.voices[place];
        v.key = key;
        v.slice = slice;
        v.releasing = false;
        v.keeps = s.mode & 1 != 0 && (0..8).contains(&s.mode);
        v.pos = f64::from(start);
        v.end = end;
        v.forward = start <= end;
        v.step = 0.0;
        v.count = 0;
        v.offset = offset;
        v.bend = bend;
        v.velocity = velocity_curve(velocity);
        v.cutoff = s.cutoff.clamp(0.0, 1.0);
        v.resonance = s.resonance.clamp(0.0, 1.0);
        v.filter = Svf::default();
        let (l, r) = pan_law(s.pan);
        let g = volume_curve(s.volume);
        v.gains = (l * g, r * g);
        v.smoothed = v.gains;
        v.envelope.start();
        self.active[self.sounding] = place;
        self.sounding += 1;
    }

    /// A MIDI note on (`FUN_18014db70`).
    fn key_on(&mut self, key: i32, velocity: f32, bend: f32) {
        if self.wave().is_none() {
            return;
        }
        let Some(track) = self.tracks.first() else {
            return;
        };
        let root = self.head.root;
        let cut = track.cut;
        let chromatic = track.chromatic;
        let count = track.slices.len();
        if chromatic < 0 {
            if cut {
                self.stop_all();
            }
            for i in 0..count {
                let own = self.tracks[0].slices[i].key;
                let at = if own < 0 { root + i as i32 } else { own };
                if at == key {
                    if cut {
                        self.stop_all();
                    }
                    self.start(i, velocity, key, 0.0, bend);
                }
            }
        } else {
            let slice = chromatic as usize;
            let own = track.slices.get(slice).map_or(-1, |s| s.key);
            let at = if own < 0 { root + chromatic } else { own };
            if cut {
                self.stop_all();
            }
            self.start(slice, velocity, key, (key - at) as f32, bend);
        }
    }

    /// A MIDI note off (`FUN_180150110`).
    fn key_off(&mut self, key: i32) {
        for k in 0..self.sounding {
            let v = &mut self.voices[self.active[k]];
            if v.key == key && !v.keeps {
                v.releasing = true;
                v.envelope.release();
            }
        }
    }

    /// One frame of one voice (`FUN_18014d180`).
    fn voice_frame(&mut self, place: usize) -> (f32, f32, bool) {
        let rate = self.rate;
        let tempo = self.tempo;
        let pi_t = self.pi_t;
        let slow = self.slow;
        let fast = self.fast;
        let every = self.every;
        let curve = self.curve;
        let Some(track) = self.tracks.first() else {
            return (0.0, 0.0, false);
        };
        let wave = self.embedded.as_ref().or_else(|| self.bank.wave(0));
        let v = &mut self.voices[place];
        let s = &track.slices[v.slice];
        if v.count == 0 {
            // The step (`FUN_18014d180`); Elastique's mode 2 stands in as
            // mode 1.
            if let Some(wave) = wave {
                let fine = ((track.fine - 0.5) + s.fine) - 0.5;
                let semis =
                    ((s.pitch + track.pitch) as f32 + fine + fine + (v.offset as i32) as f32)
                        + v.bend;
                let mut step =
                    2f64.powf(f64::from(semis / 12.0)) * (f64::from(wave.rate) / f64::from(rate));
                if track.mode == 1 || track.mode == 2 {
                    step *= tempo / f64::from(track.tempo);
                }
                v.step = step;
            } else {
                v.step = 0.0;
            }
            // The filter (`FUN_18014c5d0`).
            if s.filter != 0 {
                v.filter.design(s.filter, v.cutoff, v.resonance, pi_t);
                v.cutoff += (s.cutoff.clamp(0.0, 1.0) - v.cutoff) * slow;
                v.resonance += (s.resonance.clamp(0.0, 1.0) - v.resonance) * slow;
            }
            v.envelope.set(s);
            let (l, r) = pan_law(s.pan);
            let g = volume_curve(s.volume);
            v.gains = (l * g, r * g);
        }
        v.count += 1;
        if v.count == every {
            v.count = 0;
        }
        // The read (`FUN_18014cb90`).
        let (mut l, mut r) = (0.0, 0.0);
        let past = if v.forward {
            f64::from(v.end) <= v.pos
        } else {
            v.pos < f64::from(v.end)
        };
        if past || wave.is_none() {
            v.releasing = true;
            v.envelope.stop();
        } else if let Some(wave) = wave {
            let i = v.pos as usize;
            let t = (v.pos - i as f64) as f32;
            l = hermite(&wave.left, i, t);
            r = wave.right.as_deref().map_or(l, |side| hermite(side, i, t));
            if v.forward {
                v.pos += v.step;
            } else {
                v.pos -= v.step;
            }
            if s.fade > 0.0 && v.forward {
                let span = f64::from(s.fade) * f64::from(wave.rate);
                let from = f64::from(v.end) - span;
                if from <= v.pos {
                    let x = (((v.pos - from) / span) as f32).min(1.0);
                    l *= 1.0 - x;
                    r *= 1.0 - x;
                }
            }
        }
        if s.filter != 0 {
            l = v.filter.run(0, l);
            r = v.filter.run(1, r);
        }
        v.envelope.tick(&curve);
        let done = v.releasing && v.envelope.stage == 7;
        v.smoothed.0 += (v.gains.0 - v.smoothed.0) * fast;
        v.smoothed.1 += (v.gains.1 - v.smoothed.1) * fast;
        let e = v.velocity * v.envelope.level;
        (e * l * v.smoothed.0, e * r * v.smoothed.1, done)
    }

    /// One frame of the track (`FUN_18014e150`).
    fn frame(&mut self) -> (f32, f32) {
        let (mut l, mut r) = (0.0f32, 0.0f32);
        let (mut bl, mut br) = (0.0f32, 0.0f32);
        let mut k = self.sounding;
        while k > 0 {
            k -= 1;
            let place = self.active[k];
            let (a, b, done) = self.voice_frame(place);
            let own = self.tracks[0].slices[self.voices[place].slice].output;
            if own < 0 {
                l += a;
                r += b;
            } else {
                bl += a;
                br += b;
            }
            if done {
                self.active.copy_within(k + 1..self.sounding, k);
                self.sounding -= 1;
            }
        }
        let track = &self.tracks[0];
        let (kind, cutoff, resonance) = (track.filter, track.cutoff, track.resonance);
        if self.count == 0 && kind != 0 {
            self.filter
                .design(kind, self.cutoff, self.resonance, self.pi_t);
            self.cutoff += (cutoff - self.cutoff) * self.slow;
            self.resonance += (resonance - self.resonance) * self.slow;
        }
        self.count += 1;
        if self.count == self.every {
            self.count = 0;
        }
        if kind != 0 {
            l = self.filter.run(0, l);
            r = self.filter.run(1, r);
        }
        self.smoothed.0 += (self.gains.0 - self.smoothed.0) * self.fast;
        self.smoothed.1 += (self.gains.1 - self.smoothed.1) * self.fast;
        (self.smoothed.0 * l + bl, r * self.smoothed.1 + br)
    }

    /// Whether anything goes to an output other than the first.
    fn routed(&self) -> bool {
        self.tracks
            .first()
            .is_some_and(|track| track.output != 0 || track.slices.iter().any(|s| s.output >= 0))
    }
}

impl Generator for FruitySlicer2 {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let parsed = read(bytes)?;
        let mut bank = Bank::default();
        if parsed.embedded.is_none()
            && let Some(track) = parsed.tracks.first()
        {
            bank.place(&track.path);
        }
        let rate = self.rate;
        let tempo = self.tempo;
        *self = FruitySlicer2 {
            head: parsed.head,
            tracks: parsed.tracks,
            embedded: parsed.embedded,
            bank,
            tempo,
            ..FruitySlicer2::default()
        };
        self.set_rate(rate);
        self.track_gains();
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        self.set_rate(hz);
    }

    fn set_tempo(&mut self, bpm: f64) {
        self.tempo = bpm;
    }

    fn note_on(&mut self, note: NoteOn) {
        if note.slide {
            return;
        }
        let key = i32::from(note.key);
        if !(0..128).contains(&key) {
            return;
        }
        let v = ((f64::from(note.velocity) * 10.0 + 1.0).ln() as f32 * 41.714_26).clamp(0.0, 127.0);
        let velocity = (v as i32) as f32 / 127.0;
        if self.held.len() < self.held.capacity() {
            self.held.push((note.voice, key));
        }
        self.key_on(key, velocity, note.fine_pitch * 0.01);
    }

    fn note_off(&mut self, voice: u32) {
        if let Some(at) = self.held.iter().position(|(v, _)| *v == voice) {
            let (_, key) = self.held.remove(at);
            self.key_off(key);
        }
    }

    fn all_notes_off(&mut self) {
        while let Some((_, key)) = self.held.pop() {
            self.key_off(key);
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if self.tracks.is_empty() {
            return;
        }
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let (a, b) = self.frame();
            *l += a;
            *r += b;
        }
    }

    fn param(&self, _index: usize) -> f32 {
        0.0
    }

    fn set_param(&mut self, _index: usize, _value: f32) {}

    fn reset(&mut self) {
        self.sounding = 0;
        self.held.clear();
    }

    /// The whole sample two octaves down and the longest release.
    fn tail(&self) -> usize {
        let Some(wave) = self.wave() else {
            return 1;
        };
        let frames = wave.len() as f64 / f64::from(wave.rate) * f64::from(self.rate) * 4.0;
        (frames + 2f64.powf(15.0) * 0.001 * f64::from(self.rate)) as usize + 1
    }

    fn fidelity(&self) -> Fidelity {
        if self.embedded.is_none() && self.bank.lacking() {
            return Fidelity::Approximate(
                "Fruity Slicer 2: its sample file is missing, and its slices are silent",
            );
        }
        if self.tracks.first().is_some_and(|t| t.mode == 2) {
            return Fidelity::Approximate(
                "Fruity Slicer 2: Elastique is left out, and the slices are resampled to the tempo",
            );
        }
        if self.routed() {
            return Fidelity::Approximate(
                "Fruity Slicer 2: its other outputs are mixed into the first",
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
        self.sounding = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    const RATE: u32 = 44_100;

    fn string_to(bytes: &mut Vec<u8>, text: &str) {
        bytes.extend((text.len() as i32 + 1).to_le_bytes());
        bytes.extend(text.as_bytes());
        bytes.push(0);
    }

    /// A version 4 state as `FUN_1801505c0` writes it, inside FL's
    /// wrapper, with one track: the sample embedded when `file` is given
    /// and else named by `path`.
    fn state(track: &Track, file: Option<&[f32]>, path: &str) -> Vec<u8> {
        let mut s = b"slc1".to_vec();
        for word in [4, 1, i32::from(file.is_some()), 1, 16] {
            s.extend(word.to_le_bytes());
        }
        s.extend(0.75f32.to_le_bytes());
        s.push(0);
        s.extend(0.5f32.to_le_bytes());
        s.extend(60i32.to_le_bytes());
        s.extend(track.output.to_le_bytes());
        s.extend(track.volume.to_le_bytes());
        s.extend(track.pan.to_le_bytes());
        s.extend(track.pitch.to_le_bytes());
        s.extend(track.fine.to_le_bytes());
        s.extend(track.mode.to_le_bytes());
        s.push(u8::from(track.cut));
        s.extend(track.filter.to_le_bytes());
        s.extend(track.cutoff.to_le_bytes());
        s.extend(track.resonance.to_le_bytes());
        s.extend(0i32.to_le_bytes());
        s.extend(0i32.to_le_bytes());
        string_to(&mut s, path);
        s.extend((track.slices.len() as i32).to_le_bytes());
        s.extend(track.tempo.to_le_bytes());
        s.extend(track.chromatic.to_le_bytes());
        s.extend(1f32.to_le_bytes());
        s.extend(i32::from(file.is_some()).to_le_bytes());
        if let Some(file) = file {
            for word in [file.len() as i32, 1, RATE as i32] {
                s.extend(word.to_le_bytes());
            }
            for x in file {
                s.extend(x.to_le_bytes());
            }
        }
        for slice in &track.slices {
            for word in [slice.output, slice.key, slice.start, slice.end] {
                s.extend(word.to_le_bytes());
            }
            s.extend(slice.volume.to_le_bytes());
            s.extend(slice.pan.to_le_bytes());
            s.extend(slice.pitch.to_le_bytes());
            s.extend(slice.fine.to_le_bytes());
            s.push(slice.mode as u8);
            s.extend(slice.fade.to_le_bytes());
            s.extend(slice.filter.to_le_bytes());
            for x in [
                slice.cutoff,
                slice.resonance,
                slice.attack,
                slice.hold,
                slice.decay,
                slice.sustain,
                slice.release,
            ] {
                s.extend(x.to_le_bytes());
            }
            string_to(&mut s, &slice.name);
        }
        s.extend(0i32.to_le_bytes());
        let mut chunk = Vec::new();
        for word in [0, s.len() as i32 + 12, 0, 1, 8, 0] {
            chunk.extend(word.to_le_bytes());
        }
        chunk.extend(s);
        let mut bytes = 1u32.to_le_bytes().to_vec();
        bytes.extend(1i32.to_le_bytes());
        bytes.extend((chunk.len() as i64).to_le_bytes());
        bytes.extend(chunk);
        bytes.extend(2i32.to_le_bytes());
        bytes.extend(4i64.to_le_bytes());
        bytes.extend([0; 4]);
        bytes
    }

    fn track(slices: &[(i32, i32)]) -> Track {
        Track {
            output: 0,
            volume: 0.787_401_56,
            pan: 0.5,
            pitch: 0,
            fine: 0.5,
            mode: 0,
            cut: false,
            filter: 0,
            cutoff: 1.0,
            resonance: 0.0,
            unknown_18: 0,
            selected: 0,
            path: String::new(),
            tempo: 120.0,
            chromatic: -1,
            unknown_2c: 1.0,
            slices: slices
                .iter()
                .map(|(start, end)| Slice {
                    start: *start,
                    end: *end,
                    name: "s".into(),
                    ..Slice::default()
                })
                .collect(),
            unknown_380c: 0,
        }
    }

    fn slicer(track: &Track, file: &[f32]) -> FruitySlicer2 {
        let mut plugin = FruitySlicer2::default();
        plugin.set_sample_rate(RATE as f32);
        plugin.set_state(&state(track, Some(file), "")).unwrap();
        plugin
    }

    fn note(voice: u32, key: i16) -> NoteOn {
        NoteOn {
            voice,
            key,
            ..NoteOn::default()
        }
    }

    fn play(plugin: &mut FruitySlicer2, len: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut left, mut right) = (vec![0.0; len], vec![0.0; len]);
        plugin.process(&mut left, &mut right);
        (left, right)
    }

    fn ramp(len: usize) -> Vec<f32> {
        (0..len).map(|i| (i % 500) as f32 / 1000.0 + 0.01).collect()
    }

    fn sine(len: usize, period: f32) -> Vec<f32> {
        let w = std::f32::consts::TAU / period;
        (0..len).map(|i| (i as f32 * w).sin() * 0.5).collect()
    }

    fn crossings(sound: &[f32]) -> usize {
        sound
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count()
    }

    /// `Default.fst`'s state, as FL 2026 writes it.
    const DEFAULT: &str = "0100000001000000ce0000000000000000000000c20000000000000001000000\
        0800000000000000736c63310300000001000000000000000100000010000000\
        0000403f000000003f3c000000000000002693493f0000003f00000000000000\
        3f0000000000000000000000803f000000000000000000000000010000000001\
        0000000000f042ffffffff0000803f00000000ffffffffffffffff0000000000\
        000000b81e453f0000003f000000000000003f0000000000000000000000803f\
        0000000000000000000000000000003f0000803fcdcccc3d0200000031000200\
        0000380000000000000001000000000000000000000000000000000000000000\
        0000000000000000000000000000000000000000000000000000000000000000\
        0000";

    fn default_state() -> Vec<u8> {
        let hex: String = DEFAULT.chars().filter(|c| !c.is_whitespace()).collect();
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn the_preset_fl_ships_reads_to_its_last_byte() {
        let bytes = default_state();
        assert_eq!(bytes.len(), 290);
        let mut plugin = FruitySlicer2::default();
        plugin.set_state(&bytes).unwrap();
        assert_eq!(plugin.head.version, 3);
        assert_eq!(plugin.head.root, 60);
        let track = &plugin.tracks[0];
        assert_eq!(track.tempo, 120.0);
        assert_eq!(track.chromatic, -1);
        assert_eq!(track.path, "");
        assert_eq!(track.slices.len(), 1);
        let slice = &track.slices[0];
        assert_eq!((slice.name.as_str(), slice.key, slice.start), ("1", -1, 0));
        assert_eq!((slice.volume, slice.decay, slice.release), (0.77, 0.5, 0.1));
        assert!(plugin.sample_paths().is_empty());
        // No sample: a note is silent.
        plugin.note_on(note(1, 60));
        let (left, _) = play(&mut plugin, 256);
        assert!(left.iter().all(|x| *x == 0.0));
        // Cut short, or another plugin's, it is turned down.
        assert!(plugin.set_state(&bytes[..200]).is_err());
        let mut other = bytes.clone();
        other[40] = b'x';
        assert!(plugin.set_state(&other).is_err());
    }

    #[test]
    fn each_key_from_the_root_plays_its_slice_at_the_file_s_pitch() {
        let file = ramp(2000);
        let mut plugin = slicer(&track(&[(0, 300), (300, 1000), (1000, 2000)]), &file);
        let fast = plugin.fast;
        let (target, _) = plugin.gains;
        let s = Slice::default();
        let gain = pan_law(s.pan).0 * volume_curve(s.volume);
        let velocity =
            velocity_curve((((10f64 + 1.0).ln() as f32 * 41.714_26) as i32) as f32 / 127.0);
        let mut smoothed = 1.0f32;
        plugin.note_on(note(1, 61));
        let (left, right) = play(&mut plugin, 800);
        assert_eq!(left, right);
        for (n, out) in left.iter().enumerate() {
            smoothed += (target - smoothed) * fast;
            let expected = if n < 700 {
                velocity * file[300 + n] * gain * smoothed
            } else {
                0.0
            };
            assert!((out - expected).abs() < 1e-6, "{n}: {out} {expected}");
        }
        // A key no slice sits on is silent.
        plugin.note_on(note(2, 70));
        let (left, _) = play(&mut plugin, 100);
        assert!(left.iter().all(|x| *x == 0.0));
    }

    #[test]
    fn an_octave_up_doubles_the_pitch_and_backwards_plays_backwards() {
        let file = sine(20_000, 100.0);
        let mut t = track(&[(0, 20_000)]);
        t.slices[0].pitch = 12;
        let mut plugin = slicer(&t, &file);
        plugin.note_on(note(1, 60));
        let (left, _) = play(&mut plugin, 8000);
        let n = crossings(&left);
        assert!((318..=322).contains(&n), "{n}");
        // An octave up runs through the slice in half the time.
        let (rest, _) = play(&mut plugin, 4000);
        assert!(rest[2100..].iter().all(|x| x.abs() < 1e-6));

        let file: Vec<f32> = (0..1000).map(|i| i as f32 / 1000.0 + 0.01).collect();
        let mut t = track(&[(100, 600)]);
        t.slices[0].mode = 4;
        let mut plugin = slicer(&t, &file);
        plugin.note_on(note(1, 60));
        let (left, _) = play(&mut plugin, 600);
        // From the slice's end down to its start, one frame a frame.
        assert!(left[10] > left[200] && left[200] > left[400]);
        assert!(left[505..].iter().all(|x| *x == 0.0));
    }

    #[test]
    fn a_note_off_releases_unless_the_slice_ignores_it() {
        let file = vec![0.5; 40_000];
        let mut plugin = slicer(&track(&[(0, 40_000)]), &file);
        plugin.note_on(note(1, 60));
        play(&mut plugin, 1000);
        plugin.note_off(1);
        let (left, _) = play(&mut plugin, 1000);
        // A release of 0.1 is 2^1.5 ms.
        assert!(left[0] > 0.1);
        assert!(left[200..].iter().all(|x| *x == 0.0));
        assert_eq!(plugin.sounding, 0);

        let mut t = track(&[(0, 40_000)]);
        t.slices[0].mode = 1;
        let mut plugin = slicer(&t, &file);
        plugin.note_on(note(1, 60));
        play(&mut plugin, 1000);
        plugin.note_off(1);
        let (left, _) = play(&mut plugin, 1000);
        assert!(left[999] > 0.1);
    }

    #[test]
    fn cut_stops_what_sounds_and_sixteen_voices_take_the_oldest() {
        let file = vec![0.5; 40_000];
        let mut t = track(&[(0, 40_000)]);
        t.cut = true;
        let mut plugin = slicer(&t, &file);
        plugin.note_on(note(1, 60));
        play(&mut plugin, 100);
        plugin.note_on(note(2, 60));
        play(&mut plugin, 200);
        assert_eq!(plugin.sounding, 1);

        let mut plugin = slicer(&track(&[(0, 40_000)]), &file);
        for v in 0..20 {
            plugin.note_on(note(v, 60));
            play(&mut plugin, 10);
        }
        assert_eq!(plugin.sounding, VOICES);
        let mut serials: Vec<u32> = (0..VOICES)
            .map(|k| plugin.voices[plugin.active[k]].serial)
            .collect();
        serials.sort_unstable();
        assert_eq!(serials, (5..21).collect::<Vec<u32>>());
    }

    #[test]
    fn the_hermite_read_lands_on_the_frames_and_follows_a_line() {
        let side = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0];
        for (i, x) in side.iter().enumerate().take(4).skip(1) {
            assert_eq!(hermite(&side, i, 0.0), *x);
            assert!((hermite(&side, i, 0.25) - (x + 0.25)).abs() < 1e-6);
        }
        // Past the ends are the engine's zeros.
        assert_eq!(hermite(&side, 5, 0.0), 5.0);
        assert!(hermite(&side, 5, 0.5) < 5.0);
    }

    #[test]
    fn the_envelope_s_curve_runs_from_0_to_1_bowed_down() {
        let curve = Curve::new(-1.0);
        assert!(curve.at(0.0).abs() < 1e-6);
        assert!((curve.at(1.0) - 1.0).abs() < 1e-5);
        assert!(curve.at(0.5) < 0.05);
        let straight = Curve::new(0.0);
        assert!((straight.at(0.3) - 0.3).abs() < 1e-6);
        // An attack knob p takes 2^(15 p) ms at the engine's fixed rate.
        let mut envelope = Envelope::new();
        let slice = Slice {
            attack: 0.5,
            ..Slice::default()
        };
        envelope.set(&slice);
        envelope.start();
        let frames = (2f64.powf(7.5) * 44.1) as usize;
        for _ in 0..frames - 1 {
            envelope.tick(&curve);
        }
        assert_eq!(envelope.stage, 1);
        envelope.tick(&curve);
        envelope.tick(&curve);
        assert_eq!(envelope.stage, 2);
    }

    #[test]
    fn the_filter_passes_below_its_cutoff_and_stops_above() {
        let level = |kind: i32, period: f32| {
            let mut svf = Svf::default();
            let pi_t = (std::f64::consts::PI / 44_100.0) as f32;
            // 14.35 * 2^(10.58 * 0.5): about 593 Hz.
            svf.design(kind, 0.5, 0.0, pi_t);
            let x = sine(44_100, period);
            let y: Vec<f32> = x.iter().map(|v| svf.run(0, *v)).collect();
            y[22_050..].iter().fold(0.0f32, |m, v| m.max(v.abs()))
        };
        assert!(level(1, 441.0) > 0.45, "{}", level(1, 441.0));
        assert!(level(1, 8.0) < 0.01, "{}", level(1, 8.0));
        assert!(level(2, 441.0) < 0.05, "{}", level(2, 441.0));
        assert!(level(2, 8.0) > 0.45, "{}", level(2, 8.0));
    }

    #[test]
    fn a_named_sample_comes_from_the_host() {
        let file = ramp(1000);
        let mut plugin = FruitySlicer2::default();
        plugin.set_sample_rate(RATE as f32);
        plugin
            .set_state(&state(&track(&[(0, 500)]), None, "C:\\loops\\a.wav"))
            .unwrap();
        assert_eq!(plugin.sample_paths(), ["C:\\loops\\a.wav"]);
        assert!(matches!(plugin.fidelity(), Fidelity::Approximate(_)));
        plugin.set_sample(
            "C:\\loops\\a.wav",
            Some(SampleFile {
                left: &file,
                right: None,
                rate: RATE,
            }),
        );
        assert_eq!(plugin.fidelity(), Fidelity::Exact);
        plugin.note_on(note(1, 60));
        let (left, _) = play(&mut plugin, 600);
        assert!(left[100] > 0.0 && left[550] == 0.0);
    }

    #[test]
    fn every_state_fl_ships_loads() {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, Fruity Slicer 2's states skipped");
            return;
        };
        let ours = |saved: &testing::Saved| saved.plugin.eq_ignore_ascii_case(NAME);
        let projects: Vec<_> = testing::projects(&root).into_iter().filter(ours).collect();
        let presets = testing::every_preset(&root, NAME);
        assert!(!presets.is_empty());
        for saved in projects.iter().chain(&presets) {
            let mut plugin = FruitySlicer2::default();
            plugin
                .set_state(&saved.state)
                .unwrap_or_else(|e| panic!("{}: {e:?}", saved.file));
            plugin.note_on(note(1, 60));
            let (left, right) = play(&mut plugin, 4410);
            assert!(left.iter().chain(&right).all(|x| x.is_finite()));
        }
    }
}
