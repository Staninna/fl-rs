# Edison: when its sound is its input

Edison is FL's audio editor and recorder. Its `Eff_Render` can mute its
input and add its own playback to the output, so it is hosted only for the
states where neither can happen: in fl-rs it is `crates/plugins/src/edison.rs`,
which passes the block through and turns any other state down as unhosted,
so that the host passes it through and counts it.

Read from `Edison_x64.dll` (FL Studio 26.1.7, Delphi, image base 0x400000)
with Ghidra decompiles and capstone listings.

## The way in

`CreatePlugInstance` @ `0x87e880`, class reference `0x87e678`, VMT
`0x87e740` (`TTestPlug`, 0x4a0 bytes): Dispatcher `0x87e8e0`,
SaveRestoreState (slot 3) `0x87f1f0`, ProcessParam (slot 6) `0x881850`,
`Eff_Render` (slot 7) `0x882bb0`. `TFruityPlugInfo` @ `0x902804`: flags
`0x32c08000`, 3 parameters, no `FPF_NoProcess`.

## The state (`0x87f1f0`)

Written as version `0x000d0003`; a version over that is left alone, and a
version over 0xffff is taken by its high word (13). In order:

| bytes | versions | meaning |
|---|---|---|
| i32 | all | version |
| 8 or 12 | 12 from version 5 | the three parameters at `this+0x148` |
| i32, or i64 from version 13 | all | length of the sample's file |
| that many | all | the sample, a WAV file (`0x61a2c0` loads it) |
| a string | 7 on | a name, kept at `sample+0xa0` |
| 36 | all | the editor's view: selection, zoom, `+0x1204` threshold |
| records | 3 on | one per editor region list (`0x788aa0`) |
| 12, or 20 from version 6 | all | the flags from `this+0x414` |

The flags, by offset from `this+0x414`: 0 i32 record mode, 4 to 7 bytes
(6 is the mute-input flag `this+0x41a`), 8 i32 the record length limit (an
index into 0, 1, 3, 5, 10, 30 and 60 minutes at `0x902eb8`), 12 i32 `this+0x420`,
which MIDI notes need above 0, 16 the play-with-the-song flag `this+0x424`. Loading also
stops the playback (`0x8820c0(this, 0, 0, 1)` sets the speed `this+0x1a0` to
0). The parameters only send the editor messages (play, record): they do not
reach the sound themselves.

## `Eff_Render` @ `0x882bb0`

1. While recording (`this+0x410`, which the editor sets), the input is
   appended to the sample at `this+0x3e0`, under the record mode's
   threshold rules. Nothing is written to the output here.
2. With the mute-input flag the input is copied away (`ippsCopy_64s`,
   `[0x9052e0]`, bound by name at `0x5c80a2`) and the output zeroed
   (`ippsZero_64s`, `[0x903cf0]`, bound at `0x5c8216`). Without it,
   `ippsCopy_64s(Source, Dest)`: the output is the input, bit for bit.
3. A file preview (`this+0x3d8`, set only by the editor's message 9 in
   `0x883c50`) is added with `[0x904b50]`, a stereo add (`0x639be0`, chosen
   at `0x63b1d7`).
4. When not recording, and the playback runs (`this+0x215` clear, the speed
   `this+0x1a0` not 0, or the play-with-the-song flag), the sample is
   rendered (`0x622220`) and added with gains (`[0x904900]`, `0x636100`, a
   gain-matrix add) for as many frames as `0x622220` rendered. With the
   flag and the song playing, the place follows the song (`0x882aa0`).
   When the song starts (dispatch 12) with the flag set, `0x883e60` moves
   the playback to the song's place. `0x622220` renders nothing from an
   empty play region, which an empty sample has. A MIDI note on (dispatch
   0x18, with `this+0x420` above 0) and the play button start the playback
   too, through the editor's messages.
5. With `this+0x430`, a meter reads the output (`0x638c60`).

## What fl-rs hosts

A state is hosted when the mute-input flag is clear and the sample's WAV has
an empty `data` chunk: then steps 3 and 4 add nothing whatever starts them,
step 1 writes only to the sample, and the output is the input. Five of the
six corpus states are such: Jay Cactus x Confz - Mic Check Ready (record
mode 3 and play with the song: should it record while the song plays, the
playback is skipped while it does), SeamlessR - Bass Antics Performance,
JuiceBass - SeamlessR - Menagerie (two) and the first of Asher Postman -
Future House's two.

Left passed through, and counted as unhosted:

- A state that mutes the input: none in the corpus. Its output is silence
  plus its playback.
- A state that holds audio: the second Edison of Asher Postman - Future
  House, an 11758-frame sample at 48 kHz, with `this+0x420` at 3 in a
  project that has a MIDI Out channel. Its playback follows MIDI notes and
  the play button, and fl-rs routes no MIDI to effects, so what it adds is
  not known here.

Not followed: were Mic Check Ready's Edison to record and then stop in the
middle of the song (its length limit is 5 minutes), the play-with-the-song
flag would then play back what it recorded.
