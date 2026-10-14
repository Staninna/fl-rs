# FLP event dictionary: FL Studio 26.1.7 loader, PyFLP, census

One table for every `.flp` event ID that the 165 shipped projects use
(`census.txt`), that the FLEngine_x64.dll project loader dispatches
(`ghidra/out/projloop/FUN_00f46490_00f46490.c`), or that PyFLP names
(`event-ids.md`). The loader column is read from the decompiled C, not from
PyFLP, and PyFLP's facts are restated in my own words. Where the two disagree
the row says so.

Decompiled C is lossy. Ghidra dropped the third and fourth register arguments
(r8, r9) of several calls, so some rows say "argument lost". Anything I worked
out rather than read is marked "(inferred)".

## How to read the rows

Shorthand used in the `loader action` column. Section "Reading the loader" at
the end explains where each comes from.

- `value`: the decoded fixed-size payload, read as a signed 64-bit integer
  (`stream+0x10002c`). `lo`/`hi`: its low and high 16 bits. For var events the
  same slot holds a pointer to the payload bytes and `len` is
  `stream+0x100024`.
- `ver`: the writer version from ID 199, as `major*1000000 + minor*1000 +
  patch`. Gates are written as the dotted version, so `ver<3.6.5` means the
  constant `0x2dde35`.
- `chan`: the current channel object. `smp` is its sampler sub-object
  (`chan+0x330`). `plug`: the current plugin host (the channel after a 64, the
  slot plugin after a 212). `ins[e0]`: the current insert record (array
  `PTR_DAT_015e06a0`, stride 0x1478, index `e0`). `pat[c4]`: the current
  pattern record (array `PTR_DAT_015e2aa0`, stride 0xc0, index `c4`);
  `patobj(c4)` is the pattern's note container from `FUN_012d16e0(c4,1)`.
  `arr`: the current arrangement; `arr.trk[c8]` is its track record
  (`arr+0x70 + c8*0x114`). `projobj`: the project object behind
  `PTR_DAT_015e4790->+0x10`. `ui(x)`: a UI control fetched from a global, with
  its setter called.
- `Move(n)`: byte copy of the first `n` payload bytes (`FUN_00409c80` or the
  `PTR_DAT_015e0030` function pointer). `string`: `FUN_00a9e8e0`, which decodes
  8-bit or UTF-16LE depending on the flag set by 199.
- "no branch": the dispatcher has no case for the ID. FL reads the payload (the
  stream advance is generic) and discards it.
- `kind` is the size class from the ID range; `i8`, `i16`, `i32`, `f32` where
  PyFLP reads it signed or as a float.
- `confidence`: `loader+pyflp` both agree; `loader` only the binary says
  anything; `pyflp` the loader has no branch; `conflict` they disagree (the row
  says how); `none` neither source gives a meaning.
- Names: PyFLP's `Enum.Member` when it has one; "comment:" for names PyFLP only
  carries as a commented-out line. Names I made up end in "(coined)". Three
  names come from FL's own abort strings (`FLP_ChanType`, `FLP_TargetFXTrack`,
  `FLP_FXTrackCount`).

## The table

| id | kind | census files | name | scope | loader action | payload | confidence | notes |
|---:|------|-------------:|------|-------|---------------|---------|------------|-------|
| 0 | u8 | 165 | ChannelID.IsEnabled | channel | chan+0x32e = (value == 1) via FUN_00f3ad10 | bool | loader+pyflp | Only target is the channel. The 360 surplus occurrences PyFLP noticed are not explained by the loader; see 172 for a likely census artefact. |
| 1 | u8 | 8 | Unknown1 (coined) | ? | no branch | u8 | none | Census artefact, confirmed: with the 0xAC escape decoded the ID vanishes from all 8 files. The byte was the low byte of the extended ID 0xC0000101. |
| 2 | u8 | 1 | ChannelID._VolByte | channel | chan+0x170 = value, x100 when ver<5.3.7 (FUN_00f3ad20) | u8 0..128 | loader+pyflp | Same slot as 219 @4. |
| 3 | u8 | 0 | ChannelID._PanByte | channel | chan+0x16c = value, x100 when ver<5.3.7 (FUN_00f3ad40) | u8 | loader+pyflp | Same slot as 219 @0. |
| 4 | u8 | 1 | Unknown4 (coined) | ? | no branch | u8 | none | Census artefact, confirmed: vanishes once the escape is decoded. |
| 9 | u8 | 161 | ProjectID.LoopActive | project | ui(PTR_DAT_015e2eb8->+0x770) setter +0x200 gets (value != 1) | bool | loader+pyflp | The loader inverts: it passes "value is not 1". Which way the control reads that is not visible. |
| 10 | u8 | 161 | ProjectID.ShowInfo | project | ui(PTR_DAT_015e13b8->+0x7d8) setter +0x200 gets (value == 1) | bool | loader+pyflp | |
| 11 | u8 | 161 | RackID.Swing | project | FUN_01311070(value) | u8 | loader+pyflp | The "Enable Swing?" dialog after load (ver<11.5.0) calls FUN_01311070(0) when declined. |
| 12 | u8 | 0 | ProjectID._Volume | project | global PTR_DAT_015e36d8 = value; x100 when ver<5.3.7 | u8 | loader+pyflp | |
| 13 | u8 | 0 | RackID._FitToSteps | channel | shared handler with 140: ver<4.5.3 value x48; chan vtable+0x130((double)value) | u8 | conflict | PyFLP: rack-level, meaning unknown. Loader: it is the u8 form of 140 and sets the current channel's stretch time (`LAB_00f47778`). |
| 14 | u8 | 0 | ChannelBool28F (coined) | channel | chan+0x28f = (value == 1) | bool | loader | Meaning unknown. |
| 15 | u8 | 0 | ChannelID.Zipped | channel | FUN_00ffef70(chan, 1); value ignored | presence | loader+pyflp | |
| 16 | u8 | 2 | ChannelByte28E (coined) | channel | chan+0x28e = value | u8 | loader | Next to 14's field. Meaning unknown. |
| 17 | u8 | 161 | ArrangementsID.TimeSigNum | project | global PTR_DAT_015e39c0 = value, <1 -> 4; ver<1.4.0 divide by 18's value; sets flag param+0x444f; ver<12.0.0 FUN_00f46360 caps num*den at 64 steps | u8 | loader+pyflp | Whether this is a numerator or beats-per-bar is decided by 35. |
| 18 | u8 | 161 | ArrangementsID.TimeSigBeat | project | global PTR_DAT_015e1c70 = value, <1 -> 4; 1.4.0<=ver<3.6.2: if num % den != 0 then den = 1, num /= den; FUN_013114f0/FUN_01311510 refresh; flag param+0x444e; ver<12.0.0 FUN_00f46360 | u8 | loader+pyflp | |
| 19 | u8 | 0 | comment: ChannelID._19 | channel (sampler) | smp+0x14d = 1; value ignored | presence | loader | smp+0x14d is also 143 bit 3 (PyFLP: "use loop points"), so 19 is that flag's legacy form (inferred). |
| 20 | u8 | 165 | ChannelID.PingPongLoop | channel (sampler) | if smp+0x14c != value or smp+0x14d != 0: smp+0x14c = value; ver<2.2.0 also chan vtable+0x158 and FUN_00ff0db0(chan,1) | u8 | loader+pyflp | Ghidra garbled the condition; in practice the byte is written. |
| 21 | u8 | 165 | ChannelID.Type | channel | value outside 0..5: log "ABORT FLP_ChanType ItemData out of range", error 7, stop; else chan vtable+0x190(value) | u8 enum 0..5 | loader+pyflp | FL's own name is FLP_ChanType. The type is later read back from chan+0x160. Corpus-derived values in crates/model: 0 sampler, 2 plugin, 3 layer, 4 audio clip, 5 automation (inferred from correlation with 201/196 and PyFLP). |
| 22 | i8 | 131 | ChannelID.RoutedTo (FL: FLP_TargetFXTrack) | channel | skipped when global PTR_DAT_015e2308 set; chan+0x258 = value; ver<3.6.5 value+1; outside 0..501: reset to 0, log "WARNING FLP_TargetFXTrack target track out of range", jump to the loop exit | i8 | conflict | PyFLP: -1 means "current insert". Loader rejects anything negative or above 501 after the fixup. Internally 501 is the "current" slot (inferred from the 147 remap), so in this loader "current" is only expressible through 104. Whether the jump to the exit really aborts is unclear (Delphi finally blocks confuse Ghidra). |
| 23 | u8 | 161 | ProjectID.PanLaw | project | ui(PTR_DAT_015e13b8->+0xd68) setter +0x1f0 gets ((value & 2) != 0) | u8, only bit 1 read | loader+pyflp | 0 circular, 2 triangular matches: bit 1 set = triangular. |
| 25 | u8 | 2 | PatternU32At1C (coined) | pattern | pat[c4]+0x1c = value | u8 | loader | Meaning unknown. |
| 26 | u8 | 2 | PatternID.Looped | pattern | patobj(c4)+0x54 = (value > 0) | bool | loader+pyflp | |
| 27 | u8 | 59 | comment: ChannelID.FXProperties | insert | ins[e0]: bit0 -> +0x390, bit1 -> +0x391, bit2 -> +0x13f5, bit3 -> FUN_012c1810(ins, b), bit4 clear -> +0x380 = 1 | u8 bitfield | conflict | PyFLP's comment files it under the channel. The loader applies it to the current insert, and the five bits are the low five bits of 236's flags word. Legacy per-insert flags: 6195 occurrences over 59 old files is about 105 inserts per file. |
| 28 | u8 | 165 | ProjectID.Licensed | project | FUN_00f46420: value == 0 and ver>20.99.0 -> stream+0x11 = 1 (demo-saved flag); value == 0 and the host edition string's first char < 'A' -> error 1 and stop | bool | loader+pyflp | |
| 29 | u8 | 165 | MixerID.APDC | project (mixer) | global PTR_DAT_015e3448 = (value > 0); after the loop, if set and ver<12.9.0 -> FUN_012a3920(-1) | bool | loader+pyflp | |
| 30 | u8 | 152 | PatternsID.PlayTruncatedNotes | project | global PTR_DAT_015e1b18 = (value > 0); ui(13b8->+0xaf8) setter | bool | loader+pyflp | |
| 31 | u8 | 165 | ProjectBool16E8 (coined) | project | global PTR_DAT_015e16e8 = (value > 0). After load: if set and PTR_DAT_015e0120 > 0 and PTR_DAT_015e0ef0 == 1 -> FUN_00e7d8e0(playlist, playlist+0xd2c, {0, -1, 015e0120}) | bool | loader | Meaning unknown; the post-load call looks like a playlist view or selection action (inferred). |
| 32 | u8 | 106 | ChannelID.IsLocked | channel | chan+0x892 = (value & 1) | bool | loader+pyflp | |
| 33 | u8 | 26 | TimeMarkerID.Numerator | time marker (pending) | local_f9 = value; consumed by the next 205 | u8 | loader+pyflp | |
| 34 | u8 | 26 | TimeMarkerID.Denominator | time marker (pending) | local_fa = value; consumed by the next 205 | u8 | loader+pyflp | |
| 35 | u8 | 80 | TimeSigMode (coined) | project | global PTR_DAT_015e37c8 = (value == 1); FUN_01365810(13b8). After load: forced to 1 when clear and 17 == 18 == 4; when set, the time-signature UI gets FUN_01357120(17) and FUN_013570e0(18), otherwise the constants 3 and 1 | bool | loader | This looks like the "time signature vs step division" mode PyFLP could not find for 17/18 (inferred). |
| 36 | u8 | 84 | ArrangementByte10 (coined) | arrangement | arr+0x10 = value | u8 | loader | Meaning unknown. 93 occurrences against 87 arrangements. |
| 37 | u8 | 76 | Unknown37 (coined) | ? | no branch | u8 (1 or 2 per PyFLP's discussion) | none | Whitelisted by FUN_00a9ed80 but matched by no branch in FUN_00f46490: the loader reads and discards it. Deprecated event (inferred). |
| 38 | u8 | 76 | MixerDockPanels (coined) | project (mixer) | FUN_0060a270(PTR_DAT_015e22d8->+0x938, bit0); FUN_0060a270(22d8->+0x930, bit1) | u8 bits 0,1 | loader | FUN_0060a270 flips a shown state at obj+0xa9 and posts message 0xb00b. The post-load code pairs these two objects with inserts whose dock byte (+0x2d) is 0 and 2, so they are the two mixer dock panels (inferred). |
| 39 | u8 | 73 | ProjectBool0568 (coined) | project | global PTR_DAT_015e0568 = (value > 0); after load, ver<20.7.0 forces it to 0 | bool | loader | Meaning unknown. |
| 40 | u8 | 60 | ProjectBool40 (coined) | project | projobj+0x18 = (value > 0); ui(13b8->+0xf00) setter; after load, ver<20.8.0 forces it to 0 | bool | loader | Meaning unknown. |
| 41 | u8 | 53 | PluginColorIsCustom (coined) | plugin | if plug: plug+0xf9 = (value != 0) | bool | loader | 128's handler recomputes the same byte (colour differs from the class default). After load, channels with it clear get the per-type default colour from PTR_DAT_015e4180. |
| 42 | u8 | 53 | InsertColorIsCustom (coined) | insert | ins[e0]+0 = (value != 0) | bool | loader | 149's handler writes the same byte as (colour != 0x716c63). |
| 43 | u8 | 53 | TrackColorIsCustom (coined) | track | arr.trk[c8]+0x70 = (value != 0); when 0, trk+0x2c = default colour PTR_DAT_015e1740 | bool | loader | 238's handler computes the same from colour != 0x565148. |
| 44 | u8 | 50 | ProjectToggle44 (coined) | project | FUN_00e34030(value != 0, 1): global byte DAT_0152c363 and ui(13b8->+0xfd0) setter | bool | loader | Meaning unknown. |
| 45 | u8 | 13 | TimeMarkerFlag (coined) | time marker (pending) | local_fc = value; at 205, kind-12 markers only: marker+0x28 = (value != 0) | u8 | loader | Group with 46, 101, 168: all consumed only by kind-12 markers, which carry a 12-note mask (inferred: key or scale markers). |
| 46 | u8 | 13 | TimeMarkerByte (coined) | time marker (pending) | local_fb = value; at 205, kind 12: marker+0x30 = value | u8 | loader | |
| 47 | u8 | 48 | ProjectBool47 (coined) | project | if globals PTR_DAT_015e2980 and PTR_DAT_015e4fa8 are clear: projobj vtable+8 (value > 0) | bool | loader | Meaning unknown. |
| 48 | u8 | 39 | ChannelBool224 (coined) | channel | chan+0x224 = (value != 0) | bool | loader | Written once per sampler (census count equals 196's). |
| 49 | u8 | 35 | InsertEnum1472 (coined) | insert | ins[e0]+0x1472 = value when 0..2, otherwise untouched | u8 enum 0..2 | loader | |
| 50 | u8 | 34 | ChannelByte1FC (coined) | channel | chan+0x1fc = value | u8 | loader | Pairs with 104 in the census. |
| 51 | u8 | 15 | ChannelFlags51 (coined) | channel | bit0 -> chan+0x738, bit1 -> chan+0x79d, bit2 -> chan+0x79c | u8 bitfield | loader | Pairs with 170. |
| 52 | u8 | 8 | PatternBoolA4 (coined) | pattern | pat[c4]+0xa4 = (value != 0) | bool | loader | Group with 171, 252, 253 (pattern key/scale, inferred from 253). |
| 54 | u8 | 7 | Unknown54 (coined) | ? | no branch | u8 | none | Census artefact, confirmed: it was the LEB128 length byte of the escaped event 0xC0000101, misread by the pre-escape census. No corpus file contains it, and the whitelist (see "ID whitelist") rejects it. |
| 64 | u16 | 165 | ChannelID.New | channel (opens) | project: FUN_011d5600(); create channels (FUN_01004230(-1,0)) until count > value; chan = channels[value]. Channel-state format: the first 64 binds the target channel, later ones create a child. Then: 218 index = 0, 228 index = 0, current plugin = none, plug = chan | u16 index | loader+pyflp | The value is a 0-based index into the channel list and creates channels on demand. |
| 65 | u16 | 135 | PatternID.New | pattern (selects) | c4 = min(value, 999) | u16 index | loader+pyflp | Selects only; nothing is reset. Pattern storage is pat[c4]; the note container is created on demand. The loader uses the value directly as the array index. |
| 66 | u16 | 0 | ProjectID._TempoCoarse | project | if !PTR_DAT_015e0c50: tempo control (2eb8->+0x778) setter +0x1f0 gets (current % 1000 + value*1000) | u16 BPM | loader+pyflp | |
| 67 | u16 | 158 | PatternsID.CurrentlySelected | project | global PTR_DAT_015e4018 = value, outside 1..999 -> 1; applied to ui(2eb8->+0x7a8) after load | u16 | loader+pyflp | |
| 68 | u16 | 0 | StepMask (coined) | channel x pattern | FUN_012d2f90(chan+0x7ac, c4); for each set bit i in 0..15: steps[c4]+0x4800+i = 1 | u16 bitmask | loader | Legacy 16-step on/off grid of the current channel in the current pattern. |
| 69 | u16 | 165 | ChannelID.FreqTilt | channel | chan+0x228 = value | u16 | loader+pyflp | |
| 70 | u16 | 165 | ChannelID.FXFlags | channel | bit0 -> chan+0x25d, bit1 -> +0x25e, bit2 -> +0x25f, bit8 -> +0x25c | u16 bits | loader+pyflp | |
| 71 | u16 | 165 | ChannelID.Cutoff | channel | ver<1.3.61 log remap; ver<2.0.1 value = 1024 - FUN_00f44d30(value, 1024); clamp 16..1024 -> chan+0x234 | u16 | loader+pyflp | |
| 72 | u16 | 4 | ChannelID._VolWord | channel x pattern step | FUN_012d45b0(chan+0x7ac, c4, step, kind, value >> 8) | u16: high byte value, low byte step (inferred) | conflict | PyFLP: legacy channel volume. Loader: a per-step "dot" write like 77/78/87/88/91/92 (step and kind travel in r8/r9, lost). 307 occurrences in 4 files fits per-step data, not per-channel. |
| 73 | u16 | 0 | ChannelID._PanWord | channel x pattern step | same dot write | u16 | conflict | Same disagreement as 72. |
| 74 | u16 | 165 | ChannelID.Preamp | channel | chan+0x23c = value | u16 | loader+pyflp | |
| 75 | u16 | 165 | ChannelID.FadeOut | channel | chan+0x240 = value | u16 | loader+pyflp | |
| 76 | u16 | 165 | ChannelID.FadeIn | channel | chan+0x244 = value | u16 | loader+pyflp | |
| 77 | u16 | 0 | comment: ChannelID._DotNote | channel x pattern step | dot write (value >> 8) | u16 | loader+pyflp | |
| 78 | u16 | 0 | comment: ChannelID._DotPitch | channel x pattern step | dot write (value >> 8) | u16 | loader+pyflp | |
| 79 | u16 | 0 | comment: ChannelID._DotMix | | no branch | | pyflp | |
| 80 | i16 | 161 | ProjectID.Pitch | project | global PTR_DAT_015e2c50 = (i16)value | i16 cents | loader+pyflp | |
| 83 | u16 | 165 | ChannelID.Resonance | channel | ver<1.3.61 exp remap; ver<2.0.1 FUN_00f44d30(value, 1024); ver<3.2.2 FUN_00f44df0; clamp <= 640 -> chan+0x238 | u16 | loader+pyflp | |
| 84 | u16 | 0 | comment: ChannelID._LoopBar | project (playlist) | only when ver<21.0.1: global PTR_DAT_015dfd10 = value (same slot as 152) | u16 | loader+pyflp | After load the slot is multiplied by PTR_DAT_015e0db8 for ver<3.6.0 (bars to ticks, inferred) and applied through FUN_00e543b0 for ver<9.6.0. |
| 85 | u16 | 165 | ChannelID.StereoDelay | channel | chan+0x248 = value | u16 | loader+pyflp | |
| 86 | u16 | 165 | ChannelID.Pogo | channel | chan+0x24c = value | u16 | loader+pyflp | |
| 87 | u16 | 0 | comment: ChannelID._DotReso | channel x pattern step | dot write; v = value >> 8; ver<2.0.1 FUN_00f44d30(v,128); ver<2.2.0 (v>>1)+80; ver<3.2.2 FUN_00f44df0(v) | u16 | loader+pyflp | |
| 88 | u16 | 0 | comment: ChannelID._DotCutOff | channel x pattern step | dot write; ver<2.0.1: type==1 ? 128-v : 128-FUN_00f44d30(v,128); ver<2.2.0 and type==1: 256-v | u16 | loader+pyflp | type is chan+0x160. |
| 89 | u16 | 165 | ChannelID.TimeShift | channel | ver<3.5.2 FUN_0117bc50 remap; chan+0x21c = value | u16 | loader+pyflp | |
| 91 | u16 | 1 | comment: ChannelID._Dot | channel x pattern step | dot write (value >> 8) | u16 | loader+pyflp | |
| 92 | u16 | 1 | comment: ChannelID._DotShift | channel x pattern step | dot write; ver<2.1.0 v = v*48/255 | u16 | loader+pyflp | |
| 93 | u16 | 0 | ProjectID._TempoFine | project | tempo setter gets ((current/1000)*1000 + value) | u16 thousandths | loader+pyflp | |
| 94 | u16 | 7 | ChannelID.Children | channel (layer) | append value to the list at chan+0x264 | u16 iid | loader+pyflp | |
| 95 | i16 | 48 | InsertID.Icon | insert | ins[e0]+0x14 = value | u32 write | loader+pyflp | |
| 96 | u16 | 0 | comment: ChannelID._DotRel | | no branch | | pyflp | |
| 97 | u16 | 106 | ChannelID.Swing | channel | chan+0x220 = value | u16 | loader+pyflp | |
| 98 | u16 | 106 | SlotID.Index | insert slot | slot = value & 0x7f; if slot < 10: ins[e0].slot[slot]+0x14a = ((value & 0x100) != 0) | u16: bits 0-6 slot, bit 8 flag | conflict | PyFLP: a slot delimiter. Loader: writes one flag on that slot and keeps no "current slot"; 212 carries its own insert and slot indices. |
| 99 | u16 | 84 | ArrangementID.New | arrangement (opens) | if value > count-1: FUN_012f9930(0,0) creates one; if the count still does not match: log "FLP : Wrong arrangement index !"; arr = arrangement[value] | u16 index | loader+pyflp | Does not reset c4 or c8. |
| 100 | u16 | 84 | ArrangementsID.Current | project | FUN_012e3920(FUN_012e1a60()); FUN_012fb5f0(value if 0 <= value < count else 0) | u16 index | loader+pyflp | FUN_012fb5f0 logs "Current playlist arrangement: %s". |
| 101 | u16 | 13 | TimeMarkerNoteMask (coined) | time marker (pending) | bits 0..11 -> local_10c[11-i]; at 205, kind 12: copied to marker+0x1c..+0x27 | u16, 12 bits | loader | 12 bits = 12 notes (inferred). |
| 103 | u16 | 34 | FLP_FXTrackCount (FL string) | project (mixer) | value outside 3..502: log "ABORT FLP_FXTrackCount ItemData out of range", error 7, stop; FUN_0127cee0(value) resizes the mixer (clamped 3..502); flag param+0x4447 = 1 | u16 count | loader | The flag switches the insert-index remap in 147, 212 and 235 from the fixed "126 -> 501" rule to "count-1 -> 501" (ver>=24.2.99). |
| 104 | u16 | 34 | FLP_TargetFXTrack16 (coined) | channel | same handler as 22 (`LAB_00f47bae`) | u16 0..501 | loader | The 16-bit form of 22; the two are mutually exclusive per file in the census (131 + 34 = 165). |
| 128 | u32 | 165 | PluginID.Color | plugin | if plug: ver>3.5.3 -> plug vtable+0x78(value), else the class default; then plug+0xf9 = colour-is-custom (not 0x565148 for one class, not in the six-entry default palette for another) | RGBA u32 | loader+pyflp | Skipped when local_202 is set (never set in this build). |
| 129 | u32 | 0 | comment: ArrangementID._PlaylistItem | arrangement (playlist) | item.pos = lo * (17's value) * (18's value) * PTR_DAT_015e25f8 (ver<2.1.0 FUN_00661450 fixup); hi = pattern/track; FUN_01062240 then playlist vtable+8 appends | two u16 | loader+pyflp | |
| 130 | u32 | 0 | comment: ChannelID.Echo | channel | if lo != 0: chan+0x20c = lo; chan+0x200 = (128/(lo+1))*lo; chan+0x210 = hi >> 3 | two u16 | loader+pyflp | Legacy delay: echo count and feedback (inferred). 209 writes the same slots. |
| 131 | u32 | 165 | ChannelID.RingMod | channel | chan+0x22c = lo, +0x230 = hi | two u16 | loader+pyflp | |
| 132 | u32 | 165 | ChannelID.CutGroup | channel | chan+0x27c = lo, +0x280 = hi; channel-state format and lo == hi != 0 -> FUN_00feaaf0(chan,1) | two u16 | loader+pyflp | |
| 133 | u32 | 165 | RackID.WindowHeight | project | no branch | u32 | pyflp | Every FL in the corpus writes it; nothing in the dump reads it. Whitelisted by FUN_00a9ed80 but matched by no branch in FUN_00f46490: the loader reads and discards it. |
| 135 | u32 | 29 | ChannelID.RootNote | channel (sampler) | smp+0x15c = value | u32 | loader+pyflp | |
| 137 | u32 | 0 | comment: ChannelID._MainResoCutOff | | no branch | | pyflp | |
| 138 | u32 | 165 | ChannelID.DelayModXY | channel | chan+0x214 = lo, +0x218 = hi | two u16 | loader+pyflp | |
| 139 | u32 | 165 | ChannelID.Reverb | channel | chan+0x250 = lo (mix), +0x254 = hi (type) | two u16 | loader+pyflp | PyFLP's "+65536 means type B" is hi == 1. |
| 140 | f32 | 0 | ChannelID._StretchTime | channel | shared handler with 13 | u32 integer, cast to double | conflict | PyFLP types it f32 and doubts it ever held stretch time. The loader casts the integer value to double and calls the stretch-time setter, so it did. |
| 141 | u32 | 0 | ChannelU32At290 (coined) | channel | chan+0x290 = value | u32 | loader | Meaning unknown. |
| 142 | i32 | 2 | ChannelID.FineTune | channel (sampler) | smp+0x154 = (double)value | i32 | loader+pyflp | |
| 143 | u32 | 165 | ChannelID.SamplerFlags | channel (sampler) | bit0 -> smp+0xbd, bit8 -> smp+0xf8, bit3 -> smp+0x14d; ver>4.5.4 only: bit1 -> smp+0xf9, bit2 -> smp+0xfa | u32 bits | loader+pyflp | |
| 144 | u32 | 165 | ChannelID.LayerFlags | channel | chan+0x260 = value | u32 | loader+pyflp | |
| 145 | i32 | 165 | ChannelID.GroupNum | channel | project format only: chan+0x798 = value | i32 | loader+pyflp | |
| 146 | i32 | 164 | ProjectID.CurGroupId | project | FUN_0103b750(value, 3) | i32 | loader+pyflp | |
| 147 | i32 | 165 | InsertID.Output | insert (closes) | ver<3.6.3: value >= 0 -> +65, master (e0 == 0) at least 65; ver<5.3.5 and value >= 0: clear ins.sends; value < 65 -> ins.sends[value] = 1 and value = -1, else value -= 65; clamp >= -1 -> ins[e0]+0x370. Then e0 += 1 with remaps: ver<3.6.4 17 -> 49; ver<8.5.0 65 -> 100; ver<12.9.0 104 -> 501; ver<24.2.99 or no 103: 126 -> 501; else count-1 -> 501 | i32 | loader+pyflp | The last insert in file order lands on internal index 501 (the "current" slot, inferred). |
| 148 | u32 | 38 | TimeMarkerID.Position | time marker (pending) | local_e8 = value; at 205: kind = value >> 24 (clamped to 12), position = value & 0xffffff | u32: 24-bit position, 8-bit kind | loader+pyflp | PyFLP's 0x08000000 "signature" flag is kind 8. |
| 149 | u32 | 65 | InsertID.Color | insert | ver<4.0.1 -> default 0x716c63; ver<11.5.9 FUN_00660a00(value, 0x24) adds 36 to each byte, saturating; FUN_012c17e0(ins, colour); ins[e0]+0 = (ins+4 != 0x716c63) | RGBA | loader+pyflp | |
| 150 | u32 | 52 | PatternID.Color | pattern | ver<8.1.0: default marker -> PTR_DAT_015e3730, else FUN_00660a00(value, -112) darkens; FUN_012d14b0(c4, colour) | RGBA | loader+pyflp | |
| 152 | u32 | 1 | ArrangementsID._LoopPos | project (playlist) | global PTR_DAT_015dfd10 = value (same slot as 84) | u32 | conflict | PyFLP: two u16 start/end. Loader: one integer, handed to FUN_00e543b0 after load (ver<9.6.0 and value > 0). Whether that call splits it is not visible here. |
| 153 | u32 | 0 | ChannelID.AUSampleRate | channel | chan+0x6e8 = value | u32 Hz | loader+pyflp | After load, ver<4.5.4 recomputes it from the loaded sample. |
| 154 | i32 | 165 | InsertID.Input | insert | ins[e0]+0x378 = value | i32 | loader+pyflp | |
| 155 | u32 | 165 | PluginID.Icon | plugin | if plug: plug+0x7c = value | u32 | loader+pyflp | |
| 156 | u32 | 157 | ProjectID.Tempo | project | if !PTR_DAT_015e0c50: tempo setter(value) | u32 BPM x 1000 | loader+pyflp | Corpus: present in all 165 projects (the pre-escape census under-counted it at 157). |
| 157 | i32 | 35 | comment: PatternID._157 | pattern | pat[c4]+0x10 = value | i32 | loader | Meaning unknown. |
| 158 | i32 | 33 | comment: PatternID._158 | pattern | pat[c4]+0x18 = value | i32 | loader | Meaning unknown. |
| 159 | u32 | 84 | ProjectID.FLBuild | project | no branch | u32 | pyflp | 199 is parsed instead, and its build component is ignored too. Whitelisted by FUN_00a9ed80 but matched by no branch in FUN_00f46490: the loader reads and discards it. |
| 160 | u32 | 8 | PatternID.ChannelIID | pattern (pending) | local_f8 = value; consumed by 161 | u32 | loader+pyflp | |
| 161 | i32 | 7 | PatternID._161 | pattern x channel | FUN_012cf240(c4, local_f8, ?, ?, 1): find or add a 20-byte record keyed by channel in patobj(c4)+0x58 and set its i32 | i32 | loader+pyflp | The i32 argument is in r8 (lost). PyFLP's "-3 when looped" fits the loader's "nonzero" branch. |
| 162 | u32 | 2 | PatternID._162 | pattern | patobj(c4)+0x50 = value | u32 | loader+pyflp | |
| 164 | u32 | 31 | PatternID.Length | pattern | pat[c4]+0x44 = value | u32 ticks | loader+pyflp | |
| 165 | u32 | 58 | InsertEnum2F (coined) | insert | ins[e0]+0x2f = value when 0..5, else 3 | u32 enum | loader | Before 20.9.0 the same byte was derived from 236 flag bit 13: 1 when set, else 3 or 5 by the global PTR_DAT_015e0b10. |
| 166 | u32 | 58 | InsertEnum30 (coined) | insert | ins[e0]+0x30 = value when 0..3, else 3 | u32 enum | loader | Meaning unknown. |
| 167 | u32 | 45 | ProjectEnum167 (coined) | project | DAT_016bac6c = 1; projobj vtable+0 gets value when 0..2, else 0 | u32 enum 0..2 | loader | Meaning unknown. |
| 168 | u32 | 13 | TimeMarkerU32 (coined) | time marker (pending) | local_100 = value; at 205, kind 12: marker+0x2c = value | u32 | loader | |
| 169 | u32 | 35 | DataFormatFlags (coined) | project | local_298 = value. bit0 clear -> 215 @96 is an int that gets converted to float. Passed to FUN_00f558e0 for 216, 223, 224, 225, 233; for 233 at ver>=20.99.1: bit1 clear -> 60-byte items, bit3 clear -> 80-byte, both set -> the playlist object's native size | u32 bits | loader | Must precede the data events it describes. Item size rule (FUN_00f558e0): before 20.99.1 always 32 bytes; else bit 1 clear = 60, bit 1 set and bit 3 clear = 80, both set = 88. See docs/records-224-233.md. |
| 170 | u32 | 15 | ChannelU32At714 (coined) | channel | chan+0x714 = value | u32 | loader | Pairs with 51. |
| 171 | u32 | 8 | PatternU32AtA8 (coined) | pattern | pat[c4]+0xa8 = value | u32 | loader | Group with 52, 252, 253. |
| 172 | u32 | 8 | ExtendedIdEscape (coined) | stream | FUN_00a9e7a0: an ID byte of 0xAC means "the real event ID is the next u32"; bits 28..31 of that ID pick the payload class (0 u8, 4 u16, 8 u32, 0xC var) | escape + u32 id | loader | The census sizes it as a plain u32 event, so its 8 occurrences, and ID 1's 8 files, are most likely one extended event per file misread (inferred). See "Extended IDs" below. |
| 192 | var | 59 | ChannelID._Name | plugin | same as 203 (`LAB_00f4a96f`) | string | loader+pyflp | Corpus: 13 channels in three FL 11.0.3 projects carry both 192 and 203 with different text, so they are not the same field in that era. |
| 193 | var | 52 | PatternID.Name | pattern | pat[c4]+0 = string | string | loader+pyflp | |
| 194 | var | 161 | ProjectID.Title | project | FUN_0076c080(13b8->+0x7f8, string) | string | loader+pyflp | |
| 195 | var | 105 | ProjectID.Comments | project | FUN_0076c080(13b8->+0x7e0, string) | string | loader+pyflp | Corpus: "Astes - Bien Duro.flp" has unpaired high surrogates (cut-off emoji) in this string, so a decoder must keep raw UTF-16 units to round-trip it. |
| 196 | var | 63 | ChannelID.SamplePath | channel | type 2 (chan+0x160) with class check and vtable+0x140 false: path handed to the channel's plugin (vtable+8, 0x3c) and chan+0x76c cleared; else project format: FUN_00f3b8b0(local_60, path) queues it for loading after the loop; other formats: FUN_00f3b150(path, chan) loads now | string path | loader+pyflp | FUN_00f3b150 owns "Sample not found: " and ".fldownload". |
| 197 | var | 40 | ProjectID.Url | project | FUN_0076c080(13b8->+0x7e8, string) | string | loader+pyflp | |
| 198 | var | 59 | ProjectID._RTFComments | project | len > 0: payload taken as 8-bit text, FUN_00f39000 converts, result goes to the comments field (13b8->+0x7e0) | 8-bit RTF | loader+pyflp | Same destination as 195. |
| 199 | var | 165 | ProjectID.FLVersion | project | ver = FUN_006615f0(string): three dot-separated numbers as major*1e6 + minor*1e3 + patch, build ignored; stream+0x10 (UTF-16 strings from here on) = ver > 11.5.4; project with ver<24.2.99 or no 103: FUN_0127cee0(127) default insert count; host older than ver: "created using a more recent version" dialog, decline -> error 2; project and ver>3.1.1 -> FUN_011d3670(0,0); project and ver<11.5.10 -> clear every insert's send table | 8-bit string | loader+pyflp | Read before the UTF-16 flag is set, so it is always 8-bit. Corpus: 8 presets carry two-part versions such as "12.2" and "21.0"; the patch number counts as 0. |
| 200 | var | 157 | ProjectID.Licensee | project | string read, then FUN_00f46480, which is empty | string | loader+pyflp | Discarded by this build. |
| 201 | var | 164 | PluginID.InternalName | plugin (pending) | local_1c8 = string; used by 212 to instantiate, and by 231's "VFX " check | string | loader+pyflp | |
| 202 | var | 164 | ProjectID.DataPath | project | only ver>=8.0.0: string; if globals 015e4fa8 and 015e2980 clear: PTR_DAT_015e2ba8 = string; FUN_00f45b70 when the folder is missing ("Invalid project data folder" dialog) | string path | loader+pyflp | |
| 203 | var | 121 | PluginID.Name | plugin | if plug and (project or insert-state format, or one of two class checks with a UI flag): plug vtable+0x70(string) | string | loader+pyflp | |
| 204 | var | 69 | InsertID.Name | insert | ins[e0]+0xc = string | string | loader+pyflp | |
| 205 | var | 38 | TimeMarkerID.Name | time marker (closes) | list = (c4 == -1 or ver<11.5.1) ? arr+0x21c48 : pat[c4]+0x30; FUN_010637c0(list, pos, name, kind, 33, 34) appends a 0x34-byte marker (+0 pos, +4 kind, +8 name, +0x10 signature); if kind == 12: marker+0x30 = 46, +0x28 = 45, +0x2c = 168, +0x1c..+0x27 = 101 | string | loader+pyflp | c4 becomes -1 at 233, so markers after the playlist data belong to the arrangement. kind and the pending values 33/34 travel partly in lost registers. |
| 206 | var | 161 | ProjectID.Genre | project | FUN_0076c080(13b8->+0x800, string) | string | loader+pyflp | |
| 207 | var | 162 | ProjectID.Artists | project | FUN_0076c080(13b8->+0x7f0, string) | string | loader+pyflp | |
| 208 | var | 1 | Unknown208 (coined) | ? | no branch | 251 bytes | none | Whitelisted by FUN_00a9ed80 but matched by no branch in FUN_00f46490: the loader reads and discards it. Deprecated event (inferred). |
| 209 | var | 165 | ChannelID.Delay | channel | Move(min(len,20)) -> chan+0x200; ver<3.3.1: @8 = (@8 - 120)*10; ver<5.3.7: @0 and @4 x100; always @4 += 6400 | 20-byte struct | loader+pyflp | @4 pan is stored centred at 6400 in memory, so the file holds -6400..6400 as PyFLP says. |
| 210 | var | 0 | LegacySampleState (coined) | channel (sample object) | if chan+0x358: FUN_00b958e0 gives a buffer and size; Move(min(len, size)) into it; ver<3.2.91: obj+0x48 = 128 - obj+0x48 | raw struct | loader | Not in PyFLP or the census. |
| 212 | var | 164 | PluginID.Wrapper | plugin (opens) | len <= 52 checked (FUN_00f463e0); Move(52) into a stack struct; see "Row details: 212" | 52-byte struct | loader+pyflp | Loader reads @16 as a u32 flags word, which resolves PyFLP's bit 16/17 puzzle. |
| 213 | var | 153 | PluginID.Data | plugin | only if a plugin exists and it is not in the deferred list: kind 0 (212 @8): len/4 values set one by one through vtable+0x30 (ints, or floats scaled by 0x10000 or 0x40000000 when FUN_00aa3b10 says so); kind 1 or 2: FUN_00aa38d0 reads a chunk from a sub-stream over the payload (stream+0x100054 marks nesting), with "fruity wrapper" special-cased by name; afterwards FUN_012b7270, Patcher check, deferred-list bookkeeping | plugin-specific | loader+pyflp | The payload layout per plugin lives inside each plugin's own loader, not here. PyFLP's per-plugin structs are not contradicted. |
| 215 | var | 165 | ChannelID.Parameters | channel | Move(min(len,168)) -> chan+0x284, then version fixups; see "Row details: 215" | struct <= 168 | loader+pyflp | @96 is a float when 169 bit0 is set, else an int converted in place. |
| 216 | var | 135 | GlobalControllerEvents (coined) | project | FUN_00f558e0(PTR_DAT_015dfc48, substream, mode 0, ver, 169): 12-byte records (fixed below 20.99.1, the object's own size from there) loaded through the target's vtable+0x38. After load, ver<2.8.1: the list is moved into a freshly created pattern and cleared | 12-byte records | loader | Same record format as 223 (inferred). What the global list is for in modern files is unknown. |
| 217 | var | 1 | ArrangementsID.PLSelection | project (playlist) | ver>3.6.0: Move(8) -> start, end; FUN_00e43d30(PTR_DAT_015e35c8, start, end, 1) | two u32 | loader+pyflp | |
| 218 | var | 165 | ChannelID.EnvelopeLFO | channel (sampler) | Move(min(len,68)) -> smp+0x198 + idx*0xa0; len < 68: byte0 bit3 -> @0x38 = -101, bit6 -> @0x3c = -101, byte0 &= ~DAT_00f518a8; idx += 1 | 68-byte struct | loader+pyflp | Index resets at 64, so the five envelopes are positional. Stride in memory is 0xa0, not 68. |
| 219 | var | 165 | ChannelID.Levels | channel | Move -> chan+0x16c | 24-byte struct | loader+pyflp | The copy length is in a lost register; 2, 3 and 220 write into the same 24 bytes. Corpus: writers 3.0.0 to 3.2.61 (98 presets) store 12 bytes here, pan/volume/u32 at 0/4/8, with the rest of the struct in 220. |
| 220 | var | 0 | comment: ChannelID._Filter | channel | n = ver<2.5.1 ? 8 : len; Move -> chan+0x178; ver<3.2.2: chan+0x17c = (chan+0x17c << 8) / 80 | two u32 | loader+pyflp | Legacy filter cutoff/resonance pair, landing in 219's @12/@16 (mod X, mod Y). PyFLP only has the name. Corpus: the same 98 old presets store 12 bytes here (three u32), the second half of 219. |
| 221 | var | 165 | ChannelID.Polyphony | channel | Move(min(len,9)) -> chan+0x1ec; ver<3.5.2: @4 = FUN_0117bc50(@4) | 9-byte struct | loader+pyflp | |
| 223 | var | 12 | PatternID.Controllers | pattern | target = automation-state format ? the target object : FUN_012d1660(c4,1); FUN_00f558e0(target, substream, mode 0, ver, 169); FUN_00f463b0 | 12-byte records | loader+pyflp | |
| 224 | var | 134 | PatternID.Notes | pattern | target = score format ? the target : patobj(c4); FUN_00f558e0(mode 1): 24-byte records (ver<8.1.0 legacy converter FUN_00f553b0); then per note: ver<3.1.6 @16 = 120; ver<5.3.0 @19 = (@19 & 0xf) << 2 kept in the low nibble; ver<6.5.0 @18 = 64 | 24-byte records | loader+pyflp | Layout resolved in `docs/records-224-233.md`: position i32 @0, tag u16 @4 (bit 3 slide, 0xFFFFFFFF = deleted), channel u16 @6, length i32 @8, key i16 @12, group u16 @14, fine pitch @16 (120 centre, 10 cents/step), repeats @17, release @18, flags @19 (bits 0-3 colour, 4 porta, 5 muted, 7 selected), pan @20, velocity @21, mod X/Y @22/@23. |
| 225 | var | 165 | MixerID.Params | project (initial control values) | FUN_00f558e0(local_68, substream, mode 0, ver, 169): 12-byte records. After the loop every record is replayed as FUN_01046a70(rec@4, rec@8, 0x10011); the insert-state format first rewrites rec@4 to point at the target insert | 12-byte {u32, u32 REC id, i32 value} | conflict | PyFLP: mixer parameters with its own id/target encoding. Loader: generic REC-event initial values dispatched by control ID; mixer targets are one range of that ID space. The encoding PyFLP documents (insert in bits 6..12, slot in 0..5 of @6) is a view of the REC id at @4. Resolved in `docs/rec-225.md`: the REC id is `0x70000000 + (insert << 22) + (slot << 16) + control` (base 0x20000000 and current-insert 126 in files before 24.2.99, remapped by FUN_00f45460). Per-insert controls: 0x1FC0 volume (0..16000, 12800 = 0 dB), 0x1FC1 pan (-6400..6400), 0x1FC2 stereo separation, 0x1FD0/0x1FD8/0x1FE0 + band EQ gain/freq/width, 0x1F00/0x1F01 + slot enable/mix, 0x2000 + dest send level. One global 0x40000000 main volume. No channel-scope records in the corpus. "Insert in bits 6..12" holds only for pre-24.2.99 files. |
| 226 | var | 162 | ControllerID.MIDI | controller link | obj = FUN_00f82530(0); copy payload -> obj+8; ver<4.6.1: @16 *= -4; obj+0x14 or-assign 4; append to list PTR_DAT_015e0a00[0] | 20 bytes | loader+pyflp | The main loop handles 226 and 227 in one range compare (`iVar3 - 0xe2U < 2`). |
| 227 | var | 62 | ControllerID.Remote | controller link | as 226 with obj+0x14 or-assign 8 and list PTR_DAT_015e0a00[1] | 20 bytes | loader+pyflp | Automation binding: 20 bytes {u32 source REC id = automation channel << 16, u32, u32 target REC id in the rec-225 space, u32 flags (8 remote, 4 MIDI), i32}. See docs/automation-234.md. |
| 228 | var | 165 | ChannelID.Tracking | channel | Move -> chan+0x1ac + idx*0x20; idx += 1 | 16-byte struct | loader+pyflp | Index resets at 64. |
| 229 | var | 165 | ChannelID.LevelAdjusts | channel | Move -> chan+0x184; ver<5.3.7: @0 and @4 x100 | 20-byte struct | loader+pyflp | |
| 230 | var | 25 | ControllerFormula (coined) | controller link | string -> FUN_00f81f70 compiles it as a formula (variables "Input" and "Tension", log "Compiled ok."), reusing an identical compiled formula if one exists; result stored at the most recent 226/227 object +0 | string | loader | The mapping formula of a remote-control link. |
| 231 | var | 164 | DisplayGroupID.Name | display group | "Audio clips" and "Automation clips" are swapped for localised names; FUN_0103ba50(idx, string); idx += 1 | string | loader+pyflp | |
| 233 | var | 112 | ArrangementID.Playlist | arrangement | c4 = -1; FUN_00f558e0(arr+0x14, substream, mode 2, ver, 169): 32-byte items below 20.99.1 (ver<8.1.0 converter FUN_00f55510), else 60, 80 or the object's native size by 169; FUN_00f3c7b0 validation, failure -> error 5 and stop; FUN_01060a30(playlist) | 32/60/80+ byte items | loader+pyflp | PyFLP's 60-byte FL 21 item matches the "169 bit1 clear" case. Layout resolved in `docs/records-224-233.md`: 32, 60, 80 or 88 bytes by writer version and 169 flags. Position @0, source u32 @4 (>= 0x50000000 pattern), length @8, track @12 as 500 - track (199 - track before 12.9.1), group @14, flags byte @19 (bit 5 muted, bit 7 selected), trims @24/@28 (i32 ticks for patterns, f32 milliseconds for audio clips, -1 none), id @32, gain f32 @52, pitch f32 @60, stretch f64 @64, flags u32 @72 (bit 0 reverse), pan f32 @80. |
| 234 | var | 49 | ChannelID.Automation | channel (automation clip) | if chan+0x360: substream over the payload -> clip vtable+0x10 | raw, handed to the clip object | loader+pyflp | The point layout lives in the clip class, not here. Layout resolved in `docs/automation-234.md`: 9-byte articulator header, an envelope block (48 + 24n bytes) and an LFO block (52 + 24m bytes); points are {f64 x beats from previous point, f64 y 0..1, f32 tension, u8 mode 0..12, u8, u8, i8}. Total 109 + 24(n+m) fits all 1,105 corpus clips; the "112-byte trailer" was the LFO block with m = 1. The 62 automation clip presets include block versions 1 and 2, which are shorter. Presets: 62 `.fst` files carry 234 with version 1 (16-byte points, 56 files) or version 2 (2 files); projects are all version 3. Automation clip item trims are in beats. |
| 235 | var | 165 | InsertID.Routing | insert | n = min(len, 501); for i < n: idx = i; ver<8.5.0 and i > 64: +35; ver<12.9.0 and idx > 103: +397; else (ver<24.2.99 or no 103) and idx > 125: +375; if idx < 502: ins[e0].sends[idx] (+0x398 + idx*8) = (byte != 0) | u8 array | loader+pyflp | Engine: byte i is the send to insert i, applied on top of the default send to master; see docs/rec-225.md. |
| 236 | var | 165 | InsertID.Flags | insert | len 5..12 checked; see "Row details: 236" | 12-byte {u32, u32 flags, u32} (5 or 8 in old files) | loader+pyflp | Bits 0..4, 6..7, 10..15 consumed; bits 5, 8, 9 not read here. Engine: bit 3 is "insert disabled" (FUN_012c1810); in files with a 5-byte 236 the same bit sits in event 27. |
| 237 | var | 164 | ProjectID.Timestamp | project | len <= 16 checked; Move -> PTR_DAT_015dfdc0; then @8 = now - @8 | two f64 | loader+pyflp | Storing now minus time-spent lets the save path write time-spent as now minus the stored value. |
| 238 | var | 165 | TrackID.Data | track (opens) | len 13..70 checked; see "Row details: 238" | struct <= 70 | loader+pyflp | Extends PyFLP's layout past @48. |
| 239 | var | 60 | TrackID.Name | track | FUN_012e60f0(arr, c8, string, trk+0x2c) | string | loader+pyflp | |
| 240 | var | 3 | ProjectStringList (coined) | project | grow the global string list PTR_DAT_015e18f8 by one; last = string | string | loader | Purpose unknown. |
| 241 | var | 84 | ArrangementID.Name | arrangement | FUN_012e3be0(arr, string) | string | loader+pyflp | |
| 242 | var | 48 | ArrangementViewState (coined) | arrangement (by index) | len <= 28 checked; a = arrangement[@0 u32] if it exists; a+0x21c5c = @4 u64; a+0x21c64 = (f32)(@12 f64) * zoom scale (PTR_DAT_015e35c8->...+0xc); a+0x21c6c = @20 u32; a+0x21c70 = round(@24 i32 * scale) | 28-byte struct | loader | Scroll and zoom of the playlist view per arrangement (inferred from the DPI-style scaling). |
| 243 | var | 48 | ProjectPairU32 (coined) | project | len <= 8 checked; PTR_DAT_015e3a90 = @0; PTR_DAT_015e3da8 = @4 | two u32 | loader | Meaning unknown. |
| 245 | var | 0 | ChannelString718 (coined) | channel | chan+0x718 = string | string | loader | |
| 246 | var | 0 | ChannelString728 (coined) | channel | chan+0x728 = string | string | loader | |
| 247 | var | 0 | PatternString94 (coined) | pattern | pat[c4]+0x94 = string | string | loader | |
| 248 | var | 0 | ChannelString720 (coined) | channel | chan+0x720 = string | string | loader | |
| 249 | var | 0 | ChannelString75C (coined) | channel | chan+0x75c = string | string | loader | |
| 250 | var | 0 | PatternString9C (coined) | pattern | pat[c4]+0x9c = string | string | loader | |
| 251 | var | 2 | ChannelSubState (coined) | channel | substream -> FUN_01006f00(chan+0x784, substream, 0): u32 record version; 1: 56 bytes to obj+0x4c and two fields zeroed; other: FUN_01006e10 defaults; then u8 -> smp+0x14d, u8 -> obj+0x10; obj+0x48 = 1 | 43 bytes | loader | The write side emits version 2, so the 43-byte census size is the version-2 shape; its middle part is not readable from this dump. |
| 252 | var | 8 | PatternStringAC (coined) | pattern | pat[c4]+0xac = string | string (empty in the corpus) | loader | |
| 253 | var | 8 | PatternNoteMaskText (coined) | pattern | FUN_012d22d0: split the string on a separator into up to 12 tokens; pat[c4]+0xb4+i = (token == "1") | text list of 12 "0"/"1" | loader | 48 bytes = 23 UTF-16 chars + NUL, which fits twelve one-char tokens with eleven separators (inferred). |
| 254 | var | 0 | ChannelString73C (coined) | channel | chan+0x73c = string | string | loader | |
| 255 | var | 0 | ChannelString744 (coined) | channel | chan+0x744 = string | string | loader | |

### Extended IDs (above 255, reached through the 172 escape)

| id | class | name | scope | loader action | confidence |
|----|-------|------|-------|---------------|------------|
| 0x100 | u8 | ProjectBool256 (coined) | project | projobj vtable+0x10 gets (value != 0) | loader |
| 0xC0000100 | var | ProjectString100 (coined) | project | string -> projobj vtable+0x130 | loader |
| 0xC0000101 | var | WriterName (coined) | project | no branch: the loop compares 0xC0000100 (line 601) and 0xC0000102 (line 606) and skips this one. The corpus writes it in 8 files from FL 25.2.3 through 26.1.0 as the UTF-16 string "FL Studio <version>", so FL writes a human-readable writer tag that its own loader ignores (inferred from the payload). | loader |
| 0xC0000102 | var | ChannelIntList102 (coined) | channel | FUN_0091fca0(chan+0x764) clears a list; the string is split into items; each item that parses as an int is added with FUN_0091fb90(chan+0x764, n) | loader |

### Row details

**212 (wrapper).** Struct fields as the loader uses them: @0 i32 insert
index, @4 i32 slot index, @8 i32 plugin kind, @12 i32 (only used with @8 as a
u64 for ver<2.5.2), @16 u32 flags (len < 21 -> flags = 1), @20 u8 (stored at
plugobj+0x38; PyFLP's "page"), @36 u64 and @44 u64 editor rectangle (position
and size, inferred). Flags consumed: bit4 generator (plugin belongs to the
current channel) versus effect (plugin is `ins[remap(@0)].slot[@4]`, where the
remap is: ver<3.6.4 and @0 > 16 -> +48, ver<8.5.0 and > 64 -> +35, ver<12.9.0
and == 104 -> 501, ver<24.2.99 or no 103 and == 126 -> 501; the insert-state
format uses e0 instead); bit5 -> vtable+0x88; bit6 -> vtable+0xf8 (forced on
for ver<6.1.6); bit7 -> when set and the plugin reports 0x8000 in
FUN_01330870, the plugin goes to a deferred list (local_70) finished by
FUN_00c31890 after the loop; bits 0, 2, 3, 8, 9 (mask 0x30d) -> plugobj+0x34.
For ver<2.5.2, bit16 stands in for @8 and bit17 says @36 is present. @8 picks
how 213 is read (0 parameter array, 1 or 2 chunk) and is the "type:" shown in
the missing-plugin message. The editor rectangle is DPI-scaled when ver<11.1.2
or ver>12.3.0 (copied as read; the gap is odd). The plugin is instantiated by
201's name through vtable+0x28; a 0x32 result sets error 6, any other failure
shows FUN_012b88d0's "Missing plugin" dialog. A generator/effect mismatch with
the preset format destroys the plugin and records the offending format code
(0x31 or 0x32) in DAT_016bac68. After all that `plug` (local_158) is the new
plugin object, so 128, 155, 192/203 and 213 address it.

**215 (channel parameters).** Base chan+0x284. ver<2.2.0 and @4 != 0xff:
legacy sample fields set on smp (+0x23c = -1, +0x244 = 0, +0x248 = 0, +0x250
= 128, +0x254 from a conversion of @4). ver<3.3.4: @61, @62, @63 = 0 and @60
reduced to 0/1. ver<3.5.2: @52 and @64 both go through FUN_0117bc50 and are
clamped to 450..1447 (PyFLP calls @52 "arp time, f32"; the loader treats it as
an int in the same unit as @64's gate time, for old files only). ver<3.5.2 with
a legacy sample object at chan+0x358: @62 = 1, @64 from the object, and 219's
mod X/Y doubled from the object. ver<5.3.7: chan+0x170 and chan+0x16c (volume,
pan) x100. ver<6.1.5: @108 stretch mode: 1..2 -> +2, >=3 -> +4. ver<6.4.1:
@4..@7 zeroed. ver<11.5.13: @108 = table PTR_DAT_015e37a8[clamp(@108, 0, 9)],
then 5 -> 6. Always: if 169 bit0 is clear, @96 (int) is converted to float in
place.

**236 (insert flags).** len == 5: `u32 @0 ^ DAT_00f50e60` goes to ins+0x1c8
when byte @4 is 0, else to ins+0x1cc. Otherwise flags = @4: bit2 ->
ins+0x13f5 always; the rest is skipped for the master (e0 == 501) in
insert-state format. ver<12.9.0: `@0 ^ DAT_00f50e60` -> ins+0x1c8 (bit5 clear)
or ins+0x1cc (bit5 set); ver>=12.9.0: ins+0x1cc = @0, ins+0x1c8 = @8. bit0 ->
+0x390, bit1 -> +0x391, bit3 -> FUN_012c1810(ins, b), bit4 clear -> +0x380 = 1
(same as 27). bits 6..7 -> +0x2d dock position (unless insert-state format).
bit10 -> +0x2c, bit11 -> +0x1b, bit12 -> +0x1a, bit13 -> +0x2e (and for
ver<20.9.0 derives +0x2f, see 165), bit14 clear -> +0x1d1 = 1, bit15 clear ->
+0x1d0 = 1. What the two XOR-masked u32s at @0 and @8 are is not readable
here.

**238 (track).** c8 = @0 u32 (track index). @4 colour: `(@4 & 0xffffff) !=
0x565148` sets trk+0x70 (custom colour); default colour replaces it;
FUN_012e60f0(arr, c8, 0, colour). trk+0x30 = @8 icon. trk+0x3c = @12 u8. len >
16: trk+0x34 = @13 f32, trk+0x38 = @17 i32. len > 21: trk+0x3d = @21 u8. len >
22: 24 bytes from @22 copied to trk+0x40 (the six performance fields PyFLP
lists at @22..@45). len > 46: trk+0x3e = @46 u8. len > 48: trk+0xd5 = @47 u8,
trk+0xd4 = @48 u8. len > 56: trk+0xd8 = @49 u32, trk+0xdc = @53 u32. len > 60:
trk+0xe0 = @57 u32. len > 61: trk+0xe4 = @61 u8. len > 69: trk+0xf0 = (@66 u32
& 1). PyFLP's @46 "grouped" and @47 "locked" match the loader's one-byte reads;
everything from @48 on is new here and unnamed.

## Object delimiters

Read from where the loop switches its "current" pointers.

- **Channel: 64.** `chan` becomes `channels[value]`, creating channels until
  the index exists (FUN_01004230(-1, 0) per missing one). Resets: the 218
  envelope counter and the 228 tracking counter to 0, the current plugin
  (local_50) to none, and the plugin host (local_158) to the channel. Nothing
  else resets, so a channel that lacks an event keeps the allocation default,
  not the previous channel's value. In channel-state presets (format 0x20) the
  first 64 binds the target channel and later ones create children.
- **Pattern: 65.** Sets only `c4 = min(value, 999)`. The pattern record
  `pat[c4]` and the note container are addressed by index, so the two 65 blocks
  per pattern that PyFLP describes simply select the same record twice. 233
  sets `c4 = -1`, which redirects 205 markers to the arrangement.
- **Insert: 147 closes one.** There is no opening event. 27, 42, 49, 95, 149,
  154, 165, 166, 204, 235, 236 and 98's slot flag all address `ins[e0]`, and
  147 writes the output and then advances `e0` with the version remaps listed
  in its row. `e0` starts at 0 (or at the target insert for the insert-state
  format). The last insert lands on internal index 501.
- **Slot plugin: 212 opens one.** The wrapper struct names the insert (@0) and
  slot (@4) itself; 98 does not set a current slot. After 212, `plug` is the
  slot plugin, so 128, 155, 192, 203 and 213 go to it until the next 64 or 212.
  201 must come before 212 (its string is the plugin name used to instantiate).
- **Arrangement: 99.** `arr = arrangement[value]`, creating one when value ==
  count. Does not reset `c4` or `c8`. 100 selects the active arrangement and
  is not a terminator in the loader (it can appear anywhere).
- **Track: 238.** `c8 = @0` of the struct; 239 and 43 then address
  `arr.trk[c8]`.
- **Automation clip: none.** 234 is delivered to the current channel's clip
  object (chan+0x360); the channel is the delimiter.
- **Controller link: 226 or 227** each create one object; 230 attaches to the
  most recent.
- **Time marker: 205 closes one.** 33, 34, 45, 46, 101, 148, 168 are parked in
  locals until the next 205 consumes them. The locals are not cleared after
  use, so a marker that omits one inherits the previous marker's value.
- **Pattern x channel record: 160 then 161.**
- **Display group: 231**, positional (an index counter increments per event).

## Version gates

`ver` is `major*1e6 + minor*1e3 + patch` from 199's string; the build number
is dropped. Every comparison in the loop, by version. "Pass" means a
post-load fixup callback handed to FUN_012cb6c0 or FUN_012dfa70 whose body is
not in the dump.

| below version | constant | what changes |
|---|---|---|
| 1.3.61 | 0xf4e35 | 71 and 83 go through log/exp curve remaps before clamping. |
| 1.4.0 | 0xf51e0 | 17: numerator divided by 18's value. |
| 1.4.0..3.6.2 | 0xf51df, 0x2dde32 | 18: if num % den != 0 then den = 1; num /= den. |
| 2.0.1 | 0x1e8481 | 71, 83, 87, 88: FUN_00f44d30 curve remap (88 also inverted). |
| 2.1.0 | 0x1e8868 | 92 scaled x48/255; 129 position fixup; after load PPQ forced to 64, each channel's +0x21c x48/256 and +0x210 x3. |
| 2.2.0 | 0x1e8c50 | 20 forces a reload; 87 (v>>1)+80; 88 inverted for type 1; 215 legacy sample fields from @4. |
| 2.5.1 | 0x1e9809 | 220 copy length fixed at 8. |
| 2.5.2 | 0x1e980a | 212: flags bit16 and bit17 stand in for @8 and the editor rectangle. |
| 2.8.1 | 0x1ea3c1 | After load, 216's global list is moved into a new pattern. |
| 3.1.1 | 0x2dcaa9 | 199 (project): FUN_011d3670(0,0) only for newer files. |
| 3.1.6 | 0x2dcaae | 224 note @16 = 120. |
| 3.2.2 | 0x2dce92 | 83 and 87 FUN_00f44df0 remap; 220 Y rescaled x256/80; pass FUN_00f45610. |
| 3.2.91 | 0x2dceeb | 210: obj+0x48 = 128 - obj+0x48. |
| 3.3.1 | 0x2dd279 | 209 @8 = (@8-120)*10; pass FUN_00f45690. |
| 3.3.4 | 0x2dd27c | 215 @60..@63 normalised. |
| 3.5.1 | 0x2dda49 | After load, each channel's vtable+0x1a8 is called (also in channel-state loads). |
| 3.5.2 | 0x2dda4a | 89, 221 @4, 215 @52/@64 remapped with FUN_0117bc50; 215 legacy sample object; pass FUN_00f456c0. |
| 3.5.3 | 0x2dda4b | 128: colour ignored, class default used instead. |
| 3.6.0 | 0x2dde30 | 217 ignored; after load 84/152's slot x PTR_DAT_015e0db8 and FUN_012d2e30(7). |
| 3.6.3 | 0x2dde33 | 147: output +65, master at least 65. |
| 3.6.4 | 0x2dde34 | Insert index 17 -> 49 (147 advance, 212 @0 > 16 -> +48); pass FUN_00f45790. |
| 3.6.5 | 0x2dde35 | 22/104 value +1; pass FUN_00f457c0. |
| 4.0.1 | 0x3d0901 | 149 colour replaced by default 0x716c63. |
| 4.5.3 | 0x3d1c8b | 13/140 value x48. |
| 4.5.4 | 0x3d1c8c | 143 bits 1 and 2 ignored; after load playlist item start/end offsets recomputed from the sample and 153 recomputed. |
| 4.6.1 | 0x3d2071 | 226/227 @16 x(-4). |
| 5.3.0 | 0x4c56f8 | 224 @19 low nibble x4 (and the 3.1.6 fixup runs inside this gate). |
| 5.3.5 | 0x4c56fd | 147: output index < 65 becomes a send flag; pass FUN_00f457e0 (also for insert-state loads). |
| 5.3.7 | 0x4c56ff | 2, 3, 12, 209 @0/@4, 229 @0/@4, and 215's volume/pan x100; pass FUN_00f45840. |
| 6.1.5 | 0x5b916d | 215 @108 stretch mode shifted (+2 or +4). |
| 6.1.6 | 0x5b916e | 212 flag bit6 forced on. |
| 6.4.1 | 0x5b9d21 | 215 @4..@7 zeroed. |
| 6.5.0 | 0x5ba108 | 224 note @18 = 64. |
| 8.0.0 | 7999999 | 202 not read at all. |
| 8.1.0 | 0x7a15e8 | 150 colour darkened; FUN_00f558e0 runs legacy record converters for 224 and 233. |
| 8.5.0 | 0x7a2588 | Insert index 65 -> 100 (147), > 64 -> +35 (212, 235); pass FUN_00f458e0. |
| 9.6.0 | 0x896bb0 | After load, 84/152's slot applied through FUN_00e543b0. |
| 10.1.0 | 0x989a68 | Pass FUN_00f45960; "Tempo (coarse)" automation rescaled. |
| 11.1.2, and above 12.3.0 | 0xa7dcaa, 0xb726b8 | 212 editor rectangle DPI-scaled. |
| 11.5.0 | 0xa7ec48 | After load, swing migration per channel (FUN_010012e0 x999) and the "Enable Swing?" dialog. |
| 11.5.1 | 0xa7ec49 | 205 markers always go to the arrangement. |
| 11.5.4 | 0xa7ec4c | Strings are 8-bit; above it UTF-16LE (stream+0x10). |
| 11.5.6 | 0xa7ec4e | Pass FUN_00f45980. |
| 11.5.9 | 0xa7ec51 | 149 colour brightened by 36 per byte. |
| 11.5.10 | 0xa7ec52 | 199 (project): every insert's send table cleared. |
| 11.5.12 | 0xa7ec54 | 213: FUN_0132fa40 for one plugin class; after load the same for every slot. |
| 11.5.13 | 0xa7ec55 | 215 @108 remapped through PTR_DAT_015e37a8. |
| 12.0.0 | 12000000 | 17/18: FUN_00f46360 caps num*den at 64. |
| 12.9.0 | 0xb73e28 | Insert index 104 -> 501 (147, 212, 235 via +397); 236 @0 XOR-masked single field; 29 triggers FUN_012a3920(-1); pass FUN_00f459e0. |
| 12.9.1 | 0xb73e29 | Pass FUN_00f45a40. |
| 12.9.4 | 0xb73e2c | After load chan+0x320 = 0 for every channel. |
| 20.2.0 | 0x13134d0 | After load chan+0x321 = 0 for every channel. |
| 20.7.0 | 0x1314858 | 39 forced to 0 after load. |
| 20.8.0 | 0x1314c40 | 40 forced to 0 after load. |
| 20.9.0 | 0x1315028 | 236 bit13 derives ins+0x2f (165's field). |
| 20.99.0 | 0x132afb8 | 28 == 0 sets the demo-saved flag only above this. |
| 20.99.1 | 0x132afb9 | FUN_00f558e0 record sizes fixed at 12/24/32; from here they come from the target object and 169. |
| 21.0.1 | 0x1406f41 | 84 read; from 21.0.1 ignored. |
| 24.2.99 (with 103 present) | 0x16e3e33 | Insert index 126 -> 501 (147, 212, 235 via +375) and 199 sets a default insert count of 127. From 24.2.99 with 103: the remap is count-1 -> 501. |

The loader also checks the host version against `ver` at 199 and warns when
the file is newer.

## Still unknown

Neither source gives a readable meaning. What the loader shows:

- 1 and 4: no branch. 1 is probably the escape artefact described at 172.
- 37, 54, 133, 159, 208: no branch in the loop, the caller
  (`decompiled/FUN_00f51fd0`) only parses `FLhd`/`FLdt`, and the per-event
  filter's two callees (FUN_00a9ed80, FUN_00a9ee60) were never decompiled.
  Treat them as read-and-discarded by FL 26.1.7.
- 14, 16, 25, 36, 39, 40, 44, 47, 48, 49, 50, 51, 141, 157, 158, 165, 166, 167,
  170, 171, 240, 243, 245..250, 252, 254, 255: a plain store into a named
  object field, nothing else. The field offset is in the table so a later
  pass with a debugger or more decompiled callers can name them.
- 31: a project bool that gates a post-load call into the playlist.
- 236 @0 and @8: two u32s, XOR-masked with DAT_00f50e60 in files before 12.9.0.
- 212 @12, @20 and the two u64s: stored, not interpreted here.
- 238 @48 onward: stored into the track record at +0xd4..+0xf0, unnamed.
- 251: the version-2 body between the u32 header and the two trailing bytes.
- 216: the record format is 223's, but what FL does with a project-level
  controller event list in modern files is not visible.
- 79, 96, 137: PyFLP comment names with no branch anywhere. FL 26 does not
  read them.
- The i8/u8 question for 22: whether the fixed-size decoders (FUN_00a9e430 and
  siblings, not decompiled) sign-extend. Many branches test `value < 0`, so
  they probably do (inferred).

## ID whitelist

`FUN_00a9ed40` runs before every dispatch. It calls `FUN_00a9ed80`, which accepts only these IDs: 0..52, 64..104, 128..172, 192..255, 0x100, and 0xC0000100..0xC0000102. Any other ID makes the project loop break (`FUN_00f46490` line 553, error state via `local_245`). Its second callee `FUN_00a9ee60` counts consecutive ID-0 events and stops the loop after 1000 of them. So the upper bound of each class is the highest ID FL 26.1.7 knows, and a writer must never emit anything outside those ranges.

## Reading the loader

How to verify a row in `FUN_00f46490_00f46490.c`.

- The loader state is `param_1`. The stream object is `*(param_1 + 0x4450)`.
  `param_1+0x4488` is the format being loaded (0 project, 0x10 score, 0x18
  automation state, 0x20 channel state, 0x30..0x32 plugin states, 0x40 insert
  state), `param_1+0x4490` the target object for preset formats,
  `param_1+0x4448` the error code (1 demo refused, 2 bad header or newer file
  declined, 4 ID filter rejected, 5 cancelled or invalid playlist or format
  mismatch, 6 plugin failed, 7 range abort), `param_1+0x4447` "103 seen",
  `param_1+0x444e/+0x444f` "18/17 seen".
- Stream fields: `+0x100020` the event ID (u32, after escape expansion);
  `+0x10002c` the decoded value for u8/u16/u32 events, or the payload pointer
  for var events; `+0x100024` the var payload length; `+0x10` strings are
  UTF-16; `+0x11` demo-saved; `+0x100054` nesting flag around a 213 sub-read;
  `+0x100018` a buffer freed per event.
- `FUN_00a9e7a0(stream, &pos)` advances to the next event. It reads one ID
  byte; 0xAC means a u32 ID follows. The size class is the ID's range for
  bytes, or bits 28..31 for extended IDs. The four class readers
  (FUN_00a9e430, FUN_00a9e4d0, FUN_00a9e570, FUN_00a9e610) are not in the dump.
- `FUN_00a9ed40(id, &out)` is called before dispatch; false stops the loop
  with error 4. Its callees are not decompiled.
- `FUN_00a9e8e0(stream, &str)` reads a var payload as a string, 8-bit or
  UTF-16 by `stream+0x10`.
- `FUN_00409c80(src, dst)` and the function pointer `PTR_DAT_015e0030` are
  byte copies; the length argument is in a lost register. The loop computes
  the clamped length into a local right before each call, which is how the
  table's `Move(min(len, n))` was read.
- `FUN_00f463e0(ctx, max, min)` checks `min <= len <= max` and returns true
  when fine; it is used for 212, 236, 237, 238, 242, 243.
- `FUN_0081d700(class, 1, ptr, len)` wraps a payload in a memory stream;
  `FUN_00f558e0(target, stream, mode, ver, flags169)` loads fixed-size records
  from it (mode 0 = 12-byte controller events, 1 = 24-byte notes, 2 = playlist
  items), calling `target->vtable+0x38(buf, byteLen, recordSize)`.
- `DAT_016bac50` is `ver`. `DAT_016bac68` is the header's format byte (and is
  overwritten with 0x31/0x32/0x50 on plugin-preset mismatches). The version
  parser is `FUN_006615f0`.
- Current-object locals: `local_150` channel, `local_158` plugin host,
  `local_50` plugin object, `local_e0` insert index, `local_c4` pattern index,
  `local_270` arrangement, `local_c8` track index, `local_bc`/`local_c0` the
  218/228 counters, `local_dc` the 231 counter, `local_1f8` last controller
  link, `local_1c8` 201's string, `local_298` 169's flags, `local_e8`,
  `local_f9`, `local_fa`, `local_fb`, `local_fc`, `local_100`, `local_10c` the
  pending time-marker fields, `local_f8` 160's value.
- `local_b18`, `local_b10`, `local_b08` are not locals. They sit at the bottom
  of the frame and are the fifth and later stack arguments of the next call.
  When a row says a value "travels in r8/r9 (lost)", Ghidra showed a two- or
  three-argument call whose real signature (visible in the callee, e.g.
  `FUN_012d45b0(obj, pattern, step, kind, value)`) has more.
- `FUN_01046a70(recId, value, flags)` is FL's REC-event dispatcher (strings
  "Phase: %s", "Hold", "preserve"); the loader calls it after the loop for each
  225 record and for IDs 0x40000000..0x40000002. It is not an event
  sub-reader, and none of the census IDs route to it.
- Post-load work (after `LAB_00f50cbd` is skipped) includes the fixup passes,
  the deferred sample list (local_60, FUN_00f3bcf0), the REC replay, the
  deferred plugins (local_70, FUN_00c31890), and the "Missing plugins" and
  "Invalid data" dialogs.
