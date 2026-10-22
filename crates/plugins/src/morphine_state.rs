//! Morphine's state: the chunk tree its engine writes, as
//! `docs/plugins/morphine.md` maps it out of `Morphine_x64.dll`.
//!
//! Every chunk is a four-letter tag, which the file holds backwards
//! ("0roM" for `Mor0`), and the fields of the object it fills. A child
//! chunk has its length before it; the fields an object adds after its
//! children are a block that starts with its own length, which counts
//! itself, and a block longer than the engine knows is cut to what it
//! knows (`FUN_180130440` copies at most that much).
//!
//! ```text
//! "SSLF" u32 len | "0roM" | "0RPM" (FUN_18010f060)
//!   4 x (u32 len, "0CVM" generator)      FUN_1801135b0
//!   u32 len, "0LMM" morph line           FUN_180102450
//!   u32 len, "0DMM" envelopes, matrix    FUN_1800fc8d0
//!   block 0x5c4: the program's parameters, noise file and name
//! "0CVM": u32 len, "0HZM" zones, then block 0x1c                FUN_1801135b0
//! "0HZM": u32 n, n x (i32 keys, f32 level, u32 len, "0PSM")     FUN_18012afa0
//! "0PSM": u32 n, n x (i32 duration, f32 gain, u32 len, "0PBM"),
//!         then block 0x424                                      FUN_1800ec5d0
//! "0PBM": three arrays of 128 f32 (amplitude, pan, frequency), each sent
//!         eight at a time behind a byte whose bit i says that value i is
//!         there; a missing one is 0                             FUN_1800f0390
//! "0LMM": u32 n, u32 stride, n x (f32 x, f32 y, i32 duration), block 0x54
//! "0DMM": u32 n (4), n x (u32 len, "0GEM"), block 0x94: twelve sources,
//!         twelve destinations, twelve amounts                    FUN_1800fc8d0
//! "0GEM": u32 n, u32 stride, n x (f32 value, i32 duration), block 0x2c
//!                                                                FUN_1800f46e0
//! ```

use crate::StateError;

/// Partials in a spectrum.
pub const PARTIALS: usize = 128;
/// Parameters the engine has: FL's 141 and four it keeps for itself.
pub const PARAMS: usize = 0x91;

/// The bytes of a state, read from the front, each read checked.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Reader { bytes, at: 0 }
    }

    fn left(&self) -> usize {
        self.bytes.len() - self.at
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], StateError> {
        let end = self.at.saturating_add(len);
        let Some(taken) = self.bytes.get(self.at..end) else {
            return Err(StateError::Length {
                expected: end,
                found: self.bytes.len(),
            });
        };
        self.at = end;
        Ok(taken)
    }

    fn u32(&mut self) -> Result<u32, StateError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn i32(&mut self) -> Result<i32, StateError> {
        Ok(self.u32()? as i32)
    }

    fn f32(&mut self) -> Result<f32, StateError> {
        Ok(f32::from_bits(self.u32()?))
    }

    /// A tag, which must be this one.
    fn tag(&mut self, tag: &[u8; 4]) -> Result<(), StateError> {
        let found = self.take(4)?;
        if found == tag {
            Ok(())
        } else {
            Err(StateError::Version {
                found: i32::from_le_bytes([found[0], found[1], found[2], found[3]]),
            })
        }
    }

    /// A child: its length and as many bytes.
    fn child(&mut self) -> Result<Reader<'a>, StateError> {
        let len = self.u32()? as usize;
        Ok(Reader::new(self.take(len)?))
    }

    /// The block of fields an object ends with: its length, which counts
    /// itself, and the bytes, of which the engine reads at most `most`.
    /// What is not there reads as 0. None where the object has no block.
    fn block(&mut self, most: usize) -> Result<Option<Vec<u8>>, StateError> {
        if self.left() < 4 {
            return Ok(None);
        }
        let len = self.u32()? as usize;
        if len < 4 || len - 4 > self.left() {
            return Ok(None);
        }
        let taken = self.take(len - 4)?;
        let mut block = vec![0; most.saturating_sub(4)];
        let keep = taken.len().min(block.len());
        block[..keep].copy_from_slice(&taken[..keep]);
        Ok(Some(block))
    }
}

/// A word of a block: `at` counts from the block's length, as the engine's
/// offsets do.
fn word(block: &[u8], at: usize) -> u32 {
    block
        .get(at - 4..at)
        .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn float(block: &[u8], at: usize) -> f32 {
    f32::from_bits(word(block, at))
}

fn int(block: &[u8], at: usize) -> i32 {
    word(block, at) as i32
}

/// A value the engine keeps between `-1e-9` and `1e-9` as 0
/// (`FUN_1800ef5c0`).
fn tidy(v: f32) -> f32 {
    if v < 1e-9 && v > -1e-9 { 0.0 } else { v }
}

/// One analysed moment of a sound.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// How long it lasts, in hundredths of a second at rate 1. At least 1.
    pub duration: i32,
    /// Its gain, from 0 to 1, on the cube root of the amplitudes.
    pub gain: f32,
    /// The cube root of each partial's amplitude.
    pub amp: [f32; PARTIALS],
    /// Each partial's pan, from -1 to 1.
    pub pan: [f32; PARTIALS],
    /// How far each partial is from its harmonic, in harmonics.
    pub freq: [f32; PARTIALS],
}

/// A run of frames: one zone's analysis, and how it plays.
#[derive(Debug, Clone, PartialEq)]
pub struct Frames {
    pub frames: Vec<Frame>,
    /// The loop, in hundredths of a second from the first frame.
    pub loop_start: i32,
    pub loop_end: i32,
    /// Where the generator's parameters live in the engine. The port reads
    /// them from the first zone's, as FL's controls show them.
    pub release_loop: bool,
    pub loop_mode: i32,
    pub rate: f32,
    pub key_track: f32,
    pub vel_track: f32,
    pub sync: bool,
    /// How much velocity takes off each partial, and its gain.
    pub vel_sense: [f32; PARTIALS],
    pub partial_gain: [f32; PARTIALS],
}

/// A key range of a generator.
#[derive(Debug, Clone, PartialEq)]
pub struct Zone {
    /// How many keys it covers, from where the last one ended. At least 1.
    pub keys: i32,
    /// Its level, from 0 to 1.
    pub level: f32,
    pub frames: Frames,
}

/// One of the four generators.
#[derive(Debug, Clone, PartialEq)]
pub struct Gen {
    pub zones: Vec<Zone>,
}

/// A point of the morph line: where on the square, and how long the way
/// to the next point is in hundredths of a second.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MorphPoint {
    pub x: f32,
    pub y: f32,
    pub duration: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MorphLine {
    pub points: Vec<MorphPoint>,
    pub loop_start: i32,
    pub loop_end: i32,
    /// 1 to 4 plays generator A to D alone (+0x80); anything else morphs.
    pub solo: i32,
}

/// A point of an envelope: its value and the time to the next one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EgPoint {
    pub value: f32,
    pub duration: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Eg {
    pub points: Vec<EgPoint>,
    pub loop_start: i32,
    pub loop_end: i32,
    /// Its value runs from -1 to 1 and not from 0 to 1.
    pub bipolar: bool,
}

/// One slot of the modulation matrix: a source, a destination and an
/// amount from 0 to 1, where 0.5 is none.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Route {
    pub source: i32,
    pub dest: i32,
    pub amount: f32,
}

/// Everything a state holds.
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub gens: Vec<Gen>,
    pub morph: MorphLine,
    pub egs: Vec<Eg>,
    pub routes: [Route; 12],
    /// The parameters as FL numbers them, from 0 to 1 but for the
    /// integers the engine keeps as floats (polyphony, the modes).
    pub params: [f32; PARAMS],
    /// The noise sample's file name in `Noise Samples`, or empty.
    pub noise: String,
    pub name: String,
}

/// Where a parameter of the program's block lives in it: the engine's
/// field (`FUN_18010dc50`) less 0x604. None for the ones the block of
/// another chunk holds.
fn program_offset(index: usize) -> Option<usize> {
    let field = match index {
        0 => 0x608,
        1 => 0x60c,
        2 => 0x618,
        3 => 0x628,
        4 => 0x61c,
        5 => 0x620,
        6 => 0x624,
        7..=29 => 0x63c + (index - 7) * 4,
        30 => 0x62c,
        31 => 0x630,
        32 => 0x634,
        33 => 0x638,
        34 => 0x698,
        35 => 0x69c,
        36 => 0x6a0,
        37 => 0x6c0,
        38 => 0x6a4,
        39 => 0x6a8,
        40 => 0x6b0,
        41 => 0x6b4,
        42 => 0x6b8,
        0x7c => 0x614,
        0x7d => 0x610,
        0x7e => 0x6ac,
        0x7f => 0x6bc,
        _ => return None,
    };
    // The block copies 0x608..0x6b0 in place and the five fields after it
    // from its end (FUN_18010eb40).
    Some(match field {
        0x6bc => 0x5b0,
        0x6b0 => 0x5b4,
        0x6b4 => 0x5b8,
        0x6b8 => 0x5bc,
        0x6c0 => 0x5c0,
        f => f - 0x604,
    })
}

/// The parameters a new engine starts with, where a state does not say.
pub fn default_params() -> [f32; PARAMS] {
    let mut p = [0.5; PARAMS];
    // Generators (FUN_1800ebab0, FUN_180113750): rates and tracking 0.5,
    // loop mode 3, PWM rate 0.5, detune and pan range 1.
    for g in 0..4 {
        let b = 0x2b + 10 * g;
        p[b] = 0.0;
        p[b + 1] = 0.0;
        p[b + 5] = 0.0;
        p[b + 7] = 0.0;
        p[b + 8] = 1.0;
        p[b + 9] = 1.0;
        p[0x81 + 2 * g] = 3.0;
        p[0x82 + 2 * g] = 0.0;
    }
    p[0x53] = 0.0;
    for g in 0..4 {
        p[0x57 + 3 * g] = 1.0;
        p[0x59 + 3 * g] = 0.0;
    }
    p[0x80] = 3.0;
    for e in 0..4 {
        let b = 0x63 + 6 * e;
        p[b] = 0.0;
        p[b + 1] = 0.0;
        p[b + 5] = 0.0;
        p[0x8c - e] = 3.0;
    }
    p[0x7c] = 15.0;
    p[0x7d] = 0.0;
    p[0x7e] = 0.0;
    p[0x7f] = 0.0;
    p
}

fn read_bank(mut r: Reader<'_>) -> Result<Frame, StateError> {
    r.tag(b"0PBM")?;
    let mut arrays = [[0.0; PARTIALS]; 3];
    'all: for (n, array) in arrays.iter_mut().enumerate() {
        let mut i = 0;
        // The last array runs to the end of the chunk; the first two to
        // their 128th value.
        while i < PARTIALS {
            if r.left() == 0 {
                if n == 2 {
                    break 'all;
                }
                return Err(StateError::Length {
                    expected: r.at + 1,
                    found: r.bytes.len(),
                });
            }
            let mask = r.take(1)?[0];
            for bit in 0..8 {
                if mask >> bit & 1 != 0 && i < PARTIALS {
                    array[i] = tidy(r.f32()?);
                }
                i += 1;
            }
        }
    }
    let [amp, pan, freq] = arrays;
    Ok(Frame {
        duration: 1,
        gain: 1.0,
        amp,
        pan,
        freq,
    })
}

fn read_frames(mut r: Reader<'_>) -> Result<Frames, StateError> {
    r.tag(b"0PSM")?;
    let count = r.u32()? as usize;
    if count > r.left() / 12 {
        return Err(StateError::Length {
            expected: r.at + count * 12,
            found: r.bytes.len(),
        });
    }
    let mut frames = Vec::with_capacity(count);
    for _ in 0..count {
        let duration = r.i32()?.max(1);
        let gain = r.f32()?.clamp(0.0, 1.0);
        let mut frame = read_bank(r.child()?)?;
        frame.duration = duration;
        frame.gain = gain;
        frames.push(frame);
    }
    let mut set = Frames {
        frames,
        loop_start: 100,
        loop_end: 110,
        release_loop: false,
        loop_mode: 3,
        rate: 0.5,
        key_track: 0.5,
        vel_track: 0.5,
        sync: false,
        vel_sense: [1.0; PARTIALS],
        partial_gain: [1.0; PARTIALS],
    };
    if let Some(b) = r.block(0x424)? {
        set.loop_start = int(&b, 4);
        set.loop_end = int(&b, 8);
        set.release_loop = word(&b, 12) != 0;
        set.loop_mode = int(&b, 16);
        set.rate = float(&b, 20);
        set.key_track = float(&b, 24);
        set.vel_track = float(&b, 28);
        set.sync = word(&b, 32) != 0;
        for k in 0..PARTIALS {
            set.vel_sense[k] = float(&b, 36 + 4 * k);
            set.partial_gain[k] = float(&b, 36 + 4 * PARTIALS + 4 * k);
        }
    }
    if set.frames.is_empty() {
        // The engine makes a frame of silence for an empty set
        // (FUN_1800eacd0).
        set.frames.push(Frame {
            duration: 100,
            gain: 1.0,
            amp: [0.0; PARTIALS],
            pan: [0.0; PARTIALS],
            freq: [0.0; PARTIALS],
        });
    }
    Ok(set)
}

/// What one generator's block sets: PWM rate, mode, phase and free, and
/// the detune and pan ranges, at the parameters of generator `g`.
fn read_gen(mut r: Reader<'_>, g: usize, p: &mut [f32; PARAMS]) -> Result<Gen, StateError> {
    r.tag(b"0CVM")?;
    let mut zones_chunk = r.child()?;
    zones_chunk.tag(b"0HZM")?;
    let count = zones_chunk.u32()? as usize;
    if count > zones_chunk.left() / 12 {
        return Err(StateError::Length {
            expected: zones_chunk.at + count * 12,
            found: zones_chunk.bytes.len(),
        });
    }
    let mut zones = Vec::with_capacity(count);
    for _ in 0..count {
        let keys = zones_chunk.i32()?.max(1);
        let level = zones_chunk.f32()?.clamp(0.0, 1.0);
        let frames = read_frames(zones_chunk.child()?)?;
        zones.push(Zone {
            keys,
            level,
            frames,
        });
    }
    let b = 0x2b + 10 * g;
    if let Some(first) = zones.first() {
        let f = &first.frames;
        p[b] = f32::from(u8::from(f.release_loop));
        p[b + 1] = f32::from(u8::from(f.sync));
        p[b + 2] = f.rate;
        p[b + 3] = f.vel_track;
        p[b + 4] = f.key_track;
        p[0x81 + 2 * g] = f.loop_mode as f32;
    }
    if let Some(block) = r.block(0x1c)? {
        p[b + 6] = float(&block, 4);
        p[0x82 + 2 * g] = int(&block, 8) as f32 * 0.5;
        p[b + 7] = float(&block, 12);
        p[b + 5] = f32::from(u8::from(word(&block, 16) != 0));
        p[b + 8] = float(&block, 20);
        p[b + 9] = float(&block, 24);
    }
    Ok(Gen { zones })
}

fn read_morph(mut r: Reader<'_>, p: &mut [f32; PARAMS]) -> Result<MorphLine, StateError> {
    r.tag(b"0LMM")?;
    let count = r.u32()? as usize;
    let stride = r.u32()? as usize;
    if stride < 12 || count > r.left() / stride {
        return Err(StateError::Length {
            expected: r.at + count * stride.max(12),
            found: r.bytes.len(),
        });
    }
    let mut points = Vec::with_capacity(count);
    for _ in 0..count {
        let mut point = Reader::new(r.take(stride)?);
        let x = point.f32()?.clamp(0.0, 1.0);
        let y = point.f32()?.clamp(0.0, 1.0);
        let duration = point.i32()?.max(1);
        points.push(MorphPoint { x, y, duration });
    }
    let mut line = MorphLine {
        points,
        loop_start: 100,
        loop_end: 110,
        solo: 0,
    };
    if let Some(b) = r.block(0x54)? {
        line.loop_start = int(&b, 4);
        line.loop_end = int(&b, 8);
        p[0x80] = int(&b, 12) as f32;
        p[0x53] = f32::from(u8::from(word(&b, 16) != 0));
        for g in 0..4 {
            p[0x57 + 3 * g] = float(&b, 20 + 4 * g);
            p[0x58 + 3 * g] = float(&b, 36 + 4 * g);
            p[0x59 + 3 * g] = f32::from(u8::from(word(&b, 52 + 4 * g) != 0));
        }
        p[0x54] = float(&b, 68);
        p[0x55] = float(&b, 72);
        p[0x56] = float(&b, 76);
        line.solo = int(&b, 80);
    }
    if line.points.is_empty() {
        line.points.push(MorphPoint {
            x: 0.0,
            y: 1.0,
            duration: 100,
        });
    }
    Ok(line)
}

fn read_eg(mut r: Reader<'_>, e: usize, p: &mut [f32; PARAMS]) -> Result<Eg, StateError> {
    r.tag(b"0GEM")?;
    let count = r.u32()? as usize;
    let stride = r.u32()? as usize;
    if stride < 8 || count > r.left() / stride {
        return Err(StateError::Length {
            expected: r.at + count * stride.max(8),
            found: r.bytes.len(),
        });
    }
    let mut points = Vec::with_capacity(count);
    for _ in 0..count {
        let mut point = Reader::new(r.take(stride)?);
        let value = point.f32()?.clamp(0.0, 1.0);
        let duration = point.i32()?.max(1);
        points.push(EgPoint { value, duration });
    }
    let mut eg = Eg {
        points,
        loop_start: 100,
        loop_end: 110,
        bipolar: false,
    };
    if let Some(b) = r.block(0x2c)? {
        let base = 0x63 + 6 * e;
        eg.loop_start = int(&b, 4);
        eg.loop_end = int(&b, 8);
        p[base + 5] = float(&b, 12);
        p[base] = f32::from(u8::from(word(&b, 16) != 0));
        p[0x8c - e] = int(&b, 20) as f32;
        p[base + 2] = float(&b, 24);
        p[base + 1] = f32::from(u8::from(word(&b, 28) != 0));
        p[base + 4] = float(&b, 32);
        p[base + 3] = float(&b, 36);
        eg.bipolar = word(&b, 40) != 0;
    }
    if eg.points.is_empty() {
        eg.points.push(EgPoint {
            value: 1.0,
            duration: 100,
        });
    }
    Ok(eg)
}

/// Read the engine's chunk: "0roM" and what follows, as a `.mrp` preset
/// file holds it.
pub fn read_chunk(bytes: &[u8]) -> Result<Program, StateError> {
    let mut r = Reader::new(bytes);
    r.tag(b"0roM")?;
    r.tag(b"0RPM")?;
    let mut params = default_params();
    let mut gens = Vec::with_capacity(4);
    for g in 0..4 {
        gens.push(read_gen(r.child()?, g, &mut params)?);
    }
    let morph = read_morph(r.child()?, &mut params)?;
    let mut mods = r.child()?;
    mods.tag(b"0DMM")?;
    let count = mods.u32()? as usize;
    let mut egs = Vec::with_capacity(4);
    for e in 0..count {
        let eg = read_eg(mods.child()?, e.min(3), &mut params)?;
        if e < 4 {
            egs.push(eg);
        }
    }
    while egs.len() < 4 {
        egs.push(Eg {
            points: vec![EgPoint {
                value: 1.0,
                duration: 100,
            }],
            loop_start: 100,
            loop_end: 110,
            bipolar: false,
        });
    }
    let mut routes = [Route {
        source: 0,
        dest: 0,
        amount: 0.5,
    }; 12];
    if let Some(b) = mods.block(0x94)? {
        for (i, route) in routes.iter_mut().enumerate() {
            route.source = int(&b, 4 + 4 * i);
            route.dest = int(&b, 52 + 4 * i);
            route.amount = float(&b, 100 + 4 * i);
        }
    }
    let mut noise = String::new();
    let mut name = String::new();
    if let Some(b) = r.block(0x5c4)? {
        for (index, value) in params.iter_mut().enumerate() {
            if let Some(at) = program_offset(index) {
                *value = float(&b, at);
            }
        }
        let text = |from: usize, len: usize| {
            let raw = &b[from - 4..from - 4 + len];
            let end = raw.iter().position(|c| *c == 0).unwrap_or(len);
            raw[..end]
                .iter()
                .map(|c| char::from(*c))
                .collect::<String>()
        };
        noise = text(0xac, 0x400);
        name = text(0x4b0, 0x100);
    }
    for v in params.iter_mut() {
        if !v.is_finite() {
            *v = 0.0;
        }
    }
    Ok(Program {
        gens,
        morph,
        egs,
        routes,
        params,
        noise,
        name,
    })
}

/// Read event 213: "SSLF", the chunk's length and the chunk
/// (`FUN_1800c4090`).
pub fn read_state(bytes: &[u8]) -> Result<Program, StateError> {
    let mut r = Reader::new(bytes);
    r.tag(b"SSLF")?;
    let len = r.u32()? as usize;
    let chunk = r.take(len)?;
    read_chunk(chunk)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bank(arrays: &[[f32; PARTIALS]; 3]) -> Vec<u8> {
        let mut out = b"0PBM".to_vec();
        for array in arrays {
            for group in array.chunks(8) {
                let mask = group
                    .iter()
                    .enumerate()
                    .fold(0u8, |m, (i, v)| if *v != 0.0 { m | 1 << i } else { m });
                out.push(mask);
                for v in group.iter().filter(|v| **v != 0.0) {
                    out.extend(v.to_le_bytes());
                }
            }
        }
        out
    }

    #[test]
    fn a_bank_keeps_the_values_its_masks_send_and_zeroes_the_rest() {
        let mut arrays = [[0.0; PARTIALS]; 3];
        arrays[0][0] = 1.0;
        arrays[0][9] = 0.25;
        arrays[1][127] = -0.5;
        arrays[2][3] = 0.125;
        let bytes = bank(&arrays);
        // A mask a group, and a word for each value.
        assert_eq!(bytes.len(), 4 + 48 + 4 * 4);
        let frame = read_bank(Reader::new(&bytes)).unwrap();
        assert_eq!(frame.amp, arrays[0]);
        assert_eq!(frame.pan, arrays[1]);
        assert_eq!(frame.freq, arrays[2]);
    }

    #[test]
    fn a_tiny_value_is_kept_as_zero() {
        assert_eq!(tidy(5e-10), 0.0);
        assert_eq!(tidy(-5e-10), 0.0);
        assert_eq!(tidy(2e-9), 2e-9);
    }

    #[test]
    fn the_program_block_puts_each_parameter_where_the_engine_reads_it() {
        // Master level at 0x608 is the block's word 4, the noise filter
        // mode at 0x6bc its word 0x5b0.
        assert_eq!(program_offset(0), Some(4));
        assert_eq!(program_offset(0x7f), Some(0x5b0));
        assert_eq!(program_offset(37), Some(0x5c0));
        assert_eq!(program_offset(0x2b), None);
    }

    #[test]
    fn a_state_that_is_not_morphine_s_is_turned_down() {
        assert!(read_state(b"SSLF\x04\0\0\0Kik2").is_err());
        assert!(read_state(b"FPCh").is_err());
    }
}
