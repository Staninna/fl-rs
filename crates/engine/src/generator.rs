//! The plugins on channels that the engine hosts, and the notes it hands
//! them.
//!
//! A plugin channel plays when [`plugins::generator_by_name`] has its
//! plugin and the plugin takes the channel's state. Every note of the
//! channel is then a note on at the sample the note starts on and a note
//! off at the sample it ends on. The generator adds what it makes of them
//! to a block, the channel's volume and pan go on that, and the result
//! plays into the channel's insert next to the sampler voices.
//!
//! The engine ends its block where the next of those two is due, so a
//! generator hears of a note on the first frame of a block and needs no
//! offsets into it.
//!
//! A step sequencer note has no length. Its note off comes on the sample
//! of its note on, so a generator plays its release and no more. What FL
//! does with one is not read yet.
//!
//! A note on carries what FL's host hands a plugin in `TVoiceParams`, as
//! [`levels::trigger`](crate::levels::trigger) works it out:
//!
//! - `(key - 60) * 100 + fine_pitch` is `FinalLevels.Pitch` in cents, less
//!   the channel's root. The whole keys the channel moves a note by are in
//!   the key, and the rest is in the fine pitch with the note's own.
//! - `velocity` is `InitLevels.Vol`, the gain the curves make of the
//!   note's velocity.
//! - `pan`, `mod_x` and `mod_y` are the note's own, from -1 to 1, with
//!   what the channel's tracking adds.
//! - `release` is the note's release velocity, from 0 to 1.
//!
//! - `color` is the note's colour, which FL's host hands a plugin for
//!   `FPV_GetColor`.
//!
//! FL puts the channel's volume and pan into `FinalLevels` as well. Here
//! they go on what the generator puts out, so that automation can move
//! them under a sounding note, and the generator hears the volume's gain
//! with [`Generator::set_channel_volume`] for a plugin that does more with
//! `FinalLevels.Vol` than multiply by it.
//!
//! # Pitch that moves under a note
//!
//! FL's host works `FinalLevels` out again on every tick (`FUN_00d2ab30`
//! calls `FUN_00d265e0`), and a plugin reads them as its voices run. Two
//! things move the pitch there while a note sounds, and the engine tells a
//! generator of both with [`Generator::set_note_pitch`]:
//!
//! - A slide note, which starts no voice ([`crate::slide`]): the notes of
//!   its channel and its colour that sound slide to its key a tick at a
//!   time.
//! - A clip on the channel's pitch (control 4). The note takes the pitch
//!   the clip gives as it starts, and is told on every tick that the clip
//!   moves it after.
//!
//! The slides are worked out with the cues, from the song. The clip is
//! read on every tick as the song plays, and [`Hosted::bend`] tells the
//! notes it moves. Only the generators of
//! channels with one or the other keep the notes they have sounding, in
//! room the graph makes for the most that sound at once.

use crate::clock::Clock;
use crate::levels::Triggered;
use crate::sequence::Played;
use crate::slide::Slides;
use crate::{Curves, voice};
use model::{Channel, ChannelKind};
use plugins::{Fidelity, Generator, NoteOn};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// A generator the engine hosts, on the channel it plays for.
pub(crate) struct Hosted {
    /// The channel, by the index its 64 has.
    pub channel: u16,
    /// The loader's number of the insert the channel plays into.
    pub insert: u16,
    pub generator: Box<dyn Generator>,
    /// Left and right gain of the channel's volume and pan, which go on
    /// what the generator puts out.
    pub gains: (f32, f32),
    /// The notes it sounds and what moves their pitch, for a generator on
    /// a channel with slide notes or a clip on its pitch.
    pub bend: Option<Bend>,
}

/// What a generator is told.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Signal {
    /// A note starts, with what a clip on the channel's pitch has moved
    /// it by, in cents. That is in the note's pitch already.
    On(NoteOn, f32),
    /// The note that was started under this number ends.
    Off(u32),
    /// A slide has the note's pitch at this many cents from key 60, as the
    /// note on's key and fine pitch are, before what a clip on the
    /// channel's pitch adds.
    Slide { voice: u32, pitch: f32 },
}

/// The notes a generator has sounding, for a generator whose notes'
/// pitch moves: what each started at and has been told.
#[derive(Debug, Default)]
pub(crate) struct Bend {
    /// Whether a clip drives the channel's pitch.
    pub follows: bool,
    /// How far the clip had moved the channel's pitch, in cents, when the
    /// notes were last told.
    shift: f32,
    /// In the order they started. The room is made when the graph is
    /// built, and when it runs out the note that started first goes.
    notes: Vec<Bent>,
}

impl Bend {
    /// How far the clip had moved the channel's pitch, in cents, when the
    /// notes were last told.
    pub fn shift(&self) -> f32 {
        self.shift
    }
}

/// A note of a [`Bend`].
#[derive(Debug, Clone, Copy, PartialEq)]
struct Bent {
    voice: u32,
    /// The note's pitch in cents from key 60 with its slide, without what
    /// a clip on the channel's pitch adds.
    pitch: f32,
    /// The pitch the generator has of it.
    told: f32,
    /// The output sample from which it no longer sounds: past its note
    /// off and the generator's tail. `usize::MAX` while it is held.
    until: usize,
}

/// The pitch of a note on in cents from key 60.
fn pitch_of(note: &NoteOn) -> f32 {
    f32::from(note.key - 60) * 100.0 + note.fine_pitch
}

impl Hosted {
    /// Hand the generator a cue. Returns whether it let go of a note.
    pub fn hear(&mut self, cue: &Cue) -> bool {
        match cue.signal {
            Signal::On(note, shift) => {
                if let Some(bend) = &mut self.bend {
                    if bend.notes.len() == bend.notes.capacity() && !bend.notes.is_empty() {
                        bend.notes.remove(0);
                    }
                    let told = pitch_of(&note);
                    bend.notes.push(Bent {
                        voice: note.voice,
                        pitch: told - shift,
                        told,
                        until: usize::MAX,
                    });
                }
                self.generator.note_on(note);
                false
            }
            Signal::Off(voice) => {
                self.generator.note_off(voice);
                if let Some(bend) = &mut self.bend {
                    let until = cue.at.saturating_add(self.generator.tail());
                    for bent in bend.notes.iter_mut().filter(|b| b.voice == voice) {
                        bent.until = until;
                    }
                }
                true
            }
            Signal::Slide { voice, pitch } => {
                let Some(bend) = &mut self.bend else {
                    return false;
                };
                let shift = bend.shift;
                for bent in bend.notes.iter_mut().filter(|b| b.voice == voice) {
                    bent.pitch = pitch;
                    tell(self.generator.as_mut(), bent, shift);
                }
                false
            }
        }
    }

    /// The channel's pitch is `shift` cents off the project's at output
    /// sample `at`, by the clip on it: tell every note that still sounds
    /// whose pitch that moves. Notes that have stopped sounding are
    /// forgotten.
    pub fn bend(&mut self, at: usize, shift: f32) {
        let Some(bend) = &mut self.bend else {
            return;
        };
        bend.notes.retain(|bent| bent.until > at);
        if shift == bend.shift {
            return;
        }
        bend.shift = shift;
        for bent in &mut bend.notes {
            tell(self.generator.as_mut(), bent, shift);
        }
    }

    /// Forget the notes, which the generator has cut or let go of all at
    /// once.
    pub fn forget(&mut self) {
        if let Some(bend) = &mut self.bend {
            bend.notes.clear();
        }
    }
}

/// Tell the generator the pitch of a note when it has moved.
fn tell(generator: &mut dyn Generator, bent: &mut Bent, shift: f32) {
    let pitch = bent.pitch + shift;
    if pitch != bent.told {
        bent.told = pitch;
        generator.set_note_pitch(bent.voice, pitch);
    }
}

/// A signal and the output sample it is due on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Cue {
    pub at: usize,
    /// The generator's place among the [`Hosted`] of its graph.
    pub generator: usize,
    pub signal: Signal,
}

/// The plugin of a channel with the channel's state in it, when the engine
/// hosts it: the channel is a plugin channel, [`plugins::generator_by_name`]
/// has its plugin and the plugin takes the state. A channel without a 213
/// keeps the plugin's defaults.
///
/// `timing` is the rate in Hz, the tempo in BPM and the project's PPQ the
/// plugin is told before it gets its state. Without one it keeps its own, which is good
/// enough to ask it about its parameters.
///
/// `plugin_dir` is where FL keeps its generators. A plugin whose sound is
/// in a file there reads it now, and one that is not told the directory
/// says what it lacks in its fidelity.
pub(crate) fn hosted(
    channel: &Channel,
    timing: Option<(u32, f64, u16)>,
    plugin_dir: Option<&Path>,
) -> Option<Box<dyn Generator>> {
    if channel.kind != Some(ChannelKind::Plugin) {
        return None;
    }
    let mut generator = plugins::generator_by_name(channel.plugin_name.as_deref()?)?;
    if let Some((sample_rate, tempo, ppq)) = timing {
        generator.set_sample_rate(sample_rate as f32);
        generator.set_tempo(tempo);
        generator.set_ppq(ppq);
    }
    if let Some(dir) = plugin_dir {
        generator.set_resource_dir(dir);
    }
    let state = channel.plugin_state.as_deref();
    let taken = state.map_or(Ok(()), |state| generator.set_state(state));
    taken.ok()?;
    Some(generator)
}

/// The lowest and the highest value a parameter of a generator takes, when
/// it has the parameter. This leaves the parameter at its highest.
///
/// A plugin keeps a parameter inside the range of its control, so the two
/// ends are what it makes of the least and the most a float can be.
pub(crate) fn param_range(generator: &mut dyn Generator, index: usize) -> Option<(i32, i32)> {
    if index >= generator.param_count() {
        return None;
    }
    generator.set_param(index, f32::MIN);
    let low = generator.param(index);
    generator.set_param(index, f32::MAX);
    Some((low as i32, generator.param(index) as i32))
}

/// The generators of a song as its graph is built, and the cues for them.
#[derive(Default)]
pub(crate) struct Rack {
    pub hosted: Vec<Hosted>,
    /// In the order the notes came. [`Rack::cues`] puts them in the order
    /// they are due.
    cues: Vec<Cue>,
    /// The plugin channels with a note whose plugin is not hosted.
    passed: BTreeSet<u16>,
    /// Their plugins by event 201 name, and the channels each is on.
    pub unhosted: BTreeMap<String, usize>,
    /// FL's `Plugins/Fruity/Generators` directory, which every generator
    /// is told as it is made.
    pub plugin_dir: Option<PathBuf>,
    /// The notes cued so far, which numbers the next.
    voices: u32,
    /// Every note cued, by its number: the generator it is for, the output
    /// samples it starts and stops sounding on, and its pitch as its note
    /// on has it, without a clip's, in cents from key 60.
    notes: Vec<(usize, usize, usize, f32)>,
    /// The slides of the notes, by their number and their generator.
    slides: Slides,
}

impl Rack {
    /// The place in `hosted` of the generator of a plugin channel, which
    /// is made the first time the channel is asked about. `None` for a
    /// channel the engine does not host, whose plugin is then counted, and
    /// for one that is switched off, which is not.
    ///
    /// `timing` is the rate in Hz, the tempo in BPM and the PPQ the
    /// generator is made at, and `adjusts` the channel's [`voice::adjust`]
    /// and [`voice::pan_offset`].
    pub fn place(
        &mut self,
        channel: &Channel,
        insert: u16,
        timing: (u32, f64, u16),
        adjusts: (i32, i32),
        curves: &dyn Curves,
    ) -> Option<usize> {
        if channel.enabled == Some(false) || self.passed.contains(&channel.index) {
            return None;
        }
        let made = |hosted: &Hosted| hosted.channel == channel.index;
        if let Some(place) = self.hosted.iter().position(made) {
            return Some(place);
        }
        let Some(mut generator) = hosted(channel, Some(timing), self.plugin_dir.as_deref()) else {
            self.passed.insert(channel.index);
            let name = channel.plugin_name.as_deref().filter(|n| !n.is_empty());
            if let Some(name) = name {
                *self.unhosted.entry(name.to_owned()).or_default() += 1;
            }
            return None;
        };
        let (volume, pan) = voice::levels(channel);
        generator
            .set_channel_volume(curves.volume_adjust_gain(adjusts.0) * curves.volume_gain(volume));
        self.hosted.push(Hosted {
            bend: None,
            channel: channel.index,
            insert,
            generator,
            gains: voice::gains(
                volume,
                pan as i32 + adjusts.1,
                curves.volume_adjust_gain(adjusts.0),
                0.0,
                curves,
            ),
        });
        Some(self.hosted.len() - 1)
    }

    /// The hosted generators that stand in for a thing their state asks
    /// for, by what [`Fidelity::Approximate`] says of it, and how many
    /// channels each is on.
    pub fn approximated(&self) -> BTreeMap<&'static str, usize> {
        let mut approximated = BTreeMap::new();
        for hosted in &self.hosted {
            if let Fidelity::Approximate(what) = hosted.generator.fidelity() {
                *approximated.entry(what).or_default() += 1;
            }
        }
        approximated
    }

    /// A clip drives the pitch of the channel of the generator at `place`,
    /// so its notes are told when it moves.
    pub fn follow_pitch(&mut self, place: usize) {
        self.hosted[place].bend.get_or_insert_default().follows = true;
    }

    /// Cue a note for the generator at `place`: its note on at `start` and
    /// its note off where the clock puts its end. Returns the output sample
    /// the generator is done with it by.
    ///
    /// `levels` is what the host makes of the note, and `shift` how far
    /// the channel moves its notes, in cents: by its own settings, and by a
    /// clip on its pitch as the note starts.
    pub fn cue(
        &mut self,
        place: usize,
        played: &Played,
        start: usize,
        levels: &Triggered,
        shift: (f32, f32),
        clock: &Clock,
    ) -> usize {
        let note = &played.note;
        let length = played.length.unwrap_or(0);
        let end = clock.sample(played.tick + length).max(start);
        // One number for each note of the song, by the note's place in it.
        let voice = self.voices;
        self.voices += 1;
        let (own, clip) = shift;
        let shift = own + clip;
        // Whole keys go to the key and the rest to the fine pitch.
        let keys = (shift / 100.0).round();
        let on = NoteOn {
            voice,
            key: note.key.saturating_add(keys as i16),
            velocity: levels.vol,
            pan: levels.pan,
            fine_pitch: shift - keys * 100.0 + note.fine_pitch_cents() as f32,
            release: levels.release,
            mod_x: levels.mod_x,
            mod_y: levels.mod_y,
            slide: note.slide(),
            porta: note.porta(),
            color: note.color(),
        };
        for (at, signal) in [(start, Signal::On(on, clip)), (end, Signal::Off(voice))] {
            self.cues.push(Cue {
                at,
                generator: place,
                signal,
            });
        }
        let done = end.saturating_add(self.hosted[place].generator.tail());
        self.slides.note(place, voice as usize, played, done);
        self.notes.push((place, start, done, pitch_of(&on) - clip));
        done
    }

    /// A slide note for the generator at `place`, which bends the notes
    /// of its colour that sound ([`Slides::slide`]).
    pub fn slide(&mut self, place: usize, played: &Played, clock: &Clock) {
        if self.slides.slide(place, played, clock) {
            self.hosted[place].bend.get_or_insert_default();
        }
    }

    /// The cues in the order they are due. Where two are due on one
    /// sample, the note that started first comes first, a note's on comes
    /// before its off, and the notes' come before the slides'.
    ///
    /// The generators whose notes' pitch moves get room for as many notes
    /// as they sound at once.
    pub fn cues(&mut self, clock: &Clock) -> Vec<Cue> {
        let mut cues = std::mem::take(&mut self.cues);
        for step in std::mem::take(&mut self.slides).steps(clock) {
            let (place, _, _, pitch) = self.notes[step.id];
            cues.push(Cue {
                at: step.at,
                generator: place,
                signal: Signal::Slide {
                    voice: step.id as u32,
                    pitch: pitch + step.cents,
                },
            });
        }
        for (place, hosted) in self.hosted.iter_mut().enumerate() {
            let Some(bend) = &mut hosted.bend else {
                continue;
            };
            let mut edges: Vec<(usize, i32)> = (self.notes.iter())
                .filter(|note| note.0 == place)
                .flat_map(|&(_, start, done, _)| [(start, 1), (done, -1)])
                .collect();
            // A note that ends where another starts is gone by then.
            edges.sort_unstable();
            let mut sounding = 0;
            let mut most = 1;
            for (_, step) in edges {
                sounding += step;
                most = most.max(sounding);
            }
            bend.notes = Vec::with_capacity(most as usize);
        }
        cues.sort_by_key(|cue| cue.at);
        cues
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Approximate, FlExact, levels};
    use flp::event::layout::{Levels, Note};

    /// The note on of one note of a TestSine channel with this pitch knob,
    /// root note and fine tune.
    fn note_on(note: Note, tuning: (i32, u32, i32), curves: &dyn Curves) -> NoteOn {
        let (knob, root, fine_tune) = tuning;
        let channel = Channel {
            kind: Some(ChannelKind::Plugin),
            plugin_name: Some("TestSine".to_owned()),
            levels: Some(Levels {
                pan: 6400,
                volume: 10_000,
                unknown_8: knob.to_le_bytes(),
                mod_x: 0,
                mod_y: 0,
                unknown_20: [0; 4],
                len: 24,
            }),
            root_note: Some(root),
            fine_tune: Some(fine_tune),
            ..Channel::default()
        };
        let mut rack = Rack::default();
        let place = rack
            .place(&channel, 0, (48_000, 120.0, 96), (12_800, 0), curves)
            .unwrap();
        let played = Played {
            tick: 0,
            length: Some(96),
            note,
        };
        let clock = Clock::new(120.0, 96, 48_000).unwrap();
        let triggered = levels::trigger(&note, &channel, curves);
        rack.cue(
            place,
            &played,
            0,
            &triggered,
            (levels::shift(&channel), 0.0),
            &clock,
        );
        // What the generator makes its pitch of is FinalLevels.Pitch, from
        // the channel's root.
        let Signal::On(on, _) = rack.cues(&clock)[0].signal else {
            panic!("the first cue is the note on");
        };
        let pitch = (f32::from(on.key) - 60.0) * 100.0 + on.fine_pitch;
        assert_eq!(pitch, triggered.pitch - levels::root(&channel));
        on
    }

    #[test]
    fn a_note_on_has_what_fl_hands_a_plugin() {
        let note = Note {
            key: 69,
            fine_pitch: 125,
            velocity: 64,
            pan: 32,
            mod_x: 192,
            mod_y: 0,
            release: 96,
            ..Note::default()
        };
        let on = note_on(note, (0, 60, 0), &FlExact);
        assert_eq!((on.key, on.fine_pitch), (69, 50.0));
        assert_eq!(on.velocity, FlExact.velocity_gain(64));
        assert_eq!((on.pan, on.mod_x, on.mod_y), (-0.5, 0.5, -1.0));
        assert_eq!(on.release, 0.75);
        // The other curves differ in the velocity alone.
        let plain = note_on(note, (0, 60, 0), &Approximate);
        assert_eq!(plain.velocity, 0.64);
        assert_eq!(
            NoteOn {
                velocity: on.velocity,
                ..plain
            },
            on
        );

        // A new note at key 60 is pitch 0: no fine pitch, at unity.
        let new = note_on(
            Note {
                key: 60,
                ..Note::default()
            },
            (0, 60, 0),
            &FlExact,
        );
        assert_eq!((new.key, new.fine_pitch, new.velocity), (60, 0.0, 1.0));
        assert_eq!(
            (new.pan, new.mod_x, new.mod_y, new.release),
            (0.0, 0.0, 0.0, 0.5)
        );
    }

    #[test]
    fn the_channel_s_whole_keys_go_to_the_key_and_the_rest_to_the_fine_pitch() {
        let note = Note {
            key: 69,
            ..Note::default()
        };
        // A root 3 keys under 60 is 3 keys up.
        let on = note_on(note, (0, 57, 0), &FlExact);
        assert_eq!((on.key, on.fine_pitch), (72, 0.0));
        // 250 cents on the knob with it: 5.5 keys, which is 6 less 50.
        let on = note_on(note, (250, 57, 0), &FlExact);
        assert_eq!((on.key, on.fine_pitch), (75, -50.0));
        // The fine tune and the note's fine pitch stay cents.
        let sharp = Note {
            fine_pitch: 123,
            ..note
        };
        let on = note_on(sharp, (-1200, 60, 7), &FlExact);
        assert_eq!((on.key, on.fine_pitch), (57, 37.0));
    }
}
