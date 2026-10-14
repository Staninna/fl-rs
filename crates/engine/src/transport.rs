//! Realtime playback: an [`Engine`] inside a cpal output stream.
//!
//! Two threads share the work. The caller's thread owns the [`Transport`].
//! It reads the project, builds the graph the engine plays, and pushes
//! commands into a ring. The audio thread owns the [`Player`], and with it
//! the engine. At the top of each block it takes the commands that are
//! waiting, then it fills the block and stores the position and the peaks
//! in atomics for the caller to read.
//!
//! The audio thread never allocates, frees, locks or waits. A graph it is
//! done with goes back through a second ring, and the caller's thread frees
//! it.

use crate::engine::{self, Engine, Mode, VOICES};
use crate::graph::Graph;
use crate::mix::BLOCK;
use crate::{Error, PreparedSamples, Ticks};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, I24, Sample, SampleFormat, SizedSample};
use cpal::{Stream, StreamConfig, SupportedStreamConfig};
use model::Project;
use model::rec::LAST_INSERT;
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// The commands that can wait for the audio thread, and the graphs that can
/// wait to be freed.
const COMMANDS: usize = 64;

enum Command {
    Play,
    Stop,
    Seek(Ticks),
    Loop(bool),
    Swap(Box<Graph>),
}

/// What the audio thread tells the caller's.
#[derive(Default)]
struct Shared {
    position: AtomicU64,
    playing: AtomicBool,
    /// The bits of an `f32` that is not negative. Such bits sort the way
    /// the numbers do, so `fetch_max` keeps the larger number.
    peak_left: AtomicU32,
    peak_right: AtomicU32,
    insert_peaks: InsertPeaks,
    /// Errors the stream reported.
    faults: AtomicU32,
}

/// The peak of every insert on the left and on the right, by the loader's
/// number of the insert, kept as [`Shared::peak_left`] is.
struct InsertPeaks(Box<[[AtomicU32; 2]]>);

impl Default for InsertPeaks {
    fn default() -> Self {
        InsertPeaks((0..=LAST_INSERT).map(|_| Default::default()).collect())
    }
}

/// The audio thread's half.
struct Player {
    engine: Engine,
    commands: Consumer<Command>,
    retired: Producer<Box<Graph>>,
    shared: Arc<Shared>,
    left: [f32; BLOCK],
    right: [f32; BLOCK],
}

impl Player {
    /// Fill a buffer of interleaved frames. The first two channels of a
    /// frame get the left and the right side and any others stay silent. A
    /// device with one channel gets the mean of the two sides.
    fn fill<T: Sample + FromSample<f32>>(&mut self, data: &mut [T], channels: usize) {
        let channels = channels.max(1);
        for frames in data.chunks_mut(channels * BLOCK) {
            let count = frames.len() / channels;
            self.block(count);
            let sides = self.left.iter().zip(&self.right);
            for (frame, (left, right)) in frames.chunks_exact_mut(channels).zip(sides) {
                frame.fill(T::EQUILIBRIUM);
                match frame {
                    [mono] => *mono = T::from_sample((left + right) * 0.5),
                    [first, second, ..] => {
                        *first = T::from_sample(*left);
                        *second = T::from_sample(*right);
                    }
                    [] => {}
                }
            }
        }
    }

    /// Take the waiting commands and play `count` samples into `left` and
    /// `right`.
    fn block(&mut self, count: usize) {
        // A swap hands the old graph back, so commands wait while there is
        // no room for one.
        while !self.retired.is_full() {
            match self.commands.pop() {
                Ok(Command::Play) => {
                    self.engine.play();
                    self.shared.playing.store(true, Ordering::Relaxed);
                }
                Ok(Command::Stop) => {
                    self.engine.stop();
                    self.shared.playing.store(false, Ordering::Relaxed);
                }
                Ok(Command::Seek(ticks)) => self.engine.seek(ticks),
                Ok(Command::Loop(on)) => self.engine.set_loop(on),
                Ok(Command::Swap(graph)) => {
                    // This thread is the only one that pushes, and the ring
                    // had room a moment ago.
                    let _ = self.retired.push(self.engine.swap(graph));
                }
                Err(_) => break,
            }
        }
        let (left, right) = (&mut self.left[..count], &mut self.right[..count]);
        let playing = self.engine.is_playing();
        self.engine.process(left, right);
        // Only the end of the song is told here. A caller that has just
        // asked for play would see any other store take it back.
        if playing && !self.engine.is_playing() {
            self.shared.playing.store(false, Ordering::Relaxed);
        }
        let position = self.engine.position();
        self.shared.position.store(position, Ordering::Relaxed);
        raise(&self.shared.peak_left, left);
        raise(&self.shared.peak_right, right);
        let peaks = &self.shared.insert_peaks.0;
        self.engine.insert_peaks(|insert, (left, right)| {
            if let Some([peak_left, peak_right]) = peaks.get(usize::from(insert)) {
                peak_left.fetch_max(left.to_bits(), Ordering::Relaxed);
                peak_right.fetch_max(right.to_bits(), Ordering::Relaxed);
            }
        });
    }
}

/// Lift a peak to the loudest sample of a block, when that is louder.
fn raise(peak: &AtomicU32, block: &[f32]) {
    let loudest = block.iter().fold(0f32, |max, v| max.max(v.abs()));
    peak.fetch_max(loudest.to_bits(), Ordering::Relaxed);
}

/// Plays a project on the default output device.
///
/// The stream runs from [`Transport::open`] until the transport is dropped,
/// and plays silence while the transport is stopped. Every command is
/// taken at the start of the next block, a few milliseconds later.
pub struct Transport {
    // Dropped first: on ALSA that joins the audio thread.
    _stream: Stream,
    commands: Producer<Command>,
    retired: Consumer<Box<Graph>>,
    shared: Arc<Shared>,
    project: Arc<Project>,
    samples: PreparedSamples,
    sample_rate: u32,
    mode: Mode,
}

impl Transport {
    /// Open a stream on the default output device, stopped at the start of
    /// `project`. `samples` is what [`prepare`](crate::prepare) decoded for
    /// it, so no sample file is read here. A generator that plays a file FL
    /// ships reads it here, and wherever the transport builds a graph, from
    /// the plugin directory `samples` has.
    ///
    /// The stream runs at `sample_rate_hint` when the device offers that
    /// rate with its default channel count and sample format, and at the
    /// device's default rate otherwise. [`Transport::sample_rate`] says
    /// which.
    ///
    /// Playback uses the [`Approximate`](crate::Approximate) curves.
    pub fn open(
        project: Arc<Project>,
        samples: PreparedSamples,
        sample_rate_hint: Option<u32>,
    ) -> Result<Self, Error> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or(Error::NoDevice)?;
        let config = config(&device, sample_rate_hint).map_err(stream_error)?;
        let sample_rate = config.sample_rate();
        let (commands, waiting) = RingBuffer::new(COMMANDS);
        let (done, retired) = RingBuffer::new(COMMANDS);
        let shared = Arc::new(Shared::default());
        let player = Player {
            engine: Engine::with_graph(engine::graph(&project, &samples, sample_rate)?, VOICES),
            commands: waiting,
            retired: done,
            shared: Arc::clone(&shared),
            left: [0.0; BLOCK],
            right: [0.0; BLOCK],
        };
        let format = config.sample_format();
        let config = config.config();
        let stream = match format {
            SampleFormat::F32 => stream::<f32>(&device, config, player),
            SampleFormat::I16 => stream::<i16>(&device, config, player),
            SampleFormat::I24 => stream::<I24>(&device, config, player),
            SampleFormat::I32 => stream::<i32>(&device, config, player),
            other => {
                let reason = format!("the device wants {other} samples");
                return Err(Error::Stream { reason });
            }
        };
        let stream = stream.map_err(stream_error)?;
        stream.play().map_err(stream_error)?;
        Ok(Transport {
            _stream: stream,
            commands,
            retired,
            shared,
            project,
            samples,
            sample_rate,
            mode: Mode::default(),
        })
    }

    /// Run the clock from where it is.
    pub fn play(&mut self) -> Result<(), Error> {
        // Before the command, so that the audio thread has the last word.
        self.shared.playing.store(true, Ordering::Relaxed);
        let sent = self.send(Command::Play);
        if sent.is_err() {
            self.shared.playing.store(false, Ordering::Relaxed);
        }
        sent
    }

    /// Hold the clock where it is and cut every voice.
    pub fn stop(&mut self) -> Result<(), Error> {
        self.shared.playing.store(false, Ordering::Relaxed);
        self.send(Command::Stop)
    }

    /// Whether the clock runs: true from [`Transport::play`] on, and false
    /// again after [`Transport::stop`] or once the song has played to its
    /// end. At the end the clock goes back to tick 0 and the voices that
    /// are sounding play out.
    pub fn is_playing(&self) -> bool {
        self.shared.playing.load(Ordering::Relaxed)
    }

    /// Whether the song starts over from tick 0 when it ends. Off when the
    /// transport opens. A pattern on its own starts over either way.
    pub fn set_loop(&mut self, on: bool) -> Result<(), Error> {
        self.send(Command::Loop(on))
    }

    /// Play the playlist, which the transport does when it opens, or with
    /// `false` the pattern [`Transport::set_pattern`] chose, round and
    /// round from its first tick, as FL's PAT mode does. See
    /// [`Engine::set_song_mode`].
    ///
    /// The graph is built here, on the caller's thread. On an error the
    /// mode stays what it was.
    pub fn set_song_mode(&mut self, on: bool) -> Result<(), Error> {
        self.set_mode(Mode {
            song: on,
            ..self.mode
        })
    }

    /// The pattern that plays when song mode is off, by its index: pattern
    /// 1 when the transport opens. A pattern that is not in the project is
    /// a bar of silence.
    pub fn set_pattern(&mut self, index: u16) -> Result<(), Error> {
        self.set_mode(Mode {
            pattern: index,
            ..self.mode
        })
    }

    fn set_mode(&mut self, mode: Mode) -> Result<(), Error> {
        if mode.alone() != self.mode.alone() {
            let project = Arc::clone(&self.project);
            self.swap(&project, mode)?;
        }
        self.mode = mode;
        Ok(())
    }

    /// Build the graph of `project` in `mode` and send it to the audio
    /// thread.
    fn swap(&mut self, project: &Project, mode: Mode) -> Result<(), Error> {
        // Free the graphs the audio thread is done with.
        while self.retired.pop().is_ok() {}
        let graph = engine::graph_of(project, &self.samples, self.sample_rate, mode)?;
        self.send(Command::Swap(Box::new(graph)))
    }

    /// Move the clock and cut every voice. A note or a clip that started
    /// before `ticks` is not picked up part-way.
    pub fn seek(&mut self, ticks: Ticks) -> Result<(), Error> {
        self.send(Command::Seek(ticks))
    }

    /// Play another project from the tick the clock is at, starting with
    /// the next block. The voices that are sounding play out as they
    /// started. A voice is cut only when no note of the new project plays
    /// into its insert.
    ///
    /// The graph is built here, on the caller's thread. The samples are the
    /// ones the transport has, so a channel whose sample is not among them
    /// stays silent until [`Transport::add_samples`] brings it. On an error
    /// the old project plays on.
    /// The mode stays: with song mode off, the new project's pattern plays.
    pub fn set_project(&mut self, project: Arc<Project>) -> Result<(), Error> {
        self.swap(&project, self.mode)?;
        self.project = project;
        Ok(())
    }

    /// Add decoded samples to the ones the transport was opened with, and
    /// play the project with them from the next block on. This is how a
    /// channel that got its sample after the transport opened comes to
    /// sound: [`prepare`](crate::prepare) a project that has the sample,
    /// on a thread that may read files, and hand over what it decoded.
    pub fn add_samples(&mut self, samples: PreparedSamples) -> Result<(), Error> {
        self.samples.extend(samples);
        let project = Arc::clone(&self.project);
        self.swap(&project, self.mode)
    }

    /// The project the last [`Transport::set_project`] sent, or the one
    /// the transport was opened with.
    pub fn project(&self) -> &Arc<Project> {
        &self.project
    }

    /// The rate the stream runs at, in Hz.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Where the clock was at the end of the last block.
    pub fn position(&self) -> Ticks {
        self.shared.position.load(Ordering::Relaxed)
    }

    /// The loudest sample on the left and on the right since the last call,
    /// as the engine put it out: 1.0 is full scale and nothing is clipped.
    pub fn peak(&self) -> (f32, f32) {
        let take = |peak: &AtomicU32| f32::from_bits(peak.swap(0, Ordering::Relaxed));
        (take(&self.shared.peak_left), take(&self.shared.peak_right))
    }

    /// The loudest sample the insert with this loader number put out on
    /// the left and on the right since the last call for it, as
    /// [`Engine::insert_peaks`] says what an insert puts out. Silence for
    /// an insert that sound cannot reach.
    pub fn insert_peak(&self, insert: u16) -> (f32, f32) {
        let take = |peak: &AtomicU32| f32::from_bits(peak.swap(0, Ordering::Relaxed));
        let peaks = self.shared.insert_peaks.0.get(usize::from(insert));
        peaks.map_or((0.0, 0.0), |[left, right]| (take(left), take(right)))
    }

    /// How many errors the stream has reported since it opened. An
    /// underrun is one, and so is a device that went away.
    pub fn faults(&self) -> u32 {
        self.shared.faults.load(Ordering::Relaxed)
    }

    fn send(&mut self, command: Command) -> Result<(), Error> {
        self.commands.push(command).map_err(|_| Error::Backlog)
    }
}

fn stream_error(error: cpal::Error) -> Error {
    Error::Stream {
        reason: error.to_string(),
    }
}

/// The device's default output configuration, at the hinted rate when the
/// device offers that rate with the same channel count and sample format.
fn config(device: &cpal::Device, hint: Option<u32>) -> Result<SupportedStreamConfig, cpal::Error> {
    let default = device.default_output_config()?;
    let Some(rate) = hint.filter(|rate| *rate != default.sample_rate()) else {
        return Ok(default);
    };
    let hinted = device.supported_output_configs().ok().and_then(|ranges| {
        ranges
            .filter(|range| {
                range.channels() == default.channels()
                    && range.sample_format() == default.sample_format()
            })
            .find_map(|range| range.try_with_sample_rate(rate))
    });
    Ok(hinted.unwrap_or(default))
}

fn stream<T>(
    device: &cpal::Device,
    config: StreamConfig,
    mut player: Player,
) -> Result<Stream, cpal::Error>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels);
    let shared = Arc::clone(&player.shared);
    device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| player.fill(data, channels),
        // The audio thread calls this too, so it only counts.
        move |_| {
            shared.faults.fetch_add(1, Ordering::Relaxed);
        },
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::Sample;
    use model::{Arrangement, Channel, ChannelKind, ItemSource, Note, Pattern, PlaylistItem};

    const RATE: u32 = 48_000;

    /// 120 BPM at PPQ 96, a tick of 250 samples, and a note on each of
    /// `ticks` that plays 100 samples of 0.5 on the left and -0.25 on the
    /// right.
    fn graph(ticks: &[i32]) -> Graph {
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
        let sample = Arc::new(Sample {
            left: vec![0.5; 100],
            right: Some(vec![-0.25; 100]),
            rate: RATE,
        });
        let found = |_: &str| Ok(Some(Arc::clone(&sample)));
        Graph::new(&project, RATE, &crate::Approximate, usize::MAX, None, found).unwrap()
    }

    /// The two halves of a transport with no stream between them.
    struct Halves {
        player: Player,
        commands: Producer<Command>,
        retired: Consumer<Box<Graph>>,
        shared: Arc<Shared>,
    }

    fn halves(ticks: &[i32]) -> Halves {
        let (commands, waiting) = RingBuffer::new(COMMANDS);
        let (done, retired) = RingBuffer::new(2);
        let shared = Arc::new(Shared::default());
        let player = Player {
            engine: Engine::with_graph(graph(ticks), VOICES),
            commands: waiting,
            retired: done,
            shared: Arc::clone(&shared),
            left: [0.0; BLOCK],
            right: [0.0; BLOCK],
        };
        Halves {
            player,
            commands,
            retired,
            shared,
        }
    }

    fn peak(shared: &Shared) -> (f32, f32) {
        let take = |peak: &AtomicU32| f32::from_bits(peak.swap(0, Ordering::Relaxed));
        (take(&shared.peak_left), take(&shared.peak_right))
    }

    #[test]
    fn commands_are_taken_at_the_top_of_a_block() {
        let mut halves = halves(&[0, 4]);
        let mut data = [9f32; 2 * 600];
        halves.player.fill(&mut data, 2);
        assert_eq!(data, [0.0; 2 * 600]);
        assert_eq!(halves.shared.position.load(Ordering::Relaxed), 0);
        assert_eq!(peak(&halves.shared), (0.0, 0.0));

        // 600 frames are two blocks, of 512 and 88. The clock is at 600.
        assert!(halves.commands.push(Command::Play).is_ok());
        halves.player.fill(&mut data, 2);
        assert_eq!(data[..200], [0.5, -0.25].repeat(100));
        assert_eq!(data[200..], [0.0; 2 * 500]);
        assert_eq!(halves.shared.position.load(Ordering::Relaxed), 2);
        // The peak holds until it is read.
        assert_eq!(peak(&halves.shared), (0.5, 0.25));
        assert_eq!(peak(&halves.shared), (0.0, 0.0));

        assert!(halves.commands.push(Command::Seek(4)).is_ok());
        assert!(halves.commands.push(Command::Stop).is_ok());
        halves.player.fill(&mut data, 2);
        assert_eq!(data, [0.0; 2 * 600]);
        assert_eq!(halves.shared.position.load(Ordering::Relaxed), 4);
    }

    #[test]
    fn every_insert_has_a_peak_that_holds_until_it_is_read() {
        let mut halves = halves(&[0]);
        let mut data = [0f32; 2 * 600];
        assert!(halves.commands.push(Command::Play).is_ok());
        halves.player.fill(&mut data, 2);
        let take = |insert: usize| {
            let [left, right] = &halves.shared.insert_peaks.0[insert];
            let take = |peak: &AtomicU32| f32::from_bits(peak.swap(0, Ordering::Relaxed));
            (take(left), take(right))
        };
        // The channel plays into the master, and no other insert has a bus.
        assert_eq!(take(0), (0.5, 0.25));
        assert_eq!(take(0), (0.0, 0.0));
        assert_eq!(take(1), (0.0, 0.0));
        assert_eq!(halves.shared.insert_peaks.0.len(), 502);
    }

    #[test]
    fn the_end_of_the_song_is_told_and_a_loop_is_not() {
        // The song is 96,000 samples: 160 buffers of 600 frames.
        let mut halves = halves(&[0]);
        let mut data = [0f32; 2 * 600];
        assert!(halves.commands.push(Command::Play).is_ok());
        for _ in 0..160 {
            halves.player.fill(&mut data, 2);
        }
        assert!(halves.shared.playing.load(Ordering::Relaxed));
        halves.player.fill(&mut data, 2);
        assert!(!halves.shared.playing.load(Ordering::Relaxed));
        assert_eq!(halves.shared.position.load(Ordering::Relaxed), 0);

        assert!(halves.commands.push(Command::Loop(true)).is_ok());
        assert!(halves.commands.push(Command::Play).is_ok());
        for _ in 0..161 {
            halves.player.fill(&mut data, 2);
        }
        assert!(halves.shared.playing.load(Ordering::Relaxed));
        assert_eq!(data[..200], [0.5, -0.25].repeat(100));
        assert_eq!(halves.shared.position.load(Ordering::Relaxed), 2);
        assert!(halves.commands.push(Command::Stop).is_ok());
        halves.player.fill(&mut data, 2);
        assert!(!halves.shared.playing.load(Ordering::Relaxed));
    }

    #[test]
    fn an_old_graph_goes_back_to_be_freed() {
        let mut halves = halves(&[0]);
        let mut data = [0f32; 64];
        assert!(halves.commands.push(Command::Play).is_ok());
        for ticks in [[1], [2], [3]] {
            let swap = Command::Swap(Box::new(graph(&ticks)));
            assert!(halves.commands.push(swap).is_ok());
        }
        // The ring back holds two graphs, so the third swap waits.
        halves.player.fill(&mut data, 2);
        assert_eq!(halves.retired.slots(), 2);
        assert_eq!(halves.commands.slots(), COMMANDS - 1);
        assert_eq!(halves.retired.pop().unwrap().events[0].start, 0);
        halves.player.fill(&mut data, 2);
        assert_eq!(halves.commands.slots(), COMMANDS);
        assert_eq!(halves.retired.pop().unwrap().events[0].start, 250);
        assert_eq!(halves.retired.pop().unwrap().events[0].start, 500);
    }

    #[test]
    fn one_channel_gets_the_mean_and_spare_channels_stay_silent() {
        let mut mono = halves(&[0]);
        assert!(mono.commands.push(Command::Play).is_ok());
        let mut data = [9f32; 4];
        mono.player.fill(&mut data, 1);
        assert_eq!(data, [0.125; 4]);

        let mut wide = halves(&[0]);
        assert!(wide.commands.push(Command::Play).is_ok());
        let mut data = [9i16; 8];
        wide.player.fill(&mut data, 4);
        assert_eq!(data, [16_384, -8192, 0, 0, 16_384, -8192, 0, 0]);
    }
}
