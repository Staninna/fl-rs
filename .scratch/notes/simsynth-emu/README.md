# SimSynth under the emulator

The harness behind `docs/plugins/simsynth.md` and `crates/plugins/src/simsynth.rs`.
Python 3 with `pefile` and `unicorn`. Run everything with `python3 -I`.

- `emu_simsynth.py <dll> <state.bin> "<events>" <frames> [block] [rate]` runs
  `SimSynth_Eng_x64.dll` through its `TFruityPlug` interface: `CreatePlugInstance`
  with a stub host, the sample rate (dispatcher 4), `SaveRestoreState`, then
  `TriggerVoice` / `Voice_Release` / `Gen_Render` / `Voice_Kill` for events like
  `on:60:0 off:60:6000`. The host's `ComputeLRVol` is the circular law over the
  integer pan / 64. `rand`, `srand`, `malloc`, `realloc` and `free` are hooked
  because the CRT's start-up is skipped. `TRACE=1` prints the last blocks on a crash.
- `states.py <plugin> <outdir> <roots...>` writes every event 213 of a plugin
  from `.flp` and `.fst` files (projects as `<dir>__<name>_<n>.bin`).
- `run.py <state> <key> <frames> <release>` runs the DLL (cached in `out/`) and the
  port's `simsynth::tests::dump_for_dll` from the newest release test binary, then
  `cmp.py`. `SIMSYNTH_WORK` names the directory with `states/` and `out/`.
- `batch.py <every nth> <key> <frames> <release>` runs `run.py` over the states.

Results at the commit that added SimSynth: every one of the 77 corpus states at key
48 (6000 frames, let go at 3000), every third at key 72 (9000, let go at 2000), the
render test at key 60, and three presets playing an eight-event chord sequence
(`SIMSYNTH_EVENTS`), all bit for bit.
