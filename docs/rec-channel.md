# REC channel controls: what a link to `channel << 16 | control` drives

Read from `FLEngine_x64.dll` (FL Studio 2026, Ghidra base 0x400000) and
checked against the 165 projects under `Data/`. This completes the channel
line of `docs/rec-225.md` for the controls automation clips and controller
links bind to. Confidence tags as in the other docs: `loader` read from the
decompiled C or the disassembly, `dfm` read from the channel settings form
in the DLL's resources, `corpus` the projects agree, `inferred` a reading no
second source confirms.

## Where it is read

The REC dispatcher `FUN_01046a70(id, value, flags)` takes an ID below
0x10000000 as `channel = id >> 16`, `control = id & 0xffff` (decompile lines
998 to 2030). For each control it finds the channel object `chan` and the
knob that shows the control in the channel settings form (`TPluginForm`,
whose fields sit at `form + 0x...`), then:

- with flag 0x20, a value from an automation clip or a controller in
  2^30ths, maps it onto the knob's range: `knob.min + round(v / 2^30 *
  (knob.max - knob.min))`, with `knob.min` at `knob+0x3b8` and `knob.max` at
  `knob+0x3bc`. A few controls use a fixed range through `FUN_01046a30(v,
  min, max)`, which is that same formula with constants (it is a scale, not
  the clamp `docs/rec-225.md` took it for).
- with flag 1, stores the raw value in `chan` and calls the function that
  works the channel's levels out again.

The knob ranges come from the form's DFM resource (`TPLUGINFORM`, RCDATA):
every knob there has a `Tag` that is its control number, a `Min`, a `Max`
and a `DefaultValue`. The DFM was dumped with a small TPF0 reader, not
with Ghidra.

The channel struct offsets below are those of `docs/plugins/host-voice.md`
and `docs/automation-234.md`: 219 `Levels` is copied to `chan+0x16c` and 229
`LevelAdjusts` to `chan+0x184` (loader `FUN_00f46490`, the 229 branch, which
multiplies @0 and @4 by 100 for writers below 5.3.7).

## The map

| control | what | storage | range (DFM knob or fixed) | update | engine | confidence |
|---|---|---|---|---|---|---|
| 0 | volume | 219 @4, `chan+0x170` | 0..12800, default 10000 (`VolWheel`); fixed in the dispatcher | `FUN_00fe9d20` | plays | loader, dfm |
| 1 | pan | 219 @0, `chan+0x16c` | 0..12800, 6400 centre (`PanWheel`) | `FUN_00fe9bf0` | plays | loader, dfm |
| 2 | mod X (filter cutoff) | 219 @12, `chan+0x178` | 0..256, default 256 (`FCutWheel`) | `FUN_00fea140` | plays on a sampler or audio clip channel | loader, dfm |
| 3 | mod Y (filter resonance) | 219 @16, `chan+0x17c` | 0..256, default 0 (`FResWheel`) | `FUN_00fea140` | plays on a sampler or audio clip channel | loader, dfm |
| 4 | pitch knob, cents | 219 @8, `chan+0x174` | -4800..4800 fixed (`FUN_01046a30(v, -4800, 4800)`); the knob itself shows the pitch range, `PitchRangeSelect` 1..48 semitones, default 2 | VMT +0x128 = `FUN_00ff86f0` | plays | loader, dfm, corpus |
| 5 | filter type, 0..7 (below) | 219 @20, `chan+0x180` | `FTypeSelect` at `form+0xa88`, tag 5, whose `FTypeSelectChange` (`FUN_00f62640`) sends control 5 | `FUN_007a0940` | read for the filter, not automated | loader, dfm |
| 6 | portamento time | `chan+0x1f0` | 0..1660 (`PolySlideWheel`) | `FUN_00fea220` | no | loader, dfm |
| 7 | mute | byte `chan+0x32e` | on above 2^29 | `FUN_00ffa730` | no | loader |
| 8 | target mixer track | `chan+0x258` | 0..insert count - 2 | | no | loader, dfm |
| 9 | gate | `chan+0x2c4` | 450..1447 (`GateWheel`) | | no | loader, dfm |
| 10 | layer crossfade | `chan+0x2d0` | 0..256 (`Layer_FadeWheel`) | | no | loader, dfm |
| 11 | fine time shift | `chan+0x21c` | 0..1024 (`ShiftWheel`) | | no | loader, dfm |
| 12 | swing mix | `chan+0x220` | 0..128 (`SwingMixWheel`) | | no | loader, dfm |
| 13 | sample start offset | f64 `chan+0x314` | 0..16384 | | no | loader, dfm |
| 14 | time stretch (time) | f32 `chan+0x2e4` | 0..36864 (`StretchTimeWheel`), 48ths of a step | VMT +0x130 = `FUN_00fea570` | the stored time plays (below, "Time stretch"); a clip on it does not | loader, dfm |
| 15 | formant shift | f64 `chan+0x324` | 0..2400, 1200 centre (`FormantShiftWheel`) | | no | loader, dfm |
| 0x10 | pan offset | 229 @0, `chan+0x184` | -6400..6400 (`PanOfsWheel`) | `FUN_00fe9bf0` | plays | loader, dfm, corpus |
| 0x11 | volume multiplier | 229 @4, `chan+0x188` | 0..25600, 12800 unity (`VolOfsWheel`) | `FUN_00fe9d20` | plays | loader, dfm, corpus |
| 0x12 | 229 @8 | `chan+0x18c` | no knob in the form | `FUN_00fea140` (reads nothing of it) | no | loader |
| 0x13 | mod X offset | 229 @12, `chan+0x190` | -256..256 (`FCutOfsWheel`) | `FUN_00fea140` | plays on a sampler or audio clip channel | loader, dfm |
| 0x14 | mod Y offset | 229 @16, `chan+0x194` | -256..256 (`FResOfsWheel`) | `FUN_00fea140` | plays on a sampler or audio clip channel | loader, dfm |
| 0x103..0x125 | TS404 parameters | `*(chan+0x358) + 8 + 4n` | per knob | | no | loader, dfm |
| 0x200..0x207 | echo delay: 0x200 feedback, 0x201 pan, 0x202 pitch, 0x203 echoes, 0x204 time, 0x205 mod Y, 0x206 mod X | `chan+0x200 + 4n` | per knob (`Delay*Wheel`, tags 512..518) | | no | loader, dfm |
| 0x300..0x3FF | arpeggiator: 0x300 direction, 0x301 range, 0x302 chord, 0x303 time (450..1447), 0x304 gate (0..48), 0x305 repeat (`docs/arpeggiator.md`) | `chan+0x2ac + 4n` (0x302 stored less 2, 0x305 at `chan+0x2e0`) | per knob (`Arp*`, tags 769..773) | 0x303: `FUN_00fea220` | 0x303 and 0x304 play from a clip | loader, dfm |
| 0x500..0x5FF | key and velocity tracking: record `(c - 0x500) >> 4`, field `c & 0xf` | `chan+0x1b0 + 0x20 r + 4 f` | -256..256 (`Track*Wheel`, tags 1280..1282) | | no | loader, dfm |
| 0x600..0x6FF | an automation clip's own LFO: 0x600 speed (512..65536), 0x601 tension, 0x602 skew, 0x603 pulse width, 0x604 level (-128..128) | the clip's `TACArticulator` | per knob (`ACLFO*`, tags 1536..1540) | | no | loader, dfm |
| 0x1000..0x17FF | the instrument envelopes and LFOs: envelope `(c - 0x1000) >> 8`, field `c & 0xff` (2 predelay, 3 attack, 4 hold, 5 decay, 6 sustain, 7 release, 8 amount, 9 LFO predelay, ...) | `*(chan+0x330) + 0x198 + 0xa0 e + 4 f` | per knob (`Env*`, `LFO*`, tags 4098..4112) | | no | loader, dfm |
| 0x1800..0x7FFF | nothing: the dispatcher has no branch for them | | | | no | loader |
| 0x8000.. | plugin parameter `c - 0x8000` | the plugin | the plugin's | plugin VMT +0x30 | plays where the generator is hosted | loader |

The pitch, pan and volume levels a voice plays by (`docs/plugins/host-voice.md`):

    chan.Vol   (chan+0x19c) = curve(219 @4 / 12800) * curve(229 @4 / 12800)       FUN_00fe9d20, curve = (exp(x ln 11) - 1) / 10, 1 at 1
    chan.Pan   (chan+0x198) = (219 @0 + 229 @0 - 6400) / 6400                       FUN_00fe9bf0 (1/6400 is *0x13e757c)
    chan.FCut  (chan+0x1a4) = (219 @12 + 229 @12) / 256                             FUN_00fea140 (1/256 is *0x13e7574), chan+0x164 < 2
    chan.FRes  (chan+0x1a8) = (219 @16 + 229 @16) / 256                             same, and the offsets alone for chan+0x164 2 and 3

and the host puts them into every voice's `FinalLevels` once a tick
(`FUN_00d265e0`), so a sounding voice follows them.

## Old projects: the link IDs are rewritten

FL's loader runs version fixups on every link's target (and on its source
behind a flag, bit 3 of `0x193fc70`) through `FUN_012cb6c0`, and not on the
225 records (the version gates at the end of `FUN_00f46490`):

| writer below | function | rewrite |
|---|---|---|
| 10.1.0 | `FUN_00f45960` | tempo values x 1000 (values only, so not a link's) |
| 11.5.6 | `FUN_00f45980` | a channel control 0x2000..0x3FFF + 0x6000, a legacy mixer control below 0x1F00 + 0x8000: plugin parameters were at 0x2000 on a channel and at 0 in a slot |
| 12.9.0 | `FUN_00f459e0` | a legacy mixer ID on insert 104 to insert 501, new base |
| 24.2.99 | `FUN_00f45460` | the base and send remap of `docs/rec-225.md` |

`model::rec::remap_old_link` and `Project::link_target` do the first two that
touch IDs. The corpus has two projects from before 11.5.6 with channel
controls in 0x2000..0x3FFF: `nucleon - Housework.flp` (11.0.3, Harmless
parameter 0x36) and `Synth_dfr - Overcome - Sytrus.flp` (11.1.0, Sytrus
parameters 0, 0x12, 0x13 and 0x226).

## What the engine plays (crates/engine/src/automation.rs, voice.rs)

- Pitch (4). A sampler or audio clip voice changes speed where the controls
  are read (every 512th sample and where a voice starts), and goes on from
  the frame it is at. Its end is where it runs off its sample, and it is
  made to last as long as the lowest pitch the clip can reach would take.
  A hosted generator's note takes the channel's pitch as it starts. FL
  refreshes `FinalLevels.Pitch` of its sounding voices every tick, and the
  engine reads the clip on a plugin channel's pitch every tick and hands
  each sounding note the new pitch (`plugins::Generator::set_note_pitch`).
  BooBass, Fruity Granulizer, Harmless, Harmor, Sakura, Sytrus, 3x Osc and
  Fruity Envelope Controller take it, and Patcher hands it to the
  generators inside it. The other generators keep the pitch their notes
  started with. A channel with a time stretch queues its sample to be
  stretched again when the pitch moves (`FUN_00ff86f0`, `chan+0x2f0` not
  -1 or -2, and no time); the engine stretches a voice for the pitch it
  starts at (below, "Time stretch"), and a pitch clip then moves its
  speed, as on any other voice.
- Pan offset (0x10) and volume multiplier (0x11), with the channel's volume
  and pan, as the formulas above. The pan offset now goes on every channel
  from its 229, automated or not; before, the engine read only the volume
  multiplier of a 229. Among the channels a clip drives, `Square a Saw -
  Oxygen` channel 49 has one of 6400 and `Olbaid - Sytrus Essentials`
  channel 53 one of -3768.
- Plugin parameters of old projects, through the rewrite above.
- The arpeggiator's time (0x303) and gate (0x304) from a clip. The engine
  plays a channel's arpeggiator (`docs/arpeggiator.md`), and reads the
  clip at the tick of every step, where FL works the step out again.
  Corpus: `nucleon - Harmless Galactic` channel 14 and `Mehran Abbasi -
  With You` channels 12, 16, 17 and 20.

Left out, and why:

- Mod X and Y (2, 3) and their offsets (0x13, 0x14) of a plugin channel.
  `chan+0x164`, which the kind setter `FUN_00ff7850` sets to 0 for a
  sampler (kind 0) and an audio clip channel (kind 4), 3 for a layer and 4
  for an automation clip, decides what `FUN_00fea140` makes of them: below
  2 the channel's levels are `(219 + 229) / 256`, at 2 and 3 the 229
  offsets alone, so for a plugin that takes mod X and Y as its own filter's
  (3x Osc answers `FPD_UseVoiceLevels` with 2) a clip on 219 @12 reaches
  no level. Where a plugin channel's `+0x164` is set was not found, so the
  engine plays none of the four on a plugin channel: `Square a Saw -
  Oxygen` channel 51 (3x Osc) keeps its two clips on mod X unsupported.
- A clip or a link on the stretch time (14), Stretch Pro's formant shift
  (15), the instrument envelopes (0x1000..), the tracking of mod X and Y
  on a sampler whose filter does not run, the echo delay (0x200..), TS404
  (0x103..): the engine plays none of these parts of a channel, and counts
  the channels that turn each on (below, "Channel parts the engine
  counts"). The stretch itself is played, as FL's lengths, speeds and
  pitches with a stand-in for élastique (below, "Time stretch"). Nor the arpeggiator's direction, range, chord and repeat
  (0x300..0x302, 0x305) from a clip, which no corpus clip drives, nor its
  time and gate from a controller link: the arpeggios are worked out from
  the song before it plays.
- An automation clip's LFO (0x600, 0x604) when that clip's LFO is on. The
  engine plays a clip's envelope alone. A clip whose LFO is off, which
  every clip these drive in the corpus is, is played: see below.

## Channel parts the engine counts (crates/engine/src/parts.rs)

A render counts, in `Rendered::unplayed_parts`, the channels that turn on
a part the engine leaves out, as it counts the arpeggiators it does not
build (`docs/arpeggiator.md`). The corpus test prints the counts on each
project's line (`unplayed envelopes`, `unplayed time stretch`, `unplayed
tracking`, `unplayed echo delays`, `TS404`) and adds them up on its
`corpus channel parts` line, and `tools/progress.py --render` counts a
project with any of them as not played in full. A channel counts when it
has a note that sounds in the render (a sampler voice with its sample, or
a note for a hosted generator), and for the envelopes and the time
stretch also an audio clip that sounds; a TS404 counts with a note, as
nothing of it sounds. A channel counts once for each part it turns on.

What turns each part on is a reading of what moves the sound, in
`model::Channel`:

| part | events and layout | on when | confidence |
|---|---|---|---|
| instrument envelopes and LFOs (`envelope_on`) | five 218 `EnvelopeLFO`, 68 bytes each, one per target in order pan, volume, mod X, mod Y, pitch. i32s: @0 flags (a byte), @4 the envelope's switch, @8 predelay, @12 attack, @16 hold, @20 decay, @24 sustain, @28 release, @32 amount, @36 LFO predelay, @40 LFO attack, @44 LFO amount, @48 LFO speed, @52 LFO shape, @56..@67 tensions. The loader copies each to `smp+0x198 + 0xa0 e`, and control `0x1000 + 0x100 e + f` is the i32 at `4 f` | an envelope's switch is on and its amount is off 0, or its switch is on and it is the volume's; or an LFO's amount is off 0 | fields 2 to 9 loader and dfm (the map above); the switch @4, the LFO fields and the order of the targets PyFLP and inferred. A new sampler (`model`'s `NEW_SAMPLER`, from FL's `Empty.flp`) has every switch off and every amount 0, and its second record alone has other flags and a tension, which fits the volume being second. That the volume envelope works at amount 0 is inferred |
| Stretch Pro's formant shift (`crate::stretch::unplayed`), sampler and audio clip channels only | 215 `Parameters` @160 f64 the formant (control 15, `chan+0x324`), with @108 the mode (`chan+0x2f0`) | the mode is Stretch Pro (-2) and the formant is in the payload and off 0.5. The rest of the stretch is played (below, "Time stretch"); before, any time, pitch, multiplier or formant counted (`model::Channel::time_stretch_on`), 52 channels in the render at `5d3f1b8` | `FUN_0067db90` takes the formant as a factor of `2^(1 - 2 formant)` on the pitch; that only élastique Pro shifts formants is the DLL's string "Elastique Pro with formant shifting" for the mode. 0.5 for the centre inferred from a new sampler |
| tracking of mod X and Y (`tracks_mods`), sampler channels whose filter does not run | two 228 `Tracking`, 16 bytes each, the velocity's then the key's: i32 @0 the middle, @4 pan, @8 mod X, @12 mod Y. The loader copies record `r` to `chan+0x1ac + 0x20 r`, so control `0x500 + 0x10 r + f` is the knob at @`4 + 4 f` | a mod X or mod Y knob of either record is off 0, on a sampler whose voices do not run the filter (no clip or link drives its mod X, mod Y or their offsets). Pan tracking is played on every channel, and a plugin is handed its tracked mods (`crates/engine/src/levels.rs`) | loader and dfm for the knobs; the middle and the velocity-then-key order as `levels.rs` reads them. A new sampler has middles 100 and 60 and every knob 0 |
| echo delay (`echo_delay_on`) | 209 `Delay`, 20 bytes, five i32: @0 feedback, @4 pan, @8 pitch, @12 echoes, @16 time, copied to `chan+0x200`, controls 0x200..0x204 in that order | feedback above 0 and at least one echo | loader and dfm for the order; the legacy 130 writes echoes to `chan+0x20c` (@12) and a feedback that grows with them to `chan+0x200` (@0). That feedback 0 makes no echo is inferred: a new sampler has feedback 0, 4 echoes and time 144 |
| TS404 (`is_ts404`) | 21 `ChannelID.Type` 1, whose controls 0x103..0x125 address the object at `chan+0x358` (210 `LegacySampleState` fills it) | the channel is of type 1 | inferred: FL 3's TS404 was a channel type, and no corpus project has type 1 or a 210. No filter type is a TS404: 219 @20 has the eight kinds of the table at 0x1510e40 |

## Time stretch (crates/engine/src/stretch.rs)

A sampler or audio clip channel's 215 holds a stretch: @96 the time
(`chan+0x2e4`), @100 the pitch in cents (`chan+0x2e8`), @104 the
multiplier (`chan+0x2ec`), @108 the mode (`chan+0x2f0`) and @160 the
formant (`chan+0x324`). FL keeps `chan+0x6f4 = 2^(mult / 10000)`
(`FUN_00fea570`, the time setter at VMT +0x130, through `FUN_01186110`),
so 10000 is twice as long. The time is in 48ths of a step: `FUN_00d26890`
and `FUN_00ff0640` make ticks of it as `time * (PPQ >> 2) / 48` (`0x15d89c0`
is the PPQ and `0x15d89c4` a quarter of it, both set by `FUN_013111e0`), and
output samples with the samples a tick lasts (`0x15d8980`, 196.875 at
44.1 kHz, 140 BPM and PPQ 96). Corpus: `Broken Crash.wav` is 3.0142 s long
and its channel in `Ookay - Thief` (150 BPM) has a time of 1447, 3.0146 s.
Writers before 169 bit 0 store an integer; the engine, which does not keep
169, reads one below 2^23 as an integer and any other as an f32.

The modes, in the order of their names in the DLL ("stretch pro",
"stretch", "resample", "e3 generic", "e3 mono", "slice stretch", "slice
map", "auto", "e2 generic", "e2 transient", "e2 mono", "e2 speech"): -2 to
9. The loader remaps them for old writers (`FUN_00f46490`): below 6.1.5 a
mode above 2 moves up 4 and 1 and 2 move up 2, and below 11.5.13 the mode,
held to 0..9, goes through the table `0, 6, 6, 7, 7, 8, 9, 3, 4, 5` at
`0x14e01fb`, and a 5 out of it becomes 6.

How a voice plays, with `R` the sample's rate over the output's, `M` the
multiplier, `D` the time in output samples times `M`, `N` the voice's
pitch from the sample's own speed in cents and `P` the stretch pitch:

| mode | with a time | without |
|---|---|---|
| Stretch, Stretch Pro (below 0) | élastique as the voice plays (`FUN_00d26890` into `FUN_0067db90`): the sample is read at `len / D` and stretched to `D`, and shifted `N + P` cents without a change of length | the same at `R / M`: `M` times as long, shifted `N + P` |
| Resample (0) | read at `len / D` (`FUN_0067d9b0`), whatever `N` | read at `R / M * 2^(N / 1200)` (`FUN_0067db00`) |
| e3, e2, slice and auto (above 0) | stretched once to `round(D R)` frames and shifted `P` (`FUN_00ff0640` into `FUN_00a97410`), then read at that over `D`, whatever `N` | stretched to `M` times its length and shifted `P`, read at `R * 2^(N / 1200)` |

In Resample, `FUN_00ff0640` also shifts the sample by `P` once, keeping
its length (`FUN_00a97410` with mode 5). `FUN_00fea6e0` is what decides
on a load-time stretch: a mode above 0 with a time, a pitch or a
multiplier. A mode above 0 with none of them plays as a channel with no
stretch, and so does Resample with none; Stretch and Stretch Pro always
run élastique, so their notes keep the sample's length on every key. In
auto with `chan+0x370` at 2 FL slices the sample instead (`FUN_00f36060`).

What the engine builds of it: every length, speed and pitch above. The
stretch is not FL's: élastique (`Shared/elastique_x64.dll`, zplane's) and
FL's slicer are stood in for by a WSOLA, 40 ms Hann frames at half overlap,
each taken from within 5 ms of where the stretch puts it at the offset that
best continues the frame before, and a pitch shift `p` is a stretch by `p`
more read `p` times as fast. A stretch under 0.2% with no pitch shift is
played by speed alone, 3.5 cents off at most: that is most audio clips,
stretched to their own length at the project's tempo. The stretch is made
when the graph is built, once for each sample, length and pitch, so a
voice plays a stretched sample and the audio thread does nothing new; a
pitch clip or a slide on a stretched voice changes its speed, where FL's
élastique would shift its pitch. A clip's start offset is moved to the
same place in the stretched sample. Left out: Stretch Pro's formant
(counted), a clip or link on the time, the auto mode's slicing (stretched
as the others), and an item's own stretch (`Rendered::unsupported_stretch`,
which reads `voice+0x39c` into `M` in `FUN_00d26890`).

## Corpus

Channel-control bindings below 0x8000 in the 165 projects (each once,
`project.automation_bindings()`): 4 pitch 31, 0x11 volume multiplier 56,
2 mod X 7, 0x10 pan offset 6, 0x303 arp time 5, 0x600 and 0x604 clip LFO 6
each, 0x13 2, 15 1, 0x304 1, 0x1408 1, and 12 in 0x2000..0x3FFF that the
rewrite turns into plugin parameters. The 219 range of every pitch clip
fits the -4800..4800 scale: 6133..6667 is the knob's default 2 semitones
and 4800..8000 an octave either way. Every volume multiplier clip has
0..6400, which is 0 to unity.

## Functions read

| address | what |
|---|---|
| `FUN_01046a70` | REC dispatcher, channel branch (decompile lines 998 to 2030) |
| `FUN_01046a30` | `min + round(v * 2^-30 * (max - min))` (disassembly) |
| `FUN_00fe9d20`, `FUN_00fe9bf0`, `FUN_00fea140` | channel volume, pan, mod X/Y |
| `FUN_00ff86f0` | channel VMT +0x128, after a pitch change: queues a stretch again |
| `FUN_00fea570` | channel VMT +0x130, sets the stretch time and `chan+0x6f4` |
| `FUN_00fea6e0`, `FUN_00fea720` | whether a load-time stretch is on; whether one plays as the voice runs |
| `FUN_00ff0640` | the load-time stretch: its length and pitch, into `FUN_00a97410` |
| `FUN_00d26890` | a sampler voice's speed and élastique settings |
| `FUN_0067d9b0`, `FUN_0067db00`, `FUN_0067db90` | the voice's resampler speed; speed with a pitch; élastique's time, pitch and formant |
| `FUN_01186110` | `2^(mult / 10000)` |
| `FUN_00f46490` | loader: 226/227 branch, 229 branch, version gates |
| `FUN_012cb6c0` | runs an ID rewrite over the links and the other ID lists |
| `FUN_00f45960`, `FUN_00f45980`, `FUN_00f459e0`, `FUN_00f45a40` | link and ID rewrites for writers below 10.1.0, 11.5.6, 12.9.0 and 12.9.1 |
| `TPLUGINFORM` (RCDATA) | knob tags, ranges and defaults |

## An automation clip's LFO controls (0x600..0x604)

The dispatcher's branch for 0x600..0x6FF (`FUN_01046a70`, decompile lines
1876 to 1943) runs only for a channel with an articulator (`chan+0x360`,
an automation clip). It writes 0x600..0x603 to the clip's `TMLFO` at
`+0xd0 + 4 n` (speed, tension, skew, pulse width) and calls its VMT
`+0x40` (`FUN_00ba0250`, which rebuilds the oscillator through
`FUN_00677660`), and 0x604 to the articulator's `+0x30` and calls
`FUN_00fe9b90`, which makes the LFO's amount `+0x28` of it:

    art+0x20 = 219@0 / 12800;  art+0x24 = 219@4 / 12800 - art+0x20
    art+0x28 = art+0x30 * (1/128) * art+0x24          (*0x15e3890 = 1/128)

Nothing in the branch touches the LFO's switch, the i32 at `+0x48` (offset
4 of the LFO block's tail). The articulator (`FUN_00ba1700`) adds the LFO
only when that is not 0, so on a clip whose LFO is off these controls
change nothing FL plays. The engine plays a binding or a controller link
to one of them by setting nothing when the clip's LFO is off, and counts
it unsupported when it is on. Corpus: `SFX transitions.flp` drives 0x600
and 0x604 of six clips, all with the LFO off, which are 12 bindings that
now play.

## The filter of a sampler or audio clip channel (crates/engine/src/channel_filter.rs)

FL's host plays the voices of a sampler and an audio clip channel itself
and runs them through a filter, the same unit 3x Osc carries a copy of
(`docs/plugins/3x-osc.md`, section 3.3). Read from `FLEngine_x64.dll`:

- Voice set-up (`0xd2795c`): the filter type is `chan+0x180`, 219 @20, and
  the table at 0x1510e40 gives a kind and whether it runs twice: 0 Fast LP,
  1 to 4 the low-pass, band-pass, high-pass and notch biquads, 5 the
  low-pass twice, 6 the state-variable low-pass, 7 it twice. `rec+0x234`,
  the filter's switch, is on for every kind but the Fast LP; `FUN_006a05a0`
  clears the state.
- Coefficients (`FUN_00d24650`, run where a tick starts and as a voice
  starts): the cutoff is `FinalLevels.FCut + rec+0x110` and the resonance
  `FinalLevels.FRes + rec+0x134`, the two `+0x110` and `+0x134` being what
  the mod X and mod Y envelopes and LFOs add. `FUN_00981df0` (Fast LP),
  `FUN_00981c50` (biquads, through the RBJ design `FUN_009815f0`) and
  `FUN_00981f20` (state-variable) make them, with `ln 601`, `ln 51`, `ln 51
  * 80 / 128` and `ln 26` worked out in f64 and kept as f32 (`0x6a690b`,
  `0x9823e8`). A Fast LP switches itself on for good once its coefficients
  leave 1 and 0, and its speed moves to the new one over the tick, by no
  more than `limit` a sample (`FUN_006a3810`), except on the voice's first
  block, where it starts there.
- The sample-rate code at 0x6a0230 sets `s = 44100 / rate` (`0x13ec894`)
  and `limit = s * 0.001` (`0x13ec898`).
- Running (`FUN_00d247a0`, every block, before the gains): the Fast LP
  kernel at 0x6a5eb0 keeps its level and speed in f64 and steps its own
  speed in f32; the biquad (0x982020) and state-variable (0x982140) kernels
  are f32. The twice low-pass runs the biquad again on its own output and
  multiplies the voice's gains by `1 - q (3.2 - w) * 0.046875` (`0xd24886`);
  the twice state-variable filter runs the second stage with the damping
  `q + (1 - q) * 0.25` (`0xd2490b`). The kernels zero what is left in their
  state under 2^-24 after each block.
- `FUN_00d24ae0`, the voice's render, calls the coefficients at a tick and
  the filter on every block, between the generator's render and the
  gains.

What the engine plays: a voice on a sampler or audio clip channel whose
mod X, mod Y or one of their offsets a clip or a link drives runs this
filter with the cutoff and resonance of its note (its mod X and Y and
tracking) and of its channel. It works the coefficients out where it reads
its controls, every 512th sample and where a voice starts, and moves the
Fast LP over 512 samples rather than a tick. It leaves out the mod X and
mod Y envelopes and LFOs, and the zeroing under 2^-24, which would make a
voice depend on where its blocks fall. A channel whose filter nothing
drives plays unfiltered, as before: 85 of the corpus's 1,277 sampler and
audio clip channels have a filter that is not wide open, and playing it on
all of them would change projects no clip touches.

Corpus: `Square a Saw - Oxygen` channel 14, a sampler with type 5 (the
low-pass twice) and mod Y at 114, and `Bombs Away ft KARRA - I'm Awake`
channels 227 and 228, audio clip channels with a clip on the mod X offset.
