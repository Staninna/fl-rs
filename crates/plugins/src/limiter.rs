//! Fruity Limiter: a lookahead limiter, an RMS compressor, a gate and a
//! soft or hard clip at the ceiling, as `docs/plugins/fruity-limiter.md`
//! has it.
//!
//! The state is a little-endian i32 version, the plugin's 18 parameters as
//! i32, the same 18 again for the A/B compare, 13 bytes of what the display
//! shows, from version 6 on three bytes of padding and an i32 for the
//! sidechain input, and in version 7 a byte whose lowest bit takes the
//! margin off the ceiling. Version 5 is 161 bytes, version 6 is 168 and
//! version 7 is 169.
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Gain | 0 to 2000 | 1000 |
//! | 1 Soft saturation threshold | 0 to 1000 | 1000 |
//! | 2 Limiter ceiling | 0 to 2000 | 1000 |
//! | 3 Limiter attack time | 0 to 10000 | 1115 |
//! | 4 Limiter attack curve | 1 to 8 | 3 |
//! | 5 Limiter release time | 0 to 10000 | 5000 |
//! | 6 Limiter release curve | 1 to 8 | 3 |
//! | 7 Limiter peak window | 0 to 10000 | 2882 |
//! | 8 Compressor threshold | 0 to 1000 | 1000 |
//! | 9 Compressor ratio | -1000 to 1000 | 0 |
//! | 10 Compressor knee | -1000 to 1000 | 0 |
//! | 11 Compressor attack time | 0 to 10000 | 0 |
//! | 12 Compressor release time | 0 to 10000 | 6666 |
//! | 13 Compressor curve | 1 to 8 | 1 |
//! | 14 Compressor RMS window | 0 to 10000 | 650 |
//! | 15 Noise gain | 0 to 1000 | 1000 |
//! | 16 Noise threshold | 0 to 1000 | 0 |
//! | 17 Noise release time | 0 to 10000 | 4187 |
//!
//! A time wheel is `(501^x - 1) * 2` ms with `x` the wheel over 10000, a
//! gain wheel `(7^x - 1) / 6` and a ceiling wheel `(3^x - 1) / 2` with `x`
//! the wheel over 1000. A curve wheel is a number of one-pole stages in a
//! row, each with the time over the root of that number.
//!
//! The peak detector takes the highest level of the last window of the
//! input as it comes in. The sound itself is held back by the attack time,
//! so the gain for a frame knows the peak that is on its way: where the
//! peak times the gain is over the ceiling, the gain is the ceiling over
//! the peak. The ceiling the gain aims at is 0.975 of the wheel's unless
//! the state's flag says the whole of it. [`Effect::latency`] is the attack
//! time in frames.
//!
//! A state with a sidechain input has the compressor take its level from
//! the block a host hands [`Effect::set_sidechain`]. For a block the host
//! hands it none, the compressor listens to the plugin's own input. The
//! limiter and the gate always do.

use crate::{Effect, Sidechain, StateError, knob_value, read_state, state_version};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Limiter";

/// The range of each parameter's wheel.
const RANGES: [(i32, i32); 18] = [
    (0, 2000),
    (0, 1000),
    (0, 2000),
    (0, 10000),
    (1, 8),
    (0, 10000),
    (1, 8),
    (0, 10000),
    (0, 1000),
    (-1000, 1000),
    (-1000, 1000),
    (0, 10000),
    (0, 10000),
    (1, 8),
    (0, 10000),
    (0, 1000),
    (0, 1000),
    (0, 10000),
];
const DEFAULTS: [i32; 18] = [
    1000, 1000, 1000, 1115, 3, 5000, 3, 2882, 1000, 0, 0, 0, 6666, 1, 650, 1000, 0, 4187,
];

/// How long each version of the state is, and where the parameters end.
const V5_LEN: usize = 161;
const V6_LEN: usize = 168;
const V7_LEN: usize = 169;
const PARAMS_END: usize = 76;
/// Where version 6 has the sidechain input and version 7 its flag.
const SIDECHAIN_AT: usize = 164;
const FLAG_AT: usize = 168;

/// The most stages a curve wheel asks for.
const STAGES: usize = 8;

/// The part of the ceiling the gain aims at without the state's flag.
const MARGIN: f32 = 0.975;

/// A block with no sample above this is silence.
const QUIET: f32 = 1e-6;

/// The parameters whose move makes FL set the lookahead up anew, and the
/// ones whose move makes it set the releases up anew.
const LOOKAHEAD_WHEELS: [usize; 4] = [3, 4, 7, 14];
const RELEASE_WHEELS: [usize; 6] = [5, 6, 11, 12, 13, 17];

/// What a gain wheel comes to: `(7^x - 1) / 6`, and 1 at 1000 without
/// working it out, as in FL.
pub(crate) fn gain(value: i32) -> f32 {
    if value == 1000 {
        return 1.0;
    }
    ((7f64.powf(f64::from(value) / 1000.0) - 1.0) / 6.0) as f32
}

/// What a ceiling wheel comes to: `(3^x - 1) / 2`.
fn ceiling(value: i32) -> f32 {
    if value == 1000 {
        return 1.0;
    }
    ((3f64.powf(f64::from(value) / 1000.0) - 1.0) / 2.0) as f32
}

/// What a time wheel comes to in ms: `(501^x - 1) * 2`, a second at most.
pub fn time_ms(value: i32) -> f64 {
    (501f64.powf(f64::from(value) / 10_000.0) - 1.0) * 2.0
}

/// A time wheel in frames.
pub(crate) fn frames(value: i32, hz: f64) -> usize {
    (time_ms(value) * 0.001 * hz).round_ties_even() as usize
}

/// What a one-pole stage keeps of its way to its input every frame, for a
/// hundredth of the way to be left after `frames`. Nothing for no time.
pub(crate) fn coef(frames: f64) -> f64 {
    if frames > 0.0 {
        0.01f64.powf(1.0 / frames.max(1.0))
    } else {
        0.0
    }
}

/// What the parameters, the state's flag and the rate come to.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Tune {
    gain: f32,
    /// The ceiling, and the part of it the gain aims at.
    ceiling: f32,
    aim: f32,
    noise_gain: f32,
    /// A peak under this is the gate's.
    gate: f32,
    /// What the gate's gain keeps of its way to the noise gain.
    gate_keep: f32,
    /// The soft knee, when it is on: where it starts, how far it is from
    /// there to its end, and one over that.
    knee_out: Option<[f32; 3]>,
    /// The compressor: whether it is on, its threshold as a level and in
    /// dB, and the slope `1 / ratio`.
    comp: bool,
    threshold: f32,
    threshold_db: f64,
    slope: f64,
    /// The knee wheel's sign and the `K` it comes to.
    knee_sign: i32,
    knee: f64,
    /// How many frames the sound is held back by, and how long the two
    /// detectors' windows are.
    lookahead: usize,
    window: usize,
    rms_window: usize,
    /// How many stages the limiter's attack, its release and the
    /// compressor have, and what each stage keeps.
    attack_stages: usize,
    attack_keep: f64,
    release_stages: usize,
    release_keep: f64,
    comp_stages: usize,
    comp_up: f64,
    comp_down: f64,
    /// How many frames of silence put the plugin to sleep.
    hold: i64,
}

impl Tune {
    fn new(params: &[i32; 18], whole_ceiling: bool, hz: f64) -> Self {
        let p = |index: usize| params[index];
        let stages = |index: usize| p(index) as usize;
        let root = |index: usize| f64::from(p(index)).sqrt();
        let ceil = ceiling(p(2));
        let aim = if whole_ceiling { ceil } else { ceil * MARGIN };

        let knee_start = ceiling(p(1));
        let knee_out = (knee_start < 1.0).then(|| {
            let span = (ceil.min(1.0) - knee_start).max(1.5e-5);
            [knee_start, span, 1.0 / span]
        });

        let comp = p(8) < 1000;
        let threshold = if comp { gain(p(8)).max(0.001) } else { 100.0 };
        let size = f64::from(p(9).abs()) / 1000.0;
        let slope = if p(9) <= 0 {
            (51f64.powf(size) - 1.0) * 0.06 + 1.0
        } else {
            1.0 / ((51f64.powf(size) - 1.0) * 0.38 + 1.0)
        };
        let knee_scale = if p(10) > 0 { 0.005 } else { 0.001 };
        let knee = (201f64.powf(f64::from(p(10).abs()) / 1000.0) - 1.0) * knee_scale;

        let lookahead = frames(p(3), hz);
        let release = frames(p(5), hz);
        Tune {
            gain: gain(p(0)),
            ceiling: ceil,
            aim,
            noise_gain: gain(p(15)),
            gate: aim.min(gain(p(16))),
            gate_keep: 0.01f64.powf(1.0 / (2.0 * frames(p(17), hz).max(1) as f64)) as f32,
            knee_out,
            comp,
            threshold,
            threshold_db: 20.0 * f64::from(threshold).log10(),
            slope,
            knee_sign: p(10).signum(),
            knee,
            lookahead,
            window: lookahead.max(frames(p(7), hz)).max(1),
            rms_window: frames(p(14), hz).max(1),
            attack_stages: if lookahead < 1 { 0 } else { stages(4) },
            attack_keep: coef(0.8 * lookahead as f64 / root(4)),
            release_stages: if release == 0 { 0 } else { stages(6) },
            release_keep: coef(2.0 * release as f64 / root(6)),
            comp_stages: stages(13),
            comp_up: coef(frames(p(11), hz) as f64 / root(13)),
            comp_down: coef(2.0 * frames(p(12), hz) as f64 / root(13)),
            hold: lookahead as i64 + (1.5 * hz).round_ties_even() as i64,
        }
    }

    /// The compressor's gain for a level over its threshold.
    ///
    /// The level counts for no more than 40 dB over. Without a knee the
    /// gain in dB is what the level is over by, times `slope - 1`. A knee
    /// bends what the level is over by first: on a log for a wheel above
    /// 0 and on an exponential below.
    fn squeeze(&self, level: f32) -> f32 {
        let capped = level.min(self.threshold * 100.0);
        let over = 20.0 * f64::from(capped).log10() - self.threshold_db;
        let db = if self.knee_sign == 0 {
            over * (self.slope - 1.0)
        } else {
            let u = over + 1.0;
            let bent = if self.knee_sign > 0 {
                (1.0 + self.knee * u).ln() / (1.0 + self.knee).ln()
            } else {
                (((1.0 + self.knee).ln() * u).exp() - 1.0) / self.knee
            };
            self.slope * (bent - 1.0) - over
        };
        (db * std::f64::consts::LN_10 / 20.0).exp() as f32
    }

    /// The output stage for one sample: the soft knee when it is on, and
    /// a hard clip at the ceiling when it is not.
    fn clip(&self, sample: f32) -> f32 {
        let size = sample.abs();
        match self.knee_out {
            Some([start, span, inverse]) if size > start => {
                let bent = start + span * (1.0 - f64::from(-(size - start) * inverse).exp() as f32);
                bent.copysign(sample)
            }
            None if size > self.ceiling => self.ceiling.copysign(sample),
            _ => sample,
        }
    }
}

/// The highest of the last `window` levels, with a ring of them and a queue
/// of the ones that can still become the highest.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Highest {
    pub(crate) levels: Vec<f32>,
    /// Places in `levels`, the highest level's first and each later one's
    /// lower than the one before. A ring as well: `first` and `count`.
    queue: Vec<u32>,
    first: usize,
    count: usize,
    at: usize,
}

impl Highest {
    /// Have room for a window of `longest` levels.
    pub(crate) fn make_room(&mut self, longest: usize) {
        self.levels
            .reserve_exact(longest.saturating_sub(self.levels.len()));
        self.queue
            .reserve_exact(longest.saturating_sub(self.queue.len()));
    }

    /// Start over with a window of `window` levels, which is not 0.
    pub(crate) fn start(&mut self, window: usize) {
        self.levels.clear();
        self.levels.resize(window, 0.0);
        self.queue.clear();
        self.queue.resize(window, 0);
        (self.first, self.count, self.at) = (0, 0, 0);
    }

    /// Take one level, which is not under 0, and give the highest of the
    /// window that ends with it.
    pub(crate) fn push(&mut self, level: f32) -> f32 {
        let window = self.levels.len();
        // The level this one takes the place of has left the window.
        if self.count > 0 && self.queue[self.first] as usize == self.at {
            self.first = (self.first + 1) % window;
            self.count -= 1;
        }
        // Nothing that came before and is no higher can be the highest
        // again.
        while self.count > 0 {
            let last = (self.first + self.count - 1) % window;
            if self.levels[self.queue[last] as usize] > level {
                break;
            }
            self.count -= 1;
        }
        self.queue[(self.first + self.count) % window] = self.at as u32;
        self.count += 1;
        self.levels[self.at] = level;
        self.at = (self.at + 1) % window;
        self.levels[self.queue[self.first] as usize]
    }
}

/// The mean of the last `window` values, with a ring of them and their sum.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Mean {
    pub(crate) values: Vec<f32>,
    sum: f64,
    at: usize,
}

impl Mean {
    pub(crate) fn make_room(&mut self, longest: usize) {
        self.values
            .reserve_exact(longest.saturating_sub(self.values.len()));
    }

    /// Start over with a window of `window` values, which is not 0.
    pub(crate) fn start(&mut self, window: usize) {
        self.values.clear();
        self.values.resize(window, 0.0);
        (self.sum, self.at) = (0.0, 0);
    }

    pub(crate) fn push(&mut self, value: f32) -> f64 {
        let place = &mut self.values[self.at];
        self.sum += f64::from(value) - f64::from(*place);
        *place = value;
        self.at = (self.at + 1) % self.values.len();
        self.sum / self.values.len() as f64
    }
}

/// Fruity Limiter. A new one has FL's defaults: a limiter at 0 dBFS that
/// looks 2 ms ahead, with the compressor and the gate off.
#[derive(Debug, Clone, PartialEq)]
pub struct Limiter {
    params: [i32; 18],
    /// The state's flag: the gain aims at the whole ceiling.
    whole_ceiling: bool,
    /// The sidechain input the state chose, 0 for none, and the block of
    /// it for the next [`Effect::process`].
    sidechain: usize,
    side: Sidechain,
    hz: f64,
    tune: Tune,
    peak: Highest,
    rms: Mean,
    /// The frames the lookahead holds back, in a ring.
    held: Vec<[f32; 2]>,
    at: usize,
    /// What the stages of the limiter's attack, of its release and of the
    /// compressor are at.
    attack: [f64; STAGES],
    release: [f64; STAGES],
    comp: [f64; STAGES],
    /// The gain of the frame before, for the gate to glide from.
    last_gain: f32,
    /// How many frames of silence are left before the plugin sleeps, and
    /// whether it does.
    countdown: i64,
    asleep: bool,
}

impl Default for Limiter {
    fn default() -> Self {
        let hz = 44_100.0;
        let mut plugin = Limiter {
            params: DEFAULTS,
            whole_ceiling: false,
            sidechain: 0,
            side: Sidechain::default(),
            hz,
            tune: Tune::new(&DEFAULTS, false, hz),
            peak: Highest::default(),
            rms: Mean::default(),
            held: Vec::new(),
            at: 0,
            attack: [1.0; STAGES],
            release: [0.0; STAGES],
            comp: [0.0; STAGES],
            last_gain: 1.0,
            countdown: 0,
            asleep: false,
        };
        plugin.make_room();
        plugin.reset();
        plugin
    }
}

impl Limiter {
    /// Have the memory for the longest time a wheel can ask for at this
    /// rate, a second, so that the wheels do not have to ask for it.
    fn make_room(&mut self) {
        let longest = frames(10_000, self.hz).max(1);
        self.peak.make_room(longest);
        self.rms.make_room(longest);
        self.held
            .reserve_exact(longest.saturating_sub(self.held.len()));
    }

    /// Take parameters that are inside their ranges.
    ///
    /// As in FL, a moved attack time, attack curve or window sets the
    /// lookahead up anew, and that or a moved release sets the releases up
    /// anew. A detector or a delay of another length starts from nothing.
    fn set(&mut self, params: [i32; 18]) {
        let moved = |wheels: &[usize]| wheels.iter().any(|w| self.params[*w] != params[*w]);
        let (lookahead, release) = (moved(&LOOKAHEAD_WHEELS), moved(&RELEASE_WHEELS));
        self.params = params;
        self.tune = Tune::new(&params, self.whole_ceiling, self.hz);
        if lookahead {
            self.lookahead_anew();
        }
        if lookahead || release {
            self.releases_anew();
        }
    }

    fn lookahead_anew(&mut self) {
        if self.held.len() != self.tune.lookahead {
            self.held.clear();
            self.held.resize(self.tune.lookahead, [0.0; 2]);
            self.at = 0;
        }
        if self.peak.levels.len() != self.tune.window {
            self.peak.start(self.tune.window);
        }
        if self.rms.values.len() != self.tune.rms_window {
            self.rms.start(self.tune.rms_window);
        }
        self.attack = [1.0; STAGES];
    }

    fn releases_anew(&mut self) {
        self.release = [0.0; STAGES];
        self.comp = [0.0; STAGES];
        self.last_gain = 1.0;
    }

    /// One frame in, and the frame from the lookahead before out. The
    /// compressor's level is that of `detect`, the sidechain's frame, when
    /// there is one, and else the frame's own.
    fn frame(&mut self, left: f32, right: f32, detect: Option<[f32; 2]>) -> [f32; 2] {
        let tune = self.tune;
        let peak = self.peak.push(left.abs().max(right.abs()));

        // The compressor's gain, with the input gain in it.
        let mut gain = tune.gain;
        if tune.comp {
            let [heard_left, heard_right] = detect.unwrap_or([left, right]);
            let mean = self
                .rms
                .push(heard_left * heard_left + heard_right * heard_right);
            let mut level = (mean * 0.5).max(0.0).sqrt();
            for stage in &mut self.comp[..tune.comp_stages] {
                let keep = if *stage <= level {
                    tune.comp_up
                } else {
                    tune.comp_down
                };
                *stage = level + keep * (*stage - level);
                level = *stage;
            }
            let level = level as f32;
            if level > tune.threshold {
                gain *= tune.squeeze(level);
            }
        }

        let mut out = [left, right];
        if !self.held.is_empty() {
            std::mem::swap(&mut out, &mut self.held[self.at]);
            self.at = (self.at + 1) % self.held.len();
        }

        // The peak rises at once and falls through the release's stages.
        let mut envelope = f64::from(peak);
        for stage in &mut self.release[..tune.release_stages] {
            if *stage > envelope {
                *stage = envelope + tune.release_keep * (*stage - envelope);
            } else {
                *stage = envelope;
            }
            envelope = *stage;
        }
        let peak = envelope as f32;

        if peak * gain > tune.aim {
            gain = tune.aim / peak;
        }
        if peak < tune.gate {
            gain = tune.noise_gain + (self.last_gain - tune.noise_gain) * tune.gate_keep;
        }
        self.last_gain = gain;

        let mut smooth = f64::from(gain);
        for stage in &mut self.attack[..tune.attack_stages] {
            *stage = smooth + tune.attack_keep * (*stage - smooth);
            smooth = *stage;
        }
        let gain = smooth as f32;
        out.map(|sample| tune.clip(sample * gain))
    }

    /// What FL does when it flushes the plugin: the attack's stages at 1,
    /// the others at 0, the gate's gain at 1 and nothing held back.
    fn flush(&mut self) {
        self.attack = [1.0; STAGES];
        self.releases_anew();
        self.held.clear();
        self.held.resize(self.tune.lookahead, [0.0; 2]);
        self.at = 0;
        self.peak.start(self.tune.window);
        self.rms.start(self.tune.rms_window);
    }
}

impl Effect for Limiter {
    /// FL reads nothing of a version under 5 and keeps the parameters it
    /// has, and so does this. It reads a version above 7 as it reads 7.
    /// Any version puts the sidechain input back at none first.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let version = state_version(bytes, V7_LEN)?;
        if version >= 5 {
            let expected = match version {
                5 => V5_LEN,
                6 => V6_LEN,
                _ => V7_LEN,
            };
            if bytes.len() != expected {
                return Err(StateError::Length {
                    expected,
                    found: bytes.len(),
                });
            }
            self.params = read_state(&bytes[4..PARAMS_END], &RANGES)?;
            // Only version 7 has the flag, and an older one leaves it.
            if let Some(flags) = bytes.get(FLAG_AT) {
                self.whole_ceiling = flags & 1 == 1;
            }
        }
        let word = bytes
            .get(SIDECHAIN_AT..)
            .and_then(|rest| rest.first_chunk());
        let word = word.filter(|_| version >= 6);
        self.sidechain = word.map_or(0, |word| i32::from_le_bytes(*word)).max(0) as usize;
        self.reset();
        Ok(())
    }

    /// The times are worked out for the rate, and the plugin starts from
    /// nothing. Asks for memory when the rate is higher than before. A
    /// rate that is not a number above 0 is not taken.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz > 0.0 {
            self.hz = f64::from(hz);
            self.make_room();
            self.reset();
        }
    }

    /// A block of silence counts down, and after the lookahead and a
    /// second and a half of them the plugin sleeps: it forgets what it
    /// held and passes its input on until a block has sound again. FL
    /// counts in blocks as well, so where the sleep starts depends on how
    /// long the blocks are.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        if len == 0 {
            return;
        }
        let (left, right) = (&mut left[..len], &mut right[..len]);
        let quiet = left.iter().chain(right.iter()).all(|v| v.abs() <= QUIET);
        self.countdown = if quiet {
            self.countdown.saturating_sub(len as i64)
        } else {
            self.tune.hold
        };
        if self.countdown <= -(len as i64) {
            if !self.asleep {
                self.flush();
                self.asleep = true;
            }
            self.side.clear();
            return;
        }
        self.asleep = false;
        for (at, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
            let detect = self.side.frame(at);
            [*l, *r] = self.frame(*l, *r, detect);
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

    /// The lookahead: the limiter's attack time in frames.
    fn latency(&self) -> usize {
        self.tune.lookahead
    }

    /// The attack wheel's end, a second.
    fn max_latency(&self) -> usize {
        frames(RANGES[3].1, self.hz)
    }

    fn reset(&mut self) {
        self.tune = Tune::new(&self.params, self.whole_ceiling, self.hz);
        self.flush();
        self.side.clear();
        self.countdown = self.tune.hold;
        self.asleep = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{presets, state};

    const HZ: f64 = 48_000.0;

    /// A version 7 state with these parameters.
    fn v7(params: [i32; 18], sidechain: i32, flag: u8) -> Vec<u8> {
        let mut bytes = state(&[7]);
        bytes.extend(state(&params));
        // The A/B copy, the display's speed and tab, its four curves, its
        // off switch and the padding.
        bytes.extend(state(&DEFAULTS));
        bytes.extend(state(&[50, 0]));
        bytes.extend([1, 1, 1, 1, 0, 0, 0, 0]);
        bytes.extend(state(&[sidechain]));
        bytes.push(flag);
        bytes
    }

    fn v6(params: [i32; 18], sidechain: i32) -> Vec<u8> {
        let mut bytes = v7(params, sidechain, 0);
        bytes.truncate(V6_LEN);
        bytes[0] = 6;
        bytes
    }

    fn v5(params: [i32; 18]) -> Vec<u8> {
        let mut bytes = v7(params, 0, 0);
        bytes.truncate(V5_LEN);
        bytes[0] = 5;
        bytes
    }

    /// The plugin at 48 kHz with this state.
    fn at_48k(bytes: &[u8]) -> Limiter {
        let mut plugin = Limiter::default();
        plugin.set_sample_rate(HZ as f32);
        plugin.set_state(bytes).unwrap();
        plugin
    }

    fn with(params: [i32; 18]) -> Limiter {
        at_48k(&v7(params, 0, 0))
    }

    /// The defaults with some parameters put elsewhere.
    fn moved(moves: &[(usize, i32)]) -> [i32; 18] {
        let mut params = DEFAULTS;
        for (index, value) in moves {
            params[*index] = *value;
        }
        params
    }

    /// Both sides get `input`, in blocks of 256, and the left comes back.
    fn run(plugin: &mut Limiter, input: &[f32]) -> Vec<f32> {
        let (mut left, mut right) = (input.to_vec(), input.to_vec());
        for (l, r) in left.chunks_mut(256).zip(right.chunks_mut(256)) {
            plugin.process(l, r);
        }
        assert!(left == right);
        left
    }

    /// `len` samples that are all `size` big, two up and two down, so that
    /// the peak and the RMS are both `size` from the first sample on.
    fn steady(size: f32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|n| if n % 4 < 2 { size } else { -size })
            .collect()
    }

    fn db(gain: f64) -> f64 {
        20.0 * gain.log10()
    }

    /// The doc's time curve in frames at 48 kHz.
    fn doc_frames(value: i32) -> f64 {
        (501f64.powf(f64::from(value) / 10_000.0) - 1.0) * 2.0 * HZ / 1000.0
    }

    #[test]
    fn the_curves_are_the_doc_s() {
        // 1115 is 2 ms, 2882 is 10, 5000 is 42.8, 6666 is 124 and the
        // wheel's end is a second.
        for (value, ms) in [(1115, 2.0), (2882, 10.0), (5000, 42.8), (6666, 124.0)] {
            assert!((time_ms(value) - ms).abs() < 0.005 * ms, "{value}");
        }
        assert_eq!((time_ms(0), time_ms(10_000)), (0.0, 1000.0));
        assert_eq!(frames(1115, HZ), 96);
        assert_eq!(frames(10_000, 44_100.0), 44_100);
        // Gains: 7^x and 3^x, at 1 in the middle without working it out.
        assert_eq!(
            (gain(0), gain(1000), ceiling(0), ceiling(1000)),
            (0.0, 1.0, 0.0, 1.0)
        );
        assert!((gain(2000) - 8.0).abs() < 1e-6 && (ceiling(2000) - 4.0).abs() < 1e-6);
        assert!((f64::from(gain(500)) - (7f64.sqrt() - 1.0) / 6.0).abs() < 1e-7);
        assert!((f64::from(ceiling(500)) - (3f64.sqrt() - 1.0) / 2.0).abs() < 1e-7);
        // A stage has a hundredth of its way left after its time.
        assert!((coef(200.0).powi(200) - 0.01).abs() < 1e-12);
        assert_eq!((coef(0.0), coef(0.5)), (0.0, 0.01));
    }

    #[test]
    fn a_curve_wheel_is_that_many_stages_at_the_time_over_its_root() {
        let tune = with(moved(&[(4, 4), (6, 2), (13, 3), (11, 1115), (12, 5000)])).tune;
        let (attack, release) = (doc_frames(1115).round(), doc_frames(5000).round());
        assert_eq!((attack, tune.lookahead), (96.0, 96));
        assert_eq!(tune.attack_stages, 4);
        assert_eq!(tune.attack_keep, 0.01f64.powf(1.0 / (0.8 * attack / 2.0)));
        assert_eq!(tune.release_stages, 2);
        assert_eq!(
            tune.release_keep,
            0.01f64.powf(1.0 / (2.0 * release / 2f64.sqrt()))
        );
        assert_eq!(tune.comp_stages, 3);
        assert_eq!(tune.comp_up, 0.01f64.powf(1.0 / (attack / 3f64.sqrt())));
        assert_eq!(
            tune.comp_down,
            0.01f64.powf(1.0 / (2.0 * release / 3f64.sqrt()))
        );
        // The window is the longer of the attack and the window wheel.
        assert_eq!(tune.window, doc_frames(2882).round() as usize);
        assert_eq!(with(moved(&[(7, 0)])).tune.window, 96);
        // No attack is no delay and no stages, and no release no stages.
        let instant = with(moved(&[(3, 0), (5, 0), (7, 0)])).tune;
        assert_eq!(
            (
                instant.lookahead,
                instant.attack_stages,
                instant.release_stages,
                instant.window
            ),
            (0, 0, 0, 1)
        );
        // A second and a half of silence after the lookahead.
        assert_eq!(tune.hold, 96 + 72_000);
    }

    #[test]
    fn the_default_holds_a_burst_at_the_ceiling_after_the_lookahead() {
        // 20 ms at -12 dB, then 100 ms 6 dB over full scale.
        let burst = 10f32.powf(6.0 / 20.0);
        let mut input = steady(0.25, 960);
        input.extend(steady(burst, 4800));
        for (flag, aim) in [(0, 0.975), (1, 1.0)] {
            let mut plugin = at_48k(&v7(DEFAULTS, 0, flag));
            let lookahead = plugin.latency();
            assert_eq!(lookahead, 96);
            let out = run(&mut plugin, &input);
            // What is under the ceiling comes out as it went in, 2 ms
            // late: the gain is still 1 until the burst is in the window.
            assert!(out[..lookahead].iter().all(|v| *v == 0.0));
            assert!(out[lookahead..960] == input[..960 - lookahead]);
            // Nothing is ever over the ceiling.
            assert!(out.iter().all(|v| v.abs() <= 1.0));
            // From where the burst comes out, it is at the ceiling the
            // gain aims at, to 0.05 dB: 0.975 of the wheel's, or all of it
            // with the state's flag.
            let held = &out[960 + lookahead..];
            let (low, high) = held.iter().fold((f64::MAX, 0f64), |(low, high), v| {
                let size = f64::from(v.abs());
                (low.min(size), high.max(size))
            });
            assert!(db(high / aim) < 0.05, "flag {flag}: {}", db(high / aim));
            assert!(db(low / aim) > -0.05, "flag {flag}: {}", db(low / aim));
            // The gain is the aim over the peak, exactly so once the
            // attack's stages have settled.
            let last = f64::from(out[out.len() - 1].abs());
            assert!((last - aim).abs() < 1e-6, "flag {flag}: {last}");
        }
    }

    #[test]
    fn the_attack_and_the_release_take_the_time_the_curve_gives() {
        for wheel in [1115, 3034, 5000] {
            let wanted = doc_frames(wheel);

            // The attack, with one stage and no release: 0.5 for a while,
            // then 4. The sound is `lookahead` late, so while the gain
            // falls it is still the 0.5 that comes out, and the gain can
            // be read off it. One stage has a hundredth of its way left
            // after 0.8 of the time, and so a tenth after 0.4 of it.
            let mut plugin = with(moved(&[(3, wheel), (4, 1), (5, 0), (7, 0)]));
            let lookahead = plugin.latency();
            let mut input = steady(0.5, 2 * lookahead);
            input.extend(steady(4.0, lookahead));
            let out = run(&mut plugin, &input);
            let target = 0.975 / 4.0;
            let left = |n: usize| (f64::from(out[n].abs()) / 0.5 - target) / (1.0 - target);
            assert!(left(2 * lookahead - 1) == 1.0);
            let tenth = (0..lookahead)
                .find(|k| left(2 * lookahead + k) <= 0.1)
                .unwrap();
            let measured = (tenth + 1) as f64 / 0.4;
            assert!(
                (measured / wanted - 1.0).abs() < 0.05,
                "attack {wheel}: {measured} and not {wanted}"
            );

            // The release, with one stage and no attack: 4 for a while,
            // then 0.5. The gain is 0.975 over the envelope while that is
            // over 0.975, so the envelope can be read off the output. One
            // stage has a hundredth of its way left after twice the time,
            // and so a half after log(0.5) / log(0.01) of twice the time.
            let mut plugin = with(moved(&[(3, 0), (5, wheel), (6, 1), (7, 0)]));
            assert_eq!(plugin.latency(), 0);
            let mut input = steady(4.0, 64);
            input.extend(steady(0.5, 2 * wanted as usize));
            let out = run(&mut plugin, &input);
            assert!((f64::from(out[63].abs()) - 0.975).abs() < 1e-6);
            let left = |n: usize| (0.975 * 0.5 / f64::from(out[n].abs()) - 0.5) / 3.5;
            let half = (0..).find(|k| left(64 + k) <= 0.5).unwrap();
            let measured = (half + 1) as f64 / (2.0 * 0.5f64.ln() / 0.01f64.ln());
            assert!(
                (measured / wanted - 1.0).abs() < 0.05,
                "release {wheel}: {measured} and not {wanted}"
            );
        }
    }

    /// What a steady level comes out as once the compressor has settled,
    /// over what went in.
    fn settled(params: [i32; 18], size: f32) -> f64 {
        let mut plugin = with(params);
        let out = run(&mut plugin, &steady(size, 48_000));
        f64::from(out[out.len() - 1].abs()) / f64::from(size)
    }

    #[test]
    fn the_compressor_settles_on_the_doc_s_gain() {
        // The threshold at (sqrt(7) - 1) / 6, -11.2 dB.
        let threshold = (7f64.sqrt() - 1.0) / 6.0;
        let over = |size: f32| db(f64::from(size) / threshold);
        // The ratio wheel at 500 is 3.33 : 1, at 1000 it is 20 : 1, and
        // under 0 the gain goes up: at -500 by 0.37 dB a dB.
        let slopes = [
            (500, 1.0 / ((51f64.sqrt() - 1.0) * 0.38 + 1.0)),
            (1000, 1.0 / 20.0),
            (-500, (51f64.sqrt() - 1.0) * 0.06 + 1.0),
            (0, 1.0),
        ];
        assert!((1.0 / slopes[0].1 - 3.33).abs() < 0.01);
        for (ratio, slope) in slopes {
            for size in [0.4f32, 0.6] {
                let got = settled(moved(&[(8, 500), (9, ratio)]), size);
                let want = over(size) * (slope - 1.0);
                assert!(
                    (db(got) - want).abs() < 0.001,
                    "ratio {ratio} at {size}: {} and not {want}",
                    db(got)
                );
            }
            // Under the threshold nothing happens.
            assert_eq!(settled(moved(&[(8, 500), (9, ratio)]), 0.2), 1.0);
        }
        // The input gain is in the compressor's gain, and the level the
        // compressor reads is the input's without it.
        let got = settled(moved(&[(0, 500), (8, 500), (9, 500)]), 0.6);
        let want = over(0.6) * (slopes[0].1 - 1.0) + db(threshold);
        assert!((db(got) - want).abs() < 0.001);
        // The level counts for no more than 40 dB over the threshold: at
        // a threshold of 0.001, 0.5 is 54 dB over.
        let got = settled(moved(&[(8, 0), (9, 500), (0, 2000)]), 0.5);
        let want = 40.0 * (slopes[0].1 - 1.0) + db(8.0);
        assert!((db(got) - want).abs() < 0.001, "{}", db(got));
        // With the wheel at its end the compressor is off.
        assert!(!with(DEFAULTS).tune.comp && with(moved(&[(8, 999)])).tune.comp);
    }

    #[test]
    fn a_sidechain_input_is_the_level_the_compressor_reads() {
        // The threshold at -11.2 dB and 20 : 1, listening to input 2.
        let threshold = (7f64.sqrt() - 1.0) / 6.0;
        let squash = moved(&[(8, 500), (9, 1000)]);
        let quiet = steady(0.2, 48_000);
        let ducked = |plugin: &mut Limiter, index: usize, size: f32| -> f64 {
            let loud = steady(size, 256);
            let (mut left, mut right) = (quiet.clone(), quiet.clone());
            for (l, r) in left.chunks_mut(256).zip(right.chunks_mut(256)) {
                plugin.set_sidechain(index, &loud[..l.len()], &loud[..l.len()]);
                plugin.process(l, r);
            }
            f64::from(left[left.len() - 1].abs()) / f64::from(0.2f32)
        };
        // The plugin's own sound is under the threshold and the sidechain
        // over it, and the gain is what the sidechain's level asks for.
        let mut plugin = at_48k(&v7(squash, 2, 0));
        let got = ducked(&mut plugin, 2, 0.6);
        let want = db(0.6 / threshold) * (1.0 / 20.0 - 1.0);
        assert!((db(got) - want).abs() < 0.001, "{}", db(got));
        // Once no block of it comes, the compressor reads its own input,
        // as it does all along for another input than the state's.
        let out = run(&mut plugin, &quiet);
        assert_eq!(out[out.len() - 1].abs(), 0.2);
        assert_eq!(ducked(&mut at_48k(&v7(squash, 2, 0)), 1, 0.6), 1.0);
        assert_eq!(ducked(&mut at_48k(&v7(squash, 0, 0)), 0, 0.6), 1.0);
        // The limiter's peak is the plugin's own: a sidechain over the
        // ceiling takes nothing off with the compressor off.
        let mut plain = at_48k(&v7(DEFAULTS, 2, 0));
        assert_eq!(ducked(&mut plain, 2, 4.0), 1.0);
        // Asleep, the plugin drops the block it was handed.
        let mut asleep = at_48k(&v7(squash, 2, 0));
        run(&mut asleep, &vec![0.0; 96_000]);
        assert!(asleep.asleep);
        asleep.set_sidechain(2, &[1.0; 256], &[1.0; 256]);
        asleep.process(&mut [0.0; 256], &mut [0.0; 256]);
        assert!(asleep.side.left.is_empty() && asleep.side.left.capacity() >= 4096);
    }

    #[test]
    fn the_knee_bends_what_the_level_is_over_by() {
        let threshold = (7f64.sqrt() - 1.0) / 6.0;
        let slope = 1.0 / ((51f64.sqrt() - 1.0) * 0.38 + 1.0);
        for size in [0.3f32, 0.6, 0.9] {
            let over = db(f64::from(size) / threshold);
            let u = over + 1.0;
            // Above 0: K = (201^x - 1) * 0.005, on a log.
            let k = (201f64.powf(0.644) - 1.0) * 0.005;
            let want = slope * ((1.0 + k * u).ln() / (1.0 + k).ln() - 1.0) - over;
            let got = settled(moved(&[(8, 500), (9, 500), (10, 644)]), size);
            assert!((db(got) - want).abs() < 0.001, "knee 644 at {size}");
            // Under 0: K = (201^x - 1) * 0.001, on an exponential.
            let k = (201f64.powf(0.776) - 1.0) * 0.001;
            let want = slope * (((1.0 + k).powf(u) - 1.0) / k - 1.0) - over;
            let got = settled(moved(&[(8, 500), (9, 500), (10, -776)]), size);
            assert!((db(got) - want).abs() < 0.001, "knee -776 at {size}");
        }
        // Either knee leaves the level at the threshold alone.
        let tune = with(moved(&[(8, 500), (9, 500), (10, 1000)])).tune;
        assert!((tune.squeeze(tune.threshold) - 1.0).abs() < 1e-6);
        let tune = with(moved(&[(8, 500), (9, 500), (10, -1000)])).tune;
        assert!((tune.squeeze(tune.threshold) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn the_gate_silences_what_is_under_its_threshold() {
        // The noise gain at nothing and the threshold at (sqrt(7) - 1) / 6.
        // No attack, no release and a window of one frame, so that the
        // peak is the frame's own and the gain can be read off the output.
        let threshold = ((7f64.sqrt() - 1.0) / 6.0) as f32;
        let gate = moved(&[(3, 0), (5, 0), (7, 0), (15, 0), (16, 500), (17, 3034)]);
        let mut plugin = with(gate);
        assert_eq!(plugin.tune.gate, threshold);
        // Over the threshold the sound passes as it is.
        let loud = steady(threshold + 0.01, 4800);
        assert!(run(&mut plugin, &loud) == loud);
        // Under it the gain glides to the noise gain: a hundredth of the
        // way is left after twice the release time.
        let release = doc_frames(3034).round() as usize;
        let quiet = steady(threshold - 0.01, 6 * release);
        let out = run(&mut plugin, &quiet);
        let passed = |n: usize| f64::from(out[n] / quiet[n]);
        assert!((passed(2 * release - 1) - 0.01).abs() < 1e-4);
        assert!((passed(release - 1) - 0.1).abs() < 1e-3);
        assert!(db(passed(6 * release - 1)) < -100.0);
        // Back over the threshold the gain is back at once.
        assert!(run(&mut plugin, &loud) == loud);

        // A noise gain of 1 takes the input gain off under the threshold:
        // the doc's "Max loudness except noise".
        let quieter = [(0, 1654), (3, 0), (5, 0), (7, 0), (16, 108), (17, 3034)];
        let mut plugin = with(moved(&quieter));
        let boost = f64::from(gain(1654));
        assert!((db(boost) - 12.0).abs() < 0.05);
        let out = run(&mut plugin, &steady(0.1, 256));
        assert!((f64::from(out[255].abs()) / 0.1 - boost).abs() < 1e-6);
        let out = run(&mut plugin, &steady(0.001, 6 * release));
        assert!((f64::from(out[0].abs()) / 0.001 - boost).abs() < 0.01 * boost);
        assert!((f64::from(out[out.len() - 1].abs()) / 0.001 - 1.0).abs() < 1e-3);
        // With the threshold wheel at 0 there is no gate.
        assert_eq!(with(DEFAULTS).tune.gate, 0.0);
    }

    #[test]
    fn the_output_is_clipped_hard_or_bent_into_the_ceiling() {
        // Without the limiter's delay and its stages, a ceiling of
        // (sqrt(3) - 1) / 2 and a first sample over it: the gain puts the
        // peak at 0.975 of the ceiling.
        let ceil = ceiling(500);
        assert!((f64::from(ceil) - (3f64.sqrt() - 1.0) / 2.0).abs() < 1e-7);
        let mut plugin = with(moved(&[(2, 500), (3, 0), (5, 0), (7, 0)]));
        let out = run(&mut plugin, &[0.9, -0.2, 0.3]);
        assert!((out[0] - ceil * 0.975).abs() < 1e-6);
        assert_eq!((out[1], out[2]), (-0.2, 0.3));
        // The hard clip is for what the attack lets through: with a
        // lookahead the gain is still on its way when a peak that started
        // the window comes out.
        let tune = with(moved(&[(2, 500)])).tune;
        assert_eq!(tune.clip(0.9), ceil);
        assert_eq!(tune.clip(-0.9), -ceil);
        assert_eq!(tune.clip(0.3), 0.3);

        // The soft knee: from the saturation threshold on, the output
        // goes to the ceiling, or to 1 for a ceiling over 1, on an
        // exponential that never gets there.
        let tune = with(moved(&[(1, 760), (2, 1180)])).tune;
        let start = (3f64.powf(0.76) - 1.0) / 2.0;
        assert!((db(start) + 3.7).abs() < 0.05 && (db(f64::from(tune.ceiling)) - 2.5).abs() < 0.05);
        let span = 1.0 - start;
        for x in [0.1, start as f32, 0.8, 1.0, 1.3, 5.0, -0.9] {
            let size = f64::from(x.abs());
            let want = if size > start {
                start + span * (1.0 - (-(size - start) / span).exp())
            } else {
                size
            };
            let got = f64::from(tune.clip(x));
            assert!((got.abs() - want).abs() < 1e-6, "{x}: {got}");
            assert!(got.abs() < 1.0 + 1e-6 && got.signum() == f64::from(x.signum()));
        }
        // With the knee on there is no hard clip, and with its wheel at
        // the end there is no knee.
        assert!(with(DEFAULTS).tune.knee_out.is_none());
        let tune = with(moved(&[(1, 500), (2, 500)])).tune;
        assert_eq!(tune.knee_out.unwrap()[1], 1.5e-5);
    }

    #[test]
    fn the_latency_is_the_attack_time_in_frames() {
        for (wheel, hz) in [
            (1115, 48_000.0),
            (0, 48_000.0),
            (7547, 44_100.0),
            (10_000, 96_000.0),
        ] {
            let mut plugin = Limiter::default();
            plugin.set_sample_rate(hz);
            plugin.set_param(3, wheel as f32);
            let want = (time_ms(wheel) * f64::from(hz) / 1000.0).round() as usize;
            assert_eq!(plugin.latency(), want);
            assert_eq!(plugin.max_latency(), hz as usize);
            // An impulse under the ceiling comes out that many frames on.
            let mut input = vec![0.0; want + 16];
            input[0] = 0.5;
            let out = run(&mut plugin, &input);
            let hits: Vec<usize> = (0..out.len()).filter(|n| out[*n] != 0.0).collect();
            assert_eq!(hits, [want], "attack {wheel}");
            assert_eq!(out[want], 0.5);
        }
        // "Ducking music behind voice" looks 216 ms ahead.
        assert!((time_ms(7547) - 216.0).abs() < 0.5);
    }

    #[test]
    fn silence_puts_the_plugin_to_sleep_and_sound_wakes_it() {
        // 12 dB of gain, so that the attack's stages sit at 4 and not at
        // the 1 a flush puts them at.
        let mut plugin = with(moved(&[(0, 1654)]));
        run(&mut plugin, &steady(0.01, 4800));
        assert!(plugin.attack[0] > 3.9 && !plugin.asleep);
        // The lookahead and a second and a half of silence, and the block
        // that counts past them.
        let hold = plugin.tune.hold as usize;
        run(&mut plugin, &vec![0.0; hold]);
        assert!(plugin.attack[0] > 3.9 && !plugin.asleep);
        run(&mut plugin, &vec![0.0; 512]);
        assert!(plugin.asleep);
        assert_eq!(plugin.attack, [1.0; STAGES]);
        // Asleep, the input passes as it is.
        let mut tiny = [1e-7, -1e-7];
        plugin.process(&mut tiny.clone(), &mut tiny);
        assert_eq!(tiny, [1e-7, -1e-7]);
        // Sound wakes it.
        run(&mut plugin, &steady(0.01, 256));
        assert!(!plugin.asleep && plugin.countdown == plugin.tune.hold);
        // An empty block changes nothing.
        let before = plugin.clone();
        plugin.process(&mut [], &mut []);
        assert_eq!(plugin, before);
    }

    #[test]
    fn a_moved_wheel_asks_for_no_memory_and_is_the_same_plugin_as_its_state() {
        let mut plugin = Limiter::default();
        plugin.set_sample_rate(48_000.0);
        let memory = |plugin: &Limiter| {
            (
                plugin.held.as_ptr(),
                plugin.peak.levels.as_ptr(),
                plugin.peak.queue.as_ptr(),
                plugin.rms.values.as_ptr(),
            )
        };
        let before = memory(&plugin);
        for wheel in [10_000.0, 0.0, 5000.0, 1115.0] {
            for index in [3, 7, 14] {
                plugin.set_param(index, wheel);
                plugin.process(&mut [0.5; 64], &mut [0.5; 64]);
            }
        }
        assert_eq!(plugin.held.len(), 96);
        assert_eq!(memory(&plugin), before);

        let moves = [
            1654.4, 760.0, 1180.0, 411.0, 2.0, 7320.0, 9.0, 1147.0, 548.0, -2000.0, 644.0, 3936.0,
            7178.0, 0.0, 650.0, 0.0, 108.0, 7424.0,
        ];
        for (index, value) in moves.into_iter().enumerate() {
            plugin.set_param(index, value);
        }
        plugin.set_param(18, 1.0);
        assert_eq!((plugin.param(18), plugin.param_count()), (0.0, 18));
        let params = [
            1654, 760, 1180, 411, 2, 7320, 8, 1147, 548, -1000, 644, 3936, 7178, 1, 650, 0, 108,
            7424,
        ];
        assert_eq!(
            std::array::from_fn::<f32, 18, _>(|i| plugin.param(i)),
            params.map(|v| v as f32)
        );
        plugin.reset();
        assert_eq!(plugin, with(params));
        // A rate that is not one leaves the plugin as it is.
        plugin.set_sample_rate(f32::NAN);
        plugin.set_sample_rate(-1.0);
        assert_eq!(plugin, with(params));
    }

    #[test]
    fn the_peak_detector_is_the_highest_of_its_window() {
        let levels: Vec<f32> = (0..500)
            .map(|n| ((n * 7919 % 1009) as f32 / 1009.0).powi(3))
            .collect();
        for window in [1, 2, 7, 64] {
            let mut highest = Highest::default();
            highest.make_room(64);
            highest.start(window);
            for (n, level) in levels.iter().enumerate() {
                let from = (n + 1).saturating_sub(window);
                let want = levels[from..=n].iter().copied().fold(0.0, f32::max);
                assert_eq!(highest.push(*level), want, "window {window} at {n}");
            }
        }
        let mut mean = Mean::default();
        mean.start(4);
        let means: Vec<f64> = [4.0, 8.0, 0.0, 4.0, 0.0].map(|v| mean.push(v)).to_vec();
        assert_eq!(means, [1.0, 3.0, 3.0, 4.0, 3.0]);
    }

    #[test]
    fn the_three_versions_load_and_others_are_turned_down() {
        let params = moved(&[(0, 1654), (8, 500), (9, 500)]);
        let newest = at_48k(&v7(params, 0, 0));
        assert_eq!(v7(params, 0, 0).len(), 169);
        assert_eq!(at_48k(&v6(params, 0)), newest);
        assert_eq!(at_48k(&v5(params)), newest);
        assert_eq!((v6(params, 0).len(), v5(params).len()), (168, 161));
        assert_eq!(Limiter::default().params, DEFAULTS);
        let mut default = Limiter::default();
        default.set_state(&v7(DEFAULTS, 0, 0)).unwrap();
        assert_eq!(default, Limiter::default());

        // The flag is version 7's, and the sidechain version 6's and 7's.
        let flagged = at_48k(&v7(params, 3, 1));
        assert!(flagged.whole_ceiling && flagged.tune.aim == 1.0);
        assert_eq!(
            (flagged.sidechain_input(), newest.sidechain_input()),
            (3, 0)
        );
        assert_eq!(at_48k(&v6(params, 2)).sidechain_input(), 2);
        assert!((newest.tune.aim, newest.tune.ceiling) == (0.975, 1.0));

        let mut plugin = flagged.clone();
        for (version, len, expected) in [(5, 168, 161), (6, 169, 168), (7, 168, 169), (9, 170, 169)]
        {
            let mut bytes = v7(params, 0, 0);
            bytes.resize(len, 0);
            bytes[0] = version;
            assert_eq!(
                plugin.set_state(&bytes),
                Err(StateError::Length {
                    expected,
                    found: len
                })
            );
        }
        assert_eq!(
            plugin.set_state(&[7, 0]),
            Err(StateError::Length {
                expected: 169,
                found: 2
            })
        );
        for (param, value) in [(0, 2001), (4, 0), (6, 9), (9, -1001), (13, 0), (17, 10_001)] {
            let bytes = v7(moved(&[(param, value)]), 0, 0);
            assert_eq!(
                plugin.set_state(&bytes),
                Err(StateError::Range { param, value })
            );
        }
        assert_eq!(plugin, flagged);
        // FL reads nothing of a version under 5: the parameters stay, and
        // the sidechain goes.
        assert_eq!(plugin.set_state(&[0; 8]), Ok(()));
        assert_eq!((plugin.params, plugin.sidechain_input()), (params, 0));
        // And it reads a later version as it reads 7.
        let mut later = v7(DEFAULTS, 0, 0);
        later[0] = 8;
        assert_eq!(plugin.set_state(&later), Ok(()));
        assert_eq!(plugin.params, DEFAULTS);
    }

    #[test]
    fn fl_s_presets_load_as_the_doc_reads_them() {
        let Some(presets) = presets(NAME) else { return };
        assert_eq!(presets.len(), 13);
        let named = |file: &str| {
            let preset = presets.iter().find(|p| p.file == file);
            let preset = preset.unwrap_or_else(|| panic!("no preset {file}"));
            at_48k(&preset.state)
        };
        for preset in &presets {
            // Every one is a version 5.
            assert_eq!(preset.state.len(), 161, "{}", preset.file);
            assert_eq!(preset.state[..4], [5, 0, 0, 0], "{}", preset.file);
            assert_eq!(preset.plugin, NAME, "{}", preset.file);
            assert_eq!(named(&preset.file).sidechain_input(), 0);
        }
        assert_eq!(named("Default").params, DEFAULTS);

        // The doc's table: gain in dB, attack and release in ms.
        let read = |file: &str| {
            let plugin = named(file);
            let p = plugin.params;
            (
                db(f64::from(plugin.tune.gain)),
                time_ms(p[3]),
                time_ms(p[5]),
                plugin,
            )
        };
        let (gain, attack, release, loud) = read("Max loudness");
        assert!((gain - 12.0).abs() < 0.05 && (attack - 2.0).abs() < 0.01);
        assert!((release - 43.0).abs() < 0.5 && (time_ms(loud.params[7]) - 2.0).abs() < 0.01);
        let (gain, _, _, bent) = read("Max loudness (slight saturation)");
        assert!((gain - 13.1).abs() < 0.05);
        assert!((db(f64::from(bent.tune.knee_out.unwrap()[0])) + 3.7).abs() < 0.05);
        assert!((db(f64::from(bent.tune.ceiling)) - 2.5).abs() < 0.05);
        let (_, _, _, gate) = read("Noise gate");
        assert_eq!(gate.tune.noise_gain, 0.0);
        assert!((db(f64::from(gate.tune.gate)) + 30.1).abs() < 0.05);
        assert!((time_ms(gate.params[17]) - 113.0).abs() < 0.5);
        let (gain, attack, release, drums) = read("Drum compression");
        assert!((gain - 14.0).abs() < 0.05 && (attack, release) == (0.0, 0.0));
        assert_eq!((drums.latency(), drums.tune.attack_stages), (0, 0));
        assert!((drums.tune.threshold_db + 22.0).abs() < 0.05);
        assert!((1.0 / drums.tune.slope - 20.0).abs() < 1e-9);
        assert_eq!((drums.params[10], drums.tune.comp_stages), (644, 2));
        let (_, attack, _, ducking) = read("Ducking music behind voice");
        assert!((attack - 216.0).abs() < 0.5);
        assert_eq!(ducking.latency(), (time_ms(7547) * 48.0).round() as usize);
        // "Drum compression 2" is 6.2 : 1.
        let second = named("Drum compression 2");
        assert!((1.0 / second.tune.slope - 6.2).abs() < 0.05);
        // The doc reads "Drum expanding" as a ratio of -776. The preset
        // has the ratio at 0 and the knee at -776, and it is the knee
        // that expands: at a slope of 1 the gain in dB is what the knee
        // bends the level's dB over the threshold by. 10 dB over, that is
        // 4 dB up.
        let expanding = named("Drum expanding");
        assert_eq!((expanding.params[9], expanding.params[10]), (0, -776));
        assert_eq!(expanding.tune.slope, 1.0);
        let k = (201f64.powf(0.776) - 1.0) * 0.001;
        let want = ((1.0 + k).powf(11.0) - 1.0) / k - 11.0;
        let level = expanding.tune.threshold * 10f32.powf(0.5);
        let got = db(f64::from(expanding.tune.squeeze(level)));
        assert!(
            (got - want).abs() < 1e-3 && (want - 4.0).abs() < 0.1,
            "{got} {want}"
        );
        // In the same way the log knee of "Transient shaping" takes gain
        // off at a ratio of 1 : 1, where the doc has it change nothing.
        let shaping = named("Transient shaping");
        assert_eq!((shaping.params[9], shaping.params[10]), (0, 1000));
        let want = (1.0 + 11.0f64).ln() / 2f64.ln() - 11.0;
        let level = shaping.tune.threshold * 10f32.powf(0.5);
        let got = db(f64::from(shaping.tune.squeeze(level)));
        assert!((got - want).abs() < 1e-3 && want < -7.0, "{got} {want}");
    }
}
