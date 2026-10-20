//! What more than one plugin is built from: the ramped 2x2 mix of Balance,
//! Delay 2 and Stereo Enhancer, the coefficient slew and the follower of
//! Filter and Fast LP, and the offset curve of Delay 2 and Stereo Enhancer.

/// Four gains on a stereo block, `[LL, RR, RL, LR]`: left to left, right to
/// right, right into left, left into right.
///
/// After the gains have moved, they go from where the last block ended to
/// where they were put, in a straight line over one block. FL also holds
/// each step of that line to a most its host hands the plugin, so that a
/// long way takes more than one block. That number is not in the DLLs and
/// is not applied.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Matrix {
    /// The gains the parameters ask for.
    target: [f32; 4],
    /// The gains the last block ended on.
    current: [f32; 4],
}

impl Matrix {
    pub(crate) fn new(gains: [f32; 4]) -> Self {
        Matrix {
            target: gains,
            current: gains,
        }
    }

    /// Have these gains from the next sample on, with no ramp.
    pub(crate) fn jump(&mut self, gains: [f32; 4]) {
        *self = Matrix::new(gains);
    }

    /// Ramp to these gains over the next block.
    pub(crate) fn aim(&mut self, gains: [f32; 4]) {
        self.target = gains;
    }

    /// End a ramp that is under way.
    pub(crate) fn settle(&mut self) {
        self.current = self.target;
    }

    /// The gains for the first frame of a block of `len` frames and what
    /// to add to them after every frame. `len` is not 0.
    pub(crate) fn ramp(&mut self, len: usize) -> ([f32; 4], [f32; 4]) {
        let from = self.current;
        let step = std::array::from_fn(|k| (self.target[k] - from[k]) / len as f32);
        self.current = self.target;
        (from, step)
    }

    /// One frame through the gains `m`.
    pub(crate) fn mix(m: &[f32; 4], left: f32, right: f32) -> (f32, f32) {
        (left * m[0] + right * m[2], right * m[1] + left * m[3])
    }

    /// Run one block through the gains, in place.
    pub(crate) fn apply(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        if len == 0 {
            return;
        }
        let frames = left.iter_mut().zip(right.iter_mut());
        let [ll, rr, rl, lr] = self.current;
        if self.current == self.target && rl == 0.0 && lr == 0.0 {
            // FL scales each side on its own here, and at unity it leaves
            // the block alone.
            if ll != 1.0 || rr != 1.0 {
                for (l, r) in frames {
                    *l *= ll;
                    *r *= rr;
                }
            }
            return;
        }
        let (mut m, step) = self.ramp(len);
        for (l, r) in frames {
            (*l, *r) = Matrix::mix(&m, *l, *r);
            for (gain, step) in m.iter_mut().zip(step) {
                *gain += step;
            }
        }
    }
}

/// How many samples a filter coefficient takes to get where it was put,
/// when the blocks are shorter than that.
const SLEW: usize = 128;

/// Move a filter coefficient towards its target over a block of `len`
/// frames, which is not 0. This gives what the coefficient is as the block
/// starts and what to add to it every frame, and leaves `current` where the
/// block ends.
///
/// A block of 128 frames or more gets all the way. A shorter one goes at
/// the pace that would take 128 frames from where it starts, so every block
/// covers `len / 128` of what is left.
///
/// FL has a floor under which the coefficient becomes 0, at a number it
/// works out at startup. That is not here. What is here and not in FL: a
/// block too short to move the coefficient by one bit ends on the target,
/// so that a ramp does end.
pub(crate) fn slew(current: &mut f32, target: f32, len: usize) -> (f32, f32) {
    let start = *current;
    if start == target {
        return (start, 0.0);
    }
    if len >= SLEW {
        *current = target;
        return (start, (target - start) / len as f32);
    }
    let step = (target - start) / SLEW as f32;
    let end = start + step * len as f32;
    *current = if end == start { target } else { end };
    (start, step)
}

/// A value that goes after its input with a speed it keeps a part of:
///
/// ```text
/// v = v + f * (x - y)
/// y = y + v
/// v = v * q
/// ```
///
/// Fast LP is this, and Filter's Center takes it off its input. With `q`
/// at 0 it is the one pole `y += f * (x - y)`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Follower {
    pub(crate) y: f64,
    v: f64,
}

impl Follower {
    /// Take one sample and give `y`.
    pub(crate) fn step(&mut self, x: f64, f: f64, q: f64) -> f64 {
        self.v += f * (x - self.y);
        self.y += self.v;
        self.v *= q;
        self.y
    }

    /// What FL does after every block: a number too small for a double to
    /// hold in full becomes 0, before it slows the sums down.
    pub(crate) fn flush(&mut self) {
        for state in [&mut self.y, &mut self.v] {
            if state.is_subnormal() {
                *state = 0.0;
            }
        }
    }

    /// What Fruity Center's kernel does after every block instead: a part
    /// of the state nearer 0 than `floor` becomes 0.
    pub(crate) fn clear_below(&mut self, floor: f64) {
        for state in [&mut self.y, &mut self.v] {
            if state.abs() < floor {
                *state = 0.0;
            }
        }
    }
}

/// How many frames the offset wheel of Delay 2 or Stereo Enhancer holds one
/// side back by. The wheel's ±512 is `(1001^(|v| / 512) - 1) * 50` units of
/// 10 microseconds, which is 2.3 ms at 128 and half a second at 512.
pub(crate) fn offset_frames(value: i32, hz: f64) -> usize {
    let units = (1001f64.powf(f64::from(value.abs()) / 512.0) - 1.0) * 50.0;
    (units * hz / 100_000.0).round_ties_even() as usize
}

/// Move a gain or a coefficient to `target` over `len` frames, by no more
/// than `cap` a frame, as Love Philter and Convolver do. This gives what it
/// is as the frames start and what to add every frame, and leaves `current`
/// where they end. `len` is not 0.
///
/// The cap is the number [`Matrix`] does without: FL's plugins have it from
/// an object of their own, `TAudioRenderer`, which works it out from the
/// rate as `0.001 * 44100 / rate`. Convolver's gains go by that and Love
/// Philter's by ten times it.
pub(crate) fn glide(current: &mut f32, target: f32, cap: f32, len: usize) -> (f32, f32) {
    let start = *current;
    let mut step = target - start;
    if step != 0.0 {
        *current = target;
        step /= len as f32;
        if cap < step.abs() {
            step = cap.copysign(step);
            *current = start + step * len as f32;
            // FL's "too small to keep": 2^-24.
            if current.abs() < 5.960_464_5e-8 {
                *current = 0.0;
                step = -start / len as f32;
            }
        }
    }
    (start, step)
}

/// The four gains `[LL, RR, RL, LR]` of FL's pan, volume and stereo
/// separation stage, which Love Philter's units and Convolver's two mixes
/// share.
///
/// The pan is a constant power law that is at `volume` on both sides in the
/// middle, and the side that loses level is fed half of what it loses from
/// the other. A separation from 0 to 1 is `(5^s - 1) / 4` of the way to
/// mono, and one below 0 takes that much of the other side away.
pub(crate) fn stereo_gains(pan: f32, volume: f32, separation: f32) -> [f32; 4] {
    if pan == 0.0 && separation == 0.0 {
        return [volume, volume, 0.0, 0.0];
    }
    if volume == 0.0 {
        return [0.0; 4];
    }
    let turn = (f64::from(pan + 1.0) * std::f64::consts::FRAC_PI_4) as f32;
    let (sin, cos) = f64::from(turn).sin_cos();
    let full = volume * (1.0 / std::f64::consts::FRAC_PI_4.sin()) as f32;
    let (mut left, mut right) = (full * cos as f32, full * sin as f32);
    let (mut to_left, mut to_right) = (0.0, 0.0);
    if pan < 0.0 {
        to_left = 1.0 - right / volume;
    } else if pan > 0.0 {
        to_right = 1.0 - left / volume;
    }
    if separation != 0.0 {
        let mut merge = separation;
        if separation > 0.0 {
            let curve = (f64::from(separation) * 5f64.ln()).exp() - 1.0;
            merge = curve as f32 * 0.25;
        }
        to_left = (to_left + merge).min(1.0);
        to_right = (to_right + merge).min(1.0);
    }
    let from_right = left * to_left * 0.5;
    left *= 1.0 - to_left * 0.5;
    let from_left = right * to_right * 0.5;
    right *= 1.0 - to_right * 0.5;
    [left, right, from_right, from_left]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_glide_is_a_straight_line_that_the_cap_slows_down() {
        let mut gain = 0.0;
        assert_eq!(glide(&mut gain, 1.0, 1.0, 4), (0.0, 0.25));
        assert_eq!(gain, 1.0);
        assert_eq!(glide(&mut gain, 1.0, 1.0, 4), (1.0, 0.0));
        // No more than the cap a frame, so it gets part of the way.
        assert_eq!(glide(&mut gain, 0.0, 0.125, 4), (1.0, -0.125));
        assert_eq!(gain, 0.5);
        assert_eq!(glide(&mut gain, 0.0, 0.125, 4), (0.5, -0.125));
        assert_eq!(gain, 0.0);
    }

    #[test]
    fn the_stereo_gains_pan_by_constant_power_and_merge_or_widen() {
        assert_eq!(stereo_gains(0.0, 0.5, 0.0), [0.5, 0.5, 0.0, 0.0]);
        assert_eq!(stereo_gains(0.3, 0.0, 0.5), [0.0; 4]);
        // All the way left: the right side is gone and half of it is in
        // the left.
        let left = stereo_gains(-1.0, 1.0, 0.0);
        let root = std::f32::consts::SQRT_2;
        assert!((left[0] - root * 0.5).abs() < 1e-6 && left[1].abs() < 1e-6);
        assert!((left[2] - root * 0.5).abs() < 1e-6 && left[3] == 0.0);
        // Full separation is mono, and -1 is half of the other side taken
        // away.
        let near = |gains: [f32; 4], wanted: [f32; 4]| {
            let apart = gains.iter().zip(wanted).map(|(g, w)| (g - w).abs());
            apart.fold(0f32, f32::max) < 1e-6
        };
        assert!(near(stereo_gains(0.0, 1.0, 1.0), [0.5, 0.5, 0.5, 0.5]));
        assert!(near(stereo_gains(0.0, 1.0, -1.0), [1.5, 1.5, -0.5, -0.5]));
        let some = stereo_gains(0.0, 2.0, 0.5);
        let merge = (5f32.sqrt() - 1.0) * 0.25;
        assert!((some[2] - merge).abs() < 1e-6 && (some[0] - (2.0 - merge)).abs() < 1e-6);
    }

    #[test]
    fn a_short_block_slews_at_the_pace_of_128_samples() {
        let mut current = 0.0;
        // 32 frames are a quarter of the way, at 1/128 a frame.
        assert_eq!(slew(&mut current, 1.0, 32), (0.0, 1.0 / 128.0));
        assert_eq!(current, 0.25);
        // The next block starts over from where this one ended: a quarter
        // of the 0.75 that is left.
        assert_eq!(slew(&mut current, 1.0, 32), (0.25, 0.75 / 128.0));
        assert_eq!(current, 0.4375);
        // 128 frames or more get there.
        assert_eq!(slew(&mut current, 1.0, 128), (0.4375, 0.5625 / 128.0));
        assert_eq!(current, 1.0);
        assert_eq!(slew(&mut current, 0.0, 500), (1.0, -1.0 / 500.0));
        assert_eq!(current, 0.0);
        // Nothing to do at the target.
        assert_eq!(slew(&mut current, 0.0, 7), (0.0, 0.0));
    }

    #[test]
    fn a_slew_in_blocks_of_one_frame_ends() {
        let mut current = 0.0;
        let mut blocks = 0;
        while current != 1.0 {
            slew(&mut current, 1.0, 1);
            blocks += 1;
            assert!(blocks < 10_000, "stuck at {current}");
        }
        // 127/128 of what is left stays after every frame, which takes
        // 24 bits about 128 * ln(2^24) frames.
        assert!(blocks > 1500, "{blocks}");
    }

    #[test]
    fn the_follower_is_a_one_pole_at_q_0() {
        let mut follower = Follower::default();
        let mut y = 0.0;
        for x in [1.0, 1.0, -0.5, 0.25, 0.0] {
            y += 0.3 * (x - y);
            assert_eq!(follower.step(x, 0.3, 0.0), y);
        }
        // With q it keeps that much of its last step.
        let mut follower = Follower::default();
        assert_eq!(follower.step(1.0, 0.5, 0.5), 0.5);
        assert_eq!(follower.step(1.0, 0.5, 0.5), 0.5 + (0.25 + 0.25));
        follower.y = f64::MIN_POSITIVE / 4.0;
        follower.flush();
        assert_eq!(follower.y, 0.0);
    }

    #[test]
    fn the_offset_wheel_is_the_doc_s_curve() {
        assert_eq!(offset_frames(0, 44_100.0), 0);
        // Half a second at either end.
        assert_eq!(offset_frames(512, 44_100.0), 22_050);
        assert_eq!(offset_frames(-512, 96_000.0), 48_000);
        // 128 is 2.3 ms, 256 is 15.3 ms and 384 is 88 ms.
        for (value, ms) in [(128, 2.3), (256, 15.3), (-384, 88.0)] {
            let frames = offset_frames(value, 48_000.0) as f64;
            assert!((frames / 48.0 - ms).abs() < 0.01 * ms, "{value}: {frames}");
        }
        // The presets of Stereo Enhancer: 8 and 604 samples at 44.1 kHz.
        assert_eq!(offset_frames(-24, 44_100.0), 8);
        assert_eq!(offset_frames(-248, 44_100.0), 604);
    }

    #[test]
    fn a_matrix_ramps_over_one_block_and_leaves_unity_alone() {
        let mut matrix = Matrix::new([1.0, 1.0, 0.0, 0.0]);
        let (mut left, mut right) = ([-0.0, 0.5], [f32::MIN_POSITIVE, -1.0]);
        matrix.apply(&mut left, &mut right);
        assert_eq!(left.map(f32::to_bits), [-0.0, 0.5].map(f32::to_bits));
        assert_eq!(right, [f32::MIN_POSITIVE, -1.0]);

        matrix.aim([0.0, 1.0, 0.0, 1.0]);
        let (mut left, mut right) = ([1.0; 4], [0.0; 4]);
        matrix.apply(&mut left, &mut right);
        assert_eq!(left, [1.0, 0.75, 0.5, 0.25]);
        assert_eq!(right, [0.0, 0.25, 0.5, 0.75]);
        assert_eq!(matrix.ramp(4), ([0.0, 1.0, 0.0, 1.0], [0.0; 4]));

        matrix.aim([2.0, 2.0, 0.0, 0.0]);
        matrix.settle();
        let (mut left, mut right) = ([1.0], [-1.0]);
        matrix.apply(&mut left, &mut right);
        assert_eq!((left, right), ([2.0], [-2.0]));
    }
}
