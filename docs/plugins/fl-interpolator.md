# FL's sample interpolator

The class FL's plugins link in to read a sample at any place and speed. It
is in every plugin DLL built on IL's shared Delphi units; the addresses
here are in `Fruity Convolver_x64.dll` (image base 0x400000), where the
sample class resamples itself with it. `crates/dsp/src/interpolator.rs`
rebuilds the sinc kind.

## The object (class `PTR_PTR_0061ac18`, constructor `FUN_0061f3d0`)

| Offset | Type | What |
|---|---|---|
| +0x08 | f64 | the place, in frames of the source |
| +0x10 | f64 | the speed: frames of the source per frame read |
| +0x40 | ptr | the source's data |
| +0x48 | i32 | log2 of a frame's bytes (`PTR_DAT_008f76d8[format] + channels - 1`) |
| +0x50, +0x58 | i64 | the first frame and one past the last |
| +0x60, +0x68 | i64 | the loop's first frame and one past its last (the region's when not looping) |
| +0x70 | i32 | how many times the loop came round |
| +0x74 | u8 | looping |
| +0x75 | u8 | a speed of exactly 1 from a whole place: frames are copied |
| +0x76 | u8 | taps past the ends read 0; clear, they read the end frame |
| +0x77 | u8 | a read past the end fills 0 and goes on; clear, it stops short |
| +0xa0, +0xa1 | u8 | channels (1, 2) and format (0 16-bit, 1 float) |
| +0xc0, +0xc8 | i64 | the frames from which a read needs the held copy (`FUN_0061f560`) |
| +0xd0, +0xd8 | i64 | the same, three frames further in: a block clear of both reads directly |
| +0xe0 | u8 | low-pass a fast read, set by the constructor |
| +0xe4, +0xe8 | i32 | taps a side now, and as the quality asked |
| +0xf0 | fn | the kernel |
| +0x110 | ptr | the held copy of the taps around a place near an end |
| +0x120 | f64 | `pi / points` |
| +0x128 | f64 | `w`: `pi`, or `pi / speed` above a speed of 1 |
| +0x130 | f64 | `2 cos w` |
| +0x138 | f64 | `2 cos(pi / points)` |

`FUN_0061f6c0(obj, points)` picks the kind: fewer than 3 points is kind 1,
3 is kind 2, more is kind 3 (the sinc) up to 512. The kernel comes from
`DAT_008ce060[kind][channels][format]`, or from `PTR_FUN_008ce0c0` (the
same maths in AVX, `FUN_0061f090`) when the processor has it.
`FUN_0061f5d0` sets the points and, above 6, `+0x120` and `+0x138`.

The qualities FL's sample class asks for by mode (`0x008cebe0`): 1, 3, 8,
12, 16, 32, 64, 128, 256 for modes 1 to 9, and 256 after an FFT low-pass
of the sample for mode 10.

## The speed (`FUN_0061fc90`)

```
speed = v; copy = (v == 1 and frac(place) == 0)
if points > 6:
    if +0xe0 and |v| > 1: w = pi / |v|, +0x130 = 2 cos w   else w = pi, +0x130 = -2
    points = min(512, max(asked, (Round(|v|) + 3) & ~3))      // only a speed above 254 widens it
```

## A block (`FUN_00620bc0(obj, out, n, _)`)

```
if Round(place) < +0xd0 or place + speed * n >= +0xd8:
    frame by frame: i = Trunc(place); a place at or past +0x68 stops the read (or loops)
        frac[k] = place - i; place += speed
        i < +0xc0 or i >= +0xc8: copy the taps round i into +0x110, holding the end frames (FUN_00620420),
            run the kernel on the frames so far, and look at the rest of the block again
        else: tap[k] = the source at i
else:
    place[k] = place + k * speed (ippsVectorSlope_64f); place += speed * n
    split each into Trunc and the rest (FUN_0061c950)
kernel(out, n)
```

`Round` is Delphi's, half to even.

## The sinc kernel (`FUN_0061c1f0` mono, `FUN_0061c690` stereo, float)

```
frac = max(frac, 1e-25)                                    // ippsThreshold_LT_64f_I
a0 = frac * -(pi / N) + pi / 2; a1 = a0 - pi / N           // 0x8ce040 = pi / 2
b0 = frac * w;                  b1 = b0 + w
sines of the four (ippsSin_64f_A50), then one step of each recurrence in double precision, then floats:
    window: cos(pi d / N) for the taps +1, +2 (ahead) and 0, -1 (back)
    sinc:   sin(w d)      for the same
    pi d:   (frac - 1) pi, (frac - 2) pi, frac pi, (frac + 1) pi
N / 2 times, two taps ahead and two back:
    term = sin(w d) * 1/(pi d) * (0.64 c^2 + c + 0.36) * tap      // 1/x is rcpps and one Newton step
    sines move two taps on by s'' = 2 cos(t) s' - s; pi d by -2 pi ahead and +2 pi back
out = (back + ahead) * 0.5                                         // 2^-16 for 16-bit samples
```

So a tap at distance `d` weighs `sin(w d) / (pi d) * (1 + c)(0.36 + 0.64 c) / 2`
with `c = cos(pi d / N)`, over `N` taps a side: a Hann window times a line
that leans it to the centre. The stereo kernel does both sides with the same
numbers in the same order as the mono one does one.

## Not read

- Kinds 1 and 2 (`FUN_0061ad40`.. and `FUN_0061ae70`..), and the 16-bit
  kernels beyond their scale.
- The streaming path (`+0xf8`, `FUN_006277d0`) for a sample that is not in
  memory.
