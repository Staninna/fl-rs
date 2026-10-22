//! Fruity Parametric EQ, the first one: seven bands of cookbook filters in
//! series after a main level, as `docs/plugins/fruity-parametric-eq.md`
//! has it.
//!
//! The state is the plugin's 29 parameters as little-endian i32, 116 bytes
//! with no version.
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 to 6 Band level, 0.01 dB | -1800 to 1800 | 0 |
//! | 7 to 13 Band frequency | 0 to 65536 | 5777, 13024, 19904, 33145, 42327, 49307, 55825 |
//! | 14 to 20 Band width | 0 to 65536 | 17500 |
//! | 21 to 27 Band type | 0 to 7 | low shelf, peak x 5, high shelf |
//! | 28 Main level | 0 to 256 | 128 |
//!
//! A frequency is `(101^(v / 65536) - 1) * 159.9 + 10` Hz, 10 Hz to
//! 16 kHz. The types are Off, low pass, band pass, high pass, notch, low
//! shelf, peak and high shelf, one cookbook filter each. The width is a Q
//! of `18.9 w + 0.1` for the passes and the notch, `(1001^w - 1) 0.0999 +
//! 0.1` for the peak and a slope of `1.9 w + 0.1` for the shelves. The main
//! level is `(11^(v / 128) - 1) / 10`, unity at 128 and +21.6 dB at 256.
//!
//! Each filter runs in double precision with single precision samples
//! between them. A peak or a shelf at 0 dB runs for a fifth of a second
//! and then drops out. Unlike the second one, the plugin never sleeps,
//! has no HQ mode and ramps nothing: a new main level counts from the
//! next block on.

use crate::{Effect, Fidelity, StateError, knob_value, read_state};
use dsp::biquad::{Biquad64, Coefficients64};
use std::f64::consts::TAU;

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Parametric EQ";

const BANDS: usize = 7;
const PARAMS: usize = 4 * BANDS + 1;
/// Where each group of parameters starts.
const FREQ: usize = BANDS;
const WIDTH: usize = 2 * BANDS;
const KIND: usize = 3 * BANDS;
const MAIN: usize = 4 * BANDS;

/// The range of each parameter's control.
const RANGES: [(i32, i32); PARAMS] = {
    let mut ranges = [(-1800, 1800); PARAMS];
    let mut band = 0;
    while band < BANDS {
        ranges[FREQ + band] = (0, 65536);
        ranges[WIDTH + band] = (0, 65536);
        ranges[KIND + band] = (0, 7);
        band += 1;
    }
    ranges[MAIN] = (0, 256);
    ranges
};

#[rustfmt::skip]
const DEFAULTS: [i32; PARAMS] = [
    0, 0, 0, 0, 0, 0, 0,
    5777, 13024, 19904, 33145, 42327, 49307, 55825,
    17500, 17500, 17500, 17500, 17500, 17500, 17500,
    5, 6, 6, 6, 6, 6, 7,
    128,
];

/// The types a band can have, by their number in the state.
const OFF: i32 = 0;
const LOW_PASS: i32 = 1;
const BAND_PASS: i32 = 2;
const HIGH_PASS: i32 = 3;
const NOTCH: i32 = 4;
const LOW_SHELF: i32 = 5;
const PEAK: i32 = 6;

/// `ln 11` as FL keeps it, a float worked out at startup.
fn ln_11() -> f32 {
    11f64.ln() as f32
}

/// The frequency wheel in Hz, as a float.
fn hertz(freq: i32) -> f32 {
    let share = (f64::from(freq) * (1.0 / 65536.0)) as f32;
    let curve = ((f64::from(share) * 101f64.ln()).exp() - 1.0) as f32;
    (f64::from(curve) * 159.9 + 10.0) as f32
}

/// The width wheel as a part of its way, 0 to 1.
fn width(value: i32) -> f32 {
    (f64::from(value) * (1.0 / 65536.0)) as f32
}

/// The level in dB.
fn decibels(level: i32) -> f32 {
    level as f32 * 0.01
}

/// The main level's gain, or `None` at 128, where FL leaves the samples
/// as they are.
fn main_gain(level: i32) -> Option<f32> {
    let share = level as f32 * (1.0 / 128.0);
    if share == 1.0 {
        return None;
    }
    let curve = f64::from(share * ln_11()).exp();
    Some(((curve - 1.0) * f64::from(0.1f32)) as f32)
}

/// `10^(dB / 40)` by Delphi's `Power`: `exp(x ln 10)`, and 1 for 0 dB.
fn amplitude(db: f32) -> f64 {
    let x = f64::from(db) * 0.025;
    if x == 0.0 {
        1.0
    } else {
        (x * 10f64.ln()).exp()
    }
}

/// The parameters of one band.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Knobs {
    level: i32,
    freq: i32,
    width: i32,
    kind: i32,
}

impl Knobs {
    /// The filter of a band that is not off (`FUN_005dbfa0`). `step` is
    /// `2 pi / SR` as FL has it, and `top` the highest frequency, a float
    /// of `0.4999 SR`.
    fn design(&self, step: f64, top: f32) -> Coefficients64 {
        let mut hz = hertz(self.freq);
        if hz > top {
            hz = top;
        }
        let w0 = f64::from(hz) * step;
        let (sin, cos) = (w0.sin(), w0.cos());
        let width = width(self.width);
        let db = decibels(self.level);
        match self.kind {
            LOW_PASS | BAND_PASS | HIGH_PASS | NOTCH => {
                let alpha = sin / ((f64::from(width) * 18.9 + 0.1) * 2.0);
                match self.kind {
                    LOW_PASS => Coefficients64::low_pass(cos, alpha),
                    BAND_PASS => Coefficients64::band_pass(cos, alpha),
                    HIGH_PASS => Coefficients64::high_pass(cos, alpha),
                    _ => Coefficients64::notch(cos, alpha),
                }
            }
            PEAK => {
                let a = amplitude(db);
                let spread =
                    ((f64::from(width) * f64::from(1000.0f32 + 1.0).ln()).exp() - 1.0) as f32;
                let alpha = sin / ((f64::from(spread) * 0.0999 + 0.1) * 2.0);
                Coefficients64::peak(cos, alpha, a)
            }
            _ => {
                let a = amplitude(db);
                let slope = f64::from(width) * 1.9 + 0.1;
                let beta = ((a * a + 1.0) / slope - (a - 1.0) * (a - 1.0)).sqrt();
                if self.kind == LOW_SHELF {
                    Coefficients64::low_shelf(cos, beta * sin, a)
                } else {
                    Coefficients64::high_shelf(cos, beta * sin, a)
                }
            }
        }
    }
}

/// One band as it runs.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Band {
    /// 0 for a band that does nothing, -1 for one that runs, and for a
    /// peak or a shelf at 0 dB the frames it still runs for.
    tail: i32,
    filter: Coefficients64,
    /// What the filter keeps of each side.
    state: [Biquad64; 2],
}

/// Fruity Parametric EQ. A new one has FL's defaults: seven bands at
/// 0 dB, which pass everything.
#[derive(Debug, Clone, PartialEq)]
pub struct ParametricEq {
    params: [i32; PARAMS],
    rate: i32,
    /// The main level, `None` at unity.
    main: Option<f32>,
    bands: [Band; BANDS],
}

impl Default for ParametricEq {
    fn default() -> Self {
        let band = Band {
            tail: 0,
            filter: Coefficients64::IDENTITY,
            state: Default::default(),
        };
        let mut plugin = ParametricEq {
            params: DEFAULTS,
            rate: 44_100,
            main: None,
            bands: [band; BANDS],
        };
        plugin.design_all();
        plugin
    }
}

impl ParametricEq {
    fn knobs(&self, band: usize) -> Knobs {
        Knobs {
            level: self.params[band],
            freq: self.params[FREQ + band],
            width: self.params[WIDTH + band],
            kind: self.params[KIND + band],
        }
    }

    /// Work a band's filter out from its parameters (`FUN_006cf980`).
    /// What the filter holds stays.
    fn design(&mut self, band: usize) {
        let knobs = self.knobs(band);
        let rate = self.rate;
        let step = (1.0 / f64::from(rate)) * TAU;
        let top = (f64::from(rate) * 0.4999) as f32;
        let band = &mut self.bands[band];
        if knobs.kind == OFF {
            band.tail = 0;
            return;
        }
        // A peak or a shelf with no gain rings out and then drops out.
        band.tail = if knobs.kind >= LOW_SHELF && decibels(knobs.level) == 0.0 {
            rate / 5
        } else {
            -1
        };
        band.filter = knobs.design(step, top);
    }

    fn design_all(&mut self) {
        for band in 0..BANDS {
            self.design(band);
        }
    }
}

impl Effect for ParametricEq {
    /// A state sets every parameter as moving it would, which empties
    /// every band.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let params = read_state(bytes, &RANGES)?;
        self.params = params;
        self.main = main_gain(params[MAIN]);
        self.design_all();
        for band in &mut self.bands {
            band.state = Default::default();
        }
        Ok(())
    }

    /// The filters are worked out anew for the rate and keep what they
    /// hold. A rate that is not a number of 1 Hz or more is not taken.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz >= 1.0 {
            self.rate = hz.round() as i32;
            self.design_all();
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        let (left, right) = (&mut left[..len], &mut right[..len]);
        if let Some(gain) = self.main {
            for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                *l *= gain;
                *r *= gain;
            }
        }
        let frames = i32::try_from(len).unwrap_or(i32::MAX);
        for band in &mut self.bands {
            if band.tail == 0 {
                continue;
            }
            let [l, r] = &mut band.state;
            l.process_block(&band.filter, left);
            r.process_block(&band.filter, right);
            l.flush();
            r.flush();
            if band.tail > 0 {
                band.tail = band.tail.saturating_sub(frames);
                if band.tail <= 0 {
                    band.tail = 0;
                    band.state = Default::default();
                }
            }
        }
    }

    fn param_count(&self) -> usize {
        PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |value| *value as f32)
    }

    /// The main level counts from the next block on. A band's level,
    /// frequency or width moves its filter and leaves what it holds, and
    /// its type also empties it.
    fn set_param(&mut self, index: usize, value: f32) {
        let Some(range) = RANGES.get(index) else {
            return;
        };
        let value = knob_value(value, *range);
        self.params[index] = value;
        if index == MAIN {
            self.main = main_gain(value);
            return;
        }
        let band = index % BANDS;
        if index >= KIND {
            self.bands[band].state = Default::default();
        }
        self.design(band);
    }

    fn fidelity(&self) -> Fidelity {
        Fidelity::Exact
    }

    /// Empties every filter.
    fn reset(&mut self) {
        for band in &mut self.bands {
            band.state = Default::default();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{bits, corpus, partial, presets, projects, response, sine, state};

    fn with(params: &[i32; PARAMS]) -> ParametricEq {
        let mut plugin = ParametricEq::default();
        plugin.set_state(&state(params)).unwrap();
        plugin
    }

    /// Where the frequency wheel is for this many Hz.
    fn wheel(hz: f64) -> i32 {
        (65536.0 * ((hz - 10.0) / 159.9 + 1.0).ln() / 101f64.ln()).round() as i32
    }

    /// The default state with one band set and the others typed off.
    fn one_band(kind: i32, hz: f64, level: i32, width: i32) -> [i32; PARAMS] {
        let mut params = DEFAULTS;
        params[KIND..MAIN].fill(OFF);
        params[0] = level;
        params[FREQ] = wheel(hz);
        params[WIDTH] = width;
        params[KIND] = kind;
        params
    }

    /// The gain in dB the plugin has at 44.1 kHz for a tone of a whole
    /// number of Hz, once it has settled.
    fn measured(plugin: &mut ParametricEq, hz: usize) -> f64 {
        let turns = hz as f64 / 44_100.0;
        let mut left: Vec<f32> = sine(turns, 2 * 44_100).iter().map(|v| v * 0.25).collect();
        let mut right = left.clone();
        for (l, r) in left.chunks_mut(512).zip(right.chunks_mut(512)) {
            plugin.process(l, r);
        }
        assert!(left == right);
        20.0 * (partial(&left[44_100..], turns) / 0.25).log10()
    }

    /// The gain in dB at `hz` of a filter given by its numerator and
    /// denominator at 44.1 kHz.
    fn cookbook(num: [f64; 3], den: [f64; 3], hz: f64) -> f64 {
        20.0 * response(&num, &den, hz / 44_100.0).log10()
    }

    #[test]
    fn the_wheels_are_the_doc_s_curves() {
        assert_eq!((hertz(0), hertz(65536)), (10.0, 16_000.0));
        assert!((f64::from(hertz(wheel(1_000.0))) - 1_000.0).abs() < 0.2);
        assert_eq!((width(0), width(65536)), (0.0, 1.0));
        assert_eq!(decibels(-1800), -18.0);
        // The main level: unity at 128, 0 at 0, 12 times at 256.
        assert_eq!(main_gain(128), None);
        assert_eq!(main_gain(0), Some(0.0));
        let top = f64::from(main_gain(256).unwrap());
        assert!((top - 12.0).abs() < 1e-5, "{top}");
        let half = f64::from(main_gain(64).unwrap());
        assert!((half - (11f64.sqrt() - 1.0) / 10.0).abs() < 1e-6);
        // 10^(dB / 40), and exactly 1 at 0 dB.
        assert_eq!(amplitude(0.0), 1.0);
        assert!((amplitude(6.0) - 10f64.powf(0.15)).abs() < 1e-12);
    }

    #[test]
    fn the_default_passes_everything_and_its_bands_drop_out() {
        let mut plugin = ParametricEq::default();
        assert_eq!(plugin, with(&DEFAULTS));
        assert_eq!(plugin.fidelity(), Fidelity::Exact);
        assert_eq!(plugin.param_count(), 29);
        assert_eq!(plugin.param(MAIN), 128.0);
        let running = |plugin: &ParametricEq| plugin.bands.iter().filter(|b| b.tail != 0).count();
        assert_eq!(running(&plugin), 7);
        let input: Vec<f32> = sine(0.01, 3 * 8_820).iter().map(|v| v * 0.5).collect();
        let mut left = input.clone();
        let mut right = input.clone();
        for (block, (l, r)) in left.chunks_mut(441).zip(right.chunks_mut(441)).enumerate() {
            assert_eq!(running(&plugin), if block < 20 { 7 } else { 0 }, "{block}");
            plugin.process(l, r);
        }
        assert!(left.iter().zip(&input).all(|(a, b)| (a - b).abs() < 1e-6));
        assert_eq!(bits(&left[8_820..]), bits(&input[8_820..]));
    }

    #[test]
    fn each_type_is_the_cookbook_s_filter() {
        let w = |hz: f64| TAU * f64::from(hertz(wheel(hz))) / 44_100.0;
        // A peak of +6 dB at 1 kHz, its width wheel at 0.3: a Q of
        // (1001^0.3 - 1) 0.0999 + 0.1.
        let params = one_band(PEAK, 1_000.0, 600, 19_661);
        let q = ((f64::from(width(19_661)) * 1001f64.ln()).exp() - 1.0) * 0.0999 + 0.1;
        let w0 = w(1_000.0);
        let alpha = w0.sin() / (2.0 * q);
        let a = 10f64.powf(6.0 / 40.0);
        let num = [1.0 + alpha * a, -2.0 * w0.cos(), 1.0 - alpha * a];
        let den = [1.0 + alpha / a, -2.0 * w0.cos(), 1.0 - alpha / a];
        for hz in [100, 700, 1_000, 3_000, 10_000] {
            let got = measured(&mut with(&params), hz);
            let want = cookbook(num, den, hz as f64);
            assert!((got - want).abs() < 2e-3, "{hz} Hz: {got} and not {want}");
        }
        assert!((measured(&mut with(&params), 1_000) - 6.0).abs() < 0.01);

        // A low pass at 2 kHz with the width at 0.05: a Q of
        // 18.9 * 0.05 + 0.1, which it has at its corner.
        let params = one_band(LOW_PASS, 2_000.0, 0, 3_277);
        let q = f64::from(width(3_277)) * 18.9 + 0.1;
        let w0 = w(2_000.0);
        let (alpha, c) = (w0.sin() / (2.0 * q), w0.cos());
        let num = [(1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0];
        let den = [1.0 + alpha, -2.0 * c, 1.0 - alpha];
        for hz in [200, 2_000, 8_000] {
            let got = measured(&mut with(&params), hz);
            let want = cookbook(num, den, hz as f64);
            assert!((got - want).abs() < 2e-3, "{hz} Hz: {got} and not {want}");
        }
        let corner = measured(&mut with(&params), 2_000);
        assert!((corner - 20.0 * q.log10()).abs() < 0.05, "{corner}");

        // A high pass passes the top and cuts the bottom, a band pass
        // passes its centre, and a notch takes it out.
        let high = one_band(HIGH_PASS, 500.0, 0, 3_277);
        assert!(measured(&mut with(&high), 10_000).abs() < 0.05);
        assert!(measured(&mut with(&high), 50) < -35.0);
        let band = one_band(BAND_PASS, 1_000.0, 0, 3_277);
        assert!(measured(&mut with(&band), 1_000).abs() < 0.01);
        assert!(measured(&mut with(&band), 100) < -20.0);
        let notch = one_band(NOTCH, 1_000.0, 0, 3_277);
        assert!(measured(&mut with(&notch), 1_000) < -40.0);
        assert!(measured(&mut with(&notch), 100).abs() < 0.1);

        // The shelves have their gain at their end, half of it at their
        // corner and none at the other end.
        let low = one_band(LOW_SHELF, 500.0, -900, 32_768);
        assert!((measured(&mut with(&low), 20) + 9.0).abs() < 0.02);
        assert!((measured(&mut with(&low), 500) + 4.5).abs() < 0.1);
        assert!(measured(&mut with(&low), 15_000).abs() < 0.02);
        let high = one_band(7, 5_000.0, 900, 32_768);
        assert!((measured(&mut with(&high), 20_000) - 9.0).abs() < 0.1);
        assert!(measured(&mut with(&high), 30).abs() < 0.02);
    }

    #[test]
    fn the_main_level_comes_first_and_jumps() {
        let mut params = DEFAULTS;
        params[KIND..MAIN].fill(OFF);
        params[MAIN] = 64;
        let mut plugin = with(&params);
        let (mut left, mut right) = ([1.0; 4], [-0.5; 4]);
        plugin.process(&mut left, &mut right);
        let gain = main_gain(64).unwrap();
        assert_eq!((left, right), ([gain; 4], [-0.5 * gain; 4]));
        plugin.set_param(MAIN, 256.0);
        let (mut left, mut right) = ([1.0; 4], [1.0; 4]);
        plugin.process(&mut left, &mut right);
        assert_eq!(left, [main_gain(256).unwrap(); 4]);
        // At 128 the samples are not touched.
        plugin.set_param(MAIN, 128.0);
        let input = [0.3, -0.7, 1e-30, 0.0];
        let (mut left, mut right) = (input, input);
        plugin.process(&mut left, &mut right);
        assert_eq!(bits(&left), bits(&input));
    }

    #[test]
    fn a_moved_parameter_is_the_same_plugin_as_its_state() {
        let mut plugin = ParametricEq::default();
        for (index, value) in [
            (1, -420.4),
            (FREQ + 1, 70_000.0),
            (WIDTH + 1, 9000.2),
            (KIND + 1, 3.6),
            (KIND + 4, 0.0),
            (MAIN, 300.0),
        ] {
            plugin.set_param(index, value);
        }
        let mut params = DEFAULTS;
        for (index, value) in [
            (1, -420),
            (FREQ + 1, 65536),
            (WIDTH + 1, 9000),
            (KIND + 1, NOTCH),
            (KIND + 4, OFF),
            (MAIN, 256),
        ] {
            params[index] = value;
        }
        let mut loaded = with(&params);
        // A band typed off keeps the filter it had, which does not run.
        loaded.bands[4].filter = plugin.bands[4].filter;
        assert_eq!(plugin, loaded);
        // A type empties the band's filter, a width does not.
        let mut plugin = with(&one_band(LOW_PASS, 200.0, 0, 0));
        plugin.process(&mut [1.0; 64], &mut [1.0; 64]);
        let filled = |p: &ParametricEq| p.bands[0].state != [Biquad64::default(); 2];
        plugin.set_param(WIDTH, 30_000.0);
        assert!(filled(&plugin));
        plugin.set_param(KIND, f32::from(HIGH_PASS as i8));
        assert!(!filled(&plugin));
    }

    #[test]
    fn a_state_the_plugin_does_not_write_is_turned_down() {
        let mut plugin = with(&one_band(PEAK, 1_000.0, 600, 9_000));
        let before = plugin.clone();
        let bytes = state(&DEFAULTS);
        for wrong in [&bytes[..115], &[&bytes[..], &[0]].concat(), &[]] {
            assert_eq!(
                plugin.set_state(wrong),
                Err(StateError::Length {
                    expected: 116,
                    found: wrong.len()
                })
            );
        }
        for (param, value) in [(0, -1801), (FREQ, 65537), (KIND + 6, 8), (MAIN, 257)] {
            let mut params = DEFAULTS;
            params[param] = value;
            assert_eq!(
                plugin.set_state(&state(&params)),
                Err(StateError::Range { param, value })
            );
        }
        assert_eq!(plugin, before);
    }

    #[test]
    fn fl_s_presets_and_projects_load() {
        if let Some(presets) = presets(NAME) {
            assert_eq!(presets.len(), 21);
            for preset in &presets {
                assert_eq!(preset.state.len(), 116, "{}", preset.file);
                let mut plugin = ParametricEq::default();
                plugin.set_state(&preset.state).unwrap();
                let (mut left, mut right) = (sine(0.01, 4_410), sine(0.02, 4_410));
                plugin.process(&mut left, &mut right);
                let sane = left
                    .iter()
                    .chain(&right)
                    .all(|v| v.is_finite() && v.abs() < 20.0);
                assert!(sane, "{}", preset.file);
            }
            // The preset called Default is the defaults but for two
            // wider peaks.
            let default = presets.iter().find(|p| p.file == "Default").unwrap();
            let mut plugin = ParametricEq::default();
            plugin.set_state(&default.state).unwrap();
            let mut params = DEFAULTS;
            params[WIDTH + 4..WIDTH + 6].fill(24_000);
            assert_eq!(plugin.params, params);
        }
        let Some(root) = corpus() else { return };
        let states: Vec<_> = projects(&root)
            .into_iter()
            .filter(|saved| saved.plugin.eq_ignore_ascii_case(NAME))
            .collect();
        assert_eq!(states.len(), 9);
        for saved in states {
            let mut plugin = ParametricEq::default();
            plugin.set_state(&saved.state).unwrap();
        }
    }
}
