# MiniSynth: state, parameters, voice, effects

Recovered from `Plugins/Fruity/Generators/Minisynth/MiniSynth_x64.dll` (FL
Studio 2026, 542 KB, MSVC C++, image base 0x180000000). It is FL Mobile's
MiniSynth inside the C++ wrapper FL uses for its mobile synths, the one
Drumpad and BassDrum's engines sit in (`docs/plugins/drumpad.md`): the same
`SSLF` state, `ProcessParam`, TriggerVoice and slot fill, at the same
addresses. RTTI names the classes `CFLWrapper`, `CPluginRenderer`,
`CSynthesizer`, `CMiniSynth`, `CMiniSynthVoice` and `MiniSynthInstance`.

The whole DLL was decompiled with headless Ghidra and every function cited
below was read again in its asm, with rip-relative constants resolved
(Capstone). The decompile and the scripts stayed in that session's scratch
directory.

Corpus: 12 states in projects (10 in Eric Kauffmann & NrgMind's Exoplanet,
1 in the SFX transitions template, the render test `System/Render
tests/MiniSynth.flp`) and 6 presets. All are 176-byte `DATA` blocks of 43
parameters; the Rust loads and sounds each of them.

## How the plugin is wired

| function | address | what it does |
|---|---|---|
| CreatePlugInstance | 0x1800022c0 | the wrapper; the synth is made by `MiniSynthInstance` (vtable 0x1800641e8) slot 1, `0x180028cb0` |
| Dispatcher | 0x1800029b0 | 4: sample rate to synth `+0x50`; 12 (`FPD_SetPlaying`): playing flag `+0xc58` |
| SaveRestoreState | 0x180002f10 | `SSLF`, a u32 length, the synth's chunk to synth `+0x128` |
| ProcessParam | 0x180003420 | set: `int / 256`, clamped to 0..1, to synth `+0xb8`; get: `int(256 v)` |
| slot fill | 0x180003dd0 | per-key gains, pitch, cutoff and resonance from FL's `FinalLevels` |
| Gen_Render | 0x180004780 -> 0x180004100 | asks FL the song position in beats (`FHD_GetMixingTime`) while playing, renders, adds |
| TriggerVoice | 0x1800048f0 | key `int(Pitch / 100)`, byte `int(127 ln(1 + 10 Vol) / ln 21)`; a key with an older voice gets a note off first |
| Voice_Release | 0x180004a10 | the synth's note off for the key |

`MiniSynthInstance`'s config (`FUN_180001230`): 43 parameters, polyphony
99. The synth's vtable is at `0x180063848`: note on `+0x130`
(`0x180022950`), note off `+0x138` (`0x180022970`), set parameter `+0xb8`
(`0x180023c20`), get `+0xd8` (`0x180023c00`), render `+0x88`
(`0x180025a40`), sample rate `+0x50` (`0x1800248c0`), load `+0x120`
(`0x180025e70`).

`CPluginRenderer` (`0x1800078c0`) keeps the velocity byte over 127 per key
at `+0x1068` (`0x1800077a0`: a byte of 0 is a note off) and five per-key
tables the slot fill writes: left gain `+0x20`, right gain `+0x220`, pitch
in semitones `+0x420`, cutoff in octaves `+0x620`, resonance `+0x820`.
The fill takes `ComputeLRVol(Final.Pan, Final.Vol)`, `Final.Pitch / 100 -
key + 60`, `Final.FCut` and `Final.FRes` of the key's voice. The Rust takes
these as the circular pan law of the note's pan and velocity gain, its fine
pitch over 100 and its mod X and Y, as `drumpad.rs` does; the binary does
not say what FL puts in `FinalLevels`.

## 1. State (event 213)

`SSLF`, u32 length, then the chunk (`0x180025e70`; `0x180007700` first
skips a 60-byte `CcnK`/`FPCh` VST header):

| offset | what |
|---|---|
| 0 | `99 46 4C 53 0D 0A 1A 0A` |
| 8 | `HEAD`, u32 `2 n + 6`, u16 1, u16 1, u16 `n`, `n` UTF-16 units of the preset name |
| `0x16 + 2n` | `DATA`, u32 length `L`, `0rev`, then f32 parameters |

Parameter `i` is read while `L - 4 (i + 1) > 3`, so 176 bytes give all 43.
Each goes through set parameter, which clamps it to 0..1.

## 2. Parameters

`FUN_180023c20` stores the clamped float at `+0x4018 + 4i` and derives:

| # | name | default | derived |
|---|---|---:|---|
| 0 | Voices Mode | 0.375 | `int(2v)`: 0 POLY (`+0x39cc` 1), 1 MONO, 2 LEAD (mono legato, `+0x39cd` 1) |
| 1 | Transpose | 0.5 | `+0x39c8 = int(24 (v - 0.5))` |
| 2 | Slide Time | 0 | `+0x39d0 = 0.9 ((1-v)^2)^2 + 0.01`, the glide per block |
| 3 | MW Dest | 0 | `int(v)`: 0 Filter, 1 LFO amount |
| 4 | MW Amount | 0 | `v` |
| 5 | OSC Kind | 0 | `int(9v)`: SAW, SAW+SAW, PULSE, SQR+SAW, SQR+SQR, SUPERSAW, BELL, DEAF SAW, SAW SPREAD, SAW SPREAD 2 |
| 6 | OSC Mod | 0 | `v` |
| 7 | OSC Noise | 0 | `v^2` |
| 8 | FLT Freq | 1 | `v` |
| 9 | FLT Peak | 0 | `0.99 v` |
| 10, 11 | FLT Kbd, FLT Vel | 0 | `v` |
| 12, 13, 15 | EGF Atk, Dec, Rel | 0, 1, 1 | `rate(v, 500)`, `rate(v, 50)`, `rate(v, 50)` |
| 14 | EGF Sus | 1 | `v` |
| 16 | EGF Amnt | 0.5 | `2v - 1` |
| 17 | LFO Shape | 0 | `int(2v)`: SINE, SAW, SQUARE |
| 18 | LFO Dest | 0 | `int(2v)`: FLT, FRQ, OSCMOD |
| 19 | LFO Rate | 0.25 | `int(11v)` into 1/4, 3/8, 1/2, 3/4, 1, 2, 4, 8, 16, 64, 128, 256 beats a turn |
| 20 | LFO Amnt | 0.5 | `2v - 1` |
| 21, 22 | AMP Atk, Dec | 0, 1 | `rate(v, 500)`, `rate(v, 50)` |
| 23 | AMP Sus | 1 | `v` |
| 24 | AMP Rel | 0.07 | `10 rate(0.95 v + 0.05, 50)` |
| 25 | DIST | 0 | on when not 0 |
| 26, 27 | Overdrive, Decimator | 0 | read raw by the effect |
| 28 | DELAY | 0 | on when not 0; a change clears the line |
| 29 | DELAY Time | 0.7 | `(int(3v) + 1) / 4` beats |
| 30 to 32 | DELAY Fdbk, Filter, Mix | 0.5, 1, 0.5 | raw |
| 33 | CHORUS | 0 | on when not 0 |
| 34 to 36 | CHORUS ModRate, ModDepth, Mix | 0.25, 0.5, 0.5 | raw |
| 37 | PHASER | 0 | on when not 0 |
| 38 | PHASER ModSpeed | 0.1122 | as LFO Rate, as f32 |
| 39 to 42 | PHASER ModWidth, Fdbk, Mix, Freq | 0.4, 0.1, 1, 0.5 | raw |

The defaults are `FUN_1800234b0`. `rate(v, k)` is `FUN_1800229c0`: for `v
>= 0.001`, `x = 0.95 v + 0.05`, `d = x^4 k`, and `0.0002 / d` when `d >=
0.001`; else 1. It is a step per sample and does not know the rate.

## 3. Notes and voices

99 voices of `0x220` bytes (`0x180021070`), all made at once. A note goes
`0x180022950` -> `FUN_180024520` (key, velocity 0..1):

- The key's velocity is kept at `+0x3ae4`.
- POLY takes the last free voice, else the one with the smallest count
  (`+0x48`, ties to the later). MONO and LEAD take voice 0.
- The count `+0x3ce4` goes up and the voice gets it. Its glide `+0x4c`
  starts at the last key played (`+0x3ce8`); in LEAD with no other key held
  it starts at the key.
- `0x180021330`: the voice's key and velocity; unless LEAD finds it not
  released, the filter envelope restarts from 0 towards 4 at EGF Atk and the
  amplitude envelope heads for 4 at AMP Atk from where it is.

A note off (`FUN_180024520`, velocity 0): every voice on the key is
released (`0x1800213a0`: both envelopes head for 0 at their Rel), but in
LEAD, where the voice moves to the highest held key below, else the lowest
above, at that key's velocity.

A voice frees itself (`0x180021260`) once released, its amplitude envelope
under 1e-5 heading for 0 and both its gains under 1e-5. Freeing resets its
ladder and envelopes and draws its eight phases from the global seed,
`s = s * 0xb464b2f9 + 0x9d6f2492` run 128 times for each.

## 4. The render (`0x180025a40`)

In blocks of 32 frames from the start of FL's call:

1. The LFO (`0x1800249b0`): shape 0 `sin(2 pi phase)`, 1 `2 (1 - phase) -
   1`, 2 `1` below half a turn and `-1` from it; then times `a^2` and the
   sign of `a` for the amount `a`, unchanged past -1 or 1. Its phase is
   then `frac(beats / rate)` while FL plays (`+0x39b8`), else it moves by
   `tempo / 60 / rate` a second.
2. The song position moves on by the block.
3. Every voice with a key: its block (`0x180022250`), then its samples
   (`0x180022440`).
4. Frames since a voice sounded (`+0x40c4`) count up, reset by any voice
   and by the delay. While under half a second, and from the first voice
   after a new rate, the effects run in order: distortion, delay, phaser,
   chorus.

### A voice's block (`0x180022250`)

- Gain targets `velocity * slot left/right`, with the velocity taken as 0
  once the amplitude envelope is under 1e-5 heading for 0. Gains start at
  their target.
- Drift: the seed steps `s = s * 0xbb38435 + 0x3619636b`; `drift += (r -
  drift) 0.01` with `r = s_old / 2^31 - 1`.
- Glide `+= (key - glide) * slide`. Pitch `= glide + transpose + 0.08
  drift (+ 12 lfo on FRQ) + slot semitones`, `f = 220 * 2^((pitch - 57) /
  12)`.
- Oscillators (`0x180021d10`): `m = OSC Mod (+ 12 lfo on OSCMOD)`, `r =
  2^(int(24 m - 12) / 12 + 0.00625)`. Up to eight oscillators of a phase,
  a step and a gain:

  | kind | oscillators |
  |---|---|
  | SAW | 3: `f`, `f`, `f` at gains 1, -1, 1; phase 2 = phase 0, phase 1 = phase 0 + `(1 - abs m) 2^32`; just one when that is 0 |
  | SAW+SAW | `f` and `f r`, gains 1 |
  | PULSE | `f`, `f`, gains 1, -1, phase 1 as SAW's |
  | SQR+SAW | `f`, `f`, `f r`, gains 1, -1, 1, phase 1 = phase 0 + 0x7fffffff |
  | SQR+SQR | `f`, `f`, `f r`, `f r`, gains 1, -1, 1, -1, phases 1 and 3 = 0 and 2 - 0x70000001 |
  | SUPERSAW | 8 at `((0.0125 j - 0.05) m + 1) f`, gains 0.5 |
  | BELL | 6 at `f`, gains 0.5, phase `j` = phase 0 + `j abs(m / 2) 2^32` |
  | DEAF SAW | `f`, `f`, gains 1, phase 1 as SAW's |
  | SAW SPREAD (2) | `(1 - m/100) f / 2` (`* 1.5`), `f`, `2f (1 + m/100)`, gains 1 |

  Steps are `int(f / rate * (2^32 - 1))`. The table is picked by the first
  oscillator: `1023 - int(rate / 2f)`, clamped, or 0 for `f` under
  `rate / 2048`, and every oscillator reads it.
- Filter (`0x1800213e0`): octaves `= 14 FLTVel (vel - 0.5) + FLTKbd (pitch
  - 48) / 12 + 4 + 0.103 drift + 10.3 FLTFreq (+ 4 lfo on FLT) (+ 4 MW on
  Filter)`; `base = 2^(slot cutoff + octaves)`, `fc = min(base + (base
  2^(10 EGFAmnt) - base) * filterEnv, 20000)`, `g = clamp(6.25 fc / (4096
  rate), 0, 1)`, `k = -4.05 clamp(slot resonance + FLTPeak, 0, 1)`. One step
  of four one-pole stages, `s0' = s0 + g (x + k s3 - s0)`, `s1' = s1 + g
  (s0' - s1)` and so on, is written as a 4 by 5 matrix with the input as
  the fifth column and squared twelve times: 4096 steps a sample.

### A voice's samples (`0x180022440`)

Per sample: the seed steps once; each oscillator reads its table at `(phase
+ (s & 0x3fffff)) >> 21` (the low bits dither the read) and steps;
`x = ((noise OSCNoise + sum) 0.2 + 0.07) ampEnv` with `noise = 2 s_old /
2^32 - 1`. The ladder matrix takes `x`; the first stage is then bent as
`v / (v^2 + 1)`. Out `y = 5 s3`, less a DC follower `dc += (y - dc) 0.005`.
Each envelope steps `level += (target - level) step`; past 1 it is set to 1
and heads for its Sus at its Dec. The gains follow their targets by 0.005
a sample; `L += gainL y`, `R += gainR y`.

### Effects

- Distortion (`0x180025150`), with DIST on: `D = 20 Overdrive^2`; `x >= 0`
  goes to `1 - 10^(-D x) (1 - x)`, below to `10^(D x) (x + 1) - 1`, times
  `0.7 (1 - D/16)^8 + 0.3`. Decimator: a sample is held for `int(64 clamp
  (Dec^2))` frames.
- Delay (`0x1800257c0`): 1.5 s of `L + R + echo`. Length `(len - 1) d` with
  `d = time 40 / tempo` halved while over 1. Per sample `echo += ((fb
  line + 1e-30) - echo) (0.99 filter + 0.01)`, `line = L + R + echo`, both
  sides get `echo mix^2`.
- Phaser (`0x180024b50`): six first-order allpasses a side, `v = z - a u;
  z = a v + u`, with `a = (1 - w) / (1 + w)`, `w = clamp((c + (1 +- cos)
  width) (hi - lo) + lo, 1e-5, 0.499)`, `lo = 50 / rate`, `hi = 20000 /
  rate`, `width = ModWidth^3 / 2`, the cosine of `2 pi frac(beats / rate)`
  after the block, and `c` gliding to `Freq^2` by 0.001 a sample. The last
  stage feeds back by `0.98 Fdbk^(1/3)` and is added at `Mix`.
- Chorus (`0x1800253f0`): 10 ms of `L + R`. `depth = 0.36 ModDepth +
  0.04`; the left tap reads `len depth s^2` on from the write point with `s
  = depth sin(2 pi p1) + 0.5`, the right `s` itself (so next to the oldest
  sample), each between two points; `p1` and `p2` start at 0.62 and 0.1 and
  turn at `4 ModRate^2 + 0.2` and that plus 0.05 Hz. Each side gets its tap
  times `Mix^2`.

### The tables (`0x180022a30`)

`oscdata.dat` beside the DLL is 1024 f32 amplitudes then 1024 f32 phases.
Harmonic `h` is added as `amp cos(2 pi h j / 2048 + phase)` (SVML's
vectorised cosine) to a running 2048-point sum, which is copied to table
`1024 - h`: table 1023 is the first harmonic alone, table 0 all 1024. The
8 MB of tables are built once per synth.

## 5. What `minisynth.rs` builds

All of the above, sample for sample as read, with these differences:

- The two seeds are FL's one global, which every MiniSynth and every voice
  FL frees steps, from whenever FL loaded the DLL. Each Rust instance
  starts from 0 and runs its own: the drift, the dither and the noise are
  the same process but not the same numbers, and new phases differ.
- The song position is counted from the start of the song as the host
  plays it, at its tempo; FL reads its own. The LFO and phaser follow it
  as while FL plays.
- The slots are the guessed `FinalLevels` above; the mod wheel and pitch
  bend stay at 0, as FL sends neither to this plugin.
- The ladder's twelve squarings sum in another order, and `pow`, `sin` and
  `cos` are Rust's.
- The tables are shared between instances reading the same file.
