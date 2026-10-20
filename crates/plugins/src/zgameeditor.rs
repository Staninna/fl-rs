//! ZGameEditor Visualizer, in the cases its sound is its input: every layer
//! runs an effect that makes no sound and no video is loaded, as
//! `docs/plugins/zgameeditor-visualizer.md` reads the DLL.
//!
//! `Eff_Render` copies the input to the output with `ippsCopy_64s`, then
//! adds three things: the audio of a layer whose effect renders sound (it
//! defines `AudioOutputModel`, `AudioOutputRenderTarget`, `AudioOutputTime`
//! and `AudioOutputSampleRate`, as Misc\AudioGenerator does), the audio of
//! the video clips loaded as content, and ZGameEditor's own mixer, which
//! plays what an effect's `Sound` and `Music` components start. The state
//! is the visualizer's settings as INI text. A state whose layers all run
//! effects read to have none of these, with no video content, leaves the
//! sound as it came and is hosted; any other is turned down as
//! [`StateError::Unhosted`], and a host passes it through.

use crate::{Effect, StateError};

/// The plugin's event 201 name.
pub const NAME: &str = "ZGameEditor Visualizer";

/// The effects FL ships that the corpus's states run, each read to make no
/// sound: no `AudioOutput` producer, no `Sound`, `Music`, `MusicControl` or
/// mixer component, no video transport. `(none)` is an empty layer.
const SILENT: [&str; 32] = [
    "(none)",
    "Background\\FogMachine",
    "Background\\FourCornerGradient",
    "Background\\ItsFullOfStars",
    "Background\\SolidColor",
    "Blend\\BufferBlender",
    "Canvas effects\\Digital Brain",
    "Canvas effects\\Electric",
    "Canvas effects\\FreqRing",
    "Canvas effects\\ShimeringCage",
    "Image effects\\Image",
    "Misc\\Automator",
    "Object Arrays\\Rings",
    "Particles\\ColorBlobs",
    "Particles\\ReactiveFlow",
    "Particles\\ReactiveMob",
    "Peak Effects\\JoyDividers",
    "Peak Effects\\Linear",
    "Peak Effects\\Polar",
    "Peak Effects\\Reactive Sphere",
    "Peak Effects\\ReflectedPeeks",
    "Peak Effects\\WaveSimple",
    "Postprocess\\Ascii",
    "Postprocess\\AudioShake",
    "Postprocess\\Blooming",
    "Postprocess\\ColorCyclePalette",
    "Postprocess\\FrameBlur",
    "Postprocess\\RGB Shift",
    "Scenes\\Lantern",
    "Terrain\\CubesAndSpheres",
    "Terrain\\GoopFlow",
    "Text\\TextDraw",
];

/// The keys of the `[UserContent]` section that hold no sound, besides
/// `Images` and `VideoCues`, which are checked.
const CONTENT: [&str; 7] = [
    "Text",
    "Html",
    "Meshes",
    "MeshAutoScale",
    "MeshWithColors",
    "VideoUseSync",
    "Filtering",
];

/// The file types a still image comes in.
const STILLS: [&str; 4] = [".png", ".jpg", ".jpeg", ".bmp"];

/// Whether a state's INI text leaves the sound alone.
fn silent(text: &str) -> bool {
    let mut section = "";
    for line in text.lines() {
        let line = line.trim_start_matches('\u{feff}').trim_end();
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name;
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if section.starts_with("AppSet") && key == "App" && !SILENT.contains(&value) {
            return false;
        }
        if section == "UserContent" {
            let still = |file: &str| {
                let file = file.to_ascii_lowercase();
                STILLS.iter().any(|kind| file.ends_with(kind))
            };
            let fine = match key {
                "Images" => value.split(',').filter(|f| !f.is_empty()).all(still),
                "VideoCues" => value.is_empty(),
                _ => CONTENT.contains(&key),
            };
            if !fine {
                return false;
            }
        }
    }
    true
}

/// ZGameEditor Visualizer with a state that leaves the sound as it came.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ZGameEditor;

impl Effect for ZGameEditor {
    /// The state is a little-endian u32 length and that many bytes of
    /// UTF-8 INI text. Takes one whose layers and content make no sound.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let len = bytes
            .first_chunk::<4>()
            .map(|word| u32::from_le_bytes(*word) as usize);
        let text = len.and_then(|len| bytes.get(4..4usize.checked_add(len)?));
        let Some(text) = text else {
            return Err(StateError::Length {
                expected: len.map_or(4, |len| len.saturating_add(4)),
                found: bytes.len(),
            });
        };
        match std::str::from_utf8(text) {
            Ok(text) if silent(text) => Ok(()),
            _ => Err(StateError::Unhosted),
        }
    }

    fn set_sample_rate(&mut self, _hz: f32) {}

    /// The block stays as it came: `ippsCopy_64s`, and nothing added.
    fn process(&mut self, _left: &mut [f32], _right: &mut [f32]) {}

    fn param(&self, _index: usize) -> f32 {
        0.0
    }

    fn set_param(&mut self, _index: usize, _value: f32) {}

    fn reset(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(text: &str) -> Vec<u8> {
        let mut bytes = (text.len() as u32).to_le_bytes().to_vec();
        bytes.extend(text.as_bytes());
        bytes
    }

    const LAYERS: &str = "\u{feff}[General]\r\nLayerCount=2\r\n\r\n[AppSet0]\r\n\
        App=Peak Effects\\Linear\r\nEnabled=1\r\n\r\n[AppSet1]\r\nApp=(none)\r\n\r\n\
        [UserContent]\r\nText=\"x\"\r\nImages=[plugpath]Content\\Bitmaps\\a.png\r\n\
        VideoCues=\r\nVideoUseSync=0\r\n";

    #[test]
    fn layers_that_make_no_sound_pass_the_input() {
        let mut zge = ZGameEditor;
        assert_eq!(zge.set_state(&state(LAYERS)), Ok(()));
        let (mut l, mut r) = ([0.5, -0.25], [1.0, 0.0]);
        zge.process(&mut l, &mut r);
        assert_eq!((l, r), ([0.5, -0.25], [1.0, 0.0]));
    }

    #[test]
    fn a_sounding_effect_or_a_video_is_not_hosted() {
        let mut zge = ZGameEditor;
        for (from, to) in [
            ("Peak Effects\\Linear", "Misc\\AudioGenerator"),
            ("Peak Effects\\Linear", "My effects\\Unknown"),
            ("a.png", "clip.mp4"),
            ("VideoCues=", "VideoCues=1,2"),
            ("VideoUseSync=0", "Videos=clip.avi"),
        ] {
            let text = LAYERS.replace(from, to);
            assert_eq!(
                zge.set_state(&state(&text)),
                Err(StateError::Unhosted),
                "{to}"
            );
        }
        assert!(matches!(
            zge.set_state(&state(LAYERS)[..20]),
            Err(StateError::Length { .. })
        ));
    }

    #[test]
    fn every_state_in_fl_s_projects_is_hosted() {
        let Some(root) = crate::testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, the states in FL's projects skipped");
            return;
        };
        let mut hosted = 0;
        for saved in crate::testing::projects(&root) {
            if saved.plugin.eq_ignore_ascii_case(NAME) {
                ZGameEditor.set_state(&saved.state).unwrap();
                hosted += 1;
            }
        }
        assert_eq!(hosted, 9);
    }
}
