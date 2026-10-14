# Fruity Balance: state, parameters, signal chain

Recovered from `Fruity Balance_x64.dll` (FL Studio 2026, Delphi native plugin,
image base 0x400000). Ghidra project `ghidra/proj` (program `Fruity
Balance_x64.dll`), decompiles under `ghidra/out/balance/`. Corpus: 57 uses in
the 165 `.flp`, one `.fst` preset. Four Ghidra runs were spent on this plugin
(import + seed dump, `CreatePlugInstance` + callees, the five SDK vtable methods
+ callees, the gain-apply routine).

Confidence tags: `loader` (read from `SaveRestoreState`/`ProcessParam`),
`dfm` (the Delphi form resource in `.rsrc`, which holds wheel Min/Max/Default),
`corpus` (observed in the 165 projects), `inferred`.

## How the plugin is wired

The way in is the export. `CreatePlugInstance` @ `0x006bdde0` calls the Delphi
constructor `FUN_006be800` with the class reference at `0x006bdbf8`, which
holds the VMT address `0x006bdcc0`. (That pointer sits at VMT-200, the Delphi
x64 `vmtSelfPtr`, so the same trick finds any Delphi class.) The constructor
stores the `TFruityPlugInfo` pointer at `this+0x10`, then for each parameter
calls `vtable+0x30` with `REC_GetValue` and again with
`REC_UpdateValue|REC_UpdateControl`, which pins `vtable+0x30` as
`ProcessParam` and fixes the SDK slot order:

| slot | offset | SDK name | address |
|---|---|---|---|
| 0 | 0x00 | DestroyObject | 0x006b92f0 |
| 1 | 0x08 | Dispatcher | 0x006bde30 |
| 2 | 0x10 | Idle_Public | 0x006ba140 |
| 3 | 0x18 | SaveRestoreState | 0x006be740 |
| 4 | 0x20 | GetName | 0x006bee90 |
| 5 | 0x28 | ProcessEvent | 0x006b97d0 |
| 6 | 0x30 | ProcessParam | 0x006beaa0 |
| 7 | 0x38 | Eff_Render | 0x006befc0 |
| 8..19 | | Gen_Render and voice methods | 0x00409ec0 (abstract-error stub) |

`TFruityPlugInfo` at `0x0070e3cc` is packed: SDKVersion 1 @0, LongName
"Fruity Balance" @4, ShortName "Balance" @0xc, Flags 0x10400000 @0x14,
NumParams 2 @0x18. Bit 22 is `FPF_Reserved1` (Delphi build); bit 28 is not in
the public header.

Object fields that matter (all offsets from `this`):

| offset | type | meaning |
|---|---|---|
| 0x120..0x12c | f32[4] | ramp state: target reached at end of last block, per gain |
| 0x130, 0x138, 0x140, 0x148 | f64 | target gains LL, RR, R->L, L->R (set by ProcessParam) |
| 0x150 | f64 | pan position 0..1 |
| 0x158 | f64 | pan -1..1 |
| 0x160 | f64 | volume fraction v/320 |
| 0x168 | f64 | pan compensation factor |
| 0x170..0x17c | f32[4] | current gains handed to the apply routine |
| 0x180..0x18c | f32[4] | per-sample gain deltas for the current block |
| 0x190 | f64 | constant `1 / (cos(pi/4) * 10)` = 0.141421356, set in the constructor |
| 0x198 | i32[2] | the two parameters, saved state |
| 0x1a0, 0x1a4 | f32 | output peak L, R for the meter |

## 1. State layout (event 213, 8 bytes)

`SaveRestoreState` (`FUN_006be740`) is a verbatim dump: `Stream.Write(this+0x198,
8)` on save, `Stream.Read(this+0x198, 8)` on restore followed by the same
GetValue/UpdateValue loop the constructor runs. So the payload is the parameter
array in index order, little-endian int32.

| byte | type | parameter | range | default | corpus | confidence |
|---|---|---|---|---|---|---|
| 0..3 | i32 | 0 Balance (pan) | -128..128 | 0 | 0 in all 57 uses | loader, dfm, corpus |
| 4..7 | i32 | 1 Volume | 0..320 | 256 | 0, 191, 223, 228, 245, 256 (x41), 275, 288, 290, 320 | loader, dfm, corpus |

Range and default come from the DFM: `PanWheel: Min = -128, Max = 128,
DefaultValue = 0`; `VolWheel: Max = 320, Value = 256, DefaultValue = 256`
(Min defaults to 0). Both wheels have `OnChange = CutoffWheelChange` and the
Volume wheel carries `Tag = 1`, which is the parameter index.

Preset check: the install ships one Fruity Balance preset (`Fruity
Balance.fst`), the default `[0, 256]`, and no named ones. The five-preset
check could not be run for this plugin. The corpus covers the volume range
end to end (0 = silence, 320 = maximum) and never moves the balance off 0.

## 2. Parameter semantics (`ProcessParam` @ 0x006beaa0)

Flags behave per the SDK. `REC_FromMIDI` (0x20) rescales the incoming value
with the wheel's Min/Max: `v = Min + Round(V * 2^-30 * (Max - Min))`, the
constant at `0x006bee10` being 2^-30. `REC_GetValue` (2) returns `params[i]`.
`REC_UpdateControl` (0x10) pushes the value to the wheel. `REC_ShowHint` (4)
formats the value text via `FUN_006be4b0`. `REC_UpdateValue` (1) stores
`params[i]` and recomputes the four gains below.

Constants read from the file: `0x006bee18` = 0.5, `0x006bee20` = pi,
`0x006bee28` = 1.0, `0x006bee30` = 1.25, `0x006bee38` = 11.0, float at
`0x006fc21c` = 1/256, float at `0x006fc218` = 1/128, double at `0x006fc1f8`
= pi/4. RTL callees identified by their range-reduction constants:
`FUN_00409400` = Cos, `FUN_004094f0` = Sin, `FUN_00409870` = Ln,
`FUN_00408810` = Exp, `FUN_005d10a0` -> `FUN_0045d040` = SinCos(theta, sin,
cos), `FUN_00409d00` = Round.

```
b   = params[0]                      // -128..128
v   = params[1]                      // 0..320

// index 0 only
pos  = (b + 128) / 256               // 0..1, 0.5 = centre         -> this+0x150
pan  = b / 128                       // -1..1                      -> this+0x158
comp = (1 - 0.5 * sin(|pos - 0.5| * pi)) * 0.141421356           -> this+0x168
       // 0.141421 at centre, 0.070711 hard left or right

// index 1 only
vol  = v / 320                       // wheel Max read from the control -> this+0x160

// both
gain  = (11 ^ (1.25 * vol) - 1) * comp      // Exp((float)(vol*1.25) * Ln(11))
angle = (pan + 1) * pi / 4                   // 0 .. pi/2
LL = gain * cos(angle)                       // -> this+0x130
RR = gain * sin(angle)                       // -> this+0x138
RL = LR = 0
if pos < 0.5:  RL = LL - RR                  // right input bleeds into left out, this+0x140
else:          LR = RR - LL                  // left input bleeds into right out, this+0x148
```

What the numbers mean. At the defaults (b = 0, v = 256): vol = 0.8, `11^1 - 1 =
10`, comp = 0.141421, gain = 1.41421, angle = pi/4, so LL = RR = 1.0 and no
crossfeed. The constant in `this+0x190` exists to make exactly that come out
at unity. At v = 320 the gain is `(11^1.25 - 1) * 0.141421 * 0.7071 = 1.90`,
about +5.6 dB; at v = 0 it is 0 (silence). Volume is therefore not a dB knob:
per channel at centre pan the gain is `(11^(v/256) - 1) / 10`.

The pan is constant-power (cos/sin of a quarter turn) with two twists. The
`comp` factor drops the overall gain by up to 6 dB (comp halves the gain; the net near-side level drops about 3 dB) towards the extremes, and
the crossfeed folds the far channel into the near one by the difference of
the two gains. Hard left (b = -128): LL = 0.7071, RR = 0, RL = 0.7071, so the
output is `L' = 0.7071 (L + R)`, `R' = 0`: a mono fold-down at -3 dB per source.
Half left (b = -64): LL = 0.845, RR = 0.350, RL = 0.495.

Display (`FUN_006be4b0`, `GetName` section `FPN_ParamValue`): balance prints
`b / 128` (constant 128.0 at `0x006be660`); volume prints the gain
`g = (11^(v/256) - 1) / 10` as a 2-decimal number with `20 * log10(g)` dB.
`GetName` section `FPN_Param` returns the wheel hint, "Balance" and "Volume".
The `Ln(11)` float used by the display lives in `.bss` at `0x0071e46c`; I did
not disassemble its initialiser, it is inferred from the identical pattern in
Fast Dist (same unit, same offset shape) and from the `== 1.0` shortcut that
only makes sense for base 11. Tag: `inferred`.

## 3. Signal chain (`Eff_Render` @ 0x006befc0)

Per block of `Length` stereo frames:

```
// 1. ramp each of the four gains from where the last block ended to its target
for k in 0..3:
    ramp(delta = this+0x180+4k, state = this+0x120+4k, cur = this+0x170+4k,
         target = (float) *(double*)(this+0x130+8k), max_step, Length)

// FUN_005dfef0:
cur   = state                       // gain at the start of this block
delta = target - cur
if delta != 0:
    state = target                  // where we will be at the end of the block
    delta = delta / Length          // per-sample step
    if |delta| > max_step:
        delta = sign(delta) * max_step
        state = cur + delta * Length    // did not reach the target this block
        if |state| is tiny: state = 0; delta = -cur / Length   // denormal guard

// 2. apply the ramped 2x2 matrix   (FUN_005dee90, reached via the pointer at 0x0071e4f0)
m = [LL, RR, RL, LR] = this+0x170..0x17c ;  d = this+0x180..0x18c
for each frame (L, R):
    L' = L * m[0] + R * m[2]
    R' = R * m[1] + L * m[3]
    m += d
// fast paths: all d == 0 and m[2] == m[3] == 0 -> plain per-channel scale, 8 floats at a time;
// skipped entirely when m[0] == m[1] == DAT_006fc220 (1.0, inferred) and src == dst.

// 3. peak meter   (FUN_005df940)
peakL, peakR = max |L'|, max |R'| over the block   -> this+0x1a0, this+0x1a4
```

`FUN_005dee90` is selected at startup (`lea` at `0x005e1cb3`, stored at
`0x005e1cb8`); only one variant is registered, so there is no CPU-specific
alternative to worry about. `max_step` is `*(*(this+0xcc) + 0x38)`, a float
from a host-provided structure (see unknowns).

Re-implementation note: the whole effect is `out = M(t) * in` with `M`
recomputed on parameter change and linearly ramped over each block. Match
FL's sound by keeping the ramp (a step change at 44.1 kHz clicks), the
constant-power law, the `comp` dip, and the crossfeed. The exact `max_step`
only matters for how fast a knob sweep settles.

## 4. What stays unknown

- `max_step` for the ramp: read from `*(*(this+0xcc) + 0x38)`. `this+0xcc` is
  filled in by the base class constructor (`FUN_006b8e80`) from the host; the
  field is beyond the public `TFruityPlugHost` layout, so its value is FL's
  business. Likely a per-sample slew limit scaled to the sample rate.
- The denormal threshold in `FUN_005dfef0` (`DAT_0071e434`, `.bss`, set at
  runtime) and the sign mask `DAT_005dffb0`.
- `DAT_006fc220` in the apply routine, assumed 1.0 from context (the skip is
  "unity gain and in-place").
- Flags bit 28 of the plugin info.
- `Dispatcher` (`0x006bde30`) and `ProcessEvent` were decompiled but not read
  closely; the constructor sends host message 0x21 and `FUN_006bd210` queries
  0x47 and 0x56 for UI setup. None of it touches audio.
- Whether `vol` divides by the wheel's current `Max` (it reads the control at
  runtime, `+0x3ac`) could in theory change if FL ever rescaled the wheel; the
  DFM says 320 and every corpus value is <= 320.

## Notes from the implementation (crates/plugins/src/balance.rs)

- At balance 0 the crossfeed term `RR − LL` is one float ulp off zero; the implementation zeroes it there.
- The gain ramp's starting state on plugin creation is undocumented; a loaded state starts on target.
