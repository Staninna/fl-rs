# BassDrum: state, parameters, voice model

Recovered from `BassDrum_x64.dll` (FL Studio 2026, Delphi shell, image base
0x400000) and its `engine_x64.dll` (MSVC C++, image base 0x180000000, export
`CreatePlugInstance`, classes `CFLWrapper` / `CPluginWrapper` around a synth
core). The shell is a knob panel: state, voices and sound all live in the
engine. Ghidra project `ghidra/proj`; decompiles under
`ghidra/out/bassdrum/vmt/` and `ghidra/out/bassdrumeng/` (`vmt/`
CreatePlugInstance, `ctor/` the wrapper and core constructors, `slots/` the
SDK slots, `core/` and `state2/` the core's note and state methods, `render/`,
`render2/` and `render3/` the render path down to the per-sample loop, with
`asm/` listings). One run on the shell, ten on the engine; the Ghidra project
holds the engine as `bassdrum_engine_x64.dll` (a plain `engine_x64.dll` hits
3x Osc's). Corpus: 5 uses in 4 `.flp` and 56 shipped presets in 54 `.fst`,
all 1384 bytes.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `inferred`, `asm`.

## How the plugin is wired

Shell: `CreatePlugInstance` @ `0x006ff060` -> constructor `FUN_006ff0b0`;
class reference `*0x006fee10` -> VMT `0x006feed8` (`TTestPlug`, 0x1b8 bytes).
`TFruityPlugInfo` @ `0x00759a9c`: SDK 1, "BassDrum", Flags `0x10600011`
(Generator, GetNoteInput, NewVoiceParams, bits 22, 28), NumParams 23.

| slot | SDK name | shell | engine (`CFLWrapper::vftable` @ 0x180068468) |
|---|---|---|---|
| 1 | Dispatcher | 0x006ff3f0 | 0x180002a40 |
| 3 | SaveRestoreState | 0x006ffbd0 -> engine, then `FUN_006ffc10` | 0x180002fa0 |
| 4 | GetName | 0x00700210 | 0x1800031a0 |
| 5 | ProcessEvent | 0x006ffcc0 | 0x180003410 |
| 6 | ProcessParam | 0x006ffce0 | 0x1800034b0 |
| 8 | Gen_Render | 0x00700340 -> engine | 0x180004810 |
| 9 | TriggerVoice | 0x00700380 -> engine | 0x180004980 |
| 10 | Voice_Release | 0x00700400 -> engine | 0x180004aa0 |
| 11 | Voice_Kill | 0x00700420 -> engine | 0x180004ac0 |
| 14 | NewTick | 0x00700360 -> engine slot 14 | 0x180001c10 (stub) |

Shell fields: `0x138` the engine wrapper (`FUN_00700440`: loads
`engine_x64.dll`, `CreatePlugInstance(tag, host, ...)`, then engine
Dispatcher `0x10` and `0x36`), `0x148` i32[23] wheel values. Engine wrapper
(0xc60 bytes): `+0x8` tag, `+0xe0` / `+0x240` host, `+0x218` and `+0x228` the
synth core (class vtable `0x18006ac20`, constructed by `FUN_18002a380`, which
registers the 23 parameter names), `+0x250` f32[] extra parameters, `+0x458`
params pointer per MIDI note, `+0x858` host tag per note. The engine's
descriptor (`*0x1800799f8`): `+0x118` total params, `+0x11c` = 23 public,
`+0x129` output count, `+0x52a` demo flag, `+0x531` the data folder from
which "Noise Samples" and "sounddata" are resolved.

## 1. State layout (event 213: 1384 bytes)

The shell hands the stream to the engine (`FUN_180002fa0`), which wraps the
core's chunk:

```
save:    Write("SSLF"); n = core.getStateSize(); Write(i32 n); buf = core.getState(); Write(buf, n)      // core slots 35/36 at 0x18002bce0
restore: Read(i32 magic); Read(i32 n); if magic == "SSLF" and n: Read(buf, n); core.setState(buf, n) (0x180007980 -> 0x18002be70)
         then the shell (FUN_006ffc10) reads every param back from the engine and sets the 23 wheels
```

`setState` first skips a 60-byte VST `CcnK`/`FPCh` header if present, resets
all 23 parameters to their defaults, then:

| byte | type | field | corpus / presets | confidence |
|---|---|---|---|---|
| 0..3 | char[4] | "SSLF" | all | loader, corpus, preset |
| 4..7 | u32 | chunk size, 1376 | all | loader, corpus, preset |
| 8..11 | char[4] | core magic: "Kik2" (`0x4b696b32`), "Kik1" accepted; "EDK2"/"ED1K" are demo saves and refuse to load unregistered | Kik2 x56 presets, Kik1 x5 projects | loader, corpus, preset |
| 12..267 | char[256] | preset name, zero padded ("<noname>" default) | "Bassdrum 01" .. | loader, preset |
| 268..1291 | char[1024] | noise sample path; `%` prefix is a host path macro expanded through host message 0xe on load and produced by message 0x1c on save | `%FLStudioFactoryData%/Plugins/Fruity/Generators/BassDrum/Noise Samples/Hard Clap.wav`; projects carry absolute `C:\...\FL Studio 12\...` paths | loader, corpus, preset |
| 1292..1383 | f32[23] | parameters 0..22, each 0..1, applied in order through `core.setParam` | | loader, preset |

A chunk shorter than 260 bytes loads the name only; shorter than 1284 stops
after the path; the parameter loop takes as many floats as remain (up to 23).

Parameter order (engine index = DFM Tag; names from the core's registration
strings at `0x180068318..`), with the shell's display conversion
(`FUN_006ffef0`, `v` the 0..1 float):

| idx | parameter (DFM object) | display |
|---|---|---|
| 0 | Output level (`OutputLevelWheel`) | `(v - 0.5) * 40` dB |
| 1 | Sampling ratio (`SamplingRatioWheel`) | `2^((v - 0.5) * 4)` |
| 2 | Duration | `v * 100` % |
| 3 | Sub bass (`BassBoostWheel`) | % |
| 4 | Drive (`MasterDriveWheel`) | % |
| 5 | Attack (`MasterAttackWheel`) | % |
| 6 | Base (`BaseFreqWheel`) | `2^(4 v + 4)` Hz, 16..256 Hz |
| 7 | Peak (`BasePeakFreqWheel`) | % |
| 8 | Slide time (`BaseFreqSlideTimeWheel`) | % |
| 9 | Variation base | `(v - 0.5) * 200` % |
| 10 | Variation peak | `(v - 0.5) * 200` % |
| 11 | Variation slide | `(v - 0.5) * 200` % |
| 12 | Variation phase | % |
| 13 | Click cut (`ClickCutWheel`) | % |
| 14 | Click frequency | % |
| 15 | Click amount | % |
| 16 | Noise A | % |
| 17 | Noise D | % |
| 18 | Noise S | % |
| 19 | Noise start | `(v - 0.5) * 200` % |
| 20 | Noise pitch | `(v - 0.5) * 48` semitones |
| 21 | Noise mix | % |
| 22 | Noise filter | % |

Preset check (`.fst` name "BassDrum"):

| preset | noise sample | out | ratio | dur | sub | drive | base | peak | slide | click cut/freq/amt | noise A/D/S | start | pitch | mix | filter |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Bassdrum 01 | Hard Clap | +0.6 dB | 1.14 | 63 % | 68 % | 52 % | 37.4 Hz | 37 % | 27 % | 80/11/100 % | 34/43/65 % | +38 % | -13.8 st | 66 % | 85 % |
| Bassdrum 02 | Lo-Fi Crunch | +0.6 dB | 1.19 | 63 % | 68 % | 54 % | 38.9 Hz | 34 % | 36 % | 80/21/100 % | 35/42/66 % | +31 % | -16.1 st | 76 % | 100 % |
| Bassdrum 03 | Low Bit | +0.6 dB | 1.12 | 60 % | 94 % | 59 % | 39.4 Hz | 32 % | 36 % | 88/15/100 % | 23/49/72 % | -46 % | -3.9 st | 12 % | 100 % |
| Bassdrum 04 | Metallic | +0.6 dB | 1.12 | 64 % | 60 % | 35 % | 33.5 Hz | 38 % | 41 % | 88/15/100 % | 36/53/70 % | +16 % | -17.9 st | 36 % | 84 % |
| Bassdrum 05 | H-Freq Metal | +0.6 dB | 0.48 | 54 % | 60 % | 45 % | 62.3 Hz | 41 % | 34 % | 88/15/100 % | 30/55/50 % | +30 % | -6.3 st | 17 % | 100 % |
| Bassdrum 07 | Evolving 2 | +0.6 dB | 0.48 | 54 % | 81 % | 49 % | 83.6 Hz | 43 % | 27 % | 80/29/100 % | 36/39/66 % | +37 % | -11.2 st | 31 % | 100 % |

Output level is 0.514 in every factory preset (+0.6 dB); attack is 0 in all
of them; variation base/peak/slide sit at 0.63/0.379/0.663 in most.

## 2. Parameter semantics

Shell `ProcessParam` (`FUN_006ffce0`): REC_FromMIDI rescales with the wheel's
range; REC_UpdateValue stores the wheel int and calls the engine with
`v = clamp((int - Min) * (1 / (Max - Min)), 0, 1)` (`FUN_006ff840`, the
wheel's `+0x3cc` holds the reciprocal) and flag 1. Engine `ProcessParam`
(`FUN_1800034b0`): in int mode `v = int / 256` clamped to 0..1, in float mode
(the shell's case, `+0x220` set) `v` as given; if it differs from the core's
current value `core.setParam(idx, v)` (core slot 23, `0x18002ae90`) and
`core.markDirty(idx)`; REC_GetValue returns `core.getParam(idx)` (slot 27,
`0x180008450`: `*(params + idx * 0x94 + 4)`, 0x94-byte parameter records).
Indexes above 23 (up to 128 extra) are stored in `wrapper+0x250` as floats.

`core.setParam` (`FUN_18002ae90`) clamps `v` to 0..1 and writes a derived
value into each of the 99 voice objects (`BassDrumGen`, 0x188 bytes, list at
`core+0x39b8`). `asm` for the constants; the `pow` bases come from the
listing (`render2/asm/FUN_18002ae90`):

| idx | parameter | voice field | stored value |
|---|---|---|---|
| 0 | Output level | +0x5c | `10^(2 (v - 0.5))`, the displayed dB |
| 1 | Sampling ratio | (core render) | `2^(4 (v - 0.5))`, see 3.1 |
| 2 | Duration | +0x54 | `v^3`, seconds of decay at the internal rate |
| 3 | Sub bass ("Butt") | +0x7c | `v^2` |
| 4 | Drive | +0x80 | `v` |
| 5 | Attack | +0x58 | `v` |
| 6 | Base | +0x48 | `2^(4 v + 4)` Hz |
| 7 | Peak | +0x4c | `100 v^3`, a multiplier on Base |
| 8 | Slide time | +0x50 | `20 v^3 + 0.001` |
| 9 | Variation base | +0x60 | `2 v - 1`, octaves |
| 10 | Variation peak | +0x68 | `2 v - 1`, octaves |
| 11 | Variation slide | +0x64 | `2 v - 1`, octaves |
| 12 | Variation phase | +0x6c | `v` |
| 13 | Click cut | +0x70 | `v` |
| 14 | Click frequency | +0x74 | `2^(14 v)` Hz, 1 Hz to 16.4 kHz |
| 15 | Click amount | +0x78 | `v^5` |
| 16 | Noise A | +0x84 | `v` |
| 17 | Noise D | +0x88 | `v` |
| 18 | Noise S | +0x8c | `v` |
| 19 | Noise start | +0x90 | `v` |
| 20 | Noise pitch | +0x98 | `v` |
| 21 | Noise mix | +0x94 | `v` |
| 22 | Noise filter | +0x9c | `v` |

How the voice turns these into per-sample quantities is in 3.1. Two
surprises: Peak is not a frequency but how many times above Base the sweep
starts, and the three Variation knobs do not randomise anything. They define
a second, detuned copy of the sweep.

## 3. Voice model

`TriggerVoice(params, tag)` (`FUN_180004980`):

```
Host.Voice_ProcessEvent(tag, 6 = FPV_SetLinkVelocity, 0, 0)     // the engine handles velocity itself
note = clamp((int)(params.InitLevels.Pitch * 0.01), 0, 127)     // InitLevels.Pitch at +8: cents, 100 per MIDI note
if a voice is active on that note under another tag: core.noteOff(note), Host.Voice_Kill(oldTag, 0)
paramsPerNote[note] = params; tagPerNote[note] = tag
vel = clamp((int)FUN_180005fa0(), 0, 127)
core.noteOn(note, vel)       // slot 1, FUN_180007a20: level[note] = vel / 127 (0x18006aea4), then core slot 38
return note                  // the voice handle is the MIDI note
```

Core slot 38 (`FUN_18002b7b0`) hands the note to `FUN_180026690`, which marks
`core+0x390c[note]` held and calls core slot 57 (`FUN_180025ec0(note,
level)`), the voice allocator:

```
level > 0:  for each voice in the active list (core+0x1670, up to core+0x38f0 voices):
                claim the note slot (FUN_1800268a0: core+0xa20[note] += 1)
                voice+0x2c = 12 / (SR * core+0x38d0)             // glide rate, semitones per sample (0x18006afc0 = 12.0)
                if the voice was free or released:
                    voice+0x20 = level ^ core+0x38d8              // velocity curve, exponent not read
                    voice+0x30 = note                             // current pitch, glides to voice+0x10 (FUN_180026840)
                    voice.slot2()                                 // FUN_180028ff0, note on (3.1)
level == 0: find the nearest still-held note; if there is one, retarget the voice to it (legato),
            else mark the voice released and call voice slot 3, a no-op: a hit always plays out
```

The `BassDrumGen` vtable is `0x18006ab50`: slot 1 `FUN_180026810` free the
note slot, 2 `FUN_180028ff0` note on, 3 no-op, 4 `FUN_180028f80` reset and
stop, 5 `FUN_180026840` glide step.

So BassDrum keys one voice per MIDI note, 128 slots, and uses
`InitLevels.Pitch` as an absolute note number: here `Pitch / 100` is the MIDI
note, which pins the host convention `TVoiceParams` pitch = 100 cents per MIDI
note from note 0 for `InitLevels`. The three Delphi generators read
`FinalLevels.Pitch` instead, which the host offsets by -6000 (and the channel
pitch knob), so their zero is MIDI 60; see `host-voice.md`, section 3.

`Voice_Release(note)` -> `core.noteOff(note, 0)`; `Voice_Kill(note)` adds
`Host.Voice_Kill(tagPerNote[note], 1)`.

`Gen_Render(dest, &len)` (`FUN_180004810`): `core2(+0x218).slot10()`
(prepare), `Host.slot37(tag, 0, &{.., 0})` (lock the destination),
`FUN_180004190(wrapper, 0, dest, len, 0)`, unlock, `FUN_1800048d0` (post).
`FUN_180004190` gathers one buffer per engine output through host slot 26
(`0xd0`) for every output beyond the first (`descriptor+0x129 - 1` of them),
then renders through core slot 17 in blocks of up to `core+0xc20 - 4` frames.

### 3.1 Render

The synth runs at a fixed internal rate of 44100 Hz and the Sampling ratio
plus the host rate are handled by a linear-interpolation resampler per voice
(`FUN_180025140`, `FUN_1800251f0`, `FUN_180025250`). Constants are `asm`
unless tagged.

Core slot 17 (`FUN_18002b830`), per active voice (`voice+0xa0`):

```
r = clamp(2^(4 (p1 - 0.5)), 0.25, 4) * 2^(core+0x420[note] / 12)     // 0x18006aeb4 = 1/12; the per-note table is 0 unless set
voice+0x100 = r
tmp[0..n) = 0;  FUN_180029910(voice, tmp, n, noiseSample)
out.L += tmp * core+0x20[note];  out.R += tmp * core+0x220[note]       // per-note gain tables, 1.0 unless set
```

Note on (`FUN_180028ff0`), with `k = 1024 / tableLen` so increments are in
cycles for the 1024-point sine table at `voice+0x138` (`0x18006b070`), `f_s =
44100` and the stored parameters of section 2:

```
ampEnv = 1;  ampStep = dur > 0 ? 1 / (dur * f_s + 44.1) : 1          // dur = v^3 seconds (+ 1 ms)
ampSmooth = 1;  attackEnv = 0;  attackCoef = 1 / (attack^8 * 1e5 + 1)
drive = (Drive)^4 * 2560;  gain = outputLevel * velocityGain (voice+0x20)
sweep:   e = 1;  phase = 0.25;  decay = 2e-4 / slide;  baseInc = Base * k / f_s;  peak = Peak (100 v^3) * k
second:  e2 = 1;  phase2 = 0.25;  baseInc2 = 2^varBase * baseInc;  peak2 = 2^varPeak * peak
         decay2 = 2e-4 / (2^varSlide * slide);  delay2 = int(varPhase^2 * 350) samples
click:   pos = cut^(1/8) * 0.25 + 0.75;  posInc = clickFreq / f_s;  clickGain = amount (v^5) * 25
sub:     FUN_18001d4e0(filter, 42 Hz, q = 6 sub^2 + 0.5, SR)         // 0x18006b030 = 42, 0x18006aff8 = 6
noise:   pos = 0;  step = 2^(4 (pitch - 0.5));  mix = 2 mix^2;  lpCoef = filter^2
         startDelay = (2 (start - 0.5))^3 * 4410 samples              // +-100 ms; > 0: noise first, < 0: noise late
         A >= 1e-4: env = 0, target = 1, coef = 1 / (f_s * A^6)       // one-pole attack
         A <  1e-4: env = 0.99, target = S, coef = 1 / (D^6 * f_s + 44.1)
         decayTarget = S;  decayCoef = 1 / (D^6 * f_s + 44.1)
```

Per internal sample (`FUN_180029440`, `render2/`), `tbl(x)` reading the sine
table with linear interpolation at fractional cycle `x` (`FUN_180028c90`):

```
if startDelay > 0:                   // noise pre-roll: body waits
    body = 0;  startDelay -= 1
else:
    c = pos < 1 ? tbl(pos) * clickGain : 0;  pos += posInc           // click: the last quarter of the sine, once
    e -= e * decay;  inc = clamp((e^4 * peak + 1) * baseInc, 0, 0.5);  phase = frac(phase + inc)
    body = tbl(phase + c)                                             // the click phase-modulates the oscillator
    if delay2 > 0: delay2 -= 1
    else: e2 -= e2 * decay2;  inc2 = clamp((e2^4 * peak2 + 1) * baseInc2, 0, 0.5);  phase2 = frac(phase2 + inc2)
          body += tbl(phase2 + c)
    if startDelay < 0: startDelay += 1  (noise waits)
if the noise is due and a sample is loaded:
    s = sample[pos] (linear);  pos += step
    s = lp4(s, lpCoef)                                                // four one-pole stages y += (x - y) * lpCoef
    env += (target - env) * coef;  if env > 0.99: env = 0.99, target = decayTarget, coef = decayCoef
    body += s * mix * env^16
g = ampStep < 1 && ampEnv > 0 ? 7 ampEnv / (7 ampEnv + 1) : 0;  ampEnv -= ampStep      // 0x18006b000 = 7
ampSmooth += (g - ampSmooth) * 0.005                                                   // 0x18006aea0
out = attackEnv^2 * ampSmooth * body;  attackEnv += (1 - attackEnv) * attackCoef
```

So the pitch sweep is `f(t) = Base * (1 + 100 Peak^3 * e(t)^4)` with `e`
decaying exponentially, time constant `slide / 2e-4` samples (`(20 v^3 +
0.001) / 2e-4`, 5 to 100005 samples), and the amplitude is a linear ramp of
`dur^3` seconds through a `7x / (7x + 1)` knee, smoothed by a 5 ms one-pole.

Post stage (`FUN_180029910`), on the resampled output, per frame:

```
lp += (x - lp) * 0.7;  h = x - lp                                    // 0x18006aef8
h = h / (|h| d + 1);  h = h / (h^2 d + 1);  h = h / (|h^3| d + 1);  h = h / (h^4 d + 1)     // d = drive
y = h / N + lp                                                       // N: the same chain applied to 1.0
y = subFilter(y)                                                     // FUN_18001d5e0 below
out = y * gain * 0.4                                                 // 0x18006aedc
silent for more than SR * 0.0232 frames (|out| < 1e-5): voice.slot4 -> stop, free the note slot
```

Drive therefore saturates only what is above the 0.7-coefficient one-pole
(a few kHz at the internal rate) and leaves the low end clean. The sub filter
(`FUN_18001d4e0`, `FUN_18001d5e0`) is a two-pole high-pass whose resonance is
the Sub bass knob:

```
w = 2 pi f / SR  (f = 42);  (sn, cs) = sincos(w)                     // which half of the pair is sin is `inferred`
b = (1 + cs) / (2 + sn / q);  a1 = -2 q cs / (q + sn);  a2 = (q - sn) / (q + sn)
y = (x - 2 x1 + x2) * b - a1 y1 - a2 y2                              // denormals below 1e-9 flushed
```

At q = 0.5 it is a gentle low cut; at q = 6.5 a peak around 42 Hz.

The shell's `TriggerVoice` (`FUN_00700380`) also asks the host for the voice's
velocity (`Voice_ProcessEvent(tag, 3)`) and posts it to the form (message
0x54e) for the display.

## 4. What stays unknown

- The velocity exponent `core+0x38d8` and the per-note tables `core+0x20`,
  `+0x220`, `+0x420` (gain L/R and semitone offset): defaults 1, 1, 0 are
  what the render sees; no writer in the decompiled code.
- `voice+0x18 = 2 (rand - 1) * core+0x38cc`, set at note on and not read by
  the render path above.
- The sine table at `voice+0x138` is taken as 1024 points from the `1024 /
  len` scaling; its builder was not read.
- The core parameter record (0x94 bytes each: default, range, name, ...).
- `FUN_180005fa0` (velocity source) and host slot 26 (`0xd0`).
- Dispatcher IDs: the engine handles 0, 2, 4 (sample rate), 7, 0xb, 0xc,
  0xd, 0x10, 0x12 (file path), 0x19, 0x26; only the switch heads were read.
- The noise sample set: the shipped `Noise Samples` folder is listed by the
  engine (`FUN_18002a380` walks "Noise Samples" and "sounddata"); the names
  the presets use are in the preset table.

## Corrections from the implementation (crates/plugins/src/bassdrum.rs)

- The magic on disk is `2kiK`/`1kiK`; 4 presets are Kik1.
- The oscillator table is the file `BassDrum/sounddata`, not a sine.
- The saturator is normalised at 0.5, not 1. Velocity is `int(127·log21(10·vol + 1))/127`.
- Parameters are the state's 0..1 floats. Missing wave or noise files report Approximate.
