# The channel arpeggiator

Read from `FLEngine_x64.dll` (FL Studio 2026, Ghidra base 0x400000). The
engine plays it in `crates/engine/src/arp.rs`. Confidence tags as in the
other docs: `loader` read from the decompiled C or the disassembly, `dfm`
read from the channel settings form, `corpus` the projects agree,
`inferred` a reading no second source confirms.

FL's host arpeggiates a channel's notes itself, before any generator hears
of them. A sampler and a plugin channel get the same thing: one note on
and note off per step, with the levels of the held note it plays.

## 1. Where the settings live

Event 215 (`ChannelID.Parameters`) is copied to `chan+0x284`, `min(len,
168)` bytes over the defaults a new channel has (`FUN_00fe90d0`). The
arpeggiator is the run at 215 @40 (`loader`):

| 215 | channel | REC control | what | default |
|---|---|---|---|---|
| @40 | `chan+0x2ac` | 0x300 | direction: 0 off, 1 up, 2 down, 3 up and down, 4 up and down with both ends twice, 5 random; any other plays up | 0 |
| @44 | `chan+0x2b0` | 0x301 | range in octaves | 1 |
| @48 | `chan+0x2b4` | 0x302, stored less 2 | chord: below 0 the held notes, 0 the note alone, `n` entry `n - 1` of FL's chord table | -1 |
| @52 | `chan+0x2b8` | 0x303, 450..1447 (`dfm`) | time knob, section 2 | 1024 |
| @56 | `chan+0x2bc` | 0x304, 0..48 (`dfm`) | gate in 48ths of a step | 48 |
| @60 | byte `chan+0x2c0` | none | slide | 0 |
| @92 | `chan+0x2e0` | 0x305 | repeat | 1 |

PyFLP reads @52 as an f32. The loader treats it as an int. For writers
below 3.5.2 it remaps @52 with `FUN_0117bc50` and clamps it to 450..1447;
for writers below 3.3.4 it reduces @60 to 0 or 1. No corpus project is
that old.

The REC dispatcher (`FUN_01046a70`, decompile lines 1743 to 1790) stores
control `0x300 + n` at `chan+0x2ac + 4n`, 0x302 less 2, and 0x305 at
`chan+0x2e0`. After 0x303 it calls `FUN_00fea220`, which works out the step
in ticks again. The gate is read where a step is scheduled, so 0x304 needs
nothing more.

## 2. Time and gate

`FUN_00fea220` sets `chan+0x6e0 = FUN_0117bce0(min(chan+0x2b8, 1446))`,
which is `FUN_01312200(FUN_0117bbc0(t))` (`loader`, `asm`):

```
FUN_0117bbc0(t) = round_even(f64(f32(f32(exp(f64(f32(t / 1024)) * ln 27) - 1) * 48f)) * (1/26))
FUN_01312200(n) = n * DAT_015d89c4 / 48            signed, truncating
```

`DAT_015d89c4` is the project's PPQ shifted right by 2 (`FUN_013111e0`
writes it on load), so `n` counts 48ths of a step of the song, a 16th
note. 1024 is one step, 450 an eighth of one, 1446 four. Values in the
corpus at PPQ 96:

| time | 48ths | ticks |
|---|---|---|
| 450 | 6 | 3 |
| 626 | 12 | 6 |
| 766 | 20 | 10 |
| 916 | 33 | 16 |
| 924 | 34 | 17 |
| 1024 | 48 | 24 |
| 1234 | 96 | 48 |
| 1303 | 121 | 60 |

A step is in ticks, so it follows the tempo as the notes do. 1447, the top
of the knob, is no steps at all (section 4, state 3).

`FUN_00d26eb0` schedules the next step from the tick it runs on:

```
gate = max(1, step * chan+0x2bc / 48)
rec+0x1d4 = now + gate          the note of this step is let go of here
rec+0x1d8 = now + step          the next step
```

Below PPQ 32 the fastest times cut down to a step of 0 ticks (450 at PPQ 24
is 6 * 6 / 48). The gate is still a tick, so the next step comes on the
tick after this one, with `rec+0x1d8` a tick behind it. That tick goes into
the step note's record as its start, but FL reads a voice's start
(`+0x270`) only for its length (`FUN_0132af20`), and a step note has no
end: the note sounds from the tick it is made on. So such an arpeggio plays
a note of one tick on every tick.

## 3. The held notes

Each channel keeps 16 records of 0x34 bytes at `chan + 0x398 + 0x34 *
colour`, one per note colour (`FUN_00fe95e0` makes them). A record holds:

| offset | what |
|---|---|
| +0x14 | the held notes, a list kept in order: removing one shifts the rest (`FUN_00663960` with its third argument 1 sets `+0x18`, which `FUN_00663b80` reads) |
| +0x1c | the master voice |
| +0x24 | the pattern, records of 0x14 bytes: pan, volume, pitch, mod X, mod Y |
| +0x2c | the chord the pattern was last built for, 0 before the first |
| +0x30 | the pattern needs building (1 for a new channel) |

A note starts (`FUN_00d293d0` → `FUN_00d237e0`). `FUN_00d28910` gives its
voice record a mode at `rec+0x1c8`:

- 0 when the arpeggiator is off, or for a note flagged 0x80000, which every
  note the arpeggiator starts is.
- With a chord below 0: 1, the master, when the list is empty; 2 when the
  list holds a note already. Both go into the list (`FUN_00663b10`) before
  the first note of a higher or equal `InitLevels.Pitch` (`rec+0x280`, key
  times 100).
- With a chord of 0 or more: -1, a note with an arpeggio of its own that is
  not in the list.

`FUN_00d28b00` then sets the record up. Every voice gets `rec+0x1d0` (the
place in the pattern) from note `+0x44`, which is -1 (the default
`FUN_00d29bd0` gives it; that a pattern note keeps it is `inferred`, so the
first step plays the pattern's first entry); `rec+0x1e0`, the
cents added to `FinalLevels.Pitch`, from its fine pitch; and `rec+0x200`
and `+0x204` at 2^31. A mode 2 voice makes no sound (`rec+0x22c = -20` and
the voice is not started). The others take the tick as their gate and step,
a master becomes the record's `+0x1c`, and `rec+0x1cc` picks how the steps
play:

| `rec+0x1cc` | when | the voice |
|---|---|---|
| 1 | time below 1447, slide off | is silent (`rec+0x22c = -21`) and starts a note on each step |
| 2 | time below 1447, slide on | sounds, and glides its pitch from step to step |
| 3 | time 1447 | sounds, at the first note of the pattern |

Whenever a note joins or leaves the list, `FUN_00feac10` marks the pattern
for building and has every voice of the channel and colour that has played
a step (`rec+0x200` not 2^31) set `rec+0x210`, to find its place again.

## 4. A tick

`FUN_00d0d4c0` runs once per tick of the song, in this order (`loader`):

1. `FUN_00d0bac0`: every voice through `FUN_00d2aa00`. A voice whose end
   tick (`rec+0x274`) has come is let go of (section 5).
2. The notes due on the tick start (`FUN_00d293d0`).
3. `FUN_00d0cbb0`: every voice through `FUN_00d2ab30`, which runs
   `FUN_00d26f10`, the arpeggio's tick, first. The byte behind
   `PTR_DAT_015e4bd8` (0x154529f, initialised to 1 and never written)
   chooses this place for it over the one in step 1.

So the notes that end on a tick are gone, and the ones that start on it are
in the list, before any arpeggio steps there. A master's first step is on
the tick its note starts.

`FUN_00d26f10`, for a voice whose gate tick has come:

- State 1. With the record's chord at -1 and a note of the step sounding,
  the pattern is built when it needs to be; if it is one note and that is
  the note sounding (`rec+0x200`), the note sounds on and only the next
  step is scheduled. Otherwise the note of the step is let go of, and if the
  step's tick has come, `FUN_00d26b70` moves on to the next note and a note
  record starts it: from `rec+0x1d8`, with no end, the levels of the step
  (`rec+0x1ec`), its pitch `rec+0x1f4 + InitLevels.Pitch`, fine pitch 0, the
  voice's release velocity, its flags with 0xb0000, and its layer. Then
  `FUN_00d26eb0`.
- State 2. On the tick `rec+0x1dc` names (the gate), `FUN_00d26e40` moves
  on and sets a rate of `(target - rec+0x1e0) / (step - gate + 1)` cents a
  tick. On a step, `FUN_00d26eb0` and the next glide at the new gate. Every
  tick from the gate on, `rec+0x1e4 += rate` and `rec+0x1e0 =
  round_even(rec+0x1e4)`. The pitch arrives on the step's tick. At a gate
  of 48 the gate is the step, so the pitch jumps.
- State 3. Moves on once, sets `rec+0x1e0` to the step's pitch, and never
  steps again.

`FUN_00d26b70` moves a voice on:

```
rec+0x1ec..+0x1fc = the voice's own InitLevels (FUN_00d26320)
build the pattern if marked (FUN_00feac50)
if rec+0x210: for each entry k, by f32 cents rounded to an int:
        if |entry.pitch - rec+0x200| <= best: index = k
        if |entry.pitch - rec+0x204| <  best: index = k - 1
n = pattern length
index = direction 5 ? trunc(MT() * 2^-32 * n) : (index + n + 1) % n
chord < 0: rec+0x1ec..+0x1fc = pattern[index]       (the held note's levels)
else:      rec+0x1f4 = pattern[index].pitch
rec+0x200 = rec+0x1f4;  rec+0x204 = pattern[(index + 1) % n].pitch
chord < 0: rec+0x1f4 -= InitLevels.Pitch;  else: rec+0x1f4 += fine pitch
```

Where the list changed, the arpeggio goes on from the entry nearest the
note it played, or the one before the entry nearest the note that was to
come, rather than starting over.

## 5. Letting go

`FUN_00d241e0` lets go of a voice through `FUN_00d23c00`:

- A master whose list still holds a note (its own included) is not let go
  of: its end becomes open. Any other voice stops stepping
  (`rec+0x1cc = 0`).
- A voice in the list leaves it (`FUN_00d23b80`, in order), and its mode
  becomes -1. When the list is empty the master is let go of. Then
  `FUN_00feac10`.
- With the list empty, the voice's sounding step note is let go of.

So the master steps on while any note of its list is held, its own note
long over or not, and its last step note stops as the last held note does.
A voice of mode -1 (a chord of 0 or more) stops with its own note.

## 6. The pattern (`FUN_00feac50`)

```
notes = 1, table = none, record+0x2c = chord
chord < 0: notes = list length; with none held, record+0x2c = 1 and table 0
chord > 0: table = chord - 1, notes = table length + 1
per = notes * repeat
up  = max(range, 1) * per
len = up;  if up > 1: direction 3 -> 2up - 2, direction 4 -> 2up
for i in 0..len:
    j = i;  if up > 1: direction 2: up - 1 - i
                       direction 3, i >= up: 2up - 2 - i
                       direction 4, i >= up: 2up - 1 - i
    octave = j / per;  note = (j % per) % notes
    range 0: octave += min(note, 5)
    entry = 0
    chord > 0: entry.pitch = sum of the first `note` intervals of the table * 100
    chord < 0: entry = InitLevels of the list's note `note`
    entry.pitch += octave * 1200
```

The table is a global array of Delphi int arrays behind `PTR_DAT_015e4f50`
(the piano roll's chord tools read it too, `FUN_00ded3b0`). It is filled at
run time and was not found in the DLL.

Range 0 is a quirk of the code: one round, with each note of it an octave
above the one before, up to five. `Treben - Gimbal and Sinan - RawFL`
channel 28 has it.

## 7. Random

`FUN_00981360` is MT19937 (`DAT_0141e54c` is `{0, 0x9908b0df}`, the usual
tempering), on the global generator behind `PTR_DAT_015e08f8`.
`FUN_01196c40` creates it with seed 0x12bd6aa (19650218) and
`FUN_011d3790`, which clears a project, seeds it with that again. Other
code draws from it too: layers set to random (`FUN_00d29240`),
`FUN_00d18130`, `FUN_0100bbd0`, `FUN_00c4d260`, `FUN_011c7e00`.

## 8. What the engine plays

`engine::arp` runs sections 3 to 6 over the song's notes before it plays,
tick by tick, for every channel whose direction is not 0. The notes of
those channels are replaced with what FL starts: the step notes of a
master, or a sounding master with its pitch moves. The engine then plays
those like any note, on sampler and plugin channels alike. A clip on 0x303
or 0x304 is read at the tick of each step. A render with a time limit
works the arpeggios out up to the first tick on or past it, where it lets
go of what still sounds; the output before the limit is the same.

Built as the code reads: every direction, range (0 included), repeat,
the held notes as chord (-1 and -2, which differ only in the one-note case
of state 1), chord 0, time and gate on the song's PPQ, slide (state 2,
through the same pitch steps a slide note uses, `Slides::glide`), the top of
the time knob (state 3), the order of the list and finding the place again,
masters stepping on past their own note, random with MT19937 seeded
0x12bd6aa when the song starts, and the time remap of writers below 3.5.2.

Not built, and counted by the corpus render as an unplayed arpeggiator,
whose notes play as they are:

- a chord above 0: the chord table is not read;
- a repeat below 1: FL builds an empty pattern and plays the master's own
  pitch twice over, which is not worth copying.

Inferred or left out:

- The random generator's state when a song starts. FL seeds it when a
  project is cleared, and the draws other code makes before the first step
  are not counted.
- Voices that start on the same tick step in the order their notes come in
  the song; FL steps them in the order of its voice pool.
- `FUN_00d26320` adds what a running slide note still has to move to the
  levels a chord step copies. Not played.
- With the channel's portamento on, FL's step notes glide like any note on
  that channel. The engine plays no channel portamento.
- A controller link (not a clip) on 0x303 or 0x304 is counted unsupported:
  the arpeggios are worked out before the song plays.
- The piano roll's live preview (`crates/engine/src/live.rs`) plays notes
  without the arpeggiator.

## 9. Corpus

14 channels in 8 projects have the arpeggiator on. All have a chord of -1
or -2, a direction from 1 to 4 and a repeat of 1 or more, so all 14 play:

| project | channel | plugin | dir | range | chord | time | gate | slide | repeat |
|---|---|---|---|---|---|---|---|---|---|
| Bombs Away ft KARRA - I'm Awake | 29 | 3x Osc | 1 | 2 | -1 | 626 | 26 | 0 | 2 |
| Mehran Abbasi - With You | 12 | Fruity DrumSynth Live | 1 | 1 | -1 | 1303 | 48 | 0 | 6 |
| Mehran Abbasi - With You | 16 | FLEX | 1 | 1 | -1 | 450 | 48 | 0 | 6 |
| Mehran Abbasi - With You | 17 | 3x Osc | 1 | 1 | -1 | 1303 | 48 | 0 | 6 |
| Mehran Abbasi - With You | 20 | FLEX | 1 | 1 | -1 | 1024 | 48 | 0 | 2 |
| nucleon - Steptime | 4 | Sytrus | 2 | 2 | -1 | 1024 | 48 | 0 | 1 |
| nucleon - Steptime | 11 | Sytrus | 1 | 1 | -1 | 1024 | 48 | 0 | 1 |
| Olbaid - Sytrus Essentials | 97 | Sytrus | 1 | 3 | -1 | 766 | 16 | 0 | 1 |
| Synth_dfr - Overcome - Sytrus | 51 | Sytrus | 1 | 1 | -1 | 924 | 48 | 1 | 1 |
| nucleon - Harmless Galactic | 14 | Harmless | 3 | 4 | -1 | 916 | 48 | 0 | 1 |
| JuiceBass - SeamlessR - Menagerie | 7 | Harmor | 3 | 2 | -1 | 1024 | 48 | 0 | 1 |
| Treben - Gimbal and Sinan - RawFL | 13 | 3x Osc | 4 | 1 | -2 | 1024 | 48 | 0 | 2 |
| Treben - Gimbal and Sinan - RawFL | 15 | 3x Osc | 4 | 1 | -2 | 1234 | 48 | 0 | 2 |
| Treben - Gimbal and Sinan - RawFL | 28 | sampler | 1 | 0 | -1 | 1024 | 34 | 0 | 3 |

Clips drive 0x303 on `Harmless Galactic` channel 14 and `With You`
channels 12, 16, 17 and 20, and 0x304 on `With You` channel 20. FLEX is
not built, so channels 16 and 20 of `With You` stay silent.
`Overcome` channel 51 holds two notes at velocity 0 and moves them with
slide notes, so what it plays rests on the slide notes' volume, which the
engine leaves out (`crates/engine/src/slide.rs`).

## Functions read

| address | what |
|---|---|
| `FUN_00f46490` | loader, the 215 branch: copy to `chan+0x284`, version gates |
| `FUN_00fe90d0` | a new channel's defaults |
| `FUN_00fe95e0` | the 16 held-note records |
| `FUN_01046a70` | REC dispatcher, 0x300..0x3FF |
| `FUN_00fea220` | step and other times in ticks, `chan+0x6e0` |
| `FUN_0117bce0`, `FUN_0117bbc0`, `FUN_01312200` | time knob to 48ths, to ticks |
| `FUN_0117bc50` | the time remap for writers below 3.5.2 |
| `FUN_013111e0` | PPQ, and `DAT_015d89c4 = PPQ >> 2` |
| `FUN_00d293d0`, `FUN_00d237e0` | a note's voice record |
| `FUN_00d28910` | the mode, `rec+0x1c8` |
| `FUN_00d28b00` | the voice's arpeggio set-up |
| `FUN_00feac10`, `FUN_00d2b620`, `FUN_00d2b5e0` | mark the pattern and the voices' places |
| `FUN_00feac50` | build the pattern |
| `FUN_00d26f10` | an arpeggio's tick |
| `FUN_00d26b70` | the next note, finding the place, random |
| `FUN_00d26e40` | a glide's rate |
| `FUN_00d26eb0` | schedule the next step |
| `FUN_00d26320` | a voice's own levels |
| `FUN_00d241e0`, `FUN_00d23c00`, `FUN_00d23b80` | letting go |
| `FUN_00663b10`, `FUN_00663b80`, `FUN_00663960` | the list: insert, ordered remove, create |
| `FUN_00d0d4c0`, `FUN_00d0bac0`, `FUN_00d0cbb0`, `FUN_00d2aa00`, `FUN_00d2ab30` | a tick of the song |
| `FUN_00981250`, `FUN_00981360` | MT19937 seed and draw |
| `FUN_01196c40`, `FUN_011d3790` | the global generator's seeds |
