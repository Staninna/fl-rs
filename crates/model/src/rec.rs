//! REC control IDs: what a 225 record in
//! [`Project::initial_values`](crate::Project::initial_values) addresses,
//! and what a controller link drives.
//!
//! `docs/rec-225.md` is the source for everything here. A mixer ID is
//! `0x70000000 + (insert << 22) + (slot << 16) + control`. Files from before
//! FL 24.2.99, and files without a 103, use base 0x20000000 and a few other
//! numbers, and FL's loader rewrites those IDs first. [`remap_legacy`] is
//! that rewrite and [`RecTarget::decode`] applies it when asked.
//!
//! The record fixups for files older than 8.5.0 are not here. The corpus has
//! no file that old.

/// Control numbers of an insert, the low 16 bits of a mixer ID.
pub mod control {
    /// Slot enabled, 0 or 1. The slot is in the ID.
    pub const SLOT_ENABLED: u16 = 0x1F00;
    /// Slot mix level, 0..=12800. The slot is in the ID.
    pub const SLOT_MIX: u16 = 0x1F01;
    /// Insert volume, 0..=16000 with 12800 at 0 dB.
    pub const VOLUME: u16 = 0x1FC0;
    /// Insert pan, -6400..=6400.
    pub const PAN: u16 = 0x1FC1;
    /// Stereo separation, -64..=64.
    pub const SEPARATION: u16 = 0x1FC2;
    /// Gain of EQ band 0. Bands 1 and 2 follow.
    pub const EQ_GAIN: u16 = 0x1FD0;
    /// Frequency of EQ band 0. Bands 1 and 2 follow.
    pub const EQ_FREQUENCY: u16 = 0x1FD8;
    /// Bandwidth of EQ band 0. Bands 1 and 2 follow.
    pub const EQ_WIDTH: u16 = 0x1FE0;
    /// Send level to insert 0, 0..=16000. Insert `d` is at `SEND + d`.
    pub const SEND: u16 = 0x2000;
    /// The global main volume, 0x40000000.
    pub const MAIN_VOLUME: u16 = 0;
    /// The global tempo, 0x40000005, in thousandths of a BPM.
    pub const TEMPO: u16 = 5;
    /// Channel volume, 0..=12800. A channel's ID is `channel << 16` plus
    /// the control.
    pub const CHANNEL_VOLUME: u16 = 0;
    /// Channel pan, 0..=12800 with 6400 in the centre.
    pub const CHANNEL_PAN: u16 = 1;
}

/// The highest insert index. FL keeps the "current" track there.
pub const LAST_INSERT: u16 = 501;

const MIXER: u32 = 0x7000_0000;
/// The first ID past those that address a channel.
const CHANNELS_END: u32 = 0x1000_0000;
const LEGACY_MIXER: u32 = 0x2000_0000;
/// The first writer version with the 0x70000000 base, 24.2.99.
pub(crate) const NEW_MIXER_VERSION: u32 = 24_002_099;

/// What a REC control ID addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RecTarget {
    /// A control of mixer insert `index`, 0 for the master up to
    /// [`LAST_INSERT`].
    Insert {
        index: u16,
        /// The effect slot for a slot's own controls: enabled, mix and
        /// plugin parameters (0x8000 and up). `None` for the insert's
        /// controls, whose IDs have 0 in the slot bits.
        slot: Option<u8>,
        control: u16,
    },
    /// A project-wide control, 0x40000000 plus `control`.
    Global { control: u16 },
    /// A control of the channel whose 64 has `index`: an ID below
    /// 0x10000000. Controls from 0x8000 up are parameters of the channel's
    /// plugin. No 225 record has such an ID. Controller links do.
    Channel { index: u16, control: u16 },
    /// An ID outside those three ranges, as stored.
    Other(u32),
}

/// An output of an internal controller in an effect slot, which is what
/// the source of a controller link names when a plugin drives the link.
///
/// FL gives output `n` of a slot's plugin the ID of the slot's control
/// `0x8000 + n`, the number parameter `n` of the plugin has as a target
/// (`docs/plugins/fruity-peak-controller.md`, section 4). Fruity Peak
/// Controller has three: 0 Peak+LFO, 1 Peak and 2 LFO.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ControllerOutput {
    /// The loader's number of the insert the plugin is on.
    pub insert: u16,
    pub slot: u8,
    /// The output's place among the plugin's, from 0.
    pub output: u16,
}

/// The control number of a slot's first controller output, and of its
/// plugin's first parameter.
const PLUGIN: u16 = 0x8000;

impl RecTarget {
    /// The controller output this ID names when it is the source of a
    /// link: control `0x8000` or above of an effect slot. `None` for any
    /// other ID. Whether the slot holds a controller is for the caller to
    /// find out.
    pub fn controller_output(&self) -> Option<ControllerOutput> {
        match *self {
            RecTarget::Insert {
                index,
                slot: Some(slot),
                control,
            } => Some(ControllerOutput {
                insert: index,
                slot,
                output: control.checked_sub(PLUGIN)?,
            }),
            _ => None,
        }
    }

    /// Decode an ID. Pass `legacy` for a project from before 24.2.99 or
    /// without a 103, see [`Project::legacy_rec_ids`](crate::Project::legacy_rec_ids).
    pub fn decode(id: u32, legacy: bool) -> Self {
        let id = if legacy { remap_legacy(id) } else { id };
        if id >> 16 == 0x4000 {
            return RecTarget::Global { control: id as u16 };
        }
        if id < CHANNELS_END {
            return RecTarget::Channel {
                index: (id >> 16) as u16,
                control: id as u16,
            };
        }
        // The dispatcher takes 0x70000000..=0xEFFFFFFF as mixer IDs. The
        // insert is added to the base and carries into bit 31.
        let Some(rel) = id.checked_sub(MIXER).filter(|rel| *rel < 0x8000_0000) else {
            return RecTarget::Other(id);
        };
        let index = (rel >> 22) as u16;
        if index > LAST_INSERT {
            return RecTarget::Other(id);
        }
        let control = id as u16;
        let slot = ((rel >> 16) & 0x3f) as u8;
        let slot_control =
            matches!(control, control::SLOT_ENABLED | control::SLOT_MIX) || control >= 0x8000;
        RecTarget::Insert {
            index,
            slot: (slot_control || slot != 0).then_some(slot),
            control,
        }
    }

    /// The lowest and the highest raw value of the control, for the
    /// controls the engine plays: insert volume, pan and send level,
    /// channel volume and pan, and the tempo. `None` for any other.
    ///
    /// The mixer and channel ranges are the dispatcher's clamps in
    /// `docs/rec-225.md`. The tempo's is FL's 10 to 522 BPM. No clamp was
    /// read for it: with that range the 219 of all seven corpus clips that
    /// drive the tempo says 60 to 180 BPM, which is what FL's "create
    /// automation clip" writes for a tempo (`docs/automation-234.md`).
    pub fn raw_range(&self) -> Option<(i32, i32)> {
        let sends = control::SEND..=control::SEND + LAST_INSERT;
        match *self {
            RecTarget::Insert {
                slot: None,
                control,
                ..
            } => match control {
                control::VOLUME => Some((0, 16_000)),
                control::PAN => Some((-6400, 6400)),
                send if sends.contains(&send) => Some((0, 16_000)),
                _ => None,
            },
            RecTarget::Global {
                control: control::TEMPO,
            } => Some((10_000, 522_000)),
            RecTarget::Channel {
                control: control::CHANNEL_VOLUME | control::CHANNEL_PAN,
                ..
            } => Some((0, 12_800)),
            _ => None,
        }
    }
}

impl crate::Project {
    /// What the target of a 226 or 227 link addresses in this project:
    /// [`Project::rec_target`](crate::Project::rec_target) after the
    /// rewrites FL's loader runs on the links of an old project, see
    /// [`remap_old_link`].
    ///
    /// FL rewrites a link's source the same way only when a flag of its
    /// own is set (bit 3 of the word at 0x193fc70), which is not read, so
    /// a source is decoded with [`Project::rec_target`](crate::Project::rec_target).
    /// An automation clip's source, control 0 of its channel, comes out
    /// the same either way.
    pub fn link_target(&self, id: u32) -> RecTarget {
        RecTarget::decode(remap_old_link(id, self.version()), self.legacy_rec_ids())
    }
}

/// The first writer version whose links number plugin parameters from
/// 0x8000, 11.5.6.
const PLUGIN_PARAMS_AT_0X8000: u32 = 11_005_006;
/// The first writer version whose current track is insert 126, 12.9.0.
const CURRENT_TRACK_126: u32 = 12_009_000;

/// The rewrites FL's loader runs on the IDs of a project's links when the
/// project's writer `version` is old (the version gates at the end of
/// `FUN_00f46490`, each rewrite run through `FUN_012cb6c0`, which walks
/// the links and the other lists of IDs, but not the 225 records):
///
/// - Before 11.5.6 (`FUN_00f45980`), a channel control from 0x2000 to
///   0x3FFF moves up by 0x6000 and a control below 0x1F00 of a legacy mixer
///   ID up by 0x8000. Plugin parameters started at 0x2000 on a channel and
///   at 0 in a slot then.
/// - Before 12.9.0 (`FUN_00f459e0`), a legacy mixer ID on insert 104 moves
///   to insert 501 with the new base, as [`remap_legacy`] does for 126.
///
/// The ones for writers before 8.5.0 are left out. The corpus has no
/// project that old.
pub fn remap_old_link(id: u32, version: u32) -> u32 {
    let mut id = id;
    let control = id & 0xffff;
    let legacy = (LEGACY_MIXER..2 * LEGACY_MIXER).contains(&id);
    if version < PLUGIN_PARAMS_AT_0X8000 {
        if id < CHANNELS_END && (0x2000..0x4000).contains(&control) {
            id += 0x6000;
        } else if legacy && control < 0x1F00 {
            id += 0x8000;
        }
    }
    if version < CURRENT_TRACK_126 && legacy && (id - LEGACY_MIXER) >> 22 == 104 {
        let slot = ((id - LEGACY_MIXER) >> 16) & 0x3f;
        id = MIXER + (((u32::from(LAST_INSERT) << 6) + slot) << 16) + (id & 0xffff);
    }
    id
}

/// The rewrite FL's loader runs on every 225 record of a legacy project
/// (`FUN_00f45460`): base 0x20000000 becomes 0x70000000, the send controls
/// move from 0x1F40.. to 0x2000.., and the current track moves from insert
/// 126 to 501. IDs outside 0x20000000..=0x3FFFFFFF come back as they are.
pub fn remap_legacy(id: u32) -> u32 {
    if !(LEGACY_MIXER..2 * LEGACY_MIXER).contains(&id) {
        return id;
    }
    let sends = (0x1F40..=0x1FBF).contains(&(id & 0xffff));
    let id = id + (MIXER - LEGACY_MIXER) + if sends { 0xC0 } else { 0 };
    let rel = id - MIXER;
    if rel >> 22 != 126 {
        return id;
    }
    let slot = (rel >> 16) & 0x3f;
    MIXER + (((u32::from(LAST_INSERT) << 6) + slot) << 16) + (id & 0xffff)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(insert: u32, slot: u32, control: u16) -> u32 {
        MIXER + (insert << 22) + (slot << 16) + u32::from(control)
    }

    fn insert(index: u16, slot: Option<u8>, control: u16) -> RecTarget {
        RecTarget::Insert {
            index,
            slot,
            control,
        }
    }

    #[test]
    fn a_mixer_id_splits_into_insert_slot_and_control() {
        assert_eq!(
            RecTarget::decode(0x7000_1FC0, false),
            insert(0, None, control::VOLUME)
        );
        assert_eq!(
            RecTarget::decode(id(5, 0, control::PAN), false),
            insert(5, None, control::PAN)
        );
        assert_eq!(
            RecTarget::decode(id(1, 0, control::SEND + 27), false),
            insert(1, None, control::SEND + 27)
        );
        // Slot controls keep their slot, slot 0 included.
        assert_eq!(
            RecTarget::decode(id(3, 0, control::SLOT_ENABLED), false),
            insert(3, Some(0), control::SLOT_ENABLED)
        );
        assert_eq!(
            RecTarget::decode(id(3, 9, control::SLOT_MIX), false),
            insert(3, Some(9), control::SLOT_MIX)
        );
        assert_eq!(
            RecTarget::decode(id(3, 2, 0x8004), false),
            insert(3, Some(2), 0x8004)
        );
    }

    #[test]
    fn the_insert_carries_into_bit_31() {
        // rec-225.md: hi16 is 0x8000 for insert 64 and 0xED40 for 501.
        assert_eq!(id(64, 0, 0), 0x8000_0000);
        assert_eq!(
            RecTarget::decode(0x8000_1FC0, false),
            insert(64, None, control::VOLUME)
        );
        assert_eq!(
            RecTarget::decode(0xED40_1FC1, false),
            insert(501, None, control::PAN)
        );
        assert_eq!(
            RecTarget::decode(0xED49_1F01, false),
            insert(501, Some(9), control::SLOT_MIX)
        );
    }

    #[test]
    fn ids_outside_the_mixer_and_the_globals_stay_as_they_are() {
        assert_eq!(
            RecTarget::decode(0x4000_0000, false),
            RecTarget::Global {
                control: control::MAIN_VOLUME
            }
        );
        assert_eq!(
            RecTarget::decode(0x4000_0005, true),
            RecTarget::Global { control: 5 }
        );
        for other in [
            0x1000_0000,
            0x4001_0000,
            0x5000_0000,
            0x6FFF_FFFF,
            // Insert 502 and up.
            0xED80_0000,
            0xF000_0000,
            0xFFFF_FFFF,
        ] {
            assert_eq!(
                RecTarget::decode(other, false),
                RecTarget::Other(other),
                "{other:#x}"
            );
        }
        // Without the remap an old ID addresses nothing.
        assert_eq!(
            RecTarget::decode(0x2040_1FC0, false),
            RecTarget::Other(0x2040_1FC0)
        );
    }

    #[test]
    fn an_id_below_0x10000000_is_a_channel_s_control() {
        let channel = |index, control| RecTarget::Channel { index, control };
        assert_eq!(RecTarget::decode(0, false), channel(0, 0));
        assert_eq!(
            RecTarget::decode(0x0003_0001, false),
            channel(3, control::CHANNEL_PAN)
        );
        // The legacy remap starts at 0x20000000 and leaves these alone.
        assert_eq!(RecTarget::decode(0x002A_8005, true), channel(42, 0x8005));
        assert_eq!(RecTarget::decode(0x0FFF_FFFF, true), channel(0xFFF, 0xFFFF));
    }

    #[test]
    fn the_controls_the_engine_plays_have_a_range() {
        let range = |id| RecTarget::decode(id, false).raw_range();
        assert_eq!(range(id(1, 0, control::VOLUME)), Some((0, 16_000)));
        assert_eq!(range(id(1, 0, control::PAN)), Some((-6400, 6400)));
        assert_eq!(range(id(1, 0, control::SEND)), Some((0, 16_000)));
        assert_eq!(range(id(1, 0, control::SEND + 501)), Some((0, 16_000)));
        assert_eq!(range(0x4000_0005), Some((10_000, 522_000)));
        assert_eq!(range(0x0002_0000), Some((0, 12_800)));
        assert_eq!(range(0x0002_0001), Some((0, 12_800)));
        // Separation, a slot's mix, a plugin parameter, the main volume
        // and a channel's pitch.
        for other in [
            id(1, 0, control::SEPARATION),
            id(1, 3, control::SLOT_MIX),
            id(1, 3, 0x8000),
            0x4000_0000,
            0x0002_0004,
        ] {
            assert_eq!(range(other), None, "{other:#x}");
        }
    }

    #[test]
    fn a_link_from_before_11_5_6_numbers_plugin_parameters_from_0x2000() {
        let old = 11_000_003;
        // A channel's plugin parameter 0x12.
        assert_eq!(remap_old_link(0x001B_2012, old), 0x001B_8012);
        assert_eq!(remap_old_link(0x001B_3FFF, old), 0x001B_9FFF);
        // A channel's own controls and its parameters from 0x4000 stay.
        for id in [0x001B_0004, 0x001B_1FFF, 0x001B_4000, 0x001B_8012] {
            assert_eq!(remap_old_link(id, old), id, "{id:#x}");
        }
        // A slot's parameter 5 on insert 3, slot 2, and a slot's mix level,
        // which stays.
        let legacy = |insert: u32, slot: u32, control: u32| {
            LEGACY_MIXER + (insert << 22) + (slot << 16) + control
        };
        assert_eq!(remap_old_link(legacy(3, 2, 5), old), legacy(3, 2, 0x8005));
        assert_eq!(
            remap_old_link(legacy(3, 2, 0x1F01), old),
            legacy(3, 2, 0x1F01)
        );
        // From 11.5.6 on nothing moves.
        assert_eq!(remap_old_link(0x001B_2012, 11_005_006), 0x001B_2012);
        assert_eq!(remap_old_link(legacy(3, 2, 5), 11_005_006), legacy(3, 2, 5));
    }

    #[test]
    fn a_link_from_before_12_9_0_to_insert_104_goes_to_501() {
        let id = LEGACY_MIXER + (104 << 22) + (3 << 16) + 0x1F01;
        assert_eq!(remap_old_link(id, 12_008_999), 0xED43_1F01);
        assert_eq!(
            RecTarget::decode(remap_old_link(id, 12_008_999), true),
            RecTarget::Insert {
                index: 501,
                slot: Some(3),
                control: control::SLOT_MIX
            }
        );
        assert_eq!(remap_old_link(id, 12_009_000), id);
        // A slot's parameter is shifted first, as FL's loader does.
        let param = LEGACY_MIXER + (104 << 22) + 7;
        assert_eq!(remap_old_link(param, 11_000_000), 0xED40_8007);
    }

    #[test]
    fn a_slot_s_control_from_0x8000_up_is_a_controller_output_as_a_source() {
        let output = |id, legacy| RecTarget::decode(id, legacy).controller_output();
        let at = |insert, slot, output| {
            Some(ControllerOutput {
                insert,
                slot,
                output,
            })
        };
        // The doc's example: insert 0, slot 9, output 1 of a legacy file.
        assert_eq!(output(0x2009_8001, true), at(0, 9, 1));
        assert_eq!(output(id(3, 0, 0x8000), false), at(3, 0, 0));
        assert_eq!(output(id(64, 2, 0x8002), false), at(64, 2, 2));
        // A slot's switch, an insert's volume, a channel's plugin and the
        // tempo are no controller in a slot.
        for other in [
            id(3, 2, control::SLOT_ENABLED),
            id(3, 0, control::VOLUME),
            0x0001_8001,
            0x4000_0005,
        ] {
            assert_eq!(output(other, false), None, "{other:#x}");
        }
    }

    #[test]
    fn a_legacy_id_moves_to_the_new_base() {
        assert_eq!(remap_legacy(0x2000_1FC0), 0x7000_1FC0);
        assert_eq!(
            RecTarget::decode(0x2040_1FC0, true),
            insert(1, None, control::VOLUME)
        );
        // Insert 100 is in the 0x3 range of the old encoding.
        assert_eq!(
            RecTarget::decode(0x2000_0000 + (100 << 22) + 0x1FC1, true),
            insert(100, None, control::PAN)
        );
        assert_eq!(
            RecTarget::decode(0x2083_1F00, true),
            insert(2, Some(3), control::SLOT_ENABLED)
        );
        // The remap leaves new IDs and globals alone.
        assert_eq!(remap_legacy(0x7040_1FC0), 0x7040_1FC0);
        assert_eq!(remap_legacy(0x4000_0000), 0x4000_0000);
        assert_eq!(remap_legacy(0x1FFF_FFFF), 0x1FFF_FFFF);
    }

    #[test]
    fn legacy_sends_move_from_0x1f40_to_0x2000() {
        assert_eq!(remap_legacy(0x2040_1F40), 0x7040_2000);
        assert_eq!(
            RecTarget::decode(0x2040_1F40 + 100, true),
            insert(1, None, control::SEND + 100)
        );
        assert_eq!(
            RecTarget::decode(0x2040_1FBF, true),
            insert(1, None, control::SEND + 127)
        );
        // 0x1F3F and 0x1FC0 are on either side of the send block.
        assert_eq!(remap_legacy(0x2040_1F3F), 0x7040_1F3F);
        assert_eq!(remap_legacy(0x2040_1FC0), 0x7040_1FC0);
    }

    #[test]
    fn the_legacy_current_track_moves_from_126_to_501() {
        let old = |slot: u32, control: u16| {
            LEGACY_MIXER + (126 << 22) + (slot << 16) + u32::from(control)
        };
        assert_eq!(remap_legacy(old(0, control::VOLUME)), 0xED40_1FC0);
        assert_eq!(
            RecTarget::decode(old(0, control::VOLUME), true),
            insert(501, None, control::VOLUME)
        );
        assert_eq!(
            RecTarget::decode(old(7, control::SLOT_MIX), true),
            insert(501, Some(7), control::SLOT_MIX)
        );
        // A send on the current track gets both moves.
        assert_eq!(
            RecTarget::decode(old(0, 0x1F40 + 3), true),
            insert(501, None, control::SEND + 3)
        );
        // 125 and 127 stay where they are.
        assert_eq!(
            RecTarget::decode(LEGACY_MIXER + (125 << 22) + 0x1FC0, true),
            insert(125, None, control::VOLUME)
        );
        assert_eq!(
            RecTarget::decode(LEGACY_MIXER + (127 << 22) + 0x1FC0, true),
            insert(127, None, control::VOLUME)
        );
    }
}
