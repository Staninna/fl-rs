//! Fruity Stereo Enhancer: a 2x2 mix that pulls the sides apart or
//! together, and a short delay on one of them, as
//! `docs/plugins/fruity-stereo-enhancer.md` has it.
//!
//! The state is six little-endian i32, the plugin's six parameters:
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Panning | -128 (left) to 128 (right) | 0 |
//! | 1 Volume | 0 to 320 | 256 |
//! | 2 Stereo separation | -96 (apart) to 96 (merged) | 0 |
//! | 3 Phase offset | -512 (left late) to 512 (right late) | 0 |
//! | 4 Phase effect position | 0 before the mix, 1 after it | 1 |
//! | 5 Phase inversion | 0 none, 1 left, 2 right | 0 |
//!
//! With `s` the separation over 128 and `g` the volume, each side comes
//! out as `g (1 - s / 2) (own + s * other)`. What the two sides share gets
//! the gain `g (1 - s / 2) (1 + s)` and what they differ in gets
//! `g (1 - s / 2) (1 - s)`, so a separation under 0 widens and one above 0
//! narrows. [`gains`] has the pan and the inversion as well.
//!
//! The phase offset holds one side back by a whole number of samples,
//! which grows with the rate so that the time stays.

use crate::parts::{Matrix, offset_frames};
use crate::{Effect, StateError, knob_gain, knob_value, read_state};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Stereo Enhancer";

/// The range of each parameter's control.
const RANGES: [(i32, i32); 6] = [
    (-128, 128),
    (0, 320),
    (-96, 96),
    (-512, 512),
    (0, 1),
    (0, 2),
];
const DEFAULTS: [i32; 6] = [0, 256, 0, 0, 1, 0];

/// The mix for the first, second, third and sixth parameter, as
/// `[LL, RR, RL, LR]`: left to left, right to right, right into left, left
/// into right.
///
/// The pan turns down the output it leans away from and leaves the other
/// as it is. The inversion turns one output upside down.
///
/// At the defaults it is 1, 1, 0, 0 to the bit.
pub fn gains(pan: i32, volume: i32, separation: i32, inversion: i32) -> [f32; 4] {
    let pan = pan as f32 / 128.0;
    // FL has 256 down as 1 without working it out.
    let volume = if volume == 256 {
        1.0
    } else {
        knob_gain(f64::from(volume) / 256.0) as f32
    };
    let separation = separation as f32 / 128.0;

    let own = volume * (1.0 - 0.5 * separation);
    let other = own * separation;
    let (mut left, mut right) = ([own, other], [own, other]);
    if pan < 0.0 {
        right = right.map(|gain| gain * (pan + 1.0));
    } else {
        left = left.map(|gain| gain * (1.0 - pan));
    }
    match inversion {
        1 => left = left.map(|gain| -gain),
        2 => right = right.map(|gain| -gain),
        _ => {}
    }
    [left[0], right[0], left[1], right[1]]
}

/// Fruity Stereo Enhancer. A new one has FL's defaults, which pass a block
/// through untouched.
#[derive(Debug, Clone, PartialEq)]
pub struct StereoEnhancer {
    params: [i32; 6],
    hz: f64,
    matrix: Matrix,
    /// What the late side has yet to play: as many samples as it is late
    /// by, in a ring.
    held: Vec<f32>,
    /// Where in the ring the next sample goes.
    at: usize,
}

impl Default for StereoEnhancer {
    fn default() -> Self {
        let [pan, volume, separation, _, _, inversion] = DEFAULTS;
        let mut plugin = StereoEnhancer {
            params: DEFAULTS,
            hz: 44_100.0,
            matrix: Matrix::new(gains(pan, volume, separation, inversion)),
            held: Vec::new(),
            at: 0,
        };
        plugin.make_room();
        plugin
    }
}

impl StereoEnhancer {
    /// Have the memory for the longest offset the wheel can ask for at this
    /// rate, half a second, so that the wheel does not have to ask for
    /// memory.
    fn make_room(&mut self) {
        let longest = offset_frames(RANGES[3].1, self.hz);
        self.held
            .reserve_exact(longest.saturating_sub(self.held.len()));
    }

    /// Take parameters that are inside their ranges, or a new rate.
    ///
    /// An offset of another length or on the other side starts with
    /// nothing held, as FL's does.
    fn set(&mut self, params: [i32; 6]) {
        let late = offset_frames(params[3], self.hz);
        let moved_over = (params[3] > 0) != (self.params[3] > 0);
        if late != self.held.len() || moved_over {
            self.held.clear();
            self.held.resize(late, 0.0);
            self.at = 0;
        }
        self.params = params;
        let [pan, volume, separation, _, _, inversion] = params;
        self.matrix.aim(gains(pan, volume, separation, inversion));
    }

    /// Hold the late side back: every sample changes places with the one
    /// that came as many samples before it as the offset is long.
    fn hold(&mut self, left: &mut [f32], right: &mut [f32]) {
        if self.held.is_empty() {
            return;
        }
        let late = if self.params[3] > 0 { right } else { left };
        for sample in late {
            std::mem::swap(sample, &mut self.held[self.at]);
            self.at += 1;
            if self.at == self.held.len() {
                self.at = 0;
            }
        }
    }
}

impl Effect for StereoEnhancer {
    /// The mix is the state's from the first sample on, with no ramp.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        self.set(read_state(bytes, &RANGES)?);
        self.matrix.settle();
        Ok(())
    }

    /// Asks for memory when the rate is higher than before. A rate that is
    /// not a number above 0 is not taken.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz > 0.0 {
            self.hz = f64::from(hz);
            self.make_room();
            self.set(self.params);
        }
    }

    /// After a parameter has moved, the mix goes from where the last block
    /// ended to where the parameters put it, in a straight line over this
    /// block.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        let (left, right) = (&mut left[..len], &mut right[..len]);
        let before = self.params[4] == 0;
        if before {
            self.hold(left, right);
        }
        self.matrix.apply(left, right);
        if !before {
            self.hold(left, right);
        }
    }

    fn param_count(&self) -> usize {
        RANGES.len()
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |value| *value as f32)
    }

    /// Moving the phase offset drops what the late side had yet to play,
    /// as in FL.
    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(range) = RANGES.get(index) {
            let mut params = self.params;
            params[index] = knob_value(value, *range);
            self.set(params);
        }
    }

    /// Drops what the late side had yet to play and ends a ramp.
    fn reset(&mut self) {
        self.held.fill(0.0);
        self.at = 0;
        self.matrix.settle();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{bits, presets, state};
    use dsp::gain_to_db;

    fn with(params: [i32; 6]) -> StereoEnhancer {
        let mut plugin = StereoEnhancer::default();
        plugin.set_state(&state(&params)).unwrap();
        plugin
    }

    /// What one frame comes out as, with no offset.
    fn frame(params: [i32; 6], left: f32, right: f32) -> (f32, f32) {
        let (mut l, mut r) = ([left], [right]);
        with(params).process(&mut l, &mut r);
        (l[0], r[0])
    }

    /// The doc's matrix, as `[LL, RR, RL, LR]`.
    fn by_the_doc([pan, volume, separation, _, _, inversion]: [i32; 6]) -> [f64; 4] {
        let pan = f64::from(pan) / 128.0;
        let x = f64::from(volume) / 256.0;
        let vol = if x == 1.0 {
            1.0
        } else {
            (11f64.powf(x) - 1.0) / 10.0
        };
        let sep = f64::from(separation) / 128.0;
        let (mut ll, mut rr) = (vol * (1.0 - 0.5 * sep), vol * (1.0 - 0.5 * sep));
        let (mut rl, mut lr) = (ll * sep, ll * sep);
        if pan < 0.0 {
            rr *= pan + 1.0;
            lr *= pan + 1.0;
        } else {
            ll *= 1.0 - pan;
            rl *= 1.0 - pan;
        }
        if inversion == 1 {
            (ll, rl) = (-ll, -rl);
        }
        if inversion == 2 {
            (rr, lr) = (-rr, -lr);
        }
        [ll, rr, rl, lr]
    }

    /// The doc's offset in samples.
    fn late(value: i32, hz: f64) -> usize {
        let units = (1001f64.powf(f64::from(value.abs()) / 512.0) - 1.0) * 50.0;
        (units * hz / 100_000.0).round() as usize
    }

    /// `len` frames that start with one frame of 1 on both sides, through
    /// the plugin in blocks of 100.
    fn impulse(plugin: &mut StereoEnhancer, len: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut left, mut right) = (vec![0.0; len], vec![0.0; len]);
        (left[0], right[0]) = (1.0, 1.0);
        for (l, r) in left.chunks_mut(100).zip(right.chunks_mut(100)) {
            plugin.process(l, r);
        }
        (left, right)
    }

    /// The frames of a side that are not 0, with what they are.
    fn hits(side: &[f32]) -> Vec<(usize, f32)> {
        let frames = side.iter().copied().enumerate();
        frames.filter(|(_, value)| *value != 0.0).collect()
    }

    fn close(got: f32, want: f64, within: f64) -> bool {
        (f64::from(got) - want).abs() < within
    }

    #[test]
    fn the_defaults_pass_a_stereo_signal_to_the_bit() {
        assert_eq!(gains(0, 256, 0, 0), [1.0, 1.0, 0.0, 0.0]);
        let left = [0.0, -0.0, 1.0, -1.0, 0.3, -1e-30, 7.5, f32::MIN_POSITIVE];
        let right = [0.25, 1e-20, -3.0, 0.1, -0.0, 1e30, -1e-38, 0.5];
        for mut plugin in [StereoEnhancer::default(), with(DEFAULTS)] {
            for _ in 0..2 {
                let (mut l, mut r) = (left, right);
                plugin.process(&mut l, &mut r);
                assert_eq!(bits(&l), bits(&left));
                assert_eq!(bits(&r), bits(&right));
            }
        }
        assert_eq!(StereoEnhancer::default(), with(DEFAULTS));
    }

    #[test]
    fn the_mix_is_the_doc_s_matrix() {
        for params in [
            DEFAULTS,
            [0, 256, -40, 0, 1, 0],
            [-18, 256, -54, 0, 1, 0],
            [22, 318, 48, 0, 1, 0],
            [-128, 100, 96, 0, 1, 1],
            [128, 320, -96, 0, 0, 2],
            [64, 200, 17, 0, 1, 2],
        ] {
            let [pan, volume, separation, _, _, inversion] = params;
            let got = gains(pan, volume, separation, inversion);
            let want = by_the_doc(params);
            for (got, want) in got.iter().zip(want) {
                assert!(close(*got, want, 1e-6), "{params:?}");
            }
            // L' = L * LL + R * RL and R' = R * RR + L * LR.
            let [ll, rr, rl, lr] = want;
            let (left, right) = frame(params, 0.5, -0.25);
            assert!(close(left, 0.5 * ll - 0.25 * rl, 1e-6), "{params:?}");
            assert!(close(right, -0.25 * rr + 0.5 * lr, 1e-6), "{params:?}");
        }
    }

    #[test]
    fn a_separation_under_0_widens_and_one_above_0_narrows() {
        for separation in [-96, -47, -40, -1, 0, 1, 48, 96] {
            let s = f64::from(separation) / 128.0;
            let params = [0, 256, separation, 0, 1, 0];
            // What the sides share, and what they differ in.
            let (mid, also_mid) = frame(params, 1.0, 1.0);
            let (side, minus_side) = frame(params, 1.0, -1.0);
            assert_eq!((mid, side), (also_mid, -minus_side));
            assert!(
                close(mid, (1.0 - s / 2.0) * (1.0 + s), 1e-6),
                "{separation}"
            );
            assert!(
                close(side, (1.0 - s / 2.0) * (1.0 - s), 1e-6),
                "{separation}"
            );
            match separation {
                ..0 => assert!(side > mid, "{separation}"),
                0 => assert_eq!((mid, side), (1.0, 1.0)),
                _ => assert!(side < mid, "{separation}"),
            }
        }
        // All the way apart: the centre at 0.34, 9 dB down, and the sides
        // at 2.41, 7.6 dB up.
        let (mid, _) = frame([0, 256, -96, 0, 1, 0], 1.0, 1.0);
        let (side, _) = frame([0, 256, -96, 0, 1, 0], 1.0, -1.0);
        assert!(close(mid, 0.34, 0.005) && close(side, 2.41, 0.005));
        assert!((gain_to_db(mid) + 9.0).abs() < 0.4 && (gain_to_db(side) - 7.6).abs() < 0.05);
        // All the way merged: the centre at 1.09 and the sides at 0.16.
        let (mid, _) = frame([0, 256, 96, 0, 1, 0], 1.0, 1.0);
        let (side, _) = frame([0, 256, 96, 0, 1, 0], 1.0, -1.0);
        assert!(close(mid, 1.09, 0.005) && close(side, 0.16, 0.005));
    }

    #[test]
    fn the_phase_offset_holds_one_side_back_by_n_samples() {
        // The doc's lengths at 44.1 kHz: 0.19 ms is 8 samples, and the
        // wheel ends at half a second.
        assert_eq!(late(24, 44_100.0), 8);
        assert_eq!(late(-248, 44_100.0), 604);
        assert_eq!(late(512, 44_100.0), 22_050);
        assert!((late(384, 44_100.0) as f64 / 44.1 - 88.0).abs() < 0.5);

        for (offset, hz) in [
            (-24, 44_100.0),
            (24, 44_100.0),
            (-248, 44_100.0),
            (128, 96_000.0),
        ] {
            let n = late(offset, f64::from(hz));
            let mut plugin = with([0, 256, 0, offset, 1, 0]);
            plugin.set_sample_rate(hz);
            let (left, right) = impulse(&mut plugin, n + 250);
            // Under 0 it is the left side that is late, above 0 the right.
            let (on_time, held) = if offset < 0 {
                (right, left)
            } else {
                (left, right)
            };
            assert_eq!(hits(&on_time), [(0, 1.0)], "offset {offset}");
            assert_eq!(hits(&held), [(n, 1.0)], "offset {offset}");
        }
        // The same time is more samples at a higher rate.
        assert_eq!(late(128, 96_000.0), 222);
        assert_eq!(late(128, 44_100.0), 102);

        // With no offset there is no delay at either position.
        for position in [0, 1] {
            let (left, right) = impulse(&mut with([0, 256, 0, 0, position, 0]), 50);
            assert_eq!(
                (hits(&left), hits(&right)),
                (vec![(0, 1.0)], vec![(0, 1.0)])
            );
        }
    }

    #[test]
    fn the_offset_is_before_or_after_the_mix() {
        // Merged half way, each output has 0.75 of its own input and 0.375
        // of the other. The left input alone, 8 samples late.
        let run = |position: i32| {
            let mut plugin = with([0, 256, 64, -24, position, 0]);
            let (mut left, mut right) = (vec![0.0; 20], vec![0.0; 20]);
            left[0] = 1.0;
            plugin.process(&mut left, &mut right);
            (hits(&left), hits(&right))
        };
        // After the mix only the left output is late.
        assert_eq!(run(1), (vec![(8, 0.75)], vec![(0, 0.375)]));
        // Before it the left input is, and so all that the mix makes of
        // it.
        assert_eq!(run(0), (vec![(8, 0.75)], vec![(8, 0.375)]));
    }

    #[test]
    fn the_pan_turns_one_output_down() {
        // Half left: the right output at a half, the left as it was.
        assert_eq!(frame([-64, 256, 0, 0, 1, 0], 1.0, 1.0), (1.0, 0.5));
        assert_eq!(frame([64, 256, 0, 0, 1, 0], 1.0, 1.0), (0.5, 1.0));
        assert_eq!(frame([128, 256, 0, 0, 1, 0], 1.0, 1.0), (0.0, 1.0));
        assert_eq!(frame([-128, 256, 0, 0, 1, 0], 1.0, 1.0), (1.0, 0.0));
        // What the mix puts into that output from the other side goes down
        // with it.
        assert_eq!(frame([-64, 256, 64, 0, 1, 0], 1.0, 0.0), (0.75, 0.1875));
        assert_eq!(frame([-64, 256, 64, 0, 1, 0], 0.0, 1.0), (0.375, 0.375));
    }

    #[test]
    fn the_inversion_turns_one_output_upside_down() {
        assert_eq!(frame([0, 256, 0, 0, 1, 1], 0.5, 0.25), (-0.5, 0.25));
        assert_eq!(frame([0, 256, 0, 0, 1, 2], 0.5, 0.25), (0.5, -0.25));
        assert_eq!(frame([0, 256, 64, 0, 1, 1], 1.0, 0.0), (-0.75, 0.375));
        assert_eq!(frame([0, 256, 64, 0, 1, 2], 1.0, 0.0), (0.75, -0.375));
    }

    #[test]
    fn the_volume_is_the_knob_curve_and_tops_out_at_5_6_db() {
        let (left, right) = frame([0, 320, 0, 0, 1, 0], 1.0, 1.0);
        assert_eq!(left, right);
        assert!(close(left, (11f64.powf(1.25) - 1.0) / 10.0, 1e-6));
        assert!((gain_to_db(left) - 5.6).abs() < 0.02);
        assert_eq!(frame([0, 0, 0, 0, 1, 0], 1.0, -1.0), (0.0, -0.0));
        for volume in [1, 64, 255, 257, 318] {
            let want = (11f64.powf(f64::from(volume) / 256.0) - 1.0) / 10.0;
            assert!(close(
                frame([0, volume, 0, 0, 1, 0], 1.0, 1.0).0,
                want,
                1e-6
            ));
        }
    }

    #[test]
    fn a_moved_parameter_ramps_the_mix_over_one_block() {
        let mut plugin = StereoEnhancer::default();
        plugin.set_param(1, 0.0);
        let (mut left, mut right) = ([1.0; 4], [-1.0; 4]);
        plugin.process(&mut left, &mut right);
        assert_eq!(left, [1.0, 0.75, 0.5, 0.25]);
        assert_eq!(right, [-1.0, -0.75, -0.5, -0.25]);
        // A reset ends a ramp, and a state starts where it is.
        plugin.set_param(1, 256.0);
        plugin.reset();
        let (mut left, mut right) = ([1.0], [1.0]);
        plugin.process(&mut left, &mut right);
        assert_eq!((left, right), ([1.0], [1.0]));
        plugin.set_state(&state(&[0, 256, 96, 0, 1, 0])).unwrap();
        let (mut left, mut right) = ([1.0], [0.0]);
        plugin.process(&mut left, &mut right);
        assert_eq!((left, right), ([0.625], [0.46875]));
    }

    #[test]
    fn a_moved_offset_drops_what_was_held_and_asks_for_no_memory() {
        let held = |change: &dyn Fn(&mut StereoEnhancer)| {
            let mut plugin = with([0, 256, 0, -24, 1, 0]);
            plugin.process(&mut [1.0; 3], &mut [1.0; 3]);
            change(&mut plugin);
            let (mut left, mut right) = ([0.0; 12], [0.0; 12]);
            plugin.process(&mut left, &mut right);
            (hits(&left), hits(&right))
        };
        let kept = (vec![(5, 1.0), (6, 1.0), (7, 1.0)], vec![]);
        let dropped = (vec![], vec![]);
        assert_eq!(held(&|_| {}), kept);
        // Another parameter, the same rate or the same state keeps it.
        assert_eq!(held(&|p| p.set_param(2, -40.0)).0, kept.0);
        assert_eq!(held(&|p| p.set_sample_rate(44_100.0)), kept);
        assert_eq!(held(&|p| p.set_param(3, -24.0)), kept);
        // Another length, the other side or a reset drops it.
        assert_eq!(held(&|p| p.set_param(3, -25.0)), dropped);
        assert_eq!(held(&|p| p.set_param(3, 24.0)), dropped);
        assert_eq!(held(&|p| p.reset()), dropped);

        let mut plugin = StereoEnhancer::default();
        plugin.set_sample_rate(48_000.0);
        let memory = (plugin.held.as_ptr(), plugin.held.capacity());
        for offset in [512.0, -512.0, 3.0, 0.0, 400.0] {
            plugin.set_param(3, offset);
            plugin.process(&mut [1.0; 64], &mut [1.0; 64]);
        }
        assert_eq!(plugin.held.len(), late(400, 48_000.0));
        plugin.set_state(&state(&[0, 256, 0, -512, 0, 0])).unwrap();
        assert_eq!(plugin.held.len(), 24_000);
        assert_eq!((plugin.held.as_ptr(), plugin.held.capacity()), memory);
        // A rate that is not one leaves the plugin as it is.
        plugin.set_sample_rate(f32::NAN);
        plugin.set_sample_rate(-1.0);
        assert_eq!(plugin.held.len(), 24_000);
    }

    #[test]
    fn a_moved_parameter_is_the_same_plugin_as_its_state() {
        let mut plugin = StereoEnhancer::default();
        let moves = [-18.2, 400.0, -54.0, -248.4, 0.0, 2.0];
        for (index, value) in moves.into_iter().enumerate() {
            plugin.set_param(index, value);
        }
        plugin.set_param(6, 1.0);
        assert_eq!(plugin.param(6), 0.0);
        assert_eq!(
            [0, 1, 2, 3, 4, 5].map(|i| plugin.param(i)),
            [-18.0, 320.0, -54.0, -248.0, 0.0, 2.0]
        );
        plugin.reset();
        assert_eq!(plugin, with([-18, 320, -54, -248, 0, 2]));
    }

    #[test]
    fn a_state_the_plugin_does_not_write_is_turned_down() {
        let mut plugin = with([22, 318, -47, 384, 1, 0]);
        let before = plugin.clone();
        assert_eq!(
            plugin.set_state(&[0; 20]),
            Err(StateError::Length {
                expected: 24,
                found: 20
            })
        );
        for (param, value) in [
            (0, 129),
            (1, 321),
            (2, -97),
            (2, 97),
            (3, -513),
            (4, 2),
            (5, 3),
            (5, -1),
        ] {
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
    fn fl_s_presets_load_as_the_doc_reads_them() {
        let Some(presets) = presets(NAME) else { return };
        // The doc's table: the parameters, LL and RL, and how many samples
        // the left side is late by at 44.1 kHz.
        let want = [
            ("Default", DEFAULTS, 1.0, 0.0, 0),
            ("For strings", [-18, 256, -54, -248, 1, 0], 1.21, -0.51, 604),
            ("Subtle", [0, 256, -40, -24, 1, 0], 1.16, -0.36, 8),
        ];
        assert_eq!(presets.len(), want.len());
        for (preset, (file, params, ll, rl, held)) in presets.iter().zip(want) {
            // The presets have the name in lower case.
            assert_eq!(
                (preset.file.as_str(), preset.plugin.as_str()),
                (file, "Fruity stereo enhancer")
            );
            assert_eq!(preset.state, state(&params), "{file}");
            let mut plugin = with([64, 0, 96, 512, 0, 2]);
            plugin.set_state(&preset.state).unwrap();
            assert_eq!(plugin, with(params), "{file}");
            assert_eq!(plugin.held.len(), held, "{file}");
            // The mix on its own, with nothing late.
            let mut mix = params;
            mix[3] = 0;
            let (left, _) = frame(mix, 1.0, 0.0);
            let (crossed, _) = frame(mix, 0.0, 1.0);
            assert!(
                close(left, ll, 0.005) && close(crossed, rl, 0.005),
                "{file}"
            );
        }
        // For strings has the right output 14 % down.
        let (_, right) = frame([-18, 256, -54, 0, 1, 0], 0.0, 1.0);
        assert!(close(right, 1.21 * 0.86, 0.005));
    }
}
