# Fruity Wrapper: the VST builds of FL's own effects

Fruity Wrapper is FL's VST host. Every one of the 36 Fruity Wrapper states in
FL's projects (13 render tests and 23 mixer slots in two ZGameEditor
Visualizer demo songs) holds no third-party plugin: each holds the VST build
of one of FL's own effects, which FL 11, 12 and 13 shipped under
`Plugins\VST\Fruity <name>.dll`. FL 2026 has no such VSTs. Its project loader
replaces each with the effect's own build before the wrapper DLL is ever
asked to load anything, so what plays is the native effect, and the wrapper
DLL (`Fruity Wrapper_x64.dll`) does not take part.

Read from `FLEngine_x64.dll` (FL Studio 26.1.7, image base 0x400000),
imported into Ghidra without analysis and decompiled function by function
(`/root/ghidra/wrap/`), and from the five effects' `ProcessParam` and
`Dispatcher`. Confidence tags: `asm` (read in the listing), `corpus`.

## 1. The swap table (`0xaa2cb0`) `asm`

At start-up FL registers renames, each with a kind, a target kind, a flag
and a conversion function (`0xaa34c0`, keyed `"%d|%s"` on the kind and the
lower-cased name by `0xaa3780`):

| from | to | kind | conversion |
|---|---|---:|---|
| Newtone | Newtime | 2 | `0xaa29e0` |
| Fruity Soundfont Player | Soundfont Player | 2 | none |
| Fruity 7 Band EQ, Fruity 7 Band Equalizer | Fruity 7 Band EQ | 0 | `0xaa28e0` |
| Fruity Balance, Bass Boost, Blood Overdrive, Center, Chorus, Compressor, Delay, Fast LP, Filter, Flanger, Free Filter, Mute 2, Phase Inverter, Phaser, Reeverb | the same name | 0 | `0xaa28e0` |

`wrapper::NATIVE` is the last row.

## 2. When the loader swaps (`0xaa38d0`, called from `0xf46490`) `asm`

For each plugin the project loads, `0xaa38d0` lower-cases its event 201
name. When it is `fruity wrapper`, `0xaa3850` reads the wrapper's state
(`0xa9fb30`) and takes the name and the kind from it instead. It looks the
pair up and calls the entry's conversion. `0xaa28e0` reads the state again,
with the VST chunk this time, sets the plugin to load to the entry's target
name (following one more rename, `0xaa2c00`), and keeps the chunk's
parameter count and a copy of its floats at `ctx+0x10` and `ctx+0x14`. The
caller's flag that would skip them is 0 (`0xf51150`).

## 3. The wrapper's state (`0xa9fb30`, `0xa9f840`) `asm` `corpus`

An i32 version first. -1 and 13 and up are no state. 0 to 4 are an older
layout read by `0xa9f7f0` (not read here; no project has one). 5 to 12
are a run of tags, each `i32 tag, i64 length, bytes` (`0x8e9180`; a tag 0
is a header to read past). Every corpus state is version 8 with these tags:

| tag | length | meaning |
|---|---:|---|
| 0x01, 0x02, 0x1e, 0x1f, 0x20 | 20, 25, 16, 12, 12 | the wrapper's own settings, not read by the swap |
| 0x32 | 16 | first byte: the kind the name is looked up under, 0 in all 36 |
| 0x33 | 4 | the VST's unique ID, e.g. `laBF` for Balance |
| 0x36 | n | the plugin's name, `Fruity Balance` |
| 0x37 | n | the VST's path, `C:\Program Files (x86)\Image-Line\FL Studio 11\Plugins\VST\Fruity Balance.dll` |
| 0x35 | n | the VST's chunk, read only when the conversion asks (`0xa9f3c0`) |

The chunk (`0xa9f3c0`): an i32 `v`; 123456 and anything from -12 down are no
chunk. From `v < -5` a byte; from `v < -4` an i32 that is -2 before an i64
program number and the number itself otherwise; from `v < -5` one more i32.
The count is the next i32 when `v` is -2, or `v < -4` and the program
number is 0 or -1; otherwise it is `v`. A count of 0 or more is followed by
that many f32, the VST's parameters from 0 to 1. FL 11's chunks are
`-9, 5, 0, 0, count`; FL 12's and 13's `-9, 5, -2, 0 (i64), 0, count`. What
follows the floats (a 1 and the program name) is not read.

## 4. What the swapped effect gets (`0xf46490`, around `0xf4c142`) `asm`

The loader makes the native effect at its defaults: the wrapper's state is
not handed to it. Then, for `i` below the smaller of the chunk's count and
the effect's `NumParams`:

    value = Round(f[i] * (plugin+0xa9 ? 2^30 : 2^16))     // FUN_0040c660 = cvtsd2si
    ProcessParam(i, value, 0x31)                         // REC_UpdateValue | REC_UpdateControl | REC_FromMIDI

`plugin+0xa9` is what the effect answers to `Dispatcher` ID 0x32
(`FPD_UseIncreasedMIDIResolution`, stored at `0x132f061`). The SDK base
class every one of FL's effects is built on answers 1 (`Fruity
Balance_x64.dll` `0x6b9ae3`), so the scale is 2^30. Each effect's
`ProcessParam` then takes `REC_FromMIDI` back across its control's range,

    v = Min + Round(value * 2^-30 * (Max - Min))

with `Min` and `Max` the wheel's (`+0x3a8`, `+0x3ac`) and the same
`cvtsd2si` rounding, half to even: Balance `0x6beb02`, Chorus `0x6c0a82`,
Compressor `0x6bded2`, Fast LP `0x6c882a`, Filter `0x6cbd6a`. The value is
not clamped; every corpus float is between 0 and 1, which keeps it inside
the range, and the Rust effects clamp what is outside it.

`wrapper::from_vst` is the two roundings, and `Wrapper::set_state` makes the
effect with `crate::by_name`, tells it the rate, tempo and PPQ, and moves its
parameters with `Effect::set_param`, each effect's `ProcessParam` with
`REC_UpdateValue`. The ranges are what `set_param` clamps to, taken from a
second instance so the first moves only to FL's values.

## 5. The corpus `corpus`

| where | wrapped effect | uses | hosted |
|---|---|---:|---|
| render tests | Balance, Chorus, Compressor, Fast LP, Filter | 5 | yes |
| render tests | Bass Boost, Center, Flanger, Free Filter, Mute 2, Phase Inverter, Phaser, Reeverb | 8 | no, not built |
| Treben - Gimbal and Sinan - RawFL | Compressor x11, Chorus x2 | 13 | yes |
| Treben - Gimbal and Sinan - RawFL | Flanger | 5 | no, not built |
| JuiceBass - SeamlessR - Menagerie | Fast LP x2, Chorus x3 | 5 | yes |

No channel holds a Fruity Wrapper: the generator row's two projects are
these two songs' mixer slots. A wrapper that holds a plugin not built here
or a third-party VST is turned down (`StateError::Unhosted`) and its slot
is passed through.
