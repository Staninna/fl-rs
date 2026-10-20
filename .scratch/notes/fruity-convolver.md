# Fruity Convolver: handover for the impulse preparation (2026-10-20)

`crates/plugins/src/convolver.rs` plays every state; what the corpus render
still calls approximate is in the impulse preparation (rebuild thread
`FUN_0087b750`, `docs/plugins/fruity-convolver.md` section 3). Addresses are
in `Fruity Convolver_x64.dll` (image base 0x400000). Ghidra outputs are
gone; rerun `DecompileAddrs.java` on the addresses below.

Built since (2026-10-20, second session): the resampler (FL's interpolator,
`crates/dsp/src/interpolator.rs`, `docs/plugins/fl-interpolator.md`) and the
self-convolution (`self_convolve` in `convolver.rs`, correction 12 of the
plugin doc). Left: the EQ curve (3 corpus slots) and the layers (none).

## What the corpus asks for (survey of the 21 project states)

- Layers drawn: none. Only presets (Blur Pink, Blur White) have them.
- Self-convolution: 2 slots. Astes - Bien Duro (148, with an EQ curve and
  stretch 279) and LollieVox - I'll Be Your Madonna (102, stretch -1000).
- EQ curve that is not level: 3 slots. Bien Duro (the one above) and Square
  a Saw - Lightning and Oxygen (IMP Centered (long)).
- Resampled impulse: 9 slots at the render's 48 kHz (every stretch != 0, and
  every impulse whose rate is not the song's).

So the order of payoff is the resampler (9), then the sample convolver class
for self-convolution and EQ (4), then the layers (0 in the corpus).

## The rebuild thread, confirmed order (`FUN_0087b750`)

```
IR = copy of the editor sample, 2 float channels (FUN_00626aa0(IR, 2, 1, 0, 0))
if normalize: TRMSLookahead (mode 2) ... IR *= 0.05 / peak             // built
FUN_00867c40(layers, IR)                                                 // layers, below
crop to the sample's first region (FUN_00629c20 / FUN_0062c150)          // editor regions; corpus states have none
if 0x414 != 0: FUN_0087b670(this, IR, 0x414)                             // self-convolution, below
FUN_0062b090(IR, float(SR) * 0x418, 9)                                   // resample, below
if FUN_00793a00(eq, &y) (flat):  gain (3^(2y) - 1) / 2                   // built
else: FUN_0087b2b0(this, IR, eq, stretch)                                // EQ curve, below
trim the tail under 2^-24; lead / block as built
```

`PTR_DAT_008f7898` -> `0x8c7d50` is 0.5 (the flat EQ's `/ 2`), and
`PTR_DAT_008f7ba8` -> `0x90653c` is ln 3.

## Resampler (`FUN_0062b090` -> `FUN_0062aad0(sample, ratio, mode, 0)`) -- built

- `FUN_0062b090(s, rate, mode)`: when `rate != s+0x70`, resample by
  `rate / s+0x70` with `mode`, then `s+0x70 = s+0x78 = rate`.
- `FUN_0062aad0`: a resampler object of class `PTR_PTR_0061ac18`
  (constructor `FUN_0061f3d0`); interpolation set by `FUN_0061f6c0(obj,
  table[mode])`, table at `0x008cebe0` (i32): mode 1..9 = 1, 3, 8, 12, 16,
  32, 64, 128, 256 points; mode 10 = 256 after an FFT low-pass of the sample
  when shrinking (the `FUN_00624000` class below). Mode 9 is the 256-point
  one. Then `FUN_0061fe40(obj, sample, loop?, -1...)`, `FUN_0061fc90(obj,
  1.0 / ratio)` (the speed), output length `round(len * ratio)`, rendered by
  `FUN_00620bc0(obj, out, n <= 1024, 0)` in chunks.
- Read since, see `docs/plugins/fl-interpolator.md`. Was not read: the resampler class itself (`0x0061f3d0..0x00620bc0` and what
  `FUN_0061f6c0` builds for 256 points: the sinc table, its window, how a
  fractional position is interpolated in the table, any normalisation).
  This is FL's sample interpolator, the one FPC, Fruity Slicer and Slicex
  stand in for with a Hermite curve, so one rebuild in `dsp` serves four
  plugins.
- The Rust has a windowed sinc (`resample` in `convolver.rs`) in its place.

## The sample convolver class (`FUN_00624000`, class `PTR_DAT_00623ec0`)

Edison's FFT filter/convolver. Fields: `+8` the source sample, `+0x10` the
kernel sample, `+0x18` the output sample, `+0x20` output offset, `+0x24`
kernel start, `+0x28` dry gain, `+0x2c` wet gain, `+0x30` kernel gain,
`+0x38/39/3a` flags, `+0x3c` kernel mode, `+0x40` an FFT object, `+0x50`
stop level (-1 none). `FUN_00624160` resets: offsets 0, flags 1, mode 0,
wet 1, dry 0, kernel gain 1.

- FFT object (`FUN_00622ac0`, `FUN_00622c40(obj, order, type, flag, hint)`):
  IPP. Type 1 is the real FFT in Pack format (`[Re0, Re1, Im1, ..,
  Re(N/2)]`), flag 2 is `IPP_FFT_DIV_INV_BY_N`; forward `FUN_00622e10`,
  inverse `FUN_00622f00`.
- `FUN_006245c0(c, kernel, mode)`: copy the kernel into `+0x10`, then
  `FUN_006241b0`: resample the kernel to `max(source rate, kernel rate)` by
  mode 9; `+0x30 = kernel rate / that`; `FUN_0062c190` finds the kernel's
  first non-silent frame -> `+0x24`; mode 1: `+0x30 = sqrt(+0x30) / max over
  channels of sum(x)`, else `+0x30 = sqrt(+0x30) / sqrt(max over channels of
  sum(x^2))` (unit energy); `+0x20 = +0x24 + kernel len - 1`.
- `FUN_00624910(c, spectrum, resample?)`: the EQ's kernel from a magnitude
  sample (`n/2 + 1` frames, channel 0 the magnitude, channel 1 a phase that
  the EQ leaves 0): Pack spectrum `pf[0] = m0, pf[2k-1] = m cos(ph), pf[2k] =
  m sin(ph)` (`FUN_00607ca0` is SinCos), the top bin real; inverse FFT;
  rotate by `N/2` (zero phase made causal, centred); window
  `FUN_00636ca0(buf, N + 2, 2, 0.25)` (type 2, parameter 0.25, not read)
  and multiply; store as a mono kernel at the source's rate; `+0x20 = (len
  - 1) >> 1`; flags 0.
- `FUN_00624f90(c, &from, &to)`: overlap-save over the source. FFT size =
  next power of two of `kernel len + 0x8000`, hop `N - kernel len + 1`;
  kernel spectra once; per hop: read the source from `from - +0x20`,
  zero what is before `from` / after `to` when flags 0x39 / 0x3a, forward,
  multiply (`PTR_DAT_008f6f60`, Pack multiply), inverse, write to `+0x18`.
  `FUN_00624db0` first resamples the source to the larger rate.
- `FUN_00625690(c, from, to)`: mix back into the source: `wet = +0x2c *
  +0x30` (mode 2 instead matches RMS), output scaled by it, source scaled by
  `+0x28` when not 0 (`PTR_DAT_008f69e8`), then added (`FUN_005c7090`).
  The result keeps the source's length.

### Self-convolution (`FUN_0087b670(this, IR, amount)`) -- built

```
c = new; c+8 = IR; FUN_006245c0(c, IR, 0)        // the IR is its own kernel, unit energy
c+0x2c = amount * 0.5; c+0x28 = 1.0 - amount * 0.5
FUN_00624f90(c, 0, len - 1); FUN_00625690(c, 0, len - 1)
```

So `IR' = (1 - a/2) IR + (a/2) (IR * IR) / ||IR||`, cut to the IR's length.
Checked: `FUN_0062c190` trims the kernel's exact-zero ends and `+0x24` is
the frames trimmed at the start; the overlap-save starting `+0x20` early puts
the output on `(h * x)[t]` of the untrimmed kernel.

### EQ curve (`FUN_0087b2b0(this, IR, eq, stretch)`)

```
if mix (0x41c) > 0:
    order = clamp(log2(round(IR rate * 2)), 4, 20); N = 1 << order (FUN_00622aa0 is log2)
    spectrum sample: 2 channels, N + 1 frames (FUN_00626010)
    scale = N * this+0x4f4 * stretch * 2;  step = IR rate / scale
    for k in 0..=N: x = FUN_0087bde0(this, float(k * step))      // FUN_00637aa0(this+0x4a8, f * this+0x4f4): frequency to the editor's x
        y = eq value at x (FUN_00791500(eq, ctx, x - previous x): the TMEnvelope runner, 290 lines, not read)
        m = float(y * 2) == 1 ? 1 : float((Exp(float(y * 2) * ln 3) - 1) * 0.5)
        channel 0 = m (16-bit samples get round(m * 32768)), channel 1 = 0
    c = new; c+8 = IR; FUN_00624910(c, spectrum, 1); c+0x2c = mix; c+0x28 = 1 - mix
    FUN_00624f90 / FUN_00625690 over the IR
```

Unread: `FUN_00637aa0`'s map fields (`this+0x4a8`, flags at `[8]`, log
axis), `this+0x4f4`, the window `FUN_00636ca0` type 2, `FUN_00624620` (the
spectrum's own resampling when the third argument is 1), and
`FUN_00791500`.

## Layers (`FUN_00867c40(layers, IR)`)

`FUN_00867b40(layers, 7) < 1` gates it (likely "any of the first three not
flat"; not read). Per frame `i` of the IR: each of layers 0, 1, 2 advances
its runner by `dt = layer0+0x68 / len` (`FUN_00791500`) and gives `v0, v1,
v2` (context `+0x38`); `FUN_00622470(m, v0 * 2, v1 * 2, v2 * 2, 0)` (the
pan/volume/separation matrix, `parts::stereo_gains`; `0x867f18` is 2.0),
then the frame goes through the 2x2 matrix (`PTR_DAT_008f6ef8`, one
frame). With no layer list the matrix is `FUN_006222f0(m, 2.0, ...)`. So
layer 0 is pan, 1 volume, 2 separation, each centred at 0.5. Whether
`MEnvelope`'s `Track::advance` is the same runner as `FUN_00791500` is the
first thing to check.

## Next steps

1. The EQ curve: read `FUN_00636ca0` (window type 2, 0.25), `FUN_00637aa0`
   (frequency to the editor's x), `FUN_00624620` (the spectrum's
   resampling) and `FUN_00791500` (the envelope runner), then build
   `FUN_00624910` (kernel from a magnitude spectrum) and run it through the
   same convolution `self_convolve` uses (the kernel is mono and centred:
   `+0x20 = (len - 1) >> 1`).
2. Layers last: no corpus state draws one.
