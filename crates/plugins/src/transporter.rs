//! Transporter: FL's transient looper, as `docs/plugins/transporter.md`
//! has it.
//!
//! `Transporter_x64.dll` is a Delphi shell ([`Shell`]: the state, and the
//! values it hands its engine). The sound is made by `TransientLooper`,
//! an object it gets from `TransientLooper_CreateInstance` in
//! `Shared/ildsp_x64.dll`: a Faust program that cuts the input at its
//! transients (or on the song's beat) into two loops, A and B, and plays
//! them back reversed, an octave up or down and panned, by chance or in
//! turn, under the dry signal. `transient_looper.rs` rebuilds it, matched
//! to the DLL's own code under an emulator.
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
//!
//! # Sound
//!
//! The shell's `Eff_Render` (`0x00714700`) hands the engine the block,
//! the transient source (the input send track's sound, or its own input
//! when it has none), and the place in a 4-beat bar, which only Tempo mode
//! with a ratio trigger reads; the engine writes the block back and its
//! two loops to the second output, which goes to the output send track.
//! The engine is late by its 5 ms lookahead, which the shell reports.

use crate::transient_looper::TransientLooper;
use crate::{Effect, Fidelity, Sidechain, StateError, knob_value};

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
/// The last two are not in a state of that version: 20 is the engine's
/// default (its tempo multiplier's 21, Off), and 21 is what the loader
/// sets for an older state.
pub const DEFAULTS: [i32; PARAMS] = [
    12_800, 12_800, 12_800, 0, 0, 0, 512, 500, 8, 512, 512, 0, 0, 0, 0, 0, 0, 0, 0, 0, 21, 0,
];

/// The newest version FL reads, and the one it writes.
const VERSION: i32 = 8;

/// The parameter bytes under version 8, and from it on.
const OLD_PARAMS: usize = 20;

/// The most a state holds: version 8 in full.
const LEN: usize = 4 + 4 * PARAMS + 2 + 4 * 4;

/// What the plugin keeps from its state.
#[derive(Debug, Clone, PartialEq)]
pub struct Shell {
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

impl Default for Shell {
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

impl Shell {
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
    let v = (f64::from(value) - f64::from(min)) * (1.0 / f64::from(max - min));
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

/// The longest block the plugin keeps its second output for without
/// asking for memory. A longer one asks once.
const ROOM: usize = 0x4000;

/// Transporter, the shell over its engine.
pub struct Transporter {
    shell: Shell,
    engine: Box<TransientLooper>,
    hz: f32,
    ppq: u16,
    /// The place in a 4-beat bar the shell hands the engine, in beats.
    beat: f32,
    /// Flush as last sent (`0x130`), -1 before.
    flush: i32,
    side: Sidechain,
    aux: [Vec<f32>; 2],
}

impl Default for Transporter {
    /// A new plugin at 44.1 kHz: the render test's state. This allocates
    /// the loops, 20 seconds each.
    fn default() -> Self {
        let mut plugin = Transporter {
            shell: Shell::default(),
            engine: Box::new(TransientLooper::new()),
            hz: 44_100.0,
            ppq: 96,
            beat: 0.0,
            flush: -1,
            side: Sidechain::default(),
            aux: [Vec::with_capacity(ROOM), Vec::with_capacity(ROOM)],
        };
        plugin.engine.set_rate(plugin.hz);
        plugin.apply();
        plugin
    }
}

impl Transporter {
    /// `ProcessParam` hands the engine parameter `index`. Flush goes only
    /// when it has changed and Bypass is off.
    fn send(&mut self, index: usize) {
        if index == 12 {
            let value = self.shell.params[12];
            if self.shell.params[14] != 0 || value == self.flush {
                return;
            }
            self.flush = value;
        }
        let value = self.shell.engine_value(index);
        self.engine.set_param(index, value);
    }

    /// What a load does after the state is read: the transient source and
    /// the architecture, then every parameter.
    fn apply(&mut self) {
        self.engine
            .set_param(24, self.shell.transient_source as f32);
        self.engine.set_param(25, self.shell.architecture as f32);
        for index in 0..PARAMS {
            self.send(index);
        }
    }

    /// Whether the tempo's beat starts loops: Tempo mode with a ratio
    /// trigger that is not Off.
    fn on_the_beat(&self) -> bool {
        self.shell.params[13] == 3 && self.shell.params[20] != 21
    }
}

impl Effect for Transporter {
    /// The loops keep their sound, as FL's do.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        self.shell.set_state(bytes)?;
        self.apply();
        Ok(())
    }

    /// The engine anew at the rate, which empties the loops and turns its
    /// lookahead on, then the state's source and architecture again
    /// (`inferred`: the shell's rate handler is not read). This allocates.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz >= 1.0 && hz != self.hz {
            self.hz = hz;
            self.engine.set_rate(hz);
            self.engine
                .set_param(24, self.shell.transient_source as f32);
            self.engine.set_param(25, self.shell.architecture as f32);
        }
    }

    /// Engine parameter 22, which Tempo mode's loop length follows.
    fn set_tempo(&mut self, bpm: f64) {
        if bpm > 0.0 && bpm.is_finite() {
            self.engine.set_param(22, bpm as f32);
        }
    }

    fn set_ppq(&mut self, ppq: u16) {
        if ppq > 0 {
            self.ppq = ppq;
        }
    }

    /// The song's place in ticks over the PPQ and 4, its fraction times 4:
    /// the beat in the bar, which the shell fills the engine's beat input
    /// with for the block (`inferred`: the fill routine is not read).
    fn tick(&mut self, ppq_tick: u64, _tempo: f64) {
        let bars = ppq_tick as f64 / f64::from(self.ppq) / 4.0;
        self.beat = (bars.fract() * 4.0) as f32;
    }

    fn follows_song(&self) -> bool {
        self.on_the_beat()
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        let (left, right) = (&mut left[..len], &mut right[..len]);
        let [aux_l, aux_r] = &mut self.aux;
        aux_l.resize(len, 0.0);
        aux_r.resize(len, 0.0);
        let side = (self.shell.sidechain_in > 0 && self.side.left.len() >= len)
            .then(|| (&self.side.left[..len], &self.side.right[..len]));
        self.engine
            .render(left, right, side, self.beat, (aux_l, aux_r));
        self.side.clear();
    }

    fn param_count(&self) -> usize {
        PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        self.shell
            .params
            .get(index)
            .map_or(0.0, |value| *value as f32)
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(range) = RANGES.get(index) {
            self.shell.params[index] = knob_value(value, *range);
            self.send(index);
        }
    }

    /// The song stops: with "Flush on pause/stop" the engine is flushed
    /// and both loops fade out and empty (`FUN_007133a0`).
    fn reset(&mut self) {
        self.side.clear();
        if self.shell.flush_on_stop {
            self.engine.clear();
            self.engine.set_param(12, 1.0);
        }
    }

    fn fidelity(&self) -> Fidelity {
        if self.on_the_beat() {
            Fidelity::Approximate(
                "Transporter's tempo trigger reads the bar's place once a block, as inferred",
            )
        } else {
            Fidelity::Exact
        }
    }

    fn latency(&self) -> usize {
        self.engine.latency()
    }

    fn max_latency(&self) -> usize {
        self.engine.max_latency()
    }

    fn aux_output(&self) -> Option<(&[f32], &[f32])> {
        (self.shell.sidechain_out > 0).then(|| (self.aux[0].as_slice(), self.aux[1].as_slice()))
    }

    fn sidechain_output(&self) -> usize {
        self.shell.sidechain_out.max(0) as usize
    }

    fn sidechain_input(&self) -> usize {
        self.shell.sidechain_in.max(0) as usize
    }

    fn set_sidechain(&mut self, index: usize, left: &[f32], right: &[f32]) {
        if index > 0 && index == self.sidechain_input() {
            self.side.keep(left, right);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{corpus, every_preset, presets, projects, state};
    use crate::{Effect, Fidelity};

    #[test]
    fn the_levels_follow_fl_s_volume_curve() {
        let at = |value: i32| {
            let mut plugin = Shell::default();
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
        let mut plugin = Shell::default();
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
        let mut plugin = Shell::default();
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

    /// Bursts of noise a quarter second apart over a quiet tone.
    fn bursts(len: usize) -> (Vec<f32>, Vec<f32>) {
        let mut seed = 1u32;
        (0..len)
            .map(|i| {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                let noise = (seed >> 9) as f32 / (1u32 << 23) as f32 - 1.0;
                let env = (-((i % 11025) as f32) / 600.0).exp();
                let v = 0.4 * env * noise + 0.05 * (i as f32 * 0.02).sin();
                (v, v * 0.8)
            })
            .unzip()
    }

    #[test]
    fn the_render_test_plays_its_loops_under_the_input_late_by_the_lookahead() {
        let mut plugin = Transporter::default();
        assert_eq!(plugin.latency(), 220);
        assert_eq!(plugin.fidelity(), Fidelity::Exact);
        plugin.set_sample_rate(48_000.0);
        assert_eq!((plugin.latency(), plugin.max_latency()), (240, 240));
        let (mut l, mut r) = bursts(96_000);
        let (dry_l, _) = (l.clone(), r.clone());
        for (l, r) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
            plugin.process(l, r);
        }
        assert!(l.iter().chain(&r).all(|v| v.is_finite() && v.abs() < 4.0));
        // What is not the input late by 240 frames is the loops.
        let late = |at: usize| l[at] - dry_l[at - 240];
        assert!((40_000..96_000).any(|at| late(at).abs() > 0.05));
        let rms = (l.iter().map(|v| v * v).sum::<f32>() / l.len() as f32).sqrt();
        assert!((0.05..0.5).contains(&rms), "{rms}");
        // No send track: no second output.
        assert_eq!(plugin.aux_output(), None);
    }

    #[test]
    fn the_demo_song_s_state_bounces_its_loops_and_sends_them_on() {
        let mut bytes = state(&[8]);
        bytes.extend(state(&[
            11_000, 5362, 5675, 4969, 0, 0, 499, 2000, 12, 1024, 512, 0, 0, 1, 0, 0, 0, 0, 0, 1,
            21, 0,
        ]));
        bytes.extend([1, 1]);
        bytes.extend(state(&[2, 1, 3, 0]));
        let mut plugin = Transporter::default();
        plugin.set_state(&bytes).unwrap();
        assert_eq!(plugin.sidechain_output(), 3);
        assert!(!plugin.follows_song());
        let (mut l, mut r) = bursts(44_100 * 6);
        for (l, r) in l.chunks_mut(441).zip(r.chunks_mut(441)) {
            plugin.process(l, r);
        }
        assert!(l.iter().chain(&r).all(|v| v.is_finite()));
        let (al, ar) = plugin.aux_output().unwrap();
        assert_eq!((al.len(), ar.len()), (441, 441));
        assert!(al.iter().chain(ar).any(|v| *v != 0.0));
        // Tempo mode with a trigger follows the song's beat.
        plugin.set_param(13, 3.0);
        plugin.set_param(20, 2.0);
        assert!(plugin.follows_song());
        assert!(matches!(plugin.fidelity(), Fidelity::Approximate(_)));
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
            let mut plugin = Shell::default();
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
        let mut plugin = Shell::default();
        plugin.set_state(&test.state).unwrap();
        assert_eq!(plugin, Shell::default());
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
