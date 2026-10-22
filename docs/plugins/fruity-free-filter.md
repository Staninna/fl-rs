# Fruity Free Filter: state, parameters, signal chain

Recovered from `Fruity Free Filter_x64.dll` (FL Studio 2026, Delphi native
plugin, image base 0x400000), Ghidra project `fx6` and objdump / capstone
listings. Corpus: 4 states in 2 `.flp` (Ookay - Thief, NewStuff), each
version 2, 21 bytes, and the shipped presets.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `asm`.

## How the plugin is wired

`CreatePlugInstance` @ `0x006cc390` -> constructor `FUN_006cc810`, class
`0x006cc278`.

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006cc3e0 |
| 3 | SaveRestoreState | 0x006cc620 |
| 4 | GetName | 0x006cd450 |
| 6 | ProcessParam | 0x006ccac0 |
| 7 | Eff_Render | 0x006cd580 |

Plugin fields:

| offset | type | meaning |
|---|---|---|
| 0x120 | i32[4] | the parameters, saved |
| 0x130 | record | the filter: i32 type 1..7, f32 Hz, f32 Q (0..1), f32 gain dB, then f32 b0, b1, b2, a1, a2 at +0x10..+0x20 |
| 0x158 | f32[8] | its memory: x1 L, x1 R, x2 L, x2 R, y1 L, y1 R, y2 L, y2 R |
| 0x198 | record | the DC remover: f32 gain at +4, f32 damping at +8, f64 level / speed per side at +0x28..+0x40 |
| 0x200 | bool | center (remove DC) |

Eff_Render calls two routines through pointers the unit's initialiser
fills by the CPU (`0x006cb200`, `0x005e19xx`): the biquad `0x006cb0f0`
(else `0x006cae50`, the same sums plus a flush of tiny states) and the DC
remover `0x005e1190` in f64 (else `0x005de400` in x87).

## 1. State layout (event 213, 21 bytes, version 2)

`SaveRestoreState` (`FUN_006cc620`). Save: `i32 2`, the four parameters
(16 bytes), one byte for center. Restore: the version, nothing when it is
3 or more; else the 16 bytes, center off, and from version 2 on the byte
(`& 1`). A type under its Min (100) is an old numbering in tens and
becomes `100 + round(((type div 10) - 0.5) / 7 * 600)`. Then GetValue /
UpdateValue (flags 2 and 0x11) for the four.

| byte | idx | parameter (DFM object) | range | default | corpus |
|---|---|---|---|---|---|
| 0..3 | | version | | 2 | 2 x4 |
| 4..7 | 0 | Type (`TypeWheel`) | 100..700 | 100 | 100 x4 |
| 8..11 | 1 | Frequency (`FrequencyWheel`) | 0..1024 | 512 | 1024 x4 |
| 12..15 | 2 | Q (`QWheel`) | 0..1024 | 169 | 169 x2, 0 x2 |
| 16..19 | 3 | Gain (`GainWheel`) | 0..1024 | 676 | 676 x2, 705 x2 |
| 20 | | Center (`CenterSelector`, "Remove DC offset") | 0, 1 | 1 | 1 x2, 0 x2 |

## 2. Parameter semantics (`FUN_006cd1e0`, design `FUN_006cac20` / `FUN_006ca5c0`)

`v = clamp((value - Min) * scale, 0, 1)`, `scale = 1 / (Max - Min)` f64,
then f32. exp and ln are Delphi's (`0x00408810`, `0x00409870`), Power is
`exp(y ln x)` (`0x0045d180`), SinCos `0x005d0fc0`.

```
0  type = clamp(round(f64(v * 7) - 0.5) + 1, 1, 7)     // 1 LP, 2 BP, 3 HP, 4 notch, 5 low shelf, 6 peaking, 7 high shelf;
                                                     // the filter's memory is cleared when it changes
1  Hz   = f32( f64(f32(exp(v ln 101) - 1)) * 159.9 + 10 )   // 10 Hz .. 16 kHz
2  Q    = v
3  gain = v * 36 - 18                                 // dB
```

Every change runs the design (f64, each coefficient rounded to f32):

```
Hz = min(Hz, 22000)
w  = max(Hz * 1.4247585730565955e-4, 0.0004)        // 2 pi / 44100, a constant: the rate does not enter
s, c = sin w, cos w
types 1..4: alpha = s / ((Q * 18.9 + 0.1) * 2)
type 6:     A = Power(10, gain * 0.025); alpha = s / ((f32(exp(Q ln 1001) - 1) * 0.0999 + 0.1) * 2)
types 5, 7: A = Power(10, gain * 0.025); beta = sqrt((A^2 + 1) / (Q * 1.9 + 0.1) - (A - 1)^2) * s

1 LP:    n = 1 / (alpha + 1); b1 = n (1 - c); b0 = b2 = f32(b1) * 0.5; a1 = n c (-2); a2 = n (1 - alpha)
2 BP:    b0 = n alpha; b1 = 0; b2 = -n alpha; a1, a2 as LP
3 HP:    b1 = n (-(c + 1)); b0 = b2 = -f32(b1) * 0.5; a1, a2 as LP
4 notch: b0 = b2 = n; b1 = a1; a1, a2 as LP
6 peak:  n = 1 / (alpha / A + 1); b0 = n (alpha A + 1); b1 = a1 = n c (-2); b2 = n (1 - alpha A); a2 = n (1 - alpha / A)
5 low shelf:  m = (A - 1) c; n = 1 / (A + 1 + m + beta)
         b0 = n A ((A + 1 - m) + beta); b1 = n A 2 ((A - 1) - (A + 1) c); b2 = n A ((A + 1 - m) - beta)
         a1 = n ((A - 1) + (A + 1) c) (-2); a2 = n ((A + 1 + m) - beta)
7 high shelf: n = 1 / ((A + 1 - m) + beta)
         b0 = n A (A + 1 + m + beta); b1 = n A (-2) ((A - 1) + (A + 1) c); b2 = n A ((A + 1 + m) - beta)
         a1 = n ((A - 1) - (A + 1) c) 2; a2 = n ((A + 1 - m) - beta)
```

These are the cookbook's filters with `Q` as the bandwidth. The corner is
worked out at 44.1 kHz whatever the host's rate, and stops at 22 kHz.

The constructor (`FUN_006cc810`) runs the four defaults through the
design and then writes type 1, 100 Hz, Q 0.5, gain 0 into the record
without designing again: the next wheel moved on a new plugin designs from
those, not from the other wheels. A state sets all four.

## 3. Signal chain (`Eff_Render` @ 0x006cd580)

Per frame and side, in f32:

```
y = x b0 + (((x2 b2 + x1 b1) - y1 a1) - y2 a2)
```

Then, with center on, per side in f64 (`0x005e1190`):

```
speed += (x - level) * gain;  level += speed;  speed *= damping;  out = f32(x - level)
```

with `gain = f32(1.0070235 / SR)` and `damping = 0.96` (`0x3f75c28f`), set
with the memory cleared by Dispatcher 4 (the rate). After each block a
level or speed under 2^-24 in size becomes 0.

The plugin adds no latency. Flush clears nothing; this port's `reset`
clears both memories.

## Implementation notes (crates/plugins/src/free_filter.rs)

- As above with Rust's `exp`, `ln`, `sin_cos` and `sqrt` for Delphi's, and
  the f64 DC remover (FL's x87 one is the fallback for an old CPU).
