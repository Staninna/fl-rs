# Frequency Splitter: state, parameters, signal chain

Recovered from `Frequency Splitter_x64.dll` (FL Studio 2026, Delphi native
plugin, image base 0x400000) and the `engine_x64.dll` it loads from beside
it (MSVC C++ VST, image base 0x180000000). The Delphi shell holds the state,
the parameters and the routing. The engine does all the DSP. The Ghidra
project and decompile paths were not recorded in the handover.

Corpus: 17 project states (Astes - Bien Duro 14, Atthar 2, Sid De La Cruz 1),
9 shipped presets, and 3 copies in the plugin database. Every one is
version 4 and 113 bytes. Every target mixer track is 0, so no shipped state
sends a band anywhere. Four Astes states use linear phase, on inserts 50,
54, 56 and 82.

Confidence: facts below are read in the shell or the engine unless the
text says guessed. Section 6 lists the guesses in one place.

## How the plugin is wired

`CreatePlugInstance` @ `0x0073f140` calls the constructor `FUN_00740280`
with the class reference `PTR_PTR_0073ef58`. The class VMT is at
`0x0073f020`.

| SDK name | address |
|---|---|
| Dispatcher | 0x0073f380 |
| SaveRestoreState | 0x0073fac0 |
| GetName | 0x00741cd0 |
| ProcessEvent | 0x00740de0 |
| ProcessParam | 0x00740e30 |
| Eff_Render | 0x007423b0 |

PlugInfo is at `0x007a2204` and gives 14 parameters. `FUN_00741e10` loads
`engine_x64.dll`.

The engine's constructor `FUN_1801e7ab0` builds an object of 0x11f0 bytes.
It has 26 parameters, set through `setParameter` (`FUN_1801e6d00`).
`processReplacing` is `FUN_1801e56d0` and `setSampleRate` is
`FUN_1801e55c0`.

The engine takes 2 inputs and writes 8 outputs. Band `i` (low, mid, high)
goes to outputs `2i` and `2i + 1` at its send level. Outputs 6 and 7 are the
main output, the three bands summed at their main levels. In `Eff_Render`
the shell plays outputs 6 and 7 as the insert's output. It adds each band
to mixer track `targets[i]` through the PlugHost vtable at +0x128, when that
target is above 0.

## 1. State layout (versions 1 to 4)

`SaveRestoreState` reads the version as an i32 and goes on only when it is
below 5. All values are little-endian.

| bytes | type | versions | meaning |
|---|---|---|---|
| 0..3 | i32 | all | version |
| 4..59 | i32[14] | all | parameters 0..13, kept at shell+0x1dc |
| 60..71 | i32[3] | all | target mixer track of the low, mid and high band, at shell+0x214; 0 is none |
| 72..75 | i32 | all | monitor |
| 76..79 | i32 | all | read and discarded |
| 80..83 | i32 | all | window toggle bits |
| 84..87 | i32 | all | mute bits, 1 is muted |
| 88 | u8 | 2 on | flag byte |
| 89..92 | i32 | 3 on | not identified |
| 93..112 | i32[5] | 4 | analyser settings |

Lengths: version 1 is 88 bytes, version 2 is 89, version 3 is 93, version 4
is 113.

Mute bits 0..5 are the low, mid and high send, then the low, mid and high
main output.

The parameters, with the ranges and defaults `frequency_splitter.rs`
carries:

| idx | parameter | range | default |
|---|---|---|---|
| 0 | Low/Mid Freq | 0..65536 | 19800 (200 Hz) |
| 1 | Mid/High Freq | 0..65536 | 48065 (3 kHz) |
| 2 | Low/Mid Slope | 0..7 | 3 (24 dB) |
| 3 | Mid/High Slope | 0..7 | 3 (24 dB) |
| 4 | Linear phase | 0..1 | 0 |
| 5 | Number of bands, 2 or 3 | 0..1 | 1 (3 bands) |
| 6 | Linear phase quality | 0..5 | 2 |
| 7 | Linear phase speed | 0..65536 | 45820 (50 ms) |
| 8, 9, 10 | Low, Mid, High send level | -1800..1800 | 0 |
| 11, 12, 13 | Low, Mid, High main level | -1800..1800 | 0 |

## 2. Parameter semantics (`ProcessParam` @ 0x00740e30)

What the shell hands the engine, with `v` the stored integer and `p` the
float the engine receives:

```
0, 1   freq      engine 0 = share of min(v0, v1), engine 1 = share of max(v0, v1); share = v / 65536
2, 3   slope     p = v / 7.0; the engine counts eighths of p into [6, 12, 18, 24, 36, 48, 72, 96] dB an octave
4      linear    p = v as a float; mode = int(p * 1.998), 1 is linear phase
5      bands     p = v as a float; 2 bands when p < 0.5, else 3
6      quality   p = v / 5.0; quality = int(p * 5.994)
7      speed     p = v / 65536
8..13  levels    p = (v + 1800) / 3600
```

So the lower wheel is always the first crossover, whichever of the two it
is.

The engine turns these into:

```
frequency  = 2^(p * 9.058894) * 30              // 30 Hz to 16 kHz
level      = 2p                     for p < 0.5
           = 1 + (p - 0.5) * 13.886564          // 1 at the middle, +18 dB at the top
```

`FUN_00740050` sends the mute bit field as engine parameters 14..19, one per
bit. Engine parameters 20..25 are solos, and the shell never sets them.

The main gains sit at engine+0x1138 onward. Each frame each one glides
toward `level * !mute`. `setSampleRate` sets the glide to
`1 - 0.1^(200 / sr)`, so a gain covers 90 % of the way in 5 ms. It also
sends every parameter again and marks the filters dirty.

## 3. The IIR signal chain

The crossover objects sit at engine+0x130, +0x650 and +0xb70.
`FUN_1801e8190` designs one and `FUN_1801e7f10` runs one.

A crossover is a chain of up to 8 biquads on 4 lanes: low pass left, low
pass right, high pass left, high pass right. `FUN_1801e33b0` designs each
biquad from the RBJ cookbook low and high pass, worked in doubles and kept
as floats. The one pole design uses `tan(pi f / sr)` and the bilinear
transform.

| slope | design | Q per biquad | high pass inverted |
|---|---|---|---|
| 6 dB | one pole low and high pass | | no |
| 12 dB | 1 biquad | 0.5 | yes |
| 18 dB | allpass pair, see below | | |
| 24 dB | 2 biquads | 0.707 (as f32) twice | no |
| 36 dB | 3 biquads | 0.5412, 1.3066, 0.707 (as f32) | yes |
| 48 dB | 4 biquads | 0.5412, 1.3066, then the same two again | no |
| 72 dB | 6 biquads | 0.5176, 0.7071, 1.9319, then the same three again | no |
| 96 dB | 8 biquads | 0.5098, 0.6013, 0.9000, 2.5629, then the same four again | no |

The exact Q values are in the source. From 24 dB up, apart from 36 dB, each
is a Linkwitz-Riley pair: a Butterworth chain run twice. The 36 dB chain is
not, and its two halves do not sum flat (the Rust test allows 0.5 off unity
there, against 0.01 for every other slope).

At 18 dB the crossover runs the allpass pair at engine+0x500 and +0x508 in
place of the biquads. The input is halved and goes through a first order
allpass and a second order allpass. Their sum is the low pass and their
difference (first minus second) is the high pass.

Three bands use all three crossovers:

```
c0 at f1 on the input
c1 at f2 on c0's high pass
c2 at f2 on c0's low pass
low  = c2.lp + c2.hp
mid  = c1.lp
high = c1.hp
```

`c2` puts the low band through the same phase shift that `c1` gives the
rest. With 2 bands only `c0` runs: low is its low pass, high is its high
pass, and mid is silent.

## 4. Linear phase mode

The engine functions are `FUN_1801e4890` (the mode's frame loop and a new
setting), `FUN_1801e4730` (the bands of one filter set), `FUN_1801e4ce0`
(a new design), `FUN_1801e2c90` (one FIR filter from a magnitude curve) and
`FUN_1801e2ad0` (one frame through a filter).

FFT size: `N = 512 << quality`, scaled by the sample rate's octaves above
44.1 kHz (`FUN_1801e6310`). The Rust reads that as doubled per octave,
rounded to the nearest, halved once below 44.1 kHz, and capped at 65536.
At 48 kHz quality 2 is 2048. The latency is `3N / 4`.

The magnitude curve uses quick log2 and exp2 approximations. The low pass is
-6 dB at the crossover. On the stop side it falls at the slope in dB an
octave. On the pass side it is one less the stop curve mirrored, so the low
and high curves at one frequency add to 1. Bin 0 is 1 for the low pass and 0
for the high pass. Past -120 dB the stop side is 0 and the pass side 1.

Each filter is the engine's convolver (`FUN_1801e3040`), the one the
`mydispatcher` contexts of Maximus and Parametric EQ 2 hold two of:
`FUN_1801e2c90` designs it and `FUN_1801e2ad0` runs a frame through it.
`docs/plugins/maximus.md` section 5.8 has both read from the disassembly:
the curve times the spectrum of a click at `N / 2`, transformed back and
scaled by `1 / N`, the `N / 2` taps from `N / 4` under the half sine
`sinf(i pi / (N/2 - 1))`, and blocks of `N / 2` overlap-added with one more
`1 / N` on the way out. IPP's FFTs here scale neither way
(`IPP_FFT_NODIV_BY_ANY`). A click comes out `3 N / 4` frames late.

Bands:

```
late = input delayed 3N/4
low  = low FIR
high = high FIR
mid  = late - low - high
```

With 2 bands only the low FIR runs, high is `late - low`, and mid is silent.

A new setting designs a second filter set. That set first hears the last
`N` frames of input, so its blocks are full. Then it crossfades with the old
set over `speed * sr` frames. The Rust has the speed as `10^(2p) * 0.002`
seconds, 2 ms to 200 ms, which is 50 ms at the default.

## 5. Band routing, and why it is left out

The shell adds each band's stereo output to an absolute mixer track number,
1 to 999. The engine of fl-rs cannot carry that. `Effect::aux_output` and
`Hosted.aux` in `crates/engine/src/mix.rs` give a plugin one stereo second
output. That output goes to one insert, and that insert has to be among the
ones the plugin's own insert sends to. Routing the bands would take two
engine changes:

1. A list of second outputs per plugin.
2. Addressing by absolute insert index, with the target insert reached and
   processed after the source.

No corpus state sets a target, so nothing shipped needs it. The plugin plays
the main output only. It reports `Fidelity::Approximate` when any target is
above 0.

## 6. What `frequency_splitter.rs` builds

The state parser takes versions 1 to 4 and checks the length for the
version. It refuses version 5 and up, a parameter outside its range, and a
target outside 0..999. It reads the mute word at byte 84 and ignores the
monitor, the toggles and the window settings. Every corpus state loads and
plays.

The parameter maps follow section 2 with the engine's float constants as
bit patterns. Send levels and send mutes are kept but do nothing, because
the sends go nowhere. The main levels glide as in the engine. A sample-rate
change sends every parameter again, marks the filters dirty and sets the
glide, as `setSampleRate` does. It also sizes the linear phase buffers for
quality 5 at the new rate.

The IIR mode builds section 3: the cookbook and one pole designs
(`FUN_1801e33b0`), the four-lane biquad chain, the 18 dB allpass pair, the
inversion for 12 and 36 dB, and the three-band split. It redesigns the
filters at the start of a block after a change. A redesign keeps what the
filters hold.

The linear phase mode builds section 4: the quick log2 and exp2, the
magnitude curve, two filter sets of the shared convolver in
`crates/plugins/src/linear_phase.rs` (its radix-2 FFT stands in for IPP's,
so the last bits of the sums may differ), the priming with `N` frames of
history, the crossfade and the `3N / 4` delay line for the middle band. A
redesign starts only once the previous fade has finished.

Exact, up to libm and the FFT's rounding: the state, the parameter maps, the
level curve and glide, the IIR crossovers and linear phase. The Rust reports
`Fidelity::Exact` when no target is set.

Left out: band routing, as section 5 says.

Guessed, not confirmed:

- The main gains start at 0 and glide in. The engine object comes from
  operator new and nothing initialises those fields.
- FTZ and DAZ are not reproduced, and libm `sin`, `cos`, `tan` and `powf`
  may differ from FL's by an ulp.

Unknown: what the monitor word, the flag byte, the version 3 word and the
version 4 analyser words mean beyond their positions. The corpus render runs
with this plugin in place, but no render has been compared with FL's output.
