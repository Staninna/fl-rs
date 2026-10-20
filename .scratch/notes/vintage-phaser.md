# Vintage Phaser: handover notes

Not built. The engine is several times Vintage Chorus's (oversampling,
four-wide SIMD stage models, 36 parameters), so it was left at a map.
One shipped project uses it: Purps On The Beat - Stroll Up ("Lowgrounder",
149 bytes, version 3). Read `docs/plugins/vintage-chorus.md` first: the
shell and the VST host around the engine are the same design.

## Where the DSP is

`Plugins/Fruity/Effects/Vintage Phaser/Vintage Phaser_x64.dll` is a Delphi
shell (`CreatePlugInstance` 0x006e9f90, constructor `FUN_006eac30`, class
0x006e9df8: Dispatcher 0x006ea1e0, SaveRestoreState 0x006ea8e0, GetName
0x006ed890, ProcessParam 0x006eb1a0, Eff_Render 0x006edbf0). It loads
`engine_x64.dll` beside it, an MSVC VST 2 plugin (313 kB, exports
`VSTPluginMain` 0x18001e270, `TestPlug` 0x508 bytes, 0x24 = 36 parameters,
vtable 0x180030950). Ghidra project `fx6` on the machine this started on
(`/root/ghidra/fx6`), every engine function decompiled to
`out/vphaser_engine.c`, listing from 0x180016000 in `out/vpe.s`; both are
scratch and not kept.

Engine entry points (the DSP object `E` is `TestPlug+0xf0`):

| what | address |
|---|---|
| `E` constructor | `FUN_18001a250` (2156 bytes) |
| setSampleRate: `E` rate setup, then every parameter again | `FUN_18001d020` -> `FUN_180019560` |
| setParameter: 36 cases into `FUN_180019450` -> `FUN_180018cd0` (1672 bytes); VST indexes 0x16 and 0x17 swap | `FUN_18001dc80` |
| getParameter / names / display | `FUN_18001da80`, `FUN_18001d790`, ... |
| processReplacing: `VstTimeInfo` (0x3e02), ppqPos when playing else `DAT_180031630`, tempo | `FUN_18001d040` |
| block render | `FUN_180018210` (2021 bytes) |
| LFO, once a frame | `FUN_180001190` (1230 bytes) |
| upsampler a side, factor `E+0x194` (the HQ switch) | `FUN_18001bd20`, objects at `E+0x1a0`, `E+0x1a8` (`FUN_18001be70`) |
| phaser stages, four-wide | `FUN_180001660` |
| the delay section when `E+0x400 == 1` | `FUN_180001940` (1648 bytes) |
| the noise's filter design (`4` = a type) | `FUN_18001b780` |
| final colour EQ when `E+0x2b0` | `FUN_18001c200` (1198 bytes) |
| `pow` | `FUN_1800224b0` |

Per frame in `FUN_180018210` (read from the decompile, not checked against
the listing):

1. LFO step; mono sums the input at 0.5 (`E+0x1c0`).
2. Two biquads in series a side (`E+0x88..0xb8`, then `E+0x8..0x38`): the
   input high-pass and the colour low shelf, by their parameter names.
3. A gain glide `E+0x408` towards `E+0x404` by `E+0x40c`; the input times
   it goes into the upsamplers, the stages run at the higher rate, the
   delay section if on, and the result is divided by the glide again
   (`DAT_1800314bc / E+0x408`).
4. Noise when `E+0x160 > 0`: the same LCG as Vintage Chorus (`*
   0x0bb38435 + 0x3619636b`, `2^-31`), each side through a biquad
   redesigned every frame at frequencies from `E+0xd4`, `E+0xd8`.
5. A noise gate like Vintage Chorus's (`E+400`, threshold `E+0x170`, hold
   `E+0x174`, fall `E+0x180`), here keyed on the outputs and on input plus
   noise.
6. The colour EQ (`FUN_18001c200`) when on.

The tempo sync in `FUN_180018210` (`E+0xc4`, `E+0x1b0`, `E+0x1b8`, `E+200`,
`E+0x240`, `E+0x1e0/0x1e8`) is Vintage Chorus's scheme in f64.

## Shell state (confirmed from the decompile of `FUN_006ea8e0`)

- Save: `i32 3`, `this+0x180` i32[36] (0x90 bytes), one byte (the UI
  toggle at `+0x48e` of the control at `editor+0x7e8`, an editor option).
- Restore: version 4 or more ignored. Version 1: 32 i32 (0x80 bytes)
  scattered through the table at `0x00742a30` into `this+0x180`, and
  parameter 30 (`this+0x1e0`) from a control's default. Versions 1 and 2:
  parameters 33..35 (`+0x204`, `+0x208`, `+0x20c`) set to 0. Version 2
  and 3: the 0x90 bytes; version 3 also the byte. Then GetValue /
  UpdateValue (2, 0x11) over every parameter, as in the other shells.
- Control tags and ranges are in the DFM (`tools/plugstate/dfm.py`): the
  sliders' Min / Max are not 0-based (frequencies 125..2048000, delay low
  pass 3200..204800, rates 25..150000, ...), so `v = (value - Min) /
  (Max - Min)` as in Vintage Chorus.

## Next steps

1. Confirm the shell's ProcessParam (0x006eb1a0) hands `v` to the host's
   setParameter with the same index, and read the VST 0x16 / 0x17 swap.
2. Read `FUN_180018cd0` for the 36 parameter maps, `FUN_18001a250` for the
   constructor values and `FUN_180019560` for the rate setup (the
   four-wide stage coefficients and the oversampled rate
   `SR * E+0x194`).
3. Read `FUN_180001190`, `FUN_180001660`, `FUN_180001940`,
   `FUN_18001bd20` / `FUN_18001be70` and `FUN_18001c200` with their
   listings: the four-wide code mixes `divps` and per-lane constants
   (`0x18003bd30..0x18003bdbc`) that the decompile shows only as globals.
4. Port it like `crates/plugins/src/vintage_chorus.rs` (shell + engine in
   one file), add it to `lib.rs` and `port.rs`, and the
   `("vintage phaser", 149, 1)` row to the corpus table.
