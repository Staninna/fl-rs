# Gross Beat: state, slots, transport, signal chain

Recovered from `Gross Beat_x64.dll` (FL Studio 2026, Delphi native plugin,
image base 0x400000; the envelope classes, the delay lines and the sinc
reader are compiled in, and the vector maths is Intel IPP bound by name from
`dsp_ippv2.dll`). Ghidra project `ghidra/proj/grossbeat`; decompiles under
`ghidra/out/grossbeat/`: `c1/` the constructor, `vmt/` the SDK slots with
callees, `menu/` the option handlers and setters, `env/` the envelope class
and the delay lines, `k/` the smoother, DC and split kernels, `kasm/` and
`rasm/` the listings of `ProcessParam` and `Eff_Render`, `sinc*/` the
resampler, `ipp/` the IPP binder, `dref/` the writers of the dispatch
pointers. Corpus: 35 uses in the 165 `.flp`, 9 shipped presets plus three
`Gross Beat.fst` templates.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `asm`, `inferred`.

## How the plugin is wired

`CreatePlugInstance` @ `0x0083f230` -> constructor `FUN_0083fcf0`, class
`TTestPlug`, VMT `0x0083f0f0` (instance size 0x740, parent
`TDelphiFruityPlug`).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x0083f290 |
| 3 | SaveRestoreState | 0x0083f890 |
| 4 | GetName | 0x008411f0 |
| 5 | ProcessEvent | 0x00840ad0 (`FPE_Tempo` -> `0x17c`, then `FUN_00841ac0`) |
| 6 | ProcessParam | 0x00840b20 |
| 7 | Eff_Render | 0x008420f0 |
| 14 | NewTick | 0x0040c5a0 (the empty default: the plugin takes no ticks) |
| 20, 25 | Idle | 0x00841810, 0x00841500 (GUI only) |

The constructor reports a latency of 15 frames (`FHD_SetLatency`, host
dispatcher 0x1e with 15) and never changes it.

Objects inside the plugin:

| offset | what |
|---|---|
| 0x144 | i32[9], the parameters |
| 0x168 | i32, the envelope the editor shows (0 time, 1 volume) |
| 0x16c..0x170 | u8 options: remove DC, attack compensation, HQ resampling, view scratch clock, click reduction (0 off, 1 low, 2 high) |
| 0x173 | playing (Dispatcher 12) |
| 0x174 | first block since a flush |
| 0x175 | holding |
| 0x176 | `PM_HQ_NonRealtime` from `FPD_ProcessMode` |
| 0x178 | hold bits: 1 the Hold knob, 2 the time slot's hold, 4 the volume slot's |
| 0x17c | f32 tempo, 120 at construction |
| 0x184 | PPQ, 96 at construction (Dispatcher 14 `FPD_SetTimeSig`) |
| 0x188 + 0x204 t | lane t (0 time, 1 volume): +0 slot playing, +4 slot wanted, +0xc slot pending, +0x10 tick it is due, +0x18 the 36 `TBRSlot` pointers, +0x154 the slot playing, +0x15c its run, +0x1dc beats a frame, +0x1ec attack compensation in frames |
| 0x590 | `TBRDelay_32FS`, the ring of past input |
| 0x598 | `TBasicDelay_32FS` of 15 frames, the dry signal |
| 0x5a0, 0x5a8 | f64 two bars and one bar in frames |
| 0x5b0, 0x5b8 | the sinc reader, and a second one for a change of tempo |
| 0x5c0 | 16 with HQ resampling or HQ rendering, 8 without |
| 0x5c4 | f64 the time offset of the last frame |
| 0x5cc, 0x5d0, 0x5d4, 0x5d8 | hold: the tick it is due, the read base it loops, the base saved, the length index (5) |
| 0x5dc | three smoothers on the scratch offset |
| 0x648..0x670 | GUI values, the click correction and the last output |
| 0x674 | the tension curve |
| 0x680, 0x6a4 | the volume smoothers |
| 0x6c8 | the DC blocker |

## 1. State layout (event 213)

`SaveRestoreState` (`FUN_0083f890`). Save writes version `0x00090003`.
Load reads an i32 and turns the state down from `0x00090004` up; a value
over 0xffff is shifted down 16 bits, so the corpus's `0x00080003` is version
8 (`loader`).

| bytes | what | versions |
|---|---|---|
| i32 | version | all |
| i32[9] | the parameters into `0x144`; 8 of them before version 2, when the ninth is 65536 | all |
| u8[9] | `0x168` (i32) and the five option bytes; 8 bytes before version 5, 7 before version 3. A byte not read keeps 1 for click reduction and view scratch clock, which the loader sets first | all |
| 72 slots | 36 time slots, then 36 volume slots | all |
| u8 | trigger slots from MIDI notes (`0x731`); 1 when absent | 9 |

A slot is:

| bytes | what |
|---|---|
| u8 n (0xff: u32 n), n bytes | the name, ANSI; before version 4 "Slot N" reads as no name |
| i32 | trigger sync, 0 to 5: off, 1/4, 1/2, 1, 2, 4 beats |
| i32 | position sync, same values |
| i32 | hold: 0 off, 1 last bar, 2 next bar |
| u8 | link volume slot |
| u8 | key-held (back to the slot before on key release) |
| u8 | one-shot (back to slot 0 once done) |
| stream | the envelope, a `TMMap` stream (`crates/plugins/src/menvelope.rs`): kind 3, version 2 or 3, points of f64 x, f64 y, f32 tension, u8 mode and three bytes, then flags, active and three markers |

The 15 bytes after the name go straight into the slot object at `0xd0`
(`dfm`: `TrigSyncSel`, `PosSyncSel`, `ForceHoldSel`, `LinkOtherBtn`,
`KeyHoldBtn`, `OneShotBtn`). The constructor gives every slot trigger and
position sync 5 and the envelope `(0, 1)`, `(4, 1)` in mode 2 (`loader`).
The loader asks each envelope whether it is still that one (`TBRSlot`
VMT `0x0083eec0` slot 16, `FUN_0078bd90`) and keeps the answer at `0xe4`;
`Eff_Render` skips a lane whose slot says yes.

Every corpus and preset state parses to its last byte: 32 of version
`0x80003` and 3 of version 8 in the projects; presets of version 4 (6),
`0x80003` (2) and 8 (1). All 35 corpus states have HQ resampling on; 14
remove DC, 3 compensate the attack, click reduction is low in 33. Every
envelope is 4 beats long. Point modes in the corpus: 0 (20787), 2 (19437),
6 (1824), 3 (161), 5 (124), 7 (60), 9 (20), 4 (17), 1 (6), 8 (2).

## 2. Parameters (`ProcessParam` @ 0x00840b20)

| idx | parameter (DFM) | range | default | effect |
|---|---|---|---|---|
| 0 | Time slot (`TimeSlotSel`) | 0..35 | 0 | `FUN_00843af0`: wanted slot; while playing `FUN_00843fe0` schedules it |
| 1 | Volume slot (`VolSlotSel`) | 0..35 | 0 | same for lane 1 |
| 2 | Time mix (`TimeMixWheel`) | 0..256 | 256 | read in `Eff_Render` |
| 3 | Volume mix (`VolMixWheel`) | 0..256 | 256 | read in `Eff_Render` |
| 4 | Volume attack (`VolAttWheel`) | 0..10000 | 1765 | `FUN_00841fb0` |
| 5 | Volume release (`VolRelWheel`) | 0..10000 | 5000 | `FUN_00841fb0` |
| 6 | Volume tension (`VolTnsWheel`) | -256..256 | 0 | `FUN_00611cd0(0x674, t)` |
| 7 | Hold (`HoldBtn`) | 0, 1 | 0 | hold bit 1 |
| 8 | Scratch | 0..65536 | 65536 | `0x650 = v / 65536 - 1` |

The defaults are the DFM wheel values and `Default.fst`.

- Attack and release, `FUN_00841de0`: `ms = 501^(v * 1e-4f) - 1` (as f32),
  frames `= ms * 0.001 * SR` (`FUN_00841ec0`). `FUN_00841fb0` sets both
  volume smoothers to rise in `attack / sqrt 2` and fall in
  `2 release / sqrt 2` frames, and the attack compensation of both lanes
  to `attack` frames when that option is on (0 otherwise).
- A smoother's time `T` (`FUN_00608c30`): coefficient `exp(ln 0.01 /
  max(T, 1))`, 0 for `T <= 0`.
- Tension (`asm` 0x840cc3..0x840d3b): `t = sign(v) (1001^|v / 256| - 1)`
  as f32. `FUN_00611cd0` keeps `t`, `1 / |t|` and `ln(|t| + 1)`.
- Hold (`asm`): the bit changes only where it turns "any hold" on or off
  (`(holds > 0) != (v > 0)`), and a hold it starts is due at the next
  multiple of 4 beats (`FUN_00843fa0`).

## 3. Time, tempo and the host (`Dispatcher` @ 0x0083f290)

The song position comes from the host on demand: `FUN_00841190` calls the
host dispatcher with `FHD_GetMixingTime` (0x24) and format 4, ticks, and
rounds the double (`FUN_0040c3e0`, round half even). Nothing else in the
plugin reads the song's place: the envelopes run free at the tempo between
the moments below.

| message | handler | what |
|---|---|---|
| `FPE_Tempo` | `FUN_00840ad0` | tempo -> `0x17c`, then `FUN_00841ac0` |
| 3 `FPD_SetBlockSize` | `FUN_00841a20` | ring of `SR * 240 / 10 * 3 + 16` frames plus the block and 64 guard frames: three bars at 10 BPM; then `FUN_00841ac0` |
| 4 `FPD_SetSampleRate` | | `FUN_00841fb0`, `FUN_00841ef0`, `FUN_00841920`; DC blocker `a = 44100 / SR * 2.2835e-5`, `b = 0.96` |
| 12 `FPD_SetPlaying` | `FUN_00841cf0` | on: flush (`FUN_00843450`), first block, `FUN_00844320`. Off: clear the ring, hold bits `&= 1`, hold off |
| 13 `FPD_SongPosChanged` | `FUN_00844320` | only while playing |
| 14 `FPD_SetTimeSig` | | PPQ -> `0x184`, sync table (`FUN_00844a90`), `FUN_00841ac0` |

`FUN_00841ac0`, the tempo: `bar = (f32)(SR * 240) / tempo_f32` frames,
`span = 2 bar`, beats a frame `4 / bar`. The reader's base is put
`round(span + 0.5) + 15` frames behind the ring's write position (clamped to
15..len - 15). While playing, a change of `span` by more than 8 frames hands
the old base and span to the second reader, which the next block fades out
over `max(1, round(SR / 1000))` frames. The saved hold base is set to the
new base too.

The sync table (`FUN_00844a90`): `[0, PPQ >> 2, PPQ / 2, PPQ, 2 PPQ, 4 PPQ]`
ticks.

`FUN_00844320`, the song started or moved: restore the base when holding;
holding off; the hold is due 4 beats on if a hold bit is set; both pending
triggers off; choose each lane's wanted slot (`FUN_00844060`).

`FUN_00844060(lane, slot)`, choosing a slot:

1. A volume lane whose time slot has "link" takes the time slot's number.
2. The run starts over (`FUN_00787050`): segment 0, which has length 0, so
   phase 1.
3. Hold bit `2 << lane` follows the slot's hold; a hold that starts is due
   at once ("last") or 4 beats on ("next"); the last hold bit going off
   makes the end due at once.
4. With position sync `p > 0` the run is moved on by
   `(at mod table[p]) / PPQ` beats, `at` the rounded tick, or while a hold
   is due `at + 256 PPQ - hold_tick`; plus the attack compensation in
   beats. One step of `FUN_00788d60` does it, so the run walks through the
   segments it passes.
5. One-shot: slot 0 is pending, due 4 beats on.
6. If the new or the old time slot links, the volume lane is chosen again.

`FUN_00843fe0`, the parameter asks for a slot while playing: with trigger
sync off it is chosen at once; otherwise it is pending until the next
multiple of `table[trigger]` ticks, `ceil(now / len) len`. `Eff_Render`
chooses a pending slot at the first block whose rounded tick has reached
it.

## 4. Signal chain (`Eff_Render` @ 0x008420f0)

Stopped (`0x173` clear): the input passes through, on time. If the last
output frame is not 0, the block fades from it to the input along
`(cos(pi i / (n - 1)) + 1) / 2`.

Playing, per block of `n` frames:

```
now = round(GetMixingTime(ticks))
for each lane: if due <= now: choose its pending slot
hold: if due <= now:
        no hold bits: if holding, base = saved, holding off, jump
        else: if not holding, saved = base, loop = base - round(bar * 4 PPQ / 4 PPQ), holding on
              base = loop; due = now + 4 PPQ; jump        (jump only with click reduction on)
if not holding: write the input into the ring              (FUN_007802d0)
dry = input 15 frames late                                 (TBasicDelay_32FS, FUN_007801d0)
scratching = scratch < 0, or a scratch smoother not yet at it
time lane on = (slot not the default and time mix != 0) or holding or scratching or a click correction left or jump
```

Time lane on:

```
y[i]   = time envelope, stepped then read, per frame        (FUN_00789a70, f64)
if scratching: y += 3 cascaded smoothers of the scratch value (FUN_00841ef0: rise 0.3 SR / sqrt 3, fall 0.2 SR / sqrt 3); y = max(y, 0)
off[i] = y[i] * span                                        (ippsMulC_64f_I)
speed  = |off[i] - off[i - 1] + 1|                          (f32; ippsAddC_32f_I, ippsAbs_32f_I)
not HQ (0x5c0 = 8): 8 taps, the sinc at full band
HQ (16):  speed smoothed by a one-pole of 1, 1 or 5 ms by click reduction (FUN_00608d10)
          max > 1: 16 taps, per frame cut = pi / max(speed, 1), 2 cos(cut)   (ippsThreshold_LT_32f, ippsDiv_64f_A53, ippsCos_64f_A53)
          else:    cut = pi; 16 taps if min < 1 - 2^-24, else 8
wet[i] = sinc(ring, base + i + trunc(off[i]), frac(off[i]))   (FUN_00843890; base moves on n)
a tempo change in flight: second reader at y * old span, 16 taps; wet = old (1 - t) + wet t, t = i / min(fade, n)
click reduction: where the unsmoothed speed > 0.002 SR (or frame 0 after a hold jump)
                 corr += last_raw - wet[i]; then wet[i] += corr; corr *= 0.01^(1 / (ms SR / 1000)), ms 25, 25, 35
time mix m != 256: wet = wet m + dry (1 - m), m = v / 256
```

Time lane off: the run steps once by `n` beats-a-frame, the last offset is
`span`, the base moves on `n`, and `wet = dry`. So a default time slot
plays the dry signal 15 frames late, bit for bit.

The volume lane:

```
on (slot not the default and volume mix != 0):
   g = volume envelope per frame (FUN_00788d60, f32)
   tension t > 0: g = 1 - ((t + 1)^(1 - g) - 1) / t;  t < 0: g = ((|t| + 1)^g - 1) / |t|     (FUN_00611d60, ippsExp_32f_A11)
   mix m != 256: g = g m + (1 - m)
off: the run steps once; g = 1
first block: both smoothers start at g[0]
g through the two volume smoothers in turn                   (FUN_00608dd0: y = x + k (y - x), k the rise
                                                              coefficient where y <= x, else the fall one)
out = wet * g                                                (FUN_0062cfa0)
remove DC on: per side v += (x - s) a; s += v; v *= b; out = x - s      (FUN_0062bf00)
```

### The envelope run (`FUN_00788d60`, `FUN_00789a70`, `DSP_MEnvelope.pas`)

A run is a segment index, a phase in it, a rate (1 over the segment's
length in beats), the start value and the rise. Each frame adds `step *
rate` to the phase first and reads after. Past 1, the overshoot turns back
into beats, the segment ends, the next starts (after the last comes point
0, whose length is 0 and is skipped), and the rise is the next point's
value (f32) less the start. The shape inside a segment is the 13-mode curve
of `crates/dsp/src/curve.rs`; the evaluator here is the same formulas
(`asm`). An envelope's points are kept as f32 `y`.

### The sinc reader (`FUN_00617d80`, kernel `FUN_00614d50`)

Kernel table `0x00893880`, entry for 2 channels of f32 and more than 3 taps:
`FUN_00614d50`. For a frame `p + f` with `taps` N:

```
w(x) = sin(x cut) / (x pi) * (0.36 + c + 0.64 c^2),  c = cos(x pi / N)
out  = 0.5 * sum over k = -(N-1)..N of frame[p + k] * w(f - k)
```

`f` is clamped up to 1e-25. The kernel starts the sines and the window
cosines with `ippsSin_64f_A50` and walks them by the f32 recurrence
`s(k+2) = 2 cos(step) s(k+1) - s(k)`, four lanes at a time (frames p + 1,
p, p + 2, p - 1), with `rcpps` and one Newton step for `1 / (x pi)`
(`FUN_00614ad0`).

## 5. What stays unknown

- `TBRSlot`'s "still the default" test (`FUN_00784ad0`) compares point
  records; which fields it looks at was not read. Here it is x, y, tension
  and mode.
- Whether FL sends `FPD_SongPosChanged` when a song or pattern loops back.
  `inferred`: the implementation counts a loop as a move.
- The MIDI and keyboard slot triggers run on the GUI thread (Dispatcher
  0x17 and 0x18 post window messages 0x54f and 0x550); not read further.
- The GUI's own state (`0x648`, `0x64c`, the scratch clock) is left out.

## Implementation notes (crates/plugins/src/gross_beat.rs)

- The engine calls `Effect::tick` for a plugin that says
  `Effect::follows_song`, and cuts its blocks at ticks. The first tick is
  `FPD_SetPlaying(1)`; a tick that does not follow the one before is
  `FPD_SongPosChanged`; `Effect::reset` is `FPD_SetPlaying(0)` and a flush.
  The rounded mixing time is the last tick plus the frames since it.
- The ring is FL's: three bars at 10 BPM plus 16 frames, the 512-frame
  block and 64 guard frames, sized when the sample rate is set and never
  after. A tempo change only moves the reader, so `set_tempo` asks for no
  memory on the audio thread, and every tap reads what FL's reads. This
  costs 72 seconds of stereo f32 a plugin (about 28 MB at 48 kHz), as in
  FL. Sizing from the slowest tempo the host could promise in a cold path
  would save memory, but needs a promise the `Effect` API does not make
  and would leave the taps past the newest frame reading other audio than
  FL's. Below 10 BPM, which FL does not reach, the reader stops at the
  ring's oldest frames.
- The kernel follows FL's f32 recurrences and order with an exact
  reciprocal in place of `rcpps` and Newton's step.
- `exp` in the tension curve is full precision; FL's `ippsExp_32f_A11` is
  good to 11 bits. `fidelity()` says so when the tension is not 0.
- Slot changes from MIDI, the keyboard and the GUI buttons, and key-held
  returns, have no source here.
