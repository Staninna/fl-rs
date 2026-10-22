# Fruity X-Y Controller: a moving point whose X and Y drive links

Fruity X-Y Controller is a pad with a point on it. Its X and Y levels say
where the point goes, the point moves there at a speed and with an
acceleration of its own, once a tick, and its two coordinates, each
through a tension curve and onto a range, are two internal controller
outputs that a project links to other controls. It has no sound path. In
fl-rs it is `XyController` (`crates/plugins/src/xy_controller.rs`), whose
`controller_outputs` the engine carries along the 227 links like Fruity
Peak Controller's.

Read from `Fruity X-Y Controller_x64.dll` (FL Studio 26.1.7, Delphi, image
base 0x400000) with headless Ghidra (`/root/ghidra/ctlsurf`, decompiles of
the addresses below) and capstone listings. Tags: `asm` read from the
binary, `dfm` from the editor's form, `corpus` from the shipped projects,
`inferred` where the binary does not say.

## The plugin `asm`

`CreatePlugInstance` @ `0x6c5710` constructs `TTestPlug`, class reference
`0x6c5530`, VMT `0x6c55f8` (instance 0x1b8 bytes, parent
`TDelphiFruityPlug`). `TFruityPlugInfo` @ `0x718868`: "Fruity X-Y
controller" / "X-Y ctrl", Flags `0x10400060` = `FPF_WantNewTick |
FPF_NoProcess | FPF_Reserved1` and bit 28, 10 parameters, 2 output
controllers.

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x6c5760 |
| 3 | SaveRestoreState | 0x6c5d20 |
| 4 | GetName | 0x6c6a10 |
| 6 | ProcessParam | 0x6c6650 |
| 7 | Eff_Render | 0x409ec0, the RTL's abstract-method stub |
| 14 | NewTick | 0x6c6d90 |

`FPF_NoProcess` tells FL never to call `Eff_Render`, so the slot's block
passes on as it came.

## State `asm` `corpus`

`SaveRestoreState` reads, in order:

| bytes | what |
|---|---|
| 4 | version, 2 when saved by FL 26 |
| 40 | the ten parameters, i32, into `0x120` |
| 1 | the axis the editor shows (`0x148`) |
| 1 + n | the grid's name, a Delphi short string (`0x6c5bc0`) |
| 1 or 4 | the grid: a byte, less 1, before version 2, an i32 after |
| 20 | five words for the joystick (`0x190`) |

It then sets every parameter again through `ProcessParam` with
`REC_UpdateValue | REC_UpdateControl` (0x11). All 19 corpus states are of
version 2 and 77 or 81 bytes; `set_state` takes any version and refuses a
state of another length than the layout gives.

## Parameters `asm` `dfm`

The editor shows one axis at a time with four wheels (`MinWheel`,
`MaxWheel`, `LvlWheel`, `PeakLogWheel`) and two for both (`SpeedWheel`,
`AccelWheel`). `ProcessParam` splits a parameter under 8 into axis
`n / 4` and wheel `n % 4` (`0x5ae7b0`).

| parameter | wheel | range | default |
|---|---|---|---|
| 0 / 4 | X / Y level | 0 to 65536 | 32768 |
| 1 / 5 | X / Y min | 0 to 65536 | 0 |
| 2 / 6 | X / Y max | 0 to 65536 | 65536 |
| 3 / 7 | X / Y tension | -128 to 128 | 0 |
| 8 | Speed | 1024 to 2048 | 1536 |
| 9 | Acceleration | 256 to 1024 | 1024 |

Each axis keeps three singles at `0x15c + 12 * axis`: where the point is,
where it goes, and its velocity. With `REC_UpdateValue` `ProcessParam`
stores the value at `0x120 + 4n`, and then:

- A level sets where the point goes: `level * (1/65536)` as singles
  (`0x705220`).
- A tension (`0x6c6360`) sets the axis's curve: its sign `t` at `0x154`
  and, when it is not 0, its depth `d = (Exp(Ln(1001) * |t / 128|) - 1) *
  0.1` at `0x14c`, the `|t / 128|` a single.
- Speed and acceleration (`0x6c6470`) set the step and the carry. Speed
  2048 is a step of 1.0; otherwise `((Exp(Ln(1e8) * s / 2304) - 1) * 1e-8
  + 1e-4) * 96 / PPQ` at `0x178`. Acceleration 1024 is a carry of 0;
  otherwise `Power(1 - (Exp(Ln(1001) * a / 1024) - 1) * 0.001, 96 / PPQ)`
  at `0x174`. The PPQ (`0x184`, 96 until told) comes with
  `FPD_SetTimeSig` (14) in the dispatcher, which sets both again.
- Min and max are only stored; the tick reads them.

With `REC_FromMIDI` (0x20) the value is a host's 0 to 2^30 and is first
put on the wheel's range, `min + Round(v * 2^-30 * (max - min))`.

## NewTick `asm`

Once a tick (`0x6c6d90`), with `dx`, `dy` from the point to where it goes:

1. The heading `0x6c6cb0`: `-3π/2 - atan(dx / dy)` for `dy > 0`,
   `-π/2 - atan(dx / dy)` for `dy < 0`, and for `dy = 0` 0 or π (as a
   single) by the sign of `dx`; its `SinCos` gives the direction, cos for
   X and sin for Y.
2. The distance, `sqrt(dx² + dy²)` (`0x6c6d60`).
3. The carry, eased within 0.3 of the target: `carry * (1 - (Exp(Ln(1e10)
   * (1 - dist * 3.3333333)) - 1) * 1e-10)`.
4. The step, `speed * (1 - carry)` with the carry before easing, but no
   more than the distance.
5. For each axis: `velocity = velocity * carry + direction * step`,
   `at += velocity`.
6. The curve: for `t > 0`, `1 - ((d + 1)^(1 - at) - 1) / d`; for `t < 0`,
   `((d + 1)^at - 1) / d`; for 0, `at`. Each power is `Exp(Ln(d + 1) *
   x)`.
7. `value = clamp(min + Round(curve * (max - min)), min, max)`, an
   integer, with `max` winning when min is above it.
8. When the value is not what was sent last (`0x17c`), it goes to the
   host's `OnControllerChanged` (host method `+0x98`) as output `axis`,
   `Round(value * (1/65536) * 2^30)` in singles: X is output 0, Y output
   1, from 0 to 1 once FL divides by 2^30.

So at speed 2048 and acceleration 1024 the point jumps to its levels in a
tick (two for a jump longer than the pad's side), and the outputs follow
the levels at once. Touchscreen's pads glide, at speed 1900: 0.0397 of the
pad a tick at 96 PPQ.

Every float operation is done in the precision the listing has: singles
where FL keeps singles, doubles through the RTL's `Ln` (`0x409870`), `Exp`
(`0x408810`), `ArcTan` (`0x409be0`), `Sqrt` (`0x409cf0`), `SinCos`
(`0x4095e0`) and `Power` (`0x45d280`, which multiplies for an integral
exponent), and `Round` as `cvtsd2si`, ties to even. The Rust has the
standard library's `ln`, `exp`, `atan`, `sqrt` and `sin_cos` for the RTL's,
which may differ in the last bit of a double; every output is a whole
number of 65536ths, so that only shows where a coordinate falls within an
ulp of a half step.

## Start and flush `asm` `inferred`

The dispatcher's `FPD_Flush` (2) puts every point where it goes and sets
both last-sent values to -1, so the next tick sends both outputs.
`Effect::reset` does that. A loaded state starts the same way here: the
point at its levels and both outputs sent on the first tick. In FL the
point starts at 0 on a new instance and FL calls `NewTick` while the song
is stopped too, so by the time a song plays from a loaded project the
point has reached its levels; that is inferred, not read.

## Corpus `corpus`

Three projects use it, 19 instances, 60 links from it:

- Surround mix: 16 pads, "1" to "16", at X and Y 0.5, speed 2048. Each
  links X to its insert's pan, and Y twice through `Tension()` formulas to
  its insert's sends to inserts 17 and 18. The engine reads `Tension()`,
  so all 48 links are applied.
- Touchscreen: "Low Pass" at (1.0, 0.5) and "High Pass" at (0, 0), speed
  1900, each X and Y to parameters 6 and 7 of the effect next to it on
  its insert (4 links, all applied).
- Drumloop Masher: "Pitch" at (0.5, 0), its X to 8 channels' control
  32773, which the engine does not play. A clip drives its parameter 0,
  the X level, and moves the point through `set_param`.

## What is not FL's

- The joystick input (the editor's `EnableJoyBtn`, the five words at the
  end of the state) is not built. No corpus state enables it.
- The start, as above.
- The editor and its grid.
