# Sawer under the emulator

The harness behind `docs/plugins/sawer.md` and `crates/plugins/src/sawer.rs`, from
Sakura's (`../sakura-emu`). Python 3 with `pefile` and `unicorn`; run with `python3 -I`.

- `emu.py`: the generic unicorn loader (imports stubbed, a bump allocator).
  `SAWER_DLL` names `engine_x64.dll`.
- `emu_sawer.py <state.bin> "<events>" <frames> [block] [rate]`: builds a `SawerSynth`
  (`0x180031680`, the preset list skipped, `data.saw` answered from `SAWER_DATA`),
  polyphony 0, the sample rate, the state, then note events (`on:<key>:<frame>`,
  `off:<key>:<frame>`) and blocks through slot 17. Use block 64: FL's wrapper cuts
  host blocks to 64 frames, and the engine's 6x buffer holds no more.
  `SAWER_INFO=1` prints the random generators and the default parameters;
  `SAWER_TRACE=1` the first voice's fields, the first 6x buffer (`emu_over.bin`)
  and the samples into the highpass.
- `run.py <state> "<events>" <frames> [tol]`: the DLL (cached in `out64/`) against
  the port's `sawer::tests::dump_for_dll` from the newest test binary
  (`PROFILE=release` for a release one), then `cmp.py`.
- `batch.py <every nth> "<events>" <frames> [nofx]`: `run.py` over `states/`; with
  `nofx` each state's chorus, phaser, delay and reverb are switched off first.
  The arpeggiator presets crash the harness (it gives the engine no song position).

States come from `../simsynth-emu/states.py sawer states <FL>/Data`.
