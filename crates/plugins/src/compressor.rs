//! Fruity Compressor: a peak detector, a gain computer with four knees, a
//! lookahead limiter that is always on and a 3 Hz high pass, as
//! `docs/plugins/fruity-compressor.md` has it.
//!
//! The state is a little-endian i32 version, the plugin's six parameters
//! as i32, from version 2 on an i32 whose lowest bit says whether the host
//! is told of the delay, and in version 3 an i32 for the sidechain input:
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Threshold, 0.1 dB | -600 to 0 | 0 |
//! | 1 Ratio, 0.1 : 1 | 4 to 300 | 10 |
//! | 2 Gain, 0.1 dB | -300 to 300 | 0 |
//! | 3 Attack, 0.1 ms | 0 to 4000 | 150 |
//! | 4 Release, 1 ms | 1 to 4000 | 200 |
//! | 5 Type | 0 to 7 | 0 |
//!
//! Version 1 is 28 bytes, version 2 is 32 and version 3 is 36. The types
//! are Hard, Medium, Vintage and Soft, and the same four again with a
//! release that speeds up after a deep cut.
//!
//! Every frame goes through the high pass, and the louder side of what
//! comes out is the level. An envelope follows the level up in 0.01 ms and
//! down in 1 ms. Its natural log gives a gain off the type's curve, the
//! attack and the release smooth that gain, and the limiter holds what the
//! gain and the make-up leave under -0.1 dBFS. The limiter looks 64 frames
//! ahead, so the plugin's output is always [`LOOKAHEAD`] frames late.
//!
//! Where the doc and the DLL differ, this follows the DLL: a reset fills
//! the limiter's gains with 1 and not 0, and a moved parameter puts the
//! slow follower of the gain back at 1.
//!
//! A state with a sidechain input chosen takes its level from the block a
//! host hands [`Effect::set_sidechain`], as it comes and with no high pass
//! on it. For a block the host hands it none, it listens to its own input.

use crate::{Effect, Sidechain, StateError, knob_value, read_state, state_version};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Compressor";

/// How many frames late the output is, whatever the parameters.
pub const LOOKAHEAD: usize = 64;

/// The range of each parameter's wheel.
const RANGES: [(i32, i32); 6] = [
    (-600, 0),
    (4, 300),
    (-300, 300),
    (0, 4000),
    (1, 4000),
    (0, 7),
];
const DEFAULTS: [i32; 6] = [0, 10, 0, 150, 200, 0];

/// How long each version of the state is.
const V1_LEN: usize = 28;
const V2_LEN: usize = 32;
const V3_LEN: usize = 36;

/// The width of each type's knee in dB, either side of the threshold.
const KNEES: [i32; 4] = [0, 6, 7, 15];

/// What FL adds to a sample before it works with it, to stay clear of
/// numbers too small to hold in full.
const TINY: f64 = 1e-30;

/// -0.1 dBFS, where the limiter stops a peak.
const CEILING: f64 = 0.988_553_1;

/// The limiter keeps a gain for each of the next 128 frames.
const RING: usize = 2 * LOOKAHEAD;

/// What the limiter's release starts from after a parameter has moved.
const LIMIT_RELEASE: f32 = 0.01;

/// The part of a difference that is left one sample on, for a time in ms
/// to fall to a tenth in: `0.1^(1000 / (ms * rate))`, and 0 for no time.
fn coef(ms: f32, rate: f32) -> f32 {
    if ms == 0.0 {
        return 0.0;
    }
    0.1f64.powf(f64::from(1.0 / (ms * rate / 1000.0))) as f32
}

/// What the parameters and the rate come to. The logs are natural ones.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Curve {
    /// What the envelope keeps of its way to a louder level, and to a
    /// quieter one.
    detect_up: f32,
    detect_down: f32,
    /// What the gain keeps of its way down.
    attack: f32,
    /// What the gain keeps of its way up, and what it keeps at its
    /// fastest. The two are one number for the types under 4.
    release: f32,
    release_fast: f32,
    /// The log of the threshold, and of where the knee starts and ends.
    threshold: f32,
    knee_low: f32,
    knee_high: f32,
    /// `1 / ratio - 1`, the slope above the knee. Vintage has its own.
    slope: f32,
    /// `0.5 / (knee_high - knee_low)`, for the knee's parabola.
    half: f32,
    /// Vintage: `1 / (knee_high - knee_low)`, how far the threshold is
    /// above the knee's start, 1 at most, and the gain at the knee's end.
    span: f32,
    blend: f32,
    floor: f32,
    vintage: bool,
    makeup: f32,
    /// The allpass coefficient of the high pass.
    dc: f32,
}

impl Curve {
    fn new(params: &[i32; 6], rate: f32) -> Self {
        let [threshold, ratio, gain, attack, release, kind] = params.map(f64::from);
        let shape = params[5] as usize % 4;
        let vintage = shape == 2;
        let ratio = ratio / 10.0;

        let nominal = coef(release as f32, rate);
        let fast = if kind < 4.0 {
            nominal
        } else {
            coef((release * 0.1) as f32, rate)
        };

        let linear = 10f64.powf(threshold / 200.0) as f32;
        let knee = 10f64.powf(f64::from(-KNEES[shape]) / 20.0) as f32;
        let mut slope = (1.0 / f64::from(ratio as f32) - 1.0) as f32;
        let threshold = f64::from(linear).ln() as f32;
        let knee_low = f64::from(linear * knee).ln() as f32;
        let mut knee_high = f64::from(linear / knee).ln() as f32;
        let half = (0.5 / f64::from(knee_high - knee_low)) as f32;
        let mut floor = 1.0;
        if vintage {
            // The knee ends 1.25 * ratio^0.25 nepers above the threshold.
            // The gain the plain slope has there is the least this type
            // gives. The knee is then stretched by its width, and gets the
            // slope that still ends on that gain.
            knee_high = (f64::from(threshold) + ratio.powf(0.25) * 1.25) as f32;
            floor = f64::from((knee_high - threshold) * slope).exp() as f32;
            knee_high = (f64::from(knee_high).exp() / f64::from(knee)).ln() as f32;
            slope = (f64::from(floor).ln() / f64::from(knee_high - threshold)) as f32;
        }

        // A first-order allpass turned into a high pass at 3 Hz.
        let tan = f64::from((3.0 * std::f64::consts::PI / f64::from(rate)).tan() as f32);
        Curve {
            detect_up: coef(0.01, rate),
            detect_down: coef(1.0, rate),
            attack: coef((attack / 10.0) as f32, rate),
            release: nominal,
            release_fast: fast,
            threshold,
            knee_low,
            knee_high,
            slope,
            half,
            span: (1.0 / f64::from(knee_high - knee_low)) as f32,
            blend: (threshold - knee_low).min(1.0),
            floor,
            vintage,
            makeup: 10f64.powf(gain / 200.0) as f32,
            dc: ((tan - 1.0) / (tan + 1.0)) as f32,
        }
    }

    /// The gain for an envelope whose natural log is `x`.
    ///
    /// Hard has no knee: its two ends are the threshold, and above it the
    /// gain is `(envelope / threshold)^(1 / ratio - 1)`. Medium and Soft
    /// have a parabola in between. Vintage is an S from 7 dB under the
    /// threshold to the knee's end, and takes no more off above that.
    fn gain(&self, x: f32) -> f32 {
        if x < self.knee_low {
            return 1.0;
        }
        let log = if !self.vintage {
            if x < self.knee_high {
                let over = x - self.knee_low;
                self.slope * over * over * self.half
            } else {
                (x - self.threshold) * self.slope
            }
        } else if x < self.knee_high {
            let t = (x - self.knee_low) * self.span;
            let s = (f64::from(t * t) * (3.0 - f64::from(t) * 2.0)) as f32;
            (self.knee_high - self.threshold) * self.slope * (t + (s - t) * self.blend)
        } else {
            return self.floor;
        };
        f64::from(log).exp() as f32
    }
}

/// Fruity Compressor. A new one has FL's defaults: a ratio of 1, which
/// leaves the limiter and the high pass.
#[derive(Debug, Clone, PartialEq)]
pub struct Compressor {
    params: [i32; 6],
    /// Whether FL tells its host of the 64 frames.
    report_latency: bool,
    /// The sidechain input the state chose, 0 for none, and the block of
    /// it for the next [`Effect::process`].
    sidechain: usize,
    side: Sidechain,
    rate: f32,
    curve: Curve,
    /// The high pass of each side: its allpass and the sample before.
    allpass: [f32; 2],
    before: [f32; 2],
    /// The envelope of the level.
    envelope: f32,
    /// The gain the attack and the release have smoothed, and a follower
    /// that falls with it and comes back up at the release's pace.
    gain: f32,
    slow: f32,
    /// What the gain keeps of its way up just now.
    release: f32,
    /// The limiter's gain, what its way back up keeps growing by, and how
    /// many frames it still holds.
    limit: f32,
    limit_release: f32,
    hold: u32,
    /// The limiter's gain for the frame that leaves now.
    applied: f32,
    /// The limiter's gains for the frames to come and the frames it has
    /// taken in and not yet given out, with where both are at.
    gains: [f32; RING],
    held: [[f32; 2]; LOOKAHEAD],
    at: usize,
}

impl Default for Compressor {
    fn default() -> Self {
        let rate = 44_100.0;
        let mut plugin = Compressor {
            params: DEFAULTS,
            report_latency: true,
            sidechain: 0,
            side: Sidechain::default(),
            rate,
            curve: Curve::new(&DEFAULTS, rate),
            allpass: [0.0; 2],
            before: [0.0; 2],
            envelope: 0.0,
            gain: 1.0,
            slow: 1.0,
            release: 0.5,
            limit: 1.0,
            limit_release: LIMIT_RELEASE,
            hold: 0,
            applied: 1.0,
            gains: [1.0; RING],
            held: [[0.0; 2]; LOOKAHEAD],
            at: 0,
        };
        plugin.reset();
        plugin
    }
}

impl Compressor {
    /// Take parameters that are inside their ranges. As in FL, working the
    /// curve out anew also lets the limiter go and puts the slow follower
    /// at 1.
    fn set(&mut self, params: [i32; 6]) {
        self.params = params;
        self.curve = Curve::new(&params, self.rate);
        self.slow = 1.0;
        self.limit = 1.0;
        self.hold = 0;
        self.limit_release = LIMIT_RELEASE;
    }

    /// How many frames FL tells its host the output is late by: 64, or 0
    /// when the state has that switched off. The output is 64 frames late
    /// either way.
    pub fn reported_latency(&self) -> usize {
        if self.report_latency { LOOKAHEAD } else { 0 }
    }

    /// One frame through the high pass, the gain and the make-up. The
    /// level is that of `detect`, the sidechain's frame, when there is
    /// one, and else of what the high pass lets through.
    fn squeeze(&mut self, left: f32, right: f32, detect: Option<[f32; 2]>) -> [f32; 2] {
        let curve = self.curve;
        let mut passed = [0.0; 2];
        for (side, sample) in [left, right].into_iter().enumerate() {
            let x = (f64::from(sample) + TINY) as f32;
            self.allpass[side] = (x - self.allpass[side]) * curve.dc + self.before[side];
            self.before[side] = x;
            passed[side] = (f64::from(x - self.allpass[side]) * 0.5) as f32;
        }

        let heard = match detect {
            Some(frame) => frame.map(|sample| (f64::from(sample) + TINY) as f32),
            None => passed,
        };
        let level = heard[0].abs().max(heard[1].abs());
        let level = (f64::from(level) + TINY) as f32;
        let keep = if level <= self.envelope {
            curve.detect_down
        } else {
            curve.detect_up
        };
        self.envelope = level + (self.envelope - level) * keep;
        let wanted = curve.gain(f64::from(self.envelope).ln() as f32);

        // A gain on its way down keeps the attack's part, and one on its
        // way up the release's.
        let keep = if wanted <= self.gain {
            curve.attack
        } else {
            self.release
        };
        self.gain = wanted + (self.gain - wanted) * keep;
        self.slow = (self.gain + (self.slow - self.gain) * curve.release).max(self.gain);
        // How far the release goes towards its fastest: by how far the
        // gain is under its slow follower, or for Vintage by how far it is
        // under 0.707.
        let hurry = if curve.vintage {
            ((0.707 - f64::from(self.gain)) * 1.414) as f32
        } else {
            self.slow - self.gain
        };
        self.release = (curve.release + (curve.release_fast - curve.release) * hurry).max(0.0);

        let gain = self.gain * curve.makeup;
        passed.map(|sample| sample * gain)
    }

    /// One frame into the limiter, and the frame from 64 frames back out
    /// of it.
    ///
    /// A peak above full scale that the limiter's gain does not yet cover
    /// puts the gain at what brings it to the ceiling, and writes a ramp
    /// down to that gain over the 64 frames before the peak leaves. The
    /// gain then holds for 64 frames and goes back up at a pace that
    /// starts from nothing.
    fn limit(&mut self, frame: [f32; 2]) -> [f32; 2] {
        let peak = frame[0].abs().max(frame[1].abs());
        let over = f64::from(peak) > 1.0;
        let needed = if over {
            (CEILING / f64::from(peak)) as f32
        } else {
            1.0
        };
        if over && self.limit > needed {
            let from = self.applied;
            self.limit = needed;
            if self.hold == 0 {
                self.gains = [from; RING];
            }
            self.hold = LOOKAHEAD as u32;
            self.limit_release = 0.0;
            for ahead in 0..RING {
                let t = ((ahead + 1) as f32 / LOOKAHEAD as f32).min(1.0);
                let ramp = from + (needed - from) * t * t * (3.0 - t * 2.0);
                let slot = &mut self.gains[(self.at + ahead) % RING];
                if ramp < *slot {
                    *slot = ramp;
                }
            }
        } else if self.hold == 0 {
            let pace = f64::from(self.limit_release);
            self.limit_release = (pace + (0.09 - pace) * 5e-5) as f32;
            self.limit = if over {
                self.limit + (needed - self.limit) * self.limit_release
            } else {
                let limit = f64::from(self.limit);
                (limit + (1.0 - limit) * f64::from(self.limit_release)) as f32
            };
        }
        self.hold = self.hold.saturating_sub(1);

        self.applied = self.gains[self.at];
        self.gains[self.at] = self.limit;
        let held = &mut self.held[self.at % LOOKAHEAD];
        let out = held.map(|sample| sample * self.applied);
        *held = frame;
        self.at = (self.at + 1) % RING;
        out
    }
}

impl Effect for Compressor {
    /// FL reads a version under 2 as 28 bytes, and so does this. It turns
    /// a version above 3 down.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let version = state_version(bytes, V3_LEN)?;
        if version > 3 {
            return Err(StateError::Version { found: version });
        }
        let expected = match version {
            3 => V3_LEN,
            2 => V2_LEN,
            _ => V1_LEN,
        };
        if bytes.len() != expected {
            return Err(StateError::Length {
                expected,
                found: bytes.len(),
            });
        }
        let params = read_state(&bytes[4..V1_LEN], &RANGES)?;
        // A word the version does not have reads as 0.
        let word = |at: usize| {
            let word = bytes.get(at..).and_then(|rest| rest.first_chunk::<4>());
            word.map_or(0, |word| i32::from_le_bytes(*word))
        };
        self.report_latency = word(V1_LEN) & 1 == 1;
        self.sidechain = word(V2_LEN).max(0) as usize;
        self.set(params);
        Ok(())
    }

    /// The times and the high pass's corner are worked out for the rate.
    /// As in FL, a new rate starts the plugin from nothing. A rate that is
    /// not a number above 0 is not taken.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz > 0.0 {
            self.rate = hz;
            self.reset();
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (at, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
            let detect = self.side.frame(at);
            let squeezed = self.squeeze(*l, *r, detect);
            [*l, *r] = self.limit(squeezed);
        }
        self.side.clear();
    }

    fn sidechain_input(&self) -> usize {
        self.sidechain
    }

    fn set_sidechain(&mut self, index: usize, left: &[f32], right: &[f32]) {
        if index > 0 && index == self.sidechain {
            self.side.keep(left, right);
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

    /// The limiter's 64 frames, whether or not the state has FL tell its
    /// host of them: [`Compressor::reported_latency`] is what FL says.
    fn latency(&self) -> usize {
        LOOKAHEAD
    }

    /// What FL does when it flushes the plugin: the envelope at nothing,
    /// the gains at 1, the limiter's 64 frames empty.
    fn reset(&mut self) {
        self.allpass = [TINY as f32; 2];
        self.before = [TINY as f32; 2];
        self.envelope = 0.0;
        self.gain = 1.0;
        self.release = 0.5;
        self.applied = 1.0;
        self.gains = [1.0; RING];
        self.held = [[0.0; 2]; LOOKAHEAD];
        self.at = 0;
        self.side.clear();
        self.set(self.params);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{presets, sine, state};

    /// Threshold -20 dB, ratio 4, attack 15 ms, release 200 ms, Hard.
    const FOUR_TO_ONE: [i32; 6] = [-200, 40, 0, 150, 200, 0];

    fn v1(params: [i32; 6]) -> Vec<u8> {
        let mut bytes = state(&[1]);
        bytes.extend(state(&params));
        bytes
    }

    fn v2(params: [i32; 6], latency: i32) -> Vec<u8> {
        let mut bytes = state(&[2]);
        bytes.extend(state(&params));
        bytes.extend(state(&[latency]));
        bytes
    }

    fn v3(params: [i32; 6], latency: i32, sidechain: i32) -> Vec<u8> {
        let mut bytes = state(&[3]);
        bytes.extend(state(&params));
        bytes.extend(state(&[latency, sidechain]));
        bytes
    }

    fn with(bytes: &[u8]) -> Compressor {
        let mut plugin = Compressor::default();
        plugin.set_state(bytes).unwrap();
        plugin
    }

    fn db(gain: f64) -> f64 {
        20.0 * gain.log10()
    }

    /// A tone at a quarter of the rate, caught where every sample is the
    /// same size: `db` dBFS, two up and two down. The detector then has a
    /// level that stands still, and the gain can be read off any sample.
    fn steady(db: f64, len: usize) -> Vec<f32> {
        let size = 10f64.powf(db / 20.0) as f32;
        (0..len)
            .map(|n| if n % 4 < 2 { size } else { -size })
            .collect()
    }

    /// The gain each frame of a [`steady`] tone got: what left 64 frames
    /// later over what went in. Both sides get the input.
    ///
    /// Each frame is read against the one two on, which has the other
    /// sign. That takes out what the high pass still holds of a louder
    /// tone before, which is a tenth of a tone 50 dB down.
    fn gains(plugin: &mut Compressor, input: &[f32]) -> Vec<f64> {
        let mut left = input.to_vec();
        left.extend([0.0; LOOKAHEAD]);
        let mut right = left.clone();
        plugin.process(&mut left, &mut right);
        assert!(left == right);
        let swing = |samples: &[f32], n: usize| f64::from(samples[n]) - f64::from(samples[n + 2]);
        let frames = 0..input.len() - 2;
        frames
            .map(|n| swing(&left, n + LOOKAHEAD) / swing(input, n))
            .collect()
    }

    /// One tone after the other with nothing in between.
    fn tones(parts: &[(f64, usize)]) -> Vec<f32> {
        let whole = parts.iter().flat_map(|(db, len)| steady(*db, *len));
        whole.collect()
    }

    /// The doc's time constant: what is left of a difference one sample
    /// on.
    fn doc_coef(ms: f64, rate: f64) -> f64 {
        0.1f64.powf(1000.0 / (ms * rate))
    }

    /// One neper in dB.
    const NEPER: f64 = 8.685_889_638_065_035;

    /// The gain in dB the doc's curves give a level that stands still, all
    /// of it worked out in dB.
    fn doc_gain(params: [i32; 6], level: f64) -> f64 {
        let threshold = f64::from(params[0]) / 10.0;
        let ratio = f64::from(params[1]) / 10.0;
        let knee = f64::from([0, 6, 7, 15][params[5] as usize % 4]);
        let slope = 1.0 / ratio - 1.0;
        let low = threshold - knee;
        if level < low {
            return 0.0;
        }
        if params[5] % 4 != 2 {
            let high = threshold + knee;
            return if level < high {
                slope * (level - low).powi(2) * 0.5 / (high - low)
            } else {
                slope * (level - threshold)
            };
        }
        // Vintage: the knee ends 1.25 * ratio^0.25 nepers and its own
        // width above the threshold, on the gain the slope has before the
        // width is added.
        let reach = 1.25 * ratio.powf(0.25) * NEPER;
        let floor = reach * slope;
        let high = threshold + reach + knee;
        if level >= high {
            return floor;
        }
        let t = (level - low) / (high - low);
        let blend = (knee / NEPER).min(1.0);
        floor * (t + (t * t * (3.0 - 2.0 * t) - t) * blend)
    }

    /// The gain in dB a level that stands still has got after 3 seconds.
    fn settled(params: [i32; 6], level: f64) -> f64 {
        let mut plugin = with(&v1(params));
        let gains = gains(&mut plugin, &steady(level, 3 * 44_100));
        db(*gains.last().unwrap())
    }

    #[test]
    fn a_steady_tone_above_the_threshold_settles_to_the_gain_of_the_formula() {
        // 10 dB over a threshold of -20 dB at 4:1: (env / thr)^(1/4 - 1),
        // which is 7.5 dB off.
        let want = db(10f64.powf(10.0 / 20.0).powf(1.0 / 4.0 - 1.0));
        assert!((want + 7.5).abs() < 1e-12);
        assert!((doc_gain(FOUR_TO_ONE, -10.0) - want).abs() < 1e-12);

        // From silence, where the attack brings the gain down.
        let mut plugin = with(&v1(FOUR_TO_ONE));
        let second = 44_100;
        let input = tones(&[(-10.0, second), (0.0, second), (-10.0, second)]);
        let gains = gains(&mut plugin, &input);
        let down = db(gains[second - 4]);
        assert!((down - want).abs() < 0.01, "{down}");
        // And from a louder tone, where the release brings it back up:
        // 15 dB off at 0 dBFS, still more than 10 dB off 10 ms on, and at
        // the formula's gain a second later.
        assert!((db(gains[2 * second - 4]) + 15.0).abs() < 0.01);
        assert!(db(gains[2 * second + 441]) < -10.0);
        let up = db(gains[3 * second - 4]);
        assert!((up - want).abs() < 0.1, "{up}");
        assert!((up - want).abs() < 0.01);

        // Under the threshold nothing is taken off.
        assert!(settled(FOUR_TO_ONE, -20.5).abs() < 0.01);
        assert!((settled(FOUR_TO_ONE, -19.0) + 0.75).abs() < 0.01);
        // The make-up gain is 10^(v / 200) on top: +6 dB at 60.
        let made_up = settled([-200, 40, 60, 150, 200, 0], -10.0);
        assert!((made_up - (want + 6.0)).abs() < 0.01, "{made_up}");
    }

    #[test]
    fn each_type_has_its_knee() {
        // Medium is 6 dB either side of the threshold and Soft 15. Inside
        // it the gain in dB is a parabola, and above it the hard line.
        for kind in [0, 1, 3] {
            let params = [-200, 40, 0, 150, 200, kind];
            for level in [-36.0, -30.0, -24.0, -20.0, -17.0, -12.0, -6.0, -2.0] {
                let got = settled(params, level);
                let want = doc_gain(params, level);
                assert!((got - want).abs() < 0.02, "type {kind} at {level}: {got}");
            }
        }
        // At the threshold a knee of K dB has taken (1 - 1/R) * K / 4 off.
        assert!(settled([-200, 40, 0, 150, 200, 0], -20.0).abs() < 0.01);
        assert!((settled([-200, 40, 0, 150, 200, 1], -20.0) + 0.75 * 1.5).abs() < 0.02);
        assert!((settled([-200, 40, 0, 150, 200, 3], -20.0) + 0.75 * 3.75).abs() < 0.02);

        // Vintage at 8:1 from -15 dB, FL's "Vintage" preset: an S that
        // ends 16 dB down and takes no more off however loud the tone.
        let vintage = [-150, 80, 0, 292, 1532, 2];
        let floor = doc_gain(vintage, 20.0);
        assert!((floor + 16.0).abs() < 0.03, "{floor}");
        for level in [-25.0, -21.0, -15.0, -8.0, 0.0] {
            let got = settled(vintage, level);
            let want = doc_gain(vintage, level);
            assert!((got - want).abs() < 0.02, "vintage at {level}: {got}");
            assert!(got >= floor - 0.02 && got < 0.01);
        }
        assert!(settled(vintage, -23.0).abs() < 0.01);
        // "Master V" is capped at 12.9 dB.
        assert!((doc_gain([-220, 49, 75, 181, 262, 6], 20.0) + 12.9).abs() < 0.05);
        // The knee's end is 7 dB past the S: -15 + 18.3 + 7 dB, where the
        // curve gets to its floor and not before.
        assert!((doc_gain(vintage, 10.0) - floor).abs() > 1e-6);
        assert_eq!(doc_gain(vintage, 10.4), floor);
        let curve = Curve::new(&vintage, 44_100.0);
        let top = f64::from(curve.knee_high) * NEPER;
        assert!((top - (-15.0 + 18.26 + 7.0)).abs() < 0.01, "{top}");
        assert!((db(f64::from(curve.floor)) - floor).abs() < 1e-4);
    }

    #[test]
    fn the_attack_and_the_release_fall_to_a_tenth_in_their_times() {
        for (rate, attack, release) in [
            (44_100.0, 150, 200),
            (48_000.0, 150, 200),
            (44_100.0, 50, 60),
            (96_000.0, 400, 30),
        ] {
            let params = [-200, 40, 0, attack, release, 0];
            let mut plugin = with(&v1(params));
            plugin.set_sample_rate(rate);
            let (attack_ms, release_ms) = (f64::from(attack) / 10.0, f64::from(release));
            let floor = 10f64.powf(-7.5 / 20.0);

            // A tone under the threshold, then the same 30 dB louder. The
            // gain goes from 1 to the formula's, and what is left of the
            // way after k samples is coef(attack)^k.
            let second = rate as usize;
            let input = tones(&[(-40.0, 4_000), (-10.0, second), (-40.0, second)]);
            let gains = gains(&mut plugin, &input);
            assert!((gains[3_990] - 1.0).abs() < 1e-3);
            let down = &gains[4_000..4_000 + second];
            let left = |gain: f64| (gain - floor) / (1.0 - floor);
            let reached = down.iter().position(|g| left(*g) <= 0.1).unwrap();
            let want = attack_ms * f64::from(rate) / 1000.0;
            assert!(
                (reached as f64 - want).abs() < 0.05 * want,
                "attack {attack_ms} ms at {rate}: {reached} and not {want}"
            );
            let c = doc_coef(attack_ms, f64::from(rate));
            for k in [want as usize / 4, want as usize / 2, want as usize] {
                // The detector takes a sample or two to get to the level.
                let by_doc = c.powi(k as i32);
                assert!((left(down[k]) - by_doc).abs() < 0.02, "attack at {k}");
            }

            // Back under the threshold the gain goes up to 1, and what is
            // left after k samples is coef(release)^k. The detector lets
            // go in 1 ms to a tenth, so it is under the threshold half a
            // ms in.
            let up = &gains[4_000 + second..];
            let left = |gain: f64| (1.0 - gain) / (1.0 - floor);
            let lag = (0.5 * f64::from(rate) / 1000.0) as usize;
            let reached = up.iter().position(|g| left(*g) <= 0.1).unwrap();
            let want = release_ms * f64::from(rate) / 1000.0;
            assert!(
                (reached as f64 - want).abs() < 0.05 * want,
                "release {release_ms} ms at {rate}: {reached} and not {want}"
            );
            let c = doc_coef(release_ms, f64::from(rate));
            for k in [want as usize / 4, want as usize / 2, want as usize] {
                let by_doc = c.powi((k - lag) as i32);
                assert!((left(up[k]) - by_doc).abs() < 0.02, "release at {k}");
            }
        }
        // The doc's two detector coefficients at 44.1 kHz, and no time at
        // all is no coefficient.
        assert!((f64::from(coef(0.01, 44_100.0)) - 0.0054).abs() < 5e-5);
        assert!((f64::from(coef(1.0, 44_100.0)) - 0.949).abs() < 5e-4);
        assert_eq!(coef(0.0, 44_100.0), 0.0);
        for (ms, rate) in [(15.0f32, 44_100.0f32), (200.0, 48_000.0), (1.0, 96_000.0)] {
            let want = doc_coef(f64::from(ms), f64::from(rate));
            assert!((f64::from(coef(ms, rate)) - want).abs() < 1e-7);
        }
    }

    #[test]
    fn the_r_types_let_go_faster_after_a_sudden_deep_cut() {
        // 0 dBFS over a threshold of -30 dB at 10:1 is 27 dB off. Then a
        // tone under the threshold. This gives the gain in dB 50 ms after
        // the loud tone ends and at the end of 3 seconds.
        let recovered = |kind: i32, loud: usize| {
            let mut plugin = with(&v1([-300, 100, 0, 0, 200, kind]));
            let input = tones(&[(0.0, loud), (-50.0, 3 * 44_100)]);
            let gains = gains(&mut plugin, &input);
            (db(gains[loud + 2_205]), db(gains[loud + 3 * 44_100 - 4]))
        };
        // Hard has gone the part of the way back that coef(release)
        // gives. The detector takes 1.5 ms, 66 samples, to fall the 30 dB
        // to the threshold first.
        let floor = 10f64.powf(-27.0 / 20.0);
        let want = 1.0 - (1.0 - floor) * doc_coef(200.0, 44_100.0).powi(2_205 - 66);
        for loud in [220, 44_100] {
            let (hard, end) = recovered(0, loud);
            assert!((hard - db(want)).abs() < 0.1, "{hard} and not {}", db(want));
            assert!(end.abs() < 0.01);
        }
        // After 5 ms of the loud tone Hard/R is most of the way back by
        // then: its slow follower of the gain is still far above the gain,
        // which takes the release towards a tenth of its time.
        let (fast, end) = recovered(4, 220);
        assert!(fast > db(want) + 4.0, "{fast} against {}", db(want));
        assert!(end.abs() < 0.01);
        // After a second of it the follower has come down to the gain,
        // and there is no hurry left.
        let (fast, _) = recovered(4, 44_100);
        assert!((fast - db(want)).abs() < 0.1, "{fast} and not {}", db(want));

        // The curves are those of the first four.
        for kind in [4, 5, 7] {
            let got = settled([-200, 40, 0, 150, 200, kind], -10.0);
            let want = doc_gain([-200, 40, 0, 150, 200, kind], -10.0);
            assert!((got - want).abs() < 0.02, "type {kind}: {got}");
        }
        // Vintage/R hurries by how far the gain is under 0.707. Above
        // that its release coefficient is past the nominal one, and the
        // gain comes to rest where the coefficient is 1, short of unity.
        let (_, rest) = recovered(6, 44_100);
        let (slow, quick) = (doc_coef(200.0, 44_100.0), doc_coef(20.0, 44_100.0));
        let want = 0.707 - (1.0 - slow) / (quick - slow) / 1.414;
        assert!(
            (rest - db(want)).abs() < 0.05,
            "{rest} and not {}",
            db(want)
        );
        assert!(rest < -1.0);
    }

    #[test]
    fn the_limiter_stops_a_burst_over_full_scale_at_minus_0_1_dbfs() {
        let ceiling = 10f64.powf(-0.1 / 20.0);
        assert!((ceiling - CEILING).abs() < 1e-6);
        let burst = |gain: f32| {
            let mut input = vec![0.0; 500];
            input.extend(sine(1_000.0 / 44_100.0, 4_410).iter().map(|v| v * gain));
            input.extend(vec![0.0; 500]);
            input
        };
        let peak = |out: &[f32]| f64::from(out.iter().fold(0.0f32, |peak, v| peak.max(v.abs())));
        // The loudest that leaves of the frames that would have passed
        // full scale at a gain of `gain`. The high pass turns a 1 kHz tone
        // by a sixth of a degree, so a frame within 2 % of full scale may
        // be on either side of it.
        let caught = |input: &[f32], out: &[f32], gain: f32| {
            let over = (0..input.len() - LOOKAHEAD).filter(|n| (input[*n] * gain).abs() > 1.02);
            over.map(|n| f64::from(out[n + LOOKAHEAD].abs()))
                .fold(0.0, f64::max)
        };

        // A burst at 0 dBFS with 6 dB of make-up would be at +6 dBFS.
        // Every frame of it that would pass full scale leaves under the
        // ceiling, and its crests leave on it.
        let mut plugin = with(&v1([0, 10, 60, 150, 200, 0]));
        let input = burst(1.0);
        let (mut left, mut right) = (input.clone(), input.clone());
        plugin.process(&mut left, &mut right);
        let got = caught(&input, &left, 10f32.powf(0.3));
        assert!(got <= CEILING + 1e-6, "{got}");
        assert!((db(got) + 0.1).abs() < 0.01, "{} dBFS", db(got));
        assert!(left == right);
        // A frame just under full scale is not the limiter's business, so
        // the block as a whole stays under full scale and no more.
        assert!(peak(&left) <= 1.0);
        // The gain comes down ahead of the peak: no step between two
        // samples is much bigger than the tone's own 0.14.
        let step = left.windows(2).map(|w| (w[1] - w[0]).abs());
        assert!(step.fold(0.0f32, f32::max) < 0.2);

        // The same with the burst itself over full scale, on one side.
        // The quiet side gets the gain of the loud one.
        let mut plugin = Compressor::default();
        let (mut left, mut right) = (burst(2.0), burst(0.1));
        plugin.process(&mut left, &mut right);
        let got = caught(&input, &left, 2.0);
        assert!(
            got <= CEILING + 1e-6 && (db(got) + 0.1).abs() < 0.01,
            "{got}"
        );
        assert!((peak(&right) - 0.05).abs() < 0.001);

        // A burst at 0 dBFS is caught too: the high pass holds a little
        // of the tone's first half wave, which takes the crests on the
        // other side a third of a percent past full scale.
        let mut plugin = Compressor::default();
        let (mut left, mut right) = (burst(1.0), burst(1.0));
        plugin.process(&mut left, &mut right);
        assert!((db(peak(&left)) + 0.1).abs() < 0.01, "{}", db(peak(&left)));
        // What stays under full scale is left alone, above the ceiling or
        // not: FL's limiter only wakes above 1.0.
        let mut plugin = Compressor::default();
        let (mut left, mut right) = (burst(0.994), burst(0.994));
        plugin.process(&mut left, &mut right);
        assert!(peak(&left) > 0.9939 && peak(&left) < 1.0, "{}", peak(&left));

        // After a peak the limiter lets go at a pace that starts from
        // nothing. A tone 12 dB over full scale that falls to -30 dBFS
        // still has the loud tone's gain 50 frames on, since the gain a
        // frame gets is the one from 64 frames before it. It is half way
        // back 400 frames on and has its own gain within 0.1 s.
        let mut plugin = Compressor::default();
        let gains = gains(&mut plugin, &tones(&[(12.0, 1_000), (-30.0, 6_000)]));
        for frame in [990, 1_050] {
            assert!(
                (db(gains[frame]) + 12.1).abs() < 0.01,
                "{}",
                db(gains[frame])
            );
        }
        assert!(db(gains[1_400]) < -3.0 && db(gains[1_400]) > -12.0);
        assert!(db(gains[5_410]).abs() < 0.01, "{}", db(gains[5_410]));
    }

    #[test]
    fn the_output_is_64_frames_late_and_has_no_offset() {
        // A click comes out 64 frames on, as it went in.
        let mut plugin = Compressor::default();
        let (mut left, mut right) = ([0.0; 200], [0.0; 200]);
        (left[10], right[10]) = (0.5, -0.25);
        plugin.process(&mut left, &mut right);
        assert!((left[74] - 0.5).abs() < 1e-3 && (right[74] + 0.25).abs() < 1e-3);
        assert!(left[..74].iter().all(|v| v.abs() < 1e-6));
        // The default is a ratio of 1: a tone passes at unity.
        let mut plugin = Compressor::default();
        let passed = gains(&mut plugin, &steady(-6.0, 4_000));
        assert!((passed[3_990] - 1.0).abs() < 1e-3);
        assert_eq!(plugin.reported_latency(), 64);

        // A level that stands still is gone in a second, at any rate: the
        // high pass is at 3 Hz, so 0.1 s leaves e^(-0.6 pi) of it. Its
        // state is a float, which stops short of nothing by 80 dB.
        for rate in [44_100usize, 96_000] {
            let mut plugin = Compressor::default();
            plugin.set_sample_rate(rate as f32);
            let (mut left, mut right) = (vec![0.5; rate], vec![-0.5; rate]);
            plugin.process(&mut left, &mut right);
            let want = 0.5 * (-0.6 * std::f64::consts::PI).exp();
            let got = f64::from(left[rate / 10 + LOOKAHEAD]);
            assert!((got - want).abs() < 0.002, "{got} and not {want}");
            assert!(left[rate - 1].abs() < 2e-4 && right[rate - 1].abs() < 2e-4);
        }
    }

    #[test]
    fn every_type_and_every_version_of_the_state_loads() {
        let params = [-130, 100, 130, 0, 800, 4];
        for kind in 0..8 {
            let params = [-130, 100, 130, 0, 800, kind];
            let plugin = with(&v2(params, 1));
            assert_eq!(plugin.params, params);
            assert_eq!(plugin.curve.vintage, kind % 4 == 2);
            // Only the types from 4 on have a faster release to go to.
            let hurries = plugin.curve.release_fast < plugin.curve.release;
            assert_eq!(hurries, kind >= 4, "type {kind}");
        }
        // 28 bytes of version 1, with nothing after the parameters.
        assert_eq!(v1(params).len(), 28);
        let old = with(&v1(params));
        assert_eq!((old.reported_latency(), old.sidechain), (0, 0));
        // 32 bytes of version 2, with the lowest bit of the flag.
        assert_eq!(v2(params, 1).len(), 32);
        assert_eq!(with(&v2(params, 1)).reported_latency(), 64);
        assert_eq!(with(&v2(params, 3)).reported_latency(), 64);
        assert_eq!(with(&v2(params, 0)), old);
        assert_eq!(with(&v2(params, 2)), old);
        // 36 bytes of version 3, with the sidechain input.
        assert_eq!(v3(params, 1, 0).len(), 36);
        assert_eq!(with(&v3(params, 0, 0)), old);
        assert_eq!(with(&v3(params, 1, 2)).sidechain, 2);
        assert_eq!(with(&v3(params, 1, -1)).sidechain, 0);
        // FL reads any version under 2 as version 1.
        let mut ancient = v1(params);
        ancient[..4].copy_from_slice(&0i32.to_le_bytes());
        assert_eq!(with(&ancient), old);
        assert_eq!(with(&v3(params, 1, 2)).sidechain_input(), 2);
        assert_eq!(old.sidechain_input(), 0);
    }

    #[test]
    fn a_sidechain_input_sets_the_gain_in_place_of_the_plugin_s_own() {
        // A tone 30 dB under the threshold, which the plugin leaves alone.
        let quiet = steady(-50.0, 6_000);
        let mut alone = with(&v3(FOUR_TO_ONE, 1, 1));
        let passed = gains(&mut alone, &quiet);
        assert!((passed[5_990] - 1.0).abs() < 1e-3);

        // With a sidechain 20 dB over the threshold it comes down as that
        // one would: 20 dB at 4 : 1 is 15 dB less.
        let ducked = |plugin: &mut Compressor, loud: &[f32]| -> Vec<f64> {
            let mut left = quiet.clone();
            let mut right = quiet.clone();
            for (at, (l, r)) in left.chunks_mut(512).zip(right.chunks_mut(512)).enumerate() {
                let side = &loud[at * 512..][..l.len()];
                plugin.set_sidechain(1, side, side);
                plugin.process(l, r);
            }
            let swing =
                |samples: &[f32], n: usize| f64::from(samples[n]) - f64::from(samples[n + 2]);
            let frames = 0..quiet.len() - LOOKAHEAD - 2;
            frames
                .map(|n| swing(&left, n + LOOKAHEAD) / swing(&quiet, n))
                .collect()
        };
        let loud = vec![1.0; 6_000];
        let mut fed = with(&v3(FOUR_TO_ONE, 1, 1));
        let got = ducked(&mut fed, &loud);
        assert!((db(got[5_900]) + 15.0).abs() < 0.1, "{}", db(got[5_900]));

        // A block it is handed none for goes by its own level again, and
        // so does every block of another input than the state's.
        let passed = gains(&mut fed, &steady(-50.0, 40_000));
        assert!((passed[39_990] - 1.0).abs() < 1e-3);
        let mut other = with(&v3(FOUR_TO_ONE, 1, 1));
        other.set_sidechain(2, &loud, &loud);
        assert_eq!(other, with(&v3(FOUR_TO_ONE, 1, 1)));
        let mut none = with(&v3(FOUR_TO_ONE, 1, 0));
        none.set_sidechain(0, &loud, &loud);
        none.set_sidechain(1, &loud, &loud);
        assert_eq!(none, with(&v3(FOUR_TO_ONE, 1, 0)));

        // A sidechain shorter than the block covers the frames it has.
        let mut short = with(&v3(FOUR_TO_ONE, 1, 1));
        short.set_sidechain(1, &loud[..10], &loud[..10]);
        short.process(&mut [0.0; 20], &mut [0.0; 20]);
        assert!(short.side.left.is_empty() && short.side.left.capacity() >= 4096);
    }

    #[test]
    fn a_state_the_plugin_does_not_write_is_turned_down() {
        let mut plugin = with(&v3([-84, 32, 34, 108, 117, 7], 1, 0));
        let before = plugin.clone();
        let mut newer = v3(DEFAULTS, 1, 0);
        newer[..4].copy_from_slice(&4i32.to_le_bytes());
        assert_eq!(
            plugin.set_state(&newer),
            Err(StateError::Version { found: 4 })
        );
        // Each version has its one length.
        for (bytes, expected) in [
            (v1(DEFAULTS)[..27].to_vec(), 28),
            (v2(DEFAULTS, 1)[..28].to_vec(), 32),
            (v3(DEFAULTS, 1, 0)[..32].to_vec(), 36),
            (v2(DEFAULTS, 1).into_iter().chain([0; 4]).collect(), 32),
            (vec![3, 0, 0], 36),
            (vec![], 36),
        ] {
            assert_eq!(
                plugin.set_state(&bytes),
                Err(StateError::Length {
                    expected,
                    found: bytes.len()
                })
            );
        }
        for (param, value) in [
            (0, -601),
            (0, 1),
            (1, 3),
            (1, 301),
            (2, -301),
            (2, 301),
            (3, -1),
            (3, 4001),
            (4, 0),
            (4, 4001),
            (5, -1),
            (5, 8),
        ] {
            let mut params = DEFAULTS;
            params[param] = value;
            for bytes in [v1(params), v2(params, 1), v3(params, 1, 0)] {
                assert_eq!(
                    plugin.set_state(&bytes),
                    Err(StateError::Range { param, value })
                );
            }
        }
        assert_eq!(plugin, before);
    }

    #[test]
    fn a_moved_parameter_is_the_same_plugin_as_its_state() {
        let mut plugin = Compressor::default();
        assert_eq!(plugin, with(&v2(DEFAULTS, 1)));
        assert_eq!(plugin.param_count(), 6);
        let moves = [
            (0, -84.2),
            (1, 31.6),
            (2, 400.0),
            (3, -5.0),
            (4, 0.0),
            (5, 7.4),
        ];
        for (index, value) in moves {
            plugin.set_param(index, value);
        }
        plugin.set_param(6, 1.0);
        assert_eq!(plugin.param(6), 0.0);
        assert_eq!(plugin, with(&v2([-84, 32, 300, 0, 1, 7], 1)));
        // A reset is a new plugin with the same parameters.
        plugin.process(&mut [0.9; 300], &mut [-0.9; 300]);
        assert_ne!(plugin, with(&v2([-84, 32, 300, 0, 1, 7], 1)));
        plugin.reset();
        assert_eq!(plugin, with(&v2([-84, 32, 300, 0, 1, 7], 1)));
    }

    #[test]
    fn fl_s_presets_load_as_the_doc_reads_them() {
        let Some(presets) = presets(NAME) else { return };
        // 15 in `External` and 5 in `Internal`, every one of version 2
        // with the delay told.
        assert_eq!(presets.len(), 20);
        for preset in &presets {
            assert_eq!(preset.plugin, NAME, "{}", preset.file);
            assert_eq!(preset.state.len(), 32, "{}", preset.file);
            let plugin = with(&preset.state);
            assert_eq!(preset.state, v2(plugin.params, 1), "{}", preset.file);
            assert_eq!(plugin.sidechain_input(), 0);
        }
        let want = [
            ("Default", DEFAULTS),
            ("Fast Master", [-39, 18, 19, 160, 54, 0]),
            ("Master 1", [-84, 32, 34, 108, 117, 7]),
            ("Master V", [-220, 49, 75, 181, 262, 6]),
            ("Vintage", [-150, 80, 86, 292, 1532, 2]),
            ("V Driver", [-402, 182, 252, 365, 4000, 2]),
            ("Brickwall", [-130, 100, 130, 0, 800, 4]),
            ("Drums", [-125, 75, 90, 100, 200, 5]),
            ("Full Reset", DEFAULTS),
        ];
        for (file, params) in want {
            let preset = presets.iter().find(|preset| preset.file == file).unwrap();
            assert_eq!(with(&preset.state).params, params, "{file}");
        }
        let default = presets.iter().find(|preset| preset.file == "Default");
        assert_eq!(with(&default.unwrap().state), Compressor::default());
        // The doc's readings: "Default" takes nothing off, "V Driver"
        // squashes by up to 21.2 dB, and "Brickwall" has no attack time.
        assert_eq!(doc_gain(DEFAULTS, 0.0), 0.0);
        assert!((doc_gain([-402, 182, 252, 365, 4000, 2], 20.0) + 21.2).abs() < 0.05);
        assert_eq!(
            Curve::new(&[-130, 100, 130, 0, 800, 4], 44_100.0).attack,
            0.0
        );
    }
}
