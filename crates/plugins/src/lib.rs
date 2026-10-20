//! FL Studio's stock effects, each rebuilt from what `docs/plugins/` reads
//! out of its DLL: the state it saves in event 213, what its parameters
//! mean, and what it does to the samples.
//!
//! An [`Effect`] works on one stereo block in place. [`by_name`] makes the
//! effect for the name a project's event 201 gives a slot's plugin.
//!
//! A [`Generator`] is a plugin on a channel: it is handed notes and adds
//! their sound to a block. [`generator_by_name`] makes the one for the name
//! a channel's event 201 has. So far: [`bassdrum::BassDrum`],
//! [`boobass::BooBass`], [`fruit_kick::FruitKick`], [`sytrus::Sytrus`] and
//! [`three_osc::ThreeOsc`].
//!
//! So far: [`balance::Balance`], [`blood_overdrive::BloodOverdrive`],
//! [`chorus::Chorus`], [`compressor::Compressor`],
//! [`convolver::Convolver`], [`delay_2::Delay2`], [`delay_3::Delay3`],
//! [`delay_bank::DelayBank`], [`fast_dist::FastDist`], [`fast_lp::FastLp`], [`filter::Filter`],
//! [`flangus::Flangus`],
//! [`limiter::Limiter`], [`love_philter::LovePhilter`],
//! [`maximus::Maximus`],
//! [`multiband_compressor::MultibandCompressor`], [`panomatic::PanOMatic`],
//! [`parametric_eq_2::ParametricEq2`],
//! [`peak_controller::PeakController`], [`pitcher::Pitcher`],
//! [`reeverb_2::Reeverb2`],
//! [`soft_clipper::SoftClipper`], [`soundgoodizer::Soundgoodizer`],
//! [`spreader::Spreader`],
//! [`stereo_enhancer::StereoEnhancer`], [`stereo_shaper::StereoShaper`],
//! [`transient_processor::TransientProcessor`] and
//! [`waveshaper::WaveShaper`].
//!
//! Fruity Peak Controller is an internal controller: beside its sound it
//! has [`Effect::controller_outputs`], which a host carries to the controls
//! its project links them to, once for every [`Effect::tick`].

pub mod autogun;
mod autogun_engine;
pub mod balance;
pub mod bassdrum;
pub mod blood_overdrive;
pub mod boobass;
pub mod center;
pub mod chorus;
pub mod compressor;
pub mod control_surface;
pub mod convolver;
pub mod db_meter;
pub mod delay;
pub mod delay_2;
pub mod delay_3;
pub mod delay_bank;
pub mod directwave;
pub mod drumpad;
mod drumpad_voice;
pub mod dx10;
pub mod effector;
pub mod equo;
pub mod fast_dist;
pub mod fast_lp;
pub mod filter;
pub mod fl_fx;
pub mod fl_keys;
pub mod flangus;
pub mod fpc;
pub mod frequency_splitter;
pub mod fruit_kick;
mod generator;
pub mod gross_beat;
pub mod harmless;
pub mod harmor;
mod harmor_art;
mod harmor_fx;
mod harmor_state;
pub mod html_notebook;
pub mod hyper_chorus;
pub mod limiter;
pub mod love_philter;
pub mod luxeverb;
pub mod maximus;
mod menvelope;
pub mod multiband_compressor;
pub mod mute_2;
pub mod notebook;
pub mod notebook_2;
pub mod ogun;
pub mod panomatic;
pub mod parametric_eq_2;
mod parts;
pub mod pass_through;
pub mod peak_controller;
pub mod phase_inverter;
pub mod pitch_shifter;
pub mod pitcher;
pub mod port;
pub mod razer_chroma;
pub mod reeverb;
pub mod reeverb_2;
mod sampler;
pub mod send;
pub mod slicex;
pub mod soft_clipper;
pub mod soundgoodizer;
pub mod spreader;
pub mod squeeze;
pub mod stereo_enhancer;
pub mod stereo_shaper;
pub mod sytrus;
#[cfg(any(test, feature = "test-generators"))]
pub mod test_sine;
#[cfg(test)]
mod testing;
pub mod three_osc;
pub mod transient_processor;
pub mod vocodex;
pub mod wave_candy;
pub mod waveshaper;
pub mod wrapper;
pub mod xy_controller;

pub use generator::{Generator, NoteOn, SampleFile, generator_by_name};
use std::fmt;
use std::path::Path;

/// One plugin in an effect slot.
///
/// A parameter is numbered as FL numbers it, and its value is the integer
/// FL saves for it, as an `f32`.
pub trait Effect: Send {
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
    /// `Plugins/Fruity/Generators` directory, the one
    /// [`Generator::set_resource_dir`] is told. An effect whose sound is in
    /// a file FL ships finds the file from there, and this may read files:
    /// Fruity Convolver's impulses are under `Data`, which is next to
    /// `Plugins`. A host that has no such directory does not call this, and
    /// such a plugin then says what it lacks in [`Effect::fidelity`]. The
    /// others take no notice.
    fn set_resource_dir(&mut self, _dir: &Path) {}

    /// A tick of the song starts with the next block: FL's `NewTick`,
    /// which it calls before it mixes the tick. `ppq_tick` is the tick's
    /// place in the song and `tempo` the song's tempo there, in BPM.
    ///
    /// A host calls this for a plugin that has
    /// [`Effect::controller_outputs`] or [`Effect::follows_song`], once for
    /// every tick and in order while the song plays on, and cuts its blocks
    /// so that the tick's first frame is the first of the next
    /// [`Effect::process`]. A tick that does not follow the one before it
    /// says the song was started or moved. A plugin that does not say takes
    /// no notice. This allocates nothing.
    fn tick(&mut self, _ppq_tick: u64, _tempo: f64) {}

    /// Whether the plugin keeps in step with the song's place and so wants
    /// every [`Effect::tick`], as Gross Beat does. A plugin that does not
    /// say does not.
    fn follows_song(&self) -> bool {
        false
    }

    /// What the plugin puts out beside sound, for a plugin that is an
    /// internal controller: one value from 0 to 1 for every output it has,
    /// as the last [`Effect::tick`] left them. FL counts these in 2^30ths
    /// and a link carries one to the control it drives, where 0 is the
    /// bottom of the control's range and 1 the top. Empty for a plugin that
    /// is no controller, and the length never changes.
    fn controller_outputs(&self) -> &[f32] {
        &[]
    }

    /// Whether the last [`Effect::tick`] moved one of the
    /// [`Effect::controller_outputs`], which is when FL's plugin calls its
    /// host's `OnControllerChanged`. Asking takes the answer away: the next
    /// call says no until a tick moves an output again.
    fn controller_changed(&mut self) -> bool {
        false
    }

    /// Run one block through the effect, in place. The two sides are as
    /// long as each other. This allocates nothing, so an audio callback can
    /// run it.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]);

    /// How many parameters the plugin has: [`Effect::param`] and
    /// [`Effect::set_param`] know the indexes under this. A plugin that
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

    /// Forget what the blocks so far left behind, as when the song stops.
    /// The parameters stay.
    fn reset(&mut self);

    /// Whether the plugin, with the state and the parameters it has now,
    /// does what FL's does. A plugin that does not say does.
    fn fidelity(&self) -> Fidelity {
        Fidelity::Exact
    }

    /// How many frames late the plugin's output is, with the state, the
    /// parameters and the tempo it has now. A host holds every other path
    /// back by as many, so that they all come out together. A plugin that
    /// does not say is on time.
    fn latency(&self) -> usize {
        0
    }

    /// The most [`Effect::latency`] can come to at the rate the plugin has
    /// and at its tempo or a faster one, whatever its parameters are. A
    /// host sizes its delay lines by this once. A plugin that does not say
    /// is never later than it is now.
    fn max_latency(&self) -> usize {
        self.latency()
    }

    /// What the plugin made of the last block beside its output, for a
    /// plugin with a second output: the left side and the right, each as
    /// long as that block. `None` for a plugin that has none, or has it
    /// switched off.
    ///
    /// Stereo Shaper's difference is the one there is. A host adds it to
    /// the bus of the insert [`Effect::sidechain_output`] counts to.
    fn aux_output(&self) -> Option<(&[f32], &[f32])> {
        None
    }

    /// Where [`Effect::aux_output`] goes, as the plugin's state numbers
    /// the inserts its own insert sends to: 1 is the first of them. 0 for
    /// a plugin without a second output, or with it switched off.
    fn sidechain_output(&self) -> usize {
        0
    }

    /// The sidechain input the plugin's state chose, as FL numbers the
    /// inserts that send to the plugin's insert: 1 is the first of them. 0
    /// for a plugin that has none, or has none chosen. The parameters may
    /// leave the plugin with no use for the input it chose.
    fn sidechain_input(&self) -> usize {
        0
    }

    /// Every sidechain input the plugin's state chose, for a plugin that
    /// listens to two, numbered as [`Effect::sidechain_input`] numbers
    /// them, 0 for none. A host hands each one it has to
    /// [`Effect::set_sidechain`]. A plugin that does not say listens to
    /// its [`Effect::sidechain_input`] alone.
    fn sidechain_inputs(&self) -> [usize; 2] {
        [self.sidechain_input(), 0]
    }

    /// Take one block of sidechain input `index` for the next
    /// [`Effect::process`], whose block is as long. A host that has the
    /// input [`Effect::sidechain_input`] names calls this ahead of every
    /// `process`. For a block it was handed none, the plugin listens to
    /// its own input, as FL's does when its host has no buffer for the
    /// number.
    ///
    /// A plugin copies the block. That asks for memory when the block is
    /// longer than any before it and longer than 4096 frames, and never
    /// otherwise. A plugin that does not say takes no notice.
    fn set_sidechain(&mut self, _index: usize, _left: &[f32], _right: &[f32]) {}
}

/// How near a plugin comes to FL's with the state it has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fidelity {
    /// Everything the state asks for is done as `docs/plugins/` has it.
    Exact,
    /// The state asks for something that another thing stands in for.
    /// The text says what, for a line in a report.
    Approximate(&'static str),
}

/// Why a plugin turned a state down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateError {
    /// The state is not as long as the plugin writes it.
    Length { expected: usize, found: usize },
    /// A parameter is outside the range of its control, which FL's own
    /// controls cannot save.
    Range { param: usize, value: i32 },
    /// The state starts with a version newer than the plugin reads. FL
    /// leaves such a state alone too.
    Version { found: i32 },
    /// The state holds another plugin, one this crate does not host, as a
    /// Fruity Wrapper's can.
    Unhosted,
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StateError::Length { expected, found } => {
                write!(f, "the state is {found} bytes and not {expected}")
            }
            StateError::Range { param, value } => {
                write!(f, "parameter {param} is {value}, outside its range")
            }
            StateError::Version { found } => {
                write!(f, "the state is of version {found}, which is too new")
            }
            StateError::Unhosted => write!(f, "the state holds a plugin that is not hosted"),
        }
    }
}

impl std::error::Error for StateError {}

/// The effects that leave the sound as it is, by their event 201 names.
const PASS_THROUGH: [&str; 6] = [
    db_meter::NAME,
    html_notebook::NAME,
    notebook::NAME,
    notebook_2::NAME,
    razer_chroma::NAME,
    wave_candy::NAME,
];

/// The effect FL loads under this event 201 name, set to its defaults.
/// `None` for a plugin that is not built yet.
///
/// Case does not count: FL's own presets name Fast Dist "Fruity fast dist".
pub fn by_name(name: &str) -> Option<Box<dyn Effect>> {
    let is = |known: &str| name.eq_ignore_ascii_case(known);
    if is(balance::NAME) {
        Some(Box::new(balance::Balance::default()))
    } else if is(center::NAME) {
        Some(Box::new(center::Center::default()))
    } else if is(mute_2::NAME) {
        Some(Box::new(mute_2::Mute2::default()))
    } else if is(phase_inverter::NAME) {
        Some(Box::new(phase_inverter::PhaseInverter::default()))
    } else if is(blood_overdrive::NAME) {
        Some(Box::new(blood_overdrive::BloodOverdrive::default()))
    } else if is(chorus::NAME) {
        Some(Box::new(chorus::Chorus::default()))
    } else if is(compressor::NAME) {
        Some(Box::new(compressor::Compressor::default()))
    } else if is(convolver::NAME) {
        Some(Box::new(convolver::Convolver::default()))
    } else if is(delay::NAME) {
        Some(Box::new(delay::Delay::default()))
    } else if is(delay_2::NAME) {
        Some(Box::new(delay_2::Delay2::default()))
    } else if is(delay_3::NAME) {
        Some(Box::new(delay_3::Delay3::default()))
    } else if is(delay_bank::NAME) {
        Some(Box::new(delay_bank::DelayBank::default()))
    } else if is(effector::NAME) {
        Some(Box::new(effector::Effector::default()))
    } else if is(equo::NAME) {
        Some(Box::new(equo::Equo::default()))
    } else if is(fast_dist::NAME) {
        Some(Box::new(fast_dist::FastDist::default()))
    } else if is(fast_lp::NAME) {
        Some(Box::new(fast_lp::FastLp::default()))
    } else if is(filter::NAME) {
        Some(Box::new(filter::Filter::default()))
    } else if is(flangus::NAME) {
        Some(Box::new(flangus::Flangus::default()))
    } else if is(frequency_splitter::NAME) {
        Some(Box::new(frequency_splitter::FrequencySplitter::default()))
    } else if is(gross_beat::NAME) {
        Some(Box::new(gross_beat::GrossBeat::default()))
    } else if is(hyper_chorus::NAME) {
        Some(Box::new(hyper_chorus::HyperChorus::default()))
    } else if is(limiter::NAME) {
        Some(Box::new(limiter::Limiter::default()))
    } else if is(love_philter::NAME) {
        Some(Box::new(love_philter::LovePhilter::default()))
    } else if is(luxeverb::NAME) {
        Some(Box::new(luxeverb::Luxeverb::default()))
    } else if is(maximus::NAME) {
        Some(Box::new(maximus::Maximus::default()))
    } else if is(multiband_compressor::NAME) {
        Some(Box::new(
            multiband_compressor::MultibandCompressor::default(),
        ))
    } else if is(panomatic::NAME) {
        Some(Box::new(panomatic::PanOMatic::default()))
    } else if is(parametric_eq_2::NAME) {
        Some(Box::new(parametric_eq_2::ParametricEq2::default()))
    } else if is(peak_controller::NAME) {
        Some(Box::new(peak_controller::PeakController::default()))
    } else if is(pitch_shifter::NAME) {
        Some(Box::new(pitch_shifter::PitchShifter::default()))
    } else if is(pitcher::NAME) {
        Some(Box::new(pitcher::Pitcher::default()))
    } else if is(reeverb::NAME) {
        Some(Box::new(reeverb::Reeverb::default()))
    } else if is(reeverb_2::NAME) {
        Some(Box::new(reeverb_2::Reeverb2::default()))
    } else if is(send::NAME) {
        Some(Box::new(send::Send::default()))
    } else if is(soft_clipper::NAME) {
        Some(Box::new(soft_clipper::SoftClipper::default()))
    } else if is(soundgoodizer::NAME) {
        Some(Box::new(soundgoodizer::Soundgoodizer::default()))
    } else if is(spreader::NAME) {
        Some(Box::new(spreader::Spreader::default()))
    } else if is(squeeze::NAME) {
        Some(Box::new(squeeze::Squeeze::default()))
    } else if is(stereo_enhancer::NAME) {
        Some(Box::new(stereo_enhancer::StereoEnhancer::default()))
    } else if is(stereo_shaper::NAME) {
        Some(Box::new(stereo_shaper::StereoShaper::default()))
    } else if is(transient_processor::NAME) {
        Some(Box::new(transient_processor::TransientProcessor::default()))
    } else if is(vocodex::NAME) {
        Some(Box::new(vocodex::Vocodex::default()))
    } else if is(waveshaper::NAME) {
        Some(Box::new(waveshaper::WaveShaper::default()))
    } else if PASS_THROUGH.iter().any(|known| is(known)) {
        Some(Box::new(pass_through::PassThrough::new()))
    } else if is(control_surface::NAME) {
        Some(Box::new(control_surface::ControlSurface::default()))
    } else if is(xy_controller::NAME) {
        Some(Box::new(xy_controller::XyController::default()))
    } else if is(wrapper::NAME) {
        Some(Box::new(wrapper::Wrapper::default()))
    } else {
        None
    }
}

/// One block of a sidechain input, kept from [`Effect::set_sidechain`] to
/// the [`Effect::process`] after it.
#[derive(Debug, Clone, PartialEq)]
struct Sidechain {
    left: Vec<f32>,
    right: Vec<f32>,
}

impl Default for Sidechain {
    /// With room for a block of [`Sidechain::ROOM`] frames.
    fn default() -> Self {
        Sidechain {
            left: Vec::with_capacity(Self::ROOM),
            right: Vec::with_capacity(Self::ROOM),
        }
    }
}

impl Sidechain {
    /// The longest block that is kept without asking for memory.
    const ROOM: usize = 4096;

    /// Keep a block, in place of the one before.
    fn keep(&mut self, left: &[f32], right: &[f32]) {
        let len = left.len().min(right.len());
        self.clear();
        self.left.extend_from_slice(&left[..len]);
        self.right.extend_from_slice(&right[..len]);
    }

    /// The frame at `at` of the block that is kept, when it is that long.
    fn frame(&self, at: usize) -> Option<[f32; 2]> {
        Some([*self.left.get(at)?, *self.right.get(at)?])
    }

    fn clear(&mut self) {
        self.left.clear();
        self.right.clear();
    }
}

/// The curve FL's gain knobs share: `(11^x - 1) / 10`, which is 0 at 0 and
/// 1 at 1. Where a knob is at 1 exactly, its plugin says so itself.
fn knob_gain(x: f64) -> f64 {
    ((x * 11f64.ln()).exp() - 1.0) * 0.1
}

/// A state of `N` little-endian i32 parameters, each inside its range.
fn read_state<const N: usize>(
    bytes: &[u8],
    ranges: &[(i32, i32); N],
) -> Result<[i32; N], StateError> {
    if bytes.len() != 4 * N {
        return Err(StateError::Length {
            expected: 4 * N,
            found: bytes.len(),
        });
    }
    let mut params = [0; N];
    for (param, word) in bytes.as_chunks::<4>().0.iter().enumerate() {
        let value = i32::from_le_bytes(*word);
        let (min, max) = ranges[param];
        if value < min || value > max {
            return Err(StateError::Range { param, value });
        }
        params[param] = value;
    }
    Ok(params)
}

/// The version a state starts with, for the plugins that save one. A state
/// too short to have it is as wrong as one of any other length, and
/// `expected` is the length the plugin writes.
fn state_version(bytes: &[u8], expected: usize) -> Result<i32, StateError> {
    match bytes.first_chunk::<4>() {
        Some(word) => Ok(i32::from_le_bytes(*word)),
        None => Err(StateError::Length {
            expected,
            found: bytes.len(),
        }),
    }
}

/// What [`Effect::set_param`] makes of a value for a control with this
/// range. A NaN counts as 0.
fn knob_value(value: f32, (min, max): (i32, i32)) -> i32 {
    (value.round() as i32).clamp(min, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plugin_is_found_by_its_201_name_in_any_case() {
        assert!(by_name("Fruity Balance").is_some());
        assert!(by_name("Fruity Fast Dist").is_some());
        // What the presets FL ships have.
        assert!(by_name("Fruity fast dist").is_some());
        assert!(by_name("FRUITY BALANCE").is_some());
        assert!(by_name("fruity reeverb 2").is_some());
        assert!(by_name("Fruity Reeverb").is_some());
        assert!(by_name("Fruity Reeverb 3").is_none());
        assert!(by_name("Fruity Balance ").is_none());
        assert!(by_name("").is_none());
    }

    #[test]
    fn every_new_plugin_is_found_with_its_defaults() {
        // The name, how many parameters it has, and what they start at.
        let eq: Vec<f32> = [
            [0.0; 7],
            [
                10923.0, 18204.0, 25486.0, 32768.0, 40050.0, 47332.0, 54613.0,
            ],
            [
                40124.0, 25412.0, 25412.0, 25412.0, 25412.0, 25412.0, 40124.0,
            ],
            [5.0, 6.0, 6.0, 6.0, 6.0, 6.0, 7.0],
            [0.0; 7],
        ]
        .concat();
        let eq = [eq, vec![0.0]].concat();
        // Maximus's defaults, and Soundgoodizer's preset A at 600.
        let band = [
            0.0, 1000.0, 1000.0, 0.0, 0.0, 1000.0, 1115.0, 0.0, 2.0, 5000.0, 3.0, 2882.0, 0.0,
        ];
        let maximus: Vec<f32> = [
            vec![19_800.0, 1.0, 48_065.0, 1.0, 1115.0, 1000.0, 32_768.0],
            band.to_vec(),
            band.to_vec(),
            band.to_vec(),
            band.to_vec(),
        ]
        .concat();
        let preset = &soundgoodizer::PRESETS[0].params;
        let mut goodizer: Vec<f32> = preset.iter().map(|v| *v as f32).collect();
        goodizer[5] = 600.0;
        let luxeverb: Vec<f32> = [
            721, 12991, 20, 1, 11625, 326, 192, 253, 40, 20, 47, 20, 403, 0, 1, 0, 120, 2988, 192,
            0, 1719, 720, 1000, 0, -4000, 50, 400, 0, 0, 0, 301, 10, 746, 746, 1570, 315, 865, 120,
            1, 0, 0, 24, 20, 1719, 20, 1000, 192, 1000, 192,
        ]
        .iter()
        .map(|v| *v as f32)
        .collect();
        let plugins: [(&str, &[f32]); 20] = [
            ("Luxeverb", &luxeverb),
            (
                "Fruity Flangus",
                &[4.0, 512.0, 768.0, 8.0, 128.0, -32.0, 96.0, 96.0],
            ),
            (
                "Fruity Blood Overdrive",
                &[0.0, 5000.0, 0.0, 0.0, 0.0, 10_000.0],
            ),
            (
                "Fruity Chorus",
                &[
                    512.0, 2250.0, 341.0, 1500.0, 2500.0, 3500.0, 23.0, 0.0, 0.0, 0.0, 2500.0, 0.0,
                ],
            ),
            ("Spreader", &[5000.0, 0.0, 1.0, 0.0, 1.0]),
            (
                "Fruity PanOMatic",
                &[128.0, 256.0, 0.0, 0.0, 64.0, 27_500.0],
            ),
            (
                "Fruity Peak Controller",
                &[0.0, 0.0, 0.0, 66.0, 0.0, 0.0, 0.0, 32_950.0, 0.0, 0.0],
            ),
            (
                "Fruity Limiter",
                &[
                    1000.0, 1000.0, 1000.0, 1115.0, 3.0, 5000.0, 3.0, 2882.0, 1000.0, 0.0, 0.0,
                    0.0, 6666.0, 1.0, 650.0, 1000.0, 0.0, 4187.0,
                ],
            ),
            (
                "Fruity Reeverb 2",
                &[
                    75.0, 40.0, 0.0, 50.0, 100.0, 15.0, 40.0, 100.0, 500.0, 0.0, 128.0, 64.0, 64.0,
                    684.0, 0.0,
                ],
            ),
            (
                "Fruity Stereo Shaper",
                &[0.0, 12800.0, 12800.0, 0.0, 0.0, 0.0],
            ),
            ("Fruity WaveShaper", &[128.0, 128.0, 128.0]),
            ("Fruity Compressor", &[0.0, 10.0, 0.0, 150.0, 200.0, 0.0]),
            (
                "Fruity Delay 2",
                &[0.0, 128.0, 128.0, 64.0, 144.0, 0.0, 0.0, 128.0],
            ),
            ("Fruity Fast LP", &[1024.0, 0.0]),
            ("Fruity Filter", &[1024.0, 205.0, 1024.0, 0.0, 512.0, 0.0]),
            ("Fruity Parametric EQ 2", &eq),
            ("Maximus", &maximus),
            ("Soundgoodizer", &goodizer),
            ("Fruity Soft Clipper", &[100.0, 128.0]),
            ("Fruity Stereo Enhancer", &[0.0, 256.0, 0.0, 0.0, 1.0, 0.0]),
        ];
        for (name, defaults) in plugins {
            for name in [name.to_owned(), name.to_lowercase()] {
                let mut effect = by_name(&name).unwrap();
                let params: Vec<f32> = (0..defaults.len()).map(|i| effect.param(i)).collect();
                assert_eq!(params, defaults, "{name}");
                assert_eq!(effect.param(defaults.len()), 0.0, "{name}");
                assert_eq!(effect.param_count(), defaults.len(), "{name}");
                // The host can give every one of them a tempo.
                effect.set_tempo(120.0);
                effect.set_ppq(96);
                effect.set_sample_rate(48_000.0);
                effect.process(&mut [0.5; 8], &mut [-0.5; 8]);
                // And a tick. Only the controller has outputs to move.
                effect.tick(0, 120.0);
                let controller = name.eq_ignore_ascii_case(peak_controller::NAME);
                assert_eq!(
                    effect.controller_outputs().len(),
                    if controller { 3 } else { 0 }
                );
                assert_eq!(effect.controller_changed(), controller, "{name}");
            }
        }
    }

    #[test]
    fn every_state_in_fl_s_projects_loads() {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, the states in FL's projects skipped");
            return;
        };
        // How many states of each length every plugin has.
        let mut loaded = std::collections::BTreeMap::new();
        for saved in testing::projects(&root) {
            let Some(mut effect) = by_name(&saved.plugin) else {
                continue;
            };
            match effect.set_state(&saved.state) {
                // A wrapper of an effect not built yet, which
                // `wrapper::tests` counts.
                Err(StateError::Unhosted) if saved.plugin == wrapper::NAME => continue,
                Err(error) => panic!("{} in {}: {error}", saved.plugin, saved.file),
                Ok(()) => {}
            }
            let plugin = saved.plugin.to_ascii_lowercase();
            *loaded.entry((plugin, saved.state.len())).or_insert(0) += 1;
        }
        // What the docs count in the 165 projects, but for Soft Clipper,
        // where the doc has 27.
        for (plugin, len, uses) in [
            ("effector", 316, 27),
            ("equo", 4143, 10),
            ("fruity compressor", 28, 2),
            ("fruity compressor", 32, 51),
            ("fruity compressor", 36, 35),
            ("fruity delay", 28, 1),
            ("fruity delay 2", 32, 62),
            ("fruity delay 3", 108, 110),
            ("fruity delay bank", 792, 41),
            ("fruity fast lp", 12, 17),
            ("fruity filter", 28, 20),
            ("fruity filter", 29, 12),
            ("fruity blood overdrive", 36, 26),
            ("fruity chorus", 52, 20),
            ("fruity flangus", 32, 56),
            ("fruity limiter", 168, 156),
            ("fruity limiter", 169, 190),
            ("fruity multiband compressor", 124, 35),
            ("fruity panomatic", 24, 193),
            ("fruity parametric eq 2", 305, 21),
            ("fruity parametric eq 2", 309, 328),
            ("fruity parametric eq 2", 350, 199),
            ("fruity parametric eq 2", 354, 590),
            ("fruity peak controller", 42, 36),
            ("fruity reeverb", 44, 7),
            ("fruity reeverb 2", 58, 62),
            ("fruity reeverb 2", 66, 271),
            ("fruity send", 16, 2),
            ("fruity soft clipper", 8, 28),
            ("fruity squeeze", 52, 2),
            ("fruity stereo enhancer", 24, 24),
            ("fruity stereo shaper", 36, 77),
            ("frequency splitter", 113, 17),
            ("hyper chorus", 52, 20),
            ("luxeverb", 165, 13),
            ("luxeverb", 169, 11),
            ("pitcher", 160, 61),
            ("pitcher", 164, 1),
            ("pitch shifter", 75, 15),
            ("spreader", 30, 33),
            ("transient processor", 49, 47),
        ] {
            let found = loaded.get(&(plugin.to_owned(), len)).copied();
            assert_eq!(found, Some(uses), "{plugin} at {len} bytes");
        }
        // Soundgoodizer is 12 bytes, and Maximus 1342 to 1462 with its
        // curves: 125 states, 53 of them the 1375 of version 15.
        assert_eq!(
            loaded.get(&("soundgoodizer".to_owned(), 12)).copied(),
            Some(97)
        );
        let maximus = loaded.iter().filter(|((p, _), _)| p == "maximus");
        assert_eq!(maximus.map(|(_, n)| *n).sum::<usize>(), 125);
        assert_eq!(loaded.get(&("maximus".to_owned(), 1375)).copied(), Some(53));
        // And no state of theirs has another length. WaveShaper's is as
        // long as its curve: 74 states, of 106 to 876 bytes.
        // Love Philter's and Convolver's are as long as their envelopes and
        // the impulse inside them: 50 and 21 states.
        for (plugin, uses) in [("fruity love philter", 50), ("fruity convolver", 21)] {
            let states = loaded.iter().filter(|((p, _), _)| p == plugin);
            assert_eq!(states.map(|(_, n)| *n).sum::<usize>(), uses, "{plugin}");
        }
        let others = [
            "fruity balance",
            "fruity convolver",
            "fruity fast dist",
            "fruity love philter",
            "fruity waveshaper",
            "gross beat",
            "maximus",
            "soundgoodizer",
            "vocodex",
            // Counted in `wrapper::tests`.
            "fruity wrapper",
            // Counted in their own tests.
            "control surface",
            "fruity x-y controller",
        ];
        // Any state does for an effect that leaves the sound alone.
        let passes = |plugin: &str| PASS_THROUGH.iter().any(|n| n.eq_ignore_ascii_case(plugin));
        let lengths = loaded
            .keys()
            .filter(|(plugin, _)| !others.contains(&plugin.as_str()) && !passes(plugin));
        assert_eq!(lengths.count(), 41);
        let shapers = loaded
            .iter()
            .filter(|((plugin, _), _)| plugin == "fruity waveshaper");
        let (lens, uses): (Vec<usize>, Vec<usize>) =
            shapers.map(|((_, len), uses)| (*len, *uses)).unzip();
        assert_eq!(uses.iter().sum::<usize>(), 74);
        assert_eq!((lens.first(), lens.last()), (Some(&106), Some(&876)));
    }

    #[test]
    fn the_limiter_the_reverb_and_the_shapers_run_every_state_in_fl_s_projects() {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, the states in FL's projects skipped");
            return;
        };
        let names = [
            limiter::NAME,
            reeverb_2::NAME,
            stereo_shaper::NAME,
            waveshaper::NAME,
        ];
        // A tone on the left and another on the right, 12 dB over full
        // scale at their loudest.
        let tone = |turns: f64| -> Vec<f32> {
            let loud = testing::sine(turns, 1024).into_iter().enumerate();
            loud.map(|(n, v)| v * n as f32 / 256.0).collect()
        };
        let mut run = 0;
        for saved in testing::projects(&root) {
            if !names
                .iter()
                .any(|name| saved.plugin.eq_ignore_ascii_case(name))
            {
                continue;
            }
            let mut effect = by_name(&saved.plugin).unwrap();
            effect.set_sample_rate(48_000.0);
            effect.set_tempo(140.0);
            effect.set_state(&saved.state).unwrap();
            let (mut left, mut right) = (tone(0.01), tone(0.037));
            for (l, r) in left.chunks_mut(300).zip(right.chunks_mut(300)) {
                effect.process(l, r);
            }
            let sound = left.iter().chain(&right);
            assert!(
                sound.clone().all(|v| v.is_finite() && v.abs() < 100.0),
                "{} in {}",
                saved.plugin,
                saved.file
            );
            assert!(effect.latency() <= effect.max_latency());
            run += 1;
        }
        assert_eq!(run, 346 + 333 + 77 + 74);
    }

    #[test]
    fn the_two_plugins_are_told_apart() {
        // Balance has 2 parameters and Fast Dist 5, and 256 and 128 are
        // their defaults.
        let balance = by_name("fruity balance").unwrap();
        assert_eq!((balance.param(1), balance.param(4)), (256.0, 0.0));
        let dist = by_name("fruity fast dist").unwrap();
        assert_eq!((dist.param(1), dist.param(4)), (10.0, 128.0));
    }

    #[test]
    fn the_knob_curve_runs_from_0_through_1_at_1() {
        assert_eq!(knob_gain(0.0), 0.0);
        assert!((knob_gain(1.0) - 1.0).abs() < 1e-12);
        assert!((knob_gain(0.5) - (11f64.sqrt() - 1.0) / 10.0).abs() < 1e-12);
        assert!((knob_gain(1.5) - 3.548).abs() < 1e-3);
    }

    #[test]
    fn a_state_of_the_wrong_length_or_out_of_range_is_turned_down() {
        let ranges = [(-5, 5), (0, 10)];
        let bytes =
            |values: &[i32]| -> Vec<u8> { values.iter().flat_map(|v| v.to_le_bytes()).collect() };
        assert_eq!(read_state(&bytes(&[-5, 10]), &ranges), Ok([-5, 10]));
        assert_eq!(
            read_state(&bytes(&[0]), &ranges),
            Err(StateError::Length {
                expected: 8,
                found: 4
            })
        );
        assert_eq!(
            read_state(&bytes(&[0, 0, 0]), &ranges)
                .unwrap_err()
                .to_string(),
            "the state is 12 bytes and not 8"
        );
        assert_eq!(
            read_state(&bytes(&[0, 11]), &ranges),
            Err(StateError::Range {
                param: 1,
                value: 11
            })
        );
    }

    #[test]
    fn a_state_starts_with_its_version() {
        assert_eq!(state_version(&[2, 0, 0, 0, 9], 29), Ok(2));
        assert_eq!(state_version(&(-1i32).to_le_bytes(), 12), Ok(-1));
        assert_eq!(
            state_version(&[1, 0, 0], 12),
            Err(StateError::Length {
                expected: 12,
                found: 3
            })
        );
        assert_eq!(
            StateError::Version { found: 3 }.to_string(),
            "the state is of version 3, which is too new"
        );
    }

    #[test]
    fn a_knob_takes_the_nearest_whole_number_inside_its_range() {
        assert_eq!(knob_value(3.4, (0, 10)), 3);
        assert_eq!(knob_value(3.5, (0, 10)), 4);
        assert_eq!(knob_value(-7.0, (0, 10)), 0);
        assert_eq!(knob_value(1e12, (0, 10)), 10);
        assert_eq!(knob_value(f32::NAN, (1, 10)), 1);
    }
}
