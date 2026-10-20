# Frequency Shifter: built

Rebuilt in `crates/plugins/src/frequency_shifter.rs` (commit "feat(plugins):
rebuild Frequency Shifter from its Engine_x64.dll"); the map is in
`docs/plugins/frequency-shifter.md`. All 5 corpus states load and the render
test is hosted.

Left, if anyone wants bits:

- The CRT's vector `sinf` (`0x180016813` -> `0x1800167b0`), `sin`
  (`0x1800186c0`), `tanhf` (`0x18001b948`), `powf` (`0x18001a2e0`) and `pow`
  (`0x180018c40`) are Rust's. `tools/plugstate/engine_emu.py` can run the
  engine's own under unicorn for a reference, as was done for Parametric EQ
  2's designer.
- `set_state` lands the glides from the engine's own defaults at once (FL's
  mixer runs from the moment a project is open). FL's first frames after a
  fresh load sweep the frequency from the bottom of its wheel.
- Tempo sync counts frames from the host's ticks and takes four steps a
  beat; FL reads its song clock in ms and its time signature.

Ghidra (cloud box, not in the repo): `/root/ghidra/fs/proj` (engine) and
`/root/ghidra/fsh/proj` (shell).
