# 3x Osc: state, parameters, voice model

Recovered from `3x Osc_x64.dll` (FL Studio 2026, Delphi shell, image base
0x400000) and its `engine_x64.dll` (MSVC, image base 0x180000000, one export
`dispatch`). The engine only renders anti-aliased oscillators for the "HQ"
checkbox; everything else, including the envelopes, LFOs and filter of the
"instrument settings" tab, is compiled into the shell. Ghidra project
`ghidra/proj`; decompiles under `ghidra/out/osc3/` (`vmt/` the SDK slots with
callees, `voice/` the articulation unit, `artic/` its envelope stepping,
filters and pan law, `kernels/` the CPU-dispatched oscillator, filter, pan and
mix kernels with `asm/` listings) and `ghidra/out/osc3eng/` (`vmt/` dispatch
and the render kernel, `shapes/` the shape functions and mip builder, `asm/`
listings). Corpus: 152 uses in 75 of the 165 `.flp` (93 blocks of 92 bytes, 2
of 93, 57 of 460), 181 shipped presets in 178 `.fst` (1 of 44 bytes, 82 of 84,
70 of 88, 28 of 92). Four Ghidra runs on the shell after the import, two on the
engine, one on FLEngine for the host services (shared with the other three
generators, `ghidra/out/fruityhost/`). The host side of the voice record (pan
law, note to cents, velocity) is in `host-voice.md`.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `inferred`, `asm`.

## How the plugin is wired

`CreatePlugInstance` @ `0x00737f40` -> constructor `FUN_007389f0`; class
reference `*0x00737d58` -> VMT `0x00737e20` (`TTestPlug`, instance size 0x240,
parent `TDelphiFruityPlug` VMT `0x00722348`). Generator slots in FL SDK order:

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x00737f90 |
| 2 | Idle_Public | 0x00725e10 |
| 3 | SaveRestoreState | 0x00738580 |
| 4 | GetName | 0x00739640 |
| 5 | ProcessEvent | 0x00738e50 |
| 6 | ProcessParam | 0x00739060 |
| 7 | Eff_Render | abstract |
| 8 | Gen_Render | 0x0073ab70 |
| 9 | TriggerVoice | 0x00739a40 |
| 10 | Voice_Release | 0x00739c30 |
| 11 | Voice_Kill | 0x00739c50 |
| 12 | Voice_ProcessEvent | 0x00739c90 (returns 0) |
| 13 | Voice_Render | 0x0073a230 |
| 14 | NewTick | 0x0073ac60 |

`TFruityPlugInfo` @ `0x00791978` (packed): SDK version 1, "3x Osc" / "3xOsc",
Flags `0x11600039` (bits 0 Generator, 3 GetChanCustomShape, 4 GetNoteInput, 5
WantNewTick, 21 NewVoiceParams, 22, 24, 28), NumParams 114 (21 own, two unused, 91 of the
instrument tab), DefPoly 0. `FPF_UseSampler` and `FPF_RenderVoice` are not set:
the host calls `Gen_Render` once per block and the plugin renders and
articulates its own voices.

Object fields the audio path touches:

| offset | meaning |
|---|---|
| 0x10 | Info pointer |
| 0xbc | host (`TFruityPlugHost`): `+0x18` WaveTables[0..9] (16384 floats each), `+0x68` TempBuffers[0..3] |
| 0xcc | host sample-rate record, `+8` i32 sample rate |
| 0xe0 | max polyphony (0 = unlimited) |
| 0xe4 | f32 base increment = `2247346493527.166 / SR` (double at `0x00738540`) |
| 0x120 | i32[21] parameters |
| 0x174..0x177 | u8 invert osc 1, invert osc 2, invert osc 3, AM |
| 0x178 | custom shape table (FPD_ChanSampleChanged) |
| 0x180 | 3 x 0x24 derived oscillator records (below) |
| 0x1ec | voice list |
| 0x1f4 | AM oscillator index: -1, or 2 when AM is on (table `0x00791a18`) |
| 0x1f8 | phase randomness range = p20 << 18 |
| 0x1fc | "needs a stereo render" flag |
| 0x1fd | HQ *render mode* (FPD_ProcessMode value bit 1), not the checkbox |
| 0x200 / 0x214 / 0x20c | engine module, its `dispatch`, the engine object |
| 0x208 | HQ checkbox (`HqModeBtn`) |
| 0x21c | voice manager of the articulation unit (holds `Voice_Render` at `+0x30`, this at `+0x38`) |
| 0x224 | articulation settings, 0x338 bytes (five 0xa0 blocks at `+8`, filter at `+0x328`) |
| 0x234 | an articulation block was loaded from the state |

Oscillator record `this+0x180 + i*0x24` (`ProcessParam`, section 2):

| offset | meaning |
|---|---|
| +0 | i32 pitch offset in cents = coarse * 100 + fine |
| +4 | f32 level; the invert flag flips its sign bit |
| +8 / +0xc | f32 left / right gain from `Host.ComputeLRVol(pan / 64, level)` |
| +0x10 | i32 stereo phase offset = p * 0x1ffffff (p / 128 of a cycle) |
| +0x14 | i32 stereo detune, cents |
| +0x18 | wavetable pointer |
| +0x20 | u8 noise shape |
| +0x21 | u8 shape wants the box-filter anti-aliasing (not sine, not noise) |

## 1. State layout (event 213: 460 bytes; 93, 92, 88, 84 and 44 in older files)

`SaveRestoreState` (`FUN_00738580`), parameters at `this+0x120`:

```
save:    Write(i32 14); Write(buf, 0x1c8)        // 84 bytes params, 4 flag bytes, 1 HQ byte, 3 pad, 364 articulation
restore: Read(i32 v)
         v < 10:   HQ = 0; zero the 21 params; the int just read is data; Read(40 more bytes)
                   11 ints old layout [s1 c1 f1 | m2 | s2 c2 f2 | m3 | s3 c3 f3] -> params 1..3, 6, 8..10, 13, 15..17
         v 10, 11: Read(this+0x120, 0x50) (20 params), p20 = 0, Read(this+0x174, 4) (flags; v10 files end before it)
         v 12, 13: zero params; Read(buf, 0x59): 84 params, 4 flags, 1 HQ (bit 0 of byte 88)
         v 14:     zero params; Read(buf, 0x1c8, &got); same, then if got - 92 > 363: load the articulation block (FUN_0072c2e0), this+0x234 = 1
         v < 11:   the 4 flag bytes are forced to 0
         push the five flags to their buttons, GetValue/UpdateValue loop over all 114 params
```

The corpus: every project block is version 12, 13 or 14 (FL writes 14 since
the instrument tab moved into the plugin); shipped presets are 10, 11, 12 and
one unversioned (`Default.fst`, 44 bytes).

Version 14 (460 bytes):

| byte | type | idx | parameter (DFM object) | range | default | corpus (57 blocks) | confidence |
|---|---|---|---|---|---|---|---|
| 0..3 | i32 | | version 14 | | | 14 x57 | loader, corpus |
| 4..7 | i32 | 0 | Osc 1 panning (`VectorWheel4`, no Tag) | -64..64 | 0 | 0 x55 | loader, dfm, corpus |
| 8..11 | i32 | 1 | Osc 1 shape (`Osc1Select`) 0 sine 1 tri 2 square 3 saw 4 rounded saw 5 noise 6 custom | 0..6 | 0 | 3 x21, 2 x11, 0 x10, 5 x8 | loader, dfm, corpus |
| 12..15 | i32 | 2 | Osc 1 coarse (`VectorWheel2`), semitones | -24..24 | 0 | -24 x28, 0 x22, -12 x7 | loader, dfm, corpus |
| 16..19 | i32 | 3 | Osc 1 fine (`VectorWheel3`), cents | -100..100 | 0 | 0 x52 | loader, dfm, corpus |
| 20..23 | i32 | 4 | Osc 1 stereo phase offset (`Osc1SPWheel`) | -64..64 | 0 | 0 x54 | loader, dfm, corpus |
| 24..27 | i32 | 5 | Osc 1 stereo detune (`Osc1SDWheel`), cents | -50..50 | 0 | 0 x40, 13 x3, 50 x2 | loader, dfm, corpus |
| 28..31 | i32 | 6 | Osc 2 mix level (`VectorWheel7`) | 0..128 | 64 | 0 x21, 64 x16 | loader, dfm, corpus, preset |
| 32..35 | i32 | 7 | Osc 2 panning | -64..64 | 0 | 0 x56 | loader, dfm, corpus |
| 36..39 | i32 | 8 | Osc 2 shape | 0..6 | 0 | 0 x26, 3 x13, 4 x10 | loader, dfm, corpus |
| 40..43 | i32 | 9 | Osc 2 coarse | -24..24 | -12 | -12 x33, 0 x10, -24 x9 | loader, dfm, corpus, preset |
| 44..47 | i32 | 10 | Osc 2 fine | -100..100 | 0 | 0 x55 | loader, dfm, corpus |
| 48..51 | i32 | 11 | Osc 2 stereo phase offset | -64..64 | 0 | 0 x53 | loader, dfm, corpus |
| 52..55 | i32 | 12 | Osc 2 stereo detune | -50..50 | 0 | 0 x39, 50 x3 | loader, dfm, corpus |
| 56..59 | i32 | 13 | Osc 3 mix level | 0..128 | 32 | 0 x25, 32 x9, 128 x3 | loader, dfm, corpus, preset |
| 60..63 | i32 | 14 | Osc 3 panning | -64..64 | 0 | 0 x56 | loader, dfm, corpus |
| 64..67 | i32 | 15 | Osc 3 shape | 0..6 | 0 | 0 x26, 3 x10, 2 x9, 5 x7 | loader, dfm, corpus |
| 68..71 | i32 | 16 | Osc 3 coarse | -24..24 | -24 | -24 x34, 0 x12, 24 x5 | loader, dfm, corpus, preset |
| 72..75 | i32 | 17 | Osc 3 fine | -100..100 | 0 | 0 x55 | loader, dfm, corpus |
| 76..79 | i32 | 18 | Osc 3 stereo phase offset | -64..64 | 0 | 0 x55, -64 x1 | loader, dfm, corpus |
| 80..83 | i32 | 19 | Osc 3 stereo detune | -50..50 | 0 | 0 x43, 50 x2 | loader, dfm, corpus |
| 84..87 | i32 | 20 | Stereo phase randomness (`RandPhaseWheel`) | 0..64 | 0 | 0 x46, 64 x6 | loader, dfm, corpus |
| 88 | u8 | | invert osc 1 (`InvOsc1Btn`) | 0, 1 | 0 | 0 | loader, corpus |
| 89 | u8 | | invert osc 2 | 0, 1 | 0 | 1 x1 | loader, corpus |
| 90 | u8 | | invert osc 3 | 0, 1 | 0 | 1 x2 | loader, corpus |
| 91 | u8 | | osc 3 = amplitude modulation (`AMBtn`) | 0, 1 | 0 | 1 x4 | loader, corpus |
| 92 | u8 | | HQ, anti-aliased oscillators (`HqModeBtn`), bit 0 | 0, 1 | 0 | 1 x13 | loader, corpus |
| 93..95 | | | padding (0) | | | 0 | loader |
| 96..435 | | | 5 x 68-byte envelope/LFO blocks: panning, volume, mod X (cutoff), mod Y (resonance), pitch | | | | loader, corpus |
| 436..443 | | | unused, written as 0 | | | 0 | loader |
| 444..447 | i32 | | filter cutoff (`FCutWheel`) | 0..256 | 256 | 256, 26, 0 | loader, dfm, corpus |
| 448..451 | i32 | | filter resonance (`FResWheel`) | 0..256 | 0 | 0, 168, 191 | loader, dfm, corpus |
| 452..455 | i32 | | unused | | | 0 | loader |
| 456..459 | i32 | | filter type (`FTypeSelect`) 0 Fast LP, 1 LP, 2 BP, 3 HP, 4 Notch, 5 LPx2, 6 SVF LP, 7 SVF LPx2 | 0..7 | 0 | 0, 2 | loader, inferred |

The invert and AM order follows the form field offsets the loader pushes the
bytes to (`0x798`, `0x7c0`, `0x7f0`, `0x848`, `0x888`), which match the DFM
component order `InvOsc1Btn, InvOsc2Btn, InvOsc3Btn, AMBtn, HqModeBtn`.

A 68-byte envelope/LFO block (`FUN_0072a640` defaults, `FUN_0072a400`
derived values; the layout is the same as the channel's own envelope record in
the `.flp`):

| off | type | field | default |
|---|---|---|---|
| 0 | i32 | flags: bit 0 envelope not tempo-based (scales by tempo/120 when clear, read as-is from `FUN_0072a400`), bit 1 same for the LFO, bit 2 "start at full level when disabled", bit 5 global LFO phase | 0 (volume block gets an extra bit from `0x0072c2d0`) |
| 4 | i32 | enabled | 0 |
| 8 | i32 | predelay | 100 |
| 12 | i32 | attack | 20000 |
| 16 | i32 | hold | 20000 |
| 20 | i32 | decay | 30000 |
| 24 | i32 | sustain, 0..128 | 50 |
| 28 | i32 | release | 20000 |
| 32 | i32 | envelope amount, -128..128 (cents for the pitch block) | 0 |
| 36 | i32 | LFO predelay | 100 |
| 40 | i32 | LFO attack | 20000 |
| 44 | i32 | LFO amount, -128..128 | 0 |
| 48 | i32 | LFO speed | 32950 |
| 52 | i32 | LFO shape 0..2 (table `*0x00792a68`: sine, triangle, square, order `inferred`) | 0 |
| 56 | i32 | attack tension, -128..128 | 0 |
| 60 | i32 | decay tension | 0 |
| 64 | i32 | release tension | 0 (volume block: -101) |

Shorter versions: v13 (93 bytes) stops after the HQ byte; v12 (92) after the
four flag bytes; v11 (88) is `version, 20 params, 4 flag bytes` (p20 is set to
0); v10 (84) is `version, 20 params` (flags forced to 0); the unversioned 44
bytes are the 11 ints `[s1 c1 f1 m2 s2 c2 f2 m3 s3 c3 f3]`. For every version
below 14 the articulation block takes the defaults above (`FUN_0072c250`),
filter cutoff 256, resonance 0, type 0.

Preset check (`.fst` name "3x Osc"; every version-14 corpus block that was
checked by hand decodes with the table above):

| preset | version | osc 1 | osc 2 | osc 3 | other | reading |
|---|---|---|---|---|---|---|
| Default | none, 44 B | sine, 0 | sine, -12 st, mix 64 | sine, -24 st, mix 32 | | the three-octave sine stack; levels 1/4, 1/4, 1/4 by section 2 |
| Accordeon | 10 | custom, +7 ct, det 5, pan -4 | saw +12 st, -3 ct, phase -50, det 5, mix 65 | tri -12 st, +3 ct, phase -15, det -7, pan 6, mix 72 | | custom shape without a channel sample falls back to saw |
| Accordeon 2 | 11 | custom, det -4 | custom -12 st, +13 ct, det 5, pan -8, mix 86 | custom -12 st, phase 3, det -6, pan 6, mix 38 | p20 = 256 | p20 above the DFM max 64: the random range is `256 << 18`, more than a cycle |
| Analog dreamz | 11 | saw -12 st, det -34 | saw -12 st, det 34, mix 64 | saw -24 st, mix 64 | byte 88 = 1 | 88-byte v11 block carries the flag bytes: invert osc 1 set |
| Hoover | 11 | saw, +13 ct | saw +12 st, -13 ct, mix 64 | square -24 st, mix 32 | | |

## 2. Parameter semantics (`ProcessParam` @ 0x00739060)

Indexes 23..113 go to the instrument tab's handler (object at `this+0x22c`,
`*(form+0x898)+0x6ac`, class `*0x733e28`, VMT `0x733ef0`, ProcessParam slot 0
`FUN_007378a0`); 21 and 22 reach neither and do nothing. The handler's index
record (`*0x00792f88` = `0x804628`) is filled by `FUN_0072c440(23)` from
`0x0073ad35` (Fruit Kick passes 6):

```
first 23, stride 17; blocks 23..107: block b = (i - 23) / 17, word w = (i - 23) % 17
     -> tab+8 + 0xa0 b + 4 w, then FUN_0072a400 (the block's derived values), tab+0x334 = 1
cutoff 110 -> tab+0x328, resonance 111 -> tab+0x32c, type 113 -> tab+0x330 (tab = frame+0x6b8)
108, 109, 112: nothing stored, nothing read
REC_FromMIDI (0x20): the control from FUN_007369e0 (frame+0x6a4 [w], built by
     FUN_007364a0: w 0 none, w 1 the "use envelope" box, w 2..16 the Tag 0x1000 + w
     wheels; 0x640 / 0x638 / 0x630 for cutoff, resonance and type), rescaled by
     FUN_00737820 over its Min (+0x3a8) .. Max (+0x3ac); words 8 and 11 (envelope and
     LFO amount) over -a..a with a = *0x790a60[b] = 64, 128, 128, 128, 2400
     (pan, volume, mod X, mod Y, pitch); w 0 has no control and is stored as given
```

Ranges (DFM): predelay, attack, hold, decay, release, LFO predelay and LFO
attack 100..65536, sustain 0..128, LFO speed 200..65536, LFO shape 0..2,
tensions -128..128, cutoff and resonance 0..256, type 0..7.

For 0..20: REC_FromMIDI rescales with the wheel's Min/Max; REC_UpdateValue
stores the int and recomputes, by `(idx + 1) mod 7` with osc `i = (idx + 1) div 7`:

```
idx 20              this+0x1f8 = p20 << 18                                  // random phase range
rem 0 (6, 13)       levels (FUN_00738f10):
                      m3 = p13 / 128;  m2 = p6 / 128                         // *0x00792be0 = 1/128
                      k  = AM ? 1 : 1 - m3
                      osc3.level = m3;  osc2.level = k * m2;  osc1.level = k - osc2.level
                      invert flags: level ^= 0x80000000
                      then ComputeLRVol for all three
rem 1 (0, 7, 14)    pan: Host.ComputeLRVol(&osc.L, &osc.R, p / 64, osc.level)   // host slot 31, *0x00792d18 = 1/64
rem 2 (1, 8, 15)    shape: table = shape < 6 ? Host.WaveTables[shape] : (this+0x178 or WaveTables[3])
                      osc.noise = shape == 5;  osc.aa = !noise && table != WaveTables[0]
rem 3, 4            osc.cents = coarse * 100 + fine
rem 5 (4, 11, 18)   osc.phaseOffset = p * 0x1ffffff
rem 6 (5, 12, 19)   osc.detune = p
then FUN_00738ff0:  this+0x1fc = p20 != 0 || any osc (noise || phaseOffset || detune || pan)
```

Shape 6 ("custom") uses the channel's sample as a 16384-point table, pushed
by Dispatcher ID 10 (FPD_ChanSampleChanged) into `this+0x178` and into the
engine (`dispatch(obj, 3, table)`); without one `FUN_00739610` reports shape 3.

Pitch to increment (`FUN_00725970`): `inc = round(this+0xe4 * 2^(cents / 1200))`
with `this+0xe4 = 2247346493527.166 / SR`, i.e. a 32-bit phase where 0 cents
is 523.25 Hz (the constant is `2^32 * 523.2511`). The host's
`FinalLevels.Pitch` is `(key - 60) * 100 + fine pitch + channel pitch`
(`host-voice.md`, section 3), so an oscillator at 0 semitones plays 523.25 Hz
for C5 (MIDI 60): one octave above a standard instrument, which is what the
default preset's -12 and -24 on oscillators 2 and 3 lean on. The voice
reads `FinalLevels.Pitch` again every tick (3.1), so a pitch the host moves
under a sounding note (a slide note, a clip on the channel's pitch) is
heard from the next tick: `three_osc.rs` takes it from
`Generator::set_note_pitch`.

Dispatcher (`FUN_00737f90`): 1 FPD_ProcessMode -> `0x1fd = value & 2`; 4
SetSampleRate -> `0xe4`, engine `dispatch(obj, 2, SR)`; 7 FPD_UseVoiceLevels
returns 2 (mod X/Y are the filter's); 10 custom shape; 13 SongPosChanged ->
global LFO phase (`FUN_0072b590`); 14 SetTimeSig -> recompute envelope times
(`FUN_0072c410`); 0x1b/0x28 -> host message 0x5a with the param range when
no articulation block was loaded; 0x33 param lookup (`FUN_0073a6f0`).

## 3. Voice model

`TriggerVoice(params, tag)` (`FUN_00739a40`): first `FUN_00739b80` releases
the oldest voice when `this+0xe0` (max polyphony) is reached. Then a 0x34-byte
voice record:

```
rec+0x00  host tag                       rec+0x08  TVoiceParams pointer
rec+0x10 + i*8  left phase  = 0          rec+0x14 + i*8  right phase = osc[i].phaseOffset
          both += (rand(N) - N/2) * 256 with N = this+0x1f8 (independent draws for L and R)
rec+0x28  stereo flag (copied from this+0x1fc at trigger)
rec+0x2c  articulation voice (FUN_0072b3b0: creates it, registers it with the manager)
```

`TVoiceParams` as read by the plugin: `+0x14` FinalLevels.Pan, `+0x18` Vol,
`+0x1c` Pitch (cents), `+0x20` FCut, `+0x24` FRes.

`Gen_Render(dest, &len)` (`FUN_0073ab70`) when voices exist: prepare the
manager's three block buffers (`FUN_0072b350`), for each voice
`FUN_0072b450` (below), add the manager's mix buffer into `dest`
(`FUN_0072b510`, through the host's add function `*0x00792fb8`), then
`Host.Voice_Kill(tag, -1)` (host slot 5) for every voice whose articulation
flagged it finished.

Per voice (`FUN_0072b450`): if a tick passed, step the envelopes (3.2); call
`Voice_Render(rec, renderBuf, &len)`; apply the articulation (3.3); finished =
volume envelope done or Voice_Render returned 1 (it always returns 0).

### 3.1 Voice_Render (`FUN_0073a230`): the oscillators

```
pitch = params.FinalLevels.Pitch                       // cents, already includes the pitch envelope/LFO (3.2)
first = true
for pass in (L, R):                                    // R only when rec.stereo; else R = copy of L (FUN_00739e70)
    for i in 0..2:
        osc = this+0x180 + i*0x24
        shape = FUN_00739610(i)
        inc = osc.noise ? rand(0x99326) + 0x99326      // noise: a random increment per block, 0x99326..2*0x99326
                        : round(base * 2^((pitch + osc.cents [+ osc.detune on R]) / 1200))
        if osc.level == 0:  phase += inc * len; continue           // silent oscillator still runs its phase
        gain = pass == L ? osc.L : osc.R
        if i == this+0x1f4 (AM osc):                 mode = 12     // multiply
        elif this+0x1fd == 0 || this+0x208 || !osc.aa: mode = first ? 11 : 10;  first = false   // write / add
        else: FUN_0073a000(ctx, pass); continue      // box-filter AA (below)
        phase = FUN_00739e90(ctx, mode, shape, table, buf, len, phase, inc, gain)
if this+0x1fc: rec.stereo = 1
```

`FUN_00739e90` picks the renderer: when the HQ checkbox is on and the engine
loaded, `dispatch(engineObj, mode, {shape, buf, len, phase, inc, gain})`;
otherwise the software kernels behind `*0x00804680` (add), `*0x00804688`
(write) and `*0x00804690` (multiply), filled by the unit init `FUN_0073acc0`:
`FUN_00739d70` / `FUN_00739dc0` / `FUN_00739e10` (SSE scalar), or
`FUN_00739ca0` / `FUN_00739ce0` / `FUN_00739d20` (x87) when the CPU flag byte
`PTR_DAT_00793030[0x16]` is clear. All six are the same loop, `asm`:

```
kernel(table, buf, len, phase: u32, inc: u32, gain) -> phase:
    for i in 0..len:
        s = table[phase >> 18] * gain            // 16384-point host table, no interpolation
        add:      buf[2i] += s
        write:    buf[2i]  = s
        multiply: buf[2i] -= buf[2i] * s         // AM: buf *= 1 - s
        phase += inc                             // wraps at 2^32
    return phase
```

The buffers are interleaved stereo; the kernel touches one channel (stride
2) and `Voice_Render` runs it once per side. Mode 11 (write) is the "zero the
channel first" of the engine path; 12 is AM with depth `gain` (osc 3's level,
so mix level 128 is 100 % modulation).

Box-filter anti-aliasing (`FUN_0073a000`, used in HQ *render mode* with the
checkbox off, for non-sine non-noise shapes): `n = max(1, inc >> 22)`; render
the same block `n` times with gain `gain / n`, each pass starting
`inc / n` further in phase. That averages `n` copies spread over one sample's
phase advance, a one-sample box filter.

Engine (`dispatch` @ `0x180001010`): id 0 creates the 0x48-byte object (SR
44100) and on first use builds four 2048-point shape tables (`FUN_180001310`:
shapes 1..4) plus their mip chains (`FUN_180002c40`, `FUN_180002d90`); id 2
sets the sample rate; id 3 loads a custom 16384-point table, decimated by 8
(`FUN_1800014f0`); ids 10/11/12 render (`FUN_180001660`), 11 zeroing the
channel first. The kernel:

```
sine  (shape 0): out = sinf(phase * 2pi / 2^32) * gain            // 0x18001ea08 = 2pi
noise (shape 5): seed = seed * 0xbb38435 + 0x3619636b; out = seed * 2^-31 * gain   // one global seed 0x180024a1c
else:            cyc = inc * 2^-32 (cycles per sample)                         // 0x18001e978 = 2^-32
                 f   = cyc / engine+0x40                                        // +0x40 = 1 / SR, so f in Hz
                 level = 4 log2(f / 8.1758) - 5 = (MIDI note) / 3 - 5           // 0x18001e9b0 = 1/8.1758 (MIDI note 0), 0x18001e9f0 = 4/log10 2, 0x18001e9e0 = 5
                 l   = int(clamp(level, 0, N - 2))                              // N = levels, 0x18001e9d0 = 2
                 w   = 1 - frac(level)                                          // stored at +0x1c, not used by this loop
                 if cyc < 5e-4: out = shape(phase / 2^32) analytically (FUN_1800011f0)      // below ~22 Hz
                 else: p = (phase * 2^-32) * 32768; out = lerp(table_l[int p], table_l[int p + 1], frac p)
                 out *= gain; buf += out (or buf -= buf * out for AM)
phase += inc per sample; the final phase is returned
```

Shape functions (`FUN_1800011f0`, x in 0..1): triangle `x < 0.25 ? 4x : x < 0.75 ? 1 - 4(x - 0.25) : 4(x - 0.75) - 1`;
square `x < 0.5 ? 1 : -1`; saw `2x - 1`; rounded saw: `-1` for `x >= 5/6`, `1`
for `x >= 0.4639`, else `1 - 2 f(x * 16384)` with `f = FUN_180014090` (the
CRT `pow`, as its other call site `pow(2, -1/n)` shows; the exponent register
for this call was not read).

Mip tables (`FUN_180002d90(levelsPerOctave, ratio, shape2048)`, `asm` for the
constants, `inferred` for the FFT identity): `N = levelsPerOctave * 11`
(`0x18001e9e8 = 11.0`) tables of `ratio * 2048` points plus a 2-point wrap
guard; the render's `4 log2` fixes `levelsPerOctave = 4`, so `N = 44` and
`ratio = 16` (32768 points). Each table is the 2048-point shape upsampled by
linear interpolation, then band-limited in the frequency domain
(`FUN_1800027c0` forward, `FUN_180001be0` cut and inverse) to its first
`1024 * 2^(-l/4)` harmonics (`0x18001ea00 = 1024`, ratio `pow(2, -1/4)` from
`0x18001ea20 = -1.0` and `0x18001e9d8 = 2.0`). The lowest note of level `l`
is `8.1758 * 2^((l + 5) / 4)` Hz, whose harmonic count under 22.05 kHz is
`1134 * 2^(-l/4)`, just above the table's cut. Level 0 keeps everything below
MIDI note 15 (19.4 Hz); level 42 (the last one reachable) is for MIDI 141 and
above.

### 3.2 Envelopes and LFOs (the articulation unit, `ghidra/out/osc3/voice`, `artic`)

Per voice five 0x24-byte envelope states at `avoice+0x50 + k*0x24` (k = pan,
vol, mod X, mod Y, pitch): `[0]` counter, `[1]` LFO phase (u32), `[2]` LFO
predelay left, `[3]` stage, `[4]` ramp value, `[5]` shaped output, `[6]` LFO
attack ramp, `[7]` LFO output. Derived per block (`FUN_0072a400`, SR from
`0x00790a84`, tempo from `0x00790a78`):

```
samples(v) = v <= 100 ? 0 : round((1001^(v / 65536) - 1) * 0.512 * SR [* tempo / 120 when flag bit 0/1 clear])
+0x58 predelay, +0x5c hold, +0x60 LFO predelay (samples); +0x44 = 1/(attack+1), +0x48 = -1/(decay+1), +0x50 = -1/(release+1), +0x54 = 1/(lfoAttack+1)
+0x4c sustain/128, +0x70 amount/128, +0x74 lfoAmount/128, +0x64 = 2^32 / max(3, samples(lfoSpeed)), +0x68 LFO shape table
tension k = sign(t) * (6347^|t/128| - 1) * 0.1 -> FUN_005f4ed0 fills (a, b) per segment at +0x7c, +0x88, +0x94
```

Stepping once per host tick (`FUN_00729eb0`): stages 0 predelay (count to
`+0x58`), 1 attack (`v += 1/(n+1)`, at 1 go to hold or decay), 2 hold, 3 decay
(`v -= 1/(n+1)` down to sustain, then stage 4), 6 release (`v -= 1/(n+1)`), any
stage > 2 ends at 9 when the output drops under `6e-8`. The shaped output for
tension `t`: `t > 0: 1 - (e^{(1-v) b} - 1) a`, `t < 0: (e^{v b} - 1) a`, `t = 0: v`.
Release (`FUN_0072a6e0`) converts the current shaped level back through the
inverse curve so the release starts without a jump. LFO: after `+0x60` samples
of predelay, `phase += +0x64` (or the global phase `block+0x78`, advanced per
tick by `FUN_0072b550`), `out = shape[phase >> 18] * attackRamp`.

Combination per tick (`FUN_0072bec0`): pan `+0x70 = lfo * lfoAmt * 2`; volume
`+0x94 = env * (lfo * lfoAmt + 1)`; mod X `+0xb8 = env * amt + lfo * lfoAmt`;
mod Y `+0xdc` the same; pitch `+0x100 = dEnv * amt + dLfo * lfoAmt` (deltas,
amounts as raw ints). `FUN_0072b8e0` then does `params.FinalLevels.Pitch += +0x100`:
the pitch modulation is written into the host's voice record, which is why
`Voice_Render` sees it.

### 3.3 Filter, pan and mix (`FUN_0072b920`, `FUN_0072bae0`)

```
cutoff = FinalLevels.FCut + filterCutoff / 256 + modX;  res = FinalLevels.FRes + filterRes / 256 + modY   // *0x00778fc0 = 1/256
type -> (kind, twice) from 0x00790b38: (0,0) (1,0) (2,0) (3,0) (4,0) (1,1) (6,0) (6,1)
kind 0:    FUN_00727940 coefficients, kernel *0x007a25f0 (Fast LP)
kind 1..4: FUN_007277a0 + FUN_00727140 coefficients, kernel *0x007a45e0 (biquad LP, BP, HP, Notch)
kind 6:    FUN_00727a70 coefficients, kernel *0x007a45f0 (Chamberlin SVF low-pass)
twice (voice+0x16c): the kernel runs again in place on its own output with the second state block (voice+0x14c)
(L, R) = ComputeLRVol(clamp(FinalLevels.Pan + panMod, -1, 1), FinalLevels.Vol * volEnv)   // FUN_005fa230, the plugin's copy of the host unit
mix buffer += render buffer * (L, R), ramped from the previous block's gains              // *0x00792fe8 -> *0x007a24e0, *0x00792060 -> *0x007a24e8
```

Mod X/Y from the host (the channel's filter knobs and their automation) and
the plugin's own cutoff / resonance are summed, so FL's channel filter and the
tab's filter are one filter. `s` below is `*0x0077e6f4`, 1.0 in the image and
rewritten by the sample-rate code at `0x005f8493` (`inferred` 44100 / SR).

Fast LP (kind 0). Coefficients, `asm`, with `*0x007a24b8 = ln 601`
(`FUN_005fbe80` from `0x005fc518 = 601.0`) and `*0x0077e6ec = 1/600`:

```
f  = cut <= 0 ? 0 : cut >= 1 ? 1 : min(1, (601^cut - 1) / 600 * s)
fb = res <= 0 ? 0 : 1 - (601^(1 - 0.625 min(res, 1)) - 1) / 600                       // 0x007907e4 = 0.625
```

`f` ramps across the block: `FUN_005fa110` turns the previous and new `f` into
a start value and a per-sample step limited to 0.001 (`*0x0077e6f8`). Kernel
(`FUN_005f8730` x87, `FUN_005fb4a0` SSE; per channel state `y` at state+0x28 /
+0x30 and `v` at +0x38 / +0x40, doubles):

```
for each sample:  v += (x - y) * f;  y += v;  v *= fb;  out = y;  f += df
```

a two-integrator low-pass whose resonance is the undamped fraction `fb`. The
SSE variant flushes `|state| < 1e-30`-class values to zero afterwards.

Biquad (kinds 1..4). `FUN_007277a0`, `asm`, with `*0x007a45fc = ln 51`
(`FUN_00727f20` from `0x00727fd8 = 51.0`) and `*0x007a4600 = 0.625 ln 51`:

```
w = clamp(((51^cut - 1) * 0.0596 + 0.02) * s, 0.02 s, 3.0)       // radians per sample: 140 Hz .. 21 kHz at 44.1 kHz
q = res <= 0 ? 0.1 : (51^(0.625 min(res, 1)) - 1) * 0.4 + 0.1    // 0.1 .. 4.7
(sn, cs) = sincos(w);  alpha = sn / (2 q);  n = 1 / (1 + alpha)
a1 = -2 cs n;  a2 = (1 - alpha) n
LP (1):    b1 = (1 - cs) n;  b0 = b2 = b1 / 2
BP (2):    b0 = alpha n;  b1 = 0;  b2 = -b0
HP (3):    b1 = -(1 + cs) n;  b0 = b2 = -b1 / 2
Notch (4): b0 = b2 = n;  b1 = -2 cs n
```

These are the RBJ cookbook forms. `FUN_00727140` also holds shelving (5, 7),
peaking (6) and all-pass (8) cases that no filter type selects. Kernel
(`FUN_00727b70`, `FUN_00727e10`), direct form I with the state `[x1 x1' x2 x2'
y1 y1' y2 y2']` (primes are the right channel) at voice+0x12c:

```
y = b0 x + b1 x1 + b2 x2 - a1 y1 - a2 y2
```

LPx2 (type 5) runs the LP twice and then scales the voice gain by
`1 - res * (3.2 - w) * 0.046875` (`0x0072bea0`, `0x0072bea8`).

SVF low-pass (kind 6). `FUN_00727a70` with `0x00727b58 = 26.0`:

```
f = cut <= 0 ? 0 : min(1, (26^cut - 1) * 0.04 * s)              // (26^cut - 1) / 25
q = res <= 0 ? 1 : res < 1 ? 1 - 0.8 res : 0.2                  // damping, 0x007907ec = 0.8
```

Kernel (`FUN_00727c90`, `FUN_00727d20`), Chamberlin form with `low` at
state[0..1] and `band` at state[2..3]:

```
low += f * band;  band += f * ((x - low) - q * band);  out = low
```

SVF LPx2 (type 7) runs the second stage with `q2 = q + (1 - q) * 0.25`
(`*0x00778fa0`) and restores `q`.

Pan. `FUN_005fa230` is the host's `ComputeLRVol` compiled into the plugin
(kernels `FUN_005fa1c0` / `FUN_005fa1f0`, SSE pair `FUN_005fb630` /
`FUN_005fb6a0`, selector `DAT_007a2478`). The selector is only written by its
own unit init, so the voice pan is always the circular law
`L = cos((1 + pan) pi/4) vol, R = sin(...) vol` regardless of FL's pan-law
setting (`host-voice.md`, section 1).

Mix. `*0x007a24e0 -> FUN_005f8a30(src, dst, n, gL, gR)`: `dst += src * (gL,
gR)` on interleaved stereo. `*0x007a24e8 -> FUN_005f8c00(src, dst, n, gL, gR,
dL, dR)`: the same with the gains advanced by `(dL, dR)` per frame, which is how
the block ramps from the previous gains. (The first slot of each pair,
`FUN_005f89c0` / `FUN_005f8b90`, raises "Not implemented in i64/ARM".)

## 4. What stays unknown

- The host's ten wavetables (`TFruityPlugHost+0x18`): the FLEngine routine
  that generates them was not found; shapes 0..5 are assumed to be sine,
  triangle, square, saw, rounded saw, noise in that order (matches the
  shape selector and the engine's own shape numbering).
- `*0x0077e6f4`, the sample-rate factor in every filter coefficient: 1.0 in
  the image, written at `0x005f8493`; taken as `44100 / SR`.
- Engine: the two arguments of `FUN_180002d90` (4 levels per octave and the
  2048 to 32768 upsampling ratio are `inferred` from the render's `4 log2` and
  the 32768 index), the FFT identity of `FUN_1800027c0` / `FUN_180001be0`,
  and the exponent `pow` takes in the rounded saw. The cross-level weight the
  kernel computes is unused by the scalar loop that was read; a SIMD path may
  blend two levels.
- `TDelphiFruityPlug` slots 20 and 21 (`0x00725e20`, `0x00725eb0`), GUI only.

## Corrections from the implementation (crates/plugins/src/three_osc.rs)

- Envelope times are ticks, not samples: `0x00790a84` is PPQ/4, so the time curve is `(1001^(v/65536) − 1)·0.064` s. The flag meaning is inverted too.
- The host wavetables are at FLEngine `0x9cdfc0`; the non-HQ tables are exact.
- Rounded saw is `1 − 2·0.997^(16384x)`.
- AM depth and oscillator gains carry the pan law's 0.7071; default levels are 3/8, 3/8, 1/4; Q tops out at 4.37.
- Note-off without a volume envelope is a 10 ms cosine fade.
- 152 project states, not 116; 181 presets. HQ (13 states) and custom shape 6 report Approximate.
- Envelope ticks use the song PPQ through `Generator::set_ppq`.
