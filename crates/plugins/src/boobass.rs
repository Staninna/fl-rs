//! BooBass: a sampled bass guitar of one note at a time, as
//! `docs/plugins/boobass.md` and the DLL's own listings have it.
//!
//! The state is 16 bytes: a version, which FL writes as 1, and the three
//! parameters as little-endian i32.
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Bass | 0 to 65535 | 32767 |
//! | 1 Mid | 0 to 65535 | 32767 |
//! | 2 Treble | 0 to 65535 | 32767 |
//!
//! # The samples
//!
//! The sound is twelve runs of 16-bit PCM in the `.data` of
//! `BooBass_Eng_x64.dll`: for each of six zones a plucked attack recorded
//! at 16 kHz and one turn of the string after it. They are FL's and not
//! in this repository. [`Generator::set_resource_dir`] reads them out of
//! the DLL of an install, and [`Samples::from_dll`] says where they are.
//! Without them the plugin is silent and [`Generator::fidelity`] says so.
//!
//! # A note
//!
//! A note's pitch is 100 cents a key from key 60, and 2400 on top of it
//! are its cents: 0 cents is 16.35 Hz, so key 60 plays 65.4 Hz. The doc
//! has not read which key FL gives a pitch of 0. Fruit Kick's reasons for
//! key 60 hold here. The zone is the one whose root is nearest, of
//! [`ROOTS`]. A new note cuts the one before it.
//!
//! Every sample:
//!
//! 1. The loop, 16384 points a turn, is read at a phase that moves by
//!    `2^(cents / 1200) * 267955.84 / rate`.
//! 2. While the attack lasts, it is read at
//!    `2^((cents - root) / 1200) * 16000 / rate` samples a sample and
//!    fades into the loop in a straight line over its whole length.
//! 3. The envelope's level is looked up in 48 steps of 1 dB and
//!    multiplied in. It starts at 1, falls to 0.9375 in four turns of the
//!    note, then to 0 in 16 times the attack's length, and goes to 0 in a
//!    55th of a second from the note off on.
//! 4. Two one-pole lowpasses split the sound into three bands, which the
//!    three knobs' gains of [`gains`] go on. The same split is made of the
//!    result, its lowest band is clipped at 1 and bent by a sine by as
//!    much as the Bass knob drives it, and the gains go on once more.
//! 5. Three times that is what the output goes to, by no more than
//!    `(p + 0.0328) * 44100 / rate` a sample, where `p` is how far the
//!    attack has got and 1 after it.
//! 6. Four one-pole lowpasses in a row smooth the result.
//!
//! What is not FL's:
//!
//! - The note's velocity is its gain and its pan is [`dsp::pan_linear`],
//!   the pan the engine's curves have, until FL's `ComputeLRVol` is read.
//! - FL works a sounding note's steps out again at the top of every
//!   block. Here that is done when the host moves the note's pitch
//!   ([`Generator::set_note_pitch`]), from the next block on, which is
//!   the same.
//! - FL's high quality mode, which reads the loop through a box filter,
//!   is not built.
//! - FL ends a note with the block its envelope ends in, and here it ends
//!   on the sample.
//! - Under key 36 FL reads the note's decay from before its table. Here
//!   the table's formula goes on down.
//!
//! Where this differs from the doc, the listings under
//! `ghidra/out/boobasseng` have it this way: the saturator is
//! `sin(x * pi / 2)` and not a `tanh`, the slew limit follows the attack
//! and not the pitch, the level table has 48 steps to its top and not
//! 47, the loop is read between the last point and this one, the trims
//! of the doc are the attacks' while the loops have their own and are
//! turned over, and the second pointer of each sample points at its
//! length in bytes.

use crate::{Fidelity, Generator, NoteOn, StateError, knob_value, state_version};
use dsp::resample::read_hermite;
use std::f64::consts::FRAC_PI_2;
use std::path::Path;

/// The plugin's event 201 name.
pub const NAME: &str = "BooBass";

/// Where the samples are, from the directory
/// [`Generator::set_resource_dir`] is handed.
pub const DLL: &str = "BooBass/BooBass_Eng_x64.dll";

/// The pitch each zone's samples were recorded at, in cents over 16.35 Hz:
/// B0, E1, B1, E2, B2 and E3.
pub const ROOTS: [i32; 6] = [1100, 1600, 2300, 2800, 3500, 4000];

/// The range of each of the three wheels.
const RANGE: (i32, i32) = (0, 65_535);
const DEFAULTS: [i32; 3] = [32_767; 3];

/// How many dB each zone's attack is turned down by, and its loop.
const ATTACK_TRIMS: [f32; 6] = [0.666, -0.333, -0.75, -1.5, -2.0, -3.0];
const LOOP_TRIMS: [f32; 6] = [4.0, 3.5, 3.0, 3.25, 3.5, 3.0];

/// The points one turn of a loop is made into.
const TABLE: usize = 16_384;

/// The highest a note goes, in cents.
const TOP: i32 = 9600;

/// The level the envelope falls to in the first four turns of a note.
const HELD: f32 = 0.9375;

/// FL takes a sample nearer to 0 than this for 0.
const TINY: f32 = 5.960_465e-8;

/// The four smoothing lowpasses: `y = A * (x + x1) + B * y1`.
const SMOOTH_A: f32 = 0.298_08;
const SMOOTH_B: f32 = 0.403_84;

fn tiny(value: f32) -> bool {
    value < TINY && -TINY < value
}

/// The gain of this many dB, as FL works it out: the power in doubles of
/// a float.
fn db(db: f32) -> f32 {
    10f64.powf(f64::from(db / 20.0)) as f32
}

/// What a wheel's value makes of the sound: the gains of the bass, the mid
/// and the treble band, and how far the bass band is driven into its
/// saturator. With `x = value / 65535`:
///
/// ```text
/// bass   dB(12 x - 12)    drive dB(24 x - 24)
/// mid    dB(10 x - 10)
/// treble dB(12 x - 9)
/// ```
///
/// and 0 for all of a wheel under a tenth of its way.
pub fn gains(params: [i32; 3]) -> ([f32; 3], f32) {
    let [bass, mid, treble] = params.map(|value| (f64::from(value) / 65_535.0) as f32);
    let above = |x: f32, db_at: f32| if x < 0.1 { 0.0 } else { db(db_at) };
    (
        [
            above(bass, bass * 12.0 - 12.0),
            above(mid, mid * 10.0 - 10.0),
            above(treble, treble * 12.0 - 9.0),
        ],
        above(bass, bass * 24.0 - 24.0),
    )
}

/// The zone a note of this many cents plays: the highest whose root is not
/// over it, or the one above where that root is nearer.
pub fn zone_of(cents: i32) -> usize {
    let under = ROOTS.iter().rposition(|root| *root <= cents).unwrap_or(0);
    match ROOTS.get(under + 1) {
        Some(above) if (above - cents).abs() < (ROOTS[under] - cents).abs() => under + 1,
        _ => under,
    }
}

/// One zone's two samples, as FL keeps them once it has loaded.
#[derive(Debug, Clone, PartialEq)]
struct Zone {
    /// The attack at 16 kHz, turned down by its trim and by half.
    attack: Vec<f32>,
    /// One turn of the loop in [`TABLE`] points, turned down by its trim
    /// and by half, and turned over.
    wave: Vec<f32>,
}

impl Zone {
    fn new(zone: usize, attack: &[i16], cycle: &[i16]) -> Self {
        let float = |value: &i16| f32::from(*value) / 32_768.0;
        let mut attack: Vec<f32> = attack.iter().map(float).collect();
        // FL keeps four samples of room before the first one and turns
        // down as many floats as there are samples from the room on, so
        // the last four stay as loud as they were.
        let gain = db(-ATTACK_TRIMS[zone]) * 0.5;
        let turned_down = attack.len().saturating_sub(4);
        for value in &mut attack[..turned_down] {
            *value *= gain;
        }

        // The turn with the sample before it and the two after it, which
        // the hermite reads around a point.
        let len = cycle.len();
        let mut around = Vec::with_capacity(len + 3);
        around.push(float(&cycle[len - 1]));
        around.extend(cycle.iter().map(float));
        around.extend([float(&cycle[0]), float(&cycle[1 % len])]);
        let gain = db(-LOOP_TRIMS[zone]) * 0.5;
        let point = |k: usize| {
            let at = k as f64 * len as f64 / TABLE as f64;
            -read_hermite(&around, at + 1.0) * gain
        };
        Zone {
            attack,
            wave: (0..TABLE).map(point).collect(),
        }
    }
}

/// The six attacks and the six loops.
#[derive(Debug, Clone, PartialEq)]
pub struct Samples {
    zones: [Zone; 6],
}

impl Samples {
    /// Where the DLL of FL Studio 2026 has its two tables of six samples
    /// each, from the start of its image: the attacks and the loops. An
    /// entry is two addresses: of the 16-bit PCM, and of an i32 right
    /// after it that is the PCM's length in bytes.
    const ATTACKS: u64 = 0xcb920;
    const LOOPS: u64 = 0xcb980;

    /// From the PCM of each zone's attack and of one turn of its loop.
    /// `None` where an attack is under five samples or a loop is empty.
    pub fn from_pcm(attacks: [&[i16]; 6], loops: [&[i16]; 6]) -> Option<Self> {
        if attacks.iter().any(|pcm| pcm.len() < 5) || loops.iter().any(|pcm| pcm.is_empty()) {
            return None;
        }
        Some(Samples {
            zones: std::array::from_fn(|zone| Zone::new(zone, attacks[zone], loops[zone])),
        })
    }

    /// From the bytes of `BooBass_Eng_x64.dll`. `None` for a file that is
    /// no such DLL, or a build of it that has its tables elsewhere.
    pub fn from_dll(bytes: &[u8]) -> Option<Self> {
        let image = Image::parse(bytes)?;
        let read = |table: u64| -> Option<Vec<Vec<i16>>> {
            (0..6).map(|zone| image.pcm(table + 16 * zone)).collect()
        };
        let (attacks, loops) = (read(Self::ATTACKS)?, read(Self::LOOPS)?);
        Self::from_pcm(
            std::array::from_fn(|zone| attacks[zone].as_slice()),
            std::array::from_fn(|zone| loops[zone].as_slice()),
        )
    }

    /// From the DLL under FL's `Plugins/Fruity/Generators` directory.
    pub fn read(dir: &Path) -> Option<Self> {
        Self::from_dll(&std::fs::read(dir.join(DLL)).ok()?)
    }
}

/// A 64-bit Windows DLL as it is on disk.
struct Image<'a> {
    bytes: &'a [u8],
    /// The address the DLL asks to be loaded at, which the addresses in
    /// its file count from.
    base: u64,
    /// Each section: where it starts in the image, where in the file, and
    /// how many bytes of it the file has.
    sections: Vec<(u64, usize, usize)>,
}

/// `N` bytes of `bytes` from `at` on.
fn bytes_at<const N: usize>(bytes: &[u8], at: usize) -> Option<[u8; N]> {
    bytes.get(at..)?.first_chunk::<N>().copied()
}

impl<'a> Image<'a> {
    fn parse(bytes: &'a [u8]) -> Option<Self> {
        let u16_at = |at: usize| bytes_at(bytes, at).map(|b| usize::from(u16::from_le_bytes(b)));
        let u32_at = |at: usize| bytes_at(bytes, at).map(|b| u32::from_le_bytes(b) as usize);
        let header = u32_at(0x3c)?;
        let optional = header.checked_add(24)?;
        // "PE", and the magic of a 64-bit image.
        if bytes_at(bytes, header)? != *b"PE\0\0" || u16_at(optional)? != 0x20b {
            return None;
        }
        let table = optional + u16_at(header + 20)?;
        let section = |k: usize| {
            let at = table + 40 * k;
            Some((u32_at(at + 12)? as u64, u32_at(at + 20)?, u32_at(at + 16)?))
        };
        Some(Image {
            bytes,
            base: u64::from_le_bytes(bytes_at(bytes, optional + 24)?),
            sections: (0..u16_at(header + 6)?)
                .map(section)
                .collect::<Option<_>>()?,
        })
    }

    /// `len` bytes at an address of the loaded image.
    fn at(&self, address: u64, len: usize) -> Option<&'a [u8]> {
        let place = address.checked_sub(self.base)?;
        self.sections.iter().find_map(|(start, file, size)| {
            let from = usize::try_from(place.checked_sub(*start)?).ok()?;
            let to = from.checked_add(len).filter(|to| to <= size)?;
            self.bytes
                .get(file.checked_add(from)?..file.checked_add(to)?)
        })
    }

    /// The sample of the table entry `entry` bytes into the image.
    fn pcm(&self, entry: u64) -> Option<Vec<i16>> {
        let pair = self.at(self.base.checked_add(entry)?, 16)?;
        let (start, length_at) = (
            u64::from_le_bytes(bytes_at(pair, 0)?),
            u64::from_le_bytes(bytes_at(pair, 8)?),
        );
        let length = i32::from_le_bytes(bytes_at(self.at(length_at, 4)?, 0)?);
        let length = usize::try_from(length).ok()?;
        // The length is right after the sample, on a 4-byte boundary. A
        // table that says otherwise is not the table.
        let room = usize::try_from(length_at.checked_sub(start)?).ok()?;
        if room < length || room - length >= 4 {
            return None;
        }
        let words = self.at(start, length / 2 * 2)?.as_chunks::<2>().0;
        Some(words.iter().map(|word| i16::from_le_bytes(*word)).collect())
    }
}

/// The gain for each 48th of the envelope's level: 0 at 0, then a dB a
/// step up to 1 at 48. One more for the read past the top.
fn level_table() -> [f32; 50] {
    std::array::from_fn(|step| match step {
        0 | 49 => 0.0,
        48 => 1.0,
        _ => db(step as f32 - 48.0),
    })
}

/// The gain of an envelope level, read between two steps of the table.
fn level_gain(table: &[f32; 50], level: f32) -> f32 {
    let at = level.clamp(0.0, 1.0) * 48.0;
    let step = at as usize;
    (table[step + 1] - table[step]) * (at - step as f32) + table[step]
}

/// The envelope of a note: straight lines from 1 to [`HELD`], to 0 and,
/// from the note off on, from where it is to 0.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Envelope {
    /// 0 before the first sample, 1 the fall to [`HELD`], 2 the fall to
    /// 0, 3 the release and [`Envelope::DONE`] after it.
    stage: usize,
    /// The samples left of the stage.
    left: u32,
    /// What a sample adds to the level.
    step: f32,
    level: f32,
    lengths: [u32; 5],
    steps: [f32; 5],
    /// The level each stage ends on.
    targets: [f32; 5],
}

impl Envelope {
    const DONE: usize = 4;

    /// With this many samples for the fall to [`HELD`], for the fall to 0
    /// and for the release. The level is 0 until the first step.
    fn new(decay: u32, fade: u32, release: u32) -> Self {
        let step = |from: f32, to: f32, len: u32| match len {
            0 => 0.0,
            _ => (to - from) / len as f32,
        };
        Envelope {
            stage: 0,
            left: 0,
            step: 0.0,
            level: 0.0,
            lengths: [0, decay, fade, release, u32::MAX],
            steps: [
                0.0,
                step(1.0, HELD, decay),
                step(HELD, 0.0, fade),
                step(0.0, 0.0, release),
                0.0,
            ],
            targets: [1.0, HELD, 0.0, 0.0, 0.0],
        }
    }

    /// One sample on.
    fn advance(&mut self) {
        if self.stage >= Self::DONE {
            return;
        }
        let mut value = self.level;
        while self.left == 0 {
            if self.stage == 2 {
                // The fall to 0 is over: release from where it got to.
                let len = self.lengths[3];
                self.steps[3] = if len == 0 { 0.0 } else { -value / len as f32 };
            } else {
                value = self.targets[self.stage];
            }
            self.stage += 1;
            self.left = self.lengths[self.stage];
            self.step = self.steps[self.stage];
        }
        self.left -= 1;
        self.level = value + self.step;
    }

    /// The note off. A note that has played no sample yet has no level to
    /// release and stays silent.
    fn release(&mut self) {
        if self.stage < 3 {
            self.left = self.lengths[3];
            if self.left != 0 {
                self.step = -(self.level / self.left as f32);
            }
            self.stage = 3;
        }
    }
}

/// The note that sounds.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Note {
    /// The host's number for it.
    id: u32,
    zone: usize,
    /// Where in the loop's table it is, and how far a sample moves it.
    phase: f32,
    loop_step: f32,
    /// The same for the attack, and the attack's last sample.
    attack_at: f32,
    attack_step: f32,
    attack_end: f32,
    envelope: Envelope,
    /// Left and right gain: the note's velocity and its pan.
    gains: (f32, f32),
}

impl Note {
    /// The next sample of the two recordings, and how far the attack has
    /// got: 1 once it is over.
    fn read(&mut self, zone: &Zone) -> (f32, f32) {
        let wave = |at: i32| zone.wave[at as usize & (TABLE - 1)];
        let before = wave(self.phase as i32);
        self.phase += self.loop_step;
        let mut at = self.phase as i32;
        if at >= TABLE as i32 {
            self.phase -= TABLE as f32;
            at -= TABLE as i32;
        }
        let mut value = before + (wave(at) - before) * (self.phase - at as f32);

        let mut progress = 1.0;
        if self.attack_at < self.attack_end {
            let at = self.attack_at as usize;
            let (a, b) = (zone.attack[at], zone.attack[at + 1]);
            let attack = (b - a) * (self.attack_at - at as f32) + a;
            self.attack_at += self.attack_step;
            progress = self.attack_at / self.attack_end;
            value = attack + (value - attack) * progress;
        }
        (value, progress)
    }
}

/// The three bands, the saturator and the slew limit, which keep what
/// they hold from one note to the next.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Tone {
    /// The lowpasses of the first split and of the second: the low one
    /// and the high one of each.
    first: [f32; 2],
    second: [f32; 2],
    out: f32,
}

impl Tone {
    /// One sample through. `k` is what the low and the high lowpass move
    /// by, and `slew` the most the output moves by.
    fn shape(&mut self, value: f32, gains: &[f32; 3], drive: f32, k: [f32; 2], slew: f32) -> f32 {
        let [bass, mid, treble] = *gains;
        let follow = |state: f32, input: f32, k: f32| (input - state) * k + state;
        let low = follow(self.first[0], value, k[0]);
        let high = follow(self.first[1], value, k[1]);
        self.first = [low, high];
        let split = (high - low) * mid + low * bass + (value - high) * treble;

        let mut low = follow(self.second[0], split, k[0]);
        let high = follow(self.second[1], split, k[1]);
        if low.abs() > 1.0 {
            low = low.signum();
        }
        self.second = [low, high];
        let bent = (f64::from(low) * FRAC_PI_2).sin() as f32;
        let driven = (bent - low) * drive + low;
        let wanted = (driven * bass + (high - low) * mid + (split - high) * treble) * 3.0;

        let step = wanted - self.out;
        self.out += if -slew > step { -slew } else { slew.min(step) };
        self.out
    }
}

/// BooBass. A new one is at 44.1 kHz with FL's defaults and no samples.
#[derive(Debug, Clone, PartialEq)]
pub struct BooBass {
    params: [i32; 3],
    rate: f32,
    samples: Option<Samples>,
    levels: [f32; 50],
    /// The gains the knobs ask for, and how far the bass is driven.
    gains: [f32; 3],
    drive: f32,
    /// The gains that are on the sound, which follow the knobs.
    smooth: [f32; 3],
    note: Option<Note>,
    tone: Tone,
    /// The input and the output of each smoothing lowpass, a sample ago.
    poles: [[f32; 2]; 4],
}

impl Default for BooBass {
    fn default() -> Self {
        let (gains, drive) = gains(DEFAULTS);
        BooBass {
            params: DEFAULTS,
            rate: 44_100.0,
            samples: None,
            levels: level_table(),
            gains,
            drive,
            smooth: gains,
            note: None,
            tone: Tone::default(),
            poles: [[0.0; 2]; 4],
        }
    }
}

impl BooBass {
    /// How far a sample moves the loop and the attack of a note of this
    /// many cents in `zone` (`FUN_180003f90`).
    fn steps(&self, zone: usize, cents: i32) -> (f32, f32) {
        let from_root = cents - ROOTS[zone];
        let attack_ratio = match from_root {
            1.. => up(from_root) as f32,
            0 => 1.0,
            _ => 1.0 / up(-from_root) as f32,
        };
        let loop_step = up(cents) as f32 * (267_955.84 / self.rate);
        (loop_step, attack_ratio * (16_000.0 / self.rate))
    }

    /// Play these samples.
    pub fn set_samples(&mut self, samples: Samples) {
        self.samples = Some(samples);
        self.note = None;
    }

    /// The note of this many cents, on its first sample.
    fn start(&self, id: u32, cents: i32, gains: (f32, f32), samples: &Samples) -> Note {
        let cents = cents.min(TOP);
        let zone = zone_of(cents);
        let len = samples.zones[zone].attack.len();
        // Four turns of the note.
        let decay = self.rate / (up(cents) * 4.088_681_640_625) as f32;
        let fade = (len << 4) as f32 * (self.rate / 44_100.0);
        let (loop_step, attack_step) = self.steps(zone, cents);
        Note {
            id,
            zone,
            phase: 0.0,
            loop_step,
            attack_at: 0.0,
            attack_step,
            attack_end: (len - 1) as f32,
            envelope: Envelope::new(decay as u32, fade as u32, (self.rate / 55.0) as u32),
            gains,
        }
    }
}

/// 2 to the power of `cents` over 1200.
fn up(cents: i32) -> f64 {
    (f64::from(cents) / 1200.0).exp2()
}

/// What a block of `len` samples adds to a gain that is `delta` from its
/// knob, every 16 samples.
fn ramp_step(delta: f32, len: usize) -> f32 {
    let step = delta / len as f32;
    if tiny(delta) || tiny(step) {
        0.0
    } else {
        step * 16.0
    }
}

impl Generator for BooBass {
    /// The gains are the state's from the next sample on.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let version = state_version(bytes, 16)?;
        if version >= 2 {
            return Err(StateError::Version { found: version });
        }
        if bytes.len() != 16 {
            return Err(StateError::Length {
                expected: 16,
                found: bytes.len(),
            });
        }
        let mut params = [0; 3];
        for (param, word) in bytes[4..].as_chunks::<4>().0.iter().enumerate() {
            let value = i32::from_le_bytes(*word);
            if value < RANGE.0 || value > RANGE.1 {
                return Err(StateError::Range { param, value });
            }
            params[param] = value;
        }
        self.params = params;
        (self.gains, self.drive) = gains(params);
        self.smooth = self.gains;
        Ok(())
    }

    /// Cuts the note, as FL's change of rate does.
    fn set_sample_rate(&mut self, hz: f32) {
        self.rate = hz;
        self.reset();
    }

    /// Reads the samples out of FL's DLL, and keeps the ones it has where
    /// the directory has none.
    fn set_resource_dir(&mut self, dir: &Path) {
        if let Some(samples) = Samples::read(dir) {
            self.set_samples(samples);
        }
    }

    /// Cuts the note that sounds. Nothing happens without the samples.
    fn note_on(&mut self, note: NoteOn) {
        let Some(samples) = &self.samples else {
            return;
        };
        let pitch = (f32::from(note.key) - 60.0) * 100.0 + note.fine_pitch;
        let (left, right) = dsp::pan_linear(note.pan);
        let gains = (note.velocity * left, note.velocity * right);
        self.note = Some(self.start(note.voice, (pitch + 2400.0) as i32, gains, samples));
        self.smooth = self.gains;
    }

    fn note_off(&mut self, voice: u32) {
        if let Some(note) = self.note.as_mut().filter(|note| note.id == voice) {
            note.envelope.release();
        }
    }

    fn all_notes_off(&mut self) {
        if let Some(note) = &mut self.note {
            note.envelope.release();
        }
    }

    /// `Gen_Render` works the steps out again from `FinalLevels.Pitch` at
    /// the top of every block (`FUN_180003f90`), in the zone the note
    /// started in.
    fn set_note_pitch(&mut self, voice: u32, pitch: f32) {
        let Some(note) = self.note.filter(|note| note.id == voice) else {
            return;
        };
        let cents = ((pitch + 2400.0) as i32).min(TOP);
        let (loop_step, attack_step) = self.steps(note.zone, cents);
        self.note = Some(Note {
            loop_step,
            attack_step,
            ..note
        });
    }

    /// A knob that has moved pulls its gain along in steps of 16 samples,
    /// most of the way in one block.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let (Some(samples), Some(note)) = (&self.samples, &mut self.note) else {
            return;
        };
        let zone = &samples.zones[note.zone];
        let len = left.len().min(right.len());
        let per = 44_100.0 / self.rate;
        let k = [per * 0.025, per * 0.155];
        let steps: [f32; 3] =
            std::array::from_fn(|band| ramp_step(self.gains[band] - self.smooth[band], len));
        let mut done = false;
        for (n, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
            let (mut value, progress) = note.read(zone);
            value *= level_gain(&self.levels, note.envelope.level);
            note.envelope.advance();
            if tiny(value) {
                value = 0.0;
            }
            let slew = (progress + 0.032_812_502) * per;
            value = self.tone.shape(value, &self.smooth, self.drive, k, slew);
            if (len - n) & 0xf == 0 {
                for (gain, step) in self.smooth.iter_mut().zip(steps) {
                    *gain += step;
                }
            }

            if tiny(value) {
                value = 0.0;
            }
            for pole in &mut self.poles {
                let out = SMOOTH_A * value + SMOOTH_A * pole[0] + SMOOTH_B * pole[1];
                *pole = [value, out];
                value = out;
            }
            *l += value * note.gains.0;
            *r += value * note.gains.1;
            if note.envelope.stage == Envelope::DONE {
                done = true;
                break;
            }
        }
        if done {
            self.note = None;
        }
    }

    fn param_count(&self) -> usize {
        self.params.len()
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |value| *value as f32)
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(param) = self.params.get_mut(index) {
            *param = knob_value(value, RANGE);
            (self.gains, self.drive) = gains(self.params);
        }
    }

    fn reset(&mut self) {
        self.note = None;
        self.tone = Tone::default();
        self.poles = [[0.0; 2]; 4];
        self.smooth = self.gains;
    }

    /// The release: a 55th of a second.
    fn tail(&self) -> usize {
        (self.rate / 55.0) as usize + 2
    }

    fn fidelity(&self) -> Fidelity {
        match self.samples {
            Some(_) => Fidelity::Exact,
            None => Fidelity::Approximate("BooBass samples not available"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, partial, state};
    use std::f64::consts::TAU;

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

    /// `len` samples of a sine of this period and peak.
    fn pcm_sine(len: usize, period: f64, peak: f64) -> Vec<i16> {
        (0..len)
            .map(|n| ((TAU * n as f64 / period).sin() * peak) as i16)
            .collect()
    }

    /// Samples that are not FL's: for every zone an attack of `attack`
    /// samples of a sine at the zone's root, and one turn of a sine.
    fn sines(attack: usize) -> Samples {
        let period =
            |zone: usize| 16_000.0 / (16.3547 * 2f64.powf(f64::from(ROOTS[zone]) / 1200.0));
        let attacks: Vec<Vec<i16>> = (0..6)
            .map(|zone| pcm_sine(attack, period(zone), 16_000.0))
            .collect();
        let loops: Vec<Vec<i16>> = (0..6)
            .map(|zone| pcm_sine(period(zone) as usize, period(zone).floor(), 32_000.0))
            .collect();
        Samples::from_pcm(
            std::array::from_fn(|zone| attacks[zone].as_slice()),
            std::array::from_fn(|zone| loops[zone].as_slice()),
        )
        .unwrap()
    }

    fn with_sines() -> BooBass {
        let mut bass = BooBass::default();
        bass.set_samples(sines(4000));
        bass
    }

    /// FL's own samples, where `FL_RS_CORPUS` names an install's `Data`.
    fn with_fl_s() -> Option<BooBass> {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, BooBass's samples skipped");
            return None;
        };
        let mut bass = BooBass::default();
        bass.set_resource_dir(&root.join("../Plugins/Fruity/Generators"));
        assert_eq!(bass.fidelity(), Fidelity::Exact, "no {DLL}");
        Some(bass)
    }

    fn play(bass: &mut BooBass, len: usize) -> Vec<f32> {
        let (mut left, mut right) = (vec![0.0; len], vec![0.0; len]);
        bass.process(&mut left, &mut right);
        assert_eq!(left, right);
        left
    }

    /// The period of a sound in samples: the lag within a tenth of
    /// `around` at which it is most like itself, and how like.
    fn period(sound: &[f32], around: f64) -> (usize, f64) {
        let lags = (around * 0.9) as usize..=(around * 1.1) as usize;
        let like = |lag: usize| {
            let (a, b) = (&sound[..sound.len() - lag], &sound[lag..]);
            let dot = |x: &[f32], y: &[f32]| -> f64 {
                x.iter().zip(y).map(|(x, y)| f64::from(x * y)).sum()
            };
            dot(a, b) / (dot(a, a) * dot(b, b)).sqrt()
        };
        lags.map(|lag| (lag, like(lag)))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap()
    }

    fn db_of(gain: f32) -> f32 {
        20.0 * gain.log10()
    }

    #[test]
    fn a_note_follows_the_pitch_the_host_moves_it_to() {
        let samples = sines(2000);
        testing::follows_note_pitch(
            || {
                let mut bass = BooBass::default();
                bass.set_samples(samples.clone());
                Box::new(bass)
            },
            44_100.0,
        );
    }

    #[test]
    fn the_state_is_a_version_and_the_three_wheels() {
        let mut bass = BooBass::default();
        assert_eq!(bass.params, [32_767; 3]);
        bass.set_state(&state(&[1, 65_535, 0, 12])).unwrap();
        assert_eq!(bass.params, [65_535, 0, 12]);
        assert_eq!((bass.gains, bass.drive), gains([65_535, 0, 12]));
        assert_eq!(bass.smooth, bass.gains);
        let before = bass.clone();
        // FL reads any version under 2.
        bass.set_state(&state(&[0, 1, 2, 3])).unwrap();
        assert_eq!(bass.params, [1, 2, 3]);
        let mut bass = before.clone();
        for version in [2, i32::MAX] {
            assert_eq!(
                bass.set_state(&state(&[version, 0, 0, 0])),
                Err(StateError::Version { found: version })
            );
        }
        let length = |found| {
            Err(StateError::Length {
                expected: 16,
                found,
            })
        };
        assert_eq!(bass.set_state(&[1, 0]), length(2));
        assert_eq!(bass.set_state(&state(&[1, 0, 0])), length(12));
        assert_eq!(bass.set_state(&state(&[1, 0, 0, 0, 0])), length(20));
        for (param, value) in [(0, -1), (1, 65_536), (2, i32::MIN)] {
            let mut words = [1, 0, 0, 0];
            words[param + 1] = value;
            assert_eq!(
                bass.set_state(&state(&words)),
                Err(StateError::Range { param, value })
            );
        }
        assert_eq!(bass, before);

        bass.set_param(0, 1e9);
        bass.set_param(1, -4.0);
        bass.set_param(2, 100.4);
        bass.set_param(3, 1.0);
        assert_eq!(bass.params, [65_535, 0, 100]);
        assert_eq!((bass.param(2), bass.param(3)), (100.0, 0.0));
        // A moved knob leaves the gain that is on the sound to the ramp.
        assert_eq!(bass.gains, gains(bass.params).0);
        assert_eq!(bass.smooth, before.smooth);
    }

    #[test]
    fn the_wheels_are_the_doc_s_db_and_the_default_is_not_flat() {
        let close = |gain: f32, db: f32| (db_of(gain) - db).abs() < 0.001;
        // Half way: -6, -5 and -3 dB, and the drive at -12.
        let ([bass, mid, treble], drive) = gains(DEFAULTS);
        assert!(close(bass, -6.0) && close(mid, -5.0) && close(treble, -3.0));
        assert!(close(drive, -12.0));
        // All the way: 0, 0 and +3 dB.
        let ([bass, mid, treble], drive) = gains([65_535; 3]);
        assert_eq!((bass, mid, drive), (1.0, 1.0, 1.0));
        assert!(close(treble, 3.0));
        // A tenth of the way is the lowest that sounds.
        assert_eq!(gains([6553; 3]), ([0.0; 3], 0.0));
        let ([bass, mid, treble], drive) = gains([6554; 3]);
        assert!(close(bass, -10.8) && close(mid, -9.0) && close(treble, -7.8));
        assert!(close(drive, -21.6));
        // The treble is flat at three quarters.
        assert!(close(gains([0, 0, 49_151]).0[2], 0.0));
    }

    /// What the two lowpasses of a split let through of a tone at `turns`
    /// a sample, as complex numbers.
    fn lowpasses(k: [f32; 2], turns: f64) -> [(f64, f64); 2] {
        k.map(|k| {
            // k / (1 - (1 - k) z^-1)
            let (k, angle) = (f64::from(k), TAU * turns);
            let (re, im) = (1.0 - (1.0 - k) * angle.cos(), (1.0 - k) * angle.sin());
            let size = re * re + im * im;
            (k * re / size, -k * im / size)
        })
    }

    /// The gain of the whole tone path for a quiet tone, from the doc's
    /// maths: the split with the gains on it, the same split with the
    /// bass through the start of the sine, and the 3.
    fn tone_gain(gains: [f32; 3], drive: f32, k: [f32; 2], turns: f64) -> f64 {
        let [bass, mid, treble] = gains.map(f64::from);
        let [low, high] = lowpasses(k, turns);
        let split = |bass: f64| {
            (
                bass * low.0 + mid * (high.0 - low.0) + treble * (1.0 - high.0),
                bass * low.1 + mid * (high.1 - low.1) - treble * high.1,
            )
        };
        let (first, second) = (
            split(bass),
            split(bass * (1.0 + f64::from(drive) * (FRAC_PI_2 - 1.0))),
        );
        3.0 * first.0.hypot(first.1) * second.0.hypot(second.1)
    }

    /// The gain the tone path has for a quiet tone, by playing one.
    fn measured(gains: [f32; 3], drive: f32, turns: f64) -> f64 {
        let k = [0.025, 0.155];
        let mut tone = Tone::default();
        let input = testing::sine(turns, 8000);
        let output: Vec<f32> = input
            .iter()
            .map(|value| tone.shape(value * 1e-3, &gains, drive, k, 1.0328))
            .collect();
        partial(&output[4000..], turns) / 1e-3
    }

    #[test]
    fn the_three_bands_add_up_to_the_sound_and_the_default_leans_to_the_treble() {
        // With one gain on all three and no drive the split is undone:
        // three times the gain twice, whatever the pitch.
        for turns in [0.001, 0.01, 0.1, 0.25, 0.45] {
            let flat = measured([0.5; 3], 0.0, turns);
            assert!((flat - 0.75).abs() < 1e-4, "{flat} at {turns}");
        }
        let mut tone = Tone::default();
        for value in [0.3, -0.2, 0.25, 0.0, -0.31] {
            let out = tone.shape(value, &[1.0; 3], 0.0, [0.025, 0.155], 9.0);
            assert!((out - 3.0 * value).abs() < 1e-6);
        }

        // The knobs at their default against the doc's maths, for a tone
        // in each band. It is not flat: the bass is 9 dB under the top.
        let (gains, drive) = gains(DEFAULTS);
        let mut found = Vec::new();
        for turns in [0.0005, 0.0125, 0.4] {
            let (got, want) = (
                measured(gains, drive, turns),
                tone_gain(gains, drive, [0.025, 0.155], turns),
            );
            assert!(
                (got / want - 1.0).abs() < 0.01,
                "{got} for {want} at {turns}"
            );
            found.push(20.0 * got.log10());
        }
        // 3 * 0.5012^2 * (1 + 0.2512 * 0.5708) is -1.3 dB at the very
        // bottom and 3 * 0.7079^2 is 3.5 dB at the very top. The three
        // tones are 22, 551 and 17640 Hz at 44.1 kHz.
        let want = [-1.28, 0.28, 3.23];
        for (found, want) in found.iter().zip(want) {
            assert!((found - want).abs() < 0.02, "{found} dB for {want}");
        }
    }

    #[test]
    fn the_bass_is_clipped_and_bent_by_a_sine_as_far_as_it_is_driven() {
        // A level that stays, with the bass band alone: both lowpasses of
        // both splits end on it.
        let settle = |level: f32, drive: f32| {
            let mut tone = Tone::default();
            let mut out = 0.0;
            for _ in 0..2000 {
                out = tone.shape(level, &[1.0, 0.0, 0.0], drive, [0.025, 0.155], 9.0);
            }
            out
        };
        assert!((settle(0.5, 0.0) - 1.5).abs() < 1e-5);
        // sin(pi / 4) in place of 0.5, all of it and a quarter of it.
        let bent = std::f32::consts::FRAC_1_SQRT_2;
        assert!((settle(0.5, 1.0) - 3.0 * bent).abs() < 1e-5);
        assert!((settle(0.5, 0.25) - 3.0 * (0.5 + 0.25 * (bent - 0.5))).abs() < 1e-5);
        assert!((settle(-0.5, 1.0) + 3.0 * bent).abs() < 1e-5);
        // Over 1 the bass band stays at 1, where the sine is 1 too.
        assert!((settle(4.0, 1.0) - 3.0).abs() < 1e-5);
        assert!((settle(-4.0, 0.0) + 3.0).abs() < 1e-5);
    }

    #[test]
    fn the_output_moves_no_faster_than_the_slew_limit() {
        let mut tone = Tone::default();
        let k = [0.025, 0.155];
        // A step up to 3: 0.25 a sample, and then it is there.
        let mut last = 0.0;
        let mut climbed = 0;
        for _ in 0..400 {
            let out = tone.shape(1.0, &[1.0; 3], 0.0, k, 0.25);
            assert!(out - last <= 0.25 + 1e-6);
            climbed += usize::from((out - last - 0.25).abs() < 1e-6);
            last = out;
        }
        assert!(
            climbed >= 11 && (last - 3.0).abs() < 1e-3,
            "{climbed} {last}"
        );
        // And down as slowly.
        let out = tone.shape(-1.0, &[1.0; 3], 0.0, k, 0.25);
        assert!((last - out - 0.25).abs() < 1e-6);

        // In a note the limit opens as the attack goes, from 0.0328 at
        // 44.1 kHz. An attack that jumps between two levels is held to it.
        let square: Vec<i16> = (0..400)
            .map(|n| if n / 8 % 2 == 0 { 30_000 } else { -30_000 })
            .collect();
        let turn = [0i16, 100, 0, -100];
        let mut bass = BooBass::default();
        bass.set_samples(Samples::from_pcm([&square[..]; 6], [&turn[..]; 6]).unwrap());
        bass.set_state(&state(&[1, 65_535, 65_535, 65_535]))
            .unwrap();
        bass.note_on(note(1, 47));
        let (mut last, mut held) = (0.0, 0);
        for n in 0..300 {
            play(&mut bass, 1);
            let playing = bass.note.unwrap();
            let progress = playing.attack_at / playing.attack_end;
            assert!((progress - (n + 1) as f32 * 16_000.0 / 44_100.0 / 399.0).abs() < 1e-4);
            let (limit, moved) = (progress + 0.032_812_5, (bass.tone.out - last).abs());
            assert!(moved <= limit + 1e-6, "{moved} over {limit} at {n}");
            held += usize::from((moved - limit).abs() < 1e-6);
            last = bass.tone.out;
        }
        assert!(held > 100, "{held}");
    }

    #[test]
    fn four_lowpasses_smooth_what_comes_out() {
        // y = 0.29808 (x + x1) + 0.40384 y1, four times: all of a level
        // that stays, and none of a tone at half the rate.
        let run = |input: &dyn Fn(usize) -> f32, len: usize| {
            let mut poles = [[0.0f32; 2]; 4];
            let mut out = Vec::new();
            for n in 0..len {
                let mut value = input(n);
                for pole in &mut poles {
                    let next = SMOOTH_A * value + SMOOTH_A * pole[0] + SMOOTH_B * pole[1];
                    *pole = [value, next];
                    value = next;
                }
                out.push(value);
            }
            out
        };
        let level = run(&|_| 1.0, 200);
        assert!((level[0] - 0.298_08f32.powi(4)).abs() < 1e-7);
        assert!((level[199] - 1.0).abs() < 1e-5);
        let top = run(&|n| if n % 2 == 0 { 1.0 } else { -1.0 }, 200);
        assert!(top[199].abs() < 1e-6);
        // A quarter of the rate: |0.29808 (1 - i) / (1 + 0.40384 i)|^4.
        let tone = testing::sine(0.25, 400);
        let quarter = run(&|n| tone[n], 400);
        let want = (0.298_08f64 * 2f64.sqrt() / 1f64.hypot(0.403_84)).powi(4);
        assert!((partial(&quarter[200..], 0.25) - want).abs() < 1e-4);

        // The plugin's are these, on what the slew limit lets out.
        let mut bass = with_sines();
        bass.note_on(note(1, 100));
        let (mut limited, mut sound) = (Vec::new(), Vec::new());
        for _ in 0..500 {
            sound.extend(play(&mut bass, 1));
            limited.push(bass.tone.out);
        }
        let smoothed = run(&|n| limited[n], 500);
        assert!(sound.iter().any(|value| value.abs() > 0.01));
        for (n, (value, want)) in sound.iter().zip(&smoothed).enumerate() {
            assert!((value - want).abs() < 1e-6, "sample {n}");
        }
    }

    #[test]
    fn the_envelope_is_four_lines_and_its_first_fall_is_four_turns_of_the_note() {
        let mut envelope = Envelope::new(4, 8, 2);
        let mut levels = Vec::new();
        while envelope.stage != Envelope::DONE {
            levels.push(envelope.level);
            envelope.advance();
        }
        // Nothing on the first sample, then from 1 to 0.9375 in 4, to 0
        // in 8 and 2 more of release from there.
        let fall = |k: f32| 1.0 - 0.015_625 * k;
        let fade = |k: f32| 0.9375 * (1.0 - k / 8.0);
        let want = [
            0.0,
            fall(1.0),
            fall(2.0),
            fall(3.0),
            fall(4.0),
            fade(1.0),
            fade(2.0),
            fade(3.0),
            fade(4.0),
            fade(5.0),
            fade(6.0),
            fade(7.0),
            0.0,
            0.0,
            0.0,
        ];
        assert_eq!(levels.len(), want.len());
        for (level, want) in levels.iter().zip(want) {
            assert!((level - want).abs() < 1e-6, "{levels:?}");
        }
        // A stage of no samples is passed over.
        let mut envelope = Envelope::new(0, 0, 0);
        envelope.advance();
        assert_eq!(envelope.stage, Envelope::DONE);

        // The note off: from where the level is to 0 in the release.
        let mut envelope = Envelope::new(100, 100, 4);
        envelope.advance();
        envelope.advance();
        let from = envelope.level;
        envelope.release();
        let mut levels = Vec::new();
        while envelope.stage != Envelope::DONE {
            envelope.advance();
            levels.push(envelope.level);
        }
        assert_eq!(levels.len(), 5);
        for (k, level) in levels.iter().enumerate() {
            let want = from * (1.0 - (k + 1).min(4) as f32 / 4.0);
            assert!((level - want).abs() < 1e-6, "{levels:?}");
        }
        // A second note off does not start it over.
        let mut twice = Envelope::new(100, 100, 4);
        twice.advance();
        twice.release();
        twice.advance();
        let once = twice;
        twice.release();
        assert_eq!(twice, once);

        // The lengths of a note: rate / (2^(c / 1200) * 4.0887), 16 times
        // the attack at 44.1 kHz, and a 55th of a second.
        let mut bass = with_sines();
        let lengths = |bass: &mut BooBass, key: i16| {
            bass.note_on(note(1, key));
            bass.note.unwrap().envelope.lengths
        };
        assert_eq!(lengths(&mut bass, 36), [0, 10_785, 64_000, 801, u32::MAX]);
        assert_eq!(lengths(&mut bass, 48)[1], 5392);
        assert_eq!(lengths(&mut bass, 60)[1], 2696);
        bass.set_sample_rate(48_000.0);
        assert_eq!(lengths(&mut bass, 36), [0, 11_739, 69_659, 872, u32::MAX]);
        assert_eq!(bass.tail(), 874);
    }

    #[test]
    fn the_level_is_read_in_48_steps_of_a_db() {
        let table = level_table();
        assert_eq!((table[0], table[48]), (0.0, 1.0));
        for (step, gain) in table.iter().enumerate().take(48).skip(1) {
            let db = db_of(*gain);
            assert!((db - (step as f32 - 48.0)).abs() < 1e-4, "{step}: {db}");
        }
        assert_eq!(level_gain(&table, 1.0), 1.0);
        assert_eq!(level_gain(&table, 7.0), 1.0);
        assert_eq!(level_gain(&table, 0.0), 0.0);
        assert_eq!(level_gain(&table, -1.0), 0.0);
        // 0.9375 is 3 dB down, and between two steps is between them.
        assert_eq!(level_gain(&table, HELD), table[45]);
        assert!((db_of(level_gain(&table, HELD)) + 3.0).abs() < 1e-4);
        let between = level_gain(&table, 24.5 / 48.0);
        assert!((between - (table[24] + table[25]) / 2.0).abs() < 1e-6);
    }

    #[test]
    fn a_note_plays_the_zone_whose_root_is_nearest() {
        for (cents, zone) in [
            (-900, 0),
            (0, 0),
            (1100, 0),
            (1350, 0),
            (1351, 1),
            (1600, 1),
            (1950, 1),
            (1951, 2),
            (2550, 2),
            (2551, 3),
            (3150, 3),
            (3151, 4),
            (3750, 4),
            (3751, 5),
            (4000, 5),
            (9600, 5),
        ] {
            assert_eq!(zone_of(cents), zone, "{cents} cents");
        }
        // Key 60 is 2400 cents, and 50 cents of fine pitch count.
        let mut bass = with_sines();
        let zone = |bass: &mut BooBass, key: i16, fine: f32| {
            bass.note_on(NoteOn {
                fine_pitch: fine,
                ..note(1, key)
            });
            bass.note.unwrap().zone
        };
        assert_eq!(zone(&mut bass, 60, 0.0), 2);
        assert_eq!(zone(&mut bass, 61, 50.0), 2);
        assert_eq!(zone(&mut bass, 61, 51.0), 3);
        // No higher than 9600 cents, which is 5 keys over the last one.
        let step = |bass: &mut BooBass, fine: f32| {
            bass.note_on(NoteOn {
                fine_pitch: fine,
                ..note(1, 127)
            });
            bass.note.unwrap().loop_step
        };
        let top = 256.0 * (267_955.84 / 44_100.0);
        assert_eq!(step(&mut bass, 500.0), top);
        assert_eq!(step(&mut bass, 900.0), top);
        assert!(step(&mut bass, 0.0) < top * 0.75);
    }

    #[test]
    fn a_new_note_cuts_the_one_before_it() {
        // Long attacks, so that the notes fall slowly: 16 times 40000
        // samples.
        let mut bass = BooBass::default();
        bass.set_samples(sines(40_000));
        bass.note_on(note(1, 47));
        play(&mut bass, 12_000);
        bass.note_on(note(2, 59));
        assert_eq!(bass.note.map(|note| (note.id, note.zone)), Some((2, 2)));
        play(&mut bass, 12_000);
        // 61.7 Hz is there and the 30.9 Hz of the first note is not: ten
        // turns of the one are twenty of the other.
        let turns =
            |root: i32| 267_955.84 / 16_384.0 * 2f64.powf(f64::from(root) / 1200.0) / 44_100.0;
        let whole = (10.0 / turns(1100)).round() as usize;
        let sound = play(&mut bass, whole);
        let first = partial(&sound, 10.0 / whole as f64);
        let second = partial(&sound, 20.0 / whole as f64);
        assert!(second > 0.05 && first < second * 0.02, "{first} {second}");
        assert!((turns(2300) / (20.0 / whole as f64) - 1.0).abs() < 1e-3);

        // The first note's off is no longer anyone's. The second's lets
        // the note go in a 55th of a second.
        bass.note_off(1);
        assert_eq!(bass.note.unwrap().envelope.stage, 2);
        bass.note_off(2);
        assert_eq!(bass.note.unwrap().envelope.stage, 3);
        let sound = play(&mut bass, 2000);
        assert!(bass.note.is_none());
        assert!(sound[300..500].iter().any(|value| value.abs() > 1e-3));
        assert_eq!(sound[bass.tail()..], vec![0.0; 2000 - bass.tail()]);
        assert_eq!(play(&mut bass, 16), [0.0; 16]);

        // Every note is let go at once, and a reset cuts.
        bass.note_on(note(3, 59));
        bass.all_notes_off();
        assert_eq!(bass.note.unwrap().envelope.stage, 3);
        bass.note_on(note(4, 59));
        bass.reset();
        assert_eq!(play(&mut bass, 16), [0.0; 16]);
    }

    #[test]
    fn a_note_sounds_from_its_second_sample_and_fades_out_by_itself() {
        let mut bass = BooBass::default();
        bass.set_samples(sines(100));
        bass.note_on(note(1, 47));
        // 16 times 100 samples of fall, after the 5713 of four turns.
        let lengths = bass.note.unwrap().envelope.lengths;
        assert_eq!(lengths[..3], [0, 5713, 1600]);
        let sound = play(&mut bass, 10_000);
        assert_eq!(sound[0], 0.0);
        assert!(sound[2] != 0.0);
        assert!(bass.note.is_none());
        let last = sound.iter().rposition(|value| *value != 0.0).unwrap();
        assert!((7000..=1 + 5713 + 1600 + 801 + 1).contains(&last), "{last}");
    }

    #[test]
    fn velocity_and_pan_set_the_two_gains_and_a_block_adds_to_what_it_is_handed() {
        let centre = {
            let mut bass = with_sines();
            bass.note_on(note(1, 52));
            play(&mut bass, 600)
        };
        assert!(centre.iter().any(|value| value.abs() > 0.01));
        let mut bass = with_sines();
        bass.note_on(NoteOn {
            velocity: 0.5,
            pan: 1.0,
            ..note(1, 52)
        });
        let (mut left, mut right) = (vec![1.0; 600], vec![1.0; 600]);
        bass.process(&mut left, &mut right);
        for n in 0..600 {
            assert_eq!(left[n], 1.0);
            assert_eq!(right[n], 1.0 + centre[n] * 0.5, "sample {n}");
        }
    }

    #[test]
    fn the_blocks_can_fall_anywhere() {
        let whole = {
            let mut bass = with_sines();
            bass.note_on(note(1, 50));
            let mut sound = play(&mut bass, 3000);
            bass.note_off(1);
            sound.extend(play(&mut bass, 1500));
            sound
        };
        for block in [1, 16, 100, 1024] {
            let mut bass = with_sines();
            bass.note_on(note(1, 50));
            let mut sound = Vec::new();
            while sound.len() < 4500 {
                if sound.len() == 3000 {
                    bass.note_off(1);
                }
                let next = if sound.len() < 3000 { 3000 } else { 4500 };
                sound.extend(play(&mut bass, block.min(next - sound.len())));
            }
            assert_eq!(testing::bits(&sound), testing::bits(&whole), "{block}");
        }
    }

    #[test]
    fn a_moved_knob_pulls_its_gain_along_every_16_samples() {
        let mut bass = with_sines();
        bass.note_on(note(1, 47));
        play(&mut bass, 64);
        let from = bass.smooth[2];
        bass.set_param(2, 65_535.0);
        let to = bass.gains[2];
        assert!(to > from && bass.smooth[2] == from);
        // A sixteenth of the way after each 16 of a block of 256.
        play(&mut bass, 0);
        let mut block = BooBass::clone(&bass);
        play(&mut block, 256);
        assert!((block.smooth[2] - to).abs() < 1e-6);
        let (mut l, mut r) = ([0.0; 256], [0.0; 256]);
        bass.process(&mut l[..15], &mut r[..15]);
        // Under 16 samples a block has no step in it.
        assert_eq!(bass.smooth[2], from);
        bass.process(&mut l[..32], &mut r[..32]);
        assert!((bass.smooth[2] - to).abs() < 1e-6);
        // The next note starts on the knobs.
        bass.set_param(0, 65_535.0);
        bass.note_on(note(2, 47));
        assert_eq!(bass.smooth, bass.gains);
        assert_eq!(ramp_step(1e-9, 64), 0.0);
        assert_eq!(ramp_step(0.5, 64), 0.125);
    }

    #[test]
    fn a_loop_is_one_turn_in_16384_points_and_an_attack_keeps_its_last_four_samples() {
        let samples = sines(4000);
        for (zone, sampled) in samples.zones.iter().enumerate() {
            assert_eq!(sampled.wave.len(), TABLE);
            // Half, the trim, and turned over.
            let gain = -0.5 * 10f64.powf(-f64::from(LOOP_TRIMS[zone]) / 20.0) * 32_000.0 / 32_768.0;
            for k in (0..TABLE).step_by(97) {
                let want = gain * (TAU * k as f64 / TABLE as f64).sin();
                assert!(
                    (f64::from(sampled.wave[k]) - want).abs() < 2e-3,
                    "{zone} {k}"
                );
            }
            assert_eq!(sampled.attack.len(), 4000);
        }
        // An attack of one level: the trim and the half on all but four.
        let level = [16_384i16; 9];
        let turn = [0i16, 100, 0, -100];
        let samples = Samples::from_pcm([&level[..]; 6], [&turn[..]; 6]).unwrap();
        let attack = &samples.zones[5].attack;
        let gain = 0.25 * 10f32.powf(3.0 / 20.0);
        assert!(attack[..5].iter().all(|value| (value - gain).abs() < 1e-6));
        assert_eq!(attack[5..], [0.5; 4]);
        // The points on the samples are the samples.
        let wave = &samples.zones[5].wave;
        let peak = -0.5 * 10f32.powf(-3.0 / 20.0) * 100.0 / 32_768.0;
        assert!((wave[TABLE / 4] - peak).abs() < 1e-7 && wave[TABLE / 2].abs() < 1e-9);
        assert!((wave[3 * TABLE / 4] + peak).abs() < 1e-7 && wave[0] == 0.0);

        assert!(Samples::from_pcm([&level[..4]; 6], [&turn[..]; 6]).is_none());
        assert!(Samples::from_pcm([&level[..]; 6], [&turn[..0]; 6]).is_none());
    }

    /// A DLL of one section that has the two tables where FL's has them,
    /// and these samples after them: six attacks and six loops.
    fn dll(samples: &[Vec<i16>]) -> Vec<u8> {
        const BASE: u64 = 0x1_8000_0000;
        const SECTION: u64 = 0xcb000;
        // The tables are 0x920 into the section, and 0xc0 long.
        let mut data = vec![0u8; 0x9e0];
        for (k, pcm) in samples.iter().enumerate() {
            let start = BASE + SECTION + data.len() as u64;
            data.extend(pcm.iter().flat_map(|value| value.to_le_bytes()));
            let length = pcm.len() as i32 * 2;
            data.resize(data.len().next_multiple_of(4), 0);
            let length_at = BASE + SECTION + data.len() as u64;
            data.extend(length.to_le_bytes());
            let entry = 0x920 + 16 * k;
            data[entry..entry + 8].copy_from_slice(&start.to_le_bytes());
            data[entry + 8..entry + 16].copy_from_slice(&length_at.to_le_bytes());
        }
        let mut file = vec![0u8; 0x200];
        file[..2].copy_from_slice(b"MZ");
        file[0x3c] = 0x80;
        file[0x80..0x84].copy_from_slice(b"PE\0\0");
        // One section, and an optional header of 240 bytes.
        file[0x86] = 1;
        file[0x94] = 240;
        file[0x98..0x9a].copy_from_slice(&0x20bu16.to_le_bytes());
        file[0xb0..0xb8].copy_from_slice(&BASE.to_le_bytes());
        let section = 0x98 + 240;
        file[section..section + 5].copy_from_slice(b".data");
        let words = [data.len() as u32, SECTION as u32, data.len() as u32, 0x200];
        for (k, word) in words.iter().enumerate() {
            let at = section + 8 + 4 * k;
            file[at..at + 4].copy_from_slice(&word.to_le_bytes());
        }
        file.extend(data);
        file
    }

    #[test]
    fn the_samples_are_read_out_of_a_dll_s_data() {
        let attack = |zone: usize| pcm_sine(50 + zone, 20.0, 9000.0);
        let turn = |zone: usize| pcm_sine(31 + 2 * zone, 31.0 + 2.0 * zone as f64, 30_000.0);
        let pcm: Vec<Vec<i16>> = (0..6).map(attack).chain((0..6).map(turn)).collect();
        let file = dll(&pcm);
        let read = Samples::from_dll(&file).unwrap();
        let slices: Vec<&[i16]> = pcm.iter().map(Vec::as_slice).collect();
        let want = Samples::from_pcm(
            std::array::from_fn(|zone| slices[zone]),
            std::array::from_fn(|zone| slices[6 + zone]),
        );
        assert_eq!(Some(read), want);

        // A file that is cut short, is no 64-bit image, or has something
        // else where the tables are has no samples.
        assert!(Samples::from_dll(&file[..file.len() - 40]).is_none());
        assert!(Samples::from_dll(&file[..0x150]).is_none());
        let mut other = file.clone();
        other[0x98] = 0x0b;
        other[0x99] = 0x01;
        assert!(Samples::from_dll(&other).is_none());
        let mut moved = file.clone();
        moved[0x200 + 0x928] ^= 0x10;
        assert!(Samples::from_dll(&moved).is_none());
        let mut zeroed = file;
        zeroed[0x200 + 0x920..0x200 + 0x9e0].fill(0);
        assert!(Samples::from_dll(&zeroed).is_none());
    }

    #[test]
    fn fl_s_dll_has_six_attacks_and_six_turns_of_a_string() {
        let Some(bass) = with_fl_s() else {
            return;
        };
        let zones = &bass.samples.as_ref().unwrap().zones;
        let attacks: Vec<usize> = zones.iter().map(|zone| zone.attack.len()).collect();
        assert_eq!(attacks, [25_066, 27_087, 30_322, 30_802, 29_795, 22_456]);
        for (zone, sampled) in zones.iter().enumerate() {
            // Full scale in the DLL, so half and the trim here.
            let peak = |sound: &[f32]| sound.iter().fold(0.0f32, |peak, v| peak.max(v.abs()));
            let attack = 0.5 * 10f32.powf(-ATTACK_TRIMS[zone] / 20.0);
            let turn = 0.5 * 10f32.powf(-LOOP_TRIMS[zone] / 20.0);
            assert!(
                (peak(&sampled.attack) / attack - 1.0).abs() < 0.01,
                "{zone}"
            );
            assert!((peak(&sampled.wave) / turn - 1.0).abs() < 0.02, "{zone}");
        }
    }

    #[test]
    fn key_36_is_the_lowest_zone_11_semitones_under_its_root() {
        let Some(mut bass) = with_fl_s() else {
            return;
        };
        bass.set_sample_rate(48_000.0);
        bass.note_on(note(1, 36));
        // 0 cents: zone 0, whose root is 1100. The loop turns at 16.35 Hz
        // and the attack plays at 2^(-1100 / 1200) of its 16 kHz.
        let started = bass.note.unwrap();
        assert_eq!((started.zone, ROOTS[started.zone]), (0, 1100));
        let turn = f64::from(started.loop_step) / 16_384.0 * 48_000.0;
        assert!((turn - 16.3547).abs() < 1e-3, "{turn} Hz");
        let ratio = f64::from(started.attack_step) * 48_000.0 / 16_000.0;
        assert!(
            (ratio - 2f64.powf(-1100.0 / 1200.0)).abs() < 1e-6,
            "{ratio}"
        );
        assert_eq!(started.attack_end, 25_065.0);

        // The attack is over after 25065 / 0.1766 samples, and what
        // sounds then repeats every 48000 / 16.35 of them.
        let over = (25_065.0 / f64::from(started.attack_step)) as usize + 1;
        assert!((141_900..142_000).contains(&over), "{over}");
        let sound = play(&mut bass, over + 16_000);
        assert!(bass.note.unwrap().attack_at >= 25_065.0);
        let (lag, like) = period(&sound[over + 1000..], 48_000.0 / 16.3547);
        assert!(
            (lag as f64 - 2934.9).abs() < 3.0 && like > 0.98,
            "{lag} {like}"
        );

        // A key on a zone's root plays the attack as it was recorded,
        // which is at the root: the plucked start has the loop's period.
        for (zone, key) in [47, 52, 59, 64, 71, 76].into_iter().enumerate() {
            bass.reset();
            bass.note_on(note(1, key));
            let started = bass.note.unwrap();
            assert_eq!(started.zone, zone);
            assert_eq!(started.attack_step, 16_000.0 / 48_000.0);
            let root = 16.3547 * 2f64.powf(f64::from(ROOTS[zone]) / 1200.0);
            let turn = f64::from(started.loop_step) / 16_384.0 * 48_000.0;
            assert!((turn / root - 1.0).abs() < 1e-5);
            let sound = play(&mut bass, 30_000);
            let (lag, like) = period(&sound[12_000..], 48_000.0 / root);
            assert!(
                (lag as f64 * root / 48_000.0 - 1.0).abs() < 0.015 && like > 0.9,
                "zone {zone}: {lag} {like}"
            );
        }
    }

    #[test]
    fn every_preset_and_every_state_in_fl_s_projects_loads() {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, BooBass's states skipped");
            return;
        };
        let mut saved = testing::every_preset(&root, NAME);
        saved.extend(testing::projects(&root));
        let mut states = Vec::new();
        for saved in saved {
            if !saved.plugin.eq_ignore_ascii_case(NAME) {
                continue;
            }
            let mut bass = crate::generator_by_name(&saved.plugin).unwrap();
            if let Err(error) = bass.set_state(&saved.state) {
                panic!("{}: {error}", saved.file);
            }
            assert_eq!(saved.state.len(), 16, "{}", saved.file);
            states.push([0, 1, 2].map(|index| bass.param(index) as i32));
        }
        // Three presets and three projects, and "5.1 bass" is the one
        // that is not the default: all bass.
        states.sort();
        assert_eq!(states[..5], [[32_767; 3]; 5]);
        assert_eq!(states[5..], [[65_535, 0, 0]]);
    }
}
