# Fruity PanOMatic: state, parameters, signal chain

Recovered from `Fruity PanOMatic_x64.dll` (FL Studio 2026, Delphi native
plugin, image base 0x400000, no engine DLL beside it). Ghidra project
`ghidra/proj`, decompiles under `ghidra/out/panomatic/` (`vmt/` the SDK slots
with callees, `misc/` the clamp and ln helpers plus the base constructor,
`asm/` listings of `Eff_Render` and the LFO speed routine). Corpus: 193 uses in
the 165 `.flp`, 1 shipped preset (`Default.fst`) plus the three template
`Fruity PanOMatic.fst`. Five Ghidra runs: import, slots, helpers, listings, the ln helper.

The plugin is tiny: a 2x2 gain matrix fed by a pan and a volume, both of
which an LFO can move. The LFO reads the host's wavetables, runs once per
render block and the matrix ramps over the block. The only maths it shares
with other Fruity plugins is the `(11^x - 1)/10` volume law.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `inferred`, `asm`.

## How the plugin is wired

`CreatePlugInstance` @ `0x006c5bd0` -> constructor `FUN_006c6240`, class
`TTestPlug`, VMT `0x006c5a28` (instance size 0x190, parent
`TDelphiFruityPlug` VMT `0x006bf228`).

| slot | SDK name | address |
|---|---|---|
| 0 | DestroyObject | 0x006c64a0 |
| 1 | Dispatcher | 0x006c5c10 |
| 2 | Idle_Public | 0x006c2c70 (empty) |
| 3 | SaveRestoreState | 0x006c6180 |
| 4 | GetName | 0x006c6b80 |
| 5 | ProcessEvent | 0x006c2300 (base) |
| 6 | ProcessParam | 0x006c6650 |
| 7 | Eff_Render | 0x006c6cc0 |

Object fields the audio path touches:

| offset | type | meaning |
|---|---|---|
| 0x120 | i32[6] | the parameters, saved (section 1) |
| 0x138..0x147 | f32[4] | current matrix (ramp state): LL, RR, RL-into-L, LR-into-R |
| 0x148..0x157 | f32[4] | target matrix, written by `FUN_006c6550` |
| 0x158 | f32 | pan, -1..1 |
| 0x15c | f32 | volume gain, linear |
| 0x168 | u32 | LFO phase, 2^32 per cycle |
| 0x16c | u32 | LFO phase increment per sample |
| 0x170 | f32 | LFO amount, scaled for the target |
| 0x174 | ptr | the wavetable in use (host `WaveTables[shape]`) |
| 0x17c | u8 | matrix dirty flag |
| 0x180 | f32 | `2048 / SR`, set by `FPD_SetSampleRate`, 0.04644 at construction (44.1 kHz) |
| 0xbc | ptr | host (`TFruityPlugHost`); `host+0x18` is the `WaveTables[10]` array of the SDK |
| 0xcc | ptr | engine info block; `+0x34` is the per-sample ramp step cap |

## 1. State layout (event 213: 24 bytes, one version)

`SaveRestoreState` (`FUN_006c6180`): save writes `this+0x120, 0x18`; restore
reads the same 24 bytes, then runs the GetValue/UpdateValue loop over the six
parameters (`vtable+0x30` with flags 2 then 0x11). No version word, no size
switch: every corpus block and every preset is 24 bytes.

| byte | type | idx | parameter (DFM object, hint) | range | default | corpus (197 blocks) | confidence |
|---|---|---|---|---|---|---|---|
| 0..3 | i32 | 0 | Pan (`PanWheel`), 128 = centre | 0..256 | 128 | 128 x187, 119 x9, 127 (Default.fst) | loader, dfm, corpus, preset |
| 4..7 | i32 | 1 | Volume (`VolWheel`), 256 = unity | 0..320 | 256 | 256 x128, 0 x17, 239 (Default.fst), 60 others 10..222 | loader, dfm, corpus, preset |
| 8..11 | i32 | 2 | LFO shape (`LFOShapeSelect`): 0 Sine, 1 Triangle, 2 Square | 0..2 | 0 | 0 x187, 1 x7, 2 x3 | loader, dfm, corpus |
| 12..15 | i32 | 3 | LFO target (`LFODestSelect`): 0 OFF, 1 PAN, 2 VOL | 0..2 | 0 | 0 x177, 1 x14, 2 x3 | loader, dfm, corpus |
| 16..19 | i32 | 4 | LFO amount (`LFOAmtWheel`) | 0..128 | 64 | 64 x177, 87 x6, 128 x2, 20..105 | loader, dfm, corpus |
| 20..23 | i32 | 5 | LFO speed (`LFOSpdWheel`) | 0..55000 | 27500 | 27500 x177, 45614 x6, 55000 x2, 40490..53245 | loader, dfm, corpus |

The names come from the string tables at `0x00717958` (shapes) and
`0x00717940` (targets, "OFF", "PAN", "VOL"). The one use of shape 2 with
target 2 in the corpus is a square-wave tremolo.

Preset check (`Default.fst` is the only shipped preset; the three
`Fruity PanOMatic.fst` templates hold the DFM defaults 128/256/0/0/64/27500):

| preset | Pan | Vol | Shape | Target | Amt | Speed | reading |
|---|---|---|---|---|---|---|---|
| Default | 127 (-0.8 %) | 239 (-1.5 dB) | Sine | OFF | 64 | 27500 | a hair left and a hair quiet; the LFO is off |
| template | 128 | 256 (0 dB) | Sine | OFF | 64 | 27500 | unity pass-through |
| corpus x6 | 128 | 256 | Triangle | PAN | 87 | 45614 | auto-pan, 68 % width |
| corpus x2 | 128 | 256 | Sine | PAN | 31 | 46225 | gentle auto-pan |
| corpus x17 | 128 | 0 | Sine | OFF | 64 | 27500 | volume 0: a mute |

## 2. Parameter semantics (`ProcessParam` @ 0x006c6650)

`REC_FromMIDI` (flag 0x20) rescales with the wheel's Min/Max and 2^-30
(`_DAT_006c6968`). `REC_UpdateValue` stores the integer and then:

```
idx 0 pan     0x158 = (p0 - 128) / 128                      // 0x70523c = 1/128
idx 1 volume  x = p1 / 256                                  // 0x705240 = 1/256
              0x15c = x == 1 ? 1 : (11^x - 1) / 10          // exp(x * K), K at 0x72746c (.bss; ln 11 inferred), 0x705230 = 0.1
idx 2 shape   0x174 = host->WaveTables[p2]                  // host+0x18 + p2*8
idx 3,4 amt   0x170 = p3 == 1 ? p4 / 128 : p4 / 256          // full amount swings pan by +-1, or volume by +-0.5
idx 5 speed   FUN_006c64b0 (below)
always        0x17c = 1 (matrix dirty)
```

Volume: 0 is silence, 128 is -12.7 dB, 256 is unity, 320 is x1.9 (+5.6 dB).
The same curve FL uses for insert volumes and the Reeverb 2 dry knob.

Pan law (`FUN_006c6550(pan p, vol v)`, writes the target matrix
`[c0, c1, c2, c3]` at 0x148):

```
p < 0:  c1 = (1 - |p|) v;  c2 = |p| v / 2;  c0 = v - c2 = (1 - |p|/2) v;  c3 = 0
p >= 0: c0 = (1 - p) v;    c3 = p v / 2;    c1 = v - c3 = (1 - p/2) v;    c2 = 0
```

With the shared mixer convention `outL = c0 L + c2 R, outR = c1 R + c3 L`
(the Reeverb 2 matrix, `inferred`: the mix function is reached through the
CPU-dispatch pointer `PTR_DAT_00718428 -> 0x7274f0`, a `.bss` slot that is
not resolvable statically) this gives

```
pan left  (p < 0):  outL = v [(1 - |p|/2) L + (|p|/2) R];   outR = v (1 - |p|) R
pan right (p > 0):  outL = v (1 - p) L;                      outR = v [(1 - p/2) R + (p/2) L]
```

Hard left is `v (L + R) / 2` on the left and silence on the right: the
vanishing channel is folded into the surviving one at half gain, so a mono
source keeps its level and a stereo source loses nothing. Centre is a plain
gain.

LFO speed (`FUN_006c64b0`, listing in `asm/`, read against the decompile):

```
i    = 65536 - p5                                  // 65536 .. 10536
P    = round((50001^(i / 65536) - 1) * i * (2048 / SR))     // 0x6c6530 = 50001, 0x6c6538 = 65536, 0x6c6540 = 1, 0x6c5ef0 = 2048
0x16c = 2^32 / P                                   // FUN_005ae800: increment per sample
```

In detail (`0x006c64b0..0x006c652e`): `d = float(Exp(double(float(i /
65536)) * Ln(50001)) - 1.0)`, then `float(d * float(i)) * 0x180` in singles,
rounded by `CVTSD2SI` (half to even, `FUN_00409d00`); `FUN_005ae800` is
`MOV EDX,1; XOR EAX,EAX; DIV ECX`, so `2^32 / P` truncated. `0x180` is
`float(2048.0 / double(SR))`, written by the dispatcher's case 4
(`0x006c5e2e`), `FPD_SetSampleRate`, whose value is the rate.

So `P` is the period in samples and the frequency is `SR / P`. At 44.1 kHz:
p5 = 0 gives 0.29 mHz (an hour per cycle), 27500 (the default) 0.047 Hz
(21 s), 45614 (the corpus favourite) 1.1 Hz, 55000 19.2 Hz. Because `P`
carries `1/SR`, the frequency scales with SR^2: the same project runs the
LFO 18 % faster at 48 kHz than at 44.1 kHz. That is what the listing says
(`CVTSI2SS i; MULSS (d-1); MULSS [rbx+0x180]`), and `0x180` is set from the
rate the host sends, so it is what FL does, meant or not. The GUI shows the
knob as a percentage of 55000, not as Hz.

Dispatcher (`FUN_006c5c10`): `FPD_Flush` (2) copies the target matrix into
the current one and zeroes the LFO phase; `FPD_SetSampleRate` (4) stores
`0x180 = 2048 / SR` and recomputes the increment; `FPD_ShowEditor` (0) builds
the form. Nothing else touches audio.

## 3. Signal chain (`Eff_Render` @ 0x006c6cc0)

```
if target (p3) == 0:
    if dirty: FUN_006c6550(pan 0x158, vol 0x15c)                // static matrix
else:
    phase 0x168 += 0x16c * n                                     // one LFO step per block, FUN_005ae810 is a plain multiply
    lfo = wavetable[phase >> 18] * amt                           // 16384-entry host table, -1..1
    target 1: FUN_006c6550(clamp(pan + lfo, -1, 1), vol)        // 0x6c6eac = -1, 0x6c6eb0 = 1
    target 2: FUN_006c6550(pan, max(vol + lfo, 0))              // 0x6c6eb4 = 0
for k in 0..3: ramp[k] = FUN_005dfe70(cur[k] (0x138+4k), target[k] (0x148+4k), host step cap, n)
(*PTR_DAT_00718428)(Source, Dest, n, ramp)                      // 2x2 matrix with per-sample ramps
```

The LFO is block-rate: a 512-sample block at 44.1 kHz samples a 19 Hz LFO
only four times per cycle. The ramp (`FUN_005dfe70`) hides that for slow
settings: `step = (target - current) / n`, capped at the host's per-sample
maximum (`info+0x34`); when the cap bites and the new value is below the
denormal floor `DAT_00727434` the coefficient snaps to 0.

The host wavetables (`TFruityPlugHost.WaveTables`, SDK comment: "32Bit
float (-1..1), 16384 samples each; sine, triangle, square, saw, analog saw,
noise") are filled by FLEngine, not by the plugin; their exact shapes were
not read (`inferred` from the SDK comment). Shape 0..2 index the first three.

## 4. What stays unknown

- The orientation of the matrix (which coefficient multiplies which input)
  rests on the shared-library convention; the dispatch pointer lives in
  `.bss`.
- `K = ln 11` for the volume curve is inferred from the same `.bss`
  constant the other Fruity plugins use.
- The host wavetables are the ones `crates/plugins/src/three_osc.rs`
  rebuilds for 3x Osc, read there, not here.
- GUI only: the Dispatcher's window messages, the `Left/Centered` pan hint
  and the dB formatter.

## Corrections

Found while implementing `crates/plugins/src/panomatic.rs` against the corpus:

- LFO speed 45614 is 1.85 Hz, not 1.1 Hz.
- The LFO target counts in the corpus are 163 OFF, 26 PAN and 4 VOL, not 177, 14 and 3.

## Implementation notes (crates/plugins/src/panomatic.rs)

- The LFO period is `FUN_006c64b0` in its order of operations, `Exp` and `Ln`
  by Rust's (they may be an ulp from Delphi's), at every rate.
- `FUN_005dfe70` is `parts::glide`: each of the four gains moves by no more
  than the engine info's `+0x34` a frame, `0.001 * 44100 / SR`
  (`TAudioRenderer`, as Convolver and Delay Bank have it), and snaps to 0 under
  2^-24 when the cap bites. `Matrix::apply_capped` runs it.
