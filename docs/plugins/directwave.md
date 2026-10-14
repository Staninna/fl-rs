# DirectWave: state, programs, zones, voice model

Recovered from `DirectWave_x64.dll` (FL Studio 2026, MSVC C++, image base
0x180000000, exports `CreatePlugInstance` and `VSTPluginMain`). The editor
lives in `editor_x64.dll` and was not read. Ghidra project
`ghidra/proj/directwave`; decompiles under `ghidra/out/directwave/` (`vmt/`
CreatePlugInstance, `slots/` the SDK slots with callees, `state/` the bank
loader and saver, `zone/` the zone chunks and the voice) and REA decompiles of
the small helpers. Corpus: 57 states in 18 `.flp`, 140 in `.fst` presets,
197 in all, 2876 zones, 12 of them carrying their samples.

Confidence tags: `loader`, `asm`, `corpus`, `inferred`.

## How the plugin is wired

`CreatePlugInstance` @ `0x1800240c0` allocates 0xaaa9a0 bytes and runs the
constructor `FUN_180014ef0`: vtable `0x1800d0418`, the engine object at
`+0x230` (`FUN_18000f6f0`).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x1800155c0 |
| 3 | SaveRestoreState | 0x180016140: load `FUN_18003dcd0`, save `FUN_180040610` |
| 5 | ProcessEvent | 0x180016040 |
| 6 | ProcessParam | 0x180016070 (`FUN_18002a660` set, `FUN_180026e10` get) |
| 8 | Gen_Render | 0x180015ba0 -> engine render `FUN_18002b130` |
| 9 | TriggerVoice | 0x180016260 -> note on `FUN_180024f90` |
| 10 | Voice_Release | 0x180016600 -> note off `FUN_180024e30` |
| 11 | Voice_Kill | 0x180016520 (drops FL's voice record only) |

The engine is a MIDI instrument: 16 parts (MIDI channels, 0x670 bytes each
at `engine+0xca2`), 128 programs (0xd60 bytes each, array at
`engine+0x73a2`), 256 voices (0x960 bytes each from `engine+0x12510`).

## 1. State layout (event 213)

`FUN_18003dcd0` reads a version and then chunks of `i32 id, i64 size, size
bytes`, the format FPC uses. Above 0x26 the state is turned down; below 0x24
it is the fixed layout of `FUN_18003e050`. FL 2026 saves 0x26 (`FUN_180040750`
writes it); the corpus has 0x24 x100, 0x25 x46, 0x26 x49, 0x20 x1, 0x22 x1.

Top level (`FUN_18003eb30`):

| id | size | meaning |
|---|---|---|
| 6 | 0x10 | flags: 1 `engine+0xaaa738`, 2 `+0xaaa739`, 4 `+0xaaa73a`, 8 not a plugin build, 0x10 / 0x20 a mode byte (`+0x240`) |
| 0x206 | 7 | `b0 +0x23b`, `b1..4 +0x23c` (1: high quality interpolation), `b5 +0x241` (steps of 8 samples), `b6` the mode byte |
| 5 | 0x274 | a part (below) |
| 1 | any | a program (below) |

Corpus: chunk 6 is 0 in 194 states and 0x10 in one; chunk 0x206 has
`+0x23c = 0` and `+0x241 = 0` everywhere.

The fixed layout (`FUN_18003e050`): `i32 mode`, 16 parts of 0x270 bytes, 128
programs of 0x1b4 (below 0x20), 0x1c4 (below 0x23) or 0x15ac bytes with the
zone count at `+0x130`, each zone 0x6a2 bytes and its samples when the mode
is 1. The two such states FL ships (`System/DirectWaveTrack.fst`,
`System/FL Mobile/DirectWave.fst`) have no zone.

### Part (chunk 5; the part's memory in brackets)

| offset | type | meaning |
|---|---|---|
| 0x000 | i32 | part index |
| 0x004 | i32 | last note `[+0x000]` |
| 0x008 | 128 x i32 | velocities held, ignored on load |
| 0x208 | u8 | program `[+0x604]` |
| 0x209 | u8 | output `[+0x605]` |
| 0x214 | i32 | pitch bend range, cents `[+0x610]`, 200 |
| 0x224 | 2 x f32 | peak meters, ignored on load |
| 0x234 | f32 | pitch bend -1..1 `[+0x630]` |
| 0x238 | f32 | mod wheel `[+0x634]` |
| 0x23c | f32 | volume `[+0x638]`, 100/127 in every corpus part |
| 0x240 | f32 | pan 0..1 `[+0x63c]`, 0.5 |
| 0x244 | f32 | expression `[+0x640]`, 1 |
| 0x248 | f32 | sustain pedal `[+0x644]`, above 0.5 holds |

### Program (chunk 1, `FUN_18003f130`; offsets in the 0xd60-byte program)

The first chunk must be 100 (or 2, which ends it).

| id | size | fields |
|---|---|---|
| 100 | 0x1e | `i32` index; `b4 +0x289` poly mode (0 poly, 1 mono, 2 legato); `b5 +0x28a` glide only from a held note; `f32@6 +0x29e` volume; `f32@10 +0x2a2` glide time; `i32@14 +0x29a` zone count; `b18 - 1 +0x288` key switch (-1 none); `b19 +0x326 = (b19 != 1)` |
| 0x66 | text | name `+0x000` |
| 0x67 | text | the program's `.dwp` path `+0x080` |
| 0x68 | 10 | `b0 +0x28b` tanh shaper on, `b1 +0x28c` sine shaper on, `f32@2 +0x2e6`, `f32@6 +0x2ea` their drives |
| 0x69 | 0x12 | effect 1: `b0 +0x28d` on, `b1 +0x28e`, 4 x f32 `+0x2ee` |
| 0x6a | 0x11 | effect 3, a reverb (`FUN_18002d0b0`): `b0 +0x28f` on, 4 x f32 `+0x2fe` |
| 0x6b | 0x11 | effect 2: `b0 +0x290` on, 4 x f32 `+0x30e` |
| 0x6c | 0x14 x 2 | program LFOs `+0x2a6` and `+0x2ba`: shape, sync, 2 bytes, rate, phase, fade, one more |
| 0x6d | 4 x 4 | the four knobs `+0x2ce`, sources 16..19 |
| 0x6e | 13 x 100 | group `i` (1..99): mode `+0x32a + 10i` (above 3 reads as 0), chance `+0x32c + 10i`, crossfade `+0x330 + 10i`. Group 0 keeps its defaults: mode 0, chance 1, crossfade 0 |
| 3 | any | a zone, while fewer than the count are read |
| 2 | 0 | end |

### Zone (chunk 3, `FUN_18003f840`; offsets in the 0x41a-byte zone)

The first chunk must be 500 (or 4, which ends it).

| id | size | fields |
|---|---|---|
| 500 | 0x19 | root `+0x2b4`, key low/high `+0x2b5/6`, velocity low/high `+0x2b7/8`, cut group `+0x2b9`, `+0x2ba`, byte 7 skipped, filter routing `+0x2c4` (0 in series, else side by side), `f32@9` volume `+0x302`, `f32@13` pan 0..1 `+0x306`, `i32@17` group `+0x396`, `i32@21` the frames around the samples (512) |
| 0x1f5 | text | name |
| 0x1f6 | text | sample path `+0xac` |
| 0x1f7 | 0x28 | `i64` frames `[0]`; `i32` channels `[1]`, slices `[2]`; `f32` rate `[6]`; `i32` loop mode `[7]` (0, 1 once with no note off, 2 loop, 3 loop until the note off, 4 back and forth), loop start `[8]`, loop end `[9]`, start `[10]` (0 when past the frames), bits `[3]` (32 when the chunk is 0x24 bytes) |
| 0x1f8 | 8 | `f32` pitch knob `+0x2ee` (0.5 is none), coarse `i8 +0x2bc`, fine `i8 +0x2bd`, key tracking `i16 +0x2be` (cents a key) |
| 0x1f9 | 0xe | stretch mode `+0x2c2` (1 granular), `+0x2c3`, 3 x f32 `+0x386` |
| 0x1fa | 0x30 | 4 x f32 `+0x32e`: sends to effects 1, 2, 3 and the dry level |
| 0x1fb | 0x14 x 2 | filters `+0x2c6` / `+0x2da`: type, 3 bytes, cutoff, resonance, clip, one more |
| 0x1fc | 2 | tempo sync `+0x2c0`, start at the key's slice `+0x2c1` |
| 0x1fd | 0x10 | volume envelope `+0x2f2`: attack, decay, sustain, release |
| 0x1fe..0x201 | 9 each | ring modulator, decimator, bit crusher, phaser: on `+0x30a..+0x30d`, two f32 each `+0x30e..+0x32a` |
| 0x202 | 0x10 x 2 | envelopes 1, 2 `+0x33e`, `+0x34e`: attack, decay, sustain, release |
| 0x203 | 0x14 x 2 | LFOs `+0x35e`, `+0x372`: shape, sync, 2 bytes, rate, start phase, fade, one more |
| 0x204 | 8 x 16 | matrix slot `i`: `i16` source `+0x39a + 8i`, `i16` destination, `f32` amount |
| 0x205 | any | the samples, per channel `(frames + extra)` values of the zone's bits, the first 256 before the sound |
| 0x206 | any | compressed samples (`FUN_180037a70`); none in the corpus |
| 4 | 0 | end |

On load every zone with no samples of its own is read from its path
(`FUN_18003e9b0` -> `FUN_18001ddb0`), which sets frames, channels, rate and
bits from the file and keeps the loop and the start of the state. A WAV
(`FUN_180043d40`) keeps 8- and 16-bit PCM as 16 bits (8-bit as `(b - 128) *
256`) and turns 24- and 32-bit PCM into floats (`x * 2^-31` on the value
shifted to 32 bits); its Ogg Vorbis kinds go to `FUN_1800437c0`. Every buffer
has 256 frames of silence before and after.

Corpus (flp zones): loop mode 0 x907, 2 x373, 3 x7; filter 1 type 0 or 1
(lowpass, cutoff 0.4..1); envelopes `(0, 0.5, 1, 0.25..0.55)`; matrix slots
`2 -> 2 @ 1.0` (every zone), `3 -> 34 @ 0.75`, `12 -> 1 @ 0.5`, `2 -> 7 @
0.91` and `1 -> 7 @ 0.75` where the filter is on, a few LFOs to pitch, pan and
volume. No stretch, sync, zone effect or filter type above 1.

## 2. A note (`FUN_180016260`, `FUN_180024f90`)

```
note     = clamp(trunc(InitLevels.Pitch * 0.01 + 0.5), 0, 127)
level    = clamp(trunc(FinalLevels.Vol * 127), 1, 127)                       // the voice's volume source
velocity = clamp(trunc(Host.Voice_ProcessEvent(tag, FPV_GetVelocity) * 127), 1, 127)
part     = the note's colour when the MIDI channel mode is on, else 0
```

The engine's note on, for the part's program:

1. A key switch: a program whose `+0x288` is the note becomes the part's, and
   nothing sounds.
2. Every zone with `low <= note <= high` and `vlow - fade <= velocity <= vhigh
   + fade` (`fade = crossfade * 0`) goes on its group's list.
3. Per group with a list: when its chance is below 1 a `rand_s` draw can empty
   it; then with more than one zone mode 1 takes the next after the last,
   2 one at random (`rand() / (0x7fff / n)`), 3 one at random that is not the
   last, and 0 keeps them all.
4. Per zone left: a cut group releases the voices of that group (forced);
   mono releases every voice of the part (forced, marking them ended); legato
   moves a sounding voice of the part to the new note instead of starting
   one; then the first voice with its envelope off takes the zone.
5. The part keeps `velocity` and the voice tag under the key, and the note as
   its last.

`FUN_180033f00` releases a voice: its volume envelope and both matrix
envelopes go to release when they are sounding, unless the zone is loop mode
1 and the release is not forced; a voice that has not run yet is dropped.

The note off (`FUN_180024e30`) clears the key, and unless the sustain pedal
is down releases the voices of that note, part and tag. A mono or legato
program whose last note it was starts the lowest key still held again.

## 3. Rendering (`FUN_18002b130`)

The block goes in steps of at most 256 samples (8 with `+0x241`). Per step:

```
per_second = SR / n                        // ctx+0x1c
lfo_turn   = 2 pi * 20 / per_second        // ctx+0x40
beat       = per_second * 15 / tempo       // ctx+0x20
smooth     = exp(-1 / (SR * 0.005))        // ctx+0x30, FUN_18002bf20 at a rate change
attack     = exp(-1000 / SR), release = exp(-10 / SR)      // ctx+0x34, +0x38
```

For each part whose program has zones: the program LFOs move, eight buffers
(dry, three sends, two sides) are cleared, every voice of the part runs
(`FUN_180030090`), the three effects run (`FUN_180011f10`, `FUN_1800119c0`, the
reverb `FUN_18002d0b0`), the shapers run
(`x = tanhf(x / g^3) * g` and `x = sinf(x / g^3) * g` with `g = 1 - drive *
0.75`), and the part's buffers add into the output its output number names.
In FL's plugin build every voice takes FL's levels for its tag once a step:
`[0] = Final.Pan`, `[1] = Final.Vol`, `[2] = Final.Pitch + 6000 - note * 100`,
`[3]`, `[4]` the cutoff and resonance.

## 4. A voice (`FUN_180030090`)

The first step a voice runs: position 0, envelope follower 0, the two matrix
envelopes and the volume envelope start, each LFO restarts its fade and its
phase (`2 pi * phase`, only when the phase knob is below 1, so 1 keeps where the
slot's last voice left it), a random value `rand() / 32768`, and the glide
starts at the last note (at the note itself with no glide time, or when the
program glides only from a held note and none was).

Then every step:

```
env1, env2 = the matrix envelopes, at per_second ticks a second
lfo1, lfo2 = LFO(shape, 20 * rate^2 Hz, or 2 pi / (trunc(rate^2 * 127.5 + 1) * beat) synced), fade in at per_second
the matrix (below) over d[0..48]
start, loop low and high (d[4..6] when a slot moves them, as fractions of the frames)
cents = (glide - root) * keytrack + coarse * 100 + fine + d1 * 2400 - 1200 + bendRange * bend + FL cents
glide moves to the note by pace(glideTime, per_second) * |note - previous|   (after the pitch is taken)
step  = trunc(2^(cents / 1200) * fileRate / SR * 2^32)          // pow in double
amp   = d2 [* zoneVol unless flag 2] * programVol * partVol * expression
L, R  = sqrt((1 - partPan)(1 - d3)) * amp, sqrt(partPan * d3) * amp      // flag 2: * 2 * amp more
```

The envelopes (`FUN_180013f20`): attack adds `a` until 1, decay takes `d`
until the sustain, release takes `r` until 0, and the output is the level
squared. `pace(x, per) = 1 / (x^4 * per * 10 + per * 0.001)` (`FUN_180011020`)
turns a knob into a step: `x^4 * 10 s + 1 ms` to cross the whole range. The
volume envelope's release is 0.25 once the file has played out.

LFO shapes (`FUN_180025df0`): 0 `sin`, 1 `|sin|`, 2 triangle, 3 square, 4
saw up, 5 saw down, 6 a random value drawn each turn (`(rand() - 0x4000) /
16384`), 7 that value smoothed by the turn's step (the value before the step
is the output). The output is `fade * value`.

### The matrix

`d[1]` pitch (zone `+0x2ee`), `d[2]` 1 (the zone volume with flag 2), `d[3]`
`clamp(FL pan * 0.5 + zone pan, 0, 1)`, `d[4..6]` start and loop, `d[7..12]`
the two filters' cutoff, resonance and clip, `d[13..16]` the volume envelope,
`d[17..24]` the zone effects, `d[25..28]` the sends and the dry level,
`d[29..31]` stretch, `d[32 + i]` slot `i`'s amount.

Sources: 1 `note / 127`, 2 `Final.Vol`, 3 mod wheel, 4 bend, 5 and 6 the
program LFOs, 9 and 10 the envelopes, 11 and 12 the LFOs, 13 the envelope
follower, 14 `frame / frames`, 15 the voice's random value, 16..19 the
program knobs; 0, 7 and 8 add nothing.

```
amount(a) = (2 (a - 0.5))^3
slots 15..0 whose destination is the amount of an earlier slot: d[t] = clamp(d[t] + amount(d[32+i]) * source, 0, 1)
slots 0..15 for every other destination:
    source 2 to destination 2 without flag 2:  d[2] = d[2] * amount * Final.Vol, not clamped unless flag 1
    otherwise d[t] = clamp(d[t] + amount * source, 0, 1)
```

So the default slot `2 -> 2 @ 1.0` makes the volume FL's note volume.

### Reading the file

32.32 fixed point (`+0x88`), stepping by `step`. A 16-bit file is read as
`i16 * 3.051851e-5` (1/32767). The readers draw a line between two frames
with `f = frac * 2^-32` taken through a double:

```
mono (FUN_1800332d0):          (s1 - s0) * f + s0
stereo (FUN_180033550):        (1 - f) * s0 + f * s1,  16-bit: (i0 (1 - f) + i1 f) * 3.051851e-5
looped (FUN_1800333a0 / FUN_180033670): at frame loopEnd - 1 before the first wrap or while looping,
                               s1 is the loop start and the mono form is used on both sides
```

Loop mode 0 and 1, and 3 after the note off: step on, and at the end of the
file hold there, mark the voice ended and release its volume envelope; the
whole step goes first, so a file that ends in a step starts its release at
the step's first sample. Modes 2 and 3: past the loop end the frame goes to
`low + (frame - low) mod length`, the fraction kept. Mode 4 turns at the loop
end and at the loop start.

### Filters, limiter, amp

When either filter type is set: the cutoffs (knob units) move toward `d[7]`
and `d[10]` by `exp(-1 / (per_second * 0.005))` a step, after the
coefficients are taken from them. Coefficients (`FUN_180003a10`) are the
cookbook's with `w = 2 pi f / SR` in float, `f = 440 * 2^(cutoff * 127 / 12 -
5)` (`FUN_180010f30`) and `Q = sqrt(0.5) / (1.01 - resonance)`: type 1
lowpass, 2 highpass, 3 bandpass, 4 notch, 5 allpass; 6 and 7 are other
filters (`FUN_18003b4b0`, `FUN_180044f60`) not read. Each runs as two equal
biquad stages (`FUN_180014dd0`) with the input plus `rand() * 1e-14`, then a
clip at `1 - clip` when the clip is above 0, in series or side by side. A
limiter follows (`FUN_180003980`): `env` follows `max(|L|, |R|)` with the
attack or release coefficient, and above 1 both sides are divided by it.

The zone effects follow (ring modulator, decimator, crusher, phaser), then the
volume: per sample `L *= env * gainL` with the gains moving to their step's
targets by `g = (g - t) * smooth + t + 1e-14`; they start on the targets.
The dry level adds the voice into the part (as is at 1 or more), and each
send adds it into an effect's buffer when that effect is on.

## 5. What stays unknown

- The three effects (`FUN_180011f10`, `FUN_1800119c0`, the reverb
  `FUN_18002d0b0`, read but not built), the program LFOs' fade, filter types
  6 and 7, the zone effects, stretching, tempo sync and the high quality
  interpolation (`FUN_18002f0d0`, `FUN_18002f620`).
- Whether FL links velocity into `Final.Vol` for DirectWave. Nothing in the
  DLL asks for it (`FPV_SetLinkVelocity` is never sent); the engine passes
  the note's velocity as `Final.Vol`, as for the other generators.
- The editor's names for the fields.

## Implementation notes (crates/plugins/src/directwave.rs)

- Every note goes to part 0, and every part plays into the channel.
- The host decodes the files; a 16-bit one is told apart by its values all
  being whole 32768ths, and is read back as FL holds it. Samples a zone
  carries go into the bank with `Bank::hold`.
- `rand` is MSVC's, seeded at 1 when the state is read. FL's stream is shared
  with the rest of its thread, so the random sources and the 1e-14 guard are
  not FL's draws.
- The steps start where the engine's blocks start. FL's blocks are its own,
  so what moves once a step moves at other samples.
- A note off for a voice that has not run yet waits until it has: the engine
  sends a step note's note off on its note on's sample, which FL never does.
- The VST host part (`+0x77cc`) and the preview and metronome paths are left
  out.
