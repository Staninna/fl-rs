# Control Surface: no sound path; its outputs drive links

Control Surface is a panel of knobs, sliders, pads and X-Y pads whose values
are internal controller outputs: a project links them to other controls,
and that is all it is for. It has no sound path. In fl-rs it is a
`PassThrough` (`crates/plugins/src/pass_through.rs`) that leaves the block
alone, as FL does, and says in its `fidelity()` that its outputs are not
driven: the links from it play nothing yet.

Read from `Control Surface_x64.dll` (FL Studio 26.1.7, Delphi, image base
0x400000) with capstone listings; tag `asm`.

## The sound `asm`

`CreatePlugInstance` @ `0x852750` constructs the class whose reference is at
`0x84fe40`, VMT `0x84ff08`: Dispatcher `0x853250`, slot 7 `Eff_Render`
`0x40c900`, the RTL's abstract-method stub (`mov ecx, 0xd2`, runtime error
210). `TFruityPlugInfo` @ `0x8c74c8`: "Control Surface" / "Surface", Flags
`0x404060` = `FPF_WantNewTick | FPF_NoProcess | FPF_MIDIOut |
FPF_Reserved1`, 2048 parameters, 2048 output controllers. `FPF_NoProcess`
tells FL never to call `Eff_Render`, so the slot's block passes on as it
came.

## What is left out

- Its outputs. All 17 projects that use it link from it, 174 links in all
  (10 or 12 each in Breakbeat, Chillout, Trap and the other genre
  projects, 14 and 6 in the two `nucleon` demos); none of them is driven. The values are its controls', saved in
  its state, which is not read here.
- Its MIDI out, its parameters and its window.
