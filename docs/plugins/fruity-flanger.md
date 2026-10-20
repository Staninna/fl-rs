# Fruity Flanger: state, parameters, signal chain

Recovered from `Fruity Flanger_x64.dll` (FL Studio 2026, Delphi native
plugin, image base 0x400000), read as listings (capstone) with the
constants each instruction loads, cross-checked in the Ghidra project
`fx6`. Corpus: 5 states in 4 `.flp` (Asher Postman - Future House, Bombs
Away - I'm Awake, Purps On The Beat - Stroll Up twice, Rock), 27 shipped
presets and one mixer preset in a format the plugin no longer reads.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `asm`.

## How the plugin is wired

`CreatePlugInstance` @ `0x006bab00` -> constructor `FUN_006baec0`, class
`0x006ba8f0`.

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006bab50 |
| 2 | Idle_Public | 0x006b7d00 |
| 3 | SaveRestoreState | 0x006badc0 |
| 4 | GetName | 0x006bbc70 |
| 5 | ProcessEvent | 0x006b7390 |
| 6 | ProcessParam | 0x006bb210 |
| 7 | Eff_Render | 0x006bc0e0 |

The plugin owns two helper objects: an LFO (`0x120`, constructor
`FUN_006b90f0`) and a stereo ring (`0x128`, constructor `FUN_006b96b0`).

Plugin fields:

| offset | type | meaning |
|---|---|---|
| 0x130, 0x134 | f32 | delay and depth, ms |
| 0x138 | f32 | feedback gain |
| 0x13c | f32 | LFO rate, Hz |
| 0x140 | i32 | middle of the sweep, 1/4096 samples |
| 0x144 | i32 | half the depth, whole samples |
| 0x148, 0x14c | f32 | damped tap, left / right (also the feedback source) |
| 0x150 | f32 | damping coefficient |
| 0x154, 0x158, 0x15c | f32 | dry, wet, cross |
| 0x160 | i32[12] | the parameters, saved |
| 0x190 | f32[12] | the parameters as 0..1 |

LFO fields: `+0x8` i32[4096] the blended table, `+0x4008` the triangle,
`+0x8008` the sine, `+0xc008` the blended slopes, `+0x10008` and
`+0x14008` the slopes of the two shapes, `+0x18008` the left phase,
`+0x1800c` the right phase, `+0x18010` the step, `+0x18014` f32 sample
rate (44100 until the host sets one).

Ring fields: `+0x8`, `+0x10` f32 rings left / right, `+0x18` length,
`+0x1c` mask, `+0x20` write index, `+0x24` f32[4096] `i / 4096`.

## 1. State layout (event 213, 52 bytes, version 1)

`SaveRestoreState` (`FUN_006badc0`): `Write(i32 1); Write(this+0x160,
0x30)`. Restore reads the version, does nothing when it is 2 or more, else
reads 48 bytes and runs GetValue / UpdateValue (flags 2 and 0x11) over
every parameter.

`Patches/Mixer presets/Flanger send.fst` holds 48 bytes: a 0 and eleven
floats of an older build. FL takes the 0 as a version it reads and reads
the floats' bits as integers; this port turns the state down for its
length.

`v` is `value / Max` (all wheels have Min 0), kept as f32.

| byte | idx | parameter (DFM object) | range | default | confidence |
|---|---|---|---|---|---|
| 0..3 | | version | | 1 | loader, corpus, preset |
| 4..7 | 0 | Delay (`DelayWheel`) | 0..1024 | 0 | loader, dfm, corpus |
| 8..11 | 1 | Depth (`DepthWheel`) | 0..5000 | 2000 | loader, dfm, corpus |
| 12..15 | 2 | Rate (`RateWheel`) | 0..5000 | 2000 | loader, dfm, corpus |
| 16..19 | 3 | Phase (`PhaseWheel`) | 0..360 | 118 | loader, dfm, corpus |
| 20..23 | 4 | Damp (`DampWheel`) | 0..5000 | 0 | loader, dfm, corpus |
| 24..27 | 5 | Shape (`ShapeWheel`) | 0..1024 | 0 | loader, dfm, corpus |
| 28..31 | 6 | Feed (`FeedWheel`) | 0..100 | 23 | loader, dfm, corpus |
| 32..35 | 7 | Invert feedback (`InvFeedbackWheel`) | 0..1024 | 1024 | loader, dfm, corpus |
| 36..39 | 8 | Invert wet (`InvWetWheel`) | 0..1024 | 1024 | loader, dfm, corpus |
| 40..43 | 9 | Dry (`DryWheel`) | 0..5000 | 3500 | loader, dfm, corpus |
| 44..47 | 10 | Wet (`WetWheel`) | 0..5000 | 3000 | loader, dfm, corpus |
| 48..51 | 11 | Cross (`CrossWheel`) | 0..5000 | 2500 | loader, dfm, corpus |

The `Default` preset has phase 119 and feed 24 where the DFM has 118 and 23.

## 2. Parameter semantics (`ProcessParam` @ 0x006bb210)

On UpdateValue the integer is stored, `v` kept at `0x190[idx]`, and
`FUN_006bbb60` dispatches. `SR` is the host's rate as an int, `trunc` is
`cvttsd2si` (`0x409df0`), `round` is `cvtsd2si`, to nearest even
(`0x409d00`). The wheel curve `FUN_006baa10` is

```
c(x, k) = f32( (f64(k) + 1) * x / (f64(k * x) + 1) )      // k * x in f32
```

```
0, 1 FUN_006bbe30   delay = c(v0, -0.8) * 20            // ms, 0..20
                    depth = c(v1, -0.8) * 20            // ms, 0..20
                    0x140 = trunc((delay + depth * 0.5) * SR * 4096 / 1000)
                    0x144 = trunc(depth * 0.5 * SR / 1000)
2, 3 FUN_006bbda0   rate = c(v2, -0.9) * 5              // Hz, 0..5
                    LFO FUN_006b9410: step = round(f32(rate * 4096 / SRf * 4096))
                                      right phase = left phase + round(f32(v3 * 2^24))
4    FUN_006bbe00   0x150 = f32(1 - v4)
5    FUN_006bbf30   LFO FUN_006b94a0: a = round(v5 * 4096), b = round(f32(1 - v5) * 4096)
                                      table[i] = a * triangle[i] + b * sine[i]
                                      slope[i] = (a * dtriangle[i] + b * dsine[i]) >> 12   (i32, arithmetic)
6, 7 FUN_006bbf50   0x138 = f32((v7 > 0.5 ? -v6 : v6) * 0.98)
8..11 FUN_006bbfe0  0x158 = v8 > 0.5 ? -v10 : v10       // wet
                    0x15c = v8 > 0.5 ? -v11 : v11       // cross
                    0x154 = v9                          // dry
```

The rate and phase setter is called with a flag that puts the left phase
at 0; only the constructor and the flush pass it.

LFO shapes (`FUN_006b91e0`, in the LFO's constructor), entries
`trunc(x * 32767)`:

```
triangle   x from 0 up by 1/1024 for i < 1024, down by 1/1024 to i = 3071, then up again
sine       x = sin(i * 6.283185307 / 4096)       // 0x401921fb54411744, not quite 2 pi
slopes     d[i] = s[(i + 1) & 4095] - s[i]
```

So Shape 0 is the sine and 1024 the triangle.

## 3. Signal chain (`Eff_Render` @ 0x006bc0e0)

Per frame, all in f32 but the integer sweep:

```
for side in L, R (phases p, p + phase offset):
    i = (p >> 12) & 0xfff;  f = p & 0xfff
    m = (table[i] + f * slope[i]) >> 12                   // i32 sums, arithmetic shift: -32767..32767
p += step (both sides)
back = (half * m) >> 3 + center                           // 1/4096 samples, from center - half to center + half

ring[w] = in + damped * feed                              // per side
n = back >> 12;  f = back & 0xfff
a = ring[(w - n) & mask];  b = ring[(w - n - 1) & mask]
tap = a + (b - a) * (f / 4096)
w = (w + 1) & mask
damped += (tap - damped) * damp                           // 0x148 / 0x14c
outL = dampedL * wet + inL * dry + cross * dampedR
outR = dampedR * wet + inR * dry + cross * dampedL
```

The input is written before the tap is read, so a sweep reaching 0 reads
the input itself. The damped tap is both what is fed back and what is
heard. Nothing is flushed below a threshold.

The ring is the smallest power of two at least `round(SR / 10)` long
(`FUN_006b9830`; 8192 at 44.1 and 48 kHz); the longest sweep, 40 ms, fits.
Dispatcher 4 (sample rate, `FUN_006bc070`) resizes it with `ReallocMem`,
sets the LFO's rate and runs the delay and the rate setters again.

Dispatcher 11 with 1 (`FUN_006bad80`, the host switching the plugin on):
the rings zeroed, the write index and the left phase to 0, the damped taps
to 0. The constructor does the same.

The plugin adds no latency.

## Implementation notes (crates/plugins/src/flanger.rs)

- Bit for bit as above but for `sin` (Rust's, then truncated). `v` is the
  value times the wheel's f64 scale, `1 / (Max - Min)` (`0x00651f80` in
  the Vintage Chorus DLL, the same VCL code), as FL has it.
- A new sample rate makes new, empty rings, where FL's `ReallocMem` keeps
  what was there.
- The right phase is kept as the left plus the offset, which is what FL's
  setter makes it whenever either moves.
