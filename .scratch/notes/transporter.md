# Transporter: handover

Stopped because the DSP is not on the machine. Transporter_x64.dll is a
Delphi shell; the sound is `TransientLooper`, from
`TransientLooper_CreateInstance` in `Shared/ildsp_x64.dll`
(`FUN_00710800` -> `FUN_005e8910` -> `FUN_005e8890` loads `ildsp`). The
FL install at `/root/fl-studio/drive_c/Program Files/Image-Line/FL Studio 2026`
has no `Shared/` directory at all, so nothing of the engine could be read.

## Done

- `docs/plugins/transporter.md`: the shell in full. VMT, object fields,
  state versions 3/4/8 (85/94/110 bytes) with the read-short behaviour,
  the 22 parameters with ranges and the exact maps to the engine's
  `SetParam` values, engine parameters 22..25 (tempo, lookahead,
  transient source, architecture), the engine vtable the shell uses, and
  what `Eff_Render` passes (sidechain in/out, beat buffer).
- `crates/plugins/src/transporter.rs`: `Transporter::set_state` and
  `engine_value(index)`, with tests over every corpus and preset state
  (all load). Not an `Effect`, not in `by_name`, no `port.rs` line.

## Next

1. On a machine whose install has `Shared/ildsp_x64.dll`, import it in
   Ghidra (or reuse the `ildsp` project Luxeverb/Spreader used), find the
   export `TransientLooper_CreateInstance` and its class: the vtable slots
   +0x08 rate, +0x10 SetParam, +0x18 defaults, +0x28 render, +0x30 flush,
   +0x40 latency. Read the defaults first (parameter 20's is unknown here).
2. Map the transient detector (sensitivity, source, lookahead), the loop
   capture (minimum length A, ratio, the four loop modes, Tempo mode's
   ratio length and trigger against the beat buffer), the playback
   (reverse, octave shift, spread/auto pan, bouncing rate), the
   probabilistic and deterministic choosers (find the RNG and its seed:
   the output depends on it), and the mix (dry, loop A, loop B levels in dB).
3. Make `Transporter` an `Effect` with `follows_song` (see `gross_beat.rs`
   for `set_tempo`, `set_ppq`, `tick`), the second output through
   `aux_output`/`sidechain_output`, the input track through
   `sidechain_input`/`set_sidechain`, then the lib.rs registration, the
   port line and the docs/plugins.md row.

The corpus is small: the render test (the default) and one demo song
(Edlan & Ella Noël - Song For You: dry 11000, loop A 5362, loop B 5675,
reverse 0.4969, Bouncing mode, minimum length 2 s, ratio 12 (4.0),
trigger Off).
