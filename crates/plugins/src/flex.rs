//! FLEX: a preset player. The state names a pack and a preset in it; the
//! preset (`flex_preset.rs`, unmasked by `flex_pack.rs`) holds up to 16
//! layers of zones, and each zone is a voice of FLEX's v1 engine
//! (`engine_x64.dll`): four oscillators of eight unison voices each, four
//! envelopes, two filters and a matrix of 64 modulation routes over the
//! zone's 751 parameters, as `docs/plugins/flex.md` has them.
//!
//! # A note
//!
//! Every active layer (`layer[5] >= 0.5`) starts a voice for each of its
//! zones whose key and velocity ranges hold the note (`FUN_1803c7060`).
//! The voice works in control steps of 32 samples (`FUN_1804163d0`): the
//! envelopes move a step, the routes add their sources to the parameters
//! they target (`FUN_1803da290`), and the gains ramp over the step.
//!
//! An oscillator plays its zone's sample (type 0), noise (2), one of the
//! 28 waves of `flex_waves.rs` (5, 7), a mix of two (1), or a wavetable read
//! over its slot's sample (3, 4, 6), at `8.1758 * 2^(note / 12)` Hz or, for
//! a sample, `2^((note - root) / 12)` times its own rate. Its unison voices
//! spread its pitch by the detune and its pan by the spread.
//!
//! The volume is FLEX's taper `0.98435 v^3 - 0.231573 v^2 + 0.247223 v`
//! of the zone's volume, of each oscillator's and of the oscillators' sum,
//! times the first envelope; the pan is its law `(1.828 - 0.828 p) p *
//! 1.41443` on either side. The second envelope opens the filters:
//! `14.35 * 2^(10.5833 c)` Hz for a cutoff `c` from 0 to 1, through a
//! state-variable filter of damping `1 - 0.99 res` (`FUN_180395c00`).
//!
//! # What is not FLEX's
//!
//! - The LFOs, frequency and amplitude modulation between oscillators,
//!   pulse width, the phaser, comb and vowel filters, glide and the
//!   arpeggiator are not built; their routes and settings are ignored.
//! - The layers' and the master's effect chains (delay, reverb, limiter,
//!   master filter and the FL plugins a chain hosts) are not built.
//! - The unison spread, its gain and the envelope curves follow the
//!   engine's shapes where they were read and stand in for them where not.
//! - A `.flex2pack` (engine v2) preset is not read and stays silent.

use crate::flex_pack::{self, Pack};
use crate::flex_preset::{self, Preset};
use crate::flex_waves::Waves;
use crate::{Fidelity, Generator, NoteOn, StateError};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The plugin's event 201 name.
pub const NAME: &str = "FLEX";

/// The parameters the shell keeps and hands the engine (REC 0..44).
pub const PARAMS: usize = 45;
/// Parameters of a zone's voice (`FUN_180417810`).
const ZONE: usize = 0x2ef;
/// The routes of a zone's modulation matrix.
const ROUTES: usize = 64;
/// The voices this port keeps, over all layers.
const VOICES: usize = 96;
const OSCS: usize = 4;
const UNISON: usize = 8;
/// Samples in a control step.
const STEP: usize = 32;
/// The longest a release may ring, in seconds.
const LONGEST: f32 = 30.0;

/// The shell's parameters: the lowest and highest integer of each control
/// as the shell saves it, and the default (`docs/plugins/flex.md`).
const RANGES: [(i32, i32, i32); PARAMS] = {
    const WHEEL: (i32, i32, i32) = (0, 65536, 32768);
    let mut out = [WHEEL; PARAMS];
    let mut k = 10;
    while k < 18 {
        out[k] = (0, 65536, 0);
        k += 1;
    }
    out[21] = (0, 65536, 65536);
    out[23] = (0, 31, 0);
    out[24] = (0, 3, 2);
    out[26] = (1, 768, 192);
    out[36] = (0, 65536, 65536);
    out[37] = (0, 1, 1);
    out[38] = (0, 1, 1);
    out[39] = (-2400, 2400, 0);
    out[40] = (0, 1, 1);
    out[41] = (0, 1, 1);
    out[42] = (0, 65536, 65536);
    out[43] = (0, 15, 0);
    out[44] = (0, 65536, 21845);
    out
};

/// FLEX's volume taper (`FUN_180394fc0`).
pub(crate) fn taper(v: f32) -> f32 {
    v * v * v * 0.984_35 - v * v * 0.231_573 + v * 0.247_223
}

/// FLEX's pan law (`FUN_180388d30`): the left and right gains of a pan
/// from 0 (left) to 1 (right), both 1 in the middle.
pub(crate) fn pan_law(p: f32) -> (f32, f32) {
    let p = p.clamp(0.0, 1.0);
    let side = |x: f32| (1.828 - x * 0.828) * x * 1.414_427_2;
    (side(1.0 - p), side(p))
}

/// An envelope time knob in seconds (`FUN_180390c00`): `2^(15 v)` ms, and
/// none at 0.
pub(crate) fn env_time(v: f32) -> f32 {
    if v <= 0.0 {
        0.0
    } else {
        2f32.powf(v * 15.0) * 0.001
    }
}

/// A filter cutoff from 0 to 1 in Hz (`FUN_180395c00`).
pub(crate) fn cutoff_hz(c: f32) -> f32 {
    2f32.powf(c.clamp(0.0, 1.0) * 10.583_333) * 14.35
}

/// A bend of FLEX's (`FUN_180402330`, `FUN_180402290`), from a knob of 0 to
/// 1 that is straight at 0.5.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Curve {
    k: f32,
    inverse: f32,
    log: f32,
}

impl Curve {
    fn of(knob: f32) -> Self {
        let x = 2.0 * knob - 1.0;
        let sign = if x > 0.0 { 0.1 } else { -0.1 };
        let k = ((x.abs() * 8.755_738).exp() - 1.0) * sign;
        if k == 0.0 {
            return Curve::default();
        }
        Curve {
            k,
            inverse: 1.0 / k.abs(),
            log: (k.abs() + 1.0).ln(),
        }
    }

    /// Where `t` from 0 to 1 comes out, from 0 to 1.
    fn at(&self, t: f32) -> f32 {
        if self.k == 0.0 {
            t
        } else if self.k > 0.0 {
            1.0 - (((1.0 - t) * self.log).exp() - 1.0) * self.inverse
        } else {
            ((t * self.log).exp() - 1.0) * self.inverse
        }
    }

    /// The same for a source from -1 to 1, its sign kept.
    fn signed(&self, x: f32) -> f32 {
        if x < 0.0 { -self.at(-x) } else { self.at(x) }
    }
}

/// A decoded sample of the pack.
#[derive(Debug, Clone, PartialEq)]
struct Sample {
    left: Vec<f32>,
    right: Option<Vec<f32>>,
    rate: f32,
}

impl Sample {
    fn len(&self) -> usize {
        self.left.len()
    }

    #[inline]
    fn read(&self, pos: f64) -> (f32, f32) {
        let i = pos as usize;
        if i + 1 >= self.left.len() {
            let l = self.left.last().copied().unwrap_or(0.0);
            let r = self
                .right
                .as_ref()
                .and_then(|r| r.last().copied())
                .unwrap_or(l);
            return (l, r);
        }
        let frac = (pos - i as f64) as f32;
        let line = |s: &[f32]| s[i] + (s[i + 1] - s[i]) * frac;
        let l = line(&self.left);
        (l, self.right.as_deref().map_or(l, line))
    }
}

/// A slot of a zone with its sample found.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct SlotDef {
    sample: Option<usize>,
    looped: bool,
    start: usize,
    loop_start: usize,
    loop_end: usize,
    /// For a wavetable: frames in a cycle and cycles.
    wavelength: usize,
    cycles: usize,
}

/// A route: the source's number, the parameter it moves, the parameter
/// that holds its amount and the bend of its source.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Route {
    source: u32,
    target: usize,
    amount: usize,
    curve: Curve,
}

#[derive(Debug, Clone, PartialEq)]
struct ZoneDef {
    low_key: i32,
    high_key: i32,
    low_velocity: i32,
    high_velocity: i32,
    root: f32,
    params: Vec<f32>,
    slots: [SlotDef; OSCS],
    routes: Vec<Route>,
    /// The longest release of its first envelope, in seconds.
    release: f32,
}

#[derive(Debug, Clone, PartialEq)]
struct LayerDef {
    zones: Vec<ZoneDef>,
    volume: f32,
}

/// What the preset plays.
#[derive(Debug, Clone, PartialEq)]
struct Program {
    layers: Vec<LayerDef>,
    samples: Vec<Sample>,
    /// Whether a zone asks for what is not built.
    approximated: bool,
    missing_samples: bool,
}

/// What the state says (chunks 1 and 2).
#[derive(Debug, Clone, PartialEq)]
struct State {
    pack: String,
    preset: String,
}

/// The state's chunk stream (`FUN_007e0040`): a version, then `i32 id,
/// i64 size` and the bytes.
fn read_state(bytes: &[u8]) -> Result<(State, Option<[f32; PARAMS]>), StateError> {
    let short = StateError::Length {
        expected: 2707,
        found: bytes.len(),
    };
    let word = |at: usize| -> Option<i32> {
        Some(i32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
    };
    let version = word(0).ok_or(short)?;
    if version > 1 {
        return Err(StateError::Version { found: version });
    }
    let mut at = 4;
    let mut state = None;
    let mut params = None;
    while at + 12 <= bytes.len() {
        let id = word(at).ok_or(short)?;
        let size = i64::from_le_bytes(bytes[at + 4..at + 12].try_into().map_err(|_| short)?);
        let size = usize::try_from(size).map_err(|_| short)?;
        let body = bytes.get(at + 12..at + 12 + size).ok_or(short)?;
        at += 12 + size;
        match id {
            1 if body.len() >= 2050 => {
                let name = |raw: &[u8]| {
                    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
                    String::from_utf8_lossy(&raw[..end]).into_owned()
                };
                state = Some(State {
                    pack: name(&body[..1025]),
                    preset: name(&body[1025..2050]),
                });
            }
            2 if body.len() >= 20 + PARAMS * 4 => {
                let mut floats = [0.0; PARAMS];
                for (k, f) in floats.iter_mut().enumerate() {
                    let at = 20 + k * 4;
                    *f = f32::from_le_bytes(body[at..at + 4].try_into().map_err(|_| short)?);
                }
                params = Some(floats);
            }
            _ => {}
        }
    }
    Ok((state.ok_or(short)?, params))
}

/// The shell's float for a parameter's integer, and back.
fn to_float(k: usize, v: i32) -> f32 {
    let (lo, hi, _) = RANGES[k];
    ((v - lo) as f32 / (hi - lo) as f32).clamp(0.0, 1.0)
}

fn to_int(k: usize, f: f32) -> i32 {
    let (lo, hi, _) = RANGES[k];
    lo + (f.clamp(0.0, 1.0) * (hi - lo) as f32).round() as i32
}

/// The zone's parameter as the shell moves it (`FUN_180416f10`): the
/// shell's knob from its middle pulls the value towards 0 or 1 by `depth`.
fn shell_pull(value: f32, shell: f32, depth: f32) -> f32 {
    let pull = 2.0 * (shell - 0.5);
    let room = if pull >= 0.0 { 1.0 - value } else { value };
    (value + pull * room * depth).clamp(0.0, 1.0)
}

/// Build what the preset plays, decoding the samples it names.
fn build(preset: &Preset, pack: &Pack, shell: &[f32; PARAMS]) -> Program {
    let mut samples: Vec<Sample> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut missing = false;
    let mut approximated = false;
    let mut find = |file: &str, samples: &mut Vec<Sample>, missing: &mut bool| -> Option<usize> {
        if file.is_empty() {
            return None;
        }
        let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
        let entry = format!("{stem}.flac");
        if let Some(i) = names.iter().position(|n| *n == entry) {
            return Some(i);
        }
        let decoded = pack.read(&entry).and_then(flex_pack::decode_flac);
        let Some(decoded) = decoded else {
            *missing = true;
            return None;
        };
        names.push(entry);
        samples.push(Sample {
            left: decoded.left,
            right: decoded.right,
            rate: decoded.rate as f32,
        });
        Some(samples.len() - 1)
    };
    let mut layers = Vec::new();
    for layer in &preset.layers {
        let lp = |k: usize| layer.params.get(k).copied().unwrap_or(0.0);
        if lp(5) < 0.5 {
            continue;
        }
        let mut zones = Vec::new();
        for zone in &layer.zones {
            let mut params = vec![0.0; ZONE];
            for (p, v) in params.iter_mut().zip(&zone.params) {
                *p = *v;
            }
            // The shell's filter and envelope knobs (`FUN_180416f10`).
            let depth = |p: &[f32], k: usize| p[k];
            let pulls = [
                (3, 18, 10),
                (7, 18, 11),
                (4, 20, 12),
                (8, 20, 13),
                (5, 19, 14),
                (9, 19, 15),
            ];
            for (at, knob, d) in pulls {
                params[at] = shell_pull(params[at], shell[knob], depth(&params, d));
            }
            let envs = [
                (354, 0),
                (357, 1),
                (359, 2),
                (362, 3),
                (364, 4),
                (384, 5),
                (387, 6),
                (389, 7),
                (392, 8),
                (394, 9),
            ];
            for (at, knob) in envs {
                params[at] = shell_pull(params[at], shell[knob], 1.0);
            }
            let mut slots = [SlotDef::default(); OSCS];
            for (k, slot) in slots.iter_mut().enumerate() {
                let Some(def) = zone.slots.get(k) else {
                    continue;
                };
                slot.sample = find(&def.file, &mut samples, &mut missing);
                slot.looped = def.mode != 0 && def.loop_end > def.loop_start;
                slot.start = def.start.max(0) as usize;
                slot.loop_start = def.loop_start.max(0) as usize;
                slot.loop_end = def.loop_end.max(0) as usize;
                if let Some(table) = zone.tables.get(k).filter(|t| !t.name.is_empty()) {
                    slot.sample = find(&table.name, &mut samples, &mut missing).or(slot.sample);
                    slot.wavelength = table.wavelength.max(2) as usize;
                    slot.cycles = table.cycles.max(1) as usize;
                }
            }
            let routes: Vec<Route> = (0..ROUTES)
                .filter_map(|i| {
                    let source = (params[473 + 3 * i] * 65535.0 + 0.5) as u32;
                    let target = (params[474 + 3 * i] * 65535.0 + 0.5) as usize;
                    (source > 0 && target > 0 && target <= ZONE).then(|| Route {
                        source,
                        target: target - 1,
                        amount: 475 + 3 * i,
                        curve: Curve::of(params[679 + i]),
                    })
                })
                .collect();
            for k in 0..OSCS {
                let b = 33 + 64 * k;
                if params[b] >= 0.5 {
                    let kind = (params[b + 1] * 127.0 + 0.5) as i32;
                    approximated |= matches!(kind, 1 | 7) && (19..23).any(|f| params[b + f] > 0.0);
                }
            }
            approximated |= routes
                .iter()
                .any(|r| (261..269).contains(&r.source) || (273..285).contains(&r.source));
            for f in [2, 6] {
                let kind = (params[f] * 127.0 + 0.5) as i32;
                approximated |= matches!(kind, 6 | 10..=19);
            }
            zones.push(ZoneDef {
                low_key: zone.low_key,
                high_key: zone.high_key,
                low_velocity: zone.low_velocity,
                high_velocity: zone.high_velocity,
                root: zone.root as f32,
                release: env_time(params[364]).min(LONGEST),
                params,
                slots,
                routes,
            });
        }
        approximated |= !layer.chain.effects.is_empty();
        layers.push(LayerDef {
            zones,
            volume: taper(lp(6)),
        });
    }
    Program {
        layers,
        samples,
        approximated,
        missing_samples: missing,
    }
}

/// One of a voice's envelopes (`Env`, `FUN_180390750`), stepped once a
/// control step.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Env {
    stage: u8,
    t: f32,
    count: u32,
    level: f32,
    from: f32,
}

const DELAY: u8 = 0;
const ATTACK: u8 = 1;
const HOLD: u8 = 2;
const DECAY: u8 = 3;
const SUSTAIN: u8 = 4;
const RELEASE: u8 = 5;
const DONE: u8 = 6;

impl Env {
    fn start(&mut self) {
        *self = Env::default();
    }

    fn release(&mut self) {
        if self.stage < RELEASE {
            self.stage = RELEASE;
            self.t = 0.0;
            self.from = self.level;
        }
    }

    /// Move a step, at `steps` control steps a second, with the
    /// envelope's 15 parameters from `p`.
    fn step(&mut self, p: &[f32], steps: f32) {
        let rate = |v: f32| {
            let t = env_time(v);
            if t <= 0.0 { 2.0 } else { 1.0 / (t * steps) }
        };
        loop {
            match self.stage {
                DELAY => {
                    let n = (p[0] * 3.0 * steps) as u32;
                    if self.count >= n {
                        self.stage = ATTACK;
                        self.t = 0.0;
                        self.from = self.level;
                        continue;
                    }
                    self.count += 1;
                }
                ATTACK => {
                    self.t += rate(p[1]);
                    if self.t >= 1.0 {
                        self.level = 1.0;
                        self.stage = HOLD;
                        self.count = 0;
                        continue;
                    }
                    self.level = self.from + Curve::of(p[3]).at(self.t) * (1.0 - self.from);
                }
                HOLD => {
                    let n = (p[4] * 10.0 * steps) as u32;
                    if self.count >= n {
                        self.stage = DECAY;
                        self.t = 0.0;
                        continue;
                    }
                    self.count += 1;
                    self.level = 1.0;
                }
                DECAY => {
                    self.t += rate(p[6]);
                    if self.t >= 1.0 {
                        self.stage = SUSTAIN;
                        continue;
                    }
                    self.level = 1.0 + Curve::of(p[8]).at(self.t) * (p[9] - 1.0);
                }
                SUSTAIN => self.level = p[9],
                RELEASE => {
                    self.t += rate(p[11]);
                    if self.t >= 1.0 {
                        self.stage = DONE;
                        self.level = 0.0;
                        continue;
                    }
                    self.level = self.from - Curve::of(p[13]).at(self.t) * self.from;
                }
                _ => self.level = 0.0,
            }
            return;
        }
    }
}

/// A state-variable filter of the cookbook FLEX uses (`FUN_180395c00`):
/// `g = tan(pi f / rate)`, damping `k`, two channels.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Svf {
    ic1: [f32; 2],
    ic2: [f32; 2],
}

/// The responses a stage gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Low,
    High,
    Band,
    Notch,
}

impl Svf {
    #[inline]
    fn run(&mut self, ch: usize, x: f32, g: f32, k: f32, shape: Shape) -> f32 {
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        let v3 = x - self.ic2[ch];
        let v1 = a1 * self.ic1[ch] + a2 * v3;
        let v2 = self.ic2[ch] + a2 * self.ic1[ch] + a3 * v3;
        self.ic1[ch] = 2.0 * v1 - self.ic1[ch];
        self.ic2[ch] = 2.0 * v2 - self.ic2[ch];
        match shape {
            Shape::Low => v2,
            Shape::Band => v1,
            Shape::High => x - k * v1 - v2,
            Shape::Notch => x - k * v1,
        }
    }
}

/// What a filter type does here: its response and stages, or nothing.
fn filter_shape(kind: i32) -> Option<(Shape, usize)> {
    Some(match kind {
        1 => (Shape::Low, 1),
        2 => (Shape::High, 1),
        3 | 5 | 7 => (Shape::Low, 2),
        4 | 9 => (Shape::High, 2),
        8 => (Shape::Band, 2),
        20 => (Shape::Notch, 1),
        21 | 22 | 25 | 26 | 29 | 30 => (Shape::Low, 1),
        23 | 24 | 27 | 28 | 31 | 32 => (Shape::Low, 2),
        _ => return None,
    })
}

/// A filter's settings for a step: `g`, the damping `k` and what it does.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Stage {
    g: f32,
    k: f32,
    shape: Option<(Shape, usize)>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct OscVoice {
    on: bool,
    kind: i32,
    voices: usize,
    phase: [f64; UNISON],
    step: [f64; UNISON],
    left: [f32; UNISON],
    right: [f32; UNISON],
    level: [usize; UNISON],
    done: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct Voice {
    on: bool,
    id: u32,
    age: u64,
    layer: usize,
    zone: usize,
    key: f32,
    velocity: f32,
    random: f32,
    released: bool,
    values: Vec<f32>,
    delta: Vec<f32>,
    envs: [Env; 4],
    oscs: [OscVoice; OSCS],
    filters: [[Svf; 2]; 2],
    /// The gain of each oscillator's side over the step: where it is and
    /// what it adds a sample.
    gain: [[f32; 2]; OSCS],
    slope: [[f32; 2]; OSCS],
    fresh: bool,
    left: usize,
    /// Filter settings for the step: `g`, `k` and the shape, per filter.
    filter: [Stage; 2],
    noise: u32,
}

impl Voice {
    fn new() -> Self {
        Voice {
            on: false,
            id: 0,
            age: 0,
            layer: 0,
            zone: 0,
            key: 60.0,
            velocity: 1.0,
            random: 0.0,
            released: false,
            values: vec![0.0; ZONE],
            delta: vec![0.0; ZONE],
            envs: [Env::default(); 4],
            oscs: [OscVoice::default(); OSCS],
            filters: [[Svf::default(); 2]; 2],
            gain: [[0.0; 2]; OSCS],
            slope: [[0.0; 2]; OSCS],
            fresh: true,
            left: 0,
            filter: [Stage::default(); 2],
            noise: 1,
        }
    }
}

/// The voice engine of FLEX, as far as it is built.
pub struct Flex {
    rate: f32,
    generators: Option<PathBuf>,
    data: Option<PathBuf>,
    state: Option<State>,
    shell: [f32; PARAMS],
    program: Option<Arc<Program>>,
    waves: Option<Arc<Waves>>,
    voices: Vec<Voice>,
    clock: u64,
    seed: u32,
    lacking: Option<&'static str>,
}

impl Default for Flex {
    fn default() -> Self {
        let mut shell = [0.0; PARAMS];
        for (k, f) in shell.iter_mut().enumerate() {
            *f = to_float(k, RANGES[k].2);
        }
        Flex {
            rate: 44_100.0,
            generators: None,
            data: None,
            state: None,
            shell,
            program: None,
            waves: None,
            voices: (0..VOICES).map(|_| Voice::new()).collect(),
            clock: 0,
            seed: 0x1234_5678,
            lacking: None,
        }
    }
}

impl std::fmt::Debug for Flex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Flex")
            .field("state", &self.state)
            .field("loaded", &self.program.is_some())
            .finish()
    }
}

/// FLEX's random stream (`* 0xbb38435 + 0x3619636b`), as a float from -1
/// to 1.
fn noise(seed: &mut u32) -> f32 {
    *seed = seed.wrapping_mul(0x0bb3_8435).wrapping_add(0x3619_636b);
    (*seed as i32) as f32 * (1.0 / 2_147_483_648.0)
}

impl Flex {
    /// Read the pack and the preset the state names, once both the state
    /// and the install are known.
    fn load(&mut self) {
        self.program = None;
        let Some(state) = self.state.clone() else {
            return;
        };
        let Some(data) = self.data.clone() else {
            self.lacking = Some("FLEX: FL's Data directory is not known, so no pack is read");
            return;
        };
        let dir = flex_pack::packs_dir(&data);
        let v1 = dir.join(format!("{}.flexpack", state.pack));
        let Some(pack) = Pack::open(&v1).filter(|p| p.kind == flex_pack::V1) else {
            self.lacking = Some(if dir.join(format!("{}.flex2pack", state.pack)).is_file() {
                "FLEX: the preset is in a v2 pack, which is not built"
            } else {
                "FLEX: the preset's pack is not installed"
            });
            return;
        };
        let preset = pack
            .read(&format!("{}.flexpreset", state.preset))
            .and_then(|bytes| flex_preset::parse(&bytes));
        let Some(preset) = preset else {
            self.lacking = Some("FLEX: the preset is not in its pack, or does not read");
            return;
        };
        if self.waves.is_none() {
            self.waves = Some(Arc::new(Waves::load(self.generators.as_deref())));
        }
        self.program = Some(Arc::new(build(&preset, &pack, &self.shell)));
        self.lacking = None;
    }

    fn control_rate(&self) -> f32 {
        self.rate / STEP as f32
    }

    /// Work out a voice's step: envelopes, routes, gains, pitches and
    /// filters (`FUN_1804163d0`).
    fn control(&mut self, v: usize) {
        let Some(program) = self.program.clone() else {
            return;
        };
        let steps = self.control_rate();
        let rate = self.rate;
        let shell = self.shell;
        let voice = &mut self.voices[v];
        let zone = &program.layers[voice.layer].zones[voice.zone];
        let layer_volume = program.layers[voice.layer].volume;
        // The envelopes, on the values the last step left.
        for (k, env) in voice.envs.iter_mut().enumerate() {
            let base = 353 + 30 * k;
            env.step(&voice.values[base..base + 15], steps);
        }
        // The routes (`FUN_1803da290`): each adds its bent source times
        // its amount from -1 to 1 to the parameter it moves.
        voice.delta.iter_mut().for_each(|d| *d = 0.0);
        let random = noise(&mut voice.noise);
        for route in &zone.routes {
            let source = match route.source {
                1 => 1.0,
                2 => voice.velocity,
                3 => random,
                4 | 285 => voice.key / 127.0,
                290 | 291 => voice.key / 127.0 - 0.5,
                286 | 288 => voice.random,
                287 | 289 => voice.random * 2.0 - 1.0,
                5..=12 => shell[route.source as usize + 5],
                133..=140 => shell[route.source as usize - 123] - 0.5,
                269..=272 => voice.envs[route.source as usize - 269].level,
                _ => 0.0,
            };
            let amount = zone.params[route.amount] * 2.0 - 1.0;
            voice.delta[route.target] += route.curve.signed(source) * amount;
        }
        for (i, value) in voice.values.iter_mut().enumerate() {
            *value = (zone.params[i] + voice.delta[i]).clamp(0.0, 1.0);
        }
        let p = &voice.values;
        // Gains (`FUN_1801a2200`).
        let env = voice.envs[0].level;
        let (pan_l, pan_r) = pan_law(p[745]);
        let (all_l, all_r) = pan_law(p[678]);
        let common = taper(p[744]) * env * layer_volume * taper(p[677]);
        let pitch = (p[743] - 0.5) * 96.0 + (shell[39] - 0.5) * 48.0;
        for k in 0..OSCS {
            let b = 33 + 64 * k;
            let osc = &mut voice.oscs[k];
            let (ol, or) = pan_law(p[669 + k]);
            let level = taper(p[665 + k]) * p[b + 11] * 2.0;
            let target = [
                common * level * ol * all_l * pan_l,
                common * level * or * all_r * pan_r,
            ];
            for (side, want) in target.iter().enumerate() {
                if voice.fresh {
                    voice.gain[k][side] = *want;
                    voice.slope[k][side] = 0.0;
                } else {
                    voice.slope[k][side] = (want - voice.gain[k][side]) / STEP as f32;
                }
            }
            if !osc.on {
                continue;
            }
            // The pitch of each unison voice (`FUN_1803e15f0`).
            let semis =
                ((p[b + 6] - 0.5) * 96.0).trunc() + (p[b + 9] - 0.5) + (p[b + 10] - 0.5) * 96.0;
            let detune = p[b + 3];
            let spread = p[b + 5];
            let n = osc.voices;
            for u in 0..n {
                let x = if n > 1 {
                    u as f32 / (n - 1) as f32 * 2.0 - 1.0
                } else {
                    0.0
                };
                let note = voice.key + pitch + semis + x * detune * 0.5;
                if osc.kind == 0 {
                    if let Some(sample) = zone.slots[k].sample.map(|s| &program.samples[s]) {
                        osc.step[u] =
                            f64::from(2f32.powf((note - zone.root) / 12.0) * sample.rate / rate);
                    }
                } else {
                    let hz = 8.175_8 * 2f32.powf(note / 12.0);
                    osc.step[u] = f64::from(hz / rate);
                    osc.level[u] = Waves::level(osc.step[u]);
                }
                let place = if n > 1 { 0.5 + x * spread * 0.5 } else { 0.5 };
                let (l, r) = pan_law(place);
                let norm = 1.0 / (n as f32).sqrt();
                osc.left[u] = l * norm;
                osc.right[u] = r * norm;
            }
        }
        // The filters, opened by the second envelope.
        let fenv = voice.envs[1].level;
        for (f, base) in [1usize, 5].into_iter().enumerate() {
            let kind = (p[base + 1] * 127.0 + 0.5) as i32;
            let cut = p[base + 2] + fenv * p[base + 4];
            let hz = cutoff_hz(cut).min(rate * 0.49);
            let g = (std::f32::consts::PI * hz / rate).tan();
            let k = 1.0 - p[base + 3] * 0.99;
            voice.filter[f] = Stage {
                g,
                k: k.max(0.01),
                shape: filter_shape(kind),
            };
        }
        voice.fresh = false;
    }

    fn start_voice(&mut self, note: &NoteOn, layer: usize, zone: usize, key: f32, velocity: f32) {
        let Some(program) = self.program.clone() else {
            return;
        };
        let slot = self
            .voices
            .iter()
            .position(|v| !v.on)
            .or_else(|| (0..self.voices.len()).min_by_key(|&i| self.voices[i].age))
            .unwrap_or(0);
        self.clock += 1;
        let seed = noise(&mut self.seed);
        let def = &program.layers[layer].zones[zone];
        let voice = &mut self.voices[slot];
        voice.on = true;
        voice.id = note.voice;
        voice.age = self.clock;
        voice.layer = layer;
        voice.zone = zone;
        voice.key = key;
        voice.velocity = velocity;
        voice.random = seed * 0.5 + 0.5;
        voice.released = false;
        voice.noise = self.seed | 1;
        voice.values.copy_from_slice(&def.params);
        for env in &mut voice.envs {
            env.start();
        }
        voice.filters = [[Svf::default(); 2]; 2];
        voice.fresh = true;
        voice.left = 0;
        for k in 0..OSCS {
            let b = 33 + 64 * k;
            let p = &def.params;
            let osc = &mut voice.oscs[k];
            *osc = OscVoice::default();
            osc.on = p[b] >= 0.5;
            osc.kind = (p[b + 1] * 127.0 + 0.5) as i32;
            osc.voices = ((p[b + 2] * 7.0 + 1.0) as usize).clamp(1, UNISON);
            if osc.kind == 0 {
                let slot = def.slots[k];
                match slot.sample {
                    Some(s) => {
                        let len = program.samples[s].len();
                        let start = slot.start + (p[b + 45] * len as f32) as usize;
                        osc.phase = [start.min(len) as f64; UNISON];
                    }
                    None => osc.on = false,
                }
            } else {
                let retrigger = p[b + 8] >= 0.5;
                for u in 0..UNISON {
                    osc.phase[u] = if retrigger {
                        f64::from(p[b + 7] * 0.999)
                    } else {
                        f64::from(noise(&mut self.seed) * 0.5 + 0.5)
                    };
                }
            }
        }
    }

    /// Run one voice for `len` samples into the two sides.
    fn run_voice(&mut self, v: usize, left: &mut [f32], right: &mut [f32]) {
        let Some(program) = self.program.clone() else {
            return;
        };
        let Some(waves) = self.waves.clone() else {
            return;
        };
        let mut done = 0;
        let len = left.len();
        while done < len {
            if self.voices[v].left == 0 {
                self.control(v);
                let voice = &mut self.voices[v];
                voice.left = STEP;
                if voice.envs[0].stage == DONE {
                    voice.on = false;
                    return;
                }
            }
            let voice = &mut self.voices[v];
            let n = voice.left.min(len - done);
            let zone = &program.layers[voice.layer].zones[voice.zone];
            for i in done..done + n {
                let mut out = [0.0f32; 2];
                for k in 0..OSCS {
                    let osc = &mut voice.oscs[k];
                    voice.gain[k][0] += voice.slope[k][0];
                    voice.gain[k][1] += voice.slope[k][1];
                    if !osc.on || osc.done {
                        continue;
                    }
                    let b = 33 + 64 * k;
                    let p = &voice.values;
                    let mut sum = [0.0f32; 2];
                    for u in 0..osc.voices {
                        let (l, r) = match osc.kind {
                            0 => {
                                let slot = &zone.slots[k];
                                let Some(sample) = slot.sample.map(|s| &program.samples[s]) else {
                                    break;
                                };
                                let pos = osc.phase[u];
                                if pos >= sample.len() as f64 {
                                    osc.done = true;
                                    break;
                                }
                                let frame = sample.read(pos);
                                let mut next = pos + osc.step[u];
                                if slot.looped && next >= slot.loop_end as f64 {
                                    next -= (slot.loop_end - slot.loop_start) as f64;
                                }
                                osc.phase[u] = next;
                                frame
                            }
                            2 => {
                                let x = noise(&mut voice.noise);
                                (x, x)
                            }
                            _ => {
                                let phase = osc.phase[u];
                                let a = (p[b + 13] * 1023.0 + 0.5) as usize;
                                let mut x = match osc.kind {
                                    1 => {
                                        let bw = (p[b + 14] * 1023.0 + 0.5) as usize;
                                        let mix = p[b + 15];
                                        waves.read(a, osc.level[u], phase) * (1.0 - mix)
                                            + waves.read(bw, osc.level[u], phase) * mix
                                    }
                                    _ => waves.read(a, osc.level[u], phase),
                                };
                                if matches!(osc.kind, 3 | 4 | 6) {
                                    let slot = &zone.slots[k];
                                    if let Some(sample) = slot.sample.map(|s| &program.samples[s]) {
                                        let cycles = slot.cycles.max(1);
                                        let length = slot.wavelength;
                                        let at = p[b + 53] * (cycles - 1) as f32;
                                        let frame = (at as usize).min(cycles - 1);
                                        let pos = frame * length + (phase * length as f64) as usize;
                                        x = sample.left.get(pos).copied().unwrap_or(0.0);
                                    }
                                }
                                let mut next = phase + osc.step[u];
                                if next >= 1.0 {
                                    next -= next.floor();
                                }
                                osc.phase[u] = next;
                                (x, x)
                            }
                        };
                        sum[0] += l * osc.left[u];
                        sum[1] += r * osc.right[u];
                    }
                    out[0] += sum[0] * voice.gain[k][0];
                    out[1] += sum[1] * voice.gain[k][1];
                }
                for f in 0..2 {
                    let Stage { g, k, shape } = voice.filter[f];
                    if let Some((shape, stages)) = shape {
                        for (ch, x) in out.iter_mut().enumerate() {
                            let mut y = *x;
                            for _ in 0..stages {
                                y = voice.filters[f][ch].run(ch, y, g, k, shape);
                            }
                            *x = y;
                        }
                    }
                }
                left[i] += out[0];
                right[i] += out[1];
            }
            voice.left -= n;
            done += n;
            if voice.oscs.iter().all(|o| !o.on || o.done) && voice.released {
                voice.on = false;
                return;
            }
        }
    }
}

impl Generator for Flex {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let (state, params) = read_state(bytes)?;
        self.state = Some(state);
        if let Some(params) = params {
            self.shell = params;
        }
        self.reset();
        self.load();
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz > 0.0 {
            self.rate = hz;
        }
    }

    /// `dir` is FL's `Plugins/Fruity/Generators`: the waves are in
    /// `FLEX/engine_x64.dll` there, and the packs under the `Data`
    /// directory beside `Plugins` (or `dir` itself when it holds
    /// `Patches`).
    fn set_resource_dir(&mut self, dir: &Path) {
        let install = dir.parent().and_then(Path::parent).and_then(Path::parent);
        let beside = install.map(|install| install.join("Data"));
        let own = dir.join("Patches").is_dir().then(|| dir.to_path_buf());
        self.data = own.or(beside.filter(|data| data.is_dir()));
        self.generators = Some(dir.to_path_buf());
        self.waves = None;
        if self.state.is_some() {
            self.load();
        }
    }

    fn note_on(&mut self, note: NoteOn) {
        if note.slide {
            return;
        }
        let Some(program) = self.program.clone() else {
            return;
        };
        let key = f32::from(note.key) + note.fine_pitch / 100.0;
        let velocity = (note.velocity * 100.0 / 127.0).clamp(0.0, 1.0);
        let midi = (velocity * 127.0 + 0.5) as i32;
        let key_int = i32::from(note.key);
        for (l, layer) in program.layers.iter().enumerate() {
            for (z, zone) in layer.zones.iter().enumerate() {
                let keys = zone.low_key..=zone.high_key;
                let velocities = zone.low_velocity..=zone.high_velocity;
                if keys.contains(&key_int) && velocities.contains(&midi) {
                    self.start_voice(&note, l, z, key, velocity);
                }
            }
        }
    }

    fn note_off(&mut self, voice: u32) {
        for v in self.voices.iter_mut().filter(|v| v.on && v.id == voice) {
            v.released = true;
            for env in &mut v.envs {
                env.release();
            }
        }
    }

    fn all_notes_off(&mut self) {
        for v in self.voices.iter_mut().filter(|v| v.on) {
            v.released = true;
            for env in &mut v.envs {
                env.release();
            }
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if self.program.is_none() {
            return;
        }
        let len = left.len().min(right.len());
        for v in 0..self.voices.len() {
            if self.voices[v].on {
                self.run_voice(v, &mut left[..len], &mut right[..len]);
            }
        }
    }

    fn param_count(&self) -> usize {
        PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        if index < PARAMS {
            to_int(index, self.shell[index]) as f32
        } else {
            0.0
        }
    }

    /// The shell's knobs that this port plays are the envelope and filter
    /// pulls, the macros and the transpose; they take effect from the next
    /// note on.
    fn set_param(&mut self, index: usize, value: f32) {
        if index < PARAMS {
            let (lo, hi, _) = RANGES[index];
            let v = (value.round() as i32).clamp(lo, hi);
            self.shell[index] = to_float(index, v);
        }
    }

    fn reset(&mut self) {
        for v in &mut self.voices {
            v.on = false;
        }
    }

    fn tail(&self) -> usize {
        let longest = self.program.as_ref().map_or(1.0, |p| {
            p.layers
                .iter()
                .flat_map(|l| &l.zones)
                .map(|z| z.release)
                .fold(0.0f32, f32::max)
        });
        ((longest + 0.1) * self.rate) as usize
    }

    fn fidelity(&self) -> Fidelity {
        if let Some(lacking) = self.lacking {
            return Fidelity::Approximate(lacking);
        }
        match &self.program {
            None => Fidelity::Approximate("FLEX: no preset is loaded"),
            Some(p) if p.missing_samples => {
                Fidelity::Approximate("FLEX: a sample the preset names is not in its pack")
            }
            Some(_) if self.waves.as_ref().is_some_and(|w| !w.found) => Fidelity::Approximate(
                "FLEX: the engine's waves were not found, and 24 of its waves are silent",
            ),
            Some(p) if p.approximated => Fidelity::Approximate(
                "FLEX: the preset uses LFOs, FM, filter types or effects that are left out",
            ),
            Some(_) => Fidelity::Approximate(
                "FLEX: the master effects are left out, and the voice is approximated",
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_taper_and_the_pan_law_meet_flex_s_ends() {
        assert_eq!(taper(0.0), 0.0);
        assert!((taper(1.0) - 1.0).abs() < 1e-5);
        let (l, r) = pan_law(0.5);
        assert!((l - 1.0).abs() < 1e-3 && (r - 1.0).abs() < 1e-3, "{l} {r}");
        let (l, r) = pan_law(0.0);
        assert!(
            r.abs() < 1e-6 && (l - 1.414_427 * 1.0).abs() < 1e-3,
            "{l} {r}"
        );
    }

    #[test]
    fn the_times_and_the_cutoff_span_flex_s_ranges() {
        assert_eq!(env_time(0.0), 0.0);
        assert!((env_time(0.5) - 0.181_02).abs() < 1e-4);
        assert!((env_time(1.0) - 32.768).abs() < 1e-3);
        assert!((cutoff_hz(0.0) - 14.35).abs() < 1e-3);
        assert!((cutoff_hz(1.0) - 14.35 * 1024.0 * 2f32.powf(7.0 / 12.0)).abs() < 1.0);
    }

    #[test]
    fn a_curve_is_straight_in_the_middle_and_bends_either_way() {
        let flat = Curve::of(0.5);
        assert_eq!(flat.at(0.3), 0.3);
        for knob in [0.1, 0.9] {
            let c = Curve::of(knob);
            assert!(c.at(0.0).abs() < 1e-5 && (c.at(1.0) - 1.0).abs() < 1e-4);
        }
        assert!(Curve::of(0.9).at(0.5) > 0.5);
        assert!(Curve::of(0.1).at(0.5) < 0.5);
        assert_eq!(Curve::of(0.1).signed(-0.5), -Curve::of(0.1).at(0.5));
    }

    #[test]
    fn an_envelope_attacks_holds_decays_and_lets_go() {
        let mut p = [0.0f32; 15];
        p[1] = 0.3; // about 22.6 ms
        p[3] = 0.5;
        p[6] = 0.3;
        p[8] = 0.5;
        p[9] = 0.4;
        p[11] = 0.3;
        p[13] = 0.5;
        let mut env = Env::default();
        let steps = 1000.0;
        for _ in 0..10 {
            env.step(&p, steps);
        }
        assert!(env.level > 0.3 && env.level < 0.6, "{}", env.level);
        for _ in 0..100 {
            env.step(&p, steps);
        }
        assert_eq!(env.stage, SUSTAIN);
        assert!((env.level - 0.4).abs() < 1e-6);
        env.release();
        for _ in 0..100 {
            env.step(&p, steps);
        }
        assert_eq!(env.stage, DONE);
        assert_eq!(env.level, 0.0);
    }

    #[test]
    fn a_state_names_its_pack_and_preset_and_carries_the_knobs() {
        let mut bytes = 1i32.to_le_bytes().to_vec();
        let mut one = vec![0u8; 2066];
        one[..5].copy_from_slice(b"Packy");
        one[1025..1029].copy_from_slice(b"Pres");
        bytes.extend_from_slice(&1i32.to_le_bytes());
        bytes.extend_from_slice(&(one.len() as i64).to_le_bytes());
        bytes.extend_from_slice(&one);
        let mut two = vec![0u8; 532];
        two[20 + 39 * 4..24 + 39 * 4].copy_from_slice(&0.75f32.to_le_bytes());
        bytes.extend_from_slice(&2i32.to_le_bytes());
        bytes.extend_from_slice(&(two.len() as i64).to_le_bytes());
        bytes.extend_from_slice(&two);
        let mut flex = Flex::default();
        flex.set_state(&bytes).unwrap();
        let state = flex.state.clone().unwrap();
        assert_eq!(
            (state.pack.as_str(), state.preset.as_str()),
            ("Packy", "Pres")
        );
        assert_eq!(flex.param(39), 1200.0);
        // Without the install nothing plays, and it says why.
        assert!(matches!(flex.fidelity(), Fidelity::Approximate(_)));
        flex.note_on(NoteOn::default());
        let (mut l, mut r) = ([0.0; 64], [0.0; 64]);
        flex.process(&mut l, &mut r);
        assert!(l.iter().all(|x| *x == 0.0));
        // A newer version is turned down.
        let mut newer = bytes.clone();
        newer[0] = 2;
        assert_eq!(
            flex.set_state(&newer),
            Err(StateError::Version { found: 2 })
        );
    }

    /// The corpus's FLEX channels play from FL's own packs.
    #[test]
    fn the_corpus_presets_play_from_the_install() {
        let Some(data) = std::env::var_os("FL_RS_CORPUS").map(PathBuf::from) else {
            return;
        };
        let generators = data.join("../Plugins/Fruity/Generators");
        let packs = flex_pack::packs_dir(&data);
        let cases = [
            ("General Midi Library", "Harp"),
            ("Arksun Cityscape", "Tesla Pipe"),
            ("Melodic Techno", "PLK Navid"),
            ("Mobile Tuned 808 Bass", "808 Clean"),
            ("Olbaid Compendium", "Driving Through"),
        ];
        for (pack, preset) in cases {
            if !packs.join(format!("{pack}.flexpack")).is_file() {
                continue;
            }
            let mut bytes = 1i32.to_le_bytes().to_vec();
            let mut one = vec![0u8; 2066];
            one[..pack.len()].copy_from_slice(pack.as_bytes());
            one[1025..1025 + preset.len()].copy_from_slice(preset.as_bytes());
            bytes.extend_from_slice(&1i32.to_le_bytes());
            bytes.extend_from_slice(&(one.len() as i64).to_le_bytes());
            bytes.extend_from_slice(&one);
            let mut flex = Flex::default();
            flex.set_sample_rate(44_100.0);
            flex.set_resource_dir(&generators);
            flex.set_state(&bytes).unwrap();
            assert!(flex.program.is_some(), "{preset}: {:?}", flex.fidelity());
            flex.note_on(NoteOn::default());
            let (mut l, mut r) = (vec![0.0; 22_050], vec![0.0; 22_050]);
            flex.process(&mut l, &mut r);
            let peak = l.iter().chain(&r).fold(0.0f32, |m, x| m.max(x.abs()));
            eprintln!("{preset}: peak {peak}, {:?}", flex.fidelity());
            assert!(l.iter().chain(&r).all(|x| x.is_finite()), "{preset}");
            assert!(peak > 1e-3 && peak < 8.0, "{preset}: peak {peak}");
            flex.note_off(0);
            let mut tail = vec![0.0; flex.tail().min(441_000)];
            let mut tail_r = tail.clone();
            flex.process(&mut tail, &mut tail_r);
        }
    }
}
