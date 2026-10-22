# Emphasizer: handover

State of the port (`crates/plugins/src/emphasizer.rs`, `docs/plugins/emphasizer.md`):

- Done: the Delphi shell `Emphasizer_x64.dll` is mapped (VMT, state versions 1 and 2, the six parameters, the values `0x006c2bd0` hands the limiter, the limiter's interface slots). Every corpus state parses (1 project, 5 presets). Port level Gaps; `fidelity()` is Approximate for every state; the sound passes through.
- Left: the limiter itself. All the DSP is `LimiterLite` in `Shared/ildsp_x64.dll` (`LimiterLite_CreateInstance`, looked up by name). The install this was first read from had no `Shared/` directory. The one at `/shared/code/fl-studio-target/.../FL Studio 2026/Shared/ildsp_x64.dll` has it, and it exports `LimiterLite_CreateInstance`, so the steps below can start.

Next steps, with `ildsp_x64.dll` at hand:

1. Import it into Ghidra (MSVC, image base 0x180000000). `docs/plugins/emphasis.md` already maps `Limiter` in the same DLL (`Limiter_CreateInstance` @ `0x1801aafd0` in that build); find `LimiterLite_CreateInstance` in the exports and its vtable.
2. The shell calls, by vtable byte offset: +0x08 init(float rate, i32 max block 0x800), +0x10 setParam(i, float), +0x18 getParam(i), +0x20 process(planar in, planar out, frames), +0x28 flush, +0x30 LUFS reset, +0x48/+0x50/+0x70 meters, +0x58/+0x60 set/get the switch the state's last byte holds, +0x68 latency.
3. The parameter values it gets: 0 input gain in dB (24 n), 1 emphasis share, 2 clipping 0 soft / 1 hard, 3 level matching, 4 bypass, 5 output gain in dB (`20 log10((11^n - 1)/10)`, 0 at the top). `Emphasizer::limiter_values` computes them.
4. Read getParam's defaults (the wheels' defaults; a version 1 state's output gain is the default of parameter 5, taken as 10000 for now), the latency, and whether the stages are `emphasis_dsp.rs`'s (`GrStatic`, `GrAdaptive`, `TruePeak`...). Reuse them from there if so.
5. Build `process` behind the existing state code, report `latency()`, and move the port to Exact when nothing is left out.
