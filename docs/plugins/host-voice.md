# FLEngine host: voice levels, pan law, Voice_ProcessEvent

What the host does between a pattern note and the `TVoiceParams` record a
generator reads. Recovered from `FLEngine_x64.dll` (FL Studio 2026, Delphi,
image base 0x400000). Ghidra project `ghidra/proj`; decompiles under
`ghidra/out/hostvoice/` (`dec/` the pan kernels and their init, `dec2/` the
voice set-up from a note, `dec3/` and `callers3/` the note builders,
`const6000/` every function that touches the constant 6000.0, `asm/` listings
with constant pools). Four Ghidra runs. The scripts `DecompileCallers.java`
and `ConstRefs.java` in `ghidra/scripts/` were written for this.

Confidence tags: `asm` (read from the listing), `inferred` (follows from the
code but the writer itself was not read).

## 1. Pan law: `ComputeLRVol` (host slot 31, `FUN_006a3a00`)

```
ComputeLRVol(&L, &R, pan, vol):                       // pan -1..1, vol linear
    pan = clamp(pan, -1, 1)                            // 0x006a3a5c / 0x006a3a60
    if DAT_015f49a4 == 1:  (*DAT_015f4b30)(&L, &R, pan, vol * DAT_015f49ac)   // triangular
    else:                  (*DAT_015f4b28)(&L, &R, pan, vol)                   // circular (default)
```

The two kernels, filled at unit init by `FUN_006a6890` (an SSE pair at
`0x006a6040` / `0x006a60b0` or an x87 pair at `0x006a3990` / `0x006a39c0`,
same maths):

```
circular   (FUN_006a3990, FUN_006a6040):  a = (1 + pan) * pi/4          // 0x013e7538 = 0.7853981633974483
                                          L = cos(a) * vol;  R = sin(a) * vol
triangular (FUN_006a39c0, FUN_006a60b0):  v = vol * sin(pi/4) = 0.70710677 * vol      // DAT_015f49ac = sin(pi/4)
                                          pan >= 0:  R = v;  L = (1 - pan) * v
                                          pan <  0:  L = v;  R = (1 + pan) * v
```

Both laws give 0.7071 per side at centre. Hard left is `vol` under the circular
law and `0.7071 vol` under the triangular one. `DAT_015f49a4` is the pan-law
setting (`FUN_006a3a70` is its setter, nothing else writes it); 0 is the
shipped default, so FL projects use the circular law unless the user changed
the audio setting. The slot wrappers are `FUN_0132acb0` (float pan) and
`FUN_0132acd0` (int pan, scaled by `*0x013e7564 = 1/64`, so the SDK's integer
pan is -64..64).

3x Osc carries a private copy of this unit (`FUN_005fa230`, selector
`DAT_007a2478`); only its own unit init touches the selector, so the
articulation unit's voice pan is always circular. Its per-oscillator pan goes
through the host slot and follows the setting.

## 2. From a pattern note to `InitLevels` (`FUN_00d29aa0`)

The sequencer builds a 0x78-byte note record (defaults from `FUN_00d29bd0`)
from the 24-byte pattern note. The conversion, `asm`:

| note byte | record field | formula | plugin field |
|---|---|---|---|
| key, i16 at +0xc | +0x2c f32 | `key * 100` | `InitLevels.Pitch`, cents, 100 per MIDI note from note 0 |
| fine pitch, u8 at +0x10 | +0x38 i32 | `fine * 10 - 1200` | per-voice offset, cents (see `rec+0x1e0`) |
| release, u8 at +0x12 | +0x3c f32 | `rel / 128` | release velocity (`FPV_GetRelVelocity`) |
| colour / MIDI channel, u8 at +0x13 | +0x40 low nibble | as is | `FPV_GetColor` |
| pan, u8 at +0x14 | +0x24 f32 | `(pan - 64) / 64` | `InitLevels.Pan`, -1..1 |
| velocity, u8 at +0x15 | +0x28 f32 | `v = vel / 100; vol = v == 1 ? 1 : (11^v - 1) / 10` | `InitLevels.Vol` |
| mod X, u8 at +0x16 | +0x30 f32 | `(x - 128) / 128` | `InitLevels.FCut`, -1..1 offset |
| mod Y, u8 at +0x17 | +0x34 f32 | `(y - 128) / 128` | `InitLevels.FRes`, -1..1 offset |

Constants: `*0x013e755c = 0.01`, `*0x015f49ec = ln 11` (computed in
`FUN_006a6890` from `0x006a6f30 = 11.0`), `*0x013e7558 = 0.1`,
`*0x013e7564 = 1/64`, `*0x013e756c = 1/128`.

So velocity 100 (FL's default) is exactly unity gain; velocity 128 gives 2.05,
velocity 64 gives 0.364, velocity 1 gives 0.0024. Other note builders use the
same layout: `FUN_00d298e0` writes `key * 100` into +0x2c and `FUN_00ffa0a0`
writes 6000.0 (middle C, MIDI 60) for the voice of a playlist clip (section 3,
"Channel volume").

Record defaults (`FUN_00d29bd0`): +8 f64 -1.0, +0x10 -1, +0x3c 0.5, +0x44 -1,
+0x48 layer parent 0, +0x68 f64 1.0, +0x70 extra pan 0.0, +0x74 colour
override -1.

## 3. The voice record and `FinalLevels`

`FUN_00d293d0(note, ...)` creates the voice record, `FUN_00d237e0(rec, note,
end, ...)` copies the note into it, `FUN_00d265e0(rec)` computes
`FinalLevels`. `TVoiceParams` as handed to the plugin is `rec + 0x278`:

| rec offset | meaning |
|---|---|
| 0x38 | i32 fine pitch, cents (note +0x38) |
| 0x64 / 0x78 | old-format int levels of Init / Final (`FUN_00d26200`: pan * 64, vol, round(pitch), fcut, fres), only when the plugin lacks `FPF_NewVoiceParams` (`rec+0x237 = (Info.Flags & 0x200000) == 0`) |
| 0x24 | i32 group: the low 32 bits of the channel's address plus the colour (the layer parent's when the note has one); slide notes find their voices by it (section 5) |
| 0x50..0x60 | the levels a slide or a glide is headed for: pan, vol, pitch (`0x58`, `key * 100` until a slide moves it), fcut, fres (`FUN_00d237e0`, `FUN_00d26460`) |
| 0x8c, 0x90..0xa0 | slide: remaining ticks and per-tick steps for pan, vol, fcut, fres, pitch (`FUN_00d26460`, applied by `FUN_00d26250`) |
| 0x1c8, 0x1cc | arpeggiator mode and state (`FUN_00d28b00`, `FUN_00d26f10`; the channel's arpeggiator at `chan+0x2ac`, `+0x6e0`) |
| 0x1e0 | f32 pitch offset, cents: `(float)rec+0x38` at trigger; the arpeggiator moves it per tick (`FUN_00d26f10` mode 2: `+= rec+0x1e8`, rounded) |
| 0x23c | 0 held, 1 let go of, 2 killed (`FUN_00d241e0` writes 1, `0x00d243ce` writes 2) |
| 0x214 / 0x21c | layer parent channel and child index |
| 0x224 | note flags (note +0x40) |
| 0x237 | plugin wants the old int levels |
| 0x240 | velocity linked (`FPV_SetLinkVelocity`) |
| 0x25c | extra pan (note +0x70), added to Final.Pan when non-zero |
| 0x260 | channel |
| 0x26c | colour / MIDI channel: the note's flags (`note+0x40`, byte 19 of the record) `& 0xf`, set by `FUN_00d293d0` through `FUN_00d237e0`'s sixth argument |
| 0x270 / 0x274 | start and end tick; 0x7ffffffc and above means open |
| 0x278..0x288 | `InitLevels`: Pan, Vol, Pitch, FCut, FRes (note +0x24..+0x34) |
| 0x28c..0x29c | `FinalLevels`: Pan, Vol, Pitch, FCut, FRes |
| 0x2a0 | release velocity (note +0x3c) |

Channel levels at `chan+0x198` Pan, `+0x19c` Vol, `+0x1a0` Pitch (cents),
`+0x1a4` FCut, `+0x1a8` FRes.

```
FUN_00d265e0(rec):                                     // called at trigger, per tick (FUN_00d2ab30), on FPV_SetLinkVelocity
    chan = rec.channel
    Final.Vol   = rec.linkVelocity ? Init.Vol * chan.Vol : chan.Vol
    Final.Pitch = Init.Pitch + chan.Pitch + rec+0x1e0
    Final.Pan   = Init.Pan + chan.Pan
    Final.FCut  = Init.FCut + chan.FCut
    Final.FRes  = Init.FRes + chan.FRes
    if rec+0x25c != 0:  Final.Pan = clamp(Final.Pan + rec+0x25c, -1, 1)
    if chan+0x6ed:                                     // key and velocity tracking (Misc tab), `inferred` from the shape
        dv = Final.Vol + chan+0x1bc;  dp = Final.Pitch + chan+0x1dc
        Final.Pan  += chan+0x1c0 * dv + chan+0x1e0 * dp
        Final.FCut += chan+0x1c4 * dv + chan+0x1e4 * dp
        Final.FRes += chan+0x1c8 * dv + chan+0x1e8 * dp
    if rec.layerParent (rec+0x214):
        Final.Vol   *= parent.Vol
        Final.Pitch += parent.Pitch + 6000               // 0x00d26888
        Final.Pan   += parent.Pan;  Final.FCut += parent.FCut;  Final.FRes += parent.FRes
        if childIndex < parent.childCount: Final.Vol *= parent.childGain[childIndex]   // f64 table at parent+0x26c
    if rec+0x237: FUN_00d26200(&Final, rec+0x78)        // old int format
```

### Pitch reference

`Init.Pitch` is absolute (`key * 100`, 6000 for MIDI 60), yet every Delphi
generator treats `FinalLevels.Pitch = 0` as its base note. The 6000 lives in
the channel term: `FUN_00d26570` returns the absolute pitch of a voice as
`Final.Pitch + 6000` (`0x00d265d4`), `FUN_00ff86f0` and `FUN_00ff9d70` read
`chan+0x1a0 + 6000` as the channel's absolute pitch, and the layer branch above
adds 6000 back because both the child's and the parent's channel term carry
the offset. So `chan+0x1a0 = pitch knob (cents) - 6000` (`inferred`: the
knob's writer was not read) and:

```
FinalLevels.Pitch = (key - 60) * 100 + finePitch + channelPitchKnob [+ layer parent's knob]
```

`FinalLevels.Pitch = 0` is MIDI note 60, FL's C5. 3x Osc's oscillator at
0 semitones therefore plays 523.25 Hz for C5 (one octave above a standard
instrument), BooBass plays 65.4 Hz (two octaves below), Fruit Kick starts its
sweep at 1046 Hz with the default Max freq. BassDrum reads `Init.Pitch / 100`
as the MIDI note, which is the absolute field, consistent with the above.

### Channel volume, and the level of a note and of a clip

`FUN_00fe9d20(chan)` fills `chan+0x19c`. The channel volume handler of the REC
dispatcher calls it after it stores the new value. Read from the listing
(`asm`, `0x00fe9d84` to `0x00fe9e4e`):

```
g(x)     = x == 1 ? 1 : (exp(x * ln 11) - 1) * 0.1          // the fader curve of docs/rec-225.md
chan.Vol = g(chan+0x170 / 12800) * g(chan+0x188 / 12800)
```

`chan+0x170` is the volume of 219 `Levels` (@4, 10000 in a new channel).
`chan+0x188` is @4 of 229 `LevelAdjusts`, which the loader moves to
`chan+0x184` (12800 in a new channel, and 127 other values in the corpus). A
channel of type 1 whose byte at `chan+0x32e` is 0 gets 0 instead, and a layer
(type 3) runs the function for each of its children. Constants:
`*0x015e4780 -> 0x013e7584 = 1/12800`, `*0x015e0350 -> 0x015f49ec = ln 11`,
`*0x015e4420 -> 0x013e7558 = 0.1`, `0x00fe9f98 = 1.0`.

So a new channel has `chan.Vol = g(0.78125) = 0.551`, which is -5.2 dB, and a
note on it has `Final.Vol = Init.Vol * 0.551`.

`FUN_00ffa0a0(chan, item, ...)` starts the voice of a clip on the playlist. Its
second argument is the playlist item (pitch at +0x3c, pan at +0x50). It fills
the note record itself (`asm`, `0x00ffa145` to `0x00ffa1b8`):

```
Init.Vol   = (1 / sin(pi/4)) / g(10000 / 12800)             // = 1.41421 / 0.55101 = 2.5666
Init.Pitch = 6000, plus item.pitch * 100 when chan+0x2f0 < 0
extra pan  = item.pan when it is not 0                       // note +0x70
```

`0x00ffa2c8 = 10000.0` is in the function's own constant pool, and
`*0x015e3800 -> 0x015f49a8 = 1 / sin(pi/4)` is the float `FUN_006a6890` stores
at `0x006a6a8c`, next to the `sin(pi/4)` of the triangular law. The division is
at `0x00ffa1af`.

With `ComputeLRVol` in the centre (section 1) that gives, per side, on a new
channel:

| voice | `Final.Vol` | level |
|---|---|---|
| note at velocity 100 | `1 * 0.551` | `0.551 / sqrt(2) = 0.3896`, -8.2 dB |
| playlist clip | `2.5666 * 0.551 = sqrt(2)` | 1.0, the file's level |

`inferred`: the sampler that reads `Final.Vol` is not read. The table takes it
to call `ComputeLRVol(Final.Pan, Final.Vol)` for a note and for a clip alike,
with no factor of its own. The clip's factor would be pointless otherwise.

The mixer uses the same `0x015f49a8`: `FUN_0127b050` calls
`ComputeLRVol(pan / 6400, gain * *0x015f49a8)`, so a centred insert is at its
fader's gain, and then splits the gain of the side it is panned to into a
direct part and a part across (`*0x015e4000 -> 0x013e7540 = 0.5` of what the
far side is under the fader's gain).

## 4. `Voice_ProcessEvent` (host slot 6, `FUN_0132af20(host, rec, id, ptr, flags)`)

| id | SDK name | result |
|---|---|---|
| 1 | FPV_GetLength | `rec+0x274 - rec+0x270` ticks as float; -1 (as int bits) when the end tick is open |
| 2 | FPV_GetColor | `rec+0x26c` |
| 3 | FPV_GetVelocity | `log11(10 * Init.Vol + 1) * 100 / 128`, the inverse of section 2 scaled to 0..1 of 128 (`0x0132b108 = 10`, `0x0132b10c = 1`, `*0x015f49f0 = 1 / ln 11`, `0x0132b110 = 0.78125`) |
| 4 | FPV_GetRelVelocity | `rec+0x2a0` (0..1, default 0.5) |
| 5 | FPV_GetRelTime | `2 * (1 - rec+0x2a0)` (`0x0132b118 = 2.0`) |
| 6 | FPV_SetLinkVelocity | `rec+0x240 = ptr > 0`, then `FUN_00d265e0(rec)` recomputes `FinalLevels` |
| 7 | FPV_GetInfo | fills `{length as id 1, colour, velocity as id 3, (flags & 0x400000) != 0, rec+0x1e0}` |

Everything else returns 0. Note key, fine pitch, pan, mod X/Y and release
never pass through this call; the plugin reads them from `TVoiceParams`
(section 3), and the host refreshes `FinalLevels` once per tick.

## 5. Colours and slide notes

The note record's `+0x40` is byte 19 of the pattern note, whole
(`FUN_00d29aa0`). `FUN_00d293d0`, which makes a voice of a note record,
takes `colour = +0x40 & 0xf` and hands it to `FUN_00d237e0`, which stores it
at `rec+0x26c`; `FPV_GetColor` (section 4) returns it. The same call stores
the group at `rec+0x24`: `(int)channel + colour`, or the layer parent's
`(int)` plus the colour. `TVoiceParams` itself has no colour: a plugin
asks for it, and Harmless does on every `TriggerVoice` (`docs/plugins/harmless.md`).

A slide note is a pattern note whose u32 at @4 has `& 0xf == 8`.
`FUN_00ff56d0`, the pattern player's note builder, makes its record with
`+4` = the note's length in ticks (an end tick for other notes) and `+0x1c
= 0x7fffffff`, and `FUN_00d293d0` sends such a record to `FUN_00d2b660(voices,
note+0x24 (its levels), group, length)` instead of starting a voice:

```
FUN_00d2b660(voices, levels, group, length):
    highest = -1.0                                                   // 0x00d2b7b4
    for v in voices: if v.group == group && v+0x23c < 1 && !(v+0x224 & 0x40000):
        highest = max(highest, v+0x58)                               // the held voices' targets
    any = highest >= 0.0                                             // 0x00d2b7b8
    if !any: highest = 6000.0                                        // 0x00d2b7bc
    levels.Pitch -= highest                                          // the slide note's key * 100, less it
    for v in voices: if v.group == group && v+0x23c <= any:          // held, and let go of when one is held
        FUN_00d26460(v, levels, length, v+0x58 + levels.Pitch)

FUN_00d26460(v, levels, length, pitch):                              // when chan+0x200 < 1
    v+0x8c = length + 1                                              // ticks
    step(x, to) = x == to ? 0 : (to - x) / (length + 1)              // FUN_00d26430, f32
    v+0x90 = step(Init.Pan, levels.Pan);  v+0x94 = step(Init.Vol, levels.Vol)
    v+0xa0 = step(Init.Pitch, pitch)
    v+0x98 = step(Init.FCut, levels.FCut);  v+0x9c = step(Init.FRes, levels.FRes)
    chan+0x398 + 0x34 * colour = levels with pitch                   // the colour's last levels
    v+0x50..0x60 = the same                                          // so v+0x58 = pitch

FUN_00d2ab30(v), every tick:                                         // after FUN_00d26f10 (arpeggio)
    if v+0x8c > 0: v+0x8c -= 1; FUN_00d26250(v)                      // Init += each step that is not 0
    FUN_00d265e0(v)                                                  // FinalLevels again (section 3)
```

So a slide moves every voice of the channel and the colour by the same
cents, the one whose target is highest to the slide note's key, in a
straight line over the slide note's length plus one tick, and moves their
pan, volume and mods to the slide note's. A slide that comes while another
runs starts from where `Init` has got to, for the old target moved by the
new slide. The slide note's fine pitch plays no part (`InitLevels.Pitch` is
`key * 100`, and the fine pitch is in `rec+0x1e0`). With no voice of the
group held nothing moves. `inferred`: that the slide note's tick runs its
first step, which holds when the sequencer starts the tick's notes before
it ticks the voices.

The plugin reads the moved `FinalLevels.Pitch` as its voices run, as it
does a channel pitch that automation moves (`chan+0x1a0`, section 3).

`FUN_00d23db0`, called by `FUN_00d293d0` for every new voice, is the
host's portamento: when the channel's porta switch (`chan+500 & 2`)
differs from the note's porta flag (`+0x40 & 0x10`), the colour's last
levels are known, the slide time `chan+0x6e4` is above 0 and there is no
arpeggio, the voice starts from the colour's last levels and slides to its
own over `chan+0x6e4` ticks by `FUN_00d26460`; otherwise its levels become
the colour's last. Not read further.

## 6. What stays unknown

- The writer of `chan+0x1a0`, and of the pan, cutoff and resonance levels
  next to it. The -6000 is `inferred` from three readers that add it back.
  `chan+0x19c` is read (section 3, "Channel volume").
- The sampler's own use of `Final.Vol`, and where the mixer uses the part of
  an insert's gain that goes across.
- The exact UI names behind the tracking block (`chan+0x6ed`, `+0x1bc..+0x1e8`).
- `FUN_00d2b660` and `FUN_00f0bf70` also use 6000.0 (arpeggiator and MIDI out
  by the look of them); not read.

## Engine implementation notes (crates/engine/src/levels.rs, slide.rs, generator.rs)

- Under `FlExact` a note voice has no factor on top of `Final.Vol`, and a clip has the `sqrt(2) / 0.551` of `FUN_00ffa0a0`, so a new note is 8.2 dB under its sample and a clip is at the file's level. `Approximate` plays both at the file's level.
- Tracking scale is taken as knob/256 per 1.0 of volume or per 6000 cents (a guess). Corpus pan knobs are all 0, so tracking has no corpus check.
- Under `FlExact` channel volume is `FUN_00fe9d20`'s curve, 229's adjustment included, and an insert's pan feeds its far input into its near side. Which input the part across carries is taken from the shape of `FUN_0127b050`, not read.
- The sample file's own root note is not decoded; root note and fine tune come off the channel.
- `plugins::NoteOn::color` is `rec+0x26c`. A slide note starts no voice, on a sampler channel or a plugin channel. `crate::slide` works its steps out from the song, by channel and colour, in f32 as `FUN_00d26460` does; a generator is told each with `Generator::set_note_pitch` on the sample its tick starts on, and a sampler voice plays at the speed the step's cents give from there. The pan, volume and mods a slide also moves are left out, and so is `chan+0x200`, which turns slides off. The layer parent's group is not built (layers do not play).
- A clip on the channel's pitch moves a sounding generator note on every tick it moves the pitch: the engine reads the clip at each tick's first sample and calls `set_note_pitch` where the value differs from the last it told, so a block ends only on those ticks. A controller that drives the pitch is not followed under a sounding generator note. Sampler voices follow the pitch on the 512-sample grid the other controls are read on.
- The host's portamento (`FUN_00d23db0`) is not built: a note's porta flag goes to the generator as `NoteOn::porta`.
