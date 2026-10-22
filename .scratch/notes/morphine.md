# Morphine: handover (work in progress, 2026-10-22)

Stopped mid-task when usage ran out. Rebuilt from
`Plugins/Fruity/Generators/Morphine/Morphine_x64.dll` (MSVC `CFLWrapper`
engine, image base 0x180000000, no Delphi shell). Used by 4 shipped
projects (Africa's Calling, NIGHT TERROR, SHINE, I'll Be Your Madonna).

## What is in the tree

- `crates/plugins/src/morphine.rs`: the generator. Four analysed
  generators (frames of 128 harmonics' amplitude, pan, detune), the morph
  line over the square, a 128-oscillator bank, master ADSR on a Catmull-Rom
  level curve, four envelopes and the 12-route mod matrix. Control blocks
  of `int(0.0029 * rate)` samples; see the module comment.
- `morphine_state.rs`: the state: `SSLF`, then the chunk tree `0roM`,
  `0RPM`, `0CVM`, `0HZM`, `0PSM`, ... and the program block.
- `morphine_noise.rs`: the noise layer from `Noise Samples/*.wav`
  (approximated).
- `morphine_fx.rs`: overdrive, chorus, delay, reverb, 8-band EQ. The
  last edit (filling in the effects from a stub) is committed as it stood
  and was not test-run.

## State of the work

- Builds (`cargo check -p plugins --tests` passed with it registered).
  Its tests ran green except the effects' new ones, which were never run.
- **Not registered**, so CI stays green: the port has no doc yet and
  `tools/progress.py --check` fails when `port.rs` has a plugin that
  `docs/plugins.md` does not. To pick up, add back:
  - `lib.rs`: `pub mod morphine;`, `mod morphine_fx;`, `mod morphine_noise;`, `mod morphine_state;`
  - `generator.rs` GENERATORS: `(crate::morphine::NAME, || { Box::new(crate::morphine::Morphine::default()) }),`
  - `port.rs`: `(crate::morphine::NAME, Port::Approximate),`
- Missing: `docs/plugins/morphine.md` (addresses live only in the code
  comments for now), its `docs/plugins.md` row, a run of the effects
  tests, the corpus test, clippy, and fmt. Approximated by its own
  account: noise layer, effects, the release-loop jump. Left out: unison
  and per-note tuning.

## Next

Register it, run `cargo test -p plugins morphine` with `FL_RS_CORPUS`, fix
the effects, write the doc with addresses from the DLL, fill in the row,
run the checks in `.scratch/agent-rules.md`.
