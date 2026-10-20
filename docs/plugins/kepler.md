# Kepler: state, engine, voice model

Recovered from `Kepler_x64.dll` (FL Studio 2026, Delphi shell) and its
`engine_x64.dll` (MSVC C++, image base 0x180000000, export
`CreatePlugInstance` at `0x180009da0`). The engine is Kepler Exo's: 0x102
engine parameters, f32 from 0 to 1, at `this+0x1ab80`. A "Kepler-1" state
sets 36 of them and leaves the rest at the engine's defaults, so DCO 2,
LFO 2, envelope 2, the mod matrix, EQ, saturation, delay, reverb and filter
types 1 and 2 never sound in a Kepler. Corpus: 4 states (Edlan & Ella Noël
- Song For You, Purps On The Beat - Stroll Up, Sid De La Cruz x2), all
version 4, and 277 presets under `Patches/Plugin presets/Generators/Kepler`.

Confidence tags: `dec` (decompile), `asm` (listing), `corpus`, `inferred`.

## How the plugin is wired

- `MyPlugin` vftable `0x1800ead10`: slot 6 note on `FUN_180018fe0`, 7 note
  off `FUN_180019160`, 15 save `FUN_18001ef30`, 16 load `FUN_18001f170`,
  32 process `FUN_180019210`. `dec`
- FL's wrapper (`FLWrapperPlug`): TriggerVoice `FUN_180009160` turns FL's
  voice into a MIDI note, key `int(InitLevels.Pitch) / 100` clamped 0..127,
  velocity `int(clamp(ln(InitVol * 10 + 1) * 41.71426, 0, 127))`; note
  expressions `FUN_1800184f0` (fine pitch `cents / 1200` octaves at voice
  `+0x1264`; pan `p = clamp((pan + 1) / 2)`, gains `sqrt(1 - p) * 1.4144272`
  and `sqrt(p) * 1.4144272` at `+0x1268`/`+0x126c`). Gen_Render
  `FUN_180009740` runs the process and adds the stereo result to FL's
  buffer with no gain. `dec`
- Process `FUN_180019210`: per frame `FUN_18001b940(this, outL, outR, ppq,
  flag)`; MIDI 0x90/0x80/0xb0/0xe0 at their frame; the arpeggiator's clock.
  `dec`

## 1. State (event 213)

```
u32 0 | u32 size - 12 | u32 0                               shell
i32 9 | "Kepler-1\0" | i32 version (1..4)                   FUN_18001f170
version > 1: i32 count (34 or 36) | count x f32             Kepler-1 order
             u8 flag (this+0x390f*8)
             f32 velocity -> volume (engine 0x49)
             f32 sub oscillator phase (engine 0x0b)
version > 2: u8 (this+0x1bd8c) | u8 chorus noise (0x46 on) | u8 oversampling 4 (else 2)
version < 4 and LFO sync (0x31) >= 0.5: the LFO rate (0x30) is reread
  x = (2^(7 rate) - 1) * 48.37008 + 1.1; t = int(9216 / int x) - 1;
  rate = 1 - log2(t * 0.020673936 + 1) * 0.2060993
```

Before loading, every engine parameter goes to its default `FUN_18001d900`
(`FUN_18001ed30`): 1.0 for 3, 8, 0xa, 0xf, 0x11, 0x17, 0x20, 0x21, 0x25,
0x28, 0x49, 0x5a, 0x9a; 0.5 for 0xb, 0xd, 0x12, 0x2f, 0x30, 0x3e, 0x42,
0x43, 0x48, 0x4a, 0x53, 0x5c..0x63, 0x65..0x6d, 0x95, 0x96, 0x100, 0x101;
`DAT_1800ed7d8` (0.33) for 0xe, 0x36, 0x3f; 0 else. `dec`

### Kepler-1 index -> engine parameter (`FUN_180016300`)

```
 0 0x55 Arp Active      1 0x59 Arp Mode        2 0x58 Arp Range     3 0x56 Arp Rate
 4 0x57 Arp Sync        5 0x30 LFO1 Rate       6 0x31 LFO1 BPM Sync 7 0x32 LFO1 Delay
 8 0x33 LFO1 Trig Mode  9 0x51 LFOs Trigger   10 0x00 LFO1->DCO1   11 0x05 PW amount
12 0x06 PW source      13 0x07 pulse on       14 0x08 saw on       15 0x09 sub on
16 0x0a sub level      17 0x22 noise          18 0x24 HPF          19 0x25 cutoff
20 0x26 resonance      21 0x2b env polarity   22 0x2c env amount   23 0x2d LFO1->cutoff
24 0x27 key follow     25 0x2f VCA source     26..29 0x3d..0x40 envelope ADSR
30 0x45 chorus mode    31 0x52 hold           32 0x53 octave       33 0x48 main volume
34 0x34 LFO1 retrigger phase on               35 0x35 LFO1 retrigger phase
```

The plugin's parameters are these 36, 0 to 1 as the state holds them.

## 2. Tables in the engine

All 11-point curves, read at `v` between points `int(10 v)` and the next:

| address | use | unit |
|---|---|---|
| 0x180105b40 | decay (0x3e) | s to -20 dB |
| 0x180105b70 | cutoff base (0x25) | log2 Hz |
| 0x180105ba0 | LFO1 -> DCO (0x00) | octaves |
| 0x180105bd0 | release (0x40) | s to -20 dB |
| 0x180105c00 | LFO1 -> cutoff (0x2d) | octaves |
| 0x180105c30 | attack (0x3d) | s |
| 0x180105c60 | levels (sub 0x0a, noise 0x22) | gain |
| 0x180105c90 | env -> cutoff (0x2c) | octaves |
| 0x180254100 | LFO1 delay (0x32, `FUN_180019a20` case 0x32) | s |
| 0x1802540d0 | arp rate free (0x56) | s per step |
| (not recorded) | LFO1 rate (0x30) | `21.73913 * 2^-v` Hz |

Synced arp steps: 96, 72, 64, 48, 36, 32, 24, 18, 16, 12, 9, 8, 6 /96 of a
beat, at `int(12 rate + 0.5)`. Noise: i16 at `0x180105cc0`, 0xa6905
entries, times `8.6315915e-5`; each voice starts at `k * 85280.625`. LFO1's
wave: 2170 i16 at `0x180252ed0`, normalised by its middle and peak, the
first point appended. Halfband coefficients from `FUN_1800298a0`; the
Butterworth Qs of the step table at `0x1800edd30`. `dec`, `asm`

## 3. Voices

Eight voices of 0x32e8 bytes at `this+0x1440`.

- Allocation `FUN_180018680`: the voice already on the key, else the free
  voice (gate `+0x2e60` == 0) with the highest age (`+0x11c4`, frames since
  start), else the oldest. `dec`
- Start `FUN_1800188a0`/`FUN_180022280`: RNG `x * 0xbb38435 + 0x3619636b`
  (this+0x1cd98); pitch `(note - 69) / 12` octaves; velocity gain
  `(v^3 * 0.98435 - 0.231573 v^2 + 0.247223 v) * vtv + (1 - vtv)` with `vtv`
  param 0x49; the envelope restarts in attack. Release `FUN_180018b30`:
  stage 2, gate 0, gain target 0. `dec`
- Per frame (`FUN_180023ab0`, coefficients `FUN_180022840`): envelope
  attack `e += (1.7 - e) * (1 - 0.41176^(1 / (fs t)))` to 1, decay
  `d *= 0.1^(1 / (fs t)); e = d (1 - s) + s`, release `e *= 0.1^(1/(fs t))`.
  Pitch `oct + glide + fine + 2 LFO1 * amt`, increment
  `2^(1.0005 pitch) * 440 / fs_os` clamped to 0.25. Cutoff
  `2^((key - 24)/12 * follow + pol * env * amount + 2 LFO1 * amount) * base`
  clamped 5..30000 Hz. PW `0.458 - src * 0.432 * amount`, src LFO1's
  triangle (as 0..1), 1, or the envelope. The velocity gain smoothed by
  `1 - 0.1^(400/fs)`. `dec`
- DCO (`FUN_180014f80`, `FUN_180014da0`, `FUN_180014d00`): a leaky
  integrator (`min(0.37^(10/fs), 1 - 0.267949 inc)`) of `-inc * saw` and a
  32-tap ring of band-limited steps at every edge (saw wrap: saw + pulse;
  pulse fall at the width; sub flip at `0.99 * param 0x0b`); out `2y - saw`.
  The step table `FUN_180015130`: the impulse response of a 16-pole
  Butterworth at 1024 times the rate (f64 cookbook stages), from 1536 on,
  times `0.5 / peak`, the last 1024 faded; row `r` tap `j` = `ir[1024 j - r]`.
  `dec`
- Mix: `(noise * level + dco * param 0x20) * 0.27` into the filter.
- Filter type 0 (`FUN_180013260`), four voices per lane: `x = clamp(cut,
  20, 0.4995 fs_os) * pi / fs_os`, `g = (2.4716885 - 0.18975969 x^2) x /
  (2.467401 - x^2)`, `h = 1/(1+g)`, `G = h g`, `k = 4.2 res`, input gain
  `res + 1`, `1 / (k G^4 + 1)`; input `(gain x - k fb) * 0.36429873`, Padé
  7/6 tanh on [-5, 5], times 2.745; four TPT stages each with a tanh.
  `dec`
- VCA: param 0x2f >= 0.5 is the gate (the smoothed velocity gain), else the
  envelope `max(env - 0.025, 0) * gain * 1.0256411`. Pan gains smoothed by
  `1 - 0.1^(200/fs)` after the first frame. `dec`

## 4. After the voices

- Oversampling 2 (4 from the state's last byte); `FUN_18001cd80` takes each
  halving through a polyphase halfband: two branches of six allpasses in
  `z^-2`, output `(A + B one sample late) / 2`. `dec`
- HPF (param 0x24, on with 0x9a): a one-pole TPT highpass at
  `20 + 2680 * v^1.6` Hz. `dec`
- Chorus `FUN_180001a60` at `this+0x1bbb0`, mode `int(3 v + 0.5)` of param
  0x45 (off, I, II, I+II): mono in, input highpass at 0.5 Hz, two 7 kHz
  cookbook lowpasses, two delay lines of 0x8000 with Hermite reads, a 7 kHz
  one-pole out. Modes I/II: triangle at 0.4/0.6 Hz, centre 1.08 ms, depth
  4.15 ms, the right LFO half a turn on; I+II: sine at 8 Hz, centre 2.85 ms,
  depth 0.52 ms, offset 0.0286. Delay, mix (toward 0.5, equal power) and
  level smoothed by `1 - 0.1^(20/fs)`. `dec`
- Main volume/pan (0x48/0x4a): `g = ((0.32754314 - 1.3922914 v) v -
  0.34967893) v`, `L = sqrt(1 - pan) g`, `R = sqrt(pan) g`, smoothed by
  `1 - 0.1^(400/fs)`; then the engine's output gain 6.309. `dec`

## 5. LFO 1 and the arpeggiator

- LFO 1: phase increment from the rate curve, or synced (0x31) to
  `192 / int((2^(7 v) - 1) * 48.37008 + 1.1)` turns a beat; a delay ramp
  (0x32); the voices hear the recorded wave times the ramp; trigger mode
  0x33 restarts the delay (and with 0x34 the phase at 0x35) on the first
  note. `dec`
- Arp (`FUN_180012940` on, `FUN_180012b30` off, `FUN_180012d60` step):
  held notes as played, ascending, descending; modes up, up and down, down
  (param 0x59 thirds); octaves `1 << int(2 v + 0.5)`; a step from the free
  table or the synced list, a gate of `step * param 0x5a`; one note at a
  time. `dec`

## What the port leaves out

- The step table is normalised by an exact reciprocal; FL's `rcpps` and a
  Newton step differ by CPU.
- The 15 ms crossfade when the chorus mode changes.
- `FPE_MIDI_Vol`/`FPE_MIDI_Pan`: the engine's output gain stays 6.309 and
  the host's channel volume goes on after.
- The song position the LFO and arpeggiator sync to is counted from the
  first frame played.
- Hold (0x52), the chorus noise, the arpeggiator's random modes (none
  reachable from Kepler-1's three-way switch) and automation smoothing.
- The envelope and gain are flushed to zero below 1e-30, where FL keeps
  subnormals or runs with flush-to-zero.
