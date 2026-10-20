# Fruity X-Y Controller: no sound path; its outputs drive links

Fruity X-Y Controller is a pad whose X and Y are two internal controller
outputs: a project links them to other controls. It has no sound path. In
fl-rs it is a `PassThrough` (`crates/plugins/src/pass_through.rs`) that
leaves the block alone, as FL does, and says in its `fidelity()` that its
outputs are not driven: the links from it play nothing yet.

Read from `Fruity X-Y Controller_x64.dll` (FL Studio 26.1.7, Delphi, image
base 0x400000) with capstone listings; tag `asm`.

## The sound `asm`

`CreatePlugInstance` @ `0x6c5710` constructs the class whose reference is at
`0x6c5530`, VMT `0x6c55f8`: Dispatcher `0x6c5760`, slot 7 `Eff_Render`
`0x409ec0`, the RTL's abstract-method stub (`mov ecx, 0xd2`, runtime error
210). `TFruityPlugInfo` @ `0x718868`: "Fruity X-Y controller" / "X-Y ctrl",
Flags `0x10400060` = `FPF_WantNewTick | FPF_NoProcess | FPF_Reserved1` and
bit 28, 10 parameters, 2 output controllers. `FPF_NoProcess` tells FL never
to call `Eff_Render`, so the slot's block passes on as it came.

## What is left out

- Its two outputs. All three projects that use it link from it (60 links:
  Drumloop Masher 8, Surround mix 48, Touchscreen 4); none of them is
  driven. Where X and Y come from, the pad's position, its parameters and
  any smoothing, is in its state and `ProcessParam`, not read here.
- Its parameters and its window.
