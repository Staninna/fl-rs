# Low Lifter: handover (not built)

`Low Lifter_x64.dll` is a Delphi shell; the DSP is
`LowFreqEnhancer_CreateInstance` in `Shared/ildsp_x64.dll`, a Faust class
`ILRDLowFreqEnhancer` compiled from `lowFreqEnhancementChebyshev.dsp`
(`-single -ftz 0 -mcd 16 -mdd 1024 -mdy 33`, scalar). Its `compute` is
~1450 decompiled lines: crossover state-variable filters, four peak-hold
followers with lookahead (2048-sample rings), Chebyshev polynomial
enhancement per band, a four-stage modulated diffusion network (8192- and
2048-sample rings), dry/wet and gain stages. A faithful transcription is
a day's work, so it was left; the three projects (Bien Duro x2, Song For
You) and the render test keep passing it through.

Work was done on a cloud box (not in the repo): Ghidra projects
`/root/ghidra/fx/proj` (`Low_Lifter_x64.dll`) and `/root/ghidra/fx/ildsp`
(`ildsp_x64.dll`); decompiles `/root/ghidra/fx/out/ll/all.c` (shell),
`out/lfe/all.c` (factory, constructor), `out/lfe2/all.c` (Faust methods).

## Shell (image base 0x400000)

`CreatePlugInstance` @ `0x006c5930`, class reference `0x006c5720` -> VMT
`0x006c57e8` (`TTestPlug`, size 0x1e0). Dispatcher `0x006c64d0`,
SaveRestoreState `0x006c6d50`, ProcessParam `0x006c6f90`, Eff_Render
`0x006c7a80`, ProcessEvent `0x006c2460`.

State (event 213, 82 bytes): `i32 version` (6; only 6 loads), 60 bytes =
15 i32 at `0x134`, `u8 0x194`, `u8 0x1a8`, `i32 0x19c`, `i32 0x1a0`,
`i32 0x1a4`, `i32 0x198`. Corpus: 4 states, all version 6; 7 presets.

Parameters to the Faust object (`FUN_006c71d0`, `setParam(i, x)` at
interface +0x10): 0, 1, 7: `x = n`; 2, 3, 6, 8, 9, 10, 11: a curve
`FUN_006c54c0(n, a, b, c)` with doubles at `0x006c7418..0x006c7480`
(read them); 4, 5, 12..14: `x = float(raw)`. Parameter 4 also calls
`FUN_006c7e00`.

Render: optional input/output meters, a silence counter, then
`impl->process(len, in, out)` (`+0x20`), which deinterleaves into Faust's
`compute`.

## Faust class (`ildsp_x64.dll`, image base 0x180000000)

- Factory `0x1801ab3d0`, `LowFreqEnhancerImpl` vtable `0x1803dfff8`
  (+0x10 setParam `0x1801ab500`.. +0x20 process), Faust vtable
  `0x1803de0a0`; constructor `FUN_18017bbd0` sets the defaults through
  `FUN_18017bdd0` (index -> zone through the `MapUI` built by
  `buildUserInterface`).
- `buildUserInterface` `0x180177a80`: zones Multiband (+0x54), Dry/Wet
  (+0x20), On/Off (+0x1c), Low Gain dB (+0x2104), Invert Polarity
  (+0x20ec), Lookahead (+0x20e8), Slope dB (+0x41ec), Pre Gain dB
  (+0x2168), Diffusion Mix (+0x2110), Diffusion Depth (+0x211c), High Gain
  dB (+0x48), High Switch (+0x3c), Crossover Cutoff Hz (+0x60), Low Cut Hz
  (+0x30), Low Switch (+0x20f8). Ranges in the constants it passes.
- `instanceConstants` `0x180177050`, `instanceResetUserInterface`
  `0x1801772d0`, `instanceClear` `0x180177320`, `compute` `0x180177f50`,
  `metadata` `0x180176ea0`. Maths helpers: `FUN_1801c9ef0` (tanf),
  `FUN_1801c5f40` (expf), `FUN_1801c83b0` (powf), `FUN_1801b6dec`.

Next steps: read the curve constants and `FUN_006c54c0`; transcribe
`compute` zone by zone (state as an `f32` image indexed by offset keeps
the transcription honest), check it against the listing.
