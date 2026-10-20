//! Fruity X-Y Controller: a point on a pad, moved towards where its X and
//! Y are set, whose two coordinates are two internal controller outputs.
//! It has no sound path. `docs/plugins/fruity-x-y-controller.md` reads it
//! from `Fruity X-Y Controller_x64.dll`.
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 X level | 0 to 65536, which is 1.0 | 32768 |
//! | 1 X min | 0 to 65536 | 0 |
//! | 2 X max | 0 to 65536 | 65536 |
//! | 3 X tension | -128 to 128 | 0 |
//! | 4 to 7 | the same for Y | |
//! | 8 Speed | 1024 to 2048 | 1536 |
//! | 9 Acceleration | 256 to 1024 | 1024 |
//!
//! # Ticks
//!
//! [`Effect::tick`] is FL's `NewTick` (`0x6c6d90`). The point moves
//! towards the levels by at most `speed * (1 - acceleration)` a tick, and
//! its velocity carries over by `acceleration`, eased off within 0.3 of
//! where it goes. Speed 2048 is a whole pad a tick, and acceleration 1024
//! carries nothing over, so at both ends of their wheels it jumps. Each
//! coordinate then goes through its tension curve, onto `min..=max` in
//! whole steps of 1/65536, and when that is not what was sent last it is
//! sent as the output: 0 is X and 1 is Y, from 0 to 1.
//!
//! The pad starts where its levels are, as FL's flush (`FPD_Flush`, in the
//! dispatcher at `0x6c5760`) puts it, and the first tick sends both
//! outputs.

use crate::{Effect, StateError, knob_value};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity X-Y Controller";

/// The range of each parameter's wheel, from the editor's form.
const RANGES: [(i32, i32); 10] = [
    (0, 65_536),
    (0, 65_536),
    (0, 65_536),
    (-128, 128),
    (0, 65_536),
    (0, 65_536),
    (0, 65_536),
    (-128, 128),
    (1024, 2048),
    (256, 1024),
];
const DEFAULTS: [i32; 10] = [32_768, 0, 65_536, 0, 32_768, 0, 65_536, 0, 1536, 1024];

/// A level of 1 is 65536: the single FL multiplies by.
const UNIT: f32 = 1.0 / 65_536.0;
/// An output of 1 in the units FL sends, as a single.
const FULL: f32 = 1_073_741_824.0;
/// The editor's speed and acceleration at which the pad jumps.
const JUMP: i32 = 2048;
const NO_CARRY: i32 = 1024;

/// One coordinate of the pad, as FL keeps it at `0x15c + 12 * axis`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Axis {
    /// Where the point is, from 0 to 1.
    at: f32,
    /// Where its level puts it.
    target: f32,
    /// How far it moved in the last tick.
    velocity: f32,
    /// The tension curve's depth, `0x14c`, and its sign, `0x154`.
    depth: f32,
    bend: i32,
    /// What the output was last sent as, in 65536ths; -1 sends it.
    sent: i32,
}

/// Fruity X-Y Controller, as an [`Effect`] that leaves its block alone.
#[derive(Debug, Clone, PartialEq)]
pub struct XyController {
    params: [i32; 10],
    axes: [Axis; 2],
    /// The most the point moves in a tick, `0x178`.
    speed: f32,
    /// How much of its velocity carries over, `0x174`.
    carry: f32,
    ppq: u16,
    outputs: [f32; 2],
    changed: bool,
}

impl Default for XyController {
    fn default() -> Self {
        let mut controller = XyController {
            params: DEFAULTS,
            axes: [Axis::default(); 2],
            speed: 1.0,
            carry: 0.0,
            ppq: 96,
            outputs: [0.0; 2],
            changed: false,
        };
        for index in 0..DEFAULTS.len() {
            controller.update(index);
        }
        controller.reset();
        controller
    }
}

/// Delphi's `Power` as FL's RTL has it (`0x45d280`): an integral exponent
/// multiplies, any other goes through `Exp(e * Ln(b))`.
fn power(base: f64, exponent: f64) -> f64 {
    if exponent.fract() == 0.0 && exponent.abs() <= f64::from(i32::MAX) {
        base.powi(exponent as i32)
    } else {
        (exponent * base.ln()).exp()
    }
}

/// The direction from the point to its target, as FL's `0x6c6cb0` takes it
/// with an arctangent: radians, as a single.
fn heading(dx: f32, dy: f32) -> f32 {
    if dy > 0.0 {
        (-4.71238898038469 - f64::from(dx / dy).atan()) as f32
    } else if dy < 0.0 {
        (f64::from(0.5f32) * -std::f64::consts::PI - f64::from(dx / dy).atan()) as f32
    } else if dx < 0.0 {
        std::f32::consts::PI
    } else {
        0.0
    }
}

impl XyController {
    /// Parameter `index` has changed: FL's `ProcessParam` (`0x6c6650`)
    /// with `REC_UpdateValue`.
    fn update(&mut self, index: usize) {
        let value = self.params[index];
        match index {
            0 | 4 => self.axes[index / 4].target = value as f32 * UNIT,
            3 | 7 => self.tension(index / 4),
            8 | 9 => self.motion(),
            _ => {}
        }
    }

    /// A tension wheel: `0x6c6360`.
    fn tension(&mut self, axis: usize) {
        let value = self.params[axis * 4 + 3];
        let bend = value.signum();
        self.axes[axis].bend = bend;
        if bend != 0 {
            let scaled = f64::from(value as f32 * 0.0078125).abs() as f32;
            let grown = (f64::from(scaled) * 1001f64.ln()).exp();
            self.axes[axis].depth = (grown - 1.0) as f32 * 0.1;
        }
    }

    /// Speed and acceleration: `0x6c6470`, at the song's PPQ.
    fn motion(&mut self) {
        let ppq = f64::from(self.ppq.max(1));
        let speed = self.params[8];
        self.speed = if speed == JUMP {
            1.0
        } else {
            let scaled = (f64::from(speed) * 0.000_434_027_777_777_777_75) as f32;
            let grown = (f64::from(scaled) * 1e8f64.ln()).exp();
            let grown = f64::from((grown - 1.0) as f32);
            ((grown * 1e-8 + 1e-4) * 96.0 / ppq) as f32
        };
        let carry = self.params[9];
        self.carry = if carry == NO_CARRY {
            0.0
        } else {
            let scaled = (f64::from(carry) * 0.000_976_562_5) as f32;
            let grown = (f64::from(scaled) * 1001f64.ln()).exp();
            let grown = f64::from((grown - 1.0) as f32);
            power(1.0 - grown * 0.001, 96.0 / ppq) as f32
        };
    }

    /// Where a coordinate's tension curve takes it.
    fn bent(axis: &Axis) -> f32 {
        let at = axis.at;
        let depth = axis.depth;
        let log = || f64::from(depth + 1.0).ln();
        match axis.bend {
            0 => at,
            bend if bend > 0 => {
                let grown = (f64::from(1.0 - at) * log()).exp();
                1.0 - (grown - 1.0) as f32 / depth
            }
            _ => ((f64::from(at) * log()).exp() - 1.0) as f32 / depth,
        }
    }
}

/// Delphi's `Round`, `cvtsd2si` with the rounding SSE has by default.
fn round(value: f64) -> i64 {
    value.round_ties_even() as i64
}

impl Effect for XyController {
    /// The state FL's `SaveRestoreState` (`0x6c5d20`) writes: a version, the
    /// ten parameters, the axis the editor shows, the grid's name, the grid
    /// (a byte before version 2) and five words for the joystick. Only the
    /// parameters play.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let short = |expected| StateError::Length {
            expected,
            found: bytes.len(),
        };
        let Some((version, rest)) = bytes.split_first_chunk::<4>() else {
            return Err(short(4));
        };
        let version = u32::from_le_bytes(*version);
        let Some((params, rest)) = rest.split_first_chunk::<40>() else {
            return Err(short(44));
        };
        // The axis byte and the grid name's length byte.
        let name = rest.get(1).map_or(0, |len| usize::from(*len));
        let grid = if version < 2 { 1 } else { 4 };
        let expected = 44 + 2 + name + grid + 20;
        if bytes.len() != expected {
            return Err(short(expected));
        }
        for (param, word) in self.params.iter_mut().zip(params.as_chunks::<4>().0) {
            *param = i32::from_le_bytes(*word);
        }
        for index in 0..self.params.len() {
            self.update(index);
        }
        self.reset();
        Ok(())
    }

    fn set_sample_rate(&mut self, _hz: f32) {}

    /// FL tells the plugin the PPQ with the time signature, and the speed
    /// and the acceleration are set again for it.
    fn set_ppq(&mut self, ppq: u16) {
        self.ppq = ppq;
        self.motion();
    }

    fn tick(&mut self, _ppq_tick: u64, _tempo: f64) {
        let [x, y] = &self.axes;
        let dx = x.target - x.at;
        let dy = y.target - y.at;
        let (sin, cos) = f64::from(heading(dx, dy)).sin_cos();
        let direction = [cos as f32, sin as f32];
        let distance = f64::from(dx * dx + dy * dy).sqrt() as f32;
        let mut carry = self.carry;
        if distance < 0.3 {
            let near = (1.0 - f64::from(distance) * 3.333_333_253_860_473_6) as f32;
            let eased = ((f64::from(near) * 1e10f64.ln()).exp() - 1.0) as f32;
            carry = (f64::from(carry) * (1.0 - f64::from(eased) * 1e-10)) as f32;
        }
        let step = (self.speed * (1.0 - self.carry)).min(distance);
        for (i, axis) in self.axes.iter_mut().enumerate() {
            axis.velocity = axis.velocity * carry + direction[i] * step;
            axis.at += axis.velocity;
            let (low, high) = (self.params[4 * i + 1], self.params[4 * i + 2]);
            let steps = round(f64::from(Self::bent(axis) * (high - low) as f32));
            let value = (i64::from(low) + steps).max(i64::from(low));
            let value = value.min(i64::from(high)) as i32;
            if axis.sent != value {
                let sent = round(f64::from(value as f32 * UNIT * FULL));
                self.outputs[i] = (sent as f64 / f64::from(FULL)) as f32;
                axis.sent = value;
                self.changed = true;
            }
        }
    }

    fn controller_outputs(&self) -> &[f32] {
        &self.outputs
    }

    fn controller_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    /// FL never asks the plugin for sound, so the block stays as it came.
    fn process(&mut self, _left: &mut [f32], _right: &mut [f32]) {}

    fn param_count(&self) -> usize {
        RANGES.len()
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |value| *value as f32)
    }

    /// A level moves where the point goes, and the point gets there on the
    /// ticks that follow.
    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(range) = RANGES.get(index) {
            self.params[index] = knob_value(value, *range);
            self.update(index);
        }
    }

    /// FL's flush: the point is put where its levels are, and the next
    /// tick sends both outputs whatever they are.
    fn reset(&mut self) {
        for axis in &mut self.axes {
            axis.at = axis.target;
            axis.velocity = 0.0;
            axis.sent = -1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A state as FL 26 saves one, with the grid named "Default".
    fn state(params: [i32; 10]) -> Vec<u8> {
        let mut bytes = 2u32.to_le_bytes().to_vec();
        bytes.extend(params.iter().flat_map(|p| p.to_le_bytes()));
        bytes.push(0);
        bytes.push(7);
        bytes.extend(b"Default");
        bytes.extend((-1i32).to_le_bytes());
        bytes.extend([0, 0, 1, 0, 70].iter().flat_map(|w: &i32| w.to_le_bytes()));
        bytes
    }

    fn ticked(plugin: &mut XyController) -> [f32; 2] {
        plugin.tick(0, 120.0);
        plugin.outputs
    }

    #[test]
    fn a_loaded_pad_sends_its_levels_on_the_first_tick() {
        let mut plugin = XyController::default();
        // Drumloop Masher's "Pitch".
        let pitch = [32_768, 0, 65_536, 0, 0, 0, 65_536, 0, 2048, 1024];
        plugin.set_state(&state(pitch)).unwrap();
        assert!(!plugin.controller_changed());
        assert_eq!(ticked(&mut plugin), [0.5, 0.0]);
        assert!(plugin.controller_changed());
        // Nothing moves after that.
        assert_eq!(ticked(&mut plugin), [0.5, 0.0]);
        assert!(!plugin.controller_changed());
        // A flush sends them again.
        plugin.reset();
        ticked(&mut plugin);
        assert!(plugin.controller_changed());
    }

    #[test]
    fn min_max_and_tension_shape_the_outputs() {
        let mut plugin = XyController::default();
        let params = [16_384, 16_384, 49_152, 0, 65_536, 0, 65_536, 64, 2048, 1024];
        plugin.set_state(&state(params)).unwrap();
        let [x, y] = ticked(&mut plugin);
        // A quarter of the way from a quarter to three quarters.
        assert_eq!(x, (16_384.0 + 8192.0) / 65_536.0);
        // The ends stay where they are whatever the tension.
        assert_eq!(y, 1.0);
        // Half way up with the curve bent upwards, `1 - (b^(1-x) - 1) / (b - 1)`.
        plugin.set_param(4, 32_768.0);
        let [_, y] = ticked(&mut plugin);
        let depth = ((1001f64.powf(0.5) - 1.0) * 0.1) as f32;
        let curve = 1.0 - ((1.0 + depth as f64).powf(0.5) - 1.0) / depth as f64;
        assert_eq!(y, (curve * 65_536.0).round() as f32 / 65_536.0);
    }

    #[test]
    fn at_full_speed_the_pad_jumps_and_slower_it_glides() {
        let mut plugin = XyController::default();
        plugin.set_state(&state(DEFAULTS_JUMP)).unwrap();
        ticked(&mut plugin);
        plugin.set_param(0, 65_536.0);
        assert_eq!(ticked(&mut plugin), [1.0, 0.5]);

        // Touchscreen's "Low Pass" at speed 1900: about 0.0397 of the pad a
        // tick at 96 PPQ, at a steady pace as acceleration carries nothing.
        plugin.set_param(8, 1900.0);
        plugin.set_param(0, 0.0);
        let step = ((1e8f64.powf(1900.0 / 2304.0) - 1.0) * 1e-8 + 1e-4) as f32;
        assert!((plugin.speed - step).abs() < 1e-6, "{}", plugin.speed);
        let mut ticks = 0;
        while ticked(&mut plugin)[0] > 0.0 {
            ticks += 1;
        }
        assert_eq!(ticks, (1.0 / step).floor() as i32);
        // At twice the PPQ, half as far a tick.
        plugin.set_ppq(192);
        assert!((plugin.speed - step / 2.0).abs() < 1e-6);
    }

    const DEFAULTS_JUMP: [i32; 10] = [32_768, 0, 65_536, 0, 32_768, 0, 65_536, 0, 2048, 1024];

    #[test]
    fn acceleration_carries_the_velocity_over() {
        let mut plugin = XyController::default();
        let mut params = DEFAULTS_JUMP;
        params[8] = 1536;
        params[9] = 512;
        plugin.set_state(&state(params)).unwrap();
        // Power(1 - (1001^0.5 - 1) / 1000, 1).
        let carry = 1.0 - ((1001f64.powf(0.5) - 1.0) as f32 as f64) * 0.001;
        assert!((f64::from(plugin.carry) - carry).abs() < 1e-6);
        ticked(&mut plugin);
        plugin.set_param(4, 65_536.0);
        let mut last = 0.5;
        let mut steps = Vec::new();
        for _ in 0..2000 {
            let [_, y] = ticked(&mut plugin);
            steps.push(y - last);
            last = y;
        }
        assert_eq!(last, 1.0);
        // It speeds up before it slows down.
        assert!(steps[1] > steps[0]);
    }

    #[test]
    fn parameters_stay_inside_their_wheels() {
        let mut plugin = XyController::default();
        assert_eq!(plugin.param_count(), 10);
        for (index, (low, high)) in RANGES.iter().enumerate() {
            assert_eq!(plugin.param(index), DEFAULTS[index] as f32);
            plugin.set_param(index, f32::MIN);
            assert_eq!(plugin.param(index), *low as f32);
            plugin.set_param(index, f32::MAX);
            assert_eq!(plugin.param(index), *high as f32);
        }
        assert_eq!(plugin.param(10), 0.0);
    }

    #[test]
    fn a_state_of_another_length_is_refused() {
        let mut plugin = XyController::default();
        let mut bytes = state(DEFAULTS);
        bytes.push(0);
        assert!(plugin.set_state(&bytes).is_err());
        assert!(plugin.set_state(&bytes[..30]).is_err());
        // Before version 2 the grid is a byte.
        let mut old = state(DEFAULTS);
        old[0] = 1;
        old.drain(54..57);
        assert!(plugin.set_state(&old).is_ok());
    }

    #[test]
    fn every_state_in_fl_s_projects_loads() {
        let Some(root) = crate::testing::corpus() else {
            return;
        };
        let saved = crate::testing::projects(&root);
        let mine = saved.iter().filter(|s| s.plugin.eq_ignore_ascii_case(NAME));
        let mut count = 0;
        for saved in mine {
            let mut plugin = XyController::default();
            plugin.set_state(&saved.state).unwrap();
            plugin.tick(0, 120.0);
            assert!(plugin.controller_changed(), "{}", saved.file);
            count += 1;
        }
        assert_eq!(count, 19);
    }
}
