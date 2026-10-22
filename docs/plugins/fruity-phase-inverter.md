# Fruity Phase Inverter: state, parameter, signal chain

Recovered from `Fruity Phase Inverter_x64.dll` (FL Studio 2026, Delphi native
plugin, image base 0x400000), headless Ghidra. Corpus: no project uses it
natively; its render test holds it inside a Fruity Wrapper, which swaps
this plugin in and plays it.

## How the plugin is wired

`CreatePlugInstance` @ `0x006b9fa0` builds the class whose VMT is `0x006b9e80`
(class reference at `0x006b9db8`) with constructor `0x006ba2d0`.

| slot | SDK name | address |
|---|---|---|
| 3 | SaveRestoreState | 0x006ba1d0 |
| 6 | ProcessParam | 0x006ba520 |
| 7 | Eff_Render | 0x006ba930 |

## State (event 213, 8 bytes)

`SaveRestoreState` writes a version word, 1, then the parameter at
`this+0x120`. On load it takes a version under 2 only, then re-sends every
parameter through `ProcessParam`.

| byte | type | parameter | range | default |
|---|---|---|---|---|
| 0..3 | i32 | version | 1 | |
| 4..7 | i32 | 0 State | 0..1024 | 1024 |

The range and default are the DFM's `StateWheel` (`Max = 1024`, `Value = 1024`,
`DefaultValue = 1024`).

## Parameter

`ProcessParam` keeps the value at `this+0x120`, works out the wheel's place
`p = clamp((v - Min) / (Max - Min), 0, 1)` in doubles, and hands it on as a
single to `0x006ba720`, which keeps `Round(p * 3.0 - 0.5)` (singles, `3.0` at
`0x006ba75c`, `0.5` behind `PTR_DAT_00709ff0`; Delphi's `Round`, half to even)
at `this+0x128`: 0 Off, 1 Left, 2 Right, the names at `0x00709068`. So 0..341
is Off, 342..682 Left and 683..1024 Right, and the default is Right.

## Signal chain

`Eff_Render` (`0x006ba930`) copies the block to the output when they differ,
then for a mode above 0 flips the sign bit (`xor 0x80000000`,
`FUN_006ba910`) of every left sample (mode 1) or every right sample (mode 2).
Nothing else; exact.
