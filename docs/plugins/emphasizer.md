# Emphasizer: state, parameters, the shell around its limiter

FL's light mastering limiter: input gain, an emphasis amount, soft or
hard clipping, level matching, bypass and output gain, with peak, gain
reduction and LUFS meters. It is not Emphasis (`docs/plugins/emphasis.md`):
both are Delphi shells over a C++ limiter in `Shared/ildsp_x64.dll`, but
Emphasis makes `Limiter` (`Limiter_CreateInstance`) and Emphasizer makes
`LimiterLite` (`LimiterLite_CreateInstance`). Its own DLL does nothing
to the sound, so it shares no code with Fruity Limiter or Maximus.

Recovered from `Emphasizer_x64.dll` (FL Studio 2026, Delphi native plugin,
image base 0x400000), headless Ghidra (project `emphasizer`, decompiles in
`/root/gh/out/emphasizer/`). The install this was read from has no
`Shared/` directory: `ildsp_x64.dll` is missing, so the limiter itself is
not mapped and not built. Corpus: 1 project state (`Templates/Minimal/Basic
with limiter`, 29 bytes, version 2) and 5 presets of 25 bytes (version 1:
the plugin's "Default" and the four plugin databases' "Emphasizer", all
the same bytes).

Confidence tags: `code` (read in a decompile), `asm`, `dfm`, `corpus`,
`inferred`.

## How the plugin is wired

`CreatePlugInstance` @ `0x006c19c0` -> constructor `0x006c1a30`, class
reference `0x006c1848` (`TTestPlug`, 0x1c0 bytes, parent
`TDelphiFruityPlug`). When the limiter cannot be made (`+0x164` is nil)
`CreatePlugInstance` frees the plugin and returns nil: FL does not load
it.

| slot | SDK name | address |
|---|---|---|
| 0 | destructor | 0x006c1fb0 |
| 1 | Dispatcher | 0x006c20c0 |
| 3 | SaveRestoreState | 0x006c27f0 |
| 5 | ProcessEvent | 0x006c2600 |
| 6 | ProcessParam | 0x006c2980 |
| 7 | Eff_Render | 0x006c3300 |

Object fields (`code`):

| offset | type | meaning |
|---|---|---|
| 0x124 | i32 | the longest block, 0x800 at first, FPD_SetBlockSize sets it |
| 0x128 | i32 | the rate FPD_SetSampleRate gave |
| 0x134, 0x13c, 0x144, 0x14c | f32* | planar in L, R and out L, R, sized by `0x006c31d0` |
| 0x160 | i32 | the latency last reported to FL |
| 0x164 | ptr | the limiter, from `LimiterLite_CreateInstance` (`0x006bfbf0`, looked up by name by `0x005dff40`) |
| 0x16c, 0x19c..0x1a4 | f32 | meters read from the limiter after each block |
| 0x170 | i32[6] | the parameters |
| 0x194, 0x198 | f32 | output peaks L and R |

The limiter's interface, as the shell calls it (`code`; the slots are
byte offsets into its vtable):

| slot | use |
|---|---|
| +0x00 | destroy |
| +0x08 | init(float rate, i32 longest block): from the constructor with the host rate and 0x800, and again by `0x006c1f30` |
| +0x10 | setParam(i32 index, float value) |
| +0x18 | getParam(i32 index) -> float: the constructor sets each wheel from it, so the defaults are the limiter's |
| +0x20 | process(planar in, planar out, i32 frames) |
| +0x28 | FPD_Flush (2) |
| +0x30 | reset of the LUFS meter (`0x006c3550`, on stop and on a jump of the song position) |
| +0x48, +0x50, +0x70 (0, 1) | meters |
| +0x58 / +0x60 | set / get a float switch, 1 or 0: the byte after the parameters in the state |
| +0x68 | latency in frames, reported to FL (`0x006c3460`, host message 0x1e) when it changes |

## 1. State (event 213: 25 or 29 bytes)

`SaveRestoreState` (`0x006c27f0`, `code`): save writes an i32 2, the 0x18
bytes at `+0x170` and one byte, the limiter's switch (`+0x60` == 1.0,
`0x006c35b0`). Load reads the version; at 3 or more nothing else is read.
Under 2 it reads 0x14 bytes (five parameters) and takes the sixth from the
output wheel's default (`editor+0x81c`, `+0x3b4`), which the constructor
set from the limiter; from 2 it reads 0x18. Then the byte, then every
parameter through `ProcessParam(i, GetValue, 0x11)` under the host lock,
then the switch to the limiter (`0x006c3570`, only when it differs).

| bytes | idx | parameter (DFM object, Tag) | range | the presets |
|---|---|---|---|---|
| 0..3 | | version | 1 or 2 | 1; the project 2 |
| 4..7 | 0 | input gain (`GainInWheel`, 0) | 0..10000 | 0 |
| 8..11 | 1 | emphasis (`EmphasisWheel`, 1) | 0..10000 | 5013 |
| 12..15 | 2 | clipping, soft or hard (`SoftClippingBtn`, 2) | 0..1 | 0 |
| 16..19 | 3 | level matching (`LevelMatchBtn`, 3) | 0..1 | 0 |
| 20..23 | 4 | bypass (`BypassBtn`, 4) | 0..1 | 0 |
| 24..27 (v2) | 5 | output gain (`GainOutWheel`, 5) | 0..10000 | 10000 |
| last | | the limiter's switch | 0 or 1 | 1 |

The project state (version 2) is the "Default" preset with the output
gain at 10000. A version 1 state's output gain is the limiter's default;
the port takes it to be 10000, 0 dB, as that project's (`inferred`).

## 2. Parameters (`ProcessParam` @ 0x006c2980, `0x006c2bd0`)

As in the other Delphi plugins, flag 1 stores the value and works out
`share = clamp(double(v - min) * wheel+0x3cc, 0, 1)` (`+0x3cc` is
`1 / (max - min)`); `0x006c2bd0` hands the limiter (`code`):

```
0 input gain   setParam(0, float(0 + (24 - 0) exp(ln(share) / 1)))    // 0x006c1520: 24 share dB
1 emphasis     setParam(1, share)
2, 3, 4        setParam(i, float(v))                                    // clipping, level matching, bypass
5 output gain  g = share == 1 ? 1 : float((exp(share ln 11) - 1) * 0.1f) // 0x006c1dc0
               setParam(5, g > 0 ? float(log10(g) * 20) : -inf)         // dB, 0 at the top
```

The inverse maps (`0x006c2cc0`, from the limiter's values to the wheels,
and `0x006c1e60` for the output: `ln(10^(dB/20) 10 + 1) / ln 11`) agree.
The hint texts (`0x006c2610`) name clipping 0 Soft and 1 Hard.

## 3. Signal chain (`Eff_Render` @ 0x006c3300)

```
deinterleave Source into +0x134, +0x13c        // the routine at PTR 0x00719e28
limiter.process(+0x134, +0x144, frames)        // slot +0x20
interleave +0x144, +0x14c into Dest            // PTR 0x00719ac8
peaks of Dest -> +0x194, +0x198; meters from slots +0x48, +0x50, +0x70
```

Everything that touches the sound is inside `LimiterLite`. The shell adds
nothing: no gain, no dry path, no ramp.

## 4. What stays unknown

- All of `LimiterLite`: its stages, times, the emphasis and clipping
  curves, level matching, its latency, its defaults and what the switch
  byte turns on. It needs `Shared/ildsp_x64.dll`, which an FL install has
  and this one lacks. `docs/plugins/emphasis.md` maps that DLL's `Limiter`
  class, whose stages (`GRAdaptiveRel`, `GRStatic8`) LimiterLite may well
  share; that is to be read, not assumed.

## Implementation notes (crates/plugins/src/emphasizer.rs)

- Every corpus state parses: the project's and the five presets.
- The parameters and the values the shell gives the limiter are built and
  tested; the sound passes through untouched, and `fidelity()` says
  `Approximate` for every state. The port level is Gaps: the limiter is
  the named part left out.
