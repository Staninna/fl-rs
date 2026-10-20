# Fruity Convolver: state, IR preparation, partitioned convolution

Recovered from `Fruity Convolver_x64.dll` (FL Studio 2026, Delphi native
plugin, image base 0x400000; the convolution engine is compiled into the same
DLL as classes `TConvolver`, `TDirectFIR` and an FFT partition engine, no
engine DLL beside it). Ghidra project `ghidra/proj`; decompiles under
`ghidra/out/convolver/` (`vmt/` the SDK slots with callees, `eng/` the
process path and form handlers, `callers/` and `setter/` the IR rebuild and
the engine classes). Corpus: 21 uses in the 165 `.flp` (10 of them with the
impulse embedded), 264 shipped presets. Five Ghidra runs: import, slots,
engine, callers (a dead end: only the reset and process callers), and the
setter pass with two scripts in one run.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `inferred`.

## How the plugin is wired

`CreatePlugInstance` @ `0x00874d20` -> constructor `FUN_00877640`, class
`TTestPlug`, VMT `0x00874c00` (instance size 0x538).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x00874d70 |
| 2 | Idle_Public | 0x00783550 (empty) |
| 3 | SaveRestoreState | 0x00875550 |
| 4 | GetName | 0x008791b0 |
| 5 | ProcessEvent | 0x00878760 |
| 6 | ProcessParam | 0x00878c20 |
| 7 | Eff_Render | 0x0087a000 |

Object fields the audio path touches:

| offset | type | meaning |
|---|---|---|
| 0x158 | i32[4] | the four automatable parameters, saved |
| 0x168 | ptr | the impulse sample (editor's working copy; `+0xa0` its file name, `+0x88` its length) |
| 0x178 | ptr | stereo scratch buffer (`n` frames) |
| 0x180, 0x190, 0x198, 0x1a8.., 0x3b0.. | | the IR preview player (Play button), not the effect |
| 0x3d0 | ptr | recording buffer for "Record impulse response" (`0x400` = recording) |
| 0x404 | ptr | `TConvolver` (section 4) |
| 0x40c | i32 | predelay in samples |
| 0x410 | i32 | latency block absorbed by the engine (0 or a power of two >= 512) |
| 0x414 | f32 | self-convolution amount 0..1 |
| 0x418 | f32 | stretch ratio |
| 0x41c | f32 | EQ mix 0..1 |
| 0x420 / 0x440 | f32[8] | input-width and wet matrices (4 targets + 4 ramp states each) |
| 0x460 / 0x464 | f32 | dry gain, dry ramp state |
| 0x468 | f32 | wet gain |
| 0x46c | CS | critical section around the engine |
| 0x494 / 0x49c | ptr, i32 | the IR rebuild thread and its state |
| 0x4a0 | ptr | the EQ envelope (`TMEnvelope`, kind 3) |
| 0x504 / 0x508 | i32 | predelay + IR length; silence countdown |
| 0x50c..0x50f | u8[4] | flags, saved: ?, display flag, link-to-file, normalize |
| 0x514 | i32 | reported latency in samples, saved (v15) |

## 1. State layout (event 213: variable; 549..563 bytes for file-linked presets)

`SaveRestoreState` (`FUN_00875550`), restore:

```
Read(i32 v); if v >= 0x10000: v >>= 16; accept 10..15 only
Read(this+0x158, 0x10)                               // 4 params
Read(u8 byFile); ReadString(path)                    // FUN_00783800: u8 len, or 0xff + u32 len; 8-bit chars
if byFile == 0:  Read(i32 len); Read(len bytes) -> load as a WAV from memory (FUN_00619860); editor flag "embedded"
else:            expand %FLStudioData% etc. (FUN_0087cff0/FUN_0087d020), ask the host FHD_LocateDataFile (0xe), load the file (FUN_007a8ff0)
Read(36 bytes): f64 selStart, f64 selEnd, f64 viewStart, f64 viewLength, f32 zoom   // editor view; -1, -1 = no selection
for each of the editor's 4 envelope layers: TMEnvelope.Load                           // kind 6 blocks
TMEnvelope.Load(this+0x4a0)                                                           // the EQ curve, kind 3
Read(this+0x50c, 4)                                   // flag bytes
0x514 = 0; if v >= 15: Read(this+0x514, 4)            // latency
Read(5 ints) (4 and EQ mix = 1000 when v < 11): tab, predelay, self-conv, stretch, EQ mix  -> the five non-automatable wheels
push flags to NormBtn (0x50f) and the editor; FUN_00881170 (recompute 0x40c..0x41c, start the rebuild thread); FHD_SetLatency(0x514)
```

Save writes version `0xf0003` (15 in the high word, 3 in the low: the low
word is the stream codec id the loader strips; the Love Philter uses the same
scheme), the four params, `byFile = (this+0x50e != 0) && !modified`
(`FUN_0087ade0`), the path (the editor sample's file name, kept even when
embedding), the WAV when embedding (`FUN_006133a0` writes a RIFF/WAVE with a
`JUNK` pad, 16-bit stereo in the corpus), the 36 GUI bytes, the 4 + 1
envelopes, the 4 flag bytes, the latency, and the 5 ints.

Byte map of a file-linked state (offsets after the fixed part shift with the
path length `L` and the envelope point counts):

| byte | type | field | range / default | corpus (285 blocks) | confidence |
|---|---|---|---|---|---|
| 0..3 | i32 | version: `0xf0003` (15), `0xe0003` (14), 11 or 14 plain in older files | | 14 x261, 15 x21, 11 x3 | loader, corpus |
| 4..7 | i32 | idx 0 Dry level (`DryWheel`) | 0..1000, 1000 | 1000 x243, 0 x35, 404..987 | loader, dfm, corpus, preset |
| 8..11 | i32 | idx 1 Wet level (`WetWheel`) | 0..1000, 500 | 500 x240, 1000 x34, 343..935 | loader, dfm, corpus, preset |
| 12..15 | i32 | idx 2 Input stereo separation (`FWidthWheel`) | -1000..1000, 0 | 0 x284, -1000 | loader, dfm, corpus |
| 16..19 | i32 | idx 3 Wet stereo separation (`CircleWheel`) | -1000..1000, 0 | 0 x278, 92 x2, -384, 678, -1000 | loader, dfm, corpus |
| 20 | u8 | IR by file (1) or embedded (0) | | 1 x275, 0 x10 | loader, corpus |
| 21.. | str | path: u8 length (0xff + u32 for >= 255) + chars | `%FLStudioData%\Patches\Impulses\...\*.wv`, `%FLStudioFactoryData%\...` | 54..112 chars; one 497-char path | loader, corpus |
| | i32 + bytes | embedded WAV (only when byte 20 is 0) | | 10 blocks, 69 KB .. 743 KB | loader, corpus |
| +0 | f64 | selection start, -1 none | | -1 x270, 0.0 x15 | loader, corpus |
| +8 | f64 | selection end, -1 none | | -1 x270, 2.3.. | loader, corpus |
| +16 | f64 | view start (s) | | 0 | loader, corpus |
| +24 | f64 | view length (s): the IR length | | 0.12 .. 10.02 | loader, corpus |
| +32 | f32 | zoom | | 0.02 | loader, corpus |
| +36 | | envelope layer 1..4, kind 6 (section 2) | 2 points each by default | all 285 have 4 x kind 6 | loader, corpus |
| | | EQ envelope, kind 3 | 2 points (0, 0.5) (1, 0.5) by default | kind 3 in all 285 | loader, corpus |
| | u8 | `0x50c`: not read by the audio path (editor) | 0 | 0 x285 | loader, corpus |
| | u8 | `0x50d`: set by FPD message 0x24 (display) | 0 | 0 x284, 1 | loader, corpus |
| | u8 | `0x50e`: keep the IR as a file reference (parameter 1000, `inferred` from the save test) | 1 | 1 x285 | loader, corpus |
| | u8 | `0x50f`: Normalize impulse loudness (`NormBtn`) | 1 | 1 x253, 0 x32 | loader, corpus |
| | i32 | latency (`0x514`), v15 only; `FHD_SetLatency` | 0..5 s in samples, 0 | 0 x19, 512, 8192 | loader, corpus |
| | i32 | tab shown (0 IR editor, 1 EQ) | 0 | 0 x277, 1 x8 | loader, corpus |
| | i32 | Pre-delay (`DelWheel`, hint "Pre-delay time") | 0..1000, 0 | 0 x283, 45, 114 | loader, dfm, corpus |
| | i32 | Self-convolution amount (`SelfConvWheel`) | 0..1000, 0 | 0 x283, 148, 102 | loader, dfm, corpus |
| | i32 | Stretching ratio (`ResampleMulWheel`) | -1000..1000, 0 | 0 x277, -372 x4, 279, -476, -1000, 1000 | loader, dfm, corpus |
| | i32 | Equalizer mix (`EQMixWheel`) | 0..1000, 1000 | 1000 x284, 669 | loader, dfm, corpus |

Every one of the 285 blocks parses to its last byte with this layout
(`/tmp/p4` throwaway parser).

Envelope block (the `TMEnvelope` save used here, `FUN_00792620` over the base
`FUN_0078a660`; the same bytes automation-234 documents, without the 16-byte
`+0x98` trailer):

```
u32 kind (6 editor layer, 3 EQ); u32 version (3); u32 n
n points: f64 x, f64 y, f32 tension, u8 mode, u8, u8, i8 cache          // 24 bytes; 16 bytes with f32 x, y when version < 2
u32; i32 active (-1); i32 -1; i32 -1; i32 -1                              // the 20-byte obj+0x44 block
```

For the layers `x` is in seconds along the IR (the default pair sits at 0 and
the IR length, both at y 0.5 = neutral); for the EQ `x` is 0..1 across the
spectrum.

Preset check (all presets are version 14, file-linked, flat envelopes; the
levels through section 3):

| preset | path | Dry | Wet | Width | Circle | Norm | reading |
|---|---|---|---|---|---|---|---|
| Auditorium (Doors-In) | `Chambers\IMP auditorium_doors_in.wv` | 1000 (0 dB) | 500 (-30 dB) | 0 | 0 | on | 4.6 s IR, dry passes |
| Classroom | `Real Spaces\IMP classroom.wv` | 1000 | 500 | 0 | 0 | on | 539-byte block, the shortest path |
| Blur Pink / Blur White | `Special\White Noise.wv` | 0 | 1000 (0 dB) | 0 | 0 | off | 10 s noise, 100 % wet, no normalisation: a "blur" |
| Warehouse (Hall A-Dark) | `Halls\...` | 1000 | 500 | 0 | 0 | on | |
| Oven (Closed) | `FX\...` | 1000 | 500 | 0 | 0 | on | 541 bytes |

The 30 presets with Dry 0 / Wet 1000 are the Equalizer and Blur families
(the plugin as an FIR EQ). The corpus projects embed the IR when it was
recorded or edited in place (10 blocks, all written by FL 25), and the one
project with stretch -372 across four slots is a tuned-down room.

## 2. Parameter semantics

`ProcessParam` (`FUN_00878c20`; constants `0x8c7d6c = 0.001`, `0x906534` the
`.bss` `ln 11` (inferred), `0x8c7d64 = 0.1`, `0x879000 = 1001`, `0x879008 =
0.001`, `_DAT_00878fe8 = 2^-30`):

```
idx 0 dry    x = p0 / 1000;  0x460 = x == 1 ? 1 : (11^x - 1) / 10                 // 500 -> -12.7 dB
idx 1 wet    0x468 = (1001^(p1 / 1000) - 1) * 0.001                                // 500 -> 0.0306 (-30.3 dB), 1000 -> 1.0
             FUN_00622470(0x440, pan 0, 0x468 * info+0x20, p3 / 1000, 0)           // wet matrix: gain and wet separation
idx 2 width  FUN_00622470(0x420, pan 0, 1.0, p2 / 1000, 0)                         // input matrix: separation only
idx 3 circle recomputes the wet matrix as idx 1
idx 1000     this+0x50e = value != 0                                               // "link to file": the hidden parameter the save test reads
```

`info+0x20` is a double in the engine info block the host hands the plugin
(`this+0xcc`); it multiplies the wet gain and was not identified
(`inferred`: a host-side normalisation factor). `FUN_00622470` is the shared
pan/volume/separation matrix builder also used by the Love Philter output
stage; separation `s` in -1..1 cross-feeds the channels (negative widens,
positive narrows, the Reeverb 2 convention).

Form wheels (`FUN_00881170`, called on restore and on every wheel move;
`0x8812c8 = 7`, `0x8812d8 = 83.333`, `0x8812e0 = 0.001`):

```
predelay  0x40c = round((7^(v / 1000) - 1) * 83.333 * 0.001 * SR)     // = (7^x - 1) * SR / 12 samples; 1000 -> 0.5 s, 500 -> 137 ms
selfconv  0x414 = v / 1000
stretch   0x418 = exp(v / 1000 * K)                                   // K at 0x906500 (.bss), not resolvable; ratio 1 at 0; sign: positive = longer
eqmix     0x41c = v / 1000
norm      0x50f = NormBtn
then FUN_0087bd00: start the rebuild thread (section 3) unless recording
```

Dispatcher (`FUN_00874d70`): `FPD_Flush` (2) resets the engine (`FUN_008739c0`:
direct FIR and partition histories) and snaps the ramps; `FPD_SetSampleRate`
(4) stores the preview-player constants (`0x3e4 = 0.2 SR`, `0x3f8 = SR`,
`0x3fc = 0.05 SR`, `0x3b4 = 0.001 SR`) and does not rebuild the IR by itself
(the rebuild reads `info+8` when it runs); `FPD_SetPlaying` (0xc) and
`FPD_SetEnabled` (0xb) set flags; `FPD_LoadFile` (0x12) loads a dropped IR;
message 0x31 answers the IR length in bytes when linked to a file.

Latency (`FUN_00879cc0`): `0x514 = clamp(v, 0, round(5 SR))`, sent as
`FHD_SetLatency` (0x1e). This is the user's "latency" button value, not the
engine's own delay; the default 0 means the plugin reports no latency and
the predelay logic below keeps the output on time.

## 3. IR preparation (rebuild thread `FUN_0087b750`)

`FUN_0087bd00` creates a `TThread` (class `PTR_PTR_008749e8`) whose body is
`FUN_0087b750`; a state word at 0x49c coalesces repeated requests. The thread:

```
IR  = copy of the editor sample (form+0x6f8 -> +0x11d8), as 2 float channels (FUN_00626aa0(IR, 2, 1, 0, 0))
envs = copy of the 4 layer envelopes; eq = copy of the EQ envelope
if IR not empty:
    if normalize (0x50f):
        det = detector object (PTR_PTR_007958b0, mode 2) run over the IR; peak = max |det output|       // FUN_00635f20
        if peak > 0: IR *= 0.05 / peak                                                                   // 0x87bcc0 = 0.05: -26 dB target
    apply the 4 layer envelopes to the IR (FUN_00867c40)                                                 // the editor's volume/pan/... layers
    crop to the first region of the sample if it has one (FUN_00629c20 / FUN_0062c150)
    if 0x414 != 0: FUN_0087b670(IR, 0x414)                                                              // self-convolution, amount 0..1 (not decompiled)
    resample IR to SR * 0x418 (FUN_0062b090, mode 9: the 256-point sinc, docs/plugins/fl-interpolator.md) // the stretch
    if eq is a flat line at level y (FUN_00793a00):  IR *= level(y * k) with (11^x - 1)/10 (k = _DAT_0087bcc8)
    else:                                            FUN_0087b2b0(IR, eq, stretch)                      // FFT EQ over the IR (TFFTFilter; not decompiled)
    trim trailing samples with |x| < 2^-24 on both channels (0x8cfb1c)
lead    = 0x40c (predelay) + count of leading samples with |x| < 2^-24
block   = lead >= 512 ? largest power of two <= lead : 0
IR      = (lead - block) zeros prepended, or that many samples trimmed when negative
if block != 0x410: 0x410 = block; rebuild the TConvolver (FUN_0087b250)
TConvolver.SetImpulse(IR)                               // FUN_00873bc0
0x504   = predelay + IR length                          // tail length for the silence sleep
```

So "pre-delay" is realised partly as zeros at the head of the IR and partly
as the engine's own block delay: the largest power of two that fits in
`predelay + leading silence` (512 minimum) becomes the engine's head block
`0x410`, and the convolver is rebuilt with that block so its inherent delay
replaces the silence. An IR with 100 ms of silence at its head costs nothing;
an IR that starts at once, with predelay 0, runs through the zero-latency
path (`0x410 = 0`, section 4).

## 4. Signal chain (`Eff_Render` @ 0x0087a000) and the engine

```
if recording (0x400): append input to the record buffer (0x3d0), stop at 0x3e0 frames or on a peak over threshold
silence: if every |sample| <= 1e-6 (FUN_00636150): countdown 0x508 -= n, else 0x508 = 0x504
if 0x508 <= -n:  Dest = Source (or zeros of the dry path), snap ramps, return          // asleep after predelay + IR length of silence
scratch = Source * widthMatrix(0x420, ramped over n, cap info+0x34)                    // PTR_DAT_008f6ef8
lock 0x46c; TConvolver.Process(scratch, scratch, n); unlock                            // FUN_008739f0
Dest    = Source * dry (0x460, ramped 0x464)                                           // FUN_00622140
Dest   += scratch * wetMatrix(0x440, ramped)                                           // PTR_DAT_008f72f0
if the preview player is active: Dest += player output                                 // the Play button, not the effect
```

`TConvolver` (`FUN_008735f0(class, 1, nch 2, head 0x410)`, object 0x70 bytes):

| offset | meaning |
|---|---|
| +8 | FFT partition engine (`FUN_008702e0(class, 1, nch, head/2)`) |
| +0x10 | `TDirectFIR` (`FUN_0086fd40(class, 1, nch, 128)`), only when head == 0 |
| +0x18 | channels (2) |
| +0x1c | head length: `0x410`, or 128 when `0x410 == 0` |
| +0x20 | use the direct FIR (`0x410 == 0`) |
| +0x24 / +0x2c | per-channel scratch (`0x10000` bytes each), deinterleave buffers +0x44/+0x4c, output +0x54/+0x5c |

```
Process(in, out, n):                                      // FUN_008739f0 / FUN_00873ac0
    for each chunk of <= 16384 frames:
        deinterleave in -> L, R                           // PTR_DAT_008f7bc0
        if direct: directFIR.Process(L/R -> outL/R)       // FUN_00870090: per channel (*PTR_DAT_008f7298)(in, out, n, taps[ch]); 128 taps, zero latency
        fft.Process(L/R -> scratch)                       // FUN_00872520
        out = direct ? out + scratch : scratch            // PTR_DAT_008f7bb8 add / 008f7bf8 copy
        interleave out                                    // PTR_DAT_008f7660
SetImpulse(IR):                                           // FUN_00873bc0
    directLen = direct ? 128 : 0; pad IR to >= directLen
    deinterleave IR; directFIR.SetTaps(IR[0..128))        // FUN_00870000
    fft.SetImpulse(IR + directLen, len - directLen)       // FUN_00871ef0
```

FFT engine (`FUN_008702e0`; `+0xc4` channels, `+0xc8 = B` the base block =
`head/2`, 64 when head is 0; FFT size `2B`): the impulse is cut into
partitions of growing block size. `FUN_00871e60(size)` returns
`size * round(2^trunc(log2 6)) = size * 4`, capped so that a block never
exceeds 65535 samples; `FUN_00871650` counts partitions: each partition
holds 6 blocks of its size, sizes `B, 4B, 16B, ...`, until the remaining
impulse fits; the last partition takes `round(rest / size)` blocks, at least
2 (`FUN_00871750`). The plan (`FUN_008709c0`) builds per channel an array of
0x84-byte partition records: `+4` block count, `+8` block size, `+0xc` FFT
size `2 * size`, `+0x10` spectrum length `2 * size + 4`, `+0x1c` output
accumulator, `+0x34` the block spectra, `+0x44/+0x4c` input history, `+0x14`
fill position, byte `+2 = size > 4096` (large partitions run on the engine's
worker thread, event `+0xf4`, instead of inline). `SetImpulse` zero-pads each
block to its FFT size and transforms it (`PTR_DAT_008f76f8`).

```
fft.Process(in[ch], out[ch], n):                          // FUN_00872520, under lock +0x74
    zero out
    while n > 0:
        m = min(n, B - fill[0])                            // advance by the base block's free space
        for each partition p, channel ch:
            copy m input samples into p.history at fill[p] (and the mirror copy at fill[p] + size)
            out[ch] += p.accumulator[fill[p] .. fill[p] + m]
            fill[p] = (fill[p] + m) % p.size
            if fill[p] == 0:                                // a block of this partition is complete
                small partition: FUN_00873190 (forward FFT of the new block, multiply-accumulate with the stored spectra, inverse) then FUN_00872a70 (overlap into the accumulator)
                large partition: mark it and wake the worker (+0xf4)
        n -= m
```

That is a non-uniform partitioned convolution: the head of the IR is handled
by 6 blocks of `B` samples (and, with `0x410 = 0`, the first 128 taps by a
direct FIR for zero latency), later parts by blocks 4x and 16x larger, so the
CPU cost of a 10 s IR is a few FFTs of 65536 per block. The exact
multiply-accumulate (`FUN_00873190`, `FUN_00872a70`, `FUN_00871840`) and the
worker thread were not read beyond their role; the arithmetic is a textbook
frequency-domain delay line.

## 5. What stays unknown

- The stretch constant `K` (`.bss` 0x906500) and the `info+0x20` wet factor
  could not be read statically.
- `FUN_0087b670` (self-convolution) and `FUN_0087b2b0` (EQ curve onto the IR)
  were not decompiled; the EQ envelope's x axis is taken as 0..1 across the
  spectrum from the `EQ - 40Hz cut` style map presets.
- Which of the four kind-6 layers is volume, pan, and the two others, was
  not read (`FUN_00867c40`); all corpus states leave them flat.
- The engine's inherent delay with head block `0x410` is taken to equal
  `0x410` samples because the rebuild subtracts exactly that from the lead;
  the FFT output alignment was not checked sample by sample.
- `0x50c` and the FPD message 0x24 that sets `0x50d` are GUI.
- The recording path (Record / Trigger impulse) and the preview player were
  only mapped, not read.

## Corrections

Found while implementing `crates/plugins/src/convolver.rs` from the DLL and the corpus:

1. Flat EQ gain is `(3^(2y) - 1) / 2` (`0x90653c` is ln 3, factor 0.5), not the `(11^x - 1)/10` curve.
2. The stretch constant at `0x906500` is ln 2: ratio = `2^(v/1000)`.
3. `info+0x20` is 44100 over the rate (the same `TAudioRenderer` field as Love Philter). It keeps the wet level constant across rates.
4. Normalise uses `TRMSLookahead`: a 300 ms window over L^2 + R^2 divided by 2W, square root, peak scaled to 0.05. An RMS detector, not a peak detector.
5. Not every preset is version 14 with flat envelopes: Blur Pink and Blur White have a drawn layer (3 points), Blur Pink a sloped EQ curve, and two presets are version 15 with latency 512 and 8192.
6. Paths: 36 presets name `%USERPROFILE%\Dropbox\IL_Files\...`; the files ship under `Data/Patches/Impulses` and are found by name. Four project states have repeated `%FLStudioFactoryData%\` prefixes, five an empty path.
7. Embedded WAVs: five are 16-bit PCM and five 32-bit float.
8. Layers (`FUN_00867c40`): three of the four envelopes drive pan, volume and separation through `FUN_00622470` per sample.
9. Separation in `FUN_00622470`: a positive value is `(5^s - 1) / 4` of the way to mono.
10. There is no partition latency in the binary. `0x514` is only sent to the host (`FHD_SetLatency`) and shown in the GUI; the engine's block delay is absorbed by the impulse's own lead.
11. The resampling (mode 9 of `FUN_0062aad0`) is FL's interpolator at 256 points a side: a sinc under a Hann window leaned by `0.36 + 0.64 c`, low-passed to `pi / speed` when shrinking, with the end frames held past the impulse's ends (`docs/plugins/fl-interpolator.md`).
