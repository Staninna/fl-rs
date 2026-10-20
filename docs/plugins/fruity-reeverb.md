# Fruity Reeverb (version 1): state, parameters, signal chain

Recovered from `Fruity Reeverb_x64.dll` (FL Studio 2026, Delphi native
plugin, image base 0x400000). Ghidra project `/root/ghidra/fx/proj`
(program `Fruity_Reeverb_x64.dll`), decompiles under
`/root/ghidra/fx/out/reeverb*/`. Corpus: seven uses in one of the 165
`.flp` (Exoplanet), the render test `Fruity Reeverb.flp`, 21 shipped
presets (`Internal/`, `External/`). Not the same plugin as Reeverb 2.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `inferred`.

## How the plugin is wired

`CreatePlugInstance` @ `0x006ba180`, class reference `0x006b9f98` -> VMT
`0x006ba060` (`TTestPlug`, size 0x320).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006ba1d0 |
| 2 | Idle_Public | 0x006b7eb0 |
| 3 | SaveRestoreState | 0x006ba4f0 |
| 4 | GetName | 0x006bb540 |
| 5 | ProcessEvent | 0x006b7540 |
| 6 | ProcessParam | 0x006baa00 |
| 7 | Eff_Render | 0x006bc150 |

Plugin object:

| offset | type | meaning |
|---|---|---|
| 0x120 | f32[10] | each wheel as 0..1; 0x140 dry, 0x144 reverb |
| 0x148 | i32 | low cut, Hz: `trunc(n * 2981 + 19)`, 19 = off |
| 0x14c | i32 | high cut, 100 Hz: `trunc(n * 216 + 5)`, 221 = off |
| 0x150 | i32 | decay, 0.1 s: `trunc(n * 199 + 1)` |
| 0x154 | i32 | high damping, 100 Hz: `trunc(n * 216 + 5)`, 221 = off |
| 0x158 | i32 | colour 0..4: `Round(v * 0.1)` on the raw wheel ("Brighter" ...) |
| 0x15c | i32 | diffusion, %: `trunc(n * 100)` |
| 0x160 | i32 | room size 1..100: `trunc(n * 99 + 1)` |
| 0x164 | i32 | predelay, ms: the raw wheel |
| 0x168 | u8 | coefficients to be worked out before the next block |
| 0x16c | f32 | input gain 0.5 |
| 0x170 / 0x174 | f32 | dry / wet |
| 0x178 | f32 | high cut state |
| 0x17c / 0x180 | f32 | low cut allpass output / input |
| 0x184 | f32 | low cut coefficient |
| 0x188 | f32 | decay gain per tank delay |
| 0x18c / 0x190 | f32 | decay diffusion 1 / 2 |
| 0x194 / 0x198 | f32 | input diffusion (0x198 is worked out and never read) |
| 0x19c | f32 | high cut coefficient |
| 0x1a0 | f32 | damping coefficient |
| 0x1a4 / 0x1a8 | f32 | damping states, left and right half |
| 0x1ac / 0x1b0 | f32 | colour shelf: allpass coefficient / gain |
| 0x1b4..0x1c0 | f32[4] | shelf allpass states (x, y) per half |
| 0x1c4 | ptr | one block for all lines; 0x1cc its length |
| 0x1d0 | ptr[12] | the twelve lines; 0x230 the predelay |
| 0x240 | i32[13] | positions (they run downwards); 0x278 i32[13] lengths |
| 0x2b0 / 0x2cc | i32[7] | output tap offsets, left / right |
| 0x2e8 | i32 | the room the lines were last cleared for |
| 0x2ec | i32[10] | the parameters, saved |

## 1. State layout (event 213, 44 bytes)

`SaveRestoreState` writes `i32 1` and `this+0x2ec`, 40 bytes; restore reads
the version, leaves the plugin alone above 1, then reads the 40 bytes and
runs the GetValue/UpdateValue loop.

| byte | idx | parameter (DFM object) | range | default | confidence |
|---|---|---|---|---|---|
| 4 | 0 | Low cut (`LowCutWheel`) | 0..65536 | 1250 | loader, dfm |
| 8 | 1 | High cut (`HighCutWheel`) | 0..65536 | 10800 | loader, dfm |
| 12 | 2 | Predelay (`PredelayWheel`) | 0..250 | 0 | loader, dfm |
| 16 | 3 | Room size (`RoomSizeWheel`) | 0..65536 | 26000 | loader, dfm |
| 20 | 4 | Diffusion (`DiffusionWheel`) | 0..65536 | 65536 | loader, dfm |
| 24 | 5 | Color (`ColorWheel`) | 0..40 | 20 | loader, dfm |
| 28 | 6 | Decay (`DecayWheel`) | 0..65536 | 4700 | loader, dfm |
| 32 | 7 | High damping (`HighDampingWheel`) | 0..65536 | 10800 | loader, dfm |
| 36 | 8 | Dry (`DryWheel`) | 0..65536 | 65536 | loader, dfm |
| 40 | 9 | Reverb (`ReverbWheel`) | 0..65536 | 13120 | loader, dfm |

`n = clamp(v / 65536, 0, 1)` in f32 (`ProcessParam` @ `0x006baa00`,
`FUN_006bac20`). Constants: 2981/19 @ `0x006bad84`, 216/5 @ `0x006bad8c`,
99/1 @ `0x006bad94`, 100 @ `0x006bad9c`, 199 @ `0x006bada0`, 0.1f @
`0x006f9208`.

## 2. Coefficients (`FUN_006bb8d0`, run at the start of a block when dirty)

```
low cut   f <= 19: c = 0; else f = min(f, rate/2); t = tan(f pi / rate); c = (t-1)/(t+1)
high cut  v >= 221: 1; else b = f32(2 - cos(v * 200 pi / rate)); 1 - (b - sqrt(b*b - 1))
predelay  max(1, MulDiv(ms, rate, 1000)) frames
scale     MulDiv(rate, room + 15, 115)
lines     max(1, MulDiv(LINES[k], scale, 20000)), taps MulDiv(TAPS[k], scale, 20000)
          a new room zeroes each line's first `len` samples (ippsZero_32f)
diffusion d = diff / 100; d == 0: all four 0; else, with L = ln 0.001:
          in   = exp(L * f32((len0 + len1)/2 / rate) / (d * 0.07))
          (in2 = ... len2, len3, 0.11: not used by the render)
          dec1 = ... (len4 + len8)/2, 0.35;  dec2 = ... (len6 + len10)/2, 0.51
decay     T = f32(len5 + len7 + len9 + len11) / rate; s = decay * 0.1
          g = Power(exp(L T / s), 0.25)                       // 60 dB over s, four delays a lap
colour    k = [0.2, 0.6, 1, 1.5, 3][colour]; r = Power(exp(L T / (s k)), 0.25) / g
          shelf gain = (r - 1) / 2; r = min(r, 1); t = tan(500 pi / rate); shelf c = (t - r)/(t + r)
damping   as high cut, from 0x154
```

`MulDiv` (`FUN_005ae880`) truncates. Tables: `LINES` at `0x0070aff8`
(142, 107, 379, 277, 672, 4453, 1800, 3720, 908, 4217, 2656, 3163), taps
at `0x0070b028` (266, 2974, 1913, 2496, 1690, 187, 1566) and `0x0070b044`
(353, 3627, 1728, 3173, 1811, 335, 621). Other constants: pi, 628.3185,
2, 1, 0.5, 0.001, 0.07, 0.11, 0.35, 0.51, 0.25, 500 at `0x006bc0c8..`.

## 3. Signal chain (`Eff_Render` @ 0x006bc150)

Lines run downwards: a position steps down by one after each use and wraps
to `len - 1`. A lattice allpass is `v = line - g x; line = x + g v; out v`.

```
tap gains, per block: g6, g25, g7, g5, g2 = g + (1 - g) * {0.6, 0.25, 0.7, 0.5, 0.2}
per frame:
  x  = f32((L + R) * 0.5 + 1e-30); hc += (x - hc) * high cut
  low cut: y = (hc - y) * c + x1; x1 = hc; v = c == 0 ? hc : f32((hc - y) * 0.5 + 1e-30)
  v  = allpass x4 (lines 0..3, all with `in`)
  p  = predelay[pos]; predelay[pos] = v
  for each half (left: lines 4..7, fed from line 11; right: lines 8..11, fed from line 7):
      f  = (other half's last line, read after its write) * g
      shelf: y = (f - y) * shelf c + x; x = f
      damp += ((f + (f + y) * shelf gain + p) - damp) * damping
      u  = allpass(line 4|8, dec1, damp)
      u  = swap(line 5|9, u) * g
      u  = allpass(line 6|10, dec2, u)
      line 7|11 = u (then step)
  L' = L * dry + ((((t9[a0] - t9[a1] g25) - t10[a2] g7) - t11[a3] g2) + t5[a4] g6 + t6[a5] + t7[a6] g5) * wet
  R' = R * dry + ((((t5[b0] + t5[b1] g25) - t6[b2] g7) - t7[b3] g2) - t9[b4] g6 + t10[b5] + t11[b6] g5) * wet
```

`tK[o]` is line K at its current position plus `o`, wrapped once.

Flush (Dispatcher 2, and 0xb with a value, `FUN_006ba400`) fills every line
with 1e-30 (`0x0da24260`) and zeroes the filter states. The rate
(Dispatcher 4, `FUN_006bb600`) lays all lines out for the largest room.

## Port notes

Every part above is in `reeverb.rs`. FL's single block of memory is kept;
the lines sit where FL puts them for the largest room.
