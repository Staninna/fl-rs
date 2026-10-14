# Fruity Flangus: state, parameters, signal chain

Recovered from `Fruity Flangus_x64.dll` (FL Studio 2026, Delphi native
plugin, image base 0x400000). Ghidra project `ghidra/proj/flangus`,
decompiles under `ghidra/out/flangus/` (`vmt/` the slots with callees,
`dsp/` the delay line class, its two renderers and the gain ramps, with the
listings under `dsp/asm/`). Corpus: 56 uses in 20 `.flp`, 12 shipped presets
plus the three copies of the default in the plugin database. Three Ghidra
runs: import, the slots, the delay line. No engine DLL and no FLEngine
service beyond the two ramped copies every stock effect shares. The whole
effect is the class `TStereoFlanger` (parent `TBaseDelayLine`), 0x78 bytes.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `inferred`, `asm`.

## How the plugin is wired

`CreatePlugInstance` @ `0x006b6630` -> constructor `FUN_006b6be0`,
`TTestPlug` VMT `0x006b6510` (instance size 0x170).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006b6680 |
| 2 | Idle_Public | 0x006b2510 |
| 3 | SaveRestoreState | 0x006b6b20 |
| 4 | GetName | 0x006b73e0 |
| 5 | ProcessEvent | 0x006b1ba0 |
| 6 | ProcessParam | 0x006b6fb0 |
| 7 | Eff_Render | 0x006b7510 |

Plugin object:

| offset | type | meaning |
|---|---|---|
| 0xd4 / 0xdc | ptr, i32 | wet buffer and its length in frames (grown to the block, rounded up to 4) |
| 0x120 | i32[8] | the parameters, saved |
| 0x140 | ptr | the `TStereoFlanger` |
| 0x148 / 0x14c | f32 | dry gain: target, ramp state |
| 0x150 / 0x154 / 0x158 | f32 | wet gain: target, ramp state left, ramp state right |
| 0x15c | i32 | block size from the host (Dispatcher 3) |

`TStereoFlanger` (constructor `FUN_006b3cd0`, base `FUN_006b3960`):

| offset | type | meaning |
|---|---|---|
| 0x08 | ptr | ring, interleaved stereo f32 |
| 0x10 | ptr | read position: where the current block starts in the ring |
| 0x18 | ptr | write position |
| 0x20 | ptr | ring end |
| 0x28 | i32 | ring size in bytes: `(block size + 0x54) * 8` |
| 0x2c | i32 | bytes a frame: 8 |
| 0x30 | f32 | the spread the ring was last cleared for |
| 0x34 | i32 | voices minus 1 |
| 0x38 | i32 | order (p0) |
| 0x3c | f32 | depth, p1 / 1024 |
| 0x40 | f32 | speed, p2 / 1024 |
| 0x44 | f32 | delay, p3 / 128 |
| 0x48 | f32 | spread, p4 / 128 |
| 0x4c | f32 | cross, p5 / 128 |
| 0x50 | f32 | output gain, always 1 |
| 0x54 | i32 | longest delay any voice reaches, plus 8 |
| 0x58 | dyn array | the voices, 0x2c bytes each, `2 * order` of them |
| 0x60 / 0x68 | method ptr | the renderer: `FUN_006b47f0`, or `FUN_006b4ce0` when CPU info byte +0x16 is set |

A voice (11 fields of 4 bytes):

| field | type | meaning |
|---|---|---|
| [0] | f32 | LFO depth in samples |
| [1], [2] | f32 | gain into the left and into the right output |
| [3] | f32 | centre delay in samples |
| [4] | i32 | LFO phase, the whole i32 range is one cycle |
| [5] | i32 | the phase a flush puts back |
| [6] | i32 | phase step a sample |
| [7] | i32 | byte offset of the channel it reads: 0 left, 4 right |
| [8], [9], [10] | f32 | `T / 2^31`, `T`, `T / 2` with `T = sqrt(4 * depth)` |

## 1. State layout (event 213, 32 bytes, no version)

`SaveRestoreState` (`FUN_006b6b20`): `Write(this+0x120, 0x20)`. Restore
reads the same 32 bytes and runs the GetValue/UpdateValue loop over all
eight parameters. There is no version word. Every corpus block and every
preset is 32 bytes.

The wheel `Tag` is the parameter index; the order wheel has no tag and is
parameter 0.

| byte | type | idx | parameter (DFM object, hint) | range | default | corpus | confidence |
|---|---|---|---|---|---|---|---|
| 0..3 | i32 | 0 | Order (`ChOrderWheel`) | 1..9 | 4 | 4 x36, 2 x9, 7 x4, 1 x3, 9 x2 | loader, dfm, corpus, preset |
| 4..7 | i32 | 1 | Depth (`ChDepthWheel`) | 0..1024 | 512 | 512 x36, 86 x9, 84..660 | loader, dfm, corpus, preset |
| 8..11 | i32 | 2 | Speed (`ChSpeedWheel`) | 0..1024 | 768 | 768 x18, 892 x12, 632 x9, 232..892 | loader, dfm, corpus, preset |
| 12..15 | i32 | 3 | Delay (`ChDelayWheel`) | 0..128 | 8 | 8 x38, 0 x14, 128 x2 | loader, dfm, corpus, preset |
| 16..19 | i32 | 4 | Spread (`ChSpreadWheel`) | 0..128 | 128 | 128 x41, 0 x10 | loader, dfm, corpus, preset |
| 20..23 | i32 | 5 | Cross (`ChCrossWheel`) | -128..128 | -32 | -32 x35, 0 x9, 128 x2, -128 x2 | loader, dfm, corpus, preset |
| 24..27 | i32 | 6 | Dry level (`ChDryWheel`) | -128..128 | 96 | 128 x32, 96 x13, -73..128 | loader, dfm, corpus, preset |
| 28..31 | i32 | 7 | Wet level (`ChWetWheel`) | -128..128 | 96 | 96 x13, 48 x9, 70 x9, 36 x9, 128 x7 | loader, dfm, corpus, preset |

The DFM `DefaultValue` of the last three wheels is 0, which is where a
right-click reset puts them. A new instance starts from the wheels' `Value`,
which is what the table shows and what `Default.fst` holds.

Preset check (`.fst` files name the plugin "Fruity Flangus"). Read through
section 2 at 44.1 kHz. "Swing" is the shortest and the longest delay over
all voices.

| preset | Order | Depth | Speed | Delay | Spread | Cross | Dry | Wet | reading |
|---|---|---|---|---|---|---|---|---|---|
| Default | 4 | 512 | 768 | 8 | 128 | -32 | 96 | 96 | eight voices, swing 8..593 samples, LFOs 0.007..0.68 Hz, a quarter of each side inverted into the other |
| Classic | 4 | 512 | 892 | 8 | 128 | -32 | 88 | 128 | the default, twice as fast |
| Deep chorus | 7 | 512 | 512 | 128 | 128 | 54 | 122 | 90 | 14 voices around 100 ms |
| Fast flanger | 6 | 231 | 848 | 0 | 0 | -66 | 0 | 112 | wet only, no spread: every voice on one LFO |
| Flanger | 6 | 561 | 854 | 0 | 0 | 38 | 0 | 114 | the same, deeper |
| Flanger 2 | 1 | 136 | 312 | 0 | 0 | 128 | 128 | -128 | one voice a side, 8..110 samples at 0.12 Hz, summed to both sides, wet inverted |
| For drums | 4 | 512 | 512 | 30 | 128 | -56 | -128 | 128 | dry inverted |
| Modulation | 1 | 42 | 944 | 0 | 0 | -22 | 0 | 128 | wet only, shallow and fast: a vibrato with a little width |
| Stereoized | 2 | 86 | 632 | 0 | 0 | 0 | 0 | 120 | four voices, 8..77 samples, 0.78 Hz, left and right half a cycle apart |
| Surround flanger | 9 | 81 | 170 | 0 | 0 | -66 | 0 | 104 | 18 voices on one slow LFO |
| Thick | 5 | 1024 | 856 | 0 | 128 | 68 | 42 | 128 | full depth: swing 8..2213 samples |
| Vibrate | 7 | 19 | 985 | 0 | 0 | 0 | 128 | 82 | 14 voices, 8..38 samples at 11.4 Hz |

## 2. Parameter semantics (`ProcessParam` @ 0x006b6fb0)

`REC_FromMIDI` rescales with the wheel's Min/Max and 2^-30 (`0x006b72c0`).
On `REC_UpdateValue` the integer is stored and:

```
idx 0..5   FUN_006b6ec0: lock, copy the six into the flanger, rebuild the voices, unlock
           0x38 = p0;  0x3c = p1 / 1024;  0x40 = p2 / 1024       // 0x006b6fa8 = 1/1024
           0x44 = p3 / 128;  0x48 = p4 / 128;  0x4c = p5 / 128   // *PTR_DAT_00706150 = 1/128
           FUN_006b3ef0(flanger, block size 0x15c, sample rate host+8)
idx 6      0x148 = level(p6)        // dry
idx 7      0x150 = level(p7)        // wet
level(v) = sign(v) * (|v| == 128 ? 1 : (11^(|v| / 128) - 1) / 10)   // ln 11 = DAT_0071546c, set at 0x005e1a25; 0.1 = *PTR_DAT_00706398
```

`level` is the gain curve of every stock wheel: 96 is 0.504, 64 is 0.232,
a negative wheel inverts.

### The voices (`FUN_006b3ef0`, listing in `dsp/asm/`)

`gain(x) = 11^x - 1` and `mix(a, b) = a S + b (1 - S)` (the nested
`FUN_006b3eb0`), where `S` is the spread on its curve:

```
order = max(1, p0);  voices = 2 * order
D   = gain(depth) * 109.25                       // FUN_006b3e30(depth, 1092.5): LFO depth in samples at 44.1 kHz, 0..1092.5
DL  = gain(delay) * 441                          // FUN_006b3e30(delay, 4410): 0..4410 samples, 100 ms
S   = (1001^spread - 1) / 1000                   // 0x006b4548 = 1001, 0x006b4550 = 1000
X   = (101^(1 - speed) - 1) * 75000 + 50000      // 0x006b4510 = 101 (as f64), 0x006b4520, 0x006b4528
inc = 2^31 * 500 / X                             // a tenth of this is the step for a 10-sample depth: X / 25 samples a cycle
r   = 44100 / SR                                 // f32

for i in 0..voices:
    left voice  (i <  order): k = i,               reads the left input,  phase0 = 0
    right voice (i >= order): k = voices - 1 - i,  reads the right input, phase0 = 0x80000000
    f    = 1 - k / order                         // 1, 1 - 1/order, .. 1/order
    t    = table[i mod 20];  rnd = mix(t^2, 1)   // 0x00704e78, below
    gL, gR = 1, cross * f                        // swapped for a right voice
    total += |gL| + |gR|
    depth  = D f + 10                            // 0x006b4554 = 10
    rate   = mix(inc * rnd / depth, inc / (D + 10))
    centre = mix(DL f rnd, DL)
    phase0 += round(f S 2^31 / 256) << 8         // wraps: f S = 1 is half a cycle
    step   = round(rate * r)                     // [6]
    depth  = depth / r                           // [0], samples at SR
    centre = depth + max(centre / r, 8)          // [3]; 0x006b4568 = 8
    longest = max(longest, centre + depth)
    T = sqrt(4 depth):  [8] = T / 2^31, [9] = T, [10] = T / 2

norm = (1 - (1001^(1 - 1 / total) - 1) / 1000) * 0x50      // 0x50 is 1
every gL, gR *= norm
0x54 = round(longest) + 8
if FUN_006b3900 resized the ring to (block size + 0x54) frames, or spread != 0x30:
    read position = write position;  FUN_006b4590: every phase = phase0, ring zeroed;  0x30 = spread
```

So a voice's delay swings between `max(centre, 8 samples)` and that plus
twice its depth. The LFO rate goes with one over the depth, which keeps the
pitch bend of every voice about the same: at speed 1 and depth 0 a cycle is
2000 samples (22 Hz), at the defaults the first voice turns once in 5.4 s.
`f` staggers the voices from the full depth down to `1 / order` of it.
Spread is what tells them apart: at 0 all voices share one rate and the two
sides sit half a cycle apart, at 1 each voice has its own rate and centre
from the table and its own starting phase.

The table at `0x00704e78`, 20 floats:

```
0.967 1.039 1.021 0.1013 0.977 1.033 0.997 0.971 1.061 0.947
1.031 0.1009 1.063 0.953 0.991 1.049 0.983 1.051 1.019 1.069
```

Entries 3 and 11 are a tenth of the others, so with spread up voice 3 and
voice 11 crawl (0.007 Hz at the defaults) near the shortest delay.

`norm` falls with the sum of all gains: 0.969 for two voices (order 1, no
cross), 0.823 at a total of 4, 0.527 at the defaults (total 9.25), 0.219
for 18 voices with full cross.

Anything that changes `round(longest)` clears the ring and restarts the
LFOs: the order, the depth, the delay, the block size and the sample rate
do, the speed and the cross do not. A spread change always does. New voices
from a higher order that does not move `longest` start at phase 0 (the
array is zero-filled).

## 3. Signal chain (`Eff_Render` @ 0x006b7510)

```
grow the wet buffer to Length frames
FUN_006b45f0(flanger, Source, Length)              // copy the block into the ring at the write position, wrapping
flanger.render(wet, Length)                        // 0x60, below
Dest = Source * dry          (FUN_005db1d0, ramped) // 0x148 target, 0x14c state
Dest += wet * wet level      (FUN_005db080, ramped) // 0x150 target for both sides, states 0x154, 0x158
```

Renderer (`FUN_006b47f0`): sets the x87 control word to truncate
(`FUN_0059b100`, `0x006f22cc = 0x0f3f`), renders voice 0 into the wet buffer
(`FUN_006b4680`) and adds every other voice (`FUN_006b4730`), restores the
control word, then moves the read position on by the block. One voice, one
sample (x87, extended precision until the store):

```
p = [4];  [4] += [6]                               // i32, wraps
x = |p * [8]| - [10]                               // a triangle, -T/2 .. T/2
d = ([9] - |x|) * x + [3]                          // centre - depth .. centre + depth; flat at both ends, steepest in the middle
n = trunc(d);  w = d - n
s = ring[now - n] + (ring[now - n - 1] - ring[now - n]) * w     // the voice's own channel, linear between two samples
wetL += s * [1];  wetR += s * [2]
```

`now` is the frame being rendered, so `n = 0` would be the input itself;
the floor of 8 samples keeps every voice behind it. A phase of 0 is the
shortest delay. There is no feedback anywhere.

The two ramps are the host's: `FUN_005dff50` moves a gain to its target over
the block in a straight line, by no more than `host+0x34` a sample, and
snaps a state under `DAT_00715434` (2^-24, set at `0x005e19b5`) to 0. A gain
that is not moving takes the fast paths: copy for 1, zero for 0.

Dispatcher (`FUN_006b6680`): 2 Flush puts the three ramp states on their
targets and calls `FUN_006b4590` (phases back to `phase0`, ring zeroed);
3 keeps the block size and rebuilds the voices; 4 takes the sample rate into
the host object and rebuilds.

The plugin adds no latency: the dry path is a gain.

## 4. What stays unknown

- Which renderer runs. `FUN_006b4ce0` is a Pascal twin of the x87 routine
  that calls `Round` instead of truncating, picked when byte +0x16 of the
  CPU info record (`0x00713d30`) is set. No code in the DLL writes that byte
  by address, so the x87 routine is taken as the live one. The twin would
  interpolate with a weight from -0.5 to 0.5.
- `GetName` formats parameter 0 as an integer and the others as
  `value / Max` through `FUN_005e0490`; the format string was not read.
- The ramp cap `host+0x34` is the host's number. The other maps have it as
  `0.001 * 44100 / SR`.

## Implementation notes (crates/plugins/src/flangus.rs)

- The x87 renderer is the one built. Its sums are in f64 here where FL
  keeps 80 bits until the store.
- One ring a side, sized once for the longest delay the wheels can ask for
  at the rate (7245 samples at 44.1 kHz), where FL sizes its ring to the
  block and the longest delay of the moment. The clear on a change of
  `round(longest)` or of the spread is kept.
- A state load puts the dry and the wet gain on their targets with no ramp.
