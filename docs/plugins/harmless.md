# Harmless: state, parameters, signal architecture

Recovered from `Harmless_x64.dll` (FL Studio 2026, Delphi native plugin,
image base 0x400000, no engine DLL beside it; `Harmless/Data/Compression/
1..4.fst` are Maximus states). Ghidra project `ghidra/proj`, project name
`harmless`; decompiles under `ghidra/out/harmless/` (`vmt/` the SDK slots
with callees, `voice/` the voice renderer, post mix and effects). Corpus: 160
states in the 165 `.flp` (102 of 437 bytes, 35 of 4629, 16 of 436, 7 of
4628), 433 shipped presets in 430 `.fst`. Three Ghidra runs (import, slots,
voice). The `SynthEditorForm` wheel table from `tools/plugstate/dfm.py` is at
`ghidra/out/harmless/params_dfm.tsv` (Tag = REC index).

The plugin is an additive synthesiser: each voice is a bank of harmonics
whose levels come from a 524-entry harmonic image shaped by a 12-step mask,
six timbre EQ bands, sub-harmonics and a "protection" roll-off; the filter
is applied to the harmonic levels, not to the audio, so it is an image-domain
filter; then come pluck and envelopes per voice, unison (up to 9 voices), and
a post chain of phaser, harmonizer, chorus, delay, reverb and an embedded
Maximus for compression. State is a flat block of 100 ints plus seven
selector ints, an optional pair of 524-float harmonic arrays and one flag.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `inferred`.

## How the plugin is wired

`CreatePlugInstance` @ `0x00779380` -> class `TTestPlug`, VMT `0x007791d0`
(instance size 0x828, parent `TDelphiFruityPlug` `0x0073ffa8`).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x007793e0 |
| 2 | Idle_Public | 0x00743a70 |
| 3 | SaveRestoreState | 0x0077adb0 |
| 4 | GetName | 0x00782460 |
| 5 | ProcessEvent | 0x0077f5e0 |
| 6 | ProcessParam | 0x0077f630 |
| 7 | Eff_Render | abstract |
| 8 | Gen_Render | 0x007886b0 |
| 9 | TriggerVoice | 0x00782940 |
| 10 | Voice_Release | 0x00783910 |
| 11 | Voice_Kill | 0x00783b90 |
| 12 | Voice_ProcessEvent | 0x00783c40 |
| 13 | Voice_Render | abstract (the plugin renders its voices in Gen_Render) |

Classes: `TFruityReverb` VMT `0x007494d0`, `TDelay_32FS` `0x00746f90`,
`TDelayLine` `0x0075a3f8`, `TFilterLine` `0x0075a518`, `TFPFPlug`
`0x0076e720` (the embedded Maximus host), `TOldPluginAdapter` `0x00761280`.

Plugin object fields the state and render touch:

| offset | type | meaning |
|---|---|---|
| 0x128 | i32[100] | the parameters, REC index = (offset - 0x128) / 4 = DFM Tag |
| 0x1bc / 0x1d8 | i32 | mono mode / portamento (voice allocation in TriggerVoice) |
| 0x21c | i32 | param 61 (`FResCutOfs`), rescaled on load of v < 6 |
| 0x280 | i32 | param 86 (`LFOShapeSelect`): values > 9 shift by one when loading v < 5 |
| 0x2b4 | i32 | param 99 (`VelToFatPitchBox`), added in v9 |
| 0x2b8 | ptr | voice list (`+0x10` count, `+8` items) |
| 0x2e8 | u8 | saved selector 6 (a checkbox, see below) |
| 0x300 | ptr | render buffer set (`[0]+0x18` voice sum, `+0x20` scratch, `+0x28` scratch 2, `+0x38` output) |
| 0x320 | i32 | saved selector 7 (pushed to form+0xa28) |
| 0x330 | i64 | sample counter (global LFO phase) |
| 0x338 | ptr | last triggered voice (legato) |
| 0x580 | f32[3] | sub harmonic levels (params 2..4 / 2048) |
| 0x588 | ptr | harmonic image: `n` floats levels then `n` floats (second array), n = 524 (`param_1[0xb1]`), null when untouched |
| 0x5a8 / 0x5a9 | u8 | timbre recompute pending (`FUN_00787b40`) / mask recompute pending (`FUN_00787ce0`) |
| 0x5b8 | f32 | param 0 as sign(v) (v/1000)^2 |
| 0x5d8..0x5e8 | f64 | envelope rates (attack 0x5d8, decay 0x5e0, release 0x5e8) |
| 0x5fc | f32 | protection gain `level(p1/1024)` |
| 0x614 / 0x618 | f32 | harmonic mask mix p23/1000, LFO amount p24/1000 |
| 0x61c | f32[6] | timbre EQ gains `level((p + 1024)/2048)` |
| 0x790 | f32[12] | harmonic mask levels sign(p) level(|p|/1000) |
| 0x798 | u8 | v12 flag byte (saved); 0 on load triggers a form update and host message 2 |
| 0x79c / 0x7a4 | f32 | tremolo depth p25/1000 / rate |
| 0x7c0 | ptr | phaser (`FUN_0074b0f0` process, `+0x60` kernel) |
| 0x7d0 | ptr | delay (`TDelay_32FS`, two `FUN_00747510` taps with offsets 0x7dc / 0x7e0, type 0x7d8) |
| 0x7f0 | ptr | reverb (`TFruityReverb`, `FUN_0074a540`), wet 0x7f8 |
| 0x800 | ptr | embedded effect list (`+0x128` list, each item `+0x10` a plugin with Eff_Render at vtable+0x38): the compression Maximus |

## 1. State layout (event 213: 437 bytes; 4629 with the harmonic image; 428 and 436 in older files)

`SaveRestoreState` (`FUN_0077adb0`):

```
save:    Write(i32 12); Write(this+0x128, 400)                   // 100 params
         Write(7 x i32: form+0x6f8 value, form+0x700, form+0x770, form+0x9f8, form+0x778, u8 this+0x2e8 as i32, this+0x320)
         Write(i32 n) with n = 0 or 0x20c (524); if n: Write(image, n * 8)
         Write(u8 this+0x798)
restore: Read(i32 v); v < 2: nothing
         Read(0x128, v < 9 ? 396 : 400); v < 9 zeroes param 99
         v < 5 and param 86 > 9: param 86 += 1          // an LFO shape was inserted at index 10
         v < 6: param 61 = round(param 61 * DAT_0077b2b8)
         v > 3: Read(7 ints); v < 5 and selector 4 == 3: selector 4 = 4      // a compression type inserted at 3
         v > 7: Read(i32 n); n > 0: alloc n*8, Read; v < 10: memmove second array from +n*4 to +(n+4)*4 (old layout had 4 fewer floats before it)
         v > 11: Read(u8 0x798)
         then FUN_00787990 (image rebuild), GetValue/UpdateValue loop over 100 params; v < 11 with 0x1e4 == 0: param 58 recomputed from 0x210 / (0x6b8 - 0x6b4)
```

Sizes: v4 428 (4 + 396 + 28), v9..v11 436 (4 + 400 + 28 + 4), v12 437,
plus 4192 when the image is present (524 x 2 x f32). Corpus: 437 x102 (v12),
4629 x35, 436 x16 (v9..11), 4628 x7; presets v4 (10), v9 (157), v10 (22),
v11 (137), v12 (267 across both sets).

| byte | field | content | confidence |
|---|---|---|---|
| 0..3 | version | 12 | loader, corpus |
| 4 + 4 t | param t (t = 0..99, table in section 2) | | loader, dfm, corpus |
| 404 | selector 0: filter type (`FilterTypeSelect`, form+0x6f8) | 0..16 seen (17 values) | loader, dfm order, corpus |
| 408 | selector 1: resonance type (`ResTypeSelect`, form+0x700) | 0..8 | loader, dfm order, corpus |
| 412 | selector 2: phaser type (`PhaserTypeSelect`, form+0x770) | 0..9 | loader, dfm order, corpus |
| 416 | selector 3: compression type (`CompTypeSelect`, form+0x9f8) | 0 off, 1..4 = `Data/Compression/N.fst` | loader, dfm order, corpus, preset |
| 420 | selector 4: timbre color (`HarmColSelect`, form+0x778) | 0..11 | loader, dfm order, corpus |
| 424 | selector 5: u8 this+0x2e8 as i32 (pushed to form+0x7d8 as a checkbox: `QualitySelect` or `ThreadedBox`) | 0, 1 | loader, inferred |
| 428 | selector 6: this+0x320 (pushed to form+0xa28) | 0, 1 | loader |
| 432 | n | 0 or 524 | loader, corpus |
| 436.. | n x f32 harmonic levels, then n x f32 (second array; phases or the drawn mask, `inferred`) | levels in 3e-5..1.0 | loader, corpus |
| last | u8 this+0x798 (v12) | 1 | loader, corpus |

The selector-to-form mapping follows the DFM declaration order of the combos
(`FilterTypeSelect` line 68, `ResTypeSelect` 101, `PhaserTypeSelect` 607,
`HarmColSelect` 639, `CompTypeSelect` 3298) against the ascending form field
offsets 0x6f8, 0x700, 0x770, 0x778, 0x9f8; the corpus value ranges agree
(filter 17 values, compression 0..4, color 0..11).

Preset check (`.fst` files name the plugin "Harmless"; t0 is the untagged
first wheel):

| preset | v | t0 / protect / vol / pitch | mask 1..12 | filter cut / width / res | unison order / type | LFO shape / speed | fx: phaser mix, chorus type/mix, delay type, reverb type/wet, comp vol | selectors | image |
|---|---|---|---|---|---|---|---|---|---|
| Default | 10 | 1000 / 256 / 1000 / 0 | 1000 0 1000 0 ... (odd harmonics) | 4096 / 500 / 0 | 1 / 2 | 0 / 3000 | 0, 0/500, 0, 0/500, 500 | filter 4, comp 0, color 0, 1, 0 | none |
| Bumper | 12 | 0 / 0 / 900 / -2400 | odd | 1280 / 572 / 214 | 3 / 1 | 7 / -6414 | 0, 0/500, 0, 6/182, 596 | filter 0, res 5, comp 2 | none |
| Getta | 12 | 1000 / 256 / 656 / -2400 | odd | 1472 / 672 / 0 | 1 / 2 | 0 / 3000 | 0, 1/101, 0, 0/500, 305 | comp 3, color 11 | 524 |
| Bassline GOL | 10 | 0 / 456 / 1149 / -1200 | odd | 1728 / 316 / 1000 | 1 / 2 | 0 / 3000 | 0, 0/500, 2, 0/500, 500 | filter 16, res 2, phaser 3, comp 1 | none |
| 8 Bit Wonderland FG | 9 | 0 / 0 / 571 / -1200 | 1000 0 1000 212 1000 150 1000 0 1000 0 1000 412 | 8192 / 0 / 1000 | 3 / 0 | 5 / 2320 | 1000, 8/224, 2, 4/446, 239 | phaser 9, comp 1, color 11 | 524 |
| Additivesphere FG | 9 | -1000 / 0 / 591 / -1200, detune 2400 | all 1000 | 864 / 760 / 807 | 3 / 2 | 8 / 2000 | 1000, 4/182, 2, 3/665, 200 | filter 11, res 6, phaser 8, comp 2, color 11 | 524 |
| Bit Wobbly AG | 9 | 0 / 0 / 1000 / 0, sub1 1072, mask mix 267 | 0 0 1000 0 599 0 1000 0 1000 237 537 450 | 3072 / 500 / 744 | 2 / 2 | 7 / 0 | 0, 0/500, 0, 1/350, 500 | filter 8, color 6 | none |
| Filter decay | 4 | 0 / 256 / 1000 / 0 | odd | 0 / 500 / 0 | 1 / 2 | 0 / 3000 | defaults | (no selectors in v4 beyond the 7 ints) | n/a |

Every one of the 593 blocks decodes with this table; the `Default.fst`
harmonic mask is the odd-harmonic pattern (1000 on 1, 3, 5, ... of the 12
steps), and the timbre image is only written when a preset edited it (42
presets, 42 corpus states).

## 2. Parameter semantics (`ProcessParam` @ 0x0077f630)

Indices 0..99 store at `0x128 + 4 i` (REC flag 1) under the host lock, read
back with flag 2, rescale with 0x20 using the control's Min/Max and
`DAT_00781068 = 2^-30`. `FUN_00408930` is `exp`, `FUN_00409b50` is `ln`,
`PTR_007ef3c0 = 1/1000`, `PTR_007f0758 = 1/2048`, `PTR_007ef530 = ln 11`,
`PTR_007f0620 = 1/10`, `DAT_00781080 = 1.0`. The shared level curve is the
Delphi plugin helper's `level(x) = (11^x - 1) / 10`.

| REC | DFM object | range | default | code (`this` offsets) | confidence |
|---|---|---|---|---|---|
| 0 | first timbre wheel (Tag 0, not serialised in the DFM) | -1000..1000 | 1000 | `0x5b8 = sign(v) (v/1000)^2` | loader, corpus |
| 1 | `SafetyVolSlider` low harmonics protection | 0..1024 | 256 | `0x5fc = level(v/2048)` | loader, dfm |
| 2..4 | `Sub1Slider`, `Sub3Slider`, `Sub4Slider` | 0..2048 | 0 | `0x580[k-1] = v/2048` | loader, dfm |
| 5..10 | `TEQ1Slider`.. timbre EQ 1..6 | -1024..1024 | 0 | `0x61c[k-5] = level((v + 1024)/2048)` (`PTR_007f07a8`, `PTR_007f04b8` the exp base and scale); sets 0x5a8 | loader, dfm |
| 11..22 | `HarmMask1Slider`.. harmonic mask 1..12 | 0..1000 | 0 (Default.fst: odd steps 1000) | `0x790[k-11] = sign(v) level(|v|/1000)` | loader, dfm, preset |
| 23 | `HMMixWheel` mask mix | 0..1000 | 500 | `0x614 = v/1000` | loader, dfm |
| 24 | `HMMixLFOAmtWheel` | -1000..1000 | 0 | `0x618 = v/1000` | loader, dfm |
| 25 | `TremAmtWheel` stereo tremolo depth | 0..1000 | 0 | `0x79c = v/1000` | loader, dfm |
| 26 | `VolEnvAttWheel` | 0..1000 | 0 | `0x5d8 = SR * ((e^(v/1000 ln C_c8) - 1) * C_d0)` rate words (`DAT_007810c8`, `_DAT_007810d0`) | loader, dfm |
| 27 | `VolEnvDecWheel` | 0..1000 | 500 | v < 1000: `0x5e0 = SR * ((e^(v/1000 ln C_d8) - 1) * C_e0 + C_d0)`; 1000: -4.0 (sustain forever) | loader, dfm |
| 28 | `VolEnvRelWheel` | 0..1000 | 250 | `0x5e8 = SR * ((...C_c8...) * C_e8 + C_f0)` | loader, dfm |
| 29, 30 | `VelToAttBox`, `VelToRelBox` | 0, 1 | 0 | flags | dfm |
| 31 | `PluckDecWheel` | 0..1000 | 500 | v < 1000: `0x600 = (e^(v/1000 ln C_d8) - 1) * C_108` else -4.0; then FUN_00783c90 | loader, dfm |
| 32..35 | `VelToPluckDecBox`, `PluckAltBox`, `PluckMaskedBox`, `PluckRelBox` | 0, 1 | 0 | 33, 34: FUN_00783c90 (pluck table rebuild) | loader, dfm |
| 36 | `MainVolSlider` | 0..2000 | 1000 | case 0x24 not read this pass (the shared `level()` curve by analogy with the other volumes, `inferred`) | dfm |
| 37 | `VelToVolBox` | 0, 1 | 1 | flag | dfm |
| 38 | `MainPitchWheel` cents | -2400..2400 | 0 | `0x2dc = v - 1200` (semitone offset base) | loader, dfm |
| 39 | `DetuneWheel` cents | -2400..2400 | 0 | sets 0x5a9 (mask/timbre recompute) | loader, dfm |
| 40 | `TGrittinessWheel` | -1000..1000 | 0 | `0x5ac = sign(v) (e^(|v|/1000 ln C_140) - 1) * C_148`; FUN_00783c50 | loader, dfm |
| 41 | `VibratoWheel` | 0..1000 | 0 | `0x31c = v * C_070` | loader, dfm |
| 42 | `PitchLFOAmtWheel` | -1000..1000 | 0 | `0x2e0 = sign(v) (e^(|v|/1000 ln C_c8) - 1) * C_138` | loader, dfm |
| 43, 44 | `PortaBox`, `MonoBox` | 0, 1 | 0 | 43 > 0: `FUN_00788030` (portamento setup); the voice allocator reads 0x1d8 / 0x1bc | loader, dfm |
| 45 | `PortaTimeWheel` | -1000..1000 | 0 | `0x548 = sign(v) SR (e^(|v|/1000 ln C_f8) - 1) / (0x2c8 * C_100)` | loader, dfm |
| 46 | `VelToPortaTimeBox` | 0, 1 | 0 | | dfm |
| 47 | `FEnvAltBox` adaptive filter envelope | 0, 1 | 0 | `0x648 = v > 0`; re-sends params 52 and 58 | loader, dfm |
| 48, 49 | `FEnvAttWheel`, `FEnvDecWheel` | 0..1000 | 0, 500 | v < 1000: `0x638 / 0x640 = 1 / (SR (e^(v/1000 ln C_d8) - 1) C_110 + 1)` else 0 (hold) | loader, dfm |
| 50, 51 | `FEnvAttSlopeWheel`, `FEnvDecSlopeWheel` | -1000..1000 | 0 | tension through `FUN_005fd4d0(this+0x700 / +0x70c)` with `e^(|v|/1000 ln C_120)` | loader, dfm |
| 52 | `FEnvAmtWheel` | -1000..1000 | 0 | `0x6a4 = v/1000` (times the key range unless adaptive) | loader, dfm |
| 53 | `VelToFEnvAmtBox` | 0, 1 | 0 | | dfm |
| 54 | `FCutWheel` filter frequency | 0..8192 | 4096 | `0x6a4`-family: `v/8192` mapped by the Hz slope (param 55) into the image index | loader, dfm |
| 55 | `FHzSlopeSelect` filter scale | 0, 1 | 0 | linear / log harmonic index | dfm |
| 56 | `FWidthWheel` | 0..1000 | 500 | | dfm |
| 57 | `FKbTrackWheel` key to frequency | -2000..2000 | 1000 | | dfm |
| 58 | `FLFOAmtWheel` | -1000..1000 | 0 | | dfm |
| 59 | `FResWheel` resonance amount | 0..1000 | 0 | | dfm |
| 60 | `ResHzSlopeSelect` | 0, 1 | 0 | | dfm |
| 61 | `FResCutOfsWheel` resonance offset / noise length | -2400..2400 | 0 | rescaled by `DAT_0077b2b8` when loading v < 6 | loader, dfm |
| 62 | `FOscWheel` self-oscillation | 0..2048 | 0 | | dfm |
| 63, 64 | `FResAdaptiveBox`, `FResMTBox` | 0, 1 | 0 | | dfm |
| 65 | `FatOrderSelect` unison order | 1..9 | 1 | voice copies in the renderer; setter not read | dfm |
| 66..69 | `FatPanWheel`, `FatPitchWheel`, `FatPhaseWheel`, `FatVarWheel` | 0..128, 0..256, 0..128, 0..128 | 128, 128, 64, 64 | | dfm |
| 70 | `FatTypeSelect` unison type | 0..2 | 0 (Default.fst 2) | | dfm, preset |
| 71 | `PMixWheel` phaser mix | 0..1000 | 0 | | dfm |
| 72 | `PHzSlopeSelect` | 0..2 | 0 | | dfm |
| 73 | `PWidthWheel` | 0..10000 | 5000 | | dfm |
| 74 | `PWidthLFOAmtWheel` | -1000..1000 | 0 | `0x76c = v * DAT_00781130` | loader, dfm |
| 75 | `POfsWheel` | 0..999 | 500 | | dfm |
| 76 | `POfsLFOAmtWheel` | -1000..1000 | 0 | | dfm |
| 77 | `PMotionSpeedWheel` | -8000..8000 | 3000 | | dfm |
| 78 | `PKbTrackWheel` | -2000..2000 | 1000 | | dfm |
| 79 | `SFXMixWheel` harmonizer mix | 0..1000 | 0 | | dfm |
| 80 | `SFXWidthWheel` | 0..1000 | 500 | | dfm |
| 81 | `SFXWidthLFOAmtWheel` | -1000..1000 | 0 | | dfm |
| 82 | `VelToSFXWidthBox` | 0, 1 | 0 | | dfm |
| 83 | `SFXStrengthWheel` | 0..1024 | 0 (corpus 256) | | dfm, corpus |
| 84, 85 | `SFXTypeSelect`, `SFXPosSelect` harmonizer type / position | 0..4, 0..3 | 0 | | dfm, corpus |
| 86 | `LFOShapeSelect` shape / source | 0..13 | 0 | index shifted on load of v < 5 | loader, dfm |
| 87 | `GlobalLFOBox` | 0, 1 | 1 | | dfm |
| 88 | `LFOAttWheel` | 0..1000 | 0 | | dfm |
| 89 | `LFOSpeedWheel` | -8000..8000 | 3000 | negative = tempo-synced (corpus -6414) | dfm, corpus |
| 90 | `ChTypeSelect` chorus type | 0..8 | 0 | | dfm, corpus |
| 91 | `ChMixWheel` | 0..1000 | 500 | | dfm |
| 92 | `DelTypeSelect` delay type | 0..3 | 0 | 0x7d8 selects the tap kernel | dfm, corpus |
| 93..95 | `DelVolWheel`, `DelFBWheel`, `DelTimeWheel` | 0..1000, 0..900, 0..384 | 500, 300, 144 | time in 1/48 beat steps (`inferred` from 144 = 3 beats... 3/4 beat) | dfm |
| 96 | `RevTypeSelect` | 0..6 | 0 | | dfm, corpus |
| 97 | `RevWetWheel` | 0..1000 | 500 | 0x7f8 | dfm |
| 98 | `CompVolWheel` compression level | 0..1000 | 500 | input gain into the embedded Maximus | dfm |
| 99 | `VelToFatPitchBox` | 0, 1 | 0 | (v9+) | loader, dfm |

Params 54..64 and 71..97 are routed in `ProcessParam` to the filter, phaser,
chorus, delay and reverb setters whose bodies were not read this pass (the
switch cases 0x36..0x62); their ranges and defaults come from the DFM.

## 3. Signal architecture

`Gen_Render` (`FUN_007886b0`):

```
n = Length; buffers = this+0x300 (set 0); sleep countdown 0x7b8 with hold 0x7bc while there are voices
stereo tremolo (0x79c / 0x7a0): a sine at 0x7a4 over the sample counter 0x330 into a modulation buffer, smoothed by FUN_00604a90
pending recomputes: 0x5a8 -> FUN_00787b40 (timbre EQ into the harmonic image); 0x5a9 -> FUN_00787ce0 (mask)
legato: last voice 0x338 -> 0x6dc pitch target
voices: single-threaded: for each voice FUN_00784a10(voice) then FUN_00788620 (sum)
        threaded (0x2f8 != 0, >= 2 voices): a worker pool renders voices in parallel, then the sum
voice cleanup: finished voices (state +0x11c == 10, refcount +0xa8 == 0) -> Host.Voice_Kill
phaser  0x7c0: FUN_0074b0f0 then kernel +0x60; mix 0x7c8 when != 1.0
delay   0x7d0: two taps (FUN_00747510) with per-channel offsets 0x7dc / 0x7e0, feedback kernel +0x40 or +0x60 by type 0x7d8, gain 0x7e4
reverb  0x7f0: FUN_0074a540 (TFruityReverb) into scratch 2, added with wet 0x7f8
embedded effects list 0x800+0x128: each plugin's Eff_Render(out, out, n)   // the compression Maximus loaded from Data/Compression/<CompType>.fst
global LFO phase 0x750 / 0x558, pitch glide 0x310 advance; host msg 0x128 pre/post; copy to Dest
```

Voice renderer (`FUN_00784a10`, 874 lines, read for structure only):

```
voice record: +0x2c tick countdown (0x2c8 = control tick), +0x194 envelope position, +0x34 tension record, +0x18c smoothed level
per tick:  Host.GetVoiceLevels (vtable+0xf8) into the voice's FinalLevels (+0x134 pitch, +0x138 vol...)
           envelope: FUN_00408930 (exp) tension shaping of +0x194 against the (a, b) pair at +0x34 (same as 3x Osc's articulation unit)
           pitch = FinalLevels.Pitch + 0x2dc (master pitch) + voice offset (+0x58), vibrato 0x31c * LFO
           harmonic image: FUN_0077c540(image, idx, frac, n) resamples the 524-entry level array for the voice's harmonic positions
                           (linear interpolation, DAT_0077c680 = 1.0), FUN_00784220 applies the protection / sub (mode table DAT_007ee634 + 0x10 * type)
           the filter is applied to the image (cut 54, width 56, res 59 index into the harmonic array), not to audio
per block: harmonic bank -> voice buffer (the sine bank kernels behind PTR_007f0xxx: add-with-gain, zero, copy), then
           unison copies (order 65) with pan/pitch/phase spread, pluck (FUN_00783c90 table), release fade
output:    envelope segments (attack, decay, release by state +0x11c: 0..2 attack/decay, 3.. release, 10 done) applied through
           FUN_005f5f60 ramps; mix into set[0]+0x20 with the voice gain, then FUN_005b82c0 stereo spread
```

The `TriggerVoice` (`FUN_00782940`) handles the FL "slide" and "porta" note
kinds (`local_74 == 0xf` slide, `0xd` porta), mono mode (`0x1bc`) and
legato (`0x1d8`, voice table at `0x340 + 0x20 k`).

## 4. What an implementation needs next

1. The harmonic image build: `FUN_00787990` (image rebuild on load),
   `FUN_00787b40` (timbre EQ), `FUN_00787ce0` (mask), `FUN_00783c90` (pluck)
   and `FUN_00783c50`. They decide what the 524 floats mean (levels per
   harmonic plus a second array). One DecompileAddrs run with callees.
2. The sine bank: the `PTR_007f0xxx` kernels in the voice render are
   CPU-dispatched; one DisasmAddrs run on the x87 candidates after a
   DecompileCallers on their dispatch table, as for Love Philter.
3. The filter-on-image maths (params 54..64) and the `FHzSlope` mapping:
   read the switch cases 0x36..0x40 of `ProcessParam` plus their setters.
   One run.
4. Phaser (`FUN_0074b0f0`), chorus, delay (`FUN_00747510`, `TDelay_32FS`)
   and reverb (`TFruityReverb`, shared with Sytrus's `Reverb_x64.dll`-era
   code): one run for the coefficient setters (ProcessParam cases 0x47..0x62).
5. Unison (params 65..70) and the `FatType` kernels: in the voice render; one
   DisasmAddrs on the three pan/pitch spread loops.
6. The embedded Maximus: Harmless loads `Data/Compression/N.fst` through
   `TFPFPlug`; an implementation can reuse the Maximus map, feeding it the
   compression level (param 98) as input gain. No run.

Estimated: four Ghidra runs for a full DSP spec; the state and parameter map
above is complete.
