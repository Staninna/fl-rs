# Fruity Squeeze: state, parameters, signal chain

Recovered from `Fruity Squeeze_x64.dll` (FL Studio 2026, Delphi native
plugin, image base 0x400000). Ghidra project `/root/ghidra/fx/proj`
(program `Fruity_Squeeze_x64.dll`), decompiles under
`/root/ghidra/fx/out/sq*/`. Corpus: one use in the 165 `.flp` (NIGHT
TERROR), the render test `Fruity Squeeze.flp`, six presets.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `inferred`.

## How the plugin is wired

`CreatePlugInstance` @ `0x006d2710` -> constructor `FUN_006d31b0`, class
reference `0x006d23b8` -> VMT `0x006d2480` (`TTestPlug`, size 0x1e8).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006d2760 |
| 3 | SaveRestoreState | 0x006d30c0 |
| 5 | ProcessEvent | 0x006ce850 |
| 6 | ProcessParam | 0x006d35c0 |
| 7 | Eff_Render | 0x006d40c0 |

Plugin object:

| offset | type | meaning |
|---|---|---|
| 0x124 / 0x128 | f32 | gate counters: frames held at the amount / at full level |
| 0x12c | u8 | gate holding the amount |
| 0x130 | f32 | gain, `v / 127` |
| 0x134 / 0x138 | f32 | dry `(128 - v) / 128`, wet `v / 128` |
| 0x13c | f32 | crusher steps, `Power(32768, 1 - v / 32768)` |
| 0x140 / 0x144 | f32 | impact / preserve in frames: `(Power(44101, v / 44100) - 1) * 0x1a8` |
| 0x148 / 0x150 | ptr | `TStateVariableFilter` (VMT `0x006d05c0`), left / right |
| 0x158..0x1a0 | ptr[10] | the two filters' five outputs, a block each |
| 0x1a8 | f32 | `rate / 44100` (Dispatcher 4) |
| 0x1ac | i32[12] | the parameters, saved |

## 1. State layout (event 213, 52 bytes)

`SaveRestoreState` writes `i32 0` and `this+0x1ac`, 48 bytes; restore reads
the first word and drops it, reads the 48 bytes and runs the
GetValue/UpdateValue loop.

| byte | idx | parameter (DFM object) | range | default | corpus (render test, NIGHT TERROR) |
|---|---|---|---|---|---|
| 4 | 0 | Gain (`GainSlider`) | 0..160 | 127 | 93, 127 |
| 8 | 1 | Mix (`DryWetSlider`) | 0..128 | 64 | 108, 64 |
| 12 | 2 | Steps (`StepsWheel`) | 1..32768 | 1 | 30080, 12144 |
| 16 | 3 | Filter (`FilterBtn`) | 0, 1 | 0 | 1, 1 |
| 20 | 4 | Filter type (`FilterTypeWheel`) | 0..4 | 0 | 0, 0 |
| 24 | 5 | Filter resonance (`FilterResWheel`) | 0..65536 | 32768 | 17920, 0 |
| 28 | 6 | Filter cutoff (`FilterCutOffWheel`) | 0..65536 | 32768 | 60928, 57151 |
| 32 | 7 | Filter position (`FilterPosBtn`) | 0 before, 1 after | 0 | 1, 1 |
| 36 | 8 | Relation (`RelationWheel`) | 0..3 | 0 | 0, 0 |
| 40 | 9 | Impact (`ImpactWheel`) | 0..44100 | 0 | 2892, 0 |
| 44 | 10 | Preserve (`PreserveWheel`) | 0..44100 | 7500 | 4684, 7500 |
| 48 | 11 | Amount (`AmountWheel`) | 0..65536 | 0 | 0, 0 |

## 2. Parameters (`ProcessParam` @ 0x006d35c0)

`n = v / Max` (no Min taken off; `Max` the wheel's `+0x3ac`):

```
0 gain     0x130 = f32(v * (1/127))
1 mix      0x134 = (Max - v) / Max, 0x138 = v / Max
2 steps    0x13c = Power(32768, f32(1 - n))
5 res      both filters +0x10 = n, then retune
6 cutoff   both filters +8 = n -> fc = clamp(exp(n (ln max - ln min) + ln min), min, max), retune
8 relation FUN_006d4580: 1: p9 = p10, 2: p9 = 2 p10, 3: p9 = p10 / 2 (also after p10 moves)
9 impact   0x140 = (Power(44101, n) - 1) * 0x1a8
10 preserve 0x144 = (Power(44101, n) - 1) * 0x1a8, then the relation
3, 4, 7, 11: stored only
```

`Power` (`FUN_005d1390`) is Delphi's: `IntPower` for a whole exponent,
`exp(e ln b)` otherwise. The filter's range (`FUN_006d0630`) is 50 Hz to
`0.75 * rate`, in logs; a new rate retunes it (`FUN_006d0ab0`).

### The filter (`TStateVariableFilter`, update `FUN_006d09a0`)

```
f = 2 sin(pi * min(fc / (2 rate), 0.25))
q = min(min(2 / f - f / 2, 2), (1 - Power(res, 0.25)) * 2)
per sample, twice:                         (FUN_006d4620)
  x     = flush(x)                         // |x| < 2^-24 -> 0
  notch = flush(x - q band)
  low  += f band
  high  = notch - low
  band += f high
  out  += 0.5 * {low, high, band, notch, low - high}[type]
```

## 3. Signal chain (`Eff_Render` @ 0x006d40c0)

```
if every sample of the block is within 1e-6: Dest = Source; return
work = Source
if filter on and before: filter(work)
if steps < 32768: work = Round(work * steps) / steps     // ties to even
if impact > 0, per frame:                                 // the gate
    holding full level: kept += 1 until kept > preserve, then switch with
        on = kept - preserve, off = 1 - on
    holding the amount: held += 1 until held > impact, then switch back with
        off = held - impact, on = 1 - off
    work *= amount / 65536 * on + off        // amount while held, 1 otherwise
if filter on and after: filter(work)
Dest = (Source * dry + work * wet) * gain
```

Dispatcher 2 (flush) resets the gate. Constants: 1/127 @ `0x006d39e0`,
32768 @ `0x006d39e8`, 44101 @ `0x006d39f8`, 1e-6 @ `0x006d4518`, 1/65536
@ `0x006d4538`, 0.5 @ `0x006d4ec8`, 50 / 0.75 @ `0x006d0b18`, 2, 0.25, pi,
0.5, 2 @ `0x006d0a78..0x006d0aa0`.

## Port notes

Everything above is in `squeeze.rs`. The steps wheel at its bottom (1)
still crushes to about 32758 steps, as FL's does.
