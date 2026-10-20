# Vintage Chorus: state, parameters, signal chain

Recovered from `Plugins/Fruity/Effects/Vintage Chorus/` of FL Studio 2026:
`Vintage Chorus_x64.dll`, a Delphi shell (image base 0x400000), and
`engine_x64.dll` beside it, an MSVC C++ VST 2 plugin (image base
0x180000000, 200 kB, exports `VSTPluginMain`, class `TestPlug`, uniqueID
`JUNO`) that does all the sound. Ghidra project `fx6` (every engine
function decompiled to one file) and capstone listings with the constants
each instruction loads. Corpus: 12 states in 5 `.flp` (Bombs Away - I'm
Awake 3, LollieVox - I'll Be Your Madonna 6, Stroll Up, Secret Agent
Rendezvous, and the render test `Vintage Chorus.flp`), all version 1, 72
bytes.

Confidence tags: `loader`, `dfm`, `corpus`, `asm`, `inferred`.

## How the shell drives the engine

`CreatePlugInstance` @ `0x006e8140` -> constructor `FUN_006e8b80`, class
`0x006e7f68`.

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006e8370 |
| 3 | SaveRestoreState | 0x006e89a0 |
| 5 | ProcessEvent | 0x006e8fc0 (tempo to the VST host's time info) |
| 6 | ProcessParam | 0x006e9010 |
| 7 | Eff_Render | 0x006ea9d0 |

The shell loads `engine_x64.dll` through FL's Delphi VST host class
(`FUN_006ea7a0`, object at `this+0x174`, built by `FUN_006dbd10` with the
host's rate). Eff_Render splits the interleaved block into two planes, has
the host run the VST's `processReplacing`, and joins them again.
ProcessParam hands each parameter to the VST's `setParameter` as

```
v = clamp((value - Min) * scale, 0, 1)        // scale = 1 / (Max - Min), f64 (VCL 0x00651f80); then f32
```

Dispatcher: 4 (rate) -> the VST's `setSampleRate`; 11 (enabled) and 2
(flush) -> the host's reset, which reaches the engine's `resume`, which
clears nothing (`FUN_180017bb0`); 12 (playing), 13 (song position) and 14
(time signature) -> the host's `VstTimeInfo`.

## 1. State layout (event 213, 72 bytes, version 1)

`SaveRestoreState` (`FUN_006e89a0`): `Write(i32 1); Write(this+0x17c,
0x44)`. Restore reads the version, does nothing when it is 2 or more,
else reads 68 bytes and runs GetValue / UpdateValue (flags 2 and 0x11)
over the 17 parameters in order.

The control `Tag` is the parameter index and the VST's parameter index
(`dfm`; the names are the VST's `getParameterName`, `FUN_18001b110`).

| byte | idx | parameter (DFM object / VST name) | range | default |
|---|---|---|---|---|
| 0..3 | | version | | 1 |
| 4..7 | 0 | `ModeSelect` / Mode: I, II, EDIT | 0..2 | 0 |
| 8..11 | 1 | `LinkChorusCheckBox` / Link Chorus1+2 | 0..1 | 0 |
| 12..15 | 2 | `LFOSpeedWheel` / LFO Speed | 1..245760 | 49152 |
| 16..19 | 3 | `LFOWaveFormSelector` / LFO Wave (triangle, sine) | 0..1 | 0 |
| 20..23 | 4 | `LFOLeftRightPhaseDifferenceWheel` / LFO L/R Phase | 0..6000 | 3000 |
| 24..27 | 5 | `Time1Wheel` / Delay Time 1 | 0..6000 | 60 |
| 28..31 | 6 | `Time2Wheel` / Delay Time 2 | 0..6000 | 3000 |
| 32..35 | 7 | `MixWheel` / Wet Dry Mix | 0..6000 | 3000 |
| 36..39 | 8 | `MonoInputCheckbox` / Mono Input | 0..1 | 0 |
| 40..43 | 9 | `WetPolarityCheckBox` / Wet Polarity | 0..1 | 0 |
| 44..47 | 10 | `FeedbackWheel` / Feedback | 0..6000 | 0 |
| 48..51 | 11 | `WetHPFrequencyWheel` / Wet HP Freq | 0..6000 | 3000 |
| 52..55 | 12 | `GainWheel` / Gain | 0..6000 | 0 |
| 56..59 | 13 | `HisWheel` / Noise | 0..6000 | 3000 |
| 60..63 | 14 | `NoiseGateActiveCheckBox` / Noise Gate | 0..1 | 0 |
| 64..67 | 15 | `LFOSpeedHostSyncCheckBox` / LFO Sync | 0..1 | 0 |
| 68..71 | 16 | `LFOStartPhaseWheel` / LFO Start Phase | 0..6000 | 0 |

The mode selector and the check boxes have no Max in the DFM; 2 and 1 are
`inferred` from the hints (I, II, EDIT) and from every corpus value. The
speed menu's tags (`TimeSetMenu`) are the speed values of note lengths
when synced.

## 2. Parameters (`setParameter` @ 0x18001b390)

`E` is the DSP object at `TestPlug+0x138` (constructor `FUN_180019770`).
All f32 unless marked. `pow` is the CRT's (`FUN_18001c600`), `tan`, `sin`,
`cos` the CRT's f64 ones (`FUN_18001ea80`, `FUN_18001dca0`,
`FUN_18001bcf0`), `sinf` `FUN_18001e210`.

```
0, 1  mode = trunc(v0 * 3 * 0.9999)                         // 0 I, 1 II, 2 edit
      mode 0, 1 and v1 >= 0.5 ("I + II"): step = 8 / SR, sine, L/R phase 0x3cea61da (0.0286),
                  center 0x3b3ac711 (2.85 ms), time 2 0x3b5cdb38 (3.37 ms), depth 0x3a08509c (0.52 ms)
      mode 0, 1 otherwise: step = (mode 0 ? 0.4 : 0.6) / SR, triangle, L/R phase 0.5,
                  center 0x3a8d8ec9 (1.08 ms), time 2 0x3bab606b (5.23 ms), depth 0x3b87fcb9 (4.15 ms)
      both: mix 0.5, polarity 1, feedback 0, high-pass at 0.5 Hz, sync off, resync off
      mode 2: setParameter(i, value[i]) for i = 2..16
2, 15 (mode 2) sync = v15 >= 0.5; resync off; step = v2^2 * 10 / SR      // 0..10 Hz
      beats = (f64) trunc(f32((pow(2, f64((1 - v2) * 7)) - 1) * 1.0078740119934082) * 48 * 6143 / 6144 + 1.1) / 48 * 0.25
3     (mode 2) sine = v >= 0.5
4     (mode 2) L/R phase = v
5     (mode 2) center = (v * 9.8 + 0.2) * 0.001;  depth = time2 - center     // seconds, 0.2..10 ms
6     (mode 2) time2  = (v * 9.8 + 0.2) * 0.001;  depth = time2 - center
7     (mode 2) mix target = v
8     mono = v >= 0.5
9     (mode 2) polarity = v < 0.5 ? 1 : -1
10    (mode 2) feedback = v * 0.63
11    (mode 2) high-pass at v^2 * 160 + 10 Hz
12    gain target = f32(pow(10, f64(v * 12 / 20)))                   // 0..12 dB
13    v == 0: noise 0, gate threshold 0
      else:   noise = f32(pow(10, f64((v * 50 - 90) / 20))) * 0.2;  gate threshold = noise * 8
14    gate = v >= 0.5
16    start phase = v
```

So "Time 1" is where the sweep starts and "Time 2" where it ends; with
Time 2 under Time 1 the depth is negative.

`setSampleRate` (`FUN_18001ab90`) runs `FUN_1800192e0` and then
`setParameter(i, getParameter(i))` for every parameter. `FUN_1800192e0`:

```
glide  = f32(1 - pow(0.09999999999999998, 1 / (SR * 0.05)))   // f64: a glide goes 90 % of the way in 50 ms
pi_sr  = f32(1 / SR) * pi_f32
high-pass at 50 Hz (FUN_180019680): t = tan(f64(f * pi_sr)); a0 = 1 / (t + 1); a1 = -a0; b1 = -(t - 1) / (t + 1)
low-pass at 7000 Hz:                 t = tan(f64(7000 * pi_sr)); a0 = a1 = t / (t + 1); b1 = -(t - 1) / (t + 1)
bucket low-pass: w = f64(43982.296875 / SR); s = f32 sin w; c = f32 cos w; alpha = s / 1.414; n = 1 / (alpha + 1)
                 b0 = b2 = (1 - c) * 0.5 * n; b1 = (1 - c) * n; a1 = 2c * n; a2 = -(1 - alpha) * n
noise counter = 16
```

Constructor values that matter (`FUN_180019770`): the rings, 32768 floats a
side, zeroed; the gliding delays at 10 ms, the mix and the gain glides at
0; gate hold 440 samples, gate fall `f32(pow(0.1, 1.5117157373598967e-4))`
(-20 dB in 6615 samples, whatever the rate); the noise seeds
`g, g + 0x1099e` with `g` a global that starts at 0x517d8 and grows by
0xb66ad for every engine the process makes; the last song position -1
and the resync flag on.

## 3. Signal chain (`processReplacing` @ 0x18001a950, frame @ 0x1800186a0)

`processReplacing` asks the host for `VstTimeInfo` (flags 0x3e02). Playing:
the position starts at `ppqPos` and grows by `tempo / 60 / SR` a frame;
stopped: -1. It sets flush-to-zero for the block. Per frame, in f32:

```
if mono: L = R = (L + R) * 0.5                                  // the input, from here on
if last position < 0 <= position: resync = on;  last position = position
if sync:
    if resync and position >= 0: phase = frac(f32(position / beats) + start)
    step = f32(tempo / 60 / beats) * (1 / SR)
right = frac(phase + L/R phase)
lfo   = sine ? sinf(p * 2 pi) * 0.5 + 0.5 : triangle(p)          // triangle: 2p + 0.5, 1.5 - 2p, 2 (p - 0.75); 0..1
delay[s] += ((depth * lfo[s] + center) - delay[s]) * glide     // seconds
phase = frac(phase + step)
per side:
    x = feedback * last[s] + in[s]
    x = high-pass(x)                                            // y = a1 x1 + a0 x + b1 y1
    x = bucket(bucket(x))                                       // y = b1 x1 + b0 x + b2 x2 + a1 y1 + a2 y2, two in a row, own states
    ring[w] = x;  w = (w + 1) & 0x7fff
    d = delay[s] * SR;  i = trunc d;  f = d - i;  at = w - i
    xm1, x0, x1, x2 = ring[at], ring[at - 1], ring[at - 2], ring[at - 3]
    c = (x1 - xm1) * 0.5;  v = x0 - x1;  wv = v + c;  a = (x2 - x0) * 0.5 + (wv + v);  b = a + wv
    last[s] = wet[s] = ((a f - b) f + c) f + x0                  // 4-point Hermite, i + f samples back from the newest
if noise > 0:
    counter += 1
    n[s] = i32(seed[s] = seed[s] * 0x0bb38435 + 0x3619636b) * 2^-31
    if counter >= 16: counter = 0; per side a peak at 6.5 kHz:
        w = f64(40840.70703125 / SR); s2 = 2 f32 sin w; c2 = f32 cos w * -2; A = 2 lfo[s] + 1; n = 1 / (s2 / A + 1)
        b0 = (s2 A + 1) n; b1 = c2 n; b2 = (1 - s2 A) n; a1 = -c2 n; a2 = -(1 - s2 / A) n
    wet[s] += noise * peak(n[s])
wet[s] = low-pass(wet[s])                                      // y = a1 x1 + a0 x + b1 y1, at 7 kHz
mix  += (mix target - mix) * glide;  gain += (gain target - gain) * glide
out[s] = wet[s] * sqrt(mix) * polarity + in[s] * sqrt(1 - mix)
if gate:
    if |outL| >= threshold or |outR| >= threshold: count = 440; gate gain = 1
    if count > 0: count -= 1 else gate gain *= fall
    out *= gate gain
out *= gain
```

The glides are f32 and stop where a step rounds away: a mix at 1 stays
about 3e-5 short, which leaves the input in at about -45 dB.

Nothing in the engine is ever cleared after the constructor: not by
`resume`, not by a new rate. The plugin adds no latency (the delay is the
effect).

## Implementation notes (crates/plugins/src/vintage_chorus.rs)

- The engine bit for bit as above, with Rust's `sin`, `cos`, `tan`,
  `powf` and `f32::sin` for the CRT's, and no flush to zero.
- The noise seeds are those of the first engine FL loads; a project with
  more than one Vintage Chorus has other seeds in FL. `fidelity()` says
  so while the noise is on.
- Sync takes the song's place from the host's ticks (`follows_song` while
  synced), as FL's VST host fills `ppqPos` from its song position;
  `reset()` stops the song and clears nothing else, as in FL.
- A new rate keeps the lines, as FL does.
