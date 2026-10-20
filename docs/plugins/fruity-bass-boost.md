# Fruity Bass Boost: state, parameters, signal chain

Recovered from `Fruity Bass Boost_x64.dll` (FL Studio 2026, Delphi native
plugin, image base 0x400000), Ghidra project `fx6` and capstone / objdump
listings. Corpus: 2 states in Sid De La Cruz - Secret Agent Rendezvous,
version 1, 12 bytes, and the shipped presets.

Confidence tags: `loader`, `dfm`, `corpus`, `asm`.

## How the plugin is wired

`CreatePlugInstance` @ `0x006c8060` -> constructor `FUN_006c83b0`, class
`0x006c7f18`.

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006c80b0 |
| 3 | SaveRestoreState | 0x006c82b0 |
| 4 | GetName | 0x006c8bf0 |
| 6 | ProcessParam | 0x006c8610 |
| 7 | Eff_Render | 0x006c8cb0 |

Plugin fields: `0x120` i32[2] the parameters, `0x128` i32 Hz, `0x12c` f32
amount wheel, `0x130` f32 amount, and the filter record at `0x134`: f32 k,
f32 damping, then f64 level and speed for the left and the right side
(`+8..+0x20`). Reset (`FUN_006c8290` -> `FUN_006c8030`) zeroes the four
f64.

Eff_Render calls the filter through a pointer the unit's initialiser
(`0x006c8eb0`) sets by the CPU: `0x006c8d80` in f64, else `0x006c8ce0` in
x87 with a flush of tiny states.

## 1. State layout (event 213, 12 bytes, version 1)

`SaveRestoreState` (`FUN_006c82b0`): `Write(i32 1); Write(this+0x120,
8)`. Restore reads the version, does nothing when it is 2 or more, else
reads 8 bytes and runs GetValue / UpdateValue (flags 2 and 0x11).

| byte | idx | parameter (DFM object) | range | default | corpus |
|---|---|---|---|---|---|
| 0..3 | | version | | 1 | 1 x2 |
| 4..7 | 0 | Frequency (`FrequencyWheel`) | 0..1024 | 512 | 1024 x2 |
| 8..11 | 1 | Amount (`AmountWheel`) | 0..1024 | 768 | 1024, 836 |

## 2. Parameter semantics (`FUN_006c8810`)

`v = clamp(value * scale, 0, 1)` with the wheel's f64 scale `1 / Max`,
then f32. cos, exp and ln are Delphi's (`0x00409400`, `0x00408810`,
`0x00409870`); `Power(single, 3)` is `IntPower` (`0x0045cd10`): the base
squared in f64, times the base, rounded to f32.

```
0  Hz   = round(f64(v * 80 + 20))                         // 20..100 Hz
   c    = f32 cos(Hz * 2 pi / SR)                          // SR the host's, an integer
   k    = 2 - c * 2
   damp = f32( (sqrt 2 * sqrt(f64(-IntPower(c - 1, 3))) + f64((c - 1) * 0.5)) / f64((c - 1) * 0.5) )
1  amount = f32(exp(v ln 11) - 1) * 0.5                    // 0..5
```

GetName prints the Hz and the amount.

Dispatcher 4 (rate) only tells the host object: the filter keeps the
coefficients of the rate the frequency wheel was last moved at.

## 3. Signal chain (`0x006c8d80`)

Per frame and side, in f64:

```
speed += (x - level) * k
level += speed
speed *= damp
out = f32(x + level * amount)
```

`k` puts the low-pass's corner at the frequency (`k = 2 - 2 cos w` is
`w^2` for small `w`) and `damp` is `1 - 2w` for small `w`, which damps it
critically: at the corner the low-pass is half the input a quarter turn
late, and a level that stays comes out times `1 + amount`.

The plugin adds no latency.

## Implementation notes (crates/plugins/src/bass_boost.rs)

- As above, with Rust's `cos`, `exp` and `ln` for Delphi's.
- A sample-rate change works the frequency out again, where FL keeps the
  old coefficients.
