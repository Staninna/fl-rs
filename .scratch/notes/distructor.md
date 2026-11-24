# Distructor: what is left

`crates/plugins/src/distructor.rs` plays every module FL's projects and
presets use but for 18 filter types. `docs/plugins/distructor.md` has the map.

- Filter types 2..12 and 15..21 pass their input (`fidelity()` says so).
  No project has one; the presets use 2, 3, 4, 12, 15..18 and 20. Their
  code: `FUN_0071cf80` in `Module_x64.dll` dispatches to `FUN_0071b910`
  (with `FUN_0071b6e0`/`FUN_0071b710`/`FUN_0071b6b0` as the per-type
  design), `FUN_0071be70`, `FUN_0071c6e0`, `FUN_0071b800` (the phasers, on
  the objects at filter `+0x48..+0x58`), `FUN_00719080`/`FUN_00719100`
  (vowel) and `FUN_007178a0`/`FUN_00717d30` (Lowpass 24 Alt). All are
  decompiled in `/root/ghidra-projects/distructor/out/c`.
- The cabinets are used at their files' rate; FL's `TSample` load
  (`FUN_00733a40`, `FUN_005f4a90`) may resample them at other rates.
- The 15 ms smoother base is taken as `ln 0.01` from Harmor: the variable
  behind `PTR_DAT_007aefc0` (`0x007be268`) is written by code not found.

Ghidra: `/root/ghidra-projects/distructor`, program `mod` (`Module_x64.dll`,
analysed). The rack (`Distructor_x64.dll`) was read from its listing only.
