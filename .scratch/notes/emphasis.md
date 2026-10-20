# Emphasis: handover (not built)

Emphasis does **not** share code with Maximus. `Emphasis_x64.dll` is a
Delphi shell (state, GUI, LUFS meters); all the audio is
`Limiter_CreateInstance` in `Shared/ildsp_x64.dll` (MSVC C++, the same
library as Spreader, Luxeverb and Pitch Shifter), a hand-written C++ class
`Limiter` (not Faust) of 0xa02a80 bytes: several 1 MB delay lines, two
oversampling filter chains per channel, `Lowpass1` and `ExpSmoother1`
members, an RMS/peak detector with rational-tanh saturation curves and
mode-dependent attack/release. The process function alone decompiles to
~1000 lines and `setParam` to ~600, plus ~30 filter/stage helpers. It was
judged much bigger than this pass and left; the five projects that use it
keep passing it through.

Work was done on a cloud box: Ghidra projects `/root/ghidra/fx/proj` (program
`Emphasis_x64.dll`) and `/root/ghidra/fx/ildsp` (program `ildsp_x64.dll`),
decompiles in `/root/ghidra/fx/out/emph/all.c` (shell slots + callees),
`out/lim/all.c` (factory, constructor, setParam), `out/limproc/all.c`
(process + callees). These are not in the repo; rerun with
`DecompileDeep.java` (any `DecompileAddrs.java`-style script) at the
addresses below.

## Shell (`Emphasis_x64.dll`, image base 0x400000)

`CreatePlugInstance` @ `0x00704b90` -> constructor `0x00704c00`, class
reference `0x00704850` -> VMT `0x00704918` (`TTestPlug`, size 0x230).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x00705720 |
| 3 | SaveRestoreState | 0x00706090 |
| 5 | ProcessEvent | 0x00705db0 |
| 6 | ProcessParam | 0x007063a0 |
| 7 | Eff_Render | 0x00707240 |

Fields: `0x16c` the ildsp interface (`LimiterImpl`, vtable `0x1803dff40`:
+0x10 setParam(index, float) `0x1801ab050`, +0x18 getParam `0x1801ab060`
(reads `limiter+0xa02278 + 4 i`), +0x20 process `0x1801ab070` ->
`FUN_180167330`), `0x134`/`0x13c` planar input buffers, `0x144`/`0x14c`
planar output, `0x18c` i32[13] the parameters (saved).

Render (`0x00707240`): deinterleave Source into 0x134/0x13c
(`PTR_DAT_0076a3e8`), `limiter->process(in, out, len)`, interleave into Dest
(`PTR_DAT_0076a060`), then meters only (`FUN_007075e0` is the GUI history).

### State (event 213)

`SaveRestoreState`: `i32 version` (10 now; versions up to 10 load), 52
bytes = the 13 i32 at 0x18c, `i32` editor setting (`editor+0x870`, LUFS
meter mode), `i32` (version > 6, `editor+0x878`), then `u8` (version > 8,
param 0x19) and two `u8` (version > 9, params 0x1a, 0x1b). Version 10 is 67
bytes, versions 7 and 8 are 64. Before version 8, parameter 2 is
`clamp(p2 + 1, 1, 4)`. Corpus: Bien Duro, NIGHT TERROR, With You (v8, 64
bytes), Use Me (v10, 67), render test `Emphasis.flp` (v7, 64); 21 presets,
all v10.

### Parameters to the limiter (`FUN_00706860`)

Index i goes to `setParam(i, x)` with:

- 0, 1, 3, 4, 5, 10, 11, 12, 0x19..0x1b: `x = float(raw value)`
- 2: `x = float(raw - 1)`
- 6: `x = a + (b - a) * exp(log(n) / c)`-style curve (`FUN_007043f0` with
  the doubles at `0x00706a10/18/20`; read them)
- 7, 8: `x = n` (the wheel's 0..1)
- 9: `FUN_00705110(n)`: `n == 1 ? 1 : (exp(n * k1) - c) * k2`, then
  `< 1 -> DAT_007051a0`, else `f(x) * DAT_007051a8` (`FUN_005dfeb0`)

`n` is the wheel's 0..1 from `editor+0x93c + 8 i` (Min/Max at +0x3a8/+0x3ac);
the wheel ranges were not dumped: run `tools/plugstate/dfm.py` on the DLL
(the form is not `SynthEditorForm`; search for the wheels' hints).

## The limiter (`ildsp_x64.dll`, image base 0x180000000)

- Factory `Limiter_CreateInstance` @ `0x1801aafd0`, constructor
  `FUN_18016adc0` (sets the defaults by calling setParam
  `FUN_180169150` for 0..12, 0x19..0x1b).
- `setParam` `FUN_180169150`: a switch on the index; mode (5) and
  oversampling-related indices rebuild filter chains through jump tables
  Ghidra did not recover (`0x18016a3a4`, `0x18016a3d8`, `0x18016a3f4`): read
  those with `tools/ghidra/DisasmAddrs.java`.
- `process` `FUN_180167330`: a per-sample detector (two one-pole smoothers
  per channel, squared, a rational tanh approximation with constants at
  `0x1803e0xxx`), then gain computation and the oversampled saturator
  stages.

Next steps: dump the DFM ranges; read `setParam` case by case with the jump
tables; read `process` against its listing; port the filter classes
(`Lowpass1`, `ExpSmoother1`, the 0x1e0/0x1c0/0x250-byte stage objects).
Expect this to be a Maximus-sized job.
