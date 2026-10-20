# Fruity Slicer 2: state, slices, voice

Recovered from `Fruity Slicer 2/engine_x64.dll` (FL Studio 2026, MSVC C++,
image base 0x180000000, `CreatePlugInstance` at `0x180147980`). The
plugin's `Fruity Slicer 2_x64.dll` is a Delphi shell: the panel, the
sample editor, the auto-slicer, Elastique's settings; it hands the engine
the decoded sample and its parameters. Corpus: no project uses Slicer 2;
one preset (`Plugin presets/Generators/Fruity Slicer 2/Default.fst`, also
in the plugin databases), version 3, with no sample.

**Slicer 2 shares nothing with Fruity Slicer** (`fruity-slicer.md`): that
one is a Delphi plugin of FL's own sample voice; this one is a d1framework
engine of the Kepler kind (`FLWrapperPlug`, RTTI namespace `slc`), with
zplane Elastique linked in, its own 16 voices, its own read of the sample,
envelopes and filters. So it is its own module, `fruity_slicer_2.rs`.

Confidence tags: `dec` (decompile), `asm` (listing), `corpus`, `inferred`.

## How the plugin is wired

- `MyPlugin` vftable `0x18024f5c8` (RTTI `.?AVMyPlugin@@`): slot 1
  `FUN_180149a30` note expressions (1 cutoff, 2 resonance, 3 pitch
  `x * 0.01` semitones, 4 pan, onto every sounding voice), 2
  `FUN_180149a10` top `+0x10` (a pitch in semitones the host sets), 3
  `FUN_180149e80` track pan, 4 `FUN_180149eb0` track volume (FL's
  `FPE_MIDI_Pan`/`FPE_MIDI_Vol`), 6 note on `FUN_180149d50` (velocity
  `/ 127`), 7 note off `FUN_180149df0`, 15 save `FUN_1801498c0`, 16 load
  `FUN_180149960`, 32 process `FUN_18014a6f0`. The vftable at
  `0x18024f470` is the same object as `ISlicerSampleManager`: slot 1
  `FUN_18014a230` loads a path through FL's sample loader into channel
  buffers with 256 zeros either side, a tempo (120 when the file has
  none) and the file's slice marks. `dec`
- Parameters for the shell: `DispatchToEngine` (`0x180147bc0`, the
  export) and get `FUN_18014ff90`. Per slice `0x10 + 0x80 k + i`:
  k = 0 pitch (`(p + 24) / 48`), 1 fine, 2 volume, 3 pan, 4 attack, 5
  release, 6 cutoff, 7 resonance; 0 cut, 1 cutoff, 2 resonance, 3 pitch,
  4 fine for the track. `dec`
- FL's note becomes a MIDI note in the wrapper, as in Kepler
  (`kepler.md`): key `int(Pitch) / 100`, velocity
  `int(clamp(ln(InitVol * 10 + 1) * 41.71426, 0, 127))`. `inferred` from
  Kepler's wrapper, the same framework.

## 1. State (event 213)

```
u32 1                                     FL's wrapper: version
chunks: i32 id | i64 size | bytes         id 1 the plugin's, id 2 the wrapper's (56 bytes)
chunk 1:
  u32 0 | u32 len - 12 | u32 0            shell (Kepler's)
  u32 1 | u32 8 | u32 0                   engine header, read and ignored (FUN_180149960)
  "slc1" | i32 version (1..4) | i32 tracks          FUN_180150340 / FUN_1801505c0
  i32 embed (top +0xc) | i32 +0x14 | i32 +0x18 | f32 +0x1c | u8 +0x20 | f32 +0x24
                                          the auto-slicer's (FUN_18014f6c0/f8a0), not the sound's
  i32 root key (top +0x28)
  per track (0x3810 bytes, FUN_18014d630; the engine keeps the first):
    i32 output +4 | f32 volume +0x3684 | f32 pan +0x3688 | i32 pitch +8
    f32 fine +0xc | i32 mode +0x10 (0 resample, 1 resample to tempo, 2 Elastique)
    u8 cut +0x14 | i32 filter +0x1c | f32 cutoff +0x20 | f32 resonance +0x24 | i32 +0x18
    sample block (FUN_18014bbf0):
      i32 selected slice +0x5c | string path | i32 slices (<= 128) | f32 tempo +0x50
      i32 chromatic slice +0x58 (-1 none) | f32 +0x54 (handed to the loader)
      i32 embedded: 1 -> i32 frames | i32 channels | i32 rate | channels x frames f32
      per slice (0x68 bytes at track + 0x68 (i + 1), FUN_18014b5a0, defaults FUN_18014b510):
        i32 output +0x24 (-1) | i32 key +0x28 (-1: root + i) | i32 start +0x2c | i32 end +0x30
        f32 volume +0x64 (0.7694) | f32 pan +0x60 (0.5) | i32 pitch +0x34 | f32 fine +0x38 (0.5)
        u8 mode +0x20 | f32 fade +0x3c (s) | i32 filter +0x40
        f32 cutoff +0x44 (1) | resonance +0x48 (0) | attack +0x4c (0) | hold +0x50 (0)
        f32 decay +0x54 (0.5) | sustain +0x58 (1) | release +0x5c (0.1)
        version > 2: string name
    version > 3: i32 +0x380c
```

A string is an i32 length and that many bytes, the last a NUL
(`FUN_180148720`). `corpus`: the shipped preset reads to its last byte.

## 2. A note (`FUN_18014db70`)

With a sample loaded: when the chromatic slice is -1, every slice whose
key (or root + its place) is the note's starts a voice; with cut on, every
sounding voice goes to its 1 ms release first (and again before each such
slice). Otherwise the chromatic slice starts with an offset of
`note - its key` semitones. Voices (`FUN_18014ddd0`): 16, a free one or
the one that started first.

Voice start (`FUN_18014c2d0`): velocity gain
`0.98435 v^3 - 0.231573 v^2 + 0.247223 v`. Mode (`+0x20`): bit 0 a note
off is ignored, 0/1 start..end, 2/3 start..sample end, 4/5 end..start,
6/7 end..0; both clamped into the sample, the direction from their order.

## 3. A voice's frame (`FUN_18014d180`)

Every `int(rate / 4410)` frames, from the first:

- Step: `2^(semis / 12) * file rate / host rate`, semis
  `pitch + track pitch + 2 f + int(offset) + top+0x10 + note pitch`,
  `f = ((track fine - 0.5) + fine) - 0.5`; mode 1 times
  `tempo / track tempo`. Mode 2 hands the time ratio to Elastique
  (`FUN_18014ce30`, blocks of 512) and the pitch to its shifter.
- Filter (`FUN_18014c5d0`, only with a type): `f = 14.35 * 2^(10.583333 c)`
  Hz, `g = tan(f pi / rate)`, `k = 1 - r * 0.7225 * 0.99`,
  `a1 = 1 / ((g + k) g + 1)`, `a2 = a1 g`, `a3 = a2 g`; mix lowpass
  (0, 0, 1), highpass (1, -k, -1), bandpass (0, 1, 0). Then the smoothed
  cutoff and resonance move toward the slice's by `1 - 0.1^(1/44.1)`.
- Envelope times (`FUN_18014c990`) at a fixed 44100 Hz whatever the host
  rate (`+0x130`, set once in `FUN_18014bed0`): attack `2^(15 p) * 0.001`
  s (0: none), hold `p * 10` s, decay and release `2^(15 p) * 0.001f` s,
  sustain as is.
- Gains (`FUN_18014c890`): pan law `L = (1.828 - 0.828 q) q * 1.4144272`,
  `q = 1 - p`, `R` the same of `p`, times the volume curve
  `velocity_curve(v) * 10^0.3` (`FUN_18014b450`).

Then the read (`FUN_18014cb90`): at the end (forwards `end <= pos`,
backwards `pos < end`) the 1 ms release starts and the frame is 0; else a
four-point Hermite at `int(pos)` (`c1 = (x1 - x-1) / 2`, `w = x0 - x1 +
c1`, `a = (x2 - x0) / 2 + w + x0 - x1`,
`((a t - (a + w)) t + c1) t + x0`), the step on, and forwards with a fade,
`1 - min((pos - (end - fade * file rate)) / (fade * file rate), 1)`.
The slice's filter (`a2 v3 + a1 ic1`, `ic1 a2 + ic2 + a3 v3`); the
envelope; out `velocity * envelope * x * gain`, the gains smoothed each
frame by `1 - 0.1^(1 / (0.01 rate))`. A slice with an output of its own
skips the track.

Envelope (`FUN_180150bf0`, stages `FUN_180150ee0`, `0x180150f60`,
`FUN_180150fa0`, `0x180151020`, `FUN_180151040`, `FUN_1801510b0`,
`asm` for the labels): attack `phase += inc` to 1 on `curve(phase)`;
hold; decay `phase -= inc` on `curve * (1 - s) + s`; sustain; release from
the level at the note off, `curve(phase) * from`, done below 1e-6; the
1 ms release the same at `1 / (44100 * 0.001)`. The curve
(`FUN_180151e10`/`FUN_180151ef0`) is set once to a tension of -1:
`k = (6347 - 1) * -0.1`, `(exp(ln(|k| + 1) x) - 1) / |k|`.

## 4. The track (`FUN_18014e150`)

After the voices (last to first): every `int(rate / 4410)` frames the
track's filter is designed from its smoothed cutoff and resonance (the
same filter, starting at 0) and they move toward the track's; the filter
runs with a type; then the track's gains (the same pan law and volume
curve, `FUN_18014f5f0`), smoothed from 1 each frame, and out to the
track's output.

## What the Rust port does

`crates/plugins/src/fruity_slicer_2.rs`: sections 1 to 4, with the
sample from the host by its path or out of the state. Left out:

- Elastique (mode 2): the slices are resampled to the tempo as mode 1
  does, so their pitch moves with it.
- The engine's 128 outputs all go into the one.
- `FPE_MIDI_Vol`/`FPE_MIDI_Pan`: the track keeps the volume and pan the
  state saved, and the host puts the channel's on after.
- The note expressions: a note's fine pitch goes on its own voice (the
  engine puts the last one on every voice), its pan is not taken.
- The parameters for automation (`DispatchToEngine`), the learn-a-key
  mode, top `+0x10`.
