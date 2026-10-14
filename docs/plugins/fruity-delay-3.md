# Fruity Delay 3: state, parameters, signal chain

Recovered from `Fruity Delay 3_x64.dll` (FL Studio 2026, Delphi shell, image
base 0x400000) and `engine_x64.dll` next to it (MSVC C++, image base
0x180000000, `FLDelay3.pdb`, built by Schaack Audio on their "d1framework":
RTTI names `FLDelay3Plugin`, `ILDelay3`, `DelayChorusLFOBase`,
`SaturatorComponent`, `ParameterSmoother`). The Delphi side owns the state
and the GUI and hosts the engine as a VST2 (`TVSTPlugin`, `TEngineHost`); the
engine does all the audio. Ghidra project `ghidra/proj`; decompiles under
`ghidra/out/delay3/vmt/` (shell), `ghidra/out/delay3eng/` (`params/`,
`plug/`, `proc/`) and `ghidra/out/delay3fin/` (process entry, reducer, and
under `asm/` the listings of the coefficient routines, because the decompiler
drops the arguments of `tan` and `pow`). Corpus: 110 uses in the 165 `.flp`,
10 shipped presets. Two Ghidra runs on the shell, five on the engine (import,
parameter names, plugin class, process, coefficients). Confidence for the
chain is high: every coefficient below is read from the code or the constant
pool, with its address.

The plugin object holds the delay object inline at `+0x1d8`, so a plugin
offset in the setter is the delay offset plus 0x1d8 (plugin `0x544` = delay
`0x36c`, plugin `0x490` = delay `0x2b8`). Offsets below name which object they
belong to.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `inferred`, `asm`.

## How the plugin is wired

Shell: `CreatePlugInstance` @ `0x006e88f0` -> `TTestPlug` VMT `0x006e87d0`.

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006e8940 |
| 2 | Idle_Public | 0x006d1560 |
| 3 | SaveRestoreState | 0x006e8da0 |
| 4 | GetName | 0x006e9810 |
| 5 | ProcessEvent | 0x006e9360 |
| 6 | ProcessParam | 0x006e93b0 |
| 7 | Eff_Render | 0x006e9b70 |

Shell object: `0x150/0x158` planar input L/R, `0x160/0x168` planar output
L/R, `0x178` the `TVSTPlugin` (its vtable+0x1f0 = setParameter, +0xc8 =
processReplacing), `0x180` i32[26] the parameters.

Engine: `CreateEngine` @ `0x18000e830` builds an `AEffect` whose dispatcher
is `FUN_18000dd80`, setParameter thunk `0x18000ddf0` -> `FUN_180002170`,
getParameter `FUN_1800022c0`, chunk get/set `FUN_1800022e0` /
`FUN_180002380` (unused by FL), parameter names `FUN_180003390`, displays
`FUN_180003940`. `FLDelay3Plugin` vtable `0x1800e66c0`, `ILDelay3` vtable
`0x1800e69d0` (slot 1 `FUN_18000b410` = prepare(sampleRate)). Per-sample
tick `FUN_180008ff0`; the three delay models are `FUN_1800094b0` (Mono),
`FUN_180009a20` (Stereo), `FUN_18000a370` (Ping pong); shared feedback filter
update `FUN_18000ab70`.

## 1. State layout (event 213, 108 bytes, one version)

`SaveRestoreState` (`FUN_006e8da0`): `Write(i32 1); Write(this+0x180, 0x68)`;
restore reads the 26 ints if version < 2, then runs the GetValue/UpdateValue
loop, which pushes every value to the engine. The corpus and all presets are
version 1, 108 bytes.

The wheel `Tag` is the parameter index. The engine receives the value as a
float through `setParameter`: for indices 2, 3 and 13 (mask `0x200c` at
`0x006e971c`) the raw integer, for the rest `(v - Min) * wheel.scale` clamped
to 0..1, i.e. `v / Max` for the 0-based wheels (`inferred` for the two
`TSwitchSelector`s, whose Max is not in the DFM; the engine's 3- and 4-way
quantisers and the preset names agree).

| byte | type | idx | parameter (DFM object, hint) | range | default | corpus | confidence |
|---|---|---|---|---|---|---|---|
| 0..3 | i32 | | version | | 1 | 1 | loader, corpus |
| 4..7 | i32 | 0 | Input wet (`InputWetWheel`, "Send") | 0..6000 | 6000 | 6000 x90, 0 x3, 2499..3826 | loader, dfm, corpus, preset |
| 8..11 | i32 | 1 | Delay type (`DelayTypeSelect`): 0 Mono, 1 Stereo, 2 Ping pong | 0..2 | 1 | 2 x59, 1 x46, 0 x5 | loader, dfm, corpus, preset |
| 12..15 | i32 | 2 | Tempo sync (`TempoSyncCheckBox`) | 0, 1 | 1 | 1 x80 | loader, dfm, corpus, preset |
| 16..19 | i32 | 3 | Keep pitch (`KeepPitchWheel`) | 0, 1 | 0 | 0 x93 | loader, dfm, corpus, preset |
| 20..23 | i32 | 4 | Time (`TimeWheel`) | 1..768 | 192 | 192 x26, 144 x22, 384 x14, 96 x9, 1 x10 | loader, dfm, corpus, preset |
| 24..27 | i32 | 5 | Time R / Time R offset / Ping pong pan (`TimeRWheel`) | 0..6000 | 3000 | 3000 x66, 6000 x9 | loader, dfm, corpus, preset |
| 28..31 | i32 | 6 | Time smooth (`TimeOffsetWheel`) | 0..1000 | 500 | 500 x106 | loader, dfm, corpus, preset |
| 32..35 | i32 | 7 | Stereo spread (`StereoSpreadWheel`) | 0..6000 | 6000 | 6000 x106 | loader, dfm, corpus, preset |
| 36..39 | i32 | 8 | Modulation rate (`ModRateWheel`) | 0..6000 | 3000 | 3000 x86 | loader, dfm, corpus, preset |
| 40..43 | i32 | 9 | Modulation time ("Mod Amt", `ModAmountWheel`) | 0..6000 | 0 | 0 x46, 1474..5350 | loader, dfm, corpus, preset |
| 44..47 | i32 | 10 | Modulation cutoff (`ModulationAmountCutoffWheel`) | 0..6000 | 0 | 0 x89 | loader, dfm, corpus, preset |
| 48..51 | i32 | 11 | Diffusion level (`DiffAmountWheel`) | 0..6000 | 0 | 0 x69, 6000 x7 | loader, dfm, corpus, preset |
| 52..55 | i32 | 12 | Diffusion spread (`DiffSpreadWheel`) | 0..6000 | 0 | 0 x60, 6000 x28, 3000 x12 | loader, dfm, corpus, preset |
| 56..59 | i32 | 13 | Distortion type (`FeedbackDistortionTypeSelector`) | 0, 1 | 1 | 1 x98 | loader, dfm, corpus, preset |
| 60..63 | i32 | 14 | Feedback level (`FeedbackWheel`) | 0..6000 | 3000 | 3000 x20, 0 x25, up to 5610 | loader, dfm, corpus, preset |
| 64..67 | i32 | 15 | Filter mode (`FilterModeBtn`): 0 LP, 1 HP, 2 BP | 0..2 | 0 | 0 x72, 1 x25, 2 x13 | loader, dfm, corpus, preset |
| 68..71 | i32 | 16 | Feedback cutoff (`CutoffWheel`) | 0..6000 | 6000 | 6000 x48 | loader, dfm, corpus, preset |
| 72..75 | i32 | 17 | Feedback resonance (`FeedbackResonanceWheel`) | 0..6000 | 0 | 0 x67 | loader, dfm, corpus, preset |
| 76..79 | i32 | 18 | Distortion amount (`FeedbackDistortionThresholdWheel`) | 0..6000 | 0 | 0 x88 | loader, dfm, corpus, preset |
| 80..83 | i32 | 19 | Distortion hardness (`FeedbackDistortionHardnessWheel`) | 0..6000 | 0 | 0 x104 | loader, dfm, corpus, preset |
| 84..87 | i32 | 20 | Distortion symmetry (`FeedbackDistortionSymmetryWheel`) | 0..6000 | 0 | 0 x108 | loader, dfm, corpus, preset |
| 88..91 | i32 | 21 | Rate reduction (`FeedbackRateReduceWheel`) | 0..60000 | 60000 | 60000 x94, 40299 x9 | loader, dfm, corpus, preset |
| 92..95 | i32 | 22 | Bits reduction (`FeedbackBitReduceWheel`) | 0..6000 | 6000 | 6000 x100, 2157 x8 | loader, dfm, corpus, preset |
| 96..99 | i32 | 23 | Output wet (`OutputWetWheel`) | 0..6000 | 6000 | 6000 x59, 253..3703 | loader, dfm, corpus, preset |
| 100..103 | i32 | 24 | Output tone (`OutputToneWheel`) | -3000..3000 | 0 | 0 x51, -1575..2390 | loader, dfm, corpus, preset |
| 104..107 | i32 | 25 | Output dry (`InputDryWheel`, hint "Output dry") | 0..6000 | 6000 | 6000 x88, 0 x19 | loader, dfm, corpus, preset |

The DFM `DefaultValue`s are stale (Time 80, diffusion spread 3000, cutoff
3000); the shipped `Default.fst` is the authority and is what the table
shows.

Preset check (`.fst` files name the plugin "Fruity Delay 3"):

| preset | Type | Sync | Keep | Time | Time R | Spread | Mod rate/amt/cut | Diff lvl/spr | Dist | FB | Filter | Cut | Res | Dist amt/hard/sym | Rate | Bits | Wet | Tone | Dry | reading |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Default | Stereo | on | off | 192 | 3000 | 6000 | 3000 / 0 / 0 | 0 / 0 | 1 | 3000 | LP | 6000 | 0 | 0 / 0 / 0 | 60000 | 6000 | 6000 | 0 | 6000 | 1-beat stereo echo, 62 % feedback, filter open |
| Mono | Mono | on | off | 96 | 3000 | 6000 | 3000 / 0 / 0 | 0 / 0 | 1 | 3506 | LP | 4481 | 0 | 399 / 0 / 0 | 60000 | 6000 | 6000 | 0 | 6000 | half-beat mono echo, darkened, a touch of saturation |
| Ping pong | Ping pong | on | on | 96 | 2109 | 1819 | 3000 / 0 / 0 | 0 / 0 | 1 | 3000 | LP | 4612 | 975 | 398 / 0 / 0 | 60000 | 6000 | 6000 | 0 | 6000 | pan 2109 pulls the bounce left of centre; keep pitch on |
| Chorus | Stereo | off | off | 0 | 3000 | 6000 | 1032 / 3806 / 0 | 0 / 0 | 0 | 0 | LP | 4950 | 0 | 0 / 0 / 0 | 60000 | 6000 | 6000 | 0 | 6000 | shortest time, no feedback, deep slow modulation: a chorus |
| Diffused | Stereo | on | off | 144 | 3000 | 6000 | 3000 / 0 / 0 | 5025 / 5082 | 0 | 3038 | LP | 5137 | 0 | 492 / 0 / 0 | 60000 | 6000 | 6000 | 900 | 6000 | both diffusers up: smeared, reverb-like repeats |
| Tape | Stereo | on | on | 144 | 1969 | 2737 | 3000 / 0 / 0 | 0 / 0 | 1 | 3000 | LP | 3787 | 919 | 258 / 0 / 0 | 60000 | 6000 | 6000 | 900 | 6000 | dark, resonant, slightly saturated, keep-pitch wobble |
| Saturated | Stereo | on | on | 192 | 3000 | 1819 | 3000 / 0 / 0 | 0 / 0 | 1 | 3000 | LP | 4744 | 975 | 2086 / 492 / 0 | 60000 | 6000 | 6000 | 0 | 6000 | distortion amount a third up |
| Resonator | Stereo | off | off | 37 | 3000 | 6000 | 3000 / 0 / 0 | 0 / 0 | 0 | 4200 | LP | 4762 | 0 | 2390 / 0 / 0 | 60000 | 6000 | 2997 | 0 | 6000 | unsynced 37, 87 % feedback: a comb |
| Bandpass | Stereo | on | on | 144 | 3000 | 6000 | 3000 / 500 / 1275 | 0 / 0 | 1 | 3000 | BP | 3562 | 2138 | 0 / 0 / 0 | 60000 | 6000 | 6000 | 0 | 6000 | the only BP preset; filter mode 2 = BP fixes the enum |
| Stereoized | Mono | on | off | 144 | 3469 | 6000 | 3000 / 0 / 0 | 0 / 0 | 1 | 2681 | LP | 4481 | 0 | 399 / 0 / 0 | 60000 | 6000 | 6000 | 1088 | 6000 | Mono model with Time R off centre: the right tap lands later |

The corpus favours Ping pong (59 of 110) and musical Time values (192, 144,
384, 96: a beat, dotted eighth, two beats, eighth). Ten corpus slots have
Time = 1 with sync on: the shortest step, used as a flanger.

## 2. Parameter semantics

Shell `ProcessParam` (`FUN_006e93b0`): stores the int, calls the engine's
setParameter with the value as described above, refreshes the GUI (`GetName`
section 1 formats through `FUN_006e9740`). Tempo is not forwarded by the
shell: the engine asks the host for VST time info (`receiveVstTimeInfo`) and
reads tempo and PPQ itself, so the shell's `ProcessEvent` ignores
`FPE_Tempo`.

Engine `setParameter` (`FUN_180002170`): clamps to 0..1, keeps the float at
`this+0x630[idx]`, and for indices 0, 7, 10, 11, 14, 16..25 queues the change
on the `ParameterSmoother` list at `this+0x1c8`; the others go straight to
`FUN_1800029b0`. The block process (`FUN_180002f70`) steps the list every 8
samples: `cur += (target - cur) * 0x1c0` with `0x1c0 = 1 - 0.01^(1 / (0.005
SR))` (99 % in 5 ms, set in `FUN_180004640`), drops an entry once it is
within 1e-4, and pushes each step through the same setter:

```
v in 0..1 (or the raw int for 2, 3, 13)
 0 input wet      gain  = v                                            -> 0x240
 1 delay type     model = quantise(v, 4) in {0, 1, 2}; on change resets the delay (FUN_180008e20 zeroes the 8 rings and filter states); flags retune
 2 tempo sync     on = v >= 0.5                                        -> 0x210
 3 keep pitch     on = v >= 0.5                                        -> 0x211 (off also flags retune)
 4 time           t = v                                                -> 0x544, retune
 5 time R / pan   x = 2v - 1
                  x >= 0: cL = 1 - x,  cLR = x / 2,       cR = 1 - cLR, cRL = 0
                  x <  0: cR = 1 + x,  cRL = -x / 2,      cL = 1 - cRL, cLR = 0      -> 0x26c..0x278, 0x548; retune
 6 time smooth    T = v seconds; coef = v == 0 ? 1 : 1 - 0.01^(1 / (T * SR))   -> 0x5b8 (delay 0x3e0); 99 % of a glide in T
 7 stereo spread  a = 0.5 v + 0.5, b = 1 - a                           -> 0x27c, 0x280 (direct / cross)
 8 mod rate       Delay.setModRate(0.99 v)                             (vtable slot 0, FUN_18000b840; period table below)
 9 mod amount     depth = 0.1 v^2                                      -> 0x258
10 mod cutoff     0.5 v                                                -> 0x230 (delay 0x58): LFO amount added to the filter cutoff
11 diffusion      four allpass gains = clamp(v)                        -> 0x4a0, 0x4c0, 0x4e0, 0x500 (L head 0/1, R head 0/1)
12 diff spread    0x540 = v; retune
13 distortion type  quantise(v, 2): 0 "Limit", 1 "Saturate"            -> 0x700, 0x234 (strings 0x1800cd9ec / 0x1800cd9f8)
14 feedback       g = 1.25 v                                           -> 0x264   (125 % at full)
15 filter mode    quantise(v, 4) in {0 LP, 1 HP, 2 BP}                 -> 0x224
16 fb cutoff      v                                                    -> 0x250   (curve in section 3)
17 fb resonance   v                                                    -> 0x24c   (curve in section 3)
18 dist amount    Saturator.setAmount(v): drive_dB = -28.5 v           (FUN_18000b0e0; shapes in section 3)
19 hardness       0x21c = v; rebuild shaper(drive_dB, hardness, symmetry) (FUN_18000bef0 -> 11 floats at 0x5d0)
20 symmetry       0x220 = v; same rebuild
21 rate reduce    r = (1 - v)^3                                        -> 0x350, 0x378   (0 at 60000 = off)
22 bit reduce     q = (1 - v)^2; step = 2^(2 + 14 (1 - sqrt(1 - v)))   -> 0x360/0x388 q, 0x364/0x38c step  (q = 0 at 6000 = off; 4..65536 levels)
23 output wet     v                                                    -> 0x498
24 output tone    Delay.setTone(v)                                     (FUN_18000aeb0; 0.5 = flat; design in section 3)
25 output dry     v                                                    -> 0x268
```

`quantise(v, n)` is `FUN_180012a10`: `int((n - 1) v + 0.5)`. The power base
0.01 comes from `0x1800ec098` (asm of `FUN_1800029b0` at `0x180002c8d`);
`FUN_1800bcde0` is the CRT `pow`, `FUN_1800be480` `powf`, `FUN_1800bfc40`
`tan`, `FUN_1800bf350` `sinf`.

### Time to samples (`FUN_18000b1e0`, delay object)

Called on every retune for head k (0 or 1). Inputs: `0x36c` time v, `0x38`
sync, `0x30` tempo (double, copied from the host context by the block process
`FUN_180002f70`, which also raises the retune flag `0x2b8` when it changes),
`0x368` diffusion spread, `0x370` time R, `0x10` SR.

```
if sync:  steps   = quantise(v, 768) + 1                    // 1..768; the shell sends (Time - 1) / 767, so steps == Time
          seconds = steps / 192 * 60 / tempo                // 192 steps = one beat, 768 = a 4/4 bar
else:     t       = 0.94 v + 0.06
          seconds = (t^3 - 0.000216) * 1.000216 * 0.999 + 0.001     // 1 ms at v = 0, 1.000 s at v = 1, cubic
total = int(seconds * SR)                                   // samples through the loop

A    = max(1, min(1, total / (0.015 SR)) * (SR / 44100) * (590 spread + 65))   // diffuser allpass length, 65..655 @44.1k, shrinks below 15 ms
x    = |timeR - 0.5|                                        // 0..0.5
Mono:            B = max(1, total - A);  C = int(x * total + B)        // C is the longer tap, 1.5 total at full
                 if timeR < 0.5: swap(B, C)                            // 0x3d8 remembers the side
Stereo/Ping pong: D = max(1, total - A);  B = 1;  C = max(1, x^2 * total)
                 if timeR >= 0.5: swap(B, C)
```

Stored as ints: A `0x374[k]`, B `0x37c[k]`, C `0x384[k]`, D `0x38c[k]`. The
live lengths are doubles `0x398`, `0x3a8`, `0x3b8`, `0x3c8` (+8 for head 1)
that glide towards them. The constants are the pool at `0x1800ec0b8` (0.06),
`0x1800ec0f8` (0.94), `0x1800ec050`, `0x1800ec118`, `0x1800ec100`,
`0x1800ec070` (0.001), `0x1800ec21c` (192), `0x1800ec178` (60),
`0x1800ec01c` (0.015), `0x1800ec224` (590), `0x1800ec214` (65).

Preset check: Default 192 synced at 120 bpm = 0.500 s, one beat; Mono 96 =
a half beat; the corpus 1 = 1/192 beat = 2.6 ms, a flanger; Resonator 37
unsynced: v = 36/767, t = 0.1041, 1.91 ms, a 523 Hz comb; Chorus (Time 0
in the file, clamped to v = 0, unsynced) = 1 ms plus 6..20 ms of modulation.
Tag `asm`, `preset`, `corpus`.

The delay lines are `calloc`ed in `FUN_18000b410` (prepare): 2^20 floats
when SR <= 61440, 2^21 up to 126976, 2^22 above; reads wrap through the
mask, nothing clamps a length to the buffer. Prepare also keeps `0x114 =
3.1415 / SR` for the feedback filter and `0x354 = int(0.1 SR)`, the
keep-pitch crossfade length.

The table at `0x180104a40` is not a note table. It is three small float
tables used by the LFO and the filters, followed by the LFO's random seed:

| address | floats | use |
|---|---|---|
| 0x180104a40 | 8.872, 3.808, 1.833, 0.878, 0.398, 0.150, 0.050, 0 | LFO period in seconds, index `0.99 v * 5.9994` (`0x1800ec1c0`), linear |
| 0x180104a64 | u32 seed | LCG state for the random LFO shapes (unused here) |
| 0x180104a68 | 270, 480, 1300, 7000, 22000 | low-pass cutoff Hz, index `c * 3.999` (feedback LP, tone LP) or `c * 3.6` (feedback BP) |
| 0x180104a80 | 20, 90, 400, 1300, 4200 | high-pass cutoff Hz, index `c * 3.999` (feedback HP, tone HP) |

`setModRate(rate)` (`FUN_18000b840`, rate = 0.99 v): `period = lerp(table,
5.9994 rate)`, phase
increment `0x2c = 1 / (SR * period)` (0.113 Hz at 0, 17.8 Hz at full), and
`0x7c = sqrt(period / 8.872)`, which scales the depth so fast modulation
moves fewer samples. The LFO (`FUN_1800012e0`, shape `0x28` = 0) outputs
`(sinf(2 pi phase) + 1) / 2`, 0..1.

## 3. Signal chain

Shell `Eff_Render` (`FUN_006e9b70`): deinterleave Source into the two planar
buffers (`PTR_DAT_0073e9e8`), `engine.processReplacing(in, out, Length)`,
interleave into Dest (`PTR_DAT_0073e670`). Nothing else.

Engine block process (`FUN_180002f70`): under a critical section, apply the
queued parameter changes, copy the host tempo into delay `0x30` (raising the
retune flag `0x2b8` when it changed), then per frame: step the smoother every
8 samples, squash input values below 1e-13 to zero, call the tick
`FUN_180008ff0(delay, &L, &R)`, write the outputs back.

Tick (`FUN_180008ff0`), all offsets in the delay object:

```
if keep pitch off (0x39 == 0):
    weights 0x360, 0x364 = 1, 0;  active head 0x34c = 0, idle 0x350 = 1
    if retune flag 0x2b8: recompute A, B, C, D for head 0 (FUN_18000b1e0); clear the flag
    for each live length (0x398, 0x3a8, 0x3b8, 0x3c8) with target int (0x374, 0x37c, 0x384, 0x38c):
        cur += (target - cur) * 0x3e0;  snap when |target - cur| < 0.001         // 0x3e0 = time smooth coefficient
else if not crossfading (0x348 == 0) and retune flag:
    counter 0x35c = 0; 0x348 = 1; clear the flag
    recompute A..D for the idle head and set its four live lengths to the targets at once
switch model 0x2bc:  0 FUN_1800094b0 Mono;  1 FUN_180009a20 Stereo;  2 FUN_18000a370 Ping pong
                     3 (no delay line; filter, saturator and tone on the dry input; unreachable from the shell)
if keep pitch on and crossfading:
    w = 0x35c * 0x358  (0x358 = 1 / 0x354, 0x354 = int(0.1 SR))
    weight[active] = 1 - w;  weight[idle] = w;  0x35c++
    at 0x35c == 0x354: swap active and idle, 0x348 = 0
```

The models receive `wA = 0x360` (head 0) and `wB = 0x364` (head 1); every
read below is `lerp(ring, pos - len[0]) * wA + lerp(ring, pos - len[1]) *
wB`, where `len[k]` is the live double for that head and the integer part
indexes the ring through its mask. Common prologue of all three
(`FUN_1800012e0` and `FUN_18000ab70` are called first):

```
lfo      = (sinf(2 pi phase) + 1) / 2;  phase += 0x2c
0x84    += (0x80 * 0x54 * 0x7c - 0x84) * 0x88        // depth in samples: amount * 0.5 SR * sqrt(period / 8.872), smoothed 99 % in 100 ms
mod      = (lfo + 0.5) * 0x84                          // added to every main read position, 0.5..1.5 depth
every 4th sample (0x50): recompute the feedback filter coefficients (below) for both channels
```

Mono (`FUN_1800094b0`), one line `0xe0` (mask `0xf0`, write index `0xec`):

```
in     = (L + R) / 2
read1  = line at B + mod;  read2 = line at C + mod                  // B base tap, C the Time R tap (swapped when timeR < 0.5)
fbTap  = 0x3d8 ? read2 : read1                                       // feedback is always taken from the base-length tap
fb     = HP10(0x8c * fbTap)                                          // 10 Hz DC blocker, coefficients 0x1d8/0x1dc/0x1e0, states 0x1cc/0x1c8
s      = Saturator(fb + in * 0x68)                                   // FUN_18000b960: reducer, then Limit or Saturate
s      = FeedbackFilter(s)                                           // FUN_18000bbe0, the LP/HP/BP SVF
d0     = allpass(s, ring 0x2d0, A[0], g0 = 0.499 * 0x2c8);  d1 = allpass(s, ring 0x2f0, A[1], g1 = 0.499 * 0x2e8)
line[pos++] = ((1 - g0^2) d0 + g0 s) * wA + ((1 - g1^2) d1 + g1 s) * wB
toneL  = Tone(read1, 0x268/0x26c/0x270, states 0x25c/0x258);  toneR = Tone(read2, 0x298.., states 0x28c/0x288)
L      = (0xa4 toneL + 0xa8 toneR) * 0x2c0 + L * 0x90 * 0x6c          // 0xa4 direct, 0xa8 cross (stereo spread); 0x6c is 1
R      = (0xa4 toneR + 0xa8 toneL) * 0x2c0 + R * 0x90 * 0x6c
```

Note the order: saturator before filter in Mono, filter before saturator in
the other two. One allpass per head, in parallel (`allpass(x, ring, len,
g)`: `d = lerp(ring, pos - len); ring[pos++] = x - g d; return d` and the
caller forms `(1 - g^2) d + g x`, a Schroeder allpass). The total loop
length is `A + B` in Mono and `A + D` in the other two because the allpass
sits in the write path.

Stereo (`FUN_180009a20`), lines L `0xe0` and R `0xf8`, input pre-delay rings
`0xb0` (L) and `0xc8` (R):

```
outL   = lineL at D + mod;  outR = lineR at D + mod
ringL[pos++] = L;  preL = ringL at B;  ringR[pos++] = R;  preR = ringR at C   // B or C is 1, the other x^2 * total: one input lands later
fbL    = HP10_L(0x8c * outL);  fbR = HP10_R(0x8c * outR)             // second DC blocker 0x208/0x20c/0x210, states 0x1fc/0x1f8
sL     = SaturatorL(FilterL(preL * 0x68 + fbL))                      // FUN_18000bbe0 then FUN_18000b960
sR     = SaturatorR(FilterR(preR * 0x68 + fbR))                      // FUN_18000bcc0 then FUN_18000baa0, states 0x14c.., reducer 0x1a0, envelope 0x240
lineL[pos++] = diffuse(sL, rings 0x2d0/0x2f0, gains 0x2c8/0x2e8)     // same per-head allpass pair as Mono
lineR[pos++] = diffuse(sR, rings 0x310/0x330, gains 0x308/0x328)
toneL  = Tone(outL);  toneR = Tone(outR)
L, R   = same spread matrix, wet and dry as Mono
```

With Time R above centre the swap in `FUN_18000b1e0` puts `x^2 * total` on
B, so the left input is the one pre-delayed (up to a quarter of the loop
length at full); below centre it is the right. The two lines never feed each
other; "stereo" comes from the input offset and the spread matrix.

Ping pong (`FUN_18000a370`): no pre-delay rings; each line is written from
the other line's feedback:

```
outL   = lineL at D + mod;  outR = lineR at D + mod
inL    = (0x94 L + 0xa0 R) * 0x68;  inR = (0x98 L + 0x9c R) * 0x68   // pan law from parameter 5: cL, cRL / cLR, cR
sL     = SaturatorL(FilterL(inL + HP10_L(0x8c * outL)))
sR     = SaturatorR(FilterR(inR + HP10_R(0x8c * outR)))
lineR[pos++] = diffuse(sL, rings 0x2d0/0x2f0)                       // L path writes the R line
lineL[pos++] = diffuse(sR, rings 0x310/0x330)                       // R path writes the L line
mixL   = 0xa4 Tone(outL) + 0xa8 Tone(outR);  mixR = 0xa4 Tone(outR) + 0xa8 Tone(outL)
if distortion type == 1 (Saturate): mixL = PeakLimit(mixL, env 0x228);  mixR = PeakLimit(mixR, env 0x240)   // the Limit envelope, applied at the output instead
L      = mixL * 0x2c0 + L * 0x90 * 0x6c;  R = mixR * 0x2c0 + R * 0x90 * 0x6c
```

B and C are computed but unused in Ping pong; Time R only drives the pan
matrix. In Stereo and Ping pong the right channel has its own filter state,
reducer, saturator envelope and DC blocker; coefficients are shared.

### Blocks

Feedback filter (`FUN_18000ab70`, asm at `0x18000ab83`): Simper's
trapezoidal SVF, coefficients recomputed every 4th sample and copied to both
channels (`0x12c..0x140` L, `0x160..0x174` R).

```
c   = clamp01(0x78 + 0x58 * lfo)                 // cutoff plus mod cutoff * LFO (0..1)
res = clamp01(0x74);  r2 = (0.5 res)^2
LP:  fc = lerp(LPtable, 3.999 c);  k = 1 / (4 r2 + 0.707);  mix (0, 0, 1)     // Q 0.707 .. 1.707
HP:  fc = lerp(HPtable, 3.999 c);  k = 1 / (4 r2 + 0.707);  mix (1, -k, -1)
BP:  fc = lerp(LPtable, 3.6 c);    k = 1 / (4.6 r2 + 0.107); mix (0, k, 0)    // 270 Hz .. 16 kHz, Q 0.107 .. 1.257
fc  = min(fc, 0.49 SR);  g = tan(fc * 0x114)      // 0x114 = 3.1415 / SR
a1  = 1 / (1 + g (g + k));  a2 = a1 g;  a3 = a2 g  -> 0x138, 0x13c, 0x140;  mix -> 0x12c, 0x130, 0x134
```

Tick (`FUN_18000bbe0`, bypass when mode > 2): `v3 = x - ic2; v1 = a1 ic1 +
a2 v3; v2 = ic2 + a2 ic1 + a3 v3; ic1 = 2 v1 - ic1; ic2 = 2 v2 - ic2; out =
m0 x + m1 v1 + m2 v2`. So LP = v2, HP = x - k v1 - v2, BP = k v1 (unity at
fc).

DC blockers (`FUN_18000bdc0` mode 1, re-set in prepare at `0x18000b4c9`):
first-order high-pass at `fc = min(10 Hz, 0.49 SR)`, `k = tan(pi fc / SR)`,
`b0 = 1 / (1 + k)`, `b1 = -b0`, `a1 = (1 - k) / (1 + k)`, `y = b0 x + b1 x1
+ a1 y1`. One per channel on the feedback signal.

Output tone (`FUN_18000aeb0`, asm at `0x18000aebf`): the same first-order
filter, coefficients shared by both channels.

```
v == 0.5: bypass (1, 0, 0), mode 2
v <  0.5: fc = lerp(LPtable, 2 v * 3.9999)           // 270 Hz at 0 .. 22 kHz at centre, mode 0 LP: b0 = b1 = k / (1 + k)
v >  0.5: fc = lerp(HPtable, 2 (v - 0.5) * 3.9999)   // 20 Hz at centre .. 4.2 kHz at full, mode 1 HP as above
fc = min(fc, 0.49 SR);  k = tan(fc * pi / SR)
```

Saturator (`FUN_18000b0e0` on amount, `FUN_18000bef0` on amount, hardness
or symmetry; asm at `0x18000b0f8` and `0x18000bef6`):

```
d        = 0.95 amount;  drive_dB = -30 d  -> 0x3c                   // 0 .. -28.5 dB
g        = 10^(drive_dB / 20)                                        // the ceiling
makeup   = (1 / (1 + 3.7032 d^2 - 7.5177 d^3 + 2.9843 d^4))^0.707  -> 0x3f4   // 1.0 at 0, 0.82 at full
ceiling  = g * makeup                                               -> 0x3f0
shaper 0x3f8 (11 floats): cp = (1 + sym) g;  cn = (1 - sym) g        // positive and negative ceilings
    th+ = cp hard;  knee+ = max(1e-11, cp - th+);  [0] th+  [1] knee+  [2] 2 knee+  [3] 0.25 / knee+
    th- = cn hard;  knee- = max(1e-11, cn - th-);  [4] th-  [5] knee-  [6] 2 knee-  [7] 0.25 / knee-
    [8] drive_dB  [9] sym  [10] hard
```

Per sample (`FUN_18000b960`, R twin `FUN_18000baa0`), after the reducer:

```
type 1 Saturate:  x >= 0: if x >= th+: d = min(x - th+, 2 knee+); x = th+ + d - d^2 * 0.25 / knee+
                  x <  0: same with th-, knee- on |x|, sign restored
                  out = x * makeup
```

A parabola from the threshold that reaches the ceiling with zero slope at
`d = 2 knee`, flat beyond: hardness 0 is a pure parabola from zero,
hardness 1 a hard clip at the ceiling; symmetry moves the two ceilings apart.

```
type 0 Limit:     x /= ceiling;  env 0x228 += (|x| - env) * (|x| >= env ? 0x22c : 0x230)
                  if env > 1: x /= env;  out = x * ceiling
```

`0x22c = 1 - 0.01^(1/441)` and `0x230 = 1 - 0.01^(1/4410)`, set once in the
constructor (`FUN_1800088e0`): 10 ms attack and 100 ms release to 99 %,
fixed for 44.1 kHz. The same envelope follower, with gain 1, is what Ping
pong applies to its outputs in Saturate mode (`0x228` L, `0x240` R).

Rate and bit reducer (`FUN_1800011f0`, object `0x178` L / `0x1a0` R, run at
the top of the saturator):

```
r = [0] = (1 - v21)^3;  q = [4] = (1 - v22)^2;  step = [5];  held = [6];  scale = [8] = SR / 44100;  n = [9]
if r > 0:  f = 127 r;  hold = max(1, (0.0002 f^3 - 0.0171 f^2 + 0.7435 f + 1) * scale)   // 1 .. 229 samples (44.1k): 192 Hz at full
           n += 1;  if n < hold: x = held  else: held = x;  n = frac(n / hold) * n
if q > 0:  x = int(step * x) / step
```

Diffusion: one Schroeder allpass per head and channel (four rings `0x2d0`,
`0x2f0`, `0x310`, `0x330`), length A from Diff. spread (section 2), `g =
0.499 * level`, weighted by the head weights. At level 0 it is a plain
delay of A samples, which is why A is subtracted from the loop length.

Keep pitch: the idle head takes the new lengths at once and the two heads
are crossfaded over `int(0.1 SR)` samples, so a time change never
pitch-shifts the tail; with it off the live lengths glide with the time
smooth coefficient and the tail bends.

## 4. What stays unknown

- Model 3 (the chorus-style path with no delay line) and the LFO shapes
  1..5 exist in the engine but nothing in the shell selects them.
- Engine parameter 0x18 "Send Colour" exists in the name table but has no
  wheel in the shell; it is never set.
- The shell's exact float for Time is taken as `(Time - 1) / 767` from the
  wheel's Min/Max; only that mapping makes 192 land on one beat through the
  engine's `int(767 v + 0.5) + 1`. Not re-read from the shell asm.
- The reducer's counter reset `n = frac(n / hold) * n` is written as found;
  it looks like a slip for `frac(n / hold) * hold`.

## Implementation notes (crates/plugins/src/delay_3.rs)

- 10 presets ship, not six. The preset table's Ping pong, Tape, Saturated and Bandpass rows show mod rate and mod amount under Time R and Spread; Bandpass has distortion amount 2485.
- Makeup at full amount is 2.20 by the polynomial, not 0.82. Unsynced full time is 44,099 samples at 44.1 kHz.
- `v / Max` with Max 2 cannot yield models 0, 1, 2; the saved int is taken as the model and the filter mode directly.
- Assumed: tempo 140 until set (clamped 10..522); glides skipped on state load, rate change and reset; rings sized to rate and tempo, capped at FL's sizes; time 0 accepted (the Chorus preset saves it).
