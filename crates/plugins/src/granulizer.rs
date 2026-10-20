//! Fruity Granulizer: the channel's sample played as a stream of short,
//! overlapping grains, as `docs/plugins/fruity-granulizer.md` and the
//! listings of `Fruity Granulizer_x64.dll` have it.
//!
//! # The state
//!
//! A version (FL 2026 writes 7) and, as far as the state goes, twelve
//! little-endian i32 parameters, the transient source, the trigger mode,
//! the tempo lock and the instrument tab (`0x7519f0`):
//!
//! | version | bytes | what it holds |
//! |---|---|---|
//! | 1 | 44 | parameters 0 to 9 |
//! | 2, 3 | 48 | and parameter 10 |
//! | 4 | 56 | and parameter 11 and the transient source |
//! | 5, 6 | 60 | and the trigger mode |
//! | 7 | 428 | and the tempo lock and the instrument tab |
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Grain attack | 50 to 500 | 204 |
//! | 1 Grain hold | 0 to 500 | 50 |
//! | 2 Grain spacing | 125 to 1000 | 611 |
//! | 3 Wave spacing | -1000 to 1000 | 611 |
//! | 4 Effect depth | 0 to 384 | 0 |
//! | 5 Effect speed | 4 to 200 | 180 |
//! | 6 Randomness | 0 to 256 | 0 |
//! | 7 Stereo separation | 0 to 64 | 0 |
//! | 8 Loop | 0 to 1 | 0 |
//! | 9 Hold | 0 to 1 | 0 |
//! | 10 Sample start, of 4096 | 0 to 4096 | 0 |
//! | 11 Transient hold | 0 to 500 | 228 |
//!
//! A state missing parameter 10 has it at 0, and one missing 11 at 248.
//!
//! # The sound
//!
//! The sample is the channel's: FL hands it to the plugin with
//! `FPD_SetSample` (`0x754b30`), and the host here does that with
//! [`Generator::set_channel_sample`]. A note plays nothing without one.
//!
//! Every `spacing` samples a note starts a grain at its place in the
//! sample, and the place moves on by `advance` frames (`0x753ea0`). A
//! grain reads the sample at the note's pitch from there, and fades in
//! for `attack` samples, holds for `hold` and fades out for `attack`:
//!
//! ```text
//! attack, hold = round((51^(p / 500) - 1) * 10) ms         (0x752170)
//! curve(p)     = sign(p) round((11^(|p| / 1000) - 1) * 76.8)  (0x7521f0)
//! spacing      = max((attack + hold) * curve(p2) / 256, 5)
//! advance      = (attack + hold) * curve(p3) / 256 * file rate / rate
//! ```
//!
//! so the defaults, 256 of 256, play the sample at its own speed. The
//! grains of a note pan to either side in turn by the stereo separation,
//! through the circular pan law at a volume of 1.42. The effect takes
//! `|sin(phase)| * advance * depth / 256` off the place before each step,
//! where the phase moves by `pi / (204 - speed)` a grain. Randomness
//! jumps the place by a random part of the sample instead, for that
//! share of 256 of the grains. Hold keeps the place where it is. Past
//! the end the place wraps, and without loop the note stops making
//! grains there and ends when the last is over.
//!
//! The trigger mode sets where a note starts: key to pitch plays every
//! note from the start knob at its pitch; key to start plays it at the
//! sample's own pitch, from `(key - 60) / 24` of the sample on, or from
//! `(key - 60) / 4` beats of the song's tempo on.
//!
//! What a note makes goes through the instrument tab (`crate::tab`).
//!
//! What is not FL's:
//!
//! - FL reads the sample with a kernel that is not read. A 4-point
//!   Hermite curve stands in between frames; at the file's own pitch and
//!   rate the frames are read as they are, as FL's player does.
//! - The transients (the regions of the file, or the ones FL detects), and
//!   so the transient trigger mode and the transient hold, are not built,
//!   and fidelity says so for a state that asks for them.
//! - The tempo lock needs the sample's length in beats, which the host
//!   hands FL's plugin and no host here does: the plugin plays unlocked,
//!   as FL's does until it is told.
//! - The loop points of a file are not read: a loop is the whole sample.
//! - FL draws its random numbers from a seed it shares with everything
//!   else. Here each plugin starts from 0.

use crate::fruit_kick::Articulation;
use crate::sampler::{Wave, circular};
use crate::tab::{Articulator, Clock, Tab};
use crate::three_osc::{PPQ, new_tab};
use crate::{Fidelity, Generator, NoteOn, SampleFile, StateError, knob_value, state_version};
use std::f64::consts::PI;

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Granulizer";

/// The version FL 2026 writes.
const WRITTEN: i32 = 7;

/// How many notes sound at once. A note past them cuts the oldest.
const VOICES: usize = 32;

/// How many grains a note has at once. The parameters' ranges keep it
/// under 20.
const GRAINS: usize = 32;

/// The samples a voice renders at a time.
const CHUNK: usize = 256;

const RANGES: [(i32, i32); 12] = [
    (50, 500),
    (0, 500),
    (125, 1000),
    (-1000, 1000),
    (0, 384),
    (4, 200),
    (0, 256),
    (0, 64),
    (0, 1),
    (0, 1),
    (0, 4096),
    (0, 500),
];

/// The wheels' defaults, from the plugin's form.
const DEFAULTS: [i32; 12] = [204, 50, 611, 611, 0, 180, 0, 0, 0, 0, 0, 228];

/// The pan law's volume for a grain (`0x753e98`).
const GRAIN_VOLUME: f32 = 1.42;

/// Where a note starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// Every note from the start knob, at its pitch.
    Pitch,
    /// The key picks the place, two octaves over the sample.
    SampleTime,
    /// The key picks the place, a quarter beat a key.
    SongTime,
    /// The key picks a transient.
    TransientTime,
}

/// The milliseconds a time wheel stands for (`0x752170`).
pub fn millis(value: i32) -> i32 {
    let along = (f64::from(value) * 0.002) as f32;
    let curve = ((f64::from(along) * 51f64.ln()).exp() - 1.0) as f32;
    f64::from(curve * 10.0).round_ties_even() as i32
}

/// How many 256ths of a grain a spacing wheel stands for (`0x7521f0`).
pub fn spacing(value: i32) -> i32 {
    let along = (f64::from(value.unsigned_abs()) * 0.001) as f32;
    let curve = ((f64::from(along) * 11f64.ln()).exp() - 1.0) as f32;
    let size = (f64::from(curve) * 76.8).round_ties_even() as i32;
    if value < 0 { -size } else { size }
}

/// Delphi's `Random(range)`.
fn random(seed: &mut u32, range: u32) -> u32 {
    *seed = seed.wrapping_mul(0x0808_8405).wrapping_add(1);
    ((u64::from(*seed) * u64::from(range)) >> 32) as u32
}

/// What the parameters come to at a rate, for a sample.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Derived {
    /// Samples (`+0x16c`, `+0x170`).
    attack: f32,
    hold: f32,
    /// Samples between two grains (`+0x180`).
    spacing: f32,
    /// Frames the place moves on by a grain (`+0x188`).
    advance: f32,
    /// The effect's depth, of 1, and what a grain adds to its phase.
    depth: f32,
    speed: f32,
    /// The grains' pan, of 1.
    pan: f32,
    /// How many frames the start knob is from the start.
    start: i64,
}

impl Derived {
    fn of(params: &[i32; 12], rate: f32, file_rate: Option<u32>, last: i64) -> Self {
        // FL's time base is its rate as a whole number.
        let base = f64::from(rate as i32) * f64::from(0.001f32);
        let samples = |value: i32| (f64::from(millis(value)) * base) as f32;
        let (attack, hold) = (samples(params[0]), samples(params[1]));
        let grain = attack + hold;
        let ratio = file_rate.map_or(1.0, f64::from) * (1.0 / f64::from(rate));
        let ws = f64::from(grain * spacing(params[3]) as f32);
        Derived {
            attack,
            hold,
            spacing: (grain * spacing(params[2]) as f32 * (1.0 / 256.0)).max(5.0),
            advance: (ws * ratio * f64::from(1.0f32 / 256.0)) as f32,
            depth: params[4] as f32 * (1.0 / 256.0),
            speed: (PI / f64::from((204 - params[5]).max(1))) as f32,
            pan: params[7] as f32 * (1.0 / 64.0),
            start: (i128::from(params[10]) * i128::from(last) / 4096) as i64,
        }
    }
}

/// One grain: where it reads the sample and how loud it is.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Grain {
    /// The frame it reads next, and what a sample moves that by.
    at: f64,
    step: f64,
    /// The fade's gain and what a sample adds to it.
    gain: f32,
    gain_step: f32,
    gains: (f32, f32),
    /// Samples from the end of the fade in, which starts it below 0; where
    /// the hold ends and where the grain ends.
    pos: i32,
    hold_end: i32,
    end: i32,
    /// The samples into the block it starts at.
    delay: usize,
}

/// One note.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Voice {
    id: u32,
    /// The note's cents from key 60 as the host has it, the cents the
    /// trigger mode takes off it, and what the tab's pitch block adds.
    pitch: f32,
    offset: f32,
    bend: f32,
    /// Samples since the note started, and where the next grain starts.
    counter: i64,
    next: i64,
    /// The place in the sample, in frames, and the effect's phase.
    place: f32,
    phase: f32,
    /// The grains started so far, which turns their pan.
    started: u32,
    grains: [Grain; GRAINS],
    sounding: usize,
    /// The place ran off the end, and the note makes no more grains; the
    /// place has wrapped into the loop.
    ended: bool,
    looped: bool,
    articulator: Articulator,
}

/// Fruity Granulizer. A new one is at 44.1 kHz with FL's defaults and no
/// sample.
#[derive(Debug, Clone, PartialEq)]
pub struct Granulizer {
    params: [i32; 12],
    transients: i32,
    trigger: i32,
    tempo_lock: bool,
    tab: Articulation,
    rate: f32,
    tempo: f64,
    ppq: f64,
    derived: Derived,
    played: Tab,
    clock: Clock,
    /// The path the channel names, and its file once the host has handed
    /// it over.
    path: Option<String>,
    wave: Option<Wave>,
    missing: bool,
    voices: Vec<Voice>,
    seed: u32,
    scratch: [[f32; CHUNK]; 2],
}

impl Default for Granulizer {
    fn default() -> Self {
        let tab = new_tab();
        let mut plugin = Granulizer {
            params: DEFAULTS,
            transients: 0,
            trigger: 0,
            tempo_lock: false,
            played: Tab::new(&tab, 44_100.0, 140.0, PPQ),
            tab,
            rate: 44_100.0,
            tempo: 140.0,
            ppq: PPQ,
            derived: Derived::of(&DEFAULTS, 44_100.0, None, 0),
            clock: Clock::default(),
            path: None,
            wave: None,
            missing: false,
            voices: Vec::with_capacity(VOICES),
            seed: 0,
            scratch: [[0.0; CHUNK]; 2],
        };
        plugin.derive();
        plugin
    }
}

impl Granulizer {
    /// Where a note starts in the sample.
    pub fn trigger(&self) -> Trigger {
        match self.trigger {
            1 => Trigger::SampleTime,
            2 => Trigger::SongTime,
            3 => Trigger::TransientTime,
            _ => Trigger::Pitch,
        }
    }

    /// The parameters' values, as the state has them.
    pub fn params(&self) -> &[i32; 12] {
        &self.params
    }

    /// The last frame of the sample, and -1 without one.
    fn last(&self) -> i64 {
        self.wave.as_ref().map_or(-1, |wave| wave.len() as i64 - 1)
    }

    fn derive(&mut self) {
        let file_rate = self.wave.as_ref().map(|wave| wave.rate);
        self.derived = Derived::of(&self.params, self.rate, file_rate, self.last().max(0));
    }

    fn retab(&mut self) {
        self.played = Tab::new(&self.tab, self.rate, self.tempo, self.ppq);
    }

    /// Hand over the sample, as the host does for the channel's.
    pub(crate) fn set_wave(&mut self, wave: Wave) {
        self.wave = Some(wave);
        self.voices.clear();
        self.derive();
    }

    /// The place a note of this key starts at, and the cents taken off its
    /// pitch (`0x753580`). FL reads the key from `InitLevels.Pitch`, which
    /// is `key * 100` with no fine pitch.
    fn start(&self, key: i16, last: i64) -> (f32, f32) {
        let start = self.derived.start as f32;
        let mode = self.trigger();
        if mode == Trigger::Pitch {
            return (start, 0.0);
        }
        let key = i32::from(key) - 60;
        let offset = key as f32 * 100.0;
        let key = key.max(0);
        let file_rate = self.wave.as_ref().map_or(44_100, |wave| wave.rate);
        let moved = match mode {
            Trigger::SampleTime => (last * i64::from(key)) as f64 / 24.0,
            Trigger::SongTime => {
                f64::from(key) / 4.0 * 60.0 * f64::from(file_rate) / self.tempo.max(1.0)
            }
            _ => 0.0,
        };
        let place = (f64::from(start) + moved).round_ties_even() as i64;
        ((place % (last + 1)) as f32, offset)
    }

    /// Start a grain at `place` for the voice (`0x753c70`).
    fn spawn(
        derived: &Derived,
        wave: &Wave,
        rate: f32,
        voice: &mut Voice,
        place: i32,
        delay: usize,
    ) {
        let last = wave.len() as i64 - 1;
        if i64::from(place) > last || voice.sounding == GRAINS {
            return;
        }
        voice.started += 1;
        let attack = f64::from(derived.attack).round_ties_even() as i32;
        let hold = f64::from(derived.hold).round_ties_even() as i32;
        let cents = f64::from(voice.pitch + voice.bend - voice.offset);
        let ratio = f64::from(wave.rate) * (1.0 / f64::from(rate));
        let step = if cents == 0.0 && ratio == 1.0 {
            1.0
        } else {
            (cents * (std::f64::consts::LN_2 / 1200.0)).exp() * ratio
        };
        let fade = (1.0 / f64::from(attack.max(1))) as f32;
        let (cos, sin) = circular(derived.pan, GRAIN_VOLUME);
        let gains = match voice.started & 1 {
            0 => (cos, sin),
            _ => (sin, cos),
        };
        voice.grains[voice.sounding] = Grain {
            at: f64::from(place.clamp(0, last as i32 + 1)),
            step,
            gain: fade,
            gain_step: fade,
            gains,
            pos: -attack,
            hold_end: hold,
            end: hold + attack,
            delay,
        };
        voice.sounding += 1;
    }

    /// The next `len` samples of a voice's grains into `out`, which they
    /// fill. Whether the voice is over comes back (`0x753ea0`).
    fn grains(&mut self, index: usize, len: usize) -> bool {
        let Some(wave) = &self.wave else {
            return true;
        };
        let derived = &self.derived;
        let voice = &mut self.voices[index];
        let [out_l, out_r] = &mut self.scratch;
        out_l[..len].fill(0.0);
        out_r[..len].fill(0.0);
        let params = &self.params;
        let last = wave.len() as i64 - 1;
        let loop_len = last + 1;
        let looping = params[8] != 0;
        let mut over = false;

        if !voice.ended {
            let before = voice.counter;
            voice.counter += len as i64;
            while voice.next < voice.counter {
                let mut place = f64::from(voice.place).round_ties_even() as i64;
                let share = params[6];
                let calm = share < 1 || params[9] != 0 || {
                    let drawn = random(&mut self.seed, 256) as i32;
                    share <= 0xff - drawn
                };
                if !calm {
                    place += i64::from(random(&mut self.seed, loop_len as u32));
                    while place > last {
                        place -= loop_len;
                    }
                }
                let delay = (voice.next - before) as usize;
                Self::spawn(derived, wave, self.rate, voice, place as i32, delay);
                if voice.next == 0 && voice.place == 0.0 && voice.sounding > 0 {
                    // The first grain of a note starting at the top plays
                    // in full at once.
                    let first = &mut voice.grains[0];
                    first.hold_end -= first.pos;
                    first.end -= first.pos;
                    (first.pos, first.gain_step, first.gain) = (0, 0.0, 1.0);
                }
                voice.next += f64::from(derived.spacing).round_ties_even() as i64;
                if params[9] == 0 {
                    if params[4] > 0 {
                        let sway = f64::from(voice.phase).sin().abs();
                        let back = sway * f64::from(derived.advance) * f64::from(derived.depth);
                        voice.place -= back.round_ties_even() as i32 as f32;
                    }
                    voice.phase += derived.speed;
                    voice.place += derived.advance;
                    let end = last as f32;
                    if end < voice.place {
                        if looping {
                            voice.looped = true;
                        } else {
                            voice.ended = true;
                            if voice.sounding == 0 {
                                over = true;
                            }
                        }
                        while end < voice.place {
                            voice.place -= loop_len as f32;
                        }
                    }
                    if !voice.looped {
                        while voice.place < 0.0 {
                            voice.place += end + 1.0;
                        }
                    }
                }
            }
        }

        let mut k = 0;
        while k < voice.sounding {
            let grain = &mut voice.grains[k];
            let from = grain.delay.min(len);
            grain.delay = 0;
            let mut done = false;
            let mut at = from;
            while at < len {
                // The frames to the end of the stretch the grain is in.
                let stretch = if grain.pos < 0 {
                    -grain.pos
                } else if grain.pos < grain.hold_end {
                    grain.hold_end - grain.pos
                } else {
                    grain.end - grain.pos
                };
                let run = (stretch.max(0) as usize).min(len - at);
                for n in 0..run {
                    let floor = grain.at.floor() as i64;
                    if floor >= loop_len {
                        if looping {
                            grain.at -= loop_len as f64;
                        } else {
                            done = true;
                            break;
                        }
                    }
                    let (l, r) = wave.frame(grain.at);
                    grain.at += grain.step;
                    let gain = grain.gain + n as f32 * grain.gain_step;
                    out_l[at + n] += l * gain * grain.gains.0;
                    out_r[at + n] += r * gain * grain.gains.1;
                }
                if done {
                    break;
                }
                grain.gain += run as f32 * grain.gain_step;
                grain.pos += run as i32;
                at += run;
                if grain.pos >= grain.end {
                    done = true;
                    break;
                }
                if grain.pos == grain.hold_end {
                    grain.gain_step = (-1.0 / f64::from(grain.end - grain.hold_end)) as f32;
                } else if grain.pos == 0 {
                    grain.gain_step = 0.0;
                }
            }
            if done {
                voice.sounding -= 1;
                voice.grains[k] = voice.grains[voice.sounding];
                if voice.ended && voice.sounding == 0 {
                    over = true;
                }
            } else {
                k += 1;
            }
        }
        over
    }
}

impl Generator for Granulizer {
    /// The notes that sound are cut.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let version = state_version(bytes, 428)?;
        if !(1..=WRITTEN).contains(&version) {
            return Err(StateError::Version { found: version });
        }
        if bytes.len() < 44 {
            return Err(StateError::Length {
                expected: 44,
                found: bytes.len(),
            });
        }
        let words: Vec<i32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|word| i32::from_le_bytes(*word))
            .collect();
        let mut params = DEFAULTS;
        (params[10], params[11]) = (0, 248);
        let given = (words.len() - 1).min(12);
        params[..given].copy_from_slice(&words[1..=given]);
        let mut at = 13;
        let mut next = |present: bool| {
            let value = words.get(at).copied().filter(|_| present);
            if present {
                at += 1;
            }
            value.unwrap_or(0)
        };
        let transients = next(version >= 4);
        let trigger = next(version >= 5);
        let lock = next(version > 6);
        self.tab = match bytes.get(4 * at..).and_then(|tab| tab.first_chunk::<364>()) {
            Some(tab) => Articulation::read(tab),
            None => new_tab(),
        };
        self.params = params;
        self.transients = transients;
        self.trigger = trigger;
        self.tempo_lock = lock & 0xff != 0;
        self.voices.clear();
        self.derive();
        self.retab();
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        self.rate = hz;
        self.derive();
        self.retab();
    }

    fn set_tempo(&mut self, bpm: f64) {
        if bpm.is_finite() && bpm > 0.0 {
            self.tempo = bpm;
            self.retab();
        }
    }

    fn set_ppq(&mut self, ppq: u16) {
        if ppq > 0 {
            self.ppq = f64::from(ppq);
            self.retab();
        }
    }

    fn set_channel_sample(&mut self, path: &str) {
        self.path = Some(path.to_owned());
    }

    fn sample_paths(&self) -> Vec<String> {
        self.path.iter().cloned().collect()
    }

    fn set_sample(&mut self, path: &str, file: Option<SampleFile<'_>>) {
        if self.path.as_deref() != Some(path) {
            return;
        }
        self.missing = file.is_none();
        match file {
            Some(file) => self.set_wave(Wave::of(file)),
            None => {
                self.wave = None;
                self.voices.clear();
                self.derive();
            }
        }
    }

    /// Nothing happens without a sample.
    fn note_on(&mut self, note: NoteOn) {
        let last = self.last();
        if last < 0 {
            return;
        }
        if self.voices.len() == VOICES {
            self.voices.remove(0);
        }
        let pitch = (f32::from(note.key) - 60.0) * 100.0 + note.fine_pitch;
        let (place, offset) = self.start(note.key, last);
        self.voices.push(Voice {
            id: note.voice,
            pitch,
            offset,
            bend: 0.0,
            counter: 0,
            next: 0,
            place,
            phase: 0.0,
            started: 0,
            grains: [Grain::default(); GRAINS],
            sounding: 0,
            ended: false,
            looped: false,
            articulator: Articulator::new(&note, &self.played),
        });
    }

    fn note_off(&mut self, voice: u32) {
        let tab = &self.played;
        for sounding in self.voices.iter_mut().filter(|v| v.id == voice) {
            sounding.articulator.release(tab);
        }
    }

    fn all_notes_off(&mut self) {
        for sounding in &mut self.voices {
            sounding.articulator.release(&self.played);
        }
    }

    /// The grains a note starts from then on take the new pitch.
    fn set_note_pitch(&mut self, voice: u32, pitch: f32) {
        for sounding in self.voices.iter_mut().filter(|v| v.id == voice) {
            sounding.pitch = pitch;
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        let mut at = 0;
        while at < len {
            if self.clock.tick(&self.played) {
                for voice in &mut self.voices {
                    voice.articulator.tick();
                }
            }
            let run = (len - at).min(self.clock.to_tick).min(CHUNK);
            let mut index = 0;
            while index < self.voices.len() {
                let voice = &mut self.voices[index];
                if voice.articulator.stale {
                    let globals = self.clock.globals;
                    voice.bend =
                        voice
                            .articulator
                            .begin(&self.played, &globals, self.clock.to_tick);
                }
                let mut over = self.grains(index, run);
                let [scratch_l, scratch_r] = &self.scratch;
                let voice = &mut self.voices[index];
                let (l, r) = (&mut left[at..at + run], &mut right[at..at + run]);
                let voiced = (&scratch_l[..run], &scratch_r[..run]);
                over |= voice.articulator.run(&self.played, voiced, l, r);
                if over {
                    self.voices.remove(index);
                } else {
                    index += 1;
                }
            }
            at += run;
            self.clock.to_tick -= run;
        }
    }

    fn param_count(&self) -> usize {
        self.params.len()
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |value| *value as f32)
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(param) = self.params.get_mut(index) {
            *param = knob_value(value, RANGES[index]);
            self.derive();
        }
    }

    fn reset(&mut self) {
        self.voices.clear();
        self.clock = Clock::default();
    }

    /// The tab's release, and the last grain of a note, which is no longer
    /// than two attacks and a hold.
    fn tail(&self) -> usize {
        let grain = 2.0 * self.derived.attack + self.derived.hold;
        self.played.tail() + grain.ceil() as usize
    }

    fn fidelity(&self) -> Fidelity {
        if self.path.is_some() && (self.missing || self.wave.is_none()) {
            Fidelity::Approximate("Fruity Granulizer sample not available")
        } else if self.transients != 0 || self.trigger == 3 {
            Fidelity::Approximate("Fruity Granulizer transients")
        } else {
            Fidelity::Exact
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use std::f32::consts::FRAC_1_SQRT_2;

    fn with(len: usize, value: f32) -> Granulizer {
        let mut plugin = Granulizer::default();
        plugin.set_wave(Wave {
            left: vec![value; len],
            right: None,
            rate: 44_100,
        });
        plugin
    }

    fn play(plugin: &mut Granulizer, len: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut left, mut right) = (vec![0.0; len], vec![0.0; len]);
        plugin.process(&mut left, &mut right);
        (left, right)
    }

    #[test]
    fn the_wheels_are_milliseconds_and_256ths_of_a_grain() {
        assert_eq!([millis(50), millis(204), millis(500)], [5, 40, 500]);
        assert_eq!(millis(0), 0);
        assert_eq!(
            [spacing(611), spacing(-611), spacing(1000)],
            [256, -256, 768]
        );
        assert_eq!(spacing(125), 27);
        let plugin = Granulizer::default();
        let derived = plugin.derived;
        // FL's milliseconds are its rate times a single-precision 0.001,
        // so the hold is a hair over half a sample and rounds up.
        assert_eq!((derived.attack, derived.hold), (1764.0001, 220.50002));
        assert_eq!(derived.spacing.round_ties_even(), 1985.0);
        assert_eq!(derived.speed, (PI / 24.0) as f32);
    }

    #[test]
    fn the_defaults_overlap_the_grains_into_the_sample_at_its_own_speed() {
        let mut plugin = with(100_000, 1.0);
        plugin.note_on(NoteOn::default());
        let (left, right) = play(&mut plugin, 8000);
        let grain = FRAC_1_SQRT_2 * GRAIN_VOLUME * FRAC_1_SQRT_2;
        // The first grain is in full from the start, and holds to where
        // the second starts; the fades of two grains then add up to a
        // little over 1.
        for (at, sum) in [(0, 1.0), (1984, 1.0), (1985, 1.0 + 1.0 / 1764.0)] {
            let want = grain * sum;
            assert!((left[at] - want).abs() < 1e-5, "{at}: {} {want}", left[at]);
            assert!((right[at] - want).abs() < 1e-5, "{at}");
        }
        let later = grain * (1.0 + 2.0 / 1764.0);
        assert!((left[1985 * 2 + 10] - later).abs() < 1e-5, "{}", left[3980]);
        // A grain reads the sample where the note has got to, at its pitch.
        let mut plugin = Granulizer::default();
        let ramp: Vec<f32> = (0..20_000).map(|n| n as f32 / 20_000.0).collect();
        plugin.set_wave(Wave {
            left: ramp,
            right: None,
            rate: 44_100,
        });
        plugin.note_on(NoteOn::default());
        let (left, _) = play(&mut plugin, 1900);
        assert!((left[1000] / grain - 0.05).abs() < 1e-6);
    }

    #[test]
    fn a_sample_without_loop_ends_the_note_and_the_grains_pan_in_turn() {
        let mut plugin = with(5000, 0.5);
        plugin.set_param(7, 64.0);
        plugin.note_on(NoteOn::default());
        let (left, right) = play(&mut plugin, 20_000);
        // The first grain pans left, the second right.
        assert!(right[100].abs() < 1e-6 && left[100] > 0.4);
        assert!(right[3000] > left[3000]);
        assert!(left[15_000..].iter().all(|v| *v == 0.0));
        assert!(plugin.voices.is_empty());
        // With loop on it plays until it is let go of.
        let mut plugin = with(5000, 0.5);
        plugin.set_param(8, 1.0);
        plugin.note_on(NoteOn::default());
        let (left, _) = play(&mut plugin, 40_000);
        assert!(left[39_000] > 0.3);
        plugin.note_off(0);
        let tail = plugin.tail();
        play(&mut plugin, tail);
        assert!(plugin.voices.is_empty());
    }

    #[test]
    fn key_to_start_moves_the_note_s_place_and_keeps_its_pitch() {
        let mut plugin = with(24_001, 1.0);
        plugin.trigger = 1;
        let note = NoteOn {
            key: 66,
            fine_pitch: 30.0,
            ..NoteOn::default()
        };
        assert_eq!(plugin.start(note.key, 24_000), (6000.0, 600.0));
        plugin.trigger = 2;
        plugin.tempo = 120.0;
        // Six keys are a beat and a half, 0.75 s at 120.
        assert_eq!(plugin.start(note.key, 24_000), (33_075.0 % 24_001.0, 600.0));
        assert_eq!(plugin.start(50, 24_000), (0.0, -1000.0));
        plugin.trigger = 0;
        plugin.set_param(10, 2048.0);
        assert_eq!(plugin.start(note.key, 24_000), (12_000.0, 0.0));
    }

    #[test]
    fn a_state_holds_what_its_version_has() {
        let short = testing::state(&[1, 200, 50, 611, 611, 0, 180, 0, 0, 0, 0]);
        let mut plugin = Granulizer::default();
        plugin.set_state(&short).unwrap();
        assert_eq!(
            plugin.params,
            [200, 50, 611, 611, 0, 180, 0, 0, 0, 0, 0, 248]
        );
        let mut long = testing::state(&[7, 204, 50, 611, 611, 0, 180, 0, 0, 0, 0, 0, 248, 2, 1, 0]);
        long.extend([0; 364]);
        plugin.set_state(&long).unwrap();
        assert_eq!(
            (plugin.transients, plugin.trigger()),
            (2, Trigger::SampleTime)
        );
        assert_ne!(plugin.fidelity(), Fidelity::Exact);
        assert!(plugin.set_state(&testing::state(&[8; 15])).is_err());
        assert!(plugin.set_state(&[1, 0, 0, 0]).is_err());
    }

    #[test]
    fn every_preset_and_every_state_in_fl_s_projects_loads_and_plays() {
        let Some(root) = testing::corpus() else {
            return;
        };
        // An install that also has `Patches/Plugin database` has one more.
        let mut saved = testing::every_preset(&root, NAME);
        assert!((12..=13).contains(&saved.len()), "{}", saved.len());
        let ours = |saved: &testing::Saved| saved.plugin.eq_ignore_ascii_case(NAME);
        let projects = testing::projects(&root).into_iter().filter(ours);
        saved.extend(projects);
        for saved in saved {
            let mut plugin = with(44_100, 0.5);
            plugin.set_state(&saved.state).unwrap();
            plugin.set_wave(Wave {
                left: (0..44_100).map(|n| (n as f32 * 0.05).sin() * 0.5).collect(),
                right: None,
                rate: 44_100,
            });
            plugin.note_on(NoteOn::default());
            let (left, right) = play(&mut plugin, 44_100);
            let peak = left
                .iter()
                .chain(&right)
                .fold(0.0f32, |p, v| p.max(v.abs()));
            assert!((0.05..4.0).contains(&peak), "{peak} in {}", saved.file);
        }
    }
}
