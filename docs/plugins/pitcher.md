# Pitcher: state, parameters, signal chain

Recovered from `Pitcher_x64.dll` (FL Studio 2026, "Pitcher 1.3.0 Rev2661",
MSVC C++, image base 0x180000000, 436 kB). The DSP is in this DLL: it loads
no engine DLL, and the only DLL names in it (`oggio_x64.dll`,
`wavpackdll_x64.dll`) belong to the framework's sample loader, which
Pitcher never calls. RTTI names the classes: `CFLWrapper` (FL's side),
`PitcherSynth` (a `CSynthesizer`, the plugin), `PitcherEditor`.

Ghidra project `ghidra/proj/pitcher`, output under `ghidra/out/pitcher/`:
`all.c` is every function decompiled, `alla.c` the same with each
constant-pool load annotated as f32 and f64, `asm/` the listings the maths
below was read from. Corpus: 61 states in 17 of the 165 projects, the
render test and 3 preset copies in the plugin database. All 65 parse.

Confidence tags: `loader`, `asm`, `corpus`, `inferred`.

## How the plugin is wired

`CreatePlugInstance` @ `0x180002380`. `CFLWrapper` VMT `0x18005b328`, in
the FL SDK's order:

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x180002a70 |
| 3 | SaveRestoreState | 0x180002fd0 |
| 6 | ProcessParam | 0x1800034e0 |
| 7 | Eff_Render | 0x180004760 |
| 16 | MIDIIn | 0x180004eb0 |

The wrapper keeps the `PitcherSynth` at `this+0x228` (VMT `0x18005d340`,
constructor `FUN_18001c240`, 0x78d0 bytes) and calls it through its
virtual methods:

| PitcherSynth slot | address | what it is |
|---|---|---|
| +0x08 | 0x180007a40 | note on: `notes[n] = velocity / 127`, then `+0x130` |
| +0x10 | 0x180007a80 | note off: `notes[n] = 0`, then `+0x138` |
| +0x50 | 0x18001d420 | set sample rate, or with 0 a reset |
| +0x88 | 0x18001d5e0 | render one sub-block |
| +0xb8 | 0x18001cab0 | setParameter |
| +0xd8 | 0x180008430 | getParameter |
| +0xe8 | 0x18001d020 | default of a parameter |
| +0x118 | 0x18001d6e0 | getChunk |
| +0x120 | 0x18001d790 | setChunk |
| +0x130 / +0x138 | 0x18001d560 / 0x18001d5a0 | after a note: find the allowed notes again |
| +0x178 | 0x18001c230 | the MIDI port |

`Eff_Render` runs the synth on sub-blocks of the synth's buffer size less
4 (`FUN_1800041c0`), with the MIDI events of the block applied first.

## 1. State layout (event 213, 160 bytes)

`SaveRestoreState` (`FUN_180002fd0`) writes the magic `SSLF`
(0x464c5353), the chunk's length, then the chunk from getChunk. On load it
reads the magic and length, then the chunk, and hands it to setChunk; any
bytes after it are not read. Three states in the corpus (the render test
and two preset copies) carry 4 zero bytes past the chunk.

getChunk (`FUN_18001d6e0`) writes `PTR1` (`PTD1` from an unregistered
copy), parameters 0 to 32 as f32 from getParameter, the 12 scale bytes at
`+0x1670`, and parameter 33. It returns 0x98, 152 bytes. setChunk
(`FUN_18001d790`) reads the magic, calls setParameter for each f32 while 4
bytes are left and up to parameter 32, then takes the scale bytes if 12
are left and parameter 33 if 4 more are, and last finds the allowed notes
(`FUN_18001d9c0`). A registered copy refuses a `PTD1` chunk. Nothing is
range-checked.

| byte | type | idx | parameter | default | corpus | confidence |
|---|---|---|---|---|---|---|
| 0..3 | | | `SSLF` | | x65 | loader, corpus |
| 4..7 | u32 | | chunk length | 152 | 152 (x65) | loader, corpus |
| 8..11 | | | `PTR1` | | x65 | loader, corpus |
| 12..15 | f32 | 0 | Correction Speed | 1 | 0, 0.40..1 | loader, corpus |
| 16..19 | f32 | 1 | Stereo (harmony spread) | 1 | 1 | loader, corpus |
| 20..23 | f32 | 2 | Gender | 0.5 | 0.49, 0.5, 0.5025, 1 | loader, corpus |
| 24..27 | f32 | 3 | Fine Tune | 0.5 | 0.5, 0.5025 | loader, corpus |
| 28..31 | f32 | 4 | Bypass All | 0 | 0 | loader, corpus |
| 32..79 | f32 | 5..16 | Key C .. Key B Bypass | 0 | 0 | loader, corpus |
| 80..95 | f32 | 17..20 | Voice 1..4 Pan | 0.5 | 0.5, 0.5012 | loader, corpus |
| 96..99 | f32 | 21 | not read by the audio path | 0 | 0 | loader, corpus |
| 100..103 | f32 | 22 | formant mode (`BTN Formant`) | 0 | 0, 1 (x7) | loader, asm, corpus |
| 104..107 | f32 | 23 | Note *A* Freq | 0.5 | 0.5 | loader, corpus |
| 108..111 | f32 | 24 | Pitch Reference | 0 | 0, 0.1125, 0.7 | loader, corpus |
| 112..115 | f32 | 25 | MIDI mode (`BTN Midi`) | 0 | 0, 1 (x2) | loader, asm, corpus |
| 116..119 | f32 | 26 | MIDI octaves count (`BTN Octaves`) | 0 | 0, 1 (x2) | loader, asm, corpus |
| 120..123 | f32 | 27 | harmonize (`BTN Harmonize`) | 0 | 0, 1 (x2) | loader, asm, corpus |
| 124..127 | f32 | 28 | root key, 0 C .. 11 B | 0 | 0, 1, 2, 3, 4, 6, 7 | loader, corpus |
| 128..131 | f32 | 29 | scale: 0 Minor, 1 Major, 2 Pentatonic, 3 All, 4 Single | 3 | 0, 1, 3 | loader, corpus |
| 132..135 | f32 | 30 | harmony: the input plays too | 0 | 0 | loader, asm |
| 136..139 | f32 | 31 | harmony: velocity sets a voice's level | 0 | 0 | loader, asm |
| 140..143 | f32 | 32 | lowest pitch: 220, 170, 110, 80, 55, 25 Hz | 2 | 2, 3 | loader, asm, corpus |
| 144..155 | u8[12] | | scale, C first: 1 where the class is in | all 1 | | loader, corpus |
| 156..159 | f32 | 33 | MIDI input port | 0 | 0, 0.0064, 0.0112 | loader, corpus |

Parameters 28 and 29 are what the window shows. The sound goes by the 12
scale bytes, which the window writes from them with `FUN_18001d890`: the
pattern of the scale, rotated by the root. Minor is C D D# F G G# A#, and
the corpus agrees: root 3, scale 0 is saved as D# F F# G# A# B C#.

The corpus's Pitch Reference values of 0.1125 and 0.7 come to 0 (left)
and 1 (each side). Speed 0, which one state in Astes has, is a correction
that takes a third of a second to come two thirds of the way.

## 2. Parameter semantics

### FL's side (`ProcessParam` @ 0x1800034e0)

FL sees each parameter as the f32 times 256, truncated. A value from FL is
divided by 256 and kept from 0 to 1 before it reaches setParameter, so the
whole-number parameters (28, 29, 32) cannot be reached that way.

### setParameter (`FUN_18001cab0`)

Constants from `.rdata` `0x18005d4e8..0x18005d7dc`.

```
0  speed  = 1 - p < 0.01 ? 1 : (double) 8.82f / (SR * (3 (1-p)^3 + 0.001))    -> +0x1688, at the rate of the moment
2  gender = powf(2, 2p - 1)                                                     -> +0x16a0 (0.5 .. 2)
3  fine   = 2p - 1 semitones                                                    -> +0x168c
4  bypassAll = p != 0, then find the allowed notes                              -> +0x1694
5..16 keyBypass[i-5] = (char)(int)p, then find the allowed notes                -> +0x167c..
22 formant = p != 0; restart the shifters at the next render                    -> +0x169c, +0x16a4
23 aFreq  = powf(2, p * 0.16666667f - 0.083333336f) * 55 Hz                     -> +0x1698 (55 Hz +-1 semitone)
24 reference = (int)(p + p): 0 left, 1 each side, 2 right                       -> +0x1690
25 midi, 26 octaves, 27 harmonize = p != 0                                      -> +0x169d, +0x169e, +0x169f
32 lowest = {220, 170, 110, 80, 55, 25}[(int)p], else 0; on a change: resize   -> +0x78ac, +0x78a8
33 port   = ~(int)(p * -256)                                                    -> +0x78b0
```

The others are only stored.

### The sizes (`FUN_18001d0b0`)

```
frame N   = 4 * ((u64)(SR / lowest) >> 2)        // +0x16c8   (400 at 44.1 kHz and 110 Hz)
ring R    = 2 N                                  // +0x37b8, the delay-line shifter
grain L   = (int)(SR * 0.1)                      // +0x3860, the formant shifter's rings
```

and then a reset (`FUN_18001d420` with 0), which clears every follower,
ring and voice and puts each reader `R - 10` ahead of the writer
(`FUN_18001d290`).

### The allowed notes (`FUN_18001d9c0`)

A table of 128 at `+0x172c`, note 9 being the A of `aFreq`, so 0 is a C:

- not MIDI: every octave of each class whose scale byte is set;
- MIDI, octaves off: every octave of each class a held note has;
- MIDI, octaves on: `allowed[n - 12]` for each held note `n` over 12.

`+0x17ac` says whether any note is allowed.

## 3. Signal chain

### The render (`FUN_18001e030`, from `FUN_18001d5e0`)

```
if restart (+0x16a4): FUN_18001d290
harmony = harmonize ? FUN_18001f800() : none                 // once a sub-block
per frame:
    shifts = harmony ? harmony.shifts
                     : [fine + corr[refL], 0, 0, 0], [fine + corr[refR], 0, 0, 0]
    levels = harmony ? harmony.levels : [1, 0, 0, 0] for each side
    out = formant ? FUN_18001f2b0(in, shifts, levels) : FUN_18001ed60(in, shifts, levels)
    if harmony and parameter 30: out += in
    for each side: the detector frame below
    if ++count == N: FUN_18001e810; FUN_18001ea80; sums = 0; count = 0
```

`refL` and `refR` come from the reference: 0 takes both from the left, 1
each from its own side, 2 both from the right.

### The detector, per side

```
dc   = dc + (x - dc) * 0.6                     // f32, +0x37b0
y    = x - dc
16 x stage = stage + ((double) y - stage) * 0.125;  y = (float) stage    // f64, +0x17b0
     // each state under 1e-9 in size is set to 0 after its output is taken
marks[w2] = y                                  // the formant shifter's ring of marks
sums[k] += |history[k] - y|  for k < N         // history oldest first, then shifted on by y
corr = freq == 0 ? 0 : corr + (target - corr) * speed     // +0x1724
```

`FUN_18001e810`, every `N` frames: the sums are divided by their largest.
From lag 2 up (`k` from `N - 2` down to 1, lag `N - k`) the first local
minimum under 0.3 is the period. `freq = SR / period`, kept only from the
lowest pitch to 1000 Hz, and 0 (with the period) otherwise.

`FUN_18001ea80`: with no pitch or no allowed note, the target and the
correction go to 0. Otherwise

```
note    = 9 - 12 log2f(aFreq / freq)
octave  = (int)(note / 12);  class = note - 12 octave
nearest = (int)|note + 0.5|
down, up = nearest if allowed, else the nearest allowed below (stopping at 0) and above (stopping at 127)
goal    = note < down + (up - down) / 2 ? down : up
target  = goal - note
not MIDI: key = (int)(class + 0.5), 0 if 12 or more; if keyBypass[key] or bypassAll: target = 0
```

A class under -0.5 (pitches under 32.7 Hz with the 25 Hz floor) gives a
negative key, which reads the scale bytes before the key bytes; the port
keeps that.

The period is a whole number of samples, so the pitch it measures is up
to `1 / (2 period)` out: 8 cents near 440 Hz at 44 kHz. The correction
takes the measured pitch to the note, so the output is as far off.

### The delay-line shifter (`FUN_18001ed60`)

Per side, four voices, each with a reader into the side's ring of `2 N`:

```
ref 0: right readers = left readers; ref 2: the other way round
rate = period ? powf(2, clamp(shift, -12, 12) / 12) : 1
y = cubic(ring, pos)
rate < 1 and (pos - w) mod R <= 128:      old = pos; pos += period; fade = 128
rate > 1 and (pos - w) mod R >= R - 128:  old = pos; pos -= period (wrapped); fade = 128
if fade > 0: g = fade / 128; fade -= 1; y = (1 - g) y + g cubic(ring, old); old += rate
pos wrapped into the ring; reader = pos + rate
level += (target level - level) * 0.007
out += level * y
then ring[w] = input; w = (w + 1) mod R after both sides
```

`cubic` (`FUN_18001ec70`) is the Catmull-Rom curve through the samples at
`int(pos)` .. `int(pos) + 3`, between the second and the third, so a
reader reads one sample later than its place. With no shift the output is
the input 9 samples late, times the level, which settles a few millionths
under 1.

### The formant shifter (`FUN_18001f2b0`)

TD-PSOLA on rings of `L`: the input, the detector's marks and an output
accumulator. Per side and voice a countdown; ref 0 copies the left
countdowns to the right each frame, ref 2 the other way.

```
rate = period ? powf(2, shift * -1/12) : 1          (no clamp)
if --countdown <= 0:
    if no period or shift == 0:
        grain = hann(256) * input[w2 - 257 ..]; add it at w2 (no squeeze); countdown = 128
    else:
        size = (int)(2 period); half = size / 2
        mark = the lag, from half + 1 on for (int)(period * 1.1) lags back from w2,
               of the highest strict local peak of marks over 0
        if found: grain = hann(size) * input[w2 - half - mark - 70 ..]; add it at w2 squeezed by gender
        countdown = (int)(rate * period)
level += (target level - level) * 0.007             (after the grain, which takes the old level)
then input[w2] = in; out += acc[w2]; acc[w2] = 0; w2 = (w2 + 1) mod L after both sides
```

`hann(n)[i] = (1 - cos(2 pi i / n)) / 2`, `cos` in double. The grain is
added by `FUN_18001f180`: as it is when the squeeze is 0, and otherwise
read at steps of `gender` with a straight line between samples, so a
gender over 0.5 shortens it and raises the formants. The output lags the
input by about two periods and 70 samples, 257 samples where it is quiet.

The hop is cut to a whole number of samples, so the pitch laid down is
`SR / (int)(rate * period)`: a correction of 431.4 Hz to 440 at 44 kHz
comes out at 444.4 Hz.

### The harmony (`FUN_18001f800`)

Only with harmonize on, once a sub-block:

```
levels = 0; shifts = 0; if !parameter 30: levels[side][0] = 1
slots = -1 x 4; weights = 0
MIDI: the first four held notes, low to high; without octaves each as its class,
      a class already in slots 0..2 skipped, weight 2 v^2; with octaves as n - 12
for side, voice: if slot >= 0 and freq[side] != 0:
    from  = class[side] (+ 12 octave[side] with octaves)
    s     = (pan[voice] - 0.5) * stereo
    level = clamp(2 (side == 0 ? 0.5 - s : 0.5 + s), 0, 1), times weight with parameter 31
    shift = slot - from
return parameter 30 != 0      // the input plays beside the voices
```

Without octaves a voice moves the class of the input to the class of its
note, up to 11 semitones either way and not to the nearest octave.

### MIDI

`MIDIIn` (`FUN_180004eb0`) passes a note on to the synth's event buffer
when the message's port byte is the plugin's port; the synth applies the
events of a sub-block before it renders it. In FL the notes come from a
MIDI Out channel set to that port.

## 4. What stays unknown

- `FUN_18002df70`, `FUN_18002c134` (a call of `FUN_18002bfa4` with -1) and
  `FUN_180042200` are taken to be the CRT's `powf`, `log2f` and `cos` from
  their arguments, their constants (1.442695 in the log) and the maths
  around them.
- The synth's buffer size, which sets how long FL's sub-blocks are, was
  not read.
- Whether FL is told of the shifters' latency was not read.
- Parameter 21 and what the window does with parameters 1, 17..20, 28 and
  29 beyond what is above.

## Implementation notes (crates/plugins/src/pitcher.rs)

- Every block of the host is one sub-block. The harmony is worked out
  once a block, so where the host's blocks are not FL's the voices follow
  the detector at other moments.
- The MIDI port and `Pitcher::note` are there, but no host sends notes
  yet: in MIDI mode nothing is allowed and the voices stay off, as in FL
  with no MIDI Out channel playing. `fidelity()` says so whenever MIDI mode
  is on. Two corpus states (Astes, "Pitcher (MIDI)") use it, on inserts
  the corpus render does not reach.
- The buffers are sized once per rate for a lowest pitch of 25 Hz, so a
  change of the lowest pitch asks for no memory. A value of parameter 32
  outside 0..5 leaves the lowest pitch as it was. FL's would set 0 Hz.
- The peak search wraps its neighbours' indexes inside the ring. FL's
  reads past either end.
- A `PTD1` chunk loads, as it does in an unregistered FL.
- No latency is reported.
