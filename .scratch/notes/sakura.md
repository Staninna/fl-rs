# Sakura: what is left (2026-10-22)

Sakura is registered, tested and documented (`docs/plugins/sakura.md`,
`Port::Gaps`). Against `engine_x64.dll` run under unicorn it plays bit for
bit on every preset state swept but one, whose pitch moves and whose voice
step drifts by an f64 step through the runtime's `pow`. What is left is
optional:

- Transcribe the MSVC runtime's f64 `pow` (`0x180042ef0`, called through
  `0x18002e650` for `2^x` and directly for the lo-cut, damping and noise
  gate). Rust's parts from it in the last bit on 6 of 4000 arguments of
  `2^x`. Read the FMA3 build too (the CPU switch the runtime reads is
  `0x180083040`; FL on a CPU with AVX and FMA runs that one, the emulator
  the SSE2 one). `low_lifter.rs`'s `msvc` module transcribed the f32
  `tanf`, `expf` and `powf` of another DLL's runtime the same way.
- The same for the f32 `powf`, `expf`, `tanf`, `cosf` (`0x180044590`,
  `0x18005aac0`, `0x18005bbd0`, `0x18005a5c0`): the port rounds f64
  results once, which matched the SSE2 builds on 1500 arguments each but
  for two cosines.
- The engine's noise is one static generator for every Sakura FL has
  loaded; the port gives each instance its own.

## The harness (`sakura-emu/`)

`pip install pefile unicorn capstone numpy` into a venv. `emu.py` loads the
DLL from `SAKURA_DLL` (default: this container's FL install path).

- `dump.py <FL Data dir> <work dir>` writes every Sakura state in the
  corpus to `<work dir>/states/<file>_<n>.bin` (the render test is
  `Sakura_3.bin`).
- `emu_sakura.py <state> <key> <frames> [block] [rate] [release_at]
  [velocity]` prints the DLL's output, a block ending at each event, the
  velocity a MIDI one. With `SAKURA_INNER=<prefix>` it also writes the
  engine's 48 kHz samples (`.0`), the host-rate samples before and after
  the effects (`.1`, `.2`), after the highpass, resonators, chorus and
  reverb (`.3` to `.6`), and voice 0's block state (`.v`).
- The port's side is the test `sakura::tests::dump_for_dll`
  (`SAKURA_STATE`, `SAKURA_KEY`, `SAKURA_FRAMES`, `SAKURA_RELEASE`,
  `SAKURA_VELOCITY` as FL's 0..128, `SAKURA_RATE`, `SAKURA_OUT`;
  `SAKURA_FTZ` sets the harness's MXCSR). It writes only the output; to
  bisect, mirror the `.0` to `.6` stages with a test-only hook at the
  matching points of `process`, `render_inner` and `effects`.
- `run.sh <state> <key> <frames> [release]` (with `SAKURA_WORK`, `PYTHON`,
  `BIN` or `CARGO_TARGET_DIR`) runs both and compares with `cmp.py`, which
  compares as f32.

What the bit-matching needed, for the next engine of this kind: constants
that are not the nearest f32 to their decimal (1.8 as `0x3fe66667`, 0.002
as `0x3b03126e`, 0.8995 a step low in f64, pi as 3.141592653 for the LFO),
the order of every product and sum as the listing has it, the engine's own
vector `sinf` and `sin`, and the runtime's f32 maths through f64.
