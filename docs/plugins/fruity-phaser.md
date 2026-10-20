# Fruity Phaser: state, parameters, signal chain

Recovered from `Fruity Phaser_x64.dll` (FL Studio 2026, Delphi native
plugin, image base 0x400000), Ghidra project `fx6` and capstone listings
with the constants each instruction loads. Corpus: 3 states in 3 `.flp`
(Never Out, Stroll Up, The Ogun Dream), 27 shipped presets, all version 1,
40 bytes.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `asm`.

## How the plugin is wired

`CreatePlugInstance` @ `0x006ba470` -> constructor `FUN_006ba8b0`, class
`0x006ba350`.

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006ba4c0 |
| 3 | SaveRestoreState | 0x006ba7b0 |
| 4 | GetName | 0x006bb8d0 |
| 6 | ProcessParam | 0x006bace0 |
| 7 | Eff_Render | 0x006bbba0 |

Two LFO objects (`0x2ac` left, `0x2b4` right; constructor `FUN_006b9030`,
tables `FUN_006b9140`).

Plugin fields:

| offset | type | meaning |
|---|---|---|
| 0x120, 0x1d8 | f32[23] | each all-pass stage's last output, left / right |
| 0x17c, 0x234 | f32 | the chain's last input, left / right (only entry 0 is used) |
| 0x2a0, 0x2a4 | f32 | the chain's last output, fed back |
| 0x2a8 | i32 | stages |
| 0x2bc | i32[9] | the parameters, saved |
| 0x2e0 | f32 | LFO rate, Hz |
| 0x2ec | f32[9] | the parameters as 0..1 |

LFO fields: `+0x8` f32[4096] the sine, `+0x10` its slopes, `+0x18`
`i / 4096`, `+0x20` f32 `2^24 / SR`, `+0x24` u32 phase (2^24 a cycle),
`+0x28` i32 step.

## 1. State layout (event 213, 40 bytes, version 1)

`SaveRestoreState` (`FUN_006ba7b0`): `Write(i32 1); Write(this+0x2bc,
0x24)`. Restore reads the version, does nothing when it is 2 or more,
else reads 36 bytes and runs GetValue / UpdateValue (flags 2 and 0x11).

| byte | idx | parameter (DFM object) | range | default |
|---|---|---|---|---|
| 0..3 | | version | | 1 |
| 4..7 | 0 | Sweep frequency (`SweepFreqWheel`) | 0..5000 | 2500 |
| 8..11 | 1 | Minimum depth (`MinDepthWheel`) | 0..1000 | 100 |
| 12..15 | 2 | Maximum depth (`MaxDepthWheel`) | 0..1000 | 800 |
| 16..19 | 3 | Frequency range (`FreqRangeWheel`) | 0..1024 | 0 |
| 20..23 | 4 | Stereo (`StereoWheel`) | 0..1024 | 512 |
| 24..27 | 5 | Number of stages (`NrStagesWheel`) | 0..22 | 7 |
| 28..31 | 6 | Feedback (`FeedbackWheel`) | 0..1000 | 400 |
| 32..35 | 7 | Dry - wet (`DryWetWheel`) | 0..1024 | 512 |
| 36..39 | 8 | Output gain (`OutGainWheel`) | 0..5000 | 4000 |

## 2. Parameter semantics (`ProcessParam` @ 0x006bace0, `FUN_006bb670`)

`v = clamp(value * scale, 0, 1)` with the wheel's f64 scale `1 / Max`,
kept as f32 at `0x2ec[idx]`. Then:

```
0, 3   rate = f32( (exp(f64(v0 * 4)) - 1) / (exp(4) - 1) * (f64(v3) <= 0.5 ? 2 : 10) )   // Hz; exp is Delphi's (0x00408810)
5      stages = clamp(value5 + 1, 3, 23)
```

GetName (`FUN_006bb0a0`) names the range "small" and "large", the stages
`value + 1`, and the dry - wet as `100 - round(100 v)` "% wet".

LFO tables (`FUN_006b9140`, once): for `i` in 0..4095, `x = i / 4096`
(f64): `under[i] = f32 x`, `sine[i] = f32((sin(x * 6.283185307) + 1) *
0.5)` (the not-quite 2 pi `0x401921fb54411744`), `slope[i] = sine[i + 1] -
sine[i]`, and the last slope back to entry 0. The LFO's value at a phase
is `sine[i] + under[f] * slope[i]` with `i` bits 12..23 and `f` bits 0..11.

## 3. Signal chain (`Eff_Render` @ 0x006bbba0)

Once a block:

```
feedback = f32(f64(v6) * 0.99)
gain     = f32(f64(v8) * 2 * sqrt(f64(1 - v6 * v6)))
min = v1;  span = v2 - v1
step     = round(f64(rate * (2^24 / SR)))           // both LFOs (FUN_006b9300)
right phase = left phase + round(f64(v4) * 2^23)    // up to half a cycle (FUN_006b9350)
```

Per frame and side, in f32:

```
a  = -min - lfo * span                              // the LFO from 0 to 1, so a from -min to -max
x  = in + last * feedback
stage 0:  y0 = a * (x - y0') + x'                    // ' the stage's values a frame ago
stage k:  yk = a * (y(k-1) - yk') + y(k-1)'          // k = 1 .. stages - 1   (FUN_006bba00)
last = y(stages - 1)
out = (v7 * (in - last) + last) * gain
```

So the dry - wet wheel at 0 is all chain and at its end all input; at half
the two sum, which is where a phaser's notches come from.

Dispatcher 2 (flush, `FUN_006ba6d0`): every stage and the feedback to 0,
the left phase to 0. Dispatcher 4 (rate): both LFOs' `2^24 / SR`. Nothing
else changes with the rate.

The plugin adds no latency.

## Implementation notes (crates/plugins/src/phaser.rs)

- As above, with Rust's `exp`, `sin` and `sqrt` for Delphi's.
- The right LFO is kept as the left one plus the stereo offset, which is
  what FL sets it to every block.
