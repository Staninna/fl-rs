//! BeepMap: a picture played as a bank of sines, as
//! `docs/plugins/beepmap.md` reads it out of `BeepMap_x64.dll`.
//!
//! # The state
//!
//! An i32 version, 0 or 1. Version 0 is 60 bytes: the 7 parameters as
//! i32, 7 more that are not read, and the most rows a picture keeps; then
//! the length of a BMP file as a u32 and the file. Version 1 has the
//! instrument tab's 364 bytes after the 60; no shipped state has it. The
//! picture is in the state: no file is looked up for it. A new plugin, or a
//! state whose picture does not load, plays FL's `BeepMap/Images/Default.bmp`,
//! which [`Generator::set_resource_dir`] reads.
//!
//! | parameter | range | default | what it does |
//! |---|---|---|---|
//! | 0 Freq range | 24 to 9600 | 7200 | cents from the top row to the bottom |
//! | 1 Pixel duration | 0 to 4096 | 2048 | `round(256 (33^(v / 4096) - 1)) + 64` frames at 44.1 kHz a column |
//! | 2 Scale | 0 to 2 | 0 | log, linear, harmonics |
//! | 3 Use Blue | 0, 1 | 0 | blue narrows a row's pitch |
//! | 4 Grainy | 0, 1 | 0 | a sine of one turn a column on the whole note |
//! | 5 Loop | 0, 1 | 1 | start again from the left |
//! | 6 Widen | 0, 1 | 1 | each row's right side at a random phase of its own |
//!
//! # A note
//!
//! Each row of the picture is a sine from FL's host table. Its left
//! amplitude is the pixel's red, its right the green, both over `255
//! sqrt(rows)`, reached in a straight line by the end of the column. The
//! note plays the columns from the left and ends past the last unless it
//! loops. FL's instrument tab then puts the note through its Fast LP, here
//! at the note's mod X and Y, and the circular pan law of its pan and
//! velocity. A note off fades it out in 10 ms, as the tab does with its
//! volume envelope off, the only way a shipped state has it.
//!
//! What is not FL's:
//!
//! - FL draws the phases from Delphi's `Random`, whose seed it shares with
//!   everything else. Here each plugin starts from 0.
//! - A picture taller than the state's row limit is shrunk by averaging
//!   rows; FL's `StretchBlt` was not read. A 16-bit picture's 5-bit
//!   colours are widened as `c << 3 | c >> 2`. No project has either.
//! - A version 1 state's instrument tab is not built.
//! - [`VOICES`] notes at once; one more takes the oldest.

use crate::three_osc::{fast_lp, host_table, increment, pan_law};
use crate::{Fidelity, Generator, NoteOn, StateError};
use std::path::Path;

/// The name FL's projects give the plugin in event 201.
pub const NAME: &str = "BeepMap";

/// The parameters FL can move.
pub const PARAMS: usize = 7;

/// The notes it plays at once.
pub const VOICES: usize = 32;

/// The picture a new plugin plays, under the generators' directory.
pub const DEFAULT_IMAGE: &str = "BeepMap/Images/Default.bmp";

/// The range of each parameter's control.
const RANGES: [(i32, i32); PARAMS] = [
    (24, 9600),
    (0, 4096),
    (0, 2),
    (0, 1),
    (0, 1),
    (0, 1),
    (0, 1),
];

/// The controls' values in a new plugin (the form's).
pub const DEFAULTS: [i32; PARAMS] = [7200, 2048, 0, 0, 0, 1, 1];

/// The frames a block of rendering takes at most.
const CHUNK: usize = 256;

/// `ln 2 / 1200`, a cent as a power of e.
const CENT: f64 = std::f64::consts::LN_2 / 1200.0;

/// 0 cents, the note at key 60, in Hz.
const C5: f64 = 523.251_130_601_197;

/// A picture as the plugin reads it: rows from the top, each pixel blue,
/// green, red.
#[derive(Debug, Clone, PartialEq)]
pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<[u8; 3]>,
    /// Whether it took a reading FL's was not read for.
    approximate: bool,
}

impl Picture {
    /// A BMP file of 16, 24 or 32 bits a pixel, uncompressed. `None` for
    /// anything else.
    pub fn from_bmp(bytes: &[u8]) -> Option<Picture> {
        let u32_at = |at: usize| Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
        let u16_at = |at: usize| Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?));
        if bytes.get(..2)? != b"BM" {
            return None;
        }
        let offset = u32_at(10)? as usize;
        let width = u32_at(18)? as i32;
        let height = u32_at(22)? as i32;
        let bits = u16_at(28)?;
        let compression = u32_at(30)?;
        if width <= 0 || height == 0 || !(compression == 0 || compression == 3 && bits != 24) {
            return None;
        }
        let (width, rows) = (width as usize, height.unsigned_abs() as usize);
        let bytes_per = usize::from(bits / 8);
        if !matches!(bits, 16 | 24 | 32) {
            return None;
        }
        let stride = (width * bytes_per).div_ceil(4) * 4;
        let mut pixels = Vec::with_capacity(width * rows);
        for row in 0..rows {
            let from = if height > 0 { rows - 1 - row } else { row };
            let line =
                bytes.get(offset + from * stride..offset + from * stride + width * bytes_per)?;
            for px in line.chunks_exact(bytes_per) {
                pixels.push(match bits {
                    16 => {
                        let v = u16::from_le_bytes([px[0], px[1]]);
                        let five = |c: u16| ((c & 31) << 3 | (c & 31) >> 2) as u8;
                        [five(v), five(v >> 5), five(v >> 10)]
                    }
                    _ => [px[0], px[1], px[2]],
                });
            }
        }
        Some(Picture {
            width,
            height: rows,
            pixels,
            approximate: bits == 16,
        })
    }

    /// Shrunk to `rows` by averaging the rows each new one covers.
    fn shrunk(self, rows: usize) -> Picture {
        if rows == 0 || self.height <= rows {
            return self;
        }
        let mut pixels = Vec::with_capacity(self.width * rows);
        for row in 0..rows {
            let (from, to) = (
                row * self.height / rows,
                ((row + 1) * self.height / rows).max(row * self.height / rows + 1),
            );
            for x in 0..self.width {
                let mut sum = [0u32; 3];
                for y in from..to {
                    for (s, c) in sum.iter_mut().zip(self.pixels[y * self.width + x]) {
                        *s += u32::from(c);
                    }
                }
                pixels.push(sum.map(|s| (s / (to - from) as u32) as u8));
            }
        }
        Picture {
            width: self.width,
            height: rows,
            pixels,
            approximate: true,
        }
    }

    fn at(&self, row: usize, column: usize) -> [u8; 3] {
        self.pixels[row * self.width + column]
    }
}

/// One row's sine (0x30 bytes in FL's voice).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Row {
    phase: u32,
    /// What the right side's phase is ahead by.
    offset: u32,
    step: u32,
    /// The row's pitch in Hz, for the linear and harmonic scales.
    hz: i32,
    amp: [f32; 2],
    delta: [f32; 2],
    target: [f32; 2],
    /// 256, or 256 less the pixel's blue.
    width: i32,
    active: bool,
}

/// One note.
#[derive(Debug, Clone)]
struct Voice {
    id: Option<u32>,
    /// The note's cents from key 60, rounded, as `FinalLevels.Pitch`.
    cents: i32,
    gains: [f32; 2],
    /// The Fast LP's coefficients and its level and speed per side.
    f: f64,
    fb: f32,
    filtered: bool,
    lp: [[f64; 2]; 2],
    /// The pitch the rows were tuned for.
    tuned: Option<i32>,
    column: i32,
    pos: i64,
    next: i64,
    grain: u32,
    grain_step: u32,
    rows: Vec<Row>,
    fade: Option<usize>,
    age: u64,
}

impl Voice {
    fn new() -> Self {
        Voice {
            id: None,
            cents: 0,
            gains: [0.0; 2],
            f: 1.0,
            fb: 0.0,
            filtered: false,
            lp: [[0.0; 2]; 2],
            tuned: None,
            column: -1,
            pos: 0,
            next: 0,
            grain: 0,
            grain_step: 0,
            rows: Vec::new(),
            fade: None,
            age: 0,
        }
    }
}

/// What the rows read of the plugin.
struct Setup<'a> {
    params: &'a [i32; PARAMS],
    picture: &'a Picture,
    rate: u32,
    column_len: i64,
    inverse_len: f32,
    scale: f32,
}

impl Voice {
    /// `FUN_0072f7e0`: each row's step for the note's pitch.
    fn tune(&mut self, s: &Setup) {
        let rows = self.rows.len() as i32;
        let last = rows - 1;
        let range = s.params[0];
        let pitch = self.cents;
        let quarter = s.rate as i32 / 4;
        let base = |cents: i32| {
            let hz = ((f64::from(cents) * CENT).exp() * C5).round_ties_even() as i64;
            let hz = hz.min(i64::from(quarter)) as i32;
            let top = if hz != 0 { quarter / hz * hz } else { quarter };
            (hz, top)
        };
        let to_step = |hz: i32, top: i32| {
            let hz = hz.max(40).min(top) as u64;
            ((hz << 32) / u64::from(s.rate)) as u32
        };
        match s.params[2] {
            0 => {
                for (i, row) in self.rows.iter_mut().enumerate() {
                    let spread = (2 * i as i32 - last)
                        .wrapping_mul(range)
                        .wrapping_mul(row.width);
                    let cents = pitch - spread / (last * 512);
                    let cents = if cents < -4799 {
                        -4800
                    } else {
                        cents.min(4800)
                    };
                    row.step = increment(cents as f32, s.rate as f32);
                }
            }
            1 => {
                let (hz, top) = base(pitch);
                let half = range * 3 / 2;
                for (i, row) in self.rows.iter_mut().enumerate() {
                    let spread = (2 * i as i32 - last)
                        .wrapping_mul(half)
                        .wrapping_mul(row.width);
                    row.hz = hz - spread / (last * 512);
                    row.step = to_step(row.hz, top);
                }
            }
            _ => {
                let (hz, top) = base(pitch - 2400);
                for (i, row) in self.rows.iter_mut().enumerate() {
                    let times = 9 - row.width / 32;
                    row.hz = (rows - i as i32).wrapping_mul(hz).wrapping_mul(times);
                    row.step = to_step(row.hz, top);
                }
            }
        }
    }

    /// `FUN_0072fb20`: the targets of the next column.
    fn column(&mut self, s: &Setup) {
        let p = s.params;
        let fade_from = if p[2] == 2 {
            let x = (f64::from(p[0]) * 0.000_104_166_666_666_666_67) as f32;
            let grown = (f64::from(x) * 11f64.ln()).exp();
            (f64::from((grown - 1.0) as f32) * 1102.5).round_ties_even() as i32
        } else {
            i32::MAX
        };
        let width = s.picture.width as i32;
        let mut lit = self.rows.len() as i32;
        if width <= self.column {
            if p[5] > 0 {
                self.column = 0;
            } else {
                lit = -1;
            }
            if width == 0 {
                lit = -1;
            }
        }
        for (i, row) in self.rows.iter_mut().enumerate() {
            for amp in &mut row.amp {
                if amp.to_bits() & 0x7fff_ffff < 0x3380_0000 {
                    *amp = 0.0;
                }
            }
            row.width = 256;
            row.target = [0.0; 2];
            if (i as i32) < lit {
                let [b, g, r] = s.picture.at(i, self.column as usize);
                row.target = [f32::from(r) * s.scale, f32::from(g) * s.scale];
                if p[3] > 0 {
                    row.width = 256 - i32::from(b);
                }
            }
        }
        self.tune(s);
        for row in &mut self.rows {
            if fade_from < row.hz {
                let fade =
                    (1.0 - f64::from(row.hz - fade_from) * 0.002_083_333_333_333_333_3) as f32;
                let fade = fade.max(0.0);
                row.target = row.target.map(|t| t * fade);
            }
            for side in 0..2 {
                row.delta[side] = (row.target[side] - row.amp[side]) * s.inverse_len;
            }
            row.active = row.amp.iter().chain(&row.target).any(|v| v.to_bits() != 0);
        }
        self.grain_step = (0x1_0000_0000u64 / s.column_len as u64) as u32;
    }

    /// `FUN_0072fe80`: the next frames of the note before the tab. Whether
    /// it has played its last column comes back.
    fn render(&mut self, s: &Setup, out: &mut [[f32; 2]]) -> bool {
        let table = host_table(0);
        if self.tuned != Some(self.cents) {
            self.tuned = Some(self.cents);
            self.tune(s);
        }
        let end = self.pos + out.len() as i64;
        let mut at = 0;
        while self.pos < end {
            if self.pos == self.next {
                self.next += s.column_len;
                self.column += 1;
                self.column(s);
            }
            let to = self.next.min(end);
            let len = (to - self.pos) as usize;
            self.pos = to;
            let buf = &mut out[at..at + len];
            for (k, row) in self.rows.iter_mut().enumerate() {
                if k > 0 && !row.active {
                    row.phase = row.phase.wrapping_add(row.step.wrapping_mul(len as u32));
                    continue;
                }
                for frame in buf.iter_mut() {
                    let l = table[(row.phase >> 18) as usize] * row.amp[0];
                    let r = table[(row.phase.wrapping_add(row.offset) >> 18) as usize] * row.amp[1];
                    if k == 0 {
                        *frame = [l, r];
                    } else {
                        frame[0] += l;
                        frame[1] += r;
                    }
                    row.amp[0] += row.delta[0];
                    row.amp[1] += row.delta[1];
                    row.phase = row.phase.wrapping_add(row.step);
                }
            }
            if self.rows.is_empty() {
                buf.fill([0.0; 2]);
            }
            if s.params[4] > 0 {
                for frame in buf.iter_mut() {
                    let g = table[(self.grain >> 18) as usize];
                    *frame = frame.map(|v| v * g);
                    self.grain = self.grain.wrapping_add(self.grain_step);
                }
            }
            at += len;
        }
        (s.picture.width as i32) < self.column
    }
}

/// BeepMap. A new one is at 44.1 kHz with the form's values and no
/// picture until it is given a state or FL's default picture.
pub struct BeepMap {
    params: [i32; PARAMS],
    limit: i32,
    picture: Option<Picture>,
    from_state: bool,
    tab: bool,
    voices: Vec<Voice>,
    seed: u32,
    rate: u32,
    column_len: i64,
    inverse_len: f32,
    fade: Vec<f32>,
    scratch: Vec<[f32; 2]>,
    count: u64,
    crowded: bool,
}

impl Default for BeepMap {
    fn default() -> Self {
        host_table(0);
        let mut plugin = BeepMap {
            params: DEFAULTS,
            limit: 23,
            picture: None,
            from_state: false,
            tab: false,
            voices: (0..VOICES).map(|_| Voice::new()).collect(),
            seed: 0,
            rate: 44_100,
            column_len: 1,
            inverse_len: 1.0,
            fade: Vec::new(),
            scratch: vec![[0.0; 2]; CHUNK],
            count: 0,
            crowded: false,
        };
        plugin.set_sample_rate(44_100.0);
        plugin
    }
}

/// Delphi's `Random(range)`.
fn random(seed: &mut u32, range: u32) -> u32 {
    *seed = seed.wrapping_mul(0x0808_8405).wrapping_add(1);
    ((u64::from(*seed) * u64::from(range)) >> 32) as u32
}

impl BeepMap {
    /// The parameters as the controls hold them.
    pub fn params(&self) -> &[i32; PARAMS] {
        &self.params
    }

    /// The picture the notes play.
    pub fn picture(&self) -> Option<&Picture> {
        self.picture.as_ref()
    }

    /// Frames a column lasts at this rate (`ProcessParam` of parameter 1).
    fn time_column(&mut self) {
        let along = (f64::from(self.params[1]) / 4096.0) as f32;
        let grown = (f64::from(along) * 33f64.ln()).exp();
        let frames = (f64::from((grown - 1.0) as f32 * 256.0)).round_ties_even() as i64 + 64;
        self.column_len = (frames * i64::from(self.rate) / 44_100).max(1);
        self.inverse_len = (1.0 / self.column_len as f64) as f32;
    }

    /// `FUN_007300d0`: take a picture, shrunk to the row limit, and give
    /// every voice room for its rows.
    fn take(&mut self, picture: Picture) {
        let limit = if self.limit > 0 {
            self.limit as usize
        } else {
            picture.height
        };
        let picture = picture.shrunk(limit);
        for voice in &mut self.voices {
            voice.id = None;
            voice.rows = vec![Row::default(); picture.height];
        }
        self.picture = Some(picture);
    }

    fn setup(&self) -> Option<Setup<'_>> {
        let picture = self.picture.as_ref()?;
        let scale = (1.0 / ((picture.height as f64).sqrt() * 255.0)) as f32;
        Some(Setup {
            params: &self.params,
            picture,
            rate: self.rate,
            column_len: self.column_len,
            inverse_len: self.inverse_len,
            scale,
        })
    }
}

impl Generator for BeepMap {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let int = |at: usize| {
            bytes
                .get(at..at + 4)
                .map(|b| i32::from_le_bytes(b.try_into().unwrap()))
                .ok_or(StateError::Length {
                    expected: at + 4,
                    found: bytes.len(),
                })
        };
        let version = int(0)?;
        if !(0..=1).contains(&version) {
            return Err(StateError::Version { found: version });
        }
        let head = if version == 0 { 60 } else { 0x1a8 };
        let file_at = 4 + head + 4;
        let len = int(4 + head)? as u32 as usize;
        let file = bytes
            .get(file_at..file_at + len)
            .ok_or(StateError::Length {
                expected: file_at + len,
                found: bytes.len(),
            })?;
        for (k, param) in self.params.iter_mut().enumerate() {
            *param = int(4 + 4 * k)?;
        }
        self.limit = int(4 + 56)?;
        self.tab = version == 1;
        self.time_column();
        if let Some(picture) = Picture::from_bmp(file).filter(|p| p.height > 2) {
            self.from_state = true;
            self.take(picture);
        }
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        if hz > 0.0 {
            self.rate = hz.round() as u32;
        }
        self.time_column();
        let len = ((f64::from(self.rate) * 0.01).round_ties_even() as usize).max(1);
        let step = (std::f64::consts::PI / (len + 1) as f64) as f32;
        self.fade = (0..len)
            .map(|n| (((n + 1) as f32 * step).cos() + 1.0) * 0.5)
            .collect();
        self.reset();
    }

    fn set_resource_dir(&mut self, dir: &Path) {
        if self.from_state {
            return;
        }
        let file = std::fs::read(dir.join(DEFAULT_IMAGE)).ok();
        if let Some(picture) = file.as_deref().and_then(Picture::from_bmp) {
            self.take(picture);
        }
    }

    fn note_on(&mut self, note: NoteOn) {
        if self.picture.is_none() {
            return;
        }
        let pick = match self.voices.iter().position(|v| v.id.is_none()) {
            Some(free) => free,
            None => {
                self.crowded = true;
                let oldest = self.voices.iter().enumerate().min_by_key(|(_, v)| v.age);
                oldest.map_or(0, |(k, _)| k)
            }
        };
        let widen = self.params[6] != 0;
        let s = 44_100.0 / self.rate as f32;
        let mut seed = self.seed;
        let voice = &mut self.voices[pick];
        for row in &mut voice.rows {
            *row = Row {
                phase: random(&mut seed, 0x100_0000) << 8,
                offset: if widen {
                    random(&mut seed, 0x100_0000) << 8
                } else {
                    0
                },
                ..Row::default()
            };
        }
        self.seed = seed;
        let cents = f64::from(note.key - 60) * 100.0 + f64::from(note.fine_pitch);
        let (f, fb) = fast_lp(1.0 + note.mod_x, note.mod_y, s);
        self.count += 1;
        *voice = Voice {
            id: Some(note.voice),
            cents: cents.round_ties_even() as i32,
            gains: pan_law(note.pan, note.velocity),
            f: f64::from(f),
            fb,
            filtered: f != 1.0 || fb != 0.0,
            age: self.count,
            rows: std::mem::take(&mut voice.rows),
            ..Voice::new()
        };
    }

    fn note_off(&mut self, voice: u32) {
        for v in &mut self.voices {
            if v.id == Some(voice) {
                v.fade.get_or_insert(0);
            }
        }
    }

    fn all_notes_off(&mut self) {
        for v in &mut self.voices {
            if v.id.is_some() {
                v.fade.get_or_insert(0);
            }
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let mut scratch = std::mem::take(&mut self.scratch);
        let fade = std::mem::take(&mut self.fade);
        let mut voices = std::mem::take(&mut self.voices);
        if let Some(setup) = self.setup() {
            for voice in voices.iter_mut().filter(|v| v.id.is_some()) {
                let mut at = 0;
                while at < left.len() && voice.id.is_some() {
                    let len = (left.len() - at).min(CHUNK);
                    let buf = &mut scratch[..len];
                    let done = voice.render(&setup, buf);
                    for (k, frame) in buf.iter_mut().enumerate() {
                        let mut frame = *frame;
                        if voice.filtered {
                            for (side, [level, speed]) in voice.lp.iter_mut().enumerate() {
                                *speed += (f64::from(frame[side]) - *level) * voice.f;
                                *level += *speed;
                                *speed *= f64::from(voice.fb);
                                frame[side] = *level as f32;
                            }
                        }
                        if let Some(n) = &mut voice.fade {
                            if *n >= fade.len() {
                                voice.id = None;
                                break;
                            }
                            frame = frame.map(|v| v * fade[*n]);
                            *n += 1;
                        }
                        left[at + k] += frame[0] * voice.gains[0];
                        right[at + k] += frame[1] * voice.gains[1];
                    }
                    if done {
                        voice.id = None;
                    }
                    at += len;
                }
            }
        }
        self.voices = voices;
        self.fade = fade;
        self.scratch = scratch;
    }

    fn param_count(&self) -> usize {
        PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |v| *v as f32)
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let Some(&(low, high)) = RANGES.get(index) else {
            return;
        };
        self.params[index] = (value.round() as i64).clamp(i64::from(low), i64::from(high)) as i32;
        match index {
            0 => {
                for voice in &mut self.voices {
                    voice.tuned = None;
                }
            }
            1 => self.time_column(),
            _ => {}
        }
    }

    fn reset(&mut self) {
        for voice in &mut self.voices {
            voice.id = None;
        }
    }

    fn tail(&self) -> usize {
        self.fade.len() + 1
    }

    fn fidelity(&self) -> Fidelity {
        match &self.picture {
            None => Fidelity::Approximate("BeepMap has no picture"),
            Some(p) if p.approximate => Fidelity::Approximate("BeepMap picture shrunk or 16-bit"),
            Some(_) if self.tab => Fidelity::Approximate("BeepMap instrument tab not built"),
            Some(_) if self.crowded => Fidelity::Approximate("BeepMap ran out of voices"),
            Some(_) => Fidelity::Exact,
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
            velocity: 1.0,
            pan: 0.0,
            fine_pitch: 0.0,
            release: 0.5,
            mod_x: 0.0,
            mod_y: 0.0,
            color: 0,
            slide: false,
            porta: false,
        }
    }

    /// A 32-bit top-down BMP of `width` by `rows`, each pixel from `at`.
    fn bmp(width: usize, rows: usize, at: impl Fn(usize, usize) -> [u8; 3]) -> Vec<u8> {
        let mut file = b"BM".to_vec();
        file.extend(((54 + 4 * width * rows) as u32).to_le_bytes());
        file.extend([0; 4]);
        file.extend(54u32.to_le_bytes());
        file.extend(40u32.to_le_bytes());
        file.extend((width as i32).to_le_bytes());
        file.extend((-(rows as i32)).to_le_bytes());
        file.extend(1u16.to_le_bytes());
        file.extend(32u16.to_le_bytes());
        file.extend([0; 24]);
        for y in 0..rows {
            for x in 0..width {
                let [b, g, r] = at(y, x);
                file.extend([b, g, r, 0]);
            }
        }
        file
    }

    fn state(params: [i32; PARAMS], limit: i32, picture: &[u8]) -> Vec<u8> {
        let mut bytes = 0i32.to_le_bytes().to_vec();
        for v in params.iter().chain(&[0; 7]).chain(&[limit]) {
            bytes.extend(v.to_le_bytes());
        }
        bytes.extend((picture.len() as u32).to_le_bytes());
        bytes.extend(picture);
        bytes
    }

    fn play(plugin: &mut BeepMap, frames: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
        for (l, r) in left.chunks_mut(512).zip(right.chunks_mut(512)) {
            plugin.process(l, r);
        }
        (left, right)
    }

    #[test]
    fn a_bmp_reads_from_the_top_whichever_way_it_is_stored() {
        let file = bmp(2, 3, |y, x| [y as u8, x as u8, 9]);
        let top_down = Picture::from_bmp(&file).unwrap();
        assert_eq!((top_down.width, top_down.height), (2, 3));
        assert_eq!(top_down.at(2, 1), [2, 1, 9]);
        // The same rows stored bottom up.
        let mut bottom_up = file[..54].to_vec();
        bottom_up[22..26].copy_from_slice(&3i32.to_le_bytes());
        for y in (0..3).rev() {
            bottom_up.extend(&file[54 + 8 * y..54 + 8 * y + 8]);
        }
        assert_eq!(Picture::from_bmp(&bottom_up).unwrap(), top_down);
        assert!(Picture::from_bmp(b"BMxx").is_none());
    }

    #[test]
    fn a_state_reads_its_parameters_and_picture() {
        let picture = bmp(4, 3, |_, _| [0, 0, 255]);
        let mut plugin = BeepMap::default();
        let params = [4800, 0, 1, 1, 0, 0, 0];
        plugin.set_state(&state(params, 23, &picture)).unwrap();
        assert_eq!(plugin.params(), &params);
        assert_eq!(plugin.picture().unwrap().width, 4);
        // Pixel duration 0 is 64 frames a column.
        assert_eq!(plugin.column_len, 64);
        plugin.set_param(1, 2048.0);
        assert_eq!(plugin.column_len, 1279);
        plugin.set_param(0, 1e9);
        assert_eq!(plugin.param(0), 9600.0);
        assert_eq!(plugin.param(PARAMS), 0.0);
        assert!(plugin.set_state(&[2, 0, 0, 0]).is_err());
        assert!(
            plugin
                .set_state(&state(params, 23, &picture)[..80])
                .is_err()
        );
        // A picture of two rows is not taken.
        plugin
            .set_state(&state(params, 23, &bmp(4, 2, |_, _| [0; 3])))
            .unwrap();
        assert_eq!(plugin.picture().unwrap().height, 3);
    }

    #[test]
    fn a_lit_row_sounds_at_its_place_in_the_range_and_red_and_green_are_the_sides() {
        // Five rows over 2400 cents, the middle one lit: key 60 itself.
        let picture = bmp(8, 5, |y, _| if y == 2 { [0, 128, 255] } else { [0; 3] });
        let mut plugin = BeepMap::default();
        let params = [2400, 4096, 0, 0, 0, 0, 0];
        plugin.set_state(&state(params, 23, &picture)).unwrap();
        plugin.note_on(note(1, 60));
        let (left, right) = play(&mut plugin, 44_100);
        let tail = &left[20_000..40_000];
        let at = testing::partial(tail, C5 / 44_100.0);
        let off = testing::partial(tail, C5 * 1.2 / 44_100.0);
        assert!(at > 20.0 * off && at > 0.05, "{at} {off}");
        let ratio = testing::partial(&right[20_000..40_000], C5 / 44_100.0) / at;
        assert!((ratio - 128.0 / 255.0).abs() < 0.01, "{ratio}");
        // The top row is half the range up.
        let picture = bmp(8, 5, |y, _| if y == 0 { [0, 255, 255] } else { [0; 3] });
        plugin.set_state(&state(params, 23, &picture)).unwrap();
        plugin.note_on(note(2, 60));
        let (left, _) = play(&mut plugin, 44_100);
        let up = testing::partial(&left[20_000..40_000], C5 * 2.0 / 44_100.0);
        assert!(up > 0.05, "{up}");
    }

    #[test]
    fn a_note_ends_past_its_last_column_unless_it_loops_and_a_note_off_fades_it() {
        let picture = bmp(3, 3, |_, _| [0, 255, 255]);
        let mut plugin = BeepMap::default();
        plugin
            .set_state(&state([2400, 0, 0, 0, 0, 0, 0], 23, &picture))
            .unwrap();
        plugin.note_on(note(1, 60));
        play(&mut plugin, 64 * 5 + 1);
        assert!(plugin.voices.iter().all(|v| v.id.is_none()));
        plugin.set_param(5, 1.0);
        plugin.note_on(note(2, 60));
        play(&mut plugin, 64 * 50);
        assert!(plugin.voices.iter().any(|v| v.id.is_some()));
        plugin.note_off(2);
        let (left, _) = play(&mut plugin, 1024);
        assert!(plugin.voices.iter().all(|v| v.id.is_none()));
        assert!(left[441..].iter().all(|v| *v == 0.0));
    }

    #[test]
    fn every_state_fl_ships_loads_and_plays() {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, BeepMap's states skipped");
            return;
        };
        let dir = root.join("../Plugins/Fruity/Generators");
        let mut fresh = BeepMap::default();
        fresh.set_resource_dir(&dir);
        assert_eq!(fresh.picture().unwrap().height, 23);
        let mut saved = testing::every_preset(&root, NAME);
        let presets = saved.len();
        saved.extend(
            testing::projects(&root)
                .into_iter()
                .filter(|saved| saved.plugin.eq_ignore_ascii_case(NAME)),
        );
        for saved in &saved {
            let mut plugin = crate::generator_by_name(&saved.plugin).unwrap();
            plugin.set_resource_dir(&dir);
            if let Err(error) = plugin.set_state(&saved.state) {
                panic!("{}: {error}", saved.file);
            }
            plugin.note_on(note(1, 60));
            let (mut left, mut right) = (vec![0.0; 44_100], vec![0.0; 44_100]);
            for (l, r) in left.chunks_mut(512).zip(right.chunks_mut(512)) {
                plugin.process(l, r);
            }
            assert!(
                left.iter()
                    .chain(&right)
                    .all(|v| v.is_finite() && v.abs() < 4.0),
                "{}",
                saved.file
            );
            let peak = left.iter().fold(0f32, |m, v| m.max(v.abs()));
            assert!(peak > 0.0, "{}", saved.file);
            eprintln!("{}: peak {peak}", saved.file);
        }
        eprintln!("{presets} presets, {} in projects", saved.len() - presets);
        assert!(saved.len() > presets);
    }
}
