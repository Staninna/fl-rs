# Fruity Granulizer: state, parameters, grain engine

Recovered from `Fruity Granulizer_x64.dll` (FL Studio 2026, Delphi, image
base 0x400000). Headless Ghidra; decompiles of the VMT slots and their
callees. Corpus: 2 projects (the render test and the Vocoder template) and
12 presets, in 44, 48, 56, 60 and 428 bytes.

Confidence tags: `loader`, `dfm`, `corpus`, `inferred`.

## How the plugin is wired

`CreatePlugInstance` @ `0x7514f0` -> constructor `FUN_00751ce0`; class
reference `*0x7512f0` -> VMT `0x7513b8` (`TTestPlug` on
`TDelphiFruityPlug`, 0x1f0 bytes).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x751540 (4 sample rate, 10 `FPD_SetSample` -> `0x754b30`, 0x13 the sample's beats) |
| 3 | SaveRestoreState | 0x7519f0 |
| 5 | ProcessEvent | 0x752110 (tempo -> `+0x1a0`) |
| 6 | ProcessParam | 0x752580 |
| 8 | Gen_Render | 0x754f00 |
| 9 | TriggerVoice | 0x753580 |
| 10 | Voice_Release | 0x753870 |
| 11 | Voice_Kill | 0x753890 |
| 13 | Voice_Render | 0x754830 -> `0x753ea0` |
| 14 | NewTick | 0x754ff0 |

Fields: `+0x120..+0x14c` the twelve parameters, `+0x150` the sample
(`+0x70` its rate, `+0x88` frames, `+0x100` its transients, `+0x108` /
`+0x110` the loop's first and last frame), `+0x158` the loop's length,
`+0x164` the transient source, `+0x168` the trigger mode, `+0x16c` attack,
`+0x170` hold, `+0x174` transient hold (samples), `+0x178` attack + hold,
`+0x17c` its inverse, `+0x180` grain spacing (samples), `+0x184` the start
knob in frames, `+0x188` advance a grain (frames), `+0x18c`, `+0x190`
effect depth, `+0x194` effect speed, `+0x198` the sample's length in
samples at the song's tempo, `+0x19c` the sample's beats, `+0x1a0` the
tempo, `+0x1b8` loop, `+0x1bb` tempo lock, `+0x1c8` the instrument tab's
voice engine.

## 1. State (`0x7519f0`, `loader`, `corpus`)

| at | what |
|---|---|
| 0 | version, i32: FL 2026 writes 7 |
| 4 | up to 12 i32 parameters (`+0x120`, read as up to 0x30 bytes); before the read parameter 10 is set to 0 and parameter 11 to 248 |
| next | the transient source, i32, from version 4 on (0 otherwise) |
| next | the trigger mode, i32, from version 5 on (0 otherwise) |
| next | the tempo lock, i32 whose low byte goes to `+0x1bb`, from version 7 on |
| next | the instrument tab, 0x16c bytes (`0x746080`): five blocks of 17 i32 and the filter, as 3x Osc's |

The corpus has version 1 (44 bytes: ten parameters), 2 (48), 4 (56), 5
(60) and 7 (428).

## 2. Parameters (`dfm`, `0x752580`)

| # | wheel | min | max | default |
|---|---|---:|---:|---:|
| 0 | GrainAttWheel | 50 | 500 | 204 |
| 1 | GrainHoldWheel | 0 | 500 | 50 |
| 2 | GrainSpaceWheel | 125 | 1000 | 611 |
| 3 | WaveSpaceWheel | -1000 | 1000 | 611 |
| 4 | FXDepthWheel | 0 | 384 | 0 |
| 5 | FXSpeedWheel | 4 | 200 | 180 |
| 6 | RandomWheel | 0 | 256 | 0 |
| 7 | StereoWheel | 0 | 64 | 0 |
| 8 | LoopBtn | 0 | 1 | 0 |
| 9 | HoldBtn | 0 | 1 | 0 |
| 10 | StartWheel | 0 | 4096 | 0 |
| 11 | GrainTransHoldWheel | 0 | 500 | 228 |

Not parameters: the transient source (`TransModeSelect`: off, use
regions, detect), the trigger mode (`TriggerModeSelect`: key to pitch, key
to start in sample time C5 to C7, in song time, in transient time) and
the tempo lock.

ProcessParam: 0 to 3, 8 and 11 call `0x752290`; 4 sets
`+0x190 = v / 256`; 5 sets `+0x194 = pi / (204 - v)`; 10 sets
`+0x184 = MulDiv(v, last frame, 4096)` (`0x754840`, 128-bit, truncated).

`0x752290`, with `rate` FL's rate as an integer (`0x79cd6c`, written by
the dispatcher's sample rate):

```text
ms(v)    = Round(float(51^(float(v * 0.002)) - 1) * 10f)       0x752170
curve(v) = sign(v) Round(float(11^(float(|v| * 0.001)) - 1) * 76.8)   0x7521f0
attack   = float(ms(p0) * rate * 0.001f)                        0x60dbf0
hold     = float(ms(p1) * rate * 0.001f)
grain    = attack + hold
spacing  = max(grain * curve(p2) * (1/256), 5)
advance  = float(grain * curve(p3) * file rate * (1 / rate) * (1/256))
```

With the tempo lock on (and the host's beats above 0) `advance` is
instead `spacing * frames / (+0x198)`. The 0.001 is single precision, so
the default hold is 220.50002 samples at 44.1 kHz and rounds to 221.

## 3. A note (`0x753580`)

No voice without a sample. The voice: its params, `+8` cents to take off
(as a float), the grain list, `+0x1c` grains started, `+0x20` the sample a
grain starts next, `+0x24` the samples rendered, `+0x28` the transient,
`+0x2c` the place (float, starts at `+0x184`), `+0x30` the effect's
phase, `+0x34` ended, `+0x35` looped, `+0x36` the tab's voice
(`0x745150`).

Trigger modes, with `k = Round(InitLevels.Pitch * 0.01) - 60`, which is
`key - 60` (no fine pitch), and `+8 = -100 k`; `k` below 0 counts as 0:

- 1, sample time: the place moves on by `last * k / 24`;
- 2, song time: by `k / 4 * 60 * file rate / tempo` with no beats from
  the host, else `last * (k / 4) / beats`;
- 3, transient time: to the `k`th transient (not built here).

The place is then `Round(place + moved) mod (last + 1)`.

## 4. A block of a voice (`0x753ea0`, `inferred` for names)

The voice's output is cleared, then, while the voice has not ended:

```text
before = counter; counter += len
while next < counter:
    place_i = Round(place)
    if random < 1 or hold or random <= 255 - Random(256):
        (the transient branch: only with transients and advance > 0)
    else:
        place_i += Random(loop length); wrap down by the loop length
    start a grain at place_i, next - before samples into the block
    if next == 0 and place == 0: the first grain starts at full gain,
        its hold longer by the attack
    next += Round(spacing)
    if not hold:
        if depth > 0: place -= Round(|sin(phase)| * advance * depth)
        phase += speed
        place += advance
        past the last frame: looped with loop, else ended (and over
            when no grain sounds); wrap down by the loop length
        not looped: below 0, up by last + 1; looped: up into the loop
```

A grain (`0x753c70`, only at `place_i <= last`): a sample player
(`0x5fd080`, region the whole sample, looping with loop on) from
`place_i`, at `exp((FinalLevels.Pitch + (+8)) * ln 2 / 1200) * file rate /
rate` frames a sample (exactly 1 at 0 cents and equal rates, when the
player copies the frames as they are, `0x5fce50`). Its fade runs from
`-attack` (gain and step `1/attack`) to 0, holds to `hold`, then steps
`-1 / attack` to `hold + attack` (`Round`ed samples). Its pan is
`stereo / 64` through the pan law at `0x60db70` with a volume of 1.42
(`0x753e98`); every grain with an odd count has left and right swapped,
so the first goes left.

Each grain renders its frames (`0x5fd890`) and is mixed into the voice
with its fade, stretch by stretch (`0x60b3f0` constant, `0x60b5c0`
ramped). A grain whose player has run out (no loop) or whose fade is
over is freed; the voice is over when it has ended and has no grain.

## 5. Gen_Render and the tab (`0x754f00`, `0x7451f0`)

Every voice goes through the instrument tab's voice engine, which calls
Voice_Render and runs the tab's envelopes on the ticks (`0x745680`, which
also adds the pitch envelope to `FinalLevels.Pitch`) and its filter, pan
and volume over the block (`0x7456c0`). This is the code 3x Osc links,
and `crates/plugins/src/tab.rs` plays it from 3x Osc's port.

## Not built

- The sample player's interpolation kernel (`+0xf0`, a windowed sinc of
  up to 512 points by `0x5fce50`): a Hermite curve stands in.
- Transients: the regions of the file, FL's detection (`0x7397a0`,
  `0x7400a0`), the transient branch of the block and the transient hold.
- The tempo lock: it needs the sample's beats, the dispatcher's 0x13.
- A file's own loop points (`+0x108`): the loop is the whole sample.
