# Transient Processor: state, parameters, signal chain

Recovered from `Transient Processor_x64.dll` (FL Studio 2026, Delphi native
plugin, image base 0x400000) and `engine_x64.dll` beside it (MSVC C++, image
base 0x180000000, 190 kB). Ghidra projects `ghidra/proj/transient` (both DLLs)
and `ghidra/proj/transienteng` (the engine alone), output under
`ghidra/out/transient/`: `vmt/`, `slots/` and `wrap/` for the Delphi side,
`engine_all.c` for every function of the engine ahead of its CRT, `engasm/`
for the listings the maths below was read from. Corpus: 47 uses in 15 of the
165 `.flp`, 10 presets in the plugin's folder and 3 more `.fst` elsewhere in
`Data`. All 60 states are 49 bytes and parse to the last byte.

The Delphi DLL has no DSP. It is a shell: it loads `engine_x64.dll`, calls its
one export `CreateEngine`, and gets back a VST 2 `AEffect` (magic `VstP`, id
`TrSM`, class `TransientProcessorAudioEffect`, 8 parameters, 2 inputs, 4
outputs). Nothing in FLEngine is called for audio.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `asm`, `inferred`.

## How the plugin is wired

`CreatePlugInstance` @ `0x006ebea0` -> constructor `FUN_006ecb60`, class
reference `PTR_PTR_006ebc30` -> VMT `0x006ebcf8` (`TTestPlug`, parent
`TDelphiFruityPlug`).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006ebf00 |
| 3 | SaveRestoreState | 0x006ec7c0 |
| 4 | GetName | 0x006ed2f0 |
| 5 | ProcessEvent | 0x006ecfe0 |
| 6 | ProcessParam | 0x006ed030 |
| 7 | Eff_Render | 0x006edaf0 |

The constructor calls `FUN_006ed430`, which builds the path of
`engine_x64.dll` next to the plugin and hands it with the export name
`CreateEngine` to `TVSTPlugin` (VMT `0x006dc8d8`, constructor `FUN_006de610`),
FL's own VST host class. The shell keeps that object at `this+0x188` and talks
to the engine through it:

| TVSTPlugin slot | address | what it sends the engine |
|---|---|---|
| +0xc8 process | 0x006e16e0 | `processReplacing(inputs, outputs, n)` |
| +0x130 sample rate | 0x006e4360 | `effSetSampleRate` (opcode 10), between `effMainsChanged` 0 and 1 |
| +0xf0 flush | 0x006e0ea0 | `effMainsChanged` 0 then 1 |
| +0x1f0 parameter | 0x006e4190 | `setParameter(index, float)` |
| `FUN_006e3e90` | | `effVendorSpecific` (opcode 0x32) with 1001 |

Shell fields the audio path touches:

| offset | type | meaning |
|---|---|---|
| 0x190 | i32[9] | the parameters, saved |
| 0x1b0 | i32 | parameter 8, the Effect switch (inside the array above) |
| 0x1b4..0x1b6 | u8[3] | graph toggles, saved as one byte |
| 0x1b7 | u8 | graph frozen, saved |
| 0x1b8 | i32 | graph scroll speed, saved |
| 0x150, 0x158 | ptr | the two deinterleaved input buffers |
| 0x160..0x178 | ptr[4] | the engine's four outputs |
| 0x1f0 / 0x1f4 | i32 | frames the engine runs on after sound / frames left |

## 1. State layout (event 213, 49 bytes)

`SaveRestoreState` (`FUN_006ec7c0`):

```
save:    Write(int32 1); Write(this+0x190, 0x24); Write(byte toggles); Write(int32 frozen); Write(int32 speed)
restore: Read(int32 ver); if ver < 2:
           Read(this+0x190, 0x24); Read(byte toggles); Read(int32 frozen & 1); Read(int32 speed)
           push speed and the three toggles to their controls
           for each parameter: ProcessParam(i, GetValue, UpdateValue | 0x10)
           engine.vendorSpecific(1001)        // put every parameter in use at once, no smoothing
```

A version of 2 or more is left unread. The loader checks no range; the value
sent to the engine is clamped to 0..1.

| byte | type | idx | parameter (DFM object) | range | default | corpus and presets | confidence |
|---|---|---|---|---|---|---|---|
| 0..3 | i32 | | version | | 1 | 1 (x60) | loader, corpus, preset |
| 4..7 | i32 | 0 | Attack (`AttackWheel`) | 0..160 | 80 | 38..160 | loader, dfm, corpus, preset |
| 8..11 | i32 | 1 | Drive (`DriveWheel`, Tag 1) | 0..160 | 0 | 0..116 | loader, dfm, corpus, preset |
| 12..15 | i32 | 2 | Release (`ReleaseWheel`, Tag 2) | 0..160 | 80 | 0..160 | loader, dfm, corpus, preset |
| 16..19 | i32 | 3 | Gain (`GainWheel`, Tag 3) | 0..160 | 80 | 0..122 | loader, dfm, corpus, preset |
| 20..23 | i32 | 4 | Attack shape (`AttackShape`, Tag 4) | 0..2 | 1 | 0, 1, 2 | loader, dfm, corpus, preset |
| 24..27 | i32 | 5 | Release shape (`ReleaseShape`, Tag 5) | 0..2 | 1 | 0, 1, 2 | loader, dfm, corpus, preset |
| 28..31 | i32 | 6 | Split frequency (`SplitFreqWheel`, Tag 6) | 0..160 | 80 | 0..93 | loader, dfm, corpus, preset |
| 32..35 | i32 | 7 | Split balance (`MixWheel`, Tag 7) | 0..160 | 80 | 80, 115, 117 | loader, dfm, corpus, preset |
| 36..39 | i32 | 8 | Effect (`BypassBtn`, Tag 8) | 0, 1 | 1 | 1, 0 (x1) | loader, dfm, corpus |
| 40 | u8 | | graph toggles: bit 0 output peaks, bit 1 input peaks, bit 2 transient process | | 7 | 5, 7 | loader, dfm, corpus |
| 41..44 | i32 | | graph frozen (bit 0) | 0, 1 | 0 | 0 | loader, corpus |
| 45..48 | i32 | | graph scroll speed (`SpeedSlider`) | 0..100 | 50 | 50, 74, 100 | loader, dfm, corpus |

Bytes 40..48 are the window's. No sample depends on them.

Preset check (`Data/Patches/Plugin presets/Effects/Transient Processor/`):

| preset | Att | Drive | Rel | Gain | A shape | R shape | Split | Bal | reading |
|---|---|---|---|---|---|---|---|---|---|
| Default | 80 | 0 | 80 | 80 | 1 | 1 | 80 | 80 | both amounts 0, drive 0.1, split 254 Hz: the input, but for the soft clip's 0.3 % at full scale |
| Expander | 80 | 0 | 36 | 80 | 1 | 1 | 80 | 80 | release amount -0.30: tails cut |
| Max attack | 160 | 38 | 80 | 80 | 2 | 1 | 0 | 80 | attack +0.4, Slow, split 5 Hz, drive 1.5 |
| Max release | 80 | 0 | 160 | 80 | 1 | 2 | 0 | 80 | release +0.6, Slow |
| Max transients | 160 | 38 | 160 | 80 | 2 | 2 | 0 | 80 | both |
| Max highs | 160 | 38 | 160 | 80 | 2 | 2 | 80 | 80 | the same above 254 Hz only |
| Overdiven | 120 | 110 | 160 | 53 | 2 | 1 | 50 | 80 | drive 5.9: clip ceiling at -15 dBFS, gain -3 dB |
| Soft attack | 40 | 0 | 80 | 80 | 2 | 1 | 80 | 80 | attack -0.15 |
| Subtle attack | 132 | 0 | 80 | 80 | 0 | 1 | 0 | 80 | attack +0.17, Sharp |
| Subtle release | 80 | 0 | 120 | 80 | 1 | 1 | 0 | 80 | release +0.15 |

## 2. Parameter semantics

### The shell (`ProcessParam` @ 0x006ed030)

Flags per the SDK. `REC_FromMIDI` (0x20) rescales with the wheel's Min/Max and
2^-30 (`0x006ed218`). `REC_UpdateValue` stores the integer at
`this+0x190+4*i` and, for parameters 0 to 7, sends the engine

```
p = clamp((value - wheel.Min) * wheel.scale, 0, 1)        // scale = 1 / (Max - Min), doubles 0.0 @0x006ed220, 1.0 @0x006ed228
engine.setParameter(i, (float) p)
```

Parameter 8 goes nowhere. `Eff_Render` reads it.

### The engine's setParameter (`FUN_180001d00`) and its smoothing

Parameters 4 and 5 are applied at once (`FUN_180002010`). The other six go
into a list of up to 30 `(index, target)` pairs at `engine+0xfc`. Every 8th
frame of `processReplacing` each pair moves:

```
k    = 1 - 0.1 ^ (200 / SR)                       -> engine+0xf8   (a tenth of the way is left after SR/200 steps, 40 ms)
next = now + (target - now) * k                   // now is engine+0x380+4*i
if |next - target| < 1e-4:  apply(i, target); drop the pair
else:                       apply(i, next)
```

`apply` is `FUN_180002010`. `effVendorSpecific(1001)` (`FUN_180001b50`) applies
every target at once, which is how a loaded state skips the smoothing.

### `FUN_180002010(index, p)`, constants from `.rdata` `0x180026e18..0x180026f40`

```
0 Attack:   t = (p - 0.5) + p - 0.5;  s = t * |t|
            attackTo  = s >= 0 ? 0.4 * s : 0.6 * s                  -> shaper+0x30   (-0.6 .. +0.4)
1 Drive:    driveTo   = p < 0.1 ? 0.1 + 9 p : 15 ^ ((p - 0.1) / 0.9)   -> shaper+0x18   (0.1 .. 1 .. 15)
2 Release:  releaseTo = s >= 0 ? 0.6 * s : s                        -> shaper+0x38   (-1 .. +0.6)
3 Gain:     gain      = 10 ^ ((p - 0.5) * 0.9)                      -> engine+0x35c  (-9 .. +9 dB)
4 Attack shape:   aShape = p < 0.333 ? 0.5 : p < 0.666 ? 1 : 2      -> shaper+0x20, then retune
5 Release shape:  rShape = the same                                 -> shaper+0x24, then retune
6 Split:    f = 5 + 995 p^2 Hz;  k = tan(pi f / SR)
            high pass: b0 = 1/(k+1), b1 = -b0, a = (1-k)/(1+k)      -> engine+0x320, 0x324, 0x328
            low pass:  b0 = b1 = k/(k+1), a = (1-k)/(1+k)           -> engine+0x2e0, 0x2e4, 0x2e8
7 Balance:  lowGain = min(1, 2 (1 - p));  highGain = min(1, 2 p)    -> engine+0x354, 0x358
```

The shaper is the object at `engine+0x240`. The split's angle is computed in
single precision as `p^2 * (1/SR) * 3125.8848 + (1/SR) * 15.707964`, which is
`995 pi` and `5 pi`.

Display (`FUN_180001e60`): attack and release `200 p - 100` %, drive `100 p`
%, gain `(p - 0.5) * 18` dB, shapes Sharp, Mid, Slow, split in Hz, balance as
"x% Dry/y% Wet" from the two gains.

### The shaper's paces (`FUN_180001000`, on a new rate or shape)

`keep(T) = 0.1 ^ (1 / (T * SR))`, what a follower keeps a frame to fall 20 dB
in `T` seconds. Doubles 5.0, 0.01, 0.02, 0.1, 14.2857 and float 4.0.

```
fall      = keep(0.2)                      -> shaper+0x74
rise      = 1 - keep(aShape * 0.01)        -> shaper+0x78     (5, 10 or 20 ms)
followMore = keep(rShape * 0.02)           -> shaper+0x80
follow    = 1 - followMore                 -> shaper+0x7c
tailFall  = keep(rShape * 0.1)             -> shaper+0x84     (50, 100 or 200 ms)
heldFall  = keep(4)                        -> shaper+0x8c
            1 - 0.1 ^ (14.2857 / SR)       -> shaper+0x88     (written, never read)
```

## 3. Signal chain

### The shell (`Eff_Render` @ 0x006edaf0)

```
silent = every sample of the block, both sides, has |x| <= 1e-6        // FUN_005e06f0, float 1e-6 @0x006edcd4
left   = silent ? left - n : round(SR * 1.5) + 88                      // this+0x1f4, this+0x1f0 (FUN_006ec6c0)
if left <= -n:      copy the input to the output; left = 0xc0000001    // asleep until a block with sound
elif param 8 == 0:  copy the input to the output
else:               deinterleave; engine.processReplacing; interleave outputs 0 and 1
```

Outputs 2 and 3 of the engine feed the graph (`FUN_006ed660`), as does
parameter 1000, the clip LED.

### The engine (`processReplacing` @ 0x180002610)

It sets FTZ and DAZ for the block. Per frame, after the smoothing step above:

```
// one pole split, per side, state: x1, lowY1, highY1
low  = lb0 * x + lb1 * x1 + la * lowY1
high = hb0 * x + hb1 * x1 + ha * highY1
(shL, shR) = shaper(highL, highR)
outL = (lowGain * lowL + highGain * shL) * gain
outR = (lowGain * lowR + highGain * shR) * gain
```

`low + high` is the input, so with the shaper at unity the split is not heard.

### The shaper (`FUN_1800011c0`)

One gain for both sides, from five followers of `a = (|L| + |R|) / 2`:

```
// attack: a peak that jumps and a follower that takes the attack time
peak = a > peak ? a : peak * fall
slow = a > slow ? slow + (a - slow) * rise : slow * fall
ra = 1
if slow > 0 and (r = peak / slow) >= 1:
    ra = attack < 0 ? r : r > 10 ? 6.6 : r * (0.96 - 0.003 r^2)

// release: a peak held for seconds against a follower of the tail
held = a >= held ? a : held * heldFall
tail = a < tail ? tail * tailFall : a
q    = (release >= 0 and tail > 0) ? (tail + 1e-4 - fol) / (tail + 1e-4) : 1
q    = clamp(3 q, 0, 1)
fol  = fol + (q * followMore + follow) * (tail - fol)
rr = 1
if fol > 1e-6 and (r = held / fol) >= 1:
    rr = release < 0 ? r : r > 20 ? 12 : 0.84 r - 0.0006 r^3

g = (rr - 1) * release + (ra - 1) * attack
g = g >= 0 ? 1 + g : 1 / (1 - g)

// soft clip with its ceiling at 1 / drive
soft(v) = v < -3 ? -1 : v > 3 ? 1 : v (v^2 + 27) / (9 v^2 + 27)
shL = soft(drive * L * g) / drive
shR = soft(drive * R * g) / drive

// the amounts and the drive in use reach their targets a frame late
attack += attackTo - attack;  release += releaseTo - release;  drive += driveTo - drive   (differences taken before the frame)
```

What that means in practice:

- A positive attack amount multiplies the high part by up to `1 + 0.4 * 5.6`,
  10 dB, while the level is rising faster than the 5 to 20 ms follower. A
  negative one divides by `1 + 0.6 (r - 1)` with no cap on `r`.
- At a steady level `r` is 1 and the curve gives 0.957, so a positive attack
  leaves a sustained sound 0.15 dB down at most. A sine's peak stays above its
  follower, so in practice the sustained gain is a little over 1.
- The release compares the loudest level of the last seconds with a follower
  of the current tail. A positive amount lifts what comes after a peak by up
  to `1 + 0.6 * 11`, 17.6 dB. A negative one divides, down to
  `1 / (1 + (r - 1))`: the tail falls at twice its dB rate.
- Drive does not add gain. It scales the soft clip's input and divides its
  output, so it only lowers the ceiling: 20 dB over full scale at 0, full
  scale at a tenth of the wheel, -23.5 dBFS at the top. The clip follows the
  transient gain, so a boosted attack runs into it first.
- The clip sits on the high part only. At the default drive of 0.1 a
  full-scale sample comes out 0.3 % low.

## 4. What stays unknown

- `FUN_180019c20`, `FUN_18001b2c0` and `FUN_18001c190` are taken to be the
  CRT's `pow`, `powf` and `tanf` from how they are called and from the
  display strings agreeing with that reading (18 dB, Hz). Their bodies were
  not read.
- The engine has no code on `suspend` or `resume`: the followers and the
  split's filters live on through FL's flush. What a host should do on a
  transport stop is its own choice.
- The first frame of the engine's life runs with a drive of 0.01, the
  constructor's value, before the wheel's 0.1 or more is in use.
- The graph and the meters (`FUN_006ed660`, `FUN_006edd50`, `FUN_006ede80`)
  were not read past what they take from the audio path.

## Implementation notes (crates/plugins/src/transient_processor.rs)

- The 49 bytes load with the nine parameters range-checked, which FL does not
  do. The window's bytes are dropped.
- `reset` starts the followers and filters over. FL never does (see above).
- FTZ and DAZ are stood in for by zeroing a follower or filter state that has
  gone subnormal.
- No latency.
