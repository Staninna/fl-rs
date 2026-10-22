# Fruity 7 Band EQ: state, parameters, signal chain

FL's old graphic EQ: seven faders over a low shelf, five peaks and a high
shelf. Recovered from `Fruity 7 Band EQ_x64.dll` (FL Studio 2026, Delphi
native plugin, image base 0x400000), headless Ghidra (project `eq7`,
decompiles in `/root/gh/out/eq7/`) plus disassembly read with capstone.
It links the same Delphi section library as Fruity Parametric EQ, in an
older form: single precision coefficients and state.
Corpus: no project uses it, natively or in a Fruity Wrapper (which swaps
it in under both "Fruity 7 Band EQ" and "Fruity 7 Band Equalizer"); 14 presets of 32 bytes in
`Patches/Plugin presets/Effects/Fruity 7 band EQ/`.

Confidence tags: `asm` (read from the disassembly), `code` (read in a
decompile), `dfm`, `preset`, `inferred`.

## How the plugin is wired

`CreatePlugInstance` @ `0x006b2de0` -> constructor `0x006b3440`, class
reference `0x006b2cc0` (`TTestPlug`, 0x438 bytes, parent
`TDelphiFruityPlug`).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006b2f60 |
| 3 | SaveRestoreState | 0x006b3340 |
| 6 | ProcessParam | 0x006b3750 |
| 7 | Eff_Render | 0x006b3c30 |

Object fields (`asm`):

| offset | type | meaning |
|---|---|---|
| 0x120 | i32[7] | the faders |
| 0x13c | i32[7] | each band's tail counter: 0 out, -1 always, n > 0 blocks left |
| 0x158 | section[7] | 0x68 each: +0 kind, +4 Hz, +8 width, +0xc dB (floats but the kind), +0x10 f32 b0 b1 b2 a1 a2, +0x28 0x40 bytes of state (the kernel uses f32 x1, x2, y1, y2 for L and R) |

The constructor (`asm`) sets band 0 to kind 5 (low shelf), band 6 to kind
7 (high shelf) and the others to kind 6 (peak); Hz from the table at
`0x00701eb4`: 63, 250, 500, 1500, 3000, 5000, 8000; widths from
`0x00701ed0`: 0.5, 2, 2, 2, 1, 1, 0.5; dB 0. Then every parameter goes
through `ProcessParam` with flag 0x11, as on a load.

## 1. State (event 213: 32 bytes)

`SaveRestoreState` (`0x006b3340`, `asm`): save writes an i32 1 and the 28
bytes at `+0x120`. Load reads the i32; if it is more than 1 nothing else is
read. Otherwise it reads 28 bytes over the faders and sets each through
`ProcessParam(i, GetValue, 0x11)`. Nothing is range-checked; FL's
controls cannot save a value outside their range, and the port turns such
a state down.

| bytes | idx | parameter (DFM object) | range | default |
|---|---|---|---|---|
| 0..3 | | version | 1 | |
| 4..31 | 0..6 | Band 1..7 level (`Level1Slider`..), 0.01 dB | -1800..1800 | 0 |

The `FreqLabel` captions (63, 250, 500, 1500, 3000, 5000, 8000) are the
band table's numbers (`dfm`). "Default" and "Flat" are all zeros
(`preset`).

## 2. Parameters (`ProcessParam` @ 0x006b3750, `0x006b3a50`, designer `0x006b2e30`)

```
share = clamp(double(v - min) * slider+0x3cc, 0, 1)      // +0x3cc = 1 / (max - min), set at 0x006469e0
dB    = float(share) * 36f + -18f                        // 0x006b3a50, constants at 0x006b3af0
tail  = dB == 0 ? 48 : -1                                // the state stays
```

The designer (`code` and `asm`):

```
w        = float(2 pi as float * Hz) / float(SR)          // SR: the i32 at 0x006f3dc4, 44100, written nowhere
sin, cos = SinCos(w) as floats                           // 0x0045cd70
A        = Power(10f, dB / 40f)                           // 0x0045d0b0: exp(y ln 10), 1 for y = 0
peak:    alpha = float(sin * Sinh(ln2/2 * width * w / sin))  // ln2/2 a double made at startup (0x005e1553); Sinh 0x0045d1e0 = (e^x - e^-x)/2
shelves: beta  = float(sqrt(float((A*A + 1) / width - (A - 1)*(A - 1))))
coefficients(section, A, alpha or beta, sin, cos)        // 0x006b0ef0, all doubles
```

The rate the designer divides by is a variable no instruction writes: a
data reference search (`DataRefs`) and a byte search for its address find
only the designer's read through the pointer at `0x00702830`. The
dispatcher's FPD_SetSampleRate (4) hands the rate to the host object at
`+0xcc` and redesigns nothing. So the bands are designed for 44100 Hz at
any rate: at 48 kHz each one sits 8.8% above its label (`asm`, `code`).

`0x006b0ef0` is the cookbook (`asm`), with `c = (A - 1) cos` and `s = beta
sin` for the shelves, each result rounded to a float:

```
peak:       inv = 1 / (alpha/A + 1); a1 = inv cos (-2); a2 = inv (1 - alpha/A)
            b0 = inv (alpha A + 1); b1 = a1; b2 = inv (1 - alpha A)
low shelf:  inv = 1 / ((A+1) + c + s)
            b0 = inv (A ((A+1) - c + s)); b1 = inv ((2A) ((A-1) - (A+1) cos)); b2 = inv (A ((A+1) - c - s))
            a1 = inv ((A-1) + (A+1) cos) (-2); a2 = inv ((A+1) + c - s)
high shelf: inv = 1 / ((A+1) - c + s)
            b0 = inv (A ((A+1) + c + s)); b1 = inv ((-2A) ((A-1) + (A+1) cos)); b2 = inv (A ((A+1) + c - s))
            a1 = inv ((A-1) - (A+1) cos) 2; a2 = inv ((A+1) - c - s)
```

The switch also has kinds 1 to 4 (low pass, band pass, high pass, notch)
and 8 (all pass); the plugin never picks them.

## 3. Signal chain (`Eff_Render` @ 0x006b3c30)

```
copy Source -> Dest when they differ
for band in 0..7: if tail != 0:
    if tail > 0: tail -= 1; if tail == 0: zero the band's state   // 0x005de220
    kernel(section, state, Dest, Dest, Length)
```

The kernel is a pointer at `0x00714550`, set at startup (the writes at
`0x006b1963` and `0x006b19a8`): `0x006b17f0`, single precision SSE, when
byte `+0x16` of the CPU record at `0x00710d30` is set, and `0x006b1550`,
x87, when it is not. The record is zeroed (0x18 bytes) and filled by the
CPUID code at `0x0059afa0`; its feature set is the dword at `+0x12`, and
no instruction writes `+0x16` (`DataRefs` finds only reads). So the x87
kernel is the live one, as `fruity-flangus.md` found for its renderer
(`code`).

`0x006b1550` is direct form I on the interleaved block, left then right,
with the coefficients and the state as floats on the x87 stack (`asm`):

```
acc = x1 b1 + x2 b2; acc -= y1 a1; acc -= y2 a2; acc += x b0   // in the x87 unit
x2 = x1; x1 = x; y2 = y1; y1 = float(acc); out = float(acc)
```

The sum stays in the x87 unit at the precision control Windows x64 starts
a thread with, a double's (`inferred`: the DLL sets no control word), and
the product of two floats is exact in it, so the port sums in `f64`.
After each block `0x005de160` makes every one of the eight state floats
that is under 2^-24 (the float at `0x006f3dec`, compared as bits) 0.

No ramp, no oversampling, no latency. The tail counts
host blocks, not samples: a band at 0 dB runs for 48 blocks after its
last move, with its state emptied before the 48th, and then drops out.
FPD_Flush (2) zeroes every band's state under the host lock
(`0x006b3d10`) and leaves the counters.

## 4. What stays unknown

- `SinCos`, `Ln`, `Exp` are Delphi's RTL; Rust's may be an ulp apart in
  double precision, which rounding to a float almost always hides.
- The SSE kernel `0x006b17f0`, which no machine selects, is not
  rebuilt: it rounds the sum to a float after every step.
- The GUI is not rebuilt.

## Implementation notes (crates/plugins/src/seven_band_eq.rs)

- All 14 presets parse and play.
- `set_sample_rate` changes nothing, as in FL.
- The tail counts calls to `process`, which are the host's blocks.
- Nothing is approximated: the port level is Exact.
