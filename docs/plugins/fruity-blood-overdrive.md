# Fruity Blood Overdrive: state, parameters, signal chain

Recovered from `Fruity Blood Overdrive_x64.dll` (FL Studio 2026, Delphi
native plugin, image base 0x400000). Ghidra project `ghidra/proj/bloodod`,
decompiles under `ghidra/out/bloodod/vmt/` (the slots with callees). Corpus:
26 uses in 11 `.flp`, 3 shipped presets plus the three copies of the default
in the plugin database. Two Ghidra runs (import, the slots); the arguments
the decompiler dropped from the three table lookups were read from the
listing with `objdump`. No engine DLL and no FLEngine service: the render
loop is 90 lines of scalar SSE inside `Eff_Render`, and the only table is
the 65536-entry sine the constructor fills.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `inferred`, `asm`.

## How the plugin is wired

`CreatePlugInstance` @ `0x006be5c0` -> constructor `FUN_006becd0`,
`TTestPlug` VMT `0x006be480` (instance size 0x401c8: the table is inline).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006be610 |
| 2 | Idle_Public | 0x006bc0a0 |
| 3 | SaveRestoreState | 0x006beaa0 |
| 4 | GetName | 0x006bf4e0 |
| 5 | ProcessEvent | 0x006bb730 |
| 6 | ProcessParam | 0x006bf130 |
| 7 | Eff_Render | 0x006bf610 |

Object fields (all f32 unless said):

| offset | meaning |
|---|---|
| 0x120, 0x124, 0x128, 0x12c, 0x130, 0x134 | the parameters as 0..1: pre band, x100, color, pre amp, post filter, post gain |
| 0x138, 0x13c / 0x140, 0x144 | the two post low-pass states, left / right |
| 0x148 | pre amp cubed |
| 0x14c, 0x150, 0x154 | what the smoothers of G, B and P are fed |
| 0x158, 0x15c, 0x160 | G (amp gain), B (band blend), P (post gain), smoothed |
| 0x164, 0x168 | post filter `a` and `1 - a` |
| 0x16c, 0x170, 0x174 | left colour filter: low, band, high |
| 0x178, 0x17c | F (colour coefficient), smoothed, and what its smoother is fed |
| 0x180, 0x184, 0x188 | right colour filter: low, band, high |
| 0x18c, 0x190, 0x194 | left drive filter: low, band, high |
| 0x198, 0x19c, 0x1a0 | right drive filter: low, band, high |
| 0x1a4 | f32[65536], the shaper table |
| 0x401a4 | i32[6], the parameters, saved |

The editor form keeps a colour scheme number at `+0x728` and its parameter
controls at `+0x734[tag]`; the x100 button sits in that array with the
wheels (both pass the class test against `0x0064b8d0`).

## 1. State layout (event 213, 36 bytes, version 3)

`SaveRestoreState` (`FUN_006beaa0`): `Write(i32 3); Write(this+0x401a4,
0x18); Write(i32 editor+0x728); Write(i32 0)`. Restore reads the version and
gives up when it is above 3, reads the 24 bytes, reads the scheme word only
for version 3 (into a local it never uses), runs the GetValue/UpdateValue
loop, runs the four smoothers `host+8` times (the sample rate as an int, so
one second: every smoothed value lands on its target), and reads the last
word. A version under 3 is therefore 32 bytes. Every corpus block and every
preset is version 3, 36 bytes.

The control's `Tag` is the parameter index; the pre band wheel has no tag
and is parameter 0.

| byte | type | idx | parameter (DFM object, hint) | range | default | corpus | confidence |
|---|---|---|---|---|---|---|---|
| 0..3 | i32 | | version | | 3 | 3 x26 | loader, corpus, preset |
| 4..7 | i32 | 0 | Pre band (`PreBandWheel`) | 0..10000 | 0 | 0 x19, 4000 x2, up to 10000 | loader, dfm, corpus, preset |
| 8..11 | i32 | 1 | Color (`ColorWheel`) | 0..10000 | 5000 | 5000 x12, 4353 x5, 5016 x4, 1586..10000 | loader, dfm, corpus, preset |
| 12..15 | i32 | 2 | Pre amp (`PreAmpWheel`) | 0..10000 | 0 | 582 x5, 2000 x4, 10000 x2, 582..10000 | loader, dfm, corpus, preset |
| 16..19 | i32 | 3 | x100 (`x100Btn`, a `TQuickBtn`; "Off" / "On") | 0, 1 | 0 | 0 x24, 1 x2 | loader, dfm, corpus |
| 20..23 | i32 | 4 | Post filter (`PostFilterWheel`) | 0..10000 | 0 | 0 x21, 2500 x2, up to 10000 | loader, dfm, corpus, preset |
| 24..27 | i32 | 5 | Post gain (`PostGainWheel`) | 0..10000 | 10000 | 10000 x11, 7906 x5, 2465..10000 | loader, dfm, corpus, preset |
| 28..31 | i32 | | editor colour scheme (`editor+0x728`, an index into the 0x4c-byte records at `*0x007106f8`); not read back | 0..3 seen | 0 | 0 x24, 2 x1, 3 x1 | loader, corpus |
| 32..35 | i32 | | always 0 | | 0 | 0 x26 | loader, corpus |

Preset check (`.fst` files name the plugin "Fruity Blood Overdrive"), read
through section 2:

| preset | Pre band | Color | Pre amp | x100 | Post filter | Post gain | reading |
|---|---|---|---|---|---|---|---|
| Reset | 0 | 5000 | 0 | off | 0 | 10000 | amp gain 1, post gain 1, post filter at its most open: only the shaper is heard |
| Boost | 4000 | 5000 | 2000 | off | 2500 | 10000 | amp gain 1.2, 40 % of the way to the colour band alone |
| Drive | 8500 | 5000 | 10000 | off | 5000 | 5000 | amp gain 26 on mostly the 878 Hz band, post gain 0.25 |

In the corpus the amp is used lightly: 582 is a gain of 1.005, 2000 of 1.2.
The two x100 slots have pre amp 791 (gain 2.2) and 2578 (gain 43.8).

## 2. Parameter semantics (`ProcessParam` @ 0x006bf130)

`REC_FromMIDI` rescales with the control's Min/Max and 2^-30
(`0x006bf2f8`). On `REC_UpdateValue` the integer is stored and the control's
own scale turns it into `v = clamp((value - Min) * control+0x3cc, 0, 1)`,
which is `value / 10000` for the wheels and 0 or 1 for the button
(`0x006bf1d9`). `FUN_006bf450` stores `v` in the field for the index and
calls `FUN_006bfbc0`, which rebuilds everything derived:

```
0x148 = amp^3
0x17c = color^3 * 0.00025                              // 0x0070fe88
0x14c = ((x100 > 0.5 ? 2500 : 25) * amp^3 + 1) * 0.00025     // 0x006bfcd8 = 25, 0x006bfcdc = 2500, 0x006bfcd0 = 0.5
0x150 = (1 - band) * 0.00025
0x154 = gain^2 * 0.00025
0x164 = a = filter * 0.7 + 0.2                         // 0x0070fe8c, 0x0070fe90
0x168 = 1 - a
```

Four of these feed a one-pole smoother that runs once a frame in the render
loop, `s = s * 0.99975 + input` (`0x0070fe80`), so the 0.00025 is the
smoother's own gain and the values it settles on are:

```
G = 1 + 25 amp^3        (1 + 2500 amp^3 with x100)     // amp gain: 1..26, or 1..2501
B = 1 - band                                            // how much of everything but the colour band is kept
P = gain^2                                              // post gain, 0..1
F = color^3                                             // colour filter coefficient
```

The smoother is 99 % there after 18,400 samples at any sample rate (0.42 s
at 44.1 kHz). The post filter coefficient is not smoothed. In f32 the
smoother stops a few parts in 10,000 short of its target: that close, what
a frame would add is under half a unit in the last place.

The constructor starts G, B and P at 1 and F at 0 and then pushes the
wheels' defaults, so a new instance glides to its defaults. A loaded state
does not glide (section 1).

`GetName` (`FUN_006bf350`): "Off"/"On" for the button, `value / 10000` as
`%.4f` for the wheels, less 1 for the post gain.

## 3. Signal chain (`Eff_Render` @ 0x006bf610)

One loop over the frames, the two sides one after the other with their own
filter states and shared coefficients. All f32. `Source` and `Dest` are
interleaved stereo.

```
G = G * 0.99975 + 0x14c;  B = B * 0.99975 + 0x150;  P = P * 0.99975 + 0x154;  F = F * 0.99975 + 0x17c

for each side:
    // colour filter: a Chamberlin state-variable filter, damping 1
    low  += F * band
    high  = in - low - band
    band += F * high
    x     = ((high + low) * B + band) * G        // B = 1: the input (plus F * high); B = 0: the band alone
    // drive filter: the same filter at a fixed 0.25                 *PTR_DAT_00710690 = 0.25
    low2 += 0.25 * band2
    high2 = x - low2 - band2
    band2 += 0.25 * high2
    y     = (shape(low2) + shape(band2) + shape(high2)) * P          // FUN_006bfcf0, three times
    y     = clamp(y, -0.99995, 0.99995)                              // 0x0070fe84
    // post filter: two one-poles
    lp1   = y * (1 - a) + lp1 * a
    lp2   = lp1 * (1 - a) + lp2 * a
    out   = lp2

shape(v) = table[clamp(trunc(v * 32767) + 0x8000, 0, 0xffff)]        // 0x006bfd34 = 32767; cvttsd2si, low 32 bits
table[0x8000 + i] = sin(i * pi / 65536),  table[0x7fff - i] = -sin(i * pi / 65536),  i = 0..0x7fff     // constructor, 0x006bf100, 0x006bf108
```

So `shape(v)` is `sin(v pi / 2)` on a 16-bit grid, held at ±1 beyond ±1.
The two halves of the table are a step apart, so a level under 0 comes out
one step nearer to 0 than the same level above it. A product too large for
64 bits, or a NaN, converts to 0x8000000000000000, whose low half is 0: it
reads the middle of the table and comes out as 0.
The drive filter splits the amplified signal into three bands whose sum is
the signal again, each band is folded through the quarter sine on its own,
and the three are added. Under the knee the sum is `pi / 2` times the
signal; pushed hard, each band saturates to ±1 separately, so the sum can
reach ±3 before the clamp at ±0.99995 cuts it. That clamp is what the post
gain of 1 runs into, and why the Drive preset turns the post gain down.

Nothing in the chain is linear even with the pre amp at 0: the shaper is
always in the path. A constant level `c` comes out as
`clamp(shape(c B G) * P)`, and a level below 1/32767 comes out as nothing.

No coefficient follows the sample rate. At 44.1 kHz the colour filter sits
at `SR / pi * asin(F / 2)`: 878 Hz at the default 5000, 7.35 kHz at 10000.
The drive filter sits at 1759 Hz, and the post filter's two poles run from
11.3 kHz (`a = 0.2`) down to 740 Hz (`a = 0.9`). At 48 kHz all of these are
8.8 % higher.

The Dispatcher (`FUN_006be610`) has no case for Flush, so the filter states
live for as long as the instance. There is no dry path and no latency.

## 4. What stays unknown

- Whether the base class Dispatcher (`FUN_006bb750`) clears anything on
  Flush. Only its first 60 lines were read; none touch the fields above.
- The x100 button's Min, Max and scale (`control+0x3a8`, `+0x3cc`) were not
  read from its class. The reading "0 is off, 1 is on, the value passes
  through as 0.0 or 1.0" is `inferred` from the display strings and from
  the test against 0.5.
- The colour scheme records at `*0x007106f8` were not dumped. They colour
  the editor and change no sound.

## Implementation notes (crates/plugins/src/blood_overdrive.rs)

- A new instance starts with its smoothers settled on the wheels, as a
  loaded state does, and not at G = B = P = 1, F = 0.
- `reset` empties the filters and settles the smoothers. FL's Flush does
  neither.
- Filter states that have gone subnormal are zeroed after each block; FL
  runs with flush-to-zero set.
