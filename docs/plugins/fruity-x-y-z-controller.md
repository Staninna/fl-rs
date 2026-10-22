# Fruity X-Y-Z Controller: three mapped, smoothed values that drive links

Fruity X-Y-Z Controller holds three values, X, Y and Z. Each goes through
a map when it is set, eases towards that point once a tick, goes through a
second map and is one of three internal controller outputs. It has no
sound path. In fl-rs it is `XyzController`
(`crates/plugins/src/xyz_controller.rs`). No shipped project has one in a
mixer slot: all 32 in the projects sit inside Patchers, wired to Control
Surfaces, Limiters, PanOMatics and Stereo Shapers, and Patcher's presets
hold 467 more.

Read from `Fruity X-Y-Z Controller_x64.dll` (FL Studio 26.1.7, Delphi,
image base 0x400000) with REA's Ghidra. Tags: `asm` read from the binary,
`dfm` from the editor's form, `corpus` from the shipped files, `inferred`
where the binary does not say.

## The plugin `asm`

`CreatePlugInstance` @ `0x83b070` builds the plugin at `0x83b8d0`, VMT
`0x83af58`. `TFruityPlugInfo` @ `0x8aa688`: "Fruity X-Y-Z Controller" /
"X-Y-Z Controller", flags `0x10400060` (the same as X-Y Controller's,
with `FPF_NoProcess`), 11 parameters, 3 output controllers.

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x83b0c0 |
| 3 | SaveRestoreState | 0x83b520 |
| 4 | GetName | 0x83c800 |
| 6 | ProcessParam | 0x83c0a0 |
| 7 | Eff_Render | 0x40c5a0, the abstract stub |
| 14 | NewTick | 0x83cbf0 |

The instance keeps one block of 0x88 bytes per axis at `0x14c + 0x88 *
axis`: the output (`+0`), where the axis goes (`+4`), what was sent last
(`+0x10`), the joystick (`+0x14`, 8 bytes), the smoother's coefficients
(`+0x24`) and levels (`+0x58`), and the two maps (`+0x78`, `+0x80`).

## Parameters `asm` `dfm`

| parameter | control | range | default |
|---|---|---|---|
| 0, 1, 2 | `ModXWheel`..`ModZWheel`, X / Y / Z value | 0 to 65536 | 32768 |
| 3, 4, 5 | `CSpeedWheel`, smoothing of X / Y / Z | 0 to 4096 | 2048 |
| 6, 7, 8 | `CAccWheel`, smoothing order of X / Y / Z | 0 to 3072 | 3072 |
| 9 | `GlobalSpeedWheel`, global smoothing speed | 0 to 4096 | 2048 |
| 10 | `JoySpeedWheel`, external controller speed | 0 to 4096 | 1024 |

GetName appends X, Y or Z to the names of 3 to 8. The editor shows one
axis's smoothing at a time.

`ProcessParam` with `REC_UpdateValue` stores the value at `0x120 + 4n`,
and then:

- A value (0 to 2) sets where its axis goes: the axis's first map at
  `value * (1/65536)` as a single.
- 3 to 9 set all three smoothers again (`0x83c400`).
- 10 is the joystick's speed, `Exp((p - 4096) * 14/4096 * Ln 2) * 65.536`
  at `0x2ec`.

With `REC_FromMIDI` (0x20) a host's 0 to 2^30 is put on the control's
range first, `min + Round(v * 2^-30 * (max - min))`.

## State `asm` `corpus`

`SaveRestoreState` writes:

| bytes | what |
|---|---|
| 4 | version, 3 |
| 44 | the eleven parameters, i32, into `0x120` |
| 4 | flags: bit 0 to `0x2e7`, bit 1 to `0x2e8`, bit 2 an editor button |
| 4 | the display colour (`CHWheel`) |
| per axis, 3 times: | |
| 8 | the joystick: its number (0 for none) and its mode |
| map | the map a value goes through |
| map | the map the eased value goes through |

A map is a `TMMap`, the stream `crates/plugins/src/menvelope.rs`
describes, of kind 3. The loader (`0x77e970`) reads no further than the
kind when it is another class's, and nothing of a version past 3; the
plugin reads nothing past the version of a state of version 4 or more.
`set_state` refuses all three. It then sets every parameter again with
`REC_UpdateValue | REC_UpdateControl`.

All 513 states in FL's files are of version 3, 560 to 1136 bytes long:
the 32 in the projects, 14 plugin presets and 467 inside Patcher's
presets. No joystick is set in any. Map points use modes 0, 1, 2, 3, 6,
7, 10, 11 and 12.

## The maps `asm`

The constructor makes both maps of every axis a straight line from (0, 0)
to (1, 1). A loaded map is made ready in three steps:

1. `0x781c10` makes it one long. It adds a point at 0 when the first is
   past 0, and scales every point's length by `1 / total` as singles
   (`0x7845c0`), the total summed in doubles.
2. `0x782750` works out each point's shape, as `dsp::curve` has it.
3. `0x787ac0` builds a table only for two cases. When every point has the
   same value (the bits of the single), the table is that value. When
   there are two points and the second is of mode 0, 1, 7, 8, 9, 11 or 12
   with a tension of exactly 0 (the mask `0x1b83`), the table is the two
   values. The plugin never sets a table size (`+0xb8`), so any other map
   has none.

Reading a map at `x` (`0x787da0`):

- A flat table gives its value.
- A two-value table rounds `x - 0.5` to `i`: below 0 the first value, at
  0 `t0 * (1 - d) + t1 * d` with `d = x - i` in doubles, above it the
  second.
- Without a table, a run starts at the first point (`0x784970`) and moves
  on by `x` (`0x7858b0`). It starts at a position of 1 at a rate of 1
  when the first point is at 0, so it steps straight into the first
  segment. While the position is 1 or more (the test is on its high word)
  it takes the position back to `x` units, `(pos - 1) / rate`, moves to
  the next point, and divides by that point's length. The value is `from +
  shape(pos) * (to - from)`, all singles.

The run's steps can fall short of a point. In a map whose lengths add up
to a hair under 1, `x = 1` still lands in the last segment and gives the
point before the last. fl-rs runs the same steps, so it gets the same
value.

## The smoothers `asm`

`0x83c400` sets each axis's smoother with

```
global = Exp((single)(p9 * (1/4096)) * Ln 17)
speed  = p[3+i] / 2048, as a single; if not 1: (Exp(speed * (single)Ln 3) - 1) * 0.5
ticks  = (single)(global - 1) * 24 * PPQ/96 * speed
order  = p[6+i] / 1024, as a single
```

and `0x604a90` in its mode 4. A count of 0 or less turns the smoother off,
and it passes the value as it is. Otherwise `n = Round(order +
0.49999999999999994)`, the count is divided by `sqrt(order)`, and for each
of three stages `k` from 0:

- before `n - 1`: that count,
- at `n - 1`: the count times `1 - (n - order)`,
- from `n` on: 0,

and the stage's coefficient is `Exp(Ln(0.01) / count)`, or 0 for a count
of 0. The mode is then always 2: three one-pole stages in a row, each
`s = in + (s - in) * c`, run on the distance from a base level
(`0x604ff0`). A stage with a coefficient of 0 passes at once.

464 of the 513 states have a global speed of 0, which turns every
smoother off, and so do all 32 in the projects. `FPD_SetTimeSig` (14) in
the dispatcher sets `0x2f0` to `PPQ / 96` and the smoothers again.

## NewTick `asm`

For each axis (`0x83cbf0`):

1. Keep the output the tick before left, `old`.
2. Ease: `eased = smoother(target)` as a single. When `|eased - target|`
   is under 2^-16 / 10 (`0x83cd48`), the smoother is put at the target
   with its stages emptied (`0x604fd0`).
3. `output = second map(eased)`.
4. `send = clamp(Round(old * 2^30), 0, 2^30)`, the product a single. When
   it is not what was sent last it goes to the host's
   `OnControllerChanged` (host method `+0x98`) as output `axis`.

So a value set on a tick reaches the output on the next one. X is output
0, Y 1 and Z 2.

## Start and flush `asm` `inferred`

`FPD_Flush` (2) puts every smoother at its target. It does not touch the
outputs or what was sent, and `Effect::reset` does the same.

A new instance in FL starts with every output and every sent value at 0,
so a 0 is never sent until the output has been something else, and the
first value goes out on the second tick. FL ticks a plugin while the song
is stopped, so by the time a loaded song plays the outputs have gone out.
fl-rs starts a loaded state with every axis at its target, the outputs
worked out and nothing sent, so the first tick sends all three. A host
here carries all of a plugin's outputs when one of them moves, so an
output FL would hold back at 0 is applied anyway; its target then gets the
0 the project most likely saved.

## In fl-rs

Inside a Patcher the controller gets the Control Surface's outputs through
control wires and its exposed parameters, and its outputs drive the inner
parameters they are wired to. In the five projects the render reaches,
its 30 instances have 66 wires in, 96 wires out and 66 of the Patchers'
exposed parameters. Before it was built they did nothing.

## What is not FL's

- The joystick input (`0x83c5c0`, the 8 bytes per axis, parameter 10).
  `fidelity()` says so when a state sets a joystick; none does.
- A map with no point at 0, or no length, which FL mends by adding a
  point. `fidelity()` says so; no shipped map is one.
- Rust's `ln`, `exp` and `sqrt` stand in for the RTL's, and the shapes'
  constants are doubles in `dsp::curve` where FL keeps singles. Either can
  move an output by the last bit of a single.
- The start, as above.
