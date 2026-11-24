# Fruity Voltage Controller: state, sources, the voltages it puts out

A generator that makes control voltages: DC on its outputs, for a
DC-coupled audio interface to drive modular gear with ("Enable output;
Warning, generates continous DC signals !"). Recovered from
`Plugins/Fruity/Generators/Fruity Voltage Controller/Fruity Voltage
Controller_x64.dll` (FL Studio 26.1.7, Delphi native plugin, image base
0x400000), headless Ghidra. Corpus: the render test (`Fruity Voltage
Controller.flp`, 115 bytes, version 2, ON) and four copies of one preset
(103 bytes, version 1, off).

Confidence tags: `code`, `dfm`, `corpus`, `inferred`.

## How the plugin is wired

`CreatePlugInstance` @ `0x00702900` -> constructor `0x00703890`, class
reference `0x007027e8` (`TTestPlug`, 0x560 bytes) `code`.

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x00702950 |
| 3 | SaveRestoreState | 0x00702bd0 |
| 6 | ProcessParam | 0x00703fb0 |
| 8 | Gen_Render | 0x00706180 |
| 9 | TriggerVoice | 0x007058e0 |
| 10 | Voice_Release | 0x00705eb0 |
| 11 | Voice_Kill | 0x00705ef0 |
| 14 | NewTick | 0x007057f0 |
| 25 | the editor's timer | 0x00705610 |

Fields `code`:

| offset | meaning |
|---|---|
| 0x120 | i32[16]: 0..7 each output's offset, 8..15 each output's scale |
| 0x160 + 3 i | per output i (0 main L, 1 main R, 2..7 outputs 2 to 4): source, colour, unipolar offset |
| 0x178 | ON |
| 0x179 | output range: 0 -1..1, 1 halved, 2 halved and lifted to 0..1 |
| 0x17a | pitch: 0 an octave a volt, 1 a semitone a volt, other none |
| 0x184 | C5, `440 exp(3 ln 2 / 12)` Hz |
| 0x18c | f64 pitch scale, 1 |
| 0x194 + 0x38 c | the values of colour c (16): f64 pitch, pan, velocity, volume, mod X, mod Y at +0, +8, +0x10, +0x18, +0x20, +0x28; u8 gate +0x30, trigger +0x31; i32 trigger frames left +0x34 |
| 0x514 | pulse length in frames, 15 |
| 0x518..0x530 | the clock: tick count, frames left, on, PPQ (96), division (24), playing |
| 0x534 | i32[3]: the mixer tracks outputs 2 to 4 go to, 0 none |

## 1. State (event 213)

`SaveRestoreState` `code`; save writes version 3. Load reads the version;
from 4 nothing else.

| bytes | what |
|---|---|
| 0..3 | version (1, 2 or 3) |
| 4..67 | the 16 parameters as i32 |
| 8 | the sources |
| 1, 1, 1 | ON, the range, the pitch mode |
| 8 | the polarities (0 bipolar) |
| 12 | the three tracks |
| 4 | the clock's division |
| 4 + 8 | version 2 on: the pulse length, the pitch scale (f64) |
| 8 | version 3 on: each output's colour, 0..15 (`LeftChanColor` 1..16 in the editor); 0 before |

Lengths: version 1 103 bytes, 2 115, 3 123 `corpus`.

Sources (`0x00704c60`): 0 pitch, 1 gate, 2 trigger, 3 velocity, 4 volume,
5 pan, 6 mod X, 7 mod Y, 8 clock, 9 none. Defaults: offsets 32768, scales
65536, sources 0, 1, 3, 5, 6, 7, 9, 9, everything else 0 but the pitch
scale 1, pulses of 15 frames, division 24.

## 2. Parameters

The wheels (`dfm`): offsets and scales 0..65536. An output's offset
(`0x007054f0`) is `v / 65536 * 2 - 1`, or `v / 65536` when unipolar; its
scale (`0x00703190`) `float(v * 2^-16 * 2) - 1`.

## 3. Notes

`TriggerVoice` asks the host for the note's colour (`FPV_GetColor`) and
fills that colour's values from the voice's levels (`0x00705ba0`), opens
its gate, starts its trigger with the pulse length, and keeps the voice in
a list. With `key = (FinalLevels.Pitch + 6000) * 0.01`:

```
pitch   mode 0: scale * log2(exp((key - 57) ln 2 / 12) 440 / C5) / 10     // (key - 60) / 120: a volt an octave, 10 V full scale
        mode 1: scale * (key - 60) / 10
pan     FinalLevels.Pan in -1..1;  volume FinalLevels.Vol in 0..1
velocity ln(10 InitLevels.Vol + 1) / ln 11 / 1.28 in 0..1                    // vel / 128
mod X, mod Y  FinalLevels.FCut, FRes in -1..1
```

`NewTick` runs it again for every voice in the list (and counts the
clock: a pulse every `PPQ / min(PPQ, division)` ticks while playing).
`Voice_Release` closes the colour's gate and has the host kill the voice,
which `Voice_Kill` drops from the list.

## 4. Render (`Gen_Render` @ 0x00706180)

Off: the triggers and the clock stop; nothing is added. On:

1. The main output's left and right each get one voltage for the whole
   block: `(source + offset) * scale`, then the range: clamped to -1..1,
   or halved and clamped, or halved, lifted by 0.5 and clamped to 0..1.
2. A trigger (or clock) output adds `scale + offset` on the frames its
   pulse has left, up to the block.
3. Outputs 2 to 4 do the same into the buffers of their mixer tracks.
4. The clock's and each trigger's frames left go down by the block.

## Implementation notes (crates/plugins/src/voltage_controller.rs)

- Every corpus state loads; the render test plays its pitch on the left
  and its gate, at half scale, on the right.
- The colour is the note's (`NoteOn::color`); a moved pitch
  (`set_note_pitch`) is what `NewTick` reads again.
- Left out, and said by `fidelity()`: outputs 2 to 4 (this crate's
  generators have one output; no corpus state routes one), and the clock,
  which counts ticks a generator here is not given (no corpus state uses
  it). The f64 `ln` and `exp` are Rust's where FL's are Delphi's: equal
  but for the last bit, which the f32 output does not show.
