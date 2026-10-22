//! Plays the sampler channels, the audio clips, the automation clips and
//! the hosted plugin channels of a [`model::Project`]: offline to a stereo
//! buffer with [`render`], or on the default output device with a
//! [`Transport`].
//!
//! Both run the same [`Engine`], which works a block at a time:
//!
//! 1. Clock. Ticks become samples from the project's tempo and PPQ. The
//!    tempo is one value for the whole song, unless an automation clip
//!    drives it.
//! 2. Sequence. The current arrangement's pattern items become notes on the
//!    song's timeline, each item showing its window of its pattern, and its
//!    items on audio clip channels become clips. Muted items and muted
//!    notes add nothing. A project with an empty playlist plays pattern 1
//!    once. The notes and clips due in a block start their voices in it.
//! 3. Voices. Every note on a sampler channel plays the channel's sample
//!    from note-on for the note's length, at a speed set by its key, its
//!    fine pitch and the channel's pitch, with velocity, channel volume and
//!    pan applied. Every clip plays its channel's sample for the item's
//!    length, from the item's trim start on, forwards or backwards, with
//!    the item's gain, pan and pitch on top of the channel's. A plugin
//!    channel plays when the [`plugins`] crate has its generator: every
//!    note of the channel is a note on and a note off for the generator,
//!    which keeps its own voices, and the channel's volume and pan go on
//!    what it puts out. A slide note starts no voice: it bends the notes
//!    of its channel and its colour that sound to its key, a tick at a
//!    time, on a sampler channel and on a plugin channel alike.
//! 4. Mix. A channel's voices sum into the channel's insert. Each insert
//!    runs its bus through the plugins in its effect slots and its
//!    three-band EQ, applies its volume, pan, stereo separation, polarity
//!    and left-right swap, and adds its bus to every insert it routes to,
//!    at that route's send level. The master's bus with the main volume on it is
//!    the output. Nothing is clipped. Every insert keeps the peak of what
//!    it puts out, for a meter to read. Where a plugin is late, every
//!    other way to the master is held back by as much, so that all of
//!    them come out together: [`Rendered::latency`] frames behind the
//!    clock, which does not move for it. A plugin whose state names a
//!    sidechain input is handed what the insert at that number puts out,
//!    among the inserts that send to its own: a route at level 0 is FL's
//!    sidechain, heard by the plugin and not in the mix.
//!    [`Rendered::sidechain_slots`] counts those plugins.
//! 5. Automation. An automation clip plays where the playlist has an item
//!    for its channel, and drives the controls its links bind it to: an
//!    insert's volume, pan, stereo separation, EQ and send levels, the
//!    volume, pan, pitch, pan offset and volume multiplier of a sampler or
//!    audio clip channel, the tempo, and of an effect slot with a hosted
//!    plugin its switch, its mix level and the plugin's parameters, which
//!    [`Rendered::slot_targets`] counts. Of a plugin channel with a hosted
//!    generator it drives the same five levels and the generator's
//!    parameters. The controls are read at the
//!    start of a block and stay as read until the next one: a block starts
//!    on every 512th sample of the song and where a note or a clip starts.
//!    The tempo is read on every 512th sample only. A control of an effect
//!    slot that holds no plugin changes nothing, and is left alone. Every
//!    other control a clip is bound to is counted in
//!    [`Rendered::unsupported_targets`], and so is a clip bound to nothing. [`automation::Curve`] is the curve a
//!    clip draws.
//! 6. Controllers. A hosted plugin that is an internal controller (Fruity
//!    Peak Controller, Formula Controller, X-Y Controller and Control
//!    Surface in an effect slot, Fruity Envelope Controller and Keyboard
//!    Controller on a channel) works its outputs out once a tick of the
//!    song, and the engine ends a block where a tick starts when it hosts
//!    one. A 227 link from one of its outputs sets its control as the tick
//!    starts: the output's 0 to 1, through the link's formula when it has
//!    one, covers the control's whole range. The controls are the ones a
//!    clip drives, but for the tempo. [`Rendered::controller_links`]
//!    counts the links and [`Rendered::controller_targets`] the ones that
//!    are played. [`automation`] has the rest.
//!
//! Layer channels are not played, and nor is a plugin channel whose
//! generator the [`plugins`] crate lacks:
//! [`Rendered::unhosted_generators`] names the ones a song asks for. A
//! generator hears of a note on the first sample of a block, because a
//! block ends where the next note on or note off of a hosted channel is
//! due. The controls are not read again there. A step sequencer note is a
//! note on and a note off on one sample. Of an automation clip only
//! the envelope is played, and its LFO is not. An audio clip is not
//! time-stretched: its item's stretch is counted in
//! [`Rendered::unsupported_stretch`] and the channel's own stretch settings
//! are not read.
//!
//! An effect slot plays its plugin when the [`plugins`] crate has it, which
//! [`Rendered::hosted_slots`] counts. Any other slot passes its input
//! through, and [`Rendered::unhosted_slots`] names its plugin. A hosted
//! plugin is told the song's tempo, and again where an automation clip
//! changes it, and where FL keeps its plugins, for the one that reads a
//! file of FL's. [`Rendered::approximated_slots`] counts the hosted plugins
//! that say they stand in for what FL's does with their state or lack a
//! file, and the ones that ask for a sidechain input their insert does not
//! have.
//!
//! A song ends where its last playlist item does, and [`song_length`] says
//! where that is. An [`Engine`] stops there and lets its last voices play
//! out, or starts over when it loops. With song mode off it plays one
//! pattern round and round and leaves the playlist out. [`render`] plays
//! the song once, to the end of its last voice.
//!
//! [`render`] decodes the samples it needs as it goes. Realtime playback
//! takes them decoded: [`prepare`] reads the files on the caller's thread,
//! and nothing after it touches the disk.
//!
//! The note and playlist records are [`model::Note`] and
//! [`model::PlaylistItem`], and `docs/records-224-233.md` says where each
//! of their fields comes from.
//! [`render`] and [`Engine`] use the [`Approximate`] curves, which are not
//! FL's, so levels and pan positions are close to FL's and not equal to
//! them. [`FlExact`] has FL's own curves for insert volume, send level,
//! velocity, channel volume and pan, and its key and velocity tracking,
//! and [`render_with`] takes it. `docs/plugins/host-voice.md` says how FL's
//! host gets from a note to the levels a voice plays by.

pub mod automation;
mod channel_filter;
mod clock;
mod curves;
mod engine;
mod eq;
mod formula;
mod generator;
mod graph;
mod levels;
mod live;
mod mix;
mod pool;
mod sample;
mod sequence;
mod slide;
mod transport;
mod voice;

pub use curves::{Approximate, Curves, FlExact};
pub use engine::Engine;
pub use mix::Part;
pub use sample::{PreparedSamples, prepare};
pub use transport::Transport;

use graph::Graph;
use mix::BLOCK;
use model::{ChannelKind, Project};
use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

/// A position on the song's timeline, in ticks from its start. A beat is
/// as many ticks as the project's PPQ.
pub type Ticks = u64;

#[derive(Debug, Clone, PartialEq)]
pub struct RenderOptions {
    /// Output rate in Hz.
    pub sample_rate: u32,
    /// Where samples are looked up: FL's `Data` directory or its install
    /// directory for the samples FL ships, and the base for relative paths.
    pub sample_root: PathBuf,
    /// The render stops here even when the song goes on.
    pub max_seconds: f32,
    /// FL's `Plugins/Fruity/Generators` directory, where the generators
    /// that play a file FL ships find it. With `None` it is taken to be
    /// next to `sample_root` when that is FL's `Data` directory, and
    /// otherwise there is none: such a generator then plays what it can
    /// and is one of the [`Rendered::approximated_generators`].
    ///
    /// The plugins in the effect slots are told the same directory. Fruity
    /// Convolver finds FL's `Data` from it, where its impulses are, and
    /// without it passes its input on and is one of the
    /// [`Rendered::approximated_slots`].
    pub plugin_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    pub left: Vec<f32>,
    pub right: Vec<f32>,
    pub sample_rate: u32,
    /// Channels that were not played because of their kind: plugin, layer,
    /// automation, or a kind the model has no name for. A plugin channel
    /// that is one of the [`Rendered::hosted_generators`] is not counted.
    pub skipped: usize,
    /// Notes that start inside the render, on any channel. Muted notes and
    /// the notes of muted items are not among them.
    pub notes: usize,
    /// The notes among those that played a sample.
    pub voices: usize,
    /// Audio clips that start inside the render and played a sample.
    pub clips: usize,
    /// The clips among those whose item has a stretch other than 1. They
    /// played at the sample's own speed.
    pub unsupported_stretch: usize,
    /// Automation clips that start inside the render and drive a control
    /// the engine plays. A clip is an automation channel here, however
    /// many items the playlist has for it.
    pub automation_clips: usize,
    /// The links from those clips to a control of an effect slot whose
    /// plugin the engine hosts: the slot's switch, its mix level, or a
    /// parameter the plugin has.
    pub slot_targets: usize,
    /// What the automation clips that start inside the render could not
    /// do: one for every link to a control the engine does not play, and
    /// one for every clip without a link.
    pub unsupported_targets: usize,
    /// The 227 links whose source is an output of a controller plugin the
    /// engine hosts, in the whole project, such as Fruity Peak Controller's
    /// Peak + LFO, Peak and LFO.
    pub controller_links: usize,
    /// The ones among them that drive a control the engine plays, which
    /// are the controls an automation clip can drive, but for the tempo.
    pub controller_targets: usize,
    /// The others: a link to a control the engine does not play, or with
    /// a formula it does not read.
    pub unsupported_controller_targets: usize,
    /// Effect slots whose plugin the engine hosts, on the inserts that
    /// sound can reach. A slot that is switched off is one of them.
    pub hosted_slots: usize,
    /// The plugins in the other effect slots of those inserts, by the name
    /// event 201 gives them, and how many slots each is in. Those slots
    /// pass their input through. A hosted plugin is here for a slot whose
    /// state it turns down.
    pub unhosted_slots: BTreeMap<String, usize>,
    /// The hosted slots whose plugin stands in for a thing its state asks
    /// for, by what [`plugins::Fidelity::Approximate`] says of it, and how
    /// many slots each is in. A slot whose plugin asks for a sidechain
    /// input that its insert does not have is here as well.
    pub approximated_slots: BTreeMap<&'static str, usize>,
    /// The sidechain inputs the hosted plugins listen to: every pair of
    /// an insert and an insert that sends to it which a plugin's state
    /// names. An input no sound reaches is one of them.
    pub sidechain_routes: usize,
    /// The hosted slots whose plugin is handed a sidechain input.
    pub sidechain_slots: usize,
    /// Plugin channels whose generator the engine hosts: the ones that
    /// are switched on and have a note inside the render.
    pub hosted_generators: usize,
    /// The plugins of the other plugin channels that are switched on and
    /// have a note inside the render, by the name event 201 gives them,
    /// and how many channels each is on. Those channels are silent. A
    /// hosted generator is here for a channel whose state it turns down.
    pub unhosted_generators: BTreeMap<String, usize>,
    /// The hosted generators that stand in for a thing their state asks
    /// for, or that lack a file of FL's, by what
    /// [`plugins::Fidelity::Approximate`] says of it, and how many channels
    /// each is on.
    pub approximated_generators: BTreeMap<&'static str, usize>,
    /// How many frames late the output is when the render starts: what the
    /// plugins on the latest way to the master add up to. Every other way
    /// there is held back to come out with it.
    pub latency: usize,
    /// The inserts that are held back for it: the ones with a send that
    /// is delayed, and the ones whose voices are.
    pub delayed_inserts: usize,
    /// Sample paths with no file behind them, each listed once. Only
    /// channels with a note or a clip inside the render are looked up.
    pub unresolved: Vec<String>,
    /// Sample paths whose file is a kind of WavPack that is not decoded,
    /// each listed once. FL ships 24 of those, all hybrid.
    pub unsupported: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum Error {
    /// The project has no tempo, or one that is not above zero.
    Tempo,
    /// The header's PPQ is 0.
    Ppq,
    /// A sample file is there and could not be decoded.
    Decode { path: PathBuf, reason: String },
    /// The mixer's routes lead from this insert back to itself. FL does not
    /// let a project have such a loop.
    RouteCycle { insert: u16 },
    /// The host has no default output device.
    NoDevice,
    /// The output device turned down the stream, or the stream did not
    /// start.
    Stream { reason: String },
    /// The audio thread has 64 commands waiting and took none of them, so
    /// this one was dropped. The stream has stopped calling back.
    Backlog,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Tempo => write!(f, "the project has no usable tempo"),
            Error::Ppq => write!(f, "the project's PPQ is 0"),
            Error::Decode { path, reason } => {
                write!(f, "cannot decode {}: {reason}", path.display())
            }
            Error::RouteCycle { insert } => {
                write!(f, "the mixer routes insert {insert} back into itself")
            }
            Error::NoDevice => write!(f, "there is no default output device"),
            Error::Stream { reason } => write!(f, "cannot open the audio stream: {reason}"),
            Error::Backlog => write!(f, "the audio thread is not taking commands"),
        }
    }
}

impl std::error::Error for Error {}

/// Render with the [`Approximate`] curves.
pub fn render(project: &Project, opts: &RenderOptions) -> Result<Rendered, Error> {
    render_with(project, opts, &Approximate)
}

/// How many output samples the song is long at `sample_rate`: up to the
/// end of the last playlist item of the current arrangement, on the clock
/// that an automation clip on the tempo bends. This is where an [`Engine`]
/// in song mode stops.
///
/// It is [`Project::song_length_ticks`] in samples, but for a project with
/// an empty playlist: that one plays pattern 1 once, so it is as long as
/// pattern 1 rounded up to a whole bar. 0 for a project [`Engine::new`]
/// turns down for its tempo or its PPQ.
pub fn song_length(project: &Project, sample_rate: u32) -> u64 {
    let song = sequence::sequence(project);
    graph::timeline(project, &song, sample_rate, usize::MAX)
        .map_or(0, |(clock, _)| clock.sample(song.end) as u64)
}

/// Render with curves of your own.
pub fn render_with(
    project: &Project,
    opts: &RenderOptions,
    curves: &dyn Curves,
) -> Result<Rendered, Error> {
    render_with_progress(project, opts, curves, &mut |_| {})
}

/// [`render_with`], telling `progress` how far it is: the part of the
/// output that is mixed, from 0 to 1, after every half second of audio or
/// less, and 1.0 at the end. Decoding the samples comes before the first
/// call and is not counted. [`RenderOptions`] has no field for this because
/// it is `Clone` and `PartialEq`, which a callback is not.
pub fn render_with_progress(
    project: &Project,
    opts: &RenderOptions,
    curves: &dyn Curves,
    progress: &mut dyn FnMut(f32),
) -> Result<Rendered, Error> {
    // A NaN limit counts as 0.
    let limit = (f64::from(opts.max_seconds.max(0.0)) * f64::from(opts.sample_rate)) as usize;
    let mut samples = PreparedSamples::default();
    let load = |path: &str| samples.load(path, &opts.sample_root);
    let derived = sample::plugin_dir(&opts.sample_root);
    let plugin_dir = opts.plugin_dir.as_deref().or(derived.as_deref());
    let graph = Graph::new(project, opts.sample_rate, curves, limit, plugin_dir, load)?;
    let (notes, clips, stretched) = (graph.notes, graph.clips, graph.stretched);
    let (automation_clips, unsupported_targets) =
        (graph.automation_clips, graph.unsupported_targets);
    let slot_targets = graph.slot_targets;
    let (controller_links, controller_targets) = (graph.controller_links, graph.controller_targets);
    let unsupported_controller_targets = graph.unsupported_controller_targets;
    let (events, len) = (graph.events.len(), graph.len);
    let hosted_slots = graph.mixer.hosted_slots;
    let unhosted_slots = graph.mixer.unhosted_slots.clone();
    let approximated_slots = graph.mixer.approximated_slots.clone();
    let (sidechain_routes, sidechain_slots) =
        (graph.mixer.sidechain_routes, graph.mixer.sidechain_slots);
    let hosted_generators = graph.mixer.generators();
    let unhosted_generators = graph.unhosted_generators.clone();
    let approximated_generators = graph.approximated_generators.clone();
    let (latency, delayed_inserts) = (graph.mixer.latency(), graph.mixer.delayed());

    // A voice for every note and clip, so that none is ever stolen.
    let mut engine = Engine::with_graph(graph, events);
    engine.run_past_end();
    engine.play();
    let (mut left, mut right) = (vec![0.0; len], vec![0.0; len]);
    // Whole blocks, so the engine cuts the song where one long call would.
    let step = ((opts.sample_rate / 2) as usize / BLOCK).max(1) * BLOCK;
    let mut done = 0;
    for (left, right) in left.chunks_mut(step).zip(right.chunks_mut(step)) {
        engine.process_with(curves, left, right);
        done += left.len();
        progress(done as f32 / len as f32);
    }
    if len == 0 {
        progress(1.0);
    }

    Ok(Rendered {
        left,
        right,
        sample_rate: opts.sample_rate,
        skipped: project
            .channels
            .iter()
            .filter(|c| !matches!(c.kind, Some(ChannelKind::Sampler | ChannelKind::AudioClip)))
            .count()
            - hosted_generators,
        notes,
        voices: events - clips,
        clips,
        unsupported_stretch: stretched,
        automation_clips,
        slot_targets,
        unsupported_targets,
        controller_links,
        controller_targets,
        unsupported_controller_targets,
        hosted_slots,
        unhosted_slots,
        approximated_slots,
        sidechain_routes,
        sidechain_slots,
        hosted_generators,
        unhosted_generators,
        approximated_generators,
        latency,
        delayed_inserts,
        unresolved: samples.unresolved(),
        unsupported: samples.unsupported(),
    })
}
