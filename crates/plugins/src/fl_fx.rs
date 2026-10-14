//! FL's own effect classes, as the Delphi plugins link them in: the chorus
//! `TStereoFlanger`, the delay `TDelay_32FS` and the reverb `TFruityReverb`.
//! Harmless, Harmor and Sytrus build their post chains from them, Fruity
//! Flangus is the flanger alone, and Fruity Reeverb 2 drives the same
//! reverb engine with modulation and early reflections on top.
//! `docs/plugins/fl-fx.md` maps them with addresses in Harmless.
//!
//! Each class here is the class alone. A plugin brings its own front end:
//! its tables or wheel maps, its dry and wet laws, its block size and when
//! it lets a class sleep. Nothing on the audio path allocates; the memory
//! is asked for by [`StereoFlanger::make_room`], [`Delay32::with_capacity`]
//! and [`FruityReverb::new`].
//!
//! # `StereoFlanger`
//!
//! Up to nine voices a side, each a tap on its side's ring that a parabolic
//! triangle LFO swings, linear between two samples, no feedback.
//!
//! - [`FlangerSettings`] holds the class fields +0x38..+0x50: the order,
//!   depth and speed (wheel / 1024), delay, spread and cross (wheel / 128),
//!   and an output gain that the class folds into its normalisation
//!   (Flangus 1, Harmless its 5^m wet, Harmor its signed wet).
//!   [`FlangerSettings::from_wheels`] makes one from the six wheels.
//! - [`StereoFlanger::set`] rebuilds the voices. It returns whether the
//!   ring started over: FL clears it, and puts every LFO back, when the
//!   longest delay or the spread changes.
//! - [`StereoFlanger::frame`] writes one frame in and gives the wet pair;
//!   the caller does dry and wet. [`StereoFlanger::skip`] moves the LFOs
//!   on without sound, for a front end that lets the class sleep.
//! - [`StereoFlanger::length`] is the longest delay plus 8: FL's ring is
//!   that plus the host block, which Harmless and Harmor add to their
//!   sleep counters.
//! - [`StereoFlanger::fresh`] is a new object, as FL makes when a chorus
//!   goes from off to on.
//!
//! # `Delay32`
//!
//! A stereo ring with a read and a write position `delay` frames apart.
//!
//! - [`Delay32::set_length`] is `FUN_00747070`: a ring of `extra + delay`
//!   frames, cleared, with the feedback ramp put on its target, when that
//!   size changes. `extra` is the host block plus any stereo offset.
//! - [`Delay32::add`] adds one side of a block at the write position plus
//!   an offset (`FUN_00747510`).
//! - [`Delay32::render`] runs one of the three kernels over a block: it
//!   reads and clears the ring, writes the output, and adds the feedback
//!   times what it read at the write position, flushing below 2^-24. The
//!   feedback goes to its target by [`crate::parts::glide`].
//! - [`Delay32::span`] is the ring from the read position to its end, for
//!   a front end that filters inside the loop (Harmor's damping).
//!
//! # `FruityReverb`
//!
//! The wrapper and the unmodulated process of `Shared/Reverb_x64.dll`
//! (`FUN_180002980`): Reeverb 2's network without its early reflections,
//! its modulation or its crossover wheel.
//!
//! - [`ReverbSettings`] holds the wrapper fields +0x38..+0x54: low cut in
//!   Hz, high cut and damping in 100 Hz, each 0 for off, predelay in
//!   samples, room size, diffusion in %, decay in samples and the bass
//!   colour index into {0.2, 0.6, 1, 1.5, 3}.
//! - [`FruityReverb::set`] takes them. Another predelay empties the
//!   reverb; anything else is worked out at the next process
//!   (`FUN_00749b60`), and another room size empties the twelve lines.
//! - [`FruityReverb::process`] takes a mono block (FL's front ends pass the
//!   sum of the two sides) and writes the stereo wet.

use crate::parts::glide;
use std::f64::consts::PI;

// ---------------------------------------------------------------------------
// TStereoFlanger

/// Two voices for every step of the order.
pub const FLANGER_VOICES: usize = 18;

/// What the spread tells the voices apart by: voice `i` has entry
/// `i mod 20`, squared.
const TABLE: [f32; 20] = [
    0.967, 1.039, 1.021, 0.1013, 0.977, 1.033, 0.997, 0.971, 1.061, 0.947, 1.031, 0.1009, 1.063,
    0.953, 0.991, 1.049, 0.983, 1.051, 1.019, 1.069,
];

/// The LFO depth in samples at 44.1 kHz for a depth of 1, and the delay
/// for a delay of 1: 100 ms.
const DEPTH: f32 = 1092.5;
const DELAY: f32 = 4410.0;

/// What every voice has under its depth, and the shortest delay, both in
/// samples.
const FLOOR_DEPTH: f32 = 10.0;
const SHORTEST: f32 = 8.0;

/// `(11^x - 1) * scale / 10`, the curve of the depth and the delay.
fn curve(x: f32, scale: f32) -> f32 {
    ((f64::from(x) * 11f64.ln()).exp() - 1.0) as f32 * (scale / 10.0)
}

/// The flanger's settings, the class fields +0x38..+0x50.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlangerSettings {
    /// Voices a side, 1 to 9.
    pub order: i32,
    /// Depth and speed, 0 to 1.
    pub depth: f32,
    pub speed: f32,
    /// Centre delay and spread, 0 to 1.
    pub delay: f32,
    pub spread: f32,
    /// How much of each voice goes into the other side, -1 to 1.
    pub cross: f32,
    /// The output gain the normalisation is multiplied by.
    pub gain: f32,
}

impl FlangerSettings {
    /// The settings of the six wheels order, depth, speed, delay, spread
    /// and cross, as Fruity Flangus and the Harmless and Harmor chorus
    /// tables have them.
    pub fn from_wheels(wheels: [i32; 6], gain: f32) -> Self {
        FlangerSettings {
            order: wheels[0],
            depth: wheels[1] as f32 / 1024.0,
            speed: wheels[2] as f32 / 1024.0,
            delay: wheels[3] as f32 / 128.0,
            spread: wheels[4] as f32 / 128.0,
            cross: wheels[5] as f32 / 128.0,
            gain,
        }
    }
}

/// One tap.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct FlangerVoice {
    /// How far the delay swings to either side of the centre, in samples.
    pub(crate) depth: f32,
    /// The gain into the left and into the right output.
    pub(crate) gains: [f32; 2],
    /// The middle of the swing, in samples.
    pub(crate) centre: f32,
    /// Where the LFO is. The whole range of an i32 is one cycle, and 0 is
    /// the shortest delay.
    pub(crate) phase: i32,
    /// Where a flush puts the LFO.
    pub(crate) start: i32,
    /// What a sample adds to the phase.
    pub(crate) step: i32,
    /// Whether it reads the right side.
    pub(crate) right: bool,
    /// `T = sqrt(4 * depth)`, the size of the triangle the curve is made
    /// from, with `T / 2^31` and `T / 2`.
    top: f32,
    slope: f32,
    half: f32,
}

impl FlangerVoice {
    /// The delay at the LFO's phase, in samples, and the phase moved on.
    pub(crate) fn delay(&mut self) -> f64 {
        let phase = self.phase;
        self.phase = phase.wrapping_add(self.step);
        let x = (f64::from(phase) * f64::from(self.slope)).abs() - f64::from(self.half);
        (f64::from(self.top) - x.abs()) * x + f64::from(self.centre)
    }
}

/// The voices for these settings at this rate (`FUN_0074a9b0`), and the
/// longest delay any of them reaches plus 8, in samples. The phases are
/// left alone.
fn build(
    s: &FlangerSettings,
    hz: f64,
    voices: &mut [FlangerVoice; FLANGER_VOICES],
) -> (usize, i32) {
    let order = s.order.clamp(1, 9);
    let count = 2 * order as usize;
    let depth = curve(s.depth, DEPTH);
    let cycle = ((f64::from(1.0 - s.speed) * 101f64.ln()).exp() - 1.0) as f32;
    let pace = 2_147_483_648.0 / (f64::from(cycle) * 75_000.0 + 50_000.0) * 500.0;
    let delay = curve(s.delay, DELAY);
    let apart = ((f64::from(s.spread) * 1001f64.ln()).exp() - 1.0) as f32 / 1000.0;
    // `a` with the spread all the way up, `b` with none.
    let mix = |a: f64, b: f64| a * f64::from(apart) + b * f64::from(1.0 - apart);
    let rate = (44_100.0 / hz) as f32;

    let (mut total, mut longest) = (0f32, 0f32);
    for (i, voice) in voices[..count].iter_mut().enumerate() {
        let right = i >= order as usize;
        let k = if right { count - 1 - i } else { i };
        let share = 1.0 - k as f32 / order as f32;
        let own = TABLE[i % TABLE.len()];
        let own = mix(f64::from(own * own), 1.0) as f32;
        let gains = [1.0, s.cross * share];
        voice.gains = if right { [gains[1], gains[0]] } else { gains };
        let [into_left, into_right] = voice.gains.map(|gain| f64::from(gain.abs()));
        total = (f64::from(total) + into_left + into_right) as f32;

        let swing = depth * share + FLOOR_DEPTH;
        let step = mix(
            pace * f64::from(own) / f64::from(swing),
            pace / f64::from(depth + FLOOR_DEPTH),
        );
        let centre = mix(f64::from(delay * share * own), f64::from(delay)) as f32;
        let offset = f64::from(share * apart * 2_147_483_648.0 / 256.0).round_ties_even();
        let start = if right { i32::MIN } else { 0 };
        voice.start = start.wrapping_add((offset as i64 as i32).wrapping_shl(8));
        voice.right = right;
        voice.step = (step * f64::from(rate)).round_ties_even() as i64 as i32;
        voice.depth = swing / rate;
        let centre = centre / rate;
        // Written as FL has it, so that a NaN would be the floor.
        voice.centre = voice.depth + if centre > SHORTEST { centre } else { SHORTEST };
        longest = longest.max(voice.centre + voice.depth);
        let top = f64::from(voice.depth * 4.0).sqrt();
        voice.slope = (top / 2_147_483_648.0) as f32;
        voice.top = top as f32;
        voice.half = (top / 2.0) as f32;
    }

    let over = ((f64::from(1.0 - 1.0 / total) * 1001f64.ln()).exp() - 1.0) as f32;
    let norm = (1.0 - over / 1000.0) * s.gain;
    for voice in &mut voices[..count] {
        voice.gains = voice.gains.map(|gain| gain * norm);
    }
    (
        count,
        f64::from(longest).round_ties_even() as i32 + SHORTEST as i32,
    )
}

/// How long a ring holds the longest delay the settings can ask for at
/// this rate, with the sample after it: a power of two.
pub(crate) fn flanger_room(hz: f64) -> usize {
    // Twice the deepest swing and the longest centre with the largest of
    // the table's numbers, 1.069 squared.
    let worst = f64::from(2.0 * (DEPTH + FLOOR_DEPTH) + DELAY * 1.143);
    ((worst * hz / 44_100.0).ceil() as usize + 16).next_power_of_two()
}

/// `TStereoFlanger`. A new one has no voices and no ring.
#[derive(Debug, Clone, PartialEq)]
pub struct StereoFlanger {
    pub(crate) voices: [FlangerVoice; FLANGER_VOICES],
    /// How many of the voices play: twice the order.
    pub(crate) count: usize,
    /// The longest delay plus 8 and the spread the ring was last cleared
    /// for. A change to either clears it again.
    pub(crate) longest: i32,
    spread: f32,
    /// What came in, one ring a side, and where the next frame goes. FL
    /// sizes its ring to the block and the longest delay of the moment;
    /// this one holds the longest the settings can reach, which reads the
    /// same.
    pub(crate) ring: [Vec<f32>; 2],
    at: usize,
}

impl Default for StereoFlanger {
    fn default() -> Self {
        StereoFlanger {
            voices: [FlangerVoice::default(); FLANGER_VOICES],
            count: 0,
            longest: -1,
            spread: -1.0,
            ring: [Vec::new(), Vec::new()],
            at: 0,
        }
    }
}

impl StereoFlanger {
    /// Have the rings for this rate. This allocates when the length
    /// changes, and then empties them.
    pub fn make_room(&mut self, hz: f64) {
        let len = flanger_room(hz);
        if self.ring[0].len() != len {
            for ring in &mut self.ring {
                ring.clear();
                ring.resize(len, 0.0);
            }
            self.at = 0;
        }
    }

    /// Rebuild the voices for these settings at this rate. A voice the
    /// order adds starts at phase 0, as FL's new memory does. Returns
    /// whether the ring started over.
    pub fn set(&mut self, settings: &FlangerSettings, hz: f64) -> bool {
        let was = self.count;
        let (count, longest) = build(settings, hz, &mut self.voices);
        for voice in self.voices.iter_mut().take(count).skip(was) {
            voice.phase = 0;
        }
        self.count = count;
        if longest != self.longest || settings.spread != self.spread {
            self.longest = longest;
            self.spread = settings.spread;
            self.flush();
            true
        } else {
            false
        }
    }

    /// Forget what came in and put every LFO where it starts
    /// (`FUN_0074b050`).
    pub fn flush(&mut self) {
        for ring in &mut self.ring {
            ring.fill(0.0);
        }
        for voice in &mut self.voices {
            voice.phase = voice.start;
        }
    }

    /// A flush that also puts the write position back at the start.
    pub fn reset(&mut self) {
        self.flush();
        self.at = 0;
    }

    /// A new object: no voices, nothing in the ring, and the next
    /// [`StereoFlanger::set`] flushes.
    pub fn fresh(&mut self) {
        self.voices = [FlangerVoice::default(); FLANGER_VOICES];
        self.count = 0;
        self.longest = -1;
        self.spread = -1.0;
        for ring in &mut self.ring {
            ring.fill(0.0);
        }
        self.at = 0;
    }

    /// The longest delay any voice reaches plus 8, in frames.
    pub fn length(&self) -> i32 {
        self.longest
    }

    /// Move the LFOs on by this many frames without reading the ring
    /// (`FUN_0074b0b0`).
    pub fn skip(&mut self, frames: usize) {
        for voice in &mut self.voices[..self.count] {
            let by = voice.step.wrapping_mul(frames as i32);
            voice.phase = voice.phase.wrapping_add(by);
        }
    }

    /// One frame in, and the voices' sum into each side out. This is the
    /// x87 renderer (`FUN_0074b2f0`, voices `FUN_0074b180` /
    /// `FUN_0074b230`), which truncates the delay to a whole sample; its
    /// sums are in f64 here where FL keeps 80 bits until the store.
    pub fn frame(&mut self, left: f32, right: f32) -> (f32, f32) {
        if self.ring[0].is_empty() {
            return (0.0, 0.0);
        }
        let mask = self.ring[0].len() - 1;
        let at = self.at;
        self.ring[0][at] = left;
        self.ring[1][at] = right;
        self.at = (at + 1) & mask;

        let (mut sum_l, mut sum_r) = (0f32, 0f32);
        for voice in &mut self.voices[..self.count] {
            let delay = voice.delay();
            let whole = delay as i64;
            let part = delay - whole as f64;
            let ring = &self.ring[usize::from(voice.right)];
            let back = at.wrapping_sub(whole as usize);
            let near = f64::from(ring[back & mask]);
            let far = f64::from(ring[back.wrapping_sub(1) & mask]);
            let tap = near + (far - near) * part;
            sum_l = (tap * f64::from(voice.gains[0]) + f64::from(sum_l)) as f32;
            sum_r = (tap * f64::from(voice.gains[1]) + f64::from(sum_r)) as f32;
        }
        (sum_l, sum_r)
    }
}

// ---------------------------------------------------------------------------
// TDelay_32FS

/// FL's "too small to keep": 2^-24.
const TINY: f32 = 5.960_464_5e-8;

/// What a delay kernel puts out and feeds back of the pair it reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelayKernel {
    /// Out as read, fed back as read (+0x40, `FUN_007475a0`).
    Straight,
    /// Out with the sides swapped, fed back as read (+0x50,
    /// `FUN_007476d0`).
    Swapped,
    /// Out swapped and fed back swapped: ping-pong (+0x60,
    /// `FUN_007477f0`).
    PingPong,
}

/// `TDelay_32FS`: a stereo ring, interleaved, with the read position
/// `delay` frames behind the write position.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Delay32 {
    ring: Vec<f32>,
    /// The frames in use (+0x28 / 8), and the two positions.
    size: usize,
    read: usize,
    write: usize,
    /// The feedback the ramp goes to (+0x30), and where it is (+0x34).
    feedback: f32,
    state: f32,
}

impl Delay32 {
    /// A new object with room for this many frames, which is all the ring
    /// can ever hold. This allocates.
    pub fn with_capacity(frames: usize) -> Self {
        let mut delay = Delay32 {
            ring: vec![0.0; 2 * frames],
            ..Delay32::default()
        };
        delay.fresh();
        delay
    }

    /// The most frames the ring can hold.
    pub fn capacity(&self) -> usize {
        self.ring.len() / 2
    }

    /// As FL's constructor leaves it: no ring yet, the feedback at 0.5.
    pub fn fresh(&mut self) {
        self.size = 0;
        self.read = 0;
        self.write = 0;
        self.feedback = 0.5;
        self.state = 0.0;
    }

    /// The feedback the next blocks ramp to.
    pub fn set_feedback(&mut self, feedback: f32) {
        self.feedback = feedback;
    }

    /// The feedback the ramp is aiming at.
    pub fn feedback(&self) -> f32 {
        self.feedback
    }

    /// The frames between writing and reading.
    pub fn delay(&self) -> usize {
        if self.size == 0 {
            0
        } else {
            (self.write + self.size - self.read) % self.size
        }
    }

    /// Size the ring to `extra + delay` frames, with the write position
    /// `delay` frames ahead of the read (`FUN_00747070`). The delay is at
    /// least 1, and at most what the capacity leaves after `extra`. Only
    /// a change of size does anything: then the ring is emptied and the
    /// feedback ramp put on its target. Returns whether it did.
    pub fn set_length(&mut self, extra: usize, delay: usize) -> bool {
        let Some(room) = self.capacity().checked_sub(extra) else {
            return false;
        };
        let delay = delay.max(1).min(room);
        if delay == 0 || extra + delay == self.size {
            return false;
        }
        self.size = extra + delay;
        self.ring[..2 * self.size].fill(0.0);
        self.state = self.feedback;
        self.read = 0;
        self.write = delay;
        true
    }

    /// Add one side (0 left, 1 right) of a block into the ring from the
    /// write position plus `offset` frames on (`FUN_00747510`).
    pub fn add(&mut self, side: usize, input: &[f32], offset: usize) {
        if self.size == 0 {
            return;
        }
        let mut at = (self.write + offset) % self.size;
        for x in input {
            let place = &mut self.ring[2 * at + side];
            *place += *x;
            at += 1;
            if at == self.size {
                at = 0;
            }
        }
    }

    /// The ring from the read position to its end, interleaved, which is
    /// what a kernel reads next without wrapping (Harmor's
    /// `FUN_007a5260`).
    pub fn span(&mut self) -> &mut [f32] {
        &mut self.ring[2 * self.read..2 * self.size]
    }

    /// Run a kernel over a block: read and clear the ring at the read
    /// position, write the output, and add what was read times the
    /// feedback at the write position. The feedback moves to its target
    /// over the block by no more than `cap` a frame.
    pub fn render(&mut self, kernel: DelayKernel, left: &mut [f32], right: &mut [f32], cap: f32) {
        let len = left.len().min(right.len());
        if len == 0 {
            return;
        }
        if self.size == 0 {
            left[..len].fill(0.0);
            right[..len].fill(0.0);
            return;
        }
        let (mut gain, step) = glide(&mut self.state, self.feedback, cap, len);
        let (mut r, mut w) = (self.read, self.write);
        for (l_out, r_out) in left[..len].iter_mut().zip(&mut right[..len]) {
            let (a, b) = (self.ring[2 * r], self.ring[2 * r + 1]);
            let (out, back) = match kernel {
                DelayKernel::Straight => ((a, b), (a, b)),
                DelayKernel::Swapped => ((b, a), (a, b)),
                DelayKernel::PingPong => ((b, a), (b, a)),
            };
            (*l_out, *r_out) = out;
            self.ring[2 * r] = 0.0;
            self.ring[2 * r + 1] = 0.0;
            for (place, x) in [(2 * w, back.0), (2 * w + 1, back.1)] {
                let y = x * gain + self.ring[place];
                self.ring[place] = if y.abs() < TINY { 0.0 } else { y };
            }
            gain += step;
            r += 1;
            if r == self.size {
                r = 0;
            }
            w += 1;
            if w == self.size {
                w = 0;
            }
        }
        (self.read, self.write) = (r, w);
    }
}

// ---------------------------------------------------------------------------
// TFruityReverb

/// How long the twelve lines are at room size 100, in 1/20000 s: the four
/// diffusers, then an allpass, a delay, an allpass and a delay for each
/// half of the tank.
pub(crate) const LINES: [u64; 12] = [
    142, 107, 379, 277, 672, 4453, 1800, 3720, 908, 4217, 2656, 3163,
];
/// Where the wet sound's seven taps a side are, in the same unit.
pub(crate) const TAPS_LEFT: [u64; 7] = [266, 2974, 1913, 2496, 1690, 187, 1566];
pub(crate) const TAPS_RIGHT: [u64; 7] = [353, 3627, 1728, 3173, 1811, 335, 621];
/// What the tap weights are at no feedback. They go to 1 as it grows.
pub(crate) const TAP_BLEND: [f32; 5] = [0.25, 0.6, 0.7, 0.2, 0.5];

/// ln 0.001: a tail is over when it is 60 dB down.
pub(crate) const DOWN_60_DB: f64 = -6.907_755_278_982_137;

/// The bass multiplier of each colour (`0x74a348..0x74a358`).
const COLOURS: [f32; 5] = [0.2, 0.6, 1.0, 1.5, 3.0];

/// The engine's denormal guard (`0x180060364`).
const GUARD: f32 = 1e-30;

/// A delay line that is written and read at one place, which moves down
/// through its memory.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Line {
    pub(crate) samples: Vec<f32>,
    pub(crate) at: usize,
}

impl Line {
    pub(crate) fn make_room(&mut self, longest: usize) {
        self.samples
            .reserve_exact(longest.saturating_sub(self.samples.len()));
    }

    /// Start over at this length.
    pub(crate) fn start(&mut self, len: usize) {
        self.samples.clear();
        self.samples.resize(len, 0.0);
        self.at = 0;
    }

    /// Put a sample in, and give the one from the line's length before.
    pub(crate) fn delay(&mut self, x: f32) -> f32 {
        std::mem::replace(&mut self.samples[self.at], x)
    }

    /// One sample through the line as a Schroeder allpass with gain `g`.
    pub(crate) fn allpass(&mut self, x: f32, g: f32) -> f32 {
        let place = &mut self.samples[self.at];
        let v = *place - g * x;
        *place = x + g * v;
        v
    }

    /// Once this frame's sample is in, what was put in `back` frames
    /// before it. Until then `tap(0)` is the sample that is about to leave.
    pub(crate) fn tap(&self, back: usize) -> f32 {
        self.samples[(self.at + back) % self.samples.len()]
    }

    pub(crate) fn step(&mut self) {
        self.at = self.at.checked_sub(1).unwrap_or(self.samples.len() - 1);
    }
}

/// Delphi's `MulDiv` here: the product over the divisor, cut toward 0
/// (`FUN_005b8070`).
fn mul_div(a: i64, b: i64, c: i64) -> i64 {
    a * b / c
}

/// Delphi's `Power` for a base above 0 (`FUN_00749680`).
fn power(base: f64, exponent: f64) -> f64 {
    (exponent * base.ln()).exp()
}

/// The reverb's settings, the wrapper fields +0x38..+0x54.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReverbSettings {
    /// Low cut in Hz, 0 for off.
    pub low_cut: f32,
    /// High cut in 100 Hz, 0 for off.
    pub high_cut: f32,
    /// Predelay in samples.
    pub predelay: i32,
    /// Room size, 1 to 100.
    pub room: i32,
    /// Diffusion in %.
    pub diffusion: i32,
    /// Decay in samples.
    pub decay: f32,
    /// High damping in 100 Hz, 0 for off.
    pub damping: f32,
    /// The bass multiplier's index into {0.2, 0.6, 1, 1.5, 3}; others are 1.
    pub colour: i32,
}

/// What the constructor (`FUN_0074a360`) starts with.
pub const REVERB_DEFAULTS: ReverbSettings = ReverbSettings {
    low_cut: 75.0,
    high_cut: 40.0,
    predelay: 0,
    room: 50,
    diffusion: 100,
    decay: 15.0,
    damping: 40.0,
    colour: 2,
};

/// What [`FruityReverb`]'s update works out, the engine's data block.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Coefficients {
    /// The low cut's allpass coefficient, 0 for off, and what the high cut
    /// and the damping take of their way, 1 for off.
    low_cut: f32,
    high_cut: f32,
    damping: f32,
    /// What each tank delay keeps, the four diffusers' allpass gain and
    /// the tank's two.
    feedback: f32,
    diffuser: f32,
    tank: [f32; 2],
    /// The bass shelf's allpass coefficient and its amount.
    crossover: f32,
    bass: f32,
}

/// `TFruityReverb` over the unmodulated engine.
#[derive(Debug, Clone, PartialEq)]
pub struct FruityReverb {
    settings: ReverbSettings,
    rate: i32,
    /// Whether the next process works the coefficients out first.
    dirty: bool,
    c: Coefficients,
    lines: [Line; 12],
    predelay: Line,
    taps: [[usize; 7]; 2],
    /// The room size the lines were last emptied for.
    last_room: i32,
    /// The high cut's state; the low cut's allpass output and input; each
    /// half's shelf input and output; each half's damping.
    high: f32,
    low: [f32; 2],
    shelf: [[f32; 2]; 2],
    damped: [f32; 2],
}

impl FruityReverb {
    /// A new reverb at this rate, with room for a predelay this long. The
    /// lines take what room size 100 asks for. This allocates.
    pub fn new(rate: i32, longest_predelay: usize) -> Self {
        let rate = rate.max(1);
        let mut reverb = FruityReverb {
            settings: REVERB_DEFAULTS,
            rate,
            dirty: true,
            c: Coefficients::default(),
            lines: Default::default(),
            predelay: Line::default(),
            taps: [[0; 7]; 2],
            last_room: 0,
            high: 0.0,
            low: [0.0; 2],
            shelf: [[0.0; 2]; 2],
            damped: [0.0; 2],
        };
        for (line, units) in reverb.lines.iter_mut().zip(LINES) {
            line.make_room(mul_div(units as i64, i64::from(rate), 20_000) as usize);
        }
        reverb.predelay.make_room(longest_predelay.max(1));
        reverb.size();
        reverb
    }

    /// The rate it was made for.
    pub fn rate(&self) -> i32 {
        self.rate
    }

    /// The settings it has.
    pub fn settings(&self) -> &ReverbSettings {
        &self.settings
    }

    /// As FL's constructor leaves it, with nothing in its lines.
    pub fn fresh(&mut self) {
        self.settings = REVERB_DEFAULTS;
        self.last_room = 0;
        self.size();
    }

    /// Take new settings (the setter's stores). Another predelay empties
    /// everything; the rest waits for the next process. A predelay longer
    /// than the room [`FruityReverb::new`] asked for is cut to it.
    pub fn set(&mut self, settings: ReverbSettings) {
        let longest = self.predelay.samples.capacity() as i32;
        let settings = ReverbSettings {
            predelay: settings.predelay.min(longest),
            ..settings
        };
        let predelay = settings.predelay != self.settings.predelay;
        self.settings = settings;
        if predelay {
            self.size();
        }
        self.dirty = true;
    }

    /// Slot 0 (`FUN_00749970`): every line at its room size 100 length,
    /// the predelay at its length (at least 1), the taps at room 100, and
    /// everything empty.
    fn size(&mut self) {
        let rate = i64::from(self.rate);
        for (line, units) in self.lines.iter_mut().zip(LINES) {
            line.start(mul_div(units as i64, rate, 20_000) as usize);
        }
        self.predelay.start(self.settings.predelay.max(1) as usize);
        for (taps, units) in self.taps.iter_mut().zip([TAPS_LEFT, TAPS_RIGHT]) {
            *taps = units.map(|u| mul_div(u as i64, rate, 20_000) as usize);
        }
        self.reset();
        self.dirty = true;
    }

    /// Empty the lines and the filters (`FUN_007498f0`).
    pub fn reset(&mut self) {
        for line in self.lines.iter_mut().chain([&mut self.predelay]) {
            line.samples.fill(0.0);
        }
        self.high = 0.0;
        self.low = [0.0; 2];
        self.shelf = [[0.0; 2]; 2];
        self.damped = [0.0; 2];
    }

    /// The update (`FUN_00749b60`).
    fn update(&mut self) {
        let s = self.settings;
        let rate = self.rate;
        let inv = (1.0 / f64::from(rate)) as f32;
        let nyquist = (f64::from(rate) * 0.5) as f32;
        let below = |x: f32| if nyquist <= x { nyquist } else { x };
        // One-pole low pass for a corner in 100 Hz, 1 when off.
        let low_pass = |corner: f32| {
            if corner == 0.0 {
                return 1.0;
            }
            let w = f64::from(below(corner)) * 628.318_530_717_958_7 * f64::from(inv);
            let c = (2.0 - w.cos()) as f32;
            (1.0 - (f64::from(c) - (f64::from(c * c) - 1.0).sqrt())) as f32
        };
        let c = &mut self.c;
        c.low_cut = if s.low_cut == 0.0 {
            0.0
        } else {
            let t = (f64::from(below(s.low_cut)) * PI * f64::from(inv)).tan() as f32;
            ((f64::from(t) - 1.0) / (f64::from(t) + 1.0)) as f32
        };
        c.high_cut = low_pass(s.high_cut);

        let scale = mul_div(i64::from(rate), i64::from(s.room) + 15, 115);
        let lens = LINES.map(|u| (mul_div(u as i64, scale, 20_000) as usize).max(1));
        for (taps, units) in self.taps.iter_mut().zip([TAPS_LEFT, TAPS_RIGHT]) {
            *taps = units.map(|u| mul_div(u as i64, scale, 20_000) as usize);
        }
        let room = s.room != self.last_room;
        self.last_room = s.room;
        for (line, len) in self.lines.iter_mut().zip(lens) {
            if room {
                line.start(len);
            } else if line.samples.len() != len {
                line.samples.resize(len, 0.0);
                line.at %= len;
            }
        }

        let diffusion = (f64::from(s.diffusion) * 0.01) as f32;
        let half_inv = (f64::from(inv) * 0.5) as f32;
        let gain = |a: usize, b: usize, spread: f64| {
            let seconds = f64::from((lens[a] + lens[b]) as f32 * half_inv);
            ((DOWN_60_DB * seconds) / (f64::from(diffusion) * spread)).exp() as f32
        };
        if diffusion == 0.0 {
            c.diffuser = 0.0;
            c.tank = [0.0; 2];
        } else {
            c.diffuser = gain(0, 1, 0.07);
            c.tank = [gain(4, 8, 0.35), gain(6, 10, 0.51)];
        }

        let around = (lens[5] + lens[7] + lens[9] + lens[11]) as f64;
        let keep = |decay: f32| power((DOWN_60_DB * around / f64::from(decay)).exp(), 0.25);
        c.feedback = keep(s.decay) as f32;
        c.damping = low_pass(s.damping);
        let colour = usize::try_from(s.colour).ok().and_then(|i| COLOURS.get(i));
        let multiplier = colour.copied().unwrap_or(1.0);
        let mut ratio = keep(s.decay * multiplier) as f32 / c.feedback;
        c.bass = ((f64::from(ratio) - 1.0) * 0.5) as f32;
        if f64::from(ratio) > 1.0 {
            ratio = 1.0;
        }
        let t = (500.0 * PI * f64::from(inv)).tan() as f32;
        c.crossover = (t - ratio) / (t + ratio);
        self.dirty = false;
    }

    /// A mono block through the network into the stereo wet
    /// (`FUN_0074a540`, then the engine's `FUN_180002980`).
    pub fn process(&mut self, input: &[f32], left: &mut [f32], right: &mut [f32]) {
        if self.dirty {
            self.update();
        }
        let c = self.c;
        let g = c.feedback;
        let rest = 1.0 - g;
        let [w1, w2, w3, w4, w5] = TAP_BLEND.map(|blend| rest * blend + g);
        let frames = input.iter().zip(left.iter_mut().zip(right.iter_mut()));
        for (x, (out_l, out_r)) in frames {
            // The high cut, then the low cut: half of what its allpass and
            // its input differ by.
            let lp = ((x + GUARD) - self.high) * c.high_cut + self.high;
            self.high = lp;
            let turned = (lp - self.low[0]) * c.low_cut + self.low[1];
            self.low = [turned, lp];
            let mut x = if c.low_cut != 0.0 {
                (lp - turned) * 0.5 + GUARD
            } else {
                lp
            };

            let (diffusers, tank) = self.lines.split_at_mut(4);
            for line in diffusers.iter_mut() {
                x = line.allpass(x, c.diffuser);
                line.step();
            }
            let fed = self.predelay.delay(x);
            self.predelay.step();

            // Each half starts from the other half's last delay: the first
            // reads it before the second writes, the second after the
            // first has written and stepped.
            for half in 0..2 {
                let (lines, other) = if half == 0 {
                    let (a, b) = tank.split_at_mut(4);
                    (a, &b[3])
                } else {
                    let (a, b) = tank.split_at_mut(4);
                    (b, &a[3])
                };
                let end = other.samples[other.at] * g;
                let shelf = &mut self.shelf[half];
                let turned = (end - shelf[1]) * c.crossover + shelf[0];
                *shelf = [end, turned];
                let y = (turned + end) * c.bass + end + fed;
                let y = (y - self.damped[half]) * c.damping + self.damped[half];
                self.damped[half] = y;
                let v = lines[0].allpass(y, c.tank[0]);
                lines[0].step();
                let v = lines[1].delay(v) * g;
                lines[1].step();
                let v = lines[2].allpass(v, c.tank[1]);
                lines[2].step();
                lines[3].delay(v);
                lines[3].step();
            }

            // The taps, read after every line has stepped: lines 5, 6 and
            // 7 are the tank's 1, 2 and 3 here, and 9, 10 and 11 its 5, 6
            // and 7.
            let (l, r) = (&self.taps[0], &self.taps[1]);
            *out_l = tank[5].tap(l[0])
                - w1 * tank[5].tap(l[1])
                - w3 * tank[6].tap(l[2])
                - w4 * tank[7].tap(l[3])
                + w2 * tank[1].tap(l[4])
                + tank[2].tap(l[5])
                + w5 * tank[3].tap(l[6]);
            *out_r = w1 * tank[1].tap(r[1]) + tank[1].tap(r[0])
                - w3 * tank[2].tap(r[2])
                - w4 * tank[3].tap(r[3])
                - w2 * tank[5].tap(r[4])
                + tank[6].tap(r[5])
                + w5 * tank[7].tap(r[6]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_delay_echoes_after_its_time_and_feeds_back() {
        let mut delay = Delay32::with_capacity(1000);
        delay.set_feedback(0.5);
        assert!(delay.set_length(64, 10));
        assert!(!delay.set_length(64, 10));
        let mut input = [0.0f32; 64];
        input[0] = 1.0;
        let (mut l, mut r) = ([0.0; 64], [0.0; 64]);
        delay.add(0, &input, 0);
        delay.add(1, &input, 3);
        delay.render(DelayKernel::Straight, &mut l, &mut r, 1.0);
        let hits = |side: &[f32]| -> Vec<(usize, f32)> {
            (0..side.len())
                .filter(|n| side[*n] != 0.0)
                .map(|n| (n, side[n]))
                .collect()
        };
        assert_eq!(
            hits(&l),
            [
                (10, 1.0),
                (20, 0.5),
                (30, 0.25),
                (40, 0.125),
                (50, 0.0625),
                (60, 0.03125)
            ]
        );
        assert_eq!(hits(&r)[..2], [(13, 1.0), (23, 0.5)]);
    }

    #[test]
    fn ping_pong_crosses_the_sides_every_echo() {
        let mut delay = Delay32::with_capacity(1000);
        delay.set_feedback(1.0);
        delay.set_length(64, 8);
        let mut input = [0.0f32; 40];
        input[0] = 1.0;
        let (mut l, mut r) = ([0.0; 40], [0.0; 40]);
        delay.add(0, &input, 0);
        delay.render(DelayKernel::PingPong, &mut l, &mut r, 1.0);
        // In on the left, out first on the right, then back and forth.
        assert_eq!((l[8], r[8], l[16], r[16], r[24]), (0.0, 1.0, 1.0, 0.0, 1.0));
    }

    #[test]
    fn a_feedback_change_ramps_by_the_cap() {
        let mut delay = Delay32::with_capacity(100);
        delay.set_feedback(0.0);
        delay.set_length(10, 1);
        delay.set_feedback(1.0);
        let (mut l, mut r) = ([0.0; 10], [0.0; 10]);
        delay.render(DelayKernel::Straight, &mut l, &mut r, 0.01);
        assert!((delay.state - 0.1).abs() < 1e-6);
    }

    #[test]
    fn the_flanger_s_gain_scales_every_voice() {
        let wheels = [4, 512, 768, 8, 128, -32];
        let (mut one, mut half) = (StereoFlanger::default(), StereoFlanger::default());
        for (f, gain) in [(&mut one, 1.0), (&mut half, 0.5)] {
            f.make_room(44_100.0);
            assert!(f.set(&FlangerSettings::from_wheels(wheels, gain), 44_100.0));
        }
        for (a, b) in one.voices.iter().zip(&half.voices).take(one.count) {
            assert_eq!(a.gains.map(|g| g * 0.5), b.gains);
        }
        // Sleeping moves the LFOs as far as playing does.
        let mut slept = one.clone();
        slept.skip(100);
        for _ in 0..100 {
            one.frame(0.0, 0.0);
        }
        let phases = |f: &StereoFlanger| f.voices.map(|v| v.phase);
        assert_eq!(phases(&slept), phases(&one));
    }

    fn reverb(settings: ReverbSettings) -> FruityReverb {
        let mut reverb = FruityReverb::new(44_100, 4096);
        reverb.set(settings);
        reverb
    }

    #[test]
    fn the_reverb_s_lines_follow_the_room_with_whole_numbers_cut_down() {
        let mut r = reverb(ReverbSettings {
            room: 40,
            ..REVERB_DEFAULTS
        });
        r.process(&[0.0], &mut [0.0], &mut [0.0]);
        // 44100 * 55 / 115 = 21091 (cut), 4453 * 21091 / 20000 = 4695.
        assert_eq!(r.lines[5].samples.len(), 4695);
        assert_eq!(r.lines[0].samples.len(), 142 * 21_091 / 20_000);
        assert_eq!(r.taps[1][1], 3627 * 21_091 / 20_000);
        assert_eq!(r.predelay.samples.len(), 1);
    }

    #[test]
    fn the_reverb_rings_for_its_decay() {
        let decay = 1.5 * 44_100.0;
        let mut r = reverb(ReverbSettings {
            decay,
            ..REVERB_DEFAULTS
        });
        let n = 44_100 * 3;
        let mut input = vec![0.0f32; n];
        input[0] = 1.0;
        let (mut l, mut rr) = (vec![0.0; n], vec![0.0; n]);
        r.process(&input, &mut l, &mut rr);
        assert!(l.iter().chain(&rr).all(|v| v.is_finite()));
        let energy = |from: usize| -> f64 {
            l[from..from + 4410]
                .iter()
                .map(|v| f64::from(*v).powi(2))
                .sum()
        };
        // Four tank delays in the loop give 60 dB in the decay time; the
        // allpasses add some. It falls at least 40 dB in 1.5 s and does not
        // stop.
        let fall = 10.0 * (energy(4410) / energy(4410 + 66_150)).log10();
        assert!(fall > 40.0 && fall < 80.0, "{fall}");
        // The right side is not the left.
        assert!(l[2000..3000] != rr[2000..3000]);
    }

    #[test]
    fn the_reverb_s_coefficients_are_the_update_s() {
        // Colour 3 makes the bass ring 1.5 times as long: each delay keeps
        // g^(2/3) of it, so the shelf adds (g^(-1/3) - 1) / 2, and its
        // allpass sits at 500 Hz with the ratio held at 1.
        let decay = 2.0 * 44_100.0;
        let mut r = reverb(ReverbSettings {
            colour: 3,
            decay,
            ..REVERB_DEFAULTS
        });
        r.process(&[0.0], &mut [0.0], &mut [0.0]);
        let c = r.c;
        let g = f64::from(c.feedback);
        assert!(g > 0.5 && g < 1.0, "{c:?}");
        assert!((f64::from(c.bass) - (g.powf(-1.0 / 3.0) - 1.0) / 2.0).abs() < 1e-5);
        let t = (500.0 * PI / 44_100.0).tan();
        assert!((f64::from(c.crossover) - (t - 1.0) / (t + 1.0)).abs() < 1e-5);
        // A shorter bass keeps the ratio and shelves the other way.
        let mut short = reverb(ReverbSettings {
            colour: 0,
            decay,
            ..REVERB_DEFAULTS
        });
        short.process(&[0.0], &mut [0.0], &mut [0.0]);
        assert!(short.c.bass < 0.0 && short.c.crossover > c.crossover);
        // Off is 0 for the low cut, 1 for the low passes, and colour 2 is flat.
        let settings = ReverbSettings {
            low_cut: 0.0,
            high_cut: 0.0,
            damping: 0.0,
            decay,
            ..REVERB_DEFAULTS
        };
        let mut off = reverb(settings);
        off.process(&[0.0], &mut [0.0], &mut [0.0]);
        let c = off.c;
        assert_eq!(
            (c.low_cut, c.high_cut, c.damping, c.bass),
            (0.0, 1.0, 1.0, 0.0)
        );
    }
}
