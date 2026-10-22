# Fruity WaveShaper: state, parameters, signal chain

Recovered from `Fruity WaveShaper_x64.dll` (FL Studio 2026, Delphi native
plugin, image base 0x400000; the DLL carries the whole Edison tool set in its
forms, which is why it is 5.7 MB). Ghidra project `ghidra/proj`, decompiles
under `ghidra/out/waveshaper/` (`vmt/` the slots with callees, `env/` the
`TMEnvelope` class and the CPU dispatch, `shaper/` the five table lookups,
`env2`..`env4/` the envelope recompute). Corpus: 74 uses in the 165 `.flp`,
34 shipped presets plus the five `Data/Maps/*.fnv` curve files. Seven Ghidra
runs; the overage went into the envelope class, which is FL's shared
`DSP_MEnvelope` unit and bigger than the plugin.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `inferred`, `asm`.

## How the plugin is wired

`CreatePlugInstance` @ `0x00830ba0`, `TTestPlug` VMT `0x00830a88` (instance
size 0x1f8).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x00830bf0 |
| 2 | Idle_Public | 0x0076fc10 |
| 3 | SaveRestoreState | 0x00831040 |
| 4 | GetName | 0x00831fd0 |
| 5 | ProcessEvent | 0x00831c30 |
| 6 | ProcessParam | 0x00831c40 |
| 7 | Eff_Render | 0x00832390 |

Object fields:

| offset | type | meaning |
|---|---|---|
| 0x120 | i32[3] | pre amp, mix, post gain, saved |
| 0x12c / 0x130 / 0x134 | f32 | dry gain, wet gain, pre amp (linear) |
| 0x138 / 0x139 | u8 | visual off, "Center" (DC removal) on |
| 0x13c | i32 | oversampling selector (`OverSelect`, 0..4), saved |
| 0x140 | i32 | oversampling factor in use (`2^0x13c`, from `FUN_00830030`) |
| 0x144..0x14c | f32[3] | per-sample smoothed copies of 0x134, 0x130, 0x12c |
| 0x150 / 0x154 | f32, i32 | peak hold for the display |
| 0x158 | u8 | host rendering flag (Dispatcher 1, flag 2): forces 4 stages (16x) and the HQ lookup |
| 0x15c | f32 | smoothing coefficient for 0x144.. (Dispatcher 4) |
| 0x160 / 0x168 | ptr | upsampler, downsampler: `TOversampler` objects, run by `FUN_008300a0`, `FUN_00830140` (section 3.1) |
| 0x170 | struct | DC remover: `+4` coefficient, `+8` 0.96, `+0x28..+0x40` four f64 states |
| 0x1d8 | ptr | the `TMEnvelope` (VMT `0x00775f70`): the drawn curve |
| 0x1e0 / 0x1e8 | ptr, i32 | the baked lookup table (i16, 2 guard entries) and its last index |

## 1. State layout (event 213, variable length; plugin versions 1 and 4, envelope versions 0 to 3)

`SaveRestoreState` (`FUN_00831040`) wraps the host stream in a `TStream`
(`FUN_007244d0`) and reads:

```
Read(u32 v); if v > 0x40003: refuse;  version = v > 0xffff ? v >> 16 : v        // the two shapes: 0x00000001 and 0x00040003
Read(this+0x120, 12)                                                             // pre amp, mix, post gain
Read(u8 polar)  -> PolarBtn, envelope.SetCenter(polar * 0.5)                     // FUN_00776500: bipolar when 1
Read(u8 center) -> CenterBtn (DC removal)
Read(i32)       -> this+0x13c, OverSelect
envelope.SaveRestore(stream, 0)                                                  // TMEnvelope slot 2, FUN_0077e5f0 -> FUN_00776550, below
if version > 1: Read(i32 f) -> SettingsBtn.Down = f & 1                          // settings panel shown
if version > 2: Read(u8)    -> HqCheckBox
if version > 3: Read(u8)    -> SmoothCheckBox
rebuild oversamplers (FUN_00832300), bake the table (FUN_00832ba0), GetValue/UpdateValue loop
save: Write(u32 0x40003); the same fields; the envelope at version 3
```

Plugin header, 22 bytes:

| byte | type | parameter (DFM object, hint) | range | default | corpus | confidence |
|---|---|---|---|---|---|---|
| 0..3 | u32 | version: `0x00000001` (presets), `0x00040003` (every corpus block, version 4) | | 0x40003 | 0x40003 x74; presets 1 | loader, corpus, preset |
| 4..7 | i32 | Pre amp (`PreAmpWheel`), 128 = unity | 64..192 | 128 | 128 x61, 192 x4, 64..162 | loader, dfm, corpus, preset |
| 8..11 | i32 | Mix level (`MixWheel`), 128 = fully wet | 0..128 | 128 | 128 x70, 83, 13 | loader, dfm, corpus, preset |
| 12..15 | i32 | Post gain (`PostGainWheel`), 128 = unity | 0..128 | 128 | 128 x70, 68, 98, 111 | loader, dfm, corpus, preset |
| 16 | u8 | Unipolar (0) / bipolar (1) (`PolarBtn`) | 0, 1 | 0 | 0 x66, 1 x8; presets 14 bipolar | loader, corpus, preset |
| 17 | u8 | Center: remove DC offset (`CenterBtn`) | 0, 1 | 0 | 0 x60, 1 x14 | loader, corpus, preset |
| 18..21 | i32 | Oversampling (`OverSelect`): factor `2^v` | 0..4 | 0 | 0 x59, 1..4 | loader, dfm, corpus, preset |

Then the envelope (`TMEnvPoints.SaveRestore`, `FUN_00776550`; the format
version is a global, `0x008b167c`, so presets of any age load):

| field | type | meaning |
|---|---|---|
| kind | i32 | 3 for this envelope (`this+0x58`); a different value means the version follows at once |
| version | i32 | 0, 1, 2 or 3; the plugin writes 3 |
| count | i32 | points |
| points | | version 0: `f32 dx, f32 y, f32 tension` (12 bytes); version 1: plus `u8 mode, u8, u8, i8 dir` (16); version 2 and 3: `f64 dx, f64 y, f32 tension, u8 mode, u8, u8, i8 dir` (24) |
| trailer | 20 bytes | `this+0x44..0x57`: an i32 (0 in every file) and four i32 -1 (loop/sustain/attack/release point indices, none) |

`dx` is the segment length *before* the point, as a fraction of the whole
curve; the first point has 0 and the lengths sum to 1 (`Crossover`: 0,
0.375, 0.625). `y` is the level 0..1; in bipolar mode the curve is read as
`y - 0.5`. `tension` is -1..1. `mode` selects the segment shape (section 2),
`dir` is the sign of the tension recomputed on load (1, -1, 0; 2 forces
linear). The `.fnv` files under `Data/Maps` are exactly this block with no
plugin header (`Default.fnv`: kind 3, version 0, two points, 56 bytes).

Then, version 4 only, 6 bytes: `i32` settings panel shown (1 in every
corpus block), `u8` HQ (0 or 1), `u8` smooth modulation (0 or 1).

Sizes: version 1 preset = 22 + 12 + 12n (env 0) or 16n (env 1) or 24n
(env 2) + 20; version 4 corpus = 22 + 12 + 24n + 20 + 6, so 108 bytes for
two points, 132 for three, 156 for four. The 34 `.fst` presets range from
78 bytes (`Default`, two points) to 3486 (`Stereo tape simulation`, 145
points, 24 bytes each).

Preset check (`.fst` files name the plugin "Fruity WaveShaper"):

| preset | Pre | Mix | Post | polar | center | over | points (dx, y, tension, mode) | reading |
|---|---|---|---|---|---|---|---|---|
| Default | 128 (0 dB) | 128 | 128 (0 dB) | uni | 0 | 1x | (0,0,0) (1,1,0) | identity: y = x |
| Crossover | 192 (+7.4 dB) | 128 | 128 | uni | 0 | 4x | (0,0,+1) (0.375,0,0) (0.625,0.625,-0.005) | dead zone to 0.375 then a line to 0.625: crossover distortion, output capped at 0.625 |
| Kinda tanh | 64 (-9.5 dB) | 128 | 128 | bi | 1 | 2x | (0,0,0) (0.5,0.5,-0.25) (0.5,1,+0.25) | an S through the centre; bipolar so the two halves can differ |
| Fast dist 1 | 162 (+4.0 dB) | 128 | 128 | uni | 1 | 2x | (0,0,0) (0.625,0.625,0) (0.375,0.625,+0.005) | linear to 0.625 then flat: a hard clip at -4 dB after +4 dB of drive |
| Cold valves | 93 (-4.7 dB) | 128 | 128 | uni | 1 | 2x | (0,0,0) (0.062,0,0) (0.188,0.062,-0.245) (0.375,0.5,0) (0.375,0.625,+0.27) | a gate below 0.06, a slow knee, saturation above 0.63 |
| Asymetric half sin shaper | 128 | 128 | 128 | bi | 0 | 16x | (0,0.25,0) (0.25,0.25,0,m9) (0.25,0.5,-1,m9 dir -1) (0.5,1,+1,m9) (0,1,0) | version 4 preset: half-sine segments, bipolar, so negative input is flattened at -0.25 |
| Class B dist | 107 (-2.7 dB) | 128 | 111 (-3.1 dB) | bi | 1 | 2x | 7 points, env 2 | |
| Bitmunched | 138 (+1.2 dB) | 128 | 128 | bi | 0 | 4x | 18 points in 0.125 steps, env 2 | a staircase: 3-bit quantiser drawn by hand |

## 2. Parameter semantics

`ProcessParam` (`FUN_00831c40`), on `REC_UpdateValue` of any of the three
(under the host lock):

```
pre  = (4^(p0/128) - 1) / 3                                  -> 0x134     // 0x00831ef8 = 4, 0x00831f08 = 3; 64 -> 0.333 (-9.5 dB), 192 -> 2.33 (+7.4 dB)
post = p2/128 == 1 ? 1 : (11^(p2/128) - 1) / 10              // 0x8af524 = ln 11 (FUN_0062d170), 0x87660c = 0.1
wet  = p1/128 * post                                         -> 0x130
dry  = (post - wet) * pre * post                             -> 0x12c     // as found: the dry path is scaled by pre and by post twice
```

`REC_FromMIDI` uses Min/Max and 2^-30 (`0x00831ef0`). The display
(`FUN_00831f50`) was not read.

Oversampling (`FUN_00832300`, on load and on Dispatcher 1): both
oversamplers are rebuilt (`FUN_0082ffa0`) with `p = 0x13c` stages, or 4
when the host is rendering (`0x158`), and `0x140 = 1 << p`
(`FUN_00830030`). A stage is 0xc0 bytes, zeroed on every rebuild and on
Dispatcher 2 (`FUN_00830040`). Section 3.1 has the filter.

Smoothing (Dispatcher 4, `FPD_SetSampleRate`, `0x00830bf0`):

```
0x15c = (f32)(1 - Power(0.1, 1 / (f32)(rate * 0.005)))     // 0x00830f38 = 0.1, 0x00830f48 = 0.005 (f32), Power = FUN_004664d0, exp(y ln x)
```

that is, 90% of the way in 5 ms of frames at the song's rate. The glided
gains `0x144`, `0x148`, `0x14c` are 0 from the constructor
(`FUN_008313f0`) and nothing else sets them, so a state with smooth on
fades in.

Table bake (`FUN_00832ba0`, after every curve edit and on load):

```
N      = (bipolar + 1) * 0x4000;  table = i16[N + 2] + 1 (one guard entry each side); last = N - 1     // 0x1e0, 0x1e8
it     = envelope.StartIterator (slot +0x50, FUN_0077c550)
for i in 0..N-1:  table[i] = clamp(round(it.value * ((bipolar + 1) * 0x8000 - 1)), -32768, 32767);  it.step(1 / last)     // FUN_0077d490
table[-1] = table[0];  table[N] = table[N - 1]
```

Unipolar: 16384 entries over input 0..1, values 0..32767 (the curve `y`
times 32767). Bipolar: 32768 entries over input -1..1, values -32768..32767
(`(y - 0.5) * 65535`). Sixteen bits of table is where the "HQ" checkbox's
hint ("don't truncate below -85 dB") comes from.

Curve evaluation (`FUN_0077d490`, the iterator step; `FUN_0077a330`
recomputes the per-point coefficients `a, b, c` at `+0x10, +0x14, +0x18`
from `tension` and `mode`; `FUN_0060de00` for modes 0 and 1): within a
segment `t` runs 0..1 from the previous point to this one, the output is
`y_prev + f(t) * (y - y_prev)`, and `f` depends on the point's mode:

```
dir == 2 (or tension 0 for most modes):  f = t                                            // linear
mode 0 single curve:   a = sign(T) (B^|T| - 1), B = 1000001 (0x0077ae08), b = 1/|a|, c = ln(|a| + 1) = |T| ln B
                       T < 0: f = (e^(c t) - 1) b             = (B^(|T| t) - 1) / (B^|T| - 1)
                       T > 0: f = 1 - (e^(c (1 - t)) - 1) b   = 1 - (B^(|T| (1 - t)) - 1) / (B^|T| - 1)
mode 1 double curve:   the same curve on each half (t < 0.5 and t >= 0.5), scaled by 0.5
mode 2 hold:           f = 0 until the point (dir 2 is forced)
mode 3 stairs:         f = round(t a - c) b, a = steps + 1, b = 1/(a -+ 1), c = +-0.5  (steps from |T|, FUN_0077bd20)
mode 4 smooth stairs:  a cubic between the steps: f = (k + 0.5 u^3) / a with u the position inside a step (constants 0.5, 2.0)
mode 5 pulse:          f = round(t a - c) & 1, a = 2 (steps + 1)
mode 6 wave:           f = sin-shaped: T > 0: (sin((t a + 0.75) 2 pi) + 1) / 2;  T <= 0: |triangle|
mode 7 single curve 2: u = T < 0 ? t : 1 - t;  f = t - (u^6 - u) a, a = T
mode 8 double curve 2: the same on each half
mode 9 half sine:      f = t - (t - s) a, s = T <= 0 ? 1 - cos(t pi/2) : sin(t pi/2), a = |T|      // PTR_DAT_0089fa40 = pi/2 (inferred from the shape)
mode 10 smooth:        cubic Hermite through the point with end slopes a, b computed from the neighbours (FUN_0077a330 lines 182..284), scaled by c = 1 - |T|
mode 11 single curve 3: f = b / (1 - a t) - c (T > 0 mirrored), a = 1 - (1 - 0.9 |T|)^3 (floored at 1.5e-5), b = (1 - a)/a, c = b
mode 12 double curve 3: the same on each half with b, c halved
```

Modes 0 and 1 are the ones a user gets from dragging a segment's handle; the
others come from the right-click menu. The tension range -1..1 maps, for the
default mode, to an exponential with base `1000001^|T|`: at `|T| = 0.25` the
segment is `e^(3.45 t)` normalised, at `|T| = 1` it is `e^(13.8 t)`, nearly
a step.

## 3. Signal chain (`Eff_Render` @ 0x00832390)

```
if visual on: peak-follow max(|L|, |R|) for the display (0x150, 0x154); GUI only
if oversampling (0x13c > 0): buf = upsample(Source), n *= 0x140                        // FUN_008300a0, section 3.1
else: buf = Source
if Smooth off (SmoothCheckBox, GUI object +0x778, +0x48e):
    shape(buf -> out, 2n, table, last, dry, wet, pre)                                  // one of four, below
else:
    per frame of buf: g += (target - g) * 0x15c in f32 for 0x144 <- pre, 0x148 <- wet, 0x14c <- dry,
                      then shape that frame with the glided gains                      // at the oversampled rate
if oversampling: Dest = downsample(out)                                                // FUN_00830140
if Center (0x139): DC remover on Dest                                                  // FUN_00627af0

shape, unipolar, nearest (FUN_00629d80; HQ off):
    i = round(x * pre * last);  v = table[min(|i|, last)] with the sign of i             // odd symmetry: the curve is mirrored through the origin
    out = v * wet / 32768 + x * dry                                                    // 0x0087e238 = 1/32768
shape, unipolar, linear (FUN_00629e00; HQ on):
    p = x * pre * last;  i = round(p);  fr = p - i;  v = lerp(table[|i|], table[|i|+1], fr) with the sign;  same mix
shape, bipolar, nearest (FUN_00629f50):
    i = clamp(round(x * pre * last/2 + last/2), 0, last);  v = table[i];  same mix        // 0x008765f8 = 0.5
shape, bipolar, linear (FUN_00629fd0):
    p = x * pre * last/2 + last/2;  i = clamp(round(p), -1, last);  v = lerp(table[i], table[i+1], p - i)

DC remover (FUN_00627af0, per channel, doubles):
    v += (x - m) * c1;  m += v;  v *= 0.96;  out = x - m                                 // c1 = 0x174 = host+0x20 * 2.2835e-5 (set on sample-rate change, 0x00830bf0), 0x178 = 0.96
```

`host+0x20` is the renderer's `44100 / rate`, as Delay Bank's filters
read it, so `c1 = 1.0070235 / rate`.

The SSE variants (`FUN_0062c380`, `FUN_0062c500`, `FUN_0062c620`,
`FUN_0062cc10`, `FUN_0062cd10`) are picked at start-up on CPU flags and do
the same maths. The selection: HQ off and not rendering uses the nearest
lookups; HQ on or rendering uses the linear ones; the polar flag of the
envelope (`FUN_007764e0`: centre == 0.5) picks unipolar or bipolar.

What that means for a port: the curve is never evaluated per sample. Bake it
into 16384 or 32768 16-bit points as above and index with
`x * pre * last` (unipolar, with sign) or `x * pre * last/2 + last/2`
(bipolar), then mix `wet / 32768` of the table against `dry` of the input.
Rounding is to nearest, clamping at the table ends is a hard clip: input
beyond `1/pre` holds the curve's last value, which is why every "Soft clip"
preset works as a clipper.

### 3.1 The oversampler

The class is the one Delay Bank, Love Philter, EQUO, Maximus and
Parametric EQ 2 have; `crates/dsp/src/halfband.rs` rebuilds it.

- `FUN_008300a0` (up): for each stage from the last to the first, the
  kernel at `DAT_008b2010` doubles the block into the tail of the work
  buffer, then the flush at `PTR_DAT_0089fe08` runs on the stage with a
  count of 12, and `n` doubles.
- `FUN_00830140` (down): `n` halves, the kernel at `DAT_008b2018` halves
  the block, the last stage into `Dest`, and the same flush.
- `FUN_00830260` fills the two pointers at startup: `FUN_0082fb70` and
  `FUN_0082fd40` (SSE2, picked on CPU flag 0x100) or `FUN_0082fb00` /
  `FUN_0082fcd0`, which only raise "Not implemented in i64/ARM".
  `FUN_008301d0` copies the 13 doubles at `0x0089ee70` into a table
  (`0x008b2020`), each twice for the two sides.
- The kernels, per frame and side, in doubles: an allpass is `y = x1 +
  (x - y1) c`. Up, the input goes through the path `0.14737711360104660,
  0.48246854276970014, 0.88300502576937310` for the second output frame
  and through `0.039151597734460045, 0.30264684832849340,
  0.67461591854696390` for the first. Down, the first frame of a pair
  goes through the first path, the second through the second, and the
  output is `(second + first) * 0.5` (the 13th double). Each output is cut
  to f32.
- The flush (`FUN_0062ca40`, SSE) zeroes each of the stage's first 12
  doubles, the six allpass outputs of each side, when its size is under
  2^-24 (`0x0087e26c`). The inputs are not flushed.
- Nothing reports latency: the Dispatcher sends no message to the host.
  The filter is recursive; at low frequencies it is 1.65 frames of the
  lower rate late going up and 1.15 going down.

`tools/plugstate/halfband_emu.py` runs the two SSE kernels and the flush
of this DLL under unicorn, and `dsp::halfband`'s test
`the_dll_s_kernels_give_these_bits` has its output to the bit.

## 4. What stays unknown

- `TMEnvelope.SaveRestore` (`FUN_0077e5f0`) also writes 16 bytes from
  `this+0x98` after the trailer and on restore seeks past them when its
  flag bit 3 is clear. Neither the corpus nor the `.fnv` files contain those
  16 bytes; the 20-byte trailer is followed directly by the plugin's 6-byte
  tail. The byte layout above is what the files hold; how the seek lands on
  the right offset was not resolved (the stream wrapper may clamp).
- Mode 10 (smooth) end-slope formulas and the step counts of modes 3..6
  (`FUN_0077bd20`, constants `0x0077be10..0x0077be40`) were not reduced to
  closed form.
- `GetName` display text, the Dispatcher's GUI IDs, `Idle_Public`.

## Corrections from the implementation (crates/plugins/src/waveshaper.rs)

- Versions 0x20003 (25 corpus states) and plain 4 exist besides 1 and 0x40003.
- Mode 6 signs are swapped against docs/automation-234.md.
- The 3486-byte state is the "Fuzzy" preset.
- A straight bipolar curve returns x·32767/32768, not the input bit for bit.
- Oversampling is built (section 3.1, `dsp::halfband::Oversampler`) and so
  is smooth modulation; FL's render-time switch to 16x and the HQ lookup is
  not, since the engine plays as FL does live: the state decides.
