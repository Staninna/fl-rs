# FLEX: handover

State on this branch: FLEX plays. `crates/plugins/src/flex.rs` (voice),
`flex_pack.rs` (pack index, XOR mask, FLAC), `flex_preset.rs` (v1 preset
reader), `flex_waves.rs` (the 28 band-limited cycles). The format and the
voice are in `docs/plugins/flex.md` sections 3 to 7, with addresses.

## Tools used

- Ghidra project `/root/ghidra-projects/flex` (engine_x64.dll analysed once;
  reuse with `-process engine_x64.dll -noanalysis -readOnly`). Decompiles of
  this session were in the scratchpad and are not kept; the doc names every
  function.
- Python helpers worth rewriting if needed: pack index parse (big-endian
  index, see doc), key guess = most common byte per position mod 16, then
  check `xelf00su`.

## Next steps, in order of what they would change in the corpus

1. Master effect chain: the `hcxf` after the layers in `FUN_1803f1300`
   (delay type 6 = ILDelay3Plugin, reverb type 7 = FruityReeverb2,
   limiter, master filter type 9 FilterProcessor). The shell's params
   21..44 drive them through `FUN_1803eefc0` -> `FUN_1803ed870(chain,
   effect type, param index, value)`. Most corpus presets have reverb on
   (shell 38) at mix ~0.46: the biggest audible gap.
2. LFOs: `LFO` vtable 0x1804867c8 (setParam 0x1803d22f0, display
   0x1803d2460); sources 261..268.
3. FM (`Osc1->OscN FM`, osc r19..22) and AM (673..676), PW (r16..18).
4. Unison: detune curve `FUN_180321170(8 r4 + 2, x)` and gain table
   `DAT_1819221ec` (filled at run time; find its writer).
5. Filter types 5 (Minisynth), 6 (Vowel), 10..19 and the Moog/Juno
   ladders: `FUN_180397870` (FilterProcessor ctor), `FUN_180395c00`.
6. Layer effect chains (corpus: types 1, 2, 4, 8, 9, 10, 12, 14, 15, 19;
   FL plugins carry their own state and could be hosted through
   `crate::by_name`).
7. `.flex2pack` via `engineV2_x64.dll` (Templates/Minimal/Basic with
   limiter uses Core 808s).
