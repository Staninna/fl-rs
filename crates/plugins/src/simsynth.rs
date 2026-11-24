//! SimSynth (David Billen's SimSynth Live): three wavetable oscillators,
//! a state variable filter, two envelopes and an LFO per voice, twelve
//! voices and a chorus, rebuilt from `SimSynth_Eng_x64.dll`
//! (`docs/plugins/simsynth.md`). `SimSynth_x64.dll` beside it is the
//! panel only.
//!
//! The engine keeps the knobs as 45 f64 and 19 i32 (`SaveRestoreState`
//! `0x18000a610`), turns them into a patch of integers (`0x180001820`,
//! `0x180002340`) and builds each voice from the patch (`0x180008180`).
//! Everything that sounds is ported as the DLL computes it, f32 for f32,
//! and checked against the DLL run under unicorn
//! (`.scratch/notes/simsynth-emu/`).
//!
//! What cannot be FL's to the bit: the oscillators start at random phases
//! read from a noise table whose read position is global to the DLL, so
//! the phases depend on how many SimSynth instances FL made and in what
//! order it called them. This port starts from one instance that FL
//! created and gave a sample rate (`NOISE_AFTER_CREATE`).

use crate::generator::{Generator, NoteOn};
use crate::{Fidelity, StateError};

pub const NAME: &str = "SimSynth";

/// The state: 45 f64, 19 i32 and 12 unused i32 in their places, the
/// version.
pub(crate) const STATE_LEN: usize = 564;
const DOUBLES: usize = 45;
const INTS: usize = 19;
const INTS_AT: usize = 360 + 76;
const VERSION_AT: usize = 560;
/// The version this engine writes.
const VERSION: i32 = 14;

const VOICES: usize = 12;
const TABLE: usize = 4096;
const PITCHES: usize = 30_000;
/// The pitch table's last entry the note map reaches (`0x180008f40`).
const PITCH_TOP: usize = 29_996;
const NOISE: usize = 65_536;
/// The chorus's delay line, in samples.
const LINE: usize = 65_536;
/// The most frames one engine render runs: longer blocks are cut up.
const BLOCK: usize = 512;

/// The noise read position after `CreatePlugInstance` and one sample rate
/// (`emu`: 96 after the create, 120 after the rate).
const NOISE_AFTER_CREATE: u16 = 96;

/// The chorus's length in milliseconds (`0x180009ca0`): 333 plus a
/// counter global to the DLL that moves on 5 with every chorus set-up, so
/// 338 for one instance that FL gave a sample rate.
const CHORUS_SPAN: i32 = 338;

/// The parameters as the engine numbers them (`ProcessParam`
/// `0x18000ac40`, table `0x1800831e0`): a knob scales the f64 at the index
/// by its maximum, a button sets the i32 at the index, a shape picks one
/// of [`SHAPES`] for the f64.
#[derive(Clone, Copy)]
enum Param {
    Knob(usize, i32),
    Button(usize),
    Shape(usize),
}

const PARAMS: [Param; 58] = {
    use Param::{Button as B, Knob as K, Shape as S};
    const M: i32 = 65_535;
    const C: i32 = 20_160;
    [
        K(3, M),  // Osc1 Level
        K(4, M),  // Osc1 LFO
        K(5, M),  // Osc1 Env
        K(10, M), // Osc2 Level
        K(11, M), // Osc2 LFO
        K(12, M), // Osc2 Env
        K(17, M), // Osc3 Level
        K(18, M), // Osc3 LFO
        K(19, M), // Osc3 Env
        K(21, M), // LFO Delay
        K(22, M), // LFO Rate
        K(26, M), // SVF Cut
        K(27, M), // SVF Emphasis
        K(28, M), // SVF Env
        K(29, M), // SVF LFO
        K(32, M), // SVF High
        K(33, M), // SVF Band
        K(35, M), // Env Attack
        K(36, M), // Env Decay
        K(37, M), // Env Sustain
        K(38, M), // Env Release
        K(39, M), // VCA Velo (the voice volume)
        K(41, M), // SVF Env Attack
        K(42, M), // SVF Env Decay
        K(43, M), // SVF Env Sustain
        K(44, M), // SVF Env Release
        K(0, M),  // Osc1 Pulse Width
        K(1, C),  // Osc1 Coarse
        K(2, M),  // Osc1 Fine
        K(7, M),  // Osc2 Pulse Width
        K(8, C),  // Osc2 Coarse
        K(9, M),  // Osc2 Fine
        K(14, M), // Osc3 Pulse Width
        K(15, C), // Osc3 Coarse
        K(16, M), // Osc3 Fine
        K(30, M), // SVF Kb
        B(18),    // Chorus
        B(13),    // Retrigger
        B(16),    // SVF Track Amp
        B(12),    // LFO On
        B(14),    // SVF On
        B(0),     // Osc1 On
        B(4),     // Osc2 On
        B(8),     // Osc3 On
        B(17),    // Ring Osc1 x Osc2
        B(1),     // Osc1 +1 Octave
        B(2),     // Osc1 +2 Octaves
        B(3),     // Osc1 Warm
        B(5),     // Osc2 +1 Octave
        B(6),     // Osc2 +2 Octaves
        B(7),     // Osc2 Warm
        B(9),     // Osc3 +1 Octave
        B(10),    // Osc3 +2 Octaves
        B(11),    // Osc3 Warm
        S(6),     // Osc1 shape
        S(13),    // Osc2 shape
        S(20),    // Osc3 shape
        S(24),    // LFO shape
    ]
};

/// What a shape selector's position stores (`0x180083540`): pulse, saw,
/// triangle, noise, sine to sine^4.
const SHAPES: [f64; 5] = [0.2, 0.1, 0.0, 0.4, 0.3];

/// The patch `FUN_180009f20` loads into a new instance.
const DEFAULT_DOUBLES: [f64; DOUBLES] = [
    0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5,
    0.5, 0.5, 0.0, 0.5, 0.5, 0.5, 0.0, 0.25, 1.0, 0.6, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0,
    0.0, 0.5, 1.0, 0.0, 0.5, 0.0, 0.0,
];
const DEFAULT_INTS: [i32; INTS] = [1, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1];

/// The SVF off value of the patch's filter word (`0xc178`).
const SVF_OFF: i32 = 0x2253;

/// One oscillator's integers in the patch (`0xc008 + 0x34 i`).
#[derive(Debug, Clone, Copy, PartialEq)]
struct OscPatch {
    lfo: i32,
    pw: i32,
    coarse: i32,
    fine: i32,
    level: i32,
    env: i32,
    off: bool,
    up1: i32,
    up2: i32,
    warm: i32,
    shape: i32,
}

/// The engine's patch (`0x180007f10` builds it, `0x180001820` and
/// `0x180002340` fill it), less the parts nothing reads.
#[derive(Debug, Clone, PartialEq)]
struct Patch {
    osc: [OscPatch; 3],
    ring: i32,
    lfo_rate: i32,
    lfo_delay: i32,
    lfo_off: bool,
    retrigger: i32,
    lfo_shape: i32,
    amp_times: [u32; 5],
    amp_levels: [i32; 5],
    flt_times: [u32; 5],
    flt_levels: [i32; 5],
    /// `0xc128`: the filter follows its own envelope (Track Amp off).
    own_env: bool,
    cut: i32,
    env_amt: i32,
    lfo_amt: i32,
    emph: i32,
    high: i32,
    band: i32,
    kb: f32,
    svf: i32,
}

impl Default for Patch {
    /// `FUN_180007f10`.
    fn default() -> Self {
        let osc = OscPatch {
            lfo: 0,
            pw: 0x32,
            coarse: 0x48,
            fine: 0,
            level: 0xff,
            env: 0,
            off: false,
            up1: 0,
            up2: 0,
            warm: 0,
            shape: 1,
        };
        let mut two = osc;
        two.off = true;
        Patch {
            osc: [osc, two, two],
            ring: 0,
            lfo_rate: 0xc,
            lfo_delay: 0,
            lfo_off: false,
            retrigger: 0,
            lfo_shape: 2,
            amp_times: [0, 400, 0, 0, 2],
            amp_levels: [0xff, 0, 0, 0, 0],
            flt_times: [0, 0x40, 0, 0, 2],
            flt_levels: [0xff, 0, 0, 0, 0],
            own_env: false,
            cut: 0x40,
            env_amt: 0x40,
            lfo_amt: 0,
            emph: 0,
            high: 0xff,
            band: 0,
            kb: 0.0,
            svf: 0x2254,
        }
    }
}

/// The DLL's tables (`0x180008830`, `0x180008f40`).
struct Tables {
    /// The rate they were made for, twice the host's: the oscillators
    /// step twice a sample.
    rate: f32,
    /// Phase steps by cents from 4.0887 Hz, for `rate`.
    step: Vec<u32>,
    /// `rand() / 32767` after `srand(666)`.
    noise: Vec<f32>,
    saw: Vec<f32>,
    tri: Vec<f32>,
    sine: Vec<f32>,
    sine4: Vec<f32>,
}

impl Tables {
    fn new(rate: f32) -> Self {
        // `0x180008f40` regenerates the noise every time the rate moves,
        // and every patch build ends in `srand(666)` (`0x180008180`): so
        // the table is the run after that seed.
        let mut hold: u32 = 666;
        let noise = (0..NOISE)
            .map(|_| {
                hold = hold.wrapping_mul(214_013).wrapping_add(2_531_011);
                ((hold >> 16) & 0x7fff) as f32 / 32_767.0
            })
            .collect();
        // sin(x pi/2) for x from 1 down by 1/2048 in f32 steps.
        let mut x = 1.0f32;
        let saw = (0..TABLE)
            .map(|_| {
                let v = (f64::from(x) * std::f64::consts::FRAC_PI_2).sin() as f32;
                x += -0.000_488_281_25;
                v
            })
            .collect();
        let mut a = 0.0f32;
        let sines: Vec<f32> = (0..TABLE)
            .map(|_| {
                let v = f64::from(a).sin() as f32;
                a += f32::from_bits(0x3ac9_0fdb);
                v
            })
            .collect();
        let sine4 = sines.iter().map(|v| v * v * v * v).collect();
        let mut t = 0.0f32;
        let mut tri = Vec::with_capacity(TABLE);
        for (n, d) in [
            (1024, 0.000_976_562_5f32),
            (2048, -0.000_976_562_5),
            (1024, 0.000_976_562_5),
        ] {
            for _ in 0..n {
                tri.push(t);
                t += d;
            }
        }
        let mut tables = Tables {
            rate: 0.0,
            step: vec![0; PITCHES + 21],
            noise,
            saw,
            tri,
            sine: sines,
            sine4,
        };
        tables.set_rate(rate);
        tables
    }

    fn set_rate(&mut self, rate: f32) {
        if rate == self.rate {
            return;
        }
        self.rate = rate;
        let k = 4_294_967_296.0f32 / rate;
        for (i, step) in self.step.iter_mut().enumerate().take(PITCHES) {
            let hz = (2f64.powf(i as f64 / 1200.0) * 4.088_681_697_845_459) as f32;
            *step = (k * hz) as i64 as u32;
        }
        // Past the end the DLL reads its next table, the sync periods;
        // only pitches past 30 kHz reach there.
        for i in PITCHES..self.step.len() {
            self.step[i] = self.step[PITCHES - 1];
        }
    }

    /// The note map `0x1800dc4b0`, for an index `pitch + 32`.
    fn map(index: i32) -> usize {
        match index {
            i if i >= 30_028 => PITCH_TOP,
            i if i >= 74 => (i - 32) as usize,
            i if (42..74).contains(&i) => 0,
            _ => 10,
        }
    }
}

/// `x as i32` as `cvttss2si` does it: out of range is `i32::MIN`.
fn trunc(x: f32) -> i32 {
    if x.is_nan() || !(-2_147_483_648.0..2_147_483_648.0).contains(&x) {
        i32::MIN
    } else {
        x as i32
    }
}

fn trunc64(x: f64) -> i32 {
    if x.is_nan() || !(-2_147_483_648.0..2_147_483_648.0).contains(&x) {
        i32::MIN
    } else {
        x as i32
    }
}

/// A three-stage envelope (`FUN_1800045f0`): attack, decay, a sustain of
/// 0x4fffffff samples, release.
#[derive(Debug, Clone, Copy, Default)]
struct Env {
    stage: u32,
    count: i32,
    step: f32,
    counts: [i32; 5],
    steps: [f32; 5],
    levels: [f32; 5],
    value: f32,
}

impl Env {
    fn set(&mut self, times: [u32; 5], levels: [i32; 5], ms: f32) {
        let level = |i: usize| {
            let v = (levels[i] as i16 as u16) << 8;
            let v = if v == 0x100 { 1 } else { v };
            f32::from(v) / 65_280.0
        };
        let samples = |t: u32| trunc(t as f32 * ms);
        let sustain = level(2);
        let attack = samples(times[0]);
        let decay = samples(times[1]);
        let release = samples(times[3]);
        self.counts = [attack, decay, 0x4fff_ffff, release, 0x4fff_ffff];
        self.steps = [
            if attack == 0 {
                0.0
            } else {
                1.0 / attack as f32
            },
            if decay == 0 {
                0.0
            } else {
                (sustain - 1.0) / decay as f32
            },
            0.0,
            if release == 0 {
                0.0
            } else {
                (0.0 - sustain) / release as f32
            },
            0.0,
        ];
        self.levels = [1.0, sustain, sustain, 0.0, 0.0];
        self.count = attack;
        self.step = self.steps[0];
        self.stage = 0;
        self.value = 0.0;
    }

    fn start(&mut self) {
        self.count = self.counts[0];
        self.step = self.steps[0];
        self.stage = 0;
        self.value = 0.0;
    }

    /// `FUN_180007980`.
    fn release(&mut self) {
        if self.stage < 3 {
            let n = if self.counts[3] == 0 {
                1
            } else {
                self.counts[3]
            };
            self.count = n;
            self.stage = 3;
            self.step = -(self.value / n as u32 as f32);
        }
    }

    /// One sample of the render loop: the value, and whether the stage is
    /// past the attack.
    fn tick(&mut self) -> (f32, bool) {
        let mut count = self.count;
        loop {
            if count != 0 {
                self.value += self.step;
                self.count = count - 1;
                return (self.value, self.stage != 0);
            }
            let v = self.levels[self.stage as usize];
            self.value = v;
            self.stage += 1;
            count += self.counts[self.stage as usize];
            self.count = count;
            self.step = self.steps[self.stage as usize];
            if self.stage == 2 && v == 0.0 {
                self.stage = 4;
                self.count = -1;
                self.step = 0.0;
                return (v, true);
            }
        }
    }
}

/// The LFO (`p_lfo`, tick `FUN_180004a30`).
#[derive(Debug, Clone, Copy, Default)]
struct Lfo {
    shape: i32,
    count: i32,
    period: u32,
    inv: f32,
    half: u32,
    ramp: f32,
    random: f32,
    gain: f32,
    gain_step: f32,
    gain_count: i32,
    delay: u32,
    retrigger: i32,
    out: [f32; 2],
}

impl Lfo {
    /// `FUN_180004b80`.
    fn set_rate(&mut self, rate: i32, hz: f32) {
        let f = 1.0 - (rate as u16) as f32 / 999.0;
        let mut period = (f * f * f * hz * 4.0) as i64 as u32;
        if period < 0x40 {
            period = 0x40;
        }
        self.period = period;
        self.inv = 1.0 / period as f32;
        self.count = (period as f32 * self.ramp) as i64 as i32;
    }

    /// `FUN_180004c10`.
    fn start(&mut self, at: u32, noise: &[f32], idx: &mut u16) {
        if at == 0 {
            self.gain_count = 0;
            if self.delay != 0 {
                let n = (self.delay as i32).wrapping_mul(0xdc8);
                self.gain = 0.0;
                self.gain_count = n;
                self.gain_step = 1.0 / n as f32;
            }
        }
        let period = self.period.max(1);
        let r = at % period;
        self.half = (at / period) & 1;
        self.ramp = 1.0 - r as f32 * self.inv;
        self.count = (period as f32 * self.ramp) as i64 as i32;
        if r == 0 {
            self.random = noise[usize::from(*idx)];
            *idx = idx.wrapping_add(1);
        }
    }

    fn tick(&mut self, noise: &[f32], idx: &mut u16) {
        self.count -= 1;
        let mut r = self.ramp - self.inv;
        self.ramp = r;
        let mut half = self.half;
        if self.count == 0 {
            half = half.wrapping_sub(1) & 1;
            self.count = self.period as i32;
            self.half = half;
            self.ramp = 1.0;
            self.random = noise[usize::from(*idx)];
            *idx = idx.wrapping_add(1);
            r = 1.0;
        }
        match (half * 4) | self.shape as u32 {
            0 => self.out = [0.0, 0.0],
            4 => self.out = [1.0, 1.0],
            1 | 5 => self.out = [r, r],
            3 | 7 => self.out = [self.random, self.random],
            2 => {
                let t = if r <= 0.5 { r } else { 1.0 - r };
                self.out = [1.0 - r, -(t + t)];
            }
            6 => {
                let t = if 0.5 < r { 1.0 - r } else { r };
                self.out = [r, t + t];
            }
            _ => {}
        }
        if self.gain_count != 0 {
            let g = self.gain;
            self.gain_count -= 1;
            self.out[0] *= g;
            self.out[1] *= g;
            self.gain = g + self.gain_step;
        }
    }
}

/// An oscillator: the wavetable one the DLL builds at its quality
/// (`p_loopquality`, `FUN_180003940`) or the noise (`FUN_180003d60`).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Table,
    Noise,
}

#[derive(Debug, Clone, Copy)]
struct Osc {
    kind: Kind,
    /// `+0x10`: the pitch the steps were last worked out for.
    cached: u16,
    /// `+0x12`: the pitch before modulation.
    base: u16,
    level: f32,
    step: u32,
    phase: u32,
    step2: u32,
    phase2: u32,
    warm: bool,
    held: [f32; 2],
    /// `+0x40`: the voice's detune plus the oscillator's number, in
    /// cents and in steps.
    offset: i32,
    /// The noise's band pass.
    low: f32,
    band: f32,
}

impl Osc {
    fn new(kind: Kind, phase: u32) -> Self {
        Osc {
            kind,
            cached: 0,
            base: 0,
            level: 0.5,
            step: 0,
            phase,
            step2: 0,
            phase2: phase,
            warm: false,
            held: [0.0; 2],
            offset: 0,
            low: 0.0,
            band: 0.0,
        }
    }

    /// `FUN_1800039f0`.
    fn set_pitch(&mut self, offset: i32, steps: &[u32]) {
        if self.kind != Kind::Table {
            return;
        }
        let u = (offset as u16).wrapping_add(self.base);
        if u != self.cached {
            let m = Tables::map(self.offset + 0x20 + i32::from(u));
            self.cached = m as u16;
            self.step = steps[m].wrapping_add(self.offset as u32);
            self.step2 = steps[m + 20].wrapping_add(self.offset as u32);
        }
    }

    fn tick(&mut self, table: &[f32], noise: &[f32], idx: &mut u16) -> f32 {
        match self.kind {
            Kind::Table => {
                let third = f32::from_bits(0x3eaa_aa9f);
                let a = table[(self.phase >> 20) as usize] + self.held[0];
                self.phase = self.phase.wrapping_add(self.step);
                let b = table[(self.phase >> 20) as usize];
                self.phase = self.phase.wrapping_add(self.step);
                let one = (a + b) * third;
                self.held[0] = one;
                if !self.warm {
                    return one;
                }
                let a = table[(self.phase2 >> 20) as usize] + self.held[1];
                self.phase2 = self.phase2.wrapping_add(self.step2);
                let b = table[(self.phase2 >> 20) as usize];
                self.phase2 = self.phase2.wrapping_add(self.step2);
                let two = (a + b) * third;
                self.held[1] = two;
                two + one
            }
            Kind::Noise => {
                let k = f32::from_bits(0x3e9a_e148);
                let band = self.band;
                let n = noise[usize::from(*idx)];
                *idx = idx.wrapping_add(1);
                self.low += band * k;
                let high = (n - self.low) - band;
                self.band = high * k + band;
                self.band * 16.0
            }
        }
    }
}

/// The voice's filter (`p_filt`, `FUN_180004020`): two state variable
/// stages in f64, mixed low, band and high by the panel.
#[derive(Debug, Clone, Copy, Default)]
struct Svf {
    on: bool,
    fc: f32,
    base: f32,
    emph: u16,
    damp: f64,
    f1: f64,
    f2: f64,
    low_gain: f64,
    band_gain: f64,
    high_gain: f64,
    low1: f64,
    band1: f64,
    high1: f64,
    low2: f64,
    band2: f64,
    high2: f64,
}

impl Svf {
    /// `FUN_180005d70` and the note-on's copy: the emphasis moved by
    /// `offset`.
    fn set_emphasis(&mut self, offset: i32) {
        let e = (i32::from(self.emph) + offset).clamp(0, 255);
        let d = 1.0 - f64::from(e) / 255.0;
        self.damp = 1.1 - (1.0 - d * d);
    }

    /// `FUN_180004130`.
    fn set_cutoff(&mut self, offset: f32, ratio: f32) {
        let mut fc = offset + self.base;
        if fc > 1.0 {
            fc = 1.0;
        } else if fc < f32::from_bits(0x38d1_b717) {
            fc = f32::from_bits(0x38d1_b717);
        }
        self.fc = fc;
        self.f1 = f64::from(fc * ratio).min(1.0);
        self.f2 = (f64::from(ratio) * f64::from(fc * fc)).min(1.0);
    }

    fn clear(&mut self) {
        self.low1 = 0.0;
        self.band1 = 0.0;
        self.high1 = 0.0;
        self.low2 = 0.0;
        self.band2 = 0.0;
        self.high2 = 0.0;
    }

    fn run(&mut self, x: f32) -> f32 {
        let low2 = self.band2 * self.f1 + self.low2;
        self.low2 = low2;
        let low1 = self.f2 * self.band1 + self.low1;
        self.low1 = low1;
        let high1 = (f64::from(x) - low1) - self.damp * self.band1;
        self.high1 = high1;
        let band1 = high1 * self.f2 + self.band1;
        self.band1 = band1;
        let mix = ((band1 * self.band_gain + low1 * self.low_gain + high1 * self.high_gain) - low2)
            - self.band2 * self.damp;
        self.high2 = mix;
        let band2 = mix * self.f1 + self.band2;
        self.band2 = band2;
        (band2 * self.band_gain + low2 * self.low_gain + mix * self.high_gain) as f32
    }
}

/// One of the twelve voices: the engine's `patch` object (0x340 bytes)
/// and the plugin's wrapper around it.
#[derive(Debug, Clone, Copy)]
struct Voice {
    /// The host's note, or none for a free slot.
    note: Option<u32>,
    /// The note's levels as FL hands an old-format plugin them.
    pan: i32,
    volume: f32,
    pitch: i32,
    cutoff: f32,
    resonance: f32,
    /// The gains, their ramp and where it heads.
    gains: [f32; 2],
    ramp: i32,
    ramp_step: [f32; 2],
    seen_resonance: f32,
    started: u32,
    released: u32,
    amp: Env,
    flt: Env,
    lfo_on: bool,
    lfo: Lfo,
    osc: [Option<Osc>; 3],
    lfo_mod: [f32; 3],
    env_mod: [f32; 3],
    /// `+0x250`: the note's pitch plus 7200.
    root: i32,
    /// `+0x254`: per oscillator, the pitch the next note-on sets.
    next: [u16; 3],
    /// `+0x25a`: per oscillator, its pitch.
    pitches: [u16; 3],
    ring: bool,
    own_env: bool,
    filter: Svf,
    flt_env: Option<f32>,
    flt_lfo: Option<f32>,
    /// `+0x264`: the key tracking plus mod X.
    track: f32,
    level: f32,
    count: u32,
    detune: i32,
}

impl Default for Voice {
    fn default() -> Self {
        Voice {
            note: None,
            pan: 0,
            volume: 1.0,
            pitch: 0,
            cutoff: 0.0,
            resonance: 0.0,
            gains: [0.0; 2],
            ramp: 0,
            ramp_step: [0.0; 2],
            seen_resonance: 0.0,
            started: 0,
            released: 0,
            amp: Env {
                stage: 4,
                ..Env::default()
            },
            flt: Env {
                stage: 4,
                ..Env::default()
            },
            lfo_on: false,
            lfo: Lfo {
                period: 0x40,
                ..Lfo::default()
            },
            osc: [None; 3],
            lfo_mod: [0.0; 3],
            env_mod: [0.0; 3],
            root: 0,
            next: [0; 3],
            pitches: [0; 3],
            ring: false,
            own_env: false,
            filter: Svf::default(),
            flt_env: None,
            flt_lfo: None,
            track: 0.0,
            level: 0.0,
            count: 0,
            detune: 0,
        }
    }
}

/// The chorus (`FUN_180009ca0`, run in `Gen_Render` `0x18000ae50`): the
/// left side through a delay whose read speed swings about 1.
struct Chorus {
    line: Vec<f32>,
    write: usize,
    read: f64,
    speed: f64,
    swing: f64,
    high: f64,
    low: f64,
    tail: i32,
    span: i32,
}

impl Chorus {
    fn new(rate: f32) -> Self {
        let mut chorus = Chorus {
            line: vec![0.0; LINE + 1],
            write: 0,
            read: 0.0,
            speed: 1.0,
            swing: 0.0,
            high: 0.0,
            low: 0.0,
            tail: 0,
            span: 0,
        };
        chorus.set(rate);
        chorus
    }

    fn set(&mut self, rate: f32) {
        let ms = rate / 1000.0;
        self.write = 0;
        self.read = f64::from(65_536.0f32 - ms * 20.0);
        let d = 12.0 / 100.0;
        let d = d * d * 0.5;
        self.high = d + 1.0;
        self.low = 1.0 - d;
        self.speed = 1.0;
        let span = CHORUS_SPAN as f32 * ms;
        let swing = (self.high - self.low) / f64::from(span);
        self.swing = -(swing + swing);
        self.tail = 0;
        self.span = (ms * 20.0) as i32;
    }

    fn tick(&mut self, input: f32) -> f32 {
        self.line[self.write] = input;
        self.write = (self.write + 1) & (LINE - 1);
        let at = self.read;
        let i = at as i32;
        let a = self.line[i as usize & (LINE - 1)];
        let b = self.line[(i as u16).wrapping_add(1) as usize];
        let mut read = self.speed + at;
        if read >= 65_536.0 {
            read -= 65_536.0;
        }
        self.read = read;
        let swing = self.swing;
        let speed = swing + self.speed;
        self.speed = speed;
        if speed <= 0.9928 || 1.0072 <= speed {
            self.swing = -swing;
            if self.low <= speed {
                if self.high < speed {
                    self.speed = self.high;
                }
            } else {
                self.speed = self.low;
            }
        }
        (b - a) * (at as f32 - i as f32) + a
    }
}

/// The plugin.
pub struct SimSynth {
    doubles: [f64; DOUBLES],
    ints: [i32; INTS],
    patch: Patch,
    waves: Box<[[f32; TABLE]; 3]>,
    scratch: Box<[f32; TABLE]>,
    tables: Tables,
    rate: f32,
    voices: [Voice; VOICES],
    /// `0x1800dc4a0`.
    noise_at: u16,
    /// `0x40dd8`: samples rendered while something sounded.
    clock: u32,
    /// `0x1801376a8`: renders.
    renders: u32,
    chorus_on: bool,
    gain: f32,
    chorus: Chorus,
    mono: Vec<f32>,
    mix: Vec<f32>,
}

impl Default for SimSynth {
    fn default() -> Self {
        let rate = 44_100.0;
        let mut synth = SimSynth {
            doubles: DEFAULT_DOUBLES,
            ints: DEFAULT_INTS,
            patch: Patch::default(),
            waves: Box::new([[0.0; TABLE]; 3]),
            scratch: Box::new([0.0; TABLE]),
            tables: Tables::new(rate * 2.0),
            rate,
            voices: [Voice::default(); VOICES],
            noise_at: NOISE_AFTER_CREATE,
            clock: 0,
            renders: 0,
            chorus_on: true,
            gain: 0.5,
            chorus: Chorus::new(rate),
            mono: vec![0.0; BLOCK],
            mix: vec![0.0; 2 * BLOCK],
        };
        // The voices' detunes (`FUN_180002d60`).
        let (mut up, mut down, step) = (-8.0f64, 8.0f64, 8.0 / 6.0);
        let mut at = 0.0f64;
        for voice in &mut synth.voices {
            voice.detune = at as i32;
            if at >= 0.0 {
                at = up;
                up += step;
            } else {
                at = down;
                down -= step;
            }
        }
        // The default patch, and the sample rate's rebuild of every voice.
        synth.load(DEFAULT_DOUBLES, DEFAULT_INTS);
        synth
    }
}

impl SimSynth {
    /// `0x180002340` for every i32, then `0x180001820` for every f64,
    /// then the tables and every voice (`0x180002a30`).
    fn load(&mut self, doubles: [f64; DOUBLES], ints: [i32; INTS]) {
        self.doubles = doubles;
        self.ints = ints;
        for (i, v) in ints.into_iter().enumerate() {
            self.set_int(i, v);
        }
        for (i, v) in doubles.into_iter().enumerate() {
            self.set_double(i, v);
        }
        for i in 0..3 {
            self.build_wave(i);
        }
        for slot in 0..VOICES {
            self.build_voice(slot);
        }
    }

    fn set_int(&mut self, index: usize, v: i32) {
        let p = &mut self.patch;
        match index {
            0 => p.osc[0].off = v == 0,
            1 => p.osc[0].up1 = v,
            2 => p.osc[0].up2 = v,
            3 => p.osc[0].warm = v,
            4 => p.osc[1].off = v == 0,
            5 => p.osc[1].up1 = v,
            6 => p.osc[1].up2 = v,
            7 => p.osc[1].warm = v,
            8 => p.osc[2].off = v == 0,
            9 => p.osc[2].up1 = v,
            10 => p.osc[2].up2 = v,
            11 => p.osc[2].warm = v,
            12 => p.lfo_off = v == 0,
            13 => p.retrigger = v,
            14 => p.svf = if v != 0 { 0 } else { SVF_OFF },
            16 => p.own_env = v == 0,
            17 => p.ring = v,
            18 => self.chorus_on = v == 1,
            _ => {}
        }
    }

    fn set_double(&mut self, index: usize, v: f64) {
        let d = v.clamp(0.0, 1.0);
        let p = &mut self.patch;
        let osc = index / 7;
        if index < 21 {
            let o = &mut p.osc[osc];
            match index % 7 {
                0 => o.pw = 2 - trunc64(d * -96.0),
                1 => o.coarse = (trunc64((d * 48.0).floor() - 24.0)).wrapping_mul(100),
                2 => o.fine = trunc64(d * 200.0 - 100.0),
                3 => o.level = trunc64(d * 100.0),
                4 => o.lfo = trunc64(d * 200.0 - 100.0),
                5 => o.env = trunc64(d * 200.0 - 100.0),
                _ => {
                    if let Some(shape) = shape_of(d, true) {
                        o.shape = shape;
                    }
                }
            }
            return;
        }
        let cube = |d: f64| trunc64(d * d * d * 8000.0);
        let square = |d: f64| trunc64(d * d * 8000.0);
        match index {
            21 => p.lfo_delay = trunc64(d * 100.0),
            22 => p.lfo_rate = trunc64(d * 999.0),
            24 => {
                if let Some(shape) = shape_of(d, false) {
                    p.lfo_shape = shape;
                }
            }
            26 => p.cut = trunc64(d * 255.0),
            27 => p.emph = trunc64(d * 255.0),
            28 => p.env_amt = trunc64((d - 0.5) * 255.0),
            29 => p.lfo_amt = trunc64((d - 0.5) * 255.0),
            30 => p.kb = d as f32,
            32 => p.high = trunc64(d * 255.0),
            33 => p.band = trunc64(d * 255.0),
            35 => {
                p.amp_times[0] = cube(d) as u32;
                p.amp_levels[0] = 0xff;
            }
            36 => p.amp_times[1] = square(d) as u32,
            37 => {
                p.amp_levels[1] = trunc64(d * 255.0);
                p.amp_levels[2] = p.amp_levels[1];
                p.amp_times[2] = u32::MAX;
            }
            38 => {
                p.amp_levels[3] = 0;
                p.amp_times[3] = square(d) as u32;
            }
            39 => self.gain = d as f32,
            41 => {
                p.flt_times[0] = cube(d) as u32;
                p.flt_levels[0] = 0xff;
            }
            42 => p.flt_times[1] = square(d) as u32,
            43 => {
                p.flt_levels[1] = trunc64(d * 255.0);
                p.flt_levels[2] = p.flt_levels[1];
                p.flt_times[2] = u32::MAX;
            }
            44 => {
                p.flt_levels[3] = 0;
                p.flt_times[3] = square(d) as u32;
            }
            _ => {}
        }
    }

    /// `FUN_180007ae0`: an oscillator's wavetable from its shape, pulse
    /// width and octave buttons, smoothed by a two-tap mean.
    fn build_wave(&mut self, i: usize) {
        let o = self.patch.osc[i];
        let pw = o.pw as u16;
        let s = &mut self.scratch;
        match o.shape {
            0 => {
                let (k, c) = (f32::from_bits(0x3f26_6666), f32::from_bits(0x3e68_f5c2));
                let high = f32::from(pw) * 0.01 * 4096.0;
                let mut x = 0.0f32;
                let mut n = 0usize;
                if 0.0 < high {
                    loop {
                        n += 1;
                        x = x * k + c;
                        s[n - 1] = x;
                        if (n as i32) as f32 >= high {
                            break;
                        }
                    }
                }
                while n < 0xff8 {
                    x = x * k - c;
                    s[n] = x;
                    n += 1;
                }
                n = n.max(0xff8);
                while n < TABLE {
                    x *= k;
                    s[n] = x;
                    n += 1;
                }
            }
            1 => s.copy_from_slice(&self.tables.saw),
            2 => s.copy_from_slice(&self.tables.tri),
            4 => {
                let mix = f32::from(pw.wrapping_add(2)) / 96.0;
                let rest = 1.0 - mix;
                for (n, v) in s.iter_mut().enumerate() {
                    *v = self.tables.sine[n] * rest + self.tables.sine4[n] * mix;
                }
            }
            _ => {}
        }
        let w = &mut self.waves[i];
        w.copy_from_slice(&s[..]);
        match (o.up1 != 0, o.up2 != 0) {
            (false, true) => {
                for n in 0..TABLE {
                    w[n] = s[n] + s[(4 * n) & (TABLE - 1)];
                }
            }
            (true, false) => {
                for n in 0..TABLE {
                    w[n] = s[n] + s[(2 * n) & (TABLE - 1)];
                }
            }
            (true, true) => {
                for n in 0..TABLE {
                    w[n] = s[(2 * n) & (TABLE - 1)] + s[(4 * n) & (TABLE - 1)] + s[n];
                }
            }
            (false, false) => {}
        }
        for n in 0..TABLE - 1 {
            w[n] = (w[n] + w[n + 1]) * 0.5;
        }
    }

    /// `FUN_180008180`: a voice made over from the patch. The voice falls
    /// silent.
    fn build_voice(&mut self, slot: usize) {
        let p = &self.patch;
        let ms = self.rate / 1000.0;
        let noise = &self.tables.noise;
        let v = &mut self.voices[slot];
        let mut amp_times = p.amp_times;
        amp_times[3] = amp_times[3].max(2);
        v.amp.set(amp_times, p.amp_levels, ms);
        v.own_env = p.own_env;
        let mut flt_times = p.flt_times;
        flt_times[3] = flt_times[3].max(1);
        v.flt.set(flt_times, p.flt_levels, ms);
        v.lfo_on = false;
        v.lfo.out[0] = 0.0;
        if !p.lfo_off {
            v.lfo_on = true;
            v.lfo.shape = p.lfo_shape;
            v.lfo.set_rate(p.lfo_rate, self.rate);
            v.lfo.retrigger = p.retrigger;
            v.lfo.delay = p.lfo_delay as u16 as u32;
            v.lfo.half = 0;
            v.lfo.start(0, noise, &mut self.noise_at);
        }
        // `FUN_180006660`: the oscillators.
        for i in 0..3 {
            let o = p.osc[i];
            v.next[i] = (o.coarse as u16).wrapping_add(o.fine as u16);
            v.lfo_mod[i] = 0.0;
            v.env_mod[i] = 0.0;
            if o.off {
                v.osc[i] = None;
                continue;
            }
            let kind = if o.shape == 3 {
                Kind::Noise
            } else {
                Kind::Table
            };
            let osc = match v.osc[i] {
                Some(osc) if osc.kind == kind => v.osc[i].insert(osc),
                old => v.osc[i].insert(Osc::new(kind, old.map_or(0, |o| o.phase))),
            };
            let base = (o.coarse as u16)
                .wrapping_add(v.root as u16)
                .wrapping_add(o.fine as u16);
            osc.cached = base;
            osc.base = base;
            match kind {
                Kind::Table => {
                    osc.warm = o.warm != 0;
                    osc.level = 0.5;
                    osc.phase2 = osc.phase;
                    osc.set_pitch(1, &self.tables.step);
                    osc.set_pitch(0x12c0, &self.tables.step);
                    let n = noise[usize::from(self.noise_at)];
                    self.noise_at = self.noise_at.wrapping_add(1);
                    let phase = trunc(n * 2_147_483_648.0) as u32;
                    osc.phase = phase;
                    osc.phase2 = phase;
                    osc.offset = v.detune + i as i32;
                }
                Kind::Noise => osc.level = 1.0,
            }
            if o.lfo != 0 {
                v.lfo_mod[i] = o.lfo as f32 * 12.0;
            }
            if o.env != 0 {
                v.env_mod[i] = o.env as f32 * 12.0;
            }
        }
        for i in 0..3 {
            if let Some(osc) = &mut v.osc[i] {
                v.pitches[i] = osc.base;
                let l = p.osc[i].level as f32 / 100.0;
                osc.level = l * l;
            }
        }
        v.ring = p.ring != 0;
        v.filter.on = false;
        if p.svf as u16 != SVF_OFF as u16 {
            let f = &mut v.filter;
            let band = f64::from(p.band as u16) / 255.0 * std::f64::consts::FRAC_PI_2;
            let high = f64::from(p.high as u16) / 255.0 * std::f64::consts::FRAC_PI_2;
            f.base = f32::from(p.cut as u16) / 255.0;
            f.high_gain = band.cos() * high.sin();
            f.low_gain = band.cos() * high.cos();
            f.band_gain = band.sin();
            f.emph = p.emph as u16;
            f.set_emphasis(0);
            f.on = true;
            v.flt_env = (p.env_amt != 0).then_some(p.env_amt as f32 * 0.007_812_5);
            v.flt_lfo = (p.lfo_amt != 0).then_some(p.lfo_amt as f32 * 0.007_812_5);
            f.clear();
        }
        v.count = 0;
        v.next[0] = v.pitches[0];
        v.next[1] = v.pitches[1].wrapping_sub(4);
        v.next[2] = v.pitches[2].wrapping_add(4);
        for i in 0..3 {
            if let Some(osc) = &mut v.osc[i] {
                osc.base = v.next[i];
                osc.set_pitch(0, &self.tables.step);
            }
        }
        v.lfo.start(0, noise, &mut self.noise_at);
        v.level = 0.0;
        v.amp.stage = 4;
        v.flt.stage = 4;
    }

    /// The pitch a note sets for the block (`FUN_180001700` and
    /// `FUN_180006510`): the note's cents plus 4800, at least 100, plus
    /// 2400.
    fn set_root(&mut self, slot: usize, pitch: i32) {
        let v = &mut self.voices[slot];
        v.root = pitch.wrapping_add(4800).max(100).wrapping_add(2400);
        for i in 0..3 {
            if let Some(osc) = &mut v.osc[i] {
                let o = self.patch.osc[i];
                v.next[i] = (o.fine as u16).wrapping_add(o.coarse as u16);
                osc.base = (v.root as u16).wrapping_add(v.next[i]);
                v.pitches[i] = osc.base;
            }
        }
    }

    /// `TriggerVoice` (`0x18000b700`) and the voice's note-on
    /// (`FUN_1800061a0`).
    fn trigger(&mut self, note: NoteOn) {
        let slot = self.free_slot();
        let pan = (note.pan * 64.0).round() as i32;
        let pitch = ((f32::from(note.key) - 60.0) * 100.0 + note.fine_pitch).round() as i32;
        let gains = lr(pan, self.gain * note.velocity);
        let v = &mut self.voices[slot];
        v.note = Some(note.voice);
        v.pan = pan;
        v.volume = note.velocity;
        v.pitch = pitch;
        v.cutoff = note.mod_x;
        v.resonance = note.mod_y;
        v.gains = gains;
        v.ramp = 0;
        v.seen_resonance = note.mod_y;
        v.started = self.renders;
        v.released = 0;
        // The engine reads the old format's integer pitch as a float here
        // (`cvttss2si` at 0x18000bb84): 0 for a pitch from 0 up, and out
        // of range below. The blocks put the right pitch in.
        let read = trunc(f32::from_bits(pitch as u32));
        let at = read.wrapping_add(0x960).wrapping_add(2400).max(100);
        v.root = at.wrapping_add(0x960);
        for i in 0..3 {
            if let Some(osc) = &mut v.osc[i] {
                let o = self.patch.osc[i];
                v.next[i] = (o.fine as u16).wrapping_add(o.coarse as u16);
                osc.base = (v.root as u16).wrapping_add(v.next[i]);
                v.pitches[i] = osc.base;
            }
        }
        v.filter.set_emphasis(-trunc(note.mod_y * -255.0));
        v.track = (at as f32 / 12_700.0) * self.patch.kb + note.mod_x;
        v.count = 0;
        if v.amp.value == 0.0 && v.filter.on {
            v.filter.clear();
        }
        v.amp.start();
        v.flt.start();
        v.next[0] = v.pitches[0];
        v.next[1] = v.pitches[1].wrapping_sub(4);
        v.next[2] = v.pitches[2].wrapping_add(4);
        for i in 0..3 {
            if let Some(osc) = &mut v.osc[i] {
                osc.phase2 = osc.phase;
                osc.base = v.next[i];
                osc.set_pitch(0, &self.tables.step);
            }
        }
        if v.lfo_on {
            let at = if v.lfo.retrigger != 0 { 0 } else { self.clock };
            v.lfo.start(at, &self.tables.noise, &mut self.noise_at);
        }
        v.level = 0.0;
    }

    /// The slot a new note takes: the first free one, else the one let go
    /// of longest ago, else the one started last.
    fn free_slot(&mut self) -> usize {
        if let Some(slot) = self.voices.iter().position(|v| v.note.is_none()) {
            return slot;
        }
        let t = self.renders;
        let mut best: Option<(usize, u32)> = None;
        for (slot, v) in self.voices.iter().enumerate() {
            let age = t.wrapping_sub(v.released);
            if age != t && age != 0 && best.is_none_or(|(_, b)| b < age) {
                best = Some((slot, age));
            }
        }
        let slot = match best {
            Some((slot, _)) => slot,
            None => {
                let mut slot = 0;
                for (s, v) in self.voices.iter().enumerate() {
                    if v.started > self.voices[slot].started {
                        slot = s;
                    }
                }
                slot
            }
        };
        let v = &mut self.voices[slot];
        v.amp.stage = 4;
        v.flt.stage = 4;
        v.note = None;
        slot
    }

    /// One voice's samples into `self.mono` (`FUN_1800050c0`).
    fn render_voice(&mut self, slot: usize, n: usize) {
        let ratio = 44_100.0 / self.rate;
        let tables = &self.tables;
        let waves = &self.waves;
        let noise_at = &mut self.noise_at;
        let v = &mut self.voices[slot];
        let ring = v.ring && v.osc[0].is_some() && v.osc[1].is_some();
        let mut level = v.level;
        let (mut lfo_pitch, mut lfo_filter) = (0.0f32, 0.0f32);
        for out in &mut self.mono[..n] {
            let (env, past) = v.amp.tick();
            let env = if past { env * env } else { env };
            let (fenv, _) = v.flt.tick();
            let fmod = if v.own_env { fenv } else { env };
            if v.lfo_on {
                v.lfo.tick(&tables.noise, noise_at);
                lfo_filter = v.lfo.out[0];
                lfo_pitch = v.lfo.out[1];
            }
            let update = v.count & 0x1f == 0;
            v.count = v.count.wrapping_add(1);
            let mut sum = 0.0f32;
            for i in 0..3 {
                let Some(osc) = &mut v.osc[i] else {
                    continue;
                };
                if update {
                    let mut m = 0.0f32;
                    if v.lfo_mod[i] != 0.0 {
                        m += lfo_pitch * v.lfo_mod[i];
                    }
                    if v.env_mod[i] != 0.0 {
                        m += env * v.env_mod[i];
                    }
                    osc.set_pitch(trunc(m), &tables.step);
                }
                let s = osc.tick(&waves[i], &tables.noise, noise_at) * osc.level;
                sum = match i {
                    0 => s + 0.0,
                    1 if ring => sum * (s + s),
                    _ => sum + s,
                };
            }
            if v.filter.on {
                if update {
                    let mut c = v.track;
                    if let Some(k) = v.flt_env {
                        c += fmod * k;
                    }
                    if let Some(k) = v.flt_lfo {
                        c += lfo_filter * k;
                    }
                    v.filter.set_cutoff(c, ratio);
                }
                sum = v.filter.run(sum);
            }
            level += (env * env - level) * f32::from_bits(0x3cf5_c28f);
            if level < f32::from_bits(0x3380_0001) && f32::from_bits(0xb380_0001) < level {
                level = 0.0;
            }
            *out = sum * level;
        }
        v.level = level;
    }

    /// `Gen_Render` (`0x18000ae50`) for up to [`BLOCK`] frames.
    fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len();
        let mut sounding = false;
        for slot in 0..VOICES {
            if self.voices[slot].note.is_none() || self.voices[slot].amp.stage == 4 {
                continue;
            }
            if !sounding {
                self.mix[..2 * n].fill(0.0);
                sounding = true;
            }
            let v = &mut self.voices[slot];
            if v.ramp == 0 {
                let g = lr(v.pan, v.volume * self.gain);
                if g != v.gains {
                    v.ramp = 0x80;
                    v.ramp_step = [
                        (g[0] - v.gains[0]) * 0.007_812_5,
                        (g[1] - v.gains[1]) * 0.007_812_5,
                    ];
                }
            }
            let pitch = v.pitch;
            let (cutoff, resonance) = (v.cutoff, v.resonance);
            self.set_root(slot, pitch);
            let v = &mut self.voices[slot];
            let p = (pitch.wrapping_add(0x960)) as f32;
            let at = (2400.0f32 + p).max(100.0);
            v.track = (at / 12_700.0) * self.patch.kb + cutoff;
            if resonance != v.seen_resonance {
                v.seen_resonance = resonance;
                v.filter.set_emphasis(trunc(resonance * 255.0));
            }
            self.render_voice(slot, n);
            let v = &mut self.voices[slot];
            let mut g = v.gains;
            let mut ramp = v.ramp;
            for (frame, s) in self.mix[..2 * n].chunks_exact_mut(2).zip(&self.mono[..n]) {
                if ramp != 0 {
                    g[0] += v.ramp_step[0];
                    g[1] += v.ramp_step[1];
                    ramp -= 1;
                }
                frame[0] += s * g[0];
                frame[1] += s * g[1];
            }
            v.gains = g;
            v.ramp = ramp;
        }
        let mut done = n;
        if sounding {
            if !self.chorus_on {
                for (i, frame) in self.mix[..2 * n].chunks_exact(2).enumerate() {
                    left[i] += frame[0];
                    right[i] += frame[1];
                }
                for l in left.iter_mut() {
                    if self.chorus.tail == 0 {
                        break;
                    }
                    *l += self.chorus.tick(0.0);
                    self.chorus.tail -= 1;
                }
            } else {
                for (i, frame) in self.mix[..2 * n].chunks_exact(2).enumerate() {
                    left[i] += self.chorus.tick(frame[0]);
                    right[i] += frame[1];
                }
                self.chorus.tail = self.chorus.span;
            }
        } else if !self.chorus_on || self.chorus.tail == 0 {
            done = 0;
        } else {
            // The DLL counts the frame the tail ends on out of the block.
            let mut ran = n;
            for (i, l) in left.iter_mut().enumerate() {
                *l += self.chorus.tick(0.0);
                self.chorus.tail -= 1;
                if self.chorus.tail == 0 {
                    ran = i;
                    break;
                }
            }
            done = ran;
        }
        for v in &mut self.voices {
            if v.note.is_some() && v.amp.stage == 4 {
                v.note = None;
            }
        }
        self.clock = self.clock.wrapping_add(done as u32);
        self.renders = self.renders.wrapping_add(1);
    }

    fn set_param_value(&mut self, index: usize, whole: i32) {
        match PARAMS[index] {
            Param::Knob(at, max) => {
                self.doubles[at] = f64::from(whole) / f64::from(max);
            }
            Param::Button(at) => self.ints[at] = i32::from(whole != 0),
            Param::Shape(at) => self.doubles[at] = SHAPES[whole.clamp(0, 4) as usize],
        }
        // The DLL moves the sounding voices piece by piece
        // (`0x180001000`); this applies the patch to new notes and leaves
        // the sounding ones as they are.
        self.patch_from_values();
    }

    /// The patch again from the stored values, for notes to come.
    fn patch_from_values(&mut self) {
        let (doubles, ints) = (self.doubles, self.ints);
        for (i, v) in ints.into_iter().enumerate() {
            self.set_int(i, v);
        }
        for (i, v) in doubles.into_iter().enumerate() {
            self.set_double(i, v);
        }
        for i in 0..3 {
            self.build_wave(i);
        }
        for slot in 0..VOICES {
            if self.voices[slot].note.is_none() {
                self.build_voice(slot);
            }
        }
    }
}

/// What a shape's stored f64 makes: the selector's value for the
/// oscillator (`0xc034`) or the LFO (`0xc0d0`), none for a value it
/// leaves alone.
fn shape_of(d: f64, oscillator: bool) -> Option<i32> {
    if d == 0.0 {
        Some(2)
    } else if d == 0.1 {
        Some(1)
    } else if d == 0.2 {
        Some(0)
    } else if d == 0.3 && oscillator {
        Some(4)
    } else if d == 0.4 {
        Some(3)
    } else {
        None
    }
}

/// The host's `ComputeLRVol` with the old format's integer pan
/// (`docs/plugins/host-voice.md`): the circular law over `pan / 64`.
fn lr(pan: i32, volume: f32) -> [f32; 2] {
    let (l, r) = crate::sampler::circular(pan as f32 / 64.0, volume);
    [l, r]
}

/// The doubles, the ints and the version of a state, with the engine's
/// fixes for older versions (`0x18000a610`).
fn parse(bytes: &[u8]) -> Result<([f64; DOUBLES], [i32; INTS]), StateError> {
    if bytes.len() != STATE_LEN {
        return Err(StateError::Length {
            expected: STATE_LEN,
            found: bytes.len(),
        });
    }
    let mut d = [0.0f64; DOUBLES];
    for (i, v) in d.iter_mut().enumerate() {
        *v = f64::from_le_bytes(bytes[8 * i..8 * i + 8].try_into().unwrap());
    }
    let mut n = [0i32; INTS];
    for (i, v) in n.iter_mut().enumerate() {
        let at = INTS_AT + 4 * i;
        *v = i32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    }
    let version = i32::from_le_bytes(bytes[VERSION_AT..VERSION_AT + 4].try_into().unwrap());
    if version > VERSION {
        return Err(StateError::Version { found: version });
    }
    if version == 0 {
        d[39] *= 0.75;
    }
    if version < 5 {
        for osc in [1, 8, 15] {
            let mut s = trunc64(d[osc] * 24.0 - 12.0);
            if d[osc + 5] == 0.3 {
                s += 12;
            }
            d[osc] = f64::from(s + 24) / 48.0;
        }
    }
    if version < 13 {
        for osc in [1, 8, 15] {
            d[osc] = f64::from(trunc64(d[osc] * 48.0 - 24.0) + 24) / 48.0;
        }
    }
    Ok((d, n))
}

impl Generator for SimSynth {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let (doubles, ints) = parse(bytes)?;
        self.load(doubles, ints);
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        if hz > 0.0 && hz != self.rate {
            self.rate = hz;
            self.tables.set_rate(hz * 2.0);
            self.chorus.set(hz);
            for slot in 0..VOICES {
                self.voices[slot].note = None;
                self.build_voice(slot);
            }
        }
    }

    fn note_on(&mut self, note: NoteOn) {
        if note.slide {
            return;
        }
        self.trigger(note);
    }

    fn note_off(&mut self, voice: u32) {
        let t = self.renders;
        for v in &mut self.voices {
            if v.note == Some(voice) {
                v.released = t;
                v.amp.release();
                v.flt.release();
            }
        }
    }

    fn all_notes_off(&mut self) {
        let t = self.renders;
        for v in &mut self.voices {
            if v.note.is_some() {
                v.released = t;
                v.amp.release();
                v.flt.release();
            }
        }
    }

    /// Every block reads `FinalLevels.Pitch` again (`0x18000b046`).
    fn set_note_pitch(&mut self, voice: u32, pitch: f32) {
        for v in &mut self.voices {
            if v.note == Some(voice) {
                v.pitch = pitch.round() as i32;
            }
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.chunks_mut(BLOCK).zip(right.chunks_mut(BLOCK)) {
            self.render(l, r);
        }
    }

    fn param_count(&self) -> usize {
        PARAMS.len()
    }

    /// `ProcessParam`'s read (`0x18000ac40`).
    fn param(&self, index: usize) -> f32 {
        match PARAMS.get(index) {
            Some(Param::Knob(at, max)) => {
                (f64::from(*max) * self.doubles[*at]).floor() as i32 as f32
            }
            Some(Param::Button(at)) => self.ints[*at] as f32,
            Some(Param::Shape(at)) => {
                let d = self.doubles[*at];
                let pos = SHAPES.iter().position(|s| *s == d).unwrap_or(1);
                pos as f32
            }
            None => 0.0,
        }
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let Some(param) = PARAMS.get(index) else {
            return;
        };
        let max = match param {
            Param::Knob(_, max) => *max,
            Param::Button(_) => 1,
            Param::Shape(_) => 4,
        };
        let whole = (value.round() as i32).clamp(0, max);
        self.set_param_value(index, whole);
    }

    fn reset(&mut self) {
        for v in &mut self.voices {
            v.note = None;
            v.amp.stage = 4;
            v.flt.stage = 4;
        }
        self.chorus.line.fill(0.0);
        self.chorus.tail = 0;
    }

    fn tail(&self) -> usize {
        // The release, the level's smoothing to nothing and the chorus.
        let release = (self.patch.amp_times[3].max(2) as f32 * self.rate / 1000.0) as usize;
        release + 1000 + self.chorus.span.max(0) as usize
    }

    fn fidelity(&self) -> Fidelity {
        Fidelity::Exact
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
            ..NoteOn::default()
        }
    }

    /// Key 60 played for `frames` from the start and let go of at
    /// `release`, in blocks of 512 as the emulator harness renders.
    fn render(synth: &mut SimSynth, key: i16, frames: usize, release: usize) -> [Vec<f32>; 2] {
        let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
        synth.note_on(note(1, key));
        let mut at = 0;
        while at < frames {
            let mut end = (at + 512).min(frames);
            if at < release && release < end {
                end = release;
            }
            if at == release {
                synth.note_off(1);
            }
            synth.process(&mut left[at..end], &mut right[at..end]);
            at = end;
        }
        [left, right]
    }

    #[test]
    fn the_tables_are_the_dll_s() {
        let t = Tables::new(88_200.0);
        // `emu`: the step for 261.68 Hz, the last one, and the noise.
        assert_eq!(t.step[7200], 12_742_497);
        assert_eq!(t.step[29_999], 2_501_902_336);
        assert_eq!(Tables::map(74), 42);
        assert_eq!(Tables::map(30_027), 29_995);
        assert_eq!(Tables::map(40_000), PITCH_TOP);
        // The first values of `rand()` after `srand(666)`.
        assert_eq!(t.noise[0], 2_213.0 / 32_767.0);
        assert!(t.noise.iter().all(|v| (0.0..=1.0).contains(v)));
        // The saw falls from 1 to -1, the triangle peaks at 1024.
        assert_eq!(t.saw[0], 1.0);
        assert!(t.saw[4095] < -0.999);
        assert_eq!(t.tri[1024], 1.0);
        assert_eq!(t.tri[3072], -1.0);
    }

    #[test]
    fn the_default_is_a_saw_through_the_filter_with_chorus() {
        let mut synth = SimSynth::default();
        synth.set_sample_rate(44_100.0);
        let [left, right] = render(&mut synth, 60, 8192, 4096);
        assert!(left.iter().chain(&right).all(|v| v.is_finite()));
        let peak = right.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((0.01..1.0).contains(&peak), "{peak}");
        // The chorus delays the left side by 20 ms: the right side sounds
        // first.
        let first = |s: &[f32]| s.iter().position(|v| v.abs() > 1e-6).unwrap();
        assert!(first(&left) >= 882 && first(&right) < 10);
    }

    /// The oscillator steps twice a sample through its table: key 60
    /// plays 261.68 Hz (4.0887 Hz times 64).
    #[test]
    fn key_60_plays_middle_c() {
        let mut synth = SimSynth::default();
        let mut state = vec![0u8; STATE_LEN];
        let mut d = DEFAULT_DOUBLES;
        d[6] = 0.3; // sine
        d[0] = 0.0;
        d[26] = 1.0; // the filter open
        d[37] = 1.0; // full sustain
        let mut n = DEFAULT_INTS;
        n[14] = 0; // no filter
        n[18] = 0; // no chorus
        n[3] = 0; // not warm
        for (i, v) in d.iter().enumerate() {
            state[8 * i..8 * i + 8].copy_from_slice(&v.to_le_bytes());
        }
        for (i, v) in n.iter().enumerate() {
            state[INTS_AT + 4 * i..INTS_AT + 4 * i + 4].copy_from_slice(&v.to_le_bytes());
        }
        state[VERSION_AT..].copy_from_slice(&VERSION.to_le_bytes());
        synth.set_state(&state).unwrap();
        let [left, _] = render(&mut synth, 60, 44_100, usize::MAX);
        let tail = &left[22_050..];
        let crossings = tail
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count();
        assert!((130..=131).contains(&crossings), "{crossings}");
    }

    #[test]
    fn follows_note_pitch() {
        testing::follows_note_pitch(|| Box::new(SimSynth::default()), 44_100.0);
    }

    /// Bit for bit what `SimSynth_Eng_x64.dll` plays for the render test's
    /// state under the harness in `.scratch/notes/simsynth-emu` (created,
    /// given 44.1 kHz, the state loaded; key 60 for 8000 frames in blocks
    /// of 512, let go at 6000): every sample's bits folded together, and
    /// three of them.
    #[test]
    fn plays_the_render_test_as_the_engine_dll_does() {
        let Some(root) = testing::corpus() else {
            return;
        };
        let saved = testing::projects(&root.join("System/Render tests"));
        let ours: Vec<_> = saved.iter().filter(|s| s.plugin == NAME).collect();
        assert_eq!(ours.len(), 1);
        let mut synth = SimSynth::default();
        synth.set_sample_rate(44_100.0);
        synth.set_state(&ours[0].state).unwrap();
        let [left, right] = render(&mut synth, 60, 8000, 6000);
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for (l, r) in left.iter().zip(&right) {
            for bits in [l.to_bits(), r.to_bits()] {
                hash = (hash ^ u64::from(bits)).wrapping_mul(0x0100_0000_01b3);
            }
        }
        let at = |n: usize| [left[n].to_bits(), right[n].to_bits()];
        assert_eq!(at(1000), [0x3d11_61e3, 0x3d11_61e3]);
        assert_eq!(at(5000), [0x3da7_d46f, 0x3da7_d46f]);
        assert_eq!(at(7000), [0x3d25_9b0f, 0x3d25_9b0f]);
        assert_eq!(hash, 0xcb5a_b4d6_1b37_ad4d);
    }

    /// Every SimSynth state FL ships loads and plays a note that is heard
    /// and stays finite.
    #[test]
    fn every_shipped_state_loads_and_plays() {
        let Some(root) = testing::corpus() else {
            return;
        };
        let mut saved = testing::every_preset(&root, NAME);
        let ours = |saved: &testing::Saved| saved.plugin.eq_ignore_ascii_case(NAME);
        saved.extend(testing::projects(&root).into_iter().filter(ours));
        assert!(saved.len() >= 77, "{}", saved.len());
        for saved in saved {
            let mut synth = SimSynth::default();
            if let Err(error) = synth.set_state(&saved.state) {
                panic!("{}: {error}", saved.file);
            }
            let [left, right] = render(&mut synth, 60, 16_384, 8192);
            let all = || left.iter().chain(&right);
            assert!(all().all(|v| v.is_finite()), "{}", saved.file);
            let peak = all().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(peak < 8.0, "{peak} in {}", saved.file);
        }
    }

    /// Events as the emulator harness takes them, `on:<key>:<frame>` and
    /// `off:<key>:<frame>`, each note's voice its key; blocks of 512 that
    /// start again at every event.
    fn render_events(synth: &mut SimSynth, events: &str, frames: usize) -> [Vec<f32>; 2] {
        let mut list: Vec<(usize, bool, i16)> = events
            .split_whitespace()
            .map(|e| {
                let p: Vec<&str> = e.split(':').collect();
                (p[2].parse().unwrap(), p[0] == "on", p[1].parse().unwrap())
            })
            .collect();
        list.sort_by_key(|(at, on, _)| (*at, *on));
        let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
        let mut done = 0;
        let mut next = 0;
        while done < frames {
            while next < list.len() && list[next].0 <= done {
                let (_, on, key) = list[next];
                if on {
                    synth.note_on(note(key as u32, key));
                } else {
                    synth.note_off(key as u32);
                }
                next += 1;
            }
            let mut n = 512.min(frames - done);
            if next < list.len() {
                n = n.min(list[next].0 - done);
            }
            synth.process(&mut left[done..done + n], &mut right[done..done + n]);
            done += n;
        }
        [left, right]
    }

    fn env<T: std::str::FromStr>(name: &str, default: T) -> T {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }

    /// Writes what the port plays for the harness in
    /// `.scratch/notes/simsynth-emu` to compare with the DLL.
    #[test]
    fn dump_for_dll() {
        let Ok(path) = std::env::var("SIMSYNTH_STATE") else {
            return;
        };
        let frames: usize = env("SIMSYNTH_FRAMES", 8000);
        let key: i16 = env("SIMSYNTH_KEY", 60);
        let release: usize = env("SIMSYNTH_RELEASE", usize::MAX);
        let mut synth = SimSynth::default();
        synth.set_sample_rate(44_100.0);
        synth.set_state(&std::fs::read(path).unwrap()).unwrap();
        let [left, right] = match std::env::var("SIMSYNTH_EVENTS") {
            Ok(events) => render_events(&mut synth, &events, frames),
            Err(_) => render(&mut synth, key, frames, release),
        };
        let out: String = left
            .iter()
            .zip(&right)
            .map(|(l, r)| format!("{l:?} {r:?}\n"))
            .collect();
        std::fs::write(std::env::var("SIMSYNTH_OUT").unwrap(), out).unwrap();
    }
}
