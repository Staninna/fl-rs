# Sakura: state, parameters, voice and effects

Recovered from `Plugins/Fruity/Generators/Sakura/engine_x64.dll` (FL Studio
2026, MSVC C++, image base 0x180000000), where the state, the voices and the
sound live: a `CSakuraSynth` inside FL's `CFLWrapper` frame, as BassDrum's
engine is. `Sakura_x64.dll` beside it is the Delphi panel only. Read with
objdump/capstone listings and checked by running the DLL's own code under
unicorn (`.scratch/notes/sakura-emu/`). Corpus: 504 shipped preset states
(the plugin's own presets and the plugin database's), 5 states in 4
projects (Africa's Calling, Exoplanet with two channels, Lightning and the
render test `System/Render tests/Sakura.flp`), all 596 bytes.

Port: `crates/plugins/src/sakura.rs` (state, parameters, voices, the
engine's rate and resampling) and `crates/plugins/src/sakura_fx.rs` (what
follows the voices, and the engine's maths). Against the DLL under the
emulator it plays bit for bit in 422 of 423 runs: every third of the 509
corpus states at key 48 for 8000 frames and every second at key 72 for
12000, at 44.1 kHz and let go mid-block (339 states in all), the render
test's state (pinned in the test `plays_the_render_test_as_the_engine_dll_does`),
and velocities 30 and 64. The one that parts, PD Aurora Borealis at key 72,
does so by 8e-7 through the runtime's f64 `pow` (section 6).

Confidence tags: `asm` (read from the listing), `emu` (matched against the
DLL run under the emulator), `corpus`, `inferred`.

## How the plugin is wired

The wrapper calls into the synth through these (all `asm`, `emu`):

| address | what |
|---|---|
| 0x18002e670 | `CSakuraSynth` constructor; 0x18002e9c0 builds the preset list from files |
| 0x1800222b0 | polyphony (slot 53); 0 is the most, 99 voices (`0x180001230`) |
| 0x180030660 | sample rate (slot 10) |
| 0x1800313d0 | set state: `"1rkS"` then 114 floats through slot `0xb8` (0x18002f4c0), then the name |
| 0x180007a20 | note on/off: key and a MIDI velocity, 0 lets go |
| 0x180031740 | start a voice (`0x18002d510` the strings' lengths) |
| 0x180031830 | let every voice of a key go (`0x18002d830` each) |
| 0x180030720 | process a host block |
| 0x18002f4c0 | a parameter's engine value (jump table at `0x18003044c`) |
| 0x18002f200 | the parameters' defaults |
| 0x18002cc60 | the song tempo, every block, for the delay's sync |

Synth fields: `+0x20`/`+0x220` per key left and right gains (the note's
pan, circular law), `+0x420` per key pitch offset, `+0x1284` volume and
`+0x1290` pan the host sets (1 and 0.5), `+0x3bb8` the level knob's gain,
`+0x3c50` LFO rate, `+0x3c64` LFO phase, `+0x3c68` LFO value, `+0x3c88` the
effects, `+0x3c90` the voice table, `+0x3fc4`/`+0x43c4` the engine's left
and right samples of a block (index 0 holds the last of the block before),
`+0x47c0` the read position, `+0x47c8` how many samples the block made.

The wrapper turns FL's velocity gain into a whole MIDI velocity,
`int(log21(10 gain + 1) * 127)`, and the voice takes that over 127
(`bassdrum::velocity_level`; `emu` at velocities 30, 64 and 100).

## 1. State (event 213: 596 bytes)

| byte | type | field |
|---|---|---|
| 0..3 | char[4] | `"SSLF"` |
| 4..7 | u32 | the engine chunk's length, 588 (`0x24c`) |
| 8..11 | char[4] | `"1rkS"` (`"Skr1"` high byte first); `"Skr D"` is a demo save. A VST host's 60-byte `CcnK` header before it is skipped |
| 12..467 | f32[114] | the parameters in the engine's order, applied in order |
| 468..595 | char[128] | the program's name, zero padded (`<noname>` by default) |

## 2. Parameters

`v` is the state's float. Most are knobs from 0 to 1; the matrix keeps
sources and targets, and 111 the ratio, as whole numbers (`asm`, `emu`).

| idx | name | engine value |
|---|---|---|
| 0 | Master Level | `10^(4.8 v - 1.8)`, both constants the f32s a step above the nearest (`0x3fe66667` for 1.8) |
| 1 | Tune | `int(198 v - 99)` cents |
| 2, 3, 4, 5 | Amp Attack, Decay, Sustain, Release | `35 v^5 + 0.001`, `50 v^5 + 0.001` s, `v^2`, `10 v^5 + 0.1` s |
| 6, 11 | Str1/Str2 Decay | `v`; the line gain is `x^0.05` |
| 7, 12 | Str1/Str2 Decay Inv | negates the gain |
| 8, 13 | Str1/Str2 Damp Offset | `v^2` |
| 9, 14 | Str1/Str2 Damp Amount | `v` |
| 10, 15 | Str1/Str2 Damp Shrp | `v^2` |
| 16 | Str Saturation | `v^2`; the voice uses `100 s^3` |
| 17 | Str Mix | `v` |
| 18 | Str Stereo Sep | `v` |
| 19 | Comb Ratio | `v` |
| 20, 21 | Comb Fdbk, Inv | `sqrt(v)`, sign |
| 22, 23 | Comb Level, Inv | `v`, sign |
| 24, 25 | Lo-Cut Cutoff, Res | `v` |
| 26..30 | Hi-Cut Attack, Decay, Sustain, Cutoff, Res | `v` |
| 31 | Exc Noise Level | `1 - v` |
| 32 | Exc Noise Rate | `v` |
| 33 | Hi-Cut Drive | `1000 v^4` |
| 34..37 | EG Attack, Decay, Sustain, Release | `10 v^3 + 0.001`, `50 v^5 + 0.001`, `v^2`, `50 v^5 + 0.1` |
| 38 | LFO Rate | `10 v^3 + 0.01` Hz |
| 39..46 | Resonator Frq 1..8 | `1 - v^0.1` of the line |
| 47..54 | Resonator Fdbk 1..8 | `0.9999 sqrt(v)` |
| 55..62 | Resonator On 1..8 | turning one on clears its lines |
| 63..69 | Delay ON/OFF, Time, Fdbk, Mod Speed, Mod Depth, Blur, Wet | |
| 70..73 | Chorus ON/OFF, Rate, Depth, Wet | |
| 74..77 | Reverb ON/OFF, Mix (`0.999 v + 0.001`), Decay (`0.6 v^3`), Color (`v^3`) | the reverb's knobs 12 (wet), 8 (decay time) and 2 (high cut) |
| 78 | unused | |
| 79..110 | the matrix: source, amount, invert, target for each of 8 slots | |
| 111 | string length ratio | whole number |
| 112 | Delay sync | 0 free, 1..9 note lengths |
| 113 | octave | `int(6 v)`: 0 is -3 octaves, 3 none, 6 +3 |

The matrix (`0x18002e470`): each slot whose target is asked for adds
`amount^2` (negated by invert) times its source: 1 envelope, 2 LFO, 3 mod
wheel, 4 pitch wheel, 5 aftertouch, 6 velocity, 7 key (`0.0125 key`, in
f64), 8 release (1 once let go). Targets: 0 LFO amount, 1 level, 2 stereo
separation, 3 pitch, 4 lo-cut, 5 hi-cut, 6 hi-cut attack, 7 hi-cut decay, 8
and 9 decay of string 1 and 2, 10 both, 11 and 12 damping of string 1 and
2, 13 both, 14 noise rate. The LFO amount and the level read 1 where no
slot with a source feeds them.

## 3. A block

`0x180030720`, once per host block:

1. The LFO value is `cos(2 phase * 3.141592653)` (`0x18005a040`, FL's
   truncated pi), then the phase moves by `frames * rate / host rate` and
   wraps past 1. The LFO moves once a host block.
2. The host block is made 48 frames at a time. Frames between the samples
   of the last engine block are read first; then the engine makes
   `ceil(n * 48000 / rate + position)` samples at 48 kHz (`0x1800343e0`)
   and the frames are read between them in straight lines, the fraction
   rounded to an f32.
3. The effects (`0x18002c700`) run once on the whole block, then the master
   level and the host's volume and pan (`0x180031640`, gain from
   `0x1800315e0`).

## 4. The voice

`CSakuraKey`, 0x300 bytes (`asm`, `emu`): `+0x4c` first block, `+0x50`
envelope source, `+0x54` LFO, `+0x68` release source, `+0x70` the envelope,
`+0x100` amp rate, target, value, `+0x118` step, `+0x128` frequency,
`+0x130` scale, `+0x150` time, `+0x158` time since let go (-1 while held),
`+0x170` hi-cut rate, target, envelope, cutoff, resonance, `+0x19c` the
lo-cut's b0, a1, a2, `+0x1a8` active, `+0x1b8..` the four lines and two
combs, `+0x254` comb level and feedback, `+0x25c` saturation, `+0x290`
damping, `+0x2a0` sharpness, `+0x2b0` gain target, `+0x2c0` gain,
`+0x2d0` mix target, `+0x2e0` mix, `+0x2f4` the followers' rates.

Note on (`0x180031740`): a free voice, or the oldest. The strings' lengths
(`0x18002d510`): `f = 220 * 2^((key - 57) / 12)` moved by the octave, `n =
int(48000 / f)`; ratio 0, 1, 2 give the first string `n` rounded down to 4,
3 or 2 times `n/4`, `n/3`, `n/2` and the second that quarter, third or half;
4 and 5 give the second `2n` and `3n`; 6 gives it `4n`; anything else `n`
and `n`. The combs are `comb ratio` of each string. The envelope
(`0x180018910`) starts from where it was.

Every block (`0x18002da50`), for `frames` engine samples:

- step `= f / (48000 / first)`, times `2^((key pitch + tune) / 12 + pitch
  matrix / 6)`;
- the envelope (`0x1800189b0`: straight lines in f64, rates
  `1 / (48000 t)`) and the LFO through its matrix amount;
- noise gate `0.996 (1 - rate)^0.25`, noise level and offset, drive;
- hi-cut: attack `a^2 / 2`, decay `3 d^3 + 0.001`; rate `0.0002 / a` to 1
  in the attack, `0.0002 / d` to `sustain^2` after, and `0x3b03126e`
  (0.0019999999) to 0 once let go; cutoff `(matrix / 2 + knob)^2`, resonance
  `max(-10 (1 - cutoff) res, -4)`;
- lo-cut: a biquad highpass at `2^(9.3 c + 5)` Hz with `q = 10 res + 1`,
  in f64 through the engine's `sincos` (`0x180057960`);
- per string: damping `min(1, 50^(2.57 - 0.8995 x^3) / 22050)` (0.8995 the
  f64 a step below the nearest), the second line's times `1 - offset`,
  sharpness `-(1 - c) sharp / 2`; gain `x^0.05`, negated by invert;
- mix targets: level `10^(2 matrix)`, `f = (1 - mix) level / 20` for string
  1 and `level mix / 20` for string 2, split by the separation and the
  key's pan gains;
- the amplitude: attack to 5 at `4.6587e-6 / attack` a sample, then decay
  to the sustain at `8.82 / (48000 decay)`, and once let go to 0 at
  `8.82 / (48000 release)` until the release time has passed, when the
  voice is killed (`0x18002d870`).

Every engine sample, four voices at a time (`0x180031960`):

1. Four noise draws, `x = x * 0x0bb38435 + 0x3619636b`, one per lane; a
   voice holds a draw that passes its gate.
2. The exciter: `level * held + offset` through the lo-cut biquad, then a
   four-pole ladder with the hi-cut envelope on its cutoff and the last
   stage saturated by `x / (drive x^2 + 1)`.
3. The strings, `int(step + along)` steps: each of the four lines is fed
   the exciter less a one-pole follower of it (rate `1 / length`), and
   gives back what it held through a two-pole lowpass with the sharpness
   fed back and `x / (sat x^2 + 1)`, times the gain. The first line of each
   string goes out with `comb level` times its comb, the second line feeds
   the comb (`comb * feedback + held`). Gains move by 0.05 a step to their
   targets, mixes by 0.001.
4. The output is the pair between the last two steps, times the
   amplitude, in f64; a voice quiet (under 1e-8) for 1000 samples is
   killed.

## 5. The effects (`0x18002c700`)

In this order on the host's rate (all `asm`, `emu`):

- A highpass at 32 Hz, q 1, each side (`0x180019d50` design: `b0 = (c +
  1) / (s / q + 2)`, `a1 = c / (s + q) * (-2 q)`, `a2 = (q - s) / (s +
  q)`; `0x180019e50` run; both flushing under 1e-9). `s, c` from the
  engine's `sincosf`.
- The resonators (`0x180024f20`): eight, each two `CStringDelay` lines of
  up to 0.1 s (`0x180023640`, `0x1800257e0`), the second 1.05 times as long
  and fed the first's output turned over, panned left and right in turn,
  then a 32 Hz highpass each side.
- The chorus (`0x180024d50`): three `CTapeDelay` lines (`0x1800239b0`) of
  up to 22, 17 and 20 ms on the sum, left, both and right.
- The reverb (`fxReverb`, process `0x180028f50`, coefficients
  `0x1800273e0`, layout `0x180025990`, reset `0x180026320`, knob ranges
  `0x180026530`, default program `0x1800258f0`): input highpass and
  lowpass, four allpasses, a predelay, early reflections from taps at
  `0x180074190`/`0x1800741d0`, and a figure-of-eight tank of two allpasses
  and a delay a side, with 14 output taps. The output is `in * dry +
  (difference + sum)` on the left and `in * dry + (sum - difference)` on
  the right.
- The delay (`0x18002cbe0` setup): three `CTapeDelay` lines in a row a
  side, the second and third at Blur and Blur squared of the wet, synced
  to the tempo from `0x18002cc60` when Delay sync is set.

## 6. The engine's maths

What the port had to take from the DLL to the bit (`emu`):

- `sinf` (`0x180058720`, four lanes; the AVX build `0x180059060` does the
  same sums): `|x|` less `n` pi in four parts (`0x40490000`, `0x3a7da000`,
  `0x34222000`, `0x2cb4611a`), `n` by the 1.5 * 2^23 trick, a degree 9
  polynomial (`0xbe2aaaa6`, `0x3c088766`, `0xb94fb7ff`, `0x362edef8`), all in
  f32. Past 10000 an exact reduction, which nothing in Sakura reaches.
- `sincosf` (`0x1800579d0`): `(sinf(x), sinf(|x| + pi/2))`, and `(x, 1 -
  x^2 / 2)` under 2^-13. The cosine is often a step off the true one.
- `sin` (`0x180057a20`; the AVX build `0x1800580a0` has no FMA either) and `sincos`
  (`0x180057960`): the same in f64 with a degree 15 polynomial, exact
  reduction past 2^24.
- `powf` (`0x180044590`), `expf` (`0x18005aac0`), `tanf` (`0x18005bbd0`),
  `cosf` (`0x18005a5c0`): the runtime's, which work in f64 and round once.
  The port does the same with Rust's f64 functions; Rust's own f32 `powf`
  parts from it (one preset's string mix was a step off). On 1500 random
  arguments each the DLL's agree with these but for two cosines. On a CPU
  with FMA3 the runtime takes other builds of these four, which part from
  the SSE2 ones the emulator runs on about one argument in many thousands.
- The LFO's `cos` (`0x18005a040`, f64): Rust's `cos`.
- `pow` (`0x180042ef0`, f64), for the pitch (`0x18002e650`: `2^x`), the
  lo-cut's corner, the damping and the noise gate: not correctly rounded.
  On 4000 arguments of `2^x` it parts from Rust's `powf` in the last bit
  on 6. Most of those vanish when the result is rounded to an f32; the
  voice's step keeps f64, so a state whose pitch moves (the matrix on
  pitch, or the LFO) can drift by a step. One preset of the 254 at key 72
  did, by 8e-7 at most.

## 7. What the port does not do as FL does

- The noise generator: the engine's is one static for every Sakura FL has
  loaded, drawn in the order FL renders them; the port gives each instance
  its own, starting where FL's first does.
- The four runtime f32 maths functions above are taken as rounding once
  from f64, which the DLL's SSE2 builds do on every argument tried; their
  FMA3 builds were not read.
- The runtime's f64 `pow` is Rust's, which parts from it in the last bit
  on about one argument in 700 (above).
- The voices' lines are allocated for the lowest key with the state's
  octave and ratio; a parameter change that asks for longer lines gets the
  longest the voice holds, and `fidelity` says so. No shipped state does.
- The mod wheel, pitch wheel and aftertouch sources read 0: FL's piano roll
  sends none.
