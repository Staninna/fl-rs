//! The block renderer: a clock, the voices that are sounding, and the mixer
//! they play into, which has the generators of the plugin channels in it.

use crate::graph::Graph;
use crate::mix::BLOCK;
use crate::voice::Voice;
use crate::{Approximate, Curves, Error, PreparedSamples, Ticks};
use model::Project;

/// The voices a realtime engine can sound at once.
pub(crate) const VOICES: usize = 256;

/// Plays a project block by block, with the [`Approximate`] curves. It
/// reads no files and talks to no device: [`Engine::process`] fills the
/// buffers it is handed, and [`render`](crate::render) and
/// [`Transport`](crate::Transport) are both built on it.
///
/// A new engine is stopped at the start of the song. It sounds up to 256
/// voices at once, where a note is one and an audio clip is one. A voice
/// that finds them all taken stops the one that started first. A note on a
/// plugin channel is no voice of the engine's: the channel's generator
/// keeps its own.
pub struct Engine {
    graph: Box<Graph>,
    /// The voices that are sounding, in the order they started.
    voices: Vec<Voice>,
    max_voices: usize,
    /// The next output sample, counted from the start of the song.
    position: usize,
    /// The first of the graph's events that has not started.
    next: usize,
    /// The first of the graph's cues that its generator has not heard.
    next_cue: usize,
    /// The first tick of the song that does not start before `position`:
    /// the next one the mixer's controller plugins are told of.
    next_tick: Ticks,
    playing: bool,
    /// The song has ended and its last voices and generator notes are
    /// playing out. The clock runs on under them until they are done, and
    /// reads as 0 meanwhile.
    ringing: bool,
    looping: bool,
    /// Whether the end of the song stops the clock or turns it back. A
    /// render plays past it, to the end of the last voice.
    ends: bool,
    mode: Mode,
    /// What [`Engine::new`] was given, to build the graph of another mode
    /// from. An engine made of a graph has none.
    source: Option<Source>,
}

/// What an engine plays: the playlist, or one pattern round and round.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Mode {
    pub song: bool,
    pub pattern: u16,
}

impl Default for Mode {
    fn default() -> Self {
        Mode {
            song: true,
            pattern: 1,
        }
    }
}

impl Mode {
    /// The pattern that plays alone, when one does.
    pub(crate) fn alone(self) -> Option<u16> {
        (!self.song).then_some(self.pattern)
    }
}

struct Source {
    project: Project,
    samples: PreparedSamples,
    sample_rate: u32,
}

/// The graph of a project for realtime playback: the whole song, with the
/// samples that are already decoded.
pub(crate) fn graph(
    project: &Project,
    samples: &PreparedSamples,
    sample_rate: u32,
) -> Result<Graph, Error> {
    graph_of(project, samples, sample_rate, Mode::default())
}

/// [`graph`] for a mode: the whole song, or the one pattern.
pub(crate) fn graph_of(
    project: &Project,
    samples: &PreparedSamples,
    sample_rate: u32,
    mode: Mode,
) -> Result<Graph, Error> {
    let sample = |path: &str| Ok(samples.get(path));
    match mode.alone() {
        Some(pattern) => {
            let dir = samples.plugin_dir();
            Graph::pattern(project, pattern, sample_rate, &Approximate, dir, sample)
        }
        None => {
            let dir = samples.plugin_dir();
            Graph::new(project, sample_rate, &Approximate, usize::MAX, dir, sample)
        }
    }
}

impl Engine {
    /// `samples` is what [`prepare`](crate::prepare) decoded for the
    /// project. A channel whose sample is not in it stays silent.
    pub fn new(
        project: &Project,
        samples: &PreparedSamples,
        sample_rate: u32,
    ) -> Result<Self, Error> {
        let mut engine = Self::with_graph(graph(project, samples, sample_rate)?, VOICES);
        engine.source = Some(Source {
            project: project.clone(),
            samples: samples.clone(),
            sample_rate,
        });
        Ok(engine)
    }

    pub(crate) fn with_graph(graph: Graph, max_voices: usize) -> Self {
        Engine {
            graph: Box::new(graph),
            voices: Vec::with_capacity(max_voices),
            max_voices,
            position: 0,
            next: 0,
            next_cue: 0,
            next_tick: 0,
            playing: false,
            ringing: false,
            looping: false,
            ends: true,
            mode: Mode::default(),
            source: None,
        }
    }

    /// Play past the end of the song, as a render does.
    pub(crate) fn run_past_end(&mut self) {
        self.ends = false;
    }

    /// Run the clock from where it is.
    pub fn play(&mut self) {
        self.settle();
        self.playing = true;
    }

    /// Hold the clock where it is and cut every voice and every note a
    /// generator is playing.
    pub fn stop(&mut self) {
        self.settle();
        self.playing = false;
        self.voices.clear();
        self.graph.mixer.cut();
    }

    /// Whether the clock runs. It stops by itself at the end of the song,
    /// unless the song loops.
    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// Whether the song starts over from tick 0 when it ends. Off in a new
    /// engine. A pattern on its own starts over either way.
    pub fn set_loop(&mut self, on: bool) {
        self.looping = on;
    }

    /// Play the playlist, which a new engine does, or with `false` the
    /// pattern [`Engine::set_pattern`] chose, round and round from its
    /// first tick, as FL's PAT mode does. The pattern is as long as its
    /// notes need, rounded up to a whole bar, and the playlist and its
    /// automation clips add nothing to it.
    ///
    /// The clock keeps its tick, and a tick past the end of the pattern
    /// starts the pattern over. The voices that are sounding play out.
    ///
    /// This builds a graph, so it allocates and is not for an audio
    /// callback. [`Transport`](crate::Transport) has its own.
    pub fn set_song_mode(&mut self, on: bool) -> Result<(), Error> {
        self.set_mode(Mode {
            song: on,
            ..self.mode
        })
    }

    /// The pattern that plays when song mode is off, by its index. A new
    /// engine has pattern 1. A pattern that is not in the project is a bar
    /// of silence. Like [`Engine::set_song_mode`], this builds a graph.
    pub fn set_pattern(&mut self, index: u16) -> Result<(), Error> {
        self.set_mode(Mode {
            pattern: index,
            ..self.mode
        })
    }

    fn set_mode(&mut self, mode: Mode) -> Result<(), Error> {
        if let Some(source) = &self.source
            && mode.alone() != self.mode.alone()
        {
            let graph = graph_of(&source.project, &source.samples, source.sample_rate, mode)?;
            self.swap(Box::new(graph));
        }
        self.mode = mode;
        Ok(())
    }

    /// Put the clock back at the start once the song has ended.
    fn settle(&mut self) {
        if self.ringing {
            self.ringing = false;
            self.rewind();
        }
    }

    /// Put the clock at the start of the song.
    fn rewind(&mut self) {
        self.position = 0;
        self.next = 0;
        self.next_cue = 0;
        self.next_tick = 0;
    }

    /// Move the clock and cut every voice and every note a generator is
    /// playing. A note or a clip that started before `ticks` is not picked
    /// up part-way.
    pub fn seek(&mut self, ticks: Ticks) {
        self.ringing = false;
        self.position = self.graph.clock.sample(ticks);
        self.voices.clear();
        self.graph.mixer.cut();
        self.find_next();
    }

    /// Where the clock is.
    pub fn position(&self) -> Ticks {
        if self.ringing {
            return 0;
        }
        self.graph.clock.tick(self.position)
    }

    /// Hand `each` the loudest sample every insert has put out since the
    /// last call, on the left and on the right, with the loader's number
    /// of the insert, and start again from silence. 1.0 is full scale.
    ///
    /// An insert's output is its bus behind its effects, its volume and
    /// its pan, and the master's is in front of the main volume. Only the
    /// inserts that sound can reach are handed over, and one that is
    /// switched off puts out nothing. Like [`Engine::process`], this is
    /// for an audio callback.
    pub fn insert_peaks(&mut self, each: impl FnMut(u16, (f32, f32))) {
        self.graph.mixer.take_peaks(each);
    }

    /// Play another graph from the tick the clock is at. The voices that
    /// are sounding go on as they are, except those whose insert the new
    /// graph has no bus for. The notes a generator is playing end here:
    /// the generators belong to their graph, and the new one's start
    /// silent. The old graph comes back so that the caller can free it
    /// where freeing is allowed.
    pub(crate) fn swap(&mut self, mut graph: Box<Graph>) -> Box<Graph> {
        self.settle();
        std::mem::swap(&mut self.graph, &mut graph);
        if self.graph.clock != graph.clock {
            self.position = self.graph.clock.sample(graph.clock.tick(self.position));
        }
        let mixer = &self.graph.mixer;
        self.voices.retain(|voice| mixer.plays(voice.insert));
        self.find_next();
        graph
    }

    fn find_next(&mut self) {
        let events = &self.graph.events;
        self.next = events.partition_point(|event| event.start < self.position);
        let cues = &self.graph.cues;
        self.next_cue = cues.partition_point(|cue| cue.at < self.position);
        // The tick the position is in starts here or has started. One
        // that has started is not told again.
        let clock = &self.graph.clock;
        let mut tick = clock.tick_at(self.position) as Ticks;
        while tick > 0 && clock.sample(tick - 1) >= self.position {
            tick -= 1;
        }
        while clock.sample(tick) < self.position {
            tick += 1;
        }
        self.next_tick = tick;
    }

    /// Fill both buffers with what comes next and move the clock on by
    /// their length. When the two differ in length the shorter one counts,
    /// and the rest of the longer one is silent. A stopped engine writes
    /// silence.
    ///
    /// This allocates nothing, takes no lock and makes no system call, so
    /// an audio callback can run it.
    ///
    /// The song ends where its last playlist item does, which is what
    /// [`song_length`](crate::song_length) says. There the clock stops and
    /// goes back to tick 0, and the voices that are sounding play out. With
    /// [`Engine::set_loop`] on, and for a pattern on its own, the clock
    /// goes back to tick 0 and runs on. Either way the generators are told
    /// to let go of the notes they hold, and play their tails.
    ///
    /// Only a clock that plays up to the end stops there. One that a seek
    /// put past the end runs on in silence, unless the song loops. So does
    /// the clock of a song with nothing in it, which has no end.
    pub fn process(&mut self, out_left: &mut [f32], out_right: &mut [f32]) {
        self.process_with(&Approximate, out_left, out_right);
    }

    /// How long each generator and effect slot took in the last
    /// [`Engine::process`], a part once for every block it played in it.
    #[cfg(feature = "profile")]
    pub fn spent(&self) -> &[(crate::Part, std::time::Duration)] {
        &self.graph.mixer.spent.parts
    }

    /// [`Engine::process`] with the curves the graph was built with.
    pub(crate) fn process_with(
        &mut self,
        curves: &dyn Curves,
        out_left: &mut [f32],
        out_right: &mut [f32],
    ) {
        let len = out_left.len().min(out_right.len());
        out_left[len..].fill(0.0);
        out_right[len..].fill(0.0);
        self.graph.mixer.spent.clear();
        let mut at = 0;
        while at < len {
            let end = self.graph.end;
            if self.playing && self.ends && end > 0 && self.position >= end {
                if self.looping || self.graph.pattern {
                    self.rewind();
                    self.graph.mixer.release();
                } else if self.position == end {
                    self.playing = false;
                    self.ringing = true;
                    self.graph.mixer.release();
                }
            }
            // Past the end of the song the clock runs on under the voices
            // and the generator tails that are left, so they come out as a
            // render has them.
            let sounding = !self.voices.is_empty() || self.graph.mixer.ringing();
            if !self.playing && !(self.ringing && sounding) {
                self.settle();
                out_left[at..len].fill(0.0);
                out_right[at..len].fill(0.0);
                return;
            }
            let mut count = (len - at).min(BLOCK);
            if self.playing && self.ends && self.position < end {
                count = count.min(end - self.position);
            }
            // Every note and clip due now starts a voice. The block then
            // ends where the next one starts, so a voice always starts on
            // the first sample of a block.
            while let Some(event) = self.graph.events.get(self.next) {
                if !self.playing {
                    break;
                }
                if event.start > self.position {
                    count = count.min(event.start - self.position);
                    break;
                }
                self.next += 1;
                if event.voice.finished() || self.max_voices == 0 {
                    continue;
                }
                if self.voices.len() == self.max_voices {
                    self.voices.remove(0);
                }
                self.voices.push(event.voice.clone());
            }
            // Every note on and note off due now goes to its generator,
            // and the block ends where the next one is due, so a generator
            // hears of a note on the first sample of a block.
            let graph = &mut *self.graph;
            while let Some(cue) = graph.cues.get(self.next_cue) {
                if !self.playing {
                    break;
                }
                if cue.at > self.position {
                    count = count.min(cue.at - self.position);
                    break;
                }
                self.next_cue += 1;
                graph.mixer.cue(cue);
            }
            // The automated controls are read on every `BLOCK`th sample
            // of the song and where a note or a clip starts, so a block
            // ends where the next of those is. What a control is at a
            // sample then does not depend on how the caller cuts its
            // buffers.
            let cell = self.position - self.position % BLOCK;
            count = count.min(cell + BLOCK - self.position);
            let started = self
                .next
                .checked_sub(1)
                .map(|last| graph.events[last].start);
            let read_at = started.map_or(cell, |started| started.max(cell));
            let automation = &mut graph.automation;
            automation.apply(read_at, &graph.clock, &mut graph.mixer, curves);
            // A controller plugin works its outputs out as a tick starts,
            // from what it heard up to there, and the controls they drive
            // have them from the tick's first sample on. So with one in the
            // mixer a block ends where the next tick starts. FL calls its
            // plugin before it mixes a tick, which is the same order.
            if graph.mixer.ticking() {
                while graph.clock.sample(self.next_tick) <= self.position {
                    let outputs = |insert, slot, outputs: &[f32]| {
                        automation.drive(insert, slot, outputs);
                    };
                    graph.mixer.tick(self.next_tick, outputs);
                    self.next_tick += 1;
                }
                automation.carry(&mut graph.mixer, curves);
                count = count.min(graph.clock.sample(self.next_tick) - self.position);
            }
            automation.follow(&mut self.voices, curves);
            let block = at..at + count;
            let (left, right) = (&mut out_left[block.clone()], &mut out_right[block]);
            self.graph.mixer.process(&mut self.voices, left, right);
            self.voices.retain(|voice| !voice.finished());
            self.position += count;
            at += count;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::Sample;
    use model::{Arrangement, Channel, ChannelKind, ItemSource, Note, Pattern, PlaylistItem};
    use std::sync::Arc;

    const RATE: u32 = 48_000;

    /// A project at 120 BPM and PPQ 96, so a tick is 250 samples, with one
    /// sampler channel and one pattern that holds a note on each of `ticks`.
    fn project(ticks: &[i32]) -> Project {
        let mut project = Project::new(flp::Header {
            format: 0,
            channel_count: 1,
            ppq: 96,
        });
        project.tempo_milli = Some(120_000);
        let note = |position: &i32| Note {
            position: *position,
            key: 60,
            ..Note::default()
        };
        project.patterns.push(Pattern {
            index: 1,
            notes: ticks.iter().map(note).collect(),
            ..Pattern::default()
        });
        project.channels.push(Channel {
            index: 0,
            kind: Some(ChannelKind::Sampler),
            sample_path: Some("sample".to_owned()),
            ..Channel::default()
        });
        project.arrangements.push(Arrangement {
            items: vec![PlaylistItem::new(ItemSource::Pattern(1), 0, 384)],
            ..Arrangement::default()
        });
        project
    }

    /// The graph of a project whose channel plays `frames`.
    fn graph(project: &Project, frames: &[f32]) -> Graph {
        let sample = Arc::new(Sample {
            left: frames.to_vec(),
            right: None,
            rate: RATE,
        });
        let found = |_: &str| Ok(Some(Arc::clone(&sample)));
        Graph::new(project, RATE, &Approximate, usize::MAX, None, found).unwrap()
    }

    fn play(engine: &mut Engine, len: usize) -> Vec<f32> {
        let (mut left, mut right) = (vec![9.0; len], vec![9.0; len]);
        engine.process(&mut left, &mut right);
        assert_eq!(left, right);
        left
    }

    #[test]
    fn a_stopped_engine_is_silent_and_stays_where_it_is() {
        let mut engine = Engine::with_graph(graph(&project(&[0, 2]), &[1.0]), 8);
        assert_eq!(play(&mut engine, 300), [0.0; 300]);
        assert_eq!(engine.position(), 0);
        engine.play();
        let out = play(&mut engine, 750);
        assert_eq!((out[0], out[500]), (1.0, 1.0));
        assert_eq!(out.iter().filter(|v| **v != 0.0).count(), 2);
        assert_eq!(engine.position(), 3);
    }

    #[test]
    fn stop_cuts_the_voices_and_play_goes_on_from_there() {
        let mut engine = Engine::with_graph(graph(&project(&[0, 2]), &[1.0; 400]), 8);
        engine.play();
        assert_eq!(play(&mut engine, 100), [1.0; 100]);
        engine.stop();
        assert_eq!(play(&mut engine, 100), [0.0; 100]);
        engine.play();
        // The first note's other 300 samples are gone. The second comes
        // 400 samples on, at tick 2.
        let out = play(&mut engine, 500);
        assert_eq!(out[..400], [0.0; 400]);
        assert_eq!(out[400..], [1.0; 100]);
    }

    #[test]
    fn seek_starts_at_the_notes_from_that_tick_on() {
        let mut engine = Engine::with_graph(graph(&project(&[0, 2, 4]), &[1.0; 100]), 8);
        engine.play();
        play(&mut engine, 50);
        engine.seek(2);
        assert_eq!(engine.position(), 2);
        let out = play(&mut engine, 600);
        assert_eq!(out[..100], [1.0; 100]);
        assert_eq!(out[100..500], [0.0; 400]);
        assert_eq!(out[500..], [1.0; 100]);
        // Past the last note there is nothing, and the clock runs on.
        engine.seek(5);
        assert_eq!(play(&mut engine, 500), [0.0; 500]);
        assert_eq!(engine.position(), 7);
    }

    #[test]
    fn a_note_with_no_free_voice_takes_the_oldest() {
        // Three notes a tick apart, each 1,000 samples of its own level.
        let mut graph = graph(&project(&[0, 1, 2]), &[1.0; 1000]);
        for (event, level) in graph.events.iter_mut().zip([1.0, 10.0, 100.0]) {
            let sample = Arc::new(Sample {
                left: vec![level; 1000],
                right: None,
                rate: RATE,
            });
            event.voice = Voice::new(sample, 0, None, 1.0, (1.0, 1.0));
        }
        let mut engine = Engine::with_graph(graph, 2);
        engine.play();
        let capacity = engine.voices.capacity();
        let out = play(&mut engine, 1600);
        assert_eq!((out[0], out[250], out[499]), (1.0, 11.0, 11.0));
        // The third note takes the first one's place.
        assert_eq!((out[500], out[1249], out[1250]), (110.0, 110.0, 100.0));
        assert_eq!((out[1499], out[1500]), (100.0, 0.0));
        assert_eq!(engine.voices.capacity(), capacity);
    }

    #[test]
    fn a_swap_keeps_the_voices_and_plays_the_new_notes() {
        let mut engine = Engine::with_graph(graph(&project(&[0, 2]), &[1.0; 400]), 8);
        engine.play();
        play(&mut engine, 100);
        // The new project has its second note a tick later and half as
        // loud. The note that is sounding plays out as it started.
        let mut edited = project(&[0, 3]);
        edited.patterns[0].notes[1].velocity = 50;
        let old = engine.swap(Box::new(graph(&edited, &[1.0; 400])));
        assert_eq!(old.events[1].start, 500);
        let out = play(&mut engine, 800);
        assert_eq!(out[..300], [1.0; 300]);
        assert_eq!(out[300..650], [0.0; 350]);
        assert_eq!(out[650..], [0.5; 150]);
    }

    #[test]
    fn a_swap_to_another_tempo_keeps_the_tick() {
        let mut engine = Engine::with_graph(graph(&project(&[0, 4]), &[1.0]), 8);
        engine.play();
        play(&mut engine, 500);
        assert_eq!(engine.position(), 2);
        // At 60 BPM a tick is 500 samples, so tick 4 is 1,000 samples on.
        let mut slow = project(&[0, 4]);
        slow.tempo_milli = Some(60_000);
        engine.swap(Box::new(graph(&slow, &[1.0])));
        assert_eq!(engine.position(), 2);
        let out = play(&mut engine, 1500);
        assert_eq!(out.iter().position(|v| *v != 0.0), Some(1000));
    }

    #[test]
    fn a_swap_cuts_a_voice_whose_insert_is_gone() {
        let mut engine = Engine::with_graph(graph(&project(&[0]), &[1.0; 400]), 8);
        engine.play();
        play(&mut engine, 100);
        // No notes, so no voice plays into the master and it has no bus.
        engine.swap(Box::new(graph(&project(&[]), &[1.0; 400])));
        assert!(engine.voices.is_empty());
        assert_eq!(play(&mut engine, 100), [0.0; 100]);
    }

    #[test]
    fn the_last_voices_play_out_past_the_end_of_the_song() {
        // The song is 384 ticks, 96,000 samples. The note on its last tick
        // starts at 95,750 and plays 400 samples.
        let mut engine = Engine::with_graph(graph(&project(&[383]), &[1.0; 400]), 8);
        engine.play();
        let out = play(&mut engine, 96_100);
        assert_eq!(out[..95_750], [0.0; 95_750]);
        assert_eq!(out[95_750..], [1.0; 350]);
        assert!(!engine.is_playing());
        assert_eq!(engine.position(), 0);
        let out = play(&mut engine, 200);
        assert_eq!(out[..50], [1.0; 50]);
        assert_eq!(out[50..], [0.0; 150]);
        assert_eq!(engine.position(), 0);

        // Stop cuts them.
        engine.play();
        play(&mut engine, 96_100);
        engine.stop();
        assert_eq!(play(&mut engine, 100), [0.0; 100]);
        // A clock that a seek put past the end runs on, and one that loops
        // goes back to the start.
        engine.seek(500);
        engine.play();
        assert_eq!(play(&mut engine, 250), [0.0; 250]);
        assert_eq!((engine.is_playing(), engine.position()), (true, 501));
        engine.set_loop(true);
        play(&mut engine, 250);
        assert_eq!(engine.position(), 1);
    }

    #[test]
    fn a_song_with_nothing_in_it_has_no_end_to_stop_at() {
        let mut empty = project(&[]);
        empty.arrangements.clear();
        assert_eq!(crate::song_length(&empty, RATE), 0);
        let mut engine = Engine::with_graph(graph(&empty, &[1.0]), 8);
        engine.set_loop(true);
        engine.play();
        assert_eq!(play(&mut engine, 500), [0.0; 500]);
        assert_eq!((engine.is_playing(), engine.position()), (true, 2));
    }

    #[test]
    fn a_song_that_loops_starts_over_on_the_sample_it_ends() {
        let mut engine = Engine::with_graph(graph(&project(&[0, 383]), &[1.0; 400]), 8);
        engine.set_loop(true);
        engine.play();
        let out = play(&mut engine, 2 * 96_000 + 100);
        // The first note again, on top of the last one's 150 samples left.
        let sounding = |at: usize| (out[at - 1], out[at]);
        assert_eq!((sounding(400), sounding(95_750)), ((1.0, 0.0), (0.0, 1.0)));
        assert_eq!(
            (sounding(96_000), sounding(96_150)),
            ((1.0, 2.0), (2.0, 1.0))
        );
        assert_eq!(
            (sounding(96_400), sounding(192_000)),
            ((1.0, 0.0), (1.0, 2.0))
        );
        assert!(engine.is_playing());
        assert_eq!(engine.position(), 0);
        // With the loop off again the song plays to its end and stops.
        engine.set_loop(false);
        play(&mut engine, 96_000);
        assert!(!engine.is_playing());
    }

    #[test]
    fn buffers_of_two_lengths_play_the_shorter_one() {
        let mut engine = Engine::with_graph(graph(&project(&[0]), &[1.0; 400]), 8);
        engine.play();
        let (mut left, mut right) = ([9.0; 100], [9.0; 60]);
        engine.process(&mut left, &mut right);
        assert_eq!(left[..60], [1.0; 60]);
        assert_eq!(left[60..], [0.0; 40]);
        assert_eq!(right, [1.0; 60]);
        assert_eq!(play(&mut engine, 2)[0], 1.0);
    }
}
