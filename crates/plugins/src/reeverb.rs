//! Fruity Reeverb, the first one: a plate in Dattorro's figure-of-eight
//! shape, with a low cut, a high cut, a predelay after the input
//! diffusers, a colour shelf and damping in the tank, as
//! `docs/plugins/fruity-reeverb.md` has it.
//!
//! The state is a version, 1, and ten little-endian i32:
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Low cut | 0 to 65536 | 1250 |
//! | 1 High cut | 0 to 65536 | 10800 |
//! | 2 Predelay, ms | 0 to 250 | 0 |
//! | 3 Room size | 0 to 65536 | 26000 |
//! | 4 Diffusion | 0 to 65536 | 65536 |
//! | 5 Color | 0 to 40 | 20 |
//! | 6 Decay | 0 to 65536 | 4700 |
//! | 7 High damping | 0 to 65536 | 10800 |
//! | 8 Dry | 0 to 65536 | 65536 |
//! | 9 Reverb | 0 to 65536 | 13120 |

use crate::{Effect, StateError, knob_value, read_state, state_version};
use std::f64::consts::PI;

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Reeverb";

const RANGES: [(i32, i32); 10] = [
    (0, 65536),
    (0, 65536),
    (0, 250),
    (0, 65536),
    (0, 65536),
    (0, 40),
    (0, 65536),
    (0, 65536),
    (0, 65536),
    (0, 65536),
];
const DEFAULTS: [i32; 10] = [1250, 10800, 0, 26000, 65536, 20, 4700, 10800, 65536, 13120];
/// The state's length: the version and the ten wheels.
const LEN: usize = 44;

/// The twelve lines at 20 kHz and the largest room (`0x0070aff8`): four
/// input diffusers, then each half of the tank: a decay diffuser, a delay,
/// a second decay diffuser and a delay.
const LINES: [i64; 12] = [
    142, 107, 379, 277, 672, 4453, 1800, 3720, 908, 4217, 2656, 3163,
];
/// The output taps at 20 kHz (`0x0070b028` for the left, `0x0070b044` for
/// the right).
const TAPS_LEFT: [i64; 7] = [266, 2974, 1913, 2496, 1690, 187, 1566];
const TAPS_RIGHT: [i64; 7] = [353, 3627, 1728, 3173, 1811, 335, 621];
/// What the lines are filled with after a flush: 1e-30, `0x0da24260`.
const FILL: f32 = 1e-30;

/// Delphi's `MulDiv` as the plugin has it: a 128-bit product divided,
/// toward 0.
fn mul_div(a: i64, b: i64, c: i64) -> i64 {
    (i128::from(a) * i128::from(b) / i128::from(c)) as i64
}

/// Where a wheel sits between its ends, as FL hands it on: `f32`.
fn norm(value: i32, (min, max): (i32, i32)) -> f32 {
    let n = f64::from(value - min) * (1.0 / f64::from(max - min));
    n.clamp(0.0, 1.0) as f32
}

/// The coefficient of the one-pole low pass both cuts above use, for a
/// wheel value `v` in hundreds of Hz: 1 (open) from 221 up.
fn one_pole(v: i32, hz: f64) -> f32 {
    if v as f32 >= 221.0 {
        return 1.0;
    }
    let c = (f64::from(v) * 628.318_530_717_958_7 / hz).cos();
    let b = (2.0 - c) as f32;
    let root = (f64::from(b * b) - 1.0).sqrt();
    (1.0 - (f64::from(b) - root)) as f32
}

/// FL's `Power(x, 0.25)`: `exp(0.25 ln x)`.
fn fourth_root(x: f64) -> f64 {
    (0.25 * x.ln()).exp()
}

/// What the wheels come to (`FUN_006bb8d0`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Coefficients {
    low_cut: f32,
    high_cut: f32,
    input_diffusion: [f32; 2],
    decay_diffusion: [f32; 2],
    decay: f32,
    shelf: f32,
    shelf_gain: f32,
    damping: f32,
    dry: f32,
    wet: f32,
}

/// Fruity Reeverb. A new one has FL's defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct Reeverb {
    params: [i32; 10],
    hz: f64,
    c: Coefficients,
    /// The room the lines were last cleared for.
    room: i32,
    /// The twelve lines and the predelay, one after the other in `memory`,
    /// each with room for its longest: where each starts, how long it is
    /// now, and where it is.
    memory: Vec<f32>,
    start: [usize; 13],
    len: [usize; 13],
    at: [usize; 13],
    taps_left: [usize; 7],
    taps_right: [usize; 7],
    /// The cuts', the shelves' and the damping's memories.
    high_cut: f32,
    low_cut: [f32; 2],
    shelves: [[f32; 2]; 2],
    damped: [f32; 2],
}

impl Default for Reeverb {
    fn default() -> Self {
        let mut plugin = Reeverb {
            params: DEFAULTS,
            hz: 44_100.0,
            c: Coefficients::default(),
            room: -1,
            memory: Vec::new(),
            start: [0; 13],
            len: [1; 13],
            at: [0; 13],
            taps_left: [0; 7],
            taps_right: [0; 7],
            high_cut: 0.0,
            low_cut: [0.0; 2],
            shelves: [[0.0; 2]; 2],
            damped: [0.0; 2],
        };
        plugin.allocate();
        plugin.set(DEFAULTS);
        plugin
    }
}

impl Reeverb {
    /// Lay the lines out for the largest room at this rate, as FL does when
    /// it is told the rate (`FUN_006bb600`), and flush them.
    fn allocate(&mut self) {
        let sr = self.hz as i64;
        let mut total = 0;
        for (k, line) in LINES.iter().enumerate() {
            self.start[k] = total;
            total += mul_div(*line, sr, 20_000).max(1) as usize;
        }
        self.start[12] = total;
        total += mul_div(250, sr, 1000).max(1) as usize;
        self.memory = vec![0.0; total];
        self.at = [0; 13];
        self.room = -1;
        self.flush();
    }

    /// FL's flush (`FUN_006ba400`): every line full of 1e-30, every filter
    /// at 0.
    fn flush(&mut self) {
        self.memory.fill(FILL);
        self.high_cut = 0.0;
        self.low_cut = [0.0; 2];
        self.shelves = [[0.0; 2]; 2];
        self.damped = [0.0; 2];
    }

    /// Take parameters that are inside their ranges.
    fn set(&mut self, params: [i32; 10]) {
        self.params = params;
        let n = |i: usize| norm(params[i], RANGES[i]);
        let sr = self.hz as i32;
        let hz = f64::from(sr);
        let srf = sr as f32;
        // The wheels, as `FUN_006bac20` keeps them.
        let low_cut = (n(0) * 2981.0 + 19.0) as i32;
        let high_cut = (n(1) * 216.0 + 5.0) as i32;
        let predelay = params[2];
        let room = (n(3) * 99.0 + 1.0) as i32;
        let diffusion = (n(4) * 100.0) as i32;
        let color = f64::from(params[5] as f32 * 0.1f32).round_ties_even() as i32;
        let decay = (n(6) * 199.0 + 1.0) as i32;
        let damping = (f64::from(n(7) * 216.0 + 5.0)) as i32;

        let c = &mut self.c;
        c.low_cut = if low_cut as f32 <= 19.0 {
            0.0
        } else {
            let f = (low_cut as f32).min(srf * 0.5);
            let t = (f64::from(f) * PI / hz).tan() as f32;
            (t - 1.0) / (t + 1.0)
        };
        c.high_cut = one_pole(high_cut, hz);
        self.len[12] = mul_div(i64::from(predelay), i64::from(sr), 1000).max(1) as usize;
        self.at[12] %= self.len[12];
        let scale = mul_div(i64::from(sr), i64::from(room + 15), 115);
        for (k, line) in LINES.iter().enumerate() {
            self.len[k] = mul_div(*line, scale, 20_000).max(1) as usize;
            self.at[k] %= self.len[k];
        }
        for k in 0..7 {
            self.taps_left[k] = mul_div(TAPS_LEFT[k], scale, 20_000) as usize;
            self.taps_right[k] = mul_div(TAPS_RIGHT[k], scale, 20_000) as usize;
        }
        if room != self.room {
            self.room = room;
            for k in 0..12 {
                let at = self.start[k];
                self.memory[at..at + self.len[k]].fill(0.0);
            }
        }
        let len = |k: usize| self.len[k] as f64;
        let ln = 0.001f64.ln() as f32;
        let spread = (f64::from(diffusion) / 100.0) as f32;
        if spread == 0.0 {
            c.input_diffusion = [0.0; 2];
            c.decay_diffusion = [0.0; 2];
        } else {
            let g = |time: f64, k: f64| {
                let t = (time / hz) as f32;
                (f64::from(ln * t) / (f64::from(spread) * k)).exp() as f32
            };
            c.input_diffusion = [
                g((len(0) + len(1)) * 0.5, 0.07),
                g((len(2) + len(3)) * 0.5, 0.11),
            ];
            c.decay_diffusion = [
                g((len(4) + len(8)) * 0.5, 0.35),
                g((len(6) + len(10)) * 0.5, 0.51),
            ];
        }
        let tank = (self.len[5] + self.len[7] + self.len[9] + self.len[11]) as f32 / srf;
        let seconds = decay as f32 * 0.1f32;
        c.decay = fourth_root(f64::from((ln * tank) / seconds).exp()) as f32;
        let tilt = [0.2, 0.6, 1.0, 1.5, 3.0][color.clamp(0, 4) as usize];
        let tilted = fourth_root(f64::from((ln * tank) / (seconds * tilt)).exp()) as f32;
        let mut ratio = tilted / c.decay;
        c.shelf_gain = ((f64::from(ratio) - 1.0) * 0.5) as f32;
        if f64::from(ratio) > 1.0 {
            ratio = 1.0;
        }
        let t = (500.0 * PI / hz).tan() as f32;
        c.shelf = (t - ratio) / (t + ratio);
        c.damping = one_pole(damping, hz);
        c.dry = n(8);
        c.wet = n(9);
    }

    /// One line's sample `back` places on from where it is.
    fn tap(&self, k: usize, back: usize) -> f32 {
        let mut i = self.at[k] + back;
        if i >= self.len[k] {
            i -= self.len[k];
        }
        self.memory[self.start[k] + i]
    }

    /// Step a line back by one, as FL's lines run downwards.
    fn step(&mut self, k: usize) {
        self.at[k] = if self.at[k] == 0 {
            self.len[k] - 1
        } else {
            self.at[k] - 1
        };
    }

    /// A lattice allpass on line `k`: `v = line - g x`, `line = x + g v`.
    fn allpass(&mut self, k: usize, g: f32, x: f32) -> f32 {
        let i = self.start[k] + self.at[k];
        let v = self.memory[i] - g * x;
        self.memory[i] = x + g * v;
        self.step(k);
        v
    }

    /// Read line `k` and put `x` in its place.
    fn swap(&mut self, k: usize, x: f32) -> f32 {
        let i = self.start[k] + self.at[k];
        let out = self.memory[i];
        self.memory[i] = x;
        self.step(k);
        out
    }

    /// One half of the tank: the shelf, the damping, a decay diffuser, a
    /// delay, the second decay diffuser and the last delay, which is read
    /// after it is written.
    fn half(&mut self, side: usize, from: f32, input: f32, lines: [usize; 4]) -> f32 {
        let c = self.c;
        let [x1, y1] = &mut self.shelves[side];
        *y1 = (from - *y1) * c.shelf + *x1;
        *x1 = from;
        let shelved = from + (from + *y1) * c.shelf_gain + input;
        let damped = &mut self.damped[side];
        *damped += (shelved - *damped) * c.damping;
        let damped = *damped;
        let v = self.allpass(lines[0], c.decay_diffusion[0], damped);
        let late = self.swap(lines[1], v) * c.decay;
        let v = self.allpass(lines[2], c.decay_diffusion[1], late);
        let k = lines[3];
        let i = self.start[k] + self.at[k];
        self.memory[i] = v;
        self.step(k);
        self.memory[self.start[k] + self.at[k]] * c.decay
    }
}

impl Effect for Reeverb {
    /// FL leaves a state of a version above 1 alone.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let version = state_version(bytes, LEN)?;
        if version > 1 {
            return Err(StateError::Version { found: version });
        }
        if bytes.len() != LEN {
            return Err(StateError::Length {
                expected: LEN,
                found: bytes.len(),
            });
        }
        self.set(read_state(&bytes[4..], &RANGES)?);
        Ok(())
    }

    /// Lays the lines out again for the rate, which asks for memory and
    /// empties them. A rate that is not a number above 0 is not taken.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz >= 1.0 {
            self.hz = f64::from(hz);
            self.allocate();
            self.set(self.params);
        }
    }

    /// `Eff_Render` (`FUN_006bc150`), a frame at a time.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let c = self.c;
        let g = f64::from(c.decay);
        let gain = |k: f64| (g + (1.0 - g) * k) as f32;
        let (g6, g25, g7, g5, g2) = (gain(0.6), gain(0.25), gain(0.7), gain(0.5), gain(0.2));
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let (in_l, in_r) = (*l, *r);
            let x = (f64::from((in_l + in_r) * 0.5) + 1e-30) as f32;
            self.high_cut += (x - self.high_cut) * c.high_cut;
            let cut = self.high_cut;
            let [x1, y1] = &mut self.low_cut;
            *y1 = (cut - *y1) * c.low_cut + *x1;
            *x1 = cut;
            let mut v = if c.low_cut == 0.0 {
                cut
            } else {
                (f64::from(cut - *y1) * 0.5 + 1e-30) as f32
            };
            // All four take the first coefficient: FL works out a second
            // for the last two and does not use it.
            for k in 0..4 {
                v = self.allpass(k, c.input_diffusion[0], v);
            }
            let input = self.swap(12, v);
            let from_right = self.memory[self.start[11] + self.at[11]] * c.decay;
            let from_left = self.half(0, from_right, input, [4, 5, 6, 7]);
            self.half(1, from_left, input, [8, 9, 10, 11]);

            let (a, b) = (self.taps_left, self.taps_right);
            let wet_l = ((((self.tap(9, a[0]) - self.tap(9, a[1]) * g25)
                - self.tap(10, a[2]) * g7)
                - self.tap(11, a[3]) * g2)
                + self.tap(5, a[4]) * g6
                + self.tap(6, a[5]))
                + self.tap(7, a[6]) * g5;
            let wet_r = ((((self.tap(5, b[0]) + self.tap(5, b[1]) * g25)
                - self.tap(6, b[2]) * g7)
                - self.tap(7, b[3]) * g2)
                - self.tap(9, b[4]) * g6
                + self.tap(10, b[5]))
                + self.tap(11, b[6]) * g5;
            *l = in_l * c.dry + wet_l * c.wet;
            *r = in_r * c.dry + wet_r * c.wet;
        }
    }

    fn param_count(&self) -> usize {
        RANGES.len()
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |value| *value as f32)
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(range) = RANGES.get(index) {
            let mut params = self.params;
            params[index] = knob_value(value, *range);
            self.set(params);
        }
    }

    fn reset(&mut self) {
        self.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{corpus, presets, state};

    fn with(values: [i32; 10]) -> Reeverb {
        let mut plugin = Reeverb::default();
        plugin.set_sample_rate(44_100.0);
        let mut bytes = state(&[1]);
        bytes.extend(state(&values));
        plugin.set_state(&bytes).unwrap();
        plugin
    }

    #[test]
    fn the_lines_scale_with_the_rate_and_the_room() {
        // The largest room at 20 kHz is the table.
        let mut plugin = Reeverb::default();
        plugin.set_sample_rate(20_000.0);
        plugin.set_param(3, 65536.0);
        assert_eq!(&plugin.len[..12], &LINES.map(|v| v as usize));
        // Room 1 of 100 is 16 / 115 of it, at 44.1 kHz.
        let plugin = with([0, 0, 0, 0, 0, 20, 0, 0, 65536, 65536]);
        let scale = 44_100 * 16 / 115;
        assert_eq!(plugin.len[5], (4453 * scale / 20_000) as usize);
        // The predelay is in ms.
        let plugin = with([0, 0, 100, 0, 0, 20, 0, 0, 65536, 65536]);
        assert_eq!(plugin.len[12], 4410);
    }

    #[test]
    fn the_decay_is_60_db_over_the_decay_time_round_the_tank() {
        let plugin = with(DEFAULTS);
        // Decay 4700 is round(4700 / 65536 * 199 + 1) = 15, 1.5 s.
        let tank: usize = [5, 7, 9, 11].iter().map(|k| plugin.len[*k]).sum();
        let want = 0.001f64.powf(tank as f64 / 44_100.0 / 1.5).powf(0.25);
        assert!((f64::from(plugin.c.decay) - want).abs() < 1e-5);
        // Colour 20 is the middle: no shelf.
        assert_eq!(plugin.c.shelf_gain, 0.0);
    }

    #[test]
    fn the_cuts_open_at_their_tops() {
        let plugin = with([0, 65536, 0, 26000, 65536, 20, 4700, 65536, 65536, 13120]);
        assert_eq!(
            (plugin.c.low_cut, plugin.c.high_cut, plugin.c.damping),
            (0.0, 1.0, 1.0)
        );
        // A 1 kHz high cut: b = 2 - cos(w), 1 - (b - sqrt(b^2 - 1)).
        let w = 2.0 * PI * 1000.0 / 44_100.0;
        let b = 2.0 - w.cos();
        let want = 1.0 - (b - (b * b - 1.0).sqrt());
        assert!((f64::from(one_pole(10, 44_100.0)) - want).abs() < 1e-6);
    }

    #[test]
    fn dry_alone_passes_the_input_and_the_tail_dies_away() {
        let mut plugin = with([0, 65536, 0, 26000, 65536, 20, 4700, 65536, 65536, 0]);
        let (mut l, mut r) = (vec![0.25; 64], vec![-0.5; 64]);
        plugin.process(&mut l, &mut r);
        assert!(l.iter().all(|v| *v == 0.25) && r.iter().all(|v| *v == -0.5));
        // Wet alone: an impulse rings and then decays.
        let mut plugin = with([0, 65536, 0, 26000, 65536, 20, 4700, 65536, 0, 65536]);
        let len = 44_100 * 4;
        let (mut l, mut r) = (vec![0.0; len], vec![0.0; len]);
        l[0] = 1.0;
        for (a, b) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
            plugin.process(a, b);
        }
        let energy = |s: &[f32]| s.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>();
        let early = energy(&l[..44_100]) + energy(&r[..44_100]);
        let late = energy(&l[3 * 44_100..]) + energy(&r[3 * 44_100..]);
        assert!(early > 1e-3, "{early}");
        assert!(late < early * 1e-4, "{early} {late}");
        assert!(l.iter().chain(&r).all(|v| v.is_finite()));
    }

    #[test]
    fn every_state_fl_ships_loads_and_runs() {
        let Some(presets) = presets(NAME) else {
            return;
        };
        assert!(presets.len() >= 20);
        for preset in presets {
            let mut plugin = Reeverb::default();
            plugin.set_sample_rate(48_000.0);
            plugin.set_state(&preset.state).unwrap();
            let (mut l, mut r) = (vec![0.5; 2048], vec![0.5; 2048]);
            plugin.process(&mut l, &mut r);
            assert!(l.iter().chain(&r).all(|v| v.is_finite() && v.abs() < 10.0));
        }
        assert!(corpus().is_some());
    }
}
