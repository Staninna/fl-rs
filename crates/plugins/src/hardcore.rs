//! Hardcore: a guitar rack of five pedal boxes between an input and an
//! output gain, with a cabinet and an eight-band equaliser after them, as
//! `docs/plugins/hardcore.md` reads it from `Hardcore_x64.dll`.
//!
//! The state is FL's wrapper around the plugin's chunk: `SSLF`, an i32
//! length, then `1RCH`, the 50 parameters as f32 and the program's name in
//! 256 bytes.
//!
//! | parameter | what |
//! |---|---|
//! | 0, 1 | input and output level: `10^(2.4 v - 1.2)`, ±24 dB |
//! | 2 | equaliser on |
//! | 3..10 | the eight bands |
//! | 11 | the cabinet, 0 for none, else its place in `Imp` from 1 |
//! | 13..37 | five parameters for each of the five boxes |
//! | 39..43 | each box on |
//! | 45..49 | each box's pedal, as a whole number: 0 none, 1 Distortion (model A), 2..11 the others |
//!
//! Built: the levels, the input's 30 Hz high pass, the boxes and the
//! Distortion pedal (`CPedal_DistModelA`), which is what FL's project and
//! render test use. The other ten pedals, the cabinet and the equaliser
//! are left out and named by [`Effect::fidelity`] when a state asks for
//! them.

use crate::{Effect, Fidelity, StateError};

/// The plugin's event 201 name.
pub const NAME: &str = "Hardcore";

/// The parameters of the chunk.
const PARAMS: usize = 50;
/// The boxes, and each box's parameters.
const BOXES: usize = 5;
const BOX_PARAMS: usize = 5;
/// Where the boxes' parameters, switches and pedals start.
const BOX_START: usize = 13;
const BOX_ON: usize = 39;
const BOX_KIND: usize = 45;
const EQ_ON: usize = 2;
const CABINET: usize = 11;

/// The magic words: FL's wrapper and the plugin's chunk.
const SSLF: u32 = 0x464c_5353;
const CHUNK: u32 = 0x4843_5231;

/// What the plugin's default program has (`--- Defult ---` as FL's
/// project saves it, which is the plugin's program 0).
const DEFAULTS: [f32; PARAMS] = [
    0.5, 0.5, 0.0, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.0, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5,
    0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5,
    0.5, 1.0, 0.0, 0.0, 0.0, 0.0, 0.5, 1.0, 0.0, 0.0, 0.0, 0.0,
];

/// What [`Effect::fidelity`] says.
const OTHER_PEDAL: &str = "Hardcore has a pedal on that is not built: it passes";
const CABINET_OUT: &str = "Hardcore has a cabinet that is left out";
const EQ_OUT: &str = "Hardcore has its equaliser on, which is left out";

/// The equaliser's two pi (`_DAT_180075eb0`), short of the real one.
#[expect(clippy::approx_constant, reason = "the digits the DLL has")]
const TWO_PI: f64 = 6.283_185;

/// The smallest a filter keeps (`DAT_180075cec`): anything nearer 0 is 0.
const TINY: f32 = 1e-9;

fn flush(x: f32) -> f32 {
    if x < TINY && x > -TINY { 0.0 } else { x }
}

/// `sinf` and `cosf` together (`FUN_180055e50`), in f64 here.
fn sincos(w: f32) -> (f32, f32) {
    let (s, c) = f64::from(w).sin_cos();
    (s as f32, c as f32)
}

/// A second-order section: its design's frequency, Q and rate, three
/// coefficients and its last two inputs and outputs (`FUN_18001d010` and
/// kin). The kinds differ in their numerators.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Section {
    key: [f32; 3],
    c: [f32; 3],
    /// x2, x1, y2, y1.
    s: [f32; 4],
}

impl Section {
    fn snap(c: &mut [f32; 3]) {
        for v in c.iter_mut() {
            *v = flush(*v);
        }
    }

    /// The high pass of `FUN_18001d050`.
    fn high_pass(&mut self, hz: f32, q: f32, rate: f32) {
        if self.key == [hz, q, rate] {
            return;
        }
        self.key = [hz, q, rate];
        let w = (f64::from(hz) * std::f64::consts::TAU / f64::from(rate)) as f32;
        let (s, c) = sincos(w);
        let k = 1.0 / (s + q);
        let mut co = [(c + 1.0) / (s / q + 2.0), c * k * q * -2.0, (q - s) * k];
        Self::snap(&mut co);
        self.c = co;
    }

    /// The low pass of `FUN_18001d240`.
    fn low_pass(&mut self, hz: f32, q: f32, rate: f32) {
        if self.key == [hz, q, rate] {
            return;
        }
        self.key = [hz, q, rate];
        let w = (f64::from(hz) * std::f64::consts::TAU / f64::from(rate)) as f32;
        let (s, c) = sincos(w);
        let k = 1.0 / (s + q);
        let mut co = [(1.0 - c) / (s / q + 2.0), c * k * q * -2.0, (q - s) * k];
        Self::snap(&mut co);
        self.c = co;
    }

    /// The band pass of `FUN_18001d420`, designed in f64.
    fn band_pass(&mut self, hz: f32, q: f32, rate: f32) {
        if self.key == [hz, q, rate] {
            return;
        }
        self.key = [hz, q, rate];
        let w = f64::from(hz) * std::f64::consts::TAU / f64::from(rate);
        let (s, c) = w.sin_cos();
        let a0 = 1.0 / ((0.5 / f64::from(q)) * s + 1.0);
        let mut co = [
            (f64::from((s * 0.5) as f32) * a0) as f32,
            (f64::from((c + c) as f32) * a0) as f32,
            (f64::from((1.0 - s / f64::from(q + q)) as f32) * a0) as f32,
        ];
        Self::snap(&mut co);
        self.c = co;
    }

    fn shift(&mut self, x: f32, y: f32) -> f32 {
        let y = flush(y);
        let [_, x1, _, y1] = self.s;
        self.s = [x1, x, y1, y];
        y
    }

    /// `FUN_18001d150`.
    fn run_high(&mut self, x: f32) -> f32 {
        let x = flush(x);
        let [x2, x1, y2, y1] = self.s;
        let y = ((x - (x1 + x1)) + x2) * self.c[0] - y1 * self.c[1] - y2 * self.c[2];
        self.shift(x, y)
    }

    /// `FUN_18001d340`.
    fn run_low(&mut self, x: f32) -> f32 {
        let x = flush(x);
        let [x2, x1, y2, y1] = self.s;
        let y = (x1 + x1 + x + x2) * self.c[0] - y1 * self.c[1] - y2 * self.c[2];
        self.shift(x, y)
    }

    /// `FUN_18001d580`.
    fn run_band(&mut self, x: f32) -> f32 {
        let x = flush(x);
        let [x2, _, y2, y1] = self.s;
        let y = ((x - x2) * self.c[0] + y1 * self.c[1]) - y2 * self.c[2];
        self.shift(x, y)
    }

    fn clear(&mut self) {
        self.s = [0.0; 4];
    }
}

/// The peaking equaliser of `FUN_18001d660` (type 1), normalised.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Peak {
    b: [f32; 3],
    a: [f32; 2],
    /// x1, x2, y1, y2.
    s: [f32; 4],
}

impl Peak {
    fn design(&mut self, hz: f32, octaves: f32, db: f32, rate: f32) {
        let big_a = 10f64.powf(f64::from(db * 0.05)) as f32;
        let w = ((f64::from(hz) * TWO_PI) / f64::from(rate)) as f32;
        let w = w.clamp(0.0, 3.0);
        let sin = f64::from(w).sin() as f32;
        let x = f64::from(octaves) * 0.346_573_6 * f64::from(w) / f64::from(sin);
        let alpha = sin * ((x.sinh() * 2.0) as f32) * 0.5;
        let cos = f64::from(w).cos() as f32;
        let (up, down) = (alpha * big_a, alpha / big_a);
        let a0 = down + 1.0;
        let k = 1.0 / a0;
        let c = [up + 1.0, cos * -2.0, 1.0 - up, cos * -2.0, 1.0 - down].map(|v| flush(k * v));
        self.b = [c[0], c[1], c[2]];
        self.a = [c[3], c[4]];
    }

    /// `FUN_18001da00`.
    fn run(&mut self, x: f32) -> f32 {
        let x = flush(x);
        let [x1, x2, y1, y2] = self.s;
        let y = flush(
            ((x1 * self.b[1] + x * self.b[0] + self.b[2] * x2) - y1 * self.a[0]) - self.a[1] * y2,
        );
        self.s = [x, x1, y, y1];
        y
    }
}

/// The Distortion pedal (`CPedal_DistModelA`, process `FUN_18002bc00`):
/// the input in two bands, each eight times between its samples and
/// through an exponential shaper, the two mixed through two short combs,
/// then a low pass, a high pass and a peak.
#[derive(Debug, Clone, PartialEq)]
struct DistModelA {
    params: [f32; BOX_PARAMS],
    rate: f32,
    /// The shaper's power and the gain after the low pass.
    drive: f32,
    makeup: f32,
    low: Section,
    high: Section,
    smooth: Section,
    tone: Section,
    peak: Peak,
    /// The last three of each band, the oldest first.
    a: [f32; 3],
    b: [f32; 3],
    ring: Vec<f32>,
    at: usize,
    /// The second comb: its ring, the most it is long, how long it is now.
    ring2: Vec<f32>,
    most2: usize,
    len2: usize,
    at2: usize,
}

impl DistModelA {
    fn new(rate: f32) -> Self {
        let mut pedal = DistModelA {
            params: [0.5; BOX_PARAMS],
            rate,
            drive: 1.0,
            makeup: 1.0,
            low: Section::default(),
            high: Section::default(),
            smooth: Section::default(),
            tone: Section::default(),
            peak: Peak::default(),
            a: [0.0; 3],
            b: [0.0; 3],
            ring: Vec::new(),
            at: 0,
            ring2: Vec::new(),
            most2: 1,
            len2: 0,
            at2: 0,
        };
        pedal.set_rate(rate);
        pedal
    }

    /// `FUN_18002c060`. This asks for memory.
    fn set_rate(&mut self, rate: f32) {
        self.rate = rate;
        let len = ((f64::from(rate * 8.0) * 0.001) as i32).max(1) as usize;
        self.ring = vec![0.0; len];
        self.most2 = ((f64::from(rate * 8.0) * 0.0002) as i32).max(1) as usize;
        self.ring2 = vec![0.0; self.most2];
        self.low.band_pass(200.0, 0.99, rate);
        self.high.band_pass(1000.0, 0.99, rate);
        self.smooth.low_pass(500.0, 4.0, rate);
        self.reset();
        self.update();
    }

    /// `FUN_18002c2f0`.
    fn update(&mut self) {
        let [drive, length, tone, peak, _] = self.params;
        self.drive = drive * drive * 99.0 + 1.0;
        self.makeup = 1.0 / (drive * 8.0 + 1.0);
        self.tone.high_pass(85.0, tone * 30.0 + 1.0, self.rate);
        self.peak
            .design(2500.0, 3.0, (peak - 0.5) * 20.0, self.rate);
        let most = self.most2 as i32;
        let mut len = (most as f32 * length) as i32;
        if len >= most {
            len = most - 1;
        }
        self.len2 = len.max(0) as usize;
        if self.at2 >= self.len2.max(1) {
            self.at2 = 0;
        }
    }

    /// `FUN_18002c220`.
    fn reset(&mut self) {
        self.low.clear();
        self.high.clear();
        self.ring.fill(0.0);
        self.at = 0;
        self.ring2.fill(0.0);
        self.at2 = 0;
        self.smooth.clear();
        self.tone.clear();
        self.a = [0.0; 3];
        self.b = [0.0; 3];
    }

    /// The shaper: `1 - 10^(-d y) (1 - y)` above 0, `10^(d y) (y + 1) - 1`
    /// under.
    fn shape(&self, y: f32) -> f32 {
        let d = self.drive * y;
        if y >= 0.0 {
            1.0 - powf(10.0, -d) * (1.0 - y)
        } else {
            powf(10.0, d) * (y + 1.0) - 1.0
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let m = *l + *r;
            let a_new = (f64::from(self.low.run_band(m)) * 0.05) as f32;
            let b_new = self.high.run_band(m) * 5.0;
            let [a0, a1, a2] = self.a;
            let [b0, b1, b2] = self.b;
            self.a = [a1, a2, a_new];
            self.b = [b1, b2, b_new];
            let a3 = (a1 - a2) * 1.5 + (a_new - a0) * 0.5;
            let a_c1 = (a2 - a0) * 0.5;
            let a_c2 = ((a0 - a1) + a_c1) - a3;
            let b3 = (b1 - b2) * 1.5 + (b_new - b0) * 0.5;
            let b_c1 = (b2 - b0) * 0.5;
            let b_c2 = ((b0 - b1) + b_c1) - b3;
            let mut t = 0.0f32;
            let mut v = 0.0;
            for _ in 0..8 {
                let ya = ((t * a3 + a_c2) * t + a_c1) * t + a1;
                let yb = ((t * b3 + b_c2) * t + b_c1) * t + b1;
                t += 0.125;
                let ya = self.shape(ya);
                let yb = self.shape(yb);
                let held = self.ring[self.at];
                self.ring[self.at] = yb - held * 0.25;
                self.at += 1;
                if self.at >= self.ring.len() {
                    self.at = 0;
                }
                let mut s = (f64::from(held) * 0.7 + f64::from(ya)) as f32;
                if self.len2 != 0 {
                    let old = self.ring2[self.at2];
                    self.ring2[self.at2] = s;
                    self.at2 += 1;
                    if self.at2 >= self.len2 {
                        self.at2 = 0;
                    }
                    s = (f64::from(s) - f64::from(old) * 0.3) as f32;
                }
                v = self.smooth.run_low(s);
            }
            let y = self.tone.run_high(v * self.makeup);
            let y = self.peak.run(y);
            *l = y;
            *r = y;
        }
    }
}

/// MSVC's `powf` (`FUN_180042a10`), taken as f64's rounded.
fn powf(base: f32, exponent: f32) -> f32 {
    f64::from(base).powf(f64::from(exponent)) as f32
}

/// A pedal in a box: the one built, or one that passes.
#[derive(Debug, Clone, PartialEq)]
enum Pedal {
    Distortion(Box<DistModelA>),
    Other(i32),
}

/// Hardcore. A new one has its default program: box 1 a Distortion that
/// is on, the rest empty, no cabinet and no equaliser.
#[derive(Debug, Clone)]
pub struct Hardcore {
    params: [f32; PARAMS],
    rate: f32,
    boxes: [Option<Pedal>; BOXES],
    input: [Section; 2],
    /// The levels: where each is and where it goes.
    gains: [f32; 2],
    targets: [f32; 2],
}

impl Default for Hardcore {
    fn default() -> Self {
        let mut plugin = Hardcore {
            params: DEFAULTS,
            rate: 44_100.0,
            boxes: Default::default(),
            input: [Section::default(); 2],
            gains: [-1.0; 2],
            targets: [1.0; 2],
        };
        plugin.apply_all();
        plugin
    }
}

impl Hardcore {
    /// Every parameter in order, as `FUN_180029e30` and a state do. Making
    /// a pedal asks for memory.
    fn apply_all(&mut self) {
        let rate = self.rate;
        for side in &mut self.input {
            side.high_pass(30.0, 1.0, rate);
        }
        for index in 0..PARAMS {
            self.apply(index);
        }
    }

    /// `FUN_18002a010` for one parameter.
    fn apply(&mut self, index: usize) {
        let v = self.params[index];
        match index {
            0 | 1 => self.targets[index] = powf(10.0, v * 2.4 - 1.2),
            BOX_KIND..=49 => {
                let slot = index - BOX_KIND;
                let kind = (v as i32 as f32 as i32).clamp(-1, 11);
                self.boxes[slot] = match kind {
                    1 => {
                        let mut pedal = DistModelA::new(self.rate);
                        let start = BOX_START + slot * BOX_PARAMS;
                        pedal
                            .params
                            .copy_from_slice(&self.params[start..start + BOX_PARAMS]);
                        pedal.update();
                        Some(Pedal::Distortion(Box::new(pedal)))
                    }
                    2..=11 => Some(Pedal::Other(kind)),
                    _ => None,
                };
            }
            BOX_START..=37 => {
                let slot = (index - BOX_START) / BOX_PARAMS;
                if let Some(Pedal::Distortion(pedal)) = &mut self.boxes[slot] {
                    pedal.params[(index - BOX_START) % BOX_PARAMS] = v;
                    pedal.update();
                }
            }
            _ => {}
        }
    }
}

impl Effect for Hardcore {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let word = |at: usize| {
            bytes
                .get(at..at + 4)
                .map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
        };
        let short = StateError::Length {
            expected: 8 + 4 + 4 * PARAMS,
            found: bytes.len(),
        };
        if word(0) != Some(SSLF) {
            return Err(StateError::Version {
                found: word(0).unwrap_or(0) as i32,
            });
        }
        let len = word(4).ok_or(short)? as usize;
        let chunk = bytes.get(8..8 + len).ok_or(short)?;
        if chunk.len() < 4 || u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) != CHUNK
        {
            return Err(StateError::Unhosted);
        }
        let mut params = self.params;
        for (param, word) in params.iter_mut().zip(chunk[4..].as_chunks::<4>().0) {
            let v = f32::from_le_bytes(*word);
            if !v.is_finite() {
                return Err(StateError::Range {
                    param: 0,
                    value: v.to_bits() as i32,
                });
            }
            *param = v;
        }
        self.params = params;
        self.boxes = Default::default();
        self.apply_all();
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz > 0.0 {
            self.rate = hz;
            for side in &mut self.input {
                side.high_pass(30.0, 1.0, hz);
                side.clear();
            }
            for pedal in self.boxes.iter_mut().flatten() {
                if let Pedal::Distortion(pedal) = pedal {
                    pedal.set_rate(hz);
                }
            }
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len());
        let (left, right) = (&mut left[..n], &mut right[..n]);
        // A level that is under 0 is one that starts where it is going.
        for (gain, target) in self.gains.iter_mut().zip(self.targets) {
            if *gain < 0.0 {
                *gain = target;
            }
        }
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let g = self.gains[0];
            *l = self.input[0].run_high(*l) * g;
            *r = self.input[1].run_high(*r) * g;
            self.gains[0] = (self.targets[0] - g) * 0.001 + g;
        }
        for (slot, pedal) in self.boxes.iter_mut().enumerate() {
            if self.params[BOX_ON + slot] == 0.0 {
                continue;
            }
            if let Some(Pedal::Distortion(pedal)) = pedal {
                pedal.process(left, right);
            }
        }
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let g = self.gains[1];
            *l *= g;
            *r *= g;
            self.gains[1] = (self.targets[1] - g) * 0.001 + g;
        }
    }

    fn param_count(&self) -> usize {
        PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).copied().unwrap_or(0.0)
    }

    /// A parameter is a float from 0 to 1. The pedals' kinds are not
    /// moved here: a new pedal asks for memory.
    fn set_param(&mut self, index: usize, value: f32) {
        if index < PARAMS && value.is_finite() && !(BOX_KIND..PARAMS).contains(&index) {
            // A VST parameter is 0 to 1; the cabinet is the place of a file.
            let most = if index == CABINET { 64.0 } else { 1.0 };
            self.params[index] = value.clamp(0.0, most);
            self.apply(index);
        }
    }

    fn reset(&mut self) {
        for side in &mut self.input {
            side.clear();
        }
        for pedal in self.boxes.iter_mut().flatten() {
            if let Pedal::Distortion(pedal) = pedal {
                pedal.reset();
            }
        }
    }

    fn fidelity(&self) -> Fidelity {
        let other = self.boxes.iter().enumerate().any(|(slot, pedal)| {
            matches!(pedal, Some(Pedal::Other(_))) && self.params[BOX_ON + slot] != 0.0
        });
        if other {
            Fidelity::Approximate(OTHER_PEDAL)
        } else if self.params[CABINET] as i32 > 0 {
            Fidelity::Approximate(CABINET_OUT)
        } else if self.params[EQ_ON] != 0.0 {
            Fidelity::Approximate(EQ_OUT)
        } else {
            Fidelity::Exact
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{corpus, presets, projects, sine};

    fn state(params: &[f32; PARAMS]) -> Vec<u8> {
        let mut chunk = CHUNK.to_le_bytes().to_vec();
        for p in params {
            chunk.extend(p.to_le_bytes());
        }
        chunk.extend([0u8; 256]);
        let mut out = SSLF.to_le_bytes().to_vec();
        out.extend((chunk.len() as u32).to_le_bytes());
        out.extend(chunk);
        out
    }

    #[test]
    fn the_levels_are_24_db_either_way() {
        let mut p = Hardcore::default();
        let mut params = DEFAULTS;
        params[0] = 1.0;
        params[1] = 0.0;
        p.set_state(&state(&params)).unwrap();
        assert!((p.targets[0] - 15.848_93).abs() < 1e-3);
        assert!((p.targets[1] - 0.063_095_73).abs() < 1e-6);
    }

    #[test]
    fn with_every_box_off_it_is_a_30_hz_high_pass() {
        let mut params = DEFAULTS;
        params[BOX_ON] = 0.0;
        let mut p = Hardcore::default();
        p.set_state(&state(&params)).unwrap();
        let level = |p: &mut Hardcore, hz: f64| {
            p.reset();
            let input = sine(hz / 44_100.0, 44_100);
            let (mut l, mut r) = (input.clone(), input);
            p.process(&mut l, &mut r);
            l[22_050..].iter().fold(0f32, |m, x| m.max(x.abs()))
        };
        assert!((level(&mut p, 1000.0) - 1.0).abs() < 0.01);
        assert!(level(&mut p, 5.0) < 0.05);
    }

    #[test]
    fn the_shaper_saturates_towards_one() {
        let mut d = DistModelA::new(44_100.0);
        d.params[0] = 1.0;
        d.update();
        assert_eq!(d.shape(0.0), 0.0);
        assert!(d.shape(1.0) > 0.999 && d.shape(1.0) <= 1.0);
        assert!(d.shape(-1.0) < -0.999 && d.shape(-1.0) >= -1.0);
        // An odd curve near 0, with a slope of 1 + d there.
        let small = d.shape(1e-4) / 1e-4;
        assert!(
            (small - (1.0 + 100.0 * 10f32.ln())).abs() / small < 0.02,
            "{small}"
        );
    }

    #[test]
    fn the_combs_are_8_ms_and_up_to_1_6_ms_at_8x() {
        let d = DistModelA::new(44_100.0);
        assert_eq!(d.ring.len(), 352);
        assert_eq!(d.most2, 70);
        assert_eq!(d.len2, 35);
    }

    #[test]
    fn fl_s_states_load_and_play() {
        let Some(root) = corpus() else {
            eprintln!("FL_RS_CORPUS unset, Hardcore's states skipped");
            return;
        };
        let mut saved = presets(NAME).unwrap();
        let in_projects: Vec<_> = projects(&root)
            .into_iter()
            .filter(|s| s.plugin.eq_ignore_ascii_case(NAME))
            .collect();
        // Treben - Umziky and the render test.
        assert_eq!(in_projects.len(), 2);
        saved.extend(in_projects);
        let input: Vec<f32> = sine(110.0 / 44_100.0, 8192)
            .iter()
            .map(|x| x * 0.5)
            .collect();
        for s in saved {
            let mut p = Hardcore::default();
            p.set_state(&s.state)
                .unwrap_or_else(|e| panic!("{}: {e}", s.file));
            let (mut l, mut r) = (input.clone(), input.clone());
            p.process(&mut l, &mut r);
            let peak = l.iter().chain(&r).fold(0f32, |m, x| m.max(x.abs()));
            assert!(l.iter().chain(&r).all(|x| x.is_finite()), "{}", s.file);
            assert!(peak > 0.01 && peak < 8.0, "{}: {peak}", s.file);
        }
    }
}
