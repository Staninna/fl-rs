//! Fruity 7 Band EQ: a low shelf, five peaks and a high shelf in series,
//! one fader each, as `docs/plugins/fruity-7-band-eq.md` has it.
//!
//! The state is 32 bytes: a little-endian i32 version, 1, and the seven
//! faders as i32. FL leaves a state alone whose version is 2 or more, and
//! so does this.
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 to 6 Band 1 to 7 level, 0.01 dB | -1800 to 1800 | 0 |
//!
//! A fader at `v` is `36 (v + 1800) / 3600 - 18` dB, worked out in
//! single precision. The bands sit at 63, 250, 500, 1500, 3000, 5000 and
//! 8000 Hz. The first is a cookbook low shelf and the last a high shelf,
//! both of slope 0.5; the five between are cookbook peaks 2, 2, 2, 1 and 1
//! octaves wide. The coefficients are worked out in double precision and
//! kept as floats. Each band is direct form I with float samples and
//! state and its sum in double precision, as FL's x87 kernel has it, and
//! a state value under 2^-24 becomes 0 after each block.
//!
//! FL designs the bands at 44100 Hz whatever the rate is: the rate it
//! divides by is a variable of the plugin that nothing writes. So does
//! this, and at another rate every band sits where FL puts it, off its
//! label by the ratio of the rates.
//!
//! A band at 0 dB runs for 48 more blocks, whatever their length, then
//! has its memory emptied and drops out. A band that is moved off 0 dB
//! comes back with the memory it had.

use crate::{Effect, StateError, knob_value, state_version};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity 7 Band EQ";

const BANDS: usize = 7;

/// The range of each fader, in 0.01 dB.
const RANGE: (i32, i32) = (-1800, 1800);

/// How long the state is, and the newest version FL reads.
const LEN: usize = 4 + 4 * BANDS;
const VERSION: i32 = 1;

/// Where each band is, in Hz, and its width: a slope for the shelves and
/// octaves for the peaks (the tables at `0x00701eb4` and `0x00701ed0`).
const HERTZ: [f32; BANDS] = [63.0, 250.0, 500.0, 1500.0, 3000.0, 5000.0, 8000.0];
const WIDTHS: [f32; BANDS] = [0.5, 2.0, 2.0, 2.0, 1.0, 1.0, 0.5];

/// What each band is, by the section kinds FL numbers them with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    LowShelf,
    Peak,
    HighShelf,
}

const KINDS: [Kind; BANDS] = [
    Kind::LowShelf,
    Kind::Peak,
    Kind::Peak,
    Kind::Peak,
    Kind::Peak,
    Kind::Peak,
    Kind::HighShelf,
];

/// The rate FL designs with, the variable at `0x006f3dc4`.
const DESIGN_RATE: i32 = 44_100;

/// How many blocks a band at 0 dB still runs for.
const TAIL: i32 = 48;

/// A band that runs until its level changes.
const ALWAYS: i32 = -1;

/// A fader's level in dB, as `ProcessParam` (`0x006b3750`) and
/// `0x006b3a50` work it out: the fader's share of its way, then 36 dB of
/// travel from -18.
fn decibels(level: i32) -> f32 {
    let share = (f64::from(level - RANGE.0) * (1.0 / f64::from(RANGE.1 - RANGE.0))).clamp(0.0, 1.0);
    share as f32 * 36.0 + -18.0
}

/// Delphi's `Power` for floats with the base 10: `exp(y ln 10)`, and 1
/// for `y = 0` (`0x0045d0b0`).
fn power_of_10(y: f32) -> f32 {
    if y == 0.0 {
        1.0
    } else {
        (f64::from(y) * 10f64.ln()).exp() as f32
    }
}

/// Delphi's `Sinh` (`0x0045d1e0`): 0 where `IsZero` holds, under
/// 1e-12.
fn sinh(x: f64) -> f64 {
    if x.abs() <= 1e-12 {
        0.0
    } else {
        (x.exp() - (-x).exp()) / 2.0
    }
}

/// One band's coefficients, as floats: `y = b0 x + b1 x1 + b2 x2 - a1 y1
/// - a2 y2`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Section {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl Section {
    const IDENTITY: Section = Section {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    /// The band's filter at this level (`0x006b2e30`, then the cookbook
    /// at `0x006b0ef0`).
    fn design(kind: Kind, hz: f32, width: f32, db: f32) -> Section {
        let w = core::f32::consts::TAU * hz / DESIGN_RATE as f32;
        let (sin, cos) = f64::from(w).sin_cos();
        let (sin, cos) = (sin as f32, cos as f32);
        let a = power_of_10(db / 40.0);
        let x = if kind == Kind::Peak {
            let ln2_2 = 2f64.ln() / 2.0;
            let spread = ln2_2 * f64::from(width) * f64::from(w) / f64::from(sin);
            (f64::from(sin) * sinh(spread)) as f32
        } else {
            let beta = (a * a + 1.0) / width - (a - 1.0) * (a - 1.0);
            f64::from(beta).sqrt() as f32
        };
        Section::cookbook(
            kind,
            f64::from(a),
            f64::from(x),
            f64::from(sin),
            f64::from(cos),
        )
    }

    /// `0x006b0ef0`: `x` is the alpha of a peak and the beta of a shelf.
    /// Every coefficient is worked out in double precision and kept as a
    /// float, in FL's order of operations.
    fn cookbook(kind: Kind, a: f64, x: f64, sin: f64, cos: f64) -> Section {
        let f = |v: f64| v as f32;
        match kind {
            Kind::Peak => {
                let over = x / a;
                let inv = 1.0 / (over + 1.0);
                let a1 = f(inv * cos * -2.0);
                let under = x * a;
                Section {
                    b0: f(inv * (under + 1.0)),
                    b1: a1,
                    b2: f(inv * (1.0 - under)),
                    a1,
                    a2: f(inv * (1.0 - over)),
                }
            }
            Kind::LowShelf => {
                let c = (a - 1.0) * cos;
                let s = x * sin;
                let inv = 1.0 / ((a + 1.0) + c + s);
                Section {
                    b0: f(inv * (a * ((a + 1.0) - c + s))),
                    b1: f(inv * (a * 2.0 * ((a - 1.0) - (a + 1.0) * cos))),
                    b2: f(inv * (a * ((a + 1.0) - c - s))),
                    a1: f(inv * ((a - 1.0) + (a + 1.0) * cos) * -2.0),
                    a2: f(inv * ((a + 1.0) + c - s)),
                }
            }
            Kind::HighShelf => {
                let c = (a - 1.0) * cos;
                let s = x * sin;
                let inv = 1.0 / ((a + 1.0) - c + s);
                Section {
                    b0: f(inv * (a * ((a + 1.0) + c + s))),
                    b1: f(inv * (a * -2.0 * ((a - 1.0) + (a + 1.0) * cos))),
                    b2: f(inv * (a * ((a + 1.0) + c - s))),
                    a1: f(inv * ((a - 1.0) - (a + 1.0) * cos) * 2.0),
                    a2: f(inv * ((a + 1.0) - c - s)),
                }
            }
        }
    }
}

/// A state value under this is made 0 after a block: 2^-24, the float at
/// `0x006f3dec`.
const TINY: f32 = 5.960_464_5e-8;

/// What a band keeps of one side: the last two inputs and outputs.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Memory {
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Memory {
    /// One sample through the section, in the order of FL's x87 kernel
    /// (`0x006b1550`). The x87 unit keeps the sum between the loads and
    /// the stores at the precision Windows sets it to, a double's; the
    /// products of two floats are exact in it.
    fn tick(&mut self, s: &Section, x: f32) -> f32 {
        let wide = |a: f32, b: f32| f64::from(a) * f64::from(b);
        let mut acc = wide(self.x1, s.b1) + wide(self.x2, s.b2);
        acc -= wide(self.y1, s.a1);
        acc -= wide(self.y2, s.a2);
        acc += wide(x, s.b0);
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = acc as f32;
        self.y1
    }

    /// What FL does to the state after each block (`0x005de160`): a value
    /// under 2^-24 either way becomes 0.
    fn flush(&mut self) {
        for v in [&mut self.x1, &mut self.x2, &mut self.y1, &mut self.y2] {
            if v.abs() < TINY {
                *v = 0.0;
            }
        }
    }
}

/// One band as it runs.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Band {
    /// 0 for a band that is out, [`ALWAYS`] for one that runs, and for one
    /// at 0 dB the blocks it still runs for.
    tail: i32,
    section: Section,
    memory: [Memory; 2],
}

/// Fruity 7 Band EQ. A new one has every band at 0 dB.
#[derive(Debug, Clone, PartialEq)]
pub struct SevenBandEq {
    params: [i32; BANDS],
    bands: [Band; BANDS],
}

impl Default for SevenBandEq {
    fn default() -> Self {
        let band = Band {
            tail: 0,
            section: Section::IDENTITY,
            memory: Default::default(),
        };
        let mut plugin = SevenBandEq {
            params: [0; BANDS],
            bands: [band; BANDS],
        };
        for index in 0..BANDS {
            plugin.set_level(index, 0);
        }
        plugin
    }
}

impl SevenBandEq {
    /// A fader moved: the band designed again, and its tail started again
    /// (`0x006b3a50`). Its memory stays.
    fn set_level(&mut self, index: usize, level: i32) {
        self.params[index] = level;
        let db = decibels(level);
        let band = &mut self.bands[index];
        band.tail = if db == 0.0 { TAIL } else { ALWAYS };
        band.section = Section::design(KINDS[index], HERTZ[index], WIDTHS[index], db);
    }
}

impl Effect for SevenBandEq {
    /// Every fader is set as moving it would, as FL's loader does; the
    /// bands keep their memory.
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
        let mut params = [0; BANDS];
        for (param, word) in bytes[4..].as_chunks::<4>().0.iter().enumerate() {
            let value = i32::from_le_bytes(*word);
            if value < RANGE.0 || value > RANGE.1 {
                return Err(StateError::Range { param, value });
            }
            params[param] = value;
        }
        for (index, level) in params.into_iter().enumerate() {
            self.set_level(index, level);
        }
        Ok(())
    }

    /// Nothing changes: FL designs at 44100 Hz whatever the rate.
    fn set_sample_rate(&mut self, _hz: f32) {}

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for band in &mut self.bands {
            if band.tail == 0 {
                continue;
            }
            if band.tail > 0 {
                band.tail -= 1;
                if band.tail == 0 {
                    band.memory = Default::default();
                }
            }
            let section = band.section;
            let [l_memory, r_memory] = &mut band.memory;
            for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                *l = l_memory.tick(&section, *l);
                *r = r_memory.tick(&section, *r);
            }
            l_memory.flush();
            r_memory.flush();
        }
    }

    fn param_count(&self) -> usize {
        BANDS
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |value| *value as f32)
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if index < BANDS {
            self.set_level(index, knob_value(value, RANGE));
        }
    }

    /// FL's flush: every band's memory emptied (`0x006b3d10`). The tails
    /// go on.
    fn reset(&mut self) {
        for band in &mut self.bands {
            band.memory = Default::default();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{corpus, every_preset, partial, response, sine, state};

    /// A state of version 1 with these faders.
    fn bytes(levels: [i32; BANDS]) -> Vec<u8> {
        let mut bytes = state(&[1]);
        bytes.extend(state(&levels));
        bytes
    }

    fn with(levels: [i32; BANDS]) -> SevenBandEq {
        let mut plugin = SevenBandEq::default();
        plugin.set_state(&bytes(levels)).unwrap();
        plugin
    }

    /// A band's gain at `turns` a sample, from its coefficients.
    fn gain(section: &Section, turns: f64) -> f64 {
        let num = [section.b0, section.b1, section.b2].map(f64::from);
        let den = [1.0, f64::from(section.a1), f64::from(section.a2)];
        response(&num, &den, turns)
    }

    fn db(gain: f64) -> f64 {
        20.0 * gain.log10()
    }

    #[test]
    fn a_fader_s_travel_is_36_db_from_minus_18() {
        assert_eq!(decibels(0), 0.0);
        assert_eq!(decibels(-1800), -18.0);
        assert_eq!(decibels(1800), 18.0);
        assert_eq!(decibels(675), 6.75);
        assert_eq!(decibels(-900), -9.0);
        // Worked out as a share of the way and not as v / 100.
        assert_eq!(
            decibels(1),
            (1801.0 * (1.0f64 / 3600.0)) as f32 * 36.0 - 18.0
        );
        assert!((decibels(1) - 0.01).abs() < 1e-5);
    }

    #[test]
    fn each_band_has_its_level_at_its_centre_at_44_1_khz() {
        for (index, hz) in HERTZ.iter().enumerate() {
            for level in [-1800, -600, 450, 1800] {
                let want = f64::from(level) / 100.0;
                let section = Section::design(KINDS[index], *hz, WIDTHS[index], decibels(level));
                let turns = f64::from(*hz) / 44_100.0;
                let got = db(gain(&section, turns));
                match KINDS[index] {
                    // A peak has all of it at its centre.
                    Kind::Peak => assert!((got - want).abs() < 1e-3, "{index} {level} {got}"),
                    // A shelf has half of it at its corner, and nearly all
                    // of it far out on its side: a slope of 0.5 is gentle.
                    Kind::LowShelf => {
                        assert!((got - want / 2.0).abs() < 5e-3, "{index} {level} {got}");
                        let far = db(gain(&section, 5.0 / 44_100.0));
                        assert!((far - want).abs() < 0.25, "{far}");
                        let other = db(gain(&section, 0.4));
                        assert!(other.abs() < 0.05, "{other}");
                    }
                    Kind::HighShelf => {
                        assert!((got - want / 2.0).abs() < 5e-3, "{index} {level} {got}");
                        let far = db(gain(&section, 0.49));
                        assert!((far - want).abs() < 0.25, "{far}");
                        let other = db(gain(&section, 10.0 / 44_100.0));
                        assert!(other.abs() < 0.05, "{other}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_peak_is_as_many_octaves_wide_as_the_table_says() {
        // The cookbook's bandwidth is between the points at half the gain
        // in dB, `width` octaves apart around the centre, in the warped
        // frequencies of the bilinear transform; at 63 Hz and 250 Hz the
        // warp is small.
        let section = Section::design(Kind::Peak, 250.0, 2.0, 12.0);
        let edge = |octaves: f64| db(gain(&section, 250.0 * 2f64.powf(octaves) / 44_100.0));
        assert!((edge(1.0) - 6.0).abs() < 0.1, "{}", edge(1.0));
        assert!((edge(-1.0) - 6.0).abs() < 0.1, "{}", edge(-1.0));
        let narrow = Section::design(Kind::Peak, 3000.0, 1.0, 12.0);
        let half = db(gain(&narrow, 3000.0 * 2f64.sqrt() / 44_100.0));
        assert!((half - 6.0).abs() < 0.3, "{half}");
    }

    #[test]
    fn the_coefficients_are_the_cookbook_s_as_floats() {
        // The low shelf at +6 dB, by the cookbook in double precision.
        let section = Section::design(Kind::LowShelf, 63.0, 0.5, 6.0);
        let w = f64::from(core::f32::consts::TAU * 63.0 / 44_100.0);
        let a = 10f64.powf(6.0 / 40.0);
        let beta = ((a * a + 1.0) / 0.5 - (a - 1.0) * (a - 1.0)).sqrt();
        let (sin, cos) = w.sin_cos();
        let a0 = (a + 1.0) + (a - 1.0) * cos + beta * sin;
        let want = [
            a * ((a + 1.0) - (a - 1.0) * cos + beta * sin) / a0,
            2.0 * a * ((a - 1.0) - (a + 1.0) * cos) / a0,
            a * ((a + 1.0) - (a - 1.0) * cos - beta * sin) / a0,
            -2.0 * ((a - 1.0) + (a + 1.0) * cos) / a0,
            ((a + 1.0) + (a - 1.0) * cos - beta * sin) / a0,
        ];
        let got = [section.b0, section.b1, section.b2, section.a1, section.a2];
        for (got, want) in got.iter().zip(want) {
            assert!((f64::from(*got) - want).abs() < 1e-6, "{got} {want}");
        }
        // A peak's b1 is its a1.
        let peak = Section::design(Kind::Peak, 1500.0, 2.0, -3.0);
        assert_eq!(peak.b1, peak.a1);
        // At 0 dB a peak passes everything.
        let flat = Section::design(Kind::Peak, 500.0, 2.0, 0.0);
        assert_eq!((flat.b0, flat.b1, flat.b2), (1.0, flat.a1, flat.a2));
    }

    #[test]
    fn the_bands_run_in_series_on_each_side() {
        let levels = [600, -300, 0, 1200, -1800, 450, 900];
        let mut plugin = with(levels);
        let tone = sine(1000.0 / 44_100.0, 8820);
        let (mut l, mut r) = (tone.clone(), tone.iter().map(|v| -v).collect::<Vec<_>>());
        for (l, r) in l.chunks_mut(441).zip(r.chunks_mut(441)) {
            plugin.process(l, r);
        }
        // The same, band by band over the whole tone, with the flush
        // after every block.
        let mut want = tone.clone();
        for (index, level) in levels.iter().enumerate() {
            let section =
                Section::design(KINDS[index], HERTZ[index], WIDTHS[index], decibels(*level));
            let mut memory = Memory::default();
            for block in want.chunks_mut(441) {
                for v in block {
                    *v = memory.tick(&section, *v);
                }
                memory.flush();
            }
        }
        assert_eq!(l, want);
        assert!(l.iter().zip(&r).all(|(l, r)| *l == -r));
        // The level at 1 kHz is the product of the seven bands'.
        let sections = levels
            .iter()
            .enumerate()
            .map(|(i, v)| Section::design(KINDS[i], HERTZ[i], WIDTHS[i], decibels(*v)));
        let product: f64 = sections.map(|s| gain(&s, 1000.0 / 44_100.0)).product();
        let got = partial(&l[4410..], 1000.0 / 44_100.0);
        assert!((got / product - 1.0).abs() < 1e-3, "{got} {product}");
        assert_eq!(plugin.latency(), 0);
    }

    #[test]
    fn a_sample_is_summed_in_double_precision_and_the_state_flushed_after_a_block() {
        let section = Section {
            b0: 0.3,
            b1: -1.7,
            b2: 0.9,
            a1: -1.9,
            a2: 0.95,
        };
        let mut memory = Memory {
            x1: 0.123_456_7,
            x2: -0.765_432_1,
            y1: 0.333_333_3,
            y2: -0.111_111_1,
        };
        let w = |v: f32| f64::from(v);
        let sum = w(0.123_456_7) * w(-1.7) + w(-0.765_432_1) * w(0.9)
            - w(0.333_333_3) * w(-1.9)
            - w(-0.111_111_1) * w(0.95)
            + w(0.5) * w(0.3);
        let y = memory.tick(&section, 0.5);
        assert_eq!(y, sum as f32);
        assert_eq!(
            memory,
            Memory {
                x1: 0.5,
                x2: 0.123_456_7,
                y1: y,
                y2: 0.333_333_3
            }
        );
        // Under 2^-24 either way is 0, and 2^-24 itself stays.
        let mut memory = Memory {
            x1: 5.9e-8,
            x2: -5.9e-8,
            y1: TINY,
            y2: -1e-30,
        };
        memory.flush();
        assert_eq!(
            memory,
            Memory {
                x1: 0.0,
                x2: 0.0,
                y1: TINY,
                y2: 0.0
            }
        );
        assert_eq!(TINY, 2f32.powi(-24));
    }

    #[test]
    fn a_band_at_0_db_drops_out_after_48_blocks_with_its_memory_emptied() {
        let mut plugin = with([0, 0, 0, 900, 0, 0, 0]);
        assert_eq!(plugin.bands.map(|b| b.tail), [48, 48, 48, -1, 48, 48, 48]);
        for _ in 0..47 {
            plugin.process(&mut [0.5; 3], &mut [0.25; 3]);
        }
        assert_eq!(plugin.bands[0].tail, 1);
        assert_ne!(plugin.bands[0].memory[0], Memory::default());
        // The 48th block empties it before it runs, the next skips it.
        plugin.process(&mut [0.5; 3], &mut [0.25; 3]);
        assert_eq!(plugin.bands[0].tail, 0);
        assert_eq!(plugin.bands[0].memory[0].x2, 0.5);
        let before = plugin.bands[0];
        plugin.process(&mut [0.5; 3], &mut [0.25; 3]);
        assert_eq!(plugin.bands[0], before);
        assert_eq!(plugin.bands[3].tail, -1);
        // A move off 0 dB brings it back with what it had.
        plugin.set_param(0, 300.0);
        assert_eq!(plugin.bands[0].tail, -1);
        assert_eq!(plugin.bands[0].memory, before.memory);
        // And back to 0 starts the count again.
        plugin.set_param(0, 0.0);
        assert_eq!(plugin.bands[0].tail, 48);
        // A flush empties every band and leaves the counts.
        plugin.reset();
        assert!(
            plugin
                .bands
                .iter()
                .all(|b| b.memory == [Memory::default(); 2])
        );
        assert_eq!(plugin.bands[0].tail, 48);
    }

    #[test]
    fn the_rate_moves_nothing() {
        let mut plugin = with([300, -300, 600, -600, 900, -900, 1200]);
        let before = plugin.clone();
        plugin.set_sample_rate(96_000.0);
        assert_eq!(plugin, before);
        // At 96 kHz the 8 kHz shelf sits at 8000 * 96 / 44.1 Hz.
        let section = plugin.bands[6].section;
        let corner = db(gain(&section, 8000.0 / 44_100.0));
        assert!((corner - 6.0).abs() < 1e-3);
    }

    #[test]
    fn a_moved_fader_is_the_same_plugin_as_its_state() {
        let mut plugin = SevenBandEq::default();
        for (index, value) in [1800.4, -3000.0, 17.0, f32::NAN, -1799.6, 0.0, 999.0]
            .into_iter()
            .enumerate()
        {
            plugin.set_param(index, value);
        }
        plugin.set_param(7, 5.0);
        assert_eq!((plugin.param(7), plugin.param_count()), (0.0, 7));
        let same = with([1800, -1800, 17, 0, -1800, 0, 999]);
        assert_eq!(plugin, same);
    }

    #[test]
    fn a_state_the_plugin_does_not_write_is_turned_down() {
        let mut plugin = with([100, 200, 300, 400, 500, 600, 700]);
        let before = plugin.clone();
        for len in [3, 28, 36] {
            let mut wrong = bytes([0; BANDS]);
            wrong.resize(len, 0);
            assert_eq!(
                plugin.set_state(&wrong),
                Err(StateError::Length {
                    expected: 32,
                    found: len
                })
            );
        }
        for (param, value) in [(0, 1801), (6, -1801)] {
            let mut levels = [0; BANDS];
            levels[param] = value;
            assert_eq!(
                plugin.set_state(&bytes(levels)),
                Err(StateError::Range { param, value })
            );
        }
        let mut newer = bytes([0; BANDS]);
        newer[0] = 2;
        assert_eq!(
            plugin.set_state(&newer),
            Err(StateError::Version { found: 2 })
        );
        assert_eq!(plugin, before);
    }

    #[test]
    fn fl_s_presets_load_and_play() {
        let Some(root) = corpus() else {
            eprintln!("FL_RS_CORPUS unset, presets of {NAME} skipped");
            return;
        };
        let all = every_preset(&root, NAME);
        assert_eq!(all.len(), 14);
        for preset in &all {
            let mut plugin = SevenBandEq::default();
            assert_eq!(plugin.set_state(&preset.state), Ok(()), "{}", preset.file);
            let (mut l, mut r) = (sine(0.013, 4410), sine(0.0021, 4410));
            for (l, r) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
                plugin.process(l, r);
            }
            let peak = l.iter().chain(&r).fold(0f32, |m, v| m.max(v.abs()));
            assert!(
                peak.is_finite() && peak > 0.01 && peak < 8.0,
                "{}",
                preset.file
            );
        }
        let named = |file: &str| {
            let preset = all.iter().find(|p| p.file == file).unwrap();
            with(std::array::from_fn(|i| {
                i32::from_le_bytes(preset.state[4 + 4 * i..8 + 4 * i].try_into().unwrap())
            }))
        };
        assert_eq!(named("Flat"), SevenBandEq::default());
        assert_eq!(
            named("Telephone").params,
            [-1800, -1800, -56, 1800, 1181, -900, -1800]
        );
    }
}
