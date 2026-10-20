# Low Lifter: state, parameters, signal chain

Recovered from `Low Lifter_x64.dll` (FL Studio 2026, Delphi shell, image
base 0x400000) and `Shared/ildsp_x64.dll` (MSVC C++, image base
0x180000000). The shell owns the state, the GUI and the two spectrum
monitors; the library does the audio in `LowFreqEnhancer_CreateInstance`,
a Faust class `ILRDLowFreqEnhancer` compiled from
`lowFreqEnhancementChebyshev.dsp` with `-single -ftz 0 -mcd 16 -mdd 1024
-mdy 33 -es 1` (its `metadata` @ `0x180176ea0`). Corpus: 3 uses in 2
projects (Astes - Bien Duro twice, Edlan & Ella Noël - Song For You) and
the render test `Low Lifter.flp`, all 82 bytes of version 6; 7 presets of
its own and 4 copies in the databases, the same format.

Confidence: every line of `compute` was transcribed and then checked by
running the library's own code under unicorn (below): the Rust port puts
out the same bits as the binary, sample for sample, over every state that
was tried.

## How the plugin is wired

Shell: `CreatePlugInstance` @ `0x006c5930`, class reference `0x006c5720`
-> VMT `0x006c57e8` (`TTestPlug`, instance 0x1e0), constructor
`FUN_006c59a0` (it makes the processor with `FUN_006c4100`, reads the
processor's defaults back through the curves' inverses `FUN_006c5520` and
sets the editor fields; it also loads Frequency Splitter's
`engine_x64.dll` for the spectrum view).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006c64d0 |
| 3 | SaveRestoreState | 0x006c6d50 |
| 5 | ProcessEvent | 0x006c2460 |
| 6 | ProcessParam | 0x006c6f90 |
| 7 | Eff_Render | 0x006c7a80 |

Shell object: `0x124` the rate (the render's wake count), `0x128` the
render's quiet count, `0x12c` the processor (an interface from the
library), `0x134` i32[15] the parameters, `0x184` / `0x18c` the monitors'
capture buffers, `0x194..0x1a8` the editor's spectrum settings.

`LowFreqEnhancer_CreateInstance` (`0x1801ab3d0`) returns a 16-byte
`LowFreqEnhancerImpl` (vtable `0x1803dfff8`) holding the Faust object
(0x346b8 bytes, vtable `0x1803de0a0`, constructor `FUN_18017bbd0`):

| Impl slot | address | does |
|---|---|---|
| +0x08 | 0x1801ab4d0 | set rate: Faust `instanceConstants(int(rate))`, then `instanceClear` |
| +0x10 | 0x1801ab500 -> `FUN_18017bdd0` | set a zone by index (table below); the setter stores the float as given |
| +0x18 | 0x1801ab510 -> `FUN_18017c5f0` | read a zone back |
| +0x20 | 0x1801ab520 -> `FUN_18017ce10` | process: deinterleave at most 0x4000 frames, `compute`, interleave |
| +0x28 | 0x1801ab530 | flush: `instanceClear` |

Faust methods: `buildUserInterface` `0x180177a80`, `instanceConstants`
`0x180177050`, `instanceResetUserInterface` `0x1801772d0`, `instanceClear`
`0x180177320`, `compute` `0x180177f50`.

## 1. State layout (event 213, 82 bytes)

`SaveRestoreState` (`FUN_006c6d50`) writes `i32 6`, `0x134` (60 bytes, the
15 parameters), `u8 0x194`, `u8 0x1a8`, `i32 0x19c`, `i32 0x1a0`,
`i32 0x1a4`, `i32 0x198`. On load it reads the first word and reads on
only when it is 6 (`5 < v < 7`); otherwise nothing changes. After a load it
sends every parameter through `ProcessParam` with `REC_UpdateValue`, so
each goes to the processor as below. The 18 editor bytes are the spectrum
monitor's histogram switch, range, pivot slope and precision; nothing in
the audio reads them. A new plugin saves `01 00 03000000 03000000
02000000 02000000`.

## 2. Parameters

The DFM (`SynthEditorForm`) has every wheel 0..10000 and the switches as
checkboxes. `ProcessParam` (`FUN_006c6f90`) stores the raw value and hands
`FUN_006c71d0` the wheel over its range, `n = (raw - min) * scale`
clamped to 0..1 and passed as an f32. `FUN_006c71d0` then calls the
processor's setter with:

| # | control | to the zone | zone (offset) |
|---|---|---|---|
| 0 | Enabled | `n` | On/Off (0x1c) |
| 1 | Dry/Wet | `n` | Dry/Wet (0x20) |
| 2 | Low band gain | curve(-120, 12, 7.275) dB | Low Gain (0x2104) |
| 3 | Harmonics decay slope | curve(-60, 0, 3.105) dB | Slope (0x41ec) |
| 4 | Lookahead | raw | Lookahead (0x20e8); also reports the latency |
| 5 | Invert polarity | raw | Invert Polarity (0x20ec) |
| 6 | Pre-shaping boost | curve(0, 18, 1) dB | Pre Gain (0x2168) |
| 7 | Diffuser mix | `n` | Diffusion Mix (0x2110) |
| 8 | Diffuser coefficient | curve(-0.99, 0.99, 1) | Diffusion Depth (0x211c) |
| 9 | High band gain | curve(-120, 5, 5) dB | High Gain (0x48) |
| 10 | Crossover cutoff | curve(20, 120, 0.498918) Hz | Crossover Cutoff (0x60) |
| 11 | Low cut | curve(20, 120, 0.498918) Hz | Low Cut (0x30) |
| 12 | Multiband | raw | Multiband (0x54) |
| 13 | High band | raw | High Switch (0x3c) |
| 14 | Low band | raw | Low Switch (0x20f8) |

`curve(a, b, c)` is `FUN_006c54c0`: `a + (b - a) * Exp(Ln(n) / c)` in
f64 with the Delphi runtime's `Ln` (`0x00409870`, kernel `FUN_00409780`,
coefficients at `0x006e2a30`) and `Exp` (`0x00408810`, table of
`e^(j/64) - 1` at `0x006e24e8`, polynomial at `0x006e2910`); the constants
are the doubles at `0x006c7418..0x006c7480`. The wheel at 0 gives `Ln`
minus infinity (FL runs with the error masked) and so `a`. The port
transcribes `Ln` and `Exp` too: an f64 `powf` misses FL's f32 in 35 of
the 60006 wheel positions of the six curves, the transcription in none
(checked against `FUN_006c54c0` run under unicorn). `scale` is taken as
`1 / 10000`, the DFM's range.

The render-test project saves `[1, 10000, 4999, 7210, 1, 0, 0, 0, 5000,
10000, 7750, 0, 0, 1, 1]`: on, all wet, 0 dB low gain, -6 dB slope,
lookahead, no pre gain, a dry diffuser of coefficient 0, +5 dB high band,
80 Hz crossover, 20 Hz low cut, multiband off, both bands on. That is the
port's default.

Parameter 4 also calls `FUN_006c7e00`, which tells FL (host call 0x1e)
the plugin is `Round(rate * 0.01)` frames late with the lookahead on and 0
off.

## 3. Render

`Eff_Render` (`FUN_006c7a80`): feed the input monitor when the editor is
open; then `FUN_005dfd30(buf, 2 * len, 1e-6)` asks whether every sample
of the block is at or under 1e-6 (compared as `|x|` bits, so a NaN is
loud). A loud block sets the count at `0x128` to the rate (`0x124`, set
with the rate, `FPD_SetSampleRate`); a quiet one takes the block's length
off it. When the count comes to `-len` or under, the block is copied
through untouched, the processor is not run, and the count is parked at
`0xc0000001` until a loud block. So after a second of quiet input the
plugin passes its input and its state freezes; a new plugin, with the
count at 0, sleeps until it first hears something. Otherwise the block
goes to the processor's process slot.

## 4. Signal chain (`compute` @ `0x180177f50`)

Per call it reads the zones once:

- `dry_t = on ? 1 - drywet : 1`, `high_t = powf(10, highdB / 20)`,
  `low_t`, `pre_t` likewise, `polarity_t = invert ? -1 : 1`;
- `ahead = lookahead ? 0.01 : 0`, `lag = int(ahead * fs)` frames;
- `hold = rintf((ahead + 0.05) * fs / 4)` (`FUN_1801b6dec` is `rintf`);
- `rise = expf(-(14.444787 / fs) / ahead)` (0 with no lookahead).

`instanceConstants` (rate clamped to 1..192000, `over = 1 / fs`):
`g = tanf(20 pi over)`, smoother `take = g / (1 + g)` and twice it; the
20 Hz highpass after each band `g + sqrt 2`, `1 / den`, `g / den`, twice
that and `2 g`; `turn = pi over` and its quarter and half; `fall =
expf(-144.44786 over)`; the slope's smoother at `tanf(5 pi over)`; the
fade-in's at `tanf(pi over)`; delays `fs * {0.5, 0.20710678, 0.15891862,
0.1339746}`.

Per sample, with every control through `glide(state, target)`: `out = take
(target - s) + s`, `s += 2 take (target - s)`:

```text
dry = glide(dry_t); wet = 1 - dry
cut = tanf(glide(lowcut) * turn)                 low cut tangent
hi = glide(high_t) * glide(high_switch)
mb = glide(multiband); nmb = 1 - mb
c = max(glide(cutoff), 20)
g, g4, g2 = tanf(c turn), tanf(c turn/4), tanf(c turn/2)   with gk = g + sqrt2, d = 1/(g gk + 1)
gain = glide(low_t) * (glide(low_switch) * glide(polarity_t))
mix = glide(diffusion mix)
sn = clamp(glide(depth), -0.99, 0.99); cs = sqrt(1 - sn^2)
pre = glide(pre_t)
fade = glide1Hz(1)                                0 after a clear, to 1
s = powf(10, slope/20), s2 = powf(10, slope/10), norm = 1/(2s + 1)   slope at 5 Hz
back[k] = int(clamp(fs K[k] / c - 1, 0, 48000))
```

Each side then (state-variable filters are Zavalishin's, `v = (x - (a gk +
b)) g d`, `band = a + v`, `low = b + g band`, `a += 2v`, `b += 2 g band`;
`ap(x) = x - 2 sqrt2 band`):

```text
band1, low1 = svf(x, g);  low2 = svf(low1, g)          LR4 low of x
band3, low3 = svf(x, g2); ap = x - 2√2 band3;  low4 = svf(low3, g2)
m  = ap - (2√2 band5 + low4), band5 from svf on ap with low4 inside the pre
band6, low6 = svf(m, g);  low7 = svf(low6, g)
high line[n] = ((m - 2√2 band6) - low7) mb + ((x - 2√2 band1) - low2) nmb
band8 = svf(low4, g);  u = low4 - 2√2 band8
band9, low9 = svf(u, g4);  low10 = svf(low9, g4)
in = [pre low10, pre ((u - 2√2 band9) - low10), pre low7, pre low2]
```

Each band (a 2048 line, four peak holders, four followers, a 20 Hz
highpass):

```text
line[n] = in;  y = line[n - lag]
p = |in|; 4x: if count < hold and p < held { count++; p = held } else count = 0; held = p
p = max(p, 1)
4x: pole = p <= f ? fall : rise;  f = p = (1 - pole) p + pole f
u = y^2 / p^2;  t2 = 2u - 1
w = (((2 t2 - 1) s2 + 1) y / p + t2 s) norm        = (x + s T2(x) + s^2 T3(x)) / (1 + 2s), x = y/p
v = w - (dc.a (g20 + √2) + dc.b);  dc.a += v * 2g/den;  dc.b += (v g/den + dc.a) 2g
band out = v / den
```

Then, with `k = 1 / den` of the highpass:

```text
sum  = (((v2 k + v1 k) + v3 k) mb + nmb (v4 k)) fade         left
sum  = ((v2 k + v1 k) + v3 k) mb + (v4 k) nmb                 right, fade applied after
4 rotations (lines 8192, 2048, 2048, 2048; z the tap read last sample):
     line[n] = cs i - sn z;  o = sn i + cs z;  z = line[n - back[k]]
mixed = (o4 mix + (1 - mix) sum) gain + hi high[n - lag]
x27 = mixed - (lc.a (cut + √2) + lc.b);  D = 1 / (cut (cut + √2) + 1)
lc.a += 2 cut x27 D;  lc.b += 2 cut (cut x27 D + lc.a)
wet_out = D x27                                               2nd-order highpass at the low cut
```

The dry path takes `-x` through the same allpasses: an SVF at `g` taken
with its step negated, a first-order allpass at the low cut (`v = 2 (x -
s) cut / (cut + 1)`, `s += v`, `ap = 2s + v - x`), the allpass at `g4`,
the one at `g2`, mixed `(ap_h) mb + ap nmb`, into a 2048 line;
`out = dry line[n - lag] + wet_out wet`.

The right side's dry path divides by `den` where the left multiplies by
`1 / den`, and a few products group differently (the first rotation's
input is `((sum fade) cs)` on the right, `cs (sum fade)` on the left; its
output `(sn fade) sum` and `sn (sum fade)`); the compiler made the two
sides so, and the port keeps each. Ghidra prints some of these products in
an order the listing does not have (`lg * lsw * pol` is `lg * (lsw *
pol)` in the listing); the port follows the listing.

Then the count goes up and every "next" state slot is copied to its
"current" one.

## 5. The runtime's maths

`compute` and `instanceConstants` call the MSVC runtime linked into
`ildsp_x64.dll`: `tanf` `0x1801c9ef0`, `expf` `0x1801c5f40`, `powf`
`0x1801c83b0` (always `powf(10, y)` here). Each works in f64 and rounds
once, and each has an SSE2 path and an FMA3 path; the runtime picks FMA3 on
a CPU with AVX and FMA (`FUN_1801c3860` sets the switch at `0x1803f84f4`),
which is what FL runs on any machine of the last decade. The port
transcribes the FMA3 paths for the arguments the plugin gives:

- `expf`: `n = rint(x 64/ln2)`, `r = fma(-n, ln2/64, x)`, `p = fma(r^2,
  fma(1/6, r, 0.5), r)`, `fma(p, T[n & 63], T[n & 63]) * 2^(n >> 6)`, with
  `T = 2^(j/64)` at `0x1803e3720`.
- `powf(10, y)`: the logarithm of 10 is `3 ln2 + ln 1.25` from the table
  at `0x1803e2670` with a zero polynomial; then `t = y ln10`, the same
  reduction, `q = r fma(fma(1/6, r, 0.5), r, 1)`, `fma(q, T, T)`, the
  exponent added to the bits. `y = 0` gives 1 and `y = 1` gives 10.
- `tanf`: under 2^-27 `x`; under 2^-13 `fma(x^3, 1/3, x)`; to pi/4 the
  rational `fma(x^3, (-0.0172 x^2 + 0.3853) / ((0.0184 x^2 - 0.5140) x^2 +
  1.1559), x)` with Horner by fma. The plugin's arguments stay under pi/4
  at any rate of 480 Hz or more; past that the port takes the f64 tangent.

The SSE2 paths (the same constants, plain multiplies and adds) were
transcribed too and matched the emulated runtime on 20000 arguments of
each; the FMA3 and SSE2 results then differed on none of 20000. Rust's
own `f32::tan`/`powf` (the C library's) differ from both on about one
argument in 1500 (`powf(10, -0.5632547)` is 0.27336648 in FL, 0.27336651
in glibc and in a correctly rounded f64 power).

## 6. How it was checked

`tools/plugstate/low_lifter_emu.py` maps `ildsp_x64.dll` in unicorn,
calls `instanceConstants`, `instanceResetUserInterface`, `instanceClear`,
writes the zones, and runs `compute` block by block; `--fma` answers the
three maths calls from its Python transcription of their FMA3 paths, and
`--check` holds its SSE2 transcription against the emulated runtime.
Against it, the port was bit-identical on 96000-frame stereo signals for
the render test's state, both Bien Duro states, the Song For You state,
all seven presets and a few made-up states (lookahead on and off,
polarity inverted, every switch off, every wheel at an end), at 22050,
44100, 48000, 96000 and 192000 Hz, in blocks of 1, 64, 77, 100, 333, 441,
512 and 1024 frames, with the runtime's real SSE2 maths and with the FMA3
transcription. The zones came from the port's `zone`. The
test `it_comes_out_as_ildsp_s_own_code_makes_it_to_the_bit` keeps four of
those runs as hashes of every output bit.

What it does not check: the render's quiet gate and the parameter path
from the shell were read, not run (the curves were run: `FUN_006c54c0` over
every wheel position of the six curves); null tests against FL's own
export are still to come.
