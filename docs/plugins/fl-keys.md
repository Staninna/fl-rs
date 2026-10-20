# FL Keys: state, wavetables, voice model

Recovered from `FL Keys_x64.dll` (FL Studio 2026, Delphi, image base
0x400000, no engine DLL). Headless Ghidra 12.1.2 with `tools/ghidra/DecompileAddrs.java`:
one import, then the SDK slots with callees, the constructor, and the
wavetable loader with callees. Ranges from the DLL's DFM (`tools/plugstate/dfm.py`).
Corpus: 10 uses in 8 `.flp` (7 Grand Piano, 3 mda Piano), the render test,
nine presets (eight from FL 4.1 naming "Piano", one "Rhodes (mda ePiano)") and
three copies of the default in the plugin databases. All 23 states parse.

The plugin is Paul Kellett's mda Piano and mda ePiano made into one: the
names, the constants (0.05776226505, 0.0078, 6.283, the 5000 and 14000 Hz
shelf) and the integer sample reader are mda's, and the `.wti` format comes
from the "Piano/ePiano plugins" its description file names. The differences
from mda are listed in section 5.

Confidence tags: `loader`, `dfm`, `corpus`, `asm`, `inferred`.

## 1. How the plugin is wired

`CreatePlugInstance` @ `0x0071f030` -> constructor `FUN_0071fa40`,
`TTestPlug` VMT `0x0071eef0` over `TDelphiFruityPlug` `0x0070e518`.

| slot | SDK name | address |
|---|---|---|
| 0 | DestroyObject | 0x0071ffe0 |
| 1 | Dispatcher | 0x0071f260 (4 sample rate: `+0x120 = 1 / rate`, then `FUN_00721230(-1)`) |
| 3 | SaveRestoreState | 0x0071f860 |
| 4 | GetName | 0x00721070 |
| 6 | ProcessParam | 0x007200b0 |
| 8 | Gen_Render | 0x00722360 |
| 9 | TriggerVoice | 0x00721660 |
| 10 | Voice_Release | 0x00722140 |
| 11 | Voice_Kill | 0x00722300 |
| 12 | Voice_ProcessEvent | 0x00722350 |

Other functions: parameter update `FUN_00721230`, pitch update on a moved
voice `FUN_00722e20`, text to value `FUN_007202e0`, value to text
`FUN_00720ad0`, wavetable list `FUN_007232a0`, wavetable select
`FUN_007230a0`, the load thread's `Execute` `FUN_00717370` (class
`TLoadWavTableThread`, VMT `0x00717118`), the `.wti` loader `FUN_00713920`
(class `TPianoWaveTable`, VMT `0x007133f8`), its tidy-up `FUN_00716470`, raw
file reader `FUN_00715350`, RIFF reader `FUN_00715440`, the INI bool reader
`FUN_00715710`, `default.wti` redirect `FUN_00715840`, the header reader for
the list `FUN_00715990`. RTL: `FUN_00409fe0` Round (cvtsd2si, ties to even),
`FUN_00408930` Exp, `FUN_00409b50` Ln, `FUN_0071f170` Power.

Plugin object (`asm`, from the slots and the constructor):

| offset | type | meaning |
|---|---|---|
| 0xe0 | i32 | polyphony, `DAT_0077ad74 = 32` |
| 0x120 | f32 | `1 / rate` |
| 0x124 / 0x128 / 0x12c | i32 / i32 / f32[256] | comb place, mask 0x7f, comb buffer |
| 0x134 / 0x138 / 0x13c | f32 | comb depth, key width, trim |
| 0x140 | i32 | size (hardness offset in keys) |
| 0x144 / 0x148 / 0x14c | f32 | fine, random, stretch |
| 0x150 | f32 | overdrive |
| 0x154 | f32 | muffle range, 160 |
| 0x158 / 0x15c | f32 | velocity to muffle, velocity to size |
| 0x160 | f32 | velocity sensitivity |
| 0x164 | f32 | volume, 0.2 |
| 0x168 / 0x16c | f32 | LFO sine and cosine, start 0 and 1 |
| 0x170 / 0x174 / 0x178 | f32 | LFO step, left and right depth |
| 0x17c / 0x180 | f32 | treble gain and coefficient |
| 0x184 / 0x188 | f32 | treble lowpass, left and right |
| 0x18c | i32[15] | wheel defaults |
| 0x1c8 / 0x1d0 | ptr / i32 | the stereo mix buffer and its length |
| 0x1e0 | i32[15] | the saved values |
| 0x21c | f32[15] | `p`, each value made 0..1 |
| 0x25c / 0x264 | TStringList | wavetable names (what a state saves) / their files |
| 0x26c | TPianoWaveTable | the loaded table |
| 0x274 | TList | the voices |

## 2. State (`SaveRestoreState`, `loader`)

```
i32     version, written 1; a state of 2 or more is ignored
u8      n
n bytes the wavetable's name, ANSI
i32[15] the values, read straight into +0x1e0
```

Load: the name is looked up with `IndexOf` in the names list (+0x25c,
case-insensitive); not found, or found at 0, is index 0. That table is
loaded (`FUN_007230a0(index, 0)`, from a state, so the table's own
`[parameters]` do not apply), then every value goes through `ProcessParam`
with flags 0x11. Nothing clamps a value to its wheel. The corpus has values
up to 1983 (velocity to muffle), 1844 (stereo) and 43400 (LFO), all inside.

Names in the corpus: "Grand Piano" (the database default and 7 projects),
"mda Piano" (3), "Piano" (eight FL 4.1 presets: not a name, so index 0) and
"Rhodes (mda ePiano)".

## 3. Parameters (`dfm`, `FUN_007200b0`, `FUN_00721230`)

`ProcessParam` stores `p = (float)(v - min) / (float)(max - min)` with the
wheel's range. Ranges and the wheel defaults:

| index | wheel | range | DFM default | `.wti` key |
|---|---|---|---|---|
| 0 | Decay | -1000..1000 | 0 | 0 |
| 1 | Release | -1000..1000 | 0 | 1 |
| 2 | Hardness | -1000..1000 | 0 | 2 |
| 3 | Velocity to Hardness | -1000..1000 | 0 | 3 |
| 4 | Muffle | 0..1000 | 784 | 4 |
| 5 | Velocity to Muffle | 0..2000 | 502 | 5 |
| 6 | Velocity Sensitivity | -1000..1000 | 0 | 6 |
| 7 | Stereo | 0..2000 | 1000 | 7 |
| 8 | Tune | -1000..1000 | 0 | 8 |
| 9 | Detune | 0..500 | 123 | 9 |
| 10 | Stretch | -500..500 | 0 | 10 |
| 11 | Treble | -1000..1000 | 0 | 11 |
| 12 | Pan/Tremolo | -1000..1000 | 0 | 12 |
| 13 | LFO Rate | 0..65536 | 0 | 13 |
| 14 | Overdrive | 0..1000 | 0 | 14 |

`FUN_00721230` (in doubles unless said, stored as f32):

```
size      = Round(12 p2 - 6)
sizevel   = 0.12 p3
muffvel   = 5 (p5 * p5)
treb      = 4 p11 p11 - 1;  tfrq = p11 <= 0.5 ? 0x3ddb851f (0.10715) : 0x3e8b442a (0.27200)
lmod      = 2 p12 - 1;  rmod = lmod, sign flipped when p12 < 0.5
dlfo      = (1/rate) * 6.283 * exp(9.22 p13 - 4.61)
velsens   = 1 + 2 p6;  when p6 < 0.25: velsens -= 0.75 - 3 p6
fine      = 2 p8 - 1                      keys
random    = 0.077 p9 p9
stretch   = 0.000434 (p10 - 0.5)
overdrive = 1.8 p14
cdep      = p7 * p7 (f32);  trim = 1 - 0.53 cdep;  width = min(0.04 p7, 0.03)
```

`tfrq` is mda's `1 - exp(-f / rate)` for 5000 and 14000 Hz, worked out at
44100 Hz and kept as constants at every rate. A sample rate change runs the
whole update; voices that sound keep what they were started with.

## 4. Wavetables (`FUN_007232a0`, `FUN_00713920`, `FUN_00716470`)

`Plugins/Fruity/Generators/FL Keys/Wavtables/*.wti`, INI files read with
`TMemIniFile`; `wti_description.txt` beside them is accurate. The list skips
`default.wti`, whose `[default] file=` names the table put first (index 0);
the others follow in the order the directory search returns them. The name
saved in a state is `[info] name`; the combo shows `shortname`, with " (FL)"
for `isstandard=1`. FL 2026 ships:

| file | name | type | rate | layers | samples | velocity factor | release high |
|---|---|---|---|---|---|---|---|
| mda Piano.wti | mda Piano | raw `mda piano.dat` | 22050 | 1 | 15 | 3 | no |
| Rhodes.wti | Rhodes (mda ePiano) | raw `rhodes.dat` | 32000 | 3 (0, 48, 80) | 33 | 3 | yes |
| Grand Piano.wti | Grand Piano | riff, `Grand Piano/*.wav`, `canloop=false`, `resample=false` | 44100 | 1 | 30 | 4 | no |
| Roto Organ.wti | Roto Organ | riff, `Roto Organ/*.wav` | 44100 | 1 | 7 | 1 | no |

Raw files are 16-bit mono little-endian. The Grand Piano WAVs are Ogg Vorbis
inside RIFF (format 0x674f, stereo); the Roto Organ WAVs are 16-bit mono PCM
with a `smpl` loop.

Loader, per table:

- `[info]`: `samples`, `samplefile`, `filetype` (default `riff`), `path`
  (added to the `.wti`'s directory), `velocityfactor` (default 1),
  `releasehighnotes` (default false), `isstandard`, `resample` (default
  true), `canloop` (default true), `samplerate` (default 44100). Bools are
  true when the text is "TRUE" in any case, and the default when empty.
- `[velocitylayers]`: `count` as a byte (0 becomes 1), then `0..count-1`
  with default 128; layer 0 is forced to 0.
- `[sampleN]`: `root` (default -1: the one before, starting at 60), `high`
  (default 0; -1: the one before), `pos`, `end`, `loop`, `canloop`, `name`.
- Raw with `samplefile`: the file is read whole and `pos`, `end`, `loop` are
  frames into it.
- RIFF with a `name` per sample: each file is appended to one buffer of
  16-bit points (both sides by turns when the file is stereo, the table's
  stereo flag `+0x60` being the last file's), with one spare frame after
  it. A sample that names a file an earlier sample named copies that
  sample's record. Otherwise, with `b0` the file's first frame:
  `pos = b0 + pos`, `end = b0 + end` and, when that is `b0`,
  `b0 + loop_end`; `loop` defaults to `loop_end - loop_start + 1`, from the
  file's `smpl` loop clamped into the file (FL's WAV reader, `+0x108` and
  `+0x110`). The spare frame gets the point `end - loop`. With `resample`
  the file is first resampled to the song's rate (`FUN_005fae10`, not read).
- A file that does not load fails the whole table.

Tidy-up `FUN_00716470`: `root` into 0..127, `high` into 0..999, `pos` into
0..frames-1, `end` into pos..frames-1, `loop` into 0..frames-pos, and a sample
that may not loop gets `loop = end`, which ends its voice at its end.

`TPianoWaveTable` fields: `+0x40` the samples (0x124 bytes each: root,
high, pos, end, loop at +0..+0x10, can-loop byte +0x14, name +0x24), `+0x48`
the layer thresholds, `+0x50` the layer count, `+0x54` the points, `+0x5c`
their bytes, `+0x60` stereo, `+0x64` f32 rate, `+0x68` f32 velocity factor,
`+0x6c` release high.

## 5. A voice

### Trigger (`FUN_00721660`)

With `Final` and `Init` the SDK's `TVoiceParams` levels (`docs/plugins/host-voice.md`):

```
note  = clamp((Final.Pitch + 6000) * 0.01f, 0, 127)                f32, keys
vel   = min(127, Round((f32)(ln(Init.Vol * 10 + 1) * (1/ln 11)f) * 100))
k     = Round((note - 60)^2)
tune  = fine + random * ((k mod 13) - 6.5);  if note > 60: tune += stretch * k
s     = size;  if vel > 40: s += Round(sizevel * (vel - 40))
group = first multiple of layers with (f32)(high + s) >= note
layer = 1; while layer < layers and threshold[layer] < vel: group++, layer++
delta = Round((f32)(rate * (1/rate_song) * exp((tune + note - root) * 0.05776226505)) * 65536)
env   = velocity factor                         (mda: (0.5 + velsens) (0.0078 vel)^velsens)
l     = p4 p4 160 + 50 + muffvel (vel - 64);  l = max(l, 0.4 note + 55);  l = min(l, 210)
ff    = l l / rate_song
kp    = clamp(note, 12, 108);  v = 0.2 trim
outr  = v + v width (kp - 60);  outl = 2 v - outr
dec   = p0 == 0 ? 1 : exp(-(1/rate) exp(0.033 max(kp, 44) - 0.6 - L)),
        L = 2 p0, and when under 1: L + 0.25 - 0.5 p0
```

At 32 voices or more that are not dying, the one with the lowest `env` is
marked to die. Voice record (100 bytes): `+0x14` dying, `+0x18` the pitch it
was started at, `+0x1c` delta, `+0x20` frac, `+0x24` pos, `+0x28` end,
`+0x2c` loop, `+0x30` env, `+0x34` dec, `+0x38/+0x3c` left lowpass and last
input, `+0x40/+0x44` right, `+0x48` ff, `+0x4c/+0x50` outl/outr, `+0x54` the
note, `+0x58` velsens, `+0x5c/+0x60` the pan law's gains.

### Gen_Render (`FUN_00722360`)

Nothing runs, the effects included, while the voice list is empty. Per
block, per voice: when `Final.Pitch + 6000` is not what the voice started
at, `FUN_00722e20` works `delta` out again; then
`ComputeLRVol(&gains, Final.Pan, Power(max(Final.Vol, 0), velsens))` (host
slot 0xf8). Per point:

```
frac += delta;  pos += frac >> 16;  frac &= 0xffff
if pos > end: if end <= loop: die, stop this voice;  else pos -= loop
i  = (s[pos] << 7) + (frac >> 9) * (s[pos+1] - s[pos]) + 0x40400000
x  = (f32)(env * (bits_as_f32(i) - 3.0));  env *= dec
if x > 0 and overdrive != 0: x -= overdrive x x;  if x < 0: x += 0.5 x x
f0 += ff (x + f1 - f0);  f1 = x
L += gainL outl f0;  R += gainR outr f0r        (mono: f0r = f0)
```

Stereo tables read the two sides of the frame with the same `pos` and `frac`
and run their own overdrive and lowpass. Then, per point of the sum:

```
tl += tfrq (L - tl);  tr += tfrq (R - tr)
R += treb (R - tr);  L += treb (L - tl)
lfo0 += dlfo lfo1;  lfo1 -= dlfo lfo0
L += L lmod lfo1;  R += R rmod lfo1
comb[c] = L + R;  c = (c & 0x7f) + 1;  x = cdep comb[c]
out L = L + x;  out R = R - x
```

So the comb's delay is 127 points (mda's is 128). At the end of the block a
voice whose `env`, compared as bits, is under 1e-4 dies, dying voices go to
the host's kill, and the two treble states under 1e-10 go to 0. The sum is
then added to the output.

### Release (`FUN_00722140`)

```
if not release high and (note < 94 or note == 128):
    dec = exp(-(1/rate) exp(0.017 note + 2 - 2 p1))      mda Piano
else:
    dec = exp(-(1/rate) exp(0.01 note + 6 - 5 p1))       mda ePiano
```

mda Piano does not release keys from 94 up; FL gives them the ePiano's rate.

### Differences from mda

The envelope starts at the table's velocity factor and the velocity reaches
the level only through `Power(Final.Vol, velsens)`; the muffle floor is
`0.4 note + 55` for both tables (mda Piano's is `0.25 note`); `trim` is
`1 - 0.53 cdep` (mda: `1.5 - 0.79 cdep`); fine tune spans a key each way;
the reader steps before it reads; the comb runs 127 points.

## 6. In fl-rs

`crates/plugins/src/fl_keys.rs`. `set_resource_dir` reads every `.wti` and
the raw files; for a RIFF table `sample_paths` gives the files by full path
and the host decodes them, and the `smpl` loops are read from the files'
headers. Exact as above, with these left:

- The voice's level is `Power(Final.Vol, velsens)` with `Final.Vol` the
  note's velocity gain times `chan.Vol`, which the engine hands over with
  `Generator::set_channel_volume` on load and whenever automation moves the
  channel's volume; the voice divides by `chan.Vol`, since the engine puts
  it on the plugin's output. Gen_Render works the gains out every block, so
  a volume that moves under a note moves it as FL's does.
- The RIFF files come decoded from the host, rounded to 16 bits. FL decodes
  the Vorbis itself.
- Pitch is fixed at the trigger and a slide note sounds nothing; FL's
  `FUN_00722e20` follows `Final.Pitch` every block.
- The `resample` load step is not built (Roto Organ only), and fidelity says
  so for such a table at another rate than its own.
- The spare frame of a RIFF table's first sample reads before the buffer in
  FL (`end - loop` is -1); here it is 0. It is read once, by the last point
  of a voice that is about to end.
