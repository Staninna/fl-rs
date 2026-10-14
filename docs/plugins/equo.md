# EQUO: state, parameters, signal chain

Recovered from `Plugins/Fruity/Effects/EQUO/EQUO_x64.dll` (FL Studio 2026,
Delphi native plugin, image base 0x400000). Ghidra project
`ghidra/proj/equo`, decompiles under `ghidra/out/equo/` (`r2/` the slots
with callees, `r3/` to `r5/` the curve, filter, mixing and resampler
helpers). Corpus: 10 states in 7 `.flp` (the render test among them), 7
shipped presets under `Patches/Plugin presets/Effects/EQUO` and the three
copies of the default in the plugin database.

The DSP is inside `EQUO_x64.dll`: its own Delphi units `DSP_FiltersEx`,
`DSP_Mixing`, `DSP_Oversample` and `DSP_Smoothing`. Nothing sits beside it
in the plugin's directory, and the only thing it takes from `Shared/` is
`dsp_ippv2_x64.dll`, loaded by name for `ippsCopy_64s`, `ippsZero_64s`,
`ippsAdd_32f` and `ippsMulC_32f_I`: copies, clears and plain sums. No
FLEngine service is called on the audio path. EQUO is no FFT filter, for
all that `DSP_FFTFilter` is linked in: it is 31 band passes.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `inferred`, `asm`.

## How the plugin is wired

`CreatePlugInstance` @ `0x006cca60` -> constructor `FUN_006cd420`,
`TTestPlug` VMT `0x006cc8e0` (instance size 0x35b8, parent
`TDelphiFruityPlug`).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006ccc80 |
| 2 | Idle_Public | 0x006c8b90 |
| 3 | SaveRestoreState | 0x006cd0a0 |
| 4 | GetName | 0x006cec50 |
| 5 | ProcessEvent | 0x006c7f60 |
| 6 | ProcessParam | 0x006ce840 |
| 7 | Eff_Render | 0x006cf0a0 |
| 14 | NewTick | 0x006cf020 |

Plugin object:

| offset | type | meaning |
|---|---|---|
| 0xcc | ptr | `TAudioRenderer` (VMT 0x005def78): +8 rate, +0x28 `2 pi / rate`, +0x34 `44100 / rate * 0.001` |
| 0xd4 / 0xdc | ptr, i32 | four stereo work buffers, each `dc` frames |
| 0x120 | i32[5] | the knobs, saved |
| 0x134 + 0x140 b | band `b`, 31 of them | below |
| 0x27f4 | f32[9][31][3] | the banks, saved: banks 1 to 8 and the master curve |
| 0x3508 | i32 | oversampling, saved: 0 off, 1 on (2^n stages) |
| 0x350c | i32 | bandwidth wheel, saved, 80..500 |
| 0x3510 | i32 | send insert, saved; -1 none |
| 0x3514, 0x3518 | i32 | target and bank the editor shows, saved |
| 0x351c, 0x351d, 0x351e | u8 | send only, add mode, master button, saved |
| 0x3520 | i32 | bank the editor edits: 8 with the master button, else 0x3518 |
| 0x3524, 0x3528 | i32 | host rate; oversampling factor |
| 0x352c, 0x3530 | i32 | silence tail `round(rate / 5)`; frames of it left |
| 0x3534, 0x3538, 0x353c | f32 | shift in bands, mix's pull, morph in banks |
| 0x3540 | smoother | the per-tick smoother all targets share |
| 0x3574 | f32 | band level scale |
| 0x3578, 0x3580, 0x3588 | ptr | resamplers: up, down main, down send |
| 0x359c | f32 | samples per tick (Dispatcher 20), 196.875 at first |
| 0x35a0, 0x35a4 | f32 | output gain: target, ramp state |
| 0x35a8 | u8 | gains jump to their targets in the next block |
| 0x35ac | u8 | analysis on (editor open and not rendering) |

Band, at `0x134 + 0x140 b`:

| offset | type | meaning |
|---|---|---|
| +0x00 | f32 | centre in Hz |
| +0x04 | filter | type (2), centre, width; coefficients b0, b1, b2, a1, a2 as f64 at +0x54..+0x7c |
| +0x14 | f64[8] | filter state: x1, x2, y1, y2 of each side |
| +0x94 + 0x38 t | target `t` | +0 value (saved), +4 mode (saved), +8 goal, +0xc smoothed, +0x10/+0x14 gain ramp left/right, +0x18 smoother state |
| +0x13c | u8 | runs at twice the rate |
| +0x13d | u8 | runs at all |
| +0x13e | u8 | at or over half the rate |

## 1. State layout (event 213)

`SaveRestoreState` (`0x006cd0a0`) writes, in order:

| bytes | type | what | confidence |
|---|---|---|---|
| 0..3 | i32 | version: 4 (3 in the shipped presets) | loader, corpus, preset |
| 4..747 | (f32, i32)[31][3] | each band's volume, pan and send: value and mode | loader, corpus |
| 748..4095 | f32[9][31][3] | banks 1 to 8 and the master curve, `[bank][band][target]` | loader, corpus |
| 4096..4115 | i32[5] | knobs 0 to 4 | loader, dfm, corpus |
| 4116..4135 | i32[5] | oversampling, bandwidth, send insert, shown target, shown bank | loader, dfm, corpus |
| 4136..4138 | u8[3] | send only, add mode, master | loader, corpus |
| 4139..4142 | i32 | version 4 only: bit 0 is an editor switch | loader, corpus |

Restore reads nothing from a state older than version 3, and reads a
version above 4 as 4. A value runs from 0 to 2 and 1 is flat; a mode is 0
on, 1 off, 2 locked (the editor's "On / off / locked"). Every corpus state
has every mode at 0, oversampling off, bandwidth 150, no send, and the
three switches off.

Parameters (wheel `Tag`, DFM ranges; `ProcessParam` reaches 0x34a = 842):

| idx | wheel | range | default |
|---|---|---|---|
| 0 | Band shift (`BandShiftWheel`) | -12000..12000 | 0 |
| 1 | Band mix (`BandMixWheel`) | -128..128 | 128 |
| 2 | Morph between banks (`MorphWheel`) | 0..7000 | 0 |
| 3 | Motion smoothness (`SmoothWheel`) | 0..1024 | 170 |
| 4 | Output volume (`VolWheel`) | 0..256 | 128 |
| 5..841 | a bank's band (`FUN_006cca00`) | value in 2^29ths | 2^29 |

Parameter `5 + 93 bank + 31 target + band` is that value. Setting one
changes the bank, and the band's kept value too when the bank is the one
the editor edits, but only for a target whose mode is 0. The bandwidth
wheel (`BWWheel`, 80..500, 150), the bank and target selectors, the
oversample, send only and add mode buttons and the send insert are no
parameters.

Corpus (all at bandwidth 150, smooth 170 but where said):

| project | shift, mix, morph, smooth, vol | curve |
|---|---|---|
| Bombs Away ft KARRA - I'm Awake (x2) | defaults | flat to band 20 (2 kHz), then up to +0.9 dB |
| DEDZII - NIGHT TERROR | 0, 128, 4000, 110, 128 | banks 1, 3 and 5 drawn, bank 5 playing |
| DEDZII - NIGHT TERROR | defaults | about -7 dB under 70 Hz |
| DEDZII - NIGHT TERROR | 0, 128, 7000, 170, 128 | bank 8 playing |
| EQUO (render test) | defaults | the preset "Random bank" |
| Breakbeat | 0, 68, 0, 170, 173 | a cut from 50 to 300 Hz, up to +12 dB over 1.3 kHz |
| Chillout, Chillout-Ambient | 0, 37, 0, 170, 128 | the same shape |
| Rock | 0, 71, 0, 170, 148 | the same shape |

The render test `System/Render tests/EQUO.flp` runs a 3x Osc through one
EQUO holding the preset "Random bank" with every knob at its default:
eight random level curves, bank 1 playing. Its curve is +10 dB at band 17
(1016 Hz) and -7 dB at band 24 (5120 Hz); what plays at those two
frequencies is +8.6 and -2.3 dB against a flat EQUO, because neighbouring
bands overlap. The plugin's test checks both.

## 2. The curve (`FUN_006cda50`)

Two scales, `FUN_006ccb70` from a curve value `v` and `FUN_006ccc00` back:

```
gain(v)  = f32(f32(exp(v * ln 3) - 1) * 0.5)      // level and send: 0 at 0, 1 at 1, 4 at 2
pan(v)   = v - 1                                  // -1..1
curve(g) = f32(ln(f32(g / 0.5) + 1) / ln 3)
```

The knobs (`FUN_006ce220`): `shift = p0 * 0.0025` bands, `mix = 1 - p1 /
128`, `morph = p2 * 0.001` banks.

A target's goal, for band `b` and target `t`:

- mode 1 (off): `gain(1)`, flat. mode 2 (locked): `gain(value)` of the
  saved value, with no knob or master.
- mode 0: `x = max(b - shift, 0)`, `i = trunc(x)`, `f = x - i`, and bank
  `k = trunc(morph)`, `m = morph - k`. `FUN_006cd980` reads bank `k` at
  bands `min(i, 30)` and `min(i + 1, 30)`, each through `FUN_006cd940`,
  `gain(v + (1 - v) * mix)`, and goes `f` of the way between them. With
  `m` not 0 it does the same for bank `k + 1` and goes `m` of the way
  between the two on the curve scale: `gain(c0 + (c1 - c0) m)`.
- The master curve (bank 9) then multiplies a level or a send by
  `gain(master)`, or adds `pan(master)` to a pan, clamped to -1..1.

So a mix of 128 plays the curve, 0 a flat line, -128 the curve mirrored
about 1.

## 3. The bands (`FUN_006ce050`)

```
width  = f32(bw / 100)
octaves = width / 3                         // 0.5 at the default 150
scale  = f32(exp(f32(ln 10 * 0.05) * -3)) / width   // 0x3574: 10^(-3/20) / width
centre(b) = f32(exp(b / 3 * ln 2) * 20)     // 20 Hz .. 20480 Hz
```

`rate` here is the host's rate times the oversampling factor. A band with
`centre >= f32(rate / 2)` is off. One with `centre > f32(factor * 22100 /
2)` runs at twice the rate and is designed at half its centre. The design
is `FUN_005dc090` -> `FUN_005dbac0` type 2, the cookbook band pass that is
unity at its centre:

```
w = centre * (2 pi / rate);  (sin, cos) = SinCos(w)
alpha = sin * sinh(ln 2 / 2 * octaves * w / sin)
d = 1 / (alpha + 1)
b0 = d alpha, b1 = 0, b2 = -d alpha, a1 = -2 d cos, a2 = d (1 - alpha)
```

`FUN_005dc5e0` runs it in f64 on f32 frames, both sides with one set of
coefficients: `y = x b0 + ((x2 b2 - y2 a2) - y1 a1)`. A band at twice the
rate holds each frame for two (`FUN_005e13a0`) and averages each two out,
`f32(f32(a + b) * 0.5)` (`FUN_005e1450`). There is no denormal flush on
the bands.

A band runs (`FUN_006cdf70`) when it is under half the rate and its volume
or pan is not off, or its send is not off and a send insert is set.

## 4. Render (`Eff_Render` @ 0x006cf0a0)

1. Silence: a block whose every sample is at most 10^-6 (`FUN_005e0850`)
   takes its length off the tail count, any other block sets it to
   `round(rate / 5)`. At minus the block or under, the input is copied to
   the output and the next block's gains jump to their targets. The count
   starts at 0, so the plugin starts asleep.
2. With oversampling, the input goes up through the resampler (below).
3. The main bus is cleared, or in add mode is the input.
4. Each running band, in order: its band pass into a work buffer. Unless
   send only is on or its volume is off, its gains are FL's balance law
   (`FUN_005e1f80`) of its smoothed pan and its smoothed volume times
   `scale`: the side the pan points away from is `level * (1 - |pan|)` in
   f64. In add mode `scale` is taken off each gain. `FUN_005db690` then
   adds the band to the bus along a line from the last block's gains:
   `FUN_005e0d20` makes the step `(target - now) / n`, no steeper than
   field 0x34, and `FUN_005df6c0` sums it in two lanes of even and odd
   frames, eight frames a round. Constant gains go through `FUN_005df4f0`,
   both at 1 through `ippsAdd_32f`. The send works the same way into the
   send buffer, which goes to insert `send + 1` through the host.
5. The bus comes down through the resampler, or is copied; with send only
   the output is the input.
6. The output gain, `(11^(p4/128) - 1) / 10` and 1 exactly at 128, comes
   last along a line no steeper than field 0x34 over the factor
   (`FUN_005db7e0`, `FUN_005e03f0`).

## 5. Smoothing (`NewTick` @ 0x006cf020)

Once a tick every target's smoothed value moves towards its goal through
the one-pole smoother `FUN_005d9850` type 0: `s = goal + (s - goal) *
pole`, in f64. `FUN_006ce480` sets the pole from knob 3 and the samples
per tick (Dispatcher 20, `rate * 60 / (tempo * PPQ)`):

```
x = f32(p3 * (1 / 1024))
time = f32(f32(exp(x * ln 11) - 1) * 40000 / samples_per_tick)    // ticks
pole = exp(ln 0.01 / max(1, time)), or no smoothing with time <= 0
```

## 6. The resampler (`FUN_006ca820`, `FUN_006ca8c0`)

Two paths of three first-order allpasses at the lower rate, `y = before +
(x - y) c`, with the coefficients at 0x00721938: 0.14738, 0.48247, 0.88301
on one path and 0.03915, 0.30265, 0.67462 on the other, the same as
Parametric EQ 2's. Going up, the second path gives the first frame of each
two and the first path the second (`FUN_006ca2f0`); going down, the first
frame goes through the first path, the second through the second, and
the output is their sum times 0.5 (`FUN_006ca4c0`). After every block an
allpass output under 2^-24 becomes 0 (`FUN_005e2030`). In a render with
FL's HQ flag (Dispatcher 1, flag 2), `FUN_006ce760` runs one stage whatever
the state says.

## 7. What fl-rs does

`crates/plugins/src/equo.rs` follows sections 1 to 6. It differs in:

- the send buffer: there is no host call to give it to another insert, so
  a state with a send loses that output (`fidelity()` says so), and send
  only passes the input on. No corpus state has a send.
- the start: a loaded state starts with every smoother at its goal and the
  gains jumping to theirs. FL keeps the smoothers where its constructor
  left them, flat, and moves them on from there, so a project opened and
  played at once in FL glides into its curve over the smoothing time
  (inferred: how many ticks FL runs before a render is not read).
- the ticks: counted inside the plugin from the tempo and PPQ the host
  gives, at the plugin's own block boundaries.
- the HQ render flag, which the host never sets: the state's own
  oversampling setting is played.
- `sin`, `cos`, `exp`, `ln` and `sinh` are Rust's, not the Delphi
  runtime's, so a coefficient can differ in its last bit.
