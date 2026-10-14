# Spreader: state, parameters, signal chain

Recovered from `Spreader_x64.dll` (FL Studio 2026, Delphi shell, image base
0x400000) and `Shared/ildsp_x64.dll` (MSVC C++, image base 0x180000000,
`ildsp_x64.pdb`): a library of Faust-generated processors that plugins bind
by name. Spreader's is `StereoEnhancer_CreateInstance`, class
`ILRDStereoEnhancer`, compiled from `StereoEnhancer.dsp` with
`-single -ftz 0`. The Delphi side owns the state, the GUI and the two
spectrum monitors; the library does all the audio. Ghidra projects
`ghidra/proj/spreader` and `ghidra/proj/ildsp`, decompiles under
`ghidra/out/spreader/vmt/` (the shell's slots with callees) and
`ghidra/out/ildsp/senh2/` (the wrapper methods, `instanceConstants`,
`buildUserInterface`, `instanceClear`, `compute`, with the listings of the
last four under `asm/`). Corpus: 33 uses in 9 `.flp`, no preset folder, four
copies of the default in the plugin databases (one in the 12-byte format of
an older build). Five Ghidra runs: two imports, the shell's slots, the
library's factory, the Faust class. Confidence for the chain is high: every
line of `compute` was read against its listing.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `inferred`, `asm`.

## How the plugin is wired

Shell: `CreatePlugInstance` @ `0x006d8a40` -> constructor `FUN_006d8ab0`,
`TTestPlug` VMT `0x006d88f8` (instance size 0x1c0).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006d8fb0 |
| 2 | Idle_Public | 0x006d2430 |
| 3 | SaveRestoreState | 0x006d94f0 |
| 4 | GetName | 0x006d9eb0 |
| 5 | ProcessEvent | 0x006d1ac0 |
| 6 | ProcessParam | 0x006d9740 |
| 7 | Eff_Render | 0x006da430 |

Shell object: `0x180` the processor (an interface from the library), `0x188`
i32[5] the parameters, `0x19c` / `0x1a4` the two monitors' capture buffers
(`FUN_006d6c30`, fed with the input and the output while the editor is
open), `0x1b0` editor hidden.

`FUN_006d6f50` loads `ildsp` (`FUN_005e0470`, module name at `0x005e04b4`)
and resolves `StereoEnhancer_CreateInstance` (`0x1801abca0`), which returns
a 16-byte `StereoEnhancerImpl` (vtable `0x1803e00c8`) holding the Faust
object (0x4198 bytes, vtable `0x1803deae0`, constructor `FUN_18018a210`):

| Impl slot | address | does |
|---|---|---|
| +0x08 | 0x1801abda0 | set sample rate: Faust `instanceConstants(int(rate))`, then `instanceClear` |
| +0x10 | 0x1801abdd0 -> `FUN_18018a590` | set parameter by index: writes the Faust zone named below |
| +0x18 | 0x1801abde0 -> `FUN_18018a870` | read a zone back (the shell takes its defaults from here) |
| +0x20 | 0x1801abdf0 -> `FUN_18018ab50` | process: deinterleave at most 0x4000 frames, `compute`, interleave |
| +0x28 | 0x1801ab530 | flush: `instanceClear` |

Faust zones (`buildUserInterface` @ `0x1801896c0`, defaults from
`instanceResetUserInterface` @ `0x180189560` and then the constructor):

| index | name | offset | range | default |
|---|---|---|---|---|
| 0 | width | 0x2078 | 0..1 | 0.5 |
| 1 | separation | 0x40e8 | 0..1 | 0.5 |
| 2 | lowbypass | 0x10 | 10..10000 Hz | 10 |
| 3 | monobass | 0x40f4 | checkbox | 0 |
| 4 | enable | 0x24 | checkbox | 1 |

The zone setter (`FUN_180176c60`) stores the float as given; nothing clamps.

## 1. State layout (event 213, 30 bytes, version 0x10003)

`SaveRestoreState` (`FUN_006d94f0`): `Write(i32 0x10003); Write(this+0x188,
0x14); Write(i32 editor+0x748); Write(u8 Post-FX monitor checkbox);
Write(u8 Pre-FX monitor checkbox)`.

Restore reads the first word and takes `max(1, word - 0x10000)` as the
version:

- 4 or more: nothing is read.
- 1, which is any first word under 0x10002: the oldest format, three
  parameters and no version. The five parameters are zeroed, the first word
  is parameter 0, and 8 more bytes are parameters 1 and 2. Mono and Enabled
  stay 0, so such a state loads switched off.
- 2: 20 bytes of parameters, 24 bytes in all.
- 3: the 20 bytes, the editor word (read and dropped), and the two bytes,
  which set the monitor checkboxes (`FUN_006dd330`, `FUN_006dd380`).

Then the GetValue/UpdateValue loop. Every corpus block is version 3, 30
bytes. The plugin databases hold three of those and one 12-byte state
(5000, 1000, 1).

The control's `Tag` is the parameter index; the spread wheel has no tag and
is parameter 0.

| byte | type | idx | parameter (DFM object, hint) | range | default | corpus | confidence |
|---|---|---|---|---|---|---|---|
| 0..3 | i32 | | version, 0x10003 | | | 0x10003 x33 | loader, corpus |
| 4..7 | i32 | 0 | Spread (`SpreadWheel`) | 0..10000 | 5000 | 5000 x10, 3350..8675 | loader, dfm, corpus |
| 8..11 | i32 | 1 | Stereo separation (`SeparationWheel`) | -10000..10000 | 0 | 0 x14, 1000 x2, -1312..8932 | loader, dfm, corpus |
| 12..15 | i32 | 2 | Low-Frequency Bypass (`LFBypassWheel`) | 1..1000000 | 1 | 1 x19, 1000..717250 | loader, corpus, inferred |
| 16..19 | i32 | 3 | Mono (`MonoSwitch`, a `TQuickBtn`) | 0, 1 | 0 | 0 x29, 1 x4 | loader, dfm, corpus |
| 20..23 | i32 | 4 | Enabled (`OnOffChekcBox`) | 0, 1 | 1 | 1 x33 | loader, dfm, corpus |
| 24..27 | i32 | | `editor+0x748`, not read back | | 0 | 0 x33 | loader, corpus |
| 28 | u8 | | Post-FX monitor shown | 0, 1 | 1 | 1 x33 | loader, dfm, corpus |
| 29 | u8 | | Pre-FX monitor shown | 0, 1 | 1 | 1 x33 | loader, dfm, corpus |

The DFM gives the low-frequency wheel `Min = 1000`, `Max = 1000000`, but a
new instance saves 1 and 19 corpus slots hold 1. The constructor computes
that default as `Min + round(norm * (Max - Min))` with `norm = 0` for the
library's 10 Hz, so the wheel's Min is 1 at run time. Tag `inferred`: the
wheel class was not read.

## 2. Parameter semantics (`ProcessParam` @ 0x006d9740)

`REC_FromMIDI` rescales with the control's Min/Max and 2^-30
(`0x006d9900`). On `REC_UpdateValue` the integer is stored and
`FUN_006d9960` hands the processor a float:

```
v = clamp((value - Min) * control+0x3cc, 0, 1)
0 spread       width      = v                              // value / 10000
1 separation   separation = v                              // (value + 10000) / 20000: 0.5 in the middle
2 LF bypass    lowbypass  = max(10, hz(v))                 // FUN_006d99f0, then fmaxf in FUN_18018a590 (0x1803e07b8 = 10)
3 mono         monobass   = float(value)                   // 0 or 1
4 enabled      enable     = float(value)

hz(v) = 1000 v                      for v <= 0.5           // 0..500 Hz over the first half of the wheel
      = 500 + 9500 * (2 v - 1)      above                  // 500 Hz..10 kHz over the second
```

`hz` is `FUN_006d8760(x, a, b, p) = a + (b - a) * exp(ln(x) / p)` with `p =
1` on both halves (`0x006d9a60..0x006d9a90`). The corpus values read as
1 -> 10 Hz (the floor), 106000 -> 106 Hz, 516274 -> 809 Hz, 717250 ->
4.6 kHz.

`GetName` (`FUN_006d9ce0`): spread as `value / 100` percent, separation as
`-value / 10000` through the host's "merged .. original" formatter, the
bypass as `hz(v)` with `%f Hz`, the two switches as off/on.

Inside the library every zone is followed by a one-pole,
`s = s * p + (1 - p) * zone` with `p = exp(-500 / SR)` (2 ms), once a
sample. `instanceClear` zeroes those five smoothed values with everything
else, so after a flush or a sample-rate change the processor fades in from
"switched off, no width" over a few milliseconds.

## 3. Signal chain

Shell `Eff_Render` (`FUN_006da430`): feed the Pre-FX monitor when the editor
is open, `processor.process(Length, Source, Dest)`, feed the Post-FX
monitor. Nothing else.

Constants (`instanceConstants` @ `0x180189420`), `SR` clamped to 1..192000:

```
p     = exp(-500 / SR)                 0x0c      q = 1 - p            0x14
turn  = 3.1415927 / SR                 0x20
N     = int(SR * 0.0063492064)         0x2074    // longest delay: 280 samples at 44.1 kHz, 6.35 ms
fade  = 1 / int(SR * 0.023219954)      0x2088    // 1 / 1024 at 44.1 kHz
c     = 1 / tan(47123.89 / SR)                   // 15 kHz
h0    = 1 - c    0x20b4;   h1 = c / 2    0x20c4;   h2 = 1 / (1 + c)    0x20b8
```

`compute` (`0x180189900`), one frame, all f32. `k = 1.4142135`
(`0x1803e06b0`), `g8 = 0.39810717` (-8 dB, `0x1803e0538`; `0x1803e0608` is
twice that):

```
f   = f * p + q * lowbypass;   g = tan(f * turn);   d = 1 / (1 + g (g + k))
en  = en * p + q * enable

// crossover, per side: two state-variable low-passes in a row (a fourth-order Linkwitz-Riley)
x     = en * in
v     = (x - (s1 (g + k) + s2)) * g * d;      b = s1 + v;   s1 += 2 v
ap    = x - 2.828427 b                        // the second-order allpass the two bands sum to   0x1803e0750
l     = b g + s2;                             s2 += 2 b g
v     = (l - (s3 (g + k) + s4)) * g * d;      b = s3 + v;   s3 += 2 v
low   = b g + s4;                             s4 += 2 b g
high  = ap - low

S = highL - highR;   M = highL + highR        // not halved
lineS[n] = S;        lineM[n] = M             // two rings of 2048

w   = w * p + q * width
dt  = int(w * N)                              // the delay the width asks for
(i, d0, d1) = sdelay control, below
dS  = lineS[n - d0] + (lineS[n - d1] - lineS[n - d0]) * i        // d0, d1 held to 0..N
dM  = lineM[n - d0] + (lineM[n - d1] - lineM[n - d0]) * i

// a first-order split at 15 kHz of each delayed signal, at half level
xS = w dS / 2:   lpS = ((xS + xS1) - h0 lpS) * h2;    hpS = (c (xS - xS1) - h0 hpS) * h2
xM = w dM / 2:   lpM, hpM the same
shelfS = lpS + g8 hpS;   shelfM = lpM + g8 hpM                   // a high shelf: -8 dB above 15 kHz

mb  = mb * p + q * monobass
sp  = sp * p + q * separation
A   = M / 2 + mb (lowL + lowR) / 2 + shelfS
B   = 2 sp (S / 2 + shelfM)
outL = (1 - en) inL + (1 - mb) lowL + A + B
outR = (1 - en) inR + (1 - mb) lowR + A - B
```

The listing computes `lpS` and `hpS` on `-w dS` and negates, and forms `A`
and `B` as

```
t = hpS * 0.7962143 + (apL + apR)
A = ((mb (lowL + lowR) + lpS) + (t + lpS) - (lowL + lowR)) * 0.5
B = (((lowL + lowR) - (2 (lpS - highL) + t)) * 0.5 + (hpM + hpS) * g8 + (lpM + lpS)) * (2 sp)
```

which is the same thing: `lpS` and `g8 hpS` cancel out of `B`.

The delay control is Faust's `de.sdelay`: two taps and a crossfade between
them, so the delay can move without a click. State `j` (fade step), `i`
(fade position, 0 is tap 0), `d0`, `d1`:

```
j  = j1 != 0 ? (0 < i1 < 1 ? j1 : 0)
   : i1 == 0 ? (dt != d0 ? fade : 0)
   : i1 == 1 ? (dt != d1 ? -fade : 0) : 0
i  = max(0, min(1, i1 + j))
d0 = (i1 >= 1 and d1 != dt) ? dt : d0         // the idle tap takes the new delay
d1 = (i1 <= 0 and d0 != dt) ? dt : d1
```

Read as a whole: the band under the bypass frequency passes (summed to mono
by Mono). Above it, the mid gets the side added, late by `width * 6.35 ms`
and scaled by the width, and the side gets the mid the same way. A mono
signal therefore comes out with a side made of its own echo; that is the
spread. Separation scales the side that results: 0 at one end of the wheel
(mono above the bypass too), 1 in the middle, 2 at the other end. The shelf
keeps the added signal from brightening the top octave.

With the width at 0 nothing is added, and with separation in the middle each
side is `low + high = ap`: the input through one second-order allpass at
the bypass frequency. Levels and the stereo image are as they came; only
the phase around that frequency turns.

`enable` crossfades to the untouched input: at 0 the crossover is fed
silence and the output is the input.

The dry path is not delayed, so there is no latency. The wrapper processes
at most 0x4000 frames a call and leaves the rest of a longer block
unwritten.

The smoothers are f32 and stop a hair short of what they follow: with the
spread wheel at 5000 the width settles on 0.4999987, so the delay is 139
samples at 44.1 kHz and not 140; at 2500 it is 69, at 10000 it is 279.

## 4. What stays unknown

- `editor+0x748`, the word after the parameters. It is 0 in every state and
  the loader drops it.
- The low-frequency wheel's run-time Min (section 1). With the DFM's 1000
  instead of 1 a frequency in the first half of the wheel would be up to
  1 Hz lower (106000 would be 105.1 Hz and not 106.0).
- `FUN_1801c5f40` and `FUN_1801c9ef0` are taken as `expf` and `tanf` from
  how Faust writes these constants; their bodies were not read.
- Whether the host flushes the plugin before the first block. If it does
  not, a loaded state still glides from the constructor's defaults.

## Implementation notes (crates/plugins/src/spreader.rs)

- A block of any length is processed whole; the library's wrapper stops at
  0x4000 frames.
- States that have gone subnormal are zeroed after each block; FL runs with
  flush-to-zero set.
