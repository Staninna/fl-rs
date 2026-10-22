# FL's shared effect classes: TStereoFlanger, TDelay_32FS, TFruityReverb

Harmless, Harmor, Sytrus, Autogun and Ogun link the same three Delphi
classes for their chorus, delay and reverb. Fruity Flangus is
`TStereoFlanger` with a plugin around it, and Fruity Reeverb 2 drives the reverb engine in
`Shared/Reverb_x64.dll` with modulation and early reflections on top.
`crates/plugins/src/fl_fx.rs` ports the three classes once. Each plugin
keeps its own front end: the tables or wheel maps, the dry and wet laws,
the block size and when it lets a class sleep.

Addresses are in `Harmless_x64.dll` (image base 0x400000). The decompiles
of this pass are in `ghidra/out/harmless/fx/` with listings in `fx/asm/`
(DecompileAddrs and DisasmAddrs on the `harmless` project); the older ones
are in `ghidra/out/harmless/run4`, `run4refs` and `voice`. The reverb
engine's are in `ghidra/out/reverbeng/`. Harmor's copies sit
about 0x5e000 higher (`docs/plugins/harmor.md` section 4).

Confidence tags: `code` (read in a decompile), `asm` (read in the
listing), `inferred`.

## TStereoFlanger (VMT `0x0074a718`, parent `TBaseDelayLine` `0x00746e68`, 0x78 bytes)

The field map, the voice record and the maths are in
`docs/plugins/fruity-flangus.md` sections 2 and 3: the Harmless copy is the
same code. Functions:

| function | Harmless | does |
|---|---|---|
| constructor | `FUN_0074a790` | +0x2c = 8 bytes a frame, order 4, gains 1; renderer +0x60 = `FUN_0074b2f0` (x87) or `FUN_0074b7e0` when CPU byte +0x16 is set |
| set up | `FUN_0074a9b0(this, block, SR)` | the voices, the normalisation times +0x50, +0x54 = round(longest) + 8; ring of (block + +0x54) frames by `FUN_00747070`; on a resize or another spread: read = write, `FUN_0074b050` |
| flush | `FUN_0074b050` | every phase = its start, slot 0 `FUN_00746ff0` zeroes the ring |
| write | `FUN_0074b0f0(this, src, n)` | copy the block into the ring at the write position, wrapping |
| render | `FUN_0074b2f0(this, dst, n)` | `FLDCW 0x0f3f` (`FUN_005a43a0`, truncate), voice 0 `FUN_0074b180` writes, the others `FUN_0074b230` add, `FLDCW 0x033f` (`FUN_005a4380`); read position += n |
| sleep | `FUN_0074b0b0(this, n)` | every phase += step × n; ring and positions stay |

The output gain +0x50 is what tells the users apart: Flangus leaves it at
1 and applies its own ramped wet level, Harmless puts its `5^m` wet there,
Harmor its signed wet. The voices' gains carry it. `code`, `asm` for the
kernels.

## TDelay_32FS (VMT `0x00746f90`, parent `TBaseDelayLine`, 0xe8 bytes)

| offset | type | meaning |
|---|---|---|
| 0x08 | ptr | ring, interleaved stereo f32 |
| 0x10 / 0x18 / 0x20 | ptr | read position, write position, ring end |
| 0x28 / 0x2c | i32 | ring size in bytes, bytes a frame (8) |
| 0x30 / 0x34 | f32 | feedback target, ramp state |
| 0x38 | ptr | the host object (ramp cap at +0x34) |
| 0x40..0xd8 | method ptrs | ten kernels; Harmless and Harmor use the first three |

| function | Harmless | does |
|---|---|---|
| constructor | `FUN_007471d0` | kernels, 8 bytes a frame, feedback 0.5 |
| size | `FUN_00747070(this, extra, delay)` | delay = max(1, delay); when `(extra + delay) × 8` differs from +0x28: reallocate (`FUN_00747010`), slot 0 `FUN_007474d0` (zero the ring, state = target), read = start, write = read + delay frames. Returns whether it resized |
| input | `FUN_00747510(this, src, n, at)` | add every other float of `src` (one side) into the ring from `at`, wrapping once; `at` = write + offset bytes, + 4 for the right side |
| kernel +0x40 | `FUN_00747660` → `FUN_007475a0` | out = (L, R) read; ring[read] = 0; ring[write] += (L, R) × g |
| kernel +0x50 | `FUN_00747780` → `FUN_007476d0` | out = (R, L); feedback as read |
| kernel +0x60 | `FUN_007478b0` → `FUN_007477f0` | out = (R, L); ring[write] += (R, L) × g: ping-pong |

Each kernel first runs the host ramp `FUN_00612db0` (the same as
`parts::glide`) from +0x34 to +0x30 over the block, capped at host+0x34 a
frame, and steps g a frame. After each add it sets a side to 0 when its
magnitude is under 2^-24 (`DAT_007d7774 = 0x33800000`). Both positions
move one frame a frame and wrap at the ring end. With the input added at
write + offset before the kernel runs, a sample comes out `delay + offset`
frames later, then every `delay` frames times g. The block must not
exceed `extra - offset`, which is why the front ends pass the host block in
`extra`. `code`.

## TFruityReverb (VMT `0x007494d0`, parent `TBaseReverb` `0x00749398`, 0xa8 bytes)

The wrapper loads `Reverb_x64.dll`, calls `CreateReverbInstance(0)` and
asks the object for its data block (`FUN_00749540`). Wrapper fields:

| offset | type | meaning |
|---|---|---|
| 0x10 | i32 | sample rate (44100 at construction) |
| 0x18 | ptr | the one buffer of all lines |
| 0x20 | u8 | dirty |
| 0x28 / 0x30 | ptr | engine object, its data block `D` (engine offset `E = D + 0x10`) |
| 0x38 | f32 | low cut, Hz, 0 = off |
| 0x3c | f32 | high cut, 100 Hz, 0 = off |
| 0x40 | i32 | predelay, samples |
| 0x44 / 0x48 | i32 | room size, diffusion % |
| 0x4c | f32 | decay, samples |
| 0x50 | f32 | high damping, 100 Hz, 0 = off |
| 0x54 | i32 | bass colour 0..4 |
| 0x58 / 0x5c | f32 | modulation speed and depth (0.5 each), unused by this process |

The constructor `FUN_0074a360` starts at `(75, 40, 0, 50, 100, 15, 40, 2)`
and runs slot 0.

Slot 0 `FUN_00749970` (size): the twelve lengths at full size,
`MulDiv(T[i], SR, 20000)` with `T` = 142 107 379 277 672 4453 1800 3720
908 4217 2656 3163; the predelay line `max(1, +0x40)` long, so a predelay
of 0 is one sample; indices 0; slot 1 `FUN_00749ae0` carves the buffer; the
taps at full size; then `FUN_007498f0` zeroes the buffer and the eight
filter states. The setters call it on another rate or predelay.

`FUN_005b8070` is this DLL's `MulDiv`: a 128-bit product and a truncating
`IDIV`, no rounding. `code`.

Update `FUN_00749b60` (run at the next process when dirty), with
`inv = (f32)(1 / SR)`:

```
low cut   lc = +0x38: 0 -> D+0xc = 0; else lc = min(lc, SR/2); t = (f32)tan(lc pi inv); D+0xc = (t - 1)/(t + 1)
high cut  hc = +0x3c: 0 -> D+0x24 = 1; else hc = min(hc, SR/2)   // in 100 Hz units against Hz: never bites
          c = (f32)(2 - cos(hc 200pi inv)); D+0x24 = 1 - (c - sqrt(c*c - 1))
scale     = MulDiv(SR, room + 15, 115)
len[i]    = max(1, MulDiv(T[i], scale, 20000)); idx[i] %= len[i]
tapL, tapR = MulDiv(TL / TR, scale, 20000)      // TL 266 2974 1913 2496 1690 187 1566, TR 353 3627 1728 3173 1811 335 621
room != D+0x1e8: zero the twelve lines; D+0x1e8 = room
d = (f32)(diffusion 0.01); h = (f32)(inv 0.5)
d == 0: D+0x14, 0x18, 0x1c, 0x20 = 0
else    D+0x1c = exp(ln0.001 (f32)((len0 + len1) h) / (d 0.07))     // the four input allpasses
        D+0x20 = same on len2 + len3, 0.11                          // not read
        D+0x14 = same on len4 + len8, 0.35;  D+0x18 = len6 + len10, 0.51
loop  = len5 + len7 + len9 + len11
g     = Power(exp(ln0.001 loop / decay), 0.25)          // D+0x10
damping D+0x28 as the high cut on +0x50
m     = {0.2, 0.6, 1.0, 1.5, 3.0}[colour], else 1     // 0x74a34c, 0x74a350, 0x74a348, 0x74a354, 0x74a358
r     = (f32)Power(exp(ln0.001 loop / (decay m)), 0.25) / g
D+0x38 = (r - 1) 0.5;  r = min(r, 1)
t = (f32)tan(500 pi inv);  D+0x34 = (t - r)/(t + r)    // 500 Hz at 0x74a35c
```

`ln 0.001` sits in `.bss` (`DAT_00801688`) and is `inferred` from its use,
as in Reeverb 2.

Process `FUN_0074a540`: update when dirty, then engine slot 2,
`FUN_180002980(E, mono, stereo, n)`, which has no modulation lines and no
early reflections. Per sample (`ap` is `v = line[i] - g x; line[i] = g v +
x`, every line's index counts down after its access and wraps to its
length - 1):

```
lp = ((x + 1e-30) - lp) E34 + lp
a  = (lp - a1) E1c + x1;  x1 = lp;  a1 = a
x  = E1c != 0 ? (lp - a) 0.5 + 1e-30 : lp
x  = ap(line0..3, E2c)
p  = predelay[i];  predelay[i] = x
half A: c = line11[i11] g                                   // before B writes it
        s = (c - sA_out) E44 + sA_in;  sA_in = c;  sA_out = s
        y = (((s + c) E48 + c + p) - dA) E38 + dA;  dA = y
        line7 <- ap(line6, E28) of (line5 <- ap(line4, E24) of y) g
half B: c = line7[i7] g                                     // after A wrote and stepped it
        the same through lines 8..11 with its own states
outL = line9[i9+tL0] - w1 line9[i9+tL1] - w3 line10[tL2] - w4 line11[tL3] + w2 line5[tL4] + line6[tL5] + w5 line7[tL6]
outR = w1 line5[tR1] + line5[tR0] - w3 line6[tR2] - w4 line7[tR3] - w2 line9[tR4] + line10[tR5] + w5 line11[tR6]
w = (1 - g) K + g with K = 0.25, 0.6, 0.7, 0.2, 0.5 (0x18006036c, 374, 378, 368, 370)
```

The taps read after every index has stepped, so a tap of `k` is the sample
written `k - 1` frames before this one. `code`, `asm` for the tap order.

## Reeverb 2

Reeverb 2 drives the modulated process `FUN_180003270`, which has the same
structure as `FUN_180002980` plus modulation and early reflections. It
keeps its own network in `reeverb_2.rs` and takes `Line` and the tables
from `fl_fx.rs`. `docs/plugins/fruity-reeverb-2.md` has where that network
reads the tank and the taps.

## Front ends

| user | flanger | delay | reverb |
|---|---|---|---|
| Fruity Flangus | the eight wheels, gain 1, its own ramped dry and wet | | |
| Harmless (`harmless.md` section 8) | table of six wheels, `5^m` dry and wet, host block, sleeps with the voices | 192nds of a beat, one offset side, straight or ping-pong, kernels always run | table of eight settings, L + R, wet `level / 2` |
| Harmor (`harmor.md` 4.3..4.5) | its REC wheels, signed wet, 384-frame chunks | filtered input or in-loop filters on `span` (the port filters the input either way), three modes, 384-frame chunks | its own maps, smoothed wet |

Sytrus (`sytrus.md` section 9) and Autogun and Ogun (`autogun.md` section
5) use the three classes from `fl_fx.rs` too, behind front ends their own
docs map.
