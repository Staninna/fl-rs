# Fruity Multiband Compressor: state, parameters, signal chain

Recovered from `Fruity Multiband Compressor_x64.dll` (FL Studio 2026, Delphi
native plugin, image base 0x400000) and `comp_eng_x64.dll` beside it (MSVC
C++ with RTTI, image base 0x180000000, 538 kB). Ghidra projects
`ghidra/proj/mbcomp` (both DLLs) and `ghidra/proj/mbcompeng` (the engine
alone), output under `ghidra/out/mbcomp/`: `vmt/` and `slots/` for the Delphi
side, `engine_all.c` for every function of the engine ahead of its CRT,
`engasm/` for the listings the maths was read from. Corpus: 35 uses in 13 of
the 165 `.flp`, 31 presets in the plugin's folder and 13 more `.fst` elsewhere
in `Data`. All 79 states are 124 bytes and parse to the last byte.

The Delphi DLL has no DSP. It loads `Comp_Eng_x64.dll`, calls its one export
`CreateEngine`, and gets a C++ object of class `MultiBandComp`. The engine is
its own code: none of FL's limiter or Maximus classes (`TPeakLookahead2`,
`TRMSLookahead`, `TBasicDelay_32FS`) are in it, and nothing in FLEngine is
called for audio. It is about five times the size of Transient Processor's
engine: three band classes, two filter families, an envelope class and a
limiter.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `asm`, `inferred`.

## How the plugin is wired

`CreatePlugInstance` @ `0x006d3ed0` -> constructor `FUN_006d4040`, class
reference `PTR_PTR_006d3ce8` -> VMT `0x006d3db0` (`TTestPlug`, parent
`TDelphiFruityPlug`).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006d4340 |
| 3 | SaveRestoreState | 0x006d5b50 |
| 4 | GetName | 0x006d4d90 |
| 6 | ProcessParam | 0x006d4fe0 |
| 7 | Eff_Render | 0x006d49a0 |

The engine object is at `this+0x1c8`. Its vtable (`0x180069998`):

| slot | address | what |
|---|---|---|
| 1 | 0x1800095c0 | `process(inputs, outputs, n)`, two pointers each |
| 2 | 0x180009640 | `setParameter(index 0..27, float 0..1)` |
| 3 | 0x180009520 | `(0, rate)` sample rate, `(2, 0)` reset, `(3, 0)` |
| 4 | 0x1800094c0 | destructor |

`Dispatcher` ID 4 sends `(0, rate)`. A flush (`FUN_006d4cf0`) sends `(2, 0)`
and `(3, 0)`. `Eff_Render` deinterleaves, calls `process`, interleaves, and
keeps peaks for the meters.

Engine classes, from RTTI: `MultiBandComp`; `Band` with `HighBand`
(`0x180069808`), `MidBand` (`0x180069930`), `LowBand` (`0x1800698f0`); `LPF`
(`0x180069548`) and `HPF` (`0x180069598`); `FirFilter` with `FirLpf`
(`0x180069670`), `FirHpf` (`0x1800696d8`), `FirBpf` (`0x180069740`); `Comp`;
`Env` and `LimEnv` (`0x180069888`); `Limiter` (`0x180069850`).

## 1. State layout (event 213, 124 bytes)

`SaveRestoreState` (`FUN_006d5b50`):

```
save:    Write(int32 0); Write(this+0x138, 0x78)
restore: this+0x1a8..0x1ab = 0; Read(int32 ver); if ver < 1:
           Read(this+0x138, 0x78)
           for each of the 28 parameters: ProcessParam(i, GetValue, UpdateValue | 0x10)
           push this+0x1ac to its control; redraw
```

The 0x78 bytes are 28 parameters and two i32 of the window. A version of 1 or
more is left unread. The loader checks no range.

| byte | type | idx | parameter (DFM object) | range | default | corpus and presets | confidence |
|---|---|---|---|---|---|---|---|
| 0..3 | i32 | | version | | 0 | 0 (x79) | loader, corpus, preset |
| 4..7 | i32 | 0 | Master volume (`MasterVolumeWheel`) | 0..65536 | 32768 | 25088..65536 | loader, dfm, corpus, preset |
| 8..11 | i32 | 1 | Limiter (`FreezeBtn`, Tag 1) | 0, 1 | 0 | 0 (x29), 1 (x50) | loader, dfm, corpus, preset |
| 12..15 | i32 | 2 | Filter type (`IIRFIRSelect`, Tag 2) | 0 IIR, 1 FIR | 0 | 0 (x40), 1 (x39) | loader, dfm, corpus, preset |
| 16..19 | i32 | 3 | High band state (`HighbandStateSel`) | 0 active, 1 muted, 2 bypassed | 0 | 0 (x51), 1 (x6), 2 (x22) | loader, dfm, corpus, preset |
| 20..23 | i32 | 4 | H Freq | 20..20000 | 17000 | 2430..20000 | loader, dfm, corpus, preset |
| 24..27 | i32 | 5 | H Output | 0..65536 | 21845 | 12343..65536 | loader, dfm, corpus, preset |
| 28..31 | i32 | 6 | H Threshold | 0..65536 | 45875 | 6554..65536 | loader, dfm, corpus, preset |
| 32..35 | i32 | 7 | H Ratio | 0..65536 | 32768 | 11045..65536 | loader, dfm, corpus, preset |
| 36..39 | i32 | 8 | H Attack | 1..1000 | 100 | 1..765 | loader, dfm, corpus, preset |
| 40..43 | i32 | 9 | H Release | 100..10000 | 1000 | 100..4610 | loader, dfm, corpus, preset |
| 44..47 | i32 | 10 | H Knee | 0..65536 | 52429 | 26573..65536 | loader, dfm, corpus, preset |
| 48..51 | i32 | 11 | Mid band state | 0..2 | 0 | 0 (x54), 1 (x9), 2 (x16) | loader, dfm, corpus, preset |
| 52..55 | i32 | 12 | M Freq H | 20..20000 | 17000 | 20..20000 | loader, dfm, corpus, preset |
| 56..59 | i32 | 13 | M Freq L | 20..20000 | 6500 | 20..20000 | loader, dfm, corpus, preset |
| 60..79 | i32 x5 | 14..18 | M Output, Threshold, Ratio, Attack, Release | as high | 21845, 48875, 32768, 100, 1000 | | loader, dfm, corpus, preset |
| 80..83 | i32 | 19 | M Knee | 0..65536 | 52429 | 0..65536 | loader, dfm, corpus, preset |
| 84..87 | i32 | 20 | Low band state | 0..2 | 0 | 0 (x52), 1 (x7), 2 (x20) | loader, dfm, corpus, preset |
| 88..91 | i32 | 21 | L Freq | 20..20000 | 6500 | 20..11437 | loader, dfm, corpus, preset |
| 92..111 | i32 x5 | 22..26 | L Output, Threshold, Ratio, Attack, Release | as high | 21845, 45875, 32768, 100, 1000 | | loader, dfm, corpus, preset |
| 112..115 | i32 | 27 | L Knee | 0..65536 | 52429 | 0..65536 | loader, dfm, corpus, preset |
| 116..119 | u8 x4 | | meter shows the output and not the input: master, low, mid, high (`MasterIOBtn` ..) | 0, 1 | 0 | 0, 0x01010100, 0x01010101 | loader, corpus, inferred |
| 120..123 | i32 | | graph speed, pushed to the control at form+0x838 | | 50 | 0, 29, 50, 55 | loader, corpus |

Bytes 116..123 change no sample. `MidbandRatioWheel` has `DefaultValue`
32678 in the DFM against a `Value` of 32768; the Default preset has 32768.

Preset check: "Default" is the table's defaults byte for byte. Of the 31
presets, 13 use the FIR type and 22 the limiter.

## 2. Parameter semantics

### The shell (`ProcessParam` @ 0x006d4fe0)

`REC_UpdateValue` stores the integer at `this+0x138+4*i` and sends

```
engine.setParameter(i, (float)(value - Min) / (float)(Max - Min))
```

Parameter 2 also tells the host the latency (host message 0x1e): 0x34 = 52
frames when it is not 0, else 0. The frequency wheels are tied in the window:
moving L Freq, M Freq L, M Freq H or H Freq drags its neighbour along so the
bands stay in order (the tail of `ProcessParam`, GUI only, the state keeps
all four).

### The engine's map (`FUN_180009640` -> `FUN_18000aab0`)

The engine has 47 parameters of its own; the 28 map onto them. `p` is the 0..1
value. Constants: floats 1.0 `0x180069398`, 0.5 `0x1800694e8`, 0.33
`0x1800699c0`, 0.66 `0x1800699c4`, double 0.33 `0x1800699c8`.

```
0  Master:       master = 4^p - 1                         -> engine+0x268   (0 .. 1 at the middle .. 3)
1  Limiter:      on = p > 0.5                             -> engine+0x26c
2  Filter type:  iir = (1 - p) >= 0.5; reset everything   -> each band+0x10, engine+0x320
3, 11, 20 State: compress = p < 0.33                      -> band+0x30
                 mix = (0.33 <= p <= 0.66) ? 0 : 1        -> engine+0x280, 0x284, 0x288 (high, mid, low)
4  H Freq, 12 M Freq H, 13 M Freq L, 21 L Freq:
                 f = 19981^sqrt(p) + 19 Hz                (20 Hz .. 20 kHz; float 19981.0 @0x1800695f4, 19.0 @0x1800695f0)
5, 14, 22 Output: out = 8^p - 1                           -> band+0x38      (0 .. 1 at a third .. 7; float 8.0 @0x180069840)
6, 15, 23 Threshold: T = 1000^p * 0.001                   -> env+0x28       (-60 .. 0 dB)
7, 16, 24 Ratio:     r = p                                -> env+0x30
8, 17, 25 Attack:    fc = 1 / (0.1 p + 1/17000) Hz        -> env+0x68, the corner of the level's low pass
9, 18, 26 Release:   rel = 0.99 p + 0.01 s                -> env+0x1c       (10 ms .. 1 s)
10, 19, 27 Knee:     k = 0.995 p^2 + 0.005                -> env+0x2c
```

So the wheel numbers are not the units their ranges suggest. A frequency wheel
at 6500 is 300 Hz and at 17000 it is 9.24 kHz: the defaults cross at 300 Hz
and 9.2 kHz. Attack 1..1000 is `p * 100 ms`, release 100..10000 is tenths of
a millisecond.

### The envelope (`Env`, set by `FUN_180002d40`, knee by `FUN_180002cc0`)

```
lo = T * k - 0.005;  hi = 2 T - lo                        -> env+0x38, env+0x40
a  = 1 / (4 (lo - T))                                     -> env+0x44
over(s) = s < lo  ? 0
        : s <= hi ? s - (a (s - hi)^2 + T)                // stored as a s^2 - 2 a hi s + (a hi^2 + T)
        :           s - T
gain(s) = T / (over(s) * r + T)
```

The knee is a parabola from `lo` to `hi`, which sit the same distance either
side of the threshold. `k = 1` is the hard end (5e-3 of full scale wide),
`k -> 0` starts the knee at nothing. The ratio is not a ratio in dB: `r = 0`
is no compression and `r = 1` holds the output at `T` (`T / s` above the
knee).

## 3. Signal chain

### `MultiBandComp::process` (`0x1800095c0`)

```
awake = some input sample > 1e-7 or < -1e-7, or some band's filter still holds a value beyond +-1e-7    // FUN_18000a390
if not awake: reset every band and the limiter; copy the input to the output; return
in chunks of 1024 frames:                                                                          // FUN_18000aee0
    copy the input into three buffer pairs; each band works on its own in place
    out = (mid * mixMid + low * mixLow + high * mixHigh) * master                                  // FUN_180009990
    if limiter on: out = limiter(out)
```

### A band (`HighBand::process` 0x180007260, `MidBand` 0x180008b60, `LowBand` 0x180008310)

```
x = iir ? IIR filter(s)(x) : FIR filter(x)
if compress:  g = env.gain(max(|L|, |R|));  L *= g;  R *= g       // Comp::process 0x180001180, Env 0x180002b30
else:         env.reset()
x *= out
```

High is one high pass, low one low pass. Mid in the IIR type is the low pass
at M Freq H and then the high pass at M Freq L; in the FIR type one band pass.

### IIR filters (`LPF`/`HPF`, design 0x1800032a0 / 0x180003150, run 0x1800034d0)

4th order Butterworth as two RBJ biquads, `w = 2 pi f / SR`, `alpha = sin w /
1.082` and `sin w / 2.614` (twice the Qs 0.541 and 1.307):

```
low pass:  b = ((1 - cos w)/2, 1 - cos w, (1 - cos w)/2)      high pass: b = ((1 + cos w)/2, -(1 + cos w), (1 + cos w)/2)
a = (1 + alpha, -2 cos w, 1 - alpha), everything divided by a0
y = ((x1 b1 + x b0 + x2 b2) - y1 a1) - y2 a2                  // per side, stage 1 over the block and then stage 2
```

An input or output with a zero exponent (denormal) is made 0. These are
Butterworth, not Linkwitz-Riley: a low pass and a high pass at one corner are
each 3 dB down there and in phase, so their sum is 3 dB up at the corner. The
IIR type does not sum flat.

### FIR filters (`FirFilter`, 105 taps, run 0x180005800 / 0x180005320)

Transposed direct form, 52 frames late. The taps are the inverse transform of
a response drawn in straight lines, times a Kaiser window:

```
window[k] = I0(5.10226 * sqrt(1 - (n/52)^2)) / I0(5.10226),  n = k - 52      // FUN_180005290, I0 as 1 + sum_{m=1..15} ((x/2)^m / m!)^2 (FUN_180004650)
low  (FirLpf, FUN_180006880):  1 up to 0.83 f, a line down to 0 at 1.17 f
high (FirHpf, FUN_180006360):  0 up to 0.83 f, a line up to 1 at 1.17 f, 1 above
mid  (FirBpf, FUN_180005ce0):  a line up from 0.83 fL to 1.17 fL, 1 to 0.83 fH, a line down to 1.17 fH;
                               where 0.83 fH <= 1.17 fL the two lines meet in a point and there is no flat top;
                               where 0.83 fL > 1.17 fH the taps are all 0
a corner above SR/2 is put at SR/2 - 1
```

The pieces (`T = 1/SR`, `n = k - 52`): a flat part from 0 to `f`
(`FUN_180004e80`) is `2 g f T sinc`, a flat part from `f` up
(`FUN_180004b70`) is `g (sin(n pi)/(n pi) - 2 f T sin(2 pi f T n)/(2 pi f T
n))`, and a line `G(w) = a + b w` from 0 to `w` (`FUN_180004cd0`) is

```
n = 0:  (b w^2 + 2 a w) / (2 pi SR)
else:   sin(n w T)/(n w T) * (2 b w^2 + 2 a w)/(2 pi SR) - (1 - cos(n w T)) * 2 b / (2 pi SR T^2 n^2)
```

A line between two points (`FUN_180004830`) is that at its upper end minus
that at its lower end. With L Freq = M Freq L and M Freq H = H Freq the three
responses add up to 1 at every frequency, so the FIR type does sum flat: an
impulse through three bypassed bands comes back as itself 52 frames later
(tested to 1e-4).

### The compressor's level (`Env::gain` 0x180002b30)

```
x = max(|L|, |R|)
// peak with a release that is a curve from the last peak, not a recursion    (FUN_180002ba0)
if x > level:  level = peak = x;  since = 0
elif since <= 10 SR:  level = peak * e^(-(since / SR) / rel);  since += 1
// attack: one pole low pass at fc (bilinear, FUN_18000b300 / 0x18000b230)
t = tan(pi fc / SR);  s = (level + levelBefore) * t/(1+t) + s * (1-t)/(1+t)
g = gain(s)
```

The frame after a peak still has the peak (`since` is 0 there). After ten
seconds without a new peak the level stops falling.

### The limiter (`Limiter::process` 0x180007c10)

One `LimEnv` for each side, each the envelope above with fixed wheels: threshold
0.98 (-1.2 dB), ratio 0.75, attack 3e-5, release 0.005 (15 ms), knee 0.9. The
gain is `T / (over * 0.75 + T)` on that side's own level. `LimEnv`'s peak
(`FUN_180007800`) stretches the release by a factor `G` that every new peak
moves:

```
on a new peak x:  G = (G < 1) ? 1 : max(0.01, 1 - 0.4 (x - 1))
release time = G * rel
```

`G` is one static float (`0x18007e000`) for both sides and for every instance
of the plugin in the process. Then a soft clip and a fixed gain:

```
if |y| >= 0.998:  t = (|y| - 0.998) / 0.003;  y = sign(y) * (0.998 + 0.002 * (t >= 1 ? 1 : (1.5 - 0.5 t^2) t))
out = y * 0.99999
```

## 4. What stays unknown

- `FUN_18001bc80`, `FUN_18001cb50`, `FUN_18001d050`, `FUN_18001d7f0` and
  `FUN_18001d748` are taken to be the CRT's `powf`, `cosf`, `sinf`, `tan` and
  `sqrtf` from their arguments and from the designs coming out right (the FIR
  bands sum to 1, the IIR corners are 3 dB down). Their bodies were not read.
- Engine parameters 2..5, 0x11..0x14, 0x1e..0x21, 0x2a..0x2d (per band: a flag
  at band+0x3c and two floats) are not reachable from the 28 and were not seen
  in the audio path. `FUN_18000a6e0` and `FUN_18000a430` move them to the
  meters.
- The 16 factory programs in `FUN_180001410` ("Default", "Comp1.9" ..
  "Weirdie") fill the engine's own parameter block. The shell overwrites all 28
  on every load, so they were not read.
- The limiter's static `G` makes two instances with their limiters on depend on
  each other, and on the order the host runs them.

## Implementation notes (crates/plugins/src/multiband_compressor.rs)

- Nothing is shared with `limiter.rs` or `maximus.rs`: the engine has none of
  their classes, and its crossover is Butterworth or FIR where Maximus's is
  Linkwitz-Riley.
- The limiter's `G` is one per instance, and `reset` puts it back at 1.
- Two points of a line at one frequency add nothing, where FL divides by
  zero. No state FL's wheels can make gets there.
- The silence test runs once for every block the host hands over, as FL's
  does for its own blocks.
- Latency is 52 frames in the FIR type and 0 in the IIR type.
