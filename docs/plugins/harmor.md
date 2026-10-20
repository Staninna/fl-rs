# Harmor: state, parameters, signal architecture

Recovered from `Harmor_x64.dll` (FL Studio 2026, Delphi native plugin, image
base 0x400000, 6.2 MB; no engine DLL: the articulator, delay, reverb, flanger
and tone-bank classes are compiled in, the vector maths comes from Intel IPP
in FL's `dsp_ippv2.dll`). Ghidra project
`/shared/code/fl-studio-target/ghidra/proj`, project name `harmor`;
decompiles under `ghidra/out/harmor/`: `range/f/` holds every function in
0x862000..0x8bb000 (the plugin) and 0x799000..0x7ae000 (the shared classes),
one file each; `tree/` the transitive callees of the DSP roots; `vmt/`, `r2/`,
`r3/` the SDK slots and the state loaders; `osc1/` the tone bank and
`TToneBMP` (0x783000..0x785400, 0x792f00..0x799100); `disp/` the IPP import
table; `lvl_dec/`, `vdec/`, `fxa/`, `fxb/` and the `*dis*` directories the
later runs and disassemblies. State parser:
`tools/plugstate/harmor_state.py` (on the output of `dump213b.py`).

Corpus: 100 states in 21 of the 165 `.flp` (versions 18 x51, 19 x46, 16 x2,
17 x1; 84.5 KB to 1.06 MB), 128 enabled parts among them; 328 shipped presets
(versions 19 x101, 16 x130, 11 x48, 12 x40, 17 x9). All 428 blocks parse to
the last byte with the layout in section 1.

Harmor is an additive synthesiser with two parts, A and B, running the same
engine side by side. Per voice, each part builds a spectrum of up to 516
harmonics plus four special slots from two drawn timbres (or an image
resynthesised from a sample), runs it through twelve spectral units in a user
order (local EQ, prism, harmonic randomness, harmonizer, two filters, phaser,
pluck, blur, protection, clipping, the volume articulator), applies the global
EQ, and plays it on a recurrence sine bank (`TFastToneBank`, the same class as
Harmless). Unison copies are separate voices. Voices add into an FX bus and a
dry bus; the FX bus then runs six global units in a user order (distortion,
chorus, delay, reverb, the dry return, compression by an embedded
Soundgoodizer). Almost every knob has an articulator (envelope, LFO, nine
mapping curves), so every part serialises 69 curve sets: the state is 84 KB
before anything is drawn.

Confidence tags: `code` (read in the decompile or disassembly), `dfm` (the
editor form), `corpus`, `preset`, `inferred`.

## How the plugin is wired

`CreatePlugInstance` @ `0x0086dd40` -> constructor `FUN_00870750`, class
`TTestPlug`, VMT `0x0086dbd0` (instance size 0x2d10, parent
`TDelphiFruityPlug` `0x00779108`).

| slot | SDK name | address |
|---|---|---|
| 0 | destructor | 0x00871de0 |
| 1 | Dispatcher | 0x0086dda0 |
| 2 | Idle_Public | 0x0077cd90 |
| 3 | SaveRestoreState | 0x0086f2f0 |
| 4 | GetName | 0x008822f0 (text via `FUN_0087ca70`) |
| 5 | ProcessEvent | 0x00879280 |
| 6 | ProcessParam | 0x00879540 (index decoder `FUN_00879380`) |
| 7 | Eff_Render | abstract |
| 8 | Gen_Render | 0x00891320 |
| 9 | TriggerVoice | 0x00883cd0 |
| 10 | Voice_Release | 0x00885390 |
| 11 | Voice_Kill | 0x00885730 |
| 12 | Voice_ProcessEvent | 0x00885840 (returns 0) |
| 20 | (TDelphiFruityPlug) | 0x00892d00 |

Classes: `TOpArticulator` VMT `0x0086d568` (size 0xb8, over `TArticulator`
`0x0079a5b8`), `TCustomMEnvelope` `0x00799ef0` with `TMEnvelope`
`0x0079a0d8`, `TMLFO` `0x0079a278`, `TMMap` `0x0079a408`, `TMEQMap`
`0x007effe8`; `TFastToneBank` `0x00783248` (the oscillator bank);
`TToneBMP` `0x00792ef8` (the image); `TDelay_32FS` `0x007a5020`,
`TFruityReverb` `0x007a7590`, `TStereoFlanger` `0x007a87d8`, `TFilterLine`
`0x007acea8`, `TDeclicker` `0x00862ba8`, `TFPFPlug` `0x007c4d00` (the
embedded-plugin host), `TRenderer` `0x0086d6c8` (worker threads),
`TAudioRenderer` `0x00629cb8` (the host block at `this+0xcc`).

Maths helpers: `FUN_0040ad30` exp, `FUN_0040c090` ln, `FUN_0040c520` round,
`FUN_0040c610` trunc, `FUN_0040acf0` frac, `FUN_0040bb50` sin, `FUN_0040ba60`
cos, `FUN_00466b30` tanh, `FUN_00466730` ceil, `FUN_00466820` floor,
`FUN_0040c510` sqrt. Calls through `PTR_DAT_0092xxxx` are IPP functions bound
by `FUN_005bd1c0` from `dsp_ippv2.dll` (`ippsAdd_32f_I`, `ippsTone_Direct_32f`,
...), except 16 that bind FL's own SSE / AVX kernels (e.g. `PTR_DAT_0092c200`
-> `FUN_006270c0`, the biquad). Runtime constants written at DLL init
(`FUN_00630170`, `FUN_006071f0`): 0x93b4f0 ln 2, 0x93b4f8 1/ln 2, 0x93b508
ln2/1200, 0x93b510 ln2/12, 0x93b52c ln 3, 0x93b524 ln 11, 0x93b53c ln10/20,
0x93b540 log2 440, 0x93b2c8 ln 0.01, 0x93ad08 1/sqrt 2, 0x94e118 16.351598
(C0 = 440·2^(-57/12)). Plain pool floats: `PTR_0092acb8` 0.001, `c438` 0.01,
`c330` 1/1024, `be78` 1/128, `c058` 0.5, `b168` 0.25, `c1e8` 0.1, `af50`
0.125, `b078` 2π.

Host block `this+0xcc` (`TAudioRenderer`, set by `FUN_0062d4e0`): +8 sample
rate SR, +0x18 1/SR, +0x20 44100/SR, +0x28 2π/SR, +0x30 0.4999·SR (filter
clamp), +0x34 0.001·44100/SR (largest ramp step per sample), +0x40 the
default smoother coefficient `exp(ln 0.01 / (0.015·SR))` (15 ms to 1 %).

Notation below: `T` = control tick in samples (`this+0xe30`), `REC[k]` = part
parameter k of the part in question (REC index `386 p + k`), `art[a]` = the
smoothed output of articulator a for the voice (`voice + 0x2d4 + 0x120 a`),
`level3(x) = x == 1 ? 1 : (3^x - 1)/2`, `level11(x) = (11^x - 1)/10`.

### Plugin object

| offset | type | meaning |
|---|---|---|
| 0x138 | i32[825] | REC parameters: part A 0..385, part B 386..771, globals 772..824 |
| 0xd48 | i32[53] | = REC 772..824 (the global block of the state) |
| 0xe20 | list | voices (`+0x10` count, `+8` items); free list 0xe28 |
| 0xe30 | i32 | control tick T |
| 0xe34 | f32 | tempo BPM (ProcessEvent id 0) |
| 0xe58 | ptr[] | render buffer sets, one per worker (section 3.11) |
| 0xe64 | i32 | block length |
| 0xe68 / 0xe70 | i64 | sample clock / strum window end |
| 0x1094, 0x1098 | i32, u8 | voice randomness mode, same random for all maps |
| 0x109c | smoother | post volume |
| 0x10d0 / 0x10cc | i32 / f32 | partial cap Nmax / safety ceiling (octaves) |
| 0x10d4 | i32 | mono mode |
| 0x1108..0x1110 | f32[3] | mod X, Y, Z targets (v/512) |
| 0x1128 | u8 | mod X/Y/Z smoothing on |
| 0x112c..0x113c | | strum mode, time, interval, tension, direction |
| 0x1140 | i32 | precision (0 Average, 1 High, 2 Perfect) |
| 0x1150..0x1178 | fn[6] | vector exp, ln, tanh, sqrt, cbrt, div chosen by precision (`FUN_00878f60`) |
| 0x1184 / 0x1188 / 0x118c / 0x1194 | i32 | editor: part shown, articulator shown, articulator page, main page |
| 0x11e8 + 0xb70 p | part | part p, below |
| 0x28c8 | i32[6] | FX unit order |
| 0x28e0 | {fn, ctx}[6] | the FX call list (`FUN_00892e10`); count - 1 at 0x2940 |
| 0x2944 / 0x2948 / 0x294c | i32 | FX tail counter / its reset value / tail alive this block |
| 0x2958 / 0x2968 / 0x2a98 | ptr | chorus `TStereoFlanger`, delay `TDelay_32FS`, reverb `TFruityReverb` (null when off) |
| 0x2cc0 | i32 | distortion mode |
| 0x2ce0 | ptr | embedded plugin host (compressor) |
| 0x2cfc | i32 | 1 while restoring |
| 0x2d00 | u32 | saved-sections mask (bit 0 part A, bit 1 part B, bit 2 globals) |

### Part object (`part = this + 0x11e8 + 0xb70 p`)

| offset | type | meaning |
|---|---|---|
| +0x8 | i32 | REC base (0 or 386) |
| +0xc | u8 | part enabled |
| +0x14 / +0x18 / +0x1c | f32 | time-domain gain, frequency-domain volume, part-mix gain (section 3.11) |
| +0x28 / +0x30 / +0x38 | f32 | pitch offset sum, multiplier pitch, main pitch (cents) |
| +0x40 / +0x20 | f64 / f32 | harmonic detuning D, -log2 D |
| +0x5c | i32 | image kind + 1 (0 none, 1 drawn or embedded, 2 resynthesised) |
| +0x60 | i32 | timbre blending mode |
| +0x64 | i32 | sub-harmonic mode (0 around, 1 below) |
| +0x68 | f32[4] | self-oscillation level, sub 1, sub 3, sub 4 |
| +0xc0 | i32 | clipping mode |
| +0xe0 | i32 | partial pitch distribution (0 oct, 1 Hz) |
| +0xe8 | f64[520] | partial ratio table |
| +0x100 / +0x104 | i32 | prism multiply mode / prism from volume |
| +0x130 | ptr | `TToneBMP` |
| +0x138 | string | image source file |
| +0x23c | u8 | mid/side (`MidSideBtn`: negate the R output gain) |
| +0x240 | i32 | resynthesis mode (`BMPResynthModeSelect`) |
| +0x244 | i32 | resynthesis denoise (`BMPDenoiseWheel`) |
| +0x278 | u8 | pluck to blur |
| +0x3e8 + 0xc0 f | | filter f record (section 3.6.4) |
| +0x568 | | phaser shape record (type at +0x570 = 200 + item) |
| +0x660 | `TDeclicker` | |
| +0x668 / +0x66c | i32 | declick time, ramp time |
| +0x670 + 8 a | ptr[69] | 43 `TOpArticulator`, then 26 shape curves |
| +0x8a0 / +0x920 | i32[] | articulator evaluation lists (main 0..30, pre-pass 31..42), -1 ends |
| +0xa6c | i32[12] | spectral unit order |
| +0xaa0 | {fn, part}[12] | the unit call list (`FUN_008737e0`); count - 1 at +0xb60 |
| +0xb64 | i32 | which filter runs first |
| +0xb68 / +0xb69 | u8 | local EQ is first (baked into the timbres) / harmonizer variant |

## 1. State layout (event 213)

`SaveRestoreState` (`FUN_0086f2f0`), the part reader `FUN_0086ed30`:

```
save:    Write(u32 0x00130003)                      // version 19 in the high word, 3 in the low word
         Write(u32 mask = this+0x2d00)              // 7: part A, part B, globals
         for p in 0, 1 with mask bit p:
           Write(u32 size)                          // patched afterwards: bytes of the part that follow
           Write(params of part p, 386 x i32)
           Write(part+0xa6c, 12 x i32)              // spectral unit order
           Write(part struct, 0x4b bytes)
           43 x TOpArticulator stream, 26 x shape stream
           TToneBMP stream (FUN_00793300)
           ansi string: image source file
         if mask bit 2:
           Write(this+0xd48, 53 x i32)              // REC 772..824
           Write(this+0x28c8, 6 x i32)              // FX unit order
           Write(global struct, 0x3d bytes)
           wide string: InfoEdit text (preset name, author, info lines, CR separated)
           Write(u8 flags)                          // bit 0 this+0x1180 (KbBtn), bit 1 VisFBScaleSelect
restore: Read(u32 v); v == 0xffff or v >= 0x130004: ignore the state
         v > 0xffff: v >>= 16                       // 11, 12, 16 and 17 are stored plain
         Read(u32 mask); mask & 3 == 3: parts 0 and 1; == 1 or 2: the one part goes into the part shown (this+0x1184)
         per part: Read(u32 size); if this+0x2d04 is set and the part is the one shown, seek size bytes; else
           v < 16: Read 385 params and insert 0 at k = 376    // articulator 33 is new in 16
           v < 12: k = 343..385 rescaled (below)
           Read 12 x i32 order; Read struct (v < 11: 0x47 bytes, the last int absent)
           43 articulators (v < 16: articulator 33 is not in the stream; FUN_00896fe0 gives it defaults), 26 shapes, image, file name
         mask & 4: 53 ints, 6 ints, 0x3d bytes; v < 18: i32 n + n ansi bytes, else wide string; v > 18: u8 flags
         then FUN_008b8f20, FUN_00892e10 (FX list), FUN_008974d0, per part FUN_008737e0 (unit list) and FUN_00878e40,
         FUN_00878690, every param re-sent through ProcessParam (flags 0x11), per part FUN_0088f310 for -1, 0, 1
         (phase table, timbres), FUN_0088fbb0, FUN_0088f880
```

Smoothing rescale for v < 12: the old curve was `(41^(v/1000) - 1)·37.5`
ms, the new one is `(2^(0.009 v) - 1)/51.1` s, and the loader maps
`v' = round(log2(1 + 1.91625·(41^(v/1000) - 1))·111.11)` (constants at
0x86f2c8: 41, 1, 37.5, 0.0511, 111.11; `code`).

Strings: ansi = `u8 n` (0xff: `u32 n` follows) then n bytes, trailing
control characters trimmed (`FUN_0077d050`); wide = the same length prefix
counting UTF-16 units (`FUN_0077d190` / `FUN_0077d320`).

Sizes: a v18 state with nothing drawn is 84522 bytes = 8 + 2 x (4 + 42060) +
386; a part without image or file name is 42060 bytes (1544 params, 48
order, 75 struct, 40332 of curves, 60 image header, 1 file name). Larger
states carry more envelope points or an embedded image.

### Byte map of one part

| offset in part | size | field | corpus | confidence |
|---|---|---|---|---|
| 0 | 4 | size of the rest of the part | 42060 when nothing is drawn | code |
| 4 + 4 k, k = 0..94 | 4 | direct parameter k (section 2) | | code, dfm |
| 4 + 4 k, k = 95..342 | 4 | slot of the articulator quick knobs; ProcessParam stores those knobs inside the envelope objects, so the slot is never written | 0 in all 428 blocks | code, corpus |
| 4 + 4 k, k = 343..385 | 4 | articulator a = k - 343 output smoothing (`ArtSmoothWheel`, 0..1001) | 0 in 8582 of 8600; else 89..644, 1001 | code |
| 1548 | 48 | spectral unit order, a permutation of 0..11 | identity in 194 of 200 parts | code, corpus |
| 1596 | 75 | the part struct (next table) | | code |
| 1671 | var | 43 articulators | | code |
| | var | 26 shapes | | code |
| | var | image | | code |
| | var | image source file | 9 parts | code, corpus |

Part struct (0x4b bytes; editor handler names in parentheses):

| struct offset | -> part | control | values | corpus (200 parts) |
|---|---|---|---|---|
| 0 | +0x60 | `HarmBlendSelect` (`HarmColSelectChange` 0x8b6fb0) | 0 fade, 1 subtract, 2 multiply, 3 maximum, 4 minimum, 5 pluck | 0 everywhere |
| 4 | +0x668 | `DeclickWheel` (0x8b9030) | 0..1000 | 0 everywhere |
| 8 | +0x66c | `RampWheel` (0x8b1a00) | -2000..2000, default 1000 | 1000 mostly; 500, 768, 224, 1256, 1512, 144, 280 |
| 12 | +0x100 | `GrittyModeBox` prism mode | 0 add, 1 multiply | |
| 16 | +0x104 | `GrittyFromVolSelect` | 0 off, 1 on, 2 inverted | |
| 20 | +0xc0 | `HarmClipModeSelect` (0x8b85c0) | 0..4 | |
| 24 | +0x64 via `FUN_0088ffe0` | `SubVolModeSelect` | 0 around fundamental, 1 below | |
| 28 | +0xe0 | `HarmPitchModeSelect` partial pitch distribution | 0 oct, 1 Hz (default) | 0 in 2 parts |
| 32 | +0x240 | `BMPResynthModeSelect` | 0 generic, 1 high precision | |
| 36 | +0x244 | `BMPDenoiseWheel` | default 250 | |
| 40 | +0x570 | `PhaserTypeSelect` | 200 + item | 200 x112, 206 x6, ... |
| 44 | +0x1b4 | `BMPViewScaleSelect` | editor only | |
| 48 | +0x2c8 | `GlobalEQMixWheel` | the f32 v/1000 itself, not an int | 1.0 in 197 of 200 parts; 0.999, 0.796, 0.661 |
| 52, 56 | +0x3f8, +0x4b8 | `F1TypeSelect`, `F2TypeSelect` | filter ID 0..18 | (4, 18) x67, (0, 18) x17, (0, 0) x8, (0, 3) x8, (17, 18) x7 |
| 60, 64 | +0x430, +0x4f0 | `F1ResTypeSelect`, `F2ResTypeSelect` | 100 + item | (108, 111) x90, (100, 111) x9, (100, 100) x8 |
| 68 | u8 +0xc | part enabled | | A on in 98, B on in 30 of 100 states |
| 69 | u8 +0x23c | `MidSideBtn` | | |
| 70 | u8 +0x278 | `PluckToBlurBox` | | |
| 71 | i32 +0x238 (v11+) | `AutoGainModeSelect` | 0 absolute, 1 relative | |

Combo values count items without the "-" separator lines (`F1TypeSelectChange`
0x8af210; checked against the compression files 1..4 and the hints).

### Curves

`TOpArticulator` stream (vtable+0x10 = `FUN_00897ae0` -> `FUN_007a4370`):

```
u32 format (3); f32[3] at art+0x20                      // (1, 1, 1) x3711, (0, 1, 1) x1793 in the corpus
TMEnvelope stream (kind 1), TMLFO stream (kind 2), then 9 x TMMap stream (kind 3):
   keyboard, velocity, mod X, mod Y, mod Z, random, unison index, held index, mod X speed
```

The curve stream is the shared `TCustomMEnvelope` grammar of
`crates/plugins/src/menvelope.rs` (`FUN_0079a810`): `u32 kind; u32 version;
u32 n; n points (f64 x, f64 y, f32 tension, u8 x4); u32 flags; i32 active;
i32 decay; i32 loop start; i32 loop end`; the envelope adds `i32[4]` (the
quick knobs at +0x98: attack, decay, sustain, release), the LFO adds `i32[4]`
(+0xd0: speed, tension, skew, pulse width) and an `i32` phase (version 3).
All 402952 curves in the corpus and presets are version 3. The version word
is read only when the stream's kind matches the object's or the object's kind
is 0. Shapes (`FUN_0079abf0` -> vtable+0x10) are a bare `TMMap` stream (kind
3, active -1 in all 22256); `TMEQMap` (`FUN_007f0180`) reads the same bytes
and rescales x afterwards (`FUN_007f0320`), so it has no extra fields.

Articulator order (index a). The renderer reads each output at the offset
for this index; every entry was matched to the parameter it modulates.

| a | target | a | target | a | target |
|---|---|---|---|---|---|
| 0 | panning | 15 | pluck amount | 30 | image freq pixel scale |
| 1 | volume | 16 | phaser mix | 31 | filter 1 envelope amount |
| 2 | FX / dry mix | 17 | phaser width | 32 | filter 2 envelope amount |
| 3 | timbre 1 & 2 mix | 18 | phaser offset | 33 | global envelope attack time (v16+) |
| 4 | filter 1 frequency | 19 | harmonizer mix | 34 | global envelope attack scale |
| 5 | filter 2 frequency | 20 | harmonizer width | 35 | global envelope decay scale |
| 6 | filter 1 width | 21 | harmonic clipping threshold | 36 | global envelope sustain offset |
| 7 | filter 2 width | 22 | harmonic blur amount | 37 | global envelope release scale |
| 8 | filter 1 resonance amount | 23 | prism amount | 38 | global LFO amount |
| 9 | filter 2 resonance amount | 24 | pitch | 39 | global LFO speed |
| 10 | filter 1 resonance width | 25 | pitch vibrato depth | 40 | global LFO phase |
| 11 | filter 2 resonance width | 26 | unison pitch thickness | 41 | unison phase |
| 12 | filter 1 resonance offset | 27 | image time offset | 42 | predelay |
| 13 | filter 2 resonance offset | 28 | image fine speed | | |
| 14 | filter 1 & 2 mix | 29 | image formant shift | | |

The editor shows the envelope and LFO tabs only for a < 31 (`FUN_00898b20`);
articulators 31..42 still save both. Shapes (index s, at part+0x670 + 8 (43 +
s); the table each one builds in parentheses): 0 local EQ (part+0x80), 1
harmonic phase (+0xf0), 2 / 3 timbre 1 / 2 harmonic level, 4 harmonic
randomness (+0xa0), 5 harmonic protection (+0x90), 6 harmonic clipping
(+0xb0), 7 harmonic prism (+0xf8), 8 harmonic unison pitch (+0x70), 9 image
formant mutation (+0x78), 10 / 11 filter custom shape 1 / 2, 12 / 13
resonance custom shape 1 / 2, 14 pluck shape (+0x258), 15 phaser custom
shape, 16 harmonizer distribution (+0x5dc), 17 global EQ (+0x298), 18..22
masks of filter 1, filter 2, resonance 1, resonance 2, phaser, 23 harmonizer
local mask (+0x610), 24 blur local mask (+0x280), 25 image freq local mask
(+0x180).

Shape centres: the constructor (`FUN_00870750`, mask 0x00019490) gives
shapes 1, 4, 7, 9, 12, 13 and the phaser custom shape (when the phaser type
is Freq) a centre of 0.5; formulas below write `s' = s - 0.5` for them.

Articulator defaults (`FUN_00896fe0`): mask `0x7d27986f035` makes a = 0, 2,
4, 5, 12..15, 17, 18, 23, 24, 27..30, 33, 36, 38..42 bipolar (mode 1, centre
0.5); mask `0x6c050804004` forces mode 2 and maps flat at 1.0 for a = 2, 14,
23, 28, 30, 38, 39, 41, 42; the rest are mode 0 with centre 0. a = 1 loads
`System/Volume.fnv` as its envelope, a = 25 gets a fade-in envelope; LFO
default speed 0.5417 for a = 24, 0.625 for 4 and 5, 0.75 otherwise.

### Image (`TToneBMP`, `FUN_00793300`)

```
u32 version (0)
0x2c bytes: i32 type (+0x20: 0 drawn or embedded, 1 resynthesised from a file), i32 loop mode (+0x58: 0 one-shot, 1 loop, 2 ping-pong),
            i32 sustain normalisation (+0x100), f64 loop start column (+0xe8), f64 loop end column (+0xf0), f64 crossfade 0..0.5 (+0xf8),
            i32 columns (+0xcc), i32 rows (+0xc4)
type 0: 3 planes (gain, frequency, phase/link): i32 n (0 or columns x rows), then columns x (rows x f32)
```

Loop columns are -1 when unset ((-1, -1, 0) in 828 of 838 blocks). Corpus
parts: 182 with no image (type 0, 0 x 0); 9 embedded images (4 x 235 to 473
x 255; gain and frequency planes in 6, gain only in 3); 9 type-1 references to
`.wav` / `.ogg` files under `%FLStudioFactoryData%` and `%FLStudioData%`
(five vocal samples of "Square a Saw - Lightning", one of "nucleon - Fruity
Delay 3", three Harmor preset samples), which FL resynthesises on load
(section 3.10).

### Global block

53 ints = REC 772..824 (section 2.3), six ints = the FX unit order
(`FXUnitOrderSelect`, a permutation of 0..5; identity in 96 of 100 states, any
value outside 0..4 counts as the compressor), then the 0x3d-byte struct:

| struct offset | source | control | meaning | corpus (100) |
|---|---|---|---|---|
| 0x00 | this+0x11a8 | `VisFBBtn` | visual feedback (ignored on load) | 0 |
| 0x04 | this+0x11c0 | `VisFBScaleSelect` | spectrogram scale (ignored on load) | 0 |
| 0x08 | this+0x10d4 | `MonoSelect` | 0 off, 1 kill same, 2 kill all, 3 release all | 0 x96, 2 x3, 1 x1 |
| 0x0c | this+0x1094 | `VoiceRandSelect` | 0 off, 1 per voice, 2 per part voice, 3 per unison voice | 1 x96, 0 x3, 3 x1 |
| 0x10 | form+0xd58 | `QLocalMaxNumTonesWheel` 3600..10800 | Nmax = clamp((ceil(2^(v/1200)) + 11) & ~7, 8, 520) | 10800 x99 |
| 0x14 | form+0xcf0 | `QSafetyWheel` 0..2400 (75) | ceiling `this+0x10cc = v/1200` octaves; >= 2400 or HQ: 10 | 75 |
| 0x18 | form+0xcf8 | `QGrainWheel` 50..300 (200) | control tick (section 3.1) | 200 x99 |
| 0x1c | form+0xd00 | `QPrecisionSelect` | 0 average, 1 high, 2 perfect | 0 x90, 1 x6, 2 x4 |
| 0x20 | this+0x1184 | `PartSelect` | editor only | |
| 0x24 | this+0x1188 | `ArtSelect` | editor only | |
| 0x28 | this+0x118c | `EnvSelect` | editor only | |
| 0x2c | this+0x1194 | `MainPageSelect` | editor only | |
| 0x30 | this+0x2cc0 | `DistModeSelect` | section 4.2 | 0 x59, 5 x9, 6 x9, 1 x7, 7 x7, 2 x4, 3 x2, 4 x2, 9 x1 |
| 0x34 | form+0x730 | `CompTypeSelect` | 0 off, 1 limiting, 2 warming, 3 heating, 4 burning | 0 x42, 2 x26, 3 x15, 1 x9, 4 x8 |
| 0x38 | u8 this+0x1098 | `VoiceSameMapRandBox` | one random value for every articulator | 0 x98 |
| 0x39 | u8 this+0x1128 | `ModSmoothBox` | mod X/Y/Z smoothing | 1 x97 |
| 0x3a | u8 this+0xe44 | `QThreadedBox` | worker threads | 1 x100 |
| 0x3b | u8 form+0xd08 | `QHQRenderBox` | offline render forces precision 2, safety 10, 520 partials, 0.5 ms tick | 1 x100 |
| 0x3c | u8 form+0x288c | `LinkPartsBox` | editor edits both parts | 0 x99 |

After the struct: the InfoEdit text and (v19) the flag byte, 1 in all 44 v19
states. Nothing in the global block is version-gated.

## 2. Parameter semantics (`ProcessParam` @ 0x00879540, decoder `FUN_00879380`)

825 REC indices (`i < 0x339`). Flags as in every Delphi plugin here: 0x20
rescales with the control's Min/Max and 2^-30, 1 stores (under the host
lock), 2 reads, 4 formats the hint, 0x10 refreshes the control.

```
i < 772:  part p = i / 386, k = i - 386 p
   k < 95:           direct part parameter k = DFM Tag k, stored at this+0x138+4i; converted below
   95 <= k < 343:    articulator knob a = (k - 95) / 8 (a = 0..30), j = (k - 95) % 8
                     j < 4: TMEnvelope+0x98+4j, then FUN_007a2c10 (scale = knob/128: attack, decay, release 0..256 = 0..2x time; sustain -128..128 = offset -1..1)
                     j >= 4: TMLFO+0xd0+4(j-4) (speed, tension, skew, pulse width), then the LFO's vmt+0x40
                     (the param array slot stays 0; the knob is saved in the curve stream)
   343 <= k < 386:   articulator a = k - 343 smoothing, stored at this+0x138+4i:
                     v >= 1001: freeze (coefficient 1); else t = (2^(0.009 v) - 1)·SR/(T·51.1) ticks (v = 1000: 10 s),
                     coefficient art+0xa8 = t > 0 ? exp(ln 0.01 / t) : 0          (FUN_008980b0)
i >= 772: global t = i - 772: Tags 10000..10016 (t = 0..16), then 20000..20035 (t = 17..52)
```

### 2.1 Part parameters (k = DFM Tag)

DFM gives range and reset default; "corpus" counts the 128 enabled parts that
hold a value other than that default. Conversions are `code` from the
ProcessParam case unless marked; "direct" means the DSP reads the stored int.

| k | control | range (default) | corpus | conversion -> field | section |
|---|---|---|---|---|---|
| 0 | `TPhaseWheel` (untagged) starting phase | 0..999 (500) | 109 | part+0x124 = v/1000 | 3.9 |
| 1 | `TPhaseRandWheel` | -1000..1000 (0) | 25 | part+0x120 = sign(v)(v/1000)^2 | 3.9 |
| 2 | `PredelayWheel` | -384..384 (0) | 0 | part+0x1a0 = SR·(v < 0 ? (2^(-v/48) - 1)/255 : v·0.3125/BPM) | 3.2 |
| 3 | `PolyRelMulWheel` | 0..1000 (1000) | 1 | part+0x1a4 = v/1000 | 3.2 |
| 4 | `HMMixWheel` timbre 1 & 2 mix | 0..1000 | 28 above 0 | part+0x290 = v/1000 | 3.5 |
| 5..7 | `Sub1Slider`, `Sub3Slider`, `Sub4Slider` | 0..2048 (0) | 6 | part+0x68[1..3] = v/1024 | 3.5 |
| 8 | `SafetyVolSlider` protection | 0..1024 (256) | 128 | part+0x230 = level11(v/1024) | 3.6.9 |
| 9 | `HarmClipVolSlider` threshold | 0..1024 (1024) | 10 | part+0x248 = v/1024 | 3.6.10 |
| 10 | `PartFXMixSlider` FX / dry | -1000..1000 (0) | 13 | direct: m = v/1000·art[2] | 3.11 |
| 11 | `PartVolSlider` time-domain volume | -2000..2000 (1000) | 29 | part+0x14 (section 3.11) | 3.11 |
| 12 | `PartVolEnvSlider` frequency-domain volume | 0..2000 (1000) | 8 | part+0x18 = level3(v/1000) | 3.6.11 |
| 13 | `AutoGainWheel` | 0..1000 (0) | 14 | part+0x234 = (v/1000)^2 | 3.7 |
| 14..16 | `VelToAttTimeBox`, `VelToAttLengthBox`, `VelToRelLengthBox` | 0, 1 | 2, 0, 0 | direct | 3.2, 3.3 |
| 17 | `TremAmtWheel` | 0..1000 (0) | 9 | part+0x624/0x628 = v/1000 | 3.11 |
| 18 | `TremTimeWheel` | 0..10000 (4000) | 11 | part+0x64c = 2^(6 - v/1000)/SR | 3.11 |
| 19 | `TremGapWheel` | 0..999 (500) | 5 | part+0x650 = frac(v/1000)·π | 3.11 |
| 20 | `BlurMixWheel` | 0..1000 (1000) | 8 | part+0x27c = (v/1000)^2 | 3.6.8 |
| 21, 22 | `TimeBlurAttWheel`, `TimeBlurDecWheel` | 0..1600 (0) | 1, 0 | part+0x270/0x274 = exp(ln 0.01/N), N = (2^(v/100) - 1)·0.001·SR/T; 0 when N <= 0 | 3.6.8 |
| 23 | `HarmBlurAmtWheel` | 0..10000 (0) | 7 | direct | 3.6.8 |
| 24, 25 | `HarmBlurLTnsWheel`, `HarmBlurRTnsWheel` | 0..1000 (500) | 1 | ln and reciprocal of A = 1000001^(v/1000) at part+0x260/0x264, 0x268/0x26c | 3.6.8 |
| 26 | `TGrittinessWheel` prism | -1000..1000 (0) | 23 | part+0x114 = v/1000 | 3.6.2 |
| 27 | `SFXMixWheel` harmonizer mix | 0..1000 (0) | 35 | direct | 3.6.4 |
| 28 | `SFXWidthWheel` | 0..1000 (500) | 36 | part+0x5c4 = v/1000 | 3.6.4 |
| 29 | `SFXStrengthWheel` | 0..1024 (DFM 0, corpus 256) | | part+0x5cc = 1.331·2^(v/256) | 3.6.4 |
| 30..33 | `SFXPat1..4Wheel` shift offset, shift step, gap offset, gap step | -2..7, -7..8, -1..8, -7..8 | | direct | 3.6.4 |
| 34 | `FatOrderSelect` unison order | 1..9 (1) | 77 above 1 | section 3.8 | 3.8 |
| 35 | `FatTypeSelect` unison distribution | 0..4 | | part+0x1b8: 0 classic, 1 blurred, 2 uniform, 3 random, 4 Hz | 3.8 |
| 36 | `FatInvBox` alternate | 0, 1 | 0 | direct | 3.8 |
| 37 | `FatPanWheel` | 0..256 (192) | 79 | part+0x1d8 = v <= 192 ? v/192 : 1 + 0.046875(v - 192) | 3.8 |
| 38 | `FatPitchWheel` thickness | 0..256 (128) | 88 | part+0x1c0 = (41^(v/256) - 1)·1.875 cents | 3.8 |
| 39 | `FatPhaseWheel` | -1000..1000 (500) | 47 | part+0x1c4 = v/1000 | 3.9 |
| 40, 41 | `PartPitchMulSelect`, `PartPitchDivSelect` | 10000..160000 (10000), 1..9 (1) | 12, 30 | part+0x30 = 1200 log2(mul/(div·10000)) - 1200 (resynthesised image: + bmp+0x148 instead of - 1200); part+0x28 = part+0x38 + part+0x30 (`FUN_00873c60`) | 3.4 |
| 42, 43 | `DetuningMulSelect`, `DetuningDivSelect` | same | 4 | part+0x40 = D = mul/(div·10000); part+0x20 = -log2 D; rebuild flag part+0x110 | 3.4 |
| 44 | `PitchEnvAmtWheel` | -6000..6000 (6000) | 1 | part+0x10 = 2v | 3.4 |
| 45 | `VibratoAmtWheel` | 0..1000 (0) | 52 | part+0x54 = 0.1 v cents | 3.4 |
| 46 | `VibratoTimeWheel` | 0..1000 (500) | 39 | part+0x50 = 2^(3 - v/1000)/SR (500: 5.66 Hz) | 3.4 |
| 47 | `PortaCurveBox` | 0, 1 | | direct (0: linear glide) | 3.2 |
| 48 | `PortaVarTimeBox` | 0, 1 | 0 | direct | 3.2 |
| 49 | `PortaTimeWheel` | -384..384 (-140) | 20 | part+0x58 (section 3.2) | 3.2 |
| 50 | `PortaLimitWheel` | 0..1200 (1200) | 0 | direct | 3.2 |
| 51 | `F1EnvAltBox` adaptive envelope | 0, 1 | | byte part+0x4a4 | 3.6.5 |
| 52 | `F1CutWheel` | 0..8192 | | part+0x488 = lo + v/8192·range | 3.6.5 |
| 53 | `F1HzSlopeSelect` scale | 0 oct, 1 Hz | 1 | direct | 3.6.5 |
| 54 | `F1WidthWheel` | 0..1000 (500) | 41 | part+0x494 = 0.004 v | 3.6.5 |
| 55 | `F1KbTrackWheel` | -2000..2000 (1000) | | part+0x484 = v/1000 | 3.6.5 |
| 56 | `F1EnvAmtWheel` | -1000..1000 | | part+0x464 = v/1000 | 3.6.5 |
| 57 | `F1ResWheel` resonance / noise amount | 0..1000 (0) | 53 | direct: r = v/1000·art[8] | 3.6.5 |
| 58 | `F1ResHzSlopeSelect` | 0 oct, 1 Hz | 5 | direct | 3.6.5 |
| 59 | `F1ResWidthWheel` | 0..1000 (500) | 27 | part+0x498 = 0.004 v | 3.6.5 |
| 60 | `F1ResCutOfsWheel` | -2400..2400 | | part+0x48c = 2^(v/1200), part+0x490 = v/1200 | 3.6.5 |
| 61 | `F1ResAWidthBox` adaptive width / free noise | 0, 1 | 0 | direct; part+0x49c (`FUN_008744c0`) | 3.6.5 |
| 62 | `F1OscWheel` self-oscillation | 0..2048 (0) | 2 | part+0x68[0] = (v/1024)^2 | 3.7 |
| 63..73 | filter 2: the same as 51..61 with record offset +0xc0 (no self-oscillation) | | | | 3.6.5 |
| 74 | `FilterTypeMixWheel` parallel - serial | -1000..1000 | | direct: m = clamp(v/1000·art[14], -1, 1) | 3.6.5 |
| 75 | `PluckDecWheel` | 0..1000 (500) | 28 below 1000 | part+0x250 = v/1000 (f64) | 3.6.7 |
| 76 | `PMixWheel` phaser mix | 0..1000 (0) | 27 | direct | 3.6.6 |
| 77 | `PHzSlopeSelect` | 0..2 | 24 | direct | 3.6.6 |
| 78 | `PWidthWheel` | 0..10000 (5000) | 38 | part+0x5c0 = v/10000 | 3.6.6 |
| 79 | `POfsWheel` | 0..999 (500) | 20 | direct | 3.6.6 |
| 80 | `PMotionSpeedWheel` | -8000..8000 (3000) | 43 | part+0x5a8 = sign(v)·2^(|v|/1000)·part+0x5b0 per sample | 3.6.6 |
| 81 | `PKbTrackWheel` | -2000..2000 (1000) | | direct | 3.6.6 |
| 82 | `BMPTimeSmoothWheel` | -1000..1000 (500) | 11 | part+0x154 = exp(ln 0.01/(x^2·2·SR/T)), x = |v|/1000, 0 at v = 0; jump threshold part+0x150: v = 0 always jump, v < 0 never, v > 0 x^2·250/cols | 3.10 |
| 83 | `BMPTimeWheel` | 0..1048575 | 14 | part+0x148 = v/2^20 | 3.10 |
| 84, 85 | `BMPSpeedWheel`, `BMPSpeedMulWheel` | -20000..20000 (10000), -60000..60000 | 15, 5 | part+0x140 = (p84/10000)·2^(p85/10000)·bmp+0x138 columns per tick | 3.10 |
| 86 | `BMPPhasinessWheel` sharpening | -1000..1000 (750) | 16 | part+0x158 = (v/1000)^2, part+0x170 = sign(v) | 3.10 |
| 87 | `BMPSmoothGainSelect` | 0..6 | | direct (curve index) | 3.10 |
| 88 | `BMPGainScaleWheel` | 0..1000 (500) | 0 | bmp+0x28 = 2^(0.016 v) - 1 (255 at 500) | 3.10 |
| 89 | `BMPGainMixWheel` | 0..1000 (1000) | 1 | part+0x178 = v/1000 | 3.10 |
| 90 | `BMPSubFreqModeSelect` | 0 oct, 1 Hz, 2 wide Hz, 3 unison | 17 | part+0x16c | 3.10 |
| 91 | `BMPSmoothSubFreqSelect` | 0..6 | | direct | 3.10 |
| 92 | `BMPSubFreqScaleWheel` | -2000..2000 (1000) | 10 | part+0x174 = s·ln 2 (oct), s (Hz), max(2^-30, (s + 2)·32) (wide), (s + 2)·0.125 (unison); s = v/1000, times bmp+0x104 | 3.10 |
| 93 | `BMPFormantShiftWheel` | -600..600 (0) | 6 | direct (cents) | 3.10 |
| 94 | `BMPFormantMixWheel` | 0..1000 (1000) | | part+0x160 = v/1000 | 3.10 |

### 2.2 Envelope quick knobs and smoothing

See the decoder above. The corpus has no articulator knob slot set in the
param array (they live in the curve streams) and 18 smoothing values set.

Ranges (`FUN_00879380` hands `ProcessParam` the control, whose Min and Max
REC_FromMIDI rescales by): a knob's control is `form+0x2848 + 8 j`, the
envelope editor's `EnvAttWheel`, `EnvDecWheel`, `EnvSusWheel`,
`EnvRelWheel`, `LFOSpeedWheel`, `LFOTensionWheel`, `LFOSkewWheel` and
`LFOPWWheel` (Tags 0..7): 0..256, 0..256, -128..128, 0..256, 512..65536,
-128..128, -128..128, -112..112. A smoothing's is `form+0x758`,
`ArtSmoothWheel` 0..1001. A knob is read from and stored into the
articulator's `TMEnvelope` (+0x98 + 4 j, then `FUN_007a2c10`) or `TMLFO`
(+0xd0 + 4 (j - 4), then its vmt+0x40), not the param array; a smoothing
into the array (this+0x138 + 4 i, then `FUN_008980b0`). The code
(`Harmor::set_param`, `harmor_state::KNOB_RANGES`) does the same; a moved
envelope knob is heard by the voices that start after it.

### 2.3 Global parameters (REC 772..824, at this+0xd48)

The loader copies them raw and re-applies them through ProcessParam;
distortion params act at once, reverb, chorus and delay params set a dirty
flag that the next block applies (`FUN_00891280`).

Every global's range is its control's (`form+0xe80 + 8 i`), in
`harmor_state::GLOBAL_RANGES`; a box is 0..1, `StrumModeSelect` 0..3,
`DelModeSelect` 0..2. Distortion: drive 0..1000, asymmetry -500..500, out
0..2000, mix -1000..1000, cutoff 500..1100. Reverb: low cut 0..1000, high
cut and damping 400..1050, predelay -384..384, room 1..100, diffusion
0..100, decay 0..900, colour 0..4, volume -1000..1000. Chorus: order 0..9,
depth and speed 0..1024, delay and spread 0..128, cross -128..128, wet
-1000..1000. Delay: feedback 0..1000, time -384..384, offset -512..512,
volume -1000..1000, pan -128..128, low cut 0..1000, high cut 400..1050.
Compression: volume 0..1000, low, mid and high 0..2000.

| REC | Tag / control | range (default) | conversion -> field | corpus (100) |
|---|---|---|---|---|
| 772..774 | 10000..10002 `ModX/Y/ZWheel` | 0..512 (256) | this+0x1108 + 4j = v/512; 0x1114 = a copy of X | 35, 34, 29 off 256 |
| 775 | 10003 `LegatoLimitWheel` | 0..2400 (0) | this+0x10e4 = v cents, 0 = no limit | 4 |
| 776 | 10004 `LegatoBox` | 0, 1 | TriggerVoice, XOR colour 13 | 22 on |
| 777 | 10005 `PortaBox` | 0, 1 | XOR colour 14; turning it on clears the per-colour records (`FUN_00890e20`) | 7 on |
| 778 | 10006 `VelToPortaTimeBox` | 0, 1 | glide time factor (section 3.2) | 0 |
| 779 | 10007 `StrumModeSelect` | 0 up, 1 down, 2 alternate, 3 random | this+0x112c | 0 |
| 780 | 10008 `StrumTimeWheel` | 0..1000 | this+0x1130 = SR·(41^(v/1000) - 1)·0.00625 samples | 0 |
| 781 | 10009 `StrumTensWheel` | -1000..1000 | this+0x1138 = 2^(v/1000) | |
| 782 | 10010 `PartMixWheel` | 0..1000 (500) | part+0x1c (section 3.11) | 19 |
| 783 | 10011 `MainVolWheel` pre-FX | 0..2000 (1000) | folded into part+0x14 | 8 |
| 784 | 10012 `PostVolWheel` | 0..2000 (1000) | this+0x109c/0x10a0 = level3(v/1000) | 5 |
| 785 | 10013 `VelToVolBox` | 0, 1 | 0: TriggerVoice calls SetLinkVelocity(0) so Final.Vol drops velocity | 87 on |
| 786 | 10014 `MainPitchWheel` | -2400..2400 cents | part+0x38 = v for both parts (`FUN_00873c60`) | 17 |
| 787 | 10015 `MainLFOLevelWheel` | -1000..1000 (1000) | this+0x10dc = v/1000 | 0 |
| 788 | 10016 `InvArpBox` | 0, 1 | this+0xd88; re-sorts held voices | 0 |
| 789..793 | 20000..20004 distortion: drive, asymmetry, out, mix, cutoff (ProcessParam reads this+0xd8c..0xd9c) | | section 4.2 | |
| 794..803 | 20005..20014 reverb | | section 4.5 | enable 39 |
| 804..810 | 20015..20021 chorus | | section 4.3 | order > 0 in 19 |
| 811..820 | 20022..20031 delay | | section 4.4 | enable 19 |
| 821..824 | 20032..20035 compression | | section 4.6 | |

## 3. The voice

### 3.1 Control rate (`FUN_00878690`, code)

| field | value |
|---|---|
| T (this+0xe30) | `max(4, 4·round(g·0.001·SR·0.25))`, g = QGrain/100 ms (default 2 ms: T = 88 at 44.1 kHz); 0.5 ms in HQ offline render (this+0xe3d) |
| this+0x10ec | `exp(ln 0.01/(0.04·SR/T))`, part pitch smoothing per tick |
| this+0x10e8 | `0.01^(T/(0.1·SR))`, filter cutoff smoothing per tick |
| this+0x10f4 / 0x10f0 | `round(0.1·SR)` / its reciprocal + 1: tremolo fade-in |
| this+0x1118..0x1120 | mod X/Y/Z smoothing `exp(ln 0.01/(0.1·SR/T))` when `ModSmoothBox`, else 0 |
| this+0x1124 | slow X for the "mod X speed" map, `exp(ln 0.01/(1.25·SR/T))` |
| this+0x10d8 | 1048576/SR, frequency clamp in oct spacing |
| part+0x164 / +0x168 | image refresh period: `max(1, round(176/T))` ticks / that times T samples |

Inside a tick, frequencies are constant and levels glide to their per-tick
targets through one-pole filters in the bank (section 3.9).

### 3.2 Voice lifecycle (code)

Voice record: 0x3348 bytes (`FUN_00883ae0`, or reused from the free list).

| off | meaning |
|---|---|
| +0x00 | `TVoiceParams*` (Final: Pan +0x14, Vol +0x18, Pitch +0x1c, FCut +0x20, FRes +0x24) |
| +0x08 | velocity `min(GetVelocity, 1)` |
| +0x10 | host voice handle |
| +0x20 / +0x28 / +0x2c | held key / held-note index / legato group |
| +0x30 | render buffer set |
| +0x38 | kind: 0 sounding, 1 colour-15 control voice, 2 legato placeholder |
| +0x3c | samples to the next tick (negative: first tick) |
| +0x40 | steal stamp (MAX = not counted; release: clock - 2^63) |
| +0x48 / +0x4c | delay samples (predelay or strum, -1 strum pending) / 1 new, 2 inside predelay, 0 running |
| +0x58 | part |
| +0x64 | key pitch = Init.Pitch (colour 11: minus 100000) |
| +0x68..+0xb0 | glide: step, counter, A, B, C for pan, vol, pitch, fcut, fres, velocity |
| +0xec.. | declicker positions (attack, release) |
| +0x128 / +0x130 / +0x138 | unison chain: next child / leader / count |
| +0x140 | `TFastToneBank` |
| +0x148 / +0x14c | random seeds |
| +0x1ac..+0x1c4 | pitch P (cents), P + unison detune, f0 Hz, log2 f0, f0/SR, smoothed part pitch |
| +0x1a0 | mixed timbre levels |
| +0x214 | state: 1 playing (held means < 6), 6 released, 9 fading out, 10 done |
| +0x21c / +0x240 | stereo gain smoothers into the FX bus / the dry bus |
| +0x2a8..+0x2c8 | unison (section 3.8) |
| +0x2d0 + 0x120 a | articulator run slot a |
| +0x3334..+0x3340 | smoothed mod X, Y, Z, slow X |

TriggerVoice (`0x883cd0`):

1. Colour `c = GetColor & 0xf`, velocity, and `SetLinkVelocity(0)` when
   VelToVol is off. Colour 15 makes a silent control voice (`this+0xe80`);
   Gen_Render then sets `part+0x48 = log2 440 + (Pitch/100 + 27)/12 -
   part+0x47c`, an octave offset on filter 1's cutoff.
2. Legato when `(LegatoBox != 0) != (c == 13)`: `FUN_00890990` picks the
   sounding leader of the same colour with the closest Init.Pitch within the
   legato limit; a kind-2 placeholder takes over its group, `FUN_00890a40`
   swaps the record bodies, `FUN_00888a50` starts the glide, and the call
   returns.
3. Mono mode: for each sounding leader in state < 9 that has the same pitch
   (mode 1) or any pitch (modes 2, 3): mode 3 releases it, modes 1 and 2
   kill it (state 9).
4. MaxPoly (`this+0xe0`, ProcessEvent id 1): kill the smallest stamp when
   the counted voices reach it (released voices first).
5. Portamento when `(PortaBox != 0) != (c == 14)` and the colour's record at
   `this+0xe94 + 0x20c` holds a pitch.
6. Per enabled part: if PolyRelMul != 1, scale the release of every released
   voice once (`FUN_007a0700(run, 3, scale·part+0x1a4)`); then create
   `part+0x1bc` unison voices (section 3.8), each initialising its 43
   articulator slots (`FUN_00897b50`, `FUN_00897c70`); VelToAttScale (k15)
   on and the volume envelope on: attack scale `*= 2 - 2 vel`. The first voice
   of the note is the leader, the rest chain to it.
7. After both parts: portamento glide `FUN_00888a50(.., porta 1, fresh 1)`,
   store the note's Final levels in the colour record (`FUN_00890960`), set
   `this+0xe91` for the strum pass.

Voice_Release (`0x885390`), leader only: glide factor from the release
velocity, return-swap to a held legato placeholder if one exists, remove the
held key; a sounding voice goes to state 6, each main-list envelope gets its
release scale times `f = VelToRelLength ? 2(1 - relvel) : 1` and enters
release (`FUN_007a09f0`), stamp `clock - 2^63`, unison children released.
Kind 1 and 2 voices are killed. Voice_Kill (`0x885730`) unlinks and frees.

End of a voice: in state 6..8 the voice ends when the volume articulator's
envelope run reaches its end and its smoothed output has settled (or at once
when the volume envelope is off), and no arpeggiator envelope is still
running. `FUN_008856b0` sets state 9 and starts the 10 ms declicker fade-out;
state 10 follows when the fade is done, and Gen_Render asks the host to kill
the voice.

Glide (`FUN_00888a50`): time in ticks `t = part+0x58·voice+0x24`, with
part+0x58 = `SR(2^(-v/64) - 1)/(T·42)` for v < 0 (`PortaTimeWheel`; -140 =
0.085 s) or `SR·v·0.3125/(BPM·T)` for v >= 0, negated when `PortaCurveBox`
is 0 (linear). The velocity factor voice+0x24 is `(1 - 0.85 vel)^2` with
VelToPortaTime, else 1. Offsets: pitch, fcut and fres always; pan, vol and
velocity only for legato. Pitch limit (portamento only): 0 none, < 1200
clamp `|Δ|`, 1200 fold by octaves while `|Δ| > 1250`. `PortaVarTimeBox`:
`t *= |Δpitch|/1200`. Linear: `n = 1 - round(t)` ticks of equal steps.
Exponential: `c = exp(√2·ln 0.01/t)`, two cascaded poles per tick (`A *= c; B
= (B - A)c + A`, `FUN_008826f0`).

Strum and held index (Gen_Render): after a trigger, new voices are sorted by
key pitch (ascending for strum mode 0, flipped per chord for 2, random for 3)
and delayed by `slot - clock`, the slot advancing by the interval I
(`this+0x1130`), `I = min(I·tension, SR)` per note. New voices get held
indices max + 1, + 2, ... in that order. The arpeggiator rank (for envelopes
with arp points) re-sorts held pitches when the held list changes.

Predelay: on the first tick the voice skips `round(art[42]·part+0x1a0)`
samples.

Voice randomness (`VoiceRandSelect`): a buffer of `ippsRandUniform` values in
[0, 1), seeded from MT19937 (`FUN_007a9c30`) once per note; articulator a
reads `buf[min(a, len - 1)]` (length 1 with `VoiceSameMapRandBox`); redrawn
per unison voice (mode 3) or per part (mode >= 2). Mode 0 disables every
random map.

### 3.3 Articulators at runtime

Harmor's `TCustomMEnvelope`, `TMEnvelope`, `TMLFO`, `TMMap` and
`TArticulator` are Fruity Love Philter's code (every VMT slot lines up at
Harmor = LP + 0x1a730..0x1abc0; the tick `0x7a0c60` matches LP `0x786350`
in 3229 of 3280 bytes, the rest RIP operands). The segment shapes, tension
curve (`ln 1000001`), step counts, markers and LFO oscillator in
`crates/plugins/src/menvelope.rs` are therefore the right maths.

`TOpArticulator` fields: +0x08 mode (0 multiply, 1 add, 2 multiply by 2v),
+0x10 envelope, +0x18 LFO, +0x20 base, +0x30 + 8k map k = 2..10, +0x78
evaluation list, +0xa0 / +0xa4 random / unison map live, +0xa8 smoothing
coefficient. Precompute `FUN_00898200`: `base = (mode != 1)`; a flat map
folds in as `base ⊕= value - centre` (⊕ is `*=`, `+=` or `*= 2v` by mode);
the envelope, LFO and non-flat maps 2..6, 9, 10 go on the list; random and
unison maps fold in per voice at trigger (`FUN_00897b50`).

Evaluation per tick (`FUN_00897d90`, then `FUN_00898120` on the first tick):

```
acc = s[0] (base)
envelope: e = run.out - centre;  mode 1: acc += e·s[0x46]   else acc *= e·s[0x46]      (s[0x46] = 2 for mode 2)
LFO:      l = (run.level - 0.5)·osc(phase);  mode 1: acc += l·lfoAmt   else acc *= 2 l lfoAmt + 1
map k:    acc ⊕= map(in[k])
s[1] = acc + (s[1] - acc)·art+0xa8          (output smoothing)
inputs: key = (Final.Pitch + part+0x38 + glide + 6000)/12000; vel = voice+8 + glide; X = sX + (Final.FCut + glide)/2;
        Y = sY + (Final.FRes + glide)/2; Z = sZ; held = index/32; X speed = 1 - (1 - |sX - slowX|)^2;
        LFO speed = art[39]; lfoAmt = art[38]·MainLFOLevel; dt = 0 on the first tick, else 1
```

Order per tick in `FUN_0088ab10`: the pre-pass list (articulators 31..42),
then on the first tick the global envelope scales on every main-list
envelope (attack `*= art[34]`, decay `*= art[35]`, sustain `+= art[36]`,
release `*= art[37]`), the attack jump `FUN_007a2580(env, min(2·art[33], 1))`
(plus velocity for the volume envelope when VelToAttTime is on), LFO phase
`+= round(art[40]·2^32)`; then the main list (0..30). The pre-pass reads the
previous tick's dt, speed and LFO amount (stack order, `code` at 0x88b073).

Rate: `SetRate(halfsec = 2T/SR, beats = T·BPM/(60 SR))` (`FUN_00897710`):
one x unit is a half second, or a beat in tempo mode, per tick.

What Harmor's curve runtime has that `menvelope.rs` lacks: sustain weights
and the sustain-offset knob, FL's release (offset carried over the release
points, plays on when there is no loop end), the arpeggiator (stage 5, rank
and count from `FUN_007a24b0`), decay scale <= 0 freezing the loop, mode 10
neighbours across the loop wrap, live scale changes, shared envelope runs
(flag 2, `FUN_007a2cc0`), global LFO phase (`this+0x2cf0`), song-position
resync (`0x897390`), negative seek (predelay) and the attack jump.

The LFO level run has centre 0.5, checked in all three DLLs: `TMLFO`'s
constructor stores 0.5 at +0x18 before the base constructor runs (Harmor
`0x7a2dd0`, Love Philter `0x788410`, Sytrus `0x789fa0`), the stream reader
leaves +0x18 alone, and the articulators' mode setters re-centre only the
envelope and the maps (Love Philter `0x8544d0` and `0x789cc0`, Sytrus
`0x861300`). The tick hands on `(level - 0.5)·osc`. `Lfo::new` in
`menvelope.rs` used 0, which made every LFO twice as deep and turned it
over below 0.5; it now uses 0.5, which fixes Love Philter, Sytrus and
Harmor together.

An LFO never keeps a decay marker. The base constructor sets a class mask
at +0x20 to 0x3f (Sytrus `0x781a20`), and `TMLFO`'s constructor clears bit
8 of it (Sytrus `0x789fa0`, Love Philter `0x7884e0`). After the 20-byte
block the stream reader (Sytrus `0x781be0`) sets the decay marker (+0x4c)
to -1 when that bit is clear (the test at `0x781f53`), for every stream
version and whatever the stream's own flags say. No other class touches
the mask, so envelopes and maps keep their marker. `MEnvelope::read` does
the same for `Class::Lfo`. Two more bits of the mask sit in the same
reader: with 0x10 clear it zeroes each point's arpeggiator byte, and with
0x4 clear it calls `0x7849a0`. Every class leaves both set.

Envelope parts the volume path uses: the volume articulator (a = 1) is
`art[1]^2·part+0x18` on the partial levels (section 3.7). With its envelope
off the voice is a gate: full level while held, and it ends at release.

### 3.4 Pitch and partial frequencies (code)

```
sp  = first tick ? part+0x28 : part+0x28 + (sp - part+0x28)·this+0x10ec       // voice+0x1c4
P   = Final.Pitch + sp + glide pitch
if REC[45] != 0: P += sin(2π φ)·part+0x54·art[25]          // φ = part+0x4c, per part, += part+0x50 per sample
P  += part+0x10·art[24]                                   // pitch articulator (add mode, ±0.5 at full swing)
log2 f0 = log2 440 + (P/100 + 3)/12;  f0c = f0/SR          // P = 0 is MIDI 60 at 523.25 Hz; the default multiplier's -1200 makes it 261.63 Hz
a4  = voice+0x2b8·art[26]^2                                // unison detune, cents
```

Ratio table part+0xe8 (`FUN_0088fcf0`, rebuilt when part+0x110 is set):

| slot | ratio |
|---|---|
| 0 | filter 1 self-oscillation tone (replaced each tick, below) |
| 1 | 0.5 |
| 2, 3 | sub mode 0: 1 + 0.5 D and 1 + 1.5 D; sub mode 1: 0.25 and 0.125 |
| 4 + j | Hz spacing: 1 + j D; oct spacing: 2^(j D/8) |

Frequency array (bank+0xb8, f64 cycles per sample), when anything changed
(pitch, a4, prism, image refresh):

1. `D' = f0c`, times `2^(a4/1200)` when unison type < 4, no image frequency
   unison mode and no unison-pitch shape.
2. `F = ratio·D'`; oct spacing clamps to this+0x10d8.
3. Unison type 4 (Hz): `F_i += u_i·(a4/150)·f0c·S_i`, u_i random per partial
   (voice+0x2c8). Else with a unison-pitch shape (shape 8, `S = (2s)^2`,
   log sampler): `F_i *= 2^(a4·S_i/1200)`.
4. Image frequency plane (section 3.10), then the slot-0 tone: `Hz0 =
   2^cutoff1·part+0x48c`, `F[0] = Hz0/SR`.
5. `voice+0x178 = F/f0c` is kept so later ticks only rescale by f0c until the
   next refresh.

Hz arrays (`FUN_00890150` / `FUN_00890210`): `Hz = max(F·SR, 16.3516)`,
`log2 Hz`.

Partial count (`FUN_0088fbc0`, `FUN_00873d10`): top = part+0x1a8, the
frequency of the global EQ's last non-zero point (section 3.7); ceiling
`part+0x198 = top·2^(Q + part+0x20)` with Q = QSafety/1200 (10 for "full" or
HQ). Hz spacing: `n = round(part+0x198/f0)`; oct spacing: `(log2 top - log2
f0)·8/D`. Prism in multiply mode divides by `max(1 - |a|, 0.01)`. Then `+1 +
blur half-width`, at least the image's row count for resynthesised images,
+4 slots, at most Nmax and 520, rounded up to 8. N only grows during a note;
new partials get extrapolated phases (`FUN_007845a0`). There is no Nyquist
test: the guard is the global EQ ceiling (about 17 kHz) times the safety
margin, plus a taper `level[456 + j] *= (cos((j - 1)π/65) + 1)/2` for j =
0..63 when N > 456 (`FUN_00870610`).

### 3.5 Timbres and per-note values (code)

Timbre tables (`FUN_0088f310(part, 0 / 1)`): shapes 2, 3 on the linear
sampler (`x = (h - 1)·len/515`; subs at t·len/515 with t = 0.5 and 1.5, or
0 in below mode), `T[i] = (4s)^2`, slot 0 = 1, slot 1 = slot 4. Blend
(part+0x60) rewrites timbre 2: fade T2; subtract `max(T1 - T2, 0)`;
multiply `T1·T2`; maximum; minimum; pluck `max(T2·2^(16(s14 - 1)), 2^-16)`.
When local EQ is the first unit, both are multiplied by the EQ table here.
Results at part+0xc8 / +0xd0.

Mix (`FUN_00885b70`, only when x changes): `x = min(REC[4]/1000·art[3], 1)`
(pluck blend: x goes to the pluck stage and the mix uses 0); `L0 = x <= 0 ?
T1 : x == 1 ? T2 : (1 - x)T1 + xT2`; slots 0..3 times part+0x68 (self-osc
level, sub 1, sub 3, sub 4).

Harmonic randomness (shape 4): table `R = s'·8·ln 2` (`FUN_008895a0`,
trimmed where the shape sits at its centre); at note-on `voice+0xd8[k] =
2^(8 s'·U[0, 1))`; unit 2 multiplies by it.

### 3.6 Spectral units

Per tick (`FUN_0088ab10`), after the articulators and pitch:

1. Frequencies (3.4), on the first tick the phases (3.9).
2. L = the timbre mix (`voice+0x1a0`), or the image path (3.10).
3. Scratch W (`*(voice+0x30)`): filter mix `m`, `W+0x8c = |m|`, `W+0x80 =
   m != 1 ? sign(m) : 0`, `W+0x84 = part+0xb64`, `W+0xb4 = art[1]^2·part+0x18`,
   `W+0xb8 = 1`.
4. The unit list, then the tail (3.7).

#### 3.6.1 Unit order (`FUN_008737e0`)

| id | unit | function | built when |
|---|---|---|---|
| 0 | local EQ | `FUN_00886e00`: `L *= part+0x80` | id 0 not first (first: baked into the timbres) |
| 1 | prism | `FUN_00886aa0` | always |
| 2 | harmonic randomness | `FUN_00886e30` | part+0xac > 0 |
| 3 | harmonizer | `FUN_00885cb0` | always |
| 4, 5 | filter slot | `FUN_00886eb0`: runs filter W+0x84 and toggles it | always; part+0xb64 = (id - 4) ^ 1 of the later of the two |
| 6 | phaser | `FUN_00887f10` | always |
| 7 | pluck | `FUN_00888500` | pluck-to-blur off |
| 8 | blur | `FUN_00886610` | always (the editor's name list has no BLUR string) |
| 9 | protection | `FUN_00886110` | part+0x9c > 0 |
| 10 | clipping | `FUN_00886160` | part+0xbc > 0 |
| 11 | volume articulator | `FUN_008860c0`: `L *= W+0xb4`, `W+0xb8 = W+0xb4`, `W+0xb4 = 1` | dropped when last; the tail applies W+0xb4 |

Local EQ (shape 0, `FUN_00889ec0`): oct spacing `E = s^2`; Hz spacing
`E[4 + i] = (4s)^2/(i + 1)`, `E[1] = (4s)^2`, `E[2, 3] = (4s)^2/(t + 1)`.

#### 3.6.2 Prism (shape 7, REC[26], art[23]; `FUN_00886aa0`, `FUN_008899d0`)

`x = REC[26]/1000·art[23]`; add mode `a = sign(x)·4x^2`, multiply mode `a =
clamp(x, -1, 1)`. Table `P = 2s'` (log sampler, slot 0 = 0; multiply mode
without from-volume adds 1). Weight w = 1, `min(L, 1)` or `1 - min(L, 1)` by
`GrittyFromVolSelect`. Add: `F_n += f0c·D·a·w_n·P_n`; multiply: `F_n = f0c
+ (F_n - f0c)(1 + a·w_n·P_n)`. Invalidates the Hz arrays.

#### 3.6.3 Shape samplers

| sampler | x for slot 4 + i | used by |
|---|---|---|
| `FUN_00888ed0` (f32), `FUN_008890d0` (f64) | `log2(i + 1)·len/9` | local EQ, protection, clip, prism, pluck, unison pitch, local masks, formant mutation |
| map vmt+0x68 / +0x70 | `i/515·len` | timbres, phase, randomness |

#### 3.6.4 Harmonizer (REC[27..33], art[19], art[20], shape 16, mask 23; `FUN_00885cb0`)

```
mix = min(REC[27]/1000·art[19], 1);  w = min(part+0x5c4·art[20], 1);  both > 0 or skip
step = 1/(8w + 1);  t = 1 - step, falling by step while t > 0
a = b = 2;  pattern i (i = 1, 3 for REC[31], REC[33]): s < 0 -> mul 1 - s, add 0; else mul 1, add s
per iteration k: a = a·mul1 + add1, b = b·mul2 + add2
   g = min(t^3·S, 1)·mix^2·dist[k]        (t^2 when part+0xb69); S = part+0x5cc; dist = (3^(2 s16) - 1)/2 at 9 points
   start = a + REC[30], stride = b + REC[32]; L4[start + j·stride] += dry[j]·g   (skip g < 5.96e-8); Σ += g (Σ0 = 1)
   a and b move on after the copy, so the first copy is at 2 + REC[30] with stride 2 + REC[32] (code, FUN_00885cb0)
part+0xb69: Σ = sqrt Σ
L4 *= 1/Σ, or with mask M (shape 23 squared): L4 = dry + (L4/Σ - dry)·M
```

#### 3.6.5 Filters 1 and 2 (`FUN_00886eb0`, `FUN_00874240`, `FUN_008744c0`, `FUN_00874930`)

Per-filter record at part+0x3e8 + 0xc0 f: REC base (+12 for filter 2), shape
struct S at +0x3f0 (ID +8, centre +0x14, reach +0x18 / +0x1c, points per
octave +0x20, identity bit +0x34), resonance struct at +0x428, env amount
+0x464, lo / hi +0x468 / +0x46c (log2 Hz), range +0x470 (the cutoff code multiplies by +0x474 instead; 2·range fits the formula, `inferred`), key track +0x484,
cutoff base +0x488, res offset +0x48c / +0x490, width +0x494, res width
+0x498, res scale +0x49c, adaptive byte +0x4a4.

```
range:  u = (2/3)·2^(width - 1)·S.reach;  lo = log2 73.4162 - max(u_left - 1, 0);  hi = log2 17739.69 + max(u_right - 1, 0)
cutoff: kt = (log2 f0 - log2 130.8128)·keytrack
        c  = smoothed(part+0x488 (+ part+0x48 for filter 1), coefficient this+0x10e8) + kt + art[4+f]·2·range
        e_amt = part+0x464·art[31+f]; env = the articulator's envelope value
        non-adaptive: c += env·e_amt·2·range
        adaptive:     e = 4 env e_amt;  e <= 0: c += e·(hi - (lo + kt));  else c += e·(hi - c)
        then the articulator smoothing; W+0x9c+4f = 2^c
curve:  sc = 2^(1 - width·art[6+f])·1024
        oct scale: pos = (log2 Hz - c)·sc + centre;   Hz scale: pos = 0.9·sc·(Hz/2^c - 1) + centre
        C = table(pos), linear (nearest when S+0x10 < 1), clamped at both ends
        L *= C, or with mask M (shapes 18/19): L *= 1 + M(C - 1)
resonance (r = REC[57]/1000·art[8+f] > 0 and the resonance table is not identity):
        sw = part+0x49c (/ f0 when adaptive)·2^(1 - res width·art[10+f])
        part+0x49c: 1024 (oct), 1024·92.4986 (oct adaptive), 1024·0.000375 (Hz), 1024·0.000375·184.997 (Hz adaptive)
        oct: pos = (log2 Hz - c - part+0x490 - 8 art[12+f])·sw + centre;   Hz: pos = (Hz - 2^c·part+0x48c·2^(8 art[12+f]))·sw + centre
        L *= 2^(r·C·M)                                                        (masks 20/21)
noise (resonance type 109): p = res width·art[10+f]; on change voice+0x298 = 9999/((801^(p/4) - 1)·1.2475057 + 1.9954649),
        voice+0x290 = T·0.10001·voice+0x298/SR
        f = r^6·2·voice+0x298 (doubled again on Hz scale); hi = f + 1; U = U[0, hi) from the held seed voice+0x27c
        oct scale: U += Hz·(-1/17739.69)
        normal: G = U < f ? 0 : sqrt(hi);  free noise (REC[61]): G = (max(U - f, 0)/(1 - Hz/17739.69))^2·2·sqrt(hi)
        L *= G; voice+0x288 += voice+0x290, at >= 1 re-hash the seed
mix (first filter A = part+0xb64, second B):
        m < 0: (1 - |m|)·A(x) + |m|·B(x);   0 <= m < 1: A(x) + m·(B(A(x)) - A(x));   m = 1: B(A(x))
```

Units placed between the two filter slots act on the intermediate signal.

Shape tables (`FUN_00874ed0`, by ID; `E(u) = 10^(-3.6u)`, `Wd(u) = 1 -
u^16`, `t = 1 - i/(N - 1)`, `rc(z) = (cos z + 1)/2`, (a, b, c) = centre
a(N - 1), left reach b, right reach c; default (0, 1, 0)):

| ID | filter | N | table | (a, b, c) |
|---|---|---|---|---|
| 0 | classic LP | 2048 | E(x)Wd(x) | default |
| 1 | classic BP | 4096 | E(y)Wd(y), y = circular distance | (0.5, 1, 1) |
| 2 | classic BS | 4096 | 1 - E(y)Wd(y) | (0.5, 1, 1) |
| 3 | classic HP | 2048 | E(t)Wd(t) | (1, 0, 1) |
| 4 | crude LP | 1024 | (1 - rc(πt))^2 | default |
| 5 | crude BP | 2048 | (1 - rc(2πt))^2 | (0.5, 1, 1) |
| 6 | crude BS | 2048 | rc(2πt)^2 | (0.5, 1, 1) |
| 7 | crude HP | 1024 | rc(πt)^2 | (1, 0, 1) |
| 8 | hill | 682 | -cos(π(1 - x/2)) | default |
| 9 | leg | 1024 | 0.00075(1001^t - 1) + 0.25 t | default |
| 10 | hybrid | 1365 | E(v)Wd(v), v = 1 + cos(π(1 - x/2)) | default |
| 11 | snake | 1024 | (1 - rc(3πt))(1 - rc(πt)) | default |
| 12 | fin | 4096 | two halves (Harmless type 12) | (0.5, 1, 0) |
| 13 | leaky | 1024 | 10^((18v - 18)/20) | default |
| 14 | phaser | 4096 | rc(8πt)^2 | (0.5, 1, 1) |
| 15 | band phaser | 4096 | rc(12πt)^2 q^2 + q/8 | (0.5, 1, 1) |
| 16 | flat band pass | 2048 | 1 with cosine ramps on the outer thirds | (0.5, 1, 1) |
| 17, 18 | custom 1, 2 | 1024 | shapes 10 / 11 squared, edge reach trimmed from leading and trailing 1.0 | (0.5, 1, 1) |

Resonance tables (all (0.5, 1, 0); values are octaves of gain, used as
`2^(r·C)`):

| ID | type | N | table |
|---|---|---|---|
| 100 | classic | 1024 | 7(1 - rc(2πt)) - 1 |
| 101 | cuberdon | 1536 | 6.25·cos((i - 768)·3π/(N - 1))·(1 - rc(2πt)) |
| 102 | pedestal | 512 | 5.8 with cosine edges |
| 103 | sedge hat | 512 | 6.9·|1 - 2m/512| |
| 104 | wide bump | 1024 | 5.4 with v^2 edges |
| 105 | double cone | 1536 | 6.2(1 - rc(4πt))^2 |
| 106 | well | 682 | -8(1 - rc(2πt)) |
| 107 | wormhole | 1024 | -10·sinc·v^0.00125 |
| 108 | lowpass | 1024 | 6.8(1 - rc) - 0.8, minus 3(c + 1) above the centre |
| 109 | noise | | no table (noise mode) |
| 110, 111 | custom 1, 2 | 1024 | shapes 12 / 13 times 24 |

Phaser tables (RMS-normalised to 1 by `FUN_00874d10`, no edge trim): 200
classic 1024 `sin + 1`; 201 triangle `(1 - 2m/N)^2`; 202 eggs `sin(iπ/N)`;
203 deep `((sin + 1)/2)^6`; 204 deeper `E(y)Wd(y)`; 205 condom `E(1 -
d^4)Wd(..)`; 206 twins 2048 (N2 = N/2); 207 cascade 2048 (N2 = N/4); 208 box
2048; 209 freq and 210 custom from shape 15 (x2 for freq). Masks (IDs 400..800)
are shapes 18..22 at N = 2160, 240 points per octave, `pos = 240 log2 Hz -
1206.5` (32.6 Hz to 16.7 kHz), squared. Common tail: points below 2^-24
become 0 (`FUN_00629eb0`), equal leading and trailing points trimmed (not
phasers), `table[N] = table[0]`.

#### 3.6.6 Phaser (REC[76..81], art[16..18], shape 15, mask 22; `FUN_00887f10`)

```
m = min(REC[76]/1000·art[16], 1); N1 = S+0x20, N2 = S+0x24 (part+0x588 / +0x58c); kt = log2 f0·REC[81]/1000
off = (-REC[79]/1000 - art[18])·N1 - motion·N2       // motion voice+0x2a4 from part+0x5a0, += part+0x5a8 per sample
wd = part+0x5c0
scale 0 (oct): x = log2 Hz, a = 2^((wd - 1)·8 + 2 art[17])·12·N2, off -= kt·a
scale 1 (Hz):  x = Hz,      a = 2^((wd - 1 + art[17])·8)·0.005·N2, off -= kt·N1
scale 2 (Hz):  x = Hz,      a = (2 - wd - art[17])·N2/f0,           off -= kt·N1
C = table[(a x + off) & (N - 1)]   (periodic, FUN_00874b50)
normal types: L *= m C + 1 - m, or with mask L *= 1 + m M(C - 1)
freq type (209): F *= 2^(m^3·C·M)
```

#### 3.6.7 Pluck (REC[75], art[15], shape 14; `FUN_00888500`, `FUN_00888da0`, `FUN_008892e0`)

`x = REC[75]/1000 + art[15]`, active when x < 1. On change: `T_p =
20000·2^(17(x - 1))`, `k = ln 0.01·T/SR/(T_p + 0.25)`, rates `E_n = exp(k·P_n)`
with `P_n = 2^(16(1 - s14_n))` (pluck blend mode: `E_n = exp(k/P'_n)`, `P' =
T2 + (2^(16(s - 1)) - T2)(1 - mix)`). Each tick `L *= D`, then `D *= E`
(`FUN_00882aa0`); new slots join at D = 1 on the first tick or `E^ticks`
later.

#### 3.6.8 Blur (REC[20..25], art[22], mask 24; `FUN_00886610`)

Time blur: `S = L + (min - L)·att + (S - min)·dec`, `min = min(L, S)`
(`FUN_00882d10`), coefficients from REC[21], REC[22]; with pluck-to-blur the
decay coefficient is the pluck rate `E_n` (`FUN_00882da0`). Harmonic blur:
width `w = (17^(REC[23]·art[22]/10000) - 1)·15.9375 + 1`, `half =
(ceil(w)|1)/2 <= 128`; kernel ramp `d = 1/(w/2 + 1)` from `1 - d·half` to 1,
each side bent by `k = (A^r - 1)/(A - 1)` (A from REC[24] bottom, REC[25]
top), normalised by `1/Σk`, convolved over slots 4.. with edge padding
(`ippsConv`). Output `L += (blur - L)·mix` (`mix = (REC[20]/1000)^2`), with
the mask `L += (blur - L)·M·mix`; the mask index is not shifted past the
four header slots (`code`).

#### 3.6.9 Protection (shape 5, REC[8]; `FUN_00886110`, `FUN_00882750`)

`L_n += (Tmix_n - L_n)·s5_n^2·g`, `Tmix = voice+0x1a0` (the timbre mix),
`g = level11(REC[8]/1024)` (1 at 1024), over slots 0 to the last non-zero
shape value.

#### 3.6.10 Clipping (shape 6, REC[9], art[21]; `FUN_00886160`, `FUN_00889750`)

`θ = (REC[9]/1024·art[21])^2`, used as θ^2. Table `c = (2s)^2·K[mode]`, `K =
{4, 128, 32, 2, 2}` (0x9286c8); modes 1, 2 store `1/max(c, 5.96e-8)`; a flat
shape uses the scalar at part+0x10c.

| mode | rule |
|---|---|
| 0 high threshold | when θ < 1: `L = min(L, c θ^2)` |
| 1 soft low | `y = min(L·c/max(θ^2, 5.96e-8), 1)`; `L·(1 - (1 - y)^4)` |
| 2 sharp low | same y; `L·(1 - (1 - y)^2)` |
| 3 hard low | `L > c θ^2 ? L : 0` |
| 4 subtraction | `max(L - c θ^2, 0)` |

#### 3.6.11 Volume articulator

`W+0xb4 = art[1]^2·part+0x18`, applied by unit 11 or, when unit 11 is last,
by the tail.

### 3.7 Level tail (`FUN_0088ab10` lines 1209..1351, code)

1. Below sub mode: `L[0..3] *= min(Hz/114.46, 1)^4` (24 dB/oct under 7·C0).
2. Slot 0: `L[0] = part+0x68[0]·W+0xac·D[1]·W+0xb8` (filter 1 resonance
   amount, pluck decay of slot 1), frequency as in 3.4.
3. Global EQ (shape 17, ID 300): 2420 points at 240 per octave, `pos = 240
   log2 Hz - 966.63` (index 0 = 16.31 Hz); points 240..2400 are `v = g·s +
   0.25(1 - g)` with `g = part+0x2c8` (`GlobalEQMixWheel`), raised-cosine
   fades `v240·rc(π(1 - i/240))` below and `v2400·rc(π j/19)` above; `G =
   (4v)^2`, `L *= G`. `part+0x1a8 = 2^(4.0313597 + i_last/240)`, the last
   point with G > 0, sets the partial ceiling (3.4).
4. Auto-gain (REC[13] != 0): `E = Σ L^2` (`FUN_00882f00`), `g = E <= 0 ?
   4096 : clamp(sqrt(ref/E), 1/4096, 4096)`, `W+0xb4 *= 1 - c + c·g`, `c =
   part+0x234`, ref = 1.6428422 (absolute) or the energy measured after image
   × timbre (relative; that measurement only runs on the image branch, so
   without an image relative acts as absolute).
5. `W+0xb4 != 1`: `L *= W+0xb4`. These levels are the bank targets.

### 3.8 Unison (code)

Parameters: `n = part+0x1bc` (REC[34]); `part+0x1cc = 1/n`, `+0x1d0 = 1/(n -
1)`, `+0x1d4 = 0.5/(n - 0.5)`; level `part+0x1c8 = n < 2 ? 1 : 1 - (3^(1 -
1/n) - 1)/2`. Offsets per voice i (`FUN_00873e40`, part+0x1e0):

| type | offset t_i |
|---|---|
| 0 classic | `d + K0[i]/(n - 1)`, mean removed |
| 1 blurred | `d·K1[i]`, mean removed |
| 2 uniform | `d` |
| 3 random | `|2(i + 0.25)/(n - 0.5) - 1|` with sign, + `U(±part+0x1d4)` per note |
| 4 Hz | `|2(i + 0.5)/n - 1|` with sign; per partial `U(±1/n)` at voice+0x2c8, offset 1 |

`d = |2i/(n - 1) - 1|`, negated when `(i odd) XOR (n odd and 2i > n)`. K0 =
0.30693, -0.28972, 0.39241, -0.22302, 0.50820, -0.23664, 0.43662, -0.34831,
0.27434; K1 = 0.89047, 1.07643, 0.91158, 1.10254, 1.03413, 1.16686, 0.85397,
1.20627, 1.0 (0x928290).

Per unison voice: `voice+0x2b8 = t_i·part+0x1c0` (detune cents), `voice+0x2c4
= 2i/(n - 1) - 1`, pan `voice+0x2b0 = clamp(that·part+0x1d8, ±1)`, level
`voice+0x2b4 = part+0x1c8`, negated when `FatInvBox` and (i + part index) is
odd, map input `i/(n - 1)` (0.5 when n = 1). Each unison voice is a full voice
with its own bank; they all add into the buses. Corpus: unison in 77 of 128
parts; types 1 x57, 0 x8, 4 x5, 2 x4, 3 x3.

### 3.9 Oscillator bank (`TFastToneBank`, code)

Constructor `FUN_007832b0`, buffers `FUN_00783450`, render `FUN_00784840`,
dispatch `FUN_00785140`. Fields: +0x08 slots (N rounded to 8), +0x10 active
oscillators, +0x20 65 groups of 0x100 bytes (8 oscillators: amplitude a, y1,
y2, c = 2 cos 2πf, target, coefficient k, active mask, partial index, each
f32[8]), +0x30 resync countdown i32[520], +0x38 dirty flags, +0x40 partial to
slot map, +0x48 precision (0 average, 2 high, 3 perfect), +0x4d no snap on
the first block, +0x70 initial phase of slot N-1, +0xa0 / +0xa8 perfect-mode
amplitude / coefficient, +0xb0 phase f64[520] (cycles), +0xb8 frequency f64,
+0xc0 target level f32 (the pipeline writes here).

```
per sample, per oscillator:  y = c·y1 - y2; y2 = y1; y1 = y; out4[lane & 3] += a·y; a = target + (a - target)·k
out[s] = Σ of the four lanes (mono, FUN_00784790)
end of block: phase += freq·m (FUN_00784680)
resync before each block (FUN_00783d70): cnt -= m; when it runs out: y1 = sin 2π(φ - f), y2 = sin 2π(φ - 2f),
   cnt = round(8192 f); dirty: c = 2 sin(2π(f + 0.25)); sines by ippsSin_32f_A21 (average) or A24 (high)
high: every counter zeroed each block (reseed every block); perfect: no recurrence, out = Σ a_i sin(2π wrap(φ_i)) in f64
activation: |target| >= θ (θ = 10^(-96/20) average, 2^-24 high and perfect); every 512 samples lanes with |a| and
   |target| < θ are dropped and the last oscillator moves into the hole
reset at trigger (FUN_00783740): all zero, k = part+0x228, N = 8, freq[0] = 1
```

Ramping (`FUN_00878030`, `RampWheel` v): `f = |v| == 1000 ? 1 : (7^(|v|/1000)
- 1)/6`, `N_r = max(1, f·25·0.001·SR)` samples; `part+0x228 = exp(ln
0.01/N_r)` for every partial; v > 0 makes it per partial, `k_i = exp(Hz_i·ln
0.01/(N_r·2^(3/12)·308))` (1 % after `N_r·366.3/Hz` samples), reloaded per
tick (`FUN_00783a20`).

Start phases (first tick):

```
d = part+0x124 (k0); r = part+0x120 (k1)
unison n > 1: u = part+0x1c4·art[41] (-|u| for type 4)
   u <= 0: d += (i/n)·u;  u == 1: random partial phases (r = -1, seed reset);  else x = (2·2^(16u) - 2)·2^(voice+0x2b8/1200), d += frac(x)
r < 0:  φ_i = U[0, |r|) + ph_i                ph = harmonic phase table (shape 1: s' + 1, linear sampler, slot 0 = 0, slot 1 = slot 4)
r >= 0: φ = ph;  d += r·frac(F[4]·277·part+0x128)        (part+0x128 = the part's sample counter)
d != 0: φ_i += F_i·d/F[4]                     (time shift by d fundamental periods)
φ[0] = φ[1] = 0; below sub mode also φ[2] = φ[3] = 0
```

Declicker (`TDeclicker`, `FUN_008781f0`, `FUN_008635c0`): attack fade-in of
`(DeclickWheel/1000)^2·25` ms when non-zero, release fade-out 10 ms, both
`(1 + cos(πk/(L + 1)))/2`; applied to the mono bank output. Declick off sets
bank+0x4d, so amplitudes rise from 0 through the ramp instead of snapping.
Corpus: declick 0 everywhere.

### 3.10 Images (code unless marked)

Type-0 images (drawn or embedded) play from the stored planes; type-1 images
are rebuilt from the sample on load. `part+0x5c` = image kind + 1.

Time position per tick: smoothed offset `o` toward part+0x148 (jump when
beyond the threshold); `t = (o + 2 art[27])·cols + pos`; one-shot images end
the voice past the end when art[27]'s envelope is off; loop modes wrap
(1) or fold (2); `pos += part+0x140·art[28]`; a transient residue
voice+0x170 decays by `clamp(|Δcol|, 1e-4, 4)·T·0.0005` per tick.

Column interpolation (`FUN_00797bd0`; `BMPSmoothGainSelect` / `BMPSmoothSubFreqSelect`):
0 block, 1 cap (`0.5(2x)^9` / mirror), 2 bell (exponent 3), 3 linear, 4
Catmull-Rom (`FUN_007979e0`), 5 rise `1 - (1 - x)^6`, 6 fall `x^6`; loop
crossfade blends with the column one loop earlier.

Gain plane (row r = harmonic r + 1): pixel p to amplitude (`FUN_00798130`,
`A = 0.19211·bmp+0x38`): S > 0: `(e^(p ln(S + 1)/255) - 1)·A/((S + 1)^(2/3) -
1)·colgain`; S = 0: `p·A/170·colgain`. `level[4 + r] = timbre[4 + r]·amp_r`,
slots 1..3 from rows 0..2, then `level += (timbre - level)(1 - gain mix)`.
Formant shift (`FUN_00888720`): envelope = forward-backward one-pole over
rows (coefficient `1 - exp(ln 0.01/(3w))`), excitation = amp/envelope, read
the envelope at `(r + 1)·2^(-shift/1200) - 1` (or the mutation shape 9's
`(r + 1)·2^(-2s')`), mix by formant mix.

Frequency plane, every image refresh: `a0 = part+0x174·art[30]` (mask shape 25
on the rows inside its range); oct `F_i *= e^(p_i a0)`; Hz `F_i += p_i a0 f0c
D`; wide Hz `F_i += (p_i + 1)|a0| f0c D`; unison mode scales the per-partial
unison detune by `(p_i + 1)^2`.

Phase plane (resynthesised images): value = link code + phase/2; code 524 no
partial, 522/523 a peak, smaller codes a row it locks to. First tick `φ_i +=
φ_img - F_i·part+0x168`; later refreshes spread the phase error over the
next period as a frequency correction (phase-vocoder locking,
`FUN_0088aa80` / `FUN_0088ab00`), rate `part+0x190·min(1, 1.5|Δcol|)·sharpening`.

Resynthesis on load (`FUN_00896490` -> `FUN_008960f0` -> `FUN_00895ea0` ->
`FUN_00794510`, outline): side signal L - R with mid/side, mono, resample to
33488 Hz (440·2^(75/12), so a 2048-point FFT has 16.35 Hz bins), hop 194
samples, 1023 rows; two windowed FFT passes (`FUN_0062cb40` windows: generic
6/6 with 36/21.6, high precision 6/3 with 21.6/0); plane 0 = |X|·gain,
plane 2 = 0.5·frac(arg X/2π + 0.25); row reduction (`FUN_00794370`),
denoise `(BMPDenoise/250)^2` (`FUN_00794490`), scale to 255 pixels, then
`FUN_00795720` (4.9 KB) builds the link codes, the per-row phase slopes,
column gains and the base pitch bmp+0x148. Short or quiet samples (length <=
1024) import as a single-cycle waveform (`FUN_008964f0`).

### 3.11 Output, part mix, tremolo (code)

```
m   = REC[10]/1000·art[2]
pan = clamp(Final.Pan + glide pan + voice+0x2b0 + 4·art[0], -1, 1)
g   = part+0x14·(Final.Vol + glide vol)·voice+0x2b4
FX bus  (buf+0x60) += s·G(pan, g(1 - |m|));   dry bus (buf+0x68) += s·G(pan, g·m)      (m < 0: inverted dry)
G(p, g): p == 0 -> (g, g); else L = √2 g cos((1 + p)π/4), R = √2 g sin((1 + p)π/4)     (FUN_007a4d50 law 0)
gains ramp per sample: cur = tgt + (cur - tgt)·host+0x40; first tick snaps; MidSide negates R
part+0x14 = part+0x1c·level3(MainVol/1000)·sign(REC[11])·level3(|REC[11]|/1000)
part+0x1c: both parts on: A = 0.5625·clamp(cos(π/2·mix), 0, 1), B = 0.5625·clamp(cos(π/2·(1 - mix))), mix = PartMix/1000; one part: 0.5
```

The first voice to write a bus in a block zeroes it (flags +0x70 / +0x74).

Tremolo (per part, Gen_Render): `g = 1 + depth·(0.21 - (0.55(1 + sin))^2)`
(0..1.21; `ippsTone` magnitude 0.55), R's phase advanced by twice the gap;
a voice turns mono into stereo and fades the tremolo in over 0.1 s.

Threading: with `QThreadedBox` and two or more voices, workers take voices
from an atomic counter (`FUN_00891210`), each with its own buffer set; sets
1.. are added into set 0. The arithmetic is the same single-threaded.

## 4. Effects (Gen_Render tail, `FUN_00891320`)

### 4.1 Chain

Buffers: `**(this+0xe58)` has eleven interleaved-stereo buffers
(+0x18..+0x68) of `2·max(N·os, 520)` floats; +0x60 FX bus, +0x68 dry bus,
+0x20..+0x40 scratch, +0x48 / +0x50 tremolo of parts A / B.

Per block: voices; `FUN_00891280` (deferred chorus, delay and reverb setters;
tail counter `this+0x2944` reset to `0x2948 = round(0.001·SR·(distortion ? 530
: 30))` plus the chorus length while voices exist, else decremented); zero
the FX bus if no voice wrote it; `0x294c = 0x2944`; the unit list; post
volume `this+0x109c` on the FX bus; `out += FX bus`.

| order value | unit | in the list when |
|---|---|---|
| 0 | distortion `FUN_008944a0` | `DistModeSelect` > 0 |
| 1 | chorus `FUN_00893ec0` | chorus order > 0 |
| 2 | delay `FUN_00893fc0` | delay enabled |
| 3 | reverb `FUN_008942b0` | reverb enabled |
| 4 | dry return `FUN_00894330`: FX += dry | always |
| other | compressor `FUN_008943f0` | `CompTypeSelect` > 0 |

The dry bus reaches the output only through unit 4, so units after it process
the dry signal too. With `0x294c < 1` (no voices, tail expired) distortion
skips, chorus only advances its LFO, delay stops feeding input but keeps its
feedback; delay, reverb and compressor set `0x294c = 0x7fffffff` so later
units run.

### 4.2 Distortion (`FUN_008944a0`, setter ProcessParam 0x315..0x319, code)

`v = DistIn`, `b = DistAsym/500`, `w = level3(DistOut/1000)`, `m =
DistMix/1000`, `s = w(1 - |m|)`.

| mode | name | drive D | wet gain | shaper y(x), x = D·(in + bias) | bias |
|---|---|---|---|---|---|
| 1 | classic | π k, k = 2^(v/100 - 4) | s/(π tanh k) | tanh x | 0.75 b^2 |
| 2 | hill | same | same | e^(tanh x) - 1 | 0.75 b^2 |
| 3 | flat hat | same | same | tanh(e^x - 1) | 0.75 b^2 |
| 4 | cube | π k, k = 2^(v/100 - 2) | s/(π tanh^3(4k)) | tanh^3 x | 0.75 b^2 |
| 5 | log | k = 2^(v/40 - 5) | s/(ln(k + 1)(v/1000 + 1)) | sign(x) ln(1 + |x|) | 0.75 b^2 |
| 6 | softsat | π k, k = 2^(v/200 - 3) | s/(π tanh k) | cbrt(tanh(x^3)) | 0.75 b^2 |
| 7 | rubber | k^2, k = π 2^(v/500 - 1) | s/√k | sign(x) sin(√|x|) | 0.75 b^2 |
| 8 | ribbon | π k, k = 2^(v/100 - 4) | s/(π tanh k) | tanh(x) tanh(x + B) tanh(x + 2B), B = b^2 D | |
| 9 | sincrush | π 2^(7 - 0.007 v) | s/(D g) | sin x + g x, g = 2^(DistAsym/250) | |
| 10 | bitcrush | 2^(9 - 0.007 v) | s/D | round(x) | b/D |
| 11 | bitjam | Q = 1024·2^(DistAsym/50) | s/Q | sign(x)·expm1(round(ln(1 + |x|)c)/c), c = 2^(10 - 0.007 v)/ln(Q + 1) | |

Bias: b > 0 gives the same bias on both channels, b <= 0 gives +bias on L and
-bias on R. Render at `os = 2^clamp(precision + 1, 1, 3)` (x2 average, x4
high, x8 perfect):

```
up  = upsample(FX)                                 // polyphase IIR halfband per stage (FUN_00863aa0 / FUN_00863c70)
dry = up·smooth(m)                                 // signed mix
up  = (up + bias)·smooth(D);  dry += shaper(up)·smooth(wet)
DistFCut != 1100: two RBJ low-passes, f = 16.3516·2^(FCut/100) clamped to 0.4999·SR·os, Q = 1/√2
FX  = DCblock(downsample(dry))                     // FUN_0062a3c0: v += (x - lp)a; lp += v; v *= 0.96; y = x - lp; a = 2.2835e-5·44100/SR
```

Smoothers: 15 ms per oversampled sample. Halfband: two chains of three
first-order allpasses `y = c(x - y1) + x1` in f64, chain A 0.1473771136010466,
0.48246854276970014, 0.8830050257693731, chain B 0.039151597734460045,
0.3026468483284934, 0.6746159185469639; up: A gives the odd output sample, B
the even; down: `y = 0.5(A(even) + B(odd))`. The vector exp, ln, tanh,
sqrt, cbrt and div come from the precision-chosen IPP slots at this+0x1150.

### 4.3 Chorus (`FUN_00893ec0`, setter `FUN_008937f0`, `TStereoFlanger`)

| REC | control | conversion |
|---|---|---|
| 804 | `ChOrderSelect` 0..9 | flanger +0x38 = v; 0 frees it |
| 805, 806 | depth, speed | +0x3c, +0x40 = v/1024 |
| 807..809 | delay, spread, cross | +0x44..+0x4c = v/128 |
| 810 | `ChWetWheel` | w = |v|/1000; dry this+0x2960 = 1 - (5^w - 1)/4; wet +0x50 = sign(v)(1 - (5^(1 - w) - 1)/4) |

384-frame chunks: write the bus into the ring (`FUN_007a91b0`), the kernel
(flanger+0x60) writes the wet, `bus *= dry`, `bus += wet`. The class is Fruity
Flangus's (`crates/plugins/src/flangus.rs` `build()` / `Voice::delay`): 2·order
voices, depth `(11^depth - 1)·109.25` samples at 44.1 kHz, centre delay
`(11^delay - 1)·441`, parabolic triangle LFO, spread `(1001^spread -
1)/1000`, cross-feed, normalisation, linear interpolation, no feedback.
Harmor's front end differs: signed wet folded into the normalisation, 5^w
dry/wet curves without a ramp, order 0 = off, fixed 0x180 block, the tail
skip.

### 4.4 Delay (`FUN_00893fc0`, setter `FUN_00892f90`, `TDelay_32FS`)

| REC | control | conversion |
|---|---|---|
| 811 | `DelEnableBtn` | < 1 frees the object |
| 812 | `DelModeSelect` 0 normal, 1 inverted, 2 ping-pong | this+0x2a60; > 0 negates the pan |
| 813 | feedback 0..1000 | target v/1000, ramped at most host+0x34 per sample |
| 814 | time -384..384 | v < 0: (2^(-v/96) - 1)/15 s; v >= 0: v·0.3125/BPM s; T_d = round(t·SR) >= 1 |
| 815 | stereo offset -512..512 | (2^(|v|/64) - 1)·1.96078 ms on L (v <= 0) or R (v > 0) |
| 816 | input volume | (v/1000)^3, signed |
| 817 | input pan | p = v/128, input smoother `G(p, vol)` |
| 818 | low cut 0..1000 | 0 off; RBJ HP, f = 16.3516·2^(v/100), alpha = sin w (Q 0.5) |
| 819 | high cut 400..1050 | 1050 off; RBJ LP, same map |
| 820 | feedback damping | filters inside the loop instead of on the input |

Ring of `offset + 384 + T_d` frames; per chunk `x = FX·smooth(in)` (filtered
when not in the loop), added at the write pointer plus the per-channel offset
(`FUN_007a55d0`); per span (`FUN_007a5260`) the mode kernel: 0
`FUN_007a5660` out = (L, R) straight feedback, 1 `FUN_007a5790` out = (R, L),
2 `FUN_007a58b0` out = (R, L) with crossed feedback; each reads the ring,
clears it and writes `fb·out` T_d frames later (flush below 2^-24); `FX +=
out`. The repo has no Rust for this class; Harmless and Sytrus use the same
one.

### 4.5 Reverb (`FUN_008942b0`, setter `FUN_00893470`, `TFruityReverb`)

| REC | control | conversion |
|---|---|---|
| 794 | `RevEnableBtn` | < 1 frees it |
| 795 | low cut | v > 0: 16.3516·2^(v/100) Hz, else off |
| 796, 801 | high cut, high damping 400..1050 | v < 1050: 16.3516·2^(v/100)·0.01 (100 Hz units), else off |
| 797 | predelay | v < 0: (2^(-v/64) - 1)/63 s; else v·0.3125/BPM s |
| 798, 799, 802 | room size, diffusion, colour | ints |
| 800 | decay 0..900 | 2^(v/100)·0.1·SR samples |
| 803 | wet -1000..1000 | (v/1000)^3·0.5, smoothed |

Mono sum L + R, the reverb, `FX += wet·smooth`. The class update
(`FUN_007a7c20`) is Reeverb 2's (`FUN_006d1f30`) with no early reflections, a
fixed 500 Hz crossover and a colour table {0.2, 0.6, 1.0, 1.5, 3.0} for the
bass multiplier. `crates/plugins/src/reeverb_2.rs` `Reeverb2::frame(input,
false)` with modulation depth 0 is the network; Harmor needs its own front end
(float Hz with 0 = off, the decay map, colour table, predelay map).

### 4.6 Compressor (`FUN_008943f0`, loader `FUN_00893a80`)

`CompTypeSelect` n > 0 loads `SG.dll` (Soundgoodizer, which names itself
Maximus) into the embedded host and then the preset
`<Harmor dir>/System/Compression/<n>.fst`; n = 0 unloads it. Render: the
plugin's Eff_Render on the FX bus in place. Afterwards REC 821..824 are
re-sent to it, so the preset's own values for these never apply:

| REC | control | Maximus REC | meaning (`maximus.md` section 2) |
|---|---|---|---|
| 821 | `CompVolWheel` 0..1000 (500) | 5 | LMH mix = v/1000 |
| 822 | `CompLWheel` 0..2000 | 8 | low band pre gain `(7^(v/1000) - 1)/6` |
| 823 | `CompMWheel` | 21 | mid band pre gain |
| 824 | `CompHWheel` | 34 | high band pre gain |

Presets (Maximus v7): 1 limiting (single-band limiter; byte-identical to
Harmless `Compression/1.fst`), 2 warming (all RMS; new), 3 heating (low,
mid saturation; Harmless 2 with other crossover, slopes and mix), 4 burning
(Harmless 3 without the low cut). Implementation: host `maximus.rs` with the
preset state as `soundgoodizer.rs` does, then set params 5, 8, 21, 34.

## 5. What Harmor shares with Harmless

Block by block, against `docs/plugins/harmless.md` and the Harmless
decompiles:

| Harmor block | Harmless | sharing |
|---|---|---|
| `TFastToneBank` (`FUN_00784840` and kernels) | `FUN_00746320` (bank at voice[0x19]) | identical code once addresses are normalised; Harmor adds per-partial ramp coefficients (`FUN_00783a20`) and resync-all, and turns the bank's frequency fade window off. One Rust bank serves both, and closes Harmless open item 2 |
| ratio table, Hz spacing, around sub mode (`FUN_0088fcf0`) | `FUN_00787ce0` | same; Harmor adds oct spacing and the below sub mode |
| filter shape tables 0..16 and lookup (`FUN_00874ed0`, `FUN_00874930`) | `FUN_0077ca30`, `FUN_0077c540` | same formulas, N and reach |
| filter range (`FUN_00874240`) | `FUN_0077c170` | same rule; top log2 17739.69 (Harmless 17000) |
| filter cutoff, envelope, Hz path | Harmless filter path | same |
| resonance | Harmless resonance types | differs: Harmor's additive log2-gain law and its own tables |
| phaser tables 200..208 and maths | phaser types 0..8 | same; the freq type differs (`2^(m^3 C)` with the custom x2, Harmless `exp(c m^3 ln2/4)`) |
| harmonizer (`FUN_00885cb0`) | `FUN_00784220` | same skeleton; Harmor adds the distribution shape, the 1/(8w + 1) step, the t^2/sqrt variant and the mask |
| pluck | `FUN_00784860` | same decay law; the rate comes from a shape |
| protection | `FUN_00784120` | same crossfade over a drawn shape and all slots |
| self-oscillation slot 0 | Harmless slot 0 | same |
| TriggerVoice skeleton, MaxPoly steal, colours 13..15, release `(1 - 0.85 rv)^2`, linear glide | `FUN_00782940`, `FUN_00783910`, `FUN_00784630` | same; Harmor's exponential glide has two poles and a limit, plus parts, mono modes, strum, held index, closest-pitch legato |
| unison distribution tables | Harmless types 1, 2 | Harmor types 2 and 1 respectively; Harmor type 0 and voices-per-copy differ |
| chorus (`TStereoFlanger`) | `FUN_0074a9b0` | identical class; also `flangus.rs` |
| delay (`TDelay_32FS`) | `FUN_00747510` | identical class; Harmless has a smaller front end |
| reverb (`TFruityReverb`) | `FUN_00749b60` | identical class; also `reeverb_2.rs` |
| compressor | embedded Maximus | same mechanism; preset 1 identical |
| articulators, envelopes, LFOs, maps | none (Harmless has its own ADSR) | build on `menvelope.rs` (Love Philter / Sytrus code) instead |
| prism, blur, clipping, local EQ, global EQ, masks, harmonic randomness, unit order, two parts, images, resynthesis, distortion, tremolo | none or different | Harmor only |

Three facts here contradict `harmless.md` and belong in it: Harmless `0x1bc`
is VelToVol (REC 37) and `0x1d8` (REC 44 "MonoBox") is the legato flag;
Harmless `0x7c0` is the chorus (`TStereoFlanger`), not a phaser, and `0x7c8`
its dry gain; Harmless forwards `CompVolWheel` to Maximus parameter 5, the LMH
mix, not an input gain.

## 6. Open items

- Resynthesis detail: `FUN_00795720` / `FUN_00795090` (link codes, per-row
  phase slopes, column gains, base pitch bmp+0x148), `FUN_00794370` row
  reduction, `FUN_00794490` denoise and the `FUN_0062cb40` window types are
  outlined only. Affects the 9 parts with file images (8 states).
- `FUN_00882f00` (auto-gain energy measure) not read; taken as Σ L^2.
- Unison type 4: the constant of the `ippsAddC_32f_I` on voice+0x2c8 at
  trigger was dropped by the decompiler (disassemble near the call in
  `FUN_00883cd0`).
- `SG_x64.dll` is assumed to run the Maximus algorithm; not diffed against
  `Maximus_x64.dll`.
- Small fields without a found writer: `this+0x10d8` is set in
  `FUN_00878690` but nothing clamps against it outside oct spacing;
  `part+0x5a0` / `+0x5b0` (phaser motion start and scale), `part+0x128`'s
  use in the phase seed, `this+0x113d` (arpeggiator re-sort gate), the ModX
  copy at `this+0x1114`, the shape interpolation flag S+0x10.
- Map centring: subtracting the centre in the non-flat sampler branch is
  `inferred`; the default shapes come from `System/*.fnv` files.
- `this+0x10c0` adds noise from editor mouse gestures (an easter egg); skip.
- Visual feedback (`FUN_00890310`) is UI only.

## 7. Implementation order

Counts from `harmor_state.py` over the 100 corpus states and their 128
enabled parts (A in 98 states, B in 30, both in 28). A block is counted when
the state turns it on (the condition in parentheses).

| step | block | parts (of 128) | states (of 100) | notes |
|---|---|---|---|---|
| 1 | state parser, both parts, all curves | 128 | 100 | `harmor_state.py` is the reference |
| 2 | tone bank, control tick, ramping, declicker | 128 | 100 | shared with Harmless |
| 3 | pitch, ratio table, partial count, global EQ ceiling, taper | 128 | 100 | |
| 4 | timbres, timbre mix, start phases, harmonic phase | 128 | 100 | timbre 2 mixed in 28; blend mode 0 in all |
| 5 | voice lifecycle, output pan / FX-dry split, part mix, pre and post volume | 128 | 100 | |
| 6 | articulators on `menvelope.rs` (envelope, LFO, maps, smoothing, global scales) | maps 70, other envelopes 63, volume envelope 44, LFOs 38, smoothing 13 | | fix the LFO centre first |
| 7 | filters 1 and 2 with mix | 128 | 100 | types (4, 18) in 67 parts, (0, 18) 17, (0, 0) 8, (0, 3) 8, (17, 18) 7 |
| 8 | unison (order > 1) | 77 | | |
| 9 | compressor (SG preset on the Maximus port) | | 58 | |
| 10 | resonance (REC 57 or 69 > 0) | 57 | | types (108, 111) in 90 of 200 parts; noise mode in 2 |
| 11 | vibrato (REC 45 > 0) | 52 | | |
| 12 | distortion (mode > 0) | | 41 | modes 5, 6, 1, 7 most used |
| 13 | reverb | | 39 | Reeverb 2 network |
| 14 | harmonizer (mix and width > 0) | 35 | | |
| 15 | pluck (REC 75 < 1000) | 28 | | |
| 16 | phaser (mix > 0) | 27 | | type 200 in 112 of 128 |
| 17 | phase randomness | 25 | | |
| 18 | prism | 23 | | |
| 19 | legato / portamento / mono | | 22 / 7 / 4 | |
| 20 | chorus, delay | | 19, 19 | Flangus class; new `TDelay_32FS` port |
| 21 | auto-gain | 14 | | |
| 22 | protection, clipping | 10, 10 | | |
| 23 | precision modes high / perfect | | 10 | |
| 24 | tremolo | 9 | | |
| 25 | images: embedded / resynthesised from files | 8 / 9 | | resynthesis needs section 6's open items |
| 26 | blur | 8 | | |
| 27 | sub-harmonics, unit order, local EQ shape, harmonic detuning, FX order, oct spacing | 6, 6, 4, 4, -, 2 | FX order 4 | |
| 28 | strum, arpeggiator rank, VelToPortaTime | 0 | 0 | |

Steps 1 to 8 make every corpus channel sound with its basic timbre, filter
and unison; with steps 9 to 13 the shared FX cover most states; steps 14 to
18 finish the common spectral units.

## 8. Implementation

`crates/plugins/src/harmor.rs` is the plugin, with three files beside it:
`harmor_state.rs` (section 1), `harmor_art.rs` (section 3.3) and
`harmor_fx.rs` (sections 4.2 to 4.5). Status by step of section 7:

| step | block | status |
|---|---|---|
| 1 | parser | built: all 100 corpus states and 328 presets parse to the last byte, 128 enabled parts |
| 2 | bank, tick, ramping, declicker | built. The bank keeps its recursion from block to block and resyncs on a new frequency, on waking, or every 8192 samples (FL: `round(8192 f)`); partials are summed eight at a time. Per-partial ramp coefficients, the -96 dB drop threshold at average precision, the 10 ms release fade. The attack declicker is not built (declick 0 in the whole corpus) |
| 3 | pitch, ratios, partial count, global EQ, taper | built. A note's pitch follows the host's (`Generator::set_note_pitch`, for slide notes and a clip on the channel's pitch) from its voices' next tick, a note held under a legato voice included |
| 4 | timbres, mix, start phases, harmonic phase | built, with the blend modes and phase randomness |
| 5 | voice lifecycle, pan, FX / dry, part mix, volumes | built. MaxPoly is not read; a full pool of 128 voices steals the oldest, released ones first |
| 6 | articulators | built on `menvelope.rs`'s `Lfo` (with its centre at 0.5) and a Harmor envelope runner in `harmor_art.rs`: per-voice scales, global scales, the attack jump, sustain offset and the release carried over from where the run is. Approximated: the per-point sustain weights (taken as 1 from the decay marker to the loop end), shared runs (flag 2) run per voice, arpeggiator points are not read, the LFO's speed articulator also speeds its level envelope, the global LFO phase (articulator 40) is not applied |
| 7 | filters 1 and 2, their mix | built, with masks. Types 0..16 are Harmless's tables (`harmless::shape`); the custom types 17 and 18 skip the edge trim, so their range takes a reach of 1 on both sides |
| 8 | unison | built; type 4 (Hz) detunes whole voices instead of each partial |
| 9 | compressor | built: `System/Compression/<n>.fst` on the Maximus port, REC 821..824 forwarded |
| 10 | resonance | built: tables 100..111 from section 3.6.5 and the noise type 109 (its random numbers are a hash, not `ippsRandUniform`) |
| 11 | vibrato | built |
| 12 | distortion | built from section 4.2, all eleven modes, the halfband chains and the DC block |
| 13 | reverb | built on `fl_fx::FruityReverb` |
| 14 | harmonizer | built |
| 15 | pluck | built; pluck-to-blur's decay is not |
| 16 | phaser | built; the motion rate takes Harmless's beat unit, since part+0x5b0 has no writer in the map |
| 17 | phase randomness | built |
| 18 | prism | built |
| 19 | legato, portamento, mono | built in outline: legato takes over the closest held note and goes back to it on release, portamento glides from the last note, the mono modes kill or release. The glide limit and per-colour records are not built |
| 20 | chorus, delay | built on `fl_fx::StereoFlanger` and `fl_fx::Delay32`; delay damping filters the input, not the loop |
| 21 | auto-gain | built, absolute mode (relative acts as absolute without an image) |
| 22 | protection, clipping | built |
| 23 | precision modes | only the drop threshold; the HQ render is not used |
| 24 | tremolo | built, its smoothing not |
| 25 | images | not built (8 embedded, 9 resynthesised parts) |
| 26 | blur | built: time blur and harmonic blur with mask |
| 27 | sub-harmonics, unit order, local EQ, detuning, FX order, oct spacing | built |
| 28 | strum, arpeggiator rank, VelToPortaTime | not built (none in the corpus) |
| - | harmonic randomness (unit 2) | not built: shape 4 sits flat at its centre in all 128 parts, which makes the unit do nothing |

Corpus (FL_RS_CORPUS render of every project, first 10 s): before Harmor,
104 projects had sound and 18 Harmor channels with notes were silent;
with it, 106 projects have sound and every Harmor channel is hosted. Two
JuiceBass states have filter 1's cutoff at 0 and only sound when their
project moves it, and one Square a Saw state opens its filter after a beat.

Corrections to the sections above, from the code: the global EQ mix in the
part struct is an f32; the distortion's five REC are drive, asymmetry, out,
mix and cutoff in that order; the harmonizer moves its pattern after each
copy.
