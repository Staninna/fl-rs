# FPC: state, kits, voice model

Recovered from `FPC_x64.dll` (FL Studio 2026, Delphi, image base 0x400000, no
engine DLL). Ghidra project `ghidra/proj/fpc`; decompiles under
`ghidra/out/fpc/` (`slots/` the SDK slots with callees, `ctor/` the
constructor, `voice/` the layer chunk and the sample player, `env/` and
`env2/` the envelope class and the control rate, `asm/` listings with the
constant pools). Six Ghidra runs after the import. Corpus: 25 uses in 20 of
the 165 `.flp` and 85 states in shipped `.fst` files, 110 in all. 32 of the
presets are older and save the plugin's name one byte a letter (versions
1000011 x18 and 1000014 x14, 8-bit strings). All 110 states parse to the
last byte: the parser is section 1 written down.

FPC is a drum sampler. A state is 32 pads, each with a MIDI note and a stack
of velocity layers, and every layer names one sample file by path. There is
no kit file format: a kit is an `.fst` preset holding one such state. Nothing
is embedded, so the state is small (17 to 46 KB) and silent without the files.

Confidence tags: `loader`, `corpus`, `preset`, `inferred`, `asm`.

## How the plugin is wired

`CreatePlugInstance` @ `0x00868eb0` -> constructor `FUN_00869010`; class
reference `*0x00868c80` -> VMT `0x00868d48` (`TTestPlug`, instance size
0xcb0, parent `TDelphiFruityPlug` `0x00777ad8`). Plugin info @ `0x008e0f14`:
Flags `0x10e00011`, NumParams 768.

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x0086a970 (4 FPD_SetSampleRate -> `FUN_00870bb0`) |
| 3 | SaveRestoreState | 0x0086d4c0 |
| 4 | GetName | 0x0086efa0 |
| 5 | ProcessEvent | 0x0086de80 |
| 6 | ProcessParam | 0x0086dec0 |
| 8 | Gen_Render | 0x00869ab0 |
| 9 | TriggerVoice | 0x008698a0 -> `FUN_0086b480` |
| 10 | Voice_Release | 0x008700f0 |
| 11 | Voice_Kill | 0x008702b0 |
| 12 | Voice_ProcessEvent | 0x00870420 |

Plugin object:

| offset | meaning |
|---|---|
| 0xbc | host; `+0x68` its stereo scratch buffer |
| 0xc4 | the form; `+0xb34` / `+0xb3c` the MIDI loop list and its index, `+0xb50` and `+0xe74` saved view state |
| 0xcc | `+8` i32 sample rate, `+0x18` f64 `1 / SR` |
| 0x120 | i32 control period in samples (section 3) |
| 0x124 | f64 control periods per 1/96 s, about 0.5 |
| 0x13c | output buffers, one per output number in use |
| 0x14c | 32 pads of 0x56 bytes |
| 0xc84 | the sounding notes |
| 0xc94 | note layout name |
| 0xc9c | u8 window size mode (chunk 3) |

Pad (`this + 0x14c + pad * 0x56`):

| offset | type | meaning |
|---|---|---|
| 0x00 / 0x01 | u8 | mute / solo |
| 0x02 | i32 | volume |
| 0x06 | i32 | pan |
| 0x0a | i32 | tune |
| 0x0e | string | name |
| 0x16 | list | layers |
| 0x1e | i32 | output number |
| 0x22 | i32 | MIDI note |
| 0x26 | u8 | flag bit 2, no reader found |
| 0x27 / 0x2b | i32 | cut group / cut-by group, -1 none |
| 0x2f / 0x37 | object | `TFPCVolEnvelope` (VMT `0x0083bfc0`) / `TFPCPanEnvelope` (`0x0083c160`), both over `TFPCEnvelope` `0x0083be28` |
| 0x44 | u8 | pick one layer per hit |
| 0x45 | i32 | how: 0 random, 1 random but not the last one, 2 round robin backwards, 3 forwards |
| 0x49 | i32 | the layer picked last |
| 0x4d | u8 | muted in effect: `(mute or any pad's solo) and not solo` |
| 0x4e / 0x52 | i32 | colour / icon |

Layer (0x48-byte object, `FUN_00870430` makes one): `+8` path, `+0x10`
resolved path, `+0x18` unresolved flag, `+0x1c` volume (127), `+0x20` pan
(0), `+0x24` the sample, `+0x2c` / `+0x30` velocity low / high (0 / 127),
`+0x34` velocity scales the volume (1), `+0x38` tune (0), `+0x3d` flag bit 1
(no reader found), `+0x44` region index (-1).

## 1. State layout (event 213)

`SaveRestoreState` (`FUN_0086d4c0`). The stream is a version and then
chunks: `i32 id`, `i64 size`, `size` bytes (`FUN_0083c4a0` writes one,
`FUN_0083c750` reads a header). A chunk of size 0 that opens a group is
patched with the group's size on the way out (`FUN_0083ca60`), so pads and
layers nest.

```
i32 version            save writes 0x1e84e403; a value above 0x1fffff is shifted right 8 bits -> 2000100
                       above 0x1e84e403: refused.  > 2000013: strings are UTF-16LE, else 8-bit
                       < 1000011: the old fixed layout (FUN_0086c5e0), not in the corpus and not read
32 x chunk 2           one pad
chunk 1                string: the MIDI loop selected in the browser
chunk 3                u8  window size mode
chunk 5                u8  form+0xb50
chunk 4                0x20 bytes: two i32 (form+0xe74, +0xe78), rest 0
```

Pad (chunk 2, `FUN_0086cc50`):

```
chunk 100   0x40 bytes, 16 i32: volume, pan, flags, note, output, cut, cut by, tune, colour, icon, 6 x 0
            flags: 1 mute, 2 solo, 4 (+0x26), 8 pick one layer
chunk 0x65  string: name
chunk 0x66  0x20 bytes, 8 i32: pick mode, 7 x 0
n x chunk 0x69   one layer (FUN_0086c8f0):
    chunk 300    string: sample path
    chunk 0x12d  0x1c bytes, 7 i32: volume, pan, velocity low, velocity high, flags, tune, region
                 flags: 1 velocity scales the volume, 2 (+0x3d)
    chunk 0x12e  end, size 0
chunk 0x67  8 bytes: i32 0, i32 0     then chunk 0x68: the volume envelope stream
chunk 0x67  8 bytes: i32 1, i32 0     then chunk 0x68: the pan envelope stream
chunk 0x6a  end, size 0
```

The envelope stream is `TMEnvelope`'s, the one Love Philter and Sytrus save
(`docs/plugins/fruity-love-philter.md`): kind 1, version 3, points of 24
bytes, the 20 bytes of flags and markers, four knobs. `active` is 0 or -1.

The table covers the 78 states that name the plugin in UTF-16 (2496 pads,
6415 layers); with the 32 older presets there are 9565 layers.

| field | range seen | most common | confidence |
|---|---|---|---|
| version | 0x1e84e403 x74, 2000014 x4 | | loader, corpus |
| pads per state | 32 | | loader, corpus |
| pad volume | 0..128 | 128 x895, 127 x426, 99 x290 | loader, corpus |
| pad pan | -64..64 | 0 x2000 | corpus |
| pad flags | 0, 1, 4, 8 | 0 x1973, 8 x304, 4 x211 | loader, corpus |
| pad note | 0..127 | 36 to 55 | corpus |
| output | 0..6 | 0 x1938 | corpus |
| cut / cut by | -1, 0, 1 | -1 x2256 | corpus |
| pad tune | 0..73 | 0 x2477 | corpus |
| pick mode | 1, 3 | 1 x2192, 3 x304 (every pad with flag 8) | corpus |
| layers per pad | 1..21 | 1 x1731, 8 x308 | corpus |
| layer volume | 0..127 | 127 x5106, 99 x1063 | corpus |
| layer pan | -33..4 | 0 x6403 | corpus |
| velocity low / high | 0..127 | 0 / 127 | corpus |
| layer flags | 1 | 1 x6415 | corpus |
| layer tune | -54..75 | 0 x6336 | corpus |
| region | -1 | -1 x6415 | corpus |
| volume envelope | 2 or 4 points; active in 400 pads | `(0, 1) -> (1.25, 0)`, tension 0.2 | corpus |
| pan envelope | 1 point at 0.5; active in 338 pads | | corpus |

Paths: 3721 `.wav`, 1336 `.flac`, 54 `.wv`, 1304 empty (a pad with no
sample still has one empty layer). 4343 start with `%FLStudioFactoryData%`,
256 with `%FLStudioData%`, and 512 with `%USERPROFILE%\desktop\fxpansion bfd
data for imageline 24bit\..`. Those 512 are the four version 2000014 presets
(`HQ Funk Kit.fst`, `HQ Jazz Kit.fst`, `HQ Metal Kit.fst`, `HQ Rock Kit.fst`
outside `Plugin presets`), saved on their maker's machine. Every other path,
4599 of them, is a file under `Data`. The shipped kits live in
`Data/Patches/Packs/Drums/Kits/<kit>/` and `Data/Patches/Packs/Legacy/FPC/`;
the plugin's own `paths.dat` lists the legacy folders for its browser.

Loading a layer (`FUN_008708e0`): a path ending `.fldownload.wav` loses the
`.fldownload` (`FUN_00868f30`); a path that is not a file goes to the host
(dispatcher id 0xe), which looks the file up by name; what still fails
leaves the layer on the shared `<empty>` sample. Samples are cached per path
in the plugin's unit (`FUN_00843d90`).

## 2. Parameters (`ProcessParam` @ 0x0086dec0, index decode `FUN_00872b90`)

| index | parameter | range | stored at |
|---|---|---|---|
| `0x00 + b * 0x40 + p` | pad volume | 0..127 | pad+0x02 |
| `0x10 + b * 0x40 + p` | pad pan | -128..128 | pad+0x06 |
| `0x20 + b * 0x40 + p` | mute | 0, 1 | pad+0x00 |
| `0x30 + b * 0x40 + p` | solo | 0, 1 | pad+0x01 |
| `0x100 + b * 0x40 + p` | pad tune | -128..128 | pad+0x0a |
| `0x200 + pad * 8 + k` | envelope knob | the knob's own | k 0..3 volume envelope A, D, S, R; 4..7 pan envelope |

`b` is the bank (0, 1) and `p` the pad in it (0..15), so the pad is
`b * 16 + p`. Everything else of the 768 is unused.

```
gain(v)  = (11^(1.28 v / 127) - 1) / 10        // 0x0086c2b0 = 127, 0x008e0fa0 = 11^1.28 - 1 = 20.527, 0x0086c2c8 = 10
pan(v)   = v / 128                              // 0x0086a8d0, -1..1
cents(v) = v * 9.375                            // 0x0086c2d0, 128 steps to the octave
```

`gain` is the host's velocity curve stretched so that 127 lands on what
velocity 128 gives: 99 is 0.994, 127 is 2.053, 128 is 2.105. `0x008e0fa0`
is in `.data` and only ever read (`asm`).

## 3. Voice model

`TriggerVoice(params, tag)` (`FUN_008698a0`):

```
velocity = clamp(round(Host.Voice_ProcessEvent(tag, FPV_GetVelocity) * 128), 0, 127)   // the MIDI velocity
note     = (InitLevels.Pitch + 50) / 100                                              // the key as written
every pad whose note it is gets +0x4d = (mute or anySolo) and not solo
no such pad unmuted: no voice (returns -1)
FUN_0086b480(note, velocity, tag, params)
```

`FUN_0086b480`, for every pad with that note:

```
layers = all of them
if pad+0x44 and there are two or more: one layer among those whose range holds the velocity (FUN_00869db0),
         picked by pad+0x45; with none in range, all of them again
a layer sounds when  low <= velocity < high,  or velocity = 127 = high,  and it has a sample, and the pad is not muted
per sounding layer, a layer voice (0x114 bytes):
    +0x19 = gain(layer volume)          +0x1d = layer pan
    +0x3a = cents(layer tune + pad tune)
    +0x39 = layer+0x34
    a sample player on the layer's sample from its first to its last frame, not looped (FUN_0061b790)
    volume envelope active: run started, +0xa3 = its value;  pan envelope likewise, +0x108 = value * 2
    +0x10c = -period                    // so the first block works the gains out
the pad voice: +0x18 = gain(pad volume), +0x1c = pad pan, +0x2c = output number
cut groups: a pad voice that is sounding from before is cut when its pad's cut-by group is the new pad's cut group and not -1
the note: +0x28 = (60 - note) * 100
```

`Gen_Render` (`FUN_00869ab0`) runs every layer voice through `FUN_0086a2d0`
into the buffer of its pad's output, and hands each output buffer to the host
(slot `+0x128`). A layer voice, per block:

```
pitch = FinalLevels.Pitch + (60 - note) * 100 + cents          // so a pad plays at the file's pitch, moved by the note's
                                                                // fine pitch, the channel's pitch and the tunes
step  = 2^(pitch / 1200) * sample rate of the file / SR         // FUN_0061b6b0
read the sample at that step into the scratch buffer (FUN_0061bf60); a mono file fills both sides
every `period` samples:
    env = volume envelope's value, then the run moves on one tick (vtable+0x58, dt 1.0); a run that is over ends the voice
    p   = clamp(pan envelope value * 2 + pan(layer pan), -1, 1), then that run moves on
    (L1, R1) = ComputeLRVol(pan(pad pan), gain(pad volume))
    (L2, R2) = ComputeLRVol(p, gain(layer volume))
    (L3, R3) = ComputeLRVol(FinalLevels.Pan, FinalLevels.Vol)
    target = (L1 L2 L3 env, R1 R2 R3 env)
    layer+0x34 clear and InitLevels.Vol != 1: target /= InitLevels.Vol          // velocity taken back out
    cut, or over: target = 0
    the gains go from where they are to the target in a straight line over the period (FUN_0086a0d0);
    the first period starts on its target
the voice ends when the player is past the last frame, or when it is cut and its gains are 0
```

The block loop in `FUN_0086a2d0` reads all of the block's frames into the
host's scratch buffer first (`FUN_0061bf60`, which returns how many it read,
fewer where the file ends), then walks them in runs that stop at the next
period edge. A run whose left slope (`+0x31`) is bit-zero goes through the
plain stereo multiply-add (`*0x008e2f70` -> `0x0062f520`) at the target gains.
Any other run goes through the ramped one (`*0x008e1bc0`), and then the
voice moves its start on by `slope * frames` (`+0x29 += +0x31 * n`, in
`float`). The two pointers are filled at load time (`0x006347b0` and
`0x00634890`); on x64 the ramp is the SSE routine at `0x0062f6f0`, and the
other candidate `0x0062f680` only raises "Not implemented in i64/ARM":

```
lanes  = [L, R, L + dL, R + dR]              // two frames at once
step   = [2 dL, 2 dR, 2 dL, 2 dR]            // dL + dL
n & ~7 frames in pairs:  out += src * lanes;  lanes += step
the last n & 7 frames one at a time from the first lane, which then moves on by one slope
```

So FL's own output depends on where its blocks fall: a period cut by a
block edge starts again from `start + slope * n` and not from where the
added-up lanes had got to. The difference is a rounding step of the gain.

The control period (`FUN_00873060(this, 48)`, on every sample rate change):

```
x      = 48 SR / 44100
period = max(4, 4 round(x / 4))                 // 48 at 44100, 52 at 48000
rate   = 2 period / SR                          // every envelope's +0x10: x units per tick, x in half seconds
```

`ComputeLRVol` is the host's (slot 31, `docs/plugins/host-voice.md`):
0.7071 a side in the centre. So a default pad (volume 127) with a default
layer (127) on a new note is `2.053 * 0.7071` twice over what a sampler
channel makes of the same file, +3.2 dB.

The sample player is FL's interpolator, the class the IL samplers share
(`FUN_0061acd0` constructor, the same object as Fruity Convolver's
`FUN_0061f3d0`; `docs/plugins/fl-interpolator.md`). FPC reads the whole
file, not looped, with the end frames held past its ends. A step of
exactly 1 from a whole position copies frames (`+0x75`); anything else goes
through the kernel its quality picks. FPC sets no quality of its own
(`+0xe4` stays 0), so `FUN_0061b0a0` takes the unit's, which the host sets
through `FPD_ProcessMode` (dispatcher `0x0086a970`, message 1: bits 8..15
are the points, `FUN_0062ede0`); before the host says, it is 12 points a
side (`FUN_0062edc0`, mode 4). FL's export settings default to mode 5, 16
points a side, the dialog's "32-point sinc" (`FLEngine_x64.dll` 0xedff62
writes it, the `Interpolation` key at 0xeeb6fc keeps it), and that is what
the Rust takes a render to send; that the render sends that setting is
inferred.

`Voice_Release` (`FUN_008700f0`) releases the two envelope runs of every
layer voice of the note (`FUN_00788ed0`) and does nothing else: a pad with no
envelope plays its file to the end whatever the note's length. `Voice_Kill`
(`FUN_008702b0`) frees the note.

## 4. What stays unknown

- The old fixed state layout below version 1000011 (`FUN_0086c5e0`).
- The quality FL's render sends plugins: the export setting's default is
  read, the code that sends it is not.
- `TFPCEnvelope`'s own overrides: what its values are counted from (0 for
  the volume and 0.5 for the pan are `inferred` from the defaults, a pan
  envelope that is one point at 0.5), and how its release differs from
  Sytrus's.
- Pad flag 4 (`+0x26`) and layer flag 2 (`+0x3d`): saved and loaded, no
  reader in the voice path.
- The region table of a sample (`sample+0x100`, 0x38 bytes a region) that a
  layer's region index points into. Every layer in the corpus has -1.
- The host's lookup of a file by name (dispatcher id 0xe) for the 512 paths
  that are not files.
- Multiple outputs: where the host sends output numbers above 0.

## Implementation notes (crates/plugins/src/fpc.rs)

- The host decodes the files. The plugin names them in
  `Generator::sample_paths` and is handed each in `Generator::set_sample`; a
  path with no file leaves its layer silent and is in the engine's
  unresolved list.
- The engine puts the channel's volume and pan on what the generator makes,
  so `ComputeLRVol(FinalLevels.Pan, FinalLevels.Vol)` is the note's velocity
  times the note's own pan here.
- The key of a note on is taken for `InitLevels.Pitch`. The engine has moved
  it by the channel's root note already, which FL does in `FinalLevels`
  only. Every FPC channel in the corpus has the root it was made with.
- Every output number plays into the channel.
- The ramp is the SSE routine's arithmetic, lane for lane, and the start
  moves on per run as FL's does. A render cut into other blocks than FL's
  differs from FL's by a rounding step of the gain, as FL's own renders at
  two buffer sizes do.
- The engine hands every generator its files when it builds the song
  (`crates/engine/src/graph.rs`), and `prepare` decodes them up front for
  realtime playback (`crates/engine/src/sample.rs`).
