//! Fruity Free Filter: one biquad a side of seven kinds, and an optional
//! remover of DC after it, as `docs/plugins/fruity-free-filter.md` has
//! it.
//!
//! The state is 21 bytes: a little-endian i32 version, 2, the four
//! parameters as i32, and a byte for the center switch. Version 1 has no
//! byte and the switch off. FL leaves a state alone whose version is 3 or
//! more, and so does this.
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Type | 100 to 700 | 100 |
//! | 1 Frequency | 0 to 1024 | 512 |
//! | 2 Q | 0 to 1024 | 169 |
//! | 3 Gain | 0 to 1024 | 676 |
//!
//! The type wheel picks low pass, band pass, high pass, notch, low shelf,
//! peaking and high shelf. The frequency is `(101^v - 1) 159.9 + 10` Hz,
//! the Q `18.9 v + 0.1` (a bandwidth for the peak, a slope for the
//! shelves), the gain `36 v - 18` dB. The filter is the cookbook's, with
//! one quirk kept: FL works out its angle at 44.1 kHz whatever the rate,
//! so at 48 kHz every corner is 48 / 44.1 higher.

use crate::{Effect, StateError, knob_value, state_version};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Free Filter";

const COUNT: usize = 4;

/// The range of each parameter's wheel.
const RANGES: [(i32, i32); COUNT] = [(100, 700), (0, 1024), (0, 1024), (0, 1024)];
const DEFAULTS: [i32; COUNT] = [100, 512, 169, 676];

/// How long the state is, and the newest version FL reads.
const LEN: usize = 21;
const VERSION: i32 = 2;

/// Two pi over 44100, the angle of 1 Hz FL takes at every rate.
const ANGLE: f64 = f64::from_bits(0x3f22_acb2_4b91_e1ee);

/// What a wheel's value is, 0 to 1: its offset times FL's f64 scale.
fn place(index: usize, value: i32) -> f32 {
    let (min, max) = RANGES[index];
    let v = f64::from(value - min) * (1.0 / f64::from(max - min));
    v.clamp(0.0, 1.0) as f32
}

/// Delphi's `Power` for a positive base: `exp(y ln x)`.
fn power(x: f64, y: f64) -> f64 {
    (y * x.ln()).exp()
}

/// The filter's settings and coefficients: FL's record at +0x130.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Design {
    /// 1 low pass, 2 band pass, 3 high pass, 4 notch, 5 low shelf,
    /// 6 peaking, 7 high shelf.
    kind: i32,
    hz: f32,
    q: f32,
    db: f32,
    /// `b0, b1, b2, a1, a2`, for `y = b0 x + b1 x1 + b2 x2 - a1 y1 - a2 y2`.
    c: [f32; 5],
}

impl Design {
    /// `FUN_006cac20` and `FUN_006ca5c0`: the coefficients from the
    /// settings.
    fn design(&mut self) {
        self.hz = self.hz.min(22_000.0);
        let w = (f64::from(self.hz) * ANGLE).max(0.0004);
        let (sin, cos) = w.sin_cos();
        let q = f64::from(self.q);
        let gain = || power(10.0, f64::from(self.db) * 0.025);
        let (a, alpha) = match self.kind {
            6 => {
                let width = (q * 1001f64.ln()).exp() - 1.0;
                let width = f64::from(width as f32) * 0.0999 + 0.1;
                (gain(), sin / (width * 2.0))
            }
            5 | 7 => {
                let a = gain();
                let slope = (a * a + 1.0) / (q * 1.9 + 0.1) - (a - 1.0) * (a - 1.0);
                (a, slope.sqrt())
            }
            _ => (0.0, sin / ((q * 18.9 + 0.1) * 2.0)),
        };
        let f = |x: f64| x as f32;
        self.c = match self.kind {
            1 => {
                let n = 1.0 / (alpha + 1.0);
                let b1 = f(n * (1.0 - cos));
                let b0 = b1 * 0.5;
                [b0, b1, b0, f(n * cos * -2.0), f(n * (1.0 - alpha))]
            }
            2 => {
                let n = 1.0 / (alpha + 1.0);
                [
                    f(n * alpha),
                    0.0,
                    f(n * -alpha),
                    f(n * cos * -2.0),
                    f(n * (1.0 - alpha)),
                ]
            }
            3 => {
                let n = 1.0 / (alpha + 1.0);
                let b1 = f(n * -(cos + 1.0));
                let b0 = -b1 * 0.5;
                [b0, b1, b0, f(n * cos * -2.0), f(n * (1.0 - alpha))]
            }
            4 => {
                let n = 1.0 / (alpha + 1.0);
                let a1 = f(n * cos * -2.0);
                [f(n), a1, f(n), a1, f(n * (1.0 - alpha))]
            }
            5 => {
                let am = (a - 1.0) * cos;
                let beta = alpha * sin;
                let n = 1.0 / (a + 1.0 + am + beta);
                [
                    f(n * a * (((a + 1.0) - am) + beta)),
                    f(n * a * 2.0 * ((a - 1.0) - (a + 1.0) * cos)),
                    f(n * a * (((a + 1.0) - am) - beta)),
                    f(n * ((a - 1.0) + (a + 1.0) * cos) * -2.0),
                    f(n * ((a + 1.0 + am) - beta)),
                ]
            }
            6 => {
                let n = 1.0 / (alpha / a + 1.0);
                let a1 = f(n * cos * -2.0);
                [
                    f(n * (alpha * a + 1.0)),
                    a1,
                    f(n * (1.0 - alpha * a)),
                    a1,
                    f(n * (1.0 - alpha / a)),
                ]
            }
            _ => {
                let am = (a - 1.0) * cos;
                let beta = alpha * sin;
                let n = 1.0 / (((a + 1.0) - am) + beta);
                [
                    f(n * a * (a + 1.0 + am + beta)),
                    f(n * a * -2.0 * ((a - 1.0) + (a + 1.0) * cos)),
                    f(n * a * ((a + 1.0 + am) - beta)),
                    f(n * ((a - 1.0) - (a + 1.0) * cos) * 2.0),
                    f(n * (((a + 1.0) - am) - beta)),
                ]
            }
        };
    }
}

/// Fruity Free Filter. A new one has FL's defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct FreeFilter {
    params: [i32; COUNT],
    center: bool,
    design: Design,
    /// Each side's `[x1, x2, y1, y2]`.
    state: [[f32; 4]; 2],
    /// The DC remover: its two gains, and a side's level and its speed.
    dc_gain: f32,
    dc_damp: f32,
    dc: [[f64; 2]; 2],
}

impl Default for FreeFilter {
    fn default() -> Self {
        let mut plugin = FreeFilter {
            params: DEFAULTS,
            center: true,
            design: Design {
                kind: 0,
                hz: 0.0,
                q: 0.0,
                db: 0.0,
                c: [0.0; 5],
            },
            state: [[0.0; 4]; 2],
            dc_gain: 0.0,
            dc_damp: 0.96,
            dc: [[0.0; 2]; 2],
        };
        for (index, value) in DEFAULTS.into_iter().enumerate() {
            plugin.set(index, value);
        }
        // FL's constructor then puts these in the record and leaves the
        // coefficients as they are, for the next wheel to find.
        plugin.design.kind = 1;
        plugin.design.hz = 100.0;
        plugin.design.q = 0.5;
        plugin.design.db = 0.0;
        plugin.state = [[0.0; 4]; 2];
        plugin.set_sample_rate(44_100.0);
        plugin
    }
}

impl FreeFilter {
    /// One wheel (`FUN_006cd1e0`).
    fn set(&mut self, index: usize, value: i32) {
        self.params[index] = value;
        let v = place(index, value);
        let d = &mut self.design;
        match index {
            0 => {
                let kind = ((f64::from(v * 7.0) - 0.5).round_ties_even() as i32 + 1).clamp(1, 7);
                if d.kind != kind {
                    d.kind = kind;
                    self.state = [[0.0; 4]; 2];
                }
            }
            1 => {
                let rise = (f64::from(v) * 101f64.ln()).exp();
                d.hz = (f64::from((rise - 1.0) as f32) * 159.9 + 10.0) as f32;
            }
            2 => d.q = v,
            _ => d.db = v * 36.0 - 18.0,
        }
        self.design.design();
    }
}

impl Effect for FreeFilter {
    /// The filter's memory stays, as in FL, unless the type changes.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let version = state_version(bytes, LEN)?;
        if version > VERSION {
            return Err(StateError::Version { found: version });
        }
        let len = if version > 1 { LEN } else { LEN - 1 };
        if bytes.len() != len {
            return Err(StateError::Length {
                expected: len,
                found: bytes.len(),
            });
        }
        let mut params = [0; COUNT];
        for (param, word) in bytes[4..20].as_chunks::<4>().0.iter().enumerate() {
            params[param] = i32::from_le_bytes(*word);
        }
        // An old type numbering, 0 up in tens: FL moves it into the range.
        if params[0] < RANGES[0].0 {
            let old = f64::from(params[0] / 10);
            params[0] = 100 + ((old - 0.5) / 7.0 * 600.0).round_ties_even() as i32;
        }
        for (param, value) in params.iter().enumerate() {
            let (min, max) = RANGES[param];
            if *value < min || *value > max {
                return Err(StateError::Range {
                    param,
                    value: *value,
                });
            }
        }
        self.center = version > 1 && bytes[20] & 1 == 1;
        for (index, value) in params.into_iter().enumerate() {
            self.set(index, value);
        }
        Ok(())
    }

    /// Only the DC remover follows the rate: the filter keeps 44.1 kHz.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz >= 1.0 {
            let hz = hz.round() as i64;
            self.dc_gain = (f64::from_bits(0x3ff0_1cc4_ac6c_daf4) / hz as f64) as f32;
            self.dc_damp = f32::from_bits(0x3f75_c28f);
            self.dc = [[0.0; 2]; 2];
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let [b0, b1, b2, a1, a2] = self.design.c;
        for (side, samples) in [left, right].into_iter().enumerate() {
            let s = &mut self.state[side];
            for x in samples.iter_mut() {
                let [x1, x2, y1, y2] = *s;
                let y = *x * b0 + (((x2 * b2 + x1 * b1) - y1 * a1) - y2 * a2);
                *s = [*x, x1, y, y1];
                *x = y;
            }
            if self.center {
                let [level, speed] = &mut self.dc[side];
                let (gain, damp) = (f64::from(self.dc_gain), f64::from(self.dc_damp));
                for x in samples.iter_mut() {
                    let input = f64::from(*x);
                    *speed += (input - *level) * gain;
                    *level += *speed;
                    *speed *= damp;
                    *x = (input - *level) as f32;
                }
                let tiny = f64::from(f32::from_bits(0x3380_0000));
                for value in [level, speed] {
                    if value.abs() < tiny {
                        *value = 0.0;
                    }
                }
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

    /// The filter's memory and the DC remover's emptied.
    fn reset(&mut self) {
        self.state = [[0.0; 4]; 2];
        self.dc = [[0.0; 2]; 2];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{every_preset, partial, presets, response, sine, state};

    fn bytes(params: [i32; COUNT], center: bool) -> Vec<u8> {
        let mut bytes = state(&[2]);
        bytes.extend(state(&params));
        bytes.push(u8::from(center));
        bytes
    }

    fn with(params: [i32; COUNT], center: bool) -> FreeFilter {
        let mut plugin = FreeFilter::default();
        plugin.set_state(&bytes(params, center)).unwrap();
        plugin
    }

    /// The gain of the design at `hz`, at 44.1 kHz.
    fn gain(d: &Design, hz: f64) -> f64 {
        let c = d.c.map(f64::from);
        response(&[c[0], c[1], c[2]], &[1.0, c[3], c[4]], hz / 44_100.0)
    }

    fn db(x: f64) -> f64 {
        20.0 * x.log10()
    }

    #[test]
    fn the_wheels_come_to_the_doc_s_numbers() {
        let d = with([100, 0, 0, 0], false).design;
        assert_eq!((d.kind, d.hz, d.q, d.db), (1, 10.0, 0.0, -18.0));
        let d = with([700, 1024, 1024, 1024], false).design;
        assert_eq!((d.kind, d.q, d.db), (7, 1.0, 18.0));
        assert!((d.hz - 16_000.0).abs() < 0.01);
        // Each seventh of the type wheel is the next kind, and the
        // hundreds are the seven.
        for (value, kind) in [
            (100, 1),
            (185, 1),
            (186, 2),
            (200, 2),
            (300, 3),
            (400, 4),
            (500, 5),
            (600, 6),
            (614, 6),
            (615, 7),
            (700, 7),
        ] {
            assert_eq!(
                with([value, 512, 512, 512], false).design.kind,
                kind,
                "{value}"
            );
        }
        // Half way is 101^0.5 - 1 of 159.9, and 10.
        let d = with([100, 512, 512, 512], false).design;
        assert!((f64::from(d.hz) - (101f64.sqrt() - 1.0) * 159.9 - 10.0).abs() < 0.01);
    }

    #[test]
    fn each_kind_has_its_shape_at_44_1_khz() {
        let at = |kind: i32, q: i32, gain_v: i32| {
            with([100 + (kind - 1) * 100, 512, q, gain_v], false).design
        };
        // The corner of the half wheel, about 1.46 kHz.
        let hz = f64::from(at(1, 512, 512).hz);
        // Low pass: whole below, nothing far above, 1 / (2 Q) at the corner.
        let low = at(1, 512, 512);
        let q = 18.9 * 0.5 + 0.1;
        assert!((gain(&low, 10.0) - 1.0).abs() < 1e-3);
        assert!(gain(&low, 20_000.0) < 0.01);
        assert!((gain(&low, hz) - q * 2.0 / 2.0).abs() < 0.01 * q);
        // High pass: the other way.
        let high = at(3, 512, 512);
        assert!(gain(&high, 10.0) < 1e-3 && (gain(&high, 21_000.0) - 1.0).abs() < 0.01);
        // Band pass: 1 at the corner.
        let band = at(2, 512, 512);
        assert!((gain(&band, hz) - 1.0).abs() < 1e-3 && gain(&band, 10.0) < 0.01);
        // Notch: nothing at the corner.
        let notch = at(4, 512, 512);
        assert!(gain(&notch, hz) < 0.01 && (gain(&notch, 10.0) - 1.0).abs() < 1e-3);
        // Peaking at +9 dB: 9 dB at the corner, flat far off.
        let peak = at(6, 512, 768);
        assert!(
            (db(gain(&peak, hz)) - 9.0).abs() < 0.05,
            "{}",
            db(gain(&peak, hz))
        );
        assert!(db(gain(&peak, 10.0)).abs() < 0.1);
        // Shelves at -9 dB: low shelf low down, high shelf high up.
        let shelf = at(5, 512, 256);
        assert!(
            (db(gain(&shelf, 10.0)) + 9.0).abs() < 0.05 && db(gain(&shelf, 20_000.0)).abs() < 0.1
        );
        let shelf = at(7, 512, 256);
        assert!(
            (db(gain(&shelf, 21_000.0)) + 9.0).abs() < 0.2 && db(gain(&shelf, 10.0)).abs() < 0.05
        );
    }

    #[test]
    fn the_filter_runs_the_design_on_each_side() {
        let mut plugin = with([300, 600, 300, 512], false);
        let d = plugin.design;
        for turns in [0.005, 0.05, 0.2] {
            plugin.reset();
            let (mut l, mut r) = (sine(turns, 8000), sine(turns, 8000));
            plugin.process(&mut l, &mut r);
            let wanted = gain(&d, turns * 44_100.0);
            assert!(
                (partial(&l[4000..], turns) - wanted).abs() < 1e-3,
                "{turns}"
            );
            assert_eq!(l, r);
        }
    }

    #[test]
    fn the_angle_is_44_1_khz_s_at_every_rate() {
        let mut plugin = with([100, 700, 300, 512], true);
        let before = plugin.design;
        plugin.set_sample_rate(96_000.0);
        assert_eq!(plugin.design, before);
        assert!((ANGLE - std::f64::consts::TAU / 44_100.0).abs() < 1e-15);
        // The DC remover does follow the rate.
        assert!((f64::from(plugin.dc_gain) - 1.007_023_5 / 96_000.0).abs() < 1e-12);
    }

    #[test]
    fn the_center_switch_takes_dc_out() {
        let mut plugin = with([100, 1024, 169, 676], true);
        let (mut l, mut r) = (vec![0.5; 200_000], vec![-0.25; 200_000]);
        for (l, r) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
            plugin.process(l, r);
        }
        assert!(
            l[199_999].abs() < 1e-3 && r[199_999].abs() < 1e-3,
            "{} {}",
            l[199_999],
            r[199_999]
        );
        // Off, the level stays.
        let mut plugin = with([100, 1024, 169, 676], false);
        let (mut l, mut r) = (vec![0.5; 5000], vec![-0.25; 5000]);
        plugin.process(&mut l, &mut r);
        assert!((l[4999] - 0.5).abs() < 1e-4);
    }

    #[test]
    fn a_new_plugin_keeps_fl_s_record_for_the_next_wheel() {
        // FL's constructor leaves the record at a 100 Hz low pass of Q 0.5
        // under the default wheels' coefficients.
        let plugin = FreeFilter::default();
        let fresh = with(DEFAULTS, true);
        assert_eq!(plugin.design.c, fresh.design.c);
        assert_eq!(
            (plugin.design.kind, plugin.design.hz, plugin.design.q),
            (1, 100.0, 0.5)
        );
        let mut moved = plugin.clone();
        moved.set_param(3, 676.0);
        assert_eq!(moved.design.q, 0.5);
        assert_ne!(moved.design.c, fresh.design.c);
    }

    #[test]
    fn states_of_both_versions_load_and_others_are_turned_down() {
        let mut plugin = FreeFilter::default();
        let mut old = state(&[1, 400, 100, 0, 512]);
        assert_eq!(plugin.set_state(&old), Ok(()));
        assert!(!plugin.center && plugin.design.kind == 4);
        old.push(1);
        assert_eq!(
            plugin.set_state(&old),
            Err(StateError::Length {
                expected: 20,
                found: 21
            })
        );
        // A type under 100 is FL's old numbering.
        assert_eq!(plugin.set_state(&bytes([35, 512, 512, 512], true)), Ok(()));
        assert_eq!(
            plugin.params[0],
            100 + ((3.0f64 - 0.5) / 7.0 * 600.0).round() as i32
        );
        let before = plugin.clone();
        let mut newer = bytes(DEFAULTS, true);
        newer[0] = 3;
        assert_eq!(
            plugin.set_state(&newer),
            Err(StateError::Version { found: 3 })
        );
        assert_eq!(
            plugin.set_state(&bytes([100, 1025, 0, 0], true)),
            Err(StateError::Range {
                param: 1,
                value: 1025
            })
        );
        assert_eq!(plugin, before);
    }

    #[test]
    fn fl_s_presets_load_and_play() {
        let Some(own) = presets(NAME) else { return };
        let all = every_preset(&crate::testing::corpus().unwrap(), NAME);
        assert!(!own.is_empty());
        for preset in &all {
            let mut plugin = FreeFilter::default();
            assert_eq!(plugin.set_state(&preset.state), Ok(()), "{}", preset.file);
            let (mut l, mut r) = (sine(0.01, 4000), sine(0.02, 4000));
            plugin.process(&mut l, &mut r);
            assert!(
                l.iter().chain(&r).all(|v| v.is_finite() && v.abs() < 50.0),
                "{}",
                preset.file
            );
        }
    }
}
