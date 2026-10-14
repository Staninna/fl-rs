# Records 224 (notes) and 233 (playlist items) from the loader

Byte layouts of the two fixed-size record types in an FLP, read out of
FLEngine_x64.dll (FL Studio 2026, image base 0x400000). Offsets are into one
record. Confidence: `loader` means a decompiled function reads or writes that
byte with a clear meaning; `corpus` means the 165 shipped `.flp` files fit it;
`pyflp` means PyFLP says so and nothing in the C contradicts it; `inferred`
means a guess from an initialiser or a default value.

## How a record reaches memory

The 224 and 233 branches of the project loader (`FUN_00f46490`, lines 2586 and
2735) wrap the payload in a memory stream and call
`FUN_00f558e0(target, stream, mode, ver, flags169)`. That function picks a
record size and calls the target's virtual slot 0x38, which every record
channel class shares: `FUN_012df3c0(obj, buf, byteLen, recSize)`.

`FUN_012df3c0` divides `byteLen / recSize` to get the count, resizes the
object's array to `count * stride` (the stride is the object's `+0x10`), and
then either memcpys the whole buffer when `stride == recSize`, or zero-fills
the array and copies `recSize` bytes per record into each `stride`-sized slot.
So the file record is a prefix of the in-memory record, and every in-memory
field reader in the binary tells you the file offset too. Nothing is
reordered.

The classes, found by scanning the Delphi VMTs:

| class | VMT | stride | set in |
|---|---|---|---|
| TRecChannel (base) | 0x12de968 | 0x18 | `FUN_01060220`: `*(obj+0x10) = 0x18` |
| TNoteRecChannel, TLoopingNoteRecChannel (patterns) | 0x1053db8, 0x1053fc0 | 0x18 | inherited |
| TPLRecChannel (playlist, the object at `arrangement+0x14`) | 0x1054168 | 0x58 | `FUN_01062350`: `*(obj+0x10) = 0x58` |

A playlist item is 88 bytes in this build, not 80. The legacy converter
`FUN_00f55510` writes items at `i * 0x58`, the clip-properties dialog copies
items with `memcpy(..., 0x58)` (`FUN_00e2b2f0`, `FUN_00e2b630`,
`FUN_01065520`), and the constructor says 0x58.

## 169 size selection (`FUN_00f558e0`)

```c
if (ver < 0x7a15e8 /* 8.1.0 */ && bit for mode set in DAT_00f55a9c) {
    mode 1: FUN_00f553b0  // 20-byte legacy notes -> 24
    mode 2: FUN_00f55510  // 28-byte (20 below 0x2dde36) legacy items -> 88
}
else if (ver < 0x132afb9 /* 20.99.1 */) {
    recSize = mode 0 ? 0xc : mode 1 ? 0x18 : 0x20;     // 32-byte items
}
else {
    recSize = *(obj+0x10);                             // native: 24 or 88
    if (mode == 2) {
        if ((flags169 & 8) == 0) recSize = 0x50;         // 80
        if ((flags169 & 2) == 0) recSize = 0x3c;         // 60, wins over bit 3
    }
}
```

`flags169` is `local_298` in the loader, zero until a 169 event sets it. So:

| file version | 169 | item size |
|---|---|---|
| below 20.99.1 | ignored | 32 |
| 20.99.1 and up, no 169 or bit 1 clear | 0, 1, 5, 0xd | 60 |
| bit 1 set, bit 3 clear | 7 | 80 |
| bits 1 and 3 set | 0xf | 88 |

Corpus: 54 files below 20.99.1 all divide by 32; 22 files from 20.99.3000 to
24.1.2.4394 have no 169 and divide by 60; 24.1.2.4408 writes 169 = 1 and
60-byte items; 24.2.99 through 25.2.5 write 169 = 7 and 80-byte items; 25.2.99
and 26.1.0 write 169 = 0xf and 88-byte items. No file contradicts the rule.

Notes take the native 24 in every branch, so 169 never changes their size.

## Note record (224), 24 bytes

| offset | size | type | meaning | confidence |
|---|---|---|---|---|
| 0 | 4 | i32 | position in ticks. `FUN_010620e0` takes `max(pos + max(len,1))` as the pattern length; `FUN_012d4140` compares it with `step * PPQ`; property 7 in `FUN_01308b40`. | loader |
| 4 | 2 | u16 | tag word, 0x4000 in every corpus note. Bit 3 is `slide`: `FUN_00c3ed10` (the `flpianoroll.Note` builder) writes `slide = (u32@4 & 0xf) == 8`; 1146 corpus notes have 0x4008. The other bits of 0x4000 have no reader found. | loader for bit 3, corpus for the rest |
| 6 | 2 | u16 | channel index. `FUN_012d4140`, `FUN_012d3e60`, `FUN_01186220` and `FUN_01308810` all do `FUN_00fe8bf0(channels, u16@6)`. The whole u32 at @4 set to -1 marks a deleted record (`FUN_01060930`, compacted by `FUN_012df5b0`). | loader |
| 8 | 4 | i32 | length in ticks; 0 for a step. `FUN_010620e0` clamps it to at least 1 for the pattern length; `FUN_01186220` treats 0 as a step note. | loader |
| 12 | 2 | i16 | key. `FUN_00c3ed10` reads it as a short for `number`; `FUN_00d29aa0` multiplies by 100 for cents; property 0 ("Note pitch") in `FUN_01308b40`. | loader |
| 14 | 2 | u16 | group. `FUN_01061ae0` and `FUN_01061bc0` read the short at @14 and remap it through `FUN_01067850` when pasting; `group` in `FUN_00c3ed10`. | loader |
| 16 | 1 | u8 | fine pitch, 120 = centre, one step = 10 cents. `FUN_00d29aa0`: `@16 * 10 - 1200`; `FUN_00c3ed10`: `pitchofs = @16 - 120`; `FUN_01062250` sets 0x78 on a new note; property 3 "Fine pitch". | loader |
| 17 | 1 | u8 | repeats. Property 8 "Repeats" in `FUN_01308b40` / `FUN_00d5b930`; `repeats` in `FUN_00c3ed10`. Corpus: 0 on all but 86 notes, which have 4, 10, 11, 12 or 13. | loader |
| 18 | 1 | u8 | release 0..128, default 64 (`FUN_01062250` writes 0x40; loader forces 0x40 below 6.5.0). Property 2 "Release". | loader |
| 19 | 1 | u8 | flags. Bits 0..3: note color, 0..15 (`FUN_00c3ed10` `color = @19 & 0xf`; the loader's below-5.3.0 fixup shifts this nibble left by 2). Bit 4 (0x10): porta. Bit 5 (0x20): muted. Bit 7 (0x80): selected. Bit 6 has no reader found. `FUN_012d4140` sets or clears 0x20 when toggling a step. | loader |
| 20 | 1 | u8 | pan, 64 = centre (`FUN_00d29aa0`: `(@20 - 0x40) * k`). Property 4. | loader |
| 21 | 1 | u8 | velocity 0..128, default 100. Property 1 "Velocity". | loader |
| 22 | 1 | u8 | mod X, 128 = centre (`FUN_00d29aa0`: `(@22 - 0x80) * k`). Property 5. | loader |
| 23 | 1 | u8 | mod Y, same scale. Property 6. | loader |

Default record from `FUN_01062250`: all zero, then `@16 = 0x78`, `@18 = 0x40`,
`u32@20 = 0x80806440` (pan 64, velocity 100, mod X 128, mod Y 128). The
engine's six fields (position, channel, length, key, pan, velocity) are
confirmed at the offsets it uses. PyFLP's guesses are right for @16, @18, @20
to @23 and the group at @14; @17 is repeats, not a flag byte, and @19 is a
flag byte that carries color, porta, mute and selection rather than a MIDI
channel on its own (the low nibble is what FL shows as note color, which it
also uses as the MIDI channel).

The loader copies all 24 bytes. The version fixups rewrite @16 (below 3.1.6),
@19's low nibble (below 5.3.0) and @18 (below 6.5.0). Nothing else in the
record is dropped at load.

The `flpianoroll.Note` builder, which doubles as a key for the whole record
(`FUN_00c3ed10`, the string table at 0xc3eec1 has `number length velocity
color pitchofs slide porta group muted selected release repeats`):

```c
*(int *)(param_2 + 0x10) = (int)*(short *)(param_1 + 3);          // @12 key
*(undefined4 *)(param_2 + 0x14) = *param_1;                        // @0  position
*(undefined4 *)(param_2 + 0x18) = param_1[2];                      // @8  length
*(uint *)(param_2 + 0x1c) = (uint)*(ushort *)((longlong)param_1 + 0xe); // @14 group
*(float *)(param_2 + 0x20) = @20 / k;                              // pan
*(float *)(param_2 + 0x24) = @21 / k;                              // velocity
*(uint *)(param_2 + 0x28) = @19 & 0xf;                             // color
*(float *)(param_2 + 0x2c) = @22 / k;  *(float *)(param_2 + 0x30) = @23 / k;  // mod x, y
*(uint *)(param_2 + 0x34) = @16 - 0x78;                            // pitchofs
*(bool *)(param_2 + 0x38) = (param_1[1] & 0xf) == 8;               // slide, u32@4
*(bool *)(param_2 + 0x39) = (@19 & 0x10) != 0;                     // porta
*(bool *)(param_2 + 0x3a) = (@19 & 0x20) != 0;                     // muted
*(bool *)(param_2 + 0x3b) = (@19 & 0x80) != 0;                     // selected
*(float *)(param_2 + 0x3c) = @18 / k;                              // release
*(uint *)(param_2 + 0x40) = @17;                                   // repeats
```

## Playlist item (233), 32 / 60 / 80 / 88 bytes

The first 24 bytes are the note layout: `FUN_01062400` (new item) calls the
note initialiser `FUN_01062250` first, which is why every item carries 120 at
@16, 0x40 at @18 and 64,100,128,128 at @20. Those are note defaults with no
item reader.

| offset | size | type | meaning | confidence |
|---|---|---|---|---|
| 0 | 4 | i32 | position in ticks. The "start time" control of the clip dialog writes it (`FUN_00e2b2f0`: `local_a0 = *(startTimeCtrl+0x3c0)` into the item copy at @0); `FUN_012df950` shifts it. | loader |
| 4 | 2 | u16 | 0x5000 in all 23,520 corpus items (PyFLP's "pattern base"). The C reads the u32 at @4 and only the high half decides anything, so this word is a tag. | corpus, pyflp |
| 6 | 2 | u16 | source. `FUN_01062720`, `FUN_010649c0`, `FUN_01065520`: `u32@4 >= 0x50000000` is a pattern, index `(u32@4 + 0xb0000000) >> 16` = `@6 - 0x5000`; `u32@4 < 0x10000000` is a channel (audio or automation clip), index `u32@4 >> 16` = `@6`, looked up with `FUN_00fe8bf0`. `u32@4 == -1` is a deleted record. | loader |
| 8 | 4 | i32 | length in ticks. Passed to the "length" control (`FUN_00e2c3d0`); recomputed from the trim by `FUN_01064c10`. | loader |
| 12 | 2 | i16 | track word: `track = 500 - @12` (`FUN_01306040` is `return 500 - x`; `FUN_012f3560` mutes every item whose `i16@12 == FUN_01306040(track)`; `FUN_012fc180` lines 122 and 147 do the same lookup). 499 is track 1. | loader |
| 14 | 2 | u16 | group, remapped by the same `FUN_01061ae0` code path as notes (TPLRecChannel inherits the slot). | loader |
| 16 | 2 | u16 | 120, note fine-pitch default, unused by items. | loader (default only) |
| 18 | 1 | u8 | 0x40, note release default, unused by items. This is why the u16 at @18 reads 0x40 on most items. | loader (default only) |
| 19 | 1 | u8 | flags. Bit 5 (0x20): muted. Bit 7 (0x80): selected. Bits 6 and 7 are cleared right after load (`FUN_01060a30`: `@19 &= 0x3f`), so selection and bit 6 never survive a load. Bit 4 (0x10) is set and tested in `FUN_012fc180` (a playlist edit pass); meaning not found. | loader |
| 20 | 4 | u8 x4 | 64,100,128,128, note defaults, unused by items. | loader (default only) |
| 24 | 4 | i32 or f32 | trim start, see "Trim" below. | loader |
| 28 | 4 | i32 or f32 | trim end. | loader |
| 32 | 4 | i32 | item id, -1 when unset. Assigned from a global counter when an item is inserted (`FUN_01062460`: `@32 = FUN_0105f2e0()`), freed when deleted (`FUN_010626c0`), matched by the clip dialog (`FUN_01062720`). The 233 load path does not reassign it. | loader |
| 36 | 16 | ? | four u32 zeroed by `FUN_010657a0(item+0x24)`; the legacy converter also zeroes @36. No reader found. | inferred |
| 52 | 4 | f32 | gain, 1.0 default (`FUN_010657a0` writes 0x3f800000; `FUN_00e2c2b0` resets it; `FUN_00e2b2f0` writes the gain control's dB value here). Corpus: 1.0 on 8,473 of 10,576 80-byte items. | loader |
| 56 | 4 | u32 | 0 by init, no reader found. | inferred |
| 60 | 4 | f32 | pitch in semitones, coarse + fine/100 (`FUN_00e2b2f0`: `local_64 = coarse + fine / 100`; `FUN_00e2c2b0` resets to 0). | loader |
| 64 | 8 | f64 | stretch factor, 1.0 default (`FUN_01064e20`, `FUN_00e2c3d0` "reset stretch factor" writes 1.0 at @0x40; `FUN_010649c0` multiplies audio trim by it). | loader |
| 72 | 4 | u32 | flags: bit 0 reverse (`FUN_01065520` flips it and reverses the trim). Corpus: 0 on all 80-byte items. | loader |
| 76 | 4 | i32 | -1 by init (`FUN_01064e20` `param_1[4] = 0xffffffff`), no reader found. | inferred |
| 80 | 4 | f32 | pan, -1..1 (`FUN_00e2b2f0`: `local_50 = (ctrl*2 - max) / max`; `FUN_00e2c2b0` resets to 0). Only present in 88-byte items. | loader |
| 84 | 4 | u32 | 0 by init, no reader found. | inferred |

The 60-byte item ends after gain; the 80-byte item adds pitch, stretch,
reverse and @76; the 88-byte item adds pan and @84. The engine's position,
source, length and trim offsets are confirmed; its guess that the base at @4
is compared against @6 works because @4 is always 0x5000 and FL's own test is
`@6 >= 0x5000`.

Legacy items (below 8.1.0, `FUN_00f55510`): 28-byte records, 20 below version
0x2dde36. Bytes 0..11 copy straight, legacy 16..31 copy to @16.., then
`@12 = legacy[12] | legacy[14] << 8`, `@16 = @17`, `@17 = @18 = 0`, `@36 = 0`.

### Mute flag

Byte 19 bit 5 (0x20), which is bit 13 (0x2000) of the little-endian u16 at
@18. Set means muted. The engine's guess is right.

The clip-properties dialog, whose controls `FUN_00e2c3d0` names (`+0x850`
and `+0x838` are "mute"), writes it in `FUN_00e2b630` on a 0x58-byte copy of
the item (`local_b0`; `local_9d` is byte 0x13 of that copy):

```c
if ((param_3 == 0) || (param_3 == *(longlong *)(param_1 + 0x850))) {
  iVar1 = *(int *)(*(longlong *)(param_1 + 0x850) + 0x3c0);
  if (iVar1 == 1)      local_9d = local_9d | 0x20;
  else if (iVar1 == 2) local_9d = local_9d & 0xdf;
  else if (iVar1 == 3) { toggle 0x20 }
}
```

and in `FUN_00e2b2f0` from the mute checkbox `+0x838`:

```c
if (*(char *)(*(longlong *)(param_1 + 0x838) + 0x49e) == '\0') local_8d &= 0xdf;
else                                                            local_8d |= 0x20;
```

Accessors: `FUN_0105f3c0(item, bool)` sets or clears `@19 & 0x20`;
`FUN_0105f390` (and its copies `FUN_012ca4e0`, `FUN_010646b0`) is
`(@19 & 0x20) != 0`; `FUN_0105f340` / `FUN_012ca4d0` / `FUN_010646a0` read bit
0x80 (selected). "Reset clip properties" (`FUN_00e2c2b0`) clears 0x20.

Readers outside the dialog, all treating a set bit as "skip this item":

- `FUN_012fc180` line 229: `if ((*(byte *)(local_c0 + 0x13) & 0x20) != 0) *(undefined4 *)(local_c0 + 4) = 0xffffffff;` deletes muted items; lines 123 and 150 require `(@19 & 0x20) == 0` to act on a track's items.
- `FUN_012f3560(arrangement, track, muted)`: `FUN_0105f3c0(item, muted)` for every item with `i16@12 == 500 - track` (mute all clips on a track).
- `FUN_012d3e60` (step sequencer "is this step lit"): a note counts only when `(@19 & 0x20) == 0`.
- `FUN_00c3ed10`: `muted = (@19 & 0x20) != 0` for the scripting API.

The same bit on notes is the piano roll's note mute.

### Selected flag and group

Selected is byte 19 bit 7 (`FUN_00e2c3d0` loops over items with
`(@19 & 0x80) != 0` to find the clips the dialog edits; `FUN_012fc180` line
471 writes `@19 = @19 & 0xdf | 0x80`). `FUN_01060a30` clears bits 6 and 7 of
every item after 233 loads, so files that carry 0x80 (88 corpus items) lose it.

Group is the u16 at @14, same as notes. 23,394 of 23,520 corpus items have 0.

## Trim (@24 and @28)

Type depends on the source, decided by the u32 at @4:

- Pattern items (`u32@4 >= 0x50000000`): two i32 in ticks. `FUN_010649c0`
  reads `(double)*(int *)(item+0x18)` and `+0x1c`; a negative start means
  untrimmed and the end becomes the pattern's length. `FUN_01062400` writes
  -1 (0xffffffff) in both for a new pattern item. `FUN_010644a0` ("is
  trimmed") is `*(int *)(item+0x18) >= 0`.
- Clip items (`u32@4 < 0x10000000`): two f32. `FUN_01062400` writes -1.0
  (0xbf800000); `FUN_010644a0` is `*(float *)(item+0x18) >= 0.0`.
  `FUN_010649c0` converts them to ticks: for an automation clip (channel type
  5) it multiplies by `DAT_01064bc0 / *(double *)(clip->+0x10)`; for an audio
  clip it runs each through `FUN_00695930(sample, value)` and multiplies by
  `FUN_00ff9d70(channel) * stretch(@64)`. The stored float is in the clip's
  own domain (sample positions for audio, the automation clip's own unit),
  not ticks.

Corpus: pattern items are i32/i32 (2,589 trimmed, 6,024 with -1) plus 2,100
with -1.0f from older writers; clip items are f32/f32 (3,715 trimmed, 5,452
with -1.0f) plus 3,640 whose start is 0, which is 0 in both encodings. The
engine's "pattern trims are i32 ticks, clip trims are f32" split is what FL
does, and `end - start == length` only holds for patterns.

## Still unknown

- Item bytes 36..51, 56, 76 (init -1) and 84: initialised, never read in the
  functions decompiled.
- Item byte 19 bit 4 (0x10) and bit 6 (0x40): bit 6 is cleared at load; bit 4
  is set and tested inside `FUN_012fc180` but its meaning was not traced.
- Note tag word @4: what 0x4000 means, and whether any bit besides slide
  (bit 3) is ever read.
- Note byte 19 bit 6.
- Whether the id at @32 is honoured or regenerated by any later pass; the load
  path keeps it.

## Functions read

Loader: `FUN_00f46490` (224 at line 2586, 233 at 2735), `FUN_00f558e0`,
`FUN_00f553b0`, `FUN_00f55510`, `FUN_00f3c7b0`, `FUN_01060a30`,
`FUN_012d16e0`, `FUN_012f9f00`.

Record channel classes: `FUN_012df3c0` (slot 0x38 load), `FUN_012df570`
(record i), `FUN_01060220`, `FUN_01062350`, `FUN_01063a60`, `FUN_010619c0`
(constructors), `FUN_01062250`, `FUN_01062400`, `FUN_010657a0`,
`FUN_01064e20` (defaults), `FUN_01060930`, `FUN_012df5b0`, `FUN_010626c0`
(delete marker), `FUN_010620e0`, `FUN_01061ae0`, `FUN_01061bc0`,
`FUN_01062460`, `FUN_010624d0`, `FUN_0105f2e0`, `FUN_01062720`,
`FUN_012dfb80`, `FUN_012dfc00`, `FUN_012dfc40`, `FUN_012df8e0`,
`FUN_012df950`, `FUN_012dfd00`.

Trim and clip properties: `FUN_010649c0`, `FUN_010646d0`, `FUN_010644a0`,
`FUN_01064c10`, `FUN_01065520`, `FUN_010650f0`, `FUN_00e2b2f0`,
`FUN_00e2b630`, `FUN_00e2c2b0`, `FUN_00e2c3d0`, `FUN_00e9d480`.

Mute and selection: `FUN_0105f3c0`, `FUN_0105f390`, `FUN_0105f340`,
`FUN_012f3560`, `FUN_01306040`, `FUN_012fc180`, `FUN_012c9510`,
`FUN_012d3e60`, `FUN_012d4140`, `FUN_01186220`, `FUN_00e3cb60`,
`FUN_00e3cc70`.

Note fields: `FUN_00c3ed10` (flpianoroll.Note), `FUN_01308b40`,
`FUN_00d5b930` (property index to byte), `FUN_00d29aa0` (note to voice),
`FUN_0130ff30` (event editor labels), `FUN_01308810`.

Decompile output: `ghidra/out/rec224/`, `ghidra/out/recslots/`,
`ghidra/out/recfields/`, `ghidra/out/recreaders/` (four runs).

## Corrections from the corpus (crates/model, crates/engine step 4)

- Track word @12: `500 − word` holds from 12.9.1. 19 older projects (6,571 items) store `199 − word`; the gate is PyFLP's and the corpus agrees.
- Audio clip trims @24/@28 are milliseconds, not sample positions: `FUN_00695930` multiplies by 0.001, and 4,763 of 6,043 trimmed clips land on a tick under that reading.
- Bytes 36..52 and 56 are not always zero: 3,250 and 3,078 of 14,117 items carry data there. Meaning unknown.
- Byte 76 is 0 in all 10,576 items that have it, not −1.
- Item count in the corpus is 27,061, not 23,520.
- No corpus file writes 88-byte items, so that size and the pan field @80 are covered by unit tests only.

## Step sequencer notes (crates/model/src/pattern.rs, checked on "Stroll Up" pattern 10 and the decompiled step toggle)

- A step-sequencer note has length 0, not one step.
- Its key is 60 whatever the channel's root note (corpus: key 60 on roots 56, 59, 62, 64).
- The rack lights a step when any unmuted note starts inside the step's 24 ticks; 3% of step notes sit off the grid.
- Clearing a step deletes the record rather than muting it.

## Piano roll notes (crates/model/src/pattern.rs)

- 469 of 4,334 zero-length notes in the corpus are not key 60 (keys 52 to 84), so a step note's key is not fixed.
- No shipped 224 holds a deleted (tag 0xFFFFFFFF) record; FL compacts on save. `model::to_flp` does the same: a deleted note keeps its record while the project is open, so the other notes keep their places, and the record is left out of the 224 that is written. Deleted playlist items (source 0xFFFFFFFF) are written, as FL writes them.
- 1,573 notes carry the selected bit in saved files.
- Notes starting together have no fixed order. Fine pitch spans 0 to 240, mod X/Y 0 to 255.
