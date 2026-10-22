# BooBass: state, parameters, voice model

Recovered from `BooBass_x64.dll` (FL Studio 2026, Delphi shell, image base
0x400000) and `BooBass_Eng_x64.dll` (MSVC C++, image base 0x180000000, export
`CreatePlugInstance`, class `BooBassPlug` on a `TCPPFruityPlug` base). The
shell owns the state and the three knobs and forwards every voice call to the
engine, which is the whole instrument. Ghidra project `ghidra/proj`;
decompiles under `ghidra/out/boobass/vmt/` (shell slots) and
`ghidra/out/boobasseng/` (`vmt/` CreatePlugInstance and the table builder,
`slots/` the SDK slots, `render/` the per-sample oscillator and note setup,
`asm/` listings for the constants). One run on the shell, three on the engine.
Corpus: 3 uses in 3 `.flp`, 3 shipped presets in one `.fst`, all 16 bytes.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `inferred`, `asm`.

## How the plugin is wired

Shell: `CreatePlugInstance` @ `0x006b8090` -> constructor `FUN_006b85a0`;
class reference `*0x006b7ea8` -> VMT `0x006b7f70` (`TTestPlug`, 0x140 bytes).
`TFruityPlugInfo` @ `0x00706f5c`: SDK 1, "BooBass", Flags `0x10600411`
(Generator, GetNoteInput, bit 10, NewVoiceParams, bit 22, bit 28), NumParams 3.

| slot | SDK name | shell | engine (`BooBassPlug::vftable` @ 0x180063630) |
|---|---|---|---|
| 1 | Dispatcher | 0x006b80e0 | 0x180002270 (sample rate -> `FUN_180003d90`, HQ flag -> `FUN_180003c60`) |
| 3 | SaveRestoreState | 0x006b84a0 | 0x180001240 (stub) |
| 4 | GetName | 0x006b8af0 | |
| 6 | ProcessParam | 0x006b88c0 | 0x1800023a0 |
| 8 | Gen_Render | 0x006b8cb0 -> engine | 0x180002420 |
| 9 | TriggerVoice | 0x006b8c20 -> engine | 0x180002790 |
| 10 | Voice_Release | 0x006b8c40 -> engine | 0x1800028f0 |
| 11 | Voice_Kill | 0x006b8c60 -> engine | 0x180002920 |
| 12 | Voice_ProcessEvent | 0x006b8c80 -> engine | 0x1800012d0 (stub) |
| 13 | Voice_Render | abstract | 0x1800012e0 (stub) |

Shell fields: `0x120` the engine object (`FUN_006b7a50(module, tag, host)`
calls the engine's `CreatePlugInstance`), `0x128` i32[3] parameters.

Engine object (0x3d0 bytes, `CreatePlugInstance` @ `0x1800029a0`):

| offset | meaning |
|---|---|
| 0x8 | host tag; 0xe0 host |
| 0x220 | the `WaveTable` voice (below) |
| 0x358 | smoothed pan; 0x35c / 0x360 last L / R gains for the ramp |
| 0x378..0x384 | four one-pole states; 0x388..0x394 their inputs; 0x39c b = -0.404, 0x3a0 a = 0.298 |
| 0x3a8 | current voice {tag, params}; 0x3b0 active; 0x3b8 mono buffer (0x3c0 capacity); 0x3c8 samples rendered |

`WaveTable` (`engine+0x220`): `+8` a 4-segment linear envelope, `+0xa0` level
in (0..1), `+0xa8` loop table, `+0xb0` attack sample, `+0xb8` loop increment,
`+0xbc` loop phase (0..16384, float), `+0xc0` bass drive, `+0xc4` attack
length - 1, `+0xc8` attack position, `+0xcc` attack increment, `+0xd0` =
16000/SR, `+0xd4` = 267955.84/SR, `+0xe8` zone, `+0xec` / `+0xf0` / `+0xf4`
bass / mid / treble gains, `+0xf8..+0x100` their smoothed copies, `+0x104` =
1 - mid gain, `+0x108..+0x110` the raw knob values, `+0x114..+0x120` four
lowpass states, `+0x124` HQ flag, `+0x130` output integrator.

## 1. State layout (event 213: 16 bytes)

`SaveRestoreState` (`FUN_006b84a0`):

```
save:    Write(i32 1); Write(this+0x128, 12)
restore: Read(i32 v); if v != 0x7fffffff and v < 2: Read(this+0x128, 12); GetValue/UpdateValue loop over 3 params
```

| byte | type | idx | parameter (DFM object) | range | default | corpus / presets | confidence |
|---|---|---|---|---|---|---|---|
| 0..3 | i32 | | version 1 | | 1 | 1 x6 | loader, corpus, preset |
| 4..7 | i32 | 0 | Bass (`BassWheel`, no Tag) | 0..65535 | 32767 | 32767 x5, 65535 x1 | loader, dfm, corpus, preset |
| 8..11 | i32 | 1 | Mid (`MidWheel`, Tag 1) | 0..65535 | 32767 | 32767 x5, 0 x1 | loader, dfm, corpus, preset |
| 12..15 | i32 | 2 | Treble (`TrebleWheel`, Tag 2) | 0..65535 | 32767 | 32767 x5, 0 x1 | loader, dfm, corpus, preset |

Preset check: the three blocks in `BooBass.fst` and two corpus uses are the
default `[1, 32767, 32767, 32767]` (bass -6 dB, mid -5 dB, treble -3 dB by
section 2, i.e. flat relative to each other). "5.1 bass" in `Astes - Bien
Duro.flp` is `[1, 65535, 0, 0]`: bass 0 dB, mid and treble under the 0.1
threshold and therefore muted, leaving only the lowpassed band plus its drive.

## 2. Parameter semantics

Shell `ProcessParam` (`FUN_006b88c0`): REC_FromMIDI rescales with the wheel's
range, REC_UpdateValue stores and forwards `(idx, v, flags & ~0x20)` to the
engine. Engine `ProcessParam` (`FUN_1800023a0`): `x = v / 65535`, then with
`SR0 = 44100` and `dB(y) = 10^(y / 20)` (`FUN_1800393d0`, `asm` constants
`0x1800637c4` 12, `0x1800637c0` 9, `0x180063520` 10, `0x1800637d0` 24,
`0x180063754` 0.1):

```
bass   (FUN_1800040e0): gain  = x < 0.1 ? 0 : dB(12 x - 12)       // -12 dB .. 0 dB, muted under 0.1
                        drive = x < 0.1 ? 0 : dB(24 x - 24)       // +0xc0, feeds the saturator in section 3
mid    (FUN_1800041c0): gain  = x < 0.1 ? 0 : dB(10 x - 10);  +0x104 = 1 - gain
treble (FUN_180004240): gain  = x < 0.1 ? 0 : dB(12 x - 9)        // -9 dB .. +3 dB
```

Gains move towards their targets over the block in 16-sample steps in the
normal path and per sample in the HQ path (`FUN_1800049f0`).

Fixed tables (`FUN_1800034f0`, built at the first instance):

```
zone roots (cents)  0x1800cb9e0: 1100 1600 2300 2800 3500 4000      // B0 E1 B1 E2 B2 E3 at 16.35 Hz * 2^(c/1200)
zone trims (dB)     0x1800cb9f8: 0.666 -0.333 -0.75 -1.5 -2.0 -3.0  // applied as dB(-trim) to the loop table
six attack samples  pairs at 0x1800cb920 (start, end) of 16-bit PCM in the DLL; six loop waveforms resampled
                    with 4-point Hermite to 16384 points (+4 guard samples each side) at *0x1800ccc98 + zone * 0x10020
increments          0x1800ccd10[c], c = 0..29999: SR / (2^(c/1200) * 4.0887)   // the pitch-dependent decay length
velocity table      0x1800ea1d0[48]: dB(i - 48) for i < 48, [48] = 1            // 48 dB range, linear in dB
lowpass coefficients  +0xd0 = 16000 / SR,  +0xd4 = 267955.84 / SR;  globals 0x1800ccd04 = 0.025 * 44100/SR, 0x1800ccd08 = 0.155 * 44100/SR
```

## 3. Voice model

BooBass is monophonic. `TriggerVoice(params, tag)` (`FUN_180002790`): lock
(host slot 32); if a voice is active, free it and `Host.Voice_Kill(oldTag, 0)`;
`+0x3a8 = {tag, params}`, `+0x358 = params.FinalLevels.Pan`; `Host.ComputeLRVol`
(slot 31) for the initial L/R; note setup `FUN_180004e60(wt, (int)(FinalLevels.Pitch + 2400))`;
unlock (slot 33). The record is the voice handle.

Note setup (`FUN_180003db0`, `FUN_180004e60`), `c = min(cents, 9600)`:

```
zone  = nearest root in 0x1800cb9e0 (search down from 5, then pick the closer neighbour)
loopInc   = 2^(c / 1200) * 267955.84 / SR          // table samples per sample; 16384 per cycle -> f = 16.35 Hz * 2^(c/1200)
attackInc = 2^((c - root) / 1200) * 16000 / SR     // the attack samples were recorded at 16 kHz
attackLen = len[zone] - 1; attackPos = 0; loop phase = 0
envelope (FUN_180002c80): segments {0, 0x1800ccd10[c], (len[zone] << 4) * SR/44100, SR / 55} samples with
                          levels 1.0 -> 0.9375 -> 0 (0x1800cba40); FUN_180002d80 restarts it from 0
```

So `FinalLevels.Pitch = 0` plays 65.4 Hz (C2), three octaves under 3x Osc's
reference, and the lowest zone root sits at 30.9 Hz.

Per block (`Gen_Render`, `FUN_180002420`), when a voice is active:

```
FUN_180003f90(wt, (int)(FinalLevels.Pitch + 2400))      // pitch slides: recompute loopInc and attackInc, zone fixed
pan = FinalLevels.Pan * 0.5 + pan * 0.5                  // 0x358, one-pole on the pan
render len samples into the mono buffer (per sample below)
four cascaded one-poles: y = 0.298 (x + x1) + 0.404 y1   // 0x378.., a fixed 4th-order smoothing lowpass
(L, R) = Host.ComputeLRVol(pan, FinalLevels.Vol)         // slot 31
Host.slot37(tag, 0, &{.., 0}) ... Host.slot37(tag, 0, &{.., 1})   // lock / unlock the destination
Host.AddWave_32FM_32FS_Ramp(mono, dest, len, L, R, &lastL, &lastR)   // slot 11, FUN_0067faf0
if envelope stage == 4: free the voice, Host.Voice_Kill(tag, 0)
```

Per sample (`FUN_1800042f0`; the HQ variant `FUN_180004640` box-filters the
loop read over `loopInc` samples and ramps the EQ gains per sample):

```
s = lerp(loop[phase], loop[phase + 1]);  phase = (phase + loopInc) mod 16384
if attackPos < attackLen:
    a = lerp(attack[attackPos]);  attackPos += attackInc
    s = a + (s - a) * attackPos / attackLen                  // crossfade from the plucked attack into the loop
v = clamp(+0xa0, 0, 1);  s *= lerp(velTable[v * 47])        // 48-step dB curve: 0 -> -48 dB, 1 -> 0 dB
step the envelope (FUN_180005460)                            // its output is +0xa0 (inferred: nothing else writes it)
lp1 += (s - lp1) * 0.025 r;  lp2 += (s - lp2) * 0.155 r      // r = 44100 / SR
y  = lp1 * bass + (lp2 - lp1) * mid + (s - lp2) * treble     // three bands split by the two one-poles
lp3 += (y - lp3) * 0.025 r;  lp4 += (y - lp4) * 0.155 r
if |lp3| > 1: lp3 = clamp(lp3, -1, 1)                        // hard clip of the bass band, state reset
b  = sat(lp3) * drive + lp3 * (1 - drive)                    // sat = FUN_18003aa70, a CRT function on lp3 (inferred tanh)
o  = (b * bass + (lp4 - lp3) * mid + (y - lp4) * treble) * 3 - out
out += clamp(o, -slew, slew),  slew = (loopInc + 0.0328) * 44100 / SR    // 0x130: a slew-rate limited integrator
```

The EQ is the same three-band split twice: once to apply the knob gains, once
more on the result to drive the bass band through the saturator. The slew
limit scales with the pitch, so high notes are allowed faster output changes.
The final `* 3` and the `0.5` pan smoothing are read from the listing (`asm`).

`Voice_Release` (`FUN_1800028f0`) -> envelope release (`FUN_180002e10`);
`Voice_Kill` additionally frees the record. `Voice_ProcessEvent` and
`Voice_Render` are stubs; there is no `NewTick`.

## 4. What stays unknown

- The envelope's output path: `FUN_180005460` steps the segment object at
  `wt+8`; `+0xa0` is tagged `inferred` as its output.
- `FUN_18003aa70` (the saturator) and `FUN_1800393d0` (`dB`) are CRT
  transcendentals identified by use, not decompiled.
- Host slot 37 (`FUN_0132bb50`): it locks a mixer track entry and returns a
  buffer; the SDK name was not matched.
- Which note the host maps to `FinalLevels.Pitch = 0`: taken to be key
  60, below.

## Pitch reference

`FinalLevels.Pitch = 0` is MIDI 60 (FL C5); see `docs/plugins/host-voice.md` for the host side of the voice path.

## Corrections from the implementation (crates/plugins/src/boobass.rs)

- The per-block `FUN_180003f90` is run when the host moves the note's
  pitch (`Generator::set_note_pitch`, for a slide or a clip on the
  channel's pitch), which gives the same steps from the next block on: the
  pitch does not move between.
- Samples are raw 16-bit PCM in `.data` of `BooBass_Eng_x64.dll`, behind pointer tables at RVA 0xcb920 (attacks) and 0xcb980 (loops); each second pointer is a byte count, not an end.
- The saturator is `sin(x·π/2)`, not tanh. The slew limit is `(attack progress + 0.0328)·44100/SR`, not pitch-dependent.
- The level table uses `v·48` with entry 0 = 0. The doc's trims belong to the attacks; loops have their own (4, 3.5, 3, 3.25, 3.5, 3 dB), negated. `+0x104` is `1.5 − mid`.
- The default EQ is not unity: −6/−5/−3 dB per pass, −1.28/+0.28/+3.23 dB overall.
- Not FL's: the HQ path's box filter (`FUN_180004640`) is not built; a
  note's pan is `dsp::pan_linear` until the host's `ComputeLRVol` is read;
  a note ends on the sample its envelope ends, where FL ends it with the
  block; under key 36 the decay table's formula goes on down where FL
  reads before the table.
