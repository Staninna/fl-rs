# Fruity Fast LP: state, parameters, signal chain

Recovered from `Fruity Fast LP_x64.dll` (FL Studio 2026, Delphi native plugin,
image base 0x400000). Ghidra project `ghidra/proj`, decompiles under
`ghidra/out/fastlp/` (`vmt/`). Corpus: 17 uses in 2 of the 165 `.flp`; the
install ships no presets for it. Two Ghidra runs. The coefficient routine and
the filter loop were read from `objdump` (the first is a second-level callee,
the second is behind a runtime function pointer).

Confidence tags: `loader`, `dfm`, `corpus`, `inferred`, `asm`.

## How the plugin is wired

`CreatePlugInstance` @ `0x006c8080` -> constructor `FUN_006c83e0`, class
reference `PTR_PTR_006c9260` -> VMT `0x006c7f60` (`TTestPlug`).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006c80d0 |
| 2 | Idle_Public | 0x006c5db0 |
| 3 | SaveRestoreState | 0x006c82e0 |
| 4 | GetName | 0x006c8bd0 |
| 5 | ProcessEvent | 0x006c5440 |
| 6 | ProcessParam | 0x006c87d0 |
| 7 | Eff_Render | 0x006c8d60 |

`TFruityPlugInfo` at `0x007171e8`: SDKVersion 1, LongName "Fruity Fast LP"
(`0x006c7e80`), ShortName (`0x006c7e8f`), Flags 0x10400000, NumParams 2.

Object fields:

| offset | type | meaning |
|---|---|---|
| 0x120 | i32[2] | the parameters, saved state: Cutoff, Resonance |
| 0x128 | f32 | cutoff as 0..1 |
| 0x12c | f32 | resonance as 0..1 |
| 0x130 | struct | filter: `+4` (0x134) f32 F target, `+8` (0x138) f32 Q, `+0x28..+0x40` (0x158..0x170) f64 yL, yR, vL, vR |
| 0x198 | f32 | current (ramped) F |

The constructor ends by pushing both wheels' default values through
`ProcessParam` with flags 0x15 and calling `FUN_006c82b0`, which zeroes the
filter doubles and sets the ramp state to the F target.

## 1. State layout (event 213, 12 bytes)

`SaveRestoreState` (`FUN_006c82e0`) is versioned:

```
save:    Write(int32 1); Write(this+0x120, 8)
restore: Read(int32 ver); if ver < 2: Read(this+0x120, 8); GetValue/UpdateValue loop
```

| byte | type | idx | parameter (DFM object, hint) | range | default | corpus | confidence |
|---|---|---|---|---|---|---|---|
| 0..3 | i32 | | version, always 1 | | 1 | 1 (x17) | loader, corpus |
| 4..7 | i32 | 0 | Cutoff (`CutoffWheel`) | 0..1024 | 1024 | 1024 (x10), 0 (x3), 931, 725, 480, 137 | loader, dfm, corpus |
| 8..11 | i32 | 1 | Resonance (`ResoWheel`, Tag 1) | 0..1024 | 0 | 0 (x15), 489, 138 | loader, dfm, corpus |

No shipped preset, so no preset cross-check; the default state is `[1, 1024,
0]`, which is a bypass (see section 3). Corpus readings with the formulas of
section 2, one-pole corner `-ln(1 - F) * fs / 2 pi` at 44.1 kHz:

| Cutoff | Reso | F | Q | corner |
|---|---|---|---|---|
| 1024 | 0 | 1.0 | 0 | bypass |
| 931 | 0 | 0.563 | 0 | 5.8 kHz |
| 725 | 0 | 0.158 | 0 | 1.2 kHz |
| 480 | 0 | 0.0337 | 0 | 240 Hz |
| 137 | 138 | 0.0026 | 0.42 | 18 Hz, mildly resonant |
| 1024 | 489 | 1.0 | 0.85 | F at 1.0 cancels the memory, so still a near-bypass |
| 0 | 0 | 0.000175 | 0 | 1.2 Hz, effectively mute |

Ten of the seventeen uses sit at the default, so the plugin is mostly parked
in a chain for automation.

## 2. Parameter semantics (`ProcessParam` @ 0x006c87d0)

Flags per the SDK; `REC_FromMIDI` uses 2^-30 at `0x006c8980`. On
`REC_UpdateValue`: `x = clamp((v - Min) * wheel.scale, 0, 1)` (`wheel+0x3cc`,
1/1024 for these wheels, tag `inferred`), stored at `this+0x128` (cutoff) or
`this+0x12c` (resonance), then both coefficients are recomputed by
`FUN_006c86b0(this+0x130, (c + 1/63) * 63/64, r * 0.625)` (floats 1/63 at
`0x00717290`, 63/64 at `0x0071728c`, 0.625 at `0x00717294`):

```
xc = (c + 1/63) * 63/64 = (63 c + 1) / 64            // 1/64 .. 1
F  = xc >= 1 ? 1.0 : xc <= 0 ? 0.0 : (601^xc - 1) / 600    -> this+0x134
                                                       // Ln 601 in .bss 0x00727468 (set at 0x005e162b from the double 601.0 at 0x005e1c48), float 1/600 at 0x00708ef4

xr = min(r * 0.625, 0.625)
Q  = xr <= 0 ? 0.0 : 1 - (601^(1 - xr) - 1) / 600     -> this+0x138
```

So F covers 0.000175 (cutoff 0) to exactly 1.0 (cutoff 1024), exponentially,
and Q covers 0 (no resonance) to 0.983 (full), with `1 - xr` making the
same 601-base curve run backwards so that Q is continuous at 0. Neither
depends on the sample rate; like Fruity Filter, the corner in Hz moves with
the project rate.

Display (`FUN_006c8b10` -> `FUN_006c8a90`): both wheels print their 0..1
fraction. `GetName` section 0 returns "Cutoff" and "Resonance".

`Dispatcher` only forwards ID 4 (SetSampleRate) to the host structure; the
plugin itself has no rate-dependent maths.

## 3. Signal chain (`Eff_Render` @ 0x006c8d60)

```
// ramp F from this+0x198 towards this+0x134   (FUN_006c8c90, same 128-sample slew as Fruity Filter)
start = cur;  d = 0
if cur != F:
    if Length < 128: d = (F - cur) / 128; cur += d * Length; denormal guard (DAT_00717288)
    else:            d = (F - cur) / Length; cur = F

if F == 1.0 and Q == 0 and d == 0:                     // bypass
    if Source != Dest: copy Source -> Dest             // pointer 0x00718038
    yL, yR = last frame of Dest                        // FUN_005de300: keep the filter memory following the signal
else:
    filter(this+0x130, Source, Dest, Length, start, d) // pointer 0x007175a0 -> 0x005de320 (AVX twin 0x005e0d60)
```

### The filter (`0x005de320`, x87, `asm`)

State per channel: `y` (the output, `this+0x158/0x160`) and `v` (a velocity,
`this+0x168/0x170`), both doubles. `F` ramps per frame, `Q` does not.

```
for each frame:
    for ch in L, R:
        v[ch] = v[ch] + F * (x[ch] - y[ch])
        y[ch] = y[ch] + v[ch]
        out[ch] = y[ch]
        v[ch] = v[ch] * Q
    F += d
flush the four doubles (0x005de1f0) and store them back
```

Written as a difference equation with `v[n]` the damped velocity carried
into the next sample:

```
v[n] = Q * v[n-1] + F * (x[n] - y[n-1])
y[n] = y[n-1] + v[n]

H(z) = F / (1 - (1 + Q - F) z^-1 + Q z^-2)
```

A two-pole low pass with unity DC gain, poles of radius `sqrt(Q)`. At Q = 0 it
collapses to the one-pole `y += F (x - y)`, which is the "fast" in Fast LP:
one multiply-add per sample. Turning Resonance up keeps a fraction Q of the
previous step, which is the momentum that overshoots and rings: at Q = 0.983
the pole radius is 0.9915, a sharp peak just below the corner. The corner for
Q = 0 is `-ln(1 - F) * fs / 2 pi`; with Q > 0 the peak sits near `acos((1 + Q
- F) / (2 sqrt Q)) * fs / 2 pi`.

Fruity Filter's Center block uses the very same recursion with `out = x - y`
(see `plugin-filter.md`); the two plugins share the unit at `0x005de3xx`.

Re-implementation note: two doubles per channel, F and Q as above. Keep the
F ramp (128-sample slew for short blocks); Q jumps. The bypass branch is
exact when F = 1 and Q = 0 (`v = x - y; y = x`), so it only saves CPU.

## 4. What stays unknown

- `wheel+0x3cc` (assumed 1/1024) and `DAT_00717288` (ramp denormal
  threshold, `.bss`), as for Fruity Filter.
- The AVX twin at `0x005e0d60` and the second pointer at `0x007275a8`
  (`0x005dfd00`, registered beside the filter but not called by this plugin).
- Flags bit 28; `Dispatcher` IDs 5 and 0x39 are GUI.

## Corrections from the implementation (crates/plugins/src/fast_lp.rs)

- Cutoff 1024 with resonance 489 is not a near-bypass: the doc's own H(z) peaks at 1/(1 − Q), about 16 dB, at fs/6.
