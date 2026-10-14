//! Harmor's event 213: the layout of `docs/plugins/harmor.md` section 1, as
//! `SaveRestoreState` (`FUN_0086f2f0`) and the part reader
//! (`FUN_0086ed30`) read it.
//!
//! ```text
//! u32 version (0x00130003 now: 19 in the high word); u32 mask (1 part A, 2 part B, 4 globals)
//! per part: u32 size; 386 x i32 (385 before 16); 12 x i32 unit order; the part struct (0x4b bytes, 0x47 before 11)
//!           43 articulators (32 before 16: articulator 33 is new), 26 shapes, the image, an ansi file name
//! globals:  53 x i32 (REC 772..824); 6 x i32 FX order; 0x3d bytes; the info text; from 19 on a flag byte
//! ```

use crate::StateError;
use crate::menvelope::{Class, ENVELOPE_KNOBS, LFO_KNOBS, MEnvelope, Reader};
use dsp::curve::Point;

/// Part parameters: 95 direct ones, 248 articulator knob slots and 43
/// articulator smoothing values.
pub const PART_PARAMS: usize = 386;
/// The global parameters, REC 772..824.
pub const GLOBAL_PARAMS: usize = 53;
/// Every parameter FL numbers: two parts and the globals.
pub const PARAMS: usize = 2 * PART_PARAMS + GLOBAL_PARAMS;
/// Articulators a part has, and shapes.
pub const ARTICULATORS: usize = 43;
pub const SHAPES: usize = 26;
/// The newest version FL writes.
const NEWEST: u32 = 0x0013_0003;

/// The range and reset value of the 95 direct part parameters, from the
/// editor form (`docs/plugins/harmor.md` section 2.1).
pub const PART_RANGES: [(i32, i32, i32); 95] = [
    (0, 999, 500),
    (-1000, 1000, 0),
    (-384, 384, 0),
    (0, 1000, 1000),
    (0, 1000, 0),
    (0, 2048, 0),
    (0, 2048, 0),
    (0, 2048, 0),
    (0, 1024, 256),
    (0, 1024, 1024),
    // 10
    (-1000, 1000, 0),
    (-2000, 2000, 1000),
    (0, 2000, 1000),
    (0, 1000, 0),
    (0, 1, 0),
    (0, 1, 0),
    (0, 1, 0),
    (0, 1000, 0),
    (0, 10000, 4000),
    (0, 999, 500),
    // 20
    (0, 1000, 1000),
    (0, 1600, 0),
    (0, 1600, 0),
    (0, 10000, 0),
    (0, 1000, 500),
    (0, 1000, 500),
    (-1000, 1000, 0),
    (0, 1000, 0),
    (0, 1000, 500),
    (0, 1024, 256),
    // 30
    (-2, 7, 0),
    (-7, 8, 0),
    (-1, 8, 0),
    (-7, 8, 0),
    (1, 9, 1),
    (0, 4, 1),
    (0, 1, 0),
    (0, 256, 192),
    (0, 256, 128),
    (-1000, 1000, 500),
    // 40
    (10000, 160000, 10000),
    (1, 9, 1),
    (10000, 160000, 10000),
    (1, 9, 1),
    (-6000, 6000, 6000),
    (0, 1000, 0),
    (0, 1000, 500),
    (0, 1, 1),
    (0, 1, 0),
    (-384, 384, -140),
    // 50
    (0, 1200, 1200),
    (0, 1, 0),
    (0, 8192, 8192),
    (0, 1, 0),
    (0, 1000, 500),
    (-2000, 2000, 1000),
    (-1000, 1000, 0),
    (0, 1000, 0),
    (0, 1, 0),
    (0, 1000, 500),
    // 60
    (-2400, 2400, 0),
    (0, 1, 0),
    (0, 2048, 0),
    (0, 1, 0),
    (0, 8192, 8192),
    (0, 1, 0),
    (0, 1000, 500),
    (-2000, 2000, 1000),
    (-1000, 1000, 0),
    (0, 1000, 0),
    // 70
    (0, 1, 0),
    (0, 1000, 500),
    (-2400, 2400, 0),
    (0, 1, 0),
    (-1000, 1000, 0),
    (0, 1000, 1000),
    (0, 1000, 0),
    (0, 2, 0),
    (0, 10000, 5000),
    (0, 999, 500),
    // 80
    (-8000, 8000, 3000),
    (-2000, 2000, 1000),
    (-1000, 1000, 500),
    (0, 1_048_575, 0),
    (-20000, 20000, 10000),
    (-60000, 60000, 0),
    (-1000, 1000, 750),
    (0, 6, 0),
    (0, 1000, 500),
    (0, 1000, 1000),
    // 90
    (0, 3, 0),
    (0, 6, 0),
    (-2000, 2000, 1000),
    (-600, 600, 0),
    (0, 1000, 1000),
];

/// The global parameters' reset values (REC 772..824).
pub const GLOBAL_DEFAULTS: [i32; GLOBAL_PARAMS] = {
    let mut g = [0; GLOBAL_PARAMS];
    g[0] = 256;
    g[1] = 256;
    g[2] = 256;
    g[10] = 500;
    g[11] = 1000;
    g[12] = 1000;
    g[13] = 1;
    g[15] = 1000;
    g
};

/// What `FUN_00896fe0` makes of each articulator: whether it adds (mode 1,
/// centre 0.5) and whether it multiplies by twice its value (mode 2).
pub const BIPOLAR: u64 = 0x7d2_7986_f035;
pub const DOUBLED: u64 = 0x6c0_5080_4004;

/// How an articulator puts its parts together (`TOpArticulator + 8`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 0: they multiply, around 0.
    Multiply,
    /// 1: they add, around 0.5.
    Add,
    /// 2: they multiply by twice their value, around 0.5.
    Double,
}

impl Mode {
    pub fn of(articulator: usize) -> Mode {
        let bit = 1u64 << articulator;
        if DOUBLED & bit != 0 {
            Mode::Double
        } else if BIPOLAR & bit != 0 {
            Mode::Add
        } else {
            Mode::Multiply
        }
    }

    /// The value the curves of such an articulator are counted from.
    pub fn centre(self) -> f64 {
        match self {
            Mode::Multiply => 0.0,
            _ => 0.5,
        }
    }
}

/// One `TOpArticulator` as the stream has it.
#[derive(Debug, Clone, PartialEq)]
pub struct ArtState {
    pub format: u32,
    pub levels: [f32; 3],
    pub envelope: MEnvelope,
    pub lfo: MEnvelope,
    /// Keyboard, velocity, mod X, mod Y, mod Z, random, unison index,
    /// held index and mod X speed.
    pub maps: Vec<MEnvelope>,
}

/// A curve of one level, switched off, of a class.
pub fn idle(kind: u32, level: f64, knobs: [i32; 4]) -> MEnvelope {
    MEnvelope {
        kind,
        version: 3,
        points: vec![Point::new(0.0, level), Point::new(1.0, level)],
        arp: vec![0, 0],
        flags: 0,
        active: false,
        decay: -1,
        loop_start: -1,
        loop_end: -1,
        knobs,
        phase: 0,
    }
}

impl ArtState {
    /// What an articulator is before anyone draws on it: every map flat
    /// at the value that changes nothing.
    pub fn new(index: usize) -> Self {
        let mode = Mode::of(index);
        let identity = match mode {
            Mode::Multiply | Mode::Double => 1.0,
            Mode::Add => 0.5,
        };
        ArtState {
            format: 3,
            levels: [1.0; 3],
            envelope: idle(1, identity, ENVELOPE_KNOBS),
            lfo: idle(2, 1.0, LFO_KNOBS),
            maps: (0..9).map(|_| idle(3, identity, ENVELOPE_KNOBS)).collect(),
        }
    }

    fn read(reader: &mut Reader<'_>) -> Result<Self, StateError> {
        let format = reader.u32()?;
        let levels = [reader.f32()?, reader.f32()?, reader.f32()?];
        let envelope = MEnvelope::read(reader, Class::Envelope)?;
        let lfo = MEnvelope::read(reader, Class::Lfo)?;
        let mut maps = Vec::with_capacity(9);
        for _ in 0..9 {
            maps.push(MEnvelope::read(reader, Class::Map)?);
        }
        Ok(ArtState {
            format,
            levels,
            envelope,
            lfo,
            maps,
        })
    }
}

/// The part's image (`TToneBMP`, `FUN_00793300`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Image {
    pub version: u32,
    /// 0 drawn or embedded, 1 resynthesised from the file the part names.
    pub kind: i32,
    pub loop_mode: i32,
    pub normalise: i32,
    pub loop_start: f64,
    pub loop_end: f64,
    pub crossfade: f64,
    pub columns: usize,
    pub rows: usize,
    /// Gain, frequency and phase planes: column after column of `rows`
    /// values, or empty.
    pub planes: [Vec<f32>; 3],
}

impl Image {
    fn read(reader: &mut Reader<'_>) -> Result<Self, StateError> {
        let version = reader.u32()?;
        let kind = reader.i32()?;
        let loop_mode = reader.i32()?;
        let normalise = reader.i32()?;
        let loop_start = reader.f64()?;
        let loop_end = reader.f64()?;
        let crossfade = reader.f64()?;
        let columns = reader.i32()?.max(0) as usize;
        let rows = reader.i32()?.max(0) as usize;
        let mut image = Image {
            version,
            kind,
            loop_mode,
            normalise,
            loop_start,
            loop_end,
            crossfade,
            columns,
            rows,
            planes: Default::default(),
        };
        if kind == 0 {
            for plane in &mut image.planes {
                if reader.i32()? > 0 {
                    let count = columns.saturating_mul(rows);
                    let bytes = reader.bytes(count.saturating_mul(4))?;
                    *plane = bytes
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|w| f32::from_le_bytes(*w))
                        .collect();
                }
            }
        }
        Ok(image)
    }

    /// Whether the image plays: an embedded one with a gain plane.
    pub fn plays(&self) -> bool {
        self.kind == 0 && !self.planes[0].is_empty() && self.columns > 0 && self.rows > 0
    }
}

/// One part, A or B.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub params: [i32; PART_PARAMS],
    /// The spectral unit order, a permutation of 0..11.
    pub order: [i32; 12],
    /// The part struct's ints at 0, 4, .. 48: blend, declick, ramp, prism
    /// mode, prism from volume, clip mode, sub mode, pitch distribution,
    /// resynthesis mode, denoise, phaser type, view scale and the global
    /// EQ mix, which is an f32 (see [`Part::global_eq_mix`]).
    pub ints: [i32; 13],
    /// Filter 1 and 2 types (0..18), then resonance types (100..111).
    pub filter_types: [i32; 2],
    pub resonance_types: [i32; 2],
    pub enabled: bool,
    pub mid_side: bool,
    pub pluck_to_blur: bool,
    pub auto_gain_mode: i32,
    pub arts: Vec<ArtState>,
    pub shapes: Vec<MEnvelope>,
    pub image: Image,
    pub file: String,
}

impl Part {
    /// A part set to the editor's reset values, a saw with no envelope.
    pub fn new(enabled: bool) -> Self {
        let mut params = [0; PART_PARAMS];
        for (k, (_, _, reset)) in PART_RANGES.iter().enumerate() {
            params[k] = *reset;
        }
        // Flat at 0.25, which is 0 dB: with the local EQ's 1/h that is a
        // saw.
        let mut shapes: Vec<MEnvelope> =
            (0..SHAPES).map(|_| idle(3, 0.25, ENVELOPE_KNOBS)).collect();
        // The centred shapes sit at their centre, the custom filter
        // shapes and the masks at 1.
        for s in [1, 4, 6, 7, 8, 9, 12, 13, 16] {
            shapes[s] = idle(3, 0.5, ENVELOPE_KNOBS);
        }
        for s in [10, 11, 18, 19, 20, 21, 22, 23, 24, 25] {
            shapes[s] = idle(3, 1.0, ENVELOPE_KNOBS);
        }
        Part {
            params,
            order: std::array::from_fn(|i| i as i32),
            ints: [0, 0, 1000, 0, 0, 0, 0, 1, 0, 250, 200, 0, 0x3f80_0000],
            filter_types: [0, 18],
            resonance_types: [100, 111],
            enabled,
            mid_side: false,
            pluck_to_blur: false,
            auto_gain_mode: 0,
            arts: (0..ARTICULATORS).map(ArtState::new).collect(),
            shapes,
            image: Image::default(),
            file: String::new(),
        }
    }

    /// `GlobalEQMixWheel` / 1000, which the struct keeps as an f32.
    pub fn global_eq_mix(&self) -> f32 {
        let g = f32::from_bits(self.ints[12] as u32);
        if g.is_finite() {
            g.clamp(0.0, 1.0)
        } else {
            1.0
        }
    }

    fn read(reader: &mut Reader<'_>, version: u32) -> Result<Self, StateError> {
        let size = reader.u32()? as usize;
        let start = reader.at;
        let mut part = Part::new(false);
        let count = if version < 16 { 385 } else { 386 };
        let mut params = Vec::with_capacity(PART_PARAMS);
        for _ in 0..count {
            params.push(reader.i32()?);
        }
        if version < 16 {
            params.insert(376, 0);
        }
        part.params.copy_from_slice(&params);
        if version < 12 {
            for v in &mut part.params[343..386] {
                // The old smoothing curve onto the new one.
                let old = (41f64.powf(f64::from(*v) / 1000.0) - 1.0) * 1.91625;
                *v = ((1.0 + old).log2() * 111.11).round_ties_even() as i32;
            }
        }
        for o in &mut part.order {
            *o = reader.i32()?;
        }
        for i in &mut part.ints {
            *i = reader.i32()?;
        }
        part.filter_types = [reader.i32()?, reader.i32()?];
        part.resonance_types = [reader.i32()?, reader.i32()?];
        part.enabled = reader.u8()? != 0;
        part.mid_side = reader.u8()? != 0;
        part.pluck_to_blur = reader.u8()? != 0;
        if version >= 11 {
            part.auto_gain_mode = reader.i32()?;
        }
        for a in 0..ARTICULATORS {
            if version < 16 && a == 33 {
                continue;
            }
            part.arts[a] = ArtState::read(reader)?;
        }
        for s in 0..SHAPES {
            part.shapes[s] = MEnvelope::read(reader, Class::Map)?;
        }
        part.image = Image::read(reader)?;
        part.file = ansi(reader)?;
        // The loader trusts the size word, as a later version may add to
        // the part.
        let end = start.saturating_add(size);
        if end > reader.bytes.len() {
            return Err(StateError::Length {
                expected: end,
                found: reader.bytes.len(),
            });
        }
        reader.at = end;
        for (k, (min, max, _)) in PART_RANGES.iter().enumerate() {
            part.params[k] = part.params[k].clamp(*min, *max);
        }
        for v in &mut part.params[343..386] {
            *v = (*v).clamp(0, 1001);
        }
        part.filter_types = part.filter_types.map(|t| t.clamp(0, 18));
        part.resonance_types = part.resonance_types.map(|t| t.clamp(100, 111));
        Ok(part)
    }
}

/// The global struct after the 53 global parameters (section 1, "Global
/// block").
#[derive(Debug, Clone, PartialEq)]
pub struct Globals {
    pub params: [i32; GLOBAL_PARAMS],
    pub fx_order: [i32; 6],
    pub mono: i32,
    pub voice_random: i32,
    pub max_tones: i32,
    pub safety: i32,
    pub grain: i32,
    pub precision: i32,
    pub dist_mode: i32,
    pub comp_type: i32,
    pub same_random: bool,
    pub mod_smooth: bool,
    pub threaded: bool,
    pub hq_render: bool,
    pub info: String,
}

impl Default for Globals {
    fn default() -> Self {
        Globals {
            params: GLOBAL_DEFAULTS,
            fx_order: [0, 1, 2, 3, 4, 5],
            mono: 0,
            voice_random: 1,
            max_tones: 10800,
            safety: 75,
            grain: 200,
            precision: 0,
            dist_mode: 0,
            comp_type: 0,
            same_random: false,
            mod_smooth: true,
            threaded: true,
            hq_render: true,
            info: String::new(),
        }
    }
}

/// A whole Harmor state.
#[derive(Debug, Clone, PartialEq)]
pub struct State {
    pub version: u32,
    pub mask: u32,
    pub parts: [Part; 2],
    pub globals: Globals,
}

impl Default for State {
    fn default() -> Self {
        State {
            version: 19,
            mask: 7,
            parts: [Part::new(true), Part::new(false)],
            globals: Globals::default(),
        }
    }
}

/// A string with a length byte, or 0xff and a length word.
fn length(reader: &mut Reader<'_>) -> Result<usize, StateError> {
    let n = reader.u8()?;
    Ok(if n == 0xff {
        reader.u32()? as usize
    } else {
        usize::from(n)
    })
}

fn ansi(reader: &mut Reader<'_>) -> Result<String, StateError> {
    let n = length(reader)?;
    let bytes = reader.bytes(n)?;
    let text: String = bytes.iter().map(|b| char::from(*b)).collect();
    Ok(text.trim_end_matches(|c: char| c.is_control()).to_string())
}

fn wide(reader: &mut Reader<'_>) -> Result<String, StateError> {
    let n = length(reader)?;
    let bytes = reader.bytes(n.saturating_mul(2))?;
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|w| u16::from_le_bytes(*w))
        .collect();
    Ok(String::from_utf16_lossy(&units))
}

impl State {
    /// Read a state. FL leaves one of version 0xffff, or newer than it
    /// writes, alone, and so does this.
    pub fn parse(bytes: &[u8]) -> Result<State, StateError> {
        let mut reader = Reader::new(bytes);
        let raw = reader.u32()?;
        if raw == 0xffff || raw > NEWEST {
            return Err(StateError::Version { found: raw as i32 });
        }
        let version = if raw > 0xffff { raw >> 16 } else { raw };
        let mask = reader.u32()?;
        let mut state = State {
            version,
            mask,
            ..State::default()
        };
        match mask & 3 {
            3 => {
                state.parts[0] = Part::read(&mut reader, version)?;
                state.parts[1] = Part::read(&mut reader, version)?;
            }
            // One part goes into the part the editor shows, A on a load.
            1 | 2 => state.parts[0] = Part::read(&mut reader, version)?,
            _ => {}
        }
        if mask & 4 != 0 {
            let g = &mut state.globals;
            for v in &mut g.params {
                *v = reader.i32()?;
            }
            for v in &mut g.fx_order {
                *v = reader.i32()?;
            }
            let mut ints = [0; 14];
            for v in &mut ints {
                *v = reader.i32()?;
            }
            g.mono = ints[2];
            g.voice_random = ints[3];
            g.max_tones = ints[4];
            g.safety = ints[5];
            g.grain = ints[6];
            g.precision = ints[7];
            g.dist_mode = ints[12];
            g.comp_type = ints[13];
            g.same_random = reader.u8()? != 0;
            g.mod_smooth = reader.u8()? != 0;
            g.threaded = reader.u8()? != 0;
            g.hq_render = reader.u8()? != 0;
            let _link_parts = reader.u8()?;
            g.info = if version < 18 {
                let n = reader.u32()? as usize;
                let text = reader.bytes(n)?;
                text.iter().map(|b| char::from(*b)).collect()
            } else {
                wide(&mut reader)?
            };
            if version > 18 {
                let _flags = reader.u8()?;
            }
        }
        reader.end()?;
        Ok(state)
    }

    /// The parts that sound.
    pub fn enabled(&self) -> impl Iterator<Item = usize> + '_ {
        (0..2).filter(|p| self.parts[*p].enabled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{corpus, every_preset, projects};

    #[test]
    fn the_modes_follow_the_masks() {
        assert_eq!(Mode::of(0), Mode::Add);
        assert_eq!(Mode::of(1), Mode::Multiply);
        assert_eq!(Mode::of(2), Mode::Double);
        assert_eq!(Mode::of(3), Mode::Multiply);
        assert_eq!(Mode::of(24), Mode::Add);
        assert_eq!(Mode::of(42), Mode::Double);
    }

    #[test]
    fn a_short_or_new_state_is_turned_down() {
        assert!(State::parse(&[3, 0, 0x13]).is_err());
        let newer = [4u8, 0, 0x13, 0, 0, 0, 0, 0];
        assert!(matches!(
            State::parse(&newer),
            Err(StateError::Version { .. })
        ));
        // A state with nothing in it is a version and an empty mask.
        let empty = [3u8, 0, 0x13, 0, 0, 0, 0, 0];
        let state = State::parse(&empty).unwrap();
        assert_eq!(state.version, 19);
        assert!(state.parts[0].enabled);
    }

    #[test]
    fn every_state_fl_ships_parses_to_its_last_byte() {
        let Some(root) = corpus() else {
            return;
        };
        let mut states = projects(&root);
        states.retain(|s| s.plugin.eq_ignore_ascii_case("Harmor"));
        let presets = every_preset(&root, "Harmor");
        assert_eq!(states.len(), 100);
        assert!(presets.len() >= 300, "{}", presets.len());
        let mut parts = 0;
        for saved in states.iter().chain(&presets) {
            let parsed = State::parse(&saved.state);
            assert!(parsed.is_ok(), "{} {:?}", saved.file, parsed.err());
        }
        for saved in &states {
            parts += State::parse(&saved.state).unwrap().enabled().count();
        }
        assert_eq!(parts, 128);
    }
}
