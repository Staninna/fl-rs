//! Fruity X-Y-Z Controller: three values, each put through a map, eased
//! towards where it is set and put through a second map, which are three
//! internal controller outputs. It has no sound path.
//! `docs/plugins/fruity-x-y-z-controller.md` reads it from
//! `Fruity X-Y-Z Controller_x64.dll`.
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0, 1, 2 X, Y, Z value | 0 to 65536, which is 1.0 | 32768 |
//! | 3, 4, 5 smoothing of X, Y, Z | 0 to 4096 | 2048 |
//! | 6, 7, 8 smoothing order of X, Y, Z | 0 to 3072, which is 3 | 3072 |
//! | 9 global smoothing speed | 0 to 4096 | 2048 |
//! | 10 external controller speed | 0 to 4096 | 1024 |
//!
//! # Ticks
//!
//! A value goes through its axis's first map when it is set, and that is
//! where the axis goes (`0x83c0a0`). [`Effect::tick`] is FL's `NewTick`
//! (`0x83cbf0`): each axis moves there through up to three one-pole stages
//! (`0x604ff0`), goes through its second map, and the output it had after
//! the tick before is sent when it is not what was sent last. So a change
//! reaches the outputs one tick after the tick that makes it.
//!
//! A loaded state starts with every axis where it is set and sends every
//! output on the first tick, as FL's would after ticking while the song is
//! stopped.

use crate::menvelope::{Class, MEnvelope, Reader};
use crate::{Effect, Fidelity, StateError, knob_value};
use dsp::curve::{Curve, Point};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity X-Y-Z Controller";

/// The range of each parameter's control, from the editor's form.
const RANGES: [(i32, i32); 11] = [
    (0, 65_536),
    (0, 65_536),
    (0, 65_536),
    (0, 4096),
    (0, 4096),
    (0, 4096),
    (0, 3072),
    (0, 3072),
    (0, 3072),
    (0, 4096),
    (0, 4096),
];
const DEFAULTS: [i32; 11] = [
    32_768, 32_768, 32_768, 2048, 2048, 2048, 3072, 3072, 3072, 2048, 1024,
];

/// The newest state `SaveRestoreState` (`0x83b520`) reads.
const VERSION: i32 = 3;
/// The kind of a `TMMap`, the class of the maps.
const MAP_KIND: u32 = 3;

/// A value of 1 is 65536, as the single FL multiplies by.
const UNIT: f32 = 1.0 / 65_536.0;
/// An output of 1 in the units FL sends, as a single.
const FULL: f32 = 1_073_741_824.0;
/// How near an axis has to come to where it goes for the smoothing to be
/// put there (`0x83cd48`).
const NEAR: f64 = 1.525_878_906_25e-6;
/// Ln(0.01): a stage's coefficient is `Exp(Ln(0.01) / ticks)`.
const LN_HUNDREDTH: f64 = -4.605_170_185_988_091;

/// What [`Effect::fidelity`] says.
const JOYSTICK: &str = "X-Y-Z Controller's joystick input is left out";
const ODD_MAP: &str = "X-Y-Z Controller has a map FL would mend first";

/// Delphi's `Round`, `cvtsd2si` with the rounding SSE has by default.
fn round(value: f64) -> i64 {
    value.round_ties_even() as i64
}

/// A `TMMap` as `0x77e970` loads it and `0x787ac0` makes it ready: the
/// points as singles, their shapes, and the table FL looks a straight or
/// flat map up in.
#[derive(Debug, Clone, PartialEq)]
struct Map {
    /// How far each point is after the one before it, and its value.
    lengths: Vec<f32>,
    values: Vec<f32>,
    /// The segments' shapes.
    curve: Curve,
    table: Table,
    /// FL would add or move a point first, which this does not.
    odd: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Table {
    /// No table: every value is worked out on the points (`0x7858b0`).
    None,
    /// Every point at one value.
    Flat(f32),
    /// Two points and a straight line, looked up from both ends.
    Line(f32, f32),
}

impl Map {
    /// A straight line from 0 to 1, as the constructor (`0x83b8d0`) makes
    /// both maps of every axis.
    fn new() -> Self {
        Map::from_points(&[Point::new(0.0, 0.0), Point::new(1.0, 1.0)])
    }

    /// A map from what a stream holds, which must be a map.
    fn read(reader: &mut Reader<'_>) -> Result<Self, StateError> {
        let envelope = MEnvelope::read(reader, Class::Map)?;
        // FL's loader reads no further than the kind of another class, and
        // the state after it would be out of step.
        if envelope.kind != MAP_KIND {
            return Err(StateError::Version {
                found: envelope.kind as i32,
            });
        }
        Ok(Map::from_points(&envelope.points))
    }

    fn from_points(points: &[Point]) -> Self {
        // Kept as singles; a value that is not a number is the one before
        // it.
        let mut lengths: Vec<f32> = points.iter().map(|p| p.x as f32).collect();
        let mut values: Vec<f32> = Vec::with_capacity(points.len());
        for point in points {
            let before = values.last().copied().unwrap_or(0.0);
            let y = point.y as f32;
            values.push(if y.is_nan() { before } else { y });
        }
        // `0x781c10` makes a map one long: it puts a point at 0 before a
        // first one that is not there, and scales the lengths to a sum of 1.
        let odd = lengths.first().is_none_or(|x| *x > 0.0);
        let total: f64 = lengths.iter().map(|x| f64::from(*x)).sum();
        if total > 0.0 {
            let by = 1.0 / total;
            if by != 1.0 {
                for x in &mut lengths {
                    *x = (f64::from(*x) * by) as f32;
                }
            }
        }
        // And `0x782750` reads a length under 0 as 0.
        for x in &mut lengths {
            if *x < 0.0 {
                *x = 0.0;
            }
        }
        let shaped: Vec<Point> = points
            .iter()
            .zip(lengths.iter().zip(&values))
            .map(|(p, (x, y))| Point {
                x: f64::from(*x),
                y: f64::from(*y),
                ..*p
            })
            .collect();
        let flat = values.first().filter(|first| {
            let bits = first.to_bits();
            values.iter().all(|v| v.to_bits() == bits)
        });
        // Modes 0, 1, 7, 8, 9, 11 and 12 with no tension are straight
        // (`0x787f40`, the mask 0x1b83).
        let straight =
            |p: &Point| p.mode < 16 && (0x1b83 >> p.mode) & 1 == 1 && p.tension.to_bits() == 0;
        let table = match (flat, points) {
            (Some(level), _) => Table::Flat(*level),
            (None, [_, end]) if straight(end) => Table::Line(values[0], values[1]),
            _ => Table::None,
        };
        Map {
            curve: Curve::new(&shaped),
            lengths,
            values,
            table,
            odd: odd || total <= 0.0,
        }
    }

    /// The map at `x`, as `0x787da0` reads it.
    fn at(&self, x: f64) -> f32 {
        match self.table {
            Table::Flat(level) => level,
            Table::Line(from, to) => {
                let i = round(x - 0.5) as i32;
                if i < 0 {
                    from
                } else if i < 1 {
                    let d = x - f64::from(i);
                    (f64::from(from) * (1.0 - d) + f64::from(to) * d) as f32
                } else {
                    to
                }
            }
            Table::None => self.run(x),
        }
    }

    /// A run started at the first point (`0x784970`) and moved on by `x`
    /// (`0x7858b0`), segment by segment as FL steps it.
    fn run(&self, x: f64) -> f32 {
        let last = self.values.len().saturating_sub(1);
        let Some(&first) = self.lengths.first() else {
            return 0.0;
        };
        let mut at = 0;
        let (mut pos, mut rate) = if first == 0.0 {
            (1.0, 1.0)
        } else {
            (0.0, 1.0 / f64::from(first))
        };
        let mut from = self.values[0];
        let mut rise = 0.0f32;
        pos += x * rate;
        // FL tests the high word: 1 and on, and anything under 0.
        while (pos.to_bits() >> 32) as u32 > 0x3fef_ffff {
            let before = at;
            pos = if rate == 0.0 { x } else { (pos - 1.0) / rate };
            from = self.values[at];
            if at < last {
                at += 1;
            }
            rise = (f64::from(self.values[at]) - f64::from(from)) as f32;
            if before == at {
                pos = 0.0;
                rate = 0.0;
            } else {
                let length = f64::from(self.lengths[at]);
                if length <= 0.0 {
                    rate = 1.0;
                    pos += 1.0;
                } else {
                    rate = 1.0 / length;
                    pos *= rate;
                }
            }
        }
        let shape = self.curve.progress(at, pos) as f32;
        from + shape * rise
    }
}

/// Up to three one-pole stages in a row, FL's smoother at `0x604a90` and
/// `0x604ff0`, set up in its mode 4 (which runs as its mode 2).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Smoother {
    coefficients: [f64; 3],
    /// Off: the value passes as it is.
    off: bool,
    /// Each stage's level, relative to `base`.
    stages: [f64; 3],
    base: f64,
}

impl Smoother {
    /// `ticks` for the value to come within 1 % of where it goes, spread
    /// over `order` stages: whole stages and a last one for the fraction.
    fn set(&mut self, ticks: f64, order: f64) {
        // FL's `ucomisd` takes a count that is not a number as above 0.
        if ticks <= 0.0 {
            self.off = true;
            return;
        }
        self.off = false;
        let whole = round(order + 0.499_999_999_999_999_94) as i32;
        let mut ticks = ticks / order.sqrt();
        for (k, coefficient) in self.coefficients.iter_mut().enumerate() {
            let k = k as i32;
            if k >= whole - 1 {
                if k < whole {
                    ticks *= 1.0 - (f64::from(whole) - order);
                } else {
                    ticks = 0.0;
                }
            }
            *coefficient = if ticks <= 0.0 {
                0.0
            } else {
                (LN_HUNDREDTH / ticks).exp()
            };
        }
    }

    fn step(&mut self, to: f64) -> f64 {
        if self.off {
            return to;
        }
        let [c0, c1, c2] = self.coefficients;
        let [s0, s1, s2] = &mut self.stages;
        let rel = to - self.base;
        *s0 = rel + (*s0 - rel) * c0;
        *s1 = *s0 + (*s1 - *s0) * c1;
        *s2 = *s1 + (*s2 - *s1) * c2;
        self.base + *s2
    }

    /// Put the value at `at` with nothing on the way (`0x604fd0`).
    fn put(&mut self, at: f64) {
        self.base = at;
        self.stages = [0.0; 3];
    }
}

/// One axis as FL keeps it at `0x14c + 0x88 * axis`.
#[derive(Debug, Clone, PartialEq)]
struct Axis {
    /// The map a value goes through when it is set, `0x1c4`, and the one
    /// the eased value goes through, `0x1cc`.
    input: Map,
    output: Map,
    /// Where the axis goes, `0x150`, and its output, `0x14c`.
    target: f32,
    out: f32,
    /// What was sent last, in 2^30ths; -1 sends the next whatever it is.
    sent: i64,
    smoother: Smoother,
    /// The joystick the axis follows, 0 for none, and its mode (`0x160`).
    joystick: [i32; 2],
}

impl Axis {
    fn new() -> Self {
        Axis {
            input: Map::new(),
            output: Map::new(),
            target: 0.0,
            out: 0.0,
            sent: -1,
            smoother: Smoother::default(),
            joystick: [0; 2],
        }
    }

    /// Where it goes, at once.
    fn settle(&mut self) {
        self.smoother.put(f64::from(self.target));
        self.out = self.output.at(f64::from(self.target));
        self.sent = -1;
    }
}

/// Fruity X-Y-Z Controller, as an [`Effect`] that leaves its block alone.
#[derive(Debug, Clone, PartialEq)]
pub struct XyzController {
    params: [i32; 11],
    axes: [Axis; 3],
    /// The song's PPQ over 96 (`0x2f0`).
    tick_scale: f64,
    outputs: [f32; 3],
    changed: bool,
}

impl Default for XyzController {
    fn default() -> Self {
        let mut controller = XyzController {
            params: DEFAULTS,
            axes: [Axis::new(), Axis::new(), Axis::new()],
            tick_scale: 1.0,
            outputs: [0.0; 3],
            changed: false,
        };
        controller.update_all();
        for axis in &mut controller.axes {
            axis.settle();
        }
        controller
    }
}

impl XyzController {
    /// Parameter `index` has changed: FL's `ProcessParam` (`0x83c0a0`)
    /// with `REC_UpdateValue`.
    fn update(&mut self, index: usize) {
        match index {
            0..=2 => {
                let axis = &mut self.axes[index];
                let x = self.params[index] as f32 * UNIT;
                axis.target = axis.input.at(f64::from(x));
            }
            3..=9 => self.smoothing(),
            // The joystick's speed.
            _ => {}
        }
    }

    fn update_all(&mut self) {
        for index in 0..self.params.len() {
            self.update(index);
        }
    }

    /// The smoothers of all three axes (`0x83c400`).
    fn smoothing(&mut self) {
        let global = self.params[9] as f32 * 0.000_244_140_62;
        let global = (f64::from(global) * 17f64.ln()).exp();
        let ln3 = 3f64.ln() as f32;
        for (i, axis) in self.axes.iter_mut().enumerate() {
            let speed = self.params[3 + i] as f32 * 0.000_488_281_25;
            let speed = if speed == 1.0 {
                1.0
            } else {
                let grown = f64::from(speed * ln3).exp();
                ((grown - 1.0) * 0.5) as f32
            };
            let ticks =
                f64::from((global - 1.0) as f32) * 24.0 * self.tick_scale * f64::from(speed);
            let order = self.params[6 + i] as f32 * 0.000_976_562_5;
            axis.smoother.set(ticks, f64::from(order));
        }
    }

    /// The state after the version, or why it is turned down.
    fn read(&self, bytes: &[u8]) -> Result<([i32; 11], [Axis; 3]), StateError> {
        let mut reader = Reader::new(bytes);
        let version = reader.i32()?;
        // FL reads nothing of a newer state.
        if version > VERSION {
            return Err(StateError::Version { found: version });
        }
        let mut params = [0; 11];
        for param in &mut params {
            *param = reader.i32()?;
        }
        // The editor's buttons and the display colour.
        reader.bytes(8)?;
        let mut axes = [Axis::new(), Axis::new(), Axis::new()];
        for axis in &mut axes {
            axis.joystick = [reader.i32()?, reader.i32()?];
            axis.input = Map::read(&mut reader)?;
            axis.output = Map::read(&mut reader)?;
        }
        reader.end()?;
        Ok((params, axes))
    }
}

impl Effect for XyzController {
    /// The state FL's `SaveRestoreState` (`0x83b520`) writes: a version,
    /// the eleven parameters, the editor's switches and colour, and for
    /// each axis its joystick and its two maps.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let (params, axes) = self.read(bytes)?;
        self.params = params;
        self.axes = axes;
        self.update_all();
        for axis in &mut self.axes {
            axis.settle();
        }
        Ok(())
    }

    fn set_sample_rate(&mut self, _hz: f32) {}

    /// FL tells the plugin the PPQ with the time signature, and the
    /// smoothing is set again for it.
    fn set_ppq(&mut self, ppq: u16) {
        self.tick_scale = f64::from(ppq) / 96.0;
        self.smoothing();
    }

    fn tick(&mut self, _ppq_tick: u64, _tempo: f64) {
        for (i, axis) in self.axes.iter_mut().enumerate() {
            let before = axis.out;
            let target = f64::from(axis.target);
            let eased = axis.smoother.step(target) as f32;
            if f64::from(eased - axis.target).abs() < NEAR {
                axis.smoother.put(target);
            }
            axis.out = axis.output.at(f64::from(eased));
            let send = round(f64::from(before * FULL)).clamp(0, 1 << 30);
            if axis.sent != send {
                self.outputs[i] = (send as f64 / f64::from(FULL)) as f32;
                axis.sent = send;
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

    /// A value moves where its axis goes, and the axis gets there on the
    /// ticks that follow.
    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(range) = RANGES.get(index) {
            self.params[index] = knob_value(value, *range);
            self.update(index);
        }
    }

    /// FL's flush (`FPD_Flush` in the dispatcher at `0x83b0c0`): every
    /// axis's smoothing is put where it goes.
    fn reset(&mut self) {
        for axis in &mut self.axes {
            axis.smoother.put(f64::from(axis.target));
        }
    }

    fn fidelity(&self) -> Fidelity {
        let mut maps = self.axes.iter().flat_map(|a| [&a.input, &a.output]);
        if self.axes.iter().any(|axis| axis.joystick[0] > 0) {
            Fidelity::Approximate(JOYSTICK)
        } else if maps.any(|map| map.odd) {
            Fidelity::Approximate(ODD_MAP)
        } else {
            Fidelity::Exact
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A map's stream: kind 3, version 3, each point's length, value,
    /// tension and mode, and the 20 bytes after them.
    fn map(points: &[(f64, f64, f32, u8)]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for word in [MAP_KIND, 3, points.len() as u32] {
            bytes.extend(word.to_le_bytes());
        }
        for &(x, y, tension, mode) in points {
            bytes.extend(x.to_le_bytes());
            bytes.extend(y.to_le_bytes());
            bytes.extend(tension.to_le_bytes());
            bytes.extend([mode, 0, 0, 2]);
        }
        bytes.extend(0u32.to_le_bytes());
        for word in [-1i32; 4] {
            bytes.extend(word.to_le_bytes());
        }
        bytes
    }

    const LINE: [(f64, f64, f32, u8); 2] = [(0.0, 0.0, 0.0, 0), (1.0, 1.0, 0.0, 0)];

    /// A state as FL 26 saves one, with these maps on every axis.
    fn state(params: [i32; 11], input: &[u8], output: &[u8]) -> Vec<u8> {
        let mut bytes = 3i32.to_le_bytes().to_vec();
        bytes.extend(params.iter().flat_map(|p| p.to_le_bytes()));
        bytes.extend(6i32.to_le_bytes());
        bytes.extend(144i32.to_le_bytes());
        for _ in 0..3 {
            bytes.extend([0; 8]);
            bytes.extend(input);
            bytes.extend(output);
        }
        bytes
    }

    /// No smoothing: the global speed at 0.
    const PLAIN: [i32; 11] = [32_768, 32_768, 32_768, 0, 0, 0, 0, 0, 0, 0, 0];

    fn ticked(plugin: &mut XyzController) -> [f32; 3] {
        plugin.tick(0, 120.0);
        plugin.outputs
    }

    /// What FL sends for a value, back from 0 to 1.
    fn sent(value: f32) -> f32 {
        (round(f64::from(value * FULL)) as f64 / f64::from(FULL)) as f32
    }

    #[test]
    fn a_loaded_state_sends_every_output_on_the_first_tick() {
        let mut plugin = XyzController::default();
        // Funk's "bypMapper": X through a line from 0 to 0.8.
        let scaled = map(&[(0.0, 0.0, 0.0, 0), (1.0, 0.800_003_051_757_812_5, 0.0, 0)]);
        let mut params = PLAIN;
        params[..3].copy_from_slice(&[65_536, 0, 32_768]);
        plugin
            .set_state(&state(params, &scaled, &map(&LINE)))
            .unwrap();
        assert_eq!(plugin.fidelity(), Fidelity::Exact);
        assert!(!plugin.controller_changed());
        let outputs = ticked(&mut plugin);
        assert!(plugin.controller_changed());
        // Z's 0.5 goes through the same line.
        let half = (0.800_003_05f32 as f64 * 0.5) as f32;
        assert_eq!(outputs, [sent(0.800_003_05), 0.0, sent(half)]);
        // Nothing moves after that.
        assert_eq!(ticked(&mut plugin), outputs);
        assert!(!plugin.controller_changed());
    }

    #[test]
    fn a_change_reaches_the_output_a_tick_later() {
        let mut plugin = XyzController::default();
        plugin
            .set_state(&state(PLAIN, &map(&LINE), &map(&LINE)))
            .unwrap();
        assert_eq!(ticked(&mut plugin), [0.5; 3]);
        assert!(plugin.controller_changed());
        plugin.set_param(1, 65_536.0);
        // The tick works the new output out and sends the one before.
        assert_eq!(ticked(&mut plugin), [0.5; 3]);
        assert!(!plugin.controller_changed());
        assert_eq!(ticked(&mut plugin), [0.5, 1.0, 0.5]);
        assert!(plugin.controller_changed());
    }

    #[test]
    fn a_map_of_more_points_follows_their_curve() {
        // Stroll Up's "mapper": down from 1 to 0.5 over the first half,
        // then level, and an output map that bends.
        let input = map(&[
            (0.0, 1.0, 0.0, 0),
            (0.500_000_119_209_289_6, 0.5, 0.0, 0),
            (0.499_999_910_593_032_84, 0.5, 0.0, 0),
        ]);
        let output = map(&[(0.0, 0.0, 0.0, 0), (1.0, 1.0, 0.42, 0)]);
        let mut plugin = XyzController::default();
        let mut params = PLAIN;
        params[0] = 16_384;
        plugin.set_state(&state(params, &input, &output)).unwrap();
        let x = plugin.axes[0].target;
        assert!((x - 0.75).abs() < 1e-6, "{x}");
        // Just short of the second point, as its length is a little over
        // a half.
        let z = plugin.axes[2].target;
        assert!(z > 0.5 && z - 0.5 < 1e-6, "{z}");
        // The bent output, as the shared curve has it.
        let curve = Curve::new(&[Point::new(0.0, 0.0), {
            let mut end = Point::new(1.0, 1.0);
            end.tension = 0.42;
            end
        }]);
        let [out, _, _] = ticked(&mut plugin);
        let expected = curve.value_at(f64::from(x)) as f32;
        assert!((out - expected).abs() < 1e-6, "{out} {expected}");
        assert!((out - x).abs() > 0.01);
    }

    #[test]
    fn a_flat_map_holds_its_value_and_a_straight_one_is_looked_up() {
        let flat = Map::from_points(&[Point::new(0.0, 0.25), Point::new(1.0, 0.25)]);
        assert_eq!(flat.table, Table::Flat(0.25));
        assert_eq!(flat.at(0.9), 0.25);
        let line = Map::from_points(&[Point::new(0.0, 0.2), Point::new(1.0, 0.6)]);
        assert_eq!(line.table, Table::Line(0.2, 0.6));
        assert_eq!(
            line.at(0.5),
            (0.2f32 as f64 * 0.5 + 0.6f32 as f64 * 0.5) as f32
        );
        assert_eq!(line.at(-1.0), 0.2);
        assert_eq!(line.at(2.0), 0.6);
        // A bend leaves the table out, and the run gives the same ends.
        let mut bent = Point::new(1.0, 0.6);
        bent.tension = -0.3;
        let bent = Map::from_points(&[Point::new(0.0, 0.2), bent]);
        assert_eq!(bent.table, Table::None);
        assert_eq!(bent.at(0.0), 0.2);
        assert_eq!(bent.at(1.0), 0.6);
        // A map is made one long.
        let long = Map::from_points(&[Point::new(0.0, 0.0), Point::new(2.0, 1.0)]);
        assert_eq!(long.lengths, [0.0, 1.0]);
        assert!(!long.odd);
    }

    #[test]
    fn smoothing_eases_through_its_stages() {
        // Global speed 2048 and one whole stage: `(17^0.5 - 1) * 24`
        // ticks to come within 1 %.
        let mut params = PLAIN;
        params[9] = 2048;
        params[3] = 2048;
        params[6] = 1024;
        let mut plugin = XyzController::default();
        plugin
            .set_state(&state(params, &map(&LINE), &map(&LINE)))
            .unwrap();
        let ticks = f64::from((17f64.sqrt() - 1.0) as f32) * 24.0;
        let smoother = plugin.axes[0].smoother;
        assert!(!smoother.off);
        let c = (LN_HUNDREDTH / ticks).exp();
        assert!((smoother.coefficients[0] - c).abs() < 1e-12);
        assert_eq!(smoother.coefficients[1..], [0.0, 0.0]);
        // Y and Z do not ease: their smoothing is 0.
        assert!(plugin.axes[1].smoother.off);
        ticked(&mut plugin);
        plugin.set_param(0, 65_536.0);
        let mut x = Vec::new();
        for _ in 0..300 {
            x.push(ticked(&mut plugin)[0]);
        }
        // One tick late, then a step of half the way times (1 - c).
        assert_eq!(x[0], 0.5);
        assert_eq!(x[1], sent((0.5 + 0.5 * (1.0 - c)) as f32));
        let n = ticks.ceil() as usize;
        assert!(x[n] > 0.995 && x[n - 2] < 0.995, "{}", x[n]);
        // Within 2^-16 / 10 of it the smoothing is put there.
        assert!(x[199] < 1.0);
        assert_eq!(x[299], 1.0);
        // At 192 PPQ it takes twice the ticks.
        plugin.set_ppq(192);
        let slower = (LN_HUNDREDTH / (ticks * 2.0)).exp();
        assert!((plugin.axes[0].smoother.coefficients[0] - slower).abs() < 1e-12);
    }

    #[test]
    fn three_stages_start_slower() {
        let mut params = PLAIN;
        params[9] = 2048;
        params[3] = 2048;
        params[6] = 3072;
        let mut plugin = XyzController::default();
        plugin
            .set_state(&state(params, &map(&LINE), &map(&LINE)))
            .unwrap();
        let coefficients = plugin.axes[0].smoother.coefficients;
        assert!(coefficients.iter().all(|c| *c > 0.0 && *c < 1.0));
        ticked(&mut plugin);
        plugin.set_param(0, 65_536.0);
        let x: Vec<f32> = (0..400).map(|_| ticked(&mut plugin)[0]).collect();
        // It gathers speed and then slows down.
        let steps: Vec<f32> = x.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(steps[2] > steps[1]);
        assert!(steps[100] < steps[10]);
        assert_eq!(x[399], 1.0);
        // A flush puts it where it goes.
        plugin.set_param(0, 0.0);
        ticked(&mut plugin);
        plugin.reset();
        ticked(&mut plugin);
        assert_eq!(ticked(&mut plugin)[0], 0.0);
    }

    #[test]
    fn parameters_stay_inside_their_controls() {
        let mut plugin = XyzController::default();
        assert_eq!(plugin.param_count(), 11);
        for (index, (low, high)) in RANGES.iter().enumerate() {
            assert_eq!(plugin.param(index), DEFAULTS[index] as f32);
            plugin.set_param(index, f32::MIN);
            assert_eq!(plugin.param(index), *low as f32);
            plugin.set_param(index, f32::MAX);
            assert_eq!(plugin.param(index), *high as f32);
        }
        assert_eq!(plugin.param(11), 0.0);
        // A new one sends its three outputs on the first tick.
        let mut plugin = XyzController::default();
        assert_eq!(ticked(&mut plugin), [0.5; 3]);
    }

    #[test]
    fn a_state_of_another_shape_is_refused() {
        let mut plugin = XyzController::default();
        let good = state(PLAIN, &map(&LINE), &map(&LINE));
        let mut long = good.clone();
        long.push(0);
        assert!(plugin.set_state(&long).is_err());
        assert!(plugin.set_state(&good[..100]).is_err());
        let mut newer = good.clone();
        newer[0] = 4;
        assert!(plugin.set_state(&newer).is_err());
        // A map of another class.
        let mut other = map(&LINE);
        other[0] = 1;
        assert!(
            plugin
                .set_state(&state(PLAIN, &other, &map(&LINE)))
                .is_err()
        );
        assert!(plugin.set_state(&good).is_ok());
        // A joystick is left out and said.
        let mut joystick = good;
        joystick[56] = 1;
        plugin.set_state(&joystick).unwrap();
        assert_eq!(plugin.fidelity(), Fidelity::Approximate(JOYSTICK));
    }

    /// Every state of the plugin in FL's projects, all of them inside a
    /// Patcher, in its presets and in Patcher's presets.
    #[test]
    fn every_state_in_fl_s_files_loads() {
        let Some(root) = crate::testing::corpus() else {
            return;
        };
        let mut counts = [0; 3];
        let check = |state: &[u8], file: &str, count: &mut usize| {
            let mut plugin = XyzController::default();
            plugin
                .set_state(state)
                .unwrap_or_else(|e| panic!("{file}: {e}"));
            assert_eq!(plugin.fidelity(), Fidelity::Exact, "{file}");
            plugin.tick(0, 120.0);
            assert!(plugin.controller_changed(), "{file}");
            let outputs = plugin.controller_outputs();
            assert!(outputs.iter().all(|v| (0.0..=1.0).contains(v)), "{file}");
            // FL's run through the segments lands where the shared curve
            // has the map, to a single's rounding. Right at a point FL's
            // steps can fall short of it, as they do at 1 in a map whose
            // lengths add up to a hair under 1.
            let maps = plugin.axes.iter().flat_map(|a| [&a.input, &a.output]);
            for map in maps {
                for i in 0..256 {
                    let x = (f64::from(i) + 0.5) / 256.0;
                    let (run, curve) = (map.run(x), map.curve.value_at(x));
                    assert!((f64::from(run) - curve).abs() < 1e-6, "{file} {x}");
                }
            }
            *count += 1;
        };
        let inside = |saved: &crate::testing::Saved| {
            let patch = crate::patcher_state::Patch::read(&saved.state).unwrap();
            patch.nodes.into_iter().filter(|n| n.plugin == NAME)
        };
        for saved in crate::testing::projects(&root) {
            assert!(!saved.plugin.eq_ignore_ascii_case(NAME), "{}", saved.file);
            if saved.plugin.eq_ignore_ascii_case(crate::patcher::NAME) {
                for node in inside(&saved) {
                    check(&node.state, &saved.file, &mut counts[0]);
                }
            }
        }
        for saved in crate::testing::every_preset(&root, NAME) {
            check(&saved.state, &saved.file, &mut counts[1]);
        }
        for saved in crate::testing::every_preset(&root, crate::patcher::NAME) {
            for node in inside(&saved) {
                check(&node.state, &saved.file, &mut counts[2]);
            }
        }
        // 30 in the Patchers of five projects that sound reaches and 2 in
        // two that it does not.
        assert_eq!(counts, [32, 14, 467]);
    }
}
