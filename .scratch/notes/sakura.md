# Sakura: handover (work in progress, 2026-10-22)

Stopped mid-task when usage ran out. Rebuilt from
`Plugins/Fruity/Generators/Sakura/engine_x64.dll` (MSVC `CSakuraSynth` in
FL's `CFLWrapper` frame, as BassDrum's; `Sakura_x64.dll` is only the Delphi
panel). Used by 3 shipped projects (Africa's Calling, Exoplanet with two
channels, Lightning) and the render test `Sakura.flp`.

## What is in the tree

- `crates/plugins/src/sakura.rs`: state (`SSLF`, 0x24c, `1rkS`, 114 f32
  parameters in the engine's order, 128-byte program name), voices (99,
  the engine at 48 kHz read out at the host's rate in straight lines, 48
  frames at a time), exciter, two strings of two stamped delay lines, the
  mod matrix. The module comment describes the voice step by step.
- `crates/plugins/src/sakura_fx.rs`: what follows the voices: highpass,
  the eight resonators, chorus, plate reverb (`fxReverb`), tape delay
  (`CStringDelay`, `CTapeDelay`).
- `.scratch/notes/sakura-emu/`: the unicorn harness that runs the engine
  DLL's own `CSakuraSynth` for reference output (`emu_sakura.py <state>
  <key> <frames> [block] [rate] [release_at]`; `emu_string.py` runs one
  `CStringDelay`; `cmp.py a b [tol]` compares two outputs). It is
  `tools/plugstate/vst_emu.py` cut down. Paths in it point at
  `/root/work/sakura/`; change `PATH` in `emu.py` to your engine DLL. Run
  with `pefile` and `unicorn` in a venv. The test `dump_for_dll` in
  `sakura.rs` prints the port's output for the same arguments
  (`SAKURA_STATE`, `SAKURA_KEY`, `SAKURA_FRAMES`, `SAKURA_BLOCK`,
  `SAKURA_RATE`, `SAKURA_RELEASE`).

## State of the work

- Builds (`cargo check -p plugins --tests` passed with it registered).
- Against the DLL under the harness, the agent's last two comparisons
  (one plucked, one bowed state, 20000 and 4096 frames) agree to a worst
  difference of 2.5e-5 and 3.6e-5 at peaks of 0.22 and 0.13: the model is
  right, with a small drift left to find, which starts around frame 14439 and frame 454.
- **Not registered**, so CI stays green (no doc yet, and
  `tools/progress.py --check` fails when `port.rs` has a plugin that
  `docs/plugins.md` does not). To pick up, add back:
  - `lib.rs`: `pub mod sakura;` and `mod sakura_fx;` (after `reeverb_2`)
  - `generator.rs` GENERATORS: `(crate::sakura::NAME, || { Box::new(crate::sakura::Sakura::default()) }),`
  - `port.rs`: `(crate::sakura::NAME, Port::Gaps),` (or `Exact` once the
    drift is found)
- Missing: unit tests from the maths (only the two probe tests
  `probe_render_test` and `dump_for_dll` exist, and they assert nothing), a
  corpus test that every shipped state loads and plays,
  `docs/plugins/sakura.md` with addresses (they live only in code
  comments), its `docs/plugins.md` row, clippy, and fmt.

## Next

Register it, find the drift with the harness, turn the probes into real
tests, write the doc and the row, run the checks in
`.scratch/agent-rules.md`.

`sakura-emu/emu_sakura.py` used to send an event that fell inside a block at the next block's start. It now ends a block at the next event. The differences quoted above were measured before that change, so render them again before trusting them for a note-off that is not on a 512-frame boundary.
