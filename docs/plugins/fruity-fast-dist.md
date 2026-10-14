# Fruity Fast Dist: state, parameters, signal chain

Recovered from `Fruity Fast Dist_x64.dll` (FL Studio 2026, Delphi native
plugin, image base 0x400000) plus one function family in `FLEngine_x64.dll`,
because the plugin does not distort anything itself: `Eff_Render` hands the
buffer to the host's `DistWave_32FM`. Ghidra project `ghidra/proj`,
decompiles under `ghidra/out/fastdist/` and `ghidra/out/distwave/`. Corpus:
52 uses in the 165 `.flp`, 8 presets in `Data/Patches/Plugin presets/Effects/Fruity
Fast Dist/`. Four Ghidra runs on this plugin (import + seed dump,
`CreatePlugInstance` + callees, the SDK vtable methods + callees,
`DistWave_32FM` in FLEngine); the host's inner loops and table generator were
read from `objdump` because the run budget was spent.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `inferred`, as in
`fruity-balance.md`. One extra: `asm`, read from disassembly rather than a
decompile.

## How the plugin is wired

`CreatePlugInstance` @ `0x006baa70` -> constructor `FUN_006baff0` with the
class reference at `0x006ba890` -> VMT `0x006ba958`. Same constructor pattern
as Balance (Info at `this+0x10`, GetValue/UpdateValue loop over
`vtable+0x30`), so the slots are:

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006baac0 |
| 3 | SaveRestoreState | 0x006baf30 |
| 4 | GetName | 0x006bb690 |
| 6 | ProcessParam | 0x006bb260 |
| 7 | Eff_Render | 0x006bb7c0 |

`TFruityPlugInfo` at `0x0070b000`: SDKVersion 1, LongName "Fruity Fast Dist",
ShortName "Fast Dist", Flags 0x10400000, NumParams 5.

Object fields:

| offset | type | meaning |
|---|---|---|
| 0x120 | i32[5] | the parameters, saved state: Pre, Threshold, Type, Mix, Post |
| 0x134 | f32 | DryVol passed to the host |
| 0x138 | f32 | WetVol passed to the host |
| 0x13c | f32 | Mul (pre gain) passed to the host |
| 0x140 | u8 | "params changed" flag for the UI curve display |

## 1. State layout (event 213, 20 bytes)

`SaveRestoreState` (`FUN_006baf30`) writes/reads 0x14 bytes at `this+0x120`
verbatim, then on restore runs the GetValue/UpdateValue loop. Five
little-endian int32 in parameter index order. The DFM gives each wheel's
`Tag` = parameter index, with its range and default.

| byte | type | idx | parameter (DFM object, hint) | range | default | corpus | confidence |
|---|---|---|---|---|---|---|---|
| 0..3 | i32 | 0 | Pre amp (`PreAmpWheel`) | 64..192 | 128 | 92, 112, 128, 192 | loader, dfm, corpus, preset |
| 4..7 | i32 | 1 | Threshold (`ThresWheel`) | 1..10 | 10 | 6, 8, 10 | loader, dfm, corpus, preset |
| 8..11 | i32 | 2 | Distortion type (`ModeSelect` button) | 0 = A, 1 = B | 0 | 0, 1 | loader, dfm, corpus, preset |
| 12..15 | i32 | 3 | Mix level (`MixWheel`) | 0..128 | 128 | 7..128 | loader, dfm, corpus, preset |
| 16..19 | i32 | 4 | Post gain (`PostGainWheel`) | 0..128 | 128 | 97, 98, 106, 113, 128 | loader, dfm, corpus, preset |

Preset check (the `.fst` files name the plugin "Fruity fast dist", lower
case, which is why a case-sensitive match on the 201 event misses them). The
gain column is `(11^(v/128) - 1) / 10`, see section 2.

| preset | Pre | Thr | Type | Mix | Post | reading |
|---|---|---|---|---|---|---|
| Default | 128 (x1.00) | 10 | A | 128 | 128 | |
| fuzz | 192 (x3.55, +11 dB) | 10 | B | 128 | 80 (x0.35) | maximum drive, smooth curve, output pulled down |
| overdrive | 192 (x3.55) | 10 | A | 128 | 93 (x0.47) | maximum drive, type A |
| The Heat | 171 (x2.36, +7.5 dB) | 10 | B | 128 | 113 (x0.73) | |
| Some Oomph | 181 (x2.87) | 5 | A | 124 | 109 (x0.67) | |
| Something | 157 (x1.79) | 5 | B | 128 | 116 (x0.78) | |
| compressor | 192 (x3.55) | 1 | A | 104 (x0.60) | 93 | Thr 1 is the straight-line curve, so this is a hard clipper driven +11 dB |
| warmth | 82 (x0.37, -8.8 dB) | 7 | A | 128 | 128 | drive below unity, so it never reaches the clip; only the knee compression works |

Every hot name (fuzz, overdrive, The Heat, Some Oomph) drives Pre to 171 or
more; the two tame ones sit at or below 128. Post gain goes down as Pre goes
up, as you would expect from someone levelling presets by ear. The reading
of Threshold is counter-intuitive and worth stating plainly: 10 (the
default) is the heaviest curve, 1 is a straight line. It is a curve-intensity
knob that happens to be labelled Threshold.

## 2. Parameter semantics (`ProcessParam` @ 0x006bb260)

Flags as in the SDK; `REC_FromMIDI` rescales with the wheel's Min/Max and the
2^-30 constant at `0x006bb588`. Any `REC_UpdateValue` on index 0, 3 or 4
recomputes the three floats the host needs. Indices 1 and 2 are passed to the
host as plain integers. Any update (0x10 or 4 set) also flags `this+0x140`
and repaints the curve display.

The gain curve, shared by Pre, Mix and Post (constants: float 1/128 at
`0x006f9208`, `(float) Ln(11.0)` computed in the unit initialiser at
`0x005e179d..0x005e17ae` into `.bss 0x0071b46c`, 1.0 at `0x006bb590`, float
0.1 at `0x006f9200`):

```
g(v) = x == 1 ? 1 : (11^x - 1) / 10      with x = v / 128     // Exp(x * Ln 11)
```

so v = 0 -> 0, v = 64 -> 0.23, v = 128 -> 1.0, v = 192 -> 3.55 (+11.0 dB).
The `== 1` shortcut is there so the default lands on exactly 1.0 in float.

```
pre  = g(params[0])
mix  = g(params[3])
post = g(params[4])
Mul    = pre                                 -> this+0x13c
WetVol = mix * post                          -> this+0x138
DryVol = (post - WetVol) * pre * post        -> this+0x134     (asm-checked at 0x6bb46e..0x6bb48d)
```

`DryVol` carries `post` twice and `pre` once. It is what the code does; I
checked the `mulss` chain in the disassembly because the decompile looked
wrong. At Mix = 128 the dry path vanishes (`WetVol == post`), so the default
is 100 % wet. At Mix = 0 the "dry" path is `pre * post^2` times the input,
not unity.

Display (`FUN_006bb5e0`): type prints "A" or "B" (strings at `0x006bb678`,
`0x006bb688`); the other four print `v / Max` as a fraction of the wheel.
`GetName` section 0 returns the wheel hints: "Pre amp", "Threshold",
"Distortion type", "Mix level", "Post gain".

## 3. Signal chain

### Plugin side (`Eff_Render` @ 0x006bb7c0)

```
if Source != Dest: copy Source -> Dest           (dispatched copy, pointer at 0x0070c438)
Host.DistWave_32FM(Type = params[2], Thres = params[1], Dest, Length * 2,
                   DryVol, WetVol, Mul)          (TFruityPlugHost slot 24, this+0xbc -> vtable+0xc0)
```

`Length * 2`: the stereo buffer is handed over as interleaved mono. The curve
is per-sample, so channels do not interact.

### Host side (`DistWave_32FM`, FLEngine `FUN_0132adc0`, `TFruityHost` VMT `0x013141b8` slot 24)

```
curve = TABLE + Type * 0x28028 + (Thres - 1) * 0x4004 + 2     // int16 entries, index 0..8191, guards at -1 and 8192
if byte[0x016baff0] == 0: distort_nearest(Dest, Dest, Length, curve, 0x1fff, Dry, Wet, Mul)   // 0x006a2b70
else:                     distort_lerp   (Dest, Dest, Length, curve, 0x1fff, Dry, Wet, Mul)   // 0x006a2bf0
```

`TABLE` is the `.bss` block at `0x0165a690` (pointer `0x015e4040`): 2 types x
10 thresholds x 8194 int16. The two inner routines are x87 code, read from
`objdump` (AVX twins at `0x006a5c2a` / `0x006a5aaa` are registered on
capable CPUs; not read, assumed equivalent):

```
scale = Mul * 8191
for each sample x:
    i = Round(x * scale)                       // nearest; the lerp variant truncates and keeps frac
    if i >= 0: t = curve[min( i, 8191)]
    else:      t = -curve[min(-i, 8191)]       // odd symmetry
    // lerp variant: t = curve[i] + (curve[i+1] - curve[i]) * frac, same mirroring, t = 0 if i == INT_MIN
    out = x * Dry + t * (Wet / 32768)          // float 1/32768 at 0x013ec880
```

So the shaper input is `|x| * pre`, hard-clipped at 1.0 by the index clamp,
and the curve output is scaled by `Wet` (which already includes `post`).

### The two curves (generator `FUN_00b96cc0`, FLEngine, `asm`)

Built once at startup in the dB domain. For threshold slot `t` = 1..10 and
`i` = 0..8191, `x = i / 8191`:

```
xdB = x > 0 ? 20 * log10(x) : -10000

// Type A: knee compressor above -25 dB with make-up, hard clip at 0 dBFS via the index clamp
r    = (11 - t) / 10                 // 1.0 at t=1 .. 0.1 at t=10
lift = 1.6 * (t - 1)                 // 0 dB .. 14.4 dB
ydB  = xdB >= -25 ? (xdB + 25) * r - 25 : xdB
if ydB <= -lift: ydB += lift
A[t][i] = Round(10^(ydB / 20) * 32767)

// Type B: power-law bend of the attenuation
p   = 1 - 0.02 * t                   // 0.98 at t=1 .. 0.80 at t=10
a   = x > 0 ? -20 * log10(x) : 1000  // attenuation in dB, >= 0
ydB = -(a ^ p)
B[t][i] = Round(10^(ydB / 20) * 32767)

// guards for the lerp variant
T[t][-1] = T[t][0];  T[t][8192] = T[t][8191]
```

Constants read from `0x00b96f20..0x00b96f98`: 10.0 and 20.0 (the `20 /
Ln 10` and `Ln 10 * 0.05` factors), 0.1, 1.6, 8191, -10000, -25, 25, 32767,
-1000, 0.02, 0.8. RTL callees by fingerprint: `0x40c1d0` Ln, `0x40ae70` Exp,
`0x40c660` Round, `0x4769f0` Power.

What the curves do. Type A at the default t = 10 compresses everything above
-25 dBFS at 10:1 and lifts the result 14.4 dB, so a full-scale input leaves
the table at -8.1 dB and a -25 dB input at -10.6 dB: a flat-topped squash,
which is the "Fast Dist" sound. At t = 1 it is the identity, and the only
non-linearity left is the clamp at 1.0, a bare hard clipper. Type B is
smooth everywhere: at t = 10 a -20 dB input comes out at -11 dB, -40 at -19,
-60 at -27, with no knee. Neither curve is a classic tanh or sine shaper, and
re-implementing them as one would not match.

Re-implementation note: generate the 20 int16 tables with the formulas above
(or keep them as f32, the quantisation is below -90 dB), then run the
per-sample loop with nearest lookup. Keep the 32767/32768 asymmetry if you
want bit-exactness; it is -0.0003 dB.

## 4. What stays unknown

- The byte at `0x016baff0` selecting the interpolating loop. No RIP-relative
  writer found in `.text`, so it is set through a register or a struct; most
  likely an engine quality option. The nearest variant is the default.
- The AVX twins (`0x006a5c2a`, `0x006a5aaa`) were not read. Assumed to be the
  same maths vectorised.
- The dispatched copy routine at `0x0070c438` (irrelevant to the maths).
- `FUN_00b96b90`, right before the generator, builds a different 256-entry
  table (`Power(1.05, i/2 + 15) * 40 * 2pi / samplerate`); unrelated to Fast
  Dist, noted only because it shares the unit.
- Flags bit 28 of the plugin info; `Dispatcher` behaviours (same host messages
  as Balance: 0x21, 0x47, 0x56).

## Corrections from the implementation (crates/plugins/src/fast_dist.rs)

- Type B at −60 dB input gives −26.5 dB, not −27.
- The `if ydB <= -lift` branch is always taken.
- The corpus has 53 Fast Dist slots, not 52.
