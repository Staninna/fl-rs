//! One voice of Drumpad: `DMPadGenerator` in the plugin's `engine_x64.dll`,
//! as `docs/plugins/drumpad.md` maps it. [`Patch::of`] is what the voice
//! makes of the parameters when a note starts (`FUN_18001f4d0`),
//! [`Voice::trigger`] starts it (`FUN_18001fca0`), [`Voice::play`] reads it
//! out at the host's rate (`FUN_180020f50`) and `Voice::render` makes its
//! samples at 38 kHz (`FUN_180020460`).

use std::f64::consts::TAU;

/// How many parameters there are.
pub const PARAMS: usize = 38;

/// The rate a voice makes its samples at, whatever the host's is, and one
/// sample of it as the binary has it (`0x18005c4e8`).
pub const RATE: f32 = 38_000.0;
const TICK: f32 = 2.631_578_9e-5;

/// The lanes of the membrane: lane `k` is a delay of `512 - k` samples.
/// Four lanes share a line of `512 - 4 i` slots of four floats.
const LANES: usize = 512;
const LINES: usize = LANES / 4;

/// The most lines one note runs: `128` lanes of shape from wherever the
/// size puts the first, and the line the last one ends in.
pub const MAX_LINES: usize = 33;

/// The most output samples one pass of [`Voice::play`] reads, as FL cuts
/// them (`FUN_180020f50`).
const CHUNK: usize = 256;

/// A sample under this is silent (`0x18005c4f0`).
const QUIET: f32 = 1e-4;

/// How a voice's gains move to where the host puts them, a sample.
const GLIDE: f32 = 0.001;

/// The floats the lines from `start` on take, `count` of them.
pub fn line_floats(start: usize, count: usize) -> usize {
    (start..start + count)
        .map(|line| (LANES - 4 * line) * 4)
        .sum()
}

/// What a parameter is for a note: the knob, or the knob moved towards
/// the target of each of the four velocity slots that picks it
/// (`FUN_1800229f0`). Slot `k` picks parameter `params[30 + k] + 1`, and
/// its target is `params[34 + k]`; the last one that picks it wins.
/// `x` is the note's `sqrt(velocity / 127)`.
pub fn blended(params: &[f32; PARAMS], index: usize, x: f32) -> f32 {
    let base = params[index];
    let mut target = base;
    for slot in 0..4 {
        let pick = params[30 + slot] as i32;
        if pick != 0 && usize::try_from(pick.wrapping_add(1)) == Ok(index) {
            target = params[34 + slot];
        }
    }
    (1.0 - x) * base + target * x
}

fn unit(value: f32) -> f32 {
    value.max(0.0).min(1.0)
}

/// What one note makes of the parameters (`FUN_18001f4d0`). The fields
/// are in the order the voice reads them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Patch {
    /// Membrane Cutoff squared: the one-pole lowpass in every lane.
    pub cutoff: f32,
    /// `-(Membrane Decay ^ 0.3)`: what a lane feeds back.
    pub feedback: f32,
    /// Membrane Phase, Tension and Material: the lanes' gains.
    pub phase: f32,
    pub tension: f32,
    pub material: f32,
    /// The first lane heard, `512 (1 - Membrane Size)` and at most 511.
    pub size: u32,
    /// How many lanes past it are heard, `128 Membrane Shape`, 1 to 128.
    pub shape: i32,
    /// `10^(-4 (1 - Mallet Amplitude))`: where the mallet starts.
    pub mallet_amp: f32,
    /// `(500 (1 - Mallet Decay)^4 + 1) 1e-4`: what it loses a sample.
    pub mallet_decay: f32,
    /// Mallet Noise, and one less it.
    pub noise: f32,
    pub tone: f32,
    /// Parameter 0, read as it is: every note starts the noise from
    /// [`Patch::seed`].
    pub repeat: bool,
    /// `Mallet Noise RP * (2^32 - 1)`.
    pub seed: u32,
    /// The steps of the Lo-Fi quantiser, 0 for none.
    pub lofi: i32,
    /// Hold, in seconds: `3 v^3 + 0.001`.
    pub hold: f32,
    /// `2^(2 Pitch - 1)`.
    pub pitch: f64,
    /// The LOW filter: on while LOW Freq is not 0, at `2^(11 v + 2)` Hz
    /// with a `q` of `8 res + 1`.
    pub low_on: bool,
    pub low_freq: f32,
    pub low_q: f32,
    /// The HIGH shelf: `2^(4.1 v + 8)` Hz, a bandwidth of `2 (1 - res) +
    /// 0.5` octaves and `36 (level - 0.5)` dB, on while that is more than
    /// 0.01 either way.
    pub high_freq: f32,
    pub high_bw: f32,
    pub high_gain: f32,
    pub high_on: bool,
    /// The MID peak: `2^(9 v + 5)` Hz, moved by `32 (mod - 0.5)`, at `32
    /// (level - 0.5)` dB moved by `48 (level mod - 0.5)`.
    pub mid_freq: f32,
    pub mid_freq_mod: f32,
    pub mid_level_on: bool,
    pub mid_level: f32,
    pub mid_mod_on: bool,
    pub mid_level_mod: f32,
    pub mid_bw: f32,
    /// `(1000 (1 - MID Attack)^4 + 1) 1e-5`: what the MID envelope loses
    /// a sample.
    pub mid_attack: f32,
    /// Parameter 1, read as it is: how many times the MID envelope starts
    /// over, 0 to 3.
    pub retrigger: i32,
    /// The SOF oscillator: Freq, Decay and Level. No preset turns it on.
    pub sof_freq: f32,
    pub sof_decay: f32,
    pub sof_on: bool,
    pub sof_level: f32,
}

impl Patch {
    /// The patch a note of `x = sqrt(velocity / 127)` plays.
    pub fn of(params: &[f32; PARAMS], x: f32) -> Self {
        let p = |index: usize| blended(params, index, x);
        let cutoff = unit(p(7));
        let mallet_decay = 1.0 - unit(p(3));
        let mid_attack = 1.0 - unit(p(23));
        let lofi = unit(p(27));
        let hold = unit(p(28));
        let pitch = f64::from(unit(p(29)));
        let sof_decay = unit(p(16));
        let half = 0.5;
        Patch {
            cutoff: cutoff * cutoff,
            feedback: -unit(p(6)).powf(0.3),
            phase: unit(p(9)),
            tension: unit(p(8)),
            material: unit(p(10)),
            size: (((unit(1.0 - p(11)) * 512.0) as i64) as u32).min(511),
            shape: ((p(12) * 128.0) as i32).clamp(1, 128),
            mallet_amp: 10f64.powf(f64::from((1.0 - unit(p(2))) * -4.0)) as f32,
            mallet_decay: (f64::from(
                mallet_decay * 500.0 * mallet_decay * (mallet_decay * mallet_decay) + 1.0,
            ) * 1e-4) as f32,
            noise: unit(p(4)),
            tone: 1.0 - unit(p(4)),
            repeat: params[0] != 0.0,
            seed: (f64::from(p(5)) * 4_294_967_295.0) as i64 as u32,
            lofi: if f64::from(lofi) >= 0.01 {
                (f64::from(1.0 - lofi).powf(5.0) * 65536.0 + 4.0) as i32
            } else {
                0
            },
            hold: hold * hold * (hold * 3.0) + 0.001,
            pitch: 2f64.powf(((pitch - 0.5) + pitch) - 0.5),
            low_on: p(13) != 0.0,
            low_freq: 2f32.powf(unit(p(13)) * 11.0 + 2.0),
            low_q: unit(p(14)) * 8.0 + 1.0,
            high_freq: 2f32.powf(unit(p(24)) * 4.1 + 8.0),
            high_bw: (1.0 - unit(p(26))) + (1.0 - unit(p(26))) + half,
            high_gain: (unit(p(25)) - half) * 36.0,
            high_on: ((unit(p(25)) - half) * 36.0).abs() > 0.01,
            mid_freq: 2f32.powf(unit(p(18)) * 9.0 + 5.0),
            mid_freq_mod: (unit(p(21)) - half) * 32.0,
            mid_level_on: p(19) != half,
            mid_level: (unit(p(19)) - half) * 32.0,
            mid_mod_on: p(22) != half,
            mid_level_mod: (unit(p(22)) - half) * 48.0,
            mid_bw: (1.0 - unit(p(20))) * 4.0 + half,
            mid_attack: (f64::from(
                mid_attack * 1000.0 * mid_attack * (mid_attack * mid_attack) + 1.0,
            ) * 1e-5) as f32,
            retrigger: ((params[1] * 3.0) as i32).clamp(0, 3),
            sof_freq: unit(p(15)),
            sof_decay: (f64::from(
                (sof_decay * half) * (sof_decay * half) * 250.0 * (sof_decay * sof_decay) + 1.0,
            ) * 1e-5) as f32,
            sof_on: p(17) != 0.0,
            sof_level: (f64::from(unit(p(17))).powf(8.0) * 0.5) as f32,
        }
    }

    /// The first line the note runs and how many: from the line of
    /// [`Patch::size`] to the one that holds lane `size + shape`, rounded
    /// up and no further than line 128.
    pub fn lines(&self) -> (usize, usize) {
        let start = (self.size / 4).min(127) as i32;
        let last = self.shape + self.size as i32;
        let end = if last >= LANES as i32 {
            LINES as i32
        } else {
            let whole = last / 4;
            let end = if last <= whole * 4 { whole } else { whole + 1 };
            end.min(LINES as i32)
        };
        (start as usize, (end - start).max(0) as usize)
    }

    /// The last lane heard: `size + shape`, and 511 past the end.
    fn last_lane(&self) -> i64 {
        let last = i64::from(self.shape) + i64::from(self.size);
        if last >= LANES as i64 { 511 } else { last }
    }
}

/// The gain of every lane from `first` on, `out.len()` of them
/// (`FUN_18001fca0`). From lane 511 down a phase steps by
/// `tension^4 / 2` a lane and wraps at 1. A lane between the size and the
/// last one heard gets a triangle of the phase, `-1` to `1`, to the power
/// `20 m^4 + 1` of `m = 2 material - 1`, or its inverse for a negative
/// `m`, with its sign kept. The others get 0.
pub fn lane_gains(patch: &Patch, first: usize, out: &mut [f32]) {
    let m = patch.material + patch.material - 1.0;
    let power = (m * 20.0) * m * (m * m) + 1.0;
    let power = if m >= 0.0 { power } else { 1.0 / power };
    let t = patch.tension;
    let step = (t * 0.5) * t * (t * t);
    let mut phase = patch.phase;
    let last = patch.last_lane();
    for lane in (0..LANES).rev() {
        let gain = if lane as u32 >= patch.size && lane as i64 <= last {
            let mut tri = ((phase - 0.5) + phase) - 0.5;
            if tri > 0.5 {
                tri = 1.0 - tri;
            }
            if tri < -0.5 {
                tri = -1.0 - tri;
            }
            let tri = tri + tri;
            if tri >= 0.0 {
                tri.powf(power)
            } else {
                -tri.abs().powf(power)
            }
        } else {
            0.0
        };
        phase += step;
        if phase > 1.0 {
            phase += -1.0;
        }
        if let Some(slot) = lane.checked_sub(first).and_then(|at| out.get_mut(at)) {
            *slot = gain;
        }
    }
}

/// Coefficients under 1e-9 either way are 0 (`0x18005c4e4`).
fn flush(value: f32) -> f32 {
    if value < 1e-9 && value > -1e-9 {
        0.0
    } else {
        value
    }
}

/// The LOW filter (`FUN_180018bf0`): a two-pole highpass whose numerator
/// and poles take the `q` differently, as the binary has them.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Low {
    set: Option<(f32, f32)>,
    c: [f32; 3],
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Low {
    fn tune(&mut self, freq: f32, q: f32) {
        if self.set == Some((freq, q)) {
            return;
        }
        self.set = Some((freq, q));
        let w = (f64::from(freq) * TAU / f64::from(RATE)) as f32;
        let (sin, cos) = w.sin_cos();
        let inverse = 1.0 / (sin + q);
        self.c = [
            flush((cos + 1.0) / (sin / q + 2.0)),
            flush(cos * inverse * (q * -2.0)),
            flush((q - sin) * inverse),
        ];
    }

    fn run(&mut self, x: f32) -> f32 {
        let [c0, c1, c2] = self.c;
        let y = ((x - (self.x1 + self.x1)) + self.x2) * c0 - self.y1 * c1 - self.y2 * c2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }

    fn clear(&mut self) {
        (self.x1, self.x2, self.y1, self.y2) = (0.0, 0.0, 0.0, 0.0);
    }
}

/// The two kinds of RBJ biquad the voice runs (`FUN_180018d50`), with the
/// gain taken as `10^(dB / 20)` where the cookbook has `/ 40`: a knob's dB
/// come out twice over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Peak,
    HighShelf,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Biquad {
    kind: Kind,
    /// The gain in dB, frequency and bandwidth in octaves it was last
    /// worked out for.
    set: Option<(f32, f32, f32)>,
    /// `b0, b1, b2, a1, a2`, over `a0`.
    c: [f32; 5],
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Biquad {
    fn new(kind: Kind) -> Self {
        Biquad {
            kind,
            set: None,
            c: [1.0, 0.0, 0.0, 0.0, 0.0],
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    fn tune(&mut self, gain: f32, freq: f32, bw: f32) {
        if self.set == Some((gain, freq, bw)) {
            return;
        }
        self.set = Some((gain, freq, bw));
        let a = 10f32.powf(gain * 0.05);
        let w = ((f64::from(freq) * 6.283_185 / f64::from(RATE)) as f32)
            .min(3.0)
            .max(0.0);
        let sin = w.sin();
        let s2 = {
            let arg = f64::from(bw) * 0.346_573_6 * f64::from(w) / f64::from(sin);
            let sinh = arg.sinh();
            (sinh + sinh) as f32
        };
        let root = a.sqrt() * s2;
        let alpha = sin * s2 * 0.5;
        let cos = w.cos();
        let (b0, b1, b2, a0, a1, a2) = match self.kind {
            Kind::Peak => {
                let up = alpha * a;
                let down = alpha / a;
                let b1 = cos * -2.0;
                (up + 1.0, b1, 1.0 - up, down + 1.0, b1, 1.0 - down)
            }
            Kind::HighShelf => {
                let s = sin * root;
                let less = a - 1.0;
                let more = a + 1.0;
                let less_cos = less * cos;
                let more_cos = more * cos;
                let plus = less_cos + more;
                let minus = more - less_cos;
                (
                    (s + plus) * a,
                    (more_cos + less) * a * -2.0,
                    (plus - s) * a,
                    minus + s,
                    {
                        let a1 = less - more_cos;
                        a1 + a1
                    },
                    minus - s,
                )
            }
        };
        let inverse = 1.0 / a0;
        self.c = [b0, b1, b2, a1, a2].map(|c| flush(inverse * c));
    }

    fn run(&mut self, x: f32) -> f32 {
        let [b0, b1, b2, a1, a2] = self.c;
        let y = x * b0 + self.x1 * b1 + self.x2 * b2 - self.y1 * a1 - self.y2 * a2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }

    fn clear(&mut self) {
        (self.x1, self.x2, self.y1, self.y2) = (0.0, 0.0, 0.0, 0.0);
    }
}

/// One voice. Its memory is asked for by [`Voice::new`] and
/// [`Voice::set_rate`]; the rest allocates nothing.
#[derive(Debug, Clone)]
pub struct Voice {
    patch: Patch,
    /// Whether the voice is sounding (`+0x49e4`), and whether it has yet
    /// to go above [`QUIET`] (`+0x49e5`).
    pub active: bool,
    fresh: bool,
    /// The lines the note runs, one after the other in `lines`: four
    /// floats a slot. FL keeps all 128 apart; one note never runs more
    /// than [`MAX_LINES`] of them.
    lines: Vec<f32>,
    first_line: usize,
    line_count: usize,
    /// Where each line starts in `lines`, how many slots it has and the
    /// slot it is at.
    at: [usize; MAX_LINES],
    len: [usize; MAX_LINES],
    slot: [usize; MAX_LINES],
    /// The lowpass of every lane, and its gain.
    low_pass: [[f32; 4]; MAX_LINES],
    gains: [[f32; 4]; MAX_LINES],
    rng: u32,
    mallet: f32,
    low: Low,
    high: Biquad,
    mid: Biquad,
    mid_env: f32,
    mid_tick: u32,
    retrigger: i32,
    /// The SOF ladder, its pitch envelope and its coefficients.
    sof: [f32; 4],
    sof_env: f32,
    sof_c: f32,
    sof_k: f32,
    /// Seconds since the note started, the hold envelope and seconds
    /// under [`QUIET`].
    time: f32,
    hold: f32,
    silent: f32,
    /// The samples one pass makes, after the last one of the pass before
    /// at 0, and how many of them there were.
    buffer: Vec<f32>,
    made: usize,
    /// Where the reading is, in samples of the buffer.
    position: f64,
    /// The gains the voice is at and the ones it moves to, and the bend
    /// the host's pitch makes. A gain under 0 is not yet set.
    gain: (f32, f32),
    pub target: (f32, f32),
    pub bend: f32,
}

impl Voice {
    /// A silent voice whose lines hold `floats` and that is read at up to
    /// four times 38 kHz over `rate`.
    pub fn new(floats: usize, rate: f32, seed: u32) -> Self {
        let mut voice = Voice {
            patch: Patch::of(&[0.0; PARAMS], 0.0),
            active: false,
            fresh: false,
            lines: vec![0.0; floats],
            first_line: 0,
            line_count: 0,
            at: [0; MAX_LINES],
            len: [0; MAX_LINES],
            slot: [0; MAX_LINES],
            low_pass: [[0.0; 4]; MAX_LINES],
            gains: [[0.0; 4]; MAX_LINES],
            rng: seed,
            mallet: 0.0,
            low: Low::default(),
            high: Biquad::new(Kind::HighShelf),
            mid: Biquad::new(Kind::Peak),
            mid_env: 1.0,
            mid_tick: 0,
            retrigger: 0,
            sof: [0.0; 4],
            sof_env: 1.0,
            sof_c: 0.0,
            sof_k: 0.0,
            time: 0.0,
            hold: 1.0,
            silent: 0.0,
            buffer: Vec::new(),
            made: 0,
            position: 0.0,
            gain: (-1.0, -1.0),
            target: (0.0, 0.0),
            bend: 1.0,
        };
        voice.set_rate(rate);
        voice
    }

    /// How many floats the lines have room for.
    pub fn capacity(&self) -> usize {
        self.lines.len()
    }

    /// Make room for the lines of a note that needs `floats`. Allocates.
    pub fn reserve(&mut self, floats: usize) {
        if floats > self.lines.len() {
            self.lines.resize(floats, 0.0);
        }
    }

    /// Size the buffer for the host's `rate`: a pass of [`CHUNK`] samples
    /// at the highest pitch and bend. Allocates.
    pub fn set_rate(&mut self, rate: f32) {
        let step = 4.0 * f64::from(RATE / rate);
        let len = (CHUNK as f64 * step).ceil() as usize + 4;
        self.buffer.resize(len.max(self.buffer.len()), 0.0);
    }

    /// Start a note on this voice (`FUN_18001fca0`). The lines are run as
    /// many as `lines` has room for; [`Voice::trigger`] says whether that
    /// was fewer than the patch asks for.
    pub fn trigger(&mut self, patch: Patch) -> bool {
        self.patch = patch;
        if patch.repeat {
            self.rng = patch.seed;
        }
        self.mallet = patch.mallet_amp;
        self.retrigger = patch.retrigger;
        self.time = 0.0;
        self.silent = 0.0;
        self.mid_env = 1.0;

        let (first, mut count) = patch.lines();
        let mut cut = false;
        while count > 0 && line_floats(first, count) > self.lines.len() {
            count -= 1;
            cut = true;
        }
        if (first, count) != (self.first_line, self.line_count) {
            // The lines a note runs are laid out for it: one that runs
            // others starts them from silence.
            self.lines.fill(0.0);
            self.low_pass = [[0.0; 4]; MAX_LINES];
            let mut at = 0;
            for line in 0..count {
                let len = LANES / 4 - (first + line);
                self.at[line] = at;
                self.len[line] = len;
                self.slot[line] = 0;
                at += len * 4;
            }
            self.first_line = first;
            self.line_count = count;
        }
        let mut gains = [0.0; MAX_LINES * 4];
        lane_gains(&patch, first * 4, &mut gains[..count * 4]);
        for (line, quad) in self.gains.iter_mut().enumerate().take(count) {
            quad.copy_from_slice(&gains[line * 4..line * 4 + 4]);
        }

        self.low.tune(patch.low_freq, patch.low_q);
        self.high.tune(
            patch.high_gain,
            patch.high_freq.max(0.0),
            patch.high_bw.max(1e-4),
        );
        self.mid_tick = 0;
        let exponent = (f64::from(patch.sof_freq * 0.5 + 0.1) * 9.3 + 5.0) as f32;
        let c = 2f32.powf(exponent) * 0.000_157_894_74;
        self.sof_c = c.max(0.0).min(1.0);
        self.sof_k = (f64::from(self.sof_c) * (1000.0 / 3.0)) as f32;
        self.sof_env = 1.0;
        self.active = true;
        self.fresh = true;
        cut
    }

    /// Silence the voice and clear what it holds (`FUN_1800201c0`). Where
    /// the reading is stays.
    pub fn clear(&mut self) {
        self.active = false;
        self.fresh = false;
        let used = line_floats(self.first_line, self.line_count).min(self.lines.len());
        self.lines[..used].fill(0.0);
        self.slot = [0; MAX_LINES];
        self.low_pass = [[0.0; 4]; MAX_LINES];
        self.buffer.fill(0.0);
        self.mid.clear();
        self.hold = 1.0;
        self.low.clear();
        self.high.clear();
        self.gain = (-1.0, -1.0);
        self.sof = [0.0; 4];
        self.sof_env = 1.0;
        self.silent = 0.0;
    }

    /// One sample of the membrane: every line the note runs, read, summed
    /// through the gains and written back through the lowpass and the
    /// feedback with the mallet added. Lane `j` of a line is `j` slots
    /// shorter than lane 0, which the binary makes by shifting lanes 1 to
    /// 3 along when the line wraps.
    fn membrane(&mut self, mallet: f32) -> f32 {
        let (cutoff, feedback) = (self.patch.cutoff, self.patch.feedback);
        let mut sum = [0.0f32; 4];
        for line in 0..self.line_count {
            let at = self.at[line];
            let len = self.len[line];
            let lane = &mut self.lines[at..at + len * 4];
            let slot = self.slot[line];
            if slot == len - 1 {
                lane[slot * 4 + 1] = lane[1];
            }
            if slot == len - 2 {
                lane[slot * 4 + 2] = lane[2];
                lane[(slot + 1) * 4 + 2] = lane[4 + 2];
            }
            if slot == len - 3 {
                lane[slot * 4 + 3] = lane[3];
                lane[(slot + 1) * 4 + 3] = lane[4 + 3];
                lane[(slot + 2) * 4 + 3] = lane[8 + 3];
            }
            let gains = self.gains[line];
            let low = &mut self.low_pass[line];
            for j in 0..4 {
                let v = lane[slot * 4 + j];
                sum[j] += v * gains[j];
                low[j] = (v - low[j]) * cutoff + low[j];
                lane[slot * 4 + j] = low[j] * feedback + mallet;
            }
            let next = slot + 1;
            if next == len {
                for shift in 1..4 {
                    for e in 0..len - shift {
                        lane[e * 4 + shift] = lane[(e + shift) * 4 + shift];
                    }
                }
                self.slot[line] = 0;
            } else {
                self.slot[line] = next;
            }
        }
        ((sum[1] + sum[0]) + sum[2]) + sum[3]
    }

    /// Make `out.len()` samples at 38 kHz (`FUN_180020460`). Says whether
    /// the voice went quiet for good: 25 ms under [`QUIET`] once it has
    /// been above it, or half a second.
    fn render(&mut self, out: &mut [f32]) -> bool {
        if !self.active {
            out.fill(0.0);
            return false;
        }
        let patch = self.patch;
        for o in out.iter_mut() {
            let r = self.rng;
            self.rng = r.wrapping_mul(0x0bb3_8435).wrapping_add(0x3619_636b);
            let env = f64::from(self.mallet);
            let env = ((1e-7 - env) * f64::from(patch.mallet_decay) + env) as f32;
            self.mallet = env;
            let noise = r as f32 * 2.328_306_4e-10;
            let mallet = ((noise - 0.5) * patch.noise + patch.tone) * env;

            let mut x = self.membrane(mallet);

            if patch.sof_on {
                let s4 = self.sof[3];
                let pushed = x + s4 * -100_000.0;
                x += s4 * patch.sof_level;
                let c = self.sof_c / ((self.sof_k + 1.0) - self.sof_env * self.sof_k);
                let bent = f64::from(pushed) / (f64::from(pushed.abs()) * 0.005 + 1.0);
                let mut stage = bent as f32;
                for s in &mut self.sof[..3] {
                    stage = (stage - *s) * c + *s;
                    *s = stage;
                }
                self.sof_env -= self.sof_env * patch.sof_decay;
                self.sof[3] = (self.sof[2] - s4) * c + s4;
            }
            if patch.low_on {
                x = self.low.run(x);
            }
            if patch.high_on {
                x = self.high.run(x);
            }
            if patch.mid_level_on || patch.mid_mod_on {
                if self.mid_tick == 0 {
                    let rise = 1.0 - self.mid_env;
                    let gain = rise * patch.mid_level_mod + patch.mid_level;
                    let swing = patch.mid_freq_mod * rise;
                    let freq = if patch.mid_freq_mod >= 0.0 {
                        (swing + 1.0) * patch.mid_freq
                    } else {
                        patch.mid_freq / (1.0 - swing)
                    };
                    let freq = freq.max(32.0).min(12_000.0).max(0.0);
                    self.mid.tune(gain, freq, patch.mid_bw.max(1e-4));
                }
                self.mid_tick = (self.mid_tick + 1) % 6;
                self.mid_env -= self.mid_env * patch.mid_attack;
                if self.retrigger != 0 && f64::from(self.mid_env) < 0.01 {
                    self.mid_env = 1.0;
                    self.retrigger -= 1;
                }
                x = self.mid.run(x);
            }
            x = x.min(1.0).max(-1.0);
            let hold = f64::from(self.hold);
            let hold = if self.time >= patch.hold {
                hold - hold * 0.001
            } else {
                f64::from(1.0 - self.hold) * 0.1 + hold
            };
            self.time += TICK;
            self.hold = hold as f32;
            x *= self.hold;
            if patch.lofi != 0 {
                let steps = patch.lofi as f32;
                x = ((steps * x) as i32) as f32 / steps;
            }
            if x.abs() > QUIET {
                self.silent = 0.0;
                self.fresh = false;
            } else {
                self.silent += TICK;
            }
            *o = x;
        }
        (!self.fresh && f64::from(self.silent) > 0.025) || self.silent > 0.5
    }

    /// Add the voice to `left` and `right`, read at `rate` (`FUN_180020f50`):
    /// `38000 / rate * pitch * bend` samples of its own a frame, between
    /// two of them, times gains that move a thousandth of the way to
    /// [`Voice::target`] a frame. Says whether the voice stopped.
    pub fn play(&mut self, left: &mut [f32], right: &mut [f32], rate: f32) -> bool {
        if !self.active {
            return false;
        }
        let step = f64::from(RATE / rate) * self.patch.pitch * f64::from(self.bend);
        if self.gain.0 < 0.0 {
            self.gain.0 = self.target.0;
        }
        if self.gain.1 < 0.0 {
            self.gain.1 = self.target.1;
        }
        let mut stopped = false;
        let mut done = 0;
        while done < left.len() {
            let n = (left.len() - done).min(CHUNK);
            let mut frame = done;
            let end = done + n;
            let mut pos = self.position;
            while pos < 0.0 && frame < end {
                let at = pos + self.made as f64;
                self.emit(at, frame, left, right, stopped);
                pos += step;
                self.position = pos;
                frame += 1;
            }
            if frame < end {
                let ahead = (end - frame) as f64 * step + pos;
                let mut made = ahead as u32 as usize;
                if (made as f64) < ahead {
                    made += 1;
                }
                let made = made.min(self.buffer.len() - 1);
                self.made = made;
                let (_, fresh) = self.buffer.split_at_mut(1);
                if self.render(&mut fresh[..made]) {
                    self.clear();
                    stopped = true;
                }
                while frame < end {
                    self.emit(self.position, frame, left, right, stopped);
                    self.position += step;
                    frame += 1;
                }
                self.position -= self.made as f64;
                self.buffer[0] = self.buffer[self.made];
            }
            done = end;
        }
        stopped
    }

    /// One frame at `at` samples into the buffer, between the sample
    /// before and the one at it, through the gains, which then move on.
    fn emit(&mut self, at: f64, frame: usize, left: &mut [f32], right: &mut [f32], mute: bool) {
        let whole = at as i32 as usize;
        let a = self.buffer[whole];
        let b = self.buffer[whole + 1];
        let s = (b - a) * (at - whole as f64) as f32 + a;
        if !mute {
            left[frame] += s * self.gain.0;
            right[frame] += s * self.gain.1;
        }
        self.gain.0 += (self.target.0 - self.gain.0) * GLIDE;
        self.gain.1 += (self.target.1 - self.gain.1) * GLIDE;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> [f32; PARAMS] {
        crate::drumpad::DEFAULTS
    }

    #[test]
    fn a_lane_is_as_many_samples_shorter_as_it_is_far_along_its_line() {
        // One line of eight slots: lane j comes back 8 - j samples later.
        let mut voice = Voice::new(line_floats(0, 1), 44_100.0, 0);
        voice.line_count = 1;
        voice.len[0] = 8;
        voice.at[0] = 0;
        voice.patch.cutoff = 1.0;
        voice.patch.feedback = 0.0;
        voice.gains[0] = [1.0, 0.0, 0.0, 0.0];
        for lane in 0..4 {
            voice.gains[0] = [0.0; 4];
            voice.gains[0][lane] = 1.0;
            voice.lines.fill(0.0);
            voice.slot[0] = 0;
            voice.low_pass[0] = [0.0; 4];
            // An impulse in, with no feedback: it is written as the mallet
            // and read back once.
            let heard: Vec<f32> = (0..40)
                .map(|t| voice.membrane(if t == 20 { 1.0 } else { 0.0 }))
                .collect();
            let at = heard.iter().position(|v| *v != 0.0).unwrap();
            assert_eq!(at - 20, 8 - lane, "lane {lane}");
        }
    }

    #[test]
    fn velocity_moves_a_parameter_its_slot_picks_to_the_target() {
        let mut params = defaults();
        params[3] = 0.2;
        params[30] = 2.0;
        params[34] = 0.8;
        assert_eq!(blended(&params, 3, 0.0), 0.2);
        assert_eq!(blended(&params, 3, 1.0), 0.8);
        assert!((blended(&params, 3, 0.5) - 0.5).abs() < 1e-6);
        // A slot of 0 picks nothing, and another parameter stays.
        assert_eq!(blended(&params, 2, 1.0), params[2]);
        params[30] = 0.0;
        assert_eq!(blended(&params, 3, 1.0), 0.2);
    }

    #[test]
    fn the_patch_has_the_binary_s_curves() {
        let mut params = defaults();
        params[2] = 0.0;
        params[3] = 1.0;
        params[11] = 0.0;
        params[12] = 0.5;
        params[28] = 1.0;
        params[29] = 1.0;
        let patch = Patch::of(&params, 0.0);
        assert!((patch.mallet_amp - 1e-4).abs() < 1e-9);
        assert!((patch.mallet_decay - 1e-4).abs() < 1e-9);
        assert_eq!(patch.size, 511);
        assert_eq!(patch.shape, 64);
        assert!((patch.hold - 3.001).abs() < 1e-6);
        assert_eq!(patch.pitch, 2.0);
        assert_eq!(patch.lines(), (127, 1));
        // The defaults: no lo-fi, the LOW filter off, shelves off.
        let patch = Patch::of(&defaults(), 0.0);
        assert_eq!(patch.lofi, 0);
        assert!(!patch.low_on && !patch.high_on && !patch.sof_on);
        assert!((patch.feedback + 0.9f32.powf(0.3)).abs() < 1e-7);
        assert_eq!(patch.lines(), (0, 32));
    }

    #[test]
    fn the_gains_are_a_triangle_of_the_phase_and_zero_outside_the_lanes() {
        let mut patch = Patch::of(&defaults(), 0.0);
        patch.size = 10;
        patch.shape = 4;
        patch.material = 0.5;
        patch.tension = 0.0;
        patch.phase = 0.75;
        let mut gains = [9.0; 512];
        lane_gains(&patch, 0, &mut gains);
        // 2 (2 * 0.75 - 1) = 1 at power 1 on lanes 10 to 14.
        for (lane, gain) in gains.iter().enumerate() {
            let expected = if (10..=14).contains(&lane) { 1.0 } else { 0.0 };
            assert_eq!(*gain, expected, "lane {lane}");
        }
    }

    #[test]
    fn a_peak_has_twice_its_db_at_its_frequency() {
        let mut peak = Biquad::new(Kind::Peak);
        peak.tune(6.0, 1000.0, 1.0);
        let [b0, b1, b2, a1, a2] = peak.c.map(f64::from);
        let gain = crate::testing::response(&[b0, b1, b2], &[1.0, a1, a2], 1000.0 / 38_000.0);
        assert!((20.0 * gain.log10() - 12.0).abs() < 0.05, "{gain}");
        let mut shelf = Biquad::new(Kind::HighShelf);
        shelf.tune(6.0, 1000.0, 1.0);
        let [b0, b1, b2, a1, a2] = shelf.c.map(f64::from);
        let top = crate::testing::response(&[b0, b1, b2], &[1.0, a1, a2], 0.45);
        let bottom = crate::testing::response(&[b0, b1, b2], &[1.0, a1, a2], 0.001);
        assert!((20.0 * top.log10() - 12.0).abs() < 0.2, "{top}");
        assert!((20.0 * bottom.log10()).abs() < 0.2, "{bottom}");
    }

    #[test]
    fn the_low_filter_passes_the_top_and_cuts_the_bottom() {
        let mut low = Low::default();
        low.tune(1000.0, 1.0);
        let [c0, c1, c2] = low.c.map(f64::from);
        let response =
            |turns| crate::testing::response(&[c0, -2.0 * c0, c0], &[1.0, c1, c2], turns);
        assert!(response(10_000.0 / 38_000.0) > 0.9);
        assert!(response(50.0 / 38_000.0) < 0.01);
    }
}
