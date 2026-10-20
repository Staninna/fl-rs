# Fruity Mute 2: state, parameters, signal chain

Recovered from `Fruity Mute 2_x64.dll` (FL Studio 2026, Delphi native plugin,
image base 0x400000), headless Ghidra. Corpus: no project uses it natively;
its render test holds it inside a Fruity Wrapper.

## How the plugin is wired

`CreatePlugInstance` @ `0x006ba070`, VMT `0x006b9f50` (class reference
`0x006b9e88`), constructor `0x006ba3a0`.

| slot | SDK name | address |
|---|---|---|
| 3 | SaveRestoreState | 0x006ba2a0 |
| 6 | ProcessParam | 0x006ba630 |
| 7 | Eff_Render | 0x006baaf0 |

## State (event 213, 12 bytes)

A version word, 1, then the two parameters at `this+0x120`. On load only a
version under 2 is taken.

| byte | type | parameter | range | default |
|---|---|---|---|---|
| 0..3 | i32 | version | 1 | |
| 4..7 | i32 | 0 Mute | 0..1024 | 512 |
| 8..11 | i32 | 1 Channel | 0..1024 | 512 |

Ranges and defaults from the DFM's `MuteWheel` and `ChannelWheel`.

## Parameters

`ProcessParam` takes the wheel's place `p = clamp(v / 1024, 0, 1)` as a
single to `0x006ba850`:

- Mute: `this+0x128 = p < 0.5` (the 0.5 behind `PTR_DAT_0070a018`). The
  default 512 is not muted.
- Channel: `this+0x12c = min(Trunc(p / 0.33333334), 2)` (single division,
  `PTR_DAT_00709af0`): 0 L, 1 L + R, 2 R (names at `0x00709098`).

## Signal chain

`Eff_Render` (`0x006baaf0`): not muted, a copy. Muted with channel 1, the
output is zeroed. With channel 0 each frame is `(L * 0.0, R * 1.0)` and with
channel 2 `(L * 1.0, R * 0.0)` (`FUN_005df490`, gains `DAT_006bab94` = 0.0
and `DAT_006bab98` = 1.0), so a muted negative sample comes out as -0. Exact.
