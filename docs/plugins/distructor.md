# Distructor: state, modules, signal chain

Recovered from `Distructor_x64.dll` (the rack, Delphi, image base
0x400000) and `module/Module_x64.dll` (the module, Delphi, image base
0x400000), FL Studio 2026. Ghidra project `/root/ghidra-projects/distructor`
(`mod` the module, analysed; the rack read from its listing with capstone).
Corpus: 4 rack states in 3 projects (DEDZII - NIGHT TERROR 496 bytes, Edlan
& Ella Noël - Song For You and Sid De La Cruz - Secret Agent Rendezvous x2,
958 bytes), 2 more inside the Patchers of Astes - Bien Duro, and 56 shipped
presets; every one parses to its last byte.

Confidence tags: `asm` (read from the listing), `dec` (decompiled), `corpus`,
`inferred`.

## 1. The rack (`Distructor_x64.dll`)

`CreatePlugInstance` @ `0x006a6ee0`, `TTestPlug` VMT `0x006a6d90` (instance
0x4e0): Dispatcher `0x006a70d0`, SaveRestoreState `0x006a7330`, GetName
`0x006a87c0`, ProcessEvent `0x006a8200`, ProcessParam `0x006a82b0`, Eff_Render
`0x006a8a10`. Each module is an instance of `Module_x64.dll`'s plugin, at
`this+0x128 + 8 i` (count at `+0x148`).

State (`0x006a7330`, `asm`):

| field | type | from | meaning |
|---|---|---|---|
| version | i32 | | 5; a newer one is left alone |
| count | i32 | | modules, 0..4 |
| modules | | | each module's own state (section 2), in rack order |
| soft clip | u8 | | `+0x4d4`, "SOFT CLIP OUTPUT" |
| panels | i32 + i32[n] | v4 | the editor panel of each module (`+0x154 + 12 i`, `FUN_006a8630`); no sound |
| | i32 | v2 | 0 |
| meters | u8 | v5 | "SHOW PEAKMETERS" |

The presets of 942 bytes have 0 panels. Restore makes a module for each
index under `count` and removes those over it (`FUN_006a8360`).

Eff_Render (`0x006a8a10`, `asm`): each module's Eff_Render in turn, in place,
in the order of `+0x128` (the state's order); the peak meters only read.
With soft clip on, the whole interleaved block goes through the pointer at
`0x0070d5f0` (`0x005df670`, SSE `0x005e11b0`) with the knee at `+0x180`, set
by `FUN_005df630(0.78125, 1.0)`:

```
T = 0.78125;  A = 1 - T (2^-16 if <= 0);  B = 1 / A
|x| > T:  y = sign(x) * (T + A * (1 - exp(-(|x| - T) * B)))      // x87
```

That is Fruity Soft Clipper's knee at its default threshold 100/128.

The rack's parameters are its modules': index `49 i + j` is module `i`'s
parameter `j` (`0x006a757e`).

## 2. A module (`Module_x64.dll`)

`CreatePlugInstance` @ `0x0074c940`, `TTestPlug` VMT `0x0074c828` (instance
0x2d0), constructor `FUN_0074eeb0`: Dispatcher `0x0074ca80`, SaveRestoreState
`0x0074ccd0`, GetName `0x00750390`, ProcessParam `0x0074f340`, Eff_Render
`0x007504b0`. It owns four section objects (`TFXProcessor` descendants),
whose slot 0 takes an int parameter, slot 1 a float one and slot 4
processes:

| field | class | VMT | process |
|---|---|---|---|
| `+0x1dc` | `TDistortionProcessor` | `0x0071df38` | `0x00720150` |
| `+0x1e4` | `TFilterProcessor` | `0x00716288` | `0x0071cf80` |
| `+0x1ec` | speaker (impulse) | `0x00733ef8` | `0x00734260` |
| `+0x1f8` | chorus (Delay 3 engine) | `0x0074aab0` | `0x0074b270` |

`+0x144` points at the one the kind selects (`FUN_0074ddd0`, `FUN_0074e270`,
`FUN_0074d4b0`, `FUN_0074d8f0`).

### State (`0x0074ccd0`, `asm`, 231 bytes at version 4)

| bytes | type | field | meaning |
|---|---|---|---|
| 0 | i32 | version | 4; over 4 nothing more is read |
| 4 | i32 | `+0x120` | kind: 0 distortion, 1 filter, 2 chorus, 3 speaker |
| 8 | i32 | `+0x124` | distortion algorithm 0..17 (`FUN_0074ddd0`) |
| 12 | i32 | `+0x128` | oversampling stages 0..4 (`FUN_0074e630`) |
| 16 | u8 | `+0x12c` | remove DC offset (distortion) |
| 17 | u8 | `+0x14c` | bypass |
| 18 | i32 | filter `+0x0c` | filter type 0..21 (`FUN_0071b070` / `FUN_0074e420`) |
| 22 | u8 | filter `+0x80` | smooth up modulation (`FUN_0071b690`) |
| 23 | i32 | `+0x1f4` | speaker impulse 0..58 (`FUN_0074e4f0`) |
| 27 | i32[49] | `+0x200` | the parameters |
| 223 | i32 | | 0 (v2 on) |
| 227 | i32 | chorus `+0xc0` | chorus mode 0 Wide, 1 Mono, 2 Stereo (v4 on) |

Then ProcessParam (`0x0074f340`) runs for every parameter, which hands 0..35
to the distortion (`FUN_00720860`), 39..40 to the filter (slot 1, as 0..1),
41..42 to the speaker and 43..48 to the chorus (`FUN_0074f540`). A new module
(`FUN_0074eeb0`): kind 0, algorithm 0, 1 stage, DC removal on, impulse 1,
filter type 2, the parameters at `0x007a7c48`.

| p | wheel | default | p | wheel | default |
|---|---|---|---|---|---|
| 0..5 | Blood overdrive: pre band, color, pre amp, post gain, post filter, x100 | 0, 5000, 0, 10000, 0, 0 | 23..27 | Harmor: drive, asymmetry, wet vol, dry mix, filter cut | 500, 0, 1000, 0, 950 |
| 6..9 | wave folder: gain, warp, bias, threshold | 500, 5000, 5000, 10000 | 28..33 | Abrasive: pre gain, gate thresh, noise mix, post gain, noise BP, BP on | 2000, 5000, 2000, 5000, 8400, 0 |
| 10..15 | Aperture: gain, warp, bias, threshold, offset, mode | 500, 5000, 5000, 10000, 2000, 0 | 34, 35 | Crusher: rate crush, bit crush | 0, 0 |
| 16..20 | Fast dist: pre, threshold, post, mix, type | 128, 10, 128, 128, 0 | 36..38 | input level, dry/wet, output level | 5000, 10000, 5000 |
| 21, 22 | Soft clipper: threshold, post gain | 100, 128 | 39, 40 | filter cutoff, resonance | 10000, 0 |
| | | | 41, 42 | stereo separation in, out | 0, 0 |
| | | | 43..48 | chorus delay, feedback, speed, depth, blur, color | 108, 2880, 1500, 1995, 0, 0 |

### Render (`0x007504b0`, `dec`)

```
bypass: out = in
mix_in = p36 / 5000;  dw = p37 / 10000;  mix_out = p38 / 5000
kind 0 or 1 and stages > 0: x = up(in), 2^stages, the filter 4x at most (FUN_00714ba0)
x    = x * mix_in;    keep = x
wet  = section(x)                                      // +0x144 slot 4
y    = (0 + keep * (1 - dw) + wet * dw) * mix_out      // ippsZero, AddScaled x2, MulC
out  = down(y)                                         // FUN_00714c40
kind 0 and DC removal: v += (out - lp) a;  lp += v;  v *= 0.96;  out -= lp   // ptr 0x007af8e0, 0x005fbdf0, x87
       a = 2.2835e-5 * 44100 / SR  (Dispatcher 4)
```

The oversampler (`TPolyphase...`, `FUN_007149b0`, stages of 0xc0 bytes,
kernels at `0x007c05f8`/`0x007c0600`, flushed with 12 after each stage) is
the half band of `dsp::halfband`: the module DLL has the same interleaved
coefficients at file offset 0x3a6218. Stage `n - 1` runs first, up and down.

### Distortion (`TDistortionProcessor`, setter `FUN_00720860`, `dec`)

The selected algorithm's method pointer is at `+0x128 + 16 a`
(`FUN_00720090`); selecting one sends every parameter again
(`FUN_007201b0`). Its smoothers (`FUN_00713fa0`) are `cur = t + (cur - t) k`
a sample with `k = exp(ln 0.01 / (0.015 · SR · 2^stages))`, snapped when
within 2^-24 at a block's start (`FUN_00713f30`), and start at 0.

| a | name | function | what it is |
|---|---|---|---|
| 0 | Blood overdrive | `0x0071efe0` | Fruity Blood Overdrive's chain (`docs/plugins/fruity-blood-overdrive.md`), its four smoothers starting at 1, 1, 1, 0 and taking `target · 0.00025` precomputed (`FUN_007203a0`); the shaper's index is cut towards 0 (`FUN_007201f0`, `cvttsd2si`) |
| 1 | Fast dist | `0x0071f590` | FL's `DistWave` (`0x005fdb90`) on Fast Dist's tables: pre p16, threshold p17, type p20, post p18 twice, mix p19, as Fruity Fast Dist |
| 2 | Soft clipper | `0x0071fcd0` | the knee at `p21 / 128` (`0x005fdb10`), then the knob gain of p22 unless it is 1 |
| 3..13 | Harmor Classic, Hill, Flathat, Cube, Log, Softsat, Rubber, Ribbon, Sincrush, Bitcrush, Bitjam | `0x0071f650` | Harmor's distortion modes 1..11 on IPP (`ippsTanh_32f_A24` and kin, names from the loader at `0x005aa87c`): dry = x · mix, x += bias, x *= drive, shape, dry += shape · wet, then two RBJ low passes at Q 1/sqrt 2 (`FUN_005f86c0` type 1, `0x005f8ee0`) unless p27 is 1100, at `16.3516 · 2^(p27/100)` Hz under `0.4999 SR · 2^stages` (less 45 Hz at 1x) |
| 14 | Wave folder | `0x0071fd40` | bias and warp, then `0x005fde90`: `g = p6/500 + 1`, `T = 0.9 p9/10000 + 0.1`, a triangle of period `4T` |
| 15 | Aperture | `0x0071ee70` | bias (p12) and warp (p11, `x += sign(x) w`), then `0x005fe170`: `g = p10/500 + 1`, `T = 0.5 p13/10000 + 0.5`, `o = 2 p14/10000 - 1`; p15 = 0: past T, `x - trunc(x/T) T - (T + o)`; else up to 8 times `sign(x) (|x| - (T + o))` while past T, then held at ±T |
| 16 | Abrasive | `0x0071ebf0` | `d = x (p28/5000 + 1)`; with noise (p30 > 0) `d = d (1 - m) + d m sign(x) [|x| > p29/10000] n`, `m = p30/10000`, `n` one MT19937 draw a frame (seed 0x12bd6aa, `FUN_00714200`, `u · 2^-31 - 1`), band passed when p33 = 1 by FL's TPT SVF at `20 · 2^(p32/1000)` Hz, `k = 0.505` (`FUN_00716690` type 2); `d *= p31/5000 + 1`, clipped at 1 |
| 17 | Crusher | `0x0071ef50` | a sample and hold of `(0.0002 f^3 - 0.0171 f^2 + 0.7435 f + 1) · SR/44100` samples, `f = 127 (p34/10000)^3` (`FUN_007218f0`), then `trunc(x L) / L`, `L = 2^(14 (1 - b^(1/4)) + 2)`, `b = (p35/10000)^2` (`FUN_00721c00`) |

Harmor's setter (p23..p26, modes as Harmor numbers them, `w = level3(p25) ·
(1 - |p26/1000|)`, `k1 = 2^(p23/100 - 4)`): drive and wet are Harmor's
(`docs/plugins/harmor.md` section 4) mode for mode. The bias is `0.75 b^2`
(Ribbon `b^2 · drive`, Bitcrush `b / drive`) with `b = p24/500`, the right
side negated when `trunc(b) < 1`; Sincrush puts `2^(p24/250)` in the bias
ramp as its `g`, Bitjam takes `q = 1024 · 2^(p24/50)`.

### Filter (`TFilterProcessor`, process `0x0071cf80`, `dec`)

22 types: LP6, HP6, LP12, HP12, BP12, LP24, HP24, Notch, Low pass 12 Alt,
Band pass 12 Alt, High pass 12 Alt, Lowpass 24 Alt, Peak, Comb +, Comb -,
Vowel, Phaser 1..3, Low shelf, High shelf, Allpass. The cutoff wheel is
`v = p39/10000`, the resonance `p40/10000`, at the module's rate.

- 0, 1 (`FUN_0071bcb0`): one pole by the bilinear transform at `20 · 2^(10 v)`
  Hz, `g = tan(f · pi / rate)`; low `b0 = b1 = g/(g+1)`, high `b0 = -b1 =
  1/(g+1)`, `a = (1 - g)/(g+1)` (`FUN_007183c0`, `FUN_00718300`).
- 13, 14 (`FUN_0071cd00`, `FUN_00718a30`): a comb on a ring of 16384 a side,
  `d = v · rate · 0.05 + 1` samples back, read between two (`FUN_00718710`,
  the index cut towards 0), times `0.95 r`. Comb - adds the read and writes
  the sum (feedback); Comb + writes the input first (feed forward).
- The others: not read.

"Smooth up modulation" glides a moved cutoff; a state sets it at once.

### Chorus (`0x0074b270`, setup `FUN_0074b000`, `dec`)

The module loads `..\..\Fruity Delay 3\engine_x64.dll` and drives its engine
(`docs/plugins/fruity-delay-3.md`) through `setParameter`: the 26 values at
`0x007adba0`, then the mode (`FUN_0074b5a0`: Wide model 0.33 and Time R
0.84, Mono 0 and 0.5, Stereo 0.66 and 0.5) and the wheels (`FUN_0074b3f0`):
time `p43/768`, feedback `p44/6000`, mod rate `p45/6000`, mod depth
`p46/6000`, diffusion and its spread `p47/6000`, tone `(p48 + 3000)/6000`.
Dry 0, wet 1, sync off, keep pitch on.

### Speaker (`0x00734260`, load `FUN_00734500`, `dec`)

The impulse is the table at `0x007ae838` (59 rows of 0x1c bytes: name, file
under `module/impulses`, gain as a double); rows 0, 20, 27 and 38 are
headings. Corpus: 1 "Blues" (gain 0.026), 4 "Muffled" (0.058). The wet sound
is

```
x  -> 2x2 separation in  (p41/1000, volume 1)            FUN_005f1f50, gliding (FUN_005fed40)
   -> TConvolver (FUN_00727e70: a head of 128 taps, FFT blocks after)
   -> 2x2 separation out (p42/1000, volume 44100/SR)
   -> * gain
```

The separation at pan 0: `f = (5^s - 1)/4` for `s > 0`, `s` under 0, at most
1; direct `1 - f/2`, cross `f/2`, times the volume. A gain glides to a new
target by at most `0.001 · 44100/SR` a sample, from 0 when the module is
made. After as many silent samples (|x| <= 1e-6) as the impulse is long the
section passes its input and skips the convolution.

## 3. Implementation notes (crates/plugins/src/distructor.rs)

- Built: the rack, the soft clipper, every module field, the oversampling,
  the DC remover, all 18 distortion algorithms, the filter's types 0, 1, 13
  and 14, the chorus (the crate's `Delay3` on the engine's values through
  `Delay3::set_engine`), the speaker (`convolver::Engine` on the cabinet's
  file through `convolver::decode_impulse`). Corpus: distortion algorithms
  0, 1, 2, 5, 7, 15, 16, 17; filter types 0 and 14; one chorus; speakers 1
  and 4.
- Approximated: IPP's A24 functions and Delphi's `Exp`, `Ln`, `Power`, `Tan`
  are Rust's f64 ones rounded to f32; the x87 soft clipper and DC remover
  run in f64; the biquads skip FL's flush of tiny states. Filter types 2..12
  and 15..21 pass their input and say so in `fidelity()`; none plays in the
  corpus. The impulse is used at its file's rate (FL's `TSample` resampling
  was not read). A missing cabinet file passes and says so.
- The 15 ms smoother base `ln 0.01` is taken from Harmor's map: in this DLL
  it is a runtime value whose writer was not found.
- The parameter ranges for `set_param` are inferred from the defaults and
  the setters; a state's values are taken as they are, as FL takes them.
