//! Fruity Flanger: one tap a side on a delay line with feedback, swept by
//! one LFO that the right side reads a phase further on, damped, and mixed
//! with the input and the other side's tap, as `docs/plugins/fruity-flanger.md`
//! has it.
//!
//! The state is 52 bytes: a little-endian i32 version, 1, and the plugin's
//! twelve parameters as i32. FL leaves a state alone whose version is 2 or
//! more, and so does this.
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Delay | 0 to 1024 | 0 |
//! | 1 Depth | 0 to 5000 | 2000 |
//! | 2 Rate | 0 to 5000 | 2000 |
//! | 3 Phase | 0 to 360 | 118 |
//! | 4 Damp | 0 to 5000 | 0 |
//! | 5 Shape | 0 to 1024 | 0 |
//! | 6 Feed | 0 to 100 | 23 |
//! | 7 Invert feedback | 0 to 1024 | 1024 |
//! | 8 Invert wet | 0 to 1024 | 1024 |
//! | 9 Dry | 0 to 5000 | 3500 |
//! | 10 Wet | 0 to 5000 | 3000 |
//! | 11 Cross | 0 to 5000 | 2500 |
//!
//! With `v` a wheel over its end and `c(v, k) = (k + 1) v / (k v + 1)`: the
//! delay and the depth are `20 c(v, -0.8)` ms each, and the tap swings from
//! the delay to the delay and the depth around their middle. The LFO turns
//! `5 c(v, -0.9)` times a second, and the right side reads it `v` of a
//! cycle on. Its shape goes from a sine to a triangle. What the tap gives
//! goes through a one-pole low-pass, `1 - v` of the way each sample, and
//! that is fed back at `0.98 v` and comes out at the wet wheel, with the
//! other side's at the cross wheel and the input at the dry wheel. The two
//! invert wheels past half turn the feedback and the wet and cross around.
//!
//! FL counts the tap's place in whole numbers with 12 bits under the
//! sample and the LFO in a 24-bit phase, and so does this.

use crate::{Effect, StateError, knob_value, state_version};
use std::sync::LazyLock;

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Flanger";

/// The range of each parameter's wheel.
const RANGES: [(i32, i32); 12] = [
    (0, 1024),
    (0, 5000),
    (0, 5000),
    (0, 360),
    (0, 5000),
    (0, 1024),
    (0, 100),
    (0, 1024),
    (0, 1024),
    (0, 5000),
    (0, 5000),
    (0, 5000),
];
const DEFAULTS: [i32; 12] = [0, 2000, 2000, 118, 0, 0, 23, 1024, 1024, 3500, 3000, 2500];

/// How long the state is, and the newest version FL reads.
const LEN: usize = 52;
const VERSION: i32 = 1;

/// How many steps an LFO table has, and what keeps an index inside it.
const STEPS: usize = 4096;
const MASK: u32 = 0xfff;

/// The rate the LFO counts with until the host gives one, as FL's does.
const FIRST_RATE: u32 = 44_100;

/// What FL has for two pi where it builds the sine table.
const TWO_PI: f64 = f64::from_bits(0x4019_21fb_5441_1744);

/// The two LFO shapes FL blends, each with the step to the next entry:
/// a triangle and a sine, from -32767 to 32767.
struct Shapes {
    triangle: [i32; STEPS],
    sine: [i32; STEPS],
    triangle_slope: [i32; STEPS],
    sine_slope: [i32; STEPS],
}

static SHAPES: LazyLock<Box<Shapes>> = LazyLock::new(|| {
    let mut shapes = Box::new(Shapes {
        triangle: [0; STEPS],
        sine: [0; STEPS],
        triangle_slope: [0; STEPS],
        sine_slope: [0; STEPS],
    });
    // Up to the top over a quarter, down to the bottom over a half, and
    // back up, in 1024ths.
    let entry = |x: f64| (x * 32_767.0) as i32;
    let mut x = 0.0;
    for i in 0..STEPS {
        shapes.triangle[i] = entry(x);
        x += if (1024..3072).contains(&i) {
            -1.0 / 1024.0
        } else {
            1.0 / 1024.0
        };
    }
    for i in 0..STEPS {
        shapes.sine[i] = entry((i as f64 * TWO_PI / 4096.0).sin());
    }
    for i in 0..STEPS {
        let next = (i + 1) % STEPS;
        shapes.triangle_slope[i] = shapes.triangle[next] - shapes.triangle[i];
        shapes.sine_slope[i] = shapes.sine[next] - shapes.sine[i];
    }
    shapes
});

/// FL's wheel curve: `(k + 1) x / (k x + 1)`, in f64 from f32.
fn curve(x: f32, k: f32) -> f32 {
    let num = (f64::from(k) + 1.0) * f64::from(x);
    (num / (f64::from(k * x) + 1.0)) as f32
}

/// `cvtsd2si`: to the nearest, ties to even.
fn round(x: f64) -> i32 {
    x.round_ties_even() as i32
}

/// What the render loop reads, worked out from the wheels and the rate.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Settings {
    /// The middle of the sweep, in 4096ths of a sample.
    center: i32,
    /// Half the depth in samples: an LFO value times this, 3 bits down,
    /// is 4096ths of a sample.
    half: i32,
    /// What a sample adds to the LFO's phase, and how far on the right
    /// side reads it.
    step: i32,
    phase: i32,
    /// How much of the shape is the triangle and how much the sine, in
    /// 4096ths.
    blend: [i32; 2],
    feed: f32,
    damp: f32,
    dry: f32,
    wet: f32,
    cross: f32,
}

impl Settings {
    fn new(params: &[i32; 12], hz: u32) -> Self {
        // The wheel's place times its f64 scale, `1 / (max - min)`.
        let v = |index: usize| {
            let scale = 1.0 / f64::from(RANGES[index].1);
            (f64::from(params[index]) * scale) as f32
        };
        let rate = f64::from(hz);
        let delay = curve(v(0), -0.8) * 20.0;
        let depth = curve(v(1), -0.8) * 20.0;
        let lfo_hz = curve(v(2), -0.9) * 5.0;
        let feed = if f64::from(v(7)) > 0.5 { -v(6) } else { v(6) };
        let turn = if f64::from(v(8)) > 0.5 { -1.0 } else { 1.0 };
        Settings {
            center: ((f64::from(delay) + f64::from(depth) * 0.5) * rate * 4096.0 / 1000.0) as i32,
            half: (f64::from(depth) * 0.5 * rate / 1000.0) as i32,
            step: round(f64::from(lfo_hz * 4096.0 / hz as f32 * 4096.0)),
            phase: round(f64::from(v(3) * 16_777_216.0)),
            blend: [
                round(f64::from(v(5) * 4096.0)),
                round(f64::from(1.0 - v(5)) * 4096.0),
            ],
            feed: (f64::from(feed) * 0.98) as f32,
            damp: (1.0 - f64::from(v(4))) as f32,
            dry: v(9),
            wet: turn * v(10),
            cross: turn * v(11),
        }
    }
}

/// The LFO's table for one blend of the two shapes, with its slopes.
#[derive(Debug, Clone, PartialEq)]
struct Table {
    values: Box<[i32; STEPS]>,
    slopes: Box<[i32; STEPS]>,
    blend: [i32; 2],
}

impl Table {
    fn new() -> Self {
        Table {
            values: Box::new([0; STEPS]),
            slopes: Box::new([0; STEPS]),
            blend: [i32::MIN; 2],
        }
    }

    /// Blend the shapes `[triangle, sine]` 4096ths each. Nothing is asked
    /// for.
    fn blend(&mut self, [a, b]: [i32; 2]) {
        if self.blend == [a, b] {
            return;
        }
        self.blend = [a, b];
        let shapes = &**SHAPES;
        for i in 0..STEPS {
            self.values[i] = a
                .wrapping_mul(shapes.triangle[i])
                .wrapping_add(b.wrapping_mul(shapes.sine[i]));
            let slope = a
                .wrapping_mul(shapes.triangle_slope[i])
                .wrapping_add(b.wrapping_mul(shapes.sine_slope[i]));
            self.slopes[i] = slope >> 12;
        }
    }

    /// The LFO at a 24-bit phase, from -32767 to 32767, between two
    /// entries.
    fn at(&self, phase: u32) -> i32 {
        let i = ((phase >> 12) & MASK) as usize;
        let under = (phase & MASK) as i32;
        self.values[i].wrapping_add(under.wrapping_mul(self.slopes[i])) >> 12
    }
}

/// Fruity Flanger. A new one has FL's defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct Flanger {
    params: [i32; 12],
    hz: u32,
    settings: Settings,
    table: Table,
    /// The LFO's phase on the left: 2^24 is one cycle.
    phase: u32,
    /// The damped taps, which are also what is fed back.
    taps: [f32; 2],
    rings: [Vec<f32>; 2],
    mask: u32,
    write: u32,
}

impl Default for Flanger {
    fn default() -> Self {
        LazyLock::force(&SHAPES);
        let mut plugin = Flanger {
            params: DEFAULTS,
            hz: FIRST_RATE,
            settings: Settings::new(&DEFAULTS, FIRST_RATE),
            table: Table::new(),
            phase: 0,
            taps: [0.0; 2],
            rings: [Vec::new(), Vec::new()],
            mask: 0,
            write: 0,
        };
        plugin.make_room();
        plugin.set(DEFAULTS);
        plugin
    }
}

impl Flanger {
    /// Take parameters that are inside their ranges, or a new rate.
    fn set(&mut self, params: [i32; 12]) {
        self.params = params;
        self.settings = Settings::new(&params, self.hz);
        self.table.blend(self.settings.blend);
    }

    /// The lines, a power of two at least a tenth of a second long. This
    /// allocates.
    fn make_room(&mut self) {
        let wanted = round(f64::from(self.hz) / 10.0);
        let mut len = 1u32;
        while (len as i32) < wanted {
            len <<= 1;
        }
        self.rings = [vec![0.0; len as usize], vec![0.0; len as usize]];
        self.mask = len - 1;
        self.write = 0;
    }

    /// FL's flush: the lines and the low-passes emptied, the LFO at the
    /// start of its cycle.
    fn flush(&mut self) {
        for ring in &mut self.rings {
            ring.fill(0.0);
        }
        self.write = 0;
        self.phase = 0;
        self.taps = [0.0; 2];
    }

    /// The ring at `back` 4096ths of a sample behind the write position,
    /// between two samples.
    fn tap(ring: &[f32], write: u32, mask: u32, back: u32) -> f32 {
        let at = write.wrapping_sub(back >> 12);
        let near = ring[(at & mask) as usize];
        let far = ring[(at.wrapping_sub(1) & mask) as usize];
        near + (far - near) * ((back & MASK) as f32 / 4096.0)
    }
}

impl Effect for Flanger {
    /// The lines and the LFO stay as they are, as in FL.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let version = state_version(bytes, LEN)?;
        if version > VERSION {
            return Err(StateError::Version { found: version });
        }
        if bytes.len() != LEN {
            return Err(StateError::Length {
                expected: LEN,
                found: bytes.len(),
            });
        }
        let mut params = [0; 12];
        for (param, word) in bytes[4..].as_chunks::<4>().0.iter().enumerate() {
            let value = i32::from_le_bytes(*word);
            let (min, max) = RANGES[param];
            if value < min || value > max {
                return Err(StateError::Range { param, value });
            }
            params[param] = value;
        }
        self.set(params);
        Ok(())
    }

    /// FL counts the rate in whole Hz. A rate that is not a number of 1 Hz
    /// or more is not taken. A new rate makes new lines, empty.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz >= 1.0 {
            let hz = hz.round() as u32;
            if hz != self.hz {
                self.hz = hz;
                self.make_room();
            }
            self.set(self.params);
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let s = self.settings;
        let offset = s.phase as u32;
        let [ring_l, ring_r] = &mut self.rings;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let (in_l, in_r) = (*l, *r);
            let sweep = [
                self.table.at(self.phase),
                self.table.at(self.phase.wrapping_add(offset)),
            ];
            self.phase = self.phase.wrapping_add(s.step as u32);
            let back = sweep.map(|m| {
                let swing = i64::from(s.half.wrapping_mul(m) >> 3);
                (swing + i64::from(s.center)) as u32
            });
            let w = self.write as usize;
            ring_l[w] = in_l + self.taps[0] * s.feed;
            ring_r[w] = in_r + self.taps[1] * s.feed;
            let tap_l = Self::tap(ring_l, self.write, self.mask, back[0]);
            let tap_r = Self::tap(ring_r, self.write, self.mask, back[1]);
            self.write = (self.write + 1) & self.mask;
            self.taps[0] += s.damp * (tap_l - self.taps[0]);
            self.taps[1] += s.damp * (tap_r - self.taps[1]);
            let [t_l, t_r] = self.taps;
            *l = t_l * s.wet + in_l * s.dry + s.cross * t_r;
            *r = t_r * s.wet + in_r * s.dry + s.cross * t_l;
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

    /// Empties the lines and starts the LFO again.
    fn reset(&mut self) {
        self.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{every_preset, presets, sine, state};

    fn bytes(params: [i32; 12]) -> Vec<u8> {
        let mut bytes = state(&[1]);
        bytes.extend(state(&params));
        bytes
    }

    fn with(params: [i32; 12]) -> Flanger {
        let mut plugin = Flanger::default();
        plugin.set_state(&bytes(params)).unwrap();
        plugin
    }

    /// The doc's chain for both sides in f64, with the tap `back` samples
    /// (a fraction) behind the sample that just went in, for every frame.
    fn model(
        input: &[(f32, f32)],
        s: &Settings,
        back: impl Fn(usize) -> [f64; 2],
    ) -> Vec<(f64, f64)> {
        let mut lines = [Vec::new(), Vec::new()];
        let mut taps = [0.0f64; 2];
        let (feed, damp) = (f64::from(s.feed), f64::from(s.damp));
        let mut out = Vec::new();
        for (n, (l, r)) in input.iter().enumerate() {
            let x = [f64::from(*l), f64::from(*r)];
            let back = back(n);
            let mut got = [0.0; 2];
            for side in 0..2 {
                lines[side].push(x[side] + taps[side] * feed);
                let line = &lines[side];
                let place = n as f64 - back[side];
                let whole = place.floor();
                let get = |i: f64| if i < 0.0 { 0.0 } else { line[i as usize] };
                // FL reads from the newer sample towards the older.
                let tap =
                    get(whole + 1.0) + (get(whole) - get(whole + 1.0)) * (whole + 1.0 - place);
                got[side] = tap;
            }
            for side in 0..2 {
                taps[side] += damp * (got[side] - taps[side]);
            }
            let (wet, dry, cross) = (f64::from(s.wet), f64::from(s.dry), f64::from(s.cross));
            out.push((
                taps[0] * wet + x[0] * dry + cross * taps[1],
                taps[1] * wet + x[1] * dry + cross * taps[0],
            ));
        }
        out
    }

    fn worst(got: &[(f32, f32)], wanted: &[(f64, f64)]) -> f64 {
        let apart = got
            .iter()
            .zip(wanted)
            .map(|((l, r), (wl, wr))| (f64::from(*l) - wl).abs().max((f64::from(*r) - wr).abs()));
        apart.fold(0.0, f64::max)
    }

    fn run(plugin: &mut Flanger, input: &[(f32, f32)], block: usize) -> Vec<(f32, f32)> {
        let mut l: Vec<f32> = input.iter().map(|p| p.0).collect();
        let mut r: Vec<f32> = input.iter().map(|p| p.1).collect();
        for (l, r) in l.chunks_mut(block).zip(r.chunks_mut(block)) {
            plugin.process(l, r);
        }
        l.into_iter().zip(r).collect()
    }

    #[test]
    fn the_shapes_are_a_triangle_and_a_sine() {
        let shapes = &**SHAPES;
        let t = &shapes.triangle;
        assert_eq!(
            (t[0], t[512], t[1024], t[2048], t[3072]),
            (0, 16_383, 32_767, 0, -32_767)
        );
        assert_eq!((t[1], t[4095]), (31, -31));
        let s = &shapes.sine;
        assert_eq!((s[0], s[1024], s[2048], s[3072]), (0, 32_767, 0, -32_767));
        assert_eq!(s[512], (std::f64::consts::FRAC_1_SQRT_2 * 32_767.0) as i32);
        // The slopes come round from the last entry to the first.
        assert_eq!(shapes.triangle_slope[4095], 31);
        assert_eq!(shapes.sine_slope[4095], -s[4095]);
        assert_eq!(shapes.sine_slope[10], s[11] - s[10]);
    }

    #[test]
    fn the_lfo_blends_the_shapes_and_reads_between_entries() {
        let mut table = Table::new();
        // Shape at 0 is all sine.
        table.blend([0, 4096]);
        assert_eq!(table.at(1024 << 12), 32_767);
        assert_eq!(table.at(0), 0);
        // Half way between two entries.
        let s = &SHAPES.sine;
        assert_eq!(table.at((100 << 12) + 2048), (s[100] + s[101]) / 2);
        table.blend([4096, 0]);
        assert_eq!(table.at(512 << 12), 16_383);
        assert_eq!(table.at(3072 << 12), -32_767);
        // Half and half.
        table.blend([2048, 2048]);
        assert_eq!(table.at(1024 << 12), 32_767);
    }

    #[test]
    fn the_wheels_come_to_the_doc_s_numbers() {
        let s = Flanger::default().settings;
        // Delay 0, depth c(0.4, -0.8) of 20 ms.
        let depth: f64 = 20.0 * 0.2 * 0.4 / (1.0 - 0.8 * 0.4);
        assert!((depth - 2.3529).abs() < 1e-3);
        assert_eq!(s.half, (depth / 2.0 * 44.1) as i32);
        assert!((f64::from(s.center) / 4096.0 - depth / 2.0 * 44.1).abs() < 0.01);
        // c(0.4, -0.9) of 5 Hz: 0.3125 Hz.
        let hz: f64 = 5.0 * 0.1 * 0.4 / (1.0 - 0.9 * 0.4);
        let turns = f64::from(s.step) * 44_100.0 / 16_777_216.0;
        assert!((turns - hz).abs() < 1e-3, "{turns}");
        // 118 of 360 degrees.
        assert!((f64::from(s.phase) - 118.0 / 360.0 * 16_777_216.0).abs() < 2.0);
        assert_eq!(s.blend, [0, 4096]);
        // Inverted feedback and wet.
        assert!((s.feed + 0.23 * 0.98).abs() < 1e-6);
        assert_eq!((s.damp, s.dry), (1.0, 0.7));
        assert!((s.wet + 0.6).abs() < 1e-7 && (s.cross + 0.5).abs() < 1e-7);

        let ends = with([1024, 5000, 5000, 360, 5000, 1024, 100, 512, 512, 0, 5000, 0]).settings;
        assert!((f64::from(ends.center) / 4096.0 - 30.0 * 44.1).abs() < 0.01);
        assert_eq!(ends.half, (10.0 * 44.1) as i32);
        assert!((f64::from(ends.step) * 44_100.0 / 16_777_216.0 - 5.0).abs() < 1e-3);
        assert_eq!((ends.phase, ends.blend), (1 << 24, [4096, 0]));
        assert!((ends.feed - 0.98).abs() < 1e-7);
        assert_eq!((ends.damp, ends.wet, ends.cross), (0.0, 1.0, 0.0));
    }

    #[test]
    fn with_the_depth_at_0_it_is_a_comb_with_feedback() {
        let tone: Vec<(f32, f32)> = sine(0.013, 4000)
            .into_iter()
            .zip(sine(0.031, 4000))
            .collect();
        for params in [
            [300, 0, 2000, 118, 0, 0, 70, 0, 0, 3500, 3000, 2500],
            [1024, 0, 0, 0, 2000, 0, 100, 1024, 1024, 5000, 5000, 0],
            [17, 0, 5000, 300, 4999, 512, 23, 1024, 0, 0, 2000, 4000],
        ] {
            let mut plugin = with(params);
            let s = plugin.settings;
            assert_eq!(s.half, 0);
            let late = f64::from(s.center) / 4096.0;
            let wanted = model(&tone, &s, |_| [late; 2]);
            let got = run(&mut plugin, &tone, 300);
            assert!(worst(&got, &wanted) < 1e-4, "{params:?}");
        }
    }

    #[test]
    fn the_tap_follows_the_lfo_and_the_right_side_reads_it_further_on() {
        let params = [
            200, 3000, 4000, 90, 1000, 300, 60, 0, 1024, 2500, 4000, 1000,
        ];
        let mut plugin = with(params);
        let s = plugin.settings;
        let table = plugin.table.clone();
        let back = |n: usize| -> [f64; 2] {
            let phase = (s.step as u32).wrapping_mul(n as u32);
            [phase, phase.wrapping_add(s.phase as u32)].map(|p| {
                let m = table.at(p);
                f64::from(s.center + ((s.half * m) >> 3)) / 4096.0
            })
        };
        // The sweep covers the depth around its middle, but for the half
        // depth FL counts in whole samples, and a quarter
        // cycle apart the sides are not the same.
        let (least, most) = (0..80_000)
            .map(|n| back(n)[0])
            .fold((f64::MAX, 0f64), |(a, b), d| (a.min(d), b.max(d)));
        let c = |x: f64| 0.2 * x / (1.0 - 0.8 * x);
        let (delay, depth) = (20.0 * c(200.0 / 1024.0) * 44.1, 20.0 * c(0.6) * 44.1);
        assert!(
            (least - delay).abs() < 1.0 && (most - delay - depth).abs() < 1.0,
            "{least} {most} {delay} {depth}"
        );
        assert!((back(0)[1] - back(0)[0]).abs() > 1.0);

        let tone: Vec<(f32, f32)> = sine(0.011, 20_000)
            .into_iter()
            .zip(sine(0.0071, 20_000))
            .collect();
        let wanted = model(&tone, &s, back);
        let got = run(&mut plugin, &tone, 512);
        assert!(worst(&got, &wanted) < 1e-4);
    }

    #[test]
    fn the_damp_is_a_one_pole_low_pass_in_the_loop() {
        // No feedback, no dry, no cross: the wet is the tap through the
        // low-pass, and a level that stays comes through whole.
        let mut plugin = with([0, 0, 0, 0, 4500, 0, 0, 0, 0, 0, 5000, 0]);
        let got = run(&mut plugin, &[(1.0, -0.5); 3000], 100);
        assert!((got[2999].0 - 1.0).abs() < 1e-5 && (got[2999].1 + 0.5).abs() < 1e-5);
        // The first samples climb by a tenth of the way each.
        assert!((got[0].0 - 0.1).abs() < 1e-6);
        assert!((got[1].0 - 0.19).abs() < 1e-6);
    }

    #[test]
    fn feedback_builds_a_level_up_to_one_over_one_less_it() {
        let mut plugin = with([0, 0, 0, 0, 0, 0, 50, 0, 0, 0, 5000, 0]);
        let got = run(&mut plugin, &[(0.5, 0.5); 4000], 256);
        assert!((got[3999].0 - 0.5 / (1.0 - 0.49)).abs() < 1e-4);
        // Inverted, it settles under the input.
        let mut plugin = with([0, 0, 0, 0, 0, 0, 50, 1024, 0, 0, 5000, 0]);
        let got = run(&mut plugin, &[(0.5, 0.5); 4000], 256);
        assert!((got[3999].0 - 0.5 / 1.49).abs() < 1e-4);
    }

    #[test]
    fn a_flush_empties_the_lines_and_starts_the_lfo_again() {
        let mut plugin = Flanger::default();
        run(&mut plugin, &[(0.3, 0.2); 1000], 100);
        assert!(plugin.phase != 0 && plugin.taps != [0.0; 2]);
        plugin.reset();
        assert_eq!((plugin.phase, plugin.write, plugin.taps), (0, 0, [0.0; 2]));
        assert!(plugin.rings.iter().all(|r| r.iter().all(|v| *v == 0.0)));
        // A tenth of a second at 44.1 kHz is 4410, in 8192.
        assert_eq!(plugin.rings[0].len(), 8192);
    }

    #[test]
    fn the_rate_moves_the_delays_and_the_lfo_step() {
        let mut plugin = Flanger::default();
        let slow = plugin.settings;
        plugin.set_sample_rate(88_200.0);
        let fast = plugin.settings;
        assert!((fast.center - 2 * slow.center).abs() <= 1);
        assert!((fast.half - 2 * slow.half).abs() <= 1);
        assert!((2 * fast.step - slow.step).abs() <= 1);
        assert_eq!(plugin.rings[0].len(), 16_384);
        let before = plugin.clone();
        plugin.set_sample_rate(f32::NAN);
        plugin.set_sample_rate(0.0);
        assert_eq!(plugin, before);
        // The longest sweep fits the line at every rate.
        let mut plugin = with([1024, 5000, 5000, 360, 0, 1024, 100, 0, 0, 5000, 5000, 5000]);
        plugin.set_sample_rate(192_000.0);
        let (l, r) = (sine(0.01, 9000), sine(0.013, 9000));
        let got = run(&mut plugin, &l.into_iter().zip(r).collect::<Vec<_>>(), 333);
        assert!(got.iter().all(|(l, r)| l.is_finite() && r.is_finite()));
    }

    #[test]
    fn a_moved_wheel_asks_for_no_memory_and_is_the_same_plugin_as_its_state() {
        let mut plugin = Flanger::default();
        let memory = |p: &Flanger| (p.rings[0].as_ptr(), p.table.values.as_ptr());
        let before = memory(&plugin);
        let moves = [
            1024.4,
            -3.0,
            2000.0,
            360.0,
            17.0,
            600.0,
            99.6,
            1024.0,
            f32::NAN,
            100.0,
            4999.6,
            300.0,
        ];
        for (index, value) in moves.into_iter().enumerate() {
            plugin.set_param(index, value);
            plugin.process(&mut [0.25; 200], &mut [0.25; 200]);
        }
        plugin.set_param(12, 1.0);
        assert_eq!((plugin.param(12), plugin.param_count()), (0.0, 12));
        let wanted = [1024, 0, 2000, 360, 17, 600, 100, 1024, 0, 100, 5000, 300];
        let params: Vec<f32> = (0..12).map(|i| plugin.param(i)).collect();
        assert_eq!(params, wanted.map(|v| v as f32));
        assert_eq!(memory(&plugin), before);
        assert_eq!(plugin.settings, with(wanted).settings);
        assert_eq!(plugin.table, with(wanted).table);
    }

    #[test]
    fn a_state_the_plugin_does_not_write_is_turned_down() {
        let mut plugin = with([112, 1062, 2000, 119, 0, 0, 30, 0, 0, 3500, 3000, 2500]);
        let before = plugin.clone();
        for len in [3, 48, 56] {
            let mut wrong = bytes(DEFAULTS);
            wrong.resize(len, 0);
            assert_eq!(
                plugin.set_state(&wrong),
                Err(StateError::Length {
                    expected: 52,
                    found: len
                })
            );
        }
        for (param, value) in [(0, 1025), (3, 361), (6, 101), (11, -1)] {
            let mut params = DEFAULTS;
            params[param] = value;
            assert_eq!(
                plugin.set_state(&bytes(params)),
                Err(StateError::Range { param, value })
            );
        }
        let mut newer = bytes(DEFAULTS);
        newer[0] = 2;
        assert_eq!(
            plugin.set_state(&newer),
            Err(StateError::Version { found: 2 })
        );
        assert_eq!(plugin, before);
    }

    #[test]
    fn fl_s_presets_load() {
        let Some(own) = presets(NAME) else { return };
        let all = every_preset(&crate::testing::corpus().unwrap(), NAME);
        assert!(own.len() >= 27);
        for preset in &all {
            let mut plugin = Flanger::default();
            let loaded = plugin.set_state(&preset.state);
            if preset.file == "Flanger send" {
                // Twelve floats of an older build after a version of 0:
                // 48 bytes, where FL reads 52.
                assert_eq!(
                    loaded,
                    Err(StateError::Length {
                        expected: 52,
                        found: 48
                    })
                );
                continue;
            }
            assert_eq!(loaded, Ok(()), "{}", preset.file);
            let (mut l, mut r) = (sine(0.01, 4000), sine(0.02, 4000));
            plugin.process(&mut l, &mut r);
            assert!(l.iter().chain(&r).all(|v| v.is_finite() && v.abs() < 20.0));
        }
    }
}
