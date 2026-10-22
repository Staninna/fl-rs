# Fruity Keyboard Controller: a channel whose notes drive links

Fruity Keyboard Controller is a generator that makes no sound. Every key
has a value from 0 to 1; a note on its channel sends its key's value as
output 0 and its velocity as output 1, glided towards once a tick. A
project links the outputs to other controls. In fl-rs it is
`KeyboardController` (`crates/plugins/src/keyboard_controller.rs`), a
`Generator` whose `controller_outputs` the engine carries along the 227
links whose source is the channel's control `0x8000 + n`.

Read from `Fruity Keyboard Controller_x64.dll` (FL Studio 26.1.7, Delphi,
image base 0x400000) with headless Ghidra and capstone. Tags: `asm` read
from the binary, `dfm` from the editor's form, `corpus` from the shipped
projects, `inferred` where the binary does not say.

## The plugin `asm`

`CreatePlugInstance` @ `0x6fccd0` constructs `TTestPlug` (`0x6fd340`),
VMT `0x6fcbb0` (class reference `0x6fcae8`, instance 0x388 bytes, parent
`TDelphiFruityPlug`). `TFruityPlugInfo` @ `0x750548`: "Fruity Keyboard
Controller" / "Kb Ctrl", Flags `0x10600460` = `FPF_WantNewTick |
FPF_NoProcess | FPF_NoWindow | FPF_NewVoiceParams | FPF_Reserved1` and
bit 28 (the one the X-Y Controller has too), 2 parameters, 2 output
controllers.

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x6fcd60 |
| 3 | SaveRestoreState | 0x6fcff0 |
| 5 | ProcessEvent | 0x6fd680 |
| 6 | ProcessParam | 0x6fd750 |
| 7, 8, 13 | Eff_Render, Gen_Render, Voice_Render | 0x40a1a0, the RTL's abstract-method stub |
| 9 | TriggerVoice | 0x6fdbd0 |
| 10 | Voice_Release | 0x6fdd90 |
| 11 | Voice_Kill | 0x6fddf0 |
| 12 | Voice_ProcessEvent | 0x6fde80, returns 0 |
| 14 | NewTick | 0x6fe180 |

No render method is the plugin's own, so the channel is silent.

## State `asm` `corpus`

`SaveRestoreState` reads, in order:

| bytes | what |
|---|---|
| 4 | version, 3 when saved by FL 26 |
| 8, 4 before version 2 | the two wheels, i32, at `0x128`; before version 2 the release is the attack |
| 528, 480 in version 0 | the key values, a single per key 0 to 131, at `0x144` |
| 8 | the keyboard's selection, its low and high key (`0x6fb410` on the editor's keyboard, then `0x6fed30` puts it into `0x13c..=0x140`) |
| 4 | the note off value, a single at `0x354` |
| 1 | "Note off value" switched on, `0x370` |
| 1, from version 3 | the link button between the wheels, editor only |

It then sets both parameters again through `ProcessParam` with
`REC_UpdateValue | REC_UpdateControl`. All 10 corpus states are version 2,
553 bytes. `0x6fed30` takes the selection as the zone unless it covers the
keyboard's whole range, when the zone is that range; the keyboard's range
is not read here, and fl-rs keeps the selection inside keys 0 to 131
(`NoteSelect`'s maximum, `dfm`).

## Parameters `asm` `dfm`

| parameter | wheel | range | default |
|---|---|---|---|
| 0 | `SpeedAttWheel`, attack | 0 to 256 | 32 |
| 1 | `SpeedRelWheel`, release | 0 to 256 | 32 |

`0x6fdea0` turns each into how much of the distance to the target stays
a tick, at `0x368` (attack) and `0x36c` (release):

```
time(x) = (single)(Exp(Ln(11) * (single)(x / 256)) - 1) * 0.1      // 0x6fd6d0, seconds
tick    = Ln(0.01) * 60 / (single)(tempo * PPQ)                      // 0x760268 = Ln(0.01), 0x5e2d70
keep    = time > 0 ? (single)Exp(tick / time) : 0
```

So the output gets within 1% of its target in the wheel's time, and a
wheel at 0 jumps. The tempo is a single at `0x134`, from `FPE_Tempo`
(`ProcessEvent` event 0); the PPQ at `0x130` comes with `FPD_SetTimeSig`
(14). Both work the glides out again. With `REC_FromMIDI` a parameter
takes `min + Round(v * 2^-30 * (max - min))`.

## Notes `asm`

`TriggerVoice` reads the voice's `InitLevels` (`TVoiceParams`): `key =
max(0, Round(Pitch / 100))`, where `Pitch` is `key * 100` from the note
record (`docs/plugins/host-voice.md`). A key outside the zone gets no
voice: the plugin asks the host to drop it. Inside, it keeps a 20-byte
voice with:

- value: the key's value, which becomes output 0's target (`0x35c`);
- velocity: `Ln(min(Vol * 10, 20.551182) + 1) / Ln(20.551182 + 1)`,
  output 1's target (`0x364`). `Vol` is `(11^(vel/100) - 1) / 10`, so
  this is `vel / 128` up to rounding, and velocity 128 is 1.

The voices are kept in a list in the order they came. `Voice_Release` asks
the host to kill the voice at once, and `Voice_Kill` takes it from the list:
then the targets are the last voice's value and velocity, or with no voice
left and the note off value on, output 0's target is that value; with it
off they stay.

## NewTick `asm`

For each output, when it is not at its target and the target is set
(not -1): an output that is not set (-1) jumps to it, one above it glides
down with the release's `keep` and one below glides up with the attack's,
`at = target + (at - target) * keep` in singles. FL compares the two as
integers, which for singles from 0 up is the order of the numbers. The new
value goes to the host's `OnControllerChanged` (host method `+0x98`) as
`Round(at * 2^30)`.

## Start and flush `asm` `inferred`

The constructor and the dispatcher's `FPD_Flush` (2) set both outputs and
both targets to -1, and with "Note off value" on output 0's target to the
note off value, so the next tick sends it. `Generator::reset` does that,
and so does a loaded state here (`inferred`: FL flushes a plugin when the
song starts).

## Corpus `corpus`

Drumloop Masher has ten, all with both wheels at 0 and "Note off value"
on at 0: "Glitch" (keys 48 to 83 from 0 to 1) to parameter 0 of insert
1's slot 1, "Filter" (59 to 67) to parameter 6 of slots 3 and 4, and
eight "Delay send" and "Verb send" channels (key 60 only, value 1) to four
parameters of insert 2's slot 0 through `Input*0.5+0.5` and to four send
levels of insert 1. All 11 links are played.

## What is not FL's

- The key a note has here is the host's, with the channel's root note and
  pitch on it; FL's plugin reads the piano roll's own key. They are the
  same for a channel at C5 with no pitch.
- FL's voice list has no end; fl-rs keeps the last 64 held notes.
- The keyboard's whole range, which takes the zone's place when the
  selection covers it, is taken to be keys 0 to 131.
