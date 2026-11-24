# PoiZone: state, parameters, voice

Recovered from `Plugins/Fruity/Generators/PoiZone/engine_x64.dll` (FL
Studio 2026, MSVC C++, image base 0x180000000), where the state, the voices
and the sound live: a `PoiZoneSynth` (a `CSynthesizer`, as Sawer's and
Sakura's are) inside FL's `CFLWrapper` frame, with 1024 `PoiZoneGenerator`
voices. `PoiZone_x64.dll` is the panel. Read with Ghidra and capstone
listings, and checked by running the DLL's code under unicorn
(`.scratch/notes/poizone-emu/`).

Corpus: 388 shipped presets, the render test `System/Render tests/PoiZone.flp`
and one demo song (`DEDZII - NIGHT TERROR`), all 700 bytes.

Port: `crates/plugins/src/poizone.rs`. Bit for bit against the DLL under the
emulator, in blocks of 64 at 44.1 kHz: the render test as one note and as a
four-note chord with releases (the four-voice path), every fourth and every
eleventh preset with the arpeggiator switched off as two-, three- and
four-note chords, delay, chorus and gate as stored, and two of them at 22.05,
48 and 96 kHz.

Confidence tags: `asm`, `emu`, `corpus`.

## How the plugin is wired

The `PoiZoneSynth` vtable is at 0x180075970 (`asm`): slot 1 note on/off
(`0x180007a20`), slot 10 sample rate (`0x180033090`), slot 11/12 the tempo
(`+0xc28`, 120 in a new instance), slot 17 a block (`0x1800312e0`), slot 23
a parameter (`0x18002e3f0`), slot 35/36 get/set state
(`0x180031020`/`0x1800310e0`), slot 38/39 note on/off
(`0x180033550`/`0x1800335a0`), slot 53 polyphony, slot 57 a key in poly
mode (`0x180024300`), slot 58 in mono mode (`0x1800248f0`). The voice's
vtable (0x1800758a0): reset `0x18002d280`, start `0x18002d0e0`, release
`0x18002d210`, made over at a new rate `0x18002d2f0`, update `0x18002c580`.
FL's wrapper cuts a host block into pieces of 64 frames.

Three random generators, `x * 0x0bb38435 + 0x3619636b`, global to the DLL:
`0x180081e20` (22222 at load: two phases for each of the 1025 voices the
constructor makes, then the noise), `0x180081e24` (2222: a key's random
detune, `(2r - 2) * 0.01` semitones) and `0x180081e28` (2222: the LFO's
sample and hold).

## 1. State (event 213: 700 bytes)

| byte | type | field |
|---|---|---|
| 0..3 | char[4] | `SSLF` |
| 4..7 | u32 | the chunk's length, 692 |
| 8..11 | char[4] | `4reV` (`Ver4`); `3reV` in 384 presets, read the same; `ED4V`/`ED3V` demo saves |
| 12..635 | f32[156] | the parameters, applied in order (slot 23) after the defaults |
| 636..699 | char[64] | the program's name |

## 2. Parameters (`0x18002e3f0`, `asm`)

`q(v, lo, hi) = clamp(v^4 (hi - lo) + lo)` ms. Choices are `int(v n + a
little)`.

| n | engine | |
|---|---|---|
| 0, 4 | `0x59ec`, `0x59f4` | osc A, B pulse (else saw) |
| 1, 5 | `0x59f0`, `0x59f8` | pulse widths, smoothed |
| 2, 3 | `0x5a0c`, `0x5a0d` | sync (A to B), ring |
| 6, 7 | `0x59fc`, `0x5a00` | osc B `-24 - int(-48 v)` semitones, `0.5 v` |
| 8, 9 | `0x5a04`, `0x5a08` | A/B balance, noise balance, smoothed |
| 10, 26, 43 | `0x5a44`, `0x5a78`, `0x5a84` | EG, LFO, mod wheel amounts `2v - 1`, smoothed |
| 11..14 | `0x5a34..0x5a40` | mod EG `q(v, 5, 50000)`, sustain `v` |
| 15 | `0x3900` | level `powf(10, 1.5 v - 1)` |
| 16, 17 | `0x5a8c`, `0x5a88` | fine `2v - 1`, transpose `-24 - int(-48 v)` |
| 18..21 | `0x5a10`, `0x5a14`, `0x5a1c`, `0x5a18` | cutoff (smoothed), resonance, key and velocity tracking |
| 22..25 | `0x5a24`, `0x5a28`, `0x5a30`, `0x5a2c` | amp attack `q(v,1,5000)`, decay `q(v,5,20000)`, sustain `v^2`, release `q(v,15,10000)` |
| 27, 28, 68, 80 | LFO at `0x5a50` | retrigger, `30 v^5` Hz, shape (saw, square, triangle, cosine, sample and hold), tempo division |
| 29, 30 | `0x38f8`, `0x38fc` | unison detune `1.5 v`, pan |
| 31..38 | `CDelay` | on, time L/R (`int(7v) + 1` sixteenths), feedback `0.84 v`, depth `0.01 v^2`, low cut `10 * 2^(10 v)`, high cut `200 * 2^(6.6 v)`, wet `v^2` |
| 39..42 | `CChorus` | on, rate `5v + 0.1` Hz, depth `v^3`, wet |
| 44, 45, 46 | `0x38e0`, `0x38e1`, `0x38d0` | mono, octaver, glide `2 v^3` |
| 47..64, 79 | gate at `0x9af0` | mix, smoothing, 16 steps, speed |
| 65, 67, 71 | `0x5a48`, `0x5a7c`, `0x5a80` | EG, LFO, mod wheel destinations |
| 66 | `0x5a20` | filter mode: LP4, BP, HP |
| 69, 70, 77 | arpeggiator | not built |
| 72, 73 | `0x38d4` | glide modes |
| 75, 76 | `0x38f0`, `0x38e4` | unison 1, 2, 3, 4, 8, 12, 16; polyphony `int(v) + 1` |
| 78 | `0x38dc` | pitch bend range |
| 81..154 | | controller links |

## 3. A block (`0x1800312e0`)

1. The smoothed knobs move 4% toward theirs.
2. The voices are sorted, sounding first, when one started or ended
   (`0x180031260`). One sounding voice runs the scalar loop
   (`0x18002d340`), more run in groups of four through the SSE loop
   (`0x180031760`), padded with idle voices whose phases and the noise
   still turn. Each voice is updated once a block (`0x18002c580`).
3. A voice at 8x: phases fall by their steps and wrap at 0 (the SSE loop
   at `<= 0`), sync resets A when B wraps, a pulse is the saw less the
   saw a width on, ring multiplies, plus noise `(u - 0.125)`, less 0.42,
   less the fourth stage times the resonance, `x / (3x^2 + 1)` when
   negative, four one-pole stages, the mode's mix of `x`, stage 3 and 4,
   a 0.0001 DC follower, the side gains. The SSE loop sums in its own
   order and hands the oscillators' gains back to its lanes reversed
   (`0x180032857`): the port keeps both.
4. Down to the host's rate (`0x1800328e0`): a biquad at pi/12 of the 8x
   rate, keeping every eighth.
5. The trance gate (`0x180033a00`), the 12 kHz high shelf of 5 dB that is
   always on (`0x18001b7e0`), the chorus (Sakura's `CChorus`, the depth
   cubed in f64), the delay (`CTapeDelay` lines as Sakura's, with a
   highpass and lowpass on what they read back), the master level moving
   0.5% a sample, then the global LFO.

The voice update (`0x18002c580`, `asm`): the glide, the mod EG, the LFO
(each voice's own phase, from the global one at its start), the amp EG
(curves 0, -2.5, -2.5: `1 - 10^(t c) (1 - t)`), the side gains as ramps over
the block, the oscillators' pitches (`220 * 2^((p - 57) / 12)` in f64), the
filter (`2^(10 x + 6) / rate`, `x` from cutoff, tracking and modulation,
kept to 0..0.91) and its resonance `(e^(1.386 - 1.523 x) + 1) * r * 2.053`.

## 4. Left open

- The arpeggiator (`CArpeggiator` at `+0x1678`, run by `0x18001ae80`): a
  state that turns it on plays the held keys plainly and says so in
  `fidelity`. 42 presets.
- The mod wheel (`0x126c`), pitch wheel (`0x1468`) and FL's per-note mod X
  and Y (`0x620`, `0x820`) read 0.
- The random generators are one instance's, where FL shares them between
  every PoiZone it has loaded.
