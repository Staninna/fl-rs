# FLEX: state, pack format, engine layers

Recovered from `FLEX_x64.dll` (FL Studio 2026, Delphi shell, image base
0x400000) with its two engines beside it: `engine_x64.dll` (exports
`CreatePlugInstance`, `DispatchToEngine`; the v1 engine for `.flexpack`) and
`engineV2_x64.dll` (exports `CreatePlugInstance`, `FLEX2Dispatcher`; for
`.flex2pack`). Neither engine was imported this pass. Ghidra project
`ghidra/proj`, project name `flex`; decompiles under `ghidra/out/flex/`
(`vmt/` the SDK slots with callees, `pack/` the preset-from-pack loader,
`packfmt/` the pack reader and store code). Corpus: 57 states in the 165
`.flp` (56 of 2707 bytes, 1 of 2626), 138 shipped presets (137 of 2694, one
of 2707: `Data/System/FLEX.fst`). Four Ghidra runs. The `SynthEditorForm`
wheel table is at `ghidra/out/flex/params_dfm.tsv`.

The shell owns 45 parameters (two AHDSR envelopes, eight macros, filter,
post filter, delay, reverb, limiter, transpose) that it hands to the engine
as 0..1 floats; the patch itself (oscillators, layers, modulation, samples)
lives in a `.flexpreset` inside a pack file. The state is therefore small
and incomplete on its own: it names a pack and a preset, carries the 45
floats as a 128-slot block, and the plugin is silent until the pack is
installed under `Data/Patches/Packs/FLEX/Packs/`.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `inferred`, `file`.

## How the plugin is wired

`CreatePlugInstance` @ `0x007dd2a0` -> class `TTestPlug`, VMT `0x007dd090`
(instance size 0xb20, parent `TDelphiFruityPlug` `0x0073e7c8`).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x007df380 |
| 2 | Idle_Public | 0x00742250 |
| 3 | SaveRestoreState | 0x007e0040 |
| 4 | GetName | 0x007e17b0 |
| 5 | ProcessEvent | 0x007dff70 |
| 6 | ProcessParam | 0x007e0e80 |
| 8 | Gen_Render | 0x007e1a00 |
| 9..12 | TriggerVoice, Voice_Release, Voice_Kill, Voice_ProcessEvent | 0x007e1bc0, 0x007e1d30, 0x007e1d70, 0x007e1db0 (forwarders to the engine) |

Classes: `TPacksData` VMT `0x00774350` (the installed-pack list; entries at
`+0x2c`, each with the pack path at `+0x70` and the display name at `+8`),
`TChunk*` (PNG), `TSharedPtr<NewsDownloader.TILProductItem>` (the store
feed from `cache.json`).

Plugin object:

| offset | type | meaning |
|---|---|---|
| 0x118 / 0x120 | ptr | engine objects: `[0x23 + sel]`, sel = `this+0x9f1` (0 engine v1, 1 engine v2); vtable+0x18 Save/LoadFromStream, +0x30 ProcessParam |
| 0x170 | ptr | kept engine stream when the pack is missing (`param_1[0x2e]`) |
| 0x178 | char[0x401] | pack name (chunk 1 start) |
| 0x579 | char[0x401] | preset name |
| 0x97a / 0x97b | u8 | 1 / 0 (written as constants) |
| 0x97c | u8 | copy of form+0xae3 (a view flag) |
| 0x97d..0x985 | u8[9] | "lock settings when changing presets" per panel (form+0xeb8 + 8 k, +0x48e): filter, envelopes, master filter, delay, reverb, limiter, output, macros, pitch (`inferred` order from the DFM lock buttons) |
| 0x986 | i32 | copy of form+0xc4c |
| 0x98a / 0x98b / 0x98c | u8 | chunk 3 start: 1 (form+0xebc), form+0xebd flag, engine selector copy (this+0x9f1) |
| 0x9c2 | u8 | chunk 4: (form+0x9a8)->0x48e (arpeggiator panel flag, `inferred`) |
| 0x9f1 | u8 | engine selector in use |
| 0x9f4 | i32[45] | the shell parameter ints (REC 0..44) |

## 1. State layout (event 213: 2707 bytes; 2694 without chunk 4; 2626 old)

`SaveRestoreState` (`FUN_007e0040`) writes a chunk stream through
`FUN_00747b20` / `FUN_00747b40(writer, id, size, data)`:

```
save:    Write(i32 1)                                   // version
         chunk 1: 0x812 bytes at this+0x178             // when no kept stream: names refreshed from the form (+0xf24 pack, +0xf2c preset), flags 0x97a.. filled
         chunk 2: the engine's SaveToStream into a memory stream, or the kept stream 0x170
         chunk 3: 0x38 bytes at this+0x98a
         chunk 4: 1 byte at this+0x9c2
restore: Read(i32 v); v > 1: refuse (FUN_007e0cf0 message)
         0x98a = 1, 0x98b = 0, 0x98c = 1; local engine sel = 1
         loop over chunks: id 1 -> 0x812 bytes to 0x178; id 2 -> memory stream; id 3 -> 0x38 bytes to 0x98a; id 4 -> 1 byte (engine selector hint)
         if chunk 1 seen: FUN_007c14e0(form, pack, preset, 0x98c): find the pack in TPacksData, select it, load the preset by name
             success: FUN_007bcea0(form, 0x97c), push the 9 lock flags, form+0xebd = 0x98b, arp flag
             failure: keep the chunk 2 stream in 0x170 and FUN_007c60f0 (form shows "missing pack" with the names)
         if chunk 2 seen: engine[sel].LoadFromStream(chunk 2)      // the 45 floats, applied in both cases
         GetValue/UpdateValue loop over 45 params; host msg 3
```

Chunk header: `i32 id`, `i64 size`, then `size` bytes. Layout seen:

| chunk | size | content | corpus | confidence |
|---|---|---|---|---|
| version | 4 | 1 | 1 x195 | loader, corpus |
| 1 | 2066 | `char[1025]` pack name (NUL-padded), `char[1025]` preset name, 16 bytes: `01 00`, u8 0x97c, u8[9] locks, i32 0x986 | tail `01 00 01 00 00x9 02 00 00 00` x154, `.. 00 00 00 00` x21, `.. 03` x20 | loader, corpus |
| 2 | 532 | engine block: `i32 0`, `i32 520` (payload), `i32 0`, `i32 2` (format), `i32 128` (slots), `f32[128]` | all 195 | loader, corpus, inferred |
| 3 | 56 | `u8 0x98a` (1), `u8 0x98b`, `u8 0x98c` engine sel (0; one state 2), 53 zero bytes | `01 00 00..` x174, zeros x19 | loader, corpus |
| 4 | 1 | 0x9c2 | present in the 57 project states (0 x55, 1 x2), absent in 137 presets | loader, corpus |

The 2626-byte project state is chunks 1 and 2 only (one older file).

Engine block floats (chunk 2, `20 + 4 k`), REC index k:

| k | DFM object | range | default | float | corpus (195) | confidence |
|---|---|---|---|---|---|---|
| 0 | volume env attack (Tag 0, not in the DFM dump) | 0..65536 | 32768 | 0.5 | 0.5 x186 | loader, inferred |
| 1..4 | `VolumeEnvHWheel`, `D`, `S`, `R` | 0..65536 | 32768 | 0.5 | 0.5 x193 | dfm, corpus |
| 5..9 | `FilterEnvAWheel`, `H`, `D`, `S`, `R` | 0..65536 | 32768 | 0.5 | 0.5 x187 | dfm, corpus |
| 10..17 | `Macros1Wheel`..`Macros8Wheel` | 0..65536 | 0 | preset-defined | 0.5 or 0 mostly; 17 distinct on macro 1 | dfm, corpus |
| 18, 19, 20 | `FilterModCutoffWheel`, `FilterModEnvAmtWheel`, `FilterModResoWheel` | 0..65536 | 32768 | 0.5 | 0.5 x172..192 | dfm, corpus |
| 21, 22 | `MasterFilterCutoffWheel`, `MasterFilterResoWheel` | 0..65536 | 65536, 32768 | 1.0, 0.5 | 1.0 x185 | dfm, corpus |
| 23 | `MasterFilterCombo` post filter type | combo | 0 | index / `DAT_007e12a8` (0.0645 = 2/31) | 0.0645 x180 | dfm, corpus |
| 24 | `DelayTypeCombo` | combo | 2 | index / `DAT_007e12b0` (0.667 = 2/3) | 0.667 x117, 0.333 x61 | dfm, corpus |
| 25..29 | `DelayMixWheel`, `DelayTimeWheel` (1..768), `DelayFeedbackWheel`, `DelayModWheel`, `DelayColorWheel` | 0..65536 (time 1..768) | 32768 | (v - min) / (max - min) | time 0.249 x135 (192 steps) | dfm, corpus |
| 30..34 | `ReverbMixWheel`, `ReverbDecayWheel`, `ReverbSizeWheel`, `ReverbModWheel`, `ReverbColorWheel` | 0..65536 | 32768 | | mix 0.46 x99 | dfm, corpus |
| 35, 36 | `LimiterPreVolumeWheel`, `LimiterPostVolumeWheel` | 0..65536 | 32768, 65536 | | post 0.78 x127, 1.0 x66 | dfm, corpus |
| 37, 38 | `DelayActiveBtn`, `ReverbActiveBtn` | 0, 1 | | 1.0 / 0 | delay on x125, reverb on x173 | dfm, corpus |
| 39 | `TransposeSlider` | -2400..2400 | 0 | 0.5 | 0.5 x195 | dfm, corpus |
| 40, 41 | `MasterFilterActiveBtn`, `OutputLimiterBtn` | 0, 1 | | | 1.0 x192, 1.0 x194 | dfm, corpus |
| 42 | `LimiterLMHMixWheel` | 0..65536 | 65536 | | 0.5 x159, 1.0 x33 | dfm, corpus |
| 43 | `LimiterTypeCombo` | combo | 0 | index / `DAT_007e12a0` | 0.0645 x163 | dfm, corpus |
| 44 | `ReverbModSpeedWheel` | 0..65536 | 21845 | 0.333 | 0 x161 | dfm, corpus |
| 45..127 | unused | | | 0 | 0 x195 | corpus |

Preset check (`.fst` files name the plugin "FLEX"):

| preset | pack / preset | envelopes | macros 1..8 | filter cut/env/res, post cut/res | delay type/mix/time/fb, on | reverb mix/decay/size, on | limiter pre/post, LMH, type |
|---|---|---|---|---|---|---|---|
| `System/FLEX.fst` (template) | Arksun Cityscape / Dystopian Lead | all 0.5 | 0 x8 | 0.5 0.5 0.5, 1.0 0.0 | 0.67 0.68 0.25 0.4, on | 0.66 0.8 0.9, on | 0.5 1.0, 1.0, 0 |
| 001 Acoustic Piano | General Midi Library / Acoustic Piano | 0.5 | 0.5 0.5 0.5 0 0 0 0 0 | 0.5 0.5 0.5, 1.0 0.5 | 0.33 0.26 0.25 0.5, off | 0.5 0.36 0.74, on | 0.9 0.75, 0.5, 0.07 |
| 002 Bright Piano | GML / Piano Bright | 0.5 | same | same | 0.67 0.5 0.5 0.5, off | 0.5 0.28 0.5, on | 0.98 0.75, 0.5, 0.07 |
| 003 Electric Grand Piano | GML / Electric Grand Piano | 0.5 | same | same | 0.67 0.22 0.25 0.5, off | 0.5 0.36 0.78, on | 0.98 0.75 |
| 004 Honky Tonk Piano | GML / Honky Tonk Piano | 0.5 | same | same | 0.67 0.34 0.19 0.36, off | 0.5 0.28 0.5, on | 0.88 0.75 |
| 005 Electric Piano 1 | GML / Electric Piano 1 | 0.5 | same | same | 0.67 0.0 0.25 0.5, on | 0.3 0.4 0.5, on | 0.64 0.75 |

The 137 shipped `.fst` are the General MIDI set (pack "General Midi
Library", 128 programs plus drums); the corpus names "Melodic Techno",
"Arksun Cityscape" and GML presets. Pack and preset are matched by name
(`FUN_007c5f70` finds the pack in `TPacksData`, `FUN_007c5e30` the preset),
case-sensitive as stored.

## 2. Parameter semantics (`ProcessParam` @ 0x007e0e80)

Indices 0..44 (`0x2d`). Flag 0x20 rescales by the control's Min/Max and
`DAT_007e1298 = 2^-30`. Flag 1 stores the int at `0x9f4 + 4 k` and forwards
a float to the engine (`FUN_007e1780(this, k, f, flags)` -> engine
vtable+0x30):

```
k = 23, 24, 43 (combos):  f = v / DAT_007e12a8, / DAT_007e12b0, / DAT_007e12a0   // item count minus one
button controls:          f = v
others:                   f = clamp((v - Min) * ctrl+0x3cc, 0, 1)                // ctrl+0x3cc = 1 / (Max - Min)
```

Flag 2 reads back from the engine and reverses the scaling; flag 0x10
refreshes the control and `FUN_007d1e10(form, k)` (macro label update);
flag 4 formats the hint through `FUN_007e1370`, "Not Used" for unlabelled
macros, and param 0x27 (39, transpose) also writes the semitone label.

The engine (`FUN_007e1780` -> `engine->+0x30(k, f)`) owns the mapping from
the 0..1 float to its own ranges; the shell never sees Hz or ms. Macros are
likewise opaque to the shell: their targets are defined inside the
`.flexpreset`.

Dispatcher (`FUN_007df380`): forwards sample rate, process mode and MIDI to
the engine (not read in detail).

## 3. Packs: where they live and what they contain

Installed packs live in `<FL>/Data/Patches/Packs/FLEX/Packs/` (`cache.json`
`file_path` = `plugindata:\<category>\Packs`), one `<name>.flexpack` or
`<name>.flex2pack` plus a `<name>.ini` with `[Ini]\r\nVersion=N` (the pack's
`file_version`). The shell's `data.json` (64 products, keys `name`,
`product_id`, `packpath`, `file_version`, `owned`, `subscribed`, ...) and
`cache.json` (same products with `filename`, `availability`, `file_checksum`,
`minimumFlVersion`, `commercial_encrypted`, `free_encrypted`) are the store
catalogue, refreshed online. Eight packs are installed in this prefix:
Arksun Cityscape, Core 808s (v2), Core Synths (v2), General Midi Library,
Melodic Techno, Mobile Synth Pluck, Mobile Tuned 808 Bass, Olbaid
Compendium.

Pack archive (`file`, parsed for all eight to the last byte):

```
u32 be  magic/flags: 0x00000002 (.flexpack), 0x01000002 (.flex2pack)
u32 be  entry count
entries: u32 be name length (including the trailing NUL), name bytes, u64 be offset, u64 be size
data:   entries back to back from the end of the index to the end of the file
```

Entry names and kinds:

| entry | count | content |
|---|---|---|
| `presetsIndex.presetsIndex` | 1 (2 in one pack) | the preset list (names, tags, categories); 1..12 KB, obfuscated |
| `pcf.pcf` | 1 | 45 bytes, byte-identical in every pack (a key or certificate block) |
| `<name>.flexpreset` | per preset | the v1 patch, 33..160 KB (1.8 MB with embedded data), obfuscated |
| `<name>.flex2preset` + `<name>.usedsamples` | per preset (v2) | the v2 patch and its sample list |
| `<name>.flac` | per sample | FLAC audio, plain (GML: 641 samples; Mobile Synth Pluck: 385) |
| `*.ini` | 0..2 | pack metadata |

The preset entries are not plain: a `.flexpreset` starts with a 16-byte
block that repeats verbatim where the plaintext repeats, and two presets
from different packs share identical blocks. Blocks that differ only in the
bytes where a double would carry `3f f0` (1.0 vs 0.0) point at a 16-byte
XOR keystream over a plain structure of doubles, not a block cipher. The key
was not recovered this pass; the decoder sits behind the preset loader
(`FUN_007ddc20(form, preset, 0, 1)` -> engine) and `FUN_007c74e0` (pack
select). `FUN_00775560` is only the pack path builder (`FUN_00448360` swaps
`/` for `\`), so the three functions that contain XOR (`FUN_00775560`,
`FUN_007b7eb0`, `FUN_007dc430` in `packfmt/`) are not it; the decoder is
inside the engine DLL or behind `FUN_007ddc20`'s callees.

Engine layers (from the file names and the shell): `engine_x64.dll` plays
`.flexpreset` (v1: layers of wavetable / sample oscillators with FLAC
samples referenced by name), `engineV2_x64.dll` plays `.flex2preset` (v2:
with a `.usedsamples` list per preset and `minimumFlVersion` 25.2.5). The
shell picks the engine from the pack kind (`0x9f1`) and both expose the same
45-parameter interface.

## 4. What an implementation needs next

1. Import `engine_x64.dll` (MSVC, 5.7 MB) and read `DispatchToEngine`: the
   45-float parameter mapping, the `.flexpreset` parser and its decoder. Two
   runs (import, dispatch with callees). This is where every Hz and ms lives.
2. `FUN_007ddc20` and its callees in the shell (one DecompileAddrs run) to
   see whether the shell or the engine strips the XOR, and to find the key
   (probably derived from `pcf.pcf` or a constant in the engine).
3. `presetsIndex` format after decoding: names, tags (`typetags.txt`,
   `styletags.txt` in the plugin folder list the vocab: Arpeggio, Bass, Bell,
   ... and Basic, Complex, Bright, ...).
4. `engineV2_x64.dll` (8.1 MB) only if v2 packs matter: the corpus has no v2
   preset.
5. The arpeggiator (`ArpeggiatorBtn`, chunk 4) and the `DumpFSCButton`
   (score to piano roll) are shell GUI; skip.

Estimated: three to four Ghidra runs before the engine's parameter maths can
be written down; the shell side (this document) is complete.
