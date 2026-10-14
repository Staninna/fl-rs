//! FL Keys: a sampled keyboard, Paul Kellett's mda Piano and mda ePiano
//! as Image-Line folded them into one plugin, as `docs/plugins/fl-keys.md`
//! reads it out of `FL Keys_x64.dll`.
//!
//! # The state
//!
//! An i32 version, which FL writes as 1 and reads below 2, then the name
//! of the wavetable as a Delphi short string (a length byte and that many
//! ANSI letters) and the fifteen parameters as i32, 60 bytes. FL takes
//! the values as they come, without a look at the range of their wheel.
//! What follows the parameters is not read.
//!
//! | index | parameter | range |
//! |---|---|---|
//! | 0 | decay, at its lowest no decay | -1000 to 1000 |
//! | 1 | release | -1000 to 1000 |
//! | 2 | hardness | -1000 to 1000 |
//! | 3 | velocity to hardness | -1000 to 1000 |
//! | 4 | muffle | 0 to 1000 |
//! | 5 | velocity to muffle | 0 to 2000 |
//! | 6 | velocity sensitivity | -1000 to 1000 |
//! | 7 | stereo | 0 to 2000 |
//! | 8 | tune | -1000 to 1000 |
//! | 9 | detune | 0 to 500 |
//! | 10 | stretch | -500 to 500 |
//! | 11 | treble | -1000 to 1000 |
//! | 12 | pan or tremolo | -1000 to 1000 |
//! | 13 | LFO rate | 0 to 65536 |
//! | 14 | overdrive | 0 to 1000 |
//!
//! Each goes to `p = (value - low) / (high - low)` and the maths reads
//! `p`.
//!
//! # The wavetables
//!
//! A wavetable is a `.wti` file in `FL Keys/Wavtables` (sic), an INI file
//! that `wti_description.txt` beside it describes: its name, the rate of
//! its samples, how many velocity layers it has, and for each sample its
//! root key, the highest key it plays and where it starts, ends and loops.
//! The samples are 16-bit, either one raw file the table points into
//! (mda Piano, Rhodes) or one RIFF file a sample (Grand Piano, Roto
//! Organ, whose files are Ogg Vorbis inside RIFF).
//! [`Generator::set_resource_dir`] reads every table and the raw files.
//! The RIFF files go through the host: [`Generator::sample_paths`] names
//! them by their full path and [`Generator::set_sample`] takes them back
//! decoded. FL's data stays in FL's install.
//!
//! The state names a table by its `name`. A name no table has, such as
//! the "Piano" of FL's oldest presets, plays the table `default.wti`
//! points at, mda Piano.
//!
//! # A note
//!
//! The key, fine pitch included, picks the sample: the first group whose
//! highest key, moved by the hardness and the velocity, the key is not
//! above, and in it the velocity layer. The sample is read at its rate
//! over the song's, moved by the key's distance from the root, the tune,
//! and a detune and a stretch that grow away from key 60. Each point is a
//! line between two 16-bit points with 7 bits of the place between them.
//! An envelope that starts at the table's velocity factor falls by the
//! decay (or holds), the point is driven by the overdrive and goes through
//! a one-pole lowpass whose cutoff follows the muffle and the velocity.
//! The voice is panned by its key, and gets FL's pan law with the note's
//! velocity gain raised to the velocity sensitivity. A note off makes the
//! envelope fall at the release rate, and a voice ends when its envelope
//! is under 1e-4 or its sample runs out.
//!
//! The voices are summed and go through a treble shelf, the LFO's tremolo
//! or auto-pan and a 127-point comb that makes the stereo, in that order.
//! 32 voices sound at most; a 33rd takes the place of the quietest.
//!
//! # What is not FL's
//!
//! - FL raises the voice's whole volume, the channel's volume in it, to
//!   the velocity sensitivity. The engine puts the channel's volume on
//!   after, so a velocity sensitivity other than 1 is approximate.
//! - The RIFF files are decoded by the host and turned into 16-bit points
//!   by rounding. FL decodes them itself.
//! - A slide note sounds nothing and a voice's pitch does not move once it
//!   starts. FL moves the pitch every block.
//! - FL resamples a RIFF table with `resample` on (Roto Organ) to the
//!   song's rate when it loads it, which is not read.
//! - The point after the last of a sample that does not loop is FL's heap
//!   for the first sample of a RIFF table, and 0 here.

use crate::sampler::circular;
use crate::{Fidelity, Generator, NoteOn, SampleFile, StateError, knob_value};
use std::path::{Path, PathBuf};

/// The plugin's event 201 name.
pub const NAME: &str = "FL Keys";

/// Where the wavetables are, under the directory
/// [`Generator::set_resource_dir`] is handed.
pub const WAVTABLES: &str = "FL Keys/Wavtables";

/// The most voices that sound at once.
pub const VOICES: usize = 32;

/// How many parameters there are.
pub const PARAMS: usize = 15;

/// The range of each parameter's wheel, from the DLL's form.
pub const RANGES: [(i32, i32); PARAMS] = [
    (-1000, 1000),
    (-1000, 1000),
    (-1000, 1000),
    (-1000, 1000),
    (0, 1000),
    (0, 2000),
    (-1000, 1000),
    (0, 2000),
    (-1000, 1000),
    (0, 500),
    (-500, 500),
    (-1000, 1000),
    (-1000, 1000),
    (0, 65_536),
    (0, 1000),
];

/// What a new FL Keys plays: FL's `default.fst` preset, mda Piano.
const DEFAULTS: [i32; PARAMS] = [0, 0, 0, -1000, 784, 502, -248, 1000, 0, 123, 0, 0, 0, 0, 0];
const DEFAULT_TABLE: &str = "mda Piano";

/// The output level and the muffle range the constructor sets.
const VOLUME: f32 = 0.2;
const MUFF: f32 = 160.0;

/// A voice ends when its envelope is under this.
const QUIET: f32 = 1e-4;

/// The comb is 256 points long and its place runs over the first 128 and
/// one.
const COMB: usize = 256;
const COMB_MASK: usize = 0x7f;

/// The two treble shelf coefficients, worked out by FL for 44100 Hz and
/// used at every rate: `1 - exp(-5000 / 44100)` and the same for 14000.
const TREBLE_LOW: f32 = f32::from_bits(0x3ddb_851f);
const TREBLE_HIGH: f32 = f32::from_bits(0x3e8b_442a);

/// What FL's LFO takes for a turn: mda's 6.283, not quite `TAU`.
#[allow(clippy::approx_constant)]
const LFO_TURN: f64 = 6.283;

/// The most points a block's voices are summed into at a time.
const CHUNK: usize = 256;

/// FL's `Round`: to the nearest, and to even from a half.
fn round(x: f64) -> i32 {
    x.round_ties_even() as i32
}

/// One INI file as Delphi's `TMemIniFile` reads it: sections and keys
/// without regard to case, the first of two keys that are the same.
#[derive(Debug, Clone, Default, PartialEq)]
struct Ini {
    sections: Vec<(String, Vec<(String, String)>)>,
}

impl Ini {
    fn parse(text: &str) -> Ini {
        let mut ini = Ini::default();
        for line in text.lines() {
            let line = line.trim();
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                ini.sections.push((name.trim().to_owned(), Vec::new()));
            } else if let Some((key, value)) = line.split_once('=')
                && let Some((_, keys)) = ini.sections.last_mut()
            {
                keys.push((key.trim().to_owned(), value.trim().to_owned()));
            }
        }
        ini
    }

    /// The value of a key that is there, empty or not.
    fn get(&self, section: &str, key: &str) -> Option<&str> {
        let (_, keys) = self
            .sections
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(section))?;
        let found = keys.iter().find(|(name, _)| name.eq_ignore_ascii_case(key));
        found.map(|(_, value)| value.as_str())
    }

    fn string(&self, section: &str, key: &str) -> String {
        self.get(section, key).unwrap_or_default().to_owned()
    }

    /// `ReadInteger`: the default for a key that is not there or not a
    /// number.
    fn int(&self, section: &str, key: &str, default: i32) -> i32 {
        let value = self.get(section, key).and_then(|v| v.parse().ok());
        value.unwrap_or(default)
    }

    fn float(&self, section: &str, key: &str, default: f64) -> f64 {
        let value = self.get(section, key).and_then(|v| v.parse().ok());
        value.unwrap_or(default)
    }

    /// FL Keys's own reader: "TRUE" in any case is true, any other text
    /// false, and no text the default.
    fn bool(&self, section: &str, key: &str, default: bool) -> bool {
        match self.get(section, key) {
            None | Some("") => default,
            Some(value) => value.eq_ignore_ascii_case("TRUE"),
        }
    }
}

/// A file under `base`, where `rel` is a Windows path that may differ from
/// the file's name in case.
fn find(base: &Path, rel: &str) -> PathBuf {
    let mut at = base.to_path_buf();
    for part in rel.split(['\\', '/']).filter(|p| !p.is_empty()) {
        let exact = at.join(part);
        at = if exact.exists() {
            exact
        } else {
            let entries = std::fs::read_dir(&at).into_iter().flatten().flatten();
            let named =
                |e: &std::fs::DirEntry| e.file_name().to_string_lossy().eq_ignore_ascii_case(part);
            entries.into_iter().find(named).map_or(exact, |e| e.path())
        };
    }
    at
}

/// One `[sampleN]` of a table, as the INI has it.
#[derive(Debug, Clone, PartialEq)]
struct SampleSpec {
    /// The file of a RIFF table's sample, as its full path. `None` for a
    /// sample that plays the file of the one before it.
    file: Option<String>,
    root: i32,
    high: i32,
    pos: i32,
    end: i32,
    /// `None` where the INI says nothing, for a RIFF table's default.
    looped: Option<i32>,
    can_loop: bool,
    /// The loop of the sample's file, from its `smpl` chunk.
    file_loop: Option<(i64, i64)>,
}

/// A `.wti` file, read.
#[derive(Debug, Clone, PartialEq)]
struct Spec {
    /// What a state names it by.
    name: String,
    rate: f32,
    velocity_factor: f32,
    release_high: bool,
    resample: bool,
    layers: u8,
    thresholds: Vec<i32>,
    /// The points of a raw table, all its samples in one file.
    raw: Option<Vec<i16>>,
    /// A raw table that has no file, or a kind of table that is not built.
    broken: bool,
    samples: Vec<SampleSpec>,
}

impl Spec {
    fn read(path: &Path) -> Option<Spec> {
        let ini = Ini::parse(&String::from_utf8_lossy(&std::fs::read(path).ok()?));
        let dir = path.parent()?;
        let sub = ini.string("info", "path");
        let base = if sub.is_empty() {
            dir.to_path_buf()
        } else {
            find(dir, &sub)
        };
        let count = ini.int("info", "samples", 0);
        let sample_file = ini.string("info", "samplefile");
        let riff = ini.string("info", "filetype").is_empty()
            || ini.string("info", "filetype").eq_ignore_ascii_case("riff");
        let raw_type = ini.string("info", "filetype").eq_ignore_ascii_case("raw");
        let mut broken =
            count < 1 || !(raw_type && !sample_file.is_empty() || riff && sample_file.is_empty());
        let raw = if raw_type && !sample_file.is_empty() {
            let bytes = std::fs::read(find(&base, &sample_file)).unwrap_or_default();
            let points: Vec<i16> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|p| i16::from_le_bytes(*p))
                .collect();
            broken |= points.is_empty();
            Some(points)
        } else {
            None
        };
        let can_loop = ini.bool("info", "canloop", true);
        let mut samples = Vec::new();
        for n in 0..count.max(0) {
            let section = format!("sample{n}");
            let name = ini.string(&section, "name");
            let file = (raw.is_none() && !name.is_empty())
                .then(|| find(&base, &name).to_string_lossy().into_owned());
            let file_loop = file.as_deref().and_then(wav_loop);
            samples.push(SampleSpec {
                file,
                root: ini.int(&section, "root", -1),
                high: ini.int(&section, "high", 0),
                pos: ini.int(&section, "pos", 0),
                end: ini.int(&section, "end", 0),
                looped: ini.get(&section, "loop").and_then(|v| v.parse().ok()),
                can_loop: can_loop && ini.bool(&section, "canloop", true),
                file_loop,
            });
        }
        let layers = match ini.int("velocitylayers", "count", 1) as u8 {
            0 => 1,
            layers => layers,
        };
        let mut thresholds: Vec<i32> = (0..layers)
            .map(|n| ini.int("velocitylayers", &n.to_string(), 0x80))
            .collect();
        thresholds[0] = 0;
        Some(Spec {
            name: ini.string("info", "name"),
            rate: ini.float("info", "samplerate", 44_100.0) as f32,
            velocity_factor: ini.float("info", "velocityfactor", 1.0) as f32,
            release_high: ini.bool("info", "releasehighnotes", false),
            resample: ini.bool("info", "resample", true),
            layers,
            thresholds,
            raw,
            broken,
            samples,
        })
    }

    /// The RIFF files the table plays, each once.
    fn files(&self) -> Vec<String> {
        let mut files: Vec<String> = Vec::new();
        for file in self.samples.iter().filter_map(|s| s.file.as_ref()) {
            if !files.iter().any(|known| known.eq_ignore_ascii_case(file)) {
                files.push(file.clone());
            }
        }
        files
    }
}

/// A decoded RIFF file as FL keeps it: 16-bit points, both sides of a
/// stereo file one after the other, and where its loop starts and ends.
#[derive(Debug, Clone, PartialEq)]
struct Riff {
    points: Vec<i16>,
    frames: usize,
    stereo: bool,
    loop_start: i64,
    loop_end: i64,
}

impl Riff {
    fn of(file: SampleFile<'_>, (loop_start, loop_end): (i64, i64)) -> Riff {
        let point = |x: f32| (f64::from(x) * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16;
        let frames = file.left.len();
        let points = match file.right {
            Some(right) => file
                .left
                .iter()
                .zip(right)
                .flat_map(|(l, r)| [point(*l), point(*r)])
                .collect(),
            None => file.left.iter().map(|x| point(*x)).collect(),
        };
        Riff {
            points,
            frames,
            stereo: file.right.is_some(),
            loop_start,
            loop_end,
        }
    }
}

/// The loop of a WAV file's `smpl` chunk: its first and last frame. A file
/// with none loops from its first frame to its last, as the frames give
/// it.
fn wav_loop(path: &str) -> Option<(i64, i64)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let mut head = [0u8; 12];
    file.read_exact(&mut head).ok()?;
    if &head[..4] != b"RIFF" || &head[8..] != b"WAVE" {
        return None;
    }
    let mut chunk = [0u8; 8];
    while file.read_exact(&mut chunk).is_ok() {
        let size = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
        if &chunk[..4] == b"smpl" && size >= 60 {
            let mut body = [0u8; 60];
            file.read_exact(&mut body).ok()?;
            let word = |at: usize| {
                u32::from_le_bytes([body[at], body[at + 1], body[at + 2], body[at + 3]])
            };
            if word(28) == 0 {
                return None;
            }
            return Some((i64::from(word(44)), i64::from(word(48))));
        }
        file.seek(SeekFrom::Current(i64::from(size) + i64::from(size & 1)))
            .ok()?;
    }
    None
}

/// One sample of a loaded table: `+0x40 + n * 0x124` in the DLL.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Zone {
    root: i32,
    high: i32,
    pos: i32,
    end: i32,
    looped: i32,
}

/// A wavetable as `TPianoWaveTable` holds it once it is loaded: every
/// point in one run, mono or both sides by turns, and the zones into it.
#[derive(Debug, Clone, PartialEq)]
struct Table {
    points: Vec<i16>,
    stereo: bool,
    zones: Vec<Zone>,
    rate: f32,
    velocity_factor: f32,
    release_high: bool,
    layers: u8,
    thresholds: Vec<i32>,
}

impl Table {
    /// The loader `FUN_00713920` and its tidy-up `FUN_00716470`. `riffs`
    /// has the decoded file of each of [`Spec::files`]; `None` when one is
    /// missing, as FL then has no table.
    fn load(spec: &Spec, riffs: &[(String, Riff)]) -> Option<Table> {
        if spec.broken {
            return None;
        }
        let mut points = spec.raw.clone().unwrap_or_default();
        let mut stereo = false;
        let mut zones: Vec<Zone> = Vec::with_capacity(spec.samples.len());
        let mut names: Vec<Option<&str>> = Vec::with_capacity(spec.samples.len());
        let (mut next, mut last_start) = (0i32, 0i32);
        let (mut default_loop, mut loop_end) = (0i32, 0i32);
        let (mut root, mut high) = (60, 999);
        for sample in &spec.samples {
            let mut zone = Zone {
                root: 0,
                high: 0,
                pos: 0,
                end: 0,
                looped: 0,
            };
            let file = sample.file.as_deref();
            let same = file.and_then(|file| {
                let found = names
                    .iter()
                    .position(|n| n.is_some_and(|n| n.eq_ignore_ascii_case(file)));
                found.map(|at| zones[at])
            });
            let mut frames = 0;
            if let Some(found) = same {
                zone = found;
            } else if let Some(file) = file {
                let (_, riff) = riffs
                    .iter()
                    .find(|(path, _)| path.eq_ignore_ascii_case(file))?;
                if riff.frames == 0 {
                    return None;
                }
                frames = riff.frames as i32;
                stereo = riff.stereo;
                let start = riff.loop_start.clamp(0, i64::from(frames)) as i32;
                loop_end = riff
                    .loop_end
                    .min(i64::from(frames) - 1)
                    .max(i64::from(start)) as i32;
                default_loop = loop_end - start + 1;
                // A slot more than the file, for the point after its last.
                let width = if stereo { 2 } else { 1 };
                points.resize((next as usize + riff.frames + 1) * width, 0);
                let at = next as usize * width;
                if riff.stereo || !stereo {
                    points[at..at + riff.points.len()].copy_from_slice(&riff.points);
                } else {
                    for (n, point) in riff.points.iter().enumerate() {
                        points[at + 2 * n] = *point;
                        points[at + 2 * n + 1] = *point;
                    }
                }
            }
            zone.root = if sample.root == -1 { root } else { sample.root };
            zone.high = if sample.high == -1 { high } else { sample.high };
            (root, high) = (zone.root, zone.high);
            if same.is_none() {
                if spec.raw.is_some() {
                    zone.pos = sample.pos;
                    zone.end = sample.end;
                    zone.looped = sample.looped.unwrap_or(0);
                } else {
                    let start = if file.is_some() { next } else { last_start };
                    zone.pos = start + sample.pos;
                    zone.end = start + sample.end;
                    if zone.end == start {
                        zone.end = start + loop_end;
                    }
                    zone.looped = sample.looped.unwrap_or(default_loop);
                    if file.is_some() {
                        let width = if stereo { 2 } else { 1 };
                        for side in 0..width {
                            let to = (zone.end + 1) as usize * width + side;
                            let from = usize::try_from(zone.end - zone.looped).ok();
                            let point = from.and_then(|from| points.get(from * width + side));
                            if let (Some(&point), true) = (point, to < points.len()) {
                                points[to] = point;
                            }
                        }
                    }
                }
                if !sample.can_loop {
                    // Set below, once the ends are inside the points.
                    zone.looped = i32::MIN;
                }
                if file.is_some() {
                    last_start = next;
                    next += frames + 1;
                }
            }
            zones.push(zone);
            names.push(file);
        }
        // The tidy-up: everything inside the points, and a sample that
        // does not loop loops by its end, which ends it there.
        let frames = if stereo {
            points.len() / 2
        } else {
            points.len()
        } as i32;
        for (zone, sample) in zones.iter_mut().zip(&spec.samples) {
            zone.root = zone.root.clamp(0, 127);
            zone.high = zone.high.clamp(0, 999);
            zone.pos = zone.pos.clamp(0, frames - 1);
            zone.end = zone.end.max(zone.pos).min(frames - 1);
            zone.looped = zone.looped.max(0).min(frames - zone.pos);
            if !sample.can_loop {
                zone.looped = zone.end;
            }
        }
        if frames < 1 {
            return None;
        }
        Some(Table {
            points,
            stereo,
            zones,
            rate: spec.rate,
            velocity_factor: spec.velocity_factor,
            release_high: spec.release_high,
            layers: spec.layers,
            thresholds: spec.thresholds.clone(),
        })
    }

    /// The point at a frame and side; 0 outside the points.
    fn point(&self, frame: i32, side: usize) -> i32 {
        let index = if self.stereo {
            frame as usize * 2 + side
        } else {
            frame as usize
        };
        if frame < 0 {
            return 0;
        }
        self.points.get(index).map_or(0, |p| i32::from(*p))
    }

    /// mda's line between two points: the point at `pos` and the one after,
    /// 7 bits of `frac` between them, made a float by its bits as `3 +
    /// x / 32768`, less 3.
    fn read(&self, pos: i32, frac: u32, side: usize) -> f64 {
        let a = self.point(pos, side);
        let b = self.point(pos + 1, side);
        let bits = (a << 7)
            .wrapping_add(((frac >> 9) as i32).wrapping_mul(b - a))
            .wrapping_add(0x4040_0000);
        f64::from(f32::from_bits(bits as u32)) - 3.0
    }
}

/// What the parameters make, from `FUN_00721230`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Derived {
    size: i32,
    size_velocity: f32,
    muffle_velocity: f32,
    treble: f32,
    treble_coefficient: f32,
    left_mod: f32,
    right_mod: f32,
    lfo_step: f32,
    velocity_sensitivity: f32,
    fine: f32,
    random: f32,
    stretch: f32,
    overdrive: f32,
    comb_depth: f32,
    trim: f32,
    width: f32,
}

impl Derived {
    fn of(p: &[f32; PARAMS], inverse_rate: f32) -> Derived {
        let d = |x: f32| f64::from(x);
        let mut velocity_sensitivity = (d(p[6]) + 1.0 + d(p[6])) as f32;
        if d(p[6]) < 0.25 {
            velocity_sensitivity = (d(velocity_sensitivity) - (0.75 - d(p[6]) * 3.0)) as f32;
        }
        let left_mod = (d(p[12] + p[12]) - 1.0) as f32;
        let comb_depth = p[7] * p[7];
        let width = (d(p[7]) * 0.04) as f32;
        Derived {
            size: round(d(p[2]) * 12.0 - 6.0),
            size_velocity: (d(p[3]) * 0.12) as f32,
            muffle_velocity: (d(p[5] * p[5]) * 5.0) as f32,
            treble: (d(p[11]) * 4.0 * d(p[11]) - 1.0) as f32,
            treble_coefficient: if d(p[11]) <= 0.5 {
                TREBLE_LOW
            } else {
                TREBLE_HIGH
            },
            left_mod,
            right_mod: if d(p[12]) < 0.5 { -left_mod } else { left_mod },
            lfo_step: (d(inverse_rate) * LFO_TURN * (d(p[13]) * 9.22 - 4.61).exp()) as f32,
            velocity_sensitivity,
            fine: (d(p[8] + p[8]) - 1.0) as f32,
            random: (d(p[9]) * 0.077 * d(p[9])) as f32,
            stretch: ((d(p[10]) - 0.5) * 0.000_434) as f32,
            overdrive: (d(p[14]) * 1.8) as f32,
            comb_depth,
            trim: (1.0 - d(comb_depth) * 0.53) as f32,
            width: if f64::from(width) > 0.03 { 0.03 } else { width },
        }
    }
}

/// One voice, the 100-byte record `TriggerVoice` makes.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Voice {
    id: u32,
    delta: i32,
    frac: u32,
    pos: i32,
    end: i32,
    looped: i32,
    envelope: f32,
    decay: f32,
    /// The muffle lowpass of each side: its output and the input before.
    low: [f32; 2],
    last: [f32; 2],
    cutoff: f32,
    /// The key-pan of each side, and FL's pan law with the velocity.
    out: [f32; 2],
    gains: [f32; 2],
    /// The key, fine pitch in it, inside 0 to 127.
    note: f32,
    dead: bool,
}

/// FL Keys with its wavetables, its parameters and its voices.
#[derive(Debug, Clone)]
pub struct FlKeys {
    values: [i32; PARAMS],
    p: [f32; PARAMS],
    derived: Derived,
    /// The name the state gave.
    table_name: String,
    specs: Vec<Spec>,
    /// Which spec plays: the state's, or the first, `default.wti`'s.
    chosen: Option<usize>,
    riffs: Vec<(String, Option<Riff>)>,
    table: Option<Table>,
    rate: f32,
    inverse_rate: f32,
    voices: Vec<Voice>,
    treble_state: [f32; 2],
    lfo: [f32; 2],
    comb: [f32; COMB],
    comb_at: usize,
}

impl Default for FlKeys {
    fn default() -> Self {
        let mut keys = FlKeys {
            values: DEFAULTS,
            p: [0.0; PARAMS],
            derived: Derived::default(),
            table_name: DEFAULT_TABLE.to_owned(),
            specs: Vec::new(),
            chosen: None,
            riffs: Vec::new(),
            table: None,
            rate: 44_100.0,
            inverse_rate: (1.0 / 44_100.0f64) as f32,
            voices: Vec::with_capacity(VOICES + 1),
            treble_state: [0.0; 2],
            lfo: [0.0, 1.0],
            comb: [0.0; COMB],
            comb_at: 0,
        };
        keys.update();
        keys
    }
}

/// `p` of a value on a wheel with this range.
fn normal(value: i32, (low, high): (i32, i32)) -> f32 {
    (value - low) as f32 / (high - low) as f32
}

impl FlKeys {
    fn update(&mut self) {
        for (n, value) in self.values.iter().enumerate() {
            self.p[n] = normal(*value, RANGES[n]);
        }
        self.derived = Derived::of(&self.p, self.inverse_rate);
    }

    /// Pick the table the state names and load it, or wait for the host's
    /// files.
    fn choose(&mut self) {
        self.table = None;
        self.riffs.clear();
        if self.specs.is_empty() {
            self.chosen = None;
            return;
        }
        let named = self
            .specs
            .iter()
            .position(|s| s.name.eq_ignore_ascii_case(&self.table_name));
        let chosen = named.unwrap_or(0);
        self.chosen = Some(chosen);
        let spec = &self.specs[chosen];
        if spec.raw.is_some() {
            self.table = Table::load(spec, &[]);
        } else {
            self.riffs = spec.files().into_iter().map(|f| (f, None)).collect();
            if self.riffs.is_empty() {
                self.table = Table::load(spec, &[]);
            }
        }
    }

    fn try_load(&mut self) {
        let Some(chosen) = self.chosen else { return };
        if self.riffs.iter().any(|(_, riff)| riff.is_none()) {
            return;
        }
        let riffs: Vec<(String, Riff)> = self
            .riffs
            .iter()
            .filter_map(|(path, riff)| Some((path.clone(), riff.clone()?)))
            .collect();
        self.table = Table::load(&self.specs[chosen], &riffs);
    }

    /// The velocity FL works out of the note's gain, 0 to 127: the
    /// inverse of its velocity curve, with `1 / ln 11` as the DLL keeps it,
    /// a float.
    fn velocity(gain: f32) -> f32 {
        let inverse_ln11 = 1.0 / (11f64.ln() as f32);
        let ln = f64::from(gain * 10.0 + 1.0).ln();
        let velocity = round(f64::from((ln * f64::from(inverse_ln11)) as f32 * 100.0));
        velocity.min(127) as f32
    }

    /// `TriggerVoice`, `FUN_00721660`.
    fn start(&self, table: &Table, note: &NoteOn) -> Voice {
        let d = &self.derived;
        let pitch = (f32::from(note.key) - 60.0) * 100.0 + note.fine_pitch;
        let key = ((pitch + 6000.0) * 0.01).clamp(0.0, 127.0);
        let velocity = FlKeys::velocity(note.velocity);
        let k = round(f64::from((key - 60.0) * (key - 60.0)));
        let mut tune = (f64::from(d.fine) + f64::from(d.random) * (f64::from(k % 13) - 6.5)) as f32;
        if key > 60.0 {
            tune += d.stretch * k as f32;
        }
        let mut size = d.size;
        if velocity > 40.0 {
            size += round(f64::from(d.size_velocity * (velocity - 40.0)));
        }
        let mut at = 0;
        while at + usize::from(table.layers) < table.zones.len()
            && ((table.zones[at].high + size) as f32) < key
        {
            at += usize::from(table.layers);
        }
        let mut layer = 1;
        while layer < usize::from(table.layers) && (table.thresholds[layer] as f32) < velocity {
            at += 1;
            layer += 1;
        }
        let zone = table.zones[at.min(table.zones.len() - 1)];
        let up = (f64::from(tune + (key - zone.root as f32)) * 0.057_762_265_05).exp();
        let delta =
            round(f64::from((f64::from(table.rate * self.inverse_rate) * up) as f32) * 65_536.0);
        let mut cutoff = (f64::from(self.p[4] * self.p[4] * MUFF)
            + 50.0
            + f64::from(d.muffle_velocity * (velocity - 64.0))) as f32;
        let floor = f64::from(key) * 0.4 + 55.0;
        if f64::from(cutoff) < floor {
            cutoff = floor as f32;
        }
        if 210.0 < f64::from(cutoff) {
            cutoff = 210.0;
        }
        let panned = key.clamp(12.0, 108.0);
        let volume = VOLUME * d.trim;
        let right = volume + volume * d.width * (panned - 60.0);
        let left = (volume + volume) - right;
        let held = panned.max(44.0);
        let decay = if self.p[0] == 0.0 {
            1.0
        } else {
            let mut l = (f64::from(self.p[0]) * 2.0) as f32;
            if f64::from(l) < 1.0 {
                l = ((f64::from(l) + 0.25) - f64::from(self.p[0]) * 0.5) as f32;
            }
            let rate = (f64::from(held) * 0.033 + -0.6 - f64::from(l)).exp();
            (f64::from(-self.inverse_rate) * rate).exp() as f32
        };
        let gain = f64::from(note.velocity.max(0.0)).powf(f64::from(d.velocity_sensitivity)) as f32;
        let (gain_left, gain_right) = circular(note.pan, gain);
        Voice {
            id: note.voice,
            delta,
            frac: 0,
            pos: zone.pos,
            end: zone.end,
            looped: zone.looped,
            envelope: table.velocity_factor,
            decay,
            low: [0.0; 2],
            last: [0.0; 2],
            cutoff: cutoff * cutoff * self.inverse_rate,
            out: [left, right],
            gains: [gain_left, gain_right],
            note: key,
            dead: false,
        }
    }

    /// `Voice_Release`, `FUN_00722140`: the envelope's rate from here on.
    fn release_rate(&self, note: f32, release_high: bool) -> f32 {
        let note = f64::from(note);
        let release = f64::from(self.p[1]);
        let rate = if !release_high && (note < 94.0 || note == 128.0) {
            (note * 0.017 + 2.0 - release * 2.0).exp()
        } else {
            (note * 0.01 + 6.0 - release * 5.0).exp()
        };
        (f64::from(-self.inverse_rate) * rate).exp() as f32
    }

    /// The voices' sum for `len` points into `mix`, left and right by turns.
    fn voices_into(&mut self, mix: &mut [f32]) {
        let Some(table) = &self.table else { return };
        let len = mix.len() / 2;
        let overdrive = self.derived.overdrive;
        let drive = |mut x: f32| {
            if f64::from(x) > 0.0 && overdrive != 0.0 {
                x -= overdrive * x * x;
                if f64::from(x) < 0.0 {
                    x = (f64::from(x) + f64::from(x) * 0.5 * f64::from(x)) as f32;
                }
            }
            x
        };
        for voice in self.voices.iter_mut().filter(|v| !v.dead) {
            for n in 0..len {
                voice.frac = voice.frac.wrapping_add(voice.delta as u32);
                voice.pos = voice.pos.wrapping_add((voice.frac >> 16) as i32);
                voice.frac &= 0xffff;
                if voice.end < voice.pos {
                    if voice.end <= voice.looped {
                        voice.dead = true;
                        break;
                    }
                    voice.pos -= voice.looped;
                }
                let envelope = f64::from(voice.envelope);
                let x = (envelope * table.read(voice.pos, voice.frac, 0)) as f32;
                let y = table
                    .stereo
                    .then(|| (envelope * table.read(voice.pos, voice.frac, 1)) as f32);
                voice.envelope *= voice.decay;
                let x = drive(x);
                voice.low[0] += voice.cutoff * ((x + voice.last[0]) - voice.low[0]);
                voice.last[0] = x;
                if let Some(y) = y {
                    let y = drive(y);
                    voice.low[1] += voice.cutoff * ((y + voice.last[1]) - voice.low[1]);
                    voice.last[1] = y;
                } else {
                    voice.low[1] = voice.low[0];
                    voice.last[1] = voice.last[0];
                }
                mix[2 * n] += voice.gains[0] * voice.out[0] * voice.low[0];
                mix[2 * n + 1] += voice.gains[1] * voice.out[1] * voice.low[1];
            }
        }
    }

    /// The treble shelf, the LFO and the comb, on the voices' sum.
    fn after(&mut self, mix: &mut [f32]) {
        let d = self.derived;
        for frame in mix.as_chunks_mut::<2>().0 {
            let (mut l, mut r) = (frame[0], frame[1]);
            self.treble_state[0] += d.treble_coefficient * (l - self.treble_state[0]);
            self.treble_state[1] += d.treble_coefficient * (r - self.treble_state[1]);
            r += d.treble * (r - self.treble_state[1]);
            l += d.treble * (l - self.treble_state[0]);
            self.lfo[0] += d.lfo_step * self.lfo[1];
            self.lfo[1] -= d.lfo_step * self.lfo[0];
            l += l * d.left_mod * self.lfo[1];
            r += r * d.right_mod * self.lfo[1];
            self.comb[self.comb_at] = l + r;
            self.comb_at = (self.comb_at & COMB_MASK) + 1;
            let x = d.comb_depth * self.comb[self.comb_at];
            frame[0] = l + x;
            frame[1] = r - x;
        }
    }
}

impl Generator for FlKeys {
    /// FL's `SaveRestoreState`: a version under 2, the table's name and the
    /// fifteen values, taken as they are. What follows is not read.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let version = crate::state_version(bytes, 5 + 4 * PARAMS)?;
        if version >= 2 {
            return Err(StateError::Version { found: version });
        }
        let name_len = usize::from(*bytes.get(4).ok_or(StateError::Length {
            expected: 5 + 4 * PARAMS,
            found: bytes.len(),
        })?);
        let expected = 5 + name_len + 4 * PARAMS;
        if bytes.len() < expected {
            return Err(StateError::Length {
                expected,
                found: bytes.len(),
            });
        }
        let name: String = bytes[5..5 + name_len]
            .iter()
            .map(|b| char::from(*b))
            .collect();
        let values = bytes[5 + name_len..expected].as_chunks::<4>().0;
        for (value, word) in self.values.iter_mut().zip(values) {
            *value = i32::from_le_bytes(*word);
        }
        self.update();
        self.voices.clear();
        self.table_name = name;
        self.choose();
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        self.rate = hz;
        self.inverse_rate = (1.0 / f64::from(hz)) as f32;
        self.update();
    }

    /// Reads every `.wti` in `FL Keys/Wavtables` and the raw files they
    /// point into. `default.wti` names the table that is first, which a
    /// state with a name no table has plays.
    fn set_resource_dir(&mut self, dir: &Path) {
        let wavtables = find(dir, WAVTABLES);
        let Ok(entries) = std::fs::read_dir(&wavtables) else {
            return;
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("wti")))
            .filter(|p| {
                !p.file_stem()
                    .is_some_and(|s| s.eq_ignore_ascii_case("default"))
            })
            .collect();
        paths.sort_by_key(|p| p.to_string_lossy().to_lowercase());
        let default = std::fs::read(find(&wavtables, "default.wti")).ok();
        let default = default
            .map(|text| Ini::parse(&String::from_utf8_lossy(&text)).string("default", "file"));
        if let Some(first) = default.and_then(|file| {
            paths.iter().position(|p| {
                p.file_name()
                    .is_some_and(|n| n.eq_ignore_ascii_case(file.as_str()))
            })
        }) {
            let path = paths.remove(first);
            paths.insert(0, path);
        }
        self.specs = paths.iter().filter_map(|p| Spec::read(p)).collect();
        self.choose();
    }

    /// `TriggerVoice`. Nothing sounds without a table or for a slide note.
    fn note_on(&mut self, note: NoteOn) {
        if note.slide {
            return;
        }
        let Some(table) = &self.table else { return };
        let voice = self.start(table, &note);
        let live = self.voices.iter().filter(|v| !v.dead).count();
        if live >= VOICES {
            let quietest = (0..self.voices.len())
                .filter(|&n| !self.voices[n].dead)
                .min_by(|&a, &b| self.voices[a].envelope.total_cmp(&self.voices[b].envelope));
            if let Some(quietest) = quietest {
                self.voices.remove(quietest);
            }
        }
        self.voices.retain(|v| !v.dead);
        self.voices.push(voice);
    }

    fn note_off(&mut self, voice: u32) {
        let Some(table) = &self.table else { return };
        let release_high = table.release_high;
        for n in 0..self.voices.len() {
            if self.voices[n].id == voice {
                self.voices[n].decay = self.release_rate(self.voices[n].note, release_high);
            }
        }
    }

    fn all_notes_off(&mut self) {
        let Some(table) = &self.table else { return };
        let release_high = table.release_high;
        for n in 0..self.voices.len() {
            self.voices[n].decay = self.release_rate(self.voices[n].note, release_high);
        }
    }

    /// `Gen_Render`, `FUN_00722360`: nothing at all, the effects included,
    /// while no voice sounds.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if self.voices.is_empty() || self.table.is_none() {
            return;
        }
        let mut mix = [0.0f32; 2 * CHUNK];
        let len = left.len().min(right.len());
        let mut done = 0;
        while done < len {
            let n = (len - done).min(CHUNK);
            let mix = &mut mix[..2 * n];
            mix.fill(0.0);
            self.voices_into(mix);
            self.after(mix);
            for (k, frame) in mix.as_chunks::<2>().0.iter().enumerate() {
                left[done + k] += frame[0];
                right[done + k] += frame[1];
            }
            done += n;
        }
        self.voices
            .retain(|v| !v.dead && v.envelope.to_bits() >= QUIET.to_bits());
        for state in &mut self.treble_state {
            if f64::from(state.abs()) < 1e-10 {
                *state = 0.0;
            }
        }
    }

    fn param_count(&self) -> usize {
        PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        self.values.get(index).map_or(0.0, |v| *v as f32)
    }

    /// The voices that sound keep what they started with, save the
    /// overdrive; the shelf, the LFO and the comb change at once.
    fn set_param(&mut self, index: usize, value: f32) {
        if index < PARAMS {
            self.values[index] = knob_value(value, RANGES[index]);
            self.update();
        }
    }

    fn reset(&mut self) {
        self.voices.clear();
    }

    /// The longest a voice can take to fall from the table's velocity
    /// factor to 1e-4 at the release rate, at the key that releases
    /// slowest.
    fn tail(&self) -> usize {
        let start = self
            .table
            .as_ref()
            .map_or(4.0, |t| t.velocity_factor)
            .max(1.0);
        let release = f64::from(self.p[1]);
        let piano = (2.0 - release * 2.0).exp();
        let high = (94.0 * 0.01 + 6.0 - release * 5.0).exp();
        let all = (6.0 - release * 5.0).exp();
        let slowest = match self.table.as_ref().is_some_and(|t| t.release_high) {
            true => all,
            false => piano.min(high),
        };
        let seconds = (f64::from(start) / f64::from(QUIET)).ln() / slowest;
        (seconds * f64::from(self.rate)).ceil() as usize + CHUNK
    }

    fn fidelity(&self) -> Fidelity {
        if self.table.is_none() {
            return Fidelity::Approximate("FL Keys wavetable not available");
        }
        let resampled = self.chosen.is_some_and(|c| {
            let spec = &self.specs[c];
            spec.raw.is_none() && spec.resample && spec.rate != self.rate
        });
        if resampled {
            return Fidelity::Approximate("FL Keys: a resampled RIFF wavetable");
        }
        if self.derived.velocity_sensitivity != 1.0 {
            return Fidelity::Approximate(
                "FL Keys: the channel volume is not raised to the velocity sensitivity",
            );
        }
        Fidelity::Exact
    }

    fn sample_paths(&self) -> Vec<String> {
        self.riffs.iter().map(|(path, _)| path.clone()).collect()
    }

    /// Takes a RIFF file of the table. The table plays once every file is
    /// there.
    fn set_sample(&mut self, path: &str, file: Option<SampleFile<'_>>) {
        let Some(at) = self.riffs.iter().position(|(p, _)| p == path) else {
            return;
        };
        let Some(file) = file else {
            self.riffs[at].1 = None;
            self.table = None;
            return;
        };
        let frames = file.left.len() as i64;
        let spec = self.chosen.map(|c| &self.specs[c]);
        let samples = spec.into_iter().flat_map(|s| &s.samples);
        let mut named = samples.filter(|s| s.file.as_deref() == Some(path));
        let looped = named.find_map(|s| s.file_loop).unwrap_or((0, frames - 1));
        self.riffs[at].1 = Some(Riff::of(file, looped));
        self.try_load();
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
            slide: false,
            porta: false,
        }
    }

    fn state(name: &str, values: &[i32; PARAMS]) -> Vec<u8> {
        let mut bytes = 1i32.to_le_bytes().to_vec();
        bytes.push(name.len() as u8);
        bytes.extend(name.bytes());
        bytes.extend(testing::state(values));
        bytes
    }

    /// A raw table of one sample: a sine of `period` points at 16000, root
    /// key 60 and played to 999, that loops over its last whole periods.
    fn sine_spec(period: usize, periods: usize) -> Spec {
        let len = period * periods;
        let raw: Vec<i16> = (0..len + 1)
            .map(|n| (16_000.0 * (std::f64::consts::TAU * n as f64 / period as f64).sin()) as i16)
            .collect();
        Spec {
            name: "Sine".to_owned(),
            rate: 44_100.0,
            velocity_factor: 1.0,
            release_high: false,
            resample: true,
            layers: 1,
            thresholds: vec![0],
            raw: Some(raw),
            broken: false,
            samples: vec![SampleSpec {
                file: None,
                root: 60,
                high: 999,
                pos: 0,
                end: len as i32 - 1,
                looped: Some((period * (periods / 2)) as i32),
                can_loop: true,
                file_loop: None,
            }],
        }
    }

    /// FL Keys at 44100 Hz with one table and these values.
    fn keys_with(spec: Spec, values: &[i32; PARAMS]) -> FlKeys {
        let mut keys = FlKeys::default();
        keys.set_sample_rate(44_100.0);
        keys.specs = vec![spec];
        keys.set_state(&state("Sine", values)).unwrap();
        keys
    }

    /// No decay, nothing on the shelf or the LFO, no stereo, no muffle
    /// floor in the way and a velocity sensitivity of 1.
    const PLAIN: [i32; PARAMS] = [-1000, 0, 0, -1000, 1000, 0, -700, 0, 0, 0, 0, 0, 0, 0, 0];

    fn play(keys: &mut FlKeys, len: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut left, mut right) = (vec![0.0; len], vec![0.0; len]);
        keys.process(&mut left, &mut right);
        (left, right)
    }

    /// The upward zero crossings of a sound after `skip` points, and the
    /// mean distance between them.
    fn period(sound: &[f32], skip: usize) -> f64 {
        let ups: Vec<usize> = (skip + 1..sound.len())
            .filter(|&n| sound[n - 1] <= 0.0 && sound[n] > 0.0)
            .collect();
        (ups[ups.len() - 1] - ups[0]) as f64 / (ups.len() - 1) as f64
    }

    #[test]
    fn an_ini_is_read_as_delphi_reads_one() {
        let ini = Ini::parse(
            "[Info]\r\nName=Grand Piano\nsamples=30\nshortname=A\nshortname=B\npos=\nok=True\n[sample0]\nroot=21",
        );
        assert_eq!(ini.string("info", "NAME"), "Grand Piano");
        assert_eq!(ini.int("info", "samples", 0), 30);
        assert_eq!(ini.string("info", "shortname"), "A");
        assert_eq!(ini.int("info", "pos", 7), 7);
        assert!(ini.bool("info", "ok", false) && ini.bool("info", "none", true));
        assert_eq!(ini.int("sample0", "root", -1), 21);
        assert_eq!(ini.float("info", "samplerate", 44_100.0), 44_100.0);
    }

    #[test]
    fn the_parameters_make_what_mda_s_update_does() {
        let mut p = [0.5f32; PARAMS];
        p[6] = 0.1;
        p[7] = 1.0;
        p[12] = 0.25;
        let d = Derived::of(&p, 1.0 / 44_100.0);
        assert_eq!(d.size, 0);
        assert!((d.size_velocity - 0.06).abs() < 1e-7);
        assert!((d.muffle_velocity - 1.25).abs() < 1e-7);
        assert_eq!((d.treble, d.treble_coefficient), (0.0, TREBLE_LOW));
        assert!((d.left_mod + 0.5).abs() < 1e-7 && (d.right_mod - 0.5).abs() < 1e-7);
        // Under a quarter the sensitivity is 0.25 + 5 p.
        assert!((d.velocity_sensitivity - 0.75).abs() < 1e-6);
        assert_eq!(d.fine, 0.0);
        assert!((d.random - 0.019_25).abs() < 1e-7 && d.stretch == 0.0);
        assert!((d.overdrive - 0.9).abs() < 1e-7);
        assert_eq!(d.comb_depth, 1.0);
        assert!((d.trim - 0.47).abs() < 1e-7 && d.width == 0.03);
        // The LFO turns at exp(9.22 p - 4.61) Hz: 1 Hz in the middle.
        let turn = f64::from(d.lfo_step) / std::f64::consts::TAU * 44_100.0;
        assert!((turn - 1.0).abs() < 2e-3, "{turn}");
        // The shelf is the 44.1 kHz one at every rate.
        assert!((f64::from(TREBLE_LOW) - (1.0 - (-5000.0f64 / 44_100.0).exp())).abs() < 1e-4);
        assert!((f64::from(TREBLE_HIGH) - (1.0 - (-14_000.0f64 / 44_100.0).exp())).abs() < 1e-4);
    }

    #[test]
    fn the_velocity_is_the_inverse_of_fl_s_curve() {
        assert_eq!(FlKeys::velocity(1.0), 100.0);
        assert_eq!(FlKeys::velocity(0.0), 0.0);
        assert_eq!(FlKeys::velocity(0.364), 64.0);
        assert_eq!(FlKeys::velocity(3.0), 127.0);
    }

    #[test]
    fn a_state_is_a_name_and_fifteen_values_and_a_newer_one_is_refused() {
        let mut keys = FlKeys::default();
        let values = [
            -1000, 250, 0, -1000, 0, 1000, -500, 0, 0, 123, 0, 0, 0, 43_400, 0,
        ];
        let mut bytes = state("Grand Piano", &values);
        bytes.extend([9, 9]);
        keys.set_state(&bytes).unwrap();
        assert_eq!(keys.table_name, "Grand Piano");
        let read: Vec<f32> = (0..PARAMS).map(|n| keys.param(n)).collect();
        assert_eq!(read, values.map(|v| v as f32));
        assert_eq!(keys.p[13], 43_400.0 / 65_536.0);
        let mut newer = bytes.clone();
        newer[0] = 2;
        assert_eq!(
            keys.set_state(&newer),
            Err(StateError::Version { found: 2 })
        );
        let short = &bytes[..bytes.len() - 10];
        assert!(matches!(
            keys.set_state(short),
            Err(StateError::Length { expected: 76, .. })
        ));
        assert!(keys.set_state(&[1, 0, 0]).is_err());
        assert_eq!(keys.table_name, "Grand Piano");
        keys.set_param(13, 1e9);
        assert_eq!(keys.param(13), 65_536.0);
        assert_eq!(keys.param(PARAMS), 0.0);
    }

    #[test]
    fn every_state_fl_ships_is_read() {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, FL Keys states skipped");
            return;
        };
        let mut saved = testing::projects(&root);
        saved.extend(testing::every_preset(&root, NAME));
        let ours: Vec<_> = saved
            .iter()
            .filter(|s| s.plugin.eq_ignore_ascii_case(NAME))
            .collect();
        // 10 in projects, the render test, nine presets and three in the
        // plugin databases.
        assert!(ours.len() >= 23, "{}", ours.len());
        for saved in ours {
            let mut keys = FlKeys::default();
            keys.set_state(&saved.state)
                .unwrap_or_else(|e| panic!("{}: {e}", saved.file));
            let in_range = (0..PARAMS).all(|n| {
                let v = keys.param(n) as i32;
                v >= RANGES[n].0 && v <= RANGES[n].1
            });
            assert!(in_range, "{}", saved.file);
        }
    }

    #[test]
    fn a_note_at_the_root_plays_the_sample_point_for_point() {
        let mut keys = keys_with(sine_spec(100, 40), &PLAIN);
        keys.note_on(note(1, 60));
        let voice = keys.voices[0];
        assert_eq!(
            (voice.delta, voice.envelope, voice.decay),
            (65_536, 1.0, 1.0)
        );
        // Muffle at its top: 50 + 160 = 210, the most.
        assert_eq!(voice.cutoff, 210.0 * 210.0 * keys.inverse_rate);
        assert_eq!(voice.out, [0.2, 0.2]);
        let (left, right) = play(&mut keys, 3000);
        assert_eq!(left, right);
        assert!((period(&left, 100) - 100.0).abs() < 1e-9);
        // The first point is the sample's second, as FL steps before it reads.
        let table = keys.table.as_ref().unwrap();
        let x = table.read(1, 0, 0) as f32;
        let low = voice.cutoff * x;
        assert_eq!(left[0], voice.gains[0] * 0.2 * low);
        assert!((voice.gains[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
    }

    #[test]
    fn a_key_an_octave_up_steps_two_points_and_a_loop_goes_on() {
        let mut keys = keys_with(sine_spec(100, 40), &PLAIN);
        keys.note_on(note(1, 72));
        assert_eq!(keys.voices[0].delta, 131_072);
        // Twice through the sample: the loop holds it.
        let (left, _) = play(&mut keys, 4000);
        assert!((period(&left, 100) - 50.0).abs() < 1e-9);
        assert!(left[3900..].iter().any(|x| x.abs() > 0.01));
        // At 48 kHz the step is the rates' ratio.
        let mut keys = keys_with(sine_spec(100, 40), &PLAIN);
        keys.set_sample_rate(48_000.0);
        keys.note_on(note(1, 60));
        let step = f64::from(44_100.0f32 * (1.0 / 48_000.0f64) as f32);
        assert_eq!(
            keys.voices[0].delta,
            round(f64::from((step) as f32) * 65_536.0)
        );
    }

    #[test]
    fn a_sample_that_does_not_loop_ends_its_voice() {
        let mut spec = sine_spec(100, 10);
        spec.samples[0].can_loop = false;
        let mut keys = keys_with(spec, &PLAIN);
        keys.note_on(note(1, 60));
        let (left, _) = play(&mut keys, 1200);
        assert!(left[1000..].iter().all(|x| *x == 0.0));
        assert!(keys.voices.is_empty());
        assert_eq!(keys.table.as_ref().unwrap().zones[0].looped, 999);
    }

    #[test]
    fn decay_and_release_are_mda_s_rates() {
        let mut values = PLAIN;
        values[0] = 0; // p 0.5
        values[1] = 0;
        let mut keys = keys_with(sine_spec(100, 40), &values);
        keys.note_on(note(1, 60));
        // l = 1 at p 0.5: exp(-iFs exp(0.033 * 60 - 0.6 - 1)).
        let decay = (-(1.0f64 / 44_100.0) * (60.0f64 * 0.033 - 0.6 - 1.0).exp()).exp();
        assert!((f64::from(keys.voices[0].decay) - decay).abs() < 1e-7);
        play(&mut keys, 1000);
        let expected = f64::from(keys.voices[0].decay).powi(1000);
        assert!((f64::from(keys.voices[0].envelope) - expected).abs() < 1e-4);
        keys.note_off(1);
        let release = (-(1.0f64 / 44_100.0) * (60.0f64 * 0.017 + 2.0 - 1.0).exp()).exp();
        assert!((f64::from(keys.voices[0].decay) - release).abs() < 1e-7);
        // High keys take the ePiano's release unless the table says.
        keys.note_on(note(2, 100));
        keys.note_off(2);
        let high = (-(1.0f64 / 44_100.0) * (100.0f64 * 0.01 + 6.0 - 2.5).exp()).exp();
        assert!((f64::from(keys.voices[1].decay) - high).abs() < 1e-7);
        // The voice ends under 1e-4, within the tail.
        let tail = keys.tail();
        play(&mut keys, tail);
        assert!(keys.voices.is_empty(), "{tail}");
    }

    #[test]
    fn velocity_layers_and_key_groups_are_picked_as_fl_picks_them() {
        let mut spec = sine_spec(100, 40);
        spec.layers = 3;
        spec.thresholds = vec![0, 48, 80];
        let zone = spec.samples[0].clone();
        spec.samples = (0..6)
            .map(|n| SampleSpec {
                root: if n < 3 { 48 } else { 72 },
                high: if n < 3 { 59 } else { 999 },
                ..zone.clone()
            })
            .collect();
        spec.samples[1].pos = 10;
        spec.samples[2].pos = 20;
        spec.samples[5].pos = 50;
        let mut keys = keys_with(spec, &PLAIN);
        let at = |keys: &FlKeys, key: i16, velocity: f32| {
            let table = keys.table.as_ref().unwrap();
            let mut note = note(1, key);
            note.velocity = velocity;
            keys.start(table, &note).pos
        };
        // Velocity 100 is the third layer, 49 the second and 48 the first.
        assert_eq!(at(&keys, 59, 1.0), 20);
        let gain_of = |v: f64| ((11f64.powf(v / 100.0) - 1.0) / 10.0) as f32;
        assert_eq!(at(&keys, 59, gain_of(49.0)), 10);
        assert_eq!(at(&keys, 59, gain_of(48.0)), 0);
        assert_eq!(at(&keys, 60, 1.0), 50);
        // Hardness moves the groups: at its top, 6 keys up.
        keys.set_param(2, 1000.0);
        assert_eq!(at(&keys, 60, gain_of(30.0)), 0);
    }

    #[test]
    fn a_thirty_third_voice_takes_the_place_of_the_quietest() {
        let mut values = PLAIN;
        values[0] = 0;
        let mut keys = keys_with(sine_spec(100, 40), &values);
        for n in 0..VOICES as u32 {
            keys.note_on(note(n, 40 + n as i16));
            play(&mut keys, 64);
        }
        assert_eq!(keys.voices.len(), VOICES);
        keys.note_on(note(99, 60));
        assert_eq!(keys.voices.len(), VOICES);
        // The first is the quietest, its envelope having fallen longest.
        assert!(keys.voices.iter().all(|v| v.id != 0));
        assert_eq!(keys.voices.last().unwrap().id, 99);
    }

    #[test]
    fn the_comb_is_127_points_and_takes_the_stereo_from_the_sum() {
        let mut keys = keys_with(sine_spec(100, 40), &PLAIN);
        keys.set_param(7, 2000.0);
        let mut mix = vec![0.0f32; 2 * 200];
        mix[2 * 5] = 1.0;
        keys.after(&mut mix);
        assert_eq!((mix[10], mix[11]), (1.0, 0.0));
        let echoes: Vec<usize> = (0..200).filter(|&n| n != 5 && mix[2 * n] != 0.0).collect();
        assert_eq!(echoes, [132]);
        assert_eq!((mix[2 * 132], mix[2 * 132 + 1]), (1.0, -1.0));
    }

    #[test]
    fn overdrive_bends_what_is_over_0_and_nothing_sounds_without_a_table() {
        let mut values = PLAIN;
        values[14] = 1000;
        let mut keys = keys_with(sine_spec(100, 40), &values);
        keys.note_on(note(1, 60));
        let (driven, _) = play(&mut keys, 1000);
        let mut clean = keys_with(sine_spec(100, 40), &PLAIN);
        clean.note_on(note(1, 60));
        let (plain, _) = play(&mut clean, 1000);
        let peak = |s: &[f32]| s.iter().fold(0.0f32, |m, x| m.max(*x));
        let trough = |s: &[f32]| s.iter().fold(0.0f32, |m, x| m.min(*x));
        assert!(peak(&driven) < peak(&plain) * 0.7);
        assert!((trough(&driven) - trough(&plain)).abs() < 1e-6);
        let mut none = FlKeys::default();
        none.note_on(note(1, 60));
        assert!(play(&mut none, 64).0.iter().all(|x| *x == 0.0));
        assert_eq!(
            none.fidelity(),
            Fidelity::Approximate("FL Keys wavetable not available")
        );
        assert!(crate::generator_by_name("fl keys").is_some());
        assert!(crate::by_name(NAME).is_none());
    }

    /// FL's own tables, where `FL_RS_CORPUS` names an install's `Data`.
    fn with_fl_s(state_bytes: &[u8]) -> Option<FlKeys> {
        let root = testing::corpus()?;
        let mut keys = FlKeys::default();
        keys.set_sample_rate(48_000.0);
        keys.set_resource_dir(&root.join("../Plugins/Fruity/Generators"));
        keys.set_state(state_bytes).unwrap();
        Some(keys)
    }

    #[test]
    fn fl_s_tables_are_read_and_mda_piano_plays_a_held_note() {
        let values = [0, 0, 0, -1000, 784, 502, -248, 1000, 0, 123, 0, 0, 0, 0, 0];
        let Some(mut keys) = with_fl_s(&state("Piano", &values)) else {
            eprintln!("FL_RS_CORPUS unset, FL Keys's tables skipped");
            return;
        };
        let names: Vec<&str> = keys.specs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names[0], "mda Piano", "{names:?}");
        for name in ["Grand Piano", "Rhodes (mda ePiano)", "Roto Organ"] {
            assert!(names.contains(&name), "{names:?}");
        }
        // "Piano" is no table's name, so the default plays.
        let table = keys.table.as_ref().unwrap();
        assert_eq!(
            (table.zones.len(), table.rate, table.velocity_factor),
            (15, 22_050.0, 3.0)
        );
        assert_eq!(
            table.zones[14],
            Zone {
                root: 93,
                high: 999,
                pos: 574_123,
                end: 586_343,
                looped: 2399
            }
        );
        assert!(keys.sample_paths().is_empty());
        keys.note_on(note(1, 57));
        let (left, right) = play(&mut keys, 48_000);
        let peak = left
            .iter()
            .chain(&right)
            .fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(peak > 0.05 && peak < 2.0, "{peak}");
        assert!(left.iter().chain(&right).all(|x| x.is_finite()));
        // A2 is 110 Hz: 436 points at 48 kHz, detuned by 0.012 of a key.
        let sound = &left[24_000..];
        let like = |lag: usize| {
            let (a, b) = (&sound[..sound.len() - lag], &sound[lag..]);
            let dot = |x: &[f32], y: &[f32]| -> f64 {
                x.iter().zip(y).map(|(x, y)| f64::from(x * y)).sum()
            };
            dot(a, b) / (dot(a, a) * dot(b, b)).sqrt()
        };
        let best = (400..480)
            .max_by(|a, b| like(*a).total_cmp(&like(*b)))
            .unwrap();
        assert!((435..=437).contains(&best), "{best}");
        // Rhodes has three velocity layers.
        let rhodes = keys
            .specs
            .iter()
            .find(|s| s.name.starts_with("Rhodes"))
            .unwrap();
        assert_eq!(
            (rhodes.layers, rhodes.thresholds.as_slice()),
            (3, &[0, 48, 80][..])
        );
    }

    #[test]
    fn grand_piano_asks_the_host_for_its_files_and_lays_them_out_as_fl() {
        let values = [
            -1000, -164, 0, -1000, 784, 502, -248, 0, 0, 123, 0, 0, 0, 0, 0,
        ];
        let Some(mut keys) = with_fl_s(&state("Grand Piano", &values)) else {
            eprintln!("FL_RS_CORPUS unset, FL Keys's Grand Piano skipped");
            return;
        };
        let paths = keys.sample_paths();
        assert_eq!(paths.len(), 30);
        assert!(paths.iter().all(|p| Path::new(p).is_file()), "{paths:?}");
        assert!(keys.table.is_none());
        assert!(matches!(keys.fidelity(), Fidelity::Approximate(_)));
        // Stand-ins of 1000 + n frames, stereo.
        for (n, path) in paths.iter().enumerate() {
            let left = testing::sine(0.01, 1000 + n);
            let right: Vec<f32> = left.iter().map(|x| -x).collect();
            let file = SampleFile {
                left: &left,
                right: Some(&right),
                rate: 44_100,
            };
            keys.set_sample(path, Some(file));
        }
        let table = keys.table.as_ref().unwrap();
        assert!(table.stereo);
        // Each file has a slot more than its frames, and plays to its last.
        assert_eq!(
            table.zones[0],
            Zone {
                root: 21,
                high: 23,
                pos: 0,
                end: 999,
                looped: 999
            }
        );
        assert_eq!(
            table.zones[1],
            Zone {
                root: 24,
                high: 26,
                pos: 1001,
                end: 2001,
                looped: 2001
            }
        );
        keys.note_on(note(1, 60));
        let (left, right) = play(&mut keys, 2000);
        assert!(left.iter().any(|x| x.abs() > 0.01) && left != right);
        // The note runs out with its file.
        assert!(keys.voices.is_empty());
    }
}
