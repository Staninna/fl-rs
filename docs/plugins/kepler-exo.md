# Kepler Exo: state and engine

Recovered from `Kepler Exo_x64.dll` (Delphi shell) and its
`engine_x64.dll` (MSVC C++, image base 0x180000000). Corpus: no project
uses Kepler Exo; 188 presets under `Patches/Plugin presets/Generators/Kepler
Exo` (143 at version 1, 45 at version 2) and the plugin databases' default.

Confidence tags: `dec` (decompile), `corpus`, `inferred`.

## One engine for Kepler and Exo

Kepler's and Exo's `engine_x64.dll` have the same `.data` section byte for
byte (the noise, LFO 1's wave and the 11-point tables at the addresses
`kepler.md` gives) and `.text` sections 0xc5e46 and 0xc5ed6 bytes long,
with the same functions at the same addresses through the voice code
(`FUN_180013260` the ladder, `FUN_180014f80` the DCO, `FUN_180015130` the
step table, `FUN_180012940` the arpeggiator...; later ones shift by up to
0x90). Both load a "Kepler-1" state and a "Kepler-2" state
(`FUN_18001efd0`). So the engine `kepler.md` maps is Exo's, and the Rust
port plays an Exo state through Kepler's port (`kepler.rs`). `dec`

## 1. State (event 213)

```
u32 1 | chunks (i32 id | i64 size | bytes)    FL's wrapper; chunk 1 is the plugin's, 2 the wrapper's
chunk 1:
  u32 0 | u32 len - 12 | u32 0                Kepler's shell
  i32 9 | "Kepler-2\0" | i32 version (FL 2026 writes 4)     FUN_18001ed60 saves, FUN_18001efd0 loads
  version > 0:
    i32 count (151 in the presets, 0x9b written) | count x f32   engine parameters 0.. in order (< 0x9b kept)
    i32 1 | i32 slots (8) | slots x (i32 source, i32 destination)   the matrix, -1 empty
    version > 2: u8 (this+0x1c878) | u8 oversampling 4 (else 2)
    version > 3: u8 (this+0x1cd90): the voices take another set of oscillator values
version < 2: LFO 1's rate (0x30) when synced (0x31) and LFO 2's (0x36) when synced (0x37)
             are read anew as for Kepler-1 (kepler.md, section 1)
```

Before loading, every parameter goes to the engine's default
(`FUN_18001ec40` -> `FUN_18001d790`, the list in `kepler.md`). A
"Kepler-1" state reads as Kepler's (`kepler.md`). `dec`, `corpus` (all 188
read to their last byte).

## 2. The parameters (`FUN_18001fa90`, the names)

```
00 LFO1->DCO1 Amount   01 LFO1->DCO1 Range   02 LFO2->DCO1 Amount   03 LFO2->DCO1 Range
04 Env2->DCO1 Amount   05 DCO1 PW Amount     06 DCO1 PW Mod Source  07 DCO1 Pulse Active
08 DCO1 Saw Active     09 DCO1 Sub Active    0a DCO1 Sub Volume     0b DCO1 Sub Phase
0c DCO2 Active         0d DCO2 Semitones     0e DCO2 Detune         0f DCO2 Saw Active
10 DCO2 Sub Active     11 DCO2 Sub Volume    12 DCO2 Sub Phase      13 DCO2 PW Amount
14 LFO1->DCO2 Amount   15 LFO1->DCO2 Range   16 LFO2->DCO2 Amount   17 LFO2->DCO2 Range
18 Env2->DCO2          19 DCO2 PW Mod Source 1a DCO2 Pulse Active   1b DCO2 Hard Sync
1c DCO1->DCO2 XMod     1d DCO2->DCO1 XMod    1e/1f their shapes     20 DCO1 Level
21 DCO2 Level          22 Noise Volume       23 DCO1/2 AM           24 HP Freq
25 Cutoff              26 Resonance          27 Key Follow          28 Filter Slope
29 Filter FM           2a Filter FM Shape    2b Env1->Cutoff Pol.   2c Env1->Cutoff
2d LFO1->Cutoff        2e LFO2->Cutoff       2f VCA Source          30..35 LFO1 Rate, Sync, Delay, Trig, Retrig on, phase
36..3c LFO2 Rate, Sync, Delay, Trig, Shape, Retrig on, phase      3d..40 Env1 ADSR
41..44 Env2 ADSR       45 Chorus Mode        46 Chorus Noise        47 Chorus Noise Gate
48 Main Volume         49 Velocity->Volume   4a Pan                 4b Mono
4c Legato              4d Always Trigger     4e Restart on Off      4f Porta Mode
50 Portamento          51 LFOs Trigger       52 Hold                53 Octave
54 Pitch Bend Range    55..5a Arpeggiator Active, Rate, Sync, Range, Mode, Gate
5b Juno Arp Bug        5c..63 Matrix Amount 1..8                    64..6d EQ Active, Q, Gain, Freq x3
6e..78 Delay Active, Sync, Amount, Time, Right Tap, Feedback, Fb Cutoff, Fb Type, Stereo, Blur, Tone
79..85 Reverb Active, Low Cut, High Cut, Predelay, Pd Feedback, Room, Diffusion, Decay, Damp, Width, Dry, Early, Wet
86..88 Saturation Active, Amount, Pre/Post  89..94 HyperChorus Active, Delay, Feedback, Rates, Lines, LP, HP, Mix, Phase
95 X Offset  96 Y Offset  97 VCA Smooth  98 Saturator HQ  99 Master Tune  9a Highpass Active
9c Filter Type  100 DCO1 Pitch  101 DCO2 Pitch
```

Matrix sources (`FUN_180017b20`): 0 LFO1 triangle, 1 LFO2, 2 Env1, 3
Env2, 4/5 velocity on/off, 6/7 mod X/Y, 8/9 gate, 10 LFO1 sine, 11 LFO1
unipolar, 12 LFO2 unipolar, 13 key follow, 14..18 the LFOs "extra", 19
alternate, 20 key follow unipolar, 21 random, 22/23 mod X/Y unipolar.

## 3. Where the rest of the engine is

- Voice per frame `FUN_180023b40`: both envelopes (`+0x2dd0` and
  `+0x2df8`, attack/decay/release as `kepler.md`), the control-rate update
  `FUN_180016180` every `+0x2e5c` frames, DCO 1 pitch (LFO 1, LFO 2, env 2,
  matrix via `FUN_180023a70`), DCO 2 when on (`+0x2010`): its own pitch
  sum with `+0x2e40` (semitones and detune) and the cross terms, both
  clamped at 0.25 of the oversampled rate; the cutoff (key follow, env 1,
  LFOs, matrix) written to the filter lanes. `dec`
- Frame `FUN_18001b7d0`: parameter smoothing, LFO 1, then three filter
  designs by type `+0x13ac` (0 the ladder of `kepler.md`, 1 and 2 other
  four-lane forms), the oversampled voice loop, halfbands, HPF, chorus,
  volume. `dec`
- Effects not yet identified among `FUN_180004320` (9726 bytes),
  `FUN_1800078c0`, `FUN_18000ba00`, `FUN_18000c6e0`, `FUN_18000d630`
  (EQ, saturation, delay, reverb, hyper chorus). `inferred`

## What the Rust port does

`crates/plugins/src/kepler_exo.rs`: section 1 in full and both state
kinds, into `kepler.rs`'s engine. What that engine plays is Kepler's part
(`kepler.md`): DCO 1, noise, LFO 1, envelope 1, the type 0 ladder, the
highpass, the chorus, the arpeggiator, main volume and pan. Left out, and
named by fidelity when a state turns it on: DCO 2 with hard sync, cross
modulation and AM; LFO 2 and envelope 2; the filter's FM, its other
slopes and types; the matrix; mono, legato and portamento; hold; the
chorus noise; EQ, saturation, delay, reverb, hyper chorus; master tune
and the X/Y and DCO pitch offsets. Two of the 188 presets use none of it.
`.scratch/notes/kepler-exo.md` is the handover.
