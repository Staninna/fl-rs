//! Fruity Delay, the first of FL's delays: a tempo-synced echo with a
//! one-pole low pass in its feedback, as `docs/plugins/fruity-delay.md`
//! has it.
//!
//! The state is a version, 1, and six little-endian i32:
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Input | 0 to 1024 | 338 |
//! | 1 Feedback | 0 to 1024 | 512 |
//! | 2 Cutoff | 0 to 1024 | 1024 |
//! | 3 Tempo | 0 to 1024 | 0 |
//! | 4 Steps | 1 to 16 | 3 |
//! | 5 Mode | 0 to 1024 | 0 |
//!
//! The echo comes `steps` sixteenth notes after the input, at the song's
//! tempo when the tempo wheel is at 0 and at `round(60 + 220 v / 1024)`
//! BPM otherwise. Every echo is the one before it times the feedback,
//! through the low pass, and the dry input always passes at full level.
//! The mode is normal, inverted stereo (the echoes swap sides) or ping pong
//! (they swap sides every time round).

use crate::parts::glide;
use crate::{Effect, StateError, knob_value, read_state, state_version};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Delay";

/// The ranges of the six wheels.
const RANGES: [(i32, i32); 6] = [
    (0, 1024),
    (0, 1024),
    (0, 1024),
    (0, 1024),
    (1, 16),
    (0, 1024),
];
const DEFAULTS: [i32; 6] = [338, 512, 1024, 0, 3, 0];
/// The state's length: the version and the six wheels.
const LEN: usize = 28;
/// FL's slowest tempo, which the delay line is sized for.
const SLOWEST_BPM: f32 = 10.0;
/// What the filter keeps from becoming a denormal: 2^-24.
const TINY: f64 = 5.960_464_477_539_063e-8;

/// Where a wheel sits between its ends, as FL hands it on: `f32`.
fn norm(value: i32, (min, max): (i32, i32)) -> f32 {
    let n = f64::from(value - min) * (1.0 / f64::from(max - min));
    n.clamp(0.0, 1.0) as f32
}

/// Delphi's `Round`: to the nearest, ties to even.
fn round(x: f64) -> i32 {
    x.round_ties_even() as i32
}

/// The three ways round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    Inverted,
    PingPong,
}

/// Fruity Delay. A new one has FL's defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct Delay {
    params: [i32; 6],
    hz: f32,
    /// The song's tempo, in BPM.
    song_bpm: f32,
    /// The input gain the wheel asks for and the one the last block ended
    /// on.
    input: f32,
    input_now: f32,
    /// The feedback the wheel asks for and the one the last block ended
    /// on. A new plugin starts the latter at 0, as FL's does.
    feedback: f32,
    feedback_now: f32,
    /// The low pass's coefficient, 1 for none.
    cutoff: f32,
    mode: Mode,
    /// How many frames late the echo is.
    late: usize,
    /// The echoes yet to come out, a frame each, and where the next one is.
    ring: Vec<[f32; 2]>,
    at: usize,
    /// The low pass's two sides.
    filter: [f64; 2],
}

impl Default for Delay {
    fn default() -> Self {
        let mut plugin = Delay {
            params: DEFAULTS,
            hz: 44_100.0,
            song_bpm: 140.0,
            input: 0.0,
            input_now: 0.0,
            feedback: 0.0,
            feedback_now: 0.0,
            cutoff: 1.0,
            mode: Mode::Normal,
            late: 1,
            ring: Vec::new(),
            at: 0,
            filter: [0.0; 2],
        };
        plugin.make_room();
        plugin.set(DEFAULTS);
        plugin.input_now = plugin.input;
        plugin
    }
}

impl Delay {
    /// A sixteenth note at this tempo, in frames: `round(rate * 15 / bpm)`.
    fn sixteenth(&self, bpm: f32) -> usize {
        let frames = (self.hz * 15.0).round() / bpm;
        round(f64::from(frames)).max(1) as usize
    }

    /// The tempo the delay keeps: the song's, or the wheel's.
    fn bpm(&self) -> f32 {
        if self.params[3] == 0 {
            self.song_bpm
        } else {
            let n = norm(self.params[3], RANGES[3]);
            round(f64::from(n * 220.0 + 60.0)) as f32
        }
    }

    /// Have the memory for the longest delay at this rate, 16 sixteenths
    /// at FL's slowest tempo, so that no wheel and no tempo asks for more.
    fn make_room(&mut self) {
        let longest = 16 * self.sixteenth(SLOWEST_BPM);
        self.ring
            .reserve_exact(longest.saturating_sub(self.ring.len()));
    }

    /// Work out how late the echo is. A new length starts empty, as FL
    /// sizes its line again then.
    fn size(&mut self) {
        let steps = round(f64::from(norm(self.params[4], RANGES[4]) * 15.0 + 1.0)).max(1);
        let late = self.sixteenth(self.bpm().max(SLOWEST_BPM)) * steps as usize;
        if late != self.ring.len() {
            self.late = late;
            self.ring.clear();
            self.ring.resize(late, [0.0; 2]);
            self.at = 0;
        }
    }

    /// Take parameters that are inside their ranges.
    fn set(&mut self, params: [i32; 6]) {
        self.params = params;
        self.input = norm(params[0], RANGES[0]);
        self.feedback = (f64::from(norm(params[1], RANGES[1])) * 0.99) as f32;
        let n = f64::from(norm(params[2], RANGES[2]));
        let curve = ((n * 21f64.ln()).exp() - 1.0) as f32;
        self.cutoff = (f64::from(curve) * 0.05) as f32;
        self.mode = match round(f64::from(norm(params[5], RANGES[5]) * 3.0 - 0.5)) {
            1 => Mode::Inverted,
            2 => Mode::PingPong,
            _ => Mode::Normal,
        };
        self.size();
    }

    /// The low pass on one side: `y += (x - y) * a`, kept in `f64` as FL's
    /// SSE loop keeps it, with what goes into the line flushed to 0 under
    /// 2^-24. Inverted stereo has no SSE loop in FL and keeps it in `f32`.
    fn filter(&mut self, side: usize, x: f32) -> f32 {
        let y = &mut self.filter[side];
        if self.mode == Mode::Inverted {
            let mut s = *y as f32;
            s = (x - s) * self.cutoff + s;
            *y = f64::from(s);
            if f64::from(s.abs()) <= TINY { 0.0 } else { s }
        } else {
            *y += (f64::from(x) - *y) * f64::from(self.cutoff);
            if y.abs() < TINY { 0.0 } else { *y as f32 }
        }
    }
}

impl Effect for Delay {
    /// FL leaves a state of a version above 1 alone.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let version = state_version(bytes, LEN)?;
        if version > 1 {
            return Err(StateError::Version { found: version });
        }
        if bytes.len() != LEN {
            return Err(StateError::Length {
                expected: LEN,
                found: bytes.len(),
            });
        }
        self.set(read_state(&bytes[4..], &RANGES)?);
        Ok(())
    }

    /// Asks for memory when the rate is higher than before. A rate that is
    /// not a number above 0 is not taken.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz > 0.0 {
            self.hz = hz;
            self.make_room();
            self.set(self.params);
        }
    }

    /// With the tempo wheel at 0 a new tempo sizes the line again, which
    /// empties it, as in FL.
    fn set_tempo(&mut self, bpm: f64) {
        if bpm.is_finite() && bpm > 0.0 {
            self.song_bpm = bpm as f32;
            self.size();
        }
    }

    /// Every frame: the echo due now comes out over the dry input, and the
    /// input at its gain plus the echo at the feedback goes through the
    /// low pass into the line. The input gain and the feedback each move
    /// towards their wheels by no more than `0.001 * 44100 / rate` a frame.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        if len == 0 {
            return;
        }
        let cap = (0.001 * 44_100.0 / f64::from(self.hz)) as f32;
        let (mut gain, gain_step) = glide(&mut self.input_now, self.input, cap, len);
        let (mut feedback, feedback_step) = glide(&mut self.feedback_now, self.feedback, cap, len);
        for (l, r) in left[..len].iter_mut().zip(&mut right[..len]) {
            let [echo_l, echo_r] = self.ring[self.at];
            let (into_l, into_r) = (*l * gain, *r * gain);
            let (back_l, back_r) = match self.mode {
                Mode::Normal => {
                    *l += echo_l;
                    *r += echo_r;
                    (echo_l, echo_r)
                }
                Mode::Inverted => {
                    *l += echo_r;
                    *r += echo_l;
                    (echo_l, echo_r)
                }
                Mode::PingPong => {
                    *l += echo_r;
                    *r += echo_l;
                    (echo_r, echo_l)
                }
            };
            let new_l = self.filter(0, back_l * feedback + into_l);
            let new_r = self.filter(1, back_r * feedback + into_r);
            self.ring[self.at] = [new_l, new_r];
            self.at += 1;
            if self.at == self.ring.len() {
                self.at = 0;
            }
            gain += gain_step;
            feedback += feedback_step;
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

    /// FL's flush: the line and the filter empty, and the input gain jumps
    /// to its wheel. The feedback keeps where it was.
    fn reset(&mut self) {
        self.ring.fill([0.0; 2]);
        self.at = 0;
        self.filter = [0.0; 2];
        self.input_now = self.input;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{presets, state};

    fn with(values: [i32; 6]) -> Delay {
        let mut plugin = Delay::default();
        plugin.set_sample_rate(48_000.0);
        plugin.set_tempo(120.0);
        let mut bytes = state(&[1]);
        bytes.extend(state(&values));
        plugin.set_state(&bytes).unwrap();
        plugin.reset();
        plugin
    }

    /// One impulse on the left through the plugin, `len` frames.
    fn impulse(plugin: &mut Delay, len: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut left, mut right) = (vec![0.0; len], vec![0.0; len]);
        left[0] = 1.0;
        for (l, r) in left.chunks_mut(512).zip(right.chunks_mut(512)) {
            plugin.process(l, r);
        }
        (left, right)
    }

    #[test]
    fn a_sixteenth_at_120_bpm_is_6000_frames_at_48_khz() {
        let plugin = with([1024, 0, 1024, 0, 1, 0]);
        assert_eq!(plugin.late, 6000);
        // The wheel's own tempo: 279 is round(119.94) = 120.
        let plugin = with([1024, 0, 1024, 279, 4, 0]);
        assert_eq!(plugin.bpm(), 120.0);
        assert_eq!(plugin.late, 24_000);
        // Its ends are 60 and 280.
        assert_eq!(with([0, 0, 1024, 1, 1, 0]).bpm(), 60.0);
        assert_eq!(with([0, 0, 1024, 1024, 1, 0]).bpm(), 280.0);
    }

    #[test]
    fn the_echoes_come_every_delay_at_the_feedback() {
        // Input 1, feedback 512 (0.495), no filter.
        let mut plugin = with([1024, 512, 1024, 0, 1, 0]);
        let (left, right) = impulse(&mut plugin, 20_000);
        assert_eq!(left[0], 1.0);
        let fb = (0.5f64 * 0.99) as f32;
        assert_eq!(left[6000], 1.0);
        assert!((left[12_000] - fb).abs() < 1e-7);
        assert!((left[18_000] - fb * fb).abs() < 1e-7);
        let quiet = left.iter().enumerate().filter(|(n, _)| n % 6000 != 0);
        assert!(quiet.into_iter().all(|(_, v)| *v == 0.0));
        assert!(right.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn inverted_stereo_swaps_the_echoes_and_ping_pong_swaps_them_each_time() {
        let mut plugin = with([1024, 1024, 1024, 0, 1, 512]);
        assert_eq!(plugin.mode, Mode::Inverted);
        let (left, right) = impulse(&mut plugin, 13_000);
        assert_eq!((left[6000], right[6000]), (0.0, 1.0));
        assert_eq!((left[12_000], right[12_000]), (0.0, 0.99));
        let mut plugin = with([1024, 1024, 1024, 0, 1, 1024]);
        assert_eq!(plugin.mode, Mode::PingPong);
        let (left, right) = impulse(&mut plugin, 19_000);
        assert_eq!((left[6000], right[6000]), (0.0, 1.0));
        assert_eq!((left[12_000], right[12_000]), (0.99, 0.0));
        assert!((right[18_000] - 0.99 * 0.99).abs() < 1e-6);
    }

    #[test]
    fn the_cutoff_is_a_one_pole_low_pass() {
        // 21^0.5 - 1, over 20.
        let plugin = with([1024, 0, 512, 0, 1, 0]);
        let a = (21f64.sqrt() - 1.0) / 20.0;
        assert!((f64::from(plugin.cutoff) - a).abs() < 1e-7);
        assert_eq!(with([1024, 0, 1024, 0, 1, 0]).cutoff, 1.0);
        assert_eq!(with([1024, 0, 0, 0, 1, 0]).cutoff, 0.0);
        let mut plugin = with([1024, 0, 512, 0, 1, 0]);
        let (left, _) = impulse(&mut plugin, 6003);
        assert!((f64::from(left[6000]) - a).abs() < 1e-7);
        assert!((f64::from(left[6001]) - a * (1.0 - a)).abs() < 1e-7);
    }

    #[test]
    fn a_new_plugin_brings_its_feedback_up_from_0() {
        let mut plugin = Delay::default();
        plugin.set_sample_rate(44_100.0);
        let (mut left, mut right) = (vec![0.0; 4], vec![0.0; 4]);
        plugin.process(&mut left, &mut right);
        assert!((plugin.feedback_now - 0.004).abs() < 1e-9);
    }

    #[test]
    fn the_presets_and_the_corpus_state_load() {
        let mut plugin = with(DEFAULTS);
        let mut bytes = state(&[1]);
        bytes.extend(state(&[608, 223, 1024, 279, 4, 809]));
        plugin.set_state(&bytes).unwrap();
        assert_eq!(plugin.mode, Mode::PingPong);
        assert_eq!(plugin.late, 4 * 6000);
        let mut newer = state(&[2]);
        newer.extend(state(&DEFAULTS));
        assert_eq!(
            plugin.set_state(&newer),
            Err(StateError::Version { found: 2 })
        );
        let Some(presets) = presets(NAME) else {
            return;
        };
        assert_eq!(presets.len(), 6);
        for preset in presets {
            Delay::default().set_state(&preset.state).unwrap();
        }
    }
}
