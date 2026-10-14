# Fruity Love Philter: state, slots, articulators, filters

Recovered from `Fruity Love Philter_x64.dll` (FL Studio 2026, Delphi native
plugin, image base 0x400000; everything, including the filter kernels and
the envelope classes, is compiled into this DLL). Ghidra project
`ghidra/proj`; decompiles under `ghidra/out/lovephilter/` (`vmt/` the SDK
slots with callees, `art/` the stream formats and slot recompute, `dsp/` the
coefficient function, routing and the scalar kernels, `dsp2/` the SSE
kernels, `art2/` the articulator evaluation). Corpus: 50 uses in the 165
`.flp`, 52 shipped presets (49 plus the three template
`Fruity Love Philter.fst`). Six Ghidra runs on the plugin (import, slots,
stream formats, DSP, articulators, SSE kernels), one over the budget because
the kernels sit behind CPU-dispatch pointers whose scalar candidates turned
out to be "Not implemented in i64" stubs.

The plugin is eight filter units in a chain, each with a filter of one of
twelve types, a waveshaper and a pan/volume stage, and each of its eight
automatable targets driven by an articulator: ten pattern envelopes, an LFO
and three mapping curves (X, Y, envelope follower). The state is a zlib
stream of about 60 KB because every one of the 8 x 8 articulators serialises
its 14 envelopes.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `inferred`.

## How the plugin is wired

`CreatePlugInstance` @ `0x00853f50` -> constructor `FUN_00856780`, class
`TTestPlug`, VMT `0x00853df8` (instance size 0x1780).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x008545c0 |
| 2 | Idle_Public | 0x007794e0 (empty) |
| 3 | SaveRestoreState | 0x008551e0 |
| 4 | GetName | 0x00858680 |
| 5 | ProcessEvent | 0x00857990 |
| 6 | ProcessParam | 0x008579e0 |
| 7 | Eff_Render | 0x0085aa10 |

Classes: `TOpArticulator` VMT `0x00853b18` (size 0x180, parent
`TArticulator` `0x0077fe88`), `TMEnvelope` `0x0077f9a8`, `TMLFO` `0x0077fb48`
(both over `TCustomMEnvelope` `0x0077f7c0`), `TFFTFilter` `0x006182b8` (the
EQ in the editor, not in the audio path).

Plugin object:

| offset | type | meaning |
|---|---|---|
| 0x128 | i32[6] | global params: master level, master LFO level, IEF level, IEF attack, IEF release, pattern count (1..10) |
| 0x140 | i32 | Modulation X (0..1024) |
| 0x144 | i32 | Modulation Y |
| 0x148 | i32 | unused (0 in every state) |
| 0x14c | f32 | X/Y smoothing coefficient, 0.9685 (0.9684 in four states) |
| 0x150 | i32 | soloed slot, -1 none |
| 0x154 | i32 | oversampling selector index (`DQOverSelect`), with 0x158 |
| 0x158 | i32 | oversampling factor (1, 2, 4) |
| 0x15c | u8 | Smooth up modulation (`ModSmoothBtn`) |
| 0x15d | u8 | Global DC offset removal (`GlobalCenterBtn`) |
| 0x15e | u8 | pattern step flag (passed to `FUN_0085cc80` with the pattern count) |
| 0x15f | u8 | editor window size flag (GUI) |
| 0x160 | u8 | (v10+) MIDI note input enabled: `FPD_MIDIIn` is only read when set |
| 0x161 | u8 | (v12+) HQ waveshaper also when not rendering |
| 0x164, 0x16c | i32 | GUI tab selections |
| 0x174 + 0x2a0 s | | slot s, s = 0..7 (table below) |
| 0x1674 / 0x167c | ptr | up- and down-sampler for the oversampling factor 0x1688 |
| 0x168c | i32 | oversampling multiplier applied to block lengths |
| 0x16a0 / 0x169c | i32 | control tick length (64 samples, 8 with "HQ envelopes") / countdown |
| 0x16ac | f32 | IEF level gain; 0x16b0 IEF state; 0x16b4 IEF peak this tick; 0x16b8 IEF source (0 off, 1 input, 2 unit 1); 0x16bc f32[2] IEF attack/release coefficients |
| 0x16c8 / 0x16cc | f32 | master gain, master LFO gain (signed) |
| 0x16d2 | u8 | rendering (FPD_ProcessMode flag 2) |
| 0x1748 / 0x174c / 0x1750 | i32 | X, Y, IEF table indices 0..1024 |
| 0x1760 | array | all 64 articulators; 0x1768 the enabled slots in order |
| 0x1770 / 0x1774 | i32 | `SR / 5` silence hold; countdown |

Slot (0x2a0 bytes at `0x174 + 0x2a0 s`):

| offset | type | meaning |
|---|---|---|
| +0x00 | i32[12] | parameters (section 1); slot 7 has 11 (no "to next") |
| +0x30 | u8 | filter mode selector (`FOrderSelect`): 0, 1 (default), 2, 3, 5 seen |
| +0x31..0x33 | u8[3] | 0 in every state |
| +0x34 | i32 | filter type 0..11 |
| +0x38 | u8 | Alternate resonance / bandwidth (`FFlatBtn`, "ALT") |
| +0x39 | u8 | unit enabled (`EnableBank`); slot 0 on by default |
| +0x3a | u8 | waveshaper enabled (`WSEnableBtn`) |
| +0x3c / +0x40 | f32 | input gain (from p10) / gain from the previous unit (from its p11) |
| +0x44 / +0x48 | f32 | pan (p8/128) / volume gain |
| +0x4c..+0x6b | f32[8] | output matrix targets and ramp states |
| +0x6c | array | 8 `TOpArticulator`: 0 panning, 1 volume, 2 cutoff, 3 resonance, 4 low/gain/drive, 5 band, 6 high, 7 waveshaper mix |
| +0x74 / +0x78 | f32 | cutoff p1/256, resonance p2/256 |
| +0x88..+0x90 | f32[3] | low/band/high gains `level(p3..p5 / 128)` |
| +0x94 | ptr | waveshaper curve (`TCustomMEnvelope`, kind 3); +0x9c the i16 lookup table, +0xa4 its last index, +0xa8 mix p7/256, +0xac pre-amp p6/256 |
| +0xb0 | u8 | this unit feeds the envelope follower (unit 1 with IEF source 2) |
| +0xb4 | i32 | stages - 1 (from the mode selector) |
| +0xb8 | i32 | stage detune (0 normally) |
| +0xbc | f32 | drive for Mango |
| +0xc4 | f32 | rate scale for the cutoff maths (set with the oversampling factor; not read, see section 3) |
| +0xc8 | stage[] | filter stages, 0x78 bytes each: biquad coefficients at +0x50 b0, +0x58 b1, +0x60 b2, +0x68 a1, +0x70 a2; state at +0x10; SVF/Lime/Mango coefficients at +4, +8, +0xc |
| +0x238 / +0x23c | f32 | modulated cutoff / resonance for this tick |
| +0x240..+0x248 | f32[3] | modulated L/B/H gains (ramped at +0x250/+0x270) |
| +0x24c | f32 | modulated waveshaper mix (ramped at +0x26c/+0x28c) |
| +0x290 / +0x294 / +0x298 | i32, i32, ptr | first REC index of the slot, number of slot params (12, 11 for slot 7), control table |

## 1. State layout (event 213: `i32 version` + zlib stream)

`SaveRestoreState` (`FUN_008551e0`):

```
save:    Write(i32 0x000c0003)                        // 12 in the high word; the low word is the stream codec (zlib)
         zlib stream (FUN_0084e3d0 level 2) of the body below
restore: Read(i32 v); refuse v == 0x7fffffff or v >= 0xc0004; if v >= 0x10000: v >>= 16
         inflate (FUN_0084e820) and read the body
```

Corpus versions: 12 (`0xc0003`, 31 projects), 11, 10 (`0xa0003`, 10 projects,
FL 20.8 era), 6 (the templates), 5 (12 presets and 7 projects), 2 (37
presets). All 102 blocks parse to the last byte with the layout below
(`/tmp/p4/lpparse3.py`, throwaway).

Body:

| field | size | content | corpus | confidence |
|---|---|---|---|---|
| globals | 6 x i32 | master level 0..160 (128), master LFO level -128..128 (128), IEF level 0..160 (128), IEF attack 0..16384 (4096), IEF release (8192), pattern count 1..10 (1) | defaults in 77 of 102; master 64..118 in the rest, one LFO level 0 | loader, dfm, corpus |
| block 0x140 | 0x1f (v < 10), 0x20 (v 10), 0x21 (v >= 11) | i32 ModX (512), i32 ModY (512), i32 0, f32 0.9685, i32 solo (-1), i32 oversampling index (0), i32 oversampling factor (1), u8 smooth (1), u8 center (0), u8 step (1); v10+: u8 MIDI (1); v11+: u8 HQ WS (1) | ModX 0..1024 varies; solo always -1; factor 1 (0, 2, 4 unseen); `01 00 01 01 01` in 13 of the 31 v12 | loader, corpus |
| checkbox + wheel | u8 + i32 | `IEFUseF1Btn` state (form+0x768) and the `HeartWheel` value (form+0x750) (`inferred` from the form slots) | 0 / 0 in 98, 1 / 0 in 3, 0 / 128 in 1 | loader, corpus |
| comment | v < 6: i32 len + bytes; v >= 6: u8 len (0xff + u32 when >= 255) + UTF-16 chars | the info panel RTF / text | 249-byte RTF in old presets, "Preset name\rPreset details" in templates | loader, corpus |
| slot s (x8) | | see below | | loader |
| v >= 12: u8 | 1 | `this+0x161` HQ waveshaper | 1 | loader, corpus |

Slot body:

```
i32[12]  params (idx: 0 env amount, 1 cutoff, 2 resonance, 3 low/gain/drive, 4 band, 5 high, 6 WS pre-amp, 7 WS mix, 8 pan, 9 volume, 10 input volume, 11 to next)
u8[11]   +0x30: mode, 0, 0, 0; i32 type; u8 alt; u8 enabled; u8 WS enabled
env      waveshaper curve: kind 3 block (below), no trailer
u8       waveshaper bipolar flag (FUN_00780090)
8 x      TOpArticulator:
           i32 1 (format); f32 base, f32 amount, f32 LFO amount        // (0, 1, 1) for panning, cutoff, low; (1, 1, 1) for the multiplicative ones
           10 x pattern envelope: kind 1 block + 16-byte trailer (128, 128, 0, 128)
           1 x LFO: kind 2 block + 16-byte trailer (speed 40000, 0, 0, 0) + i32 (block version >= 3)
           3 x mapping: kind 3 block, no trailer (X, Y, IEF curves; default (0, 0.5) (1, 0.5))
```

Defaults: slot params `128 128 128 128 0 0 128 128 0 128 128 0` in every
slot (env amount, cutoff, resonance and low at 128, band and high at 0, WS
pre-amp and mix 128, pan 0, volume 128, input 128, to next 0); flags
`01 00 00 00 | 00 00 00 00 | 00 | 01 | 00` for slot 0 (mode 1, type 0,
enabled), the same with the enabled byte 0 for the others. Corpus types in
use: 0 (17 presets on unit 1), 1 (12), 3 (7), 2, 4, 6, 7, 9, 11; mode values
0, 1, 2, 3, 5. Pattern envelopes carry one point unless the preset draws them
(9, 13, 19, 30, 33 points in the sequenced presets); the LFO block is one
point with the speed word 40000 (26259, 21502, 512, 35989, 40898 when
edited) and the active flag -1 in 11 presets.

Envelope block (`FUN_007800e0` base; the kinds here are 1 pattern, 2 LFO, 3
mapping):

```
u32 kind; u32 version (3; 1 and 2 in old states); u32 n
n points: version >= 2: f64 x, f64 y, f32 tension, u8 mode, u8, u8, i8; version < 2: f32 x, f32 y, f32 tension, u8 mode, u8, u8, i8
u32; i32 active (0 or -1); i32; i32; i32            // obj+0x44: the -1 at @4 switches the envelope on
TMEnvelope (kind 1) adds 16 bytes (obj+0x98); TMLFO (kind 2) adds 16 bytes (obj+0xd0: speed, ...) and an i32 (obj+0xe0) when version >= 3
```

The point record, the modes and the version gates are automation-234's.

Preset check (`.fst` files name the plugin "Fruity Love Philter"; unit 1
params, through section 2):

| preset | v | units on: type / mode | unit 1: env / cut / res / L B H / WS pre / mix / pan / vol / in / next | reading |
|---|---|---|---|---|
| Default | 2 | 1: Chocolate / 1 | 128 / 128 / 128 / 128 0 0 / 128 / 128 / 0 / 128 / 128 / 0 | one state-variable unit, low output only, cutoff half way, no modulation |
| Simple lowpass | 2 | 1: Lime LP / 1 | defaults | the same with the Lime filter |
| Basic 2 | 2 | 1: Chocolate / 0; 2: Chocolate / 0 | defaults; unit 2 defaults | ModY 841; unit 1's volume articulator draws a 30-point pattern |
| Phaser | 2 | 1: Chocolate / 0; 2: Vanilla AP / 5 | defaults, to next 128; unit 2 cut 163, in 0 | unit 1 feeds unit 2 (serial), unit 2 cutoff LFO active; master 113 |
| Sequenced gate | 2 | 1: Vanilla BP / 1; 2: Lime HP / 1; 3: Lime HP / 5; 4: Chocolate / 4 | 256 / 42 / 117 / 128 0 0 / 128 / 128 / 0 / 128 / 128 / 0 | master 126, master LFO -23, ModX 211, ModY 265; patterns of 9, 33 and 19 points on volume and cutoff |
| Mangled | 2 | 1: Chocolate / 1, WS on; 2: Lime HP / 1, WS on | 84 / 46 / 74 / 128 0 50 / 133 / 128 / 0 / 76 / 128 / 0 | 3-point waveshaper curves, cutoff pattern of 31 points, pan and cutoff LFOs active; ModX 384, ModY 201 |
| Tri filter damage | 2 | 1: Lime LP / 1; 2: Vanilla HP / 1; 3: Vanilla LP / 3 | 59 / 61 / 171 / 128 0 0 / 128 / 128 / -26 / 128 / 128 / 66 | unit 1 sends 66 to unit 2, unit 2 sends 93 to unit 3 (mode 3: more stages); cutoff patterns of 13, 13, 5 points |
| Formant vowels | 5 | 1..5: Vanilla BP / 3; 6: Vanilla LP / 3; 7: Vanilla HP / 1 | 128 / 128 / 51 / 128 0 0 / 128 / 128 / 0 / 115 / 160 / 0 | five parallel band passes at input +5.6 dB with resonance LFOs, master 160, master LFO 0 |
| template | 6 | 1: Chocolate / 1 | defaults | comment "Preset name / Preset details" |

## 2. Parameter semantics (`ProcessParam` @ 0x008579e0)

REC index decode (`FUN_008584e0`): indices 0..7 are the globals (0..2 ->
0x128.., 3..4 IEF attack/release, 5 pattern count, 6..7 ModX/ModY); then
per slot `base_s = 8 + sum(12 + 32)` (the slot's 12 params followed by 8
articulators x 4 LFO params: `art.lfo+0xd0 + 4 k`, recompute `vtable+0x40`).
Flags: 0x20 rescales by Min/Max and 2^-30, 1 stores, 2 reads, 0x10 refreshes
the control, 4 formats the hint.

Globals (`FUN_00858e60`, `FUN_00854cc0`, `FUN_008590b0`, `FUN_0085d050`;
`0x8a46cc = 1/128`, `0x8a46d0 = 1/256`, `0x8e3524` the `.bss` `ln 11`
(inferred), `0x8a46c0 = 0.1`):

```
level(x) = x == 1 ? 1 : (11^x - 1) / 10
master         0x16c8 = level(p0 / 128)                       // 160 -> +5.6 dB
master LFO     0x16cc = sign(p1) level(|p1| / 128)            // copied into every articulator's +0x28 (LFO amount scale)
IEF level      0x16ac = level(p2 / 128)
IEF att/rel    t_k = (81^(v_k / 16384) - 1) * 0.0125 SR       // 4096 -> 25 ms, 8192 -> 100 ms, 16384 -> 1 s
               0x16bc[k] = t_k > 0 ? exp(ln 0.01 / t_k) : 0   // one-pole, 1 % left after t
pattern count  0x13c = clamp(v, 1, 10); every articulator's current envelope +0x10 = patterns[0x13c]   // FUN_0085d050: the count selects the pattern
ModX / ModY    0x173c / 0x1740 = v / 1024
```

Slot (`FUN_00858af0`, slot `S`):

```
p0  env amount   art[2].amount (+0x24) = p0 / 256; art[2].lfoScale (+0x28) = that * 0x16cc
p1  cutoff       S+0x74 = p1 / 256
p2  resonance    S+0x78 = p2 / 256
p3..p5 L/B/H     S+0x88.. = level(p / 128)                    // 128 = unity, 0 = off
p6  WS pre-amp   S+0xac = p6 / 256                             // 64..192 -> 0.25 .. 0.75
p7  WS mix       S+0xa8 = p7 / 256
p8  pan          S+0x44 = p8 / 128                             // -1 .. 1
p9  volume       S+0x48 = level(p9 / 128) * master, or 0 when another unit is soloed
p10 input        S+0x3c = sign(p10) level(|p10| / 128)         // negative inverts the phase
p11 to next      next slot's +0x40 = sign(p11) level(|p11| / 128)   // FUN_0085d110, only when both units are enabled
then FUN_0085d110: rebuild the enabled list 0x1768 and the IEF feed flag (+0xb0 on unit 1 when 0x16b8 == 2)
```

Waveshaper table (`FUN_0085c340`, when WS enabled): `N = (bipolar + 1) *
16384`; `table[i] = clamp(round(curve(i / N) * ((bipolar + 1) * 32768 - 1)),
-32768, 32767)` as i16 with one guard entry each side; the curve is the kind-3
envelope at `S+0x94`. Mapping tables (`FUN_0085cb60`): each articulator's
three curves become 1025-float tables (`art+0x98 + 8k`) unless the curve is
untouched, in which case the shared default table `PTR_DAT_008539d0` is used.

Dispatcher (`FUN_008545c0`): `FPD_SetSampleRate` (4) -> `FUN_00854e20`:
`0x1770 = round(SR / 5)`, the oversampled rate goes to the info block,
`0xe4 = 523.25 / SR` (the C5 reference for the MIDI note mapping), control
tick from the table `DAT_008d1a08[oversampling | rendering]` (64, 8),
`0x16d8 = 1.007 / SR` and `0x16dc = 0.96` (DC filter), IEF coefficients.
`FPD_Flush` (2) clears all stage states, the oversamplers, the DC filter,
and sets `0x16c4 = 1` so the next block snaps every ramp. `FPD_SetTimeSig`
(0xe) stores PPQ and passes the signature to the pattern editor;
`FPD_ProcessMode` (1) sets the rendering flag (doubles the oversampling, caps
at 4). `FPD_MIDIIn` (0x18) with 0x160 set posts note-ons to the form (pattern
selection by note).

## 3. Signal chain (`Eff_Render` @ 0x0085aa10)

```
n' = n * oversampling (0x168c); buffers: in', mix, unitIn, unitOut, aux carved from this+0xd4
silence: |x| <= 1e-6 over the block -> 0x1774 -= n; else 0x1774 = 0x1770; asleep after SR/5 of silence (the input is still upsampled and copied)
in' = upsample(Source) if oversampling else Source
mix = 0
for each control tick of 0x16a0 samples (64; 8 with HQ envelopes):
    X, Y smoothing: 0x1754 += (ModX - 0x1754) * 0x14c when smooth, else = ModX (same for Y); indices 0x1748/0x174c = clamp(round(.), 0, 1024)
    if IEF on: 0x1750 = clamp(round(0x16b4 * 1024), 0, 1024); 0x16b4 = 0
    for every articulator a (all 64):
        v = a.base (+0x20: 1.0, or 0 in additive mode +8 == 1)
        for k in X, Y, IEF with a table: m = table_k[idx_k] (times a.amount for IEF); mode 0: v *= m; mode 1: v += m; else v *= 2 m
        a.eval (FUN_008543d0): if the current pattern envelope is active: e = env(t); mode 1: v += e * amount else v = v * e * amount
                               if the LFO is active: l = (level(t) - 0.5) * osc(t); mode 1: v += l * lfoScale else v *= l * lfoScale * 2 + 1
        a.value (+0x170) = v
    if IEF source == 1: FUN_0085a990(in')                     // follower on the input
    for each enabled unit U in order (0x1768):
        unitIn = prev * U.fromPrev (+0x40) + in' * U.inGain (+0x3c)           // zero when both are 0
        if U.type >= 0:
            cutoff = U.cut + art[2].value; res = U.res + art[3].value         // +0x238, +0x23c
            type 0: L/B/H gains = level_k * art[4..6].value (ramped), FUN_00859100 (coefficients), stages: SVF kernel -> three outputs summed with the gains
            types 1, 2: coefficients, Lime LP / HP kernel per stage, with per-sample coefficient ramps (+0x50/+0x54)
            types 3..7: biquad coefficients (FUN_00859100 -> FUN_006249d0), SSE biquad kernel per stage
            types 8..10: gain = (8^(p3/64) - 1) * 0.125 + 0.125 (asm: `(exp(x ln 8) - 1) * 0.125 + 0.125`, +0x240), then as 3..7
            type 11: drive +0x240 = p3 / 128, Mango kernel per stage
        if WS table: unitOut = WS(unitOut, table, preamp +0xac, mix ramped +0x24c)   // four kernels: unipolar/bipolar x normal/HQ (HQ when rendering or 0x161)
        if U.feedsIEF (+0xb0): FUN_0085a990(unitOut)
        matrix = FUN_00616620(pan + art[0].value * 2, vol * art[1].value)       // four coefficients, ramped (FUN_0062aee0)
        mix += unitOut * matrix                                                   // PTR_DAT_008d35e8
        prev = unitOut
    advance buffers by the tick
out = downsample(mix) if oversampling
Dest = center (0x15d) ? DC-filter(out) : out                                      // PTR_DAT_008d3e80, state 0x16d4, coefficient 0x16dc
```

The routing is therefore a chain with a tap: unit `k` hears the input at its
own input gain plus unit `k-1`'s output at unit `k-1`'s "to next" gain, and
every unit's output also goes to the master mix through its own pan and
volume. With "to next" at 0 the eight units are parallel; with input volume
0 and "to next" at full they are serial.

Articulator timing (`FUN_00856570`): every articulator's envelopes are told
the tick rate `tempo * 96 / 44100 * 2` and the pattern length
`that * (beats per bar) / 60` (constants `0x856670 = 96`, `0x856678 =
44100`, `0x856680 = 2`, `0x856688 = 60`); the LFO and envelopes run in beats
and the ten patterns are selected by the pattern count knob (or a MIDI note).

Envelope follower (`FUN_0085a990`): mono sum of the block, then the shared
peak follower `*DAT_008e6388(buf, state 0x16b0, n, coefs 0x16bc, gain
0x16ac)`, and `0x16b4 = max(0x16b4, block peak)`; the IEF mapping table is
indexed by that peak each tick.

Output matrix (`FUN_00616620(m, pan, vol, 0)`): pan 0 gives `[vol, vol, 0, 0]`;
otherwise the shared pan law (`PTR_DAT_008d2f68`) sets `m0, m1` and the
side that loses level is cross-fed by half the loss (`0x8a46ac = 0.5`), the
PanOMatic law.

### Filter designs (`FUN_00859100`, per unit, per tick)

Inputs: `x` = modulated cutoff (+0x238, 0..1 at the knob's range, more with
modulation), `r` = modulated resonance (+0x23c), `c = S+0xc4` the rate
scale, `alt` the ALT flag, `n = S+0xb4 + 1` stages. For the biquad types with
ALT the resonance is `r * (x + 1) * 0.5` (`0x8d3b40 = 0.5`), a resonance
that grows with cutoff. The stage loop runs from the last stage to the first
and, when `+0xb8 > 0`, detunes each earlier stage's cutoff through
`FUN_005fc870` (a power function; not resolved further).

Type 0, Chocolate state variable (`0x85a748 = 51`, `0x85a758 = 0.0159`,
`0x85a760 = 0.005`, `0x85a768 = 0.625`, `0x85a778 = 0.2`, `0x85a780 = 0.9`):

```
fc = x > 0 ? (51^x - 1) * 0.0159 : 0                       // x = 1 -> 0.795
F  = 2 sin(min((fc + 0.005) * c, 0.625))  -> stage+8        // Chamberlin coefficient, capped at 0.625 rad
q  = r <= 0 ? 1 : r < 1 ? 1 - 0.9 r : 0.2;  alt: q = q^2   -> S+0xd4 (damping)
kernel (FUN_006257c0, per channel, per sample):
    low  += F * band;  high = in - (band * q + low);  band += F * high
    outputs low, band, high scaled by the three gains (+0x240..+0x248) and summed
```

Types 1 and 2, Lime low / high pass (`0x85a788 = 76`, `0x85a790 = 0.01324`,
`0x85a798 = 1.6`, `0x85a7a8 = 1001`, `0x85a7b0 = 0.049`, `0x85a7a0 = 50`,
`0x85a7b8 = pi/2`, `0x85a7c0 = 0.007`, `0x85a7c8 = 0.4 pi`):

```
fc = min((76^x - 1) * 0.01324, 1.6)
q  = r <= 0 ? 1 : r < 1 ? (1001^r - 1) * 0.049 + 1 : 50;  with more than one stage q goes through a power (FUN_005fc870, exponent not resolved)
per stage:  f = min((fc + 0.007) * c * pi/2, 0.4 pi)
            g  = 2 - 2 cos f                      -> stage+8
            d  = cos f - 1
            k  = sqrt(-d^3) * K / (q * d) + 1,  K at 0x8e6380 (.bss), floor 0.1   -> stage+0xc
kernel (FUN_006259e0, LP):  band += (in - low) * g;  low += band;  band *= k;  out = low      // coefficients ramp per sample (args 6, 7); extended precision
        (FUN_00625b00, HP):  the same recurrence, out = in - low
```

Types 3..10, Vanilla (RBJ biquads; `0x85a7e8 = 1501`, `0x85a828 = 0.0004`,
`0x85a7f0 = 0.000665` for the LP, `0x85a840 = 1.0001` cap, `0x85a838 =
1e-4`, `0x85a810 = 2 pi`, `0x85a820 = 2.8495` rad cap, `0x85a7f8 = 2001`,
`0x85a800 = 0.02495`, `0x85a7e0 = 0.1`, `0x85a858 = 101`, `0x85a860 =
0.1199`, `0x85a848 = 0.01`, `0x85a850 = 12`, `0x85a868 = 3`, `0x85a870 =
0.45`, `0x85a878 = 100`, `0x85a880 = 0.04995`):

```
fc = min((1501^x - 1) * 0.0004, 1.0001)   (LP: (1501^x - 1) * 0.000665)
w  = clamp((fc + 1e-4) * 2 pi * c, 0, 2.8495);  s = sin w, k = cos w            // FUN_005fc960
LP, HP, AP (3, 6, 7):  Q = r <= 0 ? 0.1 : r < 1 ? (2001^r - 1) * 0.02495 + 0.1 : 50;  alpha = s * 0.5 / Q
BP, BS (4, 5):         bw = r <= 0 ? 0.01 : r < 1 ? (101^r - 1) * 0.1199 + 0.01 : 12;  alpha = s * sinh(ln2/2 * bw * w / s)     // FUN_00466a90
low/high shelf (8, 10): A = +0x240 (section 3 gain), S = r <= 0 ? 0.1 : r < 1 ? (3^r - 1) * 0.45 + 0.1 : 1;  alpha = s/2 * sqrt((A + 1/A)(1/S - 1) + 2)   // via sqrt(A) and FUN_0040c510
peaking (9):            A = +0x240, Q = r <= 0 ? 0.1 : r < 1 ? (201^r - 1) * 0.04995 + 0.1 : 100;  alpha = s / (2 Q)
code = table 0x8d1e4c[type] = 1 LP, 2 BP, 3 notch, 4 HP, 5 AP, 6 low shelf, 7 peak, 8 high shelf
FUN_006249d0(stage, A, alpha, s, k): the RBJ cookbook, normalised by 1/(1 + alpha):
    a1 = -2 k / (1 + alpha), a2 = (1 - alpha) / (1 + alpha);  LP b = [(1-k)/2, 1-k, (1-k)/2]; BP b = [alpha, 0, -alpha]; notch b = [1, -2k, 1];
    HP b = [(1+k)/2, -(1+k), (1+k)/2]; AP b = [a2, a1, 1]; shelves and peak with A as in the cookbook (codes 6, 7, 8)
kernel (FUN_006251f0, SSE, both channels): y = b0 x + b1 x1 + b2 x2 - a1 y1 - a2 y2 in double precision, per stage
```

Type 11, Mango low pass (`0x85a888 = 201`, `0x85a890 = 0.01495`, `0x85a868
= 3` cap, `0x85a898 = 251`, `0x85a8a0 = 0.12`, `0x85a8a8 = 0.001`, `0x85a8b8
= 0.6`, `0x85a8c0 = 1.9`, `0x85a8c8 = 0.65` cap, polynomial `0x85a8d0..0x85a900
= 1.873, 0.4955, 0.649, 0.9988, -3.9364, 1.8409, 0.9968`, `0x85a908 = 4`):

```
fc    = min((201^x - 1) * 0.01495, 3)
drive = (251^(+0x240) - 1) * 0.12;  dk = drive >= 0.001 ? 1 / drive : 1            // +0x240 = p3 / 128
res   = r <= 0 ? 0 : 1.9 * min(r, 1)                                                 // the 0.6 * min(drive, 0x85a8b0) term reads a denormal constant, so it is 0
with more than one stage: res = (res + 1)^(+0xbc) - 1   (FUN_00466720 = power; +0xbc not resolved)
per stage: g = min((fc + 0.01) * c * 0.5, 0.65)                                      // 0x85a848 = 0.01
           p = -(1.873 g^3 + 0.4955 g^2 - 0.649 g + 0.9988) g;  stage+0x50 = 1 - exp(p)      // one-pole coefficient
           stage+0xc = -res * (-3.9364 g^2 + 1.8409 g + 0.9968)                              // feedback
           with drive: stage+0x58 = drive * 2;  stage+0x50 *= dk;  stage+4 from FUN_00466b40(dk * 4) and (1 - feedback) * 0.5 (saturation make-up; only partly read)
kernel (FUN_006261c0, per channel): in' = in + feedback * lastOut;  y1 += (in' - y1) c;  y2 += (y1 - y2) c;  y3 += (y2 - y3) c;  y4 += (y3 - y4) c;  out = gain * (y4 + y3);  lastOut = out
```

A Moog-ladder style 4-pole with the classic cutoff polynomial correction and
a resonance feedback of up to 1.9. The drive terms were only partly read.

### Waveshaper kernel (`FUN_0062a380`, unipolar; `FUN_0062a550` bipolar)

```
i   = round(x * preamp * N);  i < 0: y = -table[min(-i, N)] else y = table[min(i, N)]      // unipolar mirrors the curve
out = y * (mix * 2^-15) + x * (1 - mix)
```

## 4. What stays unknown

- `S+0xc4` (the cutoff rate scale) is set with the oversampling factor in
  `FUN_00856690`, which was not decompiled; from the 2.8495 rad clamp on
  the biquads it is `0.5` per unit of oversampling (so `x = 1` is 0.6 of
  Nyquist on the Vanilla types), `inferred`.
- `K` in the Lime resonance (`.bss` 0x8e6380) and `ln 11` (0x8e3524) are
  runtime constants.
- The stage detune (`+0xb8`, `FUN_005fc870`), the exact pairing of the
  Mango drive terms and the Lime HP output were read from one candidate of
  each dispatch pair; the SSE pair of Lime/Mango was not compared.
- The pattern envelope evaluation (`TMEnvelope vtable+0x58`) and the LFO
  shape/speed word (`+0xd0 = 40000`, the same unit automation-234 leaves
  open) were not read; `FUN_00856570`'s tick scaling is read as the beats
  per second at the project tempo.
- The HeartWheel / IEFUseF1 identification of the two loose saved values is
  by form slot order.
- The mode selector byte (+0x30) to stage count (+0xb4) mapping was not
  read; corpus values 0, 1, 2, 3, 5.
- GUI: the Dispatcher's window messages, the editor forms (`MEnvEditor`),
  the `Default.dat` preset file in `Data/`.

## Corrections

Found while implementing `crates/plugins/src/love_philter.rs` from the DLL and the corpus:

1. Unit parameters p0 (envelope amount), p6 (waveshaper pre-amp) and p7 (waveshaper mix) divide by 128, not 256 (`PTR_DAT_008d3970` holds 1/128). Pre-amp 64..192 is 0.5..1.5.
2. `0x154` is the HQ envelopes flag (tick table 64 or 8), not an oversampling index. `0x158` is the number of rate doublings (`DQOverSelect` 0..4, constructor default 1, so 2x), not a factor. Corpus pairs: (0,1) x82, (0,0) x16, (1,0) x2, (1,1) x2. The default plugin runs 2x oversampled.
3. `S+0xc4` is `TAudioRenderer+0x20`, 44100 over the oversampled rate (`FUN_0062b030`), not "0.5 per unit of oversampling".
4. State block tail: three bytes for every version (smooth, centre, step); version 10 adds `0x15f`, version 11 adds `0x160`, and the HQ waveshaper byte `0x161` is only the last byte of version 12.
5. Mode byte to stages (`FUN_0085d5a0`): stages = mode, 0 is no filter; 4 and 5 are 2 and 3 stages with detune, each earlier stage's cutoff raised to the power 0.5 or 0.75 (table at `0x8d1e50`). Resonance and gain spread by the power 1/stages (Mango: 1/((stages-1)/4 + 1)).
6. Lime's `K` at `0x8e6380` is sqrt(2); `0x8e3524` is ln 11.
7. Mango: output is `gain * (y4[n] + y4[n-1])`, not y4 + y3. Pole coefficient `1 - exp(-pi * poly * g)`; the pi (`0x8e3440`) was missing. `0x85a8b0` is the f32 1.0, not a denormal, so resonance is `r * (1.9 + 0.6 * min(drive, 1))`. Make-up is `(1 - fb) / 2`, with drive `(1 - fb) / (2 tanh(4 / drive))`; ALT multiplies the feedback by that tanh.
8. Vanilla: shelf slope and peak Q use `1 - r`; the peak's base is 2001: `(2001^(1-r) - 1) * 0.04995 + 0.1`. The low pass adds 0.003 (others 1e-4) with no cap; the high pass caps at 0.6001.
9. Chocolate: only the first stage carries the resonance, later stages have damping 1. ALT also sets damping to `q * c / F`.
10. Articulators: pan, cutoff and resonance add (centre 0.5); the other five multiply (centre 0). A map's default level is `1 - centre`, so multiplying articulators default to (0,1) (1,1).
11. Timing: tick length is `round(T * rate / 44100 / 4) * 4`, minimum 4. An envelope's `x` unit is one beat with flag 1 and half a second without. LFO speed is `(10001^(v/65536) - 1) * 6871947.6736` of 2^32 per `x` unit. The 16 bytes after a kind 1 block are the attack, decay, sustain and release knobs; after a kind 2 block speed, tension, skew and pulse width.
12. The follower hears `max(|L|, |R|)`, not a mono sum.
13. The waveshaper kernels run fully wet; the mix is a separate ramped crossfade. Two more kernels interpolate (HQ).
14. Presets: 47 in the plugin's preset directory and 5 elsewhere, not 49 + 3.
15. The LFO's level envelope counts from 0.5, whatever the articulator's mode: `TMLFO`'s constructor (`0x788410`) sets its centre (+0x18) to 0.5, and the mode setter (`0x8544d0`) re-centres only the ten patterns and three maps. The value the tick (`0x788b70`) hands on is `(level - 0.5) * osc`, so a level of 1 swings ±0.5 and a multiplying articulator at full LFO amount goes from 0 to 2.
