# Maximus: state, parameters, signal architecture

Recovered from `Maximus_x64.dll` (FL Studio 2026, Delphi native plugin, image
base 0x400000, no engine DLL beside it). Ghidra project `ghidra/proj`, project
name `maximus` (one project per plugin this pass, so four imports could run
side by side); decompiles under `ghidra/out/maximus/` (`vmt/` the SDK slots
with callees, `dsp/` detectors, map stream, stage coefficients, low cut,
crossover FIR). Corpus: 125 states in the 165 `.flp` (53 of 1375 bytes, 52 of
1342, 9 of 1371, the rest 1351..1462), 45 shipped presets (41 own plus the four
`Harmless/Data/Compression/N.fst`, which are Maximus states: Harmless embeds
a Maximus for its "compression" section). Four Ghidra runs (import, slots,
DSP, detectors). `tools/plugstate/dfm.py` on the DLL gives the
`SynthEditorForm` wheel table; `ghidra/out/maximus/params_dfm.tsv` holds it.

The plugin is a three-band compressor/limiter: a two-way crossover splits the
input into LOW, MID and HIGH, each band has its own detector (peak or RMS
lookahead), a drawn compression curve rasterised into a 32768-entry gain table,
attack/release one-pole stage chains, a lookahead delay, stereo separation and
a saturator; the three bands sum into a fourth MASTER band with the same
engine, and the band mix can be blended against a delay-compensated dry path
(LMH mix). The detector, delay and stage classes are the ones Fruity Limiter
uses (`TPeakLookahead2`, `TRMSLookahead`, `TBasicDelay_32FS`, 0x24-byte
one-pole stages), with the same `(501^x - 1) * 2 ms` time curve.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `inferred`.

## How the plugin is wired

`CreatePlugInstance` @ `0x0083efb0` -> constructor `FUN_008402b0`, class
`TTestPlug`, VMT `0x0083ee68` (instance size 0x868, parent
`TDelphiFruityPlug` `0x00774ee8`).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x0083f010 |
| 2 | Idle_Public | 0x00778a40 |
| 3 | SaveRestoreState | 0x0083f8f0 |
| 4 | GetName | 0x00843a40 |
| 5 | ProcessEvent | 0x00778050 |
| 6 | ProcessParam | 0x00842480 |
| 7 | Eff_Render | 0x00844380 |

Classes: `TMMap` VMT `0x00781920` (the drawn curves; `TMEnvelope`
`0x007815f0`, `TMLFO` `0x00781790`, all over `TCustomMEnvelope` `0x00781408`),
`TPeakLookahead2` `0x007802d8` (process `0x00780cc0`), `TRMSLookahead`
`0x00780470` (process `0x00780fe0`), `TBasicDelay_32FS` `0x0077f4a0`,
`TAudioRenderer` `0x00627a88` (this+0x7c4, holds the sample rate at +8).

Plugin object:

| offset | type | meaning |
|---|---|---|
| 0x12c | i32[7] | globals (REC 0..6, table below) |
| 0x148 | i32[4][13] | band params, 13 per band (REC 7 + 13 b + k) |
| 0x218 | i32[59] | spare state (A/B compare copy of 0x12c..0x218), saved |
| 0x304 | i32 | GUI: `MonitorSel` target |
| 0x308 | i32 | GUI: displayed band (`BandSel`, 0..3) |
| 0x30c | i32 | GUI: display mode; 0x310 u8, 0x311..0x316 u8[6] `VisIBtn`..`VisSBtn`, 0x317 u8 solo, 0x319 u8 "show peaks" |
| 0x31c | i32 | low cut on (> 0); 0x320 i32 (zeroed before load) |
| 0x324 | u8 | (v6+) saved flag: MID band taken from the input rather than the crossover remainder |
| 0x325 | u8 | (v7+) saved flag |
| 0x328 / 0x330 | i32 | (v14+) `SpeedSlider` (default 2 when absent) / visual target |
| 0x32c | u8 | linear-phase crossover (`LinearPhaseModeBtn`), flags bit 0 |
| 0x334 | i32 | which band receives the crossover remainder |
| 0x338 | i32 | latency reported to the host (`FUN_00841340`) |
| 0x33c / 0x340 | f32, u8 | LMH mix = p5 / 1000; mix != 1 |
| 0x341 | u8 | load compatibility: second map replaced by a fresh `TMMap` |
| 0x380 + 0x100 b | band | band records, b = 0 LOW, 1 MID, 2 HIGH, 3 MASTER |
| 0x784 / 0x788 | i32 | silence hold (latency + 1.5 s) / countdown |
| 0x7c0 | i32 | oversampling factor for the three bands (MASTER runs at 1) |
| 0x7c4 | ptr | `TAudioRenderer` (sample rate at +8) |
| 0x7cc / 0x7d4 | ptr | the up and down oversamplers (section 5.7); the low cut is `0x344` |
| 0x7b3 | u8 | editor visible (monitor feed on) |
| 0x80c..0x820 | i32[5] | (v14+) monitor settings, saved: 0x810, 0x814, 0x818, 0x80c, 0x820 |
| 0x82c / 0x834 | ptr | per-sample callback and four handles used by the linear-phase crossover |
| 0x858 | i32 | (v15+) saved, default 1; indexes the ceiling table `DAT_008bbba8` at 0x85c |
| 0x85c | u8 | flags bit 6: ceiling selector |

Band record (0x100 bytes at `0x380 + 0x100 b`):

| offset | type | meaning |
|---|---|---|
| +0x00 | i32 | REC base of this band (7 + 13 b) |
| +0x04 | i32 | mode as set (`BandModeSel`: 0 compress, 1 bypass, 2 mute, 3 ...) |
| +0x08 / +0x0c | i32 | effective mode after solo (`FUN_00842120`): 0 compress, 1 pass with gain, 2 mute |
| +0x10 | f32 | post gain, linear (`gain(p9)`) |
| +0x14 / +0x1c | f32 | pre gain linear (`gain(p8)`) / its copy used by the render |
| +0x18 | f32 | band level into the sum |
| +0x20 | f32 | pre x post x table[0] (display); +0x5c its dB |
| +0x24 | ptr | `TMMap` compression curve (saved first) |
| +0x2c | ptr | `TMMap` envelope map (saved second; `0x341` set: replaced by a fresh map) |
| +0x34 | f32[] | gain table from the curve, 32768 + 1 entries (`FUN_00846490`); +0x3c last valid index; +0x40 f64 ceiling output level; +0x48 f64 ceiling / selector |
| +0x7c / +0x84 | arr, ptr | IIR crossover stages (0x78 bytes each, biquad like Love Philter's) / linear-phase FIR object (`FUN_0083c130`) |
| +0x90 | stage[] | attack stages (`AttTns` of them), state 1.0 |
| +0x98 | stage[] | release stages (`RelTns`), state 0 |
| +0xa0 / +0xa8 | ptr, i32 | lookahead delay / its length in frames |
| +0xac / +0xb0 | i32, ptr | `EnvModeSel` value / detector: 0 `TPeakLookahead2`, else `TRMSLookahead` |
| +0xb8 | f32[] | saturator parameters; +0xcc u8 recompute pending; +0xd4 / +0xd8 clip values; +0xdc i32 sat mode (0 off, < 0 hard clip, > 0 soft) |
| +0xe0 | f32[4] | stereo separation matrix (`FUN_00616320`, the PanOMatic law); +0xf0 previous for the ramp |

## 1. State layout (event 213)

`SaveRestoreState` (`FUN_0083f8f0`):

```
save:    Write(i32 0x000f0003)                        // 15 in the high word, 3 in the low word (the shared Delphi plugin framework's stream tag; Love Philter writes 0xc0003 the same way)
         Write(this+0x12c, 0x1fa)                      // 506 bytes: 7 + 52 params, 59 spare, 34 GUI bytes
         Write(i32 0x328); Write(i32 0x330); Write(u8 flags); Write(i32 x5: 0x810, 0x814, 0x818, 0x80c, 0x820); Write(i32 0x858)
         4 x (comp map.SaveToStream, env map.SaveToStream)
restore: Read(i32 v); v <= 0: keep defaults; v >= 0x10000: v >>= 16
         v < 3:  Read(0x12c, 0x18); 0x144 = 0x8000; Read(0x148, 0xd0); Read(0x218, 0x18); 0x230 = 0x8000; Read(0x234, 0xd0 + (v < 2 ? 0x15 : 0x16))
         else:   Read(0x12c, n) with n = 0x1ee (v3), 0x1f1 (v4), 0x1f5 (v5), 0x1f9 (v6), 0x1fa (v7+); v < 6 zeroes 0x324, v < 7 zeroes 0x325
         v < 14: 0x328 = 2, 0x330 = 0, 0x32c = 0, 0x808 = 1
         v >= 14: Read(0x328, 4); Read(0x330, 4); Read(u8 f): bit0 0x32c linear phase, bit1 0x808, bit2 0x809, bit3 0x81c, bit4 0x7dc, bit5 0x7dd, bit6 0x85c; Read 5 x i32 (0x810, 0x814, 0x818, 0x80c, 0x820)
         v >= 15: Read(0x858, 4) else 0x858 = 1
         4 x (map.LoadFromStream x2), then push GUI fields, GetValue/UpdateValue loop over 59 params, rebuild the four gain tables (FUN_00846490), latency to the host (msg 0x1e)
```

Sizes: v2 1322, v3 1330, v4 1333, v5 1337, v6 1341, v7..v13 1342, v14 1371,
v15 1375 with the default three-point maps (8 maps x 104 bytes = 832). All 125
corpus states and 44 of 45 presets parse to the last byte with this layout
(`/tmp/parse_maximus.py`, throwaway); the one failure is a v11 block of 1346
bytes ("Chillout.flp", riser flangy), four bytes longer than the layout
predicts, so versions 11..13 carried one more i32 somewhere that v14 dropped.
Corpus versions: 15 (58), 10 (57), 14 (9), 7 (2), 11 (1); presets 2..7, 10, 14.

Body of the 506-byte block (`i32` unless noted; the `0x30c..0x325` tail is
GUI state and the two flags):

| byte | field | range | default | corpus (v15) | confidence |
|---|---|---|---|---|---|
| 0..3 | version | | 0xf0003 | | loader |
| 4 | LOW freq (`LowFreqWheel`, REC 0) | 0..65536 | 19800 | 19800 x39, 49240 x3 | loader, dfm, corpus |
| 8 | LOW slope (`LowSlopeSel`, REC 1) | 0, 1 | 1 | 1 x53, 0 x5 | loader, dfm |
| 12 | HIGH freq (`HighFreqWheel`, 2) | 0..65536 | 48065 | 48065 x43 | loader, dfm, corpus |
| 16 | HIGH slope (3) | 0, 1 | 1 | 1 x53 | loader, dfm |
| 20 | LMH lookahead delay (`LMHDelWheel`, 4) | 0..10000 | 1115 (2.0 ms) | 1115 x54, 0 x2 | loader, dfm, corpus |
| 24 | LMH mix (`LMHMixWheel`, 5) | 0..1000 | 1000 | 1000 x56 | loader, dfm, corpus |
| 28 | Low cut (`LCutWheel`, 6; added in v3) | -1..65536 | 32768 | 32768 x39, -1 x8 (off) | loader, dfm, corpus |
| 32 + 52 b + 4 k | band b param k (table below) | | | | loader, dfm, corpus |
| 240..475 | spare copy of 4..239 | | = main | equal to main in every default-ish state | loader, corpus |
| 476..509 | `0x304` i32 monitor target, `0x308` i32 band shown (0..3), `0x30c` i32, `0x310` u8, `0x311..0x316` u8[6] vis buttons, `0x317` u8 solo, `0x318` u8, `0x319` u8, `0x31a..0x31b`, `0x31c` i32 low cut on, `0x320` i32, `0x324` u8, `0x325` u8 | | `32 00 00 00 03 00 00 00 00.. 00 01 01 00 01 00 01 00 00 01 ...` | 25 identical of 58 | loader, corpus |
| 510 | (v14+) `SpeedSlider` | | 2 | 2 | loader, corpus |
| 514 | (v14+) visual target | | 0 | 0 | loader, corpus |
| 518 | (v14+) flags u8 | | | 6 x42, 20 x13, 22 x7, 4 x5 | loader, corpus |
| 519..538 | (v14+) 5 monitor ints | | 3 1 2 2 2 | 3 1 2 2 2 x68 | loader, corpus |
| 539 | (v15+) `0x858` | | 1 | 1 x58 | loader, corpus |
| 543.. | 4 x (comp map, env map), kind 3 blocks | | 3 points each | (3, 3) in 645 of 680 map pairs | loader, corpus |

Band parameters (13 per band, REC `7 + 13 b + k`, byte `32 + 52 b + 4 k`):

| k | DFM object | range | default | corpus notes | confidence |
|---|---|---|---|---|---|
| 0 | `BandModeSel` mode | 0..3 | 0 | 0 x46, 1 x11, 3 x1 (LOW) | loader, dfm, corpus |
| 1 | `PreVolWheel` pre gain | 0..2000 | 1000 | 1000 x43, 1480, 1652, 985 | loader, dfm, corpus |
| 2 | `PostVolWheel` post gain | 0..2000 | 1000 | 1000 x45, 900, 472, 852 | loader, dfm, corpus |
| 3 | `SSWheel` stereo separation | -1000..1000 | 0 | 0 x41, 1000 x7 (LOW mono) | loader, dfm, corpus |
| 4 | `SoftSatThresWheel` | -1000..1000 | 0 | 0 x45, -1000 x7 | loader, dfm, corpus |
| 5 | `SoftSatCeilWheel` | 0..2000 | 1000 | 1000 x53 | loader, dfm, corpus |
| 6 | `AttTimeWheel` attack | 0..10000 | 1115 (2.0 ms) | 1115 x52 | loader, dfm, corpus |
| 7 | `Rel2TimeWheel` release 2 | 0..10000 | 0 | 0 x54, 5944, 6337 | loader, dfm, corpus |
| 8 | `AttTnsWheel` attack / release 2 curve (stages) | 1..8 | 2 | 2 x58 | loader, dfm, corpus |
| 9 | `RelTimeWheel` release | 0..10000 | 5000 (42.8 ms) | 5000 x45, 5944 x3 | loader, dfm, corpus |
| 10 | `RelTnsWheel` release curve (stages) | 1..8 | 3 | 3 x56 | loader, dfm, corpus |
| 11 | `LookaheadWheel` sustain window | 0..10000 | 2882 (10 ms) | 2882 x53 | loader, dfm, corpus |
| 12 | `EnvModeSel` sustain mode | 0 peak, 1 RMS | 0 | 0 x54, 1 x4 | loader, dfm, corpus |

Map block (`TCustomMEnvelope` stream, the same grammar as Love Philter's and
automation-234's): `u32 kind (3)`, `u32 version (3)`, `u32 n`, n x (`f64 x`,
`f64 y`, `f32 tension`, `u8 mode`, `u8`, `u8`, `i8`), then `u32; i32 active;
i32; i32; i32`. The default comp curve has three points `(0, 0) (0.5, 0.5)
(0.5, 0.5)` with the second and third carrying mode byte 2 and the trailer
`0 -1 -1 -1 -1`; `TMMap.LoadFromStream` (`FUN_0078afe0`) is the base loader
plus a recompute (vtable+0x40). The `Data/Maps/*.fnv` files are kind-3 blocks
with the same grammar ("Ratio 2 to 1" is 3 points, "Flat (top)" 2 points).

Preset check (`.fst` files name the plugin "Maximus"; columns per band:
mode, pre, post, SS, sat thr, sat ceil, att, rel2, att curve, rel, rel curve,
window, env mode):

| preset | v | globals (low f, slope, high f, slope, LMH del, mix, low cut) | bands | reading |
|---|---|---|---|---|
| Default | 14 | 19800 1 48065 1 1115 1000 32768 | all `0 1000 1000 0 0 1000 1115 0 2 5000 3 2882 0`, maps default | a transparent three-band with 2 ms lookahead and 10 ms windows |
| Clear master | 7 | same, low cut 25599 | LOW `0 1480 900 ...rel 5944`, MID `0 1456 900 ... 5992, window 2015`, HIGH `0 1424 900 ... 6136, window 1115`; MASTER `1 1000 1000 0 -16 988 ...` | +3.4 dB into each band, post -0.9 dB, the master in pass-through with a soft knee from -16 |
| Clear master RMS | 5 | default | bands `0 1480 852 ... rel curve 2, env mode 1` | the same with RMS detectors and two release stages |
| Breakbeat slam | 4 | low slope 0, high 43969 | LOW `0 1388 1204 0 -624 ...` curve (0,0) (0.3,0.3) (0.7,0.5); MID `... -384 944 ... rel2 3456 ...` curve (0.25,0.25) (0.75,0.5); HIGH curve (0.2,0.2) (0.8,0.5); MASTER `0 952 ... rel2 6096, 6 stages` | drawn knees per band, saturation on low and mid |
| De-esser narrow-band | 4 | low 45912, high 60097, low cut -1 | LOW `1`, HIGH `1`, MASTER `3`: pass; MID `0 ... rel2 5088 rel 5384` curve (0.2,0.2) (0.8,0.3) | only the mid band compresses, with a 3:1-ish drawn curve |
| Gate | 2 | 6 globals (no low cut yet) | LOW/HIGH mode 3, MID 1; MASTER curve (0,0) (0.1,0) (0.1,0.2) (0.3,0.5) (0.5,0.5), rel 5784 / 4 stages | a gate drawn as a curve with a dead zone to 0.1 |
| Distortion (no latency) | 4 | low cut 41984 | all bands mode 1 with post 2000 / 1320 / 1060, sat thr 392 / -228 / 0 / 0, sat ceil 568 / 668 / 1592 / 1000 | no compression at all: the bands are gain + saturator stages |
| Ducking music behind voice | 2 | | MASTER `0 1000 1000 0 0 1000 7483 5184 2 9032 2 7026 0` | a 182 ms attack, 850 ms release, 150 ms window; the sidechain comes from the mixer |

"1 band drums limiter" (bands LOW/HIGH mode 3, MID 1, MASTER pre 1520 with
release 6424 / 4 stages), "Kick pump" (MID attack 7184 ms-curve 6 stages,
release 6120 / 7 stages) and the four Harmless `Compression/N.fst` (v4 to v7,
1345..1441 bytes) complete the sample.

## 2. Parameter semantics (`ProcessParam` @ 0x00842480)

Index decode (`FUN_00842440`): `i < 7` is global `i`; else `b = (i - 7) div
13`, `k = 7 + (i - 7) mod 13`. Flags: 0x20 rescales by the wheel's Min/Max
and 2^-30 (`DAT_00842cc0`), 1 stores at `0x12c + 4 i`, 2 reads, 0x10 refreshes
the control (the band's when `0x308` shows it), 4 formats the hint (band
params 8..19 get "updated on release" appended through the mask
`DAT_00842d32`). The time, gain and sqrt helpers are the ones Fruity Limiter
uses; `FUN_0040bf50` is `ln`, `FUN_0040ad30` is `exp`.

```
gain(v)   = v == 1000 ? 1 : (exp(v/1000 * ln 7) - 1) / 6        // PTR_008bd8b8 = ln 7, PTR_008bd888 = 1/6, PTR_008bcb48 = 1/1000 (the Fruity Limiter curve; 2000 -> +18 dB)
t_s(v)    = (501^(v/10000) - 1) * 0.002                           // DAT_00841c88 = 501, C98 * Ca0 = 2 ms; 1115 -> 2.0 ms, 2882 -> 10 ms, 5000 -> 42.8 ms
samples(v)= round(t_s(v) * SR) * os                               // os = 0x7c0 for bands 0..2, 1 for MASTER
stage(t)  : FUN_006052f0(stage, tA, tR) sets the one-pole coefficients for a 0x24-byte stage (f64 coefA, f64 coefR, f64 state, i32 flag)

global 0, 2  (freqs)      FUN_00845750 (crossover design) and FUN_00848930 under the host lock
global 1, 3  (slopes)     FUN_00848930, FUN_008485d0
global 4     LMH delay    FUN_008415c0 for bands 0..2: their lookahead delay
global 5     LMH mix      0x33c = v / 1000; 0x340 = (0x33c != 1)
global 6     low cut      FUN_00841480: -1 is off (0x342 = 0), else a filter at the wheel frequency on the input (0x344)
band k = 7   mode         band+4 = v; FUN_00842120 resolves solo (0x317 with 0x308 as the soloed band: others forced to 1, and to 2 except MASTER) and clears
                           the monitor history when a band goes silent; then FUN_008415c0 for all bands
band k = 8   pre gain     band+0x14 = gain(v); FUN_00842120; FUN_00842100 (saturator recompute flag band+0xcc)
band k = 9   post gain    band+0x10 = gain(v); band+0x20 = post * band+0x1c * table[0]; band+0x5c = 20 log10(max(that, eps))
band k = 10  stereo sep   FUN_00616320(band+0xe0, 1.0, 1.0, v / 1000, 0): the four-coefficient matrix
band k = 11, 12 saturation  FUN_00842100: marks band+0xcc; the render recomputes (FUN_00841cc0)
band k = 13..18 times     FUN_008415c0 (below) under the host lock
band k = 19  env mode     FUN_00842380: frees band+0xb0 and creates TPeakLookahead2 (0) or TRMSLookahead (1), then FUN_008415c0
```

`FUN_008415c0(band)`:

```
L = band mode < 2 (and for bands 0..2: no band muted... see the guard) ? samples(band == MASTER ? AttTime : LMH delay) : 0     -> band+0xa8, delay.SetLength(L)
attack stages: count = AttTns; t_att = AttTime == 0 ? L : samples(AttTime); tA = t_att * 0.8 / sqrt(n) (DAT_00841ca8 = 0.8, inferred from Fruity Limiter)
               tR2 = Rel2Time == 0 ? tA : samples(Rel2Time) * 2 / sqrt(n);  stage(tR2, tA) for each; state 1.0
detector:      SetLength(max(L, 1, samples(Sustain window)))
release stages: if samples(RelTime) > 0: count = RelTns, state 0, stage(0 (instant attack), samples(RelTime) * 2 / sqrt(n)); else none
latency:       FUN_00841340 -> host msg 0x1e when changed; 0x784 = latency + round(1.5 SR)
```

Gain table (`FUN_00846490(band)`, rebuilt on load and whenever the curve
changes): 32769 floats; entry i is the drawn curve evaluated at the input level
`i * C` on a log scale (`FUN_00788a20` evaluates the map at x,
`exp(y * ln(...))` turns the curve's y back into a linear output level),
divided by the input level: `table[i] = out(level_i) / level_i`. The table
ends (`band+0x3c = i`) at the first entry where the output reaches the ceiling
`DAT_008bbba8[0x85c]`; `band+0x40` holds that output level. Above the table
the render uses `ceiling / level`, a brickwall.

Dispatcher (`FUN_0083f010`): sample rate and process mode as in Fruity
Limiter; not read further.

## 3. Signal architecture (`Eff_Render` @ 0x00844380)

```
FUN_00847e50 (pending recomputes); if Source != Dest copy; silence test over 2n samples (FUN_00629890); countdown 0x788 against 0x784, flush (FUN_00846280) when asleep
work buffers from this+0xd4 (six regions of 0xdc frames): in' 0x34c, A 0x354, B 0x35c, C 0x364 (envelope / gain), D 0x36c, E 0x374
low cut on and not linear phase: FUN_0083ab40(0x344, Dest) in place; in' = order >= 1 ? FUN_0083b340(0x7cc, Dest) (up, section 5.7) : Dest
for b in 0..3 (band = 0x380 + 0x100 b):
    b < 3:  dst = (b == 0x334 ? A : B)                                   // the remainder band gets A
            if not linear phase: run the band's IIR stages (band+0x7c, 0x78 bytes each, PTR_008bdfa8) or the FIR object (band+0x84, FUN_0083c130) from in' into dst
            else: per-sample callback 0x82c(4, handle 0x834[b], &frame) into dst   // external linear-phase crossover
            MID (b == 1) with 0x324 set reads in' directly instead of the crossover
    b == 3: dst = Dest; order >= 1: FUN_0083b3e0(0x7d4, in', Dest) (the band sum down, section 5.7)
    monitor feed (FUN_00843c90) when the editor is open
    if mode < 2: stereo separation matrix ramp (FUN_006160f0 on band+0xe0..0xfc) and apply (PTR_008bd4d0)
    if mode == 0 (compress):
        detector.Process(dst -> C, n)                                     // peak: sliding max of max(|L|,|R|); RMS: sqrt(mean(L^2+R^2)/2) over the window
        attack stages on C (PTR_008bcb40 per stage, in place)
        D = C * (pre gain * K);  E = int(D) (PTR_008bd4f8), clamp >= 0
        for i: idx = E[i]; C[i] = idx > band+0x3c ? (ceiling * band+0x1c) / C[i]            // brickwall above the curve
                                                 : lerp(table[idx], table[idx+1], D[i] - idx) * pre * band+0x1c
        release stages on C
    delay.Process(dst, n)                                                 // lookahead by band+0xa8 frames
    if LMH mix != 1 and b < 3: D = copy of dst (dry path)
    gain: mode == 0 ? dst *= C per frame (PTR_008bcce8) : dst *= pre gain (PTR_008bdcf8)
    saturator (band+0xdc): < 0 hard clip at band+0xd4/0xd8 (PTR_008bce88); > 0 soft (PTR_008be190 with band+0xb8); recompute first when band+0xcc (FUN_00841cc0)
    b < 3: if mix != 1: dst *= mix * band+0x18; dst += D * (1 - mix) * band+0x18   // LMH mix against the delayed dry band
           b == 2: in' = A + B + dst (PTR_008bd460)  else A += dst                   // sum of the three bands lands in in'
           mode 2: dst zeroed
    b == 3: MASTER works in place on the sum; mode 2 zeroes Dest
monitor: FUN_00844010 when the editor shows band levels
```

What the structure means:

- The crossover is two filters (LOW and HIGH) and a remainder: the band
  indexed by `0x334` receives `A`, the others `B`, and the MID band can be
  switched to the plain input (`0x324`). Slopes are the IIR stage counts;
  "Linear Phase" swaps the stages for a FIR object (`band+0x84`) or the
  external per-sample callback.
- Each band's gain is a table lookup indexed by the detector level, so the
  drawn curve is the whole transfer function (threshold, ratio, knee and
  ceiling at once). The envelope follows Fruity Limiter: attack stages before
  the lookup (with "release 2" as their fall time), release stages after it,
  a lookahead delay equal to the LMH delay (or the MASTER's attack).
- Stereo separation is the PanOMatic matrix applied per band; saturation is
  per band and sits after the delay, so it clips the compressed signal.
- The MASTER band is the same engine on the band sum, at the base rate, with
  its own detector, curve, delay and saturator.

## 4. What an implementation needs next

1. Crossover design `FUN_00845750` (303 lines, in `vmt/`): the frequency
   mapping of the 0..65536 wheels, stage counts per slope, and which band is
   the remainder. One run (ConstRefs on the frequency constants plus a
   DisasmAddrs listing).
2. The gain table maths in `FUN_00846490`: the constants `DAT_00846808..850`
   (level scale, the dB base of the map's y) and the ceiling table
   `DAT_008bbba8`. One DisasmAddrs run.
3. The saturator (`FUN_00841cc0` in `dsp/`) and the stereo separation ramp
   (`FUN_006160f0`): read, no run needed.
4. Low cut filter (`FUN_00841480`, `FUN_0083b340`): one run for the
   coefficient function if the decompile lacks it.
5. The monitor (`FUN_00843c90`, `FUN_00844010`) and the Dispatcher's GUI
   messages: skip for audio.
6. The v11..v13 extra i32 (one corpus state): read an old `SaveRestoreState`
   only if that project matters.
7. Linear-phase mode (`0x82c` callback, `FUN_0083c130`): one DecompileCallers
   run to find who installs the callback; expect an FFT convolution.

Estimated: three more Ghidra runs for a complete DSP spec.

## 5. The open items, read (Ghidra runs r5 to r7, `ghidra/out/maximus/r5..r7`)

Constants come from the DLL's constant pool (`tools/plugstate/pe.py`) and
the start-up writers `ripscan.py` found for the `.bss` ones:
`FUN_00852530` fills `0x8d0070 = ln 533.33 / (ln 2 * 65536)` and
`0x8d0078 = ln 16 / (ln 2 * 65536)`, `FUN_0062ddf0` fills `0x8cd4f0 = ln 2`
and the Limiter's `ln 7`, `1/6` set. Implementation:
`crates/plugins/src/maximus.rs`.

### 5.1 Crossover design (`FUN_00845750`), confidence `binary`

```
f_low  = 30 * exp(p0 * ln(533.33) / 65536) = 30 * 533.33^(p0 / 65536)   // 30 Hz .. 16 kHz; 19800 -> 200 Hz, 48065 -> 3 kHz
f_high = the same of p2
s_low  = LOW mode == 3 ? -1 : p1;  s_high = HIGH mode == 3 ? -1 : p3    // band+0x78
if s_low >= 0 and s_high >= 0 and f_high < f_low: swap the two frequencies
0x334  = first band of 0..2 whose mode != 3, else 1                      // gets buffer A
stage arrays (0x78 bytes each; +0 type, +8 freq, +0x10 8 doubles of state, +0x50.. biquad coefficients):
  LOW  (0x3fc): s_low + 1 stages of type 1 (low pass) at f_low with Q index s_low,
                plus one type 5 (allpass) at f_high with Q index 0 when s_high > 0
  MID  (0x4fc): s_low + 1 of type 4 (high pass) at f_low, index s_low; then s_high + 1 of type 1 at f_high, index s_high
  HIGH (0x5fc): s_high + 1 of type 4 at f_high, index s_high
band+0x18 (sum sign): LOW = DAT_008bbc94[s_low], HIGH = DAT_008bbc94[s_high], table = [-1.0, 1.0]; 1.0 in FIR or linear mode
```

`FUN_00624b50(stage, index, renderer)`: `w = 2 pi f / SR` (renderer+0x28),
`alpha = sin w / (2 Q)` with `Q = 2^(-1 / (index + 1))`: index 0 gives
`Q = 0.5` (one 2-pole section a band, Linkwitz-Riley 2nd order, outer bands
inverted), index 1 gives `1 / sqrt 2` (two Butterworth sections, LR4). The
frequency is clamped to renderer+0x30 (not read; above the wheel's 16 kHz
at any rate FL runs). `FUN_00624580(stage, 0, alpha, sin, cos)` is the RBJ
cookbook: type 1 low pass, 4 high pass, 5 allpass (`b0 = a2, b1 = a1, b2 =
1`), with `+0x50 b0, +0x58 b1, +0x60 b2, +0x68 a1, +0x70 a2`. The process
`FUN_00624e90` (SSE build at `0x624dbc`, via `PTR_008bdfa8`) is direct form
I in doubles on interleaved f32 stereo, with a denormal flush per block.

The render runs the MID band's high passes in place on the input buffer
when the IIR path is on (`iVar5 = s_low + 1` in `Eff_Render`), so the HIGH
band's stages take `HP_low(in)`: `LOW = AP_high(LP_low(in))`,
`MID = LP_high(HP_low(in))`, `HIGH = HP_high(HP_low(in))`. With `0x324`
set the MID band's output is the plain input and HIGH takes the input.

The sum is flat to 0.1 dB at the defaults. It is not flat to 1e-4: the LOW
band's allpass has `Q = 0.5` where the MID + HIGH sum is an allpass of
`Q = 0.707`, and the mismatch leaves 0.004 dB at 480 Hz. That is FL's
design, not a rounding error.

### 5.2 FIR crossover (`0x325` set, `0x32c` clear; `FUN_008490d0`), confidence `binary`

`FUN_008414f0`: FIR mode initialises each band's FIR object with
`FUN_0083bf60(obj, 2 channels, 1 = f32, round(SR * 0.004))`, whose tap
count is `(n | 1).max(5)`: 193 at 48 kHz, 177 at 44.1 kHz. IIR mode frees
them (`FUN_0083be90`, +8 = 0).

`FUN_00845750` then designs, with `total` that tap count,
`n = (round(os * 27500 * (slope + 1) / f) | 1).max(5).min(total)` per band
(`DAT_00846268 = 27500`) and `FUN_0083c650(obj, type, f / SR, n, 0,
f2 / SR, n2, 0)`:

```
sinc (FUN_0083c520): h[i] = sin(2 pi f d) / (pi d), d = (float)i - (float)(n-1) * 0.5; centre = 2 f    // n taps centred in total
window (FUN_0062ac70 type 0): Blackman-Harris 0.35875 - 0.48829 cos a + 0.14128 cos 2a - 0.01168 cos 3a, a = 2 pi i / (n - 1)
type 0 (LOW):  h = window(sinc(f_low, n_low))
type 3 (HIGH): h = window(invert(sinc(f_high, n_high)))      // invert (FUN_0083c4c0): negate, +1 at the centre of total
type 1 (MID):  h = invert(window(sinc(f_low, n_low)) + window(invert(sinc(f_high, n_high))))
```

All three are centred in `total`, so they share a group delay of
`(total - 1) / 2` and sum to a delta: the FIR crossover is exactly flat.
`FUN_0083c3b0` converts the doubles to f32 and sets the taps without
clearing the delay line. `FUN_0083c130` runs them: de-interleave, one FIR a
channel, re-interleave. Latency (`FUN_00841340`): `(taps - 1) / 2`, which is
0 in IIR mode.

### 5.3 Gain table (`FUN_00846490`), confidence `binary`

Constants: `DAT_00846810 = 3`, `DAT_00846818 = 0.5`, `DAT_00846820 = 2`,
`DAT_00846828 = 2^-13`, `DAT_00846838 = 2^-24`, `DAT_008bbba8 = [0.975,
1.0]` indexed by `0x85c` (flags bit 6).

```
for i in 0..=32768:  j = max(i, 1); level = j / 8192                      // 0 .. 4.0
    x = 0.5 * log3(1 + 2 level)                                           // x = 0.5 is 0 dBFS, 1.0 is +12 dB; level = (3^(2x) - 1) / 2
    y = curve(x)                                                          // FUN_00788a20, incremental
    out = ceiling * (3^(2y) - 1) / 2
    if |out - ceiling| < 2^-24: out = ceiling; last = the last index that was not at the ceiling
    table[i] = out / level
table[last + 1] = table[last]; band+0x3c = last; band+0x40 = out at i = 32768; band+0x48 = max(out) / ceiling
```

Render: `D = level * (float)(pre * 8192.25)`, `idx = int(D)`; `idx > last`
gives `gain = band+0x40 * post / level` (the brickwall at the curve's last
output), else `gain = lerp(table[idx], table[idx + 1], D - idx) * pre *
post`. So the output is `post * curve(pre * level)`: `band+0x10` is the
pre gain and `band+0x14` the post gain (section 2 had them the other way
round, see Corrections).

### 5.4 The curves (`TMMap`, `FUN_00781bd0`, `FUN_00785940`, `FUN_00788a20`), confidence `binary`

A point's first f64 is the width of the stretch that ends at it, not its
x: the default curve `(0, 0) (0.5, 0.5) (0.5, 0.5)` is points at x = 0,
0.5 and 1.0 with y = 0, 0.5, 0.5, a 1:1 line to 0 dBFS and the ceiling
from there. `y` is kept as a single. The recompute (`FUN_00785940`) turns a
point's tension into `k = sign(t) * (1000001^|t| - 1)` with `a = 1 / |k|`,
`b = ln(|k| + 1)` as singles, and the evaluation bends a stretch as `1 -
((|k| + 1)^(1 - v) - 1) / |k|` above 0 and `((|k| + 1)^v - 1) / |k|` under
it. Modes 11 and 12 are a hyperbola; mode 2 and any unknown mode hold the
start value. These are the formulas of `docs/automation-234.md`, and the
implementation reads the stream with `menvelope::MEnvelope` and draws it
with `dsp::curve`. A point past the last stays at the last height.

### 5.5 Saturator (`FUN_00841cc0`), confidence `binary`

`thr = p[k=11]`, `c = gain(p[k=12])` (the `(7^x - 1) / 6` curve),
`expect = max(reach * post, c)` with `reach = band+0x50 = (env mode 0 and
mode 0) ? max(out) / ceiling : 4.0`.

```
thr > 0:  FUN_00629fd0(band+0xb8, (1 - thr / 1000) * c, c): s0, s1 = max(c - s0, 2^-16), s2 = 1 / s1
          FUN_0062dc70 / FUN_0062a090: |x| > s0 -> s0 + s1 (1 - exp((s0 - |x|) s2)), sign kept   // Fruity Limiter's knee
thr < 0:  k = 20 - 19 ln(1 + 0.1 |thr|) / ln 101                           // 20 at -1 (hard), 1 at -1000 (an arctangent)
          table of 65537 f32 over x in 0 .. expect / c:  atan(x^k)^(1/k) / atan((expect / c)^k)^(1/k)
          FUN_0062d730 / FUN_0062a240: y = c * table[|x| / expect * 65536] (interpolated, clamped at 65536), sign kept
```

So the hard mode is `y = c * atan((min(|x|, expect) / c)^k)^(1/k) /
atan((expect / c)^k)^(1/k)`: the expected peak comes out at the ceiling
and anything above is clipped there. The implementation evaluates that
formula per sample instead of FL's table.

### 5.6 Low cut (`FUN_00841480`, `FUN_0083aa10`, `FUN_0083a940`), confidence `binary`

`0x342 = p6 >= 0`; `f = 5 * 16^(p6 / 65536)` (5 Hz .. 80 Hz, 32768 -> 20 Hz).
Four type-4 (high pass) sections at `w = 2 pi (0.8 f) / SR` with `alpha =
sin w * [0.19771184, 0.61932772, 0.67684454, 1.07935691]`
(`DAT_008bb5b0`). Applied in place on the input before the crossover
(`FUN_0083ab40`), and skipped in linear-phase mode, where the linear-phase
response includes it.

### 5.7 Oversampling (`0x31c`), confidence `binary`, implemented

Ghidra run r8 (`/root/ghidra/maximus` on the cloud box: `out/os`, `out/r2`,
`out/r3`, `out/r3asm`).

`0x31c` is not "low cut on": it is the oversampling order. `FUN_008485d0`
gives it to the two oversampler objects `0x7cc` (up) and `0x7d4` (down)
through `FUN_0083b240(obj, order)`: `obj+0x18 = order` stages, each with
a 0xc0-byte state zeroed by `FUN_0083b2e0`, then `0x7c0 = 1 << order`
(`FUN_0083b2d0`) and the band renderer `0xcc` gets the rate `SR << order`;
then the FIR objects (`FUN_008414f0`), the bands (`FUN_00842120`) and a
flush (`FUN_00846280`). The low cut is a separate object (`0x344`), not
`0x7cc` as section 3 had it.

`Eff_Render`:

```
low cut (0x342 and not linear phase): FUN_0083ab40(0x344, Dest, n)      // at the project rate, in place
order >= 1: in' = work buffer 0xd4; FUN_0083b340(0x7cc, Dest, in', &n)    // n doubles per stage
            else in' = Dest
bands 0..2 on n frames at the band rate (crossover, detectors, delays, saturators, LMH mix)
band 3:  order >= 1: FUN_0083b3e0(0x7d4, in', Dest, &n)                  // the band sum down, n halves per stage
         MASTER on Dest at the project rate
```

`FUN_0083b340` runs stage `order - 1` down to 0, each through the
function pointer `0x8d0018`, writing into the tail of the output buffer so
the last stage ends where the wide block does, and flushes the stage's
denormals (`PTR_008bd1e8`, 12 pairs) after it. `FUN_0083b3e0` the same
with `0x8d0020`, in place, the last stage into Dest. `FUN_0083b500` (unit
init) sets the pointers to `FUN_0083ae10` / `FUN_0083afe0` (SSE2; the
other pair is the "Not implemented in i64/ARM" stub) and `FUN_0083b470`
copies the 13 doubles at `0x008bb648` pairwise (left and right) into a
0xd0-byte table:

```
path A: 0.1473771136010466, 0.48246854276970014, 0.8830050257693731
path B: 0.039151597734460045, 0.3026468483284934, 0.6746159185469639
gain:   0.5
stage:  y = x1 + (x - y1) * c                                    // first-order allpass, doubles, x from f32
up   (FUN_0083ae10): x through A and through B; out[2k] = f32(B), out[2k + 1] = f32(A)
down (FUN_0083afe0): even sample through A, odd through B; out = f32((B + A) * 0.5)
```

These are Parametric EQ 2's `TPolyphase` (its section 3, the same 13
doubles), and the implementation runs `parametric_eq_2::HalfBand` a frame
at a time, which gives the block form to the bit.

What runs at the band rate (`FUN_008415c0`, `FUN_00845750`,
`FUN_008414f0`, with `os = 0x7c0` for bands 0..2 and 1 for MASTER):

- delay `round(t * SR) * os`, detector window `max(delay, 1, round(t *
  SR) * os)`; the attack, release 2 and release times `t * SR * os`
  (unrounded in FL; the implementation rounds at the project rate before
  multiplying, as it did for os 1).
- the IIR sections at `w = 2 pi f / (SR * os)` (`FUN_00624b50` with the
  renderer `0xcc`).
- the FIR crossover: `total = (round(SR * os * 0.004) | 1).max(5)` taps,
  `n = (round(os * 27500 * (slope + 1) / f) | 1).max(5).min(total)`,
  `f / (SR * os)`.
- the stereo separation ramps over the `n * os` frames.

The low cut stays at the project rate, before the upsampler. Latency
(`FUN_00841340`): `(fir_half + max band delay) / os + master delay`,
integer division; the half band adds nothing to it (FL reports none for
its IIR). 17 corpus states have order 1 (the v10 and v11 templates and
one v15), and so do two presets: "Pow" (v7) and "Distortion (no
latency)", whose v4 block ends with the first byte of `0x31c`, which FL
reads as 1. No state asks for more; the implementation runs orders up to
3 (8x) and an order past that at 3, marked `Approximate`.

### 5.8 Linear phase (`0x32c`, `FUN_00847e50`, `FUN_00841340`), confidence `binary`, implemented

Ghidra run r9 (`out/lpasm`) and the engine (`engine_x64.dll` in Frequency
Splitter's folder, objdump listing; `out/eng`, `out/engasm`).

**Where the engine comes from.** The constructor (`0x008403d4..0x8404b0`)
loads `Plugins\Fruity\Effects\Frequency Splitter\engine_x64.dll`
(`FUN_00448710`), takes `mydispatcher` into `0x82c` and calls it with op 0
three times for the band contexts at `0x834 + 8 b`. It is the dispatcher
Parametric EQ 2 uses (`.scratch/notes/fruity-parametric-eq-2.md`). The
handle at `0x800` with ops 0x100..0x108 is the editor's analyser, not the
sound.

**The magnitudes (`FUN_00847e50`, at the start of every `Eff_Render`).**

```
N      = FUN_00847730(0x328, band rate): 0x200 << p; rate < 44100: N / 2; else N << trunc(ln(rate / 44100) / ln 2 + 0.5), at most 65536
bin    = float(double(rate) / N)
f_low  = float(exp(p0 * ln(533.33) / (ln 2 * 65536) * ln 2) * 30)        // not put in order, unlike the IIR design
f_high = the same of p2
s_low, s_high = float([12, 24][the LowSlopeSel / HighSlopeSel switch])  // the editor's controls at 0xd8c / 0xd9c, Tag 1 and 3: params 1 and 3
lc     = float(exp(p6 * ln 16 / (ln 2 * 65536) * ln 2) * 5);  lc -= round(log_1.3(max(lc - 5, 1)))   // FUN_00466580 is LogN
off    = quick_exp2(0.16666667)                                          // a band without sections
window = [0.1, 0.25, 0.01, 0.02][0x330]                                  // DAT_008bbca4, the "Linear phase mod speed" menu
for b in 0..2 with 0x120[b] set and op 0xe(b) == 1:                     // FUN_00848930 sets all three: state load, rate, freqs, slopes
    0x120[b] = 0
    for k in 0..=N/2:
        f = max(0.1f, k * bin)
        if f > float(0.4999 rate): m[k] = 1; continue                   // the renderer's +0x30 (FUN_0062b450)
        m[k] = band b has sections ? curve_b(f) : off
        if p6 > 0 and the low cut has not yet given exactly 1: m[k] *= HP(lc, 24 dB, f)
    m[N/2] = m[N/2 - 1]
    op 5 (band rate), op 6 (window), op 2 (N), op 3 (m)                  // under the host lock
latency: when FUN_00841340 changes, msg 0x1e
```

`curve_b` (`FUN_00847cb0`): LOW `LP(f_low, s_low)`, HIGH `HP(f_high,
s_high)`, MID `HP(f_low, s_low)` when HIGH is off (mode 3), `LP(f_high,
s_high)` when LOW is off, else `HP(f_low, s_low) * LP(f_high, s_high)`.
A band has sections when its slope index `band+0x78` is not -1 (LOW and
HIGH in mode 3) and its stage array is not empty (MID with both outer
bands off).

`FUN_008477b0(fc, slope, f, high)`, all in floats but where noted:

```
L  = quick_log2(f / fc);  high: L = -L
level(o) = db = float(-6.0 - double(slope * o))                   // FL's dB here are sixths of an octave of level
           db < -100: 0;  db > 100: Power(2, db * 0.16666667);  else quick_exp2(db * 0.16666667)
L >= 0: level(L)                                                  // the stop side
L <  0: float(1.0 - double(level(-L)))                            // the pass side: one less the mirrored stop side
quick_log2(x) = (exponent - 127) + float(((t c1) t t - (t c2) t) - t c3 + 1.0),  t = double(1 - mantissa * 2^-23)
                c1 = -0.1556501, c2 = 0.1063157, c3 = 0.739 (doubles)
quick_exp2(x) = x > 0: float(2^n * (r (r (r d1 + d2) + d3) + d4)), n = trunc(x), r = x - n
                x < 0: 1 / quick_exp2(-x);  x = 0: 1
                d1..d4 = 0.079064658211, 0.224328091473, 0.69659850257, 0.999862219597
```

So the LOW and HIGH low and high passes add to 1 at any frequency, -6 dB
at their corner; the three bands add to `1 + LP_low HP_high`, flat to
1e-3 at the default 200 Hz and 3 kHz. This is not the IIR sections'
response: section 5.1's sections are not read here at all.

**The engine context (`mydispatcher` @ `0x1801e1e50`, jump table
`0x1801e2710`).** 0x48 bytes: `+0 N`, `+4` a new magnitude, `+8` the
magnitude (`N/2 + 1` floats), `+0x10 / +0x18` two filters, `+0x20 / +0x28`
the last N frames a side, `+0x30` rate (44100), `+0x34` window (0.1),
`+0x38` frames faded, `+0x3c` fade length (0x1000), `+0x40` the idle
filter, `+0x44` the history place.

```
op 0     new context, then op 2 with 256
op 2     FUN_1801c8850: a new N frees and remakes everything: a zero magnitude, two filters designed from it,
         zero history, idle 0, place 0, faded = fade length (settled), no new magnitude. The same N does nothing.
op 3     copy N/2 + 1 floats into the magnitude; new = 1
op 5, 6  rate, window
op 0xd   zero both histories and both filters' blocks and places (Maximus's flush, FUN_00846440)
op 0xe   faded >= fade length
op 4     FUN_1801c8a70, per frame, in place:
         if new and settled: new = 0; faded = 0; fade length = int(float(rate) * window)
              clear and design the idle filter, prime it with the last N frames from the oldest, idle = !idle
         history[place] = frame; place = (place + 1) % N
         fading: a = filter 0 (frame), b = filter 1 (frame), t = float(faded) / float(fade length)
                 w_a = idle == 0 ? 1 - t : t; out = b * (1 - w_a) + a * w_a; faded += 1
         else:   out = the playing filter (not the idle one) (frame)
```

A filter (`FUN_1801e3040`, 0xa8 bytes: `N`, `N/2`, `N/4`, input blocks,
work, spectrum `+0x38`, two IPP FFT specs forward and one inverse made by
`FUN_1801e1890` with `IPP_FFT_NODIV_BY_ANY`, output `+0x90 / +0x98`):

```
design (FUN_1801e2c90 -> FUN_1801de130, FUN_1801e1960):
    B = FFT(click at N/2);  B[k] *= m[k] for k in 0..=N/2;  B[N-k] *= m[k] for k in 1..N/2
    h = Re(IFFT(B)) * (1 / N)                                       // the zero-phase impulse, N/2 late
    g[i] = h[N/4 + i] * sinf(float(i) * rcp(N/2 - 1) * pi), i < N/2; g[i] = 0 after   // rcpps and one Newton step
    spectrum = FFT(g)
frame (FUN_1801e2ad0): in[side][place].re = x; y = out[side][place]; place += 1
    place == N/2: place = 0; per side: Y = IFFT(FFT(in) * spectrum); out[0..N/2] = out[N/2..N]; out[N/2..N] = 0
                  out[i] += (1 / N) * Re(Y[i]) for i < N                // overlap-add, the input's upper half stays 0
```

A click comes out `N/2 + N/4 = 3N/4` frames late. `Eff_Render` runs op 4
on every frame of bands 0..2 at the band rate (section 3), MID taking the
input itself afterwards when `0x324` is set, and leaves out the IIR low
cut (`0x342 && !0x32c`). The sum signs are 1.0. Latency (`FUN_00841340`):
`max band delay / os + master delay + (3N / 4) / os`, each divided on its
own.

Implementation: `crates/plugins/src/linear_phase.rs` (the context, FL's
sums in their order, a radix-2 float FFT for IPP's) and
`LinearSetting::magnitudes` in `maximus.rs`. What is not FL's to the bit:
the FFT's rounding (IPP's path depends on the CPU), the window's
`rcpps` and vector `sinf` last bits, and the libm `ln` / `exp` of the
wheels. FL sets the flags (`FUN_00848930`) on a state load (`0x840189`), in
the constructor, on the linear-phase button (`0x84aa2c`), the precision,
speed and oversampling menus (`0x83e23b`, `0x83deab`, `0x83e9b7`) and on
globals 0 to 3 (`0x842933`, `0x842960`); a moved low cut or band mode in
linear-phase mode is read with the next of those, and the implementation
does the same. Inferred: it also sets them after a new rate, which
reaches the same reset as a load. Four
corpus states (three in "Astes - Bien Duro", one in "LollieVox & Lezina
J") have the flag.

### 5.9 Soundgoodizer (`Soundgoodizer_x64.dll`, imported into `proj/maximus`), confidence `binary`

An older Maximus build: `TTestPlug` VMT `0x835de8`, bands at `0x36c` with
stride `0xf8`, parameters at `0x120`. State (`FUN_00836960`): `i32
version (3)`, then 8 bytes into `0x334`: `i32 preset`, `i32 amount` (default
600). Restore loads `Data/Presets/<a..d>.dat` next to the DLL through its
own `SaveRestoreState` (`FUN_00836420`, a version 3..7 Maximus loader), then
`ProcessParam(5, amount, 1)`: the amount is the LMH mix. The four `.dat`
are byte for byte the `.fst` states under Maximus's "Soundgoodizer
presets". The implementation (`soundgoodizer.rs`) carries the four presets
as parameters and curves and hosts them through `Maximus`. 97 states in
the corpus projects.

## Corrections

- `band+0x10` is the pre gain (`ProcessParam` k = 8 stores there) and
  `band+0x14` with its copy `band+0x1c` the post gain; the band record
  table and section 2 had them swapped. Bypass mode multiplies by the post
  gain.
- The stages run the other way round: the detector's level goes through
  the release stages (`band+0x98`, instant rise, `coef(2 t / sqrt n)` fall)
  before the table lookup, and the gain through the attack stages
  (`band+0x90`, rising by `coef(release 2)`, falling by `coef(0.8 t_att /
  sqrt n)`) after it, as Fruity Limiter does.
- `0x31c` is the oversampling order, not the low cut switch. The low cut
  switch is `0x342 = p6 >= 0`, computed. `0x7cc` / `0x7d4` are the up
  and down oversamplers, not the low cut; the low cut is `0x344`, on the
  input only.
- `0x325` set with `0x32c` clear is the FIR crossover; 15 v10 and v11
  corpus states have it.
- A curve point's first number is the width of its stretch, not its x.
- The preset table in section 1 is one column off from "SS" on: in
  "Distortion (no latency)" the LOW band has separation 392 and a
  saturation threshold of -744 (the arctangent shaper), and "Breakbeat
  slam" has `-624` as a threshold, not a separation.
- The v11 block of 1346 bytes ("Chillout.flp") has its four extra bytes
  before the curves: versions 11 to 13 wrote a block of `0x1fe` bytes, one
  i32 more (its value is 1) after the two flag bytes. FL 2026 reads `0x1fa`
  for every version from 7 and so starts the first curve four bytes early
  on this state, and reads garbage curves. The implementation tries the
  `0x1fe` block for versions 11 to 13 when the `0x1fa` one does not end
  with the bytes.
- Presets under `Patches/Plugin presets/Effects/Maximus` number 41, the
  four Soundgoodizer ones among them; the 45 counted the Harmless ones.
- Linear phase is on in four corpus states (flags 7 and 21); the flags
  tally in section 1 missed them.
- The stereo separation call is `FUN_00616320(band+0xe0, pan 0, volume
  1, s = v / 1000, 0)`: above 0 both sides get `x / 2` of the other and
  keep `1 - x / 2` with `x = (5^s - 1) / 4` (mono at 1000), under 0 `x = s`
  takes the other side off, which widens.
- The RMS detector is `sqrt(max(0, mean(L^2 + R^2) / 2))` as in Fruity
  Limiter; the `/ 2` is taken from the Limiter's reading, since
  `TRMSLookahead.SetLength` was not decompiled here.
- The linear-phase magnitudes are not the IIR sections' responses, as
  section 5.8 first had it: `FUN_008477b0` draws an analytic -6 dB
  crossover from the wheels and the 12 / 24 dB slope switches, the one
  Frequency Splitter's engine draws, with Maximus's own quick log2 and
  2^x in doubles.

- `FUN_008415c0` takes the attack, release 2 and release times as
  `t * SR * os` without rounding; only the delay and the detector window
  are rounded (then times `os`). The time is
  `float(exp(double(float(v) * 1e-4f) * ln 501) - 1) * 2 ms`, and the
  attack's 0.8 is a single (0.800000011920929). The implementation used
  to round every time with `limiter::frames`; it now works them as FL
  does (`wheel_frames`).
- Moving a slope (globals 1 and 3; `ProcessParam` `0x8428b0`, globals 0
  and 2 go to `0x842907`) calls `FUN_008485d0`, which remakes the
  oversamplers and flushes the plugin (`FUN_00846280`), leaving it
  asleep (0x788 = 0xc0000001) until a block has sound. The
  implementation now does the same.
