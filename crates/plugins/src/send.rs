//! Fruity Send: hands its input, panned and at a level, to another mixer
//! insert, and lets through what it is given at the dry level, as
//! `docs/plugins/fruity-send.md` has it.
//!
//! The state is four little-endian i32:
//!
//! | word | parameter | range | default |
//! |---|---|---|---|
//! | 0 | Send panning | -128 to 128 | 0 |
//! | 1 | Send volume | 0 to 320 | 256 |
//! | 2 | Dry level | 0 to 256 | 256 |
//! | 3 | Target mixer track | 0 to 999 | 0 |
//!
//! The target is not a parameter: it counts the inserts the plugin's own
//! insert sends to, 1 the first of them, as Stereo Shaper's sidechain
//! output does, and 0 or less sends nothing. FL asks its host for that
//! output's buffer and adds the send into it, so the plugin's
//! [`Effect::aux_output`] carries it to the host here.
//!
//! The volume and the dry level are FL's knob curve on `v / 256`, worked
//! in `f32` as the plugin does. The panning `p` is a 2x2 matrix: to the
//! right, the left side keeps `1 - p` of itself, and the right keeps
//! `1 - p / 2` of itself and takes `p / 2` of the left, so that all the way
//! right both sides come out on the right at half; to the left, the other
//! way round.

use crate::parts::{Matrix, glide};
use crate::{Effect, StateError, knob_value, read_state};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Send";

/// The ranges of the three wheels.
const RANGES: [(i32, i32); 3] = [(-128, 128), (0, 320), (0, 256)];
const DEFAULTS: [i32; 3] = [0, 256, 256];

/// The longest block the send is kept for without asking for memory.
const ROOM: usize = 4096;

/// FL's knob curve as the plugin works it: `x = v / 256` in `f32`, 1 at 1,
/// else `(e^(x ln 11) - 1) * 0.1` with `ln 11` and `0.1` held in `f32`.
fn knob(value: i32) -> f32 {
    let x = value as f32 * (1.0 / 256.0);
    if x == 1.0 {
        return 1.0;
    }
    let ln11 = 11f64.ln() as f32;
    (((f64::from(x * ln11)).exp() - 1.0) * f64::from(0.1f32)) as f32
}

/// The send's matrix for a panning and a volume: left to left, right to
/// right, right to left and left to right (`FUN_006bd4e0`).
fn matrix(pan: i32, volume: f32) -> [f32; 4] {
    let p = pan as f32 * (1.0 / 128.0);
    if p >= 0.0 {
        let a = 1.0 - p;
        let across = (1.0 - a) * 0.5 * volume;
        [a * volume, volume - across, 0.0, across]
    } else {
        let b = p + 1.0;
        let across = (1.0 - b) * 0.5 * volume;
        [volume - across, b * volume, across, 0.0]
    }
}

/// Fruity Send. A new one sends nowhere and passes its input as it is.
#[derive(Debug, Clone, PartialEq)]
pub struct Send {
    params: [i32; 3],
    target: i32,
    /// The matrix the parameters ask for, and the one the last block ended
    /// on.
    aim: [f32; 4],
    now: [f32; 4],
    dry: f32,
    /// The rate, in Hz.
    hz: f64,
    /// The last block's send, when there is a target.
    sent: [Vec<f32>; 2],
}

impl Default for Send {
    fn default() -> Self {
        let mut plugin = Send {
            params: DEFAULTS,
            target: 0,
            aim: [0.0; 4],
            now: [0.0; 4],
            dry: 1.0,
            hz: 44_100.0,
            sent: [Vec::with_capacity(ROOM), Vec::with_capacity(ROOM)],
        };
        plugin.set(DEFAULTS);
        plugin.now = plugin.aim;
        plugin
    }
}

impl Send {
    /// Take parameters that are inside their ranges.
    fn set(&mut self, params: [i32; 3]) {
        self.params = params;
        self.aim = matrix(params[0], knob(params[1]));
        self.dry = knob(params[2]);
    }
}

impl Effect for Send {
    /// The send starts at the state's level, as after FL's flush. FL takes
    /// the target as it is saved, so a target outside the wheel's range is
    /// taken too: the corpus has a -1, which sends nothing.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let [pan, volume, dry, target] = read_state(
            bytes,
            &[RANGES[0], RANGES[1], RANGES[2], (i32::MIN, i32::MAX)],
        )?;
        self.target = target;
        self.set([pan, volume, dry]);
        self.now = self.aim;
        Ok(())
    }

    /// The rate sets how fast the send's matrix may move.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz > 0.0 {
            self.hz = f64::from(hz);
        }
    }

    /// The send is made from the input, and then the input is scaled by the
    /// dry level. After a send wheel has moved, the matrix goes from where
    /// the last block ended towards where the wheels put it, in a straight
    /// line over the block that moves by no more than `0.001 * 44100 / rate`
    /// a frame, FL's `TAudioRenderer` cap.
    ///
    /// A block longer than any before it and longer than 4096 frames asks
    /// for memory to keep the send in.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        let (left, right) = (&mut left[..len], &mut right[..len]);
        if self.target > 0 && len > 0 {
            let [send_left, send_right] = &mut self.sent;
            send_left.clear();
            send_right.clear();
            let cap = (0.001 * 44_100.0 / self.hz) as f32;
            let mut m = [0.0; 4];
            let mut step = [0.0; 4];
            for k in 0..4 {
                (m[k], step[k]) = glide(&mut self.now[k], self.aim[k], cap, len);
            }
            for (l, r) in left.iter().zip(right.iter()) {
                let (sl, sr) = Matrix::mix(&m, *l, *r);
                send_left.push(sl);
                send_right.push(sr);
                for (gain, step) in m.iter_mut().zip(step) {
                    *gain += step;
                }
            }
        } else {
            self.now = self.aim;
        }
        if self.dry != 1.0 {
            for v in left.iter_mut().chain(right.iter_mut()) {
                *v *= self.dry;
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
            self.set(params);
        }
    }

    /// Ends a ramp and drops the last send.
    fn reset(&mut self) {
        self.now = self.aim;
        for kept in &mut self.sent {
            kept.clear();
        }
    }

    /// The last block's send, when the state has a target.
    fn aux_output(&self) -> Option<(&[f32], &[f32])> {
        (self.target > 0).then(|| (self.sent[0].as_slice(), self.sent[1].as_slice()))
    }

    fn sidechain_output(&self) -> usize {
        self.target.max(0) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{bits, state};

    fn with(values: &[i32]) -> Send {
        let mut plugin = Send::default();
        plugin.set_state(&state(values)).unwrap();
        plugin
    }

    #[test]
    fn the_knob_is_1_at_256_and_follows_11_to_the_x() {
        assert_eq!(knob(256), 1.0);
        assert_eq!(knob(0), 0.0);
        let half = f64::from(knob(128));
        assert!((half - (11f64.sqrt() - 1.0) / 10.0).abs() < 1e-6);
        let top = f64::from(knob(320));
        assert!((top - (11f64.powf(1.25) - 1.0) / 10.0).abs() < 1e-5);
    }

    #[test]
    fn with_no_target_it_only_scales_by_the_dry_level() {
        // FL's own render test: no target and every level at 1.
        let mut plugin = with(&[-54, 256, 256, 0]);
        let (mut left, mut right) = ([0.5, -0.25, 1.0], [0.1, 0.2, -0.3]);
        let (l0, r0) = (left, right);
        plugin.process(&mut left, &mut right);
        assert_eq!(bits(&left), bits(&l0));
        assert_eq!(bits(&right), bits(&r0));
        assert_eq!(plugin.aux_output(), None);
        assert_eq!(plugin.sidechain_output(), 0);
        // I'll Be Your Madonna's: target -1, dry at 197.
        let mut plugin = with(&[0, 256, 197, -1]);
        let (mut left, mut right) = ([1.0f32], [-1.0f32]);
        plugin.process(&mut left, &mut right);
        assert_eq!(left[0], knob(197));
        assert_eq!(right[0], -knob(197));
        assert!((f64::from(knob(197)) - (11f64.powf(197.0 / 256.0) - 1.0) / 10.0).abs() < 1e-6);
        assert_eq!(plugin.aux_output(), None);
    }

    #[test]
    fn the_send_is_the_input_through_the_pan_matrix() {
        let mut plugin = with(&[64, 256, 0, 2]);
        assert_eq!(plugin.sidechain_output(), 2);
        let (mut left, mut right) = ([1.0, 0.0], [0.0, 1.0]);
        plugin.process(&mut left, &mut right);
        // Dry at 0.
        assert_eq!((left, right), ([0.0; 2], [0.0; 2]));
        let (send_left, send_right) = plugin.aux_output().unwrap();
        // Half right: the left keeps half, a quarter of it goes right.
        assert_eq!(send_left, [0.5, 0.0]);
        assert_eq!(send_right, [0.25, 0.75]);
        // And all the way left both sides go left at half.
        let mut plugin = with(&[-128, 256, 256, 1]);
        let (mut left, mut right) = ([1.0, 0.0], [0.0, 1.0]);
        plugin.process(&mut left, &mut right);
        let (send_left, send_right) = plugin.aux_output().unwrap();
        assert_eq!(send_left, [0.5, 0.5]);
        assert_eq!(send_right, [0.0, 0.0]);
        assert_eq!(left, [1.0, 0.0]);
    }

    #[test]
    fn a_moved_volume_ramps_at_most_a_thousandth_a_frame_at_44_1_khz() {
        let mut plugin = with(&[0, 0, 256, 1]);
        plugin.set_sample_rate(44_100.0);
        plugin.set_param(1, 256.0);
        let (mut left, mut right) = ([1.0; 4], [1.0; 4]);
        plugin.process(&mut left, &mut right);
        let (send_left, _) = plugin.aux_output().unwrap();
        assert_eq!(send_left, [0.0, 0.001, 0.002, 0.003]);
        // The next block goes on from where this one ended.
        let (mut left, mut right) = ([1.0; 2], [1.0; 2]);
        plugin.process(&mut left, &mut right);
        assert_eq!(plugin.aux_output().unwrap().0[0], 0.004);
        // A small move gets there in one block, in a straight line.
        let mut plugin = with(&[0, 255, 256, 1]);
        plugin.set_param(1, 256.0);
        let (mut left, mut right) = ([1.0; 64], [1.0; 64]);
        plugin.process(&mut left, &mut right);
        let start = knob(255);
        let step = (1.0 - start) / 64.0;
        let send = plugin.aux_output().unwrap().0;
        assert_eq!(send[0], start);
        assert!((send[63] - (start + 63.0 * step)).abs() < 1e-5);
        let (mut left, mut right) = ([1.0; 2], [1.0; 2]);
        plugin.process(&mut left, &mut right);
        assert_eq!(plugin.aux_output().unwrap().0, [1.0, 1.0]);
    }

    #[test]
    fn a_wheel_outside_its_range_is_turned_down() {
        let mut plugin = Send::default();
        assert!(plugin.set_state(&state(&[0, 321, 0, 0])).is_err());
        assert!(plugin.set_state(&state(&[0, 0, 0])).is_err());
        assert_eq!(plugin.param(1), 256.0);
    }
}
