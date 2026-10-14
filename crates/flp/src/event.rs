//! Event IDs and what their payloads mean.
//!
//! The container fixes only the payload *size* per ID range. Meaning comes
//! from `docs/event-dictionary.md`, which has one row per ID: what FL
//! Studio's loader does with it and what PyFLP documents. This module is
//! that table as code. [`Id`] names every row, [`Value`] decodes the
//! fixed-size payloads, [`text`] decodes the strings, and [`layout`] holds
//! the structs and record arrays.
//!
//! A named ID is not a verified one. The tag at the end of each [`Id`]
//! constant's doc line says who vouches for the row: `(loader)` the FL
//! binary, `(pyflp)` PyFLP alone, `(loader+pyflp)` both, `(conflict)` they
//! disagree, `(none)` neither. `tests/corpus.rs` checks the size and
//! encoding claims against every shipped project and preset.

pub mod layout;

use crate::RawEvent;

/// Payload shape fixed by the ID byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    U8,
    U16,
    U32,
    Var,
}

/// Payload class of an ID. Byte IDs use bits 6..7, extended IDs bits 28..31
/// (0, 4, 8, 0xC). The parser rejects any other class, so `Var` is the only
/// case left for unknown values here.
pub fn kind(id: u32) -> Kind {
    let class = if id < 256 { (id >> 6) << 2 } else { id >> 28 };
    match class {
        0 => Kind::U8,
        4 => Kind::U16,
        8 => Kind::U32,
        _ => Kind::Var,
    }
}

/// FL Studio version that wrote the file, as a NUL-terminated ASCII string
/// such as `24.2.99.4720`. Present once in every corpus file.
pub const VERSION: u32 = 199;

/// Read the writer version out of an event stream.
pub fn version(events: &[crate::RawEvent]) -> Option<String> {
    let e = events.iter().find(|e| e.id == VERSION)?;
    let s = e.data.split(|&b| b == 0).next()?;
    Some(String::from_utf8_lossy(s).into_owned())
}

/// A version string as the number FL's loader compares against:
/// `major*1_000_000 + minor*1_000 + patch`, build number dropped.
/// `24.2.99.4720` is `24_002_099`.
///
/// The dictionary says three numbers. Eight corpus presets carry two
/// (`12.2`, `20.5`), so a missing minor or patch counts as 0 here.
pub fn version_number(s: &str) -> Option<u32> {
    let mut parts = s.split('.').map(|p| p.parse::<u32>().ok());
    let major = parts.next()??;
    let minor = parts.next().unwrap_or(Some(0))?;
    let patch = parts.next().unwrap_or(Some(0))?;
    major
        .checked_mul(1_000_000)?
        .checked_add(minor.checked_mul(1_000)?)?
        .checked_add(patch)
}

/// Whether a writer of this [`version_number`] stores strings as UTF-16LE.
/// The loader turns the flag on for versions above 11.5.4; older files hold
/// 8-bit strings.
pub fn strings_are_utf16(writer_version: u32) -> bool {
    writer_version > 11_005_004
}

/// What an event addresses. These are the values of the dictionary's scope
/// column with the parenthesised qualifier dropped, so `channel (sampler)`
/// and `channel (opens)` are both [`Scope::Channel`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    Channel,
    /// `channel x pattern` and `pattern x channel`.
    ChannelPattern,
    /// `channel x pattern step`.
    ChannelPatternStep,
    Project,
    Pattern,
    Insert,
    InsertSlot,
    Plugin,
    Arrangement,
    Track,
    TimeMarker,
    ControllerLink,
    DisplayGroup,
    /// The event stream itself: the 172 escape.
    Stream,
    /// `?`, an empty cell, or an ID the dictionary has no row for.
    Unknown,
}

/// An event ID: a byte ID below 256, or a 32-bit ID that the 0xAC escape
/// introduces.
///
/// Every dictionary row with a name has a constant, except 1, 4 and 54,
/// which the dictionary marks as census artefacts. PyFLP's `ChannelID.X`
/// and `ProjectID.X` become `X`. Members of its other enums keep the enum
/// word (`PatternID.Looped` is `PatternLooped`), and so does a channel or
/// project member whose bare word another scope also uses or that says
/// nothing alone (`ChannelName`, `ChannelNew`, `ChannelType`). A leading
/// underscore is dropped. Coined names and FL's own `FLP_` names stay as the
/// dictionary writes them.
///
/// Build one from a raw ID with `Id::from(event.id)`. Most `u32` values have
/// no name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub struct Id(pub u32);

impl From<u32> for Id {
    fn from(id: u32) -> Self {
        Id(id)
    }
}

impl From<Id> for u32 {
    fn from(id: Id) -> Self {
        id.0
    }
}

macro_rules! ids {
    ($($(#[$doc:meta])* $name:ident = $id:literal, $scope:ident;)*) => {
        #[allow(non_upper_case_globals)]
        impl Id {
            $($(#[$doc])* pub const $name: Id = Id($id);)*
        }

        /// One entry per named ID, in ascending ID order.
        const TABLE: &[(u32, &str, Scope)] = &[
            $(($id, stringify!($name), Scope::$scope),)*
        ];
    };
}

ids! {
    /// `ChannelID.IsEnabled`: channel, bool (loader+pyflp).
    IsEnabled = 0, Channel;
    /// `ChannelID._VolByte`: channel, u8 0..128 (loader+pyflp).
    VolByte = 2, Channel;
    /// `ChannelID._PanByte`: channel, u8 (loader+pyflp).
    PanByte = 3, Channel;
    /// `ProjectID.LoopActive`: project, bool (loader+pyflp).
    LoopActive = 9, Project;
    /// `ProjectID.ShowInfo`: project, bool (loader+pyflp).
    ShowInfo = 10, Project;
    /// `RackID.Swing`: project, u8 (loader+pyflp).
    RackSwing = 11, Project;
    /// `ProjectID._Volume`: project, u8 (loader+pyflp).
    Volume = 12, Project;
    /// `RackID._FitToSteps`: channel, u8 (conflict).
    RackFitToSteps = 13, Channel;
    /// channel, bool (loader).
    ChannelBool28F = 14, Channel;
    /// `ChannelID.Zipped`: channel, presence (loader+pyflp).
    Zipped = 15, Channel;
    /// channel, u8 (loader).
    ChannelByte28E = 16, Channel;
    /// `ArrangementsID.TimeSigNum`: project, u8 (loader+pyflp).
    ArrangementsTimeSigNum = 17, Project;
    /// `ArrangementsID.TimeSigBeat`: project, u8 (loader+pyflp).
    ArrangementsTimeSigBeat = 18, Project;
    /// `comment: ChannelID._19`: channel (sampler), presence (loader).
    Channel19 = 19, Channel;
    /// `ChannelID.PingPongLoop`: channel (sampler), u8 (loader+pyflp).
    PingPongLoop = 20, Channel;
    /// `ChannelID.Type`: channel, u8 enum 0..5 (loader+pyflp).
    ChannelType = 21, Channel;
    /// `ChannelID.RoutedTo (FL: FLP_TargetFXTrack)`: channel, i8 (conflict).
    RoutedTo = 22, Channel;
    /// `ProjectID.PanLaw`: project, u8, only bit 1 read (loader+pyflp).
    PanLaw = 23, Project;
    /// pattern, u8 (loader).
    PatternU32At1C = 25, Pattern;
    /// `PatternID.Looped`: pattern, bool (loader+pyflp).
    PatternLooped = 26, Pattern;
    /// `comment: ChannelID.FXProperties`: insert, u8 bitfield (conflict).
    FXProperties = 27, Insert;
    /// `ProjectID.Licensed`: project, bool (loader+pyflp).
    Licensed = 28, Project;
    /// `MixerID.APDC`: project (mixer), bool (loader+pyflp).
    MixerAPDC = 29, Project;
    /// `PatternsID.PlayTruncatedNotes`: project, bool (loader+pyflp).
    PatternsPlayTruncatedNotes = 30, Project;
    /// project, bool (loader).
    ProjectBool16E8 = 31, Project;
    /// `ChannelID.IsLocked`: channel, bool (loader+pyflp).
    IsLocked = 32, Channel;
    /// `TimeMarkerID.Numerator`: time marker (pending), u8 (loader+pyflp).
    TimeMarkerNumerator = 33, TimeMarker;
    /// `TimeMarkerID.Denominator`: time marker (pending), u8 (loader+pyflp).
    TimeMarkerDenominator = 34, TimeMarker;
    /// project, bool (loader).
    TimeSigMode = 35, Project;
    /// arrangement, u8 (loader).
    ArrangementByte10 = 36, Arrangement;
    /// ?, u8 (1 or 2 per PyFLP's discussion) (none).
    Unknown37 = 37, Unknown;
    /// project (mixer), u8 bits 0,1 (loader).
    MixerDockPanels = 38, Project;
    /// project, bool (loader).
    ProjectBool0568 = 39, Project;
    /// project, bool (loader).
    ProjectBool40 = 40, Project;
    /// plugin, bool (loader).
    PluginColorIsCustom = 41, Plugin;
    /// insert, bool (loader).
    InsertColorIsCustom = 42, Insert;
    /// track, bool (loader).
    TrackColorIsCustom = 43, Track;
    /// project, bool (loader).
    ProjectToggle44 = 44, Project;
    /// time marker (pending), u8 (loader).
    TimeMarkerFlag = 45, TimeMarker;
    /// time marker (pending), u8 (loader).
    TimeMarkerByte = 46, TimeMarker;
    /// project, bool (loader).
    ProjectBool47 = 47, Project;
    /// channel, bool (loader).
    ChannelBool224 = 48, Channel;
    /// insert, u8 enum 0..2 (loader).
    InsertEnum1472 = 49, Insert;
    /// channel, u8 (loader).
    ChannelByte1FC = 50, Channel;
    /// channel, u8 bitfield (loader).
    ChannelFlags51 = 51, Channel;
    /// pattern, bool (loader).
    PatternBoolA4 = 52, Pattern;
    /// `ChannelID.New`: channel (opens), u16 index (loader+pyflp).
    ChannelNew = 64, Channel;
    /// `PatternID.New`: pattern (selects), u16 index (loader+pyflp).
    PatternNew = 65, Pattern;
    /// `ProjectID._TempoCoarse`: project, u16 BPM (loader+pyflp).
    TempoCoarse = 66, Project;
    /// `PatternsID.CurrentlySelected`: project, u16 (loader+pyflp).
    PatternsCurrentlySelected = 67, Project;
    /// channel x pattern, u16 bitmask (loader).
    StepMask = 68, ChannelPattern;
    /// `ChannelID.FreqTilt`: channel, u16 (loader+pyflp).
    FreqTilt = 69, Channel;
    /// `ChannelID.FXFlags`: channel, u16 bits (loader+pyflp).
    FXFlags = 70, Channel;
    /// `ChannelID.Cutoff`: channel, u16 (loader+pyflp).
    Cutoff = 71, Channel;
    /// `ChannelID._VolWord`: channel x pattern step, u16: high byte value, low byte step (inferred) (conflict).
    VolWord = 72, ChannelPatternStep;
    /// `ChannelID._PanWord`: channel x pattern step, u16 (conflict).
    PanWord = 73, ChannelPatternStep;
    /// `ChannelID.Preamp`: channel, u16 (loader+pyflp).
    Preamp = 74, Channel;
    /// `ChannelID.FadeOut`: channel, u16 (loader+pyflp).
    FadeOut = 75, Channel;
    /// `ChannelID.FadeIn`: channel, u16 (loader+pyflp).
    FadeIn = 76, Channel;
    /// `comment: ChannelID._DotNote`: channel x pattern step, u16 (loader+pyflp).
    DotNote = 77, ChannelPatternStep;
    /// `comment: ChannelID._DotPitch`: channel x pattern step, u16 (loader+pyflp).
    DotPitch = 78, ChannelPatternStep;
    /// `comment: ChannelID._DotMix`: no scope or payload given (pyflp).
    DotMix = 79, Unknown;
    /// `ProjectID.Pitch`: project, i16 cents (loader+pyflp).
    Pitch = 80, Project;
    /// `ChannelID.Resonance`: channel, u16 (loader+pyflp).
    Resonance = 83, Channel;
    /// `comment: ChannelID._LoopBar`: project (playlist), u16 (loader+pyflp).
    LoopBar = 84, Project;
    /// `ChannelID.StereoDelay`: channel, u16 (loader+pyflp).
    StereoDelay = 85, Channel;
    /// `ChannelID.Pogo`: channel, u16 (loader+pyflp).
    Pogo = 86, Channel;
    /// `comment: ChannelID._DotReso`: channel x pattern step, u16 (loader+pyflp).
    DotReso = 87, ChannelPatternStep;
    /// `comment: ChannelID._DotCutOff`: channel x pattern step, u16 (loader+pyflp).
    DotCutOff = 88, ChannelPatternStep;
    /// `ChannelID.TimeShift`: channel, u16 (loader+pyflp).
    TimeShift = 89, Channel;
    /// `comment: ChannelID._Dot`: channel x pattern step, u16 (loader+pyflp).
    Dot = 91, ChannelPatternStep;
    /// `comment: ChannelID._DotShift`: channel x pattern step, u16 (loader+pyflp).
    DotShift = 92, ChannelPatternStep;
    /// `ProjectID._TempoFine`: project, u16 thousandths (loader+pyflp).
    TempoFine = 93, Project;
    /// `ChannelID.Children`: channel (layer), u16 iid (loader+pyflp).
    Children = 94, Channel;
    /// `InsertID.Icon`: insert, u32 write (loader+pyflp).
    InsertIcon = 95, Insert;
    /// `comment: ChannelID._DotRel`: no scope or payload given (pyflp).
    DotRel = 96, Unknown;
    /// `ChannelID.Swing`: channel, u16 (loader+pyflp).
    ChannelSwing = 97, Channel;
    /// `SlotID.Index`: insert slot, u16: bits 0-6 slot, bit 8 flag (conflict).
    SlotIndex = 98, InsertSlot;
    /// `ArrangementID.New`: arrangement (opens), u16 index (loader+pyflp).
    ArrangementNew = 99, Arrangement;
    /// `ArrangementsID.Current`: project, u16 index (loader+pyflp).
    ArrangementsCurrent = 100, Project;
    /// time marker (pending), u16, 12 bits (loader).
    TimeMarkerNoteMask = 101, TimeMarker;
    /// `FLP_FXTrackCount (FL string)`: project (mixer), u16 count (loader).
    FLP_FXTrackCount = 103, Project;
    /// channel, u16 0..501 (loader).
    FLP_TargetFXTrack16 = 104, Channel;
    /// `PluginID.Color`: plugin, RGBA u32 (loader+pyflp).
    PluginColor = 128, Plugin;
    /// `comment: ArrangementID._PlaylistItem`: arrangement (playlist), two u16 (loader+pyflp).
    ArrangementPlaylistItem = 129, Arrangement;
    /// `comment: ChannelID.Echo`: channel, two u16 (loader+pyflp).
    Echo = 130, Channel;
    /// `ChannelID.RingMod`: channel, two u16 (loader+pyflp).
    RingMod = 131, Channel;
    /// `ChannelID.CutGroup`: channel, two u16 (loader+pyflp).
    CutGroup = 132, Channel;
    /// `RackID.WindowHeight`: project, u32 (pyflp).
    RackWindowHeight = 133, Project;
    /// `ChannelID.RootNote`: channel (sampler), u32 (loader+pyflp).
    RootNote = 135, Channel;
    /// `comment: ChannelID._MainResoCutOff`: no scope or payload given (pyflp).
    MainResoCutOff = 137, Unknown;
    /// `ChannelID.DelayModXY`: channel, two u16 (loader+pyflp).
    DelayModXY = 138, Channel;
    /// `ChannelID.Reverb`: channel, two u16 (loader+pyflp).
    Reverb = 139, Channel;
    /// `ChannelID._StretchTime`: channel, u32 integer, cast to double (conflict).
    StretchTime = 140, Channel;
    /// channel, u32 (loader).
    ChannelU32At290 = 141, Channel;
    /// `ChannelID.FineTune`: channel (sampler), i32 (loader+pyflp).
    FineTune = 142, Channel;
    /// `ChannelID.SamplerFlags`: channel (sampler), u32 bits (loader+pyflp).
    SamplerFlags = 143, Channel;
    /// `ChannelID.LayerFlags`: channel, u32 (loader+pyflp).
    LayerFlags = 144, Channel;
    /// `ChannelID.GroupNum`: channel, i32 (loader+pyflp).
    GroupNum = 145, Channel;
    /// `ProjectID.CurGroupId`: project, i32 (loader+pyflp).
    CurGroupId = 146, Project;
    /// `InsertID.Output`: insert (closes), i32 (loader+pyflp).
    InsertOutput = 147, Insert;
    /// `TimeMarkerID.Position`: time marker (pending), u32: 24-bit position, 8-bit kind (loader+pyflp).
    TimeMarkerPosition = 148, TimeMarker;
    /// `InsertID.Color`: insert, RGBA (loader+pyflp).
    InsertColor = 149, Insert;
    /// `PatternID.Color`: pattern, RGBA (loader+pyflp).
    PatternColor = 150, Pattern;
    /// `ArrangementsID._LoopPos`: project (playlist), u32 (conflict).
    ArrangementsLoopPos = 152, Project;
    /// `ChannelID.AUSampleRate`: channel, u32 Hz (loader+pyflp).
    AUSampleRate = 153, Channel;
    /// `InsertID.Input`: insert, i32 (loader+pyflp).
    InsertInput = 154, Insert;
    /// `PluginID.Icon`: plugin, u32 (loader+pyflp).
    PluginIcon = 155, Plugin;
    /// `ProjectID.Tempo`: project, u32 BPM x 1000 (loader+pyflp).
    Tempo = 156, Project;
    /// `comment: PatternID._157`: pattern, i32 (loader).
    Pattern157 = 157, Pattern;
    /// `comment: PatternID._158`: pattern, i32 (loader).
    Pattern158 = 158, Pattern;
    /// `ProjectID.FLBuild`: project, u32 (pyflp).
    FLBuild = 159, Project;
    /// `PatternID.ChannelIID`: pattern (pending), u32 (loader+pyflp).
    PatternChannelIID = 160, Pattern;
    /// `PatternID._161`: pattern x channel, i32 (loader+pyflp).
    Pattern161 = 161, ChannelPattern;
    /// `PatternID._162`: pattern, u32 (loader+pyflp).
    Pattern162 = 162, Pattern;
    /// `PatternID.Length`: pattern, u32 ticks (loader+pyflp).
    PatternLength = 164, Pattern;
    /// insert, u32 enum (loader).
    InsertEnum2F = 165, Insert;
    /// insert, u32 enum (loader).
    InsertEnum30 = 166, Insert;
    /// project, u32 enum 0..2 (loader).
    ProjectEnum167 = 167, Project;
    /// time marker (pending), u32 (loader).
    TimeMarkerU32 = 168, TimeMarker;
    /// project, u32 bits (loader).
    DataFormatFlags = 169, Project;
    /// channel, u32 (loader).
    ChannelU32At714 = 170, Channel;
    /// pattern, u32 (loader).
    PatternU32AtA8 = 171, Pattern;
    /// stream, escape + u32 id (loader).
    ExtendedIdEscape = 172, Stream;
    /// `ChannelID._Name`: plugin, string (loader+pyflp).
    ChannelName = 192, Plugin;
    /// `PatternID.Name`: pattern, string (loader+pyflp).
    PatternName = 193, Pattern;
    /// `ProjectID.Title`: project, string (loader+pyflp).
    Title = 194, Project;
    /// `ProjectID.Comments`: project, string (loader+pyflp).
    Comments = 195, Project;
    /// `ChannelID.SamplePath`: channel, string path (loader+pyflp).
    SamplePath = 196, Channel;
    /// `ProjectID.Url`: project, string (loader+pyflp).
    Url = 197, Project;
    /// `ProjectID._RTFComments`: project, 8-bit RTF (loader+pyflp).
    RTFComments = 198, Project;
    /// `ProjectID.FLVersion`: project, 8-bit string (loader+pyflp).
    FLVersion = 199, Project;
    /// `ProjectID.Licensee`: project, string (loader+pyflp).
    Licensee = 200, Project;
    /// `PluginID.InternalName`: plugin (pending), string (loader+pyflp).
    PluginInternalName = 201, Plugin;
    /// `ProjectID.DataPath`: project, string path (loader+pyflp).
    DataPath = 202, Project;
    /// `PluginID.Name`: plugin, string (loader+pyflp).
    PluginName = 203, Plugin;
    /// `InsertID.Name`: insert, string (loader+pyflp).
    InsertName = 204, Insert;
    /// `TimeMarkerID.Name`: time marker (closes), string (loader+pyflp).
    TimeMarkerName = 205, TimeMarker;
    /// `ProjectID.Genre`: project, string (loader+pyflp).
    Genre = 206, Project;
    /// `ProjectID.Artists`: project, string (loader+pyflp).
    Artists = 207, Project;
    /// ?, 251 bytes (none).
    Unknown208 = 208, Unknown;
    /// `ChannelID.Delay`: channel, 20-byte struct (loader+pyflp).
    Delay = 209, Channel;
    /// channel (sample object), raw struct (loader).
    LegacySampleState = 210, Channel;
    /// `PluginID.Wrapper`: plugin (opens), 52-byte struct (loader+pyflp).
    PluginWrapper = 212, Plugin;
    /// `PluginID.Data`: plugin, plugin-specific (loader+pyflp).
    PluginData = 213, Plugin;
    /// `ChannelID.Parameters`: channel, struct <= 168 (loader+pyflp).
    Parameters = 215, Channel;
    /// project, 12-byte records (loader).
    GlobalControllerEvents = 216, Project;
    /// `ArrangementsID.PLSelection`: project (playlist), two u32 (loader+pyflp).
    ArrangementsPLSelection = 217, Project;
    /// `ChannelID.EnvelopeLFO`: channel (sampler), 68-byte struct (loader+pyflp).
    EnvelopeLFO = 218, Channel;
    /// `ChannelID.Levels`: channel, 24-byte struct (loader+pyflp).
    Levels = 219, Channel;
    /// `comment: ChannelID._Filter`: channel, two u32 (loader+pyflp).
    Filter = 220, Channel;
    /// `ChannelID.Polyphony`: channel, 9-byte struct (loader+pyflp).
    Polyphony = 221, Channel;
    /// `PatternID.Controllers`: pattern, 12-byte records (loader+pyflp).
    PatternControllers = 223, Pattern;
    /// `PatternID.Notes`: pattern, 24-byte records (loader+pyflp).
    PatternNotes = 224, Pattern;
    /// `MixerID.Params`: project (initial control values), 12-byte {u32, u32 REC id, i32 value} (conflict).
    MixerParams = 225, Project;
    /// `ControllerID.MIDI`: controller link, 20 bytes (loader+pyflp).
    ControllerMIDI = 226, ControllerLink;
    /// `ControllerID.Remote`: controller link, 20 bytes (loader+pyflp).
    ControllerRemote = 227, ControllerLink;
    /// `ChannelID.Tracking`: channel, 16-byte struct (loader+pyflp).
    Tracking = 228, Channel;
    /// `ChannelID.LevelAdjusts`: channel, 20-byte struct (loader+pyflp).
    LevelAdjusts = 229, Channel;
    /// controller link, string (loader).
    ControllerFormula = 230, ControllerLink;
    /// `DisplayGroupID.Name`: display group, string (loader+pyflp).
    DisplayGroupName = 231, DisplayGroup;
    /// `ArrangementID.Playlist`: arrangement, 32/60/80+ byte items (loader+pyflp).
    ArrangementPlaylist = 233, Arrangement;
    /// `ChannelID.Automation`: channel (automation clip), raw, handed to the clip object (loader+pyflp).
    Automation = 234, Channel;
    /// `InsertID.Routing`: insert, u8 array (loader+pyflp).
    InsertRouting = 235, Insert;
    /// `InsertID.Flags`: insert, 12-byte {u32, u32 flags, u32} (5 or 8 in old files) (loader+pyflp).
    InsertFlags = 236, Insert;
    /// `ProjectID.Timestamp`: project, two f64 (loader+pyflp).
    Timestamp = 237, Project;
    /// `TrackID.Data`: track (opens), struct <= 70 (loader+pyflp).
    TrackData = 238, Track;
    /// `TrackID.Name`: track, string (loader+pyflp).
    TrackName = 239, Track;
    /// project, string (loader).
    ProjectStringList = 240, Project;
    /// `ArrangementID.Name`: arrangement, string (loader+pyflp).
    ArrangementName = 241, Arrangement;
    /// arrangement (by index), 28-byte struct (loader).
    ArrangementViewState = 242, Arrangement;
    /// project, two u32 (loader).
    ProjectPairU32 = 243, Project;
    /// channel, string (loader).
    ChannelString718 = 245, Channel;
    /// channel, string (loader).
    ChannelString728 = 246, Channel;
    /// pattern, string (loader).
    PatternString94 = 247, Pattern;
    /// channel, string (loader).
    ChannelString720 = 248, Channel;
    /// channel, string (loader).
    ChannelString75C = 249, Channel;
    /// pattern, string (loader).
    PatternString9C = 250, Pattern;
    /// channel, 43 bytes (loader).
    ChannelSubState = 251, Channel;
    /// pattern, string (empty in the corpus) (loader).
    PatternStringAC = 252, Pattern;
    /// pattern, text list of 12 "0"/"1" (loader).
    PatternNoteMaskText = 253, Pattern;
    /// channel, string (loader).
    ChannelString73C = 254, Channel;
    /// channel, string (loader).
    ChannelString744 = 255, Channel;
    /// project, u8 (loader).
    ProjectBool256 = 0x0100, Project;
    /// project, var (loader).
    ProjectString100 = 0xC000_0100, Project;
    /// project, var (loader).
    WriterName = 0xC000_0101, Project;
    /// channel, var (loader).
    ChannelIntList102 = 0xC000_0102, Channel;
}

impl Id {
    fn row(self) -> Option<&'static (u32, &'static str, Scope)> {
        let i = TABLE.binary_search_by_key(&self.0, |row| row.0).ok()?;
        Some(&TABLE[i])
    }

    /// The constant's name, or `None` for an ID the dictionary has no row for.
    pub fn name(self) -> Option<&'static str> {
        Some(self.row()?.1)
    }

    /// What the event addresses. [`Scope::Unknown`] for an unnamed ID.
    pub fn scope(self) -> Scope {
        self.row().map_or(Scope::Unknown, |row| row.2)
    }

    /// Payload class, from the ID's range.
    pub fn kind(self) -> Kind {
        kind(self.0)
    }

    /// Whether this string ID is UTF-16 in a file whose strings are. 198 and
    /// 199 never are.
    fn is_wide(self, utf16: bool) -> bool {
        utf16 && self != Id::RTFComments && self != Id::FLVersion
    }

    /// Whether the dictionary gives the payload as a string.
    fn is_string(self) -> bool {
        matches!(
            self,
            Id::ChannelName
                | Id::PatternName
                | Id::Title
                | Id::Comments
                | Id::SamplePath
                | Id::Url
                | Id::RTFComments
                | Id::FLVersion
                | Id::Licensee
                | Id::PluginInternalName
                | Id::DataPath
                | Id::PluginName
                | Id::InsertName
                | Id::TimeMarkerName
                | Id::Genre
                | Id::Artists
                | Id::ControllerFormula
                | Id::DisplayGroupName
                | Id::TrackName
                | Id::ProjectStringList
                | Id::ArrangementName
                | Id::ChannelString718
                | Id::ChannelString728
                | Id::PatternString94
                | Id::ChannelString720
                | Id::ChannelString75C
                | Id::PatternString9C
                | Id::PatternStringAC
                | Id::PatternNoteMaskText
                | Id::ChannelString73C
                | Id::ChannelString744
                | Id::ProjectString100
                | Id::WriterName
                | Id::ChannelIntList102
        )
    }
}

/// A payload decoded by its [`Kind`]. Integers are little-endian and
/// unsigned; where the dictionary reads one as `i8`, `i16`, `i32` or `f32`,
/// cast it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value<'a> {
    U8(u8),
    U16(u16),
    U32(u32),
    Bytes(&'a [u8]),
}

impl<'a> Value<'a> {
    /// Decode an event's payload. `None` when a fixed-size ID carries the
    /// wrong number of bytes, which [`Project::parse`](crate::Project::parse)
    /// never produces.
    pub fn decode(e: &'a RawEvent) -> Option<Self> {
        let data = e.data.as_slice();
        Some(match kind(e.id) {
            Kind::U8 => Value::U8(u8::from_le_bytes(data.try_into().ok()?)),
            Kind::U16 => Value::U16(u16::from_le_bytes(data.try_into().ok()?)),
            Kind::U32 => Value::U32(u32::from_le_bytes(data.try_into().ok()?)),
            Kind::Var => Value::Bytes(data),
        })
    }

    pub fn kind(self) -> Kind {
        match self {
            Value::U8(_) => Kind::U8,
            Value::U16(_) => Kind::U16,
            Value::U32(_) => Kind::U32,
            Value::Bytes(_) => Kind::Var,
        }
    }
}

/// The code units of a string event, exactly as stored and without the NUL
/// terminator: UTF-16 units, or 8-bit bytes widened one to one.
///
/// `utf16` is [`strings_are_utf16`] of the file's
/// [`writer_version`](crate::Project::writer_version). 198 and 199 are 8-bit
/// in every file and ignore it.
///
/// Returns `None` when the ID is not a string in the dictionary, when the
/// payload does not end in a NUL, or when a UTF-16 payload has an odd
/// length. Nothing is replaced or dropped, so [`text_bytes`] of the result
/// is the payload again. Use this over [`text`] for anything that gets
/// written back: one corpus project has cut-off emoji in its comments.
pub fn text_units(e: &RawEvent, utf16: bool) -> Option<Vec<u16>> {
    let id = Id(e.id);
    if !id.is_string() {
        return None;
    }
    let mut units: Vec<u16> = if id.is_wide(utf16) {
        let (pairs, rest) = e.data.as_chunks::<2>();
        if !rest.is_empty() {
            return None;
        }
        pairs.iter().map(|u| u16::from_le_bytes(*u)).collect()
    } else {
        e.data.iter().map(|&b| u16::from(b)).collect()
    };
    (units.pop()? == 0).then_some(units)
}

/// The payload for a string event: the units from [`text_units`] plus the
/// NUL terminator. `None` when the ID is not a string, or when an 8-bit
/// string gets a unit above 0xFF.
pub fn text_bytes(id: Id, units: &[u16], utf16: bool) -> Option<Vec<u8>> {
    if !id.is_string() {
        return None;
    }
    let units = units.iter().chain(&[0]);
    if id.is_wide(utf16) {
        Some(units.flat_map(|u| u.to_le_bytes()).collect())
    } else {
        units.map(|&u| u8::try_from(u).ok()).collect()
    }
}

/// A string event as text, up to its first NUL. `None` in the same cases as
/// [`text_units`].
///
/// This is for reading, not for writing back: an unpaired UTF-16 surrogate
/// becomes U+FFFD. 8-bit bytes map one to one onto U+0000..U+00FF, because
/// FL writes the ANSI code page and the corpus holds nothing above ASCII to
/// tell which.
pub fn text(e: &RawEvent, utf16: bool) -> Option<String> {
    let units = text_units(e, utf16)?;
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    Some(String::from_utf16_lossy(&units[..end]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(id: Id, data: &[u8]) -> RawEvent {
        RawEvent {
            id: id.0,
            data: data.to_vec(),
        }
    }

    #[test]
    fn table_is_sorted_and_names_are_unique() {
        assert!(TABLE.windows(2).all(|w| w[0].0 < w[1].0));
        let mut names: Vec<&str> = TABLE.iter().map(|row| row.1).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), TABLE.len());
    }

    #[test]
    fn id_lookups() {
        assert_eq!(Id::ChannelName.0, 192);
        assert_eq!(Id::from(192), Id::ChannelName);
        assert_eq!(Id::ChannelName.name(), Some("ChannelName"));
        assert_eq!(Id::PatternName.scope(), Scope::Pattern);
        assert_eq!(Id::StepMask.scope(), Scope::ChannelPattern);
        assert_eq!(Id::WriterName.name(), Some("WriterName"));
        assert_eq!(Id::WriterName.kind(), Kind::Var);
        assert_eq!(Id::FLVersion.0, VERSION);
        assert_eq!(Id(5).name(), None);
        // 1, 4 and 54 were census artefacts and have no constant.
        assert!([1, 4, 54].iter().all(|&id| Id(id).name().is_none()));
        assert_eq!(Id(5).scope(), Scope::Unknown);
        assert_eq!(u32::from(Id::ProjectBool256), 0x100);
    }

    #[test]
    fn value_decodes_by_kind() {
        assert_eq!(
            Value::decode(&event(Id::IsEnabled, &[1])),
            Some(Value::U8(1))
        );
        assert_eq!(
            Value::decode(&event(Id::ChannelNew, &[0x34, 0x12])),
            Some(Value::U16(0x1234))
        );
        assert_eq!(
            Value::decode(&event(Id::Tempo, &[0x78, 0x56, 0x34, 0x12])),
            Some(Value::U32(0x1234_5678))
        );
        assert_eq!(
            Value::decode(&event(Id::PluginData, &[1, 2, 3])),
            Some(Value::Bytes(&[1, 2, 3]))
        );
        assert_eq!(
            Value::decode(&event(Id::ProjectBool256, &[7])),
            Some(Value::U8(7))
        );
        assert_eq!(Value::decode(&event(Id::Tempo, &[1, 2])), None);
        assert_eq!(Value::U16(0).kind(), Kind::U16);
    }

    #[test]
    fn version_numbers() {
        assert_eq!(version_number("24.2.99.4720"), Some(24_002_099));
        assert_eq!(version_number("11.5.4"), Some(11_005_004));
        assert_eq!(version_number("12.2"), Some(12_002_000));
        assert_eq!(version_number("20"), Some(20_000_000));
        assert_eq!(version_number(""), None);
        assert_eq!(version_number("12.x.1"), None);
        assert_eq!(version_number("99999.0.0"), None);
        assert!(!strings_are_utf16(11_005_004));
        assert!(strings_are_utf16(11_005_005));
    }

    #[test]
    fn text_8_bit() {
        let e = event(Id::Title, b"Caf\xe9\0");
        assert_eq!(text(&e, false).as_deref(), Some("Caf\u{e9}"));
        assert_eq!(text(&event(Id::Title, b"\0"), false).as_deref(), Some(""));
        assert_eq!(text(&event(Id::Title, b"no end"), false), None);
        assert_eq!(text(&event(Id::Title, b""), false), None);
        assert_eq!(text(&event(Id::Title, b"a\0b"), false), None);
        assert_eq!(
            text(&event(Id::Title, b"a\0b\0"), false).as_deref(),
            Some("a")
        );
    }

    #[test]
    fn text_utf16() {
        let e = event(Id::Title, &[b'h', 0, 0x3d, 0xd8, 0x48, 0xdc, 0, 0]);
        assert_eq!(text(&e, true).as_deref(), Some("h\u{1f448}"));
        // odd length, missing terminator, lone high surrogate
        assert_eq!(text(&event(Id::Title, &[b'h', 0, 0]), true), None);
        assert_eq!(text(&event(Id::Title, &[b'h', 0]), true), None);
        let lone = event(Id::Title, &[0x3d, 0xd8, 0, 0]);
        assert_eq!(text(&lone, true).as_deref(), Some("\u{fffd}"));
    }

    #[test]
    fn text_units_are_exact() {
        // A lone high surrogate, which `text` cannot give back.
        let wide = event(Id::Comments, &[b'h', 0, 0x3d, 0xd8, b'i', 0, 0, 0]);
        let units = text_units(&wide, true).unwrap();
        assert_eq!(units, [0x68, 0xd83d, 0x69]);
        assert_eq!(text_bytes(Id::Comments, &units, true), Some(wide.data));

        let narrow = event(Id::Comments, b"Caf\xe9\0");
        let units = text_units(&narrow, false).unwrap();
        assert_eq!(units, [0x43, 0x61, 0x66, 0xe9]);
        assert_eq!(text_bytes(Id::Comments, &units, false), Some(narrow.data));

        // An inner NUL stays in the units.
        let inner = event(Id::Title, b"a\0b\0");
        let units = text_units(&inner, false).unwrap();
        assert_eq!(units, [0x61, 0, 0x62]);
        assert_eq!(text_bytes(Id::Title, &units, false), Some(inner.data));

        // 199 is 8-bit in a UTF-16 file, in both directions.
        let version = event(Id::FLVersion, b"25.1\0");
        let units = text_units(&version, true).unwrap();
        assert_eq!(text_bytes(Id::FLVersion, &units, true), Some(version.data));

        assert_eq!(text_units(&event(Id::Title, b"no end"), false), None);
        assert_eq!(text_units(&event(Id::Title, &[b'h', 0, 0]), true), None);
        assert_eq!(text_units(&event(Id::PluginData, b"x\0"), false), None);
        assert_eq!(text_bytes(Id::Title, &[0x100], false), None);
        assert_eq!(text_bytes(Id::PluginData, &[0x61], false), None);
        assert_eq!(text_bytes(Id::Title, &[], true), Some(vec![0, 0]));
    }

    #[test]
    fn text_knows_which_ids_are_strings() {
        // 198 and 199 stay 8-bit in a UTF-16 file.
        assert_eq!(
            text(&event(Id::FLVersion, b"25.1.0.1\0"), true).as_deref(),
            Some("25.1.0.1")
        );
        assert_eq!(
            text(&event(Id::RTFComments, b"{\\rtf1}\0"), true).as_deref(),
            Some("{\\rtf1}")
        );
        assert_eq!(
            text(&event(Id::WriterName, &[b'F', 0, 0, 0]), true).as_deref(),
            Some("F")
        );
        assert_eq!(text(&event(Id::PluginData, b"x\0"), false), None);
        assert_eq!(text(&event(Id(5), b"x\0"), false), None);
    }
}
