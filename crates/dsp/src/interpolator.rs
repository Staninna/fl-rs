//! FL's sample interpolator: the class its plugins share to read a sample
//! at any place and speed (`docs/plugins/fl-interpolator.md`). Fruity Convolver
//! resamples its impulse with it; this is its sinc kind, the one a quality
//! of more than 6 points picks.
//!
//! A frame at `index + frac` is the sum, over the `points` taps each side
//! of it, of a tap times
//!
//! ```text
//! sin(w d) / (pi d) * (1 + c) (0.36 + 0.64 c) / 2,    c = cos(pi d / points)
//! ```
//!
//! where `d` is how far the place is past the tap and `w` is `pi`, or `pi /
//! speed` when the read goes faster than 1 and so must shed what is above
//! the new Nyquist. The window is a Hann window times a line that leans it
//! towards the centre. A tap before the sample's first frame reads the first
//! frame, and one after its last reads the last.
//!
//! FL works the sines out the way this does: four of them per frame in
//! double precision, and from there two steps at a time by the recurrence
//! `sin(a + 2t) = 2 cos t sin(a + t) - sin a` in single precision. The
//! reciprocal of `pi d` is the processor's estimate refined by one Newton
//! step in FL, and a division here, which is as near as a float gets but
//! is not bit for bit FL's on every processor.

use std::f64::consts::{FRAC_PI_2, PI};

/// The fraction FL never lets a place have less of, so that `d` is never 0.
const LEAST: f64 = 1e-25;

/// The most taps a side a quality can ask for.
pub const MOST: u32 = 512;

/// The sinc kind of FL's interpolator, at one quality and speed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Interpolator {
    /// Taps on each side of a place, and what the quality asked for: the
    /// speed can widen it.
    points: u32,
    asked: u32,
    speed: f64,
    /// `pi / points` and twice its cosine.
    window_step: f64,
    window_turn: f64,
    /// `w` and twice its cosine.
    cutoff: f64,
    cutoff_turn: f64,
}

impl Interpolator {
    /// An interpolator of `points` taps a side at a speed of 1
    /// (`FUN_0061f6c0`, `FUN_0061f5d0`). FL's 256-point quality is
    /// `new(256)`. Fewer than 7 points are other kinds that are not here.
    pub fn new(points: u32) -> Self {
        assert!(points > 6, "fewer than 7 points is not the sinc kind");
        let points = points.min(MOST);
        let mut it = Interpolator {
            points,
            asked: points,
            speed: 1.0,
            window_step: 0.0,
            window_turn: 0.0,
            cutoff: PI,
            cutoff_turn: -2.0,
        };
        it.set_points(points);
        it
    }

    fn set_points(&mut self, points: u32) {
        self.points = points;
        self.window_step = PI / f64::from(points);
        self.window_turn = self.window_step.cos() * 2.0;
    }

    /// How many source frames one frame read moves on (`FUN_0061fc90`).
    /// Above 1 the sinc narrows to the new Nyquist; a speed so high that it
    /// needs more taps than the quality has widens the window, up to 512.
    pub fn set_speed(&mut self, speed: f64) {
        self.speed = speed;
        let fast = speed.abs();
        if fast <= 1.0 {
            (self.cutoff, self.cutoff_turn) = (PI, -2.0);
        } else {
            self.cutoff = PI / fast;
            self.cutoff_turn = self.cutoff.cos() * 2.0;
        }
        let wide = ((fast.round_ties_even() as i64 + 3) & !3).max(i64::from(self.asked));
        let wide = wide.min(i64::from(MOST)) as u32;
        if wide != self.points {
            self.set_points(wide);
        }
    }

    pub fn speed(&self) -> f64 {
        self.speed
    }

    /// Taps on each side of a place.
    pub fn points(&self) -> u32 {
        self.points
    }

    /// The frame of `src` at `index + frac`, `frac` in 0 to 1. Taps outside
    /// `src` read its nearest end; an empty `src` reads 0.
    pub fn frame(&self, src: &[f32], index: i64, frac: f64) -> f32 {
        let n = self.points as i64;
        let last = src.len() as i64 - 1;
        if last < 0 {
            return 0.0;
        }
        let first = index - n + 1;
        if first >= 0 && index + n <= last {
            let from = first as usize;
            return self.sum(&src[from..from + 2 * n as usize], frac);
        }
        let mut held = [0f32; 2 * MOST as usize];
        for (k, v) in held[..2 * n as usize].iter_mut().enumerate() {
            *v = src[(first + k as i64).clamp(0, last) as usize];
        }
        self.sum(&held[..2 * n as usize], frac)
    }

    /// The kernel over `taps`, which are the `points` frames up to and with
    /// the place's own and the `points` after it (the nested loops of
    /// `FUN_0061c1f0` and `FUN_0061c690`, one side at a time: FL's stereo
    /// kernel does both sides with the same numbers in the same order).
    fn sum(&self, taps: &[f32], frac: f64) -> f32 {
        let f = frac.max(LEAST);
        // The four angles, their sines in double precision, and one step
        // of each recurrence before it goes to single precision.
        let a0 = f * -self.window_step + FRAC_PI_2;
        let a1 = a0 - self.window_step;
        let b0 = f * self.cutoff;
        let b1 = b0 + self.cutoff;
        let (sa0, sa1, sb0, sb1) = (a0.sin(), a1.sin(), b0.sin(), b1.sin());
        let step = sa0 * self.window_turn - sa1;
        // The window: cos of (d pi / points) for the tap after the place
        // and the one after that, and for the place's own and the one
        // before it.
        let (mut w_ahead, mut w_ahead2) = (step as f32, (step * self.window_turn - sa0) as f32);
        let (mut w_back, mut w_back2) = (sa0 as f32, sa1 as f32);
        let step = sb0 * self.cutoff_turn - sb1;
        // The sinc's sines, in the same four places.
        let (mut s_ahead, mut s_ahead2) = (step as f32, (step * self.cutoff_turn - sb0) as f32);
        let (mut s_back, mut s_back2) = (sb0 as f32, sb1 as f32);
        // pi d for the same four.
        let x = f * PI;
        let mut d_ahead = (x - PI) as f32;
        let mut d_ahead2 = ((x - PI) - PI) as f32;
        let mut d_back = x as f32;
        let mut d_back2 = (x + PI) as f32;
        let tau = std::f32::consts::TAU;
        let (cutoff_turn, window_turn) = (self.cutoff_turn as f32, self.window_turn as f32);
        let lean = |c: f32| 0.64 * c * c + c + 0.36;

        let n = self.points as usize;
        let (mut ahead, mut back) = (0f32, 0f32);
        for k in 0..n / 2 {
            let (r_ahead, r_back, r_ahead2, r_back2) =
                (1.0 / d_ahead, 1.0 / d_back, 1.0 / d_ahead2, 1.0 / d_back2);
            let s_ahead3 = s_ahead2 * cutoff_turn - s_ahead;
            let s_back3 = s_back2 * cutoff_turn - s_back;
            let w_ahead3 = w_ahead2 * window_turn - w_ahead;
            let w_back3 = w_back2 * window_turn - w_back;
            d_ahead -= tau;
            d_back += tau;
            d_ahead2 -= tau;
            d_back2 += tau;
            let at = n - 1;
            ahead = ahead
                + s_ahead * r_ahead * lean(w_ahead) * taps[at + 1 + 2 * k]
                + s_ahead2 * r_ahead2 * lean(w_ahead2) * taps[at + 2 + 2 * k];
            back = back
                + s_back * r_back * lean(w_back) * taps[at - 2 * k]
                + s_back2 * r_back2 * lean(w_back2) * taps[at - 1 - 2 * k];
            w_ahead2 = w_ahead3 * window_turn - w_ahead2;
            w_back2 = w_back3 * window_turn - w_back2;
            w_ahead = w_ahead3;
            w_back = w_back3;
            s_ahead2 = s_ahead3 * cutoff_turn - s_ahead2;
            s_back2 = s_back3 * cutoff_turn - s_back2;
            s_ahead = s_ahead3;
            s_back = s_back3;
        }
        (back + ahead) * 0.5
    }
}

/// `src` at `ratio` times its rate, as FL's sample class resamples itself
/// (`FUN_0062aad0` with a sinc quality): `round(len * ratio)` frames, read
/// at a speed of `1 / ratio` from frame 0, in blocks of 1024. A block's
/// places are `place + k * speed` when the whole block is clear of the
/// sample's ends, and otherwise the place moves on by the speed frame by
/// frame (`FUN_00620bc0`); the two give the same frames to a rounding.
pub fn resample(src: &[f32], ratio: f64, points: u32) -> Vec<f32> {
    let len = (src.len() as f64 * ratio).round_ties_even().max(0.0) as usize;
    let mut out = Vec::with_capacity(len);
    if src.is_empty() || len == 0 {
        return out;
    }
    let mut it = Interpolator::new(points);
    it.set_speed(1.0 / ratio);
    let speed = it.speed;
    let n = i64::from(it.points);
    let end = src.len() as i64;
    // Where the taps reach past the ends, and the margin a block keeps.
    let (head, tail) = (n - 1, end - n);
    let (clear, near) = (head + 3, tail - 3);
    let mut place = 0f64;
    let frame = |place: f64, out: &mut Vec<f32>| {
        let index = place as i64;
        out.push(it.frame(src, index, place - index as f64));
    };
    'blocks: while out.len() < len {
        let mut left = (len - out.len()).min(1024);
        while left > 0 {
            let fast = (place.round_ties_even() as i64) >= clear
                && place + speed * (left as f64) < near as f64;
            if fast {
                for k in 0..left {
                    frame(place + speed * k as f64, &mut out);
                }
                place += speed * left as f64;
                left = 0;
            } else {
                // Frame by frame, until a frame near an end has been read
                // with more of the block left: then the block is looked at
                // again.
                while left > 0 {
                    let index = place as i64;
                    if index >= end {
                        // FL's read stops short, and the rest stays 0.
                        out.resize(len, 0.0);
                        break 'blocks;
                    }
                    frame(place, &mut out);
                    place += speed;
                    left -= 1;
                    if index < head || index >= tail {
                        break;
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The kernel straight from its formula, in double precision.
    fn reference(src: &[f32], place: f64, points: u32, speed: f64) -> f64 {
        let n = i64::from(points);
        let index = place.floor() as i64;
        let f = (place - index as f64).max(LEAST);
        let w = if speed > 1.0 { PI / speed } else { PI };
        let last = src.len() as i64 - 1;
        let mut sum = 0.0;
        for j in -n + 1..=n {
            let d = f - j as f64;
            let c = (PI * d / n as f64).cos();
            let h = (w * d).sin() / (PI * d) * (1.0 + c) * (0.36 + 0.64 * c) / 2.0;
            sum += h * f64::from(src[(index + j).clamp(0, last) as usize]);
        }
        sum
    }

    fn noise(len: usize) -> Vec<f32> {
        let mut seed = 0x1234_5678u32;
        (0..len)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 8) as f32 / (1 << 23) as f32 - 1.0
            })
            .collect()
    }

    #[test]
    fn the_recurrences_give_the_formula() {
        let src = noise(2000);
        for (points, speed) in [(256, 1.0), (256, 1.7), (16, 1.0), (64, 0.5)] {
            let mut it = Interpolator::new(points);
            it.set_speed(speed);
            for place in [0.0, 3.25, 700.5, 1000.125, 1999.75] {
                let index = place as i64;
                let got = it.frame(&src, index, place - index as f64);
                let want = reference(&src, place, points, speed);
                assert!(
                    (f64::from(got) - want).abs() < 2e-4,
                    "{points} at {place}: {got} against {want}"
                );
            }
        }
    }

    #[test]
    fn a_whole_place_reads_its_frame() {
        let src = noise(1200);
        let it = Interpolator::new(256);
        for index in [0, 1, 300, 600, 1199] {
            let got = it.frame(&src, index, 0.0);
            assert!((got - src[index as usize]).abs() < 1e-5, "{index}");
        }
    }

    #[test]
    fn a_constant_stays_constant_to_its_ends() {
        let src = vec![0.5f32; 700];
        let it = Interpolator::new(256);
        for place in [0.0, 0.3, 255.7, 444.4, 699.9] {
            let index = place as i64;
            let got = it.frame(&src, index, place - index as f64);
            // The window's sum over the taps is 1 to within its ripple.
            assert!((got - 0.5).abs() < 2e-3, "{place}: {got}");
        }
    }

    #[test]
    fn a_tone_keeps_its_pitch_and_level_and_a_fast_read_sheds_the_top() {
        let tone = |hz: f64, rate: f64, len: usize| -> Vec<f32> {
            (0..len)
                .map(|i| (2.0 * PI * hz * i as f64 / rate).sin() as f32)
                .collect()
        };
        let level = |x: &[f32]| {
            let mid = &x[x.len() / 4..x.len() * 3 / 4];
            (mid.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / mid.len() as f64).sqrt()
        };
        let up = resample(&tone(1000.0, 44_100.0, 4410), 48_000.0 / 44_100.0, 256);
        assert_eq!(up.len(), 4800);
        let want = tone(1000.0, 48_000.0, 4800);
        let worst = up[600..4200]
            .iter()
            .zip(&want[600..4200])
            .map(|(a, b)| (a - b).abs())
            .fold(0f32, f32::max);
        assert!(worst < 1e-3, "{worst}");
        // Half the rate: 15 kHz is above the new Nyquist and goes.
        let high = resample(&tone(15_000.0, 44_100.0, 8820), 0.5, 256);
        assert_eq!(high.len(), 4410);
        assert!(level(&high) < 0.01, "{}", level(&high));
        let low = resample(&tone(1000.0, 44_100.0, 8820), 0.5, 256);
        assert!((level(&low) - 0.5f64.sqrt()).abs() < 0.01);
    }

    #[test]
    fn the_length_is_rounded_half_to_even() {
        assert_eq!(resample(&[0.0; 5], 0.5, 256).len(), 2);
        assert_eq!(resample(&[0.0; 7], 0.5, 256).len(), 4);
        assert!(resample(&[], 2.0, 256).is_empty());
    }

    #[test]
    fn speed_widens_the_window_only_past_the_quality() {
        let mut it = Interpolator::new(256);
        it.set_speed(3.0);
        assert_eq!(it.points(), 256);
        it.set_speed(300.0);
        assert_eq!(it.points(), 300);
        it.set_speed(1000.0);
        assert_eq!(it.points(), MOST);
        it.set_speed(1.0);
        assert_eq!(it.points(), 256);
    }
}
