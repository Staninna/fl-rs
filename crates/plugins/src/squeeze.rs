//! Fruity Squeeze: a bit crusher, a gate that pumps between full level and
//! an amount, and a state-variable filter before or after them, as
//! `docs/plugins/fruity-squeeze.md` has it.
//!
//! The state is a version word FL does not read and twelve little-endian
//! i32:
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Gain | 0 to 160 | 127 |
//! | 1 Mix | 0 to 128 | 64 |
//! | 2 Steps | 1 to 32768 | 1 |
//! | 3 Filter | 0, 1 | 0 |
//! | 4 Filter type | 0 to 4 | 0 |
//! | 5 Filter resonance | 0 to 65536 | 32768 |
//! | 6 Filter cutoff | 0 to 65536 | 32768 |
//! | 7 Filter position | 0 (before), 1 (after) | 0 |
//! | 8 Relation | 0 to 3 | 0 |
//! | 9 Impact | 0 to 44100 | 0 |
//! | 10 Preserve | 0 to 44100 | 7500 |
//! | 11 Amount | 0 to 65536 | 0 |

use crate::{Effect, StateError, knob_value, read_state};
use std::f64::consts::PI;

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Squeeze";

const RANGES: [(i32, i32); 12] = [
    (0, 160),
    (0, 128),
    (1, 32768),
    (0, 1),
    (0, 4),
    (0, 65536),
    (0, 65536),
    (0, 1),
    (0, 3),
    (0, 44100),
    (0, 44100),
    (0, 65536),
];
const DEFAULTS: [i32; 12] = [127, 64, 1, 0, 0, 32768, 32768, 0, 0, 0, 7500, 0];
/// The version word and the twelve parameters.
const LEN: usize = 52;
/// What FL's `FUN_005de7c0` flushes to 0: magnitudes under 2^-24.
const TINY: f32 = 5.960_464_5e-8;
/// Under this, in magnitude, every sample of a block counts as silence and
/// the block passes untouched (`0x006d4518`).
const SILENT: f32 = 1e-6;

/// Delphi's `Power` on singles: `IntPower` for a whole exponent, else
/// `exp(e ln b)`.
fn power(base: f32, exponent: f32) -> f32 {
    if exponent == 0.0 {
        return 1.0;
    }
    if base == 0.0 && exponent > 0.0 {
        return 0.0;
    }
    let whole = (f64::from(exponent) + 0.5).floor() as f32;
    if whole == exponent {
        f64::from(base).powi(whole as i32) as f32
    } else {
        (f64::from(exponent) * f64::from(base).ln()).exp() as f32
    }
}

/// Delphi's `Round`, to the nearest and ties to even.
fn round(x: f64) -> i64 {
    x.round_ties_even() as i64
}

/// FL's `TStateVariableFilter`: Chamberlin's, run twice a sample.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Svf {
    /// `f = 2 sin(pi fc / 2 rate)` and the damping.
    f: f32,
    q: f32,
    /// `in - q bp`, low, high and band.
    notch: f32,
    low: f32,
    high: f32,
    band: f32,
}

impl Svf {
    /// Work out the coefficients (`FUN_006d09a0`) for a cutoff wheel `n`
    /// and a resonance wheel `r`, both 0 to 1.
    fn tune(&mut self, n: f32, r: f32, hz: f32) {
        // The range object: 50 Hz to 0.75 of the rate, in logs.
        let (low, high) = (50.0f32, (f64::from(hz) * 0.75) as f32);
        let (ln_low, ln_high) = (f64::from(low).ln() as f32, f64::from(high).ln() as f32);
        let at = n * (ln_high - ln_low) + ln_low;
        let fc = (f64::from(at).exp() as f32).clamp(low, high);
        let x = ((f64::from(fc) / (f64::from(hz) * 2.0)) as f32).min(0.25);
        self.f = ((f64::from(x) * PI).sin() * 2.0) as f32;
        let damped = ((1.0 - f64::from(power(r, 0.25))) * 2.0) as f32;
        let most = ((2.0 / f64::from(self.f) - f64::from(self.f) * 0.5) as f32).min(2.0);
        self.q = most.min(damped);
    }

    /// One pass.
    fn step(&mut self, x: f32) {
        self.notch = flush(x - self.q * self.band);
        self.low += self.f * self.band;
        self.high = self.notch - self.low;
        self.band += self.f * self.high;
    }

    /// The output of this type after one pass.
    fn out(&self, kind: i32) -> f32 {
        match kind {
            1 => self.high,
            2 => self.band,
            3 => self.notch,
            4 => self.low - self.high,
            _ => self.low,
        }
    }

    /// A sample through both passes, the two outputs averaged.
    fn run(&mut self, x: f32, kind: i32) -> f32 {
        let x = flush(x);
        self.step(x);
        let first = (f64::from(self.out(kind)) * 0.5) as f32;
        self.step(x);
        (f64::from(first) + f64::from(self.out(kind)) * 0.5) as f32
    }
}

/// FL's denormal guard: a magnitude under 2^-24 becomes 0.
fn flush(x: f32) -> f32 {
    if x.abs() < TINY { 0.0 } else { x }
}

/// Fruity Squeeze. A new one has FL's defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct Squeeze {
    params: [i32; 12],
    hz: f32,
    gain: f32,
    dry: f32,
    wet: f32,
    /// The crusher's steps; 32768 and over is off.
    steps: f32,
    /// The gate: how long it holds the amount and full level, in frames.
    impact: f32,
    preserve: f32,
    /// Where the gate is: holding the amount, and the two counters.
    holding: bool,
    held: f32,
    kept: f32,
    filters: [Svf; 2],
}

impl Default for Squeeze {
    fn default() -> Self {
        let mut plugin = Squeeze {
            params: DEFAULTS,
            hz: 44_100.0,
            gain: 0.0,
            dry: 0.0,
            wet: 0.0,
            steps: 0.0,
            impact: 0.0,
            preserve: 0.0,
            holding: false,
            held: 0.0,
            kept: 0.0,
            filters: [Svf::default(); 2],
        };
        plugin.set(DEFAULTS, true);
        plugin
    }
}

impl Squeeze {
    /// Take parameters that are inside their ranges. With `relate`, a
    /// relation other than 0 sets the impact from the preserve wheel, as
    /// FL does when the relation or the preserve wheel moves and when a
    /// state loads (`FUN_006d4580`).
    fn set(&mut self, mut params: [i32; 12], relate: bool) {
        if relate {
            params[9] = match params[8] {
                1 => params[10],
                2 => params[10] * 2,
                3 => params[10] / 2,
                _ => params[9],
            };
        }
        self.params = params;
        self.gain = (f64::from(params[0]) * (1.0 / 127.0)) as f32;
        self.dry = (128 - params[1]) as f32 / 128.0;
        self.wet = params[1] as f32 / 128.0;
        let n = (f64::from(params[2]) / 32768.0) as f32;
        self.steps = power(32768.0, (1.0 - f64::from(n)) as f32);
        let scale = (f64::from(self.hz) / 44_100.0) as f32;
        let time = |v: i32| {
            let n = (f64::from(v) / 44_100.0) as f32;
            (power(44_101.0, n) - 1.0) * scale
        };
        self.impact = time(params[9]);
        self.preserve = time(params[10]);
        let r = (f64::from(params[5]) / 65_536.0) as f32;
        let c = (f64::from(params[6]) / 65_536.0) as f32;
        for filter in &mut self.filters {
            filter.tune(c, r, self.hz);
        }
    }

    fn filter(&mut self, left: &mut [f32], right: &mut [f32]) {
        let kind = self.params[4];
        for (filter, side) in self.filters.iter_mut().zip([left, right]) {
            for x in side.iter_mut() {
                *x = filter.run(*x, kind);
            }
        }
    }

    /// The gain of the gate for the next frame (the loop in
    /// `FUN_006d40c0`).
    fn gate(&mut self) -> f32 {
        let (mut on, mut off) = (0.0f32, 1.0f32);
        if !self.holding {
            if self.kept <= self.preserve {
                self.kept += 1.0;
            } else {
                self.holding = true;
                on = self.kept - self.preserve;
                off = (1.0 - f64::from(on)) as f32;
                self.held = on;
                self.kept = 0.0;
            }
        } else if self.held <= self.impact {
            self.held += 1.0;
            on = 1.0;
            off = 0.0;
        } else {
            self.holding = false;
            off = self.held - self.impact;
            on = (1.0 - f64::from(off)) as f32;
            self.held = 0.0;
            self.kept = off;
        }
        let amount = (f64::from(self.params[11]) * (1.0 / 65_536.0) * f64::from(on)) as f32;
        amount + off
    }
}

impl Effect for Squeeze {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        if bytes.len() != LEN {
            return Err(StateError::Length {
                expected: LEN,
                found: bytes.len(),
            });
        }
        let params = read_state(&bytes[4..], &RANGES).map_err(|error| match error {
            StateError::Length { found, .. } => StateError::Length {
                expected: LEN,
                found: found + 4,
            },
            other => other,
        })?;
        self.set(params, true);
        Ok(())
    }

    /// A rate that is not a number above 0 is not taken.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz > 0.0 {
            self.hz = hz;
            self.set(self.params, false);
        }
    }

    /// A block that is silent throughout passes as it is, and the filters
    /// and the gate stand still for it, as in FL. Otherwise: the filter if
    /// it comes first, the crusher, the gate, the filter if it comes after,
    /// and the mix of the input and that at the gain.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        let (left, right) = (&mut left[..len], &mut right[..len]);
        let loud = |x: &f32| x.abs().to_bits() > SILENT.to_bits();
        if !left.iter().chain(right.iter()).any(loud) {
            return;
        }
        let filtered = self.params[3] != 0;
        let after = self.params[7] == 1;
        // The mix needs the input, so the work goes in a copy on the
        // stack, a chunk at a time.
        for start in (0..len).step_by(CHUNK) {
            let end = (start + CHUNK).min(len);
            let mut work = [[0.0f32; CHUNK]; 2];
            let n = end - start;
            work[0][..n].copy_from_slice(&left[start..end]);
            work[1][..n].copy_from_slice(&right[start..end]);
            let [wl, wr] = &mut work;
            let (wl, wr) = (&mut wl[..n], &mut wr[..n]);
            if filtered && !after {
                self.filter(wl, wr);
            }
            if self.steps < 32768.0 {
                for x in wl.iter_mut().chain(wr.iter_mut()) {
                    let level = round(f64::from(*x * self.steps)) as f32;
                    *x = level / self.steps;
                }
            }
            if self.impact > 0.0 {
                for (l, r) in wl.iter_mut().zip(wr.iter_mut()) {
                    let g = self.gate();
                    *l *= g;
                    *r *= g;
                }
            }
            if filtered && after {
                self.filter(wl, wr);
            }
            for (k, (l, r)) in left[start..end]
                .iter_mut()
                .zip(&mut right[start..end])
                .enumerate()
            {
                *l = (*l * self.dry + wl[k] * self.wet) * self.gain;
                *r = (*r * self.dry + wr[k] * self.wet) * self.gain;
            }
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
            self.set(params, index == 8 || index == 10);
        }
    }

    /// FL's flush starts the gate over. The filters keep what they hold.
    fn reset(&mut self) {
        self.holding = false;
        self.held = 0.0;
        self.kept = 0.0;
    }
}

/// How many frames are worked on at a time, on the stack.
const CHUNK: usize = 256;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{presets, state};

    fn with(values: [i32; 12]) -> Squeeze {
        let mut plugin = Squeeze::default();
        plugin.set_sample_rate(44_100.0);
        let mut bytes = state(&[0]);
        bytes.extend(state(&values));
        plugin.set_state(&bytes).unwrap();
        plugin
    }

    #[test]
    fn all_wet_at_127_with_one_step_is_the_input_to_15_bits() {
        // The steps wheel at its bottom still crushes, to 32768^(1 - 1/32768).
        let mut plugin = with([127, 128, 1, 0, 0, 32768, 32768, 0, 0, 0, 7500, 0]);
        assert!((plugin.steps - 32_757.6).abs() < 0.1);
        let input = [0.5f32, -0.25, 0.125];
        let (mut l, mut r) = (input, input);
        plugin.process(&mut l, &mut r);
        for (out, x) in l.iter().zip(input) {
            assert!((out - x).abs() <= 0.5 / 32_757.0 + 1e-7);
        }
        // Mix at 0 is the input alone, at the gain.
        let mut plugin = with([127, 0, 32768, 1, 0, 32768, 32768, 0, 0, 0, 7500, 0]);
        let (mut l, mut r) = (input, input);
        plugin.process(&mut l, &mut r);
        assert_eq!(l, input);
    }

    #[test]
    fn the_crusher_rounds_to_its_steps() {
        // Steps wheel at 16384 of 32768: 32768^0.5 = 181 steps.
        let mut plugin = with([127, 128, 16384, 0, 0, 32768, 32768, 0, 0, 0, 7500, 0]);
        let s = 32768f32.sqrt();
        assert!((plugin.steps - s).abs() < 1e-3);
        let (mut l, mut r) = ([0.3f32, -0.7], [0.01f32, 0.5]);
        plugin.process(&mut l, &mut r);
        let q = |x: f32| (x * plugin.steps).round_ties_even() / plugin.steps;
        assert_eq!(l, [q(0.3), q(-0.7)]);
        assert_eq!(r, [q(0.01), q(0.5)]);
    }

    #[test]
    fn the_gate_holds_the_amount_for_the_impact_and_full_level_for_the_preserve() {
        // Impact 1 and preserve 2 frames: (44101^(v/44100) - 1) at 44.1 kHz.
        let v = |frames: f64| ((frames + 1.0).ln() / 44_101f64.ln() * 44_100.0).round() as i32;
        let mut plugin = with([127, 128, 1, 0, 0, 32768, 32768, 0, 0, v(3.0), v(3.0), 0]);
        assert!((plugin.impact - 3.0).abs() < 0.01 && (plugin.preserve - 3.0).abs() < 0.01);
        let (mut l, mut r) = (vec![1.0; 40], vec![1.0; 40]);
        plugin.process(&mut l, &mut r);
        // Full level, then a crossfade frame, then silence, and round.
        assert!(l[..3].iter().all(|v| *v > 0.999));
        assert!(l.contains(&0.0));
        assert!(l.iter().all(|v| (0.0..=1.001).contains(v)));
        // A relation of 2 makes the impact twice the preserve.
        let plugin = with([127, 128, 1, 0, 0, 32768, 32768, 0, 2, 0, 100, 0]);
        assert_eq!(plugin.param(9), 200.0);
        let quiet = l.iter().filter(|v| **v < 0.5).count();
        assert!((15..=25).contains(&quiet), "{quiet}");
    }

    #[test]
    fn the_low_pass_passes_a_low_tone_and_cuts_a_high_one() {
        let tone = |turns: f64| -> Vec<f32> {
            (0..8192)
                .map(|n| (std::f64::consts::TAU * turns * n as f64).sin() as f32 * 0.5)
                .collect()
        };
        let level = |s: &[f32]| s[4096..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let run = |turns: f64| {
            // A cutoff of about 1 kHz, low resonance.
            let mut plugin = with([127, 128, 1, 1, 0, 0, 34000, 0, 0, 0, 7500, 0]);
            let (mut l, mut r) = (tone(turns), tone(turns));
            plugin.process(&mut l, &mut r);
            level(&l)
        };
        assert!(run(100.0 / 44_100.0) > 0.4);
        assert!(run(10_000.0 / 44_100.0) < 0.05);
    }

    #[test]
    fn silence_passes_untouched() {
        let mut plugin = with([160, 0, 32768, 1, 0, 32768, 32768, 0, 0, 0, 7500, 0]);
        let (mut l, mut r) = ([1e-7f32, -1e-7], [0.0f32, 0.0]);
        plugin.process(&mut l, &mut r);
        assert_eq!(l, [1e-7, -1e-7]);
    }

    #[test]
    fn every_preset_loads_and_runs() {
        let Some(presets) = presets(NAME) else {
            return;
        };
        assert_eq!(presets.len(), 6);
        for preset in presets {
            let mut plugin = Squeeze::default();
            plugin.set_sample_rate(48_000.0);
            plugin.set_state(&preset.state).unwrap();
            let (mut l, mut r) = (vec![0.5; 4096], vec![-0.5; 4096]);
            plugin.process(&mut l, &mut r);
            assert!(l.iter().chain(&r).all(|v| v.is_finite() && v.abs() < 4.0));
        }
    }
}
