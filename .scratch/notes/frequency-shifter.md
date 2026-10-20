# Frequency Shifter: handover (not built)

`Plugins/Fruity/Effects/Frequency Shifter/Frequency Shifter_x64.dll` is a
Delphi shell; the DSP is the MSVC DLL beside it, `Engine_x64.dll`, one
export `dispatch(op, obj, a, b, f)`. The engine is ~1700 decompiled lines
of real work (process alone ~460, constructor ~550), with four-lane SIMD
state (L/R x two Hilbert branches) and five crossfaded filter banks
redesigned every 32 samples. It was judged too big for this pass. Corpus:
Bien Duro (two instances), Africa's Calling, I'll Be Your Madonna (mix 0,
so it passes the input), the render test, 9 presets.

Work was done on a cloud box (not in the repo): Ghidra project
`/root/ghidra/fx/proj`, programs `Frequency_Shifter_x64.dll` and
`FreqShifterEngine_x64.dll` (the engine renamed); decompiles in
`/root/ghidra/fx/out/fs/all.c` (shell) and `out/fse/all.c` (engine, depth
3).

## Shell (image base 0x400000)

`CreatePlugInstance` @ `0x006dc420`, class reference `0x006dc100` -> VMT
`0x006dc1c8` (`TTestPlug`, size 0x1e0). Dispatcher `0x006dc4c0`,
SaveRestoreState `0x006dd080`, ProcessEvent `0x006dc960` (tempo:
`0x184` f64 BPM, `0x178` = `rate * 15 / (bpm * 0x17c)`), ProcessParam
`0x006dd650`, Eff_Render `0x006ddbb0`.

State: `i32 version` (4; versions under 5 load), 40 bytes = ten i32 at
`0x140`, `i32` flags (bit 0: a GUI toggle at `editor+0x748+0x48e`), `i32`
at `0x168` (sidechain input number). 52 bytes.

Parameters go to the engine as the wheel's 0..1 (`FUN_006dd8f0`:
`dispatch(3, engine, index, 0, n)`). Engine names (dispatch 5): 0 Mix,
1 Type, 2 Frequency, 3 L/R Phase Offset, 4 L Shape, 5 R Shape,
6 Feedback, 7 Mono<->Stereo, 8 Range, 9 Left Phase Offset. Wheel ranges:
dump the DFM (`SynthEditorForm`, editor `+0x780 + 8 i`).

Render: `dispatch(7)` (block start: MXCSR FTZ/DAZ, lock), then per frame
`dispatch(4, engine, &frame)` with the frame `{L, R, sidechain L, R, flag
(sidechain on), f64 song position at this frame and the next (or -1 when
not tempo-synced), ...}`; `dispatch(8)` at the end. The sidechain comes
from `host vtable +0x120` when parameter 9 of the shell (`0x144`) is 2 and
`0x168 > 0`. A silence counter (`0x120`/`0x124`) skips the tail as other
plugins do.

## Engine (`Engine_x64.dll`, image base 0x180000000)

- `dispatch` `0x180001000`; constructor `FUN_180001350` (object 0x1de0
  bytes; Hilbert coefficients from the 128-bit constants at
  `0x180025640..0x1800256b8`; default params: 1 for all but 1, 3 (0) and
  8 (0.5)); rate `FUN_180001c20` (input filters by `FUN_180003960`: two
  high-pass at 20 Hz and two low-pass at 20 kHz, smoothing `0x58/0x5c`,
  `0x68 = 499.99997 / rate`).
- Param derivations `FUN_1800023c0`: mix `0x90 = clamp(2v,0,1)` (wet),
  `0x8c = clamp(2 - 2v,0,1)` (dry); type `0x84 = v < 0.3333 && v < 0.6666`,
  `0x88 = v < 0.6666`; shapes `0x98/0xa0`, `0x94/0x9c` = `1 - v`, `v`;
  feedback `0xe4 = 2.01 v - 1.005`; stereo `0xb0 = (1 - v) * 0.5`; phase
  offset 3 `0xac = v / 4`; 9 `0x70 = v + 0.5`; range 8 sets flags
  `0x85/0x86` against 0.6666 / 0.3333.
- Smoothing `FUN_180002560`, process `FUN_180002780`: input filters, two
  Hilbert chains `FUN_180003760` (second-order allpass sections
  `y = c (x + y[n-2]) - x[n-2]`, 4 lanes, 4 stages each, one branch a
  sample late), oscillator phase at `0xd0` (`sin` `FUN_1800186c0`,
  `sincos` `FUN_180016813`), ring-mod branch (type 0 region), the
  five banks (`FUN_180003430` designs four sections per bank with
  `FUN_180003dc0`, Qs at `0x180025568/70/78/98`; run inline and by
  `FUN_180003690`), shape mix, feedback, mono/stereo.

Next steps: dump the DFM; read `FUN_180003dc0`, `FUN_180003960`,
`FUN_180001fd0` and the constants; transcribe `FUN_180002780` lane by lane
against its listing (`DisasmAddrs.java`), since the decompiler folds the
SIMD lanes.
