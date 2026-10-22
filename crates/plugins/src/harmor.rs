//! Harmor: an additive synthesiser with two parts, as
//! `docs/plugins/harmor.md` reads it out of `Harmor_x64.dll`.
//!
//! The state ([`crate::harmor_state`]) is two parts of 386 parameters, 43
//! articulators and 26 drawn shapes each, and the global block. A note
//! starts a voice for every unison copy of every enabled part. Each voice
//! is a bank of up to 520 sines: slot 0 is filter 1's self-oscillation,
//! slots 1 to 3 the sub partials, and slot 4 on the harmonics.
//!
//! Every control tick (2 ms) a voice runs its articulators
//! ([`crate::harmor_art`]), works out its pitch and the frequency of every
//! partial from the ratio table, mixes the two timbres, runs the spectral
//! units in the part's order, and applies the global EQ and the volume
//! articulator. The bank glides every partial to its new level. The voice
//! is panned into an FX bus and a dry bus by the FX / dry mix, and the
//! buses run the global units in their order.
//!
//! `docs/plugins/harmor.md` section 8 has what is built, step by step.
//! Images and resynthesis, strum, the arpeggiator points and harmonic
//! randomness are left out, and [`Generator::fidelity`] names the first
//! of them a state asks for. The HQ offline render (0.5 ms tick, 520
//! partials, perfect precision) is not used: this plays what FL plays live.

use crate::autogun_engine::rand_uniform;
use crate::harmless::shape;
use crate::harmor_art::{ArtRun, Articulator, EnvRun, Inputs, smoothing};
use crate::harmor_fx::{FX_BLOCK, GlobalFx};
use crate::harmor_state::{
    ARTICULATORS, GLOBAL_PARAMS, GLOBAL_RANGES, KNOB_RANGES, PARAMS, PART_PARAMS, PART_RANGES,
    Part, SMOOTHING_RANGE, State,
};
use crate::maximus::Maximus;
use crate::menvelope::MEnvelope;
use crate::{Effect, Fidelity, Generator, NoteOn, StateError};
use dsp::additive::{SILENT, ShapeTable};
use dsp::curve::Curve;
use std::f64::consts::{PI, TAU};
use std::path::Path;

pub const NAME: &str = "Harmor";

/// Where the compression presets are, under the generators' directory.
pub const COMPRESSION: &str = "Harmor/System/Compression";

/// Slots a bank has.
const PARTIALS: usize = 520;
/// Voices kept, every unison copy one.
const VOICES: usize = 128;
/// Frames rendered at once.
const CHUNK: usize = 256;
/// log2 440, the pitch reference.
const LOG2_440: f64 = 8.781_359_713_524_66;
/// C0 = 440 * 2^(-57/12), the lowest frequency a partial is counted at.
const C0: f64 = 16.351_597_831_287_414;
/// The global EQ's points and their resolution.
const EQ_POINTS: usize = 2420;
/// The unison tables (0x928290).
const K0: [f64; 9] = [
    0.30693, -0.28972, 0.39241, -0.22302, 0.50820, -0.23664, 0.43662, -0.34831, 0.27434,
];
const K1: [f64; 9] = [
    0.89047, 1.07643, 0.91158, 1.10254, 1.03413, 1.16686, 0.85397, 1.20627, 1.0,
];

/// `level3(x) = x == 1 ? 1 : (3^x - 1)/2`.
fn level3(x: f64) -> f64 {
    if x == 1.0 {
        1.0
    } else {
        (3f64.powf(x) - 1.0) * 0.5
    }
}

/// `level11(x) = (11^x - 1)/10`.
fn level11(x: f64) -> f64 {
    (11f64.powf(x) - 1.0) * 0.1
}

fn frac(x: f64) -> f64 {
    x - x.floor()
}

/// `exp(ln 0.01 / n)`: what is left after a step of a fall to 1 % in `n`.
fn fall(n: f64) -> f64 {
    if n > 0.0 {
        (0.01f64.ln() / n).exp()
    } else {
        0.0
    }
}

/// A shape sampled at `count` places evenly over its span, less `centre`
/// (the maps' `vmt+0x68` sampler).
fn linear(env: &MEnvelope, count: usize, centre: f64) -> Vec<f32> {
    let curve = Curve::new(&env.points);
    let start = env.points.first().map_or(0.0, |p| p.x.max(0.0));
    let span: f64 = env.points.iter().skip(1).map(|p| p.x.max(0.0)).sum();
    let span = if span > 0.0 { span } else { 1.0 };
    (0..count)
        .map(|i| {
            let x = start + span * i as f64 / (count as f64 - 1.0).max(1.0);
            (curve.value_at(x) - centre) as f32
        })
        .collect()
}

/// A shape at `x` of its span, from 0 to 1.
fn at(env: &MEnvelope, x: f64, centre: f64) -> f32 {
    let curve = Curve::new(&env.points);
    let start = env.points.first().map_or(0.0, |p| p.x.max(0.0));
    let span: f64 = env.points.iter().skip(1).map(|p| p.x.max(0.0)).sum();
    let span = if span > 0.0 { span } else { 1.0 };
    (curve.value_at(start + span * x) - centre) as f32
}

/// A shape on the log sampler (`FUN_00888ed0`): slot 4 + i is at
/// `log2(i + 1) / 9` of its span. Slots 0 to 3 take slot 4's value.
fn logarithmic(env: &MEnvelope, centre: f64) -> Vec<f32> {
    let mut out = vec![0.0; PARTIALS];
    for i in 0..PARTIALS - 4 {
        out[4 + i] = at(env, ((i + 1) as f64).log2() / 9.0, centre);
    }
    for slot in 0..4 {
        out[slot] = out[4];
    }
    out
}

/// One of filters 1 and 2, as the part sets it.
#[derive(Debug, Clone, PartialEq)]
struct Filter {
    table: ShapeTable,
    identity: bool,
    resonance: Option<ShapeTable>,
    noise: bool,
    lo: f32,
    hi: f32,
    range: f32,
    base: f32,
    width: f32,
    keytrack: f32,
    env_amount: f32,
    adaptive: bool,
    hz: bool,
    res: f32,
    res_hz: bool,
    res_width: f32,
    res_ratio: f32,
    res_octaves: f32,
    res_adaptive: bool,
}

impl Filter {
    fn new(kind: i32, part: &Part) -> Self {
        let table = match kind {
            17 | 18 => {
                let shape = &part.shapes[10 + (kind - 17) as usize];
                let points = linear(shape, 1024, 0.0);
                ShapeTable::new(1024, 0.5, 1.0, 1.0, |i| f64::from(points[i]).powi(2))
            }
            _ => shape(kind),
        };
        let identity = table.points().iter().all(|p| *p == 1.0);
        Filter {
            table,
            identity,
            resonance: None,
            noise: false,
            lo: 0.0,
            hi: 0.0,
            range: 1.0,
            base: 0.0,
            width: 2.0,
            keytrack: 1.0,
            env_amount: 0.0,
            adaptive: false,
            hz: false,
            res: 0.0,
            res_hz: false,
            res_width: 2.0,
            res_ratio: 1.0,
            res_octaves: 0.0,
            res_adaptive: false,
        }
    }

    /// The parameters from REC `51 + 12 f` on (`FUN_00874240`, the
    /// ProcessParam cases).
    fn derive(&mut self, p: &[i32]) {
        let k = |v: i32| v as f32 / 1000.0;
        self.adaptive = p[0] > 0;
        self.hz = p[2] > 0;
        self.width = 0.004 * p[3] as f32;
        self.keytrack = k(p[4]);
        self.env_amount = k(p[5]);
        self.res = k(p[6]);
        self.res_hz = p[7] > 0;
        self.res_width = 0.004 * p[8] as f32;
        self.res_ratio = (p[9] as f32 / 1200.0).exp2();
        self.res_octaves = p[9] as f32 / 1200.0;
        self.res_adaptive = p[10] > 0;
        let u = |reach: f32| (2.0 / 3.0) * (self.width - 1.0).exp2() * reach;
        self.lo = (73.4162f32.log2()) - (u(self.table.below) - 1.0).max(0.0);
        self.hi = (17_739.69f32.log2()) + (u(self.table.above) - 1.0).max(0.0);
        self.range = self.hi - self.lo;
        self.base = self.lo + p[1] as f32 / 8192.0 * self.range;
    }
}

/// A resonance table (`FUN_00874ed0`, IDs 100 to 111): octaves of gain,
/// read as `2^(r C)`. All are centred, reaching one side below.
fn resonance(kind: i32, part: &Part) -> Option<ShapeTable> {
    let rc = |z: f64| (z.cos() + 1.0) * 0.5;
    let t = |i: usize, n: usize| 1.0 - i as f64 / (n as f64 - 1.0);
    let table = match kind {
        100 => ShapeTable::new(1024, 0.5, 1.0, 0.0, |i| {
            7.0 * (1.0 - rc(TAU * t(i, 1024))) - 1.0
        }),
        101 => ShapeTable::new(1536, 0.5, 1.0, 0.0, |i| {
            6.25 * ((i as f64 - 768.0) * 3.0 * PI / 1535.0).cos() * (1.0 - rc(TAU * t(i, 1536)))
        }),
        102 => ShapeTable::new(512, 0.5, 1.0, 0.0, |i| {
            let m = 512 / 6;
            let edge = if i < m {
                1.0 - rc(PI * i as f64 / m as f64)
            } else if i >= 512 - m {
                1.0 - rc(PI * (511 - i) as f64 / m as f64)
            } else {
                1.0
            };
            5.8 * edge
        }),
        103 => ShapeTable::new(512, 0.5, 1.0, 0.0, |i| {
            6.9 * (1.0 - 2.0 * i as f64 / 512.0).abs()
        }),
        104 => ShapeTable::new(1024, 0.5, 1.0, 0.0, |i| {
            let m = 1024 / 4;
            let v = if i < m {
                (i as f64 / m as f64).powi(2)
            } else if i >= 1024 - m {
                ((1023 - i) as f64 / m as f64).powi(2)
            } else {
                1.0
            };
            5.4 * v
        }),
        105 => ShapeTable::new(1536, 0.5, 1.0, 0.0, |i| {
            6.2 * (1.0 - rc(4.0 * PI * t(i, 1536))).powi(2)
        }),
        106 => ShapeTable::new(682, 0.5, 1.0, 0.0, |i| -8.0 * (1.0 - rc(TAU * t(i, 682)))),
        107 => ShapeTable::new(1024, 0.5, 1.0, 0.0, |i| {
            let d = (i as f64 - 512.0) * 2.0 / 1023.0 * TAU;
            let sinc = if i == 512 { 1.0 } else { d.sin() / d };
            let v = 1.0 - rc(TAU * t(i, 1024));
            -10.0 * sinc * v.max(0.0).powf(0.001_25)
        }),
        108 => ShapeTable::new(1024, 0.5, 1.0, 0.0, |i| {
            let c = rc(TAU * t(i, 1024));
            let v = 6.8 * (1.0 - c) - 0.8;
            if i > 512 { v - 3.0 * (c + 1.0) } else { v }
        }),
        110 | 111 => {
            let shape = &part.shapes[12 + (kind - 110) as usize];
            let points = linear(shape, 1024, 0.5);
            ShapeTable::new(1024, 0.5, 1.0, 0.0, |i| f64::from(points[i]) * 24.0)
        }
        _ => return None,
    };
    let mut table = table;
    table.flush();
    Some(table)
}

/// Everything a part is, made ready: the articulators, the tables its
/// shapes bake into and the parameters worked out.
#[derive(Debug, Clone, PartialEq)]
struct PartDsp {
    on: bool,
    arts: Vec<Articulator>,
    /// The two timbres (`(4 s)^2`, timbre 2 after the blend) with the local
    /// EQ in them when it is the first unit.
    timbre: [Vec<f32>; 2],
    local_eq: Vec<f32>,
    /// The harmonic phase table, in turns.
    phase: Vec<f64>,
    ratio: Vec<f64>,
    unison_shape: Option<Vec<f32>>,
    pluck_shape: Vec<f32>,
    protect: Vec<f32>,
    protect_top: usize,
    clip: Vec<f32>,
    clip_on: bool,
    eq: ShapeTable,
    top_hz: f64,
    filters: [Filter; 2],
    /// The spectral units in their order, without those not built.
    units: [u8; 12],
    unit_count: usize,
    first_filter: usize,
    // From the parameters.
    pitch_sum: f32,
    main_pitch: f32,
    detune: f64,
    neg_log2_d: f64,
    oct: bool,
    below: bool,
    pitch_env: f32,
    vibrato: f32,
    vibrato_rate: f64,
    gain: f32,
    volume: f32,
    subs: [f32; 4],
    fx_mix: f32,
    filter_mix: f32,
    timbre_mix: f32,
    pluck: f32,
    protect_gain: f32,
    clip_level: f32,
    clip_mode: i32,
    unison: usize,
    unison_type: i32,
    unison_cents: f32,
    unison_level: f32,
    unison_pan: f32,
    unison_phase: f32,
    offsets: [f32; 9],
    phase_start: f64,
    phase_random: f64,
    ramp: f64,
    ramp_partial: bool,
    smooth: [f32; ARTICULATORS],
    vel_to_attack: bool,
    vel_to_release: bool,
    more: More,
}

/// The tables and settings of the units past the filters.
#[derive(Debug, Clone, PartialEq)]
struct More {
    /// Masks: filter 1, 2, resonance 1, 2, phaser, harmonizer (shapes 18
    /// to 23), none where the shape is flat at 1.
    masks: [Option<ShapeTable>; 6],
    phaser: ShapeTable,
    phaser_freq: bool,
    phaser_mix: f32,
    phaser_scale: i32,
    phaser_width: f32,
    phaser_offset: f32,
    phaser_keytrack: f32,
    phaser_speed: f64,
    /// The harmonizer's distribution at its nine steps.
    spread: [f32; 9],
    harm_mix: f32,
    harm_width: f32,
    harm_strength: f32,
    harm_pattern: [i32; 4],
    prism: Vec<f32>,
    prism_amount: f32,
    prism_multiply: bool,
    prism_from: i32,
    auto_gain: f32,
    tremolo: f32,
    tremolo_rate: f64,
    tremolo_gap: f64,
    /// Blur: the time blur's attack and decay, the harmonic blur's wheel
    /// and its two tensions, the mix, and the mask (shape 24).
    blur_att: f32,
    blur_dec: f32,
    blur_amount: f32,
    blur_tension: [f64; 2],
    blur_mix: f32,
    blur_mask: Option<ShapeTable>,
    /// Glide time in ticks; negative for the linear glide.
    glide: f64,
    glide_limit: f32,
}

/// A mask (`FUN_00874ed0` IDs 400..800): 2160 points at 240 to the
/// octave from 32.6 Hz, squared. None for one flat at 1.
fn mask(shape: &MEnvelope) -> Option<ShapeTable> {
    if shape.flat() == Some(1.0) {
        return None;
    }
    let points = linear(shape, 2160, 0.0);
    Some(ShapeTable::new(2160, 0.0, 0.0, 0.0, |i| {
        f64::from(points[i]).powi(2)
    }))
}

/// Where a mask is read for a partial at `log2_hz`.
fn mask_at(table: &ShapeTable, log2_hz: f32) -> f32 {
    table.clamped(log2_hz, 240.0, -1206.5)
}

/// The phaser tables 200..210 (`FUN_00874ed0`): Harmless's for 200..208,
/// shape 15 for the frequency (209, centred, twice) and custom (210) ones.
fn phaser_table(kind: i32, custom: &MEnvelope) -> ShapeTable {
    match kind {
        209 | 210 => {
            let freq = kind == 209;
            let points = linear(custom, 1024, if freq { 0.5 } else { 0.0 });
            let mut t = ShapeTable::new(1024, 0.0, 1.0, 0.0, |i| {
                f64::from(points[i]) * if freq { 2.0 } else { 1.0 }
            });
            t.normalise_rms(1.0);
            t.flush();
            t
        }
        200..=208 => shape(kind),
        _ => shape(200),
    }
}

impl More {
    fn new(part: &Part) -> Self {
        let s = &part.shapes;
        let kind = part.ints[10].clamp(200, 210);
        let spread = std::array::from_fn(|k| {
            let v = at(&s[16], k as f64 / 8.0, 0.0);
            ((3f64.powf(2.0 * f64::from(v)) - 1.0) * 0.5) as f32
        });
        let mut prism = logarithmic(&s[7], 0.5);
        for v in &mut prism {
            *v *= 2.0;
        }
        prism[0] = 0.0;
        More {
            masks: std::array::from_fn(|k| mask(&s[18 + k])),
            phaser: phaser_table(kind, &s[15]),
            phaser_freq: kind == 209,
            phaser_mix: 0.0,
            phaser_scale: 0,
            phaser_width: 0.5,
            phaser_offset: 0.5,
            phaser_keytrack: 1.0,
            phaser_speed: 0.0,
            spread,
            harm_mix: 0.0,
            harm_width: 0.5,
            harm_strength: 2.662,
            harm_pattern: [0; 4],
            prism,
            prism_amount: 0.0,
            prism_multiply: part.ints[3] == 1,
            prism_from: part.ints[4],
            auto_gain: 0.0,
            tremolo: 0.0,
            tremolo_rate: 0.0,
            tremolo_gap: 0.0,
            blur_att: 0.0,
            blur_dec: 0.0,
            blur_amount: 0.0,
            blur_tension: [1000.0; 2],
            blur_mix: 1.0,
            blur_mask: mask(&s[24]),
            glide: 0.0,
            glide_limit: 1200.0,
        }
    }

    fn derive(&mut self, p: &[i32], ctx: &Ctx) {
        let k = |v: i32| v as f32 / 1000.0;
        self.phaser_mix = k(p[76]);
        self.phaser_scale = p[77];
        self.phaser_width = p[78] as f32 / 10_000.0;
        self.phaser_offset = p[79] as f32 / 1000.0;
        self.phaser_keytrack = k(p[81]);
        let beat = ctx.tempo / (ctx.rate * 3840.0);
        self.phaser_speed =
            f64::from(p[80].signum()) * (f64::from(p[80].abs()) / 1000.0).exp2() * beat;
        self.harm_mix = k(p[27]);
        self.harm_width = k(p[28]);
        self.harm_strength = (1.331 * (p[29] as f64 / 256.0).exp2()) as f32;
        self.harm_pattern = [p[30], p[31], p[32], p[33]];
        self.prism_amount = k(p[26]);
        self.auto_gain = k(p[13]) * k(p[13]);
        self.tremolo = k(p[17]);
        self.tremolo_rate = (6.0 - f64::from(p[18]) / 1000.0).exp2() / ctx.rate;
        self.tremolo_gap = frac(f64::from(p[19]) / 1000.0) * 0.5;
        let t = ctx.tick as f64;
        let coefficient = |w: i32| {
            let n = ((f64::from(w) / 100.0).exp2() - 1.0) * 0.001 * ctx.rate / t;
            if n > 0.0 { fall(n) as f32 } else { 0.0 }
        };
        self.blur_att = coefficient(p[21]);
        self.blur_dec = coefficient(p[22]);
        self.blur_amount = p[23] as f32 / 10_000.0;
        self.blur_tension = [p[24], p[25]].map(|v| 1_000_001f64.powf(f64::from(v) / 1000.0));
        self.blur_mix = k(p[20]) * k(p[20]);
        let v = f64::from(p[49]);
        let ticks = if v < 0.0 {
            ctx.rate * ((-v / 64.0).exp2() - 1.0) / (t * 42.0)
        } else {
            ctx.rate * v * 0.3125 / (ctx.tempo * t)
        };
        // The linear glide when the curve box is off.
        self.glide = if p[47] == 0 { -ticks } else { ticks };
        self.glide_limit = p[50] as f32;
    }
}

impl PartDsp {
    fn new(part: &Part) -> Self {
        let arts = part
            .arts
            .iter()
            .enumerate()
            .map(|(a, s)| Articulator::new(s, a))
            .collect();
        let mut dsp = PartDsp {
            on: part.enabled,
            arts,
            timbre: [vec![0.0; PARTIALS], vec![0.0; PARTIALS]],
            local_eq: vec![1.0; PARTIALS],
            phase: vec![0.0; PARTIALS],
            ratio: vec![0.0; PARTIALS],
            unison_shape: None,
            pluck_shape: vec![0.0; PARTIALS],
            protect: vec![0.0; PARTIALS],
            protect_top: 0,
            clip: vec![0.0; PARTIALS],
            clip_on: false,
            eq: ShapeTable::new(EQ_POINTS, 0.0, 0.0, 0.0, |_| 1.0),
            top_hz: 17_000.0,
            filters: [
                Filter::new(part.filter_types[0], part),
                Filter::new(part.filter_types[1], part),
            ],
            units: [0; 12],
            unit_count: 0,
            first_filter: 0,
            pitch_sum: 0.0,
            main_pitch: 0.0,
            detune: 1.0,
            neg_log2_d: 0.0,
            oct: false,
            below: false,
            pitch_env: 0.0,
            vibrato: 0.0,
            vibrato_rate: 0.0,
            gain: 0.5,
            volume: 1.0,
            subs: [0.0; 4],
            fx_mix: 0.0,
            filter_mix: 0.0,
            timbre_mix: 0.0,
            pluck: 1.0,
            protect_gain: 0.0,
            clip_level: 1.0,
            clip_mode: 0,
            unison: 1,
            unison_type: 0,
            unison_cents: 0.0,
            unison_level: 1.0,
            unison_pan: 0.0,
            unison_phase: 0.0,
            offsets: [0.0; 9],
            phase_start: 0.5,
            phase_random: 0.0,
            ramp: 0.0,
            ramp_partial: false,
            smooth: [0.0; ARTICULATORS],
            vel_to_attack: false,
            vel_to_release: false,
            more: More::new(part),
        };
        for f in 0..2 {
            let kind = part.resonance_types[f];
            dsp.filters[f].noise = kind == 109;
            dsp.filters[f].resonance = resonance(kind, part);
        }
        dsp.tables(part);
        dsp
    }

    /// The tables the shapes bake into (`FUN_0088f310`, `FUN_00889ec0`,
    /// `FUN_00874ed0` for the global EQ). This allocates.
    fn tables(&mut self, part: &Part) {
        let p = &part.params;
        let s = &part.shapes;
        let detune = f64::from(p[42]) / (f64::from(p[43].max(1)) * 10_000.0);
        let oct = part.ints[7] == 0;
        let below = part.ints[6] == 1;
        // Ratios (FUN_0088fcf0).
        self.ratio[0] = 1.0;
        self.ratio[1] = 0.5;
        if below {
            self.ratio[2] = 0.25;
            self.ratio[3] = 0.125;
        } else {
            self.ratio[2] = 1.0 + 0.5 * detune;
            self.ratio[3] = 1.0 + 1.5 * detune;
        }
        for j in 0..PARTIALS - 4 {
            self.ratio[4 + j] = if oct {
                (j as f64 * detune / 8.0).exp2()
            } else {
                1.0 + j as f64 * detune
            };
        }
        // Timbres: (4 s)^2 on the linear sampler, subs at harmonic 1.5 and
        // 2.5 (or 1 below).
        let sub_at = if below { [0.0, 0.0] } else { [0.5, 1.5] };
        for (t, shape) in [&s[2], &s[3]].iter().enumerate() {
            let points = linear(shape, PARTIALS - 4, 0.0);
            let table = &mut self.timbre[t];
            table[4..].copy_from_slice(&points);
            table[2] = at(shape, sub_at[0] / 515.0, 0.0);
            table[3] = at(shape, sub_at[1] / 515.0, 0.0);
            for v in table.iter_mut().skip(1) {
                *v = (*v * 4.0).powi(2);
            }
            table[0] = 1.0;
            table[1] = table[4];
        }
        // The blend rewrites timbre 2.
        let blend = part.ints[0];
        let pluck_shape = logarithmic(&s[14], 0.0);
        for (i, pluck) in pluck_shape.iter().enumerate() {
            let (a, b) = (self.timbre[0][i], self.timbre[1][i]);
            self.timbre[1][i] = match blend {
                1 => (a - b).max(0.0),
                2 => a * b,
                3 => a.max(b),
                4 => a.min(b),
                5 => (b * (16.0 * (pluck - 1.0)).exp2()).max(2f32.powi(-16)),
                _ => b,
            };
        }
        // The local EQ (FUN_00889ec0) on the log sampler.
        let eq = logarithmic(&s[0], 0.0);
        for i in 0..PARTIALS {
            self.local_eq[i] = if oct {
                eq[i] * eq[i]
            } else {
                let e = (4.0 * eq[i]).powi(2);
                match i {
                    0 => 1.0,
                    1 => e,
                    2 | 3 => {
                        let t = if below { 0.0 } else { sub_at[i - 2] };
                        e / (t as f32 + 1.0)
                    }
                    _ => e / (i - 3) as f32,
                }
            };
        }
        let eq_first = part.order[0] == 0;
        if eq_first {
            for t in 0..2 {
                for i in 0..PARTIALS {
                    self.timbre[t][i] *= self.local_eq[i];
                }
            }
        }
        // Harmonic phase: shape 1, centred, s' + 1 turns.
        let phase = linear(&s[1], PARTIALS - 4, 0.5);
        for (to, from) in self.phase[4..].iter_mut().zip(&phase) {
            *to = f64::from(*from) + 1.0;
        }
        self.phase[0] = 0.0;
        self.phase[1] = self.phase[4];
        self.phase[2] = self.phase[4];
        self.phase[3] = self.phase[4];
        // The unison pitch shape, when it is drawn: (2 s)^2.
        self.unison_shape = match s[8].flat() {
            Some(v) if (f64::from(v) - 0.5).abs() < 1e-9 => None,
            _ => Some(
                logarithmic(&s[8], 0.0)
                    .iter()
                    .map(|v| (2.0 * v).powi(2))
                    .collect(),
            ),
        };
        // Pluck rates come from shape 14.
        self.pluck_shape = pluck_shape;
        // Protection (shape 5, squared) up to its last point above 0.
        let protect = logarithmic(&s[5], 0.0);
        self.protect_top = protect.iter().rposition(|v| *v != 0.0).map_or(0, |i| i + 1);
        for (to, from) in self.protect.iter_mut().zip(&protect) {
            *to = from * from;
        }
        // Clipping (shape 6): (2 s)^2 K[mode]; modes 1 and 2 keep the
        // reciprocal.
        let mode = part.ints[5].clamp(0, 4);
        let k = [4.0f32, 128.0, 32.0, 2.0, 2.0][mode as usize];
        let clip = logarithmic(&s[6], 0.0);
        self.clip_on = clip.iter().any(|v| *v > 0.0);
        for (to, from) in self.clip.iter_mut().zip(&clip) {
            let c = (2.0 * from).powi(2) * k;
            *to = if mode == 1 || mode == 2 {
                1.0 / c.max(SILENT)
            } else {
                c
            };
        }
        // The global EQ (shape 17, ID 300).
        let g = part.global_eq_mix();
        let mut eq = vec![0.0f32; EQ_POINTS];
        let points = linear(&s[17], 2161, 0.0);
        for (i, v) in points.iter().enumerate() {
            eq[240 + i] = if g == 1.0 {
                *v
            } else {
                g * v + 0.25 * (1.0 - g)
            };
        }
        let rc = |z: f64| ((z.cos() + 1.0) * 0.5) as f32;
        let low = eq[240];
        for (i, v) in eq.iter_mut().enumerate().take(240) {
            *v = low * rc(PI * (1.0 - i as f64 / 240.0));
        }
        let high = eq[2400];
        for j in 1..EQ_POINTS - 2400 {
            eq[2400 + j] = high * rc(PI * j as f64 / 19.0);
        }
        for v in &mut eq {
            *v = (4.0 * *v).powi(2);
        }
        let last = eq.iter().rposition(|v| *v > 0.0).unwrap_or(0);
        self.top_hz = (4.031_359_7 + last as f64 / 240.0).exp2();
        self.eq = ShapeTable::new(EQ_POINTS, 0.0, 0.0, 0.0, |i| f64::from(eq[i]));
        self.eq.flush();
        // The unit list (FUN_008737e0).
        let mut later = 5;
        for id in part.order {
            if id == 4 || id == 5 {
                later = id;
            }
        }
        self.first_filter = ((later - 4) ^ 1) as usize;
        let mut count = 0;
        let last_unit = part.order[11];
        for (n, id) in part.order.iter().enumerate() {
            let keep = match id {
                0 => n != 0,
                7 => !part.pluck_to_blur,
                9 => self.protect_top > 0,
                10 => self.clip_on,
                11 => last_unit != 11,
                1..=11 => true,
                _ => false,
            };
            if keep {
                self.units[count] = *id as u8;
                count += 1;
            }
        }
        self.unit_count = count;
        self.oct = oct;
        self.below = below;
    }

    /// What the parameters make of the part (the ProcessParam cases).
    /// This allocates nothing.
    fn derive(&mut self, part: &Part, globals: &[i32; GLOBAL_PARAMS], share: f64, ctx: &Ctx) {
        let p = &part.params;
        let k = |v: i32| f64::from(v) / 1000.0;
        let main_pitch = globals[14] as f32;
        let mul = f64::from(p[40]) / (f64::from(p[41].max(1)) * 10_000.0);
        self.main_pitch = main_pitch;
        self.pitch_sum = main_pitch + (1200.0 * mul.log2() - 1200.0) as f32;
        self.detune = f64::from(p[42]) / (f64::from(p[43].max(1)) * 10_000.0);
        self.neg_log2_d = -self.detune.log2();
        self.pitch_env = 2.0 * p[44] as f32;
        self.vibrato = 0.1 * p[45] as f32;
        self.vibrato_rate = (3.0 - k(p[46])).exp2() / ctx.rate;
        // Part mix and volumes.
        let vol = p[11];
        self.gain =
            (share * level3(k(globals[11])) * f64::from(vol.signum()) * level3(k(vol.abs())))
                as f32;
        self.volume = level3(k(p[12])) as f32;
        self.subs = [
            (p[62] as f32 / 1024.0).powi(2),
            p[5] as f32 / 1024.0,
            p[6] as f32 / 1024.0,
            p[7] as f32 / 1024.0,
        ];
        self.fx_mix = k(p[10]) as f32;
        self.filter_mix = k(p[74]) as f32;
        self.timbre_mix = k(p[4]) as f32;
        self.pluck = k(p[75]) as f32;
        self.protect_gain = level11(f64::from(p[8]) / 1024.0) as f32;
        self.clip_level = p[9] as f32 / 1024.0;
        self.clip_mode = part.ints[5].clamp(0, 4);
        for f in 0..2 {
            self.filters[f].derive(&p[51 + 12 * f..62 + 12 * f]);
        }
        // Unison (FUN_00873e40).
        let n = p[34].clamp(1, 9) as usize;
        self.unison = n;
        self.unison_type = p[35].clamp(0, 4);
        let nf = n as f64;
        self.unison_level = if n < 2 {
            1.0
        } else {
            (1.0 - (3f64.powf(1.0 - 1.0 / nf) - 1.0) * 0.5) as f32
        };
        self.unison_cents = ((41f64.powf(f64::from(p[38]) / 256.0) - 1.0) * 1.875) as f32;
        self.unison_pan = if p[37] <= 192 {
            p[37] as f32 / 192.0
        } else {
            1.0 + 0.046_875 * (p[37] - 192) as f32
        };
        self.unison_phase = k(p[39]) as f32;
        let mut offsets = [0.0f64; 9];
        if n > 1 {
            for (i, o) in offsets.iter_mut().enumerate().take(n) {
                let fi = i as f64;
                let negate = (i % 2 == 1) ^ (n % 2 == 1 && 2 * i > n);
                let sign = if negate { -1.0 } else { 1.0 };
                let d = (2.0 * fi / (nf - 1.0) - 1.0).abs() * sign;
                *o = match self.unison_type {
                    0 => d + K0[i] / (nf - 1.0),
                    1 => d * K1[i],
                    2 => d,
                    3 => (2.0 * (fi + 0.25) / (nf - 0.5) - 1.0).abs() * sign,
                    _ => (2.0 * (fi + 0.5) / nf - 1.0).abs() * sign,
                };
            }
            if self.unison_type <= 1 {
                let mean = offsets[..n].iter().sum::<f64>() / nf;
                for o in &mut offsets[..n] {
                    *o -= mean;
                }
            }
        }
        self.offsets = offsets.map(|o| o as f32);
        self.phase_start = k(p[0]);
        let r = k(p[1]);
        self.phase_random = r.signum() * r * r;
        // Ramping (FUN_00878030).
        let v = f64::from(part.ints[2].clamp(-2000, 2000));
        let f = if v.abs() == 1000.0 {
            1.0
        } else {
            (7f64.powf(v.abs() / 1000.0) - 1.0) / 6.0
        };
        self.ramp = (f * 25.0 * 0.001 * ctx.rate).max(1.0);
        self.ramp_partial = v > 0.0;
        for a in 0..ARTICULATORS {
            self.smooth[a] = smoothing(p[343 + a], ctx.rate, ctx.tick as f64);
            self.arts[a].smoothing = self.smooth[a];
        }
        self.vel_to_attack = p[14] > 0;
        self.vel_to_release = p[16] > 0;
        self.more.derive(p, ctx);
    }
}

/// What every voice reads that is not its part's.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Ctx {
    rate: f64,
    tempo: f64,
    tick: usize,
    /// Pitch and cutoff smoothing per tick.
    pitch_glide: f32,
    cut_glide: f32,
    /// The output gain smoothing per sample (15 ms to 1 %).
    gain_glide: f32,
    lfo_level: f32,
    mod_xyz: [f32; 3],
    nmax: usize,
    safety: f64,
    voice_random: i32,
}

impl Ctx {
    fn rates(&self) -> [f64; 2] {
        let t = self.tick as f64;
        [2.0 * t / self.rate, t * self.tempo / (60.0 * self.rate)]
    }
}

/// The bank of sines (`TFastToneBank`): each partial is the recursion
/// `y = 2 cos(w) y1 - y2`, and its level moves to its target by its own
/// one-pole every sample. As in FL the recursion runs on from block to
/// block and is put back on the partial's phase when its frequency
/// changes, when it wakes, or every 8192 samples; FL resyncs every
/// `round(8192 f)`. Partials are summed eight at a time.
#[derive(Debug, Clone, PartialEq)]
struct Bank {
    phase: Vec<f64>,
    freq: Vec<f64>,
    level: Vec<f32>,
    target: Vec<f32>,
    coef: Vec<f32>,
    /// The recursion: its two last values, its coefficient, the frequency
    /// it was set for, and samples to the next resync.
    y1: Vec<f32>,
    y2: Vec<f32>,
    c: Vec<f32>,
    set_for: Vec<f64>,
    resync: Vec<i32>,
    /// The partials that sound this block, in order.
    live: Vec<u16>,
    len: usize,
    /// A partial is dropped under this: -96 dB at the average precision,
    /// 2^-24 above it.
    floor: f32,
}

/// Samples between two resyncs of a partial.
const RESYNC: i32 = 8192;

impl Bank {
    fn new() -> Self {
        Bank {
            phase: vec![0.0; PARTIALS],
            freq: vec![0.0; PARTIALS],
            level: vec![0.0; PARTIALS],
            target: vec![0.0; PARTIALS],
            coef: vec![0.0; PARTIALS],
            y1: vec![0.0; PARTIALS],
            y2: vec![0.0; PARTIALS],
            c: vec![0.0; PARTIALS],
            set_for: vec![f64::NAN; PARTIALS],
            resync: vec![0; PARTIALS],
            live: vec![0; PARTIALS],
            len: 0,
            floor: SILENT,
        }
    }

    fn reset(&mut self) {
        self.phase.fill(0.0);
        self.freq.fill(0.0);
        self.level.fill(0.0);
        self.target.fill(0.0);
        self.set_for.fill(f64::NAN);
        self.len = 0;
    }

    fn render(&mut self, out: &mut [f32]) {
        let n = out.len();
        let len = self.len;
        let floor = self.floor;
        let mut live = 0;
        // Which partials sound, and the recursion put on their phase where
        // it has to be.
        for k in 0..len {
            let freq = self.freq[k];
            let quiet = self.level[k].abs() < floor && self.target[k].abs() < floor;
            if quiet || !(0.0..0.5).contains(&freq) {
                self.level[k] = 0.0;
                self.set_for[k] = f64::NAN;
                continue;
            }
            if self.set_for[k] != freq || self.resync[k] <= 0 {
                let phase = self.phase[k];
                self.c[k] = (2.0 * (TAU * freq).cos()) as f32;
                self.y1[k] = (TAU * (phase - freq)).sin() as f32;
                self.y2[k] = (TAU * (phase - 2.0 * freq)).sin() as f32;
                self.set_for[k] = freq;
                self.resync[k] = RESYNC;
            }
            self.resync[k] -= n as i32;
            self.live[live] = k as u16;
            live += 1;
        }
        let mut k0 = 0;
        while k0 < live {
            let lanes = (live - k0).min(8);
            let mut y1 = [0.0f32; 8];
            let mut y2 = [0.0f32; 8];
            let mut c = [0.0f32; 8];
            let mut a = [0.0f32; 8];
            let mut t = [0.0f32; 8];
            let mut kk = [0.0f32; 8];
            for l in 0..lanes {
                let k = usize::from(self.live[k0 + l]);
                y1[l] = self.y1[k];
                y2[l] = self.y2[k];
                c[l] = self.c[k];
                a[l] = self.level[k];
                t[l] = self.target[k];
                kk[l] = self.coef[k];
            }
            for sample in out.iter_mut() {
                let mut sum = 0.0;
                for l in 0..8 {
                    let y = c[l] * y1[l] - y2[l];
                    y2[l] = y1[l];
                    y1[l] = y;
                    sum += a[l] * y;
                    a[l] = t[l] + (a[l] - t[l]) * kk[l];
                }
                *sample += sum;
            }
            for l in 0..lanes {
                let k = usize::from(self.live[k0 + l]);
                self.y1[k] = y1[l];
                self.y2[k] = y2[l];
                self.level[k] = a[l];
            }
            k0 += 8;
        }
        for k in 0..len {
            let phase = self.phase[k] + self.freq[k] * n as f64;
            self.phase[k] = phase - phase.floor();
        }
    }
}

/// One voice: a unison copy of a note in one part.
#[derive(Debug, Clone, PartialEq)]
struct Voice {
    active: bool,
    id: u32,
    part: usize,
    unison: usize,
    age: u64,
    /// 1 held, 6 released, 9 fading out.
    state: u8,
    fade: usize,
    key_pitch: f32,
    vel01: f32,
    vel_gain: f32,
    pan: f32,
    mod_x: f32,
    mod_y: f32,
    release: f32,
    held: f32,
    random: [f32; ARTICULATORS],
    runs: Vec<ArtRun>,
    art: [f32; ARTICULATORS],
    countdown: usize,
    /// The frames from the first tick to the second when they are not a
    /// tick's, and the first tick's step: a voice whose shared envelopes
    /// follow another's takes that voice's countdown (`0x883cd0` stores
    /// `-|count|` at +0x3c; `0x88ab10` turns a negative count into a first
    /// tick that does not move, and 0 into an ordinary one).
    first_len: Option<usize>,
    first_dt: f64,
    /// A shared envelope follows another voice's (+0x3330): its attack is
    /// not jumped into.
    linked: bool,
    first: bool,
    sp: f32,
    detune: f32,
    pan_offset: f32,
    level: f32,
    unison_x: f32,
    count: usize,
    f0: f64,
    log2_f0: f32,
    freq: Vec<f64>,
    hz: Vec<f32>,
    log2_hz: Vec<f32>,
    levels: Vec<f32>,
    mix: Vec<f32>,
    save: Vec<f32>,
    curve: Vec<f32>,
    pluck: Vec<f32>,
    pluck_rate: Vec<f32>,
    pluck_x: f32,
    pluck_ticks: i32,
    cut_base: [f32; 2],
    /// The phaser motion when the tick ran, in turns.
    motion: f64,
    /// The legato and portamento glide: cents from the target, and its
    /// two poles or its linear step and ticks left.
    glide: [f64; 2],
    glide_step: f64,
    glide_left: i32,
    /// The notes a legato voice has taken over and will go back to.
    stack: [(u32, f32); 8],
    stacked: usize,
    /// Samples since the start, for the tremolo fade-in.
    age_samples: usize,
    /// The noise resonance of each filter: its width, rate, step, phase
    /// and seed (voice+0x280..+0x298).
    noise_p: [f32; 2],
    noise_rate: [f64; 2],
    noise_step: [f64; 2],
    noise_acc: [f64; 2],
    noise_seed: [u32; 2],
    /// The noise resonance's numbers, one a partial.
    noise_buf: Vec<f32>,
    blur: Vec<f32>,
    blur_len: usize,
    /// The Hz each partial's ramp was worked out for.
    ramp_hz: Vec<f32>,
    cut: [f32; 2],
    res: [f32; 2],
    bank: Bank,
    gains: [f32; 4],
    aims: [f32; 4],
    mono: [f32; CHUNK],
}

impl Voice {
    fn new() -> Self {
        Voice {
            active: false,
            id: 0,
            part: 0,
            unison: 0,
            age: 0,
            state: 0,
            fade: 0,
            key_pitch: 0.0,
            vel01: 0.78,
            vel_gain: 1.0,
            pan: 0.0,
            mod_x: 0.0,
            mod_y: 0.0,
            release: 0.5,
            held: 0.0,
            random: [0.0; ARTICULATORS],
            runs: Vec::with_capacity(ARTICULATORS),
            art: [0.0; ARTICULATORS],
            countdown: 0,
            first_len: None,
            first_dt: 0.0,
            linked: false,
            first: true,
            sp: 0.0,
            detune: 0.0,
            pan_offset: 0.0,
            level: 1.0,
            unison_x: 0.5,
            count: 0,
            f0: 0.0,
            log2_f0: 0.0,
            freq: vec![0.0; PARTIALS],
            hz: vec![0.0; PARTIALS],
            log2_hz: vec![0.0; PARTIALS],
            levels: vec![0.0; PARTIALS],
            mix: vec![0.0; PARTIALS],
            save: vec![0.0; PARTIALS],
            curve: vec![0.0; PARTIALS],
            pluck: vec![1.0; PARTIALS],
            pluck_rate: vec![1.0; PARTIALS],
            pluck_x: f32::NAN,
            pluck_ticks: 0,
            cut_base: [0.0; 2],
            motion: 0.0,
            glide: [0.0; 2],
            glide_step: 0.0,
            glide_left: 0,
            stack: [(0, 0.0); 8],
            stacked: 0,
            age_samples: 0,
            noise_p: [f32::NAN; 2],
            noise_rate: [0.0; 2],
            noise_step: [0.0; 2],
            noise_acc: [0.0; 2],
            noise_seed: [0x1234_5678, 0x8765_4321],
            noise_buf: vec![0.0; PARTIALS],
            blur: vec![0.0; PARTIALS],
            blur_len: 0,
            ramp_hz: vec![f32::NAN; PARTIALS],
            cut: [0.0; 2],
            res: [0.0; 2],
            bank: Bank::new(),
            gains: [0.0; 4],
            aims: [0.0; 4],
            mono: [0.0; CHUNK],
        }
    }
}

/// `G(p, g)` of `FUN_007a4d50` law 0.
fn pan_gain(pan: f32, g: f32) -> [f32; 2] {
    if pan == 0.0 {
        return [g, g];
    }
    let angle = (1.0 + f64::from(pan)) * PI / 4.0;
    let g = f64::from(g) * std::f64::consts::SQRT_2;
    [(g * angle.cos()) as f32, (g * angle.sin()) as f32]
}

/// Harmor.
pub struct Harmor {
    state: State,
    ctx: Ctx,
    parts: [PartDsp; 2],
    voices: Vec<Voice>,
    next_age: u64,
    seed: u32,
    sample_count: u64,
    vibrato: [f64; 2],
    motion: [f64; 2],
    tremolo: [f64; 2],
    last_pitch: Option<f32>,
    post: f32,
    compression: Option<Box<Maximus>>,
    compression_states: [Option<Vec<u8>>; 4],
    missing_compression: bool,
    fx: [[f32; CHUNK]; 2],
    dry: [[f32; CHUNK]; 2],
    units: Box<GlobalFx>,
    /// The held voices changed since the arpeggiator last ranked them
    /// (this+0xe90).
    arp_dirty: bool,
    /// Every global LFO's phase (flag 2, the TMLFO's +0xa8): where its
    /// voices start, moved on with each block (`Gen_Render`'s list at
    /// this+0x2cf0).
    lfo_phase: [[u32; ARTICULATORS]; 2],
}

impl Default for Harmor {
    fn default() -> Self {
        let state = State::default();
        let parts = [PartDsp::new(&state.parts[0]), PartDsp::new(&state.parts[1])];
        let mut plugin = Harmor {
            state,
            ctx: Ctx {
                rate: 44_100.0,
                tempo: 120.0,
                tick: 88,
                pitch_glide: 0.0,
                cut_glide: 0.0,
                gain_glide: 0.0,
                lfo_level: 1.0,
                mod_xyz: [0.5; 3],
                nmax: PARTIALS,
                safety: 0.0625,
                voice_random: 1,
            },
            parts,
            voices: (0..VOICES).map(|_| Voice::new()).collect(),
            next_age: 0,
            seed: 0x2545_f491,
            sample_count: 0,
            vibrato: [0.0; 2],
            motion: [0.0; 2],
            tremolo: [0.0; 2],
            last_pitch: None,
            post: 1.0,
            compression: None,
            compression_states: [None, None, None, None],
            missing_compression: false,
            fx: [[0.0; CHUNK]; 2],
            dry: [[0.0; CHUNK]; 2],
            units: Box::new(GlobalFx::new(44_100.0)),
            arp_dirty: false,
            lfo_phase: [[0; ARTICULATORS]; 2],
        };
        plugin.derive();
        plugin
    }
}

impl Harmor {
    /// Rank the held voices for the envelopes' arpeggiator points
    /// (`Gen_Render` `0x891320`, after a voice starts or is released):
    /// the held keys, each once, in order (InvArpBox, REC 788, turns it
    /// over); every held voice's main-list envelopes learn the place of
    /// its key and how many there are (`FUN_007a24b0`). Only when an
    /// envelope of the main list has such points (this+0x113d, set by
    /// `0x8974d0`). This allocates nothing.
    fn rank_held(&mut self) {
        if !std::mem::take(&mut self.arp_dirty) {
            return;
        }
        let gate = (0..2).any(|p| self.parts[p].arts[..31].iter().any(|a| a.envelope.arp_on));
        if !gate {
            return;
        }
        let down = self.state.globals.params[16];
        let mut keys = [0f32; VOICES];
        let mut n = 0;
        for v in self.voices.iter().filter(|v| v.active && v.state < 6) {
            let key = v.key_pitch;
            if keys[..n].contains(&key) {
                continue;
            }
            let at = keys[..n]
                .iter()
                .position(|k| i32::from(key < *k) != down)
                .unwrap_or(n);
            keys.copy_within(at..n, at + 1);
            keys[at] = key;
            n += 1;
        }
        for v in self.voices.iter_mut().filter(|v| v.active && v.state < 6) {
            // FL stops looking at the last key.
            let last = n.saturating_sub(1);
            let rank = keys[..last]
                .iter()
                .position(|k| *k == v.key_pitch)
                .unwrap_or(last);
            let dsp = &self.parts[v.part];
            for a in 0..31 {
                dsp.arts[a]
                    .envelope
                    .rank(&mut v.runs[a].env, rank as i32, n as i32);
            }
        }
    }

    /// The state the plugin has now.
    pub fn state_version(&self) -> u32 {
        self.state.version
    }

    /// Everything that follows from a new state: the articulators and
    /// tables. This allocates.
    fn load(&mut self) {
        for voice in &mut self.voices {
            voice.active = false;
        }
        self.parts = [
            PartDsp::new(&self.state.parts[0]),
            PartDsp::new(&self.state.parts[1]),
        ];
        *self.units = GlobalFx::new(self.ctx.rate);
        self.derive();
        self.build_compression();
        self.restart_lfos();
    }

    /// Put every global LFO at its start (`FUN_007a33e0`).
    fn restart_lfos(&mut self) {
        for p in 0..2 {
            for (a, art) in self.parts[p].arts.iter().enumerate() {
                self.lfo_phase[p][a] = art.lfo.start_phase();
            }
        }
    }

    /// Move the main list's global LFOs on by `len` frames, at the pace
    /// of their tick: `round(len / tick x pace)` (`Gen_Render`
    /// `0x892376`; the list at this+0x2cf0 is built by `0x8974d0`). A
    /// frozen LFO's phase moves too: FL reads the pace it keeps before
    /// freezing (+0x98).
    fn move_lfos(&mut self, len: usize) {
        let rates = self.ctx.rates();
        for p in 0..2 {
            for (a, art) in self.parts[p].arts[..31].iter().enumerate() {
                if art.lfo_global && art.lfo.active() {
                    let pace = art.lfo.pace_unfrozen(rates[usize::from(art.lfo.tempo())]);
                    let ticks = len as f64 / self.ctx.tick as f64;
                    let moved = (ticks * pace).round_ties_even() as i64 as u32;
                    self.lfo_phase[p][a] = self.lfo_phase[p][a].wrapping_add(moved);
                }
            }
        }
    }

    /// The control rate and what the parameters make of every part
    /// (`FUN_00878690`). This allocates nothing.
    fn derive(&mut self) {
        let g = &self.state.globals;
        let sr = self.ctx.rate;
        let grain = f64::from(g.grain.clamp(50, 300)) / 100.0;
        let tick = (4.0 * (grain * 0.001 * sr * 0.25).round()).max(4.0) as usize;
        let t = tick as f64;
        let ctx = &mut self.ctx;
        ctx.tick = tick;
        ctx.pitch_glide = fall(0.04 * sr / t) as f32;
        ctx.cut_glide = 0.01f64.powf(t / (0.1 * sr)) as f32;
        ctx.gain_glide = fall(0.015 * sr) as f32;
        ctx.lfo_level = g.params[15] as f32 / 1000.0;
        ctx.mod_xyz = [0, 1, 2].map(|j| g.params[j] as f32 / 512.0);
        let v = f64::from(g.max_tones.clamp(3600, 10800));
        ctx.nmax = (((((v / 1200.0).exp2()).ceil() as usize) + 11) & !7).clamp(8, PARTIALS);
        ctx.safety = f64::from(g.safety.clamp(0, 2400)) / 1200.0;
        if g.safety >= 2400 {
            ctx.safety = 10.0;
        }
        ctx.voice_random = g.voice_random;
        self.post = level3(f64::from(g.params[12]) / 1000.0) as f32;
        let both = self.state.parts[0].enabled && self.state.parts[1].enabled;
        let ctx = self.ctx;
        let mix = f64::from(self.state.globals.params[10]) / 1000.0;
        for p in 0..2 {
            // Part mix (section 3.11): both on share a cosine law.
            let share = if both {
                let angle = if p == 0 { mix } else { 1.0 - mix };
                0.5625 * (PI / 2.0 * angle).cos().clamp(0.0, 1.0)
            } else {
                0.5
            };
            let part = &self.state.parts[p];
            self.parts[p].derive(part, &self.state.globals.params, share, &ctx);
        }
        let g = &self.state.globals;
        self.units
            .set(&g.params, g.dist_mode, g.precision, ctx.tempo);
    }

    /// The compression preset as a Maximus, from `System/Compression`.
    fn build_compression(&mut self) {
        let kind = self.state.globals.comp_type;
        self.compression = None;
        self.missing_compression = false;
        if kind <= 0 {
            return;
        }
        let index = (kind.clamp(1, 4) - 1) as usize;
        let Some(bytes) = &self.compression_states[index] else {
            self.missing_compression = true;
            return;
        };
        let mut maximus = Maximus::default();
        if maximus.set_state(bytes).is_err() {
            self.missing_compression = true;
            return;
        }
        maximus.set_sample_rate(self.ctx.rate as f32);
        let g = &self.state.globals.params;
        // REC 821..824 go to the Maximus's LMH mix and band pre gains.
        for (rec, param) in [(49, 5), (50, 8), (51, 21), (52, 34)] {
            maximus.set_param(param, g[rec] as f32);
        }
        self.compression = Some(Box::new(maximus));
    }

    fn random(&mut self) -> f32 {
        // xorshift32
        let mut x = self.seed;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.seed = x;
        (f64::from(x) / 4_294_967_296.0) as f32
    }

    fn free_slot(&self) -> usize {
        if let Some(i) = self.voices.iter().position(|v| !v.active) {
            return i;
        }
        // Take the oldest, released ones first.
        let key = |v: &Voice| (v.state < 6, v.age);
        (0..self.voices.len())
            .min_by_key(|i| key(&self.voices[*i]))
            .unwrap_or(0)
    }

    /// TriggerVoice (`0x883cd0`): a voice for every unison copy of every
    /// enabled part.
    fn start(&mut self, note: NoteOn) {
        self.arp_dirty = true;
        let key_pitch = f32::from(note.key - 60) * 100.0 + note.fine_pitch;
        let vel01 = (((f64::from(note.velocity) * 10.0 + 1.0).ln() / 11f64.ln()) * 100.0 / 128.0)
            .min(1.0) as f32;
        let to_volume = self.state.globals.params[13] > 0;
        let (legato, porta, limit) = {
            let g = &self.state.globals.params;
            (g[4] > 0, g[5] > 0, g[3] as f32)
        };
        // Mono modes: 1 kills the voices of the same key, 2 every voice,
        // 3 lets every voice go.
        let mono = self.state.globals.mono;
        if mono > 0 {
            for v in self.voices.iter_mut().filter(|v| v.active && v.state < 9) {
                if mono == 3 {
                    continue;
                }
                if mono == 2 || v.key_pitch == key_pitch {
                    v.state = 9;
                    v.fade = 0;
                }
            }
            if mono == 3 {
                self.release(None);
            }
        }
        // Legato: the closest sounding note within the limit glides to
        // this one.
        if legato {
            let closest = self
                .voices
                .iter()
                .filter(|v| v.active && v.state < 6 && v.unison == 0)
                .filter(|v| limit <= 0.0 || (v.key_pitch - key_pitch).abs() <= limit)
                .min_by(|a, b| {
                    let da = (a.key_pitch - key_pitch).abs();
                    let db = (b.key_pitch - key_pitch).abs();
                    da.total_cmp(&db)
                })
                .map(|v| v.id);
            if let Some(old) = closest {
                for v in self
                    .voices
                    .iter_mut()
                    .filter(|v| v.active && v.id == old && v.state < 6)
                {
                    if v.stacked < v.stack.len() {
                        v.stack[v.stacked] = (old, v.key_pitch);
                        v.stacked += 1;
                    }
                    let delta = v.key_pitch - key_pitch;
                    v.id = note.voice;
                    v.key_pitch = key_pitch;
                    start_glide(v, delta, self.parts[v.part].more.glide);
                }
                self.last_pitch = Some(key_pitch);
                return;
            }
        }
        let porta_from = (porta || note.porta).then_some(self.last_pitch).flatten();
        self.last_pitch = Some(key_pitch);
        let held = self
            .voices
            .iter()
            .filter(|v| v.active && v.state < 6 && v.unison == 0)
            .count() as f32;
        let mode = self.ctx.voice_random;
        let same = self.state.globals.same_random;
        let mut buffer = [0.0f32; ARTICULATORS];
        let draw = |plugin: &mut Harmor, buffer: &mut [f32; ARTICULATORS]| {
            if same {
                let r = plugin.random();
                buffer.fill(r);
            } else {
                for v in buffer.iter_mut() {
                    *v = plugin.random();
                }
            }
        };
        draw(self, &mut buffer);
        let rates = self.ctx.rates();
        for p in 0..2 {
            if !self.parts[p].on {
                continue;
            }
            if mode == 2 {
                draw(self, &mut buffer);
            }
            // Shared envelopes (flag 2) follow the first voice of the part
            // that is held, the oldest (`0x883cd0`: the list at +0xe20, kind
            // 0, the same part, state under 6), and the note's voices tick
            // with it.
            let shares = self.parts[p].arts[..31]
                .iter()
                .any(|a| a.envelope.active && a.envelope.shared);
            let link = shares
                .then(|| {
                    self.voices
                        .iter()
                        .filter(|v| v.active && v.part == p && v.state < 6)
                        .min_by_key(|v| v.age)
                })
                .flatten()
                .map(|v| {
                    let runs: [EnvRun; 31] = std::array::from_fn(|a| v.runs[a].env);
                    let (len, dt) = if v.first {
                        (v.first_len, v.first_dt)
                    } else if v.countdown > 0 {
                        (Some(v.countdown), 0.0)
                    } else {
                        (None, 1.0)
                    };
                    (runs, len, dt)
                });
            let n = self.parts[p].unison;
            for i in 0..n {
                if mode == 3 && i > 0 {
                    draw(self, &mut buffer);
                }
                let slot = self.free_slot();
                let age = self.next_age;
                self.next_age += 1;
                let jitter = if self.parts[p].unison_type == 3 && n > 1 {
                    (self.random() * 2.0 - 1.0) * 0.5 / (n as f32 - 0.5)
                } else {
                    0.0
                };
                let dsp = &self.parts[p];
                let v = &mut self.voices[slot];
                v.active = true;
                v.id = note.voice;
                v.part = p;
                v.unison = i;
                v.age = age;
                v.state = 1;
                v.fade = 0;
                v.key_pitch = key_pitch;
                v.vel01 = vel01;
                v.vel_gain = if to_volume { note.velocity } else { 1.0 };
                v.pan = note.pan;
                v.mod_x = note.mod_x;
                v.mod_y = note.mod_y;
                v.release = note.release;
                v.held = held;
                v.random = if mode == 0 {
                    [0.0; ARTICULATORS]
                } else {
                    buffer
                };
                v.countdown = 0;
                v.first_len = None;
                v.first_dt = 0.0;
                v.linked = false;
                v.first = true;
                v.glide = [0.0; 2];
                v.glide_left = 0;
                v.stacked = 0;
                v.age_samples = 0;
                v.noise_p = [f32::NAN; 2];
                v.noise_acc = [0.0; 2];
                v.blur_len = 0;
                v.ramp_hz.fill(f32::NAN);
                v.bank.floor = if self.state.globals.precision == 0 {
                    1.584_893_2e-5
                } else {
                    SILENT
                };
                if let Some(from) = porta_from {
                    start_glide(v, from - key_pitch, dsp.more.glide);
                }
                v.count = 0;
                v.bank.reset();
                v.gains = [0.0; 4];
                v.pluck.fill(1.0);
                v.pluck_x = f32::NAN;
                v.pluck_ticks = 0;
                let nf = n as f32;
                let lin = if n > 1 {
                    2.0 * i as f32 / (nf - 1.0) - 1.0
                } else {
                    0.0
                };
                v.detune = (dsp.offsets[i] + jitter) * dsp.unison_cents;
                v.pan_offset = (lin * dsp.unison_pan).clamp(-1.0, 1.0);
                let invert = self.state.parts[p].params[36] > 0 && (i + p) % 2 == 1;
                v.level = dsp.unison_level * if invert { -1.0 } else { 1.0 };
                v.unison_x = if n > 1 { i as f32 / (nf - 1.0) } else { 0.5 };
                v.runs.clear();
                for (a, art) in dsp.arts.iter().enumerate() {
                    let random = (mode != 0).then_some(v.random[a]);
                    let mut run = art.start(random, v.unison_x, rates);
                    if art.lfo_global
                        && let Some(lfo) = &mut run.lfo
                    {
                        lfo.set_phase(self.lfo_phase[p][a]);
                    }
                    if let Some((runs, len, dt)) = &link
                        && a < 31
                        && art.envelope.active
                        && art.envelope.shared
                    {
                        art.envelope.follow(&mut run.env, &runs[a]);
                        (v.first_len, v.first_dt, v.linked) = (*len, *dt, true);
                    }
                    if a == 1 && dsp.vel_to_attack {
                        // VelToAttScale: the volume envelope's attack is
                        // 2 - 2 vel as long.
                        run.env.scale_attack(2.0 - 2.0 * vel01);
                    }
                    v.runs.push(run);
                    v.art[a] = run.out;
                }
            }
        }
    }

    /// Voice_Release (`0x885390`): every main-list envelope lets go. A
    /// legato voice goes back to the newest note it took over that is
    /// still held.
    fn release(&mut self, id: Option<u32>) {
        self.arp_dirty = true;
        if let Some(id) = id {
            for v in self.voices.iter_mut().filter(|v| v.active && v.stacked > 0) {
                let mut keep = 0;
                for i in 0..v.stacked {
                    if v.stack[i].0 != id {
                        v.stack[keep] = v.stack[i];
                        keep += 1;
                    }
                }
                v.stacked = keep;
            }
            for v in self.voices.iter_mut() {
                if v.active && v.state < 6 && v.id == id && v.stacked > 0 {
                    v.stacked -= 1;
                    let (back, pitch) = v.stack[v.stacked];
                    let delta = v.key_pitch - pitch;
                    v.id = back;
                    v.key_pitch = pitch;
                    start_glide(v, delta, self.parts[v.part].more.glide);
                }
            }
        }
        for v in self.voices.iter_mut() {
            if !v.active || v.state >= 6 || id.is_some_and(|id| v.id != id) {
                continue;
            }
            let dsp = &self.parts[v.part];
            let scale = if dsp.vel_to_release {
                2.0 * (1.0 - v.release)
            } else {
                1.0
            };
            v.state = 6;
            for a in 0..31 {
                let art = &dsp.arts[a];
                if art.envelope.active {
                    art.envelope.release(&mut v.runs[a].env, scale);
                }
            }
        }
    }
}

/// Start a glide of `delta` cents to nothing (`FUN_00888a50`): `ticks`
/// long, linear when negative, else two poles.
fn start_glide(v: &mut Voice, delta: f32, ticks: f64) {
    let t = ticks.abs();
    if delta == 0.0 || t < 1.0 {
        v.glide = [0.0; 2];
        v.glide_left = 0;
        return;
    }
    let delta = f64::from(delta);
    if ticks < 0.0 {
        let n = t.round().max(1.0);
        v.glide = [delta, delta];
        v.glide_step = delta / n;
        v.glide_left = n as i32;
    } else {
        v.glide = [delta, delta];
        v.glide_step = (std::f64::consts::SQRT_2 * 0.01f64.ln() / t).exp();
        v.glide_left = 0;
    }
}

/// One control tick of a voice (`FUN_0088ab10`).
fn tick(dsp: &PartDsp, ctx: &Ctx, vibrato: f64, v: &mut Voice) {
    let first = v.first;
    let rates = ctx.rates();
    // Part pitch smoothing.
    v.sp = if first {
        dsp.pitch_sum
    } else {
        dsp.pitch_sum + (v.sp - dsp.pitch_sum) * ctx.pitch_glide
    };
    // The legato and portamento glide (FUN_008826d0, FUN_008826f0).
    if v.glide_left > 0 {
        v.glide[1] -= v.glide_step;
        v.glide_left -= 1;
        if v.glide_left == 0 {
            v.glide = [0.0; 2];
        }
    } else if v.glide[1] != 0.0 {
        let c = v.glide_step;
        v.glide[0] *= c;
        v.glide[1] = (v.glide[1] - v.glide[0]) * c + v.glide[0];
        if v.glide[1].abs() < 1e-3 {
            v.glide = [0.0; 2];
        }
    }
    let glide = v.glide[1] as f32;
    let [mx, my, mz] = ctx.mod_xyz;
    let mut inputs = Inputs {
        dt: if first { v.first_dt } else { 1.0 },
        speed: 1.0,
        lfo_amount: 1.0,
        at: [
            (v.key_pitch + glide + dsp.main_pitch + 6000.0) / 12000.0,
            v.vel01,
            mx + v.mod_x * 0.5,
            my + v.mod_y * 0.5,
            mz,
            0.0,
            0.0,
            v.held / 32.0,
            0.0,
        ],
        rates,
    };
    inputs.speed = f64::from(v.art[39]);
    inputs.lfo_amount = v.art[38] * ctx.lfo_level;
    // The pre-pass reads the last tick's speed and amount.
    for a in 31..ARTICULATORS {
        dsp.arts[a].tick(&mut v.runs[a], &inputs, first);
        v.art[a] = v.runs[a].out;
    }
    if first {
        let globals = [v.art[34], v.art[35], v.art[36], v.art[37]];
        let identity = globals == [1.0, 1.0, 0.0, 1.0];
        let jump = (v.art[33] * 2.0).max(0.0);
        // The global LFO phase (articulator 40) moves every LFO of the main
        // list on, once (`0x88ab10`: `round(art[40] x 2^32)`).
        let shift = (f64::from(v.art[40]) * 4_294_967_296.0).round_ties_even() as i64 as u32;
        for a in 0..31 {
            let art = &dsp.arts[a];
            if shift != 0
                && let Some(lfo) = &mut v.runs[a].lfo
            {
                lfo.shift_phase(shift);
            }
            if !art.envelope.active {
                continue;
            }
            let run = &mut v.runs[a].env;
            if !identity {
                art.envelope.scale(run, globals);
            }
            let mut share = jump;
            if a == 1 && dsp.vel_to_attack {
                share += v.vel01;
            }
            // A shared envelope that follows another voice's keeps its place.
            if share > 0.0 && !(v.linked && art.envelope.shared) {
                let rate = rates[usize::from(art.envelope.tempo)];
                art.envelope.jump(run, share.min(1.0), rate);
            }
        }
    }
    inputs.speed = f64::from(v.art[39]);
    inputs.lfo_amount = v.art[38] * ctx.lfo_level;
    for a in 0..31 {
        dsp.arts[a].tick(&mut v.runs[a], &inputs, first);
        v.art[a] = v.runs[a].out;
    }
    let art = v.art;

    // Pitch (section 3.4).
    let mut pitch = v.key_pitch + v.sp + glide;
    if dsp.vibrato > 0.0 {
        pitch += ((TAU * vibrato).sin() as f32) * dsp.vibrato * art[25];
    }
    pitch += dsp.pitch_env * art[24];
    let log2_f0 = LOG2_440 + (f64::from(pitch) / 100.0 + 3.0) / 12.0;
    let f0 = log2_f0.exp2();
    v.f0 = f0;
    v.log2_f0 = log2_f0 as f32;
    let f0c = f0 / ctx.rate;
    let a4 = v.detune * art[26] * art[26];
    let spread = if dsp.unison_shape.is_none() {
        (f64::from(a4) / 1200.0).exp2()
    } else {
        1.0
    };

    // The partial count (FUN_0088fbc0).
    let ceiling = dsp.top_hz * (ctx.safety + dsp.neg_log2_d).exp2();
    let harmonics = if dsp.oct {
        ((dsp.top_hz.log2() - log2_f0) * 8.0 / dsp.detune).max(0.0)
    } else {
        (ceiling / f0).round().max(0.0)
    };
    let mut n = (harmonics.min(1e4) as usize + 1 + 4)
        .min(ctx.nmax)
        .min(PARTIALS);
    n = ((n + 7) & !7).min(PARTIALS);
    let old = v.count;
    if n > old {
        if !first && old > 4 {
            // New partials join in step with the fundamental.
            let elapsed = v.bank.phase[4] - dsp.phase[4];
            for k in old..n {
                v.bank.phase[k] = frac(dsp.phase[k] + dsp.ratio[k] * elapsed);
            }
        }
        v.count = n;
        v.bank.len = n;
    }
    let n = v.count;

    // Frequencies.
    let base = f0c * spread;
    for k in 1..n {
        let mut f = dsp.ratio[k] * base;
        if let Some(shape) = &dsp.unison_shape {
            f *= (f64::from(a4 * shape[k]) / 1200.0).exp2();
        }
        v.freq[k] = f;
    }
    for k in 1..n {
        let hz = (v.freq[k] * ctx.rate).max(C0) as f32;
        v.hz[k] = hz;
        v.log2_hz[k] = hz.log2();
    }

    // Levels: the timbre mix (FUN_00885b70).
    let x = (dsp.timbre_mix * art[3]).min(1.0);
    let (t1, t2) = (&dsp.timbre[0], &dsp.timbre[1]);
    for k in 0..n {
        v.mix[k] = if x <= 0.0 {
            t1[k]
        } else if x == 1.0 {
            t2[k]
        } else {
            (1.0 - x) * t1[k] + x * t2[k]
        };
    }
    for k in 0..4 {
        v.mix[k] *= dsp.subs[k];
    }
    v.levels[..n].copy_from_slice(&v.mix[..n]);

    // The filter mix and the volume articulator (W+0x80.., W+0xb4).
    let m = (dsp.filter_mix * art[14]).clamp(-1.0, 1.0);
    let mut volume = art[1] * art[1] * dsp.volume;
    let mut applied = 1.0;
    let mut slot = 0;
    let mut which = dsp.first_filter;

    for u in 0..dsp.unit_count {
        match dsp.units[u] {
            0 => {
                for k in 0..n {
                    v.levels[k] *= dsp.local_eq[k];
                }
            }
            4 | 5 => {
                filter_slot(dsp, ctx, v, which, slot, m, first);
                which ^= 1;
                slot += 1;
            }
            1 => prism(dsp, v, art[23], f0c, ctx.rate),
            3 => harmonize(dsp, v, art[19], art[20]),
            6 => phaser(dsp, v, art[16], art[17], art[18], ctx.rate),
            7 => pluck(dsp, ctx, v, art[15], first),
            8 => blur(dsp, v, art[22], first),
            9 => {
                let g = dsp.protect_gain;
                for k in 0..n.min(dsp.protect_top) {
                    let l = v.levels[k];
                    v.levels[k] = l + (v.mix[k] - l) * dsp.protect[k] * g;
                }
            }
            10 => clip(dsp, v, art[21]),
            11 => {
                for k in 0..n {
                    v.levels[k] *= volume;
                }
                applied = volume;
                volume = 1.0;
            }
            _ => {}
        }
    }

    // The tail (lines 1209..1351).
    if dsp.below {
        for k in 1..4 {
            let w = (v.hz[k] / 114.46).min(1.0);
            v.levels[k] *= w * w * w * w;
        }
    }
    let osc_hz = f64::from(v.cut[0]).exp2() * f64::from(dsp.filters[0].res_ratio);
    v.freq[0] = osc_hz / ctx.rate;
    v.hz[0] = osc_hz.max(C0) as f32;
    v.log2_hz[0] = v.hz[0].log2();
    let pluck1 = if v.pluck_ticks > 0 { v.pluck[1] } else { 1.0 };
    v.levels[0] = dsp.subs[0] * v.res[0] * pluck1 * applied;
    for k in 0..n {
        v.levels[k] *= dsp.eq.clamped(v.log2_hz[k], 240.0, -966.63);
    }
    if dsp.more.auto_gain > 0.0 {
        // FUN_00882f00: the energy against 1.6428422.
        let e: f32 = v.levels[..n].iter().map(|l| l * l).sum();
        let g = if e <= 0.0 {
            4096.0
        } else {
            (1.642_842_2 / e).sqrt().clamp(1.0 / 4096.0, 4096.0)
        };
        let c = dsp.more.auto_gain;
        volume *= 1.0 - c + c * g;
    }
    if volume != 1.0 {
        for k in 0..n {
            v.levels[k] *= volume;
        }
    }
    if n > 456 {
        for j in 0..(n - 456).min(64) {
            let t = ((j as f64 - 1.0) * PI / 65.0).cos() * 0.5 + 0.5;
            v.levels[456 + j] *= t as f32;
        }
    }

    // To the bank, with the ramp (FUN_00878030, FUN_00783a20).
    for k in 0..n {
        v.bank.freq[k] = v.freq[k];
        v.bank.target[k] = v.levels[k];
    }
    if dsp.ramp_partial {
        let per_hz = (0.01f64.ln() / (dsp.ramp * 0.25f64.exp2() * 308.0)) as f32;
        for k in 0..n {
            if v.ramp_hz[k] != v.hz[k] {
                v.ramp_hz[k] = v.hz[k];
                v.bank.coef[k] = (v.hz[k] * per_hz).exp();
            }
        }
    } else {
        let c = fall(dsp.ramp) as f32;
        v.bank.coef[..n].fill(c);
    }
    if first {
        start_phases(dsp, v, art[41], f0c);
    }

    // The output gains (section 3.11).
    let pan = (v.pan + v.pan_offset + 4.0 * art[0]).clamp(-1.0, 1.0);
    let g = dsp.gain * v.vel_gain * v.level;
    let mix = dsp.fx_mix * art[2];
    let fx = pan_gain(pan, g * (1.0 - mix.abs()));
    let dry = pan_gain(pan, g * mix);
    v.aims = [fx[0], fx[1], dry[0], dry[1]];
    if first {
        v.gains = v.aims;
    }
    v.first = false;
}

/// The start phases (section 3.9).
fn start_phases(dsp: &PartDsp, v: &mut Voice, unison_phase: f32, f0c: f64) {
    let n = v.count;
    let mut d = dsp.phase_start;
    let mut r = dsp.phase_random;
    if dsp.unison > 1 {
        let mut u = f64::from(dsp.unison_phase * unison_phase);
        if dsp.unison_type == 4 {
            u = -u.abs();
        }
        if u <= 0.0 {
            d += v.unison as f64 / dsp.unison as f64 * u;
        } else if u == 1.0 {
            r = -1.0;
        } else {
            let x = (2.0 * (16.0 * u).exp2() - 2.0) * (f64::from(v.detune) / 1200.0).exp2();
            d += frac(x);
        }
    }
    let f4 = v.freq[4].max(1e-12);
    let mut seed = (v.age as u32).wrapping_mul(0x9e37_79b9) ^ 0x85eb_ca6b;
    for k in 0..n {
        let mut phase = dsp.phase[k];
        if r < 0.0 {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            phase += f64::from(seed) / 4_294_967_296.0 * -r;
        }
        v.bank.phase[k] = phase;
    }
    if r >= 0.0 {
        d += r * frac(f0c * 277.0 * v.age as f64);
    }
    if d != 0.0 {
        for k in 0..n {
            v.bank.phase[k] += v.freq[k] * d / f4;
        }
    }
    v.bank.phase[0] = 0.0;
    v.bank.phase[1] = 0.0;
    if dsp.below {
        v.bank.phase[2] = 0.0;
        v.bank.phase[3] = 0.0;
    }
    for k in 0..n {
        v.bank.phase[k] = frac(v.bank.phase[k]);
    }
}

/// A filter slot (`FUN_00886eb0`): the cutoff, the curve, the resonance,
/// and the mix of the two filters.
fn filter_slot(
    dsp: &PartDsp,
    ctx: &Ctx,
    v: &mut Voice,
    f: usize,
    slot: usize,
    m: f32,
    first: bool,
) {
    let filter = &dsp.filters[f];
    let n = v.count;
    let art = v.art;
    // The weight this filter is heard at (W+0x88).
    let weight = if slot == 0 {
        if m < 0.0 { 1.0 - m.abs() } else { 1.0 }
    } else {
        m.abs()
    };
    let parallel = m < 0.0;
    if slot == 0 && parallel {
        v.save[..n].copy_from_slice(&v.levels[..n]);
    }
    // The cutoff.
    v.cut_base[f] = if first {
        filter.base
    } else {
        filter.base + (v.cut_base[f] - filter.base) * ctx.cut_glide
    };
    let kt = (v.log2_f0 - 130.8128f32.log2()) * filter.keytrack;
    let two_range = 2.0 * filter.range;
    let mut c = v.cut_base[f] + kt + art[4 + f] * two_range;
    let cut_art = &dsp.arts[4 + f];
    if cut_art.envelope.active {
        let env = v.runs[4 + f].env.out;
        let amount = filter.env_amount * art[31 + f];
        if filter.adaptive {
            let e = 4.0 * env * amount;
            if e <= 0.0 {
                c += e * (filter.hi - (filter.lo + kt));
            } else {
                c += e * (filter.hi - c);
            }
        } else {
            c += env * amount * two_range;
        }
    }
    v.cut[f] = if first {
        c
    } else {
        c + (v.cut[f] - c) * cut_art.smoothing
    };
    let c = v.cut[f];
    let r = filter.res * art[8 + f];
    v.res[f] = r;
    let cut_hz = c.exp2();
    if !filter.identity && weight != 0.0 {
        let sc = (1.0 - filter.width * art[6 + f]).exp2() * 1024.0;
        let table = &filter.table;
        let mask = &dsp.more.masks[f];
        for k in 0..n {
            let c_k = if filter.hz {
                let s = sc * 0.9;
                table.clamped(v.hz[k], s / cut_hz, table.centre - s)
            } else {
                table.clamped(v.log2_hz[k], sc, table.centre - c * sc)
            };
            v.levels[k] *= match mask {
                Some(m) => 1.0 + mask_at(m, v.log2_hz[k]) * (c_k - 1.0),
                None => c_k,
            };
        }
    }
    // The noise resonance (type 109): partials drop out at random, the
    // more the higher the amount.
    if r > 0.0 && weight != 0.0 && filter.noise {
        noise(filter, ctx, v, f, r, art[10 + f], &dsp.more.masks[2 + f]);
    }
    // The resonance: 2^(r C), with r the amount.
    if r > 0.0
        && weight != 0.0
        && let Some(table) = &filter.resonance
    {
        let mut sw = if filter.res_hz {
            1024.0 * 0.000_375
        } else {
            1024.0
        };
        if filter.res_adaptive {
            sw *= if filter.res_hz { 184.997 } else { 92.4986 };
            sw /= v.f0 as f32;
        }
        sw *= (1.0 - filter.res_width * art[10 + f]).exp2();
        let shift = 8.0 * art[12 + f];
        let mask = &dsp.more.masks[2 + f];
        let offset = if filter.res_hz {
            table.centre - cut_hz * filter.res_ratio * shift.exp2() * sw
        } else {
            table.centre - (c + filter.res_octaves + shift) * sw
        };
        for k in 0..n {
            let x = if filter.res_hz { v.hz[k] } else { v.log2_hz[k] };
            let mut g = table.clamped(x, sw, offset);
            if let Some(m) = mask {
                g *= mask_at(m, v.log2_hz[k]);
            }
            v.levels[k] *= (r * g).exp2();
        }
    }
    // The mix.
    if slot == 0 {
        if parallel {
            // The second filter takes the input, this one waits.
            for k in 0..n {
                std::mem::swap(&mut v.levels[k], &mut v.save[k]);
            }
        } else if m < 1.0 {
            v.save[..n].copy_from_slice(&v.levels[..n]);
        }
    } else if parallel {
        for k in 0..n {
            v.levels[k] = v.save[k] + (v.levels[k] - v.save[k]) * m.abs();
        }
    } else if m < 1.0 {
        for k in 0..n {
            v.levels[k] += (v.save[k] - v.levels[k]) * (1.0 - m);
        }
    }
}

/// The noise resonance of `FUN_00886eb0` (resonance type 109).
fn noise(
    filter: &Filter,
    ctx: &Ctx,
    v: &mut Voice,
    f: usize,
    r: f32,
    width_art: f32,
    mask: &Option<ShapeTable>,
) {
    let p = filter.res_width * width_art;
    if p != v.noise_p[f] {
        v.noise_p[f] = p;
        let rate = 9999.0 / ((801f64.powf(f64::from(p) * 0.25) - 1.0) * 1.247_505_7 + 1.995_464_9);
        v.noise_rate[f] = rate;
        v.noise_step[f] = ctx.tick as f64 * 0.100_01 * rate / ctx.rate;
    }
    let r2 = f64::from(r * r);
    let mut level = (r2 * r2 * r2 * v.noise_rate[f] * 2.0) as f32;
    if filter.res_hz {
        level *= 2.0;
    }
    let hi = level + 1.0;
    let root = hi.sqrt();
    // A uniform number in [0, hi) for every partial from a copy of the
    // held seed: `ippsRandUniform_Direct_32f` through the pointer at
    // 0x92bbd0 (`0x8876b8`).
    let mut seed = v.noise_seed[f] as i32;
    let n = v.count;
    rand_uniform(&mut v.noise_buf[..n], 0.0, hi, &mut seed);
    for k in 0..n {
        let mut u = v.noise_buf[k];
        let top = v.hz[k] / 17_739.69;
        if !filter.res_hz {
            u -= top;
        }
        let g = if filter.res_adaptive {
            let d = 1.0 - top;
            let over = (u - level).max(0.0);
            let over = if over > 0.0 && d > 0.0 {
                over / d
            } else {
                over
            };
            over * over * 2.0 * root
        } else if u < level {
            0.0
        } else {
            root
        };
        v.levels[k] *= match mask {
            Some(m) => 1.0 + mask_at(m, v.log2_hz[k]) * (g - 1.0),
            None => g,
        };
    }
    v.noise_acc[f] += v.noise_step[f];
    if v.noise_acc[f] >= 1.0 {
        v.noise_acc[f] = frac(v.noise_acc[f]);
        // The hash FL moves the seed on with.
        let mut a = v.noise_seed[f].wrapping_sub(0x2036a) ^ 0xc;
        let mut b = a.wrapping_neg().wrapping_sub(0x12c58) ^ (a << 8);
        let mut c = 0x197e1u32.wrapping_sub(a).wrapping_sub(b) ^ (b >> 13);
        a = a.wrapping_sub(b).wrapping_sub(c) ^ (c >> 12);
        b = b.wrapping_sub(c).wrapping_sub(a) ^ (a << 16);
        c = c.wrapping_sub(a).wrapping_sub(b) ^ (b >> 5);
        a = a.wrapping_sub(b).wrapping_sub(c) ^ (c >> 3);
        b = b.wrapping_sub(c).wrapping_sub(a) ^ (a << 10);
        v.noise_seed[f] = c.wrapping_sub(a).wrapping_sub(b) ^ (b >> 15);
    }
}

/// The Hz and log Hz of partials 1.. after their frequencies moved.
fn refresh_hz(v: &mut Voice, rate: f64) {
    for k in 1..v.count {
        let hz = (v.freq[k] * rate).max(C0) as f32;
        v.hz[k] = hz;
        v.log2_hz[k] = hz.log2();
    }
}

/// The prism (`FUN_00886aa0`): partials move from their harmonic places
/// by the prism shape.
fn prism(dsp: &PartDsp, v: &mut Voice, amount: f32, f0c: f64, rate: f64) {
    let m = &dsp.more;
    let x = m.prism_amount * amount;
    if x == 0.0 {
        return;
    }
    let a = if m.prism_multiply {
        x.clamp(-1.0, 1.0)
    } else {
        x.signum() * 4.0 * x * x
    };
    let add_one = m.prism_multiply && m.prism_from == 0;
    for k in 1..v.count {
        let w = match m.prism_from {
            1 => v.levels[k].min(1.0),
            2 => 1.0 - v.levels[k].min(1.0),
            _ => 1.0,
        };
        let p = m.prism[k] + if add_one { 1.0 } else { 0.0 };
        let d = f64::from(a * w * p);
        v.freq[k] = if m.prism_multiply {
            f0c + (v.freq[k] - f0c) * (1.0 + d)
        } else {
            v.freq[k] + f0c * dsp.detune * d
        };
    }
    refresh_hz(v, rate);
}

/// The harmonizer (`FUN_00885cb0`): the spectrum again at multiples of
/// each harmonic.
fn harmonize(dsp: &PartDsp, v: &mut Voice, mix_art: f32, width_art: f32) {
    let m = &dsp.more;
    let mix = (m.harm_mix * mix_art).min(1.0);
    let width = (m.harm_width * width_art).min(1.0);
    if mix <= 0.0 || width <= 0.0 {
        return;
    }
    let n = v.count.saturating_sub(4);
    let step = 1.0 / (width * 8.0 + 1.0);
    let [shift, shift_step, gap, gap_step] = m.harm_pattern;
    let pattern = |s: i32| if s < 0 { (1 - s, 0) } else { (1, s) };
    let (mul_a, add_a) = pattern(shift_step);
    let (mul_b, add_b) = pattern(gap_step);
    v.save[..n].copy_from_slice(&v.levels[4..4 + n]);
    let (mut a, mut b) = (2i32, 2i32);
    let mut sum = 1.0f32;
    let mut t = 1.0 - step;
    let mut k = 0;
    while t > 0.0 && k < 9 {
        let g = (t * t * t * m.harm_strength).min(1.0) * mix * mix * m.spread[k];
        sum += g;
        let start = a + shift;
        let stride = b + gap;
        if g >= SILENT && stride > 0 && start >= 0 {
            let (start, stride) = (start as usize, stride as usize);
            let mut j = 0;
            while start + j * stride < n {
                v.levels[4 + start + j * stride] += v.save[j] * g;
                j += 1;
            }
        }
        a = a * mul_a + add_a;
        b = b * mul_b + add_b;
        t -= step;
        k += 1;
    }
    let norm = 1.0 / sum;
    match &m.masks[5] {
        None => {
            for l in &mut v.levels[4..4 + n] {
                *l *= norm;
            }
        }
        Some(mask) => {
            for j in 0..n {
                let dry = v.save[j];
                let w = mask_at(mask, v.log2_hz[4 + j]);
                v.levels[4 + j] = dry + (v.levels[4 + j] * norm - dry) * w;
            }
        }
    }
}

/// The phaser (`FUN_00887f10`): a comb read round its table over the
/// spectrum, or for the frequency type a shift of each partial.
fn phaser(dsp: &PartDsp, v: &mut Voice, mix_art: f32, width_art: f32, offset_art: f32, rate: f64) {
    let m = &dsp.more;
    let mix = (m.phaser_mix * mix_art).min(1.0);
    if mix <= 0.0 {
        return;
    }
    let table = &m.phaser;
    let (n1, n2) = (table.len() as f32, table.period);
    let kt = v.log2_f0 * m.phaser_keytrack;
    let mut off = (-m.phaser_offset - offset_art) * n1 - v.motion as f32 * n2;
    let wd = m.phaser_width;
    let (hz, a) = match m.phaser_scale {
        0 => {
            let a = ((wd - 1.0) * 8.0 + 2.0 * width_art).exp2() * 12.0 * n2;
            off -= kt * a;
            (false, a)
        }
        1 => {
            off -= kt * n1;
            (true, ((wd - 1.0 + width_art) * 8.0).exp2() * 0.005 * n2)
        }
        _ => {
            off -= kt * n1;
            (true, (2.0 - wd - width_art) * n2 / v.f0 as f32)
        }
    };
    let n = v.count;
    let mask = &m.masks[4];
    if m.phaser_freq {
        let cube = mix * mix * mix;
        for k in 1..n {
            let x = if hz { v.hz[k] } else { v.log2_hz[k] };
            let mut c = table.wrapped(x, a, off) * cube;
            if let Some(mm) = mask {
                c *= mask_at(mm, v.log2_hz[k]);
            }
            v.freq[k] *= f64::from(c).exp2();
        }
        refresh_hz(v, rate);
    } else {
        for k in 0..n {
            let x = if hz { v.hz[k] } else { v.log2_hz[k] };
            let c = table.wrapped(x, a, off);
            v.levels[k] *= match mask {
                Some(mm) => 1.0 + mix * mask_at(mm, v.log2_hz[k]) * (c - 1.0),
                None => c * mix + 1.0 - mix,
            };
        }
    }
}

/// The blur (`FUN_00886610`): a time blur that lets levels rise and fall
/// slowly, then a harmonic blur that spreads each partial over its
/// neighbours.
fn blur(dsp: &PartDsp, v: &mut Voice, amount: f32, first: bool) {
    let m = &dsp.more;
    let n = v.count;
    let timed = m.blur_att > 0.0 || m.blur_dec > 0.0;
    let width = (17f32.powf(m.blur_amount * amount) - 1.0) * 15.9375 + 1.0;
    let half = (((width.ceil() as usize) | 1) / 2).min(128);
    if !timed && half == 0 {
        return;
    }
    // The time blur (FUN_00882d10).
    if timed {
        for k in 0..n {
            let l = v.levels[k];
            let s = if first || k >= v.blur_len {
                l
            } else {
                v.blur[k]
            };
            let low = l.min(s);
            v.blur[k] = l + (low - l) * m.blur_att + (s - low) * m.blur_dec;
        }
        v.blur_len = n;
    } else {
        v.blur[..n].copy_from_slice(&v.levels[..n]);
    }
    // The harmonic blur: a ramp kernel bent by the two tensions.
    if half > 0 {
        let d = 1.0 / (width / 2.0 + 1.0);
        let mut kernel = [0.0f32; 257];
        let mut sum = 0.0;
        for (i, w) in kernel.iter_mut().enumerate().take(2 * half + 1) {
            let off = i as i32 - half as i32;
            let r = f64::from(1.0 - d * off.unsigned_abs() as f32);
            let a = m.blur_tension[usize::from(off > 0)];
            let bent = if (a - 1.0).abs() < 1e-9 {
                r
            } else {
                (a.powf(r) - 1.0) / (a - 1.0)
            };
            *w = bent as f32;
            sum += *w;
        }
        let norm = if sum > 0.0 { 1.0 / sum } else { 0.0 };
        let span = n.saturating_sub(4);
        v.save[..span].copy_from_slice(&v.blur[4..n]);
        for j in 0..span {
            let mut acc = 0.0;
            for (i, w) in kernel.iter().enumerate().take(2 * half + 1) {
                let at = (j as i32 + i as i32 - half as i32).clamp(0, span as i32 - 1) as usize;
                acc += v.save[at] * w;
            }
            v.blur[4 + j] = acc * norm;
        }
    }
    let mix = m.blur_mix;
    for k in 0..n {
        let w = match &m.blur_mask {
            Some(mm) => mask_at(mm, v.log2_hz[k]) * mix,
            None => mix,
        };
        v.levels[k] += (v.blur[k] - v.levels[k]) * w;
    }
}

/// The pluck (`FUN_00888500`): each partial dies away at a rate from the
/// pluck shape.
fn pluck(dsp: &PartDsp, ctx: &Ctx, v: &mut Voice, amount: f32, first: bool) {
    let x = dsp.pluck + amount;
    if x >= 1.0 {
        return;
    }
    let n = v.count;
    if x != v.pluck_x {
        v.pluck_x = x;
        let tp = 20000.0 * (17.0 * (f64::from(x) - 1.0)).exp2();
        let k = 0.01f64.ln() * ctx.tick as f64 / ctx.rate / (tp + 0.25);
        for i in 0..PARTIALS {
            let p = (16.0 * (1.0 - f64::from(dsp.pluck_shape[i]))).exp2();
            v.pluck_rate[i] = (k * p).exp() as f32;
        }
    }
    if first {
        v.pluck[..n].fill(1.0);
    }
    for k in 0..n {
        v.levels[k] *= v.pluck[k];
        v.pluck[k] *= v.pluck_rate[k];
    }
    v.pluck_ticks += 1;
}

/// Harmonic clipping (`FUN_00886160`).
fn clip(dsp: &PartDsp, v: &mut Voice, amount: f32) {
    let theta = (dsp.clip_level * amount).powi(2);
    let t2 = theta * theta;
    let n = v.count;
    for k in 0..n {
        let c = dsp.clip[k];
        let l = v.levels[k];
        v.levels[k] = match dsp.clip_mode {
            0 => {
                if theta < 1.0 {
                    l.min(c * t2)
                } else {
                    l
                }
            }
            1 | 2 => {
                let y = (l * c / t2.max(SILENT)).min(1.0);
                let e = if dsp.clip_mode == 1 { 4 } else { 2 };
                l * (1.0 - (1.0 - y).powi(e))
            }
            3 => {
                if l > c * t2 {
                    l
                } else {
                    0.0
                }
            }
            _ => (l - c * t2).max(0.0),
        };
    }
}

impl Harmor {
    /// Render one chunk of a voice into the buses.
    fn render_voice(
        &self,
        v: &mut Voice,
        len: usize,
        fx: &mut [[f32; CHUNK]; 2],
        dry: &mut [[f32; CHUNK]; 2],
    ) {
        let dsp = &self.parts[v.part];
        let ctx = &self.ctx;
        let mut done = 0;
        v.mono[..len].fill(0.0);
        while done < len {
            if v.countdown == 0 {
                let vib = self.vibrato[v.part] + dsp.vibrato_rate * done as f64;
                v.motion = self.motion[v.part] + dsp.more.phaser_speed * done as f64;
                let first_len = if v.first { v.first_len } else { None };
                tick(dsp, ctx, vib, v);
                v.countdown = first_len.unwrap_or(ctx.tick);
            }
            let run = v.countdown.min(len - done);
            let Voice { bank, mono, .. } = v;
            bank.render(&mut mono[done..done + run]);
            v.countdown -= run;
            done += run;
        }
        let glide = ctx.gain_glide;
        let fade_len = (0.01 * ctx.rate).max(1.0) as usize;
        let mid_side = self.state.parts[v.part].mid_side;
        let more = &dsp.more;
        let fade_in = (0.1 * ctx.rate) as usize;
        for i in 0..len {
            let mut s = v.mono[i];
            let mut trem = [1.0f32; 2];
            if more.tremolo > 0.0 {
                // Gen_Render's tremolo: 1 + d (0.21 - (0.55 (1 + sin))^2),
                // faded in over 0.1 s.
                let d = more.tremolo * (v.age_samples as f32 / fade_in as f32).min(1.0);
                let phase = self.tremolo[v.part] + more.tremolo_rate * i as f64;
                for (side, g) in trem.iter_mut().enumerate() {
                    let at = phase + 2.0 * more.tremolo_gap * side as f64;
                    let u = 0.55 * (1.0 + (TAU * at).sin()) as f32;
                    *g = 1.0 + d * (0.21 - u * u);
                }
            }
            v.age_samples += 1;
            if v.state >= 9 {
                if v.fade >= fade_len {
                    s = 0.0;
                } else {
                    let k = v.fade as f64 / (fade_len as f64 + 1.0);
                    s *= ((1.0 + (PI * k).cos()) * 0.5) as f32;
                    v.fade += 1;
                }
            }
            for (g, aim) in v.gains.iter_mut().zip(v.aims) {
                *g = aim + (*g - aim) * glide;
            }
            let r_sign = if mid_side { -1.0 } else { 1.0 };
            let (sl, sr) = (s * trem[0], s * trem[1] * r_sign);
            fx[0][i] += sl * v.gains[0];
            fx[1][i] += sr * v.gains[1];
            dry[0][i] += sl * v.gains[2];
            dry[1][i] += sr * v.gains[3];
        }
        // The end of a voice: the volume envelope is over and settled,
        // or there is none (FUN_008856b0).
        if v.state >= 6 && v.state < 9 {
            let art = &dsp.arts[1];
            let run = &v.runs[1];
            let over =
                !art.envelope.active || (run.env.over() && (run.out - run.value).abs() < 1e-4);
            // And no envelope with arpeggiator points still runs
            // (`0x88ab10`, before `FUN_008856b0`).
            let arpeggiating =
                (0..31).any(|a| dsp.arts[a].envelope.arp_on && !v.runs[a].env.over());
            if over && !arpeggiating {
                v.state = 9;
                v.fade = 0;
            }
        } else if v.state >= 9 && v.fade >= fade_len {
            v.active = false;
        }
    }
}

/// The event 213 bytes of an `.fst` preset.
fn fst_state(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.get(..4)? != b"FLhd" {
        return None;
    }
    let header = u32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?) as usize;
    let mut at = 8 + header;
    if bytes.get(at..at + 4)? != b"FLdt" {
        return None;
    }
    at += 8;
    while at < bytes.len() {
        let id = *bytes.get(at)?;
        at += 1;
        let len = match id {
            0..64 => 1,
            64..128 => 2,
            128..192 => 4,
            _ => {
                let (mut len, mut shift) = (0usize, 0);
                loop {
                    let byte = *bytes.get(at)?;
                    at += 1;
                    len |= usize::from(byte & 0x7f) << shift;
                    shift += 7;
                    if byte & 0x80 == 0 {
                        break len;
                    }
                }
            }
        };
        let data = bytes.get(at..at + len)?;
        at += len;
        if id == 213 {
            return Some(data.to_vec());
        }
    }
    None
}

/// Which articulator and which of its eight quick knobs part parameter
/// `k` is: 95 to 342 are `95 + 8 a + j` (`FUN_00879380`).
fn knob(k: usize) -> Option<(usize, usize)> {
    (95..343).contains(&k).then(|| ((k - 95) / 8, (k - 95) % 8))
}

impl Generator for Harmor {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        self.state = State::parse(bytes)?;
        self.load();
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        if hz > 0.0 && f64::from(hz) != self.ctx.rate {
            self.ctx.rate = f64::from(hz);
            *self.units = GlobalFx::new(self.ctx.rate);
            self.derive();
            if let Some(m) = &mut self.compression {
                m.set_sample_rate(hz);
            }
        }
    }

    fn set_tempo(&mut self, bpm: f64) {
        if bpm > 0.0 && bpm != self.ctx.tempo {
            self.ctx.tempo = bpm;
            self.derive();
        }
    }

    fn set_resource_dir(&mut self, dir: &Path) {
        for n in 0..4 {
            let path = dir.join(COMPRESSION).join(format!("{}.fst", n + 1));
            self.compression_states[n] = std::fs::read(path).ok().and_then(|b| fst_state(&b));
        }
        self.build_compression();
    }

    fn note_on(&mut self, note: NoteOn) {
        if note.slide {
            return;
        }
        self.start(note);
    }

    fn note_off(&mut self, voice: u32) {
        self.release(Some(voice));
    }

    fn all_notes_off(&mut self) {
        self.release(None);
    }

    /// The voices read the note's pitch on their next tick. A legato voice
    /// that holds the note under the one it plays takes the new pitch with
    /// it for when it goes back.
    fn set_note_pitch(&mut self, voice: u32, pitch: f32) {
        for v in self.voices.iter_mut().filter(|v| v.active) {
            if v.id == voice {
                v.key_pitch = pitch;
            }
            for held in &mut v.stack[..v.stacked] {
                if held.0 == voice {
                    held.1 = pitch;
                }
            }
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let total = left.len().min(right.len());
        self.rank_held();
        let mut done = 0;
        while done < total {
            let len = (total - done).min(CHUNK);
            let mut fx = self.fx;
            let mut dry = self.dry;
            for b in fx.iter_mut().chain(dry.iter_mut()) {
                b[..len].fill(0.0);
            }
            let mut voices = std::mem::take(&mut self.voices);
            for v in voices.iter_mut().filter(|v| v.active) {
                self.render_voice(v, len, &mut fx, &mut dry);
            }
            self.voices = voices;
            // The FX units in their order (FUN_00892e10).
            debug_assert!(len <= FX_BLOCK);
            for unit in self.state.globals.fx_order {
                match unit {
                    0..=3 => {
                        if self.units.runs(unit) {
                            let [l, r] = &mut fx;
                            self.units.unit(unit, &mut l[..len], &mut r[..len]);
                        }
                    }
                    4 => {
                        for c in 0..2 {
                            for i in 0..len {
                                fx[c][i] += dry[c][i];
                            }
                        }
                    }
                    _ => {
                        if let Some(m) = &mut self.compression {
                            let [l, r] = &mut fx;
                            m.process(&mut l[..len], &mut r[..len]);
                        }
                    }
                }
            }
            let post = self.post;
            for i in 0..len {
                left[done + i] += fx[0][i] * post;
                right[done + i] += fx[1][i] * post;
            }
            for p in 0..2 {
                let m = &self.parts[p].more;
                self.vibrato[p] = frac(self.vibrato[p] + self.parts[p].vibrato_rate * len as f64);
                self.motion[p] = frac(self.motion[p] + m.phaser_speed * len as f64);
                self.tremolo[p] = frac(self.tremolo[p] + m.tremolo_rate * len as f64);
            }
            self.sample_count += len as u64;
            done += len;
        }
        // The global LFOs move on once a block, after the voices.
        self.move_lfos(total);
    }

    fn param_count(&self) -> usize {
        PARAMS
    }

    /// An articulator's quick knob reads from its envelope or LFO, where
    /// FL keeps it (`0x879540`); the rest from the parameter array.
    fn param(&self, index: usize) -> f32 {
        if index < 2 * PART_PARAMS {
            let (p, k) = (index / PART_PARAMS, index % PART_PARAMS);
            let part = &self.state.parts[p];
            match knob(k) {
                Some((a, j)) if j < 4 => part.arts[a].envelope.knobs[j] as f32,
                Some((a, j)) => part.arts[a].lfo.knobs[j - 4] as f32,
                None => part.params[k] as f32,
            }
        } else if index < PARAMS {
            self.state.globals.params[index - 2 * PART_PARAMS] as f32
        } else {
            0.0
        }
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if index >= PARAMS || !value.is_finite() {
            return;
        }
        let value = value.round() as i32;
        if index < 2 * PART_PARAMS {
            let (p, k) = (index / PART_PARAMS, index % PART_PARAMS);
            // A quick knob goes to the articulator's envelope or LFO and
            // not to the array (`0x879540`, `FUN_007a2c10`).
            if let Some((a, j)) = knob(k) {
                let (min, max) = KNOB_RANGES[j];
                let value = value.clamp(min, max);
                let (state, dsp) = (&mut self.state.parts[p].arts[a], &mut self.parts[p].arts[a]);
                if j < 4 {
                    state.envelope.knobs[j] = value;
                    dsp.envelope.set_knobs(state.envelope.knobs);
                } else {
                    state.lfo.knobs[j - 4] = value;
                    dsp.lfo.set_knobs(state.lfo.knobs);
                }
                return;
            }
            let (min, max) = match PART_RANGES.get(k) {
                Some((min, max, _)) => (*min, *max),
                None => SMOOTHING_RANGE,
            };
            self.state.parts[p].params[k] = value.clamp(min, max);
        } else {
            let g = index - 2 * PART_PARAMS;
            let (min, max) = GLOBAL_RANGES[g];
            let value = value.clamp(min, max);
            self.state.globals.params[g] = value;
            if (49..53).contains(&g)
                && let Some(m) = &mut self.compression
            {
                let param = [5, 8, 21, 34][g - 49];
                m.set_param(param, value as f32);
            }
        }
        self.derive();
    }

    fn reset(&mut self) {
        for v in &mut self.voices {
            v.active = false;
        }
        if let Some(m) = &mut self.compression {
            m.reset();
        }
        self.units.reset();
        self.restart_lfos();
        let g = &self.state.globals;
        self.units
            .set(&g.params, g.dist_mode, g.precision, self.ctx.tempo);
    }

    fn tail(&self) -> usize {
        let mut seconds: f64 = 0.05;
        for (p, dsp) in self.parts.iter().enumerate() {
            if !dsp.on {
                continue;
            }
            let env = &self.state.parts[p].arts[1].envelope;
            if env.active {
                let from = if env.loop_end >= 0 {
                    env.loop_end as usize + 1
                } else {
                    0
                };
                let x: f64 = env.points.iter().skip(from).map(|p| p.x.max(0.0)).sum();
                let unit = if env.flags & 1 != 0 {
                    60.0 / self.ctx.tempo
                } else {
                    0.5
                };
                let knob = f64::from(env.knobs[3]) / 128.0;
                seconds = seconds.max(x * unit * knob * 2.0 + 0.05);
            }
        }
        (seconds.min(30.0) * self.ctx.rate) as usize + self.ctx.tick + self.units.tail()
    }

    fn latency(&self) -> usize {
        self.compression.as_ref().map_or(0, |m| m.latency())
    }

    fn fidelity(&self) -> Fidelity {
        let g = &self.state.globals;
        let parts: Vec<&Part> = self.state.enabled().map(|p| &self.state.parts[p]).collect();
        let any = |f: &dyn Fn(&Part) -> bool| parts.iter().any(|p| f(p));
        if self.missing_compression {
            return Fidelity::Approximate("Harmor without its compression preset");
        }
        if any(&|p| p.image.kind == 1 || p.image.plays()) {
            return Fidelity::Approximate("Harmor images are left out");
        }
        if g.params[7] > 0 || g.params[8] > 0 {
            return Fidelity::Approximate("Harmor strum is left out");
        }
        // An LFO's level runs on `menvelope::Track`, which reads no
        // arpeggiator points.
        let arp_lfo = |p: &Part| {
            p.arts[..31]
                .iter()
                .any(|a| a.lfo.active && a.lfo.arp.iter().any(|byte| *byte != 0))
        };
        if any(&arp_lfo) {
            return Fidelity::Approximate("Harmor LFOs with arpeggiator points run without them");
        }
        Fidelity::Approximate("Harmor: resonance tables and FX not null-tested")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{corpus, partial, projects};
    use dsp::curve::Point;

    fn note(key: i16) -> NoteOn {
        NoteOn {
            voice: 1,
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

    fn render(plugin: &mut Harmor, frames: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = (vec![0.0; frames], vec![0.0; frames]);
        for (a, b) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
            plugin.process(a, b);
        }
        (l, r)
    }

    #[test]
    fn a_note_follows_the_pitch_the_host_moves_it_to() {
        crate::testing::follows_note_pitch(|| Box::new(Harmor::default()), 44_100.0);
    }
    #[test]
    fn the_default_is_a_saw_at_c4() {
        let mut plugin = Harmor::default();
        plugin.note_on(note(60));
        let (l, _) = render(&mut plugin, 44_100);
        // 261.63 Hz, harmonics at 1/h.
        let tail = &l[22_050..22_050 + 4410];
        let f = 261.625_565 / 44_100.0;
        let h1 = partial(tail, f);
        let h2 = partial(tail, 2.0 * f);
        let h3 = partial(tail, 3.0 * f);
        assert!(h1 > 0.1, "{h1}");
        assert!((h2 / h1 - 0.5).abs() < 0.03, "{}", h2 / h1);
        assert!((h3 / h1 - 1.0 / 3.0).abs() < 0.03, "{}", h3 / h1);
        // One part alone is at 0.5.
        assert!((h1 - 0.5).abs() < 0.05, "{h1}");
    }

    #[test]
    fn every_parameter_is_kept_inside_its_control() {
        let mut plugin = Harmor::default();
        // The range a host reads back is what the plugin keeps a value in.
        let range = |plugin: &mut Harmor, index: usize| {
            plugin.set_param(index, f32::MIN);
            let low = plugin.param(index);
            plugin.set_param(index, f32::MAX);
            (low, plugin.param(index))
        };
        // Part 1's volume articulator's release knob, and an LFO's speed
        // and pulse width.
        assert_eq!(plugin.param(106), 128.0);
        assert_eq!(range(&mut plugin, 106), (0.0, 256.0));
        assert_eq!(plugin.state.parts[0].arts[1].envelope.knobs[3], 256);
        assert_eq!(plugin.state.parts[0].params[106], 0);
        assert_eq!(range(&mut plugin, 386 + 99), (512.0, 65_536.0));
        assert_eq!(range(&mut plugin, 102), (-112.0, 112.0));
        // A smoothing, Mod X, Mod Z, the strum mode, the last compression
        // band.
        assert_eq!(range(&mut plugin, 343), (0.0, 1001.0));
        assert_eq!(range(&mut plugin, 772), (0.0, 512.0));
        assert_eq!(range(&mut plugin, 774), (0.0, 512.0));
        assert_eq!(range(&mut plugin, 779), (0.0, 3.0));
        assert_eq!(range(&mut plugin, 824), (0.0, 2000.0));
        for index in 0..PARAMS {
            let (low, high) = range(&mut plugin, index);
            assert!(low > -1e6 && high < 1.1e6, "{index}: {low} {high}");
        }
        // The release knob is heard: a note on part 1 with a volume
        // envelope rings out four times as long at 256 as at 64, but for
        // the fade and a tick.
        let rings = |knob: f32| {
            let mut plugin = Harmor::default();
            let envelope = &mut plugin.state.parts[0].arts[1].envelope;
            envelope.active = true;
            envelope.points = vec![
                dsp::curve::Point::new(0.0, 1.0),
                dsp::curve::Point::new(0.1, 1.0),
                dsp::curve::Point::new(0.1, 0.0),
            ];
            envelope.decay = 1;
            envelope.loop_end = 1;
            plugin.load();
            plugin.set_param(106, knob);
            plugin.note_on(note(60));
            render(&mut plugin, 4410);
            plugin.note_off(1);
            let (l, _) = render(&mut plugin, 44_100);
            l.iter().rposition(|v| v.abs() > 1e-3).unwrap_or(0)
        };
        let (short, long) = (rings(64.0), rings(256.0));
        assert!(long > 5 * short / 2 && short > 0, "{short} {long}");
    }

    #[test]
    fn a_released_note_without_a_volume_envelope_ends_in_10_ms() {
        let mut plugin = Harmor::default();
        plugin.note_on(note(60));
        render(&mut plugin, 4410);
        plugin.note_off(1);
        let (l, _) = render(&mut plugin, 4410);
        assert!(l[1000..].iter().all(|v| *v == 0.0));
        assert!(plugin.voices.iter().all(|v| !v.active));
    }

    #[test]
    fn unison_spreads_the_copies_and_removes_their_mean() {
        let mut plugin = Harmor::default();
        plugin.state.parts[0].params[34] = 5;
        plugin.state.parts[0].params[35] = 1;
        plugin.derive();
        let o = plugin.parts[0].offsets;
        assert!(o[..5].iter().sum::<f32>().abs() < 1e-6);
        assert!(o[..5].iter().any(|v| v.abs() > 0.5));
        plugin.note_on(note(60));
        assert_eq!(plugin.voices.iter().filter(|v| v.active).count(), 5);
        let (l, r) = render(&mut plugin, 8820);
        assert!(l.iter().chain(&r).all(|v| v.is_finite()));
    }

    #[test]
    fn a_low_cutoff_takes_the_top_off() {
        let mut plugin = Harmor::default();
        plugin.state.parts[0].filter_types = [0, 18];
        plugin.state.parts[0].params[52] = 2000;
        plugin.load();
        plugin.note_on(note(48));
        let (l, _) = render(&mut plugin, 44_100);
        let tail = &l[22_050..22_050 + 4410];
        let f = 130.812_78 / 44_100.0;
        let low = partial(tail, f);
        let high = partial(tail, 20.0 * f);
        assert!(low > 0.05, "{low}");
        assert!(high < low * 0.01, "{low} {high}");
    }

    #[test]
    fn a_resonance_lifts_the_partials_at_the_cutoff() {
        let mut plugin = Harmor::default();
        let part = &mut plugin.state.parts[0];
        part.filter_types = [0, 18];
        part.resonance_types = [100, 111];
        // Cutoff near the 8th harmonic of C4.
        part.params[52] = 4600;
        part.params[57] = 500;
        plugin.load();
        plugin.note_on(note(60));
        let (l, _) = render(&mut plugin, 44_100);
        let tail = &l[22_050..22_050 + 4410];
        let f = 261.625_565 / 44_100.0;
        let cut = plugin.voices[0].cut[0].exp2() / 261.625_56;
        let h = cut.round().max(2.0);
        let near = partial(tail, h as f64 * f) * h as f64;
        let first = partial(tail, f);
        // Relative to the 1/h of a saw, the partial at the cutoff stands
        // out.
        assert!(near > first, "{near} {first} at {h}");
    }

    #[test]
    fn legato_glides_the_sounding_voice_to_the_new_note() {
        let mut plugin = Harmor::default();
        plugin.state.globals.params[4] = 1;
        plugin.derive();
        plugin.note_on(note(60));
        render(&mut plugin, 4410);
        plugin.note_on(NoteOn {
            voice: 2,
            ..note(67)
        });
        assert_eq!(plugin.voices.iter().filter(|v| v.active).count(), 1);
        let v = plugin.voices.iter().find(|v| v.active).unwrap();
        assert_eq!((v.id, v.key_pitch), (2, 700.0));
        assert!((v.glide[1] + 700.0).abs() < 1e-3);
        render(&mut plugin, 22_050);
        // Letting go of the new note goes back to the held one.
        plugin.note_off(2);
        let v = plugin.voices.iter().find(|v| v.active).unwrap();
        assert_eq!((v.id, v.key_pitch, v.state), (1, 0.0, 1));
    }

    #[test]
    fn a_shared_envelope_follows_the_oldest_held_voice_and_ticks_with_it() {
        let mut plugin = Harmor::default();
        let env = &mut plugin.state.parts[0].arts[1].envelope;
        env.points = vec![
            Point::new(0.0, 0.0),
            Point::new(1.0, 1.0),
            Point::new(2.0, 0.3),
            Point::new(1.0, 0.0),
        ];
        env.arp = vec![0; 4];
        (env.active, env.flags, env.decay, env.loop_end) = (true, 2, 1, 2);
        plugin.load();
        plugin.note_on(note(60));
        render(&mut plugin, 1000);
        plugin.note_on(NoteOn {
            voice: 2,
            ..note(64)
        });
        let first = plugin.voices.iter().find(|v| v.id == 1).unwrap().clone();
        let second = plugin.voices.iter().find(|v| v.id == 2).unwrap().clone();
        assert!(second.linked && !first.linked);
        assert_eq!(second.first_len, Some(first.countdown));
        render(&mut plugin, 3000);
        let first = plugin.voices.iter().find(|v| v.id == 1).unwrap();
        let second = plugin.voices.iter().find(|v| v.id == 2).unwrap();
        assert_eq!(first.countdown, second.countdown);
        assert_eq!(first.runs[1].env, second.runs[1].env);
    }

    #[test]
    fn a_global_lfo_moves_on_between_notes() {
        let mut plugin = Harmor::default();
        let lfo = &mut plugin.state.parts[0].arts[2].lfo;
        // Frozen, so only the global phase moves.
        (lfo.active, lfo.flags, lfo.knobs[0]) = (true, 2 | 8, 26_259);
        plugin.load();
        let start = plugin.parts[0].arts[2].lfo.start_phase();
        assert_eq!(plugin.lfo_phase[0][2], start);
        render(&mut plugin, 1024);
        let pace = plugin.parts[0].arts[2]
            .lfo
            .pace_unfrozen(plugin.ctx.rates()[0]);
        let per_block = (512.0 / plugin.ctx.tick as f64 * pace).round_ties_even() as u32;
        assert!(per_block > 0);
        assert_eq!(
            plugin.lfo_phase[0][2],
            start.wrapping_add(per_block.wrapping_mul(2))
        );
        // A reset puts it back.
        plugin.reset();
        assert_eq!(plugin.lfo_phase[0][2], start);
    }

    #[test]
    fn the_arpeggiator_ranks_the_held_keys() {
        let mut plugin = Harmor::default();
        let env = &mut plugin.state.parts[0].arts[1].envelope;
        env.points = vec![
            Point::new(0.0, 0.0),
            Point::new(1.0, 1.0),
            Point::new(1.0, 0.5),
        ];
        env.arp = vec![0, 3, 0];
        (env.active, env.loop_start, env.loop_end) = (true, 0, 2);
        plugin.load();
        for (id, key) in [(1, 64), (2, 60), (3, 67), (4, 60)] {
            plugin.note_on(NoteOn {
                voice: id,
                ..note(key)
            });
        }
        render(&mut plugin, 64);
        let rank = |plugin: &Harmor, id: u32| {
            let v = plugin.voices.iter().find(|v| v.id == id).unwrap();
            v.runs[1].env.arp_rank()
        };
        // Up, each key once.
        assert_eq!(rank(&plugin, 2), (0, 3));
        assert_eq!(rank(&plugin, 4), (0, 3));
        assert_eq!(rank(&plugin, 1), (1, 3));
        assert_eq!(rank(&plugin, 3), (2, 3));
        // InvArpBox turns it over.
        plugin.state.globals.params[16] = 1;
        plugin.note_off(1);
        render(&mut plugin, 64);
        assert_eq!(rank(&plugin, 3), (0, 2));
        assert_eq!(rank(&plugin, 2), (1, 2));
    }

    #[test]
    fn mono_kill_all_ends_the_last_note() {
        let mut plugin = Harmor::default();
        plugin.state.globals.mono = 2;
        plugin.note_on(note(60));
        render(&mut plugin, 4410);
        plugin.note_on(NoteOn {
            voice: 2,
            ..note(64)
        });
        render(&mut plugin, 4410);
        let ids: Vec<u32> = plugin
            .voices
            .iter()
            .filter(|v| v.active)
            .map(|v| v.id)
            .collect();
        assert_eq!(ids, vec![2]);
    }

    #[test]
    fn every_project_state_plays_a_sane_note() {
        let Some(root) = corpus() else {
            return;
        };
        let generators = root.parent().unwrap().join("Plugins/Fruity/Generators");
        let mut states = projects(&root);
        states.retain(|s| s.plugin.eq_ignore_ascii_case(NAME));
        assert_eq!(states.len(), 100);
        let (mut silent, mut loud) = (Vec::new(), Vec::new());
        for saved in states {
            let mut plugin = Harmor::default();
            plugin.set_resource_dir(&generators);
            plugin.set_state(&saved.state).unwrap();
            plugin.note_on(note(60));
            let (l, r) = render(&mut plugin, 22_050);
            plugin.note_off(1);
            let (l2, r2) = render(&mut plugin, 22_050);
            let all = l.iter().chain(&r).chain(&l2).chain(&r2);
            assert!(all.clone().all(|v| v.is_finite()), "{}", saved.file);
            let peak = l.iter().chain(&r).fold(0.0f32, |m, v| m.max(v.abs()));
            eprintln!("{} peak {peak:.4}", saved.file);
            if peak < 1e-3 {
                silent.push(saved.file.clone());
            }
            loud.extend((peak >= 8.0).then(|| saved.file.clone()));
        }
        eprintln!("silent: {silent:?}");
        assert!(loud.is_empty(), "{loud:?}");
    }
}
