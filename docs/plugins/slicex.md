# Slicex: state, regions, key map, voice

Recovered from `Slicex_x64.dll` (FL Studio 2026, Delphi, image base
0x400000). Ghidra project `ghidra/proj/slicex`; decompiles under
`ghidra/out/slicex/` (`slots/` the SDK slots, `ctor/` the constructor,
`state/` the loader's callees and the voice, `wav/` the WAV writer and the
sample helpers, `art*/` the articulator classes). Corpus: 43 states in 18
`.flp`, 7 in `.fst` presets, 50 in all; 21 carry their sample, 29 link a file.

Confidence tags: `loader`, `asm`, `corpus`, `inferred`.

## How the plugin is wired

`TTestPlug` (VMT `0x879d40`, 0x1358 bytes, over `TDelphiFruityPlug`), built
by `FUN_0087cfc0`.

| slot | SDK name | address |
|---|---|---|
| 3 | SaveRestoreState | 0x0087a8a0 |
| 6 | ProcessParam | 0x0087e4f0 |
| 8 | Gen_Render | 0x00880740 -> per voice `FUN_008843d0` |
| 9 | TriggerVoice | 0x00881d70 |
| 10 | Voice_Release | 0x00883970 |
| 11 | Voice_Kill | 0x00883c20 |

Plugin object: `+0x140` the eight knobs (i32), `+0x160 + d * 0x630` deck `d`
(`+8` its sample, `+0x10` its slices, 0x70 bytes each, `+0x28 + key * 8` the
key map), `+0xdc0 + g * 0x50` articulator `g` (`+0x14` its six items),
`+0x1080` the pitch knob in cents, `+0x1084` the volume, `+0x12f0` the 47
bytes of settings.

## 1. State layout (event 213)

`FUN_0087a8a0`, load side:

```
i32 version            FL writes 0x60003; 0x60004 and above: nothing is read; above 0xffff: >> 16
i32[8] knobs           +0x140: 0, 0, volume (12800 unity), 256, 128, pitch (cents), 256, 256
u8[0x2f] settings      +0x12f0 (0x2e below version 2): i32 layer mode, i32 crossfade mode (3), ...,
                       +0x2a decks kept apart, +0x2b leave the velocity linked
i32[3]                 the selected deck and two view words
2 x deck:
    u8 linked          version 2 on
    string path        when linked or above version 2: u8 length (0xff: then u32), the bytes;
                       control characters at its end go (FUN_00789250)
    not linked:  i32 n, n bytes of a whole WAV file (FUN_00619a30 loads it)
    linked:      the file is loaded from the path (FUN_007af410); a path that fails empties the deck
    u8[0x4c]           the deck's settings (two f64 -1, f64 0, f64 length in beats, ...)
    4 x MEnvelope      the deck's envelopes, kind 6, no class fields (FUN_00790720)
    i32 n, n x 17 i32  slice settings (FUN_008874a0 turns them into floats)
8 x articulator:
    i32[5]             +0xdc0 + g * 0x50 (128, 128, 128, 0, 0)
    6 x item (TGroupArticulator, load FUN_008869a0):
        i32 version (3, not checked), f32[3] (+0x20: base, envelope amount, LFO amount)
        MEnvelope, TMEnvelope class (4 knobs)       +0x10
        MEnvelope, TMLFO class (4 knobs, phase)     +0x18
        4 x MEnvelope, TMMap class                  +0x30, +0x38, +0x40 (velocity), +0x48 (random)
```

All 50 corpus states read to the last byte this way. Every corpus slice has
the 17 words `0, 0, 0, 8000, 0, 12800, 128, 128, 0, 12800, 0, 0, 12800, 0, 0, 0,
12800`. Volume knobs seen: 12800 x45, 13549 x3, 15312; pitch knobs 0, 455, 760.
The item envelopes are switched on in 8 or 9 states (items 1 to 3); no LFO is.

Slice words (`FUN_008874a0`), as the voice reads them:

| word | float | meaning |
|---|---|---|
| 0 | | above 0 a hit calls `FUN_00885150` with it, a cut group (`inferred`) |
| 2 | `/ 6400` | pan, -1..1 |
| 3 | `/ 8000`, then 1 at 1 and `(e^(x ln 3) - 1) / 2` elsewhere | volume |
| 4 | | the articulator of items 0 and 1 (volume, pan) |
| 5 | `/ 12800` | the volume articulator's depth |
| 6, 7 | `/ 256` | mod X, mod Y |
| 8 | | the articulator of items 2 and 3 |
| 9 | `/ 12800` | |
| 10 | as is | pitch, cents |
| 11 | | the articulator of item 4 |
| 12 | `* 8 / 12800` | |
| 13 | `/ 12800` | start delay |
| 14 | `/ 12800` | start offset |
| 15 | | the articulator of item 5 |
| 16 | `* 2 / 12800` | the start offset articulator's depth |

The volume knob is FL's knob curve: `(11^x - 1) / 10` with `x = knob /
12800`, 1 exactly at 12800 (`FUN_0087efe0`; `ln 11` is worked out when the unit starts, at
`0x0063a8d5`).

## 2. Regions

A sample's regions are its own (`sample+0x100`, 0x38 bytes each: `+0` start,
`+8` end or `0x7fff...` for none, `+0x10` name, `+0x18` text, `+0x28` key or
-1, `+0x2c` kind, `+0x30` cue id). In a WAV file they are the `cue ` points,
the `labl` names, the `ltxt` lengths and kinds (`loop`, `rgn `, `beat`,
`dwnb`, ... from `0x8e1574`), and a `LIST tlst` of 24-byte entries
(`'cue ', id - 1, 1, 0xff, key, 0x90, 0...`) for keys. That is how
`FUN_00613570` writes them; the reader was not found, so the reading here is
the writer turned round.

A region's last frame (`FUN_0062a6d0`) is its end, or one before the next
region that starts after it, or the sample's last.

## 3. The key map (`FUN_008855b0`)

For each deck: every region with a key, of both decks unless the decks are
kept apart, goes on its key. Then each of the deck's regions without a key
goes on `base + index` when that key is still empty, with `base`
(`FUN_0062ab40`) 60 for up to 65 regions and `60 - 12 * ((n - 54) / 12)`
(not below 0) above. Slice `i` holds region `i`'s settings.

## 4. A note (`FUN_00881d70`)

```
note  = Round(InitLevels.Pitch * 0.01)
deck  = colour & 1
vel   = Voice_ProcessEvent(FPV_GetVelocity); when +0x131b is clear, FPV_SetLinkVelocity(0)
slices = the deck's key map at the note; none: no voice
with two or more: the layer mode picks (0 all at 1, 1 all at 1/n, 2 by velocity, 3/4 by mod X/Y,
    5 at random, 6 round robin, 7 by time) and the crossfade mode shapes the others
```

Each slice gets a voice (0x97c bytes) with the slice's floats, its six
articulator items, and the shared sample player (`FUN_0061f6c0`) on the region:
`FUN_00620360(player, data + start, ..., length = end - start)`, so the
region's last frame does not sound. The voice's pitch takes `(60 - note) *
100` so a key plays its slice at the file's pitch.

## 5. Rendering (`FUN_008843d0`)

Per block, for a voice past its start delay:

```
articulators tick (TGroupArticulator +0x38 = FUN_00886a20): value = base, + env * amount, + or * LFO
pan  = clamp(slice pan + Final.Pan + articulator 0 * 2 * depth, -1, 1)
vol  = slice volume * Final.Vol * plugin volume * ((articulator 1 - 1) * depth + 1)
gains (FUN_006226e0): pan 0 -> (vol, vol); else ComputeLRVol(pan, vol * sqrt 2) with a part of
       each side sent across
pitch = slice pitch + Final.Pitch + (60 - note) * 100 + pitch knob;
        step = e^(pitch ln 2 / 1200) * file rate / SR (FUN_006200d0)
the player reads the block (FUN_00621cb0); a short read ends the voice
the declicker (FUN_00876880) shapes the first and last samples
the gains ramp over the block (FUN_00637c60) and the host adds the voice to the output
```

Voice_Release puts the voice in state 5 and releases its articulator
envelopes (`FUN_00796380`). A switched-off envelope does not run, so its
state stays below 9 and the voice plays its region out; only running
envelopes that finish end it earlier.

## 6. What stays unknown

- The WAV region reader (the writer was read instead).
- The articulators' running (`FUN_007971e0`), the layer crossfade curves,
  the filter (`FUN_00883cd0`), the declicker's tables and the sample player's
  interpolation and stretching.
- What the deck settings' bytes and the knob words 0, 1, 3, 4, 6, 7 do.

## Implementation notes (crates/plugins/src/slicex.rs)

- Every note plays on deck A.
- Articulators, the declicker, the filter, stretching, reversing, the start
  delay and offset, and the layer modes that pick one slice are left out;
  fidelity says so. With the articulators off a slice plays out as in FL.
- Between frames the file is read through a Hermite curve (`sampler::Wave`).
- A pan other than 0 uses FL's circular pan law with `vol * sqrt 2` and
  sends nothing across.
- A linked file is decoded by the host; its regions are read by the plugin
  from the file under FL's install, found from
  `Generator::set_resource_dir`.
