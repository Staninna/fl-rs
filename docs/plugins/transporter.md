# Transporter: state, parameters, the shell and its engine

Recovered from `Plugins/Fruity/Effects/Transporter/Transporter_x64.dll`
(FL Studio 2026, Delphi native plugin, image base 0x400000). Ghidra project
`transporter` (`/root/gh/proj` on the machine it was read on), decompiles
under `/root/gh/out/transporter/` (`a/` `CreatePlugInstance` and the
constructor, `b/` the VMT slots with callees, `c/` to `h/` the helpers).
Corpus: two project states (the render test, 85 bytes, and Edlan & Ella
Noël - Song For You, 110 bytes), six presets of 94 bytes and four copies
of the default (85 bytes) in the plugin database.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `asm`, `inferred`.

**The DSP is not in this DLL.** The shell creates its engine with
`TransientLooper_CreateInstance`, which it looks up in `ildsp` (FL's
`Shared/ildsp_x64.dll`, the MSVC library Spreader, Luxeverb, Low Lifter,
Emphasis and Pitch Shifter use). Section 4 maps that engine: a Faust
program behind a small C++ wrapper. `crates/plugins/src/transporter.rs`
is the shell and `crates/plugins/src/transient_looper.rs` the engine,
held to the DLL's own code under the unicorn emulator
(`tools/plugstate/transient_looper_emu.py`).

## What it does

From the editor's form and the shell `inferred`: a looper that listens for
transients (or the song's beat, in Tempo mode) and cuts the input into two
loops, A and B, the B one `A/B ratio` times as long as A. It plays them
under the dry signal, reversed, an octave up or down and panned by chance
(each with its bias) or in turn (Deterministic), and can ignore some
transients by chance. "Update loop A/B/A/B" grab a new loop now; "Ignore
transients" freezes the loops; "Flush" empties them. A second output can
go to a send track and the transients can be heard on another track.

## How the plugin is wired

`CreatePlugInstance` @ `0x007126f0` -> `FUN_00712760` (constructor), class
ref `0x00712438`, `TTestPlug` (instance size 0x260, parent
`TDelphiFruityPlug`) `asm`.

| slot | SDK name | address |
|---|---|---|
| 0 | Destroy | 0x00712d50 |
| 1 | Dispatcher | 0x00712ee0 |
| 2 | Idle | 0x0070f1c0 |
| 3 | SaveRestoreState | 0x00713760 |
| 4 | GetName | 0x00714320 |
| 5 | ProcessEvent | 0x00713350 |
| 6 | ProcessParam | 0x00713a40 |
| 7 | Eff_Render | 0x00714700 |
| 25 | Idle (meters) | 0x00712d90 |

Object fields `loader` `asm`:

| offset | type | meaning |
|---|---|---|
| 0x124 | i32 | block size, 0x400 at start |
| 0x130 | i32 | Flush as last sent, -1 at start |
| 0x134 | u8 | Lookahead, 1 at start |
| 0x138 | i32 | transient source: 0 left, 1 right, 2 left+right; 2 at start |
| 0x13c | i32 | architecture: 0 mono to stereo, 1 stereo to stereo; 1 at start |
| 0x144 | ptr | the `TransientLooper` engine |
| 0x14c | i32[22] | the parameters |
| 0x1a4 | i32 | output send track (the second output), 0 none |
| 0x1a8 | i32 | input send track (the transient source), 0 the input |
| 0x1ac, 0x1ad | u8 | editor shown, playing |
| 0x22c..0x24c | f32* | the second output L/R, the sidechain L/R, the beat buffer |
| 0x254 | u8 | Flush on pause/stop, 1 at start |

## 1. State layout (event 213, versions 3 to 8)

`SaveRestoreState` (`FUN_00713760`) `loader`. Save writes version 8:

| bytes | what |
|---|---|
| 0..4 | i32 version |
| 4..92 | the 22 parameters as i32 (version 8); 4..84, 20 of them, under it |
| +0 | u8 Flush on pause/stop (0x254) |
| +1 | u8 Lookahead (0x134) |
| +2 | i32 transient source (0x138) |
| +6 | i32 architecture (0x13c) |
| +10 | i32 output send track (0x1a4) |
| +14 | i32 input send track (0x1a8) |

Restore reads the version and does nothing when it is 9 or more. Under 8
it reads 0x50 bytes of parameters and sets parameter 21 (Ignore bias) to
0; from 8 it reads 0x58. Then it reads the rest field by field; a stream
read past the end leaves the local as it was, so a short state keeps the
plugin's own values. Then it applies the transient source and architecture
(engine parameters 24 and 25), the send tracks, Flush on pause/stop, and
runs every parameter through `ProcessParam` (get, then set with update).
**Lookahead is read and dropped**: the loader never stores it nor sends
engine parameter 23, so a loaded state leaves the engine's own lookahead.

Lengths `corpus` `preset`: version 3 is 85 bytes (parameters 0..19 and
the flush byte; the render test and the plugin database), version 4 is 94
(all fields but the send tracks; the six presets), version 8 is 110 (the
demo song).

## 2. Parameters

Ranges from the form (`TSynthEditorForm`, control `Tag` = parameter) `dfm`;
what `ProcessParam` hands the engine's `SetParam(index, f32)` from
`FUN_00713d30` `asm`. `v` is `(value - min) / (max - min)` in doubles,
clamped to 0..1 and taken as an f32 (`FUN_00713a40`, the range's
reciprocal at control +0x3cc).

| # | control | range | engine value |
|---|---|---|---|
| 0 | Dry levels | 0..12800 | dB, `FUN_007125f0` below |
| 1 | Loop level A | 0..12800 | dB |
| 2 | Loop level B | 0..12800 | dB |
| 3 | Reverse bias | 0..10000 | `v` |
| 4 | Octave shift bias | 0..10000 | `v` |
| 5 | Octave down up bias | 0..10000 | `v` |
| 6 | Spread and auto panning | 0..1024 | `(v - 0.5) * 2` |
| 7 | Minimum length A | 20..2000 ms | `v * 1.98 + 0.02` s |
| 8 | A/B ratio | 0..36 | `v * 9 + 1` |
| 9 | Transient sensitivity | 0..1024 | `v` |
| 10 | Bouncing rate | 0..1024 | `(v - 0.5) * 2` |
| 11 | Ignore transients | 0..1 | the value |
| 12 | Flush | 0..1 | the value, sent only when it changes and Bypass is 0 |
| 13 | Loop mode: Auto, Bouncing, Adaptive, Tempo | 0..3 | the value |
| 14 | Bypass | 0..1 | the value |
| 15, 16, 17 | Update loop A, B, A/B | 0..1 | the value |
| 18 | Probabilistic / Deterministic | 0..1 | the value |
| 19 | Tempo ratio length: 1/1, 1/2, 1/4, 1/8, 1/16, 1/32, the dotted five, the triplet five, the quintuplet five | 0..20 | the value |
| 20 | Tempo ratio trigger: the same 21 and Off | 0..21 | the value |
| 21 | Ignore bias | 0..10000 | `v` |

The level map (`FUN_007125f0`) `asm`: `g = 1` at `v == 1`, else
`g = (exp(v * (f32)ln 11) - 1) * 0.1`; `-inf` when `g <= 0`, else
`20 log10 g`. FL's volume curve, `(11^v - 1) / 10`, with `ln 11` and
`0.1` from the shared math unit's start-up (`FUN_005f1090`). The
constructor's inverse (`FUN_00712690`) is `ln(exp(dB * ln 10 / 20) * 10 + 1) / ln 11`.

Engine parameters past the controls `asm`: 22 the tempo in BPM (from
`ProcessEvent` `FPE_Tempo`), 23 Lookahead (`FUN_007149a0`, the options
menu, which then reports the engine's latency to the host), 24 the
transient source, 25 the architecture.

Defaults: the constructor asks the engine (`GetParam`, vtable +0x18) for
each parameter's default and inverts the maps above, so they are the
Faust zones' defaults (section 4): parameter 20's is the tempo
multiplier's 21, Off. A fresh plugin saves the render test's state: levels
12800 (0 dB), biases 0, spread 512, minimum length 500 ms, ratio 8 (3),
sensitivity 512, bouncing 512, the switches 0, length 1/1 `corpus`.
Parameter 20 is 21 (Off), the engine's default.

## 3. The engine's interface

What the shell calls on `TransientLooper` (vtable offsets) `asm`:

| offset | call | from |
|---|---|---|
| +0x00 | destroy | slot 0 |
| +0x08 | set sample rate (f32) | constructor, `FPD_SetSampleRate` (`FUN_00712cc0`) |
| +0x10 | set parameter (index, f32) | `ProcessParam`, options |
| +0x18 | parameter default (index) -> f32 | constructor |
| +0x20 | meter (0..6) -> f32 | idle, for the editor |
| +0x28 | render (frames, in, out, out2 L, out2 R, side L, side R, beat) | `Eff_Render` |
| +0x30 | flush | `FPD_Flush`, stop with Flush on pause/stop (`FUN_007133a0`) |
| +0x40 | latency -> i32 | after a rate or lookahead change (`FUN_00714950`) |

`Eff_Render` (`FUN_00714700`): the song position from the host (vtable
+0x80, in ticks) over PPQ (`FPD_SetTimeSig`) and 4, its fraction times 4,
goes with the beat buffer and the block length to a helper through the
function pointer at `0x007711d8` (`inferred`: it fills the buffer with
the place in a 4-beat bar). The sidechain buffers take the input track's
audio, or the plugin's own input when no input track is set (through the
pointer at `0x00771148`, `inferred` a deinterleave). The engine renders
from the input block to the output block and writes the second output,
which goes to the output send track when one is set.
`FPD_Flush` flushes the engine and sends Flush = 1.

## 4. The engine: `TransientLooper` in `ildsp_x64.dll`

`TransientLooper_CreateInstance` @ `0x1801abe00` makes a 0x10-byte
`TransientLooperImpl` (vtable `0x1803e0100`, constructor `0x1801abf10`)
that owns a 0x1d507a8-byte `TransientLooperFaust` (vtable `0x1803df118`):
a Faust program, `transientDetectionLooper.dsp`, compiled
`-lang cpp -i -ct 0 -cn TransientLooperFaust -es 1 -mcd 16 -mdd 1024 -mdy 33
-single -ftz 0` (its `metadata`, `0x18018ad20`). It has 5 inputs and 4
outputs. Everything is single precision; the runtime maths it calls are
`tanf` `0x1801c9ef0`, `expf` `0x1801c5f40`, `powf` `0x1801c83b0` (always
`powf(10, dB / 20)`), `logf` `0x1801c6910`, `fmodf` `0x1801c61e8` and, in
`classInit`, `sinf` `0x1801c97f0`. `asm` `emulator`.

### The wrapper (`TransientLooperImpl`)

| slot | does | address |
|---|---|---|
| +0x08 | set rate: `instanceConstants(int rate)` (which also zeroes both loops), `instanceClear`, the rate at `+0x1d50740`, the fade pole `expf(float(1 / max(1, rate)) * -94.2478)` (15 Hz), then `SetParam(23, 1)` lookahead on, `SetParam(24, 2)` both sides as source, `SetParam(25, 1)` stereo | `0x1801ac0e0` |
| +0x10 | `SetParam(index, value)` | `0x18018fc50` |
| +0x18 | `GetParam(index)`: the zone's value | `0x180190fa0` |
| +0x20 | meters 0..6: the bargraph zones `pointer0`, `pointer1`, `writePtr`, `panningCoeff`, `minDistanceFromWrtPtrA/B`, `onsetDetection` | `0x180191c80` |
| +0x28 | render | `0x180192070` |
| +0x30 | flush: the program's `instanceClear` (the loops keep their sound) | `0x1801ab530` |
| +0x38 | both loops zeroed (no caller found) | `0x1801ac1d0` |
| +0x40 | latency: `(int64)(rate * 0.005)` with lookahead on, else 0 | `0x1801ac210` |

The constructor sets the rate field to -1, the tempo to 60, the fades to
0, and makes the program with `init(44100)`.

`SetParam` writes zones through Faust's `MapUI` by name:

| index | zone (offset) | how |
|---|---|---|
| 0 | Input Level (`0x104c`, dB) | as is |
| 1, 2 | Loop Level A (`0x15e8`), Loop Level B (`0xea96ac`), dB | as is |
| 3, 4, 5 | Reverse (`0x15a0`), Octave Shift (`0x15ac`), Octave Down/Up (`0x15b0`) Probability | as is |
| 6 | Spread and Auto-Panning (`0x15f4`) | as is |
| 7 | Minimum Length A (`0x1070`, s) | kept at `+0x1d50788`; set unless the mode is Tempo |
| 8 | A/B Ratio (`0xea9614`) | as is |
| 9 | Transient Sensitivity (`0x1158`) | as is |
| 10 | Bouncing Rate (`0x1574`) | kept at `+0x1d50780`; set unless the mode is Tempo |
| 11 | Freeze (`0x152c`) | as is |
| 12 | (flush) | unless one waits, a flush waits when `v > 0.5` (`+0x1d50768`) |
| 13 | Loop Mode (`0x106c`) | `m = roundf(v)`; Tempo (3) sets Loop Mode 1 (Bouncing), Bouncing Rate 0 and Minimum Length `240 / tempo * ratio[length]`; the others set Loop Mode `m` and the kept rate and length |
| 14 | Bypass (`0x28`) | as is, and `+0x1d50769 = v > 0.5` |
| 15, 16, 17 | Trigger A (`0x14d0`), B (`0x14e8`), A/B (`0x14dc`) | 1 when `v` is above the last `v`, else 0 |
| 18 | Deterministic (`0x1550`) | `v > 0.5` |
| 19 | (ratio length) | `roundf(v)` kept; in Tempo mode Minimum Length again |
| 20 | Tempo Trigger Multiplier (`0x1538`) | `0.25 / ratio[roundf(v)]`, 0 for Off |
| 21 | Ignore Probability (`0x155c`) | as is |
| 22 | (tempo) | kept; in Tempo mode Minimum Length again |
| 23 | Lookahead (`0x1038`) | `v > 0.5`, also kept for the latency |
| 24 | Source (`0x10bc`) | 0, 1 or 2 by `roundf(v)`, **and falls through to 25** (no `break`): Mono/Stereo becomes `v > 0.5` too |
| 25 | Mono/Stereo (`0xea7610`) | `v > 0.5` |

`ratio[]` (`0x1803dd5b0`, bars): 1, 1/2, 1/4, 1/8, 1/16, 1/32, 1.5, 0.75,
0.375, 0.1875, 0.09375, 1/3, 1/6, 1/12, 1/24, 1/48, 0.2, 0.1, 0.05, 0.025,
0.0125, 0 (Off).

Render (`0x180192070`), at most 0x4000 frames: the interleaved input to
inputs 0 and 1, the shell's sidechain to 2 and 3, the beat buffer to 4.
With no flush waiting, Flush is set to 0; with one waiting and the left
fade under `1e-7`, both loops (`0x3a9800` floats each) are zeroed, Flush is
set to 1 for this block and the flush stops waiting. Then `compute`; then
each main side `out *= f^2`, `f = (f - t) p + t` per frame with `t = 1`
unless a flush waits and Bypass is off. Outputs 2 and 3 go to the second
output as they are.

### The program (`TransientLooperFaust`)

`instanceConstants` (`0x18018afd0`), `fs = min(max(rate, 1), 192000)`:
a 10 Hz TPT one-pole (`0x1c..0x24`, `g = tanf(10 pi / fs)`), a 20 Hz one
(`0x1040..0x1048`), the lookahead `0.005 fs` (`0x103c`), the buffer `20 fs`
(`0x1058`), the onset pole `1 / (1.8056 / fs + 1)` (`0x1074`), 20 ms
(`0x107c`), seven state-variable splits at `tan(F / fs)` with F = 149.44,
355.43, 845.36, 2010.62, 4782.09, 11373.78, 27051.56 (about 48 Hz to 8.6
kHz, a ratio of 2.38): `g`, `g + sqrt 2`, `g (g + sqrt 2) + 1`, `g` over
that and twice it, `2 g`; the followers' poles `1 / (288.9 / fs + 1)`
(`0x112c`) and `1 / (14444.8 / fs + 1)` (`0x1134`), `14.44 / fs`
(`0x1130`), `0.01 fs` (`0x14ac`), `expf(-10 pi / fs)` (`0x14c0`), the
auto-pan step `20 / fs` (`0x15f8`) and `2 fs` (`0x1560`). `classInit`
(`0x18018bab0`) fills 65536 sines `sinf(i * 2 pi / 65536)` at
`0x1803f8560`. `instanceResetUserInterface` (`0x18018b530`): every zone 0
but Minimum Length A 0.5, Sensitivity 0.5, Tempo Trigger Multiplier 21 and
A/B Ratio 3.

`compute` (`0x18018c4e0`), per frame:

1. The bypass: a 10 Hz one-pole of `1 - Bypass`, squared, `w`; the output
   is `w * wet + (1 - w) * dry`.
2. The dry input waits the lookahead (5 ms, or 0 frames off) in 1024-frame
   rings, left at `0x38`, right at `0xea7614`. Input Level glides at 20 Hz.
3. The write point: `(frame count + write) % N` frames, `N` the buffer, or
   1 while Flush is up, which brings it back to 0.
4. The detector: the source (left, right, or their sum over sqrt 2) goes
   through the tree of splits into eight bands. Each band's power runs
   through four peak followers in a row (`0x1134` up, `0x112c` down) and
   four slow ones whose up pole is `1 / (c 14.44 / fs + 1)` with `c =
   8388608` while `logf(sqrt(fast) / slow)` is over a smoothed threshold
   `1.5 + 6.5 (1 - sensitivity)^2` and 20 otherwise. A band has an onset
   when `logf(max(sqrt(slow), 0.001))` rose by more than 0.5 since the
   frame before. The count of such bands, **ANDed** with "20 ms since the
   last" (so only its lowest bit counts, as FL's code has it), is the
   transient; a 10 ms hold keeps the count's peak, and the onset rate
   (`0x1514`) is the transients and the update buttons through four poles.
5. Loop A: the minimum length is Minimum Length A, or in Adaptive mode
   `min(|length - 1 / (fs * onset rate)|, 2)` s, the onset rate from step 4. A new loop is taken on a
   transient once the loop has run that long (unless Freeze was on when
   the loop was taken), on Trigger A or A/B, or in Bouncing mode when the
   beat input times the multiplier wraps, and only if not ignored: by
   chance (`(lcg * 2^-32 + 0.5) > ignore`) or, Deterministic, when
   `fmodf(write point, 2 fs) < (1 - ignore) 2 fs`. The loop's start
   becomes its last end and its end the write point; its length is the
   span between (Auto, Adaptive) or, Bouncing, `fs * length` scaled by
   `expf(-rate / 2)` at every wrap, at least 48 frames. Each take draws
   reverse, octave and up by chance or in turn the same way, giving a
   speed of 1, 2 or 0.5. The read point is `start + speed * phase`, or
   `end - speed * phase` reversed, modulo N; the phase counts up and wraps
   at the length.
6. A 4-pole fade (pole `1 / (14.44 / lookahead + 1)`) closes the loop
   while the read point is within 2 frames of the write point, or the
   length or the phase jumped, and opens again after the lookahead.
   Loop Level A glides at 20 Hz.
7. Loop B as loop A, its length `1 / A/B Ratio` of A's, on Trigger B or
   A/B. The two draw from one LCG (`x * 1103515245 - 0x2c2419bb`) stepped
   8 times a frame.
8. Both loops record the left (B the right) input, or in mono their mid;
   the played sample waits the lookahead in its own ring.
9. The pan: the spread, or below 0 an auto-pan sine stepping `-spread *
   20 / fs` a frame, through the 20 Hz glide; `p = (s * spread' + 1) / 2`.
   Left = `p A + (1 - p) B + gain * dry`, right the other way round; the
   second output is each loop with its fade but not its level.

### The port

`transient_looper.rs` keeps the object's words in FL's layout so that the
code reads as the DLL's `compute`. It matches the emulator bit for bit
over two seconds in five settings (probabilistic and deterministic draws,
Adaptive, Bouncing with the beat, and the demo song's), including the
FMA3 paths of `tanf`, `expf` and `powf` that `low_lifter.rs` transcribes.
Approximated: `logf` and `sinf` are the f64 functions rounded once; the
auto-pan table is then one bit off the runtime's SSE2 `sinf` in 94 of
65536 entries (only a negative spread reads it). The shell's beat buffer
is filled with the place in the bar once a block (`inferred`, the fill
routine at `0x007711d8` is not read), which only Tempo mode with a
trigger reads. After a rate change the state's source and architecture
are sent again (`inferred`); every FL state has the defaults, both sides
and stereo.
