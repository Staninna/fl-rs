# Control Surface: a panel whose values are controller outputs

Control Surface is a panel of knobs, sliders, buttons, check boxes, digit
wheels and X-Y pads. Each value on it is a parameter, and parameter `n` is
internal controller output `n`: a project links them to other controls,
and that is all it is for. It has no sound path. In fl-rs it is
`ControlSurface` (`crates/plugins/src/control_surface.rs`), whose
`controller_outputs` the engine carries along the 227 links like Fruity
Peak Controller's.

Read from `Control Surface_x64.dll` (FL Studio 26.1.7, Delphi, image base
0x400000) with headless Ghidra (`/root/ghidra/ctlsurf`, decompiles of the
addresses below) and capstone listings. Tags: `asm` read from the binary,
`corpus` from the shipped projects, `inferred` where the binary does not
say.

## The plugin `asm`

`CreatePlugInstance` @ `0x852750` constructs `TTestPlug`, class reference
`0x84fe40`, VMT `0x84ff08` (instance 0xa160 bytes, parent
`TDelphiFruityPlug`). `TFruityPlugInfo` @ `0x8c74c8`: "Control Surface" /
"Surface", Flags `0x404060` = `FPF_WantNewTick | FPF_NoProcess |
FPF_MIDIOut | FPF_Reserved1`, 2048 parameters, 2048 output controllers.

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x853250 |
| 3 | SaveRestoreState | 0x854f70 |
| 6 | ProcessParam | 0x8549a0 |
| 7 | Eff_Render | 0x40c900, the RTL's abstract-method stub (runtime error 210) |
| 14 | NewTick | 0x854880 |

`FPF_NoProcess` tells FL never to call `Eff_Render`, so the slot's block
passes on as it came. The dispatcher's `FPD_Flush` (2) calls `0x853510`,
which does nothing.

## State `asm` `corpus`

`SaveRestoreState` reads a version word. Version 0 or 1 is chunks
(`0x853b90`), -1 the older records (`0x853850`), anything else is left
alone. All 17 corpus states are version 1.

A chunk is an id (u32), a length (u64) and its bytes. At the top: 2000
(64 bytes of panel settings), `0x7d2` (the panel's size), `0x7d3` (one
word) and `0x834`, which starts a control. A control's chunks
(`0x854650`) run to `0x835`:

| id | what |
|---|---|
| 0x834 | 32 bytes, the first word the control's type |
| 0x837 | its name, UTF-16 |
| 0x838 | its place and size, four singles |
| 0x839, 0x83a, 0x83b | its look, as Delphi form text |
| 0x836 | one value: a single from 0 to 1, a single default, and the parameter it is (i32, -1 for none) |

The type picks the control (`0x844720`) and how many values it has (each
constructor's call to `0x8489f0`): 0 knob 1, 1 X-Y pad 2 (X, Y), 2 slider
1, 3 button 1, 4 check box 1, 5 bevel 0, 6 label 0, 7 digit wheel 1, 8
keyboard 0. The loader goes through a control's values up to that count,
sets each with `SetValue` and its default, and then registers the
parameter (`0x8527a0`): the table at `0x4150 + 12n` holds the control and
the value for parameter `n`, and `0x414c` the highest parameter in use. A
later control naming the same parameter takes it over.

A state of version -1 is a count and records of 0x148 bytes, read while
one is left and the count, counted down, is not 0: the name (0x104
bytes), the type at 0x108, two values of 20 bytes from 0x110 (the value at
+8, the default at +12, and at +16 whether it is a parameter), the place
at 0x138. A value that is a parameter takes the first free one
(`0x853480`). No corpus state is of this version.

## Values, parameters and outputs `asm`

Every value control shares `SetValue` (`0x848970`; the X-Y pad's
`0x84a040` calls it and moves its point): it calls the panel's handler
with the new value, then stores it as it came, a single. The handler
(`0x8556e0`) finds the first parameter that names the control and the
value (`0x8534b0`) and queues `Round(value * 2^30)` (a single product,
then `cvtsd2si`) for it at `0x2148 + 4n`, raising a flag. `NewTick`
(`0x854880`) takes the queue when the flag is up, puts -1 back in every
place, and calls the host's `OnControllerChanged` (`+0x98`) with each
value that was queued, output `n`. So output `n` is parameter `n`'s value
from 0 to 1, sent when the value is set, once.

`ProcessParam` takes the value as a single from 0 to 1, or with
`REC_FromMIDI` (0x20) as a host's 0 to 2^30, divided by 2^30 in doubles
and kept as a single. With `REC_UpdateValue` it sets the value through
`SetValue`; the value it gives back is for the display, put on the
control's range when its flags allow. In fl-rs a parameter is in 2^30ths,
kept from 0 to 2^30: what an automation clip drives through the
`REC_FromMIDI` path.

## The start `asm` `inferred`

While the loader sets the values the parameters are not registered yet,
so the handler finds none and loading queues nothing: FL's targets keep
the values the project saved for them until a value is set. Here a loaded
state queues every value, and the first tick sends them, so the linked
controls take the values the controller's state gives from the start. In
every corpus link the two agree: the targets were saved as the links had
put them.

## Corpus `corpus`

17 projects, 133 values, 174 links from them:

- 13 genre templates (Breakbeat, Chillout and the rest): 9 or 10 values
  each, buttons and three knobs. Their links, most through formulas like
  `1-Input*0.8-0.2`, set parameters of the effects in slot 9 of the
  template's inserts and two of the master's; one or two set an insert's
  stereo separation (control 8130), which the engine does not play.
- `nucleon - Fruity Delay 3` and `nucleon - Transient Processor`: one
  button on parameter 0 at 1, linked to 14 slots' mix and to 6 effects'
  parameter 8.
- Surround mix: four sliders at 0 on the sends from inserts 17 and 18 to
  inserts 19 and 20.
- Asher Postman - Future House: one button, "Transition FX", at 0, linked
  to two slots' switch. A clip drives it (parameter 0), through
  `set_param`.

## What is not FL's

- The start, as above.
- Its MIDI out, the keyboard control, and its window.
