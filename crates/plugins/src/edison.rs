//! Edison, FL's audio editor and recorder, in the one case its sound is
//! its input: its state does not mute the input and holds no audio of its
//! own to play, as `docs/plugins/edison.md` reads `Edison_x64.dll`.
//!
//! `Eff_Render` copies the input to the output with `ippsCopy_64s`, or
//! zeroes the output when the state's mute-input flag is set. It adds a
//! file preview the editor starts (never there when a project loads) and
//! its own playback of the sample it holds, which can follow the song, a
//! MIDI note or its play button. Playback renders nothing from a sample
//! with no audio, and recording only appends the input to the sample. So a
//! state that does not mute the input and holds an empty sample leaves the
//! sound as it came, and is hosted. Any other is turned down as
//! [`StateError::Unhosted`], and a host passes it through.

use crate::{Effect, StateError, state_version};

/// The plugin's event 201 name.
pub const NAME: &str = "Edison";

/// The newest version FL reads: the state starts with `0x000d0003`, of
/// which FL takes the high word when there is one.
const NEWEST: i32 = 0x000d_0003;

/// What a state of Edison's says about the sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Saved {
    /// The mute-input flag, `this+0x41a`.
    muted: bool,
    /// The play-with-the-song flag, `this+0x424`, read from version 6 on.
    follows_song: bool,
    /// Whether the sample's WAV holds a frame of audio.
    audio: bool,
}

fn word(bytes: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_le_bytes(*bytes.get(at..)?.first_chunk::<4>()?))
}

fn too_short(bytes: &[u8], expected: usize) -> StateError {
    StateError::Length {
        expected,
        found: bytes.len(),
    }
}

/// Whether a WAV file holds audio: a `data` chunk that is not empty. A
/// file that is not a RIFF WAVE reads as holding some, which keeps the
/// state from being hosted.
fn has_audio(wav: &[u8]) -> bool {
    if wav.is_empty() {
        return false;
    }
    if wav.get(..4) != Some(b"RIFF") || wav.get(8..12) != Some(b"WAVE") {
        return true;
    }
    let mut at = 12;
    while let (Some(id), Some(len)) = (wav.get(at..at + 4), word(wav, at + 4)) {
        let len = len as u32 as usize;
        if id == b"data" {
            return len > 0;
        }
        at += 8 + len + (len & 1);
    }
    true
}

/// Read a state as `SaveRestoreState` (`0x87f1f0`) does.
fn read(bytes: &[u8]) -> Result<Saved, StateError> {
    let raw = state_version(bytes, 4)?;
    if raw > NEWEST {
        return Err(StateError::Version { found: raw });
    }
    let version = if raw > 0xffff { raw >> 16 } else { raw };
    let head = if version < 5 { 8 } else { 12 };
    let mut at = 4 + head;
    let len = if version < 13 {
        let len = word(bytes, at).ok_or(too_short(bytes, at + 4))?;
        at += 4;
        i64::from(len)
    } else {
        let low = word(bytes, at).ok_or(too_short(bytes, at + 8))?;
        let high = word(bytes, at + 4).ok_or(too_short(bytes, at + 8))?;
        at += 8;
        i64::from(high) << 32 | i64::from(low as u32)
    };
    let len = usize::try_from(len.max(0)).unwrap_or(usize::MAX);
    let wav = at
        .checked_add(len)
        .and_then(|end| bytes.get(at..end))
        .ok_or(too_short(bytes, at.saturating_add(len)))?;
    at += len;
    // The flags are the last bytes FL reads and writes, after a name and
    // the editor's view, whose lengths do not matter here: 12 bytes before
    // version 6 and 20 from it, from `this+0x414`.
    let tail = if version < 6 { 12 } else { 20 };
    if bytes.len() < at + tail {
        return Err(too_short(bytes, at + tail));
    }
    let flags = &bytes[bytes.len() - tail..];
    Ok(Saved {
        muted: flags[6] != 0,
        follows_song: tail == 20 && flags[16] != 0,
        audio: has_audio(wav),
    })
}

/// Edison with a state that leaves the sound as it came.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Edison;

impl Effect for Edison {
    /// Takes a state that does not mute the input and holds no audio.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let saved = read(bytes)?;
        if saved.muted || saved.audio {
            return Err(StateError::Unhosted);
        }
        Ok(())
    }

    fn set_sample_rate(&mut self, _hz: f32) {}

    /// The block stays as it came: `ippsCopy_64s` from the input.
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

    /// A state of version 13 with this WAV and these flags from
    /// `this+0x414`.
    fn state(wav: &[u8], flags: [u8; 20]) -> Vec<u8> {
        let mut bytes = NEWEST.to_le_bytes().to_vec();
        bytes.extend([0; 12]);
        bytes.extend((wav.len() as i64).to_le_bytes());
        bytes.extend(wav);
        // A name and the editor's view.
        bytes.extend([0; 4 + 36]);
        bytes.extend(flags);
        bytes
    }

    fn wav(frames: u32) -> Vec<u8> {
        let mut data = b"fmt ".to_vec();
        data.extend(16u32.to_le_bytes());
        data.extend([3, 0, 2, 0, 0x44, 0xac, 0, 0, 0x20, 0x62, 5, 0, 8, 0, 32, 0]);
        data.extend(b"data");
        data.extend((frames * 8).to_le_bytes());
        data.extend(vec![0; frames as usize * 8]);
        let mut riff = b"RIFF".to_vec();
        riff.extend((data.len() as u32 + 4).to_le_bytes());
        riff.extend(b"WAVE");
        riff.extend(data);
        riff
    }

    #[test]
    fn an_empty_sample_that_does_not_mute_is_a_pass_through() {
        let mut edison = Edison;
        let mut flags = [0; 20];
        flags[0] = 3;
        flags[16] = 1;
        assert_eq!(edison.set_state(&state(&wav(0), flags)), Ok(()));
        let (mut l, mut r) = ([0.5, -0.25], [1.0, 0.0]);
        edison.process(&mut l, &mut r);
        assert_eq!((l, r), ([0.5, -0.25], [1.0, 0.0]));
        assert_eq!(edison.set_state(&state(&[], flags)), Ok(()));
    }

    #[test]
    fn a_muted_input_or_a_sample_with_audio_is_not_hosted() {
        let mut edison = Edison;
        let mut flags = [0; 20];
        flags[6] = 1;
        let muted = state(&wav(0), flags);
        assert_eq!(edison.set_state(&muted), Err(StateError::Unhosted));
        let sounding = state(&wav(10), [0; 20]);
        assert_eq!(edison.set_state(&sounding), Err(StateError::Unhosted));
        assert_eq!(read(&sounding).map(|s| s.audio), Ok(true));
    }

    #[test]
    fn old_and_short_states_read_as_fl_reads_them() {
        // Version 12 counts the sample's length in 32 bits.
        let mut bytes = 0x000c_0003i32.to_le_bytes().to_vec();
        bytes.extend([0; 12]);
        let wav = wav(0);
        bytes.extend((wav.len() as i32).to_le_bytes());
        bytes.extend(&wav);
        bytes.extend([0; 40]);
        bytes.extend([3, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 3, 0, 0, 0, 1, 0, 0, 0]);
        let saved = read(&bytes).unwrap();
        assert!(saved.muted && saved.follows_song && !saved.audio);
        assert!(matches!(read(&bytes[..30]), Err(StateError::Length { .. })));
        assert_eq!(
            read(&0x000d_0004i32.to_le_bytes()),
            Err(StateError::Version { found: 0x000d_0004 })
        );
    }

    #[test]
    fn five_of_the_six_states_in_fl_s_projects_are_hosted() {
        let Some(root) = crate::testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, the states in FL's projects skipped");
            return;
        };
        let mut hosted = Vec::new();
        let mut held = Vec::new();
        for saved in crate::testing::projects(&root) {
            if !saved.plugin.eq_ignore_ascii_case(NAME) {
                continue;
            }
            let read = read(&saved.state).unwrap();
            assert!(!read.muted, "{}", saved.file);
            match Edison.set_state(&saved.state) {
                Ok(()) => hosted.push(saved.file),
                Err(StateError::Unhosted) => held.push(saved.file),
                Err(error) => panic!("{}: {error}", saved.file),
            }
        }
        hosted.sort();
        assert_eq!(hosted.len(), 5, "{hosted:?}");
        // Asher Postman - Future House keeps an 11758-frame sample, which
        // a MIDI note can start.
        assert_eq!(held, ["Asher Postman - Future House"]);
    }
}
