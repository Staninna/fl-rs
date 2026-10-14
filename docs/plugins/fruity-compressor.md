# Fruity Compressor: state, parameters, signal chain

Recovered from `Fruity Compressor_x64.dll` (FL Studio 2026, Delphi native
plugin, image base 0x400000). Ghidra project `ghidra/proj`, decompiles under
`ghidra/out/comp/` (`vmt/` holds `CreatePlugInstance` and the SDK slots with
callees). Corpus: 88 uses in the 165 `.flp`, 20 shipped presets (`Internal/`
and `External/` folders). Two Ghidra runs; the one helper the callee walk
missed (`FUN_006be900`, the time-constant routine) was read from `objdump`.
Nothing in FLEngine is called for audio; the host supplies the sample rate and,
when selected, the sidechain buffer.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `inferred`, `asm`.

## How the plugin is wired

`CreatePlugInstance` @ `0x006bd710` -> constructor `FUN_006bdbe0`, class
reference `PTR_PTR_006bd530` -> VMT `0x006bd5f8` (`TTestPlug`).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006bd760 |
| 2 | Idle_Public | 0x006bb060 |
| 3 | SaveRestoreState | 0x006bd9a0 |
| 4 | GetName | 0x006be6d0 |
| 5 | ProcessEvent | 0x006ba430 |
| 6 | ProcessParam | 0x006bde70 |
| 7 | Eff_Render | 0x006bee30 |

Object fields (the ones the audio path touches):

| offset | type | meaning |
|---|---|---|
| 0x120 | u8 | "report latency" flag, saved |
| 0x124 | i32[6] | the parameters, saved state |
| 0x140 | u8 | coefficients need recomputing |
| 0x144..0x150 | f32[4] | DC blocker: allpass state L, last input L, same for R |
| 0x154 | f32 | DC blocker allpass coefficient |
| 0x158 / 0x159 | u8 | always 1 / "vintage curve" (type mod 4 == 2) |
| 0x15c | f32 | detector envelope |
| 0x160 | f32 | smoothed gain |
| 0x164 | f32 | slow follower of the gain, drives the adaptive release |
| 0x168 | f32 | effective release coefficient |
| 0x16c / 0x170 | f32 | detector coefficients for 0.01 ms and 1 ms |
| 0x174 / 0x178 | f32 | attack and release coefficients |
| 0x17c / 0x180 / 0x184 | f32 | release coefficients: fast variant, nominal, nominal |
| 0x188 | f32 | threshold, linear |
| 0x18c | f32 | `1/R - 1`, the slope above the knee (modified for vintage) |
| 0x190 | f32 | make-up gain, linear |
| 0x194 | f32 | knee half-width as a linear factor `10^(-K/20)` |
| 0x198 / 0x19c / 0x1a0 | f32 | `ln thr`, knee lower bound, knee upper bound (natural log) |
| 0x1a4 | f32 | `0.5 / (upper - lower)` |
| 0x1a8 | f32 | vintage: gain at the knee top |
| 0x1ac | f32 | limiter gain |
| 0x1b4 / 0x1b8 | i32 | lookahead 64 frames / ring length 128 |
| 0x1bc | i32 | limiter hold counter |
| 0x1c0 / 0x1c4 | i32 | ring indices: audio (step 2), gain (step 1) |
| 0x1c8 | f32 | gain applied this sample |
| 0x1cc | f32 | limiter release coefficient (accelerating) |
| 0x1d4 | f32[128] | limiter gain ring |
| 0x3d4 | f32[128] | audio ring, 64 stereo frames |
| 0x5d8 | i32 | sidechain input number, saved |

## 1. State layout (event 213, 36 bytes; 32 and 28 in older files)

`SaveRestoreState` (`FUN_006bd9a0`) is versioned:

```
save:    Write(int32 3); Write(this+0x124, 0x18); Write(int32 latencyflag); Write(int32 sidechain)
restore: Read(int32 ver); if ver < 4:
           Read(this+0x124, 0x18); GetValue/UpdateValue loop
           f = 0; Read(int32 f); latencyflag = f & 1          // read happens for every version; a 28-byte block leaves f = 0
           sidechain = 0; if ver > 2: Read(int32 sidechain)
           push sidechain to its selector (form+0x7c0, clamped to the host's send count, FUN_006bf940)
```

The corpus has all three shapes: 2 uses of 28 bytes (version 1), 51 of 32
(version 2), 35 of 36 (version 3). Every shipped preset is version 2. A version
>= 4 refuses to load.

| byte | type | idx | parameter (DFM object, hint) | range | default | corpus | confidence |
|---|---|---|---|---|---|---|---|
| 0..3 | i32 | | version 1, 2 or 3 | | 3 | 1 (x2), 2 (x51), 3 (x35) | loader, corpus, preset |
| 4..7 | i32 | 0 | Threshold (`ThresholdWheel`), 0.1 dB | -600..0 | 0 | -402..0, -130 (x15) | loader, dfm, corpus, preset |
| 8..11 | i32 | 1 | Ratio (`MinDepthWheel`, Tag 1), 0.1 : 1 | 4..300 | 10 | 10..252, 100 (x15) | loader, dfm, corpus, preset |
| 12..15 | i32 | 2 | Gain (`GainWheel`, Tag 2), 0.1 dB | -300..300 | 0 | -70..252 | loader, dfm, corpus, preset |
| 16..19 | i32 | 3 | Attack (`AttackWheel`, Tag 3), 0.1 ms | 0..4000 | 150 | 0..1625, 150 (x20) | loader, dfm, corpus, preset |
| 20..23 | i32 | 4 | Release (`ReleaseWheel`, Tag 4), 1 ms | 1..4000 | 200 | 22..4000, 200 (x20) | loader, dfm, corpus, preset |
| 24..27 | i32 | 5 | Type (`TypeWheel`, Tag 5) | 0..7 | 0 | 0 (x39), 2 (x18), 4 (x17), 3, 5, 6 | loader, dfm, corpus, preset |
| 28..31 | i32 | | report latency (bit 0), version >= 2 | 0, 1 | 1 | 1 (x71), 0 (x15) | loader, corpus, preset |
| 32..35 | i32 | | sidechain input number, version 3 | >= 0 | 0 | 0 (x35) | loader, corpus |

The type names (`GetName` section 1, pointer table at `0x0070e3b0`): 0 Hard,
1 Medium, 2 Vintage, 3 Soft, 4 Hard/R, 5 Medium/R, 6 Vintage/R, 7 Soft/R. The
"/R" four are the same curves with the program-dependent release of section 3.

Preset check (`.fst` files name the plugin "Fruity Compressor"). Derived
columns use section 2; "knee" is the width from table `0x0070e3f0`.

| preset | Thr | Ratio | Gain | Att | Rel | Type | reading |
|---|---|---|---|---|---|---|---|
| Default | 0.0 dB | 1.0:1 | 0 dB | 15 ms | 200 ms | Hard, knee 0 | unity: a ratio of 1 gives `1/R - 1 = 0`, so no gain change at all |
| Fast Master | -3.9 dB | 1.8:1 | +1.9 dB | 16 ms | 54 ms | Hard | gentle, quick |
| Master 1 | -8.4 dB | 3.2:1 | +3.4 dB | 10.8 ms | 117 ms | Soft/R, knee 15 dB | wide knee from -15.9 to -0.9 dB, auto release |
| Master V | -22.0 dB | 4.9:1 | +7.5 dB | 18.1 ms | 262 ms | Vintage/R | knee -29 .. -5.8 dB, gain reduction capped at 12.9 dB |
| Vintage | -15.0 dB | 8.0:1 | +8.6 dB | 29.2 ms | 1532 ms | Vintage, knee 7 | S-curve up to +18.3 dB above threshold, then flat; max 16.0 dB reduction |
| V Driver | -40.2 dB | 18.2:1 | +25.2 dB | 36.5 ms | 4000 ms | Vintage | everything above -40 dB is squashed up to 21.2 dB, then +25 dB back: a sustainer |
| Brickwall | -13.0 dB | 10.0:1 | +13.0 dB | 0 ms | 800 ms | Hard/R | attack 0 is instant (coefficient 0); the built-in limiter at -0.1 dBFS does the rest |
| Drums | -12.5 dB | 7.5:1 | +9.0 dB | 10 ms | 200 ms | Medium/R, knee 6 | |

The most common corpus block, `[-130, 100, 130, 0, 800, 4]` (x15), is
"Brickwall" with the latency flag cleared.

## 2. Parameter semantics (`ProcessParam` @ 0x006bde70, recompute `FUN_006be980`)

Flags per the SDK; `REC_FromMIDI` rescales with the wheel's Min/Max and 2^-30
at `0x006be048`. `REC_UpdateValue` stores the integer and sets `this+0x140`;
the normalised value it also computes is unused. The coefficients are rebuilt
at the top of the next `Eff_Render` when the flag is set, or after a Flush.
Nothing recomputes on a sample-rate change alone (`Dispatcher` ID 4 only
stores the rate in the host structure); the flag is set again by the Flush
that follows.

Time constants (`FUN_006be900`, `asm`: float 1000.0 @`0x006be96c`, 1.0
@`0x006be970`, double 0.1 @`0x006be978`):

```
coef(t_ms) = t == 0 ? 0 : 0.1 ^ (1000 / (t_ms * SR))        // per-sample retain factor; decays to 10 % in t_ms
```

Recompute (constants: float 0.01 @`0x006bedd8`, 1.0 @`0x006beddc`, doubles
10.0 @`0x006bede0`, 0.1 @`0x006bede8`, 200.0 @`0x006bedf0`, 20.0
@`0x006bedf8`, 1.0, -1.0, 0.5, 0.25, 1.25, 3 pi = 9.42477796 @`0x006bee28`):

```
K      = [0, 6, 7, 15][type mod 4]                   // knee width in dB, table 0x0070e3f0
vint   = (type mod 4 == 2)
cDet0  = coef(0.01)                                  -> 0x16c    (0.0054 at 44.1 kHz)
cDet1  = coef(1.0)                                   -> 0x170    (0.949)
cAtt   = coef(attack / 10)                           -> 0x174
cRel   = coef(release)                               -> 0x178, 0x180, 0x184
cRelF  = type < 4 ? coef(release) : coef(release / 10)   -> 0x17c
thr    = 10 ^ (thr10 / 200)                          -> 0x188    (0.1 dB units -> linear)
slope  = 1 / (ratio10 / 10) - 1                      -> 0x18c    (0 at 1:1, -0.9 at 10:1)
makeup = 10 ^ (gain10 / 200)                         -> 0x190
k      = 10 ^ (-K / 20)                              -> 0x194
lt     = ln thr;  kl = ln(thr * k);  ku = ln(thr / k)    -> 0x198, 0x19c, 0x1a0   (knee is lt +- K dB)
half   = 0.5 / (ku - kl)                             -> 0x1a4
if vint:
    ku    = lt + 1.25 * ratio ^ 0.25                 // knee top in nepers: 1.25 * R^0.25 (R = 4 -> 15.4 dB above threshold)
    gTop  = exp((ku - lt) * slope)                   -> 0x1a8   (the deepest gain reduction this type will do)
    ku    = ku + K dB                                -> 0x1a0   (ln(exp(ku) / k): 7 dB wider)
    slope = ln gTop / (ku - lt)                      -> 0x18c   (so the curve still reaches gTop at the new top)
dc.c   = (tan(3 pi / SR) - 1) / (tan(3 pi / SR) + 1)      -> 0x154   (first-order allpass for a 3 Hz high pass)
lim    = 1.0; lookahead = 64; ring = 128; hold = 0; limRel = 0.01
```

Display (`FUN_006be220`): threshold, gain `v/10` with "dB"; ratio `v/10` as
"x.x : 1"; attack `v/10` "ms"; release `v` "ms"; type by name.

## 3. Signal chain (`Eff_Render` @ 0x006bee30)

Per block: recompute if dirty; if a sidechain input is selected, fetch its
buffer from the host (`TFruityPlugHost` slot 36, vtable+0x120, with the input
number) and detect from that instead of the processed input. Then per frame,
with `eps = 1e-30` (`0x006bf8c8`):

```
xL = L + eps; xR = R + eps
// 1. DC blocker, 3 Hz, on the signal path (first-order allpass, HP = (x - ap) / 2)
apL = (xL - apL) * dc.c + xprevL;  xprevL = xL;  hpL = (xL - apL) * 0.5         // 0x144, 0x148
apR = ...                                                                        // 0x14c, 0x150
dL, dR = sidechain ? (scL + eps, scR + eps) : (hpL, hpR)

// 2. peak detector: 0.01 ms attack, 1 ms release
d   = max(|dL|, |dR|) + eps
env = d + (env - d) * (d <= env ? cDet1 : cDet0)                                // 0x15c
x   = ln env

// 3. gain computer in the natural-log domain
g = 1
if x >= kl:
    if not vint:
        if x < ku:  g = exp(slope * (x - kl)^2 * half)        // quadratic knee: dB = slope * (x - kl)^2 / (2 (ku - kl))
        else:       g = exp((x - lt) * slope)                 // above the knee: (env / thr) ^ (1/R - 1)
    else:
        if x < ku:  t = (x - kl) / (ku - kl);  w = min(1, lt - kl)             // w = 7 dB in nepers = 0.806
                    g = exp((ku - lt) * slope * (t + (t*t*(3 - 2t) - t) * w))  // blend of a line and a smoothstep
        else:       g = gTop                                  // flat: no further reduction above the knee top

// 4. gain smoothing with program-dependent release
gs    = g + (gs - g) * (g <= gs ? cAtt : cRelEff)                              // 0x160
slow  = max(gs, gs + (slow - gs) * cRel)                                       // 0x164: lags above gs at release speed
dev   = vint ? (0.707 - gs) * 1.414 : slow - gs
cRelEff = max(0, cRel + (cRelF - cRel) * dev)                                  // 0x168: types 0..3 have cRelF == cRel, so constant
gs   *= makeup
yL = hpL * gs;  yR = hpR * gs

// 5. lookahead limiter at -0.1 dBFS (0.9885531 @0x006bf910), 64 frames, smoothstep attack
m = max(|yL|, |yR|)
if m <= 1:
    if hold == 0: limRel += (0.09 - limRel) * 5e-5;  lim += (1 - lim) * limRel
else:
    need = 0.9885531 / m
    if lim <= need:
        if hold == 0: limRel += (0.09 - limRel) * 5e-5;  lim += (need - lim) * limRel
    else:
        prev = gApplied;  lim = need
        if hold == 0: fill gainRing[0..127] with gApplied
        hold = 64;  limRel = 0
        for i in 0..127:                                                       // ramp written ahead of the audio
            t = min(1, (i + 1) / 64)
            r = prev + (need - prev) * t*t*(3 - 2t)
            gainRing[(gIdx + i) mod 128] = min(gainRing[...], r)
if hold > 0: hold -= 1
gApplied = gainRing[gIdx];  gainRing[gIdx] = lim                               // 0x1c8
Dest = audioRing[aIdx] * gApplied;  audioRing[aIdx] = (yL, yR)                 // 64-frame delay, 0x3d4
aIdx = (aIdx + 2) mod 128;  gIdx = (gIdx + 1) mod 128
```

What that means in practice:

- The detector is a near-instant peak follower (10 us attack, 1 ms release).
  The attack and release knobs act on the gain, not on the level estimate, and
  a gain *decrease* uses the attack coefficient, an increase the release one.
- "Hard" has no knee (K = 0 makes `kl == ku`, so only the above-knee branch
  runs). Medium and Soft are textbook quadratic knees of 12 and 30 dB total
  width. Vintage is a different animal: an S-shaped curve from 7 dB below the
  threshold to `1.25 * R^0.25` nepers plus 7 dB above it, after which the gain
  reduction stops growing. With the "Vintage" preset (8:1, -15 dB) the maximum
  reduction is 16 dB; loud peaks pass through with the same attenuation as
  moderate ones, which is the opto-ish character the name promises.
- The four "/R" types scale the release towards ten times faster right after a
  large, sudden gain reduction (`slow - gs` is large), settling to the nominal
  release as `slow` catches up. For Vintage/R the acceleration instead tracks
  how far below -3 dB the current gain sits.
- The output always runs through a 64-frame lookahead limiter with a -0.1 dBFS
  ceiling, a 64-sample smoothstep ramp into each new peak, a 64-sample hold,
  and an accelerating recovery (the coefficient itself ramps from 0 to 0.09
  with a 20000-sample time constant). The audio is therefore always delayed
  64 frames; the flag at `this+0x120` only decides whether the host is told
  (host message 0x1e with 64 or 0, table `0x0070e378`). Attack 0 plus this
  limiter is how "Brickwall" works.
- A 3 Hz first-order high pass sits on the signal path, not only on the
  detector. It is inaudible but it is there, and a bit-exact port needs it.

Flush (`Dispatcher` ID 2, `FUN_006be800`) zeroes the rings, sets `gs = 1`,
`env = 0`, `lim = 1`, `cRelEff = 0.5`, puts `5e-31` in the DC blocker states,
and marks the coefficients dirty.

Re-implementation note: port the per-frame block above as written, with
`coef()` evaluated for the project rate. Keep the ln-domain knee (it is not the
usual dB-domain formula scaled; the constants 1.25 and 0.707/1.414 live in
nepers and linear gain respectively), and keep the limiter, which is not
optional.

## 4. What stays unknown

- The peak meters: `Eff_Render` tracks the per-channel maximum of the detector
  input but the decompile shows no store; display only.
- `Dispatcher` ID 5 (window min/max), 0x19 (send count changed: re-clamps the
  sidechain selector) and 0x39 are GUI.
- The host service at vtable+0x120 that returns the sidechain buffer was not
  read; it is the FL SDK "get input buffer" call.
- Whether anything resets the coefficients on a sample-rate change without a
  Flush. The code path does not.

## Implementation notes (crates/plugins/src/compressor.rs)

- 20 presets ship, not 13. Flush fills the gain ring with 1.0, not 0; recompute sets the slow follower to 1. The preset table's Vintage knee tops omit the 7 dB.
- Output is always 64 frames late (the lookahead limiter); the host must compensate.
