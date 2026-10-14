# Fruity Soft Clipper: state, parameters, signal chain

Recovered from `Fruity Soft Clipper_x64.dll` (FL Studio 2026, Delphi native
plugin, image base 0x400000). Ghidra project `ghidra/proj`, decompiles under
`ghidra/out/softclip/` (`vmt/` holds `CreatePlugInstance` and the five SDK
slots with callees). Corpus: 27 uses in 23 of the 165 `.flp`, one shipped
preset (`Default.fst`). Two Ghidra runs (import + seed dump, VMT slots +
callees); the clipping loop itself is reached through a runtime function
pointer, so it was read from `objdump`.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `inferred`, `asm`, as in
`plugin-fastdist.md`.

## How the plugin is wired

`CreatePlugInstance` @ `0x006be510` -> constructor `FUN_006bebc0` with the
class reference `PTR_PTR_006be328` -> VMT `0x006be3f0` (class `TTestPlug`,
found with the `vmtSelfPtr` scan). Same constructor shape as Balance (Info at
`this+0x10`, GetValue/UpdateValue loop over `vtable+0x30`):

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006be560 |
| 2 | Idle_Public | 0x006ba5d0 |
| 3 | SaveRestoreState | 0x006beb00 |
| 4 | GetName | 0x006bf1f0 |
| 5 | ProcessEvent | 0x006bee50 |
| 6 | ProcessParam | 0x006bee60 |
| 7 | Eff_Render | 0x006bf330 |

`TFruityPlugInfo` at `0x0070e35c`: SDKVersion 1, LongName "Fruity Soft
Clipper" (`0x006be300`), ShortName "Soft Clipper" (`0x006be314`), Flags
0x10400000, NumParams 2.

Object fields:

| offset | type | meaning |
|---|---|---|
| 0x120 | i32[2] | the parameters, saved state: Threshold, Post gain |
| 0x128 | f32 | post gain as a linear factor |
| 0x12c..0x134 | f32[3] | clip state: `T`, `1 - T`, `1 / (1 - T)` |
| 0x138, 0x13c | f32 | output peak L, R for the meter |
| 0x140 | u8 | ProcessMode bit 4 (Dispatcher ID 1) |
| 0x141 | u8 | editor hidden (Dispatcher ID 0 with Value 0); skips the meter |
| 0x142 | u8 | enabled (Dispatcher ID 0xb) |
| 0x143 | u8 | "params changed" flag for the UI |

## 1. State layout (event 213, 8 bytes)

`SaveRestoreState` (`FUN_006beb00`) is a verbatim dump: `Stream.Write(this+0x120,
8)` on save, `Stream.Read(this+0x120, 8)` on restore followed by the
GetValue/UpdateValue loop. Two little-endian int32 in parameter index order.

| byte | type | idx | parameter (DFM object, hint) | range | default | corpus | confidence |
|---|---|---|---|---|---|---|---|
| 0..3 | i32 | 0 | Threshold (`ThresWheel`) | 1..127 | 100 | 100 (x16), 1 (x4), 127 (x2), 83, 102, 108, 117, 122 | loader, dfm, corpus, preset |
| 4..7 | i32 | 1 | Post gain (`PostGainWheel`, Tag 1) | 0..160 | 128 | 128 (x20), 132, 134, 140, 141, 149, 150 | loader, dfm, corpus, preset |

The one shipped preset, `Default.fst`, is `[100, 128]`. Its 201 event names
the plugin "Fruity soft clipper" (lower case, even length), which the
`dump213.py` UTF-16 guess mangles; read from the hex. There is nothing else
to cross-check against. The corpus shows two ways people use it: leave
Threshold at 100 and nudge Post up a little, or drop Threshold to 1 (four
uses) and turn it into a pure exponential saturator, see section 3.

## 2. Parameter semantics (`ProcessParam` @ 0x006bee60)

Flags per the SDK. `REC_FromMIDI` rescales with the wheel's Min/Max and the
2^-30 constant at `0x006bf0b8`. Any update (0x10 or 4 set) sets `this+0x143`
and repaints the curve. `REC_UpdateValue` on index 0 or 1 recomputes:

```
T    = params[0] / 128                      // float 1/128 at 0x006fc260 (via PTR_DAT_0070f378)
FUN_005dfd00(this+0x12c, T, 1.0):           // DAT_006bf0c0 = 1.0f
    s[0] = T
    s[1] = 1.0 - T;  if s[1] <= 0: s[1] = 2^-16 (1.5258789e-05)
    s[2] = 1.0 / s[1]

x    = params[1] / 128
post = x == 1 ? 1 : (11^x - 1) / 10         // Exp(x * Ln 11), Ln 11 in .bss 0x0071e46c (set at 0x005e1ce5..0x005e1cf7 from the double 11.0 at 0x005e22f0), 0.1 at 0x006fc254
                                            // -> this+0x128
```

So Threshold is a plain fraction of full scale (default 0.78125), and Post
gain is the same base-11 curve as Fast Dist and Balance: 128 -> 1.0, 150 ->
1.56 (+3.9 dB), 160 -> 1.90 (+5.6 dB), 0 -> silence.

Display (`FUN_006bf110`, used by `GetName` section 1 and the hint): the
threshold shows `(7^(v/127) - 1) / 6` (Ln 7 in `.bss 0x0071e47c`, float 1/127
at `0x006fc25c`, 1/6 at `0x006fc24c`), which is a cosmetic curve and not the
`v/128` the DSP uses; the post gain shows `v / 160` (double 160.0 at
`0x006bf1e8`). `GetName` section 0 returns the wheel hints "Threshold" and
"Post gain".

## 3. Signal chain (`Eff_Render` @ 0x006bf330)

```
if Source != Dest: copy Source -> Dest                 // dispatched copy, pointer at 0x0070f7a8 -> .bss 0x0071ce90
clip(Dest, Length * 2, this+0x12c)                     // pointer at 0x0070f728 -> .bss 0x0071e5f0
if not editor_hidden (this+0x141 == 0):
    peakL, peakR = max |x| over the block              // FUN_005dfb60, SSE maxps on |x|
    this+0x138 = max(this+0x138, peakL); same for R    // only ever raised; the UI resets it
if post != 1.0 (float 1.0 at 0x006bf3f4):
    ippsMulC_32f_I(post, Dest, Length * 2)             // pointer at 0x0070f3f8 -> .bss 0x0071d0a0
```

`Length * 2`: the stereo buffer is clipped as interleaved mono, per sample,
channels do not interact. The clip routine pointer is filled in at unit init
(`0x005e210d`): `0x005dfd40` by default, `0x005e1ad0` when the CPU flag byte
`[0x0070f760]+0x16` is set (an AVX twin, not read). The scale routine is Intel
IPP's `ippsMulC_32f_I`, resolved by name at `0x005a48f4` (UTF-16 name at
`0x005a8128`); it multiplies the buffer in place.

### The clipping curve (`0x005dfd40`, x87, `asm`)

```
T, oneMinusT, invOneMinusT = state
for each sample x (Length * 2 of them):
    a = |x|
    if a <= T: continue                                   // bit-compare of the positive floats
    u = (a - T) * invOneMinusT                            // 0 at the knee, 1 at full scale
    y = T + oneMinusT * (1 - exp(-u))                     // fldl2e / f2xm1 / fscale = exp
    x = copysign(y, x)
```

Closed form:

```
y(x) = x                                          |x| <= T
y(x) = sign(x) * (T + (1 - T) * (1 - e^(-(|x| - T) / (1 - T))))     |x| >  T
```

What it does. Below the threshold the signal passes untouched. Above it the
excess is squashed through `1 - e^-u`, which starts with slope 1 (the curve is
C1-continuous at the knee) and approaches 1.0 asymptotically, so the output
never exceeds full scale no matter how hot the input. At the default T =
0.78125 a full-scale input comes out at `T + (1-T)(1 - e^-1) = 0.92` (-0.7
dB); +6 dB in (2.0) gives 0.999. With Threshold at 1 (T = 1/128) the curve is
`y = 1 - e^-|x|` from almost zero: a smooth exponential saturator that is
already 4 dB down at 0 dBFS (0.635), which is why four corpus projects pair
it with a raised Post gain. Threshold 127 (T = 0.992) is a near-hard clipper with a
tiny exponential cap.

There is no pre gain. The only way to drive it harder is to lower the
threshold, and the output level is then restored with Post.

Re-implementation note: the curve is cheap and exact as written; the `1 - T`
floor at 2^-16 only matters at T = 1.0, which the wheel cannot reach (max
127). Apply post gain after the clip, as a plain multiply.

## 4. What stays unknown

- The AVX clip twin at `0x005e1ad0` was not read; assumed to be the same
  maths vectorised.
- The dispatched copy at `.bss 0x0071ce90` and the exact IPP variant chosen
  at runtime (the registration at `0x005a48f4` also lists `ippsMul_32fc_I`
  style neighbours; only the in-place scalar multiply makes sense here).
- `Dispatcher` IDs 0x15, 0x33, 0x34, 0x39 are UI plumbing (same set as
  Balance), not read closely. No sample-rate dependence anywhere in the
  audio path, which is expected for a memoryless waveshaper.
- Flags bit 28 of the plugin info.

## Corrections from the implementation

- 28 uses in the shipped projects, not 27.
