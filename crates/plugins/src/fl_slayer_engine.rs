//! The engine of FL Slayer: `Slayer_Engine.dll`, reFX's 32-bit Slayer 2
//! engine of 2003 that FL Slayer's 64-bit wrapper bridges to. Every block
//! here is read from that DLL; `docs/plugins/fl-slayer.md` maps each one to
//! its function. Names of the form `0x5cc` are the engine object's own
//! field offsets, where a field has no better name.
//!
//! The engine runs six strings, each two damped delay lines a few cents
//! apart at a fixed 44.1 kHz, read through a pickup comb and a
//! state-variable lowpass and resampled to the host's rate by a 4-point
//! Hermite. One mono bus then goes through the "AMP" effect, the amp and
//! cabinet, a fixed speaker EQ and the stereo "MST" effect. Events are
//! taken every 5 ms, on the engine's control clock.
//!
//! The engine processes with the FPU set to truncate (its process calls
//! `_controlfp(_RC_CHOP, _MCW_RC)`), so every float-to-int here is `as`.

use std::f64::consts::LOG2_E;

/// The rate the strings run at, whatever the host's.
const STRING_RATE: f32 = 44_100.0;
/// A string's delay lines and pickup buffer: 32768 floats, masked.
const LINE: usize = 0x8000;
const LINE_MASK: usize = LINE - 1;
/// The four effect buffers: 0x40000 floats each.
const FX_BUF: usize = 0x40000;
/// The engine's denormal guard, `1e-18`, added all over.
const TINY: f32 = 1e-18;
/// The engine's own 2 pi and pi, a little short of the true ones.
const ENGINE_TAU: f32 = f32::from_bits(0x40c9_0fd0);
const ENGINE_PI: f32 = f32::from_bits(0x4049_0fd0);
/// The six voices.
pub const VOICES: usize = 6;

/// The 28 parameters as the engine holds them, 0 to 1.
pub mod p {
    pub const STRINGS: usize = 0;
    pub const COILS: usize = 1;
    pub const PICKUP: usize = 2;
    pub const TONE: usize = 3;
    pub const SLAP: usize = 4;
    pub const FRET: usize = 5;
    pub const HARMONIC: usize = 6;
    pub const DAMP: usize = 7;
    pub const VEL_V: usize = 8;
    pub const VEL_D: usize = 9;
    pub const AMP: usize = 12;
    pub const DRIVE: usize = 13;
    pub const PRESENCE: usize = 14;
    pub const FEEDBACK: usize = 15;
    pub const EQ_LOW: usize = 16;
    pub const EQ_MID: usize = 17;
    pub const EQ_HIGH: usize = 18;
    pub const CAB: usize = 19;
    pub const FX: usize = 20;
    pub const FX1: usize = 21;
    pub const FX2: usize = 22;
    pub const AID: usize = 24;
    pub const TIMING: usize = 25;
    pub const LATCH: usize = 26;
}

/// The excitation samples, by the index the engine keeps in a voice's
/// first byte: 16-bit PCM read out of the DLL.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Excitations {
    /// Index 0 to 28; only 1, 7, 14 to 22, 24 and 26 to 28 are used.
    pub tables: Vec<Vec<i16>>,
}

impl Excitations {
    fn get(&self, index: u8) -> Option<&[i16]> {
        self.tables
            .get(usize::from(index))
            .map(Vec::as_slice)
            .filter(|t| !t.is_empty())
    }
}

/// `sin(i / 65536 * 2 pi)` with the engine's float constants: its
/// soft clipper reads `sin(0.785 x)` from here.
fn sine_table() -> Vec<f32> {
    let step = f64::from(1.0f32 / 65_536.0) * f64::from(ENGINE_TAU);
    (0..65_536)
        .map(|i| (f64::from(i) * step).sin() as f32)
        .collect()
}

/// The soft clipper: `sin(0.785 x)` from the table, `x` clamped to ±1.
fn clip_sin(sine: &[f32], x: f32) -> f32 {
    let x = x.clamp(-1.0, 1.0);
    sine[((x * 0.785 * 10_430.055) as i32 & 0xffff) as usize]
}

fn wrap(phase: f32) -> f32 {
    phase - phase as i32 as f32
}

/// The engine's triangle: `|2 (frac(x) - 0.5)|`, 0 to 1.
fn tri(phase: f32) -> f32 {
    let t = (phase - phase as i32 as f32) - 0.5;
    (t + t).abs()
}

/// `2^(semitones / 12)` as the engine works it, by `f2xm1` and `fscale`
/// on `ln 2 / 12`.
fn semis(note: f32) -> f64 {
    (LOG2_E * f64::from(note) * f64::from(0.057_762_265_f32)).exp2()
}

/// MSVC's `rand`, which the noise string calls.
#[derive(Debug, Clone)]
struct Rand(u32);

impl Rand {
    fn next(&mut self) -> i32 {
        self.0 = self.0.wrapping_mul(214_013).wrapping_add(2_531_011);
        ((self.0 >> 16) & 0x7fff) as i32
    }
}

/// A direct-form-I biquad, `y = b0 x + b1 x1 + b2 x2 - a1 y1 - a2 y2`.
#[derive(Debug, Clone, Copy, Default)]
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    x: [f32; 2],
    y: [f32; 2],
}

impl Biquad {
    fn run(&mut self, input: f32) -> f32 {
        let y = ((self.b[1] * self.x[0] + input * self.b[0] + self.x[1] * self.b[2])
            - self.a[0] * self.y[0])
            - self.a[1] * self.y[1];
        self.x = [input, self.x[0]];
        self.y = [y, self.y[0]];
        y
    }

    fn set(&mut self, b: [f64; 3], a: [f64; 3]) {
        self.b = b.map(|b| (b / a[0]) as f32);
        self.a = [(a[1] / a[0]) as f32, (a[2] / a[0]) as f32];
    }

    /// RBJ's shelves (FUN_10002f60 types 5 and 6, FUN_10009500), `slope`
    /// 1 for the EQ's.
    fn shelf(&mut self, high: bool, hz: f32, db: f32, slope: f32, rate: f32) {
        let a = 10f64.powf(f64::from(db * 0.025));
        let w = f64::from(hz) * f64::from(std::f32::consts::TAU) / f64::from(rate);
        let (sin, cos) = w.sin_cos();
        let root = sin * ((a * a + 1.0) / f64::from(slope) - (a - 1.0) * (a - 1.0)).sqrt();
        let (p, m) = (a + 1.0, a - 1.0);
        if high {
            self.set(
                [
                    a * (p + m * cos + root),
                    -2.0 * a * (m + p * cos),
                    a * (p + m * cos - root),
                ],
                [p - m * cos + root, 2.0 * (m - p * cos), p - m * cos - root],
            );
        } else {
            self.set(
                [
                    a * (p - m * cos + root),
                    2.0 * a * (m - p * cos),
                    a * (p - m * cos - root),
                ],
                [p + m * cos + root, -2.0 * (m + p * cos), p + m * cos - root],
            );
        }
    }

    /// RBJ's peak (FUN_10002f60 type 4), its width in octaves.
    fn peak(&mut self, hz: f32, db: f32, octaves: f32, rate: f32) {
        let a = 10f64.powf(f64::from(db * 0.025));
        let w = f64::from(hz) * f64::from(std::f32::consts::TAU) / f64::from(rate);
        let (sin, cos) = w.sin_cos();
        let alpha = (f64::from(octaves * 0.346_573_6) * w / sin).sinh() * sin;
        self.set(
            [1.0 + alpha * a, -2.0 * cos, 1.0 - alpha * a],
            [1.0 + alpha / a, -2.0 * cos, 1.0 - alpha / a],
        );
    }
}

/// The 4-pole lowpass FUN_100052d0, FUN_10005400 and FUN_10005550 run on
/// three sets of state: one-pole stages with a cubic in the last and the
/// resonance fed back from it.
#[derive(Debug, Clone, Copy, Default)]
struct Ladder {
    y: [f32; 4],
    x: f32,
    prev: [f32; 3],
}

impl Ladder {
    fn run(&mut self, input: f32, cutoff: f32, resonance: f32, top: Option<f32>) -> f32 {
        let f = top.map_or(cutoff, |top| cutoff.min(top));
        let k = (f * 3.6 - f * f * 1.6) - 1.0;
        let g = (k + 1.0) * 0.5;
        let fb = (resonance / (cutoff * 20_000.0 + 5050.0)) * 12_000.0 * self.y[3];
        let x = input - (fb + fb);
        let mut stage = (x + self.x) * g - k * self.y[0];
        self.y[0] = stage;
        for i in 0..3 {
            let before = self.prev[i];
            self.prev[i] = stage;
            stage = (stage + before) * g - k * self.y[i + 1];
            if i < 2 {
                self.y[i + 1] = stage;
            }
        }
        self.y[3] = stage - stage * stage * stage * 0.016_67;
        self.x = x;
        ((40_000.0 - f64::from(cutoff)) * f64::from(self.y[3]) * f64::from(1e-4_f32)) as f32
    }
}

/// A Chamberlin state-variable filter: `low`, `band`, `high`.
#[derive(Debug, Clone, Copy, Default)]
struct Svf {
    low: f32,
    band: f32,
    high: f32,
}

/// One string. Field comments give the engine's offsets in a voice.
pub struct Voice {
    /// +0: which excitation sample, +4 its rate, +8 its gain.
    sample: u8,
    sample_rate: f32,
    sample_gain: f32,
    /// +0xc: the key, as a float.
    pub note: f32,
    /// +0x10: the velocity, 0 to 1, and +0x14 the one held while the key is.
    velocity: f32,
    velocity_held: f32,
    /// +0x18: cents of detune, an index into the cents table.
    detune: i8,
    /// +0x19: whether the key is down.
    held: bool,
    /// +0x1c..+0x34: the excitation envelope: its output, phase, the level
    /// it released from, the gate, its stage (4 attack, 3 decay, 2 off, 1
    /// release, 0 done), the smoothed amplitude and its target.
    env_out: f32,
    env_phase: f32,
    env_release: f32,
    pub gate: f32,
    env_stage: u8,
    amp: f32,
    env_target: f32,
    /// +0xc8 and +0x4014c: the two delay lines, +0x200c8 the pickup's.
    line_a: Vec<f32>,
    line_b: Vec<f32>,
    pickup: Vec<f32>,
    index: usize,
    /// +0x80154, +0x80164: the loop lowpasses; +0x80158 the fret noise's.
    lp_a: f32,
    lp_b: f32,
    fret_lp: f32,
    /// +0x80174: samples (in tone steps) since the key went up; a voice
    /// past `0.2 rate` is silent.
    release_time: f32,
    /// +0x80178: the place in the excitation sample.
    sample_pos: f32,
    /// +0x8017c: how many blocks the voice has played, for stealing.
    age: i32,
    /// +0x80180..: the last four 44.1 kHz samples, newest first, and the
    /// place between them.
    history: [f32; 4],
    phase: f32,
    /// +0x80194: control blocks to wait before sounding (strumming).
    delay: i32,
}

impl Voice {
    fn new() -> Self {
        Voice {
            sample: 0,
            sample_rate: 0.0,
            sample_gain: 0.0,
            note: 0.0,
            velocity: 0.0,
            velocity_held: 0.0,
            detune: 0,
            held: false,
            env_out: 0.0,
            env_phase: 0.0,
            env_release: 0.0,
            gate: 0.0,
            env_stage: 0,
            amp: 0.0,
            env_target: 0.0,
            line_a: vec![0.0; LINE],
            line_b: vec![0.0; LINE],
            pickup: vec![0.0; LINE],
            index: 0,
            lp_a: 0.0,
            lp_b: 0.0,
            fret_lp: 0.0,
            // FUN_10002ad0: dead from the start.
            release_time: 1e8,
            sample_pos: 0.0,
            age: 0,
            history: [0.0; 4],
            phase: 0.0,
            delay: 0,
        }
    }

    fn clear(&mut self) {
        self.line_a.fill(0.0);
        self.line_b.fill(0.0);
    }

    /// FUN_100038a0's body: the key goes up and the voice releases.
    fn release(&mut self) {
        self.held = false;
        self.gate = 0.0;
        self.env_stage = 1;
        self.env_release = self.env_out;
        self.env_phase = 0.0;
    }

    fn key(&self) -> i8 {
        self.note as i32 as i8
    }
}

/// The keys held in a solo mode (0x829774): newest first.
#[derive(Debug, Clone, Default)]
struct Held {
    keys: Vec<i8>,
}

impl Held {
    /// FUN_10003820: pushes the key, and returns how many were held.
    fn push(&mut self, key: i8) -> usize {
        let before = self.keys.len();
        if before < 128 {
            self.keys.insert(0, key);
        }
        before
    }

    /// FUN_10003f40: removes the key.
    fn remove(&mut self, key: i8) {
        if let Some(at) = self.keys.iter().position(|&k| k == key) {
            self.keys.remove(at);
        }
    }
}

/// One event, taken on the next control tick.
#[derive(Debug, Clone, Copy)]
pub enum Event {
    On { key: i32, velocity: f32, detune: i8 },
    Off { key: i32 },
}

/// The whole engine object.
pub struct Engine {
    /// The parameters, 0 to 1.
    pub params: [f32; 28],
    rate: f32,
    tempo: f32,
    /// MIDI volume and expression (0x520ccc, 0x520cd0).
    volume: f32,
    expression: f32,
    sine: Vec<f32>,
    /// 1.000578^k for k in -127..=127: cents.
    cents: [f32; 255],
    excitations: Option<Excitations>,
    rand: Rand,
    pub voices: Vec<Voice>,
    /// The per-voice output filter: 0x53c low, 0x56c band, 0x59c high.
    voice_svf: [Svf; VOICES],
    /// 0x864: how hard the strings drive the amp's feedback.
    feedback_drive: f32,
    /// 0x868: the amp's feedback into the strings, and its write and read
    /// places.
    feedback: Vec<f32>,
    feedback_write: u32,
    feedback_read: u32,
    /// Pitch: bend (0x4d0), the fret ratio of the glissando (0x4c8) and
    /// the one last seen (0x4cc), the fret noise level and phase.
    bend: f32,
    fret_ratio: f32,
    fret_ratio_seen: f32,
    fret_noise: f32,
    fret_phase: f32,
    /// 0x2b6: whether a string sounded this block.
    sounding: bool,
    /// 0x24c: blocks of silence under the latch.
    quiet: i32,
    latch: bool,
    latch_seen: bool,
    aid_seen: i32,
    held: Held,
    strum_count: i32,
    strum_clock: i32,
    glide_target: i32,
    glide_note: f32,
    glide_rate: f32,
    /// 0x821764: samples to the next control tick.
    tick: f32,
    pending: Vec<Event>,
    // The effects' state.
    eq: [Biquad; 3],
    phase_240: f32,
    phase_244: f32,
    phase_248: f32,
    phase_250: f32,
    lfo: f32,
    env_5cc: f32,
    fb_5d0: f32,
    hp_23c: f32,
    oct_prev: f32,
    oct_flip: f32,
    oct_env: [f32; 2],
    oct_lp: f32,
    oct_square: f32,
    shifter: [f32; 22],
    ladders: [Ladder; 3],
    tube: [f32; 3],
    band: [Svf; 3],
    amp_svf: Svf,
    fx_svf: [Svf; 3],
    cab_buf: Vec<f32>,
    cab_index: u32,
    speaker_hp: f32,
    speaker: [Biquad; 2],
    fx_buf: [Vec<f32>; 4],
    fx_index: [i32; 3],
    fx_fb: [f32; 4],
}

impl Engine {
    pub fn new() -> Self {
        let mut cents = [1.0f32; 255];
        let mut f = 1.0f64;
        for k in 0..128 {
            cents[127 + k] = f as f32;
            cents[127 - k] = (1.0 / f) as f32;
            f *= 1.000_577_79;
        }
        let mut engine = Engine {
            params: [0.0; 28],
            rate: 44_100.0,
            tempo: 120.0,
            volume: 1.0,
            expression: 1.0,
            sine: sine_table(),
            cents,
            excitations: None,
            rand: Rand(1),
            voices: (0..VOICES).map(|_| Voice::new()).collect(),
            voice_svf: [Svf::default(); VOICES],
            feedback_drive: 0.0,
            feedback: vec![0.0; LINE],
            // 1 - trunc(-0.005 * 44100), at the rate the engine is made at.
            feedback_write: 221,
            feedback_read: 0,
            bend: 0.0,
            fret_ratio: 1.0,
            fret_ratio_seen: 0.0,
            fret_noise: 0.0,
            fret_phase: 0.0,
            sounding: true,
            quiet: 0,
            latch: false,
            latch_seen: true,
            // FL's starts at -1 and plays blocks before the first note.
            aid_seen: 0,
            held: Held::default(),
            strum_count: 0,
            strum_clock: 0,
            glide_target: 0,
            glide_note: 0.0,
            glide_rate: 0.0,
            tick: 0.0,
            pending: Vec::with_capacity(256),
            eq: [Biquad::default(); 3],
            phase_240: 0.0,
            phase_244: 0.0,
            phase_248: 0.0,
            phase_250: 0.0,
            lfo: 0.0,
            env_5cc: 0.0,
            fb_5d0: 0.0,
            hp_23c: 0.0,
            oct_prev: 0.0,
            oct_flip: 0.0,
            oct_env: [0.0; 2],
            oct_lp: 0.0,
            oct_square: 0.0,
            shifter: [0.0; 22],
            ladders: [Ladder::default(); 3],
            tube: [0.0; 3],
            band: [Svf::default(); 3],
            amp_svf: Svf::default(),
            fx_svf: [Svf::default(); 3],
            cab_buf: vec![0.0; 512],
            cab_index: 0,
            speaker_hp: 0.0,
            speaker: [Biquad::default(); 2],
            fx_buf: std::array::from_fn(|_| vec![0.0; FX_BUF]),
            fx_index: [0; 3],
            fx_fb: [0.0; 4],
        };
        engine.set_rate(44_100.0);
        engine
    }

    pub fn set_excitations(&mut self, excitations: Option<Excitations>) {
        self.excitations = excitations;
    }

    pub fn has_excitations(&self) -> bool {
        self.excitations.is_some()
    }

    pub fn set_tempo(&mut self, bpm: f32) {
        self.tempo = bpm;
    }

    /// FUN_10008340, and the speaker EQ's own (FUN_10009350): a low shelf
    /// of +10 dB at 130 Hz, slope 0.5, and a high shelf of +21 dB at
    /// 10 kHz.
    pub fn set_rate(&mut self, hz: f32) {
        self.rate = hz;
        let clamp = |f: f32| if hz < f + f { hz * 0.49 } else { f };
        let (low, high) = (clamp(0.325 * 400.0), clamp(0.5 * 5000.0 + 7500.0));
        let low_gain = f32::from_bits(0x3ed5_5555) * 24.0;
        self.speaker[0].shelf(false, low, low_gain, 0.5, hz);
        self.speaker[1].shelf(true, high, 0.875 * 24.0, 1.0, hz);
    }

    /// FUN_100080f0, the host's resume: every voice lets go, the delay
    /// lines empty, and the effects start again.
    pub fn reset(&mut self) {
        for v in &mut self.voices {
            v.release();
            v.clear();
            v.release_time = 1e8;
        }
        self.held.keys.clear();
        self.strum_count = 0;
        self.strum_clock = 0;
        self.quiet = 5;
        self.pending.clear();
        for buf in &mut self.fx_buf {
            buf.fill(0.0);
        }
        self.cab_buf.fill(0.0);
        self.feedback.fill(0.0);
        self.fx_fb = [0.0; 4];
    }

    /// Takes the Aid mode and the latch as already seen, as a render
    /// before the first note does in FL: a state loaded then lets no voice
    /// go.
    pub fn settle(&mut self) {
        self.aid_seen = self.aid();
        self.latch_seen = self.params[p::LATCH] as i32 != 0;
    }

    pub fn push(&mut self, event: Event) {
        if self.pending.len() < self.pending.capacity() {
            self.pending.push(event);
        }
    }

    pub fn release_all(&mut self) {
        let keys: [i32; VOICES] = std::array::from_fn(|k| i32::from(self.voices[k].key()));
        for key in keys {
            self.push(Event::Off { key });
        }
    }

    fn aid(&self) -> i32 {
        (self.params[p::AID] * 5.99) as i32
    }

    /// FUN_100087f0, the process: render up to the next control tick, take
    /// the events, step the envelopes, and so on. `left` and `right` are
    /// added to.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32], mono: &mut [f32]) {
        let mut done = 0;
        let mut left_to_do = left.len();
        while left_to_do > 0 {
            let due = self.tick as i32;
            if (left_to_do as i32) < due {
                self.tick -= left_to_do as f32;
                self.render(&mut mono[done..done + left_to_do]);
                self.fx(
                    &mut left[done..done + left_to_do],
                    &mut right[done..],
                    &mono[done..done + left_to_do],
                );
                break;
            }
            let due = due.max(0) as usize;
            left_to_do -= due;
            if due != 0 {
                self.render(&mut mono[done..done + due]);
                self.fx(
                    &mut left[done..done + due],
                    &mut right[done..],
                    &mono[done..done + due],
                );
                done += due;
            }
            let step = self.rate * 0.005;
            self.tick = (self.tick - due as f32) + step;
            let pending = std::mem::take(&mut self.pending);
            for event in &pending {
                match *event {
                    Event::On {
                        key,
                        velocity,
                        detune,
                    } => self.note_on(key, velocity, detune),
                    Event::Off { key } => self.note_off(key),
                }
            }
            self.pending = pending;
            self.pending.clear();
            self.envelopes();
        }
    }

    /// FUN_100032f0: a voice starts on this key.
    fn start(&mut self, key: i32, velocity: f32, detune: i8, delay: i32) {
        if !(0..128).contains(&key) {
            return;
        }
        let at = self.allocate(key as i8);
        let v = &mut self.voices[at];
        v.gate = 1.0;
        v.held = true;
        v.note = f32::from(key as i8);
        v.env_stage = 4;
        v.env_out = 0.0;
        v.env_phase = 0.0;
        v.sample_pos = 0.0;
        v.fret_lp = 0.0;
        v.lp_a = 0.0;
        v.detune = detune;
        v.velocity = velocity;
        v.release_time = -1.0;
        v.age = 0;
        v.phase = 0.0;
        v.history[0] = 0.0;
        v.amp = 0.0;
        v.delay = delay.max(0);
        self.phase_248 = 0.0;
        self.phase_244 = 0.5;
        self.quiet = 0;
    }

    /// FUN_100051f0: the voice on this key, else one long silent, else one
    /// let go, else the oldest.
    fn allocate(&self, key: i8) -> usize {
        let note = f32::from(key);
        if let Some(at) = self.voices.iter().position(|v| v.note == note) {
            return at;
        }
        if let Some(at) = self
            .voices
            .iter()
            .position(|v| self.rate * 0.3 <= v.release_time)
        {
            return at;
        }
        if let Some(at) = self.voices.iter().position(|v| v.gate < 0.5) {
            return at;
        }
        let mut oldest = (0, 0);
        for (k, v) in self.voices.iter().enumerate() {
            if oldest.0 < v.age {
                oldest = (v.age, k);
            }
        }
        oldest.1
    }

    /// The release of FUN_100038a0 and the note-offs: the first sounding
    /// voice on the key lets go, or under the latch only stops being held.
    fn release_key(&mut self, key: i32, under_latch: bool) {
        if !(0..128).contains(&key) {
            return;
        }
        let latch = self.latch;
        if let Some(v) = self
            .voices
            .iter_mut()
            .find(|v| v.gate != 0.0 && v.key() == key as i8)
        {
            v.held = false;
            if latch && under_latch {
                return;
            }
            v.release();
        }
    }

    fn release_voices(&mut self) {
        for k in 0..VOICES {
            let key = i32::from(self.voices[k].key());
            self.release_key(key, false);
        }
    }

    /// The glide's speed (0x829814): the solo modes slide over a time from
    /// the Timing wheel, in 200ths of a beat.
    fn glide(&mut self, velocity: f32, fixed: bool) {
        self.fret_noise = velocity * 0.3;
        let step = (self.params[p::TIMING] * 6.99) as usize;
        let distance = (self.glide_target as f32 - self.glide_note).abs();
        self.glide_rate = if fixed {
            const FIXED: [f32; 7] = [1.875, 3.75, 7.5, 15.0, 30.0, 60.0, 120.0];
            (distance / ((FIXED[step.min(6)] * 200.0) / self.tempo)).abs()
        } else {
            const DYNAMIC: [f32; 7] = [12.0, 6.0, 3.0, 1.5, 0.75, 0.375, 0.1875];
            ((DYNAMIC[step.min(6)] * 200.0) / self.tempo) / distance
        };
    }

    /// FUN_100033d0: a key goes down, by the Aid mode.
    fn note_on(&mut self, key: i32, velocity: f32, detune: i8) {
        self.latch = self.params[p::LATCH] as i32 != 0;
        let timing = self.params[p::TIMING] * 10.99;
        match self.aid() {
            0 => self.start(key, velocity, detune, 0),
            1 => {
                self.release_voices();
                let chord = CHORDS[key.rem_euclid(12) as usize];
                let mut delay = 0;
                for &tone in chord.iter().take_while(|&&t| t >= 0) {
                    self.start(i32::from(tone) + 48, velocity, 0, delay);
                    delay += timing as i32;
                }
            }
            2 => {
                self.release_voices();
                let spread = timing as i32;
                if key < 60 {
                    self.start(key + 12, velocity, detune, 0);
                    self.start(key + 17, velocity, detune.wrapping_add(5), spread);
                    self.start(key + 24, velocity, detune.wrapping_add(2), spread * 2);
                } else {
                    self.start(key - 12, velocity, detune, 0);
                    self.start(key - 5, velocity, detune.wrapping_sub(4), spread);
                    self.start(key, velocity, detune.wrapping_sub(2), spread * 2);
                }
            }
            3 => {
                if self.strum_count == 0 {
                    self.strum_clock = 0;
                }
                let delay = self.strum_count * timing as i32 - self.strum_clock;
                self.start(key, velocity, detune, delay);
                self.strum_count += 1;
            }
            aid @ (4 | 5) => {
                self.glide_target = key;
                if self.held.push(key as i8) != 0 {
                    self.glide(velocity, aid == 4);
                } else {
                    self.start(key, velocity, detune, 0);
                    self.glide_note = key as f32;
                }
            }
            _ => {}
        }
    }

    /// FUN_10003920: a key goes up, by the Aid mode.
    fn note_off(&mut self, key: i32) {
        self.latch = self.params[p::LATCH] as i32 != 0;
        match self.aid() {
            0 => self.release_key(key, true),
            1 => {
                let chord = CHORDS[key.rem_euclid(12) as usize];
                for &tone in chord.iter().take_while(|&&t| t >= 0) {
                    self.release_key(i32::from(tone) + 48, true);
                }
            }
            2 => {
                if key >= 60 {
                    for k in [key - 12, key - 5] {
                        self.release_key(k, true);
                    }
                    for k in [key, key + 7, key + 12] {
                        self.release_key(k, true);
                    }
                } else {
                    for k in [key + 12, key + 17, key + 24, key + 29, key + 36] {
                        self.release_key(k, true);
                    }
                }
            }
            3 => {
                self.release_key(key, true);
                self.strum_count -= 1;
            }
            aid @ (4 | 5) => {
                self.held.remove(key as i8);
                if let Some(&top) = self.held.keys.first() {
                    self.glide_target = i32::from(top);
                    // FL's takes the note-off's velocity, which the host
                    // here does not have.
                    self.glide(0.0, aid == 4);
                } else {
                    for k in 0..VOICES {
                        let key = i32::from(self.voices[k].key());
                        self.release_key(key, false);
                    }
                }
            }
            _ => {}
        }
    }

    /// FUN_10003fe0, once a control tick: each voice's excitation
    /// envelope, after any strum delay.
    fn envelopes(&mut self) {
        let x = (1.0 - (self.params[p::TONE] * 0.5 + 0.25)) * 0.892_27 + 0.107_73;
        let x = f64::from(x);
        let decay = 20.0 / (x * x * x * 800.0);
        let release = (20.0 / (decay * decay * decay * 800.0)) as f32;
        let decay = decay as f32;
        for v in &mut self.voices {
            if v.env_stage == 0 {
                continue;
            }
            if v.delay > 0 {
                v.delay -= 1;
                continue;
            }
            match v.env_stage {
                1 => {
                    v.env_phase += release;
                    if v.env_phase < 1.0 {
                        v.env_out = (1.0 - v.env_phase) * v.env_release;
                    } else {
                        v.env_stage = 0;
                        v.env_out = 0.0;
                    }
                }
                2 => v.env_out = 0.0,
                3 => {
                    v.env_phase += decay;
                    if 1.0 <= v.env_phase {
                        v.env_phase = 0.0;
                        v.env_stage = 2;
                        v.env_out = 0.0;
                    } else {
                        v.env_out = 1.0 - v.env_phase;
                    }
                }
                4 => {
                    v.env_phase += 19.995_396;
                    if v.env_phase < 1.0 {
                        v.env_out = v.env_phase;
                    } else {
                        v.env_phase = 0.0;
                        v.env_stage = 3;
                        v.env_out = 1.0;
                    }
                }
                _ => {}
            }
            v.env_target = v.env_out;
        }
    }

    /// Which excitation sample, its rate and gain, by string type and key
    /// (the switch of FUN_10004140).
    fn pick_sample(kind: i32, key: i8, velocity: f32) -> (u8, f32, f32) {
        let k = key;
        let rate4 = |a: f32, b: f32, c: f32, d: f32, edges: [i8; 3]| {
            if k < edges[0] {
                a
            } else if k < edges[1] {
                b
            } else if k < edges[2] {
                c
            } else {
                d
            }
        };
        let third = f32::from_bits(0x3d2a_64c3);
        match kind {
            1 => {
                let index = if k < 50 {
                    0xe
                } else if k < 60 {
                    0xf
                } else if k < 70 {
                    0x10
                } else if k < 80 {
                    0x11
                } else {
                    0x12
                };
                (index, 1.0, 0.125)
            }
            2 => {
                let twelfth = f32::from_bits(0x3daa_9931);
                if k < 50 {
                    (0x13, 1.0, twelfth)
                } else if k < 60 {
                    (0x14, 1.0, twelfth)
                } else if k < 70 {
                    (0x15, 1.0, 0.125)
                } else {
                    (0x16, 1.0, 0.125)
                }
            }
            3 => {
                let rate = if k < 48 {
                    f32::from_bits(0x3f47_ae14)
                } else if k < 56 {
                    f32::from_bits(0x3f79_999a)
                } else if k < 62 {
                    f32::from_bits(0x3fa6_6666)
                } else if k < 70 {
                    f32::from_bits(0x3fd8_51ec)
                } else if k < 78 {
                    f32::from_bits(0x400d_70a4)
                } else {
                    f32::from_bits(0x4037_0a3d)
                };
                (1, rate, third)
            }
            4 => {
                let (low, mid, high, top) = (0.9, 1.125, 1.5, f32::from_bits(0x3ff9_999a));
                match (velocity * 2.99) as i32 {
                    0 => (
                        0x1b,
                        rate4(low, mid, high, top, [48, 58, 68]),
                        f32::from_bits(0x3d57_7319),
                    ),
                    1 => (0x1a, rate4(low, mid, high, top, [48, 58, 68]), 0.0625),
                    2 => (
                        0x1c,
                        rate4(1.8, 2.25, 3.0, f32::from_bits(0x4079_999a), [48, 58, 68]),
                        0.031_25,
                    ),
                    _ => (0x1c, f32::from_bits(0x4079_999a), 0.031_25),
                }
            }
            5 => (
                7,
                rate4(0.9, 1.125, 1.5, f32::from_bits(0x3ff9_999a), [48, 58, 68]),
                0.125,
            ),
            6 => {
                let rate = if k < 48 {
                    0.9
                } else if k < 58 {
                    1.125
                } else if k > 67 {
                    f32::from_bits(0x3ff9_999a)
                } else {
                    1.5
                };
                (0x18, rate, third)
            }
            _ => (0, 1.0, 0.0),
        }
    }

    /// FUN_10004140: the six strings, mono, into `out`.
    fn render(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        let n = out.len();
        let rate = self.rate;
        let params = self.params;
        self.sounding = false;
        let aid = self.aid();
        if self.aid_seen != aid {
            self.aid_seen = aid;
            self.release_voices();
            self.strum_count = 0;
            self.held.keys.clear();
        }
        self.latch = params[p::LATCH] as i32 != 0;
        if self.latch != self.latch_seen {
            self.latch_seen = self.latch;
            if !self.latch {
                for k in 0..VOICES {
                    if !self.voices[k].held {
                        let key = i32::from(self.voices[k].key());
                        self.release_key(key, false);
                    }
                }
            }
        }
        // A voice that went quiet under the latch lets go of the key with
        // the voice's own number, as FL's does.
        for k in 0..VOICES {
            let v = &self.voices[k];
            if v.gate != 0.0 && (rate * 0.2 <= v.release_time || 3 < self.quiet) {
                self.release_key(k as i32, false);
                if self.latch {
                    self.voices[k].release_time = 1e9;
                }
            }
        }
        let ramp = 1.0 / (rate * 0.005);
        self.feedback_drive = 0.0;
        let read_from = self.feedback_read;
        self.strum_clock += 1;
        if 3 < self.quiet {
            self.feedback_read = read_from.wrapping_add(n as u32);
            return;
        }
        let kind = (params[p::STRINGS] * 6.99) as i32;
        for k in 0..VOICES {
            self.voices[k].age += 1;
            let v = &self.voices[k];
            if v.delay != 0 || rate * 0.2 <= v.release_time {
                continue;
            }
            self.sounding = true;
            if 0.5 < v.gate {
                self.feedback_drive = v.velocity * 1.5;
            }
            self.feedback_drive = self.feedback_drive.min(1.0);
            let target = if kind != 0 { 1.0 } else { v.env_target };
            let mut amp = v.amp;
            let keyed = (v.note - 64.0) * 0.031_25 + 1.0;
            let soft = 1.0 - v.velocity;
            let damp = (((((v.velocity - 1.0) * 3.0) / (soft + soft + 1.0)) + 1.0)
                * params[p::VEL_D]
                + (1.0 - params[p::VEL_D]) * (1.0 - params[p::DAMP]))
                * keyed;
            let damp = if damp < 0.03 { 0.03 } else { damp.min(0.999) };
            let low = (128.0 - v.note) * 0.007_812_5;
            let low = low * low * 16.0 + 1.0;
            if self.fret_ratio != self.fret_ratio_seen {
                self.fret_ratio_seen = self.fret_ratio;
                self.fret_noise =
                    params[p::FRET] * v.velocity * 0.08 + (1.0 - params[p::FRET]) * 0.05;
            }
            if aid < 4 {
                self.glide_note = v.note;
            } else {
                let to = self.glide_target as f32;
                let mut at = self.glide_note;
                if to < at {
                    at -= self.glide_rate;
                    if at < to {
                        at = to;
                    }
                } else if at < to {
                    at += self.glide_rate;
                    if to < at {
                        at = to;
                    }
                }
                self.glide_note = at;
            }
            let period = ((f64::from(STRING_RATE)
                / (semis(self.glide_note) * f64::from(8.175_75_f32)))
                / f64::from(self.fret_ratio))
                / (f64::from(self.bend) * f64::from(0.1224_f32) + 1.0);
            let detune =
                self.cents[(i32::from(self.voices[k].detune) + 127).clamp(0, 254) as usize];
            let period = (period + period) * f64::from(detune);
            let len_a = (period * f64::from(0.999_200_64_f32)) as f32;
            let len_b = (period * f64::from(1.000_8_f32)) as f32;
            let v = &mut self.voices[k];
            if 0.5 < v.gate {
                v.velocity_held = v.velocity;
            }
            let held = v.velocity_held;
            let loss = ((1.0 - params[p::VEL_V])
                + held * held * params[p::VEL_V] * params[p::VEL_V])
                * 0.97
                * keyed;
            let loss = if loss < 0.03 { 0.03 } else { loss.min(1.0) };
            let keep = 1.0 - loss;
            let feed = params[p::FEEDBACK] * params[p::FEEDBACK] * 0.001;
            let gain = (1.0 - params[p::DAMP]) * 0.01 + 0.99;
            let mut speed = params[p::TONE] * 0.5 + 0.25;
            speed = if 0.5 <= speed {
                (speed - 0.5) * 6.0 + 1.0
            } else {
                speed * 1.5 + 0.25
            };
            let vel_gain = (params[p::VEL_V] * v.velocity + 1.0) - params[p::VEL_V];
            let limit = 1.0 - params[p::SLAP] * 0.95;
            let limit = limit * limit;
            let makeup = 1.0 / ((1.0 - params[p::SLAP] * 0.98) + TINY);
            let mut coef = (2.0
                * ((f64::from(params[p::HARMONIC]) * 6000.0 + 1000.0)
                    * f64::from(7.123_787e-5_f32))
                .sin()) as f32;
            if 0.99 < coef {
                coef = 0.99;
            }
            let mut pick = ((params[p::PICKUP] + 0.05)
                * NOTE_SPREAD[(v.note as i32).clamp(0, 127) as usize % 6]
                * 100.0) as i32;
            let (mut w1, mut w2, mut pick2) = (0.0f32, 0.0f32, 0i32);
            match (params[p::COILS] * 2.99) as i32 {
                1 => {
                    pick2 = pick * 2;
                    w1 = -1.0;
                    w2 = 1.0;
                }
                2 => {
                    w1 = -1.0;
                    w2 = 1.0;
                    pick = (pick as f32 * len_b * 0.002) as i32;
                    pick2 = pick * 2;
                }
                _ => {}
            }
            let key = v.note as i32 as i8;
            if (1..=6).contains(&kind) {
                let (s, r, g) = Self::pick_sample(kind, key, v.velocity_held);
                v.sample = s;
                v.sample_rate = r;
                v.sample_gain = g;
            }
            let mut pos = v.sample_pos;
            let mut excited = true;
            let level = vel_gain * speed * low;
            let svf = &mut self.voice_svf[k];
            let step = STRING_RATE / rate;
            self.feedback_read = read_from;
            let (ia, fa) = (len_a as i32, len_a - len_a as i32 as f32);
            let (ib, fb) = (len_b as i32, len_b - len_b as i32 as f32);
            for (j, sample) in out.iter_mut().enumerate() {
                if 1.0 <= v.phase {
                    let last = loop {
                        let mut exc = TINY;
                        if v.env_stage != 0 {
                            if amp <= target {
                                if amp < target {
                                    amp += ramp;
                                    if target < amp {
                                        amp = target;
                                    }
                                }
                            } else {
                                amp -= ramp;
                                if amp < target {
                                    amp = target;
                                }
                            }
                            if kind == 0 {
                                let r = self.rand.next() as f32;
                                let noise = (f64::from(r) * 3.051e-5 - 0.5) as f32;
                                exc = noise * amp * level * 0.125;
                            } else if let Some(table) =
                                self.excitations.as_ref().and_then(|e| e.get(v.sample))
                            {
                                let len = table.len() as f32;
                                if len - 1.0 <= pos {
                                    excited = false;
                                } else {
                                    let i = pos as i32;
                                    let f = pos - i as f32;
                                    let i = i.max(0) as usize;
                                    let a = f32::from(table[i]);
                                    let b = f32::from(*table.get(i + 1).unwrap_or(&0));
                                    exc = (b * f + (1.0 - f) * a)
                                        * amp
                                        * v.sample_gain
                                        * level
                                        * 3.051_757_8e-5;
                                    pos += speed * v.sample_rate;
                                }
                            } else {
                                excited = false;
                            }
                        }
                        if 0.005 < self.fret_noise {
                            let saw = self.fret_phase - 0.5;
                            let level = self.fret_noise;
                            self.fret_phase += 0.01;
                            self.fret_phase = wrap(self.fret_phase);
                            self.fret_noise *= 0.9997;
                            exc = keep * v.fret_lp + (saw * level * vel_gain + exc) * loss;
                            v.fret_lp = exc;
                        }
                        let direct = exc * params[p::FRET];
                        v.index = (v.index + 1) & LINE_MASK;
                        let i = v.index;
                        let fbn = self.feedback[(self.feedback_read as usize) & LINE_MASK] * feed;
                        let read = |line: &[f32], whole: i32, frac: f32| {
                            let at = i.wrapping_sub(whole as usize);
                            frac * line[at.wrapping_sub(1) & LINE_MASK]
                                + (1.0 - frac) * line[at & LINE_MASK]
                        };
                        let a = read(&v.line_a, ia, fa) * gain + exc + fbn;
                        let b = fbn + read(&v.line_b, ib, fb) * gain + exc;
                        let a = a.clamp(-limit, limit);
                        let b = b.clamp(-limit, limit);
                        v.lp_a = a * damp + (1.0 - damp) * v.lp_a;
                        v.line_a[i] = v.lp_a;
                        v.lp_b = b * damp + (1.0 - damp) * v.lp_b;
                        v.line_b[i] = v.lp_b;
                        let mut string = (b * 0.4 + a) * 0.85 + direct;
                        if v.gate < 0.5 {
                            v.release_time += speed;
                            if amp > 0.0 {
                                amp -= ramp;
                                if amp < 0.0 {
                                    amp = 0.0;
                                }
                            } else if amp < 0.0 {
                                amp += ramp;
                                if amp > 0.0 {
                                    amp = 0.0;
                                }
                            }
                            string *= amp;
                        }
                        let string = string.clamp(-limit, limit);
                        v.pickup[i] = string;
                        let t1 = v.pickup[i.wrapping_sub(pick as usize) & LINE_MASK];
                        let t2 = v.pickup[i.wrapping_sub(pick2 as usize) & LINE_MASK];
                        svf.low += coef * svf.band;
                        let high =
                            ((t1 * w1 + t2 * w2 + string + TINY) * 0.5 - svf.low) - svf.band * 0.5;
                        svf.high = high;
                        svf.band += high * coef;
                        v.phase -= 1.0;
                        if v.phase < 1.0 {
                            break svf.low;
                        }
                    };
                    v.history = [makeup * last, v.history[0], v.history[1], v.history[2]];
                }
                self.feedback_read = read_from.wrapping_add(j as u32 + 1);
                v.phase += step;
                let t = v.phase;
                let [y0, y1, y2, y3] = v.history;
                *sample += ((y1 - y3) * 0.5
                    + ((((y2 - y1) * 3.0 - y3) + y0) * 0.5 * t
                        + ((y1 + y1 + y3) - (y2 * 5.0 + y0) * 0.5))
                        * t)
                    * t
                    + y2;
            }
            if !excited {
                v.env_stage = 0;
            }
            v.amp = amp;
            v.sample_pos = pos;
        }
        self.feedback_read = read_from.wrapping_add(n as u32);
    }

    /// The three bands of the EQ (FUN_10002ee0 three times).
    fn eq3(&mut self, x: f32) -> f32 {
        let y = self.eq[0].run(x);
        let y = self.eq[1].run(y);
        self.eq[2].run(y)
    }

    fn eq_gain(&self) -> f32 {
        ((3.5 - self.params[p::EQ_LOW]) - self.params[p::EQ_MID]) - self.params[p::EQ_HIGH]
    }

    /// FUN_10005d20: two allpass chains and a quadrature oscillator, the
    /// frequency shifter of both phasers.
    fn shift(&mut self, x: f32, phase: f32) -> f32 {
        let s = &mut self.shifter;
        let f1 = (s[6] - x) * 0.232_829 + s[4];
        s[0] = f1;
        let f2 = (s[8] - f1) * 0.843_573 + s[6];
        s[1] = f2;
        let t = s[10];
        s[10] = s[9];
        let f3 = (t - f2) * 0.980_351 + s[8];
        s[8] = s[7];
        s[6] = s[5];
        s[4] = s[3];
        let old = s[20];
        s[2] = f3;
        s[9] = f3;
        let g = (s[17] - x) * 0.612_779;
        s[7] = s[1];
        s[5] = s[0];
        let g = g + s[15];
        s[3] = x;
        s[11] = g;
        let h = (s[19] - g) * 0.942_636 + s[17];
        s[12] = h;
        let t = s[21];
        s[21] = old;
        let h = (t - h) * 0.995_302;
        let t2 = s[19];
        s[19] = s[18];
        s[17] = s[16];
        s[15] = s[14];
        s[13] = h + t2;
        let ph = phase * ENGINE_TAU;
        s[20] = s[13];
        s[18] = s[12];
        s[16] = s[11];
        s[14] = x;
        let sin = self.sine[((ph * 10_430.055) as i32 & 0xffff) as usize];
        let cos = self.sine[(((ph + 1.570_795) * 10_430.055) as i32 & 0xffff) as usize];
        (f64::from(sin) * f64::from(f3) + f64::from(cos) * f64::from(old)) as f32
    }

    /// The amp's feedback oscillator: a resonance near 2 kHz that the amp
    /// rings, written for the strings to read.
    fn ring(&mut self, input: f32) {
        let s = (f64::from(1.0 / self.rate) * f64::from(12_566.36_f32)).sin();
        let svf = &mut self.amp_svf;
        let low = (s + s) * f64::from(svf.band) + f64::from(svf.low);
        svf.low = low as f32;
        let high = (f64::from(input) * 0.7 - low) - f64::from(svf.band) * 0.7;
        svf.high = high as f32;
        let band = ((s + s) * high + f64::from(svf.band)) * f64::from(self.feedback_drive);
        svf.band = band as f32;
        self.feedback[(self.feedback_write as usize) & LINE_MASK] = (band * 8.0) as f32;
        self.feedback_write = self.feedback_write.wrapping_add(1);
    }

    /// FUN_100056a0: the tube, three one-poles each into the sine clipper,
    /// then the EQ.
    fn tube(&mut self, x: f32, presence: f32, drive: f32) -> f32 {
        let f = ((presence * presence * presence * 0.9 + 0.05) * STRING_RATE) / self.rate;
        let g = drive * drive * 32.0 + 1.0;
        let k = 1.0 - f;
        let fb = self.params[p::FEEDBACK] * self.amp_svf.band * self.params[p::FEEDBACK] * 0.1;
        self.tube[0] = k * self.tube[0] + (fb + x) * f;
        let y = clip_sin(&self.sine, self.tube[0] * g);
        self.tube[1] = k * self.tube[1] + y * f;
        let y = clip_sin(&self.sine, self.tube[1] * g);
        self.tube[2] = k * self.tube[2] + y * f;
        let y = clip_sin(&self.sine, self.tube[2] * g);
        let e = self.eq3(y + TINY);
        self.eq_gain() * e * 0.5 * 0.5
    }

    /// FUN_100058a0: the EQ first, then the three clipping stages.
    fn eq_amp(&mut self, x: f32, presence: f32, drive: f32) -> f32 {
        let f = ((presence * presence * presence * 0.9 + 0.05) * STRING_RATE) / self.rate;
        let g = drive * drive * 32.0 + 1.0;
        let fb = self.params[p::FEEDBACK] * self.amp_svf.band * self.params[p::FEEDBACK] * 0.1;
        let e = self.eq3(fb + x + TINY);
        let k = 1.0 - f;
        let first = f64::from(f) * f64::from(self.eq_gain()) * f64::from(e) * 0.5
            + f64::from(k) * f64::from(self.tube[0]);
        self.tube[0] = first as f32;
        let y = clip_sin(&self.sine, (first * f64::from(g)) as f32);
        self.tube[1] = k * self.tube[1] + y * f;
        let y = clip_sin(&self.sine, self.tube[1] * g);
        self.tube[2] = k * self.tube[2] + y * f;
        clip_sin(&self.sine, self.tube[2] * g) * 0.5
    }

    /// FUN_10005aa0: three state-variable bandpasses, each into the
    /// clipper, then the EQ.
    fn bandpass_amp(&mut self, x: f32, presence: f32, drive: f32) -> f32 {
        let c = ((f64::from(ENGINE_PI) / f64::from(self.rate))
            * (f64::from(presence) * f64::from(presence) * 2000.0 + 300.0))
            .sin();
        let c = (c + c) as f32;
        let g = drive * drive * 32.0 + 1.0;
        let fb = self.params[p::FEEDBACK] * self.amp_svf.band * self.params[p::FEEDBACK] * 0.1;
        let mut input = fb + x + 0.001;
        for k in 0..3 {
            let s = &mut self.band[k];
            s.low += c * s.band;
            s.high = (input - s.low) - s.band;
            s.band += s.high * c;
            let y = clip_sin(&self.sine, s.band * g);
            input = if k < 2 { y + 0.001 } else { y };
        }
        let e = self.eq3(input + TINY);
        self.eq_gain() * e * 0.5 * 0.7
    }

    /// FUN_10005ef0: the effects, the amp and cabinet, and the speaker,
    /// from the strings' `mono` into `left` and `right` (added to).
    fn fx(&mut self, left: &mut [f32], right: &mut [f32], mono: &[f32]) {
        let n = left.len();
        let inv = 1.0 / self.rate;
        let params = self.params;
        // Under the latch, a block the strings left silent counts.
        if self.latch {
            let sum: f32 = mono.iter().map(|x| -x.abs()).sum();
            if sum.abs() < 1e-6 {
                self.quiet += 1;
            }
        }
        let mut l = [0.0f32; 512];
        let mut r = [0.0f32; 512];
        let mut done = 0;
        // The engine's buffers are as long as the host's block; ours are
        // walked in pieces, which only the coefficient updates notice.
        while done < n {
            let len = (n - done).min(512);
            l[..len].copy_from_slice(&mono[done..done + len]);
            self.fx_block(&mut l[..len], &mut r[..len], params, inv);
            for k in 0..len {
                left[done + k] += l[k];
                right[done + k] += r[k];
            }
            done += len;
        }
    }

    fn fx_block(&mut self, l: &mut [f32], r: &mut [f32], params: [f32; 28], inv: f32) {
        let rate = self.rate;
        let out_gain = self.volume * self.expression * 1.3;
        self.eq[0].shelf(false, 500.0, params[p::EQ_LOW] * 20.0 - 10.0, 1.0, rate);
        self.eq[1].peak(1000.0, params[p::EQ_MID] * 20.0 - 10.0, 1.5, rate);
        self.eq[2].shelf(true, 2500.0, params[p::EQ_HIGH] * 20.0 - 10.0, 1.0, rate);
        let fx = (params[p::FX] * 15.99) as i32;
        let (p1, p2) = (params[p::FX1], params[p::FX2]);
        let sounding = self.sounding;
        let mut skip_amp = false;
        match fx {
            1 => {
                for x in l.iter_mut() {
                    let s = *x + TINY;
                    self.phase_240 = wrap(p1 * inv * 8.82 + self.phase_240);
                    let y = self.shift(self.fb_5d0 * p2 * 0.8 + s, self.phase_240);
                    self.fb_5d0 = y;
                    *x = y + s;
                }
            }
            2..=8 if !sounding => skip_amp = true,
            2 => {
                for x in l.iter_mut() {
                    let s = *x + TINY;
                    let k = (p1 * 3.99) as usize;
                    self.phase_248 =
                        wrap((self.tempo / (rate * 30.0)) / SYNC[k.min(3)] + self.phase_248);
                    let t = tri(self.phase_248);
                    *x = (((t - 0.5) + (t - 0.5)) * p2 + 1.0) * s;
                }
            }
            3 => {
                for x in l.iter_mut() {
                    let s = *x;
                    let c = p1 * 0.0049 + 0.995;
                    self.env_5cc = c * self.env_5cc + (1.0 - c) * s.abs();
                    let cut = (self.env_5cc * p2 * 3.0 + 0.015) * inv * STRING_RATE;
                    let y = self.ladders[1].run(s, cut, 0.75, Some(0.9));
                    *x = ((f64::from(y) * 1.5 + f64::from(s)) * 0.5) as f32;
                }
            }
            4 => {
                for x in l.iter_mut() {
                    let s = *x;
                    self.env_5cc = s.abs() * 0.003_000_021 + self.env_5cc * 0.997;
                    self.phase_244 = wrap((p1 + 0.1) * inv * 8.82 + self.phase_244);
                    let t = tri(self.phase_244);
                    let cut =
                        (((t - 0.5) + (t - 0.5) + 1.0) * self.env_5cc * self.env_5cc * p2 * 4.0
                            + 0.015)
                            * inv
                            * STRING_RATE;
                    let y = self.ladders[1].run(s, cut, 0.75, Some(0.9));
                    *x = ((f64::from(y) * 1.5 + f64::from(s)) * 0.5) as f32;
                }
            }
            5 => {
                for x in l.iter_mut() {
                    let s = *x + TINY;
                    let times = (p1 * 15.0 + 1.0) as i32;
                    let hz = semis(self.glide_note) * f64::from(8.175_75_f32);
                    let ph = f64::from(times) * (hz / f64::from(rate)) * 0.0625
                        + f64::from(self.phase_250);
                    self.phase_250 = ph as f32;
                    self.phase_250 = wrap(self.phase_250);
                    let sin = (f64::from(self.phase_250) * f64::from(ENGINE_TAU)).sin();
                    *x = (sin * f64::from(s) * 2.0) as f32;
                }
            }
            6 => {
                for x in l.iter_mut() {
                    let s = f64::from(*x) + 1e-18;
                    let g = f64::from((p1 + 0.5) * 4.0);
                    let y = if s < 0.0 {
                        ((1.0 - (-(g * s)).exp()) / g).max(-0.5)
                    } else {
                        ((g * s).exp() - 1.0) / g
                    };
                    let y = if y > 0.5 { 0.5 } else { y };
                    let q = p2 * 0.1 + 1e-4;
                    *x = q * ((y / f64::from(q)) as f32) as i32 as f32;
                }
            }
            7 => {
                for x in l.iter_mut() {
                    let s = *x + TINY;
                    let a = s.abs();
                    self.oct_env[0] = a * 0.03 + self.oct_env[0] * 0.997;
                    self.oct_env[1] = a * 0.01 + self.oct_env[1] * 0.99;
                    let follow = (a - self.oct_env[1]) * self.oct_env[0];
                    self.oct_lp = self.oct_lp * 0.995 + s * 0.005;
                    let y = self.ladders[2].run(s - self.oct_lp, 0.005, 0.0, Some(1.0));
                    if 0.0 < self.oct_prev && y < 0.0 {
                        self.oct_flip = if self.oct_flip < 0.0 { 1.0 } else { -1.0 };
                    }
                    self.oct_prev = y;
                    self.oct_square = self.oct_flip * 0.008 + self.oct_square * 0.992;
                    *x = (self.oct_square * self.oct_env[0] * p2 + s) - follow * p1;
                }
            }
            8 => {
                for x in l.iter_mut() {
                    let t = f64::from(tri(self.lfo));
                    let s = ((t - 0.5) + (t - 0.5) + 1.0) * 0.5;
                    let g = f64::from(p2) * 1.5 + 0.5;
                    let input = *x + TINY + 0.001;
                    let w = g * f64::from(inv) * f64::from(3.139_92_f32);
                    let c1 = ((2250.0 - s * 1400.0) * w).sin();
                    let c2 = ((2900.0 - s * 700.0) * w).sin();
                    let q = s * 0.97 - 0.5;
                    let c3 = ((270.0 - (q * q - 0.25) * 1560.0) * w).sin();
                    let xs = input * 0.050_000_012;
                    let mut bands = [0.0f64; 3];
                    for (k, c) in [c1, c2, c3].into_iter().enumerate() {
                        let f = &mut self.fx_svf[k];
                        let c = c + c;
                        let low = c * f64::from(f.band) + f64::from(f.low);
                        f.low = low as f32;
                        let high = (f64::from(xs) - low) - f64::from(f.band) * 0.050_000_012;
                        f.high = high as f32;
                        let band = high * c + f64::from(f.band);
                        f.band = band as f32;
                        bands[k] = band;
                    }
                    *x = ((bands[1] * 0.9
                        + (1.0 - s * s * 0.3) * bands[0]
                        + (0.4 - ((s - 0.5) * (s - 0.5) - 0.25) * 1.6) * bands[2])
                        * 8.0) as f32;
                    self.lfo = wrap(p1 * inv * 2.205 + self.lfo);
                    self.fx_index[0] = self.fx_index[0].wrapping_add(1);
                }
            }
            _ => {}
        }
        if !skip_amp && sounding {
            self.amp(l, params, out_gain);
        }
        r.copy_from_slice(l);
        self.mst(l, r, fx, params, inv);
    }

    /// The amp, the cabinet and the speaker EQ.
    fn amp(&mut self, l: &mut [f32], params: [f32; 28], out_gain: f32) {
        let rate = self.rate;
        let kind = (params[p::AMP] * 3.99) as i32;
        let drive = params[p::DRIVE] * 0.5 + 0.25;
        let (len, res, mix, gain, cut) = match (params[p::CAB] * 3.99) as i32 {
            0 => (20, 0.0, 0.0, 0.7, 0.2),
            1 => (64, 0.5, 1.0, 1.2, 0.25),
            2 => (32, 0.6, 1.0, 1.4, 0.17),
            _ => (48, 0.5, 1.0, 1.0, 0.33),
        };
        let cut = (cut * STRING_RATE) / rate;
        let len = ((len as f32) * rate * 2.267_573_6e-5) as i32;
        let dry = 1.0 - mix;
        let presence = params[p::PRESENCE];
        for x in l.iter_mut() {
            let s = *x;
            let shaped = match kind {
                0 => {
                    self.hp_23c = self.hp_23c * 0.995 + s * 0.005;
                    let e = self.eq3((s - self.hp_23c) + TINY);
                    self.eq_gain() * e * 0.5 * drive * 3.0
                }
                1 => {
                    self.hp_23c = self.hp_23c * 0.995 + s * 0.005;
                    self.tube(s - self.hp_23c, presence, drive) * 0.5
                }
                2 => {
                    self.hp_23c = self.hp_23c * 0.995 + s * 0.005;
                    self.eq_amp(s - self.hp_23c, presence, drive) * 0.5
                }
                _ => self.bandpass_amp(s, presence, drive) * 0.5,
            };
            let c = self.ladders[0].run(shaped, cut, res, None);
            let i = self.cab_index as usize;
            self.cab_buf[i & 0x1ff] = c * 0.25;
            let d = self.cab_buf[i.wrapping_sub(len as usize) & 0x1ff];
            self.cab_index = self.cab_index.wrapping_add(1);
            let y = ((c * 0.25 + d) * mix + shaped * dry) * gain;
            self.ring(if kind == 0 { 0.0 } else { y });
            *x = y * out_gain;
        }
        // The speaker (FUN_100096d0), on the left only, which is all there
        // is yet.
        let c = 176.400_01 / rate;
        for x in l.iter_mut() {
            let s = *x + TINY;
            self.speaker_hp += (s - self.speaker_hp) * c;
            let h = (s - self.speaker_hp) + TINY;
            let y = self.speaker[0].run(h);
            *x = self.speaker[1].run(y);
        }
    }

    /// The stereo effects, after the left is copied to the right.
    fn mst(&mut self, l: &mut [f32], r: &mut [f32], fx: i32, params: [f32; 28], inv: f32) {
        let rate = self.rate;
        let (p1, p2) = (params[p::FX1], params[p::FX2]);
        let sounding = self.sounding;
        let [b0, b1, b2, b3] = &mut self.fx_buf;
        match fx {
            9 => {
                let depth = p2 * rate * 0.002_902_494_3;
                let speed = p1 * inv * 4.41;
                let base = (rate * 0.009_070_295) as i32 as f32;
                for (x, y) in l.iter_mut().zip(r.iter_mut()) {
                    let s = *x;
                    let w = self.fx_index[0] as usize;
                    b0[w & 0xfff] = s;
                    let t = tri(self.lfo);
                    let t = (t - 0.5) + (t - 0.5);
                    self.lfo = wrap(speed + self.lfo);
                    let d1 = (t + 1.0) * depth + base;
                    let d2 = (1.0 - t) * depth + base;
                    let tap = |d: f32| {
                        let i = d as i32;
                        let f = d - i as f32;
                        let at = w.wrapping_sub(i as usize);
                        f * b0[at.wrapping_sub(1) & 0xfff] + (1.0 - f) * b0[at & 0xfff]
                    };
                    let (a, b) = (tap(d1), tap(d2));
                    let half = s * 0.5;
                    *x = (b * 0.5 + half) * 1.6;
                    *y = (a * 0.5 + half) * 1.6;
                    self.fx_index[0] = self.fx_index[0].wrapping_add(1);
                }
            }
            10 => {
                let depth = p2 * rate * 0.002_902_494_3;
                let speed = p1 * 1.764 / rate;
                let base = (rate * 0.005_804_988_6) as i32 as f32;
                for (x, y) in l.iter_mut().zip(r.iter_mut()) {
                    let s = *x;
                    let w = self.fx_index[0] as usize;
                    b0[w & 0xfff] = self.fx_fb[0] * 0.8 + s;
                    b1[w & 0xfff] = self.fx_fb[1] * 0.8 + s;
                    let t = tri(self.lfo);
                    let t = (t - 0.5) + (t - 0.5);
                    self.lfo = wrap(speed + self.lfo);
                    let d1 = (t + 1.0) * depth + base;
                    let d2 = (1.0 - t) * depth + base;
                    let tap = |buf: &[f32], d: f32| {
                        let i = d as i32;
                        let f = d - i as f32;
                        let at = w.wrapping_sub(i as usize);
                        f * buf[at.wrapping_sub(1) & 0xfff] + (1.0 - f) * buf[at & 0xfff]
                    };
                    let (a, b) = (tap(b0, d1), tap(b1, d2));
                    let half = s * 0.5;
                    *x = (a * 0.5 + half) * 1.6;
                    *y = (b * 0.5 + half) * 1.6;
                    let fa = a * 0.3 + self.fx_fb[2] * 0.7;
                    let fb = self.fx_fb[3] * 0.7 + b * 0.3;
                    self.fx_index[0] = self.fx_index[0].wrapping_add(1);
                    self.fx_fb = [fa, fb, fa, fb];
                }
            }
            11 => {
                for (x, y) in l.iter_mut().zip(r.iter_mut()) {
                    let s = *x + TINY;
                    self.phase_240 = wrap(p1 * inv * 8.82 + self.phase_240);
                    let v = self.shift(self.fb_5d0 * p2 * 0.8 + s, self.phase_240) + TINY;
                    self.fb_5d0 = v;
                    *x = s - v;
                    *y = v + s;
                }
            }
            12 if sounding => {
                for (x, y) in l.iter_mut().zip(r.iter_mut()) {
                    let depth = rate * 0.000_907_029_5 * (1.0 - p1);
                    let w = self.fx_index[0];
                    b0[(w as usize) & 0x1ff] = *x;
                    let t = tri(self.lfo + 0.25);
                    let t = (t - 0.5) + (t - 0.5);
                    let d9 = (t + 1.0) * depth;
                    let d1 = (1.0 - t) * depth;
                    let tap = |d: f32| {
                        let i = d as i32;
                        let f = d - i as f32;
                        let ahead = ((i + 1) as f32 + w as f32 + 3.0) as i32;
                        let here = (w as f32 + i as f32 + 3.0) as i32;
                        f * b0[(ahead as usize) & 0x1ff]
                            + (1.0 - f) * b0[(here as usize) & 0x1ff]
                            + 0.001
                    };
                    let (a9, a1) = (tap(d9), tap(d1));
                    self.lfo = wrap((p1 + 0.2) * inv * 8.82 + self.lfo);
                    self.fx_index[0] = w.wrapping_add(1);
                    let t = f64::from(tri(self.lfo));
                    let t = (t - 0.5) + (t - 0.5);
                    let s1 = (t + 1.0) * 0.5;
                    let s2 = (1.0 - t) * 0.5;
                    let w0 = f64::from(inv) * f64::from(STRING_RATE) * f64::from(0.8_f32);
                    let c1 = ((s1 * s1 * 0.9 + 0.1) * w0).sin();
                    let c2 = ((s2 * s2 * 0.9 + 0.1) * w0).sin();
                    for (k, (c, input)) in [(c1, a1), (c2, a9)].into_iter().enumerate() {
                        let f = &mut self.fx_svf[k];
                        let c = c + c;
                        let low = c * f64::from(f.band) + f64::from(f.low);
                        f.low = low as f32;
                        let high = (f64::from(input) * 0.7 - low) - f64::from(f.band) * 0.7;
                        f.high = high as f32;
                        f.band = (c * high + f64::from(f.band)) as f32;
                    }
                    let m = t * f64::from(p2);
                    *x = ((m + 1.0) * f64::from(self.fx_svf[0].low) * 1.8) as f32;
                    *y = (1.0 - m) as f32 * self.fx_svf[1].low * 1.8;
                }
            }
            13 if sounding => {
                for (x, y) in l.iter_mut().zip(r.iter_mut()) {
                    let k = (p1 * 3.99) as usize;
                    self.phase_248 =
                        wrap((self.tempo / (rate * 30.0)) / SYNC[k.min(3)] + self.phase_248);
                    let t = tri(self.phase_248);
                    *x *= ((t - 0.5) + (t - 0.5)) * p2 + 1.0;
                    *y = *x;
                }
            }
            14 => {
                let d = (p1 * rate * 0.092_879_82) as i32 + 1;
                for (x, y) in l.iter_mut().zip(r.iter_mut()) {
                    let s = *x;
                    let w = ((self.fx_index[0] + 1) & 0x7fff) as usize;
                    self.fx_index[0] = w as i32;
                    let v = b0[w.wrapping_sub(d as usize) & 0x7fff];
                    b0[w] = s;
                    *x = v;
                    *y = s;
                }
            }
            15 => {
                let k = (p1 * 3.99) as usize;
                let long = (((rate * 60.0) / self.tempo) * SYNC[k.min(3)]) as i32;
                let long = long.clamp(1, FX_BUF as i32);
                let short = ((rate * 0.001_451_247_1) as i32).clamp(1, FX_BUF as i32);
                for (x, y) in l.iter_mut().zip(r.iter_mut()) {
                    let s = *x + TINY;
                    let i3 = (self.fx_index[2] + 1) % short;
                    self.fx_index[2] = i3;
                    let early = b3[i3 as usize];
                    b3[i3 as usize] = s;
                    let i2 = (self.fx_index[1] + 1) % (long / 2 + 1);
                    self.fx_index[1] = i2;
                    let half = b2[i2 as usize] * 0.2 + self.fx_fb[0] * 0.8;
                    self.fx_fb[0] = half;
                    b2[i2 as usize] = s;
                    let i1 = (self.fx_index[0] + 1) % long;
                    self.fx_index[0] = i1;
                    let i1 = i1 as usize;
                    let a = (b0[i1] * 0.2 + self.fx_fb[2] * 0.8) * p2 + s;
                    b0[i1] = a;
                    self.fx_fb[2] = a;
                    let b = (self.fx_fb[3] * 0.8 + b1[i1] * 0.2) * p2 + half;
                    b1[i1] = b;
                    self.fx_fb[3] = b;
                    *x = a * 0.7;
                    *y = (early + b) * 0.7;
                }
            }
            _ => {}
        }
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

/// The engine's chords for AutoChords (0x10015318): by the key's place in
/// its octave, semitones over 48, -1 ending a short one.
pub const CHORDS: [[i8; 6]; 12] = [
    [12, 16, 19, 24, 28, -1],
    [13, 16, 20, 25, 28, -1],
    [14, 21, 26, 29, -1, -1],
    [15, 22, 27, 30, -1, -1],
    [4, 11, 16, 19, 23, 26],
    [17, 21, 24, 29, -1, -1],
    [18, 21, 25, 30, -1, -1],
    [7, 11, 14, 19, 23, 31],
    [8, 11, 15, 20, 23, 32],
    [9, 16, 21, 24, 28, -1],
    [10, 17, 22, 26, 29, -1],
    [11, 17, 21, 26, -1, -1],
];

/// The beats of the tempo-synced effects (0x520c68).
const SYNC: [f32; 4] = [0.25, 0.5, 1.0, 2.0];

/// The pickup's spread by key (0x5dc): 1 to 11/6 in sixths, by the key's
/// place in six, with the engine's float bits.
const NOTE_SPREAD: [f32; 6] = [
    1.0,
    f32::from_bits(0x3f95_3f7d),
    f32::from_bits(0x3faa_9fbe),
    1.5,
    f32::from_bits(0x3fd5_3f7d),
    f32::from_bits(0x3fea_a5f8),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cents_table_is_one_at_its_middle_and_a_cent_a_step() {
        let e = Engine::new();
        assert_eq!(e.cents[127], 1.0);
        assert!((e.cents[128] - 1.000_577_8).abs() < 1e-6);
        assert!((e.cents[126] * e.cents[128] - 1.0).abs() < 1e-6);
        // 1200 cents would be 2: 127 of them are 2^(127/1200).
        assert!((e.cents[254] - 2f32.powf(127.0 / 1200.0)).abs() < 1e-4);
    }

    #[test]
    fn the_sine_clipper_reads_sin_of_0_785_x() {
        let e = Engine::new();
        for x in [-1.0f32, -0.5, 0.0, 0.3, 1.0, 4.0] {
            let want = (x.clamp(-1.0, 1.0) * 0.785).sin();
            assert!((clip_sin(&e.sine, x) - want).abs() < 1e-3, "{x}");
        }
    }

    #[test]
    fn a_shelf_lifts_its_side_by_its_gain() {
        let mut low = Biquad::default();
        low.shelf(false, 500.0, 10.0, 1.0, 44_100.0);
        let dc = (low.b.iter().sum::<f32>()) / (1.0 + low.a[0] + low.a[1]);
        assert!((20.0 * dc.log10() - 10.0).abs() < 0.01, "{dc}");
        let mut high = Biquad::default();
        high.shelf(true, 2500.0, -10.0, 1.0, 44_100.0);
        let nyq = (high.b[0] - high.b[1] + high.b[2]) / (1.0 - high.a[0] + high.a[1]);
        assert!((20.0 * nyq.log10() + 10.0).abs() < 0.05, "{nyq}");
        let dc = (high.b.iter().sum::<f32>()) / (1.0 + high.a[0] + high.a[1]);
        assert!((dc - 1.0).abs() < 1e-4);
    }

    #[test]
    fn a_peak_lifts_its_centre_by_its_gain() {
        let mut peak = Biquad::default();
        peak.peak(1000.0, 6.0, 1.5, 44_100.0);
        let w = std::f64::consts::TAU * 1000.0 / 44_100.0;
        let z = |k: f64| (k * w).cos();
        let zs = |k: f64| (k * w).sin();
        let b = peak.b.map(f64::from);
        let a = [1.0, f64::from(peak.a[0]), f64::from(peak.a[1])];
        let num = ((b[0] + b[1] * z(-1.0) + b[2] * z(-2.0)).powi(2)
            + (b[1] * zs(-1.0) + b[2] * zs(-2.0)).powi(2))
        .sqrt();
        let den = ((a[0] + a[1] * z(-1.0) + a[2] * z(-2.0)).powi(2)
            + (a[1] * zs(-1.0) + a[2] * zs(-2.0)).powi(2))
        .sqrt();
        assert!((20.0 * (num / den).log10() - 6.0).abs() < 0.01);
    }

    #[test]
    fn msvc_rand_starts_as_the_crt_does() {
        let mut r = Rand(1);
        assert_eq!([r.next(), r.next(), r.next()], [41, 18_467, 6334]);
    }

    #[test]
    fn a_string_rings_an_octave_under_its_key_from_two_delay_lines() {
        // Two lines of twice the key's period: the loop's fundamental is
        // half the key's frequency, as the engine has it.
        let hz = semis(69.0) * f64::from(8.175_75_f32);
        assert!((hz - 440.0).abs() < 0.01, "{hz}");
        let period = 2.0 * 44_100.0 / hz;
        assert!((period - 200.45).abs() < 0.01);
    }
}
