# FL Slayer: state, engine, signal chain

FL Slayer is reFX's Slayer 2 ("Slayer engine copyright (c) 2003 reFX"):
a guitar and bass synth of six plucked strings, an amp with a cabinet,
and one of fifteen effects. FL's `FL Slayer_x64.dll` (Delphi, 5.7 MB) is
only a shell: its strings name `Slayer_Engine.dll` and say "The plugin
engine couldn't be loaded" when it is missing. The engine is that DLL
beside it, a 32-bit MSVC VST 2 plugin (image base 0x10000000, export
`main`, 179 KB) that FL bridges to. All the sound is in it.

Read with Ghidra 11.4.2 headless: the engine whole (0x10001000..0x10009bc4
is its own code, the rest the CRT), and the shell's DFM with
`tools/plugstate/dfm.py`. Rust: `crates/plugins/src/fl_slayer.rs` (state,
parameters, the sample reader) and `fl_slayer_engine.rs` (the engine).

No shipped project uses FL Slayer and no render test ships for it. The 64
presets in `Data/Patches/Plugin presets/Generators/FL Slayer/` are the
corpus: all 116 bytes, all load and play.

Confidence tags: `asm` (read from the engine's code), `dfm` (the shell's
form), `preset` (the 64 presets), `inferred`.

## The engine object

`main` @ 0x10007ca0 news 0x8298d8 bytes and calls the constructor
`FUN_10007f30`: an `AudioEffectX` of 64 programs and 28 parameters, VMT
0x10013690, the factory programs copied from 0x10015400 (64 x 0x88 bytes: a
24-byte name and 28 floats, the same names as FL's presets but not their
values). `FUN_100081b0` then makes the speaker EQ (`FUN_10009290`, a class
of VMT 0x10013978), sets the rate to 44100 and runs `FUN_10002ad0`, which
builds the tables and empties the buffers.

| offset | what |
|---|---|
| 0x2b6 | a string sounded this block (gates the amp and most effects) |
| 0x24c | blocks of silence while latched; over 3 the strings stop rendering |
| 0x2b0 | the Aid mode last seen, -1 at start |
| 0x4c0, 0x4c8, 0x4cc | glissando base, fret ratio (1.0), the ratio last seen |
| 0x4d0 | pitch bend, -1..1 (0.1224 of the period) |
| 0x4d4, 0x4d8 | fret noise level and saw phase |
| 0x53c, 0x56c, 0x59c | per-voice output SVF: low, band, high |
| 0x5dc | 128 floats: `1 + (key mod 6) / 6`, the pickup spread |
| 0x7f4..0x7fc | the amp's feedback resonator (low, high, band) |
| 0x864 | how hard the strings drive it: `1.5 velocity`, at most 1 |
| 0x868 | 32768 floats: the resonator's output for the strings to read |
| 0x20868, 0x120868, 0x220868, 0x320868 | four 0x40000-float effect buffers |
| 0x420868 | the cabinet's 512-float delay |
| 0x520a68 | 255 floats centred here: `1.0005778^k`, cents of detune |
| 0x520c68 | `[0.25, 0.5, 1, 2]` beats for the synced effects |
| 0x520cc0, 0x520cc4 | the feedback buffer's write and read places; write starts at `1 - trunc(-0.005 * 44100)` = 221 |
| 0x520ccc, 0x520cd0 | MIDI volume and expression, 1.0 |
| 0x520cd4, 0x520cd8, 0x520cdc | the rate, `44100 / rate`, the host tempo |
| 0x520ce8 | six voices of 0x80198 bytes |
| 0x821678 | the excitation samples by index |
| 0x8216f4 | the 28 parameters, floats 0..1 |
| 0x821764 | samples to the next 5 ms control tick |
| 0x821768 | the event queue |
| 0x829774 | the keys held in a solo mode, newest first, and their count at 0x8297f8 |
| 0x8297fc | latch (bit 1, parameter 26) and sustain pedal (bit 0, CC 64) |
| 0x829804, 0x829808 | the strum clock (renders) and how many keys are down |
| 0x82980c, 0x829810, 0x829814 | glide target key, the glided key, its speed |
| 0x8298cc | the speaker EQ object |

A voice (`asm`, `FUN_100032f0`, `FUN_10004140`):

| offset | what |
|---|---|
| +0, +4, +8 | excitation sample index, its rate, its gain |
| +0xc, +0x10, +0x14 | key (float), velocity, the velocity while held |
| +0x18 | detune in cents (signed byte), into 0x520a68 |
| +0x19 | key down |
| +0x1c, +0x20, +0x24 | excitation envelope out, phase, the level it released from |
| +0x28 | gate (1 down, 0 up) |
| +0x2c | envelope stage: 4 attack, 3 decay, 2 off, 1 release, 0 done |
| +0x30, +0x34 | the smoothed amplitude and its target |
| +0xc4, +0xc8 | write place, delay line A (32768 floats) |
| +0x200c8 | the pickup buffer (32768 floats, same place) |
| +0x40148, +0x4014c | write place, delay line B |
| +0x80154, +0x80164 | the loop lowpasses of A and B; +0x80158 the fret noise's |
| +0x80174 | time since release in tone steps (-1 while held, 1e8 dead) |
| +0x80178 | place in the excitation sample |
| +0x8017c | age in renders, for stealing |
| +0x80180..+0x8018c | the last four 44.1 kHz outputs, newest first |
| +0x80190 | the resampling phase |
| +0x80194 | control ticks to wait (strums) |

## 1. State (event 213, 116 bytes)

29 little-endian i32. Word 0 is 0 in every preset; the shell's meaning for
it is not read (`preset`). Words 1..28 are the parameters by the `Tag` of
their control (`dfm`); wheels go 0..65535, selectors count from 0. The
shell hands the engine `value / 65535` for a wheel and `index / (count - 1)`
for a selector; the engine reads a selector back as `trunc(x * (count -
0.01))` (`asm`), which gives the index for every value. That the shell
divides by `count - 1` is `inferred`: the engine's own factory programs
hold selectors that way (Coils 0.5, Amp 0.3333) and the rest of their
values are FL's presets' over 65535.

| tag | name (engine's `getParameterName` table at 0x10017600) | range | FL default | engine reads |
|---:|---|---|---:|---|
| 0 | Strings: Noise, 6String1, 6String2, 6String3, Slap, Ebass, Fretless | 0..6 | 3 | `trunc(x * 6.99)` |
| 1 | Coils: None, Single, Double | 0..2 | 1 | `trunc(x * 2.99)` |
| 2 | Pickup pos | wheel | 17695 | |
| 3 | Tone | wheel | 30147 | |
| 4 | Slap level | wheel | 0 | |
| 5 | Fret level | wheel | 65535 | |
| 6 | Harmonic | wheel | 9175 | |
| 7 | Damp | wheel | 0 | |
| 8 | Vel VScale | wheel | 655 | |
| 9 | Vel DScale | wheel | 0 | |
| 10, 11 | (unnamed, unused) | wheel | 0 | |
| 12 | Amp type: Dry, Tube, EQ, Bandpass | 0..3 | 1 | `trunc(x * 3.99)` |
| 13 | Drive | wheel | 32768 | |
| 14 | Presence | wheel | 32768 | |
| 15 | Feedback | wheel | 0 | |
| 16, 17, 18 | EQ low, mid, high | wheel | 36700, 30147, 26870 | |
| 19 | Cab type: Dry, British, Combo, StaX | 0..3 | 2 | `trunc(x * 3.99)` |
| 20 | FX: Dry, AMP:Phaser, AMP:Tremolo, AMP:WahWah, AMP:WahWah LFO, AMP:Ringmod, AMP:Fuzz Box, AMP:Harmonizer, AMP:Talkbox, MST:Chorus, MST:Flanger, MST:Phaser, MST:Leslie, MST:Tremolo, MST:Dub Delay, MST:Multitap | 0..15 | 0 | `trunc(x * 15.99)` |
| 21, 22 | FX par1, par2 | wheel | 7864, 27525 | |
| 23 | (unnamed, unused) | wheel | 0 | |
| 24 | Aid: None, AutoChords, PwrChords, Strumming, Solo fixed, Solo dynamic | 0..5 | 0 | `trunc(x * 5.99)` |
| 25 | Timing | wheel | 0 | |
| 26 | Latch | 0..1 | 0 | `ftol(x)` (setParameter `FUN_10008530`) |
| 27 | Glissando src: PitchBend, Modwheel | 0..1 | 0 | `trunc(x * 1.99)` |

## 2. Process and control clock (`FUN_100087f0`)

The process sets the FPU to truncate (`_controlfp(0x300, 0x300)`), so every
`fistp` in the engine chops. It renders up to the next control tick
(`FUN_10004140` the strings, then `FUN_10005ef0` the effects), takes the
events due in the next `0.005 rate` samples, advances the tick by `0.005
rate` and steps the envelopes (`FUN_10003fe0`). Events: 1 pitch bend (or
the glissando when Glissando src is Modwheel), 7 volume, 11 expression,
64 sustain, 133 the mod wheel (glissando, or the bend), note on
`FUN_100033d0`, note off `FUN_10003920`. The glissando quantises the bend
to the frets: `0x4c8` is set from a table of 24 ratios (0.5 to 2 in
semitones) and `0x4d4` makes a fret noise when it changes.

The tempo comes from `getTimeInfo` (0x520cdc, 120 without one).

## 3. Notes and voices

`FUN_100051f0` takes the voice already on the key, else one released over
`0.3 rate` ago, else one let go, else the oldest. `FUN_100032f0` starts it:
gate 1, envelope stage 4, sample place 0, amplitude 0, detune from the
event, time -1, and the strum delay. The delay lines are not cleared: a
string plucked again rings on from what it held.

Aid (`FUN_100033d0`, `FUN_10003920`):

- None: one string a key.
- AutoChords: every voice lets go, then the chord of the key's place in the
  octave (table 0x10015318, semitones over 48) strums with
  `trunc(Timing * 10.99)` control ticks between strings.
- PwrChords: every voice lets go; under key 60 the key +12, +17 (5 cents
  up, one strum step later) and +24 (2 cents, two steps), from 60 on -12,
  -5 (-4 cents) and the key (-2 cents).
- Strumming: each key down while others are held starts `count * step -
  clock` ticks late, the clock counting renders since the first.
- Solo fixed and dynamic: a key down while another is held glides the one
  string there (no new pluck) and makes a fret noise of `0.3 velocity`. The
  speed per render is `distance / (T * 200 / tempo)`, with T from `[1.875,
  3.75, 7.5, 15, 30, 60, 120]` by `trunc(Timing * 6.99)`, or for dynamic
  `(D * 200 / tempo) / distance` with D from `[12, 6, 3, 1.5, 0.75, 0.375,
  0.1875]`. A key up glides back to the newest key still held.

A key up releases (stage 1, gate 0) unless latched. Changing Aid releases
every voice, and so does dropping the latch for the keys not held.

The excitation envelope (`FUN_10003fe0`, once a control tick, after any
strum delay): attack adds 19.995 (one tick), decay adds
`20 / (x^3 800)` with `x = (1 - (0.5 Tone + 0.25)) 0.89227 + 0.10773`,
release adds `20 / (decay^3 800)` (the decay's own rate cubed again, as the
x87 code has it) from the level it was at.

## 4. A string (`FUN_10004140`)

Per render and voice that has no delay and is under `0.2 rate` since
release, with `v` velocity, `k` key, and the parameters as floats:

```text
keyed   = (k - 64) / 32 + 1
damp    = ((3 (v-1) / (2 (1-v) + 1) + 1) VelD + (1-VelD)(1-Damp)) keyed   clamped 0.03..0.999
low     = ((128 - k) / 128)^2 16 + 1
period  = 2 * 44100 / (8.17575 2^(glided key / 12)) / fret ratio / (0.1224 bend + 1) * cents(detune)
A, B    = period * 0.9992, period * 1.0008
loss    = ((1 - VelV) + vheld^2 VelV^2) 0.97 keyed   clamped 0.03..1
gain    = (1 - Damp) 0.01 + 0.99
feed    = Feedback^2 0.001
speed   = 0.5 Tone + 0.25 -> (s - 0.5) 6 + 1 above 0.5, else 1.5 s + 0.25
level   = (VelV v + 1 - VelV) speed low
limit   = (1 - 0.95 Slap)^2, makeup = 1 / (1 - 0.98 Slap)
svf c   = min(2 sin((6000 Harmonic + 1000) pi / 44100), 0.99)
pick    = trunc((Pickup + 0.05) spread[k] 100)
Coils   None: no comb; Single: out - t[pick] + t[2 pick]; Double: pick = trunc(pick B 0.002)
```

The string runs at 44.1 kHz whatever the host's rate: a phase gains
`44100 / rate` a host sample and each whole step runs one string sample:

```text
exc   = noise (MSVC rand: (r 3.051e-5 - 0.5) amp level 0.125), or the sample read
        linearly, amp gain level / 32768, its place gaining speed * rate
fret  = when 0x4d4 > 0.005: saw (phase += 0.01) * 0x4d4 (*= 0.9997) through a one-pole of loss
a     = clamp(lerp(A line, A) gain + exc + fb[read] feed, limit)
b     = clamp(lerp(B line, B) gain + exc + fb[read] feed, limit)
lines = one-pole(a, damp), one-pole(b, damp)
out   = clamp(((0.4 b + a) 0.85 + exc Fret) * (released ? amp ramped to 0 in 5 ms : 1), limit)
svf   = Chamberlin, input (out + comb + 1e-18) / 2, feedback 0.5; the low output
```

A 4-point Hermite between the last four outputs (`makeup` times the low)
makes the host's samples. Two lines of twice the period make the string
sound an octave under its key, as a guitar is written. The amplitude ramps
to the envelope (Noise) or to 1 (samples) by `1 / (0.005 rate)` a sample.

The samples, by string type and key (and velocity for Slap), with the
index into 0x821678, the address and the length:

| type | keys | index | rate | gain |
|---|---|---|---|---|
| 6String1 | <50 / <60 / <70 / <80 / rest | 0xe..0x12 | 1 | 0.125 |
| 6String2 | <50, <60 / <70, rest | 0x13, 0x14 / 0x15, 0x16 | 1 | 0.0833 / 0.125 |
| 6String3 | all | 1 | 0.78, 0.975, 1.3, 1.69, 2.21, 2.86 at <48, <56, <62, <70, <78, rest | 0.0417 |
| Slap soft / mid / hard | all | 0x1b / 0x1a / 0x1c | 0.9, 1.125, 1.5, 1.95 at <48, <58, <68 (hard: 1.8, 2.25, 3, 3.9) | 0.0526 / 0.0625 / 0.03125 |
| Ebass | all | 7 | 0.9, 1.125, 1.5, 1.95 | 0.125 |
| Fretless | all | 0x18 | 0.9, 1.125, 1.5 (<68), 1.95 | 0.0417 |

`FUN_10002ad0` points the indexes at 0x100179b8 (1, 2245 samples),
0x10018b48 (7, 1853), 0x100199c8..0x1001fc10 (0xe..0x16: 1860, 1887, 1889,
1513, 1122, 1453, 1293, 1542, 903), 0x10020324 (0x18, 1959), 0x10021278
(0x1a, 1660), 0x10021f74 (0x1b, 1617), 0x10022c1c (0x1c, 4010): an i16
count then the PCM. `Excitations::from_dll` reads them from the DLL file.

## 5. Effects, amp, cabinet (`FUN_10005ef0`)

Per render. The three EQ biquads (`FUN_10002f60`, RBJ, the engine's `pow`
and `sinh`): a low shelf at 500 Hz, a peak at 1000 Hz of 1.5 octaves and a
high shelf at 2500 Hz, each `20 x - 10` dB.

AMP effects, on the mono strings (2..8 only while a string sounds):

| FX | what |
|---|---|
| 1 Phaser | the frequency shifter `FUN_10005d20` (two allpass chains, a quadrature sine of rate `8.82 par1` Hz) with feedback `0.8 par2`, added to the dry |
| 2 Tremolo | a triangle of `tempo / (30 rate) / beats[trunc(3.99 par1)]` turns a sample, depth par2 |
| 3 WahWah | envelope (`0.0049 par1 + 0.995`) into the ladder `FUN_10005400`, cutoff `env par2 3 + 0.015`, resonance 0.75 |
| 4 WahWah LFO | the same with a triangle of `8.82 (par1 + 0.1)` Hz and a slow envelope |
| 5 Ringmod | a sine at `trunc(15 par1 + 1) / 16` of the glided key's frequency |
| 6 Fuzz Box | `(e^{g x} - 1) / g` clipped to ±0.5, `g = 4 (par1 + 0.5)`, quantised to `0.1 par2 + 1e-4` |
| 7 Harmonizer | an octave down: a flip-flop on the falling zero crossings of a lowpassed copy (`FUN_10005550`) |
| 8 Talkbox | three SVF bandpasses whose formants (2250-1400 s, 2900-700 s, a parabola about 270) a triangle of `2.205 par1` sweeps |

The amp, while a string sounds: a one-pole highpass (0.995), then by Amp
type Dry (EQ, `3 drive`), Tube (`FUN_100056a0`: three one-poles of `(0.9
Presence^3 + 0.05)` each into `sin(0.785 clamp(x (32 drive^2 + 1)))`, then
the EQ), EQ (`FUN_100058a0`: the EQ first) or Bandpass (`FUN_10005aa0`:
three SVF bandpasses at `2000 Presence^2 + 300` Hz each into the clipper),
`drive = 0.5 Drive + 0.25`. The amp's output rings a resonator near 2 kHz
(`0x7f4`) that feeds back into the strings `221` samples later, as `Feedback`
says. The cabinet: the ladder `FUN_100052d0` at `cut 44100 / rate` with its
resonance, a comb of `len rate / 44100` samples, mixed and gained:

| cab | len | resonance | mix | gain | cut |
|---|---|---|---|---|---|
| Dry | 20 | 0 | 0 | 0.7 | 0.2 |
| British | 64 | 0.5 | 1 | 1.2 | 0.25 |
| Combo | 32 | 0.6 | 1 | 1.4 | 0.17 |
| StaX | 48 | 0.5 | 1 | 1.0 | 0.33 |

then `volume expression 1.3`. Last the speaker (`FUN_100096d0`): a
one-pole highpass of `176.4 / rate`, a low shelf of +10 dB at 130 Hz
(slope 0.5) and a high shelf of +21 dB at 10 kHz (`FUN_10009500`, set by
`FUN_10009350` from its parameters 0.325, 0.4167, 0.5, 0.5, 0.875).

The left is copied to the right, then the MST effects:

| FX | what |
|---|---|
| 9 Chorus | two taps of a 4096 ring, `trunc(0.00907 rate)` plus a triangle of `4.41 par1` Hz times `0.0029 par2 rate`, one each side, `1.6 (tap + dry) / 2` |
| 10 Flanger | the same with `0.0058 rate`, `1.764 par1` Hz and feedback 0.8 through a one-pole |
| 11 Phaser | the shifter, dry minus left, dry plus right |
| 12 Leslie | two taps read ahead in a 512 ring, each through an SVF lowpass that a triangle sweeps, panned by `par2` (while a string sounds) |
| 13 Tremolo | the synced triangle on both sides (while a string sounds) |
| 14 Dub Delay | `trunc(0.0929 par1 rate) + 1` samples on the left, dry on the right |
| 15 Multitap | taps of `rate 60 / tempo beats[trunc(4 par1)]`, half that and 1.45 ms, feedback `par2` through one-poles |

## What is left out

- Pitch bend, the mod wheel and the glissando: the host sends neither, so
  `0x4d0` stays 0 and `0x4c8` 1. MIDI volume, expression and the sustain
  pedal stay at their defaults.
- The note's pan: the engine is mono up to the MST effect.
- A key up in a solo mode glides with the note-off's velocity, which the
  host has not; 0 is used, so it makes no fret noise.
- The shell is not read beyond its form: the meaning of state word 0, the
  selectors' `index / (count - 1)` and the velocity as `velocity 100 /
  127` are inferred, as is that FL takes the sum the engine adds to its
  output with no gain of its own.
- The speaker EQ runs on the left only, as the engine calls it; the right
  is copied after it.
