//! What a plugin on a channel is to its host: it is handed notes and adds
//! the sound of them to a stereo block.

use crate::{Fidelity, StateError};
use std::path::Path;

/// One note for a [`Generator`] to start, with what the note's record and
/// its channel say of it worked out by the host.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoteOn {
    /// The host's number for the note, which [`Generator::note_off`] ends
    /// it by. No two notes that a generator has sounding share one.
    pub voice: u32,
    /// The key, where 60 is the C that FL calls C5. The channel's root
    /// note has moved it already.
    pub key: i16,
    /// The gain the host makes of the note's velocity: 1 for the 100 of a
    /// new note, where gain is linear in velocity.
    pub velocity: f32,
    /// From -1 on the left to 1 on the right. This is the note's own pan.
    /// The host puts the channel's on what the generator plays.
    pub pan: f32,
    /// Cents on top of the key: the note's fine pitch and the channel's
    /// pitch together.
    pub fine_pitch: f32,
    /// From 0 to 1, and 0.5 for a new note.
    pub release: f32,
    /// From -1 to 1, and 0 for a new note. FL's generators read the two as
    /// filter cutoff and resonance, unless their state says otherwise.
    pub mod_x: f32,
    pub mod_y: f32,
    /// A slide note. FL sounds no voice for one: it bends the voices that
    /// are sounding to its key. A host that does that itself, with
    /// [`Generator::set_note_pitch`], does not send one.
    pub slide: bool,
    /// A note that glides in from the one before it.
    pub porta: bool,
    /// The note's colour, 0 to 15, where 0 is FL's first: the low nibble
    /// of byte 19 of its record, which FL's host keeps at `rec+0x26c` of
    /// the voice and hands a plugin for `FPV_GetColor`
    /// (`docs/plugins/host-voice.md`). FL also takes it as the note's MIDI
    /// channel.
    pub color: u8,
}

impl Default for NoteOn {
    /// Voice 0 at key 60, as a new note in FL's piano roll is: velocity
    /// 100, which is a gain of 1, in the centre, release 64, the mods in
    /// the middle and the first colour.
    fn default() -> Self {
        NoteOn {
            voice: 0,
            key: 60,
            velocity: 1.0,
            pan: 0.0,
            fine_pitch: 0.0,
            release: 0.5,
            mod_x: 0.0,
            mod_y: 0.0,
            slide: false,
            porta: false,
            color: 0,
        }
    }
}

/// One plugin on a channel.
///
/// The generator keeps its own voices, as FL's do, and says how many it
/// sounds at once. A parameter is numbered as FL numbers it, and its value
/// is the integer FL saves for it, as an `f32`.
///
/// Only [`Generator::set_state`], [`Generator::set_sample_rate`],
/// [`Generator::set_tempo`], [`Generator::set_ppq`] and
/// [`Generator::set_resource_dir`] may
/// allocate. The rest is for an audio callback.
pub trait Generator: Send {
    /// Take the bytes of the plugin's event 213. A state the plugin turns
    /// down leaves it as it was.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError>;

    /// The rate the blocks will come at, in Hz.
    fn set_sample_rate(&mut self, hz: f32);

    /// The song's tempo in beats a minute, for a plugin that keeps time
    /// with it. The others take no notice.
    fn set_tempo(&mut self, _bpm: f64) {}

    /// The song's ticks to a beat, for a plugin that counts in them. The
    /// others take no notice.
    fn set_ppq(&mut self, _ppq: u16) {}

    /// Where an install of FL keeps its generators: its
    /// `Plugins/Fruity/Generators` directory, in which every generator has
    /// a directory by its name. A plugin whose sound is in a file FL ships
    /// reads the file from there, and this may read files. A host that has
    /// no such directory does not call this, and such a plugin then says
    /// what it lacks in [`Generator::fidelity`]. The others take no notice.
    fn set_resource_dir(&mut self, _dir: &Path) {}

    /// The sample the plugin's channel holds, as its event 196 writes it,
    /// for a plugin that plays its channel's own sample: FL hands that one
    /// over with `FPD_SetSample`. Such a plugin names it among its
    /// [`Generator::sample_paths`] from then on. This allocates. The others
    /// take no notice.
    fn set_channel_sample(&mut self, _path: &str) {}

    /// Start a note on the first frame of the next block.
    fn note_on(&mut self, note: NoteOn);

    /// Let go of the note the host gave this number, on the first frame of
    /// the next block. The note may sound on for [`Generator::tail`]
    /// frames. Nothing happens for a number no sounding note has.
    fn note_off(&mut self, voice: u32);

    /// Let go of every note, as [`Generator::note_off`] does for one.
    fn all_notes_off(&mut self);

    /// The pitch of the note the host gave this number has moved, from the
    /// first frame of the next block: a slide note bends it, or a clip
    /// moves its channel's pitch. `pitch` is in cents from key 60, as
    /// `(key - 60) * 100 + fine_pitch` of its [`NoteOn`] was, and stands
    /// in for that from now on: it is FL's `FinalLevels.Pitch`, which FL's
    /// host works out again on every tick (`FUN_00d265e0`) and a plugin
    /// reads as its voices run. A note let go of may still be told until
    /// its release is over.
    ///
    /// Nothing happens for a number no sounding note has, nor in a plugin
    /// that does not say: its notes keep the pitch they started with.
    fn set_note_pitch(&mut self, _voice: u32, _pitch: f32) {}

    /// The gain the channel's volume makes, from the next block on: FL's
    /// `chan.Vol`, which its host puts in `FinalLevels.Vol` with the
    /// note's velocity and a host that does not hand it over puts on what
    /// the generator plays. A plugin that raises `FinalLevels.Vol` to a
    /// power takes it from here and plays as if the host's were not on
    /// top; the others take no notice. 1 until the host says.
    fn set_channel_volume(&mut self, _volume: f32) {}

    /// A tick of the song starts with the next block, as
    /// [`Effect::tick`](crate::Effect::tick) has it. A host calls this for
    /// a generator that has [`Generator::controller_outputs`], once for
    /// every tick and in order, after the notes due on the tick's first
    /// frame. The others take no notice. This allocates nothing.
    fn tick(&mut self, _ppq_tick: u64, _tempo: f64) {}

    /// What the plugin puts out beside sound, for a generator that is an
    /// internal controller (Fruity Envelope Controller and its kind): one
    /// value from 0 to 1 for every output, as the last
    /// [`Generator::tick`] left them, as
    /// [`Effect::controller_outputs`](crate::Effect::controller_outputs)
    /// has them. Empty for a generator that is no controller, and the
    /// length never changes.
    fn controller_outputs(&self) -> &[f32] {
        &[]
    }

    /// Whether the last [`Generator::tick`] moved one of the
    /// [`Generator::controller_outputs`]. Asking takes the answer away.
    fn controller_changed(&mut self) -> bool {
        false
    }

    /// Add one block of the generator's sound to what `left` and `right`
    /// hold. The two sides are as long as each other.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]);

    /// How many parameters the plugin has: [`Generator::param`] and
    /// [`Generator::set_param`] know the indexes under this. A plugin that
    /// does not say has none a host can move.
    fn param_count(&self) -> usize {
        0
    }

    /// What a parameter is set to. 0 for a parameter the plugin does not
    /// have.
    fn param(&self, index: usize) -> f32;

    /// Set a parameter to the whole number nearest to `value`, kept inside
    /// the range of its control. Nothing happens for a parameter the plugin
    /// does not have.
    fn set_param(&mut self, index: usize, value: f32);

    /// Cut every note where it is, as when the song stops. The parameters
    /// stay.
    fn reset(&mut self);

    /// The most frames a note sounds on for after its
    /// [`Generator::note_off`], with the state, the parameters and the rate
    /// the plugin has now. A host plays that many frames past the last
    /// note of a song.
    fn tail(&self) -> usize;

    /// Whether the plugin, with the state and the parameters it has now,
    /// does what FL's does. A plugin that does not say does.
    fn fidelity(&self) -> Fidelity {
        Fidelity::Exact
    }

    /// How many frames late the plugin's output is. A plugin that does not
    /// say is on time.
    fn latency(&self) -> usize {
        0
    }

    /// The sample files the state names, each once, as the state writes
    /// them: `%FLStudioFactoryData%\Data\Patches\..` and the like. A host
    /// looks each one up where it looks a channel's sample up, decodes it
    /// and hands it over in [`Generator::set_sample`], so a path with no
    /// file is in the host's own list of the paths it did not find. This
    /// allocates. A plugin that does not say plays no file.
    fn sample_paths(&self) -> Vec<String> {
        Vec::new()
    }

    /// Take the decoded file of one of the [`Generator::sample_paths`], or
    /// with `None` hear that there is no such file: what plays it then
    /// stays silent, and [`Generator::fidelity`] says a file is missing.
    /// This allocates. Nothing happens for a path the state does not name.
    fn set_sample(&mut self, _path: &str, _file: Option<SampleFile<'_>>) {}

    /// The plugins inside this one that are not built and are passed
    /// through or left silent, by their event 201 names, for a plugin that
    /// hosts others, as FL's Patcher does. Empty for the others.
    fn unhosted(&self) -> &[String] {
        &[]
    }
}

/// A decoded sample file for [`Generator::set_sample`]. A mono file has no
/// `right`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SampleFile<'a> {
    pub left: &'a [f32],
    pub right: Option<&'a [f32]>,
    /// The file's own rate, in Hz.
    pub rate: u32,
}

/// A way to make a generator with its defaults.
type Make = fn() -> Box<dyn Generator>;

/// Every generator there is, by its event 201 name.
const GENERATORS: &[(&str, Make)] = &[
    (crate::autogun::NAME, || {
        Box::new(crate::autogun::Autogun::default())
    }),
    (crate::bassdrum::NAME, || {
        Box::new(crate::bassdrum::BassDrum::default())
    }),
    (crate::beepmap::NAME, || {
        Box::new(crate::beepmap::BeepMap::default())
    }),
    (crate::boobass::NAME, || {
        Box::new(crate::boobass::BooBass::default())
    }),
    (crate::directwave::NAME, || {
        Box::new(crate::directwave::DirectWave::default())
    }),
    (crate::drumaxx::NAME, || {
        Box::new(crate::drumaxx::Drumaxx::default())
    }),
    (crate::drumpad::NAME, || {
        Box::new(crate::drumpad::Drumpad::default())
    }),
    (crate::drumsynth_live::NAME, || {
        Box::new(crate::drumsynth_live::DrumSynthLive::default())
    }),
    (crate::dx10::NAME, || Box::new(crate::dx10::Dx10::default())),
    (crate::envelope_controller::NAME, || {
        Box::new(crate::envelope_controller::EnvelopeController::default())
    }),
    (crate::fl_keys::NAME, || {
        Box::new(crate::fl_keys::FlKeys::default())
    }),
    (crate::fl_slayer::NAME, || {
        Box::new(crate::fl_slayer::FlSlayer::default())
    }),
    (crate::fpc::NAME, || Box::new(crate::fpc::Fpc::default())),
    (crate::fruit_kick::NAME, || {
        Box::new(crate::fruit_kick::FruitKick::default())
    }),
    (crate::fruity_slicer::NAME, || {
        Box::new(crate::fruity_slicer::FruitySlicer::default())
    }),
    (crate::fruity_slicer_2::NAME, || {
        Box::new(crate::fruity_slicer_2::FruitySlicer2::default())
    }),
    (crate::gms::NAME, || Box::new(crate::gms::Gms::default())),
    (crate::granulizer::NAME, || {
        Box::new(crate::granulizer::Granulizer::default())
    }),
    (crate::harmless::NAME, || {
        Box::new(crate::harmless::Harmless::default())
    }),
    (crate::harmor::NAME, || {
        Box::new(crate::harmor::Harmor::default())
    }),
    (crate::kepler::NAME, || {
        Box::new(crate::kepler::Kepler::default())
    }),
    (crate::kepler_exo::NAME, || {
        Box::new(crate::kepler_exo::KeplerExo::default())
    }),
    (crate::keyboard_controller::NAME, || {
        Box::new(crate::keyboard_controller::KeyboardController::default())
    }),
    (crate::minisynth::NAME, || {
        Box::new(crate::minisynth::MiniSynth::default())
    }),
    (crate::ogun::NAME, || Box::new(crate::ogun::Ogun::default())),
    (crate::patcher::NAME, || {
        Box::new(crate::patcher::PatcherGenerator::default())
    }),
    (crate::three_osc::NAME, || {
        Box::new(crate::three_osc::ThreeOsc::default())
    }),
    (crate::slicex::NAME, || {
        Box::new(crate::slicex::Slicex::default())
    }),
    (crate::sytrus::NAME, || {
        Box::new(crate::sytrus::Sytrus::default())
    }),
    (crate::wave_traveller::NAME, || {
        Box::new(crate::wave_traveller::WaveTraveller::default())
    }),
    #[cfg(any(test, feature = "test-generators"))]
    (crate::test_sine::NAME, || {
        Box::new(crate::test_sine::TestSine::default())
    }),
];

/// The generator FL loads under this event 201 name, set to its defaults.
/// `None` for a plugin that is not built yet. Case does not count, as for
/// [`by_name`](crate::by_name): FL's own presets name Fruit Kick "Fruit
/// kick".
pub fn generator_by_name(name: &str) -> Option<Box<dyn Generator>> {
    let known = |(known, _): &&(&str, Make)| name.eq_ignore_ascii_case(known);
    GENERATORS.iter().find(known).map(|(_, make)| make())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_four_generators_are_found_with_their_defaults_and_no_other_of_fl_s() {
        // The name, as FL's projects and as its presets have it, and what
        // the parameters start at.
        // 3x Osc's 21, two that are nobody's, and the instrument tab's.
        let tab = *crate::three_osc::ThreeOsc::default().articulation();
        let osc: Vec<f32> = [0, 0, 0, 0, 0, 0, 64, 0, 0, -12, 0, 0, 0, 32, 0, 0, -24]
            .iter()
            .chain(&[0; 6])
            .chain(tab.blocks.as_flattened())
            .chain(&[0, 0, tab.cutoff, tab.resonance, 0, tab.filter])
            .map(|value| *value as f32)
            .collect();
        let drum = [
            0.5, 0.5, 0.5, 0.0, 0.0, 0.0, 0.1, 0.4, 0.4, 0.5, 0.5, 0.5, 0.0, 0.5, 0.5, 0.0, 0.0,
            1.0, 1.0, 0.5, 0.5, 0.0, 1.0,
        ];
        let built: [(&[&str], &[f32]); 4] = [
            (&["3x Osc", "3x osc"], &osc),
            (&["BassDrum", "bassdrum"], &drum),
            (
                &["Fruit Kick", "Fruit kick"],
                &[0.0, -600.0, 64.0, 128.0, 32.0, 0.0],
            ),
            (&["BooBass", "boobass"], &[32_767.0; 3]),
        ];
        for (names, defaults) in built {
            for name in names {
                let mut generator = generator_by_name(name).unwrap();
                let params: Vec<f32> = (0..defaults.len()).map(|i| generator.param(i)).collect();
                assert_eq!(params, defaults, "{name}");
                assert_eq!(generator.param_count(), defaults.len(), "{name}");
                assert_eq!(generator.param(defaults.len()), 0.0, "{name}");
                // A host can hand every one of them a tempo and a place to
                // look for FL's files.
                generator.set_tempo(120.0);
                generator.set_sample_rate(48_000.0);
                generator.set_resource_dir(Path::new("/nowhere"));
                generator.process(&mut [0.5; 8], &mut [-0.5; 8]);
                assert!(generator.tail() > 0, "{name}");
            }
        }
        for name in ["3xOsc", "FLEX", "Fruity Wrapper", ""] {
            assert!(generator_by_name(name).is_none(), "{name}");
        }
        // An effect is no generator, and a generator no effect.
        assert!(generator_by_name("Fruity Balance").is_none());
        assert!(crate::by_name("Fruit Kick").is_none());
        assert!(crate::by_name("BooBass").is_none());
        assert!(crate::by_name("3x Osc").is_none());
        assert!(crate::by_name("BassDrum").is_none());
    }

    #[test]
    fn the_test_sine_is_found_in_any_case() {
        assert!(generator_by_name("TestSine").is_some());
        assert!(generator_by_name("testsine").is_some());
        assert!(generator_by_name("TestSine ").is_none());
        assert!(crate::by_name("TestSine").is_none());
    }

    #[test]
    fn every_generator_says_how_close_it_is_to_fl() {
        for (name, _) in GENERATORS {
            let port = crate::port::port(name);
            assert!(port.is_some() || *name == crate::test_sine::NAME, "{name}");
        }
    }
}
