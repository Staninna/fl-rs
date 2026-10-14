//! Soundgoodizer: a Maximus with one knob, as `docs/plugins/maximus.md`
//! has it.
//!
//! The DLL is an older build of Maximus. Its state is three little-endian
//! i32: a version, which is 3, the preset, A to D as 0 to 3, and the
//! amount, 0 to 1000 with 600 the default. Loading a state loads the
//! preset, which is one of the four Maximus states FL ships beside the
//! plugin as `Data/Presets/a.dat` to `d.dat` and again as Maximus's
//! "Soundgoodizer presets", and then puts the amount on Maximus's LMH mix:
//! the knob blends the three processed bands against the delayed input.
//!
//! The four presets are in [`PRESETS`], as their parameters and the
//! compression curve of each band. Everything else is [`Maximus`].

use crate::maximus::{Maximus, PARAMS};
use crate::{Effect, Fidelity, StateError, state_version};

/// The plugin's event 201 name.
pub const NAME: &str = "Soundgoodizer";

/// The amount goes on this Maximus parameter.
const MIX: usize = 5;
const STATE_LEN: usize = 12;
const DEFAULT_AMOUNT: i32 = 600;

/// One preset: Maximus's parameters and the compression curve of each
/// band, each point the width of the stretch that ends at it, its height
/// and that stretch's tension.
pub struct Preset {
    pub params: [i32; PARAMS],
    pub curves: [&'static [(f64, f32, f32)]; 4],
}

/// The presets A to D.
#[rustfmt::skip]
pub const PRESETS: [Preset; 4] = [
    // A
    Preset {
        params: [
            19800, 1, 48065, 1, 891, 1000, 32768,
            0, 1264, 1296, 1000, -1000, 1284, 1115, 0, 2, 5736, 3, 2882, 0,
            0, 1340, 1000, -384, 0, 1000, 1115, 0, 2, 5000, 3, 1570, 0,
            0, 1364, 1152, 0, -1000, 1000, 1115, 0, 2, 5000, 3, 1186, 0,
            0, 1000, 1000, 0, -196, 1000, 1115, 4464, 2, 5000, 3, 1538, 0,
        ],
        curves: [
            &[(0.0, 0.0, 0.0), (0.5, 0.5, 0.07), (0.5, 0.5, 0.0)],
            &[(0.0, 0.0, 0.0), (0.4000000059604645, 0.4, 0.0), (0.6000000238418579, 0.6, 0.18999997)],
            &[(0.0, 0.0, 0.0), (0.5, 0.5, 0.14000002), (0.5, 0.5, 0.0)],
            &[(0.0, 0.0, 0.0), (0.5, 0.5, 0.0), (0.5, 0.6, 0.08500001)],
        ],
    },
    // B
    Preset {
        params: [
            19800, 1, 48065, 1, 1115, 1000, 32768,
            1, 1000, 1448, 564, -1000, 1000, 1115, 0, 2, 5000, 3, 2882, 0,
            1, 1000, 1564, -228, -240, 1384, 1115, 0, 2, 5000, 3, 2882, 0,
            1, 1000, 1496, 0, -1000, 1592, 1115, 0, 2, 5000, 3, 2882, 0,
            0, 1000, 1000, 0, -296, 1000, 1115, 4464, 2, 5000, 3, 2882, 0,
        ],
        curves: [
            &[(0.0, 0.0, 0.0), (0.5, 0.5, 0.0), (0.5, 0.5, 0.0)],
            &[(0.0, 0.0, 0.0), (0.5, 0.5, 0.0), (0.5, 0.5, 0.0)],
            &[(0.0, 0.0, 0.0), (0.5, 0.5, 0.0), (0.5, 0.5, 0.0)],
            &[(0.0, 0.0, 0.0), (0.3499999940395355, 0.35, 0.0), (0.6500000059604645, 0.6, 0.15500002)],
        ],
    },
    // C
    Preset {
        params: [
            19800, 1, 43329, 0, 891, 1000, 32768,
            0, 1456, 1076, 752, -1000, 1112, 1115, 0, 2, 5736, 3, 2882, 0,
            0, 1692, 1144, -348, 0, 1000, 1115, 0, 2, 5000, 3, 1570, 0,
            0, 1612, 1152, 0, 0, 1000, 1115, 0, 2, 5000, 3, 1186, 0,
            0, 1000, 1000, 0, 0, 1000, 1115, 4464, 2, 5000, 3, 2882, 0,
        ],
        curves: [
            &[(0.0, 0.0, 0.0), (0.5, 0.5, 0.035000004), (0.5, 0.5, 0.0)],
            &[(0.0, 0.0, 0.0), (0.4000000059604645, 0.4, 0.0), (0.6000000238418579, 0.6, 0.18999997)],
            &[(0.0, 0.0, 0.0), (0.5, 0.5, 0.044999998), (0.5, 0.5, 0.0)],
            &[(0.0, 0.0, 0.0), (0.4000000059604645, 0.4, 0.0), (0.5999999940395355, 0.5, 0.35999987)],
        ],
    },
    // D
    Preset {
        params: [
            19800, 1, 48065, 1, 1115, 1000, 32768,
            3, 1000, 1000, 0, 0, 1000, 1115, 0, 2, 5000, 3, 2882, 0,
            0, 1772, 1000, 0, 0, 1000, 1115, 3296, 2, 5000, 4, 2015, 0,
            3, 1000, 1000, 0, 0, 1000, 1115, 0, 2, 5000, 3, 2882, 0,
            0, 1000, 1000, 0, 0, 1000, 1115, 5184, 2, 5000, 3, 2882, 0,
        ],
        curves: [
            &[(0.0, 0.0, 0.0), (0.5, 0.5, 0.0), (0.5, 0.5, 0.0)],
            &[(0.0, 0.0, 0.0), (0.5, 0.5, 0.0), (0.5, 0.6, 0.2649999)],
            &[(0.0, 0.0, 0.0), (0.5, 0.5, 0.0), (0.5, 0.5, 0.0)],
            &[(0.0, 0.0, 0.0), (0.5, 0.5, 0.0), (0.5, 0.5, 0.0)],
        ],
    },
];

/// Soundgoodizer. A new one is preset A at an amount of 600.
#[derive(Debug, Clone, PartialEq)]
pub struct Soundgoodizer {
    inner: Maximus,
    preset: usize,
    amount: i32,
}

impl Default for Soundgoodizer {
    fn default() -> Self {
        let mut plugin = Soundgoodizer {
            inner: Maximus::default(),
            preset: 0,
            amount: DEFAULT_AMOUNT,
        };
        plugin.load();
        plugin
    }
}

impl Soundgoodizer {
    /// Load the preset and put the amount on the mix.
    fn load(&mut self) {
        let preset = &PRESETS[self.preset];
        self.inner.load_preset(preset.params, preset.curves);
        self.inner.set_param(MIX, self.amount as f32);
    }
}

impl Effect for Soundgoodizer {
    /// FL reads nothing of a version under 1 and keeps what it has.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let version = state_version(bytes, STATE_LEN)?;
        if bytes.len() != STATE_LEN {
            return Err(StateError::Length {
                expected: STATE_LEN,
                found: bytes.len(),
            });
        }
        if version < 1 {
            return Ok(());
        }
        let word = |at: usize| i32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let (preset, amount) = (word(4), word(8));
        if !(0..4).contains(&preset) {
            return Err(StateError::Range {
                param: 0,
                value: preset,
            });
        }
        if !(0..=1000).contains(&amount) {
            return Err(StateError::Range {
                param: 1,
                value: amount,
            });
        }
        self.preset = preset as usize;
        self.amount = amount;
        self.load();
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        self.inner.set_sample_rate(hz);
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.inner.process(left, right);
    }

    /// Maximus's parameters, as FL's Soundgoodizer has them: the knob is
    /// the LMH mix.
    fn param_count(&self) -> usize {
        self.inner.param_count()
    }

    fn param(&self, index: usize) -> f32 {
        self.inner.param(index)
    }

    fn set_param(&mut self, index: usize, value: f32) {
        self.inner.set_param(index, value);
        if index == MIX {
            self.amount = self.inner.param(MIX) as i32;
        }
    }

    fn latency(&self) -> usize {
        self.inner.latency()
    }

    fn max_latency(&self) -> usize {
        self.inner.max_latency()
    }

    fn reset(&mut self) {
        self.inner.reset();
    }

    fn fidelity(&self) -> Fidelity {
        self.inner.fidelity()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, state};

    #[test]
    fn a_state_is_a_preset_and_an_amount_on_the_mix() {
        let mut plugin = Soundgoodizer::default();
        plugin.set_sample_rate(48_000.0);
        assert_eq!((plugin.param(5), plugin.param(4)), (600.0, 891.0));
        for preset in 0..4 {
            plugin.set_state(&state(&[3, preset, 250])).unwrap();
            let params: Vec<f32> = (0..PARAMS).map(|i| plugin.param(i)).collect();
            let mut want = PRESETS[preset as usize].params.map(|v| v as f32);
            want[MIX] = 250.0;
            assert_eq!(params, want, "preset {preset}");
            assert_eq!(plugin.fidelity(), Fidelity::Exact);
            // The preset's own Maximus state loads to the same plugin.
            let mut maximus = Maximus::default();
            maximus.set_sample_rate(48_000.0);
            maximus.load_preset(
                PRESETS[preset as usize].params,
                PRESETS[preset as usize].curves,
            );
            maximus.set_param(MIX, 250.0);
            assert_eq!(plugin.inner, maximus);
        }
        // The knob is parameter 5.
        plugin.set_param(MIX, 1000.0);
        assert_eq!((plugin.amount, plugin.param(MIX)), (1000, 1000.0));
        // D is a one-band preset with no latency but the lookahead.
        plugin.set_state(&state(&[3, 3, 600])).unwrap();
        assert_eq!(plugin.latency(), 2 * 96);
        // Wrong states.
        let before = plugin.clone();
        assert_eq!(
            plugin.set_state(&state(&[3, 4, 600])),
            Err(StateError::Range { param: 0, value: 4 })
        );
        assert_eq!(
            plugin.set_state(&state(&[3, 0, 1001])),
            Err(StateError::Range {
                param: 1,
                value: 1001
            })
        );
        assert_eq!(
            plugin.set_state(&state(&[3, 0])),
            Err(StateError::Length {
                expected: 12,
                found: 8
            })
        );
        assert_eq!(plugin.set_state(&state(&[0, 1, 100])), Ok(()));
        assert_eq!(plugin, before);
    }

    #[test]
    fn the_presets_are_the_states_fl_ships_beside_the_plugin() {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, the presets of {NAME} skipped");
            return;
        };
        let shipped = testing::presets("Maximus").unwrap();
        for (k, name) in ["a", "b", "c", "d"].into_iter().enumerate() {
            let saved = shipped.iter().find(|p| p.file == name).unwrap();
            let mut maximus = Maximus::default();
            maximus.set_sample_rate(48_000.0);
            maximus.set_state(&saved.state).unwrap();
            let mut ours = Maximus::default();
            ours.set_sample_rate(48_000.0);
            ours.load_preset(PRESETS[k].params, PRESETS[k].curves);
            // The same parameters, the same latency and the same sound.
            // The display's part of the state is not in the presets here.
            let params =
                |plugin: &Maximus| -> Vec<f32> { (0..PARAMS).map(|i| plugin.param(i)).collect() };
            assert_eq!(params(&maximus), params(&ours), "preset {name}");
            assert_eq!(maximus.latency(), ours.latency(), "preset {name}");
            let tone: Vec<f32> = testing::sine(0.013, 8192).iter().map(|v| v * 1.5).collect();
            let (mut left, mut right) = (tone.clone(), tone.clone());
            maximus.process(&mut left, &mut right);
            let (mut l, mut r) = (tone.clone(), tone);
            ours.process(&mut l, &mut r);
            assert!(left == l && right == r, "preset {name}");
        }
        // Every Soundgoodizer in FL's projects loads and runs.
        let named = |saved: &testing::Saved| saved.plugin.eq_ignore_ascii_case(NAME);
        let projects: Vec<_> = testing::projects(&root).into_iter().filter(named).collect();
        assert!(!projects.is_empty());
        for saved in &projects {
            let mut plugin = Soundgoodizer::default();
            plugin.set_sample_rate(48_000.0);
            if let Err(error) = plugin.set_state(&saved.state) {
                panic!("{}: {error}", saved.file);
            }
            let (mut left, mut right) = (testing::sine(0.01, 2048), testing::sine(0.037, 2048));
            for (l, r) in left.chunks_mut(300).zip(right.chunks_mut(300)) {
                plugin.process(l, r);
            }
            assert!(
                left.iter()
                    .chain(&right)
                    .all(|v| v.is_finite() && v.abs() < 16.0),
                "{}",
                saved.file
            );
        }
    }
}
