# Vocodex: state, inputs, signal chain

Read from `Plugins/Fruity/Effects/Vocodex/Vocodex_x64.dll` (FL Studio 2026,
Delphi, image base 0x400000), headless Ghidra. Corpus: 12 states in 10
projects, the three copies of the default in the plugin database and 21
presets under `Data/Patches`. Every one parses; the 12 project states write
back byte for byte.

Confidence tags: `loader`, `dfm`, `corpus`, `asm`, `inferred`, `doc` (FL's
online manual, only where it names a control the binary leaves nameless).

## How the plugin is wired

`CreatePlugInstance` @ `0x0084e600` -> constructor `FUN_0084f730`; class
reference `*0x0084e370` -> VMT `0x0084e438` (`TTestPlug`, instance size
0x528, parent `TDelphiFruityPlug`).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x0084e660 |
| 3 | SaveRestoreState | 0x0084efb0 |
| 4 | GetName | 0x00855060 (control hints at editor `+0x880 + 8 i`, `+0xe4`) |
| 6 | ProcessParam | 0x00853e80 |
| 7 | Eff_Render | 0x00856350 |

The editor is `TSynthEditorForm` (DFM at file offset 0x5a71d4); the 20
parameter controls carry their parameter number in `Tag` and their name in
`Hint`. Three things are separate objects the plugin loads: Soundgoodizer
(`SG.dll`, state `Data/SG.dat`, which is Soundgoodizer's preset A), the
carrier synthesizer (`Synthesizer/sytrus.dll`, a Sytrus, presets in
`Synthesizer/Data/Presets/*.fst`) and the "VCX" test speech
(`Data/Vocodex.wv`, a button in the editor; not played here).

The vector maths go through function pointers that `FUN_005bcdc0` fills
from IPP by name (`ippsMul_64f`, `ippsAddProduct_64f`, `ippsConvert_32f64f`,
...) and that `FUN_00629240`, `FUN_0062f250`, `FUN_00606910` and
`FUN_0077b230` fill with the plugin's own SSE/AVX routines.

## Object fields

| offset | type | meaning |
|---|---|---|
| 0x120 | i32[20] | the parameters |
| 0x170 | i32 | hold mode: 0 peak, 1 RMS |
| 0x174 | i32 | order, filter stages a band, 1..4 |
| 0x178 | i32 | unison voices beside the first, 0..4 (`min(control, 5) - 1`) |
| 0x17c | i32 | band count, 5..100 |
| 0x180 | i32 | 0 when DRAFT is on and the host is not rendering in HQ |
| 0x184 | i32 | MOD input: 0 own insert, n sidechain n |
| 0x188 | i32 | CAR input: -1 none, 0 own insert, n sidechain n |
| 0x18c..0x193 | u8[8] | L-R encoding, MIN TIMES, contour to MOD, THREADED, subtractive denoising, contour to NOISE, contour from CAR, contour from CONSONANTS |
| 0x194 | ptr[13] | the envelopes (`TMMap`, VMT 0x77ba50) |
| 0x1fc | u8[13] | per envelope: follows the modulator pitch shift |
| 0x20c | i32 | carrier synthesizer preset, -1 none |
| 0x210 | record[bands] | the bands, 0x564 bytes each: sets at `+0x360 + 0xd8 k` for k = -unison..1, k = 1 the carrier |
| 0x228, 0x230, 0x238 | 0x78-byte biquads | modulator pass-through highpass, carrier pass-through lowpass, noise highpass |
| 0x240, 0x248, 0x250 | objects | contour: peak follower, smoothers, consonant detector |
| 0x334, 0x340 | ptr | the modulator and the carrier for this block |
| 0x43c, 0x444, 0x44c, 0x454 | f32 | wet, modulator pass-through, carrier pass-through and noise gain |
| 0x45c | f32 | band width, from parameter 14 |
| 0x464 | record | the frequency map |
| 0x4c0, 0x4c8, 0x4d0 | f64 | flatness, attack curve, release curve |

## State

`SaveRestoreState` (`FUN_0084efb0`). The version word is written as
`0x00080003`; on load a value of 1 or less, or `0x80004` or more, is
ignored, and one over 0xffff is shifted down 16, so the corpus reads as 8.
`loader`, `corpus`.

| part | bytes | notes |
|---|---|---|
| version | 4 | |
| 13 envelopes | variable | each: i32 kind (3), i32 version (3), i32 count, `count` points of f64 dx, f64 y, f32 tension, 4 shape bytes, then 20 bytes of loop and sustain marks (`FUN_0077bd00`) |
| parameters | 80 | 20 i32, each clamped to its control's range |
| 0x170..0x193 | 36 | 7 i32 and 8 switch bytes (33 bytes for version 4, 32 under it) |
| 0x1fc..0x208 | 13 | pitch-follow flags (version 4 on) |
| synth preset | 1 + n | a length byte and the name, empty for none |
| held keys | 4 + 4 n | i32 count, then the keys |

The 1182-byte default is FL's: no synth, key 60 held, modulator from
sidechain 1, carrier from the own insert. The envelopes, in state order, with
what the band computation reads from each (`FUN_00851dd0`):

| n | envelope | default | use |
|---|---|---|---|
| 0 | band gain multiplier | 0.5 | `t = 2 y`, gain `(3^t - 1) / 2`, 1 exactly at t = 1 |
| 1 | band panning | 0.5 | centred: `2 (y - 0.5)` |
| 2 | band gain offset | 0.5 | centred `v = 2 (y - 0.5)`, offset `sign(v) (1025^|v| - 1) / 1024` |
| 3 | modulator noise level | 0 | threshold `(1025^y - 1) / 1024` over the band gain |
| 4 | modulator pass-through | 0 | per-band gain `(3^(2y) - 1) / 4`, scaled as the band gain |
| 5, 6, 7 | hold, attack, release | 0.5 | scale `2^(6y - 3)`, 1/8 to 8 |
| 8 | band distribution | 0.1429 to 0.9918 | band i at `x = i / (bands - 1)`, Hz through the frequency map |
| 9 | band width | 0.5 | width scale `2^(4y)` |
| 10 | modulator pitch shift | 0.5 | centred, octaves `-2 (y - 0.5)` |
| 11 | saturation mix | 0 | not played here |
| 12 | saturation curve | 0 to 1 | not played here |

Envelopes 1, 2 and 10 read `y - 0.5` (the constructor sets their centre,
`FUN_0077bcb0`, mask `0x0406` at `0x008503e2`). A point's x is its distance
from the point before. Every corpus point has tension 0, which is a straight
line (`FUN_00782b50`).

## Parameters

Ranges from the DFM, defaults from the default preset, curves from
`ProcessParam` (`FUN_00853e80`).

| n | control | range | default | what it comes to |
|---|---|---|---|---|
| 0 | Wet level | 0..2000 | 1000 | `(7^(v/1000) - 1) / 6` |
| 1 | Soundgoodizing level | 0..1000 | 150 | Soundgoodizer parameter 5; off at 0 |
| 2 | Modulator pass-through level | 0..16000 | 12800 | `(11^(v/12800) - 1) / 10`, times 4 with contour to MOD from CAR, 0 if frozen then |
| 3 | Modulator pass-through highpass | 0..65536 | 55000 | two stages at `40 * 375^(v/65536)` Hz; none at 0 |
| 4 | Carrier pass-through level | 0..16000 | 0 | as 2 |
| 5 | Carrier pass-through lowpass | 0..65536 | 28000 | two lowpass stages, none at 65536 |
| 6 | Carrier added noise level | 0..12800 | 0 | uniform noise of `(7^(v/12800) - 1) / 12` amplitude, times 4 with contour to NOISE from CAR |
| 7 | Carrier added noise highpass | 0..65536 | 55000 | as 3 |
| 8 | Hold time | 0..10000 | 1666 | `(501^(v/10000) - 1) * 2` ms, then a quarter of it |
| 9 | Attack time | 0..10000 | 1115 | `(501^(v/10000) - 1) * 2` ms |
| 10 | Attack curve | 0..2000 | 0 | `2^(v/1000)` stages |
| 11 | Release time | 0..10000 | 3500 | `(501^(v/10000) - 1) * 4` ms |
| 12 | Release curve | 0..2000 | 0 | as 10 |
| 13 | Freeze envelope | 0..1 | 0 | holds every band's envelope |
| 14 | Band width | 5..1000 | 349 | `(5^(v/1000) - 1) / 2` |
| 15 | Filter flatness | 0..100 | 100 | `v / 100` |
| 16 | Modulator band width multiplier | 0..4000 | 2000 | `2^(v/1000) / 4` |
| 17 | Modulator pitch shift | -1200..1200 | 0 | cents, the modulator filters at `2^(-v/1200)` |
| 18 | Modulator unison shift | 0..1000 | 250 | octaves `v / 1000` times the unison table |
| 19 | Modulator unison panning | -256..256 | 128 | `v / 128` |

The pass-through and noise filters are RBJ biquads with `alpha = sin(w) k`,
k = 0.35651 and 0.94379 (`*0x008cc3d0`), designed by `FUN_00853de0`
through `FUN_006260c0` (type 1 lowpass, 4 highpass).

## Inputs

`Eff_Render` picks the two signals before anything else. `asm`, `loader`.

- With L-R encoding (`0x18c`) the own input's left side is the modulator on
  both sides and its right side the carrier (`FUN_008554b0`).
- Otherwise MOD 0 is the own input. MOD n asks the host for sidechain buffer
  n (host vtable `+0x120`, as Fruity Limiter does, into the record at
  `0x334`) and falls back to the own input when the buffer is nil
  (`0x856683`). The flags FL writes beside the buffer are not read. FL's
  host hands nil for a number past the inserts that send to the plugin's
  insert (`docs/rec-225.md`), so such a number is the own input in FL too.
- CAR -1 is a zeroed buffer, CAR 0 the own input, CAR n sidechain n with the
  same fallback.
- A loaded carrier synthesizer adds its sound to the carrier, whatever CAR
  says. Its keys are held: each is a voice at pitch `key * 100` cents, volume
  0.5, centred (`FUN_00850aa0`), retriggered on load.

What the corpus uses:

| project | MOD | CAR | notes |
|---|---|---|---|
| render test `Vocodex.flp`, the 3 Umziky states, `Vocoder.flp` template | 1 / 0 | 0 | sidechain 1 absent or silent in some: the own input is both, as in FL |
| `Vocodex demo.flp` | 1 (Vocals) | 0 | 7600 noise |
| `Olbaid - Harmless Essentials`, `JuiceBass - Menagerie` | 2 | 1 | both external |
| `SeamlessR - Bass Antics` | 1 | 2 | both external, unison 1, pitch -1200 |
| `Treben - RawFL` | 0 | 2 | |
| `Ookay - Thief`, `NewStuff` | 0 | -1 | synth "PWM", keys 51, 63, 66, 73 |

No state uses L-R encoding or a saturation envelope. The engine hands a
plugin up to two sidechain inputs through `Effect::sidechain_inputs`.

## Signal chain

`Eff_Render` (`FUN_00856350`), with draft off. Buffers are interleaved stereo
f32 and the band filters run in f64.

1. **Sleep.** When the modulator and the carrier both stay under 1e-6 for
   `round(sr * 0.5 + slack + 16)` frames (`FUN_00853ce0`; slack is the
   longest hold plus release), the plugin copies its input to its output and
   stops. Never with a synth loaded or the envelope frozen.
2. **Synth.** Renders into the carrier.
3. **Contour** (exists when contour goes to MOD or NOISE and comes from CAR
   or CONSONANTS, `FUN_00851880`). From CONSONANTS: `TZXLookahead`
   (`FUN_00788680`) takes L+R of the modulator, a highpass at 90 Hz with
   alpha = sin w, adds 10^(-42/20), counts zero crossings in the last
   `round(0.008 sr)` frames, and puts out `min((count * 0.00625)^2, 1)`.
   From CAR: the carrier, or the consonant level times the carrier. Then the
   peak of the last `round(0.012 sr)` frames of `max(|L|, |R|)`
   (`TPeakLookahead2`) and three-stage smoothing, attack `0.016 sr`, release
   `0.044 sr`.
4. **Noise.** When the noise gain (`0x454`) is not 0, one call of
   `ippsRandUniform_Direct_32f` (pointer `*0x8dc668`, call at `0x856e55`)
   fills the block's interleaved stereo buffer `0x364`, two numbers a frame,
   with uniform noise in `[-a, a]` from the seed at `0x458`; then the noise
   highpass, times the contour with contour to NOISE, plus the carrier.
   This noisy carrier feeds the bands only. The seed is 0 after the reset
   `FUN_008537a0`, which a state load, a flush (dispatcher 2), a new
   process mode (1) and the slot's switch (11) run. No block draws noise
   while the plugin sleeps.
5. **Rates.** Each input at its rate and at twice it. The 2x upsampler
   (`FUN_0077ab60`) is two branches of three first-order allpasses
   `y = x[-1] + a (x - y[-1])`, a = 0.14738, 0.48247, 0.88301 and 0.03915,
   0.30265, 0.67462 (`*0x008bdbd8`); the second branch's sample comes first.
6. **Bands** (`FUN_00855cd0` for each). For each modulator voice: filter the
   modulator (`FUN_00855a60`), add it times the pass-through gain to the sum,
   follow it, subtract or gate the noise level, smooth it, and add it times
   its pan gains to a stereo envelope. Add the gain offset (clamped at 0 when
   negative). Filter the carrier with the carrier set and add envelope times
   carrier band to the sum.
7. **Mix** (`FUN_008557c0`). Output = carrier through the lowpass times its
   level, plus modulator through the highpass (times the contour with
   contour to MOD) times its level, plus the sum times wet. Level changes
   ramp over a block (`FUN_0062c760`). Filters only run while their level is
   above 0.
8. **Soundgoodizer** on the output when parameter 1 is above 0.

### The noise generator (`Shared/dsp_ippv2_x64.dll`)

`ippsRandUniform_Direct_32f(dst, len, low, high, &seed)` dispatches by CPU
(`0x1815b3fa0`, table `0x1815b3f58`). The generic code (`0x1815ed2e0`) and
the AVX2 code (`0x181645020`) put out the same numbers: both, run from the
DLL's bytes, agree on a million of them. One call:

- `x[0] = seed`, `x[-1] = 69069 x[0] + 0x3c6ef373`, `x[-2] = 69069 x[-1] +
  0x3c6ef373`, borrow `c = -1`, `L = 0x436cbae9`, all u32.
- For each number: `r = x[n-2] - x[n-3] + c`; when `r` is negative as an
  i32, `c = -1` and `x[n] = r - 18`, else `c = 0` and `x[n] = r`. Then `L =
  69069 L + 0x3c6ef373` and the number is `f32(i32(x[n] + L) * (high - low)
  * k + (low + high) * 0.5)`, in f64, with `k = 0x3df0000000000007` (2^-32
  and a hair).
- The seed handed back is the last `x[n]`. The next call starts its
  three words, its borrow and `L` over from it, so the run depends on how
  long FL's blocks are.

### Band layout (`FUN_00851dd0`)

The frequency map (`FUN_0062cb20` with 65.406 Hz, 21096.16 Hz, curvature 1)
takes x to Hz as `x * min / knee` under the knee
`ln 2 / ln(2 max / min) = 0.10714` and
`(e^((x - knee) r) - 1) (max - min) / (max / min) + min` above it, with
`r = ln(max / min + 1) / (1 - knee)`.

For each set (modulator voices k = -unison..0, then the carrier):

- the start gain `g = (RMS ? 1.5 : 1) / (order - flat (order - 1)) *
  sqrt(4 / sqrt(bands)) * 2^(order - 1) * 3` (the division only for order
  above 1);
- band i's centre `f = max(map(env8(i / (bands - 1))), 20)` Hz, its octave
  `log2(f / 65.406)`;
- a modulator voice's filters move by
  `ratio = 2^(-p17 / 1200 - 2 (env10 - 0.5) + unison[u][-k] p18 / 1000)`.
  The envelopes with their follow flag set read at the moved place;
- width `w = (mod ? 2^(p16/1000) / 4 : 1) * bandwidth * 2^(4 env9) / 4`;
- bandwidth in octaves `clamp(w * mean distance to the neighbouring
  centres, 1/60, 8)`, and the set's gain times `sqrt(1 / bandwidth)`;
- `order` RBJ bandpass stages (constant 0 dB peak,
  `alpha = sin w sinh(ln 2 / 2 * bw * w / sin w)`, `FUN_00626690`). A stage
  is `min(bandwidth * order, 2)` octaves wide; with more than one stage
  that over `1 + flat / 2`, the stages spread by `(j / (order - 1) - 0.5)
  flat bw` octaves, order 3 halves the outer two and order 4 takes 0.3636
  of the outer two;
- a set whose stages reach `2 bw` octaves above their centre past 11050 Hz
  is filtered at twice the rate and brought down by the mean of each pair
  (`FUN_0062d3a0`); the coefficient frequency is clamped at `0.4 sr`.

Then each modulator voice gets its pan gains `pan_law(pan, gain * carrier
gain / (unison + 1))` (`FUN_0062c860`: the far side the whole gain, the near
side `1 - |pan|` of it), with unison voices spread by
`((i - k) mod (unison + 1) / unison - 0.5) p19 / 128`. MIN TIMES keeps the
hold at least `sr / f` with f the first stage's frequency 3 octaves plus its
width below, clamped to 40..16000 Hz (twice that for RMS), and the attack and
release at least `sr / f` with f the carrier's first stage a width below,
clamped to 40..800 Hz.

### Followers and smoothing

- Peak (`TPeakLookahead2`, `FUN_00788010` and `FUN_00787e50`): the highest
  `max(|L|, |R|)` of the last `hold` frames.
- RMS (`TRMSLookahead`, `FUN_00788330`): `sqrt(sum(L^2 + R^2) / (2 hold))`
  over the last `hold` frames.
- Smoothing (`FUN_00606730`, run by `FUN_00606660`): `round(curve + 0.5)`
  one-pole stages, each `y = x + (y - x) c` with c the attack coefficient
  when x is above y and the release one otherwise, `c = 0.01^(1 / max(t,
  1))` for t frames, t over `sqrt(curve)` and the last stage taking the
  fractional part. FL rounds 1.5 to 2, so curve 1 is two stages and the
  second passes its input.

## What is not rebuilt

- Draft mode (`0x180 == 0`): bands decimated to octave levels and both
  inputs resampled by `FUN_008373e0`. FL uses it while it plays; this plays
  the HQ render path always.
- Saturation (envelopes 11 and 12, `*0x8dd640`). No corpus state turns it on.
- Curved envelope segments, drawn straight here. None in the corpus.
- The carrier synthesizer is this crate's Sytrus, itself approximate.
- The meters, the threads, the test speech and the editor.

The noise is FL's generator, drawn once a block as FL draws it, so its
numbers are FL's wherever the host cuts its blocks where FL's does. Where
FL cuts its blocks is the host's business and not read here.
