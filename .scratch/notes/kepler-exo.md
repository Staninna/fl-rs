# Kepler Exo: handover

Built: `crates/plugins/src/kepler_exo.rs` (port level Approximate), mapped
in `docs/plugins/kepler-exo.md`. It reads every Kepler-2 (and Kepler-1)
state and plays it through `kepler.rs`'s engine, which is the same engine
(same `.data`, `.text` within 0x90 bytes). 186 of the 188 presets turn on
something that engine port does not build, so they play approximately.

## Confirmed (decompiled, `engine_x64.dll` of Kepler Exo)

- Load `FUN_18001efd0`, save `FUN_18001ed60` (writes version 4, 0x9b
  params, matrix, three bytes), defaults `FUN_18001ec40` ->
  `FUN_18001d790`, parameter names `FUN_18001fa90`, matrix source names
  `FUN_180017b20`.
- Voice per frame `FUN_180023b40`, frame `FUN_18001b7d0`, control-rate
  voice update `FUN_180016180`, matrix source read `FUN_180023a70`.

## Next steps, by how many presets need them

1. EQ (170 presets), hyper chorus (142), filter FM (116), reverb (153 have
   0x79 on), delay (105), DCO 2 (155) with hard sync (91), AM (77),
   saturation (81), matrix (102), mono/legato/porta (63), slopes (89 not
   at 1.0).
2. Start with DCO 2: `FUN_180023b40` from `if (cVar4 != '\0')` (DCO 2
   pitch, `+0x2e40`), the voice coefficients the parameter handler writes
   (`FUN_180021d00` is the parameter-change entry; find the 0x0c..0x1f
   cases), and the oversampled loop of `FUN_18001b7d0` where each voice's
   DCOs run (`FUN_180014f80`, `FUN_180014da0`, `FUN_180014d00`: hard sync
   and cross mod live there).
3. Then the second envelope (same code as the first at `+0x2df8`) and
   LFO 2 (shape 0x3a) in `FUN_18001b7d0` after LFO 1 (`FUN_1800140c0`).
4. Identify the effects among `FUN_180004320`, `FUN_1800078c0`,
   `FUN_18000ba00`, `FUN_18000c6e0`, `FUN_18000d630` by the parameters
   they read (`this+0x1ab80 + 4 * index`).

Keep Kepler bit-identical: build each part behind its parameter, so that
a Kepler-1 state (all Exo parameters at their defaults) takes the paths
`kepler.rs` has now. `kepler_exo::tests::a_kepler_1_state_plays_as_kepler_does`
holds the two together.

## Tools

Ghidra headless on the Exo engine, `DecompileRange.java` over
0x180001000..0x180040000 (538 functions). The parameter names come out of
`FUN_18001fa90`'s switch.
