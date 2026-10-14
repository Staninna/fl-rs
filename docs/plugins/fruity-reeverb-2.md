# Fruity Reeverb 2: state, parameters, signal chain

Recovered from `Fruity Reeverb 2_x64.dll` (FL Studio 2026, Delphi shell,
image base 0x400000) and `Shared/Reverb_x64.dll` (MSVC C++, image base
0x180000000, one export `CreateReverbInstance`, RTTI `BaseReverb`). The
split is unusual: the Delphi side owns the state, the GUI, the stereo
matrices *and all the coefficient maths*; the C++ side owns the delay memory
and the per-sample network and reads its coefficients straight out of a
shared struct. Ghidra project `ghidra/proj`; decompiles under
`ghidra/out/reeverb2/` (`vmt/` slots, `helpers/` the wrapper update and
matrix code, `helpers2/` the SIMD mixers, `wrapper/` the `TFullReverb`
slots, `chooser/` the CPU dispatch, `asm/` the listing of the wrapper
update) and `ghidra/out/reverbeng/` (`create/`, `methods/` the vtable,
`asm/` the listing of the modulated process). Corpus: 333 uses in the 165
`.flp`, 6 shipped presets plus one `Mastering for strings` found in the
patches tree. Six Ghidra runs on the shell, four on the engine; the shell
needed the extra two because its coefficient maths sits behind a
`TFullReverb` wrapper and a CPU-dispatch table.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `inferred`, `asm`.

## How the plugin is wired

Shell: `CreatePlugInstance` @ `0x006d4450` -> constructor `FUN_006d4b00`,
`TTestPlug` VMT `0x006d4338` (instance size 0x1e0).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006d44a0 |
| 2 | Idle_Public | 0x006d0310 |
| 3 | SaveRestoreState | 0x006d4920 |
| 4 | GetName | 0x006d5890 |
| 5 | ProcessEvent | 0x006d48d0 |
| 6 | ProcessParam | 0x006d5160 |
| 7 | Eff_Render | 0x006d5a90 |

Shell object: `0x120` i32[15] the parameters, `0x15c` u8 tempo-based
predelay, `0x15d` u8 mid/side input, `0x178` the `TFullReverb` wrapper
(VMT `0x006d1840`), `0x180` dry gain, `0x184` dry ramp state, `0x188`
separation, `0x18c` f32[8] ER matrix (4 values, 4 ramp steps), `0x1ac` f32[8]
wet matrix, `0x1cc` tempo (f32, default 120), work buffers at `0xd4`.

Wrapper (`TFullReverb`, parent `TBaseReverb`): `+0x8` the `Reverb_x64.dll`
module, `+0x10` sample rate (int, 44100 at construction), `+0x18` the one
float buffer that holds every delay line, `+0x20` dirty flag, `+0x28` the
C++ object, `+0x30` pointer to the C++ object's data block (object + 0x10),
`+0x38` low cut Hz, `+0x3c` high cut (100 Hz units), `+0x40` predelay
samples, `+0x44` room size, `+0x48` diffusion, `+0x4c` decay samples, `+0x50`
damping (100 Hz units), `+0x54` bass multiplier, `+0x58` crossover Hz,
`+0x5c` mod speed 0..1, `+0x60` mod depth 0..1. Slots: 0 `FUN_006d1d00`
(size the buffers), 1 `FUN_006d1ed0` (carve the 14 delay lines), 2
`FUN_006d1ae0` (push SR and modulation to the engine). The constructor
(`FUN_006d18b0`) loads the DLL, calls `CreateReverbInstance(0)` and asks the
object for its data pointer (vtable slot 3, `FUN_180003f80`).

Engine (`BaseReverb::vftable` @ `0x1800603c0`): slot 0 `FUN_1800028d0`
destructor, slot 1 `FUN_180003270` process with modulation, slot 2
`FUN_180002980` process without modulation (unused by the shell), slot 3
data pointer, slot 4 empty, slot 5 `FUN_180002950` clear the two modulation
lines. The object is 0x2a0 bytes: vtable, back pointer, then the data block
at +0x10 that the Delphi code writes. Offsets below are given as `D+x` for
the data block (what the Delphi code uses) with the engine offset `E+x =
D+x+0x10` where the engine reads it.

## 1. State layout (event 213: 66 bytes; 58 and 53 in older files)

`SaveRestoreState` (`FUN_006d4920`):

```
save:    Write(i32 10001); Write(this+0x120, 0x3e)          // 15 ints, then the two flag bytes
restore: Read(i32 v)
         v >= 10002: refuse (nothing read)
         v <  10000: seek back 4, size = 0x35, this+0x15d = 0      // the first int was data, not a version
         v == 10000: size = 0x34
         v == 10001: size = 0x3e
         Read(this+0x120, size)
         if v == 10000: Read(u8 0x15c); Read(u8 0x15d)
         if v <  10001: p13 = 684 (mod speed), p14 = 0 (mod depth)
         push the two flags to TBCheckBox / MSBtn, GetValue/UpdateValue loop over 15 params
```

The corpus: 271 blocks of 66 bytes (version 10001), 62 of 58 (10000). All
seven `.fst` presets are the 53-byte unversioned form; the three template
`Fruity Reeverb 2.fst` files are version 10000.

Version 10001 (66 bytes):

| byte | type | idx | parameter (DFM object, hint) | range | default | corpus | confidence |
|---|---|---|---|---|---|---|---|
| 0..3 | i32 | | version 10001 | | 10001 | 10001 x271 | loader, corpus |
| 4..7 | i32 | 0 | Low cut (`LowCutWheel`), Hz; 19 = off | 19..3000 | 75 | 75 x65, 19 x19, 648 x9 | loader, dfm, corpus, preset |
| 8..11 | i32 | 1 | High cut (`HighCutWheel`), 100 Hz units; 221 = off | 5..221 | 40 | 40 x87, 112 x13, 91 x13, 221 x8 | loader, dfm, corpus, preset |
| 12..15 | i32 | 2 | Predelay (`DelWheel`), 1/384 s, or 1/192 beat when tempo-based | 0..384 | 0 | 0 x184, 27 x21, 18 x19, 48 x13 | loader, dfm, corpus, preset |
| 16..19 | i32 | 3 | Room size (`RoomSizeWheel`) | 1..100 | 50 | 50 x83, 100 x67, 90 x19 | loader, dfm, corpus, preset |
| 20..23 | i32 | 4 | Diffusion (`DiffWheel`), % | 0..100 | 100 | 100 x227 | loader, dfm, corpus, preset |
| 24..27 | i32 | 5 | Decay time (`DecWheel`), 0.1 s | 1..200 | 15 | 15 x46, 22 x21, 45 x20 | loader, dfm, corpus, preset |
| 28..31 | i32 | 6 | High damping (`DampWheel`), 100 Hz units; 221 = off | 5..221 | 40 | 40 x133, 31 x20, 50 x19 | loader, dfm, corpus, preset |
| 32..35 | i32 | 7 | Bass multiplier (`BassWheel`), 0.01 | 20..300 | 100 | 100 x210, 150 x17, 120 x16 | loader, dfm, corpus, preset |
| 36..39 | i32 | 8 | Crossover (`CrossWheel`), Hz | 25..2000 | 500 | 500 x203, 298 x22, 740 x17 | loader, dfm, corpus, preset |
| 40..43 | i32 | 9 | Stereo separation (`SepWheel`) | -64..64 | 0 | 0 x156, -40 x17, -64 x10 | loader, dfm, corpus, preset |
| 44..47 | i32 | 10 | Dry level (`DryWheel`) | 0..160 | 128 | 128 x179, 0 x31 | loader, dfm, corpus, preset |
| 48..51 | i32 | 11 | Early reflection level (`ERWheel`) | 0..160 | 64 (DFM 128; shipped Default 128) | 64 x146, 128 x48, 0 x40 | loader, dfm, corpus, preset |
| 52..55 | i32 | 12 | Wet level (`WetWheel`) | 0..160 | 64 (shipped Default 87) | 90 x17, 160 x14, 64 x11 | loader, dfm, corpus, preset |
| 56..59 | i32 | 13 | Mod speed (`ModSpeedWheel`) | 0..2048 | 684 | 684 x218, 610 x7 | loader, dfm, corpus |
| 60..63 | i32 | 14 | Mod depth (`ModDepthWheel`) | 0..2048 | 0 | 0 x190, 1280 x7, 1233 x6 | loader, dfm, corpus |
| 64 | u8 | | tempo-based predelay (`TBCheckBox`) | 0, 1 | 0 | 0 x254, 1 x17 | loader, corpus |
| 65 | u8 | | mid/side input (`MSBtn`) | 0, 1 | 0 | 0 | loader, corpus |

Version 10000 (58 bytes): the same without bytes 56..63; the two flag bytes
follow the 13 ints directly. Mod speed and depth are set to 684 and 0.

Unversioned (53 bytes, the shipped presets): 13 ints, then one byte that
lands in the mod speed slot and is overwritten by 684 before anything reads
it; mid/side is forced to 0, so the tempo-based flag of an old preset is
lost. Every shipped preset has that byte at 0 anyway.

Preset check (`.fst` files name the plugin "Fruity Reeverb 2"). Levels
through section 2.

| preset | Low cut | High cut | Predelay | Size | Diff | Decay | Damp | Bass | Cross | Sep | Dry | ER | Wet | reading |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Default | 75 Hz | 4.0 kHz | 0 | 50 | 100 % | 1.5 s | 4.0 kHz | x1.00 | 500 Hz | 0 | 0 dB | -12.7 dB | -18.0 dB | medium room, flat bass |
| Small Studio | 100 Hz | 6.0 kHz | 0 | 17 | 100 % | 0.5 s | 3.0 kHz | x0.90 | 330 Hz | 0 | 0 dB | -12.7 dB | -16.7 dB | shortest tank: lengths at 28 % of full size |
| Drum Room | 100 Hz | 3.6 kHz | 0 | 35 | 100 % | 0.6 s | 3.6 kHz | x1.00 | 330 Hz | 0 | 0 dB | -12.7 dB | -16.3 dB | |
| Large Hall | off | 11.2 kHz | 47 ms | 40 | 100 % | 2.2 s | 5.0 kHz | x1.00 | 500 Hz | 0 | 0 dB | -12.7 dB | -14.1 dB | low cut 19 = off: the one preset with the allpass bypassed |
| The Venue | 400 Hz | 6.0 kHz | 81 ms | 60 | 65 % | 2.3 s | 3.7 kHz | x0.70 | 330 Hz | 0 | 0 dB | -12.7 dB | -15.9 dB | diffusion below 100: the input allpass gains shrink (section 2) |
| Cathedral | 33 Hz | 2.1 kHz | 70 ms | 90 | 100 % | 4.5 s | 3.1 kHz | x1.50 | 298 Hz | 0 | 0 dB | -12.7 dB | -17.6 dB | bass x1.5 makes the band under 298 Hz ring 50 % longer |

The corpus leans on room size 100 (67 blocks) and separation -40/-64 (27):
the widening side of the separation knob (section 2).

## 2. Parameter semantics

Shell `ProcessParam` (`FUN_006d5160`): store the int, then

```
idx 0..8, 13, 14   FUN_006d4e10: push to the wrapper (below), mark dirty (wrapper+0x20 = 1)
idx 9  separation  0x188 = p9 / 64; rebuild both matrices
idx 10 dry         0x180 = level(p10 / 128)
idx 11, 12         rebuild the ER (0) or wet (1) matrix: FUN_006d5030
```

`level(x) = x == 1 ? 1 : (11^x - 1) / 10` (`0x73746c = ln 11` from
`FUN_005e2110`, `0x715224 = 0.1`). Dry uses `p/128`: 128 is unity, 160 is
+4.1 dB. ER and wet use `p/256` (`0x715234`): 128 is -12.7 dB, 160 is
-9.2 dB, so the two sliders never reach unity.

Matrix (`FUN_006d5030` -> `FUN_005db570(m, pan 0, level, sep, 0)`):

```
if sep == 0:             m = [level, level, 0, 0]                   // LL, RR, LR, RL
else:
    m0 = m1 = level      // pan law at pan 0: cos(pi/4) * level * 0x73742c, which the start-up code sets to undo the 0.707 (inferred: not read)
    s  = sep > 0 ? (5^sep - 1) * 0.25 : sep                         // 0x005db7d0 = 5, 0x715218 = 0.25
    c  = min(1, s) * 0.5                                            // 0x715210
    m2 = m0 * c;  m0 *= 1 - c;  m3 = m1 * c;  m1 *= 1 - c
if mid/side input: m1 = -m1; m3 = -m3
```

Positive separation (up to +64) feeds each side into the other up to 50 %,
a mono collapse; negative separation feeds a *negative* copy, up to -50 % at
-64, which widens. The engine returns a stereo pair for both ER and tank, so
"separation" is a post matrix on that pair, not a change to the network.
Matrix changes ramp over one block (`FUN_005db4c0` / `FUN_005e0540`: the
step is `(target - current) / n`, capped at a per-sample maximum from the
host).

Wrapper push (`FUN_006d4e10`), with `SR` the wrapper's rate:

```
predelay  n = tempo-based ? p2 * SR * 0.3125 / tempo : p2 * SR / 384      // 0x006d5010 = 60/192, 0x006d5018 = 384
          wrapper+0x40 = round(n); if changed: slot 0 (resize the buffers)   // 1/384 s steps, or 1/192 beat (the GUI shows steps:ticks with 48 ticks)
decay     +0x4c = p5 * SR * 0.1                                              // seconds = p5 / 10
room      +0x44 = p3;  diffusion +0x48 = p4
bass      +0x54 = p7 * 0.01;  crossover +0x58 = p8
low cut   +0x38 = p0 == 19  ? 0 : p0            // Hz
high cut  +0x3c = p1 == 221 ? 0 : p1            // 100 Hz units
damping   +0x50 = p6 == 221 ? 0 : p6
mod       +0x5c = p13 / 2048;  +0x60 = p14 / 2048
```

### Coefficients (`FUN_006d1f30`, run at the top of the next process call when dirty; asm at `ghidra/out/reeverb2/asm`)

`SR` the wrapper rate, `data` the shared block. Constants: `0x006d27b0`
0.5, `0x006d27b8` pi, `0x006d27c0` 200 pi, `0x006d27c8` 2, `0x006d27d0`
0.01, `0x006d27d8..0x006d27f0` 0.07, 0.11, 0.35, 0.51, `0x006d27f8` 0.25,
`0x006d2808` 3.302e-7, `0x006d2810` 0.36, `0x00739568` the decay base
(not written anywhere in `.text`; from its use it is `ln 0.001`, the
standard -60 dB constant, tag `inferred`).

```
slot 2 (FUN_006d1ae0): data+0x1f0 = SR;  data+0x1f4 = mod speed;  data+0x1f8 = mod depth

low cut   lc = +0x38:  lc == 0 ? data+0xc = 0 : (t = tan(pi min(lc, SR/2) / SR); data+0xc = (t - 1) / (t + 1))      // first-order allpass, HP = (x - ap) / 2
high cut  hc = +0x3c:  hc == 0 ? data+0x24 = 1 : (c = 2 - cos(2 pi 100 min(hc, SR/2 /100) / SR); data+0x24 = 1 - (c - sqrt(c^2 - 1)))   // one-pole LP coefficient
damping   same formula on +0x50 -> data+0x28

scale  = SR * (room + 15) / 115                                     // MulDiv; room 100 = full size, room 1 = 14 %
len[i] = max(1, T[i] * scale / 20000),  i = 0..11                    // T = 142 107 379 277 672 4453 1800 3720 908 4217 2656 3163 (0x00727574)
                                                                    // T/20 is the length in ms at room 100: 7.1 5.35 18.95 13.85 33.6 222.6 90 186 45.4 210.8 132.8 158.2
tapL[j] = TL[j] * scale / 20000,  j = 0..6                           // TL = 266 2974 1913 2496 1690 187 1566 (0x007275a4)
tapR[j] = TR[j] * scale / 20000                                     // TR = 353 3627 1728 3173 1811 335 621  (0x007275c0)
write indices data+0xc0[i] %= len[i]; if room changed: zero all 12 lines and the ER line

d = diffusion * 0.01
input diffuser gains (d == 0 -> all 0):
   data+0x1c = exp(ln0.001 * (len0 + len1) / (2 SR) / (d * 0.07))    // used for all four input allpasses (engine reads only this one)
   data+0x20 = exp(ln0.001 * (len2 + len3) / (2 SR) / (d * 0.11))    // computed, not read by the engine
   data+0x14 = exp(ln0.001 * (len4 + len8) / (2 SR) / (d * 0.35))    // tank allpass 1 (both halves)
   data+0x18 = exp(ln0.001 * (len6 + len10) / (2 SR) / (d * 0.51))   // tank allpass 2 (both halves)
loop   = len5 + len7 + len9 + len11                                 // the four tank delays
g      = exp(ln0.001 * loop / decaySamples) ^ 0.25                  // data+0x10: feedback gain per tank delay; four of them per loop give -60 dB in the decay time
gBass  = exp(ln0.001 * loop / (decaySamples * bassMult)) ^ 0.25
r      = gBass / g;   data+0x38 = (r - 1) * 0.5;   r = min(r, 1)
t      = tan(pi crossover / SR);  data+0x34 = (t - r) / (t + r)       // first-order allpass for the shelf; r < 1 only when bass < 1

ER taps: f = SR * (room + 3) * 3.302e-7                              // room 50: 0.772 at 44.1 kHz
   lenL[k] = round(EL[k] * f),  EL = 1102 2904 3784 4873 5878 7470 9062 9263 (0x007275dc)      // 19 .. 162 ms at room 50
   lenR[k] = round(ER[k] * f),  ER = 837 1423 2865 3938 5278 5978 7532 9263  (0x007275fc)
   m = min(lenL[k], lenR[k]);  gainL[k] = 0.5 m / lenL[k];  gainR[k] = 0.36 m / lenR[k]       // asm 0x006d26bd..0x006d2703; the decompiler shows m fixed at the first tap, the listing takes the min per tap
```

The buffers (`FUN_006d1d00`, slot 0) are sized for room 100 (`T * SR /
20000`), plus the predelay and an ER line of the next power of two above
`0.3163 SR` (`0x006d1ec0`); one allocation, 14 pointers (`FUN_006d1ed0`).
The live lengths shrink with room size inside those buffers. The ER line is
read with a mask, so its taps wrap if a room is big enough to exceed it: at
room 100 and 44.1 kHz the longest tap is 9263 * 44100 * 103 * 3.302e-7 =
13.9k samples against a 16384 line, which fits.

## 3. Signal chain

Shell `Eff_Render` (`FUN_006d5a90`):

```
mono = MS ? L - R : L + R                           // FUN_005dffb0 / FUN_005dfed0: a sum, not an average
engine.process(mono, wet, er, n)                    // vtable slot 1, FUN_180003270; er == null when the ER matrix is all zero
Dest = Source * dry (ramped)                        // FUN_005db3c0 / FUN_005dfc80
Dest += ER * matrixER;  Dest += wet * matrixWet     // FUN_005df6b0: [m0 L + m2 R, m1 R + m3 L], ramped
```

Engine process (`FUN_180003270`, per sample; `E+` offsets; `ap(buf, len,
g)` is the Schroeder allpass `v = buf[i] - g x; buf[i] = x + g v; return v`,
indices count down and wrap at `len`):

```
// modulation (asm 0x18000336a..0x18000345d)
rate   = speed^3 * 9.9 + 0.1 Hz;  phase += rate / SR; phase -= floor(phase)              // E+0x1fc, 0x180060420, 0x1800603f8
depth  += (depthTarget - depth) * (1 - k),  k = powf(0.01, 1 / (0.1 SR))                 // E+0x20c; depthTarget = modDepth * 200/44100 * SR samples (0x1800603f0), 99 % in 100 ms
mod    = depth * (sin(2 pi phase) + 1) * 0.5                                             // 0 .. depth samples

// input: high cut, low cut
x  = in + 1e-30
lp += (x - lp) * cHigh                                                                   // E+0x10, E+0x34
ap  = (lp - s1) * cLow + s2;  s2 = lp;  s1 = ap                                           // E+0x14, E+0x18, E+0x1c
x   = cLow != 0 ? (lp - ap) * 0.5 : lp                                                   // first-order high pass when low cut is on

// four input diffusers, one gain
x = ap(line0, len0, gIn);  x = ap(line1, len1, gIn);  x = ap(line2, len2, gIn);  x = ap(line3, len3, gIn)      // E+0x60..0x78, lengths E+0x108..0x114, gIn = E+0x2c

// predelay, then the tank. Two halves in a figure of eight; each half: read the other half's last delay, filter, allpass, delay, allpass, delay
pd = predelay[idx];  predelay[idx] = x                                                    // E+0xc0, length E+0x138
pd += 1e-20

for half in (A, B):                   // A reads line11 (E+0xb8) and writes line7 (E+0x98); B reads line7 and writes line11
    c   = lastDelay[other half] * g                                                      // g = E+0x20
    apx = (c - sX) * cCross + sX1;  sX1 = c;  sX = apx                                  // crossover allpass, E+0x44
    y   = (apx + c) * bass + c + pd                                                      // low shelf: bass = E+0x48 = (gBass/g - 1)/2; the low band is scaled by gBass/g
    y   = (y - dampState) * cDamp + dampState;  dampState = y                             // one-pole damping, E+0x38
    y   = modDelay[half].write(y); y = modDelay[half].read(mod)                          // E+0x290 / E+0x298, FUN_180001220 / FUN_180001180: linear interpolation at (now - mod)
    v   = ap(line4|8,  len4|8,  gT1)                                                     // tank allpass 1, gT1 = E+0x24
    v   = delay(line5|9, len5|9) * g                                                     // tank delay 1
    v   = ap(line6|10, len6|10, gT2)                                                     // tank allpass 2, gT2 = E+0x28
    write line7|11 (the half's last delay)

// early reflections (when requested): 8 taps per side from the ER line, which is written with the diffused input
erL = sum over the first seven L taps of gainL[k] * erLine[write - lenL[k]] with signs (+ + - + + + +); the eighth L tap is computed but not summed
erR = sum over all eight R taps with signs (+ - + + - + + -)
// tank output taps, Dattorro style, weights w_k = (1 - g) K_k + g with K = 0.25, 0.6, 0.7, 0.2, 0.5 (0x18006036c..0x180060378)
outL = line9[t0] - w1 line9[t1] - w3 line10[t2] - w4 line11[t3] + w2 line5[t4] + line6[t5] + w5 line7[t6]      // t = tapL[0..6]
outR = w1 line5[t0'] + line5[t1'] - w3 line6[t2'] - w4 line7[t3'] - w2 line9[t4'] + line10[t5'] + w5 line11[t6']   // t' = tapR[0..6]
```

Line numbering follows `T[]`: lines 0..3 are the input diffusers, 4..7 the
first half of the tank (allpass, delay, allpass, delay), 8..11 the second.
The output taps read the two halves' delays and allpasses at seven fixed
offsets each, exactly the topology of Dattorro's 1997 plate; the table
values are that paper's delay lengths scaled by about 1.49 and the tap
weights blend towards 1 as the decay gain grows.

Reset (`FUN_006d1c60`, on construction, Flush and predelay change): zero the
whole buffer, the eight filter states and the modulation lines (engine slot
5).

## 4. What stays unknown

- `0x00739568` (the decay base) and `0x0073742c` (the pan-law make-up) live
  in `.bss` and no RIP-relative writer was found; both are tagged
  `inferred` from their use (`ln 0.001` and `sqrt 2`).
- The tap weights' interaction with `g` is read from the decompile; the
  listing was not checked for the exact pairing of `K_k` with `tap_k`.
- `data+0x20` (second diffuser gain) is computed but the engine reads
  `E+0x2c` for all four input allpasses; whether that is a bug or an old
  layout is unknown.
- The ER sign pattern is read from the decompile of `FUN_180003270` lines
  126..144; the magnitudes are the `gainL/gainR` above.
- GUI only: the Dispatcher's window messages, `Idle_Public`, the visual.

## Corrections from the implementation (crates/plugins/src/reeverb_2.rs)

- 333 corpus states, not 323; 3 templates carry it besides the 7 presets.
- Dry 160 is +5.6 dB, not +4.1.
- The decay feedback counts the four delays but not the four allpasses in the loop, so RT60 measures 1.39× the setting (2.29 s at 1.5 s). Open question whether FL shows the same.
