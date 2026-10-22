# BeepMap: state, parameters, voice

Recovered from `Plugins/Fruity/Generators/BeepMap/BeepMap_x64.dll` (FL
Studio 2026, 4186 KB, Delphi, image base 0x400000). The plugin is a
`TTestPlug` on `TDelphiFruityPlug`, with FL's instrument tab (envelopes,
LFOs, filter, pan law) compiled in, the same unit 3x Osc has
(`docs/plugins/3x-osc.md` sections 3.2 and 3.3). The DSP is a few hundred
lines at `0x0072e000..0x00731000`; the whole DLL was decompiled with
headless Ghidra and the cited spots read in asm (Capstone).

Corpus: 1 project state (the render test `System/Render tests/Beepmap.flp`)
and 17 presets, every one version 0 with a 32-bit or 16-bit BMP inside.
The Rust loads and sounds each.

## How the plugin is wired

`CreatePlugInstance` `0x0072e080` -> constructor `FUN_0072e0d0`, class
reference `*0x0072dea0` -> VMT `0x0072df68` (`TTestPlug`, 0x1b0 bytes).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x0072e670 (4 sets the rate: `+0x160`, and `+0xe4 = 2^32 * 523.2511 / rate`) |
| 3 | SaveRestoreState | 0x0072ebc0 |
| 6 | ProcessParam | 0x0072efc0 |
| 8 | Gen_Render | 0x00730740 (the tab's voice manager renders every voice and mixes) |
| 9 | TriggerVoice | 0x0072f5b0 |
| 10 | Voice_Release | 0x0072f710 (the tab's release) |
| 13 | Voice_Render | 0x0072fe80 |

The constructor reads its parameters from the form's controls, takes the
host's sine table (`host + 0x18`, 16384 points, the one in `three_osc.rs`)
to `+0x180`, and loads `Images/Default.bmp` from the plugin's directory.

## 1. State (event 213)

```
i32 version (0 or 1; more is ignored)
v0: 60 bytes: i32 params[7] at +0x120, i32[7] at +0x13c (not read), i32 row limit at +0x158;
    the tab gets its defaults (FUN_00722690)
v1: 0x1a8 bytes: the 60, then the tab's 364-byte block (FUN_00722720)
u32 length, then that many bytes of a BMP file
```

The picture (`FUN_007300d0`) is taken when it is not empty and has more
than 2 rows. It is made 32-bit, and shrunk to the row limit when taller
(`FUN_00612da0`, a GDI stretch, not read). `+0x174` = its rows, and
`+0x15c = 1 / (255 sqrt(rows))`.

## 2. Parameters

The form's `SynthEditorForm` gives the ranges and defaults (Tag = index).
`ProcessParam` stores the value as is.

| # | control | range | default | use |
|---|---|---|---|---|
| 0 | Freq range (PreAmpWheel) | 24..9600 | 7200 | cents across the rows; a change retunes every voice |
| 1 | Pixel duration (LengthWheel) | 0..4096 | 2048 | `+0x178 = max(1, MulDiv(round(256 (33^(v/4096) - 1)) + 64, rate, 44100))` frames a column, `+0x17c` its inverse |
| 2 | Scale | 0..2 | 0 | Log, Linear, Harmonics |
| 3 | Use Blue | 0, 1 | 0 | row width `256 - blue` |
| 4 | Grainy | 0, 1 | 0 | ring the note with one sine turn a column |
| 5 | Loop | 0, 1 | 1 | |
| 6 | Widen | 0, 1 | 1 | a right-side phase offset per row |

## 3. A note

TriggerVoice: a voice of `0x60 + 0x30 (rows - 1)` bytes; per row the phase
`Random(2^24) << 8` and, with Widen, a right offset `Random(2^24) << 8`
(Delphi's `Random`, `seed = seed * 0x08088405 + 1`). Column -1, position 0,
pitch cache unset. The tab makes its articulation voice.

Voice_Render (`0x0072fe80`), for `n` frames: the pitch is
`round(FinalLevels.Pitch)` cents from key 60; on a change every row is
retuned (`FUN_0072f7e0`). Then until `n` frames are made: at a column's
first frame the column moves on and `FUN_0072fb20` sets the targets; the
frames to the column's end are made as below.

Rows (`FUN_0072f7e0`), `last = rows - 1`, `w` the row's width (256, or
256 less blue), integer arithmetic:

- Log: `cents = pitch - (2 i - last) range w / (last 512)`, clamped to
  -4800..4800; step `round(2^32 * 523.2511 / rate * 2^(cents/1200))`.
- Linear: `base = min(round(523.2511 * 2^(pitch/1200)), rate/4)`, `top =
  (rate/4) / base * base`; `hz = base - (2 i - last) (3 range / 2) w / (last
  512)`; step `(clamp(hz, 40, top) << 32) / rate`.
- Harmonics: `base` as linear from `pitch - 2400`; `hz = (rows - i) base (9
  - w / 32)`.

Column (`FUN_0072fb20`): past the last column it goes back to 0 with Loop,
else every row is dark. Amplitudes under 2^-24 are snapped to 0. Row `i`
targets `(red, green) * +0x15c` of pixel `(i, column)`, row 0 the top.
Harmonics fades a row above `t = round((11^(range/9600) - 1) 1102.5)` Hz
by `max(0, 1 - (hz - t) / 480)`. Each amplitude moves `(target - now) /
columnLength` a frame. A row with all four values 0 is idle. The grain step
is `2^32 / columnLength`.

Frames: row 0 writes, the others add, `L = sine[phase >> 18] ampL`, `R =
sine[(phase + offset) >> 18] ampR`; an idle row only moves its phase.
Grainy multiplies by `sine[grain >> 18]`. The render reports the note done
once the column is past the picture's width.

Gen_Render hands each voice's frames to the tab (`FUN_00721890`,
`FUN_00721d60`): filter, then the circular pan law of `FinalLevels`
pan and volume, ramped, into the mix. A voice ends when its render or its
tab is done. With version 0's defaults the tab's envelopes are off: its
Fast LP sits at cutoff 1 plus mod X and resonance mod Y, and a note off
fades the note in 10 ms, as `three_osc.rs` has it.

## 4. What `beepmap.rs` builds

All of the above but: the tab only at its defaults (a version 1 state is
said approximate); the shrink to the row limit as an average of rows; a
16-bit picture's colours widened as `c << 3 | c >> 2`; `Random` seeded per
instance from 0 where FL's seed is process-wide; 32 voices.
