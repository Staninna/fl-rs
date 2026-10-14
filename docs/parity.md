# .flp parity

Score for the `.flp` event dictionary. Source: the 165 project files FL Studio 26.1.7 ships in `Data/` (pinned in `corpus/manifest.sha256`), FL versions 10.9 through 26.1, plus the project loader in `FLEngine_x64.dll` read through Ghidra (`tools/ghidra/`). Full per-event detail, delimiters and version gates: `docs/event-dictionary.md`.

Container round trip: 165/165 projects and 8,433/8,433 `.fst` presets under `Data/` byte-exact (8,598 files), including the 0xAC escape to 32-bit event IDs that FL 25.2 introduced.

Events with a reading: 146/147. By confidence: loader+pyflp 98, loader 41, conflict 5, pyflp 2, none 1.

| id | kind | files | occurrences | name | payload | confidence |
|---:|---|---:|---:|---|---|---|
| 0 | u8 | 165 | 3443 | ChannelID.IsEnabled | bool | loader+pyflp |
| 9 | u8 | 165 | 165 | ProjectID.LoopActive | bool | loader+pyflp |
| 10 | u8 | 165 | 165 | ProjectID.ShowInfo | bool | loader+pyflp |
| 11 | u8 | 165 | 165 | RackID.Swing | u8 | loader+pyflp |
| 17 | u8 | 165 | 165 | ArrangementsID.TimeSigNum | u8 | loader+pyflp |
| 18 | u8 | 165 | 165 | ArrangementsID.TimeSigBeat | u8 | loader+pyflp |
| 20 | u8 | 165 | 3443 | ChannelID.PingPongLoop | u8 | loader+pyflp |
| 21 | u8 | 165 | 3443 | ChannelID.Type | u8 enum 0..5 | loader+pyflp |
| 22 | u8 | 131 | 2265 | ChannelID.RoutedTo (FL: FLP_TargetFXTrack) | i8 | conflict |
| 23 | u8 | 165 | 165 | ProjectID.PanLaw | u8, only bit 1 read | loader+pyflp |
| 25 | u8 | 2 | 2 | PatternU32At1C (coined) | u8 | loader |
| 26 | u8 | 2 | 2 | PatternID.Looped | bool | loader+pyflp |
| 27 | u8 | 59 | 6195 | comment: ChannelID.FXProperties | u8 bitfield | conflict |
| 28 | u8 | 165 | 165 | ProjectID.Licensed | bool | loader+pyflp |
| 29 | u8 | 165 | 165 | MixerID.APDC | bool | loader+pyflp |
| 30 | u8 | 156 | 156 | PatternsID.PlayTruncatedNotes | bool | loader+pyflp |
| 31 | u8 | 165 | 165 | ProjectBool16E8 (coined) | bool | loader |
| 32 | u8 | 106 | 3239 | ChannelID.IsLocked | bool | loader+pyflp |
| 33 | u8 | 26 | 219 | TimeMarkerID.Numerator | u8 | loader+pyflp |
| 34 | u8 | 26 | 219 | TimeMarkerID.Denominator | u8 | loader+pyflp |
| 35 | u8 | 84 | 84 | TimeSigMode (coined) | bool | loader |
| 36 | u8 | 84 | 87 | ArrangementByte10 (coined) | u8 | loader |
| 37 | u8 | 84 | 84 | Unknown37 (coined) | u8 (1 or 2 per PyFLP's discussion) | none |
| 38 | u8 | 76 | 76 | MixerDockPanels (coined) | u8 bits 0,1 | loader |
| 39 | u8 | 73 | 73 | ProjectBool0568 (coined) | bool | loader |
| 40 | u8 | 60 | 60 | ProjectBool40 (coined) | bool | loader |
| 41 | u8 | 53 | 3754 | PluginColorIsCustom (coined) | bool | loader |
| 42 | u8 | 53 | 4209 | InsertColorIsCustom (coined) | bool | loader |
| 43 | u8 | 53 | 27000 | TrackColorIsCustom (coined) | bool | loader |
| 44 | u8 | 53 | 53 | ProjectToggle44 (coined) | bool | loader |
| 45 | u8 | 13 | 113 | TimeMarkerFlag (coined) | u8 | loader |
| 46 | u8 | 13 | 113 | TimeMarkerByte (coined) | u8 | loader |
| 47 | u8 | 48 | 48 | ProjectBool47 (coined) | bool | loader |
| 48 | u8 | 39 | 1249 | ChannelBool224 (coined) | bool | loader |
| 49 | u8 | 35 | 1923 | InsertEnum1472 (coined) | u8 enum 0..2 | loader |
| 50 | u8 | 34 | 1178 | ChannelByte1FC (coined) | u8 | loader |
| 51 | u8 | 15 | 1003 | ChannelFlags51 (coined) | u8 bitfield | loader |
| 52 | u8 | 8 | 87 | PatternBoolA4 (coined) | bool | loader |
| 64 | u16 | 165 | 3443 | ChannelID.New | u16 index | loader+pyflp |
| 65 | u16 | 135 | 2811 | PatternID.New | u16 index | loader+pyflp |
| 67 | u16 | 165 | 165 | PatternsID.CurrentlySelected | u16 | loader+pyflp |
| 69 | u16 | 165 | 3443 | ChannelID.FreqTilt | u16 | loader+pyflp |
| 70 | u16 | 165 | 3443 | ChannelID.FXFlags | u16 bits | loader+pyflp |
| 71 | u16 | 165 | 3443 | ChannelID.Cutoff | u16 | loader+pyflp |
| 72 | u16 | 4 | 307 | ChannelID._VolWord | u16: high byte value, low byte step (inferred) | conflict |
| 74 | u16 | 165 | 3443 | ChannelID.Preamp | u16 | loader+pyflp |
| 75 | u16 | 165 | 3443 | ChannelID.FadeOut | u16 | loader+pyflp |
| 76 | u16 | 165 | 3443 | ChannelID.FadeIn | u16 | loader+pyflp |
| 80 | u16 | 165 | 165 | ProjectID.Pitch | i16 cents | loader+pyflp |
| 83 | u16 | 165 | 3443 | ChannelID.Resonance | u16 | loader+pyflp |
| 85 | u16 | 165 | 3443 | ChannelID.StereoDelay | u16 | loader+pyflp |
| 86 | u16 | 165 | 3443 | ChannelID.Pogo | u16 | loader+pyflp |
| 89 | u16 | 165 | 3443 | ChannelID.TimeShift | u16 | loader+pyflp |
| 91 | u16 | 1 | 177 | comment: ChannelID._Dot | u16 | loader+pyflp |
| 92 | u16 | 1 | 38 | comment: ChannelID._DotShift | u16 | loader+pyflp |
| 94 | u16 | 7 | 79 | ChannelID.Children | u16 iid | loader+pyflp |
| 95 | u16 | 48 | 1210 | InsertID.Icon | u32 write | loader+pyflp |
| 97 | u16 | 106 | 3239 | ChannelID.Swing | u16 | loader+pyflp |
| 98 | u16 | 106 | 104560 | SlotID.Index | u16: bits 0-6 slot, bit 8 flag | conflict |
| 99 | u16 | 84 | 87 | ArrangementID.New | u16 index | loader+pyflp |
| 100 | u16 | 84 | 84 | ArrangementsID.Current | u16 index | loader+pyflp |
| 101 | u16 | 13 | 113 | TimeMarkerNoteMask (coined) | u16, 12 bits | loader |
| 103 | u16 | 34 | 34 | FLP_FXTrackCount (FL string) | u16 count | loader |
| 104 | u16 | 34 | 1178 | FLP_TargetFXTrack16 (coined) | u16 0..501 | loader |
| 128 | u32 | 165 | 7008 | PluginID.Color | RGBA u32 | loader+pyflp |
| 131 | u32 | 165 | 3443 | ChannelID.RingMod | two u16 | loader+pyflp |
| 132 | u32 | 165 | 3415 | ChannelID.CutGroup | two u16 | loader+pyflp |
| 133 | u32 | 165 | 165 | RackID.WindowHeight | u32 | pyflp |
| 135 | u32 | 29 | 93 | ChannelID.RootNote | u32 | loader+pyflp |
| 138 | u32 | 165 | 3443 | ChannelID.DelayModXY | two u16 | loader+pyflp |
| 139 | u32 | 165 | 3443 | ChannelID.Reverb | two u16 | loader+pyflp |
| 142 | u32 | 2 | 5 | ChannelID.FineTune | i32 | loader+pyflp |
| 143 | u32 | 165 | 3443 | ChannelID.SamplerFlags | u32 bits | loader+pyflp |
| 144 | u32 | 165 | 3443 | ChannelID.LayerFlags | u32 | loader+pyflp |
| 145 | u32 | 165 | 3443 | ChannelID.GroupNum | i32 | loader+pyflp |
| 146 | u32 | 165 | 165 | ProjectID.CurGroupId | i32 | loader+pyflp |
| 147 | u32 | 165 | 16651 | InsertID.Output | i32 | loader+pyflp |
| 148 | u32 | 38 | 314 | TimeMarkerID.Position | u32: 24-bit position, 8-bit kind | loader+pyflp |
| 149 | u32 | 65 | 1908 | InsertID.Color | RGBA | loader+pyflp |
| 150 | u32 | 52 | 1335 | PatternID.Color | RGBA | loader+pyflp |
| 154 | u32 | 165 | 16651 | InsertID.Input | i32 | loader+pyflp |
| 155 | u32 | 165 | 7008 | PluginID.Icon | u32 | loader+pyflp |
| 156 | u32 | 165 | 165 | ProjectID.Tempo | u32 BPM x 1000 | loader+pyflp |
| 157 | u32 | 35 | 839 | comment: PatternID._157 | i32 | loader |
| 158 | u32 | 33 | 820 | comment: PatternID._158 | i32 | loader |
| 159 | u32 | 84 | 84 | ProjectID.FLBuild | u32 | pyflp |
| 160 | u32 | 7 | 321 | PatternID.ChannelIID | u32 | loader+pyflp |
| 161 | u32 | 7 | 321 | PatternID._161 | i32 | loader+pyflp |
| 162 | u32 | 2 | 2 | PatternID._162 | u32 | loader+pyflp |
| 164 | u32 | 31 | 755 | PatternID.Length | u32 ticks | loader+pyflp |
| 165 | u32 | 58 | 4844 | InsertEnum2F (coined) | u32 enum | loader |
| 166 | u32 | 58 | 4844 | InsertEnum30 (coined) | u32 enum | loader |
| 167 | u32 | 48 | 48 | ProjectEnum167 (coined) | u32 enum 0..2 | loader |
| 168 | u32 | 13 | 113 | TimeMarkerU32 (coined) | u32 | loader |
| 169 | u32 | 35 | 35 | DataFormatFlags (coined) | u32 bits | loader |
| 170 | u32 | 15 | 1003 | ChannelU32At714 (coined) | u32 | loader |
| 171 | u32 | 8 | 87 | PatternU32AtA8 (coined) | u32 | loader |
| 192 | var | 59 | 204 | ChannelID._Name | string | loader+pyflp |
| 193 | var | 52 | 1332 | PatternID.Name | string | loader+pyflp |
| 194 | var | 165 | 165 | ProjectID.Title | string | loader+pyflp |
| 195 | var | 106 | 106 | ProjectID.Comments | string | loader+pyflp |
| 196 | var | 63 | 1249 | ChannelID.SamplePath | string path | loader+pyflp |
| 197 | var | 40 | 40 | ProjectID.Url | string | loader+pyflp |
| 198 | var | 59 | 59 | ProjectID._RTFComments | 8-bit RTF | loader+pyflp |
| 199 | var | 165 | 165 | ProjectID.FLVersion | 8-bit string | loader+pyflp |
| 200 | var | 165 | 165 | ProjectID.Licensee | string | loader+pyflp |
| 201 | var | 164 | 7057 | PluginID.InternalName | string | loader+pyflp |
| 202 | var | 165 | 165 | ProjectID.DataPath | string path | loader+pyflp |
| 203 | var | 121 | 4037 | PluginID.Name | string | loader+pyflp |
| 204 | var | 69 | 1914 | InsertID.Name | string | loader+pyflp |
| 205 | var | 38 | 314 | TimeMarkerID.Name | string | loader+pyflp |
| 206 | var | 165 | 165 | ProjectID.Genre | string | loader+pyflp |
| 207 | var | 165 | 165 | ProjectID.Artists | string | loader+pyflp |
| 209 | var | 165 | 3443 | ChannelID.Delay | 20-byte struct | loader+pyflp |
| 212 | var | 164 | 7057 | PluginID.Wrapper | 52-byte struct | loader+pyflp |
| 213 | var | 153 | 4694 | PluginID.Data | plugin-specific | loader+pyflp |
| 215 | var | 165 | 3443 | ChannelID.Parameters | struct <= 168 | loader+pyflp |
| 216 | var | 136 | 136 | GlobalControllerEvents (coined) | 12-byte records | loader |
| 217 | var | 1 | 1 | ArrangementsID.PLSelection | two u32 | loader+pyflp |
| 218 | var | 165 | 17215 | ChannelID.EnvelopeLFO | 68-byte struct | loader+pyflp |
| 219 | var | 165 | 3443 | ChannelID.Levels | 24-byte struct | loader+pyflp |
| 221 | var | 165 | 3443 | ChannelID.Polyphony | 9-byte struct | loader+pyflp |
| 223 | var | 12 | 75 | PatternID.Controllers | 12-byte records | loader+pyflp |
| 224 | var | 133 | 1406 | PatternID.Notes | 24-byte records | loader+pyflp |
| 225 | var | 165 | 165 | MixerID.Params | 12-byte {u32, u32 REC id, i32 value} | conflict |
| 226 | var | 163 | 424 | ControllerID.MIDI | 20 bytes | loader+pyflp |
| 227 | var | 62 | 1624 | ControllerID.Remote | 20 bytes | loader+pyflp |
| 228 | var | 165 | 6886 | ChannelID.Tracking | 16-byte struct | loader+pyflp |
| 229 | var | 165 | 3443 | ChannelID.LevelAdjusts | 20-byte struct | loader+pyflp |
| 230 | var | 25 | 205 | ControllerFormula (coined) | string | loader |
| 231 | var | 165 | 333 | DisplayGroupID.Name | string | loader+pyflp |
| 233 | var | 112 | 115 | ArrangementID.Playlist | 32/60/80+ byte items | loader+pyflp |
| 234 | var | 49 | 1105 | ChannelID.Automation | raw, handed to the clip object | loader+pyflp |
| 235 | var | 165 | 16613 | InsertID.Routing | u8 array | loader+pyflp |
| 236 | var | 165 | 16651 | InsertID.Flags | 12-byte {u32, u32 flags, u32} (5 or 8 in old files) | loader+pyflp |
| 237 | var | 165 | 165 | ProjectID.Timestamp | two f64 | loader+pyflp |
| 238 | var | 165 | 44085 | TrackID.Data | struct <= 70 | loader+pyflp |
| 239 | var | 60 | 1957 | TrackID.Name | string | loader+pyflp |
| 240 | var | 1 | 5 | ProjectStringList (coined) | string | loader |
| 241 | var | 84 | 87 | ArrangementID.Name | string | loader+pyflp |
| 242 | var | 48 | 49 | ArrangementViewState (coined) | 28-byte struct | loader |
| 243 | var | 48 | 48 | ProjectPairU32 (coined) | two u32 | loader |
| 251 | var | 2 | 2 | ChannelSubState (coined) | 43 bytes | loader |
| 252 | var | 8 | 87 | PatternStringAC (coined) | string (empty in the corpus) | loader |
| 253 | var | 8 | 87 | PatternNoteMaskText (coined) | text list of 12 "0"/"1" | loader |
| 0x100 | u8 | 1 | 1 | ProjectBool256 (coined) |  | loader |
| 0xc0000101 | var | 8 | 8 | WriterName (coined) |  | loader |
