//! Fruity Delay Bank: eight delay units in a row, each with its own gains,
//! feedback and grain, as `docs/plugins/fruity-delay-bank.md` has it.
//!
//! The state is 792 bytes: a version of 15 or 16, 133 little-endian i32,
//! and 32 bytes for each unit. The i32 are four parameters for the whole
//! plugin, sixteen for each of the eight units, and the oversampling
//! switch:
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Dry, 1 Wet, 2 Input, 3 Feedback | 0 to 128 | 128 |
//! | 4 Input panning | -128 to 128 | 0 |
//! | 5 Input volume | -160 to 160 | 128 |
//! | 6, 7, 8 Input filter cutoff, resonance, gain | 0 to 65536, 65536, 256 | 33145, 17500, 128 |
//! | 9 Feedback panning | -128 to 128 | 0 |
//! | 10 Feedback volume | -128 to 128 | 64 |
//! | 11 Feedback stereo separation | -128 to 128 | 0 |
//! | 12 Time | 0 to 7680 | 1440 |
//! | 13 Time stereo offset | -512 to 512 | 0 |
//! | 14, 15, 16 Feedback filter cutoff, resonance, gain | as 6, 7, 8 | |
//! | 17 Output panning | -128 to 128 | 0 |
//! | 18 Output volume | -160 to 160 | 128 |
//! | 19 To next delay | -160 to 160 | 0 |
//!
//! Unit `u` has its sixteen at `4 + 16 * u`. A unit's 32 bytes are what
//! its switches are at: the two filters' modes and types, whether the
//! input filter is after the delay, the feedback mode, the grain's
//! division and shape, whether the unit is on, whether its first echo is
//! left alone, and whether its time is in beats.
//!
//! A unit takes the plugin's input through its input gains and, where the
//! unit before sends to it, that unit's output. That goes into a delay
//! line, whose output comes back into it through the feedback gains. What
//! leaves the line goes through the grain and the output gains into the
//! wet sum, beside the dry input.
//!
//! Not built: the two filters of a unit, the 2x oversampling and the
//! bend of the grain's shape. A state that has one of them on says so in
//! [`Effect::fidelity`].

use crate::parts::{Matrix, glide, offset_frames, stereo_gains};
use crate::{Effect, Fidelity, StateError, knob_gain, knob_value, state_version};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Delay Bank";

const UNITS: usize = 8;
/// The parameters for the whole plugin, and for one unit.
const GLOBALS: usize = 4;
const EACH: usize = 16;
/// The i32 the state has, the last of them the oversampling switch.
const SAVED: usize = GLOBALS + EACH * UNITS + 1;
/// The parameters a host can move: the last unit has no "to next".
const PARAMS: usize = GLOBALS + EACH * UNITS - 1;

/// How long the state is, where its i32 end, and how long a unit's
/// switches are.
const LEN: usize = 792;
const SWITCHES_AT: usize = 4 + 4 * SAVED;
const SWITCHES: usize = 32;

/// The range of each parameter of the plugin and of a unit.
const RANGES: [(i32, i32); GLOBALS + EACH] = [
    (0, 128),
    (0, 128),
    (0, 128),
    (0, 128),
    (-128, 128),
    (-160, 160),
    (0, 65_536),
    (0, 65_536),
    (0, 256),
    (-128, 128),
    (-128, 128),
    (-128, 128),
    (0, 7680),
    (-512, 512),
    (0, 65_536),
    (0, 65_536),
    (0, 256),
    (-128, 128),
    (-160, 160),
    (-160, 160),
];
const DEFAULTS: [i32; GLOBALS + EACH] = [
    128, 128, 128, 128, 0, 128, 33_145, 17_500, 0, 0, 64, 0, 1440, 0, 33_145, 17_500, 0, 0, 128, 0,
];

/// A unit's parameters, counted from the unit's first.
const IN_PAN: usize = 0;
const IN_VOL: usize = 1;
const FB_PAN: usize = 5;
const FB_VOL: usize = 6;
const FB_SEP: usize = 7;
const TIME: usize = 8;
const OFFSET: usize = 9;
const OUT_PAN: usize = 13;
const OUT_VOL: usize = 14;
const NEXT: usize = 15;

/// The feedback modes.
const OFF: i32 = 0;
const INVERTED: i32 = 2;
const PING_PONG: i32 = 3;

/// What FL makes 0 of in a delay line: a size under 2^-24.
const TINY: f32 = 5.960_464_5e-8;

/// A volume wheel that goes under 0: the knob curve on its size, with
/// its sign.
fn signed_gain(value: i32) -> f32 {
    let size = value.abs() as f32 * (1.0 / 128.0);
    let gain = if size == 1.0 {
        1.0
    } else {
        knob_gain(f64::from(size)) as f32
    };
    gain * value.signum() as f32
}

/// The range of a parameter by its index.
fn range(index: usize) -> (i32, i32) {
    if index < GLOBALS {
        RANGES[index]
    } else {
        RANGES[GLOBALS + (index - GLOBALS) % EACH]
    }
}

/// A pan and a volume as four gains that glide to where they are put.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Gain {
    pan: f32,
    volume: f32,
    /// The gains the pan and the volume make, worked out by the next
    /// block when `moved` says they are stale, and the gains in use.
    target: [f32; 4],
    current: [f32; 4],
    moved: bool,
}

impl Gain {
    /// The gains for a block's first frame and what to add every frame.
    fn ramp(&mut self, cap: f32, len: usize) -> ([f32; 4], [f32; 4]) {
        if self.moved {
            self.target = stereo_gains(self.pan, self.volume, 0.0);
            self.moved = false;
        }
        ramp(&mut self.current, self.target, cap, len)
    }
}

/// Glide four gains to their targets over a block.
fn ramp(current: &mut [f32; 4], target: [f32; 4], cap: f32, len: usize) -> ([f32; 4], [f32; 4]) {
    let (mut start, mut step) = ([0.0; 4], [0.0; 4]);
    for k in 0..4 {
        (start[k], step[k]) = glide(&mut current[k], target[k], cap, len);
    }
    (start, step)
}

/// The grain's window for a shape of 0: a raised cosine, its two halves
/// measured on their own.
fn window(len: usize) -> Vec<f32> {
    let last = len.saturating_sub(1);
    let half = last / 2;
    let rise = (std::f64::consts::PI / half as f64) as f32;
    let fall = (std::f64::consts::PI / (last - half) as f64) as f32;
    let value = |k: usize| {
        let angle = if k < half {
            rise * k as f32
        } else {
            (f64::from(fall * (k - half) as f32) + std::f64::consts::PI) as f32
        };
        let value = (0.5 - f64::from(angle).cos() * 0.5) as f32;
        // A window of one or two samples divides by 0 in FL.
        if value.is_finite() { value } else { 1.0 }
    };
    (0..len).map(value).collect()
}

/// One delay unit.
#[derive(Debug, Clone, PartialEq, Default)]
struct Unit {
    /// What the unit's 32 bytes have: the filters' modes and types, the
    /// input filter after the delay, the feedback mode, the grain, the
    /// unit on, the first echo left alone, the time in beats.
    filters: [[i32; 2]; 2],
    post: bool,
    mode: i32,
    grain_division: i32,
    grain_shape: i32,
    enabled: bool,
    first: bool,
    beats: bool,

    input: Gain,
    output: Gain,
    /// The input and the output wheels' own gains, before the plugin's
    /// Input and Wet are on them, and the gain to the next unit.
    in_volume: f32,
    out_volume: f32,
    next: f32,
    /// The feedback's gains as the last block worked them out, and the
    /// ones in use.
    fb_target: [f32; 4],
    fb_current: [f32; 4],

    /// The delay in frames, how much later each side goes in, the line
    /// and where it is read. A unit with no delay has no line.
    delay: usize,
    late: [usize; 2],
    line: Vec<[f32; 2]>,
    at: usize,
    /// The grain's window, how long it is before rounding, and where in
    /// it the unit is.
    grain: Vec<f32>,
    grain_period: f64,
    grain_phase: f64,
}

impl Unit {
    /// Whether the unit has a delay line: it is on and its feedback mode
    /// is not "off".
    fn delays(&self) -> bool {
        self.enabled && self.mode != OFF
    }

    /// The delay time and the grain, from the two time wheels.
    fn time(&mut self, time: i32, offset: i32, rate: f32, tempo: f32) {
        let late = offset_frames(offset, f64::from(rate));
        let mut frames = (time * rate as i32) as f32;
        if self.beats {
            frames = (f64::from(frames) * 0.031_25 / f64::from(tempo)) as f32;
        } else {
            frames /= 3840.0;
        }
        let delay = (frames.round_ties_even() as usize).max(1);
        if self.delays() {
            // A line of another length starts empty.
            if self.line.len() != delay + late + 1 {
                self.line = vec![[0.0; 2]; delay + late + 1];
                self.at = 0;
            }
            self.delay = delay;
            self.late = if offset > 0 { [0, late] } else { [late, 0] };
        } else {
            self.line = Vec::new();
        }

        let period = f64::from(frames / self.grain_division as f32);
        let len = period.round_ties_even();
        if self.grain_division < 1 || !self.enabled || len.is_nan() || len < 1.0 {
            self.grain = Vec::new();
        } else {
            self.grain_period = period;
            if self.grain.len() != len as usize {
                self.grain = window(len as usize);
            }
        }
    }

    /// What FL's flush does to a unit: the line and the grain start
    /// over, and the gains are where they were last worked out to be.
    fn flush(&mut self) {
        self.line.fill([0.0; 2]);
        self.grain_phase = 0.0;
        self.input.current = self.input.target;
        self.output.current = self.output.target;
        self.fb_current = self.fb_target;
    }
}

/// Fruity Delay Bank. A new one has FL's defaults: the first unit on,
/// three steps of delay, ping-pong off, half of every echo fed back.
#[derive(Debug, Clone, PartialEq)]
pub struct DelayBank {
    params: [i32; SAVED],
    rate: f32,
    tempo: f32,
    units: [Unit; UNITS],
    /// The plugin's four wheels as gains.
    dry: f32,
    wet: f32,
    input: f32,
    feedback: f32,
}

impl Default for DelayBank {
    fn default() -> Self {
        let mut params = [0; SAVED];
        params[..GLOBALS].copy_from_slice(&DEFAULTS[..GLOBALS]);
        for unit in params[GLOBALS..SAVED - 1].chunks_mut(EACH) {
            unit.copy_from_slice(&DEFAULTS[GLOBALS..]);
        }
        let mut plugin = DelayBank {
            params,
            rate: 44_100.0,
            tempo: 140.0,
            units: Default::default(),
            dry: 1.0,
            wet: 1.0,
            input: 1.0,
            feedback: 1.0,
        };
        for unit in &mut plugin.units {
            unit.mode = 1;
            unit.beats = true;
        }
        plugin.units[0].enabled = true;
        plugin.set();
        plugin
    }
}

impl DelayBank {
    /// A unit's parameter.
    fn of(&self, unit: usize, which: usize) -> i32 {
        self.params[GLOBALS + EACH * unit + which]
    }

    /// Work out everything the parameters, the switches, the rate and
    /// the tempo make. The gains are left for the next block to glide to.
    fn set(&mut self) {
        let plain = |value: i32| {
            let p = value as f32 * (1.0 / 128.0);
            if p == 1.0 {
                1.0
            } else {
                knob_gain(f64::from(p)) as f32
            }
        };
        self.dry = plain(self.params[0]);
        self.wet = plain(self.params[1]);
        self.input = plain(self.params[2]);
        self.feedback = self.params[3] as f32 * (1.0 / 128.0);
        for at in 0..UNITS {
            let of = |which: usize| self.of(at, which);
            let (in_pan, in_vol) = (of(IN_PAN), of(IN_VOL));
            let (out_pan, out_vol, next) = (of(OUT_PAN), of(OUT_VOL), of(NEXT));
            let (time, offset) = (of(TIME), of(OFFSET));
            let (input, wet, rate, tempo) = (self.input, self.wet, self.rate, self.tempo);
            let unit = &mut self.units[at];
            unit.in_volume = signed_gain(in_vol);
            unit.out_volume = signed_gain(out_vol);
            unit.next = signed_gain(next);
            let gains = [
                (&mut unit.input, in_pan, unit.in_volume * input),
                (&mut unit.output, out_pan, unit.out_volume * wet),
            ];
            for (gain, pan, volume) in gains {
                let pan = pan as f32 * (1.0 / 128.0);
                if gain.pan != pan || gain.volume != volume {
                    (gain.pan, gain.volume, gain.moved) = (pan, volume, true);
                }
            }
            unit.time(time, offset, rate, tempo);
        }
    }

    /// The feedback's four gains for a unit, as FL works them out for
    /// every block.
    fn feedback_gains(&self, at: usize) -> [f32; 4] {
        let pan = self.of(at, FB_PAN) as f32 * (1.0 / 128.0);
        let mut volume = self.of(at, FB_VOL) as f32 * (1.0 / 128.0);
        let separation = self.of(at, FB_SEP) as f32 * (1.0 / 128.0);
        if separation < 0.0 {
            volume *= separation * 0.5 + 1.0;
        }
        let [ll, rr, rl, lr] = stereo_gains(pan, volume * self.feedback, separation);
        if self.units[at].mode == PING_PONG {
            // Each side feeds the other.
            [lr, rl, rr, ll]
        } else {
            [ll, rr, rl, lr]
        }
    }

    /// What of the state this does not do, if anything.
    fn missing(&self) -> Option<&'static str> {
        let on = self.units.iter().filter(|unit| unit.enabled);
        let filtered =
            |unit: &Unit| unit.filters[0][0] != 0 || (unit.delays() && unit.filters[1][0] != 0);
        if on.clone().any(filtered) {
            Some("Delay Bank's filters are not built: the units run without them")
        } else if self.params[SAVED - 1] > 0 {
            Some("Delay Bank's 2x oversampling is not built")
        } else if on
            .clone()
            .any(|u| !u.grain.is_empty() && u.grain_shape != 0)
        {
            Some("Delay Bank's grain shape is not built: the grain is a raised cosine")
        } else {
            None
        }
    }
}

impl Effect for DelayBank {
    /// FL reads a version of 15 or more and leaves the plugin alone for
    /// an older one. Before 16 the time wheel counted in tens.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let version = state_version(bytes, LEN)?;
        if version < 15 {
            return Err(StateError::Version { found: version });
        }
        if bytes.len() != LEN {
            return Err(StateError::Length {
                expected: LEN,
                found: bytes.len(),
            });
        }
        let mut params = [0; SAVED];
        for (param, word) in bytes[4..SWITCHES_AT].as_chunks::<4>().0.iter().enumerate() {
            let mut value = i32::from_le_bytes(*word);
            if param < SAVED - 1 {
                let unit = param >= GLOBALS && (param - GLOBALS) % EACH == TIME;
                if unit && version < 16 {
                    value = value.saturating_mul(10);
                }
                let (min, max) = range(param);
                if value < min || value > max {
                    return Err(StateError::Range { param, value });
                }
            }
            params[param] = value;
        }
        self.params = params;
        let switches = bytes[SWITCHES_AT..].as_chunks::<SWITCHES>().0;
        for (unit, bytes) in self.units.iter_mut().zip(switches) {
            let word = |at: usize| i32::from_le_bytes(*bytes[at..].first_chunk::<4>().unwrap());
            unit.filters = [[word(0), word(4)], [word(8), word(12)]];
            unit.post = bytes[16] != 0;
            unit.mode = word(17);
            unit.grain_division = word(21);
            unit.grain_shape = word(25);
            unit.enabled = bytes[29] != 0;
            unit.first = bytes[30] != 0;
            unit.beats = bytes[31] != 0;
        }
        // FL sets the units up, flushes them, and only then takes the
        // parameters: the gains glide to them from where they were.
        for unit in &mut self.units {
            unit.flush();
        }
        self.set();
        Ok(())
    }

    /// The delay times are worked out for the rate. A rate that is not a
    /// number above 0 is not taken.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz > 0.0 {
            self.rate = hz;
            self.set();
            for unit in &mut self.units {
                unit.flush();
            }
        }
    }

    /// The times of the units that count in beats follow the tempo.
    fn set_tempo(&mut self, bpm: f64) {
        if bpm.is_finite() && bpm > 0.0 && bpm as f32 != self.tempo {
            self.tempo = bpm as f32;
            self.set();
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        if len == 0 {
            return;
        }
        // The most a gain moves a frame, as FL's renderer has it.
        let cap = 0.001 * 44_100.0 / self.rate;
        let mut ramps = [([0.0; 4], [0.0; 4]); UNITS * 3];
        let mut grain_left = [0usize; UNITS];
        let mut grain_at = [0usize; UNITS];
        for at in 0..UNITS {
            if !self.units[at].enabled {
                continue;
            }
            let feedback = self.feedback_gains(at);
            let unit = &mut self.units[at];
            ramps[at * 3] = unit.input.ramp(cap, len);
            ramps[at * 3 + 1] = unit.output.ramp(cap, len);
            if !unit.line.is_empty() {
                unit.fb_target = feedback;
                ramps[at * 3 + 2] = ramp(&mut unit.fb_current, feedback, cap, len);
            }
        }

        for (frame, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
            let (dry_left, dry_right) = (*l, *r);
            let mut wet = (dry_left * self.dry, dry_right * self.dry);
            let mut before = (0.0, 0.0);
            let mut send = 0.0;
            for (at, unit) in self.units.iter_mut().enumerate() {
                if !unit.enabled {
                    send = 0.0;
                    continue;
                }
                let [input, output, feedback] = [0, 1, 2].map(|k| {
                    let (start, step) = ramps[at * 3 + k];
                    std::array::from_fn::<f32, 4, _>(|g| start[g] + step[g] * frame as f32)
                });
                let mut x = Matrix::mix(&input, dry_left, dry_right);
                if send != 0.0 {
                    x.0 += before.0 * send;
                    x.1 += before.1 * send;
                }
                if !unit.line.is_empty() {
                    let size = unit.line.len();
                    let write = (unit.at + unit.delay) % size;
                    unit.line[(write + unit.late[0]) % size][0] += x.0;
                    unit.line[(write + unit.late[1]) % size][1] += x.1;
                    let [out_left, out_right] = unit.line[unit.at];
                    let mut back = Matrix::mix(&feedback, out_left, out_right);
                    if back.0.abs() < TINY {
                        back.0 = 0.0;
                    }
                    if back.1.abs() < TINY {
                        back.1 = 0.0;
                    }
                    x = if unit.first {
                        (out_left, out_right)
                    } else {
                        back
                    };
                    unit.line[unit.at] = [0.0; 2];
                    unit.line[write][0] += back.0;
                    unit.line[write][1] += back.1;
                    unit.at = (unit.at + 1) % size;
                    if unit.mode == INVERTED {
                        x = (x.1, x.0);
                    }
                }
                if !unit.grain.is_empty() {
                    if grain_left[at] == 0 {
                        while unit.grain_period <= unit.grain_phase {
                            unit.grain_phase -= unit.grain_period;
                        }
                        let phase = unit.grain_phase.round_ties_even() as usize;
                        grain_at[at] = phase % unit.grain.len();
                        grain_left[at] = (unit.grain.len() - grain_at[at]).min(len - frame);
                        unit.grain_phase += grain_left[at] as f64;
                    }
                    let shape = unit.grain[grain_at[at]];
                    x = (shape * x.0, shape * x.1);
                    grain_at[at] += 1;
                    grain_left[at] -= 1;
                }
                let out = Matrix::mix(&output, x.0, x.1);
                wet.0 += out.0;
                wet.1 += out.1;
                before = x;
                send = unit.next;
            }
            (*l, *r) = wet;
        }
    }

    fn param_count(&self) -> usize {
        PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        if index < PARAMS {
            self.params[index] as f32
        } else {
            0.0
        }
    }

    /// A time wheel that moves makes a line of another length, which asks
    /// for memory here and never in [`Effect::process`].
    fn set_param(&mut self, index: usize, value: f32) {
        if index < PARAMS {
            self.params[index] = knob_value(value, range(index));
            self.set();
        }
    }

    /// What FL's flush does: the lines empty, the grains at their start,
    /// and every gain where the last block worked it out to be.
    fn reset(&mut self) {
        for unit in &mut self.units {
            unit.flush();
        }
    }

    fn fidelity(&self) -> Fidelity {
        self.missing()
            .map_or(Fidelity::Exact, Fidelity::Approximate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{presets, state};

    /// What a unit's 32 bytes are: its switches.
    #[derive(Clone, Copy)]
    struct Switches {
        mode: i32,
        grain: i32,
        enabled: bool,
        first: bool,
        beats: bool,
        in_filter: i32,
    }

    const ON: Switches = Switches {
        mode: 1,
        grain: 0,
        enabled: true,
        first: true,
        beats: false,
        in_filter: 0,
    };
    const NOT_ON: Switches = Switches {
        enabled: false,
        ..ON
    };

    /// A state of version 16 with these units on and these parameters
    /// moved from the defaults.
    fn saved(units: &[Switches], moves: &[(usize, i32)]) -> Vec<u8> {
        let mut params = DelayBank::default().params;
        for (index, value) in moves {
            params[*index] = *value;
        }
        let mut bytes = state(&[16]);
        bytes.extend(state(&params));
        for at in 0..UNITS {
            let unit = units.get(at).copied().unwrap_or(NOT_ON);
            bytes.extend(state(&[unit.in_filter, 0, 0, 0]));
            bytes.push(0);
            bytes.extend(state(&[unit.mode, unit.grain, 0]));
            bytes.extend([unit.enabled, unit.first, unit.beats].map(u8::from));
        }
        assert_eq!(bytes.len(), LEN);
        bytes
    }

    /// A unit's parameter by its place in the unit.
    fn at(unit: usize, which: usize) -> usize {
        GLOBALS + EACH * unit + which
    }

    /// A plugin with a state, after the three blocks its gains take to
    /// come up from nothing.
    fn with(units: &[Switches], moves: &[(usize, i32)]) -> DelayBank {
        let mut plugin = DelayBank::default();
        plugin.set_sample_rate(48_000.0);
        plugin.set_state(&saved(units, moves)).unwrap();
        for _ in 0..3 {
            plugin.process(&mut [0.0; 512], &mut [0.0; 512]);
        }
        plugin
    }

    /// A click on the left, or on both sides, through the plugin in
    /// blocks of 512: where each side is not 0, and what it is there.
    fn click(plugin: &mut DelayBank, sides: [f32; 2], len: usize) -> [Vec<(usize, f32)>; 2] {
        let (mut left, mut right) = (vec![0.0; len], vec![0.0; len]);
        (left[0], right[0]) = (sides[0], sides[1]);
        for (l, r) in left.chunks_mut(512).zip(right.chunks_mut(512)) {
            plugin.process(l, r);
        }
        [left, right].map(|side| {
            let heard = side.iter().copied().enumerate();
            heard.filter(|(_, v)| *v != 0.0).collect()
        })
    }

    #[test]
    fn the_defaults_are_the_wheels_of_fl_s_window() {
        let plugin = DelayBank::default();
        assert_eq!(plugin.param_count(), 131);
        let unit: Vec<f32> = (4..20).map(|index| plugin.param(index)).collect();
        assert_eq!(
            unit,
            DEFAULTS[4..].iter().map(|v| *v as f32).collect::<Vec<_>>()
        );
        assert_eq!(plugin.param(at(7, TIME)), 1440.0);
        assert_eq!(plugin.param(131), 0.0);
        assert_eq!((plugin.dry, plugin.wet, plugin.feedback), (1.0, 1.0, 1.0));
        // The first unit is on, at three steps of 140 BPM.
        assert!(plugin.units[0].enabled && !plugin.units[1].enabled);
        assert_eq!(plugin.units[0].delay, 14_175);
        assert_eq!(plugin.latency(), 0);
        assert_eq!(plugin.fidelity(), Fidelity::Exact);
    }

    #[test]
    fn with_no_feedback_each_line_gives_one_echo_at_its_delay() {
        // Two units, 0.375 s and 0.1875 s, their first echoes left alone
        // and nothing fed back.
        let moves = [(at(0, FB_VOL), 0), (at(1, FB_VOL), 0), (at(1, TIME), 720)];
        let mut plugin = with(&[ON, ON], &moves);
        assert_eq!(
            (plugin.units[0].delay, plugin.units[1].delay),
            (18_000, 9000)
        );
        let [left, right] = click(&mut plugin, [1.0, 1.0], 48_000);
        assert_eq!(left, [(0, 1.0), (9000, 1.0), (18_000, 1.0)]);
        assert_eq!(right, left);
    }

    #[test]
    fn the_feedback_scales_every_echo_and_the_first_one_too_unless_it_is_left() {
        // Half of what leaves the line goes back in.
        let mut plugin = with(&[ON], &[]);
        let [left, _] = click(&mut plugin, [1.0, 1.0], 60_000);
        assert_eq!(
            left,
            [(0, 1.0), (18_000, 1.0), (36_000, 0.5), (54_000, 0.25)]
        );
        // With the first echo not left alone, the output is what goes
        // back: the first echo has the feedback's gain already.
        let processed = Switches { first: false, ..ON };
        let mut plugin = with(&[processed], &[]);
        let [left, _] = click(&mut plugin, [1.0, 1.0], 60_000);
        assert_eq!(
            left,
            [(0, 1.0), (18_000, 0.5), (36_000, 0.25), (54_000, 0.125)]
        );
        // The plugin's Feedback wheel is on top of the unit's.
        let mut plugin = with(&[ON], &[(3, 64)]);
        let [left, _] = click(&mut plugin, [1.0, 1.0], 40_000);
        assert_eq!(left, [(0, 1.0), (18_000, 1.0), (36_000, 0.25)]);
    }

    #[test]
    fn ping_pong_crosses_the_sides_and_inverted_swaps_the_output() {
        let pong = Switches {
            mode: PING_PONG,
            ..ON
        };
        let mut plugin = with(&[pong], &[]);
        let [left, right] = click(&mut plugin, [1.0, 0.0], 60_000);
        assert_eq!(left, [(0, 1.0), (18_000, 1.0), (54_000, 0.25)]);
        assert_eq!(right, [(36_000, 0.5)]);
        let inverted = Switches {
            mode: INVERTED,
            ..ON
        };
        let mut plugin = with(&[inverted], &[]);
        let [left, right] = click(&mut plugin, [1.0, 0.0], 40_000);
        assert_eq!(left, [(0, 1.0)]);
        assert_eq!(right, [(18_000, 1.0), (36_000, 0.5)]);
        // With the feedback mode off there is no line: the unit's input
        // is its output.
        let off = Switches { mode: OFF, ..ON };
        let mut plugin = with(&[off], &[]);
        let [left, _] = click(&mut plugin, [1.0, 1.0], 20_000);
        assert_eq!(left, [(0, 2.0)]);
    }

    #[test]
    fn the_time_is_in_seconds_or_in_beats_and_one_side_can_be_late() {
        // 1920 to a beat: 1440 is three steps, 16 800 frames at 150 BPM.
        let beats = Switches { beats: true, ..ON };
        let mut plugin = with(&[beats], &[(at(0, FB_VOL), 0)]);
        plugin.set_tempo(150.0);
        assert_eq!(plugin.units[0].delay, 14_400);
        let [left, _] = click(&mut plugin, [1.0, 1.0], 20_000);
        assert_eq!(left, [(0, 1.0), (14_400, 1.0)]);
        // An offset above 0 holds the right side back, one under 0 the
        // left: 256 is 15.3 ms.
        for (offset, late) in [(256, 1), (-256, 0)] {
            let moves = [(at(0, FB_VOL), 0), (at(0, OFFSET), offset)];
            let mut plugin = with(&[ON], &moves);
            let sides = click(&mut plugin, [1.0, 1.0], 20_000);
            assert_eq!(sides[1 - late], [(0, 1.0), (18_000, 1.0)]);
            assert_eq!(sides[late], [(0, 1.0), (18_000 + 735, 1.0)]);
        }
    }

    #[test]
    fn a_unit_sends_its_output_to_the_next_and_the_wheels_are_knob_curves() {
        // The second unit hears nothing of the input, and all of what
        // the first puts out.
        let moves = [
            (at(0, FB_VOL), 0),
            (at(1, FB_VOL), 0),
            (at(0, NEXT), 128),
            (at(1, IN_VOL), 0),
            (at(1, TIME), 720),
        ];
        let mut plugin = with(&[ON, ON], &moves);
        let [left, _] = click(&mut plugin, [1.0, 1.0], 30_000);
        assert_eq!(left, [(0, 1.0), (18_000, 1.0), (27_000, 1.0)]);
        // Dry off, wet and the output wheel as the knob curve, the input
        // wheel under 0 turned over.
        let half = knob_gain(0.5) as f32;
        let moves = [
            (0, 0),
            (1, 64),
            (at(0, FB_VOL), 0),
            (at(0, IN_VOL), -128),
            (at(0, OUT_VOL), 64),
        ];
        let mut plugin = with(&[ON], &moves);
        let [left, _] = click(&mut plugin, [1.0, 1.0], 20_000);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].0, 18_000);
        assert!((left[0].1 + half * half).abs() < 1e-6, "{}", left[0].1);
        // A pan all the way right takes the left side's echo away.
        let mut plugin = with(&[ON], &[(at(0, FB_VOL), 0), (at(0, OUT_PAN), 128)]);
        let [left, right] = click(&mut plugin, [1.0, 1.0], 20_000);
        assert!(left.len() == 2 && left[1].1.abs() < 1e-6, "{left:?}");
        assert!(right.len() == 2 && right[1].1 > 1.0, "{right:?}");
    }

    #[test]
    fn the_gains_come_up_from_nothing_after_a_state() {
        let mut plugin = DelayBank::default();
        plugin.set_sample_rate(44_100.0);
        plugin.set_state(&saved(&[ON], &[])).unwrap();
        // A thousandth a frame at 44.1 kHz: a block of 512 gets half way.
        plugin.process(&mut [0.0; 512], &mut [0.0; 512]);
        assert!((plugin.units[0].input.current[0] - 0.512).abs() < 1e-6);
        plugin.process(&mut [0.0; 512], &mut [0.0; 512]);
        assert_eq!(plugin.units[0].input.current, [1.0, 1.0, 0.0, 0.0]);
        // A flush leaves them there and empties the line.
        plugin.process(&mut [0.5; 64], &mut [0.5; 64]);
        plugin.reset();
        assert_eq!(plugin.units[0].input.current[0], 1.0);
        assert!(plugin.units[0].line.iter().all(|frame| *frame == [0.0; 2]));
    }

    #[test]
    fn the_grain_is_a_raised_cosine_a_division_of_the_delay_long() {
        let window = window(9);
        assert_eq!(window[0], 0.0);
        assert!((window[4] - 1.0).abs() < 1e-6);
        assert!((window[2] - 0.5).abs() < 1e-6 && (window[6] - 0.5).abs() < 1e-6);
        assert!(window[8].abs() < 1e-6);
        // Four grains to a delay of 18 000 frames.
        let grainy = Switches { grain: 4, ..ON };
        let plugin = with(&[grainy], &[]);
        assert_eq!(plugin.units[0].grain.len(), 4500);
        // A level that stands still comes out of the line in the
        // window's shape.
        let mut plugin = with(&[grainy], &[(0, 0), (at(0, FB_VOL), 0)]);
        let (mut left, mut right) = (vec![1.0; 40_000], vec![1.0; 40_000]);
        plugin.process(&mut left, &mut right);
        let from = 18_000 + 4500 - (18_000 + 1536) % 4500;
        assert!(left[from].abs() < 1e-6, "{}", left[from]);
        assert!((left[from + 2250] - 1.0).abs() < 1e-5);
        assert!((left[from + 1125] - 0.5).abs() < 1e-3);
    }

    #[test]
    fn a_state_with_what_is_not_built_says_so() {
        let filtered = Switches { in_filter: 4, ..ON };
        let mut plugin = with(&[filtered], &[]);
        assert!(
            matches!(plugin.fidelity(), Fidelity::Approximate(text) if text.contains("filters"))
        );
        // The filter of a unit that is off does not count.
        let off = Switches {
            enabled: false,
            ..filtered
        };
        assert_eq!(with(&[ON, off], &[]).fidelity(), Fidelity::Exact);
        let mut bytes = saved(&[ON], &[]);
        bytes[SWITCHES_AT - 4] = 1;
        plugin.set_state(&bytes).unwrap();
        assert!(matches!(plugin.fidelity(), Fidelity::Approximate(text) if text.contains("2x")));
    }

    #[test]
    fn a_state_of_another_version_length_or_range_is_turned_down() {
        let mut plugin = DelayBank::default();
        let mut times: Vec<(usize, i32)> = (1..UNITS).map(|u| (at(u, TIME), 144)).collect();
        times.push((at(0, TIME), 768));
        let good = saved(&[ON], &times);
        assert_eq!(plugin.set_state(&good), Ok(()));
        assert_eq!(plugin.param(at(0, TIME)), 768.0);
        // Version 15 counted the time in tens.
        let mut older = good.clone();
        older[0] = 15;
        assert_eq!(plugin.set_state(&older), Ok(()));
        assert_eq!(plugin.param(at(0, TIME)), 7680.0);
        older[0] = 14;
        assert_eq!(
            plugin.set_state(&older),
            Err(StateError::Version { found: 14 })
        );
        assert_eq!(
            plugin.set_state(&good[..700]),
            Err(StateError::Length {
                expected: 792,
                found: 700
            })
        );
        let wrong = saved(&[ON], &[(at(2, OFFSET), 513)]);
        assert_eq!(
            plugin.set_state(&wrong),
            Err(StateError::Range {
                param: at(2, OFFSET),
                value: 513
            })
        );
        // A wheel moves inside its range, and the last unit's "to next"
        // is no parameter.
        plugin.set_param(at(0, TIME), 9000.0);
        assert_eq!(plugin.param(at(0, TIME)), 7680.0);
        plugin.set_param(131, 5.0);
        assert_eq!(plugin.params[131], 0);
    }

    #[test]
    fn fl_s_presets_load_and_run() {
        let Some(presets) = presets(NAME) else {
            return;
        };
        assert_eq!(presets.len(), 21);
        let mut exact = 0;
        for preset in &presets {
            let mut plugin = DelayBank::default();
            plugin.set_sample_rate(48_000.0);
            plugin.set_tempo(140.0);
            plugin.set_state(&preset.state).unwrap();
            exact += usize::from(plugin.fidelity() == Fidelity::Exact);
            let [left, right] = click(&mut plugin, [0.5, -0.5], 48_000);
            let sound = left.iter().chain(&right);
            assert!(
                sound.clone().all(|(_, v)| v.is_finite() && v.abs() < 8.0),
                "{}",
                preset.file
            );
        }
        assert!(exact > 0 && exact < presets.len(), "{exact}");
        let default = presets.iter().find(|p| p.file == "Default").unwrap();
        let mut plugin = DelayBank::default();
        plugin.set_state(&default.state).unwrap();
        // But for the last unit's "to next", which is no parameter and
        // is -1 in the file.
        assert_eq!(
            plugin.params[..PARAMS],
            DelayBank::default().params[..PARAMS]
        );
        assert_eq!(plugin.params[PARAMS], -1);
        assert!(plugin.units[0].enabled && plugin.units[0].beats && !plugin.units[0].first);
    }
}
