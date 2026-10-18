# Ogun: where it stands

## Confirmed in the binary

`Ogun_x64.dll` is Autogun's code less the random presets. Ghidra project
`ogun` in `/shared/code/fl-studio-target/ghidra/proj`, decompiles in
`ghidra/out/ogun/range_810000.c` and `range_850000.c`. Every plugin function
was matched against Autogun's (normalised decompile diff, plus a check that
the constants they reference hold the same values).

- Plugin class `TTestPlug`, VMT `0x008621f8`, size 0x610 (Autogun 0x620).
  Slots: Dispatcher `0x00862770`, SaveRestoreState `0x00864d00`, GetName
  `0x00869ac0`, ProcessEvent `0x00867c60`, ProcessParam `0x008684e0`,
  Gen_Render `0x0086d450`, TriggerVoice `0x0086a8e0`, Voice_Release
  `0x0086c2a0`, Voice_Kill `0x0086c420`. Constructor `0x00866ec0`, post-load
  `0x008649e0`, quality `0x00866840`, voice render `0x0086c6e0`.
- The timbre engine is at Autogun's address less 0x28b0 (`0x00842340` to
  `0x00844bc0`), byte-identical. Articulators `TOpArticulator` VMT
  `0x00861ef0`; `TMMap` VMT `0x00781658`; fx classes are the same.
- Differences from Autogun:
  1. The state has no random-preset seed word (Autogun reads it for v > 18).
  2. The master level `FUN_0086a570` is `level3(v/128)` with no `* 0.25`.
  3. Gen_Render has no magic dust.
  4. The Dispatcher handles more editor opcodes, which do not touch the sound.
- Old states (versions 0..12, all in presets), same code in both DLLs: the
  block is a prefix of the 0x100 bytes (12 or 8 bytes, then 0xf4 less 1/3/1/16/1/1/6
  for v<1/2/7/9/11/12/13), read over the constructor's block. v<8 has no fine
  pitch. Articulators 4,5 (v<4) and 11,12 (v<10) are not saved and get
  reset. Forced curve flat at 0 for v<5. Curves before v6 are stretched by
  `(2^(order-1)-1)/32767`, a point at x=1 is added at the last level, and
  the curve is resampled at 16 points `i/15` from `2^i/32767`, with the log axis on.
- Quality 0 (17 of the 23 project states) picks FL's linear interpolator
  `0x00610ef0` (Autogun DLL address) through the resampler table at
  `0x008d5d60`. Quality 1 picks the 6-point `0x00611420`. A quality past 4
  changes nothing, so quality 1's settings stay.

## Guessed, not confirmed

- A reset articulator equals Autogun's `blank_state` articulator (from
  `FUN_0087ed10`'s shape).
- `FUN_00784a40` (it drops redundant flat points after the v<6 resample) is
  left out. It should not change the shape.
- The resampler's CPU-flag table (`PTR_FUN_008d5dc0`) is assumed to give the
  same output as the generic path.

## Built

- `crates/plugins/src/autogun.rs` (shared, not mine): a `pub enum Kind
  { Autogun, Ogun }`. `Autogun::new(kind)` and `State::parse_as(bytes,
  kind)` (`parse` is Autogun). The parser now reads versions 0..19
  (`default_block`, `block_rest`, `blank_art`, `blank_shape`, `old_curve`,
  `add_point`). Master level `* 0.25` for Autogun only. No SG for Ogun.
  Linear interpolator for quality 0. Fidelity flags only qualities 2..4.
  Two new unit tests. Module doc mentions Ogun.
- `crates/plugins/src/autogun_engine.rs` (shared): `Setup::linear` and
  `Timbre::linear`. `Timbre::render` runs the linear interpolator when it is set.
- `crates/plugins/src/ogun.rs` (mine): `Ogun(Autogun)` newtype that forwards
  `Generator`. Tests: master level is 4x Autogun's, no magic dust, all 256
  corpus states (v0..v19) parse to the last byte, project states play a
  sane note, presets play with no NaN.
- `lib.rs`: `pub mod ogun;`. `generator.rs`: one registry line.

## Status

- `cargo clippy -p plugins --all-targets -D warnings` clean, rustfmt clean
  on my files. `cargo test -p plugins --lib -- autogun ogun generator`: 25
  pass, run with the corpus.
- Engine corpus before and after: 108 then 110 projects with sound. Ogun silent
  channels 7 then 0. Generators hosted 163 then 170. Ogun.flp peaks at 0.900,
  Electric Loop at 1.000, Astes at 0.980. No NaN. The test passes.

## Next steps

1. Write `docs/plugins/ogun.md`: the addresses above, the state layout
   (point to autogun.md section 1 and add the old versions), the
   differences, quality 0's linear interpolator.
2. Fill the Ogun row in `docs/plugins.md`. Exact: the same as Autogun's,
   plus quality 0 and old-version loading. Approximated: no null test, IPP
   exp, the seed-2 counter, qualities 2..4 (no output oversampling filter,
   quality 4's longer interpolator), portamento, velocity-envelope mode 2,
   the dropped `FUN_00784a40`.
3. Maybe update `docs/plugins/autogun.md` section 6, since quality 0 is now
   exact and versions 0..12 parse.
4. Report the shared-code change in `autogun.rs` and `autogun_engine.rs` to the coordinator.
