# Effector: state, parameters, signal chain

Recovered from `Plugins/Fruity/Effects/Effector/engine_x64.dll` (FL Studio
2026, MSVC C++, image base 0x180000000, 486 kB, PDB name `Effector_x64.dll`,
version string `1.1.0 Rev2661`). `Effector_x64.dll` beside it is the Delphi
shell that loads it and has no DSP. The engine DLL is a whole FL plugin of
Image-Line's C++ FX framework (`CPluginWrapper`, `TCPPFruityPlug`,
`FXPlugin`, `FXEditor`), with its own `CreatePlugInstance`. Ghidra headless
import of the engine DLL alone, with every function decompiled to one file
and every function's listing to another (scratch, not kept). Corpus: 27 uses
in 12 of the 165 `.flp`, and the 3 database copies of `Effector.fst`.

Confidence tags: `loader`, `asm`, `corpus`, `inferred`.

## How the plugin is wired

| what | address |
|---|---|
| `FXPlugin` constructor: names the 12 parameters, builds the effects' host, applies the defaults | `FUN_180017050` |
| parameter defaults | `FUN_180017920` |
| set parameter (all the parameter maths) | `FUN_180017600` |
| `ProcessParam` path: host integer `v / 256`, clamped to 0..1 | `FUN_180003410` (asm `0x18000362f`) |
| state save / load of the framework chunk | `FUN_180017bf0` / `FUN_180017cf0` |
| FL state wrapper (`SSLF` + length) | `FUN_180002f00` |
| render: input level, then the host, then output gain | `FUN_180017a00` |
| set sample rate | `FUN_1800179a0` |
| effects' host constructor (0x88 bytes, 12 units) | `FUN_18001b7f0` |
| host: run the 12 units / output gain | `FUN_18001ba60` / `FUN_18001b970` |
| unit constructor (0xb28 bytes), reset, allocate lines | `FUN_180018590`, `FUN_180018a50`, `FUN_180018700` |
| unit dispatch by kind | `FUN_180018c20` |
| LFO | `FUN_180017ec0` |
| LFO phase sync to the song position | `FUN_180016d40` |
| knob labels per effect | `FUN_180015780` |

Effects by kind (the unit's index is its kind; the labels are X / Y):

| kind | labels | function | built |
|---:|---|---|---|
| 0 | Gain / Tone | `FUN_180019860` distortion | yes |
| 1 | SRate / Bits | `FUN_180019b60` lo-fi | yes |
| 2 | Time / Fdbk | `FUN_180019d60` flanger | yes |
| 3 | Time / Fdbk | `FUN_18001a680` | no |
| 4 | Freq / Res | `FUN_180019110` | no |
| 5 | Time / Flt | `FUN_180018de0` tape echo | no |
| 6 | Decay / Mix | `FUN_18001a130` reverb (`fxReverb`) | yes |
| 7 | Angle / Distns | `FUN_18001abc0` two short tape delays | no |
| 8 | Dropout / Smth | `FUN_180019f90` trance gate ("ducker" in the projects) | yes |
| 9 | Freq / --- | `FUN_18001ae50` loop repeat | no |
| 10 | Vowel / Size | `FUN_18001b140` vowel filter | yes |
| 11 | Freq / --- | `FUN_18001b5a0` ring modulator | no |

## 1. State layout (event 213)

`FUN_180002f00` (load):

```
u32 'SSLF' (0x464c5353); u32 len; len bytes of chunk     // len 0 or another tag: ignored
```

Chunk, `FUN_180017cf0`:

```
u32 0x46583031 ('FX01' as a little-endian word)          // else refused
256 bytes  program name (vtable 0xa8)                    // all zero in the corpus
f32 x 12   parameters, each through set parameter (clamped 0..1), until the bytes run out
```

Refused when the chunk is under 4 bytes, the tag differs, or fewer than 256
bytes follow it. Every corpus state is 316 bytes (8 + 308) and parses to the
end (`loader`, `corpus`).

## 2. Parameters (`FUN_180017600`, defaults `FUN_180017920`)

Values are f32 0..1. The host's `ProcessParam` hands an integer that the
wrapper multiplies by 1/256 (`asm` 0x18000362f), so FL counts each parameter
0..256.

| # | name | default | maths |
|---:|---|---|---|
| 0 | Effect Kind | 0.9545454 (`0x3f745d17`) | `kind = trunc(clamp(11 v + 0.5, 0, 11))`; units other than `kind` are bypassed |
| 1 | FX Bypass | 1 | `v != 0` bypasses every unit. New instances start bypassed: a performance plugin |
| 2 | Wet Mix | 1 | each unit's wet target (+0xadc) |
| 3 | Param X | 0.5 | unit +0xc |
| 4 | Param Y | 0.5 | unit +0x10 |
| 5 | Mod Rate | 1/9 (`0x3de38e39`) | `i = max(0, trunc(8 v + 0.5))`, beats a cycle `[1/8, 1/4, 3/8, 1/2, 3/4, 1, 2, 4, 8][min(i, 8)]`, clamped 1/128..128; then the LFO phases resync |
| 6 | Tempo Reduce | 0 | `rate = max((1 - v)^2 * beats, 1/512)` each block (`FUN_18001ba60`) |
| 7, 8 | Mod Amount X, Y | 0.5 | `a = 2v - 1`, stored `a^2` with `a`'s sign (`FUN_18001bc80`, `FUN_18001bd70`) |
| 9 | Mod Shape | 0 | `clamp(trunc(2v + 0.5), 0, 2)`: sine, saw, square |
| 10 | Input Level | 1 | `v^2` |
| 11 | Output Gain | 0.5 | `powf(10, (v - 0.5) * 2.4)`, ±24 dB |

Corpus kinds (rounded `11 v`): 6 reverb in 11 slots, 8 gate in 8, 10 vowel
in 4, 1 lo-fi in 2, 0 distortion in 1, 2 flanger in 1 (the render test).
Bypass 0 in every project, 1 in the three `.fst`. Tempo Reduce 0 and Mod
Shape 0 everywhere but one shape 1; Mod Rate steps 1, 3, 5 and 8 (`corpus`).

## 3. Signal chain

Render (`FUN_180017a00`), per block:

```
in gain: two one-pole smoothers toward v^2, step 0.01, snap on first use (state -1);
         L *= sL (before its step), R *= sR (before its step)
host.tempo = song tempo (vtable 0x60)
for each of the 12 units: unit.rate = max((1 - reduce)^2 * beats, 1/512); unit.run(L, R, n, tempo)
out gain: g += (target - g) * 0.005 (snap on first use); L *= g; R *= g     // g stepped before the multiply
```

Unit dispatch (`FUN_180018c20`), when `sr >= 1` and `tempo >= 1`:

```
wet_now < 0 -> wet_now = wet
seconds = rate * 60 / tempo                       // f32
step = seconds == 0 ? 0 : (double)(1.0f / (seconds * sr))
```

LFO (`FUN_180017ec0`), phase a double in 0..1, wrapped by `x - trunc(x)`:

```
shape 0: value = (float)sin(2 pi phase_before); phase += step
shape 1: phase += step; value = (float)(1 - 2 phase)
shape 2: value = phase_before > 0.5 ? -1 : 1; phase += step
```

Effects 1, 6 and 10 read it once a block and then move the phase on by
`(n - 1) step`; the flanger reads it every frame; 0 and 8 do not read it.
`X' = clamp(lfo * modX + X, 0, 1)`, `Y'` likewise. Phase sync
(`FUN_180016d40`, on a Mod Rate change or a transport event, `FUN_180017010`):
`phase = frac(0.25 / rate * 4 pos)` with `pos` the song position from
vtable 0x70 (`inferred`: in beats).

Fade and wet (effects 2, 6, 10): per frame `wet_now += (wet - wet_now) * 0.01`
and `fade += (!bypassed - fade) * 0.01`. A bypassed unit with `fade == 0`
returns at once; after a block a bypassed unit with `fade < 1e-4` is reset
(`FUN_180018a50(unit, 0)`): delay lines and filters emptied, phase 0, fade 0,
`wet_now = -1`. Effects 0, 1 and 8 skip the block when bypassed and never
fade.

### Effect 0, distortion (`FUN_180019860`)

```
d = clamp((float)(X * 0.95 + 0.05f), 0, 1);  k = d^2 * 2048        // X in double, the 0.05 widened from f32
t = 0.9 Y + 0.1;  c = (t t)(t t)                                    // tone low-pass coefficient
s(x): a = x / (|x| k + 1); a /= a a k + 1; a /= |a a a| k + 1; a /= (a k a)(a a) + 1
g = 0.5 / s(0.5)                                                    // unity at 0.5
per frame: w += (wet - w) * 0.01
           lp += (x - lp) c;  y = s(lp) g + (x - lp)
           out = (1 - w) x + w y
```

### Effect 1, lo-fi (`FUN_180019b60`)

```
q = 65536 / (Y'^2 * 2048 + 1);  hold = X'^3 * 128 + 1
per frame: if count <= 0: count += hold; hR = trunc(q R) / q; hL = trunc(q L) / q     // (1/q) as f32
           count -= 1;  w step;  out = (1 - w) x + w h
```

### Effect 2, flanger (`FUN_180019d60`)

Two `CTapeDelay`s of `N = trunc(0.005f * sr)` samples (`FUN_180018700`).
Per frame: wet and fade step, LFO, `time = clamp(X^2 + lfo modX)`,
`fb = clamp(lfo modY + Y) * -0.96`, `back = tape.run(-(x fade))`,
`x += back * w`. The delay is `time (N - 2) + 3` samples (see the tape).

`CTapeDelay` (`FUN_180017f80` constructor, `FUN_1800181c0` run,
`FUN_180018520` time, `FUN_180018560` feedback, `FUN_180018120` clear):
two "turns" of `N + 256` (value, place) pairs; the head's place runs 0..1.

```
target = 1 / ((t (1 - 2/N) + 2/N) N);  speed glides: speed += (target - speed) * 0.005 after each frame
read:  in the other turn, from its read index, the first entry with place >= head;
       linear interpolation with the entry before it (places compared as f32); |v| < 1e-9 -> 0
lp += (v - lp) kL;  hp += (lp - hp) kH;  written = (lp - hp) fb + x        // kL = 1, kH = 0 for the flanger
head += clamp(speed * wow, 1/N, 0.9)                                         // wow depth 0 in every effect
head >= 1: copy the other turn's last three entries (places - 1) to this turn's header,
           append the frame here too, swap turns, head -= 1, new turn empty
append (written, head);  return v (before the filters)
```

The read is one turn plus one frame behind the write, hence `+ 3` at
`t = 0` and `N + 1` at `t = 1`.

### Effect 6, reverb (`FUN_18001a130`, `fxReverb` VMT `0x180057fc0`)

Per block: LFO once, `X'`, `Y'`; per frame fade and wet step and the input
copied scaled, `in = fade * w * x`, into 1024-frame buffers. Then the
reverb's parameters, each a 0..1 float that `FUN_18001cc30` turns into
`floor(v scale + offset)`:

| # | name | scale, offset | Effector sets | value |
|---:|---|---|---|---|
| 0 | Input | 421, -360.5 | preset 0: `0x3f5b83f4` | 0 (0 dB) |
| 1 | LowCut | 2981, 19.5 | 0.1 | 317 Hz (19 = off) |
| 2 | HighCut | 216, 5.5 | 1 | 221 = off |
| 3 | Predelay | 250, 0.5 | 0 | 0 ms (a 1-frame line) |
| 4 | RoomSize | 99, 1.5 | 1 | 100 |
| 5 | Diffusion | 100, 0.5 | 1 | 100 |
| 6 | BassMultiplier | 28, 2.5 | preset 0: `0x3e924925` | 10 (x1.0) |
| 7 | Crossover | 1975, 25.5 | preset 0: `0x3e76474b` | 500 Hz |
| 8 | Decay | 199, 1.5 | `0.75 X'^3` (`X'^2 (0.75 X')`) | tenths of a second |
| 9 | HighDamping | 216, 5.5 | `Y'^2` | hundreds of Hz, 221 off |
| 10 | Dry | 421, -360.5 | 0 | -361: silent |
| 11 | ER | 421, -360.5 | preset 0 | muted by 16 |
| 12 | Reverb | 421, -360.5 | `0.3 * decay + 0.5` | tenths of a dB |
| 13 | Width | 200, 0.5 | preset 0: 0.5 | 100 % |
| 14..17 | MuteInput, MuteDry, MuteER, MuteReverb | 1, 0.5 | 16 = 1 | |

Preset 0 (`FUN_18001bff0`, applied by `FUN_18001c840`) gives the ones the
Effector leaves. Levels are `powf(10, i * 0.005)`, 0 under -360
(`FUN_18001dab0`). The unit tells the reverb its rate is 44100 whatever the
song's (`asm` 0x180018a8c), so all its lengths and filters are tuned for
44.1 kHz.

Tuning (`FUN_18001dae0`, when a parameter changed), `sr = 44100`:

```
in     = dB(Input), 0 if MuteInput
lowK   = LowCut > 19 ? (t - 1) / (t + 1), t = tanf(min(sr/2, LowCut) pi / sr) : 0
highK  = HighCut < 221 ? 1 - (c - sqrt(c^2 - 1)), c = 2 - cosf(HighCut * 628.31854 / sr) : 1
pre    = max(1, sr_int * Predelay / 1000)
scale  = (Room + 15) sr_int / 115;  len[i] = max(1, T[i] scale / 20000)  // T = 142 107 379 277 672 4453 1800 3720 908 4217 2656 3163
taps   = scale * {266 2974 1913 2496 1690 187 1566 | 353 3627 1728 3173 1811 335 621} / 20000
d = Diffusion * 0.01 (all four 0 when d == 0):
  gIn1 = expf(-49.34111 / d * (len0 + len1) / sr)     gIn2 = expf(-31.398888 / d * (len2 + len3) / sr)
  gT1  = expf((len4 + len8) * -3.4538777 / sr * 2.857143 / d)
  gT2  = expf(-3.4538777 / sr * (len6 + len10) * 1.9607843 / d)
dec  = Decay * 0.1;  L = (len9 + len7 + len5 + len11) / sr * -6.9077554
g    = powf(expf(L / dec), 0.25);  gB = powf(expf(10 / (Bass * dec) * L), 0.25)
r = gB / g;  bass = (r - 1) / 2;  r = min(r, 1);  t = tanf(Crossover pi / sr);  cross = (t - r) / (r + t)
damp = HighDamping as highK;  dry = dB(Dry) (0 if MuteDry);  wet = 0.3 dB(Reverb) (0 if MuteReverb)
```

The lines are zeroed when the room changes, and filled with 1e-30 on reset
(`FUN_18001ca20`, which also snaps the wet smoother through -1000).

Per frame (`FUN_18001e9f0`); `ap(n, x, g, h)`: `v = line[i] - x g;
line[i] = v h + x; i--`:

```
xL = in L; xR = in R
lp += ((xR + xL) * 0.5 + 1e-30 - lp) highK
ap = (lp - s1) lowK + s2; s2 = lp; s1 = ap;  x = lowK != 0 ? (lp - ap) * 0.5 + 1e-30 : lp
x = ap(0, x, gIn1, gIn1); x = ap(1, x, gIn2, gIn1); x = ap(2, x, gIn1, gIn1); x = ap(3, x, gIn2, gIn1)
pd = pre[i] + 1e-20; pre[i] = x
half A: c = line11[i11] g; apx = (c - xa1) cross + xa0; xa0 = c; xa1 = apx
        y = dampA += (((apx + c) bass + c + pd) - dampA) damp
        t = ap(4, y, gT1, gT1); d = swap(line5, t) g; u = ap(6, d, gT2, gT2); line7 <- u
half B: the same from line7 (read after the write moved its index), lines 8..11
wetS = wetS < -99 ? wet : wetS;  wetS += (wet - wetS) * 0.001
w_K = (1 - g) K + g  for K = 0.6, 0.25, 0.7, 0.2, 0.5
Lr = wetS ((((l9[t0] - w.25 l9[t1]) - w.7 l10[t2]) - w.2 l11[t3]) + w.6 l5[t4] + l6[t5] + w.5 l7[t6])
Rr = wetS (((((w.25 l5[t1'] + l5[t0']) - w.7 l6[t2']) - w.2 l7[t3']) - w.6 l9[t4']) + l10[t5'] + w.5 l11[t6'])
mid = Rr + Lr;  side = (Lr - Rr) * Width / 100
L += xL dry + side + mid;  R += xR dry + (mid - side)          // added to the unit's own input
```

Taps read `line[(i + tap) wrapped once by len]` after the frame's writes.
The early reflections (an ER line, 8 taps a side) are computed only when
their gain is not 0; the Effector mutes them, so they are not mapped here.
Compared with Fruity Reeverb 2's engine (`fruity-reeverb-2.md`): the same
tables and topology, no modulation, the second input gain on the way in of
diffusers 1 and 3 with the first on their way back, both halves fed the
same predelay output, and a dry path.

### Effect 8, trance gate (`FUN_180019f90`)

Runs when not bypassed and `rate != 0`. Uses the unit's phase directly, so
the gate is locked to the beat; no LFO modulation, no wet mix.

```
x = clamp(X), y = clamp(Y);  depth = 1 - y^2;  k = (1 - x)^2 * 50 + 1
per frame: env = phase >= 0.01f ? 1 - min(((float)((phase - 0.01f) * 1.0101010003706326))^4 k, 1)
                                : (float)(phase * 100.00000223517424)
           phase = frac(phase + step);  s += (env - s) * 0.01;  gain = 1 - s depth;  x *= gain
```

### Effect 10, vowel filter (`FUN_18001b140`)

```
Y'' = Y' * 0.93;  scale = powf(2, ((Y'' - 0.5) + Y'') - 0.5)
p = 3 X';  i = trunc(p);  f = p - i
weights (A, U, I): i 0 -> (f, 1 - f, 0); 1 -> (1 - f, 0, f); 2 -> (0, f, 1 - f); 3 -> (f, 1 - f, 0) with i = 0
gain = 1 - i / 8
A = 700 1250 2500 3500 4200 8500 10000 12000      // 0x1800586b0 + 0x1800586d0
U = 250  700 2500 3500 4200 8500 10000 12000      // 0x1800586a0
I = 250 2500 2700 3500 4200 8500 10000 12000      // 0x1800586c0
freq[k] = ((wA A[k] + wU U[k]) + wI I[k]) * scale
per frame: wet, fade; m = fade w
           yL = sum_k band(L) * gain;  L = yL m + (1 - m) L    (same for R)
```

Band (`FUN_180014520` coefficients, `FUN_180014680` run), recomputed when
the frequency, Q or rate changes; Q is 12:

```
w = 2 pi f / sr (double); n = 1 / ((0.5 / Q) sin w + 1)
b0 = (float)((float)(sin w * 0.5) n);  a1 = (float)((float)(2 cos w) n);  a2 = (float)((float)(1 - sin w / (2Q)) n)
y = ((x - x2) b0 + y1 a1) - a2 y2                       // constant-skirt band-pass, peak gain Q
coefficients, input and output under 1e-9 in size flush to 0
```

## 4. Not built, and what is not exact

- Effects 3, 4, 5, 7, 9 and 11 are not used by any corpus project and pass
  the input through; `fidelity()` reports it when one of them is picked.
  Addresses above for whoever builds them; 5 and 7 reuse `CTapeDelay`,
  11 is `sin(2 pi phase)` at `2^(8.7 X' + 5)` Hz.
- The song position the LFOs resync to is counted by the plugin from the
  blocks it played since the last reset, not read from the host.
- FL reads the LFO once per block for effects 1, 6 and 10; the blocks are
  the host's, so the step pattern follows our host's block sizes (cut to
  1024 frames at most, the reverb's buffer size in FL).
- `sin`, `cos`, `tan`, `exp` and `pow` are Rust's, not the MSVC runtime's:
  last-bit differences are possible.
