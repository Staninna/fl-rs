# Pitch Shifter: handover notes

## Where the DSP is

`Plugins/Fruity/Effects/Pitch Shifter/Pitch Shifter_x64.dll` is a Delphi shell (`TTestPlug`, VMT 0x72bb08). It loads two engines from `Shared/ildsp_x64.dll`:
- `PSOLA_CreateInstance` (0x1801ab540): `PSOLAImpl`, engine 0x850 bytes. Prepare `FUN_18017f700`, set `FUN_1801804b0`, process `FUN_180180830`, which calls split/gate `FUN_180181590`, detector `FUN_180181730` and synthesis `FUN_180181d60`. Vtable slot 5 returns the latency (`+0x98`).
- `GeneralPitchShifter_CreateInstance` (0x1801aa290): `GeneralPitchShifterImpl`, engine 0x2f8 bytes. Prepare `FUN_180161fc0`, set `FUN_180162760`, process `FUN_180162f90`, clear `FUN_1801aa3a0`.

The general engine is not Luxeverb's shifter class. It has its own functions and 0.2 s grain buffers (Luxeverb's are 0.8 s), delay modes 1 to 4, a tanh-table clip, active feedback biquads and its own rand static (`0x1803f67d0`, seed 0x3920a). The algorithm is the same family, and the Lagrange read and the MSVC rand step are identical, so these two helpers are duplicated in `pitch_shifter.rs` and `luxeverb.rs`. They could move to `dsp`. Nothing was moved and `luxeverb.rs` is untouched.

## Shell (confirmed from the decompile)

- `SaveRestoreState` `FUN_0072c630`. v6 is 69 bytes and v7 is 75 bytes; any other version is ignored. Layout: i32 version, i32[13] controls, i32 correction wheel (duplicate), u8 tempo sync, i32 effect (0 PSOLA, 1 general), i32 detector (0 fast, 1 accurate), then v7 only: i32 display length and two display bytes.
- `ProcessParam` `FUN_0072cda0` converts each control to `x = clamp((v-min)/(max-min), 0, 1)`. `FUN_0072d0f0` maps x onto the engines. The full map is in the `push()` doc comment in `pitch_shifter.rs`.
- `FUN_0072d020` sets the PSOLA formant to `clamp(formant + pitch*(1-corr), -12, 12)`.
- `Eff_Render` `FUN_0072de80` runs the PSOLA engine when effect == 0 and the general engine otherwise.
- `ProcessEvent` tempo calls PSOLA case 7 and general case 9.
- The sample-rate dispatcher (case 4) prepares both engines, pushes every control, then `FUN_0072dfa0` sends PSOLA case 9 with a 1024/2048 window and reports the latency.
- Corpus: 15 states in the .flp files, all v7 and 75 bytes, using both engines. No state has sync on. There are 30 preset states.

## Guessed, not confirmed

- Load order of the sync button. FL pushes the controls before it applies the state's sync byte. The code uses the state's sync byte, and `fidelity()` says Approximate when sync is on with a delay mode set.
- Defaults are taken from the untouched Mehran Abbasi state (the DFM wheels, PSOLA, fast detector).
- The PSOLA set cases 4 and 8 clamp arguments were read from objdump. The rest was read from Ghidra.
- IPP's FFT is replaced by a radix-2 FFT, and the CRT pow/cos/exp/tanh by Rust's. They may differ in the last bits.

## Built

- `crates/plugins/src/pitch_shifter.rs` (new, about 1580 lines after another agent ran `cargo fmt`) has the shell plus both engines, 11 unit tests and the corpus/preset play test.
- `crates/plugins/src/lib.rs`:
  - added `pub mod pitch_shifter;`
  - added a `by_name` branch for `pitch_shifter::NAME`
  - added the row `("pitch shifter", 75, 15)` to the corpus table
  - changed `lengths.count()` from 36 to 37. Frequency Splitter's agent had raised it from 35 to 36 just before.
- It compiles. `cargo test -p plugins --lib pitch_shifter` passes (11/11, with and without FL_RS_CORPUS). `every_state_in_fl_s_projects_loads` passes.

## Next steps

1. Run `cargo clippy -p plugins --all-targets -- -D warnings` and `cargo fmt` on the file.
2. Run the engine corpus render for the before/after numbers. The earlier baseline failed to build because of another agent's file (`OLDEST` undefined).
3. Write `docs/plugins/pitch-shifter.md` (state map, control map, both signal chains with the addresses above) and fill in the Pitch Shifter row in `docs/plugins.md`.
4. Optional: a render-test-project sanity check (`Data/System/Render tests/Pitch Shifter.flp`, general engine, +12 semitones).

Ghidra scratch work is in `/tmp/claude-1001/-shared-code-fl-rs/cd8fe48c-0ad3-4ca4-accb-d019123d9334/scratchpad/pitch-shifter/` (my own projects `proj/ps`, `proj/ildsp` copy, and the `out/` decompiles and asm).
