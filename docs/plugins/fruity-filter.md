# Fruity Filter: state, parameters, signal chain

Recovered from `Fruity Filter_x64.dll` (FL Studio 2026, Delphi native plugin,
image base 0x400000). Ghidra project `ghidra/proj`, decompiles under
`ghidra/out/frfilter/` (`vmt/` holds `CreatePlugInstance` and the SDK slots
with callees). Corpus: 32 uses in 4 of the 165 `.flp`, 4 shipped presets. Two
Ghidra runs. The filter loop is inline in `Eff_Render` and came out of the
decompiler cleanly; only the "Center" DC remover sits behind a runtime
function pointer and was read from `objdump`.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `inferred`, `asm`.

## How the plugin is wired

`CreatePlugInstance` @ `0x006cb670` -> constructor `FUN_006cbaa0`, class
reference `PTR_PTR_006cb488` -> VMT `0x006cb550` (`TTestPlug`).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006cb6c0 |
| 2 | Idle_Public | 0x006c93a0 |
| 3 | SaveRestoreState | 0x006cb920 |
| 4 | GetName | 0x006cc220 |
| 5 | ProcessEvent | 0x006c8a30 |
| 6 | ProcessParam | 0x006cbd10 |
| 7 | Eff_Render | 0x006cc3b0 |

`TFruityPlugInfo` at `0x0071b458`: SDKVersion 1, LongName "Fruity Filter"
(`0x006cb470`), ShortName (`0x006cb47e`), Flags 0x10400000, NumParams 6.

Object fields:

| offset | type | meaning |
|---|---|---|
| 0x120 | i32[6] | the parameters, saved state |
| 0x138..0x148 | f32[5] | coefficient targets: f, q, LP mix, BP mix, HP mix |
| 0x14c..0x15c | f32[5] | current coefficients (ramped towards the targets) |
| 0x160..0x16c | f32[4] | SVF `low` state: L, R, L stage 2, R stage 2 |
| 0x170..0x17c | f32[4] | SVF `band` state, same order |
| 0x180..0x18c | f32[4] | SVF `high` state, same order |
| 0x190 | u8 | x2 (second stage on) |
| 0x194 | struct | DC remover: `+4` (0x198) f32 A, `+8` (0x19c) f32 B, `+0x28..+0x40` (0x1bc..0x1d4) f64 pL, pR, vL, vR |
| 0x1fc | u8 | Center (DC removal) on; saved, not a parameter |

## 1. State layout (event 213, 29 bytes; 28 in old files)

`SaveRestoreState` (`FUN_006cb920`) is versioned:

```
save:    Write(int32 2); Write(this+0x120, 0x18); Write(u8 center)
restore: Read(int32 ver); if ver < 3: Read(this+0x120, 0x18); center = 0; if ver > 1: Read(u8 c); center = c & 1
         push center to its selector; GetValue/UpdateValue loop
```

The corpus has both shapes: 20 uses of 28 bytes (version 1, no Center byte,
so Center loads as off) and 12 of 29 (version 2). A version >= 3 refuses to
load the block at all.

| byte | type | idx | parameter (DFM object, hint) | range | default | corpus | confidence |
|---|---|---|---|---|---|---|---|
| 0..3 | i32 | | version, 1 or 2 | | 2 | 1 (x20), 2 (x12) | loader, corpus, preset |
| 4..7 | i32 | 0 | Cutoff frequency (`CutoffFreqWheel`) | 0..1024 | 1024 | 1024 (x15), 0 (x8), 134..368 | loader, dfm, corpus, preset |
| 8..11 | i32 | 1 | Resonance (`ResoWheel`, Tag 1) | 0..1024 | 205 | 0 (x23), 205 (x4), 118..235 | loader, dfm, corpus, preset |
| 12..15 | i32 | 2 | Low pass (`LowPassWheel`, Tag 2) | 0..1024 | 1024 | 1024 (x25), 0 (x7) | loader, dfm, corpus, preset |
| 16..19 | i32 | 3 | Band pass (`BandPassWheel`, Tag 3) | 0..1024 | 0 | 0 (x32) | loader, dfm, preset |
| 20..23 | i32 | 4 | High pass (`HighPassWheel`, Tag 4) | 0..1024 | 512 | 0 (x25), 1024 (x7) | loader, dfm, corpus, preset |
| 24..27 | i32 | 5 | X 2 (`X2CheckBox`, Tag 5) | 0, 1 | 0 | 0 (x30), 1 (x2) | loader, dfm, corpus, preset |
| 28 | u8 | | Center (`CenterSelector`, "Remove DC offset") | 0, 1 | 1 | 1 (x10), 0 (x2) | loader, dfm, corpus, preset |

Preset check (`.fst` files name the plugin "Fruity Filter", matched as is).
The derived columns use the formulas of section 2; `fc` is the Chamberlin
corner `(fs/pi) * asin(f/2)` at 44.1 kHz.

| preset | Cut | Reso | LP | BP | HP | x2 | C | f | fc | q | mixes LP/BP/HP | reading |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Default | 1024 | 205 | 1024 | 0 | 512 | 0 | 1 | 1.000 | 7.35 kHz | 0.84 | 1.0 / 0 / 0.23 | wide open; the HP mix is inaudible with f at its ceiling |
| Party next door | 160 | 1024 | 864 | 0 | 0 | 0 | 1 | 0.0266 | 187 Hz | 0.20 | 0.66 / 0 / 0 | muffled resonant low pass, the "next room" effect |
| Retro sound | 480 | 1024 | 0 | 0 | 1024 | 1 | 1 | 0.144 | 1.01 kHz | 0.20 | 0 / 0 / 1.0 | resonant high pass, two stages |
| Tiny speakers | 544 | 0 | 0 | 1024 | 464 | 1 | 1 | 0.186 | 1.31 kHz | 1.00 | 0 / 1.0 / 0.20 | band pass with a little HP, two stages, no resonance |

Every preset has Center on; the two corpus files with it off are the old
28-byte blocks where it defaults off.

## 2. Parameter semantics (`ProcessParam` @ 0x006cbd10)

Flags per the SDK; `REC_FromMIDI` uses 2^-30 at `0x006cbec0`. On
`REC_UpdateValue` the value is normalised with the wheel's own factor,
`x = clamp((v - Min) * wheel.scale, 0, 1)` (`wheel+0x3cc`, a double which for
these 0..1024 wheels is 1/1024, tag `inferred`; the preset numbers come out
right with it), and `FUN_006cbf10` turns it into a coefficient target:

```
idx 0  f   = (26^x - 1) * 0.04                 -> this+0x138     // Exp(x * Ln 26), doubles 26.0 @0x006cc030, 0.04 @0x006cc040
idx 1  q   = 1 - 0.8 * x                       -> this+0x13c     // 0.8 @0x006cc048
idx 2..4   mix = (11^x - 1) / 10               -> this+0x138 + 4*idx   // 11.0 @0x006cc050, float 10.0 @0x006cc058
idx 5  x2  = (v == 1); on change clear the 12 SVF state floats (this+0x160, 0x30 bytes)
```

`f` runs from 0 (x = 0) through 0.164 (x = 0.5, `26^0.5 = 5.10`) to exactly
1.0 at x = 1, and `q` from 1.0 (no resonance; the default 205 gives 0.84)
down to 0.2 at full resonance. The
mixes are the same base-11 curve as the other Fruity gains: 0 -> 0, 512 ->
0.23, 1024 -> 1.0.

None of this depends on the sample rate. `f` is the raw Chamberlin
coefficient, so the cutoff in Hz scales with the project rate: f = 1.0 is
7.35 kHz at 44.1 kHz and 16 kHz at 96 kHz. Fruity Filter is one of the
plugins that sounds different at a different rate, and a port must decide
whether to mimic that.

`REC_UpdateControl` also mirrors the value to a second slider (`FUN_006cc060`,
the `*ValueSlider` objects in the DFM). Display (`FUN_006cc140`): index 1
prints `1 - q` (so 0.16 at the default), index 5 prints Off/On, the others
print the coefficient itself.

`Dispatcher` ID 4 (SetSampleRate) is the only audio-relevant message: it sets
the DC remover's `A = 1.0070235 / SR` (double at `0x006cb8c0`), `B = 0.96`
(0x3f75c28f) and zeroes its four doubles (`FUN_005de330`). ID 2 (Flush) is not
handled here, so the SVF state survives a transport stop.

## 3. Signal chain (`Eff_Render` @ 0x006cc3b0)

```
// 1. ramp the five coefficients  (FUN_006cc2e0, per coefficient)
for k in 0..4:
    start[k] = cur[k];  d[k] = 0
    if cur[k] != target[k]:
        if Length < 128:
            d[k]   = (target - cur) / 128                       // double 1/128 at 0x006cc390
            cur[k] = cur + d * Length                           // slews over 128 samples whatever the block
            if cur[k] < DAT_0071b4f8: cur = 0; d = -start / 128 // denormal guard, .bss threshold set at 0x006cc358
        else:
            d[k]   = (target - cur) / Length;  cur[k] = target  // reaches the target within the block
ramping = any d[k] != 0

// 2. Chamberlin state-variable filter, stereo, coefficients advanced per frame when ramping
f, q, mLP, mBP, mHP = start
for each frame (xL, xR):
    if ramping: f += d0; q += d1; mLP += d2; mBP += d3; mHP += d4
    for ch in L, R:
        low[ch]  += f * band[ch]
        high[ch]  = x[ch] - low[ch] - q * band[ch]
        band[ch] += f * high[ch]
        y[ch]     = low[ch] * mLP + band[ch] * mBP + high[ch] * mHP
        if x2:                                                  // second identical stage, fixed q = 1
            low2[ch]  += f * band2[ch]
            high2[ch]  = y[ch] - low2[ch] - band2[ch]
            band2[ch] += f * high2[ch]
            y[ch]      = low2[ch] * mLP + band2[ch] * mBP + high2[ch] * mHP
    Dest = (yL, yR)
flush every state float with |s| < 2^-24 to 0                  // FUN_005de250, threshold 0x33800000 at 0x0070cf34; 6 floats, 12 with x2

// 3. Center: DC removal on the output     (this+0x1fc)
if center:
    tmp = copy of Dest (Length * 8 bytes)                       // GetMem + dispatched copy (pointer 0x0071c2b8)
    dc_remove(this+0x194, tmp, Dest, Length)                    // pointer 0x0071c700 -> 0x005de400 (AVX twin 0x005e1190)
    FreeMem(tmp)
```

The order inside the SVF is the textbook Chamberlin one (low first, then
high from the new low, then band). `q` here is the damping term (1/Q), so the
Resonance wheel lowers it: q = 1 is Q = 1, q = 0.2 is Q = 5. The second stage
runs with q fixed at 1, so "x2" doubles the slope (24 dB/oct) without
doubling the resonance peak, and uses the same three mixes. With f at 1.0 the
SVF is at the edge of its stability region, which is where FL leaves the
default cutoff: fully open and slightly ringing if resonance is high.

The three mixes are independent gains on the three SVF outputs, not a
morph, so LP + HP together give a notch-ish response and all three at 1.0
reconstruct roughly the input.

### Center (`0x005de400`, x87, `asm`)

Per channel, with `p` the tracked offset and `v` its velocity (doubles at
`this+0x1bc/0x1c4` and `0x1cc/0x1d4`), A and B from the Dispatcher:

```
v = v + A * (x - p)
p = p + v
out = x - p
v = v * B
```

That is a second-order tracker with transfer function `H(z) = (1 - z^-1)(1 -
B z^-1) / (1 - (1 + B - A) z^-1 + B z^-2)`: a zero at DC, a zero at 0.96 that
almost cancels the fast pole at 0.9606, and a slow pole at `1 - A / (1 - B)`,
which with `A = 1.007 / SR` is a corner of `1.007 / (0.04 * 2 pi) = 4.0 Hz` at
any sample rate. In short, Center is a 4 Hz first-order DC blocker. The four
state doubles are denormal-flushed after each block (`0x005de230`).

## 4. What stays unknown

- The wheel scale at `wheel+0x3cc` is read from the control at runtime; 1/1024
  reproduces the presets but was not read from the DFM (the DFM only carries
  Min/Max).
- The AVX DC-remover twin at `0x005e1190` and the dispatched copy routine
  were not read.
- `DAT_0071b4f8`, the ramp's denormal threshold, is computed at startup;
  value not recovered. It only matters for tails below -140 dB.
- Flags bit 28, `Dispatcher` IDs 0x39 and 5 (window min/max), GUI only.

## Corrections from the implementation (crates/plugins/src/filter.rs)

- The default state's HP mix is audible: it takes 19.8 kHz down 22 dB (2 dB without it).
- The Center 4 Hz one-pole reading holds to 2 %, with a lift of up to 1.4 % above the corner.
