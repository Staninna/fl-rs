# Fruity Chorus: state, parameters, signal chain

Recovered from `Fruity Chorus_x64.dll` (FL Studio 2026, Delphi native
plugin, image base 0x400000). Ghidra project `ghidra/proj/fchorus`,
decompiles under `ghidra/out/fchorus/` (`vmt/` the slots with callees,
`dsp/` the wavetable builder, the crossover design and the note-to-Hz
helper, with the listings of all of them and of `Eff_Render` under
`dsp/asm/`). Corpus: 20 uses in 9 `.flp`, 8 shipped presets, the three
copies of the default in the plugin database, and one mixer preset in a
format the plugin no longer reads. Three Ghidra runs: import, the slots, the
helpers. No engine DLL and no FLEngine service: everything is inside the
plugin object, which is why it is 0x181e8 bytes.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `inferred`, `asm`.

## How the plugin is wired

`CreatePlugInstance` @ `0x006c0420` -> constructor `FUN_006c07a0`,
`TTestPlug` VMT `0x006c0300` (instance size 0x181e8).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006c0470 |
| 2 | Idle_Public | 0x006bded0 |
| 3 | SaveRestoreState | 0x006c06a0 |
| 4 | GetName | 0x006c16c0 |
| 5 | ProcessEvent | 0x006bd560 |
| 6 | ProcessParam | 0x006c0a20 |
| 7 | Eff_Render | 0x006c1dd0 |

Object fields:

| offset | type | meaning |
|---|---|---|
| 0x120 | i32[4][4097] | the four LFO wavetables, 0..32766, each with one guard entry |
| 0x10130, 0x10134, 0x10138 | u32 | LFO phases; bits 12..23 index a table, so 2^24 is one cycle |
| 0x1013c, 0x10140, 0x10144 | i32 | LFO phase steps a sample |
| 0x10148 | i32 | table offset of the right side, from Stereo |
| 0x1014c, 0x10150, 0x10154 | i32 | which table each LFO reads, 0..3 |
| 0x10158 / 0x14158 | f32[4096] | the delay ring, left / right |
| 0x18158 | i32 | write index |
| 0x1815c | i32 | delay in 1/4096 samples |
| 0x18160 | i32 | depth, scaled so that `table * depth >> 9` is 1/4096 samples |
| 0x18164 | bool | wet only |
| 0x18165 | bool | cross type: false the band above the cutoff is processed, true the band below |
| 0x18168, 0x1816c | f32 | crossover low-pass state, left / right |
| 0x18170, 0x18174 | f32 | the previous input, left / right |
| 0x18178, 0x1817c | f32 | crossover coefficients `k` and `(1 - k) / 2` |
| 0x18180 | i32[12] | the parameters, saved |
| 0x181b0 | f32[12] | the parameters as 0..1 |

## 1. State layout (event 213, 52 bytes, version 1)

`SaveRestoreState` (`FUN_006c06a0`): `Write(i32 1); Write(this+0x18180,
0x30)`. Restore reads the version and does nothing when it is 2 or more,
else reads the 48 bytes and runs the GetValue/UpdateValue loop. Every corpus
block and every preset of the plugin's own is version 1, 52 bytes.

`Patches/Mixer presets/Chorus send.fst` holds 48 bytes for this plugin:
twelve floats (0.5, 0.45, 0.333, 0.3, 0.5, 0.7, 0, 0, 0, 0, 0.5, 0.76), the
`0x181b0` array of an older build with no version word. Today's loader
reads the first float as version 0x3f000000 and leaves the plugin alone.

The wheel `Tag` is the parameter index; the delay wheel has no tag and is
parameter 0. `v` below is `value / Max`.

| byte | type | idx | parameter (DFM object, hint) | range | default | corpus | confidence |
|---|---|---|---|---|---|---|---|
| 0..3 | i32 | | version | | 1 | 1 x20 | loader, corpus, preset |
| 4..7 | i32 | 0 | Delay (`DelayWheel`) | 0..1024 | 512 | 512 x10, 0..243 x10 | loader, dfm, corpus, preset |
| 8..11 | i32 | 1 | Depth (`DepthWheel`) | 0..5000 | 2250 | 2250 x9, 102..4336 | loader, dfm, corpus, preset |
| 12..15 | i32 | 2 | Stereo (`StereoWheel`) | 0..1024 | 341 | 341 x10, 0 x4, up to 1024 | loader, dfm, corpus, preset |
| 16..19 | i32 | 3 | LFO 1 Frequency (`LFO1FreqWheel`) | 0..5000 | 1500 | 1500 x10, 0 x4, up to 2832 | loader, dfm, corpus, preset |
| 20..23 | i32 | 4 | LFO 2 Frequency (`LFO2FreqWheel`) | 0..5000 | 2500 | 2500 x10, 0 x5, up to 4375 | loader, dfm, corpus, preset |
| 24..27 | i32 | 5 | LFO 3 Frequency (`LFO3FreqWheel`) | 0..5000 | 3500 | 3500 x12, 0 x5 | loader, dfm, corpus, preset |
| 28..31 | i32 | 6 | LFO 1 Wave (`LFO1WaveWheel`) | 0..1024 | 23 | 0 x10, 23 x9, 827 x1 | loader, dfm, corpus, preset |
| 32..35 | i32 | 7 | LFO 2 Wave (`LFO2WaveWheel`) | 0..1024 | 0 | 0 x15, 427 x2, 428 x2, 792 x1 | loader, dfm, corpus, preset |
| 36..39 | i32 | 8 | LFO 3 Wave (`LFO3WaveWheel`) | 0..1024 | 0 | 0 x17, 1024 x2, 943 x1 | loader, dfm, corpus, preset |
| 40..43 | i32 | 9 | Cross Type (`CrossTypeWheel`) | 0..100 | 0 | 0 x16, 76 x2, 65 x2 | loader, dfm, corpus, preset |
| 44..47 | i32 | 10 | Cross Cutoff (`CrossCutoffWheel`) | 0..5000 | 2500 | 2500 x10, 1719..4740 | loader, dfm, corpus, preset |
| 48..51 | i32 | 11 | Wet only (`WetOnlyWheel`) | 0..1024 | 0 | 0 x16, 96 x2, 1024 x2 | loader, dfm, corpus, preset |

The wheel's `Value` is the default; `Default.fst` has LFO 1 Wave at 0 where
the DFM and the plugin database copies have 23. Both are the sine.

Preset check (`.fst` files name the plugin "Fruity Chorus"), read through
section 2:

| preset | Delay | Depth | Stereo | LFO Hz | Waves | Cross | Wet only | reading |
|---|---|---|---|---|---|---|---|---|
| Default | 15.03 ms | 2.25 ms | 59 | 0.45, 1.25, 2.45 | sin x3 | above 320 Hz | no | three sweeps between 15.0 and 17.3 ms |
| 50 Detune | 1.22 ms | 4.38 ms | 26 | 2.09, 0, 0.12 | sin^3, sin, multi-sin^3 | below 6.3 kHz | yes | LFO 2 stands still; wet only at 491 of 1024 |
| CE Chorus | 3.41 ms | 3.67 ms | 0 | 0.43, 0, 0 | sin x3 | above 102 Hz | no | one sweep and two taps that stand still, the two sides one table step apart |
| Fat Clean | 0.05 ms | 0.34 ms | 112 | 0, 3.83, 0 | sin, sin^3, sin | below 8.6 kHz | no | cross type at 76 of 100 |
| Filtro | 1.22 ms | 0.39 ms | 61 | 0.43, 0.52, 0.59 | sin x3 | above 3.2 kHz | yes | only the top of the spectrum, and only the wet part |
| Light HF | 3.79 ms | 1.53 ms | 84 | 1.45, 0, 0.73 | sin, sin, multi-sin | above 5.8 kHz | no | LFO 3 wave at 949 of 1024 is still the third table |
| Low Enhance | 7.16 ms | 4.34 ms | 73 | 0, 0, 0.12 | sin, sin, multi-sin^3 | below 7.3 kHz | no | LFO 3 wave at 1024 is the fourth table; wet only at 96 is off |
| Poly Chorus | 2.16 ms | 1.90 ms | 28 | 0, 2.44, 0 | sin x3 | above 469 Hz | no | |

## 2. Parameter semantics (`ProcessParam` @ 0x006c0a20)

`REC_FromMIDI` rescales with the wheel's Min/Max and 2^-30 (`0x006c0bf0`).
On `REC_UpdateValue` the integer is stored, `v = clamp((value - Min) *
wheel+0x3cc, 0, 1)` is kept as a float at `0x181b0[idx]`, and `FUN_006c1360`
(listing in `dsp/asm/`) turns it into what the render loop reads. `SR` is
the host's sample rate as an int (`host+8`), `trunc` is `cvttsd2si`:

```
 0 delay      0x1815c = trunc((29.95 v + 0.05) * SR * 4.096)       // 0.05..30 ms in 1/4096 samples; 0x006c15e0, 0x006c15e8, 0x006c15f0
 1 depth      0x18160 = trunc(v * 5 * SR * 4.096 / 64)             // 0..5 ms; the /64 is undone by table * depth >> 9
 2 stereo     0x10148 = trunc(v * 4096 * 0.5) - 1                  // the right side's LFOs run 0..180 degrees ahead, less one table step
 3..5 LFO Hz  0x1013c[k] = trunc(v^2 * 83886080 / SR)              // 83886080 = 5 * 2^24: 0..5 Hz, square law
 6..8 wave    0x1014c[k] = trunc(v * 3)                            // 0 sin, 1 sin^3, 2 multi-sin, 3 multi-sin^3; only v = 1 is 3
 9 cross type 0x18165 = v > 0.25                                   // 0 "Process HF", 1 "Process LF"
10 cutoff     FUN_006c1ca0, below
11 wet only   0x18164 = v > 0.25                                   // "no" / "yes"
```

`GetName` (`FUN_006c0dc0`) prints the same numbers: `29.95 v + 0.05` and
`5 v` as `%.5f` ms, `trunc(180 v)` degrees, `5 v^2` Hz, the wave and switch
names from the tables at `0x007110d8`, `0x007110f8`, `0x00711108`.

Crossover (`FUN_006c1ca0`):

```
f = 440 * 2^((127 v - 69) / 12)            // FUN_006c1c10: MIDI note 0..127 to Hz, 8.2 Hz .. 12.5 kHz
f = min(f, SR / 4.1)
w = f / SR * 6.283184                      // 0x006c1da8
k  = (1 - sin w) / cos w                   // 0x18178
k2 = (1 - k) * 0.5                         // 0x1817c
```

A first-order low-pass by the bilinear transform: `y = k2 (x + x1) + k y`.

Wavetables (`FUN_006c18e0`, built once in the constructor): for `i` in
0..4095, `x = i / 2048` (f32), entry = `trunc((w + 1) * 16383)`:

```
table 0 "sin"          w = 4 x (1 - x)             for x < 1,   -4 (x - 1) (2 - x)  from 1 on      // two parabolas
table 1 "sin^3"        w^3 of table 0
table 2 "multi-sin"    w = (sin(3.1415 x) + (sin(9.4245 x) + sin(6.283 x)) / 3) * 3 / (2 sqrt 2 + 1)
table 3 "multi-sin^3"  w^3 of table 2
```

Entry 4096 of each table is a copy of entry 0 of table 0 (16383); the render
loop masks its index to 12 bits and never reads it.

Nothing is recomputed when the host changes the sample rate (Dispatcher 4
only stores it): the delay, the depth, the LFO steps and the crossover keep
the values of the rate they were set at until a wheel moves or a state
loads.

## 3. Signal chain (`Eff_Render` @ 0x006c1dd0)

Four copies of one loop, for the two switches. Per frame, with `in` the
input times 0.7 (`0x006c3408`), computed in f64 and rounded to f32:

```
for LFO k = 0..2:
    i      = (phase[k] >> 12) & 0xfff
    mL[k]  = (u32)(table[wave[k]][i]                    * depth) >> 9     // 0 .. depth * 64, in 1/4096 samples
    mR[k]  = (u32)(table[wave[k]][(i + stereo) & 0xfff] * depth) >> 9
    phase[k] += step[k]

lp    = k2 * (in + in1) + k * lp;  in1 = in             // per side
ring[write] = cross type ? lp : in - lp                 // the band that gets the chorus

base  = write * 4096 - delay
for each side and each k:
    p    = base - m[k]                                  // u32, wraps
    n    = (p >> 12) & 0xfff;  w = p & 0xfff
    tap  = ring[n] + (ring[(n + 1) & 0xfff] - ring[n]) * w / 4096        // 0x006c3410 = 1/4096; f64, rounded to f32
write = (write + 1) & 0xfff
wet   = (tap[0] + tap[1] + tap[2]) * 0.6                // 0x006c3418
out   = wet only ? wet : wet + in
```

After the block a crossover state under 1e-11 in size is set to 0
(`0x006c3420`).

So each side has three taps on one delay line of its own, each tap swept by
one of the three LFOs between `delay` and `delay + depth`, and the right
side reads the same LFOs up to half a cycle ahead. Only one band of the
input goes into the line. The whole input, at 0.7, is the dry part. With
every LFO standing still (or the depth at 0 and the waves alike) the three
taps coincide and the wet part is the band at 1.8, late by the delay.

There is no feedback, and no wet or dry level: full scale in comes out at
up to `0.7 + 3 * 0.6 * 0.7 = 1.96`.

The ring is 4096 samples at every rate. The longest delay is 35 ms, which is
1544 samples at 44.1 kHz and 3360 at 96 kHz; at 192 kHz it is 6720, and the
position wraps.

Flush (`FUN_006c17f0`, from the constructor and from Dispatcher 11 with a
non-zero value, which is the host switching the plugin on): the crossover
states and previous inputs to 0, both rings zeroed, the write index to 0,
and each LFO phase to `Random(0x7fff)`. That is at most 8 table steps into
the cycle of 4096, so the LFOs start together to within 0.2 % of a cycle.
`Random` is the Delphi RTL's (`RandSeed * 0x08088405 + 1`, `0x00407ad0`).

The plugin adds no latency: the dry part is not delayed.

## 4. What stays unknown

- Whether the DLL seeds `RandSeed` before the first flush. From 0 the first
  three phases are 0, 1028 and 28213. Any seed gives phases under 32767.
- Dispatcher 2 (Flush) is not handled in the plugin's own Dispatcher; the
  base class one (`FUN_006bd580`) was not read to the end.
- The wheels' scale (`+0x3cc`) is taken as `1 / Max`; all twelve have Min 0.

## Implementation notes (crates/plugins/src/chorus.rs)

- The LFO start phases are the RTL generator's from seed 0, with a seed for
  every instance: 0, 1028, 28213, then 6637, 8942, 22008 after a flush.
- A sample-rate change works the delay, the depth, the LFO steps and the
  crossover out again, where FL keeps the old ones.
- The render test `System/Render tests/Fruity Chorus.flp` holds its chorus
  inside a Fruity Wrapper (a 412-byte wrapper state), so it does not
  exercise this plugin.
