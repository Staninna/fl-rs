# Vintage Phaser: state, parameters, signal chain

Recovered from `Plugins/Fruity/Effects/Vintage Phaser/` of FL Studio
26.1.7: `Vintage Phaser_x64.dll`, a Delphi shell (image base 0x400000), and
`engine_x64.dll` beside it, an MSVC C++ VST 2 plugin (image base
0x180000000, 313 kB, exports `VSTPluginMain` 0x18001e270, 36 parameters,
`initialDelay` 44) that does all the sound. It is Vintage Chorus's design
(`docs/plugins/vintage-chorus.md`) on a much larger engine. Ghidra 11.4.2
headless, every engine function decompiled, listings with the constants
each instruction loads (`tools/ghidra/DisasmAddrs.java`), and the engine run
under unicorn (`tools/plugstate/vst_emu.py`) as the reference the port is
checked against. Corpus: one state, Purps On The Beat - Stroll Up
("Lowgrounder", 149 bytes, version 3); 16 presets of version 3 and four
plugin-database entries of version 1.

Confidence tags: `loader`, `dfm`, `asm`, `emu` (the port gives the DLL's
output to the bit under emulation), `inferred`.

## How the shell drives the engine

`CreatePlugInstance` @ `0x006e9f90` -> constructor `FUN_006eac30`, class
`0x006e9df8`.

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006ea1e0 |
| 3 | SaveRestoreState | 0x006ea8e0 |
| 6 | ProcessParam | 0x006eb1a0 |
| 7 | Eff_Render | 0x006edbf0 |

The shell loads `engine_x64.dll` through FL's Delphi VST host class (object
at `this+0x178`). Eff_Render splits the interleaved block into two planes,
has the host run `processReplacing` (VST object +200), and joins them.
ProcessParam (`loader`) keeps the value at `this+0x180+4i` and hands the
VST's `setParameter(i, v)`

```
v = f32(clamp((value - Min) * scale, 0, 1))        // scale = control+0x3cc = 1 / (Max - Min), f64
```

with no other check: a value under a wheel's Min (FL's presets have manual
frequencies of 0) reaches the engine as 0.

Dispatcher: 4 (rate) -> the host's `setSampleRate` and `FUN_006eab70`,
which grows the plane buffers and reports the VST's `initialDelay` (the host
object +0x75c, from `AEffect+0x50`) to FL with host dispatcher 0x1e
(`FUN_006ede60`): 44 frames. 11 and 2 -> the host's reset, which reaches the
engine's suspend and resume; they clear nothing (`emu`). 12, 13, 14 -> the
host's `VstTimeInfo`.

The constructor fills the parameters from the engine: `value = Min +
Round(getParameter(i) * (Max - Min))`, then sends them all (`loader`).

## 1. State layout (event 213)

`SaveRestoreState` (`FUN_006ea8e0`, `loader`): save writes `i32 3`, the 36
parameters as i32 (`this+0x180`, 0x90 bytes) and one byte, the state of the
editor's "advanced" panel (the control at `editor+0x7e8`). Restore:

- version 4 or more: nothing happens.
- version 3: the 0x90 bytes and the byte (149 bytes).
- version 2: the 0x90 bytes (148 bytes).
- version 1: 32 i32 (132 bytes) to the parameters the table at `0x00742a30`
  names, `[0 1 2 3 4 6 8 11 7 10 21 22 23 5 12 13 14 15 16 17 18 19 20 25
  26 27 28 29 30 31 32 9]`; parameter 24 (noise gate) from its control
  (+0x3b4, its default, 0 `inferred`); 33..35 set to 0 (engine V1, no
  drive, feedback upright).

Then GetValue / UpdateValue (flags 2 and 0x11) over the 36 parameters, as in
the other shells.

Control `Tag` = parameter index (`dfm`, `tools/plugstate/dfm.py`). Check
boxes, switches and buttons have no Min/Max in the DFM; 0..1 is `inferred`
from the engine's `>= 0.5` tests and every saved value. Defaults are what
the shell's constructor makes of the engine's values.

| idx | DFM object | range | default | engine |
|---|---|---|---|---|
| 0 | FreqMinSlider | 125..2048000 | 896070 | 0 |
| 1 | FreqMaxSlider | 125..2048000 | 1588996 | 1 |
| 2 | FeedbackSlider | 0..6000 | 3000 | 2 |
| 3 | ManualCheckbox | 0..1 | 0 | 3 |
| 4 | ManualFrequencySlider | 125..2048000 | 125 | 4 |
| 5 | HqCheckBox | 0..1 | 0 | 5 |
| 6 | LFOSpeedSlider | 1..245760 | 1 | 6 |
| 7 | LFOSpeedHostSyncButton | 0..1 | 0 | 7 |
| 8 | LFOWaveFormSelector (triangle, sine) | 0..1 | 0 | 8 |
| 9 | ModTensionWheel | 0..6000 | 0 | 9 |
| 10 | LFOStartPhaseWheel | 0..6000 | 0 | 10 |
| 11 | LFOLeftRightPhaseDifferenceWheel | 0..6000 | 0 | 11 |
| 12 | DelayActiveCheckBox | 0..1 | 0 | 12 |
| 13 | DelayKeepPitchCheckBox | 0..1 | 0 | 13 |
| 14 | DelayMixWheel | 0..6000 | 0 | 14 |
| 15 | DelayStereoWheel | 0..6000 | 3000 | 15 |
| 16 | DelayTimeWheel | 1..768 | 1 | 16 |
| 17 | DelayTimeBPMSyncButton | 0..1 | 0 | 17 |
| 18 | DelayFeedbackWheel | 0..6000 | 0 | 18 |
| 19 | DelayLpWheel | 3200..204800 | 3200 | 19 |
| 20 | DelayHpWheel | 2000..204800 | 2000 | 20 |
| 21 | MixWheel | 0..100 | 50 | 21 |
| 22 | MonoInputCheckbox | 0..1 | 0 | **23** |
| 23 | NoiseWheel | 0..6000 | 0 | **22** |
| 24 | NoiseGateCheckbox | 0..1 | 0 | 24 |
| 25 | ColorLowShelfGainWheel | 0..6000 | 0 | 25 |
| 26 | ColorLowShelfFrequencyWheel | 5000..650000 | 5000 | 26 |
| 27 | InputHPFreqWheel | 2000..80000 | 2000 | 27 |
| 28 | FeedbackHPFreq1Wheel | 2000..80000 | 2000 | 28 |
| 29 | RateFreq1Wheel | 25..150000 | 25 | 29 |
| 30 | RateFreq2Wheel | 25..150000 | 150000 | 30 |
| 31 | LFOMaxFreqDepthScaleWheel | 0..6000 | 6000 | 31 |
| 32 | LFOMaxFreqDepthPullDownWheel | 0..6000 | 0 | 32 |
| 33 | EngineSwitch (V1, V2) | 0..1 | 1 | 33 |
| 34 | DriveWheel | 5000..650000 | 5000 | 34 |
| 35 | InvertFeedbackSwitch | 0..1 | 0 | 35 |

The VST's `setParameter` (`FUN_18001dc80`) swaps 22 and 23 on the way in.

Stroll Up's state: min 716925, max 2048000, feedback 1320, manual off,
manual frequency 1182845, HQ off, speed 1, no sync, sine, tension 3030,
phases 0, delay off (keep pitch on, stereo 864, time 1), mix 100, noise
off, colour 2145 at 77200, input HP 58100, feedback HP 80000, rates 66220
and 25, depth 0 / 0, engine V2, drive 5000, feedback upright.

## 2. Parameters (`FUN_180018cd0`, behind a critical section in `FUN_180019450`)

`E` is the DSP object at `TestPlug+0xf0` (constructor `FUN_18001a250`). The
engine keeps every value at `E+0x218+4i`. f32 unless marked. `powf`
`FUN_180023b50`, `tanf` `FUN_180025e30`, `logf` `FUN_1800220b0`, `sinf`
`FUN_180024f90`; f64 `pow` `FUN_1800224b0`, `exp` `FUN_1800214a0`, `log`
`FUN_180021b80`, `sin` `FUN_180024a20`, `tan` `FUN_180025690`, `sincos`
`FUN_1800206e0`. The four-wide constants at `0x18003bd00..0x18003bddc` are
written by static initialisers (`0x180028280..0x1800282f0`).

```
0, 1   FUN_180018a00: lo = powf(2, v0 * 14) * 1.25, hi = powf(2, v1 * 14) * 1.25, swapped if hi < lo
       min (E+0x1c8) = lo;  octaves (E+0x1dc) = f32(log(f64(hi / lo)) * 1.4426950408889634)
2, 35  feedback (E+0x1f0) = v2 * 0.9, negated when v35 >= 0.5
3      LFO on (E+0xd0) = v < 0.5
4      manual (E+0x1c4) = powf(2, v * 14) * 1.25                     // 1.25 Hz .. 20480 Hz
5      factor (E+0x194) = v >= 0.5 ? 8 : 4; a change runs the rate setup again
6, 29, 30  FUN_180018b80: r(v) = (v^2)^2 * 149.975 + 0.025;  hi, lo = max, min (r(v30), r(v29))
       Hz = powf(2, f32(log(f64(hi / lo)) * log2 e) * v6) * lo;   step (E+0x1e8) = f64(Hz) / rate
       beats (E+0xc8) = trunc((powf(2, (1 - v6) * 7) - 1) * 48.37008 + 1.1) * 0.005208333333333333
       when synced: resync off
7      resync off; sync (E+0xc4) = v >= 0.5
8      sine (E+0xc0) = v >= 0.5
9      FUN_18001cf50(E+0x3d0, (v + 1) * 0.5): x = 2t - 1;  k = f32(exp(f64(|x|) * 8.75573753930647) - 1) * (x > 0 ? 0.1 : -0.1)
       if k changed: k, and when k != 0: 1 / |k|, logf(|k| + 1)
12     delay on (E+0x2b0);  13 keep pitch (v > 0.5, resets the fade);  14 mix;  15 stereo
16     delay time, marks the time dirty;  17 delay sync;  18 delay feedback
19     delay low-pass at f32(pow(2, f64(v * 6)) * 320):   t = tanf(f * pi_rate), n = 1 / (t + 1):  b0 = b1 = n t, a1 = (1 - t) n
20     delay high-pass at f32(pow(2, f64(v * 10)) * 20):  b0 = n, b1 = -n, a1 = -(n (t - 1))
21     mix (E+0x26c)
22     noise (E+0x160) = v == 0 ? 0 : powf(10, v * 2.5 - 4.5) * 0.35;  gate threshold (E+0x170) = noise * 8
23     mono (E+0x1c0) = v >= 0.5;    24  gate (E+0x190) = v >= 0.5
25, 26 colour (E+0x80): low shelf at f64(v26 * 600 + 50) Hz, Q 0.5, f64(v25 * 30) dB
27     input (E+0x0): first-order high-pass at (v * v) * (v * 195) + 5 Hz
28     feedback high-pass at (v * v) * (v * 195) + 5 Hz: V1's biquad at E+0x40 (at the higher rate) and V2's Ghp
33, 34 V2 (E+0x400) = v33 >= 0.5;  drive target (E+0x404) = V2 ? powf(10, v34 * 1.5 - 0.75) : 1
```

The biquad designer `FUN_18001b1e0(type, f, rate, q, dB)` is RBJ's in f64,
everything over `a0`, then rounded to f32 (`FUN_18001b780`). Type 11: `t =
tan(f pi / rate)`, `a0 = t + 1, a1 = t - 1, b0 = 1, b1 = -1`. Type 4: `A =
pow(10, dB * 0.025)`, `w = f * 2pi / rate`, `root = sqrt(A) * (1 / q) *
sin w`, `b0 = (root + (A+1 - (A-1) cos)) A`, `b1 = (A-1 - (A+1) cos)(A+A)`,
`b2 = ((A+1 - (A-1) cos) - root) A`, `a0 = root + (A+1 + (A-1) cos)`, `a1 =
(A-1)(-2) - ((A+1) cos + (A+1) cos)`, `a2 = (A+1 + (A-1) cos) - root`.
A section runs `y = ((a1 y1 + a2 y2) + b2 x2) + (b0 x + b1 x1)` with the
feedback signs folded in.

Rate setup `FUN_180019560` (`setSampleRate` `FUN_18001d020`, and parameter
5): `1 / rate` (f64), the drive glide `f32(1 - pow(0.050000000000000044, 20
/ rate))`, the delay's (`FUN_18001cab0`: its rate, the fade step `1 /
f32(rate * 0.1)`, `pi_rate = f32(1 / rate) * pi`, its two one-poles again,
the glide `f32(1 - pow(0.09999999999999998, 10 / rate))`, both 65536-sample
lines emptied), every section's rate (the feedback biquad's is `rate *
factor`), V1's `1 / (rate * factor)`, V2's `pi_f32 / f32(rate * factor)`,
two new upsamplers (empty), then every parameter again. Nothing else is
cleared.

Constructor values that matter: factor 8, drive and its glide at 1, LFO on,
triangle, beats 1, last position -1, gate gain 0 with threshold 0.0025 and
hold 440, fall 0x3f7fe931 (0.99965 a sample), noise seeds `g` and `g +
0x1099e` with `g` a global from 0x3a916, growing by 0xb66ad for every
engine the process makes.

## 3. Signal chain (`processReplacing` `FUN_18001d040`, block `FUN_180018210`)

Per block: the position is `ppqPos` while the song plays, else -1 (`VstTimeInfo`
flags 0x3e02); the delay takes the tempo. When the last position was under
0 and this one is not, resync is set. Synced: when resync and position >=
0, `phase = frac(position / beats + f64(start))`, and always `step = tempo *
(1/60) / beats * (1 / rate)` (f64). Resync stays set until parameter 6, 7,
29 or 30 clears it.

Per frame (`emu`, all of it):

```
LFO (FUN_180001190), when on:
    p = f32(phase);  l = tense(shape(p));  r = tense(shape(frac(p + v11)))
    shape: sine (sinf(p * 6.2831855) + 1) * 0.5; triangle 2p + 0.5, 1 - (((p - .25) + p) - .25), ((p - .75) + p) - .75
    tense (FUN_18001ceb0): k = 0: p;  k > 0: f32(1 - (exp(f64((1 - p) logk)) - 1) f64(1/|k|));  k < 0: f32((exp(f64(logk p)) - 1) f64(1/|k|))
    depth = (1 - v6)(1 - v31) + v31;   offset = (1 - depth) - ((v6 v32) v6)(v6 v6)
    phase = phase + step - its whole part (f64)
    Hz[side] = 2^((depth * lfo + offset) * octaves) * min    // 2^x: cubic ((f c3 + c2) f + c1) f + c0 over the fraction, times 2^whole
  when off: Hz = manual for both sides, LFO values 0
  V1: c[side] = (t - 1) / (t + 1),  t = tan(f64(Hz) * pi * (1 / (rate * factor)))
  V2 (object at E+0x410, lanes L R L R): x = Hz * pi_f32 / rate'; t = ((2.4716885 - x^2 0.18975969) x) / (2.467401 - x^2)
      G = t / (t + 1);  a = 2G - 1;  b = 1 - G;  k = 1 / (1 - (a^2)^2 fb);  hp_rest = 1 - Ghp
if mono: L = R = (R + L) * 0.5
each side: x = input high-pass(colour shelf(in))
g = drive glide;  drive glide = (target - g) * glide + g
each side: the upsampler takes g * x and gives `factor` samples
each of the `factor` samples (FUN_180001660):
  V1: y = biquad_fbhp(fb * last + up) -> four allpasses in f64, s1..s5 a side:
        y1 = (d - s2) c + s1, s1 = d;  y2 = (y1 - s3) c + s2, s2 = y1;  ... s5 = y4;  last = f32(y4)
  V2 (FUN_1800176b0): S = ((((2s0 b) a + 2s1 b) a + 2s2 b) a + 2s3 b) + (-hp) hp_rest
        u = (S fb + up) k - s0, then four stages: t = tanh(clamp(u, -5, 5)), v = G t, low = s + v, s = low + v,
        y = low - b t, u = y - (next s);  hp = (hp + d) + d,  d = (y - hp) Ghp
        tanh(u) = (((u2 + 378) u2 + 17325) u2 + 135135) u / (((u2 28 + 3150) u2 + 62370) u2 + 135135)
  sample = mix * y + (1 - mix) * up
V2 (FUN_180001940): every sample through the halfband at E+0x3e0 (FUN_180016ed0); at 8x,
  samples 0 and 4 through the one at E+0x3e8
out[side] = (1 / drive glide) * sample 0
noise, when on: each side a low shelf at 2^(3.215 lfo) * 280 Hz (FUN_18001aad0), Q 0.5, 18 dB, redesigned every frame;
  n = i32(seed = seed * 0x0bb38435 + 0x3619636b) * 2^-31;  out += noise * shelf(n)
gate, when on: open (count 440, gain 1) when |outL|, |outR|, |hissL + inL| or |hissR + inR| >= threshold;
  else count down, then gain *= 0.99965;  out *= gain
delay, when on (FUN_18001c200)
```

The upsampler (`FUN_18001be70`, `FUN_18001bd20`): a kernel of `88 factor +
1` taps, `h[i] = f32(max(sin(i / (N - 1) pi), 0) * sinc(i / factor - 44))`
(`sinc(t) = sin(t pi) / (t pi)`, 1 under 1e-6), added scaled into a ring of
`90 factor` samples at the read position; the `factor` samples after the
kernel's end are zeroed, and the next `factor` from the position come out.
The stages run in place in the ring. Its centre, 44 samples on, is the
latency the VST reports. V1 decimates by taking sample 0, with no filter.

The halfbands are Laurent de Soras's polyphase IIR form, both sides on two
paths in one four-wide vector: `E+0x3e0` six second-order allpasses in
`z^-1` a path, `y = ((x1 - y1) c0 + x2) + (x - y2) c1`, out `(pathB + pathA
a sample back) / 2`, coefficients at `0x180031880..` and `0x1800316d0..`;
`E+0x3e8` six first-order ones in `z^-2`, `y = (x - y2) c + x2`, out
`(pathA + pathB a sample back) / 2`, coefficients at `0x1800316b0..`.

The delay (object at `E+0x2b8`): when its time is dirty, the target is
`f32(rate) * time`, or synced `(1 - trunc(time * -767)) * (0.3125 /
f32(tempo)) * f32(rate)` samples. Each line takes `fb * fed + in`. Without
keep pitch the delay glides to the target and is read linearly between `w -
i` and `w - i - 1`; with it, a new target starts a fade of
`1 / (0.1 rate)` a sample between two whole taps. The read goes through a
one-pole low-pass, then a high-pass, into `fed`. Out: `L = mix wetL gA + gB
inL`, `R = mix wetR gB + gA inR`, `gA = (2.5855727 - (1 - s) 1.1711457)(1 -
s)`, `gB = (2.5855727 - s 1.1711457) s` for stereo `s` (1 at the middle).

## Checked against the DLL

`tools/plugstate/vst_emu.py` runs the engine itself under unicorn. The port
gives its output to the bit (glibc's libm on Linux) for Stroll Up's state
and 30 other settings: both engines, both factors, manual, both shapes,
both tensions, drive and inverted feedback, colour and input filters, mono,
noise, gate, the delay with and without keep pitch and synced, the LFO
synced with the song playing, 44.1, 48 and 96 kHz, blocks of 37 to 256. The
one difference is the sine LFO, an ULP at times: the DLL's `sinf` is not
glibc's. `stroll_up_s_phaser_is_the_dll_s` keeps 256 of those samples.

## Implementation notes (`crates/plugins/src/vintage_phaser.rs`)

- The engine as above, with Rust's `powf`, `tan`, `ln`, `exp`, `sin` and
  `cos` for the CRT's.
- The halfbands keep their four lanes, the ladder its two (lanes 2 and 3
  are copies of 0 and 1 for it).
- Both upsamplers are made at the start, for 4 and for 8, so that moving
  the HQ switch on the audio thread allocates nothing; the one switched to
  is emptied, as FL makes a new one.
- The noise seeds are those of the first engine FL loads; a project with
  more than one Vintage Phaser has other seeds in FL. `fidelity()` says so
  while the noise is on.
- Sync takes the song's place from the host's ticks at each block, as FL's
  VST host fills `ppqPos` per `processReplacing`; a block of ours need not
  be one of FL's.
- `reset()` stops the song and clears nothing else, as in FL. A new rate
  empties the upsamplers and the delay lines, as in FL.
