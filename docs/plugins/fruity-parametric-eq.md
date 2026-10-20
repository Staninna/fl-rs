# Fruity Parametric EQ: state, parameters, signal chain

The first parametric EQ, the one Fruity Parametric EQ 2 replaced. Recovered
from `Fruity Parametric EQ_x64.dll` (FL Studio 2026, Delphi native plugin,
image base 0x400000), headless Ghidra plus disassembly read with capstone.
It shares its Delphi DSP library with Parametric EQ 2: the same section
struct, the same double precision biquad and the same cookbook coefficient
code, at other addresses. Corpus: 9 states in the `.flp` files (Cuebur &
Thandi Draai - Africa's Calling 1, LeriQ - SHINE 6, Mehdi Ebrahiminejad -
Never Out 1, the render test 1) and 21 presets, all 116 bytes.

Confidence tags: `asm` (read from the disassembly), `dfm`, `corpus`,
`preset`.

## How the plugin is wired

`CreatePlugInstance` @ `0x006cf060` -> constructor `FUN_006cf6a0`, class
reference `0x006ced80` -> VMT `0x006cee48`.

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006cf0b0 |
| 2 | Idle_Public | 0x006cc270 |
| 3 | SaveRestoreState | 0x006cf5e0 |
| 4 | GetName | 0x006d0220 |
| 5 | ProcessEvent | 0x006cb900 |
| 6 | ProcessParam | 0x006cfbf0 |
| 7 | Eff_Render | 0x006d0350 |

Object fields:

| offset | type | meaning |
|---|---|---|
| 0x120 | i32[29] | the parameters: level x7, freq x7, width x7, type x7, main level |
| 0x194 | band[7] | 0x7c each: +0 tail counter, +4 section (kind, dB, Hz, width as floats), +0x14 f64[8] state, +0x54 f64[5] b0 b1 b2 a1 a2 |
| 0x4f8 | f32 | main gain |
| 0xcc | ptr | host rate block: +8 SR, +0x18 `1/SR`, +0x28 `(1/SR) 2 pi`, +0x30 `float(0.4999 SR)` (`0x005e0cf0`) |

## 1. State (event 213: 116 bytes)

`SaveRestoreState` (`FUN_006cf5e0`): save writes `this+0x120`, 0x74 bytes.
Load sets the main level to 128, reads 0x74 bytes over the parameters, then
for every parameter calls `ProcessParam(i, GetValue, 0x11)`, which applies
it as a moved control does. No version, no other fields.

| bytes | idx | parameter (DFM object) | range | default |
|---|---|---|---|---|
| 0..27 | 0..6 | Band 1..7 level (`Level1Slider`..), 0.01 dB | -1800..1800 | 0 |
| 28..55 | 7..13 | Band 1..7 freq (`Freq1Wheel`..) | 0..65536 | 5777, 13024, 19904, 33145, 42327, 49307, 55825 |
| 56..83 | 14..20 | Band 1..7 width (`BW1Wheel`..) | 0..65536 | 17500 |
| 84..111 | 21..27 | Band 1..7 type (`TypeSel1`..) | 0..7 | 5, 6, 6, 6, 6, 6, 7 |
| 112..115 | 28 | Main level (`VolWheel`) | 0..256 | 128 |

The preset "Default" is the defaults with bands 5 and 6 at width 24000.

## 2. Parameters (`ProcessParam` @ 0x006cfbf0, band designer `FUN_006cf980`)

Index `i < 29`: group `i div 7` (0 level, 1 freq, 2 width, 3 type), band
`i mod 7`; 28 is the main level. A type change first zeroes the band's state
(`FUN_006cfb70`), then every change of a band redesigns it under the host
lock (`asm`).

```
section kind = table 0x007207d8 [type] = 0 Off, 1 LP, 2 BP, 4 HP, 3 Notch, 6 LoShelf, 7 Peak, 8 HiShelf
Hz     = float(float(exp(float(v / 65536) * ln 101) - 1) * 159.9 + 10)      // 10 .. 16000
width  = float(v / 65536)
dB     = float(v) * 0.01f
tail   = 0 if Off; SR/5 if dB == 0 and kind >= 6 (shelves, peak); else -1
main   = v * (1/128f); 1 if that is 1, else float((exp(float(main * float(ln 11))) - 1) * 0.1f)
```

The cookbook designer `FUN_005dbfa0` (then `FUN_005db9d0` for the
coefficients, the same code as Parametric EQ 2's `FUN_005ec7e0`):

```
Hz clamped to host+0x30 (0.4999 SR); w0 = double(Hz) * host+0x28; sin, cos = SinCos(w0)
LP, BP, HP, Notch:  alpha = sin / ((width * 18.9 + 0.1) * 2)                  // Q 0.1 .. 19
Peak:               A = Power(10, dB * 0.025); q = float(exp(width * ln(1001)) - 1)
                    alpha = sin / ((q * 0.0999 + 0.1) * 2)                     // Q 0.1 .. 100
shelves:            A = Power(10, dB * 0.025); beta = sqrt((A^2 + 1) / (width * 1.9 + 0.1) - (A - 1)^2)
                    then the cookbook shelf with 2 sqrt(A) alpha = beta * sin  // slope 0.1 .. 2
```

`Power` is Delphi's: `exp(x ln 10)`, and 1 for `x = 0`. The coefficients are
the cookbook's, over `a0`: LP `b1 = (1 - cos)/a0, b0 = b2 = b1 * 0.5f`; HP
`b1 = -(1 + cos)/a0, b0 = b2 = -b1 * 0.5f`; BP `alpha/a0, 0, -alpha/a0`;
notch `1/a0, a1, 1/a0`; peak and shelves as in Parametric EQ 2's doc.

## 3. Signal chain (`Eff_Render` @ 0x006d0350)

```
if main param == 128: copy Source -> Dest
else: Dest = Source * main gain       // the routine at 0x005e02f0, no ramp
for band in 0..7: if tail != 0:
    biquad(section, Dest, Dest, Length)  // 0x005dc3c0: direct form I in doubles, denormal flush after the block
    if tail > 0: tail -= Length; if tail <= 0: tail = 0, zero the state
```

There is no silence detection, no sleep, no oversampling and no ramp: the
main level counts from the next block on. Each band is one second order
section; the seven run in series.

## 4. What stays unknown

- `SinCos` is Delphi's (x87); Rust's `sin` and `cos` may be an ulp apart.
- The GUI (`FUN_006cff80` text, the DFM) is not rebuilt.

## Implementation notes (crates/plugins/src/parametric_eq.rs)

- Every corpus state and preset parses; all are 116 bytes.
- Nothing is approximated: the port level is Exact.
