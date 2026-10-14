# Fruity Stereo Shaper: state, parameters, signal chain

Recovered from `Fruity Stereo Shaper_x64.dll` (FL Studio 2026, Delphi
native plugin, image base 0x400000). Ghidra project `ghidra/proj`,
decompiles under `ghidra/out/stshaper/` (`vmt/` the slots with callees,
`misc/` the filter design and the IPP table, `fns/` the matrix mixer,
`ap/` and `ap2/` the allpass and the constructor). Corpus: 77 uses in the
165 `.flp`, 28 shipped presets (`Mixing/` holds 15 matrix-only ones). Six
Ghidra runs, one over budget: the allpass routine behind the dispatch
pointer turned out to be an "not implemented" stub and the real one needed
its own run. No engine DLL; the vector work is Intel IPP bound by name
(`FUN_005a41a0`), the matrix mixer and the allpass are FL's own SIMD
routines chosen at start-up (`FUN_005e2760`, `FUN_005dea50`).

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `inferred`, `asm`.

## How the plugin is wired

`CreatePlugInstance` @ `0x006cdfb0` -> constructor `FUN_006ce4e0`,
`TTestPlug` VMT `0x006cde90` (instance size 0x218).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006cdff0 |
| 2 | Idle_Public | 0x006cae00 |
| 3 | SaveRestoreState | 0x006ce390 |
| 4 | GetName | 0x006cf210 |
| 5 | ProcessEvent | 0x006ca1d0 |
| 6 | ProcessParam | 0x006ceb50 |
| 7 | Eff_Render | 0x006cf460 |

Object fields:

| offset | type | meaning |
|---|---|---|
| 0x120 | i32[6] | the parameters, saved |
| 0x138 | i32 | difference sidechain output number, saved |
| 0x13c | i32 | effects position: 0 before the matrix, 1 after, saved |
| 0x140 | f32 | delay in units of 10 us (from p4) |
| 0x144 | ptr | the delay object (`TBasicDelay`, 4 bytes per sample: mono) |
| 0x14c..0x158 | f32[4] | matrix targets: L gain, R gain, R-into-L, L-into-R |
| 0x15c..0x168 | f32[4] | matrix ramp state (set by `FUN_005db6e0`) |
| 0x16c | filter | allpass design block: `[0]` type 5, coefficients as f64 at +0x50..+0x70 |
| 0x174 | f32 | allpass corner frequency, Hz |
| 0x17c | f64[8] | allpass state (x1, x2, y1, y2 and spares) |
| 0x1e4 | f32 | `SR / 100000`: 10 us in samples |
| 0x1e8 | i32 | sign of p5: 0 none, +1 right channel, -1 left channel |
| 0x1ec | u8 | matrix dirty |
| 0x1f0 / 0x1f8 / 0x200 / 0x208 | ptr | work buffers: pre-FX copy, scratch, planar L, planar R |

## 1. State layout (event 213, 36 bytes, one version)

`SaveRestoreState` (`FUN_006ce390`): `Write(i32 0); Write(this+0x120, 0x20)`.
Restore reads the version (ignored) and the 32 bytes, pushes the sidechain
number and the position to their selectors, runs the GetValue/UpdateValue
loop. Every corpus block and every preset is 36 bytes.

| byte | type | idx | parameter (DFM object, hint) | range | default | corpus | confidence |
|---|---|---|---|---|---|---|---|
| 0..3 | i32 | | version, always 0 | | 0 | 0 x77 | loader, corpus, preset |
| 4..7 | i32 | 0 | Right into left channel volume (`LRVolWheel`) | -25600..25600 | 0 | 0 x49, 8076 x13, -8076 x10 | loader, dfm, corpus, preset |
| 8..11 | i32 | 1 | Left channel volume (`LVolWheel`) | -25600..25600 | 12800 | 12800 x48, 8076 x22, 0 x7 | loader, dfm, corpus, preset |
| 12..15 | i32 | 2 | Right channel volume (`RVolWheel`) | -25600..25600 | 12800 | 12800 x43, 8076 x18, 0 x10, -8076 x4 | loader, dfm, corpus, preset |
| 16..19 | i32 | 3 | Left into right channel volume (`RLVolWheel`) | -25600..25600 | 0 | 0 x44, 8076 x13, -8076 x10 | loader, dfm, corpus, preset |
| 20..23 | i32 | 4 | Left / right delay (`DelWheel`); sign picks the channel | -4096..4096 | 0 | 0 x42, 1408 x5, 2416 x4 | loader, dfm, corpus, preset |
| 24..27 | i32 | 5 | Left / right dephasing (`APFreqWheel`); sign picks the channel | -4096..4096 | 0 | 0 x61, -2496 x5 | loader, dfm, corpus, preset |
| 28..31 | i32 | | difference sidechain output (`SideChainSelect`, "Difference sidechain output number") | >= 0 | 0 | 0 x77; presets 2 x5 | loader, corpus, preset |
| 32..35 | i32 | | effects position (`FXPosSelect`): 0 effects before the matrix, 1 after | 0, 1 | 1 | 1 x61, 0 x16 | loader, dfm, corpus, preset |

Preset check (`.fst` files name the plugin "Fruity Stereo Shaper"). Gains
through section 2 (12800 is unity, 8076 is 0.5, 25600 is 2.0).

| preset | L | R-into-L | R | L-into-R | Delay | Dephase | SC out | FX pos | reading |
|---|---|---|---|---|---|---|---|---|---|
| Default | 1 | 0 | 1 | 0 | none | none | 0 | after | identity |
| Mid | 0.5 | 0.5 | 0.5 | 0.5 | none | none | 0 | after | both outputs (L+R)/2 |
| Side | 0.5 | -0.5 | 0.5 | -0.5 | none | none | 0 | after | (L-R)/2 on the left, (R-L)/2 on the right |
| LR to MS | 0.5 | 0.5 | 0.5 | -0.5 | none | none | 0 | after | mid left, side right |
| MS to LR | 1 | 1 | 1 | -1 | none | none | 0 | after | the inverse: L = M + S, R = M - S |
| Invert Left | -1 | 0 | 1 | 0 | none | none | 0 | after | polarity flip |
| Left - Right splitter | 1 | 0 | 0 | 0 | none | none | 2 | after | right muted; the difference (the input minus that) goes to sidechain 2, which is the right channel |
| Delay | 1 | 0 | 1 | 0 | R 28.9 ms | none | 0 | after | right channel late by 28.9 ms |
| Stereoize | 1 | 0 | 1 | 0 | none | R 297 Hz | 0 | after | second-order allpass on the right, 180 degrees at 297 Hz |
| Stereoize 3 | 1 | 0 | 1 | 0 | R 4.87 ms | L 128 Hz | 0 | after | delay on one side, phase on the other |
| Phaser | 1 | 1 | 1 | 1 | none | R 591 Hz | 0 | before | mono sum of a dephased pair: a one-notch phaser |
| Shrinked | 1 | 0.51 | 1 | 0.5 | L 12.9 ms | none | 0 | before | cross-feed after a one-sided delay |

The 2-ms-and-under corpus delays (`Phaser 2` is 0.02 ms, `Old` 0.25 ms) are
comb filters once the matrix sums the channels.

## 2. Parameter semantics (`ProcessParam` @ 0x006ceb50)

`REC_FromMIDI` rescales with Min/Max and 2^-30 (`0x006ced68`). On
`REC_UpdateValue`:

```
idx 0..3   matrix dirty (0x1ec = 1); the gains are converted at the top of the next render:
           vol(v) = sign(v) * (|v| / 12800 == 1 ? 1 : (3^(|v|/12800) - 1) / 2)      // FUN_005e0ad0; 0x732474 = ln 3, 0x70f520 = 0.5
           0x154 = vol(p0) (R into L), 0x14c = vol(p1) (L), 0x150 = vol(p2) (R), 0x158 = vol(p3) (L into R)
idx 4      0x140 = (1001^(|p4| / 4096) - 1) * 50                                   // 0x006ced70 = 1001, 0x006ced78 = 1/4096, 0x006ced88 = 50; units of 10 us
           delay length = round(0x140 * 0x1e4) samples   (FUN_006ce970 -> FUN_006cc5b0)   // 0x1e4 = SR / 100000 (Dispatcher, 0x006ce348 = 1e5)
           so delay_ms = (1001^(|p4|/4096) - 1) * 0.5:  0 .. 500 ms; 2416 -> 28.9 ms; the channel is R for p4 > 0, L for p4 < 0
idx 5      0x1e8 = sign(p5)
           p5 == 0: zero the allpass state (ippsZero_8u 0x40)
           else:    f = (5001^(1 - |p5|/4096) - 1) * 4.398 + 10 Hz                     // 0x006ceb28 = 5001, 0x006ceb30 = 4.398, 0x006ceb38 = 10: 22 kHz at 0 .. 10 Hz at 4096
                    FUN_005dbf40(0x16c, 0, host): design the allpass at f (below)
```

Gains at 25600 are 2.0 (+6 dB), at 8076 exactly 0.5, negative values flip
polarity. The display (`FUN_006cedd0`) was not read; the wheels' Min/Max give
the ranges above.

Allpass design (`FUN_005dbf40` -> `FUN_005db970`, type 5 set in the
constructor at `0x006ce4e0+0x16c`): `w = f * host+0x28` (the host's
`pi/SR` or `2 pi/SR` double, not read), `(s, c) = sincos(w)`, `t = s`;
`a2 = (1 - t) / (1 + t)`, `a1 = 2 c / (1 + t)` (sign constant `0x005dbf18`
not read), `b0 = a2`, `b1 = a1`, `b2 = 1`. The stage (`FUN_005dc0a0`, direct
form I in doubles) is `y = b0 x + b1 x1 + b2 x2 - a1 y1 - a2 y2`: a
second-order allpass, 180 degrees of phase shift at `f`. Tag `inferred` for
the sign of `a1` and the exact `w`.

## 3. Signal chain (`Eff_Render` @ 0x006cf460)

```
if sidechain out > 0: ippsCopy_64s(Source, preFX, n)                 // keep the input
if position == 0 (effects first):
    if Source != Dest: ippsCopy_64s(Source, Dest, n)
    FX(Dest)
if matrix dirty: convert the four gains (section 2)
ramp = FUN_005db6e0(0x14c, n): per-frame step towards each target
Dest = matrix(src):  L' = gL L + gRL R,  R' = gR R + gLR L            // FUN_005dfb70, ramped, in one pass; src is Dest or Source
if position == 1: FX(Dest)
if sidechain out > 0:
    ippsSub_32f_I(Dest, preFX, 2n)                                   // preFX = input - output: the "difference"
    host.GetOutBuffer(out, &buf) (host vtable +0x128, flag 0); if buf: ippsAdd_32f_I(preFX, buf, 2n)
    host.GetOutBuffer(out, &buf) (flag 1)                            // hand it back

FX(buf) (FUN_006cf350):
    if delay length > 0: swap one channel's sample with the ring (channel R when p4 > 0, L when p4 < 0)   // a plain delay of that channel
    if dephase sign != 0:
        ippsCplxToReal_32fc(buf, planL, planR, n)                    // deinterleave
        allpass(planR or planL, in place)                            // right for p5 > 0, left for p5 < 0; FUN_005dc0a0
        ippsRealToCplx_32f(planL, planR, buf, n)                     // interleave
```

So: four gains make a 2x2 matrix, one channel can be delayed by up to
500 ms, the other (or the same) channel can be run through one second-order
allpass whose 180-degree point sweeps from 22 kHz (no effect) down to 10 Hz,
and the matrix can sit before or after those two. The "difference" output
sends `input - output` to a mixer track's sidechain, which is what the
splitter presets use: with R muted in the matrix the difference is the right
channel, so one instance splits a stereo pair across two tracks.

Flush: the delay and allpass states are cleared by the Dispatcher's Flush
(`FUN_006ce940`, called from restore as well).

## 4. What stays unknown

- The sign convention of the allpass `a1` and the host's `pi/SR` field
  (host+0x28) were not read from the listing; the structure (second-order
  allpass at `f`) is certain, the exact phase response near `f` is
  `inferred`.
- The matrix ramp cap per sample comes from the host (`FUN_005e0b90`'s
  `param_5`, host+0x34); its value was not traced.
- `GetName` display strings were not read.
- The delay object (`TBasicDelay`, `FUN_006cc5b0`) is a plain ring; its
  process variants were not decompiled.

## Corrections from the implementation (crates/plugins/src/stereo_shaper.rs)

- 25600 maps to 4.0, not 2.0.
- The Side row has the R and L-into-R signs swapped.
