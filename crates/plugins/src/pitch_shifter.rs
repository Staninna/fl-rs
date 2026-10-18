//! Pitch Shifter: a granular shifter and a PSOLA shifter with formant
//! control, each with a delay and a feedback loop around it, as
//! `docs/plugins/pitch-shifter.md` has it.
//!
//! The plugin is a Delphi shell around two engines in FL's
//! `Shared/ildsp_x64.dll`: `GeneralPitchShifter_CreateInstance` and
//! `PSOLA_CreateInstance`. The shell keeps the state, turns its 13 controls
//! into floats for the engines and plays one of them, the one its state's
//! effect selector picks. This file follows the same split: [`PitchShifter`]
//! is the shell, [`General`] and [`Psola`] the engines.
//!
//! The state is a version (6 or 7), the 13 controls as i32, the formant
//! correction wheel again, the tempo-sync button, the effect selector (0
//! PSOLA, 1 general), the pitch detector (0 fast, 1 accurate), and in
//! version 7 an i32 and two bytes for the editor's display. FL leaves any
//! other version alone, and so does this.
//!
//! The general engine is a sibling of the shifter inside Luxeverb's shimmer
//! reverb (`luxeverb.rs`), not the same class: its own functions, a 0.2 s
//! grain buffer where the shimmer's is 0.8 s, four delay modes, a tanh clip
//! and a random seed of its own. It is written out here.

use crate::{Effect, Fidelity, StateError, knob_value, state_version};
use std::cmp::Ordering;
use std::f64::consts::PI;

/// The plugin's event 201 name.
pub const NAME: &str = "Pitch Shifter";

/// How many controls the shell has.
const PARAMS: usize = 13;

/// The range of each control, from the editor's form.
const RANGES: [(i32, i32); PARAMS] = [
    (-1200, 1200), // 0 pitch, cents
    (-1200, 1200), // 1 formant, cents
    (0, 2),        // 2 analysis channel: left, right, both
    (0, 2400),     // 3 mix
    (2000, 15000), // 4 duration, ms * 100
    (100, 500),    // 5 density * 100
    (0, 100),      // 6 jitter
    (0, 100),      // 7 rand
    (1, 768),      // 8 delay
    (0, 100),      // 9 feedback
    (0, 3),        // 10 delay mode
    (0, 65536),    // 11 formant correction
    (0, 3),        // 12 pitch mode: relative, C4, C5, C6
];

/// A plugin FL has just made: the form's wheels, the PSOLA engine and the
/// fast detector, as the one untouched state in the corpus has them.
const DEFAULTS: [i32; PARAMS] = [0, 0, 0, 2400, 8500, 300, 30, 20, 768, 0, 0, 0, 0];

/// The versions the loader reads and how long each state is.
const V6_LEN: usize = 69;
const V7_LEN: usize = 75;

/// The window lengths at 44.1 kHz the shell gives the PSOLA detector for
/// fast and accurate (`DAT_00786568`).
const DETECTOR: [f32; 2] = [1024.0, 2048.0];

/// MSVC's `rand` step on a seed: the next 15-bit number.
fn rand(seed: &mut u32) -> u32 {
    *seed = seed.wrapping_mul(0x343fd).wrapping_add(0x269ec3);
    (*seed >> 16) & 0x7fff
}

/// The seed the library's static starts at (`DAT_1803f67d0`).
const SEED: u32 = 0x3920a;

/// The four-point Lagrange read both engines' grains use. `older` is one
/// sample before `x0` in the buffer, `x1` and `x2` after it.
fn lagrange(older: f32, x0: f32, x1: f32, x2: f32, frac: f32) -> f32 {
    const THIRD: f32 = f32::from_bits(0x3eaa_aa9f);
    const SIXTH: f32 = f32::from_bits(0x3e2a_aac1);
    let f2 = frac * frac;
    let x2 = x2 * SIXTH;
    (((x1 - (older * THIRD + x0 * 0.5)) - x2) + ((x2 - older * SIXTH) + (x0 - x1) * 0.5) * f2)
        * frac
        + x0
        + ((x1 * 0.5 + older * 0.5) - x0) * f2
}

/// Read a ring of `n` samples at a place inside it, as both engines do.
fn read_ring(buf: &[f32], place: f32) -> f32 {
    let n = buf.len() as i32;
    let i = place as i32;
    let frac = place - i as f32;
    let older = if i > 0 { i - 1 } else { i - 1 + n };
    let i1 = if i + 1 >= n { i + 1 - n } else { i + 1 };
    let i2 = if i + 2 >= n { i + 2 - n } else { i + 2 };
    lagrange(
        buf[older as usize],
        buf[i as usize],
        buf[i1 as usize],
        buf[i2 as usize],
        frac,
    )
}

/// `sin(i pi / (len - 1))^2` for `i` under `len`, in doubles, into `out`.
fn sine_window(out: &mut [f32], len: usize) {
    let step = PI / (len as f64 - 1.0);
    for (i, w) in out.iter_mut().take(len).enumerate() {
        let s = (step * i as f64).sin();
        *w = (s * s) as f32;
    }
}

/// The weight of a grain with `left` of `len` samples to go, read from a
/// window table with a straight line between its points.
fn grain_weight(window: &[f32], left: i32, len: i32) -> f32 {
    let size = window.len() as i32;
    let at = (1.0 - left as f32 / len as f32) * (size as f32 - 1.0);
    let wi = at as i32;
    let wf = at - wi as f32;
    let mut weight = (1.0 - wf) * window[wi as usize];
    if wi != size - 1 {
        weight += wf * window[wi as usize + 1];
    }
    weight
}

/// 1001 points of `tanhf` from -5 to 5: both engines' clip.
fn tanh_table() -> Vec<f32> {
    (0..1001)
        .map(|i| (i as f32 * (10.0 / 1000.0) - 5.0).tanh())
        .collect()
}

/// The table read at a place from 0 to 999, with a straight line to the
/// next point, times `gain`.
fn tanh_read(table: &[f32], place: f32, gain: f32) -> f32 {
    let place = 999.0f32.min(place).max(0.0);
    let i = place as i32;
    let f = place - i as f32;
    ((1.0 - f) * table[i as usize] + f * table[i as usize + 1]) * gain
}

/// The three biquads on each engine's feedback path, per rate, as both
/// engines' prepare functions set them: `b0 b1 b2 a1 a2` three times.
fn feedback_coefficients(rate: f32) -> [f32; 15] {
    let bits: [u32; 15] = match rate.round() as i32 {
        44100 => [
            0x3f74_76de,
            0xbff4_4ac0,
            0x3f74_1eae,
            0xbffa_216d,
            0x3f74_76de,
            0x3f80_0000,
            0xc000_08d5,
            0x3f80_11b1,
            0xbffd_a9aa,
            0x3f7b_8813,
            0x3f80_0000,
            0xbf80_1c88,
            0,
            0xbf78_cea6,
            0,
        ],
        48000 => [
            0x3f7a_a288,
            0xbffa_8fac,
            0x3f7a_7cdf,
            0xbffd_4bb6,
            0x3f7a_a289,
            0x3f80_0000,
            0xc000_193e,
            0x3f80_3284,
            0xbffe_f264,
            0x3f7d_eff6,
            0x3f80_0000,
            0xbf7f_c18e,
            0,
            0xbf7c_abb2,
            0,
        ],
        88200 => [
            0x3f7d_10ef,
            0xbffc_fde4,
            0x3f7c_eae8,
            0xbffe_86d5,
            0x3f7d_10f8,
            0x3f80_0000,
            0xc000_193e,
            0x3f80_3284,
            0xbfff_6e65,
            0x3f7e_e01b,
            0x3f80_0000,
            0xbf7f_c18e,
            0,
            0xbf7e_2ebf,
            0,
        ],
        96000 => [
            0x3f7d_4da2,
            0xbffd_3a93,
            0x3f7d_2793,
            0xbffe_a56b,
            0x3f7d_4da2,
            0x3f80_0000,
            0xc000_193e,
            0x3f80_3284,
            0xbfff_7a52,
            0x3f7e_f771,
            0x3f80_0000,
            0xbf7f_c18e,
            0,
            0xbf7e_5478,
            0,
        ],
        192000 => [
            0x3f7e_a5e7,
            0xbffe_92be,
            0x3f7e_7fa4,
            0xbfff_504a,
            0x3f7e_a148,
            0x3f80_0000,
            0xc000_193e,
            0x3f80_3284,
            0xbfff_bd17,
            0x3f7f_7ae3,
            0x3f80_0000,
            0xbf7f_c18e,
            0,
            0xbf7f_2f37,
            0,
        ],
        _ => [
            0x3f7a_2a76,
            0xbffa_17a3,
            0x3f7a_04df,
            0xbffd_0ea7,
            0x3f7a_2a75,
            0x3f80_0000,
            0xc000_193e,
            0x3f80_3284,
            0xbffe_da1d,
            0x3f7d_c179,
            0x3f80_0000,
            0xbf7f_c18e,
            0,
            0xbf7c_60dc,
            0,
        ],
    };
    bits.map(f32::from_bits)
}

/// Three biquads in a row, transposed direct form II, on one side.
#[derive(Debug, Clone, Copy, Default)]
struct Cascade {
    s: [[f32; 2]; 3],
}

impl Cascade {
    fn run(&mut self, c: &[f32; 15], x: f32) -> f32 {
        let mut x = x;
        for (k, s) in self.s.iter_mut().enumerate() {
            let c = &c[5 * k..5 * k + 5];
            let y = x * c[0] + s[0];
            s[0] = (x * c[1] + s[1]) - y * c[3];
            s[1] = x * c[2] - y * c[4];
            x = y;
        }
        x
    }
}

/// The delay both engines have ahead of or behind the shifter: a stereo
/// line of `rate * 4.01` samples whose length glides to its target and is
/// read between two samples.
#[derive(Debug, Clone, Default)]
struct Line {
    buf: [Vec<f32>; 2],
    write: i32,
    cur: f32,
    coef: f32,
    target: f32,
    /// The longest and the shortest delay, in samples.
    max: f32,
    min: f32,
}

impl Line {
    fn prepare(&mut self, rate: f32) {
        let size = (f64::from(rate) * 4.01) as i32;
        self.buf = [
            vec![0.0; size.max(2) as usize],
            vec![0.0; size.max(2) as usize],
        ];
        self.write = 0;
        self.cur = rate * 0.1;
        self.target = rate * 0.1;
        self.coef = 4.41 / rate;
        self.max = (size - 1) as f32;
        self.min = rate * 0.001;
    }

    /// Glide to `target` and read.
    fn read(&mut self, target: f32) -> [f32; 2] {
        self.cur = (target - self.cur) * self.coef + self.cur;
        let size = self.buf[0].len() as i32;
        let whole = self.cur as i32;
        let mut i = self.write - whole;
        while i < 0 {
            i += size;
        }
        let mut j = i - 1;
        while j < 0 {
            j += size;
        }
        let frac = self.cur - whole as f32;
        [0, 1].map(|s| (1.0 - frac) * self.buf[s][i as usize] + frac * self.buf[s][j as usize])
    }

    fn write(&mut self, v: [f32; 2]) {
        let w = self.write as usize;
        self.buf[0][w] = v[0];
        self.buf[1][w] = v[1];
        self.write += 1;
        if self.buf[0].len() as i32 - 1 < self.write {
            self.write = 0;
        }
    }

    fn clear(&mut self) {
        for side in &mut self.buf {
            side.fill(0.0);
        }
    }
}

/// One grain of either engine.
#[derive(Debug, Clone, Copy, Default)]
struct Grain {
    on: bool,
    len: i32,
    left: i32,
    place: f32,
    step: f32,
}

/// The first grain that is free, as both engines look for one: none when
/// all 20 play.
fn free_grain(grains: &[Grain; 20]) -> Option<usize> {
    grains.iter().position(|g| !g.on)
}

/// `GeneralPitchShifterImpl` (`FUN_180161fc0` prepare, `FUN_180162760`
/// set, `FUN_180162f90` process): grains read from a buffer at the pitch
/// ratio under a squared-sine window, with a delay and a feedback loop.
#[derive(Debug, Clone, Default)]
struct General {
    rate: f32,
    /// The feedback of the last frame: `+0x0`, `+0x4`.
    fb: [f32; 2],
    /// The grain buffers (`+0x18`, `+0x20`) and where the next sample goes.
    buf: [Vec<f32>; 2],
    write: i32,
    ratio: f32,
    density: f32,
    time_jitter: f32,
    place_jitter: f32,
    mix: f32,
    grain_len: i32,
    countdown: i32,
    jitter_scale: f32,
    grains: [Grain; 20],
    window: Vec<f32>,
    prev: [f32; 2],
    line: Line,
    feedback: f32,
    tempo: f32,
    mode: i32,
    table: Vec<f32>,
    clip_gain: f32,
    clip_scale: f32,
    clip: bool,
    coefs: [f32; 15],
    filters: [Cascade; 2],
    seed: u32,
}

impl General {
    fn prepare(&mut self, rate: f32) {
        self.rate = rate;
        let n = (rate * 0.2) as i32;
        self.buf = [vec![0.0; n.max(4) as usize], vec![0.0; n.max(4) as usize]];
        self.write = 0;
        self.ratio = 1.0;
        self.density = 2.5;
        self.time_jitter = 0.0;
        self.place_jitter = 0.0;
        self.mix = 0.5;
        self.jitter_scale = rate * 0.045_351_47;
        self.grain_len = (rate * -421.7265) as i32;
        self.countdown = f64::from(self.grain_len as f32 * 0.4).round() as i32;
        self.grains = [Grain::default(); 20];
        self.line.prepare(rate);
        self.feedback = 0.5;
        self.tempo = 120.0;
        self.mode = 1;
        self.table = tanh_table();
        self.clip_gain = 1.0;
        self.clip_scale = 0.2;
        self.clip = true;
        self.coefs = feedback_coefficients(rate);
        self.filters = [Cascade::default(); 2];
        self.window = vec![0.0; 1024];
        sine_window(&mut self.window, 1024);
        self.fb = [0.0; 2];
        self.prev = [0.0; 2];
        if self.seed == 0 {
            self.seed = SEED;
        }
    }

    /// `FUN_1801aa3a0`: empty the buffers and the feedback.
    fn clear(&mut self) {
        for side in &mut self.buf {
            side.fill(0.0);
        }
        self.line.clear();
        self.filters = [Cascade::default(); 2];
        self.fb = [0.0; 2];
    }

    /// `FUN_180162760` for the cases the shell sends.
    fn set(&mut self, case: u32, v: f32) {
        let rate = self.rate;
        match case {
            0 => self.ratio = 2f32.powf(v * 0.083_333_336),
            1 => self.grain_len = (v * 0.001 * rate) as i32,
            2 => self.density = v,
            3 => self.time_jitter = v,
            4 => self.place_jitter = v,
            5 => self.mix = v,
            6 => {
                let t = v * rate * 0.001;
                self.line.target = self.line.min.max(t.min(self.line.max));
            }
            7 => self.feedback = v,
            8 => self.mode = f64::from(v).round() as i32,
            9 => self.tempo = v,
            10 => {
                let t = (v * rate * 15.0) / self.tempo;
                self.line.target = t.min(self.line.max);
            }
            _ => {}
        }
    }

    fn clip(&self, x: f32) -> f32 {
        tanh_read(
            &self.table,
            x * self.clip_scale * 501.0 + 500.0,
            self.clip_gain,
        )
    }

    /// One frame of `FUN_180162f90`, two channels.
    fn run(&mut self, x: [f32; 2]) -> [f32; 2] {
        let mode = self.mode;
        let mut delayed = [0.0f32; 2];
        let mut input = x;
        if mode != 1 {
            delayed = self.line.read(self.line.target);
            if mode == 2 {
                input = [x[0] + self.fb[0], x[1] + self.fb[1]];
            } else if mode == 3 {
                input = delayed;
            }
        }

        // The two-tap filter into the grain buffer.
        let c = f64::from(2.0 - self.ratio).min(1.0) as f32;
        let w = self.write as usize;
        for (s, &x) in input.iter().enumerate() {
            self.buf[s][w] = ((c + 1.0) * x + (1.0 - c) * self.prev[s]) * 0.5;
            self.prev[s] = x;
        }
        let n = self.buf[0].len() as i32;
        self.write += 1;
        if n <= self.write {
            self.write = 0;
        }

        // A new grain when the countdown runs out.
        self.countdown -= 1;
        if self.countdown < 1 {
            let r = rand(&mut self.seed) as f32;
            let period = (self.grain_len as f32 / self.density + 0.5) as i32;
            self.countdown =
                period - (r * -3.051_851e-5 * self.time_jitter * self.jitter_scale) as i32;
            if let Some(slot) = free_grain(&self.grains) {
                let back = if self.ratio <= 1.0 {
                    0
                } else {
                    ((self.ratio - 1.0) * self.grain_len as f32) as i32
                };
                let r = rand(&mut self.seed) as f32;
                let mut place = ((self.write - back) - 5) as f32
                    - r * 3.051_851e-5 * self.place_jitter * self.jitter_scale;
                while place < 0.0 {
                    place += n as f32;
                }
                while place >= n as f32 {
                    place -= n as f32;
                }
                self.grains[slot] = Grain {
                    on: true,
                    len: self.grain_len,
                    left: self.grain_len,
                    place,
                    step: self.ratio,
                };
            }
        }

        // The grains.
        let mut sum = [0.0f32; 2];
        for g in self.grains.iter_mut().filter(|g| g.on) {
            let weight = grain_weight(&self.window, g.left, g.len);
            for (s, total) in sum.iter_mut().enumerate() {
                *total += read_ring(&self.buf[s], g.place) * weight;
            }
            g.place += g.step;
            if n as f32 <= g.place {
                g.place -= n as f32;
            }
            g.left -= 1;
            if g.left < 1 {
                g.on = false;
            }
        }
        if self.clip {
            sum = sum.map(|v| self.clip(v));
        }

        // The feedback, through the three biquads.
        let source = match mode {
            1 => None,
            2 | 4 => Some(delayed),
            3 => Some(sum),
            _ => Some([0.0; 2]),
        };
        if let Some(source) = source {
            for (s, filter) in self.filters.iter_mut().enumerate() {
                self.fb[s] = filter.run(&self.coefs, source[s]) * self.feedback;
            }
            if !self.clip {
                self.fb = self.fb.map(|v| self.clip(v));
            }
            let into = match mode {
                2 => sum,
                3 => [x[0] + self.fb[0], x[1] + self.fb[1]],
                4 => [sum[0] + self.fb[0], sum[1] + self.fb[1]],
                _ => [0.0; 2],
            };
            self.line.write(into);
        }

        if mode == 4 {
            sum = [sum[0] + self.fb[0], sum[1] + self.fb[1]];
        }
        [0, 1].map(|s| self.mix * sum[s] + (1.0 - self.mix) * x[s])
    }
}

/// A radix-2 complex FFT of a fixed size with its twiddles made ahead, so
/// that a transform asks for no memory: the forward transform divided by
/// the size, as IPP's `ippsFFTFwd_CToC_64fc` with `IPP_FFT_DIV_FWD_BY_N`.
#[derive(Debug, Clone, Default)]
struct Fft {
    re: Vec<f64>,
    im: Vec<f64>,
    cos: Vec<f64>,
    sin: Vec<f64>,
}

impl Fft {
    fn new(size: usize) -> Self {
        let turn = |k: usize| 2.0 * PI * k as f64 / size as f64;
        Fft {
            re: vec![0.0; size],
            im: vec![0.0; size],
            cos: (0..size / 2).map(|k| turn(k).cos()).collect(),
            sin: (0..size / 2).map(|k| -turn(k).sin()).collect(),
        }
    }

    fn forward(&mut self) {
        let len = self.re.len();
        let mut j = 0;
        for i in 1..len {
            let mut bit = len >> 1;
            while j & bit != 0 {
                j ^= bit;
                bit >>= 1;
            }
            j |= bit;
            if i < j {
                self.re.swap(i, j);
                self.im.swap(i, j);
            }
        }
        let mut span = 2;
        while span <= len {
            let stride = len / span;
            for start in (0..len).step_by(span) {
                for k in 0..span / 2 {
                    let (w_re, w_im) = (self.cos[k * stride], self.sin[k * stride]);
                    let (a, b) = (start + k, start + k + span / 2);
                    let t_re = self.re[b] * w_re - self.im[b] * w_im;
                    let t_im = self.re[b] * w_im + self.im[b] * w_re;
                    (self.re[b], self.im[b]) = (self.re[a] - t_re, self.im[a] - t_im);
                    (self.re[a], self.im[a]) = (self.re[a] + t_re, self.im[a] + t_im);
                }
            }
            span *= 2;
        }
        let scale = 1.0 / len as f64;
        for v in self.re.iter_mut().chain(self.im.iter_mut()) {
            *v *= scale;
        }
    }
}

/// The detector's frame buffer: `0x8b50` bytes whatever the rate.
const FRAME_CAP: usize = 8916;

/// How many peaks the detector keeps.
const PEAKS: usize = 100;

/// `PSOLAImpl` (`FUN_18017f700` prepare, `FUN_1801804b0` set,
/// `FUN_180180830` process): a zero-crossing gate splits the input into a
/// voiced part and the rest, a harmonic-sum detector over an FFT finds the
/// voiced part's period, and two-period grains of it are laid down at the
/// period the pitch asks for, read at the formant ratio. The rest passes
/// under them, and a delay and a feedback loop go round as in the general
/// engine.
#[derive(Debug, Clone, Default)]
struct Psola {
    rate: f32,
    /// The detector's high pass, `b0 b1 b2 a1 a2`, and its state.
    hp: [f32; 5],
    hp_s: [f32; 2],
    /// 1 left, 2 right, 3 both.
    channel: i32,
    zc_coef: f32,
    ramp: f32,
    voiced: f32,
    sign: f32,
    crossings: f32,
    /// The voiced part, delayed, that the grains read.
    src: [Vec<f32>; 2],
    src_write: i32,
    /// The latency lines: voiced, unvoiced and dry, each side.
    lat: [Vec<f32>; 6],
    lat_len: i32,
    lat_pos: i32,
    mark_count: i32,
    mark: i32,
    period: i32,
    synth_count: i32,
    pitch_mode: i32,
    ratio: f32,
    formant_ratio: f32,
    grains: [Grain; 20],
    window: Vec<f32>,
    // The detector.
    fft: Fft,
    spec: Vec<f32>,
    frame: Vec<f32>,
    frame_window: Vec<f32>,
    frame_len: i32,
    frame_pos: i32,
    f0: f32,
    f0s: f32,
    smooth: f32,
    keep: f32,
    lo_bin: i32,
    hi_bin: i32,
    peak_place: Vec<f32>,
    peak_height: Vec<f32>,
    // The rest.
    mix: f32,
    line: Line,
    feedback: f32,
    tempo: f32,
    mode: i32,
    table: Vec<f32>,
    clip_gain: f32,
    clip: bool,
    coefs: [f32; 15],
    filters: [Cascade; 2],
    delayed: [f32; 2],
    fb: [f32; 2],
}

impl Psola {
    fn prepare(&mut self, rate: f32) {
        self.rate = rate;
        let hp: [u32; 5] = match rate.round() as i32 {
            44100 => [
                0x3f7d_6f11,
                0xbffd_6f11,
                0x3f7d_6f11,
                0xbffd_6bc6,
                0x3f7a_e4b9,
            ],
            48000 => [
                0x3f7e_d166,
                0xbffe_d166,
                0x3f7e_d166,
                0xbffe_d0b4,
                0x3f7d_a432,
            ],
            88200 => [
                0x3f7f_5b25,
                0xbfff_5b25,
                0x3f7f_5b25,
                0xbfff_5af0,
                0x3f7e_b6b5,
            ],
            96000 => [
                0x3f7f_6886,
                0xbfff_6886,
                0x3f7f_6886,
                0xbfff_685a,
                0x3f7e_d166,
            ],
            192000 => [
                0x3f7f_b438,
                0xbfff_b438,
                0x3f7f_b438,
                0xbfff_b42d,
                0x3f7f_6886,
            ],
            _ => [
                0x3f7e_b6b5,
                0xbffe_b6b5,
                0x3f7e_b6b5,
                0xbffe_b5e1,
                0x3f7d_6f11,
            ],
        };
        self.hp = hp.map(f32::from_bits);
        self.hp_s = [0.0; 2];
        self.channel = 1;
        self.zc_coef = 42_115.5 / rate;
        self.ramp = 66.666_67 / rate;
        self.voiced = 1.0;
        self.sign = 0.0;
        self.crossings = 0.0;
        let src = (rate * 0.090_702_94) as i32;
        self.src = [
            vec![0.0; src.max(4) as usize],
            vec![0.0; src.max(4) as usize],
        ];
        self.src_write = 0;
        self.frame_len = (rate * 0.046_439_91) as i32;
        let lat_cap = (FRAME_CAP as f32 * 1.2) as i32;
        self.lat = std::array::from_fn(|_| vec![0.0; lat_cap as usize]);
        self.lat_len = (self.frame_len as f32 * 1.2) as i32;
        self.lat_pos = 0;
        let fft_size = if (rate as i32) > 89_999 { 8192 } else { 4096 };
        self.fft = Fft::new(fft_size);
        self.spec = vec![0.0; fft_size];
        self.frame = vec![0.0; FRAME_CAP];
        self.frame_window = vec![0.0; FRAME_CAP];
        self.frame_pos = 0;
        self.f0 = 60.0;
        self.f0s = 60.0;
        self.smooth = rate * 1.133_786_8e-7;
        self.keep = 1.0 - self.smooth;
        let per_hz = 1.0 / rate;
        self.lo_bin = (fft_size as f32 * 60.0 * per_hz) as i32;
        self.hi_bin = ((fft_size as f32 * 1500.0) * per_hz) as i32;
        self.peak_place = vec![0.0; PEAKS];
        self.peak_height = vec![0.0; PEAKS];
        self.mix = 1.0;
        self.line.prepare(rate);
        self.feedback = 0.5;
        self.tempo = 120.0;
        self.mode = 1;
        self.table = tanh_table();
        self.clip_gain = 1.0;
        self.clip = true;
        self.coefs = feedback_coefficients(rate);
        self.filters = [Cascade::default(); 2];
        sine_window(&mut self.frame_window, self.frame_len as usize);
        self.ratio = 1.0;
        self.formant_ratio = 1.0;
        self.synth_count = 600;
        self.mark_count = 500;
        self.mark = 0;
        self.period = 500;
        self.pitch_mode = 1;
        self.grains = [Grain::default(); 20];
        self.window = vec![0.0; 1024];
        sine_window(&mut self.window, 1024);
        self.delayed = [0.0; 2];
        self.fb = [0.0; 2];
    }

    /// `FUN_1801ab6b0`: empty the buffers and the feedback.
    fn clear(&mut self) {
        for side in self.src.iter_mut().chain(self.lat.iter_mut()) {
            side.fill(0.0);
        }
        self.line.clear();
        self.filters = [Cascade::default(); 2];
        self.delayed = [0.0; 2];
        self.fb = [0.0; 2];
    }

    /// The most the latency comes to at this rate: the accurate detector's
    /// window, 1.2 times.
    fn max_latency(&self) -> usize {
        let len = ((DETECTOR[1] * self.rate * 2.267_573_7e-5) as i32).min(FRAME_CAP as i32 - 1);
        (len as f32 * 1.2) as usize
    }

    /// `FUN_1801804b0` for the cases the shell sends.
    fn set(&mut self, case: u32, v: f32) {
        let rate = self.rate;
        match case {
            0 => self.ratio = 2f64.powf(f64::from(v * 0.083_333_336)) as f32,
            1 => self.formant_ratio = 2f64.powf(f64::from(v * 0.083_333_336)) as f32,
            2 => self.channel = f64::from(v).round() as i32,
            3 => self.mix = v,
            4 => {
                let t = v * rate * 0.001;
                self.line.target = self.line.min.max(t.min(self.line.max));
            }
            5 => self.feedback = v,
            6 => self.mode = f64::from(v).round() as i32,
            7 => self.tempo = v,
            8 => {
                let t = (15.0 / self.tempo) * (v * rate);
                self.line.target = 1.0f32.max(t.min(self.line.max));
            }
            9 => {
                let len = (v * rate * 2.267_573_7e-5) as i32;
                self.frame_len = len.min(FRAME_CAP as i32 - 1);
                self.frame.fill(0.0);
                self.frame_pos = 0;
                self.lat_len = (self.frame_len as f32 * 1.2) as i32;
                for line in &mut self.lat {
                    line.fill(0.0);
                }
                self.lat_pos = 0;
                sine_window(&mut self.frame_window, self.frame_len as usize);
            }
            10 => self.pitch_mode = f64::from(v).round() as i32,
            _ => {}
        }
    }

    fn clip(&self, x: f32) -> f32 {
        tanh_read(
            &self.table,
            (100.2 / self.clip_gain) * x + 500.0,
            self.clip_gain,
        )
    }

    /// `FUN_180181590`: the high pass, the zero-crossing rate and the gate
    /// that splits the input into voiced `[left, right]` and the rest.
    fn split(&mut self, input: [f32; 2]) -> ([f32; 2], [f32; 2]) {
        let a = match self.channel {
            1 => input[0],
            2 => input[1],
            _ => (input[1] + input[0]) * 0.5,
        };
        let h = &self.hp;
        let y = a * h[0] + self.hp_s[0];
        self.hp_s[0] = (a * h[1] + self.hp_s[1]) - y * h[3];
        self.hp_s[1] = a * h[2] - y * h[4];
        let before = self.sign;
        self.sign = if y < 0.0 { -1.0 } else { 1.0 };
        self.crossings = (self.sign - before).abs() * 0.05 + self.crossings * self.zc_coef;
        if self.crossings <= 0.6 {
            self.voiced += self.ramp;
            if 1.0 < self.voiced {
                self.voiced = 1.0;
            }
        } else {
            self.voiced -= self.ramp;
            if self.voiced < 0.0 {
                self.voiced = 0.0;
            }
        }
        let v = self.voiced;
        (
            [v * input[0], input[1] * v],
            [(1.0 - v) * input[0], (1.0 - v) * input[1]],
        )
    }

    /// `FUN_180181730`: one sample of the voiced part into the detector's
    /// frame; with the frame full, the period it finds. Then the smoothed
    /// frequency, kept between 60 and 1500 Hz.
    fn detect(&mut self, voiced: [f32; 2]) {
        let a = match self.channel {
            1 => voiced[0],
            2 => voiced[1],
            _ => (voiced[1] + voiced[0]) * 0.5,
        };
        let pos = self.frame_pos as usize;
        self.frame[pos] = a * self.frame_window[pos];
        self.frame_pos += 1;
        if self.frame_len - 1 < self.frame_pos {
            self.frame_pos = 0;
            self.f0 = self.analyse();
        }
        let f = self.f0 * self.smooth + self.keep * self.f0s;
        // FL takes the max first, so a NaN comes out 60, where `clamp` keeps it.
        #[allow(clippy::manual_clamp)]
        let f0s = f.max(60.0).min(1500.0);
        self.f0s = f0s;
    }

    /// The analysis of a full frame: the compressed spectrum, its peaks,
    /// and the peak whose harmonics add up highest, in Hz.
    fn analyse(&mut self) -> f32 {
        let n = self.spec.len();
        let len = self.frame_len as usize;
        for k in 0..n {
            self.fft.re[k] = if k < len {
                f64::from(self.frame[k])
            } else {
                0.0
            };
            self.fft.im[k] = 0.0;
        }
        self.fft.forward();
        let power = f64::from(0.2f32 * 0.5);
        for (k, s) in self.spec.iter_mut().enumerate() {
            let (re, im) = (self.fft.re[k], self.fft.im[k]);
            *s = (re * re + im * im).powf(power) as f32;
        }

        // The peaks, each placed and sized on a parabola.
        let s = &self.spec;
        let mut count = 0;
        for k in self.lo_bin.max(1)..self.hi_bin.min(n as i32 - 1) {
            if count == PEAKS {
                break;
            }
            let k_ = k as usize;
            let (before, at, after) = (s[k_ - 1], s[k_], s[k_ + 1]);
            if before < at && after < at {
                let d = after - before;
                let half = d * 0.5;
                let c = ((after + before) - (at + at)) * 0.5;
                self.peak_height[count] = at - d * d * (0.0625 / c);
                self.peak_place[count] = k as f32 - (0.5 / c) * half;
                count += 1;
            }
        }

        // The highest peak, and the candidates up to twice its number.
        let mut best = 0;
        for i in 1..count {
            if self.peak_height[i].partial_cmp(&self.peak_height[best]) != Some(Ordering::Less) {
                best = i;
            }
        }
        let candidates = (best * 2).min(count);
        let mut found = self.peak_place[0];
        let mut most = -9.0e-10f32;
        let quarter = (n / 4) as i32;
        for &f0 in &self.peak_place[..candidates] {
            let sigma = f0 * 2.0;
            let neg = -1.0 / (sigma * sigma);
            let inv = 1.0 / f64::from(f0);
            let mut sum = 0.0f32;
            let mut k = (f0 * 0.25) as i32;
            while k < quarter {
                let kf = k as f32;
                let cos = (f64::from(kf) * (2.0 * PI) * inv).cos() as f32;
                let dk = kf - f0;
                let g = (dk * dk * neg).exp();
                sum += cos * self.spec[k as usize] * g;
                k += 1;
            }
            if sum >= most {
                most = sum;
                found = f0;
            }
        }
        found * self.rate / n as f32
    }

    /// `FUN_180181d60`: the grains, plus the unvoiced part.
    fn synth(&mut self, unvoiced: [f32; 2]) -> [f32; 2] {
        let n = self.src[0].len() as i32;
        let mut sum = [0.0f32; 2];
        for g in self.grains.iter_mut().filter(|g| g.on) {
            let weight = grain_weight(&self.window, g.left, g.len);
            for (s, total) in sum.iter_mut().enumerate() {
                *total += weight * read_ring(&self.src[s], g.place);
            }
            g.place += g.step;
            if g.place > (n - 1) as f32 {
                g.place -= n as f32;
            }
            g.left -= 1;
            if g.left < 1 {
                g.on = false;
            }
        }
        [sum[0] + unvoiced[0], unvoiced[1] + sum[1]]
    }

    /// One frame of `FUN_180180830`, two channels.
    fn run(&mut self, x: [f32; 2]) -> [f32; 2] {
        let mode = self.mode;
        if mode != 1 {
            let target = if mode == 4 {
                self.line.target
            } else {
                (self.line.target - self.lat_len as f32).max(1.0)
            };
            self.delayed = self.line.read(target);
        }
        let input = match mode {
            2 => [self.fb[0] + x[0], self.fb[1] + x[1]],
            3 => self.delayed,
            _ => x,
        };
        let (voiced, unvoiced) = self.split(input);

        // The latency lines.
        let p = self.lat_pos as usize;
        for (line, v) in
            self.lat
                .iter_mut()
                .zip([voiced[0], unvoiced[0], voiced[1], unvoiced[1], x[0], x[1]])
        {
            line[p] = v;
        }
        self.lat_pos += 1;
        if self.lat_len - 1 < self.lat_pos {
            self.lat_pos = 0;
        }
        let p = self.lat_pos as usize;
        let late = self.lat.each_ref().map(|line| line[p]);
        let w = self.src_write as usize;
        self.src[0][w] = late[0];
        self.src[1][w] = late[2];
        self.src_write += 1;
        if self.src[0].len() as i32 - 1 < self.src_write {
            self.src_write = 0;
        }
        self.detect(voiced);

        // An analysis mark a period back, once a period.
        self.mark_count -= 1;
        if self.mark_count < 1 {
            self.mark = self.src_write - self.period;
            if self.mark < 0 {
                self.mark += self.src[0].len() as i32;
            }
            self.period = (self.rate / self.f0s) as i32;
            self.mark_count = self.period;
        }

        // A grain at the last mark, once a period of the pitch asked for.
        self.synth_count -= 1;
        if self.synth_count < 1 {
            let at = match self.pitch_mode {
                2 => (0.007_644_5 / self.ratio) * self.rate,
                3 => (0.003_822_25 / self.ratio) * self.rate,
                4 => (0.001_911_125 / self.ratio) * self.rate,
                _ => self.period as f32 / self.ratio,
            };
            self.synth_count = at as i32;
            if let Some(slot) = free_grain(&self.grains) {
                let step = self.formant_ratio;
                let len = ((self.period as f32 + self.period as f32) / step) as i32;
                self.grains[slot] = Grain {
                    on: true,
                    len,
                    left: len,
                    place: self.mark as f32,
                    step,
                };
            }
        }

        let mut sum = self.synth([late[1], late[3]]);
        if self.clip {
            sum = sum.map(|v| self.clip(v));
        }

        // The feedback, through the three biquads.
        let source = match mode {
            1 => None,
            2 | 4 => Some(self.delayed),
            3 => Some(sum),
            _ => Some([0.0; 2]),
        };
        if let Some(source) = source {
            for (s, filter) in self.filters.iter_mut().enumerate() {
                self.fb[s] = self.feedback * filter.run(&self.coefs, source[s]);
            }
            if !self.clip {
                self.fb = self.fb.map(|v| self.clip(v));
            }
            let into = match mode {
                2 => sum,
                3 => [self.fb[0] + x[0], self.fb[1] + x[1]],
                4 => [self.fb[0] + sum[0], self.fb[1] + sum[1]],
                _ => [0.0; 2],
            };
            self.line.write(into);
        }

        let dry = [late[4], late[5]];
        [0, 1].map(|s| {
            let y = if mode == 4 {
                sum[s] + self.fb[s]
            } else {
                sum[s]
            };
            y * self.mix + (1.0 - self.mix) * dry[s]
        })
    }
}

/// The plugin: the shell's state and its two engines.
#[derive(Debug, Clone)]
pub struct PitchShifter {
    params: [i32; PARAMS],
    /// The delay counts sixteenths of the tempo.
    sync: bool,
    /// The effect selector: PSOLA, or the general engine.
    psola: bool,
    /// The accurate pitch detector.
    accurate: bool,
    tempo: f32,
    general: General,
    engine: Psola,
}

impl Default for PitchShifter {
    fn default() -> Self {
        let mut plugin = PitchShifter {
            params: DEFAULTS,
            sync: false,
            psola: true,
            accurate: false,
            tempo: 120.0,
            general: General::default(),
            engine: Psola::default(),
        };
        plugin.prepare(44_100.0);
        plugin
    }
}

impl PitchShifter {
    /// The control's place in its range, 0 to 1, as the shell works it
    /// out in doubles.
    fn place(&self, index: usize) -> f32 {
        let (lo, hi) = RANGES[index];
        let x = f64::from(self.params[index] - lo) * (1.0 / f64::from(hi - lo));
        x.clamp(0.0, 1.0) as f32
    }

    /// Semitones from a pitch or formant control's place.
    fn semitones(&self, index: usize) -> f32 {
        ((f64::from(self.place(index)) - 0.5) * 24.0) as f32
    }

    /// `FUN_0072d020`: the PSOLA formant with the correction: the formant
    /// plus the pitch times what the correction leaves, within an octave.
    fn corrected_formant(&self) -> f32 {
        let formant = self.semitones(1);
        let pitch = self.semitones(0);
        let r = formant + pitch * (1.0 - self.place(11));
        r.clamp(-12.0, 12.0)
    }

    /// `FUN_0072d0f0`: the shell hands control `index` to the engines.
    fn push(&mut self, index: usize) {
        let x = self.place(index);
        let v = self.params[index];
        let full = self.place(11) >= 1.0;
        let (pitch, formant, corrected) = (
            self.semitones(0),
            self.semitones(1),
            self.corrected_formant(),
        );
        let (p, g) = (&mut self.engine, &mut self.general);
        match index {
            0 => {
                p.set(0, pitch);
                if !full {
                    p.set(1, corrected);
                }
                g.set(0, pitch);
            }
            1 => p.set(1, if full { formant } else { corrected }),
            2 => p.set(2, (v + 1) as f32),
            3 => {
                p.set(3, x);
                g.set(5, x);
            }
            4 => g.set(1, (f64::from(v) / 100.0) as f32),
            5 => g.set(2, (f64::from(v) / 100.0) as f32),
            6 => g.set(3, x),
            7 => g.set(4, x),
            8 => {
                if self.sync {
                    p.set(8, x * 16.0);
                    g.set(10, x * 16.0);
                } else {
                    let ms = x * 999.0 + 1.0;
                    p.set(4, ms);
                    g.set(6, ms);
                }
            }
            9 => {
                p.set(5, x);
                g.set(7, x);
            }
            10 => {
                p.set(6, (v + 1) as f32);
                g.set(8, (v + 1) as f32);
            }
            11 => p.set(1, corrected),
            12 => p.set(10, (v + 1) as f32),
            _ => {}
        }
    }

    /// Every control in order, then the detector's window
    /// (`FUN_0072dfa0`), as the shell does after a rate or a state.
    fn push_all(&mut self) {
        for index in 0..PARAMS {
            self.push(index);
        }
        self.engine.set(9, DETECTOR[usize::from(self.accurate)]);
    }

    /// Both engines at a rate (the dispatcher's case 4), with the tempo
    /// they had.
    fn prepare(&mut self, rate: f32) {
        self.general.prepare(rate);
        self.engine.prepare(rate);
        self.general.set(9, self.tempo);
        self.engine.set(7, self.tempo);
        self.push_all();
    }
}

impl Effect for PitchShifter {
    /// `FUN_0072c630`: versions 6 and 7. The engines keep what they hold.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let version = state_version(bytes, V7_LEN)?;
        let expected = match version {
            6 => V6_LEN,
            7 => V7_LEN,
            _ => return Err(StateError::Version { found: version }),
        };
        if bytes.len() != expected {
            return Err(StateError::Length {
                expected,
                found: bytes.len(),
            });
        }
        let word = |at: usize| i32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let mut params = [0; PARAMS];
        for (param, value) in params.iter_mut().enumerate() {
            let v = word(4 + 4 * param);
            let (min, max) = RANGES[param];
            if v < min || v > max {
                return Err(StateError::Range { param, value: v });
            }
            *value = v;
        }
        // 56: the correction wheel again, 60: the sync button, 61: the
        // effect, 65: the detector; then the editor's display.
        self.params = params;
        self.sync = bytes[60] != 0;
        self.psola = word(61) == 0;
        self.accurate = word(65) & 0xff != 0;
        self.push_all();
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz >= 1.0 && hz != self.general.rate {
            self.prepare(hz);
        }
    }

    /// `FUN_0072ccf0`: the engines keep the tempo for the next time the
    /// delay is set, as in FL.
    fn set_tempo(&mut self, bpm: f64) {
        if bpm.is_finite() && bpm > 0.0 {
            self.tempo = bpm as f32;
            self.engine.set(7, self.tempo);
            self.general.set(9, self.tempo);
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            [*l, *r] = if self.psola {
                self.engine.run([*l, *r])
            } else {
                self.general.run([*l, *r])
            };
        }
    }

    fn param_count(&self) -> usize {
        PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |v| *v as f32)
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(range) = RANGES.get(index) {
            self.params[index] = knob_value(value, *range);
            self.push(index);
        }
    }

    fn reset(&mut self) {
        self.general.clear();
        self.engine.clear();
    }

    /// The PSOLA engine's dry and wet both come out late by its latency
    /// lines; the general engine is on time.
    fn latency(&self) -> usize {
        if self.psola {
            self.engine.lat_len.max(0) as usize
        } else {
            0
        }
    }

    fn max_latency(&self) -> usize {
        self.engine.max_latency().max(self.latency())
    }

    fn fidelity(&self) -> Fidelity {
        if self.sync && self.params[10] != 0 {
            Fidelity::Approximate(
                "Pitch Shifter's tempo-synced delay takes the state's sync button, where FL's \
                 loader may still map it as milliseconds",
            )
        } else {
            Fidelity::Exact
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{every_preset, partial, presets, projects, sine};

    /// A version 7 state with these controls and the extras.
    fn bytes(params: [i32; PARAMS], sync: bool, psola: bool, accurate: bool) -> Vec<u8> {
        let mut b = 7i32.to_le_bytes().to_vec();
        for v in params {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&params[11].to_le_bytes());
        b.push(u8::from(sync));
        b.extend_from_slice(&i32::from(!psola).to_le_bytes());
        b.extend_from_slice(&i32::from(accurate).to_le_bytes());
        b.extend_from_slice(&1000i32.to_le_bytes());
        b.extend_from_slice(&[1, 1]);
        b
    }

    fn with(params: [i32; PARAMS], psola: bool, rate: f32) -> PitchShifter {
        let mut plugin = PitchShifter::default();
        plugin.set_sample_rate(rate);
        plugin
            .set_state(&bytes(params, false, psola, true))
            .unwrap();
        plugin
    }

    /// Run a mono signal on both sides and keep the left.
    fn run(plugin: &mut PitchShifter, input: &[f32]) -> Vec<f32> {
        let mut l = input.to_vec();
        let mut r = input.to_vec();
        for (a, b) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            plugin.process(a, b);
        }
        l
    }

    #[test]
    fn the_state_s_extras_pick_the_engine_and_the_detector() {
        let b = bytes(DEFAULTS, true, false, true);
        assert_eq!(b.len(), V7_LEN);
        let mut plugin = PitchShifter::default();
        plugin.set_state(&b).unwrap();
        assert!(plugin.sync && !plugin.psola && plugin.accurate);
        assert_eq!(plugin.latency(), 0);
        // Version 6 has no display fields.
        let mut v6 = bytes(DEFAULTS, false, true, false);
        v6[0] = 6;
        v6.truncate(V6_LEN);
        plugin.set_state(&v6).unwrap();
        assert!(plugin.psola && !plugin.accurate);
        assert_eq!(plugin.latency(), (1024.0f32 * 1.2) as usize);
        let mut v8 = b.clone();
        v8[0] = 8;
        assert_eq!(plugin.set_state(&v8), Err(StateError::Version { found: 8 }));
        let mut wrong = b.clone();
        wrong[4..8].copy_from_slice(&1300i32.to_le_bytes());
        assert_eq!(
            plugin.set_state(&wrong),
            Err(StateError::Range {
                param: 0,
                value: 1300
            })
        );
        assert_eq!(
            plugin.set_state(&b[..74]),
            Err(StateError::Length {
                expected: 75,
                found: 74
            })
        );
    }

    #[test]
    fn the_shell_turns_the_wheels_into_the_engines_numbers() {
        let mut params = DEFAULTS;
        params[0] = 1200;
        params[1] = -600;
        params[4] = 8500;
        params[5] = 200;
        params[8] = 768;
        params[11] = 32768;
        let plugin = with(params, true, 44_100.0);
        let g = &plugin.general;
        assert_eq!(g.ratio, 2.0);
        assert_eq!(g.grain_len, (85.0f32 * 0.001 * 44_100.0) as i32);
        assert_eq!(g.density, 2.0);
        // 768 is the top of the delay: 1000 ms.
        assert!((g.line.target - 44_100.0).abs() < 0.01);
        let p = &plugin.engine;
        assert_eq!(p.ratio, 2.0);
        // The formant: -6 + 12 * (1 - 0.5) semitones.
        assert!((p.formant_ratio - 1.0).abs() < 1e-6);
        // Past the octave the correction is clamped.
        params[1] = 1200;
        let plugin = with(params, true, 44_100.0);
        assert_eq!(plugin.engine.formant_ratio, 2.0);
        // A full correction leaves the formant alone.
        params[11] = 65536;
        params[1] = -1200;
        let plugin = with(params, true, 44_100.0);
        assert_eq!(plugin.engine.formant_ratio, 0.5);
    }

    #[test]
    fn the_clip_is_tanh() {
        let g = &PitchShifter::default().general;
        for x in [-3.0f32, -0.5, 0.0, 0.25, 1.0, 2.0, 7.0] {
            assert!((g.clip(x) - (1.002 * x).tanh()).abs() < 2e-4, "{x}");
        }
    }

    #[test]
    fn the_fft_finds_a_partial() {
        let mut fft = Fft::new(64);
        for k in 0..64 {
            fft.re[k] = (2.0 * PI * 5.0 * k as f64 / 64.0).cos();
            fft.im[k] = 0.0;
        }
        fft.forward();
        assert!((fft.re[5] - 0.5).abs() < 1e-12 && (fft.re[59] - 0.5).abs() < 1e-12);
        assert!(fft.re[4].abs() < 1e-12 && fft.im[5].abs() < 1e-12);
    }

    #[test]
    fn with_no_mix_the_general_engine_passes_the_input_and_psola_delays_it() {
        let mut params = DEFAULTS;
        params[3] = 0;
        params[0] = 700;
        let input = sine(0.01, 6000);
        let mut general = with(params, false, 48_000.0);
        assert_eq!(run(&mut general, &input), input);
        let mut psola = with(params, true, 48_000.0);
        let late = psola.latency();
        assert_eq!(
            late,
            ((2048.0f32 * 48_000.0 * 2.267_573_7e-5) as i32 as f32 * 1.2) as usize
        );
        // FL reports the length of the latency lines, and they hold one
        // sample less.
        let out = run(&mut psola, &input);
        assert_eq!(&out[late - 1..], &input[..input.len() + 1 - late]);
        assert!(out[..late - 1].iter().all(|v| *v == 0.0));
    }

    #[test]
    fn the_general_engine_moves_a_tone_an_octave() {
        let mut params = DEFAULTS;
        params[0] = 1200;
        params[5] = 200;
        params[6] = 0;
        params[7] = 0;
        let mut plugin = with(params, false, 48_000.0);
        // 375 Hz, then what comes out in its last 0.5 s.
        let input = sine(375.0 / 48_000.0, 48_000);
        let out = run(&mut plugin, &input);
        // Each grain starts a little out of step with the one before, so
        // the octave comes out a little flat: look around 750 Hz.
        let tail = &out[24_000..];
        let near = |hz: f64| {
            let at = (0..=40).map(|i| partial(tail, (hz - 10.0 + 0.5 * f64::from(i)) / 48_000.0));
            at.fold(0.0, f64::max)
        };
        let (at, up) = (near(375.0), near(750.0));
        assert!(up > 0.6 && up > 50.0 * at, "{at} {up}");
    }

    /// A tone with harmonics at `hz`.
    fn voice(hz: f64, len: usize) -> Vec<f32> {
        (0..len)
            .map(|n| {
                let t = 2.0 * PI * hz * n as f64 / 48_000.0;
                (0.5 * t.sin() + 0.3 * (2.0 * t).sin() + 0.2 * (3.0 * t).sin()) as f32
            })
            .collect()
    }

    #[test]
    fn the_detector_finds_a_voice_s_pitch() {
        let mut plugin = with(DEFAULTS, true, 48_000.0);
        run(&mut plugin, &voice(200.0, 96_000));
        let f = plugin.engine.f0s;
        assert!((f - 200.0).abs() < 4.0, "{f}");
        assert!((plugin.engine.period - 240).abs() <= 5);
    }

    #[test]
    fn psola_moves_a_voice_an_octave() {
        let mut params = DEFAULTS;
        params[0] = 1200;
        params[11] = 65536;
        let mut plugin = with(params, true, 48_000.0);
        let out = run(&mut plugin, &voice(200.0, 96_000));
        // The last 0.5 s: the fundamental at 400 Hz.
        let tail = &out[48_000..];
        let (at, up) = (
            partial(tail, 200.0 / 48_000.0),
            partial(tail, 400.0 / 48_000.0),
        );
        assert!(up > 0.1 && up > 50.0 * at, "{at} {up}");
    }

    #[test]
    fn a_delay_mode_echoes_the_input() {
        // Mode 4: the shifted sound into the delay and out on top, with
        // nothing fed back: one echo 100 ms on.
        let mut params = DEFAULTS;
        params[5] = 200;
        params[6] = 0;
        params[7] = 0;
        params[10] = 3;
        params[8] = 1 + (767.0f64 * 99.0 / 999.0).round() as i32;
        params[9] = 50;
        let mut plugin = with(params, false, 48_000.0);
        // 50 ms of 1 kHz: the feedback's filters keep that and cut the
        // lows.
        let mut input = sine(1000.0 / 48_000.0, 48_000);
        input[2400..].fill(0.0);
        let out = run(&mut plugin, &input);
        assert!(out.iter().all(|v| v.is_finite() && v.abs() < 2.0));
        // The echoes 100 and 200 ms on, the second half the first.
        let level = |from: usize| out[from..from + 2400].iter().map(|v| v.abs()).sum::<f32>();
        let gap: f32 = out[2600..4700].iter().map(|v| v.abs()).sum();
        let (first, second) = (level(4800 + 100), level(9600 + 200));
        assert!(gap < first * 0.1, "{gap} {first}");
        assert!(
            first > 300.0 && (second / first - 0.5).abs() < 0.2,
            "{first} {second}"
        );
    }

    #[test]
    fn a_moved_wheel_is_the_same_plugin_as_its_state() {
        let mut moved = with(DEFAULTS, true, 48_000.0);
        let mut params = DEFAULTS;
        params[0] = -500;
        params[11] = 20000;
        moved.set_param(0, -500.0);
        moved.set_param(11, 20000.0);
        let state = with(params, true, 48_000.0);
        assert_eq!(moved.engine.ratio, state.engine.ratio);
        assert_eq!(moved.engine.formant_ratio, state.engine.formant_ratio);
        assert_eq!(moved.param(0), -500.0);
        assert_eq!(moved.param(13), 0.0);
    }

    #[test]
    fn every_state_fl_ships_plays() {
        let Some(root) = crate::testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, Pitch Shifter's states skipped");
            return;
        };
        let own = presets(NAME).unwrap();
        let mut states: Vec<_> = projects(&root)
            .into_iter()
            .filter(|s| s.plugin.eq_ignore_ascii_case(NAME))
            .collect();
        assert_eq!(states.len(), 15);
        let all = every_preset(&root, NAME);
        assert!(!own.is_empty() && all.len() >= own.len());
        eprintln!("{} presets of its own, {} in all", own.len(), all.len());
        states.extend(all);
        let input = voice(180.0, 24_000);
        for saved in &states {
            let mut plugin = PitchShifter::default();
            plugin.set_sample_rate(48_000.0);
            plugin.set_tempo(128.0);
            plugin
                .set_state(&saved.state)
                .unwrap_or_else(|e| panic!("{}: {e}", saved.file));
            let out = run(&mut plugin, &input);
            assert!(
                out.iter().all(|v| v.is_finite() && v.abs() < 8.0),
                "{}",
                saved.file
            );
            assert!(plugin.latency() <= plugin.max_latency());
        }
    }
}
