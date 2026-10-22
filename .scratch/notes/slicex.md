# Slicex: what is left

`docs/plugins/slicex.md` maps the plugin; `crates/plugins/src/slicex.rs` builds
all of it but the parts below, and `fidelity()` names each when a state asks
for it. No corpus state does (checked over all 50: every slice takes group 0
for all four articulator groups, every map is flat at its neutral level,
cut groups and start words are 0, the layer mode is 0; no note in the
first 10 s of the six projects with Slicex has a colour). Ghidra's project is `/shared/code/fl-studio-target/ghidra/proj/slicex`;
`analyzeHeadless ... -process Slicex_x64.dll -noanalysis -readOnly` with
`tools/ghidra/DecompileAddrs.java` decompiles in seconds.

- Speed articulator (item 4): `TriggerVoice` (`FUN_00881d70`) sets
  voice+0x7c4 = 1 and the flag +0xf8 when its envelope or LFO is on; each
  tick (`FUN_008843d0`) the speed is `(value * amount(+0x78) + 1) * base
  speed` and the gains are scaled by `local_6c`, the player's move since the
  last tick times `+0xac = 1 / +0xa8`, while that move is under `+0xa8`
  (`FUN_0062bcb0(sample, 0.001)` frames times the tick). Not read further.
- Sample start articulator (item 5): only its value at the trigger counts
  (`FUN_00799c70`'s start value, combined with the start value 0); the
  start then follows section 4 of the doc.
- Maps: `FUN_00887610` bakes the mod X and mod Y maps into 513 steps by
  running the map as an envelope (`FUN_007971e0`, step `DAT_00887714`);
  the velocity map sets the value at the trigger (`FUN_00797dc0(map,
  velocity)`), the random map scales it by `(map(random) - 1) *
  randomness + 1` (or adds `map * randomness`). The mod wheels are smoothed
  by `+0x1040 = e^(ln 0.01 * unit / 96)` a tick when `+0x1315` is set.
- Cut groups: `FUN_00885150(plugin, group)` on a hit with word 0 > 0. Not
  decompiled.
- Layer modes 2 to 7 and the crossfade curves: `TriggerVoice` lines 60-250
  of the decompile; mode 7 reads the song position (host event 0x24).
