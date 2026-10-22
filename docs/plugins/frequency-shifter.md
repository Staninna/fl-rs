# Frequency Shifter: state, parameters, signal chain

Recovered from `Frequency Shifter_x64.dll` (FL Studio 26.1.7, Delphi shell,
image base 0x400000) and the `Engine_x64.dll` beside it (MSVC, image base
0x180000000, one export `dispatch`), with Ghidra decompiles checked against
the listings where the decompiler folds the SIMD lanes or drops the
arguments of a CRT call. Rebuilt in `crates/plugins/src/frequency_shifter.rs`.

Corpus: 5 states, all 52 bytes of version 4 and all the HQ shifter: Astes -
Bien Duro (two: -60480 in the 200 Hz range and 49504 in the 20 kHz range),
Cuebur & Thandi Draai - Africa's Calling (40320, 200 Hz), LollieVox & Lezina
J - I'll Be Your Madonna (mix 0) and the render test (70000 in the tempo
range, left shape 0). No shipped state uses the ring modulator or a
sidechain.

## The shell

`CreatePlugInstance` @ `0x6dc420` -> constructor `0x6dd270` with the class
reference `0x6dc100` (VMT `0x6dc1c8`). Dispatcher `0x6dc4c0`,
SaveRestoreState `0x6dd080`, ProcessEvent `0x6dc960`, ProcessParam
`0x6dd650`, Eff_Render `0x6ddbb0`.

### State (`0x6dd080`)

| bytes | type | meaning |
|---|---|---|
| 0..3 | i32 | version, 4; a state of 5 or more is left alone |
| 4..43 | i32[10] | the parameters, kept at `shell+0x140` |
| 44..47 | i32 | bit 0: a window toggle (`editor+0x748+0x48e`) |
| 48..51 | i32 | the sidechain input, `shell+0x168` |

Every version under 5 reads the same 52 bytes. After loading, the shell runs
ProcessParam on every parameter with flags 2 then 0x11, which hands each to
the engine. The constructor does the same with the wheels' DFM values, which
are their defaults.

### Parameters

The DFM (`TSynthEditorForm`) gives the wheels, each wheel's `Tag` its index:

| idx | name (engine's) | wheel | range | default |
|---|---|---|---|---|
| 0 | Mix | MixWheel | 0..10000 | 10000 |
| 1 | Type | TypeSelector: FREQ. SHIFTER HQ, FREQ. SHIFTER, RING MOD | 0..2 | 1 |
| 2 | Frequency | FreqWheel | -70000..70000 | 0 |
| 3 | L/R Phase Offset | LRPhaseWheel | 0..10000 | 0 |
| 4 | L Shape | LShapeWheel | 0..10000 | 10000 |
| 5 | R Shape | RShapeWheel | 0..10000 | 10000 |
| 6 | Feedback | FeedbackWheel | 0..10000 | 5000 |
| 7 | Mono<->Stereo | StereoWheel | 0..10000 | 10000 |
| 8 | Range | RangeSelector: 20 kHz, 200 Hz, TEMPO | 0..2 | 1 |
| 9 | Left Phase Offset | StartPhaseWheel | 0..10000 | 5000 |

ProcessParam (`0x6dd650`) with flag 1 keeps the value and hands the engine
`clamp((value - min) * (1 / (max - min)), 0, 1)` in f64, as an f32, with
`dispatch(3, engine, index, 0, v)` (`0x6dd8f0`).

### Render (`0x6ddbb0`)

`dispatch(7)` (MXCSR | 0x8040, FTZ and DAZ, and the lock), then one
`dispatch(4, engine, &frame)` a frame and `dispatch(8)`. The frame is
`{L, R, sidechain L, R, i32 sidechain on, f64 place, f64 next place, f64
BPM}`:

- The sidechain is on when parameter 1 is 2 (ring mod), `shell+0x168 > 0`
  and the host (`vtable+0x120`) has a buffer for that input.
- The place is in beats, while the song plays (`FPD_SetPlaying`, dispatch
  12, sets `shell+0x18c`): `(f32)(frame) / tick_frames / (f32)PPQ`, all in
  f32, where `frame` is the host's song time in ms (`0x6ddb30`, host call
  0x24) times the rate times 0.001, rounded, plus the frame's place in the
  block, and `tick_frames = (f32)(rate * 15 / (BPM * (PPQ / steps a beat)))`
  (`0x6dc4c0` case 14, `FPD_SetTimeSig`; ProcessEvent sets the BPM). The
  next place is the same one frame on. Both are -1 when the song stops.

The silence counter at `shell+0x120/0x124` and the two peak meters feed the
display only: the engine runs every frame.

## The engine (`Engine_x64.dll`)

`dispatch` @ `0x180001000`: 0 makes the object (0x1de0 bytes,
`0x180001350`), 2 sets the rate (`0x180001c20`), 3 a parameter, 4 a frame
(`0x180002780`), 5 names a parameter, 7 and 8 bracket a block.

### Parameters (`0x1800023c0`) and glides (`0x180002560`)

Dispatch 3 stores `v` as the target of parameter `i` (`+8 + 4i`); the type,
the frequency and the range take at once, the others glide. Each frame,
before anything else, parameters 0, 4, 5, 6, 7, 3 and 9 in that order move
from where they are (`+0x30 + 4i`) by `(target - now) * k + now`, landing on
the target within 1e-5, and are worked out again:

| param | fields |
|---|---|
| 0 Mix | wet `+0x90 = clamp(2v, 0, 1)`, dry `+0x8c = clamp(2 - 2v, 0, 1)` |
| 1 Type | HQ `+0x84 = v < 0.6666 && v < 0.3333`, shifter `+0x88 = v < 0.6666` |
| 2 Frequency | target `+0xa4 = v` |
| 3 L/R phase | `+0xac = v * 0.25` |
| 4, 5 Shapes | `+0xa0, +0x9c = v`, `+0x98, +0x94 = 1 - v` |
| 6 Feedback | `+0xe4 = (1.005 v + 1.005 v) - 1.005` |
| 7 Stereo | `+0xb0 = (1 - v) * 0.5` |
| 8 Range | tempo `+0x85 = 0.6666 <= v` (a change sets the restart flag `+0x6c`), 200 Hz `+0x86 = 0.3333 <= v < 0.6666` |
| 9 Start phase | `+0x70 = v + 0.5` |

The constructor sets the engine's own defaults (1 for the mix, 0 for the
type, the L/R phase and the range, 0.5 for the rest) and the frequency's
glide to 0, the bottom of the wheel. The shell then hands it its wheels and
the state, so the first frames in FL glide from there. FL's mixer runs from
the moment a project is open, so by the time a song plays they have landed;
fl-rs lands them when it takes a state.

Rate (`0x180001c20`): `k = (f32)(1 - 0.09999999999999998^(200 / rate))` for
the parameters (`+0x58`) and `(f32)(1 - 0.0999..^(10 / rate))` for the
frequency (`+0x5c`, whose glide lands within 1e-4); `+0x68 = 499.99997 /
rate` (the HQ crossfade's step), `+0xd8, +0xe0 = 1 / rate`. It designs the
input filters: high-pass at 20 Hz, Q 0.5, at the rate; low-pass at 20 kHz,
Q 0.5, at the rate stored at `+0x1d60`/`+0x1da0`, which only the constructor
sets, to 44100.

### Filter design (`0x180003960`)

The cookbook in f64: `w = f * 2pi / rate`, `alpha = 0.5 / Q * sin w`;
low-pass `b = ((1 - cos)/2, 1 - cos, (1 - cos)/2)`, high-pass `((1 + cos)/2,
-(1 + cos), (1 + cos)/2)`, `a = (1 + alpha, -2 cos, 1 - alpha)`, all over
`a0` and kept as f32 with the feedback terms negated. A bank (`0x180003430`,
`0x180003dc0`) is eight sections of four lanes: lanes 0 and 1 a low-pass,
lanes 2 and 3 a high-pass, Qs 0.5098, 0.6013, 0.9000, 2.5629 (`0x180025568,
70, 78, 98`: an 8th-order Butterworth), sections 4 to 7 a copy of 0 to 3.
A section's sum, per lane: `(((x1 b1 + x b0) + b2 x2) + y1 a1) + a2 y2`. The
input filters' scalar sum: `((a2 y2 + a1 y1) + b2 x2) + (b0 x + b1 x1)`.

### A frame (`0x180002780`)

1. The place: on the first frame the song plays, the restart flag is set;
   while it plays the place and the next come from the shell; while not,
   the place moves on by `BPM / 60 / rate` a frame.
2. The glides; the frequency `+0xa8` glides to `+0xa4`.
3. Each side's input plus `tanhf(feedback * last wet output)` goes through
   the high-pass then the low-pass. The dry signal is the raw input.
4. The oscillator. Outside the tempo range, a restart sets the phase `+0xd0`
   to the start phase. The ring modulator's frequency is
   `powf(2, v * K) * 0.1` (0 at `v = 0`), the shifters' (`0x180001fd0`)
   `powf(2, (2v - 1) K) * 0.1` above the middle and
   `-0.1 powf(2, (1 - 2v) K)` under it, with `K` 17.60964 (20 kHz) or
   10.965784 (200 Hz). The phase step is `(f64)(hz * (f32)(1 / rate))`.
   In the tempo range the period is `2^((1 - v) 7) * 48` for the ring
   modulator and `+-2^(7 - |v - 0.5| 14) * 48` for the shifters (negative
   under the middle), truncated to an integer and over 192: beats. A
   restart sets the phase from the song's place, `frac((f32)(place /
   period) + start)` (for a negative period `frac(1 - frac(place / -period)
   + start)`), and the step is `(f32)((next - place) / period)`. What the
   HQ crossovers take for the shift is then `(f32)(1 / (step * rate))`.
5. Ring mod: each side times the sidechain, or times `sin(2 pi phase)`,
   the right at `frac(phase + L/R)`; the phase moves on by the step.
6. Shifters: the HQ one, at every 32nd frame (counter `+0xe8`), designs its
   five banks, with `s = clamp(|hz|, 20, 20000)` and `f = clamp(22050 - |hz|,
   20, 20000)`: bank `0x390` low-pass `f`, high-pass `s`; `0x8a0` and
   `0x17d0` both at `s`; `0xdb0` and `0x12c0` both at `f`. It sets the
   crossfade's step `+0x64` to `+-499.99997 / rate` by the shift's sign.
   `0x390` splits `(L, R, L, R)`; `0x8a0` runs its low half and `0xdb0` its
   high half as Linkwitz-Riley allpasses (`0x180003690`: both halves of a
   pair, summed) to keep them in phase. The dry signal goes through `0x12c0`
   then `0x17d0` the same way. The pair crossfades by `+0x60` into `(P xf +
   Q (1 - xf), Q xf + P (1 - xf))` per side, and the crossfade moves by its
   step, kept in 0..1. The plain shifter takes `(L, R, L, R)` as it is.
7. The Hilbert pair (`0x180003760`, state `+0x100`): two chains of four
   allpass sections `y = c (x + y[n-2]) - x[n-2]` over the four lanes,
   `c` = 0.4794009, 0.8762185, 0.9765975, 0.9974992 for the first, which
   comes out a frame late, and 0.1617585, 0.7330289, 0.9453496, 0.9905992
   for the second (`0x180025640..0x1800256b8`).
8. `sinf` of `(2 pi p, 2 pi p + pi/2, 2 pi q, 2 pi q + pi/2)` with `p` the
   phase and `q = frac(p + L/R)` (`0x180016813`); the phase moves on by
   `frac(step + 1000 + phase)`. Each side's wet output is `(B2 cos + A2 sin)
   (1 - shape) + (B0 cos - A0 sin) shape` for the left, lanes 3, 1 for the
   right, `A` the late chain and `B` the other: shape 1 is the upper
   sideband of the pair's first half, shape 0 the lower of its second.
9. Feedback takes the wet outputs; `wet * out + dry * in` per side; the
   stereo wheel: `left = s R + (1 - s) L`, `right = (1 - s) R + s L`.

## What fl-rs leaves out or stands in for

- The CRT's vector `sinf`, `sin`, `tanhf`, `powf` and `pow` are Rust's, so
  the last bits can differ.
- The glides from the engine's own defaults land when a state is taken
  (above).
- In the tempo range FL's place comes from its own clock in ms and its time
  signature; fl-rs counts frames from the tick the song started or moved to
  and takes four steps a beat.
