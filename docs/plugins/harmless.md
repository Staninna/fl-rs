# Harmless: state, parameters, signal architecture

Recovered from `Harmless_x64.dll` (FL Studio 2026, Delphi native plugin,
image base 0x400000, no engine DLL beside it; `Harmless/Data/Compression/
1..4.fst` are Maximus states). Ghidra project `ghidra/proj`, project name
`harmless`; decompiles under `ghidra/out/harmless/` (`vmt/` the SDK slots
with callees, `voice/` the voice renderer, post mix and effects). Corpus: 160
states in the 165 `.flp` (102 of 437 bytes, 35 of 4629, 16 of 436, 7 of
4628), 433 shipped presets in 430 `.fst`. Six Ghidra runs (import, slots,
voice, then `run4`, `run5` and `run6` for the image build, the shape curves,
the effect setters and the bank kernels). The `SynthEditorForm` wheel table
from `tools/plugstate/dfm.py` is at `ghidra/out/harmless/params_dfm.tsv` (Tag
= REC index). The implementation is `crates/plugins/src/harmless.rs`, on the
bank and curve blocks of `crates/dsp/src/additive.rs`.

The plugin is an additive synthesiser: each voice is a bank of 528 sines
(the filter's self-oscillation, three sub partials and 524 harmonics) whose
levels come from a timbre colour shaped by six timbre EQ bands and a 12-step
mask. The filter, the resonance and the phaser are curves laid over those
levels by each partial's frequency, not filters on audio. Then come the
pluck, the harmonizer, the low-harmonic protection and the sub levels,
envelopes per voice, unison (up to 9 voices), and a post chain of stereo
tremolo, chorus, delay, reverb and an embedded Maximus for compression. State is a flat block of 100 ints plus seven
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
| 0x1bc / 0x1d4 / 0x1d8 | i32 | param 37 `VelToVol`, param 43 `PortaBox`, param 44 `MonoBox` (TriggerVoice reads all three; 0x1d8 is the legato flag, section 3) |
| 0x21c | i32 | param 61 (`FResCutOfs`), rescaled on load of v < 6 |
| 0x280 | i32 | param 86 (`LFOShapeSelect`): values > 9 shift by one when loading v < 5 |
| 0x2b4 | i32 | param 99 (`VelToFatPitchBox`), added in v9 |
| 0x2b8 | ptr | voice list (`+0x10` count, `+8` items) |
| 0x2e8 | u8 | saved selector 6 (a checkbox, see below) |
| 0x300 | ptr | render buffer set (`[0]+0x18` voice sum, `+0x20` scratch, `+0x28` scratch 2, `+0x38` output) |
| 0x320 | i32 | saved selector 7 (pushed to form+0xa28) |
| 0x330 | i64 | sample counter (global LFO phase) |
| 0x338 | ptr | last triggered voice (legato) |
| 0x570 / 0x578 | ptr | the timbre after the EQ bands / before them, 528 f32 each (one 0x10c0 block: 0x578 at +0, 0x570 at +0x840, 0x580 at +0x1080) |
| 0x580 | ptr | f32[4]: self-oscillation `p62 / 256`, then the three sub levels `p / 1024` (params 2..4) |
| 0x590 / 0x598 / 0x5a0 | ptr | f64[528] partial ratios (with the detune), the colour's phases in turns, the grittiness offsets |
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
| 0x7b8 / 0x7bc | i32 | effect sleep countdown and what voices reset it to (section 8) |
| 0x7c0 | ptr | chorus, a `TStereoFlanger` (param 90 makes it; `FUN_0074b0f0` copies the block into its ring, `+0x60` kernel); not a phaser, which is a curve in the voice (section 6) |
| 0x7c8 | f32 | the chorus's dry gain |
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
`FUN_00409fe0` is `Round` (to even), `FUN_004088f0` is `Frac`,
`FUN_00409fd0` is `sqrt`, `FUN_00409520` / `FUN_00409610` are `cos` / `sin`,
`FUN_005ec9f0` is `power`. Constants: `PTR_007ef3c0 = 1/1000`,
`PTR_007f0758 = 1/1024` (not 1/2048 as an earlier pass had it),
`PTR_007ef398 = 1/256`, `PTR_007f0328 = 1/128`, `PTR_007ef530 = ln 11`,
`PTR_007f07a8 = ln 3`, `PTR_007f04b8 = 0.5`, `PTR_007f0620 = 1/10`,
`PTR_007efad0 = ln 2`, `PTR_007f0898 = log2 440`, `PTR_007f0830 = 0.01`,
`PTR_007ef6f0 = 2 pi`, `PTR_007f0468 = ln 10 / 20` (the runtime ones are
set by `FUN_00614c80`). The shared level curve is the Delphi plugin
helper's `level(x) = (11^x - 1) / 10`; the main and timbre EQ
input wheels use `level3(x) = (3^x - 1) / 2`.

The "default" column is the editor's controls as the DFM gives them. A new
Harmless in FL is `Default.fst` (v10), which differs: mask 1000 on the odd
steps, mask mix 0, decay 1000 (hold), pluck 1000 (off), porta time 400,
FEnvAlt 1, FEnvAmt 500, FEnvDecSlope -300, keytrack 0, FatType 2, FatVar 0,
PKbTrack 0, SFXStrength 256, SFXPos 3, selectors (4, 0, 0, 0, 0, 1, 0). The
implementation starts from `Default.fst`.

| REC | DFM object | range | default | code (`this` offsets) | confidence |
|---|---|---|---|---|---|
| 0 | first timbre wheel (Tag 0, not serialised in the DFM) | -1000..1000 | 1000 | `0x5b8 = sign(v) (v/1000)^2` | loader, corpus |
| 1 | `SafetyVolSlider` low harmonics protection | 0..1024 | 256 | `0x5fc = level(v/1024)` | loader, dfm |
| 2..4 | `Sub1Slider`, `Sub3Slider`, `Sub4Slider` | 0..2048 | 0 | `0x580[k-1] = v/1024`: the partials at 0.5, 1.5 and 2.5 times the note | loader, dfm |
| 5..10 | `TEQ1Slider`.. timbre EQ 1..6 | -1024..1024 | 0 | `0x61c[k-5] = level3((v + 1024)/1024)`: 0 at -1024, 1 at 0, 4 at 1024; sets 0x5a8 | loader, dfm |
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
| 36 | `MainVolSlider` | 0..2000 | 1000 | `0x2e4 = level3(v/1000) * 0.75` | loader, dfm |
| 37 | `VelToVolBox` | 0, 1 | 1 | flag | dfm |
| 38 | `MainPitchWheel` cents | -2400..2400 | 0 | `0x2dc = v - 1200` (semitone offset base) | loader, dfm |
| 39 | `DetuneWheel` cents | -2400..2400 | 0 | sets 0x5a9 (mask/timbre recompute) | loader, dfm |
| 40 | `TGrittinessWheel` | -1000..1000 | 0 | `0x5ac = sign(v) (e^(|v|/1000 ln C_140) - 1) * C_148`; FUN_00783c50 | loader, dfm |
| 41 | `VibratoWheel` | 0..1000 | 0 | `0x31c = v * C_070` | loader, dfm |
| 42 | `PitchLFOAmtWheel` | -1000..1000 | 0 | `0x2e0 = sign(v) (e^(|v|/1000 ln C_c8) - 1) * C_138` | loader, dfm |
| 43, 44 | `PortaBox`, `MonoBox` | 0, 1 | 0 | 43 > 0: `FUN_00788030` (portamento setup); TriggerVoice reads them at 0x1d4 and 0x1d8; 44 is the legato flag | loader, dfm |
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
| 93..95 | `DelVolWheel`, `DelFBWheel`, `DelTimeWheel` | 0..1000, 0..900, 0..384 | 500, 300, 144 | time in 192nds of a beat (`FUN_00789080`: 144 is 3/4 beat) | dfm |
| 96 | `RevTypeSelect` | 0..6 | 0 | | dfm, corpus |
| 97 | `RevWetWheel` | 0..1000 | 500 | 0x7f8 | dfm |
| 98 | `CompVolWheel` compression mix | 0..1000 | 500 | Maximus parameter 5, its LMH mix: `ProcessParam(5, v)` | loader, dfm |
| 99 | `VelToFatPitchBox` | 0, 1 | 0 | (v9+) | loader, dfm |

Cases 0x36..0x62 of `ProcessParam` (read in `run4refs/FUN_0077f630`):

| REC | field | maths | confidence |
|---|---|---|---|
| 54 `FCut` | 0x6d0 | `min + v/8192 * range` (log2 Hz, range below) | loader |
| 56 `FWidth` | 0x6e8 | points of the curve an octave: `2^(1 - 0.004 v) * 1024` (2048 at 0, 512 at 500, 128 at 1000), then `FUN_0077c170` | loader |
| 57 `FKbTrack` | 0x6cc | `v/1000`; the cutoff moves by `(log2 f0 - 7) * 0x6cc` octaves | loader |
| 58 `FLFOAmt` | 0x6a8 | `v/1000 * 6` octaves | loader |
| 52 `FEnvAmt` | 0x6a4 | `v/1000`, times the range unless `FEnvAlt` | loader |
| 48, 49 `FEnvAtt`, `FEnvDec` | 0x638, 0x640 | `+1 / -1` over `(SR (1001^(v/1000) - 1) 0.12 + 1)`: a step a sample, the decay's negative; 0 at 1000 | loader |
| 50, 51 slopes | 0x700, 0x70c | `FUN_005fd4d0(t)` with `t = sign (1000001^(|v|/1000) - 1)`: record `(t, 1/|t|, ln(|t| + 1))` | loader |
| 60, 63 `ResHzSlope`, `FResAdaptive` | 0x6ec, 0x6f0 | `FUN_00788060`: (log, fixed) 1024 / -0.125; (log, adaptive) `450560 * 2^(-27/12)` / f0 and 0.875; (linear, fixed) 0.384 / -1.5; (linear, adaptive) `168.96 * 2^(-15/12)` / f0 and 0.5 | loader |
| 61 `FResCutOfs` | 0x6e4, 0x6e0 | `v/1200` octaves, and `2^` that | loader |
| 62 `FOsc` | `0x580[0]` | `v/256` | loader |
| 65 `FatOrder` | 0x5c0, 0x5cc | the order; gain `1 - (3^(1 - 1/n) - 1)/2` (1 for one voice) | loader |
| 67 `FatPitch` | 0x5c4 | `(41^(v/256) - 1) * 1.875` cents | loader |
| 68 `FatPhase` | 0x5c8 | `v/128` | loader |
| 73, 74 `PWidth`, its LFO | 0x770, 0x76c | `v * 0.0001`, `v * 0.0005` | loader |
| 76 `POfsLFOAmt` | 0x768 | `v * 0.0005` | loader |
| 77 `PMotionSpeed` | 0x758 | `sign * 2^(|v|/1000) * tempo / (SR * 3840)` turns a sample | loader |
| 80, 81 `SFXWidth`, its LFO | 0x774, 0x778 | `v/1000` | loader |
| 83 `SFXStrength` | 0x77c | `2^(v/256) * 1.331` | loader |
| 86 `LFOShape` | 0x568 | the shape (section 6) | loader |
| 88 `LFOAtt` | 0x560 | `1 / (SR (101^(v/1000) - 1) 0.05 + 1)` a sample | loader |
| 89 `LFOSpeed` | 0x558, 0x564 | `sign * 2^(|v|/1000) * tempo / (SR * 3840)` turns a sample (the LFO is always in time with the song: 3000 is a turn in 8 beats); `0x564 = 0.6 v + 1200` for the key shape | loader |
| 90, 91 chorus | `FUN_00789570` | section 8 | loader |
| 92..95 delay | `FUN_00789080` | section 8 | loader |
| 96, 97 reverb | `FUN_00789280` | section 8 | loader |
| 98 `CompVol` | | `ProcessParam(5, v)` on the embedded Maximus: its LMH mix | loader |

Params 55, 59, 64, 69..72, 75, 78, 79, 82, 84, 85, 87 and 99 have no case:
the renderer reads them from the parameter block.

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
chorus  0x7c0: 0x7b8 < 1: FUN_0074b0b0 (LFOs on); else FUN_0074b0f0 then kernel +0x60 into scratch; sum *= dry 0x7c8 when != 1.0; sum += scratch
delay   0x7d0: 0x7b8 > 0: scratch = sum * 0x7e4, added into the ring per side at offsets 0x7dc / 0x7e0 (FUN_00747510); always kernel +0x40 or +0x60 by 0x7d8 into scratch; sum += scratch
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

The `TriggerVoice` (`FUN_00782940`) handles the FL note kinds
(`local_74 == 0xf` slide, `0xd` legato, `0xe` porta), legato (`0x1d8`,
param 44 `MonoBox`) and portamento (`0x1d4`, param 43; voice table at
`0x340 + 0x20 k`). `MonoBox` is a legato flag, not a voice limit: with it
on, or for a legato note with it off, a new note while one is held glides
that voice to the new pitch (`FUN_00787ef0`) and starts no voice of its
own. `0x1bc` is param 37 `VelToVol`: when it is off the voice asks the host
(`GetVoiceParam(voice, 6)`) not to put the velocity on its volume.


## 4. The timbre (`FUN_00787990`, `FUN_007873c0`, `FUN_00787b40`, `FUN_00787ce0`, `FUN_0077b450`, `FUN_0077b2d0`)

Partials, the same for every array below: 0 the filter's self-oscillation,
1..3 the sub partials, 4 + n harmonic n + 1 (n = 0..523). 528 = 0x210 in
all.

`FUN_00787990` (on load and on the colour selector) fills the levels
(`0x578`) and the phases in turns (`0x598`, f64) from `FUN_007873c0` at
`n = 0..523` into 4 + n; slot 0 is level 1 phase 0, slot 1 a copy of slot 4,
slots 2 and 3 the colour at `n = 0.5` and `n = 1.5`. It sets 0x5a8 so the
next render runs the EQ. `FUN_007873c0` is the colour (`0x56c` = selector
4); parity is of `Trunc(n)`:

| colour | level | phase (turns) |
|---|---|---|
| 0, 3, 4 | `1 / (n + 1)` (a saw) | 3: 0.25 on odd n; 4: 0.5 where `n & 3 == 1` |
| 1 | `sqrt(0.5) / (n/2 + 1)` | 0 |
| 2 | `0.5 / (n/4 + 1)` | 0 |
| 5 | even n `1.1 / (n + 1)`, odd n `1.1 / ((n + 1) n)` | odd 0.25 |
| 6 | even n `1.1 / (n + 1)`, odd n `1.1 / (n + 1) / n^0.2` | odd 0.25 |
| 7 | `(7^(1/(n + 1)) - 1) * 0.29167` | odd 0.75 |
| 8 | `0.5 / (n + 1)`, plus `0.5 / (n/2 + 1)` on odd n | 0 |
| 9 | `1/(3 (n + 1))`, plus `1/(3 (n/2 + 1))` on odd n, plus `1/(3 (n/4 + 1))` where `n & 3 == 3` | 0 |
| 10 | `0.5/(n + 1)`, plus `0.25/(n/2 + 1)` on odd n, plus `0.125/(n/4 + 1)` where `n & 3 == 3` | 0 |
| 11 | the drawn timbre of the state: level `image[Round(n)]`, phase `image[Round(n) + 524]`; 0 without one | |

The ratio table `0x590` starts in the constructor (`FUN_0077b800`) as
`1, 0.5, 1.5, 2.5, 1, 2, 3, ...` and `FUN_00787ce0` (on `Detune`, param 39)
remakes it with `d = 2^(v/1200)`: `ratio[4 + i] = 1 + i d`, `ratio[2] = 1 +
0.5 d`, `ratio[3] = 1 + 1.5 d`; slots 0 and 1 stay. The same function fills
the grittiness table `0x5a0 = sin((ratio - 1) * 2 pi / 15)` (slot 0 = 0), and
resets the pluck (`FUN_00783c50`, `FUN_00783c90`).

The timbre EQ (`FUN_00787b40`): `0x570 = 0x578 * sum over bands b of
eq[b] * band_b[k]`, the bands built once by `FUN_0077b2d0` from the
undetuned ratios: band b weighs partial k by `(cos(x pi) + 1)/2` with
`x = clamp(log2 ratio[k] - b, -1, 1)`, band 0 is 1 below its centre and
band 5 is 1 above, so the six sum to 1 and each covers the octave around
harmonic `2^b`. `eq[b] = level3((v + 1024)/1024)`.

Other tables of `FUN_0077b450`: the protection weights `DAT_00811ac8[k] =
((cos(max(log2 ratio[k], 0) / log2 5 * pi) + 1)/2)^5` for k = 0..7 (1 for the
subs and the fundamental, 0.083 for harmonic 2, 0 above), and the 12
harmonic LFO coefficients `DAT_00811a50[12 + i] = -11.1489 sin^2(i pi/12) /
i^3`.

## 5. The sine bank (closed: bank class at `0x744e20`, kernels behind `PTR_007f0xxx`)

The `PTR_007f0xxx` / `PTR_007efxxx` kernels are Intel IPP functions:
`FUN_005acdb0` loads `dsp_ippv2.dll` and fills one `.bss` slot per export
by name, and each `PTR_` is the address of a slot. The ones the voice and
the image use:

| pointer | function | pointer | function |
|---|---|---|---|
| 007f03d0 | ippsMulC_64f | 007f03d8 | ippsMulC_32f_I |
| 007ef850 | ippsMulC_32f | 007ef8a8 | ippsMulC_64f_I |
| 007ef3c8 | ippsMul_32f | 007eff80 | ippsMul_32f_I |
| 007ef430 | ippsMul_64f_I | 007efc08 | ippsAdd_32f |
| 007f07b8 | ippsAdd_32f_I | 007efc20 | ippsAdd_64f_I |
| 007efd98 | ippsSub_32f | 007f0960 | ippsAddProductC_32f |
| 007ef5c8 | ippsAddC_64f | 007ef5d8 | ippsAddC_32f_I |
| 007efb28 | ippsSubCRev_32f_I | 007ef6b8 | ippsDivCRev_32f_I |
| 007f07e8 | ippsCopy_32f | 007efc58 | ippsCopy_64f |
| 007efda0 | ippsCopy_32s | 007f08d8 | ippsCopy_64s |
| 007eff28 | ippsMove_32f (src, dst, len) | 007f0028 | ippsMove_64s |
| 007ef400 | ippsZero_32f | 007effb8 | ippsZero_64f |
| 007f0860 | ippsSet_32f | 007efe98 | ippsSet_32s |
| 007ef4b0 | ippsVectorSlope_64f | 007effc8 | ippsVectorSlope_32f |
| 007f06c8 | ippsConvert_32f64f | 007f06d0 | ippsConvert_64f32f |
| 007f06f0 / 007efb58 | ippsLn_32f_A11 / A24 | 007f0890 / 007efd10 | ippsExp_32f_A11 / A24 |
| 007f00c8 / 007ef460 | ippsSin_32f_A21 / A24 | 007f07a0 | ippsSin_64f_A26 |
| 007f0508 | ippsPowx_32f_A21 | 007efbc0 | ippsAbs_32f |
| 007ef5a8 | ippsThreshold_GT_32f_I | 007ef7e8 | ippsThreshold_LT_32f_I |
| 007f06d8 | ippsThreshold_GTVal_32f_I | 007f0988 | ippsThreshold_LTVal_32f_I |
| 007efd58 | ippsSum_32f | 007f08b0 | ippsSum_64f |
| 007f00f8 / 007ef5a0 | ippsRandUniform_Direct_32f / 64f | 007f0888 | ippsTone_Direct_32fc |
| 007f0310 | ippsRealToCplx_32f | 007f07c0 | ippsCplxToReal_32fc |

The plugin's own kernels sit behind `DAT_00801640..68` (set by
`FUN_00746c20`) and `DAT_00811ad8/ae0` (`FUN_0078a160`): `FUN_00746220` /
`FUN_00746160` phase advance `phase += freq * n`; `FUN_00745730` /
`FUN_007457c0` (re)start a partial; `FUN_0077c400` / `FUN_00789f90` the
index and fraction of a curve lookup; `FUN_0077c3c0` / `FUN_00789f40` the
harmonizer's strided add `dst[j * step] += src[j] * gain`.

The bank (`FUN_00746320` render, groups of 8 partials of 0x100 bytes:
level, two recursion states, `2 cos w`, target level, smoothing, flags):

- Each partial is `s[n] = 2 cos(2 pi f) s[n-1] - s[n-2]`, started from its
  phase in turns (`f64`, bank `+0xb0`) at `sin(2 pi (phase - f))` and
  `sin(2 pi (phase - 2f))` so its first value is `sin(2 pi phase)`
  (`FUN_00745730`, `FUN_00745680`); f is in turns a sample (bank `+0xb8`,
  f64). A partial restarts from its phase every `Round(f * C)` samples and
  whenever its frequency changes. The SSE kernel is `FUN_007459a0` (first
  group) / `FUN_00745ad0` (the rest), 4 lanes summed by `FUN_00746270`.
- The output is `s * level`, then `level = (level - target) * k + target`
  a sample, `k = 0x5f8 = 0.01^(1 / (0.01 SR))` (`FUN_007452b0` puts it in
  every group): a 10 ms glide to 40 dB below the step. A new voice copies
  its targets to its levels.
- Partials with level and target under 2^-24 drop out of the groups
  (`FUN_00745d40`, `FUN_00745f70`, `FUN_00745de0`).
- `FUN_007453c0(bank, 32, 16, 16000, 17000 Hz)` (from `TriggerVoice`) sets
  the weight the voice puts on each partial by its frequency: 0 under 16 Hz,
  `(1 - (32 - f)/16)^2` to 32 Hz, 1 to 16 kHz, `(1 - (f - 16000)/1000)^2` to
  17 kHz and 0 above.

The bank's mono output goes through the amplitude envelope and the host's
volume and pan (`FUN_00604960`, with the voice's `FinalLevels`).

## 6. The control tick (`FUN_00784a10`, first loop)

A voice is worked out every `0x2c8` samples: `Round(22 * x) * 4` with x the
host's rate factor and 88 at 44.1 kHz, from the quality selector 6's
`DAT_007ee584 = 88, 44, 8` (`FUN_0077f170`). Per tick, in order:

1. Filter envelope: `pos += inc`; the attack's `inc = 0x638 * tick`; at
   `pos >= 1` it carries the overshoot into the decay with `inc = 0x640 *
   tick` (negative) and the decay slope record; `pos >= 0`. The value is
   the record's curve of pos: straight at 0, else for `t > 0`
   `1 - (exp((1 - x) ln(t + 1)) - 1) / t` and for `t < 0` `(exp(x
   ln(|t| + 1)) - 1) / |t|`. A release in the attack moves to the decay
   curve at the same value (`FUN_00783d80`).
2. The cutoff `0x18c` glides to `FCut` by `0x6d4 = 0.01^(tick / (0.1 SR))` a
   tick.
3. Pitch in cents from C5: the host's pitch + `MainPitch - 1200` + vibrato
   `sin(2 pi phase) * 0.075 v * a^2` (6 Hz, `a` reaching 1 in a third of a
   second) + `PitchLFO * lfo` (`0x2e0 = sign (101^|v| - 1) * 12`) + the unison
   offset (times the velocity with `VelToFatPitch`). Hz `= 440 *
   2^((cents/100 + 3)/12)`, so key 60 at 0 cents is 523.25 Hz and a new
   Harmless (`MainPitch` 0) plays it at 261.6 Hz.
4. LFO (shape `0x568`) at the voice's phase (`0x550` when global) plus the
   unison variance: 0 `sin(2 pi (x - Round(x) + 0.5))`; 1 the 12 harmonic
   series; 2 `sign(s) (1 - (1 - |s|)^6)`; 3 a saw `2 (y - Round(y))`, y =
   x - 0.5; 4 a square from `Round(2x - 0.5) & 1`; 5 three steps; 6 a
   triangle; 7 a cubed saw; 8 two sines at x and `(x + 7) / 5`; 9 the filter
   envelope; 10 the cutoff's place in its range; 11 a random value a note;
   12 the velocity; 13 the key, `(cents + 0x564) / 3600` in -1..1. Times
   `att^2` (the LFO attack), and a unipolar copy `(v + 1)/2` (9, 10, 12:
   the value itself).
5. When the pitch moved: the partial count `Round(17000 / f0 / d) + 4`
   (`FUN_00787c30`; quality 1 times 1.5, quality 2 all 528), up to a
   multiple of 8; frequencies `ratio * f0 / SR` plus `0x5a0 * f0 * grit /
   SR` (`grit = sign (65^|v| - 1) * -0.0125`); each partial's Hz and log2 Hz.
6. The mask (`FUN_00783f90`, `FUN_00783f00`): `mix = MaskMix + clamp(MaskLFO,
   -mix, 1 - mix) * lfo01`; when above 0 harmonic j gets `mask[j % 12] *
   mix + 1 - mix`, `mask = sign(v) level(|v|/1000)`.
7. Harmonizer at position 0 (selector `SFXPos`, param 85).
8. Filter: `fc` (log2 Hz) = smoothed cutoff + `(log2 f0 - 7) * KbTrack` +
   `ModX * range/2`, clamped to `[min + key, max]`; the LFO adds `lfo01 *`
   its amount limited to the room; the envelope adds `(env - 0.5) * amount`,
   or with `FEnvAlt` moves toward the top (`amount > 0`) or the bottom by
   `env * |amount|` of the way. Each partial is multiplied by the filter
   curve at `log2 Hz * W + centre - fc * W` (`FHzSlope` 0, W = `0x6e8`
   points an octave) or at `Hz * 0.9 W / 2^fc + centre - 0.9 W` (linear),
   `FUN_0077c540`: nearest point (`FUN_0077c470`) or between the two points
   either side (`FUN_0077c400`), the end points outside.
9. Resonance: `r = level(clamp(FRes/1000 + ModY/2, 0, 1)) * 0.25`; with `FResMT` times `min((fc - fc_slow)^2, 1)`, `fc_slow` gliding
   by `0x6d8 = 0.01^(tick / (1.25 SR))`. When r > 0 the resonance curve is
   read at `(log2 Hz - fc - offset) * S + centre` (or linear in Hz) with S
   `0x6ec` (over f0 when adaptive), and each partial is multiplied by
   `max(max((log2(Hz/f0) + 0x6f0) * r, 0) * curve + 1, 0)` (`FUN_00784180`).
   Resonance type 8 (`0x6c`) instead adds noise bursts (`ippsRandUniform`).
10. Harmonizer at position 1.
11. Phaser (`PMix > 0`): the periodic curve at `log2 Hz * 2^((PWidth - 1) 8 +
    lfo PWidthLFO 2) * 12 N` (`PHzSlope` 0; 1 is Hz times that times 0.005;
    2 is `Hz (2 - width) N / f0`) plus `((-POfs/1000 - lfo POfsLFO) - var) N
    - motion N'` minus the key tracking; each partial times `curve * mix +
    1 - mix`. Type 9 (`0xd1`) instead bends each partial's frequency by
    `exp(curve * mix^3 ln2 / 4)`. The motion phase `0x750` moves by `0x758`
    a sample.
12. Harmonizer at position 2.
13. Pluck (`FUN_00784860`, voice `+0x119`): a tick count starting at
    `min(1 - Round(0.002 SR / tick), -Round(attack / tick))`, from which each
    partial falls by `exp(ratio * ln(0.01) * tick / SR / (PluckDec + 0.25))`
    a tick (`PluckDec = (1001^v - 1) * 0.11975` seconds, times velocity^2
    with `VelToPluckDec`; `PluckAlt` uses `1/|timbre|` for the ratio;
    `PluckMasked` puts the mask on the rates; `PluckRel` starts it at the
    release).
14. Harmonizer at position 3.
15. Protection: `level[k] += (timbre[k] - level[k]) * protect[k] * 0x5fc` for
    k < 8 (`FUN_00784120`), then `level[0..3] *= 0x580[0..3]`
    (`FUN_00784110`).
16. Self-oscillation: `level[0] = FOsc/256 * r * pluck[1]` at `2^fc *
    2^(FResCutOfs/1200)` Hz.

The harmonizer (`FUN_00784220`, table `DAT_007ee634`, by `SFXType`): from a
copy of the harmonics, `step = 2` and then `step * times + add`, it adds
harmonic h's level at harmonic `step h + start + 1` with gain `min(left^3 *
SFXStrength, 1) * SFXMix^2`, `left` falling from 1 by `1 / (width * spread
+ 1)` each step while above 0 (`width` = `SFXWidth`, times velocity with
`VelToSFXWidth`, plus its LFO); then all harmonics times `1 / (1 + sum of
gains)`. Types: 0 `(-1, 0, 2, 5)` octaves, 1 `(0, 0, 2, 5)`, 2 `(1, 0, 2,
5)`, 3 `(-1, 1, 1, 10)` every multiple, 4 `(0, 2, 1, 10)` even multiples.

The shape curves (`FUN_0077ca30`, built for selector 0, selector 1 + 100,
selector 2 + 200; `FUN_0077c940(a, b, c)` sets the centre `a (N - 1)` and
how far the curve reaches below and above it, `FUN_0077c920` scales a
curve to an RMS; `slope(x) = 10^(-3.6 x) (1 - x^16)`, `rc(x, w) = (cos(x w)
+ 1)/2`, x = i/(N - 1)):

| type | N | point i | centre / reach / RMS |
|---|---|---|---|
| 0 LP | 2048 | `slope(x)` | 0 / 1, 0 |
| 1 BP | 4096 | `slope(1 - tent)`, tent the distance from the middle read round | 0.5 / 1, 1 |
| 2 notch | 4096 | `1 - ` type 1 | 0.5 / 1, 1 |
| 3 HP | 2048 | `slope(1 - x)` | 1 / 0, 1 |
| 4 | 1024 | `(1 - rc(1 - x, pi))^2` | 0 / 1, 0 |
| 5 | 2048 | `(1 - rc(1 - x, 2 pi))^2` | 0.5 / 1, 1 |
| 6 | 2048 | `rc(1 - x, 2 pi)^2` | 0.5 / 1, 1 |
| 7 | 1024 | `rc(1 - x, pi)^2` | 1 / 0, 1 |
| 8 | 682 | `-cos((1 - x/2) pi)` | 0 / 1, 0 |
| 9 | 1024 | `(1001^(1-x) - 1) 0.00075 + (1 - x)/4` | 0 / 1, 0 |
| 10 | 1365 | `slope(cos((1 - x/2) pi) + 1)` | 0 / 1, 0 |
| 11 | 1024 | `(1 - rc(1 - x, 3 pi)) (1 - rc(1 - x, pi))` | 0 / 1, 0 |
| 12 | 4096 | first half `(1 - (cos(2 pi (1 - x)) + 1)/4)^2`, second `slope(2 (i - N/2)/(N - 1))` | 0.5 / 1, 0 |
| 13 | 1024 | `10^((18 (1 - rc(1 - x, pi)) - 18)/20)` | 0 / 1, 0 |
| 14 | 4096 | `rc(1 - x, 8 pi)^2` | 0.5 / 1, 1 |
| 15 | 4096 | `rc(1 - x, 12 pi)^2 b^2 + b/8`, `b = 1 - rc(1 - x, 2 pi)` | 0.5 / 1, 1 |
| 16 | 2048 | 1, with raised-cosine ramps over the outer thirds | 0.5 / 1, 1 |
| res 0, 8 | 1024 | `(1 - rc(1 - x, 2 pi))^3` | 0.5 / 1, 0 / RMS 7 |
| res 1 | 682 | `(1 - rc(1 - x, 2 pi)) sinc(2 pi 2 (i - N/2)/(N - 1))` | RMS 9 |
| res 2 | 341 | as type 16 | RMS 10 |
| res 3 | 512 | `slope(1 - tent)` | RMS 18 |
| res 4 | 2048 | as type 16 | RMS 5 |
| res 5 | 1536 | `(1 - rc(1 - x, 4 pi))^4` | RMS 5 |
| res 6 | 768 | `-(1 - tent)^2` | none |
| res 7 | 1024 | `-sinc(...) (1 - rc(1 - x, 2 pi))^0.125` | RMS 10 |
| ph 0 | 1024 | `sin(2 pi (i + 3N/4)/N) + 1` | RMS 1 |
| ph 1 | 1024 | `(1 - tent)^2` | RMS 1 |
| ph 2 | 1024 | `sin(pi i / N)` | RMS 1 |
| ph 3 | 1024 | `(sin(...)/2 + 1/2)^6` | RMS 1 |
| ph 4 | 1024 | `slope(1 - tent)` | RMS 1 |
| ph 5 | 1024 | `slope(1 - s^2)`, `s = (sin/2 + 1/2)^2` | RMS 1 |
| ph 6 | 2048 | two slopes, the second times 4 | RMS 1, period N/2 |
| ph 7 | 2048 | `(sin(2 pi (4x + 0.75)) + 1)(1 - x)` moved N/8 | RMS 1, period N/4 |
| ph 8 | 2048 | cubed soft square on the middle half | RMS 1 |
| ph 9 | 2048 | `sin(2 pi (i + 3N/4)/N)` (frequency shift) | RMS 1 |

Every curve gets its points under 2^-24 set to 0 (`FUN_0060ff50`) and a
copy of point 0 after its last. The filter's range (`FUN_0077c170`):
`min = log2(440 * 2^(-31/12)) - e(below)`, `max = log2 17000 + e(above)`,
`e(r) = u - sign(u) min(u, 1)` with `u = 682.67 / W * r`; the `FWidth` and
`FCut` setters re-run on it.

## 7. The amplitude envelope (`FUN_00784a10` second loop, `FUN_005f5f60`, `FUN_005f5fe0`)

`TriggerVoice` per unison voice: `attack = 0x5d8 * var` (`0x5d8 = SR *
(101^(v/1000) - 1) * 0.05`), times `1 - min(1.125 vel, 1)` with
`VelToAtt`; a declick one-pole of `min(attack, 0x5f4 = 0.03 SR)`; when the
attack is under `0x5d0 = 0.2 SR` it runs as a 0.2 s ramp started at
`Round(0.2 SR (1 - attack/0.2 SR)^2) + 1`. The ramp is `pos / (attack + 1)`
clipped at 1 until `Round(attack + declick) + 1`; then the declick becomes
`0.08 SR` and an exponential decay (`0x5e0 = SR ((1001^v - 1) 0.11995 +
0.05)` samples to fall 40 dB; held at `Decay` 1000) runs to silence. A
release (`Voice_Release`, `FUN_00783910`: state 6, `0x124 = -1`) switches
the declick to 0.03 s and the decay to `0x5e8 = SR ((101^v - 1) 0.0249 +
0.01)` times the release velocity with `VelToRel`. `FUN_005f5f60(obj, t)`
gives a one-pole `0.01^(1/max(t, 1))` (0 for t <= 0) and `FUN_005f5fe0`
runs `y = x + c (y - x)`; a voice whose envelope ends a block under -80 dB
(`0x5f0 = 10^(-80/20)`) after its attack is killed (`FUN_00783ac0`).

The voice's gains (`FUN_00784a10`, head and tail). At the start of every
block, before its first tick, the voice asks the host's `ComputeLRVol`
(host vtable +0xf8, slot 31: the circular law of
`docs/plugins/host-voice.md`) for its left and right aims, `voice+0x134`
and `+0x138`:

```
vol = 0x2e4 (main volume) * (FinalLevels.Vol + glide +0x54) * voice+0x1c4 (= 0x5cc, the unison gain, 1 for one voice)
pan = FinalLevels.Pan + glide +0x50 + voice+0x1c0 (the unison pan offset)
```

A voice's first block (`voice+0x38`) starts its gains (`+0x13c`, `+0x140`)
at the aims. At the end of the block the bank's output times the envelope
(`ippsMul_32f_I`) goes through `FUN_00604960(mono, stereo, n, aimL, aimR,
&gainL, &gainR, host+0x34)`: `FUN_00612db0` per side is FL's capped glide
(`parts::glide`: a step of `(aim - gain)/n` a frame, at most host+0x34 =
`0.001 * 44100 / SR`, and a gain the cap leaves under 2^-24, `DAT_007ff4dc`
from `FUN_00614c80`, goes to 0 instead); both steps 0 multiplies by the
two gains (`FUN_00611660`, or zeroes when both are 0), else
`FUN_00611760` ramps them in its SSE lanes: the even and odd frames each
from their own start (`g`, `g + s`) moving `2s` a pair, eight frames to a
round, the frames after the last round from the even lane a step at a
time.

Unison (`FatOrder` n): voice i gets `x = 2i/(n - 1) - 1`, pan `x FatPan/128`,
pitch `s FatPitch` cents with `s = |x|` signed by `i` odd and by the upper
half of an odd n, times the type's spread `DAT_007ee5c8` (type 0 `0.877
1.069 0.929 1.181 1.031 1.129 0.887 1.117 0.977`, type 1 all 1, type 2
`0.890 1.076 0.912 1.103 1.034 1.167 0.854 1.206 1`), phase `i/n FatPhase`
(type 2: `frac(2^(cents/1200) 100000 FatPhase^8 + 0.5)`), attack times
`2^((|s| - 1) FatVar/128 * 3)`, LFO offset `s FatVar/256`, gain `0x5cc`.
One voice has phase 0.5 and nothing else.

Phases at the note's first tick: the colour's phase plus `ratio * (unison
phase + frac(f0 277 t) * 0x5b8)` with t the plugin's sample counter and
`0x5b8 = sign(v) (v/1000)^2` of param 0; when param 0 is negative the
phases are random (`ippsRandUniform_Direct_64f`) up to `0x5b8^2`.

## 8. The post chain (closed: setters `FUN_00789570`, `FUN_00789080`, `FUN_00789280`, `FUN_0077f170`)

Order in `Gen_Render` (`FUN_007886b0`): stereo tremolo on each voice,
then on the sum the chorus, the delay, the reverb and the compression.

- Tremolo: `ippsTone_Direct_32fc` at `0x7a4 = 2.5 / SR` with magnitude 1.25,
  `|.| - 1`, smoothed by `FUN_00604a90` with the depth `Trem/1000`, `+ 1`:
  the left follows `|cos|`, the right `|sin|`.
The three effects are FL's shared classes, the same code Harmor links in;
`docs/plugins/fl-fx.md` maps them and `crates/plugins/src/fl_fx.rs` is
their port. What Harmless adds is the front end below. The setters run on
their parameters, the block size (Dispatcher 3, this+0x2d4) and the rate;
the tempo event (`ProcessEvent` `FUN_0077f5e0`, this+0x2cc) re-runs the
delay's, so a new tempo that moves the delay time empties its ring.
A type of 0 frees the object; the next type above 0 makes a new one.

- Sleep: while there are voices, `Gen_Render` sets 0x7b8 to 0x7bc; with
  none it takes the block length off it. 0x7bc is `round(0.004 SR)`, plus
  the chorus ring (block + longest delay + 8 frames) when there is a
  chorus. Under 1 the chorus only turns its LFOs (`FUN_0074b0b0`) and the
  delay takes no input but keeps sounding its feedback; the reverb never
  sleeps.
- Chorus (`TStereoFlanger`, constructor `FUN_0074a790`, setter
  `FUN_00789570`, voices `FUN_0074a9b0`, x87 kernel `FUN_0074b2f0` with
  voices `FUN_0074b180` / `FUN_0074b230` under a truncating control word,
  `FUN_005a43a0`): table `DAT_007ee73c + 0x20 t` holds Flangus's six
  wheels (order, depth /1024, speed /1024, delay /128, spread /128, cross
  /128): 1 `(4, 512, 768, 8, 128, -32)`, 2 `(5, 1024, 856, 0, 128, 68)`, 3
  `(7, 512, 512, 128, 128, 54)`, 4 `(6, 561, 854, 0, 0, 38)`, 5 `(1, 136,
  312, 0, 0, 128)`, 6 `(2, 86, 632, 0, 0, 0)`, 7 `(1, 42, 944, 0, 0, -22)`,
  8 `(9, 81, 170, 0, 0, -66)`. Mix m = `ChMix * 0.001f`: dry `0x7c8 = 1 -
  (5^m - 1)/4`, wet `+0x50 = 1 - (5^(1-m) - 1)/4`, which the class folds
  into its voice gains. No ramp on either.
- Delay (`TDelay_32FS`, constructor `FUN_007471d0`, setter `FUN_00789080`,
  ring size `FUN_00747070`, input `FUN_00747510`, kernels `FUN_007475a0`
  straight and `FUN_007477f0` ping-pong): time `T = round(DelTime * SR *
  0.3125 / tempo)` samples (192 steps a beat; 144 is 3/4 beat), at least 1;
  feedback `DelFB * 0.001f` into +0x30, ramped by the host cap; input gain
  `0x7e4 = level(DelVol/1000)`. Table `DAT_007ee67c + 8 t` (offset ms,
  kernel): 1 `(0, 0)` straight echo (`+0x40`), 2 `(1, 2)` and 3 `(-20, 2)`
  ping-pong (`+0x60`). The offset, `round(|ms| * 0.001 * SR)` frames, goes on
  the right side when positive and the left when not, and the ring is
  block + offset + T frames.
- Reverb (`TFruityReverb`, constructor `FUN_0074a360`, setter
  `FUN_00789280`, update `FUN_00749b60`, process `FUN_0074a540` into
  `Reverb_x64.dll`'s unmodulated `FUN_180002980`): the sum is made mono as
  L + R (`FUN_006119f0`), reverberated to stereo and added at `0x7f8 =
  level(RevWet/1000) / 2`. Table `DAT_007ee67c + 0x20 t`, eight ints for
  the class's low cut (Hz), high cut (100 Hz), predelay (`round(v SR /
  384)` samples), room size, diffusion, decay (`v SR / 10` samples),
  damping (100 Hz) and bass colour (index into 0.2, 0.6, 1, 1.5, 3): 1
  `(75, 40, 0, 40, 100, 15, 40, 2)`, 2 `(0, 115, 18, 40, 100, 22, 50, 2)`,
  3 `(34, 21, 27, 90, 100, 45, 31, 3)`, 4 `(400, 60, 31, 60, 65, 23, 37,
  1)`, 5 `(100, 36, 0, 35, 100, 6, 36, 2)`, 6 `(100, 60, 0, 17, 100, 5, 30,
  2)`. The crossover is fixed at 500 Hz.
- Compression: the Maximus of `Data/Compression/<CompType>.fst` (a Maximus
  state of 1246 to 1342 bytes each), with `ProcessParam(5, CompVol)` its LMH mix, run in
  place on the sum.

## 9. What `harmless.rs` builds

Exact as read: the state and every version's corrections, the colours, the
EQ bands, the detune and grit tables, the mask, the 36 shape curves and
their lookup (interpolated; FL's nearest-point mode is not wired to a
setting), filter range and cutoff maths, resonance and its motion tracking,
phaser (types 0..8), harmonizer, pluck, protection, subs and
self-oscillation, the partial count and the bank's frequency weights, the
sine bank and its 10 ms level glide, the LFO shapes, the vibrato and pitch
LFO, the unison spreads, the attack, decay and release with their
declick, the host's volume and pan gains and their capped ramp, and the
compression through `maximus.rs`.

The chorus, delay and reverb are FL's classes from `fl_fx.rs` behind the
front end of section 8, told a block of 256 frames (the chunk the plugin
renders in): that sets their sleep and ring sizes, which FL takes from the
host. The delay has room for its longest time down to 20 BPM, and is only
made when the state has it on.

Approximated: the tremolo's smoothing; mono, legato and portamento (mono
releases the voices before, portamento glides in a straight line); slide
notes; phaser type 9 and resonance type 8 (played as plain curves).

Left to the host: slide notes, which FL plays by sliding the host levels
(`FinalLevels`) of the voices that sound and which fl-rs does not send a
generator; the order voices are summed in (FL's voice list, here the
slots), and so the last bit of the sum; the threaded render, which sums
the same voices.
