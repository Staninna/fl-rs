# FLEX: handover

State on this branch: FLEX plays (port level Approximate).
`crates/plugins/src/flex.rs` (voice), `flex_pack.rs` (pack index, the
exact XOR mask, FLAC via symphonia), `flex_preset.rs` (v1 preset reader to
the end of the layers), `flex_waves.rs` (the 28 band-limited cycles, 24 of
them read from the installed `engine_x64.dll`). Format, mask derivation and
voice are in `docs/plugins/flex.md` sections 3 to 7, with addresses.
The master chain's delay runs as `delay_3.rs`.

Corpus render after (commit 4b4e112): 223 of 227 generator channels with a
note hosted, 4 silent (Fruity Voltage Controller, PoiZone, Sawer,
SimSynth); FLEX is off the silent list. 12 FLEX channels play; the render
test `System/Render tests/FLEX.flp` names the pack "Synthesizer Science",
which FL 26.1.7 does not install, and stays silent. The template "Basic with
limiter" names a v2 pack (Core 808s). No FLEX project peaks above 1.

## Tools

- Ghidra project `/root/ghidra-projects/flex` (engine_x64.dll analysed
  once; reuse with `-process engine_x64.dll -noanalysis -readOnly` and
  `tools/ghidra/DecompileAddrs.java`). The decompiles of this session were
  in the scratchpad and are gone; the doc names every function.
- Engine classes by RTTI: Voice vtables 0x180489ba0 / 0x180489ae0, Osc
  0x180487a70, Env 0x1804836d8, LFO 0x1804867c8, FilterPlayer 0x180483b20,
  Zone 0x180489e18 (+8: 0x180489e78), Layer 0x180486660. The 23-slot
  `SimpleAudioProcessor` vtable: +0x28 set rate, +0x48 count, +0x50 set
  param, +0x58 get param, +0x68 display text.

## Next steps, in order of what they would change in the corpus

1. Master effect chain: the `hcxf` after the layers in `FUN_1803f1300`
   (parse it with `flex_preset::chain`; Maximus, type 13, carries a u16
   version, the FL state, three floats and from version 1 an i32). The
   corpus chains are filter (9), chorus (2), delay (6 =
   `ILDelay3Plugin`), reverb (7 = `FruityReeverb2`), Maximus (13). The
   shell's params override them through `FUN_1803eefc0` ->
   `FUN_1803ed870(effect type, param)`: delay type 24->6.2, mix 25->6.24,
   time 26->6.5, feedback 27->6.15, mod 28->6.10, colour 29->6.25; reverb
   mix 30->7.14, decay 31->7.10, size 32->7.6, mod 33->7.20, colour
   34->7.11, mod speed 44->7.21; master filter 21..23 -> 9.1..3.
   The delay's params are FL Delay 3's 26 shifted by one (param 0 is the
   engine's), normalised over the ranges in `delay_3.rs`, so
   `crate::delay_3` can be fed a built 108-byte state. The reverb's
   (setParam `FUN_1803b7780`) map onto FL Reeverb 2 ints with their own
   scales (case 2 low cut `v*2981+19.5`, 3 high cut `v*216+5.5`, 6 room
   `v*99+1.5`, 7 diffusion `v*100`, 9 crossover `v*1975+25.5`, 10 decay
   `v^3*199+1.5`, 11 damping `v*216+5.5`, 12..14 levels `v*421-360.5`):
   check against `reeverb_2.rs` before building.
2. FM (osc r19..22) and PW (r16..18): used by most Melodic Techno and
   several Arksun presets. The per-sample oscillator render was not found;
   start from the waveform object at osc+0x9d8 (reset `FUN_180372040`,
   tables `DAT_180514a80`) and look for its readers.
3. LFOs: `LFO` setParam `FUN_1803d22f0` (r0 wave int(15v), r1 rate via
   `FUN_1803d1810`, ...); sources 261..268. Corpus routes mostly carry
   amount 0, a few move filters, PW and AM.
4. Unison: detune curve `FUN_180321170(8 r4 + 2, x)` and the gain table
   `DAT_1819221ec` (filled at run time; find its writer).
5. Filter types 5 (Minisynth), 6 (Vowel), 10..19 and the Moog/Juno
   ladders: `FUN_180397870`, `FUN_180395c00`.
6. Layer effect chains; FL plugins in them carry their own state and could
   be hosted with `crate::by_name`.
7. `.flex2pack` via `engineV2_x64.dll` (the template "Basic with limiter"
   uses Core 808s). Its entries unmask with the same keys.
8. Bought packs: master = MD5 of the product name (or + `_subscribed`).
