# GMS (Groove Machine Synth): state, parameters, signal chain

Recovered from `Plugins/Fruity/Generators/GMS/GMS_x64.dll` (FL Studio 2026,
MSVC C++ with RTTI, image base 0x180000000). GMS is FL Groove's synth
(`GMSynth`, `GMSynthVoice`, `CSynthesizer`, `CVoiceGenerator`) in the C++
plugin wrapper (`CPluginWrapper`, `TCPPFruityPlug`) that Drumpad and
BassDrum use too (`docs/plugins/drumpad.md`). Every function from
0x1800c6000 to 0x180107000 was decompiled to one file with
`tools/ghidra/DecompileRange.java`, and IPP's FFT at 0x180001000..0x180003000
apart; the asm of each function cited here was read beside it with a
disassembler that resolves rip-relative constants. Constants are given as
the binary holds them.

Corpus: 10 states in 6 projects (the render test `System/Render
tests/GMS.flp` and five demo songs), all `Syn4`, 4968 bytes. 306 presets ship
as `.gmsynth` files (the chunk below without `SSLF`, magic `Syn3`). The Rust
is `crates/plugins/src/gms.rs` and `crates/plugins/src/gms_fx.rs`.

Confidence tags: `asm`, `loader`, `corpus`, `inferred`.

## 1. How it is wired

| what | address |
|---|---|
| wrapper state save/load: `SSLF`, u32 length, the synth's chunk | `FUN_1800c82f0` |
| wrapper `ProcessParam`: set `int * 1/256` clamped 0..1, get `int(256 v)` | `FUN_1800c8800` |
| wrapper render: chunks of the output buffer's size, song position | `FUN_1800c94e0` |
| wrapper per-key slots: left/right gain, pitch, mod X/Y | `FUN_1800c91b0` |
| synth note on / off from a key and a velocity byte (`v / 127`) | `FUN_1800ccd70` / `FUN_1800ccdb0` |
| `GMSynth` vtable, constructor | `0x18014c940`, `FUN_1800f0b20` |
| `GMSynthVoice` vtable, constructor, kill | `0x18014cb28`, `FUN_1800f0650`, `FUN_1800fe890` |
| chunk load / save | `FUN_1800f4e60` / `FUN_1800f43d0` |
| wave file to tables | `FUN_1800f1570` |
| set sample rate | `FUN_1800f12d0` |
| render (vtable slot 17) | `FUN_1800f32f0` |
| block setup: LFO rates, effect settings, master EQ | `FUN_1800f5f00` |
| per block, per voice: pitch, tables, unison steps and gains | `FUN_1800f68f0` |
| per block, per voice: envelopes, LFOs, oscillators | `FUN_1800f7530` |
| note on: voice pick | `FUN_1800f5250` |
| voice trigger: envelopes, unison phases | `FUN_1800f53e0` |
| note off | `FUN_1800f58e0` |
| parameter with the modulation matrix | `FUN_1800f50c0` (raw: `FUN_1800cd7a0`) |
| matrix destination numbers | `FUN_1800ef810` |
| FL's noise generator, and as a float | `FUN_1800ef8c0`, `FUN_1800ef8e0` |
| effect unit: constructor, rate, process, LFO | `FUN_1800fefe0`, `FUN_1800ff2c0`, `FUN_1800ff500`, `FUN_1800fea00` |

The wrapper renders in chunks of its output buffer's length less 4 (the Rust
takes 512) and adds the synth's two buffers to FL's. Before each chunk it
tells the synth the song position, `pos += tempo / 60 * frames / rate` beats
while playing (`asm`).

## 2. State (event 213)

`FUN_1800f4e60`, `loader`, `corpus`:

```
u32  'SSLF' (0x464c5353), u32 len, then len bytes:
u32  magic        'Syn4' 0x53796e34 (FL writes), 'Syn3' 0x53796e33 (presets), 'S4DE' 0x53344445 (demo)
7 x char[0x200]   paths; the first three are oscillators 1..3's waves, each loaded by FUN_1800f1570
                  as it is read (an empty one leaves the table as it was)
i32 x 3           Syn4 and S4DE only: the selected effect (+0x4474), +0x4478, +0x447c
f32 x 212         the parameters while 4 bytes remain
char[0x200]       the preset name, when 0x200 bytes remain after all 212
```

A chunk shorter than the seven paths stops the load. The paths are the
saving machine's (`C:\Program Files (x86)\Image-Line\FL Studio
11\Plugins\Fruity\Generators\GMS\Synth Waves\Saw Mean.wav`). When the file
is not there FL tries its name under a user directory and then the
plugin's own `Synth Waves` (built in the constructor from the DLL's path);
"Devils Horn" is renamed "Double Spike" first. The Rust reads the name from
`<Generators>/GMS/Synth Waves/`, case blind.

## 3. Parameters

Values are 0..1. The synth reads most of them through `FUN_1800f50c0`, which
adds the modulation matrix (section 7); "raw" below means `FUN_1800cd7a0`.
`trunc` is `cvttss2si`.

| # | name | maths |
|---:|---|---|
| 0 | Mix Level | raw; master gain `db(80 v - 60)`, `db(x) = 10^(x/20)`, 0 under -80 |
| 1 | Panning | raw; left `clamp(1 - v) * gain`, right `clamp(v) * gain`, each at least 1e-4 |
| 2 | Pitch | target key `key - trunc(-48 v + 24)` |
| 3 | Note Length | sequencer, not read by the synth |
| 4, 5, 6, 7 | Amp Attack, Decay, Sustain, Release | section 5 |
| 8 | Amp Level | `db(50 v - 25) * 200`; at the trigger the level starts at `v^2` and glides there |
| 9 | Shuffle Enable | sequencer |
| 10 | Slide Time | glide `8.82 / ((2 v^3 + 0.001) rate)` a sample |
| 11 | Slide Enable | a new voice starts at the last note's pitch and glides; off, the pitch is the target |
| 12 | Filter Cutoff | the ladder's table index `/ 65536`; raw value smoothed 1 % a sample |
| 13 | Filter Resonance | the ladder's feedback factor |
| 14 | Filter KBD Tracking | `v (pitch - 57) / 128` onto the cutoff |
| 15 | Filter Type | `trunc(2 v)`: 0 low pass, 1 band (`y3 - y4`), 2 high pass (`s - y4`), else silent |
| 16, 17 | Envelope 1/2 Amount | `v^2`, negative when 18/19 |
| 20, 21 | Envelope 1/2 Attack | `8.82 / ((300 v^5 + 0.001) rate)` |
| 22, 23 | Envelope 1/2 Decay | `d = 0.8 v + 0.2`, `8.82 / ((70 d^5 + 0.001) rate)` |
| 24, 25 | Envelope 1/2 Destination | `trunc(3 v)`: 0 cutoff, 1 modulation, 2 pitch (24 semitones), 3 the same LFO's amount |
| 26, 27 | LFO 1/2 Retrig | a note resets the phase; off and playing, a synced LFO follows the song |
| 28, 29 | LFO 1/2 Amount | `v^2`, negative when 30/31 |
| 32, 33 | LFO 1/2 Shape | `trunc(3 v)`: sine, triangle `0..1`, saw `1 - p`, square `p < 0.5` |
| 34, 35 | LFO 1/2 Rate | free: `(1 - v)^2 25 + 0.01` Hz; synced: section 6 |
| 36, 37 | LFO 1/2 Sync | `trunc(2 v)`: 0 free, 1 synced, 2 synced x4 |
| 38, 39 | LFO 1/2 Destination | `trunc(3 v)`: 0 cutoff, 1 modulation, 2 pitch (12 semitones), 3 amp |
| 40 | Mono Voice | not 0: every note takes voice 0 |
| 41 | Unison Voices | `1 - trunc(-15 v)`, 1..16 |
| 42 | Unison Detune | `d = (0.4 v)^2`; voice `u` at `2^((u / (n - 1) - 0.5) d)` |
| 43 | Unison Stereo | odd voices `-v`, even `+v`, the middle one of an odd count 0; gains `clamp(1 - s)`, `clamp(1 + s)` |
| 44 | Octave | `12 (trunc(-4 v) + 2)` semitones down |
| 45 | Noise Mix | `c = v^2` |
| 46 | Unison Phase | with 47: voice `u` starts at `phase_add(0, 4 u v)` |
| 47 | Unison Phase Retrig | each note sets the unison phases |
| 48, 49 | Mix Oscillator 2, 3 | `a = v48^2`, `b = v49^2`; weights `(1-a)(1-b)(1-c)`, `(1-b)a(1-c)`, `(1-c)b` |
| 50 | Modulation | section 6 |
| 51 | Mod Kind | `trunc(2 v)`: 0 hard sync, 1 phase modulation, 2 ring |
| 52 | Phase Offset Osc 2 | `phase_add(0, 2 v)` on oscillator 2's reads |
| 53 | Phase Retrig Osc 2 | without 47: a note starts its unison voices a random 0..511 steps in |
| 54 | Osc 2 Invert | oscillator 2 negated |
| 55, 57, 59 | Osc 1/2/3 Pitch | `trunc(72 v - 36)` semitones |
| 56, 58, 60 | Osc 1/2/3 Detune | `trunc(100 v - 50)` cents |
| 61..116 | | sequencer steps, not read by the synth |
| 117 + i | FX i X | `clamp(v)` |
| 127 + i | FX i Y | `clamp(v)` |
| 137 + i | FX i LFO Rate | section 8 |
| 147 + i | FX i on | the unit's switch (the name table calls it "Glue") |
| 157 + i | FX i LFO Shape | `trunc(3 v)` clamped 0..4 |
| 167 + i | FX i LFO Sync | not 0: the rate's length x4 |
| 177 + i | FX i LFO Amount | `(2 v - 1)^2` with the sign kept |
| 187..196 | | not read |
| 197, 200 | | 200 not 0: the amp level is multiplied by the velocity; 197 not read |
| 198..204 | modulation matrix | section 7 |
| 205 | | pitch-wheel range, `trunc(12 v)` semitones |
| 206 | master section | not 0: the five-band EQ on |
| 207..211 | master bands | `clamp(v)`, gain `24 (v - 0.5)` |

Defaults, by FL's getter `FUN_1800f1bd0`: 0 at 0.75, 3 at 0.4444, 5, 6, 12,
22, 23 and 200 at 1, 7 at 0.1, 42 at 0.25, 205 at 1/6, 1, 2, 8, 44, 55..60,
198, 201, 203, 207..211, 117..136 and 177..186 at 0.5, 137..146 at 0.5567,
the rest 0 (`inferred` from the getter taken by parameter index).

## 4. The waves (`FUN_1800f1570`)

Each oscillator has 512 tables of 1024 floats. The WAV (`Synth Waves` holds
29, 16-bit mono, 1024 or 2048 frames) is resampled to 1024 points,
`x[i] = hermite(at, frac)` with `pos = i * frames / 1024`, the 4-point
cubic of `FUN_1800dfa30` that interpolates between points `at + 1` and
`at + 2` and reads 0 past the end. IPP's real FFT
(`ippsFFTInitAlloc_R_32f`, order 10, flag 1: forward divided by N) then
`ippsMagnitude` and `ippsPhase` give `mag[k]`, `phase[k]`. Table `h` is the
running sum

```
table[h][j] = table[h-1][j] + mag[h+1] * cos(j * (h+1) * 0.006135923151542565 + phase[h+1])
```

in doubles, stored as floats after each harmonic (`__vdecl_cos2`, `asm`). So
table `h` holds harmonics 1..h+1 at half the wave's amplitude (`mag` of a
cosine of height `a` is `a / 2`), without DC. The Rust's DFT runs in f64
where IPP runs in f32.

A frequency reads table `clamp(trunc(0.5 / f * rate), 0, 511)`, or 511
from `rate / 1024` down (`FUN_1800ef980`).

A read (`FUN_1800ef9d0`) takes one step of FL's noise generator,
`seed = seed * 0x0bb38435 + 0x3619636b` (`FUN_1800ef8c0`, one global for
the process, 0 at load), and reads point `((seed & 0x3fffff) + phase) >> 22`
of the 32-bit phase: a dither of up to one point, no interpolation.

## 5. Notes, voices and envelopes

Velocity: the wrapper hands the key and a velocity byte; `v / 127` goes to
`+0x1068 + 4 key`, and a byte of 0 is a note off (`FUN_1800ccd70`). The Rust
takes the byte as Drumpad does (`bassdrum::velocity_level`), and the key's
left and right gains as the host's circular pan law of the note's pan and
velocity and its pitch offset as the note's fine pitch / 100
(`FUN_1800c91b0` keeps them per key; a voice reads its own key's).

Note on (`FUN_1800f5250`): with 40 at 0 the voice is the last inactive one
of the 99 (`rposition`), else the one with the smallest note counter; with
40 on it is voice 0 whatever it does. Target pitch from 2 and 44, the
velocity, the counter. Then the trigger (`FUN_1800f53e0`):

- level target `v8^2`; the level starts there when it was below 0 (a killed
  voice), then glides 1 % a sample to section 3's level.
- amp: coefficient `8.82 / ((50 a^3 + 0.001) rate)` with `a = v4`, target 5,
  the value kept from the voice's last note; decay `8.82 / ((50 d^4 + 0.001)
  rate)` with `d = v5`, sustain `v6`.
- envelopes 1 and 2: value 0, target 5, attack and decay as in section 3.
- slide (11) and a last pitch: the voice starts at the last note's pitch.
- unison phases: with 47, all four oscillators of voice `u` at
  `phase_add(0, 4 u v46)`; else the random start flag is `v53 != 0`.
- 26, 27: the LFO's phase to 0.

`phase_add(p, t) = (int64(t * 4194303 + 268435392) << 8) + p` in 32 bits
(`FUN_1800ef960`): about `t / 4` of a turn.

Each sample (`FUN_1800f7530`): every envelope `e += (target - e) * coef`;
when the integer part of an envelope is not 0 it is set to 1 and turns to
its decay (envelopes target 0, the amp the sustain). The pitch glides by
the slide coefficient. Note off (`FUN_1800f58e0`) gives every voice of the
key the release coefficient `8.82 / ((25 r^3 + 0.001) rate)`, `r = 0.55 v7 +
0.1`, and target 0. A voice whose amp target is 0 and value under 1e-4 is
killed at the next block (`FUN_1800fe890`), which draws new unison phases
from the noise generator: for each of 16, one value for oscillator 1 and the
sync master, 30 skipped, one for oscillator 2, 30 skipped, one for 3, 30
skipped. The constructor and every set-rate kill all 99.

## 6. A block (`FUN_1800f5f00`, `FUN_1800f68f0`, `FUN_1800f7530`)

LFO rates: free `((1 - v)^2 25 as float) + 0.01` Hz; synced, the length
`FUN_1800efa80(v)` (`trunc(9 v)`: <1 0.125, 1 0.25, 2 0.375, 3 0.5, 4 0.75,
5 1, 6 2, 7 4, >7 8 beats; x4 for sync 2) at `tempo / (60 len)` Hz, and
while playing with retrig off the phase is set to `pos * 0.25 / len` (not
wrapped). FL's second LFO writes the first one's phase (`asm`
0x1800f6005); the Rust keeps that. Steps are `hz / rate`; each sample of
the first sounding voice's pass the shape is read and `phase = frac(phase +
step)`.

Per voice and block (`FUN_1800f68f0`), with the destinations
`trunc(3 v)` of 24, 25, 38, 39:

```
pitch = (trunc(12 v205) * wheel + pitch) + ((e2 + e1) * 24 + (l2 + l1) * 12) - 57 + key offset
base  = 2^(pitch / 12) * 220                      (0.0833333358 as a float)
f_n   = 2^((semis_n + cents_n / 100) / 12) * base
m     = kind 0: clamp(v50 + e1 + ((l2 + l1) + e2)) * 3 + 1; else 1
```

`e`, `l` are an envelope's `amount * value` and an LFO's last value where its
destination is 2 (pitch) or 1 (modulation). Oscillator 1's table follows
`m f1`. Unison voice `u`: steps `m f1 d`, `f1 d`, `f2 d`, `f3 d`
(`FUN_1800ef930`: `int64(clamp(f, 0, 0.4999 rate) / rate * 4294967295.0)`),
gains `gl = clamp(1 - s) keyL`, `gr = clamp(1 + s) keyR` times each weight
times `0.005 / (0.15 (n - 1) + 1)`. With the random start flag, voice `u`'s
oscillators 1 and 2 start `cnt = (rand + 1) & 511` steps in and the sync
master with oscillator 1.

Each sample (`FUN_1800f7530`), after the envelopes: LFO `l_k = shape *
amount` (times envelope k where its destination is 3); for kinds 1 and 2 the
modulation `max(v50^2 smoothed 1 % + modulators at 1, 0)` times 15 (phase)
or 3 (ring); the amp `max(amp^2 + LFOs at 3, 0)`; the cutoff `smoothed v12 +
(v12 modulated - v12 raw) + tracking + modulators at 0`. Then each unison
voice and sample:

```
o2 = read(osc 2, offset + p2)              negated with 54
kind 0: p_sync += step1; on its wrap p1 = p_sync;  o1 = read(osc 1, p1)
kind 1: o1 = read(osc 1, phase_add(p1, o2 * mod))
kind 2: o1 = read(osc 1, p1) * (1 - o2 * mod)
o3 = read(osc 3, p3); n = rand01 - 0.5
L += (o3 gL3 + (n gLn + L)) + (o1 gL1 + o2 gL2)        R likewise
p1 += step0; p2 += step2; p3 += step3
```

and the voice's buffers are multiplied by the amp.

## 7. Modulation matrix (`FUN_1800f50c0`)

Three slots of amount and destination: (198, 199), (201, 202), (203, 204).
Where a slot's `trunc(v_dest)` names the parameter (`FUN_1800ef810`: 1 cutoff
12, 2 resonance 13, 4 modulation 50, 6, 7 envelope 1 attack 20 and decay 22,
8 and 12 envelope 1 amount 16, 10, 11 envelope 2 attack 21 and decay 23, 14,
15 LFO 1 rate 34 and amount 28, 17, 18 LFO 2 rate 35 and amount 29, 20, 21
amp attack 4 and decay 5), it adds `(2 v_amount - 1) * source` and clamps
0..1. Slot 1's source is the velocity of the note being worked on
(`+0x4804`); slots 2 and 3 read `+0x3a14` and `+0x3a08`, which nothing here
sets. Every corpus state has its amounts at 0.5, so the matrix adds 0; some
hold garbage destinations (1.8e31) that `trunc` makes `i32::MIN`.

## 8. The ladder and the sum (`FUN_1800f32f0`)

The constructor builds 65536 pairs, `g = 2^(i * 0.000149133688) * 0.0005`,
`k = (g * 7.5 + 5) * 0.756525` (`asm`). The render gathers the sounding
voices in slot order, two lanes each (left, right), four lanes to an SSE
register; lanes are independent, so the Rust runs voice by voice in the
same order. Per sample, with the pair `(g, k)` at `trunc(cut * 65536)`
clamped to 0..65535 (FL leaves a negative index unset; the Rust takes 0):

```
4 times:  u = x - (k res) y4;  s = u / (0.04 u^2 + 1)
          y1 += (s - y1) g;  y2 += (y1 - y2) g;  y3 += (y2 - y3) g;  y4 += (y3 - y4) g
out += level * (y4 | y3 - y4 | s - y4)
```

Then the effects (section 9), then the master: targets as in section 3,
each side glides 1 % a sample and multiplies its side. FL's set-rate leaves
both gains at -1; the first block sets the left one to the right one's
target and the right one to 0 (`asm` 0x1800f40d4), so the right side
fades in over the first few milliseconds. With nothing sounding and no effect awake the gains
jump to their targets.

## 9. Effects (`FUN_1800ff500`, `crates/plugins/src/gms_fx.rs`)

Ten units in a fixed order, a unit's number its kind. Each block,
`FUN_1800f5f00` sets a unit's switch from 147+i; when on, X, Y, the LFO's
amount, shape and length and, while playing, its phase
`frac(0.25 / len * pos * 4)`. In the render a unit is told it is off while
no voice and no unit before it has sounded. Then, with rate and tempo at
least 1: the LFO's period `len * 60 / tempo` seconds, step `1 / (rate *
secs)`.

The LFO (`FUN_1800fea00`) steps the phase and gives the sine of the old
phase, or of the new one the triangle `2p` folded at 1, the saw `p`, the
square `p > 0.5`, times the amount. The units that read it once a block
then move it on by `frames - 1` steps.

The wet level glides 1 % a sample to 1 when on, 0 when off; a unit stops at
1e-4 (the delay and reverb on their tails).

Biquads: `FUN_1800e4eb0` low pass `b0 = (1 - c) / (s/q + 2)`, `a1 = -2 c q /
(s + q)`, `a2 = (q - s) / (s + q)`, run as `y = b0 (x + 2 x1 + x2) - a1 y1 - a2
y2` (`FUN_1800e4fb0`); `FUN_1800e4cc0` high pass with `b0 = (1 + c) / (s/q +
2)` and `x - 2 x1 + x2`. `(s, c)` is `FUN_18012a3f0`, `c` the sine of `|w| +
pi/2`. Values within 1e-9 of 0 are flushed.

| # | unit | X / Y | what it does |
|---:|---|---|---|
| 0 | Distortion | Distortion / Filter | `FUN_1801000b0`: `d = clamp(lfo + X)`, `k = 1024 d^4`; `lp((x / (|x| k + 1)) 0.5 (k + 1))` at `2^(4.28 Y + 10)` Hz, q 0.99, mixed with the dry by the wet level |
| 1 | Crusher | Crush / Filter | `FUN_180100340`: holds a sample for `64 clamp(1 - X + lfo)^2 + 3` samples, then the same low pass |
| 2 | Flanger | Time / Feedback | `FUN_180100590`, six tape lines: not built |
| 3 | Phaser | Depth / Feedback | `FUN_180100e50`: six first-order allpasses `t = z - a x`, `z = t a + x` with `a = (1 - w) / (1 + w)`, `w = (d + lfo d / 2)(20000 - 50) / rate + 50 / rate`, `d` the smoothed `max(X^2, 1e-5)`, feedback `(0.6 Y)^(1/3) 0.98` of the last output; the wet adds to the dry |
| 4 | Low Pass | Frequency / Bandwidth | `FUN_1800ffc10`: three biquads at `2^(8.28 clamp(lfo + X) + 6)` Hz, `q = 2 Y^2 + 1` |
| 5 | Hi Pass | Frequency / Bandwidth | the same with high passes at `2^(7.424 clamp(lfo + X) + 5)` Hz |
| 6 | Delay | Feedback / Filter | `FUN_1800ff9d0`: `MFXTapeDelay` (`FUN_1800fed00`) a side, 1.5 s of tape, time the LFO's period `* 2/3` of the tape (at least 0.01); in `wet X^2`, feedback `X^2`, the loop through a one-pole low pass `min(2Y, 1)^2` and minus a one-pole of it `max(2Y - 1, 0)^2`; the read adds to the dry |
| 7 | Reverb | Feedback / Filter | `FUN_180100950`: `fxReverb` below on the input times the wet level, added to the dry |
| 8 | Panner | Panning / Binaural | `FUN_180101310`, two lines: not built |
| 9 | Trancer | Release / Depth | `FUN_1801007e0`: not built; carries the master EQ |

The tape (`FUN_1800fed00`): two lists of (sample, tape position) pairs, one
being written a lap and one being read a lap behind it. Each sample the
read head finds the first recorded position at or past its own and
interpolates linearly from the one before (positions as floats, the
fraction in doubles); the head moves `min(speed, 0.2)`, at least `1/len`, a
sample, and on passing 1 the lists swap, the new one starting with the old
read list's last three entries a lap back. `speed = 1 / ((t (1 - 2/len) +
2/len) len)` for `t` the time as a share of the tape, at most 0.99
(`FUN_1800fef90`). Its tail counter resets to 1.5 s of samples whenever a
value passes 1e-4.

`fxReverb` (vtable `0x18014d0d0`, a VST2 `AudioEffect`; set
`FUN_180102170`, update `FUN_180103020`, process `FUN_180103f30`) is the
plate Effector's effect 6 uses (`docs/plugins/effector.md`, read
independently there and here; they agree). GMS sets, each block: 9 Damping
`Y^2`, 8 Decay `0.75 X^3`, 12 Reverb level `0.3 * that + 0.5`, 2 High cut 1
(off), 1 Low cut 0.1 (317 Hz), 5 Diffusion 1, 4 Room 1, 3 Predelay 0, 10 Dry
0 (silent), 16 mute ER 1; the rest stay at the default preset
`FUN_180101530`. Each parameter runs as `floor(v scale + offset)`. Four
input diffusers (142, 107, 379, 277 at 20000ths of the rate) with gains
`exp(-49.34 (l0 + l1) / (m rate))` and `exp(-31.40 (l2 + l3) / (m rate))`, the
second and fourth writing back with the first's gain; a predelay; a tank of
two halves, each a crossover allpass and shelf `(ap + x) (r - 1)/2 + x`, a
damping one-pole, an allpass (672, 908), a delay (4453, 4217), an allpass
(1800, 2656) and a delay (3720, 3163), each fed by the other's end times the
decay `exp(-6.9078 (l5 + l7 + l9 + l11) / rate / rt)^(1/4)`; seven taps a
side. The parameter 17 (mute reverb) is not in the default preset's struct
(heap garbage in FL); the Rust takes it as 0. The unit makes its reverb
after taking its first rate, so the reverb runs at 44.1 kHz until the rate
changes (`asm` 0x1800ff2c0). Its lines start at 1e-30
(`FUN_180101f60`). The early reflections are muted by GMS and not built.
Its tail counter resets to a quarter second of samples.

Master EQ (unit 9 with 206 on, after the unit's own effect): five RBJ
filters a side in series (`FUN_1800e50a0`, run by `FUN_1800e5440`): a low
shelf at 80 Hz, peaks at 220, 1200 and 6000 Hz and a high shelf at 12 kHz,
widths 1, 2.5, 2.8, 2.5, 2 octaves (`alpha = sin w sinh(0.3465736 bw w /
sin w)`), gain `24 (v - 0.5)` dB but `A = 10^(dB/20)`, so a peak moves by
twice the dB its knob says. `w` is clamped to 3.

## 10. What the Rust does not do as FL

- FL's noise generator is one for every plugin in FL's process; here each
  GMS starts its own at 0 and runs it in FL's order. The dither of every
  table read and the noise follow, so they are FL's in kind and not sample
  for sample.
- Flanger, Panner and Trancer are not built: on, they pass the sound and
  `fidelity()` says so. No corpus state switches one on.
- The early reflections of the reverb are not built; GMS mutes them.
- The pitch wheel and the matrix's sources 2 and 3 are 0. Slide notes sound
  nothing.
- The song position an LFO syncs to is counted from the plugin's start or
  reset, as if the song played from the top.
- 16-bit waves are read as `/ 32768`; FL's WAV loader was not read for its
  scale.
- No render has been compared against FL's output.

Validation: unit tests from the maths (biquad responses against their
transfer functions, the EQ's lift, the tape's delay, the plate's decay, the
tables of a sine and a saw, a note's pitch, release and the ladder's
cutoff), and every corpus state loads, plays three notes at a sane level
with fidelity exact. Corpus render: the two silent GMS channels play, 31
silent generator channels where there were 33.
