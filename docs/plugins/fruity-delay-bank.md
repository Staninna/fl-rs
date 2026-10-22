# Fruity Delay Bank: state, parameters, signal chain

Recovered from `Fruity Delay Bank_x64.dll` (FL Studio 2026, Delphi native
plugin, image base 0x400000). Ghidra project `ghidra/proj/delaybank`,
decompiles under `ghidra/out/delaybank/` (`vmt/`, `slots/` with callees,
`dsp/` .. `dsp4/` for the kernels, the delay line and the filter setup).
Corpus: 41 uses in 16 of the 165 `.flp`, 21 presets in the plugin's folder
and 3 more `.fst` elsewhere in `Data`. All 65 states are 792 bytes and parse
to the last byte.

The DSP is in the DLL itself, in Delphi: nothing in FLEngine is called for
audio. It is a big plugin. Eight units, each with two multi-stage filters of
eight modes, a delay line with ten read and write routines, a grain window
and an optional 2x oversampler around the lot. This map has the state, the
parameters, the routing, the delay line, the gains, the grain, the
filters (section 3.1) and the oversampler (section 3.2). The grain's shape
curve was located and not read through: section 4 lists it with its
address.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `asm`, `inferred`.

## How the plugin is wired

`CreatePlugInstance` @ `0x006ddd50` -> constructor `FUN_006de4b0`, class
reference `PTR_PTR_006ddb68` -> VMT `0x006ddc30` (`TTestPlug`, instance size
0x21f8).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006ddda0 |
| 3 | SaveRestoreState | 0x006de180 |
| 4 | GetName | 0x006e0fa0 |
| 5 | ProcessEvent | 0x006dffb0 |
| 6 | ProcessParam | 0x006e0000 |
| 7 | Eff_Render | 0x006e1380 |

Object fields:

| offset | type | meaning |
|---|---|---|
| 0x120 | i32[133] | the parameters and the oversampling switch, saved |
| 0x32c | i32 | soloed unit, -1 for none (not saved) |
| 0x334 + u * 0x3d0 | | unit `u`, 0..7 (below) |
| 0x21b4 | i32 | the unit the window shows |
| 0x21bc / 0x21b8 / 0x21c0 / 0x21c4 | f32 | dry, wet, input and feedback gains |
| 0x21c8 | i32 | host block size (Dispatcher 3) |
| 0x21cc | f32 | tempo, 140 until `ProcessEvent(FPE_Tempo)` |
| 0x21d0 / 0x21d4 | i32 | host sample rate / oversampling factor 1 or 2 |
| 0x21d8, 0x21e0 | ptr | the up and the down sampler (`TOversampler`, section 3.2) |
| 0x21e8 | u8 | host flag bit 1 of Dispatcher 1: forces one stage on |

A unit `U`:

| offset | type | meaning |
|---|---|---|
| 0x00 / 0x01 | u8 | enabled / first echo left alone, saved |
| 0x04 | gain block | input: pan, volume, target[4], current[4], dirty |
| 0x30 | gain block | output, same shape |
| 0x5c / 0x60 / 0x64 | f32 | input wheel gain, output wheel gain, "to next" gain |
| 0x68 | filter | input filter: stages of 0x78 bytes; +0x170 mode, +0x174 stage count - 1 (both saved) |
| 0x1e0 | filter | feedback filter, same shape |
| 0x358 | i32 | input filter position: -1 off, 0 before the delay, 1 after |
| 0x35c | u8 | "POST", saved |
| 0x360 | ptr | delay line (class `PTR_PTR_006d8d88`), nil when there is none |
| 0x368 / 0x36c | f32 | stereo offset in 10 us units / delay in frames before rounding |
| 0x370 / 0x374 | i32 | byte offset into the line for the left / right write |
| 0x378 / 0x37c / 0x380 | f32 | feedback pan, volume, separation |
| 0x384 / 0x394 | f32[4] | feedback gains: target, current |
| 0x3a4 | i32 | feedback mode, saved |
| 0x3a8 | u8 | tempo-based time, saved |
| 0x3ac / 0x3b0 | i32 | grain division, grain shape, saved |
| 0x3b4 / 0x3bc / 0x3c4 / 0x3cc | ptr, f64, f64, i32 | grain window, its period, the phase, its length |

## 1. State layout (event 213, 792 bytes)

`SaveRestoreState` (`FUN_006de180`):

```
save:    Write(int32 16); Write(this+0x120, 0x214); for each unit: the 32 bytes below
restore: Read(int32 ver); if ver > 14:
           Read(this+0x120, 0x214)
           for each unit: read the 32 bytes; if ver < 16: param[12 + 16 u] *= 10
           configure the oversampler (FUN_006dff00); set every unit up (FUN_006dfbb0); flush (FUN_006dfe30)
           for each parameter: ProcessParam(i, GetValue, UpdateValue | 0x10)
```

A version under 15 is left unread. The corpus has 15 (x18) and 16 (x47).

The 0x214 bytes are 133 i32. Index 0..3 are the plugin's, `4 + 16 u + k` is
parameter `4 + k` of unit `u`, and 132 is the oversampling switch. Index 131
would be unit 8's "to next": it is no parameter (`ProcessParam` takes
indexes under 131) and the Default preset has -1 there.

| idx | parameter (DFM object) | range | default | corpus and presets | confidence |
|---|---|---|---|---|---|
| 0 | Dry (`DryWheel`) | 0..128 | 128 | 0 (x14), 128 (x48) | loader, dfm, corpus, preset |
| 1 | Wet (`WetWheel`) | 0..128 | 128 | 0..128 | loader, dfm, corpus, preset |
| 2 | Input (`InMulWheel`) | 0..128 | 128 | 0 (x3), 128 | loader, dfm, corpus, preset |
| 3 | Feedback (`FBMulWheel`) | 0..128 | 128 | 73..128 | loader, dfm, corpus, preset |
| 4 | Input panning | -128..128 | 0 | -128..128 | loader, dfm, corpus, preset |
| 5 | Input volume | -160..160 | 128 | -60..160 | loader, dfm, corpus, preset |
| 6 | Input filter cutoff | 0..65536 | 33145 | 0..52857 | loader, dfm, corpus |
| 7 | Input filter resonance / bandwidth | 0..65536 | 17500 | 12764..53340 | loader, dfm, corpus |
| 8 | Input filter gain | 0..256 | 128 in the DFM, 0 in the Default preset | 0..199 | loader, dfm, corpus |
| 9 | Feedback panning | -128..128 | 0 | -119..70 | loader, dfm, corpus, preset |
| 10 | Feedback volume | -128..128 | 64 | 0..121 | loader, dfm, corpus, preset |
| 11 | Feedback stereo separation | -128..128 | 0 | -79..128 | loader, dfm, corpus, preset |
| 12 | Time (`DelTimeWheel`) | 0..7680 | 1440 | 0..5760 | loader, dfm, corpus, preset |
| 13 | Time stereo offset | -512..512 | 0 | -359..336 | loader, dfm, corpus, preset |
| 14, 15, 16 | Feedback filter cutoff, resonance, gain | as 6, 7, 8 | | | loader, dfm, corpus |
| 17 | Output panning | -128..128 | 0 | -14..14 | loader, dfm, corpus, preset |
| 18 | Output volume | -160..160 | 128 | 87..128 | loader, dfm, corpus, preset |
| 19 | To next delay | -160..160 | 0 | 0..160 | loader, dfm, corpus, preset |
| 132 | 2x oversampling (`OversampleBtn`) | 0, 1 | 0 | 0 (x63), 1 (x2) | loader, dfm, corpus |

A unit's 32 bytes, from byte `536 + 32 u`:

| byte | type | field | values | corpus, over the 119 enabled units | confidence |
|---|---|---|---|---|---|
| 0..3 | i32 | input filter mode (`InFModeSelect`) | 0 off, 1..7 | 0 (x81), 1 (x9), 2, 4 (x24), 5, 7 (x3) | loader, corpus |
| 4..7 | i32 | input filter type (`InFTypeSelect`): stages - 1 | 0..7 | 0 (x117), 2 (x2) | loader, corpus, inferred |
| 8..15 | i32 x2 | feedback filter mode and type | | mode 0 (x62), 1 (x32), 2 (x13), 3, 4 (x7), 5 (x3), 7 | loader, corpus |
| 16 | u8 | input filter after the delay (`InFPostBtn`) | 0, 1 | 1 (x10) | loader, corpus |
| 17..20 | i32 | feedback mode (`DelModeSelect`) | 0 off, 1 normal, 2 inverted, 3 ping-pong | 1 (x37), 2 (x5), 3 (x77) | loader, dfm, corpus |
| 21..24 | i32 | grain division | 0..16 | 0 (x110), 1..13 | loader, dfm, corpus |
| 25..28 | i32 | grain shape | -128..128 | 0 (x112) | loader, dfm, corpus |
| 29 | u8 | unit enabled | 0, 1 | 119 of 520 | loader, corpus |
| 30 | u8 | first echo left alone (`PostFirstBtn`) | 0, 1 | 1 (x20) | loader, corpus |
| 31 | u8 | tempo-based time (`DelTempoBtn`) | 0, 1 | 1 (x96) | loader, corpus |

48 of the 65 states have one unit on, the busiest has all eight.

## 2. Parameter semantics (`ProcessParam` @ 0x006e0000)

`REC_UpdateValue` stores the integer. `knob(p) = (11^p - 1) / 10`, and exactly
1 at `p = 1`, is the curve FL's gain knobs share (`exp(p * ln 11)` with the
`ln 11` at `0x74446c` worked out at startup, float 0.1 @`0x721280`, 1/128
@`0x72128c`).

```
0  Dry:        dry  = knob(v / 128)                              -> this+0x21bc
1  Wet:        wet  = knob(v / 128)                              -> this+0x21b8; every unit: out volume = outGain * wet, dirty
2  Input:      inp  = knob(v / 128)                              -> this+0x21c0; every unit: in volume = inGain * inp, dirty
3  Feedback:   fb   = v / 128                                    -> this+0x21c4
4  In pan:     U.input.pan = v / 128; dirty
5  In volume:  U.inGain = sign(v) * knob(|v| / 128); U.input.volume = inGain * inp; dirty
6..8           input filter: FUN_006df830(u, 2)
9  FB pan:     U+0x378 = v / 128
10, 11         U+0x37c = p10 / 128;  U+0x380 = p11 / 128;  if separation < 0: volume *= 1 + separation / 2
12 Time:       FUN_006df830(u, 9)                                // time and grain
13 Offset:     U+0x368 = (1001^(|v| / 512) - 1) * 50;  FUN_006df830(u, 1)
14..16         feedback filter: FUN_006df830(u, 4)
17 Out pan:    U.output.pan = v / 128; dirty
18 Out volume: U.outGain = (no unit soloed, or this one) ? sign(v) * knob(|v| / 128) : 0; U.output.volume = outGain * wet; dirty
19 To next:    U+0x64 = sign(v) * knob(|v| / 128)
```

`ProcessEvent(FPE_Tempo)` stores the tempo and reruns `FUN_006df830(u, 9)` for
every unit.

### Time (`FUN_006df830`, bit 0)

`SR` is the host rate times the oversampling factor.

```
late   = round(U+0x368 * SR / 100000) frames                    // 2.3 ms at 128, 15.3 ms at 256, 0.5 s at 512
frames = (float)(p12 * SR)
frames = tempoBased ? frames * 0.03125 / tempo : frames / 3840  // 1920 to a beat, or 3840 to a second
D      = max(1, round(frames))
if not enabled or feedback mode == 0:  free the line
else:  line.resize(blockSize * factor + late, D)                // FUN_006d8f30: only when late + D changed, and then the line is cleared
       (lateL, lateR) = p13 > 0 ? (0, late) : (late, 0)
```

The default 1440 is three quarters of a beat, three steps. Up to version 15
the wheel counted in tens.

### Grain (`FUN_006df830`, bit 3)

```
if division < 1 or not enabled: no grain
else:  period = frames / division;  len = round(period);  window[len] = FUN_006de9a0(len, shape / 128)
```

The window (`FUN_006de9a0`) is a raised cosine whose two halves are measured
separately, `h = (len - 1) div 2`:

```
angle(k) = k < h ? k * pi / h : pi + (k - h) * pi / (len - 1 - h)
w(k)     = 0.5 - 0.5 cos(angle(k))
shape != 0:  w = 1 - (exp((1 - w) * a) - 1) * b   or   (exp(w * a) - 1) * b      // a, b and which one from FUN_005db7d0(10001^|shape|), not read
```

### Gains

A gain block (`FUN_006e11f0`) turns pan and volume into four gains
`[LL, RR, RL, LR]` with `FUN_005dbbb0`, and only when its dirty flag is set,
at the top of the next `Eff_Render`. The feedback's four come from
`FUN_005dbd30(pan, volume * fb, separation)` on every block. Both are the
pan, volume and separation stage Love Philter and Convolver have
(`parts::stereo_gains`): constant power pan that is `volume` on both sides in
the middle, half of what a side loses fed from the other, and a separation
that is `(5^s - 1) / 4` of the way to mono or, under 0, takes the other side
away.

Every gain in use glides to its target over the block with `FUN_005e1220`
(`parts::glide`): a straight line, held to `0.001 * 44100 / SR` a frame, the
cap FL's `TAudioRenderer` works out (host object +0x34).

## 3. Signal chain (`Eff_Render` @ 0x006e1380)

Kernels are reached through a table filled at startup (`0x5e30e9..0x5e3488`):
scale (`0x5e08d0`), four gains with a ramp (`0x5e0290`), the same added
(`0x5e0440`), add with a gain a side (`0x5dfb10`), multiply by a mono table
(`0x5e2090`), swap the sides (`0x5e0d40`), zero what is under 2^-24
(`0x5df750`).

```
if oversampling: in = upsample(in)                                       // FUN_006db930
wet = in * dry
send = 0
for each unit, in order:
    if not enabled: send = 0; continue
    x = inputGains(in)                                                    // glided
    if send != 0: x += previous * send                                    // the last enabled unit's x
    if filter position == 0: x = inputFilter(x)                           // FUN_006e1320, a kernel a stage
    if the unit has a line:
        line[write + lateL].L += x.L;  line[write + lateR].R += x.R       // FUN_006d9400, kernel FUN_006d93e0 adds
        target = feedbackGains;  if mode == 3: (LL, RR, RL, LR) = (LR, RL, RR, LL)
        in chunks of at most D frames:                                    // FUN_006d9090
            y = line[read ..]
            if first echo left alone: x = y
            if feedback filter on: y = feedbackFilter(y)                  // in place in the line
            y = glidedFeedbackGains(y);  zero what is under 2^-24
            if not first echo left alone: x = y
            line[write ..] += y;  line[read ..] = 0;  both move on        // FUN_006d9870 -> FUN_006d9810
            if mode == 2: swap the sides of x
    if grain: x *= window, from where the phase is                        // phase wraps at `period`, the table at `len`
    if filter position == 1: x = inputFilter(x)
    wet += outputGains(x)                                                 // glided
    previous = x;  send = U.toNext
out = oversampling ? downsample(wet) : wet                                // FUN_006db9d0
```

What that means in practice:

- The line is a ring that is read at one place and added to at `D` frames
  ahead of it, by the input and by the feedback alike. A frame is cleared
  when it has been read, so what is read is the input of `D` frames ago plus
  what was fed back then.
- "Process / leave first echo" decides where the unit's output is tapped. Left
  alone, it is what leaves the line. Processed, which is the default, it is
  what goes back in: the first echo already has the feedback's filter and its
  gain. A unit with its feedback volume at 0 and the first echo processed is
  silent.
- The default feedback volume of 64 is a gain of 0.5, linear. The plugin's
  Feedback wheel multiplies it, also linearly.
- "Inverted" swaps the sides of the unit's output and leaves the line alone.
  "Ping-pong" swaps the feedback's straight and crossed gains, so each side
  feeds the other. "Off" removes the line: the unit's output is its input.
- "To next" sends a unit's output, before its output gains, into the next
  enabled unit beside the plugin's input. A disabled unit between two breaks
  the chain.
- After a state is loaded every gain in use is 0 and its target is worked out
  by the first block, so the units fade in over about a millisecond a
  thousandth of gain: 23 ms to unity at 44.1 kHz. The dry path has no glide.
  A flush (`FUN_006dfe30` -> `FUN_006dfd00`) clears the lines, restarts the
  grains and puts the gains in use on their targets.

### 3.1 The filters (`FUN_006debc0`, `FUN_005dc180`, `FUN_006e1320`)

A filter is `0x178` bytes: three stages of `0x78` (kind `+0`, the state's
eight doubles at `+0x10`, `b0 b1 b2 a1 a2` at `+0x50..+0x70`), then `+0x168`
the width spread, `+0x16c` the stage spread, `+0x170` the mode and `+0x174`
the type, which is the stage count less one. Where they run (`Eff_Render`):
the input filter at `+0x358` (`-1` when its mode is 0, else the POST byte:
0 before the line, 1 after the grain); the feedback filter on each chunk read
from the line when its mode at `+0x350` is above 0, after the first echo is
taken and before the feedback gains.

`FUN_006debc0(band, first wheel)`: `c, r = wheel / 65536` (cutoff and
resonance), `kind = [0, 1, 2, 3, 4, 6, 7, 8][mode]`, `step = renderer+0x20
(44100 / SR) * 2 pi`. Every curve is `float(Exp(double(float(x)) * Ln(b)) -
1)` (`curve(x, b)` below). Constants from `0x6df748..0x6df820`.

```
LP (1):    fc = c > 0 ? curve(c, 1501) * 0.00066466664 : 0;   offset 0.003
HP (4):    fc = c > 0 ? min(curve(c, 1501) * 0.00039993334, 0.6001) : 0;   offset 0.0001
           Q  = r <= 0 ? 0.1 : r < 1 ? curve(r, 2001) * 0.02495 + 0.1 : 50;  stages > 1: Q = Power(Q, spread)
           alpha = sin * (0.5 / Q)
BP, notch: fc as HP but held under 1.0001;  BW = r <= 0 ? 0.01 : r < 1 ? curve(r, 101) * 0.1199 + 0.01 : 12
           alpha = sin * Sinh(BW * width * w / sin)
peak (7):  fc as BP;  q = 1 - r <= 0 ? 0.1 : 1 - r < 1 ? curve(1 - r, 2001) * 0.04995 + 0.1 : 100
           A = float(Exp(double(float(gain) / 128) * Ln(1.1111111)) - 1) * 8.1 + 0.1;  stages > 1: A = Power(A, spread)
           alpha = sin / (q * 2)
shelves:   fc as BP;  S = 1 - r <= 0 ? 0.1 : 1 - r < 1 ? curve(1 - r, 3) * 0.45 + 0.1 : 1;  A as peak
           FUN_005dc180 gets sqrt(A) * sqrt((A + 1/A)(1/S - 1) + 2) for its alpha
w = clamp((fc + offset) * step, 0, 2.8495171);  SinCos(w) (FUN_005d1750)
every stage, last first: kind, FUN_005dc180(stage, A, alpha, sin, cos)
```

So a cutoff counts in parts of 44.1 kHz; the gain wheel is an amplitude of
0.1 at 0, 1 at 128 and 2 at 256 (-40 to +12 dB at a peak's centre).

`FUN_006dfc10`, on a flush (`FUN_006dfd00` bits 2 and 4): the stages' states
to 0, `+0x16c = float(1 / n)` and `+0x168 = float(n)`, or `1 / float(n)` when
stage 0 is a notch, for `n = type + 1`. A state load designs every filter
(`FUN_006dfbb0`), flushes (`FUN_006dfe30`) and then sets each wheel, so the
designs it keeps have the spreads of their own stage count. A filter picked in
the window is designed (`FUN_006df830`, 6) before the flush (`FUN_006dfd00`, 6)
and so keeps the old spread until a wheel moves.

`FUN_005dc180(stage, A, alpha, sin, cos)`, `n = 1 / (alpha + 1)` but where
noted, is the cookbook normalised to `a0 = 1`:

```
1 LP     a1 = n cos (-2), a2 = n (1 - alpha), b1 = n (1 - cos), b0 = b2 = b1 * 0.5
2 BP     a1, a2 as LP; b0 = n alpha, b1 = 0, b2 = n (-alpha)
3 notch  a1, a2 as LP; b0 = b2 = n, b1 = a1
4 HP     a1, a2 as LP; b1 = n (-(cos + 1)), b0 = b2 = -b1 * 0.5
5 allpass  a1, a2 as LP; b0 = a2, b1 = a1, b2 = 1                       (no mode picks it)
6 low shelf   t = (A - 1) cos, s = alpha sin, n = 1 / (A + 1 + t + s)
         b0 = n A ((A + 1 - t) + s), b1 = n A 2 ((A - 1) - (A + 1) cos), b2 = n A ((A + 1 - t) - s)
         a1 = n ((A - 1) + (A + 1) cos) (-2), a2 = n ((A + 1 + t) - s)
7 peak   n = 1 / (alpha / A + 1); a1 = b1 = n cos (-2), a2 = n (1 - alpha / A), b0 = n (alpha A + 1), b2 = n (1 - alpha A)
8 high shelf  n = 1 / ((A + 1 - t) + s)
         b0 = n A ((A + 1 + t) + s), b1 = n A (-2) ((A - 1) + (A + 1) cos), b2 = n A ((A + 1 + t) - s)
         a1 = n ((A - 1) - (A + 1) cos) 2, a2 = n ((A + 1 - t) - s)
```

`FUN_006e1320` runs stage 0 to `type` in place on the block with the kernel
the startup table (`FUN_005df170`) puts at `0x7443a8`: `FUN_005dc8c0` is a
"Not implemented in i64/ARM" stub, and on x64 it is `FUN_005dc9a0`, direct
form I in doubles on interleaved stereo, `y = x b0 + (((x1 b1 - y2 a2) + x2
b2) - y1 a1)`, the output cast to float, then the eight state doubles
flushed (`DAT_007445c8`). It is `dsp::biquad::Biquad64` to the operation.

### 3.2 The oversampler (`FUN_006dff00`, `FUN_006db930`, `FUN_006db9d0`)

`FUN_006dff00` runs on a state load and on Dispatcher 4 (the rate): both
oversamplers are rebuilt (`FUN_006db830`) with `param[132]` stages, or 1
when the host renders with its HQ flag (`0x21e8`), all zeroed
(`FUN_006db8d0`); `0x21d4 = 1 << stages` (`FUN_006db8c0`); and the
renderer is told the rate `0x21d0 * 0x21d4` (slot 0 of `this+0xcc`). So
every unit works at the doubled rate: its times and stereo offset in
frames (`FUN_006df830` reads the renderer's `+8`), its grain, its filters
(`renderer+0x20 = 44100 / rate`) and its gain glides (`+0x34`).

`Eff_Render` goes up when `param[132] > 0` (`FUN_006db930`) and runs the
units on `n * 0x21d4` frames, then goes down into `Dest` (`FUN_006db9d0`).
The class is WaveShaper's to the instruction, at other addresses: the SSE2
kernels `FUN_006db400` (up) and `FUN_006db5d0` (down), picked by
`FUN_006dbaf0` over the stubs `FUN_006db390` / `FUN_006db560`; the 13
doubles at `0x00733fa0`, copied twice over into `0x00746578` by
`FUN_006dba60`; and after each kernel the flush at `PTR_DAT_007350e8`
(`DAT_007445c8`, `FUN_005e27a0`) with a count of 12, which zeroes an
allpass output under 2^-24. `docs/plugins/fruity-waveshaper.md` section
3.1 has the filter, and `crates/dsp/src/halfband.rs` rebuilds it, checked
to the bit against these kernels run under unicorn
(`tools/plugstate/halfband_emu.py`). The plugin reports no latency.

## 4. What stays unknown
- The grain's shape curve, `FUN_005db7d0`.
- The delay line's other nine routines (`FUN_006d9550` .. `FUN_006d9c80`, and
  their SSE twins from `FUN_006da1d0`). `Eff_Render` calls only the tenth,
  the advance at +0x70.
- `FUN_006d9810` is read as moving the frame at the read place to the write
  place and clearing it. Ghidra orders the left sample's load after its
  clear, which would feed back the right side only; the two sides are taken
  to be handled alike.

## Implementation notes (crates/plugins/src/delay_bank.rs)

- Built as read: the four plugin gains, a unit's input, output, feedback and
  "to next" gains, the line with its two write offsets, the four feedback
  modes, the first echo switch, the time in seconds or beats, the grain with
  a shape of 0, the fade in after a state and the flush.
- The filters are `Filter` (section 3.1), run a frame at a time, which is the
  same as FL's blocks and chunks for a recursive filter. FL's kernel flushes
  the eight state doubles after each call with `DAT_007445c8`, which zeroes
  what is under 2^-24 (`FUN_005e27a0`); here only subnormals are zeroed, at
  the end of each block. That differs below -144 dB. A type above 2 is held
  at three stages, all the filter has room for. Rust's `ln`, `exp`, `sinh`
  and `sin_cos` stand in for Delphi's and may be an ulp off.
- The 2x oversampling is built as section 3.2 has it, a frame at a time
  (`dsp::halfband::Oversampler`): each stage is a recursion of its own, so
  that is FL's block-at-a-time result to the bit. A switch other than 0 or
  1 is turned down.
- Not built, and `fidelity` says so: the grain's shape (the window stays a
  raised cosine).
- The gains glide over the block the host hands over. FL glides the feedback
  gains over each chunk of the line instead, which is the same unless they
  are still moving.
- The line is `D + late + 1` frames, not FL's `block + late + D`. What is read
  is the same.
- A state is range-checked, which FL does not do. Solo is not saved and not
  built.
