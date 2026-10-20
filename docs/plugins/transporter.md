# Transporter: state, parameters, the shell

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
Emphasis and Pitch Shifter use). The install this was read from has no
`Shared/` directory, so the engine is unmapped and `crates/plugins` has
only the shell: `transporter.rs` reads the state and works out the values
the shell hands the engine. Nothing hosts the plugin yet. `.scratch/notes/transporter.md`
has the way on.

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
each parameter's default and inverts the maps above, so they live in the
unmapped engine. A fresh plugin saves the render test's state: levels
12800 (0 dB), biases 0, spread 512, minimum length 500 ms, ratio 8 (3),
sensitivity 512, bouncing 512, the switches 0, length 1/1 `corpus`.
Parameter 20's default is not known; the form says 0.

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

## 4. Signal chain

Not mapped: it is all inside `TransientLooper` in `ildsp_x64.dll`.
