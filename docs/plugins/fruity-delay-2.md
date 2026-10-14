# Fruity Delay 2: state, parameters, signal chain

Recovered from `Fruity Delay 2_x64.dll` (FL Studio 2026, Delphi native plugin,
image base 0x400000). Ghidra project `ghidra/proj`, decompiles under
`ghidra/out/delay2/` (`vmt/` for the SDK slots, `line/` for the delay-line
class). Corpus: 62 uses in 17 of the 165 `.flp`, 6 shipped presets. Three
Ghidra runs: the third (`line/`) was needed because the three feedback modes
are instance function pointers on a separate delay-line object and the
callee walk cannot see them; their addresses were taken from the object's
constructor first. Nothing in FLEngine is called for audio; the host only
supplies the sample rate, block size, tempo and a ramp limit.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `inferred`, `asm`.

## How the plugin is wired

`CreatePlugInstance` @ `0x006bf1a0` -> constructor `FUN_006bfac0`, class
reference `PTR_PTR_006c0c50` -> VMT `0x006bf030` (`TTestPlug`).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006bf1f0 |
| 2 | Idle_Public | 0x006b9ce0 |
| 3 | SaveRestoreState | 0x006bfa00 |
| 4 | GetName | 0x006c07e0 |
| 5 | ProcessEvent | 0x006bffc0 |
| 6 | ProcessParam | 0x006c0010 |
| 7 | Eff_Render | 0x006c0910 |

`TFruityPlugInfo` at `0x007115a0`: SDKVersion 1, LongName "Fruity Delay 2"
(`0x006bef50`), ShortName (`0x006bef5f`), Flags 0x10400000, NumParams 8.

Plugin object fields:

| offset | type | meaning |
|---|---|---|
| 0xd4 / 0xdc | ptr / i32 | scratch buffer and its capacity in frames |
| 0x120 | i32[8] | the parameters, saved state |
| 0x140..0x14c | f32[4] | current input matrix `[LL, RR, RL, LR]` |
| 0x150..0x15c | f32[4] | input matrix targets, from pan and volume |
| 0x160 | f32 | input pan, -1..1 |
| 0x164 | f32 | input volume gain |
| 0x168 | f32 | dry gain |
| 0x16c | ptr | the delay-line object (class VMT `0x006bb110`, constructor `FUN_006bb350`) |
| 0x174 | i32 | host block size (Dispatcher ID 3) |
| 0x178 | f32 | stereo offset in units of 10 us |
| 0x17c, 0x180 | i32 | stereo offset for L and R in bytes (one of them is 0) |
| 0x184 | f32 | tempo in BPM, initialised to 140.0 (0x430c0000), updated by `ProcessEvent` FPE_Tempo |
| 0x188 | struct | feedback filter: `+4` (0x18c) f32 coefficient, `+0x28/+0x30` (0x1b0/0x1b8) f64 state L, R |
| 0x1f0 | u8 | input matrix needs recomputing |

Delay-line object (`FUN_006bb350`, element size 8 = one stereo frame):

| offset | meaning |
|---|---|
| +0x08 | buffer start |
| +0x10 | `pos`: the oldest frame, read and zeroed each sample |
| +0x18 | `wr`: `pos + D` frames, where new input is accumulated |
| +0x20 | buffer end; +0x28 size in bytes; +0x2c element size 8 |
| +0x30 / +0x34 | feedback target / current (ramped per block) |
| +0x38 | host structure (sample rate at +8, rate factor double at +0x20, ramp limit float at +0x34) |
| +0x40..+0xd8 | ten `{code, self}` method pairs; +0x80 invert, +0x90 normal, +0xa0 ping-pong. Default set `0x006bc1c0 / 0x006bc060 / 0x006bc2f0`; AVX set (`[0x007129d8]+0x16` set) `0x006bca20 / 0x006bd140 / 0x006bd340` |

## 1. State layout (event 213, 32 bytes)

`SaveRestoreState` (`FUN_006bfa00`) is a verbatim dump of `this+0x120`, 0x20
bytes, with the GetValue/UpdateValue loop on restore. Eight little-endian
int32 in parameter index order, matching the DFM `Tag`s.

| byte | type | idx | parameter (DFM object, hint) | range | default | corpus | confidence |
|---|---|---|---|---|---|---|---|
| 0..3 | i32 | 0 | Input panning (`InPanWheel`) | -128..128 | 0 | 0 (x37), -84..106 | loader, dfm, corpus, preset |
| 4..7 | i32 | 1 | Input volume (`InVolWheel`) | 0..160 | 128 | 128 (x17), 0 (x9), 43..121 | loader, dfm, corpus, preset |
| 8..11 | i32 | 2 | Dry level (`DryWheel`) | 0..160 | 128 | 128 (x49), 0 (x12), 160 | loader, dfm, corpus, preset |
| 12..15 | i32 | 3 | Feedback level (`FBWheel`) | 0..127 | 64 | 64 (x17), 0..104 | loader, dfm, corpus, preset |
| 16..19 | i32 | 4 | Time (`TimeWheel`) | 1..768 | 144 | 192 (x20), 144 (x14), 384 (x8), 96 (x8), 6, 15, 48, 62 | loader, dfm, corpus, preset |
| 20..23 | i32 | 5 | Time stereo offset (`TimeOfsWheel`) | -512..512 | 0 | 0 (x32), -421..439 | loader, dfm, corpus, preset |
| 24..27 | i32 | 6 | Feedback mode (`ModeSelect`) | 0 Normal, 1 Invert, 2 P.Pong | 0 | 2 (x53), 0 (x6), 1 (x3) | loader, dfm, corpus, preset |
| 28..31 | i32 | 7 | Cutoff freq (`CutoffWheel`) | 0..128 | 128 | 128 (x36), 12..119 | loader, dfm, corpus, preset |

Preset check (the four older `.fst` name the plugin "Fruity delay 2", the two
newer ones "Fruity Delay 2"). Derived columns use section 2: Time in steps
(16ths) = v/48, offset in ms, fb = v/128, cutoff coefficient `c` before the
host rate factor.

| preset | Pan | InVol | Dry | FB | Time | Ofs | Mode | Cut | reading |
|---|---|---|---|---|---|---|---|---|---|
| Default | 0 | 128 (x1.0) | 128 (x1.0) | 64 (0.50) | 144 (3 steps, dotted 8th) | 0 | Normal | 128 (c 1.0) | plain dotted-eighth echo, no damping |
| Default (send) | 0 | 128 | 0 | 64 | 144 | 0 | Normal | 128 | same, dry muted for a send bus |
| Stereo effect | 66 (0.52 right) | 128 | 128 | 64 | 144 | -128 (2.3 ms, left) | P.Pong | 119 (0.80) | input panned right, echoes alternate, tiny L offset |
| Stereo effect (send) | 66 | 128 | 0 | 64 | 144 | -128 | P.Pong | 119 | send version |
| Weird (send) | 28 | 128 | 0 | 107 (0.84) | 48 (1 step) | -358 (62 ms, left) | P.Pong | 89 (0.37) | fast 16th ping-pong, long tail, heavily damped |
| Widen | 106 (0.83 right) | 103 (x0.59) | 128 | 23 (0.18) | 6 (1/8 step, 13 ms at 140) | -394 (101 ms, left) | P.Pong | 128 | a Haas widener: one short echo to the other side |

The corpus agrees with the presets: ping-pong is the mode in 53 of 62 uses,
and Time sits on musical values (192 = 4 steps = a beat, 144, 96, 384).

## 2. Parameter semantics (`ProcessParam` @ 0x006c0010)

Flags per the SDK; `REC_FromMIDI` uses 2^-30 at `0x006c0410`. Every
`REC_UpdateValue` sets `this+0x1f0`. Constants: float 1/128 at `0x006ff208`
(via `PTR_DAT_00712600`), 0.1 at `0x006ff200`, 0.5 at `0x006ff1f0`, Ln 11 in
`.bss 0x0072146c` (set at `0x005e1a55` from the double 11.0 at `0x005e2060`),
doubles 1001.0 @`0x006c0420`, 1/512 @`0x006c0428`, 50.0 @`0x006c0430`, 21.0
@`0x006c0438`, 0.05 @`0x006c0440`, float 1.0 @`0x006c0448`, float 100000.0
@`0x006bffa8`, double 0.3125 @`0x006bffb0`.

```
g(v)  = x == 1 ? 1 : (11^x - 1) / 10,  x = v / 128          // the usual Fruity gain curve

idx 0  pan    = v / 128                       -> this+0x160, matrix dirty
idx 1  invol  = g(v)                          -> this+0x164, matrix dirty      (160 -> x1.90, +5.6 dB)
idx 2  dry    = g(v)                          -> this+0x168
idx 3  fb     = v / 128                       -> line+0x30                     (127 -> 0.992; never reaches 1)
idx 4  time                                   -> recompute lengths (FUN_006bfea0)
idx 5  ofs10us = (1001^(|v| / 512) - 1) * 50  -> this+0x178, then recompute lengths
idx 6  mode                                   -> used directly by Eff_Render
idx 7  c      = min(1.0, (21^(v / 128) - 1) * 0.05 * host.ratefactor)   -> this+0x18c   (host+0xcc -> +0x20, a double)
```

Lengths (`FUN_006bfea0`, also run on SetSampleRate, SetBlockSize and every
tempo event, under the host's plugin lock):

```
ofsFrames = Round(ofs10us * SR / 100000)                     // 10 us units -> samples
D         = Round(time * SR * 0.3125 / tempo)                // 0.3125 = 60 / 192: Time is in 1/192 of a beat = 1/48 of a step
line.resize(blockSize + ofsFrames, D)                        // FUN_006bb1f0: total = blockSize + ofsFrames + max(D, 1) frames; wr = pos + max(D, 1)
ofsL = ofsFrames * 8;  ofsR = 0
if params[5] > 0: swap(ofsL, ofsR)                           // positive offset delays the right channel, negative the left
```

Resizing reallocates and clears the line (`FUN_006bb190` calls the object's
virtual `Clear`), so a tempo change or a Time sweep wipes the echo tail. The
stereo offset reaches `(1001 - 1) * 50 = 50000` x 10 us = 500 ms at |v| =
512, exponentially: 128 -> 2.3 ms, 256 -> 15.3 ms, 384 -> 88 ms. The time
display (`FUN_006c0490` idx 4) prints `v div 48 : v mod 48`, steps and 48ths
of a step; idx 5 prints the ms value with "left" or "right"; idx 6 prints
Normal / Invert / P.Pong; the rest print `v / Max`.

Tempo arrives through `ProcessEvent` (`0x006bffc0`): EventID 0 (FPE_Tempo)
stores the float value at `this+0x184` and recomputes lengths. Before the
first event the line is sized for 140 BPM.

Input pan law (`FUN_006bfdb0`, run at the top of `Eff_Render` when dirty),
written into the matrix targets `[LL, RR, RL, LR]`:

```
if pan < 0:  a = pan + 1;  RL = (1 - a) * 0.5 * vol;  RR = a * vol;  LL = vol - RL;  LR = 0
else:        a = 1 - pan;  LR = pan * 0.5 * vol;      LL = a * vol;  RR = vol - LR;  RL = 0
```

Centre is `[vol, vol, 0, 0]`. Hard left is `LL = RL = 0.5 vol, RR = 0`: the
delay input becomes a -6 dB mono fold-down in the left channel. It is a pan
of the signal going *into* the line only; the dry path is untouched.

## 3. Signal chain (`Eff_Render` @ 0x006c0910)

```
if dirty: recompute [LL, RR, RL, LR] targets (above)
ramp the four matrix gains this+0x140.. towards 0x150.. (FUN_005dff80 x4, Balance's ramp, max step = host+0x34)
grow scratch (this+0xd4) to Length frames (rounded up to 4)
tmp = M(t) * Source                                         // 2x2 apply, pointer 0x007214f0 -> 0x005df020; L' = L*LL + R*RL, R' = R*RR + L*LR
line.add(tmp.L, Length, line.wr + ofsL)                     // FUN_006bb690: adds (not copies) one channel into the line, stride 8, wraps once
line.add(tmp.R, Length, line.wr + 4 + ofsR)                 // copy routine 0x006bb670 (AVX 0x006bd3c0), pointer DAT_00723558
out = (dry == 0.0) ? Dest : tmp
mode[params[6]](line, out, Length, this+0x188)              // +0x90 normal, +0x80 invert, +0xa0 ping-pong
if out != Dest: Dest = Source * dry + tmp                   // pointer 0x00721568 -> 0x005de9b0 (AVX 0x005e0950)
```

### The three modes (`FUN_006bbfa0` normal, `FUN_006bc0e0` invert, `FUN_006bc230` ping-pong)

Each wrapper first ramps the feedback (`line+0x34` towards `+0x30`, same
ramp routine, per-frame delta `fbd`), then runs one of three loops that
differ in two lines. `c` is the cutoff coefficient, `lpL/lpR` the one-pole
state (doubles, loaded as floats), `eps = DAT_00702fcc` a denormal floor.

```
for each frame:
    dL, dR = line.pos[0], line.pos[1]            // what was written D frames ago: the echo
    normal:     out = (dL, dR);   fL = dL;  fR = dR
    invert:     out = (dR, dL);   fL = dL;  fR = dR
    ping-pong:  out = (dR, dL);   fL = dR;  fR = dL
    lpL += (line.wr[0] + fb * fL - lpL) * c;  line.wr[0] = |lpL| <= eps ? 0 : lpL
    lpR += (line.wr[1] + fb * fR - lpR) * c;  line.wr[1] = |lpR| <= eps ? 0 : lpR
    line.pos[0] = line.pos[1] = 0
    pos += 1 frame; wr += 1 frame; wrap each at the end
    fb += fbd
```

So with `w` the line contents and `D` the delay in frames:

```
w[n] = LP_c( x_in[n] + fb * w[n - D] )        normal, per channel
y[n] = w[n - D]

invert:     y = swap(w[n - D]); feedback stays in its own channel
ping-pong:  w_L[n] = LP_c( x_L[n] + fb * w_R[n - D] ),  w_R[n] = LP_c( x_R[n] + fb * w_L[n - D] );  y = swap(w[n - D])
```

The one-pole low pass sits inside the loop, so every pass around the
feedback darkens the echo again; at the default cutoff (128) the coefficient
is 1.0 and the filter is transparent. "Invert" does not invert polarity: it
swaps the output channels of every echo while the regeneration stays per
channel, so all echoes of a left-panned input come out on the right.
Ping-pong crosses the feedback too, so echoes alternate sides, and the first
echo of a left input is already on the right. The stereo offset delays one
channel's *input* into the line by `ofsFrames` (the input is added at `wr +
ofs`), so that channel's first echo and all its regenerations arrive late by
the offset, which with ping-pong gives the lopsided bounce of the "Weird"
preset.

Feedback max is 127/128 = 0.992, so the loop always decays, slowly. The dry
path is a plain `Source * dry` added to the wet scratch; when Dry is 0 the
wet is written straight to the output.

Re-implementation note: one stereo circular buffer of `D + ofs + block`
frames, an add-into-line write at `D` frames ahead (with the per-channel
offset), the three-way mode switch above, a one-pole on the write slot, and
ramps on the input matrix and feedback. Resample `D` from tempo as
`time * SR * 60 / (192 * tempo)`; match FL's clear-on-resize if you want its
behaviour on tempo changes.

## 4. What stays unknown

- `host+0x20`, the double that scales the cutoff coefficient. It is per
  host structure (same slot Balance reads its `max_step` from at `+0x38`)
  and is most likely `44100 / SR`; not confirmed.
- `DAT_00702fcc`, the denormal floor in the feedback loop (a `.data` float,
  not read), and the ramp limit `host+0x34`.
- The AVX twins (`0x006bca20`, `0x006bd140`, `0x006bd340`, `0x005e0950`,
  `0x006bd3c0`) were not read. The default copy routine adds; the AVX one
  was checked to add too (`addss` at `0x006bd3da`).
- The other seven method pairs on the line object (`+0x40..+0x70`, `+0xb0..
  +0xd0`) are not called by this plugin; they look like variants for other
  Fruity delays sharing the class.
- `Dispatcher` 0x33/0x34/0x39 are GUI; flags bit 28.

## Implementation notes (crates/plugins/src/delay_2.rs)

- The host rate factor is taken as `44100 / sample_rate` per the doc's guess, so at 48 kHz the default cutoff is c = 0.92 rather than transparent. Unconfirmed.
- The delay line clears only when D or the stereo offset changes; memory is reserved in `set_sample_rate` and `set_tempo`, never in `process`.
- Tempo arrives through `Effect::set_tempo`; 140 BPM until the host calls it, as in FL.
