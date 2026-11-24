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
`/root/gh/out/emphasizer/`); the limiter from FL 26.1.7's
`Shared/ildsp_x64.dll` (MSVC, image base 0x180000000), section 4.
Corpus: 1 project state (`Templates/Minimal/Basic
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

## 4. The limiter: `LimiterLite` in `ildsp_x64.dll`

`LimiterLite_CreateInstance` @ `0x1801aa540` makes a 0x10-byte wrapper
(vtable `0x1803dfd80`) around a 0x240500-byte `LimiterLite` (vtable
`0x1803dfd30`, constructor `0x1801aa5e0`, zeroed first). `asm`
`emulator`. The wrapper's slots:

| slot | does | address |
|---|---|---|
| +0x08 | `prepare(rate, block)` | `0x1801aaba0` -> `0x180173610` |
| +0x10 | `setParam(index, value)` | `0x1801aabb0` |
| +0x18 | `getParam(index)`: the stored dB, emphasis, switches; -1 past 6 | `0x1801aade0` |
| +0x20 | `process(in, out, frames)` | `0x1801aae60` -> `0x180173940` |
| +0x28 | flush: `reset` (`0x180173880`): the delays, the holds and the level's one-pole empty | `0x1801aae70` |
| +0x30 | LUFS reset | `0x1801aae80` |
| +0x58 / +0x60 | set / get the LUFS meter's switch (`+0x14008c`) | `0x1801aaf20` / `0x1801aaf30` |
| +0x68 | latency `roundf(rate * 0.002)`: 88 frames at 44.1 kHz | `0x1801aaf40` |

`setParam`:

```
0 input gain   +0x40 = dB; +0x44 = expf(dB * 0.11512925)            // ln 10 / 20, 0x1803e0454
1 emphasis     q = e^2; +0x58 = q; +0x6c = 1 - q / 2                  // the level's share
               release Tr = float(double((1 - q) * 3) + 0.1) s, attack Ta = 4 q + 0.002 s
               the one-pole: up  exp(-2 pi / (rate Ta)), down exp(-2 pi / (rate Tr))  (double exp)
               holds 3.125 ms each (rate * 0.003125)
               releases r0 = expf(float(-2 pi / (rate Tr))), r1 the same over Tr / 4
2 clipping     +0x140080 (0 hard)
3 level match  +0x14007c
4 bypass       +0x140084
5 output gain  +0x4c = dB; +0x50 = expf(dB * 0.11512925)
6 the switch   +0x14008c
```

`prepare` (`0x180173610`) zeroes the input gain, the emphasis, the
switches and every state, sets the glides' pole `exp(-40 pi / rate)` (20
Hz, `+0x70`), both pairs of delays to `trunc(rate * 0.002)` frames (rings
of 2^16, `0x180163db0`), the soft clip's knee 0.8414, span 0.1586 and slope
6.305 (`+0x240350`), and the gain stage `GRStatic1Lite` (`+0x2401f0`,
`0x1801643d0`): eight peak holds (`PeakHold8Lite`, `+0x240230`,
`0x180165d20`) and a one-pole (`ExpSmoother1Lite`, `+0x2402e0`) whose
times are the last the emphasis set (0.01 s up, 0.1 s down before any).

`process` (`0x180173940`, with MXCSR's flush-to-zero and
denormals-are-zero on), per frame unless said:

1. Dry: each side through a 2 ms delay.
2. The input gain glides at 20 Hz; both sides times it.
3. Level: `share * max(|L|, |R|)`.
4. Gain: the eight holds, each keeping the largest it saw for up to its
   length; the one-pole, up when the held level is over its state; `g =
   0.98855 / max(level, 0.98855)` (-0.1 dB). `deepest` is `g` whenever
   `g` did not rise; the one-pole's down coefficient becomes `r1` when
   `deepest / max(held / (2 share deepest), 1) > g^2`, else `r0`.
5. Wet: each gained side through its own 2 ms delay, times `g`.
6. Clip: hard to [-1, 1] (`0x1801658e0`), or soft past the knee,
   `knee + span y / sqrt(y^2 + 1)` with `y = (x - knee) slope`
   (`0x180165330`).
7. The output gain glides at 20 Hz and multiplies both sides.
8. LUFS meters (K-weighting at `+0x240368`, `+0x2403d8`) when the switch
   is on: display only.
9. Level matching: both sides times `1 / input gain` as the glide stands at
   the end of the block.
10. Bypass: a 20 Hz glide to 0 when on, 1 when off (starting at 0), `out =
    ((out - dry) b + dry) * 0.9999`; RMS meters, display only.

## Implementation notes (crates/plugins/src/emphasizer.rs, limiter_lite.rs)

- Every corpus state parses: the project's and the five presets.
- The limiter matches `tools/plugstate/limiter_lite_emu.py` bit for bit
  over three seconds in four settings (the default preset; soft clipping
  with level matching; 24 dB into hard clipping; bypassed), the runtime's
  `expf` taken from its FMA3 path as `low_lifter.rs` transcribes it.
- Left out: the meters (display only), and the flush of denormals that
  the DLL's MXCSR does, which only ever makes a value under 2^-126 zero.
  Level matching reads the input gain at the end of each block of at most
  0x800 frames, the block the shell declares; FL's own blocks are its
  host's.
- A rate change sets the limiter up again and sends every value
  (`inferred`: the shell's rate handler sends them through `ProcessParam`
  as the loader does).
