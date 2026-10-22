# Event 234: automation clip data, and how a clip finds its target

Read from the FL Studio 2026 loader (`FLEngine_x64.dll`, Ghidra base 0x400000,
which is also the PE image base, so file offsets map straight to the addresses
below) and checked against the 165 `.flp` files under `Data/` (1,105 automation
channels, 9,785 points). Confidence tags: `loader` means read from decompiled C
or disassembly, `corpus` means the bytes in the projects agree, `pyflp` means
PyFLP says the same, `inferred` means a reading no second source confirms.

The loader hands the 234 payload to the channel's clip object
(`FUN_00f46490` line 1898: `chan+0x360->vtable+0x10(stream)`). That object is a
`TACArticulator` (VMT 0xfe2580, created in the kind-5 branch of the channel
kind setter `FUN_00ff7850`). It owns two sub-objects: a `TMEnvelope` (VMT
0xb97508, the points you draw) and a `TMLFO` (VMT 0xb976a8, the clip's LFO).
Each serialises itself, so the payload is three nested blocks, and PyFLP's
"112-byte trailer" is the LFO block with its default single point.

## Payload layout

### Articulator header, 9 bytes (`FUN_00fe8640`)

| offset | type | field | value in corpus | confidence |
|---|---|---|---|---|
| 0 | u32 | format tag; written as the constant 1, read into a local and dropped | 1 (1105/1105) | loader, corpus |
| 4 | i32 | `obj+0x30`. Not used by the evaluator I read. PyFLP calls it LFO amount; the reset code sets the LFO amount float `obj+0x28` to 0.5 and 64/128 = 0.5, so that reading fits | 64 (1094), 0 (7), 38, 59, 123, 128 | loader for the slot; meaning pyflp, inferred |
| 8 | u8 | `obj+8`, LFO combine mode (see "LFO" below): 1 adds, anything else multiplies | 0 (1073), 1 (32) | loader, corpus |

### Envelope block, 48 + 24n bytes, at offset 9 (`FUN_00b9fd20` -> `FUN_00b97c40`)

| offset | type | field | value in corpus | confidence |
|---|---|---|---|---|
| +0 | u32 | envelope kind (`obj+0x58`); 4 for the automation envelope. On load the stream value is compared with the object's kind and the version is read only when they match (or the object's kind is 0) | 4 | loader, corpus |
| +4 | u32 | format version, stored in the global `DAT_016b6a6c` and consulted by every later read. Current writers emit 3 | 3 | loader, corpus |
| +8 | u32 | point count n | 1..120 | loader, corpus |
| +12 | n x 24 | points, see below | | loader, corpus |
| +12+24n | 20 bytes | `obj+0x44..0x57`: u32 @0; i32 @4 (`obj+0x48`) gates evaluation, non-zero = active (`FUN_00ba1700`); i32 @8, @12, @16 are point indices (loop and sustain markers used by the Sytrus-style envelope; -1 = none; @8 is forced to -1 after load unless envelope flag 8 is set) | 1, -1, -1, -1, -1 in every clip | loader; index meaning inferred |
| +32+24n | 16 bytes | `obj+0x98..0xa7`, read only when version <= 3; four u32 | 128, 128, 0, 128 in every clip | loader for the slot; meaning unknown |

Version gates in `FUN_00b97c40` (so a reader can take old files):
version < 2 reads x as f32 (f64 when kind == 6) and y as f32; version >= 2
reads f64 x, f64 y; version < 1 reads only the 4-byte tension and zeroes bytes
20..23; version > 3 reads nothing after the version word. The corpus is all
version 3.

### Point record, 24 bytes on disk (32 in memory)

| offset | type | field | range | confidence |
|---|---|---|---|---|
| 0 | f64 | x: distance from the previous point in beats (quarter notes). The first point is always 0. Stored as f32 in memory (`point+0`) | >= 0; sum over the clip equals the playlist item length / PPQ (ratio 1.000 for 2253 of 3800 item/clip pairs, the rest are trimmed items) | loader for the delta (`FUN_00b9d390` sums x[0..i] for a position); corpus for the unit |
| 8 | f64 | y: value, 0..1 of the clip's range. NaN is replaced by the previous y on load (`FUN_00477180` check) | 0..1 (one point slightly above 1) | loader, corpus |
| 16 | f32 | tension | -1..1 | loader, corpus |
| 20 | u8 | curve mode 0..12 (see "Interpolation") | 0 (8334), 2 (856), 10 (252), 1 (165), 7, 11, 3, 6, 5, 8, 9, 12, 4 | loader, corpus |
| 21 | u8 | per-point flag; zeroed on load unless envelope flag 0x10 is set, which automation clips do not set | 0 | loader, corpus |
| 22 | u8 | not read by any code I followed | 0 | corpus |
| 23 | i8 | cache: sign of the tension (-1, 0, 1), or 2 for a wave segment with zero cycles. `FUN_00b9ba60` recomputes it after load, so a reader may ignore it and a writer can emit sign(tension) | 0xff (3434), 2 (2298), 1 (2172), 0 (1881) | loader, corpus |

A point's x, tension and mode describe the segment that *ends* at that point:
the evaluator (`FUN_00b9def0`, 0xb9e099..0xb9e208) advances the segment index,
takes the segment length from the new point's x and the shaping from the new
point's mode and tension, and interpolates from the old point's y to the new
one. The first point's tension and mode are therefore unused.

### LFO block, 52 + 24m bytes, follows the envelope block (`FUN_00ba01a0` -> `FUN_00b97c40`)

| offset | type | field | value in corpus | confidence |
|---|---|---|---|---|
| +0 | u32 | envelope kind, 5 for the LFO | 5 | loader, corpus |
| +4 | u32 | format version | 3 | loader, corpus |
| +8 | u32 | point count m | 1 (1103), 2 (2) | loader, corpus |
| +12 | m x 24 | points, same record as above. Default is one point x 0, y 1.0, tension 0, mode 0 | | loader, corpus |
| +12+24m | 20 bytes | `obj+0x44..0x57` as above | 1, 0, -1, -1, -1 | loader |
| +32+24m | 16 bytes | `obj+0xd0..0xdf`, read when version <= 3. The reset code puts 0x9fc2 (40898) in `+0xd0` and zeroes the other three; the LFO evaluator feeds `+0xac` into an oscillator (`FUN_00677820`), so `+0xd0` is most likely the LFO speed in that oscillator's unit | 45819 (1095), 26257 (6), 26259, 35989, 40898, 40000; then 0, 0, 0 | loader for the slot; meaning inferred |
| +48+24m | u32 | `obj+0xe0`, read when version >= 3, else set to 0 | 0 | loader, corpus |

### Size check

`len = 9 + (48 + 24n) + (52 + 24m) = 109 + 24(n + m)`. With the usual m = 1 this
is 133 + 24n, which is PyFLP's `21 + 24n + 112`. All 1,105 clips match the
nested formula; two clips (`Synth_dfr - Overcome - Sytrus.flp` channel 47,
`Astes - Raubana - LIFE.flp` channel 42) have m = 2 and break PyFLP's fixed
trailer by 24 bytes. A reader must parse the LFO block's own count.

## Binding: automation channel to target parameter

The clip does not store its target. The link is a separate 227 event
(ControllerID.Remote; 226 is the MIDI flavour). The loader builds one object
per event (`FUN_00f46490` line 2682, `FUN_00f82530`):

    obj = alloc(0x1c)
    obj+0x00  compiled formula, nil until a 230 event fills it
    obj+0x08  the 20 payload bytes, copied verbatim
    obj+0x14  (payload @12) |= 4 for 226, |= 8 for 227
    obj+0x18  (payload @16) *= -4 when the writer version is below 4.6.1
    appended to PTR_DAT_015e0a00[0] (226) or [1] (227)

230's string is compiled by `FUN_00f81f70` with variables `Input` and
`Tension(x, t)` and stored at the most recent link's `obj+0`.

Payload of 226/227:

| offset | type | field | confidence |
|---|---|---|---|
| 0 | u32 | source REC id. For an automation clip this is `channel_index << 16` (control 0 of the automation channel). Other internal controllers (Peak, Formula, Control Surface outputs) use their own REC id, e.g. `0x20098001` = insert 0, slot 9, plugin parameter 1 | corpus: 1322 of 1624 links have an automation channel as source and all of those have low 16 bits 0; loader for the channel REC base (`chan+0x9c`, used by `FUN_01191a50` when it creates a clip's link) |
| 4 | u32 | 0 in 1396 of 1624; the rest look like a second id (`0x79616c50`, `0x680000`) | unknown |
| 8 | u32 | target REC id, the same space as `docs/rec-225.md`: below `0x10000000` a channel control `channel << 16 \| control` (control >= 0x8000 is a plugin parameter); `0x20000000`/`0x70000000` mixer ids by writer version; `0x40000005` the tempo | corpus: all 495 channel-scope targets name a channel below the file's channel count; mixer base 0x2 only in files before 24.2.99, 0x7 after |
| 12 | u32 | flags: 8 remote link, 4 MIDI link; bits 0 and 1 also seen set (0x9, 0xa) | loader, corpus |
| 16 | i32 | default 0x1d5 (469) from the constructor; -512 in most MIDI links. Pre-4.6.1 files store it divided by -4 | loader for the default and the version fixup; meaning unknown |

So for `crates/model`: automation channel N drives every 227 whose u32@0 equals
`N << 16`; the target is u32@8 decoded with rec-225's rules (for mixer ids:
insert = `(id - 0x70000000) >> 22`, slot = `(id >> 16) & 0x3f`, control = `id &
0xffff`, after the base-0x2 remap for old files). One clip can drive several
targets (69 clips have 2 links, one has 65), and 36 clips in the corpus have no
link at all.

## Value mapping

Points are 0..1. The clip's output is scaled by the channel's 219 event, which
for kind-5 channels the loader copies to `chan+0x16c` (`FUN_00f46490` line
2574):

| 219 offset | type | field | default | confidence |
|---|---|---|---|---|
| 0 | i32 | min, as fraction of the target's full range x 12800 | 0 | loader (kind setter leaves it 0), corpus (1033 of 1105 at 0) |
| 4 | i32 | max, same unit | 12800 (`FUN_00ff7850` writes 0x3200 for kind 5) | loader, corpus |

The unit comes from `FUN_01191a50`, the "create automation clip for this
control" action: it computes a low and a high fraction (for the tempo,
`(60000 - tempoMin) / (tempoMax - tempoMin)` and the same for 180000; for a
channel's control 4 it takes the current value / 96 plus or minus 0.5), rounds
`fraction * 12800`, and sends both through the REC dispatcher to the automation
channel's own controls (REC ids `chan+0x9c` and `chan+0x9c + 1`, flags 0x11).
The "Tempo (coarse)" fixup in the loader (line 3187) rescales both fields for
old projects, which fits a range-fraction unit.

So the value reaching the target is linear in the raw control range:

    fraction = (min + v * (max - min)) / 12800        v = clip output, 0..1
    raw      = target_min + fraction * (target_max - target_min)

The last line is inferred: I did not read the dispatcher's normalised-value
path. It matches FL's behaviour that a volume clip at 0.8 gives 12800 (0 dB) on
a 0..16000 control. The `(exp(v*K) - 1) * 0.1` curve from rec-225 is the volume
control's own gain law, applied after this, not part of automation.

The articulator combines envelope and LFO before scaling (`FUN_00ba1700`):

    out = base(+0x20, 0.0) + env(t) * envAmount(+0x24, 1.0)
    if header@8 == 1:  out += lfo(t) * lfoAmount(+0x28)
    else:              out *= lfo(t) * lfoAmount(+0x28) * 2 + 1

Each term is evaluated only when that envelope's `obj+0x48` (the first i32 of
its 20-byte block) is non-zero. The envelope block carries -1 there in every
clip, the LFO block carries 0, so no corpus clip has an active LFO and the
clip's output is just the envelope. A writer that emits the default LFO block
(one point at y 1.0, `+0x48` = 0) gets the same.

## Interpolation

Setup per segment (`FUN_00b9def0`): t runs 0..1 across the segment, the
result is `y0 + f(t) * (y1 - y0)` (0xb9eb56). `f` depends on the end point's
mode and tension T; `FUN_00b9ba60` precomputes three floats per point
(`point+0x10, +0x14, +0x18`, written below as A, B, C) after every load, and
`FUN_00b9d450` gives a step count for modes 3..6:

    L     = ln(1000001) = 13.8155
    g(u)  = (exp(k*u) - 1) / (exp(k) - 1),  k = |T| * L       (A = sign(T)*(exp(k)-1), B = 1/|A|, C = k)
    steps = round((1000001^(1 - 0.6*|T|) - 1) / 1000)        (+1 for mode 3 when T >= 0)

| mode | FL name (from the string table) | f(t) | confidence |
|---|---|---|---|
| 0 | Single curve | T = 0: t. T < 0: g(t). T > 0: 1 - g(1 - t) | loader (0xb9e2c1) |
| 1 | Double curve | h(u) as mode 0's f. t < 0.5: 0.5 * h(2t). else: 1 - 0.5 * h(2(1 - t)) | loader (0xb9e352) |
| 2 | Hold | 0 (default branch, 0xb9eb4e); the segment keeps y0 | loader |
| 3 | Stairs | N = steps + 1, s = T >= 0 ? 1 : -1. `round(t*N - 0.5*s) / (N - s)` | loader (0xb9e4f3, prep 0xb9bd40) |
| 4 | Smooth stairs | N = steps + 1, s = t*N - 0.5, r = round(s - 0.5), u = 2(s - r) - 1. T < 0: w = sign(u) * (1 - (1 - abs(u))^3); else w = u^3. `(r + 0.5*w + 1) / N` | loader (0xb9e52f, prep 0xb9bd9b) |
| 5 | Pulse | N = 2(steps + 1), s as mode 3. `round(t*N - 0.5*s) & 1` | loader (0xb9e606, prep 0xb9bdbe) |
| 6 | Wave | P = steps + 0.5. T < 0: `(sin(2*pi*(t*P + 0.75)) + 1) / 2`. T >= 0: s = t*P - 0.5, `abs(2(s - round(s - 0.5)) - 1)` (triangle). steps = 0 with T > 0 marks byte 23 = 2 and gives f = t | loader (0xb9e63c, prep 0xb9be04); sin/cos identity inferred, see note |
| 7 | Single curve 2 | u = T > 0 ? 1 - t : t. `t + T * (u - u^6)` | loader (0xb9e6e2, A = T) |
| 8 | Double curve 2 | t < 0.5: 0.5 * m7(2t). else: 1 - 0.5 * m7(2(1 - t)), m7 = mode 7 | loader (0xb9e733) |
| 9 | Half sine | T > 0: q = sin(t*pi/2). else: q = 1 - cos(t*pi/2). `(1 - abs(T)) * t + abs(T) * q` | loader (0xb9e832, A = abs(T), pi/2 from `*0x15e0720`) |
| 10 | Smooth | `t + C * (3t^2 - 2t^3 + A*t^2*(t - 1) + B*t*(t - 1)^2 - t)`; A, B, C come from the neighbouring points' values and tension (prep 0xb9be64..0xb9c1b5, not read in detail) | loader for the evaluator (0xb9e8ae); prep not read |
| 11 | Single curve 3 | a = max(0.9*abs(T), 1.5259e-5), A = 1 - (1 - a)^3, B = (1 - A)/A, C = B; T > 0: B = -B, C = B - 1. `B / (1 - A * (T > 0 ? 1 - t : t)) - C` | loader (0xb9e941, prep 0xb9c1b5) |
| 12 | Double curve 3 | as 11 with B and C halved (prep 0xb9c252), applied in two halves like mode 8 | loader for the prep; evaluator at 0xb9e9d3 only partly read |

Note on sin/cos: the two libm calls are `FUN_0040bc90` and `FUN_0040bba0`. I
took the first as sin and the second as cos because that makes mode 6 start at
0 and mode 9 ease in both directions; swapped, mode 9 at T = 1 would run
backwards. Worth one check against FL's editor.

Open items: the exact prep for mode 10, the mode 12 evaluator tail, the meaning
of header @4, LFO `+0xd0`, envelope `+0x98..0xa7`, link bytes @4 and @16, and
whether the dispatcher's conversion from the 0..1 fraction to the raw control
value is linear for every control type.

## Addresses read

| address | what | how |
|---|---|---|
| `FUN_00f46490` 1898, 2574, 2682, 2717, 3187 | loader: 234, 219, 226/227, 230, tempo fixup | decompiled C (`ghidra/out/projloop`) |
| `FUN_00ff7850` | channel kind setter; kind 5 creates `TACArticulator` at `chan+0x360`, sets `chan+0x170 = 12800` | decompiled (`ghidra/out/auto234d`) |
| `FUN_01004230`, `FUN_01003c20`, `FUN_01003d80` | channel create; class `TPublicChannel` VMT 0xfe2d38 via `PTR_PTR_00fe2c70` | decompiled (`auto234b`, `auto234c`), VMT read from the DLL |
| `FUN_00fe8640` | `TACArticulator` load/save (VMT slot 0x10) | decompiled (`auto234e`) |
| `FUN_00ba1450`, `FUN_00ba1700`, `FUN_00ba18a0`, `FUN_00ba18b0` | articulator reset, evaluate, init, length | decompiled (`auto234e`) |
| `FUN_00ba1210` | articulator base constructor; class pointers `*0xb97440` (TMEnvelope) and `*0xb975e0` (TMLFO) | objdump |
| `FUN_00b9fd20`, `FUN_00ba01a0` | TMEnvelope and TMLFO load/save | objdump |
| `FUN_00b97c40` | shared envelope block load/save | objdump |
| `FUN_00b9ba60`, `FUN_00b9d450`, `FUN_006774b0` | per-point precompute, step count, exp-curve constants | objdump |
| `FUN_00b9def0` | envelope evaluator (the curve formulas) | objdump |
| `FUN_00ba0710` | LFO evaluator | objdump |
| `FUN_00f82530`, `FUN_00f81f70` | link object constructor, formula compiler | decompiled (`auto234`) |
| `FUN_01191a50` | "create automation clip" action: link creation and the min/max unit | objdump |

Scripts used for the corpus checks live in `/tmp/a234` (`dump234.py`,
`trailer.py`, `bind.py`, `beats.py`, `pe.py`); they are throwaway and not part
of the repo.


## Corrections from the automation implementation (crates/flp layout, crates/model/src/automation.rs, crates/engine/src/automation.rs)

- The corpus is not all version 3. That holds for the 165 projects. The 8,598 files also have 62 automation clip presets (`Patches/Channel presets/Automation clips/*.fst`): 56 are version 1 (writers 4.6.0 and 4.6.1), 2 are version 2 (writer 9.0.0), 4 are version 3. They fit the version gates above. A version 1 point is 16 bytes, `{f32 x, f32 y, f32 tension, u8 mode, u8, u8, i8}`, and versions 1 and 2 end the LFO block without its last u32. So the size is `105 + 16(n + m)` for version 1 and `105 + 24(n + m)` for version 2. `flp::event::layout::AutomationData` reads all three and writes each back as it was.
- The LFO is not off in every clip. The i32 that switches an envelope on is at offset 4 of the 20 bytes (`obj+0x48`), not the first one. In the LFO block it is -1 in three clips: `Synth_dfr - Overcome - Sytrus.flp` channel 47, and `Astes - Raubana - LIFE.flp` channels 42 and 95. The engine plays the envelope alone, so those three come out without their LFO.
- Mode 10 prep (0xb9be64..0xb9c1b5), read from the disassembly. With `EPS = 2^-24` (`*0x15e4ff0`), `dy = y1 - y0` and `dx = max(x, EPS)` of the segment:
  - `|dy| < EPS`: byte 23 = 2, and the evaluator returns t for byte 23 = 2 before it looks at the mode.
  - `C = 1 - |T|`. So tension 0 is the full cubic and tension 1 or -1 is the line.
  - B is the slope at the start and A the slope at the end, in units where the segment has length 1 and rise 1. Each is 0 unless the neighbouring segment is mode 10 as well: the previous point's mode for B, the next point's mode for A. Otherwise, with `r` the neighbour's length over `dx` (at least EPS) and `s` its rise over `dy` divided by `r`: 0 when `s <= 0`, else `s (3r + 3) / ((2r + 1) s + (r + 2))`. That is the weighted harmonic mean of a monotone cubic (Fritsch and Butland), with weights `2r + 1` on the neighbour's slope and `r + 2` on the segment's own slope of 1. The constant 2.0 is `*0xb9c570`.
  - The neighbour index comes from `FUN_00b9b9c0(i, -1 | -2 | +1)`, which is `i + offset`, or -1 outside the point list. It wraps inside the loop markers when those are set, and automation clips set none.
- Mode 12 evaluator (0xb9e9d3), read in full: below t = 0.5 it is mode 11's formula at `2t` with the halved B and C, and from 0.5 on it is `1 - (that formula at 2(1 - t))`. So it is two halves like modes 1 and 8.
- Byte 23 = 2 is set for more than a wave with no cycles. After the per-mode prep, a tension of exactly 0 sets it for modes 0, 1, 7, 8, 9, 11 and 12 (bit mask 0x1b83 at `0xb9c5c0`), and those segments are the straight line `f = t`. Mode 11 without the shortcut would bend by its minimum `a = 1.5259e-5`.
- A segment covers t from 0 up to, not including, 1 (`0xb9e18a` moves on at t >= 1). A hold therefore jumps on the beat of its end point.
- The trim of a playlist item on an automation channel is in beats into the clip. `end - start` equals the item's length in beats for 3,103 of the 3,104 corpus items with a trim (of 5,462 automation items, 71 of them muted).
- The tempo's range for the value mapping is 10 to 522 BPM, as 10000 and 522000. The 219 of all seven clips that drive the tempo is (1250, 4250), which is exactly 60 and 180 BPM on that range, the two tempos `FUN_01191a50` computes fractions for.
- Target counts, decoded with the legacy remap: 820 links to an insert, 495 to a channel, 7 to the tempo. When the engine first played automation, 117 of the 1,322 went to a control it played: 34 insert volumes, 1 insert pan, 28 send levels, 7 tempo, 45 channel volumes and 2 channel pans. It now plays the levels of a hosted generator's channel and the parameters of hosted plugins as well; the corpus render prints today's counts.

## Channel controls (docs/rec-channel.md)

What a binding to a channel's control below 0x8000 drives, with the knob
ranges the dispatcher scales a clip's value onto, is in
`docs/rec-channel.md`. The engine plays a channel's volume, pan, pitch, pan
offset and volume multiplier. Links in projects from before 11.5.6 number a
channel's plugin parameters from 0x2000, and FL's loader moves them to
0x8000 (`FUN_00f45980`); the model does the same.

## The formula's `Tension(x, t)` (crates/engine/src/formula.rs)

`FUN_00f81f70` registers one function with its parser besides the
arithmetic: `Tension`, two arguments, evaluated by `FUN_00f81d60`. With
`x` the first argument and `t` the second, both f64, and the constants
0.0 (`0xf81e90`), 1.0 (`0xf81e98`), the f32 1.0 (`0xf81ea0`) and the sign
mask (`0xf81eb0`):

    t == 0:  x
    t <  0:  u = -t;  (float)(exp((float)x * ln((float)u + 1f)) - 1) / u
    t >  0:  (t - (float)(exp((float)(1 - x) * ln((float)t + 1f)) - 1)) / t

(`FUN_0040c1d0` is `ln`, `FUN_0040ae70` `exp`.) So it is
`((1 + |t|)^x - 1) / |t|` below 0 and its mirror image above: 0 stays 0,
1 stays 1, and the sign of `t` bends the line down or up. The result goes
on as any formula's, kept inside 0 and 1. Corpus: the 32 links of
`Surround mix.flp` from the Y of the Fruity X-Y Controllers on inserts 1
to 16 to those inserts' sends
to inserts 17 and 18, `Tension(Input,10)` and `Tension(1-Input,10)`.

## Targets that change nothing FL plays (crates/engine/src/automation.rs, `unheard`)

A clip or a controller link on one of these is played by setting nothing:

- A parameter of a ZGameEditor Visualizer the engine hosts. Its
  parameters move its pictures; a hosted state runs only layers read to
  make no sound (`docs/plugins/zgameeditor-visualizer.md`), so no
  parameter changes the output, which is the input. The same for Fruity
  NoteBook 2, which FL never asks for sound. A Visualizer state the engine
  does not host keeps its links unsupported. Corpus: 52 clip bindings and
  7 Peak Controller links in the first 10 seconds of `Astes - Raubana -
  LIFE`, `JuiceBass - SeamlessR - Menagerie` and the Sacco projects, and
  2 NoteBook 2 bindings in `Astes - Bien Duro`.
- A control of an automation clip's LFO when that clip's LFO is off
  (`docs/rec-channel.md`).

## The arpeggiator's time and gate

A clip on a channel's arpeggiator time (0x303, 450..1447) or gate (0x304,
0..48) is played. The engine lays the arpeggios out from the song before
it plays (`docs/arpeggiator.md`, `crates/engine/src/arp.rs`) and reads the
clip at the tick of each step, which is where FL works out the next step
from the two knobs. Nothing is set while the song plays. A controller link
to either is counted unsupported, since it moves as the song plays. Corpus:
`nucleon - Harmless Galactic` channel 14 in its first 10 seconds, and the
clips on channels 12, 16, 17 and 20 of `Mehran Abbasi - With You` later on.

## What the corpus render still counts unsupported

The corpus render counts the bindings and controller links of the first
10 seconds of every project. What it still counts, and why:

- Parameters of a plugin channel whose generator the engine does not host:
  FLEX (`Cuebur & Thandi Draai - Africa's Calling`, `Edlan & Ella Noël -
  Song For You`, `Mehran Abbasi - With You`, `Purps On The Beat Feat
  Luclover - Stroll Up`) and FLEX's volume in `Mehdi Ebrahiminejad - Never
  Out`. Nothing plays on those channels.
- Parameters 39, 42, 43 and 49 of the Fruity Limiter on insert 1 of
  `Sacco - Pumping objects`: the rebuilt Limiter has 18 parameters
  (`docs/plugins/fruity-limiter.md`), and what FL drives with these
  numbers is not read.
- A clip bound to nothing: `Asher Postman - Future House` channel 34. FL
  drives nothing from it either; the render counts it so that a clip whose
  link did not decode does not pass unseen.

Past the first 10 seconds, the rest of the corpus adds: mod X on 3x Osc
(`Square a Saw - Oxygen` channel 51, see `docs/rec-channel.md`), formant
(control 15) on an audio clip channel (`Mehdi Ebrahiminejad - Never Out`
channel 14: the engine stretches nothing), the amount of 3x Osc's pitch
envelope (0x1408, `Treben - Gimbal and Sinan - RawFL`: the instrument
envelopes of a channel are not played), the switch of a Transporter slot,
which the engine does not host, and the other plugin parameters of
unhosted or parameterless plugins.
