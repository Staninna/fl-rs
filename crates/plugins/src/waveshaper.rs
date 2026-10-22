//! Fruity WaveShaper: a curve the user draws, baked into a table that every
//! sample is looked up in, as `docs/plugins/fruity-waveshaper.md` has it.
//!
//! The state is as long as its curve has points:
//!
//! - 22 bytes: a little-endian u32 version, the three parameters as i32, a
//!   byte that is 1 for a bipolar curve, a byte that is 1 for Center, and
//!   an i32 for the oversampling.
//! - The curve: an i32 kind, an i32 version 0 to 3, an i32 count, the
//!   points, and 20 bytes this does not use. A point is 12 bytes in version
//!   0 (`f32` length, `f32` level, `f32` tension), 16 in version 1 (those,
//!   then a mode byte and three more) and 24 from version 2 on (`f64`
//!   length, `f64` level, `f32` tension, the mode byte and three more).
//! - A tail by the plugin's version: an i32 from version 2 on, a byte for
//!   HQ from 3 on and a byte for smooth modulation in 4.
//!
//! The version is 1, or `0x20003` or `0x40003` with the plugin's version in
//! the high half. FL's files also have a plain 4, which has no tail, and
//! this takes a state of any version without its tail: HQ and smooth are
//! then off.
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Pre amp | 64 to 192 | 128 |
//! | 1 Mix level | 0 to 128 | 128 |
//! | 2 Post gain | 0 to 128 | 128 |
//!
//! A unipolar curve is baked into 16384 16-bit steps over an input of 0 to
//! 1 and mirrored for an input under 0. A bipolar one is 32768 steps over
//! -1 to 1. Without HQ the nearest step is the output, and with it the
//! output is on the line between two steps. The table's full scale is
//! 32767 and the output is the step over 32768, so a straight curve gives
//! `32767 / 32768` of its input and not all of it.
//!
//! With oversampling at `n`, the input goes up `n` times through FL's 2x
//! half band ([`dsp::halfband`]), every sample at `2^n` times the rate goes
//! through the table, and the output comes down through as many again
//! before Center. FL reports no latency for it, and neither does this.
//!
//! With smooth modulation on, the pre amp, wet and dry gains in use glide
//! to the parameters' a frame at a time (at the oversampled rate), 90% of
//! the way in 5 ms of the song's frames. They start at 0 when the plugin
//! is made, so such a state fades in.
//!
//! What this does not do as FL does:
//!
//! - FL looks up as with HQ and oversamples 16 times while it renders to a
//!   file (when the state oversamples at all), whatever the state says.
//!   This goes by the state.

use crate::parts::Follower;
use crate::{Effect, Fidelity, StateError, knob_gain, knob_value};
use dsp::curve::{Curve, Point};
use dsp::halfband::{Oversampler, WIDEST};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity WaveShaper";

/// The range of each parameter's wheel.
const RANGES: [(i32, i32); 3] = [(64, 192), (0, 128), (0, 128)];
const DEFAULTS: [i32; 3] = [128, 128, 128];

/// The range of the oversampling selector, which is the state's fourth
/// number.
const OVERSAMPLING: (i32, i32) = (0, 4);

/// How long the state is before its curve, and the newest version there
/// is.
const HEADER: usize = 22;
const NEWEST: u32 = 0x40003;

/// What follows a curve's points.
const TRAILER: usize = 20;

/// How many steps a unipolar table has. A bipolar one has twice as many.
const STEPS: usize = 0x4000;

/// Smooth modulation's pace: what is left of the way after 5 ms of
/// frames at the song's rate (`0x00830f38`, `0x00830f48`).
const SMOOTH_LEFT: f64 = 0.1;
const SMOOTH_TIME: f32 = 0.005;

/// What Center gives its follower: `1.0070235 / rate` to go after the
/// input with, and 0.96 of its speed kept.
const CENTER_PACE: f64 = 1.007_023_5;
const CENTER_KEEP: f64 = 0.96;

/// The bytes of a state, read from the front.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    /// The next `N` bytes. A state that ends before them is too short.
    fn take<const N: usize>(&mut self) -> Result<[u8; N], StateError> {
        let rest = self.bytes.get(self.at..).unwrap_or_default();
        let Some(taken) = rest.first_chunk::<N>() else {
            return Err(StateError::Length {
                expected: self.at + N,
                found: self.bytes.len(),
            });
        };
        self.at += N;
        Ok(*taken)
    }

    fn byte(&mut self) -> Result<u8, StateError> {
        Ok(self.take::<1>()?[0])
    }

    fn i32(&mut self) -> Result<i32, StateError> {
        Ok(i32::from_le_bytes(self.take()?))
    }

    fn f32(&mut self) -> Result<f32, StateError> {
        Ok(f32::from_le_bytes(self.take()?))
    }

    fn f64(&mut self) -> Result<f64, StateError> {
        Ok(f64::from_le_bytes(self.take()?))
    }

    /// An i32 that has to be inside the range of control `param`.
    fn ranged(&mut self, param: usize, (min, max): (i32, i32)) -> Result<i32, StateError> {
        let value = self.i32()?;
        if value < min || value > max {
            return Err(StateError::Range { param, value });
        }
        Ok(value)
    }
}

/// The points of a curve as FL saves one from byte `from` on, in a state or
/// in a `.fnv` map, and where the curve ends.
///
/// The kind is 3 in every file there is, and FL does not turn another one
/// down. The three bytes after a point's mode are what FL works out anew
/// after loading.
fn envelope(bytes: &[u8], from: usize) -> Result<(Vec<Point>, usize), StateError> {
    let mut reader = Reader { bytes, at: from };
    let _kind = reader.i32()?;
    let version = reader.i32()?;
    let size = match version {
        0 => 12,
        1 => 16,
        2 | 3 => 24,
        _ => return Err(StateError::Version { found: version }),
    };
    let count = usize::try_from(reader.i32()?).unwrap_or(usize::MAX);
    let points = count.checked_mul(size);
    let end = points.and_then(|points| points.checked_add(reader.at + TRAILER));
    let end = end.unwrap_or(usize::MAX);
    if end > bytes.len() {
        return Err(StateError::Length {
            expected: end,
            found: bytes.len(),
        });
    }

    let mut points = Vec::with_capacity(count);
    for _ in 0..count {
        let (x, y) = if size == 24 {
            (reader.f64()?, reader.f64()?)
        } else {
            (f64::from(reader.f32()?), f64::from(reader.f32()?))
        };
        let tension = reader.f32()?;
        let mode = if size == 12 {
            0
        } else {
            reader.take::<4>()?[0]
        };
        points.push(Point {
            x,
            y,
            tension,
            mode,
        });
    }
    Ok((points, end))
}

/// The table FL bakes a curve into: `STEPS` values for a unipolar curve
/// and twice as many for a bipolar one, with the first and the last once
/// more at either end.
///
/// Step `i` of `n` is the curve at `i / (n - 1)`. A unipolar curve's 0 to 1
/// becomes 0 to 32767, and a bipolar one's -32768 to 32767 with a level of
/// a half in the middle.
fn bake(curve: &Curve, bipolar: bool) -> Vec<i16> {
    let (steps, centre, scale) = if bipolar {
        (2 * STEPS, 0.5, 65_535.0)
    } else {
        (STEPS, 0.0, 32_767.0)
    };
    let last = (steps - 1) as f64;
    let step = |i: usize| {
        let level = curve.value_at(i as f64 / last) - centre;
        (level * scale).round_ties_even().clamp(-32_768.0, 32_767.0) as i16
    };
    let mut table = Vec::with_capacity(steps + 2);
    table.push(step(0));
    table.extend((0..steps).map(step));
    table.push(step(steps - 1));
    table
}

/// The three gains the parameters come to: the pre amp, the wet gain and
/// the dry one.
///
/// The dry gain has the pre amp in it and the post gain twice, as FL has
/// it.
fn gains(params: &[i32; 3]) -> [f32; 3] {
    let [pre, mix, post] = params.map(|value| f64::from(value) / 128.0);
    let pre = (4f64.powf(pre) - 1.0) / 3.0;
    let post = if post == 1.0 { 1.0 } else { knob_gain(post) };
    let wet = mix * post;
    [pre as f32, wet as f32, ((post - wet) * pre * post) as f32]
}

/// Fruity WaveShaper. A new one has FL's defaults: a straight unipolar
/// curve, which gives the input back to 15 bits.
#[derive(Debug, Clone, PartialEq)]
pub struct WaveShaper {
    params: [i32; 3],
    bipolar: bool,
    center: bool,
    /// The oversampling the state asks for: `2^n` times.
    oversampling: i32,
    hq: bool,
    smooth: bool,
    points: Vec<Point>,
    table: Vec<i16>,
    hz: f64,
    pre: f32,
    wet: f32,
    dry: f32,
    /// The pre amp, wet and dry gains smooth modulation has in use
    /// (`0x144`, `0x148`, `0x14c`), and the share of the way to the
    /// parameters' they go a frame (`0x15c`).
    glided: [f32; 3],
    pace: f32,
    /// Center's follower of each side.
    offset: [Follower; 2],
    /// The stages up and down (`0x160`, `0x168`).
    oversampler: Oversampler,
}

impl Default for WaveShaper {
    fn default() -> Self {
        let points = vec![Point::new(0.0, 0.0), Point::new(1.0, 1.0)];
        let [pre, wet, dry] = gains(&DEFAULTS);
        WaveShaper {
            params: DEFAULTS,
            bipolar: false,
            center: false,
            oversampling: 0,
            hq: false,
            smooth: false,
            table: bake(&Curve::new(&points), false),
            points,
            hz: 44_100.0,
            pre,
            wet,
            dry,
            glided: [0.0; 3],
            pace: smooth_pace(44_100.0),
            offset: [Follower::default(); 2],
            oversampler: Oversampler::default(),
        }
    }
}

impl WaveShaper {
    /// The curve's points.
    pub fn points(&self) -> &[Point] {
        &self.points
    }

    /// Take parameters that are inside their ranges.
    fn set(&mut self, params: [i32; 3]) {
        self.params = params;
        [self.pre, self.wet, self.dry] = gains(&params);
    }

    /// One sample through the table and the mix, with the pre amp, wet
    /// and dry gains `[pre, wet, dry]`.
    fn shape(&self, x: f32, [pre, wet, dry]: [f32; 3]) -> f32 {
        let table = &self.table;
        let last = (table.len() - 3) as f32;
        // Step `i` is at `table[i + 1]`: one step under the first is the
        // first again, and one over the last the last.
        let step = |i: f32| f32::from(table[(i + 1.0) as usize]);
        let value = if self.bipolar {
            let place = x * pre * (last * 0.5) + last * 0.5;
            let nearest = place.round_ties_even();
            if self.hq {
                let i = nearest.clamp(-1.0, last);
                let (from, to) = (step(i), step(i + 1.0));
                from + (to - from) * (place - i)
            } else {
                step(nearest.clamp(0.0, last))
            }
        } else {
            // The curve is for an input from 0 up, and an input under 0
            // gets what its size gets, upside down.
            let place = x * pre * last;
            let size = place.abs();
            let i = size.round_ties_even().min(last);
            let value = if self.hq {
                let (from, to) = (step(i), step(i + 1.0));
                from + (to - from) * (size - i)
            } else {
                step(i)
            };
            value.copysign(place)
        };
        value * wet / 32_768.0 + x * dry
    }
}

/// Smooth modulation's share of the way a frame (Dispatcher 4,
/// `0x00830bf0`): `1 - 0.1^(1 / (rate * 0.005))`, Delphi's `Power` being
/// `exp(y ln x)` here.
fn smooth_pace(hz: f32) -> f32 {
    let frames = f64::from(hz * SMOOTH_TIME);
    (1.0 - ((1.0 / frames) * SMOOTH_LEFT.ln()).exp()) as f32
}

impl Effect for WaveShaper {
    /// FL turns a version above `0x40003` down, and so does this. This
    /// asks for memory for the curve and its table.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let mut reader = Reader { bytes, at: 0 };
        let word = u32::from_le_bytes(reader.take()?);
        if word > NEWEST {
            return Err(StateError::Version { found: word as i32 });
        }
        let version = if word > 0xffff { word >> 16 } else { word };
        let mut params = [0; 3];
        for (param, range) in RANGES.iter().enumerate() {
            params[param] = reader.ranged(param, *range)?;
        }
        let bipolar = reader.byte()? != 0;
        let center = reader.byte()? != 0;
        let oversampling = reader.ranged(RANGES.len(), OVERSAMPLING)?;
        let (points, end) = envelope(bytes, HEADER)?;

        // The tail, or nothing where the state ends with its curve.
        let tail = &bytes[end..];
        let expected = match version {
            ..2 => 0,
            2 => 4,
            3 => 5,
            _ => 6,
        };
        if !tail.is_empty() && tail.len() != expected {
            return Err(StateError::Length {
                expected: end + expected,
                found: bytes.len(),
            });
        }

        self.table = bake(&Curve::new(&points), bipolar);
        self.points = points;
        self.bipolar = bipolar;
        self.center = center;
        self.oversampling = oversampling;
        self.hq = tail.get(4).is_some_and(|byte| *byte != 0);
        self.smooth = tail.get(5).is_some_and(|byte| *byte != 0);
        self.set(params);
        // A load builds the stages again (`FUN_00832300`), from silence.
        self.oversampler.reset();
        Ok(())
    }

    /// Center's corner stays at 4 Hz at any rate. A rate that is not a
    /// number above 0 is not taken.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz > 0.0 {
            self.hz = f64::from(hz);
            self.pace = smooth_pace(hz.round());
        }
    }

    /// `Eff_Render` (`0x00832390`): up, the table, down, then Center at
    /// the song's rate.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        let pace = CENTER_PACE / self.hz;
        let stages = self.oversampling as usize;
        let gains = [self.pre, self.wet, self.dry];
        let (mut oversampler, mut glided) = (self.oversampler, self.glided);
        let (smooth, glide) = (self.smooth, self.pace);
        // The gains for the next frame at the curve's rate.
        let mut next = || {
            if smooth {
                for (now, to) in glided.iter_mut().zip(gains) {
                    *now += (to - *now) * glide;
                }
                glided
            } else {
                gains
            }
        };
        for (l, r) in left[..len].iter_mut().zip(&mut right[..len]) {
            if stages == 0 {
                let now = next();
                [*l, *r] = [*l, *r].map(|x| self.shape(x, now));
                continue;
            }
            let mut wide = [[0.0; 2]; WIDEST];
            oversampler.up(stages, [*l, *r], &mut wide);
            for frame in &mut wide[..1 << stages] {
                let now = next();
                *frame = frame.map(|x| self.shape(x, now));
            }
            [*l, *r] = oversampler.down(stages, &mut wide);
        }
        if stages > 0 {
            oversampler.flush();
        }
        (self.oversampler, self.glided) = (oversampler, glided);
        for (side, samples) in [&mut left[..len], &mut right[..len]]
            .into_iter()
            .enumerate()
        {
            if self.center {
                let offset = &mut self.offset[side];
                for sample in samples.iter_mut() {
                    let x = f64::from(*sample);
                    *sample = (x - offset.step(x, pace, CENTER_KEEP)) as f32;
                }
                offset.flush();
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

    fn fidelity(&self) -> Fidelity {
        Fidelity::Exact
    }

    /// FL's flush (Dispatcher 2): Center's followers and the stages back
    /// at nothing.
    fn reset(&mut self) {
        self.offset = [Follower::default(); 2];
        self.oversampler.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, corpus, every_preset, maps, presets, state};

    /// A point as version 3 of the curve saves it.
    fn point(x: f64, y: f64, tension: f32, mode: u8) -> Point {
        Point {
            x,
            y,
            tension,
            mode,
        }
    }

    /// A curve of version `version`, as a state or a map has it.
    fn curve_bytes(version: i32, points: &[Point]) -> Vec<u8> {
        let mut bytes = state(&[3, version, points.len() as i32]);
        for p in points {
            if version < 2 {
                bytes.extend((p.x as f32).to_le_bytes());
                bytes.extend((p.y as f32).to_le_bytes());
            } else {
                bytes.extend(p.x.to_le_bytes());
                bytes.extend(p.y.to_le_bytes());
            }
            bytes.extend(p.tension.to_le_bytes());
            if version > 0 {
                bytes.extend([p.mode, 0, 0, 0]);
            }
        }
        bytes.extend(state(&[0, -1, -1, -1, -1]));
        bytes
    }

    /// What is ahead of a state's curve.
    fn header(word: u32, params: [i32; 3], bipolar: bool, center: bool) -> Vec<u8> {
        let mut bytes = word.to_le_bytes().to_vec();
        bytes.extend(state(&params));
        bytes.extend([u8::from(bipolar), u8::from(center)]);
        bytes.extend(state(&[0]));
        bytes
    }

    /// A state of the newest version, with HQ on or off.
    fn newest(params: [i32; 3], bipolar: bool, hq: bool, points: &[Point]) -> Vec<u8> {
        let mut bytes = header(NEWEST, params, bipolar, false);
        bytes.extend(curve_bytes(3, points));
        bytes.extend(state(&[1]));
        bytes.extend([u8::from(hq), 0]);
        bytes
    }

    fn with(bytes: &[u8]) -> WaveShaper {
        let mut plugin = WaveShaper::default();
        plugin.set_state(bytes).unwrap();
        plugin
    }

    /// The left side of what both sides get.
    fn run(plugin: &mut WaveShaper, input: &[f32]) -> Vec<f32> {
        let (mut left, mut right) = (input.to_vec(), input.to_vec());
        plugin.process(&mut left, &mut right);
        assert!(left == right);
        left
    }

    fn line() -> [Point; 2] {
        [Point::new(0.0, 0.0), Point::new(1.0, 1.0)]
    }

    /// Linear to 0.625 and flat from there: the doc's "Fast dist 1".
    fn clipper() -> [Point; 3] {
        [
            Point::new(0.0, 0.0),
            Point::new(0.625, 0.625),
            Point::new(0.375, 0.625),
        ]
    }

    #[test]
    fn a_straight_bipolar_curve_gives_the_input_back_to_15_bits() {
        // The table: 32768 steps from -32768 to 32767, two apart.
        let plugin = with(&newest(DEFAULTS, true, true, &line()));
        assert_eq!(plugin.table.len(), 32_770);
        assert_eq!(plugin.table[..3], [-32_768, -32_768, -32_765]);
        assert_eq!(plugin.table[32_767..], [32_765, 32_767, 32_767]);
        for (i, pair) in plugin.table[2..32_769].windows(2).enumerate() {
            assert_eq!(pair[1] - pair[0], 2, "step {i}");
        }

        // With HQ the output is on the line through the steps, which is
        // the input times 32767 / 32768: the table's full scale over what
        // FL divides a step by. So unity is not to the bit, by the doc's
        // own numbers. The very first step is one unit lower than the
        // line, so -1 itself does come out as -1.
        let input: Vec<f32> = (-1000..=1000).map(|n| n as f32 / 1000.0).collect();
        let out = run(&mut with(&newest(DEFAULTS, true, true, &line())), &input);
        for (x, y) in input.iter().zip(&out).skip(1) {
            let want = f64::from(*x) * 32_767.0 / 32_768.0;
            assert!((f64::from(*y) - want).abs() < 3e-7, "{x}: {y}");
        }
        assert_eq!(
            (out[0], out[1000], out[2000]),
            (-1.0, 0.0, 32_767.0 / 32_768.0)
        );
        // Without it the output is the nearest step, at most one step's
        // 2 / 32768 away from the line.
        let out = run(&mut with(&newest(DEFAULTS, true, false, &line())), &input);
        let mut worst = 0f64;
        for (x, y) in input.iter().zip(&out) {
            assert_eq!(y * 32_768.0, (y * 32_768.0).round(), "{x}");
            worst = worst.max((f64::from(*y) - f64::from(*x) * 32_767.0 / 32_768.0).abs());
        }
        assert!(
            worst > 0.5 / 32_768.0 && worst <= 1.0 / 32_768.0 + 1e-7,
            "{worst}"
        );
        // Past full scale either lookup stays on the last step.
        for hq in [false, true] {
            let mut plugin = with(&newest(DEFAULTS, true, hq, &line()));
            let out = run(&mut plugin, &[1.5, -1.5, 40.0, -40.0]);
            assert_eq!(out, [32_767.0 / 32_768.0, -1.0, 32_767.0 / 32_768.0, -1.0]);
        }
    }

    #[test]
    fn a_curve_that_goes_flat_clips_at_the_table_s_value() {
        // 0.625 of 32767 is step 20479, so the clip is at 20479 / 32768.
        let flat = 20_479.0 / 32_768.0;
        for hq in [false, true] {
            let mut plugin = with(&newest(DEFAULTS, false, hq, &clipper()));
            assert_eq!(plugin.table.len(), 16_386);
            assert_eq!(plugin.table[16_385], 20_479);
            let out = run(&mut plugin, &[0.7, 0.9, 1.0, 3.0, -0.7, -3.0, 0.0]);
            assert_eq!(out, [flat, flat, flat, flat, -flat, -flat, 0.0]);
            // Under the corner it is the line.
            let out = run(&mut plugin, &[0.25, -0.5]);
            assert!((out[0] - 0.25).abs() < 1e-4 && (out[1] + 0.5).abs() < 1e-4);
        }
        // The pre amp drives into the clip: at 192 it is 2.33, so 0.3 is
        // 0.7 and over the corner.
        let mut plugin = with(&newest([192, 128, 128], false, false, &clipper()));
        assert!((plugin.pre - 7.0 / 3.0).abs() < 1e-6);
        assert_eq!(run(&mut plugin, &[0.3, 0.2])[0], flat);
        assert!((run(&mut plugin, &[0.2])[0] - 0.2 * 7.0 / 3.0).abs() < 1e-4);
    }

    #[test]
    fn a_unipolar_curve_is_mirrored_and_a_bipolar_one_is_not() {
        // A curve that is not a line: a hold that jumps to 1 at the end,
        // and a start that is not at 0.
        let bent = [
            point(0.0, 0.25, 0.0, 0),
            point(0.5, 0.5, -0.5, 0),
            point(0.5, 1.0, 0.75, 9),
        ];
        let input: Vec<f32> = (1..200).map(|n| n as f32 / 200.0).collect();
        let minus: Vec<f32> = input.iter().map(|x| -x).collect();
        for hq in [false, true] {
            let mut plugin = with(&newest(DEFAULTS, false, hq, &bent));
            let (up, down) = (run(&mut plugin, &input), run(&mut plugin, &minus));
            for (up, down) in up.iter().zip(&down) {
                assert!(*up > 0.24 && *down == -*up);
            }
            // Bipolar, the left half of the curve is for an input under 0
            // and the right half for one above: -0.5 gets the curve at a
            // quarter of its length, and 0.5 the curve at three quarters.
            let mut plugin = with(&newest(DEFAULTS, true, hq, &bent));
            let (up, down) = (run(&mut plugin, &input), run(&mut plugin, &minus));
            assert!(up.iter().zip(&down).all(|(up, down)| *down != -*up));
            let curve = Curve::new(&bent);
            for (x, at) in [(-0.5f32, 0.25), (0.5, 0.75), (0.0, 0.5)] {
                let want = (curve.value_at(at) - 0.5) * 65_535.0 / 32_768.0;
                let got = f64::from(run(&mut plugin, &[x])[0]);
                assert!((got - want).abs() < 2e-4, "{x}: {got} and not {want}");
            }
        }
    }

    #[test]
    fn hq_reads_between_the_steps_and_without_it_the_nearest_one() {
        // A ramp over the first four steps of a unipolar line, a hundred
        // samples to the step. A step is 2 of the table's units high.
        let input: Vec<f32> = (0..=400).map(|n| n as f32 / 100.0 / 16_383.0).collect();
        let mut nearest = with(&newest(DEFAULTS, false, false, &line()));
        let mut between = with(&newest(DEFAULTS, false, true, &line()));
        assert_eq!(nearest.table[..6], [0, 0, 2, 4, 6, 8]);
        let (stepped, smooth) = (run(&mut nearest, &input), run(&mut between, &input));
        // The nearest step: five values, each held for a hundred samples
        // around its place.
        let mut values = stepped.clone();
        values.dedup();
        assert_eq!(values, [0.0, 2.0, 4.0, 6.0, 8.0].map(|v| v / 32_768.0));
        assert_eq!(stepped[49], 0.0);
        assert_eq!(stepped[51], 2.0 / 32_768.0);
        assert_eq!(stepped[149], 2.0 / 32_768.0);
        assert_eq!(stepped[151], 4.0 / 32_768.0);
        // Between the steps: the line itself, so every sample is higher
        // than the one before and none is further from the line than a
        // float's rounding.
        assert!(smooth.windows(2).all(|pair| pair[1] > pair[0]));
        for (n, y) in smooth.iter().enumerate() {
            let want = n as f64 / 100.0 * 2.0 / 32_768.0;
            assert!((f64::from(*y) - want).abs() < 1e-9, "{n}: {y}");
        }
        // The two agree on the steps and are half a step apart between.
        assert_eq!(stepped[200], smooth[200]);
        assert!((stepped[149] - smooth[149] + 0.98 / 32_768.0).abs() < 1e-8);
    }

    #[test]
    fn the_tension_bends_a_segment_on_1000001_to_its_size() {
        // Under 0: (B^(|T| t) - 1) / (B^|T| - 1). Above 0 the mirror image.
        let base = 1_000_001f64;
        for tension in [-0.25f32, -1.0, 0.25, 0.005] {
            let size = f64::from(tension.abs());
            let bent = [Point::new(0.0, 0.0), point(1.0, 1.0, tension, 0)];
            let plugin = with(&newest(DEFAULTS, false, false, &bent));
            for step in [1000usize, 8191, 12_000, 16_383] {
                let t = step as f64 / 16_383.0;
                let bend = |u: f64| (base.powf(size * u) - 1.0) / (base.powf(size) - 1.0);
                let want = if tension < 0.0 {
                    bend(t)
                } else {
                    1.0 - bend(1.0 - t)
                };
                let got = f64::from(plugin.table[step + 1]);
                assert!(
                    (got - want * 32_767.0).abs() <= 0.5 + 1e-6,
                    "{tension} at {step}"
                );
            }
        }
        // At a quarter it is e^(3.45 t) and at 1 nearly a step, as the doc
        // says: half-way, a thousandth of the way up.
        assert!((base.ln() * 0.25 - 3.45).abs() < 0.005);
        let step = [Point::new(0.0, 0.0), point(1.0, 1.0, -1.0, 0)];
        let plugin = with(&newest(DEFAULTS, false, false, &step));
        assert_eq!(plugin.table[8192 + 1], 33);
        // The 13 modes are the shared curve's: a hold jumps at its end,
        // and a half sine at full tension is a quarter of a sine.
        let held = [Point::new(0.0, 0.0), point(1.0, 1.0, 0.0, 2)];
        let plugin = with(&newest(DEFAULTS, false, false, &held));
        assert_eq!(plugin.table[16_383..], [0, 32_767, 32_767]);
        let sine = [Point::new(0.0, 0.0), point(1.0, 1.0, 1.0, 9)];
        let plugin = with(&newest(DEFAULTS, false, false, &sine));
        let want = (std::f64::consts::FRAC_PI_4).sin() * 32_767.0;
        assert!((f64::from(plugin.table[8192]) - want).abs() < 2.0);
    }

    #[test]
    fn the_mix_and_the_dry_gain_are_as_fl_has_them() {
        // The pre amp: (4^x - 1) / 3.
        for (wheel, pre) in [(64, 1.0 / 3.0), (128, 1.0), (192, 7.0 / 3.0), (162, 1.5936)] {
            let got = f64::from(gains(&[wheel, 128, 128])[0]);
            assert!((got - pre).abs() < 1e-4, "{wheel}: {got}");
        }
        assert_eq!(gains(&DEFAULTS), [1.0, 1.0, 0.0]);
        // The post gain: (11^x - 1) / 10. The wet gain is the mix times
        // it, and the dry gain what is left of it, times the pre amp and
        // the post gain once more.
        for params in [[128, 64, 128], [192, 83, 111], [93, 0, 68], [64, 128, 0]] {
            let [pre, mix, post] = params.map(|v| f64::from(v) / 128.0);
            let pre = (4f64.powf(pre) - 1.0) / 3.0;
            let post = (11f64.powf(post) - 1.0) / 10.0;
            let (wet, dry) = (mix * post, (post - mix * post) * pre * post);
            let got = gains(&params).map(f64::from);
            for (got, want) in got.into_iter().zip([pre, wet, dry]) {
                assert!((got - want).abs() < 1e-6, "{params:?}");
            }
            // A sample is its step times the wet gain plus itself times
            // the dry one.
            let mut plugin = with(&newest(params, false, false, &clipper()));
            let step =
                f64::from(plugin.table[1 + (0.9 * pre * 16_383.0).round().min(16_383.0) as usize]);
            let got = f64::from(run(&mut plugin, &[0.9])[0]);
            let want = step * wet / 32_768.0 + 0.9 * dry;
            assert!((got - want).abs() < 1e-6, "{params:?}: {got}");
        }
        // Half wet at unity: half the clipped sample and half the input.
        let mut plugin = with(&newest([128, 64, 128], false, false, &clipper()));
        let got = run(&mut plugin, &[0.9])[0];
        assert!((got - (0.5 * 20_479.0 / 32_768.0 + 0.45)).abs() < 1e-6);
    }

    #[test]
    fn center_takes_a_standing_offset_out() {
        // A curve that starts at a quarter gives silence an offset.
        let lifted = [Point::new(0.0, 0.25), Point::new(1.0, 1.0)];
        let mut bytes = header(NEWEST, DEFAULTS, false, true);
        bytes.extend(curve_bytes(3, &lifted));
        bytes.extend([1, 0, 0, 0, 0, 0]);
        let mut plugin = with(&bytes);
        plugin.set_sample_rate(48_000.0);
        let out = run(&mut plugin, &vec![0.0; 48_000]);
        assert!((out[0] - 0.25).abs() < 1e-3);
        assert!(out[47_999].abs() < 1e-4, "{}", out[47_999]);
        // A reset starts it over.
        plugin.reset();
        assert!((run(&mut plugin, &[0.0])[0] - 0.25).abs() < 1e-3);
        // Without Center the offset stays.
        bytes[17] = 0;
        let out = run(&mut with(&bytes), &vec![0.0; 4800]);
        assert!(out.iter().all(|v| (v - 0.25).abs() < 1e-4));
    }

    #[test]
    fn every_form_of_the_state_loads() {
        let points = clipper();
        // The three point sizes: 12, 16 and 24 bytes.
        let sizes = [0, 1, 2, 3].map(|version| curve_bytes(version, &points).len());
        assert_eq!(sizes, [68, 80, 104, 104]);
        let want = with(&newest(DEFAULTS, false, false, &points));
        assert_eq!(newest(DEFAULTS, false, false, &points).len(), 22 + 104 + 6);
        for version in [0, 1, 2, 3] {
            // What FL's presets have: version 1, and no tail.
            let mut bytes = header(1, DEFAULTS, false, false);
            bytes.extend(curve_bytes(version, &points));
            assert_eq!(with(&bytes), want, "curve version {version}");
        }
        // A plain 4 has no tail either, and 0x20003 has four bytes of it.
        let mut bytes = header(4, DEFAULTS, false, false);
        bytes.extend(curve_bytes(3, &points));
        assert_eq!(with(&bytes), want);
        let mut bytes = header(0x20003, DEFAULTS, false, false);
        bytes.extend(curve_bytes(3, &points));
        bytes.extend(state(&[1]));
        assert_eq!(with(&bytes), want);
        // A point's mode is read from version 1 on.
        let modes = [Point::new(0.0, 0.0), point(1.0, 1.0, 0.5, 9)];
        let mut bytes = header(1, DEFAULTS, false, false);
        bytes.extend(curve_bytes(1, &modes));
        assert_eq!(with(&bytes).points(), modes);
        let mut bytes = header(1, DEFAULTS, false, false);
        bytes.extend(curve_bytes(0, &modes));
        assert_eq!(with(&bytes).points()[1].mode, 0);

        // HQ and smooth are the tail's last two bytes.
        let mut bytes = newest(DEFAULTS, true, true, &points);
        let hq = with(&bytes);
        assert!(hq.hq && !hq.smooth && hq.bipolar && !hq.center);
        let end = bytes.len();
        bytes[end - 2..].copy_from_slice(&[0, 1]);
        assert!(!with(&bytes).hq && with(&bytes).smooth);
        // The oversampling is the byte after Center.
        assert_eq!(hq.fidelity(), Fidelity::Exact);
        bytes[18] = 2;
        let four = with(&bytes);
        assert_eq!((four.oversampling, four.fidelity()), (2, Fidelity::Exact));
    }

    /// A state of the newest version with this oversampling and smooth
    /// modulation on or off.
    fn oversampled(params: [i32; 3], points: &[Point], over: i32, smooth: bool) -> WaveShaper {
        let mut bytes = newest(params, true, true, points);
        bytes[18..22].copy_from_slice(&over.to_le_bytes());
        let end = bytes.len();
        bytes[end - 1] = u8::from(smooth);
        with(&bytes)
    }

    #[test]
    fn oversampling_is_the_half_band_up_the_curve_and_the_half_band_down() {
        // A curve that is nearly a line, so the table gives the input
        // back: oversampled, the output is the input up through as many
        // of FL's half bands and down through as many again, to within
        // the table's steps.
        let input: Vec<f32> = (0..2000)
            .map(|n| ((n * 7919 % 1009) as f32 / 1009.0 - 0.5) * 0.8)
            .collect();
        for over in 1..=4 {
            let mut plugin = oversampled(DEFAULTS, &line(), over, false);
            assert_eq!(plugin.latency(), 0);
            let out = run(&mut plugin, &input);
            let mut want = input.clone();
            for _ in 0..over {
                let mut wide = vec![0.0; want.len() * 2];
                dsp::halfband::HalfBand::default().up(&want, &mut wide);
                want = wide;
            }
            for v in &mut want {
                *v *= 32_767.0 / 32_768.0;
            }
            for _ in 0..over {
                let mut narrow = vec![0.0; want.len() / 2];
                dsp::halfband::HalfBand::default().down(&want, &mut narrow);
                want = narrow;
            }
            for (n, (y, w)) in out.iter().zip(&want).enumerate() {
                assert!((y - w).abs() < 2e-6, "{over} {n}: {y} {w}");
            }
        }
    }

    #[test]
    fn oversampling_keeps_a_clipper_s_harmonics_from_folding_back() {
        // A 7 kHz tone driven into "Fast dist 1"'s clip: its fifth
        // harmonic, 35 kHz, folds to 9.1 kHz at 44.1 kHz and its seventh,
        // 49 kHz, to 4.9 kHz. Sixteen times oversampled they are stopped
        // on the way down; the tone and its third harmonic stay.
        let tone = testing::sine(70.0 / 441.0, 3 * 4410);
        let measure = |over: i32| {
            let mut plugin = oversampled([192, 128, 128], &clipper(), over, false);
            let out = run(&mut plugin, &tone);
            let tail = &out[2 * 4410..];
            [70.0, 210.0, 91.0, 49.0].map(|k| 20.0 * testing::partial(tail, k / 441.0).log10())
        };
        let [tone_1x, third_1x, fifth_1x, seventh_1x] = measure(0);
        let [tone_16x, third_16x, fifth_16x, seventh_16x] = measure(4);
        assert!((tone_1x - tone_16x).abs() < 0.5, "{tone_1x} {tone_16x}");
        assert!((third_1x - third_16x).abs() < 1.5, "{third_1x} {third_16x}");
        assert!(
            fifth_1x > -30.0 && seventh_1x > -40.0,
            "{fifth_1x} {seventh_1x}"
        );
        assert!(fifth_16x < fifth_1x - 40.0, "{fifth_1x} {fifth_16x}");
        assert!(
            seventh_16x < seventh_1x - 40.0,
            "{seventh_1x} {seventh_16x}"
        );
    }

    #[test]
    fn smooth_modulation_glides_the_gains_from_nothing() {
        // 90% of the way in 5 ms: 220.5 frames at 44.1 kHz.
        let pace = smooth_pace(44_100.0);
        assert!(((1.0 - f64::from(pace)).powf(220.5) - 0.1).abs() < 1e-6);
        let held = vec![0.5; 2000];
        let mut plugin = oversampled(DEFAULTS, &line(), 0, true);
        let out = run(&mut plugin, &held);
        // The pre amp and the wet gain both glide: 0.5 * g^2.
        let g = 1.0 - (1.0 - pace).powi(221);
        assert!(out[0] < 1e-3, "{}", out[0]);
        assert!((out[220] - 0.5 * g * g).abs() < 2e-3, "{} {}", out[220], g);
        assert!((out[1999] - 0.5).abs() < 1e-4);
        // Off, the gains are where the parameters are at once.
        let out = run(&mut oversampled(DEFAULTS, &line(), 0, false), &held);
        assert!((out[0] - 0.5).abs() < 1e-4);
        // Oversampled, the glide steps at the higher rate: twice as fast
        // in the song's frames.
        let out = run(&mut oversampled(DEFAULTS, &line(), 1, true), &held);
        assert!((out[110] - 0.5 * g * g).abs() < 0.02, "{}", out[110]);
    }

    #[test]
    fn a_state_the_plugin_does_not_write_is_turned_down() {
        let good = newest([162, 83, 111], true, true, &clipper());
        let mut plugin = with(&good);
        let before = plugin.clone();
        // Cut short anywhere, or with a tail of another length.
        for len in [0, 3, 21, 22, 30, 34, 60, good.len() - 7, good.len() - 1] {
            let got = plugin.set_state(&good[..len]);
            assert!(
                matches!(got, Err(StateError::Length { found, .. }) if found == len),
                "{len}: {got:?}"
            );
        }
        // The error says how long the state would have to be: its header,
        // and then all of its curve and its tail.
        assert_eq!(
            plugin.set_state(&good[..21]),
            Err(StateError::Length {
                expected: 22,
                found: 21
            })
        );
        assert_eq!(
            plugin.set_state(&good[..60]),
            Err(StateError::Length {
                expected: good.len() - 6,
                found: 60
            })
        );
        let mut long = good.clone();
        long.push(0);
        assert_eq!(
            plugin.set_state(&long),
            Err(StateError::Length {
                expected: good.len(),
                found: good.len() + 1
            })
        );
        // A version newer than FL's, of the plugin or of the curve.
        let mut newer = good.clone();
        newer[0] = 4;
        assert_eq!(
            plugin.set_state(&newer),
            Err(StateError::Version { found: 0x40004 })
        );
        let mut newer = good.clone();
        newer[26] = 4;
        assert_eq!(
            plugin.set_state(&newer),
            Err(StateError::Version { found: 4 })
        );
        // A count of points the state is too short for, or under 0.
        for count in [4, 1_000_000, -1] {
            let mut counted = good.clone();
            counted[30..34].copy_from_slice(&i32::to_le_bytes(count));
            assert!(matches!(
                plugin.set_state(&counted),
                Err(StateError::Length { .. })
            ));
        }
        for (param, value) in [(0, 63), (0, 193), (1, 129), (2, -1), (3, 5)] {
            let mut ranged = good.clone();
            let at = if param == 3 { 18 } else { 4 + 4 * param };
            ranged[at..at + 4].copy_from_slice(&i32::to_le_bytes(value));
            assert_eq!(
                plugin.set_state(&ranged),
                Err(StateError::Range { param, value })
            );
        }
        assert_eq!(plugin, before);

        // The wheels stop at their ends.
        for (index, value) in [(0, 500.0), (1, -3.0), (2, 68.4), (3, 1.0)] {
            plugin.set_param(index, value);
        }
        assert_eq!(
            [0, 1, 2, 3].map(|i| plugin.param(i)),
            [192.0, 0.0, 68.0, 0.0]
        );
        assert_eq!(plugin.param_count(), 3);
    }

    #[test]
    fn fl_s_presets_and_maps_load() {
        let Some(own) = presets(NAME) else { return };
        let root = corpus().unwrap();
        let all = every_preset(&root, NAME);
        // 34 of the plugin's own, three of the plugin database and two
        // mixer presets.
        assert_eq!((own.len(), all.len()), (34, 39));
        let named = |file: &str| {
            let preset = own.iter().chain(&all).find(|p| p.file == file);
            with(&preset.unwrap_or_else(|| panic!("no preset {file}")).state)
        };
        let mut lengths: Vec<usize> = Vec::new();
        for preset in &all {
            let plugin = with(&preset.state);
            // A curve has its first point at the start and is as long as
            // 1, to a float's rounding.
            let points = plugin.points();
            let length: f64 = points.iter().map(|p| p.x).sum();
            assert!(
                points.len() >= 2 && points[0].x.abs() < 2e-6,
                "{}",
                preset.file
            );
            assert!((length - 1.0).abs() < 1e-5, "{}: {length}", preset.file);
            lengths.push(preset.state.len());
        }
        lengths.sort_unstable();
        assert_eq!((lengths[0], lengths[38]), (78, 3486));

        // The doc's table.
        let default = named("Default");
        assert_eq!(default, WaveShaper::default());
        assert_eq!(default.points(), line());
        let crossover = named("Crossover");
        assert_eq!(crossover.params, [192, 128, 128]);
        assert_eq!((crossover.bipolar, crossover.oversampling), (false, 2));
        let want = [(0.0, 0.0, 1.0), (0.375, 0.0, 0.0), (0.625, 0.625, -0.005)];
        for (point, (x, y, tension)) in crossover.points().iter().zip(want) {
            assert!((point.x - x).abs() < 1e-6 && (point.y - y).abs() < 1e-6);
            assert!((point.tension - tension).abs() < 1e-6 && point.mode == 0);
        }
        let tanh = named("Kinda tanh");
        assert_eq!(
            (tanh.params[0], tanh.bipolar, tanh.center),
            (64, true, true)
        );
        let fast = named("Fast dist 1");
        assert_eq!((fast.params[0], fast.points().len()), (162, 3));
        // The dead zone of "Crossover": nothing under 0.375 of the table,
        // which the pre amp of 2.33 puts at an input of 0.16. The curve
        // alone, without its four times oversampling.
        let mut crossover = crossover;
        crossover.oversampling = 0;
        assert_eq!(run(&mut crossover, &[0.1, -0.15, 0.16])[..2], [0.0, -0.0]);
        assert!(run(&mut crossover, &[0.2])[0] > 0.08);
        // And it tops out at 0.625.
        assert!((run(&mut crossover, &[0.9])[0] - 0.625).abs() < 1e-4);
        let sine = named("Asymetric half sin shaper");
        assert_eq!(
            (sine.bipolar, sine.oversampling, sine.points().len()),
            (true, 4, 5)
        );
        assert_eq!(sine.points()[1].mode, 9);
        // A mixer preset has the curve with the most points, 16 bytes each.
        let tape = named("Stereo tape simulation");
        assert_eq!(tape.points().len(), 145);

        // The maps are a curve and nothing else.
        let maps = maps(&root);
        assert_eq!(maps.len(), 20);
        for (name, bytes) in &maps {
            let (points, end) = envelope(bytes, 0).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(end, bytes.len(), "{name}");
            assert!(points.len() >= 2, "{name}");
            let length: f64 = points.iter().map(|p| p.x).sum();
            assert!((length - 1.0).abs() < 1e-9, "{name}");
            // And they bake.
            assert_eq!(bake(&Curve::new(&points), false).len(), 16_386, "{name}");
        }
        let default = maps.iter().find(|(name, _)| name == "Default").unwrap();
        assert_eq!(default.1.len(), 56);
        assert_eq!(envelope(&default.1, 0).unwrap().0, line());
    }
}
