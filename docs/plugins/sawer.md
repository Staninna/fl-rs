# Sawer: state, parameters, voice

Recovered from `Plugins/Fruity/Generators/Sawer/engine_x64.dll` (FL Studio
2026, MSVC C++, image base 0x180000000), where the state, the voices and the
sound live: a `SawerSynth` (a `CSynthesizer`, as Sakura's `CSakuraSynth` is)
inside FL's `CFLWrapper` frame, with 24 `CSawerGenerator` voices. `Sawer_x64.dll`
is the Delphi panel. Read with Ghidra (`DecompileRange.java` over
0x180001000..0x180061000) and capstone listings, and checked by running the
DLL's code under unicorn (`.scratch/notes/sawer-emu/`). The saw itself is
`Sawer/data.saw` beside the DLL: 8192 f32 of one period, read raw
(`0x180031680`).

Corpus: 393 shipped presets (FL's own and the plugin database's) and the
render test `System/Render tests/Sawer.flp`, all 620 bytes. No shipped
project uses Sawer.

Port: `crates/plugins/src/sawer.rs`. Bit for bit against the DLL under the
emulator: the render test at key 60 (4000 frames, let go at 3000) and as a
three-note chord with releases (the four-voice path), and, with the effects
switched off in the state, every 13th and every 40th preset that does not
use the arpeggiator, as a five-note and a two-note chord, but for two
(below).

Confidence tags: `asm`, `emu`, `corpus`.

## How the plugin is wired

The `SawerSynth` vtable is at 0x180079fa0 (`asm`): slot 1 note on/off
(`0x180007a20`: key and MIDI velocity, velocity / 127 into `+0x1068[key]`),
slot 10 sample rate (`0x180037480`), slot 17 a block (`0x180034ba0`), slot
23 a parameter (`0x180032020` -> `0x180032090`), slot 35/36 get/set state
(`0x180034660`/`0x180034770`), slot 38 note on (`0x180037c50`), slot 39 note
off (`0x180037d20`), slot 53 polyphony. FL's wrapper (`FUN_180004190`) cuts a
host block into pieces of 64 frames, its I/O buffer's length; the engine runs
each piece.

The engine's descriptor (`0x1800a5fd0`, `SawerInstance`): 0x60 bytes of extra
state, 0x49 public parameters, 8 and 24 voices, one stereo output.

Two random generators: `0x180086e30` (`x * 0x0bb38435 + 0x3619636b` for the
noise, the drift and the LFO's sample and hold; `x * 0x77a73631 +
0xf6ff3a94` when a voice is made over) and `0x180086e34` (the unison's random
detune, whose amount `0x38cc` nothing sets). They are global to the DLL; the
port starts them where one fresh instance with a sample rate leaves them.

## 1. State (event 213: 620 bytes)

| byte | type | field |
|---|---|---|
| 0..3 | char[4] | `SSLF` |
| 4..7 | u32 | the chunk's length, 612 |
| 8..11 | char[4] | `1rwS` (`Swr1`); `DrwS` is a demo save |
| 12..395 | f32[96] | the parameters, applied in order (slot 23) |
| 396..491 | u8[96] | the engine's own bytes (all 0xff in the corpus) |
| 492..619 | char[128] | the program's name |

After the 96, parameters 0x4e..0x5f (the matrix) are applied again.

## 2. Parameters (`0x180032090`, `asm`)

`v` is the stored value after the matrix (`0x180037910`: each of 6 slots whose
destination is this parameter adds `source * amount`; sources 1 velocity, 2
and 3 FL's wheels, 4.. controllers; kept to 0..1). `fifth(v) = max(50 v^5,
0.00101)` s.

| n | name | engine |
|---|---|---|
| 0 | Master Level | `powf(10, 2.4 v - 0.9)` |
| 1 | Master Pan | `v` |
| 2, 3, 5 | Master Attack, Decay, Release | `fifth(v)`, release `* 0.35 + 0.02` |
| 4 | Master Sustain | `v` |
| 6, 7 | Transpose, Fine Tune | `int(48 v - 24) - 12`, `2 v - 1` semitones |
| 8 | Mono Mode | |
| 9, 10 | Glide TimeMode, KeyMode | four modes |
| 11 | Glide Time | `v^3`, at least 1e-4 |
| 12 | Octaver | odd unison voices an octave up |
| 13 | Unison Rtrg | phases to 0 at a note |
| 14, 15, 16 | Unison, Detune, Pan | 1, 2, 4 or 8 voices; spread `0.5 v` semitones; pan |
| 17..23 | Sub-Saw Phs, Hrm, Level, Detune, Retrig, Invert, Square | offset; ratio 1/4..4; `v^2`; `v^2`; retrigger; sign; the saw less itself half a turn on |
| 24, 25 | Osc Sync Frq, Osc Sync | `4 v` octaves |
| 26 | Noise Level | `0.7 v^2` |
| 27..31 | Filter Cutoff, Reso, KeyTrk, VelTrk, Mode | `2^(9.3 v + 5 + ...)` Hz; `3.99 v`; ...; LP4, LP2, BP, HP |
| 32..37 | EGF Attack, Decay, Sustain, Release, Amount, Dest | cutoff, sub phase, sync or pitch |
| 38..46 | LFO Shape, Speed, Tmp Sync, Attack, Release, Retrig, Invert, Amount, Dest | triangle, saw, square, sample and hold; `10 v^3` Hz |
| 47..50 | Chorus On, Depth, Rate, Mix | not built |
| 51..56 | Phaser On, Center, Rate, Depth, Fdbk, Mix | not built |
| 57..64 | Delay On, Left, Right, Fdbk, Depth, LoCut, HiCut, Mix | not built |
| 65..69 | Reverb On, Decay, HiCut, Damp, Mix | not built |
| 70..72 | Arp Play, Mode, Range | not built |
| 75 | polyphony | `int((v + 1) * 4)`, 24 out of range |
| 78..95 | the matrix: amounts (`(2v-1)^2` signed), destinations, sources | |

## 3. A block (`0x180034ba0`)

1. The LFO's value from its phase, the phase on by `n * speed / rate` (or the
   tempo over 15 and the division), a new sample-and-hold value on wrapping.
2. The voices in list order, sounding ones under the polyphony first
   (`0x180034a40`). With one voice the scalar loop (`0x180036710`) runs it,
   otherwise groups of four the SSE loop (`0x180035160`), the last group
   padded with idle voices; a group's voices are updated (`0x1800305d0`) every
   `2 + int(rate / 1000)` base samples.
3. Each voice runs at 6x: the saw at `phase >> 11` (24-bit phases that reset
   to 0 past 2^24, and the sub and main reset by the sync phase), plus the
   sub, times `1 + 0.06 noise`, less 0.35, plus noise, times the amp
   envelope, into a four-pole ladder run three times with the last stage's
   output fed back through `y / (5 max(y,0)^4 + 1)`. The taps are mixed by
   the filter mode (smoothed), then by the voice's pan gains. The SSE loop
   does the same sums in another order and with the mix a step behind; the
   port keeps both.
4. Down to the host's rate (`0x180036f50`): a biquad lowpass (`sincos` of
   `16755.16 / rate`) on the 6x samples, keeping every sixth, its sums in the
   unrolled loop's pairs.
5. A 4 Hz highpass (`sakura_fx::HighPass`), one filter run on the left and
   the right samples in turn.
6. The effects (not built), then the master level and pan, moving 0.5% a
   sample (`0x180023220`).

The voice update (`0x1800305d0`, `asm`): the glide, the LFO with its attack
and release, a slow random drift of the pitch, the gains, noise, sub phase,
sync, the pitch (`220 * 2^((note - 57) / 12)` in f64), the phase steps, the
cutoff, and the envelopes: one-pole steps of `8.82 / (time * rate)` toward
`5 velocity` in the attack (time `* 39.44`), the sustain after, 0 once let
go; a voice is ended 0.1 s after its release time.

## 4. Left open

- The chorus, phaser, delay and reverb (`0x180030300`: `CChorus` and
  `CDelay` of `CTapeDelay` lines, as Sakura's but with cut filters, a phaser
  and `fxReverb`). A state that turns one on plays without it and says so in
  `fidelity`. 281 of the 393 presets use the delay.
- The arpeggiator (47 presets).
- Two chords part from the DLL: MC Gringo (two-voice unison past the
  polyphony, after the releases) and MC North Bear (mono with glide).
- The random generators are one instance's, where FL shares them between
  every Sawer it has loaded.
