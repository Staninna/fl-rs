# Hardcore: what is left

`crates/plugins/src/hardcore.rs` plays the rack and the Distortion pedal
(`CPedal_DistModelA`), which is all FL's one project (Treben - Umziky) and
the render test put in a box. `docs/plugins/hardcore.md` has the map.

Left, in the order a project would need them:

- The cabinet (parameter 11): the render test asks for cabinet 4. The
  impulse is the 4th file of `Plugins/Fruity/Effects/Hardcore/Imp` in the
  file list's order (`CFileList` at `param_1[0x2d8]`, built in the
  constructor `FUN_180029820` with the `Imp` folder; check the sort, "Model I
  .wav" has a space). Load (`FUN_180026f00`): the WAV resampled to the rate
  (`FUN_18001b760`), its length rounded up to 4, both sides; process
  (`FUN_180027260`): a direct FIR that scatters each input into an
  accumulator, left and right, output the accumulator's head. Check for a
  normalisation in `FUN_180026f00` past the part read.
- The equaliser (parameters 2..10): eight `FUN_18001d660` sections a side
  (`param_1[0x2e1 + k]`, `param_1[0x2e9 + k]`), type and frequency set in the
  constructor `FUN_180029820` (the loop before the "EQ Band" names); gain `±18 (2v - 1)^2` dB; types 0 low shelf, 1 peak, 2
  high shelf (the shelf branches of `FUN_18001d660` are decompiled).
- The ten other pedals, by kind (`FUN_18002a010`): 2 `FUN_18002af30`, 3
  `FUN_18002cdd0`, 4 `FUN_18002d8f0`, 5 `FUN_18002de10`, 6 `FUN_18002b680`, 7
  `FUN_18002d640`, 8 `FUN_18002c8a0`, 9 `FUN_18002b3c0`, 10 `FUN_18002d1a0`, 11
  `FUN_18002c3e0`; the RTTI names (Compressor, Chorus, Delay, Eq, Flanger,
  Modulator, NoiseGate, Phaser, Reverb, DistTube) tell which is which. The
  shipped presets use 2, 6, 7 and more.

Ghidra: `/root/ghidra-projects/distructor`, program `hc` (analysed); the
whole of the plugin's code decompiled into `out/hc_all.c`
(`DecompileRange.java 0x180001000 0x180030000`). MSVC's `powf` is
`FUN_180042a10`, `sincosf` `FUN_180055e50`, `sincos` `FUN_180055de0`, `sinf`
`FUN_1800598d0`, `cosf` `FUN_180058a40`, `sinh` `FUN_180059fc8`; Ghidra drops
their arguments, so read the call sites' listing.
