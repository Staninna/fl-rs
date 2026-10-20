//! What FL's host makes of a note before a voice or a plugin plays it, from
//! `docs/plugins/host-voice.md`.
//!
//! FL keeps five levels for a voice: pan, volume, pitch, and the two mods,
//! which it calls cutoff and resonance. `InitLevels` are the note's own,
//! filled from its record by `FUN_00d29aa0`. `FinalLevels` are those with
//! the channel's on them, which `FUN_00d265e0` works out and a plugin plays
//! by. [`trigger`] does both for a note that starts.
//!
//! It leaves out what the engine has no use for yet, or cannot read:
//!
//! - The channel's own cutoff and resonance. How 219 `Levels` stores them
//!   is not read, so a note's mods are its own plus tracking.
//! - A layer's levels on its children's notes. Layer channels do not play.
//! - Slides, which move a sounding voice's levels tick by tick. The
//!   pitch is [`crate::slide`]'s, and the pan, volume and mods a slide
//!   moves as well are left out.
//! - A voice that follows its channel's automation keeps the pan that
//!   tracking gave it when it started. FL works the levels out again on
//!   every tick.

use crate::{Curves, voice};
use model::{Channel, Note};

/// A note as it starts on its channel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Triggered {
    /// `InitLevels.Vol`: the gain of the note's velocity. [`voice::gains`]
    /// puts the channel's volume on it, which makes `FinalLevels.Vol`.
    pub vol: f32,
    /// The note's side of `FinalLevels.Pan`: its own pan, from -1 (left) to
    /// 1 (right), and what tracking adds. [`voice::gains`] adds the
    /// channel's pan.
    pub pan: f32,
    /// `FinalLevels.Pitch` in cents: 0 is key 60 on a channel whose pitch
    /// knob is at 0.
    pub pitch: f32,
    /// The note's side of `FinalLevels.FCut` and `FRes`: its mod X and mod
    /// Y, from -1 to 1, and what tracking adds.
    pub mod_x: f32,
    pub mod_y: f32,
    /// The note's release velocity, from 0 to 1. FL's `Voice_ProcessEvent`
    /// makes a release time of `2 * (1 - release)` of it for a plugin that
    /// asks.
    pub release: f32,
}

/// A level a key tracking knob at its end moves by 1 this many cents from
/// the knob's middle key: half the keyboard. `FUN_00d265e0` multiplies the
/// cents by a float that is not read, so this is a guess.
const KEY_RANGE: f32 = 6000.0;

/// `InitLevels.Pan` for the pan of a note record, where 64 is the centre.
fn pan(stored: u8) -> f32 {
    (f32::from(stored) - 64.0) / 64.0
}

/// `InitLevels.FCut` or `FRes` for a mod of a note record, where 128 moves
/// nothing.
fn modulation(stored: u8) -> f32 {
    (f32::from(stored) - 128.0) / 128.0
}

/// The release velocity for the release of a note record.
fn release(stored: u8) -> f32 {
    f32::from(stored) / 128.0
}

/// The channel's pitch knob in cents: the i32 at @8 of 219 `Levels`. PyFLP
/// calls it pitch shift in cents. The dictionary does not name it, so the
/// model has it as `unknown_8`.
pub(crate) fn knob(channel: &Channel) -> i32 {
    channel
        .levels
        .map_or(0, |l| i32::from_le_bytes(l.unknown_8))
}

/// The channel's pitch level, the float FL keeps at `chan+0x1a0`: the knob
/// less 6000. `InitLevels.Pitch` is 6000 at key 60, and this is what makes
/// `FinalLevels.Pitch` 0 there.
fn channel_pitch(channel: &Channel) -> f32 {
    knob(channel) as f32 - 6000.0
}

/// The `FinalLevels.Pitch` at which the channel's sample plays at its own
/// speed, in cents: the root note (135, 60 when the channel has none) less
/// 142 `FineTune`. The file's own root note is not decoded.
///
/// Where FL puts the two is not read. They are taken off the pitch of a
/// plugin channel's notes as well.
pub(crate) fn root(channel: &Channel) -> f32 {
    let root = channel.root_note.map_or(60.0, |root| root as f32);
    (root - 60.0) * 100.0 - channel.fine_tune.unwrap_or(0) as f32
}

/// How far the channel moves a note from its key, in cents: its pitch knob,
/// less its [`root`].
pub(crate) fn shift(channel: &Channel) -> f32 {
    knob(channel) as f32 - root(channel)
}

/// What the channel's tracking adds to the pan, the cutoff and the
/// resonance of a voice with this `FinalLevels.Vol` and `FinalLevels.Pitch`,
/// as `FUN_00d265e0` does:
///
/// ```text
/// dv = Final.Vol + volume offset      dp = Final.Pitch + key offset
/// Final.Pan += volume pan * dv + key pan * dp
/// ```
///
/// and the same for the cutoff and the resonance.
///
/// The model keeps a channel's two 228 records as bytes. Each is read here
/// as four i32: the middle, and the knobs for pan, mod X and mod Y. The
/// first record tracks the velocity and the second the key.
///
/// FL plays by floats it makes of those, and how it makes them is not read.
/// The offsets are taken to cancel the middle: a note at the middle
/// velocity on a channel at the 10000 of a new one moves nothing, and nor
/// does a note on the middle key. A knob is [`Curves::tracking_amount`] for a
/// volume of 1, or for [`KEY_RANGE`] cents. Tracking is on when a knob is
/// off 0, where FL has a flag at `chan+0x6ed`. The volume that is tracked
/// is without the adjustment of the channel's 229.
fn tracking(channel: &Channel, vol: f32, pitch: f32, curves: &dyn Curves) -> [f32; 3] {
    let record = |index: usize| {
        let bytes = channel.tracking.get(index).map_or([0; 16], |t| t.unknown_0);
        let field = |at: usize| {
            i32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };
        let knobs = [4, 8, 12].map(|at| curves.tracking_amount(field(at)));
        (field(0), knobs)
    };
    let (velocity, by_vol) = record(0);
    let (key, by_key) = record(1);
    if by_vol == [0.0; 3] && by_key == [0.0; 3] {
        return [0.0; 3];
    }
    let middle = curves.velocity_gain(velocity.clamp(0, 255) as u8);
    let dv = vol - middle * curves.volume_gain(10_000);
    let dp = (pitch - (key as f32 - 60.0) * 100.0) / KEY_RANGE;
    [0, 1, 2].map(|level| by_vol[level] * dv + by_key[level] * dp)
}

/// The levels of a note that starts on its channel.
pub(crate) fn trigger(note: &Note, channel: &Channel, curves: &dyn Curves) -> Triggered {
    let vol = curves.velocity_gain(note.velocity);
    // InitLevels.Pitch, the channel's pitch and the note's fine pitch.
    let pitch =
        f32::from(note.key) * 100.0 + channel_pitch(channel) + note.fine_pitch_cents() as f32;
    let (volume, _) = voice::levels(channel);
    let tracked = tracking(channel, vol * curves.volume_gain(volume), pitch, curves);
    Triggered {
        vol,
        pan: pan(note.pan) + tracked[0],
        pitch,
        mod_x: modulation(note.mod_x) + tracked[1],
        mod_y: modulation(note.mod_y) + tracked[2],
        release: release(note.release),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Approximate, FlExact};
    use flp::event::layout::{Levels, Tracking};

    fn note(key: i16) -> Note {
        Note {
            key,
            ..Note::default()
        }
    }

    /// A channel with its pitch knob at `knob` cents.
    fn channel(knob: i32) -> Channel {
        Channel {
            levels: Some(Levels {
                pan: 6400,
                volume: 10_000,
                unknown_8: knob.to_le_bytes(),
                mod_x: 0,
                mod_y: 0,
                unknown_20: [0; 4],
                len: 24,
            }),
            ..Channel::default()
        }
    }

    /// A 228 record: the middle, then the knobs for pan, mod X and mod Y.
    fn record(fields: [i32; 4]) -> Tracking {
        let mut bytes = [0; 16];
        for (at, field) in fields.into_iter().enumerate() {
            bytes[at * 4..at * 4 + 4].copy_from_slice(&field.to_le_bytes());
        }
        Tracking { unknown_0: bytes }
    }

    #[test]
    fn pitch_is_0_at_key_60() {
        let curves: [&dyn Curves; 2] = [&Approximate, &FlExact];
        for curves in curves {
            let pitch = |key, knob| trigger(&note(key), &channel(knob), curves).pitch;
            assert_eq!(pitch(60, 0), 0.0);
            assert_eq!(pitch(0, 0), -6000.0);
            assert_eq!(pitch(72, 0), 1200.0);
            // The knob is cents on top, and its level has the 6000 in it.
            assert_eq!(pitch(60, 700), 700.0);
            assert_eq!(pitch(48, -4800), -6000.0);
            // A channel without a 219 has its knob at 0.
            assert_eq!(trigger(&note(61), &Channel::default(), curves).pitch, 100.0);
        }
        assert_eq!(channel_pitch(&channel(0)), -6000.0);
        assert_eq!(channel_pitch(&channel(1200)), -4800.0);
    }

    #[test]
    fn a_fine_pitch_of_120_is_0_cents() {
        let pitch = |fine_pitch| {
            let note = Note {
                fine_pitch,
                ..note(60)
            };
            trigger(&note, &channel(0), &FlExact).pitch
        };
        assert_eq!(pitch(120), 0.0);
        // Ten cents a step, down to -1200 at 0.
        assert_eq!(pitch(121), 10.0);
        assert_eq!(pitch(0), -1200.0);
        assert_eq!(pitch(240), 1200.0);
    }

    #[test]
    fn a_new_note_is_centred_at_unity_with_no_mods() {
        let curves: [&dyn Curves; 2] = [&Approximate, &FlExact];
        for curves in curves {
            let levels = trigger(&note(60), &channel(0), curves);
            let want = Triggered {
                vol: 1.0,
                pan: 0.0,
                pitch: 0.0,
                mod_x: 0.0,
                mod_y: 0.0,
                release: 0.5,
            };
            assert_eq!(levels, want);
        }
    }

    #[test]
    fn a_note_s_bytes_scale_as_fl_scales_them() {
        // (p - 64) / 64.
        assert_eq!([0, 32, 64, 128].map(pan), [-1.0, -0.5, 0.0, 1.0]);
        // (m - 128) / 128.
        assert_eq!([0, 64, 128, 192].map(modulation), [-1.0, -0.5, 0.0, 0.5]);
        assert_eq!(modulation(255), 127.0 / 128.0);
        // r / 128.
        assert_eq!([0, 64, 128].map(release), [0.0, 0.5, 1.0]);
        let note = Note {
            pan: 96,
            velocity: 64,
            mod_x: 0,
            mod_y: 160,
            release: 32,
            ..note(60)
        };
        let levels = trigger(&note, &channel(0), &FlExact);
        assert_eq!((levels.pan, levels.release), (0.5, 0.25));
        assert_eq!((levels.mod_x, levels.mod_y), (-1.0, 0.25));
        // (11^0.64 - 1) / 10.
        assert!((levels.vol - 0.364).abs() < 1e-3);
        assert_eq!(trigger(&note, &channel(0), &Approximate).vol, 0.64);
    }

    #[test]
    fn the_root_note_and_the_fine_tune_say_where_the_sample_is_itself() {
        let mut high = channel(0);
        assert_eq!((root(&high), shift(&high)), (0.0, 0.0));
        high.root_note = Some(72);
        assert_eq!((root(&high), shift(&high)), (1200.0, -1200.0));
        // A note on the root key is the sample at its own speed.
        assert_eq!(trigger(&note(72), &high, &FlExact).pitch - root(&high), 0.0);
        high.fine_tune = Some(25);
        assert_eq!(root(&high), 1175.0);
        // The knob moves the notes and not the root.
        let mut knob = channel(300);
        knob.root_note = Some(57);
        assert_eq!((root(&knob), shift(&knob)), (-300.0, 600.0));
    }

    #[test]
    fn tracking_moves_the_pan_and_the_mods_by_velocity_and_by_key() {
        let levels = |channel: &Channel, key, velocity, curves: &dyn Curves| {
            let note = Note {
                velocity,
                ..note(key)
            };
            trigger(&note, channel, curves)
        };
        let mut tracked = channel(0);
        // Velocity around 100: pan all the way, mod Y a quarter back. Key
        // around 60: mod X half way.
        tracked.tracking = vec![record([100, 256, 0, -64]), record([60, 0, 128, 0])];
        // The middle velocity on the middle key moves nothing.
        let middle = levels(&tracked, 60, 100, &FlExact);
        assert_eq!((middle.pan, middle.mod_x, middle.mod_y), (0.0, 0.0, 0.0));
        // dv is the volume past the middle's on a new channel, and dp the
        // cents past the middle key's over 6000.
        let new = FlExact.volume_gain(10_000);
        let dv = FlExact.velocity_gain(64) * new - new;
        let loud = levels(&tracked, 72, 64, &FlExact);
        assert_eq!(loud.pan, dv);
        assert_eq!(loud.mod_x, 0.5 * 1200.0 / 6000.0);
        assert_eq!(loud.mod_y, -0.25 * dv);
        assert_eq!(loud.pitch, 1200.0);
        // The other curves have no tracking.
        let plain = levels(&tracked, 72, 64, &Approximate);
        assert_eq!((plain.pan, plain.mod_x, plain.mod_y), (0.0, 0.0, 0.0));

        // The channel's volume is part of the volume that is tracked.
        tracked.levels.as_mut().unwrap().volume = 5000;
        let quiet = FlExact.volume_gain(5000) - new;
        assert_eq!(levels(&tracked, 60, 100, &FlExact).pan, quiet);
        // A channel with its knobs at 0, or without the records, has none.
        tracked.tracking = vec![record([100, 0, 0, 0]), record([60, 0, 0, 0])];
        assert_eq!(levels(&tracked, 72, 64, &FlExact).pan, 0.0);
        tracked.tracking.clear();
        assert_eq!(levels(&tracked, 72, 64, &FlExact).pan, 0.0);
    }
}
