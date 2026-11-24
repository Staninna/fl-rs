# FLEX: state, pack format, engine layers

Recovered from `FLEX_x64.dll` (FL Studio 2026, Delphi shell, image base
0x400000) with its two engines beside it: `engine_x64.dll` (exports
`CreatePlugInstance`, `DispatchToEngine`; the v1 engine for `.flexpack`) and
`engineV2_x64.dll` (exports `CreatePlugInstance`, `FLEX2Dispatcher`; for
`.flex2pack`). The v1 engine is read in sections 4 and 5. Ghidra project
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
installed under `Data/Patches/Packs/FLEX/Packs/`. `crates/plugins/src/flex.rs`
reads the pack from there at run time (section 6).

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

## 3. Packs: where they live and how they are read

Installed packs live in `<FL>/Data/Patches/Packs/FLEX/Packs/` (`cache.json`
`file_path` = `plugindata:\<category>\Packs`), one `<name>.flexpack` or
`<name>.flex2pack` plus a `<name>.ini` with `[Ini]\r\nVersion=N`. The
shell's `data.json` and `cache.json` are the store catalogue. FL 26.1.7
installs eight: Arksun Cityscape, Core 808s (v2), Core Synths (v2), General
Midi Library, Melodic Techno, Mobile Synth Pluck, Mobile Tuned 808 Bass,
Olbaid Compendium. Nothing of a pack is in this repository: the plugin
reads it from the install at run time (`flex_pack.rs`).

Pack archive (`file`, all eight read to the last byte):

```
u32 be  kind: 0x00000002 (.flexpack, engine v1), 0x01000002 (.flex2pack, engine v2)
u32 be  entry count
entries: u32 be name length (with its NUL), name bytes, u64 be offset, u64 be size
data:   the entries back to back after the index
```

| entry | content |
|---|---|
| `presetsIndex.presetsIndex` | the preset list (names, tags); not read |
| `pcf.pcf` | 45 bytes, the same text in every pack, masked by its place (below) |
| `<name>.flexpreset` | a v1 preset, masked (below) |
| `<name>.flex2preset` + `.usedsamples` | a v2 preset and its sample list (v2 packs only) |
| `<name>.flac` | a sample, FLAC, masked like the rest; a preset names it `<name>.wav` |

### The mask (`loader`, `file`: all 1766 entries of the eight packs)

Every entry (presets, FLAC samples, `pcf.pcf`, the index of presets) is
XORed, byte `i` with `key[i % 16]`, by a 16-byte key that depends on the
entry's place in the pack's index and on a master key (`FUN_1803de7a0`).
The engine reads the packs itself (`FUN_18038b980`); the shell only names
them.

Master keys (`FUN_180389b60`): four 16-byte digests (`FUN_1801c8630`,
MD5) of the pack's product name, of the same with `_subscribed`, of
`this_pack_is_free` and of `this_pack_is_free_for_customers`, each stirred
byte by byte: `m[i] = h[i] ^ f_i(h)` with, mod 256,

```
f0 = (h9+h1-h5)*0x0c+0x74     f8  = h8*3+100
f1 = (h1+h11+h15)*0x45+1      f9  = h1+h1-h15-0x0d
f2 = (h4+h1-h5)*0x7b+0x49     f10 = (h2+h1+h5)*5+0xbe
f3 = (h2+h9+h15)*8+0x20       f11 = (h5+h7-h15)*7-0x34
f4 = h5*2+0x4c                f12 = h2+h8-h5+0x11
f5 = (h8+h12+h5)*0x4d-0x1d    f13 = (h1+h10+h15)*2+0x77
f6 = (h11+h11-h15)*0x0d+0x14  f14 = h10*0x0a+0x0b
f7 = (h2+h3-h15)*0x63+0x39    f15 = (h9+h11-h15)*0x0f+0x98
```

The free packs FL installs use `this_pack_is_free` (MD5
`5d671e7d34588d1112f38a20e6c00a67`, master `d140649dc8ab944488a985fa0fe765cb`).

Entry key for index `n` (`FUN_1803de7a0`): `b = (((3n & 0x1f) * -0x1e) &
0x3f) * 0x1f`, then by `b & 15`: 0 `^0x12`, 2 `^0x29`, 4 and 12 `^0x11`, 6
`^0xa9`, 8 `^0x02`, 10 `^0xbe`, 14 `^0x0b`; and

```
k0 = m9*b*0x74 ^ m0      k8  = (m3*0x64+b) ^ m8
k1 = (b+m0) ^ m1         k9  = m6*b*-0x0d ^ m9
k2 = (m10*0x49+b) ^ m2   k10 = (m8*0x42+b) ^ m10
k3 = m1*b*0x20 ^ m3      k11 = (b-m4*0x34) ^ m11
k4 = (m11*0x4c+b) ^ m4   k12 = b*m12*0x11 ^ m12
k5 = m9*b*-0x1d ^ m5     k13 = (m2*0x77+b) ^ m13
k6 = (m15*0x14+b) ^ m6   k14 = (b-m9*0x0b) ^ m14
k7 = (b-m7*0x39) ^ m7    k15 = (b-m1*0x68) ^ m15
```

so there are 32 keys, by `n % 32`. `pcf.pcf` (entry 1) unmasks to
`if_you_can_read_this_then_encryption_did_work`, the engine's check that the
master is right (`0x180390133`). A preset then starts `xelf00su`, a sample
`fLaC`. `flex_pack.rs` builds this; its test checks the master and two keys
against the packs.

## 4. The v1 preset (`engine_x64.dll`)

`engine_x64.dll` (MSVC x64, image base 0x180000000) imported into Ghidra
11.4.2 (`/root/ghidra-projects/flex`, one analysis run, then
DecompileAddrs). Its RTTI names the classes: `FlexPlugin`, `Engine`,
`Preset`, `Layer`, `Zone`, `Voice`, `Osc`, `Env`, `LFO`, `FilterPlayer`,
`FXChain`, `ModMatrixDSP`, and the effects (`ILDelay3Plugin`,
`FruityReeverb2`, `Chorus`, `Phaser`, `EqBank`, `TranceGate`,
`Compressor`, `FilterProcessor`, and FL plugins hosted through
`TFruityPlugHost`). The engine is DirectWave's ("DirectWaveCTEngine") grown
a synth.

All fields little-endian. Strings (`FUN_1801b64a0`) are `i32 length`
(with the NUL) and the bytes.

### Preset (`FUN_1803f1300`)

```
"xelf" "00su"
i32 version            <= 0x23 (corpus: 17 Arksun, Olbaid, Mobile; 19 GML; 35 Melodic Techno)
i32 n, f32[n]          the shell's 45 parameters (44 below 19), each through FUN_1803eefc0
i32 layers             each a layer (below); 4 in v17/19, 16 in v35
hcxf chain             the master effects (below)
v>=3  i32, i32; v>=4 u8; v>=8 strings (macro names, tags) ...   not read by the port
```

### Layer (`FUN_1803c9aa0`, 'mgrp')

```
"mgrp", i32 version (<= 4)
i32 n, f32[n]          300 layer parameters (FUN_1803c93f0): 0 int(15v), 1 polyphony int(126v)+2,
                       3 glide, 4 mono, 5 active (>= 0.5), 6 volume (taper), 8 MIDI channel int(127v)-1
i32 zones              each a zone (below)
v<=3: one matrix; v4: i32 n matrices (FUN_180375750, 8 in the corpus):
    u16, i32, then 32 rows of u16, u8 on, i32, u8, f64, f32, u8   (no row is on in the corpus)
hcxf chain             the layer's effects
v>=2  4 point envelopes (FUN_1803c0d30)
v>=3  99 x (u8, i32, i32)
```

### Zone (`FUN_180427bf0`)

```
[0xff, u8 version]     (absent: version 0; corpus 7)
string name
i32 root key           (zone+0x18b4)
i32 low key, high key, low velocity, high velocity   (bytes at +0x18b0..+0x18b3)
i32 (skipped)
v>=2  i32 round-robin group (+0x18b8), i32 group (+0x18bc)
i32 n slots (4), each (FUN_18041f770):
    i32 version, u8 flag, i32 mode (+0x10: 1 loops), i64 start, i64 loop start, i64 loop end,
    string file (.wav; the pack has it as .flac), v>=2 i32, v>=3 f64
i32 n, f32[n]          the voice's parameters (746 in the corpus, 0x2ef = 751 read);
                       below version 7 parameters 20, 21 and 43 + 64 k are set to 0.5,
                       below 6 parameters 46, 47 (+ 64 k) are converted from 2v+0.5 steps of 1/1023
v>=1  4 point envelopes (i32 count of {f64 x, f64 y, u8 type, f32 bend})
v>=5  per slot a wavetable (FUN_18041c970): i32, string name, i32 frames a cycle, i32 cycles (<= 256)
v>=9  per oscillator two strings: the custom waves 1 and 2
```

### Effect chain (`FUN_1803bd0c0`, 'hcxf')

```
"hcxf", i32 version (<= 5)
i32 n, then n x (i32 type, v>=2 u8 bypass, v>=5 u8 on)
per effect: v>=4 u8 own; own and an FL plugin: i32 size + the plugin's own state (FL's event 213);
            own and an engine effect: nothing; else i32 n, f32[n] parameters
i32 n links x (i32, i32, f32, v>=3 f32)
```

Types (`FUN_1803bbc80`'s jump table, RTTI of each constructor's vtable):
0, 10, 12, 20, 21 `SimpleAudioProcessor` kinds; 1 `Phaser`; 2 `Chorus`;
3 `FruityChorus`; 4 `EqBank`; 5 `TranceGate`; 6 `ILDelay3Plugin`;
7 `FruityReeverb2`; 8 `Compressor`; 9 `FilterProcessor`; FL plugins: 11
Fruity Convolver, 13 Maximus, 14 Fruity WaveShaper, 15 EQUO, 16 Fruity
Flangus, 17 Fruity Stereo Enhancer, 18 Patcher, 19 Fruity Love Philter.
The port parses every chain of the corpus but builds none.

## 5. The voice (`Voice`, constructor `FUN_180418ab0`)

A voice is one zone sounding. Its parameters are the zone's 751, handed
out by index to its parts (each part's base is at its `+0xc`, its count is
vtable `+0x48`): the filter pair (base 1), oscillators 1..4 (33, 97, 161,
225; 64 each), LFOs 1..4 (289, 305, 321, 337; 16 each), envelopes 1..4
(353, 383, 413, 443; 30 each), the rest the voice's own (`FUN_180416980`).
A modulation destination's id in the editor's list (`FUN_18041faf0`) is
its parameter plus one.

| parameters | meaning | source |
|---|---|---|
| 1 | filter routing int(2v+0.5) | FUN_180396fa0 |
| 2, 6 | filter 1, 2 type int(127v+0.5): 0 off, 1 LP12, 2 HP12, 3 LP24, 4 HP24, 5 Minisynth, 6 Vowel, 7..9 SVF4 LP/BP/HP, 10..12 Phaser, 13, 14 Comb +/-, 15 Peak, 16 Bell, 17, 18 shelves, 19 Allpass, 20 Notch, 21..24 Moog 6..24 dB, 25..28 MoogFast, 29..32 Juno | strings at 0x18039680f |
| 3, 4, 5 / 7, 8, 9 | filter 1 / 2 cutoff, resonance, envelope amount | FUN_180396fa0, FUN_180395c00 |
| 10..15 | how far the shell's filter knobs move 3, 7, 4, 8, 5, 9 | FUN_180416f10 |
| 17, 19 | filter insert amounts | dest list |
| 33 + 64k + r | oscillator k: r0 on, r1 type int(127v+0.5) (0 Sample, 1 Waveform Dual + FM, 2 Noise, 3 WaveTable, 4 WaveTable Xfade, 5 Waveform Simple, 6 WaveTable HQ, 7 Waveform Simple + FM), r2 unison int(7v+1), r3 detune, r4 detune curve 8v+2, r5 unison pan spread, r6 transpose int(96(v-0.5)), r7 start phase 0.999v, r8 phase retrigger, r9 fine (v-0.5), r10 pitch 96(v-0.5), r11 level 2v, r13 / r14 wave 1 / 2 int(1023v+0.5), r15 wave mix, r16 PW, r17, r18 PW amounts, r19..22 FM from oscillators 1..4, r45 sample start, r53 wavetable position, r55 morph | FUN_1803e0a90, FUN_1803e15f0, dest list |
| 353 + 30k + r | envelope k: r0 delay 3v s, r1 attack, r3 its bend, r4 hold 10v s, r6 decay, r8 its bend, r9 sustain, r11 release, r13 its bend; times `2^(15v)` ms, 0 at v = 0 | FUN_180390750, FUN_180390c00 |
| 473 + 3i, 474 + 3i, 475 + 3i | route i of 64: source round(65535 v), target round(65535 v) (a parameter plus one), amount 2v-1 | zone vtable 0x18041ea50, 0x18041ea20, FUN_180415970 |
| 665..668 | oscillator volumes (taper); 746..749 invert them | FUN_180416980 |
| 669..672, 678 | oscillator pans, all oscillators' pan (pan law) | FUN_180416980 |
| 673..676 | oscillator 1 AM by 1..4 | dest list |
| 677 | all oscillators' volume (taper) | FUN_180416980 |
| 679 + i | route i's source bend | FUN_180415950 |
| 743 | pitch 96(v-0.5) semitones | FUN_180416980 |
| 744, 745 | volume (taper), pan (pan law) | FUN_180416980 |

The shell's knobs pull a zone's value (`FUN_180416f10`): `v + 2(s-0.5) *
(s >= 0.5 ? 1-v : v) * depth`, for filter cutoff (shell 18, depths 10, 11),
envelope amount (19; 14, 15), resonance (20; 12, 13), and envelope 1's
attack, hold, decay, sustain, release (shell 0..4) and envelope 2's (5..9).
Shell 39 transposes by `48 s - 24` semitones (`FUN_1803eefc0`).

Sources (`FUN_180415af0`): 1 one, 2 velocity (0..1), 3 a fresh random
value each step, 4 the note with glide / 127, 5..12 macros 1..8 (shell 10..17),
133..140 the same less 0.5, 261..268 LFOs 1..4, 269..272 envelopes 1..4,
273..284 the layer's LFOs and envelopes, 285 the note / 127, 286..289
random at note on (per route or shared, uni- or bipolar), 290, 291 the note
bipolar, 292..295 master filter. The corpus uses 2 (velocity to volume in
every zone), the macros, the LFOs, the envelopes and 285.

### A control step (`FUN_1804163d0`, every 32 samples)

The four envelopes move a step (they run at rate / 32, `FUN_180417820`);
the routes add `bend(source) * amount` to their targets and clamp to 0..1
(`FUN_1803da290`); the oscillators work out their unison pitches
(`FUN_1803e15f0`); the filters their coefficients (`FUN_180395c00`); and the
gains `taper(744) * env1 * pan(745)` and, per oscillator, `taper(677) *
taper(665+k) * pan(669+k) * pan(678)` (`FUN_1801a2200`) ramp over the
step. Bends (`FUN_180402330`, `FUN_180402290`): `x = 2v - 1`, `k = (e^(8.7557
|x|) - 1) * 0.1 sign x`; `t -> 1 - (e^((1-t) ln(|k|+1)) - 1)/|k|` for k > 0,
`(e^(t ln(|k|+1)) - 1)/|k|` for k < 0, straight at 0.5.

An oscillator at note `n` (key, pitch 743, the oscillator's transpose,
fine and pitch, shell transpose, unison detune) plays a wave at `8.1758 *
2^(n/12)` Hz (`DAT_18048f968`), a sample at `2^((n - root)/12)` times its own
rate. Unison voice `u` of `N` sits at `x = 2u/(N-1) - 1`: pitch moved by the
detune bent by r4 (`FUN_180321170`), pan `0.5 - spread/2 + u spread/(N-1)`,
gain from a table at `DAT_1819221ec` by `N`.

Waves (`FUN_18038e890`): 28 single cycles band-limited into 48 levels at
`DAT_180514a80`: 0 saw, 1 triangle, 2 sine, 3 square, then the 24 slices of
the float WAV at `0x1804f40a0` (cue labels Beast, Crunch, Devils Horn,
Digital, Dirt, Electro, Filth, Fox, Funky, Grubby, Keys, Liquid, Ogre, Saw
Mean, Saw Retro, Silk, Smooth Sub, Square Retro, Square Smooth, Strat,
String, Throat, Troll, Vocal; 1024 frames each).

Filters: cutoff `14.35 * 2^(10.5833 c)` Hz, `c = cutoff + env2 * amount`
(envelope 2 is the filter envelope; `voice+0x270`), a TPT state-variable
stage with `g = tan(pi f / rate)` and damping `1 - 0.99 res`.

Envelope 1 is the volume envelope (`voice+0x120`); a voice ends when it is
let go and its oscillators are done.

## 6. What the port builds (`crates/plugins/src/flex.rs`)

Built: the pack index, the mask (exact), the v1 preset to the end of its layers,
FLAC samples, zones by key and velocity, the four oscillator families
(sample with its loop, the 28 waves with band-limited levels read from the
installed DLL, the two-wave mix, noise, wavetables over a slot's sample),
unison pitch and pan, the four envelopes with their bends, the 64 routes
for the sources above but the LFOs and the layer's, the two filters as SVF
stages, the volume taper and pan law, the shell's pulls and transpose.

Approximated or left out (the row in `docs/plugins.md` and `fidelity()`
say so): LFOs, FM/AM, pulse width, phaser/comb/vowel/shelf filters (passed
through), Moog/Juno ladders (SVF stages of their order), the unison
detune curve and gain table (straight spread and `1/sqrt(N)`), the layer
and master effect chains (the shell's delay, reverb, limiter and master
filter), glide, round-robin groups, macros beyond the routes, the
`.flex2pack` engine. No null test against FL.

## 7. What is left

1. The master chain (`hcxf` after the layers): its types 6 and 7 are
   FL's Delay 3 and Reeverb 2 with the engine's own parameter floats; map
   them onto `delay_3.rs` and `reeverb_2.rs`, and the limiter.
2. LFOs (`LFO`, vtable 0x1804867c8, setParam `FUN_1803d22f0`), used by the
   corpus mostly for pitch with amount 0.
3. FM between oscillators (`r19..22`), the Moog/Juno ladders
   (`FUN_180397870`), the unison table `DAT_1819221ec`.
4. Bought packs: their master is the MD5 of the product name (or with
   `_subscribed`), from `data.json`; only free packs are read now.
5. `engineV2_x64.dll` for `.flex2pack` (Core 808s and Core Synths; one
   template channel in the corpus).
