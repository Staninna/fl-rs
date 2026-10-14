//! Pitcher: pitch correction to a scale or to held notes, with a shifter
//! that keeps the formants and a harmonizer of four voices, as
//! `docs/plugins/pitcher.md` has it.
//!
//! The state is FL's wrapper around the plugin's own chunk: `SSLF`, the
//! chunk's length (152), then the chunk. That is `PTR1`, 33 parameters as
//! little-endian f32, twelve bytes that say which notes of the scale are
//! in it, and parameter 33. The parameters are the plugin's own numbers,
//! most from 0 to 1:
//!
//! | parameter | what | default |
//! |---|---|---|
//! | 0 | correction speed, 1 is at once | 1 |
//! | 1 | stereo spread of the harmony voices | 1 |
//! | 2 | gender, the formant shift | 0.5 |
//! | 3 | fine tune, -1 to +1 semitone | 0.5 |
//! | 4 | bypass all | 0 |
//! | 5 to 16 | bypass key C to B | 0 |
//! | 17 to 20 | pan of harmony voices 1 to 4 | 0.5 |
//! | 21 | not read by the sound | 0 |
//! | 22 | formant mode | 0 |
//! | 23 | the A the notes are reckoned from, 55 Hz -1 to +1 semitone | 0.5 |
//! | 24 | pitch reference: left, each side, right | 0 |
//! | 25 | MIDI mode: held notes are the scale | 0 |
//! | 26 | with MIDI: the notes' octaves count | 0 |
//! | 27 | harmonize | 0 |
//! | 28 | root key, 0 to 11, for the window | 0 |
//! | 29 | scale, 0 to 4, for the window | 3 |
//! | 30 | with harmonize: the input plays too | 0 |
//! | 31 | with harmonize: velocity sets a voice's level | 0 |
//! | 32 | lowest pitch: 220, 170, 110, 80, 55 or 25 Hz | 2 |
//! | 33 | MIDI input port | 0 |
//!
//! A detector on each side finds the period of the input by the average
//! magnitude difference of a band-passed copy, once every frame of
//! `rate / lowest pitch` samples. The nearest note of the scale gives a
//! correction in semitones, which a one-pole follows at the correction
//! speed. A delay-line shifter, or in formant mode a pitch-synchronous
//! overlap-add, plays the input at the corrected pitch.

use crate::{Effect, Fidelity, StateError};

/// The plugin's event 201 name.
pub const NAME: &str = "Pitcher";

/// How many parameters the plugin has.
const PARAMS: usize = 34;

/// The parameters by their use.
const SPEED: usize = 0;
const STEREO: usize = 1;
const GENDER: usize = 2;
const FINE: usize = 3;
const BYPASS_ALL: usize = 4;
const KEY_BYPASS: usize = 5;
const VOICE_PAN: usize = 17;
const FORMANT: usize = 22;
const A_FREQ: usize = 23;
const REFERENCE: usize = 24;
const MIDI: usize = 25;
const OCTAVES: usize = 26;
const HARMONIZE: usize = 27;
/// The scale the window last chose (parameter 28 is its root). The
/// sound goes by the twelve scale bytes alone.
const SCALE: usize = 29;
const WITH_INPUT: usize = 30;
const BY_VELOCITY: usize = 31;
const LOWEST: usize = 32;
const PORT: usize = 33;

/// The plugin's defaults (`FUN_18001d020`).
const fn defaults() -> [f32; PARAMS] {
    let mut params = [0.0; PARAMS];
    params[SPEED] = 1.0;
    params[STEREO] = 1.0;
    params[GENDER] = 0.5;
    params[FINE] = 0.5;
    params[VOICE_PAN] = 0.5;
    params[VOICE_PAN + 1] = 0.5;
    params[VOICE_PAN + 2] = 0.5;
    params[VOICE_PAN + 3] = 0.5;
    params[A_FREQ] = 0.5;
    params[SCALE] = 3.0;
    params[LOWEST] = 2.0;
    params
}

/// The lowest pitch each value of parameter 32 picks, in Hz.
const LOWEST_HZ: [f32; 6] = [220.0, 170.0, 110.0, 80.0, 55.0, 25.0];

/// The wrapper's magic and the chunk's, as they read little-endian.
const WRAPPER: u32 = 0x464c_5353;
const REGISTERED: u32 = 0x5054_5231;
const DEMO: u32 = 0x5054_4431;
/// How long the chunk is: the magic, 33 parameters, 12 scale bytes and
/// parameter 33.
const CHUNK: usize = 152;
/// Where the scale bytes are in the state, and where parameter 33 is.
const SCALE_AT: usize = 8 + 4 + 4 * 33;

/// How many notes the plugin keeps track of.
const NOTES: usize = 128;
/// The detector's band pass: a one pole at this coefficient taken off the
/// input, then sixteen one-poles at the second in double precision.
const DC_POLE: f32 = 0.6;
const STAGE_POLE: f64 = 0.125;
const STAGES: usize = 16;
/// What a filter state under this size becomes zero.
const TINY: f32 = 1e-9;
/// The deepest dip of the normalized difference that counts as a period,
/// and the highest pitch the detector takes.
const DIP: f32 = 0.3;
const HIGHEST_HZ: f32 = 1000.0;
/// The shifter's crossfade on a splice, and how near its reader may come
/// to the writer before it splices.
const FADE: i32 = 128;
/// How far behind the writer the shifter's reader starts.
const START_BEHIND: f32 = 10.0;
/// How fast a voice's level follows its target, a sample.
const LEVEL_POLE: f32 = 0.007;
/// The formant shifter's grain in silence or with no shift, and its hop.
const PLAIN_GRAIN: usize = 256;
const PLAIN_HOP: i32 = 128;
/// How far back from the newest sample the formant shifter looks for a
/// pitch mark, as a share of the period, and how far the mark is moved to
/// line up with the input.
const MARK_REACH: f64 = 1.1;
const MARK_LEAD: i32 = 70;

/// The four voices of one side.
const VOICES: usize = 4;

/// Pitcher.
#[derive(Debug, Clone)]
pub struct Pitcher {
    params: [f32; PARAMS],
    /// The pitch classes of the scale, C first (`+0x1670`).
    scale: [bool; 12],
    rate: f32,

    // What the parameters come to (`FUN_18001cab0`).
    speed: f32,
    fine: f32,
    gender: f32,
    a_freq: f32,
    reference: i32,
    bypass_all: bool,
    key_bypass: [bool; 12],
    formant: bool,
    midi: bool,
    octaves: bool,
    harmonize: bool,
    lowest: f32,
    port: i32,

    /// The velocity of each held note, from 0 to 1 (`+0x1068`).
    notes: [f32; NOTES],
    /// The notes correction may go to (`+0x172c`), and whether there is one.
    allowed: [bool; NOTES],
    any_allowed: bool,

    /// The detector's frame, a multiple of 4, and the shifter's ring, twice
    /// that.
    frame: usize,
    ring_len: usize,
    /// The formant shifter's rings, a tenth of a second.
    grain_ring_len: usize,

    sides: [Side; 2],
    /// Samples into the detector's frame (`+0x16f0`).
    count: usize,
    /// The shifter's writer (`+0x37bc`).
    write: usize,
    /// The formant shifter's writer (`+0x3864`).
    grain_write: usize,
    /// The formant switch moved: start the shifters over.
    restart: bool,
    /// A grain before it is added, with its window on.
    grain: Vec<f32>,
}

/// The harmony voices of a block: a shift in semitones and a level for
/// each side and voice, and whether the input plays beside them.
type Harmony = ([[f32; VOICES]; 2], [[f32; VOICES]; 2], bool);

/// What one side keeps.
#[derive(Debug, Clone, Default)]
struct Side {
    // The detector.
    dc: f32,
    stages: [f64; STAGES],
    /// The last `frame` band-passed samples, oldest first from `history_at`.
    history: Vec<f32>,
    history_at: usize,
    /// The difference summed for each lag over the frame.
    sums: Vec<f32>,
    /// The pitch found, 0 for none, and its period in samples.
    freq: f32,
    period: f32,
    /// Where the pitch is in its octave, its octave, the note it goes to.
    class: f32,
    octave: i32,
    target_note: i32,
    /// The correction aimed at and the one in use, in semitones.
    target: f32,
    correction: f32,

    // The delay-line shifter.
    ring: Vec<f32>,
    read: [f32; VOICES],
    fade: [i32; VOICES],
    old: [f32; VOICES],
    /// Each voice's level as it follows its target.
    level: [f32; VOICES],

    // The formant shifter.
    input: Vec<f32>,
    marks: Vec<f32>,
    out: Vec<f32>,
    countdown: [i32; VOICES],
}

impl Default for Pitcher {
    fn default() -> Self {
        let mut pitcher = Pitcher {
            params: [0.0; PARAMS],
            scale: [true; 12],
            rate: 44_100.0,
            speed: 1.0,
            fine: 0.0,
            gender: 1.0,
            a_freq: 55.0,
            reference: 0,
            bypass_all: false,
            key_bypass: [false; 12],
            formant: false,
            midi: false,
            octaves: false,
            harmonize: false,
            lowest: LOWEST_HZ[2],
            port: -1,
            notes: [0.0; NOTES],
            allowed: [false; NOTES],
            any_allowed: false,
            frame: 0,
            ring_len: 0,
            grain_ring_len: 0,
            sides: Default::default(),
            count: 0,
            write: 0,
            grain_write: 0,
            restart: false,
            grain: Vec::new(),
        };
        pitcher.size();
        for (index, value) in defaults().into_iter().enumerate() {
            pitcher.apply(index, value);
        }
        pitcher.find_allowed();
        pitcher
    }
}

impl Pitcher {
    /// Size the buffers for the rate, with room for the lowest pitch of
    /// all, and start over (`FUN_18001d0b0`, then `FUN_18001d420` with 0).
    fn size(&mut self) {
        let most = Self::frame_for(self.rate, LOWEST_HZ[5]).max(4);
        let grain_ring = (f64::from(self.rate) * 0.1) as i32;
        self.grain_ring_len = grain_ring.max(1) as usize;
        self.grain = vec![0.0; (2 * most + 2).max(PLAIN_GRAIN)];
        for side in &mut self.sides {
            side.history = vec![0.0; most];
            side.sums = vec![0.0; most];
            side.ring = vec![0.0; 2 * most];
            side.input = vec![0.0; self.grain_ring_len];
            side.marks = vec![0.0; self.grain_ring_len];
            side.out = vec![0.0; self.grain_ring_len];
        }
        self.reframe();
    }

    /// The detector's frame at a rate and a lowest pitch: the samples of
    /// the lowest period, down to a multiple of 4.
    fn frame_for(rate: f32, lowest: f32) -> usize {
        let samples = (f64::from(rate) / f64::from(lowest)) as u64;
        ((samples >> 2) * 4) as usize
    }

    /// Take the frame the lowest pitch asks for and start over.
    fn reframe(&mut self) {
        self.frame = Self::frame_for(self.rate, self.lowest).max(4);
        self.ring_len = 2 * self.frame;
        self.clear();
    }

    /// What `FUN_18001d420` does with a rate of 0: everything the blocks
    /// left behind goes.
    fn clear(&mut self) {
        for side in &mut self.sides {
            side.dc = 0.0;
            side.stages = [0.0; STAGES];
            side.history.fill(0.0);
            side.history_at = 0;
            side.sums.fill(0.0);
            side.freq = 0.0;
            side.period = 0.0;
            side.class = -1.0;
            side.octave = -1;
            side.target_note = -1;
            side.target = 0.0;
            side.correction = 0.0;
        }
        self.count = 0;
        self.restart_shifters();
    }

    /// Start both shifters over (`FUN_18001d290`).
    fn restart_shifters(&mut self) {
        let start = self.ring_len as f32 - START_BEHIND;
        for side in &mut self.sides {
            side.ring.fill(0.0);
            side.read = [start; VOICES];
            side.fade = [0; VOICES];
            side.level = [0.0; VOICES];
            side.input.fill(0.0);
            side.marks.fill(0.0);
            side.out.fill(0.0);
            side.countdown = [0; VOICES];
        }
        self.write = 0;
        self.grain_write = 0;
        self.restart = false;
    }

    /// What `setParameter` (`FUN_18001cab0`) does with a value.
    fn apply(&mut self, index: usize, value: f32) {
        let Some(param) = self.params.get_mut(index) else {
            return;
        };
        *param = value;
        match index {
            SPEED => {
                let slow = 1.0 - value;
                self.speed = if f64::from(slow) >= 0.01 {
                    (8.82f32 as f64
                        / (f64::from(self.rate) * f64::from(slow * slow * slow * 3.0 + 0.001)))
                        as f32
                } else {
                    1.0
                };
            }
            GENDER => self.gender = 2f32.powf(value + value - 1.0),
            FINE => self.fine = value + value - 1.0,
            BYPASS_ALL => {
                self.bypass_all = value != 0.0;
                self.find_allowed();
            }
            KEY_BYPASS..VOICE_PAN => {
                self.key_bypass[index - KEY_BYPASS] = value as i32 as i8 != 0;
                self.find_allowed();
            }
            FORMANT => {
                self.formant = value != 0.0;
                self.restart = true;
            }
            A_FREQ => {
                self.a_freq = 2f32.powf(value * (1.0f32 / 6.0) - 1.0 / 12.0) * 55.0;
            }
            REFERENCE => self.reference = (value + value) as i32,
            MIDI => self.midi = value != 0.0,
            OCTAVES => self.octaves = value != 0.0,
            HARMONIZE => self.harmonize = value != 0.0,
            LOWEST => {
                // A value outside the six leaves the lowest pitch as it
                // is. FL's would set 0 Hz and fail.
                if let Some(&hz) = LOWEST_HZ.get(value as i32 as usize)
                    && hz != self.lowest
                {
                    self.lowest = hz;
                    self.reframe();
                }
            }
            PORT => self.port = !((value * -256.0) as i32),
            _ => {}
        }
    }

    /// Mark the notes correction may go to (`FUN_18001d9c0`): every octave
    /// of the scale, or of the held notes in MIDI mode, or the held notes
    /// themselves an octave down when their octaves count.
    fn find_allowed(&mut self) {
        self.allowed = [false; NOTES];
        if !self.midi || !self.octaves {
            let mut classes = self.scale;
            if self.midi {
                classes = [false; 12];
                for (note, velocity) in self.notes.iter().enumerate() {
                    if *velocity != 0.0 {
                        classes[note % 12] = true;
                    }
                }
            }
            for (note, allowed) in self.allowed.iter_mut().enumerate() {
                *allowed = classes[note % 12];
            }
        } else {
            for note in 13..NOTES {
                if self.notes[note] != 0.0 {
                    self.allowed[note - 12] = true;
                }
            }
        }
        self.any_allowed = self.allowed.iter().any(|a| *a);
    }

    /// A note comes in on the plugin's MIDI port, with its velocity from 0
    /// to 127; 0 lets it go. FL hands the plugin the notes of a MIDI Out
    /// channel whose port is [`Pitcher::midi_port`].
    pub fn note(&mut self, note: u8, velocity: u8) {
        if let Some(held) = self.notes.get_mut(usize::from(note)) {
            *held = f32::from(velocity) * (1.0 / 127.0);
            self.find_allowed();
        }
    }

    /// The MIDI port the plugin listens on, or none.
    pub fn midi_port(&self) -> Option<u8> {
        u8::try_from(self.port).ok()
    }

    /// The harmony voices for the block (`FUN_18001f800`): for each side
    /// and voice a shift in semitones and a level, and whether the input
    /// plays beside them. None when harmonize is off.
    fn harmony(&self) -> Option<Harmony> {
        if !self.harmonize {
            return None;
        }
        let with_input = self.params[WITH_INPUT] != 0.0;
        let by_velocity = self.params[BY_VELOCITY] != 0.0;
        let stereo = self.params[STEREO];
        let mut shifts = [[0.0; VOICES]; 2];
        let mut levels = [[0.0; VOICES]; 2];
        if !with_input {
            levels[0][0] = 1.0;
            levels[1][0] = 1.0;
        }
        let mut slots = [-1i32; VOICES];
        let mut weights = [0.0f32; VOICES];
        if self.midi {
            let mut found = 0;
            for (note, velocity) in self.notes.iter().enumerate() {
                if *velocity == 0.0 {
                    continue;
                }
                let slot = if !self.octaves {
                    let class = (note % 12) as i32;
                    if slots[..3].contains(&class) {
                        continue;
                    }
                    weights[found] = velocity * velocity + velocity * velocity;
                    class
                } else {
                    note as i32 - 12
                };
                slots[found] = slot;
                found += 1;
                if found == VOICES {
                    break;
                }
            }
        }
        for (side, state) in self.sides.iter().enumerate() {
            let mut from = state.class;
            if self.octaves {
                from = (f64::from(state.octave) * 12.0 + f64::from(from)) as f32;
            }
            for voice in 0..VOICES {
                let slot = slots[voice];
                shifts[side][voice] = 0.0;
                if slot as f32 >= 0.0 && state.freq != 0.0 {
                    let spread = (self.params[VOICE_PAN + voice] - 0.5) * stereo;
                    let mut level = if side == 0 {
                        0.5 - spread
                    } else {
                        spread + 0.5
                    };
                    level += level;
                    level = level.clamp(0.0, 1.0);
                    if by_velocity {
                        level *= weights[voice];
                    }
                    levels[side][voice] = level;
                    shifts[side][voice] = slot as f32 - from;
                }
            }
        }
        Some((shifts, levels, with_input))
    }

    /// One frame of the detector on one side (`FUN_18001e030`'s inner
    /// loop): the band pass, the differences, and the correction moving.
    fn detect(&mut self, side: usize, x: f32) {
        let speed = self.speed;
        let grain_write = self.grain_write;
        let state = &mut self.sides[side];
        let dc = (x - state.dc) * DC_POLE + state.dc;
        state.dc = if dc < TINY && dc > -TINY { 0.0 } else { dc };
        let mut y = x - dc;
        for stage in &mut state.stages {
            let next = (f64::from(y) - *stage) * STAGE_POLE + *stage;
            *stage = if next < f64::from(TINY) && next > -f64::from(TINY) {
                0.0
            } else {
                next
            };
            y = next as f32;
        }
        state.marks[grain_write] = y;
        let frame = self.frame;
        let at = state.history_at;
        let (newer, older) = state.history[..frame].split_at(at);
        let (sums_old, sums_new) = state.sums[..frame].split_at_mut(frame - at);
        for (sum, past) in sums_old.iter_mut().zip(older) {
            *sum += (past - y).abs();
        }
        for (sum, past) in sums_new.iter_mut().zip(newer) {
            *sum += (past - y).abs();
        }
        state.history[at] = y;
        state.history_at = if at + 1 == frame { 0 } else { at + 1 };
        state.correction = if state.freq == 0.0 {
            0.0
        } else {
            (state.target - state.correction) * speed + state.correction
        };
    }

    /// Find the period of the frame on each side (`FUN_18001e810`).
    fn find_periods(&mut self) {
        let frame = self.frame;
        for state in &mut self.sides {
            let sums = &mut state.sums[..frame];
            let most = sums.iter().fold(0.0f32, |most, v| most.max(*v));
            state.period = 0.0;
            state.freq = 0.0;
            if most == 0.0 {
                continue;
            }
            let scale = 1.0 / most;
            for sum in sums.iter_mut() {
                *sum *= scale;
            }
            let dip = (1..frame.saturating_sub(1))
                .rev()
                .find(|&k| sums[k] < sums[k - 1] && sums[k] < sums[k + 1] && sums[k] < DIP);
            if let Some(k) = dip {
                state.period = frame as f32 - k as f32;
                state.freq = (f64::from(self.rate) / f64::from(state.period)) as f32;
            }
            if state.freq < self.lowest || HIGHEST_HZ < state.freq {
                state.period = 0.0;
                state.freq = 0.0;
            }
        }
    }

    /// The note each side goes to and the correction that takes it there
    /// (`FUN_18001ea80`).
    fn find_targets(&mut self) {
        for state in &mut self.sides {
            if state.freq == 0.0 || !self.any_allowed {
                state.class = -1.0;
                state.octave = -1;
                state.target_note = -1;
                state.correction = 0.0;
                state.target = 0.0;
                continue;
            }
            let note = 9.0 - (self.a_freq / state.freq).log2() * 12.0;
            let octave = (note * (1.0 / 12.0)) as i32;
            state.octave = octave;
            let nearest = ((note + 0.5).abs() as i32 as f32) as i32;
            state.class = note - octave as f32 * 12.0;
            let at = nearest.clamp(0, NOTES as i32 - 1) as usize;
            let (mut down, mut up) = (at, at);
            if !self.allowed[at] {
                while down >= 1 {
                    down -= 1;
                    if self.allowed[down] {
                        break;
                    }
                }
                while up <= NOTES - 2 {
                    up += 1;
                    if self.allowed[up] {
                        break;
                    }
                }
            }
            let (down, up) = (down as i32, up as i32);
            let goal = if note < (up - down) as f32 * 0.5 + down as f32 {
                down
            } else {
                up
            };
            state.target_note = goal;
            state.target = goal as f32 - note;
            if !self.midi {
                // A class past B reads key C; one below C reads the scale
                // byte before the keys, as FL's does.
                let key = (state.class + 0.5) as i32;
                let off = match key {
                    0..12 => self.key_bypass[key as usize],
                    -12..0 => self.scale[(12 + key) as usize],
                    k if k >= 12 => self.key_bypass[0],
                    _ => false,
                };
                if off || self.bypass_all {
                    state.target = 0.0;
                }
            }
        }
    }

    /// The delay-line shifter for one frame (`FUN_18001ed60`).
    fn shift(
        &mut self,
        input: [f32; 2],
        shifts: &[[f32; VOICES]; 2],
        levels: &[[f32; VOICES]; 2],
    ) -> [f32; 2] {
        match self.reference {
            0 => self.sides[1].read = self.sides[0].read,
            2 => self.sides[0].read = self.sides[1].read,
            _ => {}
        }
        let len = self.ring_len as i32;
        let lenf = len as f32;
        let write = self.write as f32;
        let mut out = [0.0; 2];
        for (side, state) in self.sides.iter_mut().enumerate() {
            let period = state.period;
            for voice in 0..VOICES {
                let rate = if period != 0.0 {
                    2f32.powf(shifts[side][voice].clamp(-12.0, 12.0) * (1.0f32 / 12.0))
                } else {
                    1.0
                };
                let mut pos = state.read[voice];
                let mut y = interpolate(&state.ring[..self.ring_len], pos);
                if rate < 1.0 {
                    let mut ahead = pos - write;
                    if ahead < 0.0 {
                        ahead += lenf;
                    }
                    if ahead <= FADE as f32 {
                        state.old[voice] = pos;
                        pos += period;
                        state.fade[voice] = FADE;
                        state.read[voice] = pos;
                    }
                } else if rate > 1.0 {
                    let mut ahead = pos - write;
                    if ahead < 0.0 {
                        ahead += lenf;
                    }
                    if ahead >= (len - FADE) as f32 {
                        state.old[voice] = pos;
                        pos -= period;
                        state.fade[voice] = FADE;
                        if pos < 0.0 {
                            pos += lenf;
                        }
                        state.read[voice] = pos;
                    }
                }
                let fade = state.fade[voice];
                if fade > 0 {
                    let old = state.old[voice];
                    let y2 = interpolate(&state.ring[..self.ring_len], old);
                    state.fade[voice] = fade - 1;
                    let g = fade as f32 * (1.0 / FADE as f32);
                    state.old[voice] = old + rate;
                    y = (1.0 - g) * y + g * y2;
                }
                while pos >= lenf {
                    pos -= lenf;
                }
                while pos < 0.0 {
                    pos += lenf;
                }
                state.read[voice] = pos + rate;
                let level =
                    (levels[side][voice] - state.level[voice]) * LEVEL_POLE + state.level[voice];
                state.level[voice] = level;
                out[side] += level * y;
            }
            state.ring[self.write] = input[side];
        }
        self.write += 1;
        if self.write == self.ring_len {
            self.write = 0;
        }
        out
    }

    /// The formant-keeping shifter for one frame (`FUN_18001f2b0`): grains
    /// of two periods around each pitch mark, laid down a shifted period
    /// apart and squeezed by the gender.
    fn shift_formant(
        &mut self,
        input: [f32; 2],
        shifts: &[[f32; VOICES]; 2],
        levels: &[[f32; VOICES]; 2],
    ) -> [f32; 2] {
        match self.reference {
            0 => self.sides[1].countdown = self.sides[0].countdown,
            2 => self.sides[0].countdown = self.sides[1].countdown,
            _ => {}
        }
        let len = self.grain_ring_len as i32;
        let write = self.grain_write as i32;
        let gender = self.gender;
        let mut out = [0.0; 2];
        for (side, state) in self.sides.iter_mut().enumerate() {
            let period = state.period;
            for voice in 0..VOICES {
                let shift = shifts[side][voice];
                let rate = if period != 0.0 {
                    2f32.powf(shift * (-1.0f32 / 12.0))
                } else {
                    1.0
                };
                state.countdown[voice] -= 1;
                if state.countdown[voice] < 1 {
                    let level = state.level[voice];
                    if period == 0.0 || shift.abs() <= 0.0 {
                        let mut at = write - (PLAIN_GRAIN as i32 + 1);
                        if at < 0 {
                            at += len;
                        }
                        for i in 0..PLAIN_GRAIN {
                            let window = hann(i as f32 * (1.0 / PLAIN_GRAIN as f32));
                            self.grain[i] = window * state.input[at as usize];
                            at = if at + 1 < len { at + 1 } else { 0 };
                        }
                        lay(
                            &mut state.out,
                            write,
                            &self.grain[..PLAIN_GRAIN],
                            0.0,
                            level,
                        );
                        state.countdown[voice] = PLAIN_HOP;
                    } else {
                        let size = (period + period) as i32;
                        let half = size / 2;
                        let reach = (f64::from(period) * MARK_REACH) as i32;
                        let mut best = 0.0f32;
                        let mut mark = -1;
                        let wrap = |i: i32| {
                            if i < 0 {
                                i + len
                            } else if i >= len {
                                i - len
                            } else {
                                i
                            }
                        };
                        for lag in (half + 1..).take(reach.max(0) as usize) {
                            let i = wrap(write - lag);
                            let v = state.marks[i as usize];
                            if v > best
                                && v > state.marks[wrap(i - 1) as usize]
                                && v > state.marks[wrap(i + 1) as usize]
                            {
                                best = v;
                                mark = lag;
                            }
                        }
                        if mark > -1 {
                            let mut at = write - half - mark - MARK_LEAD;
                            if at < 0 {
                                at += len;
                            }
                            let step = 1.0 / size as f32;
                            for i in 0..size.max(0) as usize {
                                let window = hann(i as f32 * step);
                                self.grain[i] = window * state.input[at as usize];
                                at = if at + 1 < len { at + 1 } else { at + 1 - len };
                            }
                            lay(
                                &mut state.out,
                                write,
                                &self.grain[..size.max(0) as usize],
                                gender,
                                level,
                            );
                        }
                        state.countdown[voice] = (rate * period) as i32;
                    }
                }
                state.level[voice] =
                    (levels[side][voice] - state.level[voice]) * LEVEL_POLE + state.level[voice];
            }
            let at = write as usize;
            state.input[at] = input[side];
            out[side] += state.out[at];
            state.out[at] = 0.0;
        }
        self.grain_write += 1;
        if self.grain_write == self.grain_ring_len {
            self.grain_write = 0;
        }
        out
    }
}

/// The window of a grain, `(1 - cos(2 pi t)) / 2`, worked as FL's does.
fn hann(t: f32) -> f32 {
    ((1.0 - (f64::from(t) * std::f64::consts::TAU).cos()) * 0.5) as f32
}

/// Add a grain into the output ring from `at` on, scaled by `level`; a
/// `step` other than 0 reads it that fast with a straight line between
/// samples (`FUN_18001f180`).
fn lay(out: &mut [f32], at: i32, grain: &[f32], step: f32, level: f32) {
    let len = out.len() as i32;
    let mut at = at;
    let next = |at: &mut i32| {
        let i = if *at < len { *at } else { *at - len };
        *at = i + 1;
        i as usize
    };
    if step == 0.0 {
        for v in grain {
            let i = next(&mut at);
            out[i] += level * v;
        }
        return;
    }
    let size = grain.len() as i32;
    let mut pos = 0.0f32;
    while pos < size as f32 {
        let whole = pos as i32;
        let frac = pos - whole as f32;
        let mut v = (1.0 - frac) * grain[whole as usize];
        if whole < size - 1 {
            v += frac * grain[whole as usize + 1];
        }
        pos += step;
        let i = next(&mut at);
        out[i] += v * level;
    }
}

/// The ring at a place between its samples, by a cubic through four of
/// them (`FUN_18001ec70`). The curve's first point is the sample before
/// the place, so it reads one sample later than the place says.
fn interpolate(ring: &[f32], pos: f32) -> f32 {
    if pos < 0.0 {
        return 0.0;
    }
    let len = ring.len() as i32;
    let wrap = |i: i32| if i < len { i } else { i - len };
    let whole = pos as i32;
    let i0 = wrap(whole);
    let i1 = wrap(i0 + 1);
    let i2 = wrap(i1 + 1);
    let i3 = wrap(i2 + 1);
    let t = pos - whole as f32;
    let (y0, y1, y2, y3) = (
        ring[i0 as usize],
        ring[i1 as usize],
        ring[i2 as usize],
        ring[i3 as usize],
    );
    let c1 = (y2 - y0) * 0.5;
    let c3 = (y3 - y0) * 0.5 + (y1 - y2) * 1.5;
    let c2 = y0 - y1 + c1 - c3;
    ((c3 * t + c2) * t + c1) * t + y1
}

impl Effect for Pitcher {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let word = |at: usize| {
            bytes
                .get(at..at + 4)
                .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
        };
        let expected = 8 + CHUNK;
        let too_short = StateError::Length {
            expected,
            found: bytes.len(),
        };
        let (Some(wrapper), Some(len)) = (word(0), word(4)) else {
            return Err(too_short);
        };
        if wrapper != WRAPPER {
            return Err(StateError::Version {
                found: wrapper as i32,
            });
        }
        let len = len as usize;
        if len < 4 + 4 * 33 || bytes.len() < 8 + len {
            return Err(too_short);
        }
        let magic = word(8).unwrap_or(0);
        if magic != REGISTERED && magic != DEMO {
            return Err(StateError::Version {
                found: magic as i32,
            });
        }
        for index in 0..33 {
            let value = f32::from_bits(word(12 + 4 * index).unwrap());
            self.apply(index, value);
        }
        if len >= 4 + 4 * 33 + 12 {
            for (class, byte) in bytes[SCALE_AT..SCALE_AT + 12].iter().enumerate() {
                self.scale[class] = *byte != 0;
            }
            if len >= CHUNK {
                self.apply(PORT, f32::from_bits(word(SCALE_AT + 12).unwrap()));
            }
        }
        self.find_allowed();
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        if hz > 0.0 && hz != self.rate {
            self.rate = hz;
            self.size();
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if self.restart {
            self.restart_shifters();
        }
        let harmony = self.harmony();
        let level_one = [[1.0, 0.0, 0.0, 0.0]; 2];
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let input = [*l, *r];
            let (shifts, levels, with_input) = match &harmony {
                Some((shifts, levels, with_input)) => (*shifts, *levels, *with_input),
                None => {
                    let (from_left, from_right) = match self.reference {
                        1 => (0, 1),
                        2 => (1, 1),
                        _ => (0, 0),
                    };
                    let mut shifts = [[0.0; VOICES]; 2];
                    shifts[0][0] = self.fine + self.sides[from_left].correction;
                    shifts[1][0] = self.fine + self.sides[from_right].correction;
                    (shifts, level_one, false)
                }
            };
            let mut out = if self.formant {
                self.shift_formant(input, &shifts, &levels)
            } else {
                self.shift(input, &shifts, &levels)
            };
            if with_input {
                out[0] += input[0];
                out[1] += input[1];
            }
            *l = out[0];
            *r = out[1];
            self.detect(0, input[0]);
            self.detect(1, input[1]);
            self.count += 1;
            if self.count == self.frame {
                self.find_periods();
                self.find_targets();
                for side in &mut self.sides {
                    side.sums.fill(0.0);
                }
                self.count = 0;
            }
        }
    }

    fn param_count(&self) -> usize {
        PARAMS
    }

    /// FL's wrapper shows a parameter as its value times 256.
    fn param(&self, index: usize) -> f32 {
        self.params
            .get(index)
            .map_or(0.0, |value| (value * 256.0) as i32 as f32)
    }

    /// FL's wrapper takes a value in 256ths and keeps it from 0 to 1.
    fn set_param(&mut self, index: usize, value: f32) {
        if index >= PARAMS {
            return;
        }
        let value = ((f64::from(value as i32) * (1.0 / 256.0)) as f32).clamp(0.0, 1.0);
        if value != self.params[index] {
            self.apply(index, value);
        }
    }

    fn reset(&mut self) {
        self.clear();
    }

    fn fidelity(&self) -> Fidelity {
        if self.harmonize && self.midi {
            Fidelity::Approximate("Pitcher's harmony voices follow MIDI notes no host sends")
        } else if self.midi {
            Fidelity::Approximate("Pitcher's MIDI mode corrects to notes no host sends")
        } else {
            Fidelity::Exact
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{bits, partial, sine};

    /// A rate at which 440 Hz is a whole 100 samples.
    const RATE: f32 = 44_000.0;
    const A: f64 = 440.0 / 44_000.0;

    /// A state as FL writes it, with the defaults but for `moves`.
    fn saved(moves: &[(usize, f32)], scale: [bool; 12]) -> Vec<u8> {
        let mut params = defaults();
        params[PORT] = 0.0;
        for (index, value) in moves {
            params[*index] = *value;
        }
        let mut bytes = b"SSLF".to_vec();
        bytes.extend(152u32.to_le_bytes());
        bytes.extend(b"1RTP");
        for value in &params[..33] {
            bytes.extend(value.to_le_bytes());
        }
        bytes.extend(scale.map(u8::from));
        bytes.extend(params[PORT].to_le_bytes());
        bytes
    }

    fn with(moves: &[(usize, f32)], scale: [bool; 12]) -> Pitcher {
        let mut plugin = Pitcher::default();
        plugin.set_sample_rate(RATE);
        plugin.set_state(&saved(moves, scale)).unwrap();
        plugin
    }

    /// The same signal on both sides through the plugin, in blocks of 512.
    fn run(plugin: &mut Pitcher, input: &[f32]) -> Vec<f32> {
        let mut left = input.to_vec();
        let mut right = input.to_vec();
        for (l, r) in left.chunks_mut(512).zip(right.chunks_mut(512)) {
            plugin.process(l, r);
        }
        assert_eq!(bits(&left), bits(&right));
        left
    }

    /// The pitch classes of C major.
    const C_MAJOR: [bool; 12] = [
        true, false, true, false, true, true, false, true, false, true, false, true,
    ];

    /// The pitch of the last two seconds of `out`, in Hz, from the time
    /// between its first and its last rising zero crossing there.
    fn pitch(out: &[f32]) -> f64 {
        let tail = &out[out.len() - 2 * RATE as usize..];
        let mut crossings = Vec::new();
        for (t, pair) in tail.windows(2).enumerate() {
            if pair[0] < 0.0 && pair[1] >= 0.0 {
                let frac = f64::from(-pair[0]) / f64::from(pair[1] - pair[0]);
                crossings.push(t as f64 + frac);
            }
        }
        let span = crossings.last().unwrap() - crossings[0];
        (crossings.len() - 1) as f64 / span * f64::from(RATE)
    }

    /// Cents from `b` to `a`.
    fn cents(a: f64, b: f64) -> f64 {
        1200.0 * (a / b).log2()
    }

    /// A tone of three seconds at `hz`, at half scale.
    fn tone(hz: f64) -> Vec<f32> {
        let turns = hz / f64::from(RATE);
        sine(turns, 3 * RATE as usize)
            .iter()
            .map(|v| v * 0.5)
            .collect()
    }

    // The detector measures a period in whole samples, so near 440 Hz at
    // 44 kHz it can be 0.5 % out, 8 cents. The corrected pitch is as far
    // from the note.

    #[test]
    fn a_sine_30_cents_sharp_comes_out_on_the_a() {
        let sharp = 440.0 * 2f64.powf(0.3 / 12.0);
        let mut plugin = with(&[], C_MAJOR);
        let out = run(&mut plugin, &tone(sharp));
        assert!(out.iter().all(|v| v.is_finite()));
        let side = &plugin.sides[0];
        assert_eq!(side.target_note, 45);
        // The detector saw 44000 / 98 Hz, and the shift takes that to 440.
        assert_eq!(side.period, 98.0);
        let seen = f64::from(RATE) / 98.0;
        let shifted = sharp * 2f64.powf(-cents(seen, 440.0) / 1200.0);
        let heard = pitch(&out);
        assert!(
            cents(heard, shifted).abs() < 1.0,
            "{heard} Hz, not {shifted}"
        );
        assert!(cents(heard, 440.0).abs() < 8.0, "{heard} Hz");
        let peak = out.iter().fold(0.0f32, |p, v| p.max(v.abs()));
        assert!((0.4..0.6).contains(&peak), "{peak}");
    }

    /// The pitch the formant shifter lays its grains at: one grain every
    /// `rate * period` samples, cut to a whole number.
    fn hop_pitch(plugin: &Pitcher) -> f64 {
        let side = &plugin.sides[0];
        let rate = 2f32.powf((plugin.fine + side.correction) * (-1.0 / 12.0));
        f64::from(RATE) / f64::from((rate * side.period) as i32)
    }

    #[test]
    fn the_formant_shifter_corrects_too() {
        let flat = 440.0 * 2f64.powf(-0.35 / 12.0);
        let mut plugin = with(&[(FORMANT, 1.0)], C_MAJOR);
        let out = run(&mut plugin, &tone(flat));
        assert!(out.iter().all(|v| v.is_finite()));
        // 431.4 Hz is seen as a period of 102, and the hop of 99.99 is cut
        // to 99: 444.4 Hz, 17 cents sharp.
        assert_eq!(plugin.sides[0].period, 102.0);
        let heard = pitch(&out);
        assert!(cents(heard, hop_pitch(&plugin)).abs() < 1.0, "{heard} Hz");
        assert!(cents(heard, 440.0).abs() < 20.0, "{heard} Hz");
        let peak = out.iter().fold(0.0f32, |p, v| p.max(v.abs()));
        assert!((0.3..0.8).contains(&peak), "{peak}");
    }

    #[test]
    fn gender_moves_the_formants_and_not_the_pitch() {
        let mut plugin = with(&[(FORMANT, 1.0), (GENDER, 0.8)], C_MAJOR);
        let out = run(&mut plugin, &tone(440.0 * 2f64.powf(0.2 / 12.0)));
        assert!(out.iter().all(|v| v.is_finite()));
        // The grains are squeezed by 2^0.6, so the tone in each is higher
        // and a count of zero crossings does not find the pitch. The
        // output still repeats at the hop.
        let hop = (f64::from(RATE) / hop_pitch(&plugin)).round() as usize;
        assert!(cents(hop_pitch(&plugin), 440.0).abs() < 20.0);
        let tail = &out[out.len() - RATE as usize..];
        let like = |lag: usize| {
            let (mut both, mut power) = (0.0, 0.0);
            for t in lag..tail.len() {
                both += f64::from(tail[t]) * f64::from(tail[t - lag]);
                power += f64::from(tail[t]) * f64::from(tail[t]);
            }
            both / power
        };
        assert!(like(hop) > 0.95, "{}", like(hop));
        assert!(like(hop * 3 / 4) < 0.5, "{}", like(hop * 3 / 4));
    }

    #[test]
    fn a_note_between_two_of_the_scale_goes_to_the_nearer() {
        // A# is not in C major. A little under it the A is nearer, a little
        // over it the B.
        for (cents, to) in [(-30.0, 0.0), (30.0, 2.0)] {
            let from = A * 2f64.powf((100.0 + cents) / 1200.0);
            let input = sine(from, 3 * RATE as usize);
            let mut plugin = with(&[], C_MAJOR);
            run(&mut plugin, &input);
            let side = &plugin.sides[0];
            assert_eq!(side.target_note, 45 + to as i32);
            let want = (to - (1.0 + cents / 100.0)) as f32;
            assert!((side.correction - want).abs() < 0.1, "{}", side.correction);
        }
    }

    #[test]
    fn with_everything_bypassed_the_input_comes_out_nine_samples_late() {
        let input: Vec<f32> = sine(A * 1.03, 20_000).iter().map(|v| v * 0.5).collect();
        let out = run(&mut with(&[(BYPASS_ALL, 1.0)], C_MAJOR), &input);
        // Once the voice's level has settled, which it does a few
        // millionths under 1: a step of 0.007 of the rest is lost in the
        // rounding from there.
        for t in 12_000..20_000 {
            assert!(
                (out[t] - input[t - 9]).abs() <= 5e-6 * input[t - 9].abs(),
                "{t}"
            );
        }
    }

    #[test]
    fn held_notes_are_the_scale_in_midi_mode() {
        let sharp = A * 2f64.powf(0.4 / 12.0);
        let input = sine(sharp, 3 * RATE as usize);
        let mut plugin = with(&[(MIDI, 1.0)], C_MAJOR);
        // No note held: nothing is corrected.
        run(&mut plugin, &input);
        assert_eq!(plugin.sides[0].target, 0.0);
        assert_ne!(plugin.fidelity(), Fidelity::Exact);
        // An E held: the A goes to the nearest E, five semitones down.
        plugin.note(64, 100);
        let out = run(&mut plugin, &input);
        assert_eq!(plugin.sides[0].target_note, 40);
        let e = 440.0 * 2f64.powf(-5.0 / 12.0);
        assert!(cents(pitch(&out), e).abs() < 8.0, "{}", pitch(&out));
        plugin.note(64, 0);
        assert!(!plugin.any_allowed);
    }

    #[test]
    fn a_harmony_voice_plays_the_held_note() {
        let input = sine(A, 3 * RATE as usize);
        let mut plugin = with(&[(MIDI, 1.0), (HARMONIZE, 1.0)], C_MAJOR);
        plugin.note(60, 127);
        let out = run(&mut plugin, &input);
        // The C is nine semitones under the A, in the A's octave.
        let c = A * 2f64.powf(-9.0 / 12.0);
        let len = (2.0 * f64::from(RATE)) as usize;
        let window = &out[out.len() - len..];
        // 2 s of a pitch that is not a whole number of samples; the
        // partial is still near its size.
        assert!(partial(window, c) > 0.5, "{}", partial(window, c));
        assert!(partial(window, A) < 0.1);
    }

    #[test]
    fn the_state_s_floats_are_the_parameters() {
        let plugin = with(&[(SPEED, 0.25), (LOWEST, 3.0)], C_MAJOR);
        assert_eq!(plugin.param(SPEED), 64.0);
        assert_eq!(plugin.param(LOWEST), 768.0);
        assert_eq!(plugin.lowest, 80.0);
        assert_eq!(plugin.frame, 548);
        assert_eq!(plugin.scale, C_MAJOR);
        let mut short = saved(&[], C_MAJOR);
        short.truncate(100);
        assert!(Pitcher::default().set_state(&short).is_err());
    }

    #[test]
    fn every_state_in_fl_s_files_loads_and_plays() {
        let Some(root) = crate::testing::corpus() else {
            return;
        };
        let ours = |saved: &crate::testing::Saved| saved.plugin.eq_ignore_ascii_case(NAME);
        let mut saved = crate::testing::every_preset(&root, NAME);
        saved.extend(crate::testing::projects(&root).into_iter().filter(ours));
        assert_eq!(saved.len(), 65);
        let input: Vec<f32> = sine(220.0 * 1.02 / 44_100.0, 44_100)
            .iter()
            .map(|v| v * 0.5)
            .collect();
        for saved in saved {
            let mut plugin = crate::by_name(&saved.plugin).unwrap();
            plugin.set_sample_rate(44_100.0);
            plugin
                .set_state(&saved.state)
                .unwrap_or_else(|e| panic!("{}: {e}", saved.file));
            for index in 0..33 {
                let at = 12 + 4 * index;
                let word = f32::from_le_bytes(saved.state[at..at + 4].try_into().unwrap());
                assert_eq!(plugin.param(index), (word * 256.0) as i32 as f32);
            }
            let (mut left, mut right) = (input.clone(), input.clone());
            plugin.process(&mut left, &mut right);
            let peak = left
                .iter()
                .chain(&right)
                .fold(0.0f32, |p, v| p.max(v.abs()));
            assert!(
                left.iter().chain(&right).all(|v| v.is_finite()),
                "{}",
                saved.file
            );
            assert!((0.2..1.0).contains(&peak), "{peak} in {}", saved.file);
        }
    }
}
