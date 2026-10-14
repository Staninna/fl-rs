# Fruity Stereo Enhancer: state, parameters, signal chain

Recovered from `Fruity Stereo Enhancer_x64.dll` (FL Studio 2026, Delphi
native plugin, image base 0x400000). Ghidra project `ghidra/proj`, decompiles
under `ghidra/out/stereoenh/` (`vmt/`). Corpus: 24 uses in 10 of the 165
`.flp`, 3 shipped presets. Two Ghidra runs; the 2x2 apply routine is behind
a runtime pointer and was checked in `objdump`.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `inferred`, `asm`.

## How the plugin is wired

`CreatePlugInstance` @ `0x006be880` -> constructor `FUN_006bf000`, class
reference `PTR_PTR_006c04e0` -> VMT `0x006be768` (`TTestPlug`).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006be8d0 |
| 2 | Idle_Public | 0x006bb630 |
| 3 | SaveRestoreState | 0x006bef40 |
| 4 | GetName | 0x006bfd20 |
| 5 | ProcessEvent | 0x006bacc0 |
| 6 | ProcessParam | 0x006bf530 |
| 7 | Eff_Render | 0x006bfec0 |

`TFruityPlugInfo` at `0x0070f508`: SDKVersion 1, LongName "Fruity Stereo
Enhancer" (`0x006be672`), ShortName (`0x006be689`), Flags 0x10400000,
NumParams 6.

Object fields:

| offset | type | meaning |
|---|---|---|
| 0x120 | i32[6] | the parameters, saved state |
| 0x138..0x144 | f32[4] | current matrix `[LL, RR, RL, LR]` |
| 0x148..0x154 | f32[4] | matrix targets |
| 0x158 | f32 | pan, -1..1 |
| 0x15c | f32 | volume gain |
| 0x160 | f32 | separation, -0.75..0.75 |
| 0x164 | f32 | phase offset in units of 10 us |
| 0x168 | i32 | where the offset delay runs: -1 none (length 0), 0 before the matrix, 1 after |
| 0x16c | ptr | one-channel delay line (class VMT `0x006bc938`, element size 4): +8 start, +0x10 pos, +0x20 end, +0x28 size bytes |
| 0x174 | f32 | `SR / 100000`, initialised 0.441 (0x3ee1cac1), set by Dispatcher ID 4 |
| 0x178 | u8 | matrix needs recomputing |

## 1. State layout (event 213, 24 bytes)

`SaveRestoreState` (`FUN_006bef40`) is a verbatim dump of `this+0x120`, 0x18
bytes, with the GetValue/UpdateValue loop on restore. Six little-endian int32
in parameter index order, matching the DFM `Tag`s.

| byte | type | idx | parameter (DFM object, hint) | range | default | corpus | confidence |
|---|---|---|---|---|---|---|---|
| 0..3 | i32 | 0 | Panning (`PanWheel`) | -128..128 | 0 | 0 (x21), -18 (x2), 22 | loader, dfm, corpus, preset |
| 4..7 | i32 | 1 | Volume (`VolWheel`) | 0..320 | 256 | 256 (x22), 318, 320 | loader, dfm, corpus, preset |
| 8..11 | i32 | 2 | Stereo separation (`SepWheel`, Tag 2) | -96..96 | 0 | 0 (x6), -96..48, mostly negative | loader, dfm, corpus, preset |
| 12..15 | i32 | 3 | Phase offset (`PhaseWheel`, Tag 3) | -512..512 | 0 | 0 (x9), -248..384 | loader, dfm, corpus, preset |
| 16..19 | i32 | 4 | Phase effect position (`PosSelect`, Tag 4) | 0 pre, 1 post | 1 | 1 (x23), 0 | loader, dfm, corpus, preset |
| 20..23 | i32 | 5 | Phase inversion (`InvSelect`, Tag 5) | 0 none, 1 left, 2 right | 0 | 0 (x24) | loader, dfm, preset |

Preset check (`.fst` files name the plugin "Fruity stereo enhancer"). Derived
columns from section 2: `sep` = v/128, `LL` and `RL` the matrix at centre
pan, offset in ms and samples at 44.1 kHz.

| preset | Pan | Vol | Sep | Phase | Pos | Inv | LL | RL | offset | reading |
|---|---|---|---|---|---|---|---|---|---|---|
| Default | 0 | 256 (x1.0) | 0 | 0 | post | none | 1.0 | 0 | 0 | identity |
| Subtle | 0 | 256 | -40 (-0.31) | -24 | post | none | 1.16 | -0.36 | 0.19 ms (8 smp) left | mild widening, a few samples of Haas on the left |
| For strings | -18 (-0.14) | 256 | -54 (-0.42) | -248 | post | none | 1.21 | -0.51 | 13.7 ms (604 smp) left | wider, right channel down 14 %, a clear Haas delay |

The corpus never uses inversion and almost always leaves the delay post
matrix; separation is negative (widening) in 15 of 18 non-zero uses. The
"Fruity Stereo Enhancer.flp" demo project runs phase 384 (88 ms right) with
separation -47.

## 2. Parameter semantics (`ProcessParam` @ 0x006bf530)

Flags per the SDK; `REC_FromMIDI` uses 2^-30 at `0x006bf840`. Any update
sets `this+0x178`. Constants: floats 1/128 at `0x006fd1f8`, 1/256 at
`0x006fd1fc`, 0.5 at `0x006fd1e0`, 0.1 at `0x006fd1f0`, Ln 11 in `.bss
0x0071f46c` (set at `0x005e1c95` from the double 11.0 at `0x005e22a0`),
doubles 1001.0 @`0x006bf850`, 1/512 @`0x006bf858`, 50.0 @`0x006bf860`,
100000.0 @`0x006bebc0`.

```
idx 0  pan   = v / 128                                   -> this+0x158
idx 1  vol   = x == 1 ? 1 : (11^x - 1) / 10,  x = v / 256   -> this+0x15c    (320 -> x1.90, +5.6 dB; same law as Balance)
idx 2  sep   = v / 128                                   -> this+0x160    (-0.75 .. 0.75)
idx 3  ofs10us = (1001^(|v| / 512) - 1) * 50             -> this+0x164, then resize the line (FUN_006bf4c0)
idx 4  pos                                               -> this+0x168 via FUN_006bf450 (see below)
idx 5  inv                                               -> read by the matrix recompute
```

Delay length (`FUN_006bf4c0`, under the plugin lock, also on SetSampleRate):

```
N = Round(ofs10us * SR / 100000)                // samples; 512 -> 500 ms, 256 -> 15.3 ms, 128 -> 2.3 ms, 24 -> 0.19 ms
line.resize(N)                                  // FUN_006bc9c0: N * 4 bytes, realloc, clear, pos = start
this+0x168 = N < 1 ? -1 : params[4]             // FUN_006bf450: no delay stage at all when the offset is 0
```

The sign of the Phase offset picks the channel: `params[3] > 0` delays the
right channel, otherwise the left (`FUN_006bfe50` offsets the buffer pointer
by one float). Display (`FUN_006bf8b0`): pan prints `v/128`, volume `v/320`,
separation `v/96` with "separated" (negative) or "merged" (positive) and
"Original" at 0, phase prints the ms value with "left"/"right".

Matrix recompute (`FUN_006bf2c0`, run at the top of `Eff_Render` when
dirty), into the targets `[LL, RR, RL, LR]`:

```
LL = RR = vol * (1 - 0.5 * sep)
RL = LR = LL * sep
if pan < 0:  f = pan + 1;  RR *= f;  LR *= f           // right output attenuated
else:        f = 1 - pan;  LL *= f;  RL *= f           // left output attenuated
if inv == 1: LL = -LL; RL = -RL                        // left output inverted
if inv == 2: RR = -RR; LR = -LR                        // right output inverted
```

What that is in mid/side terms. With `g = vol`, the output is `L' = g((1 -
s/2) L + s(1 - s/2) R)`, so mid gain `g (1 - s/2)(1 + s)` and side gain
`g (1 - s/2)(1 - s)`. At s = 0.75 (full merge) mid is 1.09 g and side 0.16 g:
almost mono, slightly louder. At s = -0.75 (full separation) mid is 0.34 g
and side 2.41 g: the centre drops 9 dB and the sides come up 7.6 dB. It is a
linear M/S rebalance with a built-in `1 - s/2` level trim, not a widener
with any frequency shaping; the Haas delay is what adds the sense of width.
Pan is a simple attenuation of one output channel (constant-gain, not
constant-power), and inversion flips one output channel's polarity.

## 3. Signal chain (`Eff_Render` @ 0x006bfec0)

```
buf = Source
if pos == 0 (pre):                                      // this+0x168
    if Source != Dest: copy Source -> Dest (pointer 0x00710938); buf = Dest
    phase_delay(buf, Length)                            // FUN_006bfe50
if dirty: recompute [LL, RR, RL, LR] (above)
ramp the four gains this+0x138.. towards 0x148.. (FUN_005e0040 x4, Balance's ramp, max step = host+0x34)
Dest = M(t) * buf                                       // pointer 0x0070ffe0 -> 0x005df0e0 (AVX 0x005df290)
if pos > 0 (post):
    phase_delay(Dest, Length)
```

`phase_delay` (`FUN_006bfe50`): on the chosen channel only, swap each sample
with the ring buffer slot under `pos` and advance `pos`, wrapping at the end:
a pure delay of N samples with no interpolation. `pos == -1` (offset 0) skips
it entirely.

The 2x2 apply (`0x005df0e0`, SSE, `asm`-checked): with `m = [LL, RR, RL,
LR]` and per-frame deltas `d`, `L' = L*m0 + R*m2`, `R' = R*m1 + L*m3`, `m +=
d`; fast paths when all deltas are zero (plain scale when `m2 = m3 = 0`,
skipped altogether when `m0 = m1 = 1.0` in place). Same routine family as
Balance's `FUN_005dee90`.

Re-implementation note: integer-sample delay on one channel (pre or post),
then the ramped 2x2 matrix above. The whole character is in the sign
convention: negative separation widens, positive narrows, and the `1 - s/2`
trim keeps full merge from clipping.

## 4. What stays unknown

- The ramp limit `host+0x34` and the ramp's denormal threshold
  (`DAT_0071f434`, `.bss`), as in Balance.
- The AVX apply twin at `0x005df290` and the dispatched copy at `.bss
  0x0071de90`.
- The delay line's `Clear` virtual (`0x006bc9a0`) was not read; it is called
  on every resize, so changing the Phase offset while playing drops the
  buffered samples.
- `Dispatcher` IDs 0x33/0x34/0x39 are GUI; flags bit 28.
