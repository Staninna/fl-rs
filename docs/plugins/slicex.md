# Slicex: state, regions, key map, voice

Recovered from `Slicex_x64.dll` (FL Studio 2026, Delphi, image base
0x400000). Ghidra project `ghidra/proj/slicex`; decompiles under
`ghidra/out/slicex/` (`slots/` the SDK slots, `ctor/` the constructor,
`state/` the loader's callees and the voice, `wav/` the WAV writer and the
sample helpers, `art*/` the articulator classes). Corpus: 43 states in 18
`.flp`, 7 in `.fst` presets, 50 in all; 21 carry their sample, 29 link a file.

Confidence tags: `loader`, `asm`, `dfm`, `corpus`, `inferred`.

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
key map), `+0xdc0 + g * 0x50` articulator group `g` (`+0x14` its six
articulators, `+0x48`/`+0x4c` its cutoff and resonance offsets), `+0x1040`
the mod wheel smoothing, `+0x1050` the control tick, `+0x106c` the
declicker, `+0x1080` the pitch knob in cents, `+0x1084` the volume, `+0x1088`
the randomness, `+0x108c` the LFO level, `+0x10a0` the tempo, `+0x12f0` the
47 bytes of settings. `+0xcc` is the unit's rate object (`FUN_00637db0`:
`+8` the rate, `+0x18` 1/rate, `+0x20` 44100/rate, `+0x38` the steepest
gain ramp, `10 * 0.001 * 44100/rate` a sample).

## 1. State layout (event 213)

`FUN_0087a8a0`, load side:

```
i32 version            FL writes 0x60003; 0x60004 and above: nothing is read; above 0xffff: >> 16
i32[8] knobs           +0x140: 0, 0, volume (12800 unity), randomness 256, LFO level 128, pitch (cents), mod X 256, mod Y 256
u8[0x2f] settings      +0x12f0 (0x2e below version 2): i32 layer mode, i32 crossfade mode (3), ...,
                       +0x28 sample start over the whole region, +0x29 a 4-sample tick,
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
8 x articulator group:
    i32[5]             +0xdc0 + g * 0x50: filter envelope amount (128), cutoff (128), resonance (128),
                       filter type (0 low, 1 band, 2 notch, 3 high pass), stages (0 off, up to 3)
    6 x articulator (TGroupArticulator, load FUN_008869a0):
        i32 version (3, not checked), f32[3] (+0x20: base, envelope amount, LFO amount)
        MEnvelope, TMEnvelope class (4 knobs)       +0x10
        MEnvelope, TMLFO class (4 knobs, phase)     +0x18
        4 x MEnvelope, TMMap class                  +0x30 mod X, +0x38 mod Y, +0x40 velocity, +0x48 random
```

All 50 corpus states read to the last byte this way. Every corpus slice has
the 17 words `0, 0, 0, 8000, 0, 12800, 128, 128, 0, 12800, 0, 0, 12800, 0, 0, 0,
12800`. Volume knobs seen: 12800 x45, 13549 x3, 15312; pitch knobs 0, 455, 760.

Slice words (`FUN_008874a0`; the names are the region panel's controls in
the editor's DFM):

| word | float | meaning |
|---|---|---|
| 0 | | above 0 a hit calls `FUN_00885150` with it: the cut group |
| 2 | `/ 6400` | pan, -1..1 (`RPanWheel`) |
| 3 | `/ 8000`, then 1 at 1 and `(e^(x ln 3) - 1) / 2` elsewhere | volume (`RVolWheel`, 0..16000) |
| 4 | | the amp articulator group (pan and volume), -1 for none (`RAmpGroupSelect`) |
| 5 | `/ 12800` | how much of it the slice takes (`RAmpGroupWheel`) |
| 6, 7 | `/ 256` | the filter's cutoff and resonance (`RFCutWheel`, `RFResWheel`) |
| 8 | | the filter articulator group |
| 9 | `/ 12800` | how much of it the slice takes |
| 10 | as is | pitch, cents (`RPitchWheel`) |
| 11 | | the speed articulator group (`RScratchGroupSelect`) |
| 12 | `* 8 / 12800` | how much of it the slice takes |
| 13 | `/ 12800` | start delay in beats (`RDelayWheel`) |
| 14 | `/ 12800` | sample start (`ROfsWheel`) |
| 15 | | the sample start articulator group |
| 16 | `* 2 / 12800` | how much of it the slice takes |

The volume knob is FL's knob curve: `(11^x - 1) / 10` with `x = knob /
12800`, 1 exactly at 12800 (`FUN_0087efe0`, in single precision with `ln 11`
and `0.1` as f32; `ln 11` is worked out when the unit starts, at
`0x0063a8d5`). The randomness (knob 3 / 256) and the LFO level (knob 4 /
128) take the same curve on their size and keep their sign.

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
colour = Voice_ProcessEvent(FPV_GetColor); deck = colour & 1; above 13 the slice plays backwards
vel   = Voice_ProcessEvent(FPV_GetVelocity); when +0x131b is clear, FPV_SetLinkVelocity(0)
slices = the deck's key map at the note; none: no voice
with two or more: the layer mode picks (0 all at 1, 1 all at 1/n, 2 by velocity, 3/4 by mod X/Y,
    5 at random, 6 round robin, 7 by time) and the crossfade mode shapes the others
```

Each slice gets a voice (0x97c bytes): the slice's 0x70 bytes at `+0x18`,
the articulators of its groups at `+0x12c` (six pointers: the amp group's
first two, the filter group's next two only when that group's filter is
on, the speed group's fifth, the sample start group's sixth), the filter
(`+0x7f8` type, `+0x7fc` stages less one, `+0x80c + i * 0x78` stage `i`) and
FL's interpolator (constructor `FUN_0061f6c0`, the class of
`docs/plugins/fl-interpolator.md`) on the region:
`FUN_00620360(player, data + start, channels, format, 0, 0, end - start)`
sets the player's frames to `0 .. end - start + 1` (`+0x58 = last + 1`), so
the region's last frame sounds, and the frames past the region's ends that
the kernel reaches are the region's own end frames, not the file's
neighbours. The voice sets `+0x77` (a read past the end goes on and says
where it ended) and no quality, so the unit's, which the host sets through
`FPD_ProcessMode`, applies (`FUN_0061fa90`), as in FPC. The voice's pitch
takes `(60 - note) * 100` so a key plays its slice at the file's pitch.

Also at the trigger:

- the start delay in samples: `Round(delay * rate * 60 / tempo)` (f32 sums);
- the sample start: `start * min(0.1 * rate, len)` frames, or `min(start,
  1) * len` with setting `+0x28`; a reversed voice counts it back from the
  region's last frame; the player starts there, kept to `0..len`;
- the declicker's state (`FUN_00876830`): its attack runs only for a voice
  that starts past frame 0;
- each linked articulator starts its envelope and LFO
  (`FUN_00799c70`); the value a voice starts from is 1 for volume and 0
  for the rest (`DAT_00883960 = 0x02`).

## 5. Rendering (`FUN_008843d0`)

`Gen_Render` hands every voice the whole block. A voice still in its start
delay waits; then it runs in chunks no longer than the time to its next
control tick (`+0x90`). The tick (`FUN_008853e0`) is `max(4, 4 *
Round(48 * rate / 44100 / 4))` samples, 48 at 44.1 kHz and 52 at 48 kHz (4
in place of 48 with setting `+0x29`). The first tick falls on the voice's
first sample and moves the articulators by 0 ticks, the rest by 1.

On a tick:

```
state > 4 (let go): state = 9
each linked articulator i: value = 1 (volume) or 0, through its mod X and mod Y maps,
    then FUN_00886a20: the envelope moves on; value + env * env amount (one that adds) or value * env;
    the LFO moves on; value + lfo * lfo amount or value * (lfo * lfo amount * 2 + 1);
    state > 4: state = min(state, envelope stage, LFO stage)
state > 8 and the declicker's release not started: start it now
pan  = clamp(slice pan + Final.Pan + pan value * 2 * amp amount, -1, 1)
vol  = slice volume * Final.Vol * plugin volume * ((volume value - 1) * amp amount + 1)
gains (FUN_006226e0): pan 0 -> (vol, vol); else ComputeLRVol(pan, vol * sqrt 2) and the far side's
       share, 1 - near / vol, half of it sent across (right into left for a pan to the left)
each gain ramps to its new value over the tick, no steeper than the rate object's +0x38 a sample
       (FUN_00637c60); on the first tick it starts there
```

A switched-off envelope or LFO starts at stage 9 (`FUN_00796130`), so a
voice let go of whose articulators are all off or over fades out at its
next tick. One that is not let go plays on, at whatever its articulators
leave, until its region ends.

Every chunk:

```
pitch = slice pitch + (60 - note) * 100 + Final.Pitch + pitch knob (f32);
        step = e^(pitch ln 2 / 1200) * file rate / rate, less than 0 reversed (FUN_006200d0)
the player reads the chunk (FUN_00621cb0). Forwards, a read that gets past the region's last
        frame says where (a short read) and goes on with that frame held; backwards it holds frame 0
a short read, or a reversed player at or below 0: state 9 and the declicker's release starts,
        from the frame the region ended on (backwards: from the chunk's start)
the filter, when the group has it on (section 7)
the declicker (FUN_00876880, section 6)
the mixer (0x91b5c0, FUN_00635d10) adds the chunk at the ramping gains:
        left += l * gL + r * gRL, right += r * gR + l * gLR, each gain stepping once a sample
```

After the block, a voice at state 9 whose declicker release has run its
length is over (state 10). Voice_Release puts the voice in state 5 (if it
was below), lets its articulators go (`FUN_00796380`), and ends a voice
still waiting to start.

## 6. The declicker (`+0x106c`, `FUN_00875ca0`)

`FUN_008854c0` (on a rate change and after a state loads) calls
`FUN_00876710(decl, 2, 4.0, 0.0, 1, 20.0, 0.0)`: attack mode 2 over 4 ms,
release mode 1 over 20 ms, no curve. `FUN_00876030` builds the tables, in
f64 and kept as f32, with `n = Round(ms * 0.001 * rate)`:

```
attack, mode 2:  a[i - 1] = (cos(i / (n + 1) * pi) + 1) / 2,   i = 1..n
release, mode 1: m = min(n, Round(rate * 0.001))
                 cut[i - 1]  = min(1, 2 / rate * e^(i * (ln 22050 / (ln 2 * n)) * ln 2))
                 gain[i - 1] = 1, and over the last m: (cos((i - (n - m)) / (m + 1) * pi) + 1) / 2
```

`ln 2` is the unit's `DAT_0091b500` (`FUN_0063a840`). The voice's state
(`+0xfc`): attack frames gone (0x7fffffff for none), release frames gone (-1
before it starts), frames the release waits, the first frame negated, the
release's one-pole per side.

On every chunk (`FUN_00876880`):

- attack: on its first frame it keeps that frame negated; frame `k` of the
  attack adds `a[k] * -first` (`0x91b568` = `FUN_006357e0`, an add of a
  mono table times a stereo constant). The voice fades in from 0.
- release, once started and past its wait: per frame, `i = min(k, n - 1)`,
  `dc += cut[i] * (x - dc)`, `x = (x - dc) * gain[i]`, in f32. Past `n` it
  keeps to the last entries until the block ends.

Modes 3 and 4 (cosine and exponential fades, tables `FUN_00875e10` and
`FUN_00875f50`) are set up nowhere in Slicex.

## 7. Articulators and the filter

A group's six articulators (`TGroupArticulator`, VMT `0x879960`; the speed
one `0x879ae0`) move pan, volume, filter cutoff, filter resonance, speed
and sample start. The constructor (`FUN_0087cfc0`) gives each its mode by
`DAT_0087dc7a = 0x12`: volume and speed multiply (centre 0), the rest add
(centre 0.5). Envelope and LFO are FL's shared `TMEnvelope` (VMT
`0x78fc00`: start `0x798490`, tick `0x796540`, release `FUN_00796380`) and
`TMLFO` (VMT `0x78fda0`). Their rates (`FUN_00885300`): `2 tick / rate` half
seconds and `tick / rate * tempo / 60` beats a tick.

After a state loads FL sets every parameter again (`ProcessParam(i,
ProcessParam(i, 0, get), set)` for each), so what `FUN_0087efe0` and the
group branch of `ProcessParam` work out holds: every articulator's LFO
amount is the LFO level knob's, the cutoff articulator's envelope amount is
the group's first word / 128 and its LFO amount that times the LFO level,
and the group's cutoff and resonance offsets are `(word - 128) / 256`.

The filter (`FUN_00883cd0`, run by `0x91b470` = `FUN_00631e20`, a direct
form I biquad in f64 per stage, the stages in turn) takes

```
cut = slice cutoff + (group cutoff offset + cutoff value) * filter amount
res = slice resonance + (group resonance offset + resonance value) * filter amount
```

and designs every stage (`FUN_00631600`, the RBJ forms; type `t` is kind
`t + 1`) as Autogun's filter does (`docs/plugins/autogun.md` section 4)
but at the song's rate: `w = (f + 0.003 or 0.0001) * 2 pi * 44100 / rate`
kept to 0..2.8495; low and high pass `f = (1501^cut - 1) * 6.6467e-4` or
`3.9993e-4` (capped at 0.6001), `Q = (2001^res - 1) * 0.02495 + 0.1` (0.1
at 0, 50 at 1) taken to the power `1 / stages`, alpha `sin w / (2 Q)`; band
pass and notch `f` capped at 1.0001 and bandwidth `(101^res - 1) * 0.1199
+ 0.01` octaves times the stages (over them for the notch), alpha `sin w
sinh(ln 2 / 2 * bw * w / sin w)`.

In the corpus the volume envelope is on in 8 states (Complextreon,
Razer Chroma, Steptime, Song For You and the Sustaining preset), the cutoff
and resonance envelopes in the 9 genre templates (`Breakbeat` to `Trap`),
and the filter in 5 song states (high pass with 1 or 2 stages in
Complextreon, Housework and Razer Chroma) and the 9 templates (low pass).
No LFO, map, speed or sample start articulator moves anything, and every
slice takes group 0 for all four.

## 8. Parameters (`ProcessParam`, `FUN_0087e4f0`)

0x160 of them (`FUN_0087f320`): the 8 knobs, then 43 for each group: its
first three words, then for its first five articulators the envelope's
attack, decay, sustain and release knobs and the LFO's speed, tension, skew
and pulse width. A value with flag 0x20 is spread over its control's range;
the controls (the editor's `FUN_0088a200` files them by `Tag`) are, from the
DFM:

| parameter | control | range |
|---|---|---|
| 0, 1 | none (`ProcessParam` makes knob 1 a switch) | 0..1 (inferred) |
| 2 | `MainVolSlider` | 0..16000 |
| 3 | `MainRandSlider` | 0..256 |
| 4 | `MainLFOSlider` | -128..128 |
| 5 | `MainPitchSlider` | -1200..1200 |
| 6, 7 | `ModXWheel`, `ModYWheel` | 0..512 |
| group + 0..2 | `FEnvWheel`, `FCutWheel`, `FResWheel` | 0..256 |
| group + 3 + 8a + 0..3 | `EnvAttWheel`, `EnvDecWheel`, `EnvSusWheel`, `EnvRelWheel` | 0..256, 0..256, -128..128, 0..256 |
| group + 3 + 8a + 4..7 | `LFOSpeedWheel`, `LFOTensionWheel`, `LFOSkewWheel`, `LFOPWWheel` | 512..65536, -128..128, -128..128, -112..112 |

Knobs 2 to 5 run `FUN_0087efe0` again, a group's words the group branch,
an envelope knob `FUN_007983d0` (voices that start later hear it: a run
copies the knobs' scales when it starts, `0x798490`) and an LFO knob the
LFO's update (running voices hear it, inferred).

## 9. What stays unknown

- The WAV region reader (the writer was read instead).
- The articulators' maps baked into tables (`FUN_00887610`), the speed
  articulator's slip of the gains (`+0xf8`, the `local_6c` correction in
  `FUN_008843d0`), the layer crossfade curves and the cut groups
  (`FUN_00885150`).
- The quality FL's render hands the player (see `docs/plugins/fpc.md`).
- What the deck settings' bytes and the knob words 0 and 1 do. The deck
  panel's stretch tools rewrite the sample, which the state then carries;
  no voice code stretches anything.

## Implementation notes (crates/plugins/src/slicex.rs)

- Sections 1 to 8 are built, but: the speed and sample start
  articulators, the articulators' maps (a map that is not flat at its
  neutral level), cut groups and the layer modes that pick one slice of
  several are left out, and `fidelity` names each when a state asks for
  it. No corpus state does.
- The envelope runner is `harmor_art.rs`'s port of the shared `TMEnvelope`
  (Harmor's `FUN_007a0c60`), the LFO `menvelope.rs`'s; the biquad, the pan
  gains and the gain ramp are Autogun's (`autogun.rs`), the same units of
  FL's code.
- The file is read through FL's interpolator on the region's frames
  (`sampler::Wave::read_in`) at FL's default render quality, 16 taps a side
  (inferred).
- A linked file is decoded by the host; its regions are read by the plugin
  from the file under FL's install, found from
  `Generator::set_resource_dir`.
