//! Wave Traveller: a sample scratched along a curve, one curve a key, as
//! `docs/plugins/wave-traveller.md` and the listings of
//! `Wave Traveller_x64.dll` have it.
//!
//! # The state
//!
//! Chunks of an `i32` id and an `i32` size (`0x74c030`), to a chunk 3000
//! that ends them:
//!
//! - 1000: the version (FL writes 1000005), the key the editor shows and
//!   the sample's path.
//! - 2000: the patch of one key, 0 to 119: its length in beats, its speed
//!   in Hz, its attack and release in ms, the start and the end of the
//!   sample it plays as parts of the whole, whether its speed is in beats
//!   or in Hz, a name, the points where its sound goes off and on again,
//!   and the points of its two curves: where in the sample it is, and how
//!   loud. FL writes a size for this chunk that is short of what it
//!   writes, so the chunk is read through and its size not trusted.
//!
//! A key the state has no patch for keeps the patch of a new plugin: the
//! whole sample once in 4 beats at full volume. The plugin has no
//! parameters a host can move.
//!
//! # The curves
//!
//! Each curve is drawn into a table of up to 1024 points, 1023 to the
//! width of the editor (`0x7558c0`). A point's shape says how the stretch
//! after it goes to the next point: 1 a straight line, 2 half a cosine, 3
//! a Bezier curve through the points' handles, 4 a quarter sine, 5 a
//! quarter cosine up, 6 an exponential bent by the point's tension, 8
//! silence. Every value is held between 0 and 1.
//!
//! # A note
//!
//! The key picks the patch. A note walks along the position table at
//! `(table - 1) * tempo / (beats * 60 * rate)` points a sample, or for a
//! patch in Hz at `44100 * hz * (table - 1) / ((frames - 1) * (end -
//! start))`, made faster or slower by its fine pitch, `1 + cents / 1200`.
//! The place in the sample is the table there, between two points in a
//! straight line, over the start to the end. The sample is read there by
//! a six-point curve (`0x752740`), through a DC blocker of
//! `1 - 126 / rate`, by gains that follow FL's pan law with the note's pan
//! and the volume curve by 1% a sample, and that the off points set to 0.
//! The note fades in and out over its attack and release in straight
//! lines. At the end of the table it plays its last 256 samples backwards
//! as they fade out.
//!
//! What is not FL's:
//!
//! - FL's host hands the plugin the sample as 16-bit stereo at 44.1 kHz.
//!   Here the file is held to 16 bits, and a file at another rate is
//!   taken to 44.1 kHz by a Hermite curve, which fidelity says.
//! - FL's high quality mode, a windowed sinc of 64 to 256 points that its
//!   render to file may ask for, is not built: the realtime reader plays.

use crate::sampler::{circular, words};
use crate::{Fidelity, Generator, NoteOn, SampleFile, StateError};
use dsp::resample::read_hermite;

/// The plugin's event 201 name.
pub const NAME: &str = "Wave Traveller";

/// The keys that have a patch.
const KEYS: usize = 120;

/// The points of a curve's table, and one past them that FL reads as 0.
const TABLE: usize = 1024;

/// How many notes sound at once. A note past them cuts the oldest.
const VOICES: usize = 32;

/// The samples a note's end plays back.
const RING: usize = 256;

/// The rate FL's host hands the sample over at.
const HOST_RATE: u32 = 44_100;

/// One point of a curve as the state has it: 60 bytes (`0x737090`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
    /// The handles before and after it, from the point.
    pub before: (f32, f32),
    pub after: (f32, f32),
    pub tension: f32,
    /// How the stretch after it is drawn.
    pub shape: i32,
}

impl Point {
    const fn new(x: f32, y: f32) -> Self {
        Point {
            x,
            y,
            before: (-0.042, 0.0),
            after: (0.042, 0.0),
            tension: 0.5,
            shape: 1,
        }
    }

    fn read(bytes: &[u8]) -> Self {
        let f = |at: usize| f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap_or([0; 4]));
        Point {
            x: f(0x8),
            y: f(0xc),
            before: (f(0x14), f(0x18)),
            after: (f(0x20), f(0x24)),
            tension: f(0x28),
            shape: i32::from_le_bytes(bytes[0x30..0x34].try_into().unwrap_or([0; 4])),
        }
    }
}

/// A key's patch.
#[derive(Debug, Clone, PartialEq)]
pub struct Patch {
    /// The length in beats and the speed in Hz.
    pub beats: f32,
    pub hz: f32,
    /// Milliseconds.
    pub attack: f32,
    pub release: f32,
    /// Of the whole sample.
    pub start: f32,
    pub end: f32,
    /// The speed is in Hz and not in beats.
    pub in_hz: bool,
    pub name: String,
    /// Where the sound goes off or on, in parts of the table.
    pub mutes: Vec<(bool, f32)>,
    pub position: Vec<Point>,
    pub volume: Vec<Point>,
}

impl Default for Patch {
    /// The patch of a new plugin (`0x7516a0`).
    fn default() -> Self {
        Patch {
            beats: 4.0,
            hz: 1.0,
            attack: 1.0,
            release: 1.0,
            start: 0.0,
            end: 1.0,
            in_hz: false,
            name: String::new(),
            mutes: vec![(true, 0.0)],
            position: vec![Point::new(0.0, 0.0), Point::new(1.0, 1.0)],
            volume: vec![Point::new(0.0, 1.0), Point::new(1.0, 1.0)],
        }
    }
}

/// A curve drawn into its table (`0x7558c0`). How many points it has
/// comes back; the table has room for [`TABLE`] and the point after them.
pub fn draw(points: &[Point], table: &mut [f32; TABLE + 1]) -> usize {
    table.fill(0.0);
    let mut at: i32 = 1;
    let mut put = |at: &mut i32, value: f32| {
        let held = if value.is_sign_negative() {
            0.0
        } else if value.to_bits() < 0x3f80_0001 {
            value
        } else {
            1.0
        };
        if let Some(slot) = usize::try_from(*at).ok().and_then(|k| table.get_mut(k)) {
            *slot = held;
        }
        *at += 1;
    };
    for pair in points.windows(2) {
        let (p, q) = (&pair[0], &pair[1]);
        let n = f64::from((q.x - p.x) * 1023.0).round_ties_even() as i32;
        at -= 1;
        let (y0, y1) = (p.y, q.y);
        match p.shape {
            1 => {
                let mut value = y0;
                for _ in 0..n.max(0) {
                    put(&mut at, value);
                    value += (y1 - y0) / (n - 1) as f32;
                }
            }
            2 | 4 | 5 => {
                let span = match p.shape {
                    2 => PI_TRUNC,
                    _ => HALF_PI_TRUNC,
                };
                let step = (span / f64::from(n - 1)) as f32;
                let half = (f64::from(y1 - y0) * 0.5) as f32;
                let (mut value, mut along) = (y0, 0.0f32);
                for _ in 0..n.max(0) {
                    put(&mut at, value);
                    along += step;
                    let a = f64::from(along);
                    value = match p.shape {
                        2 => f64::from(y0) + f64::from(half) * ((a + PI_TRUNC).cos() + 1.0),
                        4 => f64::from(y0) + f64::from(y1 - y0) * a.sin(),
                        _ => f64::from(y0) + f64::from(y1 - y0) * ((a + PI_TRUNC).cos() + 1.0),
                    } as f32;
                }
            }
            3 => {
                let bezier = bezier(p, q);
                let mut value = y0;
                let mut found = 0;
                for k in 0..n.max(0) {
                    put(&mut at, value);
                    let x = ((k + 1) as f32 * (q.x - p.x)) / (n - 1) as f32 + p.x;
                    if q.x < x {
                        break;
                    }
                    if bezier[found].0 == x {
                        value = bezier[found].1;
                    } else {
                        while found < bezier.len() - 1 && bezier[found].0 < x {
                            found += 1;
                        }
                        let (bx, by) = bezier[found];
                        if bx == x || found == 0 {
                            value = by;
                        } else {
                            let (ax, ay) = bezier[found - 1];
                            value = ay + (x - ax) * ((by - ay) / (bx - ax));
                        }
                    }
                }
            }
            6 => {
                if f64::from(p.tension) == 0.5 {
                    let mut value = y0;
                    for _ in 0..n.max(0) {
                        put(&mut at, value);
                        value += (y1 - y0) / (n - 2) as f32;
                    }
                } else {
                    let t = p.tension;
                    let k = (1.0 - t * 2.0) / (t * t);
                    let log = f64::from(k + 1.0).ln();
                    let step = (1.0 / f64::from(n - 1)) as f32;
                    let (mut value, mut along) = (y0, 0.0f32);
                    for _ in 0..n.max(0) {
                        along += step;
                        put(&mut at, value);
                        let grown = (f64::from(along * log as f32)).exp() - 1.0;
                        value = (f64::from(y0) + f64::from((y1 - y0) / k) * grown) as f32;
                    }
                }
            }
            8 => {
                for _ in 0..n.max(0) {
                    put(&mut at, 0.0);
                }
            }
            _ => {}
        }
    }
    (at - 1).clamp(0, TABLE as i32) as usize
}

/// FL's pi and half of it, to 8 places (`0x7575f0`, `0x757608`).
#[allow(clippy::approx_constant)]
const PI_TRUNC: f64 = 3.1415926;
#[allow(clippy::approx_constant)]
const HALF_PI_TRUNC: f64 = 1.5707963;

/// The 256 points of the Bezier curve from `p` to `q` (`0x757620`), whose
/// handles are three times their length.
fn bezier(p: &Point, q: &Point) -> [(f32, f32); 256] {
    let (c0x, c0y) = (p.after.0 * 3.0 + p.x, p.after.1 * 3.0 + p.y);
    let (c1x, c1y) = (q.before.0 * 3.0 + q.x, q.before.1 * 3.0 + q.y);
    let step = 1.0 / 255.0;
    std::array::from_fn(|k| {
        let t = k as f64 * step;
        let u = 1.0 - t;
        let (a, b, c, d) = (u * u * u, t * 3.0 * u * u, t * 3.0 * t * u, t * t * t);
        let x = a * f64::from(p.x) + b * f64::from(c0x) + c * f64::from(c1x) + d * f64::from(q.x);
        let y = a * f64::from(p.y) + b * f64::from(c0y) + c * f64::from(c1y) + d * f64::from(q.y);
        (x as f32, y as f32)
    })
}

/// A patch drawn.
#[derive(Debug, Clone, PartialEq)]
struct Drawn {
    position: Box<[f32; TABLE + 1]>,
    positions: usize,
    volume: Box<[f32; TABLE + 1]>,
    volumes: usize,
}

impl Drawn {
    fn of(patch: &Patch) -> Self {
        let mut position = Box::new([0.0; TABLE + 1]);
        let mut volume = Box::new([0.0; TABLE + 1]);
        let positions = draw(&patch.position, &mut position);
        let volumes = draw(&patch.volume, &mut volume);
        Drawn {
            position,
            positions,
            volume,
            volumes,
        }
    }
}

/// The stages of a note (`+0x2034`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Attack,
    Playing,
    Released,
    /// The table is over and the last samples play back.
    End,
}

/// One note.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Voice {
    id: u32,
    key: usize,
    /// Where it is in the table, and what a sample adds to that.
    at: f64,
    speed: f32,
    /// The frames the table's 0 and 1 stand for.
    start: i32,
    range: i32,
    stage: Stage,
    level: f32,
    attack: f32,
    release: f32,
    /// The next off or on point, the sound is on, and there are points
    /// left.
    mute: usize,
    on: bool,
    muting: bool,
    /// The volume table's point and its value.
    counted: i32,
    volume: f32,
    pan: f32,
    vol: f32,
    gains: [f32; 2],
    /// The DC blocker's last input and output, each side.
    dc: [[f32; 2]; 2],
    ring: [[f32; 2]; RING],
    written: i32,
    ended: i32,
}

/// Wave Traveller. A new one is at 44.1 kHz and 140 beats a minute with
/// every key on the new patch and no sample.
#[derive(Debug, Clone, PartialEq)]
pub struct WaveTraveller {
    version: i32,
    current: i32,
    path: String,
    patches: Vec<Patch>,
    drawn: Vec<Drawn>,
    /// The sample as FL's host hands it over: stereo at 44.1 kHz in 16
    /// bits.
    frames: Vec<[f32; 2]>,
    loaded: bool,
    missing: bool,
    resampled: bool,
    rate: f32,
    tempo: f32,
    /// The DC blocker's coefficient (`+0x44c`).
    dc: f32,
    voices: Vec<Voice>,
}

impl Default for WaveTraveller {
    fn default() -> Self {
        let patches = vec![Patch::default(); KEYS];
        let drawn = patches.iter().map(Drawn::of).collect();
        let mut plugin = WaveTraveller {
            version: 1_000_005,
            current: 60,
            path: String::new(),
            patches,
            drawn,
            frames: Vec::new(),
            loaded: false,
            missing: false,
            resampled: false,
            rate: 44_100.0,
            tempo: 140.0,
            dc: 0.0,
            voices: Vec::with_capacity(VOICES),
        };
        plugin.set_sample_rate(44_100.0);
        plugin
    }
}

/// A little-endian reader that says how far it got.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, len: usize) -> Result<&[u8], StateError> {
        let end = self
            .at
            .checked_add(len)
            .filter(|end| *end <= self.bytes.len());
        let end = end.ok_or(StateError::Length {
            expected: self.at.saturating_add(len),
            found: self.bytes.len(),
        })?;
        let taken = &self.bytes[self.at..end];
        self.at = end;
        Ok(taken)
    }

    fn i32(&mut self) -> Result<i32, StateError> {
        let bytes = self.take(4)?;
        Ok(i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn f32(&mut self) -> Result<f32, StateError> {
        self.i32().map(|word| f32::from_bits(word as u32))
    }

    /// A count of things, which must fit in what is left at `size` bytes
    /// each.
    fn count(&mut self, size: usize) -> Result<usize, StateError> {
        let count = usize::try_from(self.i32()?).unwrap_or(0);
        let left = self.bytes.len() - self.at;
        if count > left / size.max(1) {
            return Err(StateError::Length {
                expected: self.at + count * size,
                found: self.bytes.len(),
            });
        }
        Ok(count)
    }

    fn text(&mut self) -> Result<String, StateError> {
        let len = self.count(1)?;
        Ok(self.take(len)?.iter().map(|b| char::from(*b)).collect())
    }
}

impl WaveTraveller {
    /// The patch of a key, 0 to 119.
    pub fn patch(&self, key: usize) -> Option<&Patch> {
        self.patches.get(key)
    }

    /// The path the state names.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Take a decoded file as FL's host hands it over (`0x757e00`).
    fn take_file(&mut self, file: SampleFile<'_>) {
        let quantize = |value: f32| {
            (f64::from(value) * 32_768.0)
                .round()
                .clamp(-32_768.0, 32_767.0)
        };
        let to_16 = |value: f32| (quantize(value) / 32_768.0) as f32;
        let right = file.right.unwrap_or(file.left);
        self.resampled = file.rate != HOST_RATE;
        let frames: Vec<[f32; 2]> = if self.resampled && file.rate > 0 {
            let ratio = f64::from(file.rate) / f64::from(HOST_RATE);
            let len = (file.left.len() as f64 / ratio) as usize;
            (0..len)
                .map(|k| {
                    let at = k as f64 * ratio;
                    [
                        to_16(read_hermite(file.left, at)),
                        to_16(read_hermite(right, at)),
                    ]
                })
                .collect()
        } else {
            let pairs = file.left.iter().zip(right);
            pairs.map(|(l, r)| [to_16(*l), to_16(*r)]).collect()
        };
        self.frames = frames;
        self.loaded = !self.frames.is_empty();
        self.voices.clear();
    }

    /// The note of a key as it starts (`0x754fd0`).
    fn start(&self, note: &NoteOn) -> Option<Voice> {
        let key = usize::try_from(note.key).ok().filter(|key| *key < KEYS)?;
        let patch = &self.patches[key];
        let drawn = &self.drawn[key];
        let len = self.frames.len() as i32;
        let points = drawn.positions as i32;
        let mut speed = if patch.in_hz {
            let span = (len - 1) as f32 * (patch.end - patch.start);
            (f64::from(HOST_RATE) * f64::from(patch.hz) * f64::from(points - 1) / f64::from(span))
                as f32
        } else {
            (self.tempo * (points - 1) as f32) / (patch.beats * (self.rate as i32 * 60) as f32)
        };
        let pitch = (f32::from(note.key) - 60.0) * 100.0 + note.fine_pitch;
        let cents = pitch - ((i32::from(note.key) - 60) * 100) as f32;
        if pitch < 0.0 {
            speed /= 1.0 - cents / 1200.0;
        } else {
            speed *= cents / 1200.0 + 1.0;
        }
        let frame = |part: f32| f64::from(part * (len - 1) as f32) as i32;
        let (start, end) = (frame(patch.start), frame(patch.end));
        let ms = self.rate as i32 as f32;
        let volume = drawn.volume[0];
        let (left, right) = circular(note.pan, note.velocity * volume);
        Some(Voice {
            id: note.voice,
            key,
            at: 0.0,
            speed,
            start,
            range: end - start,
            stage: Stage::Attack,
            level: 0.0,
            attack: 1000.0 / (patch.attack * ms),
            release: 1000.0 / (patch.release * ms),
            mute: 0,
            on: true,
            muting: !patch.mutes.is_empty(),
            counted: 0,
            volume,
            pan: note.pan,
            vol: note.velocity,
            gains: [left, right],
            dc: [[0.0; 2]; 2],
            ring: [[0.0; 2]; RING],
            written: 0,
            ended: 0,
        })
    }

    /// The six-point curve through the sample at `place` on one side
    /// (`0x752740`), with nothing before the start or after the end.
    fn read(&self, place: f64, side: usize) -> f32 {
        let frames = &self.frames;
        let len = frames.len() as i64;
        let at = place as i64;
        let get = |k: i64| {
            usize::try_from(k)
                .ok()
                .filter(|_| k < len)
                .and_then(|k| frames.get(k))
                .map_or(0.0, |frame| frame[side])
        };
        let [m2, m1, p0, p1, p2, p3] = [-2, -1, 0, 1, 2, 3].map(|k| get(at + k));
        let x = place - at as f64;
        let d = |a: f32, b: f32| f64::from(a - b);
        let c3 = d(m2, p3) * (1.0 / 12.0) + d(p2, m1) * (7.0 / 12.0) + d(p0, p1) * (4.0 / 3.0);
        let c2 = ((((f64::from(m1) * 1.25 - f64::from(p0) * (7.0 / 3.0))
            + f64::from(p1) * (5.0 / 3.0))
            - f64::from(p2) * 0.5)
            + f64::from(p3) * (1.0 / 12.0))
            - f64::from(m2) * (1.0 / 6.0);
        let c1 = d(m2, p2) * (1.0 / 12.0) + d(p1, m1) * (2.0 / 3.0);
        (((c3 * x + c2) * x + c1) * x + f64::from(p0)) as f32
    }

    /// One sample of a voice, added to `out`. Whether it is over comes
    /// back.
    fn step_voice(&self, voice: &mut Voice, out: &mut [f32; 2]) -> bool {
        let patch = &self.patches[voice.key];
        let drawn = &self.drawn[voice.key];
        let points = drawn.positions as i32;
        if voice.muting {
            let (on, x) = patch.mutes[voice.mute];
            if f64::from(x) <= voice.at / f64::from(points - 1) {
                voice.on = on;
                voice.mute += 1;
                if voice.mute == patch.mutes.len() {
                    voice.muting = false;
                }
            }
        }
        if f64::from(voice.counted) < voice.at {
            voice.counted += 1;
            if voice.counted < drawn.volumes as i32 {
                voice.volume = drawn.volume[voice.counted as usize];
            }
        }
        match voice.stage {
            Stage::Attack => {
                voice.level += voice.attack;
                if voice.level.to_bits() > 0x3f80_0000 {
                    voice.level = 1.0;
                    voice.stage = Stage::Playing;
                }
            }
            Stage::Released => {
                voice.level -= voice.release;
                if voice.level.is_sign_negative() {
                    return true;
                }
            }
            Stage::End => {
                let fade = (RING as i32 - voice.ended) as f32;
                let back = voice.ring[(voice.written & 0xff) as usize];
                for (side, sum) in out.iter_mut().enumerate() {
                    *sum = (f64::from(*sum) + f64::from(fade * back[side]) * (1.0 / 256.0)) as f32;
                }
                voice.ended += 1;
                voice.written -= 1;
                return voice.ended == RING as i32;
            }
            Stage::Playing => {}
        }
        let whole = voice.at as usize;
        let table = &drawn.position;
        let (here, next) = (table[whole], table.get(whole + 1).copied().unwrap_or(0.0));
        let along = f64::from(here) + (voice.at - whole as f64) * f64::from(next - here);
        let place = along * f64::from(voice.range) + f64::from(voice.start);
        let (left, right) = circular(voice.pan, voice.vol);
        for (side, gain) in [left, right].into_iter().enumerate() {
            let kept = f64::from(voice.gains[side]) * 0.99;
            voice.gains[side] = match voice.on {
                true => (kept + f64::from(gain) * 0.01 * f64::from(voice.volume)) as f32,
                false => kept as f32,
            };
            let value = self.read(place, side);
            let [last_in, last_out] = voice.dc[side];
            let mut blocked = (value - last_in) + self.dc * last_out;
            if blocked.to_bits() & 0x7fff_ffff == 0 {
                blocked = 0.0;
            }
            voice.dc[side] = [value, blocked];
            let sample = voice.gains[side] * voice.level * blocked;
            voice.ring[(voice.written & 0xff) as usize][side] = sample;
            out[side] += sample;
        }
        voice.written += 1;
        voice.at += f64::from(voice.speed);
        if f64::from(points - 1) < voice.at {
            voice.stage = Stage::End;
            voice.written -= 1;
        }
        false
    }
}

impl Generator for WaveTraveller {
    /// The notes that sound are cut. A chunk with another id is passed
    /// over by its size.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let mut reader = Reader { bytes, at: 0 };
        let mut patches = vec![Patch::default(); KEYS];
        let (mut version, mut current, mut path) = (0, 60, String::new());
        loop {
            let id = reader.i32()?;
            let size = reader.i32()?;
            match id {
                1000 => {
                    version = reader.i32()?;
                    current = reader.i32()?;
                    path = reader.text()?;
                }
                2000 => {
                    let key = reader.i32()?;
                    let [beats, hz, attack, release, start, end] =
                        [(); 6].map(|_| reader.f32().unwrap_or(0.0));
                    let in_hz = reader.i32()? != 0;
                    let name = reader.text()?;
                    let mutes = reader.count(8)?;
                    let mutes = (0..mutes)
                        .map(|_| {
                            let pair: Vec<i32> = words(reader.take(8).unwrap_or(&[0; 8])).collect();
                            (pair[0] & 0xff != 0, f32::from_bits(pair[1] as u32))
                        })
                        .collect();
                    let curve = |reader: &mut Reader| -> Result<Vec<Point>, StateError> {
                        let count = reader.count(60)?;
                        (0..count)
                            .map(|_| reader.take(60).map(Point::read))
                            .collect()
                    };
                    let position = curve(&mut reader)?;
                    let volume = curve(&mut reader)?;
                    let patch = Patch {
                        beats,
                        hz,
                        attack,
                        release,
                        start,
                        end,
                        in_hz,
                        name,
                        mutes,
                        position,
                        volume,
                    };
                    let Some(slot) = usize::try_from(key).ok().and_then(|k| patches.get_mut(k))
                    else {
                        return Err(StateError::Range {
                            param: 0,
                            value: key,
                        });
                    };
                    *slot = patch;
                }
                3000 => break,
                _ => {
                    reader.take(usize::try_from(size).unwrap_or(usize::MAX))?;
                }
            }
        }
        self.version = version;
        self.current = current;
        if path != self.path {
            self.frames.clear();
            self.loaded = false;
            self.missing = false;
        }
        self.path = path;
        self.drawn = patches.iter().map(Drawn::of).collect();
        self.patches = patches;
        self.voices.clear();
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        self.rate = hz;
        self.dc = (1.0 - 126.0 / f64::from(hz as i32)) as f32;
        self.voices.clear();
    }

    fn set_tempo(&mut self, bpm: f64) {
        if bpm.is_finite() && bpm > 0.0 {
            self.tempo = bpm as f32;
        }
    }

    fn sample_paths(&self) -> Vec<String> {
        match self.path.is_empty() {
            true => Vec::new(),
            false => vec![self.path.clone()],
        }
    }

    fn set_sample(&mut self, path: &str, file: Option<SampleFile<'_>>) {
        if path != self.path {
            return;
        }
        self.missing = file.is_none();
        match file {
            Some(file) => self.take_file(file),
            None => {
                self.frames.clear();
                self.loaded = false;
                self.voices.clear();
            }
        }
    }

    /// Nothing sounds without a sample, nor for a key past 119.
    fn note_on(&mut self, note: NoteOn) {
        if !self.loaded {
            return;
        }
        if let Some(voice) = self.start(&note) {
            if self.voices.len() == VOICES {
                self.voices.remove(0);
            }
            self.voices.push(voice);
        }
    }

    fn note_off(&mut self, voice: u32) {
        for sounding in self.voices.iter_mut().filter(|v| v.id == voice) {
            if sounding.stage != Stage::End {
                sounding.stage = Stage::Released;
            }
        }
    }

    fn all_notes_off(&mut self) {
        for sounding in &mut self.voices {
            if sounding.stage != Stage::End {
                sounding.stage = Stage::Released;
            }
        }
    }

    /// FL renders the notes last to first.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let mut voices = std::mem::take(&mut self.voices);
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let mut k = voices.len();
            while k > 0 {
                k -= 1;
                let mut out = [*l, *r];
                if self.step_voice(&mut voices[k], &mut out) {
                    voices.remove(k);
                }
                [*l, *r] = out;
            }
        }
        self.voices = voices;
    }

    fn param(&self, _index: usize) -> f32 {
        0.0
    }

    fn set_param(&mut self, _index: usize, _value: f32) {}

    fn reset(&mut self) {
        self.voices.clear();
    }

    /// The longest release of a patch and the 256 samples of an end.
    fn tail(&self) -> usize {
        let longest = self
            .patches
            .iter()
            .map(|p| p.release)
            .fold(0.0f32, f32::max);
        (f64::from(longest) * 0.001 * f64::from(self.rate)).ceil() as usize + RING + 2
    }

    fn fidelity(&self) -> Fidelity {
        if !self.path.is_empty() && !self.loaded {
            Fidelity::Approximate("Wave Traveller sample not available")
        } else if self.resampled {
            Fidelity::Approximate("Wave Traveller sample not at 44.1 kHz")
        } else {
            Fidelity::Exact
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    fn with(frames: &[f32]) -> WaveTraveller {
        let mut plugin = WaveTraveller {
            path: "x.wav".into(),
            ..WaveTraveller::default()
        };
        plugin.set_sample(
            "x.wav",
            Some(SampleFile {
                left: frames,
                right: None,
                rate: 44_100,
            }),
        );
        plugin
    }

    #[test]
    fn a_straight_line_is_drawn_point_for_point_and_shapes_hold_to_their_ends() {
        let mut table = [0.0; TABLE + 1];
        let line = Patch::default().position;
        assert_eq!(draw(&line, &mut table), 1022);
        assert_eq!(table[0], 0.0);
        assert!((table[511] - 0.5).abs() < 1e-5);
        assert!((table[1022] - 1.0).abs() < 1e-5);
        // Half a cosine from 1 down to 0 and back up in a straight line.
        let mut down = Point::new(0.0, 1.0);
        down.shape = 2;
        let points = [down, Point::new(0.5, 0.0), Point::new(1.0, 1.0)];
        let count = draw(&points, &mut table);
        assert_eq!(count, 1022);
        assert!((table[0] - 1.0).abs() < 1e-6);
        assert!((table[256] - 0.5).abs() < 0.01, "{}", table[256]);
        assert!(table[511] < 0.001);
        assert!(table.iter().all(|v| (0.0..=1.0).contains(v)));
        // Silence draws zeros, and a shape FL does not draw leaves no
        // points.
        let mut quiet = Point::new(0.0, 1.0);
        quiet.shape = 8;
        assert_eq!(draw(&[quiet, Point::new(1.0, 1.0)], &mut table), 1022);
        assert_eq!(table[100], 0.0);
        quiet.shape = 0;
        assert_eq!(draw(&[quiet, Point::new(1.0, 1.0)], &mut table), 0);
    }

    #[test]
    fn the_new_patch_plays_the_sample_through_once_in_four_beats() {
        // A whole sample of a ramp: the note's place goes from its start to
        // its end in four beats at 120, two seconds.
        let ramp: Vec<f32> = (0..44_100).map(|k| k as f32 / 44_100.0 * 0.5).collect();
        let mut plugin = with(&ramp);
        plugin.set_tempo(120.0);
        plugin.note_on(NoteOn::default());
        let voice = plugin.voices[0];
        assert!((f64::from(voice.speed) - 1021.0 / 88_200.0).abs() < 1e-9);
        let (mut left, mut right) = (vec![0.0; 100_000], vec![0.0; 100_000]);
        plugin.process(&mut left, &mut right);
        // The DC blocker passes a ramp's steps; the sound is there and is
        // over before 2 s and 256 samples.
        assert!(left[1000].abs() > 0.0 && left.iter().all(|v| v.is_finite()));
        assert!(left[88_200 + 300..].iter().all(|v| *v == 0.0));
        assert!(plugin.voices.is_empty());
        // A note off fades it out over its release, 1 ms.
        let mut plugin = with(&ramp);
        plugin.note_on(NoteOn::default());
        plugin.process(&mut left[..1000], &mut right[..1000]);
        plugin.note_off(0);
        plugin.process(&mut left[..100], &mut right[..100]);
        assert!(plugin.voices.is_empty());
    }

    #[test]
    fn the_reader_passes_the_sample_s_own_frames_and_ends_in_zeros() {
        let frames: Vec<f32> = (0..16).map(|k| (k as f32 * 0.7).sin()).collect();
        let plugin = with(&frames);
        for (k, frame) in frames.iter().enumerate().take(12).skip(2) {
            let held = (f64::from(*frame) * 32_768.0).round() / 32_768.0;
            assert!((f64::from(plugin.read(k as f64, 0)) - held).abs() < 1e-7);
        }
        assert_eq!(plugin.read(15.5, 1), plugin.read(15.5, 0));
    }

    #[test]
    fn every_preset_and_every_state_in_fl_s_projects_loads() {
        let Some(root) = testing::corpus() else {
            return;
        };
        // An install that also has `Patches/Plugin database` has one more.
        let mut saved = testing::every_preset(&root, NAME);
        assert!((10..=11).contains(&saved.len()), "{}", saved.len());
        let ours = |saved: &testing::Saved| saved.plugin.eq_ignore_ascii_case(NAME);
        let projects = testing::projects(&root).into_iter().filter(ours);
        saved.extend(projects);
        for saved in saved {
            let mut plugin = WaveTraveller::default();
            plugin.set_state(&saved.state).unwrap();
            // The loader reads every version alike; the presets have 1000001.
            assert!(
                [1_000_001, 1_000_005].contains(&plugin.version),
                "{}",
                saved.file
            );
            let frames: Vec<f32> = (0..20_000).map(|k| (k as f32 * 0.03).sin() * 0.5).collect();
            plugin.set_sample(
                &plugin.path.clone(),
                Some(SampleFile {
                    left: &frames,
                    right: None,
                    rate: 44_100,
                }),
            );
            for key in [48, 60, 62, 64, 65, 67] {
                plugin.note_on(NoteOn {
                    key,
                    voice: key as u32,
                    ..NoteOn::default()
                });
            }
            let (mut left, mut right) = (vec![0.0; 44_100], vec![0.0; 44_100]);
            plugin.process(&mut left, &mut right);
            let peak = left
                .iter()
                .chain(&right)
                .fold(0.0f32, |p, v| p.max(v.abs()));
            if !plugin.path.is_empty() {
                assert!((0.01..8.0).contains(&peak), "{peak} in {}", saved.file);
            }
        }
    }
}
