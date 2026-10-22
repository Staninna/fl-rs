//! Notes played live on one channel, beside the song: what a piano roll's
//! keyboard plays while a key is held.
//!
//! [`Live::new`] runs on the caller's thread. It builds the graph of a
//! pattern that has a note on every key of the channel, one tick apart,
//! and takes the voices and the generator's note ons out of it, one for
//! each key. The graph keeps the channel's generator and the inserts the
//! channel plays into, with their effects. On the audio thread a note on
//! starts the key's voice, or hands its generator the key's note on, and a
//! note off ends it. Neither allocates: a voice's clone shares its sample.

use crate::engine::Engine;
use crate::generator::Signal;
use crate::graph::Graph;
use crate::sample::Sample;
use crate::voice::Voice;
use crate::{Approximate, Error};
use model::{Note, Pattern, Project};
use std::path::Path;
use std::sync::Arc;

/// The keys a note can have.
const KEYS: usize = Pattern::TOP_KEY as usize + 1;
/// The voices of a sampler channel that sound at once.
const VOICES: usize = 16;

/// What a note on of a key starts.
enum Start {
    Voice(Box<Voice>),
    /// The generator at this place of the graph, and the note on.
    Cue(usize, Signal),
}

/// One channel, played live.
pub(crate) struct Live {
    engine: Engine,
    keys: Box<[Option<Start>]>,
    /// The number of the generator note each key has sounding.
    held: Box<[Option<u32>]>,
    /// The number the next generator note gets, past the ones the graph
    /// gave the notes it was built from.
    next: u32,
}

impl Live {
    /// The channel with this index of `project`, at `sample_rate`. `sample`
    /// and `plugin_dir` are what [`Graph::keys`] takes. A channel that
    /// plays nothing, or is not there, makes a `Live` that stays silent.
    pub fn new(
        project: &Project,
        channel: u16,
        sample_rate: u32,
        plugin_dir: Option<&Path>,
        sample: impl FnMut(&str) -> Result<Option<Arc<Sample>>, Error>,
    ) -> Result<Self, Error> {
        let index = project.patterns.iter().map(|p| p.index).max().unwrap_or(0);
        let index = index.saturating_add(1);
        let notes = (0..KEYS).map(|key| Note {
            position: key as i32,
            channel,
            key: key as i16,
            ..Note::default()
        });
        let mut alone = project.clone();
        alone.patterns = vec![Pattern {
            index,
            notes: notes.collect(),
            ..Pattern::default()
        }];
        let graph = Graph::keys(&alone, index, sample_rate, &Approximate, plugin_dir, sample)?;
        Ok(Self::of(graph))
    }

    /// Take the notes out of a graph built as [`Live::new`] builds one: the
    /// key of each is the tick it starts on.
    fn of(mut graph: Graph) -> Self {
        let mut keys: Box<[Option<Start>]> = (0..KEYS).map(|_| None).collect();
        let starts: Vec<usize> = (0..KEYS)
            .map(|key| graph.clock.sample(key as u64))
            .collect();
        let key_at = |at: usize| starts.iter().position(|start| *start == at);
        let mut next = 0;
        for event in std::mem::take(&mut graph.events) {
            if let Some(key) = key_at(event.start) {
                keys[key] = Some(Start::Voice(Box::new(event.voice)));
            }
        }
        for cue in std::mem::take(&mut graph.cues) {
            if let Signal::On(note, _) = cue.signal {
                next = next.max(note.voice + 1);
                if let Some(key) = key_at(cue.at) {
                    keys[key] = Some(Start::Cue(cue.generator, cue.signal));
                }
            }
        }
        let mut engine = Engine::with_graph(graph, VOICES);
        engine.run_past_end();
        engine.play();
        Live {
            engine,
            keys,
            held: vec![None; KEYS].into(),
            next,
        }
    }

    /// Start the key, at `gain` times the level of a note of velocity 100.
    /// A key that sounds already starts again.
    pub fn note_on(&mut self, key: u8, gain: f32) {
        self.note_off(key);
        let key = usize::from(key);
        match self.keys.get(key) {
            Some(Some(Start::Voice(voice))) => {
                let mut voice = Voice::clone(voice);
                let (left, right) = voice.gains();
                voice.set_gains((left * gain, right * gain));
                self.engine.start_voice(voice);
            }
            Some(Some(Start::Cue(generator, Signal::On(note, shift)))) => {
                let mut note = *note;
                note.voice = self.next;
                note.velocity *= gain;
                self.next = self.next.wrapping_add(1);
                self.held[key] = Some(note.voice);
                self.engine.signal(*generator, Signal::On(note, *shift));
            }
            _ => {}
        }
    }

    /// End the key. A sampler channel's voices all end here, since only
    /// one key is held at a time.
    pub fn note_off(&mut self, key: u8) {
        let key = usize::from(key);
        match self.keys.get(key) {
            Some(Some(Start::Voice(_))) => self.engine.end_voices(),
            Some(Some(Start::Cue(generator, _))) => {
                if let Some(voice) = self.held[key].take() {
                    self.engine.signal(*generator, Signal::Off(voice));
                }
            }
            _ => {}
        }
    }

    /// Cut every note where it is.
    pub fn stop(&mut self) {
        self.held.iter_mut().for_each(|held| *held = None);
        self.engine.stop();
        self.engine.play();
    }

    /// Whether a note is held or anything may still sound.
    pub fn sounding(&self) -> bool {
        self.held.iter().any(Option::is_some) || self.engine.sounding()
    }

    /// Add what the channel plays next to both buffers, which are as long
    /// as each other and no longer than a block.
    pub fn add(&mut self, left: &mut [f32], right: &mut [f32], spare: (&mut [f32], &mut [f32])) {
        let (own_left, own_right) = spare;
        let len = left.len();
        let (own_left, own_right) = (&mut own_left[..len], &mut own_right[..len]);
        self.engine.process(own_left, own_right);
        for (out, own) in left.iter_mut().zip(own_left.iter()) {
            *out += own;
        }
        for (out, own) in right.iter_mut().zip(own_right.iter()) {
            *out += own;
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use model::{Channel, ChannelKind};

    const RATE: u32 = 48_000;

    /// A sampler channel at 120 BPM and PPQ 96 that plays 100 samples of
    /// 0.5, and a pattern of its own the live notes leave out.
    pub(crate) fn live() -> Live {
        let channel = Channel {
            index: 0,
            kind: Some(ChannelKind::Sampler),
            sample_path: Some("sample".to_owned()),
            ..Channel::default()
        };
        live_with(channel, 100)
    }

    /// [`live`] with this channel, playing `frames` samples of 0.5.
    fn live_with(channel: Channel, frames: usize) -> Live {
        let mut project = Project::new(flp::Header {
            format: 0,
            channel_count: 1,
            ppq: 96,
        });
        project.tempo_milli = Some(120_000);
        project.channels.push(channel);
        project.patterns.push(Pattern {
            index: 1,
            notes: vec![Note::default()],
            ..Pattern::default()
        });
        let sample = Arc::new(Sample {
            left: vec![0.5; frames],
            right: None,
            rate: RATE,
        });
        let found = |_: &str| Ok(Some(Arc::clone(&sample)));
        Live::new(&project, 0, RATE, None, found).unwrap()
    }

    fn play(live: &mut Live, len: usize) -> Vec<f32> {
        let (mut left, mut right) = (vec![0.0; len], vec![0.0; len]);
        let (mut a, mut b) = (vec![0.0; len], vec![0.0; len]);
        live.add(&mut left, &mut right, (&mut a, &mut b));
        assert_eq!(left, right);
        left
    }

    #[test]
    fn a_key_plays_until_it_is_let_go() {
        let mut live = live();
        assert!(!live.sounding());
        assert_eq!(play(&mut live, 200), [0.0; 200]);
        live.note_on(60, 1.0);
        assert!(live.sounding());
        let out = play(&mut live, 50);
        // The sample at its own pitch, at the channel's level.
        assert!(out.iter().all(|v| *v > 0.0), "{out:?}");
        let level = out[0];
        live.note_off(60);
        assert_eq!(play(&mut live, 50), [0.0; 50]);
        assert!(!live.sounding());

        // An octave up is twice as fast, and half the velocity half as
        // loud.
        live.note_on(72, 0.5);
        let out = play(&mut live, 100);
        assert_eq!(out.iter().filter(|v| **v != 0.0).count(), 50);
        assert!((out[0] - level / 2.0).abs() < 1e-6);
        // A key the channel has not got plays nothing.
        live.note_on(200, 1.0);
        assert_eq!(play(&mut live, 10), [0.0; 10]);
    }

    #[test]
    fn a_key_plays_as_it_did_before_the_arpeggiator_whatever_it_is_set_to() {
        // An arpeggiator down over two octaves, which would start an
        // octave up and let go of a step sequencer note after a tick.
        let mut bytes = [0u8; 168];
        for (at, value) in [(40, 2), (44, 2), (48, -1), (52, 1024), (56, 48), (92, 1)] {
            bytes[at..at + 4].copy_from_slice(&i32::to_le_bytes(value));
        }
        let channel = |parameters| Channel {
            index: 0,
            kind: Some(ChannelKind::Sampler),
            sample_path: Some("sample".to_owned()),
            parameters,
            ..Channel::default()
        };
        let arpeggiated = channel(flp::event::layout::Parameters::from_bytes(&bytes));
        let (mut plain, mut arpeggiated) =
            (live_with(channel(None), 1000), live_with(arpeggiated, 1000));
        plain.note_on(60, 1.0);
        arpeggiated.note_on(60, 1.0);
        // A tick is 250 samples. The key sounds on past it, at its own
        // pitch, through the whole sample.
        let out = play(&mut arpeggiated, 1200);
        assert_eq!(out, play(&mut plain, 1200));
        assert_eq!(out.iter().filter(|v| **v != 0.0).count(), 1000);
    }
}
