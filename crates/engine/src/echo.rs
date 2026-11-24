//! A channel's echo delay: FL's host plays a note again a number of times
//! after it starts, each echo quieter, panned and pitched further. Read
//! from `FLEngine_x64.dll` (`docs/rec-channel.md`, "Echo delay"):
//!
//! - `FUN_00d28990`, as a note starts (`FUN_00d293d0`): a channel whose
//!   feedback, `chan+0x200`, is above 0 echoes the note, unless the note is
//!   an echo itself. The feedback is `feedback / 12800`.
//! - `FUN_00d28c90` makes echo `k`, 1 to `chan+0x20c`, at the note's tick
//!   plus `k * time * (PPQ / 4) / 48` ticks (`FUN_01312200`, in whole
//!   ticks), as long as the note: its `InitLevels.Vol` is the one before's
//!   times the feedback; its pan the one before's plus `(pan - 6400) /
//!   6400` on top of the channel's pan, kept to -1..1 and turned back at
//!   either end when ping-pong (215 @10 bit 1) is on; its pitch the one
//!   before's plus the pitch in cents, unless the channel time stretches;
//!   and its mod X and mod Y moved by 138's two words, of which 128
//!   moves nothing.
//! - In fat mode (215 @10 bit 2) every echo has the feedback over
//!   `(1.5 echoes + 1) 2`, the note itself is turned down to `1.5 - 2
//!   echoes feedback`, and the echoes come twice, once panned and pitched
//!   one way and once the other.
//!
//! The echoes go into the song as notes of their own, after its notes, and
//! play as any note of their channel does, a plugin's included.

use crate::levels::Triggered;
use crate::sequence::{Played, Song};
use model::{Channel, Item, Project};

/// A channel's echo delay, as FL's host keeps it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Echo {
    /// What each echo's volume is multiplied by.
    feedback: f32,
    /// What each echo adds to the pan, and the channel's pan.
    pan: f32,
    channel_pan: f32,
    /// What each echo adds to the pitch, in cents, and whether it does.
    cents: i32,
    pitches: bool,
    echoes: u32,
    /// The time between echoes, in 192ths of a beat.
    time: i32,
    /// 138's two words: mod X's and mod Y's, 128 for none.
    mods: (i32, i32),
    ping_pong: bool,
    fat: bool,
}

/// Which echo a note of the song is: of the note at `source`, echo `k`
/// from 1, and in fat mode the second pass when `twin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Echoed {
    pub source: usize,
    pub k: u32,
    pub twin: bool,
}

impl Echo {
    /// The channel's echo delay, when it makes echoes.
    pub fn of(project: &Project, channel: &Channel) -> Option<Self> {
        let delay = channel.delay?;
        let version = project.writer_version.as_deref();
        let version = version.and_then(flp::event::version_number);
        // The loader's rewrites of old files (`FUN_00f46490`, 209).
        let scale = if version.is_some_and(|v| v < 5_003_007) {
            100
        } else {
            1
        };
        let feedback = delay.feedback.saturating_mul(scale);
        let pan = delay.pan.saturating_mul(scale).saturating_add(6400);
        let cents = match version {
            Some(v) if v < 3_002_985 => delay.pitch.saturating_sub(120).saturating_mul(10),
            _ => delay.pitch,
        };
        if feedback <= 0 || delay.echoes <= 0 {
            return None;
        }
        let flags = channel.parameters.map_or(0, |p| p.unknown_8[2]);
        let (_, channel_pan) = crate::voice::levels(channel);
        let channel_pan = channel_pan as i32 + crate::voice::pan_offset(project, channel);
        let stretched = channel.parameters.is_some_and(|p| {
            u32::from_le_bytes(p.stretch_time) & 0x7fff_ffff != 0 && p.stretch_mode >= 0
        });
        Some(Echo {
            feedback: feedback as f32 * (1.0 / 12_800.0),
            pan: (pan - 12_800) as f32 * (1.0 / 6400.0),
            channel_pan: (channel_pan - 6400) as f32 * (1.0 / 6400.0),
            cents,
            pitches: !stretched,
            echoes: delay.echoes as u32,
            time: delay.time,
            mods: mods(channel),
            ping_pong: flags & 2 != 0,
            fat: flags & 4 != 0,
        })
    }

    /// The ticks echo `k` comes after its note at `ppq`.
    fn ticks(&self, k: u32, ppq: u16) -> i64 {
        let units = i64::from(k) * i64::from(self.time) * i64::from(ppq / 4);
        units / 48
    }

    /// The feedback of an echo in fat mode.
    fn fat_feedback(&self) -> f32 {
        let spread = (f64::from(self.echoes) * 1.5 + 1.0) * 2.0;
        (f64::from(self.feedback) / spread) as f32
    }

    /// What a note of a channel in fat mode is turned down to.
    fn own_volume(&self, vol: f32) -> f32 {
        if !self.fat {
            return vol;
        }
        let spread = self.fat_feedback() * self.echoes as f32 * 2.0;
        (f64::from(vol) * (1.5 - f64::from(spread))) as f32
    }

    /// The levels of `echo`, from those of its note as it starts, and the
    /// cents it adds to the note's pitch.
    pub fn levels(&self, note: &Triggered, echo: Echoed) -> (Triggered, f32) {
        let mut levels = *note;
        let (mut pan, mut cents) = (self.pan, self.cents);
        if echo.twin {
            // The second pass starts from the note as it was turned down,
            // the other way.
            levels.vol = self.own_volume(note.vol);
            pan = -pan;
            cents = -cents;
        }
        let feedback = if self.fat {
            self.fat_feedback()
        } else {
            self.feedback
        };
        if self.fat {
            levels.vol *= feedback;
        }
        let mut added = 0.0;
        for _ in 0..echo.k {
            let at = (levels.pan + pan + self.channel_pan).clamp(-1.0, 1.0);
            if self.ping_pong && f64::from(at).abs() == 1.0 {
                pan = -pan;
            }
            levels.pan = at - self.channel_pan;
            if !self.fat {
                levels.vol *= feedback;
            }
            if self.pitches {
                added += cents as f32;
            }
            levels.mod_x = moved(levels.mod_x, self.mods.0);
            levels.mod_y = moved(levels.mod_y, self.mods.1);
        }
        levels.pitch += added;
        (levels, added)
    }

    /// The note's own levels: turned down in fat mode.
    pub fn own(&self, note: &Triggered) -> Triggered {
        Triggered {
            vol: self.own_volume(note.vol),
            ..*note
        }
    }
}

/// The levels a note of `channel` starts with, from `note` as the note's
/// record gives them, when it is `echo` or one of the song's own: an echo
/// gets its echo's levels, and a note of a channel in fat mode is turned
/// down. The cents an echo adds to the pitch come back with them.
pub(crate) fn levels(
    project: &Project,
    channel: &Channel,
    note: &Triggered,
    echo: Option<Echoed>,
) -> (Triggered, f32) {
    let Some(delay) = Echo::of(project, channel) else {
        return (*note, 0.0);
    };
    match echo {
        Some(echo) => delay.levels(note, echo),
        None => (delay.own(note), 0.0),
    }
}

/// A mod moved by an echo's word `by`, which leaves it at 128: toward -1
/// below, toward 1 above (`0xd28fb1`).
fn moved(level: f32, by: i32) -> f32 {
    let of_128 = 1.0f32 / 128.0;
    match by {
        128 => level,
        ..128 => (level + 1.0) * by as f32 * of_128 - 1.0,
        _ => 1.0 - (1.0 - level) * (256 - by) as f32 * of_128,
    }
}

/// 138 `DelayModXY`: mod Y in the low word, mod X in the high one, 128
/// each when the channel has none (`FUN_00fe90d0`).
fn mods(channel: &Channel) -> (i32, i32) {
    let raw = channel
        .order
        .iter()
        .rev()
        .find_map(|placed| match &placed.item {
            Item::Raw(event) if event.id == 138 && event.data.len() >= 4 => {
                Some(event.data.clone())
            }
            _ => None,
        });
    raw.map_or((128, 128), |data| {
        let word = |at: usize| i32::from(u16::from_le_bytes([data[at], data[at + 1]]));
        (word(2), word(0))
    })
}

/// The song with every note's echoes after its notes, and which echo each
/// of them is, by its place among the echoes. `None` when no channel
/// echoes.
pub(crate) fn expand(project: &Project, song: &Song) -> Option<(Song, Vec<Echoed>)> {
    let echoes: Vec<Option<Echo>> = project
        .channels
        .iter()
        .map(|channel| Echo::of(project, channel))
        .collect();
    let echo_of = |channel: u16| {
        let at = project.channels.iter().position(|c| c.index == channel)?;
        echoes[at]
    };
    let mut notes = Vec::new();
    let mut which = Vec::new();
    for (source, played) in song.notes.iter().enumerate() {
        if played.note.slide() {
            continue;
        }
        let Some(echo) = echo_of(played.note.channel) else {
            continue;
        };
        let passes = if echo.fat { 2 } else { 1 };
        for pass in 0..passes {
            for k in 1..=echo.echoes {
                let late = echo.ticks(k, project.header.ppq);
                let Some(tick) = played.tick.checked_add_signed(late) else {
                    continue;
                };
                notes.push(Played { tick, ..*played });
                which.push(Echoed {
                    source,
                    k,
                    twin: pass == 1,
                });
            }
        }
    }
    if notes.is_empty() {
        return None;
    }
    let mut echoed = Song {
        notes: song.notes.clone(),
        clips: song.clips.clone(),
        automation: song.automation.clone(),
        end: song.end,
        glides: song.glides.clone(),
    };
    for note in &notes {
        let length = note.length.unwrap_or(0);
        echoed.end = echoed.end.max(note.tick.saturating_add(length));
    }
    echoed.notes.extend(notes);
    Some((echoed, which))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn echo() -> Echo {
        Echo {
            feedback: 0.5,
            pan: 0.25,
            channel_pan: 0.0,
            cents: 100,
            pitches: true,
            echoes: 3,
            time: 144,
            mods: (128, 128),
            ping_pong: false,
            fat: false,
        }
    }

    fn note() -> Triggered {
        Triggered {
            vol: 0.8,
            pan: 0.0,
            pitch: 0.0,
            mod_x: 0.0,
            mod_y: 0.0,
            release: 0.5,
        }
    }

    #[test]
    fn echoes_fall_on_whole_ticks_of_the_time() {
        // 144 is three sixteenths: 72 ticks at PPQ 96.
        let e = echo();
        assert_eq!(e.ticks(1, 96), 72);
        assert_eq!(e.ticks(3, 96), 216);
        // FL keeps the PPQ over 4 whole, and the ticks too.
        assert_eq!(Echo { time: 5, ..e }.ticks(1, 96), 2);
    }

    #[test]
    fn each_echo_is_quieter_further_panned_and_higher() {
        let e = echo();
        let (one, cents) = e.levels(
            &note(),
            Echoed {
                source: 0,
                k: 1,
                twin: false,
            },
        );
        assert_eq!(
            (one.vol, one.pan, cents, one.pitch),
            (0.4, 0.25, 100.0, 100.0)
        );
        let (three, cents) = e.levels(
            &note(),
            Echoed {
                source: 0,
                k: 3,
                twin: false,
            },
        );
        assert_eq!((three.vol, three.pan, cents), (0.1, 0.75, 300.0));
        // The pan stops at the side, or turns back there with ping-pong.
        let far = Echo { pan: 0.75, ..e };
        let (two, _) = far.levels(
            &note(),
            Echoed {
                source: 0,
                k: 2,
                twin: false,
            },
        );
        assert_eq!(two.pan, 1.0);
        let pong = Echo {
            ping_pong: true,
            ..far
        };
        let (three, _) = pong.levels(
            &note(),
            Echoed {
                source: 0,
                k: 3,
                twin: false,
            },
        );
        assert_eq!(three.pan, 0.25);
    }

    #[test]
    fn fat_mode_turns_the_note_down_and_echoes_it_both_ways() {
        let e = Echo {
            fat: true,
            ..echo()
        };
        // 0.5 over (3 * 1.5 + 1) * 2 = 11.
        let feedback = e.fat_feedback();
        assert_eq!(feedback, (0.5f64 / 11.0) as f32);
        let own = e.own(&note());
        assert!((own.vol - 0.8 * (1.5 - 6.0 * feedback)).abs() < 1e-6);
        let (one, _) = e.levels(
            &note(),
            Echoed {
                source: 0,
                k: 2,
                twin: false,
            },
        );
        assert_eq!((one.vol, one.pan), (0.8 * feedback, 0.5));
        let (twin, cents) = e.levels(
            &note(),
            Echoed {
                source: 0,
                k: 2,
                twin: true,
            },
        );
        assert_eq!(
            (twin.vol, twin.pan, cents),
            (own.vol * feedback, -0.5, -200.0)
        );
    }

    #[test]
    fn the_mod_words_pull_toward_an_end() {
        assert_eq!(moved(0.5, 128), 0.5);
        assert_eq!(moved(0.0, 64), -0.5);
        assert_eq!(moved(0.0, 192), 0.5);
        assert_eq!(moved(-1.0, 0), -1.0);
    }
}
