# Fruity Peak Controller: state, detector, outputs, and how a link receives them

Recovered from `Fruity Peak Controller_x64.dll` (FL Studio 2026, Delphi native
plugin, image base 0x400000, no engine DLL) and from `FLEngine_x64.dll`
(Ghidra base 0x400000). Ghidra project `ghidra/proj`; decompiles under
`ghidra/out/peakctrl/` (`vmt/` the SDK slots with callees, `tick/` the
`NewTick` method and its helpers) and `ghidra/out/fruityhost2/`,
`ghidra/out/fruityhost3/` (the host's `OnControllerChanged` and the link
fan-out). Corpus: 36 uses in the 165 `.flp`, 3 shipped presets plus the
three template `Fruity Peak Controller.fst`; 47 remote links (event 227)
name a Peak Controller output as their source. Three Ghidra runs on the
plugin, two on FLEngine.

The plugin is an internal controller: its `Eff_Render` only measures the
input (and passes or mutes it), and the work happens in `NewTick`, which FL
calls once per tick. Each tick it decays a peak hold, shapes it, adds a
tempo-locked LFO, and hands three values to the host through
`OnControllerChanged`. The host turns those into REC ids `slot + 0x8000 +
index` and replays every 227 link whose source id matches.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `inferred`, `sdk`.

## How the plugin is wired

`CreatePlugInstance` @ `0x006fc680` -> constructor `FUN_006fcbe0`, class
`TTestPlug`, VMT `0x006fc540` (instance size 0x1c8, parent `TDelphiFruityPlug`).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006fc6d0 |
| 2 | Idle_Public | 0x006f9650 (empty) |
| 3 | SaveRestoreState | 0x006fcab0 |
| 4 | GetName | 0x006fd830 |
| 5 | ProcessEvent | 0x006f8ce0 (base) |
| 6 | ProcessParam | 0x006fd2e0 |
| 7 | Eff_Render | 0x006fda30 |
| 14 | NewTick | 0x006fdbc0 |

Slots 8..13 and 15..19 are the abstract stub `0x0040a1a0`. The SDK order
(`FP_PlugClass.pas`) puts `NewTick` at index 14 ("called before a new tick
is mixed (not played)", enabled by `FPF_WantNewTick`), which is where the
controller logic lives.

Object fields:

| offset | type | meaning |
|---|---|---|
| 0x120 | i32[10] | the parameters, saved |
| 0x148 | f32 | peak hold: max abs sample since the last tick, decayed per tick |
| 0x14c | f32 | decay per tick |
| 0x150 / 0x154 | u32 | LFO phase (2^32 per cycle) / increment per tick |
| 0x158 | ptr | wavetable for the LFO shape (host `WaveTables[]`) |
| 0x160 / 0x164 | f32 | peak / LFO amount, in 2^30 units, divided by the tension normaliser |
| 0x168 / 0x16c | f32 | peak / LFO tension normaliser `T` |
| 0x170 / 0x174 | i32 | sign of the peak / LFO tension |
| 0x178 / 0x180 | i64 | rounded amounts (GUI) |
| 0x188 | i32 | PPQ (ticks per beat) from `FPD_SetTimeSig` |
| 0x18c..0x197 | i32[3] | last sent value per output, -1 after Flush |
| 0x198 / 0x199 | u8 | Mute / Ramp, saved |
| 0x19a | u8 | editor closed (set by `FPD_ShowEditor` with handle 0) |
| 0x19b | u8 | playing (`FPD_SetPlaying` == 1) |
| 0x19c | u8 | `FPD_ProcessMode` flag 0x10 set |
| 0x1a0.. | | meter history ring for the visual (GUI only) |

## 1. State layout (event 213: 42 bytes, one version)

`SaveRestoreState` (`FUN_006fcab0`): save writes `this+0x120, 0x28` then
`this+0x198, 2`; restore reads the same, runs the GetValue/UpdateValue loop
over the ten parameters, and pushes the two bytes to the `MuteBtn` and
`RampBtn` check boxes. No version word; all 42 corpus blocks and the six
presets are 42 bytes.

| byte | type | idx | parameter (DFM object, hint) | range | default | corpus (36) and presets | confidence |
|---|---|---|---|---|---|---|---|
| 0..3 | i32 | 0 | Peak base level (`PeakBaseLvlWheel`), 65536 = 1.0 | 0..65536 | 0 | 0 x27, 26880, 46592, 13568, 32961, 20095, 26368, 14924, 12166, 24576 | loader, dfm, corpus |
| 4..7 | i32 | 1 | Peak amount (`PeakLvlWheel`), 179 = unity | -256..256 | 0 (DFM); 179 in every preset | 179 x15, 256 x9, 0 x5, -96, -158, 185, 157, 136, -234, 211, -30, 94, 95, 38 | loader, dfm, corpus, preset |
| 8..11 | i32 | 2 | Peak tension (`PeakLogWheel`) | -128..128 | 0 | 0 x28, 58, 92 x2, 31, -22, 84, 86, 69, 29 | loader, dfm, corpus |
| 12..15 | i32 | 3 | Peak decay (`FResOfsWheel`, hint "Peak decay speed") | 5..128 | 66 | 66 x11, 128 x13, 5 x2, 29..75 | loader, dfm, corpus, preset |
| 16..19 | i32 | 4 | LFO base level (`LFOBaseLvlWheel`) | 0..65536 | 0 | 0 x26, 65536 x2, 21299, 23552, 25600, 4864, 25805, 17408, 23502 | loader, dfm, corpus |
| 20..23 | i32 | 5 | LFO amount (`LFOLvlWheel`) | -256..256 | 0 | 0 x24, -256 x3, 256 x3, 166, 138, 124, 42, 204, 62 | loader, dfm, corpus, preset |
| 24..27 | i32 | 6 | LFO tension (`LFOLogWheel`) | -128..128 | 0 | 0 x33, 68, 93, 66 | loader, dfm, corpus |
| 28..31 | i32 | 7 | LFO speed (`LFOSpeedWheel`), 32950 = one bar | 200..65536 | 32950 | 32950 x27, 20643 x4, 15087 x2, 10272 x2, 46518, 24109, 24109, 15087 | loader, dfm, corpus, preset |
| 32..35 | i32 | 8 | LFO shape (`LFOShapeSelect`): 0 Sine, 1 Triangle, 2 Square, 3 Saw, 4 Random | 0..4 | 0 | 0 x30, 4 x5, 3 x3, 2 | loader, dfm, corpus, preset |
| 36..39 | i32 | 9 | LFO phase (`LFOPhaseWheel`), 65536 = one cycle | 0..65535 | 0 | 0 x40, 29184, 29184 | loader, dfm, corpus |
| 40 | u8 | | Mute output sound (`MuteBtn`, `this+0x198`) | 0, 1 | 0 | 0 x39, 1 x3 | loader, corpus |
| 41 | u8 | | Ramp controllers (`RampBtn`, `this+0x199`) | 0, 1 | 0 | 0 x40, 1 x2 | loader, corpus, preset |

Preset check (`.fst` files name the plugin "Fruity Peak Controller"):

| preset | Peak base/amt/tension/decay | LFO base/amt/tension/speed/shape/phase | mute/ramp | reading |
|---|---|---|---|---|
| Default | 0 / 179 / 0 / 66 | 0 / 0 / 0 / 32950 / Sine / 0 | 0 / 0 | peak follower at unity gain, 0.66 per beat decay, LFO off |
| Fast LFO | 0 / 0 / 0 / 66 | 0 / 256 / -1 / 10272 / Saw / 0 | 0 / 0 | peak off; a 1/4-beat sawtooth LFO at full amount, slight negative tension |
| Randomness | 0 / 0 / 0 / 66 | 0 / 256 / 0 / 10272 / Random / 0 | 0 / 1 | sample-and-hold noise every 1/4 beat, ramped between values |
| templates (x3) | 0 / 179 / 0 / 66 | 0 / 0 / 0 / 32950 / Sine / 0 | 0 / 0 | the Default |

The corpus favourite outside the default is decay 128 with amount 256 (nine
blocks, all in the Parametric EQ 2 render tests): instant decay, 2x gain, a
raw envelope follower.

## 2. Parameter semantics (`ProcessParam` @ 0x006fd2e0)

`REC_FromMIDI` rescales with the wheel's Min/Max and 2^-30 (`_DAT_006fd538`).
`REC_UpdateValue` stores the integer and recomputes:

```
idx 1, 2   FUN_006fcef0(this, 1, 0): peak amount and tension      (host LockPlugin/UnlockPlugin around it)
idx 5, 6   FUN_006fcef0(this, 5, 1): LFO amount and tension
idx 3      FUN_006fd0e0: decay
idx 7      FUN_006fd180: LFO period
idx 8      0x158 = host->WaveTables[p8 == 4 ? 5 : p8];  FUN_006fd180;  FUN_006fd260 (resync)
idx 9      FUN_006fd260 (resync)
```

Amount and tension (`FUN_006fcef0`, k = 0 for peak, 1 for LFO; constants
`0x740a10 = 1/256`, `0x6fd0a0 = 6`, `0x756178 = {2^32/10, 2^31/10}`,
`0x740a0c = 1/128`, `0x6fd0c0 = 1001`, `0x740a04 = 0.1`):

```
A      = sign(v) * (6^(|v| / 256) - 1)                 // v = p1 or p5; 256 -> 5.0
amt_k  = A * S_k                                       // S_0 = 429496736 (2^32/10), S_1 = 214748368 (2^31/10)
sgn_k  = sign(t)                                       // t = p2 or p6; 0, 1, -1
if sgn_k != 0:
    T_k   = (1001^(|t| / 128) - 1) * 0.1               // 0 .. 100
    amt_k = amt_k / T_k
```

In the controller's unit (2^30 = 1.0) the peak amount is `0.4 A` and the LFO
amount `0.2 A`: the default 179 gives `6^0.699 - 1 = 2.503`, times 0.4 is
1.001, so a 0 dBFS peak maps to 1.0. Full scale (256) is 2.0 for the peak and
1.0 for the LFO.

Decay (`FUN_006fd0e0`; `0x6fd160 = 1001`, `0x6fd170 = 0.0192`):

```
0x14c = (1001^(p3 / 128) - 1) * 0.0192 / PPQ           // per tick; per beat: 66 -> 0.657, 128 -> 19.2 (instant), 5 -> 0.006
```

LFO period (`FUN_006fd180`; `0x6fd238 = 65536`, `0x6fd240 = 1001`,
`0x6fd250 = 48`, `0x6fd254 = 24576`, `0x6fd258 = 1000`):

```
d      = 1001^(p7 / 65536)
period = round((d - 1) * (PPQ >> 2) / 48 * 24576 / 1000)    // ticks: (d - 1) * PPQ * 0.128 = (d - 1) * 0.128 beats
if shape == 4: period <<= 14                                // Random: see below
period = max(period, 3)
0x154  = 2^32 / period                                      // FUN_005b23a0: phase increment per tick
```

32950 gives `d = 32.33`, so 4.01 beats: one bar at 4/4. 65536 gives 128
beats, 200 gives the 3-tick floor. The Random shape multiplies the period by
16384, the table length: the phase then advances one table entry per
`period` ticks, so the 16384-entry noise table is read as sample-and-hold at
the LFO rate.

Resync (`FUN_006fd260`, only while playing, `this+0x19b`):

```
phase 0x150 = round(host.Dispatcher(FHD_GetMixingTime = 0x24, index 4)) * 0x154 + p9 * 0x10000
```

The LFO is locked to the song position in ticks plus the phase knob
(65536 = one cycle). It runs on `FPD_SetPlaying` (0xc), `FPD_SongPosChanged`
(0xd), `FPD_SetEnabled` (0xb) and on a change of shape or phase.

Dispatcher (`FUN_006fc6d0`): `FPD_Flush` (2) zeroes the peak hold and sets
the three last-sent values to -1 so the next tick resends; `FPD_SetTimeSig`
(0xe) stores PPQ (the third int of the time-sig record) and recomputes decay,
period and the meter history; `FPD_ShowEditor` (0) sets 0x19a; `FPD_ProcessMode`
(1) sets 0x19c from flag 0x10; `FPD_GetGUIColor` (0x21) and the 0x33/0x34/0x39
GUI messages do not touch the controller.

Display (`FUN_006fd640`): base levels as `v / 65536`, amounts as the rounded
`amt_k / 2^30` (`0x6fd7c8 = 2^30`), tensions as `v / 128`, decay as `v / max`,
speed through a tick-to-time formatter, shape by name.

## 3. Detector and tick (`Eff_Render` @ 0x006fda30, `NewTick` @ 0x006fdbc0)

```
Eff_Render(Source, Dest, n):
    if peak amount != 0 or editor open:
        (pkL, pkR) = max |sample| per channel over the block        // FUN_005e8810, SSE and-mask + maxps
        0x148 = max(0x148, pkL, pkR)                                // hold, no smoothing
    Mute: Dest = 0                                                 // PTR_DAT_00756a70 zero fill
    else if Source != Dest: copy                                   // PTR_DAT_007579a0

NewTick():
    pk  = shape_0(0x148)                                            // tension curve, below
    peak = round(p0 * 16384 + pk * amt_0)                           // 0x6fe090 = 16384 = 2^30 / 65536: base 65536 -> 1.0
    l   = (wavetable[phase >> 18] + 1) * 0.5                        // 0 .. 1, 0x7409f0 = 0.5
    lfo = round(p4 * 16384 + shape_1(l) * amt_1)
    phase += 0x154
    send(0, clamp(peak + lfo, 0, 2^30))                             // "Peak+LFO"
    send(1, clamp(peak, 0, 2^30))                                   // "Peak"
    send(2, clamp(lfo, 0, 2^30))                                    // "LFO"
    if editor open and not 0x19c: push 0x148 and the two outputs into the meter ring (GUI)
    0x148 = max(0x148 - 0x14c, 0)                                   // decay once per tick

shape_k(x):   sgn_k == 0: x
              sgn_k >  0: T_k - ((T_k + 1)^(1 - x) - 1)            // the division by T_k is folded into amt_k: 1 - ((T+1)^(1-x) - 1)/T
              sgn_k <  0: (T_k + 1)^x - 1                           //                                          ((T+1)^x - 1)/T
```

Both curves pass through (0, 0) and (1, 1); positive tension bows upward
(a small peak gives a large output), negative bows downward.

`send(idx, v)` (`FUN_006fdaf0`): nothing happens while `v` equals the last
sent value (0x18c + 4 idx). Without Ramp the new value is sent as is. With
Ramp the sent value moves at most `0x8000000` (2^27, one eighth of full
scale) per tick towards `v`, so a full-scale jump takes eight ticks. The
value goes to `host->OnControllerChanged(this.tag, idx, value)`, host vtable
slot 19 (`+0x98`); the SDK pas has the method at that index.

`GetName` (`FUN_006fd830`) answers `FPN_OutCtrl` (7) with the strings at
`0x00756180`: index 0 "Peak+LFO", 1 "Peak", 2 "LFO".

## 4. Host side: from `OnControllerChanged` to the link targets (FLEngine)

`TFruityHost` VMT `0x013141b8` (child of `TFruityPlugHost` `0x007b67d0`);
slot 19 is `FUN_0132b9d0`:

```
OnControllerChanged(host, slot, index, value):
    if slot->0xa9 == 0: value = round(value * 16384)             // _DAT_0132ba18: old 16-bit controllers are raised to 2^30 units
    FUN_00d0b7b0(slot->0x9c + 0x8000 + index, value)
```

`slot->0x9c` is the plugin's REC base, the same field rec-225 names for
channels; for a mixer slot it is `0x70000000 + (insert << 22) + (slot << 16)`
(0x20000000-based in files before 24.2.99). So a Peak Controller output has
the REC id of a plugin parameter `0x8000 + index` of its own slot: that is
the `0x20098001`-style source id automation-234 saw in the 227 links. The
corpus agrees: 38 links have source control `0x8001` (Peak) and 9 have
`0x8002` (LFO) on slots holding a Fruity Peak Controller; none use `0x8000`
(Peak+LFO). The same path serves the other internal controllers (Control
Surface `0x8000..0x8018`, X-Y Controller `0x8000/0x8001`, Formula `0x8000`).

The byte `slot+0xa9` is a per-plugin "30-bit controller" flag: the Peak
Controller already clamps to `0x40000000`, so it must have the flag set
(`inferred`: which plugin flag sets it was not read).

`FUN_00d0b7b0(srcId, value)` is the link fan-out:

```
list = PTR_DAT_015e0a00[1]                                       // the 227 link objects (automation-234: 20-byte payload at obj+8), sorted by source id
i = binary search for the first link with obj+8 == srcId        // FUN_00f82660
while list[i].source == srcId:
    target = obj+0x10 (payload @8)
    if target is not in the "touched" list (FUN_00663bf0 < 0):   // a knob the user is holding wins
        v = obj+0 (payload formula, from a 230 event) ? FUN_00f824b0(formula, value) : value
        if payload@12 bit 1 clear: FUN_01046a70(target, v, flags[payload@12 & 1])      // the REC dispatcher
        else:                      FUN_00f8b8e0(target, v, payload@16, flags[...])     // smoothed set, speed from payload@16
        notify the UI (FUN_006667f0(..., 6, target))
    i += 1
```

`flags = DAT_0150fe7c[2] = {0x821, 0xa61}`: REC_UpdateValue (1) |
REC_FromMIDI (0x20, "value is 0..2^30") | REC_InternalCtrl (0x800, the SDK's
"sent by an internal controller" flag), plus REC_Store (0x40) and REC_Init
(0x200) when payload bit 0 is set. The dispatcher scales the 2^30 value onto
the target's raw range (rec-225: `_DAT_0104cd18 = 1/2^30` for flag 0x20) and
writes it. `FUN_00f824b0` maps `value / 2^30` through the compiled 230
formula, clamps to 0..1 and rescales to 2^30. `FUN_00f8b8e0` keeps a per-target
smoothing record (list `DAT_016bacd0`) whose rate comes from `payload@16`
through a `(K^x)` curve and `PTR_DAT_015e4788`; not read further.

So for a model: a Peak Controller in insert `i`, slot `s` drives every 227
link whose u32@0 is `mixerId(i, s) + 0x8001` (Peak) or `+ 0x8002` (LFO) or
`+ 0x8000` (Peak+LFO); the value delivered is `0..2^30`, computed once per
tick by section 3, and lands on the target through the same dispatcher
call an automation clip uses, with REC_InternalCtrl added.

## 5. What stays unknown

- `slot+0xa9` (30-bit controller flag) was not traced to the plugin flag
  that sets it.
- The smoothed-set path (`FUN_00f8b8e0`) and the meaning of `payload@16`
  (default 0x1d5) were only skimmed.
- `FHD_GetMixingTime` with index 4 is taken to return ticks because the
  result is multiplied by the per-tick increment; the index's unit table
  was not read.
- The wavetable contents (host side) were not read; the Random shape's
  table is slot 5 of the SDK array ("noise").
- `FPD` ids 0x34 (returns a parameter kind: 6 for the speed, 9 for the
  shape, 5 otherwise) and 0x39 are newer than the SDK copy and GUI only.

## Corrections

Found while implementing `crates/plugins/src/peak_controller.rs` against the corpus:

- Peak amount 179 gives a gain of 1.0001, not 1.001.
- LFO speed 32950 gives `d = 32.25`, 384 ticks at 96 PPQ, which is one bar at 4/4.
- Shipped projects have Mute set twice and Ramp never.
- The "Fast LFO" preset is base 65536, amount -256 and tension 66, not the row given above.
