//! Channel rack entries.

use crate::codec::{self, Codec, Cx, object};
use crate::{Item, Placed};
use flp::RawEvent;
use flp::event::{Id, layout};

/// What a channel is, from 21 `ChannelID.Type`.
///
/// The dictionary gives the range 0..5 and no names. These are FL's usual
/// ones, and the corpus fits them: every type 5 channel has a 234 automation
/// payload and no other type does, 94 children only appear on type 3, every
/// type 2 has 213 plugin data, and 836 of 837 type 4 channels have a sample
/// path. Type 1 is in no corpus project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelKind {
    /// Type 0.
    Sampler,
    /// Type 2. The plugin's name and state are [`Channel::plugin_name`] and
    /// [`Channel::plugin_state`].
    Plugin,
    /// Type 3. Its members are [`Channel::children`].
    Layer,
    /// Type 4.
    AudioClip,
    /// Type 5. The clip's data is [`Channel::automation`].
    Automation,
    /// Type 1, or a value outside the range FL's loader accepts.
    Other(u8),
}

impl Codec for ChannelKind {
    fn decode(e: &RawEvent, cx: &Cx) -> Option<Self> {
        Some(match u8::decode(e, cx)? {
            0 => Self::Sampler,
            2 => Self::Plugin,
            3 => Self::Layer,
            4 => Self::AudioClip,
            5 => Self::Automation,
            other => Self::Other(other),
        })
    }

    fn encode(&self, _: Id, _: &Cx) -> Option<Vec<u8>> {
        Some(vec![match *self {
            Self::Sampler => 0,
            Self::Plugin => 2,
            Self::Layer => 3,
            Self::AudioClip => 4,
            Self::Automation => 5,
            Self::Other(other) => other,
        }])
    }
}

/// One channel. 64 `ChannelID.New` opens it, and it stays the current
/// channel until the next 64.
///
/// Every channel in the corpus carries 201, 212 and the plugin-scope events
/// whatever its kind, a sampler included, so the plugin fields sit here and
/// not inside [`ChannelKind::Plugin`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Channel {
    /// 64 `ChannelID.New`: u16 index into the channel list, from 0.
    pub index: u16,
    /// 21 `ChannelID.Type`: u8 enum 0..5.
    pub kind: Option<ChannelKind>,
    /// 203 `PluginID.Name` or 192 `ChannelID._Name`: string. The loader
    /// handles the two alike, so when a channel has both, the later one is
    /// the name.
    pub name: Option<String>,
    /// 0 `ChannelID.IsEnabled`: bool.
    pub enabled: Option<bool>,
    /// 32 `ChannelID.IsLocked`: bool.
    pub locked: Option<bool>,
    /// 219 `ChannelID.Levels`: 24-byte struct with pan, volume, mod X and
    /// mod Y as u32. See [`Channel::volume`] and [`Channel::pan`]. On an
    /// automation channel the first two fields are the clip's range, see
    /// [`Channel::automation_range`].
    pub levels: Option<layout::Levels>,
    /// 229 `ChannelID.LevelAdjusts`: 20-byte struct.
    pub level_adjusts: Option<layout::LevelAdjusts>,
    /// 196 `ChannelID.SamplePath`: string path.
    pub sample_path: Option<String>,
    /// 22 `ChannelID.RoutedTo`: i8, the target insert. Files have this or
    /// [`Channel::target_insert`], never both.
    pub routed_to: Option<i8>,
    /// 104 `FLP_TargetFXTrack16`: u16 0..501, the 16-bit form of 22.
    pub target_insert: Option<u16>,
    /// 220 `ChannelID._Filter`: two u32, the legacy cutoff and resonance
    /// pair. No corpus project has it.
    pub filter: Option<layout::Filter>,
    /// 218 `ChannelID.EnvelopeLFO`: 68-byte struct, one event per envelope
    /// in channel order.
    pub envelopes: Vec<layout::EnvelopeLfo>,
    /// 221 `ChannelID.Polyphony`: 9-byte struct.
    pub polyphony: Option<layout::Polyphony>,
    /// 209 `ChannelID.Delay`: 20-byte struct.
    pub delay: Option<layout::Delay>,
    /// 228 `ChannelID.Tracking`: 16-byte struct, two events per channel in
    /// order.
    pub tracking: Vec<layout::Tracking>,
    /// 215 `ChannelID.Parameters`: struct of up to 168 bytes.
    pub parameters: Option<layout::Parameters>,
    /// 135 `ChannelID.RootNote`: u32.
    pub root_note: Option<u32>,
    /// 142 `ChannelID.FineTune`: i32.
    pub fine_tune: Option<i32>,
    /// 145 `ChannelID.GroupNum`: i32.
    pub group: Option<i32>,
    /// 94 `ChannelID.Children`: u16 channel index, one event per member of
    /// a layer.
    pub children: Vec<u16>,
    /// 234 `ChannelID.Automation`: the automation clip's points, with the
    /// bytes around them kept. What the clip drives is not in here: see
    /// [`Project::automation_bindings`](crate::Project::automation_bindings).
    /// A payload that is not laid out as `docs/automation-234.md` says
    /// stays a raw event.
    pub automation: Option<layout::AutomationData>,
    /// 128 `PluginID.Color`: RGBA u32.
    pub color: Option<u32>,
    /// 41 `PluginColorIsCustom`: bool.
    pub color_is_custom: Option<bool>,
    /// 155 `PluginID.Icon`: u32.
    pub icon: Option<u32>,
    /// 201 `PluginID.InternalName`: string, the name FL instantiates the
    /// plugin by. Parked until the 212 that follows it.
    pub plugin_name: Option<String>,
    /// 212 `PluginID.Wrapper`: 52-byte struct. Flag bit 4 marks a generator,
    /// which is what sends the 212 to the channel and not to an insert slot.
    pub wrapper: Option<layout::PluginWrapper>,
    /// 213 `PluginID.Data`: plugin-specific bytes.
    pub plugin_state: Option<Vec<u8>>,
    /// Every event of the channel in the order it arrived.
    pub order: Vec<Placed>,
}

object!(Channel {
    key index = ChannelNew;
    one kind = ChannelType;
    one name = PluginName | ChannelName;
    one enabled = IsEnabled;
    one locked = IsLocked;
    one levels = Levels;
    one level_adjusts = LevelAdjusts;
    one sample_path = SamplePath;
    one routed_to = RoutedTo;
    one target_insert = FLP_TargetFXTrack16;
    one filter = Filter;
    each envelopes = EnvelopeLFO;
    one polyphony = Polyphony;
    one delay = Delay;
    each tracking = Tracking;
    one parameters = Parameters;
    one root_note = RootNote;
    one fine_tune = FineTune;
    one group = GroupNum;
    each children = Children;
    one automation = Automation;
    one color = PluginColor;
    one color_is_custom = PluginColorIsCustom;
    one icon = PluginIcon;
    one plugin_name = PluginInternalName;
    one wrapper = PluginWrapper;
    one plugin_state = PluginData;
});

/// The events of a new sampler channel, in the order FL writes them: those
/// of the one channel in the `Templates/Empty/Empty.flp` that FL 2026
/// ships, which FL 24.2.99 wrote, and behind them the 196 that every
/// corpus channel with a sample has right behind its 20. The payloads are
/// that channel's, but for the four that [`Channel::new_sampler`] fills in
/// and that are empty here.
///
/// The payloads are laid out as FL 24.2.99 writes them. An older file gets
/// the same bytes, and FL's loader may read some of them through that
/// file's version: 215 has grown with every generation of FL.
const NEW_SAMPLER: &[(u32, &[u8])] = &[
    // ChannelID.New, which carries the channel's index.
    (64, b""),
    // ChannelID.Type: a sampler.
    (21, b"\x00"),
    // PluginID.InternalName: empty, a sampler hosts no plugin.
    (201, b""),
    // PluginID.Wrapper: a generator, flags 0x50.
    (
        212,
        b"\x00\x00\x00\x00\x00\x00\x00\x00\xff\xff\xff\xff\x00\x00\x00\x00\
          \x50\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\
          \x00\x00\x00\x00\xc2\x00\x00\x00\x16\x01\x00\x00\x00\x00\x00\x00\
          \x00\x00\x00\x00",
    ),
    // PluginID.Name, which carries the channel's name.
    (203, b""),
    // PluginID.Icon.
    (155, b"\x00\x00\x00\x00"),
    // PluginID.Color: the grey of a channel without a colour of its own.
    (128, b"\x5c\x65\x6a\x00"),
    // PluginColorIsCustom.
    (41, b"\x00"),
    // ChannelID.IsEnabled.
    (0, b"\x01"),
    // ChannelID.Delay.
    (
        209,
        b"\x00\x00\x00\x00\x00\x19\x00\x00\x00\x00\x00\x00\x04\x00\x00\x00\x90\x00\x00\x00",
    ),
    // ChannelID.DelayModXY.
    (138, b"\x80\x00\x80\x00"),
    // ChannelID.Reverb.
    (139, b"\x00\x00\x01\x00"),
    // ChannelID.TimeShift.
    (89, b"\x00\x00"),
    // ChannelID.Swing.
    (97, b"\x80\x00"),
    // ChannelBool224.
    (48, b"\x00"),
    // ChannelID.FreqTilt.
    (69, b"\x80\x00"),
    // ChannelID.Pogo.
    (86, b"\x00\x01"),
    // ChannelID.Cutoff.
    (71, b"\x00\x04"),
    // ChannelID.Resonance.
    (83, b"\x00\x00"),
    // ChannelID.Preamp.
    (74, b"\x00\x00"),
    // ChannelID.FadeOut.
    (75, b"\x00\x00"),
    // ChannelID.FadeIn.
    (76, b"\x00\x00"),
    // ChannelID.StereoDelay.
    (85, b"\x00\x08"),
    // ChannelID.RingMod.
    (131, b"\x00\x00\x80\x00"),
    // ChannelID.FXFlags.
    (70, b"\x00\x00"),
    // FLP_TargetFXTrack16: the master.
    (104, b"\x00\x00"),
    // ChannelByte1FC.
    (50, b"\x01"),
    // ChannelID.Levels: pan 6400, volume 10000, mod X 256.
    (
        219,
        b"\x00\x19\x00\x00\x10\x27\x00\x00\x00\x00\x00\x00\x00\x01\x00\x00\
          \x00\x00\x00\x00\x00\x00\x00\x00",
    ),
    // ChannelID.LevelAdjusts.
    (
        229,
        b"\x00\x00\x00\x00\x00\x32\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00",
    ),
    // ChannelID.Polyphony.
    (221, b"\x00\x00\x00\x00\x34\x03\x00\x00\x00"),
    // ChannelID.Parameters.
    (
        215,
        b"\xff\xff\xff\xff\x00\x00\x00\x00\x00\x00\x00\x01\xff\xff\xff\xff\
          \x3c\x00\x00\x00\x00\x00\x80\x3f\x00\x00\x80\x3f\x00\x00\x80\x3f\
          \x00\x00\x80\x3f\x00\x00\x80\x3f\x00\x00\x00\x00\x01\x00\x00\x00\
          \xff\xff\xff\xff\x00\x04\x00\x00\x30\x00\x00\x00\x00\x00\x00\x00\
          \xa7\x05\x00\x00\x00\x00\x00\x00\x00\x01\x00\x00\x00\x00\x00\x00\
          \x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x01\x00\x00\x00\
          \x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\
          \x02\x00\x00\x00\xfe\xff\xff\xff\xff\xff\xff\xff\x00\x00\x00\x00\
          \x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\xf0\x3f\
          \x00\x00\x00\x00\x00\x00\x00\x00\xff\xff\xff\xff\x00\x01\x00\x00\
          \x00\x00\x00\x00\x00\x00\xe0\x3f",
    ),
    // ChannelID.CutGroup.
    (132, b"\x00\x00\x00\x00"),
    // ChannelID.LayerFlags.
    (144, b"\x00\x00\x00\x00"),
    // ChannelID.GroupNum.
    (145, b"\x00\x00\x00\x00"),
    // ChannelID.IsLocked.
    (32, b"\x00"),
    // ChannelID.Tracking, twice.
    (
        228,
        b"\x64\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00",
    ),
    (
        228,
        b"\x3c\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00",
    ),
    // ChannelID.EnvelopeLFO, one for each of the five envelopes.
    (
        218,
        b"\x00\x00\x00\x00\x00\x00\x00\x00\x64\x00\x00\x00\x20\x4e\x00\x00\
          \x20\x4e\x00\x00\x30\x75\x00\x00\x32\x00\x00\x00\x20\x4e\x00\x00\
          \x00\x00\x00\x00\x64\x00\x00\x00\x20\x4e\x00\x00\x00\x00\x00\x00\
          \xb6\x80\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\
          \x00\x00\x00\x00",
    ),
    (
        218,
        b"\x14\x00\x00\x00\x00\x00\x00\x00\x64\x00\x00\x00\x20\x4e\x00\x00\
          \x20\x4e\x00\x00\x30\x75\x00\x00\x32\x00\x00\x00\x20\x4e\x00\x00\
          \x00\x00\x00\x00\x64\x00\x00\x00\x20\x4e\x00\x00\x00\x00\x00\x00\
          \xb6\x80\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\
          \x9b\xff\xff\xff",
    ),
    (
        218,
        b"\x00\x00\x00\x00\x00\x00\x00\x00\x64\x00\x00\x00\x20\x4e\x00\x00\
          \x20\x4e\x00\x00\x30\x75\x00\x00\x32\x00\x00\x00\x20\x4e\x00\x00\
          \x00\x00\x00\x00\x64\x00\x00\x00\x20\x4e\x00\x00\x00\x00\x00\x00\
          \xb6\x80\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\
          \x00\x00\x00\x00",
    ),
    (
        218,
        b"\x00\x00\x00\x00\x00\x00\x00\x00\x64\x00\x00\x00\x20\x4e\x00\x00\
          \x20\x4e\x00\x00\x30\x75\x00\x00\x32\x00\x00\x00\x20\x4e\x00\x00\
          \x00\x00\x00\x00\x64\x00\x00\x00\x20\x4e\x00\x00\x00\x00\x00\x00\
          \xb6\x80\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\
          \x00\x00\x00\x00",
    ),
    (
        218,
        b"\x00\x00\x00\x00\x00\x00\x00\x00\x64\x00\x00\x00\x20\x4e\x00\x00\
          \x20\x4e\x00\x00\x30\x75\x00\x00\x32\x00\x00\x00\x20\x4e\x00\x00\
          \x00\x00\x00\x00\x64\x00\x00\x00\x20\x4e\x00\x00\x00\x00\x00\x00\
          \xb6\x80\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\
          \x00\x00\x00\x00",
    ),
    // ChannelID.SamplerFlags.
    (143, b"\x03\x00\x00\x00"),
    // ChannelID.PingPongLoop.
    (20, b"\x00"),
    // ChannelID.SamplePath, which carries the sample's path.
    (196, b""),
];

impl Channel {
    /// A sampler channel as FL makes a new one, with the events of
    /// [`NEW_SAMPLER`] at `pos`: `index` in its 64, `name` in its 203 and
    /// `sample` in its 196.
    ///
    /// The insert it plays into is the master. That is in a 104 when
    /// `wide` is set, as it is for a project with a 103, and in a 22
    /// otherwise: no corpus file has both.
    pub(crate) fn new_sampler(
        index: u16,
        name: &str,
        sample: &str,
        wide: bool,
        pos: usize,
    ) -> Channel {
        let mut channel = Channel {
            index,
            plugin_name: Some(String::new()),
            name: Some(name.to_owned()),
            sample_path: Some(sample.to_owned()),
            ..Channel::default()
        };
        // None of these payloads is a string, so they read the same
        // whatever the file's version is.
        let cx = Cx::default();
        for &(id, data) in NEW_SAMPLER {
            let id = match Id::from(id) {
                Id::FLP_TargetFXTrack16 if !wide => Id::RoutedTo,
                id => id,
            };
            let data = match id {
                Id::ChannelNew => index.to_le_bytes().to_vec(),
                Id::RoutedTo => vec![0],
                _ => data.to_vec(),
            };
            let strings = [Id::PluginInternalName, Id::PluginName, Id::SamplePath];
            if strings.contains(&id) {
                let item = Item::Field(id);
                channel.order.push(Placed { pos, item });
            } else {
                codec::give(&mut channel, pos, &RawEvent { id: id.0, data }, &cx);
            }
        }
        channel
    }
}

impl Channel {
    /// Volume from 219 @4, a u32 as FL stores it. The legacy 2 `_VolByte`
    /// has another scale and stays a raw event.
    pub fn volume(&self) -> Option<u32> {
        Some(self.levels?.volume)
    }

    /// Pan from 219 @0, a u32 as FL stores it. The legacy 3 `_PanByte` stays
    /// a raw event.
    pub fn pan(&self) -> Option<u32> {
        Some(self.levels?.pan)
    }

    /// The insert the channel plays into, from 104 or 22. This is the
    /// loader's insert number, where the last insert of the file is 501, and
    /// not a position in [`Project::inserts`](crate::Project::inserts).
    pub fn insert(&self) -> Option<i32> {
        match self.target_insert {
            Some(target) => Some(i32::from(target)),
            None => self.routed_to.map(i32::from),
        }
    }
}
