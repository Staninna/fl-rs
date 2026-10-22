# Fruity Envelope Controller: eight articulators a note runs

Fruity Envelope Controller is a generator that makes no sound. It has
eight articulators, each an envelope, an LFO and five maps (key,
velocity, Mod X, Mod Y and random) like Sytrus's; a note on its channel
starts the articulators whose key range holds its key, and once a tick
each one's value, kept within 0 to 1, is a controller output a project
links to other controls. In fl-rs it is `EnvelopeController`
(`crates/plugins/src/envelope_controller.rs`), a `Generator` whose eight
`controller_outputs` the engine carries along the 227 links whose source
is the channel's control `0x8000 + n`. The envelopes and LFOs run on
`crates/plugins/src/menvelope.rs`, FL's shared `TMEnvelope` and `TMLFO`.

Read from `Fruity Envelope Controller_x64.dll` (FL Studio 26.1.7, Delphi,
image base 0x400000) with headless Ghidra and capstone. Tags: `asm` read
from the binary, `dfm` from the editor's form, `corpus` from the shipped
projects, `inferred` where the binary does not say.

## The plugin `asm`

`CreatePlugInstance` @ `0x8acda0` constructs `TTestPlug` (`0x8ada00`),
VMT `0x8acc68` (class reference `0x8acba0`, instance 0x330 bytes, parent
`TDelphiFruityPlug`). `TFruityPlugInfo` @ `0x91b090`: "Fruity Envelope
Controller" / "Env Ctrl", Flags `0x10600060` = `FPF_WantNewTick |
FPF_NoProcess | FPF_NewVoiceParams | FPF_Reserved1` and bit 28, 90
parameters, 8 output controllers.

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x8ace10 |
| 3 | SaveRestoreState | 0x8ad310 |
| 5 | ProcessEvent | 0x8ae6e0 |
| 6 | ProcessParam | 0x8ae740 |
| 7, 8, 13 | Eff_Render, Gen_Render, Voice_Render | 0x40c5a0, the RTL's abstract-method stub |
| 9 | TriggerVoice | 0x8af660 |
| 10 | Voice_Release | 0x8afca0 |
| 11 | Voice_Kill | 0x8afd50 |
| 14 | NewTick | 0x8afec0 |

The eight articulators are `TECArticulator`s (VMT `0x8acac0`, parent
`TArticulator`, 0xf0 bytes), kept at `0x2a4 + 8 a`: `+8` the combining
mode, `+0x10` the envelope, `+0x18` the LFO, `+0x30..+0x50` the key,
velocity, Mod X, Mod Y and random maps (`TMMap`, VMT `0x7d1228`),
`+0x20/+0x24/+0x28` the base level, envelope amount and LFO amount,
`+0x58/+0x5c` the key range.

## State `asm` `corpus`

`SaveRestoreState` reads, in order:

| bytes | what |
|---|---|
| 4 | version: `0x60003` from FL 26, whose high word (6) is the version when it is over `0xffff` |
| 360, 176 before version 6 | the parameters, i32, at `0x130` (90, or 44 for four articulators) |
| 4 | the articulator the editor shows, `0x298` |
| 4, from version 1 | the part the editor shows, `0x29c` |
| 4 | option bits: 1 and 2 two editor buttons, 4 the outputs run without a note (`0x2a1`) |
| 4 | an editor wheel |
| per articulator, 8 (4 before version 6) | |
| 4, from version 2 | its flags: bit 0 is the "bipolar sum" mode (`+8`, `0x7e0170`), which also puts the envelope's and the maps' centre at 0.5 (`0x7d1600`) |
| stream | `0x7dfaf0`: a version, the three levels (three singles), the envelope (`TMEnvelope`, kind 1) and the LFO (`TMLFO`, kind 2), then the maps from the key map on (`0x8b1100`): five, or two before version 6, the rest set to their defaults |
| 8 | the key range, two i32 |

The parameters are then set again through `ProcessParam`, so they and not
the stream's levels and knobs count. The corpus has four states: three of
version 6 (Future House, Madonna, FlashyWaves, 5996 to 6164 bytes) and
Steptime's of version 5 (2116 bytes).

## Parameters `asm` `dfm`

Parameter `11 a + k` is knob `k` of articulator `a` (`0x5d4940` divides by
11); `ProcessParam` (`0x8ae740`):

| k | wheel | range | sets |
|---|---|---|---|
| 0 | `BaseLevelWheel` | 0 to 256 | `+0x20 = v / 256` |
| 1 | `EnvLevelWheel` | -256 to 256 | `+0x24 = v / 256` |
| 2 | `LFOLevelWheel` | -256 to 256 | `+0x28 = v / 256` |
| 3 to 6 | `EnvAtt`, `EnvDec`, `EnvSus`, `EnvRel` | Sytrus's, `inferred` | the envelope's knobs `+0x98` |
| 7 to 10 | `LFOSpeed`, `LFOTension`, `LFOSkew`, `LFOPW` | Sytrus's, `inferred` | the LFO's knobs `+0xd0` |

88 and 89 are `ModXWheel` and `ModYWheel`, 0 to 65536 (32768), stored as
`v / 65536` at `0x2ec` and `0x2f0`. The envelope and LFO wheels' ranges
are not in the form; they are taken to be the ones Sytrus's articulators
have.

## The parts a tick runs `asm`

`0x8b11c0`, when a stream is read:

- `+0xa0` is the mode's identity, 0 for a sum and 1 for a product.
- `+0x60` lists, in order, the envelope (0) when it is active, the LFO
  (1) when it is, and the key, Mod X and Mod Y maps (2, 4, 5) that are not
  at their default (`TMMap` method `+0x80`), ended by -1.
- `+0xa4` starts at the identity and takes in every Mod X or Mod Y map
  that is flat (`0x7df730`: all its points at one level), at that level
  less its centre; `+0x80` lists the ones that are not flat.
- `+0xdd` is whether the random map is at its default.

A map at its default is taken to be the flat line at the identity plus
the centre (`inferred`; method `+0x80` was not read), which in a tick is
the same as leaving it out.

## Notes `asm`

`TriggerVoice` reads the voice's `FinalLevels` (`TVoiceParams + 0x14`):

- key: `max(0, (Pitch + 6000) * 0.01)`, rounded, against each
  articulator's key range;
- velocity: `Ln(min(Vol * 10, 20.551182) + 1) / Ln(21.551182) * (1/128) *
  127`.

Every articulator whose range holds the key is first taken from any note
that held it (`0x8afb60`), so one note at a time holds an articulator.
Its envelope and LFO start (`0x7dfdf0`), and the note keeps `+0xd4`: the
velocity map at the velocity, and unless the random map is at its default
combined with it at FL's next random number times 2^-32 (`0x8ab9e0`).

`Voice_Release` releases the envelope and the LFO of every articulator
the note holds (`0x7dab60`). `Voice_Kill` takes the note away.

## NewTick `asm`

For every note, every articulator it holds:

```
v = base
for part in +0x60:
    envelope: advance a tick; v += out * envelope amount
    LFO:      advance a tick; v = sum ? v + out * LFO amount : v * (out * LFO amount * 2 + 1)
    map n:    v = v ⊕ map(n, x)           x: key (Pitch + 6000) / 12000,
                                           Mod X FCut * 0.5 + wheel 88, Mod Y FRes * 0.5 + wheel 89
v = v ⊕ +0xa0 ⊕ +0xd4                      // ⊕ is + for a sum and * for a product
+0xa8 = v
```

An articulator whose envelope and LFO are both past their last stage (9)
is let go (`0x8afa80`); a note holding none asks the host to kill it.

Then for each articulator: `r = +0xa4 ⊕ +0xa8` when a note held it this
tick; with none and option 4 on, `r = +0xa4 ⊕ base` and every map of
`+0x80` at its wheel; with none and option 4 off nothing is sent. `r` is
clamped to 0..1 by its bits as an integer and, when it is not what was
sent last (`+0xe0`), sent as `Round(r * 2^30)`.

The envelopes count `x` in half seconds or beats: `0x8afde0` gives them
`(single)(1 / PPQ) * 120 / tempo` and `1 / PPQ` a tick, with the tempo
from `FPE_Tempo` (`0x300`) and the PPQ from `FPD_SetTimeSig` (`0x2fc`).

## Flush `asm`

`FPD_Flush` (2) resets each LFO (`0x7de5b0`) and sets each `+0xe0` to -1,
so the next value is sent whatever it is. `Generator::reset` does that,
drops the notes, and a loaded state starts so.

## Corpus `corpus`

- Asher Postman - Future House: "Basic Sidechain", articulators 0 to 3 as
  products of a low base and an envelope, outputs 0 to 2 to parameter 7
  of five effect slots. All 5 links play.
- LollieVox & Lezina J - I'll Be Your Madonna: "Harmony Notes", one sum
  with an envelope and a key map, output 0 to a slot's parameter 0. Plays.
- Sacco - FlashyWaves: output 0 to a ZGameEditor Visualizer parameter,
  which the engine does not play.
- nucleon - Steptime: version 5, no link.

Every corpus articulator runs its envelope only, no LFO, so nothing the
gaps below name is used.

## What is not FL's

- An LFO with flag 2 shares one run between notes in FL (`0x8b0c80`
  lists them and `NewTick` moves their phase once); here each note runs
  its own. `fidelity()` says so.
- The random map draws from a Park-Miller step, not FL's generator.
- An envelope's sustain knob, as `menvelope.rs` leaves it out.
- FL's ordering of `TriggerVoice` and `NewTick` within a tick is taken
  to be the engine's: the notes due first, so a note's first tick moves its
  envelope on by a whole tick.
- The key range is matched against the host's key, after the channel's
  root note and pitch.
