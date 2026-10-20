//! What the generators that play sample files share: the files a state
//! names and what the host made of each, the chunks their states are
//! written in, FL's pan law and a reader of a decoded file at any place in
//! it.
//!
//! FPC, DirectWave and Slicex decode nothing themselves. Each says the
//! paths its state names in [`Generator::sample_paths`](crate::Generator::sample_paths)
//! and the host hands the decoded files back, so that a path with no file
//! is in the host's list of what it did not find.

use crate::{SampleFile, StateError};
use dsp::interpolator::Interpolator;
use std::f64::consts::FRAC_PI_4;

/// A decoded file, kept by the plugin that plays it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Wave {
    pub(crate) left: Vec<f32>,
    /// A mono file has none, and both sides play `left`.
    pub(crate) right: Option<Vec<f32>>,
    /// The file's own rate, in Hz.
    pub(crate) rate: u32,
}

impl Wave {
    pub(crate) fn of(file: SampleFile<'_>) -> Self {
        Wave {
            left: file.left.to_vec(),
            right: file.right.map(<[f32]>::to_vec),
            rate: file.rate,
        }
    }

    /// How many frames the file has.
    pub(crate) fn len(&self) -> usize {
        self.left.len()
    }

    /// Both sides at `pos` frames in. A whole position is the frame as the
    /// file has it, which is what FL's player does at a step of 1. Between
    /// frames this is a 4-point Hermite curve, in place of FL's kernels.
    pub(crate) fn frame(&self, pos: f64) -> (f32, f32) {
        let side = |side: &[f32]| {
            if pos.fract() == 0.0 && pos >= 0.0 {
                side.get(pos as usize).copied().unwrap_or(0.0)
            } else {
                dsp::resample::read_hermite(side, pos)
            }
        };
        let left = side(&self.left);
        (left, self.right.as_deref().map_or(left, side))
    }

    /// Both sides at `pos` frames in, read by `reader` as FL's player reads
    /// them: at a speed of 1 from a whole place the frames are copied, and
    /// otherwise they go through the interpolator, the file's end frames
    /// held past its ends.
    pub(crate) fn read(&self, pos: f64, reader: &Interpolator) -> (f32, f32) {
        if reader.speed() == 1.0 && pos.fract() == 0.0 && pos >= 0.0 {
            let side = |side: &[f32]| side.get(pos as usize).copied().unwrap_or(0.0);
            let left = side(&self.left);
            return (left, self.right.as_deref().map_or(left, side));
        }
        let index = pos.floor() as i64;
        let frac = pos - index as f64;
        match &self.right {
            Some(right) => {
                let [l, r] = reader.read([&self.left, right], index, frac);
                (l, r)
            }
            None => {
                let l = reader.frame(&self.left, index, frac);
                (l, l)
            }
        }
    }
}

/// What the host made of one path.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Slot {
    /// The host has not said yet.
    Waiting,
    /// The host found no file.
    Missing,
    Ready(Wave),
}

/// The files a state names, each once, in the order the state names them.
/// A plugin whose state carries samples adds them as places of their own
/// with an empty path (`Bank::hold` in DirectWave).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Bank {
    pub(crate) paths: Vec<String>,
    pub(crate) slots: Vec<Slot>,
}

impl Bank {
    /// The place of a path in the bank, which is made for a path that is
    /// new. `None` for an empty path: a pad with no sample has one.
    pub(crate) fn place(&mut self, path: &str) -> Option<usize> {
        if path.is_empty() {
            return None;
        }
        if let Some(found) = self.paths.iter().position(|known| known == path) {
            return Some(found);
        }
        self.paths.push(path.to_owned());
        self.slots.push(Slot::Waiting);
        Some(self.paths.len() - 1)
    }

    /// The place of a path the bank has.
    pub(crate) fn find(&self, path: &str) -> Option<usize> {
        let found = self.paths.iter().position(|known| known == path);
        found.filter(|_| !path.is_empty())
    }

    /// The paths a host is asked for.
    pub(crate) fn paths(&self) -> Vec<String> {
        let named = self.paths.iter().filter(|path| !path.is_empty());
        named.cloned().collect()
    }

    /// Take what the host made of `path`. Nothing happens for a path that
    /// is not in the bank.
    pub(crate) fn set(&mut self, path: &str, file: Option<SampleFile<'_>>) {
        if let Some(found) = self.find(path) {
            self.slots[found] = file.map_or(Slot::Missing, |file| Slot::Ready(Wave::of(file)));
        }
    }

    /// The file in a place, when the host handed one over.
    pub(crate) fn wave(&self, place: usize) -> Option<&Wave> {
        match self.slots.get(place)? {
            Slot::Ready(wave) => Some(wave),
            Slot::Waiting | Slot::Missing => None,
        }
    }

    /// Whether any path has no file behind it, or has not been looked up.
    pub(crate) fn lacking(&self) -> bool {
        let ready = |slot: &Slot| matches!(slot, Slot::Ready(_));
        !self.slots.iter().all(ready)
    }
}

/// The chunks FL's newer plugins write their states in: an `i32` id, an
/// `i64` size and that many bytes, one after the other to the end. A chunk
/// that runs past the end is an error, and so is a header cut short.
pub(crate) struct Chunks<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Chunks<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Chunks { bytes, at: 0 }
    }

    fn short(&self, expected: usize) -> StateError {
        StateError::Length {
            expected,
            found: self.bytes.len(),
        }
    }
}

impl<'a> Iterator for Chunks<'a> {
    type Item = Result<(i32, &'a [u8]), StateError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.at >= self.bytes.len() {
            return None;
        }
        let body = self.at.saturating_add(12);
        let Some(header) = self.bytes.get(self.at..body) else {
            self.at = self.bytes.len();
            return Some(Err(self.short(body)));
        };
        let id = i32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        let size = header[4..]
            .first_chunk::<8>()
            .map(|b| i64::from_le_bytes(*b));
        let size = size.and_then(|size| usize::try_from(size).ok());
        let end = size.map_or(usize::MAX, |size| body.saturating_add(size));
        let Some(bytes) = self.bytes.get(body..end) else {
            self.at = self.bytes.len();
            return Some(Err(self.short(end)));
        };
        self.at = end;
        Some(Ok((id, bytes)))
    }
}

/// The little-endian i32s of a chunk, as many as fit in it.
pub(crate) fn words(bytes: &[u8]) -> impl Iterator<Item = i32> + '_ {
    let words = bytes.as_chunks::<4>().0.iter();
    words.map(|word| i32::from_le_bytes(*word))
}

/// A string as FL's chunks hold one: UTF-16 when `wide`, and one byte a
/// letter otherwise. What follows a NUL is left out.
pub(crate) fn text(bytes: &[u8], wide: bool) -> String {
    let text = if wide {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|unit| u16::from_le_bytes(*unit))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        bytes.iter().map(|byte| char::from(*byte)).collect()
    };
    text.split('\0').next().unwrap_or_default().to_owned()
}

/// FL's `ComputeLRVol` with its default pan law, the circular one
/// (`docs/plugins/host-voice.md`): left and right gain for a pan from -1
/// to 1 and a volume. In the centre each side is 0.7071 of the volume.
pub(crate) fn circular(pan: f32, volume: f32) -> (f32, f32) {
    let angle = (1.0 + f64::from(pan.clamp(-1.0, 1.0))) * FRAC_PI_4;
    (
        (angle.cos() * f64::from(volume)) as f32,
        (angle.sin() * f64::from(volume)) as f32,
    )
}

/// How many source frames a voice moves on in one output sample: the
/// pitch in cents over the file's own, and the file's rate over the
/// output's.
pub(crate) fn step(cents: f64, file_rate: u32, rate: f32) -> f64 {
    2f64.powf(cents / 1200.0) * f64::from(file_rate) / f64::from(rate)
}

/// The taps a side FL's players read with when a song is rendered. A
/// player that is not told a quality takes its unit's, which the host sets
/// with `FPD_ProcessMode` (bits 8 to 15 are the points: FPC's dispatcher
/// `0x0086a970` -> `FUN_0062ede0`). FL's export settings keep the quality
/// as a mode of the points table, 1 to 9 for 1, 3, 8, 12, 16, 32, 64, 128
/// and 256, and a new install's is mode 5 (`FLEngine_x64.dll` 0xedff62),
/// the dialog's "32-point sinc". That the render hands plugins that
/// setting is inferred: the engine's sender was not found.
pub(crate) const RENDER_POINTS: u32 = 16;

/// FL's interpolator at the render's quality and `step`.
pub(crate) fn reader(step: f64) -> Interpolator {
    let mut reader = Interpolator::new(RENDER_POINTS);
    reader.set_speed(step);
    reader
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: i32, body: &[u8]) -> Vec<u8> {
        let mut bytes = id.to_le_bytes().to_vec();
        bytes.extend((body.len() as i64).to_le_bytes());
        bytes.extend(body);
        bytes
    }

    #[test]
    fn chunks_come_one_after_the_other_and_a_cut_one_is_an_error() {
        let mut bytes = chunk(7, &[1, 2, 3]);
        bytes.extend(chunk(300, &[]));
        let read: Vec<_> = Chunks::new(&bytes).map(Result::unwrap).collect();
        assert_eq!(read, [(7, &[1u8, 2, 3][..]), (300, &[][..])]);
        // A body that runs past the end, a header cut short and a size
        // below zero.
        let long = &bytes[..14];
        assert!(matches!(
            Chunks::new(long).next(),
            Some(Err(StateError::Length {
                expected: 15,
                found: 14
            }))
        ));
        assert!(Chunks::new(&bytes[..20]).nth(1).unwrap().is_err());
        let mut negative = 1i32.to_le_bytes().to_vec();
        negative.extend((-1i64).to_le_bytes());
        let mut chunks = Chunks::new(&negative);
        assert!(chunks.next().unwrap().is_err());
        assert!(chunks.next().is_none());
    }

    #[test]
    fn a_bank_keeps_each_path_once_and_knows_what_it_lacks() {
        let mut bank = Bank::default();
        assert_eq!(bank.place("a.wav"), Some(0));
        assert_eq!(bank.place("b.wav"), Some(1));
        assert_eq!(bank.place("a.wav"), Some(0));
        assert_eq!(bank.place(""), None);
        assert_eq!(bank.paths(), ["a.wav", "b.wav"]);
        assert!(bank.lacking());
        let file = SampleFile {
            left: &[0.5, -0.5],
            right: None,
            rate: 44_100,
        };
        bank.set("a.wav", Some(file));
        bank.set("b.wav", None);
        bank.set("c.wav", Some(file));
        assert_eq!(bank.wave(0).unwrap().len(), 2);
        assert!(bank.wave(1).is_none() && bank.wave(2).is_none());
        assert!(bank.lacking());
        bank.set("b.wav", Some(file));
        assert!(!bank.lacking());
        assert_eq!(
            (bank.find("b.wav"), bank.find("c.wav"), bank.find("")),
            (Some(1), None, None)
        );
    }

    #[test]
    fn a_whole_place_is_the_frame_and_a_mono_file_plays_both_sides() {
        let wave = Wave {
            left: vec![0.0, 1.0, 0.0, -1.0],
            right: None,
            rate: 44_100,
        };
        assert_eq!(wave.frame(1.0), (1.0, 1.0));
        assert_eq!(wave.frame(4.0), (0.0, 0.0));
        let between = wave.frame(1.5).0;
        assert!(between > 0.4 && between < 0.7, "{between}");
        let stereo = Wave {
            right: Some(vec![0.25; 4]),
            ..wave
        };
        assert_eq!(stereo.frame(3.0), (-1.0, 0.25));
    }

    #[test]
    fn the_pan_law_is_fl_s_circle_and_a_step_is_the_pitch_and_the_rates() {
        let (left, right) = circular(0.0, 2.0);
        assert!((left - 2f32.sqrt()).abs() < 1e-6 && (right - 2f32.sqrt()).abs() < 1e-6);
        let (left, right) = circular(-1.0, 1.0);
        assert!((left - 1.0).abs() < 1e-6 && right.abs() < 1e-6);
        assert_eq!(circular(5.0, 1.0), circular(1.0, 1.0));
        assert_eq!(step(0.0, 44_100, 44_100.0), 1.0);
        assert_eq!(step(1200.0, 22_050, 44_100.0), 1.0);
        assert_eq!(text(b"ab\0c", false), "ab");
        assert_eq!(text(&[b'h', 0, b'i', 0], true), "hi");
        assert_eq!(
            words(&[1, 0, 0, 0, 255, 255, 255, 255, 9]).collect::<Vec<_>>(),
            [1, -1]
        );
    }
}
