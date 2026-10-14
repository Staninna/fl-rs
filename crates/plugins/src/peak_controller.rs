//! Fruity Peak Controller: a peak follower and an LFO that drive other
//! controls, as `docs/plugins/fruity-peak-controller.md` has them.
//!
//! The state is 42 bytes: ten little-endian i32, the plugin's parameters,
//! then a byte each for Mute and Ramp.
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Peak base level | 0 to 65536, which is 1.0 | 0 |
//! | 1 Peak amount | -256 to 256, with 179 at unity | 0 |
//! | 2 Peak tension | -128 to 128 | 0 |
//! | 3 Peak decay | 5 to 128 | 66 |
//! | 4 LFO base level | 0 to 65536 | 0 |
//! | 5 LFO amount | -256 to 256 | 0 |
//! | 6 LFO tension | -128 to 128 | 0 |
//! | 7 LFO speed | 200 to 65536, with 32950 at a bar | 32950 |
//! | 8 LFO shape | 0 sine, 1 triangle, 2 square, 3 saw, 4 random | 0 |
//! | 9 LFO phase | 0 to 65535, a whole turn at 65536 | 0 |
//!
//! # Sound
//!
//! [`Effect::process`] passes its block on as it is, or silences it with
//! Mute on. On the way it lifts a peak hold to the loudest sample of the
//! block, either side, with no smoothing.
//!
//! # Ticks
//!
//! Everything else happens in [`Effect::tick`], which is FL's `NewTick`:
//!
//! 1. Peak. The hold goes through the tension curve and is scaled by the
//!    amount, `0.4 * (6^(|v| / 256) - 1)` of full scale, on top of the base
//!    level. At 179 that is 1.0001, so a sample at 0 dBFS gives 1.0.
//! 2. LFO. One of the host's tables of 16384 points, read at a phase that
//!    moves once a tick, lifted to 0..1, through its own tension curve and
//!    scaled by `0.2 * (6^(|v| / 256) - 1)`, on top of its base level. A
//!    turn takes `(1001^(speed / 65536) - 1) * 0.128` beats, [`lfo_beats`].
//!    Random reads the noise table one point a turn, so it holds a level
//!    for a turn and jumps.
//! 3. Outputs. Peak + LFO, Peak and LFO, each kept inside 0..1. One that
//!    is not what was sent last is sent: as it is, or with Ramp on no more
//!    than an eighth of full scale from what was sent last.
//! 4. Decay. The hold drops by `(1001^(decay / 128) - 1) * 0.0192` a beat,
//!    in equal parts a tick, [`decay_per_beat`].
//!
//! The LFO is locked to the song: a tick that does not follow the one
//! before it puts the phase at `tick * step + phase knob`, which is where a
//! song that played from its start would have it. So do the shape and the
//! phase knob when they are set.
//!
//! [`Effect::controller_outputs`] has the three outputs as they were last
//! sent, from 0 to 1. FL counts them in 2^30ths, and so does this inside.
//!
//! What is not FL's:
//!
//! - FL also measures the peak while its editor is open with the amount at
//!   0, for the meter it draws. Here the amount alone decides, which gives
//!   the same outputs.
//! - FL moves the phase when the shape or the phase knob is set, by the
//!   host's own count of the song's ticks. Here the next tick does it.
//! - FL locks the LFO only while the song plays. A host calls
//!   [`Effect::tick`] only while it does.

use crate::three_osc::host_table;
use crate::{Effect, StateError, knob_value, read_state};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Peak Controller";

/// The range of each parameter's wheel.
const RANGES: [(i32, i32); 10] = [
    (0, 65_536),
    (-256, 256),
    (-128, 128),
    (5, 128),
    (0, 65_536),
    (-256, 256),
    (-128, 128),
    (200, 65_536),
    (0, 4),
    (0, 65_535),
];
const DEFAULTS: [i32; 10] = [0, 0, 0, 66, 0, 0, 0, 32_950, 0, 0];

/// The bytes of a state: the ten parameters, Mute and Ramp.
const STATE: usize = 42;

/// The plugin's outputs, in the order FL numbers them.
pub const PEAK_AND_LFO: usize = 0;
pub const PEAK: usize = 1;
pub const LFO: usize = 2;

/// An output at 1.0, in the units FL sends.
const FULL: i64 = 1 << 30;
/// The most an output moves in a tick with Ramp on.
const RAMP: i64 = 1 << 27;
/// A base level's 65536 is [`FULL`].
const BASE: f64 = 16_384.0;
/// What an amount of 1 is in FL's units, for the peak and for the LFO:
/// 2^32 / 10 and 2^31 / 10 as singles.
const SCALE: [f32; 2] = [429_496_736.0, 214_748_368.0];
/// The noise among the host's tables, which the random shape reads.
const NOISE: usize = 5;
const RANDOM: i32 = 4;

/// How much the peak hold drops in a beat at a decay knob: 0.657 at the
/// default 66, 0.006 at 5 and 19.2, which is at once, at 128.
pub fn decay_per_beat(decay: i32) -> f64 {
    (1001f64.powf(f64::from(decay) / 128.0) - 1.0) * 0.0192
}

/// How many beats a turn of the LFO takes at a speed knob:
/// `(1001^(speed / 65536) - 1) * 0.128`, which is 4 at the default 32950,
/// 1 at 20643 and 128 at 65536.
pub fn lfo_beats(speed: i32) -> f64 {
    (1001f64.powf(f64::from(speed) / 65_536.0) - 1.0) * 0.128
}

/// How many ticks the phase takes to go round once. For every shape but
/// random that is a turn of the LFO, in whole ticks and no fewer than 3.
/// The random shape goes round its 16384 points of noise in as many turns,
/// one point a turn.
fn phase_ticks(speed: i32, shape: i32, ppq: u16) -> u64 {
    // FL's arithmetic, which is `lfo_beats` times a PPQ that divides by 4.
    let depth = 1001f64.powf(f64::from(speed) / 65_536.0) - 1.0;
    let ticks = depth * f64::from(ppq >> 2) / 48.0 * 24_576.0 / 1000.0;
    let mut ticks = ticks.round_ties_even() as u64;
    if shape == RANDOM {
        ticks <<= 14;
    }
    ticks.max(3)
}

/// An amount and a tension: what takes a level from 0 to 1 to its part of
/// an output.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Shaper {
    /// The amount in FL's units, divided by `tension` when there is one.
    amount: f32,
    /// FL's `T`, from 0 to 100.
    tension: f32,
    /// Which way the curve bows: 1 up, -1 down and 0 for a straight line.
    bow: i32,
}

impl Shaper {
    fn new(amount: i32, tension: i32, scale: f32) -> Self {
        let size = 6f64.powf(f64::from(amount.abs()) / 256.0) - 1.0;
        let mut amount = (f64::from(amount.signum()) * size) as f32 * scale;
        let bow = tension.signum();
        let mut curve = 0.0;
        if bow != 0 {
            curve = ((1001f64.powf(f64::from(tension.abs()) / 128.0) - 1.0) * 0.1) as f32;
            amount /= curve;
        }
        Shaper {
            amount,
            tension: curve,
            bow,
        }
    }

    /// The curve at `x`, before the amount: `x` itself without tension, and
    /// with it a curve through 0 and `tension`.
    fn curve(&self, x: f32) -> f32 {
        let base = f64::from(self.tension) + 1.0;
        match self.bow {
            0 => x,
            1.. => self.tension - (base.powf(f64::from(1.0 - x)) - 1.0) as f32,
            _ => (base.powf(f64::from(x)) - 1.0) as f32,
        }
    }

    /// What a level adds to an output, in FL's units.
    fn at(&self, x: f32) -> f64 {
        f64::from(self.curve(x) * self.amount)
    }
}

/// Fruity Peak Controller. A new one has the plugin's own defaults, with
/// both amounts at 0, so its outputs stay at 0. FL's "Default" preset, which
/// every corpus project starts from, has the peak amount at 179.
#[derive(Debug, Clone, PartialEq)]
pub struct PeakController {
    params: [i32; 10],
    mute: bool,
    ramp: bool,
    /// The song's ticks to a beat.
    ppq: u16,
    /// The loudest sample since the hold last decayed to under it.
    hold: f32,
    /// What the hold drops by in a tick.
    decay: f32,
    /// The LFO's place in its table, 2^32 to a turn of it, and how far it
    /// moves in a tick.
    phase: u32,
    step: u32,
    peak: Shaper,
    lfo: Shaper,
    /// The tick [`Effect::tick`] was last called for.
    last_tick: Option<u64>,
    /// The next tick puts the phase where the song has it.
    resync: bool,
    /// What each output was last sent as, in FL's units, and -1 for one
    /// that has to be sent whatever it is.
    sent: [i64; 3],
    /// The same from 0 to 1.
    outputs: [f32; 3],
    changed: bool,
}

impl Default for PeakController {
    fn default() -> Self {
        // The tables are made here, where asking for memory is allowed.
        host_table(0);
        let mut plugin = PeakController {
            params: DEFAULTS,
            mute: false,
            ramp: false,
            ppq: 96,
            hold: 0.0,
            decay: 0.0,
            phase: 0,
            step: 0,
            peak: Shaper::new(0, 0, SCALE[0]),
            lfo: Shaper::new(0, 0, SCALE[1]),
            last_tick: None,
            resync: true,
            sent: [-1; 3],
            outputs: [0.0; 3],
            changed: false,
        };
        plugin.update();
        plugin
    }
}

impl PeakController {
    /// Work out what the parameters and the PPQ come to.
    fn update(&mut self) {
        let [
            _,
            amount,
            tension,
            decay,
            _,
            lfo_amount,
            lfo_tension,
            speed,
            shape,
            _,
        ] = self.params;
        self.peak = Shaper::new(amount, tension, SCALE[0]);
        self.lfo = Shaper::new(lfo_amount, lfo_tension, SCALE[1]);
        self.decay = (decay_per_beat(decay) / f64::from(self.ppq.max(1))) as f32;
        let ticks = phase_ticks(speed, shape, self.ppq);
        self.step = ((1u64 << 32) / ticks) as u32;
    }

    /// Whether Mute is on: the plugin's input is not heard.
    pub fn mute(&self) -> bool {
        self.mute
    }

    /// Whether Ramp is on: an output moves an eighth of full scale a tick
    /// at most.
    pub fn ramp(&self) -> bool {
        self.ramp
    }

    /// Send output `index` when `value` is not what it was last sent as.
    fn send(&mut self, index: usize, value: i64) {
        let last = self.sent[index];
        if value == last {
            return;
        }
        let next = match self.ramp {
            true => last + (value - last).clamp(-RAMP, RAMP),
            false => value,
        };
        self.sent[index] = next;
        self.outputs[index] = (next.max(0) as f64 / FULL as f64) as f32;
        self.changed = true;
    }
}

impl Effect for PeakController {
    /// The outputs are sent again on the next tick, and the LFO is put
    /// where the song has it then.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let Some((params, [mute, ramp])) = bytes.split_last_chunk::<2>() else {
            return Err(StateError::Length {
                expected: STATE,
                found: bytes.len(),
            });
        };
        self.params = read_state(params, &RANGES).map_err(|error| match error {
            StateError::Length { .. } => StateError::Length {
                expected: STATE,
                found: bytes.len(),
            },
            other => other,
        })?;
        (self.mute, self.ramp) = (*mute != 0, *ramp != 0);
        self.update();
        self.resync = true;
        Ok(())
    }

    /// Nothing here depends on the rate: the plugin counts in ticks.
    fn set_sample_rate(&mut self, _hz: f32) {}

    fn set_ppq(&mut self, ppq: u16) {
        self.ppq = ppq;
        self.update();
    }

    fn tick(&mut self, ppq_tick: u64, _tempo: f64) {
        let follows = self.last_tick.and_then(|last| last.checked_add(1)) == Some(ppq_tick);
        if self.resync || !follows {
            let turn = (self.params[9] as u32) << 16;
            self.phase = (ppq_tick as u32).wrapping_mul(self.step).wrapping_add(turn);
            self.resync = false;
        }
        self.last_tick = Some(ppq_tick);

        let peak = f64::from(self.params[0]) * BASE + self.peak.at(self.hold);
        let peak = peak.round_ties_even() as i64;
        let shape = self.params[8];
        let table = host_table(if shape == RANDOM {
            NOISE
        } else {
            shape as usize
        });
        let level = (table[(self.phase >> 18) as usize] + 1.0) * 0.5;
        let lfo = f64::from(self.params[4]) * BASE + self.lfo.at(level);
        let lfo = lfo.round_ties_even() as i64;
        self.phase = self.phase.wrapping_add(self.step);

        self.send(PEAK_AND_LFO, (peak + lfo).clamp(0, FULL));
        self.send(PEAK, peak.clamp(0, FULL));
        self.send(LFO, lfo.clamp(0, FULL));
        self.hold = (self.hold - self.decay).max(0.0);
    }

    fn controller_outputs(&self) -> &[f32] {
        &self.outputs
    }

    fn controller_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if self.params[1] != 0 {
            let loudest = |hold: f32, side: &[f32]| side.iter().fold(hold, |m, v| m.max(v.abs()));
            self.hold = loudest(loudest(self.hold, left), right);
        }
        if self.mute {
            left.fill(0.0);
            right.fill(0.0);
        }
    }

    fn param_count(&self) -> usize {
        RANGES.len()
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |value| *value as f32)
    }

    /// Setting the shape or the phase knob puts the LFO where the song has
    /// it on the next tick, as it does in FL.
    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(range) = RANGES.get(index) {
            self.params[index] = knob_value(value, *range);
            self.update();
            self.resync |= matches!(index, 8 | 9);
        }
    }

    /// FL's flush: the hold is let go, and the next tick sends every
    /// output whatever it is.
    fn reset(&mut self) {
        self.hold = 0.0;
        self.sent = [-1; 3];
        self.last_tick = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, bits, state};

    /// A state of these parameters with Mute and Ramp.
    fn bytes(params: [i32; 10], mute: bool, ramp: bool) -> Vec<u8> {
        let mut bytes = state(&params);
        bytes.extend([u8::from(mute), u8::from(ramp)]);
        bytes
    }

    fn with(params: [i32; 10]) -> PeakController {
        let mut plugin = PeakController::default();
        plugin.set_state(&bytes(params, false, false)).unwrap();
        plugin
    }

    /// FL's "Default" preset: the peak at unity, the LFO off.
    const PRESET: [i32; 10] = [0, 179, 0, 66, 0, 0, 0, 32_950, 0, 0];
    /// What a sample at full scale gives at the preset's amount.
    const UNITY: f64 = 1.0001;

    /// A plugin with the LFO alone, at full amount.
    fn lfo(speed: i32, shape: i32, phase: i32) -> PeakController {
        with([0, 0, 0, 66, 0, 256, 0, speed, shape, phase])
    }

    /// Play one block that is `level` all the way through.
    fn burst(plugin: &mut PeakController, level: f32) {
        plugin.process(&mut [level; 64], &mut [-level / 2.0; 64]);
    }

    /// Tick `ticks` times from tick `from` on and give output `index`
    /// after each.
    fn run(plugin: &mut PeakController, from: u64, ticks: u64, index: usize) -> Vec<f32> {
        let mut tick = |at: u64| {
            plugin.tick(at, 120.0);
            plugin.controller_outputs()[index]
        };
        (from..from + ticks).map(&mut tick).collect()
    }

    fn close(got: f32, want: f64, within: f64) -> bool {
        (f64::from(got) - want).abs() < within
    }

    #[test]
    fn a_new_plugin_has_three_outputs_at_0_and_passes_its_input() {
        let mut plugin = PeakController::default();
        assert_eq!(plugin.controller_outputs(), [0.0; 3]);
        assert!(!plugin.controller_changed());
        assert_eq!((plugin.mute(), plugin.ramp()), (false, false));
        let params: Vec<f32> = (0..11).map(|i| plugin.param(i)).collect();
        let defaults = [0.0, 0.0, 0.0, 66.0, 0.0, 0.0, 0.0, 32_950.0, 0.0, 0.0, 0.0];
        assert_eq!(
            (params.as_slice(), plugin.param_count()),
            (&defaults[..], 10)
        );

        let input = [0.0, -0.0, 1.0, -1.0, 0.3, -1e-30, 7.5, f32::MIN_POSITIVE];
        let (mut left, mut right) = (input, input);
        right.reverse();
        plugin.process(&mut left, &mut right);
        assert_eq!(bits(&left), bits(&input));
        right.reverse();
        assert_eq!(bits(&right), bits(&input));
        // With the amount at 0 the outputs do not move, and the first tick
        // sends them all the same.
        plugin.tick(0, 120.0);
        assert!(plugin.controller_changed());
        assert_eq!(plugin.controller_outputs(), [0.0; 3]);
    }

    #[test]
    fn mute_silences_the_sound_and_still_follows_it() {
        let mut plugin = PeakController::default();
        plugin.set_state(&bytes(PRESET, true, false)).unwrap();
        assert!(plugin.mute());
        let (mut left, mut right) = ([0.25, -0.5], [0.1, 0.2]);
        plugin.process(&mut left, &mut right);
        assert_eq!((left, right), ([0.0; 2], [0.0; 2]));
        plugin.tick(0, 120.0);
        assert!(close(plugin.controller_outputs()[PEAK], 0.5 * UNITY, 1e-4));
    }

    #[test]
    fn a_burst_lifts_the_peak_and_it_decays_as_the_formula_says() {
        let mut plugin = with(PRESET);
        plugin.set_ppq(96);
        // Unity at 179: 0.4 * (6^(179 / 256) - 1) is 1.0001.
        let unity = 0.4 * (6f64.powf(179.0 / 256.0) - 1.0);
        assert!((unity - UNITY).abs() < 1e-4);
        // The doc's 0.657 a beat at the default decay.
        assert!((decay_per_beat(66) - 0.657).abs() < 1e-3);
        let per_tick = decay_per_beat(66) / 96.0;

        assert_eq!(run(&mut plugin, 0, 3, PEAK), [0.0; 3]);
        // The louder side counts, whichever it is and whatever its sign.
        burst(&mut plugin, 0.5);
        plugin.process(&mut [0.1; 8], &mut [-0.3; 8]);
        let peaks = run(&mut plugin, 3, 80, PEAK);
        for (tick, peak) in peaks.iter().enumerate() {
            let want = (0.5 - tick as f64 * per_tick).max(0.0) * unity;
            assert!(close(*peak, want, 1e-5), "tick {tick}: {peak}, {want}");
        }
        // Down to nothing after 0.5 / 0.657 of a beat: 73 ticks.
        assert!(peaks[72] > 0.0 && peaks[74] == 0.0);
        // With the LFO off, Peak + LFO is the peak and the LFO is 0.
        burst(&mut plugin, 0.25);
        plugin.tick(83, 120.0);
        let outputs = plugin.controller_outputs();
        assert!(close(outputs[PEAK], 0.25 * unity, 1e-5));
        assert_eq!((outputs[PEAK_AND_LFO], outputs[LFO]), (outputs[PEAK], 0.0));
        // A sample over full scale stops the output at 1.
        burst(&mut plugin, 3.0);
        assert_eq!(run(&mut plugin, 84, 1, PEAK), [1.0]);
    }

    #[test]
    fn the_decay_knob_runs_from_a_crawl_to_at_once() {
        assert!((decay_per_beat(5) - 0.006).abs() < 5e-4);
        assert!((decay_per_beat(128) - 19.2).abs() < 1e-9);
        // At 128 a hold of 0.27 is gone in 0.27 / 19.2 of a beat: the tick
        // after it at PPQ 24, and 14 ticks on at PPQ 960.
        for (ppq, ticks) in [(24, 1), (96, 2), (384, 6), (960, 14)] {
            let mut plugin = with([0, 256, 0, 128, 0, 0, 0, 32_950, 0, 0]);
            plugin.set_ppq(ppq);
            burst(&mut plugin, 0.27);
            let peaks = run(&mut plugin, 0, 40, PEAK);
            // Amount 256 is 2.0.
            assert!(close(peaks[0], 0.54, 1e-6), "PPQ {ppq}");
            let gone = peaks.iter().position(|peak| *peak == 0.0);
            assert_eq!(gone, Some(ticks), "PPQ {ppq}");
        }
        // The decay is a part of a beat, so the PPQ does not change how
        // long it takes: a hold of 0.5 lasts 0.76 beats.
        for ppq in [48u16, 96, 192] {
            let mut plugin = with(PRESET);
            plugin.set_ppq(ppq);
            burst(&mut plugin, 0.5);
            let peaks = run(&mut plugin, 0, 2 * u64::from(ppq), PEAK);
            let gone = peaks.iter().position(|peak| *peak == 0.0).unwrap();
            let beats = gone as f64 / f64::from(ppq);
            assert!((beats - 0.5 / 0.657).abs() < 0.03, "PPQ {ppq}: {beats}");
        }
    }

    #[test]
    fn the_amount_scales_the_peak_and_the_base_level_lifts_it() {
        // 0.4 * (6^(|v| / 256) - 1), with the sign of v.
        for (amount, base) in [
            (256, 0),
            (94, 0),
            (-96, 65_536),
            (38, 13_568),
            (-256, 32_768),
        ] {
            let mut plugin = with([base, amount, 0, 66, 0, 0, 0, 32_950, 0, 0]);
            burst(&mut plugin, 0.25);
            plugin.tick(0, 120.0);
            let size = 0.4 * (6f64.powf(f64::from(amount.abs()) / 256.0) - 1.0);
            let want = f64::from(base) / 65_536.0 + f64::from(amount.signum()) * size * 0.25;
            let peak = plugin.controller_outputs()[PEAK];
            assert!(close(peak, want.clamp(0.0, 1.0), 1e-6), "{amount}: {peak}");
        }
        // The doc's two ends: 2.0 for the peak and 1.0 for the LFO.
        assert!(
            (f64::from(Shaper::new(256, 0, SCALE[0]).amount) - 2.0 * FULL as f64).abs() < 256.0
        );
        assert!((f64::from(Shaper::new(256, 0, SCALE[1]).amount) - FULL as f64).abs() < 256.0);
        assert_eq!(Shaper::new(0, 50, SCALE[0]).amount, 0.0);
    }

    #[test]
    fn tension_bows_the_curve_between_the_same_two_ends() {
        for tension in [-128, -22, 29, 58, 92, 128] {
            let shaper = Shaper::new(256, tension, SCALE[1]);
            let at = |x: f32| shaper.at(x) / FULL as f64;
            assert!(at(0.0).abs() < 1e-5, "{tension}");
            assert!((at(1.0) - 1.0).abs() < 1e-5, "{tension}");
            // Up for a tension above 0: a small peak gives a large output.
            assert_eq!(at(0.5) > 0.5, tension > 0, "{tension}: {}", at(0.5));
            // And the curve keeps rising.
            let steps: Vec<f64> = (0..=20).map(|n| at(n as f32 / 20.0)).collect();
            assert!(steps.windows(2).all(|pair| pair[0] < pair[1]), "{tension}");
        }
        // The doc's two curves, at full tension where T is 100.
        let up = Shaper::new(256, 128, SCALE[1]);
        assert!((up.tension - 100.0).abs() < 1e-4);
        let want = 1.0 - (101f64.powf(0.75) - 1.0) / 100.0;
        assert!((up.at(0.25) / FULL as f64 - want).abs() < 1e-5);
        let down = Shaper::new(256, -128, SCALE[1]);
        let want = (101f64.powf(0.25) - 1.0) / 100.0;
        assert!((down.at(0.25) / FULL as f64 - want).abs() < 1e-5);
        // No tension is the straight line.
        assert_eq!(Shaper::new(256, 0, SCALE[1]).curve(0.3), 0.3);
    }

    #[test]
    fn the_lfo_s_period_in_beats_is_the_doc_s_formula() {
        // The doc's three: a bar at the default, 128 beats at the top and
        // the floor of 3 ticks at the bottom.
        assert!((lfo_beats(32_950) - 4.0).abs() < 0.001);
        assert!((lfo_beats(65_536) - 128.0).abs() < 1e-9);
        assert_eq!(phase_ticks(200, 0, 96), 3);
        assert_eq!(phase_ticks(32_950, 0, 96), 384);
        assert_eq!(phase_ticks(32_950, 0, 384), 4 * 384);
        // 20643 is a beat, and 10272, the "Fast LFO" preset's, a quarter.
        assert_eq!(phase_ticks(20_643, 0, 96), 96);
        assert_eq!(phase_ticks(10_272, 3, 96), 24);

        for (speed, ppq) in [(20_643, 96u16), (15_087, 96), (24_109, 384), (32_950, 48)] {
            let mut plugin = lfo(speed, 0, 0);
            plugin.set_ppq(ppq);
            let beats = lfo_beats(speed);
            let ticks = (40.0 * beats * f64::from(ppq)) as u64;
            let levels = run(&mut plugin, 0, ticks, LFO);
            // A sine from 0 to 1 that starts half-way up.
            assert!(close(levels[0], 0.5, 1e-6));
            let high = levels.iter().fold(0f32, |m, v| m.max(*v));
            let low = levels.iter().fold(1f32, |m, v| m.min(*v));
            assert!(high > 0.99 && low < 0.01, "{speed}: {low} to {high}");
            // Where it comes up through the middle.
            let up: Vec<usize> = (1..levels.len())
                .filter(|n| levels[n - 1] < 0.5 && levels[*n] >= 0.5)
                .collect();
            let turns = (up.len() - 1) as f64;
            assert!(turns >= 38.0, "{speed}: {turns}");
            let measured = (up[up.len() - 1] - up[0]) as f64 / turns / f64::from(ppq);
            // To the nearest tick, which is what FL rounds the turn to.
            let within = 0.6 / f64::from(ppq);
            assert!(
                (measured - beats).abs() < within,
                "speed {speed} at PPQ {ppq}: {measured} beats, wanted {beats}"
            );
        }
    }

    #[test]
    fn the_lfo_s_amount_tension_and_base_level_shape_it() {
        // Half the full amount's exponent: 0.2 * (6^0.5 - 1) = 0.29.
        let mut plugin = with([0, 0, 0, 66, 0, 128, 0, 20_643, 2, 0]);
        let size = 0.2 * (6f64.sqrt() - 1.0);
        let levels = run(&mut plugin, 0, 96, LFO);
        // A square: high for the first half of its beat, then low. The
        // step of the phase is rounded down, so tick 48 still reads the
        // last point of the high half.
        assert!(close(levels[0], size, 1e-6) && close(levels[48], size, 1e-6));
        assert_eq!((levels[49], levels[95]), (0.0, 0.0));
        // A base level lifts both halves, and the top stops at 1.
        let mut plugin = with([0, 0, 0, 66, 49_152, 256, 0, 20_643, 2, 0]);
        let levels = run(&mut plugin, 0, 96, LFO);
        assert_eq!((levels[0], levels[49]), (1.0, 0.75));
        // An amount under 0 hangs the LFO from its base level.
        let mut plugin = with([0, 0, 0, 66, 65_536, -256, 0, 20_643, 2, 0]);
        let levels = run(&mut plugin, 0, 96, LFO);
        assert_eq!((levels[0], levels[49]), (0.0, 1.0));
        // Peak + LFO adds the two and stops at 1 as well.
        let mut plugin = with([16_384, 179, 0, 66, 16_384, 256, 0, 20_643, 2, 0]);
        burst(&mut plugin, 0.25);
        plugin.tick(0, 120.0);
        let outputs = plugin.controller_outputs();
        assert!(close(outputs[PEAK], 0.25 + 0.25 * UNITY, 1e-4));
        assert_eq!((outputs[LFO], outputs[PEAK_AND_LFO]), (1.0, 1.0));
        let both = run(&mut plugin, 1, 60, PEAK_AND_LFO);
        assert!(close(both[59], 0.25 + 0.25, 1e-4), "{}", both[59]);
    }

    #[test]
    fn the_shapes_are_the_host_s_tables() {
        // A beat a turn at PPQ 96, read on its quarters.
        let quarters = |shape: i32| {
            let levels = run(&mut lfo(20_643, shape, 0), 0, 96, LFO);
            [0, 24, 48, 72].map(|tick| (levels[tick] * 1000.0).round() / 1000.0)
        };
        assert_eq!(quarters(0), [0.5, 1.0, 0.5, 0.0]);
        assert_eq!(quarters(1), [0.5, 1.0, 0.5, 0.0]);
        // The square's tick 48 is the last point of its high half.
        assert_eq!(quarters(2), [1.0, 1.0, 1.0, 0.0]);
        assert_eq!(run(&mut lfo(20_643, 2, 0), 0, 96, LFO)[49], 0.0);
        assert_eq!(quarters(3), [0.0, 0.25, 0.5, 0.75]);
        // The triangle is straight where the sine is round.
        let eighth = |shape: i32| run(&mut lfo(20_643, shape, 0), 0, 96, LFO)[12];
        assert!(close(eighth(1), 0.75, 1e-3));
        assert!(close(eighth(0), 0.5 + 0.5 * 0.5f64.sqrt(), 1e-3));
    }

    #[test]
    fn random_holds_a_level_for_a_turn_and_jumps() {
        // A quarter of a beat a turn, as in the "Randomness" preset.
        assert_eq!(phase_ticks(10_272, 4, 96), 24 << 14);
        let mut plugin = lfo(10_272, 4, 0);
        let levels = run(&mut plugin, 0, 24 * 40, LFO);
        let mut held = Vec::new();
        // The step is rounded down, so the first point lasts a tick more.
        assert_eq!(levels[0], levels[1]);
        for turn in levels[1..].chunks(24) {
            assert!(turn.iter().all(|level| *level == turn[0]));
            held.push(turn[0]);
        }
        // Noise: no two turns in a row alike, and all over the range.
        assert!(held.windows(2).all(|pair| pair[0] != pair[1]));
        assert!(held.iter().any(|level| *level < 0.2) && held.iter().any(|level| *level > 0.8));
        // The same noise every time: it is the host's table.
        let again = run(&mut lfo(10_272, 4, 0), 0, 24 * 40, LFO);
        assert_eq!(again, levels);
    }

    #[test]
    fn the_lfo_is_where_the_song_is_after_a_seek() {
        // A saw of a beat. Played from the start, tick 1000 is 40 ticks
        // into its eleventh turn.
        let mut played = lfo(20_643, 3, 0);
        let levels = run(&mut played, 0, 1001, LFO);
        assert!(close(levels[1000], 40.0 / 96.0, 1e-3));
        // A plugin that starts there, or is moved there, has the same.
        let mut started = lfo(20_643, 3, 0);
        assert_eq!(run(&mut started, 1000, 1, LFO), [levels[1000]]);
        let mut moved = lfo(20_643, 3, 0);
        run(&mut moved, 0, 7, LFO);
        assert_eq!(run(&mut moved, 1000, 1, LFO), [levels[1000]]);
        // And back, as when the song starts over.
        assert_eq!(run(&mut moved, 0, 30, LFO), levels[..30]);
        // The same tick twice is no step on, so it counts as a move too.
        assert_eq!(run(&mut moved, 29, 1, LFO), [levels[29]]);

        // The phase knob is a part of a turn: a quarter on.
        let mut ahead = lfo(20_643, 3, 16_384);
        assert!(close(run(&mut ahead, 0, 1, LFO)[0], 0.25, 1e-3));
        assert!(close(run(&mut ahead, 1000, 1, LFO)[0], 64.0 / 96.0, 1e-3));
        // Setting it while the song plays moves the LFO on the next tick.
        let mut plugin = lfo(20_643, 3, 0);
        run(&mut plugin, 0, 10, LFO);
        plugin.set_param(9, 32_768.0);
        assert!(close(run(&mut plugin, 10, 1, LFO)[0], 58.0 / 96.0, 1e-3));
        // The speed alone does not: the LFO goes on from where it is.
        let mut plugin = lfo(20_643, 3, 0);
        run(&mut plugin, 0, 48, LFO);
        plugin.set_param(7, 10_272.0);
        let on = run(&mut plugin, 48, 2, LFO);
        assert!(close(on[0], 0.5, 1e-3) && close(on[1], 0.5 + 1.0 / 24.0, 1e-3));
    }

    #[test]
    fn an_output_is_sent_when_it_moves_and_not_otherwise() {
        let mut plugin = with(PRESET);
        // The first tick sends all three, whatever they are.
        plugin.tick(0, 120.0);
        assert!(plugin.controller_changed());
        assert!(!plugin.controller_changed());
        plugin.tick(1, 120.0);
        assert!(!plugin.controller_changed());
        burst(&mut plugin, 0.5);
        assert!(!plugin.controller_changed());
        plugin.tick(2, 120.0);
        assert!(plugin.controller_changed());
        // While the hold decays every tick moves the peak.
        plugin.tick(3, 120.0);
        assert!(plugin.controller_changed());
        // A reset lets go of the hold and sends everything again.
        plugin.reset();
        plugin.tick(4, 120.0);
        assert!(plugin.controller_changed());
        assert_eq!(plugin.controller_outputs(), [0.0; 3]);
        plugin.tick(5, 120.0);
        assert!(!plugin.controller_changed());
    }

    #[test]
    fn ramp_moves_an_output_an_eighth_of_full_scale_a_tick() {
        // A square LFO at full amount, a beat a turn, with Ramp on.
        let mut plugin = PeakController::default();
        let params = [0, 0, 0, 66, 0, 256, 0, 20_643, 2, 0];
        plugin.set_state(&bytes(params, false, true)).unwrap();
        assert!(plugin.ramp());
        let levels = run(&mut plugin, 0, 96, LFO);
        // Up from nothing in eight ticks, and down again from tick 49,
        // the first of the square's low half.
        let eighths =
            |from: usize| -> Vec<f32> { levels[from..from + 9].iter().map(|v| v * 8.0).collect() };
        assert!(close(levels[0], 0.125, 1e-6));
        assert!(close(levels[6], 0.875, 1e-6) && levels[7] == 1.0 && levels[48] == 1.0);
        assert_eq!(eighths(49), [7.0, 6.0, 5.0, 4.0, 3.0, 2.0, 1.0, 0.0, 0.0]);
        // Without Ramp it is there at once.
        let mut plugin = lfo(20_643, 2, 0);
        let levels = run(&mut plugin, 0, 96, LFO);
        assert_eq!((levels[0], levels[48], levels[49]), (1.0, 1.0, 0.0));
    }

    #[test]
    fn parameters_are_whole_numbers_inside_the_wheels_ranges() {
        let mut plugin = PeakController::default();
        for (index, (low, high)) in RANGES.into_iter().enumerate() {
            plugin.set_param(index, -1e9);
            assert_eq!(plugin.param(index), low as f32);
            plugin.set_param(index, 1e9);
            assert_eq!(plugin.param(index), high as f32);
        }
        plugin.set_param(1, 178.6);
        plugin.set_param(10, 1.0);
        assert_eq!((plugin.param(1), plugin.param(10)), (179.0, 0.0));
        // Every corner of the ranges ticks without a number going wrong.
        for corner in 0..1024 {
            let params = std::array::from_fn(|i| match corner >> i & 1 {
                0 => RANGES[i].0,
                _ => RANGES[i].1,
            });
            let mut plugin = with(params);
            burst(&mut plugin, 1.5);
            for tick in 0..8 {
                plugin.tick(tick, 120.0);
                let outputs = plugin.controller_outputs();
                assert!(
                    outputs.iter().all(|v| (0.0..=1.0).contains(v)),
                    "{params:?}: {outputs:?}"
                );
            }
        }
    }

    #[test]
    fn a_state_the_plugin_does_not_write_is_turned_down() {
        let mut plugin = with(PRESET);
        let before = plugin.clone();
        for len in [0, 1, 40, 41, 43, 44] {
            assert_eq!(
                plugin.set_state(&vec![0; len]),
                Err(StateError::Length {
                    expected: 42,
                    found: len
                }),
                "{len} bytes"
            );
        }
        for (param, value) in [
            (0, 65_537),
            (1, -257),
            (3, 4),
            (7, 199),
            (8, 5),
            (9, 65_536),
        ] {
            let mut params = PRESET;
            params[param] = value;
            assert_eq!(
                plugin.set_state(&bytes(params, false, false)),
                Err(StateError::Range { param, value })
            );
        }
        assert_eq!(plugin, before);
    }

    #[test]
    fn every_state_fl_ships_loads_and_ticks() {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, the states of {NAME} skipped");
            return;
        };
        let named = |saved: &testing::Saved| saved.plugin.eq_ignore_ascii_case(NAME);
        let projects: Vec<_> = testing::projects(&root).into_iter().filter(named).collect();
        let presets = testing::every_preset(&root, NAME);
        // The doc's counts: 36 uses in the projects, three presets and the
        // three templates.
        assert_eq!((projects.len(), presets.len()), (36, 6));
        let (mut muted, mut ramped) = (0, 0);
        for saved in projects.iter().chain(&presets) {
            let mut plugin = PeakController::default();
            assert_eq!(saved.state.len(), 42, "{}", saved.file);
            if let Err(error) = plugin.set_state(&saved.state) {
                panic!("{}: {error}", saved.file);
            }
            muted += usize::from(plugin.mute());
            ramped += usize::from(plugin.ramp());
            let (mut left, mut right) = (testing::sine(0.01, 256), testing::sine(0.037, 256));
            for tick in 0..400 {
                if tick % 50 == 0 {
                    plugin.process(&mut left, &mut right);
                }
                plugin.tick(tick, 140.0);
                let outputs = plugin.controller_outputs();
                assert!(
                    outputs.iter().all(|v| (0.0..=1.0).contains(v)),
                    "{}: {outputs:?}",
                    saved.file
                );
            }
        }
        // Mute is on in two projects, and Ramp in the "Randomness" preset
        // alone. The doc's table has three and two.
        assert_eq!((muted, ramped), (2, 1));
        let preset = |name: &str| {
            let found = presets.iter().find(|saved| saved.file == name).unwrap();
            found.state.clone()
        };
        assert_eq!(preset("Default"), bytes(PRESET, false, false));
        let random = [0, 0, 0, 66, 0, 256, 0, 10_272, 4, 0];
        assert_eq!(preset("Randomness"), bytes(random, false, true));
        // A saw that hangs from full scale, bowed up. The doc's table has
        // it rising from 0 with a tension of -1.
        let fast = [0, 0, 0, 66, 65_536, -256, 66, 10_272, 3, 0];
        assert_eq!(preset("Fast LFO"), bytes(fast, false, false));
    }
}
