# Fruity Parametric EQ 2: state, parameters, signal chain

Recovered from `Fruity Parametric EQ 2_x64.dll` (FL Studio 2026, Delphi native
plugin, image base 0x400000) plus the C++ `engine_x64.dll` it loads from the
Frequency Splitter folder (`Plugins/Fruity/Effects/Frequency Splitter/`,
exports `mydispatcher` and `dispatch`), which supplies the "new HQ" filter
designer, the linear-phase convolver and one of the two oversamplers. Ghidra
project `ghidra/proj`; decompiles under `ghidra/out/peq2/` (`vmt/` SDK slots,
`dsp/` designer and kernels, `poly/` oversampler) and `ghidra/out/fseng/`
(the engine, imported as `fs_engine_x64.dll`). Corpus: 1138 slots in the 165
`.flp`, 13 shipped presets plus "Guitar stuff" in a template. Four Ghidra runs
on the plugin, two on the engine. Nothing in FLEngine is called for audio.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `inferred`, `asm`.

## How the plugin is wired

`CreatePlugInstance` @ `0x006ebe90` -> constructor `FUN_006ecd00`, class
reference `PTR_PTR_006ebaa0` -> VMT `0x006ebb68` (`TTestPlug`).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006ebee0 |
| 2 | Idle_Public | 0x006e3760 |
| 3 | SaveRestoreState | 0x006ec340 |
| 4 | GetName | 0x006f0210 |
| 5 | ProcessEvent | 0x006e2df0 |
| 6 | ProcessParam | 0x006ef6f0 |
| 7 | Eff_Render | 0x006f0da0 |

Object fields the audio path touches:

| offset | type | meaning |
|---|---|---|
| 0x120 | i32[36] | the parameters: level x7, freq x7, width x7, type x7, order x7, main level |
| 0x1b0 | i32[36] | the spare state (A/B compare), same layout |
| 0x2d0 | f32[36] | smoothed copies of the parameters the designer reads |
| 0x360 | u8 | HQ mode: 0 = oversample 2x, 1 = matched coefficients (no latency) |
| 0x361 | u8 | HQ button |
| 0x364 | i32 | unknown, saved, 0 everywhere |
| 0x368 | u8 | "View band tokens" button state (form+0x820) |
| 0x36c | i32 | linear phase FFT size step (0..), default 2 |
| 0x370 | u8 | linear phase on |
| 0x374 | i32 | linear phase window selector, table `0x0074e708` = 0.1, 0.25, 0.01, 0.02 |
| 0x37c | u8 | use the engine resampler (zero-stuff + FIR) instead of the built-in polyphase |
| 0x380 | i32 | parameter smoothing: 0 off, 1 = 5 ms, 2 = 10 ms (`0x0074e6f0`) |
| 0x384 | band[7] | DSP band structs, 0x1f8 each (below) |
| 0x114c | band[7] | GUI copies of the band structs, 0x228 each (freq Hz at +0x10, dB at +0xc, width at +0x14) |
| 0x2064 / 0x2068 | f32 | main gain target / current (ramped) |
| 0x206c | f64 | `2 pi / SR` |
| 0x2078, 0x20e8..0x20ea, 0x20f8, 0x20f9, 0x210c | u8 | GUI flags, saved (analyzer on = 0x20f9) |
| 0x20fc..0x2110 | i32 | analyzer settings, saved |
| 0x2080 | i32 | sample rate; 0x2094 = SR/5 (tail length) |
| 0x2084 / 0x208c | ptr | `TPolyphase` up / down (built-in 2x oversampler) |
| 0x2098 | i32 | tail countdown; 0xc0000001 = asleep |
| 0x209c | i32 | monitor: 1 = scope input, 2 = scope output |
| 0x20b0 / 0x20b8 | ptr | engine `mydispatcher` / `dispatch` |
| 0x20c0 | ptr | engine linear-phase context; 0x20c8/0x20d0 up L/R, 0x20d8/0x20e0 down L/R |
| 0x2114 | u8[7] | band enabled |
| 0x2148 | ptr | `TParameterSmoother` |

Band struct at `0x384 + b * 0x1f8`:

| offset | meaning |
|---|---|
| +0 | tail counter: 0 off, -1 active, SR/5 counting down (peak/shelf at 0 dB) |
| +4 | sections - 1 = abs(order) |
| +8 | section 0; sections are 0x78 apart |
| section +0 | kind code: table `0x0074e838` by type = 0, 1 LP, 2 BP, 4 HP, 3 Notch, 6 LoShelf, 7 Peak, 8 HiShelf |
| section +4 / +8 / +0xc | gain dB / freq Hz / width 0..1 (f32, section 0 only) |
| section +0x10 | f64[8] biquad state: x1 L R, x2 L R, y1 L R, y2 L R |
| section +0x50 | f64[5] b0, b1, b2, a1, a2 |
| +0x1e8 | `1 / n` (n = sections) |
| +0x1ec / +0x1f0 | `0.7079 sqrt(n)` and its inverse |
| +0x1f4 | order < 0 ("Steep") |
| +0x1f5 | needs redesign (smoothing path) |

## 1. State layout (event 213: 354 bytes; 350, 309, 305 in older files)

`SaveRestoreState` (`FUN_006ec340`) is versioned; the writer is version 8.

```
save:    Write(i32 8); Write(this+0x120, 0x120)              // 72 ints: current + spare
         Write(i32 hq | hqmode << 1); Write(i32 0x364); Write(u8 0x368); Write(i32 monitor)
         Write(i32 flag2078); Write(i32 0x36c); Write(i32 0x374); Write(u8 bits); Write(i32 bandsOn); Write(i32 0x212c)
         Write(i32 0x20ec); Write(i32 0x2100); Write(i32 0x2104); Write(i32 0x2108); Write(i32 0x20fc); Write(i32 0x2110)
         Write(i32 0x380)
restore: Read(ver); Read(params, 0x120); Read(i32 f): hq = f & 1, hqmode = f >> 1; Read(0x364); Read(u8 0x368); Read(i32 monitor)
         bands all on; flag2078 = 0; if ver > 2: Read(i32 f): flag2078 = f & 1
         if ver < 4: 0x36c = 2, 0x374 = 0, linphase = 0, 0x37c = 0
         else: Read(0x36c); Read(0x374); Read(u8 bits); Read(i32 bandsOn: bit b = band b on); Read(0x212c)
         if ver > 4: Read(0x20ec)
         if ver < 6: 0x20f8 = 1  else: bits 0x20/0x40/0x80 -> 0x20f8/0x20f9/0x210c; Read five analyzer ints
         if ver > 6: Read(0x380)
         GetValue/UpdateValue loop, redesign every band, reconfigure oversampler and smoother
```

`bits`: 1 linear phase, 2 engine resampler (0x37c), 4 -> 0x20e8, 8 -> 0x20e9,
0x10 -> 0x20ea, 0x20 -> 0x20f8, 0x40 analyzer on, 0x80 -> 0x210c. Versions 7
and 8 have the same bytes; what changed is not visible in the loader.

Corpus: version 2 x21 (305 bytes), 3 x328 (309), 6 x199 (350), 7 x100 and 8
x490 (354). A version 2 block has no `flag2078`; the 13-byte tail is `f,
0x364, u8, monitor`.

| byte | type | idx | parameter (DFM object, hint) | range | default | corpus | confidence |
|---|---|---|---|---|---|---|---|
| 0..3 | i32 | | version | | 8 | 2, 3, 6, 7, 8 | loader, corpus |
| 4..31 | i32[7] | 0..6 | Band 1..7 level (`Level1Slider`..), 0.01 dB | -1800..1800 | 0 | -1800..1800 | loader, dfm, corpus, preset |
| 32..59 | i32[7] | 7..13 | Band 1..7 freq (`Freq1Wheel`..) | 0..65536 | 10923, 18204, 25486, 32768, 40050, 47332, 54613 | 0..65536 | loader, dfm, corpus, preset |
| 60..87 | i32[7] | 14..20 | Band 1..7 width (`BW1Wheel`..) | 0..65536 | 40124, 25412 x5, 40124 | 0..65536 | loader, dfm, corpus, preset |
| 88..115 | i32[7] | 21..27 | Band 1..7 type (`TypeSel1`..) | 0..7 | 5, 6, 6, 6, 6, 6, 7 | 0 x1397, 6 x4347, 7 x831, 5 x648, 3 x481, 1 x257, 4 x4, 2 x1 | loader, dfm, corpus, preset |
| 116..143 | i32[7] | 28..34 | Band 1..7 order (`OrderSel1`..) | -3..3 | 0 | 0 x7325, -3 x537, -1 x40, 3 x28 | loader, dfm, corpus, preset |
| 144..147 | i32 | 35 | Main level (`MainLevelSlider`), 0.01 dB | -1800..1800 | 0 | 0 x988, else -689..610 | loader, dfm, corpus, preset |
| 148..291 | i32[36] | | spare state, same layout (A/B "Store in spare state") | | defaults | mostly defaults | loader, preset |
| 292..295 | i32 | | bit 0 HQ button, bits 1+ HQ mode | | 3 | 1 (most), 3, 0 | loader, corpus, preset |
| 296..299 | i32 | | unknown (0x364) | | 0 | 0 | loader |
| 300 | u8 | | view band tokens | | 1 | 0, 1 | loader, preset |
| 301..304 | i32 | | monitor (`MonitorSel`): 0 off, 1 in, 2 out | | 2 | 2 | loader, dfm, preset |
| 305..308 | i32 | | flag 0x2078 (v3+) | | 1 | 0, 1 | loader |
| 309..312 | i32 | | linear phase size step (v4+) | | 2 | 2 | loader, preset |
| 313..316 | i32 | | linear phase window (v4+) | 0..3 | 0 | 0 | loader |
| 317 | u8 | | bits (above) | | 0x40 | 0x40, 0x42, 0x60, 0x62, 0x66 | loader, corpus, preset |
| 318..321 | i32 | | bands enabled, bit per band | | 0x7f | 0x7f | loader, preset |
| 322..325 | i32 | | 0x212c | | 1 | 1 | loader |
| 326..329 | i32 | | 0x20ec (v5+) | | 0 | 0 | loader |
| 330..349 | i32[5] | | analyzer: 0x2100, size index (`0x0074e6a4` = 2048..16384), 0x2108, 0x20fc, 0x2110 (v6+) | | 3, 1, 2, 2, 2 | same | loader, preset |
| 350..353 | i32 | | parameter smoothing (v7+) | 0..2 | 0 | 0 | loader, preset |

Preset check (derived with section 2: `Hz = 20 * 1000^(f/65536)`, dB = v/100,
width as the designer sees it). Type names are the `GetName` strings
(`0x0074e858`), order names `0x0074e898`: Steep 8/6/4, 2, Gentle 4/6/8.

| preset | ver | bands | reading |
|---|---|---|---|
| Default | 8 | LoShelf 63 Hz, Peak 136 / 294 / 632 / 1363 / 2936 Hz, HiShelf 6324 Hz, all 0 dB, widths 61 % (S 1.0) and 39 % (2.0 oct) | flat; HQ 3 (new mode) |
| 3 bands | 8 | LoShelf 63 Hz, Peak 632 Hz (3.1 oct), HiShelf 6324 Hz, rest Off | |
| Kick | 6 | Peak 48 Hz +2.6 dB 1.07 oct, Peak 532 Hz -2.7 dB 1.14 oct, Peak 4022 Hz +1.0 dB 2.0 oct; spare holds a 3rd-type band at 199 Hz -4.3 dB | the one preset with a non-default spare state |
| 40Hz cut | 6 | HP nominal 32 Hz, Q 0.71 (Butterworth), Steep 8 | the name is loose: 8 staggered poles put the -3 dB point above the nominal 32 Hz |
| 20Hz + 18kHz cut | 6 | HP 20 Hz Steep 8, LP 18705 Hz Steep 8 | f = 0 is exactly 20 Hz |
| Old Telephone | 8 | LoShelf 540 Hz -18 dB S 1.72, Peaks 107 / 221 / 16336 Hz -18 dB, Peak 2245 Hz +8.8 dB, HiShelf 6858 Hz -18 dB S 1.66 | |
| Vocal Presence | 8 | LoShelf 52 Hz -18 dB, Peak 355 Hz -5.7 dB 1.06 oct, Peak 12105 Hz +3.7 dB 1.62 oct | |
| Radio | 8 | LoShelf 319 Hz -17.7 dB S 2.05 (width 0), Peaks 144 / 452 / 1225 / 1988 / 2497 Hz (-0.1, -3.5, 0, +7, -5.3 dB), HiShelf 4277 Hz -18 dB | |

The width wheel's default depends on type (`ProcessParam` idx group 3): LP/HP
get `Round((1 - (1/11)^(1/3)) * 65536) = 36068`, which section 2 turns into Q
exactly 1.0 (0.707 after the sqrt 2); BP/Notch/Peak get 25412 (2.0 octaves);
shelves 40124 (S = 1.0). The 40Hz presets carry 36068, which confirms the
mapping.

## 2. Parameter semantics (`ProcessParam` @ 0x006ef6f0, designer `FUN_006ef120` + `FUN_006ee6e0`)

Index `i`: group `i div 7` (0 level, 1 freq, 2 width, 3 type, 4 order), band
`i mod 7`; 35 is the main level. `REC_FromMIDI` uses 2^-30 at `0x006efce0`.
Level, freq and width go through the smoother (`FUN_006f2090`); when smoothing
is off they land in `this+0x2d0[i]` at once and the band is redesigned under
the host lock. Type and order zero the band's states and redesign.

Startup constants (`FUN_005f41c0`, `FUN_006fbf60`, `FUN_005af520`): `ln 2`
at `0x0075f490`, `ln 2 / 2` at `0x0075f4a0`, `ln 10 / 20 = 0.11513` at
`0x0075f4dc`, `ln 1000 / (65536 ln 2)` at `0x00761620`, `1 / sqrt 2` at
`0x0075ecf0`; .data floats 0.01 @`0x0073a2f0`, 1/65536 @`0x0073a2f8`, 0.5
@`0x0073a2d8`.

```
dB      = level / 100                                   // -18 .. +18
Hz      = 20 * 1000 ^ (freq / 65536)                    // 20 .. 20000, exp(freq * K1 * ln2) * 20.0 (double at 0x006ef358)
w01     = width / 65536
main    = 10 ^ (mainlevel / 2000)                       // exp(v * 0.01 * 0.11513) -> this+0x2064
n       = |order| + 1                                   // cascaded sections; order < 0 = "Steep"
w0      = min(Hz * 2 pi / SR, 0.999 pi)                 // 3.13845 @0x006ef060; peaks use 0.7 pi (2.19911 @0x006ef098)
```

Per band type (RBJ cookbook, `FUN_005ec7e0`, with doubles 1.0, -2.0, 2.0 at
`0x005ecd80..a0`):

```
LP (1), HP (3):   Q = (1 - w01)^3 * 9.9 + 0.1             // 0.1 .. 10; default w01 gives 1.0
                  Gentle: Q = Q ^ (1/n) for every section
                  Steep:  Q_i = Q ^ (1/n) * TQ[n-1][i]     // TQ = 0x0074e758: {1}, {1.983, 0.749}, {2.643, 1.082, 0.694}, {3.576, 1.142, 1.045, 0.655}
                  alpha = sin w0 * 0.7071 / Q_i             // so the knob's Q is divided by sqrt 2: 1.0 -> Butterworth
                  LP: b1 = (1 - c)/a0, b0 = b2 = b1/2;  HP: b1 = -(1 + c)/a0, b0 = b2 = -b1/2;  a1 = -2c/a0, a2 = (1 - alpha)/a0, a0 = 1 + alpha
BP (2), Notch (4): BW = (w01 * 4.9 + 0.1) octaves          // 0.1 .. 5.0
                  BP: BW *= 0.7079 sqrt n;  Notch: BW /= 0.7079 sqrt n
                  alpha = sin w0 * sinh(ln2/2 * BW * w0 / sin w0)
                  BP: b0 = alpha/a0, b1 = 0, b2 = -alpha/a0;  Notch: b0 = b2 = 1/a0, b1 = a1
Peak (6):         BW = w01 * 4.9 + 0.1;  A = 10 ^ (dB / (40 n))
                  Steep (n > 1): section i gets f_i = Hz * 2 ^ (TO[n-1][i] * BW / n), BW_i = BW / n * TW[n-1][i], dB_i = dB * TG[n-1]
                      TO = 0x0074e798: {-.25, .25}, {-.475, 0, .475}, {-.775, -.3, .3, .775};  TW = 0x0074e7e8 (1 or pi/2);  TG = 0x0074e7d8: .5, .32125, .2442, .214445 (dB scale, replaces 1/(2n))
                      n = 2 also subtracts (BW_i - 1) * 0.01 from TG
                  alpha = sin w0 * sinh(ln2/2 * BW * w0 / sin w0)
                  a0 = 1 + alpha/A;  b0 = (1 + alpha A)/a0;  b1 = a1 = -2c/a0;  b2 = (1 - alpha A)/a0;  a2 = (1 - alpha/A)/a0
LoShelf (5), HiShelf (7): A = 10 ^ (dB / (40 n));  S = (1 - w01) * 1.7193 + 0.3333     // 0.33 .. 2.05
                  Steep: S_i = S * TS[n-1][i]              // TS = 0x0074e718: {2.630, 0.616}, {5.161, 1.109, 0.510}, {10.319, 1.514, 0.694, 0.553}
                  beta = sqrt A * sqrt((A + 1/A)(1/S - 1) + 2);  2 sqrt(A) alpha = beta * sin w0
                  low:  a0 = (A+1) + (A-1)c + beta s;  b0 = A((A+1) - (A-1)c + beta s)/a0;  b1 = 2A((A-1) - (A+1)c)/a0;  b2 = A((A+1) - (A-1)c - beta s)/a0
                        a1 = -2((A-1) + (A+1)c)/a0;  a2 = ((A+1) + (A-1)c - beta s)/a0
                  high: the mirror (signs of the c terms flipped, b1 = -2A((A-1) + (A+1)c)/a0)
```

So "Gentle 4" is two identical second-order sections with the gain or Q split
between them, and "Steep 4/6/8" is a table-staggered cascade (Butterworth-like
Q spread for LP/HP, spread centre frequencies for peaks, spread slopes for
shelves). A band whose type is Off has zero sections. A peak or shelf at
exactly 0 dB keeps processing for SR/5 samples, then switches its sections off
and clears them (`FUN_006ef120`, tail counter), which is why toggling a band's
level between 0 and 0.01 dB changes the CPU load but not the sound.

New HQ mode (`0x360 == 1` with the HQ button, `FUN_006f1c60`; the default in
version 8 states): the plugin does not use the cookbook. For each section the
designer `FUN_006ee6e0` calls `FUN_006f1c80`, which calls the engine's
`dispatch` (`0x1801e0d90`) as `dispatch(kind, out, float(Hz / SR * 2),
float(Q), float(dB))` and stores the returned b0, b1, b2, a1, a2. The section
kind codes map LP 1 -> 2, BP 2 -> 9, Notch 3 -> 10, HP 4 -> 3, LoShelf 6 -> 5,
Peak 7 -> 6, HiShelf 8 -> 8. The frequency is not clamped. The arguments per
kind (`asm`):

| kind | Hz | Q | dB |
|---|---|---|---|
| LP, HP | band Hz | `Q_i * sqrt(0.5)`: `Q_i` as for the cookbook, pow'd and steep-scaled | band level (unused) |
| BP, Notch | band Hz | `1 / (2 sinh(ln2/2 BW))`, `BW` with the `0.7079 sqrt n` spread | band level (unused) |
| Peak | band Hz, or the steep centre `Hz 2^(TO BW / n)` unrounded | the same Q from `BW_i` | twice the amplitude dB: `2 dB / 2n`, or `2 dB TG'` when steep |
| shelves | band Hz | `1 / sqrt((A + 1/A)(1/S_i - 1) + 2)`, `A` the cookbook's float | `float(dB * (1/n))` |

The engine routine (`FUN_1801de630`, cases by `kind`, jump table at
`0x1801e0d38`) was transcribed instruction by instruction and checked against
the DLL's own code run under an emulator (unicorn) on 21,000 random calls:
with the DLL's `sin`/`cos`/`exp`/`cosh`/`log`/`pow` (`0x1801f1580`,
`0x1801ed000`, `0x1801edf20`, `0x1801eda80`, `0x1801ee600`, `0x1801ef010`)
in place of Rust's it is the same to the bit; with Rust's libm it is within
1e-11 relative (one ill-conditioned band pass at Q 0.08: 2e-9). Every design
works in `f` = frequency over Nyquist; `g = 10^(dB/20)`.

- Band pass (9) and notch (10): poles by impulse invariance,
  `e = exp(-f pi/2 / Q)`, `a1 = -2 e cos(pi f sqrt(1 - 1/4Q^2))` (`cosh` for
  Q <= 0.5), `a2 = e^2`. Notch zeros on the circle at `f`, scaled for unity
  at DC. Band pass by Vicanek's matched form (`phi1 = sin^2(pi f/2)`,
  `A0 = (1 + a1 + a2)^2`, `A1 = (1 - a1 + a2)^2`, `A2 = -4 a2`; `R2`, `B2`,
  `B1`, then `b1 = -sqrt(B1)/2`, `b0 = (sqrt(B2 + B1/4) - b1)/2`,
  `b2 = -b0 - b1`).
- Low pass (2), high pass (3), peak (6) and both shelves (5, 8) do not use
  impulse invariance. They fit the squared magnitude of the analog prototype
  at DC, at Nyquist (`f^4 / ((f^2 - 1)^2 + f^2/Q^2)` for the low pass) and
  at one more point (`sin^2(pi x/2)` for `x` = the prototype's peak, `0.5`, or
  a point scaled by `sqrt(1 - 1/2Q^2)`), as three sums `P` (poles), `Z`
  (zeros) and two products, and factor them all the same way:
  `pole = (sqrt P + 1)/2`, `norm = 1 / (pole + sqrt(pole^2 + Dp))`,
  `a1 = norm (1 - sqrt P)`, `a2 = -norm^2 Dp`, `zero = (sqrt Z + 1)/2`,
  `b0 = norm (zero + sqrt(zero^2 + Dz))`, `b1 = norm (1 - sqrt Z)`,
  `b2 = -norm Dz / (zero + sqrt(zero^2 + Dz))` (the low shelf scales the
  b's by `g` too). Low pass: three branches by `1/Q^2` against 2 (clamped
  to 1.99 just under it) and `(1 - 1/2Q^2) f^2` against 1. High pass: three
  branches the same way, the middle one a blend by `min(((f^2 - 1) +
  1/2Q^2) Q, 1)`. Peak: `|g - 1| < 0.001` is a wire; `f >= 1` its own
  branch. Shelves: `|g - 1| < 0.001` is a wire; `Q^2 <= 0.6` takes poles
  by impulse invariance at the prototype's corner `sqrt(den) pi/2` and fits
  the zeros at DC, Nyquist and `min(.., 0.2)`; otherwise the corner `c =
  -(r^2 + 1)/((1/Q^2 - 2) r) - sqrt(..)` (`r = sqrt(max(g, 1/g))`) picks one
  of three branches by where `fp = f' sqrt(r c)` falls (under `c`, under 1,
  past it), the middle one through `pow(fp, log(x)/log(c))`. The high shelf
  is the low one with its ends swapped and its own roundings
  (`f / g^(1/4)` against `g^(1/4) f`).

So the matched filters follow the analog EQ curve up to Nyquist instead of
cramping towards it, with no oversampling and no latency.

Display (`FUN_006efda0`): level and main `%.2f dB`, freq `Round(Hz) Hz`,
width `Round(w01 * 100) %`, type and order by name. Typed-in values
(`FUN_006f16e0`) accept dB (-18..18), Hz or kHz (20..20000) and percent.

## 3. Signal chain (`Eff_Render` @ 0x006f0da0)

```
FUN_006eddc0: report latency (0x1e): 3N/4 when linear phase, else 44 when oversampling by the engine (0x37c), else 0;
              rebuild the linear-phase kernel if a band changed (below)
if monitor == 1: scope(Source)
silent = every |sample| < 1e-6 (FUN_005f1500, 0x006f1614)
if !silent or linphase: tail = SR/5  else tail -= Length
if tail > -Length:
    Dest = Source * main, ramped (FUN_005e4310, host ramp limit host+0x34)
    if !linphase or kernel ready:
        if HQ oversampling (0x360 == 0 && HQ):
            if 0x37c: engine op 9 per frame: (x, 0) -> 2x zero-stuffed; Length *= 2   // contexts 0x20c8/0x20d0
            else:     TPolyphase up (FUN_006ea220): one 2x stage, 44.1k/88.2k
        if smoother idle:
            for band b with sections and enabled[b]: for section s: biquad(section, state, buf, buf, Length)   // FUN_005ed000
            tail bookkeeping per band (count down, zero states at 0)
        else: FUN_006f0790: per frame: smoother tick, redesign dirty bands, run every section for one frame
        if 0x37c: engine op 0xc per 2 frames (FIR decimate) else TPolyphase down (FUN_006ea2c0)
    else: engine op 4 per frame on Dest (FFT convolver, context 0x20c0)
else: copy Source -> Dest if needed; tick smoother; once (FUN_006edbf0): clear the states of the bands
      still counting down (tail > 0; a band at -1 keeps its states), clear both TPolyphase halves,
      flag the main gain to jump to its target on waking, engine op 0xd (linear phase only); tail = asleep
if monitor == 2: scope(Dest)
```

The biquad (`FUN_005ed000`, chosen at startup by `FUN_005ef7d0`; the other
entry `FUN_005ecf20` is a "Not implemented in i64" stub) is direct form I in
double precision, stereo interleaved, with a denormal flush of the eight
state doubles after each block:

```
y = b0 x + b1 x1 + b2 x2 - a1 y1 - a2 y2       per channel, x and y stored as doubles, output cast to float
```

Sections of one band run back to back on the same buffer, then the next band:
seven bands x up to four sections, all in series, no parallel paths. The main
gain sits before the filters.

### Built-in 2x oversampler (`TPolyphase`, `FUN_006e9cf0` up, `FUN_006e9ec0` down)

Two-path polyphase half-band IIR: six first-order allpass stages (three per
path), coefficients from the 13 doubles at `0x0074e290` (copied pairwise into
`DAT_007615d8` by `FUN_006ea350`; the last pair is the output gain 0.5). Each
stage is `y = y1 + (x - x1) * c` on L and R together (SSE2 doubles). Up:
one input frame produces two outputs, path A then path B. Down: path A takes
the even sample, path B the odd, output `(A + B) * 0.5`. State 0xc0 bytes per
stage set. This path reports no latency: `FUN_006eddc0` (`0x006ee493`)
reports 0x2c only when `0x37c` is set too.

### Engine resampler (`mydispatcher` @ `0x1801e1e50`, ops 7/9/10/11/12)

Read from the disassembly and checked by running the DLL's code under an
emulator (unicorn, outside the repo).

- Up, op 9 (`0x1801e2080`), per frame and side: `{x, _} -> {x + x, 0}` in
  floats; `x` is also kept in the 4-byte context op 7 made, and nothing
  reads it. There is no filter on the way up: the bands see the
  zero-stuffed signal with its image, and the low pass on the way down
  takes the image out. `Eff_Render` (`0x006f11a7`) lays the wide buffer out
  as `2L, 2R, 0, 0` per frame.
- Op 10 (`0x1801e20a0`) builds the low pass: `h[i] = float(0.5 sin(x) / x)`
  with `x = (i - 89) pi/2` (`0.5` at `x = 0`), `i = 0..175`, then
  `h[i] = float(h[i] * sin(i pi/176))` by the vector sine at `0x1801eb080`
  over pairs. Its small-angle branch and its odd symmetry show a sine, not a
  raised cosine. The context holds a 256-float ring (`+0`), the write index
  (`+0x20`, from 0), the tap offset (`+0x24`, 0) and the tap count (`+0x28`,
  176). The taps an even way from 89 are not 0 but what is left of
  `sin(k pi)`, about 1e-17.
- Down, op 0xc (`0x1801e224c`), per frame and side with `{even, odd}`:
  `ring[at] = even; at = (at - 1) & 255; ring[at] = odd;` then
  `y = sum_j h[j] ring[(at + j) & 255]` in two runs either side of the wrap,
  each by `FUN_1801e0fe0`: four float lanes over the whole groups of four,
  added to the running sum lane by lane, then the rest one at a time; then
  `at = (at - 1) & 255`. So `y[n] = sum_k 2 x[k] h[2n + 1 - 2k]`, and only
  `h[89]` is on the odd taps: a click comes out as `2 h[89] = 0.99996` 44
  frames later. That is the 44 (`0x2c`) the plugin reports. The rings are
  never cleared while the plugin runs, not even when it sleeps.
- The taps sum to 0.99984; the pass band holds to 18 kHz at 44.1 kHz
  within 0.01 dB.

### Linear phase (`FUN_006eddc0`, engine ops 0, 2, 3, 4, 5, 6, 0xd, 0xe)

```
N = 512 << step;  if SR >= 44100: N <<= floor(log2(SR / 22050) + eps), max 65536;  else N /= 2     // FUN_006edd00
for k in 0 .. N/2:  f = max(0.1, k * SR / N) Hz (snapped to a nearby resonant band centre when that band is narrow, flag section+0x1371)
    |H|^2 = product over active bands and sections of |B(e^jw)|^2 / |A(e^jw)|^2 using the b/a doubles
    kernel[k] = 10 ^ (clamp(10 log10 |H|^2, -1000, 1000) / 20)          // via a 2^x polynomial, 0x006ee5a0..b8
kernel[N/2] = kernel[N/2 - 1]
engine: op 5 = SR, op 6 = window[0x374], op 2 = N, op 3 = kernel          // under the host lock
```

The engine (`FUN_1801c8a70`) runs two overlapped FFT convolvers and
crossfades between them over `N * window` samples when a new kernel arrives.
So linear phase is "the minimum-phase IIR design, made zero-phase": the
magnitude is identical to the IIR path, the latency is 3N/4 samples (1536 at
44.1 kHz with the default step 2).

### Parameter smoothing (`TParameterSmoother`, `FUN_006f21a0`)

Off unless `0x380 > 0`. Then each changed level/freq/width parameter is a
(index, current, target) triple approached per sample with
`cur += (target - cur) * c`, `c = 1 - (1 - 0.999)^(1 / (t * SR))` for t = 5
or 10 ms (`FUN_006f1f30` with `0x006f2088`), the band is redesigned every
sample while any triple is live, and the triple is dropped once it is within
`0x0074e600` of its target. Linear phase disables the smoother.

Re-implementation note: the default (version 8, HQ 3) path needs the
engine's matched designer (section 2); legacy HQ (flags 1) is the RBJ
formulas above at twice the rate between the halves of the polyphase
half-band or the engine resampler, and HQ off is the RBJ cascade alone. Keep the
double-precision DF1 and the per-section gain split, and apply the main gain
first.

## 4. What stays unknown

- The 13 polyphase coefficients at `0x0074e290` were not transcribed; read
  them with `pe.py g 0x74e290 13`.
- `0x364`, `0x2078`, `0x20e8..0x20ea`, `0x212c`, `0x20ec`: saved GUI state,
  values constant in the corpus.
- What distinguishes state version 7 from 8 (same bytes).
- The ramped copy `FUN_005e4310` and its AVX twin were not read.
- The scope (`FUN_006f0a30`), analyzer (`FUN_006ed860`, engine ops 0x100..
  0x108) and `Dispatcher` 0x33/0x34 (typed values) are GUI.

## Implementation notes (crates/plugins/src/parametric_eq_2.rs)

- Sizes accepted: 305, 309, 326, 330, 350, 354 (versions 2 to 8); the corpus holds 1,138 states, not 1,117.
- The steep peak width table and the polyphase coefficients are not transcribed; steep peak sections clamp at 0.8π. The allpass is `y = x1 + (x − y1)·c`.
- The engine resampler is in `crates/plugins/src/parametric_eq_2_engine.rs`. Rust's `sin` gives the DLL's taps to the bit, and the dot product keeps the DLL's four lanes. The plugin reports 44 frames of latency with it.
- A band counts its 0 dB tail down once a block, after the whole block, as FL does. Going to sleep empties only the bands still counting down and the built-in polyphase, and the main level jumps to its target on waking.
- The matched designer is `matched` in `crates/plugins/src/parametric_eq_2_engine.rs`, in the DLL's order of operations. Its tests hold a table of 32 calls to the DLL's `dispatch` (every branch of every kind), run under an emulator.
- `Delphi`'s `sinh` (`FUN_0045d8d0`), used for the octave Q, is taken as Rust's `sinh`; they may be an ulp apart.
- Approximated in the corpus: linear phase (8 states, run minimum phase). 1,130 slots are exact.
