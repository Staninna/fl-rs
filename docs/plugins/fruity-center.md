# Fruity Center: state, parameter, signal chain

Recovered from `Fruity Center_x64.dll` (FL Studio 2026, Delphi native plugin,
image base 0x400000), headless Ghidra. Corpus: no project uses it natively;
its render test holds it inside a Fruity Wrapper, which swaps this plugin
in and plays it.

## How the plugin is wired

`CreatePlugInstance` @ `0x006a2850`, VMT `0x006a2730` (class reference
`0x006a2668`), constructor `0x006a2bf0`.

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006a28a0 |
| 3 | SaveRestoreState | 0x006a2ab0 |
| 6 | ProcessParam | 0x006a2e90 |
| 7 | Eff_Render | 0x006a3100 |

## State (event 213, 8 bytes)

A version word, 1, then the switch at `this+0x188`. On load only a version
under 2 is taken.

| byte | type | parameter | range | default |
|---|---|---|---|---|
| 0..3 | i32 | version | 1 | |
| 4..7 | i32 | 0 On | 0..1 | 1 |

## The follower

The filter object sits at `this+0x120`. The dispatcher's sample rate message
(4) calls `0x006a2bb0`, which sets its pace `+4 = (float)(host+0x20 *
4.567e-6)` (`DAT_006a2be0`) and its keep `+8 = 0x3f75c28f` (0.96). The
host's `+0x20` is `44100 / rate`, as WaveShaper's Center reads it (there the
factor is `2.2835e-5`, so Center's pace is a fifth of WaveShaper's). Reset
(2) zeroes the state (`0x006a3140` -> `FUN_005dde40`, 0x40 bytes from
`+0x28`).

## Signal chain

`Eff_Render` (`0x006a3100`): off, a copy; on, the routine behind the pointer
at `0x007005e0`, set by `FUN_005e1100` to the SSE2 `FUN_005e0ca0` (the x87
`FUN_005ddf10` otherwise). Per side, in doubles:

    v += (x - y) * a;  y += v;  v *= b;  out = (float)(x - y)

with `y` at `+0x28`/`+0x30` and `v` at `+0x38`/`+0x40`. After each block
every one of the four whose magnitude is under 2^-24 (`DAT_006e2d5c`) is
set to 0 (`DAT_005e0e78`). Exact, given `host+0x20`.
