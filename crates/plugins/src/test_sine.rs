//! TestSine: a sine for every note. FL has no such plugin. It is here for
//! the tests of a host, which can say to the sample what it puts out.
//!
//! A note sounds from its first frame at `velocity * level / 100`, panned
//! by [`dsp::pan_linear`], at 440 Hz for key 69 in equal temperament. Frame
//! `n` of it is `sin(2 pi * (f / rate) * n)`, worked out in f64 in that
//! order. From its note off on, every frame is [`TestSine::decay`] times
//! the one before, for [`RELEASE`] seconds, which takes it down by 60 dB.
//! Then it stops.
//!
//! The state is one little-endian i32, the plugin's one parameter:
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Level | 0 to 100 | 100 |
//!
//! A note's release, its two mods, its slide and its porta are not read. A
//! slide note sounds like any other.

use crate::{Generator, NoteOn, StateError, knob_value, read_state};
use std::f64::consts::TAU;

/// The name [`generator_by_name`](crate::generator_by_name) has it under.
pub const NAME: &str = "TestSine";

/// How long a note sounds on after its note off, in seconds.
pub const RELEASE: f64 = 0.05;

/// The notes it sounds at once. One more takes the place of the oldest.
pub const VOICES: usize = 32;

const RANGES: [(i32, i32); 1] = [(0, 100)];

#[derive(Debug, Clone, Copy, PartialEq)]
struct Voice {
    /// The host's number for the note.
    id: u32,
    /// Turns of the sine a frame.
    step: f64,
    /// Left and right gain: the note's velocity and its pan.
    gains: (f32, f32),
    /// Frames it has played.
    played: usize,
    /// What its release has brought it down to.
    level: f32,
    /// The frames of its release that are left, once it is let go.
    left: Option<usize>,
}

/// TestSine. A new one is at 48 kHz with its level all the way up.
#[derive(Debug, Clone, PartialEq)]
pub struct TestSine {
    level: i32,
    rate: f64,
    /// In the order they started, and never more than [`VOICES`].
    voices: Vec<Voice>,
}

impl Default for TestSine {
    fn default() -> Self {
        TestSine {
            level: 100,
            rate: 48_000.0,
            voices: Vec::with_capacity(VOICES),
        }
    }
}

impl TestSine {
    /// What one frame of a release is of the frame before it.
    pub fn decay(&self) -> f32 {
        0.001f64.powf(1.0 / self.tail() as f64) as f32
    }
}

impl Generator for TestSine {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        [self.level] = read_state(bytes, &RANGES)?;
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        self.rate = f64::from(hz);
    }

    fn note_on(&mut self, note: NoteOn) {
        if self.voices.len() == VOICES {
            self.voices.remove(0);
        }
        let semitones = f64::from(note.key) - 69.0 + f64::from(note.fine_pitch) / 100.0;
        let (left, right) = dsp::pan_linear(note.pan);
        self.voices.push(Voice {
            id: note.voice,
            step: 440.0 * 2f64.powf(semitones / 12.0) / self.rate,
            gains: (note.velocity * left, note.velocity * right),
            played: 0,
            level: 1.0,
            left: None,
        });
    }

    fn note_off(&mut self, voice: u32) {
        let tail = self.tail();
        let held = |v: &&mut Voice| v.id == voice && v.left.is_none();
        if let Some(voice) = self.voices.iter_mut().find(held) {
            voice.left = Some(tail);
        }
    }

    fn all_notes_off(&mut self) {
        let tail = self.tail();
        for voice in &mut self.voices {
            voice.left.get_or_insert(tail);
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let level = self.level as f32 / 100.0;
        let decay = self.decay();
        for voice in &mut self.voices {
            for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                if let Some(left) = &mut voice.left {
                    if *left == 0 {
                        break;
                    }
                    *left -= 1;
                    voice.level *= decay;
                }
                // A product and not a running sum, so a frame does not
                // depend on where the blocks fall.
                let sine = (TAU * voice.step * voice.played as f64).sin() as f32;
                let value = sine * voice.level * level;
                *l += value * voice.gains.0;
                *r += value * voice.gains.1;
                voice.played += 1;
            }
        }
        self.voices.retain(|voice| voice.left != Some(0));
    }

    fn param_count(&self) -> usize {
        RANGES.len()
    }

    fn param(&self, index: usize) -> f32 {
        if index == 0 { self.level as f32 } else { 0.0 }
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if index == 0 {
            self.level = knob_value(value, RANGES[0]);
        }
    }

    fn reset(&mut self) {
        self.voices.clear();
    }

    fn tail(&self) -> usize {
        (RELEASE * self.rate).round() as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::state;

    fn note(voice: u32, key: i16) -> NoteOn {
        NoteOn {
            voice,
            key,
            velocity: 1.0,
            pan: 0.0,
            fine_pitch: 0.0,
            release: 0.5,
            mod_x: 0.0,
            mod_y: 0.0,
            slide: false,
            porta: false,
        }
    }

    fn play(sine: &mut TestSine, len: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut left, mut right) = (vec![0.0; len], vec![0.0; len]);
        sine.process(&mut left, &mut right);
        (left, right)
    }

    /// Frame `n` of a note of this key at 48 kHz.
    fn frame(key: i16, n: usize) -> f32 {
        let step = 440.0 * 2f64.powf((f64::from(key) - 69.0) / 12.0) / 48_000.0;
        (TAU * step * n as f64).sin() as f32
    }

    #[test]
    fn a_note_is_a_sine_at_its_key_until_its_release_is_over() {
        let mut sine = TestSine::default();
        assert_eq!(sine.tail(), 2400);
        assert_eq!(play(&mut sine, 8).0, [0.0; 8]);
        sine.note_on(note(7, 69));
        let (left, right) = play(&mut sine, 480);
        assert_eq!(left, right);
        for (n, value) in left.iter().enumerate() {
            assert_eq!(*value, frame(69, n), "frame {n}");
        }
        // 440 Hz at 48 kHz is a quarter turn in 27.27 frames.
        assert!((left[27] - 1.0).abs() < 1e-3);

        sine.note_off(7);
        let (tail, _) = play(&mut sine, 2500);
        let mut level = 1.0;
        for (n, value) in tail[..2400].iter().enumerate() {
            level *= sine.decay();
            assert_eq!(*value, frame(69, 480 + n) * level, "frame {n}");
        }
        // 60 dB down, and then nothing.
        assert!((level - 0.001).abs() < 1e-5);
        assert_eq!(tail[2400..], [0.0; 100]);
        assert!(sine.voices.is_empty());
    }

    #[test]
    fn velocity_and_pan_set_the_two_gains_and_notes_add_up() {
        let mut sine = TestSine::default();
        sine.note_on(NoteOn {
            velocity: 0.5,
            pan: -1.0,
            ..note(1, 60)
        });
        let (left, right) = play(&mut sine, 100);
        assert_eq!(left[50], frame(60, 50) * 0.5);
        assert_eq!(right, [0.0; 100]);

        // A second note on top, 100 frames behind the first. A block adds
        // to what it is handed.
        sine.note_on(note(2, 72));
        let (mut left, mut right) = ([1.0; 4], [1.0; 4]);
        sine.process(&mut left, &mut right);
        for n in 0..4 {
            let first = frame(60, 100 + n) * 0.5;
            assert_eq!(left[n], 1.0 + first + frame(72, n));
            assert_eq!(right[n], 1.0 + frame(72, n));
        }
    }

    #[test]
    fn a_note_off_ends_the_note_with_its_number_and_no_other() {
        let mut sine = TestSine::default();
        sine.note_on(note(1, 60));
        sine.note_on(note(2, 60));
        sine.note_off(9);
        sine.note_off(1);
        // A second note off does not start the release over.
        play(&mut sine, 100);
        sine.note_off(1);
        play(&mut sine, 2300);
        assert_eq!(sine.voices.len(), 1);
        assert_eq!((sine.voices[0].id, sine.voices[0].left), (2, None));
        sine.all_notes_off();
        play(&mut sine, 2399);
        assert_eq!(sine.voices.len(), 1);
        play(&mut sine, 1);
        assert!(sine.voices.is_empty());

        sine.note_on(note(3, 60));
        sine.reset();
        assert_eq!(play(&mut sine, 4).0, [0.0; 4]);
    }

    #[test]
    fn the_blocks_can_fall_anywhere() {
        let whole = {
            let mut sine = TestSine::default();
            sine.note_on(note(1, 64));
            let mut out = play(&mut sine, 700).0;
            sine.note_off(1);
            out.extend(play(&mut sine, 3000).0);
            out
        };
        for block in [1, 64, 256, 1000] {
            let mut sine = TestSine::default();
            sine.note_on(note(1, 64));
            let mut out = Vec::new();
            while out.len() < 3700 {
                if out.len() == 700 {
                    sine.note_off(1);
                }
                let next = if out.len() < 700 { 700 } else { 3700 };
                out.extend(play(&mut sine, block.min(next - out.len())).0);
            }
            assert_eq!(out, whole, "blocks of {block}");
        }
    }

    #[test]
    fn the_level_is_the_state_and_the_one_parameter() {
        let mut sine = TestSine::default();
        assert_eq!(
            (sine.param_count(), sine.param(0), sine.param(1)),
            (1, 100.0, 0.0)
        );
        sine.set_state(&state(&[25])).unwrap();
        assert_eq!(sine.param(0), 25.0);
        sine.note_on(note(1, 69));
        assert_eq!(play(&mut sine, 28).0[27], frame(69, 27) * 0.25);
        assert_eq!(
            sine.set_state(&state(&[101])),
            Err(StateError::Range {
                param: 0,
                value: 101
            })
        );
        assert!(sine.set_state(&[]).is_err());
        assert_eq!(sine.param(0), 25.0);
        sine.set_param(0, 250.0);
        sine.set_param(1, 7.0);
        assert_eq!(sine.param(0), 100.0);
    }

    #[test]
    fn a_33rd_note_takes_the_place_of_the_first() {
        let mut sine = TestSine::default();
        let capacity = sine.voices.capacity();
        for voice in 0..=VOICES as u32 {
            sine.note_on(note(voice, 60));
        }
        assert_eq!(sine.voices.len(), VOICES);
        assert_eq!(sine.voices[0].id, 1);
        assert_eq!(sine.voices.capacity(), capacity);
        // The rate moves the pitch and the release with it.
        sine.set_sample_rate(44_100.0);
        assert_eq!(sine.tail(), 2205);
    }
}
