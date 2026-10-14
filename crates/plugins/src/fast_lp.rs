//! Fruity Fast LP: a low pass of one multiply and add a sample, with a
//! resonance that makes it two poles, as `docs/plugins/fruity-fast-lp.md`
//! has it.
//!
//! The state is three little-endian i32: a version, which is 1, and the
//! plugin's two parameters:
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Cutoff | 0 to 1024 | 1024 |
//! | 1 Resonance | 0 to 1024 | 0 |
//!
//! The wheels make two coefficients, [`coefficients`], and the filter is
//!
//! ```text
//! v = Q * v + F * (x - y)
//! y = y + v
//! ```
//!
//! At the defaults `F` is 1 and `Q` is 0, which is the input as it is.
//! Neither coefficient looks at the rate, so the same wheel is a higher
//! corner at a higher rate, as in FL.

use crate::parts::{Follower, slew};
use crate::{Effect, StateError, knob_value, read_state, state_version};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Fast LP";

/// The range of each parameter's wheel.
const RANGES: [(i32, i32); 2] = [(0, 1024), (0, 1024)];
const DEFAULTS: [i32; 2] = [1024, 0];

/// How long the state is: the version and the two parameters.
const LEN: usize = 12;

/// `F` and `Q` for a cutoff and a resonance.
///
/// `F` is `(601^x - 1) / 600` with `x` from 1/64 at cutoff 0 to 1 at 1024,
/// which is 0.000175 to 1. `Q` is that curve backwards over five eighths of
/// the wheel, from 0 to 0.983.
pub fn coefficients(cutoff: i32, resonance: i32) -> (f32, f32) {
    let curve = |x: f64| (601f64.powf(x) - 1.0) / 600.0;
    let c = (63.0 * f64::from(cutoff) / 1024.0 + 1.0) / 64.0;
    let r = f64::from(resonance) / 1024.0 * 0.625;
    let f = if c >= 1.0 { 1.0 } else { curve(c) };
    let q = if r <= 0.0 { 0.0 } else { 1.0 - curve(1.0 - r) };
    (f as f32, q as f32)
}

/// Fruity Fast LP. A new one has FL's defaults, which pass a block through
/// untouched.
#[derive(Debug, Clone, PartialEq)]
pub struct FastLp {
    params: [i32; 2],
    /// `F` as the cutoff asks for it.
    f: f32,
    /// `F` as the last block ended on it.
    current: f32,
    q: f32,
    /// Each side's filter.
    sides: [Follower; 2],
}

impl Default for FastLp {
    fn default() -> Self {
        let (f, q) = coefficients(DEFAULTS[0], DEFAULTS[1]);
        FastLp {
            params: DEFAULTS,
            f,
            current: f,
            q,
            sides: Default::default(),
        }
    }
}

impl FastLp {
    /// Take parameters that are inside their ranges.
    fn set(&mut self, params: [i32; 2]) {
        self.params = params;
        (self.f, self.q) = coefficients(params[0], params[1]);
    }
}

impl Effect for FastLp {
    /// `F` is the state's from the first sample on, with no slew.
    ///
    /// FL reads any version under 2, and so does this. It writes 1.
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
        self.current = self.f;
        Ok(())
    }

    /// Nothing here depends on the rate.
    fn set_sample_rate(&mut self, _hz: f32) {}

    /// After the cutoff has moved, `F` slews to where it was put: over this
    /// block when it is 128 frames or longer, and at the pace of 128 frames
    /// when it is shorter. `Q` is the resonance wheel's at once.
    ///
    /// With `F` settled at 1 and `Q` at 0 the block is left alone, and the
    /// filter takes its last frame as where it is, so that a cutoff that
    /// moves later starts from the signal.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        if len == 0 {
            return;
        }
        let (mut f, step) = slew(&mut self.current, self.f, len);
        if self.f == 1.0 && self.q == 0.0 && step == 0.0 {
            self.sides[0].y = f64::from(left[len - 1]);
            self.sides[1].y = f64::from(right[len - 1]);
            return;
        }
        let q = f64::from(self.q);
        let [left_side, right_side] = &mut self.sides;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            *l = left_side.step(f64::from(*l), f64::from(f), q) as f32;
            *r = right_side.step(f64::from(*r), f64::from(f), q) as f32;
            f += step;
        }
        left_side.flush();
        right_side.flush();
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

    /// Empties the filter and ends a slew.
    fn reset(&mut self) {
        self.sides = Default::default();
        self.current = self.f;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{bits, partial, presets, response, sine, state};
    use std::f64::consts::TAU;

    fn with(cutoff: i32, resonance: i32) -> FastLp {
        let mut plugin = FastLp::default();
        plugin.set_state(&state(&[1, cutoff, resonance])).unwrap();
        plugin
    }

    /// The doc's `F` and `Q`.
    fn by_the_doc(cutoff: i32, resonance: i32) -> (f64, f64) {
        let c = f64::from(cutoff) / 1024.0;
        let r = f64::from(resonance) / 1024.0;
        let xc = (c + 1.0 / 63.0) * 63.0 / 64.0;
        let f = if xc >= 1.0 {
            1.0
        } else {
            (601f64.powf(xc) - 1.0) / 600.0
        };
        let xr = (r * 0.625).min(0.625);
        let q = if xr <= 0.0 {
            0.0
        } else {
            1.0 - (601f64.powf(1.0 - xr) - 1.0) / 600.0
        };
        (f, q)
    }

    /// The gain the doc's transfer function has at `turns` a sample:
    /// `F / (1 - (1 + Q - F) z^-1 + Q z^-2)`.
    fn gain(cutoff: i32, resonance: i32, turns: f64) -> f64 {
        let (f, q) = by_the_doc(cutoff, resonance);
        response(&[f], &[1.0, -(1.0 + q - f), q], turns)
    }

    /// The gain the plugin has for a tone that turns `cycles` times in
    /// `window` samples, once it has settled.
    fn measured(plugin: &mut FastLp, cycles: usize, window: usize) -> f64 {
        let turns = cycles as f64 / window as f64;
        let settle = window * 40_000usize.div_ceil(window);
        let mut left = sine(turns, settle + window);
        let mut right = left.clone();
        plugin.process(&mut left, &mut right);
        assert!(left == right);
        partial(&left[settle..], turns)
    }

    fn close(got: f64, want: f64, part: f64) -> bool {
        (got - want).abs() <= part * want.abs()
    }

    #[test]
    fn the_defaults_pass_a_stereo_signal_to_the_bit() {
        assert_eq!(coefficients(1024, 0), (1.0, 0.0));
        let left = [0.0, -0.0, 1.0, -1.0, 0.3, -1e-30, 7.5e9, f32::MIN_POSITIVE];
        let right = [0.25, 1e-20, -3.0, 0.1, -0.0, 1e30, -1e-38, 0.5];
        for mut plugin in [FastLp::default(), with(1024, 0)] {
            for _ in 0..3 {
                let (mut l, mut r) = (left, right);
                plugin.process(&mut l, &mut r);
                assert_eq!(bits(&l), bits(&left));
                assert_eq!(bits(&r), bits(&right));
            }
        }
    }

    #[test]
    fn the_coefficients_are_the_doc_s_curves() {
        for cutoff in [0, 1, 137, 480, 725, 931, 1023, 1024] {
            for resonance in [0, 1, 138, 489, 1024] {
                let (f, q) = coefficients(cutoff, resonance);
                let (f_want, q_want) = by_the_doc(cutoff, resonance);
                assert!((f64::from(f) - f_want).abs() < 1e-7, "cutoff {cutoff}");
                assert!(
                    (f64::from(q) - q_want).abs() < 1e-7,
                    "resonance {resonance}"
                );
            }
        }
        // The doc's table: F and Q for what the corpus has, and the corner
        // of the one pole at 44.1 kHz, -ln(1 - F) * rate / 2 pi.
        for (cutoff, resonance, f_want, q_want, hz) in [
            (931, 0, 0.563, 0.0, 5800.0),
            (725, 0, 0.158, 0.0, 1200.0),
            (480, 0, 0.0337, 0.0, 240.0),
            (137, 138, 0.0026, 0.42, 18.0),
            (0, 0, 0.000_175, 0.0, 1.2),
        ] {
            let (f, q) = coefficients(cutoff, resonance);
            assert!(close(f64::from(f), f_want, 0.01), "cutoff {cutoff}: {f}");
            assert!((f64::from(q) - q_want).abs() < 0.005, "{resonance}: {q}");
            let corner = -(1.0 - f64::from(f)).ln() * 44_100.0 / TAU;
            assert!(close(corner, hz, 0.03), "cutoff {cutoff}: {corner}");
        }
        // Q is 0 with no resonance and 0.983 with all of it, for poles of
        // radius 0.9915.
        let (_, q) = coefficients(1024, 1024);
        assert!((q - 0.983).abs() < 5e-4 && (q.sqrt() - 0.9915).abs() < 5e-4);
        assert!((coefficients(1024, 489).1 - 0.85).abs() < 0.005);
    }

    #[test]
    fn with_no_resonance_it_is_the_one_pole() {
        let input: Vec<f32> = (0..3000)
            .map(|n| ((n * 7919) % 2001) as f32 / 1000.0 - 1.0)
            .collect();
        for cutoff in [0, 137, 480, 725, 931, 1023] {
            let (f, _) = by_the_doc(cutoff, 0);
            let (mut left, mut right) = (input.clone(), input.clone());
            right.reverse();
            with(cutoff, 0).process(&mut left, &mut right);
            // y += F * (x - y), on each side apart.
            let (mut y_left, mut y_right) = (0.0, 0.0);
            for n in 0..input.len() {
                y_left += f * (f64::from(input[n]) - y_left);
                y_right += f * (f64::from(input[input.len() - 1 - n]) - y_right);
                assert!(
                    (f64::from(left[n]) - y_left).abs() < 1e-6,
                    "cutoff {cutoff}"
                );
                assert!((f64::from(right[n]) - y_right).abs() < 1e-6);
            }
        }

        // 3 dB down at the doc's corner, which for cutoff 480 is 240 Hz at
        // 44.1 kHz, and 6 dB an octave above it.
        let (f, _) = by_the_doc(480, 0);
        let corner = (-(1.0 - f).ln() * 44_100.0 / TAU).round() as usize;
        assert_eq!(corner, 240);
        let mut plugin = with(480, 0);
        let at_corner = measured(&mut plugin, corner, 44_100);
        assert!(close(at_corner, gain(480, 0, 240.0 / 44_100.0), 1e-4));
        assert!(close(at_corner, 0.5f64.sqrt(), 0.01), "{at_corner}");
        let above = measured(&mut plugin, 16 * corner, 44_100);
        assert!(close(above, gain(480, 0, 3840.0 / 44_100.0), 1e-4));
        assert!(close(above, 1.0 / 16.0, 0.1), "{above}");
        assert!(close(measured(&mut plugin, 1, 44_100), 1.0, 1e-4));
    }

    #[test]
    fn resonance_makes_it_the_doc_s_two_poles() {
        for (cutoff, resonance) in [(137, 138), (480, 1024), (725, 489), (931, 300), (1024, 489)] {
            let mut plugin = with(cutoff, resonance);
            for (cycles, window) in [(1, 2000), (1, 300), (1, 34), (1, 12), (1, 6), (2, 5)] {
                let got = measured(&mut plugin, cycles, window);
                let want = gain(cutoff, resonance, cycles as f64 / window as f64);
                assert!(
                    (got - want).abs() < 1e-3 * want + 1e-6,
                    "{cutoff}, {resonance} at {cycles}/{window}: {got} and not {want}"
                );
            }
        }

        // The peak sits near acos((1 + Q - F) / (2 sqrt Q)) a sample.
        let (f, q) = by_the_doc(480, 1024);
        let peak = ((1.0 + q - f) / (2.0 * q.sqrt())).acos() / TAU;
        let cycles = (peak * 44_100.0).round() as usize;
        let mut plugin = with(480, 1024);
        let at_peak = measured(&mut plugin, cycles, 44_100);
        assert!(at_peak > 10.0, "{at_peak}");
        assert!(at_peak > measured(&mut plugin, cycles * 9 / 10, 44_100));
        assert!(at_peak > measured(&mut plugin, cycles * 11 / 10, 44_100));

        // The doc reads cutoff 1024 with resonance 489 as close to a
        // bypass. Its own filter is not: F = 1 leaves
        // 1 / (1 - Q z^-1 + Q z^-2), which at a sixth of the rate is
        // 1 / (1 - Q), a peak of 16 dB.
        let (_, q) = by_the_doc(1024, 489);
        let ringing = measured(&mut with(1024, 489), 1, 6);
        assert!(close(ringing, 1.0 / (1.0 - q), 1e-3) && ringing > 6.0);
    }

    #[test]
    fn the_rate_does_not_move_the_coefficients() {
        let mut heard = Vec::new();
        for hz in [44_100.0, 96_000.0] {
            let mut plugin = with(480, 300);
            plugin.set_sample_rate(hz);
            let (mut left, mut right) = (sine(0.01, 600), sine(0.2, 600));
            plugin.process(&mut left, &mut right);
            heard.push((left, right));
        }
        assert!(heard[0] == heard[1]);
        // So the corner in Hz goes with the rate: the same 240 turns in
        // 44100 samples are 522 Hz at 96 kHz.
        assert!(close(240.0 / 44_100.0 * 96_000.0, 522.0, 0.001));
    }

    #[test]
    fn a_moved_cutoff_slews_over_128_samples() {
        let mut plugin = FastLp::default();
        let (low, _) = coefficients(0, 0);
        plugin.set_param(0, 0.0);
        assert_eq!((plugin.f, plugin.current), (low, 1.0));
        // A block of 32 goes a quarter of the way, and the next one a
        // quarter of what is left.
        plugin.process(&mut [0.0; 32], &mut [0.0; 32]);
        assert!((plugin.current - (1.0 + (low - 1.0) * 0.25)).abs() < 1e-6);
        plugin.process(&mut [0.0; 32], &mut [0.0; 32]);
        assert!((plugin.current - (1.0 + (low - 1.0) * 0.4375)).abs() < 1e-6);
        plugin.process(&mut [0.0; 128], &mut [0.0; 128]);
        assert_eq!(plugin.current, low);

        // On the way down the first frame still has F at 1 and passes, and
        // frame n has it at 1 - n (1 - low) / 128.
        let mut plugin = FastLp::default();
        plugin.set_param(0, 0.0);
        let mut left = [1.0, -1.0, 1.0, -1.0];
        let mut right = left;
        plugin.process(&mut left, &mut right);
        let mut y = 0.0;
        for (n, got) in left.iter().enumerate() {
            let f = 1.0 - n as f64 * (1.0 - f64::from(low)) / 128.0;
            y += f * (if n % 2 == 0 { 1.0 } else { -1.0 } - y);
            assert!((f64::from(*got) - y).abs() < 1e-6, "frame {n}");
        }
        assert_eq!(left[0], 1.0);
        assert_eq!(left, right);

        // Back at the top, the bypass waits for the slew to end.
        let mut plugin = with(0, 0);
        plugin.set_param(0, 1024.0);
        let (mut left, mut right) = ([1.0; 64], [1.0; 64]);
        plugin.process(&mut left, &mut right);
        assert!(left[0] < 0.01 && left[5] < 0.5);
        // A state does not slew.
        plugin.set_state(&state(&[1, 480, 0])).unwrap();
        assert_eq!(plugin.current, plugin.f);
    }

    #[test]
    fn the_filter_starts_from_where_the_bypass_left_the_signal() {
        let mut plugin = FastLp::default();
        plugin.process(&mut [0.1, 0.7], &mut [0.2, -0.4]);
        plugin.set_param(0, 480.0);
        let (mut left, mut right) = ([0.7; 200], [-0.4; 200]);
        plugin.process(&mut left, &mut right);
        assert_eq!((left, right), ([0.7; 200], [-0.4; 200]));
        // A reset starts it from nothing.
        plugin.reset();
        let (mut left, mut right) = ([0.7; 4], [-0.4; 4]);
        plugin.process(&mut left, &mut right);
        assert!(left[0] < 0.03 && right[0] > -0.02);
    }

    #[test]
    fn a_moved_parameter_is_the_same_plugin_as_its_state() {
        let mut plugin = FastLp::default();
        assert_eq!((plugin.param(0), plugin.param(1)), (1024.0, 0.0));
        plugin.set_param(0, 724.6);
        plugin.set_param(1, 2000.0);
        plugin.set_param(2, 1.0);
        assert_eq!(
            (plugin.param(0), plugin.param(1), plugin.param(2)),
            (725.0, 1024.0, 0.0)
        );
        plugin.process(&mut [0.0; 128], &mut [0.0; 128]);
        assert_eq!(plugin, with(725, 1024));
    }

    #[test]
    fn a_state_the_plugin_does_not_write_is_turned_down() {
        let mut plugin = with(931, 138);
        let before = plugin.clone();
        // FL does not load a version it does not know.
        assert_eq!(
            plugin.set_state(&state(&[2, 1024, 0])),
            Err(StateError::Version { found: 2 })
        );
        for len in [0, 3, 8, 16] {
            assert_eq!(
                plugin.set_state(&state(&[1, 1024, 0, 0])[..len]),
                Err(StateError::Length {
                    expected: 12,
                    found: len
                })
            );
        }
        for (param, value) in [(0, -1), (0, 1025), (1, -1), (1, 1025)] {
            let mut params = DEFAULTS;
            params[param] = value;
            assert_eq!(
                plugin.set_state(&state(&[1, params[0], params[1]])),
                Err(StateError::Range { param, value })
            );
        }
        assert_eq!(plugin, before);
        // A version under 1 is read as FL reads it.
        plugin.set_state(&state(&[0, 480, 0])).unwrap();
        assert_eq!(plugin, with(480, 0));
    }

    #[test]
    fn fl_ships_no_preset_for_it() {
        let Some(presets) = presets(NAME) else { return };
        assert!(presets.is_empty());
    }
}
