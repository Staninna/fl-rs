//! Fruity Bass Boost: the input plus a resonant low-pass of it, as
//! `docs/plugins/fruity-bass-boost.md` has it.
//!
//! The state is 12 bytes: a little-endian i32 version, 1, and the two
//! parameters as i32. FL leaves a state alone whose version is 2 or more,
//! and so does this.
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Frequency | 0 to 1024 | 512 |
//! | 1 Amount | 0 to 1024 | 768 |
//!
//! With `v` a wheel over its end: the frequency is `round(80 v + 20)` Hz
//! and the amount `(11^v - 1) / 2`, 0 to 5. A side runs `speed += (x -
//! level) k; level += speed; speed *= d` in f64, a second-order low-pass
//! with `k = 2 - 2 cos w` and `d` that damps it critically, and puts out
//! `x + level * amount`.

use crate::{Effect, StateError, knob_value, state_version};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Bass Boost";

const COUNT: usize = 2;

/// The range of each parameter's wheel.
const RANGES: [(i32, i32); COUNT] = [(0, 1024), (0, 1024)];
const DEFAULTS: [i32; COUNT] = [512, 768];

/// How long the state is, and the newest version FL reads.
const LEN: usize = 12;
const VERSION: i32 = 1;

/// What a wheel's place is, 0 to 1: its value times FL's f64 scale.
fn place(index: usize, value: i32) -> f32 {
    let v = f64::from(value) * (1.0 / f64::from(RANGES[index].1));
    v.clamp(0.0, 1.0) as f32
}

/// Delphi's `IntPower` of a single to the third: the base squared in
/// f64, times the base, rounded to f32.
fn cube(x: f32) -> f32 {
    let x = f64::from(x);
    (x * (x * x)) as f32
}

/// Fruity Bass Boost. A new one has FL's defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct BassBoost {
    params: [i32; COUNT],
    hz: i32,
    rate: i32,
    /// The low-pass's `k`, its damping, and the amount.
    k: f32,
    damp: f32,
    amount: f32,
    /// Each side's level and speed.
    state: [[f64; 2]; 2],
}

impl Default for BassBoost {
    fn default() -> Self {
        let mut plugin = BassBoost {
            params: DEFAULTS,
            hz: 0,
            rate: 44_100,
            k: 0.0,
            damp: 0.0,
            amount: 0.0,
            state: [[0.0; 2]; 2],
        };
        for (index, value) in DEFAULTS.into_iter().enumerate() {
            plugin.set(index, value);
        }
        plugin
    }
}

impl BassBoost {
    /// One wheel (`FUN_006c8810`).
    fn set(&mut self, index: usize, value: i32) {
        self.params[index] = value;
        let v = place(index, value);
        if index == 0 {
            self.hz = f64::from(v * 80.0 + 20.0).round_ties_even() as i32;
            let w = f64::from(self.hz) * std::f64::consts::TAU / f64::from(self.rate);
            let c = w.cos() as f32;
            self.k = 2.0 - c * 2.0;
            let half = f64::from((c - 1.0) * 0.5);
            let root = f64::from(-cube(c - 1.0)).sqrt();
            self.damp = ((2f64.sqrt() * root + half) / half) as f32;
        } else {
            let rise = (f64::from(v) * 11f64.ln()).exp();
            self.amount = (rise - 1.0) as f32 * 0.5;
        }
    }
}

impl Effect for BassBoost {
    /// The filter's memory stays, as in FL.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let version = state_version(bytes, LEN)?;
        if version > VERSION {
            return Err(StateError::Version { found: version });
        }
        if bytes.len() != LEN {
            return Err(StateError::Length {
                expected: LEN,
                found: bytes.len(),
            });
        }
        let mut params = [0; COUNT];
        for (param, word) in bytes[4..].as_chunks::<4>().0.iter().enumerate() {
            let value = i32::from_le_bytes(*word);
            let (min, max) = RANGES[param];
            if value < min || value > max {
                return Err(StateError::Range { param, value });
            }
            params[param] = value;
        }
        for (index, value) in params.into_iter().enumerate() {
            self.set(index, value);
        }
        Ok(())
    }

    /// FL counts the rate in whole Hz. FL's own plugin keeps the
    /// coefficients of the rate its wheel was last moved at; this one
    /// works them out again.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz >= 1.0 {
            self.rate = hz.round() as i32;
            self.set(0, self.params[0]);
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let (k, damp, amount) = (
            f64::from(self.k),
            f64::from(self.damp),
            f64::from(self.amount),
        );
        for (side, samples) in [left, right].into_iter().enumerate() {
            let [level, speed] = &mut self.state[side];
            for x in samples.iter_mut() {
                let input = f64::from(*x);
                *speed += (input - *level) * k;
                *level += *speed;
                *speed *= damp;
                *x = (input + *level * amount) as f32;
            }
        }
    }

    fn param_count(&self) -> usize {
        COUNT
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |value| *value as f32)
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(range) = RANGES.get(index) {
            self.set(index, knob_value(value, *range));
        }
    }

    /// FL's reset: the levels and speeds to 0.
    fn reset(&mut self) {
        self.state = [[0.0; 2]; 2];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{every_preset, partial, presets, sine, state};

    fn bytes(params: [i32; COUNT]) -> Vec<u8> {
        let mut bytes = state(&[1]);
        bytes.extend(state(&params));
        bytes
    }

    fn with(params: [i32; COUNT]) -> BassBoost {
        let mut plugin = BassBoost::default();
        plugin.set_state(&bytes(params)).unwrap();
        plugin
    }

    #[test]
    fn the_wheels_come_to_the_doc_s_numbers() {
        let p = BassBoost::default();
        assert_eq!(p.hz, 60);
        let c = (60.0 * std::f64::consts::TAU / 44_100.0).cos();
        assert!((f64::from(p.k) - (2.0 - 2.0 * c)).abs() < 1e-6);
        // (11^0.75 - 1) / 2.
        assert!((f64::from(p.amount) - (11f64.powf(0.75) - 1.0) / 2.0).abs() < 1e-5);
        let ends = with([1024, 1024]);
        assert_eq!(ends.hz, 100);
        assert!((ends.amount - 5.0).abs() < 1e-5);
        let low = with([0, 0]);
        assert_eq!((low.hz, low.amount), (20, 0.0));
        // The damping is under 1, and nearer 1 the lower the frequency.
        assert!(low.damp < 1.0 && ends.damp < low.damp && ends.damp > 0.9);
        assert_eq!(cube(-0.5), -0.125);
    }

    /// The level after `n` frames of a tone, in f64 as the doc has it.
    fn model(x: &[f32], p: &BassBoost) -> Vec<f64> {
        let (k, d, a) = (f64::from(p.k), f64::from(p.damp), f64::from(p.amount));
        let (mut level, mut speed) = (0.0, 0.0);
        x.iter()
            .map(|x| {
                let x = f64::from(*x);
                speed += (x - level) * k;
                level += speed;
                speed *= d;
                x + level * a
            })
            .collect()
    }

    #[test]
    fn it_adds_its_low_pass_to_the_input() {
        let mut plugin = with([300, 900]);
        let tone = sine(0.001, 20_000);
        let wanted = model(&tone, &plugin);
        let (mut l, mut r) = (tone.clone(), tone.clone());
        plugin.process(&mut l, &mut r);
        for (got, want) in l.iter().zip(&wanted) {
            assert!((f64::from(*got) - want).abs() < 1e-6);
        }
        assert_eq!(l, r);
    }

    #[test]
    fn low_tones_come_up_by_the_amount_and_high_ones_stay() {
        let mut plugin = with([512, 1024]);
        // A level that stays comes out times 1 + 5.
        let (mut l, mut r) = (vec![0.1; 40_000], vec![-0.1; 40_000]);
        plugin.process(&mut l, &mut r);
        assert!((l[39_999] - 0.6).abs() < 1e-4 && (r[39_999] + 0.6).abs() < 1e-4);
        // Far above the corner nothing is added.
        plugin.reset();
        let tone = sine(0.1, 20_000);
        let (mut l, mut r) = (tone.clone(), tone.clone());
        plugin.process(&mut l, &mut r);
        assert!((partial(&l[10_000..], 0.1) - 1.0).abs() < 1e-3);
        // The low-pass is critically damped: at the corner it is half the
        // level a quarter turn late, so the sum is |1 - 2.5j|.
        plugin.reset();
        // 735 samples a cycle: fifteen whole ones at the end.
        let turns = 60.0 / 44_100.0;
        let tone = sine(turns, 48_510);
        let (mut l, mut r) = (tone.clone(), tone.clone());
        plugin.process(&mut l, &mut r);
        let level = partial(&l[37_485..], turns);
        assert!((level - 7.25f64.sqrt()).abs() < 0.1, "{level}");
    }

    #[test]
    fn the_rate_moves_the_corner_and_reset_empties_the_filter() {
        let mut plugin = BassBoost::default();
        let slow = plugin.k;
        plugin.set_sample_rate(88_200.0);
        assert!((plugin.k * 4.0 - slow).abs() < 1e-5);
        let before = plugin.clone();
        plugin.set_sample_rate(f32::NAN);
        assert_eq!(plugin, before);
        plugin.process(&mut [0.3; 100], &mut [0.3; 100]);
        assert_ne!(plugin.state, [[0.0; 2]; 2]);
        plugin.reset();
        assert_eq!(plugin.state, [[0.0; 2]; 2]);
    }

    #[test]
    fn a_moved_wheel_is_the_same_plugin_as_its_state() {
        let mut plugin = BassBoost::default();
        plugin.set_param(0, 836.4);
        plugin.set_param(1, 2000.0);
        plugin.set_param(2, 5.0);
        assert_eq!(
            (plugin.param(0), plugin.param(1), plugin.param(2)),
            (836.0, 1024.0, 0.0)
        );
        let same = with([836, 1024]);
        assert_eq!(
            (plugin.k, plugin.damp, plugin.amount),
            (same.k, same.damp, same.amount)
        );
        assert_eq!(plugin.param_count(), 2);
    }

    #[test]
    fn a_state_the_plugin_does_not_write_is_turned_down() {
        let mut plugin = with([1024, 836]);
        let before = plugin.clone();
        assert_eq!(
            plugin.set_state(&bytes([1025, 0])),
            Err(StateError::Range {
                param: 0,
                value: 1025
            })
        );
        let mut long = bytes(DEFAULTS);
        long.push(0);
        assert_eq!(
            plugin.set_state(&long),
            Err(StateError::Length {
                expected: 12,
                found: 13
            })
        );
        let mut newer = bytes(DEFAULTS);
        newer[0] = 2;
        assert_eq!(
            plugin.set_state(&newer),
            Err(StateError::Version { found: 2 })
        );
        assert_eq!(plugin, before);
    }

    #[test]
    fn fl_s_presets_load_and_play() {
        let Some(own) = presets(NAME) else { return };
        let all = every_preset(&crate::testing::corpus().unwrap(), NAME);
        assert!(!own.is_empty());
        for preset in &all {
            let mut plugin = BassBoost::default();
            assert_eq!(plugin.set_state(&preset.state), Ok(()), "{}", preset.file);
            let (mut l, mut r) = (sine(0.01, 4000), sine(0.002, 4000));
            plugin.process(&mut l, &mut r);
            assert!(
                l.iter().chain(&r).all(|v| v.is_finite() && v.abs() < 50.0),
                "{}",
                preset.file
            );
        }
    }
}
