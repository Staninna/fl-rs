# PoiZone under the emulator

The harness behind `docs/plugins/poizone.md` and `crates/plugins/src/poizone.rs`,
from Sawer's (`../sawer-emu`). Python 3 with `pefile` and `unicorn`; run with
`python3 -I`.

- `emu.py`: the generic unicorn loader (imports stubbed, a bump allocator).
  `POIZONE_DLL` names `engine_x64.dll`.
- `emu_poizone.py <state.bin> "<events>" <frames> [block] [rate]`: builds a
  `PoiZoneSynth` (`0x18002dca0`, the preset list `0x18002e0b0` skipped, the
  descriptor's counts written at `0x180090f90 + 0x118`), polyphony 0, the
  sample rate, the state, then note events (`on:<key>:<frame>`,
  `off:<key>:<frame>`) and blocks through slot 17. Use block 64, FL's.
  The tempo is the instance's own 120. `POIZONE_INFO=1` prints the random
  generators, the defaults, the first voices' phases and the knobs;
  `POIZONE_TRACE=<n>` the first `n` voice updates (`0x18002c580`) as hex.
- `run.py <state> "<events>" <frames> [tol]`: the DLL (cached in `out64/`)
  against the port's `poizone::tests::dump_for_dll` from the newest test
  binary, then `cmp.py`.
- `batch.py <every nth> "<events>" <frames> [nofx|noarp]`: `run.py` over
  `states/`; `nofx` switches each state's delay and arpeggiator off first,
  `noarp` only the arpeggiator (which is not built).
- `rate.sh <state> "<events>" <frames> <rate>`: the same at another rate.

States come from `../simsynth-emu/states.py poizone states <FL>/Data`.
