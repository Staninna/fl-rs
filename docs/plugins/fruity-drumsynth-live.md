# Fruity DrumSynth Live: state, patches, voice model

Recovered from `Fruity DrumSynth Live_x64.dll` (FL Studio 2026, Delphi shell,
image base 0x400000) and its `drumsynth_eng_x64.dll` (MSVC C++, image base
0x180000000, export `CreatePlugInstance` at `0x180002c40`, class `TTestPlug`).
The shell keeps 120 patches, one per key, and hands one at a time to the
engine, which renders each voice in mono. Corpus: 9 states, 8 in Mehran
Abbasi - With You (version 4) and the render test `Drumsynth Live.flp`
(version 3). The two shipped `.fst` presets hold no state of their own.

Confidence tags: `asm` (read from the listing), `dec` (decompile),
`dfm` (the editor form), `corpus`, `inferred`.

## How the plugin is wired

Shell: `CreatePlugInstance` @ `0x007164a0`, class reference `*0x00716038` ->
VMT `0x00716100` (`TTestPlug`, 0x5018 bytes).

| slot | SDK name | shell | engine (`0x18005f460`) |
|---|---|---|---|
| 1 | Dispatcher | 0x00716500 | 0x180001670 (message 4 sets the rate) |
| 3 | SaveRestoreState | 0x00717770 | 0x180001830 (35 i32) |
| 6 | ProcessParam | 0x00717ed0 | 0x1800017b0 (stores the i32) |
| 8 | Gen_Render | 0x00719660 | 0x180001700 (empty) |
| 9 | TriggerVoice | 0x007192b0 | 0x180001880 |
| 10 | Voice_Release | 0x00719540 | 0x180002430 |
| 11 | Voice_Kill | 0x00719580 | 0x180002380 |
| 13 | Voice_Render | | 0x180002560 |

Shell fields: `+0x130` the engine, `+0x148` the key the editor shows,
`+0x150` i32[120 * 38] the records, `+0x4890` f32 pairs per key (Level gain,
send gain), `+0x4c50` the 120 names, `+0x146` the auto-select flag, `+0x128`
the list of voices (0x38 bytes each: params, host tag, engine voice, ramp
state, key at `+0x1c`, mode at `+0x20`, cut flag at `+0x24`, ramp gains at
`+0x28`/`+0x2c` and send ramp at `+0x30`/`+0x34`).

## 1. State (event 213), `FUN_00717770`

```
i32 version (2..4; 1 is a 24-patch layout this port turns down; >= 5 refused)
i32 selected key (this+0x148)
120 x Delphi short string (u8 length, bytes): the names, by key   (FUN_00714660)
120 x 38 x i32: the records (0x4740 bytes at this+0x150)
version >= 3: u32 flags, bit 0 = auto-select patch (this+0x146)
```

Version 4 writes 19467 bytes with the shipped names. Before version 4 a
send track (field 36) of -1 is set to 0 (`asm`, the loop after the read).
Load then selects the key (`FUN_00719b90`), which sends every parameter
through ProcessParam, so every key's Level and send gain are worked out.

### Records and keys

The records are not in key order (`FUN_00719b00` record -> key,
`FUN_00719b40` key -> record):

```
records 0..23  -> keys 48..71
records 24..71 -> keys 0..47
records 72..119 -> keys 72..119
```

A parameter index is `record * 38 + field` (`FUN_00719a90`), so FL's
automation numbers parameters by record. Indexes from 4560 on
(`FUN_00719a60`: `index - 0x11aa`) are shell globals; none is in the state.

### Fields (DFM `TSynthEditorForm` Tags, ranges, and the engine's scale)

| field | control | range (dfm) | engine scale (`FUN_180001880`) |
|---|---|---|---|
| 0 | LevelWheel | 0..128, 75 | shell: `(11^(v/128) - 1) / 10`, 1 at 128 (`FUN_00717ed0`, ln 11 as an f32) |
| 1 | PanWheel | -64..64 | shell: added to the note's integer pan |
| 2 | ToneWheel | -50..50 | `v/100 + 0.5` |
| 3 | Osc1DistortWheel (harmonics) | -50..50 | `v/100 + 0.5` |
| 4 | Osc1 frequency (graph, no wheel) | 0..1000 `inferred` | `v * 0.001` |
| 5 | Osc1 sweep start / band width (graph) | 0..127 `inferred` | `v * 0.007874` |
| 6 | Osc1SweepWheel | 0..127 | `v * 0.007874` |
| 7 | Osc1DecayWheel | 0..127 | `v * 0.007874` |
| 8 | Osc1MixWheel | 0..127 | `v * 0.007874` |
| 9 | Osc2ShapeWheel (exp to lin) | 0..127 | `v * 0.007874` |
| 10 | Osc2 frequency (graph) | 0..1000 `inferred` | `v * 0.001` |
| 11 | Osc2 width (graph) | 0..127 `inferred` | `v * 0.007874` |
| 12 | Osc2AttackWheel | 0..127 | `v * 0.007874` |
| 13 | Osc2DecayWheel | 0..127 | `v * 0.007874` |
| 14 | Osc2MixWheel | 0..127 | `v * 0.007874` |
| 15 | WaveFormWheel | -1..17 | integer; the shell sends -1 as 0x80, none |
| 16 | WaveTuneWheel | -50..50 | `v/100 + 0.5` |
| 17 | WaveMixWheel (noise to wave) | 0..127 | `v * 0.007874` |
| 18 | RetriggerWheel (count) | 0..100 | integer |
| 19 | TrigDelayWheel (rate) | 0..127 | `v * 0.007874` |
| 20 | TrigDecayWheel | 0..127 | `v * 0.007874` |
| 21 | Osc2ReleaseWheel (release) | 0..127 | `v * 0.007874` |
| 22 | PhaseSwitchBtn (Osc1 phase invert) | 0, 1 | integer |
| 23 | ClickWheel | 0..100 | `v * 0.01` |
| 24 | MuteWheel (voice mode) | 0..5 | shell only: 0 mono, 1 poly, 2..5 cut groups |
| 25..31 | velocity to Osc1 frequency, Osc1 level, Osc2 frequency, Osc2 level, wave tune, tone, harmonics | -50..50 | `v/100 + 0.5` |
| 32 | Osc1ModeBtn | 0, 1 | integer: swept tone, noise band |
| 33 | Osc2RingWheel | 0..127 | `v * 0.007874` |
| 34 | MorphToCombo | a key | shell only |
| 35 | SendLevelWheel | 0..128, 50 | shell: the Level curve |
| 36 | SendWheel (track) | | shell only |
| 37 | ModWheel (morph amount) | 0..128, 64 | shell only |

Engine parameter 0 is always 45000 (`v / 65536`); 1 is not sent; 34 is not
read. The corpus holds -1 and 0 in field 36 and its own key in field 34.

## 2. A note, in the shell (`FUN_007192b0`)

```
key = (InitLevels.Pitch + 50) div 100            // the int TLevelParams of the old SDK
if key not in 0..119: no voice
mode = patch[key].field24
if mode != 1:
    for v in voices: if (v.mode == 0 and v.key == key) or (v.mode > 1 and v.mode == mode): v.cut = 1
FUN_00719d70(key, morph = 1)                      // the patch to the engine, below
FinalLevels.Pitch += 6000 - InitLevels.Pitch; InitLevels.Pitch = 6000
engine.TriggerVoice(params, tag)
```

`FUN_00719d70`: engine param 0 = 45000, then fields 2..33 but 24. When field
34 names another key `t` (`>= 0`, not `key`), each field but 15 is
`Round(a + s * (b - a))` (Delphi `Round`, ties to even) of the two patches,
with `s` from field 37:

```
x = min(v / 128, 1);  s = x * 1.6 - 0.3;  if s > 0.9: s = x * 0.4 + 0.6;  if s < 0.1: s = x * 0.4
```

Voice_Release (`FUN_00719540`) sends the voice's key's patch again and calls
the engine's release.

## 3. The engine's voice (`FUN_180001880`)

`rate` is the host's, as an int (`FUN_1800038d0`); `T = 1/rate` (f32).
`p[i]` the scaled parameters. `vel = 2 * InitLevels.Vol - 1 + FinalLevels.FRes`,
`ratio = exp((pitch*0.01 + 60 - 60) * ln2/12)` with `pitch` the integer cents
from the key. Helpers (all `asm`):

```
coef(t)          = 1 - exp(exp(8.16f - t * 9.21) * -T)                       // FUN_180002a80
level(k, b, v)   = k < 0.01 ? 0 : exp((b*3 - 1.5)*v + k*6.9 - 4.7)            // FUN_180002ae0
freq(x, r, b, v, d):                                                           // FUN_180002b50
    t = (b - 0.5) * 2.23
    hz = min(16000, floor(16 * exp(t*v*t*t + x*6.90777)) * r)
    w = hz * T * 2pi
    return w - ((0.04 - d*0.07)*w + d*0.3) * w*w
```

| offset | what | value |
|---|---|---|
| 0x2c, 0x30 | noise and wave gain | `sqrt(rate)/210 * (1.0001 - p17)`, `3 * p17`; with no wave `sqrt(rate)/210`, 0 |
| 0x34..0x40 | wave position, step, end, loop length, 20.12 fixed point | step `int(min(8, exp(vel*0.5*t^3 + p16*1.3863) * T*22050*ratio) * 4096)`, `t = 2 p29 - 1`; start/end/loop from the table at `+0x2d0` |
| 0x44, 0x48, 0x4c, 0x50 | Osc1 glide, target, frequency, offset | swept: target `freq(p4, r, 0.5, 0, 0)`, start `freq(p5 < 0.5 ? 2 p5 p4 : (2 p5 - 1)(1 - p4) + p4, r, p25, vel, 0)`, glide `coef(p6)`; band: `w = p5 > 0.5 ? 1 : 2 p5`, damping `w^2`, coupling `w^3`, `f = min(1.98 - w^2, freq(p4, r, p25, vel, w^2))`, and for `p5 > 0.5` with `k = 0.999 - 4(1 - p5)^2`: offset `-2 k f`, frequency `f k + f` |
| 0x54, 0x64 | Osc1 damping and coupling | 0 for a swept tone |
| 0x68, 0x6c | Osc1 second resonator's start | click `c = p23^2` (`p23 > 0.9`: the generator's second byte * 0.0036), `c^3 - 1`; phase invert negates both and the coupling |
| 0x74..0x80 | harmonics shaper `(z * k(z) + a) * z + b` | `d = (2 p31 - 1) vel + 2 p3 - 1`; `d < 0`: `k = -6.6667 max(-1,d)^2`, `k(z>0) = -k`, `k(z<=0) = k`, `a = 1 + k`, `b = 0`; else `k = 4.708 min(1,d)^2`, `k(z>0) = 0`, `k(z<=0) = k`, `a = 1 + min(1,d)^2`, `b = -k/4` |
| 0x84, 0x88 | Osc1 envelope, decay | `level(p8, p26, vel)`, `coef(p7)` |
| 0x8c..0xa4 | Osc2 resonators | `w = p11 > 0.5 ? 1 : 2 p11`, `f = min(1.98 - w^2, freq(FCut/2 + p10, r, p27, vel, w^2))`; for `p11 > 0.5`: `0x90 = f k + f`, `0x8c = f - f k` |
| 0xb4, 0xb8, 0xbc | Osc2 envelope, decay, floor | `L = level(p14, p28, vel)`, `s = -exp(p9*9 - 7)`, floor `L s`, decay `1 - exp(exp(6.91 - p13*9.21) * ln((0.03 - s)/(1 - s)) * T)` |
| 0xc0, 0xc4 | Osc2 attack step, ramp | `L`, or `L / (exp(p12*8.5169 - 9.21) * rate)` for `p12 > 0.005`; ramp from 0 |
| 0x10..0x28 | re-trigger | count `clamp(field18, 0, 100)`, period `int(exp(p19*8.5169 - 9.21) * rate)`; the envelopes' start levels and decays kept; while it runs both decays are `coef(p20)`, or 0 for `p20 >= 0.995` |
| 0xc8 | ring | `p33` |
| 0xcc, 0xd0, 0xd4 | tone | `x = clamp((p30 - 0.5) vel + p2, 0, 1)`; `x > 0.5`: highpass, `x -= 0.5`, dry -1; else dry 0.0001; `0xd0 = 1 - exp(exp(14 x) * -2pi T * 20)` |
| 0xd8 | gain | `level(p0, 0.5, 0) * 2 * FinalLevels.Vol` |

The noise generator is one per engine at `+0x2c0`: `r = r * 0xbb38435 +
0x3619636b`, read as the float of bits `(r & 0x7fffff) | 0x40000000`, minus
3. Its seed is `0x56ce` plus the sum of every 254th byte pair of the first
0x8f5c0 bytes of the waves, an i32 (`FUN_180001060`).

## 4. A sample (`FUN_180002560`)

```
if --wait == 0: left--; osc2 env, osc1 env = their start levels; ramp = 0
                if left > 0: wait = period else: decays = the kept ones
in = noise * 0x2c
if step: pos += step; if pos > end: pos -= loop
         in += (bits((s1 - s0) * ((pos >> 5) & 127) + ((s0 + 0x808000) << 7)) - 3) * 0x30   // s0, s1 at pos >> 12
f = freq += (target - freq) * glide;  g = f + offset
y = y - (damp*y + s + in + in_prev) * f;   s = y*g + s
z = z - (z*damp + t + (y + y_prev) * couple) * f;   t = z*g + t
out = ((z * k(z) + a) * z + b) * env1;   env1 = (1e-5 - env1) * decay1 + env1
if env2 > 0:
    ring = (z - 1) * 0xc8 + 1
    y2 = y2 - (damp2*y2 + s2 + in_prev + in) * 0x90;  s2 = 0x8c*y2 + s2
    q = q - ((y2 + y2_prev) * couple2 + (q*damp2 + u)) * 0x90;  u = q*0x8c + u
    e = ramp < env2 ? (ramp += step) : (env2 = (floor - env2) * decay2 + env2)
    out += e * (ring * q)
x = clamp(out, -1, 1);  low = (x - low) * 0xd0 + low;  sample = (x * dry + low) * gain
```

The voice is over at the end of a block in which `left < 1`, env1 < 1e-4 and
env2 < 1e-4.

The waves: resource `Waves`/4003 of the engine DLL, 587366 bytes of 16-bit
PCM at 44.1 kHz (tune 0 reads it at `22050 * 2 / rate`). The 18 entries at
`+0x2d0` (start, end, loop, each sample * 4096) are in `WAVES` in the code.

Release (`FUN_180002430`): `if period > 0: left = 0`; with `r = p21` (0..1):
for `r < 0.95`, the attack stops where it is, and with `m = r/2 + 1/2` decay1
(and its kept copy) is `coef(m * p7)` and Osc2's decay, kept decay and
attack step are `coef(m * p13)`. A release of 0.95 or more changes nothing.

## 5. Output, in the shell (`FUN_00719660`)

For each voice, in the order they started: the engine renders the block
into a mono buffer; then

```
(L, R) = Host.ComputeLRVol(FinalLevels.Pan + field1, FinalLevels.Vol * level[key])   // host slot 3, the int pan over 64
         or (0, 0) for a cut voice
if field36 != 0: the send, through the host's mixing track buffer (not built)
AddWave_32FM_32FS_Ramp(buffer, out, len, L, R, rampL, rampR)   // from the last block's gains; the first block starts at L, R
kill when (cut and rampL == rampR == 0) or the engine says the voice is over
```

## What the port leaves out or stands in for

- The ramp of `AddWave_32FM_32FS_Ramp` is not read; a straight line over the
  block stands in. It is heard only on a cut voice and on a moved Level.
- The engine reads `FinalLevels.Vol` into its gain and the shell again into
  the level, so FL's channel volume counts twice. The host here puts it on
  once.
- FL's blocks are not ours, and the one noise generator is drawn a block per
  voice at a time.
- Sends (field 36 over 0) are not built; version 1 states are turned down.
- 256 voices at most; FL has no limit.
