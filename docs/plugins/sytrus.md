# Sytrus: state, parameters, signal architecture

Recovered from `Sytrus_x64.dll` (FL Studio 2026, Delphi native plugin, image
base 0x400000; no engine DLL, the reverb and envelope classes are compiled
in). Ghidra project `ghidra/proj`, project name `sytrus`; decompiles under
`ghidra/out/sytrus/` (`vmt/` the SDK slots with callees, including the
constructor `FUN_00865c10` and the index decoder; `art/` the articulator
stream loaders and the part recompute stubs; `voice/` the voice renderer and
the effects). Corpus: 268 states in the 165 `.flp` (versions 24 x132, 22
x78, 25 x49, 23 x9; 1.1 to 1.5 KB compressed, 60 to 65 KB inflated), 996
shipped presets in 984 `.fst` (versions 5 to 25; 1035 of the 1264 blocks
parse to the last byte with the layout below, the 229 failures are all
version 5, 6 and 7 presets whose articulator stream predates the format read
here). Four Ghidra runs (import, slots, articulators, voice). The editor's
wheel table is at `ghidra/out/sytrus/params_dfm.tsv`; `Data/Default.dat` is a
version 12 state (`Default.dat` = i32 12 + zlib).

Sytrus is six operators, three filters and a main part, each with a list of
articulators (six per operator, eight per filter, two for the main part),
each articulator holding eight `TCustomMEnvelope` sub-objects (envelope, LFO,
key map, velocity map, mod X, mod Y, random, unison). Operators feed each
other through the FM/RM matrix and the filters, the filters chain ("to
next"), and the main part has unison, three delay units, a chorus, a reverb
and a three-band EQ. The state is a zlib stream of about 65 KB because every
one of the 62 articulators serialises its eight curves.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `inferred`.

## How the plugin is wired

`CreatePlugInstance` @ `0x00860fb0` -> constructor `FUN_00865c10`, class
`TTestPlug`, VMT `0x00860da0` (instance size 0x63b0, parent
`TDelphiFruityPlug` `0x007765f8`).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x00861360 |
| 2 | Idle_Public | 0x0077a5c0 |
| 3 | SaveRestoreState | 0x00862d70 |
| 4 | GetName | 0x00868ed0 |
| 5 | ProcessEvent | 0x00867890 |
| 6 | ProcessParam | 0x00867930 |
| 8 | Gen_Render | 0x008737a0 |
| 9 | TriggerVoice | 0x0086c7d0 |
| 10 | Voice_Release | 0x0086d5a0 |
| 11 | Voice_Kill | 0x0086d870 |
| 12 | Voice_ProcessEvent | 0x0086d950 |
| 20 | (TDelphiFruityPlug) | 0x00865b00 |

Classes: `TOpArticulator` VMT `0x008605f8` (size 0x80, over `TOscArticulator`
`0x00781978` over `TArticulator` `0x00781818`), `TSytrusShape` `0x00846ee8`
(size 0x878, the operator waveform), `TFruityReverb` `0x0083f440`,
`TDelay_32FS` `0x0083cf00`.

Plugin object:

| offset | type | meaning |
|---|---|---|
| 0x12c | i32[3] | globals REC 0..2: main volume (128), 0x130 (128), main pitch (0) -> `FUN_0086a1d0` |
| 0x138..0x3bf | | 648 bytes inside the saved main block; not resolved (the FM/RM matrix levels are the candidate, see section 4) |
| 0x3c0 | i32[8] | REC 3..10: two groups of (128, 128, 0, 128); no recompute on change |
| 0x3e0 | i32[7] | REC 11..17 unison: order, pan, vol, pitch, sub, phase, env -> `FUN_00871fd0` |
| 0x3fc | i32[9] | main part direct params 0..8: ?, chorus order, depth, speed, delay, spread, cross, vol, FX send level |
| 0x420 | i32 | FX send target (-1 none; `Host.GetInBuffer(0x420 + 1)` in Gen_Render) |
| 0x424 / 0x428 | i32 | Mod X / Mod Y (0..512, 256) -> `FUN_0086a3e0`; 0x42c f32 smoothing; 0x440 u8 smooth on |
| 0x4f0 | i32 | (v4+) saved |
| 0x4f4, 0x4f5, 0x4f6 | u8 | (v6, v7, v13) saved flags; 0x4f6 copied into every operator's +0x868 |
| 0x4f7 | u8[9] | (v14+) saved flags; defaults 0 0 1 0 0 1 0 1 0 at 0x4f7..0x4ff (0x4f8 is REC 596, portamento) |
| 0x500 | u8 | (v24+) saved |
| 0x508 / 0x514 | i32 | editor: selected operator / selected delay unit |
| 0x518 + 0x900 p | part | p = 0..5 operators, 6..8 filters, 9 main |
| 0x5f00 / 0x5f04 / 0x5f08 | i32 | main part REC base (547) / direct count 0x18 / kind 2 |
| 0x5f0c + 0x38 e | delay | three delay units: +0x24 i32[5] params, +0x38 u8[3] flags, +0x18 delay object |
| 0x5f18 | u8 | (v25+) saved |
| 0x5f20 / 0x5f28 | ptr, u8 | reverb object / reverb also on the dry path |
| 0x5fec | i32[9] + u8 + u8 | reverb params and enable (saved as 38 bytes) |
| 0x6018 + 0x90 b | eq band | three EQ bands: +0 level, +4 freq, +8 width, +0xc type (0, 1, 2), +0x18 ... |
| 0x61cc | i32 | EQ mode (0 off; 1; 2 / 3 select the buffer in Gen_Render) |
| 0x61e8 / 0x61f0 / 0x61f8 | ptr | EQ filter objects (two for the stereo pair, one for the send) |
| 0x6298 / 0x62a0 | ptr | the two mix buffers (dry, wet); 0x62f4 oversampling factor |
| 0x62c0 | ptr | voice list; 0x62cc control tick length |
| 0x6308 / 0x630c | i32 | silence countdown / hold |

Part record (0x900 bytes at `0x518 + 0x900 p`):

| offset | type | meaning |
|---|---|---|
| +0x00 | i32[10] | the saved header: operators `[?, volume 256, pitch env range 1200, 0 x7]`; filters `[type, env amount 128, cutoff, resonance, low/gain, band, high, WS pre-amp 128, WS mix, to next]` |
| +0x28 | f32 | derived (filter: pan/gain into the mix, used by the voice render) |
| +0x38 | ptr[] | articulators (dynamic array; 6, 8 or 2) |
| +0x40 | bytes[0x828] | operator block (saved): i32 freq ratio x 10000 (20000 = 2.0000), i32 freq offset Hz, i32, then the harmonic / shape tables (floats) and the `TSytrusShape` settings |
| +0x868 / +0x869 | u8 | copy of 0x4f6 / 0 |
| +0x86c / +0x870 | f32 | header[2] x 2 x C / header[1] x 1/256 (`FUN_00869e30`: pitch range, volume) |
| +0x878 | ptr | the operator's wavetable (`FUN_00848780`, rebuilt by `FUN_00869d20`) |
| +0x880 | bytes[0x30] | filter block (saved, 0x29 before v9): mode, order, ... |
| +0x8b0 | ptr | filter waveshaper curve (`TCustomMEnvelope` kind 3), +u8 bipolar (`FUN_00781b90`) |
| +0x8cc / +0x8d8 | i32, f64 | `FUN_0086a730`: header[0] x 1/256; freq ratio x base increment / C |
| +0x8e0 | i32 | kind: 0 operator, 1 filter, 2 main |
| +0x8e8 / +0x8ec | i32 | REC base of the part / number of direct REC params (3 operators, 10 filters, 9 for filter 3, 0x18 main) |
| +0x8f8 | ptr | control table for the hints |

## 1. State layout (event 213: `i32 version` + zlib)

`SaveRestoreState` (`FUN_00862d70`):

```
save:    Write(i32 0x00190003)                   // 25 in the high word, 3 the framework tag
         zlib (FUN_0085aeb0 level 2) of the body
restore: Read(i32 v); refuse 0x7fffffff and v >= 0x1a0004; v >= 0x10000: v >>= 16
         v >= 5: inflate (FUN_0085b300); v < 5: the body follows raw
```

Body (`r` = the inflated stream):

```
main      0x3c2 bytes at 0x12c (v < 1: 9 fewer; v < 2: 0xa2 fewer)        // REC 0..19 and the 648-byte region
v > 3:    i32 0x4f0
v > 5:    u8 0x4f4;  v > 6: u8 0x4f5;  v > 12: u8 0x4f6
v > 13:   u8[k] at 0x4f7, k = 1 (v14), 5 (v15..17), 8 (v18), 9 (v19+)
3 x       23 bytes at 0x5f30 + 0x38 e: i32 feedback 64, i32 time 144, i32 time offset 0, i32 volume 64, i32 mode 0, u8 enable 1, u8 serial 0, u8 tempo 0
v >= 10:  reverb 37 (v10) / 38 bytes at 0x5fec: i32 lowcut 75, highcut 40, predelay 0, room 40, diffusion 100, decay 15, high damp 40, color 2, volume 25, u8 enable 0, u8 tempo-based (v11+)
          (v < 10: FUN_00865a90 writes those defaults)
v > 2:    i32 EQ mode 0x61cc; 3 x (12 bytes (v < 14) or 16: level 0, freq, width 17500, type b)      // code defaults freq 5777 / 33145 / 55825, DFM says 8950 / 33145 / 49305
comment   v < 23: i32 len + bytes; v >= 23: u8 len (0xff + i32) + UTF-16 (FUN_0077a970), shown in the info panel
10 parts: i32[10] header (40 bytes)
          operators (p < 6): v < 8: 0xe7 bytes mapped into the block; v < 17: 0x428 bytes; else 0x828 bytes at part+0x40; then FUN_00869d20 / FUN_0086a730 / FUN_00869e30
          filters (p 6..8): 0x29 (v < 9, +0x8ac zeroed) or 0x30 bytes at part+0x880; waveshaper curve (kind 3 stream); v >= 12: u8 bipolar
          main (p 9): header only
          then every articulator in part+0x38: TOpArticulator.LoadFromStream, recompute (FUN_00874740 x2, FUN_00874850)
v > 23:   u8 0x500
v > 24:   u8 0x5f18
```

`TOpArticulator` stream (`FUN_00861180` -> `FUN_0078bb60` -> `FUN_0078b4a0`):

```
i32 format (3 in v24+, 1 in Default.dat)
f32[3] at art+0x20: base, amount, LFO amount ((0, 1, 1) in every default; the same triple as Love Philter's)
8 x TCustomMEnvelope stream in this order: envelope (kind 1), LFO (kind 2), then six kind-3 maps:
    key mapping, velocity mapping, mod X, mod Y, random, unison       (order inferred from the editor's articulator tabs: ENV LFO KEY M VEL M MOD X MOD Y RAND UNISON)
```

Envelope stream grammar (shared with Love Philter, Maximus and
automation-234): `u32 kind; u32 version (3); u32 n; n x (f64 x, f64 y, f32
tension, u8 mode, u8, u8, i8) (f32 x, y when version < 2); u32; i32 active
(-1 on); i32; i32; i32`; kind 1 adds `i32[4]` (the ENV quick knobs attack,
decay, sustain, release: 128 128 0 128), kind 2 adds `i32[4]` (LFO speed
40000, ...) plus an `i32` when version >= 3. Defaults per sub-object in the
corpus: envelope 4 points, LFO 4 points, key / vel / mod X / mod Y / random 2
points, unison 3 points; the maps are active (-1) by default, envelope and
LFO off. "Default subtractive" switches on two envelopes (filter 1 cutoff
and the main volume); "Bajo 303" six.

Preset check (v12 `Default.dat`, `.fst` files name the plugin "Sytrus";
globals = 0x12c..0x137, filter headers as `type env cut res low band high
WSpre WSmix next`):

| preset | v | globals | unison (order pan vol pitch sub phase env) | chorus (order depth speed delay spread cross vol) send | operators (hdr0, ratio x1e4) | filter 1 / 2 / 3 headers | delays / reverb / EQ | reading |
|---|---|---|---|---|---|---|---|---|
| Default.dat | 12 | 88 128 0 | 1 128 100 128 0 16 0 | 4 512 768 8 128 -32 128, send 128 | op1 0 1.0000, op2 0 1.9990, op3 0 0.5010, ops 4..6 0 2.0 | `1 128 142 0 128 0 0 64 128 0` / `3 128 128 256 86 91 0 128 128 0` / `3 128 163 205 0 128 0 128 128 0` | delays on: (64 96 -116 64 1), (64 144 360 84 2), (64 144 0 64 0); reverb on (34 21 70 100 100 33 29 1 42); EQ mode 1, bands (820, 19857), (-460, 29305), (540, 42769) | the factory sound: three operators at 1x, 2x and 0.5x, three filters, all three delays and the reverb on, EQ shaped |
| Default | 19 | 128 128 0 | 1 128 100 128 0 16 0 | 4 512 768 8 128 -32 128 | all 0, ratio 2.0 | three x `1 128 128 0 128 0 0 128 128 0` | delays off (defaults), reverb off, EQ off | the blank template |
| Default subtractive | 23 | 128 128 0 | 1 128 128 128 0 64 0 | same | all 0 | `2 128 65 88 64 0 0 128 128 0` / `1 ...` / `1 ...` | defaults | one low-pass filter (type 2) at cutoff 65, resonance 88, two active envelopes |
| Bajo 303 | 24 | 128 128 0 | 2 128 88 66 0 0 0 | same | op1 hdr0 128 ratio 0.5010, op3 hdr0 29 ratio 0.4980 | `1 196 98 201 128 0 0 192 128 256` / `2 128 115 51 128 0 0 128 128 0` / `1 ...` | reverb on (2059 133 0 40 100 27 21 0 19), EQ on: (-480, 5777), (0, 27769, 22364), (-320, 50961) | two detuned half-ratio operators into filter 1 (env 196, res 201, WS pre 192) that sends fully (256) to filter 2; six envelopes; 2-voice unison |
| Acid | 13 | 115 128 -12 | 1 128 128 190 0 16 0 | same, Mod X 146 Mod Y 388 | op1 ratio 1.0, op2 ratio 0.5 with pitch range 4800 | `1 128 128 204 ...` / `1 128 128 166 ...` / `1 ...` | delay 1 (64 144 92 64 2) on; five envelopes, one LFO | master -12 semitones, unison pitch 190, op 2 pitch envelope spanning 4 octaves |
| Clap | 13 | 159 128 0 | 1 128 128 256 0 16 0 | same, Mod Y 348 | all ratio 2.0 | `3 128 164 122 128 0 0 128 17 0` / `3 128 150 88 ...` / `1 ...` | reverb on, EQ on, four envelopes | two band-pass-ish filters (type 3) with waveshaper mix 17 on the first, +6 dB master |

## 2. Parameter semantics (`ProcessParam` @ 0x00867930, decoder `FUN_00868c90`)

597 REC indices (`param_2 < 0x255`). The decoder walks the part table:

```
i < 20:                      global i (control from form+0x1050 + 8 i)
part p with base_p <= i:     k = i - base_p
    k < count_p:             direct param k of part p (control from part+0x8f8[k])
    i >= 547 + 40:           late global k (k = 40..48 EQ: band = (k - 40) mod 3, field = (k - 40) div 3 (level, freq, width); k = 49: portamento u8 0x4f8)
    else:                    a = (k - count_p) div 8, j = (k - count_p) mod 8: articulator a of part p
                             j < 4: envelope quick knob j (TMEnvelope + 0x98 + 4 j) then FUN_00789de0
                             j >= 4: LFO knob j - 4 (TMLFO + 0xd0 + 4 (j - 4)) then LFO vtable+0x40
bases: operators 20 + 51 p (3 direct + 6 x 8), filters 326, 400, 474 (10 or 9 direct + 8 x 8), main 547 (24 direct + 2 x 8 = 40), late globals 587..596
```

Flags: 0x20 rescales with the control's Min/Max and `DAT_00868188 = 2^-30`,
1 stores, 2 reads, 0x10 refreshes the control when its part is the one shown
(`this+0x508`, `0x514` for the delay unit), 4 formats the hint. Everything
runs under the host lock (vtable +0x100 / +0x108).

Direct parameters (DFM Tag = k; Tag 0 wheels are not serialised in the DFM,
so k = 0 is the untagged wheel of each panel):

| part | k | DFM object | range | default | code |
|---|---|---|---|---|---|
| global | 0, 1, 2 | main volume (Tag 0), `MainLFOVolSlider`, `MainPitchSlider` | 0..?, -128..128, -24..24 | 128, 128 (code), 0 | 0x12c..; `FUN_0086a1d0` |
| global | 3..10 | two groups of four (128, 128, 0, 128) at 0x3c0 | | | stored only |
| global | 11..17 | `FatOrderSelect` 1..9 (1), `FatPanWheel` 0..128 (128), `FatVolWheel` 16..128 (100), `FatPitchWheel` 0..256 (128), `FatSubWheel` 0..96 (0), `FatPhaseWheel` 0..128 (16), `FatEnvWheel` 0..192 (0) | | | 0x3e0 + 4 (k - 11); `FUN_00871fd0` |
| global | 18, 19 | `ModXWheel`, `ModYWheel` | 0..512 | 256 | 0x424 / 0x428; `FUN_0086a3e0` |
| operator | 0 | Tag-0 wheel (output level into the mix, `inferred`: 128 on the sounding operators of Bajo 303) | | 0 | header[0]; `FUN_0086a730`: part+0x8cc = v / 256, part+0x8d8 = ratio x 0xe4 base / C, part+0x8d0 = offset / (0x62e4 x 100) |
| operator | 1 | `VolWheel` | -256..256 | 256 | header[1]; `FUN_00869e30`: part+0x870 = v / 256 |
| operator | 2 | `PEnvRangeWheel` pitch envelope / LFO amount, cents | 0..4800 | 1200 | header[2]; part+0x86c = 2 v x C |
| operator | (not REC) | `RatioSelect` 0..640000 (20000), `FreqSelect` -999999..999999 (0), `ShapeShapeWheel` 0..512, `ShapeTensionWheel` -128..128, `ShapeSkewWheel`, `ShapeSineWheel`, `ShapeNoiseWheel`, `ShapeFilterWheel` 128, `ShapeAbsBtn`, `ShapeHalfBtn`, `ShapeHalfSilentBtn`, `BandLimitBtn`, `CenterBtn`, `PluckModeBtn`, `DeClickBtn`, `FreeRunBtn`, `GlobalPitchBtn`, `OscUnisonBtn` | | | the 0x828 block at part+0x40 (ratio, offset as its first ints); edited through the GUI, rebuilt by `FUN_00869d20` |
| filter | 0 | `FTypeSelect` (Tag -1 in the DFM; corpus 1, 2, 3) | 0..12 | 1 | header[0] |
| filter | 1..9 | `FEnvWheel` 0..256 (128), `FCutWheel` 0..256 (128), `FResWheel` 0..256 (128), `FGainWheel` / `FLowSlider` 0..128 (128 / 0), `FBandSlider` 0..128 (0), `FHighSlider` 0..128 (0), `WSPreAmpWheel` 64..192 (128), `WSMixWheel` 0..128 (0), `ToNextFWheel` -256..256 (0) | | | header[k]; `FUN_00869e80`; k = 9 also `FUN_00865120` (routing) |
| filter | (not REC) | `FFlatBtn`, `FQualityBtn`, `WSEnableBtn`, `WSBipolarBtn`, mode / order selectors | | | the 0x30 block at part+0x880 |
| main | 0 | Tag-0 wheel | | 0 | 0x3fc |
| main | 1..8 | `ChOrderSelect` 0..9 (4), `ChDepthWheel` 0..1024 (512), `ChSpeedWheel` 0..1024 (768), `ChDelayWheel` 0..128 (8), `ChSpreadWheel` 0..128 (128), `ChCrossWheel` -128..128 (-32), `ChVolWheel` -128..128 (128), `FXSendVolWheel` 0..128 (128) | | | 0x3fc + 4 k; bit mask `DAT_008681bc` picks `FUN_0086a130` (chorus) or `FUN_00869a80` |
| main | 9 + 5 e + j | delay unit e (0..2), j: `DelFBWheel` 0..127 (64), `DelTimeWheel` 0..384 (144), `DelTimeOfsWheel` -512..512 (0), `DelVolWheel` -128..128 (64), `DelModeSelect` (0) | | | 0x5f30 + 0x38 e + 4 j; `FUN_00869540` |
| main | (not REC) | `DelEnableBtn`, `DelSerialBtn`, `DelTempoBtn` per unit; reverb `RevEnableBtn`, `RevLCWheel` 19..3000 (75), `RevHCWheel` 5..221 (40), `RevDelWheel` 0..384 (0), `RevRoomWheel` 1..100 (40), `RevDiffWheel` 0..100 (100), `RevDecWheel` 1..200 (15), `RevHDWheel` 5..221 (40), `RevColWheel` 0..4 (2), `RevVolWheel` -128..128 (25), `RevTempoBtn`; `EQModeSelect`, `MixEQType1..3Select`; `DQOverSelect` 0..6 (0), `RQOverSelect` 0..6 (3), `DQEnvBtn`, `RQEnvBtn`, `GlobalCenterBtn`, `ModSmoothBtn`, `MonoKeyBtn`, `RandBtn`, `SoftenWheel` 0..128, `FXSendSelect` -125..125 | | | the three u8 after each delay's ints; the 38-byte reverb block; 0x61cc and the EQ types; the 0x4f0 / 0x4f4.. flags |
| late global | 40..48 | `MixEQLevel1..3Slider` -1800..1800 (0), `MixEQFreq1..3Wheel` 0..65536 (8950, 33145, 49305), `MixEQBW1..3Wheel` 0..65536 (17500) | | | 0x6018 + 0x90 band + 4 field; `FUN_0086a440(band)` |
| late global | 49 (REC 596) | `PortaBtn` | 0, 1 | 0 | u8 0x4f8; 0 also resets 0x62e0 to 0x7fffffff |

Articulator quick knobs (REC `base_p + count_p + 8 a + j`): j = 0..3 the
envelope's attack, decay, sustain, release ints at `TMEnvelope + 0x98` (128
128 0 128), j = 4..7 the LFO's speed, ... at `TMLFO + 0xd0` (40000, 0, 0,
0). Operator articulators a = 0..5 are (by the strings at `0x008642a0`..
and the editor): volume, pitch, phase, oscillator harmonics ("shape"), FM /
RM modulation env, plucked damp (`inferred` order); filter articulators a =
0..7: cutoff, resonance, env amount, low / gain, band, high, WS mix, pan
(`inferred` from Love Philter's eight: panning, volume, cutoff, resonance,
low, band, high, WS mix); main a = 0..1: volume, pitch (Mod X / Mod Y are
globals).

Articulator evaluation per control tick (voice render `FUN_00872290`, read):
`value = base (+0x20)`, then for each map with a table (`art+0x60`,
`art+0x68` for Mod X / Mod Y indexed by the smoothed Mod X / Y 0..512):
mode 0 multiplies, mode 1 adds, else multiplies by `2 m` (`_DAT_008736e0`),
then the envelope and LFO (`vtable+0x38`), exactly Love Philter's
`FUN_008543d0`.

## 3. Signal architecture

`Gen_Render` (`FUN_008737a0`):

```
n' = n * oversampling (0x62f4); dry / wet buffers 0x6298 / 0x62a0 (zeroed), send buffer
silence: countdown 0x6308 against 0x630c while voices exist
for each voice (0x62c0 list): FUN_00872290(voice, n')            // operators, matrix, filters, articulators; adds into dry (and wet for the "send to effects" operators)
arpeggiator voices 0x63a0: advance their counters by n' / tick x +0x98
EQ (0x61e8 / 0x61f0 three-band, FUN_00842100) on dry and wet when 0x62f0 < 0x62ec; the band type selects which buffer (0x61cc == 3 -> wet)
FUN_008736f0 when EQ mode > 1
reverb 0x5f20 (TFruityReverb, FUN_00841060 / kernel +0x60) from wet into a third buffer, added to dry (and to wet when 0x5f28)
FX send: if 0x420 != -1: host.GetInBuffer(0x420 + 1) += wet (after the send EQ 0x61f8) x 0x61e0 gain
delays e = 0..2 (0x5f30 + 0x38 e, object +0x18): FUN_0083d480 taps on L and R of the wet buffer with the per-channel offsets (+0x20 / +0x24); serial / parallel by the unit's flag
chorus (main part params) on the mix; downsample; out
```

Voice renderer (`FUN_00872290`, 522 lines, structure read; the per-tick
operator kernel `FUN_00870820` is 621 lines and was not read):

```
tick (0x62cc samples, 0x14 countdown): Mod X / Y smoothing (0x42c, 0x440), Mod X/Y table indices (+0x78 / +0x7c, 0..512)
for each active part of the voice (list +0x6720): for each of the part's articulators: value = base; maps (Mod X, Mod Y tables) in their mode; envelope / LFO eval (vtable+0x38)
    operators: pitch increment (+0x12 = exp(pitch art x part+0x86c)), level ramp (+0xeb), shape morph (+0xdc, FUN_0086aba0)
    filters: pan / level into the mix (PTR_008fee78 pan law) from main level x part+0x28 (filter level), key tracking (param_2[1]+0x14 x PTR_008ffa98)
    main: output pan / level with the voice's FinalLevels and the unison voice (+0x60 chain of unison sub-voices, each with its own Mod X/Y)
per tick render: FUN_00870820(voice, part, n) per operator (FM / RM reading the matrix levels), then the filters in chain order, then the output pan
```

The FM / RM / output matrix is read inside `FUN_00870820`; its storage is
inside the 648-byte region `0x138..0x3bf` of the saved main block (the only
unexplained saved bytes large enough: 6 operators x (6 FM + 6 RM + F1 + F2 +
OUT + PAN) = 96 ints plus the filter rows fit in 162 ints), `inferred`.

Effects (addresses): reverb `FUN_00841060` (process) / `FUN_00841020`
(idle) on `TFruityReverb` (the same class Harmless uses); delay taps
`FUN_0083d480`; EQ `FUN_00842100` (a three-band shelving/peaking filter,
`MixEQType` low shelf / peaking / high shelf per band); chorus in the main
part recompute `FUN_0086a130`.

## 4. What an implementation needs next

1. `FUN_00870820` (the per-operator tick kernel: FM / RM input summing,
   phase accumulation, the `TSytrusShape` wavetable lookup, plucked mode) and
   the matrix storage. One DecompileAddrs run with callees plus one
   DisasmAddrs for the constants; then read the 648-byte region against
   Default.dat (ratios 1 / 2 / 0.5 with three delays on should show its
   matrix entries).
2. The 0x828 operator block (`TSytrusShape`): `FUN_00848780` (wavetable
   build from shape, tension, skew, sine, noise, pre-filter and the harmonic
   table) and `FUN_00869d20`. One run.
3. The filter block (`FUN_00869e80`, `FUN_00865120`, the 0x30 bytes at
   part+0x880) and the filter kernels: by the Love Philter map (Chocolate,
   Lime, Vanilla, Mango families; `FTypeSelect` 0..12 here against Love
   Philter's 0..11) the designs are shared; one run to confirm the type
   table and the oversampling interplay (`FQualityBtn`).
4. Articulator sub-object order (key, vel, mod X, mod Y, random, unison) and
   the unison sub-voice chain (+0x60): one run on `TOpArticulator`'s
   evaluation (`vtable+0x38`) and `FUN_0086aba0`.
5. The old articulator stream (versions 5..7, 229 shipped presets, no
   corpus state): compare `FUN_0078b4a0` with the version gate inside the
   envelope base loader (`FUN_00781bd0` in Maximus's copy); half a run.
6. Delay units (`FUN_0083d480`, `FUN_00869540`), reverb (`TFruityReverb`,
   shared with Harmless) and the EQ (`FUN_00842100`, `FUN_0086a440`): one
   run for the coefficient maths.
7. Arpeggiator (`0x63a0` list, "Arp priority" string at `0x00868eb0`),
   portamento (`0x4f8`, `0x62e0`), `SoftenWheel`: GUI-adjacent; read with
   `TriggerVoice` (`FUN_0086c7d0`) when voices are implemented.

Estimated: five Ghidra runs for a DSP spec; the state layout above is
complete for versions 8..25 and the parameter index space is fully mapped.

## 5. The fifth run: kernel, routing, shape, articulators

One run (`DecompileAddrs` with callees, output in `ghidra/out/sytrus/kernel/`)
on the two kernels `DAT_00911c28` points at (`FUN_00870580`, and the SSE
twin `FUN_00877270`; the unit init at `FUN_00877790` picks one; found with
`tools/plugstate/ripscan.py` on `0x00911c28`), the voice part init
`FUN_0086af20`, the routing `FUN_0086dd40`, the table assignment
`FUN_0086ace0`, the pitch `FUN_00872210`, the output mix `FUN_00616e20`
and the `TSytrusShape` build (`FUN_008477f0`, `FUN_00847df0`,
`FUN_00847be0`, `FUN_00847900`, `FUN_008471d0`). `FUN_00870820` turned out
to be the per-tick voice render (operators, then filters, then the output
pan), not the per-sample kernel; the kernel is called from it once a
sample over the array of operator states. The constants below were read
from the DLL with `pe.py` (`f` and `g`). Implementation:
`crates/plugins/src/sytrus.rs`.

### The matrix (`FUN_008650d0`, `FUN_00865120`, `FUN_0086dd40`)

```
cell(which, row, col) = i32 at 0x138 + which * 0x144 + row * 0x24 + col * 4   // which 0 FM, 1 RM
                        0 when u8 at 0x44c + which * 0x51 + row * 9 + col != 0   // mute, saved in the main block
rows 0..5 operators, 6..8 filters; cols 0..5 the operator that modulates or feeds the row,
6 pan, 7 send to the effects, 8 output
level(v) = sign(v) * ((11 ^ (|v| / 256) - 1) / 10), 1 at 256          // FUN_00864ff0; the low/band/high sliders use /128 (FUN_00869e80)
part+0x28 = cell(0, row, 6) / 512; part+0x2c = level(cell(0, row, 7)); part+0x30 = level(cell(0, row, 8))
```

So the 648 bytes are the FM block (81 ints) and the RM block (81 ints),
`loader`. A filter row's columns 0..5 are the operators feeding it; filter
`f` feeds only filter `f + 1`, by its to-next wheel (`part+0x8a4 =
level(header[9])`). In `Default.dat` operators 1..3 feed F1 at 194, 256,
256; F1 sends 234 and outputs 256; F2 and F3 output 234 / 256 but nothing
feeds them.

For every operator `c` and carrier `r` with `cell(0, r, c) != 0`, the
voice gets an FM entry `(target = part_r + 0x4c, depth)` with `depth =
level * 4.0` (`DAT_0086e66c`) off the diagonal and `level * 35 / 128`
(`DAT_0086e664 / DAT_0086e668`) on it, in cycles of the carrier per unit
of modulator output (the entry is multiplied by the table size when the
carrier's table is assigned, `FUN_0086ace0`). An RM cell gives `(target =
part_r + 0x44, level, 1 - |level|)`.

### The kernel (`FUN_00870580`), once a sample for every operator state `s`

```
x       = s.phase (f64, table samples) + s.fm_in (+0x4c) * s.mod (+0x364);  s.fm_in = 0
y       = table[floor(x) & mask] linearly interpolated to the next sample          // s.0x58 table, s.0x60 mask
noise:    rnd = rnd * 0x8088405 + 1;  y = rnd * 2^-31 * n - n + y * (1 - n)        // n = level(noise wheel / 128), +0x108 / +0x10c / +0x110
gain    = s.rm (+0x44) + 1 - s.mod;  s.rm = s.mod                                   // RM product gathered by the sources, reset
y      *= gain
for each RM target t of s:  t.rm *= level * y + (1 - |level|)                     // "RM IN * MOD ENV"
out     = y * s.volume (+0x360, ramped by +0x3a0);  buffer[s] = out
for each FM target t of s:  t.fm_in += depth * out                                 // "FM IN * MOD ENV": the carrier scales the sum by its mod
s.phase += s.increment (+0x50 = min((freq * pitch factor + phase delta) * size, size / 2))
```

The operators with inputs or noise (`+5` set) come first in the voice's
list; the plain ones are rendered by `FUN_00870760` a block at a time
(same maths, `inferred`). The plucked path (`+0x138` delay line,
`FUN_0086a810`) was not read.

### The voice part state (0xa38 bytes at `voice + 0x88 + 0xa38 p`)

| offset | meaning |
|---|---|
| +0 | part index; +4 active (reaches the output); +5 has an input or noise |
| +0x08..+0x24 | output L, R, their ramp starts; send L, R, their ramp starts |
| +0x28..+0x30 | the operator's levels into F1, F2, F3 (`level(cell(0, 6 + f, op))`) |
| +0x38 | f64 phase; +0x40 f32 base frequency (`FUN_00872210`); +0x44 RM product; +0x48 pitch factor; +0x4c FM input; +0x50 increment |
| +0x58 / +0x60 / +0x64 / +0x68 | table, mask, size, 1 / size |
| +0x80 | output buffer (`plugin + 0x6208 + 8 (voice.0x30 + op)`) |
| +0x88 / +0x98, +0x90 / +0x9c | FM entries (16 bytes: target, depth) and count; RM entries (target, level, offset) and count |
| +0x108 / +0x10c / +0x110 | noise level x 2^-31, 1 - level, level |
| +0x170 | filter type; +0x174 stages - 1; +0x184 .. +0x189 quality, x2 flags; +0x190 + 0x78 k stage k |
| +0x360 / +0x3a0 | level and its ramp; +0x364 / +0x3a4 modulation (operator) or cutoff (filter); +0x368 frequency x pitch (operator) or resonance (filter); +0x36c .. +0x374 low, band, high gains; +0x378 WS mix |
| +0x3e0 + 0xc8 a | articulator run a: envelope run, LFO run at +0x60, value at +0xc0 |

### Articulators (`FUN_0078b720`, `FUN_0078bbc0`, `FUN_0086af20`)

`TOpArticulator`: +0x08 mode (0 multiplies, 1 adds, else multiplies by
2v), +0x10 `TMEnvelope`, +0x18 `TMLFO`, +0x20 / +0x24 / +0x28 base,
amount, LFO amount, then the six maps +0x30 key, +0x38 velocity, +0x40
Mod X, +0x48 Mod Y, +0x50 random, +0x58 unison; +0x60 / +0x68 the Mod X /
Mod Y maps baked into 513-step tables (`FUN_00874740(a, 0 / 1)`).
`FUN_00861180` streams them in that order after the format and the three
floats: the section 1 order stands, `loader`.

```
static  = key(pitch / 120) ⊕ velocity(v) ⊕ random(r) ⊕ unison(u)        // ⊕ by the mode; FUN_0078bbc0, _DAT_0078bc58 = 1/120
moving  = base + env.out * amount, then LFO: mode 1 += lfo * lfo amount, else *= (lfo * lfo amount * 2 + 1)
lfo     = (level - 0.5) * osc                                            // TMLFO's centre is 0.5 (ctor 0x789fa0); the mode setter 0x861300 leaves it
value   = env or LFO on ? moving ⊕ static : static
value   = value ⊕ modX table[voice.0x78] ⊕ modY table[voice.0x7c]       // every tick
```

Operator articulators: 0 pan (into `part+0x28 + Final.Pan / 2 + 0x3c`,
times 2 minus 1, `ComputeLRVol`), 1 volume (times `header[1] / 256`), 2
modulation (clipped 0..1, scales the FM input and the RM depth), 3 pitch
(`exp(v * header[2] * 2 * ln2 / 1200)`), 4 phase (its change over the tick
is added to the increment), 5 shape morph (`FUN_0086aba0`). Filter
articulators: 0 pan, 1 volume (applied before the filter), 2 cutoff (its
amount is `header[1] / 128`, `FUN_00869e80`), 3 resonance, 4 .. 6 low,
band, high (or gain), 7 waveshaper mix. Main articulators: 0 and 1 are the
pan and the volume of the send path only (`FUN_00872290`, the `0xce2` list
adds `voice + 0x6120` and multiplies by `voice + 0x61e8`).

The two quick ADSR groups at 0x3c0 scale envelope knobs at voice start:
group 0 every volume articulator (index 1), group 1 the cutoff and
resonance articulators of filters 1 and 2 (`DAT_0086c7a0 = 0x0c`,
`DAT_0086c7a1 = 0xc0`). The voice's stage is the least of its envelopes'
after the release; a voice ends when every envelope is over and the output
ramps are down; a voice with no envelope ends with the ramp.

### `TSytrusShape` (the 0x828 block at `part+0x40`)

| offset | meaning |
|---|---|
| +0x40 | i32 ratio: `freq = 2^(pitch / 1200) * ratio * base / 20000 + offset / (100 rate)` (`FUN_0086a730`: `_DAT_0086a804 = 20000`), so the default 20000 is the base pitch |
| +0x44 | i32 offset, 1/100 Hz |
| +0x48 | i32 noise wheel (0..128, `level(/128)`) |
| +0x4c | f32[256] harmonic levels (f32[128] before version 17) |
| +0x44c | f32[256] harmonic phases, in half cycles (x 2 inside the build) |
| +0x84c | i32[5]: shape 0..512, tension -128..128, skew, sine 0..128, filter 0..128 |
| +0x860 | u8[8]: half, half silent, abs, free run, declick (the `0x66d8` list), soften / centre (`0x66e8`), plucked, band limit; +0x868 the GPR copy of 0x4f6 |

`FUN_00847df0` builds `N = DAT_00911b90` samples (plus one wrap sample;
the size was not read, the implementation uses 2048): for every harmonic
with a level, a phase `p` in half cycles runs at `(h + 1) * 2 / N` a sample
from `phase * 2`; skew bends `|p - 1|` with the tension curve `t = (21^(|w|
/ 128) - 1) / 20`, `k = 1e9^t - 1` (`FUN_0060ec90`); the shape function
`FUN_00847be0` of `s = shape / 128` is `sin(pi p)` mixed with a line that
rises over `1 - min(s, 1)` and falls over the rest (sine to triangle to
saw as `s` goes 0 to 1), flattened toward a square by `s - 1` for `s` in
1..2, and a pulse that is 1 while `p < 1 - 0.75 (s - 2)` above 2; the sine
wheel takes `sin(v * sine * 3 pi / 256)` (`_DAT_00848730 = 0.0368155`); the
sum is scaled to peak 1 (`FUN_0062a570`), bent in amplitude by the tension
wheel (`1e18` curve, sign kept), folded by abs, low-passed in the harmonic
domain by the filter wheel (`FUN_00847900`, FFT, cutoff `FUN_00869c90`,
sinc rolloff when the GPR flag is set), and `FUN_008471d0` keeps a
band-limited copy per pitch range (`FUN_0086ace0` picks it by the
frequency). When every shape wheel is at rest the harmonic is added as a
plain sine (`PTR_DAT_008ff980`). All of this `loader` but for the table
size and the exact cutoff of the filter wheel.

### Filters

`header[0]` is the order (`FOrderSelect`: 1..3 stages, 4 and 5 are 2 and 3
with the earlier stages tuned up, `FUN_0086af20`), the type is the i32 at
block +0x2c (`part+0x8ac`), the waveshaper switch the i32 at +0x28, +0x880
and +0x881 the quality and x2 flags. The types follow Love Philter's
numbers: 0 state variable, 1 / 2 ladder low / high, 3..10 Vanilla, 11 comb,
12 phaser (`FUN_00780ce0`, Sytrus's own). `FUN_0086ebe0` (the design, 6037
bytes) was decompiled but not read line by line; the implementation ports
Love Philter's design for types 0..10 and lets a low pass stand in for the
comb and the phaser.

### What the implementation does and leaves out

Reads every version 8..25 state to the last byte (268 project states, 744
preset states across `Data`, 733 of them in Sytrus's own directory; the
presets of versions 5..7, 229 in that directory and 252 across `Data`, are
refused with a `Version` error, their articulator stream is another
format). Of the 1012 states that load, 1000 are heard within the first
2048 frames of three notes; the rest start with a silent attack. Renders operators with
the kernel above over a 2048-sample table without band limiting, the FM
and RM matrix, the three filters for types 0..10, every articulator's
envelope, LFO and six maps (the unison map is read but not used), the pan
law and the output levels, and the send path folded into the output at
half, dry. Left out, each named in `fidelity`: plucked
operators, the waveshaper, the comb and the phaser, the delays, chorus,
reverb and EQ, the shape morph articulator, the harmonics filter wheel,
the arpeggiator and portamento. Every state reports `Approximate`. The 597
REC parameters are the host's (section 6).

## 6. The REC parameters as a host moves them

Read from `ProcessParam` (`0x867930`), its decoder (`0x868c90`) and the
functions it calls after a store; implementation `State::set_param` in
`crates/plugins/src/sytrus.rs`.

Under REC_FromMIDI (flag 0x20) the value becomes `Min + round(v x 2^-30 x
(Max - Min))` with the control's Min (+0x3a8) and Max (+0x3ac): the global
controls from `form+0x1050 + 8 i`, a part's wheels from `part+0x8f8[k]`, the
articulator knobs from `form+0x1260 + 8 j` (the envelope editor's
`EnvAttWheel` .. `LFOPWWheel`, Tags 0..7), the late globals from
`form+0x10d8 + 8 k` and portamento from `form+0xb08`. So a parameter's range
is its wheel's (the DFM ranges in section 2; `FOrderSelect` has six rows,
0..5; `DelModeSelect` three, 0..2; the knobs 0..256 attack, decay and
release, -128..128 sustain, 512..65536 LFO speed, -128..128 tension and
skew, -112..112 pulse width).

What a store does:

```
global 0..2   0x12c + 4 i, then 0x86a1d0: 0x6310 = level(main volume / 128); 0x6314 = level(LFO level / 128)
              with its sign, written into every articulator's LFO amount (+0x28); 0x6318 = main pitch x 100
global 3..10  0x3c0 + 4 (i - 3): stored only; the quick ADSR is applied when a voice starts
global 11..17 0x3e0 + 4 (i - 11), then 0x871fd0 (unison)
global 18, 19 0x424 / 0x428, then 0x86a3e0: 0x6324 / 0x6328 = v / 512, and the snap flag 0x6337 when 0x6334 is clear
late 40..48   0x6018 + 0x90 band + 4 field (band = (k - 40) mod 3, field = (k - 40) div 3), then 0x86a440(band)
late 49       u8 0x4f8 = v > 0; off resets 0x62e0 to 0x7fffffff
operator k    header k (part+4 k); k = 0: 0x86a730 (phase, ratio and offset), else 0x869e30:
              part+0x86c = 2 header[2] x C (pitch range), part+0x870 = header[1] / 256 (volume)
filter k      header k, then 0x869e80: cutoff articulator amount = header[1] / 128 and its LFO amount that
              times 0x6314; +0x884 / +0x888 = header[2] / 256, header[3] / 256 (cutoff, resonance);
              +0x898..+0x8a0 = level(header[4..6] / 128); +0x8c8 / +0x8c4 = header[7] / 128, header[8] / 128
              (waveshaper pre-amp and mix); +0x8a4 = level(header[9] / 256) with its sign; k = 9 also 0x865120 (routing)
main k < 9    0x3fc + 4 k, then 0x86a130 (chorus, the mask at 0x8681bc) or 0x869a80
main k >= 9   delay unit e = (k - 9) / 5: 0x5f0c + 0x38 e + 4 (k - 5 e), then 0x869540
knob j        j < 4: TMEnvelope+0x98 + 4 j, then 0x789de0 (scale j = knob / 128; +0xb8 the sustain scale
              when the loop end is set); j >= 4: TMLFO+0xd0 + 4 (j - 4), then the LFO's vmt+0x40
```

Every store runs under the host lock (vtable +0x100 / +0x108). A read
(flag 2) returns the stored int, the knob from its `TMEnvelope` or `TMLFO`.

The voice side (`0x872290`, read for this):

- Mod X and Mod Y are taken each control tick: with `ModSmoothBtn` (u8
  0x440, saved in the main block, 1 in a new plugin) the voice's value
  (+0x80 / +0x84) moves to the wheel as `wheel + (value - wheel) x c`, else
  it is the wheel; the table place is `round(value + FinalLevels.FCut /
  FRes x 256)` clamped to 0..512. `c` is `0x42c`, set with the sample rate
  (`0x8657b0`): `exp(ln 0.01 x T / (r x 96 x 96))` for a tick of T samples
  and r = rate / 44100, so 1 % is left 0.209 s on. The voice record comes
  from GetMem (`0x416280`) and +0x80 is not set when a voice starts, so FL
  glides from whatever the memory held; here a voice starts at the wheel.
- The main pitch: when `FinalLevels.Pitch + 0x6318` changes, every
  operator's increment is worked out again (`0x872210`), so a note that
  sounds is retuned. The main volume is taken every block (voice+0x48 =
  `FinalLevels.Vol x 0x6310`).

Implemented: every index reads back and stores inside its range; the main
volume, the LFO level, the main pitch, the quick ADSR (on the envelopes it
covers, for the voices that start after it), Mod X and Mod Y with their
glide, the operators' phase, volume and pitch range, every filter wheel
but the waveshaper's pre-amp, the "to next" wheel with the routing, the FX
send level and every articulator knob. Kept but not heard, as the parts
they move are not built: the chorus, the delays, the EQ,
portamento and the waveshaper's pre-amp. FL's saved LFO amounts differ
from `level` by up to a bit of the f32 in some states (0.5633213 against
0.56332123 for 101), and an f32 port of `0x86a1d0` misses others, so the
LFO level is worked out with `level` and the state's own amounts stand
until the host moves it.

## 7. Unison (`TriggerVoice` `0x86c7d0`, `0x871fd0`, `0x86af20`, `0x872290`, `0x870820`)

Read in run 6 (`/root/ghidra/sytrus/out/b1`, the project re-imported on the
cloud machine with the same addresses). The wheels are REC 11..17 at
`0x3e0`: order N (1..9), pan, volume, pitch, sub, phase, envelope. Two
saved bytes go with them: u8 `0x4ff`, the unison type (the last of the
`0x4f7` flags; 1 in most corpus states, 0 in the rest), and u8 `0x4f4`
(saved from version 6), which makes the voices share one set of filters.

`TriggerVoice` makes `N` voices for a note, and `N` more an octave down
when the sub wheel is not 0 (round `d` = 0, 1; voice `e` = 0..N-1), each a
full voice record (`GetMem(0x6728)`). With `c = 1/128` (`0x8ff8c0`):

```
range   = type 0: (float)(exp(ln 11 (p / 256)) - 1) * 5 * ln2/1200      // 0x86d4d0, 0x86d4e0; 0x90e508 = ln2/1200 (0x62e5f0)
          else:   (float)(exp(ln 41 (p / 256)) - 1) * 1.875 * ln2/1200  // 0x86d4e8, 0x86d4f0
g       = N > 1 ? 1 - (3^(1 - 1/N) - 1) / 2 : 1                         // 0x86d4f8 = 3
lev[0]  = g; with a sub wheel s: lev[1] = g s c, lev[0] = g (1 - s c)
shared  = 0x4f4 and (N >= 2 or s >= 1)
per voice (d, e):
  +0x54  = lev[d]
  N == 1: pan +0x3c = 0.5, +0x40 = +0x54, detune +0x4c = 0, pitch +0x44 = 1, phase +0x38 = 0, place 0.5, scale k = c
  else:   +0x38 = (e / N) phase c;  t = e / (N - 1);  x = 2t - 1
          +0x50 = shared ? 0 : x / 256;  +0x3c = +0x50 pan + 0.5
          a = |x|;  +0x58 = (type == 0 or N != 2) ? 2 +0x54 (1 - a) : +0x54
          +0x40 = +0x54 vol c + +0x58 (1 - vol c)
          a = -a for odd e; again for type != 0, odd N and N < 2e
          +0x4c = a T[type][(e + d) mod 9];  +0x44 = exp(+0x4c range)    // T at 0x8fcad8 + 0x24 type
          type 3: +0x38 = frac((phase c)^8 +0x44 100000 (d > 0 ? 0.5 : 1))   // 0x40acf0, 0x86d530
          k = ((|x| - 0.5) env E[(e + d) mod 9] c + 1) c                       // E at 0x8fcb68
          place = t
  d > 0:  +0x44 *= 0.5; the key the maps read drops 12 semitones (not under 0)
  +0x34 = +0x40; the first voice of a shared unison then +0x40 = 1
  role +0x2c: 0 alone, or shared: -1 the first voice (master), 1 the rest (slaves)
```

`T` = 0.877 1.069 0.929 1.181 1.031 1.129 0.887 1.117 0.977 for types 0
and 1, all 1 for type 2, 0.89047 1.07643 0.91158 1.10254 1.03413 1.16686
0.85397 1.20627 1 for type 3. `E` = 0.9877 1.0069 0.9929 1.0181 1.0031
1.0129 0.9887 1.0117 0.9977. Type 0 detunes every even voice up and every
odd one down; the other types also turn the upper half of an odd unison
down.

Where each value goes:

- `+0x44` multiplies the operators' frequency (`0x872210`: `exp(pitch
  ln2/1200) x part+0x8d8 x +0x44 + part+0x8d0`).
- `+0x38` adds to every operator's start phase, with the phase wheel and
  the phase articulator (`0x86af20`).
- `place` is where the articulators' unison maps (sub-object 6) are read,
  combined in the articulator's mode after the key, velocity and random
  maps (`0x86af20` calls `0x7897d0(art+0x58, place)`); `0x78bbc0` reads the
  key map at the dropped key / 120.
- `k` scales the voice's copy of the attack and decay knobs of the
  envelopes under the quick ADSR, unless the envelope is global (flag 2):
  attack `x ADSR.att x k`, decay `x ADSR.dec x (1/64 - k)` (`0x86c748`), so
  `k = 1/128` leaves them.
- `+0x3c` replaces the 0.5 in the output pan, `(FinalPan / 2 + +0x3c +
  part+0x28 + pan art) x 2 - 1`, and `+0x40` multiplies the output gain
  (`0x872290`).

Alone (`0x2c = 0`) every voice runs on its own: articulators, operators,
filters, output. Shared, the master's render (`0x872290`) ticks each
slave's articulators and calls `0x870820` for every slave in its `+0x60`
chain before its own: the second voice (`+0x30 = 0`) writes its operators'
outputs times `+0x34` into the operator buffers, the others and the master
(`+0x30 = 6`) render into the buffers past them and add those times
`+0x34`, and only the master runs the filters and the output on the sum. A
slave goes when its master does.

When a pan, volume or pitch wheel moves, `0x871fd0` works out `+0x3c` (for
a voice alone), `+0x40`, `+0x34` and `+0x44` again for every voice and
forces the pitch to be taken again (`+0x18 = 2^31`). Order, sub, phase and
envelope wait for the next note.

Implemented as read (`Unison` in `sytrus.rs`). Not built: the velocity
flags `0x4fd` (attack by velocity, `0x86af20`; 0 in every corpus state),
global envelopes shared between voices (flag 2), portamento.

## Corrections

- Operator `header[0]` is the phase wheel, not an output level: the voice
  starts at `header[0] / 256` cycles (`FUN_0086af20`, `part+0x8cc`). Bajo
  303's 128 is half a cycle. Output levels are the matrix's column 8.
- Filter `header[0]` is the order (`FOrderSelect`), not the type; the type
  is at `part+0x8ac`. Read the preset table's filter headers as `order env
  cut res low band high WSpre WSmix next`. The corpus orders are 1, 2, 3.
- The main part's two articulators are the pan and the volume of the send
  path, not volume and pitch. "Default subtractive" switches on filter 1's
  volume and cutoff envelopes, not a main volume envelope.
- Operator articulators are pan, volume, modulation, pitch, phase, shape
  (section 2 had volume, pitch, phase, shape, modulation, damp).
- `FUN_00870820` is the per-tick voice render; the per-sample kernel is
  `FUN_00870580` (or `FUN_00877270`) through `DAT_00911c28`.
- The default ratio wheel's 20000 is the base pitch, not twice it: the
  ratio is divided by 20000, so Default.dat's op1 at 10000 plays an octave
  under the base.
- The saved main block's last 162 bytes (0x44c..0x4ee) are the matrix mute
  flags, which section 1 did not name.
