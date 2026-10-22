//! Transporter: the shell of it only, its state and what it hands the
//! engine, as `docs/plugins/transporter.md` has it. It is no
//! [`crate::Effect`] yet and nothing hosts it.
//!
//! `Transporter_x64.dll` is a Delphi shell. The sound is made by
//! `TransientLooper`, an object it gets from `TransientLooper_CreateInstance`
//! in `Shared/ildsp_x64.dll`: a looper that cuts the input at its
//! transients (or on the song's beat) into two loops, A and B, and plays
//! them back reversed, an octave up or down and panned, at random or in
//! turn. The install this was read from has no `Shared/` directory, so
//! that object is not mapped; `.scratch/notes/transporter.md` says what
//! is left.
//!
//! The state is a little-endian i32 version, 8 the newest FL reads, and
//! the 22 parameters as i32 (20 under version 8), then a byte for "Flush
//! on pause/stop", a byte for "Lookahead", an i32 for the transient source
//! (0 left, 1 right, 2 both), an i32 for the architecture (0 mono to
//! stereo, 1 stereo to stereo), and the output and the input send track as
//! i32. FL reads what a shorter state holds and leaves the rest as it was.
//!
//! | parameter | range | sent to the engine as |
//! |---|---|---|
//! | 0, 1, 2 Dry level, Loop level A, Loop level B | 0 to 12800 | dB: `20 log10((11^v - 1) / 10)`, 0 at the top |
//! | 3, 4, 5 Reverse, Octave shift, Octave down/up bias | 0 to 10000 | `v` |
//! | 6 Spread and auto panning | 0 to 1024 | `2 (v - 0.5)` |
//! | 7 Minimum length A | 20 to 2000 (ms) | seconds, `1.98 v + 0.02` |
//! | 8 A/B ratio | 0 to 36 | `9 v + 1` |
//! | 9 Transient sensitivity | 0 to 1024 | `v` |
//! | 10 Bouncing rate | 0 to 1024 | `2 (v - 0.5)` |
//! | 11 Ignore transients, 12 Flush, 14 Bypass, 15 to 17 Update loop A, B, A/B | 0 to 1 | the value |
//! | 13 Loop mode: Auto, Bouncing, Adaptive, Tempo | 0 to 3 | the value |
//! | 18 Probabilistic, Deterministic | 0 to 1 | the value |
//! | 19 Tempo ratio length, 1/1 to 1/16 quint. | 0 to 20 | the value |
//! | 20 Tempo ratio trigger, the same and Off | 0 to 21 | the value |
//! | 21 Ignore bias | 0 to 10000 | `v` |
//!
//! `v` is the wheel over its range, from 0 to 1.

use crate::StateError;

/// The plugin's event 201 name.
pub const NAME: &str = "Transporter";

/// How many parameters the plugin has.
pub const PARAMS: usize = 22;

/// The range of each parameter's control, from the editor's form.
pub const RANGES: [(i32, i32); PARAMS] = [
    (0, 12_800),
    (0, 12_800),
    (0, 12_800),
    (0, 10_000),
    (0, 10_000),
    (0, 10_000),
    (0, 1024),
    (20, 2000),
    (0, 36),
    (0, 1024),
    (0, 1024),
    (0, 1),
    (0, 1),
    (0, 3),
    (0, 1),
    (0, 1),
    (0, 1),
    (0, 1),
    (0, 1),
    (0, 20),
    (0, 21),
    (0, 10_000),
];

/// The parameters of the render-test project, which a new plugin saves.
/// The last two are not in a state of that version: 20 is the editor's
/// default, as the engine's own is not known, and 21 is what the loader
/// sets for an older state.
pub const DEFAULTS: [i32; PARAMS] = [
    12_800, 12_800, 12_800, 0, 0, 0, 512, 500, 8, 512, 512, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// The newest version FL reads, and the one it writes.
const VERSION: i32 = 8;

/// The parameter bytes under version 8, and from it on.
const OLD_PARAMS: usize = 20;

/// The most a state holds: version 8 in full.
const LEN: usize = 4 + 4 * PARAMS + 2 + 4 * 4;

/// What the plugin keeps from its state.
#[derive(Debug, Clone, PartialEq)]
pub struct Transporter {
    /// The parameters, as the controls have them.
    pub params: [i32; PARAMS],
    /// "Flush on pause/stop".
    pub flush_on_stop: bool,
    /// "Lookahead", as saved. FL's loader reads it and drops it, so a
    /// loaded state leaves the engine's own setting alone.
    pub lookahead: bool,
    /// Where the transients are heard: 0 left, 1 right, 2 both.
    pub transient_source: i32,
    /// 0 mono to stereo, 1 stereo to stereo.
    pub architecture: i32,
    /// The send track the second output goes to, 0 for none.
    pub sidechain_out: i32,
    /// The send track the transients are heard on, 0 for the input.
    pub sidechain_in: i32,
}

impl Default for Transporter {
    /// As the constructor (`FUN_00712760`) leaves the shell.
    fn default() -> Self {
        Self {
            params: DEFAULTS,
            flush_on_stop: true,
            lookahead: true,
            transient_source: 2,
            architecture: 1,
            sidechain_out: 0,
            sidechain_in: 0,
        }
    }
}

/// Little-endian i32 at `at`, if the state is long enough.
fn word(bytes: &[u8], at: usize) -> Option<i32> {
    let word = bytes.get(at..at + 4)?;
    Some(i32::from_le_bytes(word.try_into().ok()?))
}

impl Transporter {
    /// Take the bytes of the plugin's event 213 as `SaveRestoreState`
    /// (`FUN_00713760`) reads them. A version over 8 is left alone. What a
    /// short state lacks stays as it was, as FL's stream reads leave it,
    /// but for the Ignore bias, which an older version sets to 0.
    pub fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let version = crate::state_version(bytes, LEN)?;
        if version > VERSION {
            return Err(StateError::Version { found: version });
        }
        let count = if version < VERSION {
            OLD_PARAMS
        } else {
            PARAMS
        };
        let mut params = self.params;
        if version < VERSION {
            params[21] = 0;
        }
        let mut at = 4;
        for param in params.iter_mut().take(count) {
            if let Some(value) = word(bytes, at) {
                *param = value;
            }
            at += 4;
        }
        self.params = params;
        if let Some(byte) = bytes.get(at) {
            self.flush_on_stop = *byte != 0;
        }
        if let Some(byte) = bytes.get(at + 1) {
            self.lookahead = *byte != 0;
        }
        at += 2;
        let fields = [
            &mut self.transient_source,
            &mut self.architecture,
            &mut self.sidechain_out,
            &mut self.sidechain_in,
        ];
        for field in fields {
            if let Some(value) = word(bytes, at) {
                *field = value;
            }
            at += 4;
        }
        Ok(())
    }

    /// What `ProcessParam` (`FUN_00713a40`, then `FUN_00713d30`) hands
    /// the engine for parameter `index`: the value the engine's
    /// `SetParam` takes.
    pub fn engine_value(&self, index: usize) -> f32 {
        let value = self.params[index];
        let v = normal(value, RANGES[index]);
        match index {
            0..=2 => level(v),
            7 => (f64::from(v) * 1.98 + 0.02) as f32,
            8 => v * 9.0 + 1.0,
            6 | 10 => ((f64::from(v) - 0.5) * 2.0) as f32,
            3..=5 | 9 | 21 => v,
            _ => value as f32,
        }
    }
}

/// A control's value over its range, from 0 to 1, as the shell works it
/// out in doubles and hands it on as an f32.
fn normal(value: i32, (min, max): (i32, i32)) -> f32 {
    let v = f64::from(value - min) * (1.0 / f64::from(max - min));
    v.clamp(0.0, 1.0) as f32
}

/// A level wheel's `v` in dB (`FUN_007125f0`): FL's volume curve
/// `(11^v - 1) / 10`, with the top exactly 1 and anything at or under 0
/// minus infinity.
fn level(v: f32) -> f32 {
    let gain = if v == 1.0 {
        1.0
    } else {
        let ln11 = 11f64.ln() as f32;
        ((f64::from(v * ln11).exp() - 1.0) * f64::from(0.1f32)) as f32
    };
    if gain <= 0.0 {
        f32::NEG_INFINITY
    } else {
        (f64::from(gain).log10() * 20.0) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{corpus, every_preset, presets, projects, state};

    #[test]
    fn the_levels_follow_fl_s_volume_curve() {
        let at = |value: i32| {
            let mut plugin = Transporter::default();
            plugin.params[0] = value;
            plugin.engine_value(0)
        };
        assert_eq!(at(12_800), 0.0);
        assert_eq!(at(0), f32::NEG_INFINITY);
        // Half way is (sqrt(11) - 1) / 10.
        let half = 20.0 * ((11f64.sqrt() - 1.0) / 10.0).log10();
        assert!((f64::from(at(6400)) - half).abs() < 1e-4, "{}", at(6400));
    }

    #[test]
    fn the_other_wheels_scale_as_the_shell_scales_them() {
        let mut plugin = Transporter::default();
        assert!((plugin.engine_value(7) - 0.5).abs() < 1e-6);
        assert_eq!(plugin.engine_value(8), 3.0);
        assert_eq!(plugin.engine_value(6), 0.0);
        assert_eq!(plugin.engine_value(9), 0.5);
        plugin.params[7] = 2000;
        plugin.params[10] = 0;
        plugin.params[20] = 21;
        assert_eq!(plugin.engine_value(7), 2.0);
        assert_eq!(plugin.engine_value(10), -1.0);
        assert_eq!(plugin.engine_value(20), 21.0);
    }

    #[test]
    fn a_short_state_leaves_what_it_lacks() {
        let mut plugin = Transporter::default();
        plugin.params[20] = 7;
        plugin.params[21] = 3000;
        plugin.transient_source = 0;
        let mut bytes = state(&[3]);
        bytes.extend(state(&[1; 20]));
        bytes.push(0);
        assert_eq!(plugin.set_state(&bytes), Ok(()));
        assert_eq!(plugin.params[..20], [1; 20]);
        assert_eq!((plugin.params[20], plugin.params[21]), (7, 0));
        assert!(!plugin.flush_on_stop && plugin.lookahead);
        assert_eq!(plugin.transient_source, 0);
        let before = plugin.clone();
        assert_eq!(
            plugin.set_state(&state(&[9])),
            Err(StateError::Version { found: 9 })
        );
        assert_eq!(plugin, before);
    }

    #[test]
    fn fl_s_states_load_as_the_doc_reads_them() {
        let Some(root) = corpus() else { return };
        let Some(own) = presets(NAME) else { return };
        let all = every_preset(&root, NAME);
        // Six of the plugin's own, four copies of the default.
        assert_eq!((own.len(), all.len()), (6, 10));
        let used: Vec<_> = projects(&root)
            .into_iter()
            .filter(|saved| saved.plugin.eq_ignore_ascii_case(NAME))
            .collect();
        assert_eq!(used.len(), 2);
        for saved in all.iter().chain(&used) {
            let mut plugin = Transporter::default();
            assert_eq!(plugin.set_state(&saved.state), Ok(()), "{}", saved.file);
            assert!(matches!(saved.state.len(), 85 | 94 | 110), "{}", saved.file);
            for (index, value) in plugin.params.iter().enumerate() {
                let (min, max) = RANGES[index];
                assert!((min..=max).contains(value), "{} {index}", saved.file);
                assert!(!plugin.engine_value(index).is_nan());
            }
        }
        // The render test is the default, and the demo song a version 8.
        let test = used.iter().find(|s| s.state.len() == 85).unwrap();
        let mut plugin = Transporter::default();
        plugin.set_state(&test.state).unwrap();
        assert_eq!(plugin, Transporter::default());
        let song = used.iter().find(|s| s.state.len() == 110).unwrap();
        plugin.set_state(&song.state).unwrap();
        assert_eq!(
            plugin.params,
            [
                11_000, 5362, 5675, 4969, 0, 0, 499, 2000, 12, 1024, 512, 0, 0, 1, 0, 0, 0, 0, 0,
                1, 21, 0
            ]
        );
        assert!(plugin.flush_on_stop && plugin.lookahead);
        assert_eq!((plugin.transient_source, plugin.architecture), (2, 1));
        assert_eq!((plugin.sidechain_out, plugin.sidechain_in), (0, 0));
    }
}
