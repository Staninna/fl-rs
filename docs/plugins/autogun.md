# Autogun: state, parameters, signal architecture

Recovered from `Autogun_x64.dll` (FL Studio 2026, Delphi native plugin,
image base 0x400000, 6.0 MB). The DLL wraps no other plugin. It ships no
patches either: `Data/Random pad/` holds the word lists and the base state
(`Default.dat`, `Default_C.dat`) that "Generate random preset" starts from,
`Data/Maps/` three bin shapes for the curve editor, and `Data/SG.dat` the
Maximus state behind "magic dust". The sound of a preset is computed from
two seed numbers in its state, so a state is all there is to read. The
vector maths is Intel IPP from `Shared/dsp_ippv2_x64.dll`, bound by name in
`FUN_005bd050`.

Ghidra project `/shared/code/fl-studio-target/ghidra/proj`, project name
`autogun`. Decompiles under `ghidra/out/autogun/`: `f/` holds one file per
function for 0x783000..0x790000 (the shared curve and articulator classes),
0x830000..0x850000 (effects and the timbre engine) and 0x850000..0x8a3200
(the plugin); `r1`..`r8` the runtime helpers and FL kernels; `d1` the
disassembly of the functions whose IPP arguments Ghidra drops. The scripts
and parsers used while mapping are in the session scratch directory and not
in the repository; the state layout below is all a parser needs.

Corpus: 9 states in 9 of the 165 projects (the render test, versions 18 x2
and 19 x7), and the plugin-database preset `Autogun.fst` (version 18) in
three places. All of them parse to the last byte with the layout in
section 1, all at quality 1.

Confidence tags: `code` (read in the decompile or disassembly), `dfm` (the
editor form), `corpus`.

## How the plugin is wired

`CreatePlugInstance` @ `0x0087a300` -> constructor `FUN_0087edd0`, class
VMT `0x0087a140`.

| slot | SDK name | address |
|---|---|---|
| 0 | destructor | 0x0087fb20 |
| 1 | Dispatcher | 0x0087a690 |
| 3 | SaveRestoreState | 0x0087cb90 |
| 4 | GetName | 0x00881c50 |
| 5 | ProcessEvent | 0x0087fdf0 |
| 6 | ProcessParam | 0x00880670 (index decoder `FUN_00881a70`) |
| 8 | Gen_Render | 0x00885600 |
| 9 | TriggerVoice | 0x00882a90 |
| 10 | Voice_Release | 0x00884450 |
| 11 | Voice_Kill | 0x008845d0 |
| 20 | (TDelphiFruityPlug) | 0x0087e640 |

Objects: 13 articulators at plugin+0x2c0 (class VMT `0x00879e38`,
constructor `FUN_0087a340`: `TArticulator` with two extra maps), 4 curves
at plugin+0x328.. (`TMMap`, VMT `0x00783108`), the timbre engine at
plugin+0x230 (`FUN_00844bf0`), chorus at +0x348 (`TStereoFlanger`), delay
at +0x360 (`TDelay_32FS`), reverb at +0x378 (`TFruityReverb`), the
embedded Soundgoodizer host at +0x550 (`SG_x64.dll` with `SG.dat`).

Runtime constants written at DLL init (`FUN_0062f6d0`): 0x9114f0 ln 2,
0x911500 ln2/2, 0x911508 ln2/1200, 0x911510 ln2/12, 0x91152c ln 3,
0x911524 ln 11, 0x91153c ln10/20, 0x9114e4 sqrt 2 (`1 / cos(pi/4)`);
0x9112c8 ln 0.01 (`FUN_00606f40`); 0x914038 = `440 * 2^(75/12)` =
33488.07 as an f32 (`FUN_00847470`).

## 1. State layout (event 213)

`SaveRestoreState` (`FUN_0087cb90`, code):

```
save:    Write(u32 0x00130003)                       // version 19 high word
         zlib stream (FUN_00872250):
           0x100 bytes of plugin+0x12c                // the block below
           3 bytes at +0x358                          // delay tempo, delay on, spare
           3 x i32 at +0x388 + 0x80 b                 // EQ band types
           wide string (FUN_0077cdd0)                 // info text
           i32 at +0x610                              // random-preset seed
           13 articulators, 4 curves (vtable+0x10)
restore: v = Read(u32); v == 0x7fffffff or v >= 0x130004: ignore
         v > 0xffff: v >>= 16
         block: 0xc bytes (8 before v8), then 0xf4 bytes less what each older version lacks
         v < 18: string as i32 n + n ansi bytes; v > 18: the seed
         v < 4, v < 10, v < 5, v < 6: articulators and curves rebuilt (not read here)
```

`autogun.rs` reads versions 0 to 19. The block is 0x100 bytes from 13 on.
Older versions save a shorter block and leave out some articulators and
curves; [ogun.md](ogun.md) section 1 lists what each one lacks. Before
version 14 `autogun.rs` ignores the 1/sqrt weighting byte (+0x226).

### The block (plugin+0x12c, 0x100 bytes)

| offset | field | values | corpus |
|---|---|---|---|
| 0x00 | REC 0..48, i32 each | section 2 | |
| 0xc4 | +0x1f0 filter type | 0 low, 1 band, 2 notch, 3 high pass | 1 (render test), 0 |
| 0xc8 | +0x1f4 filter stages | 0 off, 1..3 | 3, 2, 1, 0 |
| 0xcc | +0x1f8 alternate resonance | 0 / -1 | -1 (render test), 0 |
| 0xd0 | +0x1fc richness + 5 | the table is `2^order` long | 15, 16 |
| 0xd4 | +0x200, +0x201 bytes | reverb on, reverb predelay tempo-synced | 1, 0 |
| 0xd8 | +0x204 quality | 0..4 | 1 everywhere |
| 0xdc | +0x208..+0x213 bytes | mod smoothing, kill same key, link velocity, keyboard tracking of pitch, key to seed 1, velocity to envelope, declick, release on volume envelope, -, -, portamento mode, log curve axis | |
| 0xe8 | +0x214 4 x i32 | harmonic comb of each curve | 3 on the decay EQ in the render test |
| 0xf8 | +0x224..+0x227 bytes | "to target" decay, normalise to curves, 1/sqrt weighting, spare | |
| 0xfc | +0x228 magic dust | 0..1000, default 430 | 430 |

### Articulators and curves

Each articulator (`FUN_0087a4d0` -> `FUN_0078d590` -> `FUN_0078ce90`):
`u32 format (3); f32 base, envelope amount, LFO amount;` a `TMEnvelope`, a
`TMLFO`, then the keyboard, velocity, mod X and mod Y maps (`TMMap`). The
curve grammar is the shared one of `menvelope.rs`. A curve is a bare
`TMMap` stream. All 9 corpus states: base 0, amounts 1, no LFO active.

Articulator order (the editor's `ArtSelect`): 0 volume, 1 filter
frequency, 2 filter resonance, 3 pitch, 4 unison pitch, 5 unison phase,
6 timbre pre-decay, 7 decay time, 8 release time, 9 fullness, 10 morphing,
11 rotation, 12 EQ shifting. Bipolar ones (`DAT_0087edc8` = 0x1a0e: 1, 2,
3, 9, 11, 12) add and centre their maps and envelope on 0.5; the rest
multiply (`FUN_0087ed10`, `FUN_0078d3d0`). A map reads as its value less
its centre (`FUN_00788c20`). Only 0..4 run an envelope and LFO per voice.

Curves: 0 forced harmonics, 1 impulse EQ, 2 decay EQ, 3 release EQ. The
constructor makes them flat at 0, 0.5, 0.5, 0.5 (`FUN_0078cc00`).

## 2. Parameters (`ProcessParam` @ 0x00880670)

Tag = REC index (dfm). Ranges and defaults are in `autogun.rs` `RANGES` and
`DEFAULTS`. Index 49 and up are the articulator envelope and LFO knobs.

| REC | control | derived value (code) |
|---|---|---|
| 0 | master level 0..256 | +0x5cc = `level3(v/128) / 4`, `level3(x) = (3^x - 1)/2` |
| 1, 2 | coarse, fine pitch | +0x5d0 = `100 coarse + fine` cents |
| 3, 4 | mod X, Y 0..512 | map places, smoothed per tick when +0x208 |
| 5 | pre-decay 0..1000 | +0x24c = v/1000 |
| 6 | decay 0..10000 | +0x250 = v/10000, 10000 = none (-1) |
| 7 | release 0..10000 | +0x254 = v/10000 |
| 8 | fullness -1000..1000 | +0x248 = v/1000 |
| 9 | morphing / randomness 0..1000 | +0x258 = v/1000 |
| 10, 11 | timbre seed 1, 2, 0..9999 | +0x23c, +0x240 = `mix(v - 0x20cf, 0x1cfc)`, or -1 at 0 |
| 12..16 | unison order 1..9, pan, volume, pitch, phase | read at the note |
| 17, 18 | filter frequency, resonance | +0x530, +0x534 = v/256 |
| 19..25 | chorus order, depth, speed, delay, spread, cross, mix | `FUN_00882460` |
| 26..30 | delay feedback, time, stereo offset, level, mode | `FUN_00881f60` |
| 31..39 | reverb low cut, high cut, predelay, room, diffusion, decay, damping, colour, wet | `FUN_00882230` |
| 40..48 | EQ level x3 (cB), frequency x3, width x3 | `FUN_00882820` |

`mix(x, k)` is a Jenkins mix: `a = x; b = -a - k ^ a << 8; c = 0x1eef - a -
b ^ b >> 13`, then the usual eight rounds, and the absolute value of the
last word. TriggerVoice uses `k = 0x1e42` on `REC10 + key - 0x1f88` when
"key to seed" is on, and `k = 0x1dbc` on `counter - 0x200e` for a seed 2 of
0, counting the plugin's counter (+0x244) up a note at a time. The
constructor starts the counter at `RandSeed` right after `Randomize`
(`0x0087eeb6` calls `0x00409fb0`, which seeds from `QueryPerformanceCounter`
and `Random(0x7fffffff)`; `0x0087eec9` reads `RandSeed` at `0x008a4010`), so
it is the clock's, and FL's own render of such a note is different on every
run and in every instance.

Quality (`FUN_0087e750`, the +0x204 select) sets the interpolator (+0x268),
the table's oversampling bits (+0x260, at least 1 when the richness is
under 5 and the rate under 88.2 kHz), the frame in ms (+0x25c), the control
tick base (+0x264) and an output oversampling filter (+0x5b8): 0 `(1, 0,
50, 96, off)`, 1 `(2, 0, 50, 96, off)`, 2 `(2, 1, 50, 48, on)`, 3 `(2, 2,
25, 24, on)`, 4 `(6, 2, 25, 8, on)`.

Rates (`FUN_0087c6d0`, `FUN_0087e420`): frame = `round(ms * 0.001 * SR)`;
tick = `4 round(base * SR / 44100 / 4)` (104 at 48 kHz); declick =
`round(96 / (44100/SR)^2)`; articulator rates `2 tick / SR` half seconds
and `tick / SR * tempo / 60` beats a tick.

## 3. The timbre engine (plugin+0x230, voice+0xe50)

Sizes (`FUN_00844d60`): order `o` = +0x1fc, bits `b`; table `N = 2^(o+b)`,
partials `n = 2^(o-1) - 1` in Pack slots `2i+1, 2i+2` (partial `i+1`). The
table's step at C5 (+0x7c) is `33488.07 * 2^b / SR`, so one table is one
period at `33488.07 / 2^o` Hz: 0.511 Hz at order 16. A drawn bin every
1/64 of the curve is a harmonic every 512 partials, 261.6 Hz, FL's C5.

Random phases (`FUN_00844f90`): `ippsRandUniform_Direct_32f` of `n` numbers
in `[0, 2 pi)` with seed 3, each `(cos, sin) / sqrt(n)`.

`ippsRandUniform_Direct_32f` (export 338 of `dsp_ippv2_x64.dll`, generic
path `0x1815ed2e0`, code): a subtract-with-borrow generator over the seed
and two LCG steps of it (`69069 x + 0x3c6ef373`) plus a fixed LCG from
0x436cbae9, two i32 a loop, scaled by `(high - low) * 2^-32` about
`(low + high) / 2`; the seed comes back as the last state.
`autogun_engine.rs` ports it register for register.

Curves (`FUN_0087e900`, `FUN_00886960`, `FUN_00845210`): each curve is read
at partial `i` at `x = i / 32767` (or `log2(i+1) / 15` with +0x213), plus
2^-24, for `n` places (forced) or `min(1.5 n, 32768)` (the rest). Forced
harmonics: clamped 0..1, stored as `1 - f`, and absent when the curve is
flat at 0. The others: clamped 0..1, then 1 at 0.5 and `(9^v - 1)/2`
elsewhere, times `1/sqrt(i+1)` with +0x226, then the comb (+0x214): comb
`c` of 1..4 sets every `(c+1)`th place with `i mod (c+1) = c`, 5..7 every
place with `i mod (c-2) > 0`, to 1 on the decay and release EQ and to 0 on
the others (`DAT_00886d58` = 0x180). Repeats past `n` are trimmed. The
impulse curve (or with "to target" decay, where impulse and decay differ)
gives the first and last partial that sound (+0x24, +0x28). With +0x225
the level (+0x88) is `0.5 / rms`, the RMS of the impulse curve over `n`
(and of the decay curve with "to target"), else 1.

Per voice (`FUN_00845790`, arguments from TriggerVoice): frame length,
pre-decay `p = +0x24c * art6`, decay `D = ((21^+0x250 - 1) 997.5 art7 + 50)
ms` (none at -1), fullness `f = clamp(+0x248 + 2 art9, -1, 1)` turned into
`F = -sign(f) (10000001^|f| - 1) / 100`, the two seeds, morph `m = +0x258
art10`, EQ shift `round(clamp(art12, -0.5, 0.5) n)`, rotation
`round(art11 n + 256 n) mod n`, the pitch ratio and the unison copies.

Levels (`FUN_00845f30`, code with the IPP arguments from `d1/`):

1. Uniform numbers from seed 2, rotated up by the rotation
   (`FUN_00845e40`), shaped by `F` (`FUN_00845d20`: `(F+1)^x - 1` above 0,
   `(1-F) - (1-F)^(1-x)` below), times `m / |F|`: the base levels `B`.
2. With `m < 1`, seed 1 the same way times `(1-m) / |F|` added, or `1 - m`
   when seed 1 is -1. Negative levels are 0.
3. Forced harmonics: `B = B (1-f) + f`.
4. Start levels `A = 0.01^(1 / (B / max(p, 0.001) + 0.001))`, put at RMS
   +0x88.
5. Decay per frame: `0.01^(1 / (D' B E_decay + 25 ms))` with `D'` the decay
   less 25 ms in frames, `E_decay` the decay curve moved by the EQ shift. No
   decay: 1. "To target": the spectrum goes to `A E_decay` by `1 -
   0.01^(1/D')` of the way a frame.
6. Band limit: partials up to `n 2^b 0.907 / step`.
7. Spectrum: `A E_impulse` times the random phases.

A frame (`FUN_00845bb0`): the newer table becomes the older, the inverse
FFT (`ippsFFTInv_PackToR_32f`, no scaling) of the spectrum is the newer,
the two are interleaved with 64 frames of guard, and the spectrum takes a
frame of decay. The first render (`FUN_00845ad0`) makes two frames and
starts every unison copy at its phase share of the table plus the place
where the newer table is nearest 0.

Release (`FUN_00847080`, from `FUN_00884200`): the release EQ in place of
the decay EQ, over `((21^+0x254 - 1) 997.5 art8 + 50) ms`; a frame under
way restarts from what plays.

Render (`FUN_008471f0`): every copy reads the (newer, older) pairs at its
own phase with FL's 6-point, 5th-order interpolator (`FUN_00611420`,
coefficients `0x8d5ce8..0x8d5d2c`) and crossfades linearly from the older
to the newer over the frame, panned (`FUN_00846f90`, `FUN_00847160`). A
voice whose output stays under -80 dB for 100 ms ends.

## 4. The voice (TriggerVoice `0x00882a90`, render `FUN_00884890`)

At the note: each articulator's keyboard map at `key / 120` and velocity
map (`FUN_0078d5f0`); 5..12 also take their mod X and Y maps. Pitch: the
value times 4 (less the key when keyboard tracking is off), so the pitch is
`FinalLevels.Pitch + +0x5d0 + 1200 art3` cents. Unison (order `u`): copy
`i` sits at `2 i/(u-1) - 1`, gains 0.3 (u <= 2) or `0.3 (r v + 2 r (1 -
|x|)(1 - v))` with `r = 1/sqrt(u)`, `v` the unison volume, panned by FL's
law at `x * pan/128`; its detune share is `|x| DAT_008ff730[i]` with the
sign alternating, in cents `(11^(pitch * art4 / 256) - 1) * 5`; its start
phase `(i/u - (u-1)/(2u)) * IntPower(1 - (101^(1 - phase/128 art5) - 1) /
100, order)`.

Every tick: the mod places, the five running articulators (value op
envelope op LFO, `FUN_0087a530`), the gains from FL's pan law with the
cross terms (`FUN_00617ae0`) at `FinalLevels.Vol * +0x5cc * art0`, ramped
no faster than `0.001 * 44100 / SR` a sample (`FUN_0062cc10`).

Every chunk: the timbre, then the declick (a one-pole whose coefficient
rises from 0 to 1 over the declick length, +0x20e), then the filter:
`FUN_0087fe50` designs 1 to 3 equal biquad stages (`FUN_006264d0`, RBJ
forms, run in f64 by `FUN_00626ec0`) and, under 88.2 kHz, runs them at
twice the rate on a sample-and-hold copy, averaging pairs back down.
Low and high pass: cutoff `(1501^c - 1) 6.6467e-4` (low) or `3.9993e-4`
capped at 0.6001 (high), plus 0.003 or 0.0001, times 44100 Hz; Q `(2001^r -
1) 0.02495 + 0.1` (0.1 at 0, 50 at 1), rooted over the stages; the
alternate switch scales the resonance by `(c + 1)/2`. Band pass and notch:
cutoff `(1501^c - 1) 3.9993e-4` capped at 1.0001, bandwidth `(101^r - 1)
0.1199 + 0.01` octaves times the stages (divided for the notch).

## 5. Effects (`Gen_Render` @ 0x00885600)

Order: EQ, chorus, delay, reverb, oversampling filter (quality 2+), magic
dust, then added to the host's buffer.

- EQ (`FUN_00882820`, `FUN_00885550`): three bands, type 6 low shelf, 7
  peak, 8 high shelf; frequency `(101^(v/65536) - 1) 159.9 + 10` Hz, gain
  `v/100` dB, width `v/65536` (peak Q `(1001^w - 1) 0.0999 + 0.1`, shelf
  slope `1.9 w + 0.1`). A band at 0 dB runs for SR/5 more samples and stops.
  Every corpus band is at 0 dB.
- Chorus (`FUN_00882460`): the six wheels straight into `TStereoFlanger`;
  mix `m = |v|/128`, dry `1 - (5^m - 1)/4`, wet the same at `1 - m` with the
  wheel's sign. It and the delay sleep `SR*100/44100` plus the chorus ring
  after the last voice.
- Delay (`FUN_00881f60`): time `v SR / 384` or tempo-synced `v SR 0.3125 /
  tempo`; stereo offset `(1001^(|v|/512) - 1) * 50` in units of 10 us on the
  left (right when positive); feedback `v/128`; level `(11^|v| - 1)/10`
  signed, on the output; mode normal, inverted (sides swapped) or
  ping-pong.
- Reverb (`FUN_00882230`): `TFruityReverb` with low cut (19 is off), high
  cut and damping (221 is off), predelay `v SR / 384` (or tempo-synced),
  decay `v SR / 10`, colour; fed `L + R`, added at `v/256`.
- Magic dust: the embedded Soundgoodizer with `SG.dat` (a Maximus version 7
  state) and its parameter 5 at +0x228, when above 0.

## 6. What `autogun.rs` builds

Exact as read: the state and every corpus state to the last byte, the
parameter maps, the seeds and their mix, IPP's uniform generator (generic
path), the curve baking, the spectrum maths, the frame and crossfade
scheme, the interpolator, unison, the articulators' combination, the gains,
ramps and pan law, declick, the filter design and its doubled rate, the EQ,
and the post chain on `fl_fx.rs` and `maximus.rs`.

Approximated: nothing has been null-tested against FL. IPP's `exp` (A11 and
A21) is plain `exp`, and the CPU-specific random paths are taken to match
the generic one. The envelope runner is `harmor_art.rs`'s port of the same
shared class. A seed 2 of 0 draws from a counter that starts at 0 here; FL
starts it from the clock when the plugin is made, so FL itself has no one
sound for such a state to match. Quality 0 reads with FL's linear interpolator
(`FUN_00610ef0`). Qualities 2, 3 and 4 keep the default's 6-point
interpolator and leave out the output oversampling filter. Portamento (colour 14 notes),
velocity-to-envelope mode 2 and the host's polyphony limit are left out;
16 voices, the oldest taken first.
