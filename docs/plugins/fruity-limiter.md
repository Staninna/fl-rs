# Fruity Limiter: state, parameters, signal chain

Recovered from `Fruity Limiter_x64.dll` (FL Studio 2026, Delphi native
plugin, image base 0x400000). Ghidra project `ghidra/proj`, decompiles under
`ghidra/out/limiter/` (`vmt/` the SDK slots with callees, `dsp/` the
lookahead and delay classes plus the start-up initialisers, `misc/` the time
mapping and one-pole filter, `sat/` the saturators, `mavg/` the moving
average, `asm/` listings of `Eff_Render` and `ProcessParam` for the IPP call
arguments the decompiler drops). Corpus: 346 uses in the 165 `.flp`, 13
shipped presets. Six Ghidra runs (import, slots, DSP classes, helpers,
saturators plus listings, moving average); one over the budget of five, spent
on the moving average because the limiter's RMS detector lives behind a CPU
dispatch pointer.

The vector maths is Intel IPP, bound by name at start-up (`FUN_005a4010`
loads `ippsLog10_32f_A24`, `ippsExp_32f_A24`, `ippsMul_32f32fc_I` and the
rest into a pointer table); the names below are the IPP names, so every bulk
operation is exact. The scalar curves are FL's shared Delphi plugin helper:
`(7^x - 1) / 6` for gains, `(3^x - 1) / 2` for ceilings, `(501^x - 1) * 2 ms`
for times. Nothing in FLEngine is called for audio except the sidechain buffer
fetch.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `inferred`, `asm`.

## How the plugin is wired

`CreatePlugInstance` @ `0x006d3230` -> constructor `FUN_006d3a00`, class
`TTestPlug`, VMT `0x006d30c0` (instance size 0x280).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006d3280 |
| 2 | Idle_Public | 0x006cdf10 |
| 3 | SaveRestoreState | 0x006d3760 |
| 4 | GetName | 0x006d5980 |
| 5 | ProcessEvent | 0x006cd260 |
| 6 | ProcessParam | 0x006d4490 |
| 7 | Eff_Render | 0x006d5d60 |

DSP classes (unit `DSP_Lookahead`, `DSP_Delay`): `TPeakLookahead2` VMT
`0x006d0458` (sliding-window maximum), `TRMSLookahead` VMT `0x006d05f0`
(boxcar mean of squares), `TBasicDelay_32FS` VMT `0x006cf760` (the lookahead
delay, 8 bytes per frame).

Object fields the audio path touches:

| offset | type | meaning |
|---|---|---|
| 0x120 | i32[18] | the parameters, saved |
| 0x168 | i32[18] | the "spare state" (A/B compare copy), saved |
| 0x1b0 | i32 | visual scroll speed (`SpeedSlider`), saved, GUI only |
| 0x1b4 | i32 | visual target tab (`TargetSel`), saved, GUI only |
| 0x1b8..0x1bb | u8[4] | visual toggles `VisIBtn`, `VisOBtn`, `VisGBtn`, `VisLBtn`, saved, GUI only |
| 0x1bc | u8 | visual disabled, saved, GUI only |
| 0x1c0 | i32 | sidechain input number, saved |
| 0x1c4 / 0x1cc / 0x1d4 | ptr | peak lookahead, RMS lookahead, delay line |
| 0x1dc / 0x1e4 / 0x1ec | dyn array | one-pole stages: limiter release, comp attack/release, limiter attack (0x24 bytes each: f64 coefA, f64 coefR, f64 state, i32 same-flag) |
| 0x1f4 / 0x1f8 | f32 | gate: last gain, release coefficient |
| 0x1fc | i32 | lookahead in frames (the limiter attack time) |
| 0x200 / 0x204 / 0x208 | f32 | input gain, ceiling, ceiling times the safe factor |
| 0x20c / 0x210 / 0x214 | f32 | noise gain, noise threshold, gate threshold `min(0x208, 0x210)` |
| 0x218 / 0x21c | f32 | comp threshold linear, in dB |
| 0x220 / 0x224 | f32 | comp slope `R = 1/ratio`, `R - 1` |
| 0x228 / 0x22c / 0x230 / 0x238 | f32, i32 | knee `K`, `ln(1+K)` or `1/ln(1+K)`, `1/K`, knee sign |
| 0x234 | f32 | `thr * 100`, the +40 dB cap on the detector |
| 0x23c | f32[3] | saturator: knee start, span, 1/span |
| 0x248 / 0x249 | u8 | saturation on, compressor on |
| 0x24c / 0x250 | i32 | silence hold length, silence countdown |
| 0x277 | u8 | "safe threshold" flag, saved (version 7) |

## 1. State layout (event 213: 169 bytes; 168 and 161 in older files)

`SaveRestoreState` (`FUN_006d3760`):

```
save:    Write(i32 7); Write(this+0x120, 0xa4); Write(u8 flags)   // flags bit 0 = this+0x277
restore: Read(i32 ver); sidechain = 0
         size = ver < 6 ? 0x9d : 0xa4
         if ver > 4: Read(this+0x120, size)
         if ver > 6: Read(u8 f); this+0x277 = f & 1
         push GUI fields to their controls, GetValue/UpdateValue loop over 18 params,
         recompute (FUN_006d3ee0 via the loop), sidechain selector clamp (FUN_006d36e0),
         host message 0x1e with the lookahead (latency)
```

A version 4 or lower reads nothing and keeps the defaults. The corpus has 190
blocks of 169 bytes (version 7), 156 of 168 (version 6); the 13 shipped
presets are version 5, 161 bytes, and the three `Fruity Limiter.fst`
template presets version 6.

| byte | type | idx | parameter (DFM object, hint) | range | default | corpus (v7) | confidence |
|---|---|---|---|---|---|---|---|
| 0..3 | i32 | | version 5, 6 or 7 | | 7 | 7 x190, 6 x156 | loader, corpus, preset |
| 4..7 | i32 | 0 | Gain (`GainWheel`) | 0..2000 | 1000 | 1000 x107, 0..1816 | loader, dfm, corpus, preset |
| 8..11 | i32 | 1 | Soft saturation threshold (`SoftSatWheel`) | 0..1000 | 1000 | 1000 x179, 136..995 | loader, dfm, corpus, preset |
| 12..15 | i32 | 2 | Limiter ceiling (`PeakMaxWheel`) | 0..2000 | 1000 | 1000 x102, 2000 x32 | loader, dfm, corpus, preset |
| 16..19 | i32 | 3 | Limiter attack time (`PeakAttTimeWheel`) | 0..10000 | 1115 | 1115 x94, 0 x78, 3034 x5 | loader, dfm, corpus, preset |
| 20..23 | i32 | 4 | Limiter attack curve (`PeakAttTnsWheel`), stage count | 1..8 | 3 | 3 x189, 2 | loader, dfm, corpus, preset |
| 24..27 | i32 | 5 | Limiter release time (`PeakRelTimeWheel`) | 0..10000 | 5000 | 5000 x139, 0 x13, 3270 x5 | loader, dfm, corpus, preset |
| 28..31 | i32 | 6 | Limiter release curve (`PeakRelTnsWheel`), stage count | 1..8 | 3 | 3 x188, 1, 2 | loader, dfm, corpus, preset |
| 32..35 | i32 | 7 | Limiter peak window (`PeakLookaheadWheel`) | 0..10000 | 2882 | 2882 x145, 0 x28, 3034 x5 | loader, dfm, corpus, preset |
| 36..39 | i32 | 8 | Comp threshold (`RMSMaxWheel`) | 0..1000 | 1000 | 1000 x81, 280 x10, 93 x5 | loader, dfm, corpus, preset |
| 40..43 | i32 | 9 | Comp ratio (`RMSRatioWheel`) | -1000..1000 | 0 | 0 x82, 1000 x32, -172..752 | loader, dfm, corpus, preset |
| 44..47 | i32 | 10 | Comp knee (`RMSKneeWheel`) | -1000..1000 | 0 | 0 x178, 200 x2, 1000 | loader, dfm, corpus, preset |
| 48..51 | i32 | 11 | Comp attack time (`RMSAttTimeWheel`) | 0..10000 | 0 | 0 x173, 1115 x4, 5296 x2 | loader, dfm, corpus, preset |
| 52..55 | i32 | 12 | Comp release time (`RMSRelTimeWheel`) | 0..10000 | 6666 | 6666 x155, 5034..9645 | loader, dfm, corpus, preset |
| 56..59 | i32 | 13 | Comp curve (`RMSTnsWheel`), stage count | 1..8 | 1 | 1 x180, 2 x7, 6 x2, 3 | loader, dfm, corpus, preset |
| 60..63 | i32 | 14 | Comp RMS window (`RMSLookaheadWheel`, label SUSTAIN) | 0..10000 | 650 | 650 x164, 0 x19, 3185..4938 | loader, dfm, corpus, preset |
| 64..67 | i32 | 15 | Noise gain (`SilGainWheel`) | 0..1000 | 1000 | 1000 x176, 0 x14 | loader, dfm, corpus, preset |
| 68..71 | i32 | 16 | Noise threshold (`SilThresWheel`) | 0..1000 | 0 (DFM says 100) | 0 x178, 29..460 | loader, dfm, corpus, preset |
| 72..75 | i32 | 17 | Noise release time (`SilRelTimeWheel`) | 0..10000 | 4187 | 4187 x180, 6523 x5, 0 x2 | loader, dfm, corpus, preset |
| 76..147 | i32[18] | | spare state: the same 18 values, stored by `CompStoreBtn` for A/B compare. Not used by audio. | | defaults | defaults x188 | loader, corpus, preset |
| 148..151 | i32 | | `SpeedSlider`, visual scroll speed | | 50 | 50 x188, 100 x2; presets 50, one 55 | loader, corpus, preset |
| 152..155 | i32 | | `TargetSel`, which section the visual shows (0 limiter, 1 comp) | 0, 1 | 0 | 1 x101, 0 x89 | loader, corpus, preset |
| 156..159 | u8[4] | | `VisIBtn`, `VisOBtn`, `VisGBtn`, `VisLBtn`: input, output, gain, level curves shown | 0, 1 each | 1 1 1 1 | 01010101 x189, 00010101 x1 | loader, corpus, preset |
| 160 | u8 | | visual disabled (`this+0x1bc`); the last byte of a version 5 block | 0, 1 | 0 | 0 | loader, corpus, preset |
| 161..163 | u8[3] | | padding (version 6 and 7) | | 0 | 0 | loader, corpus |
| 164..167 | i32 | | sidechain input number (version 6 and 7) | >= 0 | 0 | 0 x116, 1 x68, 2 x4, 4, 12 | loader, corpus |
| 168 | u8 | | bit 0: "safe threshold" flag (`this+0x277`, version 7) | 0, 1 | 0 | 0 x188, 1 x2 | loader, corpus |

Bytes 148..163 are the GUI's business: they change what the display draws and
nothing in `Eff_Render` reads them. The `Default.fst` noise threshold is 0, so
the gate is off out of the box even though the DFM default says 100.

Preset check (`.fst` files name the plugin "Fruity Limiter"). Times through
the curve of section 2; the release columns show the knob time, which the
GUI displays doubled.

| preset | Gain | Sat | Ceil | Att ms/curve | Rel ms/curve | Window | Comp thr | Ratio | Knee | Comp att/rel/curve | RMS win | Noise gain | Noise thr | Noise rel | reading |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Default | 0 dB | off | 0 dB | 2.0/3 | 43/3 | 10 ms | off | 1:1 | 0 | 0/124/1 | 1.0 ms | 0 dB | -inf | 25 ms | a 2 ms lookahead brickwall at 0 dBFS and nothing else |
| Max loudness | +12.0 dB | off | 0 dB | 2.0/3 | 43/3 | 2.0 ms | off | 1:1 | 0 | 0/124/1 | 1.0 | 0 dB | -inf | 25 | 12 dB into the limiter, 2 ms peak window |
| Max loudness (slight saturation) | +13.1 dB | -3.7 dB | +2.5 dB | 2.0/3 | 43/3 | 2.0 | off | 1:1 | 0 | 0/124/1 | 1.0 | 0 dB | -inf | 25 | ceiling above 0 dBFS, so the soft knee from -3.7 dB does the clipping |
| Max loudness except noise | +12.0 dB | off | 0 dB | 2.0/3 | 43/3 | 2.0 | off | 1:1 | 0 | 0/124/1 | 1.0 | 0 dB | -28.2 dB | 200 | the gate glides to unity gain (noise gain 0 dB), so below -28 dB the 12 dB boost is undone |
| Noise gate | 0 dB | off | 0 dB | 2.0/3 | 43/3 | 10 | off | 1:1 | 0 | 0/124/1 | 1.0 | -inf | -30.1 dB | 113 | pure gate: below -30 dB the gain glides to zero |
| Drum compression | +14.0 dB | off | 0 dB | 0/3 | 0/3 | 1.7 | -22.0 dB | 20:1 | +64 | 21/171/2 | 1.0 | 0 dB | -inf | 25 | limiter attack 0: no lookahead, instant gain; two comp stages |
| Drum compression 3 | +3.2 dB | off | 0 dB | 0.6/2 | 187/1 | 2.1 | -10.0 dB | 1:1 | -100 | 0/81/1 | 1.0 | 0 dB | -inf | 25 | ratio 0 means R = 1: the knee alone does nothing, a 1:1 comp |
| Drum expanding | -9.6 dB | off | 0 dB | 0.6/2 | 63/4 | 0.6 | -22.4 dB | 1:1 | -78 | 0/269/2 | 1.0 | 0 dB | -inf | 25 | ratio -776 gives R = 1.86: above -22 dB the gain rises 0.86 dB per dB |
| Transient shaping | +6.6 dB | off | 0 dB | 0/3 | 43/3 | 10 | -22.0 dB | 1:1 | +100 | 200/319/2 | 1.0 | 0 dB | -inf | 25 | ratio 0 again; positive full knee reshapes nothing without a slope |
| Ducking music behind voice | 0 dB | off | 0 dB | 216/2 | 526/2 | 216 | off | 1:1 | 0 | 0/124/1 | 1.0 | 0 dB | -inf | 25 | a 216 ms lookahead; the sidechain is selected in the mixer, not the preset |

"Drum compression 2" (ratio 684 = 6.2:1, knee +100, comp release 203 ms)
and "For drums" (release curve 6) complete the set. The most common corpus
variation on the default is gain 1000 with a sidechain input of 1 (68
blocks): the limiter used as a ducker.

## 2. Parameter semantics (`ProcessParam` @ 0x006d4490, lookahead `FUN_006d3ee0`, release `FUN_006d4170`)

`REC_FromMIDI` rescales with the wheel's Min/Max and 2^-30 (`0x006d5298`).
`REC_UpdateValue` stores the integer, then recomputes only what that index
touches; the lookahead and release routines take and release the host lock
(host vtable +0x100 / +0x108) around the change. A sample-rate change (the
Dispatcher stores the new rate) recomputes nothing by itself; the Flush that
follows does.

Shared curves, with their start-up constants (`FUN_005e22b0` writes
`0x73c47c = ln 7`, `0x73c474 = ln 3`, `0x73c484 = ln 10 / 20`; `0x719258`
holds 0.001, `0x71924c` 1/6, `0x719240` 1/2, `0x719260` 1e-4):

```
gain(v)   = v == 1000 ? 1 : (7^(v/1000) - 1) / 6          // 0..2000 -> 0 .. 8.0 (+18 dB); 1000 = unity
ceil(v)   = v == 1000 ? 1 : (3^(v/1000) - 1) / 2          // 0..2000 -> 0 .. 4.0 (+12 dB)
t_ms(v)   = (501^(v/10000) - 1) * 2                       // FUN_006d3dc0: 0 .. 1000 ms; 1115 -> 2.0, 2882 -> 10.0, 5000 -> 42.8, 6666 -> 124
samples(v)= round(t_ms(v) * 0.001 * SR)                   // FUN_006d3eb0
coef(t)   = t > 0 ? 0.01 ^ (1 / max(t, 1)) : 0            // FUN_005d9ae0, 0x73c268 = ln 0.01; one-pole retain factor, 1 % left after t samples
```

Per index:

```
 0 gain       0x200 = gain(p0)                                            (asm: also 0x204.. block, see 1/2/15/16)
 1 sat        s = ceil(p1); 0x248 = s < 1
              if on: FUN_005e0360(0x23c, s, min(ceil(p2), 1)):  [0] = s, [1] = max(min(ceil,1) - s, 1.5e-5), [2] = 1/[1]
 2 ceiling    0x204 = ceil(p2); 0x208 = 0x204 * (this+0x277 ? 1.0 : 0.975)      // table 0x0072bc7c
15 noise gain 0x20c = gain(p15)
16 noise thr  0x210 = gain(p16); 0x214 = min(0x208, 0x210)
   (indices 0, 1, 2, 15, 16 all run the same block and recompute every one of the above)
 8 comp thr   0x249 = p8 < 1000 (compressor on)
              thr = on ? max(gain(p8), 0.001) : 100.0          // 0x218; floor 0x73e5d0 = 10^(-60/20)
              0x21c = thr > 0 ? 20 log10 thr : -inf;  0x234 = thr * 100
 9 ratio      a = |p9| / 1000
              p9 <= 0: R = (51^a - 1) * 0.06 + 1               // 1 .. 4: upward expansion
              p9 >  0: R = 1 / ((51^a - 1) * 0.38 + 1)         // 1 .. 1/20: compression, 1000 = 20:1, 500 = 3.3:1
              0x220 = R; 0x224 = R - 1
10 knee       0x238 = sign(p10); K = (201^(|p10|/1000) - 1) * (p10 > 0 ? 0.005 : 0.001)    // 0 .. 1.0 or 0 .. 0.2
              p10 <= 0: 0x228 = K; 0x22c = ln(1 + K); 0x230 = 1 / K
              p10 >  0: 0x228 = K; 0x22c = 1 / ln(1 + K)
 3, 4, 7, 14  FUN_006d3ee0 (lookahead):
              L = samples(p3)                                              -> 0x1fc; delay length L frames (FUN_006cf7e0); host msg 0x1e L (latency) when changed
              peak window W = max(1, L, samples(p7))                      -> TPeakLookahead2.SetLength
              RMS window    = max(1, samples(p14))                        -> TRMSLookahead.SetLength
              attack stages 0x1ec: L < 1 ? none : p4 stages, state 1.0, coefA = coefR = coef(0.8 L / sqrt(p4))
              then FUN_006d4170; silence hold 0x24c = L + round(1.5 SR)
 5, 6, 11, 12, 13, 17  FUN_006d4170 (release):
              t = samples(p5); release stages 0x1dc: t <= 0 ? none : p6 stages, state 0, coefA = 0 (instant), coefR = coef(2 t / sqrt(p6))
              comp stages 0x1e4: p13 stages, state 0, coefA = coef(samples(p11) / sqrt(p13)), coefR = coef(2 samples(p12) / sqrt(p13))
              gate: 0x1f8 = 0.01 ^ (1 / (2 max(samples(p17), 1)));  0x1f4 = 1
```

Constants: `0x006d52c0` 51, `0x006d52c8` 0.38, `0x006d52d0` 0.06,
`0x006d52d8` 201, `0x006d52e0` 0.005, `0x006d52e8` 0.001, `0x006d52b0` 20,
`0x006d52b8` 100; `0x006d4158` 0.8, `0x006d4160` 1.5, `0x006d4478` 2.0,
`0x006d4480` 1.0, `0x006d4488` 0.01; `0x006d3e18` 501, `0x006d3e28` 2.

Display (`FUN_006d5380`): gain, noise gain, noise threshold, comp threshold
through `gain()` in dB; saturation and ceiling through `ceil()` in dB;
attack, peak window, comp attack and RMS window as `t_ms` "ms"; the three
release knobs as `2 * t_ms` "ms" (so the shown release is the time the
stages actually use); ratio as "x : 1" (R < 1) or "1 : x" (R > 1); knee as
`p10 / 1000`; the curve wheels show their integer.

## 3. Signal chain (`Eff_Render` @ 0x006d5d60)

Work buffers: `A` (peak envelope, later the gain), `B` (RMS envelope, later
the comp gain), `C` and `D` (compaction of the frames above threshold), all
`Length` floats, carved from `this+0xd4`.

```
if Source != Dest: ippsCopy_64s(Source, Dest, n)                 // stereo frames as 64-bit units
quiet = every |sample| <= 1e-6 (FUN_005e02a0, 0x006d6570)
countdown 0x250 = quiet ? countdown - n : hold 0x24c
if countdown <= -n:                                              // asleep: flush once (FUN_006d6790), output is the copied input
    return

// 1. peak detector on the undelayed input (TPeakLookahead2.Process, 0x006d09e0)
A[i] = max(|L|, |R|)                                             // FUN_005e0150, SSE abs/max
A[i] = max(A[i - W + 1 .. i])                                    // FUN_006d0820: ring of W plus a monotonic deque, exact sliding maximum

// 2. compressor gain into B
if not comp on (0x249 == 0):
    ippsSet_32f(gain, B, n)                                      // asm 0x006d62e4: the input gain and nothing else
else:
    det = sidechain > 0 and host.GetInBuffer(sidechain) ? sidechain buffer : Dest     // host vtable +0x120
    B[i] = L^2 + R^2 of det                                      // TRMSLookahead.Process 0x006d0d00
    B[i] = sqrt(max(0, boxcar_mean(B, window) / 2))              // FUN_006d0bd0 running sum, ippsThreshold_LT_32f_I(0), ippsSqrt_32f_I
    for each comp stage (p13 of them): B[i] = B[i] + c * (B[i-1]' - B[i]), c = B' <= B ? coefA : coefR      // FUN_005d9bc0, in place, stage after stage
    gather C[k] = B[i], D[k] = i for every i with B[i] > thr (0x218)
    if none: ippsSet_32f(gain, B, n)
    else:
        ippsThreshold_GT_32f_I(C, k, thr * 100)                  // cap at +40 dB over threshold
        ippsLog10_32f_A24(C, B', k); ippsMulC_32f_I(20, B', k); ippsAddC_32f_I(-thr_dB, B', k)     // over_dB
        if knee sign == 0:
            ippsMulC_32f(B', R - 1, C, k)                        // gain_dB = over_dB * (R - 1)
        else:
            ippsAddC_32f(B', 1, C, k)                            // u = over_dB + 1
            if knee > 0: C = ln(1 + K u) / ln(1 + K)             // MulC K, AddC 1, ippsLn_32f_I, MulC 1/ln(1+K)
            else:        C = ((1 + K)^u - 1) / K                 // MulC ln(1+K), ippsExp_32f_I, AddC -1, MulC 1/K
            ippsAddC_32f_I(-1, C, k); ippsMulC_32f_I(R, C, k); ippsSub_32f_I(B', C, k)      // gain_dB = R (C - 1) - over_dB
        ippsMulC_32f(C, ln10/20, B', k); ippsExp_32f_A24(B', C, k)                        // linear gain
        ippsMulC_32f_I(gain, C, k)                               // times the input gain (0x200)
        ippsSet_32f(gain, B, n); B[D[k]] = C[k]                  // frames at or under threshold get the plain input gain

// 3. delay the audio by the lookahead (TBasicDelay_32FS, 0x1d4, function pointer +0x30 = FUN_006cfa60 / FUN_006cfab0)
Dest = Dest delayed by L frames

// 4. limiter release on the peak envelope: instant rise, one-pole fall, p6 stages
for each release stage: A[i] = A[i] + c * (A' - A[i]), c = A' <= A[i] ? 0 : coefR

// 5. gain decision per frame, then the gate
for i in 0..n:
    pk = A[i]; g = B[i]
    if pk * g > ceilSafe (0x208): g = ceilSafe / pk              // the limiter: exact, no overshoot on the detected peak
    if pk < gateThr (0x214): g = noiseGain + (gPrev - noiseGain) * gateCoef      // 0x20c, 0x1f4, 0x1f8
    gPrev = g; A[i] = g

// 6. attack smoothing of the gain: p4 symmetric one-pole stages, time 0.8 L / sqrt(p4) each
for each attack stage: A[i] = A[i] + c * (A' - A[i])

// 7. apply, then the output stage
ippsMul_32f32fc_I(A, Dest, n)                                    // each stereo frame times its gain
if sat on (0x248): FUN_005e03a0 / FUN_005e2130 (Dest, 2n, 0x23c)  // soft knee, below
else:              ippsThreshold_GTAbs_32f_I(Dest, 2n, ceiling 0x204)    // hard clip at the ceiling
```

The input gain never touches the audio directly: it rides in `B`, which the
listing fills with `ippsSet_32f(gain, B, n)` on all three exits of step 2
(`0x006d6243`, `0x006d6291`, `0x006d62e4`), and reaches the samples through
`A` in step 7. That is why the limiter's `pk * g` test sees the boosted
level while `pk` itself is the raw input peak.

Saturator (`FUN_005e03a0` x87, `FUN_005e2130` SSE; `s0` knee start,
`s1 = min(ceil, 1) - s0`, `s2 = 1/s1`):

```
if |x| > s0:  |y| = s0 + s1 * (1 - exp(-(|x| - s0) * s2));  sign kept
```

An exponential knee from the saturation threshold that approaches
`min(ceiling, 1.0)` and never reaches it. With the ceiling above 0 dBFS
("Max loudness (slight saturation)": ceiling +2.5 dB, saturation from
-3.7 dB) the limiter aims at +2.5 dB and the saturator folds that into
0 dBFS.

What the structure means:

- The lookahead delay equals the attack time. The peak detector runs on the
  undelayed input with a window of at least the lookahead, so the gain for an
  output frame already knows the peak that is about to arrive; the attack
  stages (`0.8 L / sqrt(n)` per stage) then fade the gain in over roughly
  the same time. Attack 0 removes the delay and the stages: the limiter
  becomes an instant divider ("Drum compression", "Transient shaping").
- The limiter is exact on its detector: `g = ceiling / peak`, no margin.
  What keeps overshoot away is the window maximum plus the smoothing. The
  default aims at 0.975 of the ceiling (-0.22 dB); the version 7 flag turns
  that margin off.
- The compressor detector is a true RMS over the "sustain" window, in dB,
  with a 40 dB cap on how far over threshold it reads. Ratio values above 0
  compress (20:1 at full), below 0 expand upward (gain rises above the
  threshold, up to 3 dB per dB). "Knee" is not a width: it bends the
  over-threshold curve, logarithmic for positive values (reduction arrives
  fast, then flattens), exponential for negative (reduction starts gently).
  Both curves pass through zero at the threshold, so no gain change below it.
- Every time constant is a chain of `n` one-pole stages, `n` being the
  "curve" wheel, each stage shortened by `sqrt(n)`; more stages give a
  rounder, more Gaussian step response at the same nominal time.
- The gate replaces the gain outright while the peak sits below the gate
  threshold: the gain glides from where it was towards the noise gain with
  the noise release coefficient, and jumps back through the attack stages
  when the signal returns.
- Silence handling: after `L + 1.5 s` of input below 1e-6 the plugin flushes
  and stops processing until the input is back.

Flush (`FUN_006d6790`): attack stage states to 1, release and comp stages to
0, gate gain 1, delay cleared, sleep countdown armed. A Flush does not
recompute coefficients; the UpdateValue loop in restore does.

## 4. What stays unknown

- `Idle_Public`, the visual (`FUN_006d5b10`, `FUN_006d6a40`) and the
  Dispatcher's GUI IDs were not read beyond what the state needs.
- The IPP `_A24` variants (`ippsLog10_32f_A24`, `ippsExp_32f_A24`) are 24-bit
  accurate; a port using libm will differ in the last bits of the comp gain.
- The delay line's two process variants (`FUN_006cfa60`, SSE `FUN_006cfab0`)
  were not decompiled; it is a plain ring of `L` stereo frames by its
  constructor and setter.
- The host sidechain fetch (host vtable +0x120) was not traced into FLEngine.
- `TargetSel` (byte 152) is pushed to a `TQuickCustomTabSelector` at
  form+0x780 through vtable slot 0x2c0; that it is the limiter/comp display
  switch is inferred from the control type and the corpus split (101 / 89).

## Corrections from the implementation (crates/plugins/src/limiter.rs)

- 346 corpus states, not 343.
- "Drum expanding" has ratio 0 and knee −776; a knee at ratio 0 still changes gain.
- The burst sits +0.022 dB over the 0.975 aim, exactly on it with the v7 flag.
