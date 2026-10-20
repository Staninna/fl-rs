# GMS (Groove Machine Synth): handover, not built

Started 2026-10-20 next to Fruity Slicer and stopped after mapping: GMS is
far bigger than a "subtractive synth". It is FL Groove's synth in a C++
wrapper (MSVC, RTTI): three wavetable oscillators with unison, noise and a
modulation mode, a filter, an amp ADSR, two envelopes and two LFOs routed
by destination, a three-slot mod matrix, a master section, and a chain of
ten effects (distortion, crusher, flanger, phaser, low pass, high pass,
delay, reverb, panner, trancer), each with its own LFO. The render is
hand-vectorised over four voices at a time. Nothing of it is in
`crates/plugins` yet.

Corpus: 10 states in 6 `.flp` (the render test `System/Render tests/GMS.flp`
and 5 demo songs: Asher Postman - Future House, Chan Gibbons x Ceebeaats -
Time, Cuebur & Thandi Draai - Africa's Calling, Edlan & Ella Noel - Song For
You, Mehdi Ebrahiminejad - Never Out), all 4968 bytes; 306 presets as
`.gmsynth` files under `Data/Patches/Plugin presets/Generators/GMS/` (the
chunk below without the `SSLF` header, magic `Syn3`, 4948 bytes, not event
213 states). Two channels with notes are silent in the corpus render.

Binary: `Plugins/Fruity/Generators/GMS/GMS_x64.dll` (image base 0x180000000).
Ghidra project `/root/ghidra/slicer-gms/proj/gms` (on the cloud machine);
`/root/ghidra/slicer-gms/out/gms/core.c` is every function in
0x1800c6000..0x180107000 decompiled into one file (`DecompileRange.java`),
`strings.tsv`/`functions.tsv` from `DumpFlEngine.java`. Rebuild with:

    analyzeHeadless <dir> gms -import GMS_x64.dll -max-cpu 1 -scriptPath tools/ghidra \
      -postScript DecompileRange.java core.c 1800c6000 180107000

## State (confirmed: loader read, all 10 corpus states agree)

`CPluginWrapper` save/restore `FUN_1800c82f0`: `u32 'SSLF' (0x464c5353)`,
`u32 size`, then `size` bytes handed to `GMSynth` vtable `+0x128`
(load) / `+0x118` (save, `FUN_1800f43d0`). Load is `FUN_1800f4e60`:

```
+0x08  u32 magic        'Syn4' 0x53796e34 (FL writes), 'Syn3', or 'S4DE' (demo build)
+0x0c  7 x char[0x200]  oscillator 1..3 wave paths (FUN_1800f1570), then 4 unused blocks
+0xe0c i32              +0x4474 (Syn4 and S4DE only): the selected FX slot (used with +0x448b in FUN_1800f5f00)
+0xe10 i32              +0x4478
+0xe14 i32              +0x447c
+0xe18 f32[212]         the parameters, 0..1, by index (FUN_1800cd6c0: param[i].value at +4 of 0x94-byte records)
+0x1168 char[0x200]     preset name, vtable +0xa0 (corpus: "Agressive TE" and so on, padded with 0x02)
```

Wave paths are absolute paths of the machine that saved them
(`C:\Program Files (x86)\Image-Line\FL Studio 11\Plugins\Fruity\Generators\GMS\Synth Waves\Beast.wav`).
`FUN_1800f1570` renames "Devils Horn" to "Double Spike", and when the file
is not there tries its file name under `+0x4070` (a user directory) and
`+0x4270` = the plugin's own `Synth Waves` directory (built in the
constructor `FUN_1800f0b20` from the DLL's path). So a port takes the file
name and reads it from `<resource dir>/GMS/Synth Waves/` (all 26 waves ship
there), as Slicex does with `set_resource_dir`. Each oscillator has a
0x200000-byte table (`param_1[0x74b..0x74d]`); how the WAV becomes the
table is the rest of `FUN_1800f1570`, not read yet.

## Parameters (names from `FUN_1800f28f0`, values from the corpus)

| index | name | notes |
|---|---|---|
| 0 | CHN: Mix Level | 0.51..1.0 |
| 1 | CHN: Panning | 0.5 |
| 2 | CHN: Pitch | 0.5 centre |
| 3 | CHN: Note Length | 0.4444 everywhere |
| 4..7 | CHN: Amp Attack, Decay, Sustain, Release | |
| 8 | CHN: Amp Level | |
| 9 | CHN: Shuffle Enable | 0 |
| 10, 11 | CHN: Slide Time, Slide Enable | |
| 12..15 | Filter Cutoff, Resonance, KBD Tracking, Type | type 0 or 1 in the corpus |
| 16, 17 | Envelope 1/2 Amount | |
| 18, 19 | Envelope 1/2 Invert | |
| 20, 21 | Envelope 1/2 Attack | |
| 22, 23 | Envelope 1/2 Decay | |
| 24, 25 | Envelope 1/2 Destination | thirds: 0, 0.3333, 0.6667, 1 |
| 26, 27 | LFO 1/2 Retrig | |
| 28, 29 | LFO 1/2 Amount | |
| 30, 31 | LFO 1/2 Invert | |
| 32, 33 | LFO 1/2 Shape | |
| 34, 35 | LFO 1/2 Rate | `FUN_1800efb10(rate, sync, ...)`; `FUN_1800efa80` maps a synced rate to 8 note lengths |
| 36, 37 | LFO 1/2 Sync | |
| 38, 39 | LFO 1/2 Destination | |
| 40 | SYNTH: Mono Voice | |
| 41..43 | SYNTH: Unison Voices, Detune, Stereo | |
| 44 | SYNTH: Octave | quarters |
| 45 | SYNTH: Noise Mix | |
| 46, 47 | SYNTH: Unison Phase, Unison Phase Retrig | |
| 48, 49 | SYNTH: Mix Oscillator 2, 3 | |
| 50, 51 | SYNTH: Modulation, Mod Kind | kind 0, 0.5, 1 |
| 52..54 | SYNTH: Phase Offset Osc 2, Phase Retrig Osc 2, Osc 2 Invert | |
| 55..60 | SYNTH: Osc 1..3 Pitch, Detune | pitch 0.5 centre |
| 61..116 | unnamed | 61..72 0.5, 81..84 and 89..92 1, rest 0 (one state all 0): likely Groove's step sequencer, unread |
| 117 + i | FX i X | i = 0..9: Distortion, Crusher, Flanger, Phaser, Low Pass, Hi Pass, Delay, Reverb, Panner, Trancer; X = Distortion, Crush, Time, Depth, Frequency, Frequency, Feedback, Feedback, Panning, Release (`FUN_1800f27b0`) |
| 127 + i | FX i Y | Filter, Filter, Feedback, Feedback, Bandwidth, Bandwidth, Filter, Filter, Binaural, Depth (`FUN_1800f2850`) |
| 137 + i | FX i LFO Rate | `FUN_1800efa80` |
| 147 + i | FX i on | read as the effect's switch in `FUN_1800f5f00` (`iVar15 + 0x1e`); the name table calls it "Glue" |
| 157 + i | FX i LFO Shape | `(int)(v * 5)` clamped 0..4 (`DAT_18014d60c`) |
| 167 + i | FX i LFO Sync | |
| 177 + i | FX i LFO Amount | `(2v - 1)^2` with the sign kept |
| 187..196 | unnamed | 0 |
| 197..204 | mod matrix | 198/201/203 amounts, 199/202/204 destinations as `(int)`; `FUN_1800f50c0` adds `(2 amount - 1) * source` to a parameter whose index matches. Several corpus states hold garbage floats here (1.8e31, 6.0e13) that cast to no index |
| 205 | | 0.1667 |
| 206 | master section on | `FUN_1800f5f00`: switches `param_1[0x740]` (+0x836) |
| 207..211 | master section values | clamped 0..1 into `+0x818..+0x828` |

## Classes and functions

- `GMSynth` vftable `0x18014c940` (ctor `FUN_1800f0b20`): 212 parameter
  records of 0x94 bytes at `+0x1040`, 99 voices `+0x44a0..` (0x2510 bytes,
  ctor `FUN_1800f0650`), 10 FX units `+0x3ab8` (`param_1[0x737..0x740]`,
  0x840 bytes, ctor `FUN_1800fefe0(index)`: 16+16 filter stages, 5+5 delay
  lines), a 64K-entry noise/random table `param_1[0x748]`.
- `GMSynthVoice` vftable `0x18014cb28`.
- `FUN_1800f32f0` (vtable slot 17): the render. Gathers the sounding
  voices, runs them four at a time with SSE, then the FX chain and the
  master. `FUN_1800f5f00` at its start updates the LFO phases and every FX
  unit's settings from the parameters each block.
- `FUN_1800f5250`: note on (voice allocation, mono via parameter 0x28).
- `FUN_1800f50c0(param)`: a parameter's value with the mod matrix applied;
  `FUN_1800cd7a0` the raw value.
- `FUN_1800efbc0`: read from the 64K-pair table.
- Candidate DSP, unread: `FUN_1800f58e0`, `FUN_1800f68f0`,
  `FUN_1800f7530` (voice parts), `FUN_1800ff500`..`FUN_1801015d0` (FX unit
  methods), `FUN_180103020`, `FUN_180103f30`, `FUN_180104e30` (three large
  vectorised loops, likely the oscillator/unison kernels), `fxReverb`
  (core.c line ~44549).

## Next steps

1. Name the GMSynth vtable slots against `CSynthesizer`/`CPluginWrapper`
   (the wrapper's `Gen_Render`, `TriggerVoice` and parameter calls lead in).
2. Read the WAV-to-table code in `FUN_1800f1570` and the oscillator read in
   the vectorised loops; pitch from parameters 2, 44, 55..60.
3. Amp ADSR, filter (types 0/1 at least, the corpus uses no others), the
   two envelopes and LFOs with their destinations.
4. The effects the corpus switches on (of 10 states): Distortion (147) in
   five, Reverb (154) in five, Crusher (148) in three, Hi Pass (152) and
   Delay (153) in two, Phaser (150) in one; Flanger, Low Pass, Panner and
   Trancer in none. A first port could run the voices and these effects and
   say the rest in `fidelity()`.
