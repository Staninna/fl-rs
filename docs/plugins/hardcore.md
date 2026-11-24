# Hardcore: state, rack, Distortion pedal

Recovered from `Hardcore_x64.dll` (MSVC C++ with RTTI, image base
0x180000000), FL Studio 2026. Ghidra project `/root/ghidra-projects/distructor`
(`hc`), the plugin's own code (0x180001000..0x180030000) decompiled into one
file; the CRT maths read from the listing. Corpus: Treben - Umziky - All the
same (468 bytes) and the render test `System/Render tests/Hardcore.flp`; 2
presets under `Plugin presets/Effects/Hardcore`.

Confidence tags: `asm`, `dec`, `corpus`.

## 1. State

FL's wrapper (`FUN_180002f90`, `dec`): `SSLF`, an i32 length, then the
plugin's chunk (`FUN_18002aca0` writes, `FUN_18002ad90` reads, 0x1cc
bytes): `1RCH` (`DRCH` is a demo's and loads the default program), 50 f32
parameters handed to `setParameter` in order, then the program's name in
256 bytes.

| parameter | name (`FUN_180008390`) | what (`FUN_18002a010`, `dec`) |
|---|---|---|
| 0, 1 | Master IN Lvl, Master OUT Lvl | `10^(2.4 v - 1.2)` (`powf`), glided `g += (t - g) · 0.001` a sample |
| 2 | EQ Enable | `v != 0` |
| 3..10 | EQ Band 1..8 | `±18 ((2v - 1)^2)` dB on a band of `FUN_18001d660` a side |
| 11 | (cabinet) | `trunc(v)`: 0 none, else the impulse of that place in `Imp` |
| 13..37 | | parameter `k` of box `i` is `13 + 5 i + k` |
| 39..43 | Box 1..5 Enable | a box runs when its value is not 0 |
| 45..49 | (pedals) | `trunc(v)` clamped to -1..11: 1 `CPedal_DistModelA` and ten more pedals (compressor, chorus, delay, EQ, flanger, modulator, noise gate, phaser, reverb, tube drive), anything else none |

Treben's state: box 1 a Distortion (drive 0.3475, length 0.3375, tone 0.5,
peak 0.5575), on; the equaliser off; no cabinet; the levels 0.5025 and 0.5.
The render test: the same pedal in box 1, on, and cabinet 4.

## 2. Process (`FUN_18002a740`, `dec`)

```
x = hp30(x) * g_in        each side, HP of FUN_18001d050 at 30 Hz, Q 1
for box 1..5: if on, pedal.process(left, right)
cabinet (FUN_180027260): a direct FIR, the impulse resampled to the rate
EQ (if on): eight peaking sections a side (FUN_18001da00)
x = x * g_out
```

A level starts at -1 after a resume (`FUN_18002a5a0`), which the first
block takes as "start on the target".

## 3. Distortion (`CPedal_DistModelA`, VMT `0x1800757f8`)

Constructor `FUN_18002bad0`, process `FUN_18002bc00`, rate
`FUN_18002c060`, reset `FUN_18002c220`, parameter `FUN_18002c2b0`, update
`FUN_18002c2f0`. All `asm` and `dec`.

Parameters (box parameters 0..3): drive `d`, comb length `l`, tone `t`,
peak `p`.

```
drive  = d^2 · 99 + 1          makeup = 1 / (8 d + 1)
tone   = high pass (FUN_18001d050) at 85 Hz, Q 30 t + 1
peak   = peaking EQ (FUN_18001d660 type 1) at 2500 Hz, 3 octaves, (p - 0.5) · 20 dB, A = 10^(dB/20)
comb1  = int(8 SR · 0.001) samples          comb2 = int(int(8 SR · 0.0002) · l), under that
```

A frame (`m = L + R`):

```
a = bp200(m) · 0.05             band pass of FUN_18001d420, 200 Hz, Q 0.99, at SR
b = bp1000(m) · 5               the same at 1000 Hz
for t = 0, 1/8 .. 7/8:          Catmull-Rom between the last two of each band
    ya = shape(a(t)), yb = shape(b(t))
    shape(y) = 1 - 10^(-drive y) (1 - y)  for y >= 0,  10^(drive y) (y + 1) - 1  under
    r = ring1[i];  ring1[i] = yb - 0.25 r;  s = 0.7 r + ya
    if comb2: r2 = ring2[j]; ring2[j] = s; s = s - 0.3 r2
    v = lp500(s)                low pass of FUN_18001d240, 500 Hz, Q 4, designed at SR
out = peak(tone(v · makeup)), both sides
```

The second-order sections (`FUN_18001d050` high, `FUN_18001d240` low,
`FUN_18001d420` band) come from `sincosf` (`FUN_180055e50`) and keep three
coefficients each: high `[(c + 1)/(s/Q + 2), -2 c Q/(s + Q), (Q - s)/(s +
Q)]`, low the same with `1 - c`, band (in f64) `[s/2, 2 c, 1 - s/(2Q)] / (1
+ s/(2Q))`. A value nearer 0 than 1e-9 is 0 in and out of each.

## 4. Implementation notes (crates/plugins/src/hardcore.rs)

- Built: the state, the levels and their glides, the input's high pass,
  the five boxes, the Distortion pedal.
- Left out and named by `fidelity()`: the other ten pedals (a box with one
  passes), the cabinet (the render test has one; FL's corpus project has
  none), the equaliser (off in both).
- `powf`, `sincosf` and `sinh` from MSVC's CRT are Rust's f64 ones rounded
  to f32. The trial-version path at the end of the process (a flag from
  `FUN_180006b20`) is not played: FL's copy is registered.
- The default program is taken as Treben's "--- Defult ---" with level 0.5;
  the plugin's own preset bank (`hdprg`) was not read.
