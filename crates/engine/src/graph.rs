//! A project made ready to play: its notes and audio clips as voices on a
//! sample clock, the notes of its hosted plugin channels as cues for their
//! generators, its mixer with every gain worked out, and the automation
//! that moves those gains as the song plays.

use crate::automation::Automation;
use crate::clock::Clock;
use crate::generator::{Cue, Rack};
use crate::mix::{Mixer, Told};
use crate::sample::Sample;
use crate::sequence::{self, Song};
use crate::slide::Slides;
use crate::voice::{self, Voice};
use crate::{Curves, Error, levels};
use model::{Channel, ChannelKind, Project};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

/// A voice and the output sample it starts on.
pub(crate) struct Event {
    pub start: usize,
    pub voice: Voice,
}

/// Everything [`Engine`](crate::Engine) needs of a project. Building one
/// allocates and may read files. Playing one does neither.
pub(crate) struct Graph {
    pub clock: Clock,
    /// The voices of the song in the order they start.
    pub events: Vec<Event>,
    /// The note ons and note offs for the mixer's generators, in the order
    /// they are due.
    pub cues: Vec<Cue>,
    pub mixer: Mixer,
    /// The controls that automation clips drive.
    pub automation: Automation,
    /// Output samples from the start of the song to the end of its last
    /// item, of its last voice or of the tail of its last generator note,
    /// whichever is latest, and no more than the limit the graph was built
    /// with.
    pub len: usize,
    /// The output sample the song's last item ends on. The engine stops
    /// there, or turns back to the start.
    pub end: usize,
    /// Whether the graph is one pattern, which always turns back.
    pub pattern: bool,
    /// Notes that start before the limit, on any channel.
    pub notes: usize,
    /// The events that are audio clips. The others are notes.
    pub clips: usize,
    /// The clips among those whose item has a stretch other than 1. They
    /// play at the sample's own speed all the same.
    pub stretched: usize,
    /// Automation clips that start before the limit and drive a control
    /// the engine plays.
    pub automation_clips: usize,
    /// The links from those clips to a control of an effect slot.
    pub slot_targets: usize,
    /// What the automation clips that start before the limit drive and
    /// the engine does not play, with one for a clip that drives nothing.
    pub unsupported_targets: usize,
    /// The links whose source is an output of a controller plugin the
    /// engine hosts, in the whole project.
    pub controller_links: usize,
    /// The ones among them that drive a control the engine plays.
    pub controller_targets: usize,
    /// The others: a control the engine does not play, or a formula it
    /// does not read.
    pub unsupported_controller_targets: usize,
    /// The plugins of the plugin channels that are switched on, have a
    /// note before the limit and are not hosted, by event 201 name, and
    /// the channels each is on.
    pub unhosted_generators: BTreeMap<String, usize>,
    /// The hosted generators that stand in for a thing their state asks
    /// for, by what they say of it, and the channels each is on.
    pub approximated_generators: BTreeMap<&'static str, usize>,
}

impl Graph {
    /// `limit` is the output sample the song is cut at: a note or a clip
    /// that starts there or later is left out. `sample` finds the sample
    /// for a channel's path. It is asked only about channels that are
    /// switched on: sampler channels with a note before the limit and audio
    /// clip channels with a clip before it. A plugin channel gets its
    /// generator on the same terms, and the generator is told `plugin_dir`,
    /// FL's `Plugins/Fruity/Generators` directory, which may read files.
    /// The plugins in the effect slots are told it as well.
    pub fn new(
        project: &Project,
        sample_rate: u32,
        curves: &dyn Curves,
        limit: usize,
        plugin_dir: Option<&Path>,
        sample: impl FnMut(&str) -> Result<Option<Arc<Sample>>, Error>,
    ) -> Result<Self, Error> {
        let song = sequence::sequence(project);
        let mode = (false, sample_rate, limit, plugin_dir);
        Self::of(project, &song, mode, curves, sample)
    }

    /// The graph of one pattern on its own, as FL's PAT mode plays it. The
    /// playlist adds nothing, so no automation clip plays.
    pub fn pattern(
        project: &Project,
        index: u16,
        sample_rate: u32,
        curves: &dyn Curves,
        plugin_dir: Option<&Path>,
        sample: impl FnMut(&str) -> Result<Option<Arc<Sample>>, Error>,
    ) -> Result<Self, Error> {
        let song = sequence::pattern(project, index);
        let mode = (true, sample_rate, usize::MAX, plugin_dir);
        Self::of(project, &song, mode, curves, sample)
    }

    /// `mode` is whether the song is one pattern, the rate, the limit and
    /// the plugin directory.
    fn of(
        project: &Project,
        song: &Song,
        mode: (bool, u32, usize, Option<&Path>),
        curves: &dyn Curves,
        mut sample: impl FnMut(&str) -> Result<Option<Arc<Sample>>, Error>,
    ) -> Result<Self, Error> {
        let (pattern, sample_rate, limit, plugin_dir) = mode;
        let (clock, automation) = timeline(project, song, sample_rate, limit)?;
        // The plugins are made at the slowest tempo a clip can take the
        // song to, where one that keeps time asks for the most memory, and
        // then told the tempo the song starts at.
        let tempo = project.tempo().ok_or(Error::Tempo)?;
        let slowest = automation.slowest_tempo().unwrap_or(tempo);

        let mut events = Vec::new();
        let mut rack = Rack::default();
        rack.plugin_dir = plugin_dir.map(Path::to_path_buf);
        let mut played = BTreeSet::new();
        // The slides of the sampler channels' notes, by channel and by
        // the note's place in `events`.
        let mut slides = Slides::default();
        let mut notes = 0;
        let mut len = clock.sample(song.end).min(limit);
        for note in &song.notes {
            let start = clock.sample(note.tick);
            if start >= limit {
                continue;
            }
            notes += 1;
            let Some(channel) = project.channel(note.note.channel) else {
                continue;
            };
            if channel.kind == Some(ChannelKind::Plugin) {
                let insert = target_insert(channel);
                let timing = (sample_rate, slowest, project.header.ppq);
                let adjusts = (
                    voice::adjust(project, channel),
                    voice::pan_offset(project, channel),
                );
                let Some(place) = rack.place(channel, insert, timing, adjusts, curves) else {
                    continue;
                };
                if automation.drives_pitch(channel.index) {
                    rack.follow_pitch(place);
                }
                // A slide note starts no voice: it bends the notes that
                // sound.
                if note.note.slide() {
                    rack.slide(place, note, &clock);
                    continue;
                }
                let levels = levels::trigger(&note.note, channel, curves);
                // A clip on the channel's pitch moves the note as it starts.
                let clip = automation.pitch_shift(channel.index, note.tick as f64);
                let shift = (levels::shift(channel), clip);
                let done = rack.cue(place, note, start, &levels, shift, &clock);
                len = len.max(done.min(limit));
                played.insert(insert);
                continue;
            }
            if channel.kind != Some(ChannelKind::Sampler) || channel.enabled == Some(false) {
                continue;
            }
            if note.note.slide() {
                slides.slide(usize::from(channel.index), note, &clock);
                continue;
            }
            let Some(path) = channel.sample_path.as_deref().filter(|p| !p.is_empty()) else {
                continue;
            };
            let Some(sample) = sample(path)? else {
                continue;
            };
            let levels = levels::trigger(&note.note, channel, curves);
            // FinalLevels.Pitch, from where the sample plays at its own
            // speed.
            let semitones = f64::from(levels.pitch - levels::root(channel)) / 100.0;
            let step =
                curves.pitch_ratio(semitones) * f64::from(sample.rate) / f64::from(sample_rate);
            let held = note
                .length
                .map(|length| clock.sample(note.tick + length).saturating_sub(start));
            let insert = target_insert(channel);
            let level = levels.vol * curves.volume_adjust_gain(voice::adjust(project, channel));
            let gains = gains(project, channel, level, levels.pan, curves);
            let voice = Voice::new(sample, insert, held, step, gains);
            let voice = following(voice, channel, levels.vol, levels.pan, &automation, curves);
            len = len.max(start.saturating_add(voice.frames()).min(limit));
            played.insert(insert);
            let done = start.saturating_add(voice.frames());
            slides.note(usize::from(channel.index), events.len(), note, done);
            events.push(Event { start, voice });
        }
        // A channel whose plugin drives a link is hosted whether a note
        // reaches it or not.
        for index in automation.controller_channels() {
            let Some(channel) = project.channel(index) else {
                continue;
            };
            let timing = (sample_rate, slowest, project.header.ppq);
            let adjusts = (
                voice::adjust(project, channel),
                voice::pan_offset(project, channel),
            );
            rack.place(channel, target_insert(channel), timing, adjusts, curves);
        }
        // A voice that slides plays at the speed each step of its slides
        // takes it to, from where the step falls in it.
        let mut steps = slides.steps(&clock);
        steps.sort_by_key(|step| step.id);
        for of_one in steps.chunk_by(|a, b| a.id == b.id) {
            let event = &mut events[of_one[0].id];
            let bends: Vec<_> = (of_one.iter())
                .map(|step| {
                    let ratio = curves.pitch_ratio(f64::from(step.cents) / 100.0);
                    (step.at - event.start, ratio)
                })
                .collect();
            event.voice = event.voice.clone().bending(bends.into());
            len = len.max(event.start.saturating_add(event.voice.frames()).min(limit));
        }

        // A clip plays its channel's sample for as long as the item is, at
        // the item's gain, pan and pitch on top of the channel's own. The
        // item's stretch and the channel's time stretching are not applied.
        let (mut clips, mut stretched) = (0, 0);
        for clip in &song.clips {
            let start = clock.sample(clip.tick);
            if start >= limit {
                continue;
            }
            let Some(channel) = project.channel(clip.channel) else {
                continue;
            };
            if channel.enabled == Some(false) {
                continue;
            }
            let Some(path) = channel.sample_path.as_deref().filter(|p| !p.is_empty()) else {
                continue;
            };
            let Some(sample) = sample(path)? else {
                continue;
            };
            let settings = &clip.settings;
            let semitones = f64::from(settings.pitch) + cents(channel) / 100.0;
            let step =
                curves.pitch_ratio(semitones) * f64::from(sample.rate) / f64::from(sample_rate);
            let held = clock.sample(clip.tick + clip.length).saturating_sub(start);
            let offset = f64::from(clip.offset_ms) * f64::from(sample.rate) / 1000.0;
            let insert = target_insert(channel);
            let adjust = curves.volume_adjust_gain(voice::adjust(project, channel));
            let own = settings.gain * curves.clip_gain();
            let level = own * adjust;
            let gains = gains(project, channel, level, settings.pan, curves);
            let voice = Voice::new(sample, insert, Some(held), step, gains)
                .starting_at(offset, settings.reversed);
            let voice = following(voice, channel, own, settings.pan, &automation, curves);
            // Trimmed to past the end of its sample, or no ticks long.
            if voice.finished() {
                continue;
            }
            len = len.max(start.saturating_add(voice.frames()).min(limit));
            played.insert(insert);
            events.push(Event { start, voice });
            clips += 1;
            stretched += usize::from(settings.stretch != 1.0);
        }
        // The notes are in order and so are the clips. A stable sort keeps
        // voices that start together in that order: notes, then clips.
        events.sort_by_key(|event| event.start);
        let counts = automation.counts(&clock, limit);
        let sends = automation.sends();
        // An insert whose controller drives a link is mixed even when no
        // sound reaches it.
        let controllers = automation.controller_inserts();
        let (controller_links, controller_targets) =
            (automation.controller_links, automation.controller_targets());
        let unsupported_controller_targets = automation.unsupported_controller_targets;
        let told = Told {
            sample_rate,
            tempo: slowest,
            ppq: project.header.ppq,
            plugin_dir,
        };
        let mut mixer = Mixer::new(project, &played, &sends, &controllers, told, curves)?;
        // The files a generator's state names come from where a channel's
        // sample does. One that does not decode counts as missing.
        for hosted in &mut rack.hosted {
            for path in hosted.generator.sample_paths() {
                let file = sample(&path).ok().flatten();
                let file = file.as_deref().map(|file| plugins::SampleFile {
                    left: &file.left,
                    right: file.right.as_deref(),
                    rate: file.rate,
                });
                hosted.generator.set_sample(&path, file);
            }
        }
        let cues = rack.cues(&clock);
        let approximated_generators = rack.approximated();
        mixer.host(rack.hosted);
        mixer.set_tempo(automation.tempo_at(0.0).unwrap_or(tempo));
        Ok(Graph {
            mixer,
            end: clock.sample(song.end),
            clock,
            events,
            cues,
            automation,
            len,
            pattern,
            notes,
            clips,
            stretched,
            automation_clips: counts.clips,
            slot_targets: counts.slot_targets,
            unsupported_targets: counts.unsupported,
            controller_links,
            controller_targets,
            unsupported_controller_targets,
            unhosted_generators: rack.unhosted,
            approximated_generators,
        })
    }
}

/// The clock a song plays by and the automation that may bend it. `limit`
/// is the output sample the tempo is read up to.
pub(crate) fn timeline(
    project: &Project,
    song: &Song,
    sample_rate: u32,
    limit: usize,
) -> Result<(Clock, Automation), Error> {
    let tempo = project.tempo().ok_or(Error::Tempo)?;
    if project.header.ppq == 0 {
        return Err(Error::Ppq);
    }
    let ppq = project.header.ppq;
    let clock = Clock::new(tempo, ppq, sample_rate).ok_or(Error::Tempo)?;
    let automation = Automation::new(project, song, tempo);
    // A clip that drives the tempo bends the clock, and everything in the
    // song lands where the bent clock puts it.
    let clock = match automation.tempo_at(0.0) {
        Some(_) => clock.with_tempo(ppq, sample_rate, song.end, limit, |tick| {
            automation.tempo_at(tick).unwrap_or(tempo)
        }),
        None => clock,
    };
    Ok((clock, automation))
}

/// The insert the channel plays into. A channel without a target plays into
/// the master, and so does one whose target is outside 0..=501, which is
/// what FL's loader makes of it.
fn target_insert(channel: &Channel) -> u16 {
    let target = channel
        .insert()
        .and_then(|insert| u16::try_from(insert).ok());
    target.filter(|insert| *insert <= 501).unwrap_or(0)
}

/// How far the channel moves a clip, in cents: the pitch knob and 142
/// `FineTune`. A clip moves by this much and no more, because it plays at
/// the root note.
fn cents(channel: &Channel) -> f64 {
    f64::from(levels::knob(channel)) + f64::from(channel.fine_tune.unwrap_or(0))
}

/// Left and right gain of one voice at the volume and pan the channel's
/// 219 has, with its 229's pan offset. `level` is a note's volume or a
/// clip's gain, and `pan` is the note's or the clip's, from -1 to 1.
fn gains(
    project: &Project,
    channel: &Channel,
    level: f32,
    pan: f32,
    curves: &dyn Curves,
) -> (f32, f32) {
    let (volume, channel_pan) = voice::levels(channel);
    let channel_pan = channel_pan as i32 + voice::pan_offset(project, channel);
    voice::gains(volume, channel_pan, level, pan, curves)
}

/// The voice, set to follow its channel when a clip drives one of the
/// channel's levels. `level` is the voice's own, without the channel's
/// volume multiplier.
fn following(
    voice: Voice,
    channel: &Channel,
    level: f32,
    pan: f32,
    automation: &Automation,
    curves: &dyn Curves,
) -> Voice {
    match automation.following(channel.index, level, pan, curves) {
        Some(follow) => voice.following(follow),
        None => voice,
    }
}
