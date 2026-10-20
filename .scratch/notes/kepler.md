# Kepler: handover (in progress)

Branch `claude/flrs-kepler-drumsynth`. Fruity DrumSynth Live is done there
(`crates/plugins/src/drumsynth_live.rs`); this note is Kepler's.

## Files and tools

- `Plugins/Fruity/Generators/Kepler/Kepler_x64.dll` (Delphi shell) and
  `engine_x64.dll` (MSVC C++, base 0x180000000, `CreatePlugInstance` at
  0x180009da0). The engine is Kepler Exo's: the full engine with 0x102
  parameters at `this+0x1ab80` (f32 0..1), and "Kepler-1" is a mode that
  maps 36 of them.
- Ghidra projects `/root/ghidra/p_kepeng` and `p_kepler` (on the machine
  this started on; rebuild with `analyzeHeadless ... -import`).
  `DecompileRange.java` over 0x180001000..0x180200000 gives every function
  (1988, 161k lines; most is the GUI framework).

## State (event 213)

Shell wrapper `00000000 | u32 size-12 | 00000000`, then the engine's
stream (`FUN_18001ef30` save / `FUN_18001f170` load):

```
i32 len (9) | "Kepler-1\0" | i32 version (4) | i32 count (34 or 36) | count x f32
version > 1: u8 (this+0x390f*8, flag) | f32 (this+0x1aca4, param 0x49 Velocity->Volume) | f32 (this+0x1abac, param 0xb DCO1 Sub Osc Phase)
version > 2: u8 (this+0x1bd8c) | u8 chorus noise on (param 0x46 = DAT_1800ed620 or 0) | u8 oversampling: 4 if set else 2 (this+0x1cd94)
version < 4 and param 0x... >= 0.5: an LFO rate fix (end of FUN_18001f170)
```

Corpus: 4 states (Edlan & Ella Noël - Song For You "Pads - Kepler",
Purps On The Beat - Stroll Up "Kepler", Sid De La Cruz x2) and 277 of 465
presets under `Patches/Plugin presets/Generators/Kepler`. Silent channels
with notes: the Edlan pad (no arp) and Stroll Up (arp on, up/down, 2 oct,
rate 0.511 free).

Load resets every engine param to `FUN_18001d900(i)` (defaults: 1.0 for
3, 8, 10, 0xf, 0x11, 0x17, 0x20, 0x21, 0x25, 0x28, 0x49, 0x5a, 0x9a; 0.5 for
0xb, 0xd, 0x12, 0x2f, 0x30, 0x3e, 0x42, 0x43, 0x48, 0x4a, 0x53, 0x5c..0x63,
0x65..0x6d, 0x95, 0x96, 0x100, 0x101; `DAT_1800ed7d8` for 0xe, 0x36, 0x3f;
0 else) in `FUN_18001ed30`, which also sets mod matrix slot 0 source 6 ->
0x25 (cutoff) and slot 1 source 7 -> 0x26 (resonance) at amount 1.

### Kepler-1 index -> engine param (`FUN_180016300`)

```
0 0x55 Arp Active      1 0x59 Arp Mode       2 0x58 Arp Range     3 0x56 Arp Rate
4 0x57 Arp Sync        5 0x30 LFO1 Rate      6 0x31 LFO1 BPM Sync 7 0x32 LFO1 Delay
8 0x33 LFO1 Trig Mode  9 0x51 LFOs Trigger   10 0x00 LFO1->DCO1   11 0x05 DCO1 PW amount
12 0x06 PW mod source  13 0x07 pulse active  14 0x08 saw active   15 0x09 sub active
16 0x0a sub volume     17 0x22 noise         18 0x24 HP freq      19 0x25 cutoff
20 0x26 resonance      21 0x2b env polarity  22 0x2c env amount   23 0x2d LFO1->cutoff
24 0x27 key follow     25 0x2f VCA source    26..29 0x3d..0x40 Env1 ADSR
30 0x45 chorus mode    31 0x52 hold          32 0x53 octave       33 0x48 main volume
34 0x34 LFO1 retrig phase active             35 0x35 LFO1 retrig phase
```

Engine parameter names: `/root/work/kep_params.txt` was made from the name
switch at lines ~20640..21860 of the decompile (`FUN_18001fa30`).

## Engine call tree (addresses)

- `MyPlugin` vftable 0x1800ead10: slot 6 note on `FUN_180018fe0`, 7 note
  off `FUN_180019160`, 15 save, 16 load, 32 process `FUN_180019210`.
- Process: per frame `FUN_18001b940(this, outL, outR, ppq, flag)`; MIDI
  0x90/0x80/0xb0/0xe0 dispatched at their frame.
- Param update `FUN_180019a20(this, index, ...)` (6.7 KB, a switch on the
  index; tables at 0x180254100 for LFO/env times).
- Frame: global LFOs (`FUN_1800140c0` at +0x1c6c8), 8 voices of 0x32e8
  bytes at `this+0x1440`, `FUN_180023ab0` per voice (modulation), filter
  coefficient blocks of 4 lanes (types at `this+0x13ac`: 0 at +0x1c880,
  1 at +0x1c890, 2 at +0x1c8a0), then per oversampled step
  (`this+0x1cd94` = 2 or 4): `FUN_180024200` per voice (DCO
  `FUN_180014f80`, or HQ `FUN_180014570`; noise from an i16 table at
  0x180105cc0 of 0xa6905 entries times `_DAT_1800ed5dc`), the filter
  (`FUN_180013260` type 0, `FUN_1800135b0` x4 type 1, `FUN_180013a50`
  type 2), VCA and sum; decimation `FUN_18001cd80`; then the HPF
  (`this+0x1c649`), chorus `FUN_180001a60` at +0x1bbb0 with a crossfade
  when its mode changes, and the rest of the frame (EQ, saturation, reverb,
  delay: default off; check).
- DCO: band-limited steps into a ring buffer (`FUN_180014da0`,
  `FUN_180014d00`), a leaky integrator at +0x5c.

## Next

Read the rest of `FUN_18001b940` (from the chorus on), `FUN_180023ab0`,
the DCO helpers, filter type 0, the decimator, the chorus, the note
on/off and voice allocation, the arpeggiator (`FUN_180012d60` and the
block in `FUN_180019210`), and the param update cases the 36 params and
the defaults reach. Then write `crates/plugins/src/kepler.rs`.

## Map read so far (2026-10-20, later)

Working files on the machine this ran on: `/root/work/kep_*.c` (extracted
functions), `/root/work/kepeng/tree/` (decompiles with callees).

- FL voice -> note (`FLWrapperPlug` TriggerVoice `FUN_180009160`): key =
  `int(InitLevels.Pitch) / 100` clamped 0..127, velocity =
  `clamp(logf(InitVol * 10 + 1) * 41.71426, 0, 127)` as an int; then note
  expressions (`FUN_1800184f0`): 0 velocity/127, 1 FCut, 2 FRes, 3 fine
  pitch (cents * 1/1200 -> voice+0x1264, octaves), 4 pan: `p = clamp((pan+1)/2)`,
  `L = sqrt(1-p) * 1.4144272`, `R = sqrt(p) * 1.4144272` (voice+0x1268/0x126c).
  Gen_Render (`FUN_180009740`) re-sends changed FinalLevels, runs process, adds
  the stereo result to FL's buffer with no gain.
- Voices: 8 at this+0x1440, 0x32e8 bytes. Allocation (`FUN_180018680`): the
  voice already on the key, else a free one (gate +0x2e60 == 0) with the
  lowest age (+0x11c4 counter), else the oldest. Start `FUN_180022280`:
  RNG `x*0xbb38435+0x3619636b` at this+0x1cd98 -> +0x11d0; glide from/to
  `(note-69)/12` (+0x2e48/+0x2e44); velocity curve
  `((v^2*(v*c708) - c644*v*v) + c648*v) * vtv + (1 - vtv)` (+0x2e90), envelopes
  restart. Release (`FUN_180018b30`): env stages 2, gate 0, vel target 0.
- Envelopes (`FUN_180023ab0`): stage 0 `e += (1.7 - e) * a` until >= 1;
  stage 1 `d *= k; e = d * (1 - s) + s`; stage 2 `e *= r`. Coefficients in
  `FUN_180022840` cases 0x3d..0x44 from 11-point tables (0x180105b40 decay,
  0x180105bd0 release, 0x180105c30 attack) and `pow(0.41176, 1/(rate * t))`.
- Pitch: octaves = bend*range + glide + octave(+0x1260) + fine(+0x1264) +
  master tune + LFO1*amt + DCO pitch + env2*amt; inc =
  `pow(2, oct * 1.0005) * 440 / fs_os` (f64), clamp 0..0.25.
- Cutoff: `powf(2, (note-24)/12 * keyfollow + LFO2*a + pol*env1*envamt +
  LFO1*a) * base(+0x11d4, from table 0x180105b70 via powf)`, clamp 5..30000;
  resonance = param 0x26 clamped 0..1.
- Filter type 0 (`FUN_180013260`), lanes of 4 voices: `g = tan approx
  (2.4716885 - 0.1897597 x^2) x / (2.4674011 - x^2)` with `x = min(cut,
  0.4995 fs_os), >= 20) * pi/fs_os`; `G = g/(1+g)`, `H = 1/(1+g)`,
  `k = 4.2 res`, in gain `res + 1`, `1/(1 + k G^4)`; 4 TPT stages with the
  Pade tanh `x(x^6+378x^4+17325x^2+135135)/(28x^6+3150x^4+62370x^2+135135)`
  on [-5,5]; input drive 1/2.745, 2.745 after; output pole count from
  param 0x28 (default 4 poles).
- VCA (frame): source param 0x2f: env1 (`max(env1 - 0.025, 0) * vel*c71c`)
  or gate (the velocity gain smoothed toward 0 on release); pan gains
  smoothed; sum to L/R.
- Oversampling: `this+0x1cd94` = 2 (4 when the state's last byte is set);
  decimation `FUN_18001cd80`: polyphase allpass halfband, coefficients from
  `FUN_1800298a0` (not read yet).
- DCO (`FUN_180014f80`, `FUN_180014da0`, `FUN_180014d00`): leaky (0.999)
  integrator of `-inc*saw` plus a 32-entry ring of band-limited impulses at
  each edge (saw wrap, pulse at PW, sub toggling at the sub phase point);
  impulse table from `FUN_180015130`: the impulse response of a 16-pole
  Butterworth (Q table at 0x1800edd30) in f64, peak-normalised, last 1024
  faded, 1025 fractional rows of 32 taps.
- Noise: i16 table at 0x180105cc0 (0xa6905 entries) * 8.6316e-5, one index
  per voice.
- Arp (`FUN_180012940` on, `FUN_180012b30` off, `FUN_180012d60` step,
  clock in `FUN_180019210`): lists as played / reversed / ascending /
  descending; modes up, up+down, down (+ random 3..5, 6..8 as played);
  range from param 0x58; step from table 0x1802540d0 (seconds) or synced.
- Chorus `FUN_180001a60` at this+0x1bbb0 (mode param 0x45: off, I, II,
  I+II), design `FUN_180002690`, mode change crossfade in the frame.
- HPF (param 0x24, active param 0x9a default on): one-pole TPT at
  this+0x1c64c, cutoff `powf(.., ..)` (constants at 0x1800ed7b0, 0x1800eda90,
  0x1800eda00).
- Main volume/pan (params 0x48/0x4a): `g = ((c668 - v*c760)*v - c674)*v`,
  `L = sqrt(1-pan) g`, `R = sqrt(pan) g`, smoothed by this+0x13d8.
