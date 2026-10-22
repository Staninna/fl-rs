//! What Sakura's engine does after its voices, as
//! `docs/plugins/sakura.md` reads it out of `engine_x64.dll`: a highpass,
//! the eight resonators, the chorus, the reverb and the delay, in that
//! order, on the host's rate.
//!
//! Every delay line here is the engine's stamped line (`CStringDelay`,
//! `CTapeDelay`): each sample is written with the time it came at, in
//! fractions of a turn of the line, and read back one turn later between
//! the two samples either side. The reverb (`fxReverb`) is a plate:
//! four allpasses, a predelay and a figure-of-eight tank, with fixed
//! lengths of FL's own.

use std::f64::consts::TAU;

/// The engine's flush to zero (`0x180077c3c`).
const TINY: f32 = 1e-9;

/// The engine's `sincosf` (`0x1800579d0`): the cosine is the sine of
/// `|x| + pi/2` rounded to an f32, and for `|x|` under 2^-13 the pair is
/// `x` and `1 - x^2 / 2`.
pub(crate) fn sin_cos_f32(x: f32) -> (f32, f32) {
    let a = x.abs();
    if a >= 1.220_703_1e-4 {
        (x.sin(), (a + std::f32::consts::FRAC_PI_2).sin())
    } else {
        (x, 1.0 - a * a * 0.5)
    }
}

/// The engine's `sincos` (`0x180057960`), the same in f64 with 2^-27.
pub(crate) fn sin_cos_f64(x: f64) -> (f64, f64) {
    let a = x.abs();
    if a >= 7.450_580_596_923_828e-9 {
        (x.sin(), (a + std::f64::consts::FRAC_PI_2).sin())
    } else {
        (x, 1.0 - a * a * 0.5)
    }
}

fn flush(x: f32) -> f32 {
    if x < TINY && x > -TINY { 0.0 } else { x }
}

/// The engine's biquad highpass (`0x180019d50` design, `0x180019e50`
/// run): zeros at DC, a pole pair from `w` and `q`.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct HighPass {
    b0: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl HighPass {
    pub(crate) fn new(hz: f32, q: f32, rate: f32) -> Self {
        let mut filter = HighPass::default();
        filter.set(hz, q, rate);
        filter
    }

    pub(crate) fn set(&mut self, hz: f32, q: f32, rate: f32) {
        let w = (f64::from(hz) * TAU / f64::from(rate)) as f32;
        let (s, c) = sin_cos_f32(w);
        let k = 1.0 / (s + q);
        self.b0 = flush((c + 1.0) / (s / q + 2.0));
        self.a1 = flush(c * k * q * -2.0);
        self.a2 = flush((q - s) * k);
    }

    pub(crate) fn clear(&mut self) {
        (self.x1, self.x2, self.y1, self.y2) = (0.0, 0.0, 0.0, 0.0);
    }

    pub(crate) fn run(&mut self, x: f32) -> f32 {
        let x = flush(x);
        let y = ((x - (self.x1 + self.x1)) + self.x2) * self.b0
            - self.y1 * self.a1
            - self.a2 * self.y2;
        let y = flush(y);
        self.x2 = self.x1;
        self.y2 = self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }
}

/// One of a stamped line's two halves (`0x180023640`): the samples and
/// the times they came at, from index 10 on, with three samples of the
/// half before at 7 to 9.
#[derive(Debug, Clone, Default)]
struct Half {
    value: Vec<f32>,
    stamp: Vec<f64>,
    count: usize,
    read: usize,
}

/// The line both of the engine's delays are made of. A turn of the line
/// is a phase from 0 to 1; what is written in one turn is read in the
/// next, at the same phase.
#[derive(Debug, Clone, Default)]
struct Stamped {
    halves: [Half; 2],
    /// Which half is written to.
    write: usize,
    phase: f64,
}

impl Stamped {
    fn new(points: usize) -> Self {
        let half = || Half {
            value: vec![0.0; points + 256],
            stamp: vec![0.0; points + 256],
            count: 0,
            read: 0,
        };
        Stamped {
            halves: [half(), half()],
            write: 0,
            phase: 0.0,
        }
    }

    fn clear(&mut self) {
        for half in &mut self.halves {
            half.value.fill(0.0);
            half.stamp.fill(0.0);
            half.count = 0;
            half.read = 0;
        }
    }

    /// What was written a turn ago, between the samples either side.
    fn read(&mut self) -> f32 {
        let phase = self.phase;
        let half = &mut self.halves[1 - self.write];
        if half.count == 0 {
            return 0.0;
        }
        loop {
            let k = half.read;
            if k >= half.count + 1 {
                return 0.0;
            }
            let after = half.stamp[10 + k];
            if phase <= after {
                let before = half.stamp[9 + k] as f32;
                let after = after as f32;
                let along = if before < after {
                    (phase - f64::from(before)) / f64::from(after - before)
                } else {
                    0.0
                };
                let (a, b) = (half.value[9 + k], half.value[10 + k]);
                let out = (f64::from(b - a) * along + f64::from(a)) as f32;
                return flush(out);
            }
            half.read = k + 1;
        }
    }

    /// Move the phase on by `step` and write `value` at it, turning over
    /// to the other half at 1.
    fn write(&mut self, value: f32, step: f64) {
        self.phase += step;
        if self.phase >= 1.0 {
            let (w, r) = (self.write, 1 - self.write);
            let [a, b] = &mut self.halves;
            let (written, read) = if w == 0 { (a, b) } else { (b, a) };
            let last = read.count;
            for j in [9, 8, 7] {
                written.value[j] = read.value[j + last];
                written.stamp[j] = read.stamp[j + last] - 1.0;
            }
            written.value[10 + written.count] = value;
            written.stamp[10 + written.count] = self.phase;
            written.read = 0;
            self.write = r;
            self.phase -= 1.0;
            self.halves[r].count = 0;
        }
        let half = &mut self.halves[self.write];
        half.value[10 + half.count] = value;
        half.stamp[10 + half.count] = self.phase;
        half.count += 1;
    }
}

/// The step a line of `points` moves by for a delay of `position` of its
/// length (`0x1800257e0`, `0x180023d50`): `position * (points - 2) + 2`
/// samples.
fn step_for(position: f64, points: f64) -> f64 {
    let position = position.min(0.99).max(0.0);
    let two = 2.0 / points;
    1.0 / ((position * (1.0 - two) + two) * points)
}

/// A resonator's line (`CStringDelay`): a delay of up to a tenth of a
/// second through a one-pole lowpass of 0.5 back into itself.
#[derive(Debug, Clone, Default)]
struct StringLine {
    line: Stamped,
    points: f64,
    step: f64,
    least: f64,
    low: f32,
    feedback: f32,
}

impl StringLine {
    fn new(points: i32) -> Self {
        let points = points.max(4);
        let mut line = StringLine {
            line: Stamped::new(points as usize),
            points: f64::from(points),
            least: 1.0 / f64::from(points),
            ..StringLine::default()
        };
        line.place(0.5);
        line
    }

    fn place(&mut self, position: f64) {
        let mut step = step_for(position, self.points);
        if step > 0.2 {
            step = 0.2;
        }
        if step < self.least {
            step = self.least;
        }
        self.step = step;
    }

    fn clear(&mut self) {
        self.low = 0.0;
        self.line.phase = 0.0;
        self.line.clear();
    }

    fn run(&mut self, x: f32) -> f32 {
        let x = flush(x);
        let out = self.line.read();
        self.low += (out - self.low) * 0.5;
        let value = self.low * self.feedback + x;
        self.line.write(value, self.step);
        out
    }
}

/// The eight resonators (`0x180024f20`): each two lines in a row, the
/// second 1.05 times as long and fed the first one's output turned over,
/// panned left and right in turn, and a highpass at 32 Hz on each side.
#[derive(Debug, Clone, Default)]
pub(crate) struct Resonators {
    pub(crate) position: [f32; 8],
    pub(crate) feedback: [f32; 8],
    pub(crate) on: [bool; 8],
    lines: Vec<(StringLine, StringLine)>,
    left: HighPass,
    right: HighPass,
}

impl Resonators {
    pub(crate) fn set_sample_rate(&mut self, rate: f64) {
        let points = (rate * 0.1) as i32;
        self.lines = (0..8)
            .map(|_| (StringLine::new(points), StringLine::new(points)))
            .collect();
        self.left = HighPass::new(32.0, 1.0, rate as f32);
        self.right = HighPass::new(32.0, 1.0, rate as f32);
    }

    /// Turning a resonator on, even one that is on, clears its lines.
    pub(crate) fn switch(&mut self, n: usize, on: bool) {
        self.on[n] = on;
        if on && let Some((a, b)) = self.lines.get_mut(n) {
            a.clear();
            b.clear();
        }
    }

    pub(crate) fn clear(&mut self) {
        for (a, b) in &mut self.lines {
            a.clear();
            b.clear();
        }
        self.left.clear();
        self.right.clear();
    }

    pub(crate) fn run(&mut self, left: &mut [f32], right: &mut [f32]) {
        if self.lines.is_empty() {
            return;
        }
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let x = *l + *r;
            let (mut to_left, mut to_right) = (0.0f32, 0.0f32);
            for n in 0..8 {
                if !self.on[n] {
                    continue;
                }
                let (a, b) = &mut self.lines[n];
                let position = f64::from(self.position[n]);
                a.place(position);
                b.place(position * 1.05);
                a.feedback = self.feedback[n];
                b.feedback = self.feedback[n];
                let y = a.run(x);
                let y = b.run(-y);
                let odd = (n & 1) as f32;
                to_left += (odd * 0.5 + 0.75) * y;
                to_right += ((1.0 - odd) * 0.5 + 0.75) * y;
            }
            *l += self.left.run(to_left);
            *r += self.right.run(to_right);
        }
    }
}

/// The engine's modulated line (`CTapeDelay`, `0x1800239b0`): the step
/// moves toward its target by 0.005 a sample and is sped up by a
/// smoothstep of a triangle LFO, the output gain moves toward its target
/// by 0.01 a sample.
#[derive(Debug, Clone, Default)]
struct Tape {
    line: Stamped,
    points: f64,
    least: f64,
    start: f64,
    lfo: f64,
    lfo_step: f64,
    depth: f32,
    target: f64,
    step: f64,
    feedback: f32,
    gain_target: f32,
    gain: f32,
}

impl Tape {
    fn new(points: u32, start: f64) -> Self {
        let points = points.max(4);
        let mut tape = Tape {
            line: Stamped::new(points as usize),
            points: f64::from(points),
            least: 1.0 / f64::from(points),
            start,
            gain_target: 1.0,
            ..Tape::default()
        };
        tape.place(0.5);
        tape.clear();
        tape
    }

    fn place(&mut self, position: f64) {
        self.target = step_for(position, self.points);
    }

    fn set_rate(&mut self, hz: f64, rate: f64) {
        self.lfo_step = hz.max(0.0) / rate;
    }

    fn set_depth(&mut self, depth: f64) {
        self.depth = depth.min(1.0).max(0.0) as f32;
    }

    fn clear(&mut self) {
        self.line.phase = self.start;
        self.step = self.target;
        self.gain = 0.0;
        self.lfo = 0.0;
        self.line.clear();
    }

    fn run(&mut self, x: f32) -> f32 {
        let x = flush(x);
        let lfo = self.lfo;
        let tri = if lfo >= 0.5 {
            1.0 - (((lfo - 0.5) + lfo) - 0.5)
        } else {
            lfo + lfo
        };
        let next = lfo + self.lfo_step;
        let mut speed = (3.0 - (tri + tri)) * tri * f64::from(self.depth) * tri + 1.0;
        if speed <= 1.0 {
            speed = 1.0;
        }
        self.lfo = if next >= 1.0 {
            next - f64::from(next as i32)
        } else {
            next
        };
        let out = self.line.read();
        let step = (speed * self.step).min(0.2).max(self.least);
        let value = out * self.feedback + x;
        self.line.write(value, step);
        let result = out * self.gain;
        self.gain = flush((f64::from(self.gain_target - self.gain) * 0.01 + f64::from(self.gain)) as f32);
        self.step += (self.target - self.step) * 0.005;
        result
    }
}

/// The chorus (`CChorus`): three modulated lines of up to 22, 17 and
/// 20 ms on the sum of both sides, the first to the left, the third to
/// the right and the second to both.
#[derive(Debug, Clone, Default)]
pub(crate) struct Chorus {
    pub(crate) on: bool,
    rate: f32,
    depth: f32,
    wet: f32,
    sample_rate: f64,
    lines: Vec<Tape>,
}

impl Chorus {
    pub(crate) fn set_sample_rate(&mut self, rate: f64) {
        self.sample_rate = rate;
        let span = rate * f64::from(0.02f32);
        self.lines = vec![
            Tape::new((span * 1.1) as i64 as u32, 0.0),
            Tape::new((span * 0.854) as i64 as u32, 0.467),
            Tape::new(span as i64 as u32, 0.778),
        ];
        self.set_wet(self.wet);
        self.set_rate(self.rate);
        self.set_depth(self.depth);
        for line in &mut self.lines {
            line.clear();
        }
    }

    pub(crate) fn set_rate(&mut self, v: f32) {
        self.rate = v;
        let hz = f64::from(v) * 5.0 + 0.1;
        let rate = self.sample_rate;
        for (line, scale) in self.lines.iter_mut().zip([0.896, 1.0, 0.689]) {
            line.set_rate(hz * scale, rate);
        }
    }

    pub(crate) fn set_depth(&mut self, v: f32) {
        self.depth = v;
        for line in &mut self.lines {
            line.set_depth(f64::from(v * v * v));
        }
    }

    /// FL stores the wet knob turned over and hands that to the lines
    /// again when the rate changes; a project's rate is set before its
    /// state, so the lines hear the knob as it is.
    pub(crate) fn set_wet(&mut self, v: f32) {
        self.wet = v;
        for line in &mut self.lines {
            line.gain_target = v;
        }
    }

    pub(crate) fn clear(&mut self) {
        for line in &mut self.lines {
            line.clear();
        }
    }

    pub(crate) fn run(&mut self, left: &mut [f32], right: &mut [f32]) {
        if !self.on || self.lines.is_empty() {
            return;
        }
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let x = *r + *l;
            let a = self.lines[0].run(x);
            let c = self.lines[2].run(x);
            let b = self.lines[1].run(x);
            *l += b + a;
            *r += b + c;
        }
    }
}

/// The delay (`0x18002c700`, from `0x18002cbe0`): three modulated lines in
/// a row on each side, each adding to what it is fed, the second and
/// third at Blur and Blur squared of the wet level.
#[derive(Debug, Clone, Default)]
pub(crate) struct Echo {
    pub(crate) on: bool,
    pub(crate) time: f32,
    pub(crate) feedback: f32,
    pub(crate) speed: f32,
    pub(crate) depth: f32,
    pub(crate) blur: f32,
    pub(crate) wet: f32,
    /// Delay sync, read as a whole number: 0 free, 1 to 9 a note length
    /// of the tempo.
    pub(crate) sync: f32,
    /// The song's tempo, which FL hands the engine every block
    /// (`0x18002cc60`); 120 until it does.
    pub(crate) tempo: f64,
    sample_rate: f64,
    lines: Vec<Tape>,
}

impl Echo {
    pub(crate) fn set_sample_rate(&mut self, rate: f64) {
        self.sample_rate = rate;
        let points = (rate + rate) as i64;
        let whole = points as u32;
        let span = f64::from(whole);
        self.lines = vec![
            Tape::new(whole, 0.0),
            Tape::new((span * 0.63) as i64 as u32, 0.27),
            Tape::new((span * 0.41) as i64 as u32, 0.4),
            Tape::new(whole, 0.3),
            Tape::new((span * 0.76) as i64 as u32, 0.62),
            Tape::new((span * 0.54) as i64 as u32, 0.87),
        ];
    }

    pub(crate) fn clear(&mut self) {
        for line in &mut self.lines {
            line.clear();
        }
    }

    pub(crate) fn run(&mut self, left: &mut [f32], right: &mut [f32]) {
        if !self.on || self.lines.is_empty() {
            return;
        }
        let depth = self.depth * self.depth * (self.depth * self.depth);
        let position = f64::from(self.time * self.time) + 0.01;
        let speed = f64::from(self.speed * self.speed * 20.0);
        let rate = self.sample_rate;
        for (n, line) in self.lines.iter_mut().enumerate() {
            let hz = if n & 1 != 0 { speed * 1.1 } else { speed };
            line.set_rate(hz, rate);
            line.place(f64::from(position as f32));
            line.feedback = (f64::from(self.feedback * self.feedback) * -0.99) as f32;
            line.set_depth(f64::from(depth));
        }
        let (wet, blur) = (self.wet, self.blur);
        let gains = [wet * wet, wet * wet * blur, blur * blur * (wet * wet)];
        for (n, line) in self.lines.iter_mut().enumerate() {
            line.gain_target = gains[n % 3];
        }
        let sync = self.sync as i32;
        if sync != 0 {
            // The beats a note of each length is, over 7.5.
            let beats = match sync {
                1 => 1.0 / 15.0,
                2 => 1.0 / 30.0,
                3 => 1.0 / 45.0,
                4 => 1.0 / 60.0,
                5 => 1.0 / 120.0,
                6 => 1.0 / 180.0,
                7 => 1.0 / 240.0,
                8 => 1.0 / 480.0,
                9 => 1.0 / 960.0,
                _ => 0.0,
            };
            let scale = if beats == 0.0 { 1.0 } else { self.tempo * beats };
            let position = f64::from((0.5 / scale) as f32);
            for line in &mut self.lines {
                line.place(position);
            }
        }
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            for (side, lines) in [(&mut *l, 0), (&mut *r, 3)] {
                let mut x = *side;
                x += self.lines[lines].run(x);
                x += self.lines[lines + 1].run(x);
                *side = self.lines[lines + 2].run(x) + x;
            }
        }
    }
}

/// The reverb's knob ranges (`0x180026530`): a knob from 0 to 1 is kept as
/// `floor(v * scale + offset)`.
const REVERB_RANGES: [(f32, f32); 18] = [
    (421.0, -360.5),
    (2981.0, 19.5),
    (216.0, 5.5),
    (250.0, 0.5),
    (99.0, 1.5),
    (100.0, 0.5),
    (28.0, 2.5),
    (1975.0, 25.5),
    (199.0, 1.5),
    (216.0, 5.5),
    (421.0, -360.5),
    (421.0, -360.5),
    (421.0, -360.5),
    (200.0, 0.5),
    (1.0, 0.5),
    (1.0, 0.5),
    (1.0, 0.5),
    (1.0, 0.5),
];

/// "[1] Default Reverb", the program the reverb starts on (`0x1800258f0`).
const REVERB_DEFAULT: [u32; 18] = [
    0x3f5b_83f4, 0x3c99_e459, 0x3e25_ed09, 0, 0x3efd_6a05, 0x3f80_0000, 0x3e92_4925,
    0x3e76_474b, 0x3d90_1495, 0x3e25_ed09, 0x3f5b_83f4, 0x3f46_3b97, 0x3f12_8bdc,
    0x3f00_0000, 0, 0, 0, 0,
];

/// The lengths of the plate's twelve lines, in 20000ths of the room
/// (`0x180025990`, `0x1800273e0`).
const PLATE_LINES: [i32; 12] = [
    0x8e, 0x6b, 0x17b, 0x115, 0x2a0, 0x1165, 0x708, 0xe88, 0x38c, 0x1079, 0xa60, 0xc5b,
];

/// The output taps: the line each reads and its place, in 20000ths of
/// the room. Lines 5, 7, 9 and 11 are the tank's delays, 6 and 10 two of
/// its allpasses.
const PLATE_TAPS: [(usize, i32); 14] = [
    (9, 0x10a),
    (9, 0xb9e),
    (10, 0x779),
    (11, 0x9c0),
    (5, 0x69a),
    (6, 0xbb),
    (7, 0x61e),
    (5, 0x161),
    (5, 0xe2b),
    (6, 0x6c0),
    (7, 0xc65),
    (9, 0x713),
    (10, 0x14f),
    (11, 0x26d),
];

/// The early reflections' taps, in 29400ths of the room
/// (`0x180074190`, `0x1800741d0`).
const EARLY_LEFT: [i32; 8] = [1102, 2904, 3784, 4873, 5878, 7470, 9062, 9263];
const EARLY_RIGHT: [i32; 8] = [837, 1423, 2865, 3938, 5278, 5978, 7532, 9263];

/// What the reverb's memory holds after a reset (`0x0da24260`).
const RESET_FILL: f32 = f32::from_bits(0x0da2_4260);

fn decibels(tenths: i32) -> f32 {
    if tenths > -361 {
        10f32.powf(tenths as f32 * 0.005)
    } else {
        0.0
    }
}

/// A one-pole's coefficient for a cutoff of `hundreds` of 100 Hz, or 1
/// from 22.1 kHz on.
fn one_pole(hundreds: i32, rate: f32) -> f32 {
    if (hundreds as f32) < 221.0 {
        let c = (hundreds as f32 * 628.318_54 / rate).cos();
        let x = 2.0 - c;
        1.0 - (x - (x * x - 1.0).sqrt())
    } else {
        1.0
    }
}

/// The reverb (`fxReverb`, process `0x180028f50`, coefficients
/// `0x1800273e0`).
#[derive(Debug, Clone)]
pub(crate) struct Plate {
    pub(crate) on: bool,
    rate: f32,
    knobs: [i32; 18],
    dirty: bool,
    memory: Vec<f32>,
    start: [usize; 12],
    len: [i32; 12],
    at: [i32; 12],
    delay_start: usize,
    delay_len: i32,
    delay_at: i32,
    early_start: usize,
    early_size: i32,
    early_at: i32,
    taps: [i32; 14],
    early_left: [i32; 8],
    early_right: [i32; 8],
    early_gain_left: [f32; 8],
    early_gain_right: [f32; 8],
    last_room: i32,
    input: f32,
    low_cut: f32,
    high_cut: f32,
    diffuse_a: f32,
    diffuse_b: f32,
    tank_a: f32,
    tank_b: f32,
    decay: f32,
    shelf: f32,
    crossover: f32,
    damping: f32,
    dry: f32,
    early: f32,
    wet: f32,
    high_state: f32,
    low_state: f32,
    low_last: f32,
    left_x: f32,
    left_y: f32,
    right_x: f32,
    right_y: f32,
    left_damp: f32,
    right_damp: f32,
    wet_now: f32,
    early_sum: (f32, f32),
}

impl Default for Plate {
    fn default() -> Self {
        let mut plate = Plate {
            on: false,
            rate: 44_100.0,
            knobs: [0; 18],
            dirty: true,
            memory: Vec::new(),
            start: [0; 12],
            len: [1; 12],
            at: [0; 12],
            delay_start: 0,
            delay_len: 1,
            delay_at: 0,
            early_start: 0,
            early_size: 1,
            early_at: 0,
            taps: [0; 14],
            early_left: [0; 8],
            early_right: [0; 8],
            early_gain_left: [0.0; 8],
            early_gain_right: [0.0; 8],
            last_room: 0,
            input: 0.0,
            low_cut: 0.0,
            high_cut: 0.0,
            diffuse_a: 0.0,
            diffuse_b: 0.0,
            tank_a: 0.0,
            tank_b: 0.0,
            decay: 0.0,
            shelf: 0.0,
            crossover: 0.0,
            damping: 0.0,
            dry: 0.0,
            early: 0.0,
            wet: 0.0,
            high_state: 0.0,
            low_state: 0.0,
            low_last: 0.0,
            left_x: 0.0,
            left_y: 0.0,
            right_x: 0.0,
            right_y: 0.0,
            left_damp: 0.0,
            right_damp: 0.0,
            wet_now: -1000.0,
            early_sum: (0.0, 0.0),
        };
        for (n, bits) in REVERB_DEFAULT.iter().enumerate() {
            plate.set(n, f32::from_bits(*bits));
        }
        plate
    }
}

impl Plate {
    /// A knob of the reverb, from 0 to 1.
    pub(crate) fn set(&mut self, n: usize, v: f32) {
        let (scale, offset) = REVERB_RANGES[n];
        self.knobs[n] = (v * scale + offset).floor() as i32;
        self.dirty = true;
    }

    /// Lay the memory out for a rate (`0x180025990`).
    pub(crate) fn set_sample_rate(&mut self, rate: f32) {
        self.rate = rate;
        let whole = rate as i32;
        let mut total = 0usize;
        for (n, k) in PLATE_LINES.iter().enumerate() {
            self.len[n] = whole * k / 20000;
            self.start[n] = total;
            total += self.len[n].max(0) as usize;
            self.at[n] = 0;
        }
        self.delay_len = whole * 0xfa / 1000;
        self.delay_start = total;
        self.delay_at = 0;
        total += self.delay_len.max(0) as usize;
        let mut bits = 0;
        let mut span = (f64::from(whole) * 0.316_326_530_612_244_9) as u32;
        if (span as i32) > 0 {
            while span != 0 {
                bits += 1;
                span >>= 1;
            }
        }
        self.early_size = 1 << (bits + 1);
        self.early_start = total;
        self.early_at = 0;
        total += self.early_size as usize;
        self.memory = vec![0.0; total];
        // The allocation's own lengths stand until the knobs are read.
        self.dirty = true;
        self.clear();
    }

    /// `0x180026320`: the memory to its fill, the filters to rest.
    pub(crate) fn clear(&mut self) {
        self.memory.fill(RESET_FILL);
        self.high_state = 0.0;
        self.low_state = 0.0;
        self.low_last = 0.0;
        self.left_x = 0.0;
        self.left_y = 0.0;
        self.right_x = 0.0;
        self.right_y = 0.0;
        self.left_damp = 0.0;
        self.right_damp = 0.0;
        self.wet_now = -1000.0;
    }

    /// The coefficients and lengths from the knobs (`0x1800273e0`).
    fn update(&mut self) {
        let rate = self.rate;
        let whole = rate as i32;
        let k = &self.knobs;
        self.input = if k[14] != 0 { 0.0 } else { decibels(k[0]) };
        self.low_cut = if k[1] as f32 > 19.0 {
            let t = ((rate * 0.5).min(k[1] as f32) * std::f32::consts::PI / rate).tan();
            (t - 1.0) / (t + 1.0)
        } else {
            0.0
        };
        self.high_cut = one_pole(k[2], rate);
        self.delay_len = (whole * k[3] / 1000).max(1);
        self.delay_at %= self.delay_len;
        let room = (k[4] + 15) * whole / 115;
        for (n, scale) in PLATE_LINES.iter().enumerate() {
            self.len[n] = (room * scale / 20000).max(1);
            self.at[n] %= self.len[n];
        }
        for (n, (_, scale)) in PLATE_TAPS.iter().enumerate() {
            self.taps[n] = room * scale / 20000;
        }
        let span = ((k[4] + 3) * whole / 103) as f32;
        for n in 0..8 {
            let left = (EARLY_LEFT[n] as f32 * 3.401_360_3e-5 * span) as i32;
            let right = (EARLY_RIGHT[n] as f32 * 3.401_360_3e-5 * span) as i32;
            self.early_left[n] = left;
            self.early_right[n] = right;
            // Only the first tap is given a level: the binary compares the
            // tap's number with 0 where it means to with itself.
            let least = if n == 0 {
                if left < right { left as f32 } else { right as f32 }
            } else {
                0.0
            };
            self.early_gain_left[n] = least * 0.5 / left as f32;
            self.early_gain_right[n] = least * 0.36 / right as f32;
        }
        if k[4] != self.last_room {
            self.last_room = k[4];
            for n in 0..12 {
                let at = self.start[n];
                self.memory[at..at + self.len[n] as usize].fill(0.0);
            }
            let at = self.early_start;
            self.memory[at..at + self.early_size as usize].fill(0.0);
        }
        let diffusion = k[5] as f32 * 0.01;
        if diffusion == 0.0 {
            (self.diffuse_a, self.diffuse_b, self.tank_a, self.tank_b) = (0.0, 0.0, 0.0, 0.0);
        } else {
            let per = 1.0 / rate;
            let l = &self.len;
            let first = (l[1] + l[0]) as f32;
            let second = (l[2] + l[3]) as f32;
            let third = (l[4] + l[8]) as f32;
            let fourth = (l[6] + l[10]) as f32;
            self.diffuse_a = (first * (-49.341_11 / diffusion) * per).exp();
            self.diffuse_b = ((-31.398_888 / diffusion) * second * per).exp();
            self.tank_a = (third * -3.453_877_7 / rate * (2.857_142_9 / diffusion)).exp();
            self.tank_b = ((-3.453_877_7 / rate) * fourth * (1.960_784_3 / diffusion)).exp();
        }
        let l = &self.len;
        let loop_len = (l[9] + l[7] + l[5] + l[11]) as f32;
        let per_tenth = loop_len / rate * -6.907_755_4;
        let tenths = k[8] as f32 * 0.1;
        self.decay = (per_tenth / tenths).exp().powf(0.25);
        let bass = k[6] as f32 * tenths;
        let low = ((10.0 / bass) * per_tenth).exp().powf(0.25) / self.decay;
        self.shelf = (low - 1.0) * 0.5;
        let low = low.min(1.0);
        let t = (k[7] as f32 * std::f32::consts::PI / rate).tan();
        self.crossover = (t - low) / (low + t);
        self.damping = one_pole(k[9], rate);
        self.dry = if k[15] != 0 { 0.0 } else { decibels(k[10]) };
        self.early = if k[16] != 0 { 0.0 } else { decibels(k[11]) * 0.5 };
        self.wet = if k[17] != 0 { 0.0 } else { decibels(k[12]) * 0.3 };
        self.dirty = false;
    }

    fn line(&mut self, n: usize) -> (&mut [f32], &mut i32, i32) {
        let at = self.start[n];
        let len = self.len[n];
        (&mut self.memory[at..at + len as usize], &mut self.at[n], len)
    }

    /// An allpass of line `n`: `subtract` takes the input off what the
    /// line holds, `write` puts the result back.
    fn allpass(&mut self, n: usize, x: f32, subtract: f32, write: f32) -> f32 {
        let (line, at, len) = self.line(n);
        let i = *at as usize;
        let v = line[i] - subtract * x;
        line[i] = write * v + x;
        *at -= 1;
        if *at < 0 {
            *at = len - 1;
        }
        v
    }

    /// Line `n` read where it is, then written.
    fn delay(&mut self, n: usize, x: f32) -> f32 {
        let (line, at, len) = self.line(n);
        let i = *at as usize;
        let out = line[i];
        line[i] = x;
        *at -= 1;
        if *at < 0 {
            *at = len - 1;
        }
        out
    }

    fn tap(&self, n: usize) -> f32 {
        let (line, _) = PLATE_TAPS[n];
        let mut i = self.taps[n] + self.at[line];
        if i >= self.len[line] {
            i -= self.len[line];
        }
        self.memory[self.start[line] + i as usize]
    }

    pub(crate) fn run(&mut self, left: &mut [f32], right: &mut [f32]) {
        if !self.on || self.memory.is_empty() {
            return;
        }
        if self.dirty {
            self.update();
        }
        let g = self.decay;
        let h = 1.0 - g;
        let f37 = h * 0.6 + g;
        let f38 = h * 0.25 + g;
        let f39 = h * 0.7 + g;
        let f52 = h * 0.2 + g;
        let f40 = h * 0.5 + g;
        let width = self.knobs[13] as f32 * 0.01;
        const DENORMAL: f32 = 1e-30;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let a = self.input * *l;
            let b = self.input * *r;
            let mut x = (((b + a) * 0.5 + DENORMAL) - self.high_state) * self.high_cut + self.high_state;
            self.high_state = x;
            let y = (x - self.low_state) * self.low_cut + self.low_last;
            self.low_last = x;
            self.low_state = y;
            if self.low_cut != 0.0 {
                x = (x - y) * 0.5 + DENORMAL;
            }
            if self.early != 0.0 {
                let mask = self.early_size - 1;
                let at = self.early_at;
                let base = self.early_start;
                let tap = |offset: i32| self.memory[base + ((offset + at) & mask) as usize];
                let (gl, gr) = (&self.early_gain_left, &self.early_gain_right);
                let (tl, tr) = (&self.early_left, &self.early_right);
                let el = tap(tl[3]) * gl[3] + ((tap(tl[0]) * gl[0] + tap(tl[1]) * gl[1]) - tap(tl[2]) * gl[2])
                    + tap(tl[4]) * gl[4]
                    + tap(tl[5]) * gl[5]
                    + tap(tl[6]) * gl[6];
                let er = ((((tap(tr[0]) * gr[0] - tap(tr[1]) * gr[1]) + tap(tr[2]) * gr[2]
                    + tap(tr[3]) * gr[3])
                    - tap(tr[4]) * gr[4])
                    + tap(tr[5]) * gr[5]
                    + tap(tr[6]) * gr[6])
                    - tap(tr[7]) * gr[7];
                self.early_sum = (el, er);
            }
            let (da, db) = (self.diffuse_a, self.diffuse_b);
            let v = self.allpass(0, x, da, da);
            let w = self.allpass(1, v, db, da);
            let at = self.early_at;
            self.memory[self.early_start + at as usize] = v;
            self.early_at -= 1;
            if self.early_at < 0 {
                self.early_at = self.early_size - 1;
            }
            let u = self.allpass(2, w, da, da);
            let z = self.allpass(3, u, db, da);
            let i = self.delay_at as usize;
            let held = self.memory[self.delay_start + i];
            self.memory[self.delay_start + i] = z;
            self.delay_at -= 1;
            if self.delay_at < 0 {
                self.delay_at = self.delay_len - 1;
            }
            let fed = held + 1e-20;
            // The left half of the tank, fed by the right half's end.
            let at = self.at[11] as usize;
            let back = self.memory[self.start[11] + at] * g;
            let shelved = (back - self.left_y) * self.crossover + self.left_x;
            self.left_x = back;
            self.left_y = shelved;
            let damped = (((shelved + back) * self.shelf + back + fed) - self.left_damp) * self.damping
                + self.left_damp;
            self.left_damp = damped;
            let q = self.allpass(4, damped, self.tank_a, self.tank_a);
            let out = self.delay(5, q) * g;
            let q = self.allpass(6, out, self.tank_b, self.tank_b);
            // Written, moved on, then read where it is now.
            {
                let (line, at, len) = self.line(7);
                line[*at as usize] = q;
                *at -= 1;
                if *at < 0 {
                    *at = len - 1;
                }
            }
            let at = self.at[7] as usize;
            let back = self.memory[self.start[7] + at] * g;
            let shelved = (back - self.right_y) * self.crossover + self.right_x;
            self.right_x = back;
            self.right_y = shelved;
            let damped = (((shelved + back) * self.shelf + back + fed) - self.right_damp) * self.damping
                + self.right_damp;
            self.right_damp = damped;
            let q = self.allpass(8, damped, self.tank_a, self.tank_a);
            let out = self.delay(9, q) * g;
            let q = self.allpass(10, out, self.tank_b, self.tank_b);
            {
                let (line, at, len) = self.line(11);
                line[*at as usize] = q;
                *at -= 1;
                if *at < 0 {
                    *at = len - 1;
                }
            }
            let t: [f32; 14] = std::array::from_fn(|n| self.tap(n));
            let target = if self.wet_now >= -99.0 {
                self.wet
            } else {
                self.wet_now = self.wet;
                self.wet
            };
            self.wet_now += (target - self.wet_now) * 0.001;
            let w = self.wet_now;
            let (el, er) = self.early_sum;
            let wet_left = w
                * ((((((t[0] - f38 * t[1]) - f39 * t[2]) - f52 * t[3]) + f37 * t[4]) + t[5])
                    + f40 * t[6])
                + self.early * el;
            let wet_right = w
                * ((((((f38 * t[8] + t[7]) - f39 * t[9]) - f52 * t[10]) - f37 * t[11]) + t[12])
                    + f40 * t[13])
                + self.early * er;
            let sum = wet_right + wet_left;
            let difference = (wet_left - wet_right) * width;
            *l = a * self.dry + difference + sum;
            *r = b * self.dry + (sum - difference);
        }
    }
}
